use super::*;
use crate::system_action::{AstreaSystemAction, AstreaSystemActionCapabilities};

const KEY_EQUAL: u16 = 13;

#[test]
fn logical_keyboard_press_matches_its_raw_symbolic_binding_once() {
    let mut input = NativeInputState::new(320, 200);
    input.binding_manager = AstreaBindingManager::with_specs(vec![BindingSpec {
        modifiers: ModifierMask::SHIFT,
        trigger: BindingTrigger::Press,
        input: BindingInput::KeySym(BindingKeySym::new(xkbcommon::xkb::keysyms::KEY_q)),
        action: BindingActionDefinition::EmitShortcut {
            namespace: "astrea-shell".to_string(),
            name: "raw-q".to_string(),
        },
        repeat: RepeatPolicy::Disabled,
        inhibition: InhibitionPolicy::Respect,
        reserved: false,
    }]);
    let device = KeyboardDeviceId::from_raw(1).unwrap();
    let q = KeyboardSymbolicIdentity {
        keysym: BindingKeySym::new(xkbcommon::xkb::keysyms::KEY_q),
        modifiers: ModifierMask::SHIFT,
    };
    let snapshot = KeyboardSymbolicSnapshot {
        raw: Some(q),
        translated: Some(q),
    };

    let effect = input.handle_hardware_input_event_at_with_symbolic(
        NativeHardwareInputEvent::Keyboard(NativeKeyboardInputEvent::Key {
            device,
            code: KEY_Q,
            pressed: true,
        }),
        Some(snapshot),
        1,
    );

    assert_eq!(effect.binding_action_invocations.len(), 1);
    assert_eq!(shortcut_name(&input, &effect), Some("raw-q"));
    assert_eq!(input.binding_manager.generic_lookup_count_for_tests(), 3);
}

#[test]
fn physical_and_symbolic_candidates_share_definition_order_and_inhibition_fallback() {
    let q = symbolic_snapshot(
        Some((xkbcommon::xkb::keysyms::KEY_q, ModifierMask::EMPTY)),
        None,
    );
    let device = KeyboardDeviceId::from_raw(1).unwrap();

    let mut symbolic_then_physical = NativeInputState::new(320, 200);
    symbolic_then_physical.binding_manager = AstreaBindingManager::with_specs(vec![
        shortcut_binding(
            BindingInput::KeySym(BindingKeySym::new(xkbcommon::xkb::keysyms::KEY_q)),
            ModifierMask::EMPTY,
            BindingTrigger::Press,
            "earlier-symbolic",
            InhibitionPolicy::Respect,
            RepeatPolicy::Disabled,
        ),
        shortcut_binding(
            BindingInput::PhysicalKey(KEY_Q),
            ModifierMask::EMPTY,
            BindingTrigger::Press,
            "later-physical",
            InhibitionPolicy::Respect,
            RepeatPolicy::Disabled,
        ),
    ]);
    let effect = press_with_snapshot(&mut symbolic_then_physical, device, KEY_Q, Some(q));
    assert_eq!(
        shortcut_name(&symbolic_then_physical, &effect),
        Some("later-physical")
    );

    let mut physical_then_symbolic = NativeInputState::new(320, 200);
    physical_then_symbolic.binding_manager = AstreaBindingManager::with_specs(vec![
        shortcut_binding(
            BindingInput::PhysicalKey(KEY_Q),
            ModifierMask::EMPTY,
            BindingTrigger::Press,
            "earlier-physical",
            InhibitionPolicy::Respect,
            RepeatPolicy::Disabled,
        ),
        shortcut_binding(
            BindingInput::KeySym(BindingKeySym::new(xkbcommon::xkb::keysyms::KEY_q)),
            ModifierMask::EMPTY,
            BindingTrigger::Press,
            "later-symbolic",
            InhibitionPolicy::Respect,
            RepeatPolicy::Disabled,
        ),
    ]);
    let effect = press_with_snapshot(&mut physical_then_symbolic, device, KEY_Q, Some(q));
    assert_eq!(
        shortcut_name(&physical_then_symbolic, &effect),
        Some("later-symbolic")
    );

    let mut symbolic_respect = NativeInputState::new(320, 200);
    symbolic_respect.keyboard_shortcuts_inhibited = true;
    symbolic_respect.binding_manager = AstreaBindingManager::with_specs(vec![
        shortcut_binding(
            BindingInput::PhysicalKey(KEY_Q),
            ModifierMask::EMPTY,
            BindingTrigger::Press,
            "physical-bypass-fallback",
            InhibitionPolicy::Bypass,
            RepeatPolicy::Disabled,
        ),
        shortcut_binding(
            BindingInput::KeySym(BindingKeySym::new(xkbcommon::xkb::keysyms::KEY_q)),
            ModifierMask::EMPTY,
            BindingTrigger::Press,
            "later-symbolic-respect",
            InhibitionPolicy::Respect,
            RepeatPolicy::Disabled,
        ),
    ]);
    let effect = press_with_snapshot(&mut symbolic_respect, device, KEY_Q, Some(q));
    assert_eq!(
        shortcut_name(&symbolic_respect, &effect),
        Some("physical-bypass-fallback")
    );

    let mut physical_respect = NativeInputState::new(320, 200);
    physical_respect.keyboard_shortcuts_inhibited = true;
    physical_respect.binding_manager = AstreaBindingManager::with_specs(vec![
        shortcut_binding(
            BindingInput::KeySym(BindingKeySym::new(xkbcommon::xkb::keysyms::KEY_q)),
            ModifierMask::EMPTY,
            BindingTrigger::Press,
            "symbolic-bypass-fallback",
            InhibitionPolicy::Bypass,
            RepeatPolicy::Disabled,
        ),
        shortcut_binding(
            BindingInput::PhysicalKey(KEY_Q),
            ModifierMask::EMPTY,
            BindingTrigger::Press,
            "later-physical-respect",
            InhibitionPolicy::Respect,
            RepeatPolicy::Disabled,
        ),
    ]);
    let effect = press_with_snapshot(&mut physical_respect, device, KEY_Q, Some(q));
    assert_eq!(
        shortcut_name(&physical_respect, &effect),
        Some("symbolic-bypass-fallback")
    );
}

