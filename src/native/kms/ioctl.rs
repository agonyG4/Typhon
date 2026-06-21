use std::{
    io, mem,
    os::fd::{AsRawFd, BorrowedFd, RawFd},
    slice,
};

use super::{
    AtomicCommitFlags, AtomicKmsError, AtomicKmsErrorKind, AtomicSubmission, BlobId, DrmProperty,
    ModeBlobIo, PropertyId, SerializedAtomicRequest,
};

pub fn enable_atomic_client_capability(fd: BorrowedFd<'_>) -> Result<(), AtomicKmsError> {
    drm_ffi::set_capability(fd, u64::from(drm_sys::DRM_CLIENT_CAP_ATOMIC), true)
        .map(|_| ())
        .map_err(|error| {
            classify_io_error(
                error,
                AtomicKmsErrorKind::Unsupported,
                "enable atomic client capability",
            )
        })
}

pub fn disable_atomic_client_capability(fd: BorrowedFd<'_>) {
    let _ = drm_ffi::set_capability(fd, u64::from(drm_sys::DRM_CLIENT_CAP_ATOMIC), false);
}

pub fn object_properties(
    fd: BorrowedFd<'_>,
    object_id: u32,
    object_type: u32,
) -> Result<Vec<DrmProperty>, AtomicKmsError> {
    let mut ids = Vec::new();
    let mut values = Vec::new();
    drm_ffi::mode::get_properties(
        fd,
        object_id,
        object_type,
        Some(&mut ids),
        Some(&mut values),
    )
    .map_err(|error| {
        classify_io_error(
            error,
            AtomicKmsErrorKind::MissingObject,
            "query object properties",
        )
    })?;
    if ids.len() != values.len() {
        return Err(AtomicKmsError::new(
            AtomicKmsErrorKind::MissingProperty,
            format!(
                "object {object_id} returned {} property IDs but {} values",
                ids.len(),
                values.len()
            ),
        ));
    }
    ids.into_iter()
        .zip(values)
        .map(|(id, value)| {
            let property = drm_ffi::mode::get_property(fd, id, None, None).map_err(|error| {
                classify_io_error(
                    error,
                    AtomicKmsErrorKind::MissingProperty,
                    "query property metadata",
                )
            })?;
            let id = PropertyId::new(id).ok_or_else(|| {
                AtomicKmsError::new(
                    AtomicKmsErrorKind::MissingProperty,
                    "DRM returned property ID zero",
                )
            })?;
            Ok(DrmProperty::new(
                id,
                drm_property_name(&property.name),
                value,
            ))
        })
        .collect()
}

fn drm_property_name(name: &[libc::c_char]) -> String {
    let bytes = name
        .iter()
        .take_while(|byte| **byte != 0)
        .map(|byte| *byte as u8)
        .collect::<Vec<_>>();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[derive(Debug, Clone, Copy)]
pub struct DrmModeBlobIo {
    fd: RawFd,
}

impl DrmModeBlobIo {
    pub const fn new(fd: RawFd) -> Self {
        Self { fd }
    }

    fn fd(&self) -> BorrowedFd<'_> {
        unsafe { BorrowedFd::borrow_raw(self.fd) }
    }
}

impl ModeBlobIo for DrmModeBlobIo {
    fn create_mode_blob(
        &self,
        mode: &drm_sys::drm_mode_modeinfo,
    ) -> Result<BlobId, AtomicKmsError> {
        let mut mode = *mode;
        let bytes = unsafe {
            slice::from_raw_parts_mut(
                (&mut mode as *mut drm_sys::drm_mode_modeinfo).cast::<u8>(),
                mem::size_of::<drm_sys::drm_mode_modeinfo>(),
            )
        };
        let blob = drm_ffi::mode::create_property_blob(self.fd(), bytes).map_err(|error| {
            classify_io_error(error, AtomicKmsErrorKind::BlobCreation, "create mode blob")
        })?;
        BlobId::new(blob.blob_id).ok_or_else(|| {
            AtomicKmsError::new(
                AtomicKmsErrorKind::BlobCreation,
                "DRM returned mode blob ID zero",
            )
        })
    }

    fn destroy_mode_blob(&self, blob: BlobId) -> Result<(), AtomicKmsError> {
        drm_ffi::mode::destroy_property_blob(self.fd(), blob.get())
            .map(|_| ())
            .map_err(|error| classify_io_error(error, AtomicKmsErrorKind::Io, "destroy mode blob"))
    }
}

pub fn submit_atomic(
    fd: BorrowedFd<'_>,
    submission: &AtomicSubmission,
    error_kind: AtomicKmsErrorKind,
    operation: &'static str,
) -> Result<(), AtomicKmsError> {
    let serialized = submission.request.serialize();
    submit_serialized_atomic(
        fd,
        serialized,
        submission.flags,
        submission.user_data,
        error_kind,
        operation,
    )
}

