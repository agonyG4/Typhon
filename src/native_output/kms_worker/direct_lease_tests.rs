use super::tests::{reserve_for_test, test_job, wait_for_fence_event};
use super::thread::{KmsCommitExecutor, KmsWorkerSubmission, KmsWorkerSubmitFailure};
use super::{
    KmsCommitJob, KmsCommitPayloadError, KmsCommitWorkerHandle, KmsPrimaryUpdate,
    KmsTestOnlyPolicy, KmsWorkerEvent,
};
use crate::native_output::scanout::DirectPrimaryLease;
use crate::native_output::{
    ContentEpochId, DirectScanoutCandidateKey, OutputContentKey, OutputReleasePlan,
    OutputTransaction, OutputTransactionId, runtime::AtomicCommitKind,
};
use oblivion_one::native::presentation_deadline::{
    MonotonicTimestampNs, PresentationTarget, PresentationTargetReason,
};
use oblivion_one::native::scheduler::NativeOutputPacingMode;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};

fn test_direct_key(content_epoch: u64) -> DirectScanoutCandidateKey {
    DirectScanoutCandidateKey {
        content: OutputContentKey::new(
            7,
            std::num::NonZeroU64::new(42).expect("test buffer id"),
            ContentEpochId::new(
                std::num::NonZeroU64::new(content_epoch).expect("test content epoch"),
            ),
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

fn test_target() -> PresentationTarget {
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
    }
}

fn test_direct_transaction(
    token: u64,
    key: DirectScanoutCandidateKey,
    framebuffer_id: u32,
) -> OutputTransaction {
    test_direct_transaction_with_surface_id(token, key, framebuffer_id, key.content.surface_id)
}

fn test_direct_transaction_with_surface_id(
    token: u64,
    key: DirectScanoutCandidateKey,
    framebuffer_id: u32,
    direct_surface_id: u32,
) -> OutputTransaction {
    OutputTransaction::direct(
        OutputTransactionId::new(std::num::NonZeroU64::new(token).expect("transaction id")),
        1,
        MonotonicTimestampNs::new(10),
        test_target(),
        NativeOutputPacingMode::ReactiveDouble,
        token,
        key,
        framebuffer_id,
        None,
        oblivion_one::compositor::CompositorFrameBatchId::new(
            std::num::NonZeroU64::new(token).expect("frame batch id"),
        ),
        direct_surface_id,
        OutputReleasePlan::Pageflip,
    )
    .expect("direct transaction")
}

fn test_direct_job(
    token: u64,
    key: DirectScanoutCandidateKey,
    framebuffer_id: u32,
    lease: Option<DirectPrimaryLease>,
) -> KmsCommitJob {
    let mut job = test_job(token);
    job.kind = AtomicCommitKind::DirectPrimary {
        transaction_id: job.transaction_id,
        direct_token: job.token,
        framebuffer_id,
    };
    job.primary = KmsPrimaryUpdate::Framebuffer {
        framebuffer: oblivion_one::native::kms::FramebufferId::new(framebuffer_id)
            .expect("framebuffer id"),
        in_fence: None,
        request_out_fence: false,
    };
    job.target = test_target();
    job.direct_primary_lease = lease;
    let _ = key;
    job
}

#[test]
fn direct_job_requires_matching_owned_primary_resource() {
    let key = test_direct_key(3);
    let transaction = test_direct_transaction(60, key, 42);
    let job = test_direct_job(60, key, 42, None);

    assert_eq!(
        job.validate_against(&transaction),
        Err(KmsCommitPayloadError::DirectPrimaryResourceMismatch)
    );
}

#[test]
fn direct_job_accepts_matching_owned_primary_resource() {
    let key = test_direct_key(3);
    let transaction = test_direct_transaction(600, key, 42);
    let lease = DirectPrimaryLease::test_fixture(key, 42);
    let job = test_direct_job(600, key, 42, Some(lease));

    assert_eq!(job.validate_against(&transaction), Ok(()));
}

#[test]
fn direct_job_rejects_lease_with_wrong_surface_identity() {
    let key = test_direct_key(3);
    let transaction = test_direct_transaction_with_surface_id(601, key, 42, 8);
    let (lease, cleanup_count) = DirectPrimaryLease::test_fixture_with_probe(key, 42);
    let job = test_direct_job(601, key, 42, Some(lease));

    assert_eq!(
        job.validate_against(&transaction),
        Err(KmsCommitPayloadError::DirectPrimaryResourceMismatch)
    );
    drop(job);
    assert_eq!(cleanup_count.load(std::sync::atomic::Ordering::Acquire), 1);
}

#[test]
fn direct_job_rejects_lease_with_wrong_framebuffer() {
    let key = test_direct_key(3);
    let transaction = test_direct_transaction(61, key, 42);
    let lease = DirectPrimaryLease::test_fixture(key, 43);
    let job = test_direct_job(61, key, 42, Some(lease));

    assert_eq!(
        job.validate_against(&transaction),
        Err(KmsCommitPayloadError::DirectPrimaryResourceMismatch)
    );
}

#[test]
fn direct_job_rejects_lease_with_wrong_candidate_key() {
    let key = test_direct_key(3);
    let transaction = test_direct_transaction(62, key, 42);
    let lease = DirectPrimaryLease::test_fixture(test_direct_key(4), 42);
    let job = test_direct_job(62, key, 42, Some(lease));

    assert_eq!(
        job.validate_against(&transaction),
        Err(KmsCommitPayloadError::DirectPrimaryResourceMismatch)
    );
}

#[test]
fn composited_and_cursor_jobs_reject_direct_leases() {
    let key = test_direct_key(3);
    let transaction = test_direct_transaction(63, key, 42);
    let lease = DirectPrimaryLease::test_fixture(key, 42);
    let mut job = test_direct_job(63, key, 42, Some(lease));
    job.kind = AtomicCommitKind::CompositedPrimary {
        transaction_id: job.transaction_id,
        frame_id: 63,
        framebuffer_id: 42,
    };

    assert_eq!(
        job.validate_against(&transaction),
        Err(KmsCommitPayloadError::DirectPrimaryResourceMismatch)
    );
}

#[derive(Debug)]
struct DirectLeaseRecordingExecutor {
    outcomes: Mutex<VecDeque<Result<(), oblivion_one::native::kms::AtomicKmsErrorKind>>>,
    observations: Mutex<Vec<(DirectScanoutCandidateKey, u32)>>,
}

impl KmsCommitExecutor for DirectLeaseRecordingExecutor {
    fn submit(&self, job: &KmsCommitJob) -> Result<KmsWorkerSubmission, KmsWorkerSubmitFailure> {
        if let Some(lease) = job.direct_primary_lease.as_ref() {
            self.observations
                .lock()
                .unwrap()
                .push((lease.key(), lease.framebuffer_id()));
        }
        match self.outcomes.lock().unwrap().pop_front().unwrap_or(Ok(())) {
            Ok(()) => Ok(KmsWorkerSubmission { out_fence: None }),
            Err(kind) => Err(KmsWorkerSubmitFailure::new(kind, "fake Atomic ioctl")),
        }
    }
}

#[test]
fn queued_direct_job_keeps_dmabuf_and_framebuffer_alive() {
    let executor = Arc::new(DirectLeaseRecordingExecutor {
        outcomes: Mutex::new(VecDeque::from([Err(
            oblivion_one::native::kms::AtomicKmsErrorKind::FlipRejected,
        )])),
        observations: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor).unwrap();
    let key = test_direct_key(3);
    let (lease, cleanup_count) = DirectPrimaryLease::test_fixture_with_probe(key, 42);
    reserve_for_test(&handle, test_job(64).kind)
        .enqueue(test_direct_job(64, key, 42, Some(lease)))
        .unwrap();

    let events = wait_for_fence_event(
        &handle,
        64,
        |event| matches!(event, KmsWorkerEvent::SubmitRejected { job, .. } if job.token.get() == 64),
    );
    assert_eq!(cleanup_count.load(std::sync::atomic::Ordering::Acquire), 0);
    drop(events);
    assert_eq!(cleanup_count.load(std::sync::atomic::Ordering::Acquire), 1);
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn direct_ebusy_retry_keeps_the_same_lease_identity() {
    let executor = Arc::new(DirectLeaseRecordingExecutor {
        outcomes: Mutex::new(VecDeque::from([
            Err(oblivion_one::native::kms::AtomicKmsErrorKind::Busy),
            Ok(()),
        ])),
        observations: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor.clone()).unwrap();
    let key = test_direct_key(3);
    let (lease, cleanup_count) = DirectPrimaryLease::test_fixture_with_probe(key, 42);
    reserve_for_test(&handle, test_job(65).kind)
        .enqueue(test_direct_job(65, key, 42, Some(lease)))
        .unwrap();

    let events = wait_for_fence_event(
        &handle,
        65,
        |event| matches!(event, KmsWorkerEvent::Submitted { token, .. } if token.get() == 65),
    );
    assert_eq!(
        *executor.observations.lock().unwrap(),
        vec![(key, 42), (key, 42)]
    );
    assert_eq!(cleanup_count.load(std::sync::atomic::Ordering::Acquire), 0);
    handle
        .ack_pageflip(test_job(65).token, test_job(65).transaction_id, 1)
        .unwrap();
    drop(events);
    assert_eq!(cleanup_count.load(std::sync::atomic::Ordering::Acquire), 1);
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn direct_test_rejection_returns_the_lease_once() {
    let executor = Arc::new(DirectLeaseRecordingExecutor {
        outcomes: Mutex::new(VecDeque::from([Err(
            oblivion_one::native::kms::AtomicKmsErrorKind::TestOnlyRejected,
        )])),
        observations: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor).unwrap();
    let key = test_direct_key(3);
    let (lease, cleanup_count) = DirectPrimaryLease::test_fixture_with_probe(key, 42);
    reserve_for_test(&handle, test_job(66).kind)
        .enqueue(test_direct_job(66, key, 42, Some(lease)))
        .unwrap();

    let events = wait_for_fence_event(
        &handle,
        66,
        |event| matches!(event, KmsWorkerEvent::TestRejected { job, .. } if job.token.get() == 66),
    );
    assert!(events.iter().any(|event| matches!(
        event,
        KmsWorkerEvent::TestRejected { job, .. } if job.direct_primary_lease.is_some()
    )));
    drop(events);
    assert_eq!(cleanup_count.load(std::sync::atomic::Ordering::Acquire), 1);
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn direct_submit_rejection_returns_the_lease_once() {
    let executor = Arc::new(DirectLeaseRecordingExecutor {
        outcomes: Mutex::new(VecDeque::from([Err(
            oblivion_one::native::kms::AtomicKmsErrorKind::FlipRejected,
        )])),
        observations: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor).unwrap();
    let key = test_direct_key(3);
    let (lease, cleanup_count) = DirectPrimaryLease::test_fixture_with_probe(key, 42);
    reserve_for_test(&handle, test_job(67).kind)
        .enqueue(test_direct_job(67, key, 42, Some(lease)))
        .unwrap();

    let events = wait_for_fence_event(
        &handle,
        67,
        |event| matches!(event, KmsWorkerEvent::SubmitRejected { job, .. } if job.token.get() == 67),
    );
    assert!(events.iter().any(|event| matches!(
        event,
        KmsWorkerEvent::SubmitRejected { job, .. } if job.direct_primary_lease.is_some()
    )));
    drop(events);
    assert_eq!(cleanup_count.load(std::sync::atomic::Ordering::Acquire), 1);
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn direct_shutdown_quiesce_returns_the_queued_lease() {
    let executor = Arc::new(DirectLeaseRecordingExecutor {
        outcomes: Mutex::new(VecDeque::new()),
        observations: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor).unwrap();
    let key = test_direct_key(3);
    let (lease, cleanup_count) = DirectPrimaryLease::test_fixture_with_probe(key, 42);
    let mut job = test_direct_job(68, key, 42, Some(lease));
    job.target.presentation_time = MonotonicTimestampNs::new(u64::MAX / 2);
    job.target.submit_not_before = MonotonicTimestampNs::new(u64::MAX / 2);
    reserve_for_test(&handle, job.kind).enqueue(job).unwrap();
    handle.request_quiesce();

    let events = wait_for_fence_event(&handle, 68, |event| {
        matches!(event, KmsWorkerEvent::Quiesced { .. })
    });
    assert!(events.iter().any(|event| matches!(
        event,
        KmsWorkerEvent::Quiesced { returned_jobs }
            if returned_jobs.iter().any(|job| job.direct_primary_lease.is_some())
    )));
    drop(events);
    assert_eq!(cleanup_count.load(std::sync::atomic::Ordering::Acquire), 1);
    handle.join().unwrap();
}

#[test]
fn successful_direct_submit_transfers_the_lease_to_submitted_event() {
    let executor = Arc::new(DirectLeaseRecordingExecutor {
        outcomes: Mutex::new(VecDeque::from([Ok(())])),
        observations: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor).unwrap();
    let key = test_direct_key(3);
    let (lease, cleanup_count) = DirectPrimaryLease::test_fixture_with_probe(key, 42);
    reserve_for_test(&handle, test_job(69).kind)
        .enqueue(test_direct_job(69, key, 42, Some(lease)))
        .unwrap();

    let events = wait_for_fence_event(
        &handle,
        69,
        |event| matches!(event, KmsWorkerEvent::Submitted { token, .. } if token.get() == 69),
    );
    assert!(events.iter().any(|event| matches!(
        event,
        KmsWorkerEvent::Submitted {
            direct_primary_lease: Some(lease),
            test_only_policy: KmsTestOnlyPolicy::Skip,
            ..
        } if lease.key() == key && lease.framebuffer_id() == 42
    )));
    assert_eq!(cleanup_count.load(std::sync::atomic::Ordering::Acquire), 0);
    handle
        .ack_pageflip(test_job(69).token, test_job(69).transaction_id, 1)
        .unwrap();
    drop(events);
    assert_eq!(cleanup_count.load(std::sync::atomic::Ordering::Acquire), 1);
    handle.request_quiesce();
    handle.join().unwrap();
}
