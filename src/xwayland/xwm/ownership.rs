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

    pub(crate) fn note_root_redirect_verified(&mut self) {
        if self.step == OwnershipStep::RootRedirectPending {
            self.step = OwnershipStep::RootRedirectVerified;
        }
    }

    pub(crate) fn note_composite_redirect_requested(&mut self) {
        if self.step == OwnershipStep::RootRedirectVerified {
            self.step = OwnershipStep::CompositeRedirectPending;
        }
    }

    pub(crate) fn note_composite_redirect_verified(&mut self) {
        if self.step == OwnershipStep::RootRedirectVerified {
            self.step = OwnershipStep::CompositeRedirectVerified;
        }
    }

    pub(crate) fn note_supporting_window_created(&mut self, window: u32) {
        if matches!(
            self.step,
            OwnershipStep::CompositeRedirectVerified | OwnershipStep::SupportingWindowPending
        ) {
            self.supporting_window = Some(window);
            self.step = OwnershipStep::SupportingWindowCreated;
        }
    }

    pub(crate) fn note_supporting_window_requested(&mut self) {
        if self.step == OwnershipStep::CompositeRedirectVerified {
            self.step = OwnershipStep::SupportingWindowPending;
        }
    }

    pub(crate) fn note_ewmh_properties_installed(&mut self) {
        if self.step == OwnershipStep::SupportingWindowCreated {
            self.step = OwnershipStep::EwmhPropertiesInstalled;
        }
    }

    pub(crate) fn note_ewmh_properties_requested(&mut self) {
        if self.step == OwnershipStep::SupportingWindowCreated {
            self.step = OwnershipStep::EwmhPropertiesPending;
        }
    }

    pub(crate) fn note_existing_windows_adopted(&mut self) {
        if self.step == OwnershipStep::EwmhPropertiesInstalled {
            self.step = OwnershipStep::ExistingWindowsAdopted;
        }
    }

    pub(crate) fn note_selection_claim_requested(&mut self) {
        if matches!(
            self.step,
            OwnershipStep::ExistingWindowsAdopted | OwnershipStep::SupportingWindowCreated
        ) {
            self.step = OwnershipStep::SelectionPending;
        }
    }

    pub(crate) fn note_selection_owner_verified(&mut self, owner: u32) {
        if self.step == OwnershipStep::SelectionPending && self.supporting_window == Some(owner) {
            self.selection_owner = Some(owner);
            self.step = OwnershipStep::SelectionVerified;
        }
    }

    pub(crate) fn queue_manager_message(&mut self) -> bool {
        if self.step != OwnershipStep::SelectionVerified {
            return false;
        }
        self.note_manager_message_queued();
        true
    }

    pub(crate) fn note_manager_message_queued(&mut self) {
        if self.step == OwnershipStep::SelectionVerified {
            self.manager_message_queued = true;
            self.step = OwnershipStep::ManagerMessageQueued;
        }
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

    #[test]
    fn xwm_does_not_reach_running_before_wm_s0_verification() {
        let mut gate = gate();
        gate.note_root_redirect_verified();
        gate.note_composite_redirect_verified();
        gate.note_supporting_window_created(42);
        gate.note_ewmh_properties_installed();
        gate.note_existing_windows_adopted();
        gate.note_selection_claim_requested();
        gate.note_selection_owner_verified(42);

        assert_eq!(gate.step(), OwnershipStep::SelectionVerified);
        assert!(!gate.running_ready());

        gate.note_manager_message_queued();
        assert!(gate.running_ready());
    }

    #[test]
    fn wm_s0_is_claimed_by_supporting_window() {
        let mut gate = gate();
        gate.note_root_redirect_verified();
        gate.note_composite_redirect_verified();
        gate.note_supporting_window_created(42);
        gate.note_selection_claim_requested();
        assert_eq!(gate.step(), OwnershipStep::SelectionPending);

        gate.note_selection_owner_verified(42);
        assert_eq!(gate.selection_owner(), Some(42));
        assert_eq!(gate.step(), OwnershipStep::SelectionVerified);
    }

    #[test]
    fn manager_client_message_is_emitted_after_selection_claim() {
        let mut gate = gate();
        gate.note_root_redirect_verified();
        gate.note_composite_redirect_verified();
        gate.note_supporting_window_created(42);
        assert!(!gate.queue_manager_message());
        gate.note_selection_claim_requested();
        assert!(!gate.queue_manager_message());
        gate.note_selection_owner_verified(42);
        assert!(gate.queue_manager_message());
        assert!(gate.manager_message_queued());
        assert_eq!(
            manager_message_data(STARTUP_SELECTION_TIMESTAMP, 11, 42),
            [0, 11, 42, 0, 0]
        );
    }

    #[test]
    fn selection_ownership_failure_fails_only_generation() {
        let mut gate = gate();
        gate.note_supporting_window_created(42);
        gate.note_selection_claim_requested();
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
