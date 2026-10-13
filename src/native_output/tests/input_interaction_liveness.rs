use super::input::{pump_native_input_server_until, spawn_native_input_resize_client};
use super::input_protocol::{ClientCommand, ClientEvent};
use super::input_xwayland_client::spawn_native_input_xwayland_client;
use super::*;
use crate::native_output::runtime::{
    NativePointerConstraintBackend, NativeResolvedCursorSource, resolve_native_cursor_for_server,
};
use oblivion_one::compositor::{
    DesktopWindowKind, OutputPosition, PointerMotionSample, RelativePointerMotion,
    WindowConstraints, WindowMetadata,
};
use oblivion_one::xwayland::xwm::{
    X11Geometry, X11MoveResizeDirection, X11MoveResizeRequest, X11PublishedState,
    X11WindowSnapshot, X11WindowTypes, XwmEvent,
};
use oblivion_one::xwayland::{X11WindowHandle, XwaylandGeneration};
use std::num::NonZeroU64;
use std::os::unix::net::UnixStream;

#[test]
fn physical_pointer_button_state_updates_before_binding_consumption() {
    let mut input = NativeInputState::new(320, 200);
    input.handle_key_event(KEY_RIGHTMETA, 1);

    let press = input.handle_pointer_button(u32::from(BTN_RIGHT), true);

    assert!(input.is_pointer_button_pressed(u32::from(BTN_RIGHT)));
    assert_eq!(
        press.window_actions,
        vec![NativeWindowAction::BeginResize {
            x: 160.0,
            y: 100.0,
            trigger_button: Some(u32::from(BTN_RIGHT)),
        }]
    );

    let release = input.handle_pointer_button(u32::from(BTN_RIGHT), false);

    assert!(!input.is_pointer_button_pressed(u32::from(BTN_RIGHT)));
    assert_eq!(
        release.pointer_buttons,
        vec![NativePointerButtonEvent::new_at_with_delivery(
            u32::from(BTN_RIGHT),
            false,
            160.0,
            100.0,
            320,
            200,
            NativePointerButtonDelivery::Compositor,
        )]
    );
}

#[test]
fn physical_pointer_button_state_deduplicates_repeated_transitions() {
    let mut input = NativeInputState::new(320, 200);
    let button = u32::from(BTN_MIDDLE);

    input.handle_pointer_button(button, true);
    input.handle_pointer_button(button, true);
    assert_eq!(input.pressed_pointer_buttons_snapshot(), vec![button]);

    input.handle_pointer_button(button, false);
    input.handle_pointer_button(button, false);
    assert!(input.pressed_pointer_buttons_snapshot().is_empty());
}

#[test]
fn pointer_delivery_owner_survives_modifier_and_inhibition_changes() {
    let button = u32::from(BTN_RIGHT);
    let mut compositor_owned = NativeInputState::new(320, 200);
    compositor_owned.handle_key_event(KEY_LEFTMETA, 1);
    let press = compositor_owned.handle_pointer_button(button, true);
    assert!(press.pointer_buttons.is_empty());

    compositor_owned.handle_key_event(KEY_LEFTMETA, 0);
    compositor_owned
        .reconcile_keyboard_shortcut_inhibition(KeyboardShortcutInhibitionSnapshot::new(true, 1));
    let release = compositor_owned.handle_pointer_button(button, false);
    assert_eq!(release.pointer_buttons.len(), 1);
    assert_eq!(
        release.pointer_buttons[0].delivery,
        NativePointerButtonDelivery::Compositor
    );

    let mut client_owned = NativeInputState::new(320, 200);
    client_owned
        .reconcile_keyboard_shortcut_inhibition(KeyboardShortcutInhibitionSnapshot::new(true, 1));
    client_owned.handle_key_event(KEY_LEFTMETA, 1);
    let press = client_owned.handle_pointer_button(button, true);
    assert_eq!(press.pointer_buttons.len(), 1);
    assert_eq!(
        press.pointer_buttons[0].delivery,
        NativePointerButtonDelivery::Client
    );

    client_owned
        .reconcile_keyboard_shortcut_inhibition(KeyboardShortcutInhibitionSnapshot::new(false, 2));
    client_owned.handle_key_event(KEY_LEFTMETA, 0);
    let release = client_owned.handle_pointer_button(button, false);
    assert_eq!(release.pointer_buttons.len(), 1);
    assert_eq!(
        release.pointer_buttons[0].delivery,
        NativePointerButtonDelivery::Client
    );
}

#[test]
fn pointer_delivery_owners_are_independent_and_duplicate_press_does_not_retag() {
    let left = u32::from(BTN_LEFT);
    let middle = u32::from(BTN_MIDDLE);
    let mut input = NativeInputState::new(320, 200);
    input.handle_key_event(KEY_LEFTMETA, 1);

    let left_press = input.handle_pointer_button(left, true);
    assert!(left_press.pointer_buttons.is_empty());
    let middle_press = input.handle_pointer_button(middle, true);
    assert_eq!(middle_press.pointer_buttons.len(), 1);
    assert_eq!(
        middle_press.pointer_buttons[0].delivery,
        NativePointerButtonDelivery::Client
    );

    input.binding_manager = AstreaBindingManager::with_specs(vec![BindingSpec {
        modifiers: ModifierMask::SUPER,
        trigger: BindingTrigger::PointerPress,
        input: BindingInput::PointerButton(middle),
        action: BindingActionDefinition::EmitShortcut {
            namespace: "test".to_string(),
            name: "duplicate_middle_press".to_string(),
        },
        repeat: RepeatPolicy::Disabled,
        inhibition: InhibitionPolicy::Respect,
        reserved: false,
    }]);
    let duplicate_middle_press = input.handle_pointer_button(middle, true);
    assert_eq!(duplicate_middle_press.binding_action_invocations.len(), 1);
    assert_eq!(duplicate_middle_press.pointer_buttons.len(), 1);
    assert_eq!(
        duplicate_middle_press.pointer_buttons[0].delivery,
        NativePointerButtonDelivery::Client
    );

    input.handle_key_event(KEY_LEFTMETA, 0);
    let left_release = input.handle_pointer_button(left, false);
    let middle_release = input.handle_pointer_button(middle, false);
    assert_eq!(
        left_release.pointer_buttons[0].delivery,
        NativePointerButtonDelivery::Compositor
    );
    assert_eq!(
        middle_release.pointer_buttons[0].delivery,
        NativePointerButtonDelivery::Client
    );
    assert!(input.pressed_pointer_buttons_snapshot().is_empty());
}

