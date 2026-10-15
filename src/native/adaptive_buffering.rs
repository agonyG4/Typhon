//! Deadline risk prediction and bounded adaptive-buffering policy.
#![allow(dead_code)] // Wired into the native runtime in Task 12.

use crate::control_snapshots::{
    BranchRiskDiagnostics, BranchRiskSource, ExactParallelDiagnostics, SampleWindowDiagnostics,
};
use crate::native::buffering::{
    O1CreditDemandController, O1CreditDemandReason, PresentationOpportunityId,
};
use crate::native::presentation_deadline::{MonotonicTimestampNs, PresentationTarget};
use crate::native::scheduler::NativeOutputPacingMode;
use std::collections::VecDeque;
use std::time::Duration;

const SAMPLE_CAPACITY: usize = 120;
const WARM_PARALLEL_MIN_SAMPLES: usize = 20;
const MISS_RECOVERY_EXACT_SUCCESSES: usize = 20;
const COLD_WAKE_REFRESH_INTERVALS: u64 = 100;

#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredictionEstimatorMode {
    ColdStart,
    WarmParallel,
    MissRecovery,
}

impl PredictionEstimatorMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ColdStart => "cold_start",
            Self::WarmParallel => "warm_parallel",
            Self::MissRecovery => "miss_recovery",
        }
    }
}

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

impl AdaptiveBufferingMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Double => "double",
            Self::Triple => "triple",
        }
    }
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

impl TripleCapabilityBlocker {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NonAtomicKms => "non_atomic_kms",
            Self::ExplicitSwapchainUnavailable => "explicit_swapchain_unavailable",
            Self::SlotCapacityMismatch => "slot_capacity_mismatch",
            Self::PrimaryInFenceUnavailable => "primary_in_fence_unavailable",
            Self::RenderFenceExportUnavailable => "render_fence_export_unavailable",
            Self::SubmissionTransportUnhealthy => "submission_transport_unhealthy",
            Self::SessionInactive => "session_inactive",
            Self::OutputGenerationUnstable => "output_generation_unstable",
            Self::UnsupportedPresentationMode => "unsupported_presentation_mode",
            Self::SwapchainPoisoned => "swapchain_poisoned",
            Self::SoftwareCursorVisible => "software_cursor_visible",
        }
    }
}

#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TripleCapability {
    Capable,
    Unavailable(TripleCapabilityBlocker),
}

impl TripleCapability {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Capable => "capable",
            Self::Unavailable(_) => "unavailable",
        }
    }

    pub const fn blocker(self) -> Option<TripleCapabilityBlocker> {
        match self {
            Self::Capable => None,
            Self::Unavailable(blocker) => Some(blocker),
        }
    }
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
    KmsDispatch,
    KmsApplyGuard,
}

impl ProvenDeadlineMiss {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ExactRender => "exact_render",
            Self::GuardedApproximateRender => "guarded_approximate_render",
            Self::KmsDispatch => "kms_dispatch",
            Self::KmsApplyGuard => "kms_apply_guard",
        }
    }
}

#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstimatorRecoveryDisposition {
    ResetIndependentHorizon,
    PreserveEstimatorState,
}

impl EstimatorRecoveryDisposition {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ResetIndependentHorizon => "reset_independent_horizon",
            Self::PreserveEstimatorState => "preserve_estimator_state",
        }
    }
}

#[doc(hidden)]
pub fn merge_presentation_miss(
    existing: Option<ProvenDeadlineMiss>,
    planned_sequence: u64,
    actual_sequence: u64,
) -> Option<ProvenDeadlineMiss> {
    existing.or_else(|| {
        (actual_sequence > planned_sequence).then_some(ProvenDeadlineMiss::KmsApplyGuard)
    })
}

#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FenceTimestampQuality {
    ExactSyncFile,
    ObservedApproximate,
}

#[derive(Debug, Clone, Copy)]
pub struct FrameTimingObservation {
    pub frame_id: u64,
    pub target: PresentationTarget,
    pub composite_started_at: MonotonicTimestampNs,
    pub worker_queued_at: Option<MonotonicTimestampNs>,
    pub fence_signaled_at: Option<(MonotonicTimestampNs, FenceTimestampQuality)>,
}

/// Exact worker and GPU branch samples from one frame. These branches may
/// become ready in either order; only each branch's relation to composite
/// start must be monotonic.
fn exact_parallel_branch_samples_ns(observation: &FrameTimingObservation) -> Option<(u64, u64)> {
    let (fence_signaled_at, FenceTimestampQuality::ExactSyncFile) = observation.fence_signaled_at?
    else {
        return None;
    };
    let worker_queued_at = observation.worker_queued_at?;
    let composite_started_ns = observation.composite_started_at.get();
    let fence_signaled_ns = fence_signaled_at.get();
    let worker_queued_ns = worker_queued_at.get();

    if composite_started_ns > fence_signaled_ns || composite_started_ns > worker_queued_ns {
        return None;
    }

    Some((
        fence_signaled_ns - composite_started_ns,
        worker_queued_ns - composite_started_ns,
    ))
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
    pub p95_worker_submit_wake_lateness_ns: u64,
    pub p95_atomic_submit_ns: u64,
    pub p95_worker_queue_residency_ns: u64,
    pub p95_worker_pre_submit_ns: u64,
    pub p95_worker_dispatch_ns: u64,
    pub p95_atomic_ioctl_ns: u64,
    pub main_event_loop_wake_guard_ns: u64,
    pub kms_dispatch_budget_ns: u64,
    pub kms_apply_guard_ns: u64,
    pub kms_total_lead_ns: u64,
    pub p95_target_slip_ns: u64,
    pub exact_render_ready_risk_ns: u64,
    pub exact_queue_ready_risk_ns: u64,
    pub parallel_service_ns: u64,
    pub parallel_samples: usize,
    pub estimator_mode: PredictionEstimatorMode,
    pub independent_total_cost_ns: u64,
    pub warm_parallel_total_cost_ns: u64,
    pub independent_p90_floor_ns: u64,
    pub worker_non_ioctl_lead_ns: u64,
    pub miss_recovery_remaining: usize,
    pub total_cost_ns: u64,
    pub cold_wake_guard: bool,
}

#[derive(Debug)]
struct BranchRiskHistory {
    samples_ns: VecDeque<u64>,
    ewma_ns: u64,
    upper_positive_deviation_ns: u64,
    last_sample_at: Option<MonotonicTimestampNs>,
}

impl Default for BranchRiskHistory {
    fn default() -> Self {
        Self {
            samples_ns: VecDeque::with_capacity(SAMPLE_CAPACITY),
            ewma_ns: 0,
            upper_positive_deviation_ns: 0,
            last_sample_at: None,
        }
    }
}

impl BranchRiskHistory {
    fn record(&mut self, sample_ns: u64, at: MonotonicTimestampNs) {
        if let Some(previous_at) = self.last_sample_at {
            let dt_ns = at.get().saturating_sub(previous_at.get());
            let positive_error = sample_ns.saturating_sub(self.ewma_ns);
            let (deviation_num, deviation_den) = deviation_alpha(dt_ns);
            self.upper_positive_deviation_ns = mix_rounded(
                positive_error,
                self.upper_positive_deviation_ns,
                deviation_num,
                deviation_den,
            )
            .max(positive_error);
            let (mean_num, mean_den) = mean_alpha(dt_ns);
            self.ewma_ns = mix_rounded(sample_ns, self.ewma_ns, mean_num, mean_den);
        } else {
            self.ewma_ns = sample_ns;
            self.upper_positive_deviation_ns = 0;
        }
        push_bounded(&mut self.samples_ns, sample_ns);
        self.last_sample_at = Some(at);
    }

    fn risk_ns(&self) -> u64 {
        nearest_rank(&self.samples_ns, 95).max(
            self.ewma_ns
                .saturating_add(self.upper_positive_deviation_ns.saturating_mul(2)),
        )
    }

    fn diagnostics(&self, refresh_interval_ns: u64) -> BranchRiskDiagnostics {
        let window = sample_window_diagnostics(&self.samples_ns, refresh_interval_ns);
        let central_risk_ns = self
            .ewma_ns
            .saturating_add(self.upper_positive_deviation_ns.saturating_mul(2));
        let selected_risk_ns = window.p95_ns.max(central_risk_ns);
        let selected_source = if self.samples_ns.is_empty() {
            BranchRiskSource::None
        } else if window.p95_ns >= central_risk_ns {
            // Ties intentionally report the recent p95 floor.
            BranchRiskSource::RecentP95
        } else {
            BranchRiskSource::PositiveDeviation
        };

        BranchRiskDiagnostics {
            window,
            ewma_ns: self.ewma_ns,
            upper_positive_deviation_ns: self.upper_positive_deviation_ns,
            central_risk_ns,
            selected_risk_ns,
            selected_source,
        }
    }
}

#[derive(Debug)]
struct DiagnosticSampleHistory {
    samples_ns: VecDeque<u64>,
}

impl Default for DiagnosticSampleHistory {
    fn default() -> Self {
        Self {
            samples_ns: VecDeque::with_capacity(SAMPLE_CAPACITY),
        }
    }
}

impl DiagnosticSampleHistory {
    fn record(&mut self, sample_ns: u64) {
        push_bounded(&mut self.samples_ns, sample_ns);
    }

    fn diagnostics(&self, refresh_interval_ns: u64) -> SampleWindowDiagnostics {
        sample_window_diagnostics(&self.samples_ns, refresh_interval_ns)
    }
}

#[derive(Debug, Default)]
struct ExactParallelRiskJournal {
    render_ready: BranchRiskHistory,
    queue_ready: BranchRiskHistory,
    composite_to_rendered_observed: DiagnosticSampleHistory,
    post_render_handoff: DiagnosticSampleHistory,
    fence_after_rendered_observed: DiagnosticSampleHistory,
    rendered_observed_after_fence: DiagnosticSampleHistory,
    decomposition_invalid_samples: u64,
}

impl ExactParallelRiskJournal {
    fn record(&mut self, render_ready_ns: u64, queue_ready_ns: u64, at: MonotonicTimestampNs) {
        self.render_ready.record(render_ready_ns, at);
        self.queue_ready.record(queue_ready_ns, at);
    }

    fn samples(&self) -> usize {
        self.render_ready
            .samples_ns
            .len()
            .min(self.queue_ready.samples_ns.len())
    }

    fn record_decomposition(
        &mut self,
        observation: &FrameTimingObservation,
        rendered_observed_at: Option<MonotonicTimestampNs>,
    ) {
        let Some(rendered_observed_at) = rendered_observed_at else {
            return;
        };
        let Some(worker_handoff_begin) = observation.worker_queued_at else {
            return;
        };
        let Some((fence_signal, FenceTimestampQuality::ExactSyncFile)) =
            observation.fence_signaled_at
        else {
            return;
        };

        let composite_ns = observation.composite_started_at.get();
        let rendered_ns = rendered_observed_at.get();
        let worker_handoff_ns = worker_handoff_begin.get();
        let fence_ns = fence_signal.get();

        if rendered_ns < composite_ns {
            self.decomposition_invalid_samples =
                self.decomposition_invalid_samples.saturating_add(1);
            return;
        }
        self.composite_to_rendered_observed
            .record(rendered_ns - composite_ns);

        if worker_handoff_ns < rendered_ns {
            self.decomposition_invalid_samples =
                self.decomposition_invalid_samples.saturating_add(1);
        } else {
            self.post_render_handoff
                .record(worker_handoff_ns - rendered_ns);
        }

        if fence_ns > rendered_ns {
            self.fence_after_rendered_observed
                .record(fence_ns - rendered_ns);
        } else if rendered_ns > fence_ns {
            self.rendered_observed_after_fence
                .record(rendered_ns - fence_ns);
        }
    }

