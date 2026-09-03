use super::super::*;
use crate::native_output::kms_worker::{KmsCommitJob, KmsCursorUpdate, KmsPrimaryUpdate};
use oblivion_one::native::kms::AtomicKmsError;

use super::super::presentation_transactions::{
    DirectTerminalCallbackDisposition, direct_terminal_callback_owner_leaks,
    settle_dropped_output_transaction, settle_failed_output_transaction,
};
use super::direct_rejection::WorkerRejectionKind;
use crate::native_output::scanout::{AtomicEglGbmScanout, FrozenCursorPlaneOwner};

#[allow(clippy::too_many_arguments)]
pub(in crate::native_output::runtime) fn drop_queued_worker_job_with_reason_parts(
    job: KmsCommitJob,
    drop_reason: OutputTransactionDropReason,
    scene_history: &mut NativeSceneHistory,
    scanout: &mut AtomicEglGbmScanout,
    frame_pacing: &mut NativeFramePacing,
    frame_scheduler: &mut NativeFrameScheduler,
    atomic_cursor: &mut Option<NativeAtomicCursor>,
    cursor_output_arbitration: &mut NativeCursorOutputArbitration,
    atomic_commit_arbiter: &mut AtomicCommitArbiter,
    server: &mut OwnCompositorServer,
    output_transactions: &mut OutputTransactionLedger,
    kms_commit_worker: Option<&crate::native_output::kms_worker::KmsCommitWorkerHandle>,
) -> NativeResult<()> {
    scene_history.discard_submission(job.token.get());
    let sidecar_transaction_id = job
        .owners
        .cursor()
        .filter(|owner| owner.sidecar_id.is_some())
        .map(|owner| owner.transaction.id());
    if matches!(job.kind, AtomicCommitKind::DirectPrimary { .. })
        && let Some(duration_ns) = job.test_only_duration_ns
    {
        scanout.note_direct_test_only(duration_ns, false);
    }
    let compatibility_primary = matches!(job.kind, AtomicCommitKind::CompositedPrimary { .. })
        && output_transactions
            .transaction(job.transaction_id)
            .is_some_and(|transaction| {
                matches!(
                    transaction.descriptor().planes().primary(),
                    PrimaryPlaneAssignment::CompatibilityFramebuffer { .. }
                )
            });
    if let AtomicCommitKind::PlaneDelta { cursor_epoch, .. } = job.kind {
        let cursor = atomic_cursor
            .as_mut()
            .ok_or_else(|| io::Error::other("queued cursor job has no cursor"))?;
        cursor.cancel_worker_submission(job.transaction_id, job.token, cursor_epoch)?;
        cursor_output_arbitration.clear_pending();
    } else {
        if !frame_pacing.cancel_worker_submission(job.pacing_frame_id, job.ready_submit) {
            return Err(io::Error::other("worker shutdown pacing identity mismatch").into());
        }
        if compatibility_primary {
            let scheduler_cancel = if drop_reason == OutputTransactionDropReason::SafeAbandonment {
                frame_scheduler.abandon_worker_submission(job.token.get(), job.transaction_id.get())
            } else {
                frame_scheduler.cancel_worker_submission(job.token.get(), job.transaction_id.get())
            };
            if let Err(error) = scheduler_cancel {
                if let Some(worker) = kms_commit_worker {
                    worker.record_scheduler_cancel_mismatch();
                }
                return Err(io::Error::other(error).into());
            }
            if let Some(worker) = kms_commit_worker {
                worker.record_scheduler_queued_cancellation();
            }
        }
    }
    atomic_commit_arbiter.reject_worker_queued(job.token);
    if matches!(job.kind, AtomicCommitKind::CompositedPrimary { .. }) {
        if compatibility_primary {
            return Err(io::Error::other(
                "Atomic compatibility worker requires native EGL/GBM scanout",
            )
            .into());
        } else {
            scanout
                .suspend_abandon_worker_submission(job.token)
                .map_err(io::Error::other)?;
        }
    }
    let direct_obligations = if matches!(job.kind, AtomicCommitKind::DirectPrimary { .. }) {
        Some(
            output_transactions
                .transaction(job.transaction_id)
                .ok_or_else(|| io::Error::other("dropped direct transaction is missing"))?
                .descriptor()
                .obligations(),
        )
    } else {
        None
    };
    let direct_callback_owner_leaks = direct_obligations.map(|obligations| {
        direct_terminal_callback_owner_leaks(
            server,
            job.transaction_id,
            obligations,
            DirectTerminalCallbackDisposition::Abandoned,
        )
    });
    settle_dropped_output_transaction(
        output_transactions,
        job.transaction_id,
        drop_reason,
        MonotonicTimestampNs::new(monotonic_now_ns()?),
        |obligations| {
            if let Some(batch_id) = obligations.frame_batch_id() {
                server.complete_frame_batch_after_safe_abandonment(
                    batch_id,
                    FrameBatchDiscardReason::SuspendAbandonment,
                );
            }
            Ok(())
        },
    )?;
    if let Some(sidecar_transaction_id) = sidecar_transaction_id {
        settle_dropped_output_transaction(
            output_transactions,
            sidecar_transaction_id,
            drop_reason,
            MonotonicTimestampNs::new(monotonic_now_ns()?),
            |obligations| {
                debug_assert!(obligations.frame_batch_id().is_none());
                debug_assert!(obligations.direct_surface_id().is_none());
                Ok(())
            },
        )?;
    }
    if let Some(callback_owner_leaks) = direct_callback_owner_leaks {
        scanout.note_direct_callback_owner_leaks(callback_owner_leaks);
    }
    if drop_reason == OutputTransactionDropReason::SafeAbandonment
        && let Some(worker) = kms_commit_worker
    {
        worker.record_shutdown_queued_job_settled();
    }
    Ok(())
}

