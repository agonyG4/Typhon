use super::cursor_cycle::{NativeResolvedCursorSource, resolve_native_cursor_source_with_hidden};
use super::*;
use crate::native_output::kms_worker::AttachablePrimary;
use crate::native_output::kms_worker::KmsPrimaryCursorPresentation;
use crate::native_output::presentation::plane::CursorRevision;

pub(super) fn synchronize_active_cursor_image(
    server: &OwnCompositorServer,
    cursor_manager: &mut oblivion_one::cursor_manager::CursorThemeManager,
    cursor_image: &mut std::sync::Arc<oblivion_one::cursor_theme::CompositorCursorImage>,
    frame_renderer: &mut NativeFrameRenderer,
    scanout: &mut NativeScanoutBackend,
    queued_redraw_requested: &mut bool,
) {
    cursor_manager.collect_retired_generations();
    let image = if server.interaction_cursor_override_active() {
        cursor_manager.active_image_for_shape(server.compositor_cursor_shape())
    } else {
        server.client_cursor_shape().map_or_else(
            || cursor_manager.active_image_for_shape(server.compositor_cursor_shape()),
            |shape| cursor_manager.active_image_for_protocol_shape(shape),
        )
    };
    if !std::sync::Arc::ptr_eq(cursor_image, &image) {
        *cursor_image = image.clone();
        frame_renderer.set_cursor_image(image.clone());
        scanout.set_cursor_image(image.clone());
        oblivion_one::cursor_theme::install_shared_compositor_cursor(image);
        *queued_redraw_requested = true;
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn prepare_legacy_cursor_for_frame(
    legacy_cursor: &mut Option<NativeLegacyHardwareCursor>,
    kms: &NativeDrmDevice,
    crtc_id: u32,
    cursor_image: &std::sync::Arc<oblivion_one::cursor_theme::CompositorCursorImage>,
    cursor_render_mode: &mut NativeCursorRenderMode,
    cursor_manager: &mut oblivion_one::cursor_manager::CursorThemeManager,
    client_cursor_active: bool,
    perf: NativePerfLogger,
) -> NativeResult<()> {
    refresh_legacy_cursor_theme(
        legacy_cursor,
        kms,
        crtc_id,
        cursor_image,
        cursor_render_mode,
        cursor_manager,
        perf,
    )?;
    if client_cursor_active && let Some(mut cursor) = legacy_cursor.take() {
        if let Err(error) = cursor.disable() {
            cursor.disarm_drm_cleanup();
            cursor_manager.note_hardware_fallback();
            perf.log("native.cursor", || {
                vec![
                    NativePerfField::str("event", "legacy_client_cursor_disable_failed"),
                    NativePerfField::str("error", error.to_string()),
                ]
            });
        } else {
            *legacy_cursor = Some(cursor);
        }
    }
    Ok(())
}

pub(super) fn resolve_native_cursor_visibility<'a>(
    server: &'a OwnCompositorServer,
    input_state: &NativeInputState,
) -> (
    Option<oblivion_one::compositor::ClientCursorRenderState<'a>>,
    bool,
    bool,
) {
    let theme_cursor_visible = input_state.cursor_visible();
    let client_cursor = server.client_cursor_render_state();
    let client_cursor_active = client_cursor.is_some();
    let client_shape_active =
        !server.interaction_cursor_override_active() && server.client_cursor_shape().is_some();
    let resolved_cursor_source = resolve_native_cursor_source_with_hidden(
        client_cursor_active || client_shape_active,
        server.client_cursor_explicitly_hidden(),
        server.interaction_cursor_override_active(),
        theme_cursor_visible,
    );
    let cursor_visible = !matches!(resolved_cursor_source, NativeResolvedCursorSource::Hidden);
    (client_cursor, client_cursor_active, cursor_visible)
}

pub(super) fn refresh_legacy_cursor_theme(
    legacy_cursor: &mut Option<NativeLegacyHardwareCursor>,
    kms: &NativeDrmDevice,
    crtc_id: u32,
    cursor_image: &std::sync::Arc<oblivion_one::cursor_theme::CompositorCursorImage>,
    cursor_render_mode: &mut NativeCursorRenderMode,
    cursor_manager: &mut oblivion_one::cursor_manager::CursorThemeManager,
    perf: NativePerfLogger,
) -> NativeResult<()> {
    let Some(legacy) = legacy_cursor.as_ref() else {
        return Ok(());
    };
    if legacy.matches_image(cursor_image) {
        return Ok(());
    }
    match NativeLegacyHardwareCursor::create(kms.file(), crtc_id, cursor_image) {
        Ok(replacement) => {
            if let Some(mut previous) = legacy_cursor.replace(replacement)
                && previous.active
                && let Err(error) = previous.disable()
            {
                previous.disarm_drm_cleanup();
                *legacy_cursor = None;
                *cursor_render_mode = NativeCursorRenderMode::Software;
                cursor_manager.note_hardware_fallback();
                perf.log("native.cursor", || {
                    vec![
                        NativePerfField::str("event", "legacy_cursor_disable_failed"),
                        NativePerfField::str("error", error.to_string()),
                    ]
                });
                return Ok(());
            }
            *cursor_render_mode = NativeCursorRenderMode::Hardware;
        }
        Err(error) => {
            let _ = legacy_cursor.take();
            *cursor_render_mode = NativeCursorRenderMode::Software;
            cursor_manager.note_hardware_fallback();
            perf.log("native.cursor", || {
                vec![
                    NativePerfField::str("event", "legacy_theme_replace_failed"),
                    NativePerfField::str("error", error.to_string()),
                ]
            });
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn prepare_cursor_image(
    cursor: &mut NativeAtomicCursor,
    theme_image: &std::sync::Arc<oblivion_one::cursor_theme::CompositorCursorImage>,
    theme_generation: u64,
    client_cursor: Option<oblivion_one::compositor::ClientCursorRenderState<'_>>,
    kms: &NativeDrmDevice,
    input_state: &NativeInputState,
    cursor_manager: &mut oblivion_one::cursor_manager::CursorThemeManager,
    perf: NativePerfLogger,
) -> bool {
    if let Some(client) = client_cursor {
        let source_key = NativeCursorImageKey::for_surface_at_output_scale(
            client.surface,
            client.hotspot_x,
            client.hotspot_y,
            cursor.output_scale_milli(),
        );
        let image_ready = if cursor.client_image_matches(source_key) {
            true
        } else if cursor.client_image_failure_matches(source_key) {
            false
        } else if let Some(image) = client_cursor_image(
            client.surface,
            client.hotspot_x,
            client.hotspot_y,
            cursor.output_scale(),
        ) {
            match cursor.replace_image(kms.file(), image, source_key) {
                Ok(()) => true,
                Err(_) => {
                    cursor.note_client_image_failure(source_key);
                    false
                }
            }
        } else {
            cursor.note_client_image_failure(source_key);
            false
        };
        if image_ready {
            let x = oblivion_one::compositor::scale_logical_coordinate(
                client
                    .logical_x
                    .saturating_add(client.surface.x)
                    .saturating_add(client.hotspot_x),
                cursor.output_scale(),
            );
            let y = oblivion_one::compositor::scale_logical_coordinate(
                client
                    .logical_y
                    .saturating_add(client.surface.y)
                    .saturating_add(client.hotspot_y),
                cursor.output_scale(),
            );
            cursor.set_position(x, y);
        }
        image_ready
    } else {
        let mut theme_image_ready = cursor.theme_image_matches(theme_image);
        if !theme_image_ready {
            if let Err(error) =
                cursor.replace_theme_image(kms.file(), theme_image.clone(), theme_generation)
            {
                cursor_manager.note_hardware_fallback();
                perf.log("native.cursor", || {
                    vec![
                        NativePerfField::str("event", "theme_replace_failed"),
                        NativePerfField::str("error", error.to_string()),
                    ]
                });
                theme_image_ready = false;
            } else {
                theme_image_ready = true;
            }
        }
        if theme_image_ready
            && !cursor.using_theme_image()
            && let Err(error) = cursor.restore_theme_image(kms.file())
        {
            theme_image_ready = false;
            perf.log("native.cursor", || {
                vec![
                    NativePerfField::str("event", "theme_restore_failed"),
                    NativePerfField::str("error", error.to_string()),
                ]
            });
        }
        let (x, y) = input_state.cursor_position();
        cursor.set_position(x, y);
        theme_image_ready
    }
}

pub(super) struct CursorPolicyContext<'a> {
    pub(super) cursor: &'a mut NativeAtomicCursor,
    pub(super) cursor_visible: bool,
    pub(super) cursor_image_ready: bool,
    pub(super) output_width: u32,
    pub(super) output_height: u32,
    pub(super) cursor_preference: NativeCursorPreference,
    pub(super) cursor_scheduling_policy: NativeCursorSchedulingPolicy,
    pub(super) presented_primary: Option<PresentedPrimaryAssignment>,
    pub(super) predictive_triple_active: bool,
    pub(super) client_cursor_active: bool,
    pub(super) cursor_render_mode: &'a mut NativeCursorRenderMode,
    pub(super) last_client_cursor_damage: &'a mut Option<NativeClientCursorDamageState>,
}

pub(super) struct RuntimePlanePlan {
    pub(super) decision: PlaneSchedulingDecision,
    pub(super) delta_class: CursorDeltaClass,
    pub(super) desired_hardware_state: Option<AtomicCursorVisualState>,
    pub(super) render_mode: NativeCursorRenderMode,
    pub(super) attachable_primary: Option<AttachablePrimary>,
    pub(super) primary_cursor_presentation: KmsPrimaryCursorPresentation,
}

pub(super) fn plan_primary_cursor_presentation(
    plan: Option<&RuntimePlanePlan>,
) -> KmsPrimaryCursorPresentation {
    plan.map_or(KmsPrimaryCursorPresentation::Preserve, |plan| {
        plan.primary_cursor_presentation
    })
}

pub(super) fn frozen_cursor_plan_for_render(
    delivery: crate::native_output::presentation::plane::PresentedCursorDelivery,
    primary: KmsPrimaryCursorPresentation,
    plan: Option<&RuntimePlanePlan>,
) -> crate::native_output::presentation::plane::FrozenPrimaryCursorPlan {
    crate::native_output::presentation::plane::FrozenPrimaryCursorPlan {
        delivery,
        primary_presentation: match primary {
            KmsPrimaryCursorPresentation::Preserve => {
                crate::native_output::presentation::plane::FrozenPrimaryCursorPresentation::Preserve
            }
            KmsPrimaryCursorPresentation::Promote(state) => {
                crate::native_output::presentation::plane::FrozenPrimaryCursorPresentation::Promote(
                    state,
                )
            }
        },
        cursor_test_policy: match plan
            .map(|plan| plan.decision.test_policy)
            .unwrap_or(KmsCursorTestPolicy::NotApplicable)
        {
            KmsCursorTestPolicy::Required => {
                crate::native_output::presentation::plane::FrozenCursorTestPolicy::Required
            }
            KmsCursorTestPolicy::NotApplicable | KmsCursorTestPolicy::SkipProven => {
                crate::native_output::presentation::plane::FrozenCursorTestPolicy::Skip
            }
        },
    }
}

pub(super) fn freeze_cursor_plane_owner(
    assignment: Option<&CursorPlaneAssignment>,
    cursor: Option<&NativeAtomicCursor>,
) -> NativeResult<Option<FrozenCursorPlaneOwner>> {
    let assignment = assignment.unwrap_or(&CursorPlaneAssignment::Unchanged);
    let needs_owner = !matches!(assignment, CursorPlaneAssignment::Unchanged);
    if !needs_owner {
        return Ok(None);
    }
    let cursor = cursor.ok_or_else(|| io::Error::other("cursor assignment has no cursor state"))?;
    let state = match assignment {
        CursorPlaneAssignment::Atomic { state, .. } => state.as_ref(),
        CursorPlaneAssignment::Disabled | CursorPlaneAssignment::Unchanged => None,
    };
    let pin = state
        .filter(|state| state.framebuffer_id.is_some())
        .map(|state| cursor.pin_framebuffer_for(state))
        .transpose()?;
    Ok(Some(FrozenCursorPlaneOwner {
        revision: cursor.desired_revision(),
        capability_key: state.and_then(|state| cursor.capability_key_for(state)),
        pin,
    }))
}

pub(super) fn freeze_cursor_assignment_for_render(
    effective_cursor: Option<&AtomicCursorVisualState>,
    cursor_epoch: u64,
    cursor: Option<&NativeAtomicCursor>,
) -> NativeResult<(
    Option<CursorPlaneAssignment>,
    Option<FrozenCursorPlaneOwner>,
)> {
    let assignment = effective_cursor.map(|state| CursorPlaneAssignment::Atomic {
        desired_epoch: cursor_epoch,
        state: Some(state.clone()),
    });
    let owner = freeze_cursor_plane_owner(assignment.as_ref(), cursor)?;
    Ok((assignment, owner))
}

pub(super) fn frozen_revision(
    effective_cursor: Option<&AtomicCursorVisualState>,
    cursor: Option<&NativeAtomicCursor>,
) -> Option<CursorRevision> {
    effective_cursor
        .and(cursor)
        .map(NativeAtomicCursor::desired_revision)
}

pub(super) fn freeze_primary_cursor_presentation(
    previous_delivery: crate::native_output::presentation::plane::PresentedCursorDelivery,
    next_delivery: crate::native_output::presentation::plane::PresentedCursorDelivery,
    effective_cursor: Option<&AtomicCursorVisualState>,
    atomic_cursor: Option<&NativeAtomicCursor>,
    cursor_epoch: u64,
) -> KmsPrimaryCursorPresentation {
    let needs_promotion = next_delivery
        == crate::native_output::presentation::plane::PresentedCursorDelivery::Software
        || (previous_delivery
            == crate::native_output::presentation::plane::PresentedCursorDelivery::Software
            && next_delivery
                == crate::native_output::presentation::plane::PresentedCursorDelivery::Hidden);
    if !needs_promotion {
        return KmsPrimaryCursorPresentation::Preserve;
    }
    let mut state = effective_cursor
        .cloned()
        .or_else(|| atomic_cursor.map(NativeAtomicCursor::desired).cloned())
        .unwrap_or_else(|| AtomicCursorVisualState::hidden(1, 1));
    state.visible = false;
    state.framebuffer_id = None;
    let revision = atomic_cursor.map_or_else(
        || {
            CursorRevision::from_legacy_epoch(
                std::num::NonZeroU64::new(cursor_epoch.max(1)).expect("cursor epoch is nonzero"),
            )
        },
        NativeAtomicCursor::desired_revision,
    );
    KmsPrimaryCursorPresentation::Promote(
        crate::native_output::presentation::plane::PresentedCursorState::from_atomic_with_delivery(
            revision,
            crate::native_output::presentation::plane::CursorCoupling::EmbeddedInPrimary,
            next_delivery,
            &state,
        ),
    )
}

pub(super) fn presented_delivery_for_plan(
    plan: Option<&RuntimePlanePlan>,
    hardware_state: &Option<AtomicCursorVisualState>,
) -> crate::native_output::presentation::plane::PresentedCursorDelivery {
    plan.map_or_else(
        || {
            hardware_state.as_ref().map_or(
                crate::native_output::presentation::plane::PresentedCursorDelivery::Hidden,
                |state| {
                    if state.visible {
                        crate::native_output::presentation::plane::PresentedCursorDelivery::Hardware
                    } else {
                        crate::native_output::presentation::plane::PresentedCursorDelivery::Hidden
                    }
                },
            )
        },
        |plan| match plan.decision.delivery {
            CursorDeliveryChoice::Hardware { .. } => {
                crate::native_output::presentation::plane::PresentedCursorDelivery::Hardware
            }
            CursorDeliveryChoice::Software { .. } => {
                crate::native_output::presentation::plane::PresentedCursorDelivery::Software
            }
            CursorDeliveryChoice::Rejected { .. } | CursorDeliveryChoice::Hidden { .. } => {
                crate::native_output::presentation::plane::PresentedCursorDelivery::Hidden
            }
        },
    )
}

pub(super) fn planned_cursor_hardware_usable(
    plan: Option<&RuntimePlanePlan>,
    atomic_cursor: Option<&NativeAtomicCursor>,
    cursor_render_mode: NativeCursorRenderMode,
    cursor_visible: bool,
) -> bool {
    plan.map_or_else(
        || {
            atomic_cursor.is_some_and(|cursor| {
                effective_atomic_cursor_state(cursor, cursor_render_mode, cursor_visible)
                    .hardware_usable()
            })
        },
        |plan| {
            matches!(
                plan.decision.delivery,
                CursorDeliveryChoice::Hardware { .. }
            )
        },
    )
}

pub(super) const fn plan_uses_hardware_cursor(plan: &RuntimePlanePlan) -> bool {
    matches!(
        plan.decision.delivery,
        CursorDeliveryChoice::Hardware { .. }
    )
}

pub(super) fn planned_client_cursor_software_work(
    plan: Option<&RuntimePlanePlan>,
    client_cursor_hardware_usable: bool,
    last_damage: Option<&NativeClientCursorDamageState>,
    current_damage: Option<NativeClientCursorDamageState>,
    client_cursor_active: bool,
) -> bool {
    plan.is_some_and(|plan| {
        plan.decision.pacing_constraint == CursorPacingConstraint::ReactiveDouble
    }) && !client_cursor_hardware_usable
        && last_damage != current_damage.as_ref()
        && (client_cursor_active || last_damage.is_some())
}

pub(super) fn planned_hardware_cursor_work_pending(
    plan: Option<&RuntimePlanePlan>,
    cursor_state_changed: bool,
    atomic_cursor: Option<&NativeAtomicCursor>,
    cursor_render_mode: NativeCursorRenderMode,
) -> bool {
    cursor_state_changed
        && plan.is_none_or(|plan| plan.delta_class != CursorDeltaClass::DeliveryModeTransition)
        && atomic_cursor.is_some_and(|cursor| {
            cursor.current().visible || cursor_render_mode == NativeCursorRenderMode::Hardware
        })
}

pub(super) fn effective_cursor_for_plan(
    plan: Option<&RuntimePlanePlan>,
    atomic_cursor: Option<&NativeAtomicCursor>,
    cursor_render_mode: NativeCursorRenderMode,
    cursor_visible: bool,
) -> Option<AtomicCursorVisualState> {
    plan.and_then(|plan| plan.desired_hardware_state.clone())
        .or_else(|| {
            atomic_cursor.and_then(|cursor| {
                effective_atomic_cursor_state(cursor, cursor_render_mode, cursor_visible)
                    .kms_state()
                    .cloned()
            })
        })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn cursor_damage_states(
    client_cursor: Option<oblivion_one::compositor::ClientCursorRenderState<'_>>,
    output_width: u32,
    output_height: u32,
    cursor_render_mode: NativeCursorRenderMode,
    cursor_visible: bool,
    client_cursor_active: bool,
    input_state: &NativeInputState,
    cursor_image: &oblivion_one::cursor_theme::CompositorCursorImage,
) -> (
    Option<NativeClientCursorDamageState>,
    Option<NativeDamageRect>,
) {
    let client_damage = client_cursor.map(|cursor| {
        NativeClientCursorDamageState::from_cursor(output_width, output_height, cursor)
    });
    let software_damage = (cursor_render_mode == NativeCursorRenderMode::Software
        && cursor_visible
        && !client_cursor_active)
        .then(|| {
            native_theme_cursor_rect(
                output_width,
                output_height,
                input_state.cursor_position(),
                cursor_image,
            )
        })
        .flatten();
    (client_damage, software_damage)
}

pub(super) fn log_client_cursor_path_if_changed(
    last_path: &mut Option<NativeClientCursorPath>,
    client_cursor_active: bool,
    hardware_eligible: bool,
    direct_active: bool,
    client_cursor: Option<oblivion_one::compositor::ClientCursorRenderState<'_>>,
    perf: NativePerfLogger,
) {
    let path = resolve_client_cursor_path(client_cursor_active, hardware_eligible);
    if *last_path != Some(path) {
        *last_path = Some(path);
        log_client_cursor_path(perf, path, hardware_eligible, direct_active, client_cursor);
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn apply_cursor_policy_with_runtime_inputs(
    context: CursorPolicyContext<'_>,
    worker: Option<&crate::native_output::kms_worker::KmsCommitWorkerHandle>,
    output_generation: u64,
    crtc_id: u32,
    scheduled_target: Option<oblivion_one::native::presentation_deadline::PresentationTarget>,
    presented_cursor: crate::native_output::presentation::plane::PresentedCursorState,
    atomic_commit_pending: bool,
    perf: NativePerfLogger,
) -> RuntimePlanePlan {
    let attachable_primary = scheduled_target.and_then(|scheduled_target| {
        worker.and_then(|worker| {
            worker.attachable_primary(output_generation, crtc_id, scheduled_target)
        })
    });
    let validation_base_unchanged =
        !atomic_commit_pending && presented_cursor.kms_equivalent_to(context.cursor.current());
    let previous_delivery = match presented_cursor.delivery {
        crate::native_output::presentation::plane::PresentedCursorDelivery::Hidden => {
            CursorDeliveryMode::Hidden
        }
        crate::native_output::presentation::plane::PresentedCursorDelivery::Hardware => {
            CursorDeliveryMode::Hardware
        }
        crate::native_output::presentation::plane::PresentedCursorDelivery::Software => {
            CursorDeliveryMode::Software
        }
    };
    let plan = apply_cursor_policy(
        context,
        attachable_primary,
        previous_delivery,
        validation_base_unchanged,
    );
    perf.log("native.cursor_plane_policy", || {
        vec![
            NativePerfField::str("reason", format!("{:?}", plan.decision.reason)),
            NativePerfField::str("delta_class", format!("{:?}", plan.delta_class)),
            NativePerfField::str("delivery", format!("{:?}", plan.decision.delivery)),
            NativePerfField::str(
                "cursor_action",
                format!("{:?}", plan.decision.cursor_action),
            ),
            NativePerfField::str(
                "primary_action",
                format!("{:?}", plan.decision.primary_action),
            ),
            NativePerfField::str("pacing", format!("{:?}", plan.decision.pacing_constraint)),
            NativePerfField::str("test_policy", format!("{:?}", plan.decision.test_policy)),
            NativePerfField::bool("direct_compatible", plan.decision.direct_scanout_compatible),
            NativePerfField::u64(
                "attachable_primary",
                plan.attachable_primary
                    .map_or(0, |primary| primary.transaction_id.get()),
            ),
        ]
    });
    plan
}

fn build_runtime_plane_plan(
    mut input: PlaneSchedulingInput<'_>,
    previous_delivery: CursorDeliveryMode,
    previous_state: Option<&AtomicCursorVisualState>,
    next_state: Option<&AtomicCursorVisualState>,
    client_cursor_active: bool,
    attachable_primary: Option<AttachablePrimary>,
    capability_key_unchanged: bool,
) -> RuntimePlanePlan {
    let provisional = {
        input.delta_class = CursorDeltaClass::Visual;
        schedule_planes(input)
    };
    let next_delivery = match provisional.delivery {
        CursorDeliveryChoice::Hardware { .. } => CursorDeliveryMode::Hardware,
        CursorDeliveryChoice::Software { .. } => CursorDeliveryMode::Software,
        CursorDeliveryChoice::Hidden { .. } | CursorDeliveryChoice::Rejected { .. } => {
            CursorDeliveryMode::Hidden
        }
    };
    input.next_delivery = next_delivery;
    let mut delta_class = classify_cursor_delta(
        previous_delivery,
        next_delivery,
        previous_state,
        next_state,
        input.validation_base_unchanged,
    );
    if delta_class == CursorDeltaClass::PositionOnly && !capability_key_unchanged {
        delta_class = CursorDeltaClass::Visual;
    }
    input.delta_class = delta_class;
    let decision = schedule_planes(input);
    let render_mode = match decision.delivery {
        CursorDeliveryChoice::Hardware { .. } => NativeCursorRenderMode::Hardware,
        CursorDeliveryChoice::Software { .. } | CursorDeliveryChoice::Rejected { .. } => {
            if client_cursor_active {
                NativeCursorRenderMode::SoftwareClient
            } else {
                NativeCursorRenderMode::Software
            }
        }
        CursorDeliveryChoice::Hidden { .. } => {
            if client_cursor_active {
                NativeCursorRenderMode::SoftwareClient
            } else {
                NativeCursorRenderMode::Software
            }
        }
    };
    RuntimePlanePlan {
        desired_hardware_state: matches!(decision.delivery, CursorDeliveryChoice::Hardware { .. })
            .then(|| next_state.cloned())
            .flatten(),
        decision,
        delta_class,
        render_mode,
        attachable_primary,
        primary_cursor_presentation: KmsPrimaryCursorPresentation::Preserve,
    }
}

pub(super) fn apply_cursor_policy(
    context: CursorPolicyContext<'_>,
    attachable_primary: Option<AttachablePrimary>,
    previous_delivery: CursorDeliveryMode,
    validation_base_unchanged: bool,
) -> RuntimePlanePlan {
    let CursorPolicyContext {
        cursor,
        cursor_visible,
        cursor_image_ready,
        output_width,
        output_height,
        cursor_preference,
        cursor_scheduling_policy,
        presented_primary,
        predictive_triple_active,
        client_cursor_active,
        cursor_render_mode,
        last_client_cursor_damage,
    } = context;
    let policy_preference = if cursor_scheduling_policy == NativeCursorSchedulingPolicy::Software {
        CursorPreference::Software
    } else {
        match cursor_preference {
            NativeCursorPreference::Auto => CursorPreference::Auto,
            NativeCursorPreference::Hardware => CursorPreference::Hardware,
            NativeCursorPreference::Software => CursorPreference::Software,
        }
    };
    let primary_mode = if presented_primary.is_some_and(|assignment| assignment.is_direct()) {
        PlanePrimaryMode::Direct
    } else {
        PlanePrimaryMode::Composed
    };
    let prospective = AtomicCursorVisualState {
        visible: cursor_visible && cursor_image_ready,
        ..cursor.desired().clone()
    };
    let capability_key = cursor_image_ready
        .then(|| cursor.capability_key_for(&prospective))
        .flatten();
    let previous_capability_key = cursor.capability_key_for(cursor.current());
    let input = PlaneSchedulingInput {
        revision: cursor.desired_revision(),
        preference: policy_preference,
        visible: cursor_visible,
        geometry: CursorGeometryInput {
            pointer_x: prospective.x,
            pointer_y: prospective.y,
            hotspot_x: prospective.hotspot_x,
            hotspot_y: prospective.hotspot_y,
            cursor_width: prospective.width,
            cursor_height: prospective.height,
            output_width,
            output_height,
        },
        geometry_valid: capability_key.is_some(),
        hardware: capability_key.map(|key| CursorHardwareCapability { key }),
        capabilities: cursor.capability_cache(),
        primary_mode,
        software_allowed: true,
        predictive_triple_active,
        cursor_kms_changed: !prospective.kms_equivalent(cursor.current()),
        hardware_plane_visible: cursor.current().visible,
        delta_class: CursorDeltaClass::Visual,
        previous_delivery,
        next_delivery: CursorDeliveryMode::Hidden,
        validation_base_unchanged,
        attachable_primary: attachable_primary.map(|primary| primary.transaction_id),
    };
    let mut plan = build_runtime_plane_plan(
        input,
        previous_delivery,
        Some(cursor.current()),
        Some(&prospective),
        client_cursor_active,
        attachable_primary,
        previous_capability_key == capability_key,
    );
    plan.primary_cursor_presentation = freeze_primary_cursor_presentation(
        match previous_delivery {
            CursorDeliveryMode::Hidden => {
                crate::native_output::presentation::plane::PresentedCursorDelivery::Hidden
            }
            CursorDeliveryMode::Hardware => {
                crate::native_output::presentation::plane::PresentedCursorDelivery::Hardware
            }
            CursorDeliveryMode::Software => {
                crate::native_output::presentation::plane::PresentedCursorDelivery::Software
            }
        },
        presented_delivery_for_plan(Some(&plan), &None),
        Some(&prospective),
        Some(cursor),
        cursor.desired_epoch(),
    );
    cursor.set_scheduled_test_policy(plan.decision.test_policy);
    match plan.decision.delivery {
        CursorDeliveryChoice::Hardware { .. } => {
            cursor.set_visible(true);
            *cursor_render_mode = plan.render_mode;
        }
        CursorDeliveryChoice::Software { .. } => {
            cursor.set_visible(false);
            *cursor_render_mode = plan.render_mode;
            *last_client_cursor_damage = None;
            cursor.note_software_fallback();
        }
        CursorDeliveryChoice::Hidden { .. } | CursorDeliveryChoice::Rejected { .. } => {
            cursor.set_visible(false);
            *cursor_render_mode = plan.render_mode;
        }
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_output::presentation::plane::CursorRevision;
    use crate::native_output::presentation::plane_policy::{
        CursorCapabilityKey, CursorGeometryClass, PlaneCapabilityCache,
    };

    fn key() -> CursorCapabilityKey {
        CursorCapabilityKey {
            output_generation: 1,
            crtc_id: 7,
            plane_id: 8,
            mode_width: 1920,
            mode_height: 1080,
            output_transform: 0,
            output_scale_milli: 1_000,
            format: oblivion_one::native::kms::DRM_FORMAT_ARGB8888,
            modifier: 0,
            cursor_width: 64,
            cursor_height: 64,
            hotspot_property_available: false,
            geometry_class: CursorGeometryClass::FullyVisible,
            source_x: 0,
            source_y: 0,
            source_width: 64 << 16,
            source_height: 64 << 16,
            destination_x: 0,
            destination_y: 0,
            destination_width: 64,
            destination_height: 64,
        }
    }

    #[test]
    fn runtime_adapter_decision_is_the_pure_scheduler_decision() {
        let mut capabilities = PlaneCapabilityCache::default();
        let capability_key = key();
        capabilities.mark_proven(capability_key);
        let mut previous = AtomicCursorVisualState::hidden(64, 64);
        previous.visible = true;
        previous.framebuffer_id = Some(9);
        let mut next = previous.clone();
        next.x = 12;
        let input = PlaneSchedulingInput {
            revision: CursorRevision::initial().advance_motion(),
            preference: CursorPreference::Auto,
            visible: true,
            geometry: CursorGeometryInput {
                pointer_x: next.x,
                pointer_y: next.y,
                hotspot_x: next.hotspot_x,
                hotspot_y: next.hotspot_y,
                cursor_width: next.width,
                cursor_height: next.height,
                output_width: 1920,
                output_height: 1080,
            },
            geometry_valid: true,
            hardware: Some(CursorHardwareCapability {
                key: capability_key,
            }),
            capabilities: &capabilities,
            primary_mode: PlanePrimaryMode::Direct,
            software_allowed: true,
            predictive_triple_active: true,
            cursor_kms_changed: true,
            hardware_plane_visible: true,
            delta_class: CursorDeltaClass::PositionOnly,
            previous_delivery: CursorDeliveryMode::Hardware,
            next_delivery: CursorDeliveryMode::Hardware,
            validation_base_unchanged: true,
            attachable_primary: None,
        };
        let expected = schedule_planes(input);
        let plan = build_runtime_plane_plan(
            input,
            CursorDeliveryMode::Hardware,
            Some(&previous),
            Some(&next),
            false,
            None,
            true,
        );
        assert_eq!(plan.decision, expected);
        assert_eq!(plan.delta_class, CursorDeltaClass::PositionOnly);
        assert_eq!(plan.decision.cursor_action, CursorPlaneAction::Independent);
        assert_eq!(plan.render_mode, NativeCursorRenderMode::Hardware);

        let crop_changed = build_runtime_plane_plan(
            input,
            CursorDeliveryMode::Hardware,
            Some(&previous),
            Some(&next),
            false,
            None,
            false,
        );
        assert_eq!(crop_changed.delta_class, CursorDeltaClass::Visual);
        assert_eq!(
            crop_changed.decision.test_policy,
            KmsCursorTestPolicy::Required
        );
    }
}