#[test]
fn session_clear_retires_client_and_compositor_pointer_sequence_owners() {
    let left = u32::from(BTN_LEFT);
    let middle = u32::from(BTN_MIDDLE);
    let mut input = NativeInputState::new(320, 200);
    input.handle_key_event(KEY_LEFTMETA, 1);
    assert!(
        input
            .handle_pointer_button(left, true)
            .pointer_buttons
            .is_empty()
    );
    assert_eq!(
        input
            .handle_pointer_button(middle, true)
            .pointer_buttons
            .len(),
        1
    );

    input.clear_pressed_state_for_session_switch();
    assert!(!input.is_pointer_button_pressed(left));
    assert!(!input.is_pointer_button_pressed(middle));
    assert!(input.pressed_pointer_buttons_snapshot().is_empty());

    let new_left_press = input.handle_pointer_button(left, true);
    assert_eq!(new_left_press.pointer_buttons.len(), 1);
    assert_eq!(
        new_left_press.pointer_buttons[0].delivery,
        NativePointerButtonDelivery::Client
    );
    let new_left_release = input.handle_pointer_button(left, false);
    assert_eq!(new_left_release.pointer_buttons.len(), 1);
    assert_eq!(
        new_left_release.pointer_buttons[0].delivery,
        NativePointerButtonDelivery::Client
    );

    input.binding_manager = AstreaBindingManager::with_specs(vec![BindingSpec {
        modifiers: ModifierMask::EMPTY,
        trigger: BindingTrigger::PointerPress,
        input: BindingInput::PointerButton(middle),
        action: BindingActionDefinition::EmitShortcut {
            namespace: "test".to_string(),
            name: "new_middle_press".to_string(),
        },
        repeat: RepeatPolicy::Disabled,
        inhibition: InhibitionPolicy::Respect,
        reserved: false,
    }]);
    let new_middle_press = input.handle_pointer_button(middle, true);
    assert!(new_middle_press.pointer_buttons.is_empty());
    let middle_release = input.handle_pointer_button(middle, false);
    assert_eq!(middle_release.pointer_buttons.len(), 1);
    assert_eq!(
        middle_release.pointer_buttons[0].delivery,
        NativePointerButtonDelivery::Compositor
    );
}

#[test]
fn pointer_release_binding_does_not_steal_client_owned_release() {
    let socket_name = format!("typhon-native-input-release-binding-{}", std::process::id());
    let socket_path =
        PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap()).join(&socket_name);
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (client_commands, client_events) = spawn_native_input_resize_client(socket_path);

    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::ReadyForPointer
    ));
    server.send_pointer_motion(92.0, 86.0);
    client_commands.send(ClientCommand::SetCursor).unwrap();
    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::CursorReady { .. }
    ));

    let button = u32::from(BTN_LEFT);
    let mut input = NativeInputState::new(320, 200);
    input.handle_pointer_motion_delta(-68.0, -14.0);
    let press = input.handle_pointer_button(button, true);
    assert_eq!(
        press.pointer_buttons,
        vec![NativePointerButtonEvent::new_at(
            button, true, 92.0, 86.0, 320, 200,
        )]
    );
    let mut resize_perf = NativeResizePerfState::default();
    let mut process_supervisor = ChildSupervisor::new();
    apply_native_input_effect(
        press,
        NativeInputApplyContext {
            binding_action_catalog: &BindingActionCatalog::empty(),
            server: &mut server,
            perf: NativePerfLogger::from_env(),
            resize_perf: &mut resize_perf,
            cursor_mode: NativeCursorRenderMode::Software,
            app_gpu_policy: EffectiveCompositorAppGpuPolicy::CpuOnly,
            process_supervisor: &mut process_supervisor,
            xwayland: None,
        },
    )
    .unwrap();

    input.binding_manager = AstreaBindingManager::with_specs(vec![BindingSpec {
        modifiers: ModifierMask::EMPTY,
        trigger: BindingTrigger::PointerRelease,
        input: BindingInput::PointerButton(button),
        action: BindingActionDefinition::EmitShortcut {
            namespace: "test".to_string(),
            name: "client_owned_release".to_string(),
        },
        repeat: RepeatPolicy::Disabled,
        inhibition: InhibitionPolicy::Respect,
        reserved: false,
    }]);

    let release = input.handle_pointer_button(button, false);

    assert_eq!(release.binding_action_invocations.len(), 1);
    assert_eq!(
        release.pointer_buttons,
        vec![NativePointerButtonEvent::new_at(
            button, false, 92.0, 86.0, 320, 200,
        )]
    );
    apply_native_input_effect(
        release,
        NativeInputApplyContext {
            binding_action_catalog: input.binding_manager.action_catalog(),
            server: &mut server,
            perf: NativePerfLogger::from_env(),
            resize_perf: &mut resize_perf,
            cursor_mode: NativeCursorRenderMode::Software,
            app_gpu_policy: EffectiveCompositorAppGpuPolicy::CpuOnly,
            process_supervisor: &mut process_supervisor,
            xwayland: None,
        },
    )
    .unwrap();

    client_commands.send(ClientCommand::CaptureButtons).unwrap();
    assert_eq!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::Buttons {
            pressed_count: 1,
            released_count: 1,
        }
    );
    client_commands.send(ClientCommand::Finish).unwrap();
    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::Finished { .. }
    ));
}

