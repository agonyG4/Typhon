use super::*;

#[test]
fn native_input_repeat_reuses_the_press_binding_id_without_generic_lookup() {
    let mut input = NativeInputState::new(320, 200);
    input.binding_manager = AstreaBindingManager::with_specs(vec![
        BindingSpec {
            modifiers: ModifierMask::EMPTY,
            trigger: BindingTrigger::Press,
            input: BindingInput::PhysicalKey(KEY_Z),
            action: BindingActionDefinition::EmitShortcut {
                namespace: "astrea-shell".to_string(),
                name: "repeat-earlier".to_string(),
            },
            repeat: RepeatPolicy::Enabled,
            inhibition: InhibitionPolicy::Respect,
            reserved: false,
        },
        BindingSpec {
            modifiers: ModifierMask::EMPTY,
            trigger: BindingTrigger::Press,
            input: BindingInput::PhysicalKey(KEY_Z),
            action: BindingActionDefinition::EmitShortcut {
                namespace: "astrea-shell".to_string(),
                name: "repeat-selected".to_string(),
            },
            repeat: RepeatPolicy::Enabled,
            inhibition: InhibitionPolicy::Respect,
            reserved: false,
        },
    ]);

    let press = input.handle_key_event_at(KEY_Z, true, 1_000_000);
    let active = input.active_keyboard_repeat().unwrap();
    let binding_id = active.binding;
    let action_id = press.binding_action_invocations[0].action;
    assert_eq!(
        input.binding_manager.binding(binding_id).unwrap().action,
        action_id
    );
    let lookup_count = input.binding_manager.generic_lookup_count_for_tests();
    assert_eq!(lookup_count, 1);

    let repeated = input.service_keyboard_repeat(601_000_000);

    assert_eq!(input.active_keyboard_repeat().unwrap().binding, binding_id);
    assert_eq!(repeated.binding_action_invocations[0].action, action_id);
    assert_eq!(
        input.binding_manager.generic_lookup_count_for_tests(),
        lookup_count
    );
}

#[test]
fn native_input_repeat_cancels_the_selected_respect_binding_without_fallback() {
    let mut input = NativeInputState::new(320, 200);
    input.binding_manager = AstreaBindingManager::with_specs(vec![
        BindingSpec {
            modifiers: ModifierMask::EMPTY,
            trigger: BindingTrigger::Press,
            input: BindingInput::PhysicalKey(KEY_Z),
            action: BindingActionDefinition::EmitShortcut {
                namespace: "astrea-shell".to_string(),
                name: "repeat-bypass-earlier".to_string(),
            },
            repeat: RepeatPolicy::Enabled,
            inhibition: InhibitionPolicy::Bypass,
            reserved: false,
        },
        BindingSpec {
            modifiers: ModifierMask::EMPTY,
            trigger: BindingTrigger::Press,
            input: BindingInput::PhysicalKey(KEY_Z),
            action: BindingActionDefinition::EmitShortcut {
                namespace: "astrea-shell".to_string(),
                name: "repeat-respect-selected".to_string(),
            },
            repeat: RepeatPolicy::Enabled,
            inhibition: InhibitionPolicy::Respect,
            reserved: false,
        },
    ]);
    let press = input.handle_key_event_at(KEY_Z, true, 1_000_000);
    let selected_action = press.binding_action_invocations[0].action;
    assert!(input.active_keyboard_repeat().is_some());

    input.reconcile_keyboard_shortcut_inhibition(KeyboardShortcutInhibitionSnapshot::new(true, 1));
    let lookup_count = input.binding_manager.generic_lookup_count_for_tests();
    let repeated = input.service_keyboard_repeat(601_000_000);

    assert!(input.active_keyboard_repeat().is_none());
    assert!(repeated.binding_action_invocations.is_empty());
    assert_eq!(
        input.binding_manager.generic_lookup_count_for_tests(),
        lookup_count
    );
    assert_eq!(
        input
            .binding_manager
            .action_catalog()
            .action(selected_action),
        Some(&BindingActionDefinition::EmitShortcut {
            namespace: "astrea-shell".to_string(),
            name: "repeat-respect-selected".to_string(),
        })
    );
}

#[test]
fn native_input_repeat_modifier_mismatch_does_not_retarget_to_another_candidate() {
    let mut input = NativeInputState::new(320, 200);
    input.binding_manager = AstreaBindingManager::with_specs(vec![
        BindingSpec {
            modifiers: ModifierMask::SHIFT,
            trigger: BindingTrigger::Press,
            input: BindingInput::PhysicalKey(KEY_Z),
            action: BindingActionDefinition::EmitShortcut {
                namespace: "astrea-shell".to_string(),
                name: "shift-repeat-candidate".to_string(),
            },
            repeat: RepeatPolicy::Enabled,
            inhibition: InhibitionPolicy::Bypass,
            reserved: false,
        },
        BindingSpec {
            modifiers: ModifierMask::EMPTY,
            trigger: BindingTrigger::Press,
            input: BindingInput::PhysicalKey(KEY_Z),
            action: BindingActionDefinition::EmitShortcut {
                namespace: "astrea-shell".to_string(),
                name: "initial-repeat-binding".to_string(),
            },
            repeat: RepeatPolicy::Enabled,
            inhibition: InhibitionPolicy::Respect,
            reserved: false,
        },
    ]);

    let press = input.handle_key_event_at(KEY_Z, true, 1_000_000);
    let selected_action = press.binding_action_invocations[0].action;
    let selected_binding = input.active_keyboard_repeat().unwrap().binding;
    input.handle_key_event_at(KEY_LEFTSHIFT, true, 2_000_000);
    let lookup_count = input.binding_manager.generic_lookup_count_for_tests();
    let repeated = input.service_keyboard_repeat(601_000_000);

    assert!(input.active_keyboard_repeat().is_none());
    assert!(repeated.binding_action_invocations.is_empty());
    assert_eq!(
        input.binding_manager.generic_lookup_count_for_tests(),
        lookup_count
    );
    assert_eq!(
        input
            .binding_manager
            .binding(selected_binding)
            .unwrap()
            .action,
        selected_action
    );
}

#[test]
fn unavailable_session_command_stays_consumed_and_uses_construction_snapshot() {
    let _guard = ASTREA_ENV_LOCK.lock().unwrap();
    let previous = std::env::var_os("OBLIVION_ONE_SESSION_1_COMMAND");
    unsafe {
        std::env::remove_var("OBLIVION_ONE_SESSION_1_COMMAND");
    }
    let mut input = NativeInputState::new(320, 200);
    unsafe {
        std::env::set_var("OBLIVION_ONE_SESSION_1_COMMAND", "added-after-build");
    }
    input.handle_key_event(KEY_LEFTCTRL, 1);
    input.handle_key_event(KEY_LEFTSHIFT, 1);
    input.handle_key_event(KEY_LEFTALT, 1);

    let effect = input.handle_key_event(KEY_1, 1);
    assert!(effect.keyboard_events.is_empty());
    assert!(effect.launch_command.is_none());
    assert_eq!(effect.binding_action_invocations.len(), 1);
    assert!(matches!(
        input
            .binding_manager
            .action_catalog()
            .action(effect.binding_action_invocations[0].action),
        Some(BindingActionDefinition::LaunchSessionCommand {
            index: 1,
            command: None
        })
    ));

    unsafe {
        if let Some(previous) = previous {
            std::env::set_var("OBLIVION_ONE_SESSION_1_COMMAND", previous);
        } else {
            std::env::remove_var("OBLIVION_ONE_SESSION_1_COMMAND");
        }
    }
}
