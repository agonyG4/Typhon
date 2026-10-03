use super::*;
use crate::native_output::runtime::settle_failed_output_transaction;
use oblivion_one::native::kms::AtomicFlipRequest;
use oblivion_one::native::sync_file::SyncFileDeadlineHint;

const ADAPTIVE_ASYNC_FALLBACKS: &[OutputPresentationMode] = &[
    OutputPresentationMode::AdaptiveSync,
    OutputPresentationMode::Async,
    OutputPresentationMode::Vsync,
];
const FIXED_REFRESH_FALLBACK: &[OutputPresentationMode] = &[OutputPresentationMode::Vsync];
const NO_PRESENTATION_FALLBACKS: &[OutputPresentationMode] = &[];

fn presentation_fallback_candidates(
    mode: OutputPresentationMode,
) -> &'static [OutputPresentationMode] {
    match mode {
        OutputPresentationMode::AdaptiveAsync => ADAPTIVE_ASYNC_FALLBACKS,
        OutputPresentationMode::AdaptiveSync | OutputPresentationMode::Async => {
            FIXED_REFRESH_FALLBACK
        }
        OutputPresentationMode::Vsync => NO_PRESENTATION_FALLBACKS,
    }
}

fn strongest_qualified_fallback(
    mode: OutputPresentationMode,
    qualified_modes: &[OutputPresentationMode],
) -> OutputPresentationMode {
    presentation_fallback_candidates(mode)
        .iter()
        .find(|candidate| {
            **candidate == OutputPresentationMode::Vsync || qualified_modes.contains(candidate)
        })
        .copied()
        .unwrap_or(OutputPresentationMode::Vsync)
}

pub(super) fn adaptive_async_blockers(
    adaptive_sync_qualified: bool,
    async_qualified: bool,
) -> (Option<VrrBlocker>, Option<AsyncBlocker>) {
    (
        (!adaptive_sync_qualified).then_some(VrrBlocker::ExactKmsQualificationRejected),
        (!async_qualified).then_some(AsyncBlocker::AsyncTestOnlyRejected),
    )
}

pub(super) fn adaptive_async_combination_blocker(
    requested_mode: OutputPresentationMode,
    vrr_blocker: Option<VrrBlocker>,
    async_blocker: Option<AsyncBlocker>,
) -> Option<&'static str> {
    (requested_mode == OutputPresentationMode::AdaptiveAsync
        && vrr_blocker.is_none()
        && async_blocker.is_none())
    .then_some("adaptive_async_exact_kms_rejected_as_combination")
}

pub(super) fn adaptive_async_fallback_diagnostics(
    adaptive_sync_qualified: bool,
    async_qualified: bool,
) -> (
    OutputPresentationMode,
    Option<VrrBlocker>,
    Option<AsyncBlocker>,
    Option<&'static str>,
) {
    let (vrr_blocker, async_blocker) =
        adaptive_async_blockers(adaptive_sync_qualified, async_qualified);
    let effective_mode = if adaptive_sync_qualified {
        OutputPresentationMode::AdaptiveSync
    } else if async_qualified {
        OutputPresentationMode::Async
    } else {
        OutputPresentationMode::Vsync
    };
    let combination_blocker = adaptive_async_combination_blocker(
        OutputPresentationMode::AdaptiveAsync,
        vrr_blocker,
        async_blocker,
    );
    (
        effective_mode,
        vrr_blocker,
        async_blocker,
        combination_blocker,
    )
}

fn log_composited_presentation_rejection(
    kms: &KmsBackendSelection,
    vrr_policy: VrrPolicy,
    requested_mode: OutputPresentationMode,
    effective_mode: OutputPresentationMode,
    vrr_blocker: Option<VrrBlocker>,
    async_blocker: Option<AsyncBlocker>,
    output_generation: u64,
    failure_stage: &'static str,
) {
    NativePerfLogger::from_env().log("native.output_presentation_qualification", || {
        vec![
            NativePerfField::str("configured_policy", vrr_policy.as_str()),
            NativePerfField::bool("drm_connector_capable", kms.atomic_connector_vrr_capable()),
            NativePerfField::bool(
                "crtc_vrr_property_available",
                kms.atomic_crtc_vrr_property_available(),
            ),
            NativePerfField::str("requested_mode", requested_mode.as_str()),
            NativePerfField::str("effective_mode", effective_mode.as_str()),
            NativePerfField::str(
                "vrr_blocker",
                vrr_blocker.map_or("none", VrrBlocker::as_str),
            ),
            NativePerfField::str(
                "async_blocker",
                async_blocker.map_or("none", AsyncBlocker::as_str),
            ),
            NativePerfField::str(
                "combination_blocker",
                adaptive_async_combination_blocker(requested_mode, vrr_blocker, async_blocker)
                    .unwrap_or("none"),
            ),
            NativePerfField::str("failure_stage", failure_stage),
            NativePerfField::u64("output_generation", output_generation),
        ]
    });
}