#[test]
fn raw_and_translated_candidates_also_share_definition_order() {
    let snapshot = symbolic_snapshot(
        Some((xkbcommon::xkb::keysyms::KEY_equal, ModifierMask::SHIFT)),
        Some((xkbcommon::xkb::keysyms::KEY_plus, ModifierMask::EMPTY)),
    );
    let device = KeyboardDeviceId::from_raw(1).unwrap();
    let raw = BindingInput::KeySym(BindingKeySym::new(xkbcommon::xkb::keysyms::KEY_equal));
    let translated = BindingInput::KeySym(BindingKeySym::new(xkbcommon::xkb::keysyms::KEY_plus));

    let mut translated_then_raw = NativeInputState::new(320, 200);
    translated_then_raw.binding_manager = AstreaBindingManager::with_specs(vec![
        shortcut_binding(
            translated,
            ModifierMask::EMPTY,
            BindingTrigger::Press,
            "earlier-translated",
            InhibitionPolicy::Respect,
            RepeatPolicy::Disabled,
        ),
        shortcut_binding(
            raw,
            ModifierMask::SHIFT,
            BindingTrigger::Press,
            "later-raw",
            InhibitionPolicy::Respect,
            RepeatPolicy::Disabled,
        ),
    ]);
    let effect = press_with_snapshot(&mut translated_then_raw, device, KEY_EQUAL, Some(snapshot));
    assert_eq!(
        shortcut_name(&translated_then_raw, &effect),
        Some("later-raw")
    );

    let mut raw_then_translated = NativeInputState::new(320, 200);
    raw_then_translated.binding_manager = AstreaBindingManager::with_specs(vec![
        shortcut_binding(
            raw,
            ModifierMask::SHIFT,
            BindingTrigger::Press,
            "earlier-raw",
            InhibitionPolicy::Respect,
            RepeatPolicy::Disabled,
        ),
        shortcut_binding(
            translated,
            ModifierMask::EMPTY,
            BindingTrigger::Press,
            "later-translated",
            InhibitionPolicy::Respect,
            RepeatPolicy::Disabled,
        ),
    ]);
    let effect = press_with_snapshot(&mut raw_then_translated, device, KEY_EQUAL, Some(snapshot));
    assert_eq!(
        shortcut_name(&raw_then_translated, &effect),
        Some("later-translated")
    );
}

#[test]
fn translated_symbol_uses_consumed_modifiers_without_changing_physical_matching() {
    let device = KeyboardDeviceId::from_raw(1).unwrap();
    let shift_consumed = symbolic_snapshot(
        Some((xkbcommon::xkb::keysyms::KEY_equal, ModifierMask::SHIFT)),
        Some((xkbcommon::xkb::keysyms::KEY_plus, ModifierMask::EMPTY)),
    );

    let mut translated = NativeInputState::new(320, 200);
    translated.binding_manager = AstreaBindingManager::with_specs(vec![shortcut_binding(
        BindingInput::KeySym(BindingKeySym::new(xkbcommon::xkb::keysyms::KEY_plus)),
        ModifierMask::EMPTY,
        BindingTrigger::Press,
        "translated-plus",
        InhibitionPolicy::Respect,
        RepeatPolicy::Disabled,
    )]);
    translated.handle_key_event_at(KEY_LEFTSHIFT, true, 1);
    let effect = press_with_snapshot(&mut translated, device, KEY_EQUAL, Some(shift_consumed));
    assert_eq!(shortcut_name(&translated, &effect), Some("translated-plus"));
    assert_eq!(translated.active_modifier_mask(), ModifierMask::SHIFT);

    let mut physical = NativeInputState::new(320, 200);
    physical.binding_manager = AstreaBindingManager::with_specs(vec![shortcut_binding(
        BindingInput::PhysicalKey(KEY_2),
        ModifierMask::SHIFT,
        BindingTrigger::Press,
        "physical-shift-two",
        InhibitionPolicy::Respect,
        RepeatPolicy::Disabled,
    )]);
    physical.handle_key_event_at(KEY_LEFTSHIFT, true, 1);
    let effect = press_with_snapshot(&mut physical, device, KEY_2, Some(shift_consumed));
    assert_eq!(
        shortcut_name(&physical, &effect),
        Some("physical-shift-two")
    );
}

#[test]
fn missing_symbolic_translation_does_not_block_physical_binding() {
    let mut input = NativeInputState::new(320, 200);
    input.binding_manager = AstreaBindingManager::with_specs(vec![shortcut_binding(
        BindingInput::PhysicalKey(KEY_Q),
        ModifierMask::EMPTY,
        BindingTrigger::Press,
        "physical-q-without-symbol",
        InhibitionPolicy::Respect,
        RepeatPolicy::Disabled,
    )]);
    let device = KeyboardDeviceId::from_raw(1).unwrap();

    let effect = press_with_snapshot(
        &mut input,
        device,
        KEY_Q,
        Some(KeyboardSymbolicSnapshot::default()),
    );

    assert_eq!(
        shortcut_name(&input, &effect),
        Some("physical-q-without-symbol")
    );
}

