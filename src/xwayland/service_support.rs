use std::{
    io,
    os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd},
    os::unix::net::UnixStream,
};

use super::super::{
    config::XwaylandProfile,
    diagnostics::{StderrRing, XwaylandExitClass},
    next_nonzero,
    readiness::XwaylandReadinessSnapshot,
};
use super::{
    ServiceState, StartingResources, XwaylandReactorPurpose, XwaylandReactorRegistration,
    XwaylandService,
};

impl XwaylandService {
    pub fn reactor_registrations(&self) -> impl Iterator<Item = XwaylandReactorRegistration> {
        let mut registrations = Vec::new();
        let generation = self.generation();
        if matches!(self.state, ServiceState::Armed)
            && let Some(lease) = self.lease.as_ref()
        {
            let (filesystem, abstract_socket) = lease.listener_fds();
            registrations.push(XwaylandReactorRegistration {
                fd: filesystem,
                generation,
                purpose: XwaylandReactorPurpose::ListenFilesystem,
                writable: false,
            });
            registrations.push(XwaylandReactorRegistration {
                fd: abstract_socket,
                generation,
                purpose: XwaylandReactorPurpose::ListenAbstract,
                writable: false,
            });
        }
        if let ServiceState::Starting(resources) = &self.state
            && !resources.display_ready
        {
            registrations.push(XwaylandReactorRegistration {
                fd: resources.displayfd.as_raw_fd(),
                generation: Some(resources.generation),
                purpose: XwaylandReactorPurpose::DisplayReady,
                writable: false,
            });
            if let Some(stderr) = resources.stderr.as_ref().filter(|stderr| stderr.active) {
                registrations.push(XwaylandReactorRegistration {
                    fd: stderr.fd.as_raw_fd(),
                    generation: Some(resources.generation),
                    purpose: XwaylandReactorPurpose::Stderr,
                    writable: false,
                });
            }
        }
        if let ServiceState::Starting(resources) = &self.state
            && let Some(startup) = resources.xwm_startup.as_ref()
            && let Some(fd) = startup.raw_fd()
        {
            registrations.push(XwaylandReactorRegistration {
                fd,
                generation: Some(resources.generation),
                purpose: XwaylandReactorPurpose::Xwm,
                writable: startup.wants_writable(),
            });
        }
        if let ServiceState::RunningBase(resources) = &self.state
            && let Some(stderr) = resources.stderr.as_ref().filter(|stderr| stderr.active)
        {
            registrations.push(XwaylandReactorRegistration {
                fd: stderr.fd.as_raw_fd(),
                generation: Some(resources.generation),
                purpose: XwaylandReactorPurpose::Stderr,
                writable: false,
            });
        }
        if let ServiceState::Running(resources) = &self.state {
            registrations.push(XwaylandReactorRegistration {
                fd: resources.xwm.raw_fd(),
                generation: Some(resources.generation),
                purpose: XwaylandReactorPurpose::Xwm,
                writable: resources.xwm.wants_writable(),
            });
            if let Some(stderr) = resources.stderr.as_ref().filter(|stderr| stderr.active) {
                registrations.push(XwaylandReactorRegistration {
                    fd: stderr.fd.as_raw_fd(),
                    generation: Some(resources.generation),
                    purpose: XwaylandReactorPurpose::Stderr,
                    writable: false,
                });
            }
        }
        registrations.into_iter()
    }

    pub(super) fn capture_failure_stderr(&mut self) {
        self.latest_failed_stderr = match &self.state {
            ServiceState::Starting(resources) => resources
                .stderr
                .as_ref()
                .map(|stderr| stderr.ring.clone())
                .unwrap_or_default(),
            ServiceState::RunningBase(resources) => resources
                .stderr
                .as_ref()
                .map(|stderr| stderr.ring.clone())
                .unwrap_or_default(),
            ServiceState::Running(resources) => resources
                .stderr
                .as_ref()
                .map(|stderr| stderr.ring.clone())
                .unwrap_or_default(),
            ServiceState::Disabled
            | ServiceState::Armed
            | ServiceState::Backoff { .. }
            | ServiceState::Failed => StderrRing::default(),
        };
    }

