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
    pub(super) presentation_feedbacks: Vec<PendingPresentationFeedback>,
    pub(super) shm_buffer_releases: Vec<wl_buffer::WlBuffer>,
    pub(super) dmabuf_releases_to_complete_on_present: Vec<SurfaceBufferRelease>,
}
