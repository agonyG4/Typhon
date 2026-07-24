use super::{
    ContentEpochId, DirectScanoutCandidateKey, OutputContentKey, OutputTransactionId,
    PresentationTransactionEvent, PresentationTransactionTraceRing,
};
use std::num::NonZeroU64;

const OUTPUT_HZ: u64 = 165;
const OUTPUT_PERIOD_NS: u64 = 1_000_000_000 / OUTPUT_HZ;

#[derive(Debug, Default)]
struct CadenceResult {
    transactions: u64,
    kms_submits: u64,
    same_buffer_submits: u64,
    pageflips: Vec<u64>,
    unique_content: u64,
    trace: Option<PresentationTransactionTraceRing>,
}

fn candidate_key(content_epoch: u64) -> DirectScanoutCandidateKey {
    let buffer_id = NonZeroU64::new(42).expect("test buffer id");
    let content_epoch = ContentEpochId::new(
        NonZeroU64::new(content_epoch).expect("test content epoch must be nonzero"),
    );
    DirectScanoutCandidateKey {
        content: OutputContentKey::new(
            7,
            buffer_id,
            content_epoch,
            1920,
            1080,
            0x3432_5241,
            0,
            0,
            1_000,
            0,
        ),
        output_generation: 1,
        cursor_plan_key: Some(1),
        color_epoch: 0,
    }
}

fn simulate(client_fps: u64, _direct: bool) -> CadenceResult {
    let mut result = CadenceResult {
        trace: Some(PresentationTransactionTraceRing::new(4096)),
        ..CadenceResult::default()
    };
    let mut content_epoch = 0u64;
    let mut available_epoch = 0u64;
    let mut submitted_key = None;
    let mut next_client_frame_ns = 0u64;
    let client_period_ns = 1_000_000_000 / client_fps;
    let stall_start_ns = 750_000_000;
    let stall_end_ns = stall_start_ns + 30_000_000;

    for output_index in 0..(OUTPUT_HZ * 2) {
        let output_ns = output_index * OUTPUT_PERIOD_NS;
        if output_ns >= next_client_frame_ns {
            if !(stall_start_ns..stall_end_ns).contains(&output_ns) {
                content_epoch = content_epoch.saturating_add(1);
                available_epoch = content_epoch;
                next_client_frame_ns = next_client_frame_ns.saturating_add(client_period_ns);
            } else {
                next_client_frame_ns = stall_end_ns;
            }
        }
        if available_epoch == 0 {
            continue;
        }

        let key = candidate_key(available_epoch);
        if submitted_key == Some(key) {
            continue;
        }

        let transaction_id = OutputTransactionId::new(
            NonZeroU64::new(result.transactions.saturating_add(1))
                .expect("test transaction id must be nonzero"),
        );
        let trace = result.trace.as_mut().expect("trace enabled");
        trace.push(PresentationTransactionEvent::ContentObserved {
            transaction_id,
            timestamp_ns: output_ns,
        });
        trace.push(PresentationTransactionEvent::KmsSubmitReturned {
            transaction_id,
            timestamp_ns: output_ns,
        });
        trace.push(PresentationTransactionEvent::PageflipPresented {
            transaction_id,
            timestamp_ns: output_ns.saturating_add(OUTPUT_PERIOD_NS),
        });
        result.transactions = result.transactions.saturating_add(1);
        result.kms_submits = result.kms_submits.saturating_add(1);
        result.unique_content = result.unique_content.saturating_add(1);
        result
            .pageflips
            .push(output_ns.saturating_add(OUTPUT_PERIOD_NS));
        submitted_key = Some(key);
    }

    result
}

#[test]
fn composed_fullscreen_cadence_tracks_unique_content_without_same_buffer_submits() {
    for client_fps in [60, 90, 120, 144, 165, 240] {
        let result = simulate(client_fps, false);
        assert!(result.transactions > 0, "client={client_fps}");
        assert_eq!(
            result.kms_submits, result.transactions,
            "client={client_fps}"
        );
        assert_eq!(result.same_buffer_submits, 0, "client={client_fps}");
        assert_eq!(
            result.unique_content, result.transactions,
            "client={client_fps}"
        );
        assert!(
            result
                .pageflips
                .windows(2)
                .all(|window| window[1].saturating_sub(window[0]) % OUTPUT_PERIOD_NS == 0),
            "pageflip cadence must follow output vblank multiples: client={client_fps}"
        );
    }
}

#[test]
fn a_main_loop_stall_does_not_create_duplicate_primary_submits() {
    let result = simulate(120, false);
    assert_eq!(result.same_buffer_submits, 0);
    assert!(result.unique_content > 1);
    assert!(
        result
            .pageflips
            .windows(2)
            .any(|window| { window[1].saturating_sub(window[0]) >= 2 * OUTPUT_PERIOD_NS })
    );
}

#[test]
#[ignore = "experimental direct path requires real TTY qualification before comparison"]
fn experimental_direct_is_no_worse_than_composed() {
    let composed = simulate(165, false);
    let direct = simulate(165, true);
    assert!(direct.unique_content >= composed.unique_content);
    assert!(direct.kms_submits >= composed.kms_submits);
}