    pub(super) fn snapshot_for_starting(
        &self,
        resources: &StartingResources,
    ) -> XwaylandReadinessSnapshot {
        XwaylandReadinessSnapshot {
            generation: resources.generation,
            display: self.display_number().unwrap_or(0),
            process_id: resources.process.id,
            elapsed_ns: now_ns()
                .unwrap_or(resources.started_ns)
                .saturating_sub(resources.started_ns),
            process_spawned: true,
            process_alive: true,
            displayfd_registered: resources.displayfd_registered,
            displayfd_readable: resources.displayfd_readable,
            display_number_validated: resources.display_ready,
            private_wayland_endpoint_transferred: resources.private_wayland_endpoint_transferred,
            private_client_attached: resources.private_client_attached,
            private_client_authorized: resources.private_client_authorized,
            xwayland_shell_bound: resources.shell_ready,
            xwm_connected: resources.xwm_connected,
            xwm_capabilities_validated: resources.xwm_capabilities_validated,
            root_initialized: resources.root_initialized,
            readiness_complete: false,
            managed_profile: self.config.profile == XwaylandProfile::Managed,
        }
    }

    pub(super) fn log_state_transition(&self) {
        if let Some(readiness) = self.readiness_snapshot() {
            eprintln!(
                "oblivion-one xwayland: event=state_transition state={:?} generation={:?} display={:?} process_id={} process_alive={} displayfd_registered={} displayfd_readable={} display_number_validated={} private_wayland_endpoint_transferred={} private_client_attached={} private_client_authorized={} xwayland_shell_bound={} xwm_connected={} xwm_capabilities_validated={} root_initialized={} readiness_complete={} missing={:?}",
                self.state_kind(),
                self.generation(),
                self.display_number(),
                readiness.process_id.get(),
                readiness.process_alive,
                readiness.displayfd_registered,
                readiness.displayfd_readable,
                readiness.display_number_validated,
                readiness.private_wayland_endpoint_transferred,
                readiness.private_client_attached,
                readiness.private_client_authorized,
                readiness.xwayland_shell_bound,
                readiness.xwm_connected,
                readiness.xwm_capabilities_validated,
                readiness.root_initialized,
                readiness.readiness_complete,
                readiness.missing_conditions(),
            );
        } else {
            eprintln!(
                "oblivion-one xwayland: event=state_transition state={:?} generation={:?} display={:?}",
                self.state_kind(),
                self.generation(),
                self.display_number()
            );
        }
    }

    pub(super) fn log_readiness_progress(&self, stage: &str) {
        let Some(readiness) = self.readiness_snapshot() else {
            return;
        };
        eprintln!(
            "oblivion-one xwayland: event=readiness_progress stage={stage} generation={:?} display={} process_id={} process_alive={} displayfd_registered={} displayfd_readable={} display_number_validated={} private_wayland_endpoint_transferred={} private_client_attached={} private_client_authorized={} xwayland_shell_bound={} xwm_connected={} xwm_capabilities_validated={} root_initialized={} readiness_complete={} missing={:?}",
            readiness.generation,
            readiness.display,
            readiness.process_id.get(),
            readiness.process_alive,
            readiness.displayfd_registered,
            readiness.displayfd_readable,
            readiness.display_number_validated,
            readiness.private_wayland_endpoint_transferred,
            readiness.private_client_attached,
            readiness.private_client_authorized,
            readiness.xwayland_shell_bound,
            readiness.xwm_connected,
            readiness.xwm_capabilities_validated,
            readiness.root_initialized,
            readiness.readiness_complete,
            readiness.missing_conditions(),
        );
    }

    pub(crate) fn allocate_generation(&mut self) -> io::Result<super::XwaylandGeneration> {
        self.metrics.generations_started = self.metrics.generations_started.saturating_add(1);
        next_nonzero(&mut self.next_generation).ok_or_else(|| {
            self.replace_state(ServiceState::Failed);
            io::Error::other("XWayland generation identity exhausted")
        })
    }
}

pub(super) fn duplicate_fd(fd: RawFd) -> io::Result<OwnedFd> {
    let duplicate = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 100) };
    if duplicate < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { OwnedFd::from_raw_fd(duplicate) })
    }
}

pub(super) fn owned_fd_from_stream(stream: UnixStream) -> OwnedFd {
    unsafe { OwnedFd::from_raw_fd(stream.into_raw_fd()) }
}

pub(super) fn now_ns() -> io::Result<u64> {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((time.tv_sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(time.tv_nsec as u64))
}

pub(super) fn classify_exit(
    running: bool,
    compositor_requested: bool,
    success: bool,
) -> XwaylandExitClass {
    if compositor_requested {
        return if running {
            XwaylandExitClass::ExpectedShutdownAfterRunning
        } else {
            XwaylandExitClass::CompositorRequestedTermination
        };
    }
    if running && success {
        return XwaylandExitClass::ExpectedIdleExitAfterRunning;
    }
    if !running && success {
        return XwaylandExitClass::StartupExitBeforeReadiness;
    }
    XwaylandExitClass::CrashOrSignal
}
