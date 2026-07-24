//! Deadline risk prediction and bounded adaptive-buffering policy.
#![allow(dead_code)] // Wired into the native runtime in Task 12.

use crate::native::presentation_deadline::{MonotonicTimestampNs, PresentationTarget};
use crate::native::scheduler::NativeOutputPacingMode;
use std::collections::VecDeque;
use std::time::Duration;

const SAMPLE_CAPACITY: usize = 120;
const MIN_HYSTERESIS_PRESENTATIONS: u64 = 10;
const MIN_HYSTERESIS_NS: u64 = 100_000_000;

#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdaptiveTripleBufferPolicy {
    Auto,
    Off,
    Force,
}

impl AdaptiveTripleBufferPolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Off => "off",
            Self::Force => "force",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "auto" => Ok(Self::Auto),
            "off" => Ok(Self::Off),
            "force" => Ok(Self::Force),
            other => Err(format!(
                "invalid OBLIVION_ONE_TRIPLE_BUFFERING value '{other}'; accepted values: auto, off, force"
            )),
        }
    }
}

#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdaptiveBufferingMode {
    Double,
    Triple,
}

#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TripleEntryReason {
    PredictedDeadlinePressure,
    ProvenReadinessMiss,
    ProvenSubmitMiss,
    ProvenPresentationMiss,
    ForcedValidation,
}

#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvenDeadlineMiss {
    ExactRender,
    GuardedApproximateRender,
    AtomicSubmit,
    Presentation,
}

#[doc(hidden)]
pub fn merge_presentation_miss(
    existing: Option<ProvenDeadlineMiss>,
    planned_sequence: u64,
    actual_sequence: u64,
) -> Option<ProvenDeadlineMiss> {
    existing.or_else(|| {
        (actual_sequence > planned_sequence).then_some(ProvenDeadlineMiss::Presentation)
    })
}

#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FenceTimestampQuality {
    ExactSyncFile,
    ObservedApproximate,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct FrameTimingObservation {
    pub(crate) frame_id: u64,
    pub(crate) target: PresentationTarget,
    pub(crate) composite_started_at: MonotonicTimestampNs,
    pub(crate) fence_exported_at: MonotonicTimestampNs,
    pub(crate) fence_signaled_at: Option<(MonotonicTimestampNs, FenceTimestampQuality)>,
    pub(crate) submit_started_at: Option<MonotonicTimestampNs>,
    pub(crate) submit_returned_at: Option<MonotonicTimestampNs>,
}

pub fn render_sample_duration_ns(
    composite_started_at: MonotonicTimestampNs,
    fence_signaled_at: MonotonicTimestampNs,
) -> u64 {
    fence_signaled_at
        .get()
        .saturating_sub(composite_started_at.get())
}

#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderPrediction {
    pub ewma_render_ns: u64,
    pub upper_render_deviation_ns: u64,
    pub p90_recent_render_ns: u64,
    pub render_risk_ns: u64,
    pub p95_wake_lateness_ns: u64,
    pub p95_atomic_submit_ns: u64,
    pub submit_allowance_ns: u64,
    pub safety_margin_ns: u64,
    pub total_cost_ns: u64,
    pub idle_wake_guard: bool,
}

#[doc(hidden)]
#[derive(Debug)]
pub struct AdaptiveRenderJournal {
    render_samples_ns: VecDeque<u64>,
    wake_lateness_samples_ns: VecDeque<u64>,
    atomic_submit_samples_ns: VecDeque<u64>,
    ewma_render_ns: u64,
    upper_render_deviation_ns: u64,
    last_sample_at: Option<MonotonicTimestampNs>,
    last_presented_at: Option<MonotonicTimestampNs>,
    idle_guard_consumed: bool,
    pub(crate) missed_deadlines: u64,
}

impl Default for AdaptiveRenderJournal {
    fn default() -> Self {
        Self {
            render_samples_ns: VecDeque::with_capacity(SAMPLE_CAPACITY),
            wake_lateness_samples_ns: VecDeque::with_capacity(SAMPLE_CAPACITY),
            atomic_submit_samples_ns: VecDeque::with_capacity(SAMPLE_CAPACITY),
            ewma_render_ns: 0,
            upper_render_deviation_ns: 0,
            last_sample_at: None,
            last_presented_at: None,
            idle_guard_consumed: false,
            missed_deadlines: 0,
        }
    }
}