fn take_embedded_cursor_owner(
    job: &mut KmsCommitJob,
) -> NativeResult<Option<FrozenCursorPlaneOwner>> {
    let cursor_owner = job.owners.cursor();
    match (&job.cursor, cursor_owner) {
        (KmsCursorUpdate::Unchanged, None) => {
            if job.cursor_pin.is_some() {
                return Err(io::Error::other(
                    "unchanged cursor update unexpectedly owns a framebuffer pin",
                )
                .into());
            }
            Ok(None)
        }
        (KmsCursorUpdate::Unchanged, Some(_)) => {
            Err(io::Error::other("unchanged cursor update unexpectedly owns a cursor owner").into())
        }
        (KmsCursorUpdate::Set(state), Some(owner)) => {
            if owner.sidecar_id.is_some() || owner.transaction.id() != job.transaction_id {
                return Err(io::Error::other(
                    "invalidated primary cursor owner is not embedded in the primary",
                )
                .into());
            }
            let CursorPlaneAssignment::Atomic {
                state: Some(planned),
                ..
            } = owner.transaction.planes().cursor()
            else {
                return Err(io::Error::other(
                    "embedded cursor owner does not describe a visible Atomic assignment",
                )
                .into());
            };
            if planned != state {
                return Err(io::Error::other(
                    "embedded cursor owner does not match the frozen cursor payload",
                )
                .into());
            }
            if state.visible
                && job.cursor_delivery
                    != crate::native_output::presentation::plane::PresentedCursorDelivery::Hardware
            {
                return Err(io::Error::other(
                    "visible embedded cursor does not have hardware delivery",
                )
                .into());
            }
            match (state.framebuffer_id, job.cursor_pin.as_ref()) {
                (Some(framebuffer_id), Some(pin))
                    if pin.framebuffer_id().get() == framebuffer_id => {}
                (None, None) => {}
                (Some(_), Some(_)) => {
                    return Err(io::Error::other(
                        "embedded cursor pin does not match the frozen framebuffer",
                    )
                    .into());
                }
                (Some(_), None) => {
                    return Err(io::Error::other(
                        "embedded cursor framebuffer has no retained pin",
                    )
                    .into());
                }
                (None, Some(_)) => {
                    return Err(io::Error::other(
                        "embedded cursor pin exists without a frozen framebuffer",
                    )
                    .into());
                }
            }
            Ok(Some(FrozenCursorPlaneOwner {
                revision: owner.revision,
                client_source_key: None,
                capability_key: owner.capability_key,
                pin: job.cursor_pin.take(),
            }))
        }
        (KmsCursorUpdate::Set(_), None) => {
            Err(io::Error::other("frozen cursor Set update is missing its embedded owner").into())
        }
        (KmsCursorUpdate::Disable, Some(owner)) => {
            if owner.sidecar_id.is_some() || owner.transaction.id() != job.transaction_id {
                return Err(io::Error::other(
                    "invalidated primary cursor owner is not embedded in the primary",
                )
                .into());
            }
            if !matches!(
                owner.transaction.planes().cursor(),
                CursorPlaneAssignment::Atomic { state: None, .. } | CursorPlaneAssignment::Disabled
            ) {
                return Err(io::Error::other(
                    "embedded cursor owner does not describe a disabled assignment",
                )
                .into());
            }
            if job.cursor_pin.is_some() {
                return Err(io::Error::other(
                    "disabled cursor update unexpectedly owns a framebuffer pin",
                )
                .into());
            }
            Ok(Some(FrozenCursorPlaneOwner {
                revision: owner.revision,
                client_source_key: None,
                capability_key: owner.capability_key,
                pin: None,
            }))
        }
        (KmsCursorUpdate::Disable, None) => Err(io::Error::other(
            "frozen cursor Disable update is missing its embedded owner",
        )
        .into()),
    }
}

