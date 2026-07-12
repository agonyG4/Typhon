use super::input::{
    ClientCommand, ClientEvent, pump_native_input_server_until, spawn_native_input_resize_client,
};
use super::*;

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
        vec![NativePointerButtonEvent::new_at(
            u32::from(BTN_RIGHT),
            false,
            160.0,
            100.0,
            320,
            200,
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
fn consumed_trigger_release_is_detected_by_reconciliation() {
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
    assert!(server.begin_window_resize_at_with_trigger(92.0, 86.0, button));
    server.send_pointer_motion(92.0, 86.0);
    client_commands.send(ClientCommand::SetCursor).unwrap();
    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::CursorReady { .. }
    ));

    let mut input = NativeInputState::new(320, 200);
    input.binding_manager = AstreaBindingManager::with_bindings(vec![Binding {
        modifiers: ModifierMask::EMPTY,
        trigger: BindingTrigger::PointerRelease,
        input: BindingInput::PointerButton(button),
        action: BindingAction::EmitShortcut {
            namespace: "test".to_string(),
            name: "consumed_release".to_string(),
        },
        repeat: RepeatPolicy::Disabled,
        inhibition: InhibitionPolicy::Respect,
        reserved: false,
    }]);
    input.handle_pointer_button(button, true);

    let release = input.handle_pointer_button(button, false);

    assert!(!input.is_pointer_button_pressed(button));
    assert!(release.pointer_buttons.is_empty());
    let mut resize_perf = NativeResizePerfState::default();
    let mut process_supervisor = ChildSupervisor::new();
    apply_native_input_effect(
        release,
        NativeInputApplyContext {
            server: &mut server,
            perf: NativePerfLogger::from_env(),
            resize_perf: &mut resize_perf,
            cursor_mode: NativeCursorRenderMode::Software,
            app_gpu_policy: EffectiveCompositorAppGpuPolicy::CpuOnly,
            seat_session: None,
            process_supervisor: &mut process_supervisor,
        },
    )
    .unwrap();
    assert!(server.window_interaction_active());
    assert!(server.reconcile_window_interaction_trigger(false));
    assert!(!server.window_interaction_active());

    client_commands.send(ClientCommand::Finish).unwrap();
    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::Finished { .. }
    ));
}
