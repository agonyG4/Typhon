use std::{
    collections::{HashMap, VecDeque},
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    os::unix::process::CommandExt,
    process::{Child, Command, ExitStatus, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ManagedProcessId(u64);

impl ManagedProcessId {
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpawnedProcess {
    pub id: ManagedProcessId,
    pub pid: u32,
    pub pgid: Option<i32>,
}

#[derive(Debug)]
pub struct SpawnedProcessWithStderr {
    pub process: SpawnedProcess,
    pub stderr: OwnedFd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProcessKind {
    ShellSessionCritical,
    Application,
    SessionService,
    Infrastructure,
    Xwayland,
}

impl ProcessKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ShellSessionCritical => "shell_session_critical",
            Self::Application => "application",
            Self::SessionService => "session_service",
            Self::Infrastructure => "infrastructure",
            Self::Xwayland => "xwayland",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessGroupPolicy {
    Inherit,
    Dedicated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartPolicy {
    Never,
    OnFailure,
    CriticalSessionComponent,
}

impl RestartPolicy {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::OnFailure => "on_failure",
            Self::CriticalSessionComponent => "critical_session_component",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOptions {
    pub kind: ProcessKind,
    pub session_owned: bool,
    pub process_group_policy: ProcessGroupPolicy,
    pub restart_policy: RestartPolicy,
    pub label: Option<String>,
}

impl ProcessOptions {
    pub fn new(kind: ProcessKind) -> Self {
        Self {
            kind,
            session_owned: !matches!(kind, ProcessKind::Application),
            process_group_policy: ProcessGroupPolicy::Inherit,
            restart_policy: RestartPolicy::Never,
            label: None,
        }
    }

    pub fn session_owned(mut self, session_owned: bool) -> Self {
        self.session_owned = session_owned;
        self
    }

    pub fn with_process_group_policy(mut self, policy: ProcessGroupPolicy) -> Self {
        self.process_group_policy = policy;
        self
    }

    pub fn with_restart_policy(mut self, restart_policy: RestartPolicy) -> Self {
        self.restart_policy = restart_policy;
        self
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestartGuard {
    max_restarts: usize,
    window: Duration,
}

impl RestartGuard {
    pub const fn new(max_restarts: usize, window: Duration) -> Self {
        Self {
            max_restarts,
            window,
        }
    }
}

impl Default for RestartGuard {
    fn default() -> Self {
        Self {
            max_restarts: 3,
            window: Duration::from_secs(30),
        }
    }
}

#[derive(Debug)]
pub struct ChildFdMapping {
    pub source: OwnedFd,
    pub target: RawFd,
}

pub struct SpawnCommand {
    pub command: Command,
    pub inherited_fds: Vec<ChildFdMapping>,
}

const MAX_MAPPED_CHILD_FDS: usize = 32;

impl SpawnCommand {
    pub fn new(command: Command) -> Self {
        Self {
            command,
            inherited_fds: Vec::new(),
        }
    }

    pub fn map_fd(&mut self, source: OwnedFd, target: RawFd) -> io::Result<()> {
        if source.as_raw_fd() < 0 || target < 3 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "inherited fd source and target must be valid non-stdio descriptors",
            ));
        }
        if self
            .inherited_fds
            .iter()
            .any(|mapping| mapping.target == target)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "inherited fd targets must be unique",
            ));
        }
        if self.inherited_fds.len() >= MAX_MAPPED_CHILD_FDS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "too many inherited child fds",
            ));
        }
        self.inherited_fds.push(ChildFdMapping { source, target });
        Ok(())
    }

    pub fn spawn(
        self,
        supervisor: &mut ChildSupervisor,
        options: ProcessOptions,
    ) -> io::Result<SpawnedProcess> {
        let Self {
            mut command,
            inherited_fds,
        } = self;
        validate_fd_mappings(&inherited_fds)?;
        let mappings = inherited_fds
            .iter()
            .map(|mapping| (mapping.source.as_raw_fd(), mapping.target))
            .collect::<Vec<_>>();
        // SAFETY: the closure is installed only for the child between fork and
        // exec and calls configure_child_fds, which uses only libc syscalls and
        // does not access Rust allocation, locks, or other non-async-safe APIs.
        unsafe {
            command.pre_exec(move || configure_child_fds(&mappings));
        }
        supervisor.spawn_with_identity(command, options)
    }

    pub fn spawn_with_stderr(
        self,
        supervisor: &mut ChildSupervisor,
        options: ProcessOptions,
    ) -> io::Result<SpawnedProcessWithStderr> {
        let Self {
            mut command,
            inherited_fds,
        } = self;
        validate_fd_mappings(&inherited_fds)?;
        let mappings = inherited_fds
            .iter()
            .map(|mapping| (mapping.source.as_raw_fd(), mapping.target))
            .collect::<Vec<_>>();
        // SAFETY: the closure is installed only for the child between fork and
        // exec and calls configure_child_fds, which uses only libc syscalls and
        // does not access Rust allocation, locks, or other non-async-safe APIs.
        unsafe {
            command.pre_exec(move || configure_child_fds(&mappings));
        }
        command.stderr(Stdio::piped());
        supervisor.spawn_with_identity_and_stderr(command, options)
    }
}