impl NativeRuntime {
    pub(super) fn replan_invalidated_worker_job(
        &mut self,
        mut job: KmsCommitJob,
    ) -> NativeResult<()> {
        let sidecar_transaction_id = job
            .owners
            .cursor()
            .filter(|owner| owner.sidecar_id.is_some())
            .map(|owner| owner.transaction.id());
        let compatibility_primary = matches!(job.kind, AtomicCommitKind::CompositedPrimary { .. })
            && self
                .output_transactions
                .transaction(job.transaction_id)
                .is_some_and(|transaction| {
                    matches!(
                        transaction.descriptor().planes().primary(),
                        PrimaryPlaneAssignment::CompatibilityFramebuffer { .. }
                    )
                });
        let explicit_primary_fence =
            if matches!(job.kind, AtomicCommitKind::CompositedPrimary { .. })
                && !compatibility_primary
            {
                match &mut job.primary {
                    KmsPrimaryUpdate::Framebuffer { in_fence, .. } => in_fence.take(),
                    KmsPrimaryUpdate::Unchanged => None,
                }
            } else {
                None
            };

        if let AtomicCommitKind::PlaneDelta { cursor_epoch, .. } = job.kind {
            let cursor = self
                .atomic_cursor
                .as_mut()
                .ok_or_else(|| io::Error::other("invalidated cursor job has no cursor"))?;
            cursor.cancel_worker_submission(job.transaction_id, job.token, cursor_epoch)?;
            self.cursor_output_arbitration.clear_pending();
        } else if !self
            .frame_pacing
            .cancel_worker_submission(job.pacing_frame_id, job.ready_submit)
        {
            return Err(io::Error::other("invalidated worker pacing identity mismatch").into());
        }

        if matches!(job.kind, AtomicCommitKind::CompositedPrimary { .. })
            && compatibility_primary
            && let Err(error) = self
                .frame_scheduler
                .cancel_worker_submission(job.token.get(), job.transaction_id.get())
        {
            if let Some(worker) = self.kms_commit_worker.as_ref() {
                worker.record_scheduler_cancel_mismatch();
            }
            return Err(io::Error::other(error).into());
        }
        let mut cursor_owner = if matches!(job.kind, AtomicCommitKind::CompositedPrimary { .. })
            && !compatibility_primary
        {
            take_embedded_cursor_owner(&mut job)?
        } else {
            None
        };
        if matches!(job.kind, AtomicCommitKind::CompositedPrimary { .. }) {
            if compatibility_primary {
                self.scanout
                    .return_worker_submission_for_replan(job.token, None, &mut cursor_owner)
                    .map_err(io::Error::other)?;
            } else {
                let result = self.scanout.return_worker_submission_for_replan(
                    job.token,
                    explicit_primary_fence,
                    &mut cursor_owner,
                );
                if let Err(error) = result {
                    if let Some(owner) = cursor_owner.take() {
                        debug_assert!(job.cursor_pin.is_none());
                        job.cursor_pin = owner.pin;
                    }
                    return Err(io::Error::other(error).into());
                }
            }
        }

        if self
            .atomic_commit_arbiter
            .reject_worker_queued(job.token)
            .is_none()
        {
            return Err(io::Error::other("invalidated worker Atomic identity mismatch").into());
        }
        self.output_transactions
            .rollback_queued(job.transaction_id)
            .map_err(io::Error::other)?;
        if let Some(sidecar_transaction_id) = sidecar_transaction_id
            && let Some(sidecar) = self.output_transactions.transaction(sidecar_transaction_id)
            && matches!(
                sidecar.state(),
                OutputTransactionState::Built
                    | OutputTransactionState::Ready { .. }
                    | OutputTransactionState::Queued { .. }
            )
        {
            self.output_transactions
                .mark_superseded(
                    sidecar_transaction_id,
                    None,
                    OutputTransactionSupersedeReason::NewerTransaction,
                    MonotonicTimestampNs::new(monotonic_now_ns()?),
                )
                .map_err(io::Error::other)?;
        }
        self.perf.log("native.kms_commit_worker", || {
            vec![
                NativePerfField::str("event", "validation_base_invalidated_replanned"),
                NativePerfField::u64("transaction_id", job.transaction_id.get()),
                NativePerfField::u64("token", job.token.get()),
            ]
        });
        Ok(())
    }

