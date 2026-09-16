use super::*;
use oblivion_one::compositor::CompositorFrameBatchId;
use oblivion_one::core::OutputId;
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
        physical_claim: oblivion_one::native::presentation_deadline::PrimaryRefreshClaim {
            sequence: 2,
            presentation_time: now,
            clock_generation: 1,
        },
        selection_evidence: Default::default(),
    }
}

#[test]
fn transaction_from_one_logical_output_cannot_enter_another_ledger() {
    let first = OutputId::from_raw(1).expect("nonzero output id");
    let second = OutputId::from_raw(2).expect("nonzero output id");
    let mut first_ledger = OutputTransactionLedger::for_output(first, 8, 64);
    let mut second_ledger = OutputTransactionLedger::for_output(second, 8, 64);
    let id = second_ledger.allocate_id().expect("transaction ID");
    let transaction = OutputTransaction::composited(
        second,
        id,
        1,
        MonotonicTimestampNs::new(10),
        test_target(),
        NativeOutputPacingMode::ReactiveDouble,
        id.get(),
        12,
        13,
        OutputSlotId::new(0).expect("slot zero"),
        91,
        None,
        CompositorFrameBatchId::new(NonZeroU64::new(99).expect("batch ID")),
    )
    .expect("composited transaction");

    assert_eq!(transaction.output_id(), second);
    second_ledger
        .insert(transaction.clone())
        .expect("second ledger accepts its own transaction");
    assert_eq!(
        first_ledger.insert(transaction),
        Err(OutputTransactionError::OutputMismatch)
    );
}
