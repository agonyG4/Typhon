use super::super::XwaylandDndOfferId;
use super::*;

impl XwaylandService {
    /// Acknowledge the canonical result for one admitted incoming wire Drop.
    /// The XWM starts its long terminal deadline only after this exact result.
    pub fn resolve_managed_incoming_dnd_drop(
        &mut self,
        offer_id: XwaylandDndOfferId,
        accepted: bool,
        supervisor: &mut ChildSupervisor,
    ) -> io::Result<()> {
        let result = match &mut self.state {
            ServiceState::Running(resources) if resources.generation == offer_id.generation() => {
                resources
                    .xwm
                    .resolve_incoming_dnd_drop(offer_id, accepted, now_ns()?)
                    .map(|()| true)
            }
            _ => return Ok(()),
        };
        match result {
            Ok(true) => self.bump_reactor_registration_generation(),
            Ok(false) => {}
            Err(error) => self.fail_managed_xwm(
                supervisor,
                XwaylandFailureStage::CommandFlush,
                io::Error::other(error),
            ),
        }
        Ok(())
    }

    /// Return the result of a generation/offer-qualified post-Drop terminal
    /// cancellation. False means canonical state was already gone and the XWM
    /// must fail its bounded wire operation closed.
    pub fn resolve_managed_incoming_dnd_cancel_after_drop(
        &mut self,
        offer_id: XwaylandDndOfferId,
        cancelled: bool,
        supervisor: &mut ChildSupervisor,
    ) -> io::Result<()> {
        let result = match &mut self.state {
            ServiceState::Running(resources) if resources.generation == offer_id.generation() => {
                resources
                    .xwm
                    .resolve_incoming_dnd_cancel_after_drop(offer_id, cancelled)
                    .map(|()| true)
            }
            _ => return Ok(()),
        };
        match result {
            Ok(true) => self.bump_reactor_registration_generation(),
            Ok(false) => {}
            Err(error) => self.fail_managed_xwm(
                supervisor,
                XwaylandFailureStage::CommandFlush,
                io::Error::other(error),
            ),
        }
        Ok(())
    }
}
