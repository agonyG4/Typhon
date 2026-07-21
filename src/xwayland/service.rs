use std::{
    collections::VecDeque,
    io,
    num::NonZeroU64,
    os::fd::{AsRawFd, OwnedFd, RawFd},
    os::unix::net::UnixStream,
};
use wayland_server::backend::ClientId;

use crate::process::{
    ChildExit, ChildSupervisor, ManagedProcessId, ProcessGroupPolicy, ProcessKind, ProcessOptions,
    SpawnCommand, SpawnedProcess,
};

use super::trace::{self, TraceFields};
use super::{
    XwaylandAppEnvironment, XwaylandAssociationEvent, XwaylandGeneration, XwaylandMode,
    config::XwaylandConfig,
    diagnostics::{StderrRing, XwaylandFailure, XwaylandFailureStage},
    display::DisplayLease,
    displayfd,
    launch::{ChildFdTarget, build_command},
    metrics::XwaylandMetrics,
    readiness::XwaylandReadinessSnapshot,
    xwm::{Xwm, startup::XwmStartup},
};

#[path = "displayfd_service.rs"]
mod displayfd_service;
#[path = "service_support.rs"]
mod service_support;

use service_support::{StderrPipe, classify_exit, duplicate_fd, now_ns, owned_fd_from_stream};

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
    Running,
    Backoff,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XwaylandReactorPurpose {
    ListenFilesystem,
    ListenAbstract,
    DisplayReady,
    Xwm,
    Stderr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum XwaylandReactorOwner {
    Service,
    Startup,
    Running,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XwaylandReactorRegistration {
    pub fd: RawFd,
    pub generation: Option<XwaylandGeneration>,
    pub purpose: XwaylandReactorPurpose,
    pub writable: bool,
    pub(crate) owner: XwaylandReactorOwner,
}

#[derive(Debug)]
struct StartingResources {
    generation: XwaylandGeneration,
    process: SpawnedProcess,
    displayfd: OwnedFd,
    displayfd_child_source_fd: RawFd,
    displayfd_reactor_token: Option<u64>,
    private_wayland: Option<UnixStream>,
    wm: Option<UnixStream>,
    xwm_startup: Option<XwmStartup>,
    xwm_reactor_token: Option<u64>,
    display_ready: bool,
    displayfd_registered: bool,
    displayfd_readable: bool,
    private_wayland_endpoint_transferred: bool,
    private_client_attached: bool,
    private_client_authorized: bool,
    shell_ready: bool,
    xwm_connected: bool,
    xwm_capabilities_validated: bool,
    root_initialized: bool,
    displayfd_bytes: Vec<u8>,
    stderr: Option<StderrPipe>,
    started_ns: u64,
    deadline_ns: u64,
}

#[derive(Debug)]
struct RunningBaseResources {
    generation: XwaylandGeneration,
    process: SpawnedProcess,
    private_wayland: Option<UnixStream>,
    _wm: Option<UnixStream>,
    stderr: Option<StderrPipe>,
}

#[derive(Debug)]
struct RunningResources {
    generation: XwaylandGeneration,
    process: SpawnedProcess,
    private_wayland: Option<UnixStream>,
    xwm: Xwm,
    xwm_reactor_token: Option<u64>,
    stderr: Option<StderrPipe>,
}

#[derive(Debug, Clone, Copy)]
struct PendingTermination {
    process_id: ManagedProcessId,
    deadline_ns: u64,
    escalated: bool,
}

#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
enum ServiceState {
    Disabled,
    Armed,
    Starting(StartingResources),
    RunningBase(RunningBaseResources),
    Running(Box<RunningResources>),
    Backoff { deadline_ns: u64 },
    Failed,
}

#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
enum RetiredResources {
    Starting(StartingResources),
    RunningBase(RunningBaseResources),
    Running(Box<RunningResources>),
}

#[derive(Debug)]
pub struct XwaylandService {
    pub(crate) mode: XwaylandMode,
    pub(crate) next_generation: NonZeroU64,
    pub(crate) config: XwaylandConfig,
    pub(crate) metrics: XwaylandMetrics,
    lease: Option<DisplayLease>,
    state: ServiceState,
    crash_times_ns: VecDeque<u64>,
    backoff_level: usize,
    private_client: Option<(XwaylandGeneration, ClientId)>,
    retired_resources: Vec<RetiredResources>,
    retired_lease: Option<DisplayLease>,
    last_readiness: Option<XwaylandReadinessSnapshot>,
    pending_termination: Option<PendingTermination>,
    latest_failed_stderr: StderrRing,
    latest_failure: Option<XwaylandFailure>,
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
        let state = if mode.is_enabled() {
            ServiceState::Armed
        } else {
            ServiceState::Disabled
        };
        Ok(Self {
            mode,
            next_generation: NonZeroU64::new(1).expect("one is nonzero"),
            config,
            metrics: XwaylandMetrics::default(),
            lease,
            state,
            crash_times_ns: VecDeque::new(),
            backoff_level: 0,
            private_client: None,
            retired_resources: Vec::new(),
            retired_lease: None,
            last_readiness: None,
            pending_termination: None,
            latest_failed_stderr: StderrRing::default(),
            latest_failure: None,
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
        match self.state {
            ServiceState::Disabled => XwaylandStateKind::Disabled,
            ServiceState::Armed => XwaylandStateKind::Armed,
            ServiceState::Starting(_) => XwaylandStateKind::Starting,
            ServiceState::RunningBase(_) => XwaylandStateKind::RunningBase,
            ServiceState::Running(_) => XwaylandStateKind::Running,
            ServiceState::Backoff { .. } => XwaylandStateKind::Backoff,
            ServiceState::Failed => XwaylandStateKind::Failed,
        }
    }

    pub fn readiness_snapshot(&self) -> Option<XwaylandReadinessSnapshot> {
        match &self.state {
            ServiceState::Starting(resources) => Some(self.snapshot_for_starting(resources)),
            ServiceState::RunningBase(_) | ServiceState::Running(_) => self.last_readiness,
            ServiceState::Disabled
            | ServiceState::Armed
            | ServiceState::Backoff { .. }
            | ServiceState::Failed => self.last_readiness,
        }
    }

    pub fn is_eager(&self) -> bool {
        self.mode.is_eager()
    }

    pub fn app_environment(&self) -> Option<XwaylandAppEnvironment> {
        match self.state {
            ServiceState::Disabled | ServiceState::Failed => None,
            ServiceState::Armed
            | ServiceState::Starting(_)
            | ServiceState::RunningBase(_)
            | ServiceState::Running(_)
            | ServiceState::Backoff { .. } => {
                self.lease.as_ref().map(|lease| XwaylandAppEnvironment {
                    display: lease.display().to_string(),
                    xauthority: lease.xauthority_path().to_owned(),
                })
            }
        }
    }

    pub fn normal_app_environment(&self) -> Option<XwaylandAppEnvironment> {
        if !self.mode.is_managed()
            || matches!(self.state, ServiceState::Disabled | ServiceState::Failed)
        {
            return None;
        }
        self.app_environment()
    }

    pub fn is_managed(&self) -> bool {
        self.mode.is_managed()
    }

    pub fn note_reactor_registration(
        &mut self,
        registration: XwaylandReactorRegistration,
        registered: bool,
    ) {
        self.note_reactor_registration_with_token(registration, registered, None);
    }

    pub fn note_reactor_registration_with_token(
        &mut self,
        registration: XwaylandReactorRegistration,
        registered: bool,
        reactor_token: Option<u64>,
    ) {
        self.note_reactor_registration_with_token_impl(registration, registered, reactor_token);
    }

    fn reactor_state_label(&self) -> &'static str {
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

    fn xwm_reactor_token(&self, generation: XwaylandGeneration) -> Option<u64> {
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

    fn drain_managed_xwm(&mut self, supervisor: &mut ChildSupervisor) -> bool {
        let drain = match &mut self.state {
            ServiceState::Running(resources) => resources.xwm.drain_events(256),
            _ => return false,
        };
        match drain {
            Ok(drain) => {
                let (property_metrics, pending_events, generation) = match &self.state {
                    ServiceState::Running(resources) => (
                        resources.xwm.property_metrics(),
                        resources.xwm.pending_event_count(),
                        resources.generation,
                    ),
                    _ => return false,
                };
                eprintln!(
                    "oblivion-one xwayland: event=xwm_reactor_drain generation={generation:?} processed={} budget_exhausted={} pending_events={pending_events}",
                    drain.processed, drain.budget_exhausted,
                );
                self.metrics.property_refresh_requested = property_metrics.requested;
                self.metrics.property_refresh_completed = property_metrics.completed;
                self.metrics.property_refresh_coalesced = property_metrics.coalesced;
                self.metrics.property_refresh_rejected = property_metrics.rejected;
                self.metrics.property_refresh_stale = property_metrics.stale;
                self.metrics.xwm_events_received = self
                    .metrics
                    .xwm_events_received
                    .saturating_add(drain.processed as u64);
                if drain.budget_exhausted {
                    self.metrics.xwm_drain_budget_exhaustions =
                        self.metrics.xwm_drain_budget_exhaustions.saturating_add(1);
                }
                drain.budget_exhausted
            }
            Err(error) => {
                self.fail_managed_xwm(
                    supervisor,
                    XwaylandFailureStage::Reactor,
                    io::Error::other(error),
                );
                false
            }
        }
    }

    fn fail_managed_xwm(
        &mut self,
        supervisor: &mut ChildSupervisor,
        stage: XwaylandFailureStage,
        error: io::Error,
    ) {
        let Some(process_id) = (match &self.state {
            ServiceState::Running(resources) => Some(resources.process.id),
            _ => return,
        }) else {
            return;
        };
        let generation = self.generation();
        self.metrics.xwm_connection_failures =
            self.metrics.xwm_connection_failures.saturating_add(1);
        self.record_failure(generation, stage, &error);
        eprintln!(
            "oblivion-one xwayland: event=xwm_failure generation={generation:?} stage={stage:?} reason={error}"
        );
        self.private_client = None;
        self.mark_stderr_failure();
        if let Err(cleanup_error) = self.request_process_termination(supervisor, process_id) {
            eprintln!(
                "oblivion-one xwayland: event=xwm_cleanup_failure generation={generation:?} reason={cleanup_error}"
            );
        }
        self.enter_failure_backoff(now_ns().unwrap_or_default());
    }

    pub fn display_number(&self) -> Option<u32> {
        self.lease.as_ref().map(DisplayLease::display_number)
    }

    pub fn recent_failure_diagnostics(&self) -> Vec<String> {
        self.latest_failed_stderr
            .lines()
            .map(|line| {
                if line.truncated {
                    format!("{} [truncated]", line.text)
                } else {
                    line.text.clone()
                }
            })
            .collect()
    }

    pub fn take_private_wayland_client(
        &mut self,
        generation: XwaylandGeneration,
    ) -> Option<UnixStream> {
        match &mut self.state {
            ServiceState::Starting(resources) if resources.generation == generation => {
                let stream = resources.private_wayland.take();
                if stream.is_some() {
                    resources.private_wayland_endpoint_transferred = true;
                }
                stream
            }
            ServiceState::RunningBase(resources) if resources.generation == generation => {
                resources.private_wayland.take()
            }
            ServiceState::Running(resources) if resources.generation == generation => {
                resources.private_wayland.take()
            }
            _ => None,
        }
    }

    pub fn handle_private_client_disconnected(
        &mut self,
        generation: XwaylandGeneration,
        supervisor: &mut ChildSupervisor,
    ) -> io::Result<()> {
        if self.generation() != Some(generation) {
            self.metrics.stale_events = self.metrics.stale_events.saturating_add(1);
            return Ok(());
        }
        self.private_client = None;
        let process_id = match &self.state {
            ServiceState::Starting(resources) => Some(resources.process.id),
            ServiceState::RunningBase(resources) => Some(resources.process.id),
            ServiceState::Running(resources) => Some(resources.process.id),
            _ => None,
        };
        if let Some(process_id) = process_id {
            self.request_process_termination(supervisor, process_id)?;
        }
        self.mark_stderr_failure();
        self.enter_failure_backoff(now_ns()?);
        Ok(())
    }

    pub fn handle_listener_readiness(
        &mut self,
        supervisor: &mut ChildSupervisor,
    ) -> io::Result<bool> {
        if !matches!(self.state, ServiceState::Armed) {
            return Ok(false);
        }
        if matches!(
            self.mode,
            XwaylandMode::BaseLazy | XwaylandMode::ManagedLazy
        ) {
            self.metrics.lazy_triggers = self.metrics.lazy_triggers.saturating_add(1);
        }
        match self.start_generation(supervisor) {
            Ok(()) => Ok(true),
            Err(error) => {
                self.enter_failure_backoff(now_ns()?);
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
    ) -> io::Result<bool> {
        self.handle_reactor_event_with_token(purpose, generation, flags, 0, supervisor)
    }

    pub fn handle_reactor_event_with_token(
        &mut self,
        purpose: XwaylandReactorPurpose,
        generation: Option<XwaylandGeneration>,
        flags: u32,
        reactor_token: u64,
        supervisor: &mut ChildSupervisor,
    ) -> io::Result<bool> {
        let event_flags = libc::EPOLLIN as u32
            | libc::EPOLLOUT as u32
            | libc::EPOLLERR as u32
            | libc::EPOLLHUP as u32
            | libc::EPOLLRDHUP as u32;
        let error_flags = libc::EPOLLERR as u32 | libc::EPOLLHUP as u32 | libc::EPOLLRDHUP as u32;
        if flags & event_flags == 0 {
            return Ok(false);
        }
        match purpose {
            XwaylandReactorPurpose::ListenFilesystem | XwaylandReactorPurpose::ListenAbstract => {
                if flags & libc::EPOLLIN as u32 != 0 {
                    return self.handle_listener_readiness(supervisor);
                }
            }
            XwaylandReactorPurpose::DisplayReady => {
                if let Some(generation) = generation {
                    if reactor_token != 0
                        && self.displayfd_reactor_token(generation) != Some(reactor_token)
                    {
                        self.metrics.stale_events = self.metrics.stale_events.saturating_add(1);
                        return Ok(false);
                    }
                    self.log_displayfd_event(
                        "displayfd_epoll",
                        None,
                        Some(generation),
                        self.process_id_for_generation(generation),
                        self.displayfd_parent_fd(generation),
                        self.displayfd_child_source_fd(generation),
                        Some(reactor_token),
                        Some(flags),
                        None,
                    );
                    self.handle_displayfd_ready_with_flags(generation, flags, supervisor)?;
                }
            }
            XwaylandReactorPurpose::Xwm => {
                let state = self.reactor_state_label();
                let fd = generation
                    .and_then(|generation| self.managed_xwm_fd(generation))
                    .unwrap_or(-1);
                eprintln!(
                    "oblivion-one xwayland: event=xwm_reactor_ready state={state} generation={generation:?} fd={fd} token={reactor_token} flags=0x{flags:x}",
                );
                if generation != self.generation() {
                    self.metrics.stale_events = self.metrics.stale_events.saturating_add(1);
                    eprintln!(
                        "oblivion-one xwayland: event=xwm_reactor_rejected reason=stale_generation"
                    );
                    return Ok(false);
                }
                if reactor_token != 0
                    && self.xwm_reactor_token(generation.expect("XWM event generation"))
                        != Some(reactor_token)
                {
                    self.metrics.stale_events = self.metrics.stale_events.saturating_add(1);
                    eprintln!(
                        "oblivion-one xwayland: event=xwm_reactor_rejected reason=stale_token"
                    );
                    return Ok(false);
                }
                self.metrics.xwm_reactor_events = self.metrics.xwm_reactor_events.saturating_add(1);
                let readable = flags & libc::EPOLLIN as u32 != 0;
                let writable = flags & libc::EPOLLOUT as u32 != 0;
                if matches!(self.state, ServiceState::Starting(_)) && (readable || writable) {
                    match self.initialize_managed_xwm(
                        generation.expect("XWM registration has a generation"),
                        supervisor,
                    ) {
                        Ok(()) => {}
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                        Err(error) => {
                            eprintln!(
                                "oblivion-one xwayland: event=xwm_startup_reactor_failure generation={generation:?} reason={error} detail={:?}",
                                match &self.state {
                                    ServiceState::Starting(resources) => resources
                                        .xwm_startup
                                        .as_ref()
                                        .and_then(XwmStartup::last_error),
                                    _ => None,
                                }
                            );
                        }
                    }
                }
                let startup_flush = if writable {
                    match &self.state {
                        ServiceState::Starting(resources) => {
                            resources.xwm_startup.as_ref().map(XwmStartup::flush_output)
                        }
                        _ => None,
                    }
                } else {
                    None
                };
                if let Some(Err(error)) = startup_flush {
                    let error = io::Error::other(error);
                    let _ = self.fail_generation_with_stage(
                        supervisor,
                        XwaylandFailureStage::StartupFlush,
                        error,
                    );
                    return Ok(false);
                }
                // A readable edge paired with HUP/RDHUP is drained first. This
                // preserves the final replies/events and only then retires the
                // generation if the connection is actually closed.
                let mut continuation = false;
                if readable && matches!(self.state, ServiceState::Running(_)) {
                    continuation = self.drain_managed_xwm(supervisor);
                }
                if writable
                    && let ServiceState::Running(resources) = &self.state
                    && let Err(error) = resources.xwm.flush_output()
                {
                    self.fail_managed_xwm(
                        supervisor,
                        XwaylandFailureStage::CommandFlush,
                        io::Error::other(error),
                    );
                    return Ok(false);
                }
                if flags & error_flags != 0 {
                    let error = io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "XWM connection reported a reactor error",
                    );
                    if matches!(self.state, ServiceState::Starting(_)) {
                        let _ = self.fail_generation_with_stage(
                            supervisor,
                            XwaylandFailureStage::Reactor,
                            error,
                        );
                    } else {
                        self.fail_managed_xwm(supervisor, XwaylandFailureStage::Reactor, error);
                    }
                    return Ok(false);
                }
                return Ok(continuation);
            }
            XwaylandReactorPurpose::Stderr => {
                let Some(generation) = generation else {
                    self.metrics.stale_events = self.metrics.stale_events.saturating_add(1);
                    return Ok(false);
                };
                if Some(generation) != self.generation() {
                    self.metrics.stale_events = self.metrics.stale_events.saturating_add(1);
                    return Ok(false);
                }
                self.handle_stderr_ready(generation)?;
            }
        }
        Ok(false)
    }

    fn start_generation(&mut self, supervisor: &mut ChildSupervisor) -> io::Result<()> {
        let generation = self.allocate_generation()?;
        let started_ns = now_ns()?;
        let deadline_ns = started_ns.saturating_add(STARTUP_TIMEOUT_NS);
        let lease = self
            .lease
            .as_ref()
            .ok_or_else(|| io::Error::other("XWayland start requested without a display lease"))?;
        let (private_wayland, private_wayland_child) = UnixStream::pair()?;
        let (wm, wm_child) = UnixStream::pair()?;
        let (displayfd, displayfd_child) = displayfd::create_pipe()?;
        let displayfd_parent_fd = displayfd.as_raw_fd();
        let displayfd_child_source_fd = displayfd_child.as_raw_fd();
        self.log_displayfd_event(
            "displayfd_created",
            None,
            Some(generation),
            None,
            Some(displayfd_parent_fd),
            Some(displayfd_child_source_fd),
            None,
            None,
            Some(0),
        );
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
        let options = ProcessOptions::new(ProcessKind::Xwayland)
            .session_owned(true)
            .with_process_group_policy(ProcessGroupPolicy::Dedicated);
        let spawned = launch.spawn_with_stderr(supervisor, options)?;
        let process = spawned.process;
        let stderr = Some(StderrPipe::new(spawned.stderr, self.config.log_stderr));
        self.log_displayfd_event(
            "displayfd_child_mapped",
            None,
            Some(generation),
            Some(process.id),
            Some(displayfd_parent_fd),
            Some(displayfd_child_source_fd),
            None,
            None,
            None,
        );
        self.state = ServiceState::Starting(StartingResources {
            generation,
            process,
            displayfd,
            displayfd_child_source_fd,
            displayfd_reactor_token: None,
            private_wayland: Some(private_wayland),
            wm: Some(wm),
            xwm_startup: None,
            xwm_reactor_token: None,
            display_ready: false,
            displayfd_registered: false,
            displayfd_readable: false,
            private_wayland_endpoint_transferred: false,
            private_client_attached: false,
            private_client_authorized: false,
            shell_ready: false,
            xwm_connected: false,
            xwm_capabilities_validated: false,
            root_initialized: false,
            displayfd_bytes: Vec::new(),
            stderr,
            started_ns,
            deadline_ns,
        });
        self.metrics.state_transitions = self.metrics.state_transitions.saturating_add(1);
        self.log_state_transition();
        Ok(())
    }

    pub fn handle_stderr_ready(&mut self, generation: XwaylandGeneration) -> io::Result<()> {
        self.handle_stderr_ready_impl(generation)
    }

    #[cfg(test)]
    pub(crate) fn handle_shell_bind(&mut self, generation: XwaylandGeneration) -> io::Result<()> {
        self.mark_shell_ready(generation)
    }

    #[cfg(test)]
    pub(crate) fn mark_display_ready_for_tests(&mut self, generation: XwaylandGeneration) {
        if let ServiceState::Starting(resources) = &mut self.state
            && resources.generation == generation
        {
            resources.display_ready = true;
            resources.displayfd_registered = false;
        }
    }

    #[cfg(test)]
    pub(crate) fn install_xwm_startup_for_tests(&mut self, generation: XwaylandGeneration) {
        let ServiceState::Starting(resources) = &mut self.state else {
            return;
        };
        if resources.generation != generation {
            return;
        }
        let Ok((startup_stream, _peer)) = UnixStream::pair() else {
            return;
        };
        resources.wm = None;
        resources.xwm_startup =
            Some(XwmStartup::new(generation, startup_stream).expect("test XWM startup transport"));
    }

    pub fn authorize_private_client(
        &mut self,
        generation: XwaylandGeneration,
        client_id: ClientId,
    ) {
        self.private_client = Some((generation, client_id));
        if let ServiceState::Starting(resources) = &mut self.state
            && resources.generation == generation
        {
            resources.private_client_attached = true;
            resources.private_client_authorized = true;
        }
        self.log_readiness_progress("private_client_authorized");
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
            self.metrics.unauthorized_bind_attempts =
                self.metrics.unauthorized_bind_attempts.saturating_add(1);
            eprintln!(
                "oblivion-one xwayland: event=unauthorized_shell_bind generation={generation:?}"
            );
            return Ok(());
        }
        self.mark_shell_ready(generation)
    }

    pub fn record_stale_reactor_event(&mut self) {
        self.metrics.stale_events = self.metrics.stale_events.saturating_add(1);
        eprintln!("oblivion-one xwayland: event=stale_reactor_event");
    }

    fn mark_shell_ready(&mut self, generation: XwaylandGeneration) -> io::Result<()> {
        let ServiceState::Starting(resources) = &mut self.state else {
            return Ok(());
        };
        if resources.generation != generation {
            self.metrics.stale_events = self.metrics.stale_events.saturating_add(1);
            return Ok(());
        }
        resources.shell_ready = true;
        self.maybe_mark_running();
        self.log_readiness_progress("xwayland_shell_bound");
        Ok(())
    }

    fn maybe_mark_running(&mut self) {
        let ready = matches!(
            &self.state,
            ServiceState::Starting(resources) if resources.display_ready && resources.shell_ready
        );
        if !ready || self.config.profile == super::config::XwaylandProfile::Managed {
            return;
        }
        let ServiceState::Starting(resources) =
            std::mem::replace(&mut self.state, ServiceState::Disabled)
        else {
            unreachable!("readiness state changed while promoting XWayland")
        };
        let mut readiness = self.snapshot_for_starting(&resources);
        readiness.readiness_complete = true;
        self.last_readiness = Some(readiness);
        self.state = ServiceState::RunningBase(RunningBaseResources {
            generation: resources.generation,
            process: resources.process,
            private_wayland: resources.private_wayland,
            _wm: resources.wm,
            stderr: resources.stderr,
        });
        self.metrics.startup_duration_ns = now_ns()
            .ok()
            .map(|now| now.saturating_sub(resources.started_ns));
        self.metrics.state_transitions = self.metrics.state_transitions.saturating_add(1);
        eprintln!(
            "oblivion-one xwayland: event=readiness_complete generation={:?} display={:?} startup_ns={:?}",
            resources.generation,
            self.display_number(),
            self.metrics.startup_duration_ns
        );
        self.log_state_transition();
    }

    pub fn handle_process_exit(&mut self, exit: &ChildExit) -> io::Result<bool> {
        let compositor_requested = self
            .pending_termination
            .is_some_and(|pending| pending.process_id == exit.id);
        if compositor_requested {
            self.pending_termination = None;
        }
        let ready = matches!(
            self.state,
            ServiceState::RunningBase(_) | ServiceState::Running(_)
        );
        let process = match &self.state {
            ServiceState::Starting(resources) => resources.process,
            ServiceState::RunningBase(resources) => resources.process,
            ServiceState::Running(resources) => resources.process,
            _ => return Ok(false),
        };
        let exit_class = classify_exit(ready, compositor_requested, exit.status.success());
        eprintln!(
            "oblivion-one xwayland: event=exit_class generation={:?} class={exit_class:?}",
            self.generation()
        );
        if process.id != exit.id {
            self.metrics.stale_events = self.metrics.stale_events.saturating_add(1);
            return Ok(false);
        }
        if let Some(mut readiness) = self.readiness_snapshot() {
            readiness.process_alive = false;
            readiness.readiness_complete = false;
            eprintln!(
                "oblivion-one xwayland: event=process_exit generation={:?} display={} process_id={} success={} missing={:?}",
                readiness.generation,
                readiness.display,
                readiness.process_id.get(),
                exit.status.success(),
                readiness.missing_conditions(),
            );
            self.last_readiness = Some(readiness);
        }
        match exit_class {
            super::diagnostics::XwaylandExitClass::ExpectedIdleExitAfterRunning => {
                self.rearm(true);
            }
            super::diagnostics::XwaylandExitClass::ExpectedShutdownAfterRunning
            | super::diagnostics::XwaylandExitClass::CompositorRequestedTermination => {
                self.disable_after_process_exit();
            }
            super::diagnostics::XwaylandExitClass::StartupExitBeforeReadiness
            | super::diagnostics::XwaylandExitClass::CrashOrSignal => {
                self.mark_stderr_failure();
                self.enter_failure_backoff(now_ns()?);
            }
        }
        Ok(true)
    }

    pub fn record_association_events(&mut self, events: &[XwaylandAssociationEvent]) {
        let mut stale_or_rejected = 0u64;
        if let ServiceState::Running(resources) = &mut self.state {
            for event in events.iter().copied() {
                if resources.xwm.ingest_wayland_association(event).is_err() {
                    stale_or_rejected = stale_or_rejected.saturating_add(1);
                }
            }
        }
        for event in events {
            match event {
                XwaylandAssociationEvent::Committed {
                    generation,
                    serial,
                    surface_id,
                } => {
                    self.metrics.association_commits =
                        self.metrics.association_commits.saturating_add(1);
                    trace::emit("association_wayland_event", || {
                        TraceFields::new()
                            .field("source", "wayland")
                            .field("association_action", "committed")
                            .field("generation", generation.get())
                            .field("association_serial", serial.get())
                            .field("surface_id", surface_id)
                    });
                    eprintln!("oblivion-one xwayland: event=association_commit detail={event:?}");
                }
                XwaylandAssociationEvent::Removed {
                    generation,
                    serial,
                    surface_id,
                } => {
                    self.metrics.association_removals =
                        self.metrics.association_removals.saturating_add(1);
                    trace::emit("association_wayland_event", || {
                        TraceFields::new()
                            .field("source", "wayland")
                            .field("association_action", "removed")
                            .field("generation", generation.get())
                            .field("association_serial", serial.get())
                            .field("surface_id", surface_id)
                    });
                    eprintln!("oblivion-one xwayland: event=association_remove detail={event:?}");
                }
            }
        }
        self.metrics.stale_events = self.metrics.stale_events.saturating_add(stale_or_rejected);
    }

    pub fn take_managed_association_events(&mut self) -> Vec<super::xwm::XwmAssociationEvent> {
        match &mut self.state {
            ServiceState::Running(resources) => resources.xwm.take_association_events(),
            _ => Vec::new(),
        }
    }

    pub fn mark_managed_surface_buffer_ready(
        &mut self,
        supervisor: &mut ChildSupervisor,
        generation: XwaylandGeneration,
        surface_id: u32,
    ) -> io::Result<()> {
        trace::emit("buffer_ready_forwarded", || {
            TraceFields::new()
                .field("source", "native_runtime")
                .field("generation", generation.get())
                .field("surface_id", surface_id)
        });
        let result = match &mut self.state {
            ServiceState::Running(resources) if resources.generation == generation => resources
                .xwm
                .mark_surface_buffer_ready(generation, surface_id)
                .map_err(io::Error::other),
            ServiceState::Running(_) => {
                self.metrics.stale_events = self.metrics.stale_events.saturating_add(1);
                Ok(())
            }
            _ => Ok(()),
        };
        if let Err(error) = result {
            self.fail_managed_xwm(supervisor, XwaylandFailureStage::BufferReady, error);
        }
        Ok(())
    }

    pub fn take_managed_xwm_events(&mut self) -> Vec<super::xwm::XwmEvent> {
        let events = match &mut self.state {
            ServiceState::Running(resources) => resources
                .xwm
                .take_events()
                .inspect(|event| match event {
                    super::xwm::XwmEvent::ResizeSyncAcked { .. } => {
                        self.metrics.resize_sync_acks =
                            self.metrics.resize_sync_acks.saturating_add(1);
                    }
                    super::xwm::XwmEvent::ResizeSyncPresented(_) => {
                        self.metrics.resize_sync_presented =
                            self.metrics.resize_sync_presented.saturating_add(1);
                    }
                    super::xwm::XwmEvent::ResizeSyncPresentedIntermediate(_) => {
                        self.metrics.resize_sync_presented =
                            self.metrics.resize_sync_presented.saturating_add(1);
                    }
                    super::xwm::XwmEvent::ResizeSyncTimedOut(_) => {
                        self.metrics.resize_sync_timeouts =
                            self.metrics.resize_sync_timeouts.saturating_add(1);
                    }
                    super::xwm::XwmEvent::ResizeSyncTimedOutWithFollowup(_) => {
                        self.metrics.resize_sync_timeouts =
                            self.metrics.resize_sync_timeouts.saturating_add(1);
                    }
                    _ => {}
                })
                .collect(),
            _ => Vec::new(),
        };
        for event in &events {
            trace::emit("xwm_event_emitted", || {
                TraceFields::new()
                    .field("source", "xwm")
                    .field("event", format!("{event:?}"))
            });
        }
        events
    }

    #[allow(dead_code)]
    pub fn resize_sync_snapshot(&self, handle: super::X11WindowHandle) -> Option<(u64, bool)> {
        match &self.state {
            ServiceState::Running(resources) => resources
                .xwm
                .resize_sync
                .transaction(handle)
                .map(|(id, _, final_pending)| (id, final_pending)),
            _ => None,
        }
    }

    #[doc(hidden)]
    pub fn acknowledge_resize_sync_for_test(&mut self, handle: super::X11WindowHandle, value: u64) {
        if let ServiceState::Running(resources) = &mut self.state {
            resources.xwm.note_resize_sync_ack_for_test(handle, value);
        }
    }

    pub fn execute_managed_command(
        &mut self,
        supervisor: &mut ChildSupervisor,
        command: super::xwm::XwmCommand,
    ) -> io::Result<()> {
        if matches!(command, super::xwm::XwmCommand::BeginResizeSync { .. }) {
            self.metrics.resize_sync_started = self.metrics.resize_sync_started.saturating_add(1);
        }
        let result = match &mut self.state {
            ServiceState::Running(resources) => match resources.xwm.execute(command) {
                Ok(()) => None,
                Err(error) => Some(io::Error::other(error)),
            },
            _ => None,
        };
        if let Some(error) = result {
            self.metrics.resize_sync_command_failures =
                self.metrics.resize_sync_command_failures.saturating_add(1);
            self.fail_managed_xwm(supervisor, XwaylandFailureStage::CommandWrite, error);
        }
        Ok(())
    }

    pub fn flush_managed_commands(&mut self, supervisor: &mut ChildSupervisor) -> io::Result<()> {
        let result = match &mut self.state {
            ServiceState::Running(resources) => resources.xwm.flush().err().map(io::Error::other),
            _ => None,
        };
        if let Some(error) = result {
            self.fail_managed_xwm(supervisor, XwaylandFailureStage::CommandFlush, error);
        }
        Ok(())
    }

    pub fn handle_deadline(
        &mut self,
        now_ns: u64,
        supervisor: &mut ChildSupervisor,
    ) -> io::Result<()> {
        if let Some(pending) = self.pending_termination {
            if !supervisor.contains_id(pending.process_id) {
                self.pending_termination = None;
            } else if !pending.escalated && now_ns >= pending.deadline_ns {
                self.kill_process_now(supervisor, pending.process_id)?;
                self.pending_termination = Some(PendingTermination {
                    escalated: true,
                    ..pending
                });
            }
        }
        let resize_sync_error = if let ServiceState::Running(resources) = &mut self.state {
            resources.xwm.collect_adoption_expirations(now_ns);
            resources
                .xwm
                .handle_resize_sync_deadline(now_ns)
                .err()
                .map(io::Error::other)
        } else {
            None
        };
        if let Some(error) = resize_sync_error {
            self.fail_managed_xwm(supervisor, XwaylandFailureStage::CommandFlush, error);
        }
        let startup_timed_out = matches!(
            &self.state,
            ServiceState::Starting(resources) if now_ns >= resources.deadline_ns
        );
        if startup_timed_out {
            let (generation, process_id) = match &self.state {
                ServiceState::Starting(resources) => (resources.generation, resources.process.id),
                _ => unreachable!("startup timeout state changed before diagnostics"),
            };
            let process_alive = supervisor.contains_id(process_id);
            self.log_displayfd_event(
                "displayfd_probe",
                Some("timeout_final"),
                Some(generation),
                Some(process_id),
                self.displayfd_parent_fd(generation),
                self.displayfd_child_source_fd(generation),
                self.displayfd_reactor_token(generation),
                None,
                None,
            );
            if process_alive && let Err(error) = self.probe_displayfd(generation, supervisor) {
                eprintln!(
                    "oblivion-one xwayland: event=displayfd_final_probe_failed generation={generation:?} error={error}"
                );
            }
            if !matches!(
                &self.state,
                ServiceState::Starting(resources) if resources.generation == generation
            ) {
                return Ok(());
            }
            let mut readiness = match &self.state {
                ServiceState::Starting(resources) => self.snapshot_for_starting(resources),
                _ => unreachable!("startup timeout state changed after final probe"),
            };
            readiness.process_alive = process_alive;
            self.last_readiness = Some(readiness);
            eprintln!(
                "oblivion-one xwayland: event=readiness_timeout generation={:?} display={} process_id={} elapsed_ns={} process_alive={} displayfd_registered={} displayfd_readable={} display_number_validated={} private_wayland_endpoint_transferred={} private_client_attached={} private_client_authorized={} xwayland_shell_bound={} xwm_connected={} xwm_capabilities_validated={} root_initialized={} readiness_complete=false missing={:?}",
                readiness.generation,
                readiness.display,
                readiness.process_id.get(),
                readiness.elapsed_ns,
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
                readiness.missing_conditions(),
            );
            self.metrics.readiness_failures = self.metrics.readiness_failures.saturating_add(1);
            self.request_process_termination(supervisor, process_id)?;
            self.mark_stderr_failure();
            self.enter_failure_backoff(now_ns);
        } else if matches!(&self.state, ServiceState::Backoff { deadline_ns, .. } if now_ns >= *deadline_ns)
        {
            self.rearm(false);
        }
        Ok(())
    }

    pub fn begin_shutdown(&mut self, supervisor: &mut ChildSupervisor) -> io::Result<()> {
        self.stop_current(supervisor)?;
        self.private_client = None;
        if self.retired_lease.is_none() {
            self.retired_lease = self.lease.take();
        } else {
            drop(self.lease.take());
        }
        self.replace_state(ServiceState::Disabled);
        self.log_state_transition();
        Ok(())
    }

    pub fn emergency_cleanup(&mut self, supervisor: &mut ChildSupervisor) -> io::Result<()> {
        self.begin_shutdown(supervisor)
    }

    fn stop_current(&mut self, supervisor: &mut ChildSupervisor) -> io::Result<()> {
        let process_id = match &self.state {
            ServiceState::Starting(resources) => Some(resources.process.id),
            ServiceState::RunningBase(resources) => Some(resources.process.id),
            ServiceState::Running(resources) => Some(resources.process.id),
            ServiceState::Disabled
            | ServiceState::Armed
            | ServiceState::Backoff { .. }
            | ServiceState::Failed => None,
        };
        if let Some(process_id) = process_id {
            self.request_process_termination(supervisor, process_id)?;
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
            Ok(_) => {
                eprintln!(
                    "oblivion-one xwayland: event=cleanup_result process_id={} result=ok",
                    process_id.get()
                );
                Ok(())
            }
            Err(error) => {
                self.metrics.cleanup_failures = self.metrics.cleanup_failures.saturating_add(1);
                eprintln!(
                    "oblivion-one xwayland: event=cleanup_result process_id={} result=error error={error}",
                    process_id.get()
                );
                Err(error)
            }
        }
    }

    fn request_process_termination(
        &mut self,
        supervisor: &mut ChildSupervisor,
        process_id: ManagedProcessId,
    ) -> io::Result<()> {
        if !supervisor.terminate_managed(process_id)? {
            return Ok(());
        }
        self.pending_termination = Some(PendingTermination {
            process_id,
            deadline_ns: now_ns()?.saturating_add(750_000_000),
            escalated: false,
        });
        Ok(())
    }

    fn rearm(&mut self, reset_failure_budget: bool) {
        self.private_client = None;
        if self.mode.is_enabled() && self.lease.is_some() {
            self.replace_state(ServiceState::Armed);
        } else {
            self.replace_state(ServiceState::Disabled);
        }
        if reset_failure_budget {
            self.crash_times_ns.clear();
            self.backoff_level = 0;
            self.metrics.backoff_level = 0;
        }
        self.metrics.state_transitions = self.metrics.state_transitions.saturating_add(1);
        self.log_state_transition();
    }

    fn disable_after_process_exit(&mut self) {
        self.private_client = None;
        self.replace_state(ServiceState::Disabled);
        self.metrics.state_transitions = self.metrics.state_transitions.saturating_add(1);
        self.log_state_transition();
    }

    fn enter_backoff(&mut self, now_ns: u64) {
        self.private_client = None;
        let level = self.backoff_level.min(BACKOFF_NS.len().saturating_sub(1));
        let deadline_ns = now_ns.saturating_add(BACKOFF_NS[level]);
        self.backoff_level = self.backoff_level.saturating_add(1);
        self.metrics.backoff_level = self.backoff_level;
        self.replace_state(ServiceState::Backoff { deadline_ns });
        self.metrics.state_transitions = self.metrics.state_transitions.saturating_add(1);
        eprintln!(
            "oblivion-one xwayland: event=backoff level={} deadline_ns={deadline_ns}",
            self.backoff_level
        );
        self.log_state_transition();
    }

    fn enter_failure_backoff(&mut self, now_ns: u64) {
        while self
            .crash_times_ns
            .front()
            .is_some_and(|started| now_ns.saturating_sub(*started) > CRASH_WINDOW_NS)
        {
            self.crash_times_ns.pop_front();
        }
        self.crash_times_ns.push_back(now_ns);
        self.metrics.crashes = self.metrics.crashes.saturating_add(1);
        if self.crash_times_ns.len() >= 3 {
            self.private_client = None;
            self.replace_state(ServiceState::Failed);
            self.metrics.state_transitions = self.metrics.state_transitions.saturating_add(1);
            eprintln!(
                "oblivion-one xwayland: event=failure_budget_exhausted failures={} ",
                self.crash_times_ns.len()
            );
            self.log_state_transition();
        } else {
            self.enter_backoff(now_ns);
        }
    }

    fn fail_generation(
        &mut self,
        supervisor: &mut ChildSupervisor,
        error: io::Error,
    ) -> io::Result<()> {
        self.fail_generation_with_stage(supervisor, XwaylandFailureStage::Startup, error)
    }

    fn fail_generation_with_stage(
        &mut self,
        supervisor: &mut ChildSupervisor,
        stage: XwaylandFailureStage,
        error: io::Error,
    ) -> io::Result<()> {
        let generation = self.generation();
        self.record_failure(generation, stage, &error);
        let process_id = match &self.state {
            ServiceState::Starting(resources) => Some(resources.process.id),
            _ => None,
        };
        if let Some(process_id) = process_id
            && let Err(cleanup_error) = self.request_process_termination(supervisor, process_id)
        {
            eprintln!(
                "oblivion-one xwayland: event=startup_cleanup_failure generation={generation:?} reason={cleanup_error}"
            );
        }
        self.private_client = None;
        self.mark_stderr_failure();
        self.metrics.readiness_failures = self.metrics.readiness_failures.saturating_add(1);
        eprintln!(
            "oblivion-one xwayland: event=readiness_failure generation={generation:?} stage={stage:?} reason={error}"
        );
        self.enter_failure_backoff(now_ns().unwrap_or_default());
        Err(error)
    }

    fn record_failure(
        &mut self,
        generation: Option<XwaylandGeneration>,
        stage: XwaylandFailureStage,
        error: &io::Error,
    ) {
        self.latest_failure = Some(XwaylandFailure {
            generation,
            stage,
            reason: error.to_string(),
        });
    }

    #[cfg(test)]
    pub(crate) fn inject_xwm_failure_for_tests(
        &mut self,
        supervisor: &mut ChildSupervisor,
        stage: XwaylandFailureStage,
        reason: &str,
    ) {
        let error = io::Error::other(reason);
        if matches!(self.state, ServiceState::Running(_)) {
            self.fail_managed_xwm(supervisor, stage, error);
        } else {
            let _ = self.fail_generation_with_stage(supervisor, stage, error);
        }
    }

    #[cfg(test)]
    pub(crate) fn latest_failure_stage_for_tests(&self) -> Option<XwaylandFailureStage> {
        self.latest_failure.as_ref().map(|failure| failure.stage)
    }

    fn replace_state(&mut self, next: ServiceState) {
        let previous = std::mem::replace(&mut self.state, next);
        match previous {
            ServiceState::Starting(resources) => {
                self.retired_resources
                    .push(RetiredResources::Starting(resources));
            }
            ServiceState::RunningBase(resources) => {
                self.retired_resources
                    .push(RetiredResources::RunningBase(resources));
            }
            ServiceState::Running(resources) => {
                self.retired_resources
                    .push(RetiredResources::Running(resources));
            }
            ServiceState::Disabled
            | ServiceState::Armed
            | ServiceState::Backoff { .. }
            | ServiceState::Failed => {}
        }
    }
}
