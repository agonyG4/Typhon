use oblivion_one::native::kms::{AtomicKmsError, AtomicKmsErrorKind, KmsPolicy};
use oblivion_one::native::presentation_deadline::{
    MonotonicTimestampNs, PresentationDeadlinePlanner, PresentationTarget,
    PresentationTargetAuthority, PresentationTargetReason,
};
use oblivion_one::native::scheduler::NativeOutputPacingMode;
use oblivion_one::{compositor::OwnCompositorServer, native::scheduler::NativeFrameScheduler};
use std::io;
use std::time::Duration;

use crate::native_output::presentation::kms_timing::{KmsPresentationTimingModel, KmsSubmitWindow};

/// Bounded time for the reactor to build and enqueue a cursor-only Atomic
/// transaction before the KMS worker's timed wake. This is intentionally
/// independent of primary scene render prediction.
pub(crate) const CURSOR_PREPARATION_GUARD_NS: u64 = 2_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CursorServiceOpportunity {
    pub(crate) target: PresentationTarget,
    pub(crate) submit_window: KmsSubmitWindow,
    pub(crate) service_deadline_ns: u64,
    pub(crate) ordering_after_primary: Option<PresentationTarget>,
}

/// Plan the next legal physical cursor service opportunity using only the
/// cursor preparation guard and KMS dispatch/apply costs. A reserved primary
/// target remains an ordering lower bound; an advisory visual target is not
/// cursor ownership and is ignored.
pub(crate) fn plan_cursor_service_opportunity(
    planner: &PresentationDeadlinePlanner,
    timing: &KmsPresentationTimingModel,
    now: MonotonicTimestampNs,
    primary_target: Option<PresentationTarget>,
    kms_dispatch_budget_ns: u64,
) -> Option<CursorServiceOpportunity> {
    let ready_cost_ns = CURSOR_PREPARATION_GUARD_NS
        .saturating_add(timing.apply_guard_ns())
        .saturating_add(kms_dispatch_budget_ns);
    let ready_cost = Duration::from_nanos(ready_cost_ns);
    let binding_primary_target =
        primary_target.filter(|target| target.authority() == PresentationTargetAuthority::Reserved);
    let target = match binding_primary_target {
        Some(primary_target) => planner.reactive_target_after(now, ready_cost, primary_target),
        None => planner.reactive_target(now, ready_cost),
    }?;
    let submit_window = timing
        .submit_window(
            target.presentation_time.get(),
            target.submit_not_before().get(),
            kms_dispatch_budget_ns,
        )
        .ok()?;
    let service_deadline_ns = submit_window
        .worker_wake_at_ns()
        .saturating_sub(CURSOR_PREPARATION_GUARD_NS);
    (service_deadline_ns >= now.get()).then_some(CursorServiceOpportunity {
        target,
        submit_window,
        service_deadline_ns,
        ordering_after_primary: binding_primary_target,
    })
}

/// Plan the cursor opportunity from a scene prediction without borrowing the
/// primary scene's render cost. Only the prediction's KMS dispatch estimate
/// is relevant to an independent cursor plane commit.
pub(crate) fn plan_cursor_service_opportunity_with_scene_prediction(
    planner: &PresentationDeadlinePlanner,
    timing: &KmsPresentationTimingModel,
    now: MonotonicTimestampNs,
    primary_target: Option<PresentationTarget>,
    _scene_total_cost_ns: u64,
    kms_dispatch_budget_ns: u64,
) -> Option<CursorServiceOpportunity> {
    plan_cursor_service_opportunity(planner, timing, now, primary_target, kms_dispatch_budget_ns)
}

pub(crate) fn cursor_primary_ordering_target(
    scheduled_target: Option<PresentationTarget>,
    pending_target: Option<PresentationTarget>,
) -> Option<PresentationTarget> {
    [scheduled_target, pending_target]
        .into_iter()
        .flatten()
        .filter(|target| target.authority() == PresentationTargetAuthority::Reserved)
        .max_by_key(|target| target.physical_claim().sequence)
}

pub(super) fn pending_target_for_scanout(
    scanout: &super::super::scanout::NativeScanoutBackend,
) -> io::Result<Option<PresentationTarget>> {
    match scanout {
        super::super::scanout::NativeScanoutBackend::AtomicEglGbm(explicit) => {
            Ok(explicit.swapchain()?.latest_future_primary_target())
        }
        _ => Ok(None),
    }
}

pub(super) fn plan_commit_timing_target(
    planner: &mut PresentationDeadlinePlanner,
    server: &mut OwnCompositorServer,
    _frame_scheduler: &NativeFrameScheduler,
    scheduled: Option<PresentationTarget>,
    now: MonotonicTimestampNs,
    predicted_total_cost: Duration,
) -> Option<PresentationTarget> {
    let candidates = server.commit_timing_planning_candidates();
    if candidates.is_empty() {
        return scheduled;
    }

    let mut planned_targets = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        if !candidate.clock_mapping.is_representable {
            continue;
        }
        let mut candidate_planner = *planner;
        let Some(target) = candidate_planner.plan_not_before(
            now,
            candidate.monotonic_not_before,
            predicted_total_cost,
        ) else {
            continue;
        };
        if server.arm_commit_timing_target(
            candidate,
            target.presentation_time,
            target.render_start_deadline,
            target.sequence,
            target.clock_generation,
        ) {
            planned_targets.push(target);
        }
    }

    let Some(earliest) = planned_targets.into_iter().min_by_key(|target| {
        (
            target.render_start_deadline,
            target.presentation_time,
            target.sequence,
        )
    }) else {
        return scheduled;
    };
    planner.clear_scheduled_target();
    planner
        .plan_not_before(now, earliest.presentation_time, predicted_total_cost)
        .or(Some(earliest))
}

/// Preserve the already-owned target while keeping generic output planning
/// side-effect free with respect to compositor surface publication.
pub(super) fn prepare_presentation_target(
    scheduled: Option<PresentationTarget>,
) -> Option<PresentationTarget> {
    scheduled
}

pub(super) fn reactive_or_commit_timing_target(
    planner: &PresentationDeadlinePlanner,
    scheduled: Option<PresentationTarget>,
    lower_bound: Option<PresentationTarget>,
    now: MonotonicTimestampNs,
    predicted_total_cost: Duration,
) -> Option<PresentationTarget> {
    scheduled
        .filter(|target| target.reason == PresentationTargetReason::CommitTiming)
        .or_else(|| {
            lower_bound
                .and_then(|lower_bound| {
                    planner.reactive_target_after(now, predicted_total_cost, lower_bound)
                })
                .or_else(|| planner.reactive_target(now, predicted_total_cost))
        })
}

