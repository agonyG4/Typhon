use super::thread::worker_wait_is_armed;
use super::timing::KmsWorkerDispatchModel;
use super::*;
use crate::native_output::presentation::kms_timing::{KmsPresentationOutcome, KmsSubmitWindow};
use oblivion_one::native::presentation_deadline::PresentationTargetReason;

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
    assert_eq!(snapshot.submit_ack_delay.p95_ns, 375_000);
}

#[test]
fn worker_dispatch_budget_uses_actual_wake_and_post_wake_dispatch() {
    let mut model = KmsWorkerDispatchModel::default();
    model.record(10_000, 40_000, 60_000, 100_000);
    model.record(20_000, 50_000, 70_000, 120_000);

    let budget = model.budget();

    assert_eq!(budget.wake_lateness_ns, 20_000);
    assert_eq!(budget.pre_submit_ns, 50_000);
    assert_eq!(budget.ioctl_duration_ns, 70_000);
    assert_eq!(budget.dispatch_budget_ns, 190_000);
}

#[test]
fn worker_dispatch_budget_does_not_include_queue_residency() {
    let mut model = KmsWorkerDispatchModel::default();
    model.record(0, 100_000, 200_000, 300_000);

    assert_eq!(model.budget().dispatch_budget_ns, 350_000);
}

#[test]
fn dispatch_tail_deadline_hole_is_not_render_readiness_miss() {
    let mut model = KmsWorkerDispatchModel::default();
    model.record(0, 0, 100_000, 100_000);
    let window = KmsSubmitWindow::try_new(10_000_000, 0, model.budget().dispatch_budget_ns, 0)
        .expect("test window is reachable");
    let payload_ready_at_ns = 1_000_000;
    let submit_returned_at_ns = window.commit_complete_deadline_ns() + 12_230;

    assert!(payload_ready_at_ns < window.commit_complete_deadline_ns());
    assert!(window.is_dispatch_miss_at(submit_returned_at_ns));
    assert_eq!(
        KmsPresentationOutcome::classify(
            &window,
            Some(payload_ready_at_ns),
            submit_returned_at_ns,
            1,
            2,
        ),
        KmsPresentationOutcome::KmsDispatchMiss
    );
}

#[test]
fn late_payload_dispatch_overrun_does_not_update_tail_metrics() {
    let mut model = KmsWorkerDispatchModel::default();
    model.record(0, 0, 100_000, 100_000);
    let metrics = WorkerTimingMetrics::default();

    let observation = model.observe_submission_deadline(1_000_000, 1_012_230, false);
    metrics.record_dispatch_tail(observation);
    let snapshot = metrics.snapshot();

    assert_eq!(snapshot.dispatch_tail_guard_ns, 0);
    assert_eq!(snapshot.dispatch_tail_guard_increases, 0);
    assert_eq!(snapshot.dispatch_deadline_overrun_ns, 12_230);
}

#[test]
fn reactive_double_does_not_wait_for_a_late_planned_worker_wake() {
    assert!(!worker_wait_is_armed(
        PresentationTargetReason::ReactiveDouble,
        200,
        100,
    ));
    assert!(worker_wait_is_armed(
        PresentationTargetReason::PredictedPressure,
        200,
        100,
    ));
}
