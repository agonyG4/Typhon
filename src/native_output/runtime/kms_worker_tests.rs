use super::kms_worker::{
    FatalWorkerJobDisposition, FatalWorkerJobHandler, UncertainJobRetention,
    handle_fatal_worker_jobs, prepare_frozen_cursor_presentation_for_submission,
    retain_complete_submitted_ownership, retain_uncertain_job_with_suspension,
};
use super::kms_worker_teardown::SubmittedWorkerPacingState;
use super::plane_cycle::plane_delta_reservation_outcome;
use crate::native_output::kms_worker::{
    KmsBundleOwners, KmsCommitJob, KmsCursorUpdate, KmsPrimaryCursorPresentation, KmsPrimaryUpdate,
    KmsSubmittedOwnership, KmsTestOnlyPolicy, KmsValidationBase, KmsWorkerAdmissionError,
    KmsWorkerFatalJob,
};
use crate::native_output::pacing::NativeFramePacing;
use crate::native_output::runtime::AtomicCommitKind;
use crate::native_output::scanout::{
    DirectPrimaryLease, OutputFrameIdentitySnapshot, OutputFrameKey, OutputSlotId,
};
use crate::native_output::{
    ContentEpochId, DirectScanoutCandidateKey, NativeResult, OutputContentKey, OutputReleasePlan,
    OutputTransaction, OutputTransactionId, OutputTransactionLedger, OutputTransactionState,
};
use oblivion_one::compositor::{
    CompositorFrameBatchId, PresentationFeedbackBatchId, SurfaceCommitSequence,
    SurfacePresentationCommitKey,
};
use oblivion_one::native::kms::{FramebufferId, PageFlipToken};
use oblivion_one::native::presentation_deadline::{
    MonotonicTimestampNs, PresentationTarget, PresentationTargetReason,
};
use oblivion_one::native::scheduler::NativeOutputPacingMode;
use std::num::NonZeroU64;
use std::os::fd::{FromRawFd, OwnedFd};
use std::time::Duration;

struct RecordingFatalJobHandler {
    retained: Vec<KmsCommitJob>,
}

impl RecordingFatalJobHandler {
    fn new() -> Self {
        Self {
            retained: Vec::new(),
        }
    }
}

impl FatalWorkerJobHandler for RecordingFatalJobHandler {
    fn retain_uncertain_worker_job(
        &mut self,
        job: KmsCommitJob,
    ) -> NativeResult<UncertainJobRetention> {
        self.retained.push(job);
        Ok(UncertainJobRetention::Suspended)
    }

    fn fail_known_worker_job(&mut self, job: KmsCommitJob) -> NativeResult<()> {
        drop(job);
        Ok(())
    }

    fn drop_known_worker_job(&mut self, job: KmsCommitJob) -> NativeResult<()> {
        drop(job);
        Ok(())
    }
}

fn test_direct_key() -> DirectScanoutCandidateKey {
    DirectScanoutCandidateKey {
        output_id: oblivion_one::core::OutputId::from_raw(1).expect("nonzero output id"),
        content: OutputContentKey::new(
            7,
            std::num::NonZeroU64::new(42).expect("test buffer ID"),
            ContentEpochId::new(std::num::NonZeroU64::new(3).expect("test content epoch")),
            1920,
            1080,
            0x3432_5241,
            0,
            0,
            1_000,
            0,
        ),
        output_generation: 1,
        cursor_content_key: None,
        color_epoch: 0,
    }
}

fn pacing_test_physical_identity(frame_id: u64) -> OutputFrameIdentitySnapshot {
    OutputFrameIdentitySnapshot {
        output_id: oblivion_one::core::OutputId::from_raw(1).expect("test output id"),
        frame_id,
        protocol_batch_id: oblivion_one::compositor::CompositorFrameBatchId::new(
            std::num::NonZeroU64::new(frame_id).expect("test protocol batch ID"),
        ),
        transaction_id: OutputTransactionId::new(
            std::num::NonZeroU64::new(frame_id).expect("test transaction ID"),
        ),
        slot: OutputSlotId::new(1).expect("test output slot ID"),
        framebuffer_id: FramebufferId::new(frame_id as u32).expect("test framebuffer ID"),
        render_generation: frame_id,
        pool_generation: 1,
        target: None,
    }
}

