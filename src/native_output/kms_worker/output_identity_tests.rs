use super::tests::{reserve_for_test, test_job, wait_for_fence_event};
use super::thread::{KmsCommitExecutor, KmsWorkerSubmission, KmsWorkerSubmitFailure};
use super::{
    CursorSidecar, CursorSidecarCoupling, EstablishedKmsBase, KmsBundleOwners,
    KmsCommitBundleIdentity, KmsCommitJob, KmsCommitWorkerHandle, KmsTestOnlyPolicy,
    KmsValidationBase, KmsWorkerEvent, PendingBundleSnapshot,
};
use crate::native_output::presentation::plane::{
    CursorRevision, CursorSidecarId, PresentedCursorDelivery,
};
use crate::native_output::{
    CursorPlaneAssignment, OutputReleasePlan, OutputSlotId, OutputTransaction,
};
use oblivion_one::compositor::CompositorFrameBatchId;
use oblivion_one::core::OutputId;
use oblivion_one::native::presentation_deadline::MonotonicTimestampNs;
use oblivion_one::native::scheduler::NativeOutputPacingMode;
use std::num::NonZeroU64;
use std::sync::Arc;

struct AcceptingExecutor;

impl KmsCommitExecutor for AcceptingExecutor {
    fn submit(&self, _job: &KmsCommitJob) -> Result<KmsWorkerSubmission, KmsWorkerSubmitFailure> {
        Ok(KmsWorkerSubmission { out_fence: None })
    }
}

fn offer_sidecar(handle: &KmsCommitWorkerHandle, mut sidecar: CursorSidecar) {
    for _ in 0..1_000 {
        match handle.offer_cursor_sidecar(sidecar) {
            Ok(_) => return,
            Err(error) if error.reason == super::KmsWorkerAdmissionError::AdmissionContention => {
                sidecar = *error.sidecar;
                std::thread::yield_now();
            }
            Err(error) => panic!("sidecar offer failed: {:?}", error.reason),
        }
    }
    panic!("sidecar offer remained contended");
}

fn test_sidecar(job: &KmsCommitJob) -> CursorSidecar {
    let transaction_id = crate::native_output::OutputTransactionId::new(
        NonZeroU64::new(job.transaction_id.get().saturating_mul(100)).unwrap(),
    );
    let transaction = Arc::new(
        OutputTransaction::cursor_plane_delta(
            job.output_id,
            transaction_id,
            job.output_generation,
            job.target.presentation_time,
            job.target,
            oblivion_one::native::scheduler::NativeOutputPacingMode::ReactiveDouble,
            transaction_id.get(),
            None,
            OutputReleasePlan::Pageflip,
        )
        .unwrap(),
    );
    CursorSidecar {
        id: CursorSidecarId::new(NonZeroU64::new(job.transaction_id.get()).unwrap()),
        transaction,
        revision: CursorRevision::initial(),
        assignment: CursorPlaneAssignment::Atomic {
            desired_epoch: job.transaction_id.get(),
            state: None,
        },
        lease: None,
        coupling: CursorSidecarCoupling::Independent,
        created_at: job.target.presentation_time,
        deadline: job.target,
        crtc_id: job.crtc_id,
        test_policy: KmsTestOnlyPolicy::Skip,
        cursor_delivery: PresentedCursorDelivery::Hidden,
        capability_key: None,
        trace_reveal: None,
        validation_base: job.validation_base,
    }
}

fn test_job_with_cursor_owner(token: u64) -> KmsCommitJob {
    let mut job = test_job(token);
    let transaction = Arc::new(
        OutputTransaction::composited(
            job.output_id,
            job.transaction_id,
            job.output_generation,
            MonotonicTimestampNs::new(0),
            job.target,
            NativeOutputPacingMode::ReactiveDouble,
            token,
            1,
            1,
            OutputSlotId::new(0).unwrap(),
            42,
            Some(CursorPlaneAssignment::Atomic {
                desired_epoch: token,
                state: None,
            }),
            CompositorFrameBatchId::new(NonZeroU64::new(token).unwrap()),
        )
        .unwrap(),
    );
    job.kind = crate::native_output::runtime::AtomicCommitKind::CompositedPrimary {
        transaction_id: job.transaction_id,
        frame_id: token,
        framebuffer_id: 42,
    };
    job.owners = KmsBundleOwners::for_transaction(
        job.kind,
        transaction,
        Some(CursorRevision::initial()),
        None,
    )
    .unwrap();
    job
}

#[test]
fn wrong_output_pageflip_ack_preserves_inflight_and_queued_dependents() {
    let handle = KmsCommitWorkerHandle::start(Arc::new(AcceptingExecutor)).unwrap();
    let first = test_job_with_cursor_owner(20_001);
    let first_identity = first.identity();
    let first_transaction_id = first.transaction_id;
    assert_eq!(
        first_identity.cursor_transaction_id,
        Some(first_transaction_id)
    );

    reserve_for_test(&handle, first.kind)
        .enqueue(first)
        .unwrap();
    wait_for_fence_event(
        &handle,
        20_001,
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.identity() == first_identity),
    );

    let mut dependent = test_job(20_002);
    dependent.validation_base = KmsValidationBase::Predecessor(first_identity);
    reserve_for_test(&handle, dependent.kind)
        .enqueue(dependent)
        .unwrap();
    let sidecar = test_sidecar(&test_job(20_003));
    let sidecar_id = sidecar.id;
    offer_sidecar(&handle, sidecar);
    let expected_base = Some(EstablishedKmsBase::Pending(first_identity));
    assert_eq!(handle.established_base_for_test(), expected_base);

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
    assert_eq!(handle.established_base_for_test(), expected_base);
    assert_eq!(handle.pending_cursor_sidecar_id(), Some(sidecar_id));

    handle
        .ack_pageflip_identity(first_identity, first_transaction_id)
        .unwrap();
    assert!(!handle.inflight());
    handle.request_quiesce();
    handle.join().unwrap();
}
