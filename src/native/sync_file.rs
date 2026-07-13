#![allow(dead_code)] // Production timing watches are integrated with the Atomic runtime in Task 12.

use std::{
    io, mem,
    os::fd::{AsRawFd, BorrowedFd},
};

const MAX_SYNC_FILE_FENCES: usize = 64;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RawSyncFenceInfo {
    obj_name: [libc::c_char; 32],
    driver_name: [libc::c_char; 32],
    status: i32,
    flags: u32,
    timestamp_ns: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RawSyncFileInfo {
    name: [libc::c_char; 32],
    status: i32,
    flags: u32,
    num_fences: u32,
    pad: u32,
    sync_fence_info: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RawSyncSetDeadline {
    deadline_ns: u64,
    pad: u64,
}

#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncFileInfo {
    pub fence_count: u32,
    pub signal_timestamp_ns: u64,
}

#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncFileDeadlineHint {
    Applied,
    Unsupported,
}

trait SyncFileIoctl {
    fn file_info(&self, fd: BorrowedFd<'_>, info: &mut RawSyncFileInfo) -> io::Result<()>;
    fn set_deadline(&self, fd: BorrowedFd<'_>, deadline: &RawSyncSetDeadline) -> io::Result<()>;
}

struct RealSyncFileIoctl;

impl SyncFileIoctl for RealSyncFileIoctl {
    fn file_info(&self, fd: BorrowedFd<'_>, info: &mut RawSyncFileInfo) -> io::Result<()> {
        let result = unsafe { libc::ioctl(fd.as_raw_fd(), sync_iowr::<RawSyncFileInfo>(4), info) };
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn set_deadline(&self, fd: BorrowedFd<'_>, deadline: &RawSyncSetDeadline) -> io::Result<()> {
        let result =
            unsafe { libc::ioctl(fd.as_raw_fd(), sync_iow::<RawSyncSetDeadline>(5), deadline) };
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

#[doc(hidden)]
pub fn query_sync_file_info(fd: BorrowedFd<'_>) -> io::Result<SyncFileInfo> {
    query_sync_file_info_with(&RealSyncFileIoctl, fd)
}

fn query_sync_file_info_with(
    ioctl: &impl SyncFileIoctl,
    fd: BorrowedFd<'_>,
) -> io::Result<SyncFileInfo> {
    let mut header = RawSyncFileInfo::default();
    ioctl.file_info(fd, &mut header)?;
    validate_sync_file_status(header.status)?;
    let fence_count = usize::try_from(header.num_fences)
        .map_err(|_| io::Error::other("sync-file fence count overflow"))?;
    if fence_count == 0 || fence_count > MAX_SYNC_FILE_FENCES {
        return Err(io::Error::other(format!(
            "sync-file fence count {} is outside 1..={MAX_SYNC_FILE_FENCES}",
            header.num_fences
        )));
    }

    let mut fences = vec![RawSyncFenceInfo::default(); fence_count];
    header.sync_fence_info = fences.as_mut_ptr() as u64;
    ioctl.file_info(fd, &mut header)?;
    validate_sync_file_status(header.status)?;
    let returned_count = usize::try_from(header.num_fences)
        .map_err(|_| io::Error::other("sync-file returned fence count overflow"))?;
    if returned_count == 0 || returned_count > fence_count {
        return Err(io::Error::other(
            "sync-file returned an invalid constituent fence count",
        ));
    }
    let mut maximum_timestamp = 0u64;
    for fence in &fences[..returned_count] {
        validate_sync_file_status(fence.status)?;
        if fence.timestamp_ns == 0 {
            return Err(io::Error::other(
                "signaled sync-file constituent has zero timestamp",
            ));
        }
        maximum_timestamp = maximum_timestamp.max(fence.timestamp_ns);
    }
    Ok(SyncFileInfo {
        fence_count: header.num_fences,
        signal_timestamp_ns: maximum_timestamp,
    })
}

fn validate_sync_file_status(status: i32) -> io::Result<()> {
    match status {
        1 => Ok(()),
        0 => Err(io::Error::from(io::ErrorKind::WouldBlock)),
        status => Err(io::Error::other(format!(
            "sync-file reported fence failure status {status}"
        ))),
    }
}

#[doc(hidden)]
pub fn set_sync_file_deadline(
    fd: BorrowedFd<'_>,
    deadline_ns: u64,
) -> io::Result<SyncFileDeadlineHint> {
    set_sync_file_deadline_with(&RealSyncFileIoctl, fd, deadline_ns)
}

fn set_sync_file_deadline_with(
    ioctl: &impl SyncFileIoctl,
    fd: BorrowedFd<'_>,
    deadline_ns: u64,
) -> io::Result<SyncFileDeadlineHint> {
    if deadline_ns == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "sync-file deadline must be a nonzero CLOCK_MONOTONIC timestamp",
        ));
    }
    match ioctl.set_deadline(
        fd,
        &RawSyncSetDeadline {
            deadline_ns,
            pad: 0,
        },
    ) {
        Ok(()) => Ok(SyncFileDeadlineHint::Applied),
        Err(error)
            if matches!(
                error.raw_os_error(),
                Some(libc::ENOTTY) | Some(libc::EOPNOTSUPP)
            ) =>
        {
            Ok(SyncFileDeadlineHint::Unsupported)
        }
        Err(error) => Err(error),
    }
}

const fn sync_iowr<T>(number: u8) -> libc::c_ulong {
    ioctl_number::<T>(number, 3)
}

const fn sync_iow<T>(number: u8) -> libc::c_ulong {
    ioctl_number::<T>(number, 1)
}

const fn ioctl_number<T>(number: u8, direction: u32) -> libc::c_ulong {
    const IOC_NRBITS: u32 = 8;
    const IOC_TYPEBITS: u32 = 8;
    const IOC_SIZEBITS: u32 = 14;
    const IOC_TYPESHIFT: u32 = IOC_NRBITS;
    const IOC_SIZESHIFT: u32 = IOC_TYPESHIFT + IOC_TYPEBITS;
    const IOC_DIRSHIFT: u32 = IOC_SIZESHIFT + IOC_SIZEBITS;
    ((direction << IOC_DIRSHIFT)
        | ((b'>' as u32) << IOC_TYPESHIFT)
        | (number as u32)
        | ((mem::size_of::<T>() as u32) << IOC_SIZESHIFT)) as libc::c_ulong
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    #[derive(Default)]
    struct FakeIoctl {
        timestamps: Vec<u64>,
        status: i32,
        deadline_result: RefCell<Option<io::Result<()>>>,
        observed_deadline: RefCell<Option<u64>>,
    }

    impl SyncFileIoctl for FakeIoctl {
        fn file_info(&self, _fd: BorrowedFd<'_>, info: &mut RawSyncFileInfo) -> io::Result<()> {
            info.status = self.status;
            info.num_fences = self.timestamps.len() as u32;
            if info.sync_fence_info != 0 {
                let output = unsafe {
                    std::slice::from_raw_parts_mut(
                        info.sync_fence_info as *mut RawSyncFenceInfo,
                        self.timestamps.len(),
                    )
                };
                for (fence, timestamp) in output.iter_mut().zip(&self.timestamps) {
                    fence.status = self.status;
                    fence.timestamp_ns = *timestamp;
                }
            }
            Ok(())
        }

        fn set_deadline(
            &self,
            _fd: BorrowedFd<'_>,
            deadline: &RawSyncSetDeadline,
        ) -> io::Result<()> {
            self.observed_deadline.replace(Some(deadline.deadline_ns));
            self.deadline_result.borrow_mut().take().unwrap_or(Ok(()))
        }
    }

    fn borrowed_stdin() -> BorrowedFd<'static> {
        unsafe { BorrowedFd::borrow_raw(libc::STDIN_FILENO) }
    }

    #[test]
    fn exact_sync_file_timestamp_uses_maximum_constituent_timestamp() {
        let ioctl = FakeIoctl {
            timestamps: vec![11, 37, 23],
            status: 1,
            ..Default::default()
        };
        let info = query_sync_file_info_with(&ioctl, borrowed_stdin()).unwrap();
        assert_eq!(info.fence_count, 3);
        assert_eq!(info.signal_timestamp_ns, 37);
    }

    #[test]
    fn sync_file_info_rejects_unbounded_zero_timestamp_and_failed_fences() {
        for ioctl in [
            FakeIoctl {
                timestamps: vec![1; MAX_SYNC_FILE_FENCES + 1],
                status: 1,
                ..Default::default()
            },
            FakeIoctl {
                timestamps: vec![0],
                status: 1,
                ..Default::default()
            },
            FakeIoctl {
                timestamps: vec![1],
                status: -libc::EIO,
                ..Default::default()
            },
        ] {
            assert!(query_sync_file_info_with(&ioctl, borrowed_stdin()).is_err());
        }
    }

    #[test]
    fn deadline_hint_preserves_absolute_monotonic_timestamp_and_unsupported_result() {
        let applied = FakeIoctl::default();
        assert_eq!(
            set_sync_file_deadline_with(&applied, borrowed_stdin(), 123_456).unwrap(),
            SyncFileDeadlineHint::Applied
        );
        assert_eq!(*applied.observed_deadline.borrow(), Some(123_456));

        let unsupported = FakeIoctl {
            deadline_result: RefCell::new(Some(Err(io::Error::from_raw_os_error(libc::ENOTTY)))),
            ..Default::default()
        };
        assert_eq!(
            set_sync_file_deadline_with(&unsupported, borrowed_stdin(), 99).unwrap(),
            SyncFileDeadlineHint::Unsupported
        );
    }
}