    fn diagnostics(&self, refresh_interval_ns: u64) -> ExactParallelDiagnostics {
        let mut queue_before_fence_samples = 0;
        let mut fence_before_queue_samples = 0;
        let mut equal_samples = 0;
        for (render_ready_ns, queue_ready_ns) in self
            .render_ready
            .samples_ns
            .iter()
            .zip(self.queue_ready.samples_ns.iter())
        {
            if queue_ready_ns < render_ready_ns {
                queue_before_fence_samples += 1;
            } else if render_ready_ns < queue_ready_ns {
                fence_before_queue_samples += 1;
            } else {
                equal_samples += 1;
            }
        }

        ExactParallelDiagnostics {
            render_ready: self.render_ready.diagnostics(refresh_interval_ns),
            queue_ready: self.queue_ready.diagnostics(refresh_interval_ns),
            composite_to_rendered_observed: self
                .composite_to_rendered_observed
                .diagnostics(refresh_interval_ns),
            post_render_handoff: self.post_render_handoff.diagnostics(refresh_interval_ns),
            fence_after_rendered_observed: self
                .fence_after_rendered_observed
                .diagnostics(refresh_interval_ns),
            rendered_observed_after_fence: self
                .rendered_observed_after_fence
                .diagnostics(refresh_interval_ns),
            queue_before_fence_samples,
            fence_before_queue_samples,
            equal_samples,
            decomposition_invalid_samples: self.decomposition_invalid_samples,
        }
    }
}

fn sample_window_diagnostics(
    samples_ns: &VecDeque<u64>,
    refresh_interval_ns: u64,
) -> SampleWindowDiagnostics {
    if samples_ns.is_empty() {
        return SampleWindowDiagnostics::default();
    }

    let count = samples_ns.len().min(SAMPLE_CAPACITY);
    let mut scratch = [0_u64; SAMPLE_CAPACITY];
    let values = &mut scratch[..count];
    let (front, back) = samples_ns.as_slices();
    let front_len = front.len().min(count);
    values[..front_len].copy_from_slice(&front[..front_len]);
    let back_len = count - front_len;
    values[front_len..].copy_from_slice(&back[..back_len]);
    values.sort_unstable();

    let percentile = |percent: usize| {
        let rank = (percent * count).div_ceil(100).max(1);
        values[rank - 1]
    };

    SampleWindowDiagnostics {
        count,
        latest_ns: samples_ns.back().copied().unwrap_or(0),
        p50_ns: percentile(50),
        p90_ns: percentile(90),
        p95_ns: percentile(95),
        p99_ns: percentile(99),
        max_ns: values[count - 1],
        over_refresh_samples: samples_ns
            .iter()
            .filter(|sample| **sample > refresh_interval_ns)
            .count(),
        over_two_refresh_samples: samples_ns
            .iter()
            .filter(|sample| **sample > refresh_interval_ns.saturating_mul(2))
            .count(),
    }
}

fn parallel_service_ns(
    render_ready_risk_ns: u64,
    queue_ready_risk_ns: u64,
    kms_budget_ns: u64,
) -> u64 {
    render_ready_risk_ns.max(queue_ready_risk_ns.saturating_add(kms_budget_ns))
}

fn warm_parallel_total_cost_ns(
    main_wake_guard_ns: u64,
    kms_apply_guard_ns: u64,
    render_ready_risk_ns: u64,
    queue_ready_risk_ns: u64,
    kms_dispatch_budget_ns: u64,
) -> u64 {
    main_wake_guard_ns
        .saturating_add(kms_apply_guard_ns)
        .saturating_add(parallel_service_ns(
            render_ready_risk_ns,
            queue_ready_risk_ns,
            kms_dispatch_budget_ns,
        ))
}

#[doc(hidden)]
#[derive(Debug)]
pub struct AdaptiveRenderJournal {
    render_samples_ns: VecDeque<u64>,
    wake_lateness_samples_ns: VecDeque<u64>,
    atomic_submit_samples_ns: VecDeque<u64>,
    worker_queue_residency_samples_ns: VecDeque<u64>,
    worker_submit_wake_lateness_samples_ns: VecDeque<u64>,
    worker_pre_submit_samples_ns: VecDeque<u64>,
    worker_dispatch_samples_ns: VecDeque<u64>,
    fixed_submission_budget_samples_ns: VecDeque<u64>,
    target_slip_samples_ns: VecDeque<u64>,
    exact_parallel: ExactParallelRiskJournal,
    ewma_render_ns: u64,
    upper_render_deviation_ns: u64,
    last_sample_at: Option<MonotonicTimestampNs>,
    pub(crate) missed_deadlines: u64,
    miss_recovery_remaining: usize,
}