fn test_uncertain_direct_job(lease: DirectPrimaryLease) -> KmsCommitJob {
    let token = PageFlipToken::new(70).expect("test token");
    let transaction_id =
        OutputTransactionId::new(std::num::NonZeroU64::new(70).expect("test transaction ID"));
    KmsCommitJob {
        bundle_id:
            crate::native_output::presentation::plane::KmsCommitBundleId::from_pageflip_token(token),
        owners: KmsBundleOwners::legacy_unchecked(),
        output_id: oblivion_one::core::OutputId::from_raw(1).expect("nonzero output id"),
        transaction_id,
        token,
        output_generation: 1,
        crtc_id: 7,
        kind: AtomicCommitKind::DirectPrimary {
            transaction_id,
            direct_token: token,
            framebuffer_id: 42,
        },
        submit_window: crate::native_output::presentation::kms_timing::KmsSubmitWindow::try_new(
            0, 0, 0, 0,
        )
        .unwrap(),
        target: PresentationTarget {
            sequence: 70,
            presentation_time: MonotonicTimestampNs::new(0),
            submit_not_before: MonotonicTimestampNs::new(0),
            render_start_deadline: MonotonicTimestampNs::new(0),
            refresh_interval: Duration::from_millis(16),
            reason: PresentationTargetReason::ReactiveDouble,
            clock_generation: 1,
            estimated: true,
            predicted_unreachable: false,
            physical_claim: oblivion_one::native::presentation_deadline::PrimaryRefreshClaim {
                sequence: 70,
                presentation_time: MonotonicTimestampNs::new(0),
                clock_generation: 1,
            },
            selection_evidence: Default::default(),
        },
        validation_base: KmsValidationBase::Presented {
            output_id: oblivion_one::core::OutputId::from_raw(1).expect("nonzero output id"),
            snapshot: crate::native_output::presentation::plane::PresentedPlaneSnapshot::legacy(
                None,
            ),
            output_generation: 1,
            crtc_id: 7,
        },
        queued_at: MonotonicTimestampNs::new(0),
        primary: KmsPrimaryUpdate::Framebuffer {
            framebuffer: FramebufferId::new(42).expect("test framebuffer ID"),
            in_fence: Some(test_eventfd()),
            request_out_fence: false,
        },
        cursor: KmsCursorUpdate::Unchanged,
        cursor_delivery: crate::native_output::presentation::plane::PresentedCursorDelivery::Hidden,
        primary_cursor_presentation: KmsPrimaryCursorPresentation::Preserve,
        cursor_pin: None,
        direct_primary_lease: Some(lease),
        test_only_duration_ns: None,
        pacing_ticket: None,
        test_policy: crate::native_output::kms_worker::KmsCommitTestPolicy::from_primary(
            KmsTestOnlyPolicy::Skip,
        ),
        ready_submit: false,
    }
}

fn test_eventfd() -> OwnedFd {
    let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
    assert!(fd >= 0, "test eventfd should be created");
    // SAFETY: eventfd returned a new owned descriptor for this test.
    unsafe { OwnedFd::from_raw_fd(fd) }
}

fn worker_freeze_test_target() -> PresentationTarget {
    let now = MonotonicTimestampNs::new(10);
    PresentationTarget {
        sequence: 2,
        presentation_time: now,
        submit_not_before: now,
        render_start_deadline: now,
        refresh_interval: Duration::from_millis(10),
        reason: PresentationTargetReason::ReactiveDouble,
        clock_generation: 1,
        estimated: false,
        predicted_unreachable: false,
        physical_claim: oblivion_one::native::presentation_deadline::PrimaryRefreshClaim {
            sequence: 2,
            presentation_time: now,
            clock_generation: 1,
        },
        selection_evidence: Default::default(),
    }
}

