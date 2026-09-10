//! Runtime ownership of a libseat-managed native session.

use crate::native_output::{NativeResult, NativeSeatEvent};
use oblivion_one::control_snapshots::{ControlSessionState, DoctorSeverity};
use std::io;

pub(crate) trait NativeSeatSwitch {
    fn switch_session_request(&self, session: i32) -> io::Result<()>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeVtSwitchRequestStatus {
    Requested,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeVtSwitchRequestOutcome {
    pub(crate) status: NativeVtSwitchRequestStatus,
    pub(crate) disabled_observed: bool,
    pub(crate) error: Option<String>,
}

pub(crate) fn request_native_vt_switch<S, F>(
    seat: &S,
    vt: u8,
    consume_pending_events: F,
) -> NativeResult<NativeVtSwitchRequestOutcome>
where
    S: NativeSeatSwitch,
    F: FnOnce() -> NativeResult<bool>,
{
    let request = seat.switch_session_request(i32::from(vt));
    let disabled_observed = consume_pending_events()?;
    let error = request.as_ref().err().map(ToString::to_string);
    Ok(NativeVtSwitchRequestOutcome {
        status: if request.is_ok() {
            NativeVtSwitchRequestStatus::Requested
        } else {
            NativeVtSwitchRequestStatus::Failed
        },
        disabled_observed,
        error,
    })
}

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

    pub(crate) const fn state_name(self) -> &'static str {
        match self.state {
            NativeSessionState::Active => "active",
            NativeSessionState::Suspending | NativeSessionState::Resuming => "recovering",
            NativeSessionState::Suspended => "suspended",
            NativeSessionState::Failed => "failed",
        }
    }

    pub(crate) const fn control_state(self) -> ControlSessionState {
        match self.state {
            NativeSessionState::Active => ControlSessionState::Active,
            NativeSessionState::Suspended => ControlSessionState::Suspended,
            NativeSessionState::Suspending | NativeSessionState::Resuming => {
                ControlSessionState::Recovering
            }
            NativeSessionState::Failed => ControlSessionState::Failed,
        }
    }

    pub(crate) const fn doctor_severity(self) -> DoctorSeverity {
        match self.state {
            NativeSessionState::Active => DoctorSeverity::Ok,
            NativeSessionState::Suspended
            | NativeSessionState::Suspending
            | NativeSessionState::Resuming => DoctorSeverity::Warning,
            NativeSessionState::Failed => DoctorSeverity::Error,
        }
    }
    pub(crate) const fn permits_output(self) -> bool {
        self.state.permits_output()
    }

    pub(crate) const fn is_resuming(self) -> bool {
        matches!(self.state, NativeSessionState::Resuming)
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
    fn failed_session_state_remains_failed_in_control_mapping() {
        let lifecycle = NativeSessionLifecycle {
            state: NativeSessionState::Failed,
        };
        assert_eq!(lifecycle.state_name(), "failed");
    }

    #[test]
    fn session_control_and_doctor_mappings_are_exhaustive() {
        let cases = [
            (
                NativeSessionState::Active,
                ControlSessionState::Active,
                DoctorSeverity::Ok,
            ),
            (
                NativeSessionState::Suspending,
                ControlSessionState::Recovering,
                DoctorSeverity::Warning,
            ),
            (
                NativeSessionState::Resuming,
                ControlSessionState::Recovering,
                DoctorSeverity::Warning,
            ),
            (
                NativeSessionState::Suspended,
                ControlSessionState::Suspended,
                DoctorSeverity::Warning,
            ),
            (
                NativeSessionState::Failed,
                ControlSessionState::Failed,
                DoctorSeverity::Error,
            ),
        ];
        for (state, control, severity) in cases {
            let lifecycle = NativeSessionLifecycle { state };
            assert_eq!(lifecycle.control_state(), control);
            assert_eq!(lifecycle.doctor_severity(), severity);
        }
    }

    #[test]
    fn one_hundred_suspend_query_resume_cycles_preserve_active_state() {
        let mut lifecycle = NativeSessionLifecycle::default();

        for _ in 0..100 {
            assert_eq!(
                lifecycle.begin_for_event(NativeSeatEvent::Disabled),
                Some(NativeSessionTransition::BeginSuspend)
            );
            assert_eq!(lifecycle.control_state(), ControlSessionState::Recovering);
            assert_eq!(
                lifecycle.finish_suspend(),
                Some(NativeSessionTransition::Suspended)
            );
            assert_eq!(lifecycle.control_state(), ControlSessionState::Suspended);

            assert_eq!(
                lifecycle.begin_for_event(NativeSeatEvent::Enabled),
                Some(NativeSessionTransition::BeginResume)
            );
            assert_eq!(lifecycle.control_state(), ControlSessionState::Recovering);
            assert_eq!(
                lifecycle.finish_resume(),
                Some(NativeSessionTransition::Active)
            );
            assert_eq!(lifecycle.control_state(), ControlSessionState::Active);
            assert!(lifecycle.permits_output());
        }
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

    struct FakeSeat {
        failure: bool,
    }

    impl NativeSeatSwitch for FakeSeat {
        fn switch_session_request(&self, _session: i32) -> std::io::Result<()> {
            if self.failure {
                Err(std::io::Error::from_raw_os_error(libc::EIO))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn failed_vt_request_is_nonfatal_and_not_reported_as_requested() {
        let mut lifecycle = NativeSessionLifecycle::default();
        let outcome = request_native_vt_switch(&FakeSeat { failure: true }, 3, || Ok(false))
            .expect("pending seat events should remain consumable");

        assert_eq!(outcome.status, NativeVtSwitchRequestStatus::Failed);
        assert!(!outcome.disabled_observed);
        assert!(lifecycle.permits_output());
        lifecycle.cancel_resume_for_shutdown();
    }

    #[test]
    fn reentrant_disable_is_consumed_even_when_switch_request_fails() {
        let mut lifecycle = NativeSessionLifecycle::default();
        let outcome = request_native_vt_switch(&FakeSeat { failure: true }, 3, || {
            assert_eq!(
                lifecycle.begin_for_event(NativeSeatEvent::Disabled),
                Some(NativeSessionTransition::BeginSuspend)
            );
            Ok(true)
        })
        .expect("reentrant seat events should be consumed");

        assert_eq!(outcome.status, NativeVtSwitchRequestStatus::Failed);
        assert!(outcome.disabled_observed);
        assert!(!lifecycle.permits_output());
    }

    #[test]
    fn successful_vt_request_reports_requested_after_consuming_events() {
        let outcome = request_native_vt_switch(&FakeSeat { failure: false }, 4, || Ok(false))
            .expect("successful request should return an outcome");

        assert_eq!(outcome.status, NativeVtSwitchRequestStatus::Requested);
        assert!(!outcome.disabled_observed);
    }
}
