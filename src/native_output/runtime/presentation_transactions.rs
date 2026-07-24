use super::cursor_cycle::defer_cursor_after_busy;
use super::*;
use oblivion_one::native::kms::KmsBackendKind;

#[allow(clippy::too_many_arguments)]
pub(super) fn build_compatibility_transaction(
    output_transactions: &mut OutputTransactionLedger,
    server: &OwnCompositorServer,
    scanout: &NativeScanoutBackend,
    output_generation: u64,
    target: PresentationTarget,
    pacing_mode: NativeOutputPacingMode,
    render_generation: u64,
    cursor: Option<&AtomicCursorVisualState>,
    cursor_epoch: u64,
) -> NativeResult<Option<OutputTransactionId>> {
    let Some(framebuffer_id) = scanout.compatibility_framebuffer_id() else {
        return Ok(None);
    };
    let frame_batch_id = server
        .prepared_frame_batch_id()
        .ok_or_else(|| io::Error::other("compatibility pageflip has no prepared frame batch"))?;
    let frame_id = server
        .prepared_frame_id()
        .ok_or_else(|| io::Error::other("compatibility pageflip has no prepared frame ID"))?;
    let transaction_id = output_transactions
        .allocate_id()
        .map_err(io::Error::other)?;
    let transaction = OutputTransaction::compatibility_composited(
        transaction_id,
        output_generation,
        MonotonicTimestampNs::new(monotonic_now_ns()?),
        target,
        pacing_mode,
        frame_id,
        render_generation,
        framebuffer_id,
        cursor.map(|state| CursorPlaneAssignment::Atomic {
            desired_epoch: cursor_epoch,
            framebuffer_id: state.framebuffer_id,
            visible: state.visible,
        }),
        frame_batch_id,
    )
    .map_err(io::Error::other)?;
    output_transactions
        .insert(transaction)
        .map_err(io::Error::other)?;
    Ok(Some(transaction_id))
}

