use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use super::{
    AtomicFlipRequest, AtomicFlipSubmission, AtomicKmsError, AtomicKmsErrorKind,
    AtomicPipelineProperties, AtomicRequest, AtomicSubmission,
};

pub(crate) fn submit_atomic_flip_with(
    pipeline: &AtomicPipelineProperties,
    request: AtomicFlipRequest,
    submit: impl FnOnce(&AtomicSubmission) -> Result<(), AtomicKmsError>,
) -> Result<AtomicFlipSubmission, AtomicKmsError> {
    let mut out_fence_storage = -1i32;
    let out_fence_ptr = pipeline
        .crtc_props
        .out_fence_ptr
        .map(|_| std::ptr::addr_of_mut!(out_fence_storage));
    let in_fence_property = pipeline.plane_props.in_fence_fd.ok_or_else(|| {
        AtomicKmsError::new(
            AtomicKmsErrorKind::MissingProperty,
            "primary plane is missing required IN_FENCE_FD",
        )
    })?;
    let mut atomic_request = AtomicRequest::primary_flip_with_cursor(
        pipeline,
        request.framebuffer,
        request.cursor.as_ref(),
    )?;
    atomic_request.set_plane(
        pipeline.plane,
        in_fence_property,
        u64::try_from(request.in_fence.as_raw_fd()).map_err(|_| {
            AtomicKmsError::new(
                AtomicKmsErrorKind::MissingProperty,
                "Atomic input fence FD is negative",
            )
        })?,
    )?;
    if let (Some(property), Some(pointer)) = (pipeline.crtc_props.out_fence_ptr, out_fence_ptr) {
        atomic_request.set_crtc(pipeline.crtc, property, pointer as u64)?;
    }
    let submission = AtomicSubmission::page_flip(atomic_request, request.token);
    let result = submit(&submission);
    match result {
        Ok(()) => Ok(AtomicFlipSubmission {
            out_fence: adopt_out_fence(out_fence_storage),
        }),
        Err(error) => {
            drop(adopt_out_fence(out_fence_storage));
            Err(error)
        }
    }
}

pub(super) fn adopt_out_fence(raw_fd: i32) -> Option<OwnedFd> {
    (raw_fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(raw_fd) })
}

pub(super) fn elapsed_micros(started: std::time::Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}
