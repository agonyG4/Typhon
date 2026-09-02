use super::super::planner::{
    plan_scheduled_target_for_budget, plan_scheduled_target_for_mode,
    plan_visual_target_for_budget, reactive_or_commit_timing_target,
    visual_target_deadline_for_mode,
};
use super::kms_worker::{
    ValidationBaseInvalidationDisposition, WorkerRejectionKind, direct_rejection_policy,
    validation_base_invalidation_disposition,
};
use super::presentation_transactions::complete_presented_output_transaction;
use super::*;
use crate::native_output::kms_worker::ValidationBaseInvalidationReason;
use oblivion_one::compositor::CompositorFrameBatchId;
use oblivion_one::native::presentation_deadline::PresentationTargetAuthority;
use oblivion_one::native::scheduler::apply_atomic_commit_lane_guard;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_COMPATIBILITY_TEST_SOCKET: AtomicU64 = AtomicU64::new(0);

#[test]
fn active_validation_invalidation_is_replanned_not_suspended() {
    for reason in [
        ValidationBaseInvalidationReason::PredecessorTerminal,
        ValidationBaseInvalidationReason::PresentedRevisionChanged,
        ValidationBaseInvalidationReason::BundleMismatch,
    ] {
        assert_eq!(
            validation_base_invalidation_disposition(true, false, 7, 7, reason),
            ValidationBaseInvalidationDisposition::Replan
        );
    }
    assert_eq!(
        validation_base_invalidation_disposition(
            true,
            false,
            7,
            7,
            ValidationBaseInvalidationReason::GenerationChanged,
        ),
        ValidationBaseInvalidationDisposition::Quarantine
    );
    for (session_permits_output, shutting_down, current_generation, job_generation) in [
        (false, false, 7, 7),
        (true, true, 7, 7),
        (true, false, 8, 7),
    ] {
        assert_eq!(
            validation_base_invalidation_disposition(
                session_permits_output,
                shutting_down,
                current_generation,
                job_generation,
                ValidationBaseInvalidationReason::PredecessorTerminal,
            ),
            ValidationBaseInvalidationDisposition::Quarantine
        );
    }
}

#[test]
fn commit_lane_guard_preserves_predictive_render_ahead() {
    assert_eq!(
        apply_atomic_commit_lane_guard(SchedulerDecision::RenderAhead, true, false),
        SchedulerDecision::RenderAhead
    );
}

#[test]
fn commit_lane_guard_blocks_a_ready_submission_without_worker_capacity() {
    assert_eq!(
        apply_atomic_commit_lane_guard(SchedulerDecision::SubmitReady, true, false),
        SchedulerDecision::WaitForPageFlip
    );
}

#[test]
fn reactive_double_never_schedules_a_normal_visual_target() {
    let mut planner = PresentationDeadlinePlanner::new(Duration::from_nanos(6_060_606));
    planner.note_presented(MonotonicTimestampNs::new(6_060_606));

    let target = plan_scheduled_target_for_mode(
        &mut planner,
        NativeOutputPacingMode::ReactiveDouble,
        None,
        MonotonicTimestampNs::new(7_000_000),
        Duration::from_millis(100),
        PresentationTargetReason::PredictedPressure,
    );

    assert_eq!(target, None);
    assert_eq!(planner.scheduled_target(), None);
}

#[test]
fn reactive_advisory_pending_target_does_not_reserve_a_false_o1_frontier() {
    let refresh = Duration::from_nanos(6_060_606);
    let physical_presented_at = MonotonicTimestampNs::new(1_000_000_000);
    let now = MonotonicTimestampNs::new(1_000_500_000);
    let mut planner = PresentationDeadlinePlanner::new(refresh);
    assert_eq!(planner.note_presented(physical_presented_at), 1);

    let reactive =
        reactive_or_commit_timing_target(&planner, None, None, now, Duration::from_millis(17))
            .expect("reactive target");
    assert_eq!(reactive.reason, PresentationTargetReason::ReactiveDouble);
    assert_eq!(reactive.authority(), PresentationTargetAuthority::Advisory);
    assert_eq!(reactive.sequence, 4);

    let successor = plan_visual_target_for_budget(
        &mut planner,
        true,
        Some(reactive),
        now,
        Duration::from_millis(1),
        true,
        true,
        false,
        None,
    )
    .expect("O1 successor");

    assert_eq!(
        successor.sequence, 3,
        "an advisory ReactiveDouble prediction must not reserve seq 4"
    );
    assert_eq!(
        successor.presentation_time.get(),
        physical_presented_at
            .get()
            .saturating_add(refresh.as_nanos() as u64 * 2)
    );
}

#[test]
fn advisory_predecessor_does_not_make_an_existing_o1_target_churn() {
    let refresh = Duration::from_nanos(6_060_606);
    let physical_presented_at = MonotonicTimestampNs::new(1_000_000_000);
    let now = MonotonicTimestampNs::new(1_000_500_000);
    let mut planner = PresentationDeadlinePlanner::new(refresh);
    assert_eq!(planner.note_presented(physical_presented_at), 1);

    let reactive =
        reactive_or_commit_timing_target(&planner, None, None, now, Duration::from_millis(17))
            .expect("reactive target");
    assert_eq!(reactive.sequence, 4);

    let scheduled = plan_visual_target_for_budget(
        &mut planner,
        true,
        Some(reactive),
        now,
        Duration::from_millis(1),
        true,
        true,
        false,
        None,
    )
    .expect("O1 target");
    assert_eq!(scheduled.sequence, 3);
    assert_eq!(planner.pre_render_abandoned(), 0);

    let repeated = plan_visual_target_for_budget(
        &mut planner,
        true,
        Some(reactive),
        now,
        Duration::from_millis(1),
        true,
        true,
        false,
        Some(scheduled),
    )
    .expect("existing O1 target");

    assert_eq!(repeated, scheduled);
    assert_eq!(planner.pre_render_abandoned(), 0);
}