fn worker_freeze_cursor_key() -> SurfacePresentationCommitKey {
    SurfacePresentationCommitKey {
        surface_id: 77,
        presentation_generation: 3,
        commit_sequence: SurfaceCommitSequence(1),
    }
}

#[derive(Clone, Copy)]
enum WorkerFreezePrimaryKind {
    Composited,
    Direct,
}

fn worker_freeze_fixture(
    primary_key: Option<SurfacePresentationCommitKey>,
    sidecar_key: Option<SurfacePresentationCommitKey>,
    with_feedback: bool,
    primary_kind: WorkerFreezePrimaryKind,
) -> (
    OutputTransactionLedger,
    OutputTransactionId,
    OutputTransactionId,
    Option<PresentationFeedbackBatchId>,
) {
    let mut ledger = OutputTransactionLedger::with_capacities(8, 64);
    let primary_id = ledger.allocate_id().expect("primary transaction ID");
    let presentation_batch_id = with_feedback.then(|| {
        PresentationFeedbackBatchId::new(
            NonZeroU64::new(700).expect("presentation batch ID is nonzero"),
        )
    });
    let mut primary = match primary_kind {
        WorkerFreezePrimaryKind::Composited => OutputTransaction::composited(
            ledger.output_id(),
            primary_id,
            1,
            MonotonicTimestampNs::new(1),
            worker_freeze_test_target(),
            NativeOutputPacingMode::ReactiveDouble,
            11,
            12,
            13,
            OutputSlotId::new(0).expect("slot ID"),
            91,
            None,
            CompositorFrameBatchId::new(NonZeroU64::new(701).expect("frame batch ID")),
        )
        .expect("composited primary transaction"),
        WorkerFreezePrimaryKind::Direct => OutputTransaction::direct(
            ledger.output_id(),
            primary_id,
            1,
            MonotonicTimestampNs::new(1),
            worker_freeze_test_target(),
            NativeOutputPacingMode::ReactiveDouble,
            11,
            test_direct_key(),
            91,
            None,
            CompositorFrameBatchId::new(NonZeroU64::new(701).expect("frame batch ID")),
            77,
            OutputReleasePlan::Pageflip,
        )
        .expect("direct primary transaction"),
    }
    .with_client_cursor_presentation_key(primary_key);
    if let Some(batch_id) = presentation_batch_id {
        primary = primary
            .with_presentation_feedback_batch(batch_id)
            .expect("cursor presentation feedback batch");
    }
    ledger.insert(primary).expect("insert primary");
    ledger
        .mark_queued(primary_id, 1, MonotonicTimestampNs::new(2))
        .expect("primary queued");

    let sidecar_id = ledger.allocate_id().expect("sidecar transaction ID");
    let sidecar = OutputTransaction::cursor_plane_delta(
        ledger.output_id(),
        sidecar_id,
        1,
        MonotonicTimestampNs::new(2),
        worker_freeze_test_target(),
        NativeOutputPacingMode::ReactiveDouble,
        3,
        Some(oblivion_one::native::kms::AtomicCursorVisualState {
            visible: true,
            x: 0,
            y: 0,
            hotspot_x: 0,
            hotspot_y: 0,
            width: 64,
            height: 64,
            framebuffer_id: Some(92),
            image_generation: 1,
        }),
        OutputReleasePlan::Pageflip,
    )
    .expect("sidecar transaction")
    .with_client_cursor_presentation_key(sidecar_key);
    ledger.insert(sidecar).expect("insert sidecar");
    (ledger, primary_id, sidecar_id, presentation_batch_id)
}

fn submit_worker_freeze_bundle(
    ledger: &mut OutputTransactionLedger,
    primary_id: OutputTransactionId,
    sidecar_id: Option<OutputTransactionId>,
) {
    let token = PageFlipToken::new(702).expect("submit token");
    ledger
        .mark_submitted(primary_id, token, MonotonicTimestampNs::new(3))
        .expect("primary submitted");
    if let Some(sidecar_id) = sidecar_id {
        ledger
            .mark_submitted(sidecar_id, token, MonotonicTimestampNs::new(3))
            .expect("sidecar submitted");
    }
}

