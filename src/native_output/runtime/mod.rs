use super::*;

mod atomic_commit;
mod bootstrap;
mod cursor_cycle;
mod cycle;
mod cycle_dispatch;
mod frame;
mod metrics;
mod planner;
mod presentation;
mod presentation_direct;
mod presentation_protocol;
mod session;
mod session_io;
mod shutdown;
mod shutdown_cycle;
mod xwayland;
mod xwayland_reactor;
#[cfg(test)]
mod xwayland_reactor_tests;

pub(super) use atomic_commit::validate_atomic_pageflip;
pub(super) use atomic_commit::{
    AtomicCommitArbiter, AtomicCommitCompletion, AtomicCommitKind,
    register_atomic_primary_submission,
};
pub(super) use cursor_cycle::{
    atomic_cursor_visibility_policy, effective_atomic_cursor_state, log_client_cursor_path,
    resolve_client_cursor_path, synchronize_cursor_state_for_server,
};
pub(crate) use cycle::run;
#[cfg(test)]
pub(crate) use frame::NativeCursorOutputDisposition;
#[cfg(test)]
pub(crate) use frame::update_cursor_output_arbitration;
pub(crate) use frame::{
    NativeCursorOutputArbitration, NativeCursorPreference, NativeCursorRenderMode,
    NativeCursorSchedulingPolicy, NativeFrameRenderer, NativePointerConstraintBackend,
    earliest_native_deadline, native_pointer_debug_log, normalize_refresh_hz,
};
#[cfg(test)]
pub(crate) use frame::{
    NativeFrameRequest, NativePointerConstraint, NativePointerConstraintBackendAction,
    NativeRepaintDecision, NativeRepaintInputs, native_repaint_decision,
};
pub(super) use planner::{
    NativeCursorOwnerPlan, NativeKmsStartupDecision, decide_native_cursor_owner,
    decide_native_kms_startup,
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
pub(super) use xwayland_reactor::sync_xwayland_reactor_sources;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeClientCursorPath {
    Hidden,
    Hardware,
    Software,
}

pub(crate) struct NativeRuntime {
    server: OwnCompositorServer,
    cursor_image: std::sync::Arc<oblivion_one::cursor_theme::CompositorCursorImage>,
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
    cursor_scheduling_policy: NativeCursorSchedulingPolicy,
    cursor_output_arbitration: NativeCursorOutputArbitration,
    direct_scanout_preference: NativeDirectScanoutPreference,
    direct_scanout_qualification: DirectScanoutQualificationState,
    cursor_render_mode: NativeCursorRenderMode,
    atomic_cursor: Option<NativeAtomicCursor>,
    legacy_cursor: Option<NativeLegacyHardwareCursor>,
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
    xwayland: XwaylandService,
    xwayland_reactor_tokens: Vec<(ReactorToken, XwaylandReactorRegistration)>,
    xwayland_client_identity: Option<oblivion_one::compositor::XwaylandClientIdentity>,
    drm_reactor_token: Option<ReactorToken>,
    output_render_fence_token: Option<ReactorToken>,
    frame_scheduler: NativeFrameScheduler,
    atomic_commit_arbiter: AtomicCommitArbiter,
    presentation_deadline: PresentationDeadlinePlanner,
    scheduled_presentation_target: Option<PresentationTarget>,
    render_journal: AdaptiveRenderJournal,
    adaptive_buffering: AdaptiveBufferingController,
    triple_buffer_policy: AdaptiveTripleBufferPolicy,
    pending_proven_deadline_miss: Option<ProvenDeadlineMiss>,
    effective_app_gpu_policy: EffectiveCompositorAppGpuPolicy,
    last_rendered_scene_generation: u64,
    last_direct_candidate_key: Option<DirectScanoutCandidateKey>,
    last_submitted_cursor_epoch: u64,
    last_primary_presented_at_ns: Option<u64>,
    last_renderable_surfaces: Vec<RenderableSurface>,
    last_client_cursor_damage: Option<NativeClientCursorDamageState>,
    last_software_cursor_damage: Option<NativeDamageRect>,
    last_client_cursor_path: Option<NativeClientCursorPath>,
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
    presentation_trace: PresentationTransactionTraceRing,
    presentation_trace_path: Option<std::path::PathBuf>,
    timing_scopes: std::collections::BTreeMap<&'static str, TimingSummary>,
}

impl NativeRuntime {
    pub(crate) fn bootstrap(config: NativeRuntimeConfig) -> NativeResult<Self> {
        Self::bootstrap_native(config)
    }

    pub(crate) fn run(&mut self) -> NativeResult<()> {
        match self.run_native_cycle() {
            Ok(()) => Ok(()),
            Err(error) => {
                let _ = self
                    .xwayland
                    .emergency_cleanup(&mut self.process_supervisor);
                let _ = self.sync_xwayland_reactor_sources();
                let _ = self.process_supervisor.kill_session_owned_now();
                Err(error)
            }
        }
    }

    fn sync_xwayland_reactor_sources(&mut self) -> NativeResult<()> {
        sync_xwayland_reactor_sources(
            &mut self.event_loop,
            &mut self.xwayland,
            &mut self.xwayland_reactor_tokens,
        )
    }

    fn attach_xwayland_private_client(&mut self) -> NativeResult<()> {
        let Some(generation) = self.xwayland.generation() else {
            return Ok(());
        };
        if self
            .xwayland_client_identity
            .as_ref()
            .is_some_and(|identity| identity.generation == generation)
        {
            return Ok(());
        }
        let Some(stream) = self.xwayland.take_private_wayland_client(generation) else {
            return Ok(());
        };
        let identity = self.server.insert_xwayland_client(stream, generation)?;
        self.xwayland
            .authorize_private_client(generation, identity.client_id.clone());
        self.xwayland_client_identity = Some(identity);
        Ok(())
    }

    fn revoke_xwayland_private_client(&mut self) {
        if let Some(identity) = self.xwayland_client_identity.take() {
            self.server.revoke_xwayland_generation(identity.generation);
        }
    }

    pub(super) fn note_timing_scope(&mut self, name: &'static str, elapsed: Duration) {
        self.timing_scopes
            .entry(name)
            .or_default()
            .record(elapsed.as_nanos().min(u128::from(u64::MAX)) as u64);
    }
}

impl Drop for NativeRuntime {
    fn drop(&mut self) {
        self.revoke_xwayland_private_client();
        let _ = self
            .xwayland
            .emergency_cleanup(&mut self.process_supervisor);
        let _ = self.sync_xwayland_reactor_sources();
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
            if let Some(cursor) = self.atomic_cursor.as_mut() {
                cursor.disarm_drm_cleanup();
            }
            if let Some(cursor) = self.legacy_cursor.as_mut() {
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
