use super::{
    ContentEpochId, ContentEpochTracker, CursorContentKey, DirectScanoutCandidateKey,
    OutputContentKey, OutputTransactionAllocator, OutputTransactionContent, OutputTransactionId,
    OutputTransactionState,
};
use crate::native_output::presentation::plane::{CursorSidecarId, PlaneWriteSet};
use crate::native_output::presentation::qualification::{DirectReleaseMode, DirectSyncReadiness};
use crate::native_output::presentation::trace::{
    PresentationTransactionEvent, PresentationTransactionTraceRing,
};
use crate::native_output::{ExplicitOutputCounters, OutputPresentationMode};
use oblivion_one::compositor::CompositorFrameBatchId;
use oblivion_one::native::kms::AtomicCursorVisualState;
use oblivion_one::native::presentation_deadline::MonotonicTimestampNs;
use oblivion_one::native::scheduler::NativeOutputPacingMode;
use std::num::NonZeroU64;
use std::time::Duration;

fn tx(value: u64) -> OutputTransactionId {
    OutputTransactionId::new(NonZeroU64::new(value).expect("non-zero transaction id"))
}

fn cursor_state(framebuffer_id: u32) -> AtomicCursorVisualState {
    AtomicCursorVisualState {
        visible: true,
        x: 0,
        y: 0,
        hotspot_x: 0,
        hotspot_y: 0,
        width: 64,
        height: 64,
        framebuffer_id: Some(framebuffer_id),
        image_generation: 1,
    }
}

fn observed(id: OutputTransactionId, timestamp_ns: u64) -> PresentationTransactionEvent {
    PresentationTransactionEvent::ContentObserved {
        transaction_id: id,
        timestamp_ns,
    }
}

fn submitted(id: OutputTransactionId, timestamp_ns: u64) -> PresentationTransactionEvent {
    PresentationTransactionEvent::KmsSubmitReturned {
        transaction_id: id,
        timestamp_ns,
    }
}

fn presented(id: OutputTransactionId, timestamp_ns: u64) -> PresentationTransactionEvent {
    PresentationTransactionEvent::PageflipPresented {
        transaction_id: id,
        timestamp_ns,
    }
}

fn immediate_presented(id: OutputTransactionId, timestamp_ns: u64) -> PresentationTransactionEvent {
    PresentationTransactionEvent::ImmediatePresented {
        transaction_id: id,
        timestamp_ns,
    }
}

#[test]
fn same_buffer_reattachment_advances_content_epoch_but_metadata_commits_retain_it() {
    let mut tracker = ContentEpochTracker::default();
    let buffer_id = NonZeroU64::new(42).expect("test buffer id is non-zero");

    let first = tracker.observe(7, buffer_id, 10);
    assert_eq!(first.buffer_id, buffer_id);
    assert_eq!(tracker.record_metadata_commit(7), Some(first.epoch));

    let second = tracker.observe(7, buffer_id, 11);
    assert_ne!(second.epoch, first.epoch);
    assert_eq!(second.buffer_id, first.buffer_id);
    assert_eq!(second.attachment_sequence, 11);
}

#[test]
fn unchanged_output_content_key_is_equal() {
    let content_epoch =
        ContentEpochId::new(NonZeroU64::new(7).expect("test content epoch is non-zero"));
    let transaction_id =
        OutputTransactionId::new(NonZeroU64::new(11).expect("test transaction id is non-zero"));
    let buffer_id = NonZeroU64::new(42).expect("test buffer id is non-zero");

    let first = OutputContentKey::new(
        9,
        buffer_id,
        content_epoch,
        1920,
        1080,
        0x3432_5241,
        0,
        0,
        1_000,
        3,
    );
    let second = OutputContentKey::new(
        9,
        buffer_id,
        content_epoch,
        1920,
        1080,
        0x3432_5241,
        0,
        0,
        1_000,
        3,
    );

    assert_eq!(first, second);
    assert_eq!(content_epoch.get(), 7);
    assert_eq!(transaction_id.get(), 11);
}

#[test]
fn candidate_key_ignores_protocol_only_work_but_tracks_visual_epochs() {
    let content = OutputContentKey::new(
        7,
        NonZeroU64::new(42).expect("buffer id"),
        ContentEpochId::new(NonZeroU64::new(3).expect("content epoch")),
        1920,
        1080,
        0x3432_5241,
        0,
        0,
        1_000,
        0,
    );
    let base = DirectScanoutCandidateKey {
        content,
        output_generation: 1,
        cursor_content_key: Some(CursorContentKey {
            visible: true,
            framebuffer_id: Some(1),
            image_generation: 1,
            hotspot_x: 0,
            hotspot_y: 0,
            width: 1,
            height: 1,
        }),
        color_epoch: 0,
    };

    assert_eq!(base, base);
    assert_ne!(
        base,
        DirectScanoutCandidateKey {
            content: OutputContentKey {
                content_epoch: ContentEpochId::new(NonZeroU64::new(4).expect("content epoch")),
                ..content
            },
            ..base
        }
    );
    assert_ne!(
        base,
        DirectScanoutCandidateKey {
            cursor_content_key: Some(CursorContentKey {
                image_generation: 2,
                ..base.cursor_content_key.expect("cursor content key")
            }),
            ..base
        }
    );
    assert_ne!(
        base,
        DirectScanoutCandidateKey {
            output_generation: 2,
            ..base
        }
    );
}

#[test]
fn trace_ring_is_bounded_and_reports_drops() {
    let mut ring = PresentationTransactionTraceRing::new(4);
    for index in 1..=10 {
        ring.push(observed(tx(index), index));
    }

    assert_eq!(ring.len(), 4);
    assert_eq!(ring.dropped(), 6);
}

#[test]
fn transaction_records_buffer_to_pageflip_timestamps() {
    let mut ring = PresentationTransactionTraceRing::new(16);
    let id = tx(1);
    ring.push(observed(id, 10));
    ring.push(submitted(id, 20));
    ring.push(presented(id, 30));

    let summary = ring.summarize(id).expect("transaction summary");
    assert_eq!(summary.observe_to_submit_ns, Some(10));
    assert_eq!(summary.submit_to_pageflip_ns, Some(10));
    let export = ring.export_jsonl();
    assert!(export.contains("\"event\":\"content_observed\""));
    assert!(export.contains("\"transaction_id\":1"));
}