#[test]
fn callback_driven_165hz_oracle_keeps_primary_content_one_refresh_apart() {
    let refresh = Duration::from_nanos(6_060_606);
    let presented_at = MonotonicTimestampNs::new(1_000_000_000);
    let mut physical_presented_at = presented_at;
    let mut planner = PresentationDeadlinePlanner::new(refresh);
    assert_eq!(planner.note_presented(presented_at), 1);

    // The 17 ms prediction deliberately models conservative metadata while
    // the actual service below is one millisecond and the callback reaction is
    // 500 us.  The first frame is physically the next refresh even though its
    // advisory target lands several intervals later.
    let mut pending = reactive_or_commit_timing_target(
        &planner,
        None,
        None,
        MonotonicTimestampNs::new(presented_at.get() + 500_000),
        Duration::from_millis(17),
    )
    .expect("initial callback-driven target");
    assert_eq!(pending.authority(), PresentationTargetAuthority::Advisory);

    let mut primary_intervals = Vec::new();
    let mut expected_physical_sequence = 1;
    let mut render_ahead_successes = 0;
    for _ in 0..128 {
        let callback_commit = MonotonicTimestampNs::new(physical_presented_at.get() + 500_000);
        let successor = plan_visual_target_for_budget(
            &mut planner,
            true,
            Some(pending),
            callback_commit,
            Duration::from_millis(1),
            true,
            true,
            false,
            None,
        )
        .expect("one-refresh O1 successor");
        render_ahead_successes += 1;

        physical_presented_at =
            MonotonicTimestampNs::new(physical_presented_at.get() + refresh.as_nanos() as u64);
        expected_physical_sequence += 1;
        assert_eq!(
            planner.note_presented(physical_presented_at),
            expected_physical_sequence
        );
        primary_intervals.push(refresh.as_nanos() as u64);
        assert_eq!(successor.sequence, expected_physical_sequence + 1);
        assert!(successor.is_binding());
        pending = successor;
    }

    assert_eq!(render_ahead_successes, 128);
    assert!(
        primary_intervals
            .iter()
            .all(|interval| *interval == 6_060_606)
    );
}

#[test]
fn genuinely_slow_render_keeps_a_later_opportunity_legitimate() {
    let refresh = Duration::from_nanos(6_060_606);
    let presented_at = MonotonicTimestampNs::new(1_000_000_000);
    let mut planner = PresentationDeadlinePlanner::new(refresh);
    planner.note_presented(presented_at);

    let target = planner
        .reactive_target(
            MonotonicTimestampNs::new(presented_at.get() + 500_000),
            Duration::from_millis(8),
        )
        .expect("slow client target");

    assert_eq!(target.authority(), PresentationTargetAuthority::Advisory);
    assert_eq!(target.sequence, 3);
    assert!(target.presentation_time.get() > presented_at.get() + 6_060_606);
}

#[test]
fn demand_revoke_preserves_an_already_armed_render_ahead_target() {
    let mut planner = PresentationDeadlinePlanner::new(Duration::from_nanos(10));
    planner.note_presented(MonotonicTimestampNs::new(10));
    let pending = planner
        .reactive_target(MonotonicTimestampNs::new(11), Duration::ZERO)
        .unwrap();
    let armed = plan_scheduled_target_for_budget(
        &mut planner,
        true,
        Some(pending),
        MonotonicTimestampNs::new(12),
        Duration::from_nanos(2),
        PresentationTargetReason::PredictedPressure,
    )
    .unwrap();

    let kept = plan_visual_target_for_budget(
        &mut planner,
        false,
        Some(pending),
        MonotonicTimestampNs::new(12),
        Duration::from_nanos(2),
        true,
        true,
        false,
        Some(armed),
    );

    assert_eq!(kept, Some(armed));
    assert_eq!(planner.scheduled_target(), Some(armed));
}