#[test]
fn compositor_owned_pointer_release_binding_keeps_release_in_effect() {
    let socket_name = format!(
        "typhon-native-input-compositor-release-binding-{}",
        std::process::id()
    );
    let socket_path =
        PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap()).join(&socket_name);
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (client_commands, client_events) = spawn_native_input_resize_client(socket_path);
    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::ReadyForPointer
    ));
    server.send_pointer_motion(92.0, 86.0);
    client_commands.send(ClientCommand::SetCursor).unwrap();
    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::CursorReady { .. }
    ));

    let button = u32::from(BTN_LEFT);
    let mut input = NativeInputState::new(320, 200);
    input.binding_manager = AstreaBindingManager::with_specs(vec![
        BindingSpec {
            modifiers: ModifierMask::EMPTY,
            trigger: BindingTrigger::PointerPress,
            input: BindingInput::PointerButton(button),
            action: BindingActionDefinition::EmitShortcut {
                namespace: "test".to_string(),
                name: "compositor_press".to_string(),
            },
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Respect,
            reserved: false,
        },
        BindingSpec {
            modifiers: ModifierMask::EMPTY,
            trigger: BindingTrigger::PointerRelease,
            input: BindingInput::PointerButton(button),
            action: BindingActionDefinition::EmitShortcut {
                namespace: "test".to_string(),
                name: "compositor_release".to_string(),
            },
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Respect,
            reserved: false,
        },
    ]);

    let press = input.handle_pointer_button(button, true);
    assert!(press.pointer_buttons.is_empty());
    assert_eq!(press.binding_action_invocations.len(), 1);
    let mut resize_perf = NativeResizePerfState::default();
    let mut process_supervisor = ChildSupervisor::new();
    apply_native_input_effect(
        press,
        NativeInputApplyContext {
            binding_action_catalog: input.binding_manager.action_catalog(),
            server: &mut server,
            perf: NativePerfLogger::from_env(),
            resize_perf: &mut resize_perf,
            cursor_mode: NativeCursorRenderMode::Software,
            app_gpu_policy: EffectiveCompositorAppGpuPolicy::CpuOnly,
            process_supervisor: &mut process_supervisor,
            xwayland: None,
        },
    )
    .unwrap();

    let release = input.handle_pointer_button(button, false);
    assert_eq!(release.binding_action_invocations.len(), 1);
    assert_eq!(release.pointer_buttons.len(), 1);
    assert_eq!(
        release.pointer_buttons[0].delivery,
        NativePointerButtonDelivery::Compositor
    );
    apply_native_input_effect(
        release,
        NativeInputApplyContext {
            binding_action_catalog: input.binding_manager.action_catalog(),
            server: &mut server,
            perf: NativePerfLogger::from_env(),
            resize_perf: &mut resize_perf,
            cursor_mode: NativeCursorRenderMode::Software,
            app_gpu_policy: EffectiveCompositorAppGpuPolicy::CpuOnly,
            process_supervisor: &mut process_supervisor,
            xwayland: None,
        },
    )
    .unwrap();

    client_commands.send(ClientCommand::CaptureButtons).unwrap();
    assert_eq!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::Buttons {
            pressed_count: 0,
            released_count: 0,
        }
    );
    client_commands.send(ClientCommand::Finish).unwrap();
    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::Finished { .. }
    ));
}

