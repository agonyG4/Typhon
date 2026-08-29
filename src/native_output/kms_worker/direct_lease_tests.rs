use super::tests::{reserve_for_test, test_job, wait_for_fence_event};
use super::thread::{KmsCommitExecutor, KmsWorkerSubmission, KmsWorkerSubmitFailure};
use super::{
    KmsCommitJob, KmsCommitPayloadError, KmsCommitTestPolicy, KmsCommitWorkerHandle,
    KmsCursorUpdate, KmsPrimaryUpdate, KmsTestOnlyPolicy, KmsValidationBase,
    KmsWorkerAdmissionError, KmsWorkerEvent,
};
use crate::native_output::scanout::DirectPrimaryLease;
use crate::native_output::{
    ContentEpochId, CursorPlaneAssignment, DirectScanoutCandidateKey, OutputContentKey,
    OutputReleasePlan, OutputSlotId, OutputTransaction, OutputTransactionId,
    runtime::{AtomicCommitKind, DmabufGpuReleaseSafety},
};
use oblivion_one::native::scheduler::NativeOutputPacingMode;
use oblivion_one::native::{
    kms::AtomicCursorVisualState,
    presentation_deadline::{MonotonicTimestampNs, PresentationTarget, PresentationTargetReason},
};
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
        cursor_content_key: None,
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

fn worker_direct_release_safety(handle: &KmsCommitWorkerHandle) -> DmabufGpuReleaseSafety {
    let (queued, executing, inflight) = handle.direct_content_keys();
    DmabufGpuReleaseSafety::from_ownership(
        false,
        queued.is_some(),
        executing.is_some(),
        inflight.is_some(),
    )
}

#[test]
fn submitted_composited_job_accepts_consumed_input_fence() {
    let token = 602;
    let transaction_id =
        OutputTransactionId::new(std::num::NonZeroU64::new(token).expect("transaction id"));
    let transaction = OutputTransaction::composited(
        transaction_id,
        1,
        MonotonicTimestampNs::new(10),
        test_target(),
        NativeOutputPacingMode::ReactiveDouble,
        token,
        12,
        13,
        OutputSlotId::new(0).expect("output slot"),
        42,
        None,
        oblivion_one::compositor::CompositorFrameBatchId::new(
            std::num::NonZeroU64::new(token).expect("frame batch id"),
        ),
    )
    .expect("composited transaction");
    let mut job = test_job(token);
    job.kind = AtomicCommitKind::CompositedPrimary {
        transaction_id,
        frame_id: token,
        framebuffer_id: 42,
    };
    job.target = test_target();
    job.primary = KmsPrimaryUpdate::Framebuffer {
        framebuffer: oblivion_one::native::kms::FramebufferId::new(42).expect("framebuffer"),
        in_fence: None,
        request_out_fence: false,
    };

    assert!(job.validate_against(&transaction).is_err());
    assert_eq!(job.validate_submitted_against(&transaction), Ok(()));
}

#[test]
fn composited_job_rejects_cursor_geometry_newer_than_transaction_plan() {
    let token = 603;
    let transaction_id =
        OutputTransactionId::new(std::num::NonZeroU64::new(token).expect("transaction id"));
    let planned_cursor = AtomicCursorVisualState {
        visible: true,
        x: 10,
        y: 20,
        hotspot_x: 1,
        hotspot_y: 2,
        width: 64,
        height: 64,
        framebuffer_id: None,
        image_generation: 4,
    };
    let transaction = OutputTransaction::composited(
        transaction_id,
        1,
        MonotonicTimestampNs::new(10),
        test_target(),
        NativeOutputPacingMode::ReactiveDouble,
        token,
        12,
        13,
        OutputSlotId::new(0).expect("output slot"),
        42,
        Some(CursorPlaneAssignment::Atomic {
            desired_epoch: 8,
            state: Some(planned_cursor.clone()),
        }),
        oblivion_one::compositor::CompositorFrameBatchId::new(
            std::num::NonZeroU64::new(token).expect("frame batch id"),
        ),
    )
    .expect("composited transaction");
    let mut job = test_job(token);
    job.kind = AtomicCommitKind::CompositedPrimary {
        transaction_id,
        frame_id: token,
        framebuffer_id: 42,
    };
    job.target = test_target();
    job.primary = KmsPrimaryUpdate::Framebuffer {
        framebuffer: oblivion_one::native::kms::FramebufferId::new(42).expect("framebuffer"),
        in_fence: None,
        request_out_fence: false,
    };
    let mut newer_cursor = planned_cursor;
    newer_cursor.x += 1;
    job.cursor = KmsCursorUpdate::Set(newer_cursor);

    assert_eq!(
        job.validate_submitted_against(&transaction),
        Err(KmsCommitPayloadError::CursorAssignmentMismatch)
    );
}

