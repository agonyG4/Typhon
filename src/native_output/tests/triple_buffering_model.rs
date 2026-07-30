use super::*;
use crate::native_output::presentation::pipeline::{
    ConfirmedPrimaryState, OutputPipelineSnapshot, PipelineCommitKind, PipelineValidationError,
    PreparedCompositedState, QueuedCommitSnapshot, TripleCapability,
    validate_pipeline_owner_counts,
};
use std::num::NonZeroU64;

fn target(sequence: u64) -> PresentationTarget {
    PresentationTarget {
        sequence,
        presentation_time: MonotonicTimestampNs::new(sequence * 10),
        submit_not_before: MonotonicTimestampNs::new(sequence * 10 - 2),
        render_start_deadline: MonotonicTimestampNs::new(sequence * 10 - 4),
        refresh_interval: Duration::from_nanos(10),
        reason: PresentationTargetReason::PredictedPressure,
        clock_generation: 1,
        estimated: false,
        predicted_unreachable: false,
    }
}

fn transaction_id(value: u64) -> OutputTransactionId {
    OutputTransactionId::new(NonZeroU64::new(value).unwrap())
}

fn token(value: u64) -> PageFlipToken {
    PageFlipToken::new(value).unwrap()
}

fn composed_commit(
    transaction: u64,
    sequence: u64,
    slot: u8,
    framebuffer_id: u32,
) -> QueuedCommitSnapshot {
    QueuedCommitSnapshot {
        token: token(transaction),
        output_generation: 1,
        crtc_id: 7,
        target: target(sequence),
        kind: PipelineCommitKind::CompositedPrimary {
            transaction_id: transaction_id(transaction),
            frame_id: transaction,
            slot: OutputSlotId::new(slot).unwrap(),
            framebuffer_id,
        },
    }
}

fn empty_snapshot() -> OutputPipelineSnapshot {
    OutputPipelineSnapshot {
        output_generation: 1,
        pacing_mode: NativeOutputPacingMode::PredictiveTriple,
        current_primary: None,
        kernel_submitted: None,
        worker_queued_next: None,
        prepared: PreparedCompositedState::None,
        free_compositor_slots: 2,
        triple_capability: TripleCapability::Capable,
    }
}

#[test]
fn pipeline_snapshot_accepts_two_ordered_future_primaries() {
    let mut snapshot = empty_snapshot();
    snapshot.kernel_submitted = Some(composed_commit(1, 1, 0, 10));
    snapshot.worker_queued_next = Some(composed_commit(2, 2, 1, 11));

    assert_eq!(snapshot.future_primary_depth(), 2);
    assert_eq!(snapshot.validate(), Ok(()));
}

#[test]
fn pipeline_snapshot_rejects_three_future_primaries() {
    let mut snapshot = empty_snapshot();
    snapshot.kernel_submitted = Some(composed_commit(1, 1, 0, 10));
    snapshot.worker_queued_next = Some(composed_commit(2, 2, 1, 11));
    snapshot.prepared = PreparedCompositedState::Ready {
        transaction_id: transaction_id(3),
        slot: OutputSlotId::new(2).unwrap(),
        target: target(3),
        fence_state:
            crate::native_output::presentation::pipeline::PreparedFenceState::SubmitWithInFence,
    };

    assert_eq!(
        snapshot.validate(),
        Err(PipelineValidationError::FuturePrimaryDepthExceeded { depth: 3 })
    );
}

#[test]
fn pipeline_snapshot_rejects_old_output_generation() {
    let mut snapshot = empty_snapshot();
    let mut old = composed_commit(1, 1, 0, 10);
    old.output_generation = 9;
    snapshot.output_generation = 10;
    snapshot.kernel_submitted = Some(old);

    assert_eq!(
        snapshot.validate(),
        Err(PipelineValidationError::OutputGenerationMismatch {
            transaction_id: transaction_id(1),
            expected: 10,
            actual: 9,
        })
    );
}

