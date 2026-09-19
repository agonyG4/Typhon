use super::cursor_cycle::defer_cursor_after_busy;
use super::presentation_cursor::{cursor_reveal_trace_snapshot, cursor_source_for_trace};
use super::*;
use crate::native_output::presentation::plane::PresentedCursorDelivery;
use oblivion_one::compositor::CompositorFrameBatchId;
use oblivion_one::compositor::{TerminalCallbackDisposition, TerminalCallbackOwnership};
use oblivion_one::native::kms::{KmsBackendKind, KmsBackendSelection};

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectTerminalCallbackDisposition {
    Presented,
    NoVisualChange,
    Abandoned,
    Retryable,
    Superseded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DirectCallbackLeakMetrics {
    pub(crate) leak_events: u64,
    pub(crate) leaked_callbacks: u64,
}

pub(crate) fn effective_output_presentation(
    server: &OwnCompositorServer,
    cursor_visible: bool,
    kms_backend: Option<&KmsBackendSelection>,
    output_generation: u64,
    pacing_mode: NativeOutputPacingMode,
) -> (OutputPresentationMode, DrmContentType) {
    let metadata = server
        .fullscreen_tree_presentation_metadata()
        .unwrap_or_default();
    let metrics = server.fullscreen_render_plan_metrics();
    let effective = EffectivePresentation::decide(
        TearingPolicy::from_environment(std::env::var("OBLIVION_ONE_TEARING").ok().as_deref()),
        metadata,
        AsyncEligibility {
            solitary_fullscreen: metrics.solitary_tree_active,
            async_hint: metadata.hint.is_async(),
            backend_capable: kms_backend.is_some_and(|kms| {
                kms.effective_kind() == KmsBackendKind::Legacy && kms.async_page_flip_capable()
            }),
            async_format_supported: kms_backend.is_some_and(|kms| {
                kms.effective_kind() == KmsBackendKind::Legacy && kms.async_page_flip_capable()
            }),
            output_generation_qualified: output_generation != 0,
            explicit_sync_ready: kms_backend
                .is_some_and(|kms| kms.effective_kind() == KmsBackendKind::Legacy),
            commit_timing_safe: pacing_mode == NativeOutputPacingMode::ReactiveDouble,
            kms_lane_free: kms_backend
                .is_some_and(|kms| kms.effective_kind() == KmsBackendKind::Legacy),
            async_test_only_accepted: kms_backend.is_some_and(|kms| {
                kms.effective_kind() == KmsBackendKind::Legacy && kms.async_page_flip_capable()
            }),
            cursor_visible,
            ..AsyncEligibility::default()
        },
    );
    (
        effective.mode,
        kms_backend.map_or_else(
            || effective.content_type.drm_value(),
            |kms| kms.resolved_content_type(effective.content_type.drm_value()),
        ),
    )
}

pub(crate) fn take_client_cursor_presentation_feedback_batch(
    server: &mut OwnCompositorServer,
    presentation_key: Option<oblivion_one::compositor::SurfacePresentationCommitKey>,
    cursor_delivery: PresentedCursorDelivery,
    cursor_visible: bool,
) -> Option<oblivion_one::compositor::PresentationFeedbackBatchId> {
    if cursor_delivery != PresentedCursorDelivery::Hardware || !cursor_visible {
        return None;
    }
    let presentation_key = presentation_key?;
    server.take_presentation_feedback_batch_for_samples([presentation_key])
}

pub(crate) fn restore_presentation_feedback_obligation(
    server: &mut OwnCompositorServer,
    obligations: OutputProtocolObligations,
) {
    if let Some(batch_id) = obligations.presentation_feedback_batch_id() {
        server.restore_presentation_feedback_batch_after_failure(batch_id);
    }
}

pub(crate) fn discard_presentation_feedback_obligation(
    server: &mut OwnCompositorServer,
    obligations: OutputProtocolObligations,
) {
    if let Some(batch_id) = obligations.presentation_feedback_batch_id() {
        server.discard_presentation_feedback_batch(batch_id);
    }
}

pub(crate) fn complete_presentation_feedback_obligation(
    server: &mut OwnCompositorServer,
    obligations: OutputProtocolObligations,
    presentation: FramePresentation,
) -> NativeResult<()> {
    if let Some(batch_id) = obligations.presentation_feedback_batch_id() {
        server.complete_presentation_feedback_batch(batch_id, presentation);
    }
    Ok(())
}

pub(crate) fn settle_replaced_cursor_sidecar(
    server: &mut OwnCompositorServer,
    output_transactions: &mut OutputTransactionLedger,
    replaced_transaction_id: OutputTransactionId,
    replacement_transaction_id: OutputTransactionId,
    settled_at: MonotonicTimestampNs,
) -> NativeResult<()> {
    let replaced_key = output_transactions
        .transaction(replaced_transaction_id)
        .and_then(|record| record.descriptor().client_cursor_presentation_key());
    let replacement_key = output_transactions
        .transaction(replacement_transaction_id)
        .and_then(|record| record.descriptor().client_cursor_presentation_key());
    if replaced_key == replacement_key {
        output_transactions
            .transfer_presentation_feedback_batch(
                replaced_transaction_id,
                replacement_transaction_id,
            )
            .map_err(io::Error::other)?;
    }
    settle_superseded_output_transaction(
        output_transactions,
        replaced_transaction_id,
        Some(replacement_transaction_id),
        OutputTransactionSupersedeReason::NewerTransaction,
        settled_at,
        |obligations| {
            if replaced_key != replacement_key {
                restore_presentation_feedback_obligation(server, obligations);
            }
            Ok(())
        },
    )
}

pub(crate) fn rebind_cursor_presentation_feedback_to_frozen_sidecar(
    server: &mut OwnCompositorServer,
    output_transactions: &mut OutputTransactionLedger,
    primary_transaction_id: OutputTransactionId,
    sidecar_transaction_id: OutputTransactionId,
) -> NativeResult<()> {
    let primary_key = output_transactions
        .transaction(primary_transaction_id)
        .and_then(|record| record.descriptor().client_cursor_presentation_key());
    let sidecar_key = output_transactions
        .transaction(sidecar_transaction_id)
        .and_then(|record| record.descriptor().client_cursor_presentation_key());
    if primary_key == sidecar_key {
        output_transactions
            .transfer_submitted_cursor_presentation_feedback_batch(
                primary_transaction_id,
                sidecar_transaction_id,
            )
            .map_err(io::Error::other)?;
        return Ok(());
    }
    if let Some(batch_id) = output_transactions
        .detach_submitted_cursor_presentation_feedback_batch(primary_transaction_id)
        .map_err(io::Error::other)?
    {
        server.restore_presentation_feedback_batch_after_failure(batch_id);
    }
    Ok(())
}

pub(crate) fn settle_returned_cursor_sidecar(
    server: &mut OwnCompositorServer,
    output_transactions: &mut OutputTransactionLedger,
    sidecar_transaction_id: OutputTransactionId,
    transfer_to_primary: Option<OutputTransactionId>,
    restore_if_not_transferable: bool,
    settled_at: MonotonicTimestampNs,
) -> NativeResult<()> {
    let sidecar_key = output_transactions
        .transaction(sidecar_transaction_id)
        .and_then(|record| record.descriptor().client_cursor_presentation_key());
    if let Some(primary_transaction_id) = transfer_to_primary
        && output_transactions
            .transaction(primary_transaction_id)
            .is_some_and(|record| {
                matches!(
                    record.state(),
                    OutputTransactionState::Built
                        | OutputTransactionState::Ready { .. }
                        | OutputTransactionState::Queued { .. }
                )
            })
        && output_transactions
            .transaction(sidecar_transaction_id)
            .is_some_and(|record| {
                matches!(
                    record.state(),
                    OutputTransactionState::Built
                        | OutputTransactionState::Ready { .. }
                        | OutputTransactionState::Queued { .. }
                )
            })
        && sidecar_key
            == output_transactions
                .transaction(primary_transaction_id)
                .and_then(|record| record.descriptor().client_cursor_presentation_key())
    {
        output_transactions
            .transfer_presentation_feedback_batch(sidecar_transaction_id, primary_transaction_id)
            .map_err(io::Error::other)?;
        return settle_superseded_output_transaction(
            output_transactions,
            sidecar_transaction_id,
            Some(primary_transaction_id),
            OutputTransactionSupersedeReason::NewerTransaction,
            settled_at,
            |_| Ok(()),
        );
    }
    settle_superseded_output_transaction(
        output_transactions,
        sidecar_transaction_id,
        None,
        OutputTransactionSupersedeReason::NewerTransaction,
        settled_at,
        |obligations| {
            if restore_if_not_transferable {
                restore_presentation_feedback_obligation(server, obligations);
            } else {
                discard_presentation_feedback_obligation(server, obligations);
            }
            Ok(())
        },
    )
}

pub(crate) fn direct_terminal_callback_owner_leaks(
    server: &mut OwnCompositorServer,
    transaction_id: OutputTransactionId,
    obligations: OutputProtocolObligations,
    disposition: DirectTerminalCallbackDisposition,
) -> DirectCallbackLeakMetrics {
    debug_assert!(transaction_id.get() > 0);
    debug_assert!(obligations.direct_surface_id().is_some());
    match disposition {
        DirectTerminalCallbackDisposition::Presented => obligations
            .frame_batch_id()
            .map(|batch_id| {
                match server.prepare_terminal_callback_ownership(
                    batch_id,
                    TerminalCallbackDisposition::Presented,
                ) {
                    TerminalCallbackOwnership::Leaked { unresolved, .. } => {
                        DirectCallbackLeakMetrics {
                            leak_events: 1,
                            leaked_callbacks: unresolved as u64,
                        }
                    }
                    TerminalCallbackOwnership::None
                    | TerminalCallbackOwnership::Resolved { .. }
                    | TerminalCallbackOwnership::Transferred { .. }
                    | TerminalCallbackOwnership::Cancelled { .. } => DirectCallbackLeakMetrics {
                        leak_events: 0,
                        leaked_callbacks: 0,
                    },
                }
            })
            .unwrap_or(DirectCallbackLeakMetrics {
                leak_events: 0,
                leaked_callbacks: 0,
            }),
        DirectTerminalCallbackDisposition::NoVisualChange => obligations
            .frame_batch_id()
            .map(|batch_id| {
                match server.prepare_terminal_callback_ownership(
                    batch_id,
                    TerminalCallbackDisposition::NoVisualChange,
                ) {
                    TerminalCallbackOwnership::Leaked { unresolved, .. } => {
                        DirectCallbackLeakMetrics {
                            leak_events: 1,
                            leaked_callbacks: unresolved as u64,
                        }
                    }
                    TerminalCallbackOwnership::None
                    | TerminalCallbackOwnership::Resolved { .. }
                    | TerminalCallbackOwnership::Transferred { .. }
                    | TerminalCallbackOwnership::Cancelled { .. } => DirectCallbackLeakMetrics {
                        leak_events: 0,
                        leaked_callbacks: 0,
                    },
                }
            })
            .unwrap_or(DirectCallbackLeakMetrics {
                leak_events: 0,
                leaked_callbacks: 0,
            }),
        DirectTerminalCallbackDisposition::Retryable => obligations
            .frame_batch_id()
            .map(|batch_id| {
                match server.prepare_terminal_callback_ownership(
                    batch_id,
                    TerminalCallbackDisposition::Retryable,
                ) {
                    TerminalCallbackOwnership::Leaked { unresolved, .. } => {
                        DirectCallbackLeakMetrics {
                            leak_events: 1,
                            leaked_callbacks: unresolved as u64,
                        }
                    }
                    TerminalCallbackOwnership::None
                    | TerminalCallbackOwnership::Resolved { .. }
                    | TerminalCallbackOwnership::Transferred { .. }
                    | TerminalCallbackOwnership::Cancelled { .. } => DirectCallbackLeakMetrics {
                        leak_events: 0,
                        leaked_callbacks: 0,
                    },
                }
            })
            .unwrap_or(DirectCallbackLeakMetrics {
                leak_events: 0,
                leaked_callbacks: 0,
            }),
        DirectTerminalCallbackDisposition::Abandoned => obligations
            .frame_batch_id()
            .map(|batch_id| {
                match server.prepare_terminal_callback_ownership(
                    batch_id,
                    TerminalCallbackDisposition::Cancelled,
                ) {
                    TerminalCallbackOwnership::Leaked { unresolved, .. } => {
                        DirectCallbackLeakMetrics {
                            leak_events: 1,
                            leaked_callbacks: unresolved as u64,
                        }
                    }
                    TerminalCallbackOwnership::None
                    | TerminalCallbackOwnership::Resolved { .. }
                    | TerminalCallbackOwnership::Transferred { .. }
                    | TerminalCallbackOwnership::Cancelled { .. } => DirectCallbackLeakMetrics {
                        leak_events: 0,
                        leaked_callbacks: 0,
                    },
                }
            })
            .unwrap_or(DirectCallbackLeakMetrics {
                leak_events: 0,
                leaked_callbacks: 0,
            }),
        DirectTerminalCallbackDisposition::Superseded => obligations
            .frame_batch_id()
            .map(|batch_id| {
                match server.prepare_terminal_callback_ownership(
                    batch_id,
                    TerminalCallbackDisposition::Superseded,
                ) {
                    TerminalCallbackOwnership::Leaked { unresolved, .. } => {
                        DirectCallbackLeakMetrics {
                            leak_events: 1,
                            leaked_callbacks: unresolved as u64,
                        }
                    }
                    TerminalCallbackOwnership::None
                    | TerminalCallbackOwnership::Resolved { .. }
                    | TerminalCallbackOwnership::Transferred { .. }
                    | TerminalCallbackOwnership::Cancelled { .. } => DirectCallbackLeakMetrics {
                        leak_events: 0,
                        leaked_callbacks: 0,
                    },
                }
            })
            .unwrap_or(DirectCallbackLeakMetrics {
                leak_events: 0,
                leaked_callbacks: 0,
            }),
    }
}

pub(super) fn finish_direct_terminal_callback_cleanup<Settle, Note>(
    scanout_destroyed: bool,
    settle_callback_ownership: Settle,
    note_backend_metrics: Note,
) where
    Settle: FnOnce() -> DirectCallbackLeakMetrics,
    Note: FnOnce(DirectCallbackLeakMetrics),
{
    let callback_owner_leaks = settle_callback_ownership();
    if !scanout_destroyed {
        note_backend_metrics(callback_owner_leaks);
    }
}

fn settle_accepted_output_transaction<F>(
    output_transactions: &mut OutputTransactionLedger,
    accepted: AcceptedTerminalTransition,
    settle_protocol_obligations: F,
) -> NativeResult<()>
where
    F: FnOnce(OutputProtocolObligations) -> NativeResult<()>,
{
    if let Err(error) = settle_protocol_obligations(accepted.obligations()) {
        let _ = output_transactions.fail_settlement(
            accepted,
            MonotonicTimestampNs::new(monotonic_now_ns().unwrap_or(0)),
        );
        return Err(error);
    }
    if let Err(error) = output_transactions.finalize_terminal(accepted) {
        let _ = output_transactions.fail_settlement(
            accepted,
            MonotonicTimestampNs::new(monotonic_now_ns().unwrap_or(0)),
        );
        return Err(io::Error::other(error).into());
    }
    Ok(())
}

pub(crate) fn settle_failed_output_transaction<F>(
    output_transactions: &mut OutputTransactionLedger,
    transaction_id: OutputTransactionId,
    stage: OutputTransactionFailureStage,
    at: MonotonicTimestampNs,
    settle_protocol_obligations: F,
) -> NativeResult<()>
where
    F: FnOnce(OutputProtocolObligations) -> NativeResult<()>,
{
    let accepted = output_transactions
        .accept_failed(transaction_id, stage, at)
        .map_err(io::Error::other)?;
    settle_accepted_output_transaction(output_transactions, accepted, settle_protocol_obligations)
}

pub(crate) fn settle_dropped_output_transaction<F>(
    output_transactions: &mut OutputTransactionLedger,
    transaction_id: OutputTransactionId,
    reason: OutputTransactionDropReason,
    at: MonotonicTimestampNs,
    settle_protocol_obligations: F,
) -> NativeResult<()>
where
    F: FnOnce(OutputProtocolObligations) -> NativeResult<()>,
{
    let accepted = output_transactions
        .accept_dropped(transaction_id, reason, at)
        .map_err(io::Error::other)?;
    settle_accepted_output_transaction(output_transactions, accepted, settle_protocol_obligations)
}

pub(crate) fn settle_no_visual_change_output_transaction<F>(
    output_transactions: &mut OutputTransactionLedger,
    transaction_id: OutputTransactionId,
    at: MonotonicTimestampNs,
    settle_protocol_obligations: F,
) -> NativeResult<()>
where
    F: FnOnce(OutputProtocolObligations) -> NativeResult<()>,
{
    let accepted = output_transactions
        .accept_no_visual_change(transaction_id, at)
        .map_err(io::Error::other)?;
    settle_accepted_output_transaction(output_transactions, accepted, settle_protocol_obligations)
}

fn settle_forced_shutdown_transaction<F>(
    output_transactions: &mut OutputTransactionLedger,
    transaction_id: OutputTransactionId,
    token: PageFlipToken,
    at: MonotonicTimestampNs,
    settle_protocol_obligations: F,
) -> NativeResult<bool>
where
    F: FnOnce(OutputProtocolObligations) -> NativeResult<()>,
{
    let Some(record) = output_transactions.transaction(transaction_id) else {
        return Ok(false);
    };
    match record.state() {
        OutputTransactionState::Submitted {
            token: submitted_token,
            ..
        } if submitted_token == token => {}
        OutputTransactionState::Submitted { .. } => {
            return Err(io::Error::other(
                "forced shutdown transaction token mismatches worker identity",
            )
            .into());
        }
        OutputTransactionState::Built
        | OutputTransactionState::Ready { .. }
        | OutputTransactionState::ReadyUnbound { .. }
        | OutputTransactionState::Queued { .. }
        | OutputTransactionState::Settling { .. }
        | OutputTransactionState::Terminal(_) => return Ok(false),
    }
    settle_dropped_output_transaction(
        output_transactions,
        transaction_id,
        OutputTransactionDropReason::SafeAbandonment,
        at,
        settle_protocol_obligations,
    )?;
    Ok(true)
}

pub(crate) fn settle_forced_shutdown_transaction_if_safe<F>(
    safety: KmsTeardownSafety,
    output_transactions: &mut OutputTransactionLedger,
    transaction_id: OutputTransactionId,
    token: PageFlipToken,
    at: MonotonicTimestampNs,
    settle_protocol_obligations: F,
) -> NativeResult<bool>
where
    F: FnOnce(OutputProtocolObligations) -> NativeResult<()>,
{
    if !safety.permits_release() {
        return Ok(false);
    }
    settle_forced_shutdown_transaction(
        output_transactions,
        transaction_id,
        token,
        at,
        settle_protocol_obligations,
    )
}

#[allow(dead_code)]
pub(crate) fn settle_superseded_output_transaction<F>(
    output_transactions: &mut OutputTransactionLedger,
    transaction_id: OutputTransactionId,
    by: Option<OutputTransactionId>,
    reason: OutputTransactionSupersedeReason,
    at: MonotonicTimestampNs,
    settle_protocol_obligations: F,
) -> NativeResult<()>
where
    F: FnOnce(OutputProtocolObligations) -> NativeResult<()>,
{
    let accepted = output_transactions
        .accept_superseded(transaction_id, by, reason, at)
        .map_err(io::Error::other)?;
    settle_accepted_output_transaction(output_transactions, accepted, settle_protocol_obligations)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn complete_presented_output_transaction<F>(
    output_transactions: &mut OutputTransactionLedger,
    presentation_trace: &mut PresentationTransactionTraceRing,
    transaction_id: OutputTransactionId,
    token: PageFlipToken,
    output_generation: u64,
    presented_at: MonotonicTimestampNs,
    actual_sequence: Option<u64>,
    settle_protocol_obligations: F,
) -> NativeResult<()>
where
    F: FnOnce(OutputProtocolObligations) -> NativeResult<()>,
{
    let accepted = prepare_presented_output_transaction(
        output_transactions,
        transaction_id,
        token,
        output_generation,
        presented_at,
        actual_sequence,
    )?;
    commit_prepared_presented_output_transaction(
        output_transactions,
        presentation_trace,
        accepted,
        settle_protocol_obligations,
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn prepare_presented_output_transaction(
    output_transactions: &mut OutputTransactionLedger,
    transaction_id: OutputTransactionId,
    token: PageFlipToken,
    output_generation: u64,
    presented_at: MonotonicTimestampNs,
    actual_sequence: Option<u64>,
) -> NativeResult<AcceptedTerminalTransition> {
    output_transactions
        .accept_presented(
            transaction_id,
            token,
            output_generation,
            presented_at,
            actual_sequence,
        )
        .map_err(io::Error::other)
        .map_err(Into::into)
}

pub(super) fn commit_prepared_presented_output_transaction<F>(
    output_transactions: &mut OutputTransactionLedger,
    presentation_trace: &mut PresentationTransactionTraceRing,
    accepted: AcceptedTerminalTransition,
    settle_protocol_obligations: F,
) -> NativeResult<()>
where
    F: FnOnce(OutputProtocolObligations) -> NativeResult<()>,
{
    let transaction_id = accepted.transaction_id();
    let presented_at = match accepted.terminal() {
        OutputTransactionTerminal::Presented { presented_at, .. } => presented_at,
        _ => return Err(io::Error::other("prepared output transition is not presented").into()),
    };
    settle_accepted_output_transaction(output_transactions, accepted, settle_protocol_obligations)?;
    presentation_trace.push(PresentationTransactionEvent::PageflipPresented {
        transaction_id,
        timestamp_ns: presented_at.get(),
    });
    Ok(())
}

pub(super) fn complete_dropped_output_transaction<F>(
    output_transactions: &mut OutputTransactionLedger,
    transaction_id: OutputTransactionId,
    reason: OutputTransactionDropReason,
    at: MonotonicTimestampNs,
    settle_protocol_obligations: F,
) -> NativeResult<()>
where
    F: FnOnce(OutputProtocolObligations) -> NativeResult<()>,
{
    let accepted = output_transactions
        .accept_dropped(transaction_id, reason, at)
        .map_err(io::Error::other)?;
    settle_accepted_output_transaction(output_transactions, accepted, settle_protocol_obligations)?;
    Ok(())
}

pub(super) fn complete_immediate_output_transaction(
    output_transactions: &mut OutputTransactionLedger,
    presentation_trace: &mut PresentationTransactionTraceRing,
    server: &mut OwnCompositorServer,
    transaction_id: OutputTransactionId,
    presented_at: MonotonicTimestampNs,
) -> NativeResult<()> {
    complete_immediate_output_transaction_with(
        output_transactions,
        presentation_trace,
        server,
        transaction_id,
        presented_at,
        |server, batch_id| server.finish_immediate_frame_batch(batch_id),
    )
}

pub(super) fn complete_immediate_output_transaction_with<F>(
    output_transactions: &mut OutputTransactionLedger,
    presentation_trace: &mut PresentationTransactionTraceRing,
    server: &mut OwnCompositorServer,
    transaction_id: OutputTransactionId,
    presented_at: MonotonicTimestampNs,
    finish_immediate_frame_batch: F,
) -> NativeResult<()>
where
    F: FnOnce(&mut OwnCompositorServer, CompositorFrameBatchId) -> io::Result<()>,
{
    let accepted = output_transactions
        .accept_immediate_presented(transaction_id, presented_at)
        .map_err(io::Error::other)?;
    settle_accepted_output_transaction(output_transactions, accepted, |obligations| {
        let batch_id = obligations
            .frame_batch_id()
            .ok_or_else(|| io::Error::other("Immediate transaction has no frame batch"))?;
        finish_immediate_frame_batch(server, batch_id)?;
        Ok(())
    })?;
    presentation_trace.push(PresentationTransactionEvent::ImmediatePresented {
        transaction_id,
        timestamp_ns: presented_at.get(),
    });
    Ok(())
}

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
    kms_backend: Option<&KmsBackendSelection>,
) -> NativeResult<Option<OutputTransactionId>> {
    let (presentation_mode, content_type) = effective_output_presentation(
        server,
        cursor.is_some_and(|state| state.visible),
        kms_backend,
        output_generation,
        pacing_mode,
    );
    let frame_batch_id = server
        .prepared_frame_batch_id()
        .ok_or_else(|| io::Error::other("compatibility pageflip has no prepared frame batch"))?;
    let frame_id = server
        .prepared_frame_id()
        .ok_or_else(|| io::Error::other("compatibility pageflip has no prepared frame ID"))?;
    let transaction_id = output_transactions
        .allocate_id()
        .map_err(io::Error::other)?;
    let transaction = match scanout.compatibility_framebuffer_id() {
        Some(framebuffer_id) => OutputTransaction::compatibility_composited(
            output_transactions.output_id(),
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
                state: Some(state.clone()),
            }),
            frame_batch_id,
        ),
        None if scanout.kind() == NativeScanoutKind::DumbFramebuffer => {
            OutputTransaction::compatibility_immediate(
                output_transactions.output_id(),
                transaction_id,
                output_generation,
                MonotonicTimestampNs::new(monotonic_now_ns()?),
                target,
                pacing_mode,
                frame_id,
                frame_batch_id,
            )
        }
        None => return Ok(None),
    }
    .map_err(io::Error::other)?
    .with_presentation_state(presentation_mode, content_type);
    output_transactions
        .insert(transaction)
        .map_err(io::Error::other)?;
    Ok(Some(transaction_id))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn present_compatibility_frame(
    scanout: &mut NativeScanoutBackend,
    server: &mut OwnCompositorServer,
    output_transactions: &mut OutputTransactionLedger,
    output_generation: u64,
    crtc_id: u32,
    target: PresentationTarget,
    pacing_mode: NativeOutputPacingMode,
    render_generation: u64,
    cursor: Option<&AtomicCursorVisualState>,
    cursor_epoch: u64,
    frame_index: u64,
    kms_backend: Option<&KmsBackendSelection>,
    present: impl FnOnce(
        &mut NativeScanoutBackend,
        OutputPresentationMode,
    ) -> io::Result<NativePresentResult>,
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
        kms_backend,
    )?;
    let presentation_mode = transaction_id
        .and_then(|transaction_id| output_transactions.transaction(transaction_id))
        .map(|record| record.descriptor().presentation_mode())
        .unwrap_or_default();
    let result = present(scanout, presentation_mode).map_err(|error| {
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
        Ok(NativePresentResult::Immediate) => Ok((NativePresentResult::Immediate, transaction_id)),
        Ok(NativePresentResult::Noop) => {
            if let Some(transaction_id) = transaction_id {
                settle_dropped_output_transaction(
                    output_transactions,
                    transaction_id,
                    OutputTransactionDropReason::NoVisualChange,
                    MonotonicTimestampNs::new(monotonic_now_ns()?),
                    |obligations| {
                        let batch_id = obligations.frame_batch_id().ok_or_else(|| {
                            io::Error::other("compatibility Noop transaction has no frame batch")
                        })?;
                        server.restore_frame_batch_after_render_failure(batch_id);
                        Ok(())
                    },
                )?;
            }
            Ok((NativePresentResult::Noop, None))
        }
        Err(error) => {
            if let Some(transaction_id) = transaction_id {
                settle_failed_output_transaction(
                    output_transactions,
                    transaction_id,
                    OutputTransactionFailureStage::KmsSubmit,
                    MonotonicTimestampNs::new(monotonic_now_ns()?),
                    |obligations| {
                        let batch_id = obligations.frame_batch_id().ok_or_else(|| {
                            io::Error::other(
                                "compatibility backend error transaction has no frame batch",
                            )
                        })?;
                        server.restore_frame_batch_after_render_failure(batch_id);
                        Ok(())
                    },
                )?;
            }
            Err(Box::new(error))
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn register_primary_transaction(
    atomic_commit_arbiter: &mut AtomicCommitArbiter,
    server: &mut OwnCompositorServer,
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
            OutputTransactionContent::CompatibilityImmediate { frame_id } => Some(frame_id),
            OutputTransactionContent::PlaneDelta { .. } => None,
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
            if let Some(transaction_id) = transaction_id {
                settle_failed_output_transaction(
                    output_transactions,
                    transaction_id,
                    OutputTransactionFailureStage::BackendCompletion,
                    MonotonicTimestampNs::new(monotonic_now_ns()?),
                    |obligations| {
                        let batch_id = obligations.frame_batch_id().ok_or_else(|| {
                            io::Error::other("primary registration failure has no frame batch")
                        })?;
                        server.discard_frame_batch(
                            batch_id,
                            FrameBatchDiscardReason::FatalOutputFailure,
                        );
                        Ok(())
                    },
                )?;
            }
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
    created_at: MonotonicTimestampNs,
    output_generation: u64,
    target: PresentationTarget,
    pacing_mode: NativeOutputPacingMode,
    cursor_epoch: u64,
    desired: Option<&AtomicCursorVisualState>,
    surface_damage: Option<oblivion_one::compositor::SurfaceDamagePresentation>,
    client_cursor_presentation_key: Option<oblivion_one::compositor::SurfacePresentationCommitKey>,
    presentation_mode: OutputPresentationMode,
    content_type: DrmContentType,
    presentation_feedback_batch_id: Option<oblivion_one::compositor::PresentationFeedbackBatchId>,
) -> NativeResult<OutputTransactionId> {
    let transaction_id = output_transactions
        .allocate_id()
        .map_err(io::Error::other)?;
    let transaction = OutputTransaction::cursor_plane_delta(
        output_transactions.output_id(),
        transaction_id,
        output_generation,
        created_at,
        target,
        pacing_mode,
        cursor_epoch,
        desired.cloned(),
        OutputReleasePlan::Pageflip,
    )
    .map_err(io::Error::other)?
    .with_presentation_state(presentation_mode, content_type)
    .with_client_cursor_presentation_key(client_cursor_presentation_key);
    let transaction = match presentation_feedback_batch_id {
        Some(batch_id) => transaction
            .with_presentation_feedback_batch(batch_id)
            .map_err(io::Error::other)?,
        None => transaction,
    };
    let transaction = if let Some(surface_damage) = surface_damage {
        transaction.with_surface_damage(surface_damage)
    } else {
        transaction
    };
    output_transactions
        .insert(transaction)
        .map_err(io::Error::other)?;
    presentation_trace.push(PresentationTransactionEvent::TransactionBuilt {
        transaction_id,
        timestamp_ns: created_at.get(),
    });
    Ok(transaction_id)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn submit_plane_delta(
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
    cursor_surface_damage: Option<oblivion_one::compositor::SurfaceDamagePresentation>,
    server: &mut OwnCompositorServer,
    cursor_reveal_trace: &mut Option<CursorRevealTraceLedger>,
) -> NativeResult<SchedulerDecision> {
    let cursor_capability_key = desired
        .as_ref()
        .and_then(|state| cursor.capability_key_for(state));
    match kms_backend.test_atomic_cursor_flip(desired.as_ref()) {
        Ok(()) => {
            let submitted_delivery = if desired.as_ref().is_some_and(|state| state.visible) {
                PresentedCursorDelivery::Hardware
            } else {
                PresentedCursorDelivery::Hidden
            };
            // Read the timestamp before taking presentation-only ownership so
            // every pre-pageflip failure can settle without another fallible
            // operation after the exact cursor feedback leaves the pending
            // registry.
            let submission_at_ns = monotonic_now_ns()?;
            let client_cursor_presentation_key =
                cursor.client_source_key().and_then(|source_key| {
                    server.presentation_commit_key_for_surface_commit(
                        source_key.surface_id,
                        oblivion_one::compositor::SurfaceCommitSequence(source_key.commit_sequence),
                    )
                });
            let (presentation_mode, content_type) = effective_output_presentation(
                server,
                desired.as_ref().is_some_and(|state| state.visible),
                Some(kms_backend),
                output_generation,
                pacing_mode,
            );
            let presentation_feedback_batch_id = take_client_cursor_presentation_feedback_batch(
                server,
                client_cursor_presentation_key,
                submitted_delivery,
                desired.as_ref().is_some_and(|state| state.visible),
            );
            let transaction_id = match build_cursor_transaction(
                output_transactions,
                presentation_trace,
                MonotonicTimestampNs::new(submission_at_ns),
                output_generation,
                target,
                pacing_mode,
                cursor_epoch,
                desired.as_ref(),
                cursor_surface_damage,
                client_cursor_presentation_key,
                presentation_mode,
                content_type,
                presentation_feedback_batch_id,
            ) {
                Ok(transaction_id) => transaction_id,
                Err(error) => {
                    if let Some(batch_id) = presentation_feedback_batch_id {
                        server.restore_presentation_feedback_batch_after_failure(batch_id);
                    }
                    return Err(error);
                }
            };
            let token = PageFlipToken::new(allocate_native_page_flip_token())
                .expect("allocated native pageflip token is nonzero");
            if let Err(error) = atomic_commit_arbiter.reserve(
                token,
                output_generation,
                crtc_id,
                AtomicCommitKind::PlaneDelta {
                    transaction_id,
                    cursor_epoch,
                    framebuffer_id: desired.as_ref().and_then(|state| state.framebuffer_id),
                },
                submission_at_ns,
            ) {
                settle_failed_output_transaction(
                    output_transactions,
                    transaction_id,
                    OutputTransactionFailureStage::KmsSubmit,
                    MonotonicTimestampNs::new(submission_at_ns),
                    |obligations| {
                        restore_presentation_feedback_obligation(server, obligations);
                        Ok(())
                    },
                )?;
                return Err(Box::new(io::Error::other(error)));
            }
            let submitted_state = desired.clone().unwrap_or_else(|| {
                let mut hidden = cursor.desired().clone();
                hidden.visible = false;
                hidden.framebuffer_id = None;
                hidden
            });
            let submitted_revision = cursor.revision_for_legacy_epoch(cursor_epoch);
            let trace_snapshot = cursor_reveal_trace_snapshot(
                server,
                &submitted_state,
                Some(cursor_epoch),
                Some(submitted_revision),
                submitted_delivery,
                Some(cursor_source_for_trace(cursor)),
            );
            match kms_backend.submit_cursor_flip(desired.as_ref(), token) {
                Ok(()) => {
                    if let Some(submitter) = kms_backend.atomic_commit_submitter() {
                        trace_cursor_kms_submit(
                            submitter.pipeline(),
                            if let Some(state) = desired.as_ref() {
                                CursorKmsAssignment::Set(state)
                            } else {
                                CursorKmsAssignment::Disable
                            },
                            CursorKmsSubmitContext {
                                output_id: output_transactions.output_id(),
                                output_generation,
                                transaction_id: Some(transaction_id),
                                token,
                                crtc_id,
                                cursor_epoch: Some(cursor_epoch),
                                cursor_revision: Some(submitted_revision),
                                submission_kind: "cursor_only",
                                transport: "synchronous",
                                delivery: submitted_delivery,
                            },
                        );
                    }
                    if let Some(snapshot) = trace_snapshot
                        && let Some(ledger) = cursor_reveal_trace.as_mut()
                    {
                        ledger.bind(
                            CursorRevealPhysicalIdentity {
                                output_id: output_transactions.output_id(),
                                output_generation,
                                crtc_id,
                                token,
                            },
                            snapshot,
                        );
                    }
                    output_transactions
                        .mark_submitted(
                            transaction_id,
                            token,
                            MonotonicTimestampNs::new(submission_at_ns),
                        )
                        .map_err(io::Error::other)?;
                    presentation_trace.push(PresentationTransactionEvent::KmsSubmitReturned {
                        transaction_id,
                        timestamp_ns: submission_at_ns,
                    });
                    let submitted_state = cursor.begin_submission_at_revision_with_capability_key(
                        token,
                        submitted_state,
                        cursor_epoch,
                        cursor.revision_for_legacy_epoch(cursor_epoch),
                        cursor_capability_key,
                    );
                    cursor_output_arbitration.note_plane_delta_submission();
                    *last_client_cursor_damage = current_client_cursor_damage;
                    *last_software_cursor_damage = current_software_cursor_damage;
                    cursor_output_arbitration.consume(cursor_epoch);
                    perf.log("native.cursor", || {
                        vec![
                            NativePerfField::str("event", "submit"),
                            NativePerfField::str("kind", "plane_delta"),
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
                    settle_failed_output_transaction(
                        output_transactions,
                        transaction_id,
                        OutputTransactionFailureStage::KmsSubmit,
                        MonotonicTimestampNs::new(submission_at_ns),
                        |obligations| {
                            restore_presentation_feedback_obligation(server, obligations);
                            Ok(())
                        },
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
                    settle_failed_output_transaction(
                        output_transactions,
                        transaction_id,
                        OutputTransactionFailureStage::KmsSubmit,
                        MonotonicTimestampNs::new(submission_at_ns),
                        |obligations| {
                            restore_presentation_feedback_obligation(server, obligations);
                            Ok(())
                        },
                    )?;
                    cursor.note_submit_failure_for(cursor_capability_key);
                    cursor.note_software_fallback();
                    cursor.note_composed_software_fallback();
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
            cursor.note_test_failure_for(cursor_capability_key);
            cursor.note_software_fallback();
            cursor.note_composed_software_fallback();
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

#[cfg(test)]
mod teardown_tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn destroyed_scanout_still_settles_callbacks_without_backend_access() {
        let callback_cleanup_count = Cell::new(0);
        let backend_metric_updates = Cell::new(0);
        let leaks = DirectCallbackLeakMetrics {
            leak_events: 1,
            leaked_callbacks: 2,
        };

        finish_direct_terminal_callback_cleanup(
            true,
            || {
                callback_cleanup_count.set(callback_cleanup_count.get() + 1);
                leaks
            },
            |_| backend_metric_updates.set(backend_metric_updates.get() + 1),
        );

        assert_eq!(callback_cleanup_count.get(), 1);
        assert_eq!(backend_metric_updates.get(), 0);
    }

    #[test]
    fn live_scanout_records_terminal_callback_metrics_including_zeroes() {
        let callback_cleanup_count = Cell::new(0);
        let backend_metric_updates = Cell::new(0);

        finish_direct_terminal_callback_cleanup(
            false,
            || {
                callback_cleanup_count.set(callback_cleanup_count.get() + 1);
                DirectCallbackLeakMetrics {
                    leak_events: 0,
                    leaked_callbacks: 0,
                }
            },
            |_| backend_metric_updates.set(backend_metric_updates.get() + 1),
        );

        assert_eq!(callback_cleanup_count.get(), 1);
        assert_eq!(backend_metric_updates.get(), 1);
    }
}
