use super::tests::{reserve_for_test, test_job, wait_for_fence_event};
use super::thread::{KmsCommitExecutor, KmsWorkerSubmission, KmsWorkerSubmitFailure};
use super::{
    KmsCommitBundleIdentity, KmsCommitJob, KmsCommitWorkerHandle, KmsValidationBase,
    KmsWorkerEvent, PendingBundleSnapshot,
};
use oblivion_one::core::OutputId;
use std::sync::Arc;

struct AcceptingExecutor;

impl KmsCommitExecutor for AcceptingExecutor {
    fn submit(&self, _job: &KmsCommitJob) -> Result<KmsWorkerSubmission, KmsWorkerSubmitFailure> {
        Ok(KmsWorkerSubmission { out_fence: None })
    }
}

#[test]
fn wrong_output_pageflip_ack_preserves_inflight_and_queued_dependents() {
    let handle = KmsCommitWorkerHandle::start(Arc::new(AcceptingExecutor)).unwrap();
    let first = test_job(20_001);
    let first_identity = first.identity();
    let first_transaction_id = first.transaction_id;

    reserve_for_test(&handle, first.kind).enqueue(first).unwrap();
    wait_for_fence_event(&handle, 20_001, |event| {
        matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.identity() == first_identity)
    });

    let mut dependent = test_job(20_002);
    dependent.validation_base = KmsValidationBase::Predecessor(first_identity);
    reserve_for_test(&handle, dependent.kind)
        .enqueue(dependent)
        .unwrap();

    let wrong_identity = KmsCommitBundleIdentity {
        output_id: OutputId::from_raw(2).expect("test output identity is nonzero"),
        ..first_identity
    };
    let mismatches_before = handle.metrics_snapshot().result_mismatches;

    assert_eq!(
        handle.ack_pageflip_identity(wrong_identity, first_transaction_id),
        Err(super::thread::KmsWorkerAckError::OutputMismatch)
    );
    assert_eq!(
        handle.metrics_snapshot().result_mismatches,
        mismatches_before + 1
    );
    assert_eq!(
        handle.pending_bundle_snapshot(first_identity.output_generation, first_identity.crtc_id),
        Some(PendingBundleSnapshot::InFlight(first_identity))
    );
    assert_eq!(handle.queue_depth(), 1);

    handle
        .ack_pageflip_identity(first_identity, first_transaction_id)
        .unwrap();
    assert!(!handle.inflight());
    handle.request_quiesce();
    handle.join().unwrap();
}