impl AdaptiveRenderJournal {
    pub fn record_render_sample(&mut self, sample_ns: u64, at: MonotonicTimestampNs) {
        if let Some(previous_at) = self.last_sample_at {
            let previous_mean = self.ewma_render_ns;
            let previous_deviation = self.upper_render_deviation_ns;
            let dt = at.get().saturating_sub(previous_at.get());
            let positive_error = sample_ns.saturating_sub(previous_mean);
            let (deviation_num, deviation_den) = deviation_alpha(dt);
            self.upper_render_deviation_ns = mix_rounded(
                positive_error,
                previous_deviation,
                deviation_num,
                deviation_den,
            )
            .max(positive_error);
            let (mean_num, mean_den) = mean_alpha(dt);
            self.ewma_render_ns = mix_rounded(sample_ns, previous_mean, mean_num, mean_den);
        } else {
            self.ewma_render_ns = sample_ns;
            self.upper_render_deviation_ns = 0;
        }
        push_bounded(&mut self.render_samples_ns, sample_ns);
        self.last_sample_at = Some(at);
    }

    pub fn record_wake_lateness(&mut self, sample_ns: u64) {
        push_bounded(&mut self.wake_lateness_samples_ns, sample_ns);
    }

    pub fn record_atomic_submit(&mut self, sample_ns: u64) {
        push_bounded(&mut self.atomic_submit_samples_ns, sample_ns);
    }

    pub fn note_matching_presentation(&mut self, at: MonotonicTimestampNs) {
        self.last_presented_at = Some(at);
    }

    pub fn prediction(&self, refresh_interval: Duration) -> RenderPrediction {
        self.base_prediction(refresh_interval, false)
    }

    pub fn prediction_at(
        &mut self,
        now: MonotonicTimestampNs,
        refresh_interval: Duration,
    ) -> RenderPrediction {
        let refresh_ns = duration_ns(refresh_interval).max(1);
        let idle = !self.idle_guard_consumed
            && self.last_presented_at.is_some_and(|last| {
                now.get().saturating_sub(last.get()) >= refresh_ns.saturating_mul(100)
            });
        if idle {
            self.idle_guard_consumed = true;
        }
        self.base_prediction(refresh_interval, idle)
    }

    pub(crate) const fn ewma_render_ns(&self) -> u64 {
        self.ewma_render_ns
    }

    pub(crate) const fn upper_render_deviation_ns(&self) -> u64 {
        self.upper_render_deviation_ns
    }

    pub(crate) fn p90_recent_render_ns(&self) -> u64 {
        nearest_rank(&self.render_samples_ns, 90)
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    fn base_prediction(&self, refresh_interval: Duration, idle: bool) -> RenderPrediction {
        let refresh_ns = duration_ns(refresh_interval).max(1);
        let p90 = self.p90_recent_render_ns();
        let central_risk = self
            .ewma_render_ns
            .saturating_add(self.upper_render_deviation_ns.saturating_mul(2));
        let startup = (refresh_ns / 2).min(4_000_000);
        let mut render_risk = if self.render_samples_ns.is_empty() {
            startup
        } else {
            central_risk.max(p90)
        };
        if (1..20).contains(&self.render_samples_ns.len()) {
            render_risk =
                render_risk.max(self.render_samples_ns.iter().copied().max().unwrap_or(0));
        }
        let p95_wake = nearest_rank(&self.wake_lateness_samples_ns, 95);
        let p95_submit = nearest_rank(&self.atomic_submit_samples_ns, 95);
        let submit_allowance = if self.atomic_submit_samples_ns.len() < 20 {
            p95_submit.max(250_000)
        } else {
            p95_submit
        };
        let ceiling = 2_000_000_u64.min(refresh_ns / 4).max(500_000);
        let dynamic_margin = p95_wake.saturating_add(250_000).clamp(500_000, ceiling);
        let safety_margin = if self.wake_lateness_samples_ns.len() < 20
            || self.atomic_submit_samples_ns.len() < 20
        {
            dynamic_margin.max(1_000_000)
        } else {
            dynamic_margin
        };
        let mut total = render_risk
            .saturating_add(submit_allowance)
            .saturating_add(safety_margin);
        if idle {
            total = total.max(refresh_ns.saturating_sub(100_000));
        }
        RenderPrediction {
            ewma_render_ns: self.ewma_render_ns,
            upper_render_deviation_ns: self.upper_render_deviation_ns,
            p90_recent_render_ns: p90,
            render_risk_ns: render_risk,
            p95_wake_lateness_ns: p95_wake,
            p95_atomic_submit_ns: p95_submit,
            submit_allowance_ns: submit_allowance,
            safety_margin_ns: safety_margin,
            total_cost_ns: total,
            idle_wake_guard: idle,
        }
    }
}

#[doc(hidden)]
#[derive(Debug, Clone, Copy)]
pub struct AdaptiveBufferingController {
    policy: AdaptiveTripleBufferPolicy,
    mode: AdaptiveBufferingMode,
    entry_reason: Option<TripleEntryReason>,
    entered_at: Option<(u64, MonotonicTimestampNs)>,
    low_pressure_since: Option<(u64, MonotonicTimestampNs)>,
}

impl AdaptiveBufferingController {
    pub const fn new(policy: AdaptiveTripleBufferPolicy) -> Self {
        Self {
            policy,
            mode: AdaptiveBufferingMode::Double,
            entry_reason: None,
            entered_at: None,
            low_pressure_since: None,
        }
    }