#[test]
fn immediate_trace_uses_distinct_presented_event_and_summary() {
    let mut ring = PresentationTransactionTraceRing::new(4);
    let id = tx(2);
    ring.push(observed(id, 10));
    ring.push(immediate_presented(id, 30));

    let summary = ring.summarize(id).expect("transaction summary");
    assert_eq!(summary.submit_to_pageflip_ns, None);
    let export = ring.export_jsonl();
    assert!(export.contains("\"event\":\"immediate_presented\""));
    assert!(!export.contains("\"event\":\"pageflip_presented\""));
}

#[test]
fn worker_queued_trace_has_a_bounded_event_name() {
    let mut ring = PresentationTransactionTraceRing::new(4);
    ring.push(PresentationTransactionEvent::WorkerQueued {
        transaction_id: tx(3),
        timestamp_ns: 40,
    });

    assert!(ring.export_jsonl().contains("\"event\":\"worker_queued\""));
}

#[test]
fn direct_sync_rejects_unresolved_acquire_and_unproven_buffer() {
    assert!(matches!(
        DirectSyncReadiness::from_capabilities(true, true, true, true, false, true),
        DirectSyncReadiness::Qualified {
            in_fence: None,
            release_mode: DirectReleaseMode::Pageflip,
        }
    ));
    assert!(matches!(
        DirectSyncReadiness::from_capabilities(false, true, true, true, true, true),
        DirectSyncReadiness::Unsupported("acquire_not_ready")
    ));
    assert!(matches!(
        DirectSyncReadiness::from_capabilities(true, false, true, true, true, true),
        DirectSyncReadiness::Unsupported("buffer_device_or_modifier_unproven")
    ));
    assert!(matches!(
        DirectSyncReadiness::from_capabilities(true, true, true, false, true, true),
        DirectSyncReadiness::Unsupported("primary_in_fence_property_missing")
    ));
}

#[test]
fn explicit_output_counters_distinguish_atomic_fence_paths() {
    let mut counters = ExplicitOutputCounters::default();
    counters.note_atomic_submission(OutputPresentationMode::Vsync);
    counters.note_atomic_submission(OutputPresentationMode::Async);

    assert_eq!(counters.atomic_submissions, 2);
    assert_eq!(counters.atomic_in_fence_submissions, 1);
    assert_eq!(counters.async_userspace_fence_submissions, 1);
}

#[test]
fn output_transaction_allocator_is_shared_across_content_paths() {
    let mut allocator = OutputTransactionAllocator::default();

    let composited = allocator.allocate().expect("composited transaction ID");
    let cursor = allocator.allocate().expect("cursor transaction ID");
    let direct = allocator.allocate().expect("direct transaction ID");

    assert_eq!(composited.get(), 1);
    assert_eq!(cursor.get(), 2);
    assert_eq!(direct.get(), 3);
    assert!(composited < cursor);
    assert!(cursor < direct);
}

#[test]
fn output_transaction_descriptor_is_immutable_and_path_typed() {
    let mut allocator = OutputTransactionAllocator::default();
    let id = allocator.allocate().expect("transaction ID");
    let frame_batch_id =
        CompositorFrameBatchId::new(NonZeroU64::new(7).expect("frame batch ID is nonzero"));
    let now = MonotonicTimestampNs::new(10);
    let target = oblivion_one::native::presentation_deadline::PresentationTarget {
        sequence: 2,
        presentation_time: now,
        submit_not_before: now,
        render_start_deadline: now,
        refresh_interval: Duration::from_millis(10),
        reason:
            oblivion_one::native::presentation_deadline::PresentationTargetReason::ReactiveDouble,
        clock_generation: 1,
        estimated: false,
        predicted_unreachable: false,
    };

    let transaction = super::OutputTransaction::composited(
        id,
        1,
        now,
        target,
        NativeOutputPacingMode::ReactiveDouble,
        11,
        12,
        13,
        super::OutputSlotId::new(0).expect("slot zero"),
        91,
        None,
        frame_batch_id,
    )
    .expect("composited transaction");

    assert_eq!(transaction.id(), id);
    assert_eq!(transaction.output_generation(), 1);
    assert_eq!(
        transaction.content(),
        OutputTransactionContent::Composited {
            frame_id: 11,
            render_generation: 12,
            pool_generation: 13,
            equivalent_direct_key: None,
        }
    );
    assert_eq!(
        transaction.obligations().frame_batch_id(),
        Some(frame_batch_id)
    );

    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    ledger.insert(transaction).expect("insert transaction");
    assert_eq!(
        ledger.transaction(id).expect("active transaction").state(),
        OutputTransactionState::Built
    );
}

fn test_target() -> oblivion_one::native::presentation_deadline::PresentationTarget {
    let now = MonotonicTimestampNs::new(10);
    oblivion_one::native::presentation_deadline::PresentationTarget {
        sequence: 2,
        presentation_time: now,
        submit_not_before: now,
        render_start_deadline: now,
        refresh_interval: Duration::from_millis(10),
        reason:
            oblivion_one::native::presentation_deadline::PresentationTargetReason::ReactiveDouble,
        clock_generation: 1,
        estimated: false,
        predicted_unreachable: false,
    }
}

fn test_batch(value: u64) -> CompositorFrameBatchId {
    CompositorFrameBatchId::new(NonZeroU64::new(value).expect("frame batch is nonzero"))
}

fn test_composited_transaction(
    ledger: &mut super::OutputTransactionLedger,
    batch_id: CompositorFrameBatchId,
    output_generation: u64,
) -> super::OutputTransaction {
    let id = ledger.allocate_id().expect("transaction ID");
    super::OutputTransaction::composited(
        id,
        output_generation,
        MonotonicTimestampNs::new(10),
        test_target(),
        NativeOutputPacingMode::ReactiveDouble,
        id.get(),
        12,
        13,
        super::OutputSlotId::new(0).expect("slot zero"),
        91,
        None,
        batch_id,
    )
    .expect("composited transaction")
}

#[test]
fn ready_transaction_can_be_worker_queued() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let batch = test_batch(101);
    let transaction = test_composited_transaction(&mut ledger, batch, 1);
    let id = transaction.id();
    ledger.insert(transaction).unwrap();
    ledger
        .mark_ready(id, MonotonicTimestampNs::new(20))
        .unwrap();
    ledger
        .mark_queued(id, 4, MonotonicTimestampNs::new(21))
        .unwrap();
    assert!(matches!(
        ledger.transaction(id).unwrap().state(),
        super::OutputTransactionState::Queued {
            worker_generation: 4,
            ..
        }
    ));
}

