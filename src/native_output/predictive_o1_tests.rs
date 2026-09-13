use super::*;
use crate::native_output::OutputTransactionId;
use oblivion_one::compositor::CompositorFrameBatchId;
use oblivion_one::native::kms::FramebufferId;
use std::num::NonZeroU64;

fn physical_identity(frame_id: u64) -> OutputFrameIdentitySnapshot {
    OutputFrameIdentitySnapshot {
        frame_id,
        protocol_batch_id: CompositorFrameBatchId::new(
            NonZeroU64::new(frame_id).expect("test frame batch ID"),
        ),
        transaction_id: OutputTransactionId::new(
            NonZeroU64::new(frame_id).expect("test transaction ID"),
        ),
        slot: super::super::scanout::OutputSlotId::new((frame_id % 3) as u8)
            .expect("test output slot"),
        framebuffer_id: FramebufferId::new(frame_id as u32).expect("test framebuffer ID"),
        render_generation: frame_id,
        pool_generation: 1,
        target: None,
    }
}

fn predictive_ready_frame(
    pacing: &mut NativeFramePacing,
    physical: OutputFrameIdentitySnapshot,
    bind_to_ready: bool,
) -> NativeOutputFrameId {
    pacing.queue_visual(physical.frame_id, physical.render_generation);
    pacing.note_render_decision(NativeOutputPacingMode::PredictiveTriple, true);
    pacing
        .begin_render_attempt(NativeOutputPacingMode::PredictiveTriple, true)
        .expect("predictive render attempt");
    let attempt = pacing.active.expect("active predictive attempt");
    pacing
        .bind_predictive_o1(physical)
        .expect("bind predictive attempt to physical output frame");
    pacing.note_render_ready();
    pacing.note_ready_frame(physical.frame_id, true);
    if bind_to_ready {
        pacing.note_predictive_unbound_ready();
        pacing.note_predictive_binding_after_render_completion(0);
    }
    attempt
}

#[test]
fn predictive_no_visual_change_never_admits_a_lifecycle() {
    let mut pacing = NativeFramePacing::from_env();
    pacing.enabled = true;

    for iteration in 0..(PREDICTIVE_O1_LIFECYCLE_CAPACITY * 8) {
        pacing.queue_visual(iteration as u64 + 1, iteration as u64 + 1);
        pacing.note_render_decision(NativeOutputPacingMode::PredictiveTriple, true);

        // This is the production no-primary-work outcome: damage resolution
        // retires the logical scene without ever admitting a physical render.
        assert_eq!(pacing.predictive_o1_lifecycle.active_entries(), 0);
        assert!(pacing.active_predictive_attempt_id().is_none());
        pacing.cancel_unsubmitted_render();
        assert!(pacing.active.is_none());
    }

    assert_eq!(pacing.predictive_o1_created, 0);
    assert_eq!(pacing.predictive_o1_lifecycle.active_entries(), 0);
    assert!(
        pacing
            .summary_line(0, 0)
            .contains("predictive_o1_terminal_reconciled=true")
    );
}

#[test]
fn unreachable_submit_window_cancels_predictive_decision_before_admission() {
    let mut pacing = NativeFramePacing::from_env();
    pacing.enabled = true;

    for iteration in 0..(PREDICTIVE_O1_LIFECYCLE_CAPACITY * 8) {
        pacing.queue_visual(iteration as u64 + 1, iteration as u64 + 1);
        pacing.note_render_decision(NativeOutputPacingMode::PredictiveTriple, true);

        // The explicit Atomic submit-window rejection happens before the
        // backend render call and therefore before Predictive O1 admission.
        assert_eq!(pacing.predictive_o1_lifecycle.active_entries(), 0);
        pacing.cancel_unsubmitted_render();
        assert_eq!(pacing.predictive_o1_lifecycle.active_entries(), 0);
        assert!(pacing.active.is_none());
    }

    assert_eq!(pacing.predictive_o1_created, 0);
}