#[test]
fn rejected_native_binding_begin_resize_keeps_button_sequence_compositor_owned() {
    let socket_name = format!("typhon-native-input-rejected-resize-{}", std::process::id());
    let socket_path =
        PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap()).join(&socket_name);
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (client_commands, client_events) = spawn_native_input_resize_client(socket_path);

    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::ReadyForPointer
    ));
    server.send_pointer_motion(92.0, 86.0);
    client_commands.send(ClientCommand::SetCursor).unwrap();
    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::CursorReady { .. }
    ));
    assert!(server.toggle_maximize_focused_window());
    client_commands.send(ClientCommand::CaptureActive).unwrap();
    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::Active { .. }
    ));
    server.send_pointer_motion(92.0, 86.0);
    client_commands.send(ClientCommand::CaptureActive).unwrap();
    let active = pump_native_input_server_until(&mut server, &client_events);
    let ClientEvent::Active {
        pointer_enter_count,
        ..
    } = active
    else {
        panic!("expected active pointer state, got {active:?}");
    };
    assert!(pointer_enter_count > 0);

    server.send_pointer_button(u32::from(BTN_LEFT), true);
    server.send_pointer_button(u32::from(BTN_LEFT), false);
    client_commands.send(ClientCommand::CaptureButtons).unwrap();
    assert_eq!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::Buttons {
            pressed_count: 1,
            released_count: 1,
        }
    );

    let button = u32::from(BTN_RIGHT);
    let mut input = NativeInputState::new(320, 200);
    input.handle_key_event(KEY_LEFTMETA, 1);
    input.handle_pointer_motion_delta(-68.0, -14.0);
    let press = input.handle_pointer_button(button, true);
    assert!(press.pointer_buttons.is_empty());
    assert_eq!(
        press.window_actions,
        vec![NativeWindowAction::BeginResize {
            x: 92.0,
            y: 86.0,
            trigger_button: Some(button),
        }]
    );

    let mut resize_perf = NativeResizePerfState::default();
    let mut process_supervisor = ChildSupervisor::new();
    apply_native_input_effect(
        press,
        NativeInputApplyContext {
            binding_action_catalog: &BindingActionCatalog::empty(),
            server: &mut server,
            perf: NativePerfLogger::from_env(),
            resize_perf: &mut resize_perf,
            cursor_mode: NativeCursorRenderMode::Software,
            app_gpu_policy: EffectiveCompositorAppGpuPolicy::CpuOnly,
            process_supervisor: &mut process_supervisor,
            xwayland: None,
        },
    )
    .unwrap();
    assert!(!server.window_interaction_active());

    input.handle_key_event(KEY_LEFTMETA, 0);
    let release = input.handle_pointer_button(button, false);
    assert!(!input.is_pointer_button_pressed(button));
    assert!(input.pressed_pointer_buttons_snapshot().is_empty());
    assert_eq!(release.pointer_buttons.len(), 1);
    assert_eq!(
        release.pointer_buttons[0].delivery,
        NativePointerButtonDelivery::Compositor
    );
    apply_native_input_effect(
        release,
        NativeInputApplyContext {
            binding_action_catalog: &BindingActionCatalog::empty(),
            server: &mut server,
            perf: NativePerfLogger::from_env(),
            resize_perf: &mut resize_perf,
            cursor_mode: NativeCursorRenderMode::Software,
            app_gpu_policy: EffectiveCompositorAppGpuPolicy::CpuOnly,
            process_supervisor: &mut process_supervisor,
            xwayland: None,
        },
    )
    .unwrap();
    assert!(!server.window_interaction_active());

    assert!(server.toggle_fullscreen_focused_window());
    let move_button = u32::from(BTN_LEFT);
    input.handle_key_event(KEY_LEFTMETA, 1);
    let move_press = input.handle_pointer_button(move_button, true);
    assert!(move_press.pointer_buttons.is_empty());
    assert_eq!(
        move_press.window_actions,
        vec![NativeWindowAction::BeginMove {
            x: 92.0,
            y: 86.0,
            trigger_button: Some(move_button),
        }]
    );
    apply_native_input_effect(
        move_press,
        NativeInputApplyContext {
            binding_action_catalog: input.binding_manager.action_catalog(),
            server: &mut server,
            perf: NativePerfLogger::from_env(),
            resize_perf: &mut resize_perf,
            cursor_mode: NativeCursorRenderMode::Software,
            app_gpu_policy: EffectiveCompositorAppGpuPolicy::CpuOnly,
            process_supervisor: &mut process_supervisor,
            xwayland: None,
        },
    )
    .unwrap();
    assert!(!server.window_interaction_active());

    input.handle_key_event(KEY_LEFTMETA, 0);
    let move_release = input.handle_pointer_button(move_button, false);
    assert_eq!(move_release.pointer_buttons.len(), 1);
    assert_eq!(
        move_release.pointer_buttons[0].delivery,
        NativePointerButtonDelivery::Compositor
    );
    apply_native_input_effect(
        move_release,
        NativeInputApplyContext {
            binding_action_catalog: input.binding_manager.action_catalog(),
            server: &mut server,
            perf: NativePerfLogger::from_env(),
            resize_perf: &mut resize_perf,
            cursor_mode: NativeCursorRenderMode::Software,
            app_gpu_policy: EffectiveCompositorAppGpuPolicy::CpuOnly,
            process_supervisor: &mut process_supervisor,
            xwayland: None,
        },
    )
    .unwrap();
    assert!(!server.window_interaction_active());

    client_commands.send(ClientCommand::CaptureButtons).unwrap();
    assert_eq!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::Buttons {
            pressed_count: 1,
            released_count: 1,
        }
    );
    client_commands.send(ClientCommand::Finish).unwrap();
    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::Finished { .. }
    ));
}

#[test]
fn compositor_owned_trigger_release_ends_before_reconciliation_fallback() {
    let socket_name = format!(
        "typhon-native-input-consumed-release-{}",
        std::process::id()
    );
    let socket_path =
        PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap()).join(&socket_name);
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (client_commands, client_events) = spawn_native_input_resize_client(socket_path);

    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::ReadyForPointer
    ));

    let button = u32::from(BTN_RIGHT);
    server.send_pointer_motion(92.0, 86.0);
    client_commands.send(ClientCommand::SetCursor).unwrap();
    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::CursorReady { .. }
    ));

    let mut input = NativeInputState::new(320, 200);
    input.handle_key_event(KEY_LEFTMETA, 1);
    input.handle_pointer_motion_delta(-68.0, -14.0);
    let press = input.handle_pointer_button(button, true);
    assert!(press.pointer_buttons.is_empty());
    assert!(matches!(
        press.window_actions.as_slice(),
        [NativeWindowAction::BeginResize { trigger_button: Some(trigger), .. }] if *trigger == button
    ));
    let mut resize_perf = NativeResizePerfState::default();
    let mut process_supervisor = ChildSupervisor::new();
    apply_native_input_effect(
        press,
        NativeInputApplyContext {
            binding_action_catalog: input.binding_manager.action_catalog(),
            server: &mut server,
            perf: NativePerfLogger::from_env(),
            resize_perf: &mut resize_perf,
            cursor_mode: NativeCursorRenderMode::Software,
            app_gpu_policy: EffectiveCompositorAppGpuPolicy::CpuOnly,
            process_supervisor: &mut process_supervisor,
            xwayland: None,
        },
    )
    .unwrap();
    assert!(server.window_interaction_active());

    input.binding_manager = AstreaBindingManager::with_specs(vec![BindingSpec {
        modifiers: ModifierMask::EMPTY,
        trigger: BindingTrigger::PointerRelease,
        input: BindingInput::PointerButton(button),
        action: BindingActionDefinition::EmitShortcut {
            namespace: "test".to_string(),
            name: "consumed_release".to_string(),
        },
        repeat: RepeatPolicy::Disabled,
        inhibition: InhibitionPolicy::Respect,
        reserved: false,
    }]);
    input.handle_key_event(KEY_LEFTMETA, 0);

    let release = input.handle_pointer_button(button, false);

    assert!(!input.is_pointer_button_pressed(button));
    assert_eq!(release.binding_action_invocations.len(), 1);
    assert_eq!(release.pointer_buttons.len(), 1);
    assert_eq!(
        release.pointer_buttons[0].delivery,
        NativePointerButtonDelivery::Compositor
    );
    apply_native_input_effect(
        release,
        NativeInputApplyContext {
            binding_action_catalog: input.binding_manager.action_catalog(),
            server: &mut server,
            perf: NativePerfLogger::from_env(),
            resize_perf: &mut resize_perf,
            cursor_mode: NativeCursorRenderMode::Software,
            app_gpu_policy: EffectiveCompositorAppGpuPolicy::CpuOnly,
            process_supervisor: &mut process_supervisor,
            xwayland: None,
        },
    )
    .unwrap();
    assert!(!server.window_interaction_active());
    assert!(!server.reconcile_window_interaction_trigger(false));
    let metrics = server.window_interaction_release_metrics();
    assert_eq!(metrics.window_interaction_trigger_releases, 1);
    assert_eq!(metrics.window_interaction_compositor_releases_consumed, 1);
    assert_eq!(metrics.window_interaction_client_releases_forwarded, 0);
    client_commands.send(ClientCommand::CaptureButtons).unwrap();
    assert_eq!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::Buttons {
            pressed_count: 0,
            released_count: 0,
        }
    );

    client_commands.send(ClientCommand::Finish).unwrap();
    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::Finished { .. }
    ));
}

