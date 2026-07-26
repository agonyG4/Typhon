use super::*;

impl AtomicEglGbmScanout {
    pub(crate) fn promote_worker_submission(
        &mut self,
        token: PageFlipToken,
        out_fence: Option<OwnedFd>,
        submit_started_at: MonotonicTimestampNs,
        submit_returned_at: MonotonicTimestampNs,
    ) -> io::Result<()> {
        self.swapchain_mut()?.promote_worker_queued(
            token,
            out_fence,
            submit_started_at,
            submit_returned_at,
        )
    }

    pub(crate) fn fail_worker_submission(&mut self, token: PageFlipToken) -> io::Result<()> {
        let frame = self.swapchain_mut()?.fail_worker_queued(token)?;
        self.discard_failed_frame_resources(frame);
        Ok(())
    }

    pub(crate) fn promote_worker_direct_submission(
        &mut self,
        token: PageFlipToken,
        lease: DirectPrimaryLease,
        out_fence: Option<OwnedFd>,
        submit_started_at: MonotonicTimestampNs,
        submit_returned_at: MonotonicTimestampNs,
    ) -> io::Result<CompositorFrameBatchId> {
        let has_out_fence = out_fence.is_some();
        let batch_id = self.direct.promote_worker_submission(
            token,
            lease,
            out_fence,
            submit_started_at,
            submit_returned_at,
        )?;
        self.direct.counters.submissions = self.direct.counters.submissions.saturating_add(1);
        if has_out_fence {
            self.direct.counters.out_fences_received =
                self.direct.counters.out_fences_received.saturating_add(1);
        } else {
            self.direct.counters.out_fence_missing =
                self.direct.counters.out_fence_missing.saturating_add(1);
        }
        Ok(batch_id)
    }

    pub(crate) fn fail_worker_direct_submission(
        &mut self,
        token: PageFlipToken,
    ) -> io::Result<CompositorFrameBatchId> {
        self.direct.fail_worker_submission(token)
    }

    pub(crate) fn suspend_abandon_worker_direct(&mut self, token: PageFlipToken) -> io::Result<()> {
        self.direct.suspend_worker_queued(token)
    }

    pub(crate) fn suspend_worker_direct_submission(
        &mut self,
        token: PageFlipToken,
        lease: DirectPrimaryLease,
    ) -> io::Result<()> {
        self.direct.suspend_worker_submission(token, lease)
    }

    pub(crate) fn store_worker_direct_submission(
        &mut self,
        frame: WorkerQueuedDirectFrame,
    ) -> io::Result<()> {
        self.direct.store_worker_queued(frame)
    }

    pub(crate) fn suspend_abandon_worker_submission(
        &mut self,
        token: PageFlipToken,
    ) -> io::Result<()> {
        self.swapchain_mut()?.suspend_abandon_worker_queued(token)?;
        Ok(())
    }
}
