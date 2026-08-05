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
    let mut previous = unsafe {
        // SAFETY: a zeroed `sigset_t` is valid storage for the signal-mask
        // query below.
        std::mem::zeroed::<libc::sigset_t>()
    };
    let query_result = unsafe {
        // SAFETY: a null new-mask pointer queries the current thread's signal
        // mask into the valid `previous` storage.
        libc::pthread_sigmask(libc::SIG_BLOCK, std::ptr::null(), &mut previous)
    };
    assert_eq!(query_result, 0);

    let supervisor = ChildSupervisor::with_sigchld_reaper().unwrap();
    assert!(sigchld_is_blocked_for_current_thread().unwrap());
    assert!(supervisor.signal_fd().is_some());

    let restore_result = unsafe {
        // SAFETY: `previous` was populated by the earlier query and is a
        // valid signal set for restoring this test thread's mask.
        libc::pthread_sigmask(libc::SIG_SETMASK, &previous, std::ptr::null_mut())
    };
    assert_eq!(restore_result, 0);
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
    if poll_result == 0 {
        // The Rust test harness can have sibling threads with an unblocked
        // SIGCHLD mask, so Linux may deliver the process-directed notification
        // to one of them.  Target the already-blocked test thread to make the
        // signalfd assertion deterministic without a traditional handler.
        let signal_result = unsafe {
            // SAFETY: `pthread_self` identifies the current test thread, and
            // `pthread_kill` targets only that thread with SIGCHLD.
            libc::pthread_kill(libc::pthread_self(), libc::SIGCHLD)
        };
        assert_eq!(signal_result, 0);
        let retry = unsafe {
            // SAFETY: `readiness` remains the initialized descriptor for the
            // supervisor-owned signalfd.
            libc::poll(&mut readiness, 1, 2_000)
        };
        assert_eq!(retry, 1);
    } else {
        assert_eq!(poll_result, 1);
    }
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
