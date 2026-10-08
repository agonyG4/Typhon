use oblivion_one::process::{
    ChildExit, ChildSupervisor, ProcessGroupPolicy, ProcessKind, ProcessOptions, RestartPolicy,
    SpawnCommand, block_runtime_signals_for_current_thread, block_sigchld_for_current_thread,
    prepare_child_signal_mask, prepare_runtime_signal_handling,
    sigchld_is_blocked_for_current_thread,
};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::fd::OwnedFd,
    os::unix::process::CommandExt,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const PROBE_MODE: &str = "TYPHON_CHILD_SIGNAL_PROBE";
const PROBE_EXIT_CODE: &str = "TYPHON_CHILD_SIGNAL_PROBE_EXIT_CODE";
const PROBE_ASSERT_UNRELATED: &str = "TYPHON_CHILD_SIGNAL_ASSERT_UNRELATED";
const TERMINATION_FIXTURE_MODE: &str = "TYPHON_RUNTIME_SIGNAL_FIXTURE";
const TERMINATION_FIXTURE_READY: &str = "TYPHON_RUNTIME_SIGNAL_READY";
const TERMINATION_FIXTURE_EVENTS: &str = "TYPHON_RUNTIME_SIGNAL_EVENTS";
const TERMINATION_FIXTURE_COUNT: &str = "TYPHON_RUNTIME_SIGNAL_COUNT";

fn probe_command(exit_code: i32, descendant: bool) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .arg("--exact")
        .arg("child_signal_mask_probe")
        .arg("--nocapture")
        .env(PROBE_MODE, if descendant { "descendant" } else { "direct" })
        .env(PROBE_EXIT_CODE, exit_code.to_string());
    command
}

fn child_signal_mask_probe_mode() -> Option<String> {
    std::env::var(PROBE_MODE).ok()
}

fn reap_pid(supervisor: &mut ChildSupervisor, pid: u32) -> ChildExit {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        let exits = supervisor.reap_exited().unwrap();
        if let Some(exit) = exits.into_iter().find(|exit| exit.pid == pid) {
            return exit;
        }
        thread::yield_now();
    }
    panic!("timed out waiting for supervised child {pid}");
}

fn assert_probe_exit(supervisor: &mut ChildSupervisor, pid: u32) {
    assert_eq!(reap_pid(supervisor, pid).status.code(), Some(0));
}

#[test]
fn child_signal_mask_probe() {
    let Some(mode) = child_signal_mask_probe_mode() else {
        return;
    };
    assert!(mode == "direct" || mode == "descendant");
    assert!(!sigchld_is_blocked_for_current_thread().unwrap());
    for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP, libc::SIGQUIT] {
        assert!(!signal_is_blocked(signal));
    }
    if std::env::var_os(PROBE_ASSERT_UNRELATED).is_some() {
        assert!(signal_is_blocked(libc::SIGUSR1));
    }
    if mode == "descendant" {
        let mut descendant = Command::new("sh").arg("-c").arg("exit 0").spawn().unwrap();
        assert!(descendant.wait().unwrap().success());
    }
    let exit_code = std::env::var(PROBE_EXIT_CODE)
        .unwrap()
        .parse::<i32>()
        .unwrap();
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}

#[test]
fn managed_termination_signal_fixture() {
    let Some(mode) = std::env::var_os(TERMINATION_FIXTURE_MODE) else {
        return;
    };
    assert_eq!(mode, "signalfd");
    let ready_path = std::env::var_os(TERMINATION_FIXTURE_READY).unwrap();
    let events_path = std::env::var_os(TERMINATION_FIXTURE_EVENTS).unwrap();
    let expected_count = std::env::var(TERMINATION_FIXTURE_COUNT)
        .unwrap()
        .parse::<usize>()
        .unwrap();
    prepare_runtime_signal_handling().unwrap();
    let mut supervisor = ChildSupervisor::with_sigchld_reaper().unwrap();
    let signal_fd = supervisor.signal_fd().unwrap();
    fs::write(ready_path, b"ready").unwrap();

    let mut handled = 0;
    while handled < expected_count {
        let mut ready = libc::pollfd {
            fd: signal_fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let result = unsafe {
            // SAFETY: `ready` is a valid one-element poll array and the
            // descriptor is owned by `supervisor` for the duration of poll.
            libc::poll(&mut ready, 1, 5_000)
        };
        assert!(result > 0, "timed out waiting for a managed signal");
        supervisor.reap_exited().unwrap();
        let signal = supervisor.take_termination_signal().unwrap();
        let mut events = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&events_path)
            .unwrap();
        writeln!(events, "{}", signal.as_str()).unwrap();
        handled += 1;
    }
}

