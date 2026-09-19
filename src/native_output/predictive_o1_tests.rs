use super::*;
use crate::native_output::OutputTransactionId;
use crate::native_output::scanout::OutputFrameKey;
use oblivion_one::compositor::CompositorFrameBatchId;
use oblivion_one::native::kms::FramebufferId;
use oblivion_one::native::presentation_deadline::{
    MonotonicTimestampNs, PresentationTarget, PrimaryRefreshClaim,
};
use std::num::NonZeroU64;

fn physical_identity(frame_id: u64) -> OutputFrameIdentitySnapshot {
    OutputFrameIdentitySnapshot {
        output_id: oblivion_one::core::OutputId::from_raw(1).expect("test output id"),
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
            .physical_identity_for_attempt(PredictiveO1AttemptId::new(successor_attempt.get())),
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
    assert_eq!(reserved.physical_identity(), Some(physical));
    assert_eq!(
        reserved.physical_key(),
        Some(OutputFrameKey::from(&physical))
    );

    pacing
        .note_worker_submit_exact(
            Some(reserved),
            41,
            3,
            NativeOutputPacingMode::PredictiveTriple,
        )
        .expect("worker submission should consume the exact physical identity");
    pacing.note_pageflip_exact(Some(physical), 4, 3, 41, 6_060);

    assert_ne!(reserved.frame_id().get(), physical.frame_id);
    assert_ne!(attempt.get(), physical.frame_id);
    assert_eq!(pacing.predictive_o1_presented, 1);
    assert_eq!(pacing.predictive_o1_lifecycle.active_entries(), 0);
    assert_eq!(pacing.predictive_o1_invalid_stage_transitions, 0);
}

#[test]
fn worker_aba_overlap_stress_reconciles_predictive_lifecycles() {
    let mut pacing = NativeFramePacing::from_env();
    pacing.enabled = true;
    let mut successful_successors = 0;

    for iteration in 0..10_000u64 {
        // Deliberately reuse the same logical scheduler ID on every iteration.
        // Physical output identities and worker reservation capabilities remain
        // distinct and must reconcile independently.
        pacing.ids = NativeOutputFrameIdSequence::new(1);
        pacing.queue_visual(iteration + 1, iteration + 1);
        let logical_frame = pacing.active.expect("normal predecessor");
        let predecessor_ticket = pacing
            .reserve_worker_submission(false)
            .expect("predecessor reservation")
            .expect("predecessor ticket");

        pacing
            .note_render_started(NativeOutputPacingMode::PredictiveTriple, true)
            .expect("predictive successor render");
        let successor_attempt = pacing.active_predictive_attempt.expect("successor attempt");
        let physical = physical_identity(20_000 + iteration + 1);
        pacing
            .bind_predictive_o1(physical)
            .expect("successor physical binding");
        pacing.note_render_ready();
        pacing.note_ready_frame(iteration + 2, true);
        pacing.note_predictive_unbound_ready();
        pacing.note_predictive_binding_after_render_completion(0);

        assert_eq!(predecessor_ticket.frame_id(), logical_frame);
        assert_eq!(pacing.ready, Some(logical_frame));
        assert_eq!(pacing.ready_predictive_attempt, Some(successor_attempt));
        assert_eq!(
            pacing.ready_physical_key,
            Some(OutputFrameKey::from(&physical))
        );

        let predecessor_token = iteration.saturating_mul(2).saturating_add(1);
        let successor_token = predecessor_token.saturating_add(1);
        if iteration % 2 == 0 {
            pacing
                .note_worker_submit_exact(
                    Some(predecessor_ticket),
                    predecessor_token,
                    iteration + 3,
                    NativeOutputPacingMode::PredictiveTriple,
                )
                .expect("predecessor success");
            assert_eq!(pacing.ready_predictive_attempt, Some(successor_attempt));
            pacing.note_pageflip_exact(
                None,
                iteration + 4,
                iteration + 3,
                predecessor_token,
                6_060,
            );

            let successor_ticket = pacing
                .reserve_worker_submission(true)
                .expect("successor reservation")
                .expect("successor ticket");
            pacing
                .note_worker_submit_exact(
                    Some(successor_ticket),
                    successor_token,
                    iteration + 5,
                    NativeOutputPacingMode::PredictiveTriple,
                )
                .expect("successor success");
            pacing.note_pageflip_exact(
                Some(physical),
                iteration + 6,
                iteration + 5,
                successor_token,
                6_060,
            );
            successful_successors += 1;
        } else {
            assert!(pacing.cancel_worker_submission(Some(predecessor_ticket)));
            assert_eq!(pacing.ready_predictive_attempt, Some(successor_attempt));

            let successor_ticket = pacing
                .reserve_worker_submission(true)
                .expect("successor reservation after predecessor cancellation")
                .expect("successor ticket after predecessor cancellation");
            assert!(pacing.cancel_worker_submission(Some(successor_ticket)));
        }

        assert_eq!(pacing.predictive_o1_lifecycle.active_entries(), 0);
        assert_eq!(pacing.predictive_o1_invalid_stage_transitions, 0);
        assert!(pacing.worker_reservation.is_none());
        assert!(pacing.active.is_none());
        assert!(pacing.ready.is_none());
    }

    assert_eq!(successful_successors, 5_000);
    assert_eq!(pacing.predictive_o1_created, 10_000);
    assert_eq!(pacing.predictive_o1_bound, 10_000);
    assert_eq!(pacing.predictive_o1_submitted, 5_000);
    assert_eq!(pacing.predictive_o1_presented, 5_000);
    assert_eq!(pacing.predictive_o1_failed, 5_000);
    assert_eq!(pacing.predictive_o1_invalid_stage_transitions, 0);
    assert_eq!(pacing.predictive_o1_lifecycle.active_entries(), 0);
    assert!(pacing.predictive_o1_lifecycle.peak_entries <= 4);
}

#[test]
fn deferred_target_mutation_does_not_change_physical_lifecycle_identity() {
    let mut pacing = NativeFramePacing::from_env();
    pacing.enabled = true;
    let physical = physical_identity(5_677);
    let attempt = predictive_ready_frame(&mut pacing, physical, false);
    pacing.note_submit(41, 2, true, NativeOutputPacingMode::PredictiveTriple);

    let mut completed = physical;
    let target_time = MonotonicTimestampNs::new(12_121_212);
    completed.target = Some(PresentationTarget {
        sequence: 2,
        presentation_time: target_time,
        submit_not_before: target_time,
        render_start_deadline: target_time,
        refresh_interval: std::time::Duration::from_nanos(6_060_606),
        reason:
            oblivion_one::native::presentation_deadline::PresentationTargetReason::PredictedPressure,
        clock_generation: 1,
        estimated: false,
        predicted_unreachable: false,
        physical_claim: PrimaryRefreshClaim {
            sequence: 2,
            presentation_time: target_time,
            clock_generation: 1,
        },
        selection_evidence: Default::default(),
    });

    assert_ne!(physical, completed);
    assert_eq!(
        OutputFrameKey::from(&physical),
        OutputFrameKey::from(&completed)
    );
    pacing.note_pageflip_exact(Some(completed), 3, 2, 41, 6_060);

    assert_eq!(attempt.get(), 1);
    assert_eq!(pacing.predictive_o1_presented, 1);
    assert_eq!(pacing.predictive_o1_invalid_stage_transitions, 0);
    assert_eq!(pacing.predictive_o1_lifecycle.active_entries(), 0);
}

#[test]
fn normal_predecessor_pacing_id_cannot_touch_predictive_successor() {
    let mut pacing = NativeFramePacing::from_env();
    pacing.enabled = true;
    pacing.ids = NativeOutputFrameIdSequence::new(5_615);
    pacing.predictive_o1_attempt_ids = PredictiveO1AttemptIdSequence::new(9_001);

    let successor = physical_identity(5_520);
    pacing.queue_visual(1, successor.render_generation);
    let _ = pacing.note_render_started(NativeOutputPacingMode::PredictiveTriple, true);
    let attempt = pacing
        .active_predictive_attempt
        .expect("predictive attempt");
    pacing
        .bind_predictive_o1(successor)
        .expect("bind successor");
    pacing.note_render_ready();
    pacing.note_ready_frame(2, true);

    // Model the runtime overlap: the physical predecessor is represented by
    // a normal pacing ID which numerically matches the old attempt namespace.
    // Its ownership tag is intentionally absent.
    assert_eq!(pacing.ready, Some(NativeOutputFrameId(5_615)));
    pacing.pending = Some(NativeOutputFrameId(5_615));
    pacing.pending_physical_identity = Some(physical_identity(5_615));
    pacing.pending_physical_key = Some(OutputFrameKey::from(
        &pacing.pending_physical_identity.expect("predecessor state"),
    ));
    pacing.pending_predictive_attempt = None;
    pacing.pending_token = Some(41);

    pacing.note_pageflip_exact(Some(physical_identity(5_615)), 3, 2, 41, 6_060);

    assert_ne!(attempt.get(), 5_615);
    assert_eq!(pacing.predictive_o1_invalid_stage_transitions, 0);
    assert_eq!(pacing.predictive_o1_lifecycle.active_entries(), 1);
    assert_eq!(
        pacing
            .predictive_o1_lifecycle
            .physical_identity_for_attempt(attempt),
        Some(successor)
    );
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
                pacing.ready.expect("successor remains ready").get()
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

    for iteration in 0..10_000_u64 {
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

    for sequence in 0..10_000_u64 {
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

    assert_eq!(pacing.predictive_o1_created, 10_000);
    assert_eq!(pacing.predictive_o1_presented, 10_000);
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
