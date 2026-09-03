use super::super::cursor_cycle::{commit_primary_cursor_pageflip, prepare_primary_cursor_pageflip};
use super::super::presentation_transactions::{
    DirectTerminalCallbackDisposition, direct_terminal_callback_owner_leaks,
    prepare_presented_output_transaction,
};
use super::cycle::direct_fallback::DirectFallbackTracker;
use super::*;

pub(super) fn fail_composited_transition(
    worker: Option<&crate::native_output::kms_worker::KmsCommitWorkerHandle>,
    direct_fallback_tracker: &mut Option<DirectFallbackTracker>,
    scanout: &mut NativeScanoutBackend,
    frame_scheduler: &mut NativeFrameScheduler,
    atomic_commit_arbiter: &mut AtomicCommitArbiter,
    reason: DirectReleaseViolation,
) -> NativeResult<NativeCycleState> {
    if let Some(worker) = worker {
        worker.mark_admission_fatal();
    }
    scanout.note_direct_fallback_cycles(0);
    *direct_fallback_tracker = None;
    frame_scheduler.abandon_for_session_suspend();
    atomic_commit_arbiter.abandon_for_recovery();
    scanout.suspend_page_flip()?;
    Err(io::Error::other(format!(
        "direct ownership could not be retired after composed pageflip: {reason:?}"
    ))
    .into())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn settle_direct_pageflip(
    scanout: &mut NativeScanoutBackend,
    scene_history: &mut NativeSceneHistory,
    server: &mut OwnCompositorServer,
    output_transactions: &mut OutputTransactionLedger,
    presentation_trace: &mut PresentationTransactionTraceRing,
    atomic_cursor: &mut Option<NativeAtomicCursor>,
    drm_file_generation: u64,
    pageflip_identity: crate::native_output::presentation::plane::PlanePageflipIdentity,
    transaction_id: OutputTransactionId,
    pageflip_token: PageFlipToken,
    pageflip_user_data: u64,
    pageflip_sequence: u32,
    presented_at: MonotonicTimestampNs,
    presented_at_ns: u64,
    actual_logical_sequence: u64,
    presentation: FramePresentation,
    presented_primary: &mut Option<PresentedPrimaryAssignment>,
    render_journal: &mut AdaptiveRenderJournal,
    frame_pacing: &mut NativeFramePacing,
    scheduled_presentation_target: &mut Option<PresentationTarget>,
) -> NativeResult<()> {
    let direct_info = scanout.direct_pageflip_info(transaction_id, pageflip_token)?;
    let prepared_physical =
        scanout.prepare_direct_pageflip(transaction_id, pageflip_token, presented_at)?;
    let prepared_cursor =
        prepare_primary_cursor_pageflip(atomic_cursor, pageflip_user_data, drm_file_generation)?;
    let prepared_logical = prepare_presented_output_transaction(
        output_transactions,
        transaction_id,
        pageflip_token,
        drm_file_generation,
        presented_at,
        Some(u64::from(pageflip_sequence)),
    )?;
    if prepared_logical.obligations().frame_batch_id() != Some(direct_info.protocol_batch_id)
        || prepared_logical.obligations().direct_surface_id() != Some(direct_info.surface_id)
    {
        output_transactions
            .rollback_settlement(prepared_logical)
            .map_err(io::Error::other)?;
        return Err(io::Error::other("direct pageflip obligation identity mismatch").into());
    }
    let logical_obligations = prepared_logical.obligations();
    let prepared_frame_batch = match server.prepare_direct_presented_frame_batch(
        direct_info.frame_id,
        direct_info.protocol_batch_id,
        direct_info.surface_id,
    ) {
        Ok(prepared) => prepared,
        Err(error) => {
            output_transactions
                .rollback_settlement(prepared_logical)
                .map_err(io::Error::other)?;
            if !server.has_frame_batch(direct_info.protocol_batch_id) {
                scanout.note_direct_duplicate_feedback();
                output_transactions.note_duplicate_settlement_attempt();
            }
            return Err(error.into());
        }
    };
    let callback_owner_leaks = direct_terminal_callback_owner_leaks(
        server,
        transaction_id,
        logical_obligations,
        DirectTerminalCallbackDisposition::Presented,
    );
    let completion = scanout.commit_prepared_direct_pageflip(prepared_physical);
    output_transactions.commit_prepared_terminal(prepared_logical);
    if prepared_cursor {
        commit_primary_cursor_pageflip(atomic_cursor, pageflip_user_data, drm_file_generation);
    }
    server.commit_surface_damage_presented(completion.surface_damage);
    scanout.note_direct_presentation();
    debug_assert_eq!(completion.transaction_id, transaction_id);
    debug_assert_eq!(completion.token, pageflip_token);
    let previous_assignment = *presented_primary;
    if previous_assignment.is_some_and(|assignment| assignment.is_direct()) {
        scanout.note_direct_replacement();
    } else {
        scanout.note_direct_entry();
        scanout.invalidate_presented_damage_history();
        scene_history.invalidate_presented_damage_history();
    }
    *presented_primary = Some(PresentedPrimaryAssignment::Direct {
        transaction_id,
        token: pageflip_token,
        pageflip: pageflip_identity,
        surface_id: completion.surface_id,
        key: completion.candidate_key,
        framebuffer_id: completion.framebuffer_id,
    });
    debug_assert_eq!(
        scanout.direct_scanout_presented_info(),
        Some((
            completion.surface_id,
            completion.candidate_key.content.buffer_id.get(),
            completion.framebuffer_id,
            completion.candidate_key.content.content_epoch.get(),
        ))
    );
    debug_assert!(output_transactions.transaction(transaction_id).is_none());
    presentation_trace.push(PresentationTransactionEvent::PageflipPresented {
        transaction_id,
        timestamp_ns: presented_at.get(),
    });
    server.commit_prepared_direct_presented_frame_batch(prepared_frame_batch, presentation);
    scanout.note_direct_callback_owner_leaks(callback_owner_leaks);
    drop(completion.replaced);
    let target = direct_info.target;
    let submit_started_at = direct_info.submit_started_at;
    let submit_returned_at = direct_info.submit_returned_at;
    render_journal.note_matching_presentation(presented_at);
    frame_pacing.note_explicit_present(ExplicitPresentationObservation {
        planned_sequence: target.sequence,
        actual_sequence: actual_logical_sequence,
        target_ns: target.presentation_time.get(),
        presented_ns: presented_at_ns,
        composite_started_ns: submit_started_at.get(),
        rendered_ns: submit_returned_at.get(),
        submit_started_ns: submit_started_at.get(),
        submit_returned_ns: submit_returned_at.get(),
        reactive_double: target.reason == PresentationTargetReason::ReactiveDouble,
        target_reason: target.reason,
        target_selection: target.selection_evidence(),
        previous_primary_sequence: None,
        client_commit_ns: None,
        callback_reaction_ns: None,
        callback_admission_ns: None,
        callback_surface_id: None,
        callback_surface_is_exclusive: false,
        refresh_interval_ns: u64::try_from(target.refresh_interval.as_nanos()).unwrap_or(u64::MAX),
        render_missed: false,
        submit_missed: false,
        kms_slipped: false,
    });
    *scheduled_presentation_target = None;
    Ok(())
}