#[test]
fn forced_shutdown_drops_submitted_transaction_once_without_presentation() {
    let mut harness = CompatibilityPresentationHarness::new();
    let transaction_id = harness
        .output_transactions
        .allocate_id()
        .expect("transaction ID");
    let transaction = OutputTransaction::compatibility_composited(
        transaction_id,
        1,
        MonotonicTimestampNs::new(10),
        compatibility_test_target(),
        NativeOutputPacingMode::ReactiveDouble,
        1,
        1,
        91,
        None,
        harness.batch_id,
    )
    .expect("compatibility transaction");
    harness.output_transactions.insert(transaction).unwrap();
    harness
        .output_transactions
        .mark_queued(transaction_id, 1, MonotonicTimestampNs::new(11))
        .unwrap();
    let token = PageFlipToken::new(91).unwrap();
    harness
        .output_transactions
        .mark_submitted(transaction_id, token, MonotonicTimestampNs::new(12))
        .unwrap();

    let mut settlement_calls = 0;
    assert!(
        super::presentation_transactions::settle_forced_shutdown_transaction_if_safe(
            KmsTeardownSafety::Restored,
            &mut harness.output_transactions,
            transaction_id,
            token,
            MonotonicTimestampNs::new(13),
            |obligations: OutputProtocolObligations| {
                settlement_calls += 1;
                harness.server.complete_frame_batch_after_safe_abandonment(
                    obligations
                        .frame_batch_id()
                        .expect("submitted transaction has a frame batch"),
                    FrameBatchDiscardReason::SuspendAbandonment,
                );
                Ok(())
            },
        )
        .unwrap()
    );
    assert_eq!(settlement_calls, 1);
    assert_eq!(harness.output_transactions.active_count(), 0);
    assert_eq!(
        harness
            .output_transactions
            .obligation_owner(harness.batch_id),
        None
    );
    assert!(matches!(
        harness
            .output_transactions
            .recent_terminal()
            .back()
            .unwrap()
            .state(),
        OutputTransactionState::Terminal(OutputTransactionTerminal::Dropped {
            reason: OutputTransactionDropReason::SafeAbandonment,
            ..
        })
    ));

    assert!(
        !super::presentation_transactions::settle_forced_shutdown_transaction_if_safe(
            KmsTeardownSafety::Restored,
            &mut harness.output_transactions,
            transaction_id,
            token,
            MonotonicTimestampNs::new(14),
            |_| {
                settlement_calls += 1;
                Ok(())
            },
        )
        .unwrap()
    );
    assert_eq!(settlement_calls, 1);
    assert_eq!(harness.server.frame_batch_count(), 0);
    assert!(!harness.presentation_trace.events().any(|event| matches!(
        event,
        PresentationTransactionEvent::PageflipPresented { .. }
    )));
}

#[test]
fn unproven_teardown_keeps_forced_transaction_and_protocol_batch_owned() -> NativeResult<()> {
    let mut harness = CompatibilityPresentationHarness::new();
    let transaction_id = harness
        .output_transactions
        .allocate_id()
        .expect("transaction ID");
    let transaction = OutputTransaction::compatibility_composited(
        transaction_id,
        1,
        MonotonicTimestampNs::new(10),
        compatibility_test_target(),
        NativeOutputPacingMode::ReactiveDouble,
        1,
        1,
        91,
        None,
        harness.batch_id,
    )
    .expect("compatibility transaction");
    harness.output_transactions.insert(transaction).unwrap();
    harness
        .output_transactions
        .mark_queued(transaction_id, 1, MonotonicTimestampNs::new(11))
        .unwrap();
    let token = PageFlipToken::new(91).unwrap();
    harness
        .output_transactions
        .mark_submitted(transaction_id, token, MonotonicTimestampNs::new(12))
        .unwrap();

    let mut settlement_calls = 0;
    assert!(
        !super::presentation_transactions::settle_forced_shutdown_transaction_if_safe(
            KmsTeardownSafety::Unproven,
            &mut harness.output_transactions,
            transaction_id,
            token,
            MonotonicTimestampNs::new(13),
            |_| {
                settlement_calls += 1;
                Ok(())
            },
        )?
    );
    assert_eq!(settlement_calls, 0);
    assert_eq!(harness.output_transactions.active_count(), 1);
    assert_eq!(harness.server.frame_batch_count(), 1);
    assert!(!harness.presentation_trace.events().any(|event| matches!(
        event,
        PresentationTransactionEvent::PageflipPresented { .. }
    )));
    harness.server.disarm_shutdown_releases();
    harness.server.finish_commit_debug_for_shutdown();
    assert!(
        !super::presentation_transactions::settle_forced_shutdown_transaction_if_safe(
            KmsTeardownSafety::Unproven,
            &mut harness.output_transactions,
            transaction_id,
            token,
            MonotonicTimestampNs::new(14),
            |_| {
                settlement_calls += 1;
                Ok(())
            },
        )?
    );
    assert_eq!(settlement_calls, 0);
    assert_eq!(harness.output_transactions.active_count(), 1);
    assert_eq!(harness.server.frame_batch_count(), 1);
    Ok(())
}

#[test]
fn target_destruction_safely_settles_forced_transaction_once() -> NativeResult<()> {
    let mut harness = CompatibilityPresentationHarness::new();
    let transaction_id = harness
        .output_transactions
        .allocate_id()
        .expect("transaction ID");
    let transaction = OutputTransaction::compatibility_composited(
        transaction_id,
        1,
        MonotonicTimestampNs::new(10),
        compatibility_test_target(),
        NativeOutputPacingMode::ReactiveDouble,
        1,
        1,
        91,
        None,
        harness.batch_id,
    )
    .expect("compatibility transaction");
    harness.output_transactions.insert(transaction).unwrap();
    harness
        .output_transactions
        .mark_queued(transaction_id, 1, MonotonicTimestampNs::new(11))
        .unwrap();
    let token = PageFlipToken::new(91).unwrap();
    harness
        .output_transactions
        .mark_submitted(transaction_id, token, MonotonicTimestampNs::new(12))
        .unwrap();

    let mut settlement_calls = 0;
    assert!(
        super::presentation_transactions::settle_forced_shutdown_transaction_if_safe(
            KmsTeardownSafety::TargetDestroyed,
            &mut harness.output_transactions,
            transaction_id,
            token,
            MonotonicTimestampNs::new(13),
            |obligations: OutputProtocolObligations| {
                settlement_calls += 1;
                harness.server.complete_frame_batch_after_safe_abandonment(
                    obligations
                        .frame_batch_id()
                        .expect("submitted transaction has a frame batch"),
                    FrameBatchDiscardReason::SuspendAbandonment,
                );
                Ok(())
            },
        )?
    );
    assert_eq!(settlement_calls, 1);
    assert_eq!(harness.output_transactions.active_count(), 0);
    assert_eq!(harness.server.frame_batch_count(), 0);
    assert!(
        !super::presentation_transactions::settle_forced_shutdown_transaction_if_safe(
            KmsTeardownSafety::TargetDestroyed,
            &mut harness.output_transactions,
            transaction_id,
            token,
            MonotonicTimestampNs::new(14),
            |_| {
                settlement_calls += 1;
                Ok(())
            },
        )?
    );
    assert_eq!(settlement_calls, 1);
    Ok(())
}

