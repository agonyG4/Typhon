use super::*;

#[test]
fn native_input_shortcut_inhibition_forwards_window_shortcuts_to_client() {
    let mut input = NativeInputState::new(320, 200);
    input.reconcile_keyboard_shortcut_inhibition(KeyboardShortcutInhibitionSnapshot::new(true, 1));

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
    input.reconcile_keyboard_shortcut_inhibition(KeyboardShortcutInhibitionSnapshot::new(true, 1));
    input.handle_key_event(KEY_LEFTALT, 1);

    let effect = input.handle_key_event(KEY_P, 1);

    assert!(effect.exit_requested);
    assert!(effect.keyboard_events.is_empty());
}

#[test]
fn native_input_shortcut_inhibition_reconciles_deferred_alt_and_cancels_alt_tab() {
    let mut input = NativeInputState::new(320, 200);

    assert!(
        input
            .handle_key_event(KEY_LEFTALT, 1)
            .keyboard_events
            .is_empty()
    );
    let alt_tab = input.handle_key_event(KEY_TAB, 1);
    assert_eq!(
        alt_tab.shortcut_events,
        vec![AstreaShortcutEvent::pressed("astrea-shell", "alt_tab_next")]
    );

    let transition = input
        .reconcile_keyboard_shortcut_inhibition(KeyboardShortcutInhibitionSnapshot::new(true, 1));
    assert_eq!(
        transition.keyboard_events,
        vec![NativeKeyboardEvent::new(KEY_LEFTALT, true)]
    );
    assert!(transition.shortcut_events.is_empty());

    let release = input.handle_key_event(KEY_LEFTALT, 0);
    assert_eq!(
        release.keyboard_events,
        vec![NativeKeyboardEvent::new(KEY_LEFTALT, false)]
    );
    assert!(release.shortcut_events.is_empty());
}

#[test]
fn native_input_shortcut_inhibition_preserves_forwarded_release_across_disable() {
    let mut input = NativeInputState::new(320, 200);
    input.reconcile_keyboard_shortcut_inhibition(KeyboardShortcutInhibitionSnapshot::new(true, 1));
    input.handle_key_event(KEY_F11, 1);

    let transition = input
        .reconcile_keyboard_shortcut_inhibition(KeyboardShortcutInhibitionSnapshot::new(false, 2));
    assert!(transition.keyboard_events.is_empty());

    let release = input.handle_key_event(KEY_F11, 0);
    assert_eq!(
        release.keyboard_events,
        vec![NativeKeyboardEvent::new(KEY_F11, false)]
    );
}

#[test]
fn native_input_inhibition_does_not_synthesize_release_for_consumed_shortcut() {
    let mut input = NativeInputState::new(320, 200);
    input.handle_key_event(KEY_LEFTMETA, 1);
    let consumed = input.handle_key_event(KEY_Q, 1);
    assert!(consumed.keyboard_events.is_empty());

    input.reconcile_keyboard_shortcut_inhibition(KeyboardShortcutInhibitionSnapshot::new(true, 1));
    let release = input.handle_key_event(KEY_Q, 0);
    assert!(release.keyboard_events.is_empty());
}
