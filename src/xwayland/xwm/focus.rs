//! ICCCM focus and activation policy.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FocusModel {
    Input,
    TakeFocusOnly,
    NoFocus,
}

const X11_TIME_HALF_RANGE: u32 = 0x8000_0000;
const ACTIVATION_WINDOW_MS: u32 = 10_000;

#[derive(Debug, Default)]
pub(crate) struct FocusTracker {
    server_focus: Option<u32>,
    typhon_focus: Option<u32>,
    active_property: Option<u32>,
    pending_focus: Option<u32>,
    current_time: Option<u32>,
    last_user_time: Option<u32>,
}

impl FocusTracker {
    pub(crate) fn note_server_timestamp(&mut self, timestamp: u32) {
        if timestamp == 0
            || self
                .current_time
                .is_some_and(|current| !x11_time_after_eq(timestamp, current))
        {
            return;
        }
        self.current_time = Some(timestamp);
    }

    pub(crate) fn note_user_time(&mut self, timestamp: Option<u32>) {
        if let Some(timestamp) = timestamp {
            self.note_server_timestamp(timestamp);
            self.last_user_time = Some(timestamp);
        }
    }

    pub(crate) fn note_activation_request(
        &mut self,
        xid: u32,
        timestamp: u32,
    ) -> (u32, Option<u32>) {
        let current_time = self.current_time.unwrap_or_default();
        self.note_server_timestamp(timestamp);
        self.pending_focus = Some(xid);
        (current_time, self.last_user_time)
    }

    pub(crate) fn note_focus_command(&mut self, xid: Option<u32>, timestamp: u32) {
        self.note_server_timestamp(timestamp);
        self.typhon_focus = xid;
        self.active_property = xid;
        self.pending_focus = None;
    }

    pub(crate) fn note_focus_in(&mut self, xid: u32) {
        self.server_focus = Some(xid);
        if self.pending_focus == Some(xid) {
            self.pending_focus = None;
        }
    }

    pub(crate) fn note_focus_out(&mut self, xid: u32) {
        if self.server_focus == Some(xid) {
            self.server_focus = None;
        }
    }

    pub(crate) fn note_destroyed(&mut self, xid: u32) {
        if self.server_focus == Some(xid) {
            self.server_focus = None;
        }
        if self.typhon_focus == Some(xid) {
            self.typhon_focus = None;
        }
        if self.active_property == Some(xid) {
            self.active_property = None;
        }
        if self.pending_focus == Some(xid) {
            self.pending_focus = None;
        }
    }

    #[cfg(test)]
    pub(crate) fn state(&self) -> (Option<u32>, Option<u32>, Option<u32>, Option<u32>) {
        (
            self.server_focus,
            self.typhon_focus,
            self.active_property,
            self.pending_focus,
        )
    }
}

pub(crate) const fn x11_time_after_eq(left: u32, right: u32) -> bool {
    left == right || left.wrapping_sub(right) < X11_TIME_HALF_RANGE
}

fn x11_time_elapsed(now: u32, then: u32) -> Option<u32> {
    x11_time_after_eq(now, then).then(|| now.wrapping_sub(then))
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
    let recent_request = timestamp != 0
        && x11_time_elapsed(now, timestamp).is_some_and(|elapsed| elapsed < ACTIVATION_WINDOW_MS);
    let recent_user_time = last_user_time.is_some_and(|last| {
        timestamp != 0
            && x11_time_after_eq(timestamp, last)
            && timestamp.wrapping_sub(last) < ACTIVATION_WINDOW_MS
    });
    source_is_user && recent_request
        || current_focus
        || valid_transient
        || startup_token
        || recent_user_time && recent_request
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

    #[test]
    fn x11_time_comparison_handles_wraparound() {
        assert!(x11_time_after_eq(2, u32::MAX - 2));
        assert!(!x11_time_after_eq(u32::MAX - 2, 2));
    }

    #[test]
    fn focus_in_out_reconciles_active_window() {
        let mut tracker = FocusTracker::default();
        tracker.note_activation_request(10, 100);
        tracker.note_focus_in(10);
        assert_eq!(tracker.state().0, Some(10));
        assert_eq!(tracker.state().3, None);
        tracker.note_focus_command(Some(10), 105);
        assert_eq!(tracker.state().1, Some(10));
        assert_eq!(tracker.state().2, Some(10));
        tracker.note_focus_out(10);
        assert_eq!(tracker.state().0, None);
    }

    #[test]
    fn activation_uses_real_current_and_user_times() {
        assert!(!activation_allowed(true, 100, 0, None, false, false, false));
        assert!(activation_allowed(
            true, 100, 105, None, false, false, false
        ));
    }
}
