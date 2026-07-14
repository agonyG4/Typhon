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
    output_render_fence_token: Option<ReactorToken>,
    frame_scheduler: NativeFrameScheduler,
    presentation_deadline: PresentationDeadlinePlanner,
    scheduled_presentation_target: Option<PresentationTarget>,
    render_journal: AdaptiveRenderJournal,
    adaptive_buffering: AdaptiveBufferingController,
    triple_buffer_policy: AdaptiveTripleBufferPolicy,
    pending_proven_deadline_miss: Option<ProvenDeadlineMiss>,
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
    frame_pacing: NativeFramePacing,
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
        if self.frame_pacing.enabled() {
            println!(
                "{}",
                self.frame_pacing
                    .summary_line(self.server.verbose_trace_dropped_entries())
            );
            if let Some(counters) = self.scanout.explicit_output_counters() {
                println!(
                    "typhon pacing: event=explicit_output_summary sync_file_deadline_hints_applied={} sync_file_deadline_hints_unsupported={} sync_file_deadline_hints_failed={} atomic_in_fence_submissions={} atomic_out_fences_received={} atomic_out_fence_missing={} render_fence_timing_unavailable={}",
                    counters.sync_file_deadline_hints_applied,
                    counters.sync_file_deadline_hints_unsupported,
                    counters.sync_file_deadline_hints_failed,
                    counters.atomic_in_fence_submissions,
                    counters.atomic_out_fences_received,
                    counters.atomic_out_fence_missing,
                    counters.render_fence_timing_unavailable,
                );
            }
        }
        if !self.session.permits_output() {
            self.scanout.disarm_drm_cleanup();
            self.kms_backend.disarm_drm_io();
            if let Some(cursor) = self.hardware_cursor.as_mut() {
                cursor.disarm_drm_cleanup();
            }
        } else if let Err(error) = self.kms_backend.restore() {
            eprintln!("native KMS restore before client-buffer drain failed: {error}");
        }
        // SAFETY: scanout is wrapped solely so inactive managed-session
        // teardown can disarm DRM cleanup before its normal resource drop.
        // Active KMS ownership was restored above; it is dropped exactly once
        // here, while `kms` is still alive.
        unsafe { mem::ManuallyDrop::drop(&mut self.scanout) };

        // Client buffers are released only after KMS ownership has ended and
        // the EGL/GBM renderer has been torn down, so shutdown cannot reuse a
        // buffer while KMS or GLES still owns it. The server drop repeats this
        // idempotently.
        self.server.finish_commit_debug_for_shutdown();
        let buffer_release_metrics = self.server.buffer_release_metrics();
        println!(
            "typhon pacing: event=buffer_release_summary buffer_releases_captured={} buffer_releases_completed={} buffer_releases_deferred={} buffer_releases_restored={} buffer_releases_discarded={} buffer_release_duplicate_attempts={}",
            buffer_release_metrics.buffer_releases_captured,
            buffer_release_metrics.buffer_releases_completed,
            buffer_release_metrics.buffer_releases_deferred,
            buffer_release_metrics.buffer_releases_restored,
            buffer_release_metrics.buffer_releases_discarded,
            buffer_release_metrics.buffer_release_duplicate_attempts,
        );
    }
}
