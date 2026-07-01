use super::*;
use crate::native_output::runtime::{
    NativePointerConstraint, NativePointerConstraintBackendAction,
};
use std::sync::Mutex;

static EXTERNAL_COMMAND_ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn native_input_super_space_emits_astrea_spotlight_without_forwarding_space() {
    let _guard = EXTERNAL_COMMAND_ENV_LOCK.lock().unwrap();
    // SAFETY: this test serializes access to the process environment with
    // EXTERNAL_COMMAND_ENV_LOCK.
    unsafe {
        std::env::set_var("OBLIVION_ONE_SPOTLIGHT_COMMAND", "printf spotlight");
    }
    let mut input = NativeInputState::new(320, 200);

    let super_key = input.handle_key_event(KEY_LEFTMETA, 1);
    let space = input.handle_key_event(KEY_SPACE, 1);

    assert!(super_key.keyboard_events.is_empty());
    assert!(space.keyboard_events.is_empty());
    assert_eq!(space.launch_command, None);
    assert_eq!(space.launch_source, None);
    assert_eq!(
        space.shortcut_events,
        vec![AstreaShortcutEvent::pressed(
            "astrea-shell",
            "spotlight_toggle"
        )]
    );

    // SAFETY: guarded by EXTERNAL_COMMAND_ENV_LOCK.
    unsafe {
        std::env::remove_var("OBLIVION_ONE_SPOTLIGHT_COMMAND");
    }
}

#[test]
fn ordinary_motion_does_not_apply_compositor_only_position_update() {
    let mut input = NativeInputState::new(320, 200);
    let effect = input.handle_pointer_motion_delta(24.0, 12.0);

    let compositor_visual_changed = apply_compositor_only_pointer_position(&effect, |_, _| {
        panic!("ordinary forwarded motion must not update position twice")
    });

    assert_eq!(effect.pointer_motion, Some((184.0, 112.0)));
    assert!(!compositor_visual_changed);
}

#[test]
fn native_input_alt_p_requests_session_exit_without_forwarding_p() {
    let mut input = NativeInputState::new(320, 200);

    input.handle_key_event(KEY_LEFTALT, 1);
    let p = input.handle_key_event(KEY_P, 1);

    assert!(p.exit_requested);
    assert!(p.keyboard_events.is_empty());
}

#[test]
fn native_input_alt_tab_sequence_emits_astrea_shortcuts() {
    let mut input = NativeInputState::new(320, 200);

    input.handle_key_event(KEY_LEFTALT, 1);
    let next = input.handle_key_event(KEY_TAB, 1);
    let commit = input.handle_key_event(KEY_LEFTALT, 0);

    assert_eq!(
        next.shortcut_events,
        vec![AstreaShortcutEvent::pressed("astrea-shell", "alt_tab_next")]
    );
    assert!(next.keyboard_events.is_empty());
    assert_eq!(
        commit.shortcut_events,
        vec![AstreaShortcutEvent::pressed(
            "astrea-shell",
            "alt_tab_commit"
        )]
    );
}

#[test]
fn native_input_alt_shift_tab_sequence_emits_previous() {
    let mut input = NativeInputState::new(320, 200);

    input.handle_key_event(KEY_LEFTALT, 1);
    input.handle_key_event(KEY_LEFTSHIFT, 1);
    let previous = input.handle_key_event(KEY_TAB, 1);

    assert_eq!(
        previous.shortcut_events,
        vec![AstreaShortcutEvent::pressed(
            "astrea-shell",
            "alt_tab_previous"
        )]
    );
    assert!(previous.keyboard_events.is_empty());
}

#[test]
fn native_input_ctrl_c_is_forwarded_to_clients() {
    let mut input = NativeInputState::new(320, 200);

    let ctrl = input.handle_key_event(KEY_LEFTCTRL, 1);
    let c = input.handle_key_event(KEY_C, 1);

    assert_eq!(
        ctrl.keyboard_events,
        vec![NativeKeyboardEvent::new(KEY_LEFTCTRL, true)]
    );
    assert!(!c.exit_requested);
    assert_eq!(
        c.keyboard_events,
        vec![NativeKeyboardEvent::new(KEY_C, true)]
    );
}

