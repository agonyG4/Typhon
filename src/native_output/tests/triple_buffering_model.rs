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
        presented_planes: crate::native_output::presentation::plane::PresentedPlaneSnapshot::legacy(
            None,
        ),
        current_primary: None,
        kernel_submitted: None,
        worker_queued_next: None,
        prepared: PreparedCompositedState::None,
        free_compositor_slots: 2,
        triple_capability: TripleCapability::Capable,
    }
}

fn with_current(mut snapshot: OutputPipelineSnapshot) -> OutputPipelineSnapshot {
    let current = ConfirmedPrimaryState::Composed {
        transaction_id: transaction_id(9),
        token: token(9),
        slot: OutputSlotId::new(0).unwrap(),
    };
    snapshot.current_primary = Some(current);
    snapshot.presented_planes.primary = Some(current);
    snapshot.free_compositor_slots = 2;
    snapshot
}

fn ready(transaction: u64, sequence: u64, slot: u8) -> PreparedCompositedState {
    PreparedCompositedState::Ready {
        transaction_id: transaction_id(transaction),
        slot: OutputSlotId::new(slot).unwrap(),
        target: target(sequence),
        fence_state:
            crate::native_output::presentation::pipeline::PreparedFenceState::SubmitWithInFence,
    }
}

#[test]
fn composed_reference_arrangements_cover_every_legal_two_future_pair() {
    let mut kernel_prepared = with_current(empty_snapshot());
    kernel_prepared.kernel_submitted = Some(composed_commit(1, 1, 1, 11));
    kernel_prepared.prepared = ready(2, 2, 2);
    kernel_prepared.free_compositor_slots = 0;
    assert_eq!(kernel_prepared.validate(), Ok(()));

    let mut kernel_worker = with_current(empty_snapshot());
    kernel_worker.kernel_submitted = Some(composed_commit(1, 1, 1, 11));
    kernel_worker.worker_queued_next = Some(composed_commit(2, 2, 2, 12));
    kernel_worker.free_compositor_slots = 0;
    assert_eq!(kernel_worker.validate(), Ok(()));

    let mut worker_prepared = with_current(empty_snapshot());
    worker_prepared.worker_queued_next = Some(composed_commit(1, 1, 1, 11));
    worker_prepared.prepared = ready(2, 2, 2);
    worker_prepared.free_compositor_slots = 0;
    assert_eq!(worker_prepared.validate(), Ok(()));

    let mut worker = with_current(empty_snapshot());
    worker.worker_queued_next = Some(composed_commit(1, 1, 1, 11));
    worker.free_compositor_slots = 1;
    assert_eq!(worker.validate(), Ok(()));

    let mut prepared = with_current(empty_snapshot());
    prepared.prepared = ready(1, 1, 1);
    prepared.free_compositor_slots = 1;
    assert_eq!(prepared.validate(), Ok(()));
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
    let current = ConfirmedPrimaryState::Composed {
        transaction_id: transaction_id(9),
        token: token(9),
        slot: OutputSlotId::new(0).unwrap(),
    };
    snapshot.current_primary = Some(current);
    snapshot.presented_planes.primary = Some(current);
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
fn kernel_cursor_only_allows_one_prepared_primary_but_forbids_pre_admission() {
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
    snapshot.prepared = ready(2, 2, 0);
    snapshot.free_compositor_slots = 1;

    assert!(snapshot.kernel_cursor_only());
    assert_eq!(snapshot.future_primary_depth(), 1);
    assert!(!snapshot.can_pre_admit_primary());
    assert_eq!(snapshot.validate(), Ok(()));
}

#[test]
fn scheduler_renders_then_holds_primary_behind_kernel_cursor_only() {
    let mut scheduler = NativeFrameScheduler::new(60, 0);
    scheduler.queue_visual_work();
    let mut snapshot = empty_snapshot();
    snapshot.pacing_mode = NativeOutputPacingMode::PredictiveTriple;
    snapshot.kernel_submitted = Some(QueuedCommitSnapshot {
        token: token(1),
        output_generation: 1,
        crtc_id: 7,
        target: target(1),
        kind: PipelineCommitKind::CursorOnly {
            transaction_id: transaction_id(1),
            cursor_epoch: 5,
            framebuffer_id: Some(10),
        },
    });
    snapshot.free_compositor_slots = 2;

    assert_eq!(
        scheduler.decision_with_pipeline(explicit_scheduler_context(30), &snapshot),
        SchedulerDecision::Render
    );

    snapshot.prepared = ready(2, 2, 0);
    snapshot.free_compositor_slots = 1;
    assert_eq!(
        scheduler.decision_with_pipeline(explicit_scheduler_context(31), &snapshot),
        SchedulerDecision::WaitForPageFlip
    );
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
    let current = ConfirmedPrimaryState::Direct {
        transaction_id: transaction_id(1),
        token: token(1),
        surface_id: 7,
        key: test_direct_key(),
        framebuffer_id: 22,
    };
    snapshot.current_primary = Some(current);
    snapshot.presented_planes.primary = Some(current);
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

fn explicit_scheduler_context(now: u64) -> ExplicitAtomicSchedulerContext {
    ExplicitAtomicSchedulerContext {
        now: MonotonicTimestampNs::new(now),
        predicted_total_cost: Duration::from_millis(1),
        presentation_target: None,
        render_ahead_allowed: false,
        worker_queue_available: false,
    }
}

#[test]
fn explicit_scheduler_derives_pending_wait_from_pipeline_snapshot() {
    let mut scheduler = NativeFrameScheduler::new(60, 0);
    scheduler.queue_visual_work();
    let mut snapshot = empty_snapshot();
    snapshot.pacing_mode = NativeOutputPacingMode::ReactiveDouble;
    snapshot.kernel_submitted = Some(composed_commit(1, 1, 0, 10));

    let decision =
        scheduler.decision_with_pipeline_diagnostics(explicit_scheduler_context(1), &snapshot);
    assert_eq!(decision.action, SchedulerDecision::WaitForBuffer);
    assert_eq!(
        decision.wait_reason,
        Some(PipelineWaitReason::KernelCommitPending)
    );
}

#[test]
fn explicit_scheduler_ignores_compatibility_submission_mirror() {
    let mut scheduler = NativeFrameScheduler::new(60, 0);
    scheduler.note_async_submission(99, 1).unwrap();
    scheduler.queue_visual_work();
    let mut snapshot = empty_snapshot();
    snapshot.pacing_mode = NativeOutputPacingMode::ReactiveDouble;

    assert_eq!(
        scheduler.decision_with_pipeline(explicit_scheduler_context(2), &snapshot),
        SchedulerDecision::Render,
    );
}

#[test]
fn prepared_primary_submission_does_not_depend_on_new_visual_demand() {
    let mut scheduler = NativeFrameScheduler::new(60, 0);
    let mut snapshot = with_current(empty_snapshot());
    snapshot.kernel_submitted = Some(composed_commit(1, 1, 1, 11));
    snapshot.prepared = ready(2, 2, 2);
    snapshot.free_compositor_slots = 0;
    let mut context = explicit_scheduler_context(19);
    context.render_ahead_allowed = true;
    context.worker_queue_available = true;

    assert_eq!(
        scheduler.decision_with_pipeline(context, &snapshot),
        SchedulerDecision::SubmitReady
    );
}

#[test]
fn worker_pre_admits_ready_primary_before_its_submit_not_before_deadline() {
    let mut scheduler = NativeFrameScheduler::new(60, 0);
    let mut snapshot = with_current(empty_snapshot());
    snapshot.kernel_submitted = Some(composed_commit(1, 1, 1, 11));
    snapshot.prepared = ready(2, 2, 2);
    snapshot.free_compositor_slots = 0;
    let mut context = explicit_scheduler_context(17);
    context.render_ahead_allowed = true;
    context.worker_queue_available = true;

    assert_eq!(
        scheduler.decision_with_pipeline(context, &snapshot),
        SchedulerDecision::SubmitReady
    );
}

#[test]
fn two_future_primaries_coalesce_new_visual_work_without_rendering_farther_ahead() {
    let mut scheduler = NativeFrameScheduler::new(60, 0);
    scheduler.queue_visual_work();
    let mut snapshot = with_current(empty_snapshot());
    snapshot.kernel_submitted = Some(composed_commit(1, 1, 1, 11));
    snapshot.worker_queued_next = Some(composed_commit(2, 2, 2, 12));
    snapshot.free_compositor_slots = 0;
    let mut context = explicit_scheduler_context(30);
    context.render_ahead_allowed = true;
    context.worker_queue_available = false;
    context.presentation_target = Some(target(3));

    let decision = scheduler.decision_with_pipeline_diagnostics(context, &snapshot);
    assert_eq!(decision.action, SchedulerDecision::WaitForWorkerQueue);
    assert_eq!(
        decision.wait_reason,
        Some(PipelineWaitReason::WorkerQueueOccupied)
    );
    assert!(scheduler.visual_work_queued());
}

#[test]
fn worker_queued_primary_plus_newer_visual_work_can_render_one_later_prepared_frame() {
    let mut scheduler = NativeFrameScheduler::new(60, 0);
    scheduler.queue_visual_work();
    let mut snapshot = with_current(empty_snapshot());
    snapshot.worker_queued_next = Some(composed_commit(1, 1, 1, 11));
    snapshot.free_compositor_slots = 1;
    let mut context = explicit_scheduler_context(30);
    context.render_ahead_allowed = true;
    context.presentation_target = Some(target(2));

    assert_eq!(
        scheduler.decision_with_pipeline(context, &snapshot),
        SchedulerDecision::RenderAhead
    );
}

#[test]
fn triple_capability_reports_one_exact_blocker_in_safety_order() {
    let capable = TripleCapabilityInputs {
        atomic_kms: true,
        explicit_swapchain: true,
        slot_capacity: 3,
        primary_in_fence: true,
        render_fence_export: true,
        submission_transport_healthy: true,
        session_active: true,
        output_generation_stable: true,
        ordinary_vsync: true,
        swapchain_poisoned: false,
    };
    assert_eq!(derive_triple_capability(capable), TripleCapability::Capable);
    assert_eq!(
        derive_triple_capability(TripleCapabilityInputs {
            primary_in_fence: false,
            submission_transport_healthy: false,
            ..capable
        }),
        TripleCapability::Unavailable(TripleCapabilityBlocker::PrimaryInFenceUnavailable)
    );
    assert_eq!(
        derive_triple_capability(TripleCapabilityInputs {
            ordinary_vsync: false,
            ..capable
        }),
        TripleCapability::Unavailable(TripleCapabilityBlocker::UnsupportedPresentationMode)
    );
}
