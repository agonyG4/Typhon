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
mod frame;
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
mod presentation;
mod presentation_cursor;
mod presentation_direct;
mod presentation_metrics;
mod presentation_pipeline;
mod presentation_protocol;
mod presentation_ready;
mod presentation_transactions;
mod presentation_worker;
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
    AtomicCommitArbiter, AtomicCommitCompletion, AtomicCommitKind, AtomicCommitPhase,
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
#[cfg(test)]
pub(crate) use planner::{
    NativePresentationPath, NativePresentationPlanInput, plan_native_presentation_path,
};
pub(super) use presentation_pipeline::initial_presented;
pub(crate) use presentation_transactions::{
    DirectCallbackLeakMetrics, DirectTerminalCallbackDisposition,
    direct_terminal_callback_owner_leaks, settle_dropped_output_transaction,
    settle_failed_output_transaction, settle_no_visual_change_output_transaction,
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

impl NativeCycleState {
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

pub(super) type ConfirmedPrimaryAssignment = ConfirmedPrimaryState;

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
    seat_session: Option<NativeSeatSession>,
    session: NativeSessionLifecycle,
    pending_session_recovery: Option<NativeScanoutRecovery>,
    #[cfg(test)]
    native_io_recorder: NativeIoRecorder,
    acquire_notifier: DrmAcquirePointNotifier,
    acquire_watches: ExplicitSyncWatchRegistry,
    parked_acquire_watches: Vec<oblivion_one::compositor::AcquireWatchRequest>,
    event_loop: NativeEventLoop,
    control_server: NativeControlServer,
    started_at: Instant,
    vrr_plan: NativeVrrPlan,
    xwayland: XwaylandService,
    xwayland_reactor_tokens: Vec<(ReactorToken, XwaylandReactorRegistration)>,
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
    confirmed_primary_assignment: Option<ConfirmedPrimaryAssignment>,
    confirmed_output_presentation: ConfirmedOutputPresentationState,
    presentation_deadline: PresentationDeadlinePlanner,
    scheduled_presentation_target: Option<PresentationTarget>,
    render_journal: AdaptiveRenderJournal,
    adaptive_buffering: AdaptiveBufferingController,
    triple_buffer_policy: AdaptiveTripleBufferPolicy,
    pending_proven_deadline_miss: Option<ProvenDeadlineMiss>,
    effective_app_gpu_policy: EffectiveCompositorAppGpuPolicy,
    dmabuf_feedback_compatibility: DmabufFeedbackCompatibility,
    dmabuf_feedback_compat_metrics: DmabufFeedbackCompatibilityMetrics,
    last_rendered_scene_generation: u64,
    last_direct_candidate_key: Option<DirectScanoutCandidateKey>,
    direct_fallback_tracker: Option<cycle::direct_fallback::DirectFallbackTracker>,
    last_refresh_sequence: u64,
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
        if self.frame_pacing.enabled() {
            println!(
                "{}",
                self.frame_pacing
                    .summary_line(self.server.verbose_trace_dropped_entries())
            );
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
        if !self.establish_kms_teardown_safety().permits_release() {
            self.retain_unproven_teardown_ownership();
            return;
        }
        self.confirmed_primary_assignment = None;
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