#[test]
fn native_input_super_mouse_buttons_start_window_interactions() {
    let mut input = NativeInputState::new(320, 200);
    input.handle_key_event(KEY_LEFTMETA, 1);

    let move_start = input.handle_pointer_button(u32::from(BTN_LEFT), true);
    let motion = input.handle_pointer_motion_delta(24.0, 12.0);
    let move_end = input.handle_pointer_button(u32::from(BTN_LEFT), false);

    assert_eq!(
        move_start.window_actions,
        vec![NativeWindowAction::BeginMove { x: 160.0, y: 100.0 }]
    );
    assert!(move_start.pointer_buttons.is_empty());
    assert_eq!(
        motion.window_actions,
        vec![NativeWindowAction::UpdateInteraction { x: 184.0, y: 112.0 }]
    );
    assert!(motion.pointer_motion.is_none());
    assert_eq!(
        move_end.window_actions,
        vec![NativeWindowAction::EndInteraction]
    );
}

#[test]
fn native_input_unlocked_relative_motion_moves_cursor_and_preserves_relative_delta() {
    let mut input = NativeInputState::new(320, 200);

    let effect = input.handle_pointer_motion_delta(24.0, 12.0);

    assert_eq!(input.cursor_position(), (184, 112));
    assert_eq!(
        effect.relative_motion,
        Some(RelativeMotion::accelerated_only(24.0, 12.0))
    );
    assert_eq!(effect.pointer_motion, Some((184.0, 112.0)));
    assert_eq!(effect.cursor_position, Some((184, 112)));
}

#[test]
fn native_input_locked_relative_motion_preserves_delta_without_moving_cursor() {
    let mut input = NativeInputState::new(320, 200);
    input.set_pointer_locked_at(input.cursor_position_f64());

    let effect = input.handle_pointer_motion_delta(24.0, 12.0);

    assert_eq!(input.cursor_position(), (160, 100));
    assert_eq!(
        effect.relative_motion,
        Some(RelativeMotion::accelerated_only(24.0, 12.0))
    );
    assert_eq!(effect.pointer_motion, None);
    assert_eq!(effect.cursor_position, None);
    assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Software));
    assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Hardware));
}

#[test]
fn native_input_locked_absolute_motion_does_not_move_cursor() {
    let mut input = NativeInputState::new(320, 200);
    input.set_pointer_locked_at(input.cursor_position_f64());

    let effect = input.handle_pointer_motion(PointerMotionSample::absolute(7, 25.0, 26.0));

    assert_eq!(input.cursor_position(), (160, 100));
    assert_eq!(effect.pointer_motion, None);
    assert_eq!(effect.cursor_position, None);
}

#[test]
fn locked_pointer_motion_never_accumulates_absolute_cursor_position() {
    let mut input = NativeInputState::new(800, 600);
    let anchor = CompositorOutputPosition {
        x: 400.25,
        y: 300.75,
    };
    input.restore_cursor_position(anchor);
    input.set_pointer_locked_at(anchor);

    let mut total_dx = 0.0;
    let mut total_dy = 0.0;
    for _ in 0..100 {
        let effect = input.handle_pointer_motion(PointerMotionSample::relative(
            10,
            RelativeMotion::accelerated_only(20.0, -15.0),
        ));
        total_dx += effect.relative_motion.unwrap().dx;
        total_dy += effect.relative_motion.unwrap().dy;
        assert_eq!(effect.pointer_motion, None);
        assert_eq!(effect.cursor_position, None);
    }

    assert_eq!(total_dx, 2000.0);
    assert_eq!(total_dy, -1500.0);
    assert_eq!(input.cursor_position_f64(), anchor);
}

