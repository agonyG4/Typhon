use super::kms_timing::{KmsModeTiming, KmsModeTimingKey, KmsSubmitWindow};
use oblivion_one::native::{kms::PageFlipToken, presentation_deadline::PresentationTarget};

const PREPARATION_SAMPLE_CAPACITY: usize = 64;
const PREPARATION_COLD_START_NS: u64 = 1_000_000;
const PREPARATION_SAFETY_FLOOR_NS: u64 = 100_000;
const PREPARATION_MIN_BUDGET_NS: u64 = 200_000;
const PREPARATION_MAX_BUDGET_NS: u64 = 10_000_000;
const CURSOR_GUARD_SAFETY_NS: u64 = 50_000;
const CURSOR_GUARD_CLEAN_FRACTION: u64 = 8;
const CURSOR_GUARD_AGE_MIN_NS: u64 = 100_000_000;
const CURSOR_GUARD_AGE_MAX_NS: u64 = 500_000_000;
const UNKNOWN_CURSOR_APPLY_BASELINE_NS: u64 = 1_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CursorKmsOutcome {
    TargetHit,
    DispatchMiss { lateness_ns: u64 },
    ApplyMiss { additional_lead_ns: u64 },
    LatePayload,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CursorKmsSubmissionEvidence {
    pub(crate) token: PageFlipToken,
    pub(crate) output_generation: u64,
    pub(crate) target_sequence: u64,
    pub(crate) target_presentation_ns: u64,
    pub(crate) target_refresh_interval_ns: u64,
    pub(crate) window: KmsSubmitWindow,
    pub(crate) queued_at_ns: u64,
    pub(crate) service_started_at_ns: u64,
    pub(crate) submit_returned_at_ns: u64,
}

impl CursorKmsSubmissionEvidence {
    pub(crate) fn new(
        token: PageFlipToken,
        output_generation: u64,
        target: PresentationTarget,
        window: KmsSubmitWindow,
        queued_at_ns: u64,
        service_started_at_ns: u64,
        submit_returned_at_ns: u64,
    ) -> Self {
        Self {
            token,
            output_generation,
            target_sequence: target.sequence,
            target_presentation_ns: target.presentation_time.get(),
            target_refresh_interval_ns: target.refresh_interval.as_nanos().min(u128::from(u64::MAX))
                as u64,
            window,
            queued_at_ns,
            service_started_at_ns,
            submit_returned_at_ns,
        }
    }

    pub(crate) fn classify(self, actual_sequence: u64) -> CursorKmsOutcome {
        classify_cursor_kms_outcome(
            self.window,
            self.queued_at_ns,
            self.service_started_at_ns,
            self.submit_returned_at_ns,
            self.target_sequence,
            actual_sequence,
            self.target_refresh_interval_ns,
        )
    }
}

pub(crate) fn classify_cursor_kms_outcome(
    window: KmsSubmitWindow,
    queued_at_ns: u64,
    service_started_at_ns: u64,
    submit_returned_at_ns: u64,
    target_sequence: u64,
    actual_sequence: u64,
    refresh_interval_ns: u64,
) -> CursorKmsOutcome {
    if actual_sequence <= target_sequence {
        CursorKmsOutcome::TargetHit
    } else if queued_at_ns > window.commit_complete_deadline_ns()
        || service_started_at_ns > window.commit_complete_deadline_ns()
    {
        CursorKmsOutcome::LatePayload
    } else if submit_returned_at_ns > window.commit_complete_deadline_ns() {
        CursorKmsOutcome::DispatchMiss {
            lateness_ns: submit_returned_at_ns.saturating_sub(window.commit_complete_deadline_ns()),
        }
    } else {
        CursorKmsOutcome::ApplyMiss {
            additional_lead_ns: actual_sequence
                .saturating_sub(target_sequence)
                .saturating_mul(refresh_interval_ns),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CursorKmsTimingSnapshot {
    pub(crate) target_hits: u64,
    pub(crate) dispatch_misses: u64,
    pub(crate) apply_misses: u64,
    pub(crate) unreachable_targets: u64,
    pub(crate) recovery_clean_hits: u64,
    pub(crate) guard_increases: u64,
    pub(crate) guard_decays: u64,
    pub(crate) guard_age_ms: u64,
    pub(crate) base_apply_guard_ns: u64,
    pub(crate) adaptive_excess_ns: u64,
    pub(crate) apply_guard_ns: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CursorKmsTimingModel {
    mode: KmsModeTiming,
    output_generation: u64,
    base_apply_guard_ns: u64,
    adaptive_excess_ns: u64,
    age_anchor_ns: u64,
    age_anchor_excess_ns: u64,
    last_miss_at_ns: Option<u64>,
    target_hits: u64,
    dispatch_misses: u64,
    apply_misses: u64,
    unreachable_targets: u64,
    recovery_clean_hits: u64,
    guard_increases: u64,
    guard_decays: u64,
}

impl CursorKmsTimingModel {
    pub(crate) fn new(mode: KmsModeTiming, output_generation: u64) -> Self {
        Self {
            mode,
            output_generation,
            base_apply_guard_ns: mode
                .blanking_interval_ns()
                .unwrap_or(UNKNOWN_CURSOR_APPLY_BASELINE_NS),
            adaptive_excess_ns: 0,
            age_anchor_ns: 0,
            age_anchor_excess_ns: 0,
            last_miss_at_ns: None,
            target_hits: 0,
            dispatch_misses: 0,
            apply_misses: 0,
            unreachable_targets: 0,
            recovery_clean_hits: 0,
            guard_increases: 0,
            guard_decays: 0,
        }
    }

    pub(crate) const fn mode_key(self) -> KmsModeTimingKey {
        self.mode.key()
    }

    pub(crate) const fn output_generation(self) -> u64 {
        self.output_generation
    }

    pub(crate) fn apply_guard_ns(&self, now_ns: u64) -> u64 {
        self.base_apply_guard_ns
            .saturating_add(self.effective_excess_at(now_ns))
    }

    pub(crate) fn observe(&mut self, outcome: CursorKmsOutcome, now_ns: u64) {
        self.adaptive_excess_ns = self.effective_excess_at(now_ns);
        self.reset_age_anchor(now_ns);
        match outcome {
            CursorKmsOutcome::TargetHit => {
                self.target_hits = self.target_hits.saturating_add(1);
                self.recovery_clean_hits = self.recovery_clean_hits.saturating_add(1);
                let before = self.adaptive_excess_ns;
                let decay = (before / CURSOR_GUARD_CLEAN_FRACTION).max(CURSOR_GUARD_SAFETY_NS);
                self.adaptive_excess_ns = before.saturating_sub(decay);
                if before > self.adaptive_excess_ns {
                    self.guard_decays = self.guard_decays.saturating_add(1);
                }
                self.reset_age_anchor(now_ns);
            }
            CursorKmsOutcome::DispatchMiss { lateness_ns } => {
                self.dispatch_misses = self.dispatch_misses.saturating_add(1);
                self.increase_excess(lateness_ns, now_ns);
            }
            CursorKmsOutcome::ApplyMiss { additional_lead_ns } => {
                self.apply_misses = self.apply_misses.saturating_add(1);
                self.increase_excess(additional_lead_ns, now_ns);
            }
            CursorKmsOutcome::LatePayload => {
                self.unreachable_targets = self.unreachable_targets.saturating_add(1);
            }
        }
    }

    pub(crate) fn reconfigure(&mut self, mode: KmsModeTiming, output_generation: u64) -> bool {
        if self.mode.key() == mode.key() && self.output_generation == output_generation {
            return false;
        }
        *self = Self::new(mode, output_generation);
        true
    }

    pub(crate) fn snapshot(&self, now_ns: u64) -> CursorKmsTimingSnapshot {
        let adaptive_excess_ns = self.effective_excess_at(now_ns);
        let guard_age_ms = self
            .last_miss_at_ns
            .map_or(0, |last| now_ns.saturating_sub(last) / 1_000_000);
        CursorKmsTimingSnapshot {
            target_hits: self.target_hits,
            dispatch_misses: self.dispatch_misses,
            apply_misses: self.apply_misses,
            unreachable_targets: self.unreachable_targets,
            recovery_clean_hits: self.recovery_clean_hits,
            guard_increases: self.guard_increases,
            guard_decays: self.guard_decays,
            guard_age_ms,
            base_apply_guard_ns: self.base_apply_guard_ns,
            adaptive_excess_ns,
            apply_guard_ns: self.base_apply_guard_ns.saturating_add(adaptive_excess_ns),
        }
    }

    fn increase_excess(&mut self, measured_lateness_ns: u64, now_ns: u64) {
        let max_excess_ns = self
            .mode
            .refresh_interval_ns()
            .saturating_div(2)
            .max(3_000_000);
        let requested = measured_lateness_ns
            .max(CURSOR_GUARD_SAFETY_NS)
            .saturating_add(CURSOR_GUARD_SAFETY_NS);
        let before = self.adaptive_excess_ns;
        self.adaptive_excess_ns = before.saturating_add(requested).min(max_excess_ns);
        if self.adaptive_excess_ns > before {
            self.guard_increases = self.guard_increases.saturating_add(1);
        }
        self.last_miss_at_ns = Some(now_ns);
        self.recovery_clean_hits = 0;
        self.reset_age_anchor(now_ns);
    }

    fn effective_excess_at(&self, now_ns: u64) -> u64 {
        if self.adaptive_excess_ns == 0 {
            return 0;
        }
        let age_window_ns = self
            .mode
            .refresh_interval_ns()
            .saturating_mul(8)
            .clamp(CURSOR_GUARD_AGE_MIN_NS, CURSOR_GUARD_AGE_MAX_NS);
        let elapsed_ns = now_ns.saturating_sub(self.age_anchor_ns);
        if elapsed_ns == 0 {
            return self.adaptive_excess_ns;
        }
        if elapsed_ns >= age_window_ns {
            0
        } else {
            self.age_anchor_excess_ns
                .saturating_mul(age_window_ns.saturating_sub(elapsed_ns))
                / age_window_ns
        }
    }

    fn reset_age_anchor(&mut self, now_ns: u64) {
        self.age_anchor_ns = now_ns;
        self.age_anchor_excess_ns = self.adaptive_excess_ns;
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CursorPreparationTimingModel {
    samples_ns: [u64; PREPARATION_SAMPLE_CAPACITY],
    len: usize,
    next: usize,
}

impl Default for CursorPreparationTimingModel {
    fn default() -> Self {
        Self {
            samples_ns: [0; PREPARATION_SAMPLE_CAPACITY],
            len: 0,
            next: 0,
        }
    }
}

impl CursorPreparationTimingModel {
    pub(crate) fn observe(&mut self, admitted_at_ns: u64, enqueued_at_ns: u64) {
        let sample_ns = enqueued_at_ns.saturating_sub(admitted_at_ns);
        if self.len > 0 {
            // A fresh lower preparation sample is direct recovery evidence.
            // Pull old stress samples toward it geometrically while retaining
            // the bounded percentile estimator and fixed-capacity storage.
            for prior in &mut self.samples_ns[..self.len] {
                if *prior > sample_ns {
                    *prior = sample_ns.saturating_add(prior.saturating_sub(sample_ns) / 2);
                }
            }
        }
        self.samples_ns[self.next] = sample_ns;
        self.next = (self.next + 1) % PREPARATION_SAMPLE_CAPACITY;
        self.len = (self.len + 1).min(PREPARATION_SAMPLE_CAPACITY);
    }

    pub(crate) fn budget_ns(&self) -> u64 {
        if self.len == 0 {
            return PREPARATION_COLD_START_NS;
        }
        let mut ordered = self.samples_ns;
        ordered[..self.len].sort_unstable();
        let rank = self.len.saturating_mul(95).saturating_add(99) / 100;
        ordered[rank.saturating_sub(1).min(self.len - 1)]
            .saturating_add(PREPARATION_SAFETY_FLOOR_NS)
            .clamp(PREPARATION_MIN_BUDGET_NS, PREPARATION_MAX_BUDGET_NS)
    }

    #[cfg(test)]
    fn sample_count(&self) -> usize {
        self.len
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CursorTimingDomain {
    preparation: CursorPreparationTimingModel,
    kms: CursorKmsTimingModel,
    synchronous_submission: Option<CursorKmsSubmissionEvidence>,
}

impl CursorTimingDomain {
    pub(crate) fn new(mode: KmsModeTiming, output_generation: u64) -> Self {
        Self {
            preparation: CursorPreparationTimingModel::default(),
            kms: CursorKmsTimingModel::new(mode, output_generation),
            synchronous_submission: None,
        }
    }

    pub(crate) fn preparation_budget_ns(&self) -> u64 {
        self.preparation.budget_ns()
    }

    pub(crate) fn observe_preparation(&mut self, admitted_at_ns: u64, enqueued_at_ns: u64) {
        self.preparation.observe(admitted_at_ns, enqueued_at_ns);
    }

    pub(crate) fn apply_guard_ns(&self, now_ns: u64) -> u64 {
        self.kms.apply_guard_ns(now_ns)
    }

    pub(crate) fn observe_kms(&mut self, outcome: CursorKmsOutcome, now_ns: u64) {
        self.kms.observe(outcome, now_ns);
    }

    pub(crate) fn note_synchronous_submission(&mut self, evidence: CursorKmsSubmissionEvidence) {
        debug_assert!(self.synchronous_submission.is_none());
        self.synchronous_submission = Some(evidence);
    }

    pub(crate) fn take_synchronous_submission(
        &mut self,
        token: PageFlipToken,
        output_generation: u64,
    ) -> Option<CursorKmsSubmissionEvidence> {
        if self.synchronous_submission.is_some_and(|evidence| {
            evidence.token == token && evidence.output_generation == output_generation
        }) {
            self.synchronous_submission.take()
        } else {
            None
        }
    }

    pub(crate) fn reconfigure(&mut self, mode: KmsModeTiming, output_generation: u64) -> bool {
        if self.kms.reconfigure(mode, output_generation) {
            self.preparation = CursorPreparationTimingModel::default();
            self.synchronous_submission = None;
            true
        } else {
            false
        }
    }

    pub(crate) fn snapshot(&self, now_ns: u64) -> CursorKmsTimingSnapshot {
        self.kms.snapshot(now_ns)
    }

    pub(crate) const fn mode_key(&self) -> KmsModeTimingKey {
        self.kms.mode_key()
    }

    pub(crate) const fn output_generation(&self) -> u64 {
        self.kms.output_generation()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_output::presentation::kms_timing::KmsModeTiming;
    use drm_sys::drm_mode_modeinfo;

    #[test]
    fn preparation_budget_uses_bounded_p95_and_a_cold_start_floor() {
        let mut model = CursorPreparationTimingModel::default();
        assert_eq!(model.budget_ns(), PREPARATION_COLD_START_NS);
        for sample in [100_000, 200_000, 300_000, 400_000] {
            model.observe(10, 10 + sample);
        }
        assert_eq!(model.budget_ns(), 500_000);
        assert_eq!(model.sample_count(), 4);
    }

    #[test]
    fn clean_preparation_samples_recover_stress_tail_quickly() {
        let mut model = CursorPreparationTimingModel::default();
        model.observe(1, 10_000_001);
        let stressed = model.budget_ns();
        assert!(stressed > 5_000_000);

        for step in 0..8u64 {
            model.observe(step * 1_000_000, step * 1_000_000 + 400_000);
        }

        assert!(
            model.budget_ns() < stressed / 8,
            "clean cursor preparation evidence must reduce the old tail quickly"
        );
    }

    #[test]
    fn cursor_clean_hits_recover_maximum_apply_excess_without_primary_frames() {
        const REFRESH_NS: u64 = 1_000_000_000 / 165;
        let mode = KmsModeTiming::from_mode(&drm_mode_modeinfo::default(), REFRESH_NS);
        let mut model = CursorKmsTimingModel::new(mode, 7);
        let baseline = model.apply_guard_ns(0);

        for step in 0..128 {
            model.observe(
                CursorKmsOutcome::ApplyMiss {
                    additional_lead_ns: REFRESH_NS,
                },
                step * REFRESH_NS,
            );
        }
        let stressed = model.apply_guard_ns(128 * REFRESH_NS);
        assert!(stressed > baseline);

        for step in 0..16 {
            model.observe(CursorKmsOutcome::TargetHit, (128 + step) * REFRESH_NS);
        }
        let recovered = model.apply_guard_ns(144 * REFRESH_NS);
        assert!(recovered < stressed / 2 + baseline / 2);
    }

    #[test]
    fn cursor_stale_stress_ages_to_baseline_without_a_timer() {
        const REFRESH_NS: u64 = 1_000_000_000 / 60;
        let mode = KmsModeTiming::from_mode(&drm_mode_modeinfo::default(), REFRESH_NS);
        let mut model = CursorKmsTimingModel::new(mode, 3);
        let baseline = model.apply_guard_ns(0);
        model.observe(
            CursorKmsOutcome::ApplyMiss {
                additional_lead_ns: 10_000_000,
            },
            1,
        );

        assert!(model.apply_guard_ns(1_000_000) > baseline);
        assert_eq!(model.apply_guard_ns(1_000_000_000), baseline);
    }

    #[test]
    fn late_cursor_payload_does_not_inflate_apply_guard() {
        const REFRESH_NS: u64 = 1_000_000_000 / 120;
        let mode = KmsModeTiming::from_mode(&drm_mode_modeinfo::default(), REFRESH_NS);
        let mut model = CursorKmsTimingModel::new(mode, 5);
        let baseline = model.apply_guard_ns(0);

        model.observe(CursorKmsOutcome::LatePayload, 1_000_000);

        assert_eq!(model.apply_guard_ns(1_000_000), baseline);
        assert_eq!(model.snapshot(1_000_000).unreachable_targets, 1);
    }

    #[test]
    fn output_generation_change_clears_cursor_stress_and_preparation_history() {
        const REFRESH_NS: u64 = 1_000_000_000 / 144;
        let first_mode = KmsModeTiming::from_mode(&drm_mode_modeinfo::default(), REFRESH_NS);
        let mut domain = CursorTimingDomain::new(first_mode, 4);
        domain.observe_preparation(10, 2_010_010);
        domain.observe_kms(
            CursorKmsOutcome::ApplyMiss {
                additional_lead_ns: REFRESH_NS,
            },
            1_000_000,
        );
        assert!(domain.apply_guard_ns(1_000_000) > domain.snapshot(0).base_apply_guard_ns);
        assert!(domain.preparation_budget_ns() > PREPARATION_COLD_START_NS);

        assert!(domain.reconfigure(first_mode, 5));
        assert_eq!(domain.output_generation(), 5);
        assert_eq!(
            domain.apply_guard_ns(1_000_000),
            domain.snapshot(0).base_apply_guard_ns
        );
        assert_eq!(domain.preparation_budget_ns(), PREPARATION_COLD_START_NS);
    }

    #[test]
    fn cursor_pageflip_evidence_separates_hit_dispatch_apply_and_late_payload() {
        let window = KmsSubmitWindow::late_latch(10_000_000, 1_000_000, 1_000_000, 1_000_000);
        let target = 20;
        assert_eq!(
            classify_cursor_kms_outcome(
                window, 2_000_000, 8_000_000, 8_500_000, target, target, 10_000_000,
            ),
            CursorKmsOutcome::TargetHit
        );
        assert_eq!(
            classify_cursor_kms_outcome(
                window,
                2_000_000,
                8_500_000,
                9_050_000,
                target,
                target + 1,
                10_000_000,
            ),
            CursorKmsOutcome::DispatchMiss {
                lateness_ns: 50_000
            }
        );
        assert_eq!(
            classify_cursor_kms_outcome(
                window,
                2_000_000,
                8_000_000,
                8_000_000,
                target,
                target + 1,
                10_000_000,
            ),
            CursorKmsOutcome::ApplyMiss {
                additional_lead_ns: 10_000_000
            }
        );
        assert_eq!(
            classify_cursor_kms_outcome(
                window,
                window.commit_complete_deadline_ns() - 1,
                window.commit_complete_deadline_ns() + 1,
                window.commit_complete_deadline_ns() + 2,
                target,
                target + 1,
                10_000_000,
            ),
            CursorKmsOutcome::LatePayload
        );
        assert_eq!(
            classify_cursor_kms_outcome(
                window,
                2_000_000,
                window.commit_complete_deadline_ns() + 1,
                window.commit_complete_deadline_ns() + 2,
                target,
                target + 1,
                10_000_000,
            ),
            CursorKmsOutcome::LatePayload
        );
    }
}