fn signal_fixture_command(
    ready_path: &std::path::Path,
    events_path: &std::path::Path,
    count: usize,
) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .arg("--exact")
        .arg("managed_termination_signal_fixture")
        .arg("--nocapture")
        .arg("--test-threads=1")
        .env(TERMINATION_FIXTURE_MODE, "signalfd")
        .env(TERMINATION_FIXTURE_READY, ready_path)
        .env(TERMINATION_FIXTURE_EVENTS, events_path)
        .env(TERMINATION_FIXTURE_COUNT, count.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0);
    unsafe {
        command.pre_exec(prepare_managed_signals_fixture_before_exec);
    }
    command
}

fn block_managed_signals_before_exec() -> std::io::Result<()> {
    let mut mask = unsafe { std::mem::zeroed::<libc::sigset_t>() };
    if unsafe { libc::sigemptyset(&mut mask) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    for signal in [
        libc::SIGCHLD,
        libc::SIGTERM,
        libc::SIGINT,
        libc::SIGHUP,
        libc::SIGQUIT,
    ] {
        if unsafe { libc::sigaddset(&mut mask, signal) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    let result = unsafe { libc::sigprocmask(libc::SIG_BLOCK, &mask, std::ptr::null_mut()) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn prepare_managed_signals_fixture_before_exec() -> std::io::Result<()> {
    block_managed_signals_before_exec()?;
    let mut action = unsafe { std::mem::zeroed::<libc::sigaction>() };
    if unsafe { libc::sigemptyset(&mut action.sa_mask) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    action.sa_sigaction = libc::SIG_IGN;
    action.sa_flags = 0;
    for signal in [
        libc::SIGCHLD,
        libc::SIGTERM,
        libc::SIGINT,
        libc::SIGHUP,
        libc::SIGQUIT,
    ] {
        if unsafe { libc::sigaction(signal, &action, std::ptr::null_mut()) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

fn wait_for_signal_fixture(ready_path: &std::path::Path, child: &mut std::process::Child) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if ready_path.exists() {
            return;
        }
        assert!(
            child.try_wait().unwrap().is_none(),
            "signal fixture exited before ready"
        );
        thread::sleep(Duration::from_millis(5));
    }
    panic!("signal fixture did not become ready");
}

fn wait_for_signal_events(
    events_path: &std::path::Path,
    expected_count: usize,
    child: &mut std::process::Child,
) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let events = fs::read_to_string(events_path)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if events.len() >= expected_count {
            return events;
        }
        assert!(
            child.try_wait().unwrap().is_none(),
            "signal fixture exited before handling signals"
        );
        thread::sleep(Duration::from_millis(5));
    }
    panic!("signal fixture did not handle {expected_count} signals");
}

fn signal_test_paths(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let root = std::path::Path::new("/mnt/Aether/Desktop/GitHub/Typhon-target")
        .join("signal-test-data")
        .join(format!(
            "typhon-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
    fs::create_dir_all(&root).unwrap();
    (root.join("ready"), root.join("events"))
}

#[test]
fn compositor_signal_delivery_is_graceful_for_direct_and_process_group_targets() {
    for (name, signal_number, signal_name, group_delivery) in [
        ("hup", libc::SIGHUP, "SIGHUP", false),
        ("int", libc::SIGINT, "SIGINT", false),
        ("quit", libc::SIGQUIT, "SIGQUIT", false),
        ("term", libc::SIGTERM, "SIGTERM", false),
        ("group-term", libc::SIGTERM, "SIGTERM", true),
    ] {
        let (ready_path, events_path) = signal_test_paths(name);
        let mut child = signal_fixture_command(&ready_path, &events_path, 1)
            .spawn()
            .unwrap();
        wait_for_signal_fixture(&ready_path, &mut child);
        let pid = i32::try_from(child.id()).unwrap();
        let target = if group_delivery { -pid } else { pid };
        assert_eq!(unsafe { libc::kill(target, signal_number) }, 0);
        assert_eq!(
            wait_for_signal_events(&events_path, 1, &mut child),
            [signal_name]
        );
        assert!(child.wait().unwrap().success());
        fs::remove_dir_all(ready_path.parent().unwrap()).unwrap();
    }
}

#[test]
fn compositor_repeated_signals_are_received_without_forcing_process_exit() {
    let (ready_path, events_path) = signal_test_paths("repeated-signal");
    let mut child = signal_fixture_command(&ready_path, &events_path, 2)
        .spawn()
        .unwrap();
    wait_for_signal_fixture(&ready_path, &mut child);
    assert_eq!(
        unsafe { libc::kill(i32::try_from(child.id()).unwrap(), libc::SIGTERM) },
        0
    );
    assert_eq!(
        wait_for_signal_events(&events_path, 1, &mut child),
        ["SIGTERM"]
    );
    assert_eq!(
        unsafe { libc::kill(i32::try_from(child.id()).unwrap(), libc::SIGTERM) },
        0
    );
    assert_eq!(
        wait_for_signal_events(&events_path, 2, &mut child),
        ["SIGTERM", "SIGTERM"]
    );
    assert!(child.wait().unwrap().success());
    fs::remove_dir_all(ready_path.parent().unwrap()).unwrap();
}

fn signal_is_blocked(signal: libc::c_int) -> bool {
    let mut current = unsafe {
        // SAFETY: zeroed storage is valid for `pthread_sigmask` to initialize.
        std::mem::zeroed::<libc::sigset_t>()
    };
    let result = unsafe {
        // SAFETY: a null new-mask pointer queries the current thread's mask
        // into the valid `current` storage.
        libc::pthread_sigmask(libc::SIG_BLOCK, std::ptr::null(), &mut current)
    };
    assert_eq!(result, 0);
    unsafe {
        // SAFETY: `current` was initialized by `pthread_sigmask` above.
        libc::sigismember(&current, signal) == 1
    }
}

fn block_signal_for_test(signal: libc::c_int) {
    let mut set = unsafe {
        // SAFETY: zeroed storage is valid for `sigemptyset` to initialize.
        std::mem::zeroed::<libc::sigset_t>()
    };
    let empty_result = unsafe {
        // SAFETY: `set` is valid writable storage for `sigemptyset` to initialize.
        libc::sigemptyset(&mut set)
    };
    assert_eq!(empty_result, 0);
    let add_result = unsafe {
        // SAFETY: `set` was initialized by `sigemptyset` and `signal` is the
        // caller-provided signal number used only for this test.
        libc::sigaddset(&mut set, signal)
    };
    assert_eq!(add_result, 0);
    assert_eq!(
        unsafe {
            // SAFETY: `set` is initialized and the null old-mask pointer is
            // valid because the test does not need the previous mask.
            libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut())
        },
        0
    );
}

#[test]
fn supervisor_spawn_unblocks_managed_signals_only_in_the_child() {
    block_sigchld_for_current_thread().unwrap();
    assert!(sigchld_is_blocked_for_current_thread().unwrap());
    let mut supervisor = ChildSupervisor::with_sigchld_reaper().unwrap();
    let pid = supervisor
        .spawn(
            probe_command(0, false),
            ProcessOptions::new(ProcessKind::Application).session_owned(false),
        )
        .unwrap();
    assert!(sigchld_is_blocked_for_current_thread().unwrap());
    assert_eq!(reap_pid(&mut supervisor, pid).status.code(), Some(0));
}

#[test]
fn direct_command_signal_reset_unblocks_typhon_signals_only_in_the_child() {
    block_runtime_signals_for_current_thread().unwrap();
    block_signal_for_test(libc::SIGUSR1);
    let mut command = probe_command(0, false);
    command.env(PROBE_ASSERT_UNRELATED, "1");
    prepare_child_signal_mask(&mut command);
    let mut child = command.spawn().unwrap();

    assert!(sigchld_is_blocked_for_current_thread().unwrap());
    assert!(child.wait().unwrap().success());
}

#[test]
fn child_reset_preserves_unrelated_parent_signal_policy() {
    block_sigchld_for_current_thread().unwrap();
    block_signal_for_test(libc::SIGUSR1);
    let mut supervisor = ChildSupervisor::with_sigchld_reaper().unwrap();
    let mut command = probe_command(0, false);
    command.env(PROBE_ASSERT_UNRELATED, "1");
    let pid = supervisor
        .spawn(
            command,
            ProcessOptions::new(ProcessKind::Application).session_owned(false),
        )
        .unwrap();
    assert_probe_exit(&mut supervisor, pid);
}

#[test]
fn all_supervisor_spawn_variants_start_children_with_sigchld_unblocked() {
    block_sigchld_for_current_thread().unwrap();
    let mut supervisor = ChildSupervisor::with_sigchld_reaper().unwrap();

    let pid = supervisor
        .spawn(
            probe_command(0, false),
            ProcessOptions::new(ProcessKind::Application).session_owned(false),
        )
        .unwrap();
    assert_probe_exit(&mut supervisor, pid);

    let spawned = supervisor
        .spawn_with_identity(
            probe_command(0, false),
            ProcessOptions::new(ProcessKind::Application).session_owned(false),
        )
        .unwrap();
    assert_probe_exit(&mut supervisor, spawned.pid);

    let spawned_with_stderr = supervisor
        .spawn_with_identity_and_stderr(
            probe_command(0, false),
            ProcessOptions::new(ProcessKind::Application).session_owned(false),
        )
        .unwrap();
    drop(spawned_with_stderr.stderr);
    assert_probe_exit(&mut supervisor, spawned_with_stderr.process.pid);

    let first = Arc::new(AtomicUsize::new(0));
    let restart_counter = Arc::clone(&first);
    let restarted_pid = supervisor
        .spawn_restartable(
            move || {
                let exit_code = if restart_counter.fetch_add(1, Ordering::SeqCst) == 0 {
                    1
                } else {
                    0
                };
                Ok(probe_command(exit_code, false))
            },
            ProcessOptions::new(ProcessKind::SessionService)
                .with_restart_policy(RestartPolicy::OnFailure),
        )
        .unwrap();
    let first_exit = reap_pid(&mut supervisor, restarted_pid);
    assert_eq!(first_exit.status.code(), Some(1));
    let restarted = first_exit.restarted_pid.unwrap();
    let second_exit = reap_pid(&mut supervisor, restarted);
    assert_eq!(second_exit.status.code(), Some(0));
    assert_eq!(supervisor.active_count(), 0);

    let source: OwnedFd = File::open("/dev/null").unwrap().into();
    let mut mapped = SpawnCommand::new(probe_command(0, false));
    mapped.map_fd(source, 55).unwrap();
    let mapped = mapped
        .spawn(
            &mut supervisor,
            ProcessOptions::new(ProcessKind::Application).session_owned(false),
        )
        .unwrap();
    assert_probe_exit(&mut supervisor, mapped.pid);

    let source: OwnedFd = File::open("/dev/null").unwrap().into();
    let mut mapped_with_stderr = SpawnCommand::new(probe_command(0, false));
    mapped_with_stderr.map_fd(source, 56).unwrap();
    let mapped_with_stderr = mapped_with_stderr
        .spawn_with_stderr(
            &mut supervisor,
            ProcessOptions::new(ProcessKind::Application).session_owned(false),
        )
        .unwrap();
    drop(mapped_with_stderr.stderr);
    assert_probe_exit(&mut supervisor, mapped_with_stderr.process.pid);

    let dedicated = supervisor
        .spawn_with_identity(
            probe_command(0, false),
            ProcessOptions::new(ProcessKind::Xwayland)
                .with_process_group_policy(ProcessGroupPolicy::Dedicated),
        )
        .unwrap();
    assert_eq!(dedicated.pgid, Some(i32::try_from(dedicated.pid).unwrap()));
    assert_probe_exit(&mut supervisor, dedicated.pid);
    assert!(sigchld_is_blocked_for_current_thread().unwrap());
}

#[test]
fn child_with_unblocked_sigchld_can_wait_for_its_own_descendant() {
    block_sigchld_for_current_thread().unwrap();
    let mut supervisor = ChildSupervisor::with_sigchld_reaper().unwrap();
    let child = supervisor
        .spawn(
            probe_command(0, true),
            ProcessOptions::new(ProcessKind::Application).session_owned(false),
        )
        .unwrap();
    assert_probe_exit(&mut supervisor, child);
}