#[test]
fn native_input_unlock_restore_sets_logical_cursor_position() {
    let mut input = NativeInputState::new(320, 200);
    input.set_pointer_locked_at(input.cursor_position_f64());
    input.handle_pointer_motion_delta(200.0, 200.0);

    input.clear_pointer_constraint();
    let effect = input.restore_cursor_position(CompositorOutputPosition { x: 35.0, y: 45.0 });

    assert_eq!(input.cursor_position(), (35, 45));
    assert_eq!(effect.cursor_position, Some((35, 45)));
    assert!(effect.requires_frame_repaint(NativeCursorRenderMode::Software));
    assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Hardware));
}

#[test]
fn native_input_first_real_move_after_unlock_starts_from_restored_position() {
    let mut input = NativeInputState::new(320, 200);
    input.set_pointer_locked_at(input.cursor_position_f64());
    input.handle_pointer_motion_delta(200.0, 200.0);
    input.clear_pointer_constraint();
    input.restore_cursor_position(CompositorOutputPosition { x: 35.0, y: 45.0 });

    let effect = input.handle_pointer_motion_delta(5.0, -10.0);

    assert_eq!(input.cursor_position(), (40, 35));
    assert_eq!(effect.pointer_motion, Some((40.0, 35.0)));
}

#[test]
fn native_input_confined_relative_motion_clamps_to_region_and_keeps_absolute_motion() {
    let mut input = NativeInputState::new(320, 200);
    input.restore_cursor_position(CompositorOutputPosition { x: 50.0, y: 50.0 });
    input.set_pointer_confined(OutputRegion::from_rect(
        OutputRect::new(40.0, 30.0, 60.0, 40.0).unwrap(),
    ));

    let right = input.handle_pointer_motion_delta(100.0, 0.0);
    assert_eq!(input.cursor_position(), (99, 50));
    assert_eq!(right.pointer_motion, Some((99.0, 50.0)));
    assert_eq!(
        right.relative_motion,
        Some(RelativeMotion::accelerated_only(100.0, 0.0))
    );

    let bottom = input.handle_pointer_motion_delta(0.0, 100.0);
    assert_eq!(input.cursor_position(), (99, 69));
    assert_eq!(bottom.pointer_motion, Some((99.0, 69.0)));

    let left = input.handle_pointer_motion_delta(-100.0, 0.0);
    assert_eq!(input.cursor_position(), (40, 69));
    assert_eq!(left.pointer_motion, Some((40.0, 69.0)));

    let top = input.handle_pointer_motion_delta(0.0, -100.0);
    assert_eq!(input.cursor_position(), (40, 30));
    assert_eq!(top.pointer_motion, Some((40.0, 30.0)));
}

#[test]
fn native_input_confined_absolute_motion_clamps_and_requests_cursor_repaint() {
    let mut input = NativeInputState::new(320, 200);
    input.set_pointer_confined(OutputRegion::from_rect(
        OutputRect::new(40.0, 30.0, 60.0, 40.0).unwrap(),
    ));

    let effect = input.handle_pointer_motion(PointerMotionSample::absolute(7, 500.0, 1.0));

    assert_eq!(input.cursor_position(), (99, 30));
    assert_eq!(effect.pointer_motion, Some((99.0, 30.0)));
    assert_eq!(effect.cursor_position, Some((99, 30)));
    assert!(effect.requires_frame_repaint(NativeCursorRenderMode::Software));
    assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Hardware));
}

#[test]
fn native_pointer_constraint_backend_activates_locked_once() {
    let mut backend = NativePointerConstraintBackend::new();
    let id = PointerConstraintBackendId {
        constraint_id: 7,
        generation: 1,
    };

    let first = backend.handle_request(
        PointerConstraintBackendRequest::ActivateLocked {
            id,
            anchor: CompositorOutputPosition { x: 10.0, y: 20.0 },
        },
        CompositorOutputPosition { x: 10.0, y: 20.0 },
    );
    let duplicate = backend.handle_request(
        PointerConstraintBackendRequest::ActivateLocked {
            id,
            anchor: CompositorOutputPosition { x: 30.0, y: 40.0 },
        },
        CompositorOutputPosition { x: 30.0, y: 40.0 },
    );

    assert_eq!(
        first.activated,
        Some(NativePointerConstraint {
            id,
            mode: PointerConstraintMode::Locked,
            anchor: CompositorOutputPosition { x: 10.0, y: 20.0 },
            region: None,
        })
    );
    assert_eq!(duplicate, NativePointerConstraintBackendAction::default());
    assert!(backend.active_locked());
}