pub(super) fn fail_transaction(
    output_transactions: &mut OutputTransactionLedger,
    transaction_id: Option<OutputTransactionId>,
    stage: OutputTransactionFailureStage,
) -> NativeResult<()> {
    if let Some(transaction_id) = transaction_id {
        output_transactions
            .mark_failed(
                transaction_id,
                stage,
                MonotonicTimestampNs::new(monotonic_now_ns()?),
            )
            .map_err(io::Error::other)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn present_compatibility_frame(
    scanout: &mut NativeScanoutBackend,
    kms_backend: &KmsBackendSelection,
    server: &OwnCompositorServer,
    output_transactions: &mut OutputTransactionLedger,
    output_generation: u64,
    crtc_id: u32,
    target: PresentationTarget,
    pacing_mode: NativeOutputPacingMode,
    render_generation: u64,
    cursor: Option<&AtomicCursorVisualState>,
    cursor_epoch: u64,
    frame_index: u64,
) -> NativeResult<(NativePresentResult, Option<OutputTransactionId>)> {
    let transaction_id = build_compatibility_transaction(
        output_transactions,
        server,
        scanout,
        output_generation,
        target,
        pacing_mode,
        render_generation,
        cursor,
        cursor_epoch,
    )?;
    let result = scanout.present(kms_backend, cursor).map_err(|error| {
        native_runtime_error(
            NativeRuntimeStage::Present,
            scanout.kind(),
            crtc_id,
            frame_index,
            error,
        )
    });
    match result {
        Ok(NativePresentResult::AsyncSubmitted {
            token,
            framebuffer_id,
            ..
        }) => Ok((
            NativePresentResult::AsyncSubmitted {
                token,
                framebuffer_id,
                transaction_id,
            },
            transaction_id,
        )),
        Ok(result) => {
            fail_transaction(
                output_transactions,
                transaction_id,
                OutputTransactionFailureStage::KmsSubmit,
            )?;
            Ok((result, transaction_id))
        }
        Err(error) => {
            fail_transaction(
                output_transactions,
                transaction_id,
                OutputTransactionFailureStage::KmsSubmit,
            )?;
            Err(Box::new(error))
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn register_primary_transaction(
    atomic_commit_arbiter: &mut AtomicCommitArbiter,
    kms_kind: KmsBackendKind,
    token: u64,
    generation: u64,
    crtc_id: u32,
    transaction_id: Option<OutputTransactionId>,
    frame_index: u64,
    framebuffer_id: u32,
    submitted_at_ns: u64,
    output_transactions: &mut OutputTransactionLedger,
    presentation_trace: &mut PresentationTransactionTraceRing,
) -> NativeResult<bool> {
    let transaction_frame_id = transaction_id
        .and_then(|id| output_transactions.transaction(id))
        .and_then(|record| match record.descriptor().content() {
            OutputTransactionContent::Composited { frame_id, .. }
            | OutputTransactionContent::Direct { frame_id, .. } => Some(frame_id),
            OutputTransactionContent::CursorOnly { .. } => None,
        })
        .unwrap_or(frame_index);
    let registered = match register_atomic_primary_submission(
        atomic_commit_arbiter,
        kms_kind,
        token,
        generation,
        crtc_id,
        transaction_id,
        transaction_frame_id,
        framebuffer_id,
        submitted_at_ns,
    ) {
        Ok(registered) => registered,
        Err(error) => {
            fail_transaction(
                output_transactions,
                transaction_id,
                OutputTransactionFailureStage::OutputTeardown,
            )?;
            return Err(error.into());
        }
    };
    if let Some(transaction_id) = transaction_id {
        if !registered
            || matches!(
                output_transactions.transaction(transaction_id),
                Some(record) if matches!(record.state(), OutputTransactionState::Built)
            )
        {
            output_transactions
                .mark_submitted(
                    transaction_id,
                    PageFlipToken::new(token)
                        .ok_or_else(|| io::Error::other("pageflip token is zero"))?,
                    MonotonicTimestampNs::new(submitted_at_ns),
                )
                .map_err(io::Error::other)?;
        }
        presentation_trace.push(PresentationTransactionEvent::KmsSubmitReturned {
            transaction_id,
            timestamp_ns: submitted_at_ns,
        });
    }
    Ok(registered)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_cursor_transaction(
    output_transactions: &mut OutputTransactionLedger,
    presentation_trace: &mut PresentationTransactionTraceRing,
    output_generation: u64,
    target: PresentationTarget,
    pacing_mode: NativeOutputPacingMode,
    cursor_epoch: u64,
    desired: Option<&AtomicCursorVisualState>,
) -> NativeResult<OutputTransactionId> {
    let transaction_id = output_transactions
        .allocate_id()
        .map_err(io::Error::other)?;
    let transaction = OutputTransaction::cursor_only(
        transaction_id,
        output_generation,
        MonotonicTimestampNs::new(monotonic_now_ns()?),
        target,
        pacing_mode,
        cursor_epoch,
        desired.and_then(|state| state.framebuffer_id),
        desired.is_some_and(|state| state.visible),
        OutputReleasePlan::Pageflip,
    )
    .map_err(io::Error::other)?;
    output_transactions
        .insert(transaction)
        .map_err(io::Error::other)?;
    presentation_trace.push(PresentationTransactionEvent::TransactionBuilt {
        transaction_id,
        timestamp_ns: monotonic_now_ns()?,
    });
    Ok(transaction_id)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn submit_cursor_only(
    kms_backend: &KmsBackendSelection,
    cursor: &mut NativeAtomicCursor,
    desired: Option<AtomicCursorVisualState>,
    atomic_commit_arbiter: &mut AtomicCommitArbiter,
    output_transactions: &mut OutputTransactionLedger,
    presentation_trace: &mut PresentationTransactionTraceRing,
    target: PresentationTarget,
    crtc_id: u32,
    output_generation: u64,
    pacing_mode: NativeOutputPacingMode,
    cursor_epoch: u64,
    cursor_output_arbitration: &mut NativeCursorOutputArbitration,
    frame_scheduler: &mut NativeFrameScheduler,
    pacing_now_ns: u64,
    perf: NativePerfLogger,
    client_cursor_active: bool,
    cursor_render_mode: &mut NativeCursorRenderMode,
    effective_cursor: &mut Option<AtomicCursorVisualState>,
    queued_redraw_requested: &mut bool,
    last_client_cursor_damage: &mut Option<NativeClientCursorDamageState>,
    last_software_cursor_damage: &mut Option<NativeDamageRect>,
    current_client_cursor_damage: Option<NativeClientCursorDamageState>,
    current_software_cursor_damage: Option<NativeDamageRect>,
) -> NativeResult<SchedulerDecision> {
    match kms_backend.test_atomic_cursor_flip(desired.as_ref()) {
        Ok(()) => {
            let transaction_id = build_cursor_transaction(
                output_transactions,
                presentation_trace,
                output_generation,
                target,
                pacing_mode,
                cursor_epoch,
                desired.as_ref(),
            )?;
            let token = PageFlipToken::new(allocate_native_page_flip_token())
                .expect("allocated native pageflip token is nonzero");
            if let Err(error) = atomic_commit_arbiter.reserve(
                token,
                output_generation,
                crtc_id,
                AtomicCommitKind::CursorOnly {
                    transaction_id,
                    cursor_epoch,
                    framebuffer_id: desired.as_ref().and_then(|state| state.framebuffer_id),
                },
                monotonic_now_ns()?,
            ) {
                fail_transaction(
                    output_transactions,
                    Some(transaction_id),
                    OutputTransactionFailureStage::KmsSubmit,
                )?;
                return Err(Box::new(io::Error::other(error)));
            }
            match kms_backend.submit_cursor_flip(desired.as_ref(), token) {
                Ok(()) => {
                    output_transactions
                        .mark_submitted(
                            transaction_id,
                            token,
                            MonotonicTimestampNs::new(monotonic_now_ns()?),
                        )
                        .map_err(io::Error::other)?;
                    presentation_trace.push(PresentationTransactionEvent::KmsSubmitReturned {
                        transaction_id,
                        timestamp_ns: monotonic_now_ns()?,
                    });
                    let submitted_state = desired.unwrap_or_else(|| {
                        let mut hidden = cursor.desired().clone();
                        hidden.visible = false;
                        hidden.framebuffer_id = None;
                        hidden
                    });
                    let submitted_state = cursor.begin_submission(token, submitted_state);
                    cursor_output_arbitration.note_cursor_only_submission();
                    *last_client_cursor_damage = current_client_cursor_damage;
                    *last_software_cursor_damage = current_software_cursor_damage;
                    cursor_output_arbitration.consume(cursor_epoch);
                    perf.log("native.cursor", || {
                        vec![
                            NativePerfField::str("event", "submit"),
                            NativePerfField::str("kind", "cursor_only"),
                            NativePerfField::u64("generation", cursor.generation),
                            NativePerfField::bool("visible", submitted_state.visible),
                            NativePerfField::str(
                                "position",
                                format!("{},{}", cursor.desired().x, cursor.desired().y),
                            ),
                        ]
                    });
                    Ok(SchedulerDecision::WaitForPageFlip)
                }
                Err(error) if error.kind == AtomicKmsErrorKind::Busy => {
                    atomic_commit_arbiter.cancel(token);
                    fail_transaction(
                        output_transactions,
                        Some(transaction_id),
                        OutputTransactionFailureStage::KmsSubmit,
                    )?;
                    defer_cursor_after_busy(
                        cursor_output_arbitration,
                        frame_scheduler,
                        pacing_now_ns,
                        perf,
                        "atomic_busy",
                    );
                    Ok(SchedulerDecision::Idle)
                }
                Err(error) => {
                    atomic_commit_arbiter.cancel(token);
                    fail_transaction(
                        output_transactions,
                        Some(transaction_id),
                        OutputTransactionFailureStage::KmsSubmit,
                    )?;
                    cursor.note_submit_failure();
                    cursor.note_software_fallback();
                    cursor.set_visible(false);
                    *cursor_render_mode = if client_cursor_active {
                        NativeCursorRenderMode::SoftwareClient
                    } else {
                        NativeCursorRenderMode::Software
                    };
                    *last_client_cursor_damage = None;
                    *effective_cursor = None;
                    *queued_redraw_requested = true;
                    perf.log("native.cursor", || {
                        vec![
                            NativePerfField::str("event", "fallback"),
                            NativePerfField::str("reason", "cursor_submit_failed"),
                            NativePerfField::str("error", error.to_string()),
                        ]
                    });
                    Ok(SchedulerDecision::Render)
                }
            }
        }
        Err(error) if error.kind == AtomicKmsErrorKind::Busy => {
            defer_cursor_after_busy(
                cursor_output_arbitration,
                frame_scheduler,
                pacing_now_ns,
                perf,
                "cursor_test_busy",
            );
            Ok(SchedulerDecision::Idle)
        }
        Err(error) => {
            cursor.note_test_failure();
            cursor.note_software_fallback();
            cursor.set_visible(false);
            *cursor_render_mode = if client_cursor_active {
                NativeCursorRenderMode::SoftwareClient
            } else {
                NativeCursorRenderMode::Software
            };
            *last_client_cursor_damage = None;
            *effective_cursor = None;
            *queued_redraw_requested = true;
            perf.log("native.cursor", || {
                vec![
                    NativePerfField::str("event", "fallback"),
                    NativePerfField::str("reason", "cursor_test_only_rejected"),
                    NativePerfField::str("error", error.to_string()),
                ]
            });
            Ok(SchedulerDecision::Render)
        }
    }
}
