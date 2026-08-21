use super::thread::worker_wait_is_armed;
use super::timing::KmsWorkerDispatchModel;
use super::*;
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
    model.record(10_000, 40_000, 60_000);
    model.record(20_000, 50_000, 70_000);

    let budget = model.budget();

    assert_eq!(budget.wake_lateness_ns, 20_000);
    assert_eq!(budget.pre_submit_ns, 50_000);
    assert_eq!(budget.ioctl_duration_ns, 70_000);
    assert_eq!(budget.dispatch_budget_ns, 190_000);
}

#[test]
fn worker_dispatch_budget_does_not_include_queue_residency() {
    let mut model = KmsWorkerDispatchModel::default();
    model.record(0, 100_000, 200_000);

    assert_eq!(model.budget().dispatch_budget_ns, 350_000);
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
