use std::{
    fmt,
    fs::{File, OpenOptions},
    io,
    os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd},
    sync::Arc,
};

#[derive(Clone)]
pub struct DrmSyncobjDevice {
    file: Arc<File>,
}

impl fmt::Debug for DrmSyncobjDevice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DrmSyncobjDevice")
            .finish_non_exhaustive()
    }
}

impl DrmSyncobjDevice {
    pub fn open_available() -> Option<Self> {
        syncobj_device_candidates()
            .into_iter()
            .filter_map(|path| OpenOptions::new().read(true).write(true).open(path).ok())
            .find(device_supports_timeline_syncobj)
            .map(|file| Self {
                file: Arc::new(file),
            })
    }

    pub fn import_timeline_fd(&self, fd: OwnedFd) -> io::Result<DrmSyncobjTimeline> {
        let mut args = drm_sys::drm_syncobj_handle {
            handle: 0,
            flags: drm_sys::DRM_SYNCOBJ_FD_TO_HANDLE_FLAGS_TIMELINE,
            fd: fd.as_raw_fd(),
            pad: 0,
            point: 0,
        };
        raw_drm_ioctl(
            self.file.as_fd().as_raw_fd(),
            drm_iowr::<drm_sys::drm_syncobj_handle>(0xC2),
            &mut args,
        )?;
        Ok(DrmSyncobjTimeline::from_imported_handle(
            self.clone(),
            args.handle,
        ))
    }

    pub fn create_timeline_for_tests(&self) -> io::Result<DrmSyncobjTimeline> {
        let handle = drm_ffi::syncobj::create(self.file.as_fd(), false)?.handle;
        Ok(DrmSyncobjTimeline::from_imported_handle(
            self.clone(),
            handle,
        ))
    }

    fn destroy_handle(&self, handle: u32) {
        let _ = drm_ffi::syncobj::destroy(self.file.as_fd(), handle);
    }

    fn signal_point(&self, handle: u32, point: u64) -> io::Result<()> {
        drm_ffi::syncobj::timeline_signal(self.file.as_fd(), &[handle], &[point])?;
        Ok(())
    }

    fn point_signaled(&self, handle: u32, point: u64) -> io::Result<bool> {
        match drm_ffi::syncobj::timeline_wait(
            self.file.as_fd(),
            &[handle],
            &[point],
            0,
            true,
            false,
            false,
        ) {
            Ok(_) => Ok(true),
            Err(error) if syncobj_wait_timed_out(&error) => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub fn as_file(&self) -> &File {
        &self.file
    }
}

#[derive(Clone)]
pub struct DrmSyncobjTimeline {
    inner: Arc<DrmSyncobjTimelineInner>,
}

impl fmt::Debug for DrmSyncobjTimeline {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DrmSyncobjTimeline")
            .field("handle", &self.handle())
            .finish()
    }
}

impl DrmSyncobjTimeline {
    fn from_imported_handle(device: DrmSyncobjDevice, handle: u32) -> Self {
        Self {
            inner: Arc::new(DrmSyncobjTimelineInner { device, handle }),
        }
    }

    pub fn handle(&self) -> u32 {
        self.inner.handle
    }

    pub fn same_timeline(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner) || self.handle() == other.handle()
    }

    pub fn signal_point(&self, point: u64) -> io::Result<()> {
        self.inner.device.signal_point(self.inner.handle, point)
    }

    pub fn point_signaled(&self, point: u64) -> io::Result<bool> {
        self.inner.device.point_signaled(self.inner.handle, point)
    }

    pub(crate) fn register_eventfd(
        &self,
        point: u64,
        event_fd: BorrowedFd<'_>,
    ) -> Result<(), SyncobjEventfdError> {
        drm_ffi::syncobj::eventfd(
            self.inner.device.as_file().as_fd(),
            self.handle(),
            point,
            event_fd,
            false,
        )
        .map(|_| ())
        .map_err(classify_syncobj_eventfd_error)
    }