impl Default for AdaptiveRenderJournal {
    fn default() -> Self {
        Self {
            render_samples_ns: VecDeque::with_capacity(SAMPLE_CAPACITY),
            wake_lateness_samples_ns: VecDeque::with_capacity(SAMPLE_CAPACITY),
            atomic_submit_samples_ns: VecDeque::with_capacity(SAMPLE_CAPACITY),
            worker_queue_residency_samples_ns: VecDeque::with_capacity(SAMPLE_CAPACITY),
            worker_submit_wake_lateness_samples_ns: VecDeque::with_capacity(SAMPLE_CAPACITY),
            worker_pre_submit_samples_ns: VecDeque::with_capacity(SAMPLE_CAPACITY),
            worker_dispatch_samples_ns: VecDeque::with_capacity(SAMPLE_CAPACITY),
            fixed_submission_budget_samples_ns: VecDeque::with_capacity(SAMPLE_CAPACITY),
            target_slip_samples_ns: VecDeque::with_capacity(SAMPLE_CAPACITY),
            exact_parallel: ExactParallelRiskJournal::default(),
            ewma_render_ns: 0,
            upper_render_deviation_ns: 0,
            last_sample_at: None,
            missed_deadlines: 0,
            miss_recovery_remaining: 0,
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

    pub fn record_fixed_timed_wake(&mut self, sample_ns: u64) {
        push_bounded(&mut self.worker_submit_wake_lateness_samples_ns, sample_ns);
    }

    pub fn record_worker_service(&mut self, pre_submit_ns: u64, dispatch_ns: u64) {
        push_bounded(&mut self.worker_pre_submit_samples_ns, pre_submit_ns);
        push_bounded(&mut self.worker_dispatch_samples_ns, dispatch_ns);
    }

    pub fn record_fixed_submission_budget(&mut self, sample_ns: u64) {
        if sample_ns > 0 {
            push_bounded(&mut self.fixed_submission_budget_samples_ns, sample_ns);
        }
    }

    pub fn record_target_slip(&mut self, sample_ns: u64) {
        push_bounded(&mut self.target_slip_samples_ns, sample_ns);
    }

    pub fn record_frame_service_observation(&mut self, observation: FrameTimingObservation) {
        let _ = self.record_frame_service_observation_with_rendered(observation, None);
    }

    /// Records the same authoritative exact R/Q evidence as
    /// `record_frame_service_observation` and, when supplied, adds diagnostic
    /// decomposition from the compositor's render-observation timestamp.
    pub fn record_frame_service_observation_with_rendered(
        &mut self,
        observation: FrameTimingObservation,
        rendered_observed_at: Option<MonotonicTimestampNs>,
    ) -> Option<(u64, u64)> {
        let (render_ready_ns, queue_ready_ns) = exact_parallel_branch_samples_ns(&observation)?;
        self.exact_parallel.record(
            render_ready_ns,
            queue_ready_ns,
            observation.composite_started_at,
        );
        self.exact_parallel
            .record_decomposition(&observation, rendered_observed_at);
        self.miss_recovery_remaining = self.miss_recovery_remaining.saturating_sub(1);
        Some((render_ready_ns, queue_ready_ns))
    }

    /// Computes raw exact-branch distributions only for explicit performance
    /// snapshots. Prediction and per-frame recording use their existing bounded
    /// percentile work and never call this method.
    pub fn exact_parallel_diagnostics(&self, refresh_interval_ns: u64) -> ExactParallelDiagnostics {
        self.exact_parallel.diagnostics(refresh_interval_ns.max(1))
    }

    pub fn note_proven_deadline_miss(&mut self, disposition: EstimatorRecoveryDisposition) {
        self.missed_deadlines = self.missed_deadlines.saturating_add(1);
        if disposition == EstimatorRecoveryDisposition::ResetIndependentHorizon {
            self.miss_recovery_remaining = MISS_RECOVERY_EXACT_SUCCESSES;
        }
    }

    pub fn prediction_estimator_mode(&self) -> PredictionEstimatorMode {
        if self.miss_recovery_remaining > 0 {
            PredictionEstimatorMode::MissRecovery
        } else if self.exact_parallel.samples() >= WARM_PARALLEL_MIN_SAMPLES
            && self.fixed_submission_budget_samples_ns.len() >= WARM_PARALLEL_MIN_SAMPLES
        {
            PredictionEstimatorMode::WarmParallel
        } else {
            PredictionEstimatorMode::ColdStart
        }
    }

    pub fn prediction(&self, refresh_interval: Duration) -> RenderPrediction {
        self.base_prediction(refresh_interval, false, 0)
    }

    pub fn prediction_with_kms_guard(
        &self,
        refresh_interval: Duration,
        kms_apply_guard_ns: u64,
    ) -> RenderPrediction {
        self.base_prediction(refresh_interval, false, kms_apply_guard_ns)
    }

    pub fn prediction_at(
        &self,
        now: MonotonicTimestampNs,
        refresh_interval: Duration,
    ) -> RenderPrediction {
        let cold = self.is_cold(now, refresh_interval);
        self.base_prediction(refresh_interval, cold, 0)
    }

    pub fn prediction_at_with_kms_guard(
        &self,
        now: MonotonicTimestampNs,
        refresh_interval: Duration,
        kms_apply_guard_ns: u64,
    ) -> RenderPrediction {
        let cold = self.is_cold(now, refresh_interval);
        self.base_prediction(refresh_interval, cold, kms_apply_guard_ns)
    }

    fn is_cold(&self, now: MonotonicTimestampNs, refresh_interval: Duration) -> bool {
        let cold_threshold_ns = duration_ns(refresh_interval)
            .max(1)
            .saturating_mul(COLD_WAKE_REFRESH_INTERVALS);
        match self.last_sample_at {
            None => true,
            Some(last_sample_at) => now
                .get()
                .checked_sub(last_sample_at.get())
                .is_none_or(|elapsed_ns| elapsed_ns >= cold_threshold_ns),
        }
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

    fn base_prediction(
        &self,
        refresh_interval: Duration,
        cold_wake: bool,
        kms_apply_guard_ns: u64,
    ) -> RenderPrediction {
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
        if (1..WARM_PARALLEL_MIN_SAMPLES).contains(&self.render_samples_ns.len()) {
            render_risk =
                render_risk.max(self.render_samples_ns.iter().copied().max().unwrap_or(0));
        }
        let p95_wake = nearest_rank(&self.wake_lateness_samples_ns, 95);
        let p95_worker_submit_wake = nearest_rank(&self.worker_submit_wake_lateness_samples_ns, 95);
        let p95_ioctl = nearest_rank(&self.atomic_submit_samples_ns, 95);
        let p95_worker_pre_submit = nearest_rank(&self.worker_pre_submit_samples_ns, 95);
        let p95_worker_dispatch = nearest_rank(&self.worker_dispatch_samples_ns, 95);
        let p95_queue_residency = nearest_rank(&self.worker_queue_residency_samples_ns, 95);
        let kms_dispatch_budget = if self.fixed_submission_budget_samples_ns.is_empty() {
            250_000
        } else {
            self.fixed_submission_budget_samples_ns
                .back()
                .copied()
                .unwrap_or(250_000)
        };
        let ceiling = 2_000_000_u64.min(refresh_ns / 4).max(500_000);
        let dynamic_margin = p95_wake.saturating_add(250_000).clamp(500_000, ceiling);
        let main_event_loop_wake_guard = if self.wake_lateness_samples_ns.len()
            < WARM_PARALLEL_MIN_SAMPLES
            || self.fixed_submission_budget_samples_ns.len() < WARM_PARALLEL_MIN_SAMPLES
        {
            dynamic_margin.max(1_000_000)
        } else {
            dynamic_margin
        };
        let kms_total_lead = kms_dispatch_budget.saturating_add(kms_apply_guard_ns);
        let exact_render_ready_risk_ns = self.exact_parallel.render_ready.risk_ns();
        let exact_queue_ready_risk_ns = self.exact_parallel.queue_ready.risk_ns();
        let parallel_service_ns = parallel_service_ns(
            exact_render_ready_risk_ns,
            exact_queue_ready_risk_ns,
            kms_dispatch_budget,
        );
        let independent_total_cost_ns = render_risk
            .saturating_add(main_event_loop_wake_guard)
            .saturating_add(kms_total_lead);
        let worker_non_ioctl_lead_ns = kms_dispatch_budget.saturating_sub(p95_ioctl);
        let warm_parallel_total_cost_ns = warm_parallel_total_cost_ns(
            main_event_loop_wake_guard,
            kms_apply_guard_ns,
            exact_render_ready_risk_ns,
            exact_queue_ready_risk_ns,
            kms_dispatch_budget,
        );
        let independent_p90_floor_ns = p90
            .saturating_add(main_event_loop_wake_guard)
            .saturating_add(kms_dispatch_budget)
            .saturating_add(kms_apply_guard_ns);
        let estimator_mode = self.prediction_estimator_mode();
        let mut total = match estimator_mode {
            PredictionEstimatorMode::ColdStart | PredictionEstimatorMode::MissRecovery => {
                independent_total_cost_ns
            }
            PredictionEstimatorMode::WarmParallel => warm_parallel_total_cost_ns,
        };
        if cold_wake {
            total = total.max(refresh_ns.saturating_sub(100_000));
        }
        RenderPrediction {
            ewma_render_ns: self.ewma_render_ns,
            upper_render_deviation_ns: self.upper_render_deviation_ns,
            p90_recent_render_ns: p90,
            render_risk_ns: render_risk,
            p95_wake_lateness_ns: p95_wake,
            p95_worker_submit_wake_lateness_ns: p95_worker_submit_wake,
            p95_atomic_submit_ns: p95_ioctl,
            p95_worker_queue_residency_ns: p95_queue_residency,
            p95_worker_pre_submit_ns: p95_worker_pre_submit,
            p95_worker_dispatch_ns: p95_worker_dispatch,
            p95_atomic_ioctl_ns: p95_ioctl,
            main_event_loop_wake_guard_ns: main_event_loop_wake_guard,
            kms_dispatch_budget_ns: kms_dispatch_budget,
            kms_apply_guard_ns,
            kms_total_lead_ns: kms_total_lead,
            p95_target_slip_ns: nearest_rank(&self.target_slip_samples_ns, 95),
            exact_render_ready_risk_ns,
            exact_queue_ready_risk_ns,
            parallel_service_ns,
            parallel_samples: self.exact_parallel.samples(),
            estimator_mode,
            independent_total_cost_ns,
            warm_parallel_total_cost_ns,
            independent_p90_floor_ns,
            worker_non_ioctl_lead_ns,
            miss_recovery_remaining: self.miss_recovery_remaining,
            total_cost_ns: total,
            cold_wake_guard: cold_wake,
        }
    }
}

#[doc(hidden)]
#[derive(Debug, Clone, Copy)]
pub struct AdaptiveBufferingController {
    policy: AdaptiveTripleBufferPolicy,
    mode: AdaptiveBufferingMode,
    o1_credit: O1CreditDemandController,
    last_overlap_target: Option<PresentationOpportunityId>,
    last_overlap_required_ns: u64,
    positive_overlap_observations: u64,
    nonpositive_overlap_observations: u64,
    entry_reason: Option<TripleEntryReason>,
    capability: TripleCapability,
    force_unavailable_blocker: Option<TripleCapabilityBlocker>,
}

impl AdaptiveBufferingController {
    pub const fn new(policy: AdaptiveTripleBufferPolicy) -> Self {
        Self {
            policy,
            mode: AdaptiveBufferingMode::Double,
            o1_credit: O1CreditDemandController::with_ceiling(match policy {
                AdaptiveTripleBufferPolicy::Force => 2,
                AdaptiveTripleBufferPolicy::Off | AdaptiveTripleBufferPolicy::Auto => 1,
            }),
            last_overlap_target: None,
            last_overlap_required_ns: 0,
            positive_overlap_observations: 0,
            nonpositive_overlap_observations: 0,
            entry_reason: None,
            capability: TripleCapability::Unavailable(
                TripleCapabilityBlocker::ExplicitSwapchainUnavailable,
            ),
            force_unavailable_blocker: None,
        }
    }

    pub fn apply_capability(&mut self, capability: TripleCapability) {
        self.capability = capability;
        match (self.policy, capability) {
            (AdaptiveTripleBufferPolicy::Off, _) => {
                self.o1_credit.set_ceiling(1);
                self.mode = AdaptiveBufferingMode::Double;
                self.force_unavailable_blocker = None;
            }
            (AdaptiveTripleBufferPolicy::Force, TripleCapability::Unavailable(blocker)) => {
                self.o1_credit.set_ceiling(1);
                self.force_unavailable_blocker = Some(blocker);
                if self.mode != AdaptiveBufferingMode::Triple {
                    self.mode = AdaptiveBufferingMode::Double;
                    self.entry_reason = None;
                }
            }
            (AdaptiveTripleBufferPolicy::Force, TripleCapability::Capable) => {
                self.o1_credit.set_ceiling(2);
                self.mode = AdaptiveBufferingMode::Triple;
                self.entry_reason = Some(TripleEntryReason::ForcedValidation);
                self.o1_credit.force();
                self.force_unavailable_blocker = None;
            }
            (AdaptiveTripleBufferPolicy::Auto, TripleCapability::Unavailable(_)) => {
                self.o1_credit.set_ceiling(1);
                self.force_unavailable_blocker = None;
                if self.mode != AdaptiveBufferingMode::Triple {
                    self.mode = AdaptiveBufferingMode::Double;
                    self.entry_reason = None;
                }
            }
            (AdaptiveTripleBufferPolicy::Auto, TripleCapability::Capable) => {
                self.o1_credit.set_ceiling(2);
                self.force_unavailable_blocker = None;
            }
        }
    }

    /// Feed O1's overlap decision into buffering policy.  This is capacity
    /// control only: it never changes an already armed presentation target.
    pub fn observe_overlap_required(&mut self, overlap_required_ns: u64) {
        match self.policy {
            AdaptiveTripleBufferPolicy::Off => self.o1_credit.set_ceiling(1),
            AdaptiveTripleBufferPolicy::Force => {
                if self.capability == TripleCapability::Capable {
                    self.o1_credit.set_ceiling(2);
                    self.o1_credit.force();
                } else {
                    self.o1_credit.set_ceiling(1);
                }
            }
            AdaptiveTripleBufferPolicy::Auto => {
                if self.capability == TripleCapability::Capable {
                    let before = self.o1_credit.effective();
                    self.o1_credit.observe_overlap(overlap_required_ns);
                    self.note_overlap_transition(before);
                } else {
                    self.o1_credit.set_ceiling(1);
                }
            }
        }
        self.sync_o1_mode();
    }

    /// Evaluate overlap once for a predecessor opportunity.  Scheduler wake
    /// retries may inspect the same pending target many times; those retries
    /// must not look like repeated pressure observations.
    pub fn observe_overlap_for_target(
        &mut self,
        predecessor: Option<PresentationTarget>,
        overlap_required_ns: u64,
    ) {
        let target_identity = predecessor.map(|target| target.physical_claim().opportunity_id());
        if self.last_overlap_target == target_identity {
            return;
        }
        self.last_overlap_target = target_identity;
        self.last_overlap_required_ns = overlap_required_ns;
        if overlap_required_ns > 0 {
            self.positive_overlap_observations =
                self.positive_overlap_observations.saturating_add(1);
        } else {
            self.nonpositive_overlap_observations =
                self.nonpositive_overlap_observations.saturating_add(1);
        }
        match self.policy {
            AdaptiveTripleBufferPolicy::Off => self.o1_credit.set_ceiling(1),
            AdaptiveTripleBufferPolicy::Force => {
                if self.capability == TripleCapability::Capable {
                    self.o1_credit.set_ceiling(2);
                    self.o1_credit.force();
                } else {
                    self.o1_credit.set_ceiling(1);
                }
            }
            AdaptiveTripleBufferPolicy::Auto => {
                if self.capability == TripleCapability::Capable {
                    let before = self.o1_credit.effective();
                    if let Some(target_identity) = target_identity {
                        self.o1_credit
                            .observe_opportunity(target_identity, overlap_required_ns);
                    } else {
                        self.o1_credit.observe_overlap(overlap_required_ns);
                    }
                    self.note_overlap_transition(before);
                } else {
                    self.o1_credit.set_ceiling(1);
                    self.entry_reason = None;
                }
            }
        }
        self.sync_o1_mode();
    }

    /// Apply pageflip evidence without reviving the legacy global mode
    /// hysteresis.  A proven outcome may grant capacity, but never retargets a
    /// live frame.
    pub fn observe_o1_outcome(&mut self, proven_miss: Option<ProvenDeadlineMiss>) {
        if self.policy == AdaptiveTripleBufferPolicy::Auto
            && self.capability == TripleCapability::Capable
            && let Some(miss) = proven_miss
            && matches!(
                miss,
                ProvenDeadlineMiss::ExactRender | ProvenDeadlineMiss::GuardedApproximateRender
            )
        {
            let before = self.o1_credit.effective();
            self.o1_credit.observe_render_readiness_miss();
            if before == 1 && self.o1_credit.effective() == 2 {
                self.entry_reason = Some(TripleEntryReason::ProvenReadinessMiss);
            }
        }
        self.sync_o1_mode();
    }

    fn note_overlap_transition(&mut self, before: u8) {
        if before == 1 && self.o1_credit.effective() == 2 {
            self.entry_reason = Some(TripleEntryReason::PredictedDeadlinePressure);
        } else if self.o1_credit.effective() == 1 {
            self.entry_reason = None;
        }
    }

    fn sync_o1_mode(&mut self) {
        self.mode = if self.o1_credit.effective() == 2
            && self.capability == TripleCapability::Capable
            && self.policy != AdaptiveTripleBufferPolicy::Off
        {
            AdaptiveBufferingMode::Triple
        } else {
            AdaptiveBufferingMode::Double
        };
    }

    pub const fn future_primary_credit(&self) -> u8 {
        self.o1_credit.effective()
    }

    pub const fn desired_credit(&self) -> u8 {
        self.o1_credit.desired_credit()
    }

    pub const fn clean_release_progress(&self) -> u8 {
        self.o1_credit.clean_release_progress()
    }

    pub const fn clean_release_horizon(&self) -> u8 {
        self.o1_credit.clean_release_horizon()
    }

    pub const fn admission_allows_future_primary(&self, owned_depth: u8) -> bool {
        owned_depth < self.o1_credit.desired_credit()
    }

    pub const fn demand_reason(&self) -> Option<O1CreditDemandReason> {
        self.o1_credit.demand_reason()
    }

    pub const fn extra_credit_grants(&self) -> u64 {
        self.o1_credit.grants()
    }

    pub const fn extra_credit_revokes(&self) -> u64 {
        self.o1_credit.revokes()
    }

    pub const fn last_overlap_required_ns(&self) -> u64 {
        self.last_overlap_required_ns
    }

    pub const fn positive_overlap_observations(&self) -> u64 {
        self.positive_overlap_observations
    }

    pub const fn nonpositive_overlap_observations(&self) -> u64 {
        self.nonpositive_overlap_observations
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

pub fn triple_buffering_doctor_severity(
    policy: AdaptiveTripleBufferPolicy,
    mode: AdaptiveBufferingMode,
    forced_requirement_failed: bool,
) -> crate::control_snapshots::DoctorSeverity {
    use crate::control_snapshots::DoctorSeverity;

    match (policy, mode, forced_requirement_failed) {
        (AdaptiveTripleBufferPolicy::Force, AdaptiveBufferingMode::Double, true) => {
            DoctorSeverity::Warning
        }
        _ => DoctorSeverity::Ok,
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

fn push_bounded(samples: &mut VecDeque<u64>, value: u64) {
    if samples.len() == SAMPLE_CAPACITY {
        samples.pop_front();
    }
    samples.push_back(value);
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
    fn nearest_rank_matches_sorted_reference_through_ring_wraps() {
        let mut samples = VecDeque::with_capacity(SAMPLE_CAPACITY);
        for step in 0..=SAMPLE_CAPACITY * 3 {
            let original = samples.clone();
            let mut sorted: Vec<_> = samples.iter().copied().collect();
            sorted.sort_unstable();
            for percentile in [1, 50, 90, 95, 100] {
                let expected = if sorted.is_empty() {
                    0
                } else {
                    sorted[(percentile * sorted.len()).div_ceil(100) - 1]
                };
                assert_eq!(nearest_rank(&samples, percentile), expected);
            }
            assert_eq!(samples, original, "prediction must preserve eviction order");
            let sample = match step % 4 {
                0 => 0,
                1 => u64::MAX,
                _ => ((step * 73) % 127) as u64,
            };
            push_bounded(&mut samples, sample);
        }
    }

    #[test]
    fn cold_prediction_uses_defined_allowances() {
        let journal = AdaptiveRenderJournal::default();
        let prediction = journal.prediction(Duration::from_millis(10));
        assert_eq!(prediction.render_risk_ns, 4_000_000);
        assert_eq!(prediction.kms_dispatch_budget_ns, 250_000);
        assert_eq!(prediction.main_event_loop_wake_guard_ns, 1_000_000);
        assert_eq!(prediction.total_cost_ns, 5_250_000);
    }

    #[test]
    fn variable_only_history_preserves_the_first_fixed_cold_start_floor() {
        let mut journal = AdaptiveRenderJournal::default();
        for sample in 0..=SAMPLE_CAPACITY {
            let at = MonotonicTimestampNs::new(1_000_000_000 + sample as u64 * 6_000_000);
            journal.record_render_sample(80_000, at);
            journal.record_atomic_submit(15_000);
            journal.record_worker_service(10_000, 25_000);
        }

        let prediction = journal.prediction(Duration::from_nanos(6_060_606));

        assert_eq!(
            prediction.estimator_mode,
            PredictionEstimatorMode::ColdStart
        );
        assert_eq!(prediction.kms_dispatch_budget_ns, 250_000);
    }

    pub(super) fn service_target() -> PresentationTarget {
        let timestamp = MonotonicTimestampNs::new(10_000_000);
        PresentationTarget {
            sequence: 1,
            presentation_time: timestamp,
            submit_not_before: timestamp,
            render_start_deadline: timestamp,
            refresh_interval: Duration::from_nanos(6_060_606),
            reason: crate::native::presentation_deadline::PresentationTargetReason::Normal,
            clock_generation: 1,
            estimated: false,
            predicted_unreachable: false,
            physical_claim: crate::native::presentation_deadline::PrimaryRefreshClaim {
                sequence: 1,
                presentation_time: timestamp,
                clock_generation: 1,
            },
            selection_evidence: Default::default(),
        }
    }

    pub(super) fn service_observation(
        render_started_ns: u64,
        render_signaled_ns: u64,
        worker_queued_ns: u64,
        _submit_returned_ns: u64,
    ) -> FrameTimingObservation {
        FrameTimingObservation {
            frame_id: 1,
            target: service_target(),
            composite_started_at: MonotonicTimestampNs::new(render_started_ns),
            worker_queued_at: Some(MonotonicTimestampNs::new(worker_queued_ns)),
            fence_signaled_at: Some((
                MonotonicTimestampNs::new(render_signaled_ns),
                FenceTimestampQuality::ExactSyncFile,
            )),
        }
    }

    #[test]
    fn intentional_worker_wait_does_not_change_warm_prediction() {
        fn journal_with_worker_wait(wait_ns: u64) -> AdaptiveRenderJournal {
            let mut journal = AdaptiveRenderJournal::default();
            for sample in 0..WARM_PARALLEL_MIN_SAMPLES {
                let composite_ns = 100_000_000 + sample as u64 * 50_000_000;
                let observation = service_observation(
                    composite_ns,
                    composite_ns + 3_000_000,
                    composite_ns + 1_000_000,
                    composite_ns + 1_100_000 + wait_ns,
                );
                journal.record_render_sample(
                    3_000_000,
                    MonotonicTimestampNs::new(composite_ns + 3_000_000),
                );
                journal.record_fixed_submission_budget(500_000);
                journal.record_frame_service_observation(observation);
            }
            journal
        }

        let little_wait = journal_with_worker_wait(1_000_000).prediction(Duration::from_millis(10));
        let intentional_wait =
            journal_with_worker_wait(30_000_000).prediction(Duration::from_millis(10));

        assert_eq!(
            little_wait.estimator_mode,
            PredictionEstimatorMode::WarmParallel
        );
        assert_eq!(
            little_wait.total_cost_ns, intentional_wait.total_cost_ns,
            "scheduled worker wait must not train compositor service risk"
        );
    }

    fn record_uniform_prediction_inputs(
        journal: &mut AdaptiveRenderJournal,
        render_sample_ns: u64,
        exact_render_ready_ns: u64,
        exact_queue_ready_ns: u64,
        wake_lateness_ns: u64,
        ioctl_ns: u64,
        kms_dispatch_budget_ns: u64,
    ) {
        for sample in 0..WARM_PARALLEL_MIN_SAMPLES {
            let base_ns = 100_000_000 + sample as u64 * 10_000_000;
            journal.record_render_sample(render_sample_ns, MonotonicTimestampNs::new(base_ns));
            journal.record_wake_lateness(wake_lateness_ns);
            journal.record_atomic_submit(ioctl_ns);
            journal.record_fixed_submission_budget(kms_dispatch_budget_ns);
            journal.record_frame_service_observation(service_observation(
                base_ns,
                base_ns + exact_render_ready_ns,
                base_ns + exact_queue_ready_ns,
                base_ns + exact_queue_ready_ns + 1_000_000,
            ));
        }
    }

    fn warm_parallel_journal_with_budget_history(budget_ns: u64) -> AdaptiveRenderJournal {
        let mut journal = AdaptiveRenderJournal::default();
        record_uniform_prediction_inputs(
            &mut journal,
            2_000_000,
            2_000_000,
            4_000_000,
            0,
            0,
            budget_ns,
        );
        for _ in WARM_PARALLEL_MIN_SAMPLES..SAMPLE_CAPACITY {
            journal.record_fixed_submission_budget(budget_ns);
        }
        assert_eq!(
            journal.prediction_estimator_mode(),
            PredictionEstimatorMode::WarmParallel
        );
        journal
    }

    #[test]
    fn one_new_authoritative_kms_budget_immediately_changes_warm_prediction() {
        let mut journal = warm_parallel_journal_with_budget_history(500_000);
        let before = journal.prediction_with_kms_guard(Duration::from_millis(10), 300_000);
        assert_eq!(before.kms_dispatch_budget_ns, 500_000);
        assert_eq!(before.parallel_service_ns, 4_500_000);

        journal.record_fixed_submission_budget(1_500_000);

        let after = journal.prediction_with_kms_guard(Duration::from_millis(10), 300_000);
        assert_eq!(after.kms_dispatch_budget_ns, 1_500_000);
        assert_eq!(after.parallel_service_ns, 5_500_000);
        assert_eq!(after.total_cost_ns, before.total_cost_ns + 1_000_000);
    }

    #[test]
    fn kms_dispatch_tail_increase_and_decay_reach_the_next_prediction() {
        use crate::native::kms_worker_dispatch_timing::KmsWorkerDispatchModel;

        let mut kms = KmsWorkerDispatchModel::default();
        for _ in 0..SAMPLE_CAPACITY {
            kms.record_fixed_timed_wake(0);
            kms.record_worker_service(0, 0, 450_000);
        }
        assert_eq!(kms.budget().dispatch_budget_ns, 500_000);

        let mut journal =
            warm_parallel_journal_with_budget_history(kms.budget().dispatch_budget_ns);
        let before = journal.prediction_with_kms_guard(Duration::from_millis(10), 300_000);

        let miss = kms.observe_submission_deadline(1_000_000, 1_100_000, true, true);
        assert!(miss.increased);
        let raised_budget_ns = kms.budget().dispatch_budget_ns;
        assert_eq!(raised_budget_ns, 650_000);
        journal.record_fixed_submission_budget(raised_budget_ns);

        let after_increase = journal.prediction_with_kms_guard(Duration::from_millis(10), 300_000);
        assert_eq!(after_increase.kms_dispatch_budget_ns, raised_budget_ns);
        assert_eq!(after_increase.total_cost_ns, before.total_cost_ns + 150_000);

        for _ in 0..32 {
            kms.observe_submission_deadline(2_000_000, 2_000_000, true, true);
        }
        let decayed_budget_ns = kms.budget().dispatch_budget_ns;
        assert_eq!(decayed_budget_ns, 600_000);
        journal.record_fixed_submission_budget(decayed_budget_ns);

        let after_decay = journal.prediction_with_kms_guard(Duration::from_millis(10), 300_000);
        assert_eq!(after_decay.kms_dispatch_budget_ns, decayed_budget_ns);
        assert_eq!(after_decay.total_cost_ns, before.total_cost_ns + 100_000);
    }

    #[test]
    fn exact_parallel_history_records_compositor_to_queue_ready_branch() {
        let mut journal = AdaptiveRenderJournal::default();
        journal.record_frame_service_observation(service_observation(100, 1_100, 9_000, 9_300));

        let prediction = journal.prediction(Duration::from_millis(10));
        assert_eq!(prediction.parallel_samples, 1);
        assert_eq!(prediction.exact_render_ready_risk_ns, 1_000);
        assert_eq!(prediction.exact_queue_ready_risk_ns, 8_900);
    }

    #[test]
    fn exact_parallel_gpu_branch_can_dominate() {
        let mut journal = AdaptiveRenderJournal::default();
        journal.record_frame_service_observation(service_observation(
            0, 5_000_000, 1_000_000, 2_000_000,
        ));

        let prediction = journal.prediction(Duration::from_millis(10));
        assert_eq!(prediction.parallel_samples, 1);
        assert_eq!(prediction.exact_render_ready_risk_ns, 5_000_000);
        assert_eq!(prediction.exact_queue_ready_risk_ns, 1_000_000);
    }

    #[test]
    fn exact_parallel_queue_branch_can_dominate_without_waiting_for_fence() {
        let mut journal = AdaptiveRenderJournal::default();
        journal.record_frame_service_observation(service_observation(
            0, 2_000_000, 4_000_000, 5_000_000,
        ));

        let prediction = journal.prediction(Duration::from_millis(10));
        assert_eq!(prediction.parallel_samples, 1);
        assert_eq!(prediction.exact_render_ready_risk_ns, 2_000_000);
        assert_eq!(prediction.exact_queue_ready_risk_ns, 4_000_000);
    }

    #[test]
    fn exact_parallel_accepts_fence_signal_before_worker_queue() {
        let mut journal = AdaptiveRenderJournal::default();
        journal.record_frame_service_observation(service_observation(
            0, 500_000, 1_000_000, 2_000_000,
        ));

        let prediction = journal.prediction(Duration::from_millis(10));
        assert_eq!(prediction.parallel_samples, 1);
        assert_eq!(prediction.exact_render_ready_risk_ns, 500_000);
        assert_eq!(prediction.exact_queue_ready_risk_ns, 1_000_000);
    }

    #[test]
    fn exact_parallel_accepts_worker_queue_before_fence_signal() {
        let mut journal = AdaptiveRenderJournal::default();
        journal.record_frame_service_observation(service_observation(
            0, 4_000_000, 800_000, 4_100_000,
        ));

        let prediction = journal.prediction(Duration::from_millis(10));
        assert_eq!(prediction.parallel_samples, 1);
        assert_eq!(prediction.exact_render_ready_risk_ns, 4_000_000);
        assert_eq!(prediction.exact_queue_ready_risk_ns, 800_000);
    }

    #[test]
    fn approximate_fence_timestamps_do_not_enter_exact_parallel_history() {
        let mut journal = AdaptiveRenderJournal::default();
        let mut observation = service_observation(0, 5_000_000, 1_000_000, 2_000_000);
        observation.fence_signaled_at = Some((
            MonotonicTimestampNs::new(5_000_000),
            FenceTimestampQuality::ObservedApproximate,
        ));

        journal.record_frame_service_observation(observation);

        let prediction = journal.prediction(Duration::from_millis(10));
        assert_eq!(prediction.parallel_samples, 0);
        assert_eq!(prediction.exact_render_ready_risk_ns, 0);
        assert_eq!(prediction.exact_queue_ready_risk_ns, 0);
    }

    #[test]
    fn invalid_queue_or_fence_timestamps_do_not_train_exact_parallel_history() {
        let mut journal = AdaptiveRenderJournal::default();
        journal.record_frame_service_observation(service_observation(
            5_000_000, 8_000_000, 4_000_000, 9_000_000,
        ));
        journal.record_frame_service_observation(service_observation(
            5_000_000, 4_000_000, 6_000_000, 7_000_000,
        ));

        let prediction = journal.prediction(Duration::from_millis(10));
        assert_eq!(prediction.parallel_samples, 0);
        assert_eq!(prediction.miss_recovery_remaining, 0);
    }

    #[test]
    fn warm_parallel_selects_exact_branches_instead_of_independent_deviation() {
        fn journal_with_exact_render_ready(exact_render_ready_ns: u64) -> AdaptiveRenderJournal {
            let mut journal = AdaptiveRenderJournal::default();
            for sample in 0..WARM_PARALLEL_MIN_SAMPLES {
                let base_ns = 10_000_000 + sample as u64 * 10_000_000;
                journal.record_render_sample(
                    if sample == WARM_PARALLEL_MIN_SAMPLES - 1 {
                        10_000_000
                    } else {
                        1_000_000
                    },
                    MonotonicTimestampNs::new(base_ns),
                );
                journal.record_fixed_submission_budget(500_000);
                journal.record_frame_service_observation(service_observation(
                    base_ns,
                    base_ns + exact_render_ready_ns,
                    base_ns + 500_000,
                    base_ns + 1_000_000,
                ));
            }
            journal
        }

        let low_parallel =
            journal_with_exact_render_ready(2_000_000).prediction(Duration::from_millis(10));
        let high_parallel =
            journal_with_exact_render_ready(8_000_000).prediction(Duration::from_millis(10));

        assert_eq!(
            low_parallel.estimator_mode,
            PredictionEstimatorMode::WarmParallel
        );
        assert_eq!(
            high_parallel.estimator_mode,
            PredictionEstimatorMode::WarmParallel
        );
        assert_eq!(
            low_parallel.render_risk_ns, high_parallel.render_risk_ns,
            "independent render risk remains shared diagnostic state"
        );
        assert!(
            high_parallel.upper_render_deviation_ns >= 8_000_000,
            "the regression must include a large independent render tail"
        );
        assert!(low_parallel.render_risk_ns >= 10_000_000);
        assert!(
            low_parallel.total_cost_ns < high_parallel.total_cost_ns,
            "WarmParallel must select exact branches instead of sticky independent risk"
        );
    }

    #[test]
    fn exact_parallel_branch_histories_use_queue_and_gpu_risks() {
        let mut journal = AdaptiveRenderJournal::default();
        for sample in 0..WARM_PARALLEL_MIN_SAMPLES {
            let base_ns = 10_000_000 + sample as u64 * 10_000_000;
            journal.record_render_sample(1_000_000, MonotonicTimestampNs::new(base_ns));
            journal.record_wake_lateness(4_000_000);
            journal.record_fixed_submission_budget(2_000_000);
            journal.record_worker_service(3_000_000, 2_000_000);
            journal.record_frame_service_observation(service_observation(
                base_ns,
                base_ns + 4_000_000,
                base_ns + 10_000_000,
                base_ns + 13_000_000,
            ));
        }

        let prediction = journal.prediction(Duration::from_millis(10));
        assert_eq!(prediction.exact_render_ready_risk_ns, 4_000_000);
        assert_eq!(prediction.exact_queue_ready_risk_ns, 10_000_000);
        assert_eq!(prediction.parallel_service_ns, 12_000_000);
        assert_eq!(prediction.parallel_samples, WARM_PARALLEL_MIN_SAMPLES);
        assert_eq!(
            prediction.estimator_mode,
            PredictionEstimatorMode::WarmParallel
        );
        assert_eq!(
            prediction.warm_parallel_total_cost_ns,
            prediction.main_event_loop_wake_guard_ns + 12_000_000,
            "parallel branch maximum is counted once, followed by main and apply guards"
        );
        assert_eq!(
            prediction.total_cost_ns,
            prediction.warm_parallel_total_cost_ns
        );
    }

    #[test]
    fn parallel_formula_uses_max_and_counts_each_guard_once() {
        assert_eq!(
            warm_parallel_total_cost_ns(200_000, 300_000, 5_000_000, 1_000_000, 500_000),
            5_500_000,
            "GPU branch dominates without serially adding KMS dispatch"
        );
        assert_eq!(
            warm_parallel_total_cost_ns(200_000, 300_000, 2_000_000, 4_000_000, 1_000_000),
            5_500_000,
            "queue-ready plus active KMS service dominates"
        );
    }

    #[test]
    fn exact_parallel_branch_risk_attacks_fast_decays_slowly_and_recovers() {
        let mut journal = AdaptiveRenderJournal::default();
        for sample in 0..WARM_PARALLEL_MIN_SAMPLES {
            let composite_ns = 100_000_000 + sample as u64 * 6_000_000;
            journal.record_frame_service_observation(service_observation(
                composite_ns,
                composite_ns + 1_000_000,
                composite_ns + 500_000,
                composite_ns + 2_000_000,
            ));
        }
        let stable_risk = journal
            .prediction(Duration::from_millis(10))
            .exact_render_ready_risk_ns;
        let stable_queue_risk = journal
            .prediction(Duration::from_millis(10))
            .exact_queue_ready_risk_ns;
        assert_eq!(stable_risk, 1_000_000);
        assert_eq!(stable_queue_risk, 500_000);

        let spike_at = 100_000_000 + WARM_PARALLEL_MIN_SAMPLES as u64 * 6_000_000;
        journal.record_frame_service_observation(service_observation(
            spike_at,
            spike_at + 8_000_000,
            spike_at + 6_000_000,
            spike_at + 9_000_000,
        ));
        let spike_risk = journal
            .prediction(Duration::from_millis(10))
            .exact_render_ready_risk_ns;
        let queue_spike_risk = journal
            .prediction(Duration::from_millis(10))
            .exact_queue_ready_risk_ns;
        assert!(
            spike_risk > stable_risk * 2,
            "a slow exact sample attacks risk immediately"
        );
        assert!(queue_spike_risk > stable_queue_risk * 2);

        for sample in 1..=5 {
            let composite_ns = spike_at + sample * 6_000_000;
            journal.record_frame_service_observation(service_observation(
                composite_ns,
                composite_ns + 1_000_000,
                composite_ns + 500_000,
                composite_ns + 2_000_000,
            ));
        }
        let short_recovery_risk = journal
            .prediction(Duration::from_millis(10))
            .exact_render_ready_risk_ns;
        let short_queue_recovery_risk = journal
            .prediction(Duration::from_millis(10))
            .exact_queue_ready_risk_ns;
        assert!(
            short_recovery_risk > stable_risk * 2,
            "a few fast samples must not erase learned positive tail risk"
        );
        assert!(short_queue_recovery_risk > stable_queue_risk * 2);

        let mut composite_ns = spike_at + 6 * 6_000_000;
        for _ in 0..100 {
            composite_ns += 600_000_000;
            journal.record_frame_service_observation(service_observation(
                composite_ns,
                composite_ns + 1_000_000,
                composite_ns + 500_000,
                composite_ns + 2_000_000,
            ));
        }
        let recovered_risk = journal
            .prediction(Duration::from_millis(10))
            .exact_render_ready_risk_ns;
        let recovered_queue_risk = journal
            .prediction(Duration::from_millis(10))
            .exact_queue_ready_risk_ns;
        assert!(recovered_risk < spike_risk);
        assert!(recovered_risk <= 1_100_000);
        assert!(recovered_queue_risk < queue_spike_risk);
        assert!(recovered_queue_risk <= 550_000);
    }

    #[test]
    fn warm_parallel_counts_kms_dispatch_exactly_once_on_queue_branch() {
        let mut journal = AdaptiveRenderJournal::default();
        record_uniform_prediction_inputs(
            &mut journal,
            1_000_000,
            2_000_000,
            4_000_000,
            250_000,
            500_000,
            1_000_000,
        );

        let prediction = journal.prediction_with_kms_guard(Duration::from_millis(10), 100_000);

        assert_eq!(prediction.p95_atomic_ioctl_ns, 500_000);
        assert_eq!(prediction.kms_dispatch_budget_ns, 1_000_000);
        assert_eq!(prediction.exact_render_ready_risk_ns, 2_000_000);
        assert_eq!(prediction.exact_queue_ready_risk_ns, 4_000_000);
        assert_eq!(prediction.parallel_service_ns, 5_000_000);
        assert_eq!(
            prediction.total_cost_ns,
            prediction.main_event_loop_wake_guard_ns + 100_000 + 5_000_000
        );
    }

    #[test]
    fn warm_parallel_does_not_apply_independent_floor_after_selection() {
        let mut journal = AdaptiveRenderJournal::default();
        record_uniform_prediction_inputs(
            &mut journal,
            10_000_000,
            100_000,
            100_000,
            0,
            100_000,
            500_000,
        );

        let prediction = journal.prediction_with_kms_guard(Duration::from_millis(20), 100_000);

        assert_eq!(prediction.warm_parallel_total_cost_ns, 1_200_000);
        assert_eq!(prediction.independent_p90_floor_ns, 11_100_000);
        assert_eq!(
            prediction.total_cost_ns,
            prediction.warm_parallel_total_cost_ns
        );
    }

    #[test]
    fn fewer_than_warm_samples_keep_cold_independent_selection() {
        let mut journal = AdaptiveRenderJournal::default();
        for sample in 0..WARM_PARALLEL_MIN_SAMPLES - 1 {
            let base_ns = 100_000_000 + sample as u64 * 10_000_000;
            journal.record_fixed_submission_budget(500_000);
            journal.record_frame_service_observation(service_observation(
                base_ns,
                base_ns + 8_000_000,
                base_ns + 9_000_000,
                base_ns + 10_000_000,
            ));
        }
        journal.record_fixed_submission_budget(500_000);

        let prediction = journal.prediction(Duration::from_millis(10));

        assert_eq!(
            prediction.estimator_mode,
            PredictionEstimatorMode::ColdStart
        );
        assert_eq!(
            prediction.total_cost_ns,
            prediction.independent_total_cost_ns
        );

        journal.record_frame_service_observation(service_observation(
            300_000_000,
            308_000_000,
            309_000_000,
            310_000_000,
        ));
        assert_eq!(
            journal.prediction_estimator_mode(),
            PredictionEstimatorMode::WarmParallel
        );
    }

    #[test]
    fn warm_parallel_requires_twenty_valid_kms_budget_samples() {
        let mut journal = AdaptiveRenderJournal::default();
        for sample in 0..WARM_PARALLEL_MIN_SAMPLES {
            let base_ns = 100_000_000 + sample as u64 * 10_000_000;
            if sample + 1 < WARM_PARALLEL_MIN_SAMPLES {
                journal.record_fixed_submission_budget(500_000);
            }
            journal.record_frame_service_observation(service_observation(
                base_ns,
                base_ns + 2_000_000,
                base_ns + 1_000_000,
                base_ns + 3_000_000,
            ));
        }
        assert_eq!(journal.exact_parallel.samples(), WARM_PARALLEL_MIN_SAMPLES);
        assert_eq!(
            journal.prediction_estimator_mode(),
            PredictionEstimatorMode::ColdStart
        );

        journal.record_fixed_submission_budget(500_000);
        assert_eq!(
            journal.prediction_estimator_mode(),
            PredictionEstimatorMode::WarmParallel
        );
    }

    #[test]
    fn miss_recovery_requires_clean_exact_horizon_and_resets_on_miss() {
        let mut journal = AdaptiveRenderJournal::default();
        record_uniform_prediction_inputs(
            &mut journal,
            1_000_000,
            1_000_000,
            1_000_000,
            0,
            0,
            500_000,
        );
        assert_eq!(
            journal.prediction_estimator_mode(),
            PredictionEstimatorMode::WarmParallel
        );

        journal.note_proven_deadline_miss(EstimatorRecoveryDisposition::ResetIndependentHorizon);
        let recovering = journal.prediction(Duration::from_millis(10));
        assert_eq!(journal.missed_deadlines, 1);
        assert_eq!(
            recovering.miss_recovery_remaining,
            MISS_RECOVERY_EXACT_SUCCESSES
        );
        assert_eq!(
            recovering.estimator_mode,
            PredictionEstimatorMode::MissRecovery
        );
        assert_eq!(
            recovering.total_cost_ns,
            recovering.independent_total_cost_ns
        );

        for sample in 0..MISS_RECOVERY_EXACT_SUCCESSES - 1 {
            let offset_ns = sample as u64 * 10_000;
            journal.record_frame_service_observation(service_observation(
                1_000_000 + offset_ns,
                1_001_000 + offset_ns,
                1_002_000 + offset_ns,
                1_003_000 + offset_ns,
            ));
        }
        assert_eq!(
            journal
                .prediction(Duration::from_millis(10))
                .miss_recovery_remaining,
            1
        );
        assert_eq!(
            journal.prediction_estimator_mode(),
            PredictionEstimatorMode::MissRecovery
        );

        let mut approximate = service_observation(2_000_000, 2_001_000, 2_002_000, 2_003_000);
        approximate.fence_signaled_at = Some((
            MonotonicTimestampNs::new(2_001_000),
            FenceTimestampQuality::ObservedApproximate,
        ));
        journal.record_frame_service_observation(approximate);
        let mut incomplete = service_observation(3_000_000, 3_001_000, 3_002_000, 3_003_000);
        incomplete.worker_queued_at = None;
        journal.record_frame_service_observation(incomplete);
        assert_eq!(
            journal
                .prediction(Duration::from_millis(10))
                .miss_recovery_remaining,
            1
        );

        journal.record_frame_service_observation(service_observation(
            4_000_000, 4_001_000, 4_002_000, 4_003_000,
        ));
        assert_eq!(
            journal.prediction_estimator_mode(),
            PredictionEstimatorMode::WarmParallel
        );

        journal.note_proven_deadline_miss(EstimatorRecoveryDisposition::ResetIndependentHorizon);
        for sample in 0..MISS_RECOVERY_EXACT_SUCCESSES / 2 {
            let offset_ns = sample as u64 * 10_000;
            journal.record_frame_service_observation(service_observation(
                5_000_000 + offset_ns,
                5_001_000 + offset_ns,
                5_002_000 + offset_ns,
                5_003_000 + offset_ns,
            ));
        }
        assert_eq!(
            journal
                .prediction(Duration::from_millis(10))
                .miss_recovery_remaining,
            MISS_RECOVERY_EXACT_SUCCESSES / 2
        );
        journal.note_proven_deadline_miss(EstimatorRecoveryDisposition::ResetIndependentHorizon);
        assert_eq!(
            journal
                .prediction(Duration::from_millis(10))
                .miss_recovery_remaining,
            MISS_RECOVERY_EXACT_SUCCESSES
        );
        for sample in 0..MISS_RECOVERY_EXACT_SUCCESSES - 1 {
            let offset_ns = sample as u64 * 10_000;
            journal.record_frame_service_observation(service_observation(
                6_000_000 + offset_ns,
                6_001_000 + offset_ns,
                6_002_000 + offset_ns,
                6_003_000 + offset_ns,
            ));
        }
        assert_eq!(
            journal.prediction_estimator_mode(),
            PredictionEstimatorMode::MissRecovery
        );
        journal.record_frame_service_observation(service_observation(
            7_000_000, 7_001_000, 7_002_000, 7_003_000,
        ));
        assert_eq!(
            journal.prediction_estimator_mode(),
            PredictionEstimatorMode::WarmParallel
        );
    }

    #[test]
    fn cause_specific_recovery_preserves_warm_state_and_active_recovery() {
        let mut journal = AdaptiveRenderJournal::default();
        record_uniform_prediction_inputs(
            &mut journal,
            1_000_000,
            1_000_000,
            1_000_000,
            0,
            0,
            500_000,
        );
        assert_eq!(
            journal.prediction_estimator_mode(),
            PredictionEstimatorMode::WarmParallel
        );

        journal.note_proven_deadline_miss(EstimatorRecoveryDisposition::PreserveEstimatorState);
        assert_eq!(journal.missed_deadlines, 1);
        assert_eq!(
            journal.prediction_estimator_mode(),
            PredictionEstimatorMode::WarmParallel
        );
        assert_eq!(
            journal
                .prediction(Duration::from_millis(10))
                .miss_recovery_remaining,
            0
        );

        journal.note_proven_deadline_miss(EstimatorRecoveryDisposition::ResetIndependentHorizon);
        for _ in 0..5 {
            journal.record_frame_service_observation(service_observation(
                1_000_000, 1_001_000, 1_002_000, 1_003_000,
            ));
        }
        let remaining = journal
            .prediction(Duration::from_millis(10))
            .miss_recovery_remaining;
        journal.note_proven_deadline_miss(EstimatorRecoveryDisposition::PreserveEstimatorState);
        assert_eq!(journal.missed_deadlines, 3);
        assert_eq!(
            journal
                .prediction(Duration::from_millis(10))
                .miss_recovery_remaining,
            remaining,
            "a self-recovered KMS miss must not clear or reset active recovery"
        );
    }

    #[test]
    fn dynamic_safety_margin_clamps_to_refresh_quarter() {
        let mut journal = AdaptiveRenderJournal::default();
        for _ in 0..20 {
            journal.record_wake_lateness(4_000_000);
            journal.record_atomic_submit(100_000);
            journal.record_fixed_submission_budget(150_000);
        }
        let prediction = journal.prediction(Duration::from_millis(4));
        assert_eq!(prediction.main_event_loop_wake_guard_ns, 1_000_000);
        assert_eq!(prediction.kms_dispatch_budget_ns, 150_000);
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
    fn off_never_enters_and_force_enters_for_visual_work() {
        let mut off = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Off);
        off.apply_capability(TripleCapability::Capable);
        off.observe_overlap_required(1);
        assert_eq!(off.mode(), AdaptiveBufferingMode::Double);

        let mut force = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Force);
        force.apply_capability(TripleCapability::Capable);
        force.observe_overlap_required(0);
        assert_eq!(force.mode(), AdaptiveBufferingMode::Triple);
        assert_eq!(
            force.entry_reason(),
            Some(TripleEntryReason::ForcedValidation)
        );
    }

    #[test]
    fn overlap_is_observed_once_per_predecessor_opportunity() {
        let mut policy = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Auto);
        policy.apply_capability(TripleCapability::Capable);
        let target = crate::native::presentation_deadline::PresentationTarget {
            sequence: 10,
            presentation_time: MonotonicTimestampNs::new(100),
            submit_not_before: MonotonicTimestampNs::new(0),
            render_start_deadline: MonotonicTimestampNs::new(0),
            refresh_interval: Duration::from_nanos(10),
            reason:
                crate::native::presentation_deadline::PresentationTargetReason::PredictedPressure,
            clock_generation: 1,
            estimated: false,
            predicted_unreachable: false,
            physical_claim: crate::native::presentation_deadline::PrimaryRefreshClaim {
                sequence: 10,
                presentation_time: MonotonicTimestampNs::new(100),
                clock_generation: 1,
            },
            selection_evidence: Default::default(),
        };

        policy.observe_overlap_for_target(Some(target), 1);
        for _ in 0..10 {
            policy.observe_overlap_for_target(Some(target), 0);
        }
        assert_eq!(policy.extra_credit_grants(), 1);
        assert_eq!(policy.future_primary_credit(), 2);

        policy.observe_overlap_for_target(None, 0);
        for sequence in 11..=20 {
            let target = crate::native::presentation_deadline::PresentationTarget {
                sequence,
                physical_claim: crate::native::presentation_deadline::PrimaryRefreshClaim {
                    sequence,
                    presentation_time: MonotonicTimestampNs::new(100),
                    clock_generation: 1,
                },
                ..target
            };
            policy.observe_overlap_for_target(Some(target), 0);
        }
        assert_eq!(policy.future_primary_credit(), 1);
        assert_eq!(policy.extra_credit_revokes(), 1);
    }

    #[test]
    fn presentation_sequence_slip_becomes_proven_miss() {
        assert_eq!(
            merge_presentation_miss(None, 40, 41),
            Some(ProvenDeadlineMiss::KmsApplyGuard)
        );
    }

    #[test]
    fn specific_deadline_miss_has_precedence_over_presentation_slip() {
        assert_eq!(
            merge_presentation_miss(Some(ProvenDeadlineMiss::ExactRender), 40, 41,),
            Some(ProvenDeadlineMiss::ExactRender)
        );
        assert_eq!(
            merge_presentation_miss(Some(ProvenDeadlineMiss::KmsDispatch), 40, 42,),
            Some(ProvenDeadlineMiss::KmsDispatch)
        );
    }

    #[test]
    fn on_time_presentation_does_not_create_miss() {
        assert_eq!(merge_presentation_miss(None, 40, 40), None);
        assert_eq!(merge_presentation_miss(None, 41, 40), None);
    }

    #[test]
    fn kms_presentation_miss_does_not_grant_render_credit() {
        let mut policy = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Auto);
        policy.apply_capability(TripleCapability::Capable);
        policy.observe_o1_outcome(Some(ProvenDeadlineMiss::KmsApplyGuard));

        assert_eq!(policy.future_primary_credit(), 1);
        assert_eq!(policy.extra_credit_grants(), 0);
        assert_eq!(policy.entry_reason(), None);
    }

    #[test]
    fn only_render_readiness_misses_grant_render_credit() {
        for (miss, reason) in [
            (
                ProvenDeadlineMiss::ExactRender,
                TripleEntryReason::ProvenReadinessMiss,
            ),
            (
                ProvenDeadlineMiss::GuardedApproximateRender,
                TripleEntryReason::ProvenReadinessMiss,
            ),
        ] {
            let mut policy = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Auto);
            policy.apply_capability(TripleCapability::Capable);
            policy.observe_o1_outcome(Some(miss));
            assert_eq!(policy.mode(), AdaptiveBufferingMode::Triple);
            assert_eq!(policy.entry_reason(), Some(reason));
        }
    }

    #[test]
    fn kms_dispatch_miss_does_not_grant_render_credit() {
        let mut policy = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Auto);
        policy.apply_capability(TripleCapability::Capable);
        policy.observe_o1_outcome(Some(ProvenDeadlineMiss::KmsDispatch));

        assert_eq!(policy.future_primary_credit(), 1);
        assert_eq!(policy.extra_credit_grants(), 0);
    }

    #[test]
    fn negative_overlap_can_revoke_while_depth_two_is_owned() {
        let mut policy = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Auto);
        policy.apply_capability(TripleCapability::Capable);
        let predecessor = crate::native::presentation_deadline::PresentationTarget {
            sequence: 10,
            presentation_time: MonotonicTimestampNs::new(100),
            submit_not_before: MonotonicTimestampNs::new(0),
            render_start_deadline: MonotonicTimestampNs::new(0),
            refresh_interval: Duration::from_nanos(10),
            reason: crate::native::presentation_deadline::PresentationTargetReason::Normal,
            clock_generation: 1,
            estimated: false,
            predicted_unreachable: false,
            physical_claim: crate::native::presentation_deadline::PrimaryRefreshClaim {
                sequence: 10,
                presentation_time: MonotonicTimestampNs::new(100),
                clock_generation: 1,
            },
            selection_evidence: Default::default(),
        };

        policy.observe_overlap_for_target(Some(predecessor), 1);
        for sequence in 11..=20 {
            policy.observe_overlap_for_target(
                Some(crate::native::presentation_deadline::PresentationTarget {
                    sequence,
                    physical_claim: crate::native::presentation_deadline::PrimaryRefreshClaim {
                        sequence,
                        presentation_time: MonotonicTimestampNs::new(100),
                        clock_generation: 1,
                    },
                    ..predecessor
                }),
                0,
            );
        }

        assert_eq!(policy.future_primary_credit(), 1);
        assert_eq!(policy.extra_credit_revokes(), 1);
    }

    #[test]
    fn no_miss_and_no_visual_work_does_not_transition() {
        let policy = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Auto);

        assert_eq!(policy.mode(), AdaptiveBufferingMode::Double);
        assert_eq!(policy.entry_reason(), None);
    }

