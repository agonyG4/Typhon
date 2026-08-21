use super::thread::{
    KmsCommitExecutor, KmsWorkerFatalReason, KmsWorkerSubmission, KmsWorkerSubmitFailure,
};
use super::*;
use crate::native_output::output::test_cursor_for_worker;
use crate::native_output::runtime::NativeCursorOutputArbitration;
use crate::native_output::{OutputTransactionId, runtime::AtomicCommitKind};
use oblivion_one::native::kms::AtomicCursorVisualState;
use oblivion_one::native::kms::KmsBackendKind;
use oblivion_one::native::presentation_deadline::{
    MonotonicTimestampNs, PresentationTarget, PresentationTargetReason,
};
use std::{
    collections::VecDeque,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    sync::{Arc, Barrier, Mutex},
    time::Duration,
};

#[test]
fn worker_policy_defaults_to_off() {
    assert_eq!(
        KmsCommitWorkerPolicy::from_env_value(None),
        KmsCommitWorkerPolicy::Off
    );
}

#[test]
fn worker_policy_accepts_all_rollout_values() {
    assert_eq!(
        KmsCommitWorkerPolicy::parse(Some("off")).unwrap(),
        KmsCommitWorkerPolicy::Off
    );
    assert_eq!(
        KmsCommitWorkerPolicy::parse(Some("auto")).unwrap(),
        KmsCommitWorkerPolicy::Auto
    );
    assert_eq!(
        KmsCommitWorkerPolicy::parse(Some("force")).unwrap(),
        KmsCommitWorkerPolicy::Force
    );
}

#[test]
fn auto_atomic_uses_worker_when_startup_succeeds() {
    assert_eq!(
        KmsCommitWorkerPolicy::Auto
            .effective(KmsBackendKind::Atomic, true)
            .unwrap(),
        KmsCommitWorkerTransport::Worker
    );
}

#[test]
fn auto_atomic_falls_back_to_sync_when_startup_fails() {
    assert_eq!(
        KmsCommitWorkerPolicy::Auto
            .effective(KmsBackendKind::Atomic, false)
            .unwrap(),
        KmsCommitWorkerTransport::Synchronous
    );
}

#[test]
fn force_legacy_is_unsupported() {
    assert_eq!(
        KmsCommitWorkerPolicy::Force.effective(KmsBackendKind::Legacy, true),
        Err(KmsCommitWorkerStartupError::UnsupportedBackend)
    );
}

pub(super) fn test_job(token: u64) -> KmsCommitJob {
    let transaction_id = OutputTransactionId::new(
        std::num::NonZeroU64::new(token).expect("test transaction ID is nonzero"),
    );
    KmsCommitJob {
        bundle_id:
            crate::native_output::presentation::plane::KmsCommitBundleId::from_pageflip_token(
                oblivion_one::native::kms::PageFlipToken::new(token).unwrap(),
            ),
        owners: KmsBundleOwners::legacy_unchecked(),
        transaction_id,
        token: oblivion_one::native::kms::PageFlipToken::new(token).unwrap(),
        output_generation: 1,
        crtc_id: 7,
        kind: AtomicCommitKind::DirectPrimary {
            transaction_id,
            direct_token: oblivion_one::native::kms::PageFlipToken::new(token).unwrap(),
            framebuffer_id: 42,
        },
        target: PresentationTarget {
            sequence: token,
            presentation_time: MonotonicTimestampNs::new(0),
            submit_not_before: MonotonicTimestampNs::new(0),
            render_start_deadline: MonotonicTimestampNs::new(0),
            refresh_interval: Duration::from_millis(16),
            reason: PresentationTargetReason::ReactiveDouble,
            clock_generation: 1,
            estimated: true,
            predicted_unreachable: false,
        },
        submit_window: crate::native_output::presentation::kms_timing::KmsSubmitWindow::try_new(
            0, 0, 0, 0,
        )
        .unwrap(),
        validation_base: KmsValidationBase::Presented {
            snapshot: crate::native_output::presentation::plane::PresentedPlaneSnapshot::legacy(
                None,
            ),
            output_generation: 1,
            crtc_id: 7,
        },
        queued_at: MonotonicTimestampNs::new(0),
        primary: KmsPrimaryUpdate::Framebuffer {
            framebuffer: oblivion_one::native::kms::FramebufferId::new(42).unwrap(),
            in_fence: None,
            request_out_fence: false,
        },
        cursor: KmsCursorUpdate::Unchanged,
        cursor_delivery: crate::native_output::presentation::plane::PresentedCursorDelivery::Hidden,
        primary_cursor_presentation: KmsPrimaryCursorPresentation::Preserve,
        cursor_pin: None,
        direct_primary_lease: None,
        test_only_duration_ns: None,
        pacing_frame_id: None,
        test_policy: KmsCommitTestPolicy::from_primary(KmsTestOnlyPolicy::Skip),
        ready_submit: false,
    }
}

fn test_job_with_input_fence(token: u64, fence: OwnedFd) -> KmsCommitJob {
    let mut job = test_job(token);
    job.primary = KmsPrimaryUpdate::Framebuffer {
        framebuffer: oblivion_one::native::kms::FramebufferId::new(42).unwrap(),
        in_fence: Some(fence),
        request_out_fence: false,
    };
    job
}

fn test_cursor_job(token: u64) -> KmsCommitJob {
    let mut job = test_job(token);
    job.kind = AtomicCommitKind::PlaneDelta {
        transaction_id: job.transaction_id,
        cursor_epoch: token,
        framebuffer_id: Some(42),
    };
    job.primary = KmsPrimaryUpdate::Unchanged;
    job.cursor = KmsCursorUpdate::Disable;
    job.test_policy.cursor = KmsTestOnlyPolicy::Required;
    job
}

