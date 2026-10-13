//! Deterministic, bounded worker dispatch timing primitives.
//!
//! This module is exposed for internal runtime composition and regression tests.

use std::collections::VecDeque;

const SAMPLE_CAPACITY: usize = 120;
pub const DISPATCH_GUARD_NS: u64 = 50_000;
const DISPATCH_TAIL_SAFETY_QUANTUM_NS: u64 = 50_000;
const DISPATCH_TAIL_CLEAN_STREAK: u32 = 32;
const DISPATCH_TAIL_DECAY_NS: u64 = 50_000;
const CURSOR_DISPATCH_TAIL_CLEAN_STREAK: u32 = 2;
const CURSOR_DISPATCH_TAIL_MIN_DECAY_NS: u64 = 100_000;
const CURSOR_DISPATCH_TAIL_AGE_MIN_NS: u64 = 100_000_000;
const CURSOR_DISPATCH_TAIL_AGE_MAX_NS: u64 = 500_000_000;
const MAX_DISPATCH_TAIL_GUARD_NS: u64 = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[doc(hidden)]
pub struct KmsWorkerDispatchTailObservation {
    pub binding_target: bool,
    pub dequeued_before_planned_wake: bool,
    pub fair_dispatch_chance: bool,
    pub deadline_overrun_ns: u64,
    pub guard_before_ns: u64,
    pub guard_ns: u64,
    pub increased: bool,
    pub decayed: bool,
    pub cap_hit: bool,
}

pub const fn dispatch_fairness(binding_target: bool, dequeued_before_planned_wake: bool) -> bool {
    binding_target && dequeued_before_planned_wake
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[doc(hidden)]
pub struct KmsWorkerDispatchBudget {
    pub wake_lateness_ns: u64,
    pub pre_submit_ns: u64,
    pub ioctl_duration_ns: u64,
    pub dispatch_budget_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[doc(hidden)]
pub struct KmsWorkerDispatchModel {
    wake_lateness_ns: VecDeque<u64>,
    pre_submit_ns: VecDeque<u64>,
    ioctl_duration_ns: VecDeque<u64>,
    dispatch_duration_ns: VecDeque<u64>,
    adaptive_tail_guard_ns: u64,
    clean_completion_streak: u32,
    clean_streak_limit: u32,
    fast_clean_recovery: bool,
    age_window_ns: Option<u64>,
    age_anchor_ns: u64,
    age_anchor_guard_ns: u64,
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
            clean_streak_limit: DISPATCH_TAIL_CLEAN_STREAK,
            fast_clean_recovery: false,
            age_window_ns: None,
            age_anchor_ns: 0,
            age_anchor_guard_ns: 0,
        }
    }
}

impl KmsWorkerDispatchModel {
    pub fn cursor() -> Self {
        Self {
            clean_streak_limit: CURSOR_DISPATCH_TAIL_CLEAN_STREAK,
            fast_clean_recovery: true,
            age_window_ns: Some(CURSOR_DISPATCH_TAIL_AGE_MIN_NS),
            ..Self::default()
        }
    }

    pub fn reset_cursor_state(&mut self) {
        self.wake_lateness_ns.clear();
        self.pre_submit_ns.clear();
        self.ioctl_duration_ns.clear();
        self.dispatch_duration_ns.clear();
        self.adaptive_tail_guard_ns = 0;
        self.clean_completion_streak = 0;
        self.clean_streak_limit = CURSOR_DISPATCH_TAIL_CLEAN_STREAK;
        self.fast_clean_recovery = true;
        self.age_window_ns = Some(CURSOR_DISPATCH_TAIL_AGE_MIN_NS);
        self.age_anchor_ns = 0;
        self.age_anchor_guard_ns = 0;
    }

    pub fn set_cursor_recovery_window(&mut self, refresh_interval_ns: u64) {
        if !self.fast_clean_recovery {
            return;
        }
        self.age_window_ns = Some(refresh_interval_ns.saturating_mul(8).clamp(
            CURSOR_DISPATCH_TAIL_AGE_MIN_NS,
            CURSOR_DISPATCH_TAIL_AGE_MAX_NS,
        ));
    }

