use oblivion_one::process::{
    ChildExit, ChildSupervisor, ProcessGroupPolicy, ProcessKind, ProcessOptions, RestartPolicy,
    SpawnCommand, block_sigchld_for_current_thread, sigchld_is_blocked_for_current_thread,
};
use std::{
    fs::File,
    os::fd::OwnedFd,
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
fn supervisor_spawn_unblocks_sigchld_only_in_the_child() {
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