fn validate_fd_mappings(mappings: &[ChildFdMapping]) -> io::Result<()> {
    if mappings.len() > MAX_MAPPED_CHILD_FDS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "too many inherited child fds",
        ));
    }
    for (index, mapping) in mappings.iter().enumerate() {
        if mapping.source.as_raw_fd() < 0 || mapping.target < 3 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "inherited fd source and target must be valid non-stdio descriptors",
            ));
        }
        if mappings[..index]
            .iter()
            .any(|previous| previous.target == mapping.target)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "inherited fd targets must be unique",
            ));
        }
    }
    Ok(())
}

fn configure_child_fds(mappings: &[(RawFd, RawFd)]) -> io::Result<()> {
    let mut temporary_fds = [-1; MAX_MAPPED_CHILD_FDS];
    for (index, (source, _)) in mappings.iter().enumerate() {
        let temporary = unsafe { libc::fcntl(*source, libc::F_DUPFD_CLOEXEC, 100) };
        if temporary < 0 {
            close_temporary_fds(&temporary_fds[..index]);
            return Err(io::Error::last_os_error());
        }
        temporary_fds[index] = temporary;
    }

    let close_range_result = unsafe { libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, 4u32) };
    if close_range_result < 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ENOSYS) {
            close_temporary_fds(&temporary_fds[..mappings.len()]);
            return Err(error);
        }
        for fd in 3..=65_535 {
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
            if flags >= 0 {
                let _ = unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) };
            }
        }
    }

    for (index, (_, target)) in mappings.iter().enumerate() {
        let temporary = temporary_fds[index];
        if unsafe { libc::dup2(temporary, *target) } < 0 {
            let error = io::Error::last_os_error();
            close_temporary_fds(&temporary_fds[..mappings.len()]);
            return Err(error);
        }
        let flags = unsafe { libc::fcntl(*target, libc::F_GETFD) };
        if flags < 0 {
            let error = io::Error::last_os_error();
            close_temporary_fds(&temporary_fds[..mappings.len()]);
            return Err(error);
        }
        if unsafe { libc::fcntl(*target, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0 {
            let error = io::Error::last_os_error();
            close_temporary_fds(&temporary_fds[..mappings.len()]);
            return Err(error);
        }
    }
    close_temporary_fds(&temporary_fds[..mappings.len()]);
    Ok(())
}

fn close_temporary_fds(fds: &[RawFd]) {
    for fd in fds {
        unsafe {
            libc::close(*fd);
        }
    }
}

#[derive(Debug)]
pub struct ChildExit {
    pub id: ManagedProcessId,
    pub pid: u32,
    pub pgid: Option<i32>,
    pub kind: ProcessKind,
    pub status: ExitStatus,
    pub restarted: Option<SpawnedProcess>,
    pub restarted_pid: Option<u32>,
}

type RestartFactory = Arc<dyn Fn() -> io::Result<Command> + Send + Sync + 'static>;

struct ManagedChild {
    spawned: SpawnedProcess,
    child: Child,
    options: ProcessOptions,
    launched_at: Instant,
    restart_factory: Option<RestartFactory>,
    restart_history: VecDeque<Instant>,
}

#[derive(Debug)]
struct SupervisorShutdown {
    started_at: Instant,
    sigkill_sent: bool,
}

pub struct ChildSupervisor {
    children: HashMap<ManagedProcessId, ManagedChild>,
    pid_to_id: HashMap<u32, ManagedProcessId>,
    next_process_id: u64,
    restart_guard: RestartGuard,
    restart_suppression_count: u64,
    quiescing: bool,
    shutdown: Option<SupervisorShutdown>,
    sigchld_fd: Option<OwnedFd>,
}

impl ChildSupervisor {
    const TERM_GRACE: Duration = Duration::from_millis(750);

    pub fn new() -> Self {
        Self {
            children: HashMap::new(),
            pid_to_id: HashMap::new(),
            next_process_id: 1,
            restart_guard: RestartGuard::default(),
            restart_suppression_count: 0,
            quiescing: false,
            shutdown: None,
            sigchld_fd: None,
        }
    }

    pub fn with_sigchld_reaper() -> io::Result<Self> {
        let mut supervisor = Self::new();
        supervisor.sigchld_fd = Some(create_sigchld_signalfd()?);
        Ok(supervisor)
    }

    pub fn signal_fd(&self) -> Option<RawFd> {
        self.sigchld_fd.as_ref().map(AsRawFd::as_raw_fd)
    }

    pub fn set_restart_guard(&mut self, guard: RestartGuard) {
        self.restart_guard = guard;
    }

    pub fn spawn(&mut self, mut command: Command, options: ProcessOptions) -> io::Result<u32> {
        Ok(self
            .spawn_inner(&mut command, options, None, VecDeque::new())?
            .0
            .pid)
    }

    pub fn spawn_with_identity(
        &mut self,
        mut command: Command,
        options: ProcessOptions,
    ) -> io::Result<SpawnedProcess> {
        Ok(self
            .spawn_inner(&mut command, options, None, VecDeque::new())?
            .0)
    }

    pub fn spawn_with_identity_and_stderr(
        &mut self,
        mut command: Command,
        options: ProcessOptions,
    ) -> io::Result<SpawnedProcessWithStderr> {
        command.stderr(Stdio::piped());
        let (process, stderr) = self.spawn_inner(&mut command, options, None, VecDeque::new())?;
        let stderr = stderr.ok_or_else(|| io::Error::other("child stderr pipe was not created"))?;
        if let Err(error) = set_fd_nonblocking(stderr.as_raw_fd()) {
            let _ = self.kill_managed_now(process.id);
            return Err(error);
        }
        Ok(SpawnedProcessWithStderr { process, stderr })
    }

    pub fn spawn_restartable<F>(&mut self, factory: F, options: ProcessOptions) -> io::Result<u32>
    where
        F: Fn() -> io::Result<Command> + Send + Sync + 'static,
    {
        let factory: RestartFactory = Arc::new(factory);
        let mut command = factory()?;
        Ok(self
            .spawn_inner(&mut command, options, Some(factory), VecDeque::new())?
            .0
            .pid)
    }

    fn spawn_inner(
        &mut self,
        command: &mut Command,
        options: ProcessOptions,
        restart_factory: Option<RestartFactory>,
        restart_history: VecDeque<Instant>,
    ) -> io::Result<(SpawnedProcess, Option<OwnedFd>)> {
        if options.process_group_policy == ProcessGroupPolicy::Dedicated && !options.session_owned {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "dedicated process groups require session ownership",
            ));
        }
        if options.process_group_policy == ProcessGroupPolicy::Dedicated {
            command.process_group(0);
        }
        let id = self.allocate_process_id()?;
        let mut child = command.spawn()?;
        let stderr = child.stderr.take().map(|stderr| {
            let raw_fd = std::os::fd::IntoRawFd::into_raw_fd(stderr);
            // SAFETY: `ChildStderr::into_raw_fd` transfers ownership of this
            // descriptor to the returned `OwnedFd`.
            unsafe { OwnedFd::from_raw_fd(raw_fd) }
        });
        let pid = child.id();
        let pgid = match options.process_group_policy {
            ProcessGroupPolicy::Inherit => None,
            ProcessGroupPolicy::Dedicated => match process_group_for_pid(pid) {
                Ok(pgid) => Some(pgid),
                Err(error) => {
                    let mut child = child;
                    drop(stderr);
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(error);
                }
            },
        };
        let spawned = SpawnedProcess { id, pid, pgid };
        eprintln!(
            "oblivion-one process: spawn kind={} id={} pid={} pgid={} restart_policy={} session_owned={} label={}",
            options.kind.as_str(),
            id.get(),
            pid,
            pgid.map_or_else(|| "none".to_string(), |pgid| pgid.to_string()),
            options.restart_policy.as_str(),
            options.session_owned,
            options.label.as_deref().unwrap_or("none")
        );
        self.children.insert(
            id,
            ManagedChild {
                spawned,
                child,
                options,
                launched_at: Instant::now(),
                restart_factory,
                restart_history,
            },
        );
        self.pid_to_id.insert(pid, id);
        Ok((spawned, stderr))
    }

    pub fn contains(&self, pid: u32) -> bool {
        self.pid_to_id.contains_key(&pid)
    }

    pub fn contains_id(&self, id: ManagedProcessId) -> bool {
        self.children.contains_key(&id)
    }

    pub fn active_count(&self) -> usize {
        self.children.len()
    }

    pub fn restart_suppression_count(&self) -> u64 {
        self.restart_suppression_count
    }

    pub fn reap_exited(&mut self) -> io::Result<Vec<ChildExit>> {
        self.drain_sigchld()?;
        let mut exited = Vec::new();
        let ids = self.children.keys().copied().collect::<Vec<_>>();
        for id in ids {
            let status = {
                let Some(child) = self.children.get_mut(&id) else {
                    continue;
                };
                child.child.try_wait()?
            };
            let Some(status) = status else {
                continue;
            };
            let Some(mut child) = self.children.remove(&id) else {
                continue;
            };
            self.pid_to_id.remove(&child.spawned.pid);
            let restarted = self.maybe_restart_child(&mut child, status)?;
            let restarted_pid = restarted.map(|spawned| spawned.pid);
            eprintln!(
                "oblivion-one process: exit kind={} id={} pid={} pgid={} status={} runtime_ms={} restarted_pid={}",
                child.options.kind.as_str(),
                child.spawned.id.get(),
                child.spawned.pid,
                child
                    .spawned
                    .pgid
                    .map_or_else(|| "none".to_string(), |pgid| pgid.to_string()),
                exit_status_label(status),
                child.launched_at.elapsed().as_millis(),
                restarted_pid
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| "none".to_string())
            );
            exited.push(ChildExit {
                id: child.spawned.id,
                pid: child.spawned.pid,
                pgid: child.spawned.pgid,
                kind: child.options.kind,
                status,
                restarted,
                restarted_pid,
            });
        }
        Ok(exited)
    }

    fn maybe_restart_child(
        &mut self,
        child: &mut ManagedChild,
        status: ExitStatus,
    ) -> io::Result<Option<SpawnedProcess>> {
        if self.quiescing {
            return Ok(None);
        }
        let should_restart = match child.options.restart_policy {
            RestartPolicy::Never => false,
            RestartPolicy::OnFailure => !status.success(),
            RestartPolicy::CriticalSessionComponent => true,
        };
        if !should_restart {
            return Ok(None);
        }
        let Some(factory) = child.restart_factory.clone() else {
            eprintln!(
                "oblivion-one process: restart suppressed kind={} reason=no_restart_command",
                child.options.kind.as_str()
            );
            return Ok(None);
        };
        let now = Instant::now();
        while child
            .restart_history
            .front()
            .is_some_and(|started| now.duration_since(*started) > self.restart_guard.window)
        {
            child.restart_history.pop_front();
        }
        if child.restart_history.len() >= self.restart_guard.max_restarts {
            self.restart_suppression_count = self.restart_suppression_count.saturating_add(1);
            eprintln!(
                "oblivion-one process: restart suppressed kind={} reason=crash_loop attempts={} window_ms={}",
                child.options.kind.as_str(),
                child.restart_history.len(),
                self.restart_guard.window.as_millis()
            );
            return Ok(None);
        }
        child.restart_history.push_back(now);
        let mut command = match factory() {
            Ok(command) => command,
            Err(error) => {
                self.restart_suppression_count = self.restart_suppression_count.saturating_add(1);
                eprintln!(
                    "oblivion-one process: restart suppressed kind={} reason=restart_command_failed error={}",
                    child.options.kind.as_str(),
                    error
                );
                return Ok(None);
            }
        };
        let spawned = match self.spawn_inner(
            &mut command,
            child.options.clone(),
            Some(factory),
            child.restart_history.clone(),
        ) {
            Ok((spawned, _stderr)) => spawned,
            Err(error) => {
                self.restart_suppression_count = self.restart_suppression_count.saturating_add(1);
                eprintln!(
                    "oblivion-one process: restart suppressed kind={} reason=restart_spawn_failed error={}",
                    child.options.kind.as_str(),
                    error
                );
                return Ok(None);
            }
        };
        Ok(Some(spawned))
    }

    pub fn begin_quiesce(&mut self) {
        self.quiescing = true;
    }

    pub fn begin_shutdown(&mut self, now: Instant) -> io::Result<()> {
        self.begin_quiesce();
        if self.shutdown.is_some() {
            return Ok(());
        }
        self.shutdown = Some(SupervisorShutdown {
            started_at: now,
            sigkill_sent: false,
        });
        for child in self.children.values() {
            if child.options.session_owned {
                eprintln!(
                    "oblivion-one process: shutdown signal=SIGTERM kind={} pid={} pgid={}",
                    child.options.kind.as_str(),
                    child.spawned.pid,
                    child
                        .spawned
                        .pgid
                        .map_or_else(|| "none".to_string(), |pgid| pgid.to_string())
                );
                signal_spawned_process(child.spawned, libc::SIGTERM)?;
            }
        }
        Ok(())
    }

    pub fn advance_shutdown(&mut self, now: Instant) -> io::Result<bool> {
        let _ = self.reap_exited()?;
        let Some(shutdown) = &mut self.shutdown else {
            return Ok(false);
        };
        let session_owned = self
            .children
            .iter()
            .filter(|(_, child)| child.options.session_owned)
            .map(|(_, child)| child.spawned)
            .collect::<Vec<_>>();
        if session_owned.is_empty() {
            return Ok(true);
        }
        if !shutdown.sigkill_sent && now.duration_since(shutdown.started_at) >= Self::TERM_GRACE {
            shutdown.sigkill_sent = true;
            for spawned in session_owned {
                if let Some(child) = self.children.get(&spawned.id) {
                    eprintln!(
                        "oblivion-one process: shutdown signal=SIGKILL kind={} pid={} pgid={}",
                        child.options.kind.as_str(),
                        spawned.pid,
                        spawned
                            .pgid
                            .map_or_else(|| "none".to_string(), |pgid| pgid.to_string())
                    );
                }
                signal_spawned_process(spawned, libc::SIGKILL)?;
            }
        }
        let _ = self.reap_exited()?;
        Ok(!self
            .children
            .values()
            .any(|child| child.options.session_owned))
    }

    pub fn begin_emergency_cleanup(&mut self) {
        self.begin_quiesce();
        for child in self.children.values() {
            if child.options.session_owned {
                let _ = signal_spawned_process(child.spawned, libc::SIGTERM);
            }
        }
    }

    pub fn kill_session_owned_now(&mut self) -> io::Result<()> {
        self.begin_emergency_cleanup();
        for child in self.children.values() {
            if child.options.session_owned {
                signal_spawned_process(child.spawned, libc::SIGKILL)?;
            }
        }
        let _ = self.reap_exited()?;
        Ok(())
    }

    pub fn kill_managed_now(&mut self, id: ManagedProcessId) -> io::Result<bool> {
        let Some(child) = self.children.get(&id) else {
            return Ok(false);
        };
        signal_spawned_process(child.spawned, libc::SIGKILL)?;
        let _ = self.reap_exited()?;
        Ok(!self.children.contains_key(&id))
    }

    pub fn bootstrap_guard(&mut self) -> BootstrapChildGuard<'_> {
        BootstrapChildGuard {
            supervisor: self,
            tracked: Vec::new(),
            committed: false,
        }
    }

    fn allocate_process_id(&mut self) -> io::Result<ManagedProcessId> {
        let id = ManagedProcessId(self.next_process_id);
        self.next_process_id = self
            .next_process_id
            .checked_add(1)
            .ok_or_else(|| io::Error::other("managed process identity exhausted"))?;
        Ok(id)
    }

    #[cfg(test)]
    fn terminate_all_for_tests(&mut self) {
        for child in self.children.values_mut() {
            let _ = child.child.kill();
        }
        for (_, mut child) in self.children.drain() {
            let _ = child.child.wait();
        }
        self.pid_to_id.clear();
    }

    fn drain_sigchld(&self) -> io::Result<()> {
        let Some(fd) = self.sigchld_fd.as_ref() else {
            return Ok(());
        };
        let mut info = std::mem::MaybeUninit::<libc::signalfd_siginfo>::uninit();
        loop {
            let read = unsafe {
                libc::read(
                    fd.as_raw_fd(),
                    info.as_mut_ptr().cast(),
                    std::mem::size_of::<libc::signalfd_siginfo>(),
                )
            };
            if read < 0 {
                let error = io::Error::last_os_error();
                return match error.kind() {
                    io::ErrorKind::WouldBlock => Ok(()),
                    io::ErrorKind::Interrupted => continue,
                    _ => Err(error),
                };
            }
            if read == 0 {
                return Ok(());
            }
        }
    }
}

