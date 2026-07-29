use super::cycle::direct_fallback::DirectFallbackReason;
use super::presentation_transactions::{
    DirectTerminalCallbackDisposition, direct_terminal_callback_owner_leaks,
};
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerRejectionKind {
    TestOnly,
    RealSubmit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectRejectionPolicy {
    pub invalidate_validation_key: bool,
    pub request_composited_redraw: bool,
    pub demote_hardware_cursor: bool,
}

pub const fn direct_rejection_policy(rejection_kind: WorkerRejectionKind) -> DirectRejectionPolicy {
    DirectRejectionPolicy {
        invalidate_validation_key: matches!(rejection_kind, WorkerRejectionKind::RealSubmit),
        request_composited_redraw: true,
        demote_hardware_cursor: false,
    }
}

impl NativeRuntime {
    pub(super) fn reject_direct_worker_job(
        &mut self,
        job: KmsCommitJob,
        error: AtomicKmsError,
        rejection_kind: WorkerRejectionKind,
    ) -> NativeResult<()> {
        if let Some(worker) = self.kms_commit_worker.as_ref() {
            worker.record_worker_pacing_pre_submit_rejection();
        }
        let rejection_policy = direct_rejection_policy(rejection_kind);
        let combined_cursor = matches!(&job.cursor, KmsCursorUpdate::Set(_));
        let validation_key = job
            .direct_primary_lease
            .as_ref()
            .map(|lease| lease.validation_key());
        let obligations = self
            .output_transactions
            .transaction(job.transaction_id)
            .ok_or_else(|| io::Error::other("rejected direct transaction is missing"))?
            .descriptor()
            .obligations();
        if !self
            .frame_pacing
            .cancel_worker_submission(job.pacing_frame_id, job.ready_submit)
        {
            self.quarantined_worker_jobs.push(job);
            return Err(io::Error::other("direct rejection pacing identity mismatch").into());
        }
        if let Err(error) = self
            .frame_scheduler
            .cancel_worker_submission(job.token.get(), job.transaction_id.get())
        {
            if let Some(worker) = self.kms_commit_worker.as_ref() {
                worker.record_scheduler_cancel_mismatch();
            }
            self.quarantined_worker_jobs.push(job);
            return Err(io::Error::other(error).into());
        }
        if let Some(worker) = self.kms_commit_worker.as_ref() {
            worker.record_scheduler_queued_cancellation();
        }
        if self
            .atomic_commit_arbiter
            .reject_worker_queued(job.token)
            .is_none()
        {
            self.quarantined_worker_jobs.push(job);
            return Err(io::Error::other("direct rejection Atomic identity mismatch").into());
        }
        if rejection_policy.invalidate_validation_key
            && let Some(validation_key) = validation_key
        {
            self.scanout.invalidate_direct_validation(validation_key);
        }
        let direct_job = job;
        let callback_owner_leaks = direct_terminal_callback_owner_leaks(
            &mut self.server,
            direct_job.transaction_id,
            obligations,
            DirectTerminalCallbackDisposition::Retryable,
        );
        let settlement = settle_failed_output_transaction(
            &mut self.output_transactions,
            direct_job.transaction_id,
            OutputTransactionFailureStage::KmsSubmit,
            MonotonicTimestampNs::new(monotonic_now_ns()?),
            |obligations| {
                let batch_id = obligations.frame_batch_id().ok_or_else(|| {
                    io::Error::other("rejected direct transaction has no frame batch")
                })?;
                self.server
                    .restore_frame_batch_after_render_failure(batch_id);
                Ok(())
            },
        );
        if let Err(error) = settlement {
            self.quarantined_worker_jobs.push(direct_job);
            return Err(error);
        }
        self.scanout
            .note_direct_callback_owner_leaks(callback_owner_leaks);
        debug_assert!(!self.scanout.direct_scanout_pending());
        self.scanout.note_direct_rejection(
            rejection_kind == WorkerRejectionKind::TestOnly,
            combined_cursor,
        );
        self.scanout.note_direct_blocker(match rejection_kind {
            WorkerRejectionKind::TestOnly => "test_only_rejected",
            WorkerRejectionKind::RealSubmit => "real_submit_rejected",
        });
        self.begin_direct_fallback(
            direct_job.transaction_id,
            match rejection_kind {
                WorkerRejectionKind::TestOnly => DirectFallbackReason::TestOnlyRejected,
                WorkerRejectionKind::RealSubmit => DirectFallbackReason::RealSubmitRejected,
            },
        );
        if rejection_policy.request_composited_redraw {
            self.scanout.note_direct_fallback_redraw();
            self.queued_redraw_requested = true;
        }
        self.perf.log("native.kms_commit_worker", || {
            vec![
                NativePerfField::str("event", "direct_rejected"),
                NativePerfField::str(
                    "kind",
                    match rejection_kind {
                        WorkerRejectionKind::TestOnly => "test_only",
                        WorkerRejectionKind::RealSubmit => "real_submit",
                    },
                ),
                NativePerfField::str("error", error.to_string()),
            ]
        });
        Ok(())
    }
}