pub fn submit_serialized_atomic(
    fd: BorrowedFd<'_>,
    mut request: SerializedAtomicRequest,
    flags: AtomicCommitFlags,
    user_data: u64,
    error_kind: AtomicKmsErrorKind,
    operation: &'static str,
) -> Result<(), AtomicKmsError> {
    if request.objects.len() != request.property_counts.len()
        || request.properties.len() != request.values.len()
        || request
            .property_counts
            .iter()
            .try_fold(0usize, |total, count| total.checked_add(*count as usize))
            != Some(request.properties.len())
    {
        return Err(AtomicKmsError::new(
            AtomicKmsErrorKind::DuplicateAssignment,
            format!("{operation}: malformed serialized atomic request"),
        ));
    }
    let mut atomic = drm_sys::drm_mode_atomic {
        flags: flags.bits(),
        count_objs: u32::try_from(request.objects.len()).map_err(|_| {
            AtomicKmsError::new(
                AtomicKmsErrorKind::DuplicateAssignment,
                "too many atomic objects",
            )
        })?,
        objs_ptr: request.objects.as_mut_ptr() as u64,
        count_props_ptr: request.property_counts.as_mut_ptr() as u64,
        props_ptr: request.properties.as_mut_ptr() as u64,
        prop_values_ptr: request.values.as_mut_ptr() as u64,
        reserved: 0,
        user_data,
    };
    let result = unsafe {
        libc::ioctl(
            fd.as_raw_fd(),
            drm_iowr::<drm_sys::drm_mode_atomic>(0xBC),
            &mut atomic,
        )
    };
    if result < 0 {
        let mut error = classify_io_error(io::Error::last_os_error(), error_kind, operation);
        error.detail.push_str(&format!(
            "; flags={:#x} objects={:?} property_counts={:?} properties={:?} values={:?} user_data={user_data}",
            flags.bits(),
            request.objects,
            request.property_counts,
            request.properties,
            request.values,
        ));
        Err(error)
    } else {
        Ok(())
    }
}

fn classify_io_error(
    error: io::Error,
    default_kind: AtomicKmsErrorKind,
    operation: &'static str,
) -> AtomicKmsError {
    let kind = match error.raw_os_error() {
        Some(libc::EBUSY) => AtomicKmsErrorKind::Busy,
        Some(libc::EACCES) | Some(libc::EPERM) => AtomicKmsErrorKind::PermissionOrSession,
        Some(libc::ENODEV) | Some(libc::EIO) => AtomicKmsErrorKind::DeviceLost,
        _ => default_kind,
    };
    AtomicKmsError::new(kind, format!("{operation} failed: {error}"))
}

const fn drm_iowr<T>(number: u8) -> libc::c_ulong {
    const IOC_NRBITS: u32 = 8;
    const IOC_TYPEBITS: u32 = 8;
    const IOC_SIZEBITS: u32 = 14;
    const IOC_NRSHIFT: u32 = 0;
    const IOC_TYPESHIFT: u32 = IOC_NRSHIFT + IOC_NRBITS;
    const IOC_SIZESHIFT: u32 = IOC_TYPESHIFT + IOC_TYPEBITS;
    const IOC_DIRSHIFT: u32 = IOC_SIZESHIFT + IOC_SIZEBITS;
    const IOC_WRITE: u32 = 1;
    const IOC_READ: u32 = 2;
    (((IOC_READ | IOC_WRITE) << IOC_DIRSHIFT)
        | ((b'd' as u32) << IOC_TYPESHIFT)
        | ((number as u32) << IOC_NRSHIFT)
        | ((mem::size_of::<T>() as u32) << IOC_SIZESHIFT)) as libc::c_ulong
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errno_classification_distinguishes_busy_permission_and_device_loss() {
        for errno in [libc::EACCES, libc::EPERM] {
            assert_eq!(
                classify_io_error(
                    io::Error::from_raw_os_error(errno),
                    AtomicKmsErrorKind::FlipRejected,
                    "test",
                )
                .kind,
                AtomicKmsErrorKind::PermissionOrSession
            );
        }
        for errno in [libc::ENODEV, libc::EIO] {
            assert_eq!(
                classify_io_error(
                    io::Error::from_raw_os_error(errno),
                    AtomicKmsErrorKind::FlipRejected,
                    "test",
                )
                .kind,
                AtomicKmsErrorKind::DeviceLost
            );
        }
        assert_eq!(
            classify_io_error(
                io::Error::from_raw_os_error(libc::EBUSY),
                AtomicKmsErrorKind::FlipRejected,
                "test",
            )
            .kind,
            AtomicKmsErrorKind::Busy
        );
    }
}