    pub fn age_tail_guard(&mut self, now_ns: u64) {
        let Some(age_window_ns) = self.age_window_ns else {
            return;
        };
        let elapsed_ns = now_ns.saturating_sub(self.age_anchor_ns);
        if elapsed_ns == 0 || self.adaptive_tail_guard_ns == 0 {
            return;
        }
        self.adaptive_tail_guard_ns = if elapsed_ns >= age_window_ns {
            0
        } else {
            self.age_anchor_guard_ns
                .saturating_mul(age_window_ns.saturating_sub(elapsed_ns))
                / age_window_ns
        };
    }

    pub fn record_worker_service(
        &mut self,
        pre_submit_ns: u64,
        ioctl_duration_ns: u64,
        dispatch_duration_ns: u64,
    ) {
        push_bounded(&mut self.pre_submit_ns, pre_submit_ns);
        push_bounded(&mut self.ioctl_duration_ns, ioctl_duration_ns);
        push_bounded(&mut self.dispatch_duration_ns, dispatch_duration_ns);
    }

    pub fn record_fixed_timed_wake(&mut self, lateness_ns: u64) {
        push_bounded(&mut self.wake_lateness_ns, lateness_ns);
    }

    pub fn budget(&self) -> KmsWorkerDispatchBudget {
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

    pub fn observe_submission_deadline(
        &mut self,
        commit_complete_deadline_ns: u64,
        submit_returned_at_ns: u64,
        binding_target: bool,
        dequeued_before_planned_wake: bool,
    ) -> KmsWorkerDispatchTailObservation {
        self.age_tail_guard(submit_returned_at_ns);
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
                self.age_anchor_ns = submit_returned_at_ns;
                self.age_anchor_guard_ns = self.adaptive_tail_guard_ns;
                increased = self.adaptive_tail_guard_ns > guard_before_ns;
                self.clean_completion_streak = 0;
            } else {
                self.clean_completion_streak = self.clean_completion_streak.saturating_add(1);
                if self.clean_completion_streak >= self.clean_streak_limit {
                    if self.fast_clean_recovery {
                        self.decay_cursor_history_toward_recent_clean_sample();
                    }
                    let prior_guard = self.adaptive_tail_guard_ns;
                    let decay = if self.fast_clean_recovery {
                        (prior_guard / 4).max(CURSOR_DISPATCH_TAIL_MIN_DECAY_NS)
                    } else {
                        DISPATCH_TAIL_DECAY_NS
                    };
                    self.adaptive_tail_guard_ns = prior_guard.saturating_sub(decay);
                    self.age_anchor_ns = submit_returned_at_ns;
                    self.age_anchor_guard_ns = self.adaptive_tail_guard_ns;
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

    pub const fn adaptive_tail_guard_ns(&self) -> u64 {
        self.adaptive_tail_guard_ns
    }

    fn decay_cursor_history_toward_recent_clean_sample(&mut self) {
        let wake_floor = self.wake_lateness_ns.back().copied().unwrap_or(0);
        let pre_submit_floor = self.pre_submit_ns.back().copied().unwrap_or(0);
        let ioctl_floor = self.ioctl_duration_ns.back().copied().unwrap_or(0);
        let dispatch_floor = self.dispatch_duration_ns.back().copied().unwrap_or(0);
        decay_samples_toward(&mut self.wake_lateness_ns, wake_floor);
        decay_samples_toward(&mut self.pre_submit_ns, pre_submit_floor);
        decay_samples_toward(&mut self.ioctl_duration_ns, ioctl_floor);
        decay_samples_toward(&mut self.dispatch_duration_ns, dispatch_floor);
    }
}

fn push_bounded(samples: &mut VecDeque<u64>, sample: u64) {
    if samples.len() == SAMPLE_CAPACITY {
        samples.pop_front();
    }
    samples.push_back(sample);
}

fn decay_samples_toward(samples: &mut VecDeque<u64>, floor: u64) {
    for sample in samples {
        if *sample > floor {
            *sample = floor.saturating_add(sample.saturating_sub(floor) / 2);
        }
    }
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
    fn cursor_dispatch_guard_recovers_from_cursor_only_clean_submissions() {
        const REFRESH_NS: u64 = 1_000_000_000 / 165;
        let mut cursor = KmsWorkerDispatchModel::cursor();
        cursor.set_cursor_recovery_window(REFRESH_NS);

        for _ in 0..32 {
            cursor.observe_submission_deadline(1_000_000, 2_000_000, true, true);
        }
        let stressed = cursor.adaptive_tail_guard_ns();
        assert!(stressed > 0);

        for step in 0..12u64 {
            let clean_returned_at_ns = 2_000_000 + (step + 1) * REFRESH_NS;
            cursor.observe_submission_deadline(
                clean_returned_at_ns,
                clean_returned_at_ns,
                true,
                true,
            );
        }

        assert!(
            cursor.adaptive_tail_guard_ns() < stressed / 2,
            "clean cursor-only evidence should recover without primary frames"
        );
    }
    #[test]
    fn cursor_dispatch_percentiles_recover_from_stress_with_clean_cursor_samples() {
        const REFRESH_NS: u64 = 1_000_000_000 / 165;
        let mut model = KmsWorkerDispatchModel::cursor();
        model.set_cursor_recovery_window(REFRESH_NS);

        for _ in 0..SAMPLE_CAPACITY {
            model.record_fixed_timed_wake(25_000_000);
            model.record_worker_service(10_000_000, 10_000_000, 25_000_000);
            model.observe_submission_deadline(1_000_000, 26_000_000, true, true);
        }
        let stressed = model.budget().dispatch_budget_ns;
        assert!(stressed > 25_000_000);

        for step in 0..16u64 {
            let at_ns = 26_000_000 + (step + 1) * REFRESH_NS;
            model.record_fixed_timed_wake(50_000);
            model.record_worker_service(100_000, 200_000, 300_000);
            model.observe_submission_deadline(at_ns, at_ns, true, true);
        }

        assert!(
            model.budget().dispatch_budget_ns < stressed / 4,
            "cursor-only clean samples should reduce historical tail within 16 refreshes"
        );
    }

    #[test]
    fn dispatch_histories_remain_bounded_at_sample_capacity() {
        let mut model = KmsWorkerDispatchModel::default();
        for sample in 0..=SAMPLE_CAPACITY {
            model.record_fixed_timed_wake(sample as u64);
            model.record_worker_service(sample as u64, sample as u64, sample as u64);
        }

        assert_eq!(model.wake_lateness_ns.len(), SAMPLE_CAPACITY);
        assert_eq!(model.pre_submit_ns.len(), SAMPLE_CAPACITY);
        assert_eq!(model.ioctl_duration_ns.len(), SAMPLE_CAPACITY);
        assert_eq!(model.dispatch_duration_ns.len(), SAMPLE_CAPACITY);
    }

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

        model.record_fixed_timed_wake(10);
        model.record_worker_service(100, 200, 1_000);

        assert_eq!(model.budget().dispatch_budget_ns, 51_010);
    }

    #[test]
    fn variable_service_samples_do_not_evict_fixed_timed_wake_evidence() {
        let mut model = KmsWorkerDispatchModel::default();
        for _ in 0..SAMPLE_CAPACITY {
            model.record_fixed_timed_wake(400_000);
            model.record_worker_service(20_000, 30_000, 50_000);
        }
        let fixed_wake_lateness = model.budget().wake_lateness_ns;
        assert_eq!(fixed_wake_lateness, 400_000);

        // Variable jobs train service cost but have no fixed planned wake sample.
        for _ in 0..=SAMPLE_CAPACITY {
            model.record_worker_service(10_000, 15_000, 25_000);
        }

        let first_binding_fixed_budget = model.budget();
        assert_eq!(
            first_binding_fixed_budget.wake_lateness_ns,
            fixed_wake_lateness
        );
        assert_eq!(first_binding_fixed_budget.dispatch_budget_ns, 475_000);
    }

    #[test]
    fn proven_dispatch_deadline_miss_teaches_the_next_budget_immediately() {
        let mut model = KmsWorkerDispatchModel::default();
        model.record_worker_service(0, 100_000, 100_000);
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
        model.record_worker_service(0, 100_000, 100_000);
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