#[test]
fn native_pointer_constraint_backend_activates_confined_with_region() {
    let mut backend = NativePointerConstraintBackend::new();
    let id = PointerConstraintBackendId {
        constraint_id: 8,
        generation: 1,
    };
    let region = OutputRegion::from_rect(OutputRect::new(10.0, 20.0, 100.0, 50.0).unwrap());

    let action = backend.handle_request(
        PointerConstraintBackendRequest::ActivateConfined {
            id,
            region: region.clone(),
        },
        CompositorOutputPosition { x: 10.0, y: 20.0 },
    );

    assert_eq!(
        action.activated,
        Some(NativePointerConstraint {
            id,
            mode: PointerConstraintMode::Confined,
            anchor: CompositorOutputPosition { x: 10.0, y: 20.0 },
            region: Some(region),
        })
    );
    assert!(action.failed.is_none());
}

#[test]
fn native_pointer_constraint_backend_mismatched_deactivation_cannot_unlock_newer_lock() {
    let mut backend = NativePointerConstraintBackend::new();
    let active = PointerConstraintBackendId {
        constraint_id: 9,
        generation: 2,
    };
    let stale = PointerConstraintBackendId {
        constraint_id: 9,
        generation: 1,
    };
    backend.handle_request(
        PointerConstraintBackendRequest::ActivateLocked {
            id: active,
            anchor: CompositorOutputPosition { x: 10.0, y: 20.0 },
        },
        CompositorOutputPosition { x: 10.0, y: 20.0 },
    );

    let action = backend.handle_request(
        PointerConstraintBackendRequest::Deactivate {
            id: stale,
            restore_position: Some(CompositorOutputPosition { x: 40.0, y: 50.0 }),
        },
        CompositorOutputPosition { x: 99.0, y: 99.0 },
    );

    assert_eq!(action, NativePointerConstraintBackendAction::default());
    assert!(backend.active_locked());
}

#[test]
fn native_pointer_constraint_backend_deactivation_restores_hint_or_anchor() {
    let mut backend = NativePointerConstraintBackend::new();
    let id = PointerConstraintBackendId {
        constraint_id: 10,
        generation: 1,
    };
    backend.handle_request(
        PointerConstraintBackendRequest::ActivateLocked {
            id,
            anchor: CompositorOutputPosition { x: 10.0, y: 20.0 },
        },
        CompositorOutputPosition { x: 10.0, y: 20.0 },
    );

    let action = backend.handle_request(
        PointerConstraintBackendRequest::Deactivate {
            id,
            restore_position: Some(CompositorOutputPosition { x: 30.0, y: 40.0 }),
        },
        CompositorOutputPosition { x: 99.0, y: 99.0 },
    );

    assert_eq!(action.deactivated, Some(id));
    assert_eq!(
        action.restore_position,
        Some(CompositorOutputPosition { x: 30.0, y: 40.0 })
    );
    assert!(!backend.active_locked());

    backend.handle_request(
        PointerConstraintBackendRequest::ActivateLocked {
            id,
            anchor: CompositorOutputPosition { x: 10.0, y: 20.0 },
        },
        CompositorOutputPosition { x: 10.0, y: 20.0 },
    );
    let action = backend.handle_request(
        PointerConstraintBackendRequest::Deactivate {
            id,
            restore_position: None,
        },
        CompositorOutputPosition { x: 99.0, y: 99.0 },
    );

    assert_eq!(
        action.restore_position,
        Some(CompositorOutputPosition { x: 10.0, y: 20.0 })
    );
}

