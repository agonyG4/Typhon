use super::super::cursor_cycle::complete_primary_cursor_pageflip;
use super::super::presentation_transactions::complete_presented_output_transaction;
use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn settle_direct_pageflip(
    scanout: &mut NativeScanoutBackend,
    server: &mut OwnCompositorServer,
    output_transactions: &mut OutputTransactionLedger,
    presentation_trace: &mut PresentationTransactionTraceRing,
    atomic_cursor: &mut Option<NativeAtomicCursor>,
    drm_file_generation: u64,
    transaction_id: OutputTransactionId,
    pageflip_token: PageFlipToken,
    pageflip_user_data: u64,
    pageflip_sequence: u32,
    presented_at: MonotonicTimestampNs,
    presented_at_ns: u64,
    actual_logical_sequence: u64,
    presentation: FramePresentation,
    confirmed_primary_assignment: &mut Option<ConfirmedPrimaryAssignment>,
    render_journal: &mut AdaptiveRenderJournal,
    frame_pacing: &mut NativeFramePacing,
    scheduled_presentation_target: &mut Option<PresentationTarget>,
) -> NativeResult<()> {
    let direct_info = scanout.direct_pageflip_info(transaction_id, pageflip_token)?;
    let mut completed = None;
    complete_presented_output_transaction(
        output_transactions,
        presentation_trace,
        transaction_id,
        pageflip_token,
        drm_file_generation,
        presented_at,
        Some(u64::from(pageflip_sequence)),
        |obligations| {
            debug_assert_eq!(
                obligations.direct_surface_id(),
                scanout.direct_scanout_surface()
            );
            complete_primary_cursor_pageflip(
                atomic_cursor,
                pageflip_user_data,
                drm_file_generation,
            )?;
            let surface_damage =
                scanout.take_direct_pageflip_surface_damage(transaction_id, pageflip_token)?;
            server.commit_surface_damage_presented(surface_damage);
            server.complete_direct_presented_frame_batch(
                direct_info.frame_id,
                direct_info.protocol_batch_id,
                direct_info.surface_id,
                presentation,
            );
            completed = Some((
                direct_info.target,
                direct_info.submit_started_at,
                direct_info.submit_returned_at,
            ));
            Ok(())
        },
    )?;
    let completion =
        scanout.complete_direct_pageflip(transaction_id, pageflip_token, presented_at)?;
    debug_assert_eq!(completion.transaction_id, transaction_id);
    debug_assert_eq!(completion.token, pageflip_token);
    let previous_assignment = *confirmed_primary_assignment;
    if previous_assignment.is_some_and(|assignment| assignment.is_direct()) {
        scanout.note_direct_replacement();
    } else {
        scanout.note_direct_entry();
        scanout.invalidate_presented_damage_history();
    }
    *confirmed_primary_assignment = Some(ConfirmedPrimaryAssignment::Direct {
        transaction_id,
        token: pageflip_token,
        surface_id: completion.surface_id,
        candidate_key: completion.candidate_key,
    });
    debug_assert_eq!(
        scanout.direct_scanout_presented_info(),
        Some((
            completion.surface_id,
            completion.framebuffer_id,
            completion.candidate_key.content.content_epoch.get(),
        ))
    );
    debug_assert!(output_transactions.transaction(transaction_id).is_none());
    let (target, submit_started_at, submit_returned_at) =
        completed.ok_or_else(|| io::Error::other("direct pageflip did not complete"))?;
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
    });
    *scheduled_presentation_target = None;
    Ok(())
}