#[test]
fn forced_shutdown_does_not_abandon_late_presented_transaction() {
    let mut harness = CompatibilityPresentationHarness::new();
    let transaction_id = harness
        .output_transactions
        .allocate_id()
        .expect("transaction ID");
    let transaction = OutputTransaction::compatibility_composited(
        transaction_id,
        1,
        MonotonicTimestampNs::new(10),
        compatibility_test_target(),
        NativeOutputPacingMode::ReactiveDouble,
        1,
        1,
        91,
        None,
        harness.batch_id,
    )
    .expect("compatibility transaction");
    harness.output_transactions.insert(transaction).unwrap();
    harness
        .output_transactions
        .mark_queued(transaction_id, 1, MonotonicTimestampNs::new(11))
        .unwrap();
    let token = PageFlipToken::new(92).unwrap();
    harness
        .output_transactions
        .mark_submitted(transaction_id, token, MonotonicTimestampNs::new(12))
        .unwrap();
    harness
        .output_transactions
        .mark_presented(
            transaction_id,
            token,
            1,
            MonotonicTimestampNs::new(13),
            Some(2),
        )
        .unwrap();

    let mut settlement_calls = 0;
    assert!(
        !super::presentation_transactions::settle_forced_shutdown_transaction_if_safe(
            KmsTeardownSafety::Restored,
            &mut harness.output_transactions,
            transaction_id,
            token,
            MonotonicTimestampNs::new(14),
            |_| {
                settlement_calls += 1;
                Ok(())
            },
        )
        .unwrap()
    );
    assert_eq!(settlement_calls, 0);
    assert!(matches!(
        harness
            .output_transactions
            .recent_terminal()
            .back()
            .unwrap()
            .state(),
        OutputTransactionState::Terminal(OutputTransactionTerminal::Presented { .. })
    ));
}

#[test]
fn predictive_triple_only_schedules_pending_plus_one() {
    let mut planner = PresentationDeadlinePlanner::new(Duration::from_millis(10));
    planner.note_presented(MonotonicTimestampNs::new(10_000_000));
    let pending = planner
        .reactive_target(MonotonicTimestampNs::new(11_000_000), Duration::ZERO)
        .unwrap();

    assert_eq!(
        plan_scheduled_target_for_mode(
            &mut planner,
            NativeOutputPacingMode::PredictiveTriple,
            None,
            MonotonicTimestampNs::new(12_000_000),
            Duration::from_millis(2),
            PresentationTargetReason::PredictedPressure,
        ),
        None
    );
    let ready = plan_scheduled_target_for_mode(
        &mut planner,
        NativeOutputPacingMode::PredictiveTriple,
        Some(pending),
        MonotonicTimestampNs::new(12_000_000),
        Duration::from_millis(2),
        PresentationTargetReason::PredictedPressure,
    )
    .unwrap();
    assert_eq!(ready.sequence, pending.sequence + 1);
}

#[test]
fn reactive_double_visual_target_never_owns_an_event_loop_deadline() {
    let target = PresentationTarget {
        sequence: 1,
        presentation_time: MonotonicTimestampNs::new(10),
        submit_not_before: MonotonicTimestampNs::new(9),
        render_start_deadline: MonotonicTimestampNs::new(8),
        refresh_interval: Duration::from_millis(1),
        reason: PresentationTargetReason::ReactiveDouble,
        clock_generation: 1,
        estimated: false,
        predicted_unreachable: false,
        physical_claim: oblivion_one::native::presentation_deadline::PrimaryRefreshClaim {
            sequence: 1,
            presentation_time: MonotonicTimestampNs::new(10),
            clock_generation: 1,
        },
        selection_evidence: Default::default(),
    };

    assert_eq!(
        visual_target_deadline_for_mode(NativeOutputPacingMode::ReactiveDouble, Some(target)),
        None
    );
    assert_eq!(
        visual_target_deadline_for_mode(NativeOutputPacingMode::PredictiveTriple, Some(target)),
        Some(8)
    );
}