#[test]
fn queued_transaction_can_be_submitted() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let batch = test_batch(102);
    let transaction = test_composited_transaction(&mut ledger, batch, 1);
    let id = transaction.id();
    ledger.insert(transaction).unwrap();
    ledger
        .mark_queued(id, 4, MonotonicTimestampNs::new(21))
        .unwrap();
    let token = oblivion_one::native::kms::PageFlipToken::new(9).unwrap();
    ledger
        .mark_submitted(id, token, MonotonicTimestampNs::new(30))
        .unwrap();
    assert!(matches!(
        ledger.transaction(id).unwrap().state(),
        super::OutputTransactionState::Submitted { token: actual, .. } if actual == token
    ));
    assert_eq!(ledger.counters().queued, 1);
    assert_eq!(ledger.counters().queue_wait_ns_total, 9);
}

#[test]
fn queued_transaction_rejects_presented() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let transaction = test_composited_transaction(&mut ledger, test_batch(103), 1);
    let id = transaction.id();
    ledger.insert(transaction).unwrap();
    ledger
        .mark_queued(id, 4, MonotonicTimestampNs::new(21))
        .unwrap();
    let result = ledger.mark_presented(
        id,
        oblivion_one::native::kms::PageFlipToken::new(9).unwrap(),
        1,
        MonotonicTimestampNs::new(30),
        None,
    );
    assert!(matches!(
        result,
        Err(super::OutputTransactionError::InvalidTransition { .. })
    ));
}

#[test]
fn output_transaction_allocator_overflow_is_explicit() {
    let mut allocator = super::OutputTransactionAllocator::with_next(
        NonZeroU64::new(u64::MAX).expect("maximum ID is nonzero"),
    );

    assert_eq!(
        allocator
            .allocate()
            .expect("maximum ID remains allocatable")
            .get(),
        u64::MAX
    );
    assert_eq!(
        allocator.allocate(),
        Err(super::OutputTransactionAllocationError::Exhausted)
    );
}

#[test]
fn built_transaction_can_become_ready() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let transaction = test_composited_transaction(&mut ledger, test_batch(1), 1);
    let id = transaction.id();
    ledger.insert(transaction).expect("insert transaction");

    ledger
        .mark_ready(id, MonotonicTimestampNs::new(20))
        .expect("built to ready");
    assert!(matches!(
        ledger.transaction(id).expect("active transaction").state(),
        super::OutputTransactionState::Ready { ready_at }
            if ready_at == MonotonicTimestampNs::new(20)
    ));
}

#[test]
fn built_transaction_accepts_render_failure() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let transaction = test_composited_transaction(&mut ledger, test_batch(101), 1);
    let id = transaction.id();
    ledger.insert(transaction).unwrap();

    ledger
        .mark_failed(
            id,
            super::OutputTransactionFailureStage::RenderExecution,
            MonotonicTimestampNs::new(20),
        )
        .expect("render failure is valid from Built");
}

#[test]
fn ready_transaction_accepts_kms_submit_failure() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let transaction = test_composited_transaction(&mut ledger, test_batch(102), 1);
    let id = transaction.id();
    ledger.insert(transaction).unwrap();
    ledger
        .mark_ready(id, MonotonicTimestampNs::new(20))
        .unwrap();

    ledger
        .mark_failed(
            id,
            super::OutputTransactionFailureStage::KmsSubmit,
            MonotonicTimestampNs::new(30),
        )
        .expect("KMS submit failure is valid from Ready");
}

#[test]
fn submitted_transaction_rejects_kms_submit_failure() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let transaction = test_composited_transaction(&mut ledger, test_batch(103), 1);
    let id = transaction.id();
    ledger.insert(transaction).unwrap();
    let token = super::PageFlipToken::new(103).unwrap();
    ledger
        .mark_submitted(id, token, MonotonicTimestampNs::new(20))
        .unwrap();

    assert_eq!(
        ledger
            .mark_failed(
                id,
                super::OutputTransactionFailureStage::KmsSubmit,
                MonotonicTimestampNs::new(30),
            )
            .expect_err("submitted transaction must reject a KMS submit failure"),
        super::OutputTransactionError::FailureStageMismatch {
            state: super::OutputTransactionStateKind::Submitted,
            stage: super::OutputTransactionFailureStage::KmsSubmit,
        }
    );
    assert!(ledger.transaction(id).is_some());
}

#[test]
fn submitted_transaction_accepts_pageflip_failure() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let transaction = test_composited_transaction(&mut ledger, test_batch(104), 1);
    let id = transaction.id();
    ledger.insert(transaction).unwrap();
    let token = super::PageFlipToken::new(104).unwrap();
    ledger
        .mark_submitted(id, token, MonotonicTimestampNs::new(20))
        .unwrap();

    ledger
        .mark_failed(
            id,
            super::OutputTransactionFailureStage::PageflipValidation,
            MonotonicTimestampNs::new(30),
        )
        .expect("submitted output may fail during teardown");
}

#[test]
fn accepted_presented_transition_settles_then_finalizes() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let batch_id = test_batch(107);
    let transaction = test_composited_transaction(&mut ledger, batch_id, 1);
    let id = transaction.id();
    let token = super::PageFlipToken::new(107).unwrap();
    ledger.insert(transaction).unwrap();
    ledger
        .mark_submitted(id, token, MonotonicTimestampNs::new(20))
        .unwrap();

    let accepted = ledger
        .accept_presented(id, token, 1, MonotonicTimestampNs::new(30), Some(2))
        .expect("pageflip terminal transition is accepted");
    assert_eq!(accepted.obligations().frame_batch_id(), Some(batch_id));
    assert!(matches!(
        ledger
            .transaction(id)
            .expect("settling transaction")
            .state(),
        super::OutputTransactionState::Settling { .. }
    ));
    assert_eq!(ledger.obligation_owner(batch_id), Some(id));
    assert_eq!(ledger.counters().terminal_transitions_finalized, 0);
    assert_eq!(ledger.counters().active_settling_transactions, 1);

    ledger
        .finalize_terminal(accepted)
        .expect("settlement finalizes");
    assert_eq!(ledger.active_count(), 0);
    assert_eq!(ledger.obligation_owner(batch_id), None);
    assert_eq!(ledger.counters().terminal_transitions_finalized, 1);
    assert_eq!(ledger.counters().active_settling_transactions, 0);
}

