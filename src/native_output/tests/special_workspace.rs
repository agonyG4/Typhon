use super::*;

#[test]
fn native_input_special_shortcuts_are_consumed_press_only_without_key_leak() {
    let mut input = NativeInputState::new(320, 200);
    let super_press = input.handle_key_event(KEY_LEFTMETA, 1);
    assert!(super_press.keyboard_events.is_empty());

    let toggle = input.handle_key_event(KEY_S, 1);
    assert_eq!(
        toggle.window_actions,
        vec![NativeWindowAction::ToggleDefaultSpecialWorkspace]
    );
    assert!(toggle.keyboard_events.is_empty());

    let repeat = input.handle_key_event(KEY_S, 2);
    assert!(repeat.window_actions.is_empty());
    assert!(repeat.keyboard_events.is_empty());

    let release = input.handle_key_event(KEY_S, 0);
    assert!(release.keyboard_events.is_empty());
    let super_release = input.handle_key_event(KEY_LEFTMETA, 0);
    assert!(super_release.keyboard_events.is_empty());
}

#[test]
fn native_input_layout_toggle_is_super_v_exact_press_only_and_inhibition_aware() {
    let mut input = NativeInputState::new(320, 200);
    let super_press = input.handle_key_event(KEY_LEFTMETA, 1);
    assert!(super_press.keyboard_events.is_empty());

    let toggle = input.handle_key_event(KEY_V, 1);
    assert_eq!(
        toggle.window_actions,
        vec![NativeWindowAction::ToggleFocusedWindowLayout]
    );
    assert!(toggle.keyboard_events.is_empty());

    let repeat = input.handle_key_event(KEY_V, 2);
    assert!(repeat.window_actions.is_empty());
    assert!(repeat.keyboard_events.is_empty());

    let release = input.handle_key_event(KEY_V, 0);
    assert!(release.window_actions.is_empty());
    assert!(release.keyboard_events.is_empty());

    let super_release = input.handle_key_event(KEY_LEFTMETA, 0);
    assert!(super_release.keyboard_events.is_empty());

    let mut inhibited = NativeInputState::new(320, 200);
    inhibited.keyboard_shortcuts_inhibited = true;
    inhibited.super_pressed = true;
    let pass = inhibited.handle_key_event(KEY_V, 1);
    assert!(pass.window_actions.is_empty());
}
