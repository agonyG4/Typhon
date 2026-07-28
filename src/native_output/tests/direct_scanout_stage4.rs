use super::{
    ContentEpochId, DirectContentDisposition, DirectPlaneValidationKey, DirectScanoutCandidateKey,
    OutputContentKey, OutputReleasePlan, OutputTransaction, OutputTransactionError,
    OutputTransactionFailureStage, OutputTransactionId, OutputTransactionLedger,
    OutputTransactionState, OutputTransactionTerminal, PrimaryPlaneAssignment,
    classify_direct_content,
};
use oblivion_one::compositor::CompositorFrameBatchId;
use oblivion_one::native::kms::PageFlipToken;
use oblivion_one::native::presentation_deadline::{
    MonotonicTimestampNs, PresentationTarget, PresentationTargetReason,
};
use oblivion_one::native::scheduler::NativeOutputPacingMode;
use std::num::NonZeroU64;
use std::time::Duration;

fn test_target() -> PresentationTarget {
    let now = MonotonicTimestampNs::new(10);
    PresentationTarget {
        sequence: 2,
        presentation_time: now,
        submit_not_before: now,
        render_start_deadline: now,
        refresh_interval: Duration::from_millis(10),
        reason: PresentationTargetReason::ReactiveDouble,
        clock_generation: 1,
        estimated: false,
        predicted_unreachable: false,
    }
}

fn test_direct_key(content_epoch: u64) -> DirectScanoutCandidateKey {
    let content = OutputContentKey::new(
        7,
        NonZeroU64::new(42).expect("buffer id"),
        ContentEpochId::new(NonZeroU64::new(content_epoch).expect("content epoch")),
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
        cursor_plan_key: None,
        color_epoch: 0,
    }
}

fn test_transaction_id(value: u64) -> OutputTransactionId {
    OutputTransactionId::new(NonZeroU64::new(value).expect("transaction id"))
}

fn test_frame_batch_id(value: u64) -> CompositorFrameBatchId {
    CompositorFrameBatchId::new(NonZeroU64::new(value).expect("frame batch id"))
}

#[test]
fn direct_transaction_uses_client_primary_assignment() {
    let key = test_direct_key(3);
    let transaction = OutputTransaction::direct(
        test_transaction_id(1),
        1,
        MonotonicTimestampNs::new(10),
        test_target(),
        NativeOutputPacingMode::ReactiveDouble,
        21,
        key,
        92,
        None,
        test_frame_batch_id(11),
        7,
        OutputReleasePlan::Pageflip,
    )
    .expect("direct transaction");

    assert_eq!(
        transaction.planes().primary(),
        PrimaryPlaneAssignment::ClientFramebuffer {
            key,
            framebuffer_id: 92,
        }
    );
}

#[test]
fn same_buffer_new_content_epoch_has_a_distinct_candidate_key() {
    let first = test_direct_key(3);
    let second = DirectScanoutCandidateKey {
        content: OutputContentKey {
            content_epoch: ContentEpochId::new(NonZeroU64::new(4).expect("content epoch")),
            ..first.content
        },
        ..first
    };

    assert_eq!(first.content.buffer_id, second.content.buffer_id);
    assert_ne!(first, second);
}

#[test]
fn identical_content_epoch_reuses_the_same_candidate_key() {
    let first = test_direct_key(3);
    let second = test_direct_key(3);

    assert_eq!(first, second);
}

#[test]
fn same_buffer_and_same_content_epoch_does_not_submit() {
    let candidate = test_direct_key(3);

    assert_eq!(
        classify_direct_content(candidate, Some(candidate), None),
        DirectContentDisposition::MatchesPresented
    );
}

#[test]
fn same_buffer_with_new_content_epoch_submits_new_direct_transaction() {
    let presented = test_direct_key(3);
    let candidate = test_direct_key(4);

    assert_ne!(candidate, presented);
    assert_eq!(
        classify_direct_content(candidate, Some(presented), None),
        DirectContentDisposition::NewContent
    );
}

#[test]
fn same_content_matching_queued_or_submitted_job_is_not_admitted_twice() {
    let candidate = test_direct_key(3);

    assert_eq!(
        classify_direct_content(candidate, None, Some(candidate)),
        DirectContentDisposition::MatchesQueuedOrSubmitted
    );
}

