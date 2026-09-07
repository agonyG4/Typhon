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
        dispatch_duration_ns: u64,
    ) {
        push_bounded(&mut self.wake_lateness_ns, wake_lateness_ns);
        push_bounded(&mut self.pre_submit_ns, pre_submit_ns);
        push_bounded(&mut self.ioctl_duration_ns, ioctl_duration_ns);
        push_bounded(&mut self.dispatch_duration_ns, dispatch_duration_ns);
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
    if samples.is_empty() {
        return 0;
    }
    // Histories are bounded by push_bounded; prediction must not allocate on
    // the frame path. Copy both ring segments without disturbing eviction order.
    let mut scratch = [0; SAMPLE_CAPACITY];
    let values = &mut scratch[..samples.len()];
    let (front, back) = samples.as_slices();
    values[..front.len()].copy_from_slice(front);
    values[front.len()..].copy_from_slice(back);
    let rank = (percentile * values.len()).div_ceil(100).max(1);
    *values.select_nth_unstable(rank - 1).1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_percentiles_match_sorted_reference_through_ring_wraps() {
        let mut samples = VecDeque::with_capacity(SAMPLE_CAPACITY);
        for step in 0..=SAMPLE_CAPACITY * 3 {
            let original = samples.clone();
            let mut sorted: Vec<_> = samples.iter().copied().collect();
            sorted.sort_unstable();
            let expected = if sorted.is_empty() {
                0
            } else {
                sorted[(95 * sorted.len()).div_ceil(100) - 1]
            };
            assert_eq!(nearest_rank(&samples, 95), expected);
            assert_eq!(samples, original, "budget must preserve eviction order");
            let sample = match step % 4 {
                0 => 0,
                1 => u64::MAX,
                _ => ((step * 73) % 127) as u64,
            };
            push_bounded(&mut samples, sample);
        }
    }

    #[test]
    fn dispatch_budget_uses_the_measured_full_dispatch_interval() {
        let mut model = KmsWorkerDispatchModel::default();

        model.record(10, 100, 200, 1_000);

        assert_eq!(model.budget().dispatch_budget_ns, 51_010);
    }
}