#[test]
fn backend_failure_after_admission_removes_the_unbound_attempt() {
    let mut pacing = NativeFramePacing::from_env();
    pacing.enabled = true;
    pacing.queue_visual(1, 1);
    pacing.note_render_decision(NativeOutputPacingMode::PredictiveTriple, true);
    pacing
        .begin_render_attempt(NativeOutputPacingMode::PredictiveTriple, true)
        .expect("physical render admission should create one attempt");

    assert_eq!(pacing.predictive_o1_created, 1);
    assert_eq!(pacing.predictive_o1_lifecycle.active_entries(), 1);
    pacing.note_predictive_o1_failed();
    pacing.cancel_unsubmitted_render();

    assert_eq!(pacing.predictive_o1_bound, 0);
    assert_eq!(pacing.predictive_o1_failed, 1);
    assert_eq!(pacing.predictive_o1_lifecycle.active_entries(), 0);
    assert!(pacing.active.is_none());
}

#[test]
fn predictive_pageflip_uses_physical_identity_when_logical_ids_diverge() {
    let mut pacing = NativeFramePacing::from_env();
    pacing.enabled = true;
    let predecessor = physical_identity(5_677);
    let successor = physical_identity(5_690);

    let predecessor_attempt = predictive_ready_frame(&mut pacing, predecessor, false);
    pacing.note_submit(41, 2, true, NativeOutputPacingMode::PredictiveTriple);
    let successor_attempt = predictive_ready_frame(&mut pacing, successor, false);

    assert_ne!(predecessor_attempt.get(), predecessor.frame_id);
    assert_ne!(successor_attempt.get(), successor.frame_id);
    assert_ne!(predecessor.frame_id, successor.frame_id);

    pacing.note_pageflip_exact(Some(predecessor), 3, 2, 41, 6_060);
    assert_eq!(
        pacing
            .predictive_o1_lifecycle
            .physical_identity_for_attempt(PredictiveO1AttemptId::new(successor_attempt)),
        Some(successor)
    );

    pacing.note_submit(42, 4, true, NativeOutputPacingMode::PredictiveTriple);
    pacing.note_pageflip_exact(Some(successor), 5, 4, 42, 6_060);

    assert_eq!(pacing.predictive_o1_presented, 2);
    assert_eq!(pacing.predictive_o1_lifecycle.active_entries(), 0);
}

#[test]
fn predictive_worker_submission_and_pageflip_share_one_physical_identity() {
    let mut pacing = NativeFramePacing::from_env();
    pacing.enabled = true;
    let physical = physical_identity(5_677);
    let attempt = predictive_ready_frame(&mut pacing, physical, true);
    let reserved = pacing
        .reserve_worker_submission(true)
        .expect("reserve physical predictive frame")
        .expect("predictive attempt ID");
    assert_eq!(pacing.worker_submission_output_identity(), Some(physical));

    pacing
        .note_worker_submit_exact(
            Some(reserved),
            Some(physical),
            41,
            3,
            true,
            NativeOutputPacingMode::PredictiveTriple,
        )
        .expect("worker submission should consume the exact physical identity");
    pacing.note_pageflip_exact(Some(physical), 4, 3, 41, 6_060);

    assert_eq!(reserved, attempt.get());
    assert_eq!(pacing.predictive_o1_presented, 1);
    assert_eq!(pacing.predictive_o1_lifecycle.active_entries(), 0);
    assert_eq!(pacing.predictive_o1_invalid_stage_transitions, 0);
}

#[test]
fn predecessor_pageflip_cannot_terminalize_a_predictive_successor() {
    let mut pacing = NativeFramePacing::from_env();
    pacing.enabled = true;
    let predecessor = physical_identity(100);
    let successor = physical_identity(300);

    predictive_ready_frame(&mut pacing, predecessor, false);
    pacing.note_submit(41, 2, true, NativeOutputPacingMode::PredictiveTriple);
    predictive_ready_frame(&mut pacing, successor, false);

    pacing.note_pageflip_exact(Some(predecessor), 3, 2, 41, 6_060);

    assert_eq!(pacing.predictive_o1_lifecycle.active_entries(), 1);
    assert_eq!(
        pacing
            .predictive_o1_lifecycle
            .physical_identity_for_attempt(PredictiveO1AttemptId::new(
                pacing.ready.expect("successor remains ready")
            )),
        Some(successor)
    );
}

