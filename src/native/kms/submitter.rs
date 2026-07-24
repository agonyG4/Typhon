use std::os::fd::{AsRawFd, BorrowedFd, RawFd};

use super::{
    AtomicCursorVisualState, AtomicFlipSubmission, AtomicKmsError, AtomicKmsErrorKind,
    AtomicPipelineProperties, AtomicRequest, AtomicSubmission, FramebufferId, PageFlipToken,
    submit_atomic,
};

/// Submit-only, cloneable metadata for the normal runtime Atomic path.
///
/// The DRM descriptor is borrowed by number. The runtime owns that descriptor
/// and joins the worker before the descriptor can be closed, revoked, restored,
/// or replaced. This type never performs DRM I/O in `Drop` and does not contain
/// restore or modeset state.
#[derive(Debug, Clone)]
pub struct AtomicCommitSubmitter {
    fd: RawFd,
    pipeline: AtomicPipelineProperties,
}

impl AtomicCommitSubmitter {
    pub(super) const fn new(fd: RawFd, pipeline: AtomicPipelineProperties) -> Self {
        Self { fd, pipeline }
    }

    pub const fn fd(&self) -> RawFd {
        self.fd
    }

    pub fn pipeline(&self) -> &AtomicPipelineProperties {
        &self.pipeline
    }

    pub fn submit_primary(
        &self,
        framebuffer: FramebufferId,
        token: PageFlipToken,
        cursor: Option<&AtomicCursorVisualState>,
        in_fence: Option<BorrowedFd<'_>>,
        request_out_fence: bool,
        test_only: bool,
    ) -> Result<AtomicFlipSubmission, AtomicKmsError> {
        self.submit_primary_inner(
            framebuffer,
            token,
            cursor,
            true,
            in_fence,
            request_out_fence,
            test_only,
        )
    }

    pub fn submit_primary_without_cursor(
        &self,
        framebuffer: FramebufferId,
        token: PageFlipToken,
        in_fence: Option<BorrowedFd<'_>>,
        request_out_fence: bool,
        test_only: bool,
    ) -> Result<AtomicFlipSubmission, AtomicKmsError> {
        self.submit_primary_inner(
            framebuffer,
            token,
            None,
            false,
            in_fence,
            request_out_fence,
            test_only,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn submit_primary_inner(
        &self,
        framebuffer: FramebufferId,
        token: PageFlipToken,
        cursor: Option<&AtomicCursorVisualState>,
        touch_cursor: bool,
        in_fence: Option<BorrowedFd<'_>>,
        request_out_fence: bool,
        test_only: bool,
    ) -> Result<AtomicFlipSubmission, AtomicKmsError> {
        // This pointer is request-local and remains valid until the ioctl
        // returns. It never crosses the worker boundary in a payload.
        let mut out_fence_storage = -1i32;
        let out_fence_ptr = request_out_fence.then_some(std::ptr::addr_of_mut!(out_fence_storage));
        let mut request = if touch_cursor {
            AtomicRequest::primary_flip_with_cursor_and_out_fence(
                &self.pipeline,
                framebuffer,
                cursor,
                out_fence_ptr,
            )?
        } else {
            let mut request = AtomicRequest::primary_flip(
                self.pipeline.plane,
                self.pipeline.plane_props.fb_id,
                framebuffer,
            )?;
            if let Some(property) = self.pipeline.crtc_props.out_fence_ptr
                && let Some(pointer) = out_fence_ptr
            {
                request.set_crtc(self.pipeline.crtc, property, pointer as u64)?;
            }
            request
        };
        if let Some(in_fence) = in_fence {
            let property = self.pipeline.plane_props.in_fence_fd.ok_or_else(|| {
                AtomicKmsError::new(
                    AtomicKmsErrorKind::MissingProperty,
                    "primary plane is missing required IN_FENCE_FD",
                )
            })?;
            request.set_plane(
                self.pipeline.plane,
                property,
                u64::try_from(in_fence.as_raw_fd()).map_err(|_| {
                    AtomicKmsError::new(
                        AtomicKmsErrorKind::MissingProperty,
                        "Atomic input fence FD is negative",
                    )
                })?,
            )?;
        } else if test_only {
            request.set_test_input_fence_none(&self.pipeline)?;
        }
        let submission = if test_only {
            AtomicSubmission::test_only(request)
        } else {
            AtomicSubmission::page_flip(request, token)
        };
        // SAFETY: the runtime owns the DRM fd and joins the worker before the
        // fd can be closed, revoked, restored, or replaced.
        let fd = unsafe { BorrowedFd::borrow_raw(self.fd) };
        let result = submit_atomic(
            fd,
            &submission,
            if test_only {
                AtomicKmsErrorKind::TestOnlyRejected
            } else {
                AtomicKmsErrorKind::FlipRejected
            },
            if test_only {
                "runtime atomic TEST_ONLY primary update"
            } else {
                "runtime atomic primary update"
            },
        );
        match result {
            Ok(()) if !test_only => Ok(AtomicFlipSubmission {
                out_fence: super::submission::adopt_out_fence(out_fence_storage),
            }),
            Ok(()) => Ok(AtomicFlipSubmission { out_fence: None }),
            Err(error) => {
                drop(super::submission::adopt_out_fence(out_fence_storage));
                Err(error)
            }
        }
    }

    pub fn submit_cursor(
        &self,
        cursor: Option<&AtomicCursorVisualState>,
        token: PageFlipToken,
        test_only: bool,
    ) -> Result<AtomicFlipSubmission, AtomicKmsError> {
        let request = AtomicRequest::cursor_only(&self.pipeline, cursor)?;
        let submission = if test_only {
            AtomicSubmission::test_only(request)
        } else {
            AtomicSubmission::page_flip(request, token)
        };
        // SAFETY: the runtime owns the DRM fd and joins the worker before the
        // fd can be closed, revoked, restored, or replaced.
        let fd = unsafe { BorrowedFd::borrow_raw(self.fd) };
        submit_atomic(
            fd,
            &submission,
            if test_only {
                AtomicKmsErrorKind::TestOnlyRejected
            } else {
                AtomicKmsErrorKind::FlipRejected
            },
            if test_only {
                "runtime atomic TEST_ONLY cursor update"
            } else {
                "runtime atomic cursor update"
            },
        )?;
        Ok(AtomicFlipSubmission { out_fence: None })
    }
}

#[cfg(test)]
mod tests {
    use super::AtomicCommitSubmitter;

    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}

    #[test]
    fn submitter_is_send_sync_without_an_unsafe_impl() {
        assert_send::<AtomicCommitSubmitter>();
        assert_sync::<AtomicCommitSubmitter>();
    }
}