    pub fn export_timeline_fd(&self) -> io::Result<File> {
        let mut args = drm_sys::drm_syncobj_handle {
            handle: self.handle(),
            flags: drm_sys::DRM_SYNCOBJ_HANDLE_TO_FD_FLAGS_TIMELINE,
            fd: -1,
            pad: 0,
            point: 0,
        };
        raw_drm_ioctl(
            self.inner.device.as_file().as_fd().as_raw_fd(),
            drm_iowr::<drm_sys::drm_syncobj_handle>(0xC1),
            &mut args,
        )?;
        // SAFETY: DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD returns a newly owned file
        // descriptor on success, and this File takes over that ownership.
        Ok(unsafe { File::from_raw_fd(args.fd) })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyncobjEventfdErrnoClass {
    Unsupported,
    Failure(i32),
}

#[derive(Debug)]
pub(crate) struct SyncobjEventfdError {
    source: io::Error,
    class: SyncobjEventfdErrnoClass,
}

impl SyncobjEventfdError {
    pub(crate) const fn class(&self) -> SyncobjEventfdErrnoClass {
        self.class
    }

    pub(crate) fn raw_os_error(&self) -> Option<i32> {
        self.source.raw_os_error()
    }
}

impl fmt::Display for SyncobjEventfdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.source.fmt(formatter)
    }
}

impl std::error::Error for SyncobjEventfdError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

fn classify_syncobj_eventfd_error(error: io::Error) -> SyncobjEventfdError {
    let class = error
        .raw_os_error()
        .map(classify_syncobj_eventfd_errno)
        .unwrap_or(SyncobjEventfdErrnoClass::Failure(0));
    SyncobjEventfdError {
        source: error,
        class,
    }
}

const fn classify_syncobj_eventfd_errno(errno: i32) -> SyncobjEventfdErrnoClass {
    if errno == libc::ENOTTY || errno == libc::EOPNOTSUPP || errno == libc::ENOSYS {
        SyncobjEventfdErrnoClass::Unsupported
    } else {
        SyncobjEventfdErrnoClass::Failure(errno)
    }
}

#[cfg(test)]
fn build_syncobj_eventfd_request(
    handle: u32,
    point: u64,
    event_fd: libc::c_int,
) -> io::Result<drm_sys::drm_syncobj_eventfd> {
    if event_fd < 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "syncobj eventfd must be nonnegative",
        ));
    }
    Ok(drm_sys::drm_syncobj_eventfd {
        handle,
        flags: 0,
        point,
        fd: event_fd,
        pad: 0,
    })
}

struct DrmSyncobjTimelineInner {
    device: DrmSyncobjDevice,
    handle: u32,
}

impl Drop for DrmSyncobjTimelineInner {
    fn drop(&mut self) {
        self.device.destroy_handle(self.handle);
    }
}

fn syncobj_device_candidates() -> Vec<String> {
    (128..144)
        .map(|index| format!("/dev/dri/renderD{index}"))
        .chain((0..16).map(|index| format!("/dev/dri/card{index}")))
        .collect()
}

fn device_supports_timeline_syncobj(file: &File) -> bool {
    let fd = file.as_fd();
    let syncobj = drm_ffi::get_capability(fd, u64::from(drm_sys::DRM_CAP_SYNCOBJ))
        .is_ok_and(|cap| cap.value != 0);
    let timeline = drm_ffi::get_capability(fd, u64::from(drm_sys::DRM_CAP_SYNCOBJ_TIMELINE))
        .is_ok_and(|cap| cap.value != 0);
    syncobj && timeline
}

fn syncobj_wait_timed_out(error: &io::Error) -> bool {
    // Some drivers report EINVAL while a timeline point has no submitted
    // fence yet; for non-blocking compositor polling that means "not ready".
    matches!(
        error.raw_os_error(),
        Some(code)
            if code == libc::ETIME
                || code == libc::EBUSY
                || code == libc::EAGAIN
                || code == libc::EINVAL
    )
}

