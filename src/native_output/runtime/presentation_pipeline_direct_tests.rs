use super::*;
use std::num::NonZeroU64;

fn target() -> PresentationTarget {
    PresentationTarget {
        sequence: 1,
        presentation_time: MonotonicTimestampNs::new(10),
        submit_not_before: MonotonicTimestampNs::new(8),
        render_start_deadline: MonotonicTimestampNs::new(6),
        refresh_interval: std::time::Duration::from_nanos(10),
        reason: PresentationTargetReason::ForcedValidation,
        clock_generation: 1,
        estimated: false,
        predicted_unreachable: false,
    }
}

fn key() -> DirectScanoutCandidateKey {
    key_with_generation(1)
}

fn key_with_generation(output_generation: u64) -> DirectScanoutCandidateKey {
    DirectScanoutCandidateKey {
        content: OutputContentKey::new(
            9,
            NonZeroU64::new(42).unwrap(),
            ContentEpochId::new(NonZeroU64::new(3).unwrap()),
            1920,
            1080,
            0x3432_5241,
            0,
            0,
            1_000,
            0,
        ),
        output_generation,
        cursor_content_key: None,
        color_epoch: 0,
    }
}

fn swapchain() -> AtomicOutputSwapchain {
    AtomicOutputSwapchain::from_presented_slots(
        OutputSlotSet::new([
            OutputSlotId::new(0).unwrap(),
            OutputSlotId::new(1).unwrap(),
            OutputSlotId::new(2).unwrap(),
        ])
        .unwrap(),
        OutputSlotId::new(0).unwrap(),
        1,
    )
    .unwrap()
}

fn presented_direct() -> (DirectPrimaryOwnership, PresentedPrimaryState) {
    presented_direct_with_generations(1, 1)
}

fn presented_direct_with_generations(
    key_generation: u64,
    pageflip_generation: u64,
) -> (DirectPrimaryOwnership, PresentedPrimaryState) {
    let key = key_with_generation(key_generation);
    let (lease, _cleanup_count) = DirectPrimaryLease::test_fixture_with_probe(key, 43);
    let transaction_id = OutputTransactionId::new(NonZeroU64::new(143).unwrap());
    let token = PageFlipToken::new(143).unwrap();
    let mut ownership = DirectPrimaryOwnership::default();
    ownership
        .accept_submitted(SubmittedDirectPrimary {
            transaction_id,
            token,
            lease,
            submit_started_at: MonotonicTimestampNs::new(11),
            submit_returned_at: MonotonicTimestampNs::new(12),
            out_fence: None,
            frame_id: 143,
            protocol_batch_id: oblivion_one::compositor::CompositorFrameBatchId::new(
                NonZeroU64::new(143).unwrap(),
            ),
            target: target(),
        })
        .unwrap();
    ownership
        .complete_pageflip(transaction_id, token, MonotonicTimestampNs::new(14))
        .unwrap();
    let pageflip = crate::native_output::presentation::plane::PlanePageflipIdentity::from_pageflip(
        token,
        pageflip_generation,
        7,
    );
    (
        ownership,
        PresentedPrimaryState::Direct {
            transaction_id,
            token,
            pageflip,
            surface_id: key.content.surface_id,
            key,
            framebuffer_id: 43,
        },
    )
}

#[test]
fn presented_direct_primary_rejects_stale_candidate_generation() {
    let (ownership, current) = presented_direct_with_generations(2, 1);
    let transaction_id = current.transaction_id();

    assert_eq!(
        validate_presented_primary(current, &swapchain(), Some(&ownership), 1, 7),
        Err(PipelineSnapshotError::IdentityMismatch {
            owner: "current_direct",
            field: "output_generation",
            transaction_id,
        })
    );
}

#[test]
fn presented_direct_primary_survives_origin_history_eviction() {
    let (ownership, current) = presented_direct();
    let origin = OutputTransactionId::new(NonZeroU64::new(143).unwrap());
    let mut ledger = OutputTransactionLedger::with_capacities(8, 1);
    let insert_direct = |id, batch| {
        OutputTransaction::direct(
            id,
            1,
            MonotonicTimestampNs::new(0),
            target(),
            NativeOutputPacingMode::ReactiveDouble,
            143,
            key(),
            43,
            None,
            batch,
            9,
            OutputReleasePlan::Pageflip,
        )
        .unwrap()
    };
    ledger
        .insert(insert_direct(
            origin,
            oblivion_one::compositor::CompositorFrameBatchId::new(NonZeroU64::new(143).unwrap()),
        ))
        .unwrap();
    ledger
        .mark_dropped(
            origin,
            OutputTransactionDropReason::NoVisualChange,
            MonotonicTimestampNs::new(1),
        )
        .unwrap();
    let unrelated = OutputTransactionId::new(NonZeroU64::new(144).unwrap());
    ledger
        .insert(insert_direct(
            unrelated,
            oblivion_one::compositor::CompositorFrameBatchId::new(NonZeroU64::new(144).unwrap()),
        ))
        .unwrap();
    ledger
        .mark_dropped(
            unrelated,
            OutputTransactionDropReason::NoVisualChange,
            MonotonicTimestampNs::new(2),
        )
        .unwrap();
    assert!(ledger.transaction_including_terminal(origin).is_none());

    assert_eq!(
        validate_presented_primary(current, &swapchain(), None, 1, 7),
        Err(PipelineSnapshotError::MissingPresentedDirectOwnership {
            transaction_id: origin,
        })
    );
    assert_eq!(
        validate_presented_primary(current, &swapchain(), Some(&ownership), 1, 7),
        Ok(())
    );
}
