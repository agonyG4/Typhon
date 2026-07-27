//! Atomic submit worker thread and lifecycle boundary.

use super::payload::{KmsCursorUpdate, KmsPrimaryUpdate, KmsSubmittedOwnership, KmsTestOnlyPolicy};
use super::queue::{
    KmsWorkerFatalJob, KmsWorkerForcedShutdown, KmsWorkerLifecycle, KmsWorkerShutdownSnapshot,
    RESULT_EVENT_CAPACITY, WorkerInFlight, WorkerMetricsSnapshot, WorkerShared, create_eventfd,
    drain_eventfd, notify_eventfd,
};
use super::{
    KmsCommitAdmissionPermit, KmsCommitJob, KmsCommitTimingModel, KmsWorkerAdmissionError,
};
use crate::native_output::{OutputTransactionId, runtime::AtomicCommitKind};
use oblivion_one::native::kms::AtomicCommitSubmitter;
use oblivion_one::native::kms::{AtomicKmsError, AtomicKmsErrorKind, PageFlipToken};
use oblivion_one::native::presentation_deadline::MonotonicTimestampNs;
use std::{
    io,
    os::fd::{AsRawFd, BorrowedFd, OwnedFd},
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Debug)]
pub(crate) struct KmsWorkerSubmission {
    pub(crate) out_fence: Option<OwnedFd>,
}

