//! Bounded worker dispatch timing for submit-only Atomic jobs.

use std::collections::VecDeque;

const SAMPLE_CAPACITY: usize = 120;
const DISPATCH_GUARD_NS: u64 = 50_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KmsWorkerDispatchBudget {
    pub(crate) wake_lateness_ns: u64,
    pub(crate) pre_submit_ns: u64,
    pub(crate) ioctl_duration_ns: u64,
    pub(crate) dispatch_budget_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KmsWorkerDispatchModel {
    wake_lateness_ns: VecDeque<u64>,
    pre_submit_ns: VecDeque<u64>,
    ioctl_duration_ns: VecDeque<u64>,
    dispatch_duration_ns: VecDeque<u64>,
}

impl Default for KmsWorkerDispatchModel {
    fn default() -> Self {
        Self {
            wake_lateness_ns: VecDeque::with_capacity(SAMPLE_CAPACITY),
            pre_submit_ns: VecDeque::with_capacity(SAMPLE_CAPACITY),
            ioctl_duration_ns: VecDeque::with_capacity(SAMPLE_CAPACITY),
            dispatch_duration_ns: VecDeque::with_capacity(SAMPLE_CAPACITY),
        }
    }
}

impl KmsWorkerDispatchModel {
    pub(crate) fn record(
        &mut self,
        wake_lateness_ns: u64,
        pre_submit_ns: u64,
        ioctl_duration_ns: u64,
    ) {
        push_bounded(&mut self.wake_lateness_ns, wake_lateness_ns);
        push_bounded(&mut self.pre_submit_ns, pre_submit_ns);
        push_bounded(&mut self.ioctl_duration_ns, ioctl_duration_ns);
        push_bounded(
            &mut self.dispatch_duration_ns,
            pre_submit_ns.saturating_add(ioctl_duration_ns),
        );
    }

    pub(crate) fn budget(&self) -> KmsWorkerDispatchBudget {
        let wake_lateness_ns = nearest_rank(&self.wake_lateness_ns, 95);
        let pre_submit_ns = nearest_rank(&self.pre_submit_ns, 95);
        let ioctl_duration_ns = nearest_rank(&self.ioctl_duration_ns, 95);
        let dispatch_budget_ns = wake_lateness_ns
            .saturating_add(nearest_rank(&self.dispatch_duration_ns, 95))
            .saturating_add(DISPATCH_GUARD_NS);
        KmsWorkerDispatchBudget {
            wake_lateness_ns,
            pre_submit_ns,
            ioctl_duration_ns,
            dispatch_budget_ns,
        }
    }
}

fn push_bounded(samples: &mut VecDeque<u64>, sample: u64) {
    if samples.len() == SAMPLE_CAPACITY {
        samples.pop_front();
    }
    samples.push_back(sample);
}

fn nearest_rank(samples: &VecDeque<u64>, percentile: usize) -> u64 {
    let mut sorted: Vec<_> = samples.iter().copied().collect();
    if sorted.is_empty() {
        return 0;
    }
    sorted.sort_unstable();
    let rank = (percentile * sorted.len()).div_ceil(100).max(1);
    sorted[rank - 1]
}