#[test]
fn real_worker_success_rebinds_before_submit_transition() {
    let (mut ledger, primary_id, sidecar_id, presentation_batch_id) = worker_freeze_fixture(
        Some(worker_freeze_cursor_key()),
        Some(worker_freeze_cursor_key()),
        true,
        WorkerFreezePrimaryKind::Composited,
    );

    let socket_name = format!("typhon-kms-worker-freeze-red-{}", std::process::id());
    let mut server =
        oblivion_one::compositor::OwnCompositorServer::bind_cpu_composition(&socket_name)
            .expect("bind worker-freeze test compositor");
    let result = prepare_frozen_cursor_presentation_for_submission(
        &mut server,
        &mut ledger,
        primary_id,
        sidecar_id,
        1,
        MonotonicTimestampNs::new(2),
    );

    result.expect("pre-submit worker rebind");
    assert!(matches!(
        ledger
            .transaction(primary_id)
            .expect("primary transaction")
            .state(),
        OutputTransactionState::Queued { .. }
    ));
    assert!(matches!(
        ledger
            .transaction(sidecar_id)
            .expect("sidecar transaction")
            .state(),
        OutputTransactionState::Queued { .. }
    ));
    submit_worker_freeze_bundle(&mut ledger, primary_id, Some(sidecar_id));
    assert_eq!(
        ledger.presentation_feedback_owner(presentation_batch_id.expect("feedback batch")),
        Some(sidecar_id)
    );
    assert!(matches!(
        ledger
            .transaction(primary_id)
            .expect("submitted primary")
            .state(),
        OutputTransactionState::Submitted { .. }
    ));
    assert!(matches!(
        ledger
            .transaction(sidecar_id)
            .expect("submitted sidecar")
            .state(),
        OutputTransactionState::Submitted { .. }
    ));
}

#[test]
fn real_worker_success_rebind_without_feedback_is_legal() {
    let (mut ledger, primary_id, sidecar_id, presentation_batch_id) = worker_freeze_fixture(
        Some(worker_freeze_cursor_key()),
        Some(worker_freeze_cursor_key()),
        false,
        WorkerFreezePrimaryKind::Composited,
    );
    let socket_name = format!(
        "typhon-kms-worker-freeze-no-feedback-{}",
        std::process::id()
    );
    let mut server =
        oblivion_one::compositor::OwnCompositorServer::bind_cpu_composition(&socket_name)
            .expect("bind worker-freeze no-feedback test compositor");
    prepare_frozen_cursor_presentation_for_submission(
        &mut server,
        &mut ledger,
        primary_id,
        sidecar_id,
        1,
        MonotonicTimestampNs::new(2),
    )
    .expect("no-feedback worker rebind");
    assert!(presentation_batch_id.is_none());
    submit_worker_freeze_bundle(&mut ledger, primary_id, Some(sidecar_id));
    assert!(matches!(
        ledger
            .transaction(primary_id)
            .expect("submitted primary")
            .state(),
        OutputTransactionState::Submitted { .. }
    ));
    assert!(matches!(
        ledger
            .transaction(sidecar_id)
            .expect("submitted sidecar")
            .state(),
        OutputTransactionState::Submitted { .. }
    ));
}

