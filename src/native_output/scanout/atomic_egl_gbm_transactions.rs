use super::*;
use crate::native_output::runtime::settle_failed_output_transaction;
use oblivion_one::native::kms::AtomicFlipRequest;
use oblivion_one::native::sync_file::SyncFileDeadlineHint;

impl AtomicEglGbmScanout {
    pub(crate) fn submit_ready_frame(
        &mut self,
        kms: &KmsBackendSelection,
        server: &mut OwnCompositorServer,
        output_transactions: &mut OutputTransactionLedger,
    ) -> io::Result<(u64, u32, OutputTransactionId)> {
        let ready_transaction_id = self
            .swapchain()?
            .ready_transaction_id()
            .ok_or_else(|| io::Error::other("no rendered output frame is ready"))?;
        let (mut presentation_mode, content_type, async_validation_key) = {
            let transaction = output_transactions
                .transaction(ready_transaction_id)
                .ok_or_else(|| {
                    io::Error::other("ready transaction disappeared before presentation lookup")
                })?
                .descriptor();
            (
                transaction.presentation_mode(),
                transaction.content_type(),
                transaction.async_validation_key(),
            )
        };
        if presentation_mode.is_async()
            && let Some(key) = async_validation_key
            && !self.async_validation_is_accepted(key)
        {
            let ready_slot = self
                .swapchain()?
                .ready_slot()
                .ok_or_else(|| io::Error::other("Async validation has no ready slot"))?;
            let framebuffer = self.framebuffer(ready_slot)?;
            let test_token = PageFlipToken::new(allocate_native_page_flip_token())
                .expect("allocated native pageflip token is nonzero");
            let test_result = kms
                .atomic_commit_submitter()
                .ok_or_else(|| io::Error::other("Async validation requires Atomic KMS"))?
                .test_primary_without_cursor_with_presentation(
                    framebuffer,
                    test_token,
                    OutputPresentationMode::Async,
                    content_type,
                );
            match test_result {
                Ok(()) => self.note_composited_async_validation(key, true),
                Err(error) => {
                    self.note_composited_async_validation(key, false);
                    output_transactions
                        .downgrade_presentation_to_vsync(ready_transaction_id)
                        .map_err(io::Error::other)?;
                    presentation_mode = OutputPresentationMode::Vsync;
                    eprintln!("composited Async TEST_ONLY rejected; using Vsync: {error}");
                }
            }
        }
        if presentation_mode.is_async() && !self.ready_render_fence_is_signaled()? {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Async Atomic submission requested before the render fence signaled",
            ));
        }
        let mut frame = self.swapchain_mut()?.take_ready_for_submission()?;
        let transaction_id = frame.transaction_id;
        debug_assert_eq!(transaction_id, ready_transaction_id);
        let planned_cursor = match output_transactions
            .transaction(transaction_id)
            .ok_or_else(|| io::Error::other("ready transaction disappeared before submission"))?
            .descriptor()
            .planes()
            .cursor()
        {
            CursorPlaneAssignment::Atomic {
                state: Some(state), ..
            } => Some(state.clone()),
            CursorPlaneAssignment::Atomic { state: None, .. }
            | CursorPlaneAssignment::Unchanged
            | CursorPlaneAssignment::Disabled => None,
        };
        let framebuffer = self.framebuffer(frame.slot)?;
        let token = PageFlipToken::new(allocate_native_page_flip_token())
            .expect("allocated native pageflip token is nonzero");
        if self.deadline_hints_enabled {
            match frame
                .render_fence
                .apply_deadline_hint(frame.target.presentation_time.get(), monotonic_now_ns()?)
            {
                Ok(Some(SyncFileDeadlineHint::Applied)) => {
                    self.counters.sync_file_deadline_hints_applied += 1;
                }
                Ok(None) => {}
                Ok(Some(SyncFileDeadlineHint::Unsupported)) => {
                    self.counters.sync_file_deadline_hints_unsupported += 1;
                    self.deadline_hints_enabled = false;
                }
                Err(error)
                    if matches!(error.raw_os_error(), Some(libc::EBADF) | Some(libc::EFAULT)) =>
                {
                    let failure = io::Error::other(format!(
                        "invalid native fence deadline-hint contract: {error}"
                    ));
                    settle_failed_output_transaction(
                        output_transactions,
                        transaction_id,
                        OutputTransactionFailureStage::BackendOwnershipTransfer,
                        MonotonicTimestampNs::new(monotonic_now_ns()?),
                        |obligations| {
                            let batch_id = obligations.frame_batch_id().ok_or_else(|| {
                                io::Error::other(
                                    "fence deadline-hint failure transaction has no frame batch",
                                )
                            })?;
                            let frame = self.swapchain_mut()?.submission_failed(frame)?;
                            server.discard_frame_batch(
                                batch_id,
                                FrameBatchDiscardReason::FatalOutputFailure,
                            );
                            self.discard_failed_frame_resources(frame);
                            Ok(())
                        },
                    )
                    .map_err(|error| io::Error::other(error.to_string()))?;
                    return Err(failure);
                }
                Err(error) => {
                    self.counters.sync_file_deadline_hints_failed += 1;
                    eprintln!("native sync-file deadline hints disabled: {error}");
                    self.deadline_hints_enabled = false;
                }
            }
        }
        let in_fence = match frame.render_fence.take_submission_fd() {
            Ok(fence) => fence,
            Err(error) => {
                settle_failed_output_transaction(
                    output_transactions,
                    transaction_id,
                    OutputTransactionFailureStage::BackendOwnershipTransfer,
                    MonotonicTimestampNs::new(monotonic_now_ns()?),
                    |obligations| {
                        let batch_id = obligations.frame_batch_id().ok_or_else(|| {
                            io::Error::other("fence export failure transaction has no frame batch")
                        })?;
                        let frame = self.swapchain_mut()?.submission_failed(frame)?;
                        server.discard_frame_batch(
                            batch_id,
                            FrameBatchDiscardReason::FatalOutputFailure,
                        );
                        self.discard_failed_frame_resources(frame);
                        Ok(())
                    },
                )
                .map_err(|error| io::Error::other(error.to_string()))?;
                return Err(error);
            }
        };
        let submit_started_at = MonotonicTimestampNs::new(monotonic_now_ns()?);
        let submission = kms.submit_atomic_flip(AtomicFlipRequest {
            framebuffer,
            token,
            in_fence,
            cursor: planned_cursor,
            presentation_mode,
            content_type,
        });
        let submit_returned_at = MonotonicTimestampNs::new(monotonic_now_ns()?);
        match submission {
            Ok(submission) => {
                self.counters.atomic_in_fence_submissions += 1;
                if submission.out_fence.is_some() {
                    self.counters.atomic_out_fences_received += 1;
                } else {
                    self.counters.atomic_out_fence_missing += 1;
                }
                self.swapchain_mut()?
                    .submission_succeeded(
                        frame,
                        token,
                        submission.out_fence,
                        submit_started_at,
                        submit_returned_at,
                    )
                    .map_err(|error| io::Error::other(error.to_string()))?;
                output_transactions
                    .mark_submitted(transaction_id, token, submit_returned_at)
                    .map_err(io::Error::other)?;
                Ok((token.get(), framebuffer.get(), transaction_id))
            }
            Err(error) => {
                if presentation_mode.is_async() {
                    if let Some(key) = async_validation_key {
                        self.note_async_validation(key, false);
                    }
                }
                let failure =
                    io::Error::other(format!("explicit Atomic output submission failed: {error}"));
                settle_failed_output_transaction(
                    output_transactions,
                    transaction_id,
                    OutputTransactionFailureStage::KmsSubmit,
                    MonotonicTimestampNs::new(monotonic_now_ns()?),
                    |obligations| {
                        let batch_id = obligations.frame_batch_id().ok_or_else(|| {
                            io::Error::other("Atomic submit failure transaction has no frame batch")
                        })?;
                        let frame = self.swapchain_mut()?.submission_failed(frame)?;
                        server.discard_frame_batch(
                            batch_id,
                            FrameBatchDiscardReason::FatalOutputFailure,
                        );
                        self.discard_failed_frame_resources(frame);
                        Ok(())
                    },
                )
                .map_err(|error| io::Error::other(error.to_string()))?;
                Err(failure)
            }
        }
    }
}