    pub fn observe(
        &mut self,
        predicted_total_cost_ns: u64,
        refresh_interval: Duration,
        proven_miss: Option<ProvenDeadlineMiss>,
        presentation_sequence: u64,
        presentation_time: MonotonicTimestampNs,
        visual_work: bool,
    ) {
        if self.policy == AdaptiveTripleBufferPolicy::Off {
            return;
        }
        let refresh_ns = duration_ns(refresh_interval).max(1);
        if self.mode == AdaptiveBufferingMode::Double {
            let proven_entry = proven_miss.is_some();
            let predictive_entry =
                visual_work && proven_miss.is_none() && predicted_total_cost_ns >= refresh_ns;
            let forced_entry =
                self.policy == AdaptiveTripleBufferPolicy::Force && (visual_work || proven_entry);
            let reason = match self.policy {
                AdaptiveTripleBufferPolicy::Force if forced_entry => {
                    Some(TripleEntryReason::ForcedValidation)
                }
                AdaptiveTripleBufferPolicy::Auto => {
                    proven_miss.map(triple_entry_reason_for_miss).or_else(|| {
                        predictive_entry.then_some(TripleEntryReason::PredictedDeadlinePressure)
                    })
                }
                _ => None,
            };
            if let Some(reason) = reason {
                self.mode = AdaptiveBufferingMode::Triple;
                self.entry_reason = Some(reason);
                self.entered_at = Some((presentation_sequence, presentation_time));
                self.low_pressure_since = None;
            }
            return;
        }
        if self.mode != AdaptiveBufferingMode::Triple {
            return;
        }
        let Some((entry_sequence, entry_time)) = self.entered_at else {
            return;
        };
        let hold_complete = elapsed_both(
            entry_sequence,
            entry_time,
            presentation_sequence,
            presentation_time,
        );
        let low_pressure = proven_miss.is_none()
            && predicted_total_cost_ns.saturating_mul(100) < refresh_ns.saturating_mul(80);
        if !hold_complete || !low_pressure {
            self.low_pressure_since = None;
            return;
        }
        let low_start = *self
            .low_pressure_since
            .get_or_insert((presentation_sequence, presentation_time));
        if elapsed_both(
            low_start.0,
            low_start.1,
            presentation_sequence,
            presentation_time,
        ) {
            self.mode = AdaptiveBufferingMode::Double;
            self.entry_reason = None;
            self.entered_at = None;
            self.low_pressure_since = None;
        }
    }

    pub const fn mode(&self) -> AdaptiveBufferingMode {
        self.mode
    }

    pub const fn pacing_mode(&self) -> NativeOutputPacingMode {
        match self.policy {
            AdaptiveTripleBufferPolicy::Off => NativeOutputPacingMode::ReactiveDouble,
            AdaptiveTripleBufferPolicy::Force => NativeOutputPacingMode::PredictiveTriple,
            AdaptiveTripleBufferPolicy::Auto => match self.mode {
                AdaptiveBufferingMode::Double => NativeOutputPacingMode::ReactiveDouble,
                AdaptiveBufferingMode::Triple => NativeOutputPacingMode::PredictiveTriple,
            },
        }
    }

    pub const fn entry_reason(&self) -> Option<TripleEntryReason> {
        self.entry_reason
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.policy);
    }
}

