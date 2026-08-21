//! Atomic submit worker thread and lifecycle boundary.

use super::payload::{
    KmsCursorUpdate, KmsPrimaryUpdate, KmsSubmittedOwnership, KmsTestOnlyPolicy, KmsValidationBase,
    ValidationBaseDisposition,
};
#[cfg(test)]
use super::queue::DequeuePause;
use super::queue::{
    AttachablePrimary, AttachablePrimaryPhase, KmsWorkerFatalJob, KmsWorkerForcedShutdown,
    KmsWorkerLifecycle, KmsWorkerPhase, KmsWorkerShutdownSnapshot, RESULT_EVENT_CAPACITY,
    WorkerInFlight, WorkerMetricsSnapshot, WorkerShared, create_eventfd, drain_eventfd,
    notify_eventfd,
};
use super::{
    CursorSidecar, CursorSidecarOfferError, CursorSidecarReturnReason, EstablishedKmsBase,
    KmsCommitAdmissionPermit, KmsCommitBundleIdentity, KmsCommitJob, KmsCursorOwner,
    KmsWorkerAdmissionError, KmsWorkerDispatchModel,
};
use crate::native_output::{
    OutputTransactionId, presentation::transaction::DirectScanoutCandidateKey,
    runtime::AtomicCommitKind,
};
use oblivion_one::native::kms::AtomicCommitSubmitter;
use oblivion_one::native::kms::{AtomicKmsError, AtomicKmsErrorKind, PageFlipToken};
use oblivion_one::native::presentation_deadline::MonotonicTimestampNs;
use std::{
    collections::VecDeque,
    io,
    os::fd::{AsRawFd, OwnedFd},
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[path = "validation.rs"]
mod validation;
use validation::invalidate_queued_dependents;

#[derive(Debug)]
pub(crate) struct KmsWorkerSubmission {
    pub(crate) out_fence: Option<OwnedFd>,
}

pub(crate) trait KmsCommitExecutor: Send + Sync {
    fn test_only(&self, _job: &KmsCommitJob) -> Result<(), KmsWorkerSubmitFailure> {
        Ok(())
    }

    fn submit(&self, job: &KmsCommitJob) -> Result<KmsWorkerSubmission, KmsWorkerSubmitFailure>;
}

#[derive(Debug)]
pub(crate) struct KmsWorkerSubmitFailure {
    pub(crate) error: AtomicKmsError,
}

impl KmsWorkerSubmitFailure {
    pub(crate) fn new(kind: AtomicKmsErrorKind, detail: impl Into<String>) -> Self {
        Self {
            error: AtomicKmsError::new(kind, detail),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KmsWorkerFatalReason {
    Panic,
    EventNotification,
}

#[derive(Debug)]
pub(crate) enum KmsWorkerEvent {
    Submitted {
        ownership: KmsSubmittedOwnership,
    },
    TestRejected {
        job: KmsCommitJob,
        error: AtomicKmsError,
    },
    SubmitRejected {
        job: KmsCommitJob,
        error: AtomicKmsError,
    },
    BusyDeferred {
        bundle: KmsCommitBundleIdentity,
        transaction_id: OutputTransactionId,
        token: PageFlipToken,
        retry: u8,
    },
    BusyExhausted {
        job: KmsCommitJob,
        error: AtomicKmsError,
    },
    PageflipTimeout {
        bundle: KmsCommitBundleIdentity,
        transaction_id: OutputTransactionId,
        token: PageFlipToken,
        detected_at: u64,
    },
    Quiesced {
        returned_jobs: Vec<KmsCommitJob>,
        returned_sidecar: Option<CursorSidecar>,
    },
    CursorSidecarReturned {
        sidecar: CursorSidecar,
        reason: CursorSidecarReturnReason,
    },
    ValidationBaseInvalidated {
        job: KmsCommitJob,
        expected: KmsValidationBase,
        established: Option<Box<EstablishedKmsBase>>,
        reason: ValidationBaseInvalidationReason,
    },
    Fatal {
        reason: KmsWorkerFatalReason,
        uncertain_submit: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ValidationBaseInvalidationReason {
    PredecessorTerminal,
    PresentedRevisionChanged,
    GenerationChanged,
    BundleMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KmsCommitWorkerStartError {
    EventFd,
    Thread,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KmsWorkerAckError {
    NoInFlightCommit,
    TokenMismatch,
    TransactionMismatch,
    GenerationMismatch,
    BundleMismatch,
    CrtcMismatch,
}

#[derive(Debug)]
pub(crate) struct KmsCommitWorkerHandle {
    shared: Arc<WorkerShared>,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl KmsCommitWorkerHandle {
    pub(crate) fn start(
        executor: Arc<dyn KmsCommitExecutor>,
    ) -> Result<Self, KmsCommitWorkerStartError> {
        let result_fd = create_eventfd().map_err(|_| KmsCommitWorkerStartError::EventFd)?;
        let shared = Arc::new(WorkerShared::new(result_fd));
        let thread_shared = Arc::clone(&shared);
        let join = thread::Builder::new()
            .name("typhon-kms-commit".to_string())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_worker(thread_shared.clone(), executor);
                }));
                if result.is_err() {
                    mark_fatal(&thread_shared, KmsWorkerFatalReason::Panic, true);
                }
            })
            .map_err(|_| KmsCommitWorkerStartError::Thread)?;
        Ok(Self {
            shared,
            join: Mutex::new(Some(join)),
        })
    }
    pub(crate) fn start_atomic(
        submitter: AtomicCommitSubmitter,
    ) -> Result<Self, KmsCommitWorkerStartError> {
        Self::start(Arc::new(AtomicKmsWorkerExecutor { submitter }))
    }
    pub(crate) fn try_reserve_admission(
        &self,
        _kind: AtomicCommitKind,
    ) -> Result<KmsCommitAdmissionPermit, KmsWorkerAdmissionError> {
        self.shared.try_reserve()
    }
    pub(crate) fn try_reserve_admission_slot(
        &self,
    ) -> Result<KmsCommitAdmissionPermit, KmsWorkerAdmissionError> {
        self.shared.try_reserve()
    }
    pub(crate) fn event_fd(&self) -> i32 {
        self.shared.result_fd.as_raw_fd()
    }

    pub(crate) fn fatal_reason(&self) -> Option<KmsWorkerFatalReason> {
        match self
            .shared
            .fatal_reason_code
            .load(std::sync::atomic::Ordering::Acquire)
        {
            1 => Some(KmsWorkerFatalReason::Panic),
            2 => Some(KmsWorkerFatalReason::EventNotification),
            _ => None,
        }
    }

    pub(crate) fn mark_admission_fatal(&self) {
        mark_fatal(&self.shared, KmsWorkerFatalReason::EventNotification, false);
    }

    pub(crate) fn admission_available(&self) -> bool {
        let state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !matches!(state.lifecycle, KmsWorkerLifecycle::Running) {
            return false;
        }
        let occupied = state.queued.len()
            + state.reserved
            + usize::from(state.executing)
            + usize::from(state.inflight.is_some());
        let active = usize::from(state.executing || state.inflight.is_some());
        occupied < active.saturating_add(super::queue::QUEUED_JOB_CAPACITY)
    }

    pub(crate) fn direct_content_keys(
        &self,
    ) -> (
        Option<crate::native_output::DirectScanoutCandidateKey>,
        Option<crate::native_output::DirectScanoutCandidateKey>,
        Option<crate::native_output::DirectScanoutCandidateKey>,
    ) {
        self.shared.direct_content_keys()
    }

    #[cfg(test)]
    pub(crate) fn pause_after_dequeue_for_test(&self) -> Arc<DequeuePause> {
        self.shared.pause_after_dequeue_for_test()
    }

    #[cfg(test)]
    pub(crate) fn pause_collecting_sidecar_for_test(&self) -> Arc<DequeuePause> {
        self.shared.pause_collecting_for_test()
    }

    #[cfg(test)]
    pub(crate) fn pause_after_freeze_for_test(&self) -> Arc<DequeuePause> {
        self.shared.pause_frozen_for_test()
    }

    pub(crate) fn offer_cursor_sidecar(
        &self,
        sidecar: CursorSidecar,
    ) -> Result<Option<CursorSidecar>, CursorSidecarOfferError> {
        self.shared.offer_cursor_sidecar(sidecar)
    }

    #[cfg(test)]
    pub(crate) fn pending_cursor_sidecar_id(
        &self,
    ) -> Option<crate::native_output::presentation::plane::CursorSidecarId> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .cursor_sidecar
            .pending()
            .map(|sidecar| sidecar.id)
    }

    pub(crate) fn try_reserve_direct_admission(
        &self,
        candidate_key: crate::native_output::DirectScanoutCandidateKey,
    ) -> Result<crate::native_output::kms_worker::KmsCommitAdmissionPermit, KmsWorkerAdmissionError>
    {
        self.shared.try_reserve_direct(candidate_key)
    }

    pub(crate) fn drain_eventfd(&self) -> io::Result<()> {
        drain_eventfd(&self.shared.result_fd)
    }

    pub(crate) fn drain_events(&self) -> Vec<KmsWorkerEvent> {
        let mut results = self
            .shared
            .results
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let events = results.drain(..).collect();
        self.shared.result_space.notify_all();
        events
    }

    pub(crate) fn take_fatal_jobs(&self) -> Vec<KmsWorkerFatalJob> {
        std::mem::take(
            &mut *self
                .shared
                .fatal_jobs
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }

    pub(crate) fn take_pending_cursor_sidecar(&self) -> Option<CursorSidecar> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .cursor_sidecar
            .take()
    }

    pub(crate) fn take_due_independent_cursor_sidecar(
        &self,
        output_generation: u64,
        crtc_id: u32,
        target: oblivion_one::native::presentation_deadline::PresentationTarget,
    ) -> Option<CursorSidecar> {
        let sidecar = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .cursor_sidecar
            .take_independent_due(output_generation, crtc_id, target);
        if sidecar.is_some() {
            self.record_cursor_sidecar_promoted();
        }
        sidecar
    }

    pub(crate) fn metrics_snapshot(&self) -> WorkerMetricsSnapshot {
        self.shared.metrics.snapshot()
    }

    pub(crate) fn record_submit_ack_delay(&self, delay_ns: u64) {
        self.shared.metrics.timing.record_submit_ack_delay(delay_ns);
    }

    pub(crate) fn record_cursor_sidecar_promoted(&self) {
        self.shared
            .metrics
            .cursor_sidecars_promoted
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn record_result_mismatch(&self) {
        self.shared
            .metrics
            .result_mismatches
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn record_main_thread_stall(&self) {
        self.shared
            .metrics
            .main_thread_stalls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn record_driver_timeout_suspicion(&self) {
        self.shared
            .metrics
            .driver_timeout_suspicions
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn record_scheduler_queued_cancellation(&self) {
        self.shared
            .metrics
            .scheduler_queued_cancellations
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn record_scheduler_cancel_mismatch(&self) {
        self.shared
            .metrics
            .scheduler_cancel_mismatches
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn record_cursor_worker_epoch_mismatch(&self) {
        self.shared
            .metrics
            .cursor_worker_epoch_mismatches
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn record_cursor_worker_submit_confirmed(&self) {
        self.shared
            .metrics
            .cursor_worker_submits_confirmed
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn record_cursor_worker_queued(&self) {
        self.shared
            .metrics
            .cursor_worker_jobs_queued
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn record_cursor_worker_rejection_retryable(&self) {
        self.shared
            .metrics
            .cursor_worker_rejections_retryable
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn record_cursor_worker_rejection_fallback(&self) {
        self.shared
            .metrics
            .cursor_worker_rejections_fallback
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn record_cursor_worker_arbitration_consumed(&self) {
        self.shared
            .metrics
            .cursor_worker_arbitration_consumed
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn record_worker_pacing_submit_confirmed(&self) {
        self.shared
            .metrics
            .worker_pacing_submits_confirmed
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn record_worker_pacing_pre_submit_rejection(&self) {
        self.shared
            .metrics
            .worker_pacing_pre_submit_rejections
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn record_shutdown_queued_job_settled(&self) {
        self.shared
            .metrics
            .shutdown_queued_jobs_settled
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn record_unnotified_fatal_health_check(&self) {
        self.shared
            .metrics
            .unnotified_fatal_health_checks
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn record_runtime_queue_state(&self, queue_depth: usize, kernel_inflight: bool) {
        let depth = u64::try_from(queue_depth).unwrap_or(u64::MAX);
        self.shared
            .metrics
            .runtime_queue_depth
            .store(depth, std::sync::atomic::Ordering::Relaxed);
        self.shared
            .metrics
            .runtime_queue_depth_max
            .fetch_max(depth, std::sync::atomic::Ordering::Relaxed);
        self.shared.metrics.runtime_kernel_inflight.store(
            u64::from(kernel_inflight),
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    pub(crate) fn request_quiesce(&self) {
        self.shared.request_quiesce();
    }

    pub(crate) fn begin_shutdown_quiesce(
        &self,
    ) -> Result<KmsWorkerShutdownSnapshot, KmsWorkerAdmissionError> {
        self.shared.begin_shutdown_quiesce()
    }

    pub(crate) fn force_shutdown_abandon(
        &self,
    ) -> Result<KmsWorkerForcedShutdown, KmsWorkerAdmissionError> {
        self.shared.force_shutdown_abandon()
    }

    pub(crate) fn queue_depth(&self) -> usize {
        let state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.queued.len()
    }

    pub(crate) fn inflight(&self) -> bool {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .inflight
            .is_some()
    }

    pub(crate) fn submission_active(&self) -> bool {
        let state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.executing || state.inflight.is_some()
    }

    pub(crate) fn join(&self) -> Result<(), KmsCommitWorkerStartError> {
        let join = self
            .join
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(join) = join {
            let started = Instant::now();
            join.join().map_err(|_| KmsCommitWorkerStartError::Thread)?;
            self.shared.metrics.join_ns_total.fetch_add(
                u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX),
                std::sync::atomic::Ordering::Relaxed,
            );
        }
        Ok(())
    }
}

#[derive(Debug)]
struct ExecutingDirectCandidateGuard {
    shared: Arc<WorkerShared>,
    candidate: Option<DirectScanoutCandidateKey>,
    transferred: bool,
}

impl ExecutingDirectCandidateGuard {
    fn from_dequeued(
        shared: &Arc<WorkerShared>,
        candidate: Option<DirectScanoutCandidateKey>,
    ) -> Self {
        Self {
            shared: Arc::clone(shared),
            candidate,
            transferred: false,
        }
    }

    fn transfer_to_inflight(&mut self, job: &KmsCommitJob, submit_returned_at_ns: u64) {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.inflight = Some(WorkerInFlight {
            bundle: job.identity(),
            token: job.token,
            transaction_id: job.transaction_id,
            output_generation: job.output_generation,
            kind: job.kind,
            direct_content_key: self.candidate,
            submit_returned_at_ns,
        });
        state.established_base = Some(EstablishedKmsBase::Pending(job.identity()));
        state.executing = false;
        state.executing_direct_content_key = None;
        state.executing_primary_transaction_id = None;
        state.executing_bundle_identity = None;
        state.executing_primary = None;
        state.phase = KmsWorkerPhase::KernelInFlight;
        self.transferred = true;
    }
}

impl Drop for ExecutingDirectCandidateGuard {
    fn drop(&mut self) {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.executing = false;
        state.executing_primary_transaction_id = None;
        state.executing_bundle_identity = None;
        state.executing_primary = None;
        if state.inflight.is_none() {
            state.phase = KmsWorkerPhase::Idle;
        }
        if !self.transferred && state.executing_direct_content_key == self.candidate {
            state.executing_direct_content_key = None;
        }
    }
}

#[derive(Debug)]
pub(crate) struct AtomicKmsWorkerExecutor {
    pub(super) submitter: AtomicCommitSubmitter,
}

#[path = "presentation_executor.rs"]
mod presentation_executor;

impl Drop for KmsCommitWorkerHandle {
    fn drop(&mut self) {
        self.request_quiesce();
        let _ = self.join();
    }
}

fn set_worker_phase(shared: &WorkerShared, phase: KmsWorkerPhase) {
    let mut state = shared
        .state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.phase = phase;
    if let Some(primary) = state.executing_primary.as_mut() {
        match phase {
            KmsWorkerPhase::DequeuedWaitingPredecessor => {
                primary.phase = AttachablePrimaryPhase::DequeuedWaitingPredecessor;
            }
            KmsWorkerPhase::CollectingSidecar => {
                primary.phase = AttachablePrimaryPhase::CollectingSidecar;
            }
            KmsWorkerPhase::FrozenForValidation
            | KmsWorkerPhase::TestOnly
            | KmsWorkerPhase::SubmitIoctl
            | KmsWorkerPhase::KernelInFlight
            | KmsWorkerPhase::Idle => state.executing_primary = None,
        }
    }
}

fn collect_cursor_sidecar_before_freeze(
    shared: &Arc<WorkerShared>,
    job: &mut KmsCommitJob,
) -> (bool, Option<CursorSidecar>) {
    set_worker_phase(shared, KmsWorkerPhase::CollectingSidecar);
    #[cfg(test)]
    if let Some(pause) = shared.take_collecting_pause_for_test() {
        pause.pause();
    }

    let (running, returned_sidecar) = {
        let mut state = shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !matches!(state.lifecycle, KmsWorkerLifecycle::Running) {
            return (false, None);
        }
        let primary_id = job.owners.primary_transaction_id();
        let eligible_job = matches!(job.primary, KmsPrimaryUpdate::Framebuffer { .. })
            && (primary_id.is_some() || matches!(job.cursor, KmsCursorUpdate::Unchanged));
        let claimed = eligible_job
            .then(|| {
                state.cursor_sidecar.claim_for(
                    job.output_generation,
                    job.crtc_id,
                    job.target,
                    primary_id,
                )
            })
            .flatten();
        let current_cursor_revision = job.owners.cursor().map(|owner| owner.revision);
        let (claimed, rejected) = match claimed {
            Some(sidecar)
                if sidecar.validation_base == job.validation_base
                    && current_cursor_revision
                        .is_none_or(|revision| sidecar.revision.strictly_newer_than(revision)) =>
            {
                (Some(sidecar), None)
            }
            Some(sidecar) => (None, Some(sidecar)),
            None => (None, None),
        };
        let returned = if claimed.is_none() {
            rejected.or_else(|| {
                primary_id
                    .and_then(|primary_id| state.cursor_sidecar.take_must_bundle_with(primary_id))
            })
        } else {
            None
        };
        if let Some(sidecar) = claimed {
            shared
                .metrics
                .cursor_sidecars_claimed
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            attach_sidecar(job, sidecar);
            let identity = job.identity();
            state.executing_bundle_identity = Some(identity);
            if let Some(primary) = state.executing_primary.as_mut() {
                primary.bundle_identity = identity;
            }
        }
        state.phase = KmsWorkerPhase::FrozenForValidation;
        state.executing_primary = None;
        (true, returned)
    };
    #[cfg(test)]
    if running && let Some(pause) = shared.take_frozen_pause_for_test() {
        pause.pause();
    }
    (running, returned_sidecar)
}

fn attach_sidecar(job: &mut KmsCommitJob, sidecar: CursorSidecar) {
    job.cursor = match &sidecar.assignment {
        crate::native_output::CursorPlaneAssignment::Atomic {
            state: Some(state), ..
        } => KmsCursorUpdate::Set(state.clone()),
        crate::native_output::CursorPlaneAssignment::Atomic { state: None, .. }
        | crate::native_output::CursorPlaneAssignment::Disabled => KmsCursorUpdate::Disable,
        crate::native_output::CursorPlaneAssignment::Unchanged => KmsCursorUpdate::Unchanged,
    };
    job.cursor_delivery = sidecar.cursor_delivery;
    job.cursor_pin = sidecar.lease;
    job.owners.replace_cursor(KmsCursorOwner {
        transaction: sidecar.transaction,
        sidecar_id: Some(sidecar.id),
        revision: sidecar.revision,
        capability_key: sidecar.capability_key,
    });
    job.test_policy.cursor = sidecar.test_policy;
}

fn publish_terminal_sidecar_return(
    shared: &Arc<WorkerShared>,
    primary_transaction_id: Option<OutputTransactionId>,
    reason: CursorSidecarReturnReason,
) -> bool {
    let Some(primary_transaction_id) = primary_transaction_id else {
        return true;
    };
    let sidecar = shared
        .state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .cursor_sidecar
        .take_must_bundle_with(primary_transaction_id);
    sidecar.is_none_or(|sidecar| {
        publish_event(
            shared,
            KmsWorkerEvent::CursorSidecarReturned { sidecar, reason },
        )
    })
}

fn run_worker(shared: Arc<WorkerShared>, executor: Arc<dyn KmsCommitExecutor>) {
    let mut dispatch_model = KmsWorkerDispatchModel::default();
    loop {
        let Some(ExecutingKmsJob {
            mut job,
            direct_candidate,
        }) = take_next_job(&shared)
        else {
            return;
        };
        #[cfg(test)]
        if let Some(pause) = shared.take_dequeue_pause_for_test() {
            pause.pause();
        }
        let mut executing = ExecutingDirectCandidateGuard::from_dequeued(&shared, direct_candidate);
        let now_ns = monotonic_now_ns();
        let planned_worker_wake_at = job.submit_window.worker_wake_at_ns();
        let wait_armed = planned_worker_wake_at > now_ns;
        let actual_worker_wait_returned_at = if wait_armed {
            let Some(returned_at) = wait_until_or_quiesce(&shared, planned_worker_wake_at) else {
                drop(executing);
                quiesce_with_jobs(&shared, vec![job]);
                return;
            };
            returned_at
        } else {
            now_ns
        };
        if wait_armed && actual_worker_wait_returned_at > planned_worker_wake_at {
            shared
                .metrics
                .late_wakeups
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        let pre_submit_started_at = actual_worker_wait_returned_at;
        let (running, returned_sidecar) = collect_cursor_sidecar_before_freeze(&shared, &mut job);
        if let Some(sidecar) = returned_sidecar
            && !publish_event(
                &shared,
                KmsWorkerEvent::CursorSidecarReturned {
                    sidecar,
                    reason: CursorSidecarReturnReason::RequiredPrimaryPassedFreeze,
                },
            )
        {
            drop(executing);
            quiesce_with_jobs(&shared, vec![job]);
            return;
        }
        if !running {
            drop(executing);
            quiesce_with_jobs(&shared, vec![job]);
            return;
        }

        let mut retries = 0u8;
        if matches!(job.test_policy.effective(), KmsTestOnlyPolicy::Required) {
            set_worker_phase(&shared, KmsWorkerPhase::TestOnly);
            let test = {
                let _submit_gate = shared
                    .submit_gate
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let state = shared
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if !matches!(state.lifecycle, KmsWorkerLifecycle::Running) {
                    None
                } else {
                    drop(state);
                    let test_started_ns = monotonic_now_ns();
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        executor.test_only(&job)
                    }));
                    let test_duration_ns = monotonic_now_ns().saturating_sub(test_started_ns);
                    Some((result, test_duration_ns))
                }
            };
            let Some(test) = test else {
                quiesce_with_jobs(&shared, vec![job]);
                return;
            };
            let (test_result, test_duration_ns) = test;
            shared.metrics.timing.record_test_only(test_duration_ns);
            match (test_result, test_duration_ns) {
                (Ok(Ok(())), duration_ns) => {
                    job.test_only_duration_ns = Some(duration_ns);
                }
                (Ok(Err(failure)), duration_ns) => {
                    job.test_only_duration_ns = Some(duration_ns);
                    shared
                        .metrics
                        .jobs_rejected
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if !publish_terminal_sidecar_return(
                        &shared,
                        job.owners.primary_transaction_id(),
                        CursorSidecarReturnReason::RequiredPrimaryTerminal,
                    ) {
                        return;
                    }
                    invalidate_queued_dependents(
                        &shared,
                        job.identity(),
                        ValidationBaseInvalidationReason::PredecessorTerminal,
                    );
                    if !publish_event(
                        &shared,
                        KmsWorkerEvent::TestRejected {
                            job,
                            error: failure.error,
                        },
                    ) {
                        return;
                    }
                    continue;
                }
                (Err(_), duration_ns) => {
                    job.test_only_duration_ns = Some(duration_ns);
                    retain_fatal_job(&shared, job, false);
                    mark_fatal(&shared, KmsWorkerFatalReason::Panic, false);
                    return;
                }
            }
        }
        let pre_submit_completed_at = monotonic_now_ns();
        loop {
            set_worker_phase(&shared, KmsWorkerPhase::SubmitIoctl);
            let submission = {
                let _submit_gate = shared
                    .submit_gate
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let state = shared
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if !matches!(state.lifecycle, KmsWorkerLifecycle::Running) {
                    None
                } else {
                    drop(state);
                    let submit_started_at = monotonic_now_ns();
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        executor.submit(&job)
                    }));
                    Some((submit_started_at, result))
                }
            };
            let Some((submit_started_at, submission)) = submission else {
                quiesce_with_jobs(&shared, vec![job]);
                return;
            };
            match submission {
                Ok(Ok(submission)) => {
                    let submit_returned_at = monotonic_now_ns();
                    executing.transfer_to_inflight(&job, submit_returned_at);
                    if let KmsPrimaryUpdate::Framebuffer { in_fence, .. } = &mut job.primary {
                        // The ioctl has consumed the input-fence contract. The
                        // fence must not remain owned while waiting for the pageflip.
                        let _ = in_fence.take();
                    }
                    let submit_duration_ns = submit_returned_at.saturating_sub(submit_started_at);
                    shared
                        .metrics
                        .jobs_submitted
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    shared
                        .metrics
                        .submit_duration_ns_total
                        .fetch_add(submit_duration_ns, std::sync::atomic::Ordering::Relaxed);
                    shared
                        .metrics
                        .submit_duration_ns_max
                        .fetch_max(submit_duration_ns, std::sync::atomic::Ordering::Relaxed);
                    let queue_wait_ns = submit_started_at.saturating_sub(job.queued_at.get());
                    let submit_wake_lateness_ns = wait_armed
                        .then(|| {
                            actual_worker_wait_returned_at.saturating_sub(planned_worker_wake_at)
                        })
                        .unwrap_or(0);
                    let pre_submit_duration_ns =
                        pre_submit_completed_at.saturating_sub(pre_submit_started_at);
                    let dispatch_duration_ns =
                        submit_returned_at.saturating_sub(actual_worker_wait_returned_at);
                    dispatch_model.record(
                        submit_wake_lateness_ns,
                        pre_submit_duration_ns,
                        submit_duration_ns,
                    );
                    let dispatch_budget: super::KmsWorkerDispatchBudget = dispatch_model.budget();
                    let submission_budget_ns = dispatch_budget.dispatch_budget_ns;
                    shared.metrics.timing.record_submission(
                        planned_worker_wake_at,
                        actual_worker_wait_returned_at,
                        job.submit_window.commit_complete_deadline_ns(),
                        job.target.presentation_time.get(),
                        submit_started_at,
                        submit_returned_at,
                        queue_wait_ns,
                        pre_submit_duration_ns,
                        submit_duration_ns,
                        dispatch_duration_ns,
                        submission_budget_ns,
                    );
                    shared
                        .metrics
                        .queue_wait_ns_total
                        .fetch_add(queue_wait_ns, std::sync::atomic::Ordering::Relaxed);
                    shared
                        .metrics
                        .queue_wait_ns_max
                        .fetch_max(queue_wait_ns, std::sync::atomic::Ordering::Relaxed);
                    let transaction_id = job.transaction_id;
                    let token = job.token;
                    let event = KmsWorkerEvent::Submitted {
                        ownership: KmsSubmittedOwnership {
                            job,
                            out_fence: submission.out_fence,
                            planned_worker_wake_at: MonotonicTimestampNs::new(
                                planned_worker_wake_at,
                            ),
                            actual_worker_wait_returned_at: MonotonicTimestampNs::new(
                                actual_worker_wait_returned_at,
                            ),
                            submit_started_at: MonotonicTimestampNs::new(submit_started_at),
                            submit_returned_at: MonotonicTimestampNs::new(submit_returned_at),
                            queue_residency_ns: queue_wait_ns,
                            submit_wake_lateness_ns,
                            pre_submit_duration_ns,
                            ioctl_duration_ns: submit_duration_ns,
                            dispatch_duration_ns,
                            submission_budget_ns,
                        },
                    };
                    if !publish_event(&shared, event) {
                        return;
                    }
                    if !wait_for_pageflip_or_quiesce(&shared, transaction_id, token) {
                        return;
                    }
                    break;
                }
                Ok(Err(failure)) if failure.error.kind == AtomicKmsErrorKind::Busy => {
                    shared
                        .metrics
                        .busy_deferrals
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if retries >= 2 {
                        if !publish_terminal_sidecar_return(
                            &shared,
                            job.owners.primary_transaction_id(),
                            CursorSidecarReturnReason::RequiredPrimaryTerminal,
                        ) {
                            return;
                        }
                        invalidate_queued_dependents(
                            &shared,
                            job.identity(),
                            ValidationBaseInvalidationReason::PredecessorTerminal,
                        );
                        if !publish_event(
                            &shared,
                            KmsWorkerEvent::BusyExhausted {
                                job,
                                error: failure.error,
                            },
                        ) {
                            return;
                        }
                        shared
                            .metrics
                            .busy_exhausted
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        shared
                            .metrics
                            .jobs_rejected
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        break;
                    }
                    retries += 1;
                    if matches!(
                        &job.primary,
                        KmsPrimaryUpdate::Framebuffer {
                            in_fence: Some(_),
                            ..
                        }
                    ) {
                        shared
                            .metrics
                            .input_fence_retry_attempts
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        shared
                            .metrics
                            .input_fence_retry_preserved
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    shared
                        .metrics
                        .busy_retries
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if !publish_event(
                        &shared,
                        KmsWorkerEvent::BusyDeferred {
                            bundle: job.identity(),
                            transaction_id: job.transaction_id,
                            token: job.token,
                            retry: retries,
                        },
                    ) {
                        retain_fatal_job(&shared, job, false);
                        return;
                    }
                    let delay = if retries == 1 {
                        Duration::from_micros(100)
                    } else {
                        Duration::from_micros(400)
                    };
                    let deadline = monotonic_now_ns()
                        .saturating_add(u64::try_from(delay.as_nanos()).unwrap_or(u64::MAX));
                    if wait_until_or_quiesce(&shared, deadline).is_none() {
                        quiesce_with_jobs(&shared, vec![job]);
                        return;
                    }
                }
                Ok(Err(failure)) => {
                    shared
                        .metrics
                        .jobs_rejected
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if !publish_terminal_sidecar_return(
                        &shared,
                        job.owners.primary_transaction_id(),
                        CursorSidecarReturnReason::RequiredPrimaryTerminal,
                    ) {
                        return;
                    }
                    invalidate_queued_dependents(
                        &shared,
                        job.identity(),
                        ValidationBaseInvalidationReason::PredecessorTerminal,
                    );
                    if !publish_event(
                        &shared,
                        KmsWorkerEvent::SubmitRejected {
                            job,
                            error: failure.error,
                        },
                    ) {
                        return;
                    }
                    break;
                }
                Err(_) => {
                    retain_fatal_job(&shared, job, true);
                    mark_fatal(&shared, KmsWorkerFatalReason::Panic, true);
                    return;
                }
            }
        }
    }
}

#[derive(Debug)]
struct ExecutingKmsJob {
    job: KmsCommitJob,
    direct_candidate: Option<DirectScanoutCandidateKey>,
}

fn take_next_job(shared: &Arc<WorkerShared>) -> Option<ExecutingKmsJob> {
    let mut state = shared
        .state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    loop {
        if matches!(
            state.lifecycle,
            KmsWorkerLifecycle::Fatal | KmsWorkerLifecycle::Stopped
        ) {
            return None;
        }
        if matches!(
            state.lifecycle,
            KmsWorkerLifecycle::Quiescing
                | KmsWorkerLifecycle::ShutdownQuiescing
                | KmsWorkerLifecycle::ShutdownAbandoning
        ) {
            let returned_jobs = state.queued.drain(..).collect();
            let returned_sidecar = state.cursor_sidecar.take();
            state.lifecycle = KmsWorkerLifecycle::Stopped;
            drop(state);
            publish_event(
                shared,
                KmsWorkerEvent::Quiesced {
                    returned_jobs,
                    returned_sidecar,
                },
            );
            return None;
        }
        if state.inflight.is_none()
            && let Some(front) = state.queued.front()
        {
            match WorkerShared::validation_base_disposition(&state, front.validation_base) {
                ValidationBaseDisposition::Wait => {
                    state = shared
                        .work_wakeup
                        .wait(state)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    continue;
                }
                ValidationBaseDisposition::Invalidated => {
                    let job = state.queued.pop_front().expect("front job still queued");
                    let expected = job.validation_base;
                    let established = state.established_base;
                    let reason = match expected {
                        KmsValidationBase::Presented { .. } => {
                            ValidationBaseInvalidationReason::PresentedRevisionChanged
                        }
                        KmsValidationBase::Predecessor(_) => {
                            ValidationBaseInvalidationReason::PredecessorTerminal
                        }
                    };
                    drop(state);
                    if !publish_event(
                        shared,
                        KmsWorkerEvent::ValidationBaseInvalidated {
                            job,
                            expected,
                            established: established.map(Box::new),
                            reason,
                        },
                    ) {
                        return None;
                    }
                    state = shared
                        .state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    continue;
                }
                ValidationBaseDisposition::Ready => {}
            }
            let job = state.queued.pop_front().expect("front job still queued");
            let direct_candidate = job.direct_primary_lease.as_ref().map(|lease| lease.key());
            debug_assert!(!state.executing);
            state.executing = true;
            state.executing_direct_content_key = direct_candidate;
            state.executing_primary_transaction_id = job.owners.primary_transaction_id();
            state.executing_bundle_identity = Some(job.identity());
            state.executing_primary = (job.kind.is_primary()
                && matches!(job.primary, KmsPrimaryUpdate::Framebuffer { .. }))
            .then(|| {
                Some(AttachablePrimary {
                    transaction_id: job.owners.primary_transaction_id()?,
                    bundle_identity: job.identity(),
                    validation_base: job.validation_base,
                    output_generation: job.output_generation,
                    crtc_id: job.crtc_id,
                    target: job.target,
                    phase: AttachablePrimaryPhase::DequeuedWaitingPredecessor,
                })
            })
            .flatten();
            state.phase = KmsWorkerPhase::DequeuedWaitingPredecessor;
            return Some(ExecutingKmsJob {
                job,
                direct_candidate,
            });
        }
        state = shared
            .work_wakeup
            .wait(state)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    }
}

fn wait_for_pageflip_or_quiesce(
    shared: &Arc<WorkerShared>,
    transaction_id: OutputTransactionId,
    token: PageFlipToken,
) -> bool {
    let mut state = shared
        .state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut timeout_reported = false;
    loop {
        if matches!(state.lifecycle, KmsWorkerLifecycle::Quiescing)
            || matches!(
                state.lifecycle,
                KmsWorkerLifecycle::ShutdownQuiescing | KmsWorkerLifecycle::ShutdownAbandoning
            ) && state.inflight.is_none()
        {
            let returned_jobs = state.queued.drain(..).collect();
            let returned_sidecar = state.cursor_sidecar.take();
            state.lifecycle = KmsWorkerLifecycle::Stopped;
            drop(state);
            publish_event(
                shared,
                KmsWorkerEvent::Quiesced {
                    returned_jobs,
                    returned_sidecar,
                },
            );
            return false;
        }
        if state.inflight.is_none() {
            return true;
        }
        let (next, timeout) = shared
            .work_wakeup
            .wait_timeout(state, Duration::from_secs(1))
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state = next;
        let timed_out_bundle = (timeout.timed_out() && !timeout_reported)
            .then(|| state.inflight.filter(|inflight| inflight.token == token))
            .flatten()
            .map(|inflight| inflight.bundle);
        if let Some(bundle) = timed_out_bundle {
            timeout_reported = true;
            drop(state);
            if !publish_event(
                shared,
                KmsWorkerEvent::PageflipTimeout {
                    bundle,
                    transaction_id,
                    token,
                    detected_at: monotonic_now_ns(),
                },
            ) {
                return false;
            }
            shared
                .metrics
                .pageflip_timeouts
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            state = shared
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }
}

fn wait_until_or_quiesce(shared: &Arc<WorkerShared>, deadline_ns: u64) -> Option<u64> {
    let mut state = shared
        .state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    loop {
        if matches!(
            state.lifecycle,
            KmsWorkerLifecycle::Quiescing
                | KmsWorkerLifecycle::ShutdownQuiescing
                | KmsWorkerLifecycle::ShutdownAbandoning
        ) {
            return None;
        }
        let now = monotonic_now_ns();
        if now >= deadline_ns {
            return Some(now);
        }
        let wait = Duration::from_nanos(deadline_ns - now);
        let (next, _) = shared
            .work_wakeup
            .wait_timeout(state, wait)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state = next;
    }
}

fn quiesce_with_jobs(shared: &Arc<WorkerShared>, mut returned_jobs: Vec<KmsCommitJob>) {
    let mut state = shared
        .state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    returned_jobs.extend(state.queued.drain(..));
    let returned_sidecar = state.cursor_sidecar.take();
    state.lifecycle = KmsWorkerLifecycle::Stopped;
    drop(state);
    publish_event(
        shared,
        KmsWorkerEvent::Quiesced {
            returned_jobs,
            returned_sidecar,
        },
    );
}

fn publish_event(shared: &Arc<WorkerShared>, event: KmsWorkerEvent) -> bool {
    let uncertain_submit = matches!(&event, KmsWorkerEvent::Submitted { .. });
    let mut results = shared
        .results
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    while results.len() >= RESULT_EVENT_CAPACITY {
        results = shared
            .result_space
            .wait(results)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    }
    results.push_back(event);
    drop(results);
    if notify_eventfd(&shared.result_fd).is_ok() {
        return true;
    }
    shared
        .metrics
        .eventfd_notification_failures
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    mark_fatal(
        shared,
        KmsWorkerFatalReason::EventNotification,
        uncertain_submit,
    );
    false
}

fn retain_fatal_job(shared: &Arc<WorkerShared>, job: KmsCommitJob, uncertain_submit: bool) {
    shared
        .fatal_jobs
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(KmsWorkerFatalJob {
            job,
            uncertain_submit,
        });
}

fn mark_fatal(shared: &Arc<WorkerShared>, reason: KmsWorkerFatalReason, uncertain_submit: bool) {
    let mut state = shared
        .state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if matches!(state.lifecycle, KmsWorkerLifecycle::Fatal) {
        return;
    }
    let queued_jobs = state.queued.drain(..).collect::<Vec<_>>();
    state.lifecycle = KmsWorkerLifecycle::Fatal;
    drop(state);
    if !queued_jobs.is_empty() {
        let mut fatal_jobs = shared
            .fatal_jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        fatal_jobs.extend(queued_jobs.into_iter().map(|job| KmsWorkerFatalJob {
            job,
            uncertain_submit: false,
        }));
    }
    shared.fatal_reason_code.store(
        match reason {
            KmsWorkerFatalReason::Panic => 1,
            KmsWorkerFatalReason::EventNotification => 2,
        },
        std::sync::atomic::Ordering::Release,
    );
    shared
        .metrics
        .fatal_events
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut results = shared
        .results
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    while results.len() >= RESULT_EVENT_CAPACITY {
        results = shared
            .result_space
            .wait(results)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    }
    results.push_back(KmsWorkerEvent::Fatal {
        reason,
        uncertain_submit,
    });
    drop(results);
    if let Err(error) = notify_eventfd(&shared.result_fd) {
        // The shared fatal state is authoritative when the notification fd
        // itself is unavailable; this write is only a best-effort wake.
        eprintln!("native KMS worker: fatal-state notification failed: {error}");
    }
}

fn monotonic_now_ns() -> u64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) } < 0 {
        return u64::MAX;
    }
    (time.tv_sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(time.tv_nsec as u64)
}
