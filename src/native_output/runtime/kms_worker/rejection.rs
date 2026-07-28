use super::super::*;
use crate::native_output::kms_worker::KmsCommitJob;
use oblivion_one::native::kms::AtomicKmsError;

use super::super::presentation_transactions::{
    DirectTerminalCallbackDisposition, direct_terminal_callback_owner_leaks,
    settle_dropped_output_transaction, settle_failed_output_transaction,
};
use super::direct_rejection::WorkerRejectionKind;

impl NativeRuntime {
    pub(super) fn fail_queued_worker_job(
        &mut self,
        job: KmsCommitJob,
        error: AtomicKmsError,
        rejection_kind: WorkerRejectionKind,
    ) -> NativeResult<()> {
        if matches!(job.kind, AtomicCommitKind::DirectPrimary { .. }) {
            return self.reject_direct_worker_job(job, error, rejection_kind);
        }
        if let Some(worker) = self.kms_commit_worker.as_ref() {
            worker.record_worker_pacing_pre_submit_rejection();
        }
        if !self
            .frame_pacing
            .cancel_worker_submission(job.pacing_frame_id, job.ready_submit)
        {
            return Err(io::Error::other("worker rejection pacing identity mismatch").into());
        }
        let cursor_epoch = match job.kind {
            AtomicCommitKind::CursorOnly { cursor_epoch, .. } => Some(cursor_epoch),
            AtomicCommitKind::CompositedPrimary { .. } | AtomicCommitKind::DirectPrimary { .. } => {
                None
            }
        };
        if let Some(cursor_epoch) = cursor_epoch {
            let cursor = self
                .atomic_cursor
                .as_mut()
                .ok_or_else(|| io::Error::other("cursor worker rejection has no cursor"))?;
            cursor.cancel_worker_submission(job.transaction_id, job.token, cursor_epoch)?;
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
                cursor.note_submit_failure();
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
        } else {
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
            let compatibility_primary = self
                .output_transactions
                .transaction(job.transaction_id)
                .is_some_and(|transaction| {
                    matches!(
                        transaction.descriptor().planes().primary(),
                        PrimaryPlaneAssignment::CompatibilityFramebuffer { .. }
                    )
                });
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
        if matches!(job.kind, AtomicCommitKind::DirectPrimary { .. })
            && let Some(duration_ns) = job.test_only_duration_ns
        {
            self.scanout.note_direct_test_only(duration_ns, false);
        }
        if let AtomicCommitKind::CursorOnly { cursor_epoch, .. } = job.kind {
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
            let scheduler_cancel = if drop_reason == OutputTransactionDropReason::SafeAbandonment {
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
        self.atomic_commit_arbiter.reject_worker_queued(job.token);
        if matches!(job.kind, AtomicCommitKind::CompositedPrimary { .. }) {
            let compatibility_primary = self
                .output_transactions
                .transaction(job.transaction_id)
                .is_some_and(|transaction| {
                    matches!(
                        transaction.descriptor().planes().primary(),
                        PrimaryPlaneAssignment::CompatibilityFramebuffer { .. }
                    )
                });
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
                &self.server,
                job.transaction_id,
                obligations,
                DirectTerminalCallbackDisposition::Abandoned,
                0,
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