#[test]
fn final_release_uses_the_press_time_symbol_after_a_layout_change() {
    let mut input = NativeInputState::new(320, 200);
    input.binding_manager = AstreaBindingManager::with_specs(vec![
        shortcut_binding(
            BindingInput::KeySym(BindingKeySym::new(xkbcommon::xkb::keysyms::KEY_q)),
            ModifierMask::EMPTY,
            BindingTrigger::Release,
            "q-release",
            InhibitionPolicy::Respect,
            RepeatPolicy::Disabled,
        ),
        shortcut_binding(
            BindingInput::KeySym(BindingKeySym::new(xkbcommon::xkb::keysyms::KEY_a)),
            ModifierMask::EMPTY,
            BindingTrigger::Release,
            "a-release",
            InhibitionPolicy::Respect,
            RepeatPolicy::Disabled,
        ),
    ]);
    let device = KeyboardDeviceId::from_raw(1).unwrap();
    let layout_a = symbolic_snapshot(
        Some((xkbcommon::xkb::keysyms::KEY_q, ModifierMask::EMPTY)),
        Some((xkbcommon::xkb::keysyms::KEY_q, ModifierMask::EMPTY)),
    );
    let layout_b = symbolic_snapshot(
        Some((xkbcommon::xkb::keysyms::KEY_a, ModifierMask::EMPTY)),
        Some((xkbcommon::xkb::keysyms::KEY_a, ModifierMask::EMPTY)),
    );

    let _ = press_with_snapshot(&mut input, device, KEY_Q, Some(layout_a));
    let release = input.handle_hardware_input_event_at_with_symbolic(
        NativeHardwareInputEvent::Keyboard(NativeKeyboardInputEvent::Key {
            device,
            code: KEY_Q,
            pressed: false,
        }),
        Some(layout_b),
        2,
    );

    assert_eq!(shortcut_name(&input, &release), Some("q-release"));
}

#[test]
fn multi_source_key_keeps_the_first_symbol_until_final_release() {
    let mut input = NativeInputState::new(320, 200);
    input.binding_manager = AstreaBindingManager::with_specs(vec![shortcut_binding(
        BindingInput::KeySym(BindingKeySym::new(xkbcommon::xkb::keysyms::KEY_q)),
        ModifierMask::EMPTY,
        BindingTrigger::Release,
        "original-q-release",
        InhibitionPolicy::Respect,
        RepeatPolicy::Disabled,
    )]);
    let first = KeyboardDeviceId::from_raw(1).unwrap();
    let second = KeyboardDeviceId::from_raw(2).unwrap();
    let q = symbolic_snapshot(
        Some((xkbcommon::xkb::keysyms::KEY_q, ModifierMask::EMPTY)),
        Some((xkbcommon::xkb::keysyms::KEY_q, ModifierMask::EMPTY)),
    );
    let z = symbolic_snapshot(
        Some((xkbcommon::xkb::keysyms::KEY_z, ModifierMask::EMPTY)),
        Some((xkbcommon::xkb::keysyms::KEY_z, ModifierMask::EMPTY)),
    );

    let _ = press_with_snapshot(&mut input, first, KEY_Q, Some(q));
    let duplicate_logical_press = press_with_snapshot(&mut input, second, KEY_Q, Some(z));
    assert!(
        duplicate_logical_press
            .binding_action_invocations
            .is_empty()
    );

    let non_final_release = input.handle_hardware_input_event(NativeHardwareInputEvent::Keyboard(
        NativeKeyboardInputEvent::Key {
            device: first,
            code: KEY_Q,
            pressed: false,
        },
    ));
    assert!(non_final_release.binding_action_invocations.is_empty());
    assert!(input.keyboard_key_is_logically_pressed(KEY_Q));

    let final_release = input.handle_hardware_input_event(NativeHardwareInputEvent::Keyboard(
        NativeKeyboardInputEvent::Key {
            device: second,
            code: KEY_Q,
            pressed: false,
        },
    ));
    assert_eq!(
        shortcut_name(&input, &final_release),
        Some("original-q-release")
    );
    assert!(!input.keyboard_key_is_logically_pressed(KEY_Q));
}

#[test]
fn duplicate_press_and_spurious_other_source_release_do_not_replace_cached_symbol() {
    let mut input = NativeInputState::new(320, 200);
    input.binding_manager = AstreaBindingManager::with_specs(vec![shortcut_binding(
        BindingInput::KeySym(BindingKeySym::new(xkbcommon::xkb::keysyms::KEY_q)),
        ModifierMask::EMPTY,
        BindingTrigger::Release,
        "original-q-release",
        InhibitionPolicy::Respect,
        RepeatPolicy::Disabled,
    )]);
    let owner = KeyboardDeviceId::from_raw(1).unwrap();
    let stranger = KeyboardDeviceId::from_raw(2).unwrap();
    let q = symbolic_snapshot(
        Some((xkbcommon::xkb::keysyms::KEY_q, ModifierMask::EMPTY)),
        Some((xkbcommon::xkb::keysyms::KEY_q, ModifierMask::EMPTY)),
    );
    let z = symbolic_snapshot(
        Some((xkbcommon::xkb::keysyms::KEY_z, ModifierMask::EMPTY)),
        Some((xkbcommon::xkb::keysyms::KEY_z, ModifierMask::EMPTY)),
    );

    let _ = press_with_snapshot(&mut input, owner, KEY_Q, Some(q));
    let duplicate = press_with_snapshot(&mut input, owner, KEY_Q, Some(z));
    assert!(duplicate.binding_action_invocations.is_empty());
    let spurious_release = input.handle_hardware_input_event(NativeHardwareInputEvent::Keyboard(
        NativeKeyboardInputEvent::Key {
            device: stranger,
            code: KEY_Q,
            pressed: false,
        },
    ));
    assert!(spurious_release.binding_action_invocations.is_empty());

    let release = input.handle_hardware_input_event(NativeHardwareInputEvent::Keyboard(
        NativeKeyboardInputEvent::Key {
            device: owner,
            code: KEY_Q,
            pressed: false,
        },
    ));
    assert_eq!(shortcut_name(&input, &release), Some("original-q-release"));
}