#[test]
fn native_pointer_constraint_backend_preserves_fractional_activation_anchor() {
    let mut backend = NativePointerConstraintBackend::new();
    let id = PointerConstraintBackendId {
        constraint_id: 12,
        generation: 3,
    };
    let anchor = CompositorOutputPosition {
        x: 400.25,
        y: 300.75,
    };

    backend.handle_request(
        PointerConstraintBackendRequest::ActivateLocked { id, anchor },
        anchor,
    );
    let action = backend.handle_request(
        PointerConstraintBackendRequest::Deactivate {
            id,
            restore_position: None,
        },
        CompositorOutputPosition { x: 0.0, y: 0.0 },
    );

    assert_eq!(action.restore_position, Some(anchor));
}

#[test]
fn native_pointer_constraint_backend_confined_deactivation_does_not_restore_anchor() {
    let mut backend = NativePointerConstraintBackend::new();
    let id = PointerConstraintBackendId {
        constraint_id: 11,
        generation: 1,
    };
    backend.handle_request(
        PointerConstraintBackendRequest::ActivateConfined {
            id,
            region: OutputRegion::from_rect(OutputRect::new(10.0, 20.0, 100.0, 50.0).unwrap()),
        },
        CompositorOutputPosition { x: 30.0, y: 40.0 },
    );

    let action = backend.handle_request(
        PointerConstraintBackendRequest::Deactivate {
            id,
            restore_position: None,
        },
        CompositorOutputPosition { x: 99.0, y: 99.0 },
    );

    assert_eq!(action.deactivated, Some(id));
    assert_eq!(action.restore_position, None);
    assert!(!backend.active_locked());
}

#[test]
fn native_pointer_constraint_backend_updates_confined_region_in_place() {
    let mut backend = NativePointerConstraintBackend::new();
    let id = PointerConstraintBackendId {
        constraint_id: 12,
        generation: 1,
    };
    backend.handle_request(
        PointerConstraintBackendRequest::ActivateConfined {
            id,
            region: OutputRegion::from_rect(OutputRect::new(10.0, 20.0, 100.0, 50.0).unwrap()),
        },
        CompositorOutputPosition { x: 30.0, y: 40.0 },
    );

    let action = backend.handle_request(
        PointerConstraintBackendRequest::UpdateConfinedRegion {
            id,
            region: OutputRegion::from_rect(OutputRect::new(40.0, 50.0, 20.0, 10.0).unwrap()),
        },
        CompositorOutputPosition { x: 30.0, y: 40.0 },
    );

    assert_eq!(action.deactivated, None);
    assert_eq!(action.activated, None);
    assert_eq!(
        action.cursor_position,
        Some(CompositorOutputPosition { x: 40.0, y: 50.0 })
    );
    assert_eq!(action.restore_position, None);
    assert_eq!(
        backend.active_constraint_state(),
        NativePointerConstraintState::Confined {
            region: OutputRegion::from_rect(OutputRect::new(40.0, 50.0, 20.0, 10.0).unwrap())
        }
    );
}

#[test]
fn native_pointer_constraint_backend_tracks_cursor_visibility_changes() {
    let mut backend = NativePointerConstraintBackend::new();

    let hide = backend.handle_request(
        PointerConstraintBackendRequest::ApplyCursorVisibility { visible: false },
        CompositorOutputPosition::default(),
    );
    let duplicate_hide = backend.handle_request(
        PointerConstraintBackendRequest::ApplyCursorVisibility { visible: false },
        CompositorOutputPosition::default(),
    );
    let show = backend.handle_request(
        PointerConstraintBackendRequest::ApplyCursorVisibility { visible: true },
        CompositorOutputPosition::default(),
    );

    assert_eq!(hide.cursor_visibility_changed, Some(false));
    assert_eq!(
        duplicate_hide,
        NativePointerConstraintBackendAction::default()
    );
    assert_eq!(show.cursor_visibility_changed, Some(true));
}

#[test]
fn native_input_super_right_starts_window_resize() {
    let mut input = NativeInputState::new(320, 200);
    input.handle_key_event(KEY_RIGHTMETA, 1);

    let effect = input.handle_pointer_button(u32::from(BTN_RIGHT), true);

    assert_eq!(
        effect.window_actions,
        vec![NativeWindowAction::BeginResize { x: 160.0, y: 100.0 }]
    );
    assert!(effect.pointer_buttons.is_empty());
}