fn test_primary_job_with_cursor(
    token: u64,
    framebuffer_id: u32,
    cursor_pin: crate::native_output::output::CursorFramebufferPin,
) -> KmsCommitJob {
    let mut job = test_job(token);
    job.cursor = KmsCursorUpdate::Set(AtomicCursorVisualState {
        framebuffer_id: Some(framebuffer_id),
        visible: true,
        ..AtomicCursorVisualState::hidden(64, 64)
    });
    job.cursor_pin = Some(cursor_pin);
    job
}

fn test_composited_primary_job_with_cursor(
    token: u64,
    framebuffer_id: u32,
    cursor_pin: crate::native_output::output::CursorFramebufferPin,
) -> KmsCommitJob {
    let mut job = test_primary_job_with_cursor(token, framebuffer_id, cursor_pin);
    job.kind = AtomicCommitKind::CompositedPrimary {
        transaction_id: job.transaction_id,
        frame_id: token,
        framebuffer_id,
    };
    job
}

pub(super) fn test_input_fence() -> OwnedFd {
    let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
    assert!(fd >= 0, "test eventfd should be created");
    // SAFETY: eventfd returned a new owned descriptor for this test.
    unsafe { OwnedFd::from_raw_fd(fd) }
}

pub(super) fn fd_is_closed(raw_fd: i32) -> bool {
    unsafe { libc::fcntl(raw_fd, libc::F_GETFD) == -1 }
}

pub(super) fn fd_identity(raw_fd: i32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/self/fdinfo/{raw_fd}"))
        .ok()
        .and_then(|info| {
            info.lines().find_map(|line| {
                line.strip_prefix("eventfd-id:")
                    .map(str::trim)
                    .map(str::to_owned)
            })
        })
        .or_else(|| {
            std::fs::read_link(format!("/proc/self/fd/{raw_fd}"))
                .ok()
                .map(|target| target.to_string_lossy().into_owned())
        })
}

pub(super) fn fd_is_closed_or_reused(raw_fd: i32, original_identity: Option<&str>) -> bool {
    original_identity.is_some_and(|identity| fd_identity(raw_fd).as_deref() != Some(identity))
        || (original_identity.is_none() && fd_is_closed(raw_fd))
}

#[derive(Debug)]
struct FenceRecordingExecutor {
    outcomes: Mutex<VecDeque<Result<(), oblivion_one::native::kms::AtomicKmsErrorKind>>>,
    attempts: Mutex<Vec<i32>>,
}

impl KmsCommitExecutor for FenceRecordingExecutor {
    fn submit(&self, job: &KmsCommitJob) -> Result<KmsWorkerSubmission, KmsWorkerSubmitFailure> {
        let raw_fd = match &job.primary {
            KmsPrimaryUpdate::Framebuffer { in_fence, .. } => {
                in_fence.as_ref().map(AsRawFd::as_raw_fd)
            }
            KmsPrimaryUpdate::Unchanged => None,
        };
        self.attempts.lock().unwrap().push(raw_fd.unwrap_or(-1));
        let result = self.outcomes.lock().unwrap().pop_front().unwrap_or(Ok(()));
        match result {
            Ok(()) => Ok(KmsWorkerSubmission { out_fence: None }),
            Err(kind) => Err(KmsWorkerSubmitFailure::new(kind, "fake Atomic ioctl")),
        }
    }
}

pub(super) fn wait_for_fence_event(
    handle: &KmsCommitWorkerHandle,
    token: u64,
    predicate: impl Fn(&KmsWorkerEvent) -> bool,
) -> Vec<KmsWorkerEvent> {
    let mut events = Vec::new();
    for _ in 0..200 {
        std::thread::sleep(Duration::from_millis(1));
        events.extend(collect_events(handle));
        if events.iter().any(&predicate) {
            assert!(events.iter().any(|event| match event {
                KmsWorkerEvent::Submitted { ownership } => ownership.job.token.get() == token,
                KmsWorkerEvent::BusyDeferred {
                    token: event_token, ..
                }
                | KmsWorkerEvent::PageflipTimeout {
                    token: event_token, ..
                } => event_token.get() == token,
                KmsWorkerEvent::TestRejected { job, .. }
                | KmsWorkerEvent::SubmitRejected { job, .. }
                | KmsWorkerEvent::BusyExhausted { job, .. } => job.token.get() == token,
                KmsWorkerEvent::Quiesced { .. }
                | KmsWorkerEvent::CursorSidecarReturned { .. }
                | KmsWorkerEvent::ValidationBaseInvalidated { .. }
                | KmsWorkerEvent::Fatal { .. } => true,
            }));
            return events;
        }
    }
    panic!("worker did not produce the expected event for token {token}");
}