#[test]
fn source_removal_keeps_identity_for_other_owner_and_uses_it_for_final_owner() {
    let mut input = NativeInputState::new(320, 200);
    input.binding_manager = AstreaBindingManager::with_specs(vec![shortcut_binding(
        BindingInput::KeySym(BindingKeySym::new(xkbcommon::xkb::keysyms::KEY_q)),
        ModifierMask::EMPTY,
        BindingTrigger::Release,
        "source-removed-q-release",
        InhibitionPolicy::Respect,
        RepeatPolicy::Disabled,
    )]);
    let first = KeyboardDeviceId::from_raw(1).unwrap();
    let second = KeyboardDeviceId::from_raw(2).unwrap();
    let q = symbolic_snapshot(
        Some((xkbcommon::xkb::keysyms::KEY_q, ModifierMask::EMPTY)),
        Some((xkbcommon::xkb::keysyms::KEY_q, ModifierMask::EMPTY)),
    );
    let z = symbolic_snapshot(
        Some((xkbcommon::xkb::keysyms::KEY_z, ModifierMask::EMPTY)),
        Some((xkbcommon::xkb::keysyms::KEY_z, ModifierMask::EMPTY)),
    );

    let _ = press_with_snapshot(&mut input, first, KEY_Q, Some(q));
    let _ = press_with_snapshot(&mut input, second, KEY_Q, Some(z));
    let removed_non_final = input.handle_hardware_input_event(NativeHardwareInputEvent::Keyboard(
        NativeKeyboardInputEvent::SourceRemoved { device: first },
    ));
    assert!(removed_non_final.binding_action_invocations.is_empty());
    assert!(input.keyboard_key_is_logically_pressed(KEY_Q));

    let removed_final = input.handle_hardware_input_event(NativeHardwareInputEvent::Keyboard(
        NativeKeyboardInputEvent::SourceRemoved { device: second },
    ));
    assert_eq!(
        shortcut_name(&input, &removed_final),
        Some("source-removed-q-release")
    );
    assert!(!input.keyboard_key_is_logically_pressed(KEY_Q));
}

#[test]
fn symbolic_repeat_keeps_exact_binding_and_cancels_after_translation_changes() {
    let mut input = NativeInputState::new(320, 200);
    input.binding_manager = AstreaBindingManager::with_specs(vec![shortcut_binding(
        BindingInput::KeySym(BindingKeySym::new(xkbcommon::xkb::keysyms::KEY_q)),
        ModifierMask::EMPTY,
        BindingTrigger::Press,
        "repeat-q",
        InhibitionPolicy::Respect,
        RepeatPolicy::Enabled,
    )]);
    let device = KeyboardDeviceId::from_raw(1).unwrap();
    let q = symbolic_snapshot(
        Some((xkbcommon::xkb::keysyms::KEY_q, ModifierMask::EMPTY)),
        Some((xkbcommon::xkb::keysyms::KEY_q, ModifierMask::EMPTY)),
    );
    let z = symbolic_snapshot(
        Some((xkbcommon::xkb::keysyms::KEY_z, ModifierMask::EMPTY)),
        Some((xkbcommon::xkb::keysyms::KEY_z, ModifierMask::EMPTY)),
    );

    let press = press_with_snapshot(&mut input, device, KEY_Q, Some(q));
    let binding = input.active_keyboard_repeat().unwrap().binding;
    assert_eq!(input.keyboard_repeat_symbolic_keycode(), Some(KEY_Q));
    let lookups_after_press = input.binding_manager.generic_lookup_count_for_tests();
    let generation = input.keyboard_repeat_generation();
    let repeated =
        input.service_keyboard_repeat_if_unchanged_with_symbolic(601_000_000, generation, Some(q));
    assert_eq!(shortcut_name(&input, &press), Some("repeat-q"));
    assert_eq!(shortcut_name(&input, &repeated), Some("repeat-q"));
    assert_eq!(input.active_keyboard_repeat().unwrap().binding, binding);
    assert_eq!(
        input.binding_manager.generic_lookup_count_for_tests(),
        lookups_after_press
    );

    let next_generation = input.keyboard_repeat_generation();
    let next_deadline = input.keyboard_repeat_deadline_ns().unwrap();
    let stale = input.service_keyboard_repeat_if_unchanged_with_symbolic(
        next_deadline,
        next_generation,
        Some(z),
    );
    assert!(stale.binding_action_invocations.is_empty());
    assert!(input.active_keyboard_repeat().is_none());
}