#[test]
fn native_input_super_release_does_not_end_active_window_interaction() {
    let mut input = NativeInputState::new(320, 200);
    input.handle_key_event(KEY_LEFTMETA, 1);
    input.handle_pointer_button(u32::from(BTN_LEFT), true);

    let effect = input.handle_key_event(KEY_LEFTMETA, 0);

    assert!(effect.window_actions.is_empty());
}

#[test]
fn native_input_astrea_keyboard_shortcuts_map_to_actions_and_events() {
    let mut input = NativeInputState::new(320, 200);
    input.handle_key_event(KEY_LEFTMETA, 1);

    let launch = input.handle_key_event(KEY_Q, 1);
    let close = input.handle_key_event(KEY_C, 1);
    let fullscreen = input.handle_key_event(KEY_F, 1);

    assert_eq!(launch.launch_command, Some(vec!["kitty".to_string()]));
    assert!(launch.keyboard_events.is_empty());
    assert_eq!(
        close.window_actions,
        vec![NativeWindowAction::CloseActiveWindow]
    );
    assert_eq!(
        fullscreen.window_actions,
        vec![NativeWindowAction::ToggleFullscreen]
    );
}

#[test]
fn native_input_shortcut_inhibition_forwards_window_shortcuts_to_client() {
    let mut input = NativeInputState::new(320, 200);
    input.set_keyboard_shortcuts_inhibited(true);

    let alt = input.handle_key_event(KEY_LEFTALT, 1);
    let fullscreen = input.handle_key_event(KEY_F11, 1);

    assert_eq!(
        alt.keyboard_events,
        vec![NativeKeyboardEvent::new(KEY_LEFTALT, true)]
    );
    assert!(alt.window_actions.is_empty());
    assert_eq!(
        fullscreen.keyboard_events,
        vec![NativeKeyboardEvent::new(KEY_F11, true)]
    );
    assert!(fullscreen.window_actions.is_empty());
}

#[test]
fn native_input_shortcut_inhibition_keeps_emergency_exit_shortcut() {
    let mut input = NativeInputState::new(320, 200);
    input.set_keyboard_shortcuts_inhibited(true);
    input.handle_key_event(KEY_LEFTALT, 1);

    let effect = input.handle_key_event(KEY_P, 1);

    assert!(effect.exit_requested);
    assert!(effect.keyboard_events.is_empty());
}

#[test]
fn native_input_backend_plan_prefers_libseat_libinput_when_available() {
    let plan = NativeInputBackendPlan::choose(NativeInputBackendChoice {
        preference: NativeInputBackendPreference::Auto,
        libseat_available: true,
        libinput_available: true,
        raw_evdev_available: true,
    });

    assert_eq!(plan.primary, NativeInputBackendKind::LibseatLibinputUdev);
    assert_eq!(
        plan.fallbacks,
        vec![
            NativeInputBackendKind::DirectLibinputUdev,
            NativeInputBackendKind::RawEvdev,
        ]
    );
}

#[test]
fn native_input_backend_plan_uses_direct_libinput_without_libseat() {
    let plan = NativeInputBackendPlan::choose(NativeInputBackendChoice {
        preference: NativeInputBackendPreference::Auto,
        libseat_available: false,
        libinput_available: true,
        raw_evdev_available: true,
    });

    assert_eq!(plan.primary, NativeInputBackendKind::DirectLibinputUdev);
    assert_eq!(plan.fallbacks, vec![NativeInputBackendKind::RawEvdev]);
}

#[test]
fn native_input_backend_plan_can_force_raw_evdev_for_debugging() {
    let plan = NativeInputBackendPlan::choose(NativeInputBackendChoice {
        preference: NativeInputBackendPreference::RawEvdev,
        libseat_available: true,
        libinput_available: true,
        raw_evdev_available: true,
    });

    assert_eq!(plan.primary, NativeInputBackendKind::RawEvdev);
    assert!(plan.fallbacks.is_empty());
}

