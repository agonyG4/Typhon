use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

use super::XwaylandGeneration;
use crate::process::ManagedProcessId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DisplayFdInspection {
    pub status_flags: i32,
    pub descriptor_flags: i32,
    pub bytes_available: u64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct DisplayFdLog {
    pub event: &'static str,
    pub detail: Option<&'static str>,
    pub generation: Option<XwaylandGeneration>,
    pub process_id: Option<ManagedProcessId>,
    pub leased_display: Option<u32>,
    pub parent_read_fd: Option<RawFd>,
    pub child_source_fd: Option<RawFd>,
    pub child_target_fd: Option<RawFd>,
    pub reactor_token: Option<u64>,
    pub epoll_flags: Option<u32>,
    pub inspection: Option<DisplayFdInspection>,
    pub bytes_read: Option<usize>,
}

pub(crate) fn log(log: DisplayFdLog) {
    let inspection = log.inspection;
    eprintln!(
        "oblivion-one xwayland: event={} detail={} generation={:?} process_id={} leased_display={} parent_read_fd={} child_source_fd={} child_target_fd={} reactor_token={} epoll_flags={} fd_status_flags={} fd_descriptor_flags={} bytes_available={} bytes_read={}",
        log.event,
        log.detail.unwrap_or("none"),
        log.generation,
        log.process_id
            .map(|id| id.get().to_string())
            .unwrap_or_else(|| "none".to_string()),
        log.leased_display
            .map(|display| display.to_string())
            .unwrap_or_else(|| "none".to_string()),
        log.parent_read_fd
            .map(|fd| fd.to_string())
            .unwrap_or_else(|| "none".to_string()),
        log.child_source_fd
            .map(|fd| fd.to_string())
            .unwrap_or_else(|| "none".to_string()),
        log.child_target_fd
            .map(|fd| fd.to_string())
            .unwrap_or_else(|| "none".to_string()),
        log.reactor_token
            .map(|token| token.to_string())
            .unwrap_or_else(|| "none".to_string()),
        log.epoll_flags
            .map(|flags| format!("0x{flags:x}"))
            .unwrap_or_else(|| "none".to_string()),
        inspection
            .map(|value| value.status_flags.to_string())
            .unwrap_or_else(|| "none".to_string()),
        inspection
            .map(|value| value.descriptor_flags.to_string())
            .unwrap_or_else(|| "none".to_string()),
        inspection
            .map(|value| value.bytes_available.to_string())
            .unwrap_or_else(|| "none".to_string()),
        log.bytes_read
            .map(|bytes| bytes.to_string())
            .unwrap_or_else(|| "none".to_string()),
    );
}

pub(crate) fn create_pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [-1; 2];
    // SAFETY: `fds` points to two writable integers and the flags request a
    // close-on-exec pipe. Both descriptors are wrapped immediately below.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `pipe2` initialized both descriptors on success.
    let parent_read = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    // SAFETY: `pipe2` initialized both descriptors on success.
    let child_write = unsafe { OwnedFd::from_raw_fd(fds[1]) };
    set_nonblocking(parent_read.as_raw_fd())?;
    Ok((parent_read, child_write))
}

pub(crate) fn inspect(fd: RawFd) -> io::Result<DisplayFdInspection> {
    // SAFETY: `fd` is supplied by the owner of the displayfd endpoint.
    let status_flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if status_flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is supplied by the owner of the displayfd endpoint.
    let descriptor_flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if descriptor_flags < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut available = 0i32;
    // SAFETY: `available` is writable storage for the kernel's bounded byte
    // count and `fd` is the live read endpoint.
    if unsafe { libc::ioctl(fd, libc::FIONREAD, &mut available) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(DisplayFdInspection {
        status_flags,
        descriptor_flags,
        bytes_available: u64::try_from(available.max(0)).unwrap_or(0),
    })
}

fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    // SAFETY: `fd` is live and owned by the caller for both operations.
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

#[cfg(test)]
pub(crate) fn pipe_pair_for_tests() -> io::Result<(OwnedFd, OwnedFd)> {
    create_pipe()
}