    #[test]
    fn kms_presentation_miss_does_not_queue_scheduler_work_by_itself() {
        let mut policy = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Auto);
        let scheduler = NativeFrameScheduler::new(165, 0);

        policy.apply_capability(TripleCapability::Capable);
        policy.observe_o1_outcome(Some(ProvenDeadlineMiss::KmsApplyGuard));

        assert_eq!(policy.mode(), AdaptiveBufferingMode::Double);
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
        let mut force = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Force);
        force.apply_capability(TripleCapability::Capable);
        force.observe_overlap_required(0);
        let mut auto = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Auto);
        auto.apply_capability(TripleCapability::Capable);
        auto.observe_overlap_required(1);

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
    fn cold_wake_guard_rearms_for_each_compositor_idle_episode() {
        let refresh = Duration::from_millis(10);
        let mut journal = AdaptiveRenderJournal::default();
        journal.record_render_sample(1_000_000, MonotonicTimestampNs::new(10_000_000));

        let first_cold = journal.prediction_at(MonotonicTimestampNs::new(1_010_000_000), refresh);
        journal.record_render_sample(1_000_000, MonotonicTimestampNs::new(1_010_000_000));
        let warm = journal.prediction_at(MonotonicTimestampNs::new(1_010_000_001), refresh);
        let second_cold = journal.prediction_at(MonotonicTimestampNs::new(2_010_000_000), refresh);

        assert!(first_cold.cold_wake_guard);
        assert_eq!(first_cold.total_cost_ns, 9_900_000);
        assert!(!warm.cold_wake_guard);
        assert!(second_cold.cold_wake_guard);
        assert_eq!(second_cold.total_cost_ns, 9_900_000);
    }

    #[test]
    fn cold_wake_prediction_remains_guarded_until_real_composited_sample() {
        let refresh = Duration::from_millis(10);
        let mut journal = AdaptiveRenderJournal::default();
        journal.record_render_sample(1_000_000, MonotonicTimestampNs::new(10_000_000));

        for now in [1_010_000_000, 1_010_000_001, 1_010_000_002] {
            assert!(
                journal
                    .prediction_at(MonotonicTimestampNs::new(now), refresh)
                    .cold_wake_guard
            );
        }

        journal.record_render_sample(1_000_000, MonotonicTimestampNs::new(1_010_000_002));
        let warm = journal.prediction_at(MonotonicTimestampNs::new(1_010_000_003), refresh);
        assert!(!warm.cold_wake_guard);
    }

    #[test]
    fn prediction_without_compositor_render_history_is_conservatively_cold() {
        let journal = AdaptiveRenderJournal::default();
        let prediction = journal.prediction_at(
            MonotonicTimestampNs::new(50_000_000_000),
            Duration::from_nanos(6_060_606),
        );

        assert!(prediction.cold_wake_guard);
        assert_eq!(prediction.total_cost_ns, 5_960_606);
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
    fn queue_residency_is_observable_but_never_part_of_warm_prediction() {
        let mut journal = AdaptiveRenderJournal::default();
        record_uniform_prediction_inputs(
            &mut journal,
            1_000_000,
            3_000_000,
            1_000_000,
            0,
            250_000,
            500_000,
        );
        let baseline = journal.prediction(Duration::from_millis(10));
        journal.record_worker_queue_residency(u64::MAX);
        let delayed_queue = journal.prediction(Duration::from_millis(10));

        assert_eq!(
            baseline.estimator_mode,
            PredictionEstimatorMode::WarmParallel
        );
        assert_eq!(delayed_queue.p95_worker_queue_residency_ns, u64::MAX);
        assert_eq!(
            baseline.exact_render_ready_risk_ns,
            delayed_queue.exact_render_ready_risk_ns
        );
        assert_eq!(
            baseline.exact_queue_ready_risk_ns,
            delayed_queue.exact_queue_ready_risk_ns
        );
        assert_eq!(
            baseline.kms_dispatch_budget_ns,
            delayed_queue.kms_dispatch_budget_ns
        );
        assert_eq!(baseline.total_cost_ns, delayed_queue.total_cost_ns);
    }

    #[test]
    fn timing_dimensions_are_independently_bounded() {
        let mut journal = AdaptiveRenderJournal::default();
        for sample in 1..=121 {
            let at = MonotonicTimestampNs::new(sample);
            journal.record_render_sample(sample, at);
            journal.record_wake_lateness(sample + 1_000);
            journal.record_worker_queue_residency(sample + 2_000);
            journal.record_fixed_timed_wake(sample + 3_000);
            journal.record_atomic_submit(sample + 4_000);
            journal.record_fixed_submission_budget(sample + 5_000);
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
        assert_eq!(
            journal.fixed_submission_budget_samples_ns.len(),
            SAMPLE_CAPACITY
        );
        assert_eq!(journal.target_slip_samples_ns.len(), SAMPLE_CAPACITY);

        let prediction = journal.prediction(Duration::from_millis(10));
        assert_eq!(prediction.p90_recent_render_ns, 109);
        assert_eq!(prediction.p95_wake_lateness_ns, 1_115);
        assert_eq!(prediction.p95_worker_submit_wake_lateness_ns, 3_115);
        assert_eq!(prediction.p95_worker_queue_residency_ns, 2_115);
        assert_eq!(prediction.p95_atomic_ioctl_ns, 4_115);
        assert_eq!(prediction.kms_dispatch_budget_ns, 5_121);
        assert_eq!(prediction.p95_target_slip_ns, 6_115);
    }

    #[test]
    fn main_and_worker_wake_lateness_diagnostics_remain_independent() {
        let mut journal = AdaptiveRenderJournal::default();
        journal.record_wake_lateness(25_000);

        let main_wake_only = journal.prediction(Duration::from_millis(10));
        assert_eq!(main_wake_only.p95_wake_lateness_ns, 25_000);
        assert_eq!(main_wake_only.p95_worker_submit_wake_lateness_ns, 0);

        journal.record_fixed_timed_wake(40_000);
        let both_domains = journal.prediction(Duration::from_millis(10));
        assert_eq!(both_domains.p95_wake_lateness_ns, 25_000);
        assert_eq!(both_domains.p95_worker_submit_wake_lateness_ns, 40_000);
        assert_eq!(both_domains.total_cost_ns, main_wake_only.total_cost_ns);

        let mut warm_journal = AdaptiveRenderJournal::default();
        record_uniform_prediction_inputs(
            &mut warm_journal,
            3_000_000,
            5_000_000,
            1_000_000,
            25_000,
            100_000,
            500_000,
        );
        let warm_before = warm_journal.prediction(Duration::from_millis(10));
        assert_eq!(
            warm_before.estimator_mode,
            PredictionEstimatorMode::WarmParallel
        );

        warm_journal.record_fixed_timed_wake(7_000_000);
        let warm_after = warm_journal.prediction(Duration::from_millis(10));
        assert_eq!(
            warm_after.p95_wake_lateness_ns,
            warm_before.p95_wake_lateness_ns
        );
        assert_eq!(warm_after.p95_worker_submit_wake_lateness_ns, 7_000_000);
        assert_eq!(
            warm_after.kms_dispatch_budget_ns,
            warm_before.kms_dispatch_budget_ns
        );
        assert_eq!(warm_after.total_cost_ns, warm_before.total_cost_ns);
    }

    #[test]
    fn force_never_bypasses_an_exact_capability_blocker() {
        let mut controller = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Force);
        controller.apply_capability(TripleCapability::Unavailable(
            TripleCapabilityBlocker::PrimaryInFenceUnavailable,
        ));
        controller.observe_overlap_required(1);

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
    fn capability_loss_drains_two_future_primaries_before_leaving_triple() {
        let blocker = TripleCapabilityBlocker::SubmissionTransportUnhealthy;
        let mut controller = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Auto);
        controller.apply_capability(TripleCapability::Capable);
        controller.observe_overlap_required(1);
        assert_eq!(controller.mode(), AdaptiveBufferingMode::Triple);

        controller.apply_capability(TripleCapability::Unavailable(blocker));
        assert_eq!(controller.mode(), AdaptiveBufferingMode::Triple);
        controller.observe_overlap_required(0);
        assert_eq!(controller.mode(), AdaptiveBufferingMode::Double);
    }
}