#[test]
fn new_content_epoch_can_reuse_validation_key_but_still_submits() {
    let first = test_direct_key(3);
    let second = test_direct_key(4);
    let validation_key = DirectPlaneValidationKey {
        output_generation: 1,
        crtc_id: 7,
        primary_plane_id: 8,
        mode_width: 1920,
        mode_height: 1080,
        format: 0x3432_5241,
        modifier: 0,
        buffer_width: 1920,
        buffer_height: 1080,
        plane_layout_hash: 9,
        cursor_plan_key: None,
        synchronization_key: 10,
    };

    assert_eq!(first.output_generation, second.output_generation);
    assert_eq!(validation_key.output_generation, 1);
    assert_ne!(first, second);
    assert_eq!(
        classify_direct_content(second, Some(first), None),
        DirectContentDisposition::NewContent
    );
}

#[test]
fn direct_test_rejection_never_marks_transaction_presented() {
    let mut ledger = OutputTransactionLedger::with_capacities(8, 64);
    let transaction_id = test_transaction_id(1);
    let transaction = OutputTransaction::direct(
        transaction_id,
        1,
        MonotonicTimestampNs::new(10),
        test_target(),
        NativeOutputPacingMode::ReactiveDouble,
        21,
        test_direct_key(3),
        92,
        None,
        test_frame_batch_id(11),
        7,
        OutputReleasePlan::Pageflip,
    )
    .expect("direct transaction");
    ledger
        .insert(transaction)
        .expect("insert direct transaction");

    ledger
        .mark_failed(
            transaction_id,
            OutputTransactionFailureStage::KmsSubmit,
            MonotonicTimestampNs::new(30),
        )
        .expect("direct test rejection");

    assert_eq!(ledger.counters().presented, 0);
    assert_eq!(ledger.counters().presented_direct, 0);
    assert_eq!(ledger.active_count(), 0);
    assert_eq!(
        ledger
            .recent_terminal()
            .back()
            .expect("terminal record")
            .state(),
        OutputTransactionState::Terminal(OutputTransactionTerminal::Failed {
            stage: OutputTransactionFailureStage::KmsSubmit,
            at: MonotonicTimestampNs::new(30),
        })
    );
}

#[test]
fn direct_pageflip_is_the_only_presented_transition() {
    let mut ledger = OutputTransactionLedger::with_capacities(8, 64);
    let transaction_id = test_transaction_id(1);
    let generation = 1;
    let token = PageFlipToken::new(51).expect("pageflip token");
    let transaction = OutputTransaction::direct(
        transaction_id,
        generation,
        MonotonicTimestampNs::new(10),
        test_target(),
        NativeOutputPacingMode::ReactiveDouble,
        21,
        test_direct_key(3),
        92,
        None,
        test_frame_batch_id(11),
        7,
        OutputReleasePlan::Pageflip,
    )
    .expect("direct transaction");
    ledger
        .insert(transaction)
        .expect("insert direct transaction");

    assert_eq!(
        ledger.mark_presented(
            transaction_id,
            token,
            generation,
            MonotonicTimestampNs::new(30),
            Some(2),
        ),
        Err(OutputTransactionError::InvalidTransition {
            from: super::OutputTransactionStateKind::Built,
            requested: super::OutputTransactionTransitionKind::Presented,
        })
    );

    ledger
        .mark_submitted(transaction_id, token, MonotonicTimestampNs::new(20))
        .expect("submit direct transaction");
    assert_eq!(
        ledger.submitted_transaction(token, generation),
        Some(transaction_id)
    );

    ledger
        .mark_presented(
            transaction_id,
            token,
            generation,
            MonotonicTimestampNs::new(30),
            Some(2),
        )
        .expect("present direct transaction");

    assert_eq!(ledger.counters().presented_direct, 1);
    assert_eq!(ledger.active_count(), 0);
    assert_eq!(
        ledger
            .recent_terminal()
            .back()
            .expect("terminal record")
            .state(),
        OutputTransactionState::Terminal(OutputTransactionTerminal::Presented {
            presented_at: MonotonicTimestampNs::new(30),
            actual_sequence: Some(2),
        })
    );
}
