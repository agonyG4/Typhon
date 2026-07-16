use std::{
    collections::VecDeque,
    io,
    num::NonZeroU64,
    os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd},
    os::unix::net::UnixStream,
    path::PathBuf,
    process::Command,
};
use wayland_server::backend::ClientId;

use crate::process::{
    ChildExit, ChildSupervisor, ProcessGroupPolicy, ProcessKind, ProcessOptions, SpawnCommand,
    SpawnedProcess,
};

use super::{
    XwaylandAppEnvironment, XwaylandAssociationEvent, XwaylandGeneration, XwaylandMode,
    config::XwaylandConfig, display::DisplayLease, metrics::XwaylandMetrics, next_nonzero,
};

const DISPLAYFD_MAX_BYTES: usize = 32;
const STARTUP_TIMEOUT_NS: u64 = 3_000_000_000;
const BACKOFF_NS: [u64; 3] = [250_000_000, 1_000_000_000, 4_000_000_000];
const CRASH_WINDOW_NS: u64 = 600_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XwaylandStateKind {
    Disabled,
    Armed,
    Starting,
    RunningBase,
    Backoff,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XwaylandReactorPurpose {
    ListenFilesystem,
    ListenAbstract,
    DisplayReady,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XwaylandReactorRegistration {
    pub fd: RawFd,
    pub generation: Option<XwaylandGeneration>,
    pub purpose: XwaylandReactorPurpose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum XwaylandState {
    Disabled,
    Armed,
    Starting(XwaylandGeneration),
    RunningBase(XwaylandGeneration),
    Backoff,
    Failed,
}

#[derive(Debug)]
struct StartingResources {
    generation: XwaylandGeneration,
    process: SpawnedProcess,
    displayfd: OwnedFd,
    private_wayland: UnixStream,
    wm: UnixStream,
    display_ready: bool,
    shell_ready: bool,
    displayfd_bytes: Vec<u8>,
    started_ns: u64,
    deadline_ns: u64,
}

#[derive(Debug)]
struct RunningResources {
    generation: XwaylandGeneration,
    process: SpawnedProcess,
    private_wayland: UnixStream,
    #[allow(dead_code)]
    wm: UnixStream,
}

#[derive(Debug)]
enum ServiceState {
    Disabled,
    Armed,
    Starting(StartingResources),
    RunningBase(RunningResources),
    Backoff { deadline_ns: u64 },
    Failed,
}

#[derive(Debug)]
pub struct XwaylandService {
    pub(crate) mode: XwaylandMode,
    pub(crate) state: XwaylandState,
    pub(crate) next_generation: NonZeroU64,
    pub(crate) config: XwaylandConfig,
    pub(crate) metrics: XwaylandMetrics,
    lease: Option<DisplayLease>,
    detailed_state: ServiceState,
    crash_times_ns: VecDeque<u64>,
    backoff_level: usize,
    private_client: Option<(XwaylandGeneration, ClientId)>,
}

impl XwaylandService {
    pub fn bootstrap() -> io::Result<Self> {
        Self::bootstrap_with_config(XwaylandConfig::from_environment())
    }

    pub fn bootstrap_with_config(config: XwaylandConfig) -> io::Result<Self> {
        let mode = config.mode;
        let lease = if mode.is_enabled() {
            #[cfg(test)]
            if let Some(root) = config.test_root.as_ref() {
                Some(DisplayLease::allocate_for_tests(
                    root,
                    config.display_min,
                    config.display_max,
                )?)
            } else {
                Some(DisplayLease::allocate(
                    config.display_min,
                    config.display_max,
                )?)
            }
            #[cfg(not(test))]
            Some(DisplayLease::allocate(
                config.display_min,
                config.display_max,
            )?)
        } else {
            None
        };
        let detailed_state = if mode.is_enabled() {
            ServiceState::Armed
        } else {
            ServiceState::Disabled
        };
        Ok(Self {
            mode,
            state: if mode.is_enabled() {
                XwaylandState::Armed
            } else {
                XwaylandState::Disabled
            },
            next_generation: NonZeroU64::new(1).expect("one is nonzero"),
            config,
            metrics: XwaylandMetrics::default(),
            lease,
            detailed_state,
            crash_times_ns: VecDeque::new(),
            backoff_level: 0,
            private_client: None,
        })
    }

    pub fn bootstrap_with_supervisor(
        config: XwaylandConfig,
        supervisor: &mut ChildSupervisor,
    ) -> io::Result<Self> {
        let eager = config.mode.is_eager();
        let mut service = Self::bootstrap_with_config(config)?;
        if eager {
            service.handle_listener_readiness(supervisor)?;
        }
        Ok(service)
    }

    pub fn state_kind(&self) -> XwaylandStateKind {
        match self.detailed_state {
            ServiceState::Disabled => XwaylandStateKind::Disabled,
            ServiceState::Armed => XwaylandStateKind::Armed,
            ServiceState::Starting(_) => XwaylandStateKind::Starting,
            ServiceState::RunningBase(_) => XwaylandStateKind::RunningBase,
            ServiceState::Backoff { .. } => XwaylandStateKind::Backoff,
            ServiceState::Failed => XwaylandStateKind::Failed,
        }
    }

    pub fn is_eager(&self) -> bool {
        self.mode.is_eager()
    }

    pub fn app_environment(&self) -> Option<XwaylandAppEnvironment> {
        self.lease.as_ref().map(|lease| XwaylandAppEnvironment {
            display: lease.display().to_string(),
            xauthority: lease.xauthority_path().to_owned(),
        })
    }

    pub fn reactor_registrations(&self) -> impl Iterator<Item = XwaylandReactorRegistration> {
        let mut registrations = Vec::new();
        let generation = self.generation();
        if !matches!(
            self.detailed_state,
            ServiceState::Disabled | ServiceState::Failed
        ) && let Some(lease) = self.lease.as_ref()
        {
            let (filesystem, abstract_socket) = lease.listener_fds();
            registrations.push(XwaylandReactorRegistration {
                fd: filesystem,
                generation,
                purpose: XwaylandReactorPurpose::ListenFilesystem,
            });
            registrations.push(XwaylandReactorRegistration {
                fd: abstract_socket,
                generation,
                purpose: XwaylandReactorPurpose::ListenAbstract,
            });
        }
        if let ServiceState::Starting(resources) = &self.detailed_state {
            registrations.push(XwaylandReactorRegistration {
                fd: resources.displayfd.as_raw_fd(),
                generation: Some(resources.generation),
                purpose: XwaylandReactorPurpose::DisplayReady,
            });
        }
        registrations.into_iter()
    }

    pub fn next_deadline_ns(&self) -> Option<u64> {
        match self.detailed_state {
            ServiceState::Starting(ref resources) => Some(resources.deadline_ns),
            ServiceState::Backoff { deadline_ns, .. } => Some(deadline_ns),
            ServiceState::Disabled
            | ServiceState::Armed
            | ServiceState::RunningBase(_)
            | ServiceState::Failed => None,
        }
    }

    pub fn generation(&self) -> Option<XwaylandGeneration> {
        match self.detailed_state {
            ServiceState::Starting(ref resources) => Some(resources.generation),
            ServiceState::RunningBase(ref resources) => Some(resources.generation),
            ServiceState::Disabled
            | ServiceState::Armed
            | ServiceState::Backoff { .. }
            | ServiceState::Failed => None,
        }
    }

    pub fn display_number(&self) -> Option<u32> {
        self.lease.as_ref().map(DisplayLease::display_number)
    }

    pub fn private_wayland_client(&mut self, generation: XwaylandGeneration) -> Option<UnixStream> {
        match &mut self.detailed_state {
            ServiceState::Starting(resources) if resources.generation == generation => {
                resources.private_wayland.try_clone().ok()
            }
            ServiceState::RunningBase(resources) if resources.generation == generation => {
                resources.private_wayland.try_clone().ok()
            }
            _ => None,
        }
    }

    pub fn handle_listener_readiness(
        &mut self,
        supervisor: &mut ChildSupervisor,
    ) -> io::Result<bool> {
        if !matches!(self.detailed_state, ServiceState::Armed) {
            return Ok(false);
        }
        if self.mode == XwaylandMode::BaseLazy {
            self.metrics.lazy_triggers = self.metrics.lazy_triggers.saturating_add(1);
        }
        match self.start_generation(supervisor) {
            Ok(()) => Ok(true),
            Err(error) => {
                self.enter_backoff(now_ns()?);
                Err(error)
            }
        }
    }

    pub fn handle_reactor_event(
        &mut self,
        purpose: XwaylandReactorPurpose,
        generation: Option<XwaylandGeneration>,
        flags: u32,
        supervisor: &mut ChildSupervisor,
    ) -> io::Result<()> {
        if flags & (libc::EPOLLIN as u32 | libc::EPOLLHUP as u32 | libc::EPOLLRDHUP as u32) == 0 {
            return Ok(());
        }
        match purpose {
            XwaylandReactorPurpose::ListenFilesystem | XwaylandReactorPurpose::ListenAbstract => {
                if flags & libc::EPOLLIN as u32 != 0 {
                    let _ = self.handle_listener_readiness(supervisor)?;
                }
            }
            XwaylandReactorPurpose::DisplayReady => {
                if let Some(generation) = generation {
                    self.handle_displayfd_ready(generation, supervisor)?;
                }
            }
        }
        Ok(())
    }

    fn start_generation(&mut self, supervisor: &mut ChildSupervisor) -> io::Result<()> {
        let generation = self.allocate_generation();
        let started_ns = now_ns()?;
        let deadline_ns = started_ns.saturating_add(STARTUP_TIMEOUT_NS);
        let lease = self
            .lease
            .as_ref()
            .ok_or_else(|| io::Error::other("XWayland start requested without a display lease"))?;
        let (private_wayland, private_wayland_child) = UnixStream::pair()?;
        let (wm, wm_child) = UnixStream::pair()?;
        let (displayfd, displayfd_child) = pipe_pair()?;
        let mut launch = SpawnCommand::new(build_command(&self.config.binary, lease));
        launch.map_fd(
            owned_fd_from_stream(private_wayland_child),
            ChildFdTarget::WaylandSocket.raw_fd(),
        )?;
        launch.map_fd(owned_fd_from_stream(wm_child), ChildFdTarget::Wm.raw_fd())?;
        launch.map_fd(displayfd_child, ChildFdTarget::DisplayFd.raw_fd())?;
        let (filesystem_listener, abstract_listener) = lease.listener_fds();
        launch.map_fd(
            duplicate_fd(filesystem_listener)?,
            ChildFdTarget::FilesystemListen.raw_fd(),
        )?;
        launch.map_fd(
            duplicate_fd(abstract_listener)?,
            ChildFdTarget::AbstractListen.raw_fd(),
        )?;
        let process = launch.spawn(
            supervisor,
            ProcessOptions::new(ProcessKind::Xwayland)
                .session_owned(true)
                .with_process_group_policy(ProcessGroupPolicy::Dedicated),
        )?;
        self.detailed_state = ServiceState::Starting(StartingResources {
            generation,
            process,
            displayfd,
            private_wayland,
            wm,
            display_ready: false,
            shell_ready: false,
            displayfd_bytes: Vec::new(),
            started_ns,
            deadline_ns,
        });
        self.state = XwaylandState::Starting(generation);
        self.metrics.state_transitions = self.metrics.state_transitions.saturating_add(1);
        Ok(())
    }

    pub fn handle_displayfd_ready(
        &mut self,
        generation: XwaylandGeneration,
        supervisor: &mut ChildSupervisor,
    ) -> io::Result<()> {
        let mut bytes = [0u8; 64];
        loop {
            let read_result = match &self.detailed_state {
                ServiceState::Starting(resources) if resources.generation == generation => unsafe {
                    libc::read(
                        resources.displayfd.as_raw_fd(),
                        bytes.as_mut_ptr().cast(),
                        bytes.len(),
                    )
                },
                ServiceState::Starting(_) => {
                    self.metrics.stale_events = self.metrics.stale_events.saturating_add(1);
                    return Ok(());
                }
                _ => return Ok(()),
            };
            if read_result < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::WouldBlock {
                    return Ok(());
                }
                return self.fail_generation(supervisor, error);
            }
            if read_result == 0 {
                return self.fail_generation(
                    supervisor,
                    io::Error::new(io::ErrorKind::UnexpectedEof, "XWayland displayfd closed"),
                );
            }
            self.handle_displayfd_bytes(generation, &bytes[..read_result as usize], supervisor)?;
            if !matches!(self.detailed_state, ServiceState::Starting(_)) {
                return Ok(());
            }
            if bytes[..read_result as usize].contains(&b'\n') {
                return Ok(());
            }
        }
    }

    pub fn handle_displayfd_bytes(
        &mut self,
        generation: XwaylandGeneration,
        bytes: &[u8],
        supervisor: &mut ChildSupervisor,
    ) -> io::Result<()> {
        let reserved_display = self.display_number();
        let ServiceState::Starting(resources) = &mut self.detailed_state else {
            return Ok(());
        };
        if resources.generation != generation {
            self.metrics.stale_events = self.metrics.stale_events.saturating_add(1);
            self.metrics.unauthorized_bind_attempts =
                self.metrics.unauthorized_bind_attempts.saturating_add(1);
            return Ok(());
        }
        if resources.displayfd_bytes.len().saturating_add(bytes.len()) > DISPLAYFD_MAX_BYTES {
            return self.fail_generation(
                supervisor,
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "XWayland displayfd payload is oversized",
                ),
            );
        }
        resources.displayfd_bytes.extend_from_slice(bytes);
        let Some(newline) = resources
            .displayfd_bytes
            .iter()
            .position(|byte| *byte == b'\n')
        else {
            return Ok(());
        };
        let payload = &resources.displayfd_bytes[..newline];
        if payload.is_empty() || !payload.iter().all(u8::is_ascii_digit) {
            return self.fail_generation(
                supervisor,
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "XWayland displayfd payload is malformed",
                ),
            );
        }
        let value = std::str::from_utf8(payload)
            .ok()
            .and_then(|value| value.parse::<u32>().ok());
        if value != reserved_display {
            return self.fail_generation(
                supervisor,
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "XWayland displayfd does not match lease",
                ),
            );
        }
        resources.display_ready = true;
        self.maybe_mark_running();
        Ok(())
    }

    pub fn handle_shell_bind(&mut self, generation: XwaylandGeneration) -> io::Result<()> {
        self.mark_shell_ready(generation)
    }

    pub fn authorize_private_client(
        &mut self,
        generation: XwaylandGeneration,
        client_id: ClientId,
    ) {
        self.private_client = Some((generation, client_id));
    }

    pub fn handle_shell_bind_for_client(
        &mut self,
        generation: XwaylandGeneration,
        client_id: &ClientId,
    ) -> io::Result<()> {
        if self
            .private_client
            .as_ref()
            .is_none_or(|(expected_generation, expected_client)| {
                *expected_generation != generation || expected_client != client_id
            })
        {
            self.metrics.stale_events = self.metrics.stale_events.saturating_add(1);
            return Ok(());
        }
        self.mark_shell_ready(generation)
    }

    fn mark_shell_ready(&mut self, generation: XwaylandGeneration) -> io::Result<()> {
        let ServiceState::Starting(resources) = &mut self.detailed_state else {
            return Ok(());
        };
        if resources.generation != generation {
            self.metrics.stale_events = self.metrics.stale_events.saturating_add(1);
            return Ok(());
        }
        resources.shell_ready = true;
        self.maybe_mark_running();
        Ok(())
    }

    fn maybe_mark_running(&mut self) {
        let ready = matches!(
            &self.detailed_state,
            ServiceState::Starting(resources) if resources.display_ready && resources.shell_ready
        );
        if !ready {
            return;
        }
        let ServiceState::Starting(resources) =
            std::mem::replace(&mut self.detailed_state, ServiceState::Disabled)
        else {
            unreachable!("readiness state changed while promoting XWayland")
        };
        self.detailed_state = ServiceState::RunningBase(RunningResources {
            generation: resources.generation,
            process: resources.process,
            private_wayland: resources.private_wayland,
            wm: resources.wm,
        });
        self.state = XwaylandState::RunningBase(resources.generation);
        self.metrics.startup_duration_ns = now_ns()
            .ok()
            .map(|now| now.saturating_sub(resources.started_ns));
        self.metrics.state_transitions = self.metrics.state_transitions.saturating_add(1);
    }

    pub fn handle_process_exit(&mut self, exit: &ChildExit) -> io::Result<bool> {
        let process = match &self.detailed_state {
            ServiceState::Starting(resources) => resources.process,
            ServiceState::RunningBase(resources) => resources.process,
            _ => return Ok(false),
        };
        if process.id != exit.id {
            self.metrics.stale_events = self.metrics.stale_events.saturating_add(1);
            return Ok(false);
        }
        if exit.status.success() {
            self.rearm();
            return Ok(true);
        }
        let now = now_ns()?;
        while self
            .crash_times_ns
            .front()
            .is_some_and(|started| now.saturating_sub(*started) > CRASH_WINDOW_NS)
        {
            self.crash_times_ns.pop_front();
        }
        self.crash_times_ns.push_back(now);
        self.metrics.crashes = self.metrics.crashes.saturating_add(1);
        if self.crash_times_ns.len() >= 3 {
            self.detailed_state = ServiceState::Failed;
            self.state = XwaylandState::Failed;
            return Ok(true);
        }
        self.enter_backoff(now);
        Ok(true)
    }

    pub fn record_association_events(&mut self, events: &[XwaylandAssociationEvent]) {
        for event in events {
            match event {
                XwaylandAssociationEvent::Committed { .. } => {
                    self.metrics.association_commits =
                        self.metrics.association_commits.saturating_add(1);
                }
                XwaylandAssociationEvent::Removed { .. } => {
                    self.metrics.association_removals =
                        self.metrics.association_removals.saturating_add(1);
                }
            }
        }
    }

    pub fn handle_deadline(
        &mut self,
        now_ns: u64,
        supervisor: &mut ChildSupervisor,
    ) -> io::Result<()> {
        match self.detailed_state {
            ServiceState::Starting(ref resources) if now_ns >= resources.deadline_ns => {
                self.metrics.readiness_failures = self.metrics.readiness_failures.saturating_add(1);
                let process_id = resources.process.id;
                self.kill_process_now(supervisor, process_id)?;
                self.enter_backoff(now_ns);
            }
            ServiceState::Backoff { deadline_ns, .. } if now_ns >= deadline_ns => {
                self.rearm();
            }
            _ => {}
        }
        Ok(())
    }

    pub fn begin_shutdown(&mut self, supervisor: &mut ChildSupervisor) -> io::Result<()> {
        self.stop_current(supervisor)?;
        self.private_client = None;
        self.lease.take();
        self.detailed_state = ServiceState::Disabled;
        self.state = XwaylandState::Disabled;
        Ok(())
    }

    pub fn emergency_cleanup(&mut self, supervisor: &mut ChildSupervisor) -> io::Result<()> {
        self.begin_shutdown(supervisor)
    }

    fn stop_current(&mut self, supervisor: &mut ChildSupervisor) -> io::Result<()> {
        let process_id = match &self.detailed_state {
            ServiceState::Starting(resources) => Some(resources.process.id),
            ServiceState::RunningBase(resources) => Some(resources.process.id),
            ServiceState::Disabled
            | ServiceState::Armed
            | ServiceState::Backoff { .. }
            | ServiceState::Failed => None,
        };
        if let Some(process_id) = process_id {
            self.kill_process_now(supervisor, process_id)?;
        }
        Ok(())
    }

    fn kill_process_now(
        &mut self,
        supervisor: &mut ChildSupervisor,
        process_id: crate::process::ManagedProcessId,
    ) -> io::Result<()> {
        self.metrics.cleanup_attempts = self.metrics.cleanup_attempts.saturating_add(1);
        match supervisor.kill_managed_now(process_id) {
            Ok(_) => Ok(()),
            Err(error) => {
                self.metrics.cleanup_failures = self.metrics.cleanup_failures.saturating_add(1);
                Err(error)
            }
        }
    }

    fn rearm(&mut self) {
        self.private_client = None;
        if self.mode.is_enabled() && self.lease.is_some() {
            self.detailed_state = ServiceState::Armed;
            self.state = XwaylandState::Armed;
        } else {
            self.detailed_state = ServiceState::Disabled;
            self.state = XwaylandState::Disabled;
        }
        self.backoff_level = 0;
        self.metrics.state_transitions = self.metrics.state_transitions.saturating_add(1);
    }

    fn enter_backoff(&mut self, now_ns: u64) {
        self.private_client = None;
        let level = self.backoff_level.min(BACKOFF_NS.len().saturating_sub(1));
        let deadline_ns = now_ns.saturating_add(BACKOFF_NS[level]);
        self.backoff_level = self.backoff_level.saturating_add(1);
        self.metrics.backoff_level = self.backoff_level;
        self.detailed_state = ServiceState::Backoff { deadline_ns };
        self.state = XwaylandState::Backoff;
        self.metrics.state_transitions = self.metrics.state_transitions.saturating_add(1);
    }

    fn fail_generation(
        &mut self,
        supervisor: &mut ChildSupervisor,
        error: io::Error,
    ) -> io::Result<()> {
        let process_id = match &self.detailed_state {
            ServiceState::Starting(resources) => Some(resources.process.id),
            _ => None,
        };
        if let Some(process_id) = process_id {
            self.kill_process_now(supervisor, process_id)?;
        }
        self.private_client = None;
        self.metrics.readiness_failures = self.metrics.readiness_failures.saturating_add(1);
        self.enter_backoff(now_ns()?);
        Err(error)
    }

    pub(crate) fn allocate_generation(&mut self) -> XwaylandGeneration {
        self.metrics.generations_started = self.metrics.generations_started.saturating_add(1);
        next_nonzero(&mut self.next_generation)
    }
}

#[derive(Debug, Clone, Copy)]
enum ChildFdTarget {
    WaylandSocket,
    Wm,
    DisplayFd,
    FilesystemListen,
    AbstractListen,
}

impl ChildFdTarget {
    const fn raw_fd(self) -> RawFd {
        match self {
            Self::WaylandSocket => 3,
            Self::Wm => 4,
            Self::DisplayFd => 5,
            Self::FilesystemListen => 6,
            Self::AbstractListen => 7,
        }
    }
}

fn build_command(binary: &PathBuf, lease: &DisplayLease) -> Command {
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
    command
}

fn pipe_pair() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [-1; 2];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((unsafe { OwnedFd::from_raw_fd(fds[0]) }, unsafe {
        OwnedFd::from_raw_fd(fds[1])
    }))
}

fn duplicate_fd(fd: RawFd) -> io::Result<OwnedFd> {
    let duplicate = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 100) };
    if duplicate < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { OwnedFd::from_raw_fd(duplicate) })
    }
}

fn owned_fd_from_stream(stream: UnixStream) -> OwnedFd {
    unsafe { OwnedFd::from_raw_fd(stream.into_raw_fd()) }
}

fn now_ns() -> io::Result<u64> {
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
