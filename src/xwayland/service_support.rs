use std::{
    io,
    os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd},
    os::unix::net::UnixStream,
    time::{Duration, Instant},
};

use crate::process::ChildSupervisor;

use super::super::{
    config::XwaylandProfile,
    diagnostics::{StderrRing, XwaylandExitClass},
    next_nonzero,
    readiness::XwaylandReadinessSnapshot,
    xwm::startup::XwmStartup,
};
use super::{
    RetiredResources, RunningResources, ServiceState, StartingResources, XwaylandGeneration,
    XwaylandReactorOwner, XwaylandReactorPurpose, XwaylandReactorRegistration, XwaylandService,
};

const STDERR_MAX_BUFFER: usize = 64 * 1024;
const STDERR_MAX_LINE: usize = 8 * 1024;
const STDERR_EVENT_MAX_BYTES: usize = 64 * 1024;
const STDERR_EVENT_MAX_NS: u64 = 5_000_000;
const STDERR_FINAL_DRAIN_MAX_BYTES: usize = 64 * 1024;
const STDERR_FINAL_DRAIN_MAX_NS: u64 = 50_000_000;

#[derive(Debug)]
pub(super) struct StderrPipe {
    pub(super) fd: OwnedFd,
    pub(super) buffer: Vec<u8>,
    pub(super) active: bool,
    pub(super) forward: bool,
    pub(super) failed: bool,
    pub(super) final_drain_deadline: Option<Instant>,
    pub(super) ring: StderrRing,
}