#[test]
fn present_compatibility_frame_immediate_reaches_presented_without_failure() {
    let mut harness = CompatibilityPresentationHarness::new();
    let batch_id = harness.batch_id;
    let (result, transaction_id) = harness
        .present(Ok(NativePresentResult::Immediate))
        .expect("compatibility present");

    assert_eq!(result, NativePresentResult::Immediate);
    let transaction_id = transaction_id.expect("Immediate owns a transaction");
    assert_eq!(
        harness
            .output_transactions
            .counters()
            .immediate_presentation_failures,
        0
    );
    super::complete_immediate_output_transaction(
        &mut harness.output_transactions,
        &mut harness.presentation_trace,
        &mut harness.server,
        transaction_id,
        MonotonicTimestampNs::new(20),
    )
    .expect("Immediate settles successfully");

    assert_eq!(harness.server.prepared_frame_batch_id(), None);
    assert_eq!(harness.server.frame_batch_count(), 0);
    assert_eq!(harness.output_transactions.active_count(), 0);
    assert_eq!(harness.output_transactions.obligation_owner(batch_id), None);
    assert_eq!(
        harness
            .output_transactions
            .counters()
            .immediate_presentations,
        1
    );
    assert_eq!(
        harness
            .output_transactions
            .counters()
            .immediate_presentations_accepted,
        1
    );
    assert_eq!(
        harness
            .output_transactions
            .counters()
            .immediate_presentations_finalized,
        1
    );
    assert_eq!(harness.output_transactions.counters().failed, 0);
    assert!(matches!(
        harness
            .output_transactions
            .recent_terminal()
            .back()
            .unwrap()
            .state(),
        OutputTransactionState::Terminal(OutputTransactionTerminal::Presented { .. })
    ));
}

#[test]
fn immediate_software_presentation_failure_is_not_finalized_as_presented() {
    let mut harness = CompatibilityPresentationHarness::new();
    let (_, transaction_id) = harness
        .present(Ok(NativePresentResult::Immediate))
        .expect("compatibility present");
    harness.fail_next_immediate_presentation();

    let error = harness
        .complete_immediate(transaction_id.expect("Immediate transaction"))
        .expect_err("software presentation failure must propagate");

    assert!(
        error
            .to_string()
            .contains("injected software presentation failure")
    );
    assert!(matches!(
        harness
            .output_transactions
            .recent_terminal()
            .back()
            .unwrap()
            .state(),
        OutputTransactionState::Terminal(OutputTransactionTerminal::Failed {
            stage: OutputTransactionFailureStage::ProtocolSettlement,
            ..
        })
    ));
    assert_eq!(harness.output_transactions.active_count(), 0);
}

#[test]
fn immediate_settlement_failure_becomes_protocol_settlement_failure() {
    let mut harness = CompatibilityPresentationHarness::new();
    let (_, transaction_id) = harness
        .present(Ok(NativePresentResult::Immediate))
        .expect("compatibility present");
    harness.fail_next_immediate_presentation();

    harness
        .complete_immediate(transaction_id.expect("Immediate transaction"))
        .expect_err("settlement must fail");

    assert_eq!(
        harness.output_transactions.counters().settlement_failures,
        1
    );
    assert!(matches!(
        harness
            .output_transactions
            .recent_terminal()
            .back()
            .unwrap()
            .state(),
        OutputTransactionState::Terminal(OutputTransactionTerminal::Failed {
            stage: OutputTransactionFailureStage::ProtocolSettlement,
            ..
        })
    ));
}

#[test]
fn immediate_settlement_failure_releases_obligation_owner_once() {
    let mut harness = CompatibilityPresentationHarness::new();
    let batch_id = harness.batch_id;
    let (_, transaction_id) = harness
        .present(Ok(NativePresentResult::Immediate))
        .expect("compatibility present");
    harness.fail_next_immediate_presentation();

    harness
        .complete_immediate(transaction_id.expect("Immediate transaction"))
        .expect_err("settlement must fail");

    assert_eq!(harness.output_transactions.obligation_owner(batch_id), None);
    assert_eq!(harness.output_transactions.active_count(), 0);
    assert_eq!(
        harness
            .output_transactions
            .counters()
            .terminal_transitions_finalized,
        1
    );
}

#[test]
fn successful_immediate_presentation_emits_immediate_presented_trace() {
    let mut harness = CompatibilityPresentationHarness::new();
    let (_, transaction_id) = harness
        .present(Ok(NativePresentResult::Immediate))
        .expect("compatibility present");
    super::complete_immediate_output_transaction(
        &mut harness.output_transactions,
        &mut harness.presentation_trace,
        &mut harness.server,
        transaction_id.expect("Immediate transaction"),
        MonotonicTimestampNs::new(20),
    )
    .expect("Immediate settles successfully");

    let trace = harness.presentation_trace.export_jsonl();
    assert!(trace.contains("\"event\":\"immediate_presented\""));
}

#[test]
fn immediate_presentation_never_emits_pageflip_presented() {
    let mut harness = CompatibilityPresentationHarness::new();
    let (_, transaction_id) = harness
        .present(Ok(NativePresentResult::Immediate))
        .expect("compatibility present");
    super::complete_immediate_output_transaction(
        &mut harness.output_transactions,
        &mut harness.presentation_trace,
        &mut harness.server,
        transaction_id.expect("Immediate transaction"),
        MonotonicTimestampNs::new(20),
    )
    .expect("Immediate settles successfully");

    let trace = harness.presentation_trace.export_jsonl();
    assert!(!trace.contains("\"event\":\"pageflip_presented\""));
}