#[test]
fn symbolic_repeat_survives_non_final_source_removal_and_stops_at_final_removal() {
    let mut input = NativeInputState::new(320, 200);
    input.binding_manager = AstreaBindingManager::with_specs(vec![shortcut_binding(
        BindingInput::KeySym(BindingKeySym::new(xkbcommon::xkb::keysyms::KEY_q)),
        ModifierMask::EMPTY,
        BindingTrigger::Press,
        "source-owned-repeat-q",
        InhibitionPolicy::Respect,
        RepeatPolicy::Enabled,
    )]);
    let first = KeyboardDeviceId::from_raw(1).unwrap();
    let second = KeyboardDeviceId::from_raw(2).unwrap();
    let q = symbolic_snapshot(
        Some((xkbcommon::xkb::keysyms::KEY_q, ModifierMask::EMPTY)),
        Some((xkbcommon::xkb::keysyms::KEY_q, ModifierMask::EMPTY)),
    );
    let z = symbolic_snapshot(
        Some((xkbcommon::xkb::keysyms::KEY_z, ModifierMask::EMPTY)),
        Some((xkbcommon::xkb::keysyms::KEY_z, ModifierMask::EMPTY)),
    );

    let _ = press_with_snapshot(&mut input, first, KEY_Q, Some(q));
    let _ = press_with_snapshot(&mut input, second, KEY_Q, Some(z));
    let non_final = input.handle_hardware_input_event(NativeHardwareInputEvent::Keyboard(
        NativeKeyboardInputEvent::SourceRemoved { device: first },
    ));
    assert!(non_final.binding_action_invocations.is_empty());
    assert!(input.active_keyboard_repeat().is_some());

    let repeated = input.service_keyboard_repeat_if_unchanged_with_symbolic(
        601_000_000,
        input.keyboard_repeat_generation(),
        Some(q),
    );
    assert_eq!(
        shortcut_name(&input, &repeated),
        Some("source-owned-repeat-q")
    );

    let final_removal = input.handle_hardware_input_event(NativeHardwareInputEvent::Keyboard(
        NativeKeyboardInputEvent::SourceRemoved { device: second },
    ));
    assert!(final_removal.binding_action_invocations.is_empty());
    assert!(input.active_keyboard_repeat().is_none());
    assert!(
        input
            .service_keyboard_repeat_if_unchanged_with_symbolic(
                u64::MAX,
                input.keyboard_repeat_generation(),
                Some(q),
            )
            .binding_action_invocations
            .is_empty()
    );
}

#[test]
fn translated_symbolic_repeat_retains_physical_shift_ownership() {
    let mut input = NativeInputState::new(320, 200);
    input.binding_manager = AstreaBindingManager::with_specs(vec![shortcut_binding(
        BindingInput::KeySym(BindingKeySym::new(xkbcommon::xkb::keysyms::KEY_plus)),
        ModifierMask::EMPTY,
        BindingTrigger::Press,
        "repeat-plus",
        InhibitionPolicy::Respect,
        RepeatPolicy::Enabled,
    )]);
    let device = KeyboardDeviceId::from_raw(1).unwrap();
    let _ = input.handle_key_event_at(KEY_LEFTSHIFT, true, 1);
    let translation = symbolic_snapshot(
        Some((xkbcommon::xkb::keysyms::KEY_equal, ModifierMask::SHIFT)),
        Some((xkbcommon::xkb::keysyms::KEY_plus, ModifierMask::EMPTY)),
    );
    let press = press_with_snapshot(&mut input, device, KEY_EQUAL, Some(translation));
    let active = input.active_keyboard_repeat().unwrap();
    assert_eq!(active.physical_modifiers, ModifierMask::SHIFT);
    assert_eq!(input.keyboard_repeat_symbolic_keycode(), Some(KEY_EQUAL));

    let repeated = input.service_keyboard_repeat_if_unchanged_with_symbolic(
        601_000_000,
        input.keyboard_repeat_generation(),
        Some(translation),
    );
    assert_eq!(shortcut_name(&input, &press), Some("repeat-plus"));
    assert_eq!(shortcut_name(&input, &repeated), Some("repeat-plus"));
}

#[test]
fn physical_repeat_target_does_not_request_symbolic_translation() {
    let mut input = NativeInputState::new(320, 200);
    input.binding_manager = AstreaBindingManager::with_specs(vec![shortcut_binding(
        BindingInput::PhysicalKey(KEY_Q),
        ModifierMask::EMPTY,
        BindingTrigger::Press,
        "repeat-physical-q",
        InhibitionPolicy::Respect,
        RepeatPolicy::Enabled,
    )]);
    let device = KeyboardDeviceId::from_raw(1).unwrap();
    let _ = press_with_snapshot(&mut input, device, KEY_Q, None);

    assert_eq!(input.keyboard_repeat_symbolic_keycode(), None);
}

#[test]
fn session_clear_discards_symbolic_press_identity_and_repeat_target() {
    let mut input = NativeInputState::new(320, 200);
    input.binding_manager = AstreaBindingManager::with_specs(vec![
        shortcut_binding(
            BindingInput::KeySym(BindingKeySym::new(xkbcommon::xkb::keysyms::KEY_q)),
            ModifierMask::EMPTY,
            BindingTrigger::Press,
            "q-repeat",
            InhibitionPolicy::Respect,
            RepeatPolicy::Enabled,
        ),
        shortcut_binding(
            BindingInput::KeySym(BindingKeySym::new(xkbcommon::xkb::keysyms::KEY_q)),
            ModifierMask::EMPTY,
            BindingTrigger::Release,
            "q-release",
            InhibitionPolicy::Respect,
            RepeatPolicy::Disabled,
        ),
        shortcut_binding(
            BindingInput::KeySym(BindingKeySym::new(xkbcommon::xkb::keysyms::KEY_z)),
            ModifierMask::EMPTY,
            BindingTrigger::Release,
            "z-release",
            InhibitionPolicy::Respect,
            RepeatPolicy::Disabled,
        ),
    ]);
    let device = KeyboardDeviceId::from_raw(1).unwrap();
    let q = symbolic_snapshot(
        Some((xkbcommon::xkb::keysyms::KEY_q, ModifierMask::EMPTY)),
        Some((xkbcommon::xkb::keysyms::KEY_q, ModifierMask::EMPTY)),
    );
    let z = symbolic_snapshot(
        Some((xkbcommon::xkb::keysyms::KEY_z, ModifierMask::EMPTY)),
        Some((xkbcommon::xkb::keysyms::KEY_z, ModifierMask::EMPTY)),
    );

    let _ = press_with_snapshot(&mut input, device, KEY_Q, Some(q));
    assert_eq!(input.keyboard_repeat_symbolic_keycode(), Some(KEY_Q));
    input.clear_pressed_state_for_session_switch();
    assert_eq!(input.keyboard_repeat_symbolic_keycode(), None);
    let stale_release = input.handle_hardware_input_event(NativeHardwareInputEvent::Keyboard(
        NativeKeyboardInputEvent::Key {
            device,
            code: KEY_Q,
            pressed: false,
        },
    ));
    assert!(stale_release.binding_action_invocations.is_empty());

    let _ = press_with_snapshot(&mut input, device, KEY_Q, Some(z));
    let fresh_release = input.handle_hardware_input_event(NativeHardwareInputEvent::Keyboard(
        NativeKeyboardInputEvent::Key {
            device,
            code: KEY_Q,
            pressed: false,
        },
    ));
    assert_eq!(shortcut_name(&input, &fresh_release), Some("z-release"));
}

