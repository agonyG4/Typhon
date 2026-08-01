use super::*;
use crate::native_output::kms_worker::AttachablePrimary;

pub(super) fn prepare_cursor_image(
    cursor: &mut NativeAtomicCursor,
    client_cursor: Option<oblivion_one::compositor::ClientCursorRenderState<'_>>,
    kms: &NativeDrmDevice,
    input_state: &NativeInputState,
    perf: NativePerfLogger,
) -> bool {
    if let Some(client) = client_cursor {
        let source_key =
            NativeCursorImageKey::for_surface(client.surface, client.hotspot_x, client.hotspot_y);
        let image_ready = if cursor.client_image_matches(source_key) {
            true
        } else if cursor.client_image_failure_matches(source_key) {
            false
        } else if let Some(image) =
            client_cursor_image(client.surface, client.hotspot_x, client.hotspot_y)
        {
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
            let x = client
                .logical_x
                .saturating_add(client.surface.x)
                .saturating_add(client.hotspot_x);
            let y = client
                .logical_y
                .saturating_add(client.surface.y)
                .saturating_add(client.hotspot_y);
            cursor.set_position(x, y);
        }
        image_ready
    } else {
        let mut theme_image_ready = cursor.using_theme_image();
        if !cursor.using_theme_image()
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
    pub(super) confirmed_primary_assignment: Option<ConfirmedPrimaryAssignment>,
    pub(super) predictive_triple_active: bool,
    pub(super) client_cursor_active: bool,
    pub(super) cursor_render_mode: &'a mut NativeCursorRenderMode,
    pub(super) last_client_cursor_damage: &'a mut Option<NativeClientCursorDamageState>,
    pub(super) attachable_primary: Option<AttachablePrimary>,
    pub(super) validation_base_unchanged: bool,
}

pub(super) struct RuntimePlanePlan {
    pub(super) decision: PlaneSchedulingDecision,
    pub(super) delta_class: CursorDeltaClass,
    pub(super) desired_hardware_state: Option<AtomicCursorVisualState>,
    pub(super) render_mode: NativeCursorRenderMode,
    pub(super) attachable_primary: Option<AttachablePrimary>,
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
        CursorDeliveryChoice::Software { .. } | CursorDeliveryChoice::Rejected { .. } => {
            CursorDeliveryMode::Software
        }
        CursorDeliveryChoice::Hidden { .. } => CursorDeliveryMode::Hidden,
    };
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
    }
}

pub(super) fn apply_cursor_policy(context: CursorPolicyContext<'_>) -> RuntimePlanePlan {
    let CursorPolicyContext {
        cursor,
        cursor_visible,
        cursor_image_ready,
        output_width,
        output_height,
        cursor_preference,
        cursor_scheduling_policy,
        confirmed_primary_assignment,
        predictive_triple_active,
        client_cursor_active,
        cursor_render_mode,
        last_client_cursor_damage,
        attachable_primary,
        validation_base_unchanged,
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
    let primary_mode =
        if confirmed_primary_assignment.is_some_and(|assignment| assignment.is_direct()) {
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
        validation_base_unchanged,
        attachable_primary: attachable_primary.map(|primary| primary.transaction_id),
    };
    let previous_delivery = if cursor.current().visible {
        CursorDeliveryMode::Hardware
    } else if cursor_render_mode.is_software() {
        CursorDeliveryMode::Software
    } else {
        CursorDeliveryMode::Hidden
    };
    let plan = build_runtime_plane_plan(
        input,
        previous_delivery,
        Some(cursor.current()),
        Some(&prospective),
        client_cursor_active,
        attachable_primary,
        previous_capability_key == capability_key,
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
