pub(crate) use oblivion_one::native::event_loop::{
    NativeContinuationReason, NativeContinuationReasons,
};
use oblivion_one::native::scheduler::{SchedulerWakeDeadline, SchedulerWakeDeadlineKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeDeadlineOwner {
    FrameScheduler,
    PresentationTarget,
    AtomicCommitWatchdog,
    ExplicitSyncFallback,
    XwaylandTimeout,
    CursorResponse,
    ControlTimeout,
    SurfacePacing,
    DmabufRetry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeDeadline {
    pub(crate) owner: NativeDeadlineOwner,
    pub(crate) at_ns: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct NativeWakePlan {
    pub(crate) continuation: NativeContinuationReasons,
    pub(crate) deadline: Option<NativeDeadline>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct NativeWakePlanInputs {
    pub(crate) now_ns: u64,
    pub(crate) scheduler_deadline: Option<NativeDeadline>,
    pub(crate) atomic_commit_watchdog_deadline_ns: Option<u64>,
    pub(crate) explicit_sync_fallback_deadline_ns: Option<u64>,
    pub(crate) xwayland_timeout_deadline_ns: Option<u64>,
    pub(crate) cursor_response_deadline_ns: Option<u64>,
    pub(crate) control_timeout_deadline_ns: Option<u64>,
    pub(crate) surface_pacing_deadline_ns: Option<u64>,
    pub(crate) dmabuf_retry_deadline_ns: Option<u64>,
    pub(crate) input_backlog: bool,
    pub(crate) astrea_publication: bool,
    pub(crate) commit_timing_planning: bool,
    pub(crate) xwayland_continuation: bool,
}

pub(crate) fn build_native_wake_plan(inputs: NativeWakePlanInputs) -> NativeWakePlan {
    let mut continuation = NativeContinuationReasons::default();
    if inputs.input_backlog {
        continuation = continuation.insert(NativeContinuationReason::InputBacklog);
    }
    if inputs.astrea_publication {
        continuation = continuation.insert(NativeContinuationReason::AstreaPublication);
    }
    if inputs.commit_timing_planning {
        continuation = continuation.insert(NativeContinuationReason::CommitTimingPlanning);
    }
    if inputs.xwayland_continuation {
        continuation = continuation.insert(NativeContinuationReason::XwaylandContinuation);
    }

    let mut deadline = inputs.scheduler_deadline;
    deadline = earliest_deadline(
        deadline,
        inputs
            .atomic_commit_watchdog_deadline_ns
            .map(|at_ns| NativeDeadline {
                owner: NativeDeadlineOwner::AtomicCommitWatchdog,
                at_ns,
            }),
    );
    deadline = earliest_deadline(
        deadline,
        inputs
            .explicit_sync_fallback_deadline_ns
            .map(|at_ns| NativeDeadline {
                owner: NativeDeadlineOwner::ExplicitSyncFallback,
                at_ns,
            }),
    );
    deadline = earliest_deadline(
        deadline,
        inputs
            .xwayland_timeout_deadline_ns
            .map(|at_ns| NativeDeadline {
                owner: NativeDeadlineOwner::XwaylandTimeout,
                at_ns,
            }),
    );
    deadline = earliest_deadline(
        deadline,
        inputs
            .cursor_response_deadline_ns
            .map(|at_ns| NativeDeadline {
                owner: NativeDeadlineOwner::CursorResponse,
                at_ns,
            }),
    );
    deadline = earliest_deadline(
        deadline,
        inputs
            .control_timeout_deadline_ns
            .map(|at_ns| NativeDeadline {
                owner: NativeDeadlineOwner::ControlTimeout,
                at_ns,
            }),
    );
    deadline = earliest_deadline(
        deadline,
        inputs
            .surface_pacing_deadline_ns
            .map(|at_ns| NativeDeadline {
                owner: NativeDeadlineOwner::SurfacePacing,
                at_ns,
            }),
    );
    deadline = earliest_deadline(
        deadline,
        inputs.dmabuf_retry_deadline_ns.map(|at_ns| NativeDeadline {
            owner: NativeDeadlineOwner::DmabufRetry,
            at_ns,
        }),
    );

    let _ = inputs.now_ns;
    NativeWakePlan {
        continuation,
        deadline,
    }
}

pub(crate) fn native_deadline_from_scheduler(deadline: SchedulerWakeDeadline) -> NativeDeadline {
    let owner = match deadline.kind {
        SchedulerWakeDeadlineKind::RenderStart | SchedulerWakeDeadlineKind::SubmitNotBefore => {
            NativeDeadlineOwner::PresentationTarget
        }
        SchedulerWakeDeadlineKind::ProtocolRefresh
        | SchedulerWakeDeadlineKind::PageFlipWatchdog => NativeDeadlineOwner::FrameScheduler,
    };
    NativeDeadline {
        owner,
        at_ns: deadline.at_ns,
    }
}

const fn earliest_deadline(
    current: Option<NativeDeadline>,
    candidate: Option<NativeDeadline>,
) -> Option<NativeDeadline> {
    match (current, candidate) {
        (Some(current), Some(candidate)) if candidate.at_ns < current.at_ns => Some(candidate),
        (Some(current), Some(_)) => Some(current),
        (None, Some(candidate)) => Some(candidate),
        (current, None) => current,
    }
}

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

        assert!(
            plan.continuation
                .contains(NativeContinuationReason::InputBacklog)
        );
        assert_ne!(
            plan.deadline.map(|deadline| deadline.at_ns),
            Some(10_000_000)
        );
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

        assert!(
            plan.continuation
                .contains(NativeContinuationReason::InputBacklog)
        );
        assert!(
            plan.continuation
                .contains(NativeContinuationReason::AstreaPublication)
        );
        assert!(
            plan.continuation
                .contains(NativeContinuationReason::CommitTimingPlanning)
        );
        assert!(
            plan.continuation
                .contains(NativeContinuationReason::XwaylandContinuation)
        );
        assert_eq!(plan.deadline, None);
    }
}