#[test]
fn ledger_rejection_prevents_protocol_settlement() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let batch_id = test_batch(108);
    let transaction = test_composited_transaction(&mut ledger, batch_id, 1);
    let id = transaction.id();
    ledger.insert(transaction).unwrap();
    ledger
        .mark_submitted(
            id,
            super::PageFlipToken::new(108).unwrap(),
            MonotonicTimestampNs::new(20),
        )
        .unwrap();

    assert!(
        ledger
            .accept_presented(
                id,
                super::PageFlipToken::new(109).unwrap(),
                1,
                MonotonicTimestampNs::new(30),
                Some(2),
            )
            .is_err()
    );
    assert!(matches!(
        ledger
            .transaction(id)
            .expect("submitted transaction")
            .state(),
        super::OutputTransactionState::Submitted { .. }
    ));
    assert_eq!(ledger.obligation_owner(batch_id), Some(id));
}

#[test]
fn missing_obligation_owner_rejects_terminal_acceptance() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let batch_id = test_batch(111);
    let transaction = test_composited_transaction(&mut ledger, batch_id, 1);
    let id = transaction.id();
    let token = super::PageFlipToken::new(111).unwrap();
    ledger.insert(transaction).unwrap();
    ledger
        .mark_submitted(id, token, MonotonicTimestampNs::new(20))
        .unwrap();

    assert_eq!(ledger.obligation_owner(batch_id), Some(id));
    ledger.forget_obligation_owner_for_test(batch_id);

    assert_eq!(
        ledger
            .accept_presented(id, token, 1, MonotonicTimestampNs::new(30), Some(2))
            .expect_err("missing obligation owner must reject before settlement"),
        super::OutputTransactionError::DuplicateObligationOwner
    );
    assert!(matches!(
        ledger.transaction(id).unwrap().state(),
        super::OutputTransactionState::Submitted { .. }
    ));
}

#[test]
fn prepared_presented_transition_commits_without_a_second_ledger_validation() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let batch_id = test_batch(112);
    let transaction = test_composited_transaction(&mut ledger, batch_id, 1);
    let id = transaction.id();
    let token = super::PageFlipToken::new(112).unwrap();
    ledger.insert(transaction).unwrap();
    ledger
        .mark_submitted(id, token, MonotonicTimestampNs::new(20))
        .unwrap();

    let prepared = ledger
        .accept_presented(id, token, 1, MonotonicTimestampNs::new(30), Some(2))
        .unwrap();
    ledger.commit_prepared_terminal(prepared);

    assert!(ledger.transaction(id).is_none());
    assert_eq!(ledger.obligation_owner(batch_id), None);
    assert_eq!(ledger.counters().presented, 1);
}

#[test]
fn settlement_failure_leaves_no_active_obligation_owner() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let batch_id = test_batch(109);
    let transaction = test_composited_transaction(&mut ledger, batch_id, 1);
    let id = transaction.id();
    let token = super::PageFlipToken::new(109).unwrap();
    ledger.insert(transaction).unwrap();
    ledger
        .mark_submitted(id, token, MonotonicTimestampNs::new(20))
        .unwrap();
    let accepted = ledger
        .accept_presented(id, token, 1, MonotonicTimestampNs::new(30), Some(2))
        .unwrap();

    ledger
        .fail_settlement(accepted, MonotonicTimestampNs::new(31))
        .expect("injected settlement failure is terminal");
    assert_eq!(ledger.active_count(), 0);
    assert_eq!(ledger.obligation_owner(batch_id), None);
    assert_eq!(ledger.counters().settlement_failures, 1);
    assert!(matches!(
        ledger.recent_terminal().back().unwrap().state(),
        super::OutputTransactionState::Terminal(super::OutputTransactionTerminal::Failed {
            stage: super::OutputTransactionFailureStage::ProtocolSettlement,
            ..
        })
    ));
}

#[test]
fn terminal_ownership_cross_check_detects_a_missing_batch_owner() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let batch_id = test_batch(120);
    let transaction = test_composited_transaction(&mut ledger, batch_id, 1);
    ledger.insert(transaction).unwrap();

    assert_eq!(ledger.validate_terminal_ownership(), Ok(()));
    ledger.forget_obligation_owner_for_test(batch_id);
    assert_eq!(
        ledger.validate_terminal_ownership(),
        Err(super::OutputTransactionOwnershipError::MissingObligationOwner { batch_id })
    );
}

#[test]
fn prepared_terminal_matrix_releases_every_obligation_exactly_once() {
    use super::{
        OutputTransactionDropReason as DropReason, OutputTransactionFailureStage as FailureStage,
        OutputTransactionSupersedeReason as SupersedeReason,
    };

    #[derive(Clone, Copy)]
    enum Outcome {
        Presented,
        WorkerRejected,
        AtomicFailed,
        DirectSuperseded,
        Suspended,
        GenerationInvalidated,
    }

    for (index, outcome) in [
        Outcome::Presented,
        Outcome::WorkerRejected,
        Outcome::AtomicFailed,
        Outcome::DirectSuperseded,
        Outcome::Suspended,
        Outcome::GenerationInvalidated,
    ]
    .into_iter()
    .enumerate()
    {
        let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
        let batch_id = test_batch(130 + index as u64);
        let transaction = test_composited_transaction(&mut ledger, batch_id, 1);
        let id = transaction.id();
        ledger.insert(transaction).unwrap();
        ledger
            .mark_ready(id, MonotonicTimestampNs::new(20))
            .unwrap();

        match outcome {
            Outcome::Presented => {
                let token = super::PageFlipToken::new(130 + index as u64).unwrap();
                ledger
                    .mark_submitted(id, token, MonotonicTimestampNs::new(30))
                    .unwrap();
                ledger
                    .mark_presented(id, token, 1, MonotonicTimestampNs::new(40), Some(2))
                    .unwrap();
            }
            Outcome::WorkerRejected => ledger
                .mark_failed(
                    id,
                    FailureStage::BackendOwnershipTransfer,
                    MonotonicTimestampNs::new(40),
                )
                .unwrap(),
            Outcome::AtomicFailed => ledger
                .mark_failed(id, FailureStage::KmsSubmit, MonotonicTimestampNs::new(40))
                .unwrap(),
            Outcome::DirectSuperseded => ledger
                .mark_superseded(
                    id,
                    None,
                    SupersedeReason::DirectTransition,
                    MonotonicTimestampNs::new(40),
                )
                .unwrap(),
            Outcome::Suspended => ledger
                .mark_dropped(
                    id,
                    DropReason::SessionSuspended,
                    MonotonicTimestampNs::new(40),
                )
                .unwrap(),
            Outcome::GenerationInvalidated => ledger
                .cleanup_generation(
                    1,
                    DropReason::OutputDestroyed,
                    MonotonicTimestampNs::new(40),
                )
                .map(|count| assert_eq!(count, 1))
                .unwrap(),
        }

        assert_eq!(ledger.active_count(), 0);
        assert_eq!(ledger.obligation_owner(batch_id), None);
        assert_eq!(ledger.counters().terminal_transitions_finalized, 1);
        assert_eq!(ledger.counters().duplicate_settlement_attempts, 0);
        assert_eq!(ledger.validate_terminal_ownership(), Ok(()));
    }
}