pub struct BootstrapChildGuard<'a> {
    supervisor: &'a mut ChildSupervisor,
    tracked: Vec<ManagedProcessId>,
    committed: bool,
}

impl BootstrapChildGuard<'_> {
    pub fn spawn(
        &mut self,
        command: Command,
        options: ProcessOptions,
    ) -> io::Result<SpawnedProcess> {
        let spawned = self
            .supervisor
            .spawn_with_identity(command, options.clone())?;
        if options.session_owned {
            self.tracked.push(spawned.id);
        }
        Ok(spawned)
    }

    pub fn supervisor(&mut self) -> &mut ChildSupervisor {
        self.supervisor
    }

    pub fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for BootstrapChildGuard<'_> {
    fn drop(&mut self) {
        if self.committed || self.tracked.is_empty() {
            return;
        }
        self.supervisor.begin_emergency_cleanup();
        for id in &self.tracked {
            if let Some(child) = self.supervisor.children.get(id) {
                let _ = signal_spawned_process(child.spawned, libc::SIGKILL);
            }
        }
        let _ = self.supervisor.reap_exited();
    }
}

impl Default for ChildSupervisor {
    fn default() -> Self {
        Self::new()
    }
}

fn signal_process(pid: u32, signal: libc::c_int) -> io::Result<()> {
    let pid = libc::pid_t::try_from(pid)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "pid does not fit pid_t"))?;
    let result = unsafe { libc::kill(pid, signal) };
    if result < 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(error);
        }
    }
    Ok(())
}