#[cfg(test)]
mod exact_parallel_diagnostics_tests {
    use super::*;

    const REFRESH_NS: u64 = 6_060_606;

    fn observation(composite_ns: u64, fence_ns: u64, handoff_ns: u64) -> FrameTimingObservation {
        super::tests::service_observation(composite_ns, fence_ns, handoff_ns, handoff_ns)
    }

    fn rendered(journal: &mut AdaptiveRenderJournal, observation: FrameTimingObservation, at: u64) {
        journal.record_frame_service_observation_with_rendered(
            observation,
            Some(MonotonicTimestampNs::new(at)),
        );
    }

    #[test]
    fn raw_branch_window_exposes_a_rare_spike_and_selected_source() {
        let mut journal = AdaptiveRenderJournal::default();
        for sample in 0..SAMPLE_CAPACITY {
            let composite_ns = 100_000_000 + sample as u64 * 10_000_000;
            let render_ready_ns = if sample + 1 == SAMPLE_CAPACITY {
                8_000_000
            } else {
                1_000_000
            };
            rendered(
                &mut journal,
                observation(
                    composite_ns,
                    composite_ns + render_ready_ns,
                    composite_ns + 1_000_000,
                ),
                composite_ns + 500_000,
            );
        }

        let diagnostics = journal.exact_parallel_diagnostics(REFRESH_NS);
        let render = diagnostics.render_ready;
        assert_eq!(render.window.count, 120);
        assert_eq!(render.window.latest_ns, 8_000_000);
        assert_eq!(render.window.p50_ns, 1_000_000);
        assert_eq!(render.window.p90_ns, 1_000_000);
        assert_eq!(render.window.p95_ns, 1_000_000);
        assert_eq!(render.window.p99_ns, 1_000_000);
        assert_eq!(render.window.max_ns, 8_000_000);
        assert_eq!(render.window.over_refresh_samples, 1);
        assert_eq!(render.window.over_two_refresh_samples, 0);
        assert!(render.central_risk_ns > render.window.p95_ns);
        assert_eq!(
            render.selected_risk_ns,
            render.window.p95_ns.max(render.central_risk_ns)
        );
        assert_eq!(
            render.selected_source,
            if render.central_risk_ns > render.window.p95_ns {
                BranchRiskSource::PositiveDeviation
            } else {
                BranchRiskSource::RecentP95
            }
        );
        assert_eq!(render.selected_source, BranchRiskSource::PositiveDeviation);
    }

