#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expired_visual_deadline_is_not_selected_for_worker_blocker() {
        let plan = build_native_wake_plan(NativeWakePlanInputs {
            now_ns: 5_000_000,
            scheduler_deadline: None,
            input_backlog: false,
            astrea_publication: false,
            commit_timing_planning: false,
            xwayland_continuation: false,
            ..NativeWakePlanInputs::default()
        });

        assert_eq!(plan.deadline, None);
        assert_eq!(plan.continuation, NativeContinuationReasons::default());
    }

    #[test]
    fn future_refresh_deadline_is_selected_with_owner() {
        let plan = build_native_wake_plan(NativeWakePlanInputs {
            now_ns: 4_000_000,
            scheduler_deadline: Some(NativeDeadline {
                owner: NativeDeadlineOwner::FrameScheduler,
                at_ns: 5_000_000,
            }),
            ..NativeWakePlanInputs::default()
        });

        assert_eq!(
            plan.deadline,
            Some(NativeDeadline {
                owner: NativeDeadlineOwner::FrameScheduler,
                at_ns: 5_000_000,
            })
        );
    }

    #[test]
    fn input_backlog_is_continuation_not_now_deadline() {
        let plan = build_native_wake_plan(NativeWakePlanInputs {
            now_ns: 10_000_000,
            input_backlog: true,
            ..NativeWakePlanInputs::default()
        });

        assert!(plan.continuation.contains(NativeContinuationReason::InputBacklog));
        assert_ne!(plan.deadline.map(|deadline| deadline.at_ns), Some(10_000_000));
    }

    #[test]
    fn all_immediate_reasons_are_coalesced_without_a_timer() {
        let plan = build_native_wake_plan(NativeWakePlanInputs {
            input_backlog: true,
            astrea_publication: true,
            commit_timing_planning: true,
            xwayland_continuation: true,
            ..NativeWakePlanInputs::default()
        });

        assert!(plan.continuation.contains(NativeContinuationReason::InputBacklog));
        assert!(plan
            .continuation
            .contains(NativeContinuationReason::AstreaPublication));
        assert!(plan
            .continuation
            .contains(NativeContinuationReason::CommitTimingPlanning));
        assert!(plan
            .continuation
            .contains(NativeContinuationReason::XwaylandContinuation));
        assert_eq!(plan.deadline, None);
    }
}
