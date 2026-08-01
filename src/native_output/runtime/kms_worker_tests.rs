use super::kms_worker::{
    FatalWorkerJobDisposition, FatalWorkerJobHandler, UncertainJobRetention,
    handle_fatal_worker_jobs, retain_complete_submitted_ownership,
    retain_uncertain_job_with_suspension,
};
use super::plane_cycle::plane_delta_reservation_outcome;
use crate::native_output::kms_worker::{
    KmsBundleOwners, KmsCommitJob, KmsCursorUpdate, KmsPrimaryUpdate, KmsSubmittedOwnership,
    KmsTestOnlyPolicy, KmsValidationBase, KmsWorkerAdmissionError, KmsWorkerFatalJob,
};
use crate::native_output::runtime::AtomicCommitKind;
use crate::native_output::scanout::DirectPrimaryLease;
use crate::native_output::{
    ContentEpochId, DirectScanoutCandidateKey, NativeResult, OutputContentKey, OutputTransactionId,
};
use oblivion_one::native::kms::{FramebufferId, PageFlipToken};
use oblivion_one::native::presentation_deadline::{
    MonotonicTimestampNs, PresentationTarget, PresentationTargetReason,
};
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

fn test_uncertain_direct_job(lease: DirectPrimaryLease) -> KmsCommitJob {
    let token = PageFlipToken::new(70).expect("test token");
    let transaction_id =
        OutputTransactionId::new(std::num::NonZeroU64::new(70).expect("test transaction ID"));
    KmsCommitJob {
        bundle_id:
            crate::native_output::presentation::plane::KmsCommitBundleId::from_pageflip_token(token),
        owners: KmsBundleOwners::legacy_unchecked(),
        transaction_id,
        token,
        output_generation: 1,
        crtc_id: 7,
        kind: AtomicCommitKind::DirectPrimary {
            transaction_id,
            direct_token: token,
            framebuffer_id: 42,
        },
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
        },
        validation_base: KmsValidationBase::Presented {
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
        cursor_pin: None,
        direct_primary_lease: Some(lease),
        test_only_duration_ns: None,
        pacing_frame_id: None,
        test_only: KmsTestOnlyPolicy::Skip,
        ready_submit: false,
    }
}

fn test_eventfd() -> OwnedFd {
    let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
    assert!(fd >= 0, "test eventfd should be created");
    // SAFETY: eventfd returned a new owned descriptor for this test.
    unsafe { OwnedFd::from_raw_fd(fd) }
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
        submit_started_at: MonotonicTimestampNs::new(1),
        submit_returned_at: MonotonicTimestampNs::new(2),
        queue_residency_ns: 0,
        submit_wake_lateness_ns: 0,
        submission_budget_ns: 1_000_000,
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
