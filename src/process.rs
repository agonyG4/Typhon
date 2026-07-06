use std::{
    collections::{HashMap, VecDeque},
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    process::{Child, Command, ExitStatus},
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProcessKind {
    ShellSessionCritical,
    Application,
    SessionService,
    Infrastructure,
}

impl ProcessKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ShellSessionCritical => "shell_session_critical",
            Self::Application => "application",
            Self::SessionService => "session_service",
            Self::Infrastructure => "infrastructure",
        }
    }
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
    pub restart_policy: RestartPolicy,
    pub label: Option<String>,
}

impl ProcessOptions {
    pub fn new(kind: ProcessKind) -> Self {
        Self {
            kind,
            session_owned: !matches!(kind, ProcessKind::Application),
            restart_policy: RestartPolicy::Never,
            label: None,
        }
    }

    pub fn session_owned(mut self, session_owned: bool) -> Self {
        self.session_owned = session_owned;
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
pub struct ChildExit {
    pub pid: u32,
    pub kind: ProcessKind,
    pub status: ExitStatus,
    pub restarted_pid: Option<u32>,
}

type RestartFactory = Arc<dyn Fn() -> io::Result<Command> + Send + Sync + 'static>;

struct ManagedChild {
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
    children: HashMap<u32, ManagedChild>,
    restart_guard: RestartGuard,
    restart_suppression_count: u64,
    shutdown: Option<SupervisorShutdown>,
    sigchld_fd: Option<OwnedFd>,
}

impl ChildSupervisor {
    const TERM_GRACE: Duration = Duration::from_millis(750);

    pub fn new() -> Self {
        Self {
            children: HashMap::new(),
            restart_guard: RestartGuard::default(),
            restart_suppression_count: 0,
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
        self.spawn_inner(&mut command, options, None, VecDeque::new())
    }

    pub fn spawn_restartable<F>(&mut self, factory: F, options: ProcessOptions) -> io::Result<u32>
    where
        F: Fn() -> io::Result<Command> + Send + Sync + 'static,
    {
        let factory: RestartFactory = Arc::new(factory);
        let mut command = factory()?;
        self.spawn_inner(&mut command, options, Some(factory), VecDeque::new())
    }

    fn spawn_inner(
        &mut self,
        command: &mut Command,
        options: ProcessOptions,
        restart_factory: Option<RestartFactory>,
        restart_history: VecDeque<Instant>,
    ) -> io::Result<u32> {
        let child = command.spawn()?;
        let pid = child.id();
        eprintln!(
            "oblivion-one process: spawn kind={} pid={} restart_policy={} session_owned={} label={}",
            options.kind.as_str(),
            pid,
            options.restart_policy.as_str(),
            options.session_owned,
            options.label.as_deref().unwrap_or("none")
        );
        self.children.insert(
            pid,
            ManagedChild {
                child,
                options,
                launched_at: Instant::now(),
                restart_factory,
                restart_history,
            },
        );
        Ok(pid)
    }

    pub fn contains(&self, pid: u32) -> bool {
        self.children.contains_key(&pid)
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
        let pids = self.children.keys().copied().collect::<Vec<_>>();
        for pid in pids {
            let status = {
                let Some(child) = self.children.get_mut(&pid) else {
                    continue;
                };
                child.child.try_wait()?
            };
            let Some(status) = status else {
                continue;
            };
            let Some(mut child) = self.children.remove(&pid) else {
                continue;
            };
            let restarted_pid = self.maybe_restart_child(&mut child, status)?;
            eprintln!(
                "oblivion-one process: exit kind={} pid={} status={} runtime_ms={} restarted_pid={}",
                child.options.kind.as_str(),
                pid,
                exit_status_label(status),
                child.launched_at.elapsed().as_millis(),
                restarted_pid
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| "none".to_string())
            );
            exited.push(ChildExit {
                pid,
                kind: child.options.kind,
                status,
                restarted_pid,
            });
        }
        Ok(exited)
    }

    fn maybe_restart_child(
        &mut self,
        child: &mut ManagedChild,
        status: ExitStatus,
    ) -> io::Result<Option<u32>> {
        if self.shutdown.is_some() {
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
        let pid = self.spawn_inner(
            &mut command,
            child.options.clone(),
            Some(factory),
            child.restart_history.clone(),
        )?;
        Ok(Some(pid))
    }

    pub fn begin_shutdown(&mut self, now: Instant) -> io::Result<()> {
        if self.shutdown.is_some() {
            return Ok(());
        }
        self.shutdown = Some(SupervisorShutdown {
            started_at: now,
            sigkill_sent: false,
        });
        for (pid, child) in &self.children {
            if child.options.session_owned {
                eprintln!(
                    "oblivion-one process: shutdown signal=SIGTERM kind={} pid={}",
                    child.options.kind.as_str(),
                    pid
                );
                signal_process(*pid, libc::SIGTERM)?;
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
            .map(|(pid, _)| *pid)
            .collect::<Vec<_>>();
        if session_owned.is_empty() {
            return Ok(true);
        }
        if !shutdown.sigkill_sent && now.duration_since(shutdown.started_at) >= Self::TERM_GRACE {
            shutdown.sigkill_sent = true;
            for pid in session_owned {
                if let Some(child) = self.children.get(&pid) {
                    eprintln!(
                        "oblivion-one process: shutdown signal=SIGKILL kind={} pid={}",
                        child.options.kind.as_str(),
                        pid
                    );
                }
                signal_process(pid, libc::SIGKILL)?;
            }
        }
        let _ = self.reap_exited()?;
        Ok(!self
            .children
            .values()
            .any(|child| child.options.session_owned))
    }

    #[cfg(test)]
    fn terminate_all_for_tests(&mut self) {
        for child in self.children.values_mut() {
            let _ = child.child.kill();
        }
        for (_, mut child) in self.children.drain() {
            let _ = child.child.wait();
        }
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
}
