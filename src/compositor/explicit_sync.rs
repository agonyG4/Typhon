use std::sync::Mutex;
use std::{num::NonZeroU64, time::Instant};

#[cfg(test)]
use std::{collections::VecDeque, sync::Arc};

use wayland_protocols::wp::{
    linux_drm_syncobj::v1::server::wp_linux_drm_syncobj_surface_v1,
    presentation_time::server::wp_presentation_feedback,
};
use wayland_server::{
    Resource, Weak,
    protocol::{wl_callback, wl_surface},
};

use crate::syncobj::DrmSyncobjTimeline;

use super::{
    CoreComplianceMetrics, PendingSurfaceBuffer, ProtocolErrorCategory, ProtocolErrorInterface,
    ProtocolErrorRecord, ProtocolErrorTrace, RenderableSurfaceDamage, SurfaceCommitId,
    SurfaceCommitSequence, protocol_error_timestamp_ns,
};

pub(super) const SYNCOBJ_MANAGER_ERROR_SURFACE_EXISTS: u32 = 0;
pub(super) const SYNCOBJ_MANAGER_ERROR_INVALID_TIMELINE: u32 = 1;
pub(super) const SYNCOBJ_SURFACE_ERROR_NO_SURFACE: u32 = 1;
pub(super) const SYNCOBJ_SURFACE_ERROR_UNSUPPORTED_BUFFER: u32 = 2;
pub(super) const SYNCOBJ_SURFACE_ERROR_NO_BUFFER: u32 = 3;
pub(super) const SYNCOBJ_SURFACE_ERROR_NO_ACQUIRE_POINT: u32 = 4;
pub(super) const SYNCOBJ_SURFACE_ERROR_NO_RELEASE_POINT: u32 = 5;
pub(super) const SYNCOBJ_SURFACE_ERROR_CONFLICTING_POINTS: u32 = 6;

#[derive(Debug, Clone)]
pub struct ExplicitSyncPoint {
    pub timeline: DrmSyncobjTimeline,
    pub point: u64,
    #[cfg(test)]
    pub(crate) signal_script: Option<Arc<Mutex<VecDeque<bool>>>>,
}

impl ExplicitSyncPoint {
    pub(super) fn new(timeline: DrmSyncobjTimeline, point_hi: u32, point_lo: u32) -> Self {
        Self {
            timeline,
            point: ((point_hi as u64) << 32) | u64::from(point_lo),
            #[cfg(test)]
            signal_script: None,
        }
    }

    pub(crate) fn is_signaled(&self) -> bool {
        self.timeline.point_signaled(self.point).unwrap_or(false)
    }

    pub fn signaled_result(&self) -> std::io::Result<bool> {
        self.timeline.point_signaled(self.point)
    }

    #[cfg(test)]
    pub(crate) fn for_tests(handle: u32, point: u64) -> Self {
        Self::for_tests_with_signal_script(handle, point, [true])
    }

    #[cfg(test)]
    pub(crate) fn for_tests_with_signal_script(
        handle: u32,
        point: u64,
        outcomes: impl IntoIterator<Item = bool>,
    ) -> Self {
        Self {
            timeline: DrmSyncobjTimeline::invalid_for_tests(handle),
            point,
            signal_script: Some(Arc::new(Mutex::new(
                outcomes.into_iter().collect::<VecDeque<_>>(),
            ))),
        }
    }

