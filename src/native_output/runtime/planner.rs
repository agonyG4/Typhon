use oblivion_one::native::kms::{AtomicKmsError, AtomicKmsErrorKind, KmsPolicy};
use oblivion_one::native::presentation_deadline::{
    MonotonicTimestampNs, PresentationDeadlinePlanner, PresentationTarget,
    PresentationTargetAuthority, PresentationTargetReason,
};
use oblivion_one::native::scheduler::NativeOutputPacingMode;
use oblivion_one::{compositor::OwnCompositorServer, native::scheduler::NativeFrameScheduler};
use std::io;
use std::time::Duration;

use crate::native_output::kms_worker::KmsCommitWorkerTransport;
use crate::native_output::presentation::{
    cursor_timing::CursorTimingDomain, kms_timing::KmsSubmitWindow,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CursorServiceOpportunity {
    pub(crate) target: PresentationTarget,
    pub(crate) submit_window: KmsSubmitWindow,
    pub(crate) service_deadline_ns: u64,
    pub(crate) ordering_after_primary: Option<PresentationTarget>,
    pub(crate) prepare_budget_ns: u64,
    pub(crate) dispatch_budget_ns: u64,
    pub(crate) apply_guard_ns: u64,
    pub(crate) total_lead_ns: u64,
}

/// Plan the earliest legal physical cursor opportunity. Timing budgets define
/// the service deadline and worker wake, while exact physical ownership alone
/// can move the target to a later refresh. This is the late-latch policy.
pub(crate) fn plan_cursor_service_opportunity(
    planner: &PresentationDeadlinePlanner,
    timing: &CursorTimingDomain,
    now: MonotonicTimestampNs,
    primary_target: Option<PresentationTarget>,
    kms_dispatch_budget_ns: u64,
) -> Option<CursorServiceOpportunity> {
    plan_cursor_service_opportunity_with_current(
        planner,
        timing,
        now,
        primary_target,
        kms_dispatch_budget_ns,
        None,
        KmsCommitWorkerTransport::Worker,
    )
}

/// Replan a pending cursor epoch using its prior physical target as an
/// ordering bound only after that target's KMS completion window has closed.
/// A due preparation/dispatch service deadline alone keeps the same target
/// eligible for late-latched service.
pub(crate) fn plan_cursor_service_opportunity_with_current(
    planner: &PresentationDeadlinePlanner,
    timing: &CursorTimingDomain,
    now: MonotonicTimestampNs,
    primary_target: Option<PresentationTarget>,
    kms_dispatch_budget_ns: u64,
    current_opportunity: Option<CursorServiceOpportunity>,
    transport: KmsCommitWorkerTransport,
) -> Option<CursorServiceOpportunity> {
    let prepare_budget_ns = timing.preparation_budget_ns();
    let apply_guard_ns = timing.apply_guard_ns(now.get());
    let total_lead_ns = prepare_budget_ns
        .saturating_add(kms_dispatch_budget_ns)
        .saturating_add(apply_guard_ns);
    let binding_primary_target =
        primary_target.filter(|target| target.authority() == PresentationTargetAuthority::Reserved);
    let expired_cursor_target = current_opportunity
        .filter(|opportunity| {
            opportunity.submit_window.mode_key() == Some(timing.mode_key())
                && (opportunity.target.presentation_time.get() <= now.get()
                    || (transport == KmsCommitWorkerTransport::Synchronous
                        && opportunity.submit_window.commit_complete_deadline_ns() <= now.get()))
        })
        .map(|opportunity| PresentationTarget {
            reason: PresentationTargetReason::Normal,
            selection_evidence:
                oblivion_one::native::presentation_deadline::TargetSelectionEvidence {
                    earliest_feasible_sequence: opportunity.target.sequence,
                    binding: true,
                },
            ..opportunity.target
        });
    let ordering_target = match (binding_primary_target, expired_cursor_target) {
        (Some(primary), Some(cursor)) => Some(
            if primary.physical_claim().sequence > cursor.physical_claim().sequence {
                primary
            } else {
                cursor
            },
        ),
        (Some(primary), None) => Some(primary),
        (None, Some(cursor)) => Some(cursor),
        (None, None) => None,
    };
    // Preparation and dispatch budgets affect only the service deadline and
    // never make worker transport skip a refresh. Synchronous transport keeps
    // its physical-admission gate and selects only a target with apply lead.
    let target_selection_lead = match transport {
        KmsCommitWorkerTransport::Worker => Duration::from_nanos(1),
        KmsCommitWorkerTransport::Synchronous => Duration::from_nanos(apply_guard_ns),
    };
    let target = match ordering_target {
        Some(lower_bound) => planner.reactive_target_after(now, target_selection_lead, lower_bound),
        None => planner.reactive_target(now, target_selection_lead),
    }?;
    let submit_window =
        cursor_submit_window(target, timing, kms_dispatch_budget_ns, apply_guard_ns);
    let service_deadline_ns = target.presentation_time.get().saturating_sub(total_lead_ns);
    Some(CursorServiceOpportunity {
        target,
        submit_window,
        service_deadline_ns,
        ordering_after_primary: binding_primary_target,
        prepare_budget_ns,
        dispatch_budget_ns: kms_dispatch_budget_ns,
        apply_guard_ns,
        total_lead_ns,
    })
}

fn cursor_submit_window(
    target: PresentationTarget,
    timing: &CursorTimingDomain,
    dispatch_budget_ns: u64,
    apply_guard_ns: u64,
) -> KmsSubmitWindow {
    KmsSubmitWindow::late_latch(
        target.presentation_time.get(),
        target.submit_not_before().get(),
        dispatch_budget_ns,
        apply_guard_ns,
    )
    .with_mode_key(timing.mode_key())
}

/// Plan the cursor opportunity from a scene prediction without borrowing the
/// primary scene's render cost. Only the prediction's KMS dispatch estimate
/// is relevant to an independent cursor plane commit.
pub(crate) fn plan_cursor_service_opportunity_with_scene_prediction(
    planner: &PresentationDeadlinePlanner,
    timing: &CursorTimingDomain,
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

pub(super) fn pending_fixed_target_for_scanout(
    scanout: &super::super::scanout::NativeScanoutBackend,
) -> io::Result<Option<PresentationTarget>> {
    match scanout {
        super::super::scanout::NativeScanoutBackend::AtomicEglGbm(explicit) => {
            explicit.swapchain()?.latest_future_fixed_target()
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
    use super::super::frame::NativeCursorPhysicalAdmission;
    use super::super::frame::{
        cursor_output_disposition_at_deadline, cursor_output_disposition_for_primary_state,
        plane_delta_allowed_at_deadline,
    };
    use super::super::wake_plan::{
        NativeDeadline, NativeDeadlineOwner, NativeWakePlanInputs, build_native_wake_plan,
        cycle_tail_requires_runtime_wake_plan,
    };
    use super::super::{
        NativeCursorOutputArbitration, NativeCursorOutputDisposition, NativeCursorPrimaryState,
        NativeCursorSchedulingPolicy,
    };
    use crate::native_output::presentation::cursor_timing::CursorTimingDomain;
    use crate::native_output::presentation::kms_timing::{
        KmsModeTiming, KmsPresentationTimingModel,
    };
    use crate::native_output::runtime::update_cursor_output_arbitration;
    use crate::native_output::{
        NativeScanoutKind,
        kms_worker::{KmsCommitWorkerPolicy, KmsCommitWorkerTransport},
        runtime::NativeCursorPreference,
    };
    use drm_sys::drm_mode_modeinfo;
    use oblivion_one::native::kms::{
        AtomicKmsError, AtomicKmsErrorKind, KmsBackendKind, KmsPolicy,
    };
    use oblivion_one::native::presentation_deadline::{
        MonotonicTimestampNs, PresentationDeadlinePlanner, PresentationTarget,
        PresentationTargetReason,
    };
    use oblivion_one::native::scheduler::NativeFrameScheduler;

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
        let timing = CursorTimingDomain::new(
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
        let timing = CursorTimingDomain::new(
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
    fn auto_idle_cursor_with_valid_opportunity_arms_response_and_settles_epoch() {
        const REFRESH_NS: u64 = 1_000_000_000 / 165;
        let now_ns = 1_000_000_000;
        let now = MonotonicTimestampNs::new(now_ns);
        let mut planner = PresentationDeadlinePlanner::new(Duration::from_nanos(REFRESH_NS));
        planner.note_presented(now);
        let timing = CursorTimingDomain::new(
            KmsModeTiming::from_mode(&drm_mode_modeinfo::default(), REFRESH_NS),
            7,
        );
        let opportunity = plan_cursor_service_opportunity(&planner, &timing, now, None, 500_000)
            .expect("valid KMS-timed opportunity");
        let scheduler = NativeFrameScheduler::new(165, 0);
        let mut arbitration = NativeCursorOutputArbitration::default();

        update_cursor_output_arbitration(
            &mut arbitration,
            NativeCursorSchedulingPolicy::Auto,
            7,
            6,
            now_ns,
            &scheduler,
            false,
            true,
            Some(opportunity),
        );

        assert!(arbitration.pending());
        assert!(arbitration.hardware_cursor_pending());
        assert_eq!(arbitration.desired_epoch(), 7);
        assert_eq!(arbitration.service_opportunity(), Some(opportunity));
        assert_eq!(arbitration.pending_since_ns(), Some(now_ns));
        let wake_plan = build_native_wake_plan(NativeWakePlanInputs {
            now_ns,
            cursor_response_deadline_ns: arbitration.wake_deadline_ns(now_ns),
            cursor_planning_deadline_ns: arbitration.planning_retry_wake_deadline_ns(now_ns),
            ..NativeWakePlanInputs::default()
        });
        assert_eq!(
            wake_plan.deadline,
            Some(NativeDeadline {
                owner: NativeDeadlineOwner::CursorResponse,
                at_ns: opportunity.service_deadline_ns,
            })
        );
        assert!(cycle_tail_requires_runtime_wake_plan(
            true,
            false,
            false,
            arbitration.has_future_wake_requirement(now_ns, KmsCommitWorkerTransport::Worker),
        ));

        let disposition = cursor_output_disposition_for_primary_state(
            &mut arbitration,
            NativeCursorSchedulingPolicy::Auto,
            opportunity.service_deadline_ns,
            NativeCursorPrimaryState {
                independent_kms_admission: true,
                ..NativeCursorPrimaryState::default()
            },
            true,
        );
        assert_eq!(disposition, NativeCursorOutputDisposition::SubmitPlaneDelta);
        arbitration.note_plane_delta_submission();
        arbitration.consume_submitted_epoch(
            7,
            opportunity.service_deadline_ns,
            scheduler.next_refresh_deadline_ns(opportunity.service_deadline_ns),
        );

        assert!(!arbitration.pending());
        assert_eq!(arbitration.plane_delta_submissions(), 1);
    }

    #[test]
    fn auto_replans_before_due_disposition_without_replacing_reachable_opportunity() {
        const REFRESH_NS: u64 = 1_000_000_000 / 165;
        let input_ns = 1_000_000_000;
        let input_time = MonotonicTimestampNs::new(input_ns);
        let mut planner = PresentationDeadlinePlanner::new(Duration::from_nanos(REFRESH_NS));
        planner.note_presented(input_time);
        let timing = CursorTimingDomain::new(
            KmsModeTiming::from_mode(&drm_mode_modeinfo::default(), REFRESH_NS),
            7,
        );
        let scheduler = NativeFrameScheduler::new(165, 0);
        let first = plan_cursor_service_opportunity(&planner, &timing, input_time, None, 500_000)
            .expect("initial KMS-timed opportunity");
        let mut arbitration = NativeCursorOutputArbitration::default();

        let (cursor_work, initial_due, initial_plane_delta) = update_cursor_output_arbitration(
            &mut arbitration,
            NativeCursorSchedulingPolicy::Auto,
            42,
            41,
            input_ns,
            &scheduler,
            false,
            true,
            Some(first),
        );
        assert!(cursor_work);
        assert!(!initial_due);
        assert!(!initial_plane_delta);
        let initial_wake_plan = build_native_wake_plan(NativeWakePlanInputs {
            now_ns: input_ns,
            cursor_response_deadline_ns: arbitration.wake_deadline_ns(input_ns),
            cursor_planning_deadline_ns: arbitration.planning_retry_wake_deadline_ns(input_ns),
            ..NativeWakePlanInputs::default()
        });
        assert_eq!(
            initial_wake_plan.deadline,
            Some(NativeDeadline {
                owner: NativeDeadlineOwner::CursorResponse,
                at_ns: first.service_deadline_ns,
            })
        );
        assert!(cycle_tail_requires_runtime_wake_plan(
            true,
            false,
            false,
            arbitration.has_future_wake_requirement(input_ns, KmsCommitWorkerTransport::Worker),
        ));

        // Model the actual presentation-cycle order: the CursorResponse
        // timer wakes, then the planner runs again before disposition. A
        // one-nanosecond late wake keeps O1 while its KMS window is open.
        let replan_ns = first.service_deadline_ns.saturating_add(1);
        assert!(replan_ns < first.submit_window.commit_complete_deadline_ns());
        let replanned = plan_cursor_service_opportunity_with_current(
            &planner,
            &timing,
            MonotonicTimestampNs::new(replan_ns),
            None,
            500_000,
            Some(first),
            KmsCommitWorkerTransport::Worker,
        )
        .expect("late-latch preserves the current physical opportunity");
        assert_eq!(replanned.target.identity(), first.target.identity());
        assert!(replanned.service_deadline_ns <= replan_ns);

        let (cursor_work, deadline_due, cursor_plane_delta_due) = update_cursor_output_arbitration(
            &mut arbitration,
            NativeCursorSchedulingPolicy::Auto,
            42,
            41,
            replan_ns,
            &scheduler,
            false,
            true,
            Some(replanned),
        );
        assert!(cursor_work);
        assert!(deadline_due);
        assert!(cursor_plane_delta_due);

        assert!(
            !super::super::presentation_cycle::cursor_work_requires_primary_repaint(
                NativeCursorSchedulingPolicy::Auto,
                true,
                true,
                false,
            )
        );
        assert_eq!(arbitration.pending_since_ns(), Some(input_ns));
        assert_eq!(arbitration.deadline_ns(), Some(first.service_deadline_ns));
        assert_eq!(arbitration.response_windows_opened, 1);
        assert_eq!(
            arbitration.service_opportunity(),
            Some(first),
            "service admission maturity must not expire O1 while its physical target remains reachable"
        );
        let disposition = cursor_output_disposition_for_primary_state(
            &mut arbitration,
            NativeCursorSchedulingPolicy::Auto,
            replan_ns,
            NativeCursorPrimaryState {
                independent_kms_admission: true,
                ..NativeCursorPrimaryState::default()
            },
            true,
        );
        assert_eq!(
            disposition,
            NativeCursorOutputDisposition::SubmitPlaneDelta,
            "the same unsatisfied epoch must submit after its original service deadline"
        );
        let selected = arbitration
            .service_opportunity()
            .expect("the PlaneDelta owns a real physical KMS opportunity");
        assert!(selected.submit_window.worker_wake_at_ns() > replan_ns);
        assert!(selected.submit_window.commit_complete_deadline_ns() > replan_ns);
        arbitration.note_plane_delta_submission();
        arbitration.consume_submitted_epoch(
            42,
            selected.submit_window.worker_wake_at_ns(),
            scheduler.next_refresh_deadline_ns(selected.submit_window.worker_wake_at_ns()),
        );
        assert!(!arbitration.pending());
        assert_eq!(arbitration.plane_delta_submissions(), 1);
        assert_eq!(
            arbitration.desired_epoch(),
            0,
            "the admitted cursor epoch settles without a later client event"
        );
        assert_eq!(
            cursor_output_disposition_for_primary_state(
                &mut arbitration,
                NativeCursorSchedulingPolicy::Auto,
                replan_ns,
                NativeCursorPrimaryState {
                    independent_kms_admission: true,
                    ..NativeCursorPrimaryState::default()
                },
                true,
            ),
            NativeCursorOutputDisposition::Noop,
            "settlement leaves no repeat PlaneDelta work"
        );
    }

    #[test]
    fn overdue_auto_cursor_survives_repeated_expired_target_rebinding() {
        const REFRESH_NS: u64 = 1_000_000_000 / 165;
        let input_ns = 2_000_000_000;
        let input_time = MonotonicTimestampNs::new(input_ns);
        let mut planner = PresentationDeadlinePlanner::new(Duration::from_nanos(REFRESH_NS));
        planner.note_presented(input_time);
        let timing = CursorTimingDomain::new(
            KmsModeTiming::from_mode(&drm_mode_modeinfo::default(), REFRESH_NS),
            7,
        );
        let scheduler = NativeFrameScheduler::new(165, 0);
        let first = plan_cursor_service_opportunity(&planner, &timing, input_time, None, 500_000)
            .expect("initial KMS-timed opportunity");
        let original_deadline = first.service_deadline_ns;
        let mut current = first;
        let mut arbitration = NativeCursorOutputArbitration::default();

        update_cursor_output_arbitration(
            &mut arbitration,
            NativeCursorSchedulingPolicy::Auto,
            42,
            41,
            input_ns,
            &scheduler,
            false,
            true,
            Some(first),
        );

        // Model repeated timer-only cycles where each prior physical target
        // is genuinely past before disposition can reserve the replacement.
        // The desired epoch arrives only once and no scene or primary work
        // occurs during these replans.
        let mut now_ns = input_ns;
        for _ in 0..4 {
            now_ns = current.target.presentation_time.get().saturating_add(1);
            let next = plan_cursor_service_opportunity_with_current(
                &planner,
                &timing,
                MonotonicTimestampNs::new(now_ns),
                None,
                500_000,
                Some(current),
                KmsCommitWorkerTransport::Worker,
            )
            .expect("a later legal physical target can be planned");
            assert_ne!(next.target.identity(), current.target.identity());
            update_cursor_output_arbitration(
                &mut arbitration,
                NativeCursorSchedulingPolicy::Auto,
                42,
                41,
                now_ns,
                &scheduler,
                false,
                true,
                Some(next),
            );

            assert_eq!(arbitration.pending_since_ns(), Some(input_ns));
            assert_eq!(arbitration.deadline_ns(), Some(original_deadline));
            assert!(arbitration.due(now_ns), "matured freshness debt is sticky");
            assert!(next.submit_window.worker_wake_at_ns() > now_ns);
            current = next;
        }

        let worker_physical_admission =
            arbitration.physical_admission_for_transport(KmsCommitWorkerTransport::Worker, now_ns);
        assert_eq!(
            worker_physical_admission,
            NativeCursorPhysicalAdmission::WorkerQueueNow,
            "worker transport can take ownership while its future physical wake remains queued"
        );
        assert_eq!(
            cursor_output_disposition_for_primary_state(
                &mut arbitration,
                NativeCursorSchedulingPolicy::Auto,
                now_ns,
                NativeCursorPrimaryState {
                    independent_kms_admission: true,
                    physical_admission: worker_physical_admission,
                    ..NativeCursorPrimaryState::default()
                },
                true,
            ),
            NativeCursorOutputDisposition::SubmitPlaneDelta,
            "the overdue cursor is handed to the worker immediately; the worker owns its future KMS wake"
        );
        assert_eq!(arbitration.desired_epoch(), 42);
        assert_eq!(arbitration.plane_delta_plans(), 1);
    }

    #[test]
    fn synchronous_auto_waits_for_replacement_physical_admission_without_sliding_freshness() {
        const REFRESH_NS: u64 = 1_000_000_000 / 165;
        let input_ns = 2_000_000_000;
        let input_time = MonotonicTimestampNs::new(input_ns);
        let mut planner = PresentationDeadlinePlanner::new(Duration::from_nanos(REFRESH_NS));
        planner.note_presented(input_time);
        let timing = CursorTimingDomain::new(
            KmsModeTiming::from_mode(&drm_mode_modeinfo::default(), REFRESH_NS),
            7,
        );
        let scheduler = NativeFrameScheduler::new(165, 0);
        let first = plan_cursor_service_opportunity(&planner, &timing, input_time, None, 500_000)
            .expect("initial KMS-timed opportunity");
        let original_deadline = first.service_deadline_ns;
        let mut arbitration = NativeCursorOutputArbitration::default();
        update_cursor_output_arbitration(
            &mut arbitration,
            NativeCursorSchedulingPolicy::Auto,
            42,
            41,
            input_ns,
            &scheduler,
            false,
            true,
            Some(first),
        );

        let replan_ns = first
            .submit_window
            .commit_complete_deadline_ns()
            .saturating_add(1);
        let replacement = plan_cursor_service_opportunity_with_current(
            &planner,
            &timing,
            MonotonicTimestampNs::new(replan_ns),
            None,
            500_000,
            Some(first),
            KmsCommitWorkerTransport::Synchronous,
        )
        .expect("replacement physical target after O1 expires");
        update_cursor_output_arbitration(
            &mut arbitration,
            NativeCursorSchedulingPolicy::Auto,
            42,
            41,
            replan_ns,
            &scheduler,
            false,
            true,
            Some(replacement),
        );

        assert!(arbitration.due(replan_ns));
        assert_eq!(arbitration.pending_since_ns(), Some(input_ns));
        assert_eq!(arbitration.deadline_ns(), Some(original_deadline));
        assert_eq!(arbitration.service_opportunity(), Some(replacement));
        assert!(replacement.service_deadline_ns > replan_ns);
        assert!(replacement.submit_window.worker_wake_at_ns() > replan_ns);
        for policy in [KmsCommitWorkerPolicy::Auto, KmsCommitWorkerPolicy::Off] {
            let transport = policy
                .effective(KmsBackendKind::Atomic, false)
                .expect("Auto startup degradation and explicit off both select sync");
            assert_eq!(transport, KmsCommitWorkerTransport::Synchronous);
            let physical_admission =
                arbitration.physical_admission_for_transport(transport, replan_ns);
            assert_eq!(
                physical_admission,
                NativeCursorPhysicalAdmission::PhysicalAt(replacement.service_deadline_ns)
            );
            assert_eq!(
                cursor_output_disposition_for_primary_state(
                    &mut arbitration,
                    NativeCursorSchedulingPolicy::Auto,
                    replan_ns,
                    NativeCursorPrimaryState {
                        independent_kms_admission: true,
                        physical_admission,
                        ..NativeCursorPrimaryState::default()
                    },
                    true,
                ),
                NativeCursorOutputDisposition::Noop,
                "synchronous Auto must retain O2 and wait for its admission point"
            );
        }
        assert_eq!(arbitration.plane_delta_plans(), 0);
        assert_eq!(arbitration.plane_delta_submissions(), 0);
        assert_eq!(
            super::plan_native_presentation_path(super::NativePresentationPlanInput {
                direct_active: false,
                direct_candidate_changed: false,
                direct_candidate_eligible: false,
                primary_visual_work_pending: false,
                cursor_changed: true,
                cursor_hardware_usable: true,
                cursor_visible: true,
                composition_required: false,
                atomic_commit_pending: false,
                plane_delta_allowed: false,
                render_ahead_requested: false,
            }),
            super::NativePresentationPath::IdleComposited,
            "waiting for physical admission does not generate a primary render"
        );
        let sync_admission_deadline = arbitration
            .synchronous_physical_admission_wake_deadline_ns(replan_ns)
            .expect("synchronous transport owns the future physical admission");
        assert_eq!(sync_admission_deadline, replacement.service_deadline_ns);
        let mut software_fallback = arbitration;
        software_fallback.set_software_overlay_pending(true);
        software_fallback.reconcile_hardware_cursor_liveness(false);
        assert!(!software_fallback.hardware_cursor_pending());
        assert_eq!(
            software_fallback.synchronous_physical_admission_wake_deadline_ns(replan_ns),
            None,
            "software fallback drops the hardware physical-admission timer"
        );
        let fallback_physical_admission = software_fallback
            .physical_admission_for_transport(KmsCommitWorkerTransport::Synchronous, replan_ns);
        assert_eq!(
            cursor_output_disposition_for_primary_state(
                &mut software_fallback,
                NativeCursorSchedulingPolicy::Auto,
                replan_ns,
                NativeCursorPrimaryState {
                    independent_kms_admission: true,
                    physical_admission: fallback_physical_admission,
                    ..NativeCursorPrimaryState::default()
                },
                true,
            ),
            NativeCursorOutputDisposition::SoftwareOverlay
        );
        assert!(
            arbitration
                .has_future_wake_requirement(replan_ns, KmsCommitWorkerTransport::Synchronous)
        );
        assert!(cycle_tail_requires_runtime_wake_plan(
            true,
            false,
            false,
            arbitration
                .has_future_wake_requirement(replan_ns, KmsCommitWorkerTransport::Synchronous),
        ));
        assert!(
            !arbitration.has_future_wake_requirement(replan_ns, KmsCommitWorkerTransport::Worker)
        );
        let admission_plan = build_native_wake_plan(NativeWakePlanInputs {
            now_ns: replan_ns,
            cursor_sync_admission_deadline_ns: Some(sync_admission_deadline),
            ..NativeWakePlanInputs::default()
        });
        let mut wake_metrics = super::super::wake_plan::NativeWakeAuthorityMetrics::default();
        wake_metrics.observe_plan(admission_plan, replan_ns, None, None);
        assert_eq!(wake_metrics.past_deadline_arms, 0);
        assert_eq!(wake_metrics.stale_deadline_rearms, 0);
        assert_eq!(
            admission_plan.deadline,
            Some(NativeDeadline {
                owner: NativeDeadlineOwner::CursorSynchronousAdmission,
                at_ns: replacement.service_deadline_ns,
            })
        );
        assert_eq!(
            arbitration
                .synchronous_physical_admission_wake_deadline_ns(replacement.service_deadline_ns),
            None,
            "an admission wake does not rearm itself at zero delay"
        );

        assert_eq!(
            {
                let physical_admission = arbitration.physical_admission_for_transport(
                    KmsCommitWorkerTransport::Synchronous,
                    replacement.service_deadline_ns,
                );
                cursor_output_disposition_for_primary_state(
                    &mut arbitration,
                    NativeCursorSchedulingPolicy::Auto,
                    replacement.service_deadline_ns,
                    NativeCursorPrimaryState {
                        independent_kms_admission: true,
                        physical_admission,
                        ..NativeCursorPrimaryState::default()
                    },
                    true,
                )
            },
            NativeCursorOutputDisposition::SubmitPlaneDelta
        );
        arbitration.note_plane_delta_submission();
        arbitration.consume_submitted_epoch(
            42,
            replacement.service_deadline_ns,
            scheduler.next_refresh_deadline_ns(replacement.service_deadline_ns),
        );
        assert!(!arbitration.pending());
        assert_eq!(arbitration.plane_delta_plans(), 1);
        assert_eq!(arbitration.plane_delta_submissions(), 1);
    }

    #[test]
    fn synchronous_auto_replans_an_expired_replacement_without_resetting_freshness() {
        const REFRESH_NS: u64 = 1_000_000_000 / 165;
        let input_ns = 3_000_000_000;
        let input_time = MonotonicTimestampNs::new(input_ns);
        let mut planner = PresentationDeadlinePlanner::new(Duration::from_nanos(REFRESH_NS));
        planner.note_presented(input_time);
        let timing = CursorTimingDomain::new(
            KmsModeTiming::from_mode(&drm_mode_modeinfo::default(), REFRESH_NS),
            7,
        );
        let scheduler = NativeFrameScheduler::new(165, 0);
        let first = plan_cursor_service_opportunity(&planner, &timing, input_time, None, 500_000)
            .expect("initial KMS-timed opportunity");
        let original_deadline = first.service_deadline_ns;
        let mut arbitration = NativeCursorOutputArbitration::default();
        arbitration.request_hardware_for_opportunity(42, input_ns, first);

        let o2_replan_ns = first
            .submit_window
            .commit_complete_deadline_ns()
            .saturating_add(1);
        let second = plan_cursor_service_opportunity_with_current(
            &planner,
            &timing,
            MonotonicTimestampNs::new(o2_replan_ns),
            None,
            500_000,
            Some(first),
            KmsCommitWorkerTransport::Synchronous,
        )
        .expect("O2 after O1 physically expires");
        arbitration.reconcile_pending_service_opportunity(second, o2_replan_ns);
        assert_eq!(arbitration.service_opportunity(), Some(second));
        assert_eq!(arbitration.deadline_ns(), Some(original_deadline));

        // The O2 admission wake is missed too. Replan O3 and retain the one
        // original cursor age/deadline rather than opening a fresh response.
        let o3_replan_ns = second
            .submit_window
            .commit_complete_deadline_ns()
            .saturating_add(1);
        let third = plan_cursor_service_opportunity_with_current(
            &planner,
            &timing,
            MonotonicTimestampNs::new(o3_replan_ns),
            None,
            500_000,
            Some(second),
            KmsCommitWorkerTransport::Synchronous,
        )
        .expect("O3 after O2 physically expires");
        arbitration.reconcile_pending_service_opportunity(third, o3_replan_ns);

        assert!(arbitration.due(o3_replan_ns));
        assert_eq!(arbitration.pending_since_ns(), Some(input_ns));
        assert_eq!(arbitration.deadline_ns(), Some(original_deadline));
        assert_eq!(arbitration.service_opportunity(), Some(third));
        assert_eq!(
            arbitration.synchronous_physical_admission_wake_deadline_ns(o3_replan_ns),
            Some(third.service_deadline_ns)
        );
        let physical_admission = arbitration.physical_admission_for_transport(
            KmsCommitWorkerTransport::Synchronous,
            third.service_deadline_ns,
        );
        assert_eq!(
            cursor_output_disposition_for_primary_state(
                &mut arbitration,
                NativeCursorSchedulingPolicy::Auto,
                third.service_deadline_ns,
                NativeCursorPrimaryState {
                    independent_kms_admission: true,
                    physical_admission,
                    ..NativeCursorPrimaryState::default()
                },
                true,
            ),
            NativeCursorOutputDisposition::SubmitPlaneDelta
        );
        arbitration.note_plane_delta_submission();
        arbitration.consume_submitted_epoch(
            42,
            third.service_deadline_ns,
            scheduler.next_refresh_deadline_ns(third.service_deadline_ns),
        );
        assert!(!arbitration.pending());
        assert_eq!(arbitration.plane_delta_submissions(), 1);
    }

    #[test]
    fn auto_idle_cursor_without_initial_opportunity_replans_and_settles_epoch() {
        const REFRESH_NS: u64 = 1_000_000_000 / 165;
        let event_ns = 1_000_000_000;
        let event_time = MonotonicTimestampNs::new(event_ns);
        let mut planner = PresentationDeadlinePlanner::new(Duration::from_nanos(REFRESH_NS));
        planner.note_presented(event_time);
        let timing = CursorTimingDomain::new(
            KmsModeTiming::from_mode(&drm_mode_modeinfo::default(), REFRESH_NS),
            7,
        );
        let scheduler = NativeFrameScheduler::new(165, 0);
        let mut arbitration = NativeCursorOutputArbitration::default();

        update_cursor_output_arbitration(
            &mut arbitration,
            NativeCursorSchedulingPolicy::Auto,
            8,
            7,
            event_ns,
            &scheduler,
            false,
            true,
            None,
        );

        assert!(arbitration.pending());
        assert_eq!(arbitration.desired_epoch(), 8);
        assert_eq!(arbitration.pending_since_ns(), Some(event_ns));
        assert_eq!(arbitration.service_opportunity(), None);
        let planning_wake_ns = scheduler.next_refresh_deadline_ns(event_ns);
        let planning_plan = build_native_wake_plan(NativeWakePlanInputs {
            now_ns: event_ns,
            cursor_response_deadline_ns: arbitration.wake_deadline_ns(event_ns),
            cursor_planning_deadline_ns: arbitration.planning_retry_wake_deadline_ns(event_ns),
            ..NativeWakePlanInputs::default()
        });
        assert_eq!(
            planning_plan.deadline,
            Some(NativeDeadline {
                owner: NativeDeadlineOwner::CursorPlanningRetry,
                at_ns: planning_wake_ns,
            })
        );
        assert_eq!(arbitration.retry_not_before_ns(), None);
        assert_eq!(
            cursor_output_disposition_for_primary_state(
                &mut arbitration,
                NativeCursorSchedulingPolicy::Auto,
                planning_wake_ns,
                NativeCursorPrimaryState {
                    independent_kms_admission: true,
                    ..NativeCursorPrimaryState::default()
                },
                true,
            ),
            NativeCursorOutputDisposition::Noop,
            "the planning timer must not authorize an untimed plane delta"
        );

        // The timer-only work-domain test verifies this wake enters cursor
        // presentation planning without scene, client, or KMS completion work.
        let replanning_time = MonotonicTimestampNs::new(planning_wake_ns);
        let opportunity =
            plan_cursor_service_opportunity(&planner, &timing, replanning_time, None, 500_000)
                .expect("replanning obtains a current KMS opportunity");
        update_cursor_output_arbitration(
            &mut arbitration,
            NativeCursorSchedulingPolicy::Auto,
            8,
            7,
            planning_wake_ns,
            &scheduler,
            false,
            true,
            Some(opportunity),
        );

        assert_eq!(arbitration.pending_since_ns(), Some(event_ns));
        assert_eq!(arbitration.desired_epoch(), 8);
        assert_eq!(arbitration.planning_retry_ns(), None);
        assert_eq!(arbitration.service_opportunity(), Some(opportunity));
        let response_plan = build_native_wake_plan(NativeWakePlanInputs {
            now_ns: planning_wake_ns,
            cursor_response_deadline_ns: arbitration.wake_deadline_ns(planning_wake_ns),
            cursor_planning_deadline_ns: arbitration
                .planning_retry_wake_deadline_ns(planning_wake_ns),
            ..NativeWakePlanInputs::default()
        });
        assert!(opportunity.service_deadline_ns <= planning_wake_ns);
        assert_eq!(
            response_plan.deadline, None,
            "the planning timer already woke after the soft cursor deadline"
        );

        let disposition = cursor_output_disposition_for_primary_state(
            &mut arbitration,
            NativeCursorSchedulingPolicy::Auto,
            planning_wake_ns,
            NativeCursorPrimaryState {
                independent_kms_admission: true,
                ..NativeCursorPrimaryState::default()
            },
            true,
        );
        assert_eq!(disposition, NativeCursorOutputDisposition::SubmitPlaneDelta);
        arbitration.note_plane_delta_submission();
        arbitration.consume_submitted_epoch(
            8,
            planning_wake_ns,
            scheduler.next_refresh_deadline_ns(planning_wake_ns),
        );
        assert!(!arbitration.pending());
    }

    #[test]
    fn final_auto_cursor_position_after_1000_hz_burst_settles_without_more_input() {
        const REFRESH_NS: u64 = 1_000_000_000 / 165;
        const INPUT_INTERVAL_NS: u64 = 1_000_000;
        const INPUT_COUNT: u64 = 100;
        let burst_start_ns = 2_000_000_000;
        let mut presentation_planner =
            PresentationDeadlinePlanner::new(Duration::from_nanos(REFRESH_NS));
        presentation_planner.note_presented(MonotonicTimestampNs::new(burst_start_ns));
        let timing = CursorTimingDomain::new(
            KmsModeTiming::from_mode(&drm_mode_modeinfo::default(), REFRESH_NS),
            7,
        );
        let scheduler = NativeFrameScheduler::new(165, 0);
        let mut arbitration = NativeCursorOutputArbitration::default();

        for epoch in 1..=INPUT_COUNT {
            let event_ns = burst_start_ns + epoch * INPUT_INTERVAL_NS;
            update_cursor_output_arbitration(
                &mut arbitration,
                NativeCursorSchedulingPolicy::Auto,
                epoch,
                0,
                event_ns,
                &scheduler,
                false,
                true,
                None,
            );
            assert_eq!(arbitration.desired_epoch(), epoch);
            assert!(arbitration.pending());
        }

        let final_epoch = INPUT_COUNT;
        let final_event_ns = burst_start_ns + INPUT_COUNT * INPUT_INTERVAL_NS;
        assert_eq!(
            arbitration.pending_since_ns(),
            Some(burst_start_ns + INPUT_INTERVAL_NS)
        );
        assert!(arbitration.hardware_cursor_pending());
        let planning_wake_ns = arbitration
            .planning_retry_wake_deadline_ns(final_event_ns)
            .expect("the final input owns a future planning wake");
        assert!(planning_wake_ns > final_event_ns);

        // No event calls occur after the final movement. The timer drives the
        // opportunity replan, then its KMS service deadline drives submission.
        let opportunity = plan_cursor_service_opportunity(
            &presentation_planner,
            &timing,
            MonotonicTimestampNs::new(planning_wake_ns),
            None,
            500_000,
        )
        .expect("final pointer movement receives a physical service opportunity");
        update_cursor_output_arbitration(
            &mut arbitration,
            NativeCursorSchedulingPolicy::Auto,
            final_epoch,
            0,
            planning_wake_ns,
            &scheduler,
            false,
            true,
            Some(opportunity),
        );
        assert_eq!(arbitration.desired_epoch(), final_epoch);
        assert_eq!(
            arbitration.pending_since_ns(),
            Some(burst_start_ns + INPUT_INTERVAL_NS)
        );

        let disposition = cursor_output_disposition_for_primary_state(
            &mut arbitration,
            NativeCursorSchedulingPolicy::Auto,
            opportunity.service_deadline_ns,
            NativeCursorPrimaryState {
                independent_kms_admission: true,
                ..NativeCursorPrimaryState::default()
            },
            true,
        );
        assert_eq!(disposition, NativeCursorOutputDisposition::SubmitPlaneDelta);
        arbitration.note_plane_delta_submission();
        arbitration.consume_submitted_epoch(
            final_epoch,
            opportunity.service_deadline_ns,
            scheduler.next_refresh_deadline_ns(opportunity.service_deadline_ns),
        );
        assert!(!arbitration.pending());
        assert_eq!(arbitration.plane_delta_submissions(), 1);
    }

    #[test]
    fn newly_reserved_primary_invalidates_pending_cursor_target_without_sliding_age() {
        const REFRESH_NS: u64 = 10_000_000;
        let now = MonotonicTimestampNs::new(1_000_000_000);
        let mut planner = PresentationDeadlinePlanner::new(Duration::from_nanos(REFRESH_NS));
        planner.note_presented(now);
        let timing = CursorTimingDomain::new(
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
        assert_eq!(
            arbitration.wake_deadline_ns(now.get() + 100_000),
            Some(initial.service_deadline_ns),
            "rebinding a physical target must retain the original freshness wake"
        );
    }

    #[test]
    fn output_clock_generation_change_replaces_cursor_service_opportunity() {
        const REFRESH_NS: u64 = 10_000_000;
        let now = MonotonicTimestampNs::new(1_000_000_000);
        let mut planner = PresentationDeadlinePlanner::new(Duration::from_nanos(REFRESH_NS));
        planner.note_presented(now);
        let timing = CursorTimingDomain::new(
            KmsModeTiming::from_mode(&drm_mode_modeinfo::default(), REFRESH_NS),
            7,
        );
        let initial = plan_cursor_service_opportunity(&planner, &timing, now, None, 500_000)
            .expect("initial cursor service opportunity");
        let mut arbitration = NativeCursorOutputArbitration::default();
        arbitration.request_hardware_for_opportunity(1, now.get(), initial);

        let mut new_generation_target = initial.target;
        new_generation_target.clock_generation =
            new_generation_target.clock_generation.saturating_add(1);
        new_generation_target.physical_claim.clock_generation =
            new_generation_target.clock_generation;
        let next_generation = CursorServiceOpportunity {
            target: new_generation_target,
            ..initial
        };
        arbitration.request_hardware_for_opportunity(
            2,
            now.get().saturating_add(100_000),
            next_generation,
        );

        assert_eq!(arbitration.service_opportunity(), Some(next_generation));
        assert_eq!(arbitration.pending_since_ns(), Some(now.get()));
        assert_eq!(arbitration.deadline_ns(), Some(initial.service_deadline_ns));
    }

    #[test]
    fn cursor_service_windows_are_future_and_kms_timed_at_supported_refresh_rates() {
        let now = MonotonicTimestampNs::new(2_000_000_000);

        for hz in [60u64, 120, 144, 165, 240] {
            let refresh_ns = 1_000_000_000 / hz;
            let mut planner = PresentationDeadlinePlanner::new(Duration::from_nanos(refresh_ns));
            planner.note_presented(now);
            let timing = CursorTimingDomain::new(
                KmsModeTiming::from_mode(&drm_mode_modeinfo::default(), refresh_ns),
                9,
            );
            let opportunity =
                plan_cursor_service_opportunity(&planner, &timing, now, None, 500_000).unwrap();

            assert!(opportunity.target.presentation_time.get() > now.get());
            assert_eq!(
                opportunity.service_deadline_ns,
                opportunity.target.presentation_time.get() - opportunity.total_lead_ns
            );
        }
    }

    #[test]
    fn primary_kms_stress_does_not_change_cursor_target_or_service_deadline() {
        const REFRESH_NS: u64 = 1_000_000_000 / 165;
        let now = MonotonicTimestampNs::new(2_000_000_000);
        let mut planner = PresentationDeadlinePlanner::new(Duration::from_nanos(REFRESH_NS));
        planner.note_presented(now);
        let mode = KmsModeTiming::from_mode(&drm_mode_modeinfo::default(), REFRESH_NS);
        let healthy_primary = KmsPresentationTimingModel::new(mode, 9);
        let mut stressed_primary = healthy_primary;
        for _ in 0..128 {
            stressed_primary.observe_pageflip(
                9,
                mode.key(),
                crate::native_output::presentation::kms_timing::KmsPresentationOutcome::KmsApplyGuardMiss,
            );
        }

        assert!(
            stressed_primary.adaptive_apply_guard_ns() > healthy_primary.adaptive_apply_guard_ns()
        );
        let cursor_timing = CursorTimingDomain::new(mode, 9);
        let healthy = plan_cursor_service_opportunity(&planner, &cursor_timing, now, None, 500_000)
            .expect("healthy cursor opportunity");
        let after_primary_stress =
            plan_cursor_service_opportunity(&planner, &cursor_timing, now, None, 500_000)
                .expect("cursor opportunity remains available");

        assert_eq!(
            after_primary_stress.target.identity(),
            healthy.target.identity()
        );
        assert_eq!(
            after_primary_stress.service_deadline_ns,
            healthy.service_deadline_ns
        );
    }

    #[test]
    fn cursor_soft_budget_crossing_a_refresh_does_not_create_a_refresh_divider() {
        for hz in [60u64, 120, 144, 165, 240, 360] {
            let refresh_ns = 1_000_000_000 / hz;
            let now = MonotonicTimestampNs::new(3_000_000_000);
            let mut planner = PresentationDeadlinePlanner::new(Duration::from_nanos(refresh_ns));
            planner.note_presented(now);
            let timing = CursorTimingDomain::new(
                KmsModeTiming::from_mode(&drm_mode_modeinfo::default(), refresh_ns),
                11,
            );
            let fixed_lead_ns = timing
                .preparation_budget_ns()
                .saturating_add(timing.apply_guard_ns(now.get()));
            let below = refresh_ns.saturating_sub(fixed_lead_ns).saturating_sub(1);
            let above = refresh_ns.saturating_sub(fixed_lead_ns).saturating_add(1);
            let before = plan_cursor_service_opportunity(&planner, &timing, now, None, below)
                .expect("cursor opportunity below refresh budget");
            let after = plan_cursor_service_opportunity(&planner, &timing, now, None, above)
                .expect("late-latched cursor opportunity above refresh budget");

            assert_eq!(
                after.target.sequence, before.target.sequence,
                "crossing the soft lead budget at {hz} Hz must not skip one refresh"
            );
        }
    }

    #[test]
    fn cursor_motion_1000hz_coalesces_to_one_target_per_refresh_at_supported_rates() {
        use std::collections::BTreeSet;

        const SIMULATION_NS: u64 = 1_000_000_000;
        const INPUT_INTERVAL_NS: u64 = 1_000_000;

        for hz in [60u64, 120, 144, 165, 240, 360] {
            let refresh_ns = SIMULATION_NS / hz;
            let mut planner = PresentationDeadlinePlanner::new(Duration::from_nanos(refresh_ns));
            let mut now = MonotonicTimestampNs::new(0);
            planner.note_presented(now);
            let timing = CursorTimingDomain::new(
                KmsModeTiming::from_mode(&drm_mode_modeinfo::default(), refresh_ns),
                17,
            );
            let mut arbitration = NativeCursorOutputArbitration::default();
            let mut next_input_ns = INPUT_INTERVAL_NS;
            let mut input_epoch = 0_u64;
            let mut raw_updates = 0_u64;
            let mut opportunities = 0_u64;
            let mut independent_target_sequences = BTreeSet::new();

            while now.get() < SIMULATION_NS {
                let opportunity =
                    plan_cursor_service_opportunity(&planner, &timing, now, None, 50_000)
                        .expect("fixed-refresh cursor opportunity");
                opportunities = opportunities.saturating_add(1);
                let mut oldest_motion_ns = None;

                while next_input_ns < opportunity.target.presentation_time.get()
                    && next_input_ns < SIMULATION_NS
                {
                    input_epoch = input_epoch.saturating_add(1);
                    raw_updates = raw_updates.saturating_add(1);
                    oldest_motion_ns.get_or_insert(next_input_ns);
                    arbitration.request_hardware_for_opportunity(
                        input_epoch,
                        next_input_ns,
                        opportunity,
                    );
                    assert_eq!(arbitration.pending_since_ns(), oldest_motion_ns);
                    assert_eq!(
                        arbitration.deadline_ns(),
                        Some(opportunity.service_deadline_ns)
                    );
                    assert_eq!(
                        arbitration
                            .service_opportunity()
                            .map(|current| current.target.sequence),
                        Some(opportunity.target.sequence),
                        "raw motion may refresh state but cannot create a second target"
                    );
                    next_input_ns = next_input_ns.saturating_add(INPUT_INTERVAL_NS);
                }

                if arbitration.pending() {
                    assert_eq!(arbitration.desired_epoch(), input_epoch);
                    assert!(independent_target_sequences.insert(opportunity.target.sequence));
                    arbitration.consume(input_epoch);
                }

                now = opportunity.target.presentation_time;
                planner.note_presented(now);
            }

            assert!(raw_updates > independent_target_sequences.len() as u64);
            assert!(independent_target_sequences.len() as u64 <= opportunities);
        }
    }

    #[test]
    fn cursor_motion_keeps_one_service_deadline_and_replans_missed_targets_without_resetting_age() {
        const REFRESH_NS: u64 = 10_000_000;
        let now = MonotonicTimestampNs::new(1_000_000_000);
        let mut planner = PresentationDeadlinePlanner::new(Duration::from_nanos(REFRESH_NS));
        planner.note_presented(now);
        let timing = CursorTimingDomain::new(
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

        let late_service_at = first
            .submit_window
            .commit_complete_deadline_ns()
            .saturating_add(1);
        assert!(arbitration.due(late_service_at));
        assert_eq!(
            arbitration.physical_admission_for_transport(
                KmsCommitWorkerTransport::Worker,
                late_service_at
            ),
            NativeCursorPhysicalAdmission::WorkerQueueNow,
            "worker transport can still late-latch while the physical refresh is current"
        );
        let late_latched = plan_cursor_service_opportunity_with_current(
            &planner,
            &timing,
            MonotonicTimestampNs::new(late_service_at),
            None,
            500_000,
            Some(first),
            KmsCommitWorkerTransport::Worker,
        )
        .expect("late-latched physical cursor target");
        assert_eq!(late_latched.target.identity(), first.target.identity());

        let target_missed_at = first.target.presentation_time.get().saturating_add(1);
        let next = plan_cursor_service_opportunity_with_current(
            &planner,
            &timing,
            MonotonicTimestampNs::new(target_missed_at),
            None,
            500_000,
            Some(first),
            KmsCommitWorkerTransport::Worker,
        )
        .expect("next physical cursor target after a real target miss");
        assert!(next.target.sequence > first.target.sequence);
        arbitration.request_hardware_for_opportunity(3, target_missed_at, next);
        assert_eq!(arbitration.deadline_ns(), Some(original_deadline));
        assert_eq!(arbitration.pending_since_ns(), Some(now.get()));
        assert_eq!(
            arbitration
                .service_opportunity()
                .map(|opportunity| opportunity.target.sequence),
            Some(next.target.sequence)
        );
        assert!(arbitration.due(target_missed_at));
        assert_eq!(
            arbitration.wake_deadline_ns(target_missed_at),
            None,
            "matured freshness must not be postponed to a replacement target's service point"
        );
        assert!(next.submit_window.worker_wake_at_ns() > target_missed_at);
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
                let timing = CursorTimingDomain::new(
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
