use std::os::fd::RawFd;

use crate::shell_quote;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XWaylandLaunchPlan {
    pub program: String,
    pub display: String,
    pub wayland_display: String,
    pub args: Vec<String>,
}

impl XWaylandLaunchPlan {
    pub fn new(
        display: impl Into<String>,
        wayland_display: impl Into<String>,
        listen_fd: RawFd,
        wm_fd: RawFd,
        display_fd: RawFd,
    ) -> Self {
        let display = display.into();
        let wayland_display = wayland_display.into();
        let args = vec![
            display.clone(),
            "-rootless".to_string(),
            "-terminate".to_string(),
            "-listenfd".to_string(),
            listen_fd.to_string(),
            "-wm".to_string(),
            wm_fd.to_string(),
            "-displayfd".to_string(),
            display_fd.to_string(),
        ];

        Self {
            program: "Xwayland".to_string(),
            display,
            wayland_display,
            args,
        }
    }

    pub fn env_pairs(&self) -> [(&'static str, String); 1] {
        [("WAYLAND_DISPLAY", self.wayland_display.clone())]
    }

    pub fn display_command(&self) -> String {
        std::iter::once(shell_quote(&self.program))
            .chain(self.args.iter().map(|arg| shell_quote(arg)))
            .collect::<Vec<_>>()
            .join(" ")
    }
}