#[test]
fn native_input_backend_plan_falls_back_to_raw_when_libinput_is_unavailable() {
    let plan = NativeInputBackendPlan::choose(NativeInputBackendChoice {
        preference: NativeInputBackendPreference::Auto,
        libseat_available: true,
        libinput_available: false,
        raw_evdev_available: true,
    });

    assert_eq!(plan.primary, NativeInputBackendKind::RawEvdev);
    assert!(plan.fallbacks.is_empty());
}

#[test]

fn native_seat_lifecycle_requests_suspend_then_resume() {
    let mut lifecycle = NativeSeatLifecycle::default();

    assert_eq!(
        lifecycle.apply_event(NativeSeatEvent::Disabled),
        Some(NativeSeatInputAction::Suspend)
    );
    assert_eq!(
        lifecycle.apply_event(NativeSeatEvent::Enabled),
        Some(NativeSeatInputAction::Resume)
    );
}

#[test]
fn native_input_state_handles_normalized_relative_motion() {
    let mut input = NativeInputState::new(320, 200);

    let effect = input.handle_hardware_input_event(NativeHardwareInputEvent::PointerMotion(
        PointerMotionSample::relative(10, RelativeMotion::accelerated_only(12.0, -4.0)),
    ));

    assert_eq!(effect.pointer_motion, Some((172.0, 96.0)));
    assert!(effect.redraw_requested);
}

#[test]
fn native_input_pointer_motion_can_skip_frame_repaint_with_hardware_cursor() {
    let mut input = NativeInputState::new(320, 200);

    let effect = input.handle_hardware_input_event(NativeHardwareInputEvent::PointerMotion(
        PointerMotionSample::relative(10, RelativeMotion::accelerated_only(12.0, -4.0)),
    ));

    assert_eq!(effect.pointer_motion, Some((172.0, 96.0)));
    assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Hardware));
    assert!(effect.requires_frame_repaint(NativeCursorRenderMode::Software));
}

#[test]
fn native_forwarded_keyboard_input_skips_frame_repaint_without_local_visual_change() {
    let mut input = NativeInputState::new(320, 200);

    let effect = input.handle_key_event(KEY_Z, 1);

    assert_eq!(
        effect.keyboard_events,
        vec![NativeKeyboardEvent::new(KEY_Z, true)]
    );
    assert!(effect.redraw_requested);
    assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Hardware));
    assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Software));
}

#[test]
fn native_forwarded_pointer_button_skips_frame_repaint_without_local_visual_change() {
    let mut input = NativeInputState::new(320, 200);

    let effect = input.handle_pointer_button(u32::from(BTN_LEFT), true);

    assert_eq!(effect.pointer_buttons.len(), 1);
    assert!(effect.redraw_requested);
    assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Hardware));
    assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Software));
}

#[test]
fn native_forwarded_pointer_axis_skips_frame_repaint_without_local_visual_change() {
    let mut input = NativeInputState::new(320, 200);

    let effect = input.handle_pointer_axis(0.0, 120.0);

    assert_eq!(effect.pointer_axis, Some((0.0, 120.0)));
    assert!(effect.redraw_requested);
    assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Hardware));
    assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Software));
}

#[test]
fn native_input_window_interaction_still_repaints_with_hardware_cursor() {
    let mut input = NativeInputState::new(320, 200);
    input.handle_key_event(KEY_LEFTMETA, 1);
    input.handle_pointer_button(u32::from(BTN_LEFT), true);

    let effect = input.handle_hardware_input_event(NativeHardwareInputEvent::PointerMotion(
        PointerMotionSample::relative(10, RelativeMotion::accelerated_only(12.0, -4.0)),
    ));

    assert_eq!(
        effect.window_actions,
        vec![NativeWindowAction::UpdateInteraction { x: 172.0, y: 96.0 }]
    );
    assert!(effect.requires_frame_repaint(NativeCursorRenderMode::Hardware));
}

#[test]
fn native_libinput_scroll_axis_value_skips_absent_axis_reader() {
    let value = libinput_scroll_axis_value(false, || panic!("axis value should not be read"));

    assert_eq!(value, 0.0);
}
