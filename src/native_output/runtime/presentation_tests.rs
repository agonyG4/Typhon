use super::super::planner::visual_target_deadline_for_mode;
use super::*;

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
