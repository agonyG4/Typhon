use std::{
    convert::Infallible,
    env,
    fmt::{self, Display, Formatter},
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    path::Path,
    process::Command,
    sync::{Mutex, OnceLock, mpsc::sync_channel},
    thread,
    time::{Duration, Instant},
};

use crate::{control_snapshots::DoctorSeverity, process::SpawnCommand};
use std::os::unix::process::CommandExt;

pub const INTERNAL_SCOPE_EXEC_COMMAND: &str = "__internal-app-scope-exec";
const STATUS_FD_ENV: &str = "OBLIVION_ONE_APP_SCOPE_STATUS_FD";
const STATUS_FD: RawFd = 63;
const MAX_UNIT_NAME_BYTES: usize = 64;
const MAX_PENDING_STATUS_READERS: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApplicationScopePolicy {
    Auto,
    On,
    Off,
}

impl ApplicationScopePolicy {
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "on" => Self::On,
            "off" => Self::Off,
            "auto" => Self::Auto,
            _ => Self::Auto,
        }
    }

    pub fn from_env() -> Self {
        match env::var("OBLIVION_ONE_APP_SCOPES") {
            Ok(value) => {
                let normalized = value.trim().to_ascii_lowercase();
                if !matches!(normalized.as_str(), "auto" | "on" | "off") {
                    eprintln!(
                        "application scopes: unknown OBLIVION_ONE_APP_SCOPES value={value:?}; using auto"
                    );
                }
                Self::parse(&value)
            }
            Err(env::VarError::NotPresent) => Self::Auto,
            Err(env::VarError::NotUnicode(_)) => {
                eprintln!("application scopes: invalid OBLIVION_ONE_APP_SCOPES value; using auto");
                Self::Auto
            }
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::On => "on",
            Self::Off => "off",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApplicationScopeSupport {
    Unknown,
    Available,
    Unavailable,
}

impl ApplicationScopeSupport {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Available => "available",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApplicationScopeSnapshot {
    pub policy: ApplicationScopePolicy,
    pub support: ApplicationScopeSupport,
    pub attempted_launches: u64,
    pub scoped_launches: u64,
    pub fallback_launches: u64,
    pub pending_launches: usize,
    pub last_failure: Option<String>,
}

impl ApplicationScopeSnapshot {
    pub fn doctor_severity(&self) -> DoctorSeverity {
        if self.policy == ApplicationScopePolicy::Off {
            return DoctorSeverity::Ok;
        }
        if self.fallback_launches > 0 || self.support == ApplicationScopeSupport::Unavailable {
            return DoctorSeverity::Warning;
        }
        DoctorSeverity::Ok
    }

    pub fn detail(&self) -> String {
        format!(
            "policy={} support={} attempts={} scoped={} fallback={} pending={} last_failure={}",
            self.policy.as_str(),
            self.support.as_str(),
            self.attempted_launches,
            self.scoped_launches,
            self.fallback_launches,
            self.pending_launches,
            self.last_failure.as_deref().unwrap_or("none"),
        )
    }
}

#[derive(Debug)]
pub enum ScopeError {
    EmptyTarget,
    InvalidExecutable,
    InvalidInvocation(&'static str),
    Registration(String),
    Timeout(&'static str),
    Migration(String),
    Exec(io::Error),
}

impl Display for ScopeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyTarget => formatter.write_str("internal application scope target is empty"),
            Self::InvalidExecutable => formatter.write_str("current Typhon executable is invalid"),
            Self::InvalidInvocation(reason) => {
                write!(formatter, "invalid scope helper invocation: {reason}")
            }
            Self::Registration(reason) => write!(formatter, "scope registration failed: {reason}"),
            Self::Timeout(operation) => write!(formatter, "scope {operation} timed out"),
            Self::Migration(reason) => write!(formatter, "scope migration not proven: {reason}"),
            Self::Exec(error) => write!(formatter, "application exec failed: {error}"),
        }
    }
}

impl std::error::Error for ScopeError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ScopeSetupTimeouts {
    registration: Duration,
    migration: Duration,
    poll: Duration,
}

impl ScopeSetupTimeouts {
    const fn production() -> Self {
        Self {
            registration: Duration::from_millis(500),
            migration: Duration::from_millis(500),
            poll: Duration::from_millis(10),
        }
    }

    #[cfg(test)]
    const fn test() -> Self {
        Self {
            registration: Duration::from_millis(10),
            migration: Duration::from_millis(10),
            poll: Duration::ZERO,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ScopeSetupOutcome {
    Scoped,
    Fallback { reason: String },
}

trait ScopeRegistrar: Send + 'static {
    fn register(&self, unit_name: &str, pid: u32) -> Result<(), String>;
}

fn establish_scope<R, F>(
    registrar: R,
    unit_name: &str,
    pid: u32,
    timeouts: ScopeSetupTimeouts,
    mut migration_verified: F,
) -> ScopeSetupOutcome
where
    R: ScopeRegistrar,
    F: FnMut(&str) -> bool,
{
    let unit_name = unit_name.to_owned();
    let registration_unit_name = unit_name.clone();
    let (sender, receiver) = sync_channel(1);
    thread::spawn(move || {
        let _ = sender.send(registrar.register(&registration_unit_name, pid));
    });

    match receiver.recv_timeout(timeouts.registration) {
        Ok(Ok(())) => {}
        Ok(Err(reason)) => return ScopeSetupOutcome::Fallback { reason },
        Err(_) => {
            return ScopeSetupOutcome::Fallback {
                reason: "user systemd registration timeout".to_owned(),
            };
        }
    }

    let deadline = Instant::now() + timeouts.migration;
    loop {
        if migration_verified(unit_name.as_str()) {
            return ScopeSetupOutcome::Scoped;
        }
        if Instant::now() >= deadline {
            return ScopeSetupOutcome::Fallback {
                reason: "migration was not observed before timeout".to_owned(),
            };
        }
        thread::sleep(timeouts.poll);
    }
}

fn scope_unit_name(pid: u32, counter: u64) -> String {
    let name = format!("app-typhon-{pid}-{counter}.scope");
    if name.len() <= MAX_UNIT_NAME_BYTES {
        return name;
    }
    format!("app-typhon-{pid:x}-{counter:x}.scope")
}

fn wrap_application_argv(
    real_argv: &[String],
    executable: &Path,
) -> Result<Vec<String>, ScopeError> {
    if real_argv.is_empty() {
        return Err(ScopeError::EmptyTarget);
    }
    if executable.as_os_str().is_empty() {
        return Err(ScopeError::InvalidExecutable);
    }
    let mut wrapped = Vec::with_capacity(real_argv.len() + 3);
    wrapped.push(executable.to_string_lossy().into_owned());
    wrapped.push(INTERNAL_SCOPE_EXEC_COMMAND.to_owned());
    wrapped.push("--".to_owned());
    wrapped.extend(real_argv.iter().cloned());
    Ok(wrapped)
}

pub fn maybe_wrap_application_argv(
    real_argv: &[String],
    eligible: bool,
) -> io::Result<Vec<String>> {
    let policy = ApplicationScopePolicy::from_env();
    if !eligible || policy == ApplicationScopePolicy::Off {
        return Ok(real_argv.to_vec());
    }
    let executable = env::current_exe()?;
    wrap_application_argv(real_argv, &executable)
        .map_err(|error| io::Error::other(error.to_string()))
}

pub fn prepare_application_spawn(mut command: Command, eligible: bool) -> io::Result<SpawnCommand> {
    let policy = ApplicationScopePolicy::from_env();
    if !eligible || policy == ApplicationScopePolicy::Off {
        return Ok(SpawnCommand::new(command));
    }

    let (reader, writer) = status_pipe()?;
    command.env(STATUS_FD_ENV, STATUS_FD.to_string());
    let mut spawn = SpawnCommand::new(command);
    spawn.map_fd(writer, STATUS_FD)?;
    let mut state = diagnostics()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.snapshot.attempted_launches = state.snapshot.attempted_launches.saturating_add(1);
    if state.readers.len() < MAX_PENDING_STATUS_READERS {
        state.readers.push(reader);
    }
    Ok(spawn)
}

pub fn snapshot() -> ApplicationScopeSnapshot {
    let mut state = diagnostics()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    drain_status_readers(&mut state);
    state.snapshot.policy = ApplicationScopePolicy::from_env();
    state.snapshot.pending_launches = state.readers.len();
    state.snapshot.clone()
}

pub fn run_internal_scope_exec(args: &[String]) -> Result<Infallible, ScopeError> {
    let Some(delimiter) = args.iter().position(|arg| arg == "--") else {
        return Err(ScopeError::InvalidInvocation("missing -- delimiter"));
    };
    let target = &args[delimiter + 1..];
    if target.is_empty() {
        return Err(ScopeError::EmptyTarget);
    }

    if ApplicationScopePolicy::from_env() != ApplicationScopePolicy::Off {
        let pid = std::process::id();
        let unit_name = scope_unit_name(pid, next_scope_counter());
        let outcome = establish_scope(
            UserSystemdRegistrar,
            &unit_name,
            pid,
            ScopeSetupTimeouts::production(),
            current_process_is_in_scope,
        );
        match outcome {
            ScopeSetupOutcome::Scoped => report_helper_status("S"),
            ScopeSetupOutcome::Fallback { reason } => {
                report_helper_status(&format!("F:{reason}"));
            }
        }
    }

    unsafe {
        env::remove_var(STATUS_FD_ENV);
    }
    let mut command = build_exec_command(target)?;
    Err(ScopeError::Exec(command.exec()))
}

fn build_exec_command(target: &[String]) -> Result<Command, ScopeError> {
    let Some(program) = target.first() else {
        return Err(ScopeError::EmptyTarget);
    };
    let mut command = Command::new(program);
    command.args(&target[1..]);
    Ok(command)
}

fn current_process_is_in_scope(unit_name: &str) -> bool {
    let Ok(contents) = std::fs::read_to_string("/proc/self/cgroup") else {
        return false;
    };
    let Some(path) = contents.lines().find_map(|line| {
        let (hierarchy, rest) = line.split_once(':')?;
        let (controllers, path) = rest.split_once(':')?;
        (hierarchy == "0" && controllers.is_empty()).then_some(path)
    }) else {
        return false;
    };
    path == format!("/app.slice/{unit_name}") || path.ends_with(&format!("/app.slice/{unit_name}"))
}

struct UserSystemdRegistrar;

impl ScopeRegistrar for UserSystemdRegistrar {
    fn register(&self, unit_name: &str, pid: u32) -> Result<(), String> {
        futures_lite::future::block_on(register_user_scope(unit_name, pid))
            .map_err(|error| error.to_string())
    }
}

async fn register_user_scope(unit_name: &str, pid: u32) -> zbus::Result<()> {
    use zbus::{Proxy, zvariant::Value};

    let connection = zbus::connection::Builder::session()?
        .method_timeout(Duration::from_millis(400))
        .build()
        .await?;
    let proxy = Proxy::new(
        &connection,
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
    )
    .await?;
    let properties = vec![
        ("PIDs", Value::from(vec![pid])),
        ("Slice", Value::from("app.slice")),
        ("Description", Value::from("Typhon application scope")),
    ];
    type AuxiliaryUnit<'a> = (&'a str, Vec<(&'a str, Value<'a>)>);
    let auxiliary: Vec<AuxiliaryUnit<'_>> = Vec::new();
    proxy
        .call_method(
            "StartTransientUnit",
            &(unit_name, "fail", properties, auxiliary),
        )
        .await
        .map(|_| ())
}

fn report_helper_status(status: &str) {
    let Ok(fd) = env::var(STATUS_FD_ENV)
        .ok()
        .and_then(|value| value.parse::<RawFd>().ok())
        .ok_or(())
    else {
        return;
    };
    let status = bounded_string(status.to_owned(), 192);
    let bytes = status.as_bytes();
    let mut written = 0;
    while written < bytes.len() {
        let result =
            unsafe { libc::write(fd, bytes[written..].as_ptr().cast(), bytes.len() - written) };
        if result <= 0 {
            break;
        }
        written += result as usize;
    }
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags >= 0 {
        let _ = unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) };
    }
}

fn status_pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [-1; 2];
    let result = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: pipe2 initialized both descriptors and ownership is transferred exactly once.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

struct Diagnostics {
    snapshot: ApplicationScopeSnapshot,
    readers: Vec<OwnedFd>,
}

fn diagnostics() -> &'static Mutex<Diagnostics> {
    static DIAGNOSTICS: OnceLock<Mutex<Diagnostics>> = OnceLock::new();
    DIAGNOSTICS.get_or_init(|| {
        Mutex::new(Diagnostics {
            snapshot: ApplicationScopeSnapshot {
                policy: ApplicationScopePolicy::from_env(),
                support: ApplicationScopeSupport::Unknown,
                attempted_launches: 0,
                scoped_launches: 0,
                fallback_launches: 0,
                pending_launches: 0,
                last_failure: None,
            },
            readers: Vec::new(),
        })
    })
}