fn symbolic_snapshot(
    raw: Option<(u32, ModifierMask)>,
    translated: Option<(u32, ModifierMask)>,
) -> KeyboardSymbolicSnapshot {
    let identity = |(keysym, modifiers)| KeyboardSymbolicIdentity {
        keysym: BindingKeySym::new(keysym),
        modifiers,
    };
    KeyboardSymbolicSnapshot {
        raw: raw.map(identity),
        translated: translated.map(identity),
    }
}

fn shortcut_binding(
    input: BindingInput,
    modifiers: ModifierMask,
    trigger: BindingTrigger,
    name: &str,
    inhibition: InhibitionPolicy,
    repeat: RepeatPolicy,
) -> BindingSpec {
    BindingSpec {
        modifiers,
        trigger,
        input,
        action: BindingActionDefinition::EmitShortcut {
            namespace: "astrea-shell".to_string(),
            name: name.to_string(),
        },
        repeat,
        inhibition,
        reserved: false,
    }
}

fn press_with_snapshot(
    input: &mut NativeInputState,
    device: KeyboardDeviceId,
    code: u16,
    symbolic: Option<KeyboardSymbolicSnapshot>,
) -> NativeInputEffect {
    input.handle_hardware_input_event_at_with_symbolic(
        NativeHardwareInputEvent::Keyboard(NativeKeyboardInputEvent::Key {
            device,
            code,
            pressed: true,
        }),
        symbolic,
        1,
    )
}

fn shortcut_name<'a>(input: &'a NativeInputState, effect: &NativeInputEffect) -> Option<&'a str> {
    let invocation = effect.binding_action_invocations.first()?;
    match input
        .binding_manager
        .action_catalog()
        .action(invocation.action)?
    {
        BindingActionDefinition::EmitShortcut { name, .. } => Some(name.as_str()),
        _ => None,
    }
}

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

fn system_action_binding(
    keysym: u32,
    action: AstreaSystemAction,
    repeat: RepeatPolicy,
) -> BindingSpec {
    BindingSpec {
        modifiers: ModifierMask::EMPTY,
        trigger: BindingTrigger::Press,
        input: BindingInput::KeySym(BindingKeySym::new(keysym)),
        action: BindingActionDefinition::SystemAction(action),
        repeat,
        inhibition: InhibitionPolicy::Bypass,
        reserved: true,
    }
}

fn system_symbol_snapshot(keysym: u32) -> KeyboardSymbolicSnapshot {
    symbolic_snapshot(
        Some((keysym, ModifierMask::EMPTY)),
        Some((keysym, ModifierMask::EMPTY)),
    )
}

