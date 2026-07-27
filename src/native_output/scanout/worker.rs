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

    pub(crate) fn promote_worker_direct_submission(
        &mut self,
        context: DirectPromotionContext,
        lease: DirectPrimaryLease,
        out_fence: Option<OwnedFd>,
    ) -> Result<CompositorFrameBatchId, Box<DirectPromotionError>> {
        match self {
            Self::AtomicEglGbm(scanout) => {
                scanout.promote_worker_direct_submission(context, lease, out_fence)
            }
            _ => Err(Box::new(DirectPromotionError {
                reason: DirectPromotionFailure::MissingQueued,
                error: io::Error::other("direct worker success requires explicit Atomic scanout"),
                lease,
                out_fence,
            })),
        }
    }

    pub(crate) fn fail_worker_direct_submission(
        &mut self,
        token: PageFlipToken,
    ) -> io::Result<CompositorFrameBatchId> {
        match self {
            Self::AtomicEglGbm(scanout) => scanout.fail_worker_direct_submission(token),
            _ => Err(io::Error::other(
                "direct worker failure requires explicit Atomic scanout",
            )),
        }
    }

    pub(crate) fn suspend_abandon_worker_direct(&mut self, token: PageFlipToken) -> io::Result<()> {
        match self {
            Self::AtomicEglGbm(scanout) => scanout.suspend_abandon_worker_direct(token),
            _ => Err(io::Error::other(
                "direct worker suspension requires explicit Atomic scanout",
            )),
        }
    }

    pub(crate) fn suspend_worker_direct_submission(
        &mut self,
        token: PageFlipToken,
        lease: DirectPrimaryLease,
    ) -> Result<(), super::DirectPrimaryLeaseTransferError> {
        match self {
            Self::AtomicEglGbm(scanout) => scanout.suspend_worker_direct_submission(token, lease),
            _ => Err(Box::new((
                io::Error::other("direct worker suspension requires explicit Atomic scanout"),
                lease,
            ))),
        }
    }

    pub(crate) fn store_worker_direct_submission(
        &mut self,
        frame: WorkerQueuedDirectFrame,
    ) -> io::Result<()> {
        match self {
            Self::AtomicEglGbm(scanout) => scanout.store_worker_direct_submission(frame),
            _ => Err(io::Error::other(
                "direct worker metadata requires explicit Atomic scanout",
            )),
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
