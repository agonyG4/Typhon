use super::*;

impl XwaylandService {
    pub(crate) fn reactor_state_label(&self) -> &'static str {
        match self.state {
            ServiceState::Starting(_) => "Starting",
            ServiceState::Running(_) => "Running",
            ServiceState::Disabled
            | ServiceState::Armed
            | ServiceState::RunningBase(_)
            | ServiceState::Backoff { .. }
            | ServiceState::Failed => "Inactive",
        }
    }

    #[cfg(test)]
    pub(crate) fn has_pending_reactor_teardown(&self) -> bool {
        !self.retired_resources.is_empty() || self.retired_lease.is_some()
    }

    pub fn next_deadline_ns(&self) -> Option<u64> {
        match &self.state {
            ServiceState::Starting(resources) => Some(resources.deadline_ns),
            ServiceState::Backoff { deadline_ns, .. } => Some(*deadline_ns),
            ServiceState::Running(resources) => resources
                .xwm
                .next_resize_sync_deadline_ns()
                .into_iter()
                .chain(resources.xwm.next_focus_deadline_ns())
                .chain(resources.xwm.next_adoption_deadline_ns())
                .min(),
            ServiceState::Disabled
            | ServiceState::Armed
            | ServiceState::RunningBase(_)
            | ServiceState::Failed => None,
        }
    }

    pub fn generation(&self) -> Option<XwaylandGeneration> {
        match self.state {
            ServiceState::Starting(ref resources) => Some(resources.generation),
            ServiceState::RunningBase(ref resources) => Some(resources.generation),
            ServiceState::Running(ref resources) => Some(resources.generation),
            ServiceState::Disabled
            | ServiceState::Armed
            | ServiceState::Backoff { .. }
            | ServiceState::Failed => None,
        }
    }

    pub fn managed_xwm_fd(&self, generation: XwaylandGeneration) -> Option<RawFd> {
        match &self.state {
            ServiceState::Running(resources) if resources.generation == generation => {
                Some(resources.xwm.raw_fd())
            }
            ServiceState::Starting(resources) if resources.generation == generation => {
                resources.xwm_startup.as_ref().and_then(XwmStartup::raw_fd)
            }
            _ => None,
        }
    }

    pub fn managed_xwm_root_event_mask(&self, generation: XwaylandGeneration) -> Option<u32> {
        match &self.state {
            ServiceState::Running(resources) if resources.generation == generation => {
                resources.xwm.root_event_mask()
            }
            _ => None,
        }
    }

    pub(crate) fn xwm_reactor_token(&self, generation: XwaylandGeneration) -> Option<u64> {
        match &self.state {
            ServiceState::Starting(resources) if resources.generation == generation => {
                resources.xwm_reactor_token
            }
            ServiceState::Running(resources) if resources.generation == generation => {
                resources.xwm_reactor_token
            }
            _ => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn xwm_reactor_events_for_tests(&self) -> u64 {
        self.metrics.xwm_reactor_events
    }

    #[cfg(test)]
    pub(crate) fn stderr_forwarding_for_tests(
        &self,
        generation: XwaylandGeneration,
    ) -> Option<bool> {
        match &self.state {
            ServiceState::Starting(resources) if resources.generation == generation => {
                resources.stderr.as_ref().map(|stderr| stderr.forward)
            }
            ServiceState::RunningBase(resources) if resources.generation == generation => {
                resources.stderr.as_ref().map(|stderr| stderr.forward)
            }
            ServiceState::Running(resources) if resources.generation == generation => {
                resources.stderr.as_ref().map(|stderr| stderr.forward)
            }
            _ => None,
        }
    }
}
