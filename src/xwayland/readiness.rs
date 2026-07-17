use crate::process::ManagedProcessId;

use super::XwaylandGeneration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XwaylandReadinessSnapshot {
    pub generation: XwaylandGeneration,
    pub display: u32,
    pub process_id: ManagedProcessId,
    pub elapsed_ns: u64,
    pub process_spawned: bool,
    pub process_alive: bool,
    pub displayfd_registered: bool,
    pub displayfd_readable: bool,
    pub display_number_validated: bool,
    pub private_wayland_endpoint_transferred: bool,
    pub private_client_attached: bool,
    pub private_client_authorized: bool,
    pub xwayland_shell_bound: bool,
    pub xwm_connected: bool,
    pub xwm_capabilities_validated: bool,
    pub root_initialized: bool,
    pub readiness_complete: bool,
    pub(crate) managed_profile: bool,
}

impl XwaylandReadinessSnapshot {
    pub fn missing_conditions(self) -> Vec<&'static str> {
        if self.readiness_complete {
            return Vec::new();
        }
        let mut missing = Vec::new();
        if !self.process_spawned {
            missing.push("process_spawned");
        }
        if !self.process_alive {
            missing.push("process_alive");
        }
        if !self.displayfd_registered {
            missing.push("displayfd_registered");
        }
        if !self.displayfd_readable {
            missing.push("displayfd_readable");
        }
        if !self.display_number_validated {
            missing.push("display_number_validated");
        }
        if !self.xwayland_shell_bound {
            missing.push("xwayland_shell_bound");
        }
        if self.managed_profile {
            if !self.private_wayland_endpoint_transferred {
                missing.push("private_wayland_endpoint_transferred");
            }
            if !self.private_client_attached {
                missing.push("private_client_attached");
            }
            if !self.private_client_authorized {
                missing.push("private_client_authorized");
            }
            if !self.xwm_connected {
                missing.push("xwm_connected");
            }
            if !self.xwm_capabilities_validated {
                missing.push("xwm_capabilities_validated");
            }
            if !self.root_initialized {
                missing.push("root_initialized");
            }
        }
        missing
    }
}