fn raw_drm_ioctl<T>(fd: libc::c_int, request: libc::c_ulong, data: &mut T) -> io::Result<()> {
    // SAFETY: The request code matches the concrete DRM UAPI struct type T,
    // and the pointer remains valid for the duration of the ioctl call.
    let result = unsafe { libc::ioctl(fd, request, data) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn drm_iowr<T>(nr: u8) -> libc::c_ulong {
    const IOC_NRBITS: libc::c_ulong = 8;
    const IOC_TYPEBITS: libc::c_ulong = 8;
    const IOC_SIZEBITS: libc::c_ulong = 14;
    const IOC_WRITE: libc::c_ulong = 1;
    const IOC_READ: libc::c_ulong = 2;
    const IOC_NRSHIFT: libc::c_ulong = 0;
    const IOC_TYPESHIFT: libc::c_ulong = IOC_NRSHIFT + IOC_NRBITS;
    const IOC_SIZESHIFT: libc::c_ulong = IOC_TYPESHIFT + IOC_TYPEBITS;
    const IOC_DIRSHIFT: libc::c_ulong = IOC_SIZESHIFT + IOC_SIZEBITS;

    ((IOC_READ | IOC_WRITE) << IOC_DIRSHIFT)
        | ((std::mem::size_of::<T>() as libc::c_ulong) << IOC_SIZESHIFT)
        | ((b'd' as libc::c_ulong) << IOC_TYPESHIFT)
        | ((nr as libc::c_ulong) << IOC_NRSHIFT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syncobj_eventfd_request_matches_kernel_layout() {
        assert_eq!(std::mem::size_of::<drm_sys::drm_syncobj_eventfd>(), 24);
        assert_eq!(std::mem::align_of::<drm_sys::drm_syncobj_eventfd>(), 8);
        assert_eq!(std::mem::offset_of!(drm_sys::drm_syncobj_eventfd, handle), 0);
        assert_eq!(std::mem::offset_of!(drm_sys::drm_syncobj_eventfd, flags), 4);
        assert_eq!(std::mem::offset_of!(drm_sys::drm_syncobj_eventfd, point), 8);
        assert_eq!(std::mem::offset_of!(drm_sys::drm_syncobj_eventfd, fd), 16);
        assert_eq!(std::mem::offset_of!(drm_sys::drm_syncobj_eventfd, pad), 20);
    }

    #[test]
    fn syncobj_eventfd_request_waits_for_signal_and_preserves_fields() {
        let point = 0xfedc_ba98_7654_3210;
        let request = build_syncobj_eventfd_request(0x89ab_cdef, point, 17).unwrap();

        assert_eq!(request.handle, 0x89ab_cdef);
        assert_eq!(request.point, point);
        assert_eq!(request.fd, 17);
        assert_eq!(request.flags, 0);
        assert_ne!(
            request.flags,
            drm_sys::DRM_SYNCOBJ_WAIT_FLAGS_WAIT_AVAILABLE
        );
        assert_eq!(request.pad, 0);
    }

    #[test]
    fn syncobj_eventfd_request_rejects_negative_fd() {
        let error = build_syncobj_eventfd_request(1, 2, -1).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn syncobj_eventfd_unsupported_errno_classification_is_narrow() {
        for errno in [libc::ENOTTY, libc::EOPNOTSUPP, libc::ENOSYS] {
            assert_eq!(
                classify_syncobj_eventfd_errno(errno),
                SyncobjEventfdErrnoClass::Unsupported
            );
        }
        for errno in [libc::EINVAL, libc::EBADF, libc::EMFILE, libc::ENOMEM] {
            assert_eq!(
                classify_syncobj_eventfd_errno(errno),
                SyncobjEventfdErrnoClass::Failure(errno)
            );
        }
    }

    #[test]
    fn syncobj_eventfd_error_preserves_errno() {
        let error = io::Error::from_raw_os_error(libc::ENOMEM);
        let classified = classify_syncobj_eventfd_error(error);

        assert_eq!(classified.raw_os_error(), Some(libc::ENOMEM));
        assert_eq!(classified.class(), SyncobjEventfdErrnoClass::Failure(libc::ENOMEM));
    }

    #[test]
    fn syncobj_device_opens_when_kernel_reports_timeline_support() {
        if let Some(device) = DrmSyncobjDevice::open_available() {
            let timeline = device.create_timeline_for_tests().unwrap();
            assert!(!timeline.point_signaled(1).unwrap());
            timeline.signal_point(1).unwrap();
            assert!(timeline.point_signaled(1).unwrap());
        }
    }
}
