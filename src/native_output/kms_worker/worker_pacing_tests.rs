use super::tests::{
    ScriptedExecutor, reserve_for_test, test_job, wait_for_fence_event, wait_for_inflight,
};
use super::thread::{KmsCommitExecutor, KmsWorkerSubmission, KmsWorkerSubmitFailure};
use super::{KmsCommitJob, KmsCommitWorkerHandle, KmsTestOnlyPolicy, KmsWorkerEvent};
use crate::native_output::pacing::NativeFramePacing;
use crate::native_output::runtime::AtomicCommitKind;
use crate::native_output::scanout::{OutputFrameIdentitySnapshot, OutputFrameKey, OutputSlotId};
use oblivion_one::native::kms::AtomicKmsErrorKind;
use oblivion_one::native::scheduler::NativeOutputPacingMode;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
    assert!(
        !handle.submit_gate_available_for_test(),
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
