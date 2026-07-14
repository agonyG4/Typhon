use std::num::NonZeroU64;

use wayland_server::protocol::{wl_buffer, wl_callback};

use super::{PendingPresentationFeedback, SurfaceBufferRelease};

#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CompositorFrameBatchId(NonZeroU64);

impl CompositorFrameBatchId {
    pub(super) const fn new(value: NonZeroU64) -> Self {
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

#[derive(Debug)]
pub(crate) struct CompositorFrameBatch {
    pub(super) frame_id: u64,
    pub(super) callbacks: Vec<wl_callback::WlCallback>,
    pub(super) presentation_feedbacks: Vec<PendingPresentationFeedback>,
    pub(super) shm_buffer_releases: Vec<wl_buffer::WlBuffer>,
    pub(super) dmabuf_releases_to_complete_on_present: Vec<SurfaceBufferRelease>,
}
