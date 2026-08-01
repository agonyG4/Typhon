use super::*;

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
}

pub(super) fn apply_cursor_policy(context: CursorPolicyContext<'_>) -> bool {
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
    let transition_primary = confirmed_primary_assignment.and_then(|assignment| match assignment {
        ConfirmedPrimaryState::Composed { .. } => None,
        ConfirmedPrimaryState::Direct { transaction_id, .. } => Some(transaction_id),
    });
    let prospective = AtomicCursorVisualState {
        visible: cursor_visible && cursor_image_ready,
        ..cursor.desired().clone()
    };
    let capability_key = cursor_image_ready
        .then(|| cursor.capability_key_for(&prospective))
        .flatten();
    let cursor_policy = schedule_planes(PlaneSchedulingInput {
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
        transition_primary,
    });
    let cursor_delta_class = classify_cursor_delta(
        cursor.current(),
        matches!(
            cursor_policy.delivery,
            CursorDeliveryChoice::Hardware { .. }
        )
        .then_some(&prospective),
        matches!(
            cursor_policy.delivery,
            CursorDeliveryChoice::Software { .. } | CursorDeliveryChoice::Rejected { .. }
        ),
    );
    cursor.set_scheduled_test_policy(if cursor_delta_class == CursorDeltaClass::Visual {
        KmsCursorTestPolicy::Required
    } else {
        cursor_policy.test_policy
    });
    match cursor_policy.delivery {
        CursorDeliveryChoice::Hardware { .. } => {
            cursor.set_visible(true);
            *cursor_render_mode = NativeCursorRenderMode::Hardware;
            client_cursor_active
        }
        CursorDeliveryChoice::Software { .. } => {
            cursor.set_visible(false);
            *cursor_render_mode = if client_cursor_active {
                NativeCursorRenderMode::SoftwareClient
            } else {
                NativeCursorRenderMode::Software
            };
            *last_client_cursor_damage = None;
            cursor.note_software_fallback();
            false
        }
        CursorDeliveryChoice::Hidden { .. } | CursorDeliveryChoice::Rejected { .. } => {
            cursor.set_visible(false);
            *cursor_render_mode = if client_cursor_active {
                NativeCursorRenderMode::SoftwareClient
            } else {
                NativeCursorRenderMode::Software
            };
            false
        }
    }
}