    pub(super) fn signal(&self) -> std::io::Result<()> {
        #[cfg(test)]
        if let Some(script) = &self.signal_script {
            let mut script = script
                .lock()
                .map_err(|_| std::io::Error::other("explicit-sync test signal script poisoned"))?;
            if let Some(success) = script.pop_front() {
                return success
                    .then_some(())
                    .ok_or_else(|| std::io::Error::other("scripted explicit-sync signal failure"));
            }
            return Ok(());
        }
        self.timeline.signal_point(self.point)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AcquireCommitId(NonZeroU64);

impl AcquireCommitId {
    pub const fn get(self) -> u64 {
        self.0.get()
    }

    #[cfg(test)]
    pub(crate) fn for_tests(value: u64) -> Self {
        Self(NonZeroU64::new(value).expect("test acquire commit ID must be nonzero"))
    }
}

#[derive(Debug, Default)]
pub(super) struct AcquireCommitIdAllocator {
    last: u64,
}

impl AcquireCommitIdAllocator {
    pub(super) fn allocate(&mut self) -> Option<AcquireCommitId> {
        self.last = self.last.checked_add(1)?;
        NonZeroU64::new(self.last).map(AcquireCommitId)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PendingAcquireState {
    RegistrationPending,
    EventfdBacked,
    FallbackBacked,
    Ready,
}

impl PendingAcquireState {
    pub(super) fn mark_eventfd_backed(&mut self) -> bool {
        if *self != Self::RegistrationPending {
            return false;
        }
        *self = Self::EventfdBacked;
        true
    }

    pub(super) fn mark_fallback_backed(&mut self) -> bool {
        if *self != Self::RegistrationPending {
            return false;
        }
        *self = Self::FallbackBacked;
        true
    }

    pub(super) fn mark_ready(&mut self) -> bool {
        if *self == Self::Ready {
            return false;
        }
        *self = Self::Ready;
        true
    }
}

#[derive(Debug, Clone)]
pub struct AcquireWatchRequest {
    pub commit_id: AcquireCommitId,
    pub surface_id: u32,
    pub buffer_id: u32,
    pub acquire: ExplicitSyncPoint,
    pub received_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcquireWatchCancelReason {
    Superseded,
    SurfaceDestroyed,
    BufferDestroyed,
    // Retained for telemetry compatibility; protocol proxy destruction does not emit these.
    SyncSurfaceDestroyed,
    TimelineDestroyed,
    ClientDisconnected,
    BackendShutdown,
    Rejected,
    RoleDestroyed,
}

#[derive(Debug, Clone)]
pub enum AcquireWatchChange {
    Register(AcquireWatchRequest),
    Cancel {
        commit_id: AcquireCommitId,
        reason: AcquireWatchCancelReason,
    },
}

impl PartialEq for ExplicitSyncPoint {
    fn eq(&self, other: &Self) -> bool {
        self.point == other.point && self.timeline.same_timeline(&other.timeline)
    }
}

impl Eq for ExplicitSyncPoint {}

#[derive(Debug)]
pub(super) struct PendingExplicitSyncCommit {
    pub(super) surface_commit_id: SurfaceCommitId,
    pub(super) commit_id: AcquireCommitId,
    pub(super) surface_id: u32,
    pub(super) commit_sequence: SurfaceCommitSequence,
    pub(super) pending: PendingSurfaceBuffer,
    pub(super) damage: RenderableSurfaceDamage,
    pub(super) window_geometry: Option<super::XdgWindowGeometry>,
    pub(super) frame_callbacks: Vec<wl_callback::WlCallback>,
    pub(super) presentation_feedbacks: Vec<PendingPresentationFeedback>,
    pub(super) acquire: ExplicitSyncPoint,
    pub(super) acquire_state: PendingAcquireState,
}

#[derive(Debug)]
pub(super) struct CapturedExplicitSyncState {
    pub(super) state: std::sync::Arc<SyncobjSurfaceState>,
    pub(super) acquire: Option<ExplicitSyncPoint>,
    pub(super) release: Option<ExplicitSyncPoint>,
}

impl CapturedExplicitSyncState {
    pub(super) fn capture(state: std::sync::Arc<SyncobjSurfaceState>) -> Self {
        let (acquire, release) = state.take_points();
        Self {
            state,
            acquire,
            release,
        }
    }

    pub(super) fn has_points(&self) -> bool {
        self.acquire.is_some() || self.release.is_some()
    }
}

#[derive(Debug)]
pub(super) struct RequestedPresentationFeedback {
    pub(super) surface: wl_surface::WlSurface,
    pub(super) feedback: wp_presentation_feedback::WpPresentationFeedback,
}

#[derive(Debug)]
pub(super) struct PendingPresentationFeedback {
    pub(super) surface_id: u32,
    pub(super) surface_presentation_generation: u64,
    pub(super) commit_sequence: SurfaceCommitSequence,
    pub(super) surface: wl_surface::WlSurface,
    pub(super) feedback: wp_presentation_feedback::WpPresentationFeedback,
}

#[derive(Debug)]
pub(super) struct SyncobjTimelineData {
    pub(super) timeline: DrmSyncobjTimeline,
}

#[derive(Debug)]
pub(super) struct SyncobjSurfaceState {
    surface_id: u32,
    surface: Weak<wl_surface::WlSurface>,
    resource: Mutex<Option<wp_linux_drm_syncobj_surface_v1::WpLinuxDrmSyncobjSurfaceV1>>,
    pending_acquire: Mutex<Option<ExplicitSyncPoint>>,
    pending_release: Mutex<Option<ExplicitSyncPoint>>,
}

impl SyncobjSurfaceState {
    pub(super) fn new(surface_id: u32, surface: Weak<wl_surface::WlSurface>) -> Self {
        Self {
            surface_id,
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

    pub(super) const fn surface_id(&self) -> u32 {
        self.surface_id
    }

    pub(super) fn clear_resource(&self) {
        if let Ok(mut guard) = self.resource.lock() {
            *guard = None;
        }
    }

    pub(super) fn destroy_protocol_binding(&self) {
        self.clear_resource();
        if let Ok(mut guard) = self.pending_acquire.lock() {
            *guard = None;
        }
        if let Ok(mut guard) = self.pending_release.lock() {
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

    pub(super) fn post_error_with_metrics(
        &self,
        metrics: &mut CoreComplianceMetrics,
        trace: &mut ProtocolErrorTrace,
        code: u32,
        message: &str,
    ) {
        metrics.note_protocol_error();
        let (client_id, resource_id) = self
            .resource
            .lock()
            .ok()
            .and_then(|resource| resource.as_ref().cloned())
            .map_or((None, None), |resource| {
                (
                    resource.client().map(|client| client.id()),
                    Some(resource.id().protocol_id()),
                )
            });
        trace.record(ProtocolErrorRecord {
            timestamp_ns: protocol_error_timestamp_ns(),
            client_id,
            peer_pid: None,
            interface: ProtocolErrorInterface::Syncobj,
            resource_id,
            error_code: Some(code),
            surface_id: Some(self.surface_id),
            xwayland_generation: None,
            category: ProtocolErrorCategory::InvalidState,
        });
        self.post_error(code, message);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_commit_identity_is_nonzero_and_monotonic() {
        let mut allocator = AcquireCommitIdAllocator::default();

        assert_eq!(allocator.allocate().unwrap().get(), 1);
        assert_eq!(allocator.allocate().unwrap().get(), 2);
    }

    #[test]
    fn acquire_readiness_transitions_at_most_once() {
        let mut state = PendingAcquireState::RegistrationPending;

        assert!(state.mark_eventfd_backed());
        assert!(state.mark_ready());
        assert!(!state.mark_ready());
        assert_eq!(state, PendingAcquireState::Ready);
    }

    #[test]
    fn fallback_state_is_distinct_from_eventfd_state() {
        let mut state = PendingAcquireState::RegistrationPending;

        assert!(state.mark_fallback_backed());
        assert_eq!(state, PendingAcquireState::FallbackBacked);
        assert!(state.mark_ready());
    }

    #[test]
    fn signal_result_is_propagated() {
        let point = ExplicitSyncPoint::for_tests_with_signal_script(0, 0, [false]);

        assert!(point.signal().is_err());
    }

    #[test]
    fn default_test_signal_is_successful_without_a_drm_device() {
        let point = ExplicitSyncPoint::for_tests(0, 0);

        assert!(point.signal().is_ok());
    }
}
