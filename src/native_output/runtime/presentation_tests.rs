use super::super::planner::visual_target_deadline_for_mode;
use super::presentation_transactions::complete_presented_output_transaction;
use super::*;
use oblivion_one::compositor::CompositorFrameBatchId;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_COMPATIBILITY_TEST_SOCKET: AtomicU64 = AtomicU64::new(0);

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
fn predictive_triple_only_schedules_pending_plus_one() {
    let mut planner = PresentationDeadlinePlanner::new(Duration::from_millis(10));
    planner.note_presented(MonotonicTimestampNs::new(10_000_000));
    let pending = planner
        .reactive_target(MonotonicTimestampNs::new(11_000_000))
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

struct CompatibilityPresentationHarness {
    server: OwnCompositorServer,
    scanout: NativeScanoutBackend,
    output_transactions: OutputTransactionLedger,
    presentation_trace: PresentationTransactionTraceRing,
    batch_id: CompositorFrameBatchId,
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
            |_| result,
        )
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
    }
}