fn set_fd_nonblocking(fd: RawFd) -> io::Result<()> {
    // SAFETY: `fd` is owned by the caller and remains alive for both fcntl
    // operations.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the flags came from `F_GETFL` for the same live descriptor.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn signal_spawned_process(spawned: SpawnedProcess, signal: libc::c_int) -> io::Result<()> {
    if let Some(pgid) = spawned.pgid {
        signal_process_group(pgid, signal)
    } else {
        signal_process(spawned.pid, signal)
    }
}

fn signal_process_group(pgid: i32, signal: libc::c_int) -> io::Result<()> {
    if pgid <= 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "process group id must be positive",
        ));
    }
    let target = pgid
        .checked_neg()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "process group id overflow"))?;
    let result = unsafe { libc::kill(target, signal) };
    if result < 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(error);
        }
    }
    Ok(())
}

fn process_group_for_pid(pid: u32) -> io::Result<i32> {
    let pid = libc::pid_t::try_from(pid)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "pid does not fit pid_t"))?;
    let pgid = unsafe { libc::getpgid(pid) };
    if pgid <= 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(pgid)
    }
}

impl Drop for ChildSupervisor {
    fn drop(&mut self) {
        if self.children.is_empty() {
            return;
        }
        self.begin_quiesce();
        for child in self.children.values() {
            if child.options.session_owned {
                let _ = signal_spawned_process(child.spawned, libc::SIGKILL);
            }
        }
        let _ = self.reap_exited();
    }
}