#[test]
fn client_owned_xdg_move_release_uses_production_native_routing() {
    let socket_name = format!("typhon-native-input-xdg-release-{}", std::process::id());
    let socket_path =
        PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap()).join(&socket_name);
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (client_commands, client_events) = spawn_native_input_resize_client(socket_path);

    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::ReadyForPointer
    ));
    server.send_pointer_motion(92.0, 86.0);
    client_commands.send(ClientCommand::SetCursor).unwrap();
    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::CursorReady { .. }
    ));
    let mut resize_perf = NativeResizePerfState::default();
    let mut process_supervisor = ChildSupervisor::new();
    for cycle in 0..100 {
        server.send_pointer_button(u32::from(BTN_LEFT), true);
        client_commands.send(ClientCommand::BeginXdgMove).unwrap();
        assert!(matches!(
            pump_native_input_server_until(&mut server, &client_events),
            ClientEvent::MoveRequested
        ));
        assert!(server.window_interaction_active());

        apply_native_input_effect(
            NativeInputEffect {
                pointer_buttons: vec![NativePointerButtonEvent::new_at(
                    u32::from(BTN_LEFT),
                    false,
                    92.0,
                    86.0,
                    320,
                    200,
                )],
                ..NativeInputEffect::default()
            },
            NativeInputApplyContext {
                binding_action_catalog: &BindingActionCatalog::empty(),
                server: &mut server,
                perf: NativePerfLogger::from_env(),
                resize_perf: &mut resize_perf,
                cursor_mode: NativeCursorRenderMode::Software,
                app_gpu_policy: EffectiveCompositorAppGpuPolicy::CpuOnly,
                process_supervisor: &mut process_supervisor,
                xwayland: None,
            },
        )
        .unwrap();

        assert!(!server.window_interaction_active(), "cycle {cycle}");
        assert!(server.pointer_ownership_is_clear(), "cycle {cycle}");
        server.send_pointer_button(u32::from(BTN_LEFT), false);
        client_commands.send(ClientCommand::CaptureButtons).unwrap();
        assert_eq!(
            pump_native_input_server_until(&mut server, &client_events),
            ClientEvent::Buttons {
                pressed_count: cycle + 1,
                released_count: cycle + 1,
            }
        );
    }

    let metrics = server.window_interaction_release_metrics();
    assert_eq!(metrics.window_interaction_trigger_releases, 100);
    assert_eq!(metrics.window_interaction_client_releases_forwarded, 100);
    assert_eq!(metrics.window_interaction_release_target_missing, 0);
    client_commands.send(ClientCommand::Finish).unwrap();
    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::Finished { .. }
    ));
}

#[test]
fn client_owned_xdg_resize_release_uses_production_native_routing() {
    let socket_name = format!(
        "typhon-native-input-xdg-resize-release-{}",
        std::process::id()
    );
    let socket_path =
        PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap()).join(&socket_name);
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (client_commands, client_events) = spawn_native_input_resize_client(socket_path);

    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::ReadyForPointer
    ));
    server.send_pointer_motion(92.0, 86.0);
    client_commands.send(ClientCommand::SetCursor).unwrap();
    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::CursorReady { .. }
    ));

    let mut resize_perf = NativeResizePerfState::default();
    let mut process_supervisor = ChildSupervisor::new();
    for cycle in 0..3 {
        server.send_pointer_button(u32::from(BTN_LEFT), true);
        client_commands.send(ClientCommand::BeginXdgResize).unwrap();
        assert!(matches!(
            pump_native_input_server_until(&mut server, &client_events),
            ClientEvent::ResizeRequested
        ));
        assert!(server.window_interaction_active(), "cycle {cycle}");

        apply_native_input_effect(
            NativeInputEffect {
                pointer_buttons: vec![NativePointerButtonEvent::new_at(
                    u32::from(BTN_LEFT),
                    false,
                    92.0,
                    86.0,
                    320,
                    200,
                )],
                ..NativeInputEffect::default()
            },
            NativeInputApplyContext {
                binding_action_catalog: &BindingActionCatalog::empty(),
                server: &mut server,
                perf: NativePerfLogger::from_env(),
                resize_perf: &mut resize_perf,
                cursor_mode: NativeCursorRenderMode::Software,
                app_gpu_policy: EffectiveCompositorAppGpuPolicy::CpuOnly,
                process_supervisor: &mut process_supervisor,
                xwayland: None,
            },
        )
        .unwrap();

        assert!(!server.window_interaction_active(), "cycle {cycle}");
        assert!(server.pointer_ownership_is_clear(), "cycle {cycle}");
        server.send_pointer_button(u32::from(BTN_LEFT), false);
        client_commands.send(ClientCommand::CaptureButtons).unwrap();
        assert_eq!(
            pump_native_input_server_until(&mut server, &client_events),
            ClientEvent::Buttons {
                pressed_count: cycle + 1,
                released_count: cycle + 1,
            }
        );
    }

    let metrics = server.window_interaction_release_metrics();
    assert_eq!(metrics.window_interaction_trigger_releases, 3);
    assert_eq!(metrics.window_interaction_client_releases_forwarded, 3);
    assert_eq!(metrics.window_interaction_release_target_missing, 0);
    client_commands.send(ClientCommand::Finish).unwrap();
    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::Finished { .. }
    ));
}

