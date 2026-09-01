use crate::native_output::input::NativeInputRoutingTransition;

pub const MAX_INPUT_ROUTING_GUARD_CHECKPOINTS: u8 = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum NativeInputRoutingGuardCheckpoint {
    AfterTransition = 0,
    BeforeSurfacePacing = 1,
    BeforeCursorAndControl = 2,
    BeforeXwaylandScene = 3,
    BeforeAcquirePrepare = 4,
    BeforePresentation = 5,
}

impl NativeInputRoutingGuardCheckpoint {
    pub(super) const fn index(self) -> u8 {
        self as u8
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeRoutingGuardDecision {
    NoBarrier,
    ContinueCycleTail,
    ServiceFreshInput,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GuardState {
    Disarmed,
    Armed {
        transition: NativeInputRoutingTransition,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeInputTransitionLatencyGuard {
    state: GuardState,
    checkpoints_used: u8,
}

impl Default for NativeInputTransitionLatencyGuard {
    fn default() -> Self {
        Self {
            state: GuardState::Disarmed,
            checkpoints_used: 0,
        }
    }
}

impl NativeInputTransitionLatencyGuard {
    pub(super) fn arm(&mut self, transition: NativeInputRoutingTransition) {
        self.state = GuardState::Armed { transition };
        self.checkpoints_used = 0;
    }

    pub(super) const fn armed(&self) -> bool {
        matches!(self.state, GuardState::Armed { .. })
    }

    #[cfg(test)]
    pub(super) const fn checkpoints_used(&self) -> u8 {
        self.checkpoints_used
    }

    pub(super) fn checkpoint(
        &mut self,
        _checkpoint: NativeInputRoutingGuardCheckpoint,
        input_serviceable: bool,
    ) -> NativeRoutingGuardDecision {
        let GuardState::Armed { transition } = self.state else {
            return NativeRoutingGuardDecision::NoBarrier;
        };

        if self.checkpoints_used >= MAX_INPUT_ROUTING_GUARD_CHECKPOINTS {
            self.state = GuardState::Disarmed;
            return NativeRoutingGuardDecision::ContinueCycleTail;
        }

        self.checkpoints_used = self.checkpoints_used.saturating_add(1);
        if input_serviceable {
            self.state = GuardState::Disarmed;
            NativeRoutingGuardDecision::ServiceFreshInput
        } else if self.checkpoints_used >= MAX_INPUT_ROUTING_GUARD_CHECKPOINTS {
            self.state = GuardState::Disarmed;
            NativeRoutingGuardDecision::ContinueCycleTail
        } else {
            self.state = GuardState::Armed { transition };
            NativeRoutingGuardDecision::ContinueCycleTail
        }
    }

    pub(super) fn checkpoint_with_readiness<E>(
        &mut self,
        checkpoint: NativeInputRoutingGuardCheckpoint,
        readiness: impl FnOnce() -> Result<bool, E>,
    ) -> Result<NativeRoutingGuardDecision, E> {
        if !self.armed() {
            return Ok(NativeRoutingGuardDecision::NoBarrier);
        }
        if self.checkpoints_used >= MAX_INPUT_ROUTING_GUARD_CHECKPOINTS {
            return Ok(self.checkpoint(checkpoint, false));
        }
        Ok(self.checkpoint(checkpoint, readiness()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_output::input::NativeInputRoutingTransition;
    use oblivion_one::compositor::PointerConstraintBackendId;

    fn transition() -> NativeInputRoutingTransition {
        NativeInputRoutingTransition::LockedActivated(PointerConstraintBackendId {
            constraint_id: 7,
            generation: 3,
        })
    }

    #[test]
    fn input_arriving_after_checkpoint_zero_preempts_before_next_tail_phase() {
        let mut guard = NativeInputTransitionLatencyGuard::default();
        let mut order = Vec::new();
        guard.arm(transition());
        order.push("transition");

        assert_eq!(
            guard
                .checkpoint_with_readiness(
                    NativeInputRoutingGuardCheckpoint::AfterTransition,
                    || Ok::<bool, std::convert::Infallible>(false),
                )
                .unwrap(),
            NativeRoutingGuardDecision::ContinueCycleTail
        );
        order.push("checkpoint(false)");
        order.push("cheap_tail");
        assert_eq!(
            guard
                .checkpoint_with_readiness(
                    NativeInputRoutingGuardCheckpoint::BeforeSurfacePacing,
                    || Ok::<bool, std::convert::Infallible>(true),
                )
                .unwrap(),
            NativeRoutingGuardDecision::ServiceFreshInput
        );
        order.push("fresh_input");
        order.push("expensive_tail");

        assert_eq!(
            order,
            vec![
                "transition",
                "checkpoint(false)",
                "cheap_tail",
                "fresh_input",
                "expensive_tail"
            ]
        );
        assert_eq!(guard.checkpoints_used(), 2);
        assert_eq!(
            guard
                .checkpoint_with_readiness(
                    NativeInputRoutingGuardCheckpoint::BeforeCursorAndControl,
                    || Ok::<bool, std::convert::Infallible>(true),
                )
                .unwrap(),
            NativeRoutingGuardDecision::NoBarrier
        );
    }

    #[test]
    fn no_input_finishes_after_a_fixed_number_of_checkpoints() {
        let mut guard = NativeInputTransitionLatencyGuard::default();
        guard.arm(transition());

        for checkpoint in [
            NativeInputRoutingGuardCheckpoint::AfterTransition,
            NativeInputRoutingGuardCheckpoint::BeforeSurfacePacing,
            NativeInputRoutingGuardCheckpoint::BeforeCursorAndControl,
            NativeInputRoutingGuardCheckpoint::BeforeXwaylandScene,
            NativeInputRoutingGuardCheckpoint::BeforeAcquirePrepare,
            NativeInputRoutingGuardCheckpoint::BeforePresentation,
        ] {
            assert_eq!(
                guard
                    .checkpoint_with_readiness(checkpoint, || {
                        Ok::<bool, std::convert::Infallible>(false)
                    })
                    .unwrap(),
                NativeRoutingGuardDecision::ContinueCycleTail
            );
        }

        assert_eq!(
            guard.checkpoints_used(),
            MAX_INPUT_ROUTING_GUARD_CHECKPOINTS
        );
        assert_eq!(
            guard
                .checkpoint_with_readiness(
                    NativeInputRoutingGuardCheckpoint::BeforePresentation,
                    || Ok::<bool, std::convert::Infallible>(false),
                )
                .unwrap(),
            NativeRoutingGuardDecision::NoBarrier
        );
    }

    #[test]
    fn no_transition_has_no_guard_checkpoints() {
        let mut guard = NativeInputTransitionLatencyGuard::default();
        let probe_called = std::cell::Cell::new(false);

        assert_eq!(
            guard
                .checkpoint_with_readiness(
                    NativeInputRoutingGuardCheckpoint::AfterTransition,
                    || {
                        probe_called.set(true);
                        Ok::<bool, std::convert::Infallible>(true)
                    },
                )
                .unwrap(),
            NativeRoutingGuardDecision::NoBarrier
        );
        assert_eq!(guard.checkpoints_used(), 0);
        assert!(!probe_called.get());
    }

    #[test]
    fn replacing_a_transition_restarts_its_bounded_guard_budget() {
        let mut guard = NativeInputTransitionLatencyGuard::default();
        guard.arm(transition());
        assert_eq!(
            guard
                .checkpoint_with_readiness(
                    NativeInputRoutingGuardCheckpoint::AfterTransition,
                    || Ok::<bool, std::convert::Infallible>(false),
                )
                .unwrap(),
            NativeRoutingGuardDecision::ContinueCycleTail
        );

        guard.arm(NativeInputRoutingTransition::ConfinedActivated(
            PointerConstraintBackendId {
                constraint_id: 8,
                generation: 4,
            },
        ));
        assert_eq!(guard.checkpoints_used(), 0);
        assert!(guard.armed());
    }
}
