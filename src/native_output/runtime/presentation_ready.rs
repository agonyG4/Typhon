use super::presentation_cursor::freeze_primary_cursor_presentation;
use super::presentation_transactions::{
    complete_immediate_output_transaction, present_compatibility_frame,
    register_primary_transaction,
};
use super::presentation_worker::{
    queue_compatibility_for_presentation, submit_explicit_ready_for_presentation,
    validation_base_for_submission, worker_ctx,
};
use super::*;
use crate::native_output::kms_worker::KmsCommitWorkerHandle;
#[cfg(test)]
use oblivion_one::native::kms::KmsBackendKind;

pub(super) enum ReadySubmissionResult {
    Submitted,
    Unavailable,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn submit_ready_frame(
    scheduler_decision: SchedulerDecision,
    worker_mode: bool,
    worker: Option<&KmsCommitWorkerHandle>,
    server: &mut OwnCompositorServer,
    kms_backend: &KmsBackendSelection,
    scanout: &mut NativeScanoutBackend,
    crtc_id: u32,
    output_generation: u64,
    mode_label: &str,
    refresh_hz: u32,
    compatibility_target: Option<PresentationTarget>,
    render_generation: u64,
    cursor: Option<&AtomicCursorVisualState>,
    cursor_epoch: u64,
    cursor_render_mode: NativeCursorRenderMode,
    cursor_delivery: crate::native_output::presentation::plane::PresentedCursorDelivery,
    atomic_cursor: &mut Option<NativeAtomicCursor>,
    cursor_output_arbitration: &mut NativeCursorOutputArbitration,
    last_submitted_cursor_epoch: &mut u64,
    frame_scheduler: &mut NativeFrameScheduler,
    frame_pacing: &mut NativeFramePacing,
    output_render_fence_token: &mut Option<ReactorToken>,
    event_loop: &mut NativeEventLoop,
    atomic_commit_arbiter: &mut AtomicCommitArbiter,
    output_transactions: &mut OutputTransactionLedger,
    presentation_trace: &mut PresentationTransactionTraceRing,
    pacing_mode: NativeOutputPacingMode,
    presented_planes: crate::native_output::presentation::plane::PresentedPlaneSnapshot,
    frame_index: &mut u64,
    frame_submitted: &mut bool,
    perf: NativePerfLogger,
    #[cfg(test)] native_io_recorder: &mut NativeIoRecorder,
) -> NativeResult<ReadySubmissionResult> {
    let repaint_present_start = Instant::now();
    let Some(validation_base) =
        validation_base_for_submission(worker, presented_planes, output_generation, crtc_id)
    else {
        return Ok(ReadySubmissionResult::Unavailable);
    };
    let primary_cursor_presentation = freeze_primary_cursor_presentation(
        presented_planes.cursor.delivery,
        cursor_delivery,
        cursor,
        atomic_cursor.as_ref(),
        cursor_epoch,
    );
    let explicit_submission = matches!(scanout, NativeScanoutBackend::AtomicEglGbm(_));
    let (present_result, compatibility_transaction_id) =
        if let NativeScanoutBackend::AtomicEglGbm(explicit) = scanout {
            let transaction_id = explicit
                .swapchain()?
                .ready_transaction_id()
                .ok_or_else(|| io::Error::other("ready explicit frame has no transaction ID"))?;
            let Some((token, framebuffer_id, transaction_id, _worker_queued)) =
                submit_explicit_ready_for_presentation(
                    worker_mode,
                    worker,
                    explicit,
                    kms_backend,
                    server,
                    output_transactions,
                    atomic_commit_arbiter,
                    presentation_trace,
                    transaction_id,
                    output_generation,
                    crtc_id,
                    worker_ctx(
                        atomic_cursor.as_ref(),
                        frame_pacing,
                        validation_base,
                        cursor_delivery,
                        primary_cursor_presentation,
                    ),
                    true,
                )?
            else {
                return Ok(ReadySubmissionResult::Unavailable);
            };
            explicit.mark_composited_submission();
            (
                NativePresentResult::AsyncSubmitted {
                    token,
                    framebuffer_id,
                    transaction_id: Some(transaction_id),
                },
                None,
            )
        } else if worker_mode
            && matches!(
                scanout,
                NativeScanoutBackend::NativeEglGbm(_) | NativeScanoutBackend::Gbm(_)
            )
        {
            let compatibility_target = compatibility_target
                .ok_or_else(|| io::Error::other("compatibility worker submission has no target"))?;
            let cursor_pin = match (atomic_cursor.as_ref(), cursor) {
                (Some(native_cursor), Some(state)) if state.framebuffer_id.is_some() => {
                    Some(native_cursor.pin_framebuffer_for(state)?)
                }
                _ => None,
            };
            let pacing_frame_id = frame_pacing.worker_submission_frame_id(true);
            let test_only = atomic_cursor.as_ref().map_or(
                crate::native_output::kms_worker::KmsTestOnlyPolicy::Skip,
                |cursor| match cursor.scheduled_test_policy() {
                    KmsCursorTestPolicy::Required => {
                        crate::native_output::kms_worker::KmsTestOnlyPolicy::Required
                    }
                    KmsCursorTestPolicy::NotApplicable | KmsCursorTestPolicy::SkipProven => {
                        crate::native_output::kms_worker::KmsTestOnlyPolicy::Skip
                    }
                },
            );
            let Some(result) = queue_compatibility_for_presentation(
                worker.ok_or_else(|| io::Error::other("worker transport has no worker"))?,
                scanout,
                server,
                output_transactions,
                atomic_commit_arbiter,
                presentation_trace,
                output_generation,
                crtc_id,
                compatibility_target,
                pacing_mode,
                render_generation,
                cursor,
                cursor_delivery,
                primary_cursor_presentation,
                cursor_pin,
                atomic_cursor.as_ref().and_then(|native_cursor| {
                    cursor.and_then(|state| native_cursor.capability_key_for(state))
                }),
                pacing_frame_id,
                crate::native_output::kms_worker::KmsCommitTestPolicy::from_cursor(test_only),
                cursor_epoch,
                validation_base,
            )?
            else {
                return Ok(ReadySubmissionResult::Unavailable);
            };
            result
        } else {
            let compatibility_target = compatibility_target.ok_or_else(|| {
                io::Error::other("compatibility pageflip started without a target")
            })?;
            present_compatibility_frame(
                scanout,
                server,
                output_transactions,
                output_generation,
                crtc_id,
                compatibility_target,
                pacing_mode,
                render_generation,
                cursor,
                cursor_epoch,
                *frame_index,
                |scanout| scanout.present(kms_backend, cursor),
            )?
        };
    #[cfg(test)]
    native_io_recorder.record(NativeIoOperation::ScanoutPresent);
    let repaint_present_us = elapsed_micros(repaint_present_start);
    match present_result {
        NativePresentResult::AsyncSubmitted {
            token,
            framebuffer_id,
            transaction_id,
        } => {
            let atomic_primary_registered = if worker_mode {
                true
            } else {
                register_primary_transaction(
                    atomic_commit_arbiter,
                    server,
                    kms_backend.effective_kind(),
                    token,
                    output_generation,
                    crtc_id,
                    transaction_id,
                    *frame_index,
                    framebuffer_id,
                    monotonic_now_ns()?,
                    output_transactions,
                    presentation_trace,
                )?
            };
            if !worker_mode
                && let Some(cursor_state) = atomic_cursor.as_mut()
                && cursor_state.needs_submission_for(cursor)
                && let Some(cursor_token) = PageFlipToken::new(token)
            {
                let state = cursor.cloned().unwrap_or_else(|| {
                    let mut hidden = cursor_state.desired().clone();
                    hidden.visible = false;
                    hidden.framebuffer_id = None;
                    hidden
                });
                cursor_state.begin_primary_submission(cursor_token, state);
            }
            if !worker_mode {
                *last_submitted_cursor_epoch = cursor_epoch;
                cursor_output_arbitration.consume(cursor_epoch);
            }
            if !explicit_submission {
                server.mark_prepared_frame_submitted();
            }
            #[cfg(test)]
            native_io_recorder.record(NativeIoOperation::PageflipSubmit);
            #[cfg(test)]
            native_io_recorder.record(match kms_backend.effective_kind() {
                KmsBackendKind::Atomic => NativeIoOperation::AtomicCommit,
                KmsBackendKind::Legacy => NativeIoOperation::LegacyCommit,
            });
            if worker_mode && !explicit_submission {
                let transaction_id = transaction_id.ok_or_else(|| {
                    io::Error::other("worker Atomic submission has no transaction ID")
                })?;
                frame_scheduler
                    .reserve_worker_submission(token, transaction_id.get())
                    .map_err(io::Error::other)?;
            } else if !worker_mode && !explicit_submission {
                frame_scheduler
                    .note_ready_submission(token, monotonic_now_ns()?)
                    .map_err(io::Error::other)?;
                if atomic_primary_registered {
                    frame_scheduler.defer_page_flip_watchdog_to_atomic_arbiter();
                }
            }
            if !worker_mode {
                frame_pacing.note_submit(token, monotonic_now_ns()?, true, pacing_mode);
            }
            if explicit_submission
                && !worker_mode
                && output_render_fence_token.is_none()
                && let NativeScanoutBackend::AtomicEglGbm(explicit) = &*scanout
                && let Some(fd) = explicit.pending_timing_fd()
            {
                *output_render_fence_token =
                    Some(event_loop.register(fd, NativeEventSource::OutputRenderFence)?);
            }
            *frame_submitted = true;
            if !explicit_submission {
                server.mark_render_damage_presented();
            }
            *frame_index = frame_index.saturating_add(1);
            perf.log("native.frame", || {
                vec![
                    NativePerfField::u64("index", *frame_index),
                    NativePerfField::str("phase", "ready-submit"),
                    NativePerfField::str("mode", mode_label.to_owned()),
                    NativePerfField::str("cursor", cursor_render_mode.as_str()),
                    NativePerfField::u64("refresh_hz", u64::from(refresh_hz)),
                    NativePerfField::u64("repaint_present_us", repaint_present_us),
                    NativePerfField::u64("pageflip_token", token),
                    NativePerfField::bool(
                        "render_ahead_ready",
                        scheduler_decision == SchedulerDecision::SubmitReady,
                    ),
                ]
            });
        }
        NativePresentResult::Immediate => {
            let transaction_id = compatibility_transaction_id.ok_or_else(|| {
                io::Error::other("immediate compatibility presentation has no transaction")
            })?;
            complete_immediate_output_transaction(
                output_transactions,
                presentation_trace,
                server,
                transaction_id,
                MonotonicTimestampNs::new(monotonic_now_ns()?),
            )?;
            frame_scheduler.note_immediate_completion();
        }
        NativePresentResult::Noop => {
            debug_assert!(compatibility_transaction_id.is_none());
            perf.log("native.frame_skip", || {
                vec![
                    NativePerfField::str("reason", "ready_submit_without_ready_frame"),
                    NativePerfField::bool("scanout_ready", scanout.ready_frame_queued()),
                ]
            });
            frame_scheduler.note_immediate_completion();
        }
    }
    Ok(ReadySubmissionResult::Submitted)
}