#[test]
fn abandoned_bound_predictive_frame_is_reusable_and_late_completion_is_harmless() {
    let mut pacing = NativeFramePacing::from_env();
    pacing.enabled = true;
    let physical = physical_identity(7_001);
    predictive_ready_frame(&mut pacing, physical, false);

    assert!(pacing.abandon_ready_frame());
    assert_eq!(pacing.predictive_o1_lifecycle.active_entries(), 0);
    pacing.note_pageflip_exact(Some(physical), 3, 2, 41, 6_060);
    assert_eq!(pacing.predictive_o1_lifecycle.active_entries(), 0);

    for frame in 0..PREDICTIVE_O1_LIFECYCLE_CAPACITY {
        let physical = physical_identity(7_100 + frame as u64);
        predictive_ready_frame(&mut pacing, physical, false);
        pacing.note_submit(
            100 + frame as u64,
            4,
            true,
            NativeOutputPacingMode::PredictiveTriple,
        );
        pacing.note_pageflip_exact(Some(physical), 5, 4, 100 + frame as u64, 6_060);
    }
    assert_eq!(pacing.predictive_o1_lifecycle.active_entries(), 0);
}

#[test]
fn predictive_failure_before_physical_binding_creates_no_physical_lifecycle() {
    let mut pacing = NativeFramePacing::from_env();
    pacing.enabled = true;
    pacing.queue_visual(1, 1);
    pacing
        .note_render_started(NativeOutputPacingMode::PredictiveTriple, true)
        .expect("predictive render attempt");
    pacing.note_predictive_o1_failed();

    assert_eq!(pacing.predictive_o1_bound, 0);
    assert_eq!(pacing.predictive_o1_lifecycle.active_entries(), 0);
    assert!(
        pacing
            .summary_line(0, 0)
            .contains("predictive_o1_terminal_reconciled=true")
    );
}

#[test]
fn ordinary_output_frames_do_not_create_predictive_stage_events() {
    let mut pacing = NativeFramePacing::from_env();
    pacing.enabled = true;
    pacing.queue_visual(1, 1);
    pacing
        .note_render_started(NativeOutputPacingMode::ReactiveDouble, false)
        .expect("reactive render attempt");
    pacing.note_render_ready();
    pacing.note_ready_frame(2, false);
    pacing.note_submit(41, 3, true, NativeOutputPacingMode::ReactiveDouble);
    pacing.note_pageflip(4, 3, 41, 6_060);

    assert_eq!(pacing.predictive_o1_stale_stage_events, 0);
    assert_eq!(pacing.predictive_o1_invalid_stage_transitions, 0);
    assert_eq!(pacing.predictive_o1_lifecycle.active_entries(), 0);
}

#[test]
fn unbound_predictive_completion_is_terminalized_without_claiming_another_frame() {
    let mut pacing = NativeFramePacing::from_env();
    pacing.enabled = true;
    pacing.queue_visual(1, 1);
    pacing
        .note_render_started(NativeOutputPacingMode::PredictiveTriple, true)
        .expect("predictive render attempt");
    pacing.note_ready_frame(2, true);
    pacing.note_submit(41, 3, true, NativeOutputPacingMode::PredictiveTriple);

    pacing.note_pageflip_exact(Some(physical_identity(7_777)), 4, 3, 41, 6_060);

    assert_eq!(pacing.predictive_o1_lifecycle.active_entries(), 0);
    assert_eq!(pacing.predictive_o1_invalid_stage_terminalized, 1);
}

