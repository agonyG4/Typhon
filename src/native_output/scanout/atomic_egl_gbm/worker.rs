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

    pub(crate) fn accept_direct_submitted(
        &mut self,
        submitted: SubmittedDirectPrimary,
    ) -> Result<(), Box<SubmittedDirectPrimaryError>> {
        let has_out_fence = submitted.out_fence.is_some();
        self.direct.ownership.accept_submitted(submitted)?;
        self.direct.counters.submissions = self.direct.counters.submissions.saturating_add(1);
        if has_out_fence {
            self.direct.counters.out_fences_received =
                self.direct.counters.out_fences_received.saturating_add(1);
        } else {
            self.direct.counters.out_fence_missing =
                self.direct.counters.out_fence_missing.saturating_add(1);
        }
        Ok(())
    }

    pub(crate) fn record_direct_validation_success(&mut self, key: DirectPlaneValidationKey) {
        self.direct.record_direct_validation_success(key);
    }

    pub(crate) fn invalidate_direct_validation(&mut self, key: DirectPlaneValidationKey) {
        self.direct.invalidate_direct_validation(key);
    }

    pub(crate) fn suspend_abandon_worker_submission(
        &mut self,
        token: PageFlipToken,
    ) -> io::Result<()> {
        self.swapchain_mut()?.suspend_abandon_worker_queued(token)?;
        Ok(())
    }

    pub(crate) fn return_worker_submission_for_replan(
        &mut self,
        token: PageFlipToken,
        submission_fence: OwnedFd,
        cursor_owner: &mut Option<FrozenCursorPlaneOwner>,
    ) -> io::Result<()> {
        if !self.swapchain_mut()?.return_worker_queued_for_replan(
            token,
            submission_fence,
            cursor_owner,
        )? {
            return Err(io::Error::other(
                "explicit worker re-plan has no queued output frame",
            ));
        }
        Ok(())
    }
}
