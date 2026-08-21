use drm_sys::drm_mode_modeinfo;

const NANOS_PER_KHZ_PIXEL: u64 = 1_000_000;
const UNKNOWN_BLANKING_GUARD_NS: u64 = 1_000_000;
const INITIAL_ADAPTIVE_APPLY_GUARD_NS: u64 = 1_000_000;
const APPLY_GUARD_STEP_NS: u64 = 50_000;
const APPLY_HIT_DECAY_SAMPLES: u32 = 16;
const MAX_ADAPTIVE_APPLY_GUARD_NS: u64 = 3_000_000;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct KmsModeTimingKey {
    clock: u32,
    hdisplay: u16,
    hsync_start: u16,
    hsync_end: u16,
    htotal: u16,
    hskew: u16,
    vdisplay: u16,
    vsync_start: u16,
    vsync_end: u16,
    vtotal: u16,
    vscan: u16,
    flags: u32,
    type_: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct KmsModeTiming {
    key: KmsModeTimingKey,
    refresh_interval_ns: u64,
    blanking_interval_ns: Option<u64>,
}

impl KmsModeTiming {
    pub(crate) fn from_mode(mode: &drm_mode_modeinfo, fallback_refresh_interval_ns: u64) -> Self {
        let key = KmsModeTimingKey::from_mode(mode);
        let progressive = mode.clock != 0
            && mode.htotal >= mode.hdisplay
            && mode.vtotal >= mode.vdisplay
            && mode.htotal != 0
            && mode.vtotal != 0
            && mode.vscan <= 1
            && mode.flags & (drm_sys::DRM_MODE_FLAG_INTERLACE | drm_sys::DRM_MODE_FLAG_DBLSCAN)
                == 0;
        let refresh_interval_ns = progressive
            .then(|| {
                u64::from(mode.htotal)
                    .checked_mul(u64::from(mode.vtotal))
                    .and_then(|pixels| pixels.checked_mul(NANOS_PER_KHZ_PIXEL))
                    .and_then(|pixels| pixels.checked_div(u64::from(mode.clock)))
            })
            .flatten()
            .filter(|interval| *interval != 0)
            .unwrap_or(fallback_refresh_interval_ns.max(1));
        let blanking_interval_ns = progressive
            .then(|| {
                u64::from(mode.vtotal - mode.vdisplay)
                    .checked_mul(u64::from(mode.htotal))
                    .and_then(|pixels| pixels.checked_mul(NANOS_PER_KHZ_PIXEL))
                    .and_then(|pixels| pixels.checked_div(u64::from(mode.clock)))
            })
            .flatten();

        Self {
            key,
            refresh_interval_ns,
            blanking_interval_ns,
        }
    }

    pub(crate) const fn key(self) -> KmsModeTimingKey {
        self.key
    }

    pub(crate) const fn refresh_interval_ns(self) -> u64 {
        self.refresh_interval_ns
    }

    pub(crate) const fn blanking_interval_ns(self) -> Option<u64> {
        self.blanking_interval_ns
    }
}

impl KmsModeTimingKey {
    fn from_mode(mode: &drm_mode_modeinfo) -> Self {
        Self {
            clock: mode.clock,
            hdisplay: mode.hdisplay,
            hsync_start: mode.hsync_start,
            hsync_end: mode.hsync_end,
            htotal: mode.htotal,
            hskew: mode.hskew,
            vdisplay: mode.vdisplay,
            vsync_start: mode.vsync_start,
            vsync_end: mode.vsync_end,
            vtotal: mode.vtotal,
            vscan: mode.vscan,
            flags: mode.flags,
            type_: mode.type_,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct KmsSubmitWindow {
    mode_key: Option<KmsModeTimingKey>,
    target_presentation_ns: u64,
    earliest_submit_ns: u64,
    worker_wake_at_ns: u64,
    commit_complete_deadline_ns: u64,
    dispatch_budget_ns: u64,
    apply_guard_ns: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct KmsSubmitWindowError {
    earliest_submit_ns: u64,
    commit_complete_deadline_ns: u64,
}

impl KmsSubmitWindowError {
    #[allow(dead_code)]
    pub(crate) const fn earliest_submit_ns(self) -> u64 {
        self.earliest_submit_ns
    }

    #[allow(dead_code)]
    pub(crate) const fn commit_complete_deadline_ns(self) -> u64 {
        self.commit_complete_deadline_ns
    }
}

impl KmsSubmitWindow {
    pub(crate) fn try_new(
        target_presentation_ns: u64,
        earliest_submit_ns: u64,
        dispatch_budget_ns: u64,
        apply_guard_ns: u64,
    ) -> Result<Self, KmsSubmitWindowError> {
        let commit_complete_deadline_ns = target_presentation_ns.saturating_sub(apply_guard_ns);
        if earliest_submit_ns > commit_complete_deadline_ns {
            return Err(KmsSubmitWindowError {
                earliest_submit_ns,
                commit_complete_deadline_ns,
            });
        }
        let worker_wake_at_ns =
            earliest_submit_ns.max(commit_complete_deadline_ns.saturating_sub(dispatch_budget_ns));
        Ok(Self {
            mode_key: None,
            target_presentation_ns,
            earliest_submit_ns,
            worker_wake_at_ns,
            commit_complete_deadline_ns,
            dispatch_budget_ns,
            apply_guard_ns,
        })
    }

    pub(crate) const fn with_mode_key(mut self, mode_key: KmsModeTimingKey) -> Self {
        self.mode_key = Some(mode_key);
        self
    }

    pub(crate) const fn mode_key(self) -> Option<KmsModeTimingKey> {
        self.mode_key
    }

    #[allow(dead_code)]
    pub(crate) const fn target_presentation_ns(self) -> u64 {
        self.target_presentation_ns
    }

    #[allow(dead_code)]
    pub(crate) const fn earliest_submit_ns(self) -> u64 {
        self.earliest_submit_ns
    }

    pub(crate) const fn worker_wake_at_ns(self) -> u64 {
        self.worker_wake_at_ns
    }

    pub(crate) const fn commit_complete_deadline_ns(self) -> u64 {
        self.commit_complete_deadline_ns
    }

    #[allow(dead_code)]
    pub(crate) const fn dispatch_budget_ns(self) -> u64 {
        self.dispatch_budget_ns
    }

    #[allow(dead_code)]
    pub(crate) const fn apply_guard_ns(self) -> u64 {
        self.apply_guard_ns
    }

    pub(crate) const fn is_dispatch_miss_at(self, submit_returned_at_ns: u64) -> bool {
        submit_returned_at_ns > self.commit_complete_deadline_ns
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum KmsPresentationOutcome {
    TargetHit,
    RenderReadinessMiss,
    KmsDispatchMiss,
    KmsApplyGuardMiss,
}

impl KmsPresentationOutcome {
    pub(crate) const fn classify(
        window: &KmsSubmitWindow,
        payload_ready_at_ns: Option<u64>,
        submit_returned_at_ns: u64,
        target_sequence: u64,
        presented_sequence: u64,
    ) -> Self {
        if match payload_ready_at_ns {
            Some(ready) => ready > window.commit_complete_deadline_ns,
            None => false,
        } {
            Self::RenderReadinessMiss
        } else if window.is_dispatch_miss_at(submit_returned_at_ns) {
            Self::KmsDispatchMiss
        } else if presented_sequence > target_sequence {
            Self::KmsApplyGuardMiss
        } else {
            Self::TargetHit
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct KmsPresentationTimingModel {
    mode: KmsModeTiming,
    output_generation: u64,
    base_mode_guard_ns: u64,
    adaptive_apply_guard_ns: u64,
    stable_target_hits: u32,
    target_hits: u64,
    render_readiness_misses: u64,
    dispatch_misses: u64,
    apply_guard_misses: u64,
    unreachable_targets: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct KmsPresentationTimingSnapshot {
    pub(crate) target_hits: u64,
    pub(crate) render_readiness_misses: u64,
    pub(crate) dispatch_misses: u64,
    pub(crate) apply_guard_misses: u64,
    pub(crate) unreachable_targets: u64,
}

impl KmsPresentationTimingModel {
    pub(crate) fn new(mode: KmsModeTiming, output_generation: u64) -> Self {
        Self {
            mode,
            output_generation,
            base_mode_guard_ns: mode
                .blanking_interval_ns()
                .unwrap_or(UNKNOWN_BLANKING_GUARD_NS),
            adaptive_apply_guard_ns: INITIAL_ADAPTIVE_APPLY_GUARD_NS,
            stable_target_hits: 0,
            target_hits: 0,
            render_readiness_misses: 0,
            dispatch_misses: 0,
            apply_guard_misses: 0,
            unreachable_targets: 0,
        }
    }

    pub(crate) const fn mode(self) -> KmsModeTiming {
        self.mode
    }

    pub(crate) const fn base_mode_guard_ns(self) -> u64 {
        self.base_mode_guard_ns
    }

    pub(crate) const fn adaptive_apply_guard_ns(self) -> u64 {
        self.adaptive_apply_guard_ns
    }

    pub(crate) const fn apply_guard_ns(self) -> u64 {
        self.base_mode_guard_ns
            .saturating_add(self.adaptive_apply_guard_ns)
    }

    pub(crate) fn reconfigure(&mut self, mode: KmsModeTiming, output_generation: u64) -> bool {
        if self.mode.key() == mode.key() && self.output_generation == output_generation {
            return false;
        }
        *self = Self::new(mode, output_generation);
        true
    }

    pub(crate) fn submit_window(
        &self,
        target_presentation_ns: u64,
        earliest_submit_ns: u64,
        dispatch_budget_ns: u64,
    ) -> Result<KmsSubmitWindow, KmsSubmitWindowError> {
        KmsSubmitWindow::try_new(
            target_presentation_ns,
            earliest_submit_ns,
            dispatch_budget_ns,
            self.apply_guard_ns(),
        )
        .map(|window| window.with_mode_key(self.mode.key()))
    }

    pub(crate) fn record_unreachable_target(&mut self) {
        self.unreachable_targets = self.unreachable_targets.saturating_add(1);
    }

    pub(crate) const fn snapshot(self) -> KmsPresentationTimingSnapshot {
        KmsPresentationTimingSnapshot {
            target_hits: self.target_hits,
            render_readiness_misses: self.render_readiness_misses,
            dispatch_misses: self.dispatch_misses,
            apply_guard_misses: self.apply_guard_misses,
            unreachable_targets: self.unreachable_targets,
        }
    }

    pub(crate) fn observe_pageflip(
        &mut self,
        output_generation: u64,
        mode_key: KmsModeTimingKey,
        outcome: KmsPresentationOutcome,
    ) -> bool {
        if self.output_generation != output_generation || self.mode.key() != mode_key {
            return false;
        }
        match outcome {
            KmsPresentationOutcome::TargetHit => {
                self.target_hits = self.target_hits.saturating_add(1);
                self.stable_target_hits = self.stable_target_hits.saturating_add(1);
                if self.stable_target_hits >= APPLY_HIT_DECAY_SAMPLES {
                    self.stable_target_hits = 0;
                    let decay = (self.adaptive_apply_guard_ns / APPLY_HIT_DECAY_SAMPLES as u64)
                        .max(APPLY_GUARD_STEP_NS);
                    self.adaptive_apply_guard_ns =
                        self.adaptive_apply_guard_ns.saturating_sub(decay);
                }
            }
            KmsPresentationOutcome::RenderReadinessMiss => {
                self.render_readiness_misses = self.render_readiness_misses.saturating_add(1);
                self.stable_target_hits = 0;
            }
            KmsPresentationOutcome::KmsDispatchMiss => {
                self.dispatch_misses = self.dispatch_misses.saturating_add(1);
                self.stable_target_hits = 0;
            }
            KmsPresentationOutcome::KmsApplyGuardMiss => {
                self.apply_guard_misses = self.apply_guard_misses.saturating_add(1);
                self.stable_target_hits = 0;
                self.adaptive_apply_guard_ns = self
                    .adaptive_apply_guard_ns
                    .saturating_add(APPLY_GUARD_STEP_NS)
                    .min(MAX_ADAPTIVE_APPLY_GUARD_NS);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_output::presentation::kms_timing::{
        KmsModeTiming, KmsPresentationOutcome, KmsPresentationTimingModel, KmsSubmitWindow,
    };

    fn mode(clock: u32, htotal: u16, vtotal: u16) -> drm_mode_modeinfo {
        drm_mode_modeinfo {
            clock,
            hdisplay: 1920,
            hsync_start: 2008,
            hsync_end: 2052,
            htotal,
            vdisplay: 1080,
            vsync_start: 1084,
            vsync_end: 1089,
            vtotal,
            ..drm_mode_modeinfo::default()
        }
    }

    #[test]
    fn same_refresh_with_different_blanking_has_different_mode_identity() {
        let reduced = KmsModeTiming::from_mode(&mode(325_000, 2080, 1111), 6_060_606);
        let expanded = KmsModeTiming::from_mode(&mode(325_000, 2200, 1050), 6_060_606);

        assert_ne!(reduced.key(), expanded.key());
    }

    #[test]
    fn malformed_mode_does_not_panic_or_claim_known_blanking() {
        let timing = KmsModeTiming::from_mode(&mode(0, 0, 0), 6_060_606);

        assert_eq!(timing.blanking_interval_ns(), None);
        assert_eq!(timing.refresh_interval_ns(), 6_060_606);
    }

    #[test]
    fn earliest_submit_is_not_a_submit_deadline() {
        let window = KmsSubmitWindow::try_new(100, 40, 20, 20).unwrap();

        assert_eq!(window.earliest_submit_ns(), 40);
        assert_eq!(window.commit_complete_deadline_ns(), 80);
        assert_eq!(window.worker_wake_at_ns(), 60);
        assert!(!window.is_dispatch_miss_at(70));
    }

    #[test]
    fn impossible_window_is_explicit() {
        let error = KmsSubmitWindow::try_new(100, 90, 20, 20).unwrap_err();

        assert_eq!(error.earliest_submit_ns(), 90);
        assert_eq!(error.commit_complete_deadline_ns(), 80);
    }

    #[test]
    fn late_worker_wake_before_completion_deadline_is_not_dispatch_miss() {
        let window = KmsSubmitWindow::try_new(100, 40, 20, 20).unwrap();

        assert_eq!(
            KmsPresentationOutcome::classify(&window, Some(50), 79, 4, 4),
            KmsPresentationOutcome::TargetHit
        );
    }

    #[test]
    fn submit_after_completion_deadline_is_dispatch_miss() {
        let window = KmsSubmitWindow::try_new(100, 40, 20, 20).unwrap();

        assert_eq!(
            KmsPresentationOutcome::classify(&window, Some(50), 81, 4, 5),
            KmsPresentationOutcome::KmsDispatchMiss
        );
    }

    #[test]
    fn on_time_submit_with_target_miss_is_apply_guard_miss() {
        let window = KmsSubmitWindow::try_new(100, 40, 20, 20).unwrap();

        assert_eq!(
            KmsPresentationOutcome::classify(&window, Some(50), 79, 4, 5),
            KmsPresentationOutcome::KmsApplyGuardMiss
        );
    }

    #[test]
    fn late_payload_is_render_readiness_miss_before_worker_blame() {
        let window = KmsSubmitWindow::try_new(100, 40, 20, 20).unwrap();

        assert_eq!(
            KmsPresentationOutcome::classify(&window, Some(81), 90, 4, 5),
            KmsPresentationOutcome::RenderReadinessMiss
        );
    }

    #[test]
    fn apply_guard_learning_is_runtime_owned_and_bounded() {
        let timing = KmsModeTiming::from_mode(&mode(325_000, 2200, 1125), 6_060_606);
        let mut model = KmsPresentationTimingModel::new(timing, 7);
        let initial = model.adaptive_apply_guard_ns();

        assert!(model.observe_pageflip(7, timing.key(), KmsPresentationOutcome::KmsApplyGuardMiss));
        assert!(model.adaptive_apply_guard_ns() > initial);
        let after_miss = model.adaptive_apply_guard_ns();
        for _ in 0..16 {
            assert!(model.observe_pageflip(7, timing.key(), KmsPresentationOutcome::TargetHit));
        }
        assert!(model.adaptive_apply_guard_ns() < after_miss);
    }

    #[test]
    fn stale_pageflip_cannot_tune_current_apply_model() {
        let old_timing = KmsModeTiming::from_mode(&mode(325_000, 2200, 1125), 6_060_606);
        let new_timing = KmsModeTiming::from_mode(&mode(326_000, 2200, 1125), 6_060_606);
        let mut model = KmsPresentationTimingModel::new(new_timing, 8);
        let guard = model.adaptive_apply_guard_ns();

        assert!(!model.observe_pageflip(
            7,
            old_timing.key(),
            KmsPresentationOutcome::KmsApplyGuardMiss
        ));
        assert_eq!(model.adaptive_apply_guard_ns(), guard);
    }
}