#[test]
fn direct_candidate_admission_rejects_a_duplicate_reservation_atomically() {
    let executor = Arc::new(DirectLeaseRecordingExecutor {
        outcomes: Mutex::new(VecDeque::new()),
        observations: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor).unwrap();
    let key = test_direct_key(3);
    let first = handle
        .try_reserve_direct_admission(key)
        .expect("first direct reservation should be admitted");

    assert!(matches!(
        handle.try_reserve_direct_admission(key),
        Err(KmsWorkerAdmissionError::DuplicateCandidate)
    ));

    drop(first);
    handle.request_quiesce();
    handle.join().unwrap();
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
fn direct_primary_job_has_exactly_one_direct_lease() {
    let key = test_direct_key(3);
    let transaction = test_direct_transaction(600, key, 42);
    let lease = DirectPrimaryLease::test_fixture(key, 42);
    let job = test_direct_job(600, key, 42, Some(lease));

    assert!(job.direct_primary_lease.is_some());
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
fn non_direct_job_cannot_carry_direct_lease() {
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

struct PanicDirectLeaseExecutor;

#[derive(Debug)]
struct BlockingDirectSubmitExecutor {
    started: std::sync::Barrier,
    release: std::sync::Barrier,
}

impl KmsCommitExecutor for BlockingDirectSubmitExecutor {
    fn submit(&self, _job: &KmsCommitJob) -> Result<KmsWorkerSubmission, KmsWorkerSubmitFailure> {
        self.started.wait();
        self.release.wait();
        Ok(KmsWorkerSubmission { out_fence: None })
    }
}

impl KmsCommitExecutor for PanicDirectLeaseExecutor {
    fn submit(&self, _job: &KmsCommitJob) -> Result<KmsWorkerSubmission, KmsWorkerSubmitFailure> {
        panic!("fake direct worker panic");
    }
}

fn assert_executing_direct_candidate_survives_real_submit(
    test_only: KmsTestOnlyPolicy,
    token: u64,
) {
    let executor = Arc::new(BlockingDirectSubmitExecutor {
        started: std::sync::Barrier::new(2),
        release: std::sync::Barrier::new(2),
    });
    let handle = KmsCommitWorkerHandle::start(executor.clone()).unwrap();
    let key = test_direct_key(3);
    let (lease, cleanup_count) = DirectPrimaryLease::test_fixture_with_probe(key, 42);
    let mut job = test_direct_job(token, key, 42, Some(lease));
    job.test_policy = KmsCommitTestPolicy::from_primary(test_only);
    reserve_for_test(&handle, job.kind).enqueue(job).unwrap();

    executor.started.wait();
    assert_eq!(handle.direct_content_keys().1, Some(key));
    assert!(!worker_direct_release_safety(&handle).permits_compositor_gpu_release());
    assert!(matches!(
        handle.try_reserve_direct_admission(key),
        Err(KmsWorkerAdmissionError::DuplicateCandidate)
    ));

    executor.release.wait();
    let events = wait_for_fence_event(
        &handle,
        token,
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.token.get() == token),
    );
    assert_eq!(handle.direct_content_keys().2, Some(key));
    assert!(!worker_direct_release_safety(&handle).permits_compositor_gpu_release());
    assert_eq!(cleanup_count.load(std::sync::atomic::Ordering::Acquire), 0);
    handle
        .ack_pageflip(test_job(token).token, test_job(token).transaction_id, 1)
        .unwrap();
    drop(events);
    assert_eq!(handle.direct_content_keys(), (None, None, None));
    assert_eq!(cleanup_count.load(std::sync::atomic::Ordering::Acquire), 1);
    handle.request_quiesce();
    handle.join().unwrap();
}

fn assert_duplicate_candidate_is_rejected_after_atomic_dequeue(
    test_only: KmsTestOnlyPolicy,
    token: u64,
) {
    let executor = Arc::new(BlockingDirectSubmitExecutor {
        started: std::sync::Barrier::new(2),
        release: std::sync::Barrier::new(2),
    });
    let handle = KmsCommitWorkerHandle::start(executor.clone()).unwrap();
    let pause = handle.pause_after_dequeue_for_test();
    let key = test_direct_key(3);
    let (lease, cleanup_count) = DirectPrimaryLease::test_fixture_with_probe(key, 42);
    let mut job = test_direct_job(token, key, 42, Some(lease));
    job.test_policy = KmsCommitTestPolicy::from_primary(test_only);
    reserve_for_test(&handle, job.kind).enqueue(job).unwrap();

    pause.wait_until_selected();
    assert_eq!(handle.direct_content_keys(), (None, Some(key), None));
    assert!(!worker_direct_release_safety(&handle).permits_compositor_gpu_release());
    assert!(matches!(
        handle.try_reserve_direct_admission(key),
        Err(KmsWorkerAdmissionError::DuplicateCandidate)
    ));
    assert_eq!(handle.queue_depth(), 0);
    assert_eq!(cleanup_count.load(std::sync::atomic::Ordering::Acquire), 0);

    pause.release();
    executor.started.wait();
    assert!(matches!(
        handle.try_reserve_direct_admission(key),
        Err(KmsWorkerAdmissionError::DuplicateCandidate)
    ));
    executor.release.wait();
    let events = wait_for_fence_event(
        &handle,
        token,
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.token.get() == token),
    );
    assert_eq!(handle.direct_content_keys().2, Some(key));
    assert!(!worker_direct_release_safety(&handle).permits_compositor_gpu_release());
    handle
        .ack_pageflip(test_job(token).token, test_job(token).transaction_id, 1)
        .unwrap();
    drop(events);
    assert_eq!(handle.direct_content_keys(), (None, None, None));
    assert_eq!(cleanup_count.load(std::sync::atomic::Ordering::Acquire), 1);
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn queued_to_executing_transition_is_atomic() {
    assert_duplicate_candidate_is_rejected_after_atomic_dequeue(KmsTestOnlyPolicy::Skip, 606);
}

#[test]
fn duplicate_candidate_cannot_reserve_during_dequeue() {
    assert_duplicate_candidate_is_rejected_after_atomic_dequeue(KmsTestOnlyPolicy::Required, 607);
}

#[test]
fn duplicate_candidate_cannot_reserve_before_execution_guard_returns() {
    assert_duplicate_candidate_is_rejected_after_atomic_dequeue(KmsTestOnlyPolicy::Skip, 608);
}

#[test]
fn executing_candidate_survives_required_test_only_and_blocked_real_submit() {
    assert_executing_direct_candidate_survives_real_submit(KmsTestOnlyPolicy::Required, 604);
}

#[test]
fn executing_candidate_survives_cached_skip_and_blocked_real_submit() {
    assert_executing_direct_candidate_survives_real_submit(KmsTestOnlyPolicy::Skip, 605);
}

#[test]
fn duplicate_candidate_rejected_during_required_test_submit() {
    assert_executing_direct_candidate_survives_real_submit(KmsTestOnlyPolicy::Required, 609);
}

#[test]
fn duplicate_candidate_rejected_during_cached_submit() {
    assert_executing_direct_candidate_survives_real_submit(KmsTestOnlyPolicy::Skip, 610);
}

#[test]
fn executing_candidate_survives_ebusy_retry() {
    direct_ebusy_retry_keeps_the_same_lease_identity();
}

#[test]
fn executing_candidate_transfers_to_inflight_after_submit() {
    successful_direct_submit_transfers_the_lease_to_submitted_event();
}

impl KmsCommitExecutor for DirectLeaseRecordingExecutor {
    fn test_only(&self, _job: &KmsCommitJob) -> Result<(), KmsWorkerSubmitFailure> {
        let mut outcomes = self.outcomes.lock().unwrap();
        if matches!(
            outcomes.front(),
            Some(Err(
                oblivion_one::native::kms::AtomicKmsErrorKind::TestOnlyRejected
            ))
        ) {
            return match outcomes.pop_front().expect("test-only rejection outcome") {
                Ok(()) => unreachable!("test-only outcome changed while locked"),
                Err(kind) => Err(KmsWorkerSubmitFailure::new(kind, "fake Atomic ioctl")),
            };
        }
        Ok(())
    }

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
fn queued_direct_worker_lease_blocks_gpu_release_safety_snapshot() {
    let executor = Arc::new(BlockingDirectSubmitExecutor {
        started: std::sync::Barrier::new(2),
        release: std::sync::Barrier::new(2),
    });
    let handle = KmsCommitWorkerHandle::start(executor.clone()).unwrap();
    let first_key = test_direct_key(3);
    let second_key = test_direct_key(4);
    let (first_lease, first_cleanup_count) =
        DirectPrimaryLease::test_fixture_with_probe(first_key, 42);
    let (second_lease, second_cleanup_count) =
        DirectPrimaryLease::test_fixture_with_probe(second_key, 43);
    let first = test_direct_job(80, first_key, 42, Some(first_lease));
    let first_identity = first.identity();
    reserve_for_test(&handle, first.kind)
        .enqueue(first)
        .unwrap();
    executor.started.wait();

    let mut second = test_direct_job(81, second_key, 43, Some(second_lease));
    second.validation_base = KmsValidationBase::Predecessor(first_identity);
    reserve_for_test(&handle, second.kind)
        .enqueue(second)
        .unwrap();
    assert_eq!(handle.direct_content_keys().0, Some(second_key));
    assert!(!worker_direct_release_safety(&handle).permits_compositor_gpu_release());
    assert_eq!(
        first_cleanup_count.load(std::sync::atomic::Ordering::Acquire),
        0
    );
    assert_eq!(
        second_cleanup_count.load(std::sync::atomic::Ordering::Acquire),
        0
    );

    executor.release.wait();
    let first_events = wait_for_fence_event(
        &handle,
        80,
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.token.get() == 80),
    );
    handle
        .ack_pageflip(test_job(80).token, test_job(80).transaction_id, 1)
        .unwrap();
    drop(first_events);
    executor.started.wait();
    assert_eq!(handle.direct_content_keys().1, Some(second_key));
    assert!(!worker_direct_release_safety(&handle).permits_compositor_gpu_release());

    executor.release.wait();
    let second_events = wait_for_fence_event(
        &handle,
        81,
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.token.get() == 81),
    );
    handle
        .ack_pageflip(test_job(81).token, test_job(81).transaction_id, 1)
        .unwrap();
    drop(second_events);
    assert_eq!(handle.direct_content_keys(), (None, None, None));
    assert_eq!(
        first_cleanup_count.load(std::sync::atomic::Ordering::Acquire),
        1
    );
    assert_eq!(
        second_cleanup_count.load(std::sync::atomic::Ordering::Acquire),
        1
    );
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
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.token.get() == 65),
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
    let mut job = test_direct_job(66, key, 42, Some(lease));
    job.test_policy.primary = KmsTestOnlyPolicy::Required;
    reserve_for_test(&handle, job.kind).enqueue(job).unwrap();

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
        KmsWorkerEvent::Quiesced { returned_jobs, .. }
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
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.token.get() == 69),
    );
    assert!(events.iter().any(|event| matches!(
        event,
        KmsWorkerEvent::Submitted { ownership } if ownership.job.direct_primary_lease.as_ref()
            .is_some_and(|lease| lease.key() == key && lease.framebuffer_id() == 42)
            && ownership.job.test_policy.effective() == KmsTestOnlyPolicy::Skip
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

#[test]
fn uncertain_worker_fatal_retains_the_complete_direct_job_once() {
    let handle = KmsCommitWorkerHandle::start(Arc::new(PanicDirectLeaseExecutor)).unwrap();
    let key = test_direct_key(3);
    let (lease, cleanup_count) = DirectPrimaryLease::test_fixture_with_probe(key, 42);
    reserve_for_test(&handle, test_job(70).kind)
        .enqueue(test_direct_job(70, key, 42, Some(lease)))
        .unwrap();

    for _ in 0..200 {
        if handle.fatal_reason().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(handle.fatal_reason().is_some());
    let fatal_jobs = handle.take_fatal_jobs();
    assert_eq!(fatal_jobs.len(), 1);
    assert!(fatal_jobs[0].uncertain_submit);
    assert!(fatal_jobs[0].job.direct_primary_lease.is_some());
    assert_eq!(cleanup_count.load(std::sync::atomic::Ordering::Acquire), 0);
    assert!(handle.take_fatal_jobs().is_empty());
    drop(fatal_jobs);
    assert_eq!(cleanup_count.load(std::sync::atomic::Ordering::Acquire), 1);
    handle.join().unwrap();
}