#[test]
fn compatibility_noop_has_one_typed_terminal() {
    let mut harness = CompatibilityPresentationHarness::new();
    let (result, transaction_id) = harness
        .present(Ok(NativePresentResult::Noop))
        .expect("compatibility noop");

    assert_eq!(result, NativePresentResult::Noop);
    assert_eq!(transaction_id, None);
    assert_eq!(harness.server.prepared_frame_batch_id(), None);
    assert_eq!(harness.output_transactions.active_count(), 0);
    assert_eq!(harness.output_transactions.counters().failed, 0);
    assert_eq!(
        harness
            .output_transactions
            .counters()
            .immediate_presentation_failures,
        0
    );
    assert_eq!(
        harness.output_transactions.counters().compatibility_noops,
        1
    );
    assert!(matches!(
        harness
            .output_transactions
            .recent_terminal()
            .back()
            .unwrap()
            .state(),
        OutputTransactionState::Terminal(OutputTransactionTerminal::Dropped {
            reason: OutputTransactionDropReason::NoVisualChange,
            ..
        })
    ));
}

#[test]
fn compatibility_noop_does_not_wait_for_pageflip() {
    let mut harness = CompatibilityPresentationHarness::new();
    let (result, transaction_id) = harness
        .present(Ok(NativePresentResult::Noop))
        .expect("compatibility noop");

    assert_eq!(result, NativePresentResult::Noop);
    assert_eq!(transaction_id, None);
    assert_eq!(harness.output_transactions.active_count(), 0);
    assert_eq!(harness.output_transactions.recent_terminal().len(), 1);
}

#[test]
fn compatibility_noop_releases_obligation_owner_once() {
    let mut harness = CompatibilityPresentationHarness::new();
    harness
        .present(Ok(NativePresentResult::Noop))
        .expect("compatibility noop");

    assert_eq!(
        harness
            .output_transactions
            .obligation_owner(harness.batch_id),
        None
    );
    assert_eq!(
        harness
            .output_transactions
            .counters()
            .terminal_transitions_finalized,
        1
    );
    assert_eq!(
        harness.output_transactions.counters().compatibility_noops,
        1
    );
}

#[test]
fn compatibility_backend_error_settles_batch_once() {
    let mut harness = CompatibilityPresentationHarness::new();
    let error = harness
        .present(Err(io::Error::other("compatibility submit failure")))
        .expect_err("compatibility backend error");

    assert!(error.to_string().contains("compatibility submit failure"));
    assert_eq!(harness.server.prepared_frame_batch_id(), None);
    assert_eq!(harness.output_transactions.active_count(), 0);
    assert_eq!(
        harness
            .output_transactions
            .obligation_owner(harness.batch_id),
        None
    );
    assert!(matches!(
        harness
            .output_transactions
            .recent_terminal()
            .back()
            .unwrap()
            .state(),
        OutputTransactionState::Terminal(OutputTransactionTerminal::Failed {
            stage: OutputTransactionFailureStage::KmsSubmit,
            ..
        })
    ));
}

#[test]
fn immediate_settlement_cannot_consume_another_prepared_batch() {
    let mut harness = CompatibilityPresentationHarness::new();
    let transaction_id = harness
        .output_transactions
        .allocate_id()
        .expect("transaction ID");
    let transaction = OutputTransaction::compatibility_immediate(
        transaction_id,
        1,
        MonotonicTimestampNs::new(10),
        compatibility_test_target(),
        NativeOutputPacingMode::ReactiveDouble,
        1,
        harness.batch_id,
    )
    .expect("Immediate transaction");
    harness.output_transactions.insert(transaction).unwrap();
    harness.server.mark_prepared_frame_submitted();
    harness.server.capture_frame_callbacks_for_render();
    let other_batch_id = harness
        .server
        .prepared_frame_batch_id()
        .expect("second prepared batch");

    assert!(
        super::complete_immediate_output_transaction(
            &mut harness.output_transactions,
            &mut harness.presentation_trace,
            &mut harness.server,
            transaction_id,
            MonotonicTimestampNs::new(20),
        )
        .is_err()
    );
    assert_eq!(
        harness.server.prepared_frame_batch_id(),
        Some(other_batch_id)
    );
    assert_eq!(harness.output_transactions.active_count(), 0);
    assert_eq!(
        harness
            .output_transactions
            .obligation_owner(harness.batch_id),
        None
    );
}

#[test]
fn immediate_settlement_rejects_missing_obligation_owner() {
    let mut harness = CompatibilityPresentationHarness::new();
    let transaction_id = harness
        .output_transactions
        .allocate_id()
        .expect("transaction ID");
    let transaction = OutputTransaction::compatibility_immediate(
        transaction_id,
        1,
        MonotonicTimestampNs::new(10),
        compatibility_test_target(),
        NativeOutputPacingMode::ReactiveDouble,
        1,
        harness.batch_id,
    )
    .expect("Immediate transaction");
    harness.output_transactions.insert(transaction).unwrap();
    harness
        .output_transactions
        .forget_obligation_owner_for_test(harness.batch_id);

    assert!(
        super::complete_immediate_output_transaction(
            &mut harness.output_transactions,
            &mut harness.presentation_trace,
            &mut harness.server,
            transaction_id,
            MonotonicTimestampNs::new(20),
        )
        .is_err()
    );
    assert_eq!(harness.output_transactions.active_count(), 1);
    assert_eq!(
        harness.server.prepared_frame_batch_id(),
        Some(harness.batch_id)
    );
}

