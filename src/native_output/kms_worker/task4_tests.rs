use super::tests::{
    fd_identity, fd_is_closed, fd_is_closed_or_reused, reserve_for_test, test_input_fence,
    test_job, wait_for_fence_event,
};
use super::thread::{KmsCommitExecutor, KmsWorkerSubmission, KmsWorkerSubmitFailure};
use super::{
    KmsBundleOwners, KmsCommitJob, KmsCommitWorkerHandle, KmsCursorOwner, KmsCursorUpdate,
    KmsPrimaryOwner, KmsPrimaryUpdate, KmsSubmittedOwnership, KmsTestOnlyPolicy, KmsWorkerEvent,
    KmsWorkerFatalJob,
};
use oblivion_one::native::kms::AtomicCursorVisualState;
use std::{
    collections::VecDeque,
    os::fd::AsRawFd,
    sync::{Arc, Mutex},
    time::Duration,
};

fn two_owner_job(
    token: u64,
) -> (
    KmsCommitJob,
    crate::native_output::OutputTransactionId,
    crate::native_output::OutputTransactionId,
) {
    let mut job = test_job(token);
    let primary_id = job.transaction_id;
    let cursor_id = crate::native_output::OutputTransactionId::new(
        std::num::NonZeroU64::new(token.saturating_mul(10)).unwrap(),
    );
    let transaction = |id, epoch| {
        Arc::new(
            crate::native_output::OutputTransaction::cursor_only(
                id,
                1,
                oblivion_one::native::presentation_deadline::MonotonicTimestampNs::new(0),
                job.target,
                oblivion_one::native::scheduler::NativeOutputPacingMode::ReactiveDouble,
                epoch,
                None,
                crate::native_output::OutputReleasePlan::Pageflip,
            )
            .unwrap(),
        )
    };
    job.owners = KmsBundleOwners::new(
        Some(KmsPrimaryOwner {
            transaction: transaction(primary_id, token),
        }),
        Some(KmsCursorOwner {
            transaction: transaction(cursor_id, token.saturating_mul(10)),
            sidecar_id: None,
            revision: crate::native_output::presentation::plane::CursorRevision::initial(),
        }),
    )
    .unwrap();
    (job, primary_id, cursor_id)
}

fn assert_two_owners(
    job: &KmsCommitJob,
    primary_id: crate::native_output::OutputTransactionId,
    cursor_id: crate::native_output::OutputTransactionId,
) {
    assert_eq!(job.owners.primary_transaction_id(), Some(primary_id));
    assert_eq!(job.owners.cursor_transaction_id(), Some(cursor_id));
}

#[test]
fn rejection_event_retains_both_logical_bundle_owners() {
    let (job, primary_id, cursor_id) = two_owner_job(33);
    let event = KmsWorkerEvent::TestRejected {
        job,
        error: oblivion_one::native::kms::AtomicKmsError::new(
            oblivion_one::native::kms::AtomicKmsErrorKind::TestOnlyRejected,
            "test",
        ),
    };

    let KmsWorkerEvent::TestRejected { job, .. } = event else {
        unreachable!();
    };
    assert_two_owners(&job, primary_id, cursor_id);
}

#[test]
fn terminal_worker_transports_preserve_both_bundle_owners() {
    let error = || {
        oblivion_one::native::kms::AtomicKmsError::new(
            oblivion_one::native::kms::AtomicKmsErrorKind::FlipRejected,
            "test",
        )
    };
    let (job, primary_id, cursor_id) = two_owner_job(331);
    let submitted =
        KmsSubmittedOwnership {
            job,
            out_fence: None,
            submit_started_at:
                oblivion_one::native::presentation_deadline::MonotonicTimestampNs::new(1),
            submit_returned_at:
                oblivion_one::native::presentation_deadline::MonotonicTimestampNs::new(2),
            queue_residency_ns: 1,
            submit_wake_lateness_ns: 0,
            submission_budget_ns: 1,
        };
    assert_two_owners(&submitted.job, primary_id, cursor_id);

    let (job, _, _) = two_owner_job(331);
    let KmsWorkerEvent::SubmitRejected { job, .. } = (KmsWorkerEvent::SubmitRejected {
        job,
        error: error(),
    }) else {
        unreachable!();
    };
    assert_two_owners(&job, primary_id, cursor_id);

    let (job, _, _) = two_owner_job(331);
    let KmsWorkerEvent::BusyExhausted { job, .. } = (KmsWorkerEvent::BusyExhausted {
        job,
        error: error(),
    }) else {
        unreachable!();
    };
    assert_two_owners(&job, primary_id, cursor_id);

    let (job, _, _) = two_owner_job(331);
    let KmsWorkerEvent::Quiesced { returned_jobs } = (KmsWorkerEvent::Quiesced {
        returned_jobs: vec![job],
    }) else {
        unreachable!();
    };
    assert_two_owners(&returned_jobs[0], primary_id, cursor_id);

    let (job, _, _) = two_owner_job(331);
    let fatal = KmsWorkerFatalJob {
        job,
        uncertain_submit: true,
    };
    assert_two_owners(&fatal.job, primary_id, cursor_id);
}

