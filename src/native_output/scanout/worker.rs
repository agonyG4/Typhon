use super::*;

impl NativeScanoutBackend {
    pub(crate) fn promote_worker_submission(
        &mut self,
        token: PageFlipToken,
        out_fence: Option<OwnedFd>,
        submit_started_at: MonotonicTimestampNs,
        submit_returned_at: MonotonicTimestampNs,
    ) -> io::Result<()> {
        match self {
            Self::AtomicEglGbm(scanout) => scanout.promote_worker_submission(
                token,
                out_fence,
                submit_started_at,
                submit_returned_at,
            ),
            _ => Err(io::Error::other(
                "worker submission requires explicit Atomic scanout",
            )),
        }
    }

    pub(crate) fn fail_worker_submission(&mut self, token: PageFlipToken) -> io::Result<()> {
        match self {
            Self::AtomicEglGbm(scanout) => scanout.fail_worker_submission(token),
            _ => Err(io::Error::other(
                "worker failure requires explicit Atomic scanout",
            )),
        }
    }

    pub(crate) fn suspend_abandon_worker_submission(
        &mut self,
        token: PageFlipToken,
    ) -> io::Result<()> {
        match self {
            Self::AtomicEglGbm(scanout) => scanout.suspend_abandon_worker_submission(token),
            _ => Err(io::Error::other(
                "worker suspension requires explicit Atomic scanout",
            )),
        }
    }

    pub(crate) fn return_worker_submission_for_replan(
        &mut self,
        token: PageFlipToken,
        submission_fence: Option<OwnedFd>,
    ) -> io::Result<()> {
        match self {
            Self::AtomicEglGbm(scanout) => scanout.return_worker_submission_for_replan(
                token,
                submission_fence.ok_or_else(|| {
                    io::Error::other("explicit worker re-plan is missing its input fence")
                })?,
            ),
            Self::NativeEglGbm(scanout) => {
                if submission_fence.is_some() {
                    return Err(io::Error::other(
                        "compatibility worker re-plan unexpectedly has an input fence",
                    ));
                }
                scanout.suspend_abandon_worker_submission(token)
            }
            Self::Gbm(scanout) => {
                if submission_fence.is_some() {
                    return Err(io::Error::other(
                        "GBM worker re-plan unexpectedly has an input fence",
                    ));
                }
                scanout.suspend_abandon_worker_submission(token)
            }
            Self::Dumb(_) => Err(io::Error::other(
                "worker re-plan is unavailable for dumb scanout",
            )),
        }
    }

    pub(crate) fn accept_direct_submitted(
        &mut self,
        submitted: SubmittedDirectPrimary,
    ) -> Result<(), Box<SubmittedDirectPrimaryError>> {
        match self {
            Self::AtomicEglGbm(scanout) => scanout.accept_direct_submitted(submitted),
            _ => Err(Box::new(SubmittedDirectPrimaryError {
                error: io::Error::other("direct worker success requires explicit Atomic scanout"),
                submitted,
            })),
        }
    }

    pub(crate) fn record_direct_validation_success(&mut self, key: DirectPlaneValidationKey) {
        if let Self::AtomicEglGbm(scanout) = self {
            scanout.record_direct_validation_success(key);
        }
    }

    pub(crate) fn invalidate_direct_validation(&mut self, key: DirectPlaneValidationKey) {
        if let Self::AtomicEglGbm(scanout) = self {
            scanout.invalidate_direct_validation(key);
        }
    }

    pub(crate) fn queue_worker_compatibility_submission(
        &mut self,
        token: PageFlipToken,
    ) -> io::Result<u32> {
        match self {
            Self::NativeEglGbm(scanout) => scanout.queue_worker_submission(token),
            Self::Gbm(scanout) => scanout.queue_worker_submission(token),
            _ => Err(io::Error::other(
                "Atomic compatibility worker requires native EGL/GBM scanout",
            )),
        }
    }

    pub(crate) fn promote_worker_compatibility_submission(
        &mut self,
        token: PageFlipToken,
    ) -> io::Result<()> {
        match self {
            Self::NativeEglGbm(scanout) => scanout.promote_worker_submission(token),
            Self::Gbm(scanout) => scanout.promote_worker_submission(token),
            _ => Err(io::Error::other(
                "Atomic compatibility worker requires native EGL/GBM scanout",
            )),
        }
    }

    pub(crate) fn fail_worker_compatibility_submission(
        &mut self,
        token: PageFlipToken,
    ) -> io::Result<()> {
        match self {
            Self::NativeEglGbm(scanout) => scanout.fail_worker_submission(token),
            Self::Gbm(scanout) => scanout.fail_worker_submission(token),
            _ => Err(io::Error::other(
                "Atomic compatibility worker requires native EGL/GBM scanout",
            )),
        }
    }

    pub(crate) fn suspend_abandon_worker_compatibility(
        &mut self,
        token: PageFlipToken,
    ) -> io::Result<()> {
        match self {
            Self::NativeEglGbm(scanout) => scanout.suspend_abandon_worker_submission(token),
            Self::Gbm(scanout) => scanout.suspend_abandon_worker_submission(token),
            _ => Err(io::Error::other(
                "Atomic compatibility worker requires native EGL/GBM scanout",
            )),
        }
    }

    pub(crate) fn page_flip_pending(&self) -> bool {
        match self {
            Self::AtomicEglGbm(scanout) => {
                let composited_pending = scanout.swapchain().is_ok_and(|swapchain| {
                    swapchain.pending_slot().is_some() || swapchain.worker_queued_slot().is_some()
                });
                let direct_pending = scanout.direct_scanout_pending();
                debug_assert!(!(composited_pending && direct_pending));
                composited_pending || direct_pending
            }
            Self::NativeEglGbm(scanout) => scanout.page_flip_pending(),
            Self::Gbm(scanout) => scanout.page_flip_pending(),
            Self::Dumb(_) => false,
        }
    }

    pub(crate) fn promote_worker_early_page_flip(
        &mut self,
        token: PageFlipToken,
    ) -> io::Result<()> {
        match self {
            Self::NativeEglGbm(scanout) => scanout.promote_worker_early_page_flip(token),
            Self::Gbm(scanout) => scanout.promote_worker_early_page_flip(token),
            Self::AtomicEglGbm(_) | Self::Dumb(_) => Ok(()),
        }
    }
}
