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
pub enum TripleCapabilityBlocker {
    NonAtomicKms,
    ExplicitSwapchainUnavailable,
    SlotCapacityMismatch,
    PrimaryInFenceUnavailable,
    RenderFenceExportUnavailable,
    SubmissionTransportUnhealthy,
    SessionInactive,
    OutputGenerationUnstable,
    UnsupportedPresentationMode,
    SwapchainPoisoned,
    SoftwareCursorVisible,
}

#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TripleCapability {
    Capable,
    Unavailable(TripleCapabilityBlocker),
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
    pub p95_worker_queue_residency_ns: u64,
    pub p95_worker_submit_wake_lateness_ns: u64,
    pub p95_atomic_ioctl_ns: u64,
    pub submission_budget_ns: u64,
    pub p95_target_slip_ns: u64,
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
    worker_queue_residency_samples_ns: VecDeque<u64>,
    worker_submit_wake_lateness_samples_ns: VecDeque<u64>,
    submission_budget_samples_ns: VecDeque<u64>,
    target_slip_samples_ns: VecDeque<u64>,
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
            worker_queue_residency_samples_ns: VecDeque::with_capacity(SAMPLE_CAPACITY),
            worker_submit_wake_lateness_samples_ns: VecDeque::with_capacity(SAMPLE_CAPACITY),
            submission_budget_samples_ns: VecDeque::with_capacity(SAMPLE_CAPACITY),
            target_slip_samples_ns: VecDeque::with_capacity(SAMPLE_CAPACITY),
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

    pub fn record_worker_queue_residency(&mut self, sample_ns: u64) {
        push_bounded(&mut self.worker_queue_residency_samples_ns, sample_ns);
    }

    pub fn record_worker_submit_wake_lateness(&mut self, sample_ns: u64) {
        push_bounded(&mut self.worker_submit_wake_lateness_samples_ns, sample_ns);
    }

    pub fn record_submission_budget(&mut self, sample_ns: u64) {
        push_bounded(&mut self.submission_budget_samples_ns, sample_ns);
    }

    pub fn record_target_slip(&mut self, sample_ns: u64) {
        push_bounded(&mut self.target_slip_samples_ns, sample_ns);
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
        let p95_ioctl = nearest_rank(&self.atomic_submit_samples_ns, 95);
        let p95_worker_wake = nearest_rank(&self.worker_submit_wake_lateness_samples_ns, 95);
        let p95_queue_residency = nearest_rank(&self.worker_queue_residency_samples_ns, 95);
        let measured_submission_budget = p95_worker_wake
            .saturating_add(p95_ioctl)
            .saturating_add(100_000);
        let exported_submission_budget = nearest_rank(&self.submission_budget_samples_ns, 95);
        let submission_budget = if exported_submission_budget != 0 {
            exported_submission_budget
        } else if self.atomic_submit_samples_ns.len() < 20 {
            measured_submission_budget.max(250_000)
        } else {
            measured_submission_budget
        };
        let submit_allowance = submission_budget;
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
            p95_atomic_submit_ns: p95_ioctl,
            p95_worker_queue_residency_ns: p95_queue_residency,
            p95_worker_submit_wake_lateness_ns: p95_worker_wake,
            p95_atomic_ioctl_ns: p95_ioctl,
            submission_budget_ns: submission_budget,
            p95_target_slip_ns: nearest_rank(&self.target_slip_samples_ns, 95),
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
    capability: TripleCapability,
    force_unavailable_blocker: Option<TripleCapabilityBlocker>,
}