#[test]
fn changed_cursor_property_requires_an_exact_cursor_owner() {
    let (mut job, _, _) = two_owner_job(332);
    let primary = job.owners.primary().cloned().unwrap();
    let transaction = Arc::clone(&primary.transaction);
    job.owners = KmsBundleOwners::new(Some(primary), None).unwrap();
    job.cursor = KmsCursorUpdate::Disable;

    assert_eq!(
        job.validate_against(&transaction),
        Err(super::KmsCommitPayloadError::MissingCursorOwner)
    );
}

fn required_direct_test_job(token: u64) -> KmsCommitJob {
    let mut job = test_job(token);
    job.test_only = KmsTestOnlyPolicy::Required;
    job.cursor = KmsCursorUpdate::Set(AtomicCursorVisualState {
        framebuffer_id: Some(91),
        visible: true,
        ..AtomicCursorVisualState::hidden(64, 64)
    });
    job.primary = KmsPrimaryUpdate::Framebuffer {
        framebuffer: oblivion_one::native::kms::FramebufferId::new(42).unwrap(),
        in_fence: None,
        request_out_fence: true,
    };
    job
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RecordedAtomicRequest {
    framebuffer_id: u32,
    cursor: KmsCursorUpdate,
    test_only: bool,
    request_out_fence: bool,
}

#[derive(Debug)]
struct RecordingExecutor {
    test_outcomes: Mutex<VecDeque<Result<(), oblivion_one::native::kms::AtomicKmsErrorKind>>>,
    submit_outcomes: Mutex<VecDeque<Result<(), oblivion_one::native::kms::AtomicKmsErrorKind>>>,
    requests: Mutex<Vec<RecordedAtomicRequest>>,
    real_input_fence_fds: Mutex<Vec<i32>>,
    real_input_fence_open: Mutex<Vec<bool>>,
}

impl RecordingExecutor {
    fn accepting() -> Self {
        Self {
            test_outcomes: Mutex::new(VecDeque::from([Ok(())])),
            submit_outcomes: Mutex::new(VecDeque::from([Ok(())])),
            requests: Mutex::new(Vec::new()),
            real_input_fence_fds: Mutex::new(Vec::new()),
            real_input_fence_open: Mutex::new(Vec::new()),
        }
    }

    fn record(&self, job: &KmsCommitJob, test_only: bool) {
        let (framebuffer_id, request_out_fence) = match &job.primary {
            KmsPrimaryUpdate::Framebuffer {
                framebuffer,
                request_out_fence,
                ..
            } => (framebuffer.get(), *request_out_fence),
            KmsPrimaryUpdate::Unchanged => (0, false),
        };
        self.requests.lock().unwrap().push(RecordedAtomicRequest {
            framebuffer_id,
            cursor: job.cursor.clone(),
            test_only,
            request_out_fence: !test_only && request_out_fence,
        });
    }

    fn next_outcome(
        outcomes: &Mutex<VecDeque<Result<(), oblivion_one::native::kms::AtomicKmsErrorKind>>>,
    ) -> Result<(), KmsWorkerSubmitFailure> {
        match outcomes.lock().unwrap().pop_front().unwrap_or(Ok(())) {
            Ok(()) => Ok(()),
            Err(kind) => Err(KmsWorkerSubmitFailure::new(kind, "fake Atomic ioctl")),
        }
    }
}

impl KmsCommitExecutor for RecordingExecutor {
    fn test_only(&self, job: &KmsCommitJob) -> Result<(), KmsWorkerSubmitFailure> {
        self.record(job, true);
        Self::next_outcome(&self.test_outcomes)
    }

    fn submit(&self, job: &KmsCommitJob) -> Result<KmsWorkerSubmission, KmsWorkerSubmitFailure> {
        self.record(job, false);
        let input_fence_fd = match &job.primary {
            KmsPrimaryUpdate::Framebuffer { in_fence, .. } => {
                in_fence.as_ref().map(AsRawFd::as_raw_fd)
            }
            KmsPrimaryUpdate::Unchanged => None,
        };
        self.real_input_fence_fds
            .lock()
            .unwrap()
            .push(input_fence_fd.unwrap_or(-1));
        self.real_input_fence_open
            .lock()
            .unwrap()
            .push(input_fence_fd.is_some_and(|fd| !fd_is_closed(fd)));
        Self::next_outcome(&self.submit_outcomes)?;
        Ok(KmsWorkerSubmission { out_fence: None })
    }
}

#[test]
fn direct_worker_tests_then_submits_the_same_primary_and_cursor_state() {
    let executor = Arc::new(RecordingExecutor::accepting());
    let handle = KmsCommitWorkerHandle::start(executor.clone()).unwrap();
    let job = required_direct_test_job(34);
    let token = job.token;
    let transaction_id = job.transaction_id;
    reserve_for_test(&handle, job.kind).enqueue(job).unwrap();

    let events = wait_for_fence_event(
        &handle,
        34,
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.token.get() == 34),
    );
    let requests = executor.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].test_only);
    assert!(!requests[1].test_only);
    assert_eq!(requests[0].framebuffer_id, requests[1].framebuffer_id);
    assert_eq!(requests[0].cursor, requests[1].cursor);
    assert!(!requests[0].request_out_fence);
    assert!(requests[1].request_out_fence);
    assert!(events.iter().any(|event| matches!(
        event,
        KmsWorkerEvent::Submitted { ownership }
            if ownership.job.test_only_duration_ns.is_some()
    )));

    handle.ack_pageflip(token, transaction_id, 1).unwrap();
    drop(events);
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn direct_test_rejection_prevents_real_submit() {
    let executor = Arc::new(RecordingExecutor {
        test_outcomes: Mutex::new(VecDeque::from([Err(
            oblivion_one::native::kms::AtomicKmsErrorKind::TestOnlyRejected,
        )])),
        submit_outcomes: Mutex::new(VecDeque::from([Ok(())])),
        requests: Mutex::new(Vec::new()),
        real_input_fence_fds: Mutex::new(Vec::new()),
        real_input_fence_open: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor.clone()).unwrap();
    let job = required_direct_test_job(35);
    reserve_for_test(&handle, job.kind).enqueue(job).unwrap();

    let events = wait_for_fence_event(
        &handle,
        35,
        |event| matches!(event, KmsWorkerEvent::TestRejected { job, .. } if job.token.get() == 35),
    );
    let requests = executor.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].test_only);
    assert!(events.iter().any(|event| {
        matches!(event, KmsWorkerEvent::TestRejected { job, .. } if job.token.get() == 35)
    }));
    assert!(events.iter().any(|event| matches!(
        event,
        KmsWorkerEvent::TestRejected { job, .. }
            if job.test_only_duration_ns.is_some()
    )));

    drop(events);
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn direct_test_only_does_not_consume_input_fence() {
    let executor = Arc::new(RecordingExecutor::accepting());
    let handle = KmsCommitWorkerHandle::start(executor.clone()).unwrap();
    let fence = test_input_fence();
    let raw_fd = fence.as_raw_fd();
    let original_identity = fd_identity(raw_fd);
    let mut job = required_direct_test_job(36);
    job.primary = KmsPrimaryUpdate::Framebuffer {
        framebuffer: oblivion_one::native::kms::FramebufferId::new(42).unwrap(),
        in_fence: Some(fence),
        request_out_fence: true,
    };
    reserve_for_test(&handle, job.kind).enqueue(job).unwrap();

    let events = wait_for_fence_event(
        &handle,
        36,
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.token.get() == 36),
    );
    assert_eq!(*executor.real_input_fence_fds.lock().unwrap(), vec![raw_fd]);
    assert_eq!(*executor.real_input_fence_open.lock().unwrap(), vec![true]);
    assert_eq!(executor.requests.lock().unwrap().len(), 2);

    handle
        .ack_pageflip(test_job(36).token, test_job(36).transaction_id, 1)
        .unwrap();
    drop(events);
    for _ in 0..100 {
        if fd_is_closed_or_reused(raw_fd, original_identity.as_deref()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(fd_is_closed_or_reused(raw_fd, original_identity.as_deref()));
    handle.request_quiesce();
    handle.join().unwrap();
}
