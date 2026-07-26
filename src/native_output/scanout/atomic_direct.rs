use std::{os::fd::OwnedFd, sync::Arc};

use oblivion_one::compositor::{
    CompositorFrameBatchId, DirectScanoutSceneCandidate, DirectScanoutSceneRejection,
    SurfaceDamagePresentation,
};
use oblivion_one::render_backend::buffer::DmabufBufferHandle;

use super::*;

#[derive(Debug, Clone)]
pub(crate) struct PreparedDirectFrame {
    pub(crate) frame_id: u64,
    pub(crate) transaction_id: OutputTransactionId,
    pub(crate) key: DirectScanoutCandidateKey,
    pub(crate) surface_id: u32,
    pub(crate) buffer: DmabufBufferHandle,
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

#[derive(Debug)]
pub(crate) struct WorkerQueuedDirectFrame {
    pub(crate) frame_id: u64,
    pub(crate) transaction_id: OutputTransactionId,
    pub(crate) key: DirectScanoutCandidateKey,
    pub(crate) surface_id: u32,
    pub(crate) token: PageFlipToken,
    pub(crate) protocol_batch_id: CompositorFrameBatchId,
    pub(crate) framebuffer_id: u32,
    pub(crate) target: PresentationTarget,
}

#[derive(Debug, Clone)]
pub(crate) struct PresentedDirectFrame {
    pub(crate) prepared: PreparedDirectFrame,
    pub(crate) token: PageFlipToken,
    pub(crate) presented_at: MonotonicTimestampNs,
    pub(crate) submit_started_at: MonotonicTimestampNs,
    pub(crate) submit_returned_at: MonotonicTimestampNs,
}

#[derive(Debug, Clone)]
pub(crate) struct DirectPageflipCompletion {
    pub(crate) presented: PresentedDirectFrame,
    pub(crate) protocol_batch_id: CompositorFrameBatchId,
    pub(crate) surface_damage: SurfaceDamagePresentation,
}

struct SuspendedDirectFrame {
    buffer: DmabufBufferHandle,
    framebuffer: Arc<ImportedDirectFramebuffer>,
    abandoned_batch: Option<(CompositorFrameBatchId, SurfaceDamagePresentation)>,
}

#[derive(Debug)]
pub(crate) enum DirectScanoutAttempt {
    Rejected(DirectScanoutSceneRejection),
    Fallback(&'static str),
    Unchanged,
    Submitted {
        transaction_id: OutputTransactionId,
        token: u64,
        framebuffer_id: u32,
    },
    WorkerQueued {
        transaction_id: OutputTransactionId,
        token: u64,
        framebuffer_id: u32,
        lease: DirectPrimaryLease,
        admission: crate::native_output::kms_worker::KmsCommitAdmissionPermit,
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
    pub(crate) worker_queued: Option<WorkerQueuedDirectFrame>,
    pub(crate) pending: Option<SubmittedDirectFrame>,
    suspended: Vec<SuspendedDirectFrame>,
    pub(crate) cache: DirectFramebufferCache,
    pub(crate) inhibit_until_composited_present: bool,
    pub(crate) counters: DirectScanoutCounters,
    pub(crate) drm_generation: u64,
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
            worker_queued: None,
            pending: None,
            suspended: Vec::new(),
            cache: DirectFramebufferCache::new(drm, generation),
            inhibit_until_composited_present: true,
            counters: DirectScanoutCounters::default(),
            drm_generation: generation,
            tested_plane_plan: None,
            identity_viewport_metadata_logged: false,
            last_debug_candidate: None,
        }
    }

    pub(crate) fn pending_token(&self) -> Option<PageFlipToken> {
        self.pending
            .as_ref()
            .map(|frame| frame.token)
            .or_else(|| self.worker_queued.as_ref().map(|frame| frame.token))
    }

    pub(crate) fn pending_transaction_id(&self) -> Option<OutputTransactionId> {
        self.pending
            .as_ref()
            .map(|frame| frame.prepared.transaction_id)
            .or_else(|| {
                self.worker_queued
                    .as_ref()
                    .map(|frame| frame.transaction_id)
            })
    }

    pub(crate) fn suspend_worker_queued(&mut self, token: PageFlipToken) -> io::Result<()> {
        let Some(frame) = self.worker_queued.take() else {
            return Ok(());
        };
        if frame.token != token {
            self.worker_queued = Some(frame);
            return Err(io::Error::other(
                "suspended direct worker token does not match queued ownership",
            ));
        }
        Ok(())
    }

