use super::tests::{
    ScriptedExecutor, reserve_for_test, test_job, wait_for_fence_event, wait_for_inflight,
};
use super::thread::{KmsCommitExecutor, KmsWorkerSubmission, KmsWorkerSubmitFailure};
use super::{
    KmsCommitJob, KmsCommitWorkerHandle, KmsTestOnlyPolicy, KmsValidationBase, KmsWorkerEvent,
};
use crate::native_output::pacing::NativeFramePacing;
use crate::native_output::presentation::kms_timing::KmsSubmitWindow;
use crate::native_output::runtime::AtomicCommitKind;
use crate::native_output::scanout::{OutputFrameIdentitySnapshot, OutputFrameKey, OutputSlotId};
use oblivion_one::native::kms::AtomicKmsErrorKind;
use oblivion_one::native::presentation_deadline::MonotonicTimestampNs;
use oblivion_one::native::scheduler::NativeOutputPacingMode;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn monotonic_now_ns_for_test() -> u64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    assert_eq!(
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) },
        0
    );
    (time.tv_sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(time.tv_nsec as u64)
}

fn wait_until_monotonic_ns(deadline_ns: u64) {
    while monotonic_now_ns_for_test() < deadline_ns {
        std::thread::yield_now();
    }
}

fn test_submit_window(target_presentation_ns: u64, dispatch_budget_ns: u64) -> KmsSubmitWindow {
    KmsSubmitWindow::try_new(target_presentation_ns, 0, dispatch_budget_ns, 0)
        .expect("test submit window should be reachable")
}

fn pacing_test_physical_identity(frame_id: u64) -> OutputFrameIdentitySnapshot {
    OutputFrameIdentitySnapshot {
        frame_id,
        protocol_batch_id: oblivion_one::compositor::CompositorFrameBatchId::new(
            std::num::NonZeroU64::new(frame_id).unwrap(),
        ),
        transaction_id: crate::native_output::OutputTransactionId::new(
            std::num::NonZeroU64::new(frame_id).unwrap(),
        ),
        slot: OutputSlotId::new(1).unwrap(),
        framebuffer_id: oblivion_one::native::kms::FramebufferId::new(frame_id as u32).unwrap(),
        render_generation: frame_id,
        pool_generation: 1,
        target: None,
    }
}

#[derive(Debug)]
struct WorkerPacingTestExecutor {
    reject_test_only: bool,
}

impl KmsCommitExecutor for WorkerPacingTestExecutor {
    fn test_only(&self, _job: &KmsCommitJob) -> Result<(), KmsWorkerSubmitFailure> {
        if self.reject_test_only {
            Err(KmsWorkerSubmitFailure::new(
                AtomicKmsErrorKind::TestOnlyRejected,
                "worker pacing rejection test",
            ))
        } else {
            Ok(())
        }
    }

    fn submit(&self, _job: &KmsCommitJob) -> Result<KmsWorkerSubmission, KmsWorkerSubmitFailure> {
        Ok(KmsWorkerSubmission { out_fence: None })
    }
}