fn drain_status_readers(state: &mut Diagnostics) {
    let mut index = 0;
    while index < state.readers.len() {
        let fd = state.readers[index].as_raw_fd();
        let mut buffer = [0_u8; 256];
        let result = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
        if result == 0 {
            state.readers.swap_remove(index);
            continue;
        }
        if result < 0 {
            let error = io::Error::last_os_error();
            if matches!(
                error.raw_os_error(),
                Some(code) if code == libc::EAGAIN || code == libc::EWOULDBLOCK
            ) {
                index += 1;
                continue;
            }
            state.readers.swap_remove(index);
            continue;
        }
        let status = String::from_utf8_lossy(&buffer[..result as usize]);
        if status.starts_with('S') {
            state.snapshot.scoped_launches = state.snapshot.scoped_launches.saturating_add(1);
            state.snapshot.support = ApplicationScopeSupport::Available;
            state.readers.swap_remove(index);
        } else if let Some(reason) = status.strip_prefix("F:") {
            state.snapshot.fallback_launches = state.snapshot.fallback_launches.saturating_add(1);
            state.snapshot.support = ApplicationScopeSupport::Unavailable;
            state.snapshot.last_failure = Some(bounded_string(reason.trim().to_owned(), 192));
            state.readers.swap_remove(index);
        } else {
            index += 1;
        }
    }
}