const fn triple_entry_reason_for_miss(miss: ProvenDeadlineMiss) -> TripleEntryReason {
    match miss {
        ProvenDeadlineMiss::AtomicSubmit => TripleEntryReason::ProvenSubmitMiss,
        ProvenDeadlineMiss::ExactRender | ProvenDeadlineMiss::GuardedApproximateRender => {
            TripleEntryReason::ProvenReadinessMiss
        }
        ProvenDeadlineMiss::Presentation => TripleEntryReason::ProvenPresentationMiss,
    }
}

#[doc(hidden)]
pub fn approximate_observation_is_late(
    observed_ns: u64,
    target_ns: u64,
    p95_wake_lateness_ns: u64,
) -> bool {
    observed_ns > target_ns.saturating_add(p95_wake_lateness_ns.max(500_000))
}

fn elapsed_both(
    start_sequence: u64,
    start_time: MonotonicTimestampNs,
    sequence: u64,
    time: MonotonicTimestampNs,
) -> bool {
    sequence.saturating_sub(start_sequence) >= MIN_HYSTERESIS_PRESENTATIONS
        && time.get().saturating_sub(start_time.get()) >= MIN_HYSTERESIS_NS
}

fn push_bounded(samples: &mut VecDeque<u64>, value: u64) {
    if samples.len() == SAMPLE_CAPACITY {
        samples.pop_front();
    }
    samples.push_back(value);
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

fn mean_alpha(dt_ns: u64) -> (u64, u64) {
    if dt_ns <= 5_000_000 {
        (1, 100)
    } else if dt_ns >= 500_000_000 {
        (1, 1)
    } else {
        (dt_ns, 500_000_000)
    }
}

fn deviation_alpha(dt_ns: u64) -> (u64, u64) {
    if dt_ns <= 6_000_000 {
        (1, 1_000)
    } else if dt_ns >= 600_000_000 {
        (1, 10)
    } else {
        (dt_ns, 6_000_000_000)
    }
}

fn mix_rounded(new: u64, old: u64, alpha_num: u64, alpha_den: u64) -> u64 {
    let new_weighted = u128::from(new) * u128::from(alpha_num);
    let old_weighted = u128::from(old) * u128::from(alpha_den.saturating_sub(alpha_num));
    let rounded = (new_weighted + old_weighted + u128::from(alpha_den / 2)) / u128::from(alpha_den);
    u64::try_from(rounded).unwrap_or(u64::MAX)
}

fn duration_ns(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::presentation_deadline::MonotonicTimestampNs;
    use crate::native::scheduler::NativeFrameScheduler;
    use std::time::Duration;

    #[test]
    fn first_sample_initializes_mean_without_variance() {
        let mut journal = AdaptiveRenderJournal::default();
        journal.record_render_sample(3_000_000, MonotonicTimestampNs::new(10_000_000));
        assert_eq!(journal.ewma_render_ns(), 3_000_000);
        assert_eq!(journal.upper_render_deviation_ns(), 0);
    }

    #[test]
    fn debug_log_delay_before_backend_start_does_not_change_render_sample() {
        let baseline = render_sample_duration_ns(
            MonotonicTimestampNs::new(100),
            MonotonicTimestampNs::new(700),
        );
        let delayed_debug_start = render_sample_duration_ns(
            MonotonicTimestampNs::new(1_000_100),
            MonotonicTimestampNs::new(1_000_700),
        );

        assert_eq!(baseline, 600);
        assert_eq!(delayed_debug_start, baseline);

        let mut baseline_journal = AdaptiveRenderJournal::default();
        baseline_journal.record_render_sample(baseline, MonotonicTimestampNs::new(700));
        let mut delayed_journal = AdaptiveRenderJournal::default();
        delayed_journal
            .record_render_sample(delayed_debug_start, MonotonicTimestampNs::new(1_000_700));
        assert_eq!(
            baseline_journal.prediction(Duration::from_millis(10)),
            delayed_journal.prediction(Duration::from_millis(10))
        );
    }

    #[test]
    fn upward_spike_immediately_expands_positive_deviation() {
        let mut journal = AdaptiveRenderJournal::default();
        journal.record_render_sample(2_000_000, MonotonicTimestampNs::new(10_000_000));
        journal.record_render_sample(6_000_000, MonotonicTimestampNs::new(20_000_000));
        assert_eq!(journal.upper_render_deviation_ns(), 4_000_000);
        assert_eq!(journal.ewma_render_ns(), 2_080_000);
    }

    #[test]
    fn nearest_rank_percentiles_are_exact() {
        let mut journal = AdaptiveRenderJournal::default();
        for sample in 1..=20 {
            journal.record_render_sample(sample, MonotonicTimestampNs::new(sample));
            journal.record_wake_lateness(sample);
            journal.record_atomic_submit(sample);
        }
        assert_eq!(journal.p90_recent_render_ns(), 18);
        let prediction = journal.prediction(Duration::from_millis(10));
        assert_eq!(prediction.p95_wake_lateness_ns, 19);
        assert_eq!(prediction.p95_atomic_submit_ns, 19);
    }

    #[test]
    fn cold_prediction_uses_defined_allowances() {
        let journal = AdaptiveRenderJournal::default();
        let prediction = journal.prediction(Duration::from_millis(10));
        assert_eq!(prediction.render_risk_ns, 4_000_000);
        assert_eq!(prediction.submit_allowance_ns, 250_000);
        assert_eq!(prediction.safety_margin_ns, 1_000_000);
        assert_eq!(prediction.total_cost_ns, 5_250_000);
    }

    #[test]
    fn dynamic_safety_margin_clamps_to_refresh_quarter() {
        let mut journal = AdaptiveRenderJournal::default();
        for _ in 0..20 {
            journal.record_wake_lateness(4_000_000);
            journal.record_atomic_submit(100_000);
        }
        let prediction = journal.prediction(Duration::from_millis(4));
        assert_eq!(prediction.safety_margin_ns, 1_000_000);
        assert_eq!(prediction.submit_allowance_ns, 100_000);
    }

    #[test]
    fn policy_parser_has_exact_accepted_values() {
        assert_eq!(
            AdaptiveTripleBufferPolicy::parse("auto").unwrap(),
            AdaptiveTripleBufferPolicy::Auto
        );
        assert_eq!(
            AdaptiveTripleBufferPolicy::parse("off").unwrap(),
            AdaptiveTripleBufferPolicy::Off
        );
        assert_eq!(
            AdaptiveTripleBufferPolicy::parse("force").unwrap(),
            AdaptiveTripleBufferPolicy::Force
        );
        assert_eq!(
            AdaptiveTripleBufferPolicy::parse("yes").unwrap_err(),
            "invalid OBLIVION_ONE_TRIPLE_BUFFERING value 'yes'; accepted values: auto, off, force"
        );
    }

    #[test]
    fn auto_entry_and_count_plus_time_exit_are_hysteretic() {
        let refresh = Duration::from_millis(10);
        let mut policy = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Auto);
        policy.observe(
            10_000_000,
            refresh,
            None,
            1,
            MonotonicTimestampNs::new(10_000_000),
            true,
        );
        assert_eq!(policy.mode(), AdaptiveBufferingMode::Triple);

        for sequence in 2..=21 {
            policy.observe(
                7_000_000,
                refresh,
                None,
                sequence,
                MonotonicTimestampNs::new(sequence * 10_000_000),
                true,
            );
        }
        assert_eq!(policy.mode(), AdaptiveBufferingMode::Double);
    }

    #[test]
    fn off_never_enters_and_force_enters_for_visual_work() {
        let refresh = Duration::from_millis(10);
        let mut off = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Off);
        off.observe(
            20_000_000,
            refresh,
            Some(ProvenDeadlineMiss::ExactRender),
            1,
            MonotonicTimestampNs::new(1),
            true,
        );
        assert_eq!(off.mode(), AdaptiveBufferingMode::Double);
        off.observe(
            0,
            refresh,
            Some(ProvenDeadlineMiss::Presentation),
            2,
            MonotonicTimestampNs::new(2),
            false,
        );
        assert_eq!(off.mode(), AdaptiveBufferingMode::Double);

        let mut force = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Force);
        force.observe(0, refresh, None, 1, MonotonicTimestampNs::new(1), true);
        assert_eq!(force.mode(), AdaptiveBufferingMode::Triple);
        assert_eq!(
            force.entry_reason(),
            Some(TripleEntryReason::ForcedValidation)
        );
    }

    #[test]
    fn presentation_sequence_slip_becomes_proven_miss() {
        assert_eq!(
            merge_presentation_miss(None, 40, 41),
            Some(ProvenDeadlineMiss::Presentation)
        );
    }

    #[test]
    fn specific_deadline_miss_has_precedence_over_presentation_slip() {
        assert_eq!(
            merge_presentation_miss(Some(ProvenDeadlineMiss::ExactRender), 40, 41,),
            Some(ProvenDeadlineMiss::ExactRender)
        );
        assert_eq!(
            merge_presentation_miss(Some(ProvenDeadlineMiss::AtomicSubmit), 40, 42,),
            Some(ProvenDeadlineMiss::AtomicSubmit)
        );
    }

    #[test]
    fn on_time_presentation_does_not_create_miss() {
        assert_eq!(merge_presentation_miss(None, 40, 40), None);
        assert_eq!(merge_presentation_miss(None, 41, 40), None);
    }

    #[test]
    fn proven_presentation_miss_enters_triple_without_next_frame_already_queued() {
        let mut policy = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Auto);
        let proven_miss = merge_presentation_miss(None, 100, 101);

        policy.observe(
            0,
            Duration::from_millis(10),
            proven_miss,
            101,
            MonotonicTimestampNs::new(10_000_000),
            false,
        );

        assert_eq!(policy.mode(), AdaptiveBufferingMode::Triple);
        assert_eq!(
            policy.entry_reason(),
            Some(TripleEntryReason::ProvenPresentationMiss)
        );
    }

    #[test]
    fn predicted_pressure_still_requires_visual_work() {
        let refresh = Duration::from_millis(10);
        let mut policy = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Auto);

        policy.observe(
            20_000_000,
            refresh,
            None,
            100,
            MonotonicTimestampNs::new(1_000_000_000),
            false,
        );

        assert_eq!(policy.mode(), AdaptiveBufferingMode::Double);

        policy.observe(
            20_000_000,
            refresh,
            None,
            101,
            MonotonicTimestampNs::new(1_010_000_000),
            true,
        );

        assert_eq!(policy.mode(), AdaptiveBufferingMode::Triple);
        assert_eq!(
            policy.entry_reason(),
            Some(TripleEntryReason::PredictedDeadlinePressure)
        );
    }

    #[test]
    fn adaptive_hysteresis_uses_actual_presentation_sequence() {
        let refresh = Duration::from_millis(10);
        let mut policy = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Auto);
        policy.observe(
            0,
            refresh,
            Some(ProvenDeadlineMiss::Presentation),
            100,
            MonotonicTimestampNs::new(0),
            false,
        );

        // Ten planned targets may have elapsed, but the output has not
        // presented another logical sequence yet.
        policy.observe(
            7_000_000,
            refresh,
            None,
            100,
            MonotonicTimestampNs::new(100_000_000),
            true,
        );
        assert_eq!(policy.mode(), AdaptiveBufferingMode::Triple);

        for sequence in 101..=109 {
            policy.observe(
                7_000_000,
                refresh,
                None,
                sequence,
                MonotonicTimestampNs::new(sequence * 1_000_000),
                true,
            );
        }
        assert_eq!(policy.mode(), AdaptiveBufferingMode::Triple);

        policy.observe(
            7_000_000,
            refresh,
            None,
            110,
            MonotonicTimestampNs::new(110_000_000),
            true,
        );
        assert_eq!(policy.mode(), AdaptiveBufferingMode::Triple);

        for sequence in 111..=119 {
            policy.observe(
                7_000_000,
                refresh,
                None,
                sequence,
                MonotonicTimestampNs::new(sequence * 1_000_000),
                true,
            );
        }
        assert_eq!(policy.mode(), AdaptiveBufferingMode::Triple);

        policy.observe(
            7_000_000,
            refresh,
            None,
            120,
            MonotonicTimestampNs::new(210_000_000),
            true,
        );
        assert_eq!(policy.mode(), AdaptiveBufferingMode::Double);
    }

    #[test]
    fn proven_miss_entry_reasons_preserve_specific_precedence() {
        let refresh = Duration::from_millis(10);
        for (miss, reason) in [
            (
                ProvenDeadlineMiss::AtomicSubmit,
                TripleEntryReason::ProvenSubmitMiss,
            ),
            (
                ProvenDeadlineMiss::ExactRender,
                TripleEntryReason::ProvenReadinessMiss,
            ),
            (
                ProvenDeadlineMiss::GuardedApproximateRender,
                TripleEntryReason::ProvenReadinessMiss,
            ),
            (
                ProvenDeadlineMiss::Presentation,
                TripleEntryReason::ProvenPresentationMiss,
            ),
        ] {
            let mut policy = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Auto);
            policy.observe(
                0,
                refresh,
                Some(miss),
                100,
                MonotonicTimestampNs::new(100_000_000),
                false,
            );
            assert_eq!(policy.mode(), AdaptiveBufferingMode::Triple);
            assert_eq!(policy.entry_reason(), Some(reason));
        }
    }

    #[test]
    fn presentation_miss_does_not_exit_existing_triple_hold() {
        let refresh = Duration::from_millis(10);
        let mut policy = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Auto);
        policy.observe(
            0,
            refresh,
            Some(ProvenDeadlineMiss::Presentation),
            100,
            MonotonicTimestampNs::new(0),
            false,
        );
        policy.observe(
            7_000_000,
            refresh,
            Some(ProvenDeadlineMiss::Presentation),
            120,
            MonotonicTimestampNs::new(200_000_000),
            true,
        );

        assert_eq!(policy.mode(), AdaptiveBufferingMode::Triple);
    }

    #[test]
    fn no_miss_and_no_visual_work_does_not_transition() {
        let mut policy = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Auto);
        policy.observe(
            0,
            Duration::from_millis(10),
            None,
            100,
            MonotonicTimestampNs::new(100_000_000),
            false,
        );

        assert_eq!(policy.mode(), AdaptiveBufferingMode::Double);
        assert_eq!(policy.entry_reason(), None);
    }

    #[test]
    fn presentation_miss_does_not_queue_scheduler_work_by_itself() {
        let mut policy = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Auto);
        let scheduler = NativeFrameScheduler::new(165, 0);

        policy.observe(
            0,
            Duration::from_millis(10),
            merge_presentation_miss(None, 100, 101),
            101,
            MonotonicTimestampNs::new(10_000_000),
            false,
        );

        assert_eq!(policy.mode(), AdaptiveBufferingMode::Triple);
        assert!(!scheduler.visual_work_queued());
        assert_eq!(scheduler.next_deadline_ns(), None);
    }

    #[test]
    fn pacing_mode_maps_off_and_auto_double_to_reactive_double() {
        let off = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Off);
        let auto = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Auto);

        assert_eq!(off.pacing_mode(), NativeOutputPacingMode::ReactiveDouble);
        assert_eq!(auto.pacing_mode(), NativeOutputPacingMode::ReactiveDouble);
    }

    #[test]
    fn pacing_mode_maps_force_and_auto_triple_to_predictive_triple() {
        let refresh = Duration::from_millis(10);
        let force = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Force);
        let mut auto = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Auto);
        auto.observe(
            10_000_000,
            refresh,
            None,
            1,
            MonotonicTimestampNs::new(10_000_000),
            true,
        );

        assert_eq!(
            force.pacing_mode(),
            NativeOutputPacingMode::PredictiveTriple
        );
        assert_eq!(auto.pacing_mode(), NativeOutputPacingMode::PredictiveTriple);
    }

    #[test]
    fn approximate_miss_requires_the_guard() {
        assert!(!approximate_observation_is_late(
            10_400_000, 10_000_000, 100_000
        ));
        assert!(approximate_observation_is_late(
            10_500_001, 10_000_000, 100_000
        ));
        assert!(approximate_observation_is_late(
            10_900_001, 10_000_000, 900_000
        ));
    }

    #[test]
    fn idle_guard_applies_to_exactly_one_post_idle_prediction() {
        let refresh = Duration::from_millis(10);
        let mut journal = AdaptiveRenderJournal::default();
        journal.note_matching_presentation(MonotonicTimestampNs::new(10_000_000));

        let first = journal.prediction_at(MonotonicTimestampNs::new(1_010_000_000), refresh);
        let second = journal.prediction_at(MonotonicTimestampNs::new(1_010_000_001), refresh);

        assert!(first.idle_wake_guard);
        assert_eq!(first.total_cost_ns, 9_900_000);
        assert!(!second.idle_wake_guard);
    }

    #[test]
    fn one_hundred_twenty_sample_p90_uses_nearest_rank() {
        let mut journal = AdaptiveRenderJournal::default();
        for sample in 1..=120 {
            journal.record_render_sample(sample, MonotonicTimestampNs::new(sample));
        }
        assert_eq!(journal.p90_recent_render_ns(), 108);
    }
}
