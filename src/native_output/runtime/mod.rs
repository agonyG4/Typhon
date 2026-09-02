use super::*;
use oblivion_one::compositor::{DrmContentType, FrameBatchDiscardReason, OutputPresentationMode};

macro_rules! require_validation_base {
    ($context:expr, $redraw:ident) => {
        match $context {
            (worker, presented, generation, crtc) => {
                match validation_base_for_submission(worker, presented, generation, crtc) {
                    Some(base) => base,
                    None => {
                        *$redraw = true;
                        return Ok(());
                    }
                }
            }
        }
    };
}

mod atomic_commit;
mod bootstrap;
mod commit_timing;
mod cursor_cycle;
mod cycle;
mod cycle_dispatch;
mod direct_plan;
mod dmabuf_release;
mod frame;
mod input_transition_guard;
mod kms_worker;
mod kms_worker_startup;
mod kms_worker_teardown;
#[cfg(test)]
mod kms_worker_tests;
mod metrics;
mod plane_cycle;
#[cfg(test)]
mod plane_cycle_tests;
mod planner;
mod pointer_timing;
mod presentation;
mod presentation_cursor;
mod presentation_cycle;
#[cfg(test)]
mod presentation_cycle_tests;
mod presentation_direct;
mod presentation_metrics;
mod presentation_o1;
mod presentation_pipeline;
mod presentation_protocol;
mod presentation_ready;
mod presentation_transactions;
mod presentation_worker;
mod resource_efficiency;
mod scene_history;
mod session;
mod session_io;
mod shutdown;
mod shutdown_cycle;
mod wake_plan;
mod work_domains;
mod xwayland;
mod xwayland_reactor;
#[cfg(test)]
mod xwayland_reactor_tests;

pub(super) use dmabuf_release::arm_composited_dmabuf_release;
pub(crate) use dmabuf_release::{
    DmabufGpuReleaseMetrics, DmabufGpuReleaseOrigin, DmabufGpuReleaseQualificationSummary,
    DmabufGpuReleaseRegistry, DmabufGpuReleaseSafety, DmabufReleaseRetryReason,
    dmabuf_gpu_release_safety,
};
use metrics::NativeRenderTelemetry;
pub(crate) use pointer_timing::{
    NativePointerPreReadObservation, NativePointerTimingBatch, NativePointerTimingPhase,
    NativePointerTimingPoint, NativePointerTimingTrace, NativePointerTimingTransition,
    capture_timing_point,
};
pub(crate) use resource_efficiency::{
    NativeWorkClass, NativeWorkDecision, ResourceEfficiencyMetrics,
};
pub(crate) use scene_history::{NativeFrameSceneSnapshot, NativeSceneHistory};
pub(super) use wake_plan::{
    NativeDeadline, NativeDeadlineOwner, NativePageflipTimeoutOwner, NativeWakeAuthorityMetrics,
    NativeWakePlan, NativeWakePlanInputs, atomic_commit_watchdog_deadline_for_timeout_owner,
    build_native_wake_plan, scheduler_deadline_for_timeout_owner,
};
pub(super) use work_domains::{NativeRuntimeState, NativeWorkDomains};