#[test]
fn real_worker_success_content_change_detaches_old_feedback() {
    let key_c1 = worker_freeze_cursor_key();
    let key_c2 = SurfacePresentationCommitKey {
        commit_sequence: SurfaceCommitSequence(2),
        ..key_c1
    };
    let (mut ledger, primary_id, sidecar_id, presentation_batch_id) = worker_freeze_fixture(
        Some(key_c1),
        Some(key_c2),
        true,
        WorkerFreezePrimaryKind::Composited,
    );
    let presentation_batch_id = presentation_batch_id.expect("feedback batch");
    let sidecar_presentation_batch_id = PresentationFeedbackBatchId::new(
        NonZeroU64::new(704).expect("sidecar presentation batch ID is nonzero"),
    );
    ledger
        .attach_presentation_feedback_batch(sidecar_id, sidecar_presentation_batch_id)
        .expect("sidecar presentation feedback batch");
    let socket_name = format!(
        "typhon-kms-worker-freeze-content-change-{}",
        std::process::id()
    );
    let mut server =
        oblivion_one::compositor::OwnCompositorServer::bind_cpu_composition(&socket_name)
            .expect("bind worker-freeze content-change test compositor");
    prepare_frozen_cursor_presentation_for_submission(
        &mut server,
        &mut ledger,
        primary_id,
        sidecar_id,
        1,
        MonotonicTimestampNs::new(2),
    )
    .expect("content-changing worker rebind");
    assert_eq!(
        ledger.presentation_feedback_owner(presentation_batch_id),
        None
    );
    assert_eq!(
        ledger.presentation_feedback_owner(sidecar_presentation_batch_id),
        Some(sidecar_id)
    );
    assert_eq!(
        ledger
            .transaction(primary_id)
            .expect("primary transaction")
            .descriptor()
            .obligations()
            .presentation_feedback_batch_id(),
        None
    );
    submit_worker_freeze_bundle(&mut ledger, primary_id, Some(sidecar_id));
    assert_eq!(
        ledger.presentation_feedback_owner(sidecar_presentation_batch_id),
        Some(sidecar_id)
    );
}

#[test]
fn direct_worker_success_rebinds_before_submit_transition() {
    let (mut ledger, primary_id, sidecar_id, presentation_batch_id) = worker_freeze_fixture(
        Some(worker_freeze_cursor_key()),
        Some(worker_freeze_cursor_key()),
        true,
        WorkerFreezePrimaryKind::Direct,
    );
    let presentation_batch_id = presentation_batch_id.expect("feedback batch");
    let socket_name = format!("typhon-kms-worker-direct-freeze-{}", std::process::id());
    let mut server =
        oblivion_one::compositor::OwnCompositorServer::bind_cpu_composition(&socket_name)
            .expect("bind direct worker-freeze test compositor");
    prepare_frozen_cursor_presentation_for_submission(
        &mut server,
        &mut ledger,
        primary_id,
        sidecar_id,
        1,
        MonotonicTimestampNs::new(2),
    )
    .expect("direct pre-submit worker rebind");
    assert_eq!(
        ledger.presentation_feedback_owner(presentation_batch_id),
        Some(sidecar_id)
    );
    submit_worker_freeze_bundle(&mut ledger, primary_id, Some(sidecar_id));
    assert!(matches!(
        ledger
            .transaction(primary_id)
            .expect("submitted primary")
            .state(),
        OutputTransactionState::Submitted { .. }
    ));
    assert!(matches!(
        ledger
            .transaction(sidecar_id)
            .expect("submitted sidecar")
            .state(),
        OutputTransactionState::Submitted { .. }
    ));
}

#[test]
fn primary_only_worker_success_preserves_primary_presentation_owner() {
    let (mut ledger, primary_id, _sidecar_id, presentation_batch_id) = worker_freeze_fixture(
        Some(worker_freeze_cursor_key()),
        None,
        true,
        WorkerFreezePrimaryKind::Composited,
    );
    let presentation_batch_id = presentation_batch_id.expect("feedback batch");

    submit_worker_freeze_bundle(&mut ledger, primary_id, None);

    assert_eq!(
        ledger.presentation_feedback_owner(presentation_batch_id),
        Some(primary_id)
    );
    assert!(matches!(
        ledger
            .transaction(primary_id)
            .expect("submitted primary")
            .state(),
        OutputTransactionState::Submitted { .. }
    ));
}

#[test]
fn occupied_worker_plane_delta_reservation_is_retryable_contention() {
    assert_eq!(
        plane_delta_reservation_outcome(Err("an Atomic worker commit is already queued")),
        Err(KmsWorkerAdmissionError::QueueFull)
    );
}