    #[test]
    fn diagnostic_snapshots_do_not_change_prediction() {
        let mut journal = AdaptiveRenderJournal::default();
        for sample in 0..WARM_PARALLEL_MIN_SAMPLES {
            let composite_ns = 200_000_000 + sample as u64 * 10_000_000;
            journal.record_fixed_submission_budget(500_000);
            rendered(
                &mut journal,
                observation(
                    composite_ns,
                    composite_ns + 3_000_000,
                    composite_ns + 4_000_000,
                ),
                composite_ns + 2_000_000,
            );
        }
        let before = journal.prediction_with_kms_guard(Duration::from_millis(10), 300_000);
        for _ in 0..4 {
            let _ = journal.exact_parallel_diagnostics(10_000_000);
        }
        let after = journal.prediction_with_kms_guard(Duration::from_millis(10), 300_000);
        assert_eq!(after, before);
    }

    #[test]
    fn decomposition_tracks_render_observation_handoff_and_gpu_tail() {
        let mut journal = AdaptiveRenderJournal::default();
        rendered(
            &mut journal,
            observation(0, 5_000_000, 4_000_000),
            3_000_000,
        );

        let diagnostics = journal.exact_parallel_diagnostics(REFRESH_NS);
        assert_eq!(diagnostics.render_ready.window.latest_ns, 5_000_000);
        assert_eq!(diagnostics.queue_ready.window.latest_ns, 4_000_000);
        assert_eq!(
            diagnostics.composite_to_rendered_observed.latest_ns,
            3_000_000
        );
        assert_eq!(diagnostics.post_render_handoff.latest_ns, 1_000_000);
        assert_eq!(
            diagnostics.fence_after_rendered_observed.latest_ns,
            2_000_000
        );
        assert_eq!(diagnostics.rendered_observed_after_fence.count, 0);
        assert_eq!(diagnostics.queue_before_fence_samples, 1);
        assert_eq!(diagnostics.fence_before_queue_samples, 0);
        assert_eq!(diagnostics.equal_samples, 0);
        assert_eq!(diagnostics.decomposition_invalid_samples, 0);
    }

