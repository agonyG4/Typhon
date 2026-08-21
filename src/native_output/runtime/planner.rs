use oblivion_one::native::kms::{AtomicKmsError, AtomicKmsErrorKind, KmsPolicy};
use oblivion_one::native::presentation_deadline::{
    MonotonicTimestampNs, PresentationDeadlinePlanner, PresentationTarget, PresentationTargetReason,
};
use oblivion_one::native::scheduler::NativeOutputPacingMode;
use oblivion_one::{compositor::OwnCompositorServer, native::scheduler::NativeFrameScheduler};
use std::io;
use std::time::Duration;

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

#[allow(clippy::too_many_arguments)]
pub(super) fn prepare_presentation_target_for_mode(
    pacing_mode: NativeOutputPacingMode,
    explicit_output: bool,
    planner: &mut PresentationDeadlinePlanner,
    server: &mut OwnCompositorServer,
    frame_scheduler: &NativeFrameScheduler,
    scheduled: Option<PresentationTarget>,
    now: MonotonicTimestampNs,
    predicted_total_cost: Duration,
) -> Option<PresentationTarget> {
    let scheduled = if pacing_mode == NativeOutputPacingMode::ReactiveDouble
        && scheduled.is_none_or(|target| target.reason != PresentationTargetReason::CommitTiming)
    {
        planner.clear_scheduled_target();
        None
    } else {
        scheduled
    };
    let scheduled = if explicit_output {
        plan_commit_timing_target(
            planner,
            server,
            frame_scheduler,
            scheduled,
            now,
            predicted_total_cost,
        )
    } else {
        scheduled
    };
    server.progress_surface_pacing(now.get());
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

pub(super) fn take_reactive_or_commit_timing_target(
    planner: &PresentationDeadlinePlanner,
    scheduled: &mut Option<PresentationTarget>,
    lower_bound: Option<PresentationTarget>,
    now: MonotonicTimestampNs,
    predicted_total_cost: Duration,
) -> Option<PresentationTarget> {
    scheduled
        .take()
        .filter(|target| target.reason == PresentationTargetReason::CommitTiming)
        .or_else(|| {
            lower_bound
                .and_then(|lower_bound| {
                    planner.reactive_target_after(now, predicted_total_cost, lower_bound)
                })
                .or_else(|| planner.reactive_target(now, predicted_total_cost))
        })
}

pub(super) fn plan_scheduled_target_for_mode(
    planner: &mut PresentationDeadlinePlanner,
    pacing_mode: NativeOutputPacingMode,
    pending_target: Option<PresentationTarget>,
    now: MonotonicTimestampNs,
    predicted_total_cost: Duration,
    reason: PresentationTargetReason,
) -> Option<PresentationTarget> {
    if pacing_mode != NativeOutputPacingMode::PredictiveTriple {
        return None;
    }
    planner.plan_render_ahead(pending_target?, now, predicted_total_cost, reason)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn plan_visual_target_for_mode(
    planner: &mut PresentationDeadlinePlanner,
    pacing_mode: NativeOutputPacingMode,
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
        return planner.plan_target_after(pending, now, predicted_total_cost, scheduled.reason);
    }
    if pacing_mode != NativeOutputPacingMode::PredictiveTriple {
        return None;
    }
    if scheduled.is_some() {
        return scheduled;
    }
    let reason = if force_validation {
        PresentationTargetReason::ForcedValidation
    } else {
        PresentationTargetReason::PredictedPressure
    };
    plan_scheduled_target_for_mode(
        planner,
        pacing_mode,
        pending_target,
        now,
        predicted_total_cost,
        reason,
    )
}

fn strictly_later_target(earlier: PresentationTarget, later: PresentationTarget) -> bool {
    earlier.clock_generation == later.clock_generation
        && later.sequence > earlier.sequence
        && later.presentation_time > earlier.presentation_time
}

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
    use super::super::{NativeCursorOutputArbitration, NativeCursorOutputDisposition};
    use crate::native_output::{NativeScanoutKind, runtime::NativeCursorPreference};
    use oblivion_one::native::kms::{
        AtomicKmsError, AtomicKmsErrorKind, KmsBackendKind, KmsPolicy,
    };

    use super::*;

    fn discovery_error() -> AtomicKmsError {
        AtomicKmsError::new(
            AtomicKmsErrorKind::MissingProperty,
            "missing Atomic property",
        )
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
