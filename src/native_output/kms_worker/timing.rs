//! Bounded worker dispatch timing for submit-only Atomic jobs.

use std::collections::VecDeque;

const SAMPLE_CAPACITY: usize = 120;
pub(crate) const DISPATCH_GUARD_NS: u64 = 50_000;
const DISPATCH_TAIL_SAFETY_QUANTUM_NS: u64 = 50_000;
const DISPATCH_TAIL_CLEAN_STREAK: u32 = 32;
const DISPATCH_TAIL_DECAY_NS: u64 = 50_000;
pub(crate) const MAX_DISPATCH_TAIL_GUARD_NS: u64 = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KmsWorkerDispatchTailObservation {
    pub(crate) binding_target: bool,
    pub(crate) dequeued_before_planned_wake: bool,
    pub(crate) fair_dispatch_chance: bool,
    pub(crate) deadline_overrun_ns: u64,
    pub(crate) guard_before_ns: u64,
    pub(crate) guard_ns: u64,
    pub(crate) increased: bool,
    pub(crate) decayed: bool,
    pub(crate) cap_hit: bool,
}

pub(crate) const fn dispatch_fairness(
    binding_target: bool,
    dequeued_before_planned_wake: bool,
) -> bool {
    binding_target && dequeued_before_planned_wake
}

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
    adaptive_tail_guard_ns: u64,
    clean_completion_streak: u32,
}