#[test]
fn default_xf86_system_bindings_have_typed_actions_and_expected_policies() {
    use xkbcommon::xkb::keysyms as keysym;

    let expected = [
        (
            keysym::KEY_XF86AudioRaiseVolume,
            AstreaSystemAction::OutputVolumeUp,
            RepeatPolicy::Enabled,
        ),
        (
            keysym::KEY_XF86AudioLowerVolume,
            AstreaSystemAction::OutputVolumeDown,
            RepeatPolicy::Enabled,
        ),
        (
            keysym::KEY_XF86AudioMute,
            AstreaSystemAction::ToggleOutputMute,
            RepeatPolicy::Disabled,
        ),
        (
            keysym::KEY_XF86AudioMicMute,
            AstreaSystemAction::ToggleMicrophoneMute,
            RepeatPolicy::Disabled,
        ),
        (
            keysym::KEY_XF86AudioPlay,
            AstreaSystemAction::MediaPlayPause,
            RepeatPolicy::Disabled,
        ),
        (
            keysym::KEY_XF86AudioPause,
            AstreaSystemAction::MediaPause,
            RepeatPolicy::Disabled,
        ),
        (
            keysym::KEY_XF86AudioStop,
            AstreaSystemAction::MediaStop,
            RepeatPolicy::Disabled,
        ),
        (
            keysym::KEY_XF86AudioNext,
            AstreaSystemAction::MediaNext,
            RepeatPolicy::Disabled,
        ),
        (
            keysym::KEY_XF86AudioPrev,
            AstreaSystemAction::MediaPrevious,
            RepeatPolicy::Disabled,
        ),
        (
            keysym::KEY_XF86AudioRewind,
            AstreaSystemAction::MediaRewind,
            RepeatPolicy::Disabled,
        ),
        (
            keysym::KEY_XF86AudioForward,
            AstreaSystemAction::MediaFastForward,
            RepeatPolicy::Disabled,
        ),
        (
            keysym::KEY_XF86MonBrightnessUp,
            AstreaSystemAction::DisplayBrightnessUp,
            RepeatPolicy::Enabled,
        ),
        (
            keysym::KEY_XF86MonBrightnessDown,
            AstreaSystemAction::DisplayBrightnessDown,
            RepeatPolicy::Enabled,
        ),
        (
            keysym::KEY_XF86KbdBrightnessUp,
            AstreaSystemAction::KeyboardBrightnessUp,
            RepeatPolicy::Enabled,
        ),
        (
            keysym::KEY_XF86KbdBrightnessDown,
            AstreaSystemAction::KeyboardBrightnessDown,
            RepeatPolicy::Enabled,
        ),
        (
            keysym::KEY_XF86KbdLightOnOff,
            AstreaSystemAction::ToggleKeyboardBacklight,
            RepeatPolicy::Disabled,
        ),
        (
            keysym::KEY_XF86TouchpadToggle,
            AstreaSystemAction::ToggleTouchpad,
            RepeatPolicy::Disabled,
        ),
    ];
    let specs = default_astrea_binding_specs();
    let system_specs: Vec<_> = specs
        .iter()
        .filter(|spec| matches!(&spec.action, BindingActionDefinition::SystemAction(_)))
        .collect();

    assert_eq!(system_specs.len(), expected.len());
    for (keysym, action, repeat) in expected {
        let binding = system_specs
            .iter()
            .find(|spec| spec.input == BindingInput::KeySym(BindingKeySym::new(keysym)))
            .expect("expected XF86 system binding");
        assert_eq!(binding.modifiers, ModifierMask::EMPTY);
        assert_eq!(binding.trigger, BindingTrigger::Press);
        assert_eq!(
            binding.action,
            BindingActionDefinition::SystemAction(action)
        );
        assert_eq!(binding.repeat, repeat);
        assert_eq!(binding.inhibition, InhibitionPolicy::Bypass);
        assert!(binding.reserved);
    }

    let existing_specs: Vec<_> = specs
        .iter()
        .filter(|spec| !matches!(&spec.action, BindingActionDefinition::SystemAction(_)))
        .collect();
    assert_eq!(existing_specs.len(), 38);
    assert!(existing_specs.iter().all(|spec| {
        matches!(
            spec.input,
            BindingInput::PhysicalKey(_) | BindingInput::PointerButton(_)
        )
    }));
}

#[test]
fn empty_production_capabilities_leave_xf86_press_on_client_forwarding_path() {
    use xkbcommon::xkb::keysyms;

    let mut input = NativeInputState::new(320, 200);
    let device = KeyboardDeviceId::from_raw(1).unwrap();
    let effect = press_with_snapshot(
        &mut input,
        device,
        KEY_Z,
        Some(system_symbol_snapshot(keysyms::KEY_XF86AudioRaiseVolume)),
    );

    assert!(effect.binding_action_invocations.is_empty());
    assert_eq!(
        effect.keyboard_actions,
        vec![NativeKeyboardAction::PhysicalAndClient(
            NativeKeyboardEvent::new(KEY_Z, true)
        )]
    );
    assert!(input.active_keyboard_repeat().is_none());
}

#[test]
fn system_capabilities_enable_only_the_advertised_xf86_actions() {
    use xkbcommon::xkb::keysyms;

    let volume_up = AstreaSystemActionCapabilities::for_action(AstreaSystemAction::OutputVolumeUp);
    let mut manager =
        AstreaBindingManager::with_default_bindings_and_system_capabilities(volume_up);
    let raise = manager.handle_keyboard_key(
        ModifierMask::EMPTY,
        KEY_Z,
        Some(system_symbol_snapshot(keysyms::KEY_XF86AudioRaiseVolume)),
        true,
        false,
        false,
    );
    let AstreaBindingMatch::Consumed {
        binding: Some(binding),
        action,
        repeat,
        inhibition,
        ..
    } = raise
    else {
        panic!("advertised volume-up capability should match");
    };
    assert_eq!(
        manager.action_catalog().action(action),
        Some(&BindingActionDefinition::SystemAction(
            AstreaSystemAction::OutputVolumeUp
        ))
    );
    assert_eq!(repeat, RepeatPolicy::Enabled);
    assert_eq!(inhibition, InhibitionPolicy::Bypass);

    let lower = manager.handle_keyboard_key(
        ModifierMask::EMPTY,
        KEY_Z,
        Some(system_symbol_snapshot(keysyms::KEY_XF86AudioLowerVolume)),
        true,
        false,
        false,
    );
    assert_eq!(lower, AstreaBindingMatch::Pass);

    let mut inhibited_manager =
        AstreaBindingManager::with_default_bindings_and_system_capabilities(volume_up);
    assert!(matches!(
        inhibited_manager.handle_keyboard_key(
            ModifierMask::EMPTY,
            KEY_Z,
            Some(system_symbol_snapshot(keysyms::KEY_XF86AudioRaiseVolume)),
            true,
            false,
            true,
        ),
        AstreaBindingMatch::Consumed { .. }
    ));

    let mut unsupported_manager =
        AstreaBindingManager::with_default_bindings_and_system_capabilities(
            AstreaSystemActionCapabilities::EMPTY,
        );
    assert_eq!(
        unsupported_manager.handle_keyboard_key(
            ModifierMask::EMPTY,
            KEY_Z,
            Some(system_symbol_snapshot(keysyms::KEY_XF86AudioRaiseVolume)),
            true,
            false,
            true,
        ),
        AstreaBindingMatch::Pass
    );
    assert!(
        unsupported_manager
            .match_repeat_binding(
                binding,
                KEY_Z,
                ModifierMask::EMPTY,
                Some(system_symbol_snapshot(keysyms::KEY_XF86AudioRaiseVolume)),
                false,
            )
            .is_none()
    );
    assert!(manager.binding(binding).is_some());
}

