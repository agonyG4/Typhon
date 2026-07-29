use std::num::NonZeroU64;

use wayland_server::protocol::{wl_buffer, wl_callback};

use super::{PendingPresentationFeedback, SurfaceBufferRelease};

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
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FrameCallbackMetrics {
    pub callbacks_requested: u64,
    pub callbacks_captured: u64,
    pub callbacks_completed_after_render: u64,
    pub callbacks_completed_after_abandonment: u64,
    pub callbacks_found_at_pageflip: u64,
    pub callbacks_in_discarded_rendered_batches: u64,
    pub last_callback_commit_ns: Option<u64>,
    pub last_callback_capture_batch_id: Option<u64>,
    pub last_callback_render_completed_ns: Option<u64>,
    pub last_callback_pageflip_ns: Option<u64>,
    pub last_callback_commit_to_render_ns: Option<u64>,
    pub last_callback_render_to_pageflip_ns: Option<u64>,
}

#[derive(Debug)]
pub(crate) struct CompositorFrameBatch {
    pub(super) frame_id: u64,
    pub(super) callbacks: Vec<wl_callback::WlCallback>,
    pub(super) callback_commit_ns: Option<u64>,
    pub(super) callback_render_completed_ns: Option<u64>,
    pub(super) callback_settlement: FrameCallbackSettlement,
    pub(super) callback_terminal_ownership_checked: bool,
    pub(super) presentation_feedbacks: Vec<PendingPresentationFeedback>,
    pub(super) shm_buffer_releases: Vec<wl_buffer::WlBuffer>,
    pub(super) dmabuf_releases_to_complete_on_present: Vec<SurfaceBufferRelease>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct FrameCallbackSettlement {
    pub(crate) originally_owned: usize,
    pub(crate) completed_after_render: usize,
    pub(crate) transferred: usize,
    pub(crate) cancelled: usize,
    pub(crate) unresolved: usize,
    pub(crate) count_mismatch: bool,
}

impl FrameCallbackSettlement {
    pub(crate) const fn new(originally_owned: usize) -> Self {
        Self {
            originally_owned,
            completed_after_render: 0,
            transferred: 0,
            cancelled: 0,
            unresolved: originally_owned,
            count_mismatch: false,
        }
    }

    pub(crate) fn complete(&mut self, count: usize) {
        if !self.consume_unresolved(count) {
            self.count_mismatch = true;
            return;
        }
        self.completed_after_render = self.completed_after_render.saturating_add(count);
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
            .completed_after_render
            .checked_add(self.transferred)
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