#[test]
fn newer_client_batch_does_not_replace_a_ready_transactions_obligations() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let ready_batch = test_batch(140);
    let newer_batch = test_batch(141);
    let ready = test_composited_transaction(&mut ledger, ready_batch, 1);
    let ready_id = ready.id();
    ledger.insert(ready).unwrap();
    ledger
        .mark_ready(ready_id, MonotonicTimestampNs::new(20))
        .unwrap();

    let newer = test_composited_transaction(&mut ledger, newer_batch, 1);
    let newer_id = newer.id();
    ledger.insert(newer).unwrap();

    assert_eq!(
        ledger
            .transaction(ready_id)
            .unwrap()
            .descriptor()
            .obligations()
            .frame_batch_id(),
        Some(ready_batch)
    );
    assert_eq!(ledger.obligation_owner(ready_batch), Some(ready_id));
    assert_eq!(ledger.obligation_owner(newer_batch), Some(newer_id));
    assert_eq!(ledger.validate_terminal_ownership(), Ok(()));
}

#[test]
fn second_terminal_attempt_during_settlement_is_rejected() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let transaction = test_composited_transaction(&mut ledger, test_batch(110), 1);
    let id = transaction.id();
    let token = super::PageFlipToken::new(110).unwrap();
    ledger.insert(transaction).unwrap();
    ledger
        .mark_submitted(id, token, MonotonicTimestampNs::new(20))
        .unwrap();
    let accepted = ledger
        .accept_presented(id, token, 1, MonotonicTimestampNs::new(30), Some(2))
        .unwrap();

    assert!(
        ledger
            .accept_presented(id, token, 1, MonotonicTimestampNs::new(31), Some(3))
            .is_err()
    );
    assert!(matches!(
        ledger.transaction(id).unwrap().state(),
        super::OutputTransactionState::Settling { .. }
    ));
    ledger.finalize_terminal(accepted).unwrap();
}

#[test]
fn immediate_compatibility_presentation_creates_transaction_and_transitions_built_to_presented() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let id = ledger.allocate_id().unwrap();
    let transaction = super::OutputTransaction::compatibility_immediate(
        id,
        1,
        MonotonicTimestampNs::new(10),
        test_target(),
        NativeOutputPacingMode::ReactiveDouble,
        111,
        test_batch(111),
    )
    .unwrap();
    ledger.insert(transaction).unwrap();

    let accepted = ledger
        .accept_immediate_presented(id, MonotonicTimestampNs::new(20))
        .expect("immediate presentation is accepted");
    assert!(matches!(
        ledger.transaction(id).unwrap().state(),
        super::OutputTransactionState::Settling { .. }
    ));
    ledger.finalize_terminal(accepted).unwrap();
    assert_eq!(ledger.active_count(), 0);
    assert_eq!(ledger.counters().immediate_presentations, 1);
    assert_eq!(ledger.counters().failed, 0);
}

#[test]
fn immediate_compatibility_path_never_waits_for_pageflip() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let id = ledger.allocate_id().unwrap();
    let transaction = super::OutputTransaction::compatibility_immediate(
        id,
        1,
        MonotonicTimestampNs::new(10),
        test_target(),
        NativeOutputPacingMode::ReactiveDouble,
        113,
        test_batch(113),
    )
    .unwrap();
    ledger.insert(transaction).unwrap();

    assert!(
        ledger
            .mark_presented(
                id,
                super::PageFlipToken::new(113).unwrap(),
                1,
                MonotonicTimestampNs::new(20),
                None,
            )
            .is_err()
    );
    assert!(matches!(
        ledger.transaction(id).unwrap().state(),
        super::OutputTransactionState::Built
    ));
}

#[test]
fn stale_pageflip_cannot_recomplete_immediate_transaction() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let id = ledger.allocate_id().unwrap();
    let transaction = super::OutputTransaction::compatibility_immediate(
        id,
        1,
        MonotonicTimestampNs::new(10),
        test_target(),
        NativeOutputPacingMode::ReactiveDouble,
        114,
        test_batch(114),
    )
    .unwrap();
    ledger.insert(transaction).unwrap();
    let accepted = ledger
        .accept_immediate_presented(id, MonotonicTimestampNs::new(20))
        .unwrap();
    ledger.finalize_terminal(accepted).unwrap();

    assert_eq!(ledger.counters().immediate_presentations, 1);
    assert_eq!(
        ledger
            .accept_presented(
                id,
                super::PageFlipToken::new(114).unwrap(),
                1,
                MonotonicTimestampNs::new(30),
                None,
            )
            .expect_err("stale pageflip must be rejected"),
        super::OutputTransactionError::UnknownTransaction
    );
    assert_eq!(ledger.counters().immediate_presentations, 1);
    assert_eq!(ledger.counters().terminal_transitions_finalized, 1);
}

#[test]
fn immediate_compatibility_failure_marks_typed_failure() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let id = ledger.allocate_id().unwrap();
    let transaction = super::OutputTransaction::compatibility_immediate(
        id,
        1,
        MonotonicTimestampNs::new(10),
        test_target(),
        NativeOutputPacingMode::ReactiveDouble,
        112,
        test_batch(112),
    )
    .unwrap();
    ledger.insert(transaction).unwrap();
    ledger
        .mark_failed(
            id,
            super::OutputTransactionFailureStage::KmsSubmit,
            MonotonicTimestampNs::new(20),
        )
        .unwrap();

    assert_eq!(ledger.counters().immediate_presentation_failures, 1);
    assert!(matches!(
        ledger.recent_terminal().back().unwrap().state(),
        super::OutputTransactionState::Terminal(super::OutputTransactionTerminal::Failed {
            stage: super::OutputTransactionFailureStage::KmsSubmit,
            ..
        })
    ));
}