#[test]
fn client_owned_x11_release_uses_production_native_routing() {
    let socket_name = format!("typhon-native-input-x11-release-{}", std::process::id());
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let (server_stream, client_stream) = UnixStream::pair().unwrap();
    server
        .insert_xwayland_client(server_stream, generation)
        .unwrap();
    let (client_commands, client_events) = spawn_native_input_xwayland_client(client_stream);

    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::ReadyForPointer
    ));
    let serial = NonZeroU64::new((u64::from(0x3333_4444_u32) << 32) | 0x1111_2222).unwrap();
    let surface_id = server
        .take_xwayland_association_events()
        .into_iter()
        .find_map(|event| match event {
            oblivion_one::xwayland::XwaylandAssociationEvent::Committed {
                generation: event_generation,
                serial: event_serial,
                surface_id,
            } if event_generation == generation && event_serial == serial => Some(surface_id),
            _ => None,
        })
        .expect("XWayland surface association");
    let handle = X11WindowHandle::new(generation, 0x100);
    server.apply_xwayland_window_event(XwmEvent::WindowReady(X11WindowSnapshot {
        handle,
        surface_id,
        kind: DesktopWindowKind::Managed,
        window_types: X11WindowTypes::default(),
        decoration_hints: Default::default(),
        override_redirect: false,
        geometry: X11Geometry {
            x: 80,
            y: 80,
            width: 160,
            height: 120,
        },
        metadata: WindowMetadata::default(),
        constraints: WindowConstraints::default(),
        state: X11PublishedState::default(),
        transient_for: None,
        supports_delete: true,
        supports_take_focus: true,
        accepts_input: Some(true),
        window_role: None,
        startup_id: None,
        user_time: None,
        urgency: false,
        supports_sync_request: false,
        sync_counter: None,
    }));
    server.send_pointer_motion(92.0, 86.0);
    client_commands.send(ClientCommand::SetCursor).unwrap();
    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::CursorReady { .. }
    ));

    let mut resize_perf = NativeResizePerfState::default();
    let mut process_supervisor = ChildSupervisor::new();
    let mut run_cycle = |cycle: usize, direction: X11MoveResizeDirection| {
        server.send_pointer_button(u32::from(BTN_LEFT), true);
        server.apply_xwayland_window_event(XwmEvent::MoveResizeRequested {
            window: handle,
            request: X11MoveResizeRequest {
                root_x: 92,
                root_y: 86,
                direction,
                button: 1,
                source: 1,
            },
        });
        assert!(server.window_interaction_active(), "cycle {cycle}");

        apply_native_input_effect(
            NativeInputEffect {
                pointer_buttons: vec![NativePointerButtonEvent::new_at(
                    u32::from(BTN_LEFT),
                    false,
                    92.0,
                    86.0,
                    320,
                    200,
                )],
                ..NativeInputEffect::default()
            },
            NativeInputApplyContext {
                binding_action_catalog: &BindingActionCatalog::empty(),
                server: &mut server,
                perf: NativePerfLogger::from_env(),
                resize_perf: &mut resize_perf,
                cursor_mode: NativeCursorRenderMode::Software,
                app_gpu_policy: EffectiveCompositorAppGpuPolicy::CpuOnly,
                process_supervisor: &mut process_supervisor,
                xwayland: None,
            },
        )
        .unwrap();

        assert!(!server.window_interaction_active(), "cycle {cycle}");
        assert!(server.pointer_ownership_is_clear(), "cycle {cycle}");
        server.send_pointer_button(u32::from(BTN_LEFT), false);
        client_commands.send(ClientCommand::CaptureButtons).unwrap();
        assert_eq!(
            pump_native_input_server_until(&mut server, &client_events),
            ClientEvent::Buttons {
                pressed_count: cycle + 1,
                released_count: cycle + 1,
            }
        );
    };
    for cycle in 0..4 {
        run_cycle(
            cycle,
            if cycle % 2 == 0 {
                X11MoveResizeDirection::Move
            } else {
                X11MoveResizeDirection::BottomRight
            },
        );
    }
    for cycle in 0..100 {
        run_cycle(4 + cycle, X11MoveResizeDirection::Move);
    }

    let metrics = server.window_interaction_release_metrics();
    assert_eq!(metrics.window_interaction_trigger_releases, 104);
    assert_eq!(metrics.window_interaction_client_releases_forwarded, 104);
    assert_eq!(metrics.window_interaction_release_target_missing, 0);
    client_commands.send(ClientCommand::Finish).unwrap();
    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::Finished { .. }
    ));
}

