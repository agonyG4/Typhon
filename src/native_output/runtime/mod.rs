use super::*;

mod bootstrap;
mod cycle;
mod frame;
mod presentation;
mod session;
mod session_io;
mod shutdown;

pub(crate) use cycle::run;
pub(crate) use frame::{
    NativeCursorPreference, NativeCursorRenderMode, NativeFrameRenderer,
    NativePointerConstraintBackend, earliest_native_deadline, native_pointer_debug_log,
    normalize_refresh_hz,
};
#[cfg(test)]
pub(crate) use frame::{
    NativeFrameRequest, NativePointerConstraint, NativePointerConstraintBackendAction,
    NativeRepaintDecision, NativeRepaintInputs, native_repaint_decision,
};
pub(crate) use session::{NativeSessionLifecycle, NativeSessionTransition};
#[cfg(test)]
pub(crate) use session_io::NativeIoRecorder;
pub(crate) use session_io::{
    NativeIoOperation, NativeSessionIo, NativeSuspendedReadiness, quiesce_and_acknowledge,
    recover_native_output, service_suspended_sources, teardown_without_drm_io,
};
pub(crate) use shutdown::{
    NativeShutdownLifecycle, ShutdownState, ShutdownTransition, native_shutdown_debug_log,
};

pub(super) struct NativeCycleState {
    pub(super) wakeup: NativeWakeup,
    pub(super) pageflip_drain_us: u64,
    pub(super) pageflip_completed: bool,
    pub(super) completed_pageflip_token: Option<u64>,
    pub(super) frame_completed: bool,
    pub(super) frame_rendered: bool,
    pub(super) frame_submitted: bool,
    pub(super) present_us: u64,
    pub(super) pageflip_pending_at_tick: bool,
    pub(super) tick_us: u64,
    pub(super) accepted: usize,
    pub(super) redraw_requested: bool,
    pub(super) skipped_input_repaints: usize,
    pub(super) input_drain_us: u64,
    pub(super) raw_input_events: usize,
    pub(super) coalesced_input_events: usize,
    pub(super) shutdown_requested: bool,
}

pub(crate) struct NativeRuntimeConfig {
    pub(crate) server: OwnCompositorServer,
    pub(crate) app: Vec<String>,
    pub(crate) app_gpu_preference: CompositorAppGpuPreference,
}

pub(crate) struct NativeRuntime {
    server: OwnCompositorServer,
    perf: NativePerfLogger,
    target: KmsTarget,
    mode_label: String,
    refresh_hz: u32,
    drm_file_generation: u64,
    drm_timestamp_clock: DrmTimestampClock,
    presentation_clock: PresentationClock,
    scanout: mem::ManuallyDrop<NativeScanoutBackend>,
    kms_backend: KmsBackendSelection,
    frame_renderer: NativeFrameRenderer,
    input_state: NativeInputState,
    cursor_preference: NativeCursorPreference,
    cursor_render_mode: NativeCursorRenderMode,
    hardware_cursor: Option<NativeHardwareCursor>,
    kms: NativeDrmDevice,
    input_devices: NativeInputBackend,
    seat_session: Option<NativeSeatSession>,
    session: NativeSessionLifecycle,
    pending_session_recovery: Option<NativeScanoutRecovery>,
    #[cfg(test)]
    native_io_recorder: NativeIoRecorder,
    acquire_notifier: DrmAcquirePointNotifier,
    acquire_watches: ExplicitSyncWatchRegistry,
    parked_acquire_watches: Vec<oblivion_one::compositor::AcquireWatchRequest>,
    event_loop: NativeEventLoop,
    drm_reactor_token: Option<ReactorToken>,
    frame_scheduler: NativeFrameScheduler,
    effective_app_gpu_policy: EffectiveCompositorAppGpuPolicy,
    last_render_generation: u64,
    last_renderable_surfaces: Vec<RenderableSurface>,
    queued_redraw_requested: bool,
    frame_index: u64,
    known_toplevels: usize,
    pending_launches: VecDeque<NativeAppLaunchPerf>,
    mismatched_pageflip_events: u64,
    stale_pageflip_events: u64,
    presentation_cadence: PresentationCadenceMetrics,
    last_acquire_ready_at_ns: Option<u64>,
    resize_perf: NativeResizePerfState,
    pointer_constraint_backend: NativePointerConstraintBackend,
    process_supervisor: ChildSupervisor,
    astrea_launch_tracker: AstreaLaunchLifecycleTracker,
    shutdown: NativeShutdownLifecycle,
}

impl NativeRuntime {
    pub(crate) fn bootstrap(config: NativeRuntimeConfig) -> NativeResult<Self> {
        Self::bootstrap_native(config)
    }

    pub(crate) fn run(&mut self) -> NativeResult<()> {
        self.run_native_cycle()
    }
}

impl Drop for NativeRuntime {
    fn drop(&mut self) {
        if !self.session.permits_output() {
            self.scanout.disarm_drm_cleanup();
            self.kms_backend.disarm_drm_io();
            if let Some(cursor) = self.hardware_cursor.as_mut() {
                cursor.disarm_drm_cleanup();
            }
        }
        // SAFETY: scanout is wrapped solely so inactive managed-session
        // teardown can disarm DRM cleanup before its normal resource drop.
        // It is dropped exactly once here, while `kms` is still alive.
        unsafe { mem::ManuallyDrop::drop(&mut self.scanout) };
    }
}
