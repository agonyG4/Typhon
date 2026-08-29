use std::num::NonZeroU64;

use wayland_server::protocol::wl_callback;

use super::{
    CommitTimingTargetClaim, DmabufReleaseObligation, FifoBarrierClaim,
    PendingPresentationFeedback, SurfaceDamagePresentation,
};

#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CompositorFrameBatchId(NonZeroU64);

impl CompositorFrameBatchId {
    #[doc(hidden)]
    pub const fn new(value: NonZeroU64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0.get()
    }

    pub(super) const fn for_shutdown() -> Self {
        Self(NonZeroU64::MIN)
    }
}

#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DmabufGpuReleaseLeaseId(NonZeroU64);

impl DmabufGpuReleaseLeaseId {
    pub const fn new(value: NonZeroU64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

#[derive(Debug)]
pub(crate) struct DmabufGpuReleaseLease {
    pub(super) source_batch_id: Option<CompositorFrameBatchId>,
    pub(super) obligations: Vec<DmabufReleaseObligation>,
}

#[doc(hidden)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BufferReleaseMetrics {
    pub buffer_releases_captured: u64,
    pub buffer_releases_completed: u64,
    pub buffer_releases_deferred: u64,
    pub buffer_releases_restored: u64,
    pub buffer_releases_discarded: u64,
    pub buffer_release_duplicate_attempts: u64,
}

#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameCallbackAdmission {
    Immediate,
    Ready,
    Direct,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrameCallbackPacingState {
    // The callback remains owned by this exact batch until a terminal below
    // proves that the corresponding output has crossed admission.
    Captured,
    RenderedAwaitingAdmission,
    Completed,
}

#[doc(hidden)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FrameCallbackMetrics {
    pub callbacks_requested: u64,
    pub callbacks_captured: u64,
    pub callbacks_marked_rendered: u64,
    pub callbacks_completed_after_immediate_admission: u64,
    pub callbacks_completed_after_ready_admission: u64,
    pub callbacks_deferred_ready: u64,
    pub callbacks_completed_at_presentation_fallback: u64,
    pub callbacks_retained_after_failed_admission: u64,
    pub callbacks_completed_after_abandonment: u64,
    pub callbacks_found_at_pageflip: u64,
    pub callbacks_in_discarded_rendered_batches: u64,
    pub last_callback_commit_ns: Option<u64>,
    pub last_callback_capture_batch_id: Option<u64>,
    pub last_callback_render_completed_ns: Option<u64>,
    pub last_callback_admission_ns: Option<u64>,
    pub last_callback_pageflip_ns: Option<u64>,
    pub last_callback_commit_to_render_ns: Option<u64>,
    pub last_callback_render_to_admission_ns: Option<u64>,
    pub last_callback_commit_to_admission_ns: Option<u64>,
    pub last_callback_admission_to_next_commit_ns: Option<u64>,
    pub last_callback_render_to_pageflip_ns: Option<u64>,
    pub callback_render_to_admission_us: u64,
    pub callback_commit_to_admission_us: u64,
    pub callback_admission_to_next_commit_us: u64,
}

#[derive(Debug)]
pub(crate) struct CompositorFrameBatch {
    pub(super) frame_id: u64,
    pub(super) callbacks: Vec<wl_callback::WlCallback>,
    pub(super) callback_commit_ns: Option<u64>,
    pub(super) callback_render_completed_ns: Option<u64>,
    pub(super) callback_admission_ns: Option<u64>,
    pub(super) callback_pacing_state: FrameCallbackPacingState,
    pub(super) callback_settlement: FrameCallbackSettlement,
    pub(super) callback_terminal_ownership_checked: bool,
    pub(super) presentation_feedbacks: Vec<PendingPresentationFeedback>,
    pub(super) dmabuf_releases_to_complete_on_present: Vec<DmabufReleaseObligation>,
    pub(super) fifo_barrier_claims: Vec<FifoBarrierClaim>,
    pub(super) commit_timing_target_claims: Vec<CommitTimingTargetClaim>,
    pub(super) surface_damage: Option<SurfaceDamagePresentation>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct FrameCallbackSettlement {
    pub(crate) originally_owned: usize,
    pub(crate) completed_after_admission: usize,
    pub(crate) completed_at_presentation_fallback: usize,
    pub(crate) completed_without_visual: usize,
    pub(crate) transferred: usize,
    pub(crate) cancelled: usize,
    pub(crate) unresolved: usize,
    pub(crate) count_mismatch: bool,
}

impl FrameCallbackSettlement {
    pub(crate) const fn new(originally_owned: usize) -> Self {
        Self {
            originally_owned,
            completed_after_admission: 0,
            completed_at_presentation_fallback: 0,
            completed_without_visual: 0,
            transferred: 0,
            cancelled: 0,
            unresolved: originally_owned,
            count_mismatch: false,
        }
    }

    pub(crate) fn complete_after_admission(&mut self, count: usize) {
        if !self.consume_unresolved(count) {
            self.count_mismatch = true;
            return;
        }
        self.completed_after_admission = self.completed_after_admission.saturating_add(count);
    }

    pub(crate) fn complete_at_presentation_fallback(&mut self, count: usize) {
        if !self.consume_unresolved(count) {
            self.count_mismatch = true;
            return;
        }
        self.completed_at_presentation_fallback = self
            .completed_at_presentation_fallback
            .saturating_add(count);
    }

    pub(crate) fn complete_without_visual(&mut self, count: usize) {
        if !self.consume_unresolved(count) {
            self.count_mismatch = true;
            return;
        }
        self.completed_without_visual = self.completed_without_visual.saturating_add(count);
    }

    pub(crate) const fn completed(&self) -> usize {
        self.completed_after_admission
            .saturating_add(self.completed_at_presentation_fallback)
            .saturating_add(self.completed_without_visual)
    }

    pub(crate) fn transfer(&mut self, count: usize) {
        if !self.consume_unresolved(count) {
            self.count_mismatch = true;
            return;
        }
        self.transferred = self.transferred.saturating_add(count);
    }

    pub(crate) fn cancel(&mut self, count: usize) {
        if !self.consume_unresolved(count) {
            self.count_mismatch = true;
            return;
        }
        self.cancelled = self.cancelled.saturating_add(count);
    }

    pub(crate) fn is_reconciled(&self) -> bool {
        let Some(accounted) = self
            .completed_after_admission
            .checked_add(self.completed_at_presentation_fallback)
            .and_then(|count| count.checked_add(self.completed_without_visual))
            .and_then(|count| count.checked_add(self.transferred))
            .and_then(|count| count.checked_add(self.cancelled))
            .and_then(|count| count.checked_add(self.unresolved))
        else {
            return false;
        };
        self.originally_owned == accounted
    }

    fn consume_unresolved(&mut self, count: usize) -> bool {
        let Some(unresolved) = self.unresolved.checked_sub(count) else {
            return false;
        };
        self.unresolved = unresolved;
        true
    }
}
