use super::*;

/// Physical presentation state of one output: its connector/CRTC target,
/// scanout, cursor planes, KMS commit worker, frame scheduling, pacing and
/// presentation timing. `NativeRuntime` owns one head; multi-head support
/// turns this into a set.
pub(crate) struct OutputHead {
    pub(super) output_id: OutputId,
    pub(super) target: NativeAppliedOutputConfiguration,
    /// Atomic/legacy KMS pipeline bound to this head's connector and CRTC.
    pub(super) kms_backend: KmsBackendSelection,
    pub(super) output_capabilities: NativeOutputCapabilities,
    pub(super) output_mode_mutation_policy: output_configuration::OutputModeMutationPolicy,
    pub(super) output_configuration_generation: OutputConfigurationGeneration,
    pub(super) output_configuration_transactions: OutputConfigurationTransactions,
    pub(super) pending_output_configuration: Option<PendingOutputConfigurationRequest>,
    pub(super) mode_label: String,
    pub(super) output_refresh_rate: oblivion_one::compositor::OutputRefreshRate,
    // Physical presentation lifetime; modesets and successful session recovery
    // advance it independently of the semantic Settings generation.
    pub(super) output_epoch: OutputEpochId,
    pub(super) scanout: mem::ManuallyDrop<NativeScanoutBackend>,
    pub(super) cursor_output_arbitration: NativeCursorOutputArbitration,
    pub(super) cursor_render_mode: NativeCursorRenderMode,
    pub(super) atomic_cursor: Option<NativeAtomicCursor>,
    pub(super) legacy_cursor: Option<NativeLegacyHardwareCursor>,
    pub(super) output_render_fence_token: Option<ReactorToken>,
    pub(super) kms_commit_worker: Option<super::super::kms_worker::KmsCommitWorkerHandle>,
    pub(super) kms_commit_worker_reactor_token: Option<ReactorToken>,
    pub(super) render_worker_reactor_token: Option<ReactorToken>,
    pub(super) render_worker_admission_stopped: bool,
    pub(super) pending_render_worker_identity:
        Option<super::super::scanout::AtomicRenderWorkerIdentity>,
    pub(super) pending_render_worker_context:
        Option<super::super::scanout::AtomicRenderSettlementContext>,
    pub(super) pending_render_worker_outer:
        Option<presentation_cycle::AtomicRenderOuterContinuation>,
    pub(super) pending_render_worker_completion:
        Option<super::super::scanout::AtomicRenderWorkerCompletion>,
    pub(super) pending_render_worker_capture: Option<render_worker::PendingWorkerCapture>,
    pub(super) pending_dmabuf_release_fence:
        Option<(crate::native_output::scanout::RenderWorkerJobId, u64)>,
    pub(super) output_reconfiguration_rearm_retry_deadline_ns: Option<u64>,
    pub(super) kms_commit_worker_policy: super::super::kms_worker::KmsCommitWorkerPolicy,
    pub(super) kms_commit_worker_transport: super::super::kms_worker::KmsCommitWorkerTransport,
    pub(super) kms_commit_worker_startup: super::super::kms_worker::KmsCommitWorkerStartup,
    pub(super) worker_quarantine: KmsWorkerQuarantine,
    pub(super) emergency_quarantined_worker_jobs: Vec<super::super::kms_worker::KmsCommitJob>,
    pub(super) submitted_worker_ownership: Vec<super::super::kms_worker::KmsSubmittedOwnership>,
    pub(super) emergency_quarantined_submitted_ownership:
        Vec<super::super::kms_worker::KmsSubmittedOwnership>,
    pub(super) kms_teardown_safety: KmsTeardownSafety,
    pub(super) kms_teardown_safety_established: bool,
    pub(super) scanout_destroyed: bool,
    pub(super) deferred_worker_pageflip: Option<DrmPresentationEvent>,
    pub(super) deferred_worker_completion: Option<AtomicCommitCompletion>,
    pub(super) worker_timeout_pending: Option<(PageFlipToken, u64)>,
    /// Deferred worker events taken for this cycle's pageflip settlement.
    pub(super) cycle_worker_handoff: Option<(
        Option<DrmPresentationEvent>,
        Option<AtomicCommitCompletion>,
        Option<(PageFlipToken, u64)>,
    )>,
    pub(super) forced_shutdown_inflight: Option<super::super::kms_worker::WorkerInFlight>,
    pub(super) forced_shutdown_pacing_settled: Option<(
        PageFlipToken,
        crate::native_output::pacing::WorkerPacingTicket,
    )>,
    pub(super) frame_scheduler: NativeFrameScheduler,
    pub(super) atomic_commit_arbiter: AtomicCommitArbiter,
    pub(super) output_transactions: OutputTransactionLedger,
    pub(super) presented_planes: crate::native_output::presentation::plane::PresentedPlaneSnapshot,
    pub(super) confirmed_kms_presentation: ConfirmedKmsPresentationState,
    pub(super) vrr_cadence: super::vrr_cadence::VrrCadenceRecorder,
    pub(super) presentation_timing: KmsPresentationTimingModel,
    pub(super) cursor_timing: crate::native_output::presentation::cursor_timing::CursorTimingDomain,
    pub(super) presentation_deadline: PresentationDeadlinePlanner,
    pub(super) scheduled_presentation_target: Option<PresentationTarget>,
    /// Advisory late-latch target of a queued fixed-refresh render; it only
    /// delays render start and is recomputed every cycle.
    pub(super) render_latch_target: Option<PresentationTarget>,
    pub(super) render_journal: AdaptiveRenderJournal,
    pub(super) adaptive_buffering: AdaptiveBufferingController,
    pub(super) triple_buffer_policy: AdaptiveTripleBufferPolicy,
    pub(super) pending_proven_deadline_miss: Option<(u64, ProvenDeadlineMiss)>,
    // Logical render/coalescing baseline only. This is not evidence that the
    // corresponding scene was physically rendered or presented.
    pub(super) last_rendered_scene_generation: u64,
    pub(super) last_direct_candidate_key: Option<DirectScanoutCandidateKey>,
    pub(super) direct_fallback_tracker: Option<cycle::direct_fallback::DirectFallbackTracker>,
    pub(super) last_refresh_sequence: u64,
    pub(super) last_submitted_cursor_epoch: u64,
    pub(super) last_primary_presented_at_ns: Option<u64>,
    pub(super) scene_history: NativeSceneHistory,
    pub(super) last_client_cursor_damage: Option<NativeClientCursorDamageState>,
    pub(super) last_software_cursor_damage: Option<NativeDamageRect>,
    pub(super) last_client_cursor_path: Option<NativeClientCursorPath>,
    pub(super) render_demand_coalesced_count: u64,
    pub(super) frame_index: u64,
    pub(super) mismatched_pageflip_events: u64,
    pub(super) stale_pageflip_events: u64,
    pub(super) presentation_cadence: PresentationCadenceMetrics,
    pub(super) frame_pacing: NativeFramePacing,
    pub(super) last_acquire_ready_at_ns: Option<u64>,
    pub(super) scaled_primary_probe: crate::native_output::scanout::ScaledPrimaryProbeCache,
}

impl NativeRuntime {
    pub(super) fn parked_head_count(&self) -> usize {
        self.parked_heads.len()
    }

    /// Runs `f` with parked head `index` swapped into `self.head`, so the
    /// per-head cycle phases written against `self.head` drive that head.
    /// `f` must not add or remove parked heads.
    pub(super) fn with_parked_head<R>(
        &mut self,
        index: usize,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        std::mem::swap(&mut self.head, &mut self.parked_heads[index]);
        self.server.enter_output_presentation(self.head.output_id);
        // The cursor pipeline reads the input state in output-local space;
        // give it this head's view so its cursor draws where the pointer is
        // and hides while the pointer is on another output.
        let head_view = self
            .server
            .logical_output_rect(self.head.output_id)
            .and_then(|(x, y, width, height)| {
                Some((
                    x,
                    y,
                    u32::try_from(width).ok()?,
                    u32::try_from(height).ok()?,
                ))
            })
            .map(|rect| self.input_state.enter_head_view(rect));
        let result = f(self);
        if let Some(saved) = head_view {
            self.input_state.leave_head_view(saved);
        }
        std::mem::swap(&mut self.head, &mut self.parked_heads[index]);
        self.server.enter_output_presentation(self.head.output_id);
        result
    }
}
