use std::{io, os::unix::process::CommandExt, process::Command};

pub(super) fn prepare_child_signal_mask(command: &mut Command) {
    // SAFETY: the closure is installed for the post-fork child and performs
    // only the async-signal-safe signal-mask reset before exec.
    unsafe {
        command.pre_exec(unblock_sigchld_before_exec);
    }
}

fn unblock_sigchld_before_exec() -> io::Result<()> {
    let mask = super::sigchld_mask()?;
    let result = unsafe {
        // SAFETY: `mask` is initialized with only SIGCHLD and the null old-set
        // pointer is valid because the child does not need the previous mask.
        libc::sigprocmask(libc::SIG_UNBLOCK, &mask, std::ptr::null_mut())
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