#[test]
fn xwayland_client_cursor_remains_visible_and_relative_input_flows_while_locked() {
    let socket_name = format!(
        "typhon-native-input-xwayland-lock-cursor-{}",
        std::process::id()
    );
    let mut server = OwnCompositorServer::bind_native_base(&socket_name).unwrap();
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let (server_stream, client_stream) = UnixStream::pair().unwrap();
    server
        .insert_xwayland_client(server_stream, generation)
        .unwrap();
    let (client_commands, client_events) = spawn_native_input_xwayland_client(client_stream);

    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::ReadyForPointer
    ));
    let serial = NonZeroU64::new((u64::from(0x3333_4444_u32) << 32) | 0x1111_2222).unwrap();
    let surface_id = server
        .take_xwayland_association_events()
        .into_iter()
        .find_map(|event| match event {
            oblivion_one::xwayland::XwaylandAssociationEvent::Committed {
                generation: event_generation,
                serial: event_serial,
                surface_id,
            } if event_generation == generation && event_serial == serial => Some(surface_id),
            _ => None,
        })
        .expect("XWayland surface association");
    let handle = X11WindowHandle::new(generation, 0x100);
    server.apply_xwayland_window_event(XwmEvent::WindowReady(X11WindowSnapshot {
        handle,
        surface_id,
        kind: DesktopWindowKind::Managed,
        window_types: X11WindowTypes::default(),
        decoration_hints: Default::default(),
        override_redirect: false,
        geometry: X11Geometry {
            x: 80,
            y: 80,
            width: 160,
            height: 120,
        },
        metadata: WindowMetadata::default(),
        constraints: WindowConstraints::default(),
        state: X11PublishedState::default(),
        transient_for: None,
        supports_delete: true,
        supports_take_focus: true,
        accepts_input: Some(true),
        window_role: None,
        startup_id: None,
        user_time: None,
        urgency: false,
        supports_sync_request: false,
        sync_counter: None,
    }));

    server.send_pointer_motion(92.0, 86.0);
    client_commands.send(ClientCommand::SetCursor).unwrap();
    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::CursorReady {
            pointer_enter_count: 1,
            ..
        }
    ));
    let initial_cursor =
        resolve_native_cursor_for_server(&server, &NativeInputState::new(320, 200));
    assert_eq!(
        initial_cursor.source,
        NativeResolvedCursorSource::ClientSurface
    );
    assert!(initial_cursor.visible);
    let render_state = server
        .client_cursor_render_state()
        .expect("XWayland cursor surface is active");
    assert_eq!((render_state.logical_x, render_state.logical_y), (89, 82));
    // The client cursor owns the cursor image, so the theme cursor is already
    // disabled independently of pointer constraints.
    let _ = server.take_pointer_constraint_backend_requests();

    client_commands.send(ClientCommand::LockPointer).unwrap();
    assert_eq!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::LockRequested
    );
    let mut backend = NativePointerConstraintBackend::new();
    let mut activated = None;
    for request in server.take_pointer_constraint_backend_requests() {
        let action = backend.handle_request(request, OutputPosition { x: 92.0, y: 86.0 });
        if let Some(constraint) = action.activated {
            activated = Some(constraint);
        }
    }
    let locked = activated.expect("native backend acknowledges Locked activation");
    assert_eq!(
        locked.mode,
        oblivion_one::compositor::PointerConstraintMode::Locked
    );
    server.pointer_constraint_backend_activated_at(locked.id, locked.anchor);
    assert!(backend.active_locked());
    assert!(
        server
            .take_pointer_constraint_backend_requests()
            .iter()
            .all(|request| !matches!(
                request,
                oblivion_one::compositor::PointerConstraintBackendRequest::ApplyCursorVisibility { .. }
            )),
        "lock activation alone must not request a cursor visibility transition"
    );

    client_commands
        .send(ClientCommand::CaptureCursorState)
        .unwrap();
    let baseline = match pump_native_input_server_until(&mut server, &client_events) {
        ClientEvent::CursorState {
            pointer_motion_count,
            relative_motion_count,
            locked_count,
        } => {
            assert_eq!(locked_count, 1, "client receives Locked activation");
            (pointer_motion_count, relative_motion_count)
        }
        other => panic!("unexpected client event: {other:?}"),
    };
    let locked_cursor = resolve_native_cursor_for_server(&server, &NativeInputState::new(320, 200));
    assert_eq!(
        locked_cursor.source,
        NativeResolvedCursorSource::ClientSurface
    );
    assert!(locked_cursor.visible);
    let locked_render_state = server
        .client_cursor_render_state()
        .expect("client cursor surface remains renderable while locked");
    assert_eq!(
        (locked_render_state.logical_x, locked_render_state.logical_y),
        (89, 82)
    );

    server.send_pointer_motion_sample(PointerMotionSample {
        timestamp_usec: 10,
        absolute: Some(OutputPosition { x: 120.0, y: 110.0 }),
        relative: Some(RelativePointerMotion {
            dx: 3.0,
            dy: 4.0,
            dx_unaccelerated: 3.0,
            dy_unaccelerated: 4.0,
        }),
    });
    assert_eq!(server.last_pointer_position(), (92.0, 86.0));
    client_commands
        .send(ClientCommand::CaptureCursorState)
        .unwrap();
    match pump_native_input_server_until(&mut server, &client_events) {
        ClientEvent::CursorState {
            pointer_motion_count,
            relative_motion_count,
            locked_count,
        } => {
            assert_eq!(
                pointer_motion_count, baseline.0,
                "absolute motion stays suppressed"
            );
            assert_eq!(
                relative_motion_count,
                baseline.1 + 1,
                "relative motion continues"
            );
            assert_eq!(locked_count, 1);
        }
        other => panic!("unexpected client event: {other:?}"),
    }

    client_commands.send(ClientCommand::HideCursor).unwrap();
    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::CursorState {
            locked_count: 1,
            ..
        }
    ));
    let hidden_cursor = resolve_native_cursor_for_server(&server, &NativeInputState::new(320, 200));
    assert_eq!(hidden_cursor.source, NativeResolvedCursorSource::Hidden);
    assert!(!hidden_cursor.visible);
    assert!(server.client_cursor_render_state().is_none());

    client_commands.send(ClientCommand::ReplaceCursor).unwrap();
    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::CursorState {
            locked_count: 1,
            ..
        }
    ));
    let replaced_cursor =
        resolve_native_cursor_for_server(&server, &NativeInputState::new(320, 200));
    assert_eq!(
        replaced_cursor.source,
        NativeResolvedCursorSource::ClientSurface
    );
    assert!(replaced_cursor.visible);
    let replaced_render_state = server
        .client_cursor_render_state()
        .expect("client can show a cursor surface while remaining locked");
    assert_eq!(
        (
            replaced_render_state.logical_x,
            replaced_render_state.logical_y
        ),
        (91, 84)
    );

    client_commands.send(ClientCommand::Finish).unwrap();
    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::Finished { .. }
    ));
}