impl AdaptiveBufferingController {
    pub const fn new(policy: AdaptiveTripleBufferPolicy) -> Self {
        Self {
            policy,
            mode: AdaptiveBufferingMode::Double,
            entry_reason: None,
            entered_at: None,
            low_pressure_since: None,
            capability: TripleCapability::Unavailable(
                TripleCapabilityBlocker::ExplicitSwapchainUnavailable,
            ),
            force_unavailable_blocker: None,
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
        self.observe_with_pipeline(
            predicted_total_cost_ns,
            refresh_interval,
            proven_miss,
            presentation_sequence,
            presentation_time,
            visual_work,
            TripleCapability::Capable,
            false,
            0,
        );
    }

    pub fn apply_capability(&mut self, capability: TripleCapability) {
        self.capability = capability;
        match (self.policy, capability) {
            (AdaptiveTripleBufferPolicy::Off, _) => {
                self.mode = AdaptiveBufferingMode::Double;
                self.force_unavailable_blocker = None;
            }
            (AdaptiveTripleBufferPolicy::Force, TripleCapability::Unavailable(blocker)) => {
                self.force_unavailable_blocker = Some(blocker);
                if self.mode != AdaptiveBufferingMode::Triple {
                    self.mode = AdaptiveBufferingMode::Double;
                    self.entry_reason = None;
                    self.entered_at = None;
                    self.low_pressure_since = None;
                }
            }
            (AdaptiveTripleBufferPolicy::Force, TripleCapability::Capable) => {
                self.mode = AdaptiveBufferingMode::Triple;
                self.entry_reason = Some(TripleEntryReason::ForcedValidation);
                self.force_unavailable_blocker = None;
            }
            (AdaptiveTripleBufferPolicy::Auto, TripleCapability::Unavailable(_)) => {
                self.force_unavailable_blocker = None;
                if self.mode != AdaptiveBufferingMode::Triple {
                    self.mode = AdaptiveBufferingMode::Double;
                    self.entry_reason = None;
                    self.entered_at = None;
                    self.low_pressure_since = None;
                }
            }
            (AdaptiveTripleBufferPolicy::Auto, TripleCapability::Capable) => {
                self.force_unavailable_blocker = None;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn observe_with_pipeline(
        &mut self,
        predicted_total_cost_ns: u64,
        refresh_interval: Duration,
        proven_miss: Option<ProvenDeadlineMiss>,
        presentation_sequence: u64,
        presentation_time: MonotonicTimestampNs,
        visual_work: bool,
        capability: TripleCapability,
        prepared_frame_exists: bool,
        future_primary_depth: u8,
    ) {
        self.capability = capability;
        if self.policy == AdaptiveTripleBufferPolicy::Off {
            return;
        }
        let TripleCapability::Capable = capability else {
            self.force_unavailable_blocker = match (self.policy, capability) {
                (AdaptiveTripleBufferPolicy::Force, TripleCapability::Unavailable(blocker)) => {
                    Some(blocker)
                }
                _ => None,
            };
            if self.mode != AdaptiveBufferingMode::Triple
                || (!prepared_frame_exists && future_primary_depth <= 1)
            {
                self.mode = AdaptiveBufferingMode::Double;
                self.entry_reason = None;
                self.entered_at = None;
                self.low_pressure_since = None;
            }
            return;
        };
        self.force_unavailable_blocker = None;
        let refresh_ns = duration_ns(refresh_interval).max(1);
        if self.policy == AdaptiveTripleBufferPolicy::Force {
            if self.mode != AdaptiveBufferingMode::Triple {
                self.mode = AdaptiveBufferingMode::Triple;
                self.entry_reason = Some(TripleEntryReason::ForcedValidation);
                self.entered_at = Some((presentation_sequence, presentation_time));
            }
            return;
        }
        if self.mode == AdaptiveBufferingMode::Double {
            let predictive_entry =
                visual_work && proven_miss.is_none() && predicted_total_cost_ns >= refresh_ns;
            let reason = match self.policy {
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
        if proven_miss.is_some() || at_least_percent(predicted_total_cost_ns, refresh_ns, 95) {
            self.low_pressure_since = None;
            return;
        }
        if !hold_complete {
            return;
        }
        let low_pressure = below_percent(predicted_total_cost_ns, refresh_ns, 80);
        if !low_pressure {
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
        ) && !prepared_frame_exists
            && future_primary_depth <= 1
        {
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
        match self.mode {
            AdaptiveBufferingMode::Double => NativeOutputPacingMode::ReactiveDouble,
            AdaptiveBufferingMode::Triple => NativeOutputPacingMode::PredictiveTriple,
        }
    }

    pub const fn entry_reason(&self) -> Option<TripleEntryReason> {
        self.entry_reason
    }

    pub const fn capability(&self) -> TripleCapability {
        self.capability
    }

    pub const fn force_unavailable_blocker(&self) -> Option<TripleCapabilityBlocker> {
        self.force_unavailable_blocker
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

fn at_least_percent(value: u64, total: u64, percent: u64) -> bool {
    u128::from(value) * 100 >= u128::from(total) * u128::from(percent)
}

fn below_percent(value: u64, total: u64, percent: u64) -> bool {
    u128::from(value) * 100 < u128::from(total) * u128::from(percent)
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
        assert_eq!(prediction.submit_allowance_ns, 200_000);
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
        let mut force = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Force);
        force.observe(0, refresh, None, 1, MonotonicTimestampNs::new(1), true);
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

    #[test]
    fn queue_residency_is_observable_but_never_part_of_predicted_cost() {
        let mut journal = AdaptiveRenderJournal::default();
        journal.record_worker_queue_residency(500_000_000);
        let baseline = journal.prediction(Duration::from_millis(10));

        assert_eq!(baseline.p95_worker_queue_residency_ns, 500_000_000);
        assert!(baseline.total_cost_ns < 10_000_000);
    }

    #[test]
    fn timing_dimensions_are_independently_bounded() {
        let mut journal = AdaptiveRenderJournal::default();
        for sample in 1..=121 {
            let at = MonotonicTimestampNs::new(sample);
            journal.record_render_sample(sample, at);
            journal.record_wake_lateness(sample + 1_000);
            journal.record_worker_queue_residency(sample + 2_000);
            journal.record_worker_submit_wake_lateness(sample + 3_000);
            journal.record_atomic_submit(sample + 4_000);
            journal.record_submission_budget(sample + 5_000);
            journal.record_target_slip(sample + 6_000);
        }

        assert_eq!(journal.render_samples_ns.len(), SAMPLE_CAPACITY);
        assert_eq!(journal.wake_lateness_samples_ns.len(), SAMPLE_CAPACITY);
        assert_eq!(
            journal.worker_queue_residency_samples_ns.len(),
            SAMPLE_CAPACITY
        );
        assert_eq!(
            journal.worker_submit_wake_lateness_samples_ns.len(),
            SAMPLE_CAPACITY
        );
        assert_eq!(journal.atomic_submit_samples_ns.len(), SAMPLE_CAPACITY);
        assert_eq!(journal.submission_budget_samples_ns.len(), SAMPLE_CAPACITY);
        assert_eq!(journal.target_slip_samples_ns.len(), SAMPLE_CAPACITY);

        let prediction = journal.prediction(Duration::from_millis(10));
        assert_eq!(prediction.p90_recent_render_ns, 109);
        assert_eq!(prediction.p95_wake_lateness_ns, 1_115);
        assert_eq!(prediction.p95_worker_queue_residency_ns, 2_115);
        assert_eq!(prediction.p95_worker_submit_wake_lateness_ns, 3_115);
        assert_eq!(prediction.p95_atomic_ioctl_ns, 4_115);
        assert_eq!(prediction.submission_budget_ns, 5_115);
        assert_eq!(prediction.p95_target_slip_ns, 6_115);
    }

    #[test]
    fn force_never_bypasses_an_exact_capability_blocker() {
        let mut controller = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Force);
        controller.observe_with_pipeline(
            20_000_000,
            Duration::from_millis(10),
            None,
            1,
            MonotonicTimestampNs::new(10_000_000),
            true,
            TripleCapability::Unavailable(TripleCapabilityBlocker::PrimaryInFenceUnavailable),
            false,
            0,
        );

        assert_eq!(controller.mode(), AdaptiveBufferingMode::Double);
        assert_eq!(
            controller.force_unavailable_blocker(),
            Some(TripleCapabilityBlocker::PrimaryInFenceUnavailable)
        );
        assert_eq!(
            controller.pacing_mode(),
            NativeOutputPacingMode::ReactiveDouble
        );
    }

    #[test]
    fn visible_software_cursor_forces_reactive_double_even_when_triple_is_forced() {
        let mut controller = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Force);
        controller.apply_capability(TripleCapability::Unavailable(
            TripleCapabilityBlocker::SoftwareCursorVisible,
        ));

        assert_eq!(controller.mode(), AdaptiveBufferingMode::Double);
        assert_eq!(
            controller.pacing_mode(),
            NativeOutputPacingMode::ReactiveDouble
        );
        assert_eq!(
            controller.force_unavailable_blocker(),
            Some(TripleCapabilityBlocker::SoftwareCursorVisible)
        );
    }

    #[test]
    fn triple_exit_waits_until_prepared_is_empty_and_future_depth_is_at_most_one() {
        let refresh = Duration::from_millis(10);
        let mut controller = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Auto);
        controller.observe_with_pipeline(
            10_000_000,
            refresh,
            None,
            1,
            MonotonicTimestampNs::new(10_000_000),
            true,
            TripleCapability::Capable,
            false,
            0,
        );
        for sequence in 2..=30 {
            controller.observe_with_pipeline(
                7_000_000,
                refresh,
                None,
                sequence,
                MonotonicTimestampNs::new(sequence * 10_000_000),
                true,
                TripleCapability::Capable,
                false,
                2,
            );
        }
        assert_eq!(controller.mode(), AdaptiveBufferingMode::Triple);

        for sequence in 31..=41 {
            controller.observe_with_pipeline(
                7_000_000,
                refresh,
                None,
                sequence,
                MonotonicTimestampNs::new(sequence * 10_000_000),
                true,
                TripleCapability::Capable,
                false,
                1,
            );
        }
        assert_eq!(controller.mode(), AdaptiveBufferingMode::Double);
    }

    #[test]
    fn capability_loss_drains_two_future_primaries_before_leaving_triple() {
        let refresh = Duration::from_millis(10);
        let blocker = TripleCapabilityBlocker::SubmissionTransportUnhealthy;
        let mut controller = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Auto);
        controller.observe_with_pipeline(
            10_000_000,
            refresh,
            None,
            1,
            MonotonicTimestampNs::new(10_000_000),
            true,
            TripleCapability::Capable,
            false,
            0,
        );
        assert_eq!(controller.mode(), AdaptiveBufferingMode::Triple);

        controller.apply_capability(TripleCapability::Unavailable(blocker));
        assert_eq!(controller.mode(), AdaptiveBufferingMode::Triple);
        controller.observe_with_pipeline(
            7_000_000,
            refresh,
            None,
            2,
            MonotonicTimestampNs::new(20_000_000),
            true,
            TripleCapability::Unavailable(blocker),
            false,
            2,
        );
        assert_eq!(controller.mode(), AdaptiveBufferingMode::Triple);

        controller.observe_with_pipeline(
            7_000_000,
            refresh,
            None,
            3,
            MonotonicTimestampNs::new(30_000_000),
            true,
            TripleCapability::Unavailable(blocker),
            false,
            1,
        );
        assert_eq!(controller.mode(), AdaptiveBufferingMode::Double);
    }

    #[test]
    fn ninety_five_percent_pressure_resets_exit_candidacy_at_common_refresh_rates() {
        for refresh_ns in [16_666_667, 8_333_333, 6_944_444, 6_060_606] {
            let refresh = Duration::from_nanos(refresh_ns);
            let mut controller = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Auto);
            controller.observe_with_pipeline(
                refresh_ns,
                refresh,
                None,
                1,
                MonotonicTimestampNs::new(refresh_ns),
                true,
                TripleCapability::Capable,
                false,
                0,
            );
            for sequence in 2..=20 {
                controller.observe_with_pipeline(
                    refresh_ns * 79 / 100,
                    refresh,
                    None,
                    sequence,
                    MonotonicTimestampNs::new(sequence * refresh_ns),
                    true,
                    TripleCapability::Capable,
                    false,
                    0,
                );
            }
            controller.observe_with_pipeline(
                refresh_ns.saturating_mul(95).div_ceil(100),
                refresh,
                None,
                21,
                MonotonicTimestampNs::new(21 * refresh_ns),
                true,
                TripleCapability::Capable,
                false,
                0,
            );
            for sequence in 22..=30 {
                controller.observe_with_pipeline(
                    refresh_ns * 79 / 100,
                    refresh,
                    None,
                    sequence,
                    MonotonicTimestampNs::new(sequence * refresh_ns),
                    true,
                    TripleCapability::Capable,
                    false,
                    0,
                );
            }
            assert_eq!(controller.mode(), AdaptiveBufferingMode::Triple);
        }
    }
}