#[test]
fn terminal_transaction_rejects_failure() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let transaction = test_composited_transaction(&mut ledger, test_batch(105), 1);
    let id = transaction.id();
    ledger.insert(transaction).unwrap();
    ledger
        .mark_failed(
            id,
            super::OutputTransactionFailureStage::RenderPreparation,
            MonotonicTimestampNs::new(20),
        )
        .unwrap();

    assert!(
        ledger
            .mark_failed(
                id,
                super::OutputTransactionFailureStage::RenderPreparation,
                MonotonicTimestampNs::new(30),
            )
            .is_err()
    );
}

#[test]
fn failure_stage_mismatch_does_not_release_obligation_owner() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let batch_id = test_batch(106);
    let transaction = test_composited_transaction(&mut ledger, batch_id, 1);
    let id = transaction.id();
    ledger.insert(transaction).unwrap();
    ledger
        .mark_submitted(
            id,
            super::PageFlipToken::new(106).unwrap(),
            MonotonicTimestampNs::new(20),
        )
        .unwrap();

    assert_eq!(
        ledger
            .mark_failed(
                id,
                super::OutputTransactionFailureStage::KmsSubmit,
                MonotonicTimestampNs::new(30),
            )
            .expect_err("failure-stage mismatch must be rejected"),
        super::OutputTransactionError::FailureStageMismatch {
            state: super::OutputTransactionStateKind::Submitted,
            stage: super::OutputTransactionFailureStage::KmsSubmit,
        }
    );
    assert_eq!(ledger.obligation_owner(batch_id), Some(id));
}

#[test]
fn active_capacity_is_not_silently_evicted() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(1, 64);
    let first = test_composited_transaction(&mut ledger, test_batch(1), 1);
    ledger.insert(first).unwrap();
    let second = test_composited_transaction(&mut ledger, test_batch(2), 1);

    assert_eq!(
        ledger.insert(second),
        Err(super::OutputTransactionError::ActiveCapacityExceeded)
    );
    assert_eq!(ledger.active_count(), 1);
}

#[test]
fn invalid_lifecycle_transition_is_counted_and_preserves_state() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let transaction = test_composited_transaction(&mut ledger, test_batch(1), 1);
    let id = transaction.id();
    ledger.insert(transaction).unwrap();
    ledger
        .mark_ready(id, MonotonicTimestampNs::new(20))
        .unwrap();

    assert!(
        ledger
            .mark_ready(id, MonotonicTimestampNs::new(30))
            .is_err()
    );
    assert_eq!(ledger.counters().invalid_transitions, 1);
    assert!(matches!(
        ledger.transaction(id).unwrap().state(),
        super::OutputTransactionState::Ready { .. }
    ));
}

#[test]
fn wrong_generation_cannot_terminalize_submitted_transaction() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let transaction = test_composited_transaction(&mut ledger, test_batch(1), 1);
    let id = transaction.id();
    ledger.insert(transaction).unwrap();
    let token = super::PageFlipToken::new(41).unwrap();
    ledger
        .mark_submitted(id, token, MonotonicTimestampNs::new(20))
        .unwrap();

    assert_eq!(
        ledger.mark_presented(id, token, 2, MonotonicTimestampNs::new(30), Some(2)),
        Err(super::OutputTransactionError::GenerationMismatch)
    );
    assert!(ledger.transaction(id).is_some());
}

#[test]
fn built_transaction_can_submit_without_ready_state() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let transaction = test_composited_transaction(&mut ledger, test_batch(1), 1);
    let id = transaction.id();
    ledger.insert(transaction).expect("insert transaction");
    let token = super::PageFlipToken::new(41).expect("token");

    ledger
        .mark_submitted(id, token, MonotonicTimestampNs::new(20))
        .expect("built to submitted");
    assert!(matches!(
        ledger.transaction(id).expect("active transaction").state(),
        super::OutputTransactionState::Submitted {
            token: submitted,
            ..
        } if submitted == token
    ));
}

#[test]
fn compatibility_transaction_uses_shared_lifecycle_identity() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let id = ledger.allocate_id().expect("transaction ID");
    let transaction = super::OutputTransaction::compatibility_composited(
        id,
        1,
        MonotonicTimestampNs::new(10),
        test_target(),
        NativeOutputPacingMode::ReactiveDouble,
        31,
        32,
        94,
        None,
        test_batch(12),
    )
    .expect("compatibility transaction");

    assert_eq!(
        transaction.planes().primary(),
        super::PrimaryPlaneAssignment::CompatibilityFramebuffer { framebuffer_id: 94 }
    );
    ledger.insert(transaction).expect("insert transaction");
    let token = super::PageFlipToken::new(53).expect("token");
    ledger
        .mark_submitted(id, token, MonotonicTimestampNs::new(20))
        .expect("submitted");
    ledger
        .mark_presented(id, token, 1, MonotonicTimestampNs::new(30), None)
        .expect("presented");
    assert_eq!(ledger.counters().presented_composited, 1);
}

#[test]
fn ready_transaction_can_submit_and_present() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let transaction = test_composited_transaction(&mut ledger, test_batch(1), 1);
    let id = transaction.id();
    ledger.insert(transaction).expect("insert transaction");
    ledger
        .mark_ready(id, MonotonicTimestampNs::new(20))
        .expect("ready");
    let token = super::PageFlipToken::new(41).expect("token");
    ledger
        .mark_submitted(id, token, MonotonicTimestampNs::new(30))
        .expect("submitted");
    ledger
        .mark_presented(id, token, 1, MonotonicTimestampNs::new(40), Some(3))
        .expect("presented");

    assert_eq!(ledger.active_count(), 0);
    assert!(matches!(
        ledger
            .recent_terminal()
            .back()
            .expect("terminal record")
            .state(),
        super::OutputTransactionState::Terminal(super::OutputTransactionTerminal::Presented {
            actual_sequence: Some(3),
            ..
        })
    ));
}

#[test]
fn terminal_transaction_rejects_second_completion() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let transaction = test_composited_transaction(&mut ledger, test_batch(1), 1);
    let id = transaction.id();
    ledger.insert(transaction).expect("insert transaction");
    let token = super::PageFlipToken::new(41).expect("token");
    ledger
        .mark_submitted(id, token, MonotonicTimestampNs::new(20))
        .unwrap();
    ledger
        .mark_presented(id, token, 1, MonotonicTimestampNs::new(30), Some(2))
        .unwrap();

    assert_eq!(
        ledger.mark_presented(id, token, 1, MonotonicTimestampNs::new(40), Some(3)),
        Err(super::OutputTransactionError::UnknownTransaction)
    );
}

