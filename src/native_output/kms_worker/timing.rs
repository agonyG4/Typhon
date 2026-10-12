//! Bounded worker dispatch timing for submit-only Atomic jobs.

use std::collections::VecDeque;

use crate::native_output::presentation::kms_timing::KmsModeTimingKey;
use crate::native_output::runtime::AtomicCommitKind;

const SAMPLE_CAPACITY: usize = 120;
pub(crate) const DISPATCH_GUARD_NS: u64 = 50_000;
const DISPATCH_TAIL_SAFETY_QUANTUM_NS: u64 = 50_000;
const DISPATCH_TAIL_CLEAN_STREAK: u32 = 32;
const DISPATCH_TAIL_DECAY_NS: u64 = 50_000;
const CURSOR_DISPATCH_TAIL_CLEAN_STREAK: u32 = 2;
const CURSOR_DISPATCH_TAIL_MIN_DECAY_NS: u64 = 100_000;
const CURSOR_DISPATCH_TAIL_AGE_MIN_NS: u64 = 100_000_000;
const CURSOR_DISPATCH_TAIL_AGE_MAX_NS: u64 = 500_000_000;
pub(crate) const MAX_DISPATCH_TAIL_GUARD_NS: u64 = 1_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KmsServiceClass {
    Primary,
    CursorPlaneDelta,
    DirectScanout,
}

