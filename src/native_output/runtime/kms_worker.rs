//! Main-thread adapter for the submit-only Atomic KMS worker.
//!
//! This module is intentionally small: worker results are transported here,
//! while ledger settlement, DRM event reading, and physical scanout promotion
//! remain owned by the compositor thread.

use super::super::kms_worker::{
    KmsCommitJob, KmsCommitWorkerHandle, KmsCursorUpdate, KmsPrimaryUpdate, KmsTestOnlyPolicy,
    KmsWorkerAdmissionError, KmsWorkerEvent,
};
use super::presentation_transactions::{
    build_compatibility_transaction, settle_dropped_output_transaction,
    settle_failed_output_transaction,
};
use super::*;
use crate::native_output::scanout::AtomicEglGbmScanout;
use oblivion_one::native::kms::{AtomicKmsError, FramebufferId, PageFlipToken};

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
    Unavailable(KmsWorkerAdmissionError),
}

#[allow(clippy::too_many_arguments)]
pub(super) fn queue_cursor_only(
    worker: &KmsCommitWorkerHandle,
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
    let permit = match worker.try_reserve_admission_slot() {
        Ok(permit) => permit,
        Err(error) => return Ok(WorkerQueueOutcome::Unavailable(error)),
    };
    let transaction_id = output_transactions
        .allocate_id()
        .map_err(io::Error::other)?;
    let transaction = OutputTransaction::cursor_only(
        transaction_id,
        output_generation,
        MonotonicTimestampNs::new(monotonic_now_ns()?),
        target,
        pacing_mode,
        cursor_epoch,
        desired.as_ref().and_then(|state| state.framebuffer_id),
        desired.as_ref().is_some_and(|state| state.visible),
        OutputReleasePlan::Pageflip,
    )
    .map_err(io::Error::other)?;
    output_transactions
        .insert(transaction)
        .map_err(io::Error::other)?;
    let token = PageFlipToken::new(allocate_native_page_flip_token())
        .expect("allocated native pageflip token is nonzero");
    let queued_at_ns = monotonic_now_ns()?;
    let kind = AtomicCommitKind::CursorOnly {
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
    if let Err(error) = atomic_commit_arbiter.reserve_worker_queued(
        token,
        output_generation,
        crtc_id,
        kind,
        queued_at_ns,
    ) {
        settle_failed_output_transaction(
            output_transactions,
            transaction_id,
            OutputTransactionFailureStage::BackendOwnershipTransfer,
            MonotonicTimestampNs::new(queued_at_ns),
            |_| Ok(()),
        )?;
        return Err(io::Error::other(error).into());
    }
    let job = KmsCommitJob {
        transaction_id,
        token,
        output_generation,
        crtc_id,
        kind,
        target,
        queued_at: MonotonicTimestampNs::new(queued_at_ns),
        primary: KmsPrimaryUpdate::Unchanged,
        cursor: desired.map_or(KmsCursorUpdate::Disable, KmsCursorUpdate::Set),
        test_only: KmsTestOnlyPolicy::Required,
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
    if let Err(error) = permit.enqueue(job) {
        drop(error.job);
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
    Ok(WorkerQueueOutcome::CursorQueued {
        transaction_id,
        token,
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn queue_explicit_composited_frame(
    worker: &KmsCommitWorkerHandle,
    explicit: &mut AtomicEglGbmScanout,
    server: &mut OwnCompositorServer,
    output_transactions: &mut OutputTransactionLedger,
    atomic_commit_arbiter: &mut AtomicCommitArbiter,
    presentation_trace: &mut PresentationTransactionTraceRing,
    transaction_id: OutputTransactionId,
    output_generation: u64,
    crtc_id: u32,
    cursor: Option<&AtomicCursorVisualState>,
) -> NativeResult<WorkerQueueOutcome> {
    let slot = explicit
        .swapchain()?
        .ready_slot()
        .ok_or_else(|| io::Error::other("worker composited queue has no ready slot"))?;
    let framebuffer = explicit.framebuffer(slot)?;
    let transaction = output_transactions
        .transaction(transaction_id)
        .ok_or_else(|| io::Error::other("worker composited queue transaction is missing"))?;
    let frame_id = match transaction.descriptor().content() {
        OutputTransactionContent::Composited { frame_id, .. } => frame_id,
        _ => {
            return Err(io::Error::other(
                "worker composited queue received non-composited transaction",
            )
            .into());
        }
    };
    let target = transaction.descriptor().target();
    let token = PageFlipToken::new(allocate_native_page_flip_token())
        .expect("allocated native pageflip token is nonzero");
    let kind = AtomicCommitKind::CompositedPrimary {
        transaction_id,
        frame_id,
        framebuffer_id: framebuffer.get(),
    };
    let permit = match worker.try_reserve_admission(kind) {
        Ok(permit) => permit,
        Err(error) => return Ok(WorkerQueueOutcome::Unavailable(error)),
    };
    let queued_at_ns = monotonic_now_ns()?;
    let in_fence = explicit
        .swapchain_mut()?
        .take_ready_for_worker(token, MonotonicTimestampNs::new(queued_at_ns))?;
    if let Err(error) = output_transactions.mark_queued(
        transaction_id,
        output_generation,
        MonotonicTimestampNs::new(queued_at_ns),
    ) {
        explicit.fail_worker_submission(token)?;
        settle_failed_output_transaction(
            output_transactions,
            transaction_id,
            OutputTransactionFailureStage::BackendOwnershipTransfer,
            MonotonicTimestampNs::new(queued_at_ns),
            |obligations| {
                if let Some(batch_id) = obligations.frame_batch_id() {
                    server
                        .discard_frame_batch(batch_id, FrameBatchDiscardReason::FatalOutputFailure);
                }
                Ok(())
            },
        )?;
        return Err(io::Error::other(error).into());
    }
    if let Err(error) = atomic_commit_arbiter.reserve_worker_queued(
        token,
        output_generation,
        crtc_id,
        kind,
        queued_at_ns,
    ) {
        explicit.fail_worker_submission(token)?;
        settle_failed_output_transaction(
            output_transactions,
            transaction_id,
            OutputTransactionFailureStage::BackendOwnershipTransfer,
            MonotonicTimestampNs::new(monotonic_now_ns()?),
            |obligations| {
                if let Some(batch_id) = obligations.frame_batch_id() {
                    server
                        .discard_frame_batch(batch_id, FrameBatchDiscardReason::FatalOutputFailure);
                }
                Ok(())
            },
        )?;
        return Err(io::Error::other(error).into());
    }
    let job = KmsCommitJob {
        transaction_id,
        token,
        output_generation,
        crtc_id,
        kind,
        target,
        queued_at: MonotonicTimestampNs::new(queued_at_ns),
        primary: KmsPrimaryUpdate::Framebuffer {
            framebuffer,
            in_fence: Some(in_fence),
            request_out_fence: true,
        },
        cursor: cursor.map_or(KmsCursorUpdate::Unchanged, |state| {
            KmsCursorUpdate::Set(state.clone())
        }),
        test_only: KmsTestOnlyPolicy::Skip,
    };
    let queued_descriptor = output_transactions
        .transaction(transaction_id)
        .ok_or_else(|| io::Error::other("queued worker transaction disappeared"))?;
    if let Err(error) = job.validate_against(queued_descriptor.descriptor()) {
        let _ = atomic_commit_arbiter.reject_worker_queued(token);
        explicit.fail_worker_submission(token)?;
        settle_failed_output_transaction(
            output_transactions,
            transaction_id,
            OutputTransactionFailureStage::BackendOwnershipTransfer,
            MonotonicTimestampNs::new(monotonic_now_ns()?),
            |obligations| {
                if let Some(batch_id) = obligations.frame_batch_id() {
                    server
                        .discard_frame_batch(batch_id, FrameBatchDiscardReason::FatalOutputFailure);
                }
                Ok(())
            },
        )?;
        return Err(io::Error::other(format!("invalid Atomic worker payload: {error:?}")).into());
    }
    if let Err(error) = permit.enqueue(job) {
        drop(error.job);
        let _ = atomic_commit_arbiter.reject_worker_queued(token);
        explicit.fail_worker_submission(token)?;
        settle_failed_output_transaction(
            output_transactions,
            transaction_id,
            OutputTransactionFailureStage::BackendOwnershipTransfer,
            MonotonicTimestampNs::new(monotonic_now_ns()?),
            |obligations| {
                if let Some(batch_id) = obligations.frame_batch_id() {
                    server
                        .discard_frame_batch(batch_id, FrameBatchDiscardReason::FatalOutputFailure);
                }
                Ok(())
            },
        )?;
        return Err(
            io::Error::other(format!("Atomic worker enqueue failed: {:?}", error.reason)).into(),
        );
    }
    presentation_trace.push(PresentationTransactionEvent::WorkerQueued {
        transaction_id,
        timestamp_ns: queued_at_ns,
    });
    Ok(WorkerQueueOutcome::Queued {
        transaction_id,
        token,
        framebuffer_id: framebuffer,
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn queue_atomic_compatibility_frame(
    worker: &KmsCommitWorkerHandle,
    scanout: &mut NativeScanoutBackend,
    server: &mut OwnCompositorServer,
    output_transactions: &mut OutputTransactionLedger,
    atomic_commit_arbiter: &mut AtomicCommitArbiter,
    presentation_trace: &mut PresentationTransactionTraceRing,
    output_generation: u64,
    crtc_id: u32,
    target: PresentationTarget,
    pacing_mode: NativeOutputPacingMode,
    render_generation: u64,
    cursor: Option<&AtomicCursorVisualState>,
    cursor_epoch: u64,
) -> NativeResult<WorkerQueueOutcome> {
    if scanout.compatibility_framebuffer_id().is_none() {
        return Ok(WorkerQueueOutcome::Unavailable(
            KmsWorkerAdmissionError::QueueFull,
        ));
    }
    let permit = match worker.try_reserve_admission_slot() {
        Ok(permit) => permit,
        Err(error) => return Ok(WorkerQueueOutcome::Unavailable(error)),
    };
    let Some(transaction_id) = build_compatibility_transaction(
        output_transactions,
        server,
        scanout,
        output_generation,
        target,
        pacing_mode,
        render_generation,
        cursor,
        cursor_epoch,
    )?
    else {
        return Ok(WorkerQueueOutcome::Unavailable(
            KmsWorkerAdmissionError::QueueFull,
        ));
    };
    let queued_at_ns = monotonic_now_ns()?;
    let token = PageFlipToken::new(allocate_native_page_flip_token())
        .expect("allocated native pageflip token is nonzero");
    let framebuffer_id = match scanout.queue_worker_compatibility_submission(token) {
        Ok(framebuffer_id) => framebuffer_id,
        Err(error) => {
            settle_failed_output_transaction(
                output_transactions,
                transaction_id,
                OutputTransactionFailureStage::KmsSubmit,
                MonotonicTimestampNs::new(queued_at_ns),
                |obligations| {
                    if let Some(batch_id) = obligations.frame_batch_id() {
                        server.discard_frame_batch(
                            batch_id,
                            FrameBatchDiscardReason::FatalOutputFailure,
                        );
                    }
                    Ok(())
                },
            )?;
            return Err(error.into());
        }
    };
    let frame_id = match output_transactions
        .transaction(transaction_id)
        .ok_or_else(|| io::Error::other("compatibility worker transaction disappeared"))?
        .descriptor()
        .content()
    {
        OutputTransactionContent::Composited { frame_id, .. } => frame_id,
        _ => {
            return Err(
                io::Error::other("Atomic compatibility transaction is not composited").into(),
            );
        }
    };
    let kind = AtomicCommitKind::CompositedPrimary {
        transaction_id,
        frame_id,
        framebuffer_id,
    };
    if let Err(error) = output_transactions.mark_queued(
        transaction_id,
        output_generation,
        MonotonicTimestampNs::new(queued_at_ns),
    ) {
        scanout.fail_worker_compatibility_submission(token)?;
        settle_failed_output_transaction(
            output_transactions,
            transaction_id,
            OutputTransactionFailureStage::KmsSubmit,
            MonotonicTimestampNs::new(queued_at_ns),
            |_| Ok(()),
        )?;
        return Err(io::Error::other(error).into());
    }
    if let Err(error) = atomic_commit_arbiter.reserve_worker_queued(
        token,
        output_generation,
        crtc_id,
        kind,
        queued_at_ns,
    ) {
        scanout.fail_worker_compatibility_submission(token)?;
        settle_failed_output_transaction(
            output_transactions,
            transaction_id,
            OutputTransactionFailureStage::BackendOwnershipTransfer,
            MonotonicTimestampNs::new(queued_at_ns),
            |_| Ok(()),
        )?;
        return Err(io::Error::other(error).into());
    }
    let job = KmsCommitJob {
        transaction_id,
        token,
        output_generation,
        crtc_id,
        kind,
        target,
        queued_at: MonotonicTimestampNs::new(queued_at_ns),
        primary: KmsPrimaryUpdate::Framebuffer {
            framebuffer: FramebufferId::new(framebuffer_id)
                .ok_or_else(|| io::Error::other("compatibility framebuffer ID is zero"))?,
            in_fence: None,
            request_out_fence: true,
        },
        cursor: cursor.map_or(KmsCursorUpdate::Unchanged, |state| {
            KmsCursorUpdate::Set(state.clone())
        }),
        test_only: KmsTestOnlyPolicy::Skip,
    };
    let descriptor = output_transactions
        .transaction(transaction_id)
        .ok_or_else(|| io::Error::other("compatibility worker transaction disappeared"))?;
    if let Err(error) = job.validate_against(descriptor.descriptor()) {
        atomic_commit_arbiter.reject_worker_queued(token);
        scanout.fail_worker_compatibility_submission(token)?;
        settle_failed_output_transaction(
            output_transactions,
            transaction_id,
            OutputTransactionFailureStage::BackendOwnershipTransfer,
            MonotonicTimestampNs::new(queued_at_ns),
            |_| Ok(()),
        )?;
        return Err(
            io::Error::other(format!("invalid compatibility worker payload: {error:?}")).into(),
        );
    }
    if let Err(error) = permit.enqueue(job) {
        drop(error.job);
        atomic_commit_arbiter.reject_worker_queued(token);
        scanout.fail_worker_compatibility_submission(token)?;
        settle_failed_output_transaction(
            output_transactions,
            transaction_id,
            OutputTransactionFailureStage::BackendOwnershipTransfer,
            MonotonicTimestampNs::new(queued_at_ns),
            |obligations| {
                if let Some(batch_id) = obligations.frame_batch_id() {
                    server
                        .discard_frame_batch(batch_id, FrameBatchDiscardReason::FatalOutputFailure);
                }
                Ok(())
            },
        )?;
        return Err(io::Error::other(format!(
            "compatibility worker enqueue failed: {:?}",
            error.reason
        ))
        .into());
    }
    presentation_trace.push(PresentationTransactionEvent::WorkerQueued {
        transaction_id,
        timestamp_ns: queued_at_ns,
    });
    Ok(WorkerQueueOutcome::Queued {
        transaction_id,
        token,
        framebuffer_id: FramebufferId::new(framebuffer_id)
            .ok_or_else(|| io::Error::other("compatibility framebuffer ID is zero"))?,
    })
}

pub(super) fn drain_worker_eventfd(
    worker: &KmsCommitWorkerHandle,
) -> NativeResult<Vec<KmsWorkerEvent>> {
    worker.drain_eventfd()?;
    Ok(worker.drain_events())
}

impl NativeRuntime {
    pub(super) fn check_kms_commit_worker_health(&mut self) -> NativeResult<()> {
        let Some(reason) = self
            .kms_commit_worker
            .as_ref()
            .and_then(KmsCommitWorkerHandle::fatal_reason)
        else {
            return Ok(());
        };
        let events = self
            .kms_commit_worker
            .as_ref()
            .map(KmsCommitWorkerHandle::drain_events)
            .unwrap_or_default();
        if let Some(worker) = self.kms_commit_worker.as_ref() {
            worker.record_unnotified_fatal_health_check();
        }
        let mut event_error = None;
        for event in events {
            if let Err(error) = self.process_kms_worker_event(event) {
                event_error = Some(error.to_string());
            }
        }
        let fatal_jobs = self
            .kms_commit_worker
            .as_ref()
            .map(KmsCommitWorkerHandle::take_fatal_jobs)
            .unwrap_or_default();
        for fatal_job in fatal_jobs {
            if !fatal_job.uncertain_submit {
                self.fail_queued_worker_job(
                    fatal_job.job,
                    AtomicKmsError::new(
                        oblivion_one::native::kms::AtomicKmsErrorKind::FlipRejected,
                        "KMS worker result notification failed before submit completion",
                    ),
                )?;
            }
        }
        self.quarantine_after_worker_fatal()?;
        if let Some(error) = event_error {
            return Err(io::Error::other(error).into());
        }
        Err(io::Error::other(format!(
            "Atomic KMS worker health check observed fatal state: {reason:?}"
        ))
        .into())
    }

    pub(super) fn quarantine_after_worker_fatal(&mut self) -> NativeResult<()> {
        self.frame_scheduler.abandon_for_session_suspend();
        self.atomic_commit_arbiter.abandon_for_recovery();
        self.deferred_worker_pageflip = None;
        self.deferred_worker_completion = None;
        self.worker_timeout_pending = None;
        if let Some(token) = self.output_render_fence_token.take() {
            self.event_loop.unregister(token)?;
        }
        self.scanout.suspend_page_flip()?;
        Ok(())
    }

    pub(super) fn restart_kms_commit_worker_after_recovery(&mut self) -> NativeResult<()> {
        if self.kms_commit_worker_transport
            != crate::native_output::kms_worker::KmsCommitWorkerTransport::Worker
        {
            return Ok(());
        }
        if self.kms_commit_worker.is_some() {
            return Err(
                io::Error::other("KMS commit worker still exists during recovery restart").into(),
            );
        }
        let submitter = self
            .kms_backend
            .atomic()
            .ok_or_else(|| io::Error::other("worker transport has no Atomic backend"))?
            .commit_submitter();
        match KmsCommitWorkerHandle::start_atomic(submitter) {
            Ok(worker) => {
                self.kms_commit_worker = Some(worker);
                Ok(())
            }
            Err(error)
                if self.kms_commit_worker_policy
                    == crate::native_output::kms_worker::KmsCommitWorkerPolicy::Auto =>
            {
                eprintln!(
                    "native KMS commit worker: recovery restart failed ({error:?}); using synchronous transport"
                );
                self.kms_commit_worker_transport =
                    crate::native_output::kms_worker::KmsCommitWorkerTransport::Synchronous;
                Ok(())
            }
            Err(error) => Err(io::Error::other(format!(
                "native KMS commit worker recovery restart failed: {error:?}"
            ))
            .into()),
        }
    }

    pub(super) fn process_kms_worker_event_after_join(
        &mut self,
        event: KmsWorkerEvent,
    ) -> NativeResult<()> {
        self.process_kms_worker_event(event)
    }

    pub(super) fn process_kms_worker_events(&mut self) -> NativeResult<()> {
        let Some(worker) = self.kms_commit_worker.as_ref() else {
            return Ok(());
        };
        let events = drain_worker_eventfd(worker)?;
        for event in events {
            self.process_kms_worker_event(event)?;
        }
        Ok(())
    }

    fn process_kms_worker_event(&mut self, event: KmsWorkerEvent) -> NativeResult<()> {
        match event {
            KmsWorkerEvent::Submitted {
                transaction_id,
                token,
                kind,
                output_generation,
                queued_at,
                submit_started_at,
                submit_returned_at,
                out_fence,
                cursor,
            } => {
                if output_generation != self.drm_file_generation
                    || self.atomic_commit_arbiter.worker_queued_token() != Some(token)
                    || self.atomic_commit_arbiter.worker_queued_kind() != Some(kind)
                    || !self.atomic_commit_arbiter.worker_job_queued()
                {
                    if let Some(worker) = self.kms_commit_worker.as_ref() {
                        worker.record_result_mismatch();
                    }
                    return Err(io::Error::other(
                        "worker success did not match queued Atomic ownership",
                    )
                    .into());
                }
                let transaction = self
                    .output_transactions
                    .transaction(transaction_id)
                    .ok_or_else(|| {
                        if let Some(worker) = self.kms_commit_worker.as_ref() {
                            worker.record_result_mismatch();
                        }
                        io::Error::other("worker success transaction is missing")
                    })?;
                if !matches!(transaction.state(), OutputTransactionState::Queued { .. }) {
                    if let Some(worker) = self.kms_commit_worker.as_ref() {
                        worker.record_result_mismatch();
                    }
                    return Err(io::Error::other("worker success transaction is not queued").into());
                }
                let compatibility_primary = matches!(
                    transaction.descriptor().planes().primary(),
                    PrimaryPlaneAssignment::CompatibilityFramebuffer { .. }
                );
                let has_out_fence = out_fence.is_some();
                if matches!(kind, AtomicCommitKind::CompositedPrimary { .. }) {
                    if compatibility_primary {
                        drop(out_fence);
                        self.scanout
                            .promote_worker_compatibility_submission(token)?;
                    } else {
                        self.scanout.promote_worker_submission(
                            token,
                            out_fence,
                            MonotonicTimestampNs::new(submit_started_at),
                            MonotonicTimestampNs::new(submit_returned_at),
                        )?;
                    }
                } else if matches!(kind, AtomicCommitKind::DirectPrimary { .. }) {
                    let batch_id = self.scanout.promote_worker_direct_submission(
                        token,
                        out_fence,
                        MonotonicTimestampNs::new(submit_started_at),
                        MonotonicTimestampNs::new(submit_returned_at),
                    )?;
                    self.server.complete_rendered_frame_callbacks(batch_id);
                } else if has_out_fence {
                    return Err(io::Error::other(
                        "cursor-only worker submission unexpectedly returned an out-fence",
                    )
                    .into());
                }
                self.output_transactions
                    .mark_submitted(
                        transaction_id,
                        token,
                        MonotonicTimestampNs::new(submit_returned_at),
                    )
                    .map_err(io::Error::other)?;
                self.atomic_commit_arbiter
                    .mark_kernel_submitted(token, submit_started_at, submit_returned_at)
                    .map_err(io::Error::other)?;
                if !matches!(kind, AtomicCommitKind::CursorOnly { .. }) {
                    self.frame_scheduler
                        .confirm_kernel_submission(token.get(), submit_returned_at)
                        .map_err(io::Error::other)?;
                }
                let deferred_pageflip = self.atomic_commit_arbiter.deferred_pageflip();
                let deferred_completion = self.atomic_commit_arbiter.replay_deferred_pageflip();
                if let Some(pageflip) = deferred_pageflip {
                    let Some(completion) = deferred_completion else {
                        return Err(io::Error::other(
                            "queued pageflip was not replayed after Atomic submit acknowledgment",
                        )
                        .into());
                    };
                    self.deferred_worker_pageflip = Some(pageflip);
                    self.deferred_worker_completion = Some(completion);
                }
                self.presentation_trace
                    .push(PresentationTransactionEvent::KmsSubmitStarted {
                        transaction_id,
                        timestamp_ns: submit_started_at,
                    });
                self.presentation_trace
                    .push(PresentationTransactionEvent::KmsSubmitReturned {
                        transaction_id,
                        timestamp_ns: submit_returned_at,
                    });
                self.perf.log("native.kms_commit_worker", || {
                    vec![
                        NativePerfField::u64(
                            "queue_wait_ns",
                            submit_started_at.saturating_sub(queued_at),
                        ),
                        NativePerfField::u64(
                            "submit_duration_ns",
                            submit_returned_at.saturating_sub(submit_started_at),
                        ),
                    ]
                });
                if let Some(native_cursor) = self.atomic_cursor.as_mut() {
                    match cursor {
                        KmsCursorUpdate::Set(state) => {
                            if matches!(kind, AtomicCommitKind::CursorOnly { .. }) {
                                native_cursor.begin_submission(token, state);
                            } else {
                                native_cursor.begin_primary_submission(token, state);
                            }
                        }
                        KmsCursorUpdate::Disable => {
                            let mut hidden = native_cursor.desired().clone();
                            hidden.visible = false;
                            hidden.framebuffer_id = None;
                            if matches!(kind, AtomicCommitKind::CursorOnly { .. }) {
                                native_cursor.begin_submission(token, hidden);
                            } else {
                                native_cursor.begin_primary_submission(token, hidden);
                            }
                        }
                        KmsCursorUpdate::Unchanged => {}
                    }
                }
                if matches!(kind, AtomicCommitKind::CompositedPrimary { .. })
                    && self.output_render_fence_token.is_none()
                    && let NativeScanoutBackend::AtomicEglGbm(explicit) = &*self.scanout
                    && let Some(fd) = explicit.pending_timing_fd()
                {
                    self.output_render_fence_token = Some(
                        self.event_loop
                            .register(fd, NativeEventSource::OutputRenderFence)?,
                    );
                }
            }
            KmsWorkerEvent::TestRejected { job, error }
            | KmsWorkerEvent::SubmitRejected { job, error }
            | KmsWorkerEvent::BusyExhausted { job, error } => {
                self.fail_queued_worker_job(job, error)?;
            }
            KmsWorkerEvent::BusyDeferred { .. } => {}
            KmsWorkerEvent::SubmitLate {
                transaction_id,
                token,
                late_by_ns,
            } => {
                self.perf.log("native.kms_commit_worker", || {
                    vec![
                        NativePerfField::u64("late_transaction_id", transaction_id.get()),
                        NativePerfField::u64("late_token", token.get()),
                        NativePerfField::u64("late_by_ns", late_by_ns),
                    ]
                });
            }
            KmsWorkerEvent::PageflipTimeout {
                transaction_id: _,
                token,
                detected_at,
            } => {
                if self.atomic_commit_arbiter.kernel_commit_submitted()
                    && self.atomic_commit_arbiter.pending_atomic_token() == Some(token)
                {
                    self.worker_timeout_pending = Some((token, detected_at));
                }
            }
            KmsWorkerEvent::Quiesced { returned_jobs } => {
                for job in returned_jobs {
                    self.drop_queued_worker_job(job)?;
                }
            }
            KmsWorkerEvent::Fatal {
                reason,
                uncertain_submit,
            } => {
                return Err(io::Error::other(format!(
                    "Atomic KMS worker fatal event: {reason:?}, uncertain_submit={uncertain_submit}"
                ))
                .into());
            }
        }
        Ok(())
    }

    fn fail_queued_worker_job(
        &mut self,
        job: KmsCommitJob,
        error: AtomicKmsError,
    ) -> NativeResult<()> {
        if !matches!(job.kind, AtomicCommitKind::CursorOnly { .. }) {
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
        let direct_batch = if matches!(job.kind, AtomicCommitKind::DirectPrimary { .. }) {
            Some(self.scanout.fail_worker_direct_submission(job.token)?)
        } else {
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
            None
        };
        settle_failed_output_transaction(
            &mut self.output_transactions,
            job.transaction_id,
            OutputTransactionFailureStage::KmsSubmit,
            MonotonicTimestampNs::new(monotonic_now_ns()?),
            |obligations| {
                if let Some(batch_id) = direct_batch {
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

    pub(super) fn drop_queued_worker_job(&mut self, job: KmsCommitJob) -> NativeResult<()> {
        if !matches!(job.kind, AtomicCommitKind::CursorOnly { .. }) {
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
                    .suspend_abandon_worker_compatibility(job.token)
                    .map_err(io::Error::other)?;
            } else {
                self.scanout
                    .suspend_abandon_worker_submission(job.token)
                    .map_err(io::Error::other)?;
            }
        } else if matches!(job.kind, AtomicCommitKind::DirectPrimary { .. }) {
            self.scanout
                .suspend_abandon_worker_direct(job.token)
                .map_err(io::Error::other)?;
        }
        settle_dropped_output_transaction(
            &mut self.output_transactions,
            job.transaction_id,
            OutputTransactionDropReason::SessionSuspended,
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
        Ok(())
    }
}
