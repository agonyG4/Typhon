use std::{os::fd::OwnedFd, sync::Arc};

use oblivion_one::compositor::{
    CompositorFrameBatchId, DirectScanoutSceneCandidate, DirectScanoutSceneRejection,
    FrameBatchDiscardReason, OwnCompositorServer, SurfaceDamagePresentation,
};
use oblivion_one::render_backend::buffer::DmabufBufferHandle;

use super::*;

#[derive(Debug, Clone)]
pub(crate) struct PreparedDirectFrame {
    pub(crate) frame_id: u64,
    pub(crate) transaction_id: PresentationTransactionId,
    pub(crate) key: DirectScanoutCandidateKey,
    pub(crate) candidate: DirectScanoutSceneCandidate,
    pub(crate) framebuffer: Arc<ImportedDirectFramebuffer>,
    pub(crate) target: PresentationTarget,
}

pub(crate) struct SubmittedDirectFrame {
    pub(crate) prepared: PreparedDirectFrame,
    pub(crate) token: PageFlipToken,
    pub(crate) protocol_batch_id: CompositorFrameBatchId,
    pub(crate) surface_damage: SurfaceDamagePresentation,
    pub(crate) submit_started_at: MonotonicTimestampNs,
    pub(crate) submit_returned_at: MonotonicTimestampNs,
    pub(crate) out_fence: Option<OwnedFd>,
}

#[derive(Debug, Clone)]
pub(crate) struct PresentedDirectFrame {
    pub(crate) prepared: PreparedDirectFrame,
    pub(crate) token: PageFlipToken,
    pub(crate) presented_at: MonotonicTimestampNs,
    pub(crate) submit_started_at: MonotonicTimestampNs,
    pub(crate) submit_returned_at: MonotonicTimestampNs,
}

struct SuspendedDirectFrame {
    buffer: DmabufBufferHandle,
    framebuffer: Arc<ImportedDirectFramebuffer>,
    abandoned_batch: Option<(CompositorFrameBatchId, SurfaceDamagePresentation)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectScanoutAttempt {
    Rejected(DirectScanoutSceneRejection),
    Fallback(&'static str),
    Unchanged,
    Submitted {
        transaction_id: PresentationTransactionId,
        token: u64,
    },
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct DirectScanoutCounters {
    pub(crate) candidate_checks: u64,
    pub(crate) candidates_accepted: u64,
    pub(crate) import_attempts: u64,
    pub(crate) import_cache_hits: u64,
    pub(crate) import_failures: u64,
    pub(crate) test_only_attempts: u64,
    pub(crate) test_only_rejections: u64,
    pub(crate) submissions: u64,
    pub(crate) presentations: u64,
    pub(crate) entries: u64,
    pub(crate) exits: u64,
    pub(crate) same_buffer_resubmissions: u64,
    pub(crate) same_buffer_suppressed: u64,
    pub(crate) out_fences_received: u64,
    pub(crate) out_fence_missing: u64,
    pub(crate) test_only_timing: TimingSummary,
    pub(crate) real_submit_timing: TimingSummary,
    pub(crate) composited_fallbacks: u64,
    pub(crate) stale_candidate_rejections: u64,
    pub(crate) cleanup_failures: u64,
    pub(crate) composited_render_ahead_suppressed: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct DirectPlanePlanKey {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) format: u32,
    pub(crate) modifier: u64,
    pub(crate) cursor_plan_key: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TestedDirectPlanePlan {
    pub(crate) key: DirectPlanePlanKey,
    pub(crate) drm_generation: u64,
}

pub(crate) struct DirectScanoutState {
    pub(crate) current: Option<PresentedDirectFrame>,
    pub(crate) pending: Option<SubmittedDirectFrame>,
    suspended: Vec<SuspendedDirectFrame>,
    pub(crate) cache: DirectFramebufferCache,
    pub(crate) inhibit_until_composited_present: bool,
    pub(crate) counters: DirectScanoutCounters,
    pub(crate) drm_generation: u64,
    pub(crate) transaction_ids: PresentationTransactionAllocator,
    pub(crate) tested_plane_plan: Option<TestedDirectPlanePlan>,
    pub(super) identity_viewport_metadata_logged: bool,
    pub(super) last_debug_candidate: Option<(u32, u64, u64, u64)>,
}

pub(super) fn direct_candidate_key(
    candidate: &DirectScanoutSceneCandidate,
    drm_generation: u64,
    cursor: Option<&AtomicCursorVisualState>,
) -> Option<DirectScanoutCandidateKey> {
    DirectScanoutCandidateKey::from_candidate(
        candidate,
        drm_generation,
        direct_cursor_plan_key(cursor, true),
        0,
    )
}

pub(super) fn direct_scanout_debug(message: impl std::fmt::Display) {
    if std::env::var("TYPHON_DIRECT_SCANOUT_DEBUG").ok().as_deref() == Some("1") {
        eprintln!("direct scanout: {message}");
    }
}

impl DirectScanoutState {
    pub(super) fn new(drm: std::os::fd::BorrowedFd<'_>, generation: u64) -> Self {
        Self {
            current: None,
            pending: None,
            suspended: Vec::new(),
            cache: DirectFramebufferCache::new(drm, generation),
            inhibit_until_composited_present: true,
            counters: DirectScanoutCounters::default(),
            drm_generation: generation,
            transaction_ids: PresentationTransactionAllocator::default(),
            tested_plane_plan: None,
            identity_viewport_metadata_logged: false,
            last_debug_candidate: None,
        }
    }

    pub(crate) fn pending_token(&self) -> Option<PageFlipToken> {
        self.pending.as_ref().map(|frame| frame.token)
    }

    pub(crate) fn page_flip_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub(crate) fn active_surface(&self) -> Option<u32> {
        self.pending
            .as_ref()
            .map(|frame| frame.prepared.candidate.surface_id)
            .or_else(|| {
                self.current
                    .as_ref()
                    .map(|frame| frame.prepared.candidate.surface_id)
            })
    }

    pub(crate) fn disarm_drm_cleanup(&mut self) {
        self.cache.clear_disarmed();
        if let Some(frame) = &self.current {
            frame.prepared.framebuffer.disarm_drm_cleanup();
        }
        if let Some(frame) = &self.pending {
            frame.prepared.framebuffer.disarm_drm_cleanup();
        }
        for frame in &self.suspended {
            frame.framebuffer.disarm_drm_cleanup();
        }
    }

    pub(super) fn complete_suspended(&mut self, server: &mut OwnCompositorServer) {
        for frame in self.suspended.drain(..) {
            if let Some((batch_id, surface_damage)) = frame.abandoned_batch {
                server.complete_frame_batch_after_safe_abandonment(
                    batch_id,
                    FrameBatchDiscardReason::SuspendAbandonment,
                );
                drop(surface_damage);
            }
            drop(frame.framebuffer);
            drop(frame.buffer);
        }
    }

    pub(super) fn suspend(&mut self) {
        if let Some(frame) = self.pending.take() {
            self.suspended.push(SuspendedDirectFrame {
                buffer: frame.prepared.candidate.buffer,
                framebuffer: frame.prepared.framebuffer,
                abandoned_batch: Some((frame.protocol_batch_id, frame.surface_damage)),
            });
        }
        if let Some(frame) = self.current.take() {
            self.counters.exits += 1;
            self.suspended.push(SuspendedDirectFrame {
                buffer: frame.prepared.candidate.buffer,
                framebuffer: frame.prepared.framebuffer,
                abandoned_batch: None,
            });
        }
        self.inhibit_until_composited_present = true;
    }
}
