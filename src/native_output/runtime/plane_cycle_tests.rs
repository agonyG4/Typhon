use super::plane_cycle::{PlaneDeltaPreparation, prepare_plane_delta};
use crate::native_output::kms_worker::{
    CursorSidecar, CursorSidecarCoupling, KmsCommitExecutor, KmsCommitWorkerHandle,
    KmsTestOnlyPolicy, KmsValidationBase, KmsWorkerSubmission, KmsWorkerSubmitFailure,
};
use crate::native_output::output::test_cursor_for_worker;
use crate::native_output::presentation::plane::{
    CursorRevision, CursorSidecarId, KmsCommitBundleId, PresentedCursorDelivery,
    PresentedPlaneSnapshot,
};
use crate::native_output::presentation::trace::PresentationTransactionTraceRing;
use crate::native_output::runtime::NativeOutputPacingMode;
use crate::native_output::{
    CursorPlaneAction, OutputReleasePlan, OutputTransaction, OutputTransactionId,
    OutputTransactionLedger,
};
use oblivion_one::native::presentation_deadline::{
    MonotonicTimestampNs, PresentationTarget, PresentationTargetReason,
};
use std::num::NonZeroU64;
use std::sync::Arc;
use std::time::Duration;

struct AcceptingExecutor;

impl KmsCommitExecutor for AcceptingExecutor {
    fn test_only(
        &self,
        _job: &crate::native_output::kms_worker::KmsCommitJob,
    ) -> Result<(), KmsWorkerSubmitFailure> {
        Ok(())
    }

    fn submit(
        &self,
        _job: &crate::native_output::kms_worker::KmsCommitJob,
    ) -> Result<KmsWorkerSubmission, KmsWorkerSubmitFailure> {
        Ok(KmsWorkerSubmission { out_fence: None })
    }
}

fn target() -> PresentationTarget {
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

fn transaction_id(value: u64) -> OutputTransactionId {
    OutputTransactionId::new(NonZeroU64::new(value).expect("non-zero transaction ID"))
}

fn predecessor(value: u64) -> crate::native_output::kms_worker::KmsCommitBundleIdentity {
    let token = oblivion_one::native::kms::PageFlipToken::new(value).expect("non-zero token");
    crate::native_output::kms_worker::KmsCommitBundleIdentity {
        id: KmsCommitBundleId::from_pageflip_token(token),
        token,
        output_generation: 1,
        crtc_id: 7,
        primary_transaction_id: Some(transaction_id(value)),
        cursor_transaction_id: None,
    }
}

fn independent_hidden_sidecar(validation_base: KmsValidationBase) -> CursorSidecar {
    let target = target();
    let transaction = OutputTransaction::cursor_plane_delta(
        transaction_id(91),
        1,
        MonotonicTimestampNs::new(1),
        target,
        NativeOutputPacingMode::ReactiveDouble,
        91,
        None,
        OutputReleasePlan::Pageflip,
    )
    .expect("cursor sidecar transaction");
    CursorSidecar {
        id: CursorSidecarId::new(NonZeroU64::new(91).expect("sidecar ID")),
        transaction: Arc::new(transaction),
        revision: CursorRevision::initial(),
        assignment: crate::native_output::CursorPlaneAssignment::Disabled,
        lease: None,
        coupling: CursorSidecarCoupling::Independent,
        created_at: MonotonicTimestampNs::new(1),
        deadline: target,
        crtc_id: 7,
        test_policy: KmsTestOnlyPolicy::Required,
        cursor_delivery: PresentedCursorDelivery::Hidden,
        capability_key: None,
        validation_base,
    }
}

#[test]
fn promoted_independent_sidecar_uses_fresh_standalone_validation_base() {
    let worker = KmsCommitWorkerHandle::start(Arc::new(AcceptingExecutor)).unwrap();
    let old_base = KmsValidationBase::Presented {
        snapshot: PresentedPlaneSnapshot::legacy(None),
        output_generation: 1,
        crtc_id: 7,
    };
    worker
        .offer_cursor_sidecar(independent_hidden_sidecar(old_base))
        .unwrap();

    let fresh_base = KmsValidationBase::Predecessor(predecessor(92));
    let mut cursor = test_cursor_for_worker();
    let mut transactions = OutputTransactionLedger::new();
    let mut trace = PresentationTransactionTraceRing::disabled(8);
    let preparation = prepare_plane_delta(
        &worker,
        &mut cursor,
        None,
        &mut transactions,
        &mut trace,
        target(),
        7,
        1,
        NativeOutputPacingMode::ReactiveDouble,
        92,
        fresh_base,
        None,
        CursorPlaneAction::Independent,
        PresentedCursorDelivery::Hidden,
    )
    .unwrap();

    let PlaneDeltaPreparation::Submit(preparation) = preparation else {
        panic!("independent sidecar should be promoted for standalone submission");
    };
    assert_eq!(preparation.validation_base, fresh_base);
    assert_eq!(preparation.transaction_id, transaction_id(91));
    assert_eq!(preparation.cursor_delivery, PresentedCursorDelivery::Hidden);

    worker.request_quiesce();
    worker.join().unwrap();
}