impl Default for KmsWorkerDispatchModel {
    fn default() -> Self {
        Self {
            wake_lateness_ns: VecDeque::with_capacity(SAMPLE_CAPACITY),
            pre_submit_ns: VecDeque::with_capacity(SAMPLE_CAPACITY),
            ioctl_duration_ns: VecDeque::with_capacity(SAMPLE_CAPACITY),
            dispatch_duration_ns: VecDeque::with_capacity(SAMPLE_CAPACITY),
            adaptive_tail_guard_ns: 0,
            clean_completion_streak: 0,
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
            .saturating_add(DISPATCH_GUARD_NS)
            .saturating_add(self.adaptive_tail_guard_ns);
        KmsWorkerDispatchBudget {
            wake_lateness_ns,
            pre_submit_ns,
            ioctl_duration_ns,
            dispatch_budget_ns,
        }
    }

    pub(crate) fn observe_submission_deadline(
        &mut self,
        commit_complete_deadline_ns: u64,
        submit_returned_at_ns: u64,
        binding_target: bool,
        dequeued_before_planned_wake: bool,
    ) -> KmsWorkerDispatchTailObservation {
        let fair_dispatch_chance = dispatch_fairness(binding_target, dequeued_before_planned_wake);
        let deadline_overrun_ns = submit_returned_at_ns.saturating_sub(commit_complete_deadline_ns);
        let guard_before_ns = self.adaptive_tail_guard_ns;
        let mut increased = false;
        let mut decayed = false;
        let mut cap_hit = false;
        if fair_dispatch_chance {
            if deadline_overrun_ns > 0 {
                let requested_increase =
                    deadline_overrun_ns.saturating_add(DISPATCH_TAIL_SAFETY_QUANTUM_NS);
                let next_guard = guard_before_ns.saturating_add(requested_increase);
                cap_hit = next_guard > MAX_DISPATCH_TAIL_GUARD_NS;
                self.adaptive_tail_guard_ns = next_guard.min(MAX_DISPATCH_TAIL_GUARD_NS);
                increased = self.adaptive_tail_guard_ns > guard_before_ns;
                self.clean_completion_streak = 0;
            } else {
                self.clean_completion_streak = self.clean_completion_streak.saturating_add(1);
                if self.clean_completion_streak >= DISPATCH_TAIL_CLEAN_STREAK {
                    let prior_guard = self.adaptive_tail_guard_ns;
                    self.adaptive_tail_guard_ns =
                        prior_guard.saturating_sub(DISPATCH_TAIL_DECAY_NS);
                    decayed = self.adaptive_tail_guard_ns < prior_guard;
                    self.clean_completion_streak = 0;
                }
            }
        }
        KmsWorkerDispatchTailObservation {
            binding_target,
            dequeued_before_planned_wake,
            fair_dispatch_chance,
            deadline_overrun_ns,
            guard_before_ns,
            guard_ns: self.adaptive_tail_guard_ns,
            increased,
            decayed,
            cap_hit,
        }
    }

    pub(crate) const fn adaptive_tail_guard_ns(&self) -> u64 {
        self.adaptive_tail_guard_ns
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

    #[test]
    fn proven_dispatch_deadline_miss_teaches_the_next_budget_immediately() {
        let mut model = KmsWorkerDispatchModel::default();
        model.record(0, 0, 100_000, 100_000);
        let before = model.budget().dispatch_budget_ns;

        model.observe_submission_deadline(1_000_000, 1_012_230, true, true);

        assert_eq!(model.adaptive_tail_guard_ns(), 62_230);
        assert_eq!(
            model.budget().dispatch_budget_ns,
            before.saturating_add(62_230)
        );
    }

    #[test]
    fn late_payload_does_not_train_dispatch_tail_guard() {
        let mut model = KmsWorkerDispatchModel::default();
        model.record(0, 0, 100_000, 100_000);
        let before_guard = model.adaptive_tail_guard_ns();

        let observation = model.observe_submission_deadline(1_000_000, 1_012_230, false, true);

        assert_eq!(observation.guard_ns, before_guard);
        assert!(!observation.increased);
        assert_eq!(observation.deadline_overrun_ns, 12_230);
        assert!(observation.dequeued_before_planned_wake);
    }

    #[test]
    fn dispatch_tail_guard_is_bounded_and_decays_in_clean_windows() {
        let mut model = KmsWorkerDispatchModel::default();

        for _ in 0..64 {
            model.observe_submission_deadline(1_000_000, 2_000_000, true, true);
        }
        assert_eq!(model.adaptive_tail_guard_ns(), 1_000_000);

        model.observe_submission_deadline(3_000_000, 3_000_000, true, true);
        assert_eq!(model.adaptive_tail_guard_ns(), 1_000_000);

        for _ in 0..32 {
            model.observe_submission_deadline(4_000_000, 4_000_000, true, true);
        }
        assert_eq!(model.adaptive_tail_guard_ns(), 950_000);

        for _ in 0..(32 * 19) {
            model.observe_submission_deadline(5_000_000, 5_000_000, true, true);
        }
        assert_eq!(model.adaptive_tail_guard_ns(), 0);
    }

    #[test]
    fn dispatch_tail_miss_resets_clean_decay_streak() {
        let mut model = KmsWorkerDispatchModel::default();
        model.observe_submission_deadline(1_000_000, 1_100_000, true, true);
        for _ in 0..31 {
            model.observe_submission_deadline(2_000_000, 2_000_000, true, true);
        }

        model.observe_submission_deadline(3_000_000, 3_100_000, true, true);

        assert_eq!(model.adaptive_tail_guard_ns(), 300_000);
    }

    #[test]
    fn dispatch_fairness_decomposes_binding_from_dequeue_timing() {
        assert!(!dispatch_fairness(false, true));
        assert!(!dispatch_fairness(true, false));
        assert!(dispatch_fairness(true, true));
    }

    #[test]
    fn dispatch_observation_reports_dequeue_timing_without_weakening_fairness() {
        let mut model = KmsWorkerDispatchModel::default();
        let observation = model.observe_submission_deadline(1_000_000, 1_012_230, false, true);

        assert!(!observation.binding_target);
        assert!(observation.dequeued_before_planned_wake);
        assert!(!observation.fair_dispatch_chance);
        assert!(!observation.increased);
    }
}
