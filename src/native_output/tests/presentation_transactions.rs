use super::{
    ContentEpochId, ContentEpochTracker, DirectScanoutCandidateKey, OutputContentKey,
    PresentationTransactionId,
};
use crate::native_output::presentation::qualification::{DirectReleaseMode, DirectSyncReadiness};
use crate::native_output::presentation::trace::{
    PresentationTransactionEvent, PresentationTransactionTraceRing,
};
use std::num::NonZeroU64;

fn tx(value: u64) -> PresentationTransactionId {
    PresentationTransactionId::new(NonZeroU64::new(value).expect("non-zero transaction id"))
}

fn observed(id: PresentationTransactionId, timestamp_ns: u64) -> PresentationTransactionEvent {
    PresentationTransactionEvent::ContentObserved {
        transaction_id: id,
        timestamp_ns,
    }
}

fn submitted(id: PresentationTransactionId, timestamp_ns: u64) -> PresentationTransactionEvent {
    PresentationTransactionEvent::KmsSubmitReturned {
        transaction_id: id,
        timestamp_ns,
    }
}

fn presented(id: PresentationTransactionId, timestamp_ns: u64) -> PresentationTransactionEvent {
    PresentationTransactionEvent::PageflipPresented {
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
    let transaction_id = PresentationTransactionId::new(
        NonZeroU64::new(11).expect("test transaction id is non-zero"),
    );
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
        cursor_plan_key: Some(1),
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
            cursor_plan_key: Some(2),
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