#[test]
fn invalid_physical_pageflip_terminalizes_only_the_expected_lifecycle() {
    let mut pacing = NativeFramePacing::from_env();
    pacing.enabled = true;
    let expected = physical_identity(8_001);
    let actual = physical_identity(8_099);
    predictive_ready_frame(&mut pacing, expected, true);
    pacing.note_submit(41, 2, true, NativeOutputPacingMode::PredictiveTriple);

    pacing.note_pageflip_exact(Some(actual), 3, 2, 41, 6_060);

    assert_eq!(pacing.predictive_o1_lifecycle.active_entries(), 0);
    assert_eq!(pacing.predictive_o1_invalid_stage_transitions, 1);
    assert_eq!(pacing.predictive_o1_invalid_stage_terminalized, 1);
    assert!(
        pacing
            .summary_line(0, 0)
            .contains("predictive_o1_terminal_reconciled=true")
    );
}

#[test]
fn mixed_prephysical_and_physical_stress_reconciles_without_capacity_growth() {
    let mut pacing = NativeFramePacing::from_env();
    pacing.enabled = true;

    for iteration in 0..1_000_u64 {
        let physical = physical_identity(20_000 + iteration * 3);
        pacing.queue_visual(iteration + 1, iteration + 1);
        pacing.note_render_decision(NativeOutputPacingMode::PredictiveTriple, true);

        match iteration % 4 {
            0 => {
                // NoVisualChange after exact damage resolution.
                pacing.cancel_unsubmitted_render();
            }
            1 => {
                // The target became unreachable before physical rendering.
                pacing.cancel_unsubmitted_render();
            }
            2 => {
                // The backend was admitted but failed before producing a
                // physical output frame.
                pacing
                    .begin_render_attempt(NativeOutputPacingMode::PredictiveTriple, true)
                    .expect("pre-physical backend-failure attempt");
                pacing.note_predictive_o1_failed();
                pacing.cancel_unsubmitted_render();
            }
            3 => {
                // A successful physical frame intentionally diverges from
                // both the logical attempt and its predecessor namespace.
                predictive_ready_frame(&mut pacing, physical, false);
                pacing.note_submit(
                    40_000 + iteration,
                    3,
                    true,
                    NativeOutputPacingMode::PredictiveTriple,
                );
                pacing.note_pageflip_exact(Some(physical), 4, 3, 40_000 + iteration, 6_060);
            }
            _ => unreachable!(),
        }

        assert_eq!(pacing.predictive_o1_lifecycle.active_entries(), 0);
        assert!(
            pacing.predictive_o1_lifecycle.peak_entries <= PREDICTIVE_O1_LIFECYCLE_CAPACITY as u64
        );
    }

    assert_eq!(pacing.predictive_o1_lifecycle.active_entries(), 0);
    assert!(
        pacing
            .summary_line(0, 0)
            .contains("predictive_o1_terminal_reconciled=true")
    );
}

#[test]
fn long_predictive_physical_identity_stress_reconciles_without_capacity_growth() {
    let mut pacing = NativeFramePacing::from_env();
    pacing.enabled = true;

    for sequence in 0..1_000_u64 {
        let physical = physical_identity(50_000 + sequence * 7);
        predictive_ready_frame(&mut pacing, physical, false);
        pacing.note_submit(
            sequence + 1,
            sequence + 2,
            true,
            NativeOutputPacingMode::PredictiveTriple,
        );
        pacing.note_pageflip_exact(
            Some(physical),
            sequence + 3,
            sequence + 2,
            sequence + 1,
            6_060,
        );
    }

    assert_eq!(pacing.predictive_o1_created, 1_000);
    assert_eq!(pacing.predictive_o1_presented, 1_000);
    assert_eq!(pacing.predictive_o1_lifecycle.active_entries(), 0);
    assert!(pacing.predictive_o1_lifecycle.peak_entries <= PREDICTIVE_O1_LIFECYCLE_CAPACITY as u64);
    assert_eq!(pacing.predictive_o1_invalid_stage_transitions, 0);
    assert_eq!(pacing.predictive_o1_stale_stage_events, 0);
    assert!(
        pacing
            .summary_line(0, 0)
            .contains("predictive_o1_terminal_reconciled=true")
    );
}