#[test]
fn shared_fatal_handler_retains_uncertain_job_resources_once() {
    let key = test_direct_key();
    let lease = DirectPrimaryLease::test_fixture(key, 42);
    let fatal_job = KmsWorkerFatalJob {
        job: test_uncertain_direct_job(lease),
        uncertain_submit: true,
    };
    let mut handler = RecordingFatalJobHandler::new();

    assert_eq!(
        handle_fatal_worker_jobs([fatal_job], &mut handler, FatalWorkerJobDisposition::Drop,)
            .unwrap(),
        vec![UncertainJobRetention::Suspended]
    );
    assert_eq!(handler.retained.len(), 1);
    assert!(handler.retained[0].direct_primary_lease.is_some());
    assert!(matches!(
        handler.retained[0].primary,
        KmsPrimaryUpdate::Framebuffer {
            in_fence: Some(_),
            ..
        }
    ));

    let (lease, cleanup_count) = DirectPrimaryLease::test_fixture_with_probe(key, 42);
    let known_job = KmsWorkerFatalJob {
        job: test_uncertain_direct_job(lease),
        uncertain_submit: false,
    };
    assert!(
        handle_fatal_worker_jobs([known_job], &mut handler, FatalWorkerJobDisposition::Fail,)
            .unwrap()
            .is_empty()
    );
    assert_eq!(cleanup_count.load(std::sync::atomic::Ordering::Acquire), 1);
}

#[test]
fn runtime_quarantines_uncertain_direct_job() {
    let key = test_direct_key();
    let (lease, cleanup_count) = DirectPrimaryLease::test_fixture_with_probe(key, 42);
    let job = test_uncertain_direct_job(lease);
    let mut suspended_jobs = Vec::new();
    let mut emergency_jobs = Vec::new();

    assert_eq!(
        retain_uncertain_job_with_suspension(job, &mut suspended_jobs, &mut emergency_jobs,)
            .unwrap(),
        UncertainJobRetention::EmergencyQuarantined
    );
    assert!(suspended_jobs.is_empty());
    assert_eq!(emergency_jobs.len(), 1);
    assert!(emergency_jobs[0].direct_primary_lease.is_some());
    assert!(matches!(
        emergency_jobs[0].primary,
        KmsPrimaryUpdate::Framebuffer {
            in_fence: Some(_),
            ..
        }
    ));
    assert_eq!(cleanup_count.load(std::sync::atomic::Ordering::Acquire), 0);

    emergency_jobs.clear();
    emergency_jobs.clear();
    assert_eq!(cleanup_count.load(std::sync::atomic::Ordering::Acquire), 1);
}

#[test]
fn runtime_suspension_retains_job_until_normal_recovery_cleanup() {
    let key = test_direct_key();
    let (lease, cleanup_count) = DirectPrimaryLease::test_fixture_with_probe(key, 42);
    let job = test_uncertain_direct_job(lease);
    let mut suspended_jobs = Vec::new();
    let mut emergency_jobs = Vec::new();

    assert_eq!(
        retain_uncertain_job_with_suspension(job, &mut suspended_jobs, &mut emergency_jobs,)
            .unwrap(),
        UncertainJobRetention::EmergencyQuarantined
    );
    assert!(suspended_jobs.is_empty());
    assert_eq!(emergency_jobs.len(), 1);
    assert!(emergency_jobs[0].direct_primary_lease.is_some());
    assert_eq!(cleanup_count.load(std::sync::atomic::Ordering::Acquire), 0);

    emergency_jobs.clear();
    assert_eq!(cleanup_count.load(std::sync::atomic::Ordering::Acquire), 1);
}

