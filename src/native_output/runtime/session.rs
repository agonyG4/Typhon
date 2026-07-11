//! Runtime ownership of a libseat-managed native session.

use crate::native_output::NativeSeatEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeSessionState {
    Active,
    Suspending,
    Suspended,
    Resuming,
    Failed,
}

impl NativeSessionState {
    pub(crate) const fn permits_output(self) -> bool {
        matches!(self, Self::Active)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeSessionTransition {
    BeginSuspend,
    Suspended,
    BeginResume,
    Active,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeSessionLifecycle {
    state: NativeSessionState,
}

impl Default for NativeSessionLifecycle {
    fn default() -> Self {
        Self {
            state: NativeSessionState::Active,
        }
    }
}

impl NativeSessionLifecycle {
    #[cfg(test)]
    pub(crate) const fn state(self) -> NativeSessionState {
        self.state
    }
    pub(crate) const fn permits_output(self) -> bool {
        self.state.permits_output()
    }

    pub(crate) fn begin_for_event(
        &mut self,
        event: NativeSeatEvent,
    ) -> Option<NativeSessionTransition> {
        match (self.state, event) {
            (NativeSessionState::Active, NativeSeatEvent::Disabled) => {
                self.state = NativeSessionState::Suspending;
                Some(NativeSessionTransition::BeginSuspend)
            }
            (NativeSessionState::Suspended, NativeSeatEvent::Enabled) => {
                self.state = NativeSessionState::Resuming;
                Some(NativeSessionTransition::BeginResume)
            }
            _ => None,
        }
    }

    pub(crate) fn finish_suspend(&mut self) -> Option<NativeSessionTransition> {
        (self.state == NativeSessionState::Suspending).then(|| {
            self.state = NativeSessionState::Suspended;
            NativeSessionTransition::Suspended
        })
    }

    pub(crate) fn finish_resume(&mut self) -> Option<NativeSessionTransition> {
        (self.state == NativeSessionState::Resuming).then(|| {
            self.state = NativeSessionState::Active;
            NativeSessionTransition::Active
        })
    }

    pub(crate) fn fail_resume(&mut self) -> Option<NativeSessionTransition> {
        (self.state == NativeSessionState::Resuming).then(|| {
            self.state = NativeSessionState::Failed;
            NativeSessionTransition::Failed
        })
    }

    pub(crate) fn cancel_resume_for_shutdown(&mut self) {
        if self.state == NativeSessionState::Resuming {
            self.state = NativeSessionState::Suspended;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_transitions_active_to_suspended_only_after_quiesce() {
        let mut lifecycle = NativeSessionLifecycle::default();
        assert_eq!(
            lifecycle.begin_for_event(NativeSeatEvent::Disabled),
            Some(NativeSessionTransition::BeginSuspend)
        );
        assert!(!lifecycle.permits_output());
        assert_eq!(
            lifecycle.finish_suspend(),
            Some(NativeSessionTransition::Suspended)
        );
        assert_eq!(lifecycle.state(), NativeSessionState::Suspended);
    }

    #[test]
    fn session_transitions_suspended_to_active_only_after_recovery() {
        let mut lifecycle = NativeSessionLifecycle {
            state: NativeSessionState::Suspended,
        };
        assert_eq!(
            lifecycle.begin_for_event(NativeSeatEvent::Enabled),
            Some(NativeSessionTransition::BeginResume)
        );
        assert!(!lifecycle.permits_output());
        assert_eq!(
            lifecycle.finish_resume(),
            Some(NativeSessionTransition::Active)
        );
        assert!(lifecycle.permits_output());
    }

    #[test]
    fn duplicate_and_stale_seat_events_are_ignored() {
        let mut lifecycle = NativeSessionLifecycle::default();
        assert_eq!(lifecycle.begin_for_event(NativeSeatEvent::Enabled), None);
        lifecycle.begin_for_event(NativeSeatEvent::Disabled);
        assert_eq!(lifecycle.begin_for_event(NativeSeatEvent::Disabled), None);
        lifecycle.finish_suspend();
        assert_eq!(lifecycle.begin_for_event(NativeSeatEvent::Disabled), None);
        assert_eq!(
            lifecycle.begin_for_event(NativeSeatEvent::Enabled),
            Some(NativeSessionTransition::BeginResume)
        );
        assert_eq!(lifecycle.begin_for_event(NativeSeatEvent::Enabled), None);
    }

    #[test]
    fn failed_recovery_never_reactivates_the_session() {
        let mut lifecycle = NativeSessionLifecycle {
            state: NativeSessionState::Suspended,
        };
        lifecycle.begin_for_event(NativeSeatEvent::Enabled);
        assert_eq!(
            lifecycle.fail_resume(),
            Some(NativeSessionTransition::Failed)
        );
        assert!(!lifecycle.permits_output());
        assert_eq!(lifecycle.finish_resume(), None);
    }

    #[test]
    fn enable_after_shutdown_does_not_leave_session_resuming() {
        let mut lifecycle = NativeSessionLifecycle {
            state: NativeSessionState::Suspended,
        };
        lifecycle.begin_for_event(NativeSeatEvent::Enabled);
        lifecycle.cancel_resume_for_shutdown();

        assert_eq!(lifecycle.state(), NativeSessionState::Suspended);
    }
}
