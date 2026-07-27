use super::kms_worker::{
    FatalWorkerJobDisposition, FatalWorkerJobHandler, UncertainJobRetention,
    handle_fatal_worker_jobs, retain_uncertain_job_with_suspension,
};
use crate::native_output::kms_worker::{
    KmsCommitJob, KmsCursorUpdate, KmsPrimaryUpdate, KmsTestOnlyPolicy, KmsWorkerFatalJob,
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
        cursor_plan_key: None,
        color_epoch: 0,
    }
}

fn test_uncertain_direct_job(lease: DirectPrimaryLease) -> KmsCommitJob {
    let token = PageFlipToken::new(70).expect("test token");
    let transaction_id =
        OutputTransactionId::new(std::num::NonZeroU64::new(70).expect("test transaction ID"));
    KmsCommitJob {
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
        queued_at: MonotonicTimestampNs::new(0),
        primary: KmsPrimaryUpdate::Framebuffer {
            framebuffer: FramebufferId::new(42).expect("test framebuffer ID"),
            in_fence: Some(test_eventfd()),
            request_out_fence: false,
        },
        cursor: KmsCursorUpdate::Unchanged,
        cursor_pin: None,
        direct_primary_lease: Some(lease),
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
fn runtime_retains_complete_job_when_direct_suspension_fails() {
    let key = test_direct_key();
    let (lease, cleanup_count) = DirectPrimaryLease::test_fixture_with_probe(key, 42);
    let job = test_uncertain_direct_job(lease);
    let mut suspended_jobs = Vec::new();
    let mut emergency_jobs = Vec::new();

    assert_eq!(
        retain_uncertain_job_with_suspension(
            job,
            &mut suspended_jobs,
            &mut emergency_jobs,
            |_token, lease| {
                Err(Box::new((
                    std::io::Error::other("injected suspension failure"),
                    lease,
                )))
            },
        )
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
    let mut suspended_lease = None;

    assert_eq!(
        retain_uncertain_job_with_suspension(
            job,
            &mut suspended_jobs,
            &mut emergency_jobs,
            |_token, lease| {
                suspended_lease = Some(lease);
                Ok(())
            },
        )
        .unwrap(),
        UncertainJobRetention::Suspended
    );
    assert_eq!(suspended_jobs.len(), 1);
    assert!(suspended_jobs[0].direct_primary_lease.is_none());
    assert!(emergency_jobs.is_empty());
    assert_eq!(cleanup_count.load(std::sync::atomic::Ordering::Acquire), 0);

    suspended_jobs.clear();
    suspended_jobs.clear();
    drop(suspended_lease.take());
    assert_eq!(cleanup_count.load(std::sync::atomic::Ordering::Acquire), 1);
}
