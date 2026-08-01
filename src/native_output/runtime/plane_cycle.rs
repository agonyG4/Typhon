use std::sync::Arc;

use super::*;
use crate::native_output::kms_worker::{
    CursorSidecar, CursorSidecarCoupling, KmsBundleOwners, KmsCommitJob, KmsCommitWorkerHandle,
    KmsCursorUpdate, KmsPrimaryUpdate, KmsTestOnlyPolicy, KmsWorkerAdmissionError,
};
use crate::native_output::presentation::plane::CursorRevision;
use crate::native_output::runtime::presentation_transactions::settle_failed_output_transaction;

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

pub(super) enum PlaneDeltaPreparation {
    Return(WorkerQueueOutcome),
    Submit {
        transaction_id: OutputTransactionId,
        desired: Option<AtomicCursorVisualState>,
        cursor_epoch: u64,
        owned_revision: Option<CursorRevision>,
        cursor_pin: Option<CursorFramebufferPin>,
        target: PresentationTarget,
    },
}

pub(super) fn plane_delta_reservation_outcome(
    result: Result<(), &'static str>,
) -> Result<(), KmsWorkerAdmissionError> {
    result.map_err(|_| KmsWorkerAdmissionError::QueueFull)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn queue_plane_delta(
    worker: &KmsCommitWorkerHandle,
    cursor: &mut NativeAtomicCursor,
    desired: Option<AtomicCursorVisualState>,
    atomic_commit_arbiter: &mut AtomicCommitArbiter,
    output_transactions: &mut OutputTransactionLedger,
    presentation_trace: &mut PresentationTransactionTraceRing,
    target: PresentationTarget,
    crtc_id: u32,
    output_generation: u64,
    pacing_mode: NativeOutputPacingMode,
    cursor_epoch: u64,
) -> NativeResult<WorkerQueueOutcome> {
    let preparation = prepare_plane_delta(
        worker,
        cursor,
        desired,
        output_transactions,
        presentation_trace,
        target,
        crtc_id,
        output_generation,
        pacing_mode,
        cursor_epoch,
    )?;
    let (transaction_id, desired, cursor_epoch, owned_revision, promoted_pin, target) =
        match preparation {
            PlaneDeltaPreparation::Return(outcome) => return Ok(outcome),
            PlaneDeltaPreparation::Submit {
                transaction_id,
                desired,
                cursor_epoch,
                owned_revision,
                cursor_pin,
                target,
            } => (
                transaction_id,
                desired,
                cursor_epoch,
                owned_revision,
                cursor_pin,
                target,
            ),
        };
    let permit = match worker.try_reserve_admission_slot() {
        Ok(permit) => permit,
        Err(error) => {
            output_transactions
                .mark_superseded(
                    transaction_id,
                    None,
                    OutputTransactionSupersedeReason::SameContentSuppressed,
                    MonotonicTimestampNs::new(monotonic_now_ns()?),
                )
                .map_err(io::Error::other)?;
            return Ok(WorkerQueueOutcome::Unavailable(error));
        }
    };
    let token = PageFlipToken::new(allocate_native_page_flip_token())
        .expect("allocated native pageflip token is nonzero");
    let queued_at_ns = monotonic_now_ns()?;
    let kind = AtomicCommitKind::PlaneDelta {
        transaction_id,
        cursor_epoch,
        framebuffer_id: desired.as_ref().and_then(|state| state.framebuffer_id),
    };
    if let Err(error) = output_transactions.mark_queued(
        transaction_id,
        output_generation,
        MonotonicTimestampNs::new(queued_at_ns),
    ) {
        settle_failed_output_transaction(
            output_transactions,
            transaction_id,
            OutputTransactionFailureStage::KmsSubmit,
            MonotonicTimestampNs::new(queued_at_ns),
            |_| Ok(()),
        )?;
        return Err(io::Error::other(error).into());
    }
    if let Err(reason) =
        plane_delta_reservation_outcome(atomic_commit_arbiter.reserve_worker_queued(
            token,
            output_generation,
            crtc_id,
            kind,
            queued_at_ns,
        ))
    {
        settle_failed_output_transaction(
            output_transactions,
            transaction_id,
            OutputTransactionFailureStage::BackendOwnershipTransfer,
            MonotonicTimestampNs::new(queued_at_ns),
            |_| Ok(()),
        )?;
        return Ok(WorkerQueueOutcome::Unavailable(reason));
    }
    let job = KmsCommitJob {
        bundle_id:
            crate::native_output::presentation::plane::KmsCommitBundleId::from_pageflip_token(token),
        owners: KmsBundleOwners::for_legacy_transaction(
            kind,
            Arc::new(
                output_transactions
                    .transaction(transaction_id)
                    .ok_or_else(|| io::Error::other("queued cursor transaction disappeared"))?
                    .descriptor()
                    .clone(),
            ),
        ),
        transaction_id,
        token,
        output_generation,
        crtc_id,
        kind,
        target,
        queued_at: MonotonicTimestampNs::new(queued_at_ns),
        primary: KmsPrimaryUpdate::Unchanged,
        cursor: desired
            .clone()
            .map_or(KmsCursorUpdate::Disable, KmsCursorUpdate::Set),
        cursor_pin: match promoted_pin {
            Some(pin) => Some(pin),
            None => desired
                .as_ref()
                .filter(|state| state.framebuffer_id.is_some())
                .map(|state| cursor.pin_framebuffer_for(state))
                .transpose()?,
        },
        direct_primary_lease: None,
        test_only_duration_ns: None,
        pacing_frame_id: None,
        test_only: if cursor.current_capability_proven() {
            KmsTestOnlyPolicy::Skip
        } else {
            KmsTestOnlyPolicy::Required
        },
        ready_submit: false,
    };
    let descriptor = output_transactions
        .transaction(transaction_id)
        .ok_or_else(|| io::Error::other("queued cursor transaction disappeared"))?;
    if let Err(error) = job.validate_against(descriptor.descriptor()) {
        atomic_commit_arbiter.reject_worker_queued(token);
        settle_failed_output_transaction(
            output_transactions,
            transaction_id,
            OutputTransactionFailureStage::BackendOwnershipTransfer,
            MonotonicTimestampNs::new(queued_at_ns),
            |_| Ok(()),
        )?;
        return Err(io::Error::other(format!("invalid cursor worker payload: {error:?}")).into());
    }
    let queued_visual_state = desired.clone().unwrap_or_else(|| {
        let mut hidden = cursor.desired().clone();
        hidden.visible = false;
        hidden.framebuffer_id = None;
        hidden
    });
    let queue_result = match owned_revision {
        Some(revision) => cursor.queue_owned_worker_submission(
            transaction_id,
            token,
            cursor_epoch,
            revision,
            queued_visual_state,
        ),
        None => {
            cursor.queue_worker_submission(transaction_id, token, cursor_epoch, queued_visual_state)
        }
    };
    if let Err(error) = queue_result {
        atomic_commit_arbiter.reject_worker_queued(token);
        settle_failed_output_transaction(
            output_transactions,
            transaction_id,
            OutputTransactionFailureStage::BackendOwnershipTransfer,
            MonotonicTimestampNs::new(queued_at_ns),
            |_| Ok(()),
        )?;
        return Err(error.into());
    }
    if let Err(error) = permit.enqueue(job) {
        drop(error.job);
        cursor.cancel_worker_submission(transaction_id, token, cursor_epoch)?;
        atomic_commit_arbiter.reject_worker_queued(token);
        settle_failed_output_transaction(
            output_transactions,
            transaction_id,
            OutputTransactionFailureStage::KmsSubmit,
            MonotonicTimestampNs::new(monotonic_now_ns()?),
            |_| Ok(()),
        )?;
        return Err(io::Error::other(format!(
            "cursor Atomic worker enqueue failed: {:?}",
            error.reason
        ))
        .into());
    }
    presentation_trace.push(PresentationTransactionEvent::WorkerQueued {
        transaction_id,
        timestamp_ns: monotonic_now_ns()?,
    });
    worker.record_cursor_worker_queued();
    Ok(WorkerQueueOutcome::CursorQueued {
        transaction_id,
        token,
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn prepare_plane_delta(
    worker: &KmsCommitWorkerHandle,
    cursor: &mut NativeAtomicCursor,
    desired: Option<AtomicCursorVisualState>,
    output_transactions: &mut OutputTransactionLedger,
    presentation_trace: &mut PresentationTransactionTraceRing,
    target: PresentationTarget,
    crtc_id: u32,
    output_generation: u64,
    pacing_mode: NativeOutputPacingMode,
    cursor_epoch: u64,
) -> NativeResult<PlaneDeltaPreparation> {
    if let Some(promoted) =
        take_promotable_cursor_sidecar(worker, output_generation, crtc_id, target)
    {
        let desired = match &promoted.assignment {
            CursorPlaneAssignment::Atomic { state, .. } => state.clone(),
            CursorPlaneAssignment::Disabled => None,
            CursorPlaneAssignment::Unchanged => {
                return Err(io::Error::other("promoted sidecar has no cursor update").into());
            }
        };
        let cursor_epoch = match promoted.assignment {
            CursorPlaneAssignment::Atomic { desired_epoch, .. } => desired_epoch,
            CursorPlaneAssignment::Disabled => cursor_epoch,
            CursorPlaneAssignment::Unchanged => unreachable!(),
        };
        return Ok(PlaneDeltaPreparation::Submit {
            transaction_id: promoted.transaction.id(),
            desired,
            cursor_epoch,
            owned_revision: Some(promoted.revision),
            cursor_pin: promoted.lease,
            target: promoted.deadline,
        });
    }

    let transaction_id = output_transactions
        .allocate_id()
        .map_err(io::Error::other)?;
    let transaction = OutputTransaction::cursor_plane_delta(
        transaction_id,
        output_generation,
        MonotonicTimestampNs::new(monotonic_now_ns()?),
        target,
        pacing_mode,
        cursor_epoch,
        desired.clone(),
        OutputReleasePlan::Pageflip,
    )
    .map_err(io::Error::other)?;
    output_transactions
        .insert(transaction)
        .map_err(io::Error::other)?;
    if let Some(outcome) = try_offer_cursor_sidecar(
        worker,
        cursor,
        desired.as_ref(),
        output_transactions,
        presentation_trace,
        transaction_id,
        target,
        crtc_id,
    )? {
        return Ok(PlaneDeltaPreparation::Return(outcome));
    }
    Ok(PlaneDeltaPreparation::Submit {
        transaction_id,
        desired,
        cursor_epoch,
        owned_revision: None,
        cursor_pin: None,
        target,
    })
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
    let visibility_transition =
        desired.is_some_and(|state| state.visible) != cursor.current().visible;
    let coupling = if visibility_transition {
        CursorSidecarCoupling::MustBundleWith(
            worker
                .attachable_primary_transaction_id()
                .ok_or_else(|| io::Error::other("coupled sidecar has no primary owner"))?,
        )
    } else {
        CursorSidecarCoupling::Independent
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
        coupling,
        created_at: descriptor.created_at(),
        deadline: target,
        crtc_id,
        test_policy: if cursor.current_capability_proven() {
            KmsTestOnlyPolicy::Skip
        } else {
            KmsTestOnlyPolicy::Required
        },
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

pub(super) fn take_promotable_cursor_sidecar(
    worker: &KmsCommitWorkerHandle,
    output_generation: u64,
    crtc_id: u32,
    target: PresentationTarget,
) -> Option<CursorSidecar> {
    worker.take_due_independent_cursor_sidecar(output_generation, crtc_id, target)
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

pub(super) fn output_refresh_interval(refresh_hz: u32) -> std::time::Duration {
    std::time::Duration::from_nanos(1_000_000_000 / u64::from(refresh_hz.max(1)))
}