    #[test]
    fn rendered_observation_after_fence_is_counted_separately() {
        let mut journal = AdaptiveRenderJournal::default();
        rendered(
            &mut journal,
            observation(0, 2_000_000, 6_000_000),
            5_000_000,
        );

        let diagnostics = journal.exact_parallel_diagnostics(REFRESH_NS);
        assert_eq!(diagnostics.render_ready.window.latest_ns, 2_000_000);
        assert_eq!(diagnostics.queue_ready.window.latest_ns, 6_000_000);
        assert_eq!(
            diagnostics.composite_to_rendered_observed.latest_ns,
            5_000_000
        );
        assert_eq!(diagnostics.post_render_handoff.latest_ns, 1_000_000);
        assert_eq!(diagnostics.fence_after_rendered_observed.count, 0);
        assert_eq!(
            diagnostics.rendered_observed_after_fence.latest_ns,
            3_000_000
        );
        assert_eq!(diagnostics.queue_before_fence_samples, 0);
        assert_eq!(diagnostics.fence_before_queue_samples, 1);
    }

    #[test]
    fn equal_branch_readiness_has_its_own_order_count() {
        let mut journal = AdaptiveRenderJournal::default();
        rendered(
            &mut journal,
            observation(0, 5_000_000, 5_000_000),
            3_000_000,
        );
        let diagnostics = journal.exact_parallel_diagnostics(REFRESH_NS);
        assert_eq!(diagnostics.equal_samples, 1);
        assert_eq!(diagnostics.queue_before_fence_samples, 0);
        assert_eq!(diagnostics.fence_before_queue_samples, 0);
    }

