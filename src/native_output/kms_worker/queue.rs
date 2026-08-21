//! Bounded admission and result queues for the Atomic submit worker.

use super::{
    CursorSidecar, CursorSidecarMailbox, EstablishedKmsBase, KmsCommitBundleIdentity, KmsCommitJob,
    KmsValidationBase, KmsWorkerEvent, ValidationBaseDisposition, WorkerTimingMetrics,
    WorkerTimingSnapshot, validation_base_ready,
};
use crate::native_output::DirectScanoutCandidateKey;
use oblivion_one::native::presentation_deadline::PresentationTarget;
use std::{
    collections::{HashSet, VecDeque},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    sync::{
        Arc, Condvar, Mutex, TryLockError,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

pub(crate) const QUEUED_JOB_CAPACITY: usize = 1;
pub(crate) const RESULT_EVENT_CAPACITY: usize = 8;

#[derive(Debug, Default)]
pub(crate) struct WorkerMetrics {
    pub(crate) timing: WorkerTimingMetrics,
    pub(crate) jobs_enqueued: AtomicU64,
    pub(crate) jobs_submitted: AtomicU64,
    pub(crate) jobs_rejected: AtomicU64,
    pub(crate) queue_full: AtomicU64,
    pub(crate) admission_contention: AtomicU64,
    pub(crate) busy_deferrals: AtomicU64,
    pub(crate) busy_retries: AtomicU64,
    pub(crate) busy_exhausted: AtomicU64,
    pub(crate) late_wakeups: AtomicU64,
    pub(crate) submit_duration_ns_total: AtomicU64,
    pub(crate) submit_duration_ns_max: AtomicU64,
    pub(crate) queue_wait_ns_total: AtomicU64,
    pub(crate) queue_wait_ns_max: AtomicU64,
    pub(crate) pageflip_timeouts: AtomicU64,
    pub(crate) main_thread_stalls: AtomicU64,
    pub(crate) driver_timeout_suspicions: AtomicU64,
    pub(crate) result_mismatches: AtomicU64,
    pub(crate) fatal_events: AtomicU64,
    pub(crate) quiesce_count: AtomicU64,
    pub(crate) quiesce_ns_total: AtomicU64,
    pub(crate) join_ns_total: AtomicU64,
    pub(crate) input_fence_retry_attempts: AtomicU64,
    pub(crate) input_fence_retry_preserved: AtomicU64,
    pub(crate) scheduler_queued_cancellations: AtomicU64,
    pub(crate) scheduler_cancel_mismatches: AtomicU64,
    pub(crate) cursor_pageflip_acks: AtomicU64,
    pub(crate) primary_pageflip_acks: AtomicU64,
    pub(crate) duplicate_pageflip_acks: AtomicU64,
    pub(crate) eventfd_notification_failures: AtomicU64,
    pub(crate) unnotified_fatal_health_checks: AtomicU64,
    pub(crate) runtime_queue_depth: AtomicU64,
    pub(crate) runtime_queue_depth_max: AtomicU64,
    pub(crate) runtime_kernel_inflight: AtomicU64,
    pub(crate) shutdown_admission_stops: AtomicU64,
    pub(crate) shutdown_queued_jobs_returned: AtomicU64,
    pub(crate) shutdown_queued_jobs_settled: AtomicU64,
    pub(crate) shutdown_ack_suppressed_next_submit: AtomicU64,
    pub(crate) shutdown_inflight_abandons: AtomicU64,
    pub(crate) cursor_worker_jobs_queued: AtomicU64,
    pub(crate) cursor_worker_submits_confirmed: AtomicU64,
    pub(crate) cursor_worker_rejections_retryable: AtomicU64,
    pub(crate) cursor_worker_rejections_fallback: AtomicU64,
    pub(crate) cursor_worker_arbitration_consumed: AtomicU64,
    pub(crate) cursor_worker_epoch_mismatches: AtomicU64,
    pub(crate) cursor_sidecars_materialized: AtomicU64,
    pub(crate) cursor_sidecars_replaced: AtomicU64,
    pub(crate) cursor_sidecars_claimed: AtomicU64,
    pub(crate) cursor_sidecars_promoted: AtomicU64,
    pub(crate) cursor_sidecars_missed_freeze: AtomicU64,
    pub(crate) worker_pacing_submits_confirmed: AtomicU64,
    pub(crate) worker_pacing_pre_submit_rejections: AtomicU64,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct WorkerMetricsSnapshot {
    pub(crate) timing: WorkerTimingSnapshot,
    pub(crate) jobs_enqueued: u64,
    pub(crate) jobs_submitted: u64,
    pub(crate) jobs_rejected: u64,
    pub(crate) queue_full: u64,
    pub(crate) admission_contention: u64,
    pub(crate) busy_deferrals: u64,
    pub(crate) busy_retries: u64,
    pub(crate) busy_exhausted: u64,
    pub(crate) late_wakeups: u64,
    pub(crate) submit_duration_ns_total: u64,
    pub(crate) submit_duration_ns_max: u64,
    pub(crate) queue_wait_ns_total: u64,
    pub(crate) queue_wait_ns_max: u64,
    pub(crate) pageflip_timeouts: u64,
    pub(crate) main_thread_stalls: u64,
    pub(crate) driver_timeout_suspicions: u64,
    pub(crate) result_mismatches: u64,
    pub(crate) fatal_events: u64,
    pub(crate) quiesce_count: u64,
    pub(crate) quiesce_ns_total: u64,
    pub(crate) join_ns_total: u64,
    pub(crate) input_fence_retry_attempts: u64,
    pub(crate) input_fence_retry_preserved: u64,
    pub(crate) scheduler_queued_cancellations: u64,
    pub(crate) scheduler_cancel_mismatches: u64,
    pub(crate) cursor_pageflip_acks: u64,
    pub(crate) primary_pageflip_acks: u64,
    pub(crate) duplicate_pageflip_acks: u64,
    pub(crate) eventfd_notification_failures: u64,
    pub(crate) unnotified_fatal_health_checks: u64,
    pub(crate) runtime_queue_depth: u64,
    pub(crate) runtime_queue_depth_max: u64,
    pub(crate) runtime_kernel_inflight: u64,
    pub(crate) shutdown_admission_stops: u64,
    pub(crate) shutdown_queued_jobs_returned: u64,
    pub(crate) shutdown_queued_jobs_settled: u64,
    pub(crate) shutdown_ack_suppressed_next_submit: u64,
    pub(crate) shutdown_inflight_abandons: u64,
    pub(crate) cursor_worker_jobs_queued: u64,
    pub(crate) cursor_worker_submits_confirmed: u64,
    pub(crate) cursor_worker_rejections_retryable: u64,
    pub(crate) cursor_worker_rejections_fallback: u64,
    pub(crate) cursor_worker_arbitration_consumed: u64,
    pub(crate) cursor_worker_epoch_mismatches: u64,
    pub(crate) cursor_sidecars_materialized: u64,
    pub(crate) cursor_sidecars_replaced: u64,
    pub(crate) cursor_sidecars_claimed: u64,
    pub(crate) cursor_sidecars_promoted: u64,
    pub(crate) cursor_sidecars_missed_freeze: u64,
    pub(crate) worker_pacing_submits_confirmed: u64,
    pub(crate) worker_pacing_pre_submit_rejections: u64,
}

impl WorkerMetrics {
    pub(crate) fn snapshot(&self) -> WorkerMetricsSnapshot {
        macro_rules! read {
            ($field:ident) => {
                self.$field.load(Ordering::Relaxed)
            };
        }
        WorkerMetricsSnapshot {
            timing: self.timing.snapshot(),
            jobs_enqueued: read!(jobs_enqueued),
            jobs_submitted: read!(jobs_submitted),
            jobs_rejected: read!(jobs_rejected),
            queue_full: read!(queue_full),
            admission_contention: read!(admission_contention),
            busy_deferrals: read!(busy_deferrals),
            busy_retries: read!(busy_retries),
            busy_exhausted: read!(busy_exhausted),
            late_wakeups: read!(late_wakeups),
            submit_duration_ns_total: read!(submit_duration_ns_total),
            submit_duration_ns_max: read!(submit_duration_ns_max),
            queue_wait_ns_total: read!(queue_wait_ns_total),
            queue_wait_ns_max: read!(queue_wait_ns_max),
            pageflip_timeouts: read!(pageflip_timeouts),
            main_thread_stalls: read!(main_thread_stalls),
            driver_timeout_suspicions: read!(driver_timeout_suspicions),
            result_mismatches: read!(result_mismatches),
            fatal_events: read!(fatal_events),
            quiesce_count: read!(quiesce_count),
            quiesce_ns_total: read!(quiesce_ns_total),
            join_ns_total: read!(join_ns_total),
            input_fence_retry_attempts: read!(input_fence_retry_attempts),
            input_fence_retry_preserved: read!(input_fence_retry_preserved),
            scheduler_queued_cancellations: read!(scheduler_queued_cancellations),
            scheduler_cancel_mismatches: read!(scheduler_cancel_mismatches),
            cursor_pageflip_acks: read!(cursor_pageflip_acks),
            primary_pageflip_acks: read!(primary_pageflip_acks),
            duplicate_pageflip_acks: read!(duplicate_pageflip_acks),
            eventfd_notification_failures: read!(eventfd_notification_failures),
            unnotified_fatal_health_checks: read!(unnotified_fatal_health_checks),
            runtime_queue_depth: read!(runtime_queue_depth),
            runtime_queue_depth_max: read!(runtime_queue_depth_max),
            runtime_kernel_inflight: read!(runtime_kernel_inflight),
            shutdown_admission_stops: read!(shutdown_admission_stops),
            shutdown_queued_jobs_returned: read!(shutdown_queued_jobs_returned),
            shutdown_queued_jobs_settled: read!(shutdown_queued_jobs_settled),
            shutdown_ack_suppressed_next_submit: read!(shutdown_ack_suppressed_next_submit),
            shutdown_inflight_abandons: read!(shutdown_inflight_abandons),
            cursor_worker_jobs_queued: read!(cursor_worker_jobs_queued),
            cursor_worker_submits_confirmed: read!(cursor_worker_submits_confirmed),
            cursor_worker_rejections_retryable: read!(cursor_worker_rejections_retryable),
            cursor_worker_rejections_fallback: read!(cursor_worker_rejections_fallback),
            cursor_worker_arbitration_consumed: read!(cursor_worker_arbitration_consumed),
            cursor_worker_epoch_mismatches: read!(cursor_worker_epoch_mismatches),
            cursor_sidecars_materialized: read!(cursor_sidecars_materialized),
            cursor_sidecars_replaced: read!(cursor_sidecars_replaced),
            cursor_sidecars_claimed: read!(cursor_sidecars_claimed),
            cursor_sidecars_promoted: read!(cursor_sidecars_promoted),
            cursor_sidecars_missed_freeze: read!(cursor_sidecars_missed_freeze),
            worker_pacing_submits_confirmed: read!(worker_pacing_submits_confirmed),
            worker_pacing_pre_submit_rejections: read!(worker_pacing_pre_submit_rejections),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KmsWorkerLifecycle {
    Running,
    Quiescing,
    ShutdownQuiescing,
    ShutdownAbandoning,
    Stopped,
    Fatal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KmsWorkerPhase {
    Idle,
    DequeuedWaitingPredecessor,
    CollectingSidecar,
    FrozenForValidation,
    TestOnly,
    SubmitIoctl,
    KernelInFlight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttachablePrimaryPhase {
    Queued,
    DequeuedWaitingPredecessor,
    CollectingSidecar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PendingBundleSnapshot {
    MutablePreFreeze {
        primary_transaction_id: crate::native_output::OutputTransactionId,
    },
    Frozen(KmsCommitBundleIdentity),
    InFlight(KmsCommitBundleIdentity),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AttachablePrimary {
    pub(crate) transaction_id: crate::native_output::OutputTransactionId,
    pub(crate) bundle_identity: KmsCommitBundleIdentity,
    pub(crate) validation_base: KmsValidationBase,
    pub(crate) output_generation: u64,
    pub(crate) crtc_id: u32,
    pub(crate) target: PresentationTarget,
    pub(crate) phase: AttachablePrimaryPhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KmsWorkerAdmissionError {
    DuplicateCandidate,
    QueueFull,
    AdmissionContention,
    Quiescing,
    ShutdownQuiescing,
    Stopped,
    Fatal,
}

#[derive(Debug)]
pub(crate) struct KmsCommitEnqueueError {
    pub(crate) job: KmsCommitJob,
    pub(crate) reason: KmsWorkerAdmissionError,
}

#[derive(Debug)]
pub(crate) struct CursorSidecarOfferError {
    pub(crate) sidecar: Box<CursorSidecar>,
    pub(crate) reason: KmsWorkerAdmissionError,
}

#[derive(Debug)]
pub(crate) struct KmsWorkerFatalJob {
    pub(crate) job: KmsCommitJob,
    pub(crate) uncertain_submit: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WorkerInFlight {
    pub(crate) bundle: KmsCommitBundleIdentity,
    pub(crate) token: oblivion_one::native::kms::PageFlipToken,
    pub(crate) transaction_id: crate::native_output::OutputTransactionId,
    pub(crate) output_generation: u64,
    pub(crate) kind: crate::native_output::runtime::AtomicCommitKind,
    pub(crate) direct_content_key: Option<DirectScanoutCandidateKey>,
    pub(crate) submit_returned_at_ns: u64,
}

#[derive(Debug)]
pub(crate) struct KmsWorkerShutdownSnapshot {
    pub(crate) queued_job: Option<KmsCommitJob>,
    pub(crate) inflight: Option<WorkerInFlight>,
    pub(crate) pending_sidecar: Option<CursorSidecar>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KmsWorkerForcedShutdownDisposition {
    InFlightAbandoned,
    CompletedAfterTimeout,
    AlreadyAbandoned,
    AlreadyStopped,
}

impl KmsWorkerForcedShutdownDisposition {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::InFlightAbandoned => "shutdown_inflight_abandoned",
            Self::CompletedAfterTimeout => "shutdown_inflight_completed_after_timeout",
            Self::AlreadyAbandoned => "shutdown_inflight_already_abandoned",
            Self::AlreadyStopped => "shutdown_worker_already_stopped",
        }
    }
}

#[derive(Debug)]
pub(crate) struct KmsWorkerForcedShutdown {
    pub(crate) queued_job: Option<KmsCommitJob>,
    pub(crate) inflight: Option<WorkerInFlight>,
    pub(crate) pending_sidecar: Option<CursorSidecar>,
    pub(crate) disposition: KmsWorkerForcedShutdownDisposition,
}

#[derive(Debug)]
pub(crate) struct WorkerShared {
    pub(crate) state: Mutex<WorkerState>,
    pub(crate) submit_gate: Mutex<()>,
    pub(crate) work_wakeup: Condvar,
    pub(crate) results: Mutex<VecDeque<KmsWorkerEvent>>,
    pub(crate) fatal_jobs: Mutex<Vec<KmsWorkerFatalJob>>,
    pub(crate) result_space: Condvar,
    pub(crate) result_fd: OwnedFd,
    pub(crate) metrics: WorkerMetrics,
    pub(crate) fatal_reason_code: AtomicU64,
    #[cfg(test)]
    pub(crate) dequeue_pause: Mutex<Option<Arc<DequeuePause>>>,
    #[cfg(test)]
    pub(crate) collecting_pause: Mutex<Option<Arc<DequeuePause>>>,
    #[cfg(test)]
    pub(crate) frozen_pause: Mutex<Option<Arc<DequeuePause>>>,
}

#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct DequeuePause {
    selected: (Mutex<bool>, Condvar),
    released: (Mutex<bool>, Condvar),
}

#[cfg(test)]
impl DequeuePause {
    pub(crate) fn wait_until_selected(&self) {
        let (selected, wakeup) = (&self.selected.0, &self.selected.1);
        let mut selected = selected
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while !*selected {
            selected = wakeup
                .wait(selected)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    pub(crate) fn release(&self) {
        let mut released = self
            .released
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *released = true;
        self.released.1.notify_all();
    }

    pub(crate) fn pause(&self) {
        let mut selected = self
            .selected
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *selected = true;
        self.selected.1.notify_all();
        drop(selected);

        let mut released = self
            .released
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while !*released {
            released = self
                .released
                .1
                .wait(released)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }
}

#[derive(Debug)]
pub(crate) struct WorkerState {
    pub(crate) lifecycle: KmsWorkerLifecycle,
    pub(crate) queued: VecDeque<KmsCommitJob>,
    pub(crate) reserved: usize,
    pub(crate) reserved_direct_content_keys: HashSet<DirectScanoutCandidateKey>,
    pub(crate) executing: bool,
    pub(crate) executing_direct_content_key: Option<DirectScanoutCandidateKey>,
    pub(crate) executing_primary_transaction_id: Option<crate::native_output::OutputTransactionId>,
    pub(crate) executing_bundle_identity: Option<KmsCommitBundleIdentity>,
    pub(crate) executing_primary: Option<AttachablePrimary>,
    pub(crate) inflight: Option<WorkerInFlight>,
    pub(crate) phase: KmsWorkerPhase,
    pub(crate) cursor_sidecar: CursorSidecarMailbox,
    pub(crate) established_base: Option<EstablishedKmsBase>,
}

impl WorkerShared {
    pub(crate) fn attachable_primary(
        &self,
        output_generation: u64,
        crtc_id: u32,
        target: PresentationTarget,
    ) -> Option<AttachablePrimary> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let queued = state.queued.front().and_then(|job| {
            if job.kind.is_primary()
                && matches!(job.primary, super::KmsPrimaryUpdate::Framebuffer { .. })
                && job.output_generation == output_generation
                && job.crtc_id == crtc_id
                && job.target == target
            {
                Some(AttachablePrimary {
                    transaction_id: job.owners.primary_transaction_id()?,
                    bundle_identity: job.identity(),
                    validation_base: job.validation_base,
                    output_generation: job.output_generation,
                    crtc_id: job.crtc_id,
                    target: job.target,
                    phase: AttachablePrimaryPhase::Queued,
                })
            } else {
                None
            }
        });
        queued.or_else(|| {
            state.executing_primary.filter(|primary| {
                primary.output_generation == output_generation
                    && primary.crtc_id == crtc_id
                    && primary.target == target
            })
        })
    }

    pub(crate) fn pending_bundle_snapshot(
        &self,
        output_generation: u64,
        crtc_id: u32,
    ) -> Option<PendingBundleSnapshot> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(inflight) = state.inflight {
            return (inflight.output_generation == output_generation
                && inflight.bundle.crtc_id == crtc_id)
                .then_some(PendingBundleSnapshot::InFlight(inflight.bundle));
        }
        if state.executing {
            let identity = state.executing_bundle_identity?;
            if identity.output_generation != output_generation || identity.crtc_id != crtc_id {
                return None;
            }
            return state.executing_primary.as_ref().map_or_else(
                || Some(PendingBundleSnapshot::Frozen(identity)),
                |primary| {
                    Some(PendingBundleSnapshot::MutablePreFreeze {
                        primary_transaction_id: primary.transaction_id,
                    })
                },
            );
        }
        let job = state.queued.front()?;
        if job.output_generation != output_generation || job.crtc_id != crtc_id {
            return None;
        }
        if job.kind.is_primary()
            && matches!(job.primary, super::KmsPrimaryUpdate::Framebuffer { .. })
            && let Some(primary_transaction_id) = job.owners.primary_transaction_id()
        {
            return Some(PendingBundleSnapshot::MutablePreFreeze {
                primary_transaction_id,
            });
        }
        Some(PendingBundleSnapshot::Frozen(job.identity()))
    }

    pub(crate) fn new(result_fd: OwnedFd) -> Self {
        Self {
            state: Mutex::new(WorkerState {
                lifecycle: KmsWorkerLifecycle::Running,
                queued: VecDeque::with_capacity(QUEUED_JOB_CAPACITY),
                reserved: 0,
                reserved_direct_content_keys: HashSet::new(),
                executing: false,
                executing_direct_content_key: None,
                executing_primary_transaction_id: None,
                executing_bundle_identity: None,
                executing_primary: None,
                inflight: None,
                phase: KmsWorkerPhase::Idle,
                cursor_sidecar: CursorSidecarMailbox::default(),
                established_base: None,
            }),
            submit_gate: Mutex::new(()),
            work_wakeup: Condvar::new(),
            results: Mutex::new(VecDeque::with_capacity(RESULT_EVENT_CAPACITY)),
            fatal_jobs: Mutex::new(Vec::new()),
            result_space: Condvar::new(),
            result_fd,
            metrics: WorkerMetrics::default(),
            fatal_reason_code: AtomicU64::new(0),
            #[cfg(test)]
            dequeue_pause: Mutex::new(None),
            #[cfg(test)]
            collecting_pause: Mutex::new(None),
            #[cfg(test)]
            frozen_pause: Mutex::new(None),
        }
    }

    pub(crate) fn set_established_presented_base(
        &self,
        revision: crate::native_output::presentation::plane::PlaneStateRevision,
        output_generation: u64,
        crtc_id: u32,
    ) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        debug_assert!(state.inflight.is_none());
        if state.queued.is_empty() {
            state.established_base = Some(EstablishedKmsBase::Presented {
                revision,
                output_generation,
                crtc_id,
            });
        }
        drop(state);
        self.work_wakeup.notify_all();
    }

    pub(crate) fn validation_base_disposition(
        state: &WorkerState,
        required: KmsValidationBase,
    ) -> ValidationBaseDisposition {
        if let Some(established) = state.established_base {
            return validation_base_ready(established, required);
        }
        match required {
            KmsValidationBase::Presented { .. } => ValidationBaseDisposition::Ready,
            KmsValidationBase::Predecessor(required) => {
                if state
                    .inflight
                    .is_some_and(|inflight| inflight.bundle == required)
                    || state.executing_primary.is_some()
                    || state.queued.front().is_some_and(|job| {
                        job.kind.is_primary()
                            && matches!(job.primary, super::KmsPrimaryUpdate::Framebuffer { .. })
                    })
                {
                    ValidationBaseDisposition::Wait
                } else {
                    ValidationBaseDisposition::Invalidated
                }
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn pause_after_dequeue_for_test(self: &Arc<Self>) -> Arc<DequeuePause> {
        let pause = Arc::new(DequeuePause::default());
        *self
            .dequeue_pause
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::clone(&pause));
        pause
    }

    #[cfg(test)]
    pub(crate) fn take_dequeue_pause_for_test(&self) -> Option<Arc<DequeuePause>> {
        self.dequeue_pause
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }

    #[cfg(test)]
    pub(crate) fn pause_collecting_for_test(self: &Arc<Self>) -> Arc<DequeuePause> {
        let pause = Arc::new(DequeuePause::default());
        *self
            .collecting_pause
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::clone(&pause));
        pause
    }

    #[cfg(test)]
    pub(crate) fn take_collecting_pause_for_test(&self) -> Option<Arc<DequeuePause>> {
        self.collecting_pause
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }

    #[cfg(test)]
    pub(crate) fn pause_frozen_for_test(self: &Arc<Self>) -> Arc<DequeuePause> {
        let pause = Arc::new(DequeuePause::default());
        *self
            .frozen_pause
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::clone(&pause));
        pause
    }

    #[cfg(test)]
    pub(crate) fn take_frozen_pause_for_test(&self) -> Option<Arc<DequeuePause>> {
        self.frozen_pause
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }

    pub(crate) fn offer_cursor_sidecar(
        &self,
        sidecar: CursorSidecar,
    ) -> Result<Option<CursorSidecar>, CursorSidecarOfferError> {
        let mut state = match self.state.try_lock() {
            Ok(state) => state,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => {
                return Err(CursorSidecarOfferError {
                    sidecar: Box::new(sidecar),
                    reason: KmsWorkerAdmissionError::AdmissionContention,
                });
            }
        };
        let reason = match state.lifecycle {
            KmsWorkerLifecycle::Running => None,
            KmsWorkerLifecycle::Quiescing => Some(KmsWorkerAdmissionError::Quiescing),
            KmsWorkerLifecycle::ShutdownQuiescing | KmsWorkerLifecycle::ShutdownAbandoning => {
                Some(KmsWorkerAdmissionError::ShutdownQuiescing)
            }
            KmsWorkerLifecycle::Stopped => Some(KmsWorkerAdmissionError::Stopped),
            KmsWorkerLifecycle::Fatal => Some(KmsWorkerAdmissionError::Fatal),
        };
        if let Some(reason) = reason {
            return Err(CursorSidecarOfferError {
                sidecar: Box::new(sidecar),
                reason,
            });
        }
        let replaced = state.cursor_sidecar.offer(sidecar);
        self.metrics
            .cursor_sidecars_materialized
            .fetch_add(1, Ordering::Relaxed);
        if replaced.is_some() {
            self.metrics
                .cursor_sidecars_replaced
                .fetch_add(1, Ordering::Relaxed);
        }
        if state.executing
            && matches!(
                state.phase,
                KmsWorkerPhase::FrozenForValidation
                    | KmsWorkerPhase::TestOnly
                    | KmsWorkerPhase::SubmitIoctl
                    | KmsWorkerPhase::KernelInFlight
            )
        {
            self.metrics
                .cursor_sidecars_missed_freeze
                .fetch_add(1, Ordering::Relaxed);
        }
        debug_assert!(state.cursor_sidecar.len() <= 1);
        drop(state);
        self.work_wakeup.notify_all();
        Ok(replaced)
    }

    pub(crate) fn direct_content_keys(
        &self,
    ) -> (
        Option<DirectScanoutCandidateKey>,
        Option<DirectScanoutCandidateKey>,
        Option<DirectScanoutCandidateKey>,
    ) {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let queued = state
            .queued
            .iter()
            .find_map(|job| job.direct_primary_lease.as_ref().map(|lease| lease.key()));
        let executing = state.executing_direct_content_key;
        let inflight = state
            .inflight
            .and_then(|ownership| ownership.direct_content_key);
        (queued, executing, inflight)
    }

    pub(crate) fn try_reserve(
        self: &Arc<Self>,
    ) -> Result<KmsCommitAdmissionPermit, KmsWorkerAdmissionError> {
        let mut state = match self.state.try_lock() {
            Ok(state) => state,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => {
                self.metrics
                    .admission_contention
                    .fetch_add(1, Ordering::Relaxed);
                return Err(KmsWorkerAdmissionError::AdmissionContention);
            }
        };
        match state.lifecycle {
            KmsWorkerLifecycle::Quiescing => return Err(KmsWorkerAdmissionError::Quiescing),
            KmsWorkerLifecycle::ShutdownQuiescing => {
                return Err(KmsWorkerAdmissionError::ShutdownQuiescing);
            }
            KmsWorkerLifecycle::ShutdownAbandoning => {
                return Err(KmsWorkerAdmissionError::ShutdownQuiescing);
            }
            KmsWorkerLifecycle::Stopped => return Err(KmsWorkerAdmissionError::Stopped),
            KmsWorkerLifecycle::Fatal => return Err(KmsWorkerAdmissionError::Fatal),
            KmsWorkerLifecycle::Running => {}
        }
        let occupied = state.queued.len()
            + state.reserved
            + usize::from(state.executing)
            + usize::from(state.inflight.is_some());
        let active = usize::from(state.executing || state.inflight.is_some());
        let capacity = active.saturating_add(QUEUED_JOB_CAPACITY);
        if occupied >= capacity {
            self.metrics.queue_full.fetch_add(1, Ordering::Relaxed);
            return Err(KmsWorkerAdmissionError::QueueFull);
        }
        state.reserved += 1;
        Ok(KmsCommitAdmissionPermit {
            shared: Arc::clone(self),
            active: true,
            direct_content_key: None,
        })
    }

    pub(crate) fn try_reserve_direct(
        self: &Arc<Self>,
        candidate_key: DirectScanoutCandidateKey,
    ) -> Result<KmsCommitAdmissionPermit, KmsWorkerAdmissionError> {
        let mut state = match self.state.try_lock() {
            Ok(state) => state,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => {
                self.metrics
                    .admission_contention
                    .fetch_add(1, Ordering::Relaxed);
                return Err(KmsWorkerAdmissionError::AdmissionContention);
            }
        };
        match state.lifecycle {
            KmsWorkerLifecycle::Quiescing => return Err(KmsWorkerAdmissionError::Quiescing),
            KmsWorkerLifecycle::ShutdownQuiescing => {
                return Err(KmsWorkerAdmissionError::ShutdownQuiescing);
            }
            KmsWorkerLifecycle::ShutdownAbandoning => {
                return Err(KmsWorkerAdmissionError::ShutdownQuiescing);
            }
            KmsWorkerLifecycle::Stopped => return Err(KmsWorkerAdmissionError::Stopped),
            KmsWorkerLifecycle::Fatal => return Err(KmsWorkerAdmissionError::Fatal),
            KmsWorkerLifecycle::Running => {}
        }
        if state.queued.iter().any(|job| {
            job.direct_primary_lease
                .as_ref()
                .is_some_and(|lease| lease.key() == candidate_key)
        }) || state.executing_direct_content_key == Some(candidate_key)
            || state
                .inflight
                .is_some_and(|ownership| ownership.direct_content_key == Some(candidate_key))
            || state.reserved_direct_content_keys.contains(&candidate_key)
        {
            return Err(KmsWorkerAdmissionError::DuplicateCandidate);
        }
        let occupied = state.queued.len()
            + state.reserved
            + usize::from(state.executing)
            + usize::from(state.inflight.is_some());
        let active = usize::from(state.executing || state.inflight.is_some());
        let capacity = active.saturating_add(QUEUED_JOB_CAPACITY);
        if occupied >= capacity {
            self.metrics.queue_full.fetch_add(1, Ordering::Relaxed);
            return Err(KmsWorkerAdmissionError::QueueFull);
        }
        state.reserved += 1;
        state.reserved_direct_content_keys.insert(candidate_key);
        Ok(KmsCommitAdmissionPermit {
            shared: Arc::clone(self),
            active: true,
            direct_content_key: Some(candidate_key),
        })
    }

    pub(crate) fn request_quiesce(&self) {
        let started = Instant::now();
        let _submit_gate = self
            .submit_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if matches!(state.lifecycle, KmsWorkerLifecycle::Running) {
            state.lifecycle = KmsWorkerLifecycle::Quiescing;
        }
        self.work_wakeup.notify_all();
        self.metrics.quiesce_count.fetch_add(1, Ordering::Relaxed);
        self.metrics.quiesce_ns_total.fetch_add(
            u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
    }

    pub(crate) fn begin_shutdown_quiesce(
        &self,
    ) -> Result<KmsWorkerShutdownSnapshot, KmsWorkerAdmissionError> {
        let started = Instant::now();
        // Serializing admission-stop with the ioctl boundary means that once
        // this method returns, no worker ioctl can still begin.  The gate is
        // deliberately distinct from the queue mutex and is held only across
        // the kernel call itself.
        let _submit_gate = self
            .submit_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match state.lifecycle {
            KmsWorkerLifecycle::Fatal => return Err(KmsWorkerAdmissionError::Fatal),
            KmsWorkerLifecycle::Stopped => return Err(KmsWorkerAdmissionError::Stopped),
            KmsWorkerLifecycle::Running | KmsWorkerLifecycle::Quiescing => {
                state.lifecycle = KmsWorkerLifecycle::ShutdownQuiescing;
            }
            KmsWorkerLifecycle::ShutdownQuiescing | KmsWorkerLifecycle::ShutdownAbandoning => {}
        }
        let queued_job = state.queued.pop_front();
        let pending_sidecar = state.cursor_sidecar.take();
        if queued_job.is_some() {
            self.metrics
                .shutdown_queued_jobs_returned
                .fetch_add(1, Ordering::Relaxed);
        }
        let inflight = state.inflight;
        drop(state);
        self.work_wakeup.notify_all();
        self.metrics
            .shutdown_admission_stops
            .fetch_add(1, Ordering::Relaxed);
        self.metrics.quiesce_count.fetch_add(1, Ordering::Relaxed);
        self.metrics.quiesce_ns_total.fetch_add(
            u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        Ok(KmsWorkerShutdownSnapshot {
            queued_job,
            inflight,
            pending_sidecar,
        })
    }

    pub(crate) fn force_shutdown_abandon(
        &self,
    ) -> Result<KmsWorkerForcedShutdown, KmsWorkerAdmissionError> {
        let started = Instant::now();
        // The submit gate makes the forced transition wait for an ioctl that
        // is already executing. Once it is acquired, no later ioctl can begin
        // before the in-flight identity is detached and the worker is woken.
        let _submit_gate = self
            .submit_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if matches!(state.lifecycle, KmsWorkerLifecycle::Fatal) {
            return Err(KmsWorkerAdmissionError::Fatal);
        }
        if matches!(state.lifecycle, KmsWorkerLifecycle::Stopped) {
            return Ok(KmsWorkerForcedShutdown {
                queued_job: None,
                inflight: None,
                pending_sidecar: state.cursor_sidecar.take(),
                disposition: KmsWorkerForcedShutdownDisposition::AlreadyStopped,
            });
        }
        let disposition = match state.lifecycle {
            KmsWorkerLifecycle::ShutdownQuiescing => {
                if state.inflight.is_some() {
                    KmsWorkerForcedShutdownDisposition::InFlightAbandoned
                } else {
                    KmsWorkerForcedShutdownDisposition::CompletedAfterTimeout
                }
            }
            KmsWorkerLifecycle::ShutdownAbandoning => {
                KmsWorkerForcedShutdownDisposition::AlreadyAbandoned
            }
            _ => return Err(KmsWorkerAdmissionError::Quiescing),
        };
        state.lifecycle = KmsWorkerLifecycle::ShutdownAbandoning;
        let queued_job = state.queued.pop_front();
        let pending_sidecar = state.cursor_sidecar.take();
        let inflight = state.inflight.take();
        drop(state);
        self.work_wakeup.notify_all();
        if inflight.is_some() {
            self.metrics
                .shutdown_inflight_abandons
                .fetch_add(1, Ordering::Relaxed);
        }
        self.metrics.quiesce_count.fetch_add(1, Ordering::Relaxed);
        self.metrics.quiesce_ns_total.fetch_add(
            u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        Ok(KmsWorkerForcedShutdown {
            queued_job,
            inflight,
            pending_sidecar,
            disposition,
        })
    }
}

#[derive(Debug)]
pub(crate) struct KmsCommitAdmissionPermit {
    pub(crate) shared: Arc<WorkerShared>,
    active: bool,
    direct_content_key: Option<DirectScanoutCandidateKey>,
}

impl KmsCommitAdmissionPermit {
    pub(crate) fn enqueue(mut self, job: KmsCommitJob) -> Result<(), Box<KmsCommitEnqueueError>> {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.active = false;
        state.reserved = state.reserved.saturating_sub(1);
        if let Some(key) = self.direct_content_key.take() {
            state.reserved_direct_content_keys.remove(&key);
        }
        if !matches!(state.lifecycle, KmsWorkerLifecycle::Running)
            || state.queued.len() >= QUEUED_JOB_CAPACITY
        {
            let reason = match state.lifecycle {
                KmsWorkerLifecycle::Quiescing => KmsWorkerAdmissionError::Quiescing,
                KmsWorkerLifecycle::ShutdownQuiescing => KmsWorkerAdmissionError::ShutdownQuiescing,
                KmsWorkerLifecycle::ShutdownAbandoning => {
                    KmsWorkerAdmissionError::ShutdownQuiescing
                }
                KmsWorkerLifecycle::Stopped => KmsWorkerAdmissionError::Stopped,
                KmsWorkerLifecycle::Fatal => KmsWorkerAdmissionError::Fatal,
                KmsWorkerLifecycle::Running => KmsWorkerAdmissionError::QueueFull,
            };
            return Err(Box::new(KmsCommitEnqueueError { job, reason }));
        }
        if state.established_base.is_none()
            && let KmsValidationBase::Presented {
                snapshot,
                output_generation,
                crtc_id,
            } = job.validation_base
        {
            state.established_base = Some(EstablishedKmsBase::Presented {
                revision: snapshot.revision,
                output_generation,
                crtc_id,
            });
        }
        state.queued.push_back(job);
        self.shared
            .metrics
            .jobs_enqueued
            .fetch_add(1, Ordering::Relaxed);
        drop(state);
        self.shared.work_wakeup.notify_one();
        Ok(())
    }
}

impl Drop for KmsCommitAdmissionPermit {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.reserved = state.reserved.saturating_sub(1);
        if let Some(key) = self.direct_content_key.take() {
            state.reserved_direct_content_keys.remove(&key);
        }
        self.active = false;
        self.shared.work_wakeup.notify_one();
    }
}

pub(crate) fn create_eventfd() -> std::io::Result<OwnedFd> {
    let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
    if fd < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        // SAFETY: eventfd returned a new owned descriptor.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }
}

pub(crate) fn notify_eventfd(fd: &OwnedFd) -> std::io::Result<()> {
    let value = 1u64;
    loop {
        let result = unsafe {
            libc::write(
                fd.as_raw_fd(),
                (&value as *const u64).cast(),
                std::mem::size_of::<u64>(),
            )
        };
        if result == std::mem::size_of::<u64>() as isize {
            return Ok(());
        }
        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            if error.kind() == std::io::ErrorKind::WouldBlock {
                return Err(error);
            }
            return Err(error);
        }
        return Err(std::io::Error::other("short eventfd notification write"));
    }
}

pub(crate) fn drain_eventfd(fd: &OwnedFd) -> std::io::Result<()> {
    loop {
        let mut value = 0u64;
        let result = unsafe {
            libc::read(
                fd.as_raw_fd(),
                (&mut value as *mut u64).cast(),
                std::mem::size_of::<u64>(),
            )
        };
        if result == std::mem::size_of::<u64>() as isize {
            continue;
        }
        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            if error.kind() == std::io::ErrorKind::WouldBlock {
                return Ok(());
            }
            return Err(error);
        }
        return Err(std::io::Error::other("short eventfd read"));
    }
}