#[test]
fn compositor_owned_native_binding_release_is_consumed_by_production_routing() {
    let socket_name = format!("typhon-native-input-binding-release-{}", std::process::id());
    let socket_path =
        PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap()).join(&socket_name);
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (client_commands, client_events) = spawn_native_input_resize_client(socket_path);

    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::ReadyForPointer
    ));
    server.send_pointer_motion(92.0, 86.0);
    client_commands.send(ClientCommand::SetCursor).unwrap();
    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::CursorReady { .. }
    ));

    let button = u32::from(BTN_RIGHT);
    let mut input = NativeInputState::new(320, 200);
    input.handle_key_event(KEY_LEFTMETA, 1);
    input.handle_pointer_motion_delta(-68.0, -14.0);
    let press = input.handle_pointer_button(button, true);
    assert!(press.pointer_buttons.is_empty());
    let mut resize_perf = NativeResizePerfState::default();
    let mut process_supervisor = ChildSupervisor::new();
    apply_native_input_effect(
        press,
        NativeInputApplyContext {
            binding_action_catalog: &BindingActionCatalog::empty(),
            server: &mut server,
            perf: NativePerfLogger::from_env(),
            resize_perf: &mut resize_perf,
            cursor_mode: NativeCursorRenderMode::Software,
            app_gpu_policy: EffectiveCompositorAppGpuPolicy::CpuOnly,
            process_supervisor: &mut process_supervisor,
            xwayland: None,
        },
    )
    .unwrap();
    assert!(server.window_interaction_active());

    input.handle_key_event(KEY_LEFTMETA, 0);
    let release = input.handle_pointer_button(button, false);
    assert_eq!(release.pointer_buttons.len(), 1);
    assert_eq!(
        release.pointer_buttons[0].delivery,
        NativePointerButtonDelivery::Compositor
    );
    apply_native_input_effect(
        release,
        NativeInputApplyContext {
            binding_action_catalog: &BindingActionCatalog::empty(),
            server: &mut server,
            perf: NativePerfLogger::from_env(),
            resize_perf: &mut resize_perf,
            cursor_mode: NativeCursorRenderMode::Software,
            app_gpu_policy: EffectiveCompositorAppGpuPolicy::CpuOnly,
            process_supervisor: &mut process_supervisor,
            xwayland: None,
        },
    )
    .unwrap();

    assert!(!server.window_interaction_active());
    assert!(!server.reconcile_window_interaction_trigger(false));
    assert!(!input.is_pointer_button_pressed(button));
    assert!(input.pressed_pointer_buttons_snapshot().is_empty());

    let first_release_metrics = server.window_interaction_release_metrics();
    assert_eq!(first_release_metrics.window_interaction_trigger_releases, 1);
    assert_eq!(
        first_release_metrics.window_interaction_compositor_releases_consumed,
        1
    );

    let move_button = u32::from(BTN_LEFT);
    input.handle_key_event(KEY_LEFTMETA, 1);
    let move_press = input.handle_pointer_button(move_button, true);
    assert!(move_press.pointer_buttons.is_empty());
    assert!(matches!(
        move_press.window_actions.as_slice(),
        [NativeWindowAction::BeginMove {
            trigger_button: Some(trigger),
            ..
        }] if *trigger == move_button
    ));
    apply_native_input_effect(
        move_press,
        NativeInputApplyContext {
            binding_action_catalog: &BindingActionCatalog::empty(),
            server: &mut server,
            perf: NativePerfLogger::from_env(),
            resize_perf: &mut resize_perf,
            cursor_mode: NativeCursorRenderMode::Software,
            app_gpu_policy: EffectiveCompositorAppGpuPolicy::CpuOnly,
            process_supervisor: &mut process_supervisor,
            xwayland: None,
        },
    )
    .unwrap();
    assert!(server.window_interaction_active());

    input.handle_key_event(KEY_LEFTMETA, 0);
    let move_release = input.handle_pointer_button(move_button, false);
    assert_eq!(move_release.pointer_buttons.len(), 1);
    assert_eq!(
        move_release.pointer_buttons[0].delivery,
        NativePointerButtonDelivery::Compositor
    );
    apply_native_input_effect(
        move_release,
        NativeInputApplyContext {
            binding_action_catalog: &BindingActionCatalog::empty(),
            server: &mut server,
            perf: NativePerfLogger::from_env(),
            resize_perf: &mut resize_perf,
            cursor_mode: NativeCursorRenderMode::Software,
            app_gpu_policy: EffectiveCompositorAppGpuPolicy::CpuOnly,
            process_supervisor: &mut process_supervisor,
            xwayland: None,
        },
    )
    .unwrap();
    assert!(!server.window_interaction_active());
    assert!(!server.reconcile_window_interaction_trigger(false));
    assert!(!input.is_pointer_button_pressed(move_button));
    assert!(input.pressed_pointer_buttons_snapshot().is_empty());

    client_commands.send(ClientCommand::CaptureButtons).unwrap();
    assert_eq!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::Buttons {
            pressed_count: 0,
            released_count: 0,
        }
    );
    let metrics = server.window_interaction_release_metrics();
    assert_eq!(metrics.window_interaction_trigger_releases, 2);
    assert_eq!(metrics.window_interaction_compositor_releases_consumed, 2);
    assert_eq!(metrics.window_interaction_client_releases_forwarded, 0);
    client_commands.send(ClientCommand::Finish).unwrap();
    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::Finished { .. }
    ));
}
