//! Bounded admission and result queues for the Atomic submit worker.

use super::{KmsCommitJob, KmsWorkerEvent};
use std::{
    collections::VecDeque,
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
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct WorkerMetricsSnapshot {
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
}

impl WorkerMetrics {
    pub(crate) fn snapshot(&self) -> WorkerMetricsSnapshot {
        macro_rules! read {
            ($field:ident) => {
                self.$field.load(Ordering::Relaxed)
            };
        }
        WorkerMetricsSnapshot {
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
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KmsWorkerLifecycle {
    Running,
    Quiescing,
    Stopped,
    Fatal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KmsWorkerAdmissionError {
    QueueFull,
    AdmissionContention,
    Quiescing,
    Stopped,
    Fatal,
}

#[derive(Debug)]
pub(crate) struct KmsCommitEnqueueError {
    pub(crate) job: KmsCommitJob,
    pub(crate) reason: KmsWorkerAdmissionError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WorkerInFlight {
    pub(crate) token: oblivion_one::native::kms::PageFlipToken,
}

#[derive(Debug)]
pub(crate) struct WorkerShared {
    pub(crate) state: Mutex<WorkerState>,
    pub(crate) work_wakeup: Condvar,
    pub(crate) results: Mutex<VecDeque<KmsWorkerEvent>>,
    pub(crate) result_space: Condvar,
    pub(crate) result_fd: OwnedFd,
    pub(crate) metrics: WorkerMetrics,
}

#[derive(Debug)]
pub(crate) struct WorkerState {
    pub(crate) lifecycle: KmsWorkerLifecycle,
    pub(crate) queued: VecDeque<KmsCommitJob>,
    pub(crate) reserved: usize,
    pub(crate) executing: bool,
    pub(crate) inflight: Option<WorkerInFlight>,
}

impl WorkerShared {
    pub(crate) fn new(result_fd: OwnedFd) -> Self {
        Self {
            state: Mutex::new(WorkerState {
                lifecycle: KmsWorkerLifecycle::Running,
                queued: VecDeque::with_capacity(QUEUED_JOB_CAPACITY),
                reserved: 0,
                executing: false,
                inflight: None,
            }),
            work_wakeup: Condvar::new(),
            results: Mutex::new(VecDeque::with_capacity(RESULT_EVENT_CAPACITY)),
            result_space: Condvar::new(),
            result_fd,
            metrics: WorkerMetrics::default(),
        }
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
        })
    }

    pub(crate) fn request_quiesce(&self) {
        let started = Instant::now();
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
}

#[derive(Debug)]
pub(crate) struct KmsCommitAdmissionPermit {
    pub(crate) shared: Arc<WorkerShared>,
    active: bool,
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
        if !matches!(state.lifecycle, KmsWorkerLifecycle::Running)
            || state.queued.len() >= QUEUED_JOB_CAPACITY
        {
            let reason = match state.lifecycle {
                KmsWorkerLifecycle::Quiescing => KmsWorkerAdmissionError::Quiescing,
                KmsWorkerLifecycle::Stopped => KmsWorkerAdmissionError::Stopped,
                KmsWorkerLifecycle::Fatal => KmsWorkerAdmissionError::Fatal,
                KmsWorkerLifecycle::Running => KmsWorkerAdmissionError::QueueFull,
            };
            return Err(Box::new(KmsCommitEnqueueError { job, reason }));
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
                return Ok(());
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