    #[test]
    fn malformed_rendered_timestamp_does_not_reject_exact_branch_training() {
        let malformed = observation(100, 5_100, 4_100);
        let mut journal = AdaptiveRenderJournal::default();
        let mut control = AdaptiveRenderJournal::default();
        rendered(&mut journal, malformed, 99);
        control.record_frame_service_observation(malformed);

        assert_eq!(
            journal.prediction(Duration::from_millis(10)),
            control.prediction(Duration::from_millis(10))
        );
        let diagnostics = journal.exact_parallel_diagnostics(REFRESH_NS);
        assert_eq!(diagnostics.render_ready.window.count, 1);
        assert_eq!(diagnostics.queue_ready.window.count, 1);
        assert_eq!(diagnostics.composite_to_rendered_observed.count, 0);
        assert_eq!(diagnostics.post_render_handoff.count, 0);
        assert_eq!(diagnostics.fence_after_rendered_observed.count, 0);
        assert_eq!(diagnostics.rendered_observed_after_fence.count, 0);
        assert_eq!(diagnostics.decomposition_invalid_samples, 1);
    }

    #[test]
    fn malformed_handoff_rejects_only_handoff_decomposition() {
        let mut journal = AdaptiveRenderJournal::default();
        rendered(&mut journal, observation(0, 5_000, 3_000), 4_000);

        let diagnostics = journal.exact_parallel_diagnostics(REFRESH_NS);
        assert_eq!(diagnostics.render_ready.window.latest_ns, 5_000);
        assert_eq!(diagnostics.queue_ready.window.latest_ns, 3_000);
        assert_eq!(diagnostics.composite_to_rendered_observed.latest_ns, 4_000);
        assert_eq!(diagnostics.post_render_handoff.count, 0);
        assert_eq!(diagnostics.fence_after_rendered_observed.latest_ns, 1_000);
        assert_eq!(diagnostics.decomposition_invalid_samples, 1);
        assert_eq!(
            journal
                .prediction(Duration::from_millis(10))
                .parallel_samples,
            1
        );
    }

    #[test]
    fn approximate_fence_stays_out_of_scheduling_and_diagnostic_histories() {
        let mut observation = observation(0, 5_000_000, 1_000_000);
        observation.fence_signaled_at = Some((
            MonotonicTimestampNs::new(5_000_000),
            FenceTimestampQuality::ObservedApproximate,
        ));
        let mut journal = AdaptiveRenderJournal::default();
        let accepted = journal.record_frame_service_observation_with_rendered(
            observation,
            Some(MonotonicTimestampNs::new(500_000)),
        );

        assert_eq!(accepted, None);
        assert_eq!(
            journal
                .prediction(Duration::from_millis(10))
                .parallel_samples,
            0
        );
        let diagnostics = journal.exact_parallel_diagnostics(REFRESH_NS);
        assert_eq!(diagnostics.render_ready.window.count, 0);
        assert_eq!(diagnostics.queue_ready.window.count, 0);
        assert_eq!(diagnostics.decomposition_invalid_samples, 0);
    }

    #[test]
    fn exact_parallel_diagnostic_histories_remain_bounded() {
        let mut journal = AdaptiveRenderJournal::default();
        for sample in 0..=SAMPLE_CAPACITY {
            let composite_ns = sample as u64 * 10_000_000;
            rendered(
                &mut journal,
                observation(
                    composite_ns,
                    composite_ns + 2_000_000,
                    composite_ns + 2_000_000,
                ),
                composite_ns + 1_000_000,
            );
        }

        let diagnostics = journal.exact_parallel_diagnostics(REFRESH_NS);
        assert_eq!(diagnostics.render_ready.window.count, SAMPLE_CAPACITY);
        assert_eq!(diagnostics.queue_ready.window.count, SAMPLE_CAPACITY);
        assert_eq!(
            diagnostics.composite_to_rendered_observed.count,
            SAMPLE_CAPACITY
        );
        assert_eq!(diagnostics.post_render_handoff.count, SAMPLE_CAPACITY);
        assert_eq!(
            diagnostics.fence_after_rendered_observed.count,
            SAMPLE_CAPACITY
        );
        assert_eq!(diagnostics.rendered_observed_after_fence.count, 0);
    }
}

#[cfg(test)]
mod o1_tests {
    use super::*;

    #[test]
    fn auto_credit_grants_from_overlap_not_total_refresh_cost() {
        let mut controller = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Auto);
        controller.apply_capability(TripleCapability::Capable);

        controller.observe_overlap_required(1);

        assert_eq!(controller.future_primary_credit(), 2);
        assert_eq!(controller.mode(), AdaptiveBufferingMode::Triple);
    }

    #[test]
    fn auto_credit_revokes_after_stable_negative_slack() {
        let mut controller = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Auto);
        controller.apply_capability(TripleCapability::Capable);
        controller.observe_overlap_required(1);

        for _ in 0..10 {
            controller.observe_overlap_required(0);
        }

        assert_eq!(controller.future_primary_credit(), 1);
        assert_eq!(controller.mode(), AdaptiveBufferingMode::Double);
    }

    #[test]
    fn credit_two_does_not_change_after_capability_is_lost_until_owned_depth_drains() {
        let mut controller = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Auto);
        controller.apply_capability(TripleCapability::Capable);
        controller.observe_overlap_required(1);
        controller.apply_capability(TripleCapability::Unavailable(
            TripleCapabilityBlocker::OutputGenerationUnstable,
        ));
        controller.observe_overlap_required(0);

        assert_eq!(controller.future_primary_credit(), 1);
        assert_eq!(controller.mode(), AdaptiveBufferingMode::Double);
    }
}

#[cfg(test)]
mod doctor_tests {
    use super::*;
    use crate::control_snapshots::DoctorSeverity;

    #[test]
    fn doctor_severity_does_not_penalize_intentional_double_buffering() {
        assert_eq!(
            triple_buffering_doctor_severity(
                AdaptiveTripleBufferPolicy::Off,
                AdaptiveBufferingMode::Double,
                false,
            ),
            DoctorSeverity::Ok
        );
        assert_eq!(
            triple_buffering_doctor_severity(
                AdaptiveTripleBufferPolicy::Auto,
                AdaptiveBufferingMode::Double,
                false,
            ),
            DoctorSeverity::Ok
        );
        assert_eq!(
            triple_buffering_doctor_severity(
                AdaptiveTripleBufferPolicy::Force,
                AdaptiveBufferingMode::Triple,
                false,
            ),
            DoctorSeverity::Ok
        );
        assert_eq!(
            triple_buffering_doctor_severity(
                AdaptiveTripleBufferPolicy::Force,
                AdaptiveBufferingMode::Double,
                false,
            ),
            DoctorSeverity::Ok
        );
        assert_eq!(
            triple_buffering_doctor_severity(
                AdaptiveTripleBufferPolicy::Force,
                AdaptiveBufferingMode::Double,
                true,
            ),
            DoctorSeverity::Warning
        );
    }
}