#[test]
fn predecessor_blocked_successor_overrun_does_not_train_dispatch_tail_guard() {
    let executor = Arc::new(WorkerPacingTestExecutor {
        reject_test_only: false,
    });
    let handle = KmsCommitWorkerHandle::start(executor).unwrap();
    let now_ns = monotonic_now_ns_for_test();

    let mut predecessor = test_job(7_010);
    predecessor.submit_window =
        test_submit_window(now_ns.saturating_add(1_000_000_000), 1_000_000_000);
    let predecessor_identity = predecessor.identity();
    let predecessor_token = predecessor.token;
    let predecessor_transaction = predecessor.transaction_id;

    let planned_worker_wake_at = now_ns.saturating_add(50_000_000);
    let mut successor = test_job(7_011);
    successor.validation_base = KmsValidationBase::Predecessor(predecessor_identity);
    successor.submit_window = test_submit_window(planned_worker_wake_at, 0);
    let successor_window = successor.submit_window;
    let successor_queued_at = monotonic_now_ns_for_test();
    successor.queued_at = MonotonicTimestampNs::new(successor_queued_at);

    let predecessor_dequeue_pause = handle.pause_after_dequeue_for_test();
    reserve_for_test(&handle, predecessor.kind)
        .enqueue(predecessor)
        .unwrap();
    predecessor_dequeue_pause.wait_until_selected();

    assert!(successor_queued_at <= successor_window.worker_wake_at_ns());
    reserve_for_test(&handle, successor.kind)
        .enqueue(successor)
        .unwrap();
    predecessor_dequeue_pause.release();
    wait_for_fence_event(
        &handle,
        7_010,
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.token.get() == 7_010),
    );

    let successor_dequeue_pause = handle.pause_after_dequeue_for_test();
    wait_until_monotonic_ns(planned_worker_wake_at);
    assert_eq!(handle.queue_depth(), 1);
    assert!(handle.inflight());
    handle
        .ack_pageflip(predecessor_token, predecessor_transaction, 1)
        .unwrap();
    successor_dequeue_pause.wait_until_selected();
    assert_eq!(handle.queue_depth(), 0);
    successor_dequeue_pause.release();

    wait_for_fence_event(
        &handle,
        7_011,
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.token.get() == 7_011),
    );
    let timing = handle.metrics_snapshot().timing;
    assert!(timing.dispatch_deadline_overrun_ns > 0);
    assert_eq!(timing.dispatch_tail_guard_ns, 0);
    assert_eq!(
        timing.dispatch_tail_guard_increases, 0,
        "a successor blocked by its predecessor must not train the dispatch tail guard"
    );

    handle
        .ack_pageflip(test_job(7_011).token, test_job(7_011).transaction_id, 1)
        .unwrap();
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn post_dequeue_worker_delay_remains_dispatch_tail_evidence() {
    let executor = Arc::new(WorkerPacingTestExecutor {
        reject_test_only: false,
    });
    let handle = KmsCommitWorkerHandle::start(executor).unwrap();
    let now_ns = monotonic_now_ns_for_test();
    let planned_worker_wake_at = now_ns.saturating_add(50_000_000);
    let mut job = test_job(7_012);
    job.submit_window = test_submit_window(planned_worker_wake_at, 0);
    job.queued_at = MonotonicTimestampNs::new(now_ns);

    let dequeue_pause = handle.pause_after_dequeue_for_test();
    reserve_for_test(&handle, job.kind).enqueue(job).unwrap();
    dequeue_pause.wait_until_selected();
    assert_eq!(handle.queue_depth(), 0);
    wait_until_monotonic_ns(planned_worker_wake_at);
    dequeue_pause.release();

    wait_for_fence_event(
        &handle,
        7_012,
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.token.get() == 7_012),
    );
    let timing = handle.metrics_snapshot().timing;
    assert!(timing.dispatch_deadline_overrun_ns > 0);
    assert_eq!(timing.dispatch_tail_guard_increases, 1);

    handle
        .ack_pageflip(test_job(7_012).token, test_job(7_012).transaction_id, 1)
        .unwrap();
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn worker_payload_preserves_exact_pacing_ticket_through_submit_event() {
    let executor = Arc::new(WorkerPacingTestExecutor {
        reject_test_only: false,
    });
    let handle = KmsCommitWorkerHandle::start(executor).unwrap();
    let mut pacing = NativeFramePacing::from_env();
    pacing.queue_visual(1, 1);
    let ticket = pacing
        .reserve_worker_submission(false)
        .unwrap()
        .expect("worker pacing ticket");

    let mut job = test_job(7_001);
    job.pacing_ticket = Some(ticket);
    reserve_for_test(&handle, job.kind).enqueue(job).unwrap();
    let events = wait_for_fence_event(
        &handle,
        7_001,
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.token.get() == 7_001),
    );
    let ownership = events
        .into_iter()
        .find_map(|event| match event {
            KmsWorkerEvent::Submitted { ownership } => Some(ownership),
            _ => None,
        })
        .expect("submitted worker ownership");

    assert_eq!(ownership.job.pacing_ticket, Some(ticket));
    pacing
        .note_worker_submit_exact(
            ownership.job.pacing_ticket,
            41,
            3,
            NativeOutputPacingMode::ReactiveDouble,
        )
        .unwrap();
    assert_eq!(pacing.pending, Some(ticket.frame_id()));
    assert!(pacing.reserve_worker_submission(false).unwrap().is_none());

    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn shutdown_admission_waits_for_inflight_publication_after_submit_returns() {
    let executor = Arc::new(ScriptedExecutor {
        outcomes: Mutex::new(VecDeque::from([Ok(())])),
        submitted: Mutex::new(Vec::new()),
    });
    let handle = Arc::new(KmsCommitWorkerHandle::start(executor).unwrap());
    let post_submit = handle.pause_after_submit_for_test();
    let job = test_job(44);
    let transaction_id = job.transaction_id;
    reserve_for_test(&handle, job.kind).enqueue(job).unwrap();
    post_submit.wait_until_selected();
    let submit_gate_available = handle.submit_gate_available_for_test();
    if submit_gate_available {
        post_submit.release();
    }
    assert!(
        !submit_gate_available,
        "submit gate must remain held until the successful submission is published inflight"
    );

    let (started_sender, started_receiver) = std::sync::mpsc::channel();
    let (done_sender, done_receiver) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        let handle = Arc::clone(&handle);
        scope.spawn(move || {
            started_sender.send(()).unwrap();
            done_sender
                .send(handle.begin_shutdown_quiesce().unwrap())
                .unwrap();
        });
        started_receiver.recv().unwrap();
        post_submit.release();
        let snapshot = done_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("shutdown should complete after in-flight publication");
        assert!(snapshot.queued_job.is_none());
        assert_eq!(
            snapshot.inflight.map(|inflight| inflight.token.get()),
            Some(44)
        );
        assert_eq!(
            snapshot.inflight.map(|inflight| inflight.transaction_id),
            Some(transaction_id)
        );
    });

    wait_for_inflight(&handle);
    handle
        .ack_pageflip(test_job(44).token, transaction_id, 1)
        .unwrap();
    handle.join().unwrap();
}