impl AtomicEglGbmScanout {
    fn qualify_composited_presentation_mode(
        &mut self,
        kms: &KmsBackendSelection,
        framebuffer: FramebufferId,
        test_token: PageFlipToken,
        mode: OutputPresentationMode,
        content_type: DrmContentType,
        cursor: Option<&AtomicCursorVisualState>,
        touch_cursor: bool,
        key: CompositedPresentationValidationKey,
    ) -> io::Result<bool> {
        if self.presentation_validation_is_accepted(key) {
            return Ok(true);
        }
        if self.presentation_validation_is_rejected(key) {
            return Ok(false);
        }
        let submitter = kms
            .atomic_commit_submitter()
            .ok_or_else(|| io::Error::other("presentation validation requires Atomic KMS"))?;
        let result = if touch_cursor {
            submitter.test_primary_with_presentation(
                framebuffer,
                test_token,
                cursor,
                mode,
                content_type,
            )
        } else {
            submitter.test_primary_without_cursor_with_presentation(
                framebuffer,
                test_token,
                mode,
                content_type,
            )
        };
        self.note_composited_presentation_validation(key, result.is_ok());
        Ok(result.is_ok())
    }

    pub(crate) fn submit_ready_frame(
        &mut self,
        kms: &KmsBackendSelection,
        server: &mut OwnCompositorServer,
        output_transactions: &mut OutputTransactionLedger,
        vrr_policy: VrrPolicy,
    ) -> io::Result<(u64, u32, OutputTransactionId)> {
        let ready_transaction_id = self
            .swapchain()?
            .ready_transaction_id()
            .ok_or_else(|| io::Error::other("no rendered output frame is ready"))?;
        self.swapchain()?
            .ready_identity()
            .and_then(|identity| identity.target)
            .ok_or_else(|| io::Error::other("unbound output frame cannot be submitted"))?;
        let (mut presentation_mode, content_type, mut presentation_validation_key) = {
            let transaction = output_transactions
                .transaction(ready_transaction_id)
                .ok_or_else(|| {
                    io::Error::other("ready transaction disappeared before presentation lookup")
                })?
                .descriptor();
            (
                transaction.presentation_mode(),
                transaction.content_type(),
                transaction.presentation_validation_key(),
            )
        };
        let (touch_cursor_for_test, test_cursor) = output_transactions
            .transaction(ready_transaction_id)
            .map(|record| match record.descriptor().planes().cursor() {
                CursorPlaneAssignment::Atomic { state, .. } => (true, state.clone()),
                CursorPlaneAssignment::Disabled => (true, None),
                CursorPlaneAssignment::Unchanged => (false, None),
            })
            .ok_or_else(|| {
                io::Error::other("ready transaction disappeared before cursor lookup")
            })?;
        let ready_slot = self
            .swapchain()?
            .ready_slot()
            .ok_or_else(|| io::Error::other("ready presentation validation has no ready slot"))?;
        let ready_framebuffer = self.framebuffer(ready_slot)?;
        let test_token = PageFlipToken::new(allocate_native_page_flip_token())
            .expect("allocated native pageflip token is nonzero");
        let fallback_candidates = presentation_fallback_candidates(presentation_mode).to_vec();
        let original_mode = presentation_mode;
        let original_qualified = if original_mode == OutputPresentationMode::Vsync {
            true
        } else if let Some(key) = presentation_validation_key {
            self.qualify_composited_presentation_mode(
                kms,
                ready_framebuffer,
                test_token,
                original_mode,
                content_type,
                test_cursor.as_ref(),
                touch_cursor_for_test,
                key,
            )?
        } else {
            false
        };
        let mut qualified_fallbacks = Vec::new();
        for candidate in fallback_candidates.iter().copied() {
            if candidate == OutputPresentationMode::Vsync {
                qualified_fallbacks.push((candidate, None));
                continue;
            }
            let Some(mut key) = presentation_validation_key else {
                continue;
            };
            key.presentation_mode = candidate;
            if self.qualify_composited_presentation_mode(
                kms,
                ready_framebuffer,
                test_token,
                candidate,
                content_type,
                test_cursor.as_ref(),
                touch_cursor_for_test,
                key,
            )? {
                qualified_fallbacks.push((candidate, Some(key)));
            }
        }
        if !original_qualified {
            if let Some(key) = presentation_validation_key {
                self.note_composited_presentation_validation(key, false);
            }
            let qualified_modes: Vec<_> =
                qualified_fallbacks.iter().map(|(mode, _)| *mode).collect();
            let fallback_mode = strongest_qualified_fallback(original_mode, &qualified_modes);
            let fallback_key = qualified_fallbacks
                .iter()
                .find(|(mode, _)| *mode == fallback_mode)
                .and_then(|(_, key)| *key);
            presentation_mode = fallback_mode;
            presentation_validation_key = fallback_key;
            let (vrr_blocker, async_blocker) = match original_mode {
                OutputPresentationMode::AdaptiveAsync => adaptive_async_blockers(
                    qualified_fallbacks
                        .iter()
                        .any(|(mode, _)| *mode == OutputPresentationMode::AdaptiveSync),
                    qualified_fallbacks
                        .iter()
                        .any(|(mode, _)| *mode == OutputPresentationMode::Async),
                ),
                OutputPresentationMode::AdaptiveSync => {
                    (Some(VrrBlocker::ExactKmsQualificationRejected), None)
                }
                OutputPresentationMode::Async => (None, Some(AsyncBlocker::AsyncTestOnlyRejected)),
                OutputPresentationMode::Vsync => (None, None),
            };
            let output_generation = output_transactions
                .transaction(ready_transaction_id)
                .map_or(0, |record| record.descriptor().output_generation());
            log_composited_presentation_rejection(
                kms,
                vrr_policy,
                original_mode,
                presentation_mode,
                vrr_blocker,
                async_blocker,
                output_generation,
                "test_only_rejected",
            );
        }
        if presentation_mode != original_mode {
            output_transactions
                .replace_presentation_state_before_submit(
                    ready_transaction_id,
                    presentation_mode,
                    content_type,
                    presentation_validation_key,
                )
                .map_err(io::Error::other)?;
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
        let frozen_cursor_trace_reveal = frame.frozen_cursor_trace_reveal;
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
            let target = frame
                .bound_target()
                .ok_or_else(|| io::Error::other("unbound output frame cannot be submitted"))?;
            match frame
                .render_fence
                .apply_deadline_hint(target.presentation_time.get(), monotonic_now_ns()?)
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
        let retry_fence = (presentation_mode != OutputPresentationMode::Vsync)
            .then(|| in_fence.try_clone().ok())
            .flatten();
        let mut submit_started_at = MonotonicTimestampNs::new(monotonic_now_ns()?);
        let mut submission = kms.submit_atomic_flip(AtomicFlipRequest {
            framebuffer,
            token,
            in_fence,
            cursor: planned_cursor.clone(),
            presentation_mode,
            content_type,
        });
        let mut submit_returned_at = MonotonicTimestampNs::new(monotonic_now_ns()?);
        if let Err(error) = &submission
            && error.kind == AtomicKmsErrorKind::FlipRejected
            && presentation_mode != OutputPresentationMode::Vsync
        {
            if let Some(key) = presentation_validation_key {
                self.note_composited_presentation_validation(key, false);
            }
            let failed_mode = presentation_mode;
            let planned_fallback_mode = qualified_fallbacks
                .iter()
                .find(|(mode, _)| *mode != failed_mode)
                .map_or(OutputPresentationMode::Vsync, |(mode, _)| *mode);
            let (vrr_blocker, async_blocker) = match failed_mode {
                OutputPresentationMode::AdaptiveAsync => adaptive_async_blockers(
                    qualified_fallbacks
                        .iter()
                        .any(|(mode, _)| *mode == OutputPresentationMode::AdaptiveSync),
                    qualified_fallbacks
                        .iter()
                        .any(|(mode, _)| *mode == OutputPresentationMode::Async),
                ),
                OutputPresentationMode::AdaptiveSync => {
                    (Some(VrrBlocker::ExactKmsQualificationRejected), None)
                }
                OutputPresentationMode::Async => (None, Some(AsyncBlocker::AsyncSubmitRejected)),
                OutputPresentationMode::Vsync => (None, None),
            };
            let output_generation = output_transactions
                .transaction(transaction_id)
                .map_or(0, |record| record.descriptor().output_generation());
            log_composited_presentation_rejection(
                kms,
                vrr_policy,
                failed_mode,
                planned_fallback_mode,
                vrr_blocker,
                async_blocker,
                output_generation,
                "real_submit_rejected",
            );
            for (fallback_mode, fallback_key) in qualified_fallbacks.iter().copied() {
                if fallback_mode == presentation_mode {
                    continue;
                }
                let Some(in_fence) = retry_fence
                    .as_ref()
                    .and_then(|fence| fence.try_clone().ok())
                else {
                    break;
                };
                output_transactions
                    .replace_presentation_state_before_submit(
                        transaction_id,
                        fallback_mode,
                        content_type,
                        fallback_key,
                    )
                    .map_err(io::Error::other)?;
                presentation_mode = fallback_mode;
                presentation_validation_key = fallback_key;
                submit_started_at = MonotonicTimestampNs::new(monotonic_now_ns()?);
                submission = kms.submit_atomic_flip(AtomicFlipRequest {
                    framebuffer,
                    token,
                    in_fence,
                    cursor: planned_cursor.clone(),
                    presentation_mode,
                    content_type,
                });
                submit_returned_at = MonotonicTimestampNs::new(monotonic_now_ns()?);
                if submission.is_ok() {
                    eprintln!(
                        "composited Adaptive presentation real submit rejected; retried mode={}",
                        presentation_mode.as_str(),
                    );
                    break;
                }
                if let Err(fallback_error) = &submission {
                    if let Some(key) = presentation_validation_key {
                        self.note_composited_presentation_validation(key, false);
                    }
                    if fallback_error.kind == AtomicKmsErrorKind::FlipRejected {
                        let (vrr_blocker, async_blocker) = match fallback_mode {
                            OutputPresentationMode::AdaptiveAsync => adaptive_async_blockers(
                                qualified_fallbacks
                                    .iter()
                                    .any(|(mode, _)| *mode == OutputPresentationMode::AdaptiveSync),
                                qualified_fallbacks
                                    .iter()
                                    .any(|(mode, _)| *mode == OutputPresentationMode::Async),
                            ),
                            OutputPresentationMode::AdaptiveSync => {
                                (Some(VrrBlocker::ExactKmsQualificationRejected), None)
                            }
                            OutputPresentationMode::Async => {
                                (None, Some(AsyncBlocker::AsyncSubmitRejected))
                            }
                            OutputPresentationMode::Vsync => (None, None),
                        };
                        let output_generation = output_transactions
                            .transaction(transaction_id)
                            .map_or(0, |record| record.descriptor().output_generation());
                        log_composited_presentation_rejection(
                            kms,
                            vrr_policy,
                            fallback_mode,
                            OutputPresentationMode::Vsync,
                            vrr_blocker,
                            async_blocker,
                            output_generation,
                            "fallback_real_submit_rejected",
                        );
                    }
                    if fallback_error.kind != AtomicKmsErrorKind::FlipRejected {
                        break;
                    }
                }
            }
        }
        match submission {
            Ok(submission) => {
                NativePerfLogger::from_env().log("native.output_presentation_submitted", || {
                    vec![
                        NativePerfField::str("mode", presentation_mode.as_str()),
                        NativePerfField::str("content_type", content_type.as_str()),
                        NativePerfField::u64(
                            "output_generation",
                            output_transactions
                                .transaction(transaction_id)
                                .map_or(0, |record| record.descriptor().output_generation()),
                        ),
                    ]
                });
                if let Some(submitter) = kms.atomic_commit_submitter() {
                    crate::native_output::trace_cursor_kms_submit(
                        submitter.pipeline(),
                        planned_cursor.as_ref().map_or(
                            crate::native_output::CursorKmsAssignment::Disable,
                            crate::native_output::CursorKmsAssignment::Set,
                        ),
                        crate::native_output::CursorKmsSubmitContext {
                            output_id: output_transactions.output_id(),
                            output_generation: output_transactions
                                .transaction(transaction_id)
                                .map_or(0, |transaction| {
                                    transaction.descriptor().output_generation()
                                }),
                            transaction_id: Some(transaction_id),
                            token,
                            crtc_id: submitter.pipeline().crtc.get(),
                            cursor_epoch: frozen_cursor_trace_reveal
                                .and_then(|snapshot| snapshot.expected_epoch),
                            cursor_revision: frozen_cursor_trace_reveal
                                .and_then(|snapshot| snapshot.expected_revision),
                            submission_kind: "primary_plus_cursor",
                            transport: "synchronous",
                            delivery: if planned_cursor.as_ref().is_some_and(|state| state.visible)
                            {
                                crate::native_output::presentation::plane::PresentedCursorDelivery::Hardware
                            } else {
                                crate::native_output::presentation::plane::PresentedCursorDelivery::Hidden
                            },
                        },
                    );
                }
                self.counters.note_atomic_submission(presentation_mode);
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
                if presentation_mode != OutputPresentationMode::Vsync
                    && let Some(key) = presentation_validation_key
                {
                    self.note_composited_presentation_validation(key, false);
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

#[cfg(test)]
mod adaptive_async_blocker_tests {
    use super::*;

    #[test]
    fn adaptive_async_blockers_follow_independent_single_mode_qualification() {
        assert_eq!(
            adaptive_async_blockers(false, true),
            (Some(VrrBlocker::ExactKmsQualificationRejected), None)
        );
        assert_eq!(
            adaptive_async_blockers(true, false),
            (None, Some(AsyncBlocker::AsyncTestOnlyRejected))
        );
        assert_eq!(
            adaptive_async_blockers(false, false),
            (
                Some(VrrBlocker::ExactKmsQualificationRejected),
                Some(AsyncBlocker::AsyncTestOnlyRejected)
            )
        );
        assert_eq!(adaptive_async_blockers(true, true), (None, None));
        assert_eq!(
            adaptive_async_fallback_diagnostics(true, true),
            (
                OutputPresentationMode::AdaptiveSync,
                None,
                None,
                Some("adaptive_async_exact_kms_rejected_as_combination")
            )
        );
        assert_eq!(
            adaptive_async_combination_blocker(OutputPresentationMode::AdaptiveAsync, None, None),
            Some("adaptive_async_exact_kms_rejected_as_combination")
        );
        assert_eq!(
            adaptive_async_combination_blocker(
                OutputPresentationMode::AdaptiveAsync,
                Some(VrrBlocker::ExactKmsQualificationRejected),
                None
            ),
            None
        );
    }

    #[test]
    fn kms_rejection_fallback_order_keeps_the_strongest_qualified_mode() {
        assert_eq!(
            presentation_fallback_candidates(OutputPresentationMode::AdaptiveAsync),
            &[
                OutputPresentationMode::AdaptiveSync,
                OutputPresentationMode::Async,
                OutputPresentationMode::Vsync,
            ]
        );
        assert_eq!(
            strongest_qualified_fallback(
                OutputPresentationMode::AdaptiveAsync,
                &[OutputPresentationMode::Async, OutputPresentationMode::Vsync]
            ),
            OutputPresentationMode::Async
        );
        assert_eq!(
            strongest_qualified_fallback(
                OutputPresentationMode::AdaptiveAsync,
                &[
                    OutputPresentationMode::AdaptiveSync,
                    OutputPresentationMode::Async,
                    OutputPresentationMode::Vsync,
                ]
            ),
            OutputPresentationMode::AdaptiveSync
        );
        assert_eq!(
            strongest_qualified_fallback(
                OutputPresentationMode::AdaptiveSync,
                &[OutputPresentationMode::Vsync]
            ),
            OutputPresentationMode::Vsync
        );
        assert_eq!(
            strongest_qualified_fallback(
                OutputPresentationMode::Async,
                &[OutputPresentationMode::Vsync]
            ),
            OutputPresentationMode::Vsync
        );
    }
}