    pub(super) fn fail_queued_worker_job(
        &mut self,
        job: KmsCommitJob,
        error: AtomicKmsError,
        rejection_kind: WorkerRejectionKind,
    ) -> NativeResult<()> {
        self.scene_history.discard_submission(job.token.get());
        if matches!(job.kind, AtomicCommitKind::DirectPrimary { .. }) {
            return self.reject_direct_worker_job(job, error, rejection_kind);
        }
        let sidecar_owner = job
            .owners
            .cursor()
            .filter(|owner| owner.sidecar_id.is_some())
            .cloned();
        if let Some(worker) = self.kms_commit_worker.as_ref() {
            worker.record_worker_pacing_pre_submit_rejection();
        }
        if !self
            .frame_pacing
            .cancel_worker_submission(job.pacing_frame_id, job.ready_submit)
        {
            return Err(io::Error::other("worker rejection pacing identity mismatch").into());
        }
        let compatibility_primary = matches!(job.kind, AtomicCommitKind::CompositedPrimary { .. })
            && self
                .output_transactions
                .transaction(job.transaction_id)
                .is_some_and(|transaction| {
                    matches!(
                        transaction.descriptor().planes().primary(),
                        PrimaryPlaneAssignment::CompatibilityFramebuffer { .. }
                    )
                });
        let cursor_epoch = sidecar_owner
            .as_ref()
            .and_then(|owner| match owner.transaction.planes().cursor() {
                CursorPlaneAssignment::Atomic { desired_epoch, .. } => Some(*desired_epoch),
                CursorPlaneAssignment::Unchanged | CursorPlaneAssignment::Disabled => None,
            })
            .or(match job.kind {
                AtomicCommitKind::PlaneDelta { cursor_epoch, .. } => Some(cursor_epoch),
                AtomicCommitKind::CompositedPrimary { .. }
                | AtomicCommitKind::DirectPrimary { .. } => None,
            });
        let cursor_capability_key = job.owners.cursor().and_then(|owner| owner.capability_key);
        if let Some(cursor_epoch) = cursor_epoch {
            if sidecar_owner.is_none() {
                let cursor = self
                    .atomic_cursor
                    .as_mut()
                    .ok_or_else(|| io::Error::other("cursor worker rejection has no cursor"))?;
                cursor.cancel_worker_submission(job.transaction_id, job.token, cursor_epoch)?;
            }
            if error.kind == oblivion_one::native::kms::AtomicKmsErrorKind::Busy {
                let now_ns = monotonic_now_ns()?;
                self.cursor_output_arbitration.defer_after_busy(
                    now_ns,
                    self.frame_scheduler.next_refresh_deadline_ns(now_ns),
                );
                if let Some(worker) = self.kms_commit_worker.as_ref() {
                    worker.record_cursor_worker_rejection_retryable();
                }
            } else {
                let cursor = self
                    .atomic_cursor
                    .as_mut()
                    .ok_or_else(|| io::Error::other("cursor worker rejection has no cursor"))?;
                cursor.note_submit_failure_for(cursor_capability_key);
                cursor.note_software_fallback();
                cursor.note_composed_software_fallback();
                cursor.set_visible(false);
                self.cursor_render_mode = if self.server.client_cursor_render_state().is_some() {
                    NativeCursorRenderMode::SoftwareClient
                } else {
                    NativeCursorRenderMode::Software
                };
                self.last_client_cursor_damage = None;
                self.queued_redraw_requested = true;
                if let Some(worker) = self.kms_commit_worker.as_ref() {
                    worker.record_cursor_worker_rejection_fallback();
                }
            }
        } else if compatibility_primary {
            if let Err(error) = self
                .frame_scheduler
                .cancel_worker_submission(job.token.get(), job.transaction_id.get())
            {
                if let Some(worker) = self.kms_commit_worker.as_ref() {
                    worker.record_scheduler_cancel_mismatch();
                }
                return Err(io::Error::other(error).into());
            }
            if let Some(worker) = self.kms_commit_worker.as_ref() {
                worker.record_scheduler_queued_cancellation();
            }
        }
        self.atomic_commit_arbiter.reject_worker_queued(job.token);
        if matches!(job.kind, AtomicCommitKind::CompositedPrimary { .. }) {
            if compatibility_primary {
                self.scanout
                    .fail_worker_compatibility_submission(job.token)?;
            } else {
                self.scanout.fail_worker_submission(job.token)?;
            }
        }
        let direct_job = matches!(job.kind, AtomicCommitKind::DirectPrimary { .. });
        settle_failed_output_transaction(
            &mut self.output_transactions,
            job.transaction_id,
            OutputTransactionFailureStage::KmsSubmit,
            MonotonicTimestampNs::new(monotonic_now_ns()?),
            |obligations| {
                if direct_job {
                    let batch_id = obligations.frame_batch_id().ok_or_else(|| {
                        io::Error::other("rejected direct transaction has no frame batch")
                    })?;
                    self.server
                        .restore_frame_batch_after_render_failure(batch_id);
                } else if let Some(batch_id) = obligations.frame_batch_id() {
                    self.server
                        .discard_frame_batch(batch_id, FrameBatchDiscardReason::FatalOutputFailure);
                }
                Ok(())
            },
        )?;
        if let Some(sidecar) = sidecar_owner {
            settle_failed_output_transaction(
                &mut self.output_transactions,
                sidecar.transaction.id(),
                OutputTransactionFailureStage::KmsSubmit,
                MonotonicTimestampNs::new(monotonic_now_ns()?),
                |obligations| {
                    debug_assert!(obligations.frame_batch_id().is_none());
                    debug_assert!(obligations.direct_surface_id().is_none());
                    Ok(())
                },
            )?;
        }
        self.perf.log("native.kms_commit_worker", || {
            vec![
                NativePerfField::str("event", "submit_rejected"),
                NativePerfField::str("error", error.to_string()),
            ]
        });
        Ok(())
    }

