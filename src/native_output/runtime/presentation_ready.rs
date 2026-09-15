use super::presentation_cursor::freeze_primary_cursor_presentation;
use super::presentation_transactions::{
    complete_immediate_output_transaction, present_compatibility_frame,
    register_primary_transaction,
};
use super::presentation_worker::{
    promote_immediate_and_publish, queue_compatibility_for_presentation,
    submit_explicit_ready_for_presentation, validation_base_for_submission, worker_ctx,
};
use super::*;
use crate::native_output::kms_worker::KmsCommitWorkerHandle;
use oblivion_one::compositor::FrameCallbackAdmission;
#[cfg(test)]
use oblivion_one::native::kms::KmsBackendKind;
use oblivion_one::native::presentation_deadline::{
    MonotonicTimestampNs, PresentationDeadlinePlanner, ReadyPresentationServiceEstimate,
};

pub(super) enum ReadyPullInResult {
    PulledIn,
    RejectedTooLate,
    RejectedOwned,
    RejectedIdentity,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn pull_ready_frame_into_reachable_opportunity(
    explicit: &mut AtomicEglGbmScanout,
    output_transactions: &mut OutputTransactionLedger,
    presentation_deadline: &PresentationDeadlinePlanner,
    presentation_timing: &KmsPresentationTimingModel,
    frame_pacing: &mut NativeFramePacing,
    now: MonotonicTimestampNs,
    output_generation: u64,
    dispatch_budget_ns: u64,
) -> NativeResult<ReadyPullInResult> {
    pull_ready_frame_into_reachable_opportunity_on_swapchain(
        explicit.swapchain_mut()?,
        output_transactions,
        presentation_deadline,
        presentation_timing,
        frame_pacing,
        now,
        output_generation,
        dispatch_budget_ns,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn pull_ready_frame_into_reachable_opportunity_on_swapchain(
    swapchain: &mut AtomicOutputSwapchain,
    output_transactions: &mut OutputTransactionLedger,
    presentation_deadline: &PresentationDeadlinePlanner,
    presentation_timing: &KmsPresentationTimingModel,
    frame_pacing: &mut NativeFramePacing,
    now: MonotonicTimestampNs,
    output_generation: u64,
    dispatch_budget_ns: u64,
) -> NativeResult<ReadyPullInResult> {
    frame_pacing.note_ready_pull_in_attempt();
    if frame_pacing.ready_predictive_attempt_id().is_some()
        || frame_pacing.ready_worker_submission_reserved()
    {
        return Ok(ReadyPullInResult::RejectedIdentity);
    }

    let Some(ready_identity) = swapchain.ready_identity() else {
        return Ok(ReadyPullInResult::RejectedIdentity);
    };
    let Some(current_target) = ready_identity.target else {
        return Ok(ReadyPullInResult::RejectedIdentity);
    };
    if !current_target.is_binding() {
        return Ok(ReadyPullInResult::RejectedIdentity);
    }
    let Some(record) = output_transactions.transaction(ready_identity.transaction_id) else {
        return Ok(ReadyPullInResult::RejectedIdentity);
    };
    if record.descriptor().output_generation() != output_generation {
        return Ok(ReadyPullInResult::RejectedIdentity);
    }

    let Some(mut frontier) = swapchain.last_presented_primary_claim() else {
        return Ok(ReadyPullInResult::RejectedIdentity);
    };
    if let Some(future) = swapchain.latest_future_primary_target() {
        let future_claim = future.physical_claim();
        if future_claim.sequence <= frontier.sequence
            || future_claim.presentation_time <= frontier.presentation_time
        {
            return Ok(ReadyPullInResult::RejectedIdentity);
        }
        frontier = future_claim;
    }

    let remaining_service = ReadyPresentationServiceEstimate::new(
        dispatch_budget_ns,
        presentation_timing.apply_guard_ns(),
    );
    let Some(pull_in) = presentation_deadline.ready_target_pull_in(
        now,
        remaining_service,
        frontier,
        current_target,
    ) else {
        return Ok(ReadyPullInResult::RejectedTooLate);
    };
    let old_target = pull_in.abandoned_target();
    let new_target = pull_in.replacement_target();
    let submit_window = match presentation_timing.submit_window(
        new_target.presentation_time.get(),
        now.get(),
        dispatch_budget_ns,
    ) {
        Ok(window) => window,
        Err(_) => return Ok(ReadyPullInResult::RejectedTooLate),
    };

    if swapchain
        .validate_ready_target_replacement(ready_identity.transaction_id, old_target, new_target)
        .is_err()
    {
        return Ok(ReadyPullInResult::RejectedOwned);
    }
    if output_transactions
        .replace_ready_target(ready_identity.transaction_id, old_target, new_target)
        .is_err()
    {
        return Ok(ReadyPullInResult::RejectedIdentity);
    }
    if let Err(error) = swapchain.replace_ready_target(
        ready_identity.transaction_id,
        old_target,
        new_target,
        submit_window,
    ) {
        let _ = output_transactions.replace_ready_target(
            ready_identity.transaction_id,
            new_target,
            old_target,
        );
        return Err(error.into());
    }
    frame_pacing.note_ready_pull_in_success(old_target, new_target);
    Ok(ReadyPullInResult::PulledIn)
}

pub(super) fn ensure_async_render_fence_ready(
    explicit: &AtomicEglGbmScanout,
    output_transactions: &OutputTransactionLedger,
    transaction_id: OutputTransactionId,
    output_render_fence_token: &mut Option<ReactorToken>,
    event_loop: &mut NativeEventLoop,
) -> NativeResult<bool> {
    let is_async = output_transactions
        .transaction(transaction_id)
        .is_some_and(|transaction| transaction.descriptor().presentation_mode().is_async());
    if !is_async || explicit.ready_render_fence_is_signaled()? {
        return Ok(true);
    }
    if output_render_fence_token.is_none()
        && let Some(fd) = explicit.ready_render_fence_fd()
    {
        *output_render_fence_token =
            Some(event_loop.register(fd, NativeEventSource::OutputRenderFence)?);
    }
    Ok(false)
}

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
    compatibility_submit_window: Option<KmsSubmitWindow>,
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
    cursor_reveal_trace: &mut Option<CursorRevealTraceLedger>,
    pacing_mode: NativeOutputPacingMode,
    presented_planes: crate::native_output::presentation::plane::PresentedPlaneSnapshot,
    scene_history: &mut NativeSceneHistory,
    frame_index: &mut u64,
    frame_submitted: &mut bool,
    perf: NativePerfLogger,
    #[cfg(test)] native_io_recorder: &mut NativeIoRecorder,
) -> NativeResult<ReadySubmissionResult> {
    let repaint_present_start = Instant::now();
    let explicit_submission = matches!(scanout, NativeScanoutBackend::AtomicEglGbm(_));
    let guard = if let NativeScanoutBackend::AtomicEglGbm(explicit) = scanout {
        explicit.swapchain()?.ready_identity().and_then(|identity| {
            identity
                .target
                .map(|target| (identity.protocol_batch_id, target))
        })
    } else {
        server.prepared_frame_batch_id().zip(compatibility_target)
    };
    let callback_batch_id = guard.map(|(batch_id, _)| batch_id);
    let Some(validation_base) = validation_base_for_submission(
        worker,
        output_transactions.output_id(),
        presented_planes,
        output_generation,
        crtc_id,
    ) else {
        if let Some(batch_id) = callback_batch_id {
            server.note_frame_callback_admission_failure(batch_id);
        }
        return Ok(ReadySubmissionResult::Unavailable);
    };
    let primary_cursor_presentation = freeze_primary_cursor_presentation(
        presented_planes.cursor.delivery,
        cursor_delivery,
        cursor,
        atomic_cursor.as_ref(),
        cursor_epoch,
    );
    if let Some((batch_id, guard_target)) = guard
        && !server.commit_timing_submission_is_safe_for_batch(
            batch_id,
            guard_target.presentation_time,
            guard_target.clock_generation,
        )
    {
        server.note_frame_callback_admission_failure(batch_id);
        return Ok(ReadySubmissionResult::Unavailable);
    }
    let (present_result, compatibility_transaction_id) =
        if let NativeScanoutBackend::AtomicEglGbm(explicit) = scanout {
            let transaction_id = explicit
                .swapchain()?
                .ready_transaction_id()
                .ok_or_else(|| io::Error::other("ready explicit frame has no transaction ID"))?;
            let presentation_mode = output_transactions
                .transaction(transaction_id)
                .ok_or_else(|| {
                    io::Error::other("ready transaction disappeared before fence check")
                })?
                .descriptor()
                .presentation_mode();
            if presentation_mode.is_async()
                && !ensure_async_render_fence_ready(
                    explicit,
                    output_transactions,
                    transaction_id,
                    output_render_fence_token,
                    event_loop,
                )?
            {
                if let Some(batch_id) = callback_batch_id {
                    server.note_frame_callback_admission_failure(batch_id);
                }
                return Ok(ReadySubmissionResult::Unavailable);
            }
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
                    cursor_reveal_trace,
                )?
            else {
                if let Some(batch_id) = callback_batch_id {
                    server.note_frame_callback_admission_failure(batch_id);
                }
                return Ok(ReadySubmissionResult::Unavailable);
            };
            // The explicit Atomic path owns the KMS presentation token. Move
            // the exact rendered scene from ready to submitted only after the
            // backend has accepted that token; pageflip-time transition
            // preparation is keyed by this same token.
            scene_history.queue_submission_or_error(token)?;
            server.trace_surface_pipeline_active_surfaces(
                oblivion_one::compositor::SurfacePipelineEvent::OutputFrameSubmitted,
                callback_batch_id.map(|batch_id| batch_id.get()),
                Some(transaction_id.get()),
                scene_history.submitted_frame_id(token),
                Some(token),
            );
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
            let pacing_ticket = frame_pacing
                .reserve_worker_submission(true)
                .map_err(io::Error::other)?;
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
            let Some(compatibility_submit_window) = compatibility_submit_window else {
                if let Some(batch_id) = callback_batch_id {
                    server.note_frame_callback_admission_failure(batch_id);
                }
                if pacing_ticket.is_some() && !frame_pacing.cancel_worker_submission(pacing_ticket)
                {
                    return Err(io::Error::other(
                        "missing compatibility submit window left worker pacing reservation",
                    )
                    .into());
                }
                return Ok(ReadySubmissionResult::Unavailable);
            };
            let result = match queue_compatibility_for_presentation(
                worker.ok_or_else(|| io::Error::other("worker transport has no worker"))?,
                scanout,
                kms_backend,
                server,
                output_transactions,
                atomic_commit_arbiter,
                presentation_trace,
                output_generation,
                crtc_id,
                compatibility_target,
                compatibility_submit_window,
                pacing_mode,
                render_generation,
                cursor,
                atomic_cursor
                    .as_ref()
                    .map(super::presentation_cursor::cursor_source_for_trace),
                atomic_cursor
                    .as_ref()
                    .and_then(|cursor_state| cursor.map(|_| cursor_state.desired_revision())),
                cursor_delivery,
                primary_cursor_presentation,
                cursor_pin,
                atomic_cursor.as_ref().and_then(|native_cursor| {
                    cursor.and_then(|state| native_cursor.capability_key_for(state))
                }),
                pacing_ticket,
                crate::native_output::kms_worker::KmsCommitTestPolicy::from_cursor(test_only),
                cursor_epoch,
                validation_base,
            ) {
                Ok(result) => result,
                Err(error) => {
                    scene_history.discard_ready();
                    if pacing_ticket.is_some()
                        && !frame_pacing.cancel_worker_submission(pacing_ticket)
                    {
                        return Err(io::Error::other(
                            "failed compatibility worker pacing identity mismatch",
                        )
                        .into());
                    }
                    return Err(error);
                }
            };
            let Some(result) = result else {
                if let Some(batch_id) = callback_batch_id {
                    server.note_frame_callback_admission_failure(batch_id);
                }
                if pacing_ticket.is_some() && !frame_pacing.cancel_worker_submission(pacing_ticket)
                {
                    return Err(io::Error::other(
                        "unavailable compatibility worker pacing identity mismatch",
                    )
                    .into());
                }
                return Ok(ReadySubmissionResult::Unavailable);
            };
            result
        } else {
            let compatibility_target = compatibility_target.ok_or_else(|| {
                io::Error::other("compatibility pageflip started without a target")
            })?;
            let result = present_compatibility_frame(
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
                Some(kms_backend),
                |scanout, presentation_mode| {
                    scanout.present(kms_backend, cursor, presentation_mode)
                },
            );
            match result {
                Ok(result) => result,
                Err(error) => {
                    scene_history.discard_ready();
                    return Err(error);
                }
            }
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
            if !explicit_submission && !scene_history.queue_submission(token) {
                scene_history.discard_ready();
                return Err(io::Error::other(
                    "compatibility submission has no rendered scene snapshot",
                )
                .into());
            }
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
            if let Some(batch_id) = callback_batch_id {
                server.complete_frame_callbacks_after_admission(
                    batch_id,
                    FrameCallbackAdmission::Ready,
                );
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
            if !promote_immediate_and_publish(scene_history, server) {
                return Err(io::Error::other(
                    "immediate compatibility presentation has no rendered scene snapshot",
                )
                .into());
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::egl_renderer::{EglSceneFrameCommit, native_fence::NativeRenderFence};
    use crate::native_output::presentation::plane::{
        FrozenCursorTestPolicy, FrozenPrimaryCursorPlan, FrozenPrimaryCursorPresentation,
        PresentedCursorDelivery,
    };
    use drm_sys::drm_mode_modeinfo;
    use oblivion_one::compositor::{CompositorFrameBatchId, SurfaceDamagePresentation};
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::time::Duration;

    fn test_render_fence() -> NativeRenderFence {
        let mut pipe = [-1; 2];
        assert_eq!(
            unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) },
            0
        );
        unsafe { libc::close(pipe[1]) };
        NativeRenderFence::from_submission_fd(unsafe { OwnedFd::from_raw_fd(pipe[0]) })
    }

    fn test_mode() -> drm_mode_modeinfo {
        drm_mode_modeinfo {
            clock: 325_000,
            hdisplay: 1920,
            hsync_start: 2008,
            hsync_end: 2052,
            htotal: 2080,
            vdisplay: 1080,
            vsync_start: 1084,
            vsync_end: 1089,
            vtotal: 1111,
            ..drm_mode_modeinfo::default()
        }
    }

    #[test]
    fn lane_free_ready_pull_in_precedes_worker_reservation() {
        let render_ahead = false;
        let atomic_commit_pending = false;
        let can_queue_worker_next = true;
        assert!(
            !oblivion_one::native::scheduler::rendered_primary_must_wait_for_lane(
                render_ahead,
                atomic_commit_pending,
                can_queue_worker_next,
            )
        );

        let refresh = Duration::from_nanos(6_060_606);
        let presented_at = MonotonicTimestampNs::new(1_000_000_000);
        let mut presentation_deadline = PresentationDeadlinePlanner::new(refresh);
        presentation_deadline.note_presented(presented_at);
        let old_target = presentation_deadline
            .plan_normal(
                MonotonicTimestampNs::new(1_001_000_000),
                Duration::from_nanos(5_807_000),
            )
            .expect("conservative pre-render target");
        assert_eq!(old_target.sequence, 3);

        let presentation_timing = KmsPresentationTimingModel::new(
            KmsModeTiming::from_mode(&test_mode(), refresh.as_nanos() as u64),
            1,
        );
        let mut swapchain = AtomicOutputSwapchain::from_presented_slots(
            OutputSlotSet::new([
                OutputSlotId::new(0).unwrap(),
                OutputSlotId::new(1).unwrap(),
                OutputSlotId::new(2).unwrap(),
            ])
            .unwrap(),
            OutputSlotId::new(0).unwrap(),
            1,
        )
        .unwrap();
        let frontier = oblivion_one::native::presentation_deadline::PrimaryRefreshClaim {
            sequence: 1,
            presentation_time: presented_at,
            clock_generation: 1,
        };
        swapchain
            .note_physical_primary_presentation(frontier)
            .unwrap();

        let slot = swapchain.acquire_render_slot().unwrap();
        let frame_id = swapchain.next_frame_id();
        let transaction_id = OutputTransactionId::new(
            std::num::NonZeroU64::new(frame_id).expect("test transaction ID is nonzero"),
        );
        let protocol_batch_id = CompositorFrameBatchId::new(
            std::num::NonZeroU64::new(frame_id).expect("test batch ID is nonzero"),
        );
        let frame_now = MonotonicTimestampNs::new(frame_id);
        swapchain
            .finish_render_owned(RenderedOutputFrame {
                output_id: swapchain.output_id(),
                id: frame_id,
                transaction_id,
                slot,
                framebuffer_id: FramebufferId::new(42).unwrap(),
                render_generation: 1,
                pool_generation: 1,
                reservation: FramePresentationReservation::Bound(old_target),
                submit_window: KmsSubmitWindow::try_new(
                    old_target.presentation_time.get(),
                    old_target.submit_not_before().get(),
                    0,
                    0,
                )
                .unwrap(),
                render_fence: test_render_fence(),
                fence_timing_evidence: None,
                scene_commit: EglSceneFrameCommit::empty_for_test(),
                surface_damage: SurfaceDamagePresentation::default(),
                protocol_batch_id,
                composite_started_at: frame_now,
                fence_exported_at: frame_now,
                rendered_at: frame_now,
                client_commit_ns: None,
                callback_reaction_ns: None,
                callback_admission_ns: None,
                callback_surface_id: None,
                hardware_cursor_surface_id: None,
                cpu_prepass_duration_ns: 0,
                cpu_encode_duration_ns: 0,
                frozen_cursor_plan: FrozenPrimaryCursorPlan {
                    delivery: PresentedCursorDelivery::Hidden,
                    primary_presentation: FrozenPrimaryCursorPresentation::Preserve,
                    cursor_test_policy: FrozenCursorTestPolicy::Skip,
                },
                frozen_cursor_plane_owner: None,
                frozen_cursor_trace_reveal: None,
                o1_admission: None,
            })
            .unwrap();

        let transaction = OutputTransaction::composited(
            transaction_id,
            1,
            frame_now,
            old_target,
            NativeOutputPacingMode::PredictiveTriple,
            frame_id,
            1,
            1,
            slot,
            42,
            None,
            protocol_batch_id,
        )
        .unwrap();
        let mut output_transactions = OutputTransactionLedger::with_capacities(8, 8);
        output_transactions.insert(transaction).unwrap();
        output_transactions
            .mark_ready(transaction_id, frame_now)
            .unwrap();
        assert_eq!(
            swapchain
                .ready_identity()
                .and_then(|identity| identity.target),
            Some(old_target)
        );

        let mut frame_pacing = NativeFramePacing::from_env();
        frame_pacing.queue_visual(frame_id, 1);
        frame_pacing
            .note_render_started(NativeOutputPacingMode::PredictiveTriple, false)
            .unwrap();

        let pull_in = pull_ready_frame_into_reachable_opportunity_on_swapchain(
            &mut swapchain,
            &mut output_transactions,
            &presentation_deadline,
            &presentation_timing,
            &mut frame_pacing,
            MonotonicTimestampNs::new(1_005_500_000),
            1,
            300_000,
        )
        .unwrap();
        assert!(matches!(pull_in, ReadyPullInResult::PulledIn));

        let expected_target = presentation_deadline
            .ready_target_pull_in(
                MonotonicTimestampNs::new(1_005_500_000),
                ReadyPresentationServiceEstimate::new(
                    300_000,
                    presentation_timing.apply_guard_ns(),
                ),
                frontier,
                old_target,
            )
            .unwrap()
            .replacement_target();
        assert_eq!(
            swapchain
                .ready_identity()
                .and_then(|identity| identity.target),
            Some(expected_target)
        );
        assert_eq!(
            output_transactions
                .transaction(transaction_id)
                .unwrap()
                .descriptor()
                .bound_target(),
            Some(expected_target)
        );
        let ready_window = swapchain.ready_submit_window().expect("N+1 ready window");
        assert_eq!(
            ready_window.target_presentation_ns(),
            expected_target.presentation_time.get()
        );
        assert_eq!(ready_window.earliest_submit_ns(), 1_005_500_000);

        // This is the lane-free handoff: the active pacing identity is
        // reserved only after the READY target has been replaced.
        let ticket = frame_pacing
            .reserve_worker_submission(false)
            .unwrap()
            .expect("lane-free frame reaches worker reservation");
        assert_eq!(ticket.frame_id().get(), frame_id);
        assert!(!ticket.ready_submit());
        assert_eq!(
            output_transactions
                .transaction(transaction_id)
                .unwrap()
                .descriptor()
                .bound_target(),
            Some(expected_target)
        );
    }
}
