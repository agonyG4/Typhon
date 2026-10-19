use std::{io, os::unix::process::CommandExt, process::Command};

/// Give each child its own open file description for inherited stdio pipes.
///
/// File status flags such as O_NONBLOCK live on the shared open file
/// description, so a child that marks its inherited stderr non-blocking also
/// makes Typhon's stderr non-blocking. A full log pipe then turns Typhon's
/// `eprintln!` into an EAGAIN panic. Reopening the pipe through /proc creates
/// a separate description that still writes into the same pipe.
pub fn isolate_child_stdio_pipes(command: &mut Command) {
    // SAFETY: the closure runs in the post-fork child after std has installed
    // the child's stdio, and only calls async-signal-safe fstat/open/dup2/close.
    unsafe {
        command.pre_exec(reopen_inherited_stdio_pipes);
    }
}

fn reopen_inherited_stdio_pipes() -> io::Result<()> {
    const PATHS: [(&std::ffi::CStr, libc::c_int); 2] = [
        (c"/proc/self/fd/1", libc::STDOUT_FILENO),
        (c"/proc/self/fd/2", libc::STDERR_FILENO),
    ];
    for (path, fd) in PATHS {
        let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
        if unsafe { libc::fstat(fd, &mut stat) } != 0
            || stat.st_mode & libc::S_IFMT != libc::S_IFIFO
        {
            continue;
        }
        // Best effort: without /proc the child keeps the shared description.
        let reopened = unsafe { libc::open(path.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC) };
        if reopened < 0 {
            continue;
        }
        let duplicated = unsafe { libc::dup2(reopened, fd) };
        unsafe { libc::close(reopened) };
        if duplicated < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::isolate_child_stdio_pipes;
    use std::{
        io::Read,
        os::fd::{FromRawFd, OwnedFd},
        process::{Command, Stdio},
    };

    #[test]
    fn child_nonblocking_stderr_does_not_leak_into_parent_description() {
        let mut fds = [0; 2];
        assert_eq!(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) }, 0);
        let (read_end, write_end) =
            unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
        let parent_write = write_end.try_clone().expect("clone write end");

        let mut command = Command::new("python3");
        command
            .args([
                "-I",
                "-c",
                "import os,fcntl,sys; fl=fcntl.fcntl(2,fcntl.F_GETFL); \
                 fcntl.fcntl(2,fcntl.F_SETFL,fl|os.O_NONBLOCK); sys.stderr.write('ok')",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(write_end));
        isolate_child_stdio_pipes(&mut command);
        let status = command.status().expect("child runs");
        assert!(status.success());
        // The command still owns its stderr end; drop it so the read sees EOF.
        drop(command);

        let flags = unsafe {
            libc::fcntl(
                std::os::fd::AsRawFd::as_raw_fd(&parent_write),
                libc::F_GETFL,
            )
        };
        assert_eq!(
            flags & libc::O_NONBLOCK,
            0,
            "child flag leaked into parent description"
        );
        drop(parent_write);
        let mut output = String::new();
        std::fs::File::from(read_end)
            .read_to_string(&mut output)
            .expect("read pipe");
        assert_eq!(output, "ok");
    }
}
