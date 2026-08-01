//! Main-thread adapter for the submit-only Atomic KMS worker.
//!
//! This module is intentionally small: worker results are transported here,
//! while ledger settlement, DRM event reading, and physical scanout promotion
//! remain owned by the compositor thread.

use std::sync::Arc;

use super::super::kms_worker::{
    CursorSidecarReturnReason, KmsBundleOwners, KmsCommitJob, KmsCommitWorkerHandle,
    KmsCursorUpdate, KmsPrimaryUpdate, KmsSubmittedOwnership, KmsTestOnlyPolicy, KmsValidationBase,
    KmsWorkerAdmissionError, KmsWorkerEvent, KmsWorkerFatalJob,
};
pub(super) use super::kms_worker_teardown::{
    retain_complete_submitted_ownership, retain_uncertain_job_with_suspension,
};
pub(super) use super::plane_cycle::{WorkerQueueOutcome, queue_plane_delta};
use super::presentation_transactions::{
    build_compatibility_transaction, settle_failed_output_transaction,
    settle_forced_shutdown_transaction_if_safe,
};
use super::*;
use crate::native_output::scanout::AtomicEglGbmScanout;
use oblivion_one::native::kms::{AtomicKmsError, FramebufferId, PageFlipToken};

#[path = "direct_rejection.rs"]
mod direct_rejection;
#[allow(unused_imports)]
pub(super) use direct_rejection::{WorkerRejectionKind, direct_rejection_policy};
mod rejection;

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
    cursor_update: KmsCursorUpdate,
    cursor_pin: Option<CursorFramebufferPin>,
    cursor_capability_key: Option<
        crate::native_output::presentation::plane_policy::CursorCapabilityKey,
    >,
    pacing_frame_id: Option<u64>,
    test_only: KmsTestOnlyPolicy,
    ready_submit: bool,
    validation_base: KmsValidationBase,
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
        bundle_id:
            crate::native_output::presentation::plane::KmsCommitBundleId::from_pageflip_token(token),
        owners: KmsBundleOwners::for_legacy_transaction(
            kind,
            Arc::new(
                output_transactions
                    .transaction(transaction_id)
                    .ok_or_else(|| io::Error::other("queued worker transaction disappeared"))?
                    .descriptor()
                    .clone(),
            ),
            cursor_capability_key,
        ),
        transaction_id,
        token,
        output_generation,
        crtc_id,
        kind,
        target,
        validation_base,
        queued_at: MonotonicTimestampNs::new(queued_at_ns),
        primary: KmsPrimaryUpdate::Framebuffer {
            framebuffer,
            in_fence: Some(in_fence),
            request_out_fence: true,
        },
        cursor: cursor_update,
        cursor_pin,
        direct_primary_lease: None,
        test_only_duration_ns: None,
        pacing_frame_id,
        test_only,
        ready_submit,
    };
    let queued_descriptor = output_transactions
        .transaction(transaction_id)
        .ok_or_else(|| io::Error::other("queued worker transaction disappeared"))?;
    if let Err(error) = job.validate_against(queued_descriptor.descriptor()) {
        if atomic_commit_arbiter.reject_worker_queued(token).is_none() {
            return Err(io::Error::other(
                "invalid Atomic worker payload could not roll back arbiter reservation",
            )
            .into());
        }
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
        if atomic_commit_arbiter.reject_worker_queued(token).is_none() {
            return Err(io::Error::other(
                "Atomic worker enqueue failed and arbiter rollback did not match",
            )
            .into());
        }
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
    cursor_capability_key: Option<
        crate::native_output::presentation::plane_policy::CursorCapabilityKey,
    >,
    pacing_frame_id: Option<u64>,
    test_only: KmsTestOnlyPolicy,
    cursor_epoch: u64,
    validation_base: KmsValidationBase,
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
        bundle_id:
            crate::native_output::presentation::plane::KmsCommitBundleId::from_pageflip_token(token),
        owners: KmsBundleOwners::for_legacy_transaction(
            kind,
            Arc::new(
                output_transactions
                    .transaction(transaction_id)
                    .ok_or_else(|| {
                        io::Error::other("compatibility worker transaction disappeared")
                    })?
                    .descriptor()
                    .clone(),
            ),
            cursor_capability_key,
        ),
        transaction_id,
        token,
        output_generation,
        crtc_id,
        kind,
        target,
        validation_base,
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
        test_only_duration_ns: None,
        pacing_frame_id,
        test_only,
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
        if matches!(job.kind, AtomicCommitKind::DirectPrimary { .. })
            && let Some(duration_ns) = job.test_only_duration_ns
        {
            self.scanout.note_direct_test_only(duration_ns, false);
        }
        let suspended_jobs = &mut self.worker_quarantine.jobs;
        let emergency_jobs = &mut self.emergency_quarantined_worker_jobs;
        retain_uncertain_job_with_suspension(job, suspended_jobs, emergency_jobs)
    }

    pub(super) fn fail_known_worker_job_impl(&mut self, job: KmsCommitJob) -> NativeResult<()> {
        self.fail_queued_worker_job(
            job,
            AtomicKmsError::new(
                oblivion_one::native::kms::AtomicKmsErrorKind::FlipRejected,
                "KMS worker result notification failed before submit completion",
            ),
            WorkerRejectionKind::TestOnly,
        )
    }

    pub(super) fn stop_kms_worker_admission_for_shutdown(
        &mut self,
    ) -> NativeResult<Option<super::super::kms_worker::WorkerInFlight>> {
        self.abandon_direct_fallback();
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
        self.worker_quarantine
            .cursor_sidecars
            .extend(snapshot.pending_sidecar);
        Ok(snapshot.inflight)
    }

    pub(super) fn force_kms_worker_shutdown_abandon(&mut self) -> NativeResult<()> {
        self.abandon_direct_fallback();
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
        self.worker_quarantine
            .cursor_sidecars
            .extend(snapshot.pending_sidecar);
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
        let settled = settle_forced_shutdown_transaction_if_safe(
            self.kms_teardown_safety,
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
            let event = if !self.kms_teardown_safety.permits_release() {
                "shutdown_transaction_retained_unproven"
            } else if settled {
                "shutdown_transaction_dropped_safe_abandonment"
            } else {
                "shutdown_transaction_already_terminal"
            };
            vec![
                NativePerfField::str("event", event),
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
        let fatal_sidecar = self
            .kms_commit_worker
            .as_ref()
            .and_then(KmsCommitWorkerHandle::take_pending_cursor_sidecar);
        let _ = handle_fatal_worker_jobs(fatal_jobs, self, FatalWorkerJobDisposition::Fail)?;
        self.worker_quarantine.cursor_sidecars.extend(fatal_sidecar);
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
        self.abandon_direct_fallback();
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
                self.render_journal
                    .record_worker_queue_residency(ownership.queue_residency_ns);
                self.render_journal
                    .record_worker_submit_wake_lateness(ownership.submit_wake_lateness_ns);
                self.render_journal
                    .record_submission_budget(ownership.submission_budget_ns);
                let _output_generation = ownership.job.output_generation;
                let target = ownership.job.target;
                let cursor = ownership.job.cursor.clone();
                let sidecar_owner = ownership
                    .job
                    .owners
                    .cursor()
                    .filter(|owner| owner.sidecar_id.is_some())
                    .cloned();
                let pacing_frame_id = ownership.job.pacing_frame_id;
                let ready_submit = ownership.job.ready_submit;
                let out_fence = ownership.out_fence.take();
                let direct_validation_key =
                    if matches!(kind, AtomicCommitKind::DirectPrimary { .. })
                        && matches!(ownership.job.test_only, KmsTestOnlyPolicy::Required)
                    {
                        ownership
                            .job
                            .direct_primary_lease
                            .as_ref()
                            .map(|lease| lease.validation_key())
                    } else {
                        None
                    };
                let direct_primary_lease = ownership.job.direct_primary_lease.take();
                if matches!(kind, AtomicCommitKind::DirectPrimary { .. }) {
                    if ownership.job.test_only == KmsTestOnlyPolicy::Required
                        && let Some(duration_ns) = ownership.job.test_only_duration_ns
                    {
                        self.scanout.note_direct_test_only(duration_ns, false);
                    }
                    self.scanout.note_direct_real_submit_attempt(false);
                    self.scanout.note_direct_worker_submission(
                        matches!(ownership.job.test_only, KmsTestOnlyPolicy::Required),
                        submit_started_at,
                        submit_returned_at,
                    );
                }
                let transaction = self
                    .output_transactions
                    .transaction(transaction_id)
                    .ok_or_else(|| io::Error::other("worker success transaction is missing"))?
                    .descriptor()
                    .clone();
                let compatibility_primary = matches!(
                    transaction.planes().primary(),
                    PrimaryPlaneAssignment::CompatibilityFramebuffer { .. }
                );
                let has_out_fence = out_fence.is_some();
                let direct_metadata = if matches!(kind, AtomicCommitKind::DirectPrimary { .. }) {
                    match transaction.content() {
                        OutputTransactionContent::Direct { frame_id, .. } => transaction
                            .obligations()
                            .frame_batch_id()
                            .map(|protocol_batch_id| (frame_id, protocol_batch_id)),
                        _ => None,
                    }
                } else {
                    None
                };
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
                    debug_assert!(!self.scanout.direct_scanout_pending());
                    debug_assert!(direct_primary_lease.is_some());
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
                    let Some((frame_id, protocol_batch_id)) = direct_metadata else {
                        let mut ownership = self
                            .submitted_worker_ownership
                            .pop()
                            .expect("submitted ownership was just retained");
                        ownership.job.direct_primary_lease = Some(direct_primary_lease);
                        ownership.out_fence = out_fence;
                        retain_complete_submitted_ownership(
                            ownership,
                            &mut self.emergency_quarantined_submitted_ownership,
                        );
                        self.quarantine_after_worker_fatal()?;
                        return Err(io::Error::other(
                            "direct worker transaction has no direct frame metadata",
                        )
                        .into());
                    };
                    let submitted = SubmittedDirectPrimary {
                        transaction_id,
                        token,
                        lease: direct_primary_lease,
                        submit_started_at: MonotonicTimestampNs::new(submit_started_at),
                        submit_returned_at: MonotonicTimestampNs::new(submit_returned_at),
                        out_fence,
                        frame_id,
                        protocol_batch_id,
                        target,
                    };
                    if let Err(error) = self.scanout.accept_direct_submitted(submitted) {
                        let SubmittedDirectPrimaryError { error, submitted } = *error;
                        let mut ownership = self
                            .submitted_worker_ownership
                            .pop()
                            .expect("submitted ownership was just retained");
                        ownership.job.direct_primary_lease = Some(submitted.lease);
                        ownership.out_fence = submitted.out_fence;
                        retain_complete_submitted_ownership(
                            ownership,
                            &mut self.emergency_quarantined_submitted_ownership,
                        );
                        self.quarantine_after_worker_fatal()?;
                        return Err(error.into());
                    }
                    debug_assert!(self.scanout.direct_scanout_pending());
                    if let Some(validation_key) = direct_validation_key {
                        self.scanout
                            .record_direct_validation_success(validation_key);
                    }
                    self.server
                        .complete_rendered_frame_callbacks(protocol_batch_id);
                } else if has_out_fence {
                    return Err(io::Error::other(
                        "cursor-only worker submission unexpectedly returned an out-fence",
                    )
                    .into());
                }
                if let Some(sidecar) = sidecar_owner.as_ref() {
                    self.output_transactions
                        .mark_queued(
                            sidecar.transaction.id(),
                            ownership.job.output_generation,
                            MonotonicTimestampNs::new(queued_at),
                        )
                        .map_err(io::Error::other)?;
                }
                let cursor_epoch = if let Some(sidecar) = sidecar_owner.as_ref() {
                    match sidecar.transaction.planes().cursor() {
                        CursorPlaneAssignment::Atomic { desired_epoch, .. } => Some(*desired_epoch),
                        CursorPlaneAssignment::Unchanged | CursorPlaneAssignment::Disabled => None,
                    }
                } else {
                    match kind {
                        AtomicCommitKind::PlaneDelta { cursor_epoch, .. } => Some(cursor_epoch),
                        AtomicCommitKind::CompositedPrimary { .. }
                        | AtomicCommitKind::DirectPrimary { .. } => {
                            match transaction.planes().cursor() {
                                CursorPlaneAssignment::Atomic { desired_epoch, .. } => {
                                    Some(*desired_epoch)
                                }
                                CursorPlaneAssignment::Unchanged
                                | CursorPlaneAssignment::Disabled => None,
                            }
                        }
                    }
                };
                if let Some(cursor_epoch) = cursor_epoch {
                    let (submitted_state, submitted_revision, submitted_capability_key) =
                        if let Some(sidecar) = sidecar_owner.as_ref() {
                            let submitted_state = match &cursor {
                                KmsCursorUpdate::Set(state) => state.clone(),
                                KmsCursorUpdate::Disable => {
                                    let mut hidden = self
                                        .atomic_cursor
                                        .as_ref()
                                        .ok_or_else(|| {
                                            io::Error::other("sidecar submit has no cursor")
                                        })?
                                        .desired()
                                        .clone();
                                    hidden.visible = false;
                                    hidden.framebuffer_id = None;
                                    hidden
                                }
                                KmsCursorUpdate::Unchanged => {
                                    return Err(io::Error::other(
                                        "sidecar submission has no cursor update",
                                    )
                                    .into());
                                }
                            };
                            (submitted_state, sidecar.revision, sidecar.capability_key)
                        } else if matches!(kind, AtomicCommitKind::PlaneDelta { .. }) {
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
                            let submitted_state = match cursor {
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
                            };
                            (submitted_state, queued.revision, queued.capability_key)
                        } else {
                            let submitted_state = match cursor {
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
                            };
                            let submitted_revision = self
                                .atomic_cursor
                                .as_ref()
                                .ok_or_else(|| {
                                    io::Error::other("primary cursor submit has no cursor")
                                })?
                                .revision_for_legacy_epoch(cursor_epoch);
                            (
                                submitted_state,
                                submitted_revision,
                                ownership
                                    .job
                                    .owners
                                    .cursor()
                                    .and_then(|owner| owner.capability_key),
                            )
                        };
                    if let Some(native_cursor) = self.atomic_cursor.as_mut() {
                        if matches!(kind, AtomicCommitKind::PlaneDelta { .. })
                            && sidecar_owner.is_none()
                        {
                            native_cursor.begin_submission_at_revision_with_capability_key(
                                token,
                                submitted_state,
                                cursor_epoch,
                                submitted_revision,
                                submitted_capability_key,
                            );
                        } else {
                            native_cursor.begin_primary_submission_at_revision_with_capability_key(
                                token,
                                submitted_state,
                                cursor_epoch,
                                submitted_revision,
                                submitted_capability_key,
                            );
                        }
                    }
                    self.last_submitted_cursor_epoch = cursor_epoch;
                    if matches!(kind, AtomicCommitKind::PlaneDelta { .. })
                        && sidecar_owner.is_none()
                    {
                        self.cursor_output_arbitration.note_plane_delta_submission();
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
                if let Some(sidecar) = sidecar_owner {
                    self.output_transactions
                        .mark_submitted(
                            sidecar.transaction.id(),
                            token,
                            MonotonicTimestampNs::new(submit_returned_at),
                        )
                        .map_err(io::Error::other)?;
                }
                self.atomic_commit_arbiter
                    .mark_kernel_submitted(token, submit_started_at, submit_returned_at)
                    .map_err(io::Error::other)?;
                if compatibility_primary {
                    self.frame_scheduler
                        .confirm_kernel_submission(token.get(), submit_returned_at)
                        .map_err(io::Error::other)?;
                }
                if !matches!(kind, AtomicCommitKind::PlaneDelta { .. }) {
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
            KmsWorkerEvent::TestRejected { job, error } => {
                if matches!(job.kind, AtomicCommitKind::DirectPrimary { .. })
                    && let Some(duration_ns) = job.test_only_duration_ns
                {
                    self.scanout.note_direct_test_only(duration_ns, true);
                }
                self.fail_queued_worker_job(job, error, WorkerRejectionKind::TestOnly)?;
            }
            KmsWorkerEvent::SubmitRejected { job, error }
            | KmsWorkerEvent::BusyExhausted { job, error } => {
                if matches!(job.kind, AtomicCommitKind::DirectPrimary { .. }) {
                    if job.test_only == KmsTestOnlyPolicy::Required
                        && let Some(duration_ns) = job.test_only_duration_ns
                    {
                        self.scanout.note_direct_test_only(duration_ns, false);
                    }
                    self.scanout.note_direct_real_submit_attempt(true);
                }
                self.fail_queued_worker_job(job, error, WorkerRejectionKind::RealSubmit)?;
            }
            KmsWorkerEvent::BusyDeferred { .. } => {
                if self
                    .atomic_commit_arbiter
                    .worker_queued_kind()
                    .is_some_and(|kind| matches!(kind, AtomicCommitKind::DirectPrimary { .. }))
                {
                    self.scanout.note_direct_real_submit_attempt(false);
                }
            }
            KmsWorkerEvent::SubmitLate {
                bundle: _,
                transaction_id,
                token,
                late_by_ns,
            } => {
                self.pending_proven_deadline_miss
                    .get_or_insert(ProvenDeadlineMiss::AtomicSubmit);
                self.perf.log("native.kms_commit_worker", || {
                    vec![
                        NativePerfField::u64("late_transaction_id", transaction_id.get()),
                        NativePerfField::u64("late_token", token.get()),
                        NativePerfField::u64("late_by_ns", late_by_ns),
                    ]
                });
            }
            KmsWorkerEvent::PageflipTimeout {
                bundle: _,
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
            KmsWorkerEvent::Quiesced {
                returned_jobs,
                returned_sidecar,
            } => {
                for job in returned_jobs {
                    let reason = if self.shutdown.is_shutting_down() {
                        OutputTransactionDropReason::SafeAbandonment
                    } else {
                        OutputTransactionDropReason::SessionSuspended
                    };
                    self.drop_queued_worker_job_with_reason(job, reason)?;
                }
                if let Some(sidecar) = returned_sidecar {
                    self.process_kms_worker_event(KmsWorkerEvent::CursorSidecarReturned {
                        sidecar,
                        reason: CursorSidecarReturnReason::Quiesced,
                    })?;
                }
            }
            KmsWorkerEvent::CursorSidecarReturned { sidecar, reason } => {
                let transaction_id = sidecar.transaction.id();
                if let Some(transaction) = self.output_transactions.transaction(transaction_id)
                    && matches!(
                        transaction.state(),
                        OutputTransactionState::Built
                            | OutputTransactionState::Ready { .. }
                            | OutputTransactionState::Queued { .. }
                    )
                {
                    self.output_transactions
                        .mark_superseded(
                            transaction_id,
                            None,
                            OutputTransactionSupersedeReason::NewerTransaction,
                            MonotonicTimestampNs::new(monotonic_now_ns()?),
                        )
                        .map_err(io::Error::other)?;
                }
                self.perf.log("native.kms_commit_worker", || {
                    vec![
                        NativePerfField::str("event", "cursor_sidecar_returned"),
                        NativePerfField::u64("transaction_id", transaction_id.get()),
                        NativePerfField::str("reason", format!("{reason:?}")),
                    ]
                });
                drop(sidecar);
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
        if self.deferred_worker_pageflip.is_none() {
            self.validate_output_pipeline().map_err(|error| {
                io::Error::other(format!(
                    "worker completion pipeline mismatch: generation={} crtc={} error={error}",
                    self.drm_file_generation, self.target.crtc_id,
                ))
            })?;
        }
        Ok(())
    }
}
