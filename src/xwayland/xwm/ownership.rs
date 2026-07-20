use std::fmt;

use crate::xwayland::XwaylandGeneration;

/// Startup has no input-event timestamp yet.  ICCCM explicitly permits
/// `CurrentTime` for a selection claim; the first real server timestamp is
/// recorded by the running XWM before it makes activation decisions.
pub(crate) const STARTUP_SELECTION_TIMESTAMP: u32 = x11rb::CURRENT_TIME;

pub(crate) const fn manager_message_data(timestamp: u32, selection: u32, owner: u32) -> [u32; 5] {
    [timestamp, selection, owner, 0, 0]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OwnershipStep {
    RootRedirectPending,
    RootRedirectVerified,
    CompositeRedirectPending,
    CompositeRedirectVerified,
    SupportingWindowPending,
    SupportingWindowCreated,
    EwmhPropertiesPending,
    EwmhPropertiesInstalled,
    ExistingWindowsAdopted,
    SelectionPending,
    SelectionVerified,
    ManagerMessageQueued,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OwnershipFailureKind {
    RootSubstructureRedirect,
    CompositeRedirect,
    SupportingWindowCreation,
    EwmhProperties,
    SelectionConflict,
    SelectionVerification,
    ManagerMessage,
    Transition,
    #[allow(dead_code)]
    ConnectionLoss,
    #[allow(dead_code)]
    Timeout,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OwnershipFailure {
    pub(crate) generation: XwaylandGeneration,
    pub(crate) kind: OwnershipFailureKind,
    pub(crate) reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OwnershipTransitionError {
    pub(crate) generation: XwaylandGeneration,
    pub(crate) attempted_generation: XwaylandGeneration,
    pub(crate) current: OwnershipStep,
    pub(crate) expected: OwnershipStep,
    pub(crate) attempted: OwnershipStep,
}

impl fmt::Display for OwnershipTransitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "XWM ownership transition rejected: generation={} attempted_generation={} current={:?} expected={:?} attempted={:?}",
            self.generation.get(),
            self.attempted_generation.get(),
            self.current,
            self.expected,
            self.attempted,
        )
    }
}

impl OwnershipFailure {
    pub(crate) fn new(
        generation: XwaylandGeneration,
        kind: OwnershipFailureKind,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            generation,
            kind,
            reason: reason.into(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct OwnershipGate {
    generation: XwaylandGeneration,
    step: OwnershipStep,
    supporting_window: Option<u32>,
    selection_owner: Option<u32>,
    manager_message_queued: bool,
    failure: Option<OwnershipFailure>,
}

impl OwnershipGate {
    pub(crate) fn new(generation: XwaylandGeneration) -> Self {
        Self {
            generation,
            step: OwnershipStep::RootRedirectPending,
            supporting_window: None,
            selection_owner: None,
            manager_message_queued: false,
            failure: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn generation(&self) -> XwaylandGeneration {
        self.generation
    }

    pub(crate) fn step(&self) -> OwnershipStep {
        self.step
    }

    #[cfg(test)]
    pub(crate) fn selection_owner(&self) -> Option<u32> {
        self.selection_owner
    }

    #[cfg(test)]
    pub(crate) fn failure(&self) -> Option<&OwnershipFailure> {
        self.failure.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn manager_message_queued(&self) -> bool {
        self.manager_message_queued
    }

    pub(crate) fn running_ready(&self) -> bool {
        self.step == OwnershipStep::ManagerMessageQueued && self.manager_message_queued
    }

    fn transition(
        &mut self,
        expected: OwnershipStep,
        next: OwnershipStep,
    ) -> Result<(), OwnershipTransitionError> {
        self.transition_for_generation(self.generation, expected, next)
    }

    pub(crate) fn transition_for_generation(
        &mut self,
        generation: XwaylandGeneration,
        expected: OwnershipStep,
        next: OwnershipStep,
    ) -> Result<(), OwnershipTransitionError> {
        if generation != self.generation || self.step != expected {
            let error = OwnershipTransitionError {
                generation: self.generation,
                attempted_generation: generation,
                current: self.step,
                expected,
                attempted: next,
            };
            eprintln!(
                "event=xwm_ownership_transition_rejected generation={} current={:?} expected={:?} attempted={:?} attempted_generation={}",
                error.generation.get(),
                error.current,
                error.expected,
                error.attempted,
                error.attempted_generation.get(),
            );
            return Err(error);
        }
        eprintln!(
            "event=xwm_ownership_transition generation={} from={:?} to={:?}",
            self.generation.get(),
            expected,
            next,
        );
        self.step = next;
        Ok(())
    }

    pub(crate) fn note_root_redirect_verified(&mut self) -> Result<(), OwnershipTransitionError> {
        self.transition(
            OwnershipStep::RootRedirectPending,
            OwnershipStep::RootRedirectVerified,
        )
    }

    pub(crate) fn note_composite_redirect_requested(
        &mut self,
    ) -> Result<(), OwnershipTransitionError> {
        self.transition(
            OwnershipStep::RootRedirectVerified,
            OwnershipStep::CompositeRedirectPending,
        )
    }

    pub(crate) fn note_composite_redirect_verified(
        &mut self,
    ) -> Result<(), OwnershipTransitionError> {
        self.transition(
            OwnershipStep::CompositeRedirectPending,
            OwnershipStep::CompositeRedirectVerified,
        )
    }

    pub(crate) fn note_supporting_window_requested(
        &mut self,
    ) -> Result<(), OwnershipTransitionError> {
        self.transition(
            OwnershipStep::CompositeRedirectVerified,
            OwnershipStep::SupportingWindowPending,
        )
    }

    pub(crate) fn note_supporting_window_created(
        &mut self,
        window: u32,
    ) -> Result<(), OwnershipTransitionError> {
        self.transition(
            OwnershipStep::SupportingWindowPending,
            OwnershipStep::SupportingWindowCreated,
        )?;
        self.supporting_window = Some(window);
        Ok(())
    }

    pub(crate) fn note_ewmh_properties_requested(
        &mut self,
    ) -> Result<(), OwnershipTransitionError> {
        self.transition(
            OwnershipStep::SupportingWindowCreated,
            OwnershipStep::EwmhPropertiesPending,
        )
    }

    pub(crate) fn note_ewmh_properties_installed(
        &mut self,
    ) -> Result<(), OwnershipTransitionError> {
        self.transition(
            OwnershipStep::EwmhPropertiesPending,
            OwnershipStep::EwmhPropertiesInstalled,
        )
    }

    pub(crate) fn note_existing_windows_adopted(&mut self) -> Result<(), OwnershipTransitionError> {
        self.transition(
            OwnershipStep::EwmhPropertiesInstalled,
            OwnershipStep::ExistingWindowsAdopted,
        )
    }

    pub(crate) fn note_selection_claim_requested(
        &mut self,
    ) -> Result<(), OwnershipTransitionError> {
        self.transition(
            OwnershipStep::ExistingWindowsAdopted,
            OwnershipStep::SelectionPending,
        )
    }

    pub(crate) fn note_selection_owner_verified(
        &mut self,
        owner: u32,
    ) -> Result<(), OwnershipTransitionError> {
        if self.supporting_window != Some(owner) {
            return Err(OwnershipTransitionError {
                generation: self.generation,
                attempted_generation: self.generation,
                current: self.step,
                expected: OwnershipStep::SelectionPending,
                attempted: OwnershipStep::SelectionVerified,
            });
        }
        self.transition(
            OwnershipStep::SelectionPending,
            OwnershipStep::SelectionVerified,
        )?;
        self.selection_owner = Some(owner);
        Ok(())
    }

    pub(crate) fn queue_manager_message(&mut self) -> Result<(), OwnershipTransitionError> {
        self.transition(
            OwnershipStep::SelectionVerified,
            OwnershipStep::ManagerMessageQueued,
        )?;
        self.manager_message_queued = true;
        Ok(())
    }

    pub(crate) fn fail(&mut self, failure: OwnershipFailure) {
        if failure.generation == self.generation {
            self.failure = Some(failure);
            self.step = OwnershipStep::Failed;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use super::super::super::XwaylandGeneration;
    use super::{
        OwnershipFailure, OwnershipFailureKind, OwnershipGate, OwnershipStep,
        STARTUP_SELECTION_TIMESTAMP, manager_message_data,
    };

    fn gate() -> OwnershipGate {
        OwnershipGate::new(XwaylandGeneration::new(
            NonZeroU64::new(7).expect("nonzero generation"),
        ))
    }

    fn selection_verified(gate: &mut OwnershipGate) {
        gate.note_root_redirect_verified().unwrap();
        gate.note_composite_redirect_requested().unwrap();
        gate.note_composite_redirect_verified().unwrap();
        gate.note_supporting_window_requested().unwrap();
        gate.note_supporting_window_created(42).unwrap();
        gate.note_ewmh_properties_requested().unwrap();
        gate.note_ewmh_properties_installed().unwrap();
        gate.note_existing_windows_adopted().unwrap();
        gate.note_selection_claim_requested().unwrap();
        gate.note_selection_owner_verified(42).unwrap();
    }

    #[test]
    fn xwm_does_not_reach_running_before_wm_s0_verification() {
        let mut gate = gate();
        selection_verified(&mut gate);

        assert_eq!(gate.step(), OwnershipStep::SelectionVerified);
        assert!(!gate.running_ready());

        gate.queue_manager_message().unwrap();
        assert!(gate.running_ready());
    }

    #[test]
    fn full_production_ownership_sequence_reaches_running_only_after_manager_message() {
        let mut gate = gate();
        assert!(!gate.running_ready());

        gate.note_root_redirect_verified().unwrap();
        gate.note_composite_redirect_requested().unwrap();
        gate.note_composite_redirect_verified().unwrap();
        gate.note_supporting_window_requested().unwrap();
        gate.note_supporting_window_created(42).unwrap();
        gate.note_ewmh_properties_requested().unwrap();
        gate.note_ewmh_properties_installed().unwrap();
        gate.note_existing_windows_adopted().unwrap();
        gate.note_selection_claim_requested().unwrap();
        gate.note_selection_owner_verified(42).unwrap();

        assert_eq!(gate.step(), OwnershipStep::SelectionVerified);
        assert!(!gate.running_ready());

        gate.queue_manager_message().unwrap();
        assert_eq!(gate.step(), OwnershipStep::ManagerMessageQueued);
        assert!(gate.running_ready());
    }

    #[test]
    fn wm_s0_is_claimed_by_supporting_window() {
        let mut gate = gate();
        gate.note_root_redirect_verified().unwrap();
        gate.note_composite_redirect_requested().unwrap();
        gate.note_composite_redirect_verified().unwrap();
        gate.note_supporting_window_requested().unwrap();
        gate.note_supporting_window_created(42).unwrap();
        gate.note_ewmh_properties_requested().unwrap();
        gate.note_ewmh_properties_installed().unwrap();
        gate.note_existing_windows_adopted().unwrap();
        gate.note_selection_claim_requested().unwrap();
        assert_eq!(gate.step(), OwnershipStep::SelectionPending);

        gate.note_selection_owner_verified(42).unwrap();
        assert_eq!(gate.selection_owner(), Some(42));
        assert_eq!(gate.step(), OwnershipStep::SelectionVerified);
    }

    #[test]
    fn manager_client_message_is_emitted_after_selection_claim() {
        let mut gate = gate();
        assert!(gate.queue_manager_message().is_err());
        selection_verified(&mut gate);
        gate.queue_manager_message().unwrap();
        assert!(gate.manager_message_queued());
        assert_eq!(
            manager_message_data(STARTUP_SELECTION_TIMESTAMP, 11, 42),
            [0, 11, 42, 0, 0]
        );
    }

    #[test]
    fn invalid_ownership_transitions_are_rejected() {
        let mut root_gate = gate();
        root_gate.note_root_redirect_verified().unwrap();

        let error = root_gate.note_composite_redirect_verified().unwrap_err();
        assert_eq!(error.current, OwnershipStep::RootRedirectVerified);
        assert_eq!(error.expected, OwnershipStep::CompositeRedirectPending);
        assert_eq!(error.attempted, OwnershipStep::CompositeRedirectVerified);

        let mut ewmh_gate = gate();
        ewmh_gate.note_root_redirect_verified().unwrap();
        ewmh_gate.note_composite_redirect_requested().unwrap();
        ewmh_gate.note_composite_redirect_verified().unwrap();
        ewmh_gate.note_supporting_window_requested().unwrap();
        ewmh_gate.note_supporting_window_created(42).unwrap();
        let error = ewmh_gate.note_ewmh_properties_installed().unwrap_err();
        assert_eq!(error.current, OwnershipStep::SupportingWindowCreated);
        assert_eq!(error.expected, OwnershipStep::EwmhPropertiesPending);

        let mut selection_gate = gate();
        selection_gate.note_root_redirect_verified().unwrap();
        selection_gate.note_composite_redirect_requested().unwrap();
        selection_gate.note_composite_redirect_verified().unwrap();
        selection_gate.note_supporting_window_requested().unwrap();
        selection_gate.note_supporting_window_created(42).unwrap();
        selection_gate.note_ewmh_properties_requested().unwrap();
        selection_gate.note_ewmh_properties_installed().unwrap();
        selection_gate.note_existing_windows_adopted().unwrap();
        let error = selection_gate
            .note_selection_owner_verified(42)
            .unwrap_err();
        assert_eq!(error.current, OwnershipStep::ExistingWindowsAdopted);
        assert_eq!(error.expected, OwnershipStep::SelectionPending);

        let mut manager_gate = gate();
        selection_verified(&mut manager_gate);
        manager_gate.queue_manager_message().unwrap();
        let error = manager_gate.queue_manager_message().unwrap_err();
        assert_eq!(error.current, OwnershipStep::ManagerMessageQueued);
        assert_eq!(error.expected, OwnershipStep::SelectionVerified);
    }

    #[test]
    fn stale_generation_transition_is_rejected() {
        let mut gate = gate();
        let stale = XwaylandGeneration::new(NonZeroU64::new(8).unwrap());
        let error = gate
            .transition_for_generation(
                stale,
                OwnershipStep::RootRedirectPending,
                OwnershipStep::RootRedirectVerified,
            )
            .unwrap_err();

        assert_eq!(error.generation.get(), 7);
        assert_eq!(error.attempted_generation.get(), 8);
        assert_eq!(gate.step(), OwnershipStep::RootRedirectPending);
    }

    #[test]
    fn selection_ownership_failure_fails_only_generation() {
        let mut gate = gate();
        selection_verified(&mut gate);
        assert_eq!(gate.step(), OwnershipStep::SelectionVerified);
        gate.fail(OwnershipFailure::new(
            gate.generation(),
            OwnershipFailureKind::SelectionConflict,
            "another WM owns WM_S0",
        ));

        assert_eq!(gate.step(), OwnershipStep::Failed);
        assert_eq!(
            gate.failure().map(|failure| failure.kind),
            Some(OwnershipFailureKind::SelectionConflict)
        );
        assert_eq!(gate.generation().get(), 7);
    }

    #[test]
    fn root_substructure_bad_access_is_contained() {
        let mut gate = gate();
        gate.fail(OwnershipFailure::new(
            gate.generation(),
            OwnershipFailureKind::RootSubstructureRedirect,
            "BadAccess",
        ));
        assert_eq!(gate.step(), OwnershipStep::Failed);
        assert_eq!(
            gate.failure().map(|failure| failure.kind),
            Some(OwnershipFailureKind::RootSubstructureRedirect)
        );
    }

    #[test]
    fn composite_redirect_error_is_contained() {
        let mut gate = gate();
        gate.fail(OwnershipFailure::new(
            gate.generation(),
            OwnershipFailureKind::CompositeRedirect,
            "BadMatch",
        ));
        assert_eq!(gate.step(), OwnershipStep::Failed);
        assert_eq!(
            gate.failure().map(|failure| failure.kind),
            Some(OwnershipFailureKind::CompositeRedirect)
        );
    }
}