fn create_sigchld_signalfd() -> io::Result<OwnedFd> {
    let mut mask = unsafe { std::mem::zeroed::<libc::sigset_t>() };
    if unsafe { libc::sigemptyset(&mut mask) } < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::sigaddset(&mut mask, libc::SIGCHLD) } < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::sigprocmask(libc::SIG_BLOCK, &mask, std::ptr::null_mut()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = unsafe { libc::signalfd(-1, &mask, libc::SFD_CLOEXEC | libc::SFD_NONBLOCK) };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }
}

fn exit_status_label(status: ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return format!("signal:{signal}");
        }
    }
    status
        .code()
        .map(|code| format!("code:{code}"))
        .unwrap_or_else(|| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use std::{
        process::Command,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        thread,
        time::{Duration, Instant},
    };

    use super::*;

    fn shell_command(script: &str) -> Command {
        let mut command = Command::new("sh");
        command.arg("-c").arg(script);
        command
    }

    fn wait_for_reap(supervisor: &mut ChildSupervisor) -> Vec<ChildExit> {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let exits = supervisor.reap_exited().expect("reap children");
            if !exits.is_empty() || Instant::now() >= deadline {
                return exits;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn launched_child_is_registered_with_supervisor() {
        let mut supervisor = ChildSupervisor::new();
        let pid = supervisor
            .spawn(
                shell_command("sleep 0.2"),
                ProcessOptions::new(ProcessKind::Application),
            )
            .expect("spawn child");

        assert!(supervisor.contains(pid));
        supervisor.terminate_all_for_tests();
    }

    #[test]
    fn dead_child_is_reaped_and_removed() {
        let mut supervisor = ChildSupervisor::new();
        let pid = supervisor
            .spawn(
                shell_command("exit 7"),
                ProcessOptions::new(ProcessKind::Application),
            )
            .expect("spawn child");

        let exits = wait_for_reap(&mut supervisor);

        assert_eq!(exits[0].pid, pid);
        assert_eq!(exits[0].status.code(), Some(7));
        assert!(!supervisor.contains(pid));
    }

    #[test]
    fn normal_application_is_not_restarted() {
        let mut supervisor = ChildSupervisor::new();
        supervisor
            .spawn_restartable(
                || Ok(shell_command("exit 1")),
                ProcessOptions::new(ProcessKind::Application),
            )
            .expect("spawn child");

        let exits = wait_for_reap(&mut supervisor);

        assert_eq!(exits.len(), 1);
        assert_eq!(supervisor.active_count(), 0);
    }

    #[test]
    fn critical_child_restart_policy_is_explicit() {
        let options = ProcessOptions::new(ProcessKind::ShellSessionCritical)
            .with_restart_policy(RestartPolicy::CriticalSessionComponent);

        assert_eq!(
            options.restart_policy,
            RestartPolicy::CriticalSessionComponent
        );
    }

    #[test]
    fn critical_child_crash_loop_is_bounded() {
        let mut supervisor = ChildSupervisor::new();
        supervisor.set_restart_guard(RestartGuard::new(2, Duration::from_secs(60)));
        supervisor
            .spawn_restartable(
                || Ok(shell_command("exit 1")),
                ProcessOptions::new(ProcessKind::ShellSessionCritical)
                    .with_restart_policy(RestartPolicy::CriticalSessionComponent),
            )
            .expect("spawn child");

        let deadline = Instant::now() + Duration::from_secs(3);
        while supervisor.restart_suppression_count() == 0 && Instant::now() < deadline {
            let _ = supervisor.reap_exited().expect("reap children");
            thread::sleep(Duration::from_millis(10));
        }

        assert_eq!(supervisor.restart_suppression_count(), 1);
        assert_eq!(supervisor.active_count(), 0);
    }

    #[test]
    fn critical_child_restart_spawn_failure_does_not_fail_supervisor_reap() {
        let mut supervisor = ChildSupervisor::new();
        let attempts = Arc::new(AtomicUsize::new(0));
        supervisor
            .spawn_restartable(
                {
                    let attempts = Arc::clone(&attempts);
                    move || {
                        if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                            Ok(shell_command("exit 1"))
                        } else {
                            Ok(Command::new("/definitely/not/a/typhon-test-binary"))
                        }
                    }
                },
                ProcessOptions::new(ProcessKind::ShellSessionCritical)
                    .with_restart_policy(RestartPolicy::CriticalSessionComponent),
            )
            .expect("spawn child");

        let exits = wait_for_reap(&mut supervisor);

        assert_eq!(exits.len(), 1);
        assert_eq!(exits[0].restarted_pid, None);
        assert_eq!(supervisor.restart_suppression_count(), 1);
        assert_eq!(supervisor.active_count(), 0);
    }

    #[test]
    fn quiesce_suppresses_critical_child_restart_before_shutdown_signals() {
        let mut supervisor = ChildSupervisor::new();
        supervisor
            .spawn_restartable(
                || Ok(shell_command("exit 1")),
                ProcessOptions::new(ProcessKind::ShellSessionCritical)
                    .with_restart_policy(RestartPolicy::CriticalSessionComponent),
            )
            .expect("spawn child");

        supervisor.begin_quiesce();
        let exits = wait_for_reap(&mut supervisor);

        assert_eq!(exits.len(), 1);
        assert_eq!(exits[0].restarted_pid, None);
        assert_eq!(supervisor.active_count(), 0);
    }

    #[test]
    fn session_shutdown_terminates_session_owned_children() {
        let mut supervisor = ChildSupervisor::new();
        let pid = supervisor
            .spawn(
                shell_command("sleep 30"),
                ProcessOptions::new(ProcessKind::SessionService).session_owned(true),
            )
            .expect("spawn child");

        supervisor
            .begin_shutdown(Instant::now())
            .expect("begin shutdown");
        let deadline = Instant::now() + Duration::from_secs(3);
        while supervisor.contains(pid) && Instant::now() < deadline {
            supervisor
                .advance_shutdown(Instant::now())
                .expect("advance shutdown");
            thread::sleep(Duration::from_millis(10));
        }

        assert!(!supervisor.contains(pid));
    }

    #[test]
    fn supervisor_does_not_claim_unrelated_same_uid_processes() {
        let mut unrelated = shell_command("sleep 30").spawn().expect("spawn unrelated");
        let mut supervisor = ChildSupervisor::new();

        let _ = supervisor.reap_exited().expect("reap no children");

        assert_eq!(supervisor.active_count(), 0);
        assert!(unrelated.try_wait().expect("check unrelated").is_none());
        unrelated.kill().expect("kill unrelated");
        unrelated.wait().expect("reap unrelated");
    }

    #[test]
    fn dedicated_process_has_own_group() {
        let mut supervisor = ChildSupervisor::new();
        let spawned = supervisor
            .spawn_with_identity(
                shell_command("sleep 30"),
                ProcessOptions::new(ProcessKind::Xwayland)
                    .session_owned(true)
                    .with_process_group_policy(ProcessGroupPolicy::Dedicated),
            )
            .expect("spawn dedicated child");

        assert_eq!(spawned.pgid, Some(i32::try_from(spawned.pid).unwrap()));
        supervisor.kill_session_owned_now().expect("kill child");
    }

    #[test]
    fn dedicated_process_group_requires_session_ownership() {
        let mut supervisor = ChildSupervisor::new();
        let error = supervisor
            .spawn_with_identity(
                shell_command("true"),
                ProcessOptions::new(ProcessKind::Application)
                    .session_owned(false)
                    .with_process_group_policy(ProcessGroupPolicy::Dedicated),
            )
            .expect_err("non-session child must not get a dedicated group");

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn managed_process_identity_is_not_reused_after_exit() {
        let mut supervisor = ChildSupervisor::new();
        let first = supervisor
            .spawn_with_identity(
                shell_command("exit 0"),
                ProcessOptions::new(ProcessKind::SessionService),
            )
            .expect("spawn first child");
        let _ = wait_for_reap(&mut supervisor);

        assert!(!supervisor.contains_id(first.id));
        let second = supervisor
            .spawn_with_identity(
                shell_command("sleep 30"),
                ProcessOptions::new(ProcessKind::SessionService),
            )
            .expect("spawn second child");

        assert_ne!(first.id, second.id);
        assert!(!supervisor.contains_id(first.id));
        supervisor.kill_session_owned_now().expect("kill child");
    }

    #[test]
    fn mapped_inherited_fd_is_available_only_at_requested_target() {
        let source: OwnedFd = std::fs::File::open("/dev/null")
            .expect("open source fd")
            .into();
        let mut launch = SpawnCommand::new(shell_command(
            "test -e /proc/self/fd/55 && test ! -e /proc/self/fd/56",
        ));
        launch.map_fd(source, 55).expect("map fd");

        let mut supervisor = ChildSupervisor::new();
        let spawned = launch
            .spawn(
                &mut supervisor,
                ProcessOptions::new(ProcessKind::Application).session_owned(false),
            )
            .expect("spawn mapped child");
        let exits = wait_for_reap(&mut supervisor);

        assert_eq!(exits[0].id, spawned.id);
        assert_eq!(exits[0].status.code(), Some(0));
    }

    #[test]
    fn failed_mapped_spawn_closes_owned_sources_and_registers_no_child() {
        let source: OwnedFd = std::fs::File::open("/dev/null")
            .expect("open source fd")
            .into();
        let mut launch = SpawnCommand::new(Command::new("/definitely/not/a/typhon-test-binary"));
        launch.map_fd(source, 55).expect("map fd");

        let mut supervisor = ChildSupervisor::new();
        assert!(
            launch
                .spawn(&mut supervisor, ProcessOptions::new(ProcessKind::Xwayland),)
                .is_err()
        );
        assert_eq!(supervisor.active_count(), 0);
    }

    #[test]
    fn emergency_cleanup_terminates_dedicated_child_and_grandchild() {
        let marker =
            std::env::temp_dir().join(format!("typhon-process-grandchild-{}", std::process::id()));
        let script = format!("sleep 30 & echo $! > {} ; wait", marker.display());
        let mut supervisor = ChildSupervisor::new();
        supervisor
            .spawn_with_identity(
                shell_command(&script),
                ProcessOptions::new(ProcessKind::Xwayland)
                    .with_process_group_policy(ProcessGroupPolicy::Dedicated),
            )
            .expect("spawn process group");
        let deadline = Instant::now() + Duration::from_secs(2);
        while !marker.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let grandchild = std::fs::read_to_string(&marker)
            .expect("read grandchild pid")
            .trim()
            .parse::<libc::pid_t>()
            .expect("parse grandchild pid");

        supervisor
            .kill_session_owned_now()
            .expect("emergency cleanup");
        let deadline = Instant::now() + Duration::from_secs(2);
        while process_exists(grandchild) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let _ = std::fs::remove_file(marker);
        assert!(!process_exists(grandchild));
    }

    #[test]
    fn orderly_session_shutdown_terminates_dedicated_grandchild() {
        let marker = std::env::temp_dir().join(format!(
            "typhon-process-orderly-grandchild-{}",
            std::process::id()
        ));
        let script = format!("sleep 30 & echo $! > {} ; wait", marker.display());
        let mut supervisor = ChildSupervisor::new();
        let spawned = supervisor
            .spawn_with_identity(
                shell_command(&script),
                ProcessOptions::new(ProcessKind::Xwayland)
                    .with_process_group_policy(ProcessGroupPolicy::Dedicated),
            )
            .expect("spawn process group");
        let deadline = Instant::now() + Duration::from_secs(2);
        while !marker.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let grandchild = std::fs::read_to_string(&marker)
            .expect("read grandchild pid")
            .trim()
            .parse::<libc::pid_t>()
            .expect("parse grandchild pid");

        supervisor
            .begin_shutdown(Instant::now())
            .expect("begin shutdown");
        let deadline = Instant::now() + Duration::from_secs(2);
        while supervisor.contains_id(spawned.id) && Instant::now() < deadline {
            supervisor
                .advance_shutdown(Instant::now())
                .expect("advance shutdown");
            thread::sleep(Duration::from_millis(10));
        }
        let _ = std::fs::remove_file(marker);
        assert!(!supervisor.contains_id(spawned.id));
        assert!(!process_exists(grandchild));
    }

    #[test]
    fn emergency_cleanup_leaves_unrelated_application_alive() {
        let mut supervisor = ChildSupervisor::new();
        let application = supervisor
            .spawn(
                shell_command("sleep 30"),
                ProcessOptions::new(ProcessKind::Application).session_owned(false),
            )
            .expect("spawn application");
        supervisor
            .spawn_with_identity(
                shell_command("sleep 30"),
                ProcessOptions::new(ProcessKind::Xwayland)
                    .with_process_group_policy(ProcessGroupPolicy::Dedicated),
            )
            .expect("spawn session child");

        supervisor
            .kill_session_owned_now()
            .expect("cleanup session");
        assert!(supervisor.contains(application));
        assert!(process_exists(libc::pid_t::try_from(application).unwrap()));
        signal_process(application, libc::SIGKILL).expect("kill test application");
        supervisor.terminate_all_for_tests();
    }

    #[test]
    fn bootstrap_guard_cleans_already_started_session_child_on_failure() {
        let mut supervisor = ChildSupervisor::new();
        {
            let mut guard = supervisor.bootstrap_guard();
            guard
                .spawn(
                    shell_command("sleep 30"),
                    ProcessOptions::new(ProcessKind::Xwayland)
                        .with_process_group_policy(ProcessGroupPolicy::Dedicated),
                )
                .expect("spawn bootstrap child");
        }

        let deadline = Instant::now() + Duration::from_secs(2);
        while supervisor.active_count() != 0 && Instant::now() < deadline {
            let _ = supervisor.reap_exited().expect("reap bootstrap child");
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(supervisor.active_count(), 0);
    }

    fn process_exists(pid: libc::pid_t) -> bool {
        let result = unsafe { libc::kill(pid, 0) };
        result == 0 || io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }
}