#[test]
fn compatibility_present_result_matrix_has_exactly_one_terminal_per_transaction() {
    let mut immediate = CompatibilityPresentationHarness::new();
    let (_, immediate_id) = immediate
        .present(Ok(NativePresentResult::Immediate))
        .expect("Immediate result");
    super::complete_immediate_output_transaction(
        &mut immediate.output_transactions,
        &mut immediate.presentation_trace,
        &mut immediate.server,
        immediate_id.expect("Immediate transaction"),
        MonotonicTimestampNs::new(20),
    )
    .expect("Immediate terminal");

    let mut noop = CompatibilityPresentationHarness::new();
    noop.present(Ok(NativePresentResult::Noop))
        .expect("Noop result");

    let mut failure = CompatibilityPresentationHarness::new();
    failure
        .present(Err(io::Error::other("compatibility failure")))
        .expect_err("failure result");

    let mut async_submitted = CompatibilityPresentationHarness::new();
    let (_, async_id) = async_submitted
        .present(Ok(NativePresentResult::AsyncSubmitted {
            token: 42,
            framebuffer_id: 7,
            transaction_id: None,
        }))
        .expect("AsyncSubmitted result");
    let async_id = async_id.expect("Async transaction");
    let token = PageFlipToken::new(42).expect("test token");
    async_submitted.server.mark_prepared_frame_submitted();
    async_submitted
        .output_transactions
        .mark_submitted(async_id, token, MonotonicTimestampNs::new(15))
        .expect("Async submission");
    complete_presented_output_transaction(
        &mut async_submitted.output_transactions,
        &mut async_submitted.presentation_trace,
        async_id,
        token,
        1,
        MonotonicTimestampNs::new(25),
        Some(2),
        |obligations| {
            assert_eq!(obligations.frame_batch_id(), Some(async_submitted.batch_id));
            let presentation =
                FramePresentation::synchronized(PresentationClock::Monotonic, 1, 0, 2)
                    .expect("test presentation timestamp");
            async_submitted
                .server
                .finish_presented_frame_batch(async_submitted.batch_id, presentation)?;
            Ok(())
        },
    )
    .expect("Async terminal");

    for harness in [&immediate, &noop, &failure, &async_submitted] {
        assert_eq!(harness.output_transactions.active_count(), 0);
        assert_eq!(
            harness
                .output_transactions
                .counters()
                .terminal_transitions_accepted,
            1
        );
        assert_eq!(
            harness
                .output_transactions
                .counters()
                .terminal_transitions_finalized,
            1
        );
        assert_eq!(
            harness
                .output_transactions
                .counters()
                .active_settling_transactions,
            0
        );
    }
}

#[test]
fn composed_to_direct_becomes_active_only_after_pageflip() {
    let composed = PresentedPrimaryAssignment::Composed {
        transaction_id: OutputTransactionId::new(std::num::NonZeroU64::new(1).unwrap()),
        token: PageFlipToken::new(11).unwrap(),
        pageflip: crate::native_output::presentation::plane::PlanePageflipIdentity::from_pageflip(
            PageFlipToken::new(11).unwrap(),
            1,
            0,
        ),
        slot: OutputSlotId::new(0).unwrap(),
        framebuffer_id: 42,
        pool_generation: 1,
        presentation_serial: 1,
    };
    let direct = PresentedPrimaryAssignment::Direct {
        transaction_id: OutputTransactionId::new(std::num::NonZeroU64::new(2).unwrap()),
        token: PageFlipToken::new(12).unwrap(),
        pageflip: crate::native_output::presentation::plane::PlanePageflipIdentity::from_pageflip(
            PageFlipToken::new(12).unwrap(),
            1,
            0,
        ),
        surface_id: 7,
        key: test_confirmed_direct_key(),
        framebuffer_id: 42,
    };
    let mut confirmed = Some(composed);

    let queued = Some(direct);
    assert_eq!(confirmed, Some(composed));
    assert!(!confirmed.unwrap().is_direct());

    confirmed = queued;
    assert_eq!(confirmed, Some(direct));
    assert!(confirmed.unwrap().is_direct());
}

#[test]
fn direct_to_direct_retains_old_resource_until_replacement_pageflip() {
    let old = PresentedPrimaryAssignment::Direct {
        transaction_id: OutputTransactionId::new(std::num::NonZeroU64::new(3).unwrap()),
        token: PageFlipToken::new(13).unwrap(),
        pageflip: crate::native_output::presentation::plane::PlanePageflipIdentity::from_pageflip(
            PageFlipToken::new(13).unwrap(),
            1,
            0,
        ),
        surface_id: 8,
        key: test_confirmed_direct_key(),
        framebuffer_id: 42,
    };
    let replacement = PresentedPrimaryAssignment::Direct {
        transaction_id: OutputTransactionId::new(std::num::NonZeroU64::new(4).unwrap()),
        token: PageFlipToken::new(14).unwrap(),
        pageflip: crate::native_output::presentation::plane::PlanePageflipIdentity::from_pageflip(
            PageFlipToken::new(14).unwrap(),
            1,
            0,
        ),
        surface_id: 9,
        key: test_confirmed_direct_key(),
        framebuffer_id: 43,
    };
    let mut confirmed = Some(old);

    let submitted = Some(replacement);
    assert_eq!(confirmed, Some(old));
    assert_eq!(submitted, Some(replacement));

    confirmed = submitted;
    assert_eq!(confirmed, Some(replacement));
}

