//! ICCCM focus and activation policy.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FocusModel {
    Input,
    TakeFocusOnly,
    NoFocus,
}

pub(crate) fn focus_model(input: Option<bool>, take_focus: bool) -> FocusModel {
    match (input.unwrap_or(true), take_focus) {
        (true, _) => FocusModel::Input,
        (false, true) => FocusModel::TakeFocusOnly,
        (false, false) => FocusModel::NoFocus,
    }
}

pub(crate) fn activation_allowed(
    source_is_user: bool,
    timestamp: u32,
    now: u32,
    last_user_time: Option<u32>,
    current_focus: bool,
    valid_transient: bool,
    startup_token: bool,
) -> bool {
    let recent =
        last_user_time.is_some_and(|last| timestamp == 0 || timestamp.wrapping_sub(last) < 10_000);
    source_is_user
        || current_focus
        || valid_transient
        || startup_token
        || recent && now.wrapping_sub(timestamp) < 10_000
}

pub(crate) fn should_send_take_focus(input: Option<bool>, take_focus: bool) -> bool {
    matches!(focus_model(input, take_focus), FocusModel::TakeFocusOnly)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_false_only_uses_take_focus_protocol() {
        assert_eq!(focus_model(Some(false), true), FocusModel::TakeFocusOnly);
        assert_eq!(focus_model(Some(false), false), FocusModel::NoFocus);
        assert!(should_send_take_focus(Some(false), true));
    }
}
