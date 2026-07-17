use std::{os::fd::RawFd, path::Path, process::Command};

use super::display::DisplayLease;

#[derive(Debug, Clone, Copy)]
pub(crate) enum ChildFdTarget {
    WaylandSocket,
    Wm,
    DisplayFd,
    FilesystemListen,
    AbstractListen,
}

impl ChildFdTarget {
    pub(crate) const fn raw_fd(self) -> RawFd {
        match self {
            Self::WaylandSocket => 3,
            Self::Wm => 4,
            Self::DisplayFd => 5,
            Self::FilesystemListen => 6,
            Self::AbstractListen => 7,
        }
    }
}

pub(crate) fn build_command(binary: &Path, lease: &DisplayLease, verbose: bool) -> Command {
    let mut command = Command::new(binary);
    command
        .arg(lease.display())
        .arg("-rootless")
        .arg("-terminate")
        .arg("-nolisten")
        .arg("tcp")
        .arg("-listenfd")
        .arg(ChildFdTarget::FilesystemListen.raw_fd().to_string())
        .arg("-listenfd")
        .arg(ChildFdTarget::AbstractListen.raw_fd().to_string())
        .arg("-displayfd")
        .arg(ChildFdTarget::DisplayFd.raw_fd().to_string())
        .arg("-wm")
        .arg(ChildFdTarget::Wm.raw_fd().to_string())
        .arg("-auth")
        .arg(lease.xauthority_path())
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .env_remove("XAUTHORITY")
        .env_remove("OBLIVION_ONE_XWAYLAND_DISPLAY")
        .env(
            "WAYLAND_SOCKET",
            ChildFdTarget::WaylandSocket.raw_fd().to_string(),
        );
    if verbose {
        command.arg("-verbose").arg("3");
    }
    command
}
