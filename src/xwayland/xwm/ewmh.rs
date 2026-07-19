//! EWMH normalization.  Only values with an implemented compositor action
//! are allowed through this boundary.

use super::{X11PublishedState, X11StateAction, X11StateAtom};

pub(crate) fn decode_state_action(value: u32) -> Option<X11StateAction> {
    match value {
        0 => Some(X11StateAction::Remove),
        1 => Some(X11StateAction::Add),
        2 => Some(X11StateAction::Toggle),
        _ => None,
    }
}

pub(crate) fn state_atom(
    value: u32,
    fullscreen: u32,
    max_horz: u32,
    max_vert: u32,
    hidden: u32,
) -> Option<X11StateAtom> {
    match value {
        value if value == fullscreen => Some(X11StateAtom::Fullscreen),
        value if value == max_horz => Some(X11StateAtom::MaximizedHorizontal),
        value if value == max_vert => Some(X11StateAtom::MaximizedVertical),
        value if value == hidden => Some(X11StateAtom::Hidden),
        _ => None,
    }
}

pub(crate) fn apply_state_action(value: bool, action: X11StateAction) -> bool {
    match action {
        X11StateAction::Remove => false,
        X11StateAction::Add => true,
        X11StateAction::Toggle => !value,
    }
}

pub(crate) fn aggregate_maximize(horizontal: bool, vertical: bool) -> bool {
    horizontal && vertical
}

pub(crate) fn publishable_state(state: X11PublishedState) -> X11PublishedState {
    X11PublishedState {
        maximized: state.maximized,
        ..state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_action_is_rejected() {
        assert_eq!(decode_state_action(3), None);
    }

    #[test]
    fn full_maximize_requires_both_axes() {
        assert!(!aggregate_maximize(true, false));
        assert!(aggregate_maximize(true, true));
    }
}