pub(crate) trait KmsCommitExecutor: Send + Sync {
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
        transaction_id: OutputTransactionId,
        token: PageFlipToken,
        retry: u8,
    },
    SubmitLate {
        transaction_id: OutputTransactionId,
        token: PageFlipToken,
        late_by_ns: u64,
    },
    BusyExhausted {
        job: KmsCommitJob,
        error: AtomicKmsError,
    },
    PageflipTimeout {
        transaction_id: OutputTransactionId,
        token: PageFlipToken,
        detected_at: u64,
    },
    Quiesced {
        returned_jobs: Vec<KmsCommitJob>,
    },
    Fatal {
        reason: KmsWorkerFatalReason,
        uncertain_submit: bool,
    },
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

    pub(crate) fn metrics_snapshot(&self) -> WorkerMetricsSnapshot {
        self.shared.metrics.snapshot()
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

    pub(crate) fn ack_pageflip(
        &self,
        token: PageFlipToken,
        transaction_id: OutputTransactionId,
        output_generation: u64,
    ) -> Result<(), KmsWorkerAckError> {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(inflight) = state.inflight else {
            if matches!(
                state.lifecycle,
                KmsWorkerLifecycle::ShutdownQuiescing
                    | KmsWorkerLifecycle::ShutdownAbandoning
                    | KmsWorkerLifecycle::Stopped
            ) {
                return Ok(());
            }
            self.shared
                .metrics
                .duplicate_pageflip_acks
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Err(KmsWorkerAckError::NoInFlightCommit);
        };
        if inflight.token != token {
            self.shared
                .metrics
                .result_mismatches
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Err(KmsWorkerAckError::TokenMismatch);
        }
        if inflight.transaction_id != transaction_id {
            self.shared
                .metrics
                .result_mismatches
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Err(KmsWorkerAckError::TransactionMismatch);
        }
        if inflight.output_generation != output_generation {
            self.shared
                .metrics
                .result_mismatches
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Err(KmsWorkerAckError::GenerationMismatch);
        }
        if matches!(inflight.kind, AtomicCommitKind::CursorOnly { .. }) {
            self.shared
                .metrics
                .cursor_pageflip_acks
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        } else {
            self.shared
                .metrics
                .primary_pageflip_acks
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        let suppress_next_submit = matches!(
            state.lifecycle,
            KmsWorkerLifecycle::Quiescing
                | KmsWorkerLifecycle::ShutdownQuiescing
                | KmsWorkerLifecycle::ShutdownAbandoning
        );
        state.inflight = None;
        drop(state);
        if suppress_next_submit {
            self.shared
                .metrics
                .shutdown_ack_suppressed_next_submit
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.shared.work_wakeup.notify_all();
        } else {
            self.shared.work_wakeup.notify_one();
        }
        Ok(())
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
pub(crate) struct AtomicKmsWorkerExecutor {
    submitter: AtomicCommitSubmitter,
}

impl KmsCommitExecutor for AtomicKmsWorkerExecutor {
    fn submit(&self, job: &KmsCommitJob) -> Result<KmsWorkerSubmission, KmsWorkerSubmitFailure> {
        let touch_cursor = !matches!(&job.cursor, KmsCursorUpdate::Unchanged);
        let cursor = match &job.cursor {
            KmsCursorUpdate::Set(state) => Some(state),
            KmsCursorUpdate::Disable | KmsCursorUpdate::Unchanged => None,
        };

        if matches!(job.test_only, KmsTestOnlyPolicy::Required) {
            let test = match &job.primary {
                KmsPrimaryUpdate::Framebuffer { framebuffer, .. } => {
                    if touch_cursor {
                        self.submitter.submit_primary(
                            *framebuffer,
                            job.token,
                            cursor,
                            None,
                            false,
                            true,
                        )
                    } else {
                        self.submitter.submit_primary_without_cursor(
                            *framebuffer,
                            job.token,
                            None,
                            false,
                            true,
                        )
                    }
                }
                KmsPrimaryUpdate::Unchanged => {
                    self.submitter.submit_cursor(cursor, job.token, true)
                }
            };
            if let Err(error) = test {
                return Err(KmsWorkerSubmitFailure { error });
            }
        }

        let input_fence = match &job.primary {
            KmsPrimaryUpdate::Framebuffer { in_fence, .. } => in_fence.as_ref().map(|fence| {
                // SAFETY: the job-owned OwnedFd remains alive through the
                // complete ioctl and is released only after the executor
                // returns, including every EBUSY retry.
                unsafe { BorrowedFd::borrow_raw(fence.as_raw_fd()) }
            }),
            KmsPrimaryUpdate::Unchanged => None,
        };
        let submission = match &job.primary {
            KmsPrimaryUpdate::Framebuffer {
                framebuffer,
                in_fence: _,
                request_out_fence,
                ..
            } => {
                if touch_cursor {
                    self.submitter.submit_primary(
                        *framebuffer,
                        job.token,
                        cursor,
                        input_fence,
                        *request_out_fence,
                        false,
                    )
                } else {
                    self.submitter.submit_primary_without_cursor(
                        *framebuffer,
                        job.token,
                        input_fence,
                        *request_out_fence,
                        false,
                    )
                }
            }
            KmsPrimaryUpdate::Unchanged => self.submitter.submit_cursor(cursor, job.token, false),
        };
        submission
            .map(|submission| KmsWorkerSubmission {
                out_fence: submission.out_fence,
            })
            .map_err(|error| KmsWorkerSubmitFailure { error })
    }
}

impl Drop for KmsCommitWorkerHandle {
    fn drop(&mut self) {
        self.request_quiesce();
        let _ = self.join();
    }
}

fn run_worker(shared: Arc<WorkerShared>, executor: Arc<dyn KmsCommitExecutor>) {
    let mut timing = None;
    loop {
        let Some(mut job) = take_next_job(&shared) else {
            return;
        };
        if timing.is_none() {
            timing = Some(KmsCommitTimingModel::new(job.target.refresh_interval));
        }
        let target_presentation_ns = job.target.presentation_time.get();
        let model = timing.as_ref().copied().expect("timing model initialized");
        let now_ns = monotonic_now_ns();
        let decision = model.submit_at(job.target, now_ns);
        if decision.submit_at_ns > now_ns && !wait_until_or_quiesce(&shared, decision.submit_at_ns)
        {
            quiesce_with_jobs(&shared, vec![job]);
            return;
        }
        if decision.late {
            shared
                .metrics
                .late_wakeups
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if !publish_event(
                &shared,
                KmsWorkerEvent::SubmitLate {
                    transaction_id: job.transaction_id,
                    token: job.token,
                    late_by_ns: decision.late_by_ns,
                },
            ) {
                retain_fatal_job(&shared, job, false);
                return;
            }
        }

        let mut retries = 0u8;
        loop {
            let submission = {
                let _submit_gate = shared
                    .submit_gate
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let mut state = shared
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if !matches!(state.lifecycle, KmsWorkerLifecycle::Running) {
                    None
                } else {
                    state.executing = true;
                    drop(state);
                    let submit_started_at = monotonic_now_ns();
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        executor.submit(&job)
                    }));
                    let mut state = shared
                        .state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    state.executing = false;
                    if matches!(&result, Ok(Ok(_))) {
                        state.inflight = Some(WorkerInFlight {
                            token: job.token,
                            transaction_id: job.transaction_id,
                            output_generation: job.output_generation,
                            kind: job.kind,
                        });
                    }
                    drop(state);
                    Some((submit_started_at, result))
                }
            };
            let Some((submit_started_at, submission)) = submission else {
                quiesce_with_jobs(&shared, vec![job]);
                return;
            };
            match submission {
                Ok(Ok(submission)) => {
                    if let KmsPrimaryUpdate::Framebuffer { in_fence, .. } = &mut job.primary {
                        // The ioctl has consumed the input-fence contract. The
                        // fence must not remain owned while waiting for the pageflip.
                        let _ = in_fence.take();
                    }
                    let submit_returned_at = monotonic_now_ns();
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
                            submit_started_at: MonotonicTimestampNs::new(submit_started_at),
                            submit_returned_at: MonotonicTimestampNs::new(submit_returned_at),
                        },
                    };
                    if !publish_event(&shared, event) {
                        return;
                    }
                    if !wait_for_pageflip_or_quiesce(&shared, transaction_id, token) {
                        return;
                    }
                    timing
                        .as_mut()
                        .expect("timing model initialized")
                        .observe_submit_delta_ns(
                            i64::try_from(submit_returned_at)
                                .unwrap_or(i64::MAX)
                                .saturating_sub(
                                    i64::try_from(target_presentation_ns).unwrap_or(i64::MAX),
                                ),
                        );
                    break;
                }
                Ok(Err(failure)) if failure.error.kind == AtomicKmsErrorKind::Busy => {
                    shared
                        .metrics
                        .busy_deferrals
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if retries >= 2 {
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
                    if !wait_until_or_quiesce(&shared, deadline) {
                        quiesce_with_jobs(&shared, vec![job]);
                        return;
                    }
                }
                Ok(Err(failure)) if failure.error.kind == AtomicKmsErrorKind::TestOnlyRejected => {
                    shared
                        .metrics
                        .jobs_rejected
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if !publish_event(
                        &shared,
                        KmsWorkerEvent::TestRejected {
                            job,
                            error: failure.error,
                        },
                    ) {
                        return;
                    }
                    break;
                }
                Ok(Err(failure)) => {
                    shared
                        .metrics
                        .jobs_rejected
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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

fn take_next_job(shared: &Arc<WorkerShared>) -> Option<KmsCommitJob> {
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
            state.lifecycle = KmsWorkerLifecycle::Stopped;
            drop(state);
            publish_event(shared, KmsWorkerEvent::Quiesced { returned_jobs });
            return None;
        }
        if state.inflight.is_none()
            && let Some(job) = state.queued.pop_front()
        {
            return Some(job);
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
            state.lifecycle = KmsWorkerLifecycle::Stopped;
            drop(state);
            publish_event(shared, KmsWorkerEvent::Quiesced { returned_jobs });
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
        if timeout.timed_out()
            && !timeout_reported
            && state
                .inflight
                .is_some_and(|inflight| inflight.token == token)
        {
            timeout_reported = true;
            drop(state);
            if !publish_event(
                shared,
                KmsWorkerEvent::PageflipTimeout {
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

fn wait_until_or_quiesce(shared: &Arc<WorkerShared>, deadline_ns: u64) -> bool {
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
            return false;
        }
        let now = monotonic_now_ns();
        if now >= deadline_ns {
            return true;
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
    state.lifecycle = KmsWorkerLifecycle::Stopped;
    drop(state);
    publish_event(shared, KmsWorkerEvent::Quiesced { returned_jobs });
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
