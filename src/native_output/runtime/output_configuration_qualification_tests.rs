use super::*;
use oblivion_one::compositor::OutputRefreshRate;
use oblivion_one::control::ControlErrorCode;
use oblivion_one::control_snapshots::OutputTransformSnapshot;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransitionEvent {
    SafeBoundaryChecked,
    TestOnlyPassed,
    TransactionReserved,
    AcquireWatchesParked,
    KernelCommitted,
    CandidateAdopted,
    TransactionFinalized,
    DirectScanoutRetired,
    RuntimePublished,
    AcquireWatchesRearmed,
    AcquireRearmFailed,
    ResponseQueued,
    FailureQueued,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailurePoint {
    TestOnly,
    Reservation,
    AcquireParking,
    RealCommit,
}

#[derive(Debug)]
struct FakeCandidate {
    id: u64,
    target: NativeAppliedOutputConfiguration,
    rendered: bool,
}

#[derive(Debug, Clone)]
struct FakeAcquireObligation {
    id: u64,
    signaled: bool,
    registered_generation: u64,
    rearmed_generation: Option<u64>,
}

struct FakePersistenceJob {
    purpose: OutputPersistencePurpose,
    response: Option<u64>,
    target_mode_id: u32,
}

struct FakeOutputRuntime {
    now_ns: u64,
    current: NativeAppliedOutputConfiguration,
    generation: OutputConfigurationGeneration,
    output_epoch: OutputEpochId,
    scanout_id: u64,
    scanout_dimensions: (u32, u32),
    server_geometry: (u32, u32),
    server_refresh_millihz: u32,
    input_geometry: (u32, u32),
    native_pointer_bounds: (u32, u32),
    libinput_mapping_dimensions: (u32, u32),
    cursor_output_identity: (u64, u32, u32, u32),
    cursor_position: (u32, u32),
    cursor_validation_proof_valid: bool,
    scheduler_interval_ns: u64,
    presentation_interval_ns: u64,
    damage_history_generation: u64,
    direct_validation_generation: u64,
    direct_validation_valid: bool,
    next_frame_starts_composited: bool,
    transactions: OutputConfigurationTransactions,
    persistence_job: Option<FakePersistenceJob>,
    persistence_submissions: Vec<u32>,
    persistence_compensation_required: bool,
    persistence_compensation_failed: bool,
    failure: Option<FailurePoint>,
    commit_duration_ns: u64,
    submitted_pageflip_owned: bool,
    direct_scanout_owned: bool,
    direct_owned_during_test: bool,
    direct_owned_during_commit: bool,
    direct_scanout_retire_count: usize,
    acquire_obligations: Vec<FakeAcquireObligation>,
    parked_acquire_obligations: Vec<u64>,
    acquire_watch_generation: u64,
    acquire_rearm_retry_deadline_ns: Option<u64>,
    fail_next_acquire_rearm: bool,
    acquire_ready_notifications: Vec<u64>,
    events: Vec<TransitionEvent>,
    success_responses: Vec<u64>,
    failure_responses: Vec<String>,
    last_commit_at_ns: Option<u64>,
}

impl FakeOutputRuntime {
    fn new(current: NativeAppliedOutputConfiguration) -> Self {
        let rate = output_rate(&current);
        Self {
            now_ns: 0,
            current,
            generation: OutputConfigurationGeneration::initial(),
            output_epoch: OutputEpochId::initial(),
            scanout_id: 1,
            scanout_dimensions: (1920, 1080),
            server_geometry: (1920, 1080),
            server_refresh_millihz: rate.refresh_millihz(),
            input_geometry: (1920, 1080),
            native_pointer_bounds: (1920, 1080),
            libinput_mapping_dimensions: (1920, 1080),
            cursor_output_identity: (1, 12, 1920, 1080),
            cursor_position: (1900, 1000),
            cursor_validation_proof_valid: true,
            scheduler_interval_ns: rate.interval_ns(),
            presentation_interval_ns: rate.interval_ns(),
            damage_history_generation: 1,
            direct_validation_generation: 1,
            direct_validation_valid: true,
            next_frame_starts_composited: true,
            transactions: OutputConfigurationTransactions::new(),
            persistence_job: None,
            persistence_submissions: Vec::new(),
            persistence_compensation_required: false,
            persistence_compensation_failed: false,
            failure: None,
            commit_duration_ns: 1_000,
            submitted_pageflip_owned: false,
            direct_scanout_owned: false,
            direct_owned_during_test: false,
            direct_owned_during_commit: false,
            direct_scanout_retire_count: 0,
            acquire_obligations: Vec::new(),
            parked_acquire_obligations: Vec::new(),
            acquire_watch_generation: 1,
            acquire_rearm_retry_deadline_ns: None,
            fail_next_acquire_rearm: false,
            acquire_ready_notifications: Vec::new(),
            events: Vec::new(),
            success_responses: Vec::new(),
            failure_responses: Vec::new(),
            last_commit_at_ns: None,
        }
    }

    fn temporary_operation(&self) -> OutputConfigurationOperation {
        OutputConfigurationOperation::TemporaryApply {
            previous: self.current,
        }
    }

    fn advance(
        &mut self,
        operation: OutputConfigurationOperation,
        target: NativeAppliedOutputConfiguration,
        now_ns: u64,
        response: Option<u64>,
    ) -> OutputConfigurationAdvance {
        self.now_ns = now_ns;
        let candidate = FakeCandidate {
            id: self.scanout_id.wrapping_add(1).max(1),
            target,
            rendered: true,
        };
        self.events.push(TransitionEvent::SafeBoundaryChecked);
        let safe_boundary = OutputConfigurationSafeBoundary {
            main_thread_pageflip_owned: self.submitted_pageflip_owned,
            explicit_sync_watch_owned: !self.acquire_obligations.is_empty(),
            ..OutputConfigurationSafeBoundary::default()
        };
        if !safe_boundary.is_safe() {
            return OutputConfigurationAdvance::Waiting;
        }
        advance_output_configuration_transition(
            self,
            operation,
            target,
            response,
            None,
            candidate,
            now_ns,
            Instant::now(),
        )
        .expect("fake output transition response path is infallible")
    }

    fn advance_expired_rollback(&mut self, now_ns: u64) -> OutputConfigurationAdvance {
        self.now_ns = now_ns;
        let (transaction_id, target) = self
            .transactions
            .begin_expired_rollback(now_ns)
            .expect("rollback deadline is due");
        self.advance(
            OutputConfigurationOperation::Rollback { transaction_id },
            target,
            now_ns,
            None,
        )
    }

    fn begin_keep(
        &mut self,
        now_ns: u64,
        response: Option<u64>,
    ) -> OutputConfigurationTransactionId {
        let active = self
            .transactions
            .active()
            .expect("apply is awaiting confirmation");
        let transaction_id = active.id;
        let target = self
            .transactions
            .begin_confirm(transaction_id, now_ns)
            .expect("Keep starts before the rollback deadline");
        assert_eq!(target.mode_id, self.current.mode_id);
        OutputConfigurationRuntime::begin_persistence(
            self,
            OutputPersistencePurpose::Confirm { transaction_id },
            response,
            vec![1],
        )
        .expect("fake persistence worker accepts a job");
        transaction_id
    }

    fn complete_persistence(&mut self, succeeded: bool, now_ns: u64) {
        let job = self
            .persistence_job
            .take()
            .expect("persistence job is pending");
        match job.purpose {
            OutputPersistencePurpose::Confirm { transaction_id } => {
                let completion =
                    self.transactions
                        .complete_confirm(transaction_id, succeeded, now_ns);
                match completion {
                    OutputConfigurationConfirmCompletion::Confirmed => {
                        if let Some(response) = job.response {
                            self.success_responses.push(response);
                        }
                    }
                    OutputConfigurationConfirmCompletion::PersistenceFailed => {
                        self.failure_responses
                            .push("persistence_failed".to_string());
                    }
                    OutputConfigurationConfirmCompletion::RollbackOwns
                    | OutputConfigurationConfirmCompletion::Stale => {
                        if succeeded {
                            self.persistence_compensation_required = true;
                        }
                        self.failure_responses
                            .push("rollback_owns_mode".to_string());
                    }
                }
            }
            OutputPersistencePurpose::Compensate => {
                assert_eq!(job.target_mode_id, self.current.mode_id);
                if succeeded {
                    self.persistence_compensation_required = false;
                    self.persistence_compensation_failed = false;
                } else {
                    self.persistence_compensation_required = false;
                    self.persistence_compensation_failed = true;
                }
            }
        }
    }

    fn rate(&self) -> OutputRefreshRate {
        output_rate(&self.current)
    }
}

impl OutputConfigurationRuntime for FakeOutputRuntime {
    type Candidate = FakeCandidate;
    type ConfirmationReservation =
        crate::native_output::output::PreparedOutputConfigurationTransaction;
    type Response = Option<u64>;

    fn test_candidate(&mut self, _candidate: &mut Self::Candidate) -> Result<(), String> {
        self.direct_owned_during_test = self.direct_scanout_owned;
        if self.failure == Some(FailurePoint::TestOnly) {
            return Err("TEST_ONLY injected failure".to_string());
        }
        self.events.push(TransitionEvent::TestOnlyPassed);
        Ok(())
    }

    fn direct_scanout_retirement_safe(&mut self) -> bool {
        !self.submitted_pageflip_owned
    }

    fn reserve_confirmation_transaction(
        &mut self,
        operation: OutputConfigurationOperation,
        target: NativeAppliedOutputConfiguration,
    ) -> Result<Option<Self::ConfirmationReservation>, String> {
        if !matches!(
            operation,
            OutputConfigurationOperation::TemporaryApply { .. }
        ) {
            return Ok(None);
        }
        if self.failure == Some(FailurePoint::Reservation) {
            return Err("confirmation reservation injected failure".to_string());
        }
        let OutputConfigurationOperation::TemporaryApply { previous } = operation else {
            unreachable!()
        };
        let prepared = self
            .transactions
            .prepare_temporary_apply(
                "output-test",
                previous,
                target,
                self.generation.get().wrapping_add(1).max(1),
            )
            .map_err(|error| format!("reservation: {error:?}"))?;
        self.events.push(TransitionEvent::TransactionReserved);
        Ok(Some(prepared))
    }

    fn cancel_confirmation_reservation(&mut self, reservation: Self::ConfirmationReservation) {
        self.transactions
            .cancel_prepared_temporary_apply(reservation);
    }

    fn park_acquire_watches(&mut self) -> Result<(), String> {
        self.parked_acquire_obligations = self
            .acquire_obligations
            .iter()
            .map(|item| item.id)
            .collect();
        self.events.push(TransitionEvent::AcquireWatchesParked);
        if self.failure == Some(FailurePoint::AcquireParking) {
            return Err("acquire-watch parking injected failure".to_string());
        }
        Ok(())
    }

    fn quiesce_old_output_epoch(&mut self) -> Result<QuiescedOutputEpoch, String> {
        let boundary = NativeOutputReconfigurationBoundary {
            main_thread_pageflip_owned: self.submitted_pageflip_owned,
            parked_explicit_sync_obligation_owned: !self.parked_acquire_obligations.is_empty(),
            ..NativeOutputReconfigurationBoundary::default()
        };
        QuiescedOutputEpoch::from_boundary(self.output_epoch, boundary)
            .ok_or_else(|| format!("fake epoch {} did not quiesce", self.output_epoch.get()))
    }

    fn commit_started_at_ns(&mut self, _cycle_started_at: Instant, now_ns: u64) -> u64 {
        assert_eq!(
            self.now_ns, now_ns,
            "coordinator uses injected monotonic time"
        );
        self.now_ns
    }

    fn commit_candidate(
        &mut self,
        candidate: &mut Self::Candidate,
        quiesced_epoch: QuiescedOutputEpoch,
        commit_started_at_ns: u64,
    ) -> Result<u64, String> {
        assert_eq!(quiesced_epoch.output_epoch(), self.output_epoch);
        assert!(candidate.rendered);
        self.direct_owned_during_commit = self.direct_scanout_owned;
        if self.failure == Some(FailurePoint::RealCommit) {
            return Err("real commit injected failure".to_string());
        }
        self.events.push(TransitionEvent::KernelCommitted);
        let committed_at_ns = commit_started_at_ns.saturating_add(self.commit_duration_ns);
        self.last_commit_at_ns = Some(committed_at_ns);
        Ok(committed_at_ns)
    }

    fn adopt_committed_candidate(&mut self, candidate: Self::Candidate) -> Self::Candidate {
        self.events.push(TransitionEvent::CandidateAdopted);
        candidate
    }

    fn finalize_committed_transaction(
        &mut self,
        operation: OutputConfigurationOperation,
        reservation: Option<Self::ConfirmationReservation>,
        committed_at_ns: u64,
    ) {
        match operation {
            OutputConfigurationOperation::TemporaryApply { .. } => {
                let reservation = reservation.expect("temporary apply has reserved transaction");
                self.transactions
                    .finalize_temporary_apply(reservation, committed_at_ns);
            }
            OutputConfigurationOperation::Rollback { transaction_id } => {
                self.transactions
                    .finalize_committed_rollback(transaction_id);
                if self.persistence_job.as_ref().is_some_and(|job| {
                    matches!(job.purpose, OutputPersistencePurpose::Confirm { .. })
                }) {
                    self.persistence_compensation_required = true;
                }
            }
        }
        self.events.push(TransitionEvent::TransactionFinalized);
    }

    fn publish_committed_output_configuration(
        &mut self,
        _operation: OutputConfigurationOperation,
        target: NativeAppliedOutputConfiguration,
        candidate: Self::Candidate,
        _committed_at_ns: u64,
    ) {
        assert_eq!(candidate.target.mode_id, target.mode_id);
        if self.direct_scanout_owned {
            self.direct_scanout_owned = false;
            self.direct_scanout_retire_count += 1;
            self.events.push(TransitionEvent::DirectScanoutRetired);
        }
        self.current = target;
        self.scanout_id = candidate.id;
        self.scanout_dimensions = (target.width, target.height);
        self.generation.advance();
        self.output_epoch = self.output_epoch.next();
        self.acquire_watch_generation = self.output_epoch.get();
        self.server_geometry = (target.width, target.height);
        self.server_refresh_millihz = output_rate(&target).refresh_millihz();
        self.input_geometry = (target.width, target.height);
        self.native_pointer_bounds = (target.width, target.height);
        self.libinput_mapping_dimensions = (target.width, target.height);
        self.cursor_position.0 = self.cursor_position.0.min(target.width.saturating_sub(1));
        self.cursor_position.1 = self.cursor_position.1.min(target.height.saturating_sub(1));
        self.cursor_output_identity = (
            self.output_epoch.get(),
            target.crtc_id,
            target.width,
            target.height,
        );
        self.cursor_validation_proof_valid = false;
        self.scheduler_interval_ns = output_rate(&target).interval_ns();
        self.presentation_interval_ns = output_rate(&target).interval_ns();
        self.damage_history_generation = self.damage_history_generation.wrapping_add(1).max(1);
        self.direct_validation_generation =
            self.direct_validation_generation.wrapping_add(1).max(1);
        self.direct_validation_valid = false;
        self.next_frame_starts_composited = true;
        self.events.push(TransitionEvent::RuntimePublished);
    }

    fn rearm_acquire_watches(&mut self, now_ns: u64) {
        if self.fail_next_acquire_rearm {
            self.fail_next_acquire_rearm = false;
            self.acquire_rearm_retry_deadline_ns = Some(now_ns.saturating_add(10_000_000));
            self.events.push(TransitionEvent::AcquireRearmFailed);
            return;
        }
        for obligation in &mut self.acquire_obligations {
            if self.parked_acquire_obligations.contains(&obligation.id) {
                obligation.rearmed_generation = Some(self.output_epoch.get());
            }
        }
        self.parked_acquire_obligations.clear();
        self.acquire_rearm_retry_deadline_ns = None;
        self.events.push(TransitionEvent::AcquireWatchesRearmed);
    }

    fn queue_success_response(
        &mut self,
        _operation: OutputConfigurationOperation,
        response: Self::Response,
    ) {
        if let Some(response) = response {
            self.success_responses.push(response);
        }
        self.events.push(TransitionEvent::ResponseQueued);
    }

    fn queue_failure_response(
        &mut self,
        operation: OutputConfigurationOperation,
        _target: NativeAppliedOutputConfiguration,
        response: Self::Response,
        _failure_code: Option<ControlErrorCode>,
        detail: String,
    ) -> NativeResult<()> {
        if let OutputConfigurationOperation::Rollback { transaction_id } = operation {
            let _ = self
                .transactions
                .fail_rollback(transaction_id, "test_failure");
        }
        if response.is_some() {
            self.failure_responses.push(detail);
        }
        self.events.push(TransitionEvent::FailureQueued);
        Ok(())
    }

    fn begin_persistence(
        &mut self,
        purpose: OutputPersistencePurpose,
        response: Self::Response,
        _bytes: Vec<u8>,
    ) -> Result<(), OutputPersistenceSubmitError> {
        if self.persistence_job.is_some() {
            return Err(OutputPersistenceSubmitError::Busy);
        }
        self.persistence_submissions.push(self.current.mode_id);
        self.persistence_job = Some(FakePersistenceJob {
            purpose,
            response,
            target_mode_id: self.current.mode_id,
        });
        Ok(())
    }
}

fn output_rate(configuration: &NativeAppliedOutputConfiguration) -> OutputRefreshRate {
    super::super::super::output::output_refresh_rate_for_mode(&configuration.mode)
}

fn configuration(
    width: u16,
    height: u16,
    refresh_hz: u32,
    mode_id: u32,
) -> NativeAppliedOutputConfiguration {
    let htotal = if width == 1920 { 2000 } else { 1400 };
    let vtotal = if width == 1920 { 1250 } else { 1000 };
    let clock = refresh_hz * u32::from(htotal) * u32::from(vtotal) / 1000;
    let mode = drm_sys::drm_mode_modeinfo {
        clock,
        hdisplay: width,
        hsync_start: width + 8,
        hsync_end: width + 16,
        htotal,
        vdisplay: height,
        vsync_start: height + 2,
        vsync_end: height + 4,
        vtotal,
        vrefresh: refresh_hz,
        ..Default::default()
    };
    NativeAppliedOutputConfiguration {
        connector_id: 11,
        crtc_id: 12,
        mode_id,
        mode,
        width: u32::from(width),
        height: u32::from(height),
        scale_milli: 1000,
        transform: OutputTransformSnapshot::Normal,
    }
}

fn apply(runtime: &mut FakeOutputRuntime, target: NativeAppliedOutputConfiguration, now_ns: u64) {
    let operation = runtime.temporary_operation();
    assert_eq!(
        runtime.advance(operation, target, now_ns, Some(10)),
        OutputConfigurationAdvance::Completed
    );
}

#[test]
fn refresh_change_and_timeout_rollback_publish_coordinated_state() {
    let previous = configuration(1920, 1080, 165, 1);
    let temporary = configuration(1920, 1080, 60, 2);
    let mut runtime = FakeOutputRuntime::new(previous);
    runtime.acquire_obligations.push(FakeAcquireObligation {
        id: 41,
        signaled: false,
        registered_generation: 1,
        rearmed_generation: None,
    });
    let initial_generation = runtime.generation.get();

    apply(&mut runtime, temporary, 1_000_000_000);
    let committed_at_ns = runtime.last_commit_at_ns.expect("real commit completed");
    let active = runtime.transactions.active().expect("confirmation pending");
    assert_eq!(runtime.current.mode_id, 2);
    assert_eq!(
        NativeOutputModeTimingFingerprint::from_mode(&runtime.current.mode),
        NativeOutputModeTimingFingerprint::from_mode(&temporary.mode),
        "temporary apply selects the exact requested native mode",
    );
    assert_eq!(runtime.generation.get(), initial_generation + 1);
    assert_eq!(runtime.output_epoch, OutputEpochId::initial().next());
    assert_eq!(
        active.phase,
        OutputConfigurationTransactionPhase::PendingConfirmation
    );
    assert_eq!(
        active.rollback_deadline_ns,
        committed_at_ns + 15_000_000_000
    );
    assert_eq!(runtime.rate().refresh_millihz(), 60_000);
    assert_eq!(runtime.server_refresh_millihz, 60_000);
    assert_eq!(runtime.server_geometry, (1920, 1080));
    assert_eq!(runtime.scheduler_interval_ns, runtime.rate().interval_ns());
    assert_eq!(
        runtime.presentation_interval_ns,
        runtime.rate().interval_ns()
    );
    assert_eq!(runtime.scanout_dimensions, (1920, 1080));
    assert_eq!(
        runtime.scanout_id, 2,
        "prepared candidate became authoritative"
    );
    assert!(!runtime.direct_validation_valid);
    assert_eq!(runtime.direct_validation_generation, 2);
    assert!(!runtime.cursor_validation_proof_valid);
    assert_eq!(runtime.damage_history_generation, 2);
    assert_eq!(runtime.acquire_obligations.len(), 1);
    assert!(!runtime.acquire_obligations[0].signaled);
    assert_eq!(runtime.acquire_obligations[0].rearmed_generation, Some(2));
    assert!(runtime.acquire_ready_notifications.is_empty());

    let deadline = active.rollback_deadline_ns;
    assert_eq!(
        runtime.advance_expired_rollback(deadline),
        OutputConfigurationAdvance::Completed
    );
    assert_eq!(runtime.current.mode_id, previous.mode_id);
    assert_eq!(
        NativeOutputModeTimingFingerprint::from_mode(&runtime.current.mode),
        NativeOutputModeTimingFingerprint::from_mode(&previous.mode),
        "rollback restores the exact previous native mode timing",
    );
    assert_eq!(runtime.generation.get(), initial_generation + 2);
    assert_eq!(runtime.output_epoch, OutputEpochId::initial().next().next());
    assert_eq!(runtime.rate().refresh_millihz(), 165_000);
    assert_eq!(runtime.server_refresh_millihz, 165_000);
    assert_eq!(runtime.scheduler_interval_ns, runtime.rate().interval_ns());
    assert_eq!(
        runtime.presentation_interval_ns,
        runtime.rate().interval_ns()
    );
    assert_eq!(runtime.acquire_obligations[0].rearmed_generation, Some(3));
    assert!(runtime.transactions.active().is_none());
    assert_eq!(runtime.success_responses, vec![10]);
}

#[test]
fn dimension_change_keep_updates_geometry_cursor_damage_and_persistence_together() {
    let previous = configuration(1920, 1080, 60, 1);
    let temporary = configuration(1280, 720, 60, 2);
    let mut runtime = FakeOutputRuntime::new(previous);
    apply(&mut runtime, temporary, 2_000_000_000);
    let generation_after_apply = runtime.generation.get();

    assert_eq!(runtime.scanout_dimensions, (1280, 720));
    assert_eq!((runtime.current.width, runtime.current.height), (1280, 720));
    assert_eq!(runtime.server_geometry, (1280, 720));
    assert_eq!(runtime.input_geometry, (1280, 720));
    assert_eq!(runtime.native_pointer_bounds, (1280, 720));
    assert_eq!(runtime.libinput_mapping_dimensions, (1280, 720));
    assert_eq!(runtime.cursor_output_identity, (2, 12, 1280, 720));
    assert_eq!(runtime.cursor_position, (1279, 719));
    assert!(!runtime.cursor_validation_proof_valid);
    assert!(!runtime.direct_validation_valid);
    assert_eq!(runtime.damage_history_generation, 2);
    assert_eq!(
        runtime.scheduler_interval_ns,
        output_rate(&temporary).interval_ns()
    );
    assert_eq!(
        runtime.presentation_interval_ns,
        output_rate(&temporary).interval_ns()
    );
    assert_eq!(generation_after_apply, 2);

    let active = runtime.transactions.active().expect("confirmation pending");
    let transaction_id = active.id;
    let deadline = active.rollback_deadline_ns;
    let commit_time = runtime.last_commit_at_ns.unwrap();
    let id = runtime.begin_keep(commit_time + 1, Some(11));
    assert_eq!(id, transaction_id);
    assert_eq!(
        runtime.transactions.active().unwrap().phase,
        OutputConfigurationTransactionPhase::PersistencePending
    );
    assert!(
        runtime.transactions.active().is_some(),
        "Keep has not removed rollback protection before persistence submission"
    );
    assert_eq!(runtime.persistence_submissions, vec![temporary.mode_id]);
    assert_eq!(runtime.generation.get(), generation_after_apply);
    assert_eq!(
        runtime.persistence_job.as_ref().unwrap().target_mode_id,
        temporary.mode_id
    );
    assert!(deadline > runtime.last_commit_at_ns.unwrap());

    runtime.complete_persistence(true, runtime.last_commit_at_ns.unwrap() + 2_000);
    assert!(runtime.transactions.active().is_none());
    assert_eq!(runtime.generation.get(), generation_after_apply);
    assert_eq!(runtime.success_responses, vec![10, 11]);
}

#[test]
fn precommit_failures_preserve_old_authoritative_configuration() {
    let previous = configuration(1920, 1080, 165, 1);
    let temporary = configuration(1920, 1080, 60, 2);
    for failure in [
        FailurePoint::TestOnly,
        FailurePoint::Reservation,
        FailurePoint::AcquireParking,
        FailurePoint::RealCommit,
    ] {
        let mut runtime = FakeOutputRuntime::new(previous);
        runtime.failure = Some(failure);
        if failure == FailurePoint::RealCommit {
            runtime.direct_scanout_owned = true;
        }
        let old_scanout_id = runtime.scanout_id;
        let result = runtime.advance(runtime.temporary_operation(), temporary, 100, Some(99));
        assert_eq!(result, OutputConfigurationAdvance::Completed, "{failure:?}");
        assert_eq!(runtime.current.mode_id, previous.mode_id, "{failure:?}");
        assert_eq!(runtime.scanout_id, old_scanout_id, "{failure:?}");
        assert_eq!(runtime.generation.get(), 1, "{failure:?}");
        assert!(runtime.transactions.active().is_none(), "{failure:?}");
        assert!(runtime.success_responses.is_empty(), "{failure:?}");
        assert_eq!(runtime.failure_responses.len(), 1, "{failure:?}");
        assert!(runtime.parked_acquire_obligations.is_empty(), "{failure:?}");
        assert!(
            runtime.transactions.can_begin(),
            "reservation is reusable after {failure:?}"
        );
        if failure == FailurePoint::RealCommit {
            assert!(
                runtime.direct_scanout_owned,
                "failed commit leaves direct owner intact"
            );
            assert_eq!(runtime.direct_scanout_retire_count, 0);
            assert!(
                runtime
                    .events
                    .contains(&TransitionEvent::AcquireWatchesParked)
            );
            assert!(
                runtime
                    .events
                    .contains(&TransitionEvent::AcquireWatchesRearmed)
            );
            assert!(!runtime.events.contains(&TransitionEvent::KernelCommitted));
        }
    }
}

#[test]
fn direct_scanout_primary_survives_sync_modeset_and_retires_exactly_once() {
    let previous = configuration(1920, 1080, 165, 1);
    let temporary = configuration(1920, 1080, 60, 2);
    let mut runtime = FakeOutputRuntime::new(previous);
    runtime.direct_scanout_owned = true;

    apply(&mut runtime, temporary, 1_000);

    assert!(!runtime.direct_scanout_owned);
    assert!(runtime.direct_owned_during_test);
    assert!(runtime.direct_owned_during_commit);
    assert_eq!(runtime.direct_scanout_retire_count, 1);
    let ordered = [
        TransitionEvent::SafeBoundaryChecked,
        TransitionEvent::TestOnlyPassed,
        TransitionEvent::TransactionReserved,
        TransitionEvent::AcquireWatchesParked,
        TransitionEvent::KernelCommitted,
        TransitionEvent::CandidateAdopted,
        TransitionEvent::TransactionFinalized,
        TransitionEvent::DirectScanoutRetired,
        TransitionEvent::RuntimePublished,
        TransitionEvent::AcquireWatchesRearmed,
        TransitionEvent::ResponseQueued,
    ];
    let positions: Vec<_> = ordered
        .iter()
        .map(|expected| {
            runtime
                .events
                .iter()
                .position(|event| event == expected)
                .expect("transition event was recorded")
        })
        .collect();
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(!runtime.direct_validation_valid);
    assert!(runtime.next_frame_starts_composited);
    assert_eq!(runtime.scanout_dimensions, (1920, 1080));
}

#[test]
fn unsignaled_acquire_obligation_does_not_starve_mutation_and_rearm_is_bounded() {
    let previous = configuration(1920, 1080, 165, 1);
    let temporary = configuration(1920, 1080, 60, 2);
    let mut runtime = FakeOutputRuntime::new(previous);
    runtime.acquire_obligations.push(FakeAcquireObligation {
        id: 77,
        signaled: false,
        registered_generation: 1,
        rearmed_generation: None,
    });
    runtime.submitted_pageflip_owned = true;
    let operation = runtime.temporary_operation();

    assert_eq!(
        runtime.advance(operation, temporary, 10_000, None),
        OutputConfigurationAdvance::Waiting
    );
    assert!(
        runtime
            .events
            .iter()
            .all(|event| *event != TransitionEvent::AcquireWatchesParked)
    );
    assert_eq!(
        runtime.current.mode_id, previous.mode_id,
        "submitted KMS ownership must drain first"
    );

    runtime.submitted_pageflip_owned = false;
    runtime.fail_next_acquire_rearm = true;
    assert_eq!(
        runtime.advance(operation, temporary, 20_000, None),
        OutputConfigurationAdvance::Completed
    );
    assert_eq!(runtime.current.mode_id, temporary.mode_id);
    assert_eq!(runtime.output_epoch.get(), 2);
    assert_eq!(runtime.parked_acquire_obligations, vec![77]);
    assert_eq!(runtime.acquire_obligations[0].registered_generation, 1);
    assert_eq!(runtime.acquire_obligations[0].rearmed_generation, None);
    assert!(
        runtime.acquire_ready_notifications.is_empty(),
        "never synthesize acquire-ready"
    );
    let retry = runtime.acquire_rearm_retry_deadline_ns.unwrap();
    assert!(retry > runtime.last_commit_at_ns.unwrap());
    assert!(retry <= runtime.last_commit_at_ns.unwrap() + 10_000_000);

    runtime.rearm_acquire_watches(retry);
    assert!(runtime.parked_acquire_obligations.is_empty());
    assert_eq!(runtime.acquire_obligations[0].rearmed_generation, Some(2));
    assert_eq!(runtime.acquire_watch_generation, 2);
    assert!(runtime.acquire_ready_notifications.is_empty());
}

#[test]
fn late_persistence_after_timeout_is_compensated_without_confirming() {
    let previous = configuration(1920, 1080, 165, 1);
    let temporary = configuration(1920, 1080, 60, 2);
    let mut runtime = FakeOutputRuntime::new(previous);
    apply(&mut runtime, temporary, 100_000);
    let transaction_id = runtime.begin_keep(runtime.last_commit_at_ns.unwrap() + 1, None);
    let deadline = runtime.transactions.active().unwrap().rollback_deadline_ns;
    assert_eq!(
        runtime.transactions.active().unwrap().phase,
        OutputConfigurationTransactionPhase::PersistencePending
    );

    assert_eq!(
        runtime.advance_expired_rollback(deadline),
        OutputConfigurationAdvance::Completed
    );
    assert_eq!(
        runtime.current.mode_id, previous.mode_id,
        "rollback remains authoritative"
    );
    assert!(runtime.transactions.active().is_none());
    assert!(runtime.persistence_compensation_required);

    runtime.complete_persistence(true, deadline + 1);
    assert!(
        runtime.transactions.active().is_none(),
        "late completion cannot confirm a cleared transaction"
    );
    assert!(runtime.persistence_compensation_required);
    assert_eq!(runtime.current.mode_id, previous.mode_id);

    OutputConfigurationRuntime::begin_persistence(
        &mut runtime,
        OutputPersistencePurpose::Compensate,
        None,
        vec![2],
    )
    .expect("compensation job is submitted for restored mode");
    assert_eq!(
        runtime.persistence_job.as_ref().unwrap().target_mode_id,
        previous.mode_id
    );
    runtime.complete_persistence(true, deadline + 2);
    assert!(!runtime.persistence_compensation_required);
    assert!(!runtime.persistence_compensation_failed);
    assert_eq!(transaction_id.get(), 1);

    runtime.persistence_compensation_required = true;
    OutputConfigurationRuntime::begin_persistence(
        &mut runtime,
        OutputPersistencePurpose::Compensate,
        None,
        vec![3],
    )
    .expect("retry can be submitted");
    runtime.complete_persistence(false, deadline + 3);
    assert!(runtime.persistence_compensation_failed);
}