#[test]
fn promotion_failure_quarantine_retains_complete_submitted_ownership() {
    let key = test_direct_key();
    let (lease, cleanup_count) = DirectPrimaryLease::test_fixture_with_probe(key, 42);
    let ownership = KmsSubmittedOwnership {
        job: test_uncertain_direct_job(lease),
        out_fence: Some(test_eventfd()),
        planned_worker_wake_at: MonotonicTimestampNs::new(0),
        actual_worker_wait_returned_at: MonotonicTimestampNs::new(0),
        submit_started_at: MonotonicTimestampNs::new(1),
        submit_returned_at: MonotonicTimestampNs::new(2),
        queue_residency_ns: 0,
        submit_wake_lateness_ns: 0,
        pre_submit_duration_ns: 0,
        ioctl_duration_ns: 1,
        dispatch_duration_ns: 1,
        submission_budget_ns: 1_000_000,
        dispatch_tail_observation: None,
    };
    let mut emergency = Vec::new();

    retain_complete_submitted_ownership(ownership, &mut emergency);

    assert_eq!(emergency.len(), 1);
    assert!(emergency[0].job.direct_primary_lease.is_some());
    assert!(emergency[0].out_fence.is_some());
    assert_eq!(cleanup_count.load(std::sync::atomic::Ordering::Acquire), 0);
    emergency.clear();
    assert_eq!(cleanup_count.load(std::sync::atomic::Ordering::Acquire), 1);
}

#[test]
fn submitted_worker_integration_failure_abandons_exact_pending_token() {
    let mut pacing = NativeFramePacing::from_env();
    pacing.queue_visual(1, 1);
    let logical_frame = pacing.active.expect("normal predecessor");
    let predecessor_ticket = pacing
        .reserve_worker_submission(false)
        .unwrap()
        .expect("predecessor worker ticket");
    let predecessor_reservation_id = predecessor_ticket.reservation_id();
    assert_eq!(predecessor_ticket.frame_id(), logical_frame);

    pacing
        .note_render_started(NativeOutputPacingMode::PredictiveTriple, true)
        .unwrap();
    let successor_attempt = pacing
        .active_predictive_attempt_id()
        .expect("predictive successor attempt");
    let successor_physical = pacing_test_physical_identity(5_002);
    let successor_key = OutputFrameKey::from(&successor_physical);
    pacing
        .bind_predictive_o1(successor_physical)
        .expect("bind predictive successor physical identity");
    pacing.note_render_ready();
    pacing.note_ready_frame(2, true);

    assert_eq!(pacing.ready, Some(logical_frame));
    assert_eq!(
        pacing.ready_predictive_attempt_id(),
        Some(successor_attempt)
    );
    assert_eq!(pacing.ready_physical_key(), Some(successor_key));
    assert_eq!(pacing.predictive_o1_active_entries_for_test(), 1);

    pacing
        .note_worker_submit_exact(
            Some(predecessor_ticket),
            41,
            3,
            NativeOutputPacingMode::PredictiveTriple,
        )
        .unwrap();
    assert_eq!(pacing.pending, Some(logical_frame));
    let result = super::NativeRuntime::finish_submitted_worker_pacing(
        &mut pacing,
        Some(SubmittedWorkerPacingState::new(
            PageFlipToken::new(41).unwrap(),
        )),
        Err(std::io::Error::other("integration failure").into()),
    );

    assert!(result.is_err());
    assert!(pacing.pending.is_none());
    assert!(!pacing.abandon_pending_submission(41));
    assert_eq!(pacing.ready, Some(logical_frame));
    assert_eq!(
        pacing.ready_predictive_attempt_id(),
        Some(successor_attempt)
    );
    assert_eq!(pacing.ready_physical_key(), Some(successor_key));
    assert_eq!(pacing.predictive_o1_active_entries_for_test(), 1);
    assert_eq!(pacing.predictive_o1_invalid_stage_transitions, 0);
    assert_eq!(pacing.predictive_o1_presented, 0);

    let successor_ticket = pacing
        .reserve_worker_submission(true)
        .unwrap()
        .expect("successor worker ticket");
    assert_ne!(
        successor_ticket.reservation_id(),
        predecessor_reservation_id
    );
    assert_eq!(successor_ticket.frame_id(), logical_frame);
    assert_eq!(
        successor_ticket.predictive_attempt_id(),
        Some(successor_attempt)
    );
    assert_eq!(successor_ticket.physical_key(), Some(successor_key));
    pacing
        .note_worker_submit_exact(
            Some(successor_ticket),
            42,
            4,
            NativeOutputPacingMode::PredictiveTriple,
        )
        .expect("successor worker submission");
    pacing.note_pageflip_exact(Some(successor_physical), 5, 4, 42, 6_060);

    assert_eq!(pacing.predictive_o1_presented, 1);
    assert_eq!(pacing.predictive_o1_invalid_stage_transitions, 0);
    assert_eq!(pacing.predictive_o1_active_entries_for_test(), 0);
    assert!(pacing.reserve_worker_submission(true).unwrap().is_none());
    assert!(pacing.pending.is_none());
}

