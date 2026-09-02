//! Deadline risk prediction and bounded adaptive-buffering policy.
#![allow(dead_code)] // Wired into the native runtime in Task 12.

use crate::native::buffering::{
    O1CreditDemandController, O1CreditDemandReason, PresentationOpportunityId,
};
use crate::native::presentation_deadline::{MonotonicTimestampNs, PresentationTarget};
use crate::native::scheduler::NativeOutputPacingMode;
use std::collections::VecDeque;
use std::time::Duration;

const SAMPLE_CAPACITY: usize = 120;

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
    pub p95_worker_pre_submit_ns: u64,
    pub p95_worker_dispatch_ns: u64,
    pub p95_atomic_ioctl_ns: u64,
    pub main_event_loop_wake_guard_ns: u64,
    pub kms_dispatch_budget_ns: u64,
    pub kms_apply_guard_ns: u64,
    pub kms_total_lead_ns: u64,
    pub p95_target_slip_ns: u64,
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
    worker_pre_submit_samples_ns: VecDeque<u64>,
    worker_dispatch_samples_ns: VecDeque<u64>,
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
            worker_pre_submit_samples_ns: VecDeque::with_capacity(SAMPLE_CAPACITY),
            worker_dispatch_samples_ns: VecDeque::with_capacity(SAMPLE_CAPACITY),
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

    pub fn record_worker_pre_submit(&mut self, sample_ns: u64) {
        push_bounded(&mut self.worker_pre_submit_samples_ns, sample_ns);
    }

    pub fn record_worker_dispatch(&mut self, sample_ns: u64) {
        push_bounded(&mut self.worker_dispatch_samples_ns, sample_ns);
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
        self.base_prediction(refresh_interval, idle, 0)
    }

    pub fn prediction_at_with_kms_guard(
        &mut self,
        now: MonotonicTimestampNs,
        refresh_interval: Duration,
        kms_apply_guard_ns: u64,
    ) -> RenderPrediction {
        let refresh_ns = duration_ns(refresh_interval).max(1);
        let idle = !self.idle_guard_consumed
            && self.last_presented_at.is_some_and(|last| {
                now.get().saturating_sub(last.get()) >= refresh_ns.saturating_mul(100)
            });
        if idle {
            self.idle_guard_consumed = true;
        }
        self.base_prediction(refresh_interval, idle, kms_apply_guard_ns)
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
        idle: bool,
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
        if (1..20).contains(&self.render_samples_ns.len()) {
            render_risk =
                render_risk.max(self.render_samples_ns.iter().copied().max().unwrap_or(0));
        }
        let p95_wake = nearest_rank(&self.wake_lateness_samples_ns, 95);
        let p95_ioctl = nearest_rank(&self.atomic_submit_samples_ns, 95);
        let p95_worker_wake = nearest_rank(&self.worker_submit_wake_lateness_samples_ns, 95);
        let p95_worker_pre_submit = nearest_rank(&self.worker_pre_submit_samples_ns, 95);
        let p95_worker_dispatch = nearest_rank(&self.worker_dispatch_samples_ns, 95);
        let p95_queue_residency = nearest_rank(&self.worker_queue_residency_samples_ns, 95);
        let measured_dispatch_budget = p95_worker_wake
            .saturating_add(p95_worker_dispatch.max(p95_ioctl))
            .saturating_add(50_000);
        let exported_submission_budget = nearest_rank(&self.submission_budget_samples_ns, 95);
        let kms_dispatch_budget = if exported_submission_budget != 0 {
            exported_submission_budget
        } else if self.atomic_submit_samples_ns.len() < 20 {
            measured_dispatch_budget.max(250_000)
        } else {
            measured_dispatch_budget
        };
        let ceiling = 2_000_000_u64.min(refresh_ns / 4).max(500_000);
        let dynamic_margin = p95_wake.saturating_add(250_000).clamp(500_000, ceiling);
        let main_event_loop_wake_guard = if self.wake_lateness_samples_ns.len() < 20
            || self.atomic_submit_samples_ns.len() < 20
        {
            dynamic_margin.max(1_000_000)
        } else {
            dynamic_margin
        };
        let kms_total_lead = kms_dispatch_budget.saturating_add(kms_apply_guard_ns);
        let mut total = render_risk
            .saturating_add(main_event_loop_wake_guard)
            .saturating_add(kms_total_lead);
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
            p95_worker_pre_submit_ns: p95_worker_pre_submit,
            p95_worker_dispatch_ns: p95_worker_dispatch,
            p95_atomic_ioctl_ns: p95_ioctl,
            main_event_loop_wake_guard_ns: main_event_loop_wake_guard,
            kms_dispatch_budget_ns: kms_dispatch_budget,
            kms_apply_guard_ns,
            kms_total_lead_ns: kms_total_lead,
            p95_target_slip_ns: nearest_rank(&self.target_slip_samples_ns, 95),
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
        assert_eq!(prediction.kms_dispatch_budget_ns, 250_000);
        assert_eq!(prediction.main_event_loop_wake_guard_ns, 1_000_000);
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
        let target = crate::native::presentation_deadline::PresentationTarget {
            sequence: 11,
            physical_claim: crate::native::presentation_deadline::PrimaryRefreshClaim {
                sequence: 11,
                presentation_time: MonotonicTimestampNs::new(100),
                clock_generation: 1,
            },
            ..target
        };
        policy.observe_overlap_for_target(Some(target), 0);
        let target = crate::native::presentation_deadline::PresentationTarget {
            sequence: 12,
            physical_claim: crate::native::presentation_deadline::PrimaryRefreshClaim {
                sequence: 12,
                presentation_time: MonotonicTimestampNs::new(100),
                clock_generation: 1,
            },
            ..target
        };
        policy.observe_overlap_for_target(Some(target), 0);
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
        for sequence in 11..=13 {
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
        assert_eq!(prediction.p95_wake_lateness_ns, 1_115);
        assert_eq!(prediction.p95_atomic_ioctl_ns, 4_115);
        assert_eq!(prediction.kms_dispatch_budget_ns, 5_115);
        assert_eq!(prediction.p95_target_slip_ns, 6_115);
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

        for _ in 0..3 {
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