#[test]
fn wrong_pageflip_token_cannot_present_transaction() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let transaction = test_composited_transaction(&mut ledger, test_batch(1), 1);
    let id = transaction.id();
    ledger.insert(transaction).expect("insert transaction");
    let expected = super::PageFlipToken::new(41).expect("token");
    let wrong = super::PageFlipToken::new(42).expect("token");
    ledger
        .mark_submitted(id, expected, MonotonicTimestampNs::new(20))
        .unwrap();

    assert_eq!(
        ledger.mark_presented(id, wrong, 1, MonotonicTimestampNs::new(30), Some(2)),
        Err(super::OutputTransactionError::TokenMismatch)
    );
    assert!(ledger.transaction(id).is_some());
}

#[test]
fn direct_pageflip_rejection_settles_nothing() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let id = ledger.allocate_id().unwrap();
    let batch_id = test_batch(114);
    let transaction = super::OutputTransaction::direct(
        id,
        1,
        MonotonicTimestampNs::new(10),
        test_target(),
        NativeOutputPacingMode::ReactiveDouble,
        114,
        test_direct_key(),
        92,
        None,
        batch_id,
        7,
        super::OutputReleasePlan::Pageflip,
    )
    .unwrap();
    ledger.insert(transaction).unwrap();
    ledger
        .mark_submitted(
            id,
            super::PageFlipToken::new(114).unwrap(),
            MonotonicTimestampNs::new(20),
        )
        .unwrap();

    assert_eq!(
        ledger.mark_presented(
            id,
            super::PageFlipToken::new(115).unwrap(),
            1,
            MonotonicTimestampNs::new(30),
            Some(2),
        ),
        Err(super::OutputTransactionError::TokenMismatch)
    );
    assert_eq!(ledger.obligation_owner(batch_id), Some(id));
    assert!(ledger.transaction(id).is_some());
}

#[test]
fn cursor_pageflip_rejection_terminalizes_nothing() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let id = ledger.allocate_id().unwrap();
    let transaction = super::OutputTransaction::cursor_plane_delta(
        id,
        1,
        MonotonicTimestampNs::new(10),
        test_target(),
        NativeOutputPacingMode::ReactiveDouble,
        115,
        Some(cursor_state(93)),
        super::OutputReleasePlan::Pageflip,
    )
    .unwrap();
    ledger.insert(transaction).unwrap();
    ledger
        .mark_submitted(
            id,
            super::PageFlipToken::new(115).unwrap(),
            MonotonicTimestampNs::new(20),
        )
        .unwrap();

    assert_eq!(
        ledger.mark_presented(
            id,
            super::PageFlipToken::new(116).unwrap(),
            1,
            MonotonicTimestampNs::new(30),
            None,
        ),
        Err(super::OutputTransactionError::TokenMismatch)
    );
    assert_eq!(ledger.active_count(), 1);
    assert!(ledger.transaction(id).is_some());
}

#[test]
fn one_frame_batch_cannot_be_owned_by_two_transactions() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let first = test_composited_transaction(&mut ledger, test_batch(1), 1);
    let second = test_composited_transaction(&mut ledger, test_batch(1), 1);
    ledger.insert(first).expect("first transaction");

    assert_eq!(
        ledger.insert(second),
        Err(super::OutputTransactionError::DuplicateObligationOwner)
    );
    assert_eq!(ledger.counters().duplicate_obligation_attempts, 1);
}

#[test]
fn superseding_ready_transaction_records_replacement() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let first = test_composited_transaction(&mut ledger, test_batch(1), 1);
    let first_id = first.id();
    ledger.insert(first).expect("first transaction");
    ledger
        .mark_ready(first_id, MonotonicTimestampNs::new(20))
        .unwrap();

    let second = test_composited_transaction(&mut ledger, test_batch(2), 1);
    let second_id = second.id();
    ledger.insert(second).expect("second transaction");
    ledger
        .mark_superseded(
            first_id,
            Some(second_id),
            super::OutputTransactionSupersedeReason::NewerTransaction,
            MonotonicTimestampNs::new(30),
        )
        .unwrap();

    assert_eq!(ledger.obligation_owner(test_batch(1)), None);
    assert!(matches!(
        ledger.recent_terminal().back().expect("terminal record").state(),
        super::OutputTransactionState::Terminal(
            super::OutputTransactionTerminal::Superseded { by: Some(by), .. }
        ) if by == second_id
    ));
}

#[test]
fn generation_cleanup_terminates_all_matching_active_transactions() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let first = test_composited_transaction(&mut ledger, test_batch(1), 7);
    let second = test_composited_transaction(&mut ledger, test_batch(2), 8);
    let second_id = second.id();
    ledger.insert(first).expect("first transaction");
    ledger.insert(second).expect("second transaction");

    assert_eq!(
        ledger
            .cleanup_generation(
                7,
                super::OutputTransactionDropReason::OutputDestroyed,
                MonotonicTimestampNs::new(30),
            )
            .unwrap(),
        1
    );
    assert_eq!(ledger.active_count(), 1);
    assert_eq!(ledger.obligation_owner(test_batch(1)), None);
    assert_eq!(ledger.obligation_owner(test_batch(2)), Some(second_id));
}

#[test]
fn ledger_recent_history_is_bounded() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 2);
    for value in 1..=3 {
        let transaction = test_composited_transaction(&mut ledger, test_batch(value), 1);
        let id = transaction.id();
        ledger.insert(transaction).expect("transaction");
        ledger
            .mark_dropped(
                id,
                super::OutputTransactionDropReason::NoVisualChange,
                MonotonicTimestampNs::new(value),
            )
            .expect("drop transaction");
    }

    assert_eq!(ledger.recent_terminal().len(), 2);
    assert_eq!(ledger.counters().terminal_history_overwrites, 1);
}

#[test]
fn no_visual_change_does_not_emit_presentation_feedback() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let transaction = test_composited_transaction(&mut ledger, test_batch(71), 1);
    let id = transaction.id();
    ledger.insert(transaction).expect("transaction");

    ledger
        .mark_dropped(
            id,
            super::OutputTransactionDropReason::NoVisualChange,
            MonotonicTimestampNs::new(72),
        )
        .expect("no-visual-change settlement");

    assert_eq!(ledger.counters().presented, 0);
    assert_eq!(ledger.counters().presented_composited, 0);
    assert!(matches!(
        ledger
            .recent_terminal()
            .back()
            .expect("terminal record")
            .state(),
        super::OutputTransactionState::Terminal(super::OutputTransactionTerminal::Dropped {
            reason: super::OutputTransactionDropReason::NoVisualChange,
            ..
        })
    ));
}