#[test]
fn submitted_worker_integration_success_keeps_exact_pending_token_for_pageflip() {
    let mut pacing = NativeFramePacing::from_env();
    pacing.queue_visual(1, 1);
    let ticket = pacing
        .reserve_worker_submission(false)
        .unwrap()
        .expect("worker ticket");
    pacing
        .note_worker_submit_exact(Some(ticket), 42, 3, NativeOutputPacingMode::ReactiveDouble)
        .unwrap();

    super::NativeRuntime::finish_submitted_worker_pacing(
        &mut pacing,
        Some(SubmittedWorkerPacingState::new(
            PageFlipToken::new(42).unwrap(),
        )),
        Ok(()),
    )
    .unwrap();
    assert_eq!(pacing.pending, Some(ticket.frame_id()));
    pacing.note_pageflip_exact(None, 43, 3, 42, 6_060);
    assert!(pacing.pending.is_none());
}

#[test]
fn teardown_safety_only_allows_release_after_a_proven_boundary() {
    assert!(super::KmsTeardownSafety::Restored.permits_release());
    assert!(super::KmsTeardownSafety::TargetDestroyed.permits_release());
    assert!(!super::KmsTeardownSafety::Unproven.permits_release());
}

#[test]
fn teardown_safety_requires_an_explicit_boundary_proof() {
    assert_eq!(
        super::kms_worker_teardown::classify_kms_teardown_safety(None),
        super::KmsTeardownSafety::Unproven
    );
    assert_eq!(
        super::kms_worker_teardown::classify_kms_teardown_safety(Some(
            super::KmsSafeBoundary::Restored,
        )),
        super::KmsTeardownSafety::Restored
    );
    assert_eq!(
        super::kms_worker_teardown::classify_kms_teardown_safety(Some(
            super::KmsSafeBoundary::TargetDestroyed,
        )),
        super::KmsTeardownSafety::TargetDestroyed
    );
}

#[test]
fn inactive_seat_without_target_destruction_proof_is_unproven() {
    let mut session = super::NativeSessionLifecycle::default();
    assert_eq!(
        session.begin_for_event(crate::native_output::NativeSeatEvent::Disabled),
        Some(super::NativeSessionTransition::BeginSuspend)
    );
    session.finish_suspend();
    assert!(!session.permits_output());
    assert_eq!(
        super::kms_worker_teardown::classify_kms_teardown_safety(None),
        super::KmsTeardownSafety::Unproven
    );
}

#[test]
fn restoration_outcomes_only_produce_matching_boundary_proofs() {
    use oblivion_one::native::kms::RestorationOutcome;

    assert_eq!(
        super::kms_worker_teardown::proof_from_restoration(RestorationOutcome::Exact),
        Some(super::KmsSafeBoundary::Restored)
    );
    assert_eq!(
        super::kms_worker_teardown::proof_from_restoration(RestorationOutcome::AlreadyRestored),
        Some(super::KmsSafeBoundary::Restored)
    );
    assert_eq!(
        super::kms_worker_teardown::proof_from_restoration(RestorationOutcome::SafeDisable),
        Some(super::KmsSafeBoundary::TargetDestroyed)
    );
    assert_eq!(
        super::kms_worker_teardown::proof_from_restoration(RestorationOutcome::Unavailable),
        None
    );
}