#[test]
fn worker_rejection_cancels_old_ticket_without_touching_same_logical_successor() {
    let executor = Arc::new(WorkerPacingTestExecutor {
        reject_test_only: true,
    });
    let handle = KmsCommitWorkerHandle::start(executor).unwrap();
    let mut pacing = NativeFramePacing::from_env();
    pacing.queue_visual(1, 1);
    let logical_frame = pacing.active.expect("normal predecessor");
    let predecessor_ticket = pacing
        .reserve_worker_submission(false)
        .unwrap()
        .expect("predecessor ticket");
    pacing
        .note_render_started(NativeOutputPacingMode::PredictiveTriple, true)
        .unwrap();
    let successor_attempt = pacing
        .active_predictive_attempt_id()
        .expect("successor attempt");
    let physical = pacing_test_physical_identity(7_002);
    pacing.bind_predictive_o1(physical).unwrap();
    pacing.note_render_ready();
    pacing.note_ready_frame(2, true);

    let mut job = test_job(7_003);
    job.kind = AtomicCommitKind::DirectPrimary {
        transaction_id: job.transaction_id,
        direct_token: job.token,
        framebuffer_id: 42,
    };
    job.test_policy.primary = KmsTestOnlyPolicy::Required;
    job.pacing_ticket = Some(predecessor_ticket);
    reserve_for_test(&handle, job.kind).enqueue(job).unwrap();
    let events = wait_for_fence_event(
        &handle,
        7_003,
        |event| matches!(event, KmsWorkerEvent::TestRejected { job, .. } if job.token.get() == 7_003),
    );
    let rejected_job = events
        .into_iter()
        .find_map(|event| match event {
            KmsWorkerEvent::TestRejected { job, .. } => Some(job),
            _ => None,
        })
        .expect("rejected worker job");

    assert_eq!(rejected_job.pacing_ticket, Some(predecessor_ticket));
    assert!(pacing.cancel_worker_submission(rejected_job.pacing_ticket));
    assert_eq!(pacing.ready, Some(logical_frame));
    assert_eq!(
        pacing.ready_predictive_attempt_id(),
        Some(successor_attempt)
    );
    assert_eq!(
        pacing.ready_physical_key(),
        Some(OutputFrameKey::from(&physical))
    );

    let successor_ticket = pacing
        .reserve_worker_submission(true)
        .unwrap()
        .expect("successor ticket");
    assert!(pacing.cancel_worker_submission(Some(successor_ticket)));
    assert!(pacing.reserve_worker_submission(true).unwrap().is_none());

    handle.request_quiesce();
    handle.join().unwrap();
}
