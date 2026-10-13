use super::bindings::ModifierMask;
use super::events::{
    BTN_LEFT, BTN_RIGHT, KEY_LEFTALT, KEY_LEFTCTRL, KEY_LEFTMETA, KEY_LEFTSHIFT, KEY_RIGHTALT,
    KEY_RIGHTCTRL, KEY_RIGHTMETA, KEY_RIGHTSHIFT, NativeInputEffect, NativeWindowAction,
};
use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicU64, Ordering},
};

static INPUT_TRACE_ENABLED: OnceLock<bool> = OnceLock::new();
static INPUT_TRACE_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static INPUT_TRACE_OUTPUT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub(crate) fn input_trace_enabled() -> bool {
    *INPUT_TRACE_ENABLED
        .get_or_init(|| matches!(std::env::var("TYPHON_INPUT_TRACE").as_deref(), Ok("1")))
}

fn input_trace_message_if_enabled(
    enabled: bool,
    formatter: impl FnOnce() -> String,
) -> Option<String> {
    if enabled { Some(formatter()) } else { None }
}

pub(crate) fn input_trace_log(formatter: impl FnOnce() -> String) {
    let Some(message) = input_trace_message_if_enabled(input_trace_enabled(), formatter) else {
        return;
    };

    let _output_guard = INPUT_TRACE_OUTPUT_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let sequence = INPUT_TRACE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    eprintln!("typhon input: seq={sequence} {message}");
}

pub(crate) fn linux_key_name(code: u16) -> &'static str {
    match code {
        KEY_LEFTMETA => "KEY_LEFTMETA",
        KEY_RIGHTMETA => "KEY_RIGHTMETA",
        KEY_LEFTALT => "KEY_LEFTALT",
        KEY_RIGHTALT => "KEY_RIGHTALT",
        KEY_LEFTCTRL => "KEY_LEFTCTRL",
        KEY_RIGHTCTRL => "KEY_RIGHTCTRL",
        KEY_LEFTSHIFT => "KEY_LEFTSHIFT",
        KEY_RIGHTSHIFT => "KEY_RIGHTSHIFT",
        _ => "unknown",
    }
}

pub(crate) fn linux_pointer_button_name(button: u32) -> &'static str {
    match button {
        value if value == u32::from(BTN_LEFT) => "BTN_LEFT",
        value if value == u32::from(BTN_RIGHT) => "BTN_RIGHT",
        _ => "unknown",
    }
}

pub(crate) fn native_window_action_trace_name(action: &NativeWindowAction) -> Option<&'static str> {
    match action {
        NativeWindowAction::BeginMove { .. } => Some("BeginMove"),
        NativeWindowAction::BeginResize { .. } => Some("BeginResize"),
        NativeWindowAction::EndInteraction => Some("EndInteraction"),
        _ => None,
    }
}

pub(crate) fn trace_native_input_effect_actions(effect: &NativeInputEffect) {
    for action in &effect.window_actions {
        let Some(action_name) = native_window_action_trace_name(action) else {
            continue;
        };
        input_trace_log(|| format!("event=input_effect_action action={action_name}"));
    }
}

pub(crate) fn modifier_families_released(before: ModifierMask, after: ModifierMask) -> String {
    use std::fmt::Write as _;

    let mut families = String::new();
    for (name, family) in [
        ("SUPER", ModifierMask::SUPER),
        ("ALT", ModifierMask::ALT),
        ("CTRL", ModifierMask::CTRL),
        ("SHIFT", ModifierMask::SHIFT),
    ] {
        if before.contains(family) && !after.contains(family) {
            if !families.is_empty() {
                families.push(',');
            }
            let _ = families.write_str(name);
        }
    }
    if families.is_empty() {
        families.push_str("none");
    }
    families
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn disabled_input_trace_does_not_evaluate_its_formatter() {
        let formatted = Cell::new(false);

        let message = input_trace_message_if_enabled(false, || {
            formatted.set(true);
            "event=should_not_be_formatted".to_string()
        });

        assert_eq!(message, None);
        assert!(!formatted.get());
    }
}