#[test]
fn pipeline_snapshot_rejects_slot_aliasing() {
    let mut snapshot = empty_snapshot();
    snapshot.current_primary = Some(ConfirmedPrimaryState::Composed {
        transaction_id: transaction_id(9),
        token: token(9),
        slot: OutputSlotId::new(0).unwrap(),
    });
    snapshot.kernel_submitted = Some(composed_commit(1, 1, 0, 10));

    assert_eq!(
        snapshot.validate(),
        Err(PipelineValidationError::SlotAliasing {
            slot: OutputSlotId::new(0).unwrap(),
        })
    );
}

#[test]
fn pipeline_snapshot_rejects_non_monotonic_targets() {
    let mut snapshot = empty_snapshot();
    snapshot.kernel_submitted = Some(composed_commit(1, 2, 0, 10));
    snapshot.worker_queued_next = Some(composed_commit(2, 2, 1, 11));

    assert_eq!(
        snapshot.validate(),
        Err(PipelineValidationError::NonMonotonicTargetOrder {
            earlier_sequence: 2,
            later_sequence: 2,
        })
    );
}

#[test]
fn reactive_double_rejects_pending_plus_prepared() {
    let mut snapshot = empty_snapshot();
    snapshot.pacing_mode = NativeOutputPacingMode::ReactiveDouble;
    snapshot.kernel_submitted = Some(composed_commit(1, 1, 0, 10));
    snapshot.prepared = PreparedCompositedState::Rendering {
        slot: OutputSlotId::new(1).unwrap(),
        target: target(2),
    };

    assert_eq!(
        snapshot.validate(),
        Err(PipelineValidationError::ReactiveDoubleOwnsPreparedWithQueuedPrimary)
    );
}

#[test]
fn cursor_only_commit_does_not_increase_future_primary_depth() {
    let mut snapshot = empty_snapshot();
    snapshot.kernel_submitted = Some(QueuedCommitSnapshot {
        token: token(1),
        output_generation: 1,
        crtc_id: 7,
        target: target(1),
        kind: PipelineCommitKind::CursorOnly {
            transaction_id: transaction_id(1),
            cursor_epoch: 4,
            framebuffer_id: Some(10),
        },
    });

    assert_eq!(snapshot.future_primary_depth(), 0);
    assert_eq!(snapshot.validate(), Ok(()));
}

#[test]
fn direct_active_is_derived_only_from_confirmed_primary() {
    let mut snapshot = empty_snapshot();
    let direct_commit = PipelineCommitKind::DirectPrimary {
        transaction_id: transaction_id(1),
        key: test_direct_key(),
        framebuffer_id: 22,
    };
    snapshot.kernel_submitted = Some(QueuedCommitSnapshot {
        token: token(1),
        output_generation: 1,
        crtc_id: 7,
        target: target(1),
        kind: direct_commit,
    });
    assert!(!snapshot.direct_active());
    assert_eq!(direct_commit.compositor_slot(), None);

    snapshot.kernel_submitted = None;
    snapshot.current_primary = Some(ConfirmedPrimaryState::Direct {
        transaction_id: transaction_id(1),
        token: token(1),
        surface_id: 7,
        key: test_direct_key(),
        framebuffer_id: 22,
    });
    assert!(snapshot.direct_active());
}

#[test]
fn pipeline_owner_cardinality_rejects_duplicate_positions() {
    assert_eq!(
        validate_pipeline_owner_counts(2, 0, 0),
        Err(PipelineValidationError::KernelSubmittedCapacityExceeded { count: 2 })
    );
    assert_eq!(
        validate_pipeline_owner_counts(0, 2, 0),
        Err(PipelineValidationError::WorkerQueuedCapacityExceeded { count: 2 })
    );
    assert_eq!(
        validate_pipeline_owner_counts(0, 0, 2),
        Err(PipelineValidationError::PreparedCapacityExceeded { count: 2 })
    );
}

fn test_direct_key() -> DirectScanoutCandidateKey {
    DirectScanoutCandidateKey {
        content: OutputContentKey::new(
            7,
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
        output_generation: 1,
        cursor_content_key: None,
        color_epoch: 0,
    }
}
