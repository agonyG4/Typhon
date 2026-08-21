//! Bounded, lock-free aggregate timing data for the KMS worker.

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

const UNSIGNED_BUCKET_LIMITS_NS: [u64; 14] = [
    50_000,
    100_000,
    150_000,
    250_000,
    375_000,
    500_000,
    750_000,
    1_000_000,
    1_500_000,
    2_000_000,
    5_000_000,
    10_000_000,
    25_000_000,
    u64::MAX,
];

const SIGNED_BUCKET_LIMITS_NS: [i64; 28] = [
    -25_000_000,
    -10_000_000,
    -5_000_000,
    -2_000_000,
    -1_500_000,
    -1_000_000,
    -750_000,
    -500_000,
    -375_000,
    -250_000,
    -150_000,
    -100_000,
    -50_000,
    0,
    50_000,
    100_000,
    150_000,
    250_000,
    375_000,
    500_000,
    750_000,
    1_000_000,
    1_500_000,
    2_000_000,
    5_000_000,
    10_000_000,
    25_000_000,
    i64::MAX,
];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct TimingSummarySnapshot {
    pub(crate) count: u64,
    pub(crate) total_ns: u64,
    pub(crate) last_ns: u64,
    pub(crate) mean_ns: u64,
    pub(crate) p50_ns: u64,
    pub(crate) p95_ns: u64,
    pub(crate) p99_ns: u64,
    pub(crate) max_ns: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SignedTimingSummarySnapshot {
    pub(crate) count: u64,
    pub(crate) total_ns: i64,
    pub(crate) last_ns: i64,
    pub(crate) mean_ns: i64,
    pub(crate) p50_ns: i64,
    pub(crate) p95_ns: i64,
    pub(crate) p99_ns: i64,
    pub(crate) min_ns: i64,
    pub(crate) max_ns: i64,
}

#[derive(Debug)]
struct AtomicTimingSummary {
    count: AtomicU64,
    total_ns: AtomicU64,
    last_ns: AtomicU64,
    max_ns: AtomicU64,
    buckets: [AtomicU64; UNSIGNED_BUCKET_LIMITS_NS.len()],
}

impl Default for AtomicTimingSummary {
    fn default() -> Self {
        Self {
            count: AtomicU64::new(0),
            total_ns: AtomicU64::new(0),
            last_ns: AtomicU64::new(0),
            max_ns: AtomicU64::new(0),
            buckets: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
}

impl AtomicTimingSummary {
    fn record(&self, elapsed_ns: u64) {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.total_ns.fetch_add(elapsed_ns, Ordering::Relaxed);
        self.last_ns.store(elapsed_ns, Ordering::Relaxed);
        update_max(&self.max_ns, elapsed_ns);
        let bucket = UNSIGNED_BUCKET_LIMITS_NS
            .iter()
            .position(|limit| elapsed_ns <= *limit)
            .unwrap_or(UNSIGNED_BUCKET_LIMITS_NS.len() - 1);
        self.buckets[bucket].fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> TimingSummarySnapshot {
        let count = self.count.load(Ordering::Relaxed);
        let total_ns = self.total_ns.load(Ordering::Relaxed);
        TimingSummarySnapshot {
            count,
            total_ns,
            last_ns: self.last_ns.load(Ordering::Relaxed),
            mean_ns: if count == 0 { 0 } else { total_ns / count },
            p50_ns: self.percentile_ns(count, 50),
            p95_ns: self.percentile_ns(count, 95),
            p99_ns: self.percentile_ns(count, 99),
            max_ns: self.max_ns.load(Ordering::Relaxed),
        }
    }

    fn percentile_ns(&self, count: u64, percentile: u64) -> u64 {
        if count == 0 {
            return 0;
        }
        percentile_bucket(count, percentile, UNSIGNED_BUCKET_LIMITS_NS, |index| {
            self.buckets[index].load(Ordering::Relaxed)
        })
    }
}

#[derive(Debug)]
struct AtomicSignedTimingSummary {
    count: AtomicU64,
    total_ns: AtomicI64,
    last_ns: AtomicI64,
    min_ns: AtomicI64,
    max_ns: AtomicI64,
    buckets: [AtomicU64; SIGNED_BUCKET_LIMITS_NS.len()],
}

impl Default for AtomicSignedTimingSummary {
    fn default() -> Self {
        Self {
            count: AtomicU64::new(0),
            total_ns: AtomicI64::new(0),
            last_ns: AtomicI64::new(0),
            min_ns: AtomicI64::new(i64::MAX),
            max_ns: AtomicI64::new(i64::MIN),
            buckets: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
}

impl AtomicSignedTimingSummary {
    fn record(&self, value_ns: i64) {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.total_ns.fetch_add(value_ns, Ordering::Relaxed);
        self.last_ns.store(value_ns, Ordering::Relaxed);
        update_min(&self.min_ns, value_ns);
        update_max_signed(&self.max_ns, value_ns);
        let bucket = SIGNED_BUCKET_LIMITS_NS
            .iter()
            .position(|limit| value_ns <= *limit)
            .unwrap_or(SIGNED_BUCKET_LIMITS_NS.len() - 1);
        self.buckets[bucket].fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> SignedTimingSummarySnapshot {
        let count = self.count.load(Ordering::Relaxed);
        let total_ns = self.total_ns.load(Ordering::Relaxed);
        SignedTimingSummarySnapshot {
            count,
            total_ns,
            last_ns: self.last_ns.load(Ordering::Relaxed),
            mean_ns: if count == 0 {
                0
            } else {
                total_ns / i64::try_from(count).unwrap_or(i64::MAX)
            },
            p50_ns: self.percentile_ns(count, 50),
            p95_ns: self.percentile_ns(count, 95),
            p99_ns: self.percentile_ns(count, 99),
            min_ns: if count == 0 {
                0
            } else {
                self.min_ns.load(Ordering::Relaxed)
            },
            max_ns: if count == 0 {
                0
            } else {
                self.max_ns.load(Ordering::Relaxed)
            },
        }
    }

    fn percentile_ns(&self, count: u64, percentile: u64) -> i64 {
        if count == 0 {
            return 0;
        }
        percentile_bucket_signed(count, percentile, SIGNED_BUCKET_LIMITS_NS, |index| {
            self.buckets[index].load(Ordering::Relaxed)
        })
    }
}

fn percentile_bucket<T: Copy, const N: usize>(
    count: u64,
    percentile: u64,
    limits: [T; N],
    read_bucket: impl Fn(usize) -> u64,
) -> T {
    let rank = count.saturating_mul(percentile).saturating_add(99) / 100;
    let mut seen = 0u64;
    for (index, limit) in limits.into_iter().enumerate() {
        seen = seen.saturating_add(read_bucket(index));
        if seen >= rank {
            return limit;
        }
    }
    limits[N - 1]
}

fn percentile_bucket_signed<T: Copy, const N: usize>(
    count: u64,
    percentile: u64,
    limits: [T; N],
    read_bucket: impl Fn(usize) -> u64,
) -> T {
    let rank = count.saturating_mul(percentile).saturating_add(99) / 100;
    let mut seen = 0u64;
    for (index, limit) in limits.into_iter().enumerate() {
        seen = seen.saturating_add(read_bucket(index));
        if seen >= rank {
            return limit;
        }
    }
    limits[N - 1]
}

#[derive(Debug, Default)]
pub(crate) struct WorkerTimingMetrics {
    submit_wake_lateness: AtomicSignedTimingSummary,
    pre_submit_duration: AtomicTimingSummary,
    dispatch_duration: AtomicTimingSummary,
    ioctl_duration: AtomicTimingSummary,
    queue_residency: AtomicTimingSummary,
    submit_earliness: AtomicSignedTimingSummary,
    submit_return_earliness: AtomicSignedTimingSummary,
    submit_ack_delay: AtomicTimingSummary,
    pageflip_ack_delay: AtomicTimingSummary,
    test_only_duration: AtomicTimingSummary,
    dispatch_budget_ns: AtomicU64,
    late_before_ioctl: AtomicU64,
    late_after_ioctl: AtomicU64,
    test_only_count: AtomicU64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct WorkerTimingSnapshot {
    pub(crate) submit_wake_lateness: SignedTimingSummarySnapshot,
    pub(crate) pre_submit_duration: TimingSummarySnapshot,
    pub(crate) dispatch_duration: TimingSummarySnapshot,
    pub(crate) ioctl_duration: TimingSummarySnapshot,
    pub(crate) queue_residency: TimingSummarySnapshot,
    pub(crate) submit_earliness: SignedTimingSummarySnapshot,
    pub(crate) submit_return_earliness: SignedTimingSummarySnapshot,
    pub(crate) submit_ack_delay: TimingSummarySnapshot,
    pub(crate) pageflip_ack_delay: TimingSummarySnapshot,
    pub(crate) test_only_duration: TimingSummarySnapshot,
    pub(crate) dispatch_budget_ns: u64,
    pub(crate) late_before_ioctl: u64,
    pub(crate) late_after_ioctl: u64,
    pub(crate) test_only_count: u64,
}

impl WorkerTimingMetrics {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record_submission(
        &self,
        planned_worker_wake_ns: u64,
        actual_worker_wait_returned_ns: u64,
        commit_complete_deadline_ns: u64,
        target_presentation_ns: u64,
        submit_started_ns: u64,
        submit_returned_ns: u64,
        queue_residency_ns: u64,
        pre_submit_duration_ns: u64,
        ioctl_duration_ns: u64,
        dispatch_duration_ns: u64,
        dispatch_budget_ns: u64,
    ) {
        let wake_delta_ns = signed_delta(actual_worker_wait_returned_ns, planned_worker_wake_ns);
        let submit_earliness_ns = signed_delta(target_presentation_ns, submit_started_ns);
        let submit_return_earliness_ns = signed_delta(target_presentation_ns, submit_returned_ns);
        self.submit_wake_lateness.record(wake_delta_ns);
        self.pre_submit_duration.record(pre_submit_duration_ns);
        self.dispatch_duration.record(dispatch_duration_ns);
        self.submit_earliness.record(submit_earliness_ns);
        self.submit_return_earliness
            .record(submit_return_earliness_ns);
        self.ioctl_duration.record(ioctl_duration_ns);
        self.queue_residency.record(queue_residency_ns);
        self.dispatch_budget_ns
            .store(dispatch_budget_ns, Ordering::Relaxed);
        if wake_delta_ns > 0 {
            self.late_before_ioctl.fetch_add(1, Ordering::Relaxed);
        }
        if submit_returned_ns > commit_complete_deadline_ns {
            self.late_after_ioctl.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn record_pageflip_ack_delay(&self, delay_ns: u64) {
        self.pageflip_ack_delay.record(delay_ns);
    }

    pub(crate) fn record_submit_ack_delay(&self, delay_ns: u64) {
        self.submit_ack_delay.record(delay_ns);
    }

    pub(crate) fn record_test_only(&self, duration_ns: u64) {
        self.test_only_count.fetch_add(1, Ordering::Relaxed);
        self.test_only_duration.record(duration_ns);
    }

    pub(crate) fn snapshot(&self) -> WorkerTimingSnapshot {
        WorkerTimingSnapshot {
            submit_wake_lateness: self.submit_wake_lateness.snapshot(),
            pre_submit_duration: self.pre_submit_duration.snapshot(),
            dispatch_duration: self.dispatch_duration.snapshot(),
            ioctl_duration: self.ioctl_duration.snapshot(),
            queue_residency: self.queue_residency.snapshot(),
            submit_earliness: self.submit_earliness.snapshot(),
            submit_return_earliness: self.submit_return_earliness.snapshot(),
            submit_ack_delay: self.submit_ack_delay.snapshot(),
            pageflip_ack_delay: self.pageflip_ack_delay.snapshot(),
            test_only_duration: self.test_only_duration.snapshot(),
            dispatch_budget_ns: self.dispatch_budget_ns.load(Ordering::Relaxed),
            late_before_ioctl: self.late_before_ioctl.load(Ordering::Relaxed),
            late_after_ioctl: self.late_after_ioctl.load(Ordering::Relaxed),
            test_only_count: self.test_only_count.load(Ordering::Relaxed),
        }
    }
}

fn signed_delta(lhs: u64, rhs: u64) -> i64 {
    if lhs >= rhs {
        i64::try_from(lhs - rhs).unwrap_or(i64::MAX)
    } else {
        -i64::try_from(rhs - lhs).unwrap_or(i64::MAX)
    }
}

fn update_max(target: &AtomicU64, value: u64) {
    let mut current = target.load(Ordering::Relaxed);
    while current < value {
        match target.compare_exchange_weak(current, value, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(observed) => current = observed,
        }
    }
}

fn update_min(target: &AtomicI64, value: i64) {
    let mut current = target.load(Ordering::Relaxed);
    while current > value {
        match target.compare_exchange_weak(current, value, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(observed) => current = observed,
        }
    }
}

fn update_max_signed(target: &AtomicI64, value: i64) {
    let mut current = target.load(Ordering::Relaxed);
    while current < value {
        match target.compare_exchange_weak(current, value, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(observed) => current = observed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_timing_keeps_early_and_late_values() {
        let metrics = WorkerTimingMetrics::default();
        metrics.record_submission(
            11_000, 10_000, 20_000, 20_000, 9_000, 21_000, 100, 300, 200, 500, 1_000,
        );
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.submit_wake_lateness.last_ns, -1_000);
        assert_eq!(snapshot.submit_earliness.last_ns, 11_000);
        assert_eq!(snapshot.submit_return_earliness.last_ns, -1_000);
        assert_eq!(snapshot.late_before_ioctl, 0);
        assert_eq!(snapshot.late_after_ioctl, 1);
    }
}