    pub(crate) fn suspend_worker_submission(
        &mut self,
        token: PageFlipToken,
        lease: DirectPrimaryLease,
    ) -> io::Result<()> {
        let Some(frame) = self.worker_queued.take() else {
            return Err(io::Error::other(
                "suspended direct worker token has no queued metadata",
            ));
        };
        if frame.token != token {
            self.worker_queued = Some(frame);
            return Err(io::Error::other(
                "suspended direct worker token does not match queued ownership",
            ));
        }
        let (_, _, buffer, framebuffer, surface_damage) = lease.into_parts()?;
        self.suspended.push(SuspendedDirectFrame {
            buffer,
            framebuffer,
            abandoned_batch: Some((frame.protocol_batch_id, surface_damage)),
        });
        Ok(())
    }

    pub(crate) fn worker_queued_token(&self) -> Option<PageFlipToken> {
        self.worker_queued.as_ref().map(|frame| frame.token)
    }

    pub(crate) fn store_worker_queued(&mut self, frame: WorkerQueuedDirectFrame) -> io::Result<()> {
        if self.worker_queued.is_some() || self.pending.is_some() {
            return Err(io::Error::other(
                "direct worker queue already owns a primary frame",
            ));
        }
        self.worker_queued = Some(frame);
        Ok(())
    }

    pub(crate) fn page_flip_pending(&self) -> bool {
        self.pending.is_some() || self.worker_queued.is_some()
    }

    pub(crate) fn promote_worker_submission(
        &mut self,
        token: PageFlipToken,
        lease: DirectPrimaryLease,
        out_fence: Option<OwnedFd>,
        submit_started_at: MonotonicTimestampNs,
        submit_returned_at: MonotonicTimestampNs,
    ) -> io::Result<CompositorFrameBatchId> {
        let queued = self
            .worker_queued
            .take()
            .ok_or_else(|| io::Error::other("direct worker success has no queued frame"))?;
        if queued.token != token {
            self.worker_queued = Some(queued);
            return Err(io::Error::other(
                "direct worker success token mismatches queued frame",
            ));
        }
        let protocol_batch_id = queued.protocol_batch_id;
        let (key, surface_id, buffer, framebuffer, surface_damage) = lease.into_parts()?;
        if key != queued.key || surface_id != queued.surface_id {
            return Err(io::Error::other(
                "direct worker success lease does not match queued metadata",
            ));
        }
        self.pending = Some(SubmittedDirectFrame {
            prepared: PreparedDirectFrame {
                frame_id: queued.frame_id,
                transaction_id: queued.transaction_id,
                key,
                surface_id,
                buffer,
                framebuffer,
                target: queued.target,
            },
            token,
            protocol_batch_id,
            surface_damage,
            submit_started_at,
            submit_returned_at,
            out_fence,
        });
        Ok(protocol_batch_id)
    }

    pub(crate) fn fail_worker_submission(
        &mut self,
        token: PageFlipToken,
    ) -> io::Result<CompositorFrameBatchId> {
        let queued = self
            .worker_queued
            .take()
            .ok_or_else(|| io::Error::other("direct worker failure has no queued frame"))?;
        if queued.token != token {
            self.worker_queued = Some(queued);
            return Err(io::Error::other(
                "direct worker failure token mismatches queued frame",
            ));
        }
        Ok(queued.protocol_batch_id)
    }

    pub(crate) fn active_surface(&self) -> Option<u32> {
        self.pending
            .as_ref()
            .map(|frame| frame.prepared.surface_id)
            .or_else(|| self.current.as_ref().map(|frame| frame.prepared.surface_id))
            .or_else(|| self.worker_queued.as_ref().map(|frame| frame.surface_id))
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

    pub(super) fn complete_suspended(&mut self) {
        for frame in self.suspended.drain(..) {
            if let Some((_batch_id, surface_damage)) = frame.abandoned_batch {
                drop(surface_damage);
            }
            drop(frame.framebuffer);
            drop(frame.buffer);
        }
    }

    pub(super) fn suspend(&mut self) {
        self.worker_queued.take();
        if let Some(frame) = self.pending.take() {
            self.suspended.push(SuspendedDirectFrame {
                buffer: frame.prepared.buffer,
                framebuffer: frame.prepared.framebuffer,
                abandoned_batch: Some((frame.protocol_batch_id, frame.surface_damage)),
            });
        }
        if let Some(frame) = self.current.take() {
            self.counters.exits += 1;
            self.suspended.push(SuspendedDirectFrame {
                buffer: frame.prepared.buffer,
                framebuffer: frame.prepared.framebuffer,
                abandoned_batch: None,
            });
        }
        self.inhibit_until_composited_present = true;
    }
}
