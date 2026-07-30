use std::sync::Arc;

use super::*;
use crate::native_output::kms_worker::{
    CursorSidecar, CursorSidecarCoupling, KmsCommitWorkerHandle, KmsTestOnlyPolicy,
    KmsWorkerAdmissionError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WorkerQueueOutcome {
    Queued {
        transaction_id: OutputTransactionId,
        token: PageFlipToken,
        framebuffer_id: FramebufferId,
    },
    CursorQueued {
        transaction_id: OutputTransactionId,
        token: PageFlipToken,
    },
    SidecarQueued {
        transaction_id: OutputTransactionId,
    },
    Unavailable(KmsWorkerAdmissionError),
}

#[allow(clippy::too_many_arguments)]
pub(super) fn try_offer_cursor_sidecar(
    worker: &KmsCommitWorkerHandle,
    cursor: &mut NativeAtomicCursor,
    desired: Option<&AtomicCursorVisualState>,
    output_transactions: &mut OutputTransactionLedger,
    presentation_trace: &mut PresentationTransactionTraceRing,
    transaction_id: OutputTransactionId,
    target: PresentationTarget,
    crtc_id: u32,
) -> NativeResult<Option<WorkerQueueOutcome>> {
    if !worker.has_attachable_primary_opportunity() {
        return Ok(None);
    }
    let descriptor = output_transactions
        .transaction(transaction_id)
        .ok_or_else(|| io::Error::other("sidecar transaction disappeared"))?
        .descriptor();
    let OutputTransactionContent::PlaneDelta {
        cursor_sidecar_id, ..
    } = descriptor.content()
    else {
        unreachable!("cursor plane-delta constructor returned another content kind");
    };
    let sidecar = CursorSidecar {
        id: cursor_sidecar_id,
        transaction: Arc::new(descriptor.clone()),
        revision: cursor.desired_revision(),
        assignment: descriptor.planes().cursor().clone(),
        lease: desired
            .filter(|state| state.framebuffer_id.is_some())
            .map(|state| cursor.pin_framebuffer_for(state))
            .transpose()?,
        coupling: CursorSidecarCoupling::Independent,
        created_at: descriptor.created_at(),
        deadline: target,
        crtc_id,
        test_policy: KmsTestOnlyPolicy::Required,
    };
    match worker.offer_cursor_sidecar(sidecar) {
        Ok(replaced) => {
            if let Some(replaced) = replaced {
                output_transactions
                    .mark_superseded(
                        replaced.transaction.id(),
                        Some(transaction_id),
                        OutputTransactionSupersedeReason::NewerTransaction,
                        MonotonicTimestampNs::new(monotonic_now_ns()?),
                    )
                    .map_err(io::Error::other)?;
            }
            presentation_trace.push(PresentationTransactionEvent::WorkerQueued {
                transaction_id,
                timestamp_ns: monotonic_now_ns()?,
            });
            Ok(Some(WorkerQueueOutcome::SidecarQueued { transaction_id }))
        }
        Err(error) => {
            output_transactions
                .mark_superseded(
                    transaction_id,
                    None,
                    OutputTransactionSupersedeReason::SameContentSuppressed,
                    MonotonicTimestampNs::new(monotonic_now_ns()?),
                )
                .map_err(io::Error::other)?;
            Ok(Some(WorkerQueueOutcome::Unavailable(error.reason)))
        }
    }
}

pub(super) fn cursor_worker_opportunities(
    worker_mode: bool,
    worker: Option<&KmsCommitWorkerHandle>,
    arbiter: &AtomicCommitArbiter,
) -> (bool, bool) {
    (
        super::presentation_worker::worker_cursor_queue_available(worker_mode, worker, arbiter),
        worker_mode
            && worker.is_some_and(KmsCommitWorkerHandle::has_attachable_primary_opportunity),
    )
}