    pub(crate) fn drop_queued_worker_job(&mut self, job: KmsCommitJob) -> NativeResult<()> {
        self.drop_queued_worker_job_with_reason(job, OutputTransactionDropReason::SessionSuspended)
    }

    pub(crate) fn drop_queued_worker_job_with_reason(
        &mut self,
        job: KmsCommitJob,
        drop_reason: OutputTransactionDropReason,
    ) -> NativeResult<()> {
        self.scene_history.discard_submission(job.token.get());
        let sidecar_transaction_id = job
            .owners
            .cursor()
            .filter(|owner| owner.sidecar_id.is_some())
            .map(|owner| owner.transaction.id());
        if matches!(job.kind, AtomicCommitKind::DirectPrimary { .. })
            && let Some(duration_ns) = job.test_only_duration_ns
        {
            self.scanout.note_direct_test_only(duration_ns, false);
        }
        let compatibility_primary = matches!(job.kind, AtomicCommitKind::CompositedPrimary { .. })
            && self
                .output_transactions
                .transaction(job.transaction_id)
                .is_some_and(|transaction| {
                    matches!(
                        transaction.descriptor().planes().primary(),
                        PrimaryPlaneAssignment::CompatibilityFramebuffer { .. }
                    )
                });
        if let AtomicCommitKind::PlaneDelta { cursor_epoch, .. } = job.kind {
            let cursor = self
                .atomic_cursor
                .as_mut()
                .ok_or_else(|| io::Error::other("queued cursor job has no cursor"))?;
            cursor.cancel_worker_submission(job.transaction_id, job.token, cursor_epoch)?;
            self.cursor_output_arbitration.clear_pending();
        } else {
            if !self
                .frame_pacing
                .cancel_worker_submission(job.pacing_frame_id, job.ready_submit)
            {
                return Err(io::Error::other("worker shutdown pacing identity mismatch").into());
            }
            if compatibility_primary {
                let scheduler_cancel =
                    if drop_reason == OutputTransactionDropReason::SafeAbandonment {
                        self.frame_scheduler
                            .abandon_worker_submission(job.token.get(), job.transaction_id.get())
                    } else {
                        self.frame_scheduler
                            .cancel_worker_submission(job.token.get(), job.transaction_id.get())
                    };
                if let Err(error) = scheduler_cancel {
                    if let Some(worker) = self.kms_commit_worker.as_ref() {
                        worker.record_scheduler_cancel_mismatch();
                    }
                    return Err(io::Error::other(error).into());
                }
                if let Some(worker) = self.kms_commit_worker.as_ref() {
                    worker.record_scheduler_queued_cancellation();
                }
            }
        }
        self.atomic_commit_arbiter.reject_worker_queued(job.token);
        if matches!(job.kind, AtomicCommitKind::CompositedPrimary { .. }) {
            if compatibility_primary {
                self.scanout
                    .suspend_abandon_worker_compatibility(job.token)
                    .map_err(io::Error::other)?;
            } else {
                self.scanout
                    .suspend_abandon_worker_submission(job.token)
                    .map_err(io::Error::other)?;
            }
        }
        let direct_obligations = if matches!(job.kind, AtomicCommitKind::DirectPrimary { .. }) {
            Some(
                self.output_transactions
                    .transaction(job.transaction_id)
                    .ok_or_else(|| io::Error::other("dropped direct transaction is missing"))?
                    .descriptor()
                    .obligations(),
            )
        } else {
            None
        };
        let direct_callback_owner_leaks = direct_obligations.map(|obligations| {
            direct_terminal_callback_owner_leaks(
                &mut self.server,
                job.transaction_id,
                obligations,
                DirectTerminalCallbackDisposition::Abandoned,
            )
        });
        settle_dropped_output_transaction(
            &mut self.output_transactions,
            job.transaction_id,
            drop_reason,
            MonotonicTimestampNs::new(monotonic_now_ns()?),
            |obligations| {
                if let Some(batch_id) = obligations.frame_batch_id() {
                    self.server.complete_frame_batch_after_safe_abandonment(
                        batch_id,
                        FrameBatchDiscardReason::SuspendAbandonment,
                    );
                }
                Ok(())
            },
        )?;
        if let Some(sidecar_transaction_id) = sidecar_transaction_id {
            settle_dropped_output_transaction(
                &mut self.output_transactions,
                sidecar_transaction_id,
                drop_reason,
                MonotonicTimestampNs::new(monotonic_now_ns()?),
                |obligations| {
                    debug_assert!(obligations.frame_batch_id().is_none());
                    debug_assert!(obligations.direct_surface_id().is_none());
                    Ok(())
                },
            )?;
        }
        if let Some(callback_owner_leaks) = direct_callback_owner_leaks {
            self.scanout
                .note_direct_callback_owner_leaks(callback_owner_leaks);
        }
        if drop_reason == OutputTransactionDropReason::SafeAbandonment
            && let Some(worker) = self.kms_commit_worker.as_ref()
        {
            worker.record_shutdown_queued_job_settled();
        }
        Ok(())
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;
    use crate::native_output::CursorGeometryClass;
    use crate::native_output::kms_worker::{
        KmsBundleOwners, KmsCommitTestPolicy, KmsCursorOwner, KmsCursorUpdate,
        KmsPrimaryCursorPresentation, KmsPrimaryOwner, KmsTestOnlyPolicy, KmsValidationBase,
    };
    use crate::native_output::output::CursorFramebufferPin;
    use crate::native_output::presentation::plane::{CursorRevision, PresentedCursorDelivery};
    use crate::native_output::presentation::plane_policy::CursorCapabilityKey;
    use oblivion_one::compositor::CompositorFrameBatchId;
    use oblivion_one::native::kms::{AtomicCursorVisualState, FramebufferId};
    use oblivion_one::native::presentation_deadline::{
        MonotonicTimestampNs, PresentationTarget, PresentationTargetReason,
    };
    use oblivion_one::native::scheduler::NativeOutputPacingMode;
    use std::num::NonZeroU64;
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::sync::Arc;
    use std::time::Duration;

    fn key() -> CursorCapabilityKey {
        CursorCapabilityKey {
            output_generation: 1,
            crtc_id: 7,
            plane_id: 9,
            mode_width: 1920,
            mode_height: 1080,
            output_transform: 0,
            output_scale_milli: 1000,
            format: 875713112,
            modifier: 0,
            cursor_width: 64,
            cursor_height: 64,
            hotspot_property_available: true,
            geometry_class: CursorGeometryClass::FullyVisible,
            source_x: 0,
            source_y: 0,
            source_width: 64,
            source_height: 64,
            destination_x: 0,
            destination_y: 0,
            destination_width: 64,
            destination_height: 64,
        }
    }

    fn embedded_job(
        token: u64,
        assignment: CursorPlaneAssignment,
        pin: Option<CursorFramebufferPin>,
    ) -> KmsCommitJob {
        let transaction_id = crate::native_output::OutputTransactionId::new(
            NonZeroU64::new(token).expect("transaction ID"),
        );
        let target = PresentationTarget {
            sequence: token,
            presentation_time: MonotonicTimestampNs::new(10),
            submit_not_before: MonotonicTimestampNs::new(10),
            render_start_deadline: MonotonicTimestampNs::new(1),
            refresh_interval: Duration::from_millis(16),
            reason: PresentationTargetReason::ReactiveDouble,
            clock_generation: 1,
            estimated: true,
            predicted_unreachable: false,
            physical_claim: oblivion_one::native::presentation_deadline::PrimaryRefreshClaim {
                sequence: token,
                presentation_time: MonotonicTimestampNs::new(10),
                clock_generation: 1,
            },
            selection_evidence: Default::default(),
        };
        let transaction = Arc::new(
            crate::native_output::OutputTransaction::composited(
                transaction_id,
                1,
                MonotonicTimestampNs::new(1),
                target,
                NativeOutputPacingMode::ReactiveDouble,
                token,
                1,
                1,
                crate::native_output::OutputSlotId::new(1).unwrap(),
                42,
                Some(assignment.clone()),
                CompositorFrameBatchId::new(NonZeroU64::new(token).unwrap()),
            )
            .unwrap(),
        );
        let state = AtomicCursorVisualState {
            framebuffer_id: Some(pin.as_ref().map_or(101, |pin| pin.framebuffer_id().get())),
            visible: true,
            ..AtomicCursorVisualState::hidden(64, 64)
        };
        let revision = CursorRevision::initial().advance_image();
        let cursor_owner =
            (!matches!(assignment, CursorPlaneAssignment::Unchanged)).then(|| KmsCursorOwner {
                transaction: Arc::clone(&transaction),
                sidecar_id: None,
                revision,
                capability_key: Some(key()),
            });
        let owners = KmsBundleOwners::new(
            Some(KmsPrimaryOwner {
                transaction: Arc::clone(&transaction),
            }),
            cursor_owner,
        )
        .unwrap();
        let (cursor, delivery) = match assignment {
            CursorPlaneAssignment::Atomic { .. } => (
                KmsCursorUpdate::Set(state),
                PresentedCursorDelivery::Hardware,
            ),
            CursorPlaneAssignment::Disabled => {
                (KmsCursorUpdate::Disable, PresentedCursorDelivery::Hidden)
            }
            CursorPlaneAssignment::Unchanged => {
                (KmsCursorUpdate::Unchanged, PresentedCursorDelivery::Hidden)
            }
        };
        KmsCommitJob {
            bundle_id: crate::native_output::presentation::plane::KmsCommitBundleId::new(
                NonZeroU64::new(token).unwrap(),
            ),
            owners,
            transaction_id,
            token: oblivion_one::native::kms::PageFlipToken::new(token).unwrap(),
            output_generation: 1,
            crtc_id: 7,
            kind: AtomicCommitKind::CompositedPrimary {
                transaction_id,
                frame_id: token,
                framebuffer_id: 42,
            },
            target,
            submit_window:
                crate::native_output::presentation::kms_timing::KmsSubmitWindow::try_new(
                    target.presentation_time.get(),
                    target.submit_not_before().get(),
                    0,
                    0,
                )
                .unwrap(),
            validation_base: KmsValidationBase::Presented {
                snapshot: crate::native_output::presentation::plane::PresentedPlaneSnapshot::legacy(
                    None,
                ),
                output_generation: 1,
                crtc_id: 7,
            },
            queued_at: MonotonicTimestampNs::new(1),
            primary: KmsPrimaryUpdate::Framebuffer {
                framebuffer: FramebufferId::new(42).unwrap(),
                in_fence: Some(test_input_fence()),
                request_out_fence: true,
            },
            cursor,
            cursor_delivery: delivery,
            primary_cursor_presentation: KmsPrimaryCursorPresentation::Preserve,
            cursor_pin: pin,
            direct_primary_lease: None,
            test_only_duration_ns: None,
            pacing_frame_id: None,
            test_policy: KmsCommitTestPolicy::from_primary(KmsTestOnlyPolicy::Required),
            ready_submit: true,
        }
    }

    fn test_input_fence() -> OwnedFd {
        let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
        assert!(fd >= 0);
        unsafe { OwnedFd::from_raw_fd(fd) }
    }

    #[test]
    fn invalidated_embedded_set_extracts_exact_owner_and_pin() {
        let lease = Arc::new(());
        let mut job = embedded_job(
            8801,
            CursorPlaneAssignment::Atomic {
                desired_epoch: 1,
                state: Some(AtomicCursorVisualState {
                    framebuffer_id: Some(101),
                    visible: true,
                    ..AtomicCursorVisualState::hidden(64, 64)
                }),
            },
            Some(CursorFramebufferPin::for_test(101, Arc::clone(&lease))),
        );
        let transaction = Arc::clone(&job.owners.primary().unwrap().transaction);
        assert!(job.validate_against(&transaction).is_ok());
        let owner = take_embedded_cursor_owner(&mut job).unwrap().unwrap();

        assert_eq!(owner.revision, CursorRevision::initial().advance_image());
        assert_eq!(owner.capability_key, Some(key()));
        assert_eq!(owner.pin.as_ref().unwrap().framebuffer_id().get(), 101);
        assert!(job.cursor_pin.is_none());
        assert_eq!(Arc::strong_count(&lease), 2);
        job.cursor_pin = owner.pin;
        assert!(job.validate_against(&transaction).is_ok());
        drop(job);
        assert_eq!(Arc::strong_count(&lease), 1);
    }

    #[test]
    fn invalidated_embedded_disable_extracts_metadata_without_pin() {
        let mut job = embedded_job(8802, CursorPlaneAssignment::Disabled, None);
        let owner = take_embedded_cursor_owner(&mut job).unwrap().unwrap();
        assert_eq!(owner.revision, CursorRevision::initial().advance_image());
        assert_eq!(owner.capability_key, Some(key()));
        assert!(owner.pin.is_none());
    }

    #[test]
    fn invalidated_unchanged_cursor_extracts_no_owner() {
        let mut job = embedded_job(8803, CursorPlaneAssignment::Unchanged, None);
        assert!(take_embedded_cursor_owner(&mut job).unwrap().is_none());
    }

    #[test]
    fn invalidated_sidecar_owner_is_rejected_without_consuming_pin() {
        let lease = Arc::new(());
        let mut job = embedded_job(
            8804,
            CursorPlaneAssignment::Atomic {
                desired_epoch: 1,
                state: Some(AtomicCursorVisualState {
                    framebuffer_id: Some(104),
                    visible: true,
                    ..AtomicCursorVisualState::hidden(64, 64)
                }),
            },
            Some(CursorFramebufferPin::for_test(104, Arc::clone(&lease))),
        );
        let mut owner = job.owners.cursor().cloned().unwrap();
        owner.sidecar_id = Some(
            crate::native_output::presentation::plane::CursorSidecarId::new(
                NonZeroU64::new(1).unwrap(),
            ),
        );
        job.owners = KmsBundleOwners::new(job.owners.primary().cloned(), Some(owner)).unwrap();

        assert!(take_embedded_cursor_owner(&mut job).is_err());
        assert!(job.cursor_pin.is_some());
        assert_eq!(Arc::strong_count(&lease), 2);
    }
}