#[test]
fn no_visual_change_callbacks_follow_existing_refresh_contract() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let batch_id = test_batch(73);
    let transaction = test_composited_transaction(&mut ledger, batch_id, 1);
    let id = transaction.id();
    ledger.insert(transaction).expect("transaction");

    ledger
        .mark_dropped(
            id,
            super::OutputTransactionDropReason::NoVisualChange,
            MonotonicTimestampNs::new(74),
        )
        .expect("no-visual-change settlement");

    assert_eq!(ledger.obligation_owner(batch_id), None);
    assert_eq!(ledger.active_count(), 0);
    assert_eq!(ledger.counters().terminal_transitions_finalized, 1);
    assert_eq!(
        ledger.mark_dropped(
            id,
            super::OutputTransactionDropReason::NoVisualChange,
            MonotonicTimestampNs::new(75),
        ),
        Err(super::OutputTransactionError::UnknownTransaction)
    );
}

#[test]
fn no_visual_change_cannot_terminalize_submitted_transaction() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let transaction = test_composited_transaction(&mut ledger, test_batch(76), 1);
    let id = transaction.id();
    ledger.insert(transaction).expect("transaction");
    ledger
        .mark_submitted(
            id,
            super::PageFlipToken::new(77).unwrap(),
            MonotonicTimestampNs::new(77),
        )
        .expect("submit transaction");

    assert_eq!(
        ledger.mark_no_visual_change(id, MonotonicTimestampNs::new(78)),
        Err(super::OutputTransactionError::InvalidTransition {
            from: super::OutputTransactionStateKind::Submitted,
            requested: super::OutputTransactionTransitionKind::Dropped,
        })
    );
    assert!(matches!(
        ledger
            .transaction(id)
            .expect("submitted transaction")
            .state(),
        super::OutputTransactionState::Submitted { .. }
    ));
}

fn test_direct_key() -> DirectScanoutCandidateKey {
    let content = OutputContentKey::new(
        7,
        NonZeroU64::new(42).expect("buffer id"),
        ContentEpochId::new(NonZeroU64::new(3).expect("content epoch")),
        1920,
        1080,
        0x3432_5241,
        0,
        0,
        1_000,
        0,
    );
    DirectScanoutCandidateKey {
        content,
        output_generation: 1,
        cursor_content_key: None,
        color_epoch: 0,
    }
}

#[test]
fn direct_transaction_submits_without_ready_state() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let id = ledger.allocate_id().unwrap();
    let token = super::PageFlipToken::new(51).expect("token");
    let transaction = super::OutputTransaction::direct(
        id,
        1,
        MonotonicTimestampNs::new(10),
        test_target(),
        NativeOutputPacingMode::ReactiveDouble,
        21,
        test_direct_key(),
        92,
        None,
        test_batch(11),
        7,
        super::OutputReleasePlan::Pageflip,
    )
    .unwrap();
    ledger.insert(transaction).unwrap();
    ledger
        .mark_submitted(id, token, MonotonicTimestampNs::new(20))
        .unwrap();
    ledger
        .mark_presented(id, token, 1, MonotonicTimestampNs::new(30), Some(2))
        .unwrap();

    assert_eq!(ledger.counters().submitted_direct, 1);
    assert_eq!(ledger.counters().presented_direct, 1);
}

#[test]
fn plane_delta_transaction_has_no_protocol_obligations_or_primary_owner() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let id = ledger.allocate_id().unwrap();
    let token = super::PageFlipToken::new(52).expect("token");
    let transaction = super::OutputTransaction::plane_delta(
        id,
        1,
        MonotonicTimestampNs::new(10),
        test_target(),
        NativeOutputPacingMode::ReactiveDouble,
        PlaneWriteSet::CURSOR,
        CursorSidecarId::new(std::num::NonZeroU64::new(99).unwrap()),
        99,
        Some(cursor_state(93)),
        super::OutputReleasePlan::Pageflip,
    )
    .unwrap();
    assert_eq!(transaction.obligations().frame_batch_id(), None);
    assert_eq!(
        transaction.planes().primary(),
        super::PrimaryPlaneAssignment::Unchanged
    );
    ledger.insert(transaction).unwrap();
    ledger
        .mark_submitted(id, token, MonotonicTimestampNs::new(20))
        .unwrap();
    ledger
        .mark_presented(id, token, 1, MonotonicTimestampNs::new(30), None)
        .unwrap();

    assert_eq!(ledger.active_count(), 0);
    assert_eq!(ledger.counters().presented_plane_delta, 1);
}

#[test]
fn direct_and_cursor_descriptors_have_expected_obligations() {
    let mut ledger = super::OutputTransactionLedger::with_capacities(8, 64);
    let key = test_direct_key();
    let direct_id = ledger.allocate_id().unwrap();
    let direct = super::OutputTransaction::direct(
        direct_id,
        1,
        MonotonicTimestampNs::new(10),
        test_target(),
        NativeOutputPacingMode::ReactiveDouble,
        21,
        key,
        92,
        None,
        test_batch(11),
        7,
        super::OutputReleasePlan::Pageflip,
    )
    .unwrap();
    assert_eq!(direct.obligations().direct_surface_id(), Some(7));
    assert_eq!(
        direct.planes().primary(),
        super::PrimaryPlaneAssignment::ClientFramebuffer {
            key,
            framebuffer_id: 92
        }
    );

    let cursor_id = ledger.allocate_id().unwrap();
    let cursor = super::OutputTransaction::plane_delta(
        cursor_id,
        1,
        MonotonicTimestampNs::new(10),
        test_target(),
        NativeOutputPacingMode::ReactiveDouble,
        PlaneWriteSet::CURSOR,
        CursorSidecarId::new(std::num::NonZeroU64::new(99).unwrap()),
        99,
        Some(cursor_state(93)),
        super::OutputReleasePlan::Pageflip,
    )
    .unwrap();
    assert_eq!(cursor.obligations().frame_batch_id(), None);
    assert_eq!(
        cursor.planes().primary(),
        super::PrimaryPlaneAssignment::Unchanged
    );
    assert_eq!(
        cursor.content(),
        OutputTransactionContent::PlaneDelta {
            changed: PlaneWriteSet::CURSOR,
            cursor_sidecar_id: CursorSidecarId::new(std::num::NonZeroU64::new(99).unwrap()),
        }
    );
}
