use std::{io, os::unix::process::CommandExt, process::Command};

/// Reset Typhon's managed signal mask before the command is executed so
/// subprocesses retain normal SIGCHLD and termination-signal behavior.
pub fn prepare_child_signal_mask(command: &mut Command) {
    // SAFETY: the closure is installed for the post-fork child and performs
    // only the async-signal-safe signal-mask reset before exec.
    unsafe {
        command.pre_exec(unblock_managed_signals_before_exec);
    }
}

fn unblock_managed_signals_before_exec() -> io::Result<()> {
    let mask = super::runtime_signal_mask()?;
    let result = unsafe {
        // SAFETY: `mask` contains only signals blocked by Typhon for its
        // signalfd-driven runtime path; the null old-set pointer is valid
        // because the child does not need the previous mask.
        libc::sigprocmask(libc::SIG_UNBLOCK, &mask, std::ptr::null_mut())
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
