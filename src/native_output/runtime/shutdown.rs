pub(super) const NATIVE_SHUTDOWN_PAGEFLIP_TIMEOUT_NS: u64 = 250_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShutdownState {
    Running,
    Requested,
    Draining,
    StoppingChildren,
    Restoring,
    Complete,
}

impl ShutdownState {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Requested => "requested",
            Self::Draining => "draining",
            Self::StoppingChildren => "stopping_children",
            Self::Restoring => "restoring",
            Self::Complete => "complete",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShutdownPageflipOutcome {
    NoPendingPageflip,
    ConfirmedCompletion { token: u64 },
    ForcedTimeout { token: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShutdownTransitionReason {
    Request,
    PendingPageflip,
    NoPendingPageflip,
    PageflipComplete,
    PageflipTimeout,
    ChildrenStopped,
    KmsRestored,
}

impl ShutdownTransitionReason {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::PendingPageflip => "pending_pageflip",
            Self::NoPendingPageflip => "no_pending_pageflip",
            Self::PageflipComplete => "pageflip_complete",
            Self::PageflipTimeout => "pageflip_timeout",
            Self::ChildrenStopped => "children_stopped",
            Self::KmsRestored => "kms_restored",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ShutdownTransition {
    pub(crate) from: ShutdownState,
    pub(crate) to: ShutdownState,
    pub(crate) reason: ShutdownTransitionReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KmsRestoreReason {
    NoPendingPageflip,
    ConfirmedPageflipCompletion { token: u64 },
    ForcedPageflipTimeout { token: u64 },
}

impl KmsRestoreReason {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::NoPendingPageflip => "no_pending_pageflip",
            Self::ConfirmedPageflipCompletion { .. } => "confirmed_pageflip_completion",
            Self::ForcedPageflipTimeout { .. } => "forced_pageflip_timeout",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeShutdownLifecycle {
    state: ShutdownState,
    requested_at_ns: Option<u64>,
    expected_pageflip_token: Option<u64>,
    pageflip_deadline_ns: Option<u64>,
    pageflip_outcome: Option<ShutdownPageflipOutcome>,
    child_stop_started: bool,
    kms_restore_started: bool,
}

impl NativeShutdownLifecycle {
    pub(crate) const fn new() -> Self {
        Self {
            state: ShutdownState::Running,
            requested_at_ns: None,
            expected_pageflip_token: None,
            pageflip_deadline_ns: None,
            pageflip_outcome: None,
            child_stop_started: false,
            kms_restore_started: false,
        }
    }

    pub(crate) const fn state(&self) -> ShutdownState {
        self.state
    }

    pub(crate) const fn is_running(&self) -> bool {
        matches!(self.state, ShutdownState::Running)
    }

    pub(crate) const fn is_complete(&self) -> bool {
        matches!(self.state, ShutdownState::Complete)
    }

    pub(crate) const fn expected_pageflip_token(&self) -> Option<u64> {
        self.expected_pageflip_token
    }

    pub(crate) const fn pageflip_deadline_ns(&self) -> Option<u64> {
        self.pageflip_deadline_ns
    }

    #[cfg(test)]
    pub(crate) const fn pageflip_outcome(&self) -> Option<ShutdownPageflipOutcome> {
        self.pageflip_outcome
    }

    #[cfg(test)]
    pub(crate) const fn child_stop_started(&self) -> bool {
        self.child_stop_started
    }

    pub(crate) fn request_shutdown(
        &mut self,
        now_ns: u64,
        pending_pageflip_token: Option<u64>,
    ) -> Option<ShutdownTransition> {
        if self.state != ShutdownState::Running {
            return None;
        }
        self.requested_at_ns = Some(now_ns);
        self.expected_pageflip_token = pending_pageflip_token;
        self.pageflip_deadline_ns = pending_pageflip_token
            .map(|_| now_ns.saturating_add(NATIVE_SHUTDOWN_PAGEFLIP_TIMEOUT_NS));
        Some(self.transition_to(ShutdownState::Requested, ShutdownTransitionReason::Request))
    }

    pub(crate) fn advance_requested(&mut self) -> Option<ShutdownTransition> {
        if self.state != ShutdownState::Requested {
            return None;
        }
        if self.expected_pageflip_token.is_some() {
            self.transition_to(
                ShutdownState::Draining,
                ShutdownTransitionReason::PendingPageflip,
            )
            .into()
        } else {
            self.pageflip_outcome = Some(ShutdownPageflipOutcome::NoPendingPageflip);
            self.transition_to(
                ShutdownState::StoppingChildren,
                ShutdownTransitionReason::NoPendingPageflip,
            )
            .into()
        }
    }

    pub(crate) fn note_empty_nonblocking_drm_read(&mut self) -> bool {
        false
    }

    pub(crate) fn note_pageflip_event(&mut self, token: u64) -> Option<ShutdownTransition> {
        if self.state != ShutdownState::Draining {
            return None;
        }
        if self.expected_pageflip_token != Some(token) {
            return None;
        }
        self.pageflip_outcome = Some(ShutdownPageflipOutcome::ConfirmedCompletion { token });
        self.pageflip_deadline_ns = None;
        self.transition_to(
            ShutdownState::StoppingChildren,
            ShutdownTransitionReason::PageflipComplete,
        )
        .into()
    }

    pub(crate) fn advance_pageflip_timeout(&mut self, now_ns: u64) -> Option<ShutdownTransition> {
        if self.state != ShutdownState::Draining {
            return None;
        }
        let deadline = self.pageflip_deadline_ns?;
        if now_ns < deadline {
            return None;
        }
        let token = self.expected_pageflip_token?;
        self.pageflip_outcome = Some(ShutdownPageflipOutcome::ForcedTimeout { token });
        self.pageflip_deadline_ns = None;
        self.transition_to(
            ShutdownState::StoppingChildren,
            ShutdownTransitionReason::PageflipTimeout,
        )
        .into()
    }

    pub(crate) fn mark_child_stop_started(&mut self) -> bool {
        if self.state != ShutdownState::StoppingChildren || self.child_stop_started {
            return false;
        }
        self.child_stop_started = true;
        true
    }

    pub(crate) fn note_session_children_stopped(&mut self) -> Option<ShutdownTransition> {
        if self.state != ShutdownState::StoppingChildren {
            return None;
        }
        self.transition_to(
            ShutdownState::Restoring,
            ShutdownTransitionReason::ChildrenStopped,
        )
        .into()
    }

    pub(crate) fn begin_kms_restore(&mut self) -> Option<KmsRestoreReason> {
        if self.state != ShutdownState::Restoring || self.kms_restore_started {
            return None;
        }
        self.kms_restore_started = true;
        match self.pageflip_outcome? {
            ShutdownPageflipOutcome::NoPendingPageflip => Some(KmsRestoreReason::NoPendingPageflip),
            ShutdownPageflipOutcome::ConfirmedCompletion { token } => {
                Some(KmsRestoreReason::ConfirmedPageflipCompletion { token })
            }
            ShutdownPageflipOutcome::ForcedTimeout { token } => {
                Some(KmsRestoreReason::ForcedPageflipTimeout { token })
            }
        }
    }

    pub(crate) fn note_kms_restore_complete(&mut self) -> Option<ShutdownTransition> {
        if self.state != ShutdownState::Restoring || !self.kms_restore_started {
            return None;
        }
        self.transition_to(
            ShutdownState::Complete,
            ShutdownTransitionReason::KmsRestored,
        )
        .into()
    }

    fn transition_to(
        &mut self,
        to: ShutdownState,
        reason: ShutdownTransitionReason,
    ) -> ShutdownTransition {
        let from = self.state;
        self.state = to;
        ShutdownTransition { from, to, reason }
    }
}

impl Default for NativeShutdownLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn native_shutdown_debug_log(marker: &str) {
    if std::env::var_os("OBLIVION_ONE_SHUTDOWN_DEBUG").is_some() {
        eprintln!("native shutdown: {marker}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn requested_with_pending_token() -> NativeShutdownLifecycle {
        let mut shutdown = NativeShutdownLifecycle::new();
        shutdown.request_shutdown(10, Some(41)).unwrap();
        shutdown.advance_requested().unwrap();
        shutdown
    }

    #[test]
    fn shutdown_request_transitions_once() {
        let mut shutdown = NativeShutdownLifecycle::new();

        let transition = shutdown.request_shutdown(10, None).unwrap();

        assert_eq!(transition.from, ShutdownState::Running);
        assert_eq!(transition.to, ShutdownState::Requested);
        assert!(shutdown.request_shutdown(20, Some(41)).is_none());
        assert_eq!(shutdown.state(), ShutdownState::Requested);
    }

    #[test]
    fn repeated_shutdown_request_is_idempotent() {
        let mut shutdown = NativeShutdownLifecycle::new();

        shutdown.request_shutdown(10, Some(41)).unwrap();
        let deadline = shutdown.pageflip_deadline_ns();
        let token = shutdown.expected_pageflip_token();

        assert!(shutdown.request_shutdown(20, Some(99)).is_none());
        assert_eq!(shutdown.pageflip_deadline_ns(), deadline);
        assert_eq!(shutdown.expected_pageflip_token(), token);
    }

    #[test]
    fn pending_pageflip_enters_draining() {
        let mut shutdown = NativeShutdownLifecycle::new();

        shutdown.request_shutdown(10, Some(41)).unwrap();
        let transition = shutdown.advance_requested().unwrap();

        assert_eq!(transition.to, ShutdownState::Draining);
        assert_eq!(shutdown.expected_pageflip_token(), Some(41));
    }

    #[test]
    fn empty_nonblocking_drm_read_does_not_complete_shutdown() {
        let mut shutdown = requested_with_pending_token();

        assert!(!shutdown.note_empty_nonblocking_drm_read());

        assert_eq!(shutdown.state(), ShutdownState::Draining);
    }

    #[test]
    fn exact_pageflip_token_advances_shutdown() {
        let mut shutdown = requested_with_pending_token();

        let transition = shutdown.note_pageflip_event(41).unwrap();

        assert_eq!(transition.to, ShutdownState::StoppingChildren);
        assert_eq!(
            shutdown.pageflip_outcome(),
            Some(ShutdownPageflipOutcome::ConfirmedCompletion { token: 41 })
        );
    }

    #[test]
    fn unrelated_pageflip_event_does_not_advance_shutdown() {
        let mut shutdown = requested_with_pending_token();

        assert!(shutdown.note_pageflip_event(99).is_none());

        assert_eq!(shutdown.state(), ShutdownState::Draining);
    }

    #[test]
    fn pageflip_timeout_uses_forced_shutdown_path() {
        let mut shutdown = requested_with_pending_token();
        let deadline = shutdown.pageflip_deadline_ns().unwrap();

        let transition = shutdown.advance_pageflip_timeout(deadline).unwrap();

        assert_eq!(transition.to, ShutdownState::StoppingChildren);
        assert_eq!(
            shutdown.pageflip_outcome(),
            Some(ShutdownPageflipOutcome::ForcedTimeout { token: 41 })
        );
    }

    #[test]
    fn kms_restore_waits_for_pageflip_completion_or_timeout() {
        let mut shutdown = requested_with_pending_token();

        assert!(shutdown.begin_kms_restore().is_none());
        shutdown.note_pageflip_event(41).unwrap();
        shutdown.note_session_children_stopped().unwrap();

        assert_eq!(
            shutdown.begin_kms_restore(),
            Some(KmsRestoreReason::ConfirmedPageflipCompletion { token: 41 })
        );
    }

    #[test]
    fn session_children_are_stopped_during_shutdown() {
        let mut shutdown = NativeShutdownLifecycle::new();
        shutdown.request_shutdown(10, None).unwrap();
        shutdown.advance_requested().unwrap();

        assert!(shutdown.mark_child_stop_started());
        assert!(shutdown.child_stop_started());
        let transition = shutdown.note_session_children_stopped().unwrap();

        assert_eq!(transition.to, ShutdownState::Restoring);
    }

    #[test]
    fn shutdown_reaches_complete_only_after_real_teardown() {
        let mut shutdown = NativeShutdownLifecycle::new();

        assert!(shutdown.note_kms_restore_complete().is_none());
        shutdown.request_shutdown(10, None).unwrap();
        shutdown.advance_requested().unwrap();
        assert!(shutdown.note_kms_restore_complete().is_none());
        shutdown.note_session_children_stopped().unwrap();
        assert!(shutdown.note_kms_restore_complete().is_none());
        shutdown.begin_kms_restore().unwrap();
        let transition = shutdown.note_kms_restore_complete().unwrap();

        assert_eq!(transition.to, ShutdownState::Complete);
        assert!(shutdown.is_complete());
    }
}