impl StderrPipe {
    pub(super) fn new(fd: OwnedFd, forward: bool) -> Self {
        Self {
            fd,
            buffer: Vec::new(),
            active: true,
            forward,
            failed: false,
            final_drain_deadline: None,
            ring: StderrRing::default(),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct StderrDrain {
    pub(crate) bytes_read: u64,
    pub(crate) truncated_chunks: u64,
    pub(crate) closed: bool,
    pub(crate) lines: Vec<(String, bool)>,
}

pub(crate) fn drain_stderr_pipe(
    stderr: &mut StderrPipe,
    max_bytes: usize,
    deadline: Instant,
) -> StderrDrain {
    let mut drain = StderrDrain::default();
    let mut bytes = [0u8; 4096];
    while stderr.active && (drain.bytes_read as usize) < max_bytes && Instant::now() < deadline {
        let remaining = max_bytes.saturating_sub(drain.bytes_read as usize);
        let read_len = remaining.min(bytes.len());
        // SAFETY: stderr.fd is owned by the generation and remains alive for
        // the duration of this nonblocking drain.
        let read =
            unsafe { libc::read(stderr.fd.as_raw_fd(), bytes.as_mut_ptr().cast(), read_len) };
        if read < 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::WouldBlock {
                stderr.active = false;
                drain.closed = true;
            }
            break;
        }
        if read == 0 {
            stderr.active = false;
            drain.closed = true;
            break;
        }
        let read = read as usize;
        drain.bytes_read = drain.bytes_read.saturating_add(read as u64);
        stderr.ring.push(&bytes[..read]);
        stderr.buffer.extend_from_slice(&bytes[..read]);
        if stderr.buffer.len() > STDERR_MAX_BUFFER {
            let excess = stderr.buffer.len() - STDERR_MAX_BUFFER;
            stderr.buffer.drain(..excess);
            drain.truncated_chunks = drain.truncated_chunks.saturating_add(1);
        }
        while let Some(newline) = stderr.buffer.iter().position(|byte| *byte == b'\n') {
            let raw = stderr.buffer.drain(..=newline).collect::<Vec<_>>();
            let text = String::from_utf8_lossy(&raw[..raw.len().saturating_sub(1)]).into_owned();
            drain.lines.push((text, false));
        }
        while stderr.buffer.len() > STDERR_MAX_LINE {
            let raw = stderr.buffer.drain(..STDERR_MAX_LINE).collect::<Vec<_>>();
            drain
                .lines
                .push((String::from_utf8_lossy(&raw).into_owned(), true));
        }
    }
    if drain.closed && !stderr.buffer.is_empty() {
        let raw = std::mem::take(&mut stderr.buffer);
        drain
            .lines
            .push((String::from_utf8_lossy(&raw).into_owned(), false));
    }
    drain
}

impl XwaylandService {
    pub(crate) fn handle_stderr_ready_impl(
        &mut self,
        generation: super::XwaylandGeneration,
    ) -> io::Result<()> {
        let (process_id, forward, drain) = {
            let (resources_generation, process_id, stderr) = match &mut self.state {
                ServiceState::Starting(resources) => (
                    resources.generation,
                    resources.process.id,
                    resources.stderr.as_mut(),
                ),
                ServiceState::RunningBase(resources) => (
                    resources.generation,
                    resources.process.id,
                    resources.stderr.as_mut(),
                ),
                ServiceState::Running(resources) => (
                    resources.generation,
                    resources.process.id,
                    resources.stderr.as_mut(),
                ),
                _ => return Ok(()),
            };
            if resources_generation != generation {
                self.metrics.stale_events = self.metrics.stale_events.saturating_add(1);
                return Ok(());
            }
            let Some(stderr) = stderr else {
                return Ok(());
            };
            if !stderr.active {
                return Ok(());
            }
            (
                process_id,
                stderr.forward,
                drain_stderr_pipe(
                    stderr,
                    STDERR_EVENT_MAX_BYTES,
                    Instant::now() + Duration::from_nanos(STDERR_EVENT_MAX_NS),
                ),
            )
        };
        self.record_stderr_drain(generation, process_id, forward, drain);
        Ok(())
    }

    pub(super) fn record_stderr_drain(
        &mut self,
        generation: super::XwaylandGeneration,
        process_id: super::ManagedProcessId,
        forward: bool,
        drain: StderrDrain,
    ) {
        self.metrics.stderr_bytes = self.metrics.stderr_bytes.saturating_add(drain.bytes_read);
        self.metrics.stderr_truncated = self
            .metrics
            .stderr_truncated
            .saturating_add(drain.truncated_chunks);
        if drain.closed {
            self.metrics.stderr_closed = self.metrics.stderr_closed.saturating_add(1);
        }
        for (line, truncated) in drain.lines {
            self.metrics.stderr_lines = self.metrics.stderr_lines.saturating_add(1);
            if truncated {
                self.metrics.stderr_truncated = self.metrics.stderr_truncated.saturating_add(1);
            }
            if forward {
                eprintln!(
                    "oblivion-one xwayland: event=stderr generation={generation:?} process_id={} truncated={} line={line}",
                    process_id.get(),
                    truncated,
                );
            }
        }
    }

    /// Release retired generation resources only after the runtime has removed
    /// their reactor registrations. Retired stderr remains unregistered but
    /// owned until EOF or the strict final-drain deadline.
    pub fn finish_reactor_teardown(&mut self) -> io::Result<()> {
        let mut final_stderr = Vec::new();
        let mut retained_resources = Vec::new();
        while let Some(resources) = self.retired_resources.pop() {
            let mut resources = resources;
            let drained = match &mut resources {
                RetiredResources::Starting(resources) => resources.stderr.as_mut().map(|stderr| {
                    let deadline = *stderr.final_drain_deadline.get_or_insert_with(|| {
                        Instant::now() + Duration::from_nanos(STDERR_FINAL_DRAIN_MAX_NS)
                    });
                    let drain = drain_stderr_pipe(stderr, STDERR_FINAL_DRAIN_MAX_BYTES, deadline);
                    (
                        resources.generation,
                        resources.process.id,
                        stderr.forward,
                        stderr.failed,
                        stderr.ring.clone(),
                        drain,
                        stderr.active && Instant::now() < deadline,
                    )
                }),
                RetiredResources::RunningBase(resources) => {
                    resources.stderr.as_mut().map(|stderr| {
                        let deadline = *stderr.final_drain_deadline.get_or_insert_with(|| {
                            Instant::now() + Duration::from_nanos(STDERR_FINAL_DRAIN_MAX_NS)
                        });
                        let drain =
                            drain_stderr_pipe(stderr, STDERR_FINAL_DRAIN_MAX_BYTES, deadline);
                        (
                            resources.generation,
                            resources.process.id,
                            stderr.forward,
                            stderr.failed,
                            stderr.ring.clone(),
                            drain,
                            stderr.active && Instant::now() < deadline,
                        )
                    })
                }
                RetiredResources::Running(resources) => resources.stderr.as_mut().map(|stderr| {
                    let deadline = *stderr.final_drain_deadline.get_or_insert_with(|| {
                        Instant::now() + Duration::from_nanos(STDERR_FINAL_DRAIN_MAX_NS)
                    });
                    let drain = drain_stderr_pipe(stderr, STDERR_FINAL_DRAIN_MAX_BYTES, deadline);
                    (
                        resources.generation,
                        resources.process.id,
                        stderr.forward,
                        stderr.failed,
                        stderr.ring.clone(),
                        drain,
                        stderr.active && Instant::now() < deadline,
                    )
                }),
            };
            if let Some((generation, process_id, forward, failed, ring, drain, retain)) = drained {
                if retain {
                    retained_resources.push(resources);
                    continue;
                }
                final_stderr.push((generation, process_id, forward, failed, ring, drain));
            }
            match resources {
                RetiredResources::Starting(resources) => drop(resources),
                RetiredResources::RunningBase(resources) => drop(resources),
                RetiredResources::Running(resources) => drop(resources),
            }
        }
        self.retired_resources = retained_resources;
        let mut published_failed_stderr = false;
        for (generation, process_id, forward, failed, ring, drain) in final_stderr {
            self.record_stderr_drain(generation, process_id, forward, drain);
            if failed && !published_failed_stderr {
                self.latest_failed_stderr = ring;
                published_failed_stderr = true;
            }
        }
        if self.retired_resources.is_empty() {
            drop(self.retired_lease.take());
        }
        Ok(())
    }

    pub(crate) fn mark_stderr_failure(&mut self) {
        match &mut self.state {
            ServiceState::Starting(resources) => {
                if let Some(stderr) = resources.stderr.as_mut() {
                    stderr.failed = true;
                }
            }
            ServiceState::RunningBase(resources) => {
                if let Some(stderr) = resources.stderr.as_mut() {
                    stderr.failed = true;
                }
            }
            ServiceState::Running(resources) => {
                if let Some(stderr) = resources.stderr.as_mut() {
                    stderr.failed = true;
                }
            }
            ServiceState::Disabled
            | ServiceState::Armed
            | ServiceState::Backoff { .. }
            | ServiceState::Failed => {}
        }
    }

    pub(crate) fn note_reactor_registration_with_token_impl(
        &mut self,
        registration: super::XwaylandReactorRegistration,
        registered: bool,
        reactor_token: Option<u64>,
    ) {
        if registration.purpose == super::XwaylandReactorPurpose::Xwm {
            let action = if registered { "add" } else { "remove" };
            let state = match registration.owner {
                XwaylandReactorOwner::Startup => "Starting",
                XwaylandReactorOwner::Running => "Running",
                XwaylandReactorOwner::Service => self.reactor_state_label(),
            };
            eprintln!(
                "oblivion-one xwayland: event=xwm_reactor_registration state={state} action={action} generation={:?} fd={} token={} writable={}",
                registration.generation,
                registration.fd,
                reactor_token.unwrap_or(0),
                registration.writable,
            );
        }
        match registration.purpose {
            super::XwaylandReactorPurpose::DisplayReady => {
                let Some((generation, process_id, parent_fd, child_source_fd)) =
                    (match &mut self.state {
                        ServiceState::Starting(resources)
                            if registration.generation == Some(resources.generation) =>
                        {
                            resources.displayfd_registered = registered;
                            resources.displayfd_reactor_token =
                                registered.then_some(reactor_token.unwrap_or(0));
                            Some((
                                resources.generation,
                                resources.process.id,
                                resources.displayfd.as_raw_fd(),
                                resources.displayfd_child_source_fd,
                            ))
                        }
                        _ => None,
                    })
                else {
                    return;
                };
                self.log_displayfd_event(
                    "displayfd_registered",
                    Some(if registered {
                        "registered"
                    } else {
                        "unregistered"
                    }),
                    Some(generation),
                    Some(process_id),
                    Some(parent_fd),
                    Some(child_source_fd),
                    reactor_token,
                    None,
                    None,
                );
            }
            super::XwaylandReactorPurpose::Xwm => {
                let token = registered.then_some(reactor_token.unwrap_or(0));
                match &mut self.state {
                    ServiceState::Starting(resources)
                        if registration.generation == Some(resources.generation) =>
                    {
                        resources.xwm_reactor_token = token;
                    }
                    ServiceState::Running(resources)
                        if registration.generation == Some(resources.generation) =>
                    {
                        resources.xwm_reactor_token = token;
                    }
                    _ => {}
                }
            }
            super::XwaylandReactorPurpose::ListenFilesystem
            | super::XwaylandReactorPurpose::ListenAbstract
            | super::XwaylandReactorPurpose::Stderr => {}
        }
    }

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
                owner: XwaylandReactorOwner::Service,
            });
            registrations.push(XwaylandReactorRegistration {
                fd: abstract_socket,
                generation,
                purpose: XwaylandReactorPurpose::ListenAbstract,
                writable: false,
                owner: XwaylandReactorOwner::Service,
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
                owner: XwaylandReactorOwner::Service,
            });
        }
        if let ServiceState::Starting(resources) = &self.state
            && let Some(stderr) = resources.stderr.as_ref().filter(|stderr| stderr.active)
        {
            registrations.push(XwaylandReactorRegistration {
                fd: stderr.fd.as_raw_fd(),
                generation: Some(resources.generation),
                purpose: XwaylandReactorPurpose::Stderr,
                writable: false,
                owner: XwaylandReactorOwner::Service,
            });
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
                owner: XwaylandReactorOwner::Startup,
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
                owner: XwaylandReactorOwner::Service,
            });
        }
        if let ServiceState::Running(resources) = &self.state {
            registrations.push(XwaylandReactorRegistration {
                fd: resources.xwm.raw_fd(),
                generation: Some(resources.generation),
                purpose: XwaylandReactorPurpose::Xwm,
                writable: resources.xwm.wants_writable(),
                owner: XwaylandReactorOwner::Running,
            });
            if let Some(stderr) = resources.stderr.as_ref().filter(|stderr| stderr.active) {
                registrations.push(XwaylandReactorRegistration {
                    fd: stderr.fd.as_raw_fd(),
                    generation: Some(resources.generation),
                    purpose: XwaylandReactorPurpose::Stderr,
                    writable: false,
                    owner: XwaylandReactorOwner::Service,
                });
            }
        }
        registrations.into_iter()
    }

    /// Complete the managed profile's XWM half of the readiness transaction.
    ///
    /// Foundation mode intentionally never calls this path. The WM socket is
    /// moved into `Xwm`; the service retains no duplicate endpoint.
    pub fn initialize_managed_xwm(
        &mut self,
        generation: XwaylandGeneration,
        supervisor: &mut ChildSupervisor,
    ) -> io::Result<()> {
        if self.config.profile != XwaylandProfile::Managed {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "managed XWM requested for foundation profile",
            ));
        }
        let ServiceState::Starting(mut resources) =
            std::mem::replace(&mut self.state, ServiceState::Disabled)
        else {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "managed XWM requested outside XWayland startup",
            ));
        };
        let ready = resources.generation == generation
            && resources.display_ready
            && resources.shell_ready
            && self
                .private_client
                .as_ref()
                .is_some_and(|(client_generation, _)| *client_generation == generation);
        if !ready {
            self.state = ServiceState::Starting(resources);
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "managed XWM readiness barrier is incomplete",
            ));
        }
        if resources.xwm_startup.is_none() {
            let Some(wm_stream) = resources.wm.take() else {
                let error = io::Error::other("managed XWM stream was already transferred");
                self.state = ServiceState::Starting(resources);
                return self.fail_generation(supervisor, error);
            };
            match XwmStartup::new(generation, wm_stream) {
                Ok(startup) => resources.xwm_startup = Some(startup),
                Err(error) => {
                    self.state = ServiceState::Starting(resources);
                    return self.fail_generation(supervisor, io::Error::other(error));
                }
            }
        }
        let progress = resources
            .xwm_startup
            .as_mut()
            .expect("XWM startup driver installed")
            .progress();
        match progress {
            Ok(Some(mut xwm)) => {
                if let Err(error) = xwm.start_root_event_mask_probe().and_then(|_| xwm.flush()) {
                    self.state = ServiceState::Starting(resources);
                    return self.fail_generation(supervisor, io::Error::other(error));
                }
                let startup_fd = resources
                    .xwm_startup
                    .as_ref()
                    .and_then(XwmStartup::raw_fd)
                    .unwrap_or(-1);
                let startup_writable = resources
                    .xwm_startup
                    .as_ref()
                    .is_some_and(XwmStartup::wants_writable);
                let startup_token = resources.xwm_reactor_token;
                let running_fd = xwm.raw_fd();
                let running_writable = xwm.wants_writable();
                eprintln!(
                    "oblivion-one xwayland: event=xwm_reactor_handoff generation={generation:?} startup_fd={startup_fd} running_fd={running_fd} startup_token={} running_token=pending startup_writable={startup_writable} running_writable={running_writable}",
                    startup_token.unwrap_or(0),
                );
                let mut readiness = self.snapshot_for_starting(&resources);
                readiness.xwm_connected = true;
                readiness.xwm_capabilities_validated = true;
                readiness.root_initialized = true;
                readiness.readiness_complete = true;
                self.last_readiness = Some(readiness);
                self.state = ServiceState::Running(Box::new(RunningResources {
                    generation: resources.generation,
                    process: resources.process,
                    private_wayland: resources.private_wayland,
                    xwm,
                    xwm_reactor_token: None,
                    stderr: resources.stderr,
                }));
                self.metrics.state_transitions = self.metrics.state_transitions.saturating_add(1);
                self.log_readiness_progress("xwm_running_wm_s0_verified");
                self.log_state_transition();
                Ok(())
            }
            Ok(None) => {
                if let Some(startup) = resources.xwm_startup.as_ref() {
                    eprintln!(
                        "oblivion-one xwayland: event=xwm_startup_progress generation={generation:?} state={:?} ownership_step={:?}",
                        startup.state(),
                        startup.ownership_step(),
                    );
                }
                self.state = ServiceState::Starting(resources);
                Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "managed XWM startup is waiting for the reactor",
                ))
            }
            Err(error) => {
                let error = io::Error::other(error);
                self.state = ServiceState::Starting(resources);
                self.fail_generation(supervisor, error)
            }
        }
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