fn bounded_string(value: String, limit: usize) -> String {
    if value.len() <= limit {
        return value;
    }
    let mut value = value;
    let mut end = limit.saturating_sub(3);
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    value.push_str("...");
    value
}

fn next_scope_counter() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        path::Path,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };

    struct SuccessfulRegistrar;

    impl ScopeRegistrar for SuccessfulRegistrar {
        fn register(&self, _unit_name: &str, _pid: u32) -> Result<(), String> {
            Ok(())
        }
    }

    struct FailingRegistrar;

    impl ScopeRegistrar for FailingRegistrar {
        fn register(&self, _unit_name: &str, _pid: u32) -> Result<(), String> {
            Err("user manager unavailable".to_owned())
        }
    }

    struct BlockingRegistrar;

    impl ScopeRegistrar for BlockingRegistrar {
        fn register(&self, _unit_name: &str, _pid: u32) -> Result<(), String> {
            std::thread::sleep(Duration::from_millis(50));
            Ok(())
        }
    }

    #[test]
    fn policy_parser_defaults_and_normalizes_unknown_values_to_auto() {
        assert_eq!(
            ApplicationScopePolicy::parse("auto"),
            ApplicationScopePolicy::Auto
        );
        assert_eq!(
            ApplicationScopePolicy::parse(" ON "),
            ApplicationScopePolicy::On
        );
        assert_eq!(
            ApplicationScopePolicy::parse("off"),
            ApplicationScopePolicy::Off
        );
        assert_eq!(
            ApplicationScopePolicy::parse("unexpected"),
            ApplicationScopePolicy::Auto
        );
    }

    #[test]
    fn wrapping_preserves_structural_arguments_and_rejects_empty_targets() {
        let executable = Path::new("/usr/bin/typhon");
        let wrapped = wrap_application_argv(
            &[
                "app".to_owned(),
                "name with spaces".to_owned(),
                "$(not shell)".to_owned(),
            ],
            executable,
        )
        .expect("wrapped argv");

        assert_eq!(wrapped[0], executable.to_string_lossy());
        assert_eq!(wrapped[1], INTERNAL_SCOPE_EXEC_COMMAND);
        assert_eq!(wrapped[2], "--");
        assert_eq!(wrapped[3..], ["app", "name with spaces", "$(not shell)"]);
        assert!(matches!(
            wrap_application_argv(&[], executable),
            Err(ScopeError::EmptyTarget)
        ));
    }

    #[test]
    fn generated_scope_names_are_valid_and_bounded() {
        let name = scope_unit_name(1234, 7);
        assert!(name.ends_with(".scope"));
        assert!(name.len() <= MAX_UNIT_NAME_BYTES);
        assert!(
            name.chars()
                .all(|character| character.is_ascii_alphanumeric()
                    || matches!(character, '-' | '_' | '.'))
        );

        assert!(scope_unit_name(u32::MAX, u64::MAX).len() <= MAX_UNIT_NAME_BYTES);
    }

    #[test]
    fn diagnostics_warn_after_fallback_but_not_when_explicitly_disabled() {
        let fallback = ApplicationScopeSnapshot {
            policy: ApplicationScopePolicy::Auto,
            support: ApplicationScopeSupport::Unavailable,
            attempted_launches: 1,
            scoped_launches: 0,
            fallback_launches: 1,
            pending_launches: 0,
            last_failure: Some("timeout".to_owned()),
        };
        assert_eq!(fallback.doctor_severity(), DoctorSeverity::Warning);

        let disabled = ApplicationScopeSnapshot {
            policy: ApplicationScopePolicy::Off,
            ..fallback
        };
        assert_eq!(disabled.doctor_severity(), DoctorSeverity::Ok);
    }

    #[test]
    fn successful_registration_and_migration_select_scoped_execution() {
        let observed = Arc::new(AtomicBool::new(true));
        let observer = Arc::clone(&observed);
        let outcome = establish_scope(
            SuccessfulRegistrar,
            "app-typhon-1-1.scope",
            1,
            ScopeSetupTimeouts::test(),
            move |_| observer.load(Ordering::SeqCst),
        );
        assert_eq!(outcome, ScopeSetupOutcome::Scoped);
    }

    #[test]
    fn registration_failure_selects_direct_fallback() {
        let outcome = establish_scope(
            FailingRegistrar,
            "app-typhon-1-2.scope",
            1,
            ScopeSetupTimeouts::test(),
            |_| true,
        );
        assert!(matches!(outcome, ScopeSetupOutcome::Fallback { .. }));
    }

    #[test]
    fn registration_timeout_selects_direct_fallback() {
        let outcome = establish_scope(
            BlockingRegistrar,
            "app-typhon-1-3.scope",
            1,
            ScopeSetupTimeouts {
                registration: Duration::from_millis(1),
                migration: Duration::from_millis(1),
                poll: Duration::ZERO,
            },
            |_| true,
        );
        assert!(matches!(outcome, ScopeSetupOutcome::Fallback { .. }));
    }

    #[test]
    fn migration_not_observed_selects_direct_fallback() {
        let outcome = establish_scope(
            SuccessfulRegistrar,
            "app-typhon-1-4.scope",
            1,
            ScopeSetupTimeouts {
                registration: Duration::from_millis(10),
                migration: Duration::from_millis(1),
                poll: Duration::ZERO,
            },
            |_| false,
        );
        assert!(matches!(outcome, ScopeSetupOutcome::Fallback { .. }));
    }

    #[test]
    fn final_exec_command_keeps_target_argv_separate() {
        let command = build_exec_command(&["app".to_owned(), "arg with spaces".to_owned()])
            .expect("exec command");
        assert_eq!(command.get_program(), "app");
        assert_eq!(command.get_args().collect::<Vec<_>>(), ["arg with spaces"]);
    }
}