impl KmsServiceClass {
    pub(crate) const fn from_kind(kind: AtomicCommitKind) -> Self {
        match kind {
            AtomicCommitKind::CompositedPrimary { .. } => Self::Primary,
            AtomicCommitKind::PlaneDelta { .. } => Self::CursorPlaneDelta,
            AtomicCommitKind::DirectPrimary { .. } => Self::DirectScanout,
        }
    }
}

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
    fn cursor() -> Self {
        Self {
            clean_streak_limit: CURSOR_DISPATCH_TAIL_CLEAN_STREAK,
            fast_clean_recovery: true,
            age_window_ns: Some(CURSOR_DISPATCH_TAIL_AGE_MIN_NS),
            ..Self::default()
        }
    }

    fn reset_cursor_state(&mut self) {
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

    pub(crate) fn set_cursor_recovery_window(&mut self, refresh_interval_ns: u64) {
        if !self.fast_clean_recovery {
            return;
        }
        self.age_window_ns = Some(refresh_interval_ns.saturating_mul(8).clamp(
            CURSOR_DISPATCH_TAIL_AGE_MIN_NS,
            CURSOR_DISPATCH_TAIL_AGE_MAX_NS,
        ));
    }

    pub(crate) fn age_tail_guard(&mut self, now_ns: u64) {
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

    pub(crate) fn record_worker_service(
        &mut self,
        pre_submit_ns: u64,
        ioctl_duration_ns: u64,
        dispatch_duration_ns: u64,
    ) {
        push_bounded(&mut self.pre_submit_ns, pre_submit_ns);
        push_bounded(&mut self.ioctl_duration_ns, ioctl_duration_ns);
        push_bounded(&mut self.dispatch_duration_ns, dispatch_duration_ns);
    }

    pub(crate) fn record_fixed_timed_wake(&mut self, lateness_ns: u64) {
        push_bounded(&mut self.wake_lateness_ns, lateness_ns);
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

    pub(crate) const fn adaptive_tail_guard_ns(&self) -> u64 {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KmsDispatchModels {
    primary: KmsWorkerDispatchModel,
    cursor: KmsWorkerDispatchModel,
    direct: KmsWorkerDispatchModel,
    cursor_generation: Option<u64>,
    cursor_mode_key: Option<KmsModeTimingKey>,
}

impl Default for KmsDispatchModels {
    fn default() -> Self {
        Self {
            primary: KmsWorkerDispatchModel::default(),
            cursor: KmsWorkerDispatchModel::cursor(),
            direct: KmsWorkerDispatchModel::default(),
            cursor_generation: None,
            cursor_mode_key: None,
        }
    }
}

impl KmsDispatchModels {
    pub(crate) fn model_mut(&mut self, class: KmsServiceClass) -> &mut KmsWorkerDispatchModel {
        match class {
            KmsServiceClass::Primary => &mut self.primary,
            KmsServiceClass::CursorPlaneDelta => &mut self.cursor,
            KmsServiceClass::DirectScanout => &mut self.direct,
        }
    }

    pub(crate) fn model(&self, class: KmsServiceClass) -> &KmsWorkerDispatchModel {
        match class {
            KmsServiceClass::Primary => &self.primary,
            KmsServiceClass::CursorPlaneDelta => &self.cursor,
            KmsServiceClass::DirectScanout => &self.direct,
        }
    }

    pub(crate) fn rebind_cursor_generation(&mut self, generation: u64) {
        self.rebind_cursor_timing_domain(generation, None);
    }

    pub(crate) fn rebind_cursor_timing_domain(
        &mut self,
        generation: u64,
        mode_key: Option<KmsModeTimingKey>,
    ) {
        if self.cursor_generation != Some(generation) || self.cursor_mode_key != mode_key {
            self.cursor.reset_cursor_state();
            self.cursor_generation = Some(generation);
            self.cursor_mode_key = mode_key;
        }
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
    fn primary_and_cursor_dispatch_histories_are_independent() {
        let mut models = KmsDispatchModels::default();
        let cursor_budget = models
            .model(KmsServiceClass::CursorPlaneDelta)
            .budget()
            .dispatch_budget_ns;
        let primary_budget = models
            .model(KmsServiceClass::Primary)
            .budget()
            .dispatch_budget_ns;

        for _ in 0..16 {
            let primary = models.model_mut(KmsServiceClass::Primary);
            primary.record_fixed_timed_wake(25_000_000);
            primary.record_worker_service(10_000_000, 10_000_000, 25_000_000);
            primary.observe_submission_deadline(1_000_000, 26_000_000, true, true);
        }

        assert_eq!(
            models
                .model(KmsServiceClass::CursorPlaneDelta)
                .budget()
                .dispatch_budget_ns,
            cursor_budget
        );
        assert!(
            models
                .model(KmsServiceClass::Primary)
                .budget()
                .dispatch_budget_ns
                > primary_budget
        );
    }

    #[test]
    fn cursor_dispatch_guard_recovers_from_cursor_only_clean_submissions() {
        const REFRESH_NS: u64 = 1_000_000_000 / 165;
        let mut models = KmsDispatchModels::default();
        models.rebind_cursor_generation(8);
        let cursor = models.model_mut(KmsServiceClass::CursorPlaneDelta);
        cursor.set_cursor_recovery_window(REFRESH_NS);

        for _ in 0..32 {
            cursor.observe_submission_deadline(1_000_000, 2_000_000, true, true);
        }
        let stressed = cursor.adaptive_tail_guard_ns();
        assert!(stressed > 0);

        for step in 0..12u64 {
            let clean_returned_at_ns = 2_000_000 + (step + 1) * REFRESH_NS;
            let cursor = models.model_mut(KmsServiceClass::CursorPlaneDelta);
            cursor.observe_submission_deadline(
                clean_returned_at_ns,
                clean_returned_at_ns,
                true,
                true,
            );
        }

        assert!(
            models
                .model(KmsServiceClass::CursorPlaneDelta)
                .adaptive_tail_guard_ns()
                < stressed / 2,
            "clean cursor-only evidence should recover without primary frames"
        );
        assert_eq!(
            models
                .model(KmsServiceClass::Primary)
                .adaptive_tail_guard_ns(),
            0
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
    fn output_generation_rebind_resets_cursor_dispatch_history() {
        let mut models = KmsDispatchModels::default();
        models.rebind_cursor_generation(3);
        let initial_capacity = models
            .model(KmsServiceClass::CursorPlaneDelta)
            .wake_lateness_ns
            .capacity();
        let cursor = models.model_mut(KmsServiceClass::CursorPlaneDelta);
        cursor.record_fixed_timed_wake(10_000_000);
        cursor.record_worker_service(2_000_000, 3_000_000, 4_000_000);
        cursor.observe_submission_deadline(1_000_000, 2_000_000, true, true);
        assert!(
            models
                .model(KmsServiceClass::CursorPlaneDelta)
                .adaptive_tail_guard_ns()
                > 0
        );

        models.rebind_cursor_generation(4);

        let cursor = models.model(KmsServiceClass::CursorPlaneDelta);
        assert_eq!(cursor.adaptive_tail_guard_ns(), 0);
        assert_eq!(cursor.budget().wake_lateness_ns, 0);
        assert_eq!(cursor.budget().dispatch_budget_ns, DISPATCH_GUARD_NS);
        assert_eq!(cursor.wake_lateness_ns.capacity(), initial_capacity);
    }

    #[test]
    fn mode_rebind_resets_cursor_dispatch_history_without_changing_generation() {
        use crate::native_output::presentation::kms_timing::KmsModeTiming;
        use drm_sys::drm_mode_modeinfo;

        let first_mode = KmsModeTiming::from_mode(&drm_mode_modeinfo::default(), 16_000_000).key();
        let mut changed_mode = drm_mode_modeinfo::default();
        changed_mode.hdisplay = 1920;
        let second_mode = KmsModeTiming::from_mode(&changed_mode, 16_000_000).key();

        let mut models = KmsDispatchModels::default();
        models.rebind_cursor_timing_domain(3, Some(first_mode));
        let cursor = models.model_mut(KmsServiceClass::CursorPlaneDelta);
        cursor.record_fixed_timed_wake(10_000_000);
        cursor.record_worker_service(2_000_000, 3_000_000, 4_000_000);
        cursor.observe_submission_deadline(1_000_000, 2_000_000, true, true);
        assert!(
            models
                .model(KmsServiceClass::CursorPlaneDelta)
                .adaptive_tail_guard_ns()
                > 0
        );

        models.rebind_cursor_timing_domain(3, Some(second_mode));

        let cursor = models.model(KmsServiceClass::CursorPlaneDelta);
        assert_eq!(cursor.adaptive_tail_guard_ns(), 0);
        assert_eq!(cursor.budget().wake_lateness_ns, 0);
        assert_eq!(cursor.budget().dispatch_budget_ns, DISPATCH_GUARD_NS);
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
