use oblivion_one::process::{
    ChildSupervisor, ProcessKind, ProcessOptions, block_sigchld_for_current_thread,
    sigchld_is_blocked_for_current_thread,
};
use std::{process::Command, thread};

fn shell_command(script: &str) -> Command {
    let mut command = Command::new("sh");
    command.arg("-c").arg(script);
    command
}

#[test]
fn sigchld_reaper_blocks_sigchld_in_the_bootstrap_thread() {
    // Keep every test thread in this binary blocked before any sibling test
    // can launch a child. This prevents a process-directed SIGCHLD from being
    // consumed by an unrelated unblocked harness thread.
    block_sigchld_for_current_thread().unwrap();
    let supervisor = ChildSupervisor::with_sigchld_reaper().unwrap();
    assert!(sigchld_is_blocked_for_current_thread().unwrap());
    assert!(supervisor.signal_fd().is_some());
}

#[test]
fn early_sigchld_block_is_inherited_by_later_threads() {
    block_sigchld_for_current_thread().unwrap();
    let inherited = thread::spawn(|| sigchld_is_blocked_for_current_thread().unwrap())
        .join()
        .unwrap();
    assert!(inherited);
}

#[test]
fn one_child_exit_wakes_the_sigchld_signalfd_once() {
    let mut supervisor = ChildSupervisor::with_sigchld_reaper().unwrap();
    let pid = supervisor
        .spawn(
            shell_command("exit 0"),
            ProcessOptions::new(ProcessKind::Application),
        )
        .unwrap();
    let signal_fd = supervisor.signal_fd().unwrap();
    let mut readiness = libc::pollfd {
        fd: signal_fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let poll_result = unsafe {
        // SAFETY: `readiness` points to one initialized poll descriptor; the
        // bounded timeout only waits for this supervisor-owned fd.
        libc::poll(&mut readiness, 1, 2_000)
    };
    assert_eq!(poll_result, 1);
    assert_ne!(readiness.revents & libc::POLLIN, 0);

    let exits = supervisor.reap_exited().unwrap();
    assert_eq!(exits.len(), 1);
    assert_eq!(exits[0].pid, pid);

    let mut second_readiness = libc::pollfd {
        fd: signal_fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let second_poll = unsafe {
        // SAFETY: `second_readiness` points to one initialized poll descriptor
        // for the same supervisor-owned fd.
        libc::poll(&mut second_readiness, 1, 0)
    };
    assert_eq!(second_poll, 0);
}

#[test]
fn sigchld_reaper_does_not_install_a_traditional_handler() {
    let _supervisor = ChildSupervisor::with_sigchld_reaper().unwrap();
    let mut action = unsafe {
        // SAFETY: zeroed storage is valid for `sigaction` to initialize.
        std::mem::zeroed::<libc::sigaction>()
    };
    let result = unsafe {
        // SAFETY: a null new action queries the current SIGCHLD action into
        // the valid `action` storage.
        libc::sigaction(libc::SIGCHLD, std::ptr::null(), &mut action)
    };
    assert_eq!(result, 0);
    assert_eq!(action.sa_sigaction, libc::SIG_DFL);
}
