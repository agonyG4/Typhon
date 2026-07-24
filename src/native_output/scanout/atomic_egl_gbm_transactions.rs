use super::*;
use oblivion_one::native::kms::AtomicFlipRequest;
use oblivion_one::native::sync_file::SyncFileDeadlineHint;

impl AtomicEglGbmScanout {
    pub(crate) fn submit_ready_frame(
        &mut self,
        kms: &KmsBackendSelection,
        server: &mut OwnCompositorServer,
        output_transactions: &mut OutputTransactionLedger,
        cursor: Option<&AtomicCursorVisualState>,
    ) -> io::Result<(u64, u32, OutputTransactionId)> {
        let mut frame = self.swapchain_mut()?.take_ready_for_submission()?;
        let transaction_id = frame.transaction_id;
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
                    let frame = self.swapchain_mut()?.submission_failed(frame)?;
                    output_transactions
                        .mark_failed(
                            transaction_id,
                            OutputTransactionFailureStage::FenceExport,
                            MonotonicTimestampNs::new(monotonic_now_ns()?),
                        )
                        .map_err(io::Error::other)?;
                    self.discard_failed_frame(server, frame);
                    return Err(io::Error::other(format!(
                        "invalid native fence deadline-hint contract: {error}"
                    )));
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
                let frame = self.swapchain_mut()?.submission_failed(frame)?;
                output_transactions
                    .mark_failed(
                        transaction_id,
                        OutputTransactionFailureStage::FenceExport,
                        MonotonicTimestampNs::new(monotonic_now_ns()?),
                    )
                    .map_err(io::Error::other)?;
                self.discard_failed_frame(server, frame);
                return Err(error);
            }
        };
        let submit_started_at = MonotonicTimestampNs::new(monotonic_now_ns()?);
        let submission = kms.submit_atomic_flip(AtomicFlipRequest {
            framebuffer,
            token,
            in_fence,
            cursor: cursor.cloned(),
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
                self.swapchain_mut()?.submission_succeeded(
                    frame,
                    token,
                    submission.out_fence,
                    submit_started_at,
                    submit_returned_at,
                )?;
                output_transactions
                    .mark_submitted(transaction_id, token, submit_returned_at)
                    .map_err(io::Error::other)?;
                Ok((token.get(), framebuffer.get(), transaction_id))
            }
            Err(error) => {
                let frame = self.swapchain_mut()?.submission_failed(frame)?;
                output_transactions
                    .mark_failed(
                        transaction_id,
                        OutputTransactionFailureStage::KmsSubmit,
                        MonotonicTimestampNs::new(monotonic_now_ns()?),
                    )
                    .map_err(io::Error::other)?;
                self.discard_failed_frame(server, frame);
                Err(io::Error::other(format!(
                    "explicit Atomic output submission failed: {error}"
                )))
            }
        }
    }
}