pub(super) use atomic_commit::validate_atomic_pageflip;
pub(super) use atomic_commit::{
    AtomicCommitArbiter, AtomicCommitCompletion, AtomicCommitKind, AtomicCommitPhase,
    register_atomic_primary_submission,
};
pub(super) use cursor_cycle::{
    atomic_cursor_visibility_policy, effective_atomic_cursor_state, log_client_cursor_path,
    observe_atomic_cursor_output_liveness, resolve_client_cursor_path,
    synchronize_cursor_state_for_server,
};
pub(crate) use cycle::run;
use cycle_dispatch::NativeWaylandInputDispatchOutcome;
#[cfg(test)]
pub(crate) use frame::NativeCursorOutputDisposition;
#[cfg(test)]
pub(crate) use frame::earliest_native_deadline;
#[cfg(test)]
pub(crate) use frame::update_cursor_output_arbitration;
pub(crate) use frame::{
    NativeCursorOutputArbitration, NativeCursorPreference, NativeCursorRenderMode,
    NativeCursorSchedulingPolicy, NativeFrameRenderer, NativePointerConstraintBackend,
    NativePointerConstraintBackendAction, ResolvedNativeFrameScene, native_pointer_debug_log_lazy,
    normalize_refresh_hz,
};
#[cfg(test)]
pub(crate) use frame::{
    NativeFrameRequest, NativePointerConstraint, NativeRepaintDecision, NativeRepaintInputs,
    native_repaint_decision,
};
pub(super) use input_transition_guard::{
    NativeInputRoutingGuardCheckpoint, NativeInputTransitionLatencyGuard,
    NativeRoutingGuardDecision,
};
pub(super) use planner::{
    NativeCursorOwnerPlan, NativeKmsStartupDecision, decide_native_cursor_owner,
    decide_native_kms_startup,
};
#[cfg(test)]
pub(crate) use planner::{
    NativePresentationPath, NativePresentationPlanInput, plan_native_presentation_path,
};
pub(super) use presentation_pipeline::initial_presented;
pub(crate) use presentation_transactions::{
    DirectCallbackLeakMetrics, DirectTerminalCallbackDisposition,
    direct_terminal_callback_owner_leaks, settle_failed_output_transaction,
    settle_no_visual_change_output_transaction,
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
pub(super) use xwayland_reactor::{
    sync_xwayland_reactor_sources, sync_xwayland_reactor_sources_with_generation,
};

pub(super) struct NativeCycleState {
    pub(super) wakeup: NativeWakeup,
    pub(super) work_class: NativeWorkClass,
    pub(super) fast_path_completed: bool,
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

#[derive(Clone, Copy)]
pub(super) struct NativeCycleMicroturnBaseline {
    work_class: NativeWorkClass,
    fast_path_completed: bool,
    pageflip_drain_us: u64,
    pageflip_completed: bool,
    completed_pageflip_token: Option<u64>,
    frame_completed: bool,
    frame_rendered: bool,
    frame_submitted: bool,
    present_us: u64,
    pageflip_pending_at_tick: bool,
    accepted: usize,
    redraw_requested: bool,
    skipped_input_repaints: usize,
    input_drain_us: u64,
    raw_input_events: usize,
    coalesced_input_events: usize,
    shutdown_requested: bool,
    tick_us: u64,
}

impl NativeCycleState {
    pub(super) const fn microturn_baseline(&self) -> NativeCycleMicroturnBaseline {
        NativeCycleMicroturnBaseline {
            work_class: self.work_class,
            fast_path_completed: self.fast_path_completed,
            pageflip_drain_us: self.pageflip_drain_us,
            pageflip_completed: self.pageflip_completed,
            completed_pageflip_token: self.completed_pageflip_token,
            frame_completed: self.frame_completed,
            frame_rendered: self.frame_rendered,
            frame_submitted: self.frame_submitted,
            present_us: self.present_us,
            pageflip_pending_at_tick: self.pageflip_pending_at_tick,
            accepted: self.accepted,
            redraw_requested: self.redraw_requested,
            skipped_input_repaints: self.skipped_input_repaints,
            input_drain_us: self.input_drain_us,
            raw_input_events: self.raw_input_events,
            coalesced_input_events: self.coalesced_input_events,
            shutdown_requested: self.shutdown_requested,
            tick_us: self.tick_us,
        }
    }

    pub(super) fn merge_input_microturn(&mut self, baseline: NativeCycleMicroturnBaseline) {
        self.work_class = baseline.work_class;
        self.fast_path_completed = baseline.fast_path_completed;
        self.pageflip_drain_us = baseline.pageflip_drain_us;
        self.pageflip_completed = baseline.pageflip_completed;
        self.completed_pageflip_token = baseline.completed_pageflip_token;
        self.frame_completed = baseline.frame_completed;
        self.frame_rendered = baseline.frame_rendered;
        self.frame_submitted = baseline.frame_submitted;
        self.present_us = baseline.present_us;
        self.pageflip_pending_at_tick = baseline.pageflip_pending_at_tick;
        self.accepted = baseline.accepted.saturating_add(self.accepted);
        self.redraw_requested |= baseline.redraw_requested;
        self.skipped_input_repaints = baseline
            .skipped_input_repaints
            .saturating_add(self.skipped_input_repaints);
        self.input_drain_us = baseline.input_drain_us.saturating_add(self.input_drain_us);
        self.raw_input_events = baseline
            .raw_input_events
            .saturating_add(self.raw_input_events);
        self.coalesced_input_events = baseline
            .coalesced_input_events
            .saturating_add(self.coalesced_input_events);
        self.shutdown_requested |= baseline.shutdown_requested;
        self.tick_us = baseline.tick_us.saturating_add(self.tick_us);
    }

    pub(super) const fn record_presentation_result(
        &mut self,
        frame_completed: bool,
        frame_rendered: bool,
        frame_submitted: bool,
    ) {
        self.frame_completed = frame_completed;
        self.frame_rendered = frame_rendered;
        self.frame_submitted = frame_submitted;
    }
}

#[cfg(test)]
mod microturn_tests {
    use super::*;

    fn cycle_state() -> NativeCycleState {
        NativeCycleState {
            wakeup: NativeWakeup {
                reasons: Default::default(),
                continuation: Default::default(),
                ready_sources: 0,
                blocked_ns: 0,
                timer_lateness_ns: None,
                explicit_sync_acquire_tokens: Vec::new(),
                dmabuf_gpu_release_tokens: Vec::new(),
                xwayland_events: Vec::new(),
                control_events: Vec::new(),
                cursor_io_events: Vec::new(),
            },
            work_class: NativeWorkClass::ProtocolOnly,
            fast_path_completed: true,
            pageflip_drain_us: 11,
            pageflip_completed: true,
            completed_pageflip_token: Some(12),
            frame_completed: true,
            frame_rendered: true,
            frame_submitted: true,
            present_us: 13,
            pageflip_pending_at_tick: true,
            tick_us: 14,
            accepted: 15,
            redraw_requested: true,
            skipped_input_repaints: 16,
            input_drain_us: 17,
            raw_input_events: 18,
            coalesced_input_events: 19,
            shutdown_requested: true,
        }
    }

    #[test]
    fn input_microturn_merges_only_additive_results() {
        let mut cycle = cycle_state();
        let baseline = cycle.microturn_baseline();

        cycle.work_class = NativeWorkClass::PrimaryScene;
        cycle.fast_path_completed = false;
        cycle.pageflip_drain_us = 101;
        cycle.pageflip_completed = false;
        cycle.completed_pageflip_token = Some(102);
        cycle.frame_completed = false;
        cycle.frame_rendered = false;
        cycle.frame_submitted = false;
        cycle.present_us = 103;
        cycle.pageflip_pending_at_tick = false;
        cycle.tick_us = 104;
        cycle.accepted = 105;
        cycle.redraw_requested = false;
        cycle.skipped_input_repaints = 106;
        cycle.input_drain_us = 107;
        cycle.raw_input_events = 108;
        cycle.coalesced_input_events = 109;
        cycle.shutdown_requested = false;

        cycle.merge_input_microturn(baseline);

        assert_eq!(cycle.work_class, NativeWorkClass::ProtocolOnly);
        assert!(cycle.fast_path_completed);
        assert_eq!(cycle.pageflip_drain_us, 11);
        assert!(cycle.pageflip_completed);
        assert_eq!(cycle.completed_pageflip_token, Some(12));
        assert!(cycle.frame_completed);
        assert!(cycle.frame_rendered);
        assert!(cycle.frame_submitted);
        assert_eq!(cycle.present_us, 13);
        assert!(cycle.pageflip_pending_at_tick);
        assert_eq!(cycle.tick_us, 118);
        assert_eq!(cycle.accepted, 120);
        assert!(cycle.redraw_requested);
        assert_eq!(cycle.skipped_input_repaints, 122);
        assert_eq!(cycle.input_drain_us, 124);
        assert_eq!(cycle.raw_input_events, 126);
        assert_eq!(cycle.coalesced_input_events, 128);
        assert!(cycle.shutdown_requested);
    }
}

#[derive(Debug, Default)]
struct KmsWorkerQuarantine {
    jobs: Vec<super::kms_worker::KmsCommitJob>,
    cursor_sidecars: Vec<super::kms_worker::CursorSidecar>,
}

struct PendingCursorJob {
    token: ReactorToken,
    request_id: u64,
    job_id: oblivion_one::cursor_manager::CursorJobId,
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

pub(super) type PresentedPrimaryAssignment = PresentedPrimaryState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ConfirmedOutputPresentationState {
    pub(crate) mode: OutputPresentationMode,
    pub(crate) content_type: DrmContentType,
    pub(crate) output_generation: u64,
}

impl Default for ConfirmedOutputPresentationState {
    fn default() -> Self {
        Self {
            mode: OutputPresentationMode::Vsync,
            content_type: DrmContentType::Graphics,
            output_generation: 0,
        }
    }
}

fn create_native_control_server(
    event_loop: &mut NativeEventLoop,
    server: &OwnCompositorServer,
) -> NativeResult<NativeControlServer> {
    let runtime_dir = oblivion_one::xdg_runtime_dir()?;
    Ok(NativeControlServer::bind(
        event_loop,
        &runtime_dir,
        server.socket_name(),
    )?)
}

fn sync_xwayland_bootstrap_sources(
    event_loop: &mut NativeEventLoop,
    xwayland: &mut XwaylandService,
    tokens: &mut Vec<(ReactorToken, XwaylandReactorRegistration)>,
) -> NativeResult<()> {
    sync_xwayland_reactor_sources(event_loop, xwayland, tokens)
}

// `TargetDestroyed` means the previous KMS target can no longer reference any
// submitted framebuffer. Session inactivity or disarmed I/O is not proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum KmsSafeBoundary {
    Restored,
    TargetDestroyed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum KmsTeardownSafety {
    Restored,
    TargetDestroyed,
    Unproven,
}

impl KmsTeardownSafety {
    pub(super) const fn from_proof(proof: Option<KmsSafeBoundary>) -> Self {
        match proof {
            Some(KmsSafeBoundary::Restored) => Self::Restored,
            Some(KmsSafeBoundary::TargetDestroyed) => Self::TargetDestroyed,
            None => Self::Unproven,
        }
    }

    pub(super) const fn permits_release(self) -> bool {
        matches!(self, Self::Restored | Self::TargetDestroyed)
    }

    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Restored => "restored",
            Self::TargetDestroyed => "target_destroyed",
            Self::Unproven => "unproven",
        }
    }
}

pub(crate) struct NativeRuntime {
    server: OwnCompositorServer,
    cursor_image: std::sync::Arc<oblivion_one::cursor_theme::CompositorCursorImage>,
    cursor_manager: oblivion_one::cursor_manager::CursorThemeManager,
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
    input_batch: NativeInputBatch,
    input_epoch: NativeInputEpoch,
    seat_session: Option<NativeSeatSession>,
    session: NativeSessionLifecycle,
    pending_session_recovery: Option<NativeScanoutRecovery>,
    #[cfg(test)]
    native_io_recorder: NativeIoRecorder,
    acquire_notifier: DrmAcquirePointNotifier,
    acquire_watches: ExplicitSyncWatchRegistry,
    parked_acquire_watches: Vec<oblivion_one::compositor::AcquireWatchRequest>,
    event_loop: NativeEventLoop,
    dmabuf_gpu_release_registry: DmabufGpuReleaseRegistry,
    control_server: NativeControlServer,
    started_at: Instant,
    vrr_plan: NativeVrrPlan,
    xwayland: XwaylandService,
    xwayland_reactor_tokens: Vec<(ReactorToken, XwaylandReactorRegistration)>,
    xwayland_reactor_generation: u64,
    xwayland_client_identity: Option<oblivion_one::compositor::XwaylandClientIdentity>,
    drm_reactor_token: Option<ReactorToken>,
    output_render_fence_token: Option<ReactorToken>,
    kms_commit_worker: Option<super::kms_worker::KmsCommitWorkerHandle>,
    kms_commit_worker_reactor_token: Option<ReactorToken>,
    cursor_io_worker: Option<oblivion_one::cursor_manager::CursorIoWorker>,
    cursor_io_worker_reactor_token: Option<ReactorToken>,
    pending_cursor_job: Option<PendingCursorJob>,
    next_cursor_job_id: u64,
    kms_commit_worker_policy: super::kms_worker::KmsCommitWorkerPolicy,
    kms_commit_worker_transport: super::kms_worker::KmsCommitWorkerTransport,
    kms_commit_worker_startup: super::kms_worker::KmsCommitWorkerStartup,
    worker_quarantine: KmsWorkerQuarantine,
    emergency_quarantined_worker_jobs: Vec<super::kms_worker::KmsCommitJob>,
    submitted_worker_ownership: Vec<super::kms_worker::KmsSubmittedOwnership>,
    emergency_quarantined_submitted_ownership: Vec<super::kms_worker::KmsSubmittedOwnership>,
    kms_teardown_safety: KmsTeardownSafety,
    kms_teardown_safety_established: bool,
    scanout_destroyed: bool,
    deferred_worker_pageflip: Option<DrmPresentationEvent>,
    deferred_worker_completion: Option<AtomicCommitCompletion>,
    worker_timeout_pending: Option<(PageFlipToken, u64)>,
    forced_shutdown_inflight: Option<super::kms_worker::WorkerInFlight>,
    frame_scheduler: NativeFrameScheduler,
    atomic_commit_arbiter: AtomicCommitArbiter,
    output_transactions: OutputTransactionLedger,
    presented_planes: crate::native_output::presentation::plane::PresentedPlaneSnapshot,
    confirmed_output_presentation: ConfirmedOutputPresentationState,
    presentation_timing: KmsPresentationTimingModel,
    presentation_deadline: PresentationDeadlinePlanner,
    scheduled_presentation_target: Option<PresentationTarget>,
    render_journal: AdaptiveRenderJournal,
    adaptive_buffering: AdaptiveBufferingController,
    triple_buffer_policy: AdaptiveTripleBufferPolicy,
    pending_proven_deadline_miss: Option<(u64, ProvenDeadlineMiss)>,
    effective_app_gpu_policy: EffectiveCompositorAppGpuPolicy,
    dmabuf_feedback_compatibility: DmabufFeedbackCompatibility,
    dmabuf_feedback_compat_metrics: DmabufFeedbackCompatibilityMetrics,
    // Logical render/coalescing baseline only. This is not evidence that the
    // corresponding scene was physically rendered or presented.
    last_rendered_scene_generation: u64,
    last_direct_candidate_key: Option<DirectScanoutCandidateKey>,
    direct_fallback_tracker: Option<cycle::direct_fallback::DirectFallbackTracker>,
    last_refresh_sequence: u64,
    last_submitted_cursor_epoch: u64,
    last_primary_presented_at_ns: Option<u64>,
    scene_history: NativeSceneHistory,
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
    wake_authority: NativeWakeAuthorityMetrics,
    pointer_timing: NativePointerTimingTrace,
    last_acquire_ready_at_ns: Option<u64>,
    resize_perf: NativeResizePerfState,
    pointer_constraint_backend: NativePointerConstraintBackend,
    process_supervisor: ChildSupervisor,
    astrea_launch_tracker: AstreaLaunchLifecycleTracker,
    shutdown: NativeShutdownLifecycle,
    presentation_trace: PresentationTransactionTraceRing,
    presentation_trace_path: Option<std::path::PathBuf>,
    timing_scopes: std::collections::BTreeMap<&'static str, TimingSummary>,
    render_telemetry: NativeRenderTelemetry,
}

impl NativeRuntime {
    pub(crate) fn bootstrap(config: NativeRuntimeConfig) -> NativeResult<Self> {
        // Block process-directed SIGCHLD before any native driver, graphics
        // library, or compositor initialization can create a thread.  Every
        // later worker inherits this mask and the child supervisor's signalfd
        // remains the sole normal notification path.
        oblivion_one::process::block_sigchld_for_current_thread()?;
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
        let reconciled = sync_xwayland_reactor_sources_with_generation(
            &mut self.event_loop,
            &mut self.xwayland,
            &mut self.xwayland_reactor_tokens,
            &mut self.xwayland_reactor_generation,
        )?;
        let metrics = self.resource_efficiency_mut();
        metrics.record_xwayland_sync_request();
        if reconciled {
            metrics.record_xwayland_reconciliation();
        } else {
            metrics.record_xwayland_unchanged_skip();
        }
        Ok(())
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

    pub(super) const fn resource_efficiency(&self) -> &ResourceEfficiencyMetrics {
        &self.render_telemetry.resource_efficiency
    }

    pub(super) fn resource_efficiency_mut(&mut self) -> &mut ResourceEfficiencyMetrics {
        &mut self.render_telemetry.resource_efficiency
    }

    pub(super) fn dmabuf_gpu_release_metrics(&self) -> DmabufGpuReleaseMetrics {
        self.dmabuf_gpu_release_registry.metrics()
    }

    pub(super) fn dmabuf_gpu_release_qualification_summary(
        &self,
    ) -> DmabufGpuReleaseQualificationSummary {
        self.dmabuf_gpu_release_registry.qualification_summary()
    }
}

impl Drop for NativeRuntime {
    fn drop(&mut self) {
        let _ = self
            .dmabuf_gpu_release_registry
            .cancel_all(&mut self.event_loop, &mut self.server);
        let _ = self.control_server.shutdown(&mut self.event_loop);
        if let Some(token) = self.cursor_io_worker_reactor_token.take() {
            let _ = self.event_loop.unregister(token);
        }
        self.pending_cursor_job = None;
        self.cursor_io_worker.take();
        if let Some(worker) = self.kms_commit_worker.take() {
            if let Some(token) = self.kms_commit_worker_reactor_token.take() {
                let _ = self.event_loop.unregister(token);
            }
            if self.shutdown.is_shutting_down() {
                let _ = worker.force_shutdown_abandon();
            }
            worker.request_quiesce();
            let _ = worker.join();
            let _ = self.drain_kms_worker_events_for_teardown(&worker);
            let _ = self.defer_fatal_worker_jobs_for_teardown(worker.take_fatal_jobs());
            self.worker_quarantine
                .cursor_sidecars
                .extend(worker.take_pending_cursor_sidecar());
        }
        self.revoke_xwayland_private_client();
        let _ = self
            .xwayland
            .emergency_cleanup(&mut self.process_supervisor);
        let _ = self.sync_xwayland_reactor_sources();
        if oblivion_one::xwayland::trace::enabled() {
            for line in oblivion_one::xwayland::trace::take_recent_lifecycle_trace() {
                eprintln!("oblivion-one xwayland: lifecycle_ring_dump=true {line}");
            }
        } else {
            let _ = oblivion_one::xwayland::trace::take_recent_lifecycle_trace();
        }
        if self.frame_pacing.summary_enabled() {
            println!("{}", self.wake_authority.summary_line(&self.event_loop));
            println!(
                "{}",
                self.frame_pacing
                    .summary_line(self.server.verbose_trace_dropped_entries())
            );
            println!("{}", self.frame_pacing.content_summary_line());
            let transaction_counters = self.output_transactions.counters();
            println!(
                "typhon presentation: event=output_transaction_summary active={} built={} ready={} submitted={} presented={} dropped={} superseded={} failed={} invalid_transitions={} duplicate_obligations={} active_peak={} history_overwrites={} accepted_terminals={} finalized_terminals={} rejected_terminals={} settlement_failures={} failure_stage_mismatches={} active_settling={} immediate_presentations={} immediate_presentation_failures={} immediate_presentations_accepted={} immediate_presentations_finalized={} compatibility_noops={} compatibility_failures={} built_composited={} built_direct={} built_plane_delta={} submitted_composited={} submitted_direct={} submitted_plane_delta={} presented_composited={} presented_direct={} presented_plane_delta={}",
                self.output_transactions.active_count(),
                transaction_counters.built,
                transaction_counters.ready,
                transaction_counters.submitted,
                transaction_counters.presented,
                transaction_counters.dropped,
                transaction_counters.superseded,
                transaction_counters.failed,
                transaction_counters.invalid_transitions,
                transaction_counters.duplicate_obligation_attempts,
                transaction_counters.active_peak,
                transaction_counters.terminal_history_overwrites,
                transaction_counters.terminal_transitions_accepted,
                transaction_counters.terminal_transitions_finalized,
                transaction_counters.terminal_transitions_rejected,
                transaction_counters.settlement_failures,
                transaction_counters.failure_stage_mismatches,
                transaction_counters.active_settling_transactions,
                transaction_counters.immediate_presentations,
                transaction_counters.immediate_presentation_failures,
                transaction_counters.immediate_presentations_accepted,
                transaction_counters.immediate_presentations_finalized,
                transaction_counters.compatibility_noops,
                transaction_counters.compatibility_failures,
                transaction_counters.built_composited,
                transaction_counters.built_direct,
                transaction_counters.built_plane_delta,
                transaction_counters.submitted_composited,
                transaction_counters.submitted_direct,
                transaction_counters.submitted_plane_delta,
                transaction_counters.presented_composited,
                transaction_counters.presented_direct,
                transaction_counters.presented_plane_delta,
            );
            if !self.scanout_destroyed
                && let Some(counters) = self.scanout.explicit_output_counters()
            {
                println!(
                    "typhon pacing: event=explicit_output_summary sync_file_deadline_hints_applied={} sync_file_deadline_hints_unsupported={} sync_file_deadline_hints_failed={} atomic_submissions={} atomic_in_fence_submissions={} async_userspace_fence_submissions={} atomic_out_fences_received={} atomic_out_fence_missing={} render_fence_timing_unavailable={}",
                    counters.sync_file_deadline_hints_applied,
                    counters.sync_file_deadline_hints_unsupported,
                    counters.sync_file_deadline_hints_failed,
                    counters.atomic_submissions,
                    counters.atomic_in_fence_submissions,
                    counters.async_userspace_fence_submissions,
                    counters.atomic_out_fences_received,
                    counters.atomic_out_fence_missing,
                    counters.render_fence_timing_unavailable,
                );
            }
        }
        if !self.establish_kms_teardown_safety().permits_release() {
            self.retain_unproven_teardown_ownership();
            return;
        }
        self.presented_planes.primary = None;
        self.confirmed_output_presentation = ConfirmedOutputPresentationState::default();
        if !self.scanout_destroyed
            && let Err(error) = self.scanout.release_direct_for_target_destroyed()
        {
            eprintln!("native direct target-destroyed release was not proven safe: {error}");
            self.retain_unproven_teardown_ownership();
            return;
        }
        let terminal_callback_transactions = self.output_transactions.active_transaction_ids();
        for transaction_id in terminal_callback_transactions {
            let Some(transaction) = self.output_transactions.transaction(transaction_id) else {
                continue;
            };
            if !matches!(
                transaction.descriptor().content(),
                OutputTransactionContent::Direct { .. }
            ) {
                continue;
            }
            let callback_owner_leaks = direct_terminal_callback_owner_leaks(
                &mut self.server,
                transaction_id,
                transaction.descriptor().obligations(),
                DirectTerminalCallbackDisposition::Abandoned,
            );
            self.scanout
                .note_direct_callback_owner_leaks(callback_owner_leaks);
        }
        // SAFETY: scanout is wrapped solely so teardown can disarm DRM cleanup
        // before its normal resource drop. The proven boundary above means
        // KMS no longer references submitted resources, and scanout is
        // dropped exactly once while `kms` is still alive.
        if !self.scanout_destroyed {
            unsafe { mem::ManuallyDrop::drop(&mut self.scanout) };
        }
        self.submitted_worker_ownership.clear();
        self.worker_quarantine.jobs.clear();
        self.worker_quarantine.cursor_sidecars.clear();
        self.emergency_quarantined_worker_jobs.clear();
        self.emergency_quarantined_submitted_ownership.clear();

        let abandoned_at = MonotonicTimestampNs::new(monotonic_now_ns().unwrap_or(0));
        let transaction_ids = self.output_transactions.active_transaction_ids();
        for transaction_id in transaction_ids {
            let _ = presentation_transactions::complete_dropped_output_transaction(
                &mut self.output_transactions,
                transaction_id,
                OutputTransactionDropReason::OutputDestroyed,
                abandoned_at,
                |obligations| {
                    if let Some(batch_id) = obligations.frame_batch_id() {
                        self.server.complete_frame_batch_after_safe_abandonment(
                            batch_id,
                            FrameBatchDiscardReason::OutputDestroyed,
                        );
                    }
                    Ok(())
                },
            );
        }

        // Client buffers are released only after KMS ownership has ended and
        // the EGL/GBM renderer has been torn down, so shutdown cannot reuse a
        // buffer while KMS or GLES still owns it. The server drop repeats this
        // idempotently.
        self.server.finish_commit_debug_for_shutdown();
        let buffer_release_metrics = self.server.buffer_release_metrics();
        println!(
            "typhon pacing: event=buffer_release_summary buffer_releases_captured={} buffer_releases_completed={} buffer_releases_deferred={} buffer_releases_restored={} buffer_releases_discarded={} buffer_release_duplicate_attempts={} dmabuf_release_terminal_revalidated={} dmabuf_release_terminal_requeued_current={}",
            buffer_release_metrics.buffer_releases_captured,
            buffer_release_metrics.buffer_releases_completed,
            buffer_release_metrics.buffer_releases_deferred,
            buffer_release_metrics.buffer_releases_restored,
            buffer_release_metrics.buffer_releases_discarded,
            buffer_release_metrics.buffer_release_duplicate_attempts,
            buffer_release_metrics.dmabuf_release_terminal_revalidated,
            buffer_release_metrics.dmabuf_release_terminal_requeued_current,
        );
        let dmabuf_release_metrics = self.dmabuf_gpu_release_metrics();
        println!(
            "typhon pacing: event=dmabuf_gpu_release_summary leases_registered={} leases_completed={} leases_requeued={} obligations_armed={} obligations_completed={} fences_created={} fences_signaled={} no_visual_fence_only={} fence_creation_failures={} completion_fd_failures={} registration_failures={} retry_skipped_current_token={} active_leases={} peak_active_leases={}",
            dmabuf_release_metrics.leases_registered,
            dmabuf_release_metrics.leases_completed,
            dmabuf_release_metrics.leases_requeued,
            dmabuf_release_metrics.obligations_armed,
            dmabuf_release_metrics.obligations_completed,
            dmabuf_release_metrics.fences_created,
            dmabuf_release_metrics.fences_signaled,
            dmabuf_release_metrics.no_visual_fence_only,
            dmabuf_release_metrics.fence_creation_failures,
            dmabuf_release_metrics.completion_fd_failures,
            dmabuf_release_metrics.registration_failures,
            dmabuf_release_metrics.retry_skipped_current_token,
            dmabuf_release_metrics.active_leases,
            dmabuf_release_metrics.peak_active_leases,
        );
        let dmabuf_qualification = self.dmabuf_gpu_release_qualification_summary();
        println!(
            "typhon pacing: event=dmabuf_gpu_release_timing_summary composited_correlations_armed={} composited_correlations_paired={} release_before_pageflip_leases={} release_before_pageflip_obligations={} release_after_pageflip_leases={} release_after_pageflip_obligations={} release_same_timestamp_leases={} exact_signal_timestamps={} signal_timestamp_unavailable={} correlations_unpairable_signal_timestamp={} already_signaled_before_registration={} timestamp_order_anomalies={} correlation_pending={} correlation_overflows={} correlation_duplicates={} gpu_release_registry_wait_p50_us={} gpu_release_registry_wait_p95_us={} gpu_release_registry_wait_p99_us={} release_to_pageflip_lead_p50_us={} release_to_pageflip_lead_p95_us={} release_to_pageflip_lead_p99_us={} pageflip_to_release_lag_p50_us={} pageflip_to_release_lag_p95_us={} pageflip_to_release_lag_p99_us={}",
            dmabuf_qualification.composited_correlations_armed,
            dmabuf_qualification.composited_correlations_paired,
            dmabuf_qualification.release_before_pageflip_leases,
            dmabuf_qualification.release_before_pageflip_obligations,
            dmabuf_qualification.release_after_pageflip_leases,
            dmabuf_qualification.release_after_pageflip_obligations,
            dmabuf_qualification.release_same_timestamp_leases,
            dmabuf_qualification.exact_signal_timestamps,
            dmabuf_qualification.signal_timestamp_unavailable,
            dmabuf_qualification.correlations_unpairable_signal_timestamp,
            dmabuf_qualification.already_signaled_before_registration,
            dmabuf_qualification.timestamp_order_anomalies,
            dmabuf_qualification.correlation_pending,
            dmabuf_qualification.correlation_overflows,
            dmabuf_qualification.correlation_duplicates,
            dmabuf_qualification.gpu_release_registry_wait_p50_us,
            dmabuf_qualification.gpu_release_registry_wait_p95_us,
            dmabuf_qualification.gpu_release_registry_wait_p99_us,
            dmabuf_qualification.release_to_pageflip_lead_p50_us,
            dmabuf_qualification.release_to_pageflip_lead_p95_us,
            dmabuf_qualification.release_to_pageflip_lead_p99_us,
            dmabuf_qualification.pageflip_to_release_lag_p50_us,
            dmabuf_qualification.pageflip_to_release_lag_p95_us,
            dmabuf_qualification.pageflip_to_release_lag_p99_us,
        );
    }
}