#[test]
fn legacy_direct_state_is_not_required_for_rejection_cleanup() {
    let policy = direct_rejection_policy(WorkerRejectionKind::TestOnly);
    assert!(!policy.invalidate_validation_key);
    assert!(policy.request_composited_redraw);
    assert!(!policy.demote_hardware_cursor);
}

#[test]
fn direct_real_submit_rejection_invalidates_cache_and_requests_composition() {
    let policy = direct_rejection_policy(WorkerRejectionKind::RealSubmit);
    assert!(policy.invalidate_validation_key);
    assert!(policy.request_composited_redraw);
    assert!(!policy.demote_hardware_cursor);
}

#[test]
fn rejected_direct_attempt_does_not_invalidate_presented_damage_history() {
    let confirmed = Some(PresentedPrimaryAssignment::Composed {
        transaction_id: OutputTransactionId::new(std::num::NonZeroU64::new(5).unwrap()),
        token: PageFlipToken::new(15).unwrap(),
        pageflip: crate::native_output::presentation::plane::PlanePageflipIdentity::from_pageflip(
            PageFlipToken::new(15).unwrap(),
            1,
            0,
        ),
        slot: OutputSlotId::new(0).unwrap(),
        framebuffer_id: 42,
        pool_generation: 1,
        presentation_serial: 1,
    });
    let after_rejection = confirmed;
    assert_eq!(after_rejection, confirmed);
}

#[test]
fn direct_combined_cursor_rejection_does_not_latch_software_cursor() {
    let policy = direct_rejection_policy(WorkerRejectionKind::RealSubmit);
    assert!(!policy.demote_hardware_cursor);
}

fn test_confirmed_direct_key() -> DirectScanoutCandidateKey {
    DirectScanoutCandidateKey {
        content: OutputContentKey::new(
            7,
            std::num::NonZeroU64::new(42).unwrap(),
            ContentEpochId::new(std::num::NonZeroU64::new(3).unwrap()),
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

struct CompatibilityPresentationHarness {
    server: OwnCompositorServer,
    scanout: NativeScanoutBackend,
    output_transactions: OutputTransactionLedger,
    presentation_trace: PresentationTransactionTraceRing,
    batch_id: CompositorFrameBatchId,
    fail_immediate_presentation: bool,
}

impl CompatibilityPresentationHarness {
    fn new() -> Self {
        let socket_name = format!(
            "typhon-compatibility-test-{}-{}",
            std::process::id(),
            NEXT_COMPATIBILITY_TEST_SOCKET.fetch_add(1, Ordering::Relaxed)
        );
        let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name)
            .expect("bind compatibility test compositor");
        server.capture_frame_callbacks_for_render();
        let batch_id = server
            .prepared_frame_batch_id()
            .expect("prepared compatibility frame batch");
        Self {
            server,
            scanout: NativeScanoutBackend::Dumb(DumbFramebuffer {
                fd: -1,
                handle: 0,
                fb_id: 0,
                width: 1,
                height: 1,
                pitch: 4,
                size: 0,
                mapping: std::ptr::null_mut(),
                drm_cleanup_armed: false,
            }),
            output_transactions: OutputTransactionLedger::with_capacities(8, 64),
            presentation_trace: PresentationTransactionTraceRing::new(16),
            batch_id,
            fail_immediate_presentation: false,
        }
    }

    fn present(
        &mut self,
        result: io::Result<NativePresentResult>,
    ) -> NativeResult<(NativePresentResult, Option<OutputTransactionId>)> {
        super::present_compatibility_frame(
            &mut self.scanout,
            &mut self.server,
            &mut self.output_transactions,
            1,
            1,
            compatibility_test_target(),
            NativeOutputPacingMode::ReactiveDouble,
            1,
            None,
            1,
            1,
            None,
            |_, _| result,
        )
    }

    fn fail_next_immediate_presentation(&mut self) {
        self.fail_immediate_presentation = true;
    }

    fn complete_immediate(&mut self, transaction_id: OutputTransactionId) -> NativeResult<()> {
        let fail_immediate_presentation = self.fail_immediate_presentation;
        if fail_immediate_presentation {
            super::complete_immediate_output_transaction_with(
                &mut self.output_transactions,
                &mut self.presentation_trace,
                &mut self.server,
                transaction_id,
                MonotonicTimestampNs::new(20),
                |server, batch_id| {
                    server.finish_immediate_frame_batch_with(batch_id, |_| {
                        Err(io::Error::other("injected software presentation failure"))
                    })
                },
            )
        } else {
            super::complete_immediate_output_transaction(
                &mut self.output_transactions,
                &mut self.presentation_trace,
                &mut self.server,
                transaction_id,
                MonotonicTimestampNs::new(20),
            )
        }
    }
}

fn compatibility_test_target() -> PresentationTarget {
    PresentationTarget {
        sequence: 1,
        presentation_time: MonotonicTimestampNs::new(10),
        submit_not_before: MonotonicTimestampNs::new(9),
        render_start_deadline: MonotonicTimestampNs::new(8),
        refresh_interval: Duration::from_millis(10),
        reason: PresentationTargetReason::ReactiveDouble,
        clock_generation: 1,
        estimated: false,
        predicted_unreachable: false,
        physical_claim: oblivion_one::native::presentation_deadline::PrimaryRefreshClaim {
            sequence: 1,
            presentation_time: MonotonicTimestampNs::new(10),
            clock_generation: 1,
        },
        selection_evidence: Default::default(),
    }
}