#[test]
fn busy_retry_preserves_input_fence_for_every_attempt() {
    let executor = Arc::new(FenceRecordingExecutor {
        outcomes: Mutex::new(VecDeque::from([
            Err(oblivion_one::native::kms::AtomicKmsErrorKind::Busy),
            Ok(()),
        ])),
        attempts: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor.clone()).unwrap();
    let fence = test_input_fence();
    let raw_fd = fence.as_raw_fd();
    reserve_for_test(&handle, test_job(30).kind)
        .enqueue(test_job_with_input_fence(30, fence))
        .unwrap();

    let events = wait_for_fence_event(
        &handle,
        30,
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.token.get() == 30),
    );
    assert_eq!(*executor.attempts.lock().unwrap(), vec![raw_fd, raw_fd]);
    handle
        .ack_pageflip(test_job(30).token, test_job(30).transaction_id, 1)
        .unwrap();
    drop(events);
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn successful_retry_releases_input_fence_after_submit() {
    let executor = Arc::new(FenceRecordingExecutor {
        outcomes: Mutex::new(VecDeque::from([
            Err(oblivion_one::native::kms::AtomicKmsErrorKind::Busy),
            Ok(()),
        ])),
        attempts: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor.clone()).unwrap();
    let fence = test_input_fence();
    let raw_fd = fence.as_raw_fd();
    reserve_for_test(&handle, test_job(31).kind)
        .enqueue(test_job_with_input_fence(31, fence))
        .unwrap();
    let events = wait_for_fence_event(
        &handle,
        31,
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.token.get() == 31),
    );
    assert_eq!(*executor.attempts.lock().unwrap(), vec![raw_fd, raw_fd]);
    assert!(events.iter().any(|event| matches!(
        event,
        KmsWorkerEvent::Submitted { ownership }
            if matches!(
                &ownership.job.primary,
                KmsPrimaryUpdate::Framebuffer { in_fence: None, .. }
            )
    )));
    handle
        .ack_pageflip(test_job(31).token, test_job(31).transaction_id, 1)
        .unwrap();
    drop(events);
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn busy_exhaustion_releases_input_fence_once() {
    let executor = Arc::new(FenceRecordingExecutor {
        outcomes: Mutex::new(VecDeque::from([
            Err(oblivion_one::native::kms::AtomicKmsErrorKind::Busy),
            Err(oblivion_one::native::kms::AtomicKmsErrorKind::Busy),
            Err(oblivion_one::native::kms::AtomicKmsErrorKind::Busy),
        ])),
        attempts: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor.clone()).unwrap();
    let fence = test_input_fence();
    let raw_fd = fence.as_raw_fd();
    reserve_for_test(&handle, test_job(32).kind)
        .enqueue(test_job_with_input_fence(32, fence))
        .unwrap();
    let events = wait_for_fence_event(
        &handle,
        32,
        |event| matches!(event, KmsWorkerEvent::BusyExhausted { job, .. } if job.token.get() == 32),
    );
    assert_eq!(
        *executor.attempts.lock().unwrap(),
        vec![raw_fd, raw_fd, raw_fd]
    );
    drop(events);
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn submit_rejection_releases_input_fence_once() {
    let executor = Arc::new(FenceRecordingExecutor {
        outcomes: Mutex::new(VecDeque::from([Err(
            oblivion_one::native::kms::AtomicKmsErrorKind::FlipRejected,
        )])),
        attempts: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor.clone()).unwrap();
    let fence = test_input_fence();
    let raw_fd = fence.as_raw_fd();
    reserve_for_test(&handle, test_job(33).kind)
        .enqueue(test_job_with_input_fence(33, fence))
        .unwrap();
    let events = wait_for_fence_event(
        &handle,
        33,
        |event| matches!(event, KmsWorkerEvent::SubmitRejected { job, .. } if job.token.get() == 33),
    );
    assert_eq!(*executor.attempts.lock().unwrap(), vec![raw_fd]);
    drop(events);
    handle.request_quiesce();
    handle.join().unwrap();
}

#[derive(Debug)]
struct BarrierExecutor {
    started: Barrier,
    release: Barrier,
    submitted: Mutex<Vec<u64>>,
}

impl KmsCommitExecutor for BarrierExecutor {
    fn submit(&self, job: &KmsCommitJob) -> Result<KmsWorkerSubmission, KmsWorkerSubmitFailure> {
        self.started.wait();
        self.release.wait();
        self.submitted.lock().unwrap().push(job.token.get());
        Ok(KmsWorkerSubmission { out_fence: None })
    }
}

#[derive(Debug)]
struct ScriptedExecutor {
    outcomes: Mutex<VecDeque<Result<(), oblivion_one::native::kms::AtomicKmsErrorKind>>>,
    submitted: Mutex<Vec<u64>>,
}

impl KmsCommitExecutor for ScriptedExecutor {
    fn submit(&self, job: &KmsCommitJob) -> Result<KmsWorkerSubmission, KmsWorkerSubmitFailure> {
        self.submitted.lock().unwrap().push(job.token.get());
        let result = self.outcomes.lock().unwrap().pop_front().unwrap_or(Ok(()));
        match result {
            Ok(()) => Ok(KmsWorkerSubmission { out_fence: None }),
            Err(kind) => Err(KmsWorkerSubmitFailure::new(kind, "fake Atomic ioctl")),
        }
    }
}

fn collect_events(handle: &KmsCommitWorkerHandle) -> Vec<KmsWorkerEvent> {
    handle.drain_eventfd().unwrap();
    handle.drain_events()
}

pub(super) fn reserve_for_test(
    handle: &KmsCommitWorkerHandle,
    kind: AtomicCommitKind,
) -> KmsCommitAdmissionPermit {
    loop {
        match handle.try_reserve_admission(kind) {
            Ok(permit) => return permit,
            Err(KmsWorkerAdmissionError::AdmissionContention) => std::thread::yield_now(),
            Err(error) => panic!("test admission failed unexpectedly: {error:?}"),
        }
    }
}

#[test]
fn runtime_queue_depth_metrics_match_real_runtime_state() {
    let executor = Arc::new(ScriptedExecutor {
        outcomes: Mutex::new(VecDeque::new()),
        submitted: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor).unwrap();

    handle.record_runtime_queue_state(1, true);
    let metrics = handle.metrics_snapshot();
    assert_eq!(metrics.runtime_queue_depth, 1);
    assert_eq!(metrics.runtime_queue_depth_max, 1);
    assert_eq!(metrics.runtime_kernel_inflight, 1);

    handle.record_runtime_queue_state(0, false);
    let metrics = handle.metrics_snapshot();
    assert_eq!(metrics.runtime_queue_depth, 0);
    assert_eq!(metrics.runtime_queue_depth_max, 1);
    assert_eq!(metrics.runtime_kernel_inflight, 0);

    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn main_thread_admission_returns_immediately_when_full() {
    let executor = Arc::new(BarrierExecutor {
        started: Barrier::new(2),
        release: Barrier::new(2),
        submitted: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor.clone()).unwrap();
    reserve_for_test(
        &handle,
        AtomicCommitKind::DirectPrimary {
            transaction_id: test_job(1).transaction_id,
            direct_token: test_job(1).token,
            framebuffer_id: 42,
        },
    )
    .enqueue(test_job(1))
    .unwrap();
    executor.started.wait();

    reserve_for_test(
        &handle,
        AtomicCommitKind::DirectPrimary {
            transaction_id: test_job(2).transaction_id,
            direct_token: test_job(2).token,
            framebuffer_id: 42,
        },
    )
    .enqueue(test_job(2))
    .unwrap();
    assert!(matches!(
        handle.try_reserve_admission(test_job(3).kind),
        Err(KmsWorkerAdmissionError::QueueFull)
    ));

    executor.release.wait();
    for _ in 0..100 {
        if handle.inflight() {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    handle
        .ack_pageflip(test_job(1).token, test_job(1).transaction_id, 1)
        .unwrap();
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn idle_worker_has_only_one_reserved_ready_slot() {
    let executor = Arc::new(ScriptedExecutor {
        outcomes: Mutex::new(VecDeque::new()),
        submitted: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor).unwrap();
    let permit = reserve_for_test(&handle, test_job(20).kind);
    assert!(matches!(
        handle.try_reserve_admission(test_job(21).kind),
        Err(KmsWorkerAdmissionError::QueueFull)
    ));
    drop(permit);
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn fifo_order_is_preserved_and_second_submit_waits_for_ack() {
    let executor = Arc::new(ScriptedExecutor {
        outcomes: Mutex::new(VecDeque::from([Ok(()), Ok(())])),
        submitted: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor.clone()).unwrap();
    let first = test_job(1);
    let first_identity = first.identity();
    reserve_for_test(&handle, test_job(1).kind)
        .enqueue(first)
        .unwrap();
    for _ in 0..100 {
        if handle.submission_active() {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let mut second = test_job(2);
    second.validation_base = KmsValidationBase::Predecessor(first_identity);
    reserve_for_test(&handle, second.kind)
        .enqueue(second)
        .unwrap();

    std::thread::sleep(Duration::from_millis(10));
    assert_eq!(*executor.submitted.lock().unwrap(), vec![1]);
    handle
        .ack_pageflip(test_job(1).token, test_job(1).transaction_id, 1)
        .unwrap();
    for _ in 0..100 {
        if executor.submitted.lock().unwrap().len() == 2 {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(*executor.submitted.lock().unwrap(), vec![1, 2]);
    handle
        .ack_pageflip(test_job(2).token, test_job(2).transaction_id, 1)
        .unwrap();
    handle.request_quiesce();
    let _ = collect_events(&handle);
    handle.join().unwrap();
}

#[test]
fn quiesce_rejects_new_admission() {
    let executor = Arc::new(ScriptedExecutor {
        outcomes: Mutex::new(VecDeque::new()),
        submitted: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor.clone()).unwrap();
    handle.request_quiesce();
    assert!(matches!(
        handle.try_reserve_admission(test_job(1).kind),
        Err(KmsWorkerAdmissionError::Quiescing)
    ));
    handle.join().unwrap();
}

#[test]
fn busy_retry_budget_is_bounded_and_returns_one_terminal_event() {
    let executor = Arc::new(ScriptedExecutor {
        outcomes: Mutex::new(VecDeque::from([
            Err(oblivion_one::native::kms::AtomicKmsErrorKind::Busy),
            Err(oblivion_one::native::kms::AtomicKmsErrorKind::Busy),
            Err(oblivion_one::native::kms::AtomicKmsErrorKind::Busy),
        ])),
        submitted: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor.clone()).unwrap();
    reserve_for_test(&handle, test_job(9).kind)
        .enqueue(test_job(9))
        .unwrap();

    let mut events = Vec::new();
    for _ in 0..100 {
        std::thread::sleep(Duration::from_millis(1));
        events.extend(collect_events(&handle));
        if events
            .iter()
            .any(|event| matches!(event, KmsWorkerEvent::BusyExhausted { .. }))
        {
            break;
        }
    }
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, KmsWorkerEvent::BusyDeferred { .. }))
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, KmsWorkerEvent::BusyExhausted { .. }))
            .count(),
        1
    );
    assert_eq!(*executor.submitted.lock().unwrap(), vec![9, 9, 9]);
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn one_eventfd_wakeup_drains_all_available_worker_results() {
    let executor = Arc::new(ScriptedExecutor {
        outcomes: Mutex::new(VecDeque::from([
            Err(oblivion_one::native::kms::AtomicKmsErrorKind::FlipRejected),
            Err(oblivion_one::native::kms::AtomicKmsErrorKind::FlipRejected),
        ])),
        submitted: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor).unwrap();
    reserve_for_test(&handle, test_job(10).kind)
        .enqueue(test_job(10))
        .unwrap();
    let mut first_events = Vec::new();
    for _ in 0..100 {
        first_events.extend(collect_events(&handle));
        if first_events.iter().any(|event| {
                matches!(event, KmsWorkerEvent::SubmitRejected { job, .. } if job.token.get() == 10)
            }) {
                break;
            }
        std::thread::sleep(Duration::from_millis(1));
    }
    reserve_for_test(&handle, test_job(11).kind)
        .enqueue(test_job(11))
        .unwrap();
    let mut events = first_events;
    for _ in 0..100 {
        events.extend(collect_events(&handle));
        if events.iter().any(|event| {
                matches!(event, KmsWorkerEvent::SubmitRejected { job, .. } if job.token.get() == 11)
            }) {
                break;
            }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(events.iter().any(|event| {
        matches!(event, KmsWorkerEvent::SubmitRejected { job, .. } if job.token.get() == 11)
    }));
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn worker_emits_one_pageflip_timeout_for_inflight_commit() {
    let executor = Arc::new(ScriptedExecutor {
        outcomes: Mutex::new(VecDeque::from([Ok(())])),
        submitted: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor).unwrap();
    reserve_for_test(&handle, test_job(13).kind)
        .enqueue(test_job(13))
        .unwrap();
    let mut events = Vec::new();
    for _ in 0..1_200 {
        events.extend(collect_events(&handle));
        if events
            .iter()
            .any(|event| matches!(event, KmsWorkerEvent::PageflipTimeout { .. }))
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, KmsWorkerEvent::PageflipTimeout { .. }))
            .count(),
        1
    );
    handle.request_quiesce();
    handle.join().unwrap();
}

struct PanicExecutor;

impl KmsCommitExecutor for PanicExecutor {
    fn submit(&self, _job: &KmsCommitJob) -> Result<KmsWorkerSubmission, KmsWorkerSubmitFailure> {
        panic!("fake worker panic");
    }
}

#[test]
fn worker_panic_becomes_fatal_event() {
    let handle = KmsCommitWorkerHandle::start(Arc::new(PanicExecutor)).unwrap();
    reserve_for_test(&handle, test_job(12).kind)
        .enqueue(test_job(12))
        .unwrap();
    let mut events = Vec::new();
    for _ in 0..100 {
        std::thread::sleep(Duration::from_millis(1));
        events.extend(collect_events(&handle));
        if events
            .iter()
            .any(|event| matches!(event, KmsWorkerEvent::Fatal { .. }))
        {
            break;
        }
    }
    assert!(events.iter().any(|event| {
        matches!(
            event,
            KmsWorkerEvent::Fatal {
                reason: KmsWorkerFatalReason::Panic,
                uncertain_submit: true,
            }
        )
    }));
    let fatal_jobs = handle.take_fatal_jobs();
    assert_eq!(fatal_jobs.len(), 1);
    assert!(fatal_jobs[0].uncertain_submit);
    assert!(handle.take_fatal_jobs().is_empty());
    drop(fatal_jobs);
    handle.join().unwrap();
}

#[test]
fn plane_delta_pageflip_ack_releases_worker_inflight() {
    let executor = Arc::new(ScriptedExecutor {
        outcomes: Mutex::new(VecDeque::from([Ok(())])),
        submitted: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor).unwrap();
    let job = test_cursor_job(14);
    let token = job.token;
    let transaction_id = job.transaction_id;
    reserve_for_test(&handle, job.kind).enqueue(job).unwrap();
    let events = wait_for_fence_event(
        &handle,
        14,
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.token.get() == 14),
    );
    handle.ack_pageflip(token, transaction_id, 1).unwrap();
    assert!(!handle.inflight());
    drop(events);
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn duplicate_worker_pageflip_ack_is_rejected() {
    let executor = Arc::new(ScriptedExecutor {
        outcomes: Mutex::new(VecDeque::from([Ok(())])),
        submitted: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor).unwrap();
    let job = test_job(15);
    reserve_for_test(&handle, job.kind).enqueue(job).unwrap();
    let events = wait_for_fence_event(
        &handle,
        15,
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.token.get() == 15),
    );
    handle
        .ack_pageflip(test_job(15).token, test_job(15).transaction_id, 1)
        .unwrap();
    assert_eq!(
        handle.ack_pageflip(test_job(15).token, test_job(15).transaction_id, 1),
        Err(super::thread::KmsWorkerAckError::NoInFlightCommit)
    );
    drop(events);
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn wrong_worker_ack_preserves_inflight() {
    let executor = Arc::new(ScriptedExecutor {
        outcomes: Mutex::new(VecDeque::from([Ok(()), Ok(())])),
        submitted: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor).unwrap();
    let job = test_job(16);
    reserve_for_test(&handle, job.kind).enqueue(job).unwrap();
    let events = wait_for_fence_event(
        &handle,
        16,
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.token.get() == 16),
    );
    assert_eq!(
        handle.ack_pageflip(test_job(17).token, test_job(16).transaction_id, 1),
        Err(super::thread::KmsWorkerAckError::TokenMismatch)
    );
    assert!(handle.inflight());
    handle
        .ack_pageflip(test_job(16).token, test_job(16).transaction_id, 1)
        .unwrap();
    drop(events);
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn eventfd_notification_failure_marks_worker_fatal() {
    let executor = Arc::new(ScriptedExecutor {
        outcomes: Mutex::new(VecDeque::from([Err(
            oblivion_one::native::kms::AtomicKmsErrorKind::FlipRejected,
        )])),
        submitted: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor).unwrap();
    let value = u64::MAX - 1;
    let written = unsafe {
        libc::write(
            handle.event_fd(),
            (&value as *const u64).cast(),
            std::mem::size_of::<u64>(),
        )
    };
    assert_eq!(written, std::mem::size_of::<u64>() as isize);
    reserve_for_test(&handle, test_job(17).kind)
        .enqueue(test_job(17))
        .unwrap();

    for _ in 0..200 {
        if handle.fatal_reason().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        handle.fatal_reason(),
        Some(KmsWorkerFatalReason::EventNotification)
    );
    assert!(matches!(
        handle.try_reserve_admission(test_job(18).kind),
        Err(KmsWorkerAdmissionError::Fatal)
    ));
    handle.drain_events();
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn notification_failure_does_not_drop_owned_queued_job() {
    let executor = Arc::new(FenceRecordingExecutor {
        outcomes: Mutex::new(VecDeque::from([Err(
            oblivion_one::native::kms::AtomicKmsErrorKind::Busy,
        )])),
        attempts: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor).unwrap();
    let value = u64::MAX - 1;
    let written = unsafe {
        libc::write(
            handle.event_fd(),
            (&value as *const u64).cast(),
            std::mem::size_of::<u64>(),
        )
    };
    assert_eq!(written, std::mem::size_of::<u64>() as isize);
    let fence = test_input_fence();
    let raw_fd = fence.as_raw_fd();
    let original_identity = fd_identity(raw_fd);
    let job = test_job_with_input_fence(19, fence);
    reserve_for_test(&handle, job.kind).enqueue(job).unwrap();
    for _ in 0..200 {
        if handle.fatal_reason().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        handle.fatal_reason(),
        Some(KmsWorkerFatalReason::EventNotification)
    );
    let fatal_jobs = handle.take_fatal_jobs();
    assert_eq!(fatal_jobs.len(), 1);
    assert_eq!(fd_identity(raw_fd).as_deref(), original_identity.as_deref());
    drop(fatal_jobs);
    assert!(fd_is_closed_or_reused(raw_fd, original_identity.as_deref()));
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn shutdown_ack_never_releases_queued_next_job() {
    let executor = Arc::new(ScriptedExecutor {
        outcomes: Mutex::new(VecDeque::from([Ok(()), Ok(())])),
        submitted: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor.clone()).unwrap();
    reserve_for_test(&handle, test_job(40).kind)
        .enqueue(test_job(40))
        .unwrap();
    wait_for_fence_event(
        &handle,
        40,
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.token.get() == 40),
    );
    reserve_for_test(&handle, test_job(41).kind)
        .enqueue(test_job(41))
        .unwrap();

    let snapshot = handle.begin_shutdown_quiesce().unwrap();
    assert_eq!(
        snapshot.queued_job.as_ref().map(|job| job.token.get()),
        Some(41)
    );
    assert_eq!(snapshot.inflight.map(|commit| commit.token.get()), Some(40));

    handle
        .ack_pageflip(test_job(40).token, test_job(40).transaction_id, 1)
        .unwrap();
    std::thread::sleep(Duration::from_millis(10));
    assert_eq!(*executor.submitted.lock().unwrap(), vec![40]);
    assert!(!handle.inflight());
    assert_eq!(handle.queue_depth(), 0);
    handle.join().unwrap();
}

#[test]
fn shutdown_admission_waits_for_ioctl_before_returning_inflight_identity() {
    let executor = Arc::new(BarrierExecutor {
        started: Barrier::new(2),
        release: Barrier::new(2),
        submitted: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor.clone()).unwrap();
    reserve_for_test(&handle, test_job(43).kind)
        .enqueue(test_job(43))
        .unwrap();
    executor.started.wait();

    std::thread::scope(|scope| {
        let shutdown = scope.spawn(|| handle.begin_shutdown_quiesce().unwrap());
        executor.release.wait();
        let snapshot = shutdown.join().unwrap();
        assert!(snapshot.queued_job.is_none());
        assert_eq!(snapshot.inflight.map(|commit| commit.token.get()), Some(43));
    });

    handle
        .ack_pageflip(test_job(43).token, test_job(43).transaction_id, 1)
        .unwrap();
    handle.join().unwrap();
}

#[test]
fn shutdown_returned_job_is_detached_once() {
    let executor = Arc::new(ScriptedExecutor {
        outcomes: Mutex::new(VecDeque::new()),
        submitted: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor).unwrap();
    reserve_for_test(&handle, test_job(42).kind)
        .enqueue(test_job(42))
        .unwrap();

    let snapshot = handle.begin_shutdown_quiesce().unwrap();
    assert_eq!(
        snapshot.queued_job.as_ref().map(|job| job.token.get()),
        Some(42)
    );
    let second = handle.begin_shutdown_quiesce().unwrap();
    assert!(second.queued_job.is_none());
    handle.join().unwrap();
}

#[test]
fn shutdown_quiesce_with_normal_ack_terminates_worker() {
    let executor = Arc::new(ScriptedExecutor {
        outcomes: Mutex::new(VecDeque::from([Ok(())])),
        submitted: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor).unwrap();
    reserve_for_test(&handle, test_job(46).kind)
        .enqueue(test_job(46))
        .unwrap();
    wait_for_fence_event(
        &handle,
        46,
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.token.get() == 46),
    );

    let snapshot = handle.begin_shutdown_quiesce().unwrap();
    assert_eq!(
        snapshot.inflight.map(|inflight| inflight.token.get()),
        Some(46)
    );
    handle
        .ack_pageflip(test_job(46).token, test_job(46).transaction_id, 1)
        .unwrap();
    handle.join().unwrap();
}

#[test]
fn forced_shutdown_abandonment_wakes_inflight_worker_without_next_submit() {
    let executor = Arc::new(ScriptedExecutor {
        outcomes: Mutex::new(VecDeque::from([Ok(()), Ok(())])),
        submitted: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor.clone()).unwrap();
    reserve_for_test(&handle, test_job(47).kind)
        .enqueue(test_job(47))
        .unwrap();
    wait_for_fence_event(
        &handle,
        47,
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.token.get() == 47),
    );
    reserve_for_test(&handle, test_job(48).kind)
        .enqueue(test_job(48))
        .unwrap();

    let snapshot = handle.begin_shutdown_quiesce().unwrap();
    assert_eq!(
        snapshot.queued_job.as_ref().map(|job| job.token.get()),
        Some(48)
    );
    assert_eq!(
        snapshot.inflight.map(|inflight| inflight.token.get()),
        Some(47)
    );

    let abandoned = handle.force_shutdown_abandon().unwrap();
    assert_eq!(
        abandoned.disposition,
        KmsWorkerForcedShutdownDisposition::InFlightAbandoned
    );
    assert_eq!(
        abandoned.inflight.map(|inflight| (
            inflight.token.get(),
            inflight.transaction_id.get(),
            inflight.kind
        )),
        Some((47, 47, test_job(47).kind))
    );
    let repeated = handle.force_shutdown_abandon().unwrap();
    assert!(matches!(
        repeated.disposition,
        KmsWorkerForcedShutdownDisposition::AlreadyAbandoned
            | KmsWorkerForcedShutdownDisposition::AlreadyStopped
    ));
    assert!(repeated.inflight.is_none());
    handle.join().unwrap();
    assert_eq!(*executor.submitted.lock().unwrap(), vec![47]);
}

#[test]
fn forced_shutdown_abandonment_is_idempotent_after_late_ack_stops_worker() {
    let executor = Arc::new(ScriptedExecutor {
        outcomes: Mutex::new(VecDeque::from([Ok(())])),
        submitted: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor).unwrap();
    let job = test_job(49);
    reserve_for_test(&handle, job.kind).enqueue(job).unwrap();
    wait_for_fence_event(
        &handle,
        49,
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.token.get() == 49),
    );

    let snapshot = handle.begin_shutdown_quiesce().unwrap();
    assert_eq!(
        snapshot.inflight.map(|inflight| inflight.token.get()),
        Some(49)
    );
    handle
        .ack_pageflip(test_job(49).token, test_job(49).transaction_id, 1)
        .unwrap();
    for _ in 0..100 {
        if matches!(
            handle.try_reserve_admission(test_job(50).kind),
            Err(KmsWorkerAdmissionError::Stopped)
        ) {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }

    let forced = handle.force_shutdown_abandon().unwrap();
    assert_eq!(
        forced.disposition,
        KmsWorkerForcedShutdownDisposition::AlreadyStopped
    );
    assert!(forced.inflight.is_none());
    let repeated = handle.force_shutdown_abandon().unwrap();
    assert_eq!(
        repeated.disposition,
        KmsWorkerForcedShutdownDisposition::AlreadyStopped
    );
    handle
        .ack_pageflip(test_job(49).token, test_job(49).transaction_id, 1)
        .unwrap();
    handle.join().unwrap();
    assert_eq!(handle.metrics_snapshot().duplicate_pageflip_acks, 0);
}

#[test]
fn primary_job_keeps_queued_cursor_pin_until_submission_completes() {
    let executor = Arc::new(ScriptedExecutor {
        outcomes: Mutex::new(VecDeque::from([Ok(()), Ok(())])),
        submitted: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor).unwrap();
    let cursor = test_cursor_for_worker();
    let state = AtomicCursorVisualState {
        framebuffer_id: Some(91),
        visible: true,
        ..AtomicCursorVisualState::hidden(64, 64)
    };
    let pin = cursor.pin_framebuffer_for(&state).unwrap();
    let pin_observer = pin.clone();
    let mut arbitration = NativeCursorOutputArbitration::default();
    arbitration.request(10, 1, 100);
    arbitration.request(11, 2, 100);
    let first = test_job(51);
    let first_identity = first.identity();
    reserve_for_test(&handle, test_job(51).kind)
        .enqueue(first)
        .unwrap();
    let first_submission = wait_for_fence_event(
        &handle,
        51,
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.token.get() == 51),
    );

    let mut second = test_composited_primary_job_with_cursor(52, 91, pin);
    second.validation_base = KmsValidationBase::Predecessor(first_identity);
    reserve_for_test(
        &handle,
        AtomicCommitKind::CompositedPrimary {
            transaction_id: test_job(52).transaction_id,
            frame_id: 52,
            framebuffer_id: 91,
        },
    )
    .enqueue(second)
    .unwrap();
    assert!(pin_observer.is_job_owned());
    drop(cursor);
    assert!(pin_observer.is_job_owned());

    handle
        .ack_pageflip(test_job(51).token, test_job(51).transaction_id, 1)
        .unwrap();
    drop(first_submission);
    let second_submission = wait_for_fence_event(
        &handle,
        52,
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.token.get() == 52),
    );
    arbitration.consume_submitted_epoch(10, 120, 200);
    assert!(arbitration.pending());
    assert_eq!(arbitration.desired_epoch(), 11);
    assert!(pin_observer.is_job_owned());
    handle
        .ack_pageflip(test_job(52).token, test_job(52).transaction_id, 1)
        .unwrap();
    drop(second_submission);
    for _ in 0..100 {
        if !pin_observer.is_job_owned() {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(!pin_observer.is_job_owned());
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn primary_job_preserves_exact_cursor_pin_across_busy_retry() {
    let executor = Arc::new(CursorRecordingExecutor {
        outcomes: Mutex::new(VecDeque::from([
            Err(oblivion_one::native::kms::AtomicKmsErrorKind::Busy),
            Ok(()),
        ])),
        attempts: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor.clone()).unwrap();
    let cursor = test_cursor_for_worker();
    let state = AtomicCursorVisualState {
        framebuffer_id: Some(91),
        visible: true,
        ..AtomicCursorVisualState::hidden(64, 64)
    };
    let pin = cursor.pin_framebuffer_for(&state).unwrap();
    let pin_observer = pin.clone();
    reserve_for_test(&handle, test_job(53).kind)
        .enqueue(test_primary_job_with_cursor(53, 91, pin))
        .unwrap();
    wait_for_fence_event(
        &handle,
        53,
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.token.get() == 53),
    );
    assert_eq!(*executor.attempts.lock().unwrap(), vec![(91, 91), (91, 91)]);
    assert!(pin_observer.is_job_owned());
    handle
        .ack_pageflip(test_job(53).token, test_job(53).transaction_id, 1)
        .unwrap();
    drop(cursor);
    drop(pin_observer);
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn rejected_primary_job_releases_cursor_pin_once() {
    let executor = Arc::new(ScriptedExecutor {
        outcomes: Mutex::new(VecDeque::from([Err(
            oblivion_one::native::kms::AtomicKmsErrorKind::FlipRejected,
        )])),
        submitted: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor).unwrap();
    let cursor = test_cursor_for_worker();
    let state = AtomicCursorVisualState {
        framebuffer_id: Some(91),
        visible: true,
        ..AtomicCursorVisualState::hidden(64, 64)
    };
    let pin = cursor.pin_framebuffer_for(&state).unwrap();
    let pin_observer = pin.clone();
    reserve_for_test(
        &handle,
        AtomicCommitKind::CompositedPrimary {
            transaction_id: test_job(54).transaction_id,
            frame_id: 54,
            framebuffer_id: 91,
        },
    )
    .enqueue(test_composited_primary_job_with_cursor(54, 91, pin))
    .unwrap();
    let events = wait_for_fence_event(
        &handle,
        54,
        |event| matches!(event, KmsWorkerEvent::SubmitRejected { job, .. } if job.token.get() == 54),
    );
    drop(events);
    drop(cursor);
    assert!(!pin_observer.is_job_owned());
    drop(pin_observer);
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn shutdown_detaches_queued_primary_cursor_pin_once() {
    let executor = Arc::new(ScriptedExecutor {
        outcomes: Mutex::new(VecDeque::from([Ok(())])),
        submitted: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor).unwrap();
    let cursor = test_cursor_for_worker();
    let state = AtomicCursorVisualState {
        framebuffer_id: Some(91),
        visible: true,
        ..AtomicCursorVisualState::hidden(64, 64)
    };
    let pin = cursor.pin_framebuffer_for(&state).unwrap();
    let pin_observer = pin.clone();
    reserve_for_test(&handle, test_job(55).kind)
        .enqueue(test_job(55))
        .unwrap();
    wait_for_fence_event(
        &handle,
        55,
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.token.get() == 55),
    );
    reserve_for_test(
        &handle,
        AtomicCommitKind::CompositedPrimary {
            transaction_id: test_job(56).transaction_id,
            frame_id: 56,
            framebuffer_id: 91,
        },
    )
    .enqueue(test_composited_primary_job_with_cursor(56, 91, pin))
    .unwrap();
    let snapshot = handle.begin_shutdown_quiesce().unwrap();
    drop(snapshot.queued_job);
    drop(cursor);
    assert!(!pin_observer.is_job_owned());
    handle
        .ack_pageflip(test_job(55).token, test_job(55).transaction_id, 1)
        .unwrap();
    handle.join().unwrap();
    drop(pin_observer);
}

#[derive(Debug)]
struct CursorRecordingExecutor {
    outcomes: Mutex<VecDeque<Result<(), oblivion_one::native::kms::AtomicKmsErrorKind>>>,
    attempts: Mutex<Vec<(u32, u32)>>,
}

impl KmsCommitExecutor for CursorRecordingExecutor {
    fn submit(&self, job: &KmsCommitJob) -> Result<KmsWorkerSubmission, KmsWorkerSubmitFailure> {
        let state = match &job.cursor {
            KmsCursorUpdate::Set(state) => state,
            KmsCursorUpdate::Disable | KmsCursorUpdate::Unchanged => {
                panic!("cursor recording executor requires a cursor assignment")
            }
        };
        self.attempts.lock().unwrap().push((
            state.framebuffer_id.expect("cursor framebuffer is present"),
            job.cursor_pin
                .as_ref()
                .expect("primary cursor job has a pin")
                .framebuffer_id()
                .get(),
        ));
        let result = self.outcomes.lock().unwrap().pop_front().unwrap_or(Ok(()));
        match result {
            Ok(()) => Ok(KmsWorkerSubmission { out_fence: None }),
            Err(kind) => Err(KmsWorkerSubmitFailure::new(kind, "fake Atomic ioctl")),
        }
    }
}
