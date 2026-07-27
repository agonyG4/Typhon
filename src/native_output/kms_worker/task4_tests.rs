use super::tests::{
    fd_identity, fd_is_closed, fd_is_closed_or_reused, reserve_for_test, test_input_fence,
    test_job, wait_for_fence_event,
};
use super::thread::{KmsCommitExecutor, KmsWorkerSubmission, KmsWorkerSubmitFailure};
use super::{
    KmsCommitJob, KmsCommitWorkerHandle, KmsCursorUpdate, KmsPrimaryUpdate, KmsTestOnlyPolicy,
    KmsWorkerEvent,
};
use oblivion_one::native::kms::AtomicCursorVisualState;
use std::{
    collections::VecDeque,
    os::fd::AsRawFd,
    sync::{Arc, Mutex},
    time::Duration,
};

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