#[cfg(test)]
pub(super) fn plan_scheduled_target_for_mode(
    planner: &mut PresentationDeadlinePlanner,
    pacing_mode: NativeOutputPacingMode,
    pending_target: Option<PresentationTarget>,
    now: MonotonicTimestampNs,
    predicted_total_cost: Duration,
    reason: PresentationTargetReason,
) -> Option<PresentationTarget> {
    plan_scheduled_target_for_budget(
        planner,
        pacing_mode == NativeOutputPacingMode::PredictiveTriple,
        pending_target,
        now,
        predicted_total_cost,
        reason,
    )
}

pub(super) fn plan_scheduled_target_for_budget(
    planner: &mut PresentationDeadlinePlanner,
    render_ahead_allowed: bool,
    pending_target: Option<PresentationTarget>,
    now: MonotonicTimestampNs,
    predicted_total_cost: Duration,
    reason: PresentationTargetReason,
) -> Option<PresentationTarget> {
    if !render_ahead_allowed {
        return None;
    }
    planner.plan_render_ahead(pending_target?, now, predicted_total_cost, reason)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn plan_visual_target_for_budget(
    planner: &mut PresentationDeadlinePlanner,
    render_ahead_allowed: bool,
    pending_target: Option<PresentationTarget>,
    now: MonotonicTimestampNs,
    predicted_total_cost: Duration,
    explicit_output: bool,
    visual_work_queued: bool,
    force_validation: bool,
    scheduled: Option<PresentationTarget>,
) -> Option<PresentationTarget> {
    if !explicit_output || !visual_work_queued {
        return scheduled;
    }
    if let (Some(pending), Some(scheduled)) = (pending_target, scheduled)
        && !strictly_later_target(pending, scheduled)
    {
        // The old target is no longer a valid live opportunity claim.  End it
        // before allocating a successor; never mutate an armed target into a
        // later vblank merely because the frontier moved.
        let abandoned = planner.abandon_scheduled_target();
        debug_assert_eq!(abandoned, Some(scheduled));
        return planner.plan_successor_after(pending, now, predicted_total_cost, scheduled.reason);
    }
    if scheduled.is_some() {
        return scheduled;
    }
    if !render_ahead_allowed {
        return None;
    }
    let reason = if force_validation {
        PresentationTargetReason::ForcedValidation
    } else {
        PresentationTargetReason::PredictedPressure
    };
    plan_scheduled_target_for_budget(
        planner,
        render_ahead_allowed,
        pending_target,
        now,
        predicted_total_cost,
        reason,
    )
}

fn strictly_later_target(earlier: PresentationTarget, later: PresentationTarget) -> bool {
    earlier.clock_generation == later.clock_generation
        && later.physical_claim().clock_generation == earlier.physical_claim().clock_generation
        && later.physical_claim().sequence > earlier.physical_claim().sequence
        && later.physical_claim().presentation_time > earlier.physical_claim().presentation_time
}

#[cfg(test)]
pub(super) fn visual_target_deadline_for_mode(
    pacing_mode: NativeOutputPacingMode,
    scheduled_target: Option<PresentationTarget>,
) -> Option<u64> {
    (pacing_mode == NativeOutputPacingMode::PredictiveTriple
        || scheduled_target
            .is_some_and(|target| target.reason == PresentationTargetReason::CommitTiming))
    .then(|| scheduled_target.map(|target| target.render_start_deadline.get()))
    .flatten()
}

pub(super) const fn pacing_mode_for_target(
    target: Option<PresentationTarget>,
) -> NativeOutputPacingMode {
    match target {
        Some(target) => match target.reason {
            PresentationTargetReason::PredictedPressure
            | PresentationTargetReason::ProvenReadinessMiss
            | PresentationTargetReason::ForcedValidation
            | PresentationTargetReason::CommitTiming => NativeOutputPacingMode::PredictiveTriple,
            PresentationTargetReason::ReactiveDouble | PresentationTargetReason::Normal => {
                NativeOutputPacingMode::ReactiveDouble
            }
        },
        None => NativeOutputPacingMode::ReactiveDouble,
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum NativeKmsStartupDecision {
    Atomic,
    Legacy {
        atomic_fallback_reason: Option<AtomicKmsError>,
    },
}

pub(crate) fn decide_native_kms_startup(
    policy: KmsPolicy,
    scanout: super::NativeScanoutKind,
    discovery: Result<(), AtomicKmsError>,
) -> Result<NativeKmsStartupDecision, AtomicKmsError> {
    if scanout == super::NativeScanoutKind::AtomicEglGbmExplicit && policy == KmsPolicy::Legacy {
        return Err(AtomicKmsError::new(
            AtomicKmsErrorKind::Unsupported,
            "explicit Atomic scanout cannot use Legacy KMS",
        ));
    }
    if policy == KmsPolicy::Legacy {
        return Ok(NativeKmsStartupDecision::Legacy {
            atomic_fallback_reason: None,
        });
    }
    match discovery {
        Ok(()) => Ok(NativeKmsStartupDecision::Atomic),
        Err(error) if policy == KmsPolicy::Auto => Ok(NativeKmsStartupDecision::Legacy {
            atomic_fallback_reason: Some(error),
        }),
        Err(error) => Err(error),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeCursorOwnerPlan {
    AtomicHardware,
    LegacyHardware,
    Software,
}

pub(crate) fn decide_native_cursor_owner(
    kms_kind: oblivion_one::native::kms::KmsBackendKind,
    preference: super::NativeCursorPreference,
    hardware_available: bool,
) -> Result<NativeCursorOwnerPlan, &'static str> {
    match (kms_kind, preference, hardware_available) {
        (
            oblivion_one::native::kms::KmsBackendKind::Atomic,
            super::NativeCursorPreference::Software,
            _,
        ) => Ok(NativeCursorOwnerPlan::Software),
        (
            oblivion_one::native::kms::KmsBackendKind::Atomic,
            super::NativeCursorPreference::Hardware,
            true,
        ) => Ok(NativeCursorOwnerPlan::AtomicHardware),
        (
            oblivion_one::native::kms::KmsBackendKind::Atomic,
            super::NativeCursorPreference::Hardware,
            false,
        ) => Err("Atomic hardware cursor requested but no compatible cursor is available"),
        (
            oblivion_one::native::kms::KmsBackendKind::Atomic,
            super::NativeCursorPreference::Auto,
            true,
        ) => Ok(NativeCursorOwnerPlan::AtomicHardware),
        (
            oblivion_one::native::kms::KmsBackendKind::Atomic,
            super::NativeCursorPreference::Auto,
            false,
        ) => Ok(NativeCursorOwnerPlan::Software),
        (
            oblivion_one::native::kms::KmsBackendKind::Legacy,
            super::NativeCursorPreference::Software,
            _,
        ) => Ok(NativeCursorOwnerPlan::Software),
        (
            oblivion_one::native::kms::KmsBackendKind::Legacy,
            super::NativeCursorPreference::Hardware,
            true,
        ) => Ok(NativeCursorOwnerPlan::LegacyHardware),
        (
            oblivion_one::native::kms::KmsBackendKind::Legacy,
            super::NativeCursorPreference::Hardware,
            false,
        ) => Err("Legacy hardware cursor requested but no legacy cursor is available"),
        (
            oblivion_one::native::kms::KmsBackendKind::Legacy,
            super::NativeCursorPreference::Auto,
            true,
        ) => Ok(NativeCursorOwnerPlan::LegacyHardware),
        (
            oblivion_one::native::kms::KmsBackendKind::Legacy,
            super::NativeCursorPreference::Auto,
            false,
        ) => Ok(NativeCursorOwnerPlan::Software),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativePresentationPath {
    DirectPrimary,
    CompositedPrimary,
    PlaneDelta,
    IdleDirect,
    IdleComposited,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativePresentationPlanInput {
    pub(crate) direct_active: bool,
    pub(crate) direct_candidate_changed: bool,
    pub(crate) direct_candidate_eligible: bool,
    pub(crate) primary_visual_work_pending: bool,
    pub(crate) composition_required: bool,
    pub(crate) cursor_changed: bool,
    pub(crate) cursor_hardware_usable: bool,
    pub(crate) cursor_visible: bool,
    pub(crate) atomic_commit_pending: bool,
    /// Whether a cursor-only transaction may use the primary presentation
    /// lane in this scheduling cycle.  A queued cursor state is harmless;
    /// submitting it while a primary producer is active is not.
    pub(crate) plane_delta_allowed: bool,
    pub(crate) render_ahead_requested: bool,
}

pub(crate) fn plan_native_presentation_path(
    input: NativePresentationPlanInput,
) -> NativePresentationPath {
    if input.atomic_commit_pending {
        return if input.direct_active {
            NativePresentationPath::IdleDirect
        } else {
            NativePresentationPath::IdleComposited
        };
    }

    if input.direct_active {
        if input.composition_required || !input.direct_candidate_eligible {
            return NativePresentationPath::CompositedPrimary;
        }
        if input.direct_candidate_changed {
            return NativePresentationPath::DirectPrimary;
        }
        if input.cursor_changed
            && input.plane_delta_allowed
            && (!input.cursor_visible || input.cursor_hardware_usable)
        {
            return NativePresentationPath::PlaneDelta;
        }
        return NativePresentationPath::IdleDirect;
    }

    if input.primary_visual_work_pending || input.composition_required {
        return NativePresentationPath::CompositedPrimary;
    }
    if input.direct_candidate_changed {
        return NativePresentationPath::DirectPrimary;
    }
    if input.cursor_changed
        && input.plane_delta_allowed
        && (!input.cursor_visible || input.cursor_hardware_usable)
    {
        return NativePresentationPath::PlaneDelta;
    }
    let _ = input.render_ahead_requested;
    NativePresentationPath::IdleComposited
}

#[cfg(test)]
mod tests {
    use super::super::frame::{
        cursor_output_disposition_at_deadline, plane_delta_allowed_at_deadline,
    };
    use super::super::{
        NativeCursorOutputArbitration, NativeCursorOutputDisposition, NativeCursorSchedulingPolicy,
    };
    use crate::native_output::presentation::kms_timing::{
        KmsModeTiming, KmsPresentationTimingModel,
    };
    use crate::native_output::{NativeScanoutKind, runtime::NativeCursorPreference};
    use drm_sys::drm_mode_modeinfo;
    use oblivion_one::native::kms::{
        AtomicKmsError, AtomicKmsErrorKind, KmsBackendKind, KmsPolicy,
    };
    use oblivion_one::native::presentation_deadline::{
        MonotonicTimestampNs, PresentationDeadlinePlanner, PresentationTarget,
        PresentationTargetReason,
    };

    use super::*;

    fn discovery_error() -> AtomicKmsError {
        AtomicKmsError::new(
            AtomicKmsErrorKind::MissingProperty,
            "missing Atomic property",
        )
    }

    fn scheduled_target() -> PresentationTarget {
        let now = MonotonicTimestampNs::new(10);
        PresentationTarget {
            sequence: 1,
            presentation_time: now,
            submit_not_before: MonotonicTimestampNs::new(8),
            render_start_deadline: MonotonicTimestampNs::new(6),
            refresh_interval: std::time::Duration::from_nanos(10),
            reason: PresentationTargetReason::ForcedValidation,
            clock_generation: 1,
            estimated: false,
            predicted_unreachable: false,
            physical_claim: oblivion_one::native::presentation_deadline::PrimaryRefreshClaim {
                sequence: 1,
                presentation_time: now,
                clock_generation: 1,
            },
            selection_evidence: Default::default(),
        }
    }

    #[test]
    fn generic_presentation_target_preparation_only_returns_owned_target() {
        let scheduled = Some(scheduled_target());

        assert_eq!(prepare_presentation_target(scheduled), scheduled);
    }

    #[test]
    fn explicit_scanout_atomic_policy_builds_atomic_plan() {
        assert_eq!(
            decide_native_kms_startup(
                KmsPolicy::Atomic,
                NativeScanoutKind::AtomicEglGbmExplicit,
                Ok(()),
            )
            .unwrap(),
            NativeKmsStartupDecision::Atomic
        );
    }

    #[test]
    fn opaque_scanout_atomic_policy_builds_atomic_plan() {
        assert_eq!(
            decide_native_kms_startup(
                KmsPolicy::Atomic,
                NativeScanoutKind::NativeEglGbmOpaqueCompatibility,
                Ok(()),
            )
            .unwrap(),
            NativeKmsStartupDecision::Atomic
        );
    }

    #[test]
    fn gbm_cpu_atomic_policy_builds_atomic_plan() {
        assert_eq!(
            decide_native_kms_startup(
                KmsPolicy::Atomic,
                NativeScanoutKind::GbmCpuWritePageFlip,
                Ok(()),
            )
            .unwrap(),
            NativeKmsStartupDecision::Atomic
        );
    }

    #[test]
    fn dumb_atomic_policy_builds_atomic_plan_when_supported() {
        assert_eq!(
            decide_native_kms_startup(
                KmsPolicy::Atomic,
                NativeScanoutKind::DumbFramebuffer,
                Ok(()),
            )
            .unwrap(),
            NativeKmsStartupDecision::Atomic
        );
    }

    #[test]
    fn auto_atomic_discovery_failure_selects_legacy_plan() {
        let error = discovery_error();
        assert_eq!(
            decide_native_kms_startup(
                KmsPolicy::Auto,
                NativeScanoutKind::GbmCpuWritePageFlip,
                Err(error.clone()),
            )
            .unwrap(),
            NativeKmsStartupDecision::Legacy {
                atomic_fallback_reason: Some(error),
            }
        );
    }

    #[test]
    fn forced_atomic_discovery_failure_fails() {
        let error = discovery_error();
        assert_eq!(
            decide_native_kms_startup(
                KmsPolicy::Atomic,
                NativeScanoutKind::DumbFramebuffer,
                Err(error.clone()),
            )
            .unwrap_err(),
            error
        );
    }

    #[test]
    fn legacy_hardware_cursor_uses_legacy_owner_only() {
        assert_eq!(
            decide_native_kms_startup(
                KmsPolicy::Legacy,
                NativeScanoutKind::DumbFramebuffer,
                Err(discovery_error()),
            )
            .unwrap(),
            NativeKmsStartupDecision::Legacy {
                atomic_fallback_reason: None,
            }
        );
        assert_eq!(
            decide_native_cursor_owner(
                KmsBackendKind::Legacy,
                NativeCursorPreference::Hardware,
                true
            )
            .unwrap(),
            NativeCursorOwnerPlan::LegacyHardware
        );
        assert_ne!(
            decide_native_cursor_owner(
                KmsBackendKind::Legacy,
                NativeCursorPreference::Hardware,
                true
            )
            .unwrap(),
            NativeCursorOwnerPlan::AtomicHardware
        );
    }

    #[test]
    fn atomic_hardware_cursor_missing_fails_startup() {
        assert!(
            decide_native_cursor_owner(
                KmsBackendKind::Atomic,
                NativeCursorPreference::Hardware,
                false,
            )
            .is_err()
        );
    }

    #[test]
    fn atomic_auto_cursor_missing_selects_software() {
        assert_eq!(
            decide_native_cursor_owner(
                KmsBackendKind::Atomic,
                NativeCursorPreference::Auto,
                false,
            )
            .unwrap(),
            NativeCursorOwnerPlan::Software
        );
    }

    #[test]
    fn supported_scanout_kms_matrix_has_one_startup_plan_per_row() {
        let rows = [
            (
                NativeScanoutKind::AtomicEglGbmExplicit,
                KmsPolicy::Atomic,
                Ok(()),
                NativeKmsStartupDecision::Atomic,
            ),
            (
                NativeScanoutKind::NativeEglGbmOpaqueCompatibility,
                KmsPolicy::Atomic,
                Ok(()),
                NativeKmsStartupDecision::Atomic,
            ),
            (
                NativeScanoutKind::GbmCpuWritePageFlip,
                KmsPolicy::Atomic,
                Ok(()),
                NativeKmsStartupDecision::Atomic,
            ),
            (
                NativeScanoutKind::DumbFramebuffer,
                KmsPolicy::Atomic,
                Ok(()),
                NativeKmsStartupDecision::Atomic,
            ),
            (
                NativeScanoutKind::NativeEglGbmOpaqueCompatibility,
                KmsPolicy::Legacy,
                Err(discovery_error()),
                NativeKmsStartupDecision::Legacy {
                    atomic_fallback_reason: None,
                },
            ),
            (
                NativeScanoutKind::GbmCpuWritePageFlip,
                KmsPolicy::Legacy,
                Err(discovery_error()),
                NativeKmsStartupDecision::Legacy {
                    atomic_fallback_reason: None,
                },
            ),
            (
                NativeScanoutKind::DumbFramebuffer,
                KmsPolicy::Legacy,
                Err(discovery_error()),
                NativeKmsStartupDecision::Legacy {
                    atomic_fallback_reason: None,
                },
            ),
        ];

        for (scanout, policy, discovery, expected) in rows {
            assert_eq!(
                decide_native_kms_startup(policy, scanout, discovery).unwrap(),
                expected,
                "startup plan mismatch for {scanout:?} + {policy:?}"
            );
        }
    }

    const fn direct_input() -> NativePresentationPlanInput {
        NativePresentationPlanInput {
            direct_active: true,
            direct_candidate_changed: false,
            direct_candidate_eligible: true,
            primary_visual_work_pending: false,
            composition_required: false,
            cursor_changed: false,
            cursor_hardware_usable: true,
            cursor_visible: true,
            atomic_commit_pending: false,
            plane_delta_allowed: true,
            render_ahead_requested: false,
        }
    }

    #[test]
    fn active_direct_unchanged_candidate_and_cursor_is_idle_direct() {
        assert_eq!(
            plan_native_presentation_path(direct_input()),
            NativePresentationPath::IdleDirect
        );
    }

    #[test]
    fn active_direct_cursor_motion_uses_plane_delta() {
        assert_eq!(
            plan_native_presentation_path(NativePresentationPlanInput {
                cursor_changed: true,
                ..direct_input()
            }),
            NativePresentationPath::PlaneDelta
        );
    }

    #[test]
    fn active_primary_can_defer_cursor_motion_without_reordering_primary_work() {
        assert_eq!(
            plan_native_presentation_path(NativePresentationPlanInput {
                cursor_changed: true,
                plane_delta_allowed: false,
                ..direct_input()
            }),
            NativePresentationPath::IdleDirect
        );
    }

    #[test]
    fn event_order_primary_response_preempts_queued_cursor_plan() {
        let mut arbitration = NativeCursorOutputArbitration::default();
        arbitration.request(1, 0, 6_060_606);

        // The pointer response is observed first.  The client buffer arrives
        // later in the same output opportunity and consumes the cursor state.
        assert_eq!(
            arbitration.disposition(100_000, false, true),
            NativeCursorOutputDisposition::DeferForPrimary
        );
        assert_eq!(
            arbitration.disposition(3_000_000, true, true),
            NativeCursorOutputDisposition::PiggybackPrimary
        );
        arbitration.consume(1);
        assert!(!arbitration.pending());
    }

    #[test]
    fn piggyback_policy_preserves_primary_first_behavior_under_continuous_motion() {
        let mut arbitration = NativeCursorOutputArbitration::default();

        for epoch in 1..=1_024 {
            arbitration.request_hardware(epoch, epoch, epoch.saturating_add(1));
            assert!(!plane_delta_allowed_at_deadline(
                &mut arbitration,
                NativeCursorSchedulingPolicy::Piggyback,
                epoch.saturating_add(1),
                true,
                true,
                true,
            ));
            assert_eq!(
                arbitration.disposition(epoch.saturating_add(1), true, true),
                NativeCursorOutputDisposition::PiggybackPrimary
            );
            arbitration.consume(epoch);
        }

        assert_eq!(arbitration.cursor_state_piggybacked(), 1_024);
        assert_eq!(arbitration.plane_delta_submissions(), 0);
        assert_eq!(arbitration.plane_delta_deferred_for_primary(), 0);
        assert!(!arbitration.pending());
    }

    #[test]
    fn cursor_target_and_service_time_ignore_five_hundred_ms_scene_prediction() {
        const REFRESH_NS: u64 = 6_060_606;
        const KMS_DISPATCH_BUDGET_NS: u64 = 500_000;
        let now = MonotonicTimestampNs::new(1_000_000_000);
        let mut planner = PresentationDeadlinePlanner::new(Duration::from_nanos(REFRESH_NS));
        planner.note_presented(now);
        let timing = KmsPresentationTimingModel::new(
            KmsModeTiming::from_mode(&drm_mode_modeinfo::default(), REFRESH_NS),
            7,
        );
        #[derive(Clone, Copy)]
        struct Prediction {
            total_cost_ns: u64,
            kms_dispatch_budget_ns: u64,
        }
        let scene_predictions = [
            Prediction {
                total_cost_ns: 1_000_000,
                kms_dispatch_budget_ns: KMS_DISPATCH_BUDGET_NS,
            },
            Prediction {
                total_cost_ns: 500_000_000,
                kms_dispatch_budget_ns: KMS_DISPATCH_BUDGET_NS,
            },
        ];

        let opportunities = scene_predictions.map(|prediction| {
            assert!(prediction.total_cost_ns > 0);
            plan_cursor_service_opportunity_with_scene_prediction(
                &planner,
                &timing,
                now,
                None,
                prediction.total_cost_ns,
                prediction.kms_dispatch_budget_ns,
            )
            .expect("cursor has a reachable KMS opportunity")
        });

        assert_eq!(
            opportunities[0].target.identity(),
            opportunities[1].target.identity()
        );
        assert_eq!(
            opportunities[0].target.presentation_time,
            opportunities[1].target.presentation_time
        );
        assert_eq!(
            opportunities[0].service_deadline_ns,
            opportunities[1].service_deadline_ns
        );
    }

    #[test]
    fn independent_cursor_target_ignores_advisory_target_and_follows_binding_target() {
        const REFRESH_NS: u64 = 10_000_000;
        let now = MonotonicTimestampNs::new(1_000_000_000);
        let mut planner = PresentationDeadlinePlanner::new(Duration::from_nanos(REFRESH_NS));
        planner.note_presented(now);
        let timing = KmsPresentationTimingModel::new(
            KmsModeTiming::from_mode(&drm_mode_modeinfo::default(), REFRESH_NS),
            7,
        );
        let advisory = planner
            .reactive_target(now, Duration::from_millis(500))
            .expect("advisory visual target");
        let binding = scheduled_target();
        let independent =
            plan_cursor_service_opportunity(&planner, &timing, now, None, 500_000).unwrap();
        let with_advisory =
            plan_cursor_service_opportunity(&planner, &timing, now, Some(advisory), 500_000)
                .unwrap();
        let after_binding =
            plan_cursor_service_opportunity(&planner, &timing, now, Some(binding), 500_000)
                .unwrap();

        assert_eq!(
            independent.target.identity(),
            with_advisory.target.identity()
        );
        assert!(after_binding.target.sequence > binding.sequence);
    }

    #[test]
    fn newly_reserved_primary_invalidates_pending_cursor_target_without_sliding_age() {
        const REFRESH_NS: u64 = 10_000_000;
        let now = MonotonicTimestampNs::new(1_000_000_000);
        let mut planner = PresentationDeadlinePlanner::new(Duration::from_nanos(REFRESH_NS));
        planner.note_presented(now);
        let timing = KmsPresentationTimingModel::new(
            KmsModeTiming::from_mode(&drm_mode_modeinfo::default(), REFRESH_NS),
            7,
        );
        let initial = plan_cursor_service_opportunity(&planner, &timing, now, None, 500_000)
            .expect("initial cursor service opportunity");
        let mut arbitration = NativeCursorOutputArbitration::default();
        arbitration.request_hardware_for_opportunity(1, now.get(), initial);

        // A primary reservation may arrive after cursor debt was first
        // observed. Give it the cursor's current physical claim so that the
        // existing target is no longer legal for an independent cursor.
        let mut binding = scheduled_target();
        binding.sequence = initial.target.sequence;
        binding.presentation_time = initial.target.presentation_time;
        binding.clock_generation = initial.target.clock_generation;
        binding.physical_claim = initial.target.physical_claim();
        assert_eq!(binding.authority(), PresentationTargetAuthority::Reserved);
        let after_binding = plan_cursor_service_opportunity(
            &planner,
            &timing,
            MonotonicTimestampNs::new(now.get().saturating_add(100_000)),
            Some(binding),
            500_000,
        )
        .expect("cursor opportunity after the new primary reservation");
        assert!(after_binding.target.physical_claim().sequence > binding.physical_claim().sequence);

        arbitration.request_hardware_for_opportunity(
            2,
            now.get().saturating_add(100_000),
            after_binding,
        );

        assert_eq!(arbitration.pending_since_ns(), Some(now.get()));
        assert_eq!(arbitration.deadline_ns(), Some(initial.service_deadline_ns));
        assert_eq!(arbitration.service_opportunity(), Some(after_binding));
    }

    #[test]
    fn cursor_service_windows_are_future_and_kms_timed_at_supported_refresh_rates() {
        let now = MonotonicTimestampNs::new(2_000_000_000);

        for hz in [60u64, 120, 144, 165, 240] {
            let refresh_ns = 1_000_000_000 / hz;
            let mut planner = PresentationDeadlinePlanner::new(Duration::from_nanos(refresh_ns));
            planner.note_presented(now);
            let timing = KmsPresentationTimingModel::new(
                KmsModeTiming::from_mode(&drm_mode_modeinfo::default(), refresh_ns),
                9,
            );
            let opportunity =
                plan_cursor_service_opportunity(&planner, &timing, now, None, 500_000).unwrap();

            assert!(opportunity.target.presentation_time.get() > now.get());
            assert!(opportunity.service_deadline_ns >= now.get());
            assert_eq!(
                opportunity.submit_window.worker_wake_at_ns(),
                opportunity.service_deadline_ns + CURSOR_PREPARATION_GUARD_NS
            );
        }
    }

    #[test]
    fn cursor_motion_keeps_one_service_deadline_and_replans_missed_targets_without_resetting_age() {
        const REFRESH_NS: u64 = 10_000_000;
        let now = MonotonicTimestampNs::new(1_000_000_000);
        let mut planner = PresentationDeadlinePlanner::new(Duration::from_nanos(REFRESH_NS));
        planner.note_presented(now);
        let timing = KmsPresentationTimingModel::new(
            KmsModeTiming::from_mode(&drm_mode_modeinfo::default(), REFRESH_NS),
            7,
        );
        let first = plan_cursor_service_opportunity(&planner, &timing, now, None, 500_000)
            .expect("initial cursor service opportunity");
        let mut arbitration = NativeCursorOutputArbitration::default();
        arbitration.request_hardware_for_opportunity(1, now.get(), first);
        let original_deadline = first.service_deadline_ns;

        let same_physical_target_later_deadline = CursorServiceOpportunity {
            service_deadline_ns: original_deadline.saturating_add(100_000),
            ..first
        };
        arbitration.request_hardware_for_opportunity(
            2,
            now.get().saturating_add(50_000),
            same_physical_target_later_deadline,
        );
        assert_eq!(arbitration.deadline_ns(), Some(original_deadline));
        assert_eq!(arbitration.pending_since_ns(), Some(now.get()));
        assert_eq!(arbitration.service_opportunity(), Some(first));

        let missed_at = first
            .submit_window
            .commit_complete_deadline_ns()
            .saturating_add(1);
        let next = plan_cursor_service_opportunity(
            &planner,
            &timing,
            MonotonicTimestampNs::new(missed_at),
            None,
            500_000,
        )
        .expect("later physical cursor target");
        assert!(next.target.sequence > first.target.sequence);
        arbitration.request_hardware_for_opportunity(3, missed_at, next);
        assert_eq!(arbitration.deadline_ns(), Some(original_deadline));
        assert_eq!(arbitration.pending_since_ns(), Some(now.get()));
        assert_eq!(
            arbitration
                .service_opportunity()
                .map(|opportunity| opportunity.target.sequence),
            Some(next.target.sequence)
        );
        assert!(!arbitration.due(missed_at));
        assert_eq!(
            arbitration.wake_deadline_ns(missed_at),
            Some(next.service_deadline_ns)
        );
    }

    #[test]
    fn auto_services_latest_cursor_after_bounded_wait_with_unready_primary_work() {
        const SERVICE_DEADLINE_NS: u64 = 10_000;
        let mut arbitration = NativeCursorOutputArbitration::default();
        arbitration.request_hardware(1, 100, SERVICE_DEADLINE_NS);

        for epoch in 2..=1_024 {
            arbitration.request_hardware(epoch, epoch * 100, SERVICE_DEADLINE_NS + epoch);
        }

        assert_eq!(arbitration.desired_epoch(), 1_024);
        assert_eq!(
            cursor_output_disposition_at_deadline(
                &mut arbitration,
                NativeCursorSchedulingPolicy::Auto,
                SERVICE_DEADLINE_NS - 1,
                true,
                true,
            ),
            NativeCursorOutputDisposition::DeferForPrimary
        );
        assert_eq!(
            cursor_output_disposition_at_deadline(
                &mut arbitration,
                NativeCursorSchedulingPolicy::Auto,
                SERVICE_DEADLINE_NS,
                true,
                true,
            ),
            NativeCursorOutputDisposition::SubmitPlaneDelta
        );
        assert_eq!(arbitration.plane_delta_plans(), 1);
    }

    #[test]
    fn auto_165_hz_stress_coalesces_1000_hz_motion_and_preserves_primary_opportunities() {
        use super::super::frame::{
            NativeCursorPrimaryState, cursor_output_disposition_for_primary_state,
        };

        const SIMULATION_NS: u64 = 1_000_000_000;
        const INPUT_INTERVAL_NS: u64 = 1_000_000;
        const DISPLAY_HZ: u64 = 165;
        const REFRESH_NS: u64 = SIMULATION_NS / DISPLAY_HZ;
        // Model the service decision before the physical target, with room
        // for cursor transaction construction and ordinary KMS dispatch.
        const DECISION_LEAD_NS: u64 = 2_500_000;

        let mut arbitration = NativeCursorOutputArbitration::default();
        let mut input_epoch = 0_u64;
        let mut max_cursor_age_ns = 0_u64;
        let mut max_unowned_cursor_age_ns = 0_u64;
        let mut independent_services = 0_u64;
        let mut ready_primary_piggybacks = 0_u64;
        let mut owned_primary_deferrals = 0_u64;
        let mut baseline_primary_misses = 0_u64;
        let mut auto_primary_misses = 0_u64;
        let mut max_worker_queue_depth = 0_usize;

        for opportunity in 0..DISPLAY_HZ {
            let service_deadline_ns = (opportunity + 1)
                .saturating_mul(REFRESH_NS)
                .saturating_sub(DECISION_LEAD_NS);
            let mut updates_for_opportunity = 0_u64;
            while input_epoch < SIMULATION_NS / INPUT_INTERVAL_NS {
                let next_input_at_ns = (input_epoch + 1) * INPUT_INTERVAL_NS;
                if next_input_at_ns > service_deadline_ns {
                    break;
                }
                input_epoch += 1;
                updates_for_opportunity += 1;
                arbitration.request_hardware(input_epoch, next_input_at_ns, service_deadline_ns);
            }
            if updates_for_opportunity == 0 {
                continue;
            }

            let pending_since_ns = arbitration.pending_since_ns().unwrap();
            let age_at_decision_ns = service_deadline_ns.saturating_sub(pending_since_ns);
            max_cursor_age_ns = max_cursor_age_ns.max(age_at_decision_ns);
            let latest_epoch = input_epoch;
            assert_eq!(arbitration.desired_epoch(), latest_epoch);

            // Four repeating primary states model frames ready before the
            // cursor window, unready scene work without a reserved target,
            // a late frame with a binding target, and a ready carrier after
            // the prior reservation has completed.
            let primary_ready_by_deadline = matches!(opportunity % 4, 0 | 3);
            let primary_target_owned = opportunity % 4 == 2;
            let disposition = cursor_output_disposition_for_primary_state(
                &mut arbitration,
                NativeCursorSchedulingPolicy::Auto,
                service_deadline_ns,
                NativeCursorPrimaryState {
                    primary_scene_work_pending: true,
                    ready_primary_carrier_attachable: primary_ready_by_deadline,
                    primary_target_owned,
                    independent_kms_admission: true,
                    ..Default::default()
                },
                true,
            );

            if primary_ready_by_deadline {
                assert_eq!(disposition, NativeCursorOutputDisposition::PiggybackPrimary);
                ready_primary_piggybacks += 1;
            } else if primary_target_owned {
                assert_eq!(disposition, NativeCursorOutputDisposition::DeferForPrimary);
                owned_primary_deferrals += 1;

                // Recreate the actual target planning for this case. The
                // primary becomes ready after the cursor decision but before
                // its reserved presentation. Auto's independent cursor
                // target must be strictly after the primary target.
                let model_now = MonotonicTimestampNs::new(
                    2_000_000_000_u64.saturating_add(opportunity.saturating_mul(REFRESH_NS)),
                );
                let mut target_planner =
                    PresentationDeadlinePlanner::new(Duration::from_nanos(REFRESH_NS));
                target_planner.note_presented(model_now);
                let timing = KmsPresentationTimingModel::new(
                    KmsModeTiming::from_mode(&drm_mode_modeinfo::default(), REFRESH_NS),
                    7,
                );
                let current_opportunity = plan_cursor_service_opportunity(
                    &target_planner,
                    &timing,
                    model_now,
                    None,
                    500_000,
                )
                .expect("model cursor target before a primary reservation");
                let mut primary_target = current_opportunity.target;
                primary_target.reason = PresentationTargetReason::Normal;
                assert_eq!(
                    primary_target.authority(),
                    PresentationTargetAuthority::Reserved
                );
                let ordered_cursor_opportunity = plan_cursor_service_opportunity(
                    &target_planner,
                    &timing,
                    model_now,
                    Some(primary_target),
                    500_000,
                )
                .expect("model cursor target after the binding primary");
                assert!(
                    ordered_cursor_opportunity.target.physical_claim().sequence
                        > primary_target.physical_claim().sequence
                );
                let primary_ready_at = primary_target
                    .presentation_time
                    .get()
                    .saturating_sub(100_000);
                assert!(primary_ready_at > current_opportunity.service_deadline_ns);
                assert!(primary_ready_at < primary_target.presentation_time.get());
                let baseline_miss = primary_ready_at > primary_target.presentation_time.get();
                let cursor_would_steal_target =
                    ordered_cursor_opportunity.target.physical_claim().sequence
                        <= primary_target.physical_claim().sequence;
                let auto_miss = baseline_miss
                    || (cursor_would_steal_target
                        && primary_ready_at <= primary_target.presentation_time.get());
                baseline_primary_misses += u64::from(baseline_miss);
                auto_primary_misses += u64::from(auto_miss);
            } else {
                assert_eq!(disposition, NativeCursorOutputDisposition::SubmitPlaneDelta);
                independent_services += 1;
                max_unowned_cursor_age_ns = max_unowned_cursor_age_ns.max(age_at_decision_ns);
            }

            // A primary sidecar or independent cursor uses the existing one
            // job slot. An owned but not yet attachable primary holds its
            // physical target and defers cursor admission.
            let worker_queue_depth = usize::from(
                disposition == NativeCursorOutputDisposition::PiggybackPrimary
                    || disposition == NativeCursorOutputDisposition::SubmitPlaneDelta,
            );
            max_worker_queue_depth = max_worker_queue_depth.max(worker_queue_depth);
            assert!(worker_queue_depth <= 1);
            if matches!(
                disposition,
                NativeCursorOutputDisposition::PiggybackPrimary
                    | NativeCursorOutputDisposition::SubmitPlaneDelta
            ) {
                arbitration.consume(latest_epoch);
            }
        }

        // The final few input samples arrive after the last in-second service
        // decision. They remain pending for one later opportunity and still
        // coalesce to the latest desired epoch.
        let next_service_deadline_ns = (DISPLAY_HZ + 1)
            .saturating_mul(REFRESH_NS)
            .saturating_sub(DECISION_LEAD_NS);
        while input_epoch < SIMULATION_NS / INPUT_INTERVAL_NS {
            input_epoch += 1;
            arbitration.request_hardware(
                input_epoch,
                input_epoch * INPUT_INTERVAL_NS,
                next_service_deadline_ns,
            );
        }

        assert_eq!(input_epoch, 1_000);
        assert!(independent_services <= DISPLAY_HZ);
        assert_eq!(ready_primary_piggybacks, 83);
        assert!(independent_services > 0);
        assert_eq!(independent_services, 41);
        assert_eq!(owned_primary_deferrals, 41);
        assert_eq!(
            independent_services + ready_primary_piggybacks + owned_primary_deferrals,
            DISPLAY_HZ
        );
        assert_eq!(max_worker_queue_depth, 1);
        assert_eq!(auto_primary_misses, baseline_primary_misses);
        assert_eq!(baseline_primary_misses, 0);
        assert!(max_cursor_age_ns <= REFRESH_NS.saturating_mul(2));
        assert!(max_unowned_cursor_age_ns <= REFRESH_NS);
        assert!(arbitration.pending());
        assert_eq!(arbitration.desired_epoch(), 1_000);
        assert_eq!(arbitration.pending_since_ns(), Some(998_000_000));
    }

    #[test]
    fn idle_output_still_allows_one_coalesced_plane_delta_update() {
        assert_eq!(
            plan_native_presentation_path(NativePresentationPlanInput {
                direct_active: true,
                direct_candidate_changed: false,
                direct_candidate_eligible: true,
                primary_visual_work_pending: false,
                composition_required: false,
                cursor_changed: true,
                cursor_hardware_usable: true,
                cursor_visible: true,
                atomic_commit_pending: false,
                plane_delta_allowed: true,
                render_ahead_requested: false,
            }),
            NativePresentationPath::PlaneDelta
        );
    }

    #[test]
    fn active_direct_new_candidate_uses_direct_primary() {
        assert_eq!(
            plan_native_presentation_path(NativePresentationPlanInput {
                direct_candidate_changed: true,
                ..direct_input()
            }),
            NativePresentationPath::DirectPrimary
        );
    }

    #[test]
    fn predictive_render_ahead_does_not_override_direct_steady_state() {
        assert_eq!(
            plan_native_presentation_path(NativePresentationPlanInput {
                render_ahead_requested: true,
                ..direct_input()
            }),
            NativePresentationPath::IdleDirect
        );
    }

    #[test]
    fn inactive_render_ahead_without_visual_work_stays_idle_composited() {
        assert_eq!(
            plan_native_presentation_path(NativePresentationPlanInput {
                direct_active: false,
                render_ahead_requested: true,
                ..direct_input()
            }),
            NativePresentationPath::IdleComposited
        );
    }

    #[test]
    fn popup_causes_composited_primary() {
        assert_eq!(
            plan_native_presentation_path(NativePresentationPlanInput {
                composition_required: true,
                direct_candidate_eligible: false,
                ..direct_input()
            }),
            NativePresentationPath::CompositedPrimary
        );
    }

    #[test]
    fn closing_popup_allows_direct_reentry() {
        assert_eq!(
            plan_native_presentation_path(NativePresentationPlanInput {
                direct_active: false,
                direct_candidate_changed: true,
                ..direct_input()
            }),
            NativePresentationPath::DirectPrimary
        );
    }

    #[test]
    fn software_cursor_requires_composition() {
        assert_eq!(
            plan_native_presentation_path(NativePresentationPlanInput {
                cursor_changed: true,
                cursor_hardware_usable: false,
                composition_required: true,
                direct_candidate_eligible: false,
                ..direct_input()
            }),
            NativePresentationPath::CompositedPrimary
        );
    }

    #[test]
    fn pending_primary_coalesces_cursor_without_second_commit() {
        assert_eq!(
            plan_native_presentation_path(NativePresentationPlanInput {
                cursor_changed: true,
                atomic_commit_pending: true,
                ..direct_input()
            }),
            NativePresentationPath::IdleDirect
        );
    }

    #[test]
    fn composited_output_can_move_cursor_without_replacing_primary() {
        assert_eq!(
            plan_native_presentation_path(NativePresentationPlanInput {
                direct_active: false,
                direct_candidate_eligible: false,
                cursor_changed: true,
                composition_required: false,
                ..direct_input()
            }),
            NativePresentationPath::PlaneDelta
        );
    }

    #[test]
    fn plane_delta_is_selected_when_primary_scene_is_unchanged() {
        assert_eq!(
            plan_native_presentation_path(NativePresentationPlanInput {
                direct_active: false,
                direct_candidate_eligible: true,
                cursor_changed: true,
                ..direct_input()
            }),
            NativePresentationPath::PlaneDelta
        );
    }

    #[test]
    fn active_direct_popup_still_forces_composition() {
        assert_eq!(
            plan_native_presentation_path(NativePresentationPlanInput {
                composition_required: true,
                direct_candidate_eligible: false,
                cursor_changed: true,
                ..direct_input()
            }),
            NativePresentationPath::CompositedPrimary
        );
    }

    #[test]
    fn composited_animation_and_cursor_motion_selects_composited_primary() {
        assert_eq!(
            plan_native_presentation_path(NativePresentationPlanInput {
                direct_active: false,
                direct_candidate_eligible: false,
                cursor_changed: true,
                primary_visual_work_pending: true,
                composition_required: false,
                ..direct_input()
            }),
            NativePresentationPath::CompositedPrimary
        );
    }

    #[test]
    fn popup_and_cursor_motion_selects_composited_primary() {
        assert_eq!(
            plan_native_presentation_path(NativePresentationPlanInput {
                direct_active: false,
                direct_candidate_eligible: false,
                cursor_changed: true,
                primary_visual_work_pending: true,
                composition_required: false,
                ..direct_input()
            }),
            NativePresentationPath::CompositedPrimary
        );
    }

    #[test]
    fn software_cursor_redraw_and_pointer_motion_selects_composited_primary() {
        assert_eq!(
            plan_native_presentation_path(NativePresentationPlanInput {
                direct_active: false,
                direct_candidate_eligible: false,
                cursor_changed: true,
                cursor_hardware_usable: false,
                composition_required: true,
                ..direct_input()
            }),
            NativePresentationPath::CompositedPrimary
        );
    }

    #[test]
    fn new_surface_commit_and_cursor_motion_selects_composited_primary() {
        assert_eq!(
            plan_native_presentation_path(NativePresentationPlanInput {
                direct_active: false,
                direct_candidate_eligible: false,
                cursor_changed: true,
                composition_required: true,
                ..direct_input()
            }),
            NativePresentationPath::CompositedPrimary
        );
    }

    #[test]
    fn hidden_pointer_allows_direct_scanout_without_cursor_plane() {
        assert_eq!(
            plan_native_presentation_path(NativePresentationPlanInput {
                direct_active: false,
                direct_candidate_changed: true,
                cursor_hardware_usable: false,
                cursor_visible: false,
                ..direct_input()
            }),
            NativePresentationPath::DirectPrimary
        );
    }

    #[test]
    fn hidden_software_cursor_mode_allows_direct_scanout() {
        assert_eq!(
            plan_native_presentation_path(NativePresentationPlanInput {
                direct_active: false,
                direct_candidate_changed: true,
                cursor_hardware_usable: false,
                cursor_visible: false,
                ..direct_input()
            }),
            NativePresentationPath::DirectPrimary
        );
    }

    #[test]
    fn visible_software_cursor_blocks_direct_scanout() {
        assert_eq!(
            plan_native_presentation_path(NativePresentationPlanInput {
                direct_active: false,
                direct_candidate_changed: true,
                cursor_hardware_usable: false,
                cursor_visible: true,
                composition_required: true,
                ..direct_input()
            }),
            NativePresentationPath::CompositedPrimary
        );
    }

    #[test]
    fn visible_unsupported_client_cursor_blocks_direct_scanout() {
        assert_eq!(
            plan_native_presentation_path(NativePresentationPlanInput {
                direct_active: false,
                direct_candidate_changed: true,
                cursor_hardware_usable: false,
                cursor_visible: true,
                composition_required: true,
                ..direct_input()
            }),
            NativePresentationPath::CompositedPrimary
        );
    }

    #[test]
    fn visible_usable_atomic_cursor_allows_direct_scanout() {
        assert_eq!(
            plan_native_presentation_path(NativePresentationPlanInput {
                direct_active: false,
                direct_candidate_changed: true,
                cursor_hardware_usable: true,
                cursor_visible: true,
                ..direct_input()
            }),
            NativePresentationPath::DirectPrimary
        );
    }

    #[test]
    fn legacy_cursor_does_not_satisfy_atomic_direct_compatibility() {
        assert_eq!(
            plan_native_presentation_path(NativePresentationPlanInput {
                direct_active: false,
                direct_candidate_changed: true,
                cursor_hardware_usable: false,
                cursor_visible: true,
                composition_required: true,
                ..direct_input()
            }),
            NativePresentationPath::CompositedPrimary
        );
    }
}
