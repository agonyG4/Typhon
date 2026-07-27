//! Main-thread adapter for the submit-only Atomic KMS worker.
//!
//! This module is intentionally small: worker results are transported here,
//! while ledger settlement, DRM event reading, and physical scanout promotion
//! remain owned by the compositor thread.

use super::super::kms_worker::{
    KmsCommitJob, KmsCommitWorkerHandle, KmsCursorUpdate, KmsPrimaryUpdate, KmsSubmittedOwnership,
    KmsTestOnlyPolicy, KmsWorkerAdmissionError, KmsWorkerEvent, KmsWorkerFatalJob,
};
use super::presentation_transactions::{
    build_compatibility_transaction, settle_dropped_output_transaction,
    settle_failed_output_transaction, settle_forced_shutdown_transaction,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FatalWorkerJobDisposition {
    Fail,
    #[allow(dead_code)]
    Drop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UncertainJobRetention {
    Suspended,
    EmergencyQuarantined,
}

pub(super) trait FatalWorkerJobHandler {
    fn retain_uncertain_worker_job(
        &mut self,
        job: KmsCommitJob,
    ) -> NativeResult<UncertainJobRetention>;
    fn fail_known_worker_job(&mut self, job: KmsCommitJob) -> NativeResult<()>;
    fn drop_known_worker_job(&mut self, job: KmsCommitJob) -> NativeResult<()>;
}

pub(super) fn handle_fatal_worker_jobs(
    fatal_jobs: impl IntoIterator<Item = KmsWorkerFatalJob>,
    handler: &mut impl FatalWorkerJobHandler,
    known_job_disposition: FatalWorkerJobDisposition,
) -> NativeResult<Vec<UncertainJobRetention>> {
    let mut retentions = Vec::new();
    for fatal_job in fatal_jobs {
        if fatal_job.uncertain_submit {
            retentions.push(handler.retain_uncertain_worker_job(fatal_job.job)?);
        } else {
            match known_job_disposition {
                FatalWorkerJobDisposition::Fail => handler.fail_known_worker_job(fatal_job.job)?,
                FatalWorkerJobDisposition::Drop => handler.drop_known_worker_job(fatal_job.job)?,
            }
        }
    }
    Ok(retentions)
}

pub(super) fn retain_uncertain_job_with_suspension(
    mut job: KmsCommitJob,
    suspended_jobs: &mut Vec<KmsCommitJob>,
    emergency_jobs: &mut Vec<KmsCommitJob>,
    suspend_direct: impl FnOnce(
        PageFlipToken,
        crate::native_output::scanout::DirectPrimaryLease,
    ) -> Result<
        (),
        crate::native_output::scanout::DirectPrimaryLeaseTransferError,
    >,
) -> NativeResult<UncertainJobRetention> {
    if matches!(job.kind, AtomicCommitKind::DirectPrimary { .. }) {
        let Some(lease) = job.direct_primary_lease.take() else {
            emergency_jobs.push(job);
            return Ok(UncertainJobRetention::EmergencyQuarantined);
        };
        match suspend_direct(job.token, lease) {
            Ok(()) => {}
            Err(error) => {
                let (error, lease) = *error;
                eprintln!(
                    "native KMS worker: direct uncertain-job suspension failed; retaining emergency ownership: {error}"
                );
                job.direct_primary_lease = Some(lease);
                emergency_jobs.push(job);
                return Ok(UncertainJobRetention::EmergencyQuarantined);
            }
        }
    }
    suspended_jobs.push(job);
    Ok(UncertainJobRetention::Suspended)
}

pub(super) fn retain_complete_submitted_ownership(
    ownership: KmsSubmittedOwnership,
    emergency_ownership: &mut Vec<KmsSubmittedOwnership>,
) {
    emergency_ownership.push(ownership);
}

#[allow(clippy::too_many_arguments)]
pub(super) fn queue_cursor_only(
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
        cursor: desired
            .clone()
            .map_or(KmsCursorUpdate::Disable, KmsCursorUpdate::Set),
        cursor_pin: desired
            .as_ref()
            .filter(|state| state.framebuffer_id.is_some())
            .map(|state| cursor.pin_framebuffer_for(state))
            .transpose()?,
        direct_primary_lease: None,
        pacing_frame_id: None,
        test_only: KmsTestOnlyPolicy::Required,
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
    if let Err(error) =
        cursor.queue_worker_submission(transaction_id, token, cursor_epoch, queued_visual_state)
    {
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
    cursor_pin: Option<CursorFramebufferPin>,
    pacing_frame_id: Option<u64>,
    ready_submit: bool,
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
        cursor_pin,
        direct_primary_lease: None,
        pacing_frame_id,
        test_only: KmsTestOnlyPolicy::Skip,
        ready_submit,
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
    cursor_pin: Option<CursorFramebufferPin>,
    pacing_frame_id: Option<u64>,
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
        cursor_pin,
        direct_primary_lease: None,
        pacing_frame_id,
        test_only: KmsTestOnlyPolicy::Skip,
        ready_submit: true,
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
    pub(super) fn retain_uncertain_worker_job(
        &mut self,
        job: KmsCommitJob,
    ) -> NativeResult<UncertainJobRetention> {
        let suspended_jobs = &mut self.quarantined_worker_jobs;
        let emergency_jobs = &mut self.emergency_quarantined_worker_jobs;
        let scanout = &mut self.scanout;
        retain_uncertain_job_with_suspension(job, suspended_jobs, emergency_jobs, |token, lease| {
            scanout.suspend_worker_direct_submission(token, lease)
        })
    }

    fn fail_known_worker_job(&mut self, job: KmsCommitJob) -> NativeResult<()> {
        self.fail_queued_worker_job(
            job,
            AtomicKmsError::new(
                oblivion_one::native::kms::AtomicKmsErrorKind::FlipRejected,
                "KMS worker result notification failed before submit completion",
            ),
        )
    }

    pub(super) fn stop_kms_worker_admission_for_shutdown(
        &mut self,
    ) -> NativeResult<Option<super::super::kms_worker::WorkerInFlight>> {
        let snapshot = {
            let Some(worker) = self.kms_commit_worker.as_ref() else {
                return Ok(None);
            };
            worker.begin_shutdown_quiesce().map_err(|error| {
                io::Error::other(format!("KMS worker shutdown quiesce: {error:?}"))
            })?
        };
        if let Some(job) = snapshot.queued_job {
            self.drop_queued_worker_job_with_reason(
                job,
                OutputTransactionDropReason::SafeAbandonment,
            )?;
        }
        Ok(snapshot.inflight)
    }

    pub(super) fn force_kms_worker_shutdown_abandon(&mut self) -> NativeResult<()> {
        let Some(worker) = self.kms_commit_worker.as_ref() else {
            return Ok(());
        };
        let snapshot = worker.force_shutdown_abandon().map_err(|error| {
            io::Error::other(format!("KMS worker forced shutdown abandonment: {error:?}"))
        })?;
        if let Some(job) = snapshot.queued_job {
            self.drop_queued_worker_job_with_reason(
                job,
                OutputTransactionDropReason::SafeAbandonment,
            )?;
        }
        if let Some(inflight) = snapshot.inflight {
            self.forced_shutdown_inflight = Some(inflight);
            let pacing_cleared = self
                .frame_pacing
                .abandon_pending_submission(inflight.token.get());
            self.perf.log("native.kms_commit_worker", || {
                vec![
                    NativePerfField::str("event", snapshot.disposition.as_str()),
                    NativePerfField::u64("token", inflight.token.get()),
                    NativePerfField::u64("transaction_id", inflight.transaction_id.get()),
                    NativePerfField::bool("pacing_pending_cleared", pacing_cleared),
                ]
            });
        } else {
            self.perf.log("native.kms_commit_worker", || {
                vec![NativePerfField::str("event", snapshot.disposition.as_str())]
            });
        }
        Ok(())
    }

    pub(super) fn settle_forced_shutdown_inflight(
        &mut self,
        identity: super::super::kms_worker::WorkerInFlight,
    ) -> NativeResult<()> {
        let transaction_id = identity.transaction_id;
        let token = identity.token;
        let settled = settle_forced_shutdown_transaction(
            &mut self.output_transactions,
            transaction_id,
            token,
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
        self.perf.log("native.kms_commit_worker", || {
            vec![
                NativePerfField::str(
                    "event",
                    if settled {
                        "shutdown_transaction_dropped_safe_abandonment"
                    } else {
                        "shutdown_transaction_already_terminal"
                    },
                ),
                NativePerfField::u64("token", token.get()),
                NativePerfField::u64("transaction_id", transaction_id.get()),
                NativePerfField::str("job_kind", format!("{:?}", identity.kind)),
            ]
        });
        Ok(())
    }

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
        let _ = handle_fatal_worker_jobs(fatal_jobs, self, FatalWorkerJobDisposition::Fail)?;
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

    fn validate_submitted_ownership(&self, ownership: &KmsSubmittedOwnership) -> NativeResult<()> {
        let job = &ownership.job;
        if job.output_generation != self.drm_file_generation
            || self.atomic_commit_arbiter.worker_queued_token() != Some(job.token)
            || self.atomic_commit_arbiter.worker_queued_kind() != Some(job.kind)
            || !self.atomic_commit_arbiter.worker_job_queued()
        {
            return Err(
                io::Error::other("worker success did not match queued Atomic ownership").into(),
            );
        }
        let transaction = self
            .output_transactions
            .transaction(job.transaction_id)
            .ok_or_else(|| io::Error::other("worker success transaction is missing"))?;
        if !matches!(transaction.state(), OutputTransactionState::Queued { .. }) {
            return Err(io::Error::other("worker success transaction is not queued").into());
        }
        job.validate_submitted_against(transaction.descriptor())
            .map_err(|error| {
                io::Error::other(format!("invalid submitted worker payload: {error:?}"))
            })?;
        Ok(())
    }

    pub(super) fn quarantine_submitted_ownership(
        &mut self,
        ownership: KmsSubmittedOwnership,
    ) -> NativeResult<()> {
        if matches!(ownership.job.kind, AtomicCommitKind::DirectPrimary { .. }) {
            // The submitted direct lease is the complete physical ownership
            // record. Keep it intact; suspend_page_flip below only removes
            // compositor-side queue metadata.
            retain_complete_submitted_ownership(
                ownership,
                &mut self.emergency_quarantined_submitted_ownership,
            );
        } else {
            if matches!(
                ownership.job.kind,
                AtomicCommitKind::CompositedPrimary { .. }
            ) {
                let compatibility_primary = self
                    .output_transactions
                    .transaction(ownership.job.transaction_id)
                    .is_some_and(|transaction| {
                        matches!(
                            transaction.descriptor().planes().primary(),
                            PrimaryPlaneAssignment::CompatibilityFramebuffer { .. }
                        )
                    });
                let result = if compatibility_primary {
                    self.scanout
                        .suspend_abandon_worker_compatibility(ownership.job.token)
                } else {
                    self.scanout
                        .suspend_abandon_worker_submission(ownership.job.token)
                };
                if let Err(error) = result {
                    eprintln!(
                        "native KMS worker: submitted output suspension failed; retaining emergency ownership: {error}"
                    );
                }
            }
            retain_complete_submitted_ownership(
                ownership,
                &mut self.emergency_quarantined_submitted_ownership,
            );
        }
        self.quarantine_after_worker_fatal()
    }

    pub(super) fn process_kms_worker_event(&mut self, event: KmsWorkerEvent) -> NativeResult<()> {
        match event {
            KmsWorkerEvent::Submitted { ownership } => {
                if let Err(error) = self.validate_submitted_ownership(&ownership) {
                    if let Some(worker) = self.kms_commit_worker.as_ref() {
                        worker.record_result_mismatch();
                    }
                    self.quarantine_submitted_ownership(ownership)?;
                    return Err(error);
                }
                self.submitted_worker_ownership.push(ownership);
                let ownership = self
                    .submitted_worker_ownership
                    .last_mut()
                    .expect("submitted ownership was just retained");
                let transaction_id = ownership.job.transaction_id;
                let token = ownership.job.token;
                let kind = ownership.job.kind;
                let queued_at = ownership.job.queued_at.get();
                let submit_started_at = ownership.submit_started_at.get();
                let submit_returned_at = ownership.submit_returned_at.get();
                let output_generation = ownership.job.output_generation;
                let target = ownership.job.target;
                let cursor = ownership.job.cursor.clone();
                let pacing_frame_id = ownership.job.pacing_frame_id;
                let ready_submit = ownership.job.ready_submit;
                let out_fence = ownership.out_fence.take();
                let direct_primary_lease = ownership.job.direct_primary_lease.take();
                let transaction = self
                    .output_transactions
                    .transaction(transaction_id)
                    .ok_or_else(|| io::Error::other("worker success transaction is missing"))?;
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
                    let Some(direct_primary_lease) = direct_primary_lease else {
                        let mut ownership = self
                            .submitted_worker_ownership
                            .pop()
                            .expect("submitted ownership was just retained");
                        ownership.out_fence = out_fence;
                        retain_complete_submitted_ownership(
                            ownership,
                            &mut self.emergency_quarantined_submitted_ownership,
                        );
                        self.quarantine_after_worker_fatal()?;
                        return Err(io::Error::other(
                            "direct worker submission has no primary lease",
                        )
                        .into());
                    };
                    let batch_id = match self.scanout.promote_worker_direct_submission(
                        DirectPromotionContext {
                            transaction_id,
                            token,
                            output_generation,
                            target,
                            submit_started_at: MonotonicTimestampNs::new(submit_started_at),
                            submit_returned_at: MonotonicTimestampNs::new(submit_returned_at),
                        },
                        direct_primary_lease,
                        out_fence,
                    ) {
                        Ok(batch_id) => batch_id,
                        Err(error) => {
                            let DirectPromotionError {
                                error,
                                lease,
                                out_fence,
                                ..
                            } = *error;
                            let mut ownership = self
                                .submitted_worker_ownership
                                .pop()
                                .expect("submitted ownership was just retained");
                            ownership.job.direct_primary_lease = Some(lease);
                            ownership.out_fence = out_fence;
                            retain_complete_submitted_ownership(
                                ownership,
                                &mut self.emergency_quarantined_submitted_ownership,
                            );
                            self.quarantine_after_worker_fatal()?;
                            return Err(error.into());
                        }
                    };
                    self.server.complete_rendered_frame_callbacks(batch_id);
                } else if has_out_fence {
                    return Err(io::Error::other(
                        "cursor-only worker submission unexpectedly returned an out-fence",
                    )
                    .into());
                }
                let cursor_epoch = match kind {
                    AtomicCommitKind::CursorOnly { cursor_epoch, .. } => Some(cursor_epoch),
                    AtomicCommitKind::CompositedPrimary { .. }
                    | AtomicCommitKind::DirectPrimary { .. } => match transaction
                        .descriptor()
                        .planes()
                        .cursor()
                    {
                        CursorPlaneAssignment::Atomic { desired_epoch, .. } => Some(desired_epoch),
                        CursorPlaneAssignment::Unchanged | CursorPlaneAssignment::Disabled => None,
                    },
                };
                if let Some(cursor_epoch) = cursor_epoch {
                    let submitted_state = if matches!(kind, AtomicCommitKind::CursorOnly { .. }) {
                        let native_cursor = self.atomic_cursor.as_mut().ok_or_else(|| {
                            io::Error::other("cursor worker submit has no cursor")
                        })?;
                        let queued = native_cursor
                            .take_worker_submission(transaction_id, token, cursor_epoch)
                            .inspect_err(|_error| {
                                if let Some(worker) = self.kms_commit_worker.as_ref() {
                                    worker.record_cursor_worker_epoch_mismatch();
                                }
                            })?;
                        match cursor {
                            KmsCursorUpdate::Set(state) => {
                                if state != queued.visual_state {
                                    if let Some(worker) = self.kms_commit_worker.as_ref() {
                                        worker.record_cursor_worker_epoch_mismatch();
                                    }
                                    return Err(io::Error::other(
                                        "worker cursor state does not match queued cursor state",
                                    )
                                    .into());
                                }
                                state.clone()
                            }
                            KmsCursorUpdate::Disable => queued.visual_state,
                            KmsCursorUpdate::Unchanged => {
                                return Err(io::Error::other(
                                    "worker cursor submission has no cursor update",
                                )
                                .into());
                            }
                        }
                    } else {
                        match cursor {
                            KmsCursorUpdate::Set(state) => state.clone(),
                            KmsCursorUpdate::Disable => {
                                let native_cursor =
                                    self.atomic_cursor.as_ref().ok_or_else(|| {
                                        io::Error::other("primary cursor submit has no cursor")
                                    })?;
                                let mut hidden = native_cursor.desired().clone();
                                hidden.visible = false;
                                hidden.framebuffer_id = None;
                                hidden
                            }
                            KmsCursorUpdate::Unchanged => {
                                return Err(io::Error::other(
                                    "primary cursor submission has no cursor update",
                                )
                                .into());
                            }
                        }
                    };
                    if let Some(native_cursor) = self.atomic_cursor.as_mut() {
                        if matches!(kind, AtomicCommitKind::CursorOnly { .. }) {
                            native_cursor.begin_submission_at_epoch(
                                token,
                                submitted_state,
                                cursor_epoch,
                            );
                        } else {
                            native_cursor.begin_primary_submission_at_epoch(
                                token,
                                submitted_state,
                                cursor_epoch,
                            );
                        }
                    }
                    self.last_submitted_cursor_epoch = cursor_epoch;
                    if matches!(kind, AtomicCommitKind::CursorOnly { .. }) {
                        self.cursor_output_arbitration.note_cursor_only_submission();
                        self.cursor_output_arbitration.consume_submitted_epoch(
                            cursor_epoch,
                            submit_returned_at,
                            self.frame_scheduler
                                .next_refresh_deadline_ns(submit_returned_at),
                        );
                        if let Some(worker) = self.kms_commit_worker.as_ref() {
                            worker.record_cursor_worker_submit_confirmed();
                            worker.record_cursor_worker_arbitration_consumed();
                        }
                    } else {
                        self.cursor_output_arbitration.consume_submitted_epoch(
                            cursor_epoch,
                            submit_returned_at,
                            self.frame_scheduler
                                .next_refresh_deadline_ns(submit_returned_at),
                        );
                    }
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
                if !matches!(kind, AtomicCommitKind::CursorOnly { .. }) {
                    let pacing_mode = self
                        .output_transactions
                        .transaction(transaction_id)
                        .ok_or_else(|| io::Error::other("worker pacing transaction disappeared"))?
                        .descriptor()
                        .pacing_mode();
                    self.frame_pacing
                        .note_worker_submit_exact(
                            pacing_frame_id,
                            token.get(),
                            submit_returned_at,
                            ready_submit,
                            pacing_mode,
                        )
                        .map_err(io::Error::other)?;
                    if let Some(worker) = self.kms_commit_worker.as_ref() {
                        worker.record_worker_pacing_submit_confirmed();
                    }
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
                    let reason = if self.shutdown.is_shutting_down() {
                        OutputTransactionDropReason::SafeAbandonment
                    } else {
                        OutputTransactionDropReason::SessionSuspended
                    };
                    self.drop_queued_worker_job_with_reason(job, reason)?;
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
        self.drop_queued_worker_job_with_reason(job, OutputTransactionDropReason::SessionSuspended)
    }

    pub(super) fn drop_queued_worker_job_with_reason(
        &mut self,
        job: KmsCommitJob,
        drop_reason: OutputTransactionDropReason,
    ) -> NativeResult<()> {
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
        } else if matches!(job.kind, AtomicCommitKind::DirectPrimary { .. }) {
            self.scanout
                .suspend_abandon_worker_direct(job.token)
                .map_err(io::Error::other)?;
        }
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
        if drop_reason == OutputTransactionDropReason::SafeAbandonment
            && let Some(worker) = self.kms_commit_worker.as_ref()
        {
            worker.record_shutdown_queued_job_settled();
        }
        Ok(())
    }
}

impl FatalWorkerJobHandler for NativeRuntime {
    fn retain_uncertain_worker_job(
        &mut self,
        job: KmsCommitJob,
    ) -> NativeResult<UncertainJobRetention> {
        self.retain_uncertain_worker_job(job)
    }

    fn fail_known_worker_job(&mut self, job: KmsCommitJob) -> NativeResult<()> {
        self.fail_known_worker_job(job)
    }

    fn drop_known_worker_job(&mut self, job: KmsCommitJob) -> NativeResult<()> {
        self.drop_queued_worker_job(job)
    }
}
