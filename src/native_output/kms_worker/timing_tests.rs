use super::queue::{WorkerPresentationFeedback, take_presentation_feedback_for_generation};
use super::timing::KmsTimingDecision;
use super::*;
use oblivion_one::native::presentation_deadline::{
    MonotonicTimestampNs, PresentationTarget, PresentationTargetReason,
};
use std::time::Duration;

#[test]
fn worker_timing_records_pageflip_ack_delay() {
    let metrics = WorkerTimingMetrics::default();

    metrics.record_pageflip_ack_delay(4_000_000);

    let snapshot = metrics.snapshot();
    assert_eq!(snapshot.pageflip_ack_delay.count, 1);
    assert_eq!(snapshot.pageflip_ack_delay.mean_ns, 4_000_000);
    assert_eq!(snapshot.pageflip_ack_delay.p95_ns, 5_000_000);
}

#[test]
fn worker_timing_records_submit_ack_delay() {
    let metrics = WorkerTimingMetrics::default();

    metrics.record_submit_ack_delay(300_000);

    let snapshot = metrics.snapshot();
    assert_eq!(snapshot.submit_ack_delay.count, 1);
    assert_eq!(snapshot.submit_ack_delay.mean_ns, 300_000);
    assert_eq!(snapshot.submit_ack_delay.p95_ns, 500_000);
}

#[test]
fn presentation_miss_increases_margin_from_observed_submit_headroom() {
    let mut model = KmsCommitTimingModel::new(Duration::from_millis(16));
    model.observe_missed_target(18_000_000, 20_000_000);
    assert_eq!(model.safety_margin_ns(), 2_100_000);
}

#[test]
fn presentation_miss_floor_survives_next_submission_sample() {
    let mut model = KmsCommitTimingModel::new(Duration::from_millis(16));
    model.observe_missed_target(18_000_000, 20_000_000);
    model.observe_submission(0, 0);
    assert_eq!(model.safety_margin_ns(), 2_100_000);
}

#[test]
fn presentation_miss_floor_decays_after_stable_early_samples() {
    let mut model = KmsCommitTimingModel::new(Duration::from_millis(16));
    model.observe_missed_target(18_000_000, 20_000_000);
    for _ in 0..16 {
        model.observe_submit_delta_ns(-1_000_000);
    }
    assert!(model.safety_margin_ns() < 2_100_000);
}

#[test]
fn stale_presentation_feedback_is_discarded_across_generations() {
    let mut feedback = Some(WorkerPresentationFeedback {
        output_generation: 1,
        target_sequence: 4,
        presented_sequence: 5,
        submit_returned_at_ns: 10,
        target_presentation_ns: 20,
        refresh_interval_ns: 10,
    });

    assert!(take_presentation_feedback_for_generation(&mut feedback, 2).is_none());
    assert!(feedback.is_none());
}

#[test]
fn presentation_miss_does_not_reduce_existing_margin() {
    let mut model = KmsCommitTimingModel::new(Duration::from_millis(16));
    model.observe_submit_delta_ns(3_000_000);
    model.observe_missed_target(19_000_000, 20_000_000);
    assert_eq!(model.safety_margin_ns(), 3_000_000);
}

#[test]
fn worker_target_feedback_classifies_refresh_distance() {
    assert_eq!(
        WorkerTargetResult::from_sequences(42, 42),
        WorkerTargetResult::SameRefresh
    );
    assert_eq!(
        WorkerTargetResult::from_sequences(42, 43),
        WorkerTargetResult::MissedOneRefresh
    );
    assert_eq!(
        WorkerTargetResult::from_sequences(42, 44),
        WorkerTargetResult::MissedTwoOrMoreRefreshes
    );
    assert_eq!(
        WorkerTargetResult::from_sequences(42, 41),
        WorkerTargetResult::StaleOrOutOfOrder
    );
}

#[test]
fn timing_target_decision_preserves_reactive_submit_not_before() {
    let target = PresentationTarget {
        sequence: 1,
        presentation_time: MonotonicTimestampNs::new(20_000_000),
        submit_not_before: MonotonicTimestampNs::new(10_000_000),
        render_start_deadline: MonotonicTimestampNs::new(0),
        refresh_interval: Duration::from_millis(16),
        reason: PresentationTargetReason::ReactiveDouble,
        clock_generation: 1,
        estimated: false,
        predicted_unreachable: false,
    };
    let model = KmsCommitTimingModel::new(target.refresh_interval);

    assert_eq!(
        model.submit_at(target, 1_000_000),
        KmsTimingDecision {
            submit_deadline_ns: 10_000_000,
            submit_at_ns: 10_000_000,
            late: false,
            late_by_ns: 0,
        }
    );
}