#[test]
fn unavailable_later_system_action_falls_back_and_available_one_keeps_global_order() {
    use xkbcommon::xkb::keysyms as keysym;

    let system = system_action_binding(
        keysym::KEY_XF86AudioRaiseVolume,
        AstreaSystemAction::OutputVolumeUp,
        RepeatPolicy::Enabled,
    );
    let ordinary = shortcut_binding(
        BindingInput::KeySym(BindingKeySym::new(keysym::KEY_XF86AudioRaiseVolume)),
        ModifierMask::EMPTY,
        BindingTrigger::Press,
        "earlier-ordinary",
        InhibitionPolicy::Respect,
        RepeatPolicy::Disabled,
    );
    let snapshot = Some(system_symbol_snapshot(keysym::KEY_XF86AudioRaiseVolume));

    let mut unsupported = AstreaBindingManager::with_specs_and_system_capabilities(
        vec![ordinary.clone(), system.clone()],
        AstreaSystemActionCapabilities::EMPTY,
    );
    let AstreaBindingMatch::Consumed { action, .. } =
        unsupported.handle_keyboard_key(ModifierMask::EMPTY, KEY_Z, snapshot, true, false, false)
    else {
        panic!("earlier ordinary binding should be eligible");
    };
    assert_eq!(
        unsupported.action_catalog().action(action),
        Some(&ordinary.action)
    );

    let capable = AstreaSystemActionCapabilities::for_action(AstreaSystemAction::OutputVolumeUp);
    let mut supported =
        AstreaBindingManager::with_specs_and_system_capabilities(vec![ordinary, system], capable);
    let AstreaBindingMatch::Consumed { action, .. } =
        supported.handle_keyboard_key(ModifierMask::EMPTY, KEY_Z, snapshot, true, false, false)
    else {
        panic!("available system binding should match");
    };
    assert_eq!(
        supported.action_catalog().action(action),
        Some(&BindingActionDefinition::SystemAction(
            AstreaSystemAction::OutputVolumeUp
        ))
    );
}

#[test]
fn physical_and_xf86_system_candidates_keep_one_definition_order() {
    use xkbcommon::xkb::keysyms as keysym;

    let physical = shortcut_binding(
        BindingInput::PhysicalKey(KEY_Z),
        ModifierMask::EMPTY,
        BindingTrigger::Press,
        "physical",
        InhibitionPolicy::Respect,
        RepeatPolicy::Disabled,
    );
    let system = system_action_binding(
        keysym::KEY_XF86AudioMute,
        AstreaSystemAction::ToggleOutputMute,
        RepeatPolicy::Disabled,
    );
    let capabilities =
        AstreaSystemActionCapabilities::for_action(AstreaSystemAction::ToggleOutputMute);
    let snapshot = Some(system_symbol_snapshot(keysym::KEY_XF86AudioMute));

    let mut physical_later = AstreaBindingManager::with_specs_and_system_capabilities(
        vec![system.clone(), physical.clone()],
        capabilities,
    );
    let AstreaBindingMatch::Consumed { action, .. } = physical_later.handle_keyboard_key(
        ModifierMask::EMPTY,
        KEY_Z,
        snapshot,
        true,
        false,
        false,
    ) else {
        panic!("one candidate should match");
    };
    assert_eq!(
        physical_later.action_catalog().action(action),
        Some(&physical.action)
    );

    let mut system_later = AstreaBindingManager::with_specs_and_system_capabilities(
        vec![physical, system],
        capabilities,
    );
    let AstreaBindingMatch::Consumed { action, .. } =
        system_later.handle_keyboard_key(ModifierMask::EMPTY, KEY_Z, snapshot, true, false, false)
    else {
        panic!("one candidate should match");
    };
    assert_eq!(
        system_later.action_catalog().action(action),
        Some(&BindingActionDefinition::SystemAction(
            AstreaSystemAction::ToggleOutputMute
        ))
    );
}

#[test]
fn xf86_raw_translated_duplicate_is_one_binding_and_one_repeat_target() {
    use xkbcommon::xkb::keysyms as keysym;

    let capabilities =
        AstreaSystemActionCapabilities::for_action(AstreaSystemAction::OutputVolumeUp);
    let mut input = NativeInputState::new(320, 200);
    input.binding_manager =
        AstreaBindingManager::with_default_bindings_and_system_capabilities(capabilities);
    let device = KeyboardDeviceId::from_raw(1).unwrap();
    let snapshot = system_symbol_snapshot(keysym::KEY_XF86AudioRaiseVolume);
    let press = press_with_snapshot(&mut input, device, KEY_Z, Some(snapshot));
    let active = input.active_keyboard_repeat().expect("volume step repeats");

    assert_eq!(press.binding_action_invocations.len(), 1);
    assert_eq!(
        input
            .binding_manager
            .action_catalog()
            .action(press.binding_action_invocations[0].action),
        Some(&BindingActionDefinition::SystemAction(
            AstreaSystemAction::OutputVolumeUp
        ))
    );
    assert!(!press.redraw_requested);
    assert!(press.launch_command.is_none());
    assert!(press.keyboard_events.is_empty());

    let generation = input.keyboard_repeat_generation();
    let repeat = input.service_keyboard_repeat_if_unchanged_with_symbolic(
        601_000_000,
        generation,
        Some(snapshot),
    );
    assert_eq!(repeat.binding_action_invocations.len(), 1);
    assert_eq!(
        repeat.binding_action_invocations[0].action,
        press.binding_action_invocations[0].action
    );
    assert_eq!(
        input.active_keyboard_repeat().unwrap().binding,
        active.binding
    );
}
