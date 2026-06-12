use std::{
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use wayland_protocols::wp::{
    linux_drm_syncobj::v1::server::wp_linux_drm_syncobj_surface_v1,
    presentation_time::server::wp_presentation_feedback,
};
use wayland_server::{
    Resource, Weak,
    protocol::{wl_callback, wl_surface},
};

use crate::syncobj::DrmSyncobjTimeline;

use super::{PendingSurfaceBuffer, RenderableSurfaceDamage};

pub(super) const SYNCOBJ_MANAGER_ERROR_SURFACE_EXISTS: u32 = 0;
pub(super) const SYNCOBJ_MANAGER_ERROR_INVALID_TIMELINE: u32 = 1;
pub(super) const SYNCOBJ_SURFACE_ERROR_NO_SURFACE: u32 = 1;
pub(super) const SYNCOBJ_SURFACE_ERROR_UNSUPPORTED_BUFFER: u32 = 2;
pub(super) const SYNCOBJ_SURFACE_ERROR_NO_BUFFER: u32 = 3;
pub(super) const SYNCOBJ_SURFACE_ERROR_NO_ACQUIRE_POINT: u32 = 4;
pub(super) const SYNCOBJ_SURFACE_ERROR_NO_RELEASE_POINT: u32 = 5;
pub(super) const SYNCOBJ_SURFACE_ERROR_CONFLICTING_POINTS: u32 = 6;

#[derive(Debug, Clone)]
pub(super) struct ExplicitSyncPoint {
    pub(super) timeline: DrmSyncobjTimeline,
    pub(super) point: u64,
}

impl ExplicitSyncPoint {
    pub(super) fn new(timeline: DrmSyncobjTimeline, point_hi: u32, point_lo: u32) -> Self {
        Self {
            timeline,
            point: ((point_hi as u64) << 32) | u64::from(point_lo),
        }
    }

    pub(super) fn is_signaled(&self) -> bool {
        self.timeline.point_signaled(self.point).unwrap_or(false)
    }

    pub(super) fn signal(&self) {
        let _ = self.timeline.signal_point(self.point);
    }
}

impl PartialEq for ExplicitSyncPoint {
    fn eq(&self, other: &Self) -> bool {
        self.point == other.point && self.timeline.same_timeline(&other.timeline)
    }
}

impl Eq for ExplicitSyncPoint {}

#[derive(Debug)]
pub(super) struct PendingExplicitSyncCommit {
    pub(super) surface_id: u32,
    pub(super) pending: PendingSurfaceBuffer,
    pub(super) damage: RenderableSurfaceDamage,
    pub(super) frame_callbacks: Vec<wl_callback::WlCallback>,
    pub(super) acquire: ExplicitSyncPoint,
}

#[derive(Debug)]
pub(super) struct PendingPresentationFeedback {
    pub(super) surface_id: u32,
    pub(super) surface: wl_surface::WlSurface,
    pub(super) feedback: wp_presentation_feedback::WpPresentationFeedback,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct PresentationTimestamp {
    pub(super) tv_sec_hi: u32,
    pub(super) tv_sec_lo: u32,
    pub(super) tv_nsec: u32,
}

pub(super) fn presentation_timestamp() -> PresentationTimestamp {
    let mut timespec = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let result = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut timespec) };
    if result == 0 {
        let seconds = timespec.tv_sec.max(0) as u64;
        return PresentationTimestamp {
            tv_sec_hi: (seconds >> 32) as u32,
            tv_sec_lo: seconds as u32,
            tv_nsec: timespec.tv_nsec.clamp(0, 999_999_999) as u32,
        };
    }

    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    PresentationTimestamp {
        tv_sec_hi: (duration.as_secs() >> 32) as u32,
        tv_sec_lo: duration.as_secs() as u32,
        tv_nsec: duration.subsec_nanos(),
    }
}

#[derive(Debug)]
pub(super) struct SyncobjTimelineData {
    pub(super) timeline: DrmSyncobjTimeline,
}

#[derive(Debug)]
pub(super) struct SyncobjSurfaceState {
    surface: Weak<wl_surface::WlSurface>,
    resource: Mutex<Option<wp_linux_drm_syncobj_surface_v1::WpLinuxDrmSyncobjSurfaceV1>>,
    pending_acquire: Mutex<Option<ExplicitSyncPoint>>,
    pending_release: Mutex<Option<ExplicitSyncPoint>>,
}

impl SyncobjSurfaceState {
    pub(super) fn new(surface: Weak<wl_surface::WlSurface>) -> Self {
        Self {
            surface,
            resource: Mutex::new(None),
            pending_acquire: Mutex::new(None),
            pending_release: Mutex::new(None),
        }
    }

    pub(super) fn set_resource(
        &self,
        resource: wp_linux_drm_syncobj_surface_v1::WpLinuxDrmSyncobjSurfaceV1,
    ) {
        if let Ok(mut guard) = self.resource.lock() {
            *guard = Some(resource);
        }
    }

    pub(super) fn resource_is_alive(&self) -> bool {
        self.resource
            .lock()
            .ok()
            .and_then(|resource| resource.as_ref().cloned())
            .is_some_and(|resource| resource.is_alive())
    }

    pub(super) fn surface_is_alive(&self) -> bool {
        self.surface.is_alive()
    }

    pub(super) fn clear_resource(&self) {
        if let Ok(mut guard) = self.resource.lock() {
            *guard = None;
        }
    }

    pub(super) fn post_error(&self, code: u32, message: &str) {
        if let Ok(guard) = self.resource.lock()
            && let Some(resource) = guard.as_ref()
        {
            resource.post_error(code, message);
        }
    }

    pub(super) fn set_pending_acquire(&self, point: ExplicitSyncPoint) {
        if let Ok(mut guard) = self.pending_acquire.lock() {
            *guard = Some(point);
        }
    }

    pub(super) fn set_pending_release(&self, point: ExplicitSyncPoint) {
        if let Ok(mut guard) = self.pending_release.lock() {
            *guard = Some(point);
        }
    }

    pub(super) fn take_points(&self) -> (Option<ExplicitSyncPoint>, Option<ExplicitSyncPoint>) {
        let acquire = self
            .pending_acquire
            .lock()
            .ok()
            .and_then(|mut guard| guard.take());
        let release = self
            .pending_release
            .lock()
            .ok()
            .and_then(|mut guard| guard.take());
        (acquire, release)
    }
}
