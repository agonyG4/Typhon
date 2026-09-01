use oblivion_one::native::event_loop::NativeEventLoop;
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
    pub(crate) control_timeout_pending: bool,
}

#[derive(Debug, Default)]
pub(crate) struct NativeWakeAuthorityMetrics {
    pub(crate) runtime_timer_arms: u64,
    pub(crate) runtime_timer_disarms: u64,
    pub(crate) stale_deadline_rearms: u64,
    pub(crate) past_deadline_arms: u64,
    pub(crate) input_backlog_continuations: u64,
    pub(crate) astrea_publication_continuations: u64,
    pub(crate) commit_timing_planning_continuations: u64,
    pub(crate) xwayland_continuations: u64,
    pub(crate) control_timeout_continuations: u64,
    pub(crate) deadline_owner_frame_scheduler: u64,
    pub(crate) deadline_owner_presentation_target: u64,
    pub(crate) deadline_owner_atomic_watchdog: u64,
    pub(crate) deadline_owner_explicit_sync: u64,
    pub(crate) deadline_owner_xwayland: u64,
    pub(crate) deadline_owner_cursor: u64,
    pub(crate) deadline_owner_control: u64,
    pub(crate) deadline_owner_surface_pacing: u64,
    pub(crate) deadline_owner_dmabuf_retry: u64,
}

impl NativeWakeAuthorityMetrics {
    pub(crate) fn note_continuation(&mut self, reason: NativeContinuationReason) {
        match reason {
            NativeContinuationReason::InputBacklog => {
                self.input_backlog_continuations =
                    self.input_backlog_continuations.saturating_add(1)
            }
            NativeContinuationReason::AstreaPublication => {
                self.astrea_publication_continuations =
                    self.astrea_publication_continuations.saturating_add(1)
            }
            NativeContinuationReason::CommitTimingPlanning => {
                self.commit_timing_planning_continuations =
                    self.commit_timing_planning_continuations.saturating_add(1)
            }
            NativeContinuationReason::XwaylandContinuation => {
                self.xwayland_continuations = self.xwayland_continuations.saturating_add(1)
            }
            NativeContinuationReason::ControlTimeout => {
                self.control_timeout_continuations =
                    self.control_timeout_continuations.saturating_add(1)
            }
        }
    }

    pub(crate) fn observe_plan(
        &mut self,
        plan: NativeWakePlan,
        now_ns: u64,
        previously_armed: Option<u64>,
    ) {
        match plan.deadline {
            Some(deadline) => {
                self.runtime_timer_arms = self.runtime_timer_arms.saturating_add(1);
                if deadline.at_ns <= now_ns {
                    self.past_deadline_arms = self.past_deadline_arms.saturating_add(1);
                }
                if previously_armed == Some(deadline.at_ns) && deadline.at_ns <= now_ns {
                    self.stale_deadline_rearms = self.stale_deadline_rearms.saturating_add(1);
                }
                match deadline.owner {
                    NativeDeadlineOwner::FrameScheduler => {
                        self.deadline_owner_frame_scheduler =
                            self.deadline_owner_frame_scheduler.saturating_add(1)
                    }
                    NativeDeadlineOwner::PresentationTarget => {
                        self.deadline_owner_presentation_target =
                            self.deadline_owner_presentation_target.saturating_add(1)
                    }
                    NativeDeadlineOwner::AtomicCommitWatchdog => {
                        self.deadline_owner_atomic_watchdog =
                            self.deadline_owner_atomic_watchdog.saturating_add(1)
                    }
                    NativeDeadlineOwner::ExplicitSyncFallback => {
                        self.deadline_owner_explicit_sync =
                            self.deadline_owner_explicit_sync.saturating_add(1)
                    }
                    NativeDeadlineOwner::XwaylandTimeout => {
                        self.deadline_owner_xwayland =
                            self.deadline_owner_xwayland.saturating_add(1)
                    }
                    NativeDeadlineOwner::CursorResponse => {
                        self.deadline_owner_cursor = self.deadline_owner_cursor.saturating_add(1)
                    }
                    NativeDeadlineOwner::ControlTimeout => {
                        self.deadline_owner_control = self.deadline_owner_control.saturating_add(1)
                    }
                    NativeDeadlineOwner::SurfacePacing => {
                        self.deadline_owner_surface_pacing =
                            self.deadline_owner_surface_pacing.saturating_add(1)
                    }
                    NativeDeadlineOwner::DmabufRetry => {
                        self.deadline_owner_dmabuf_retry =
                            self.deadline_owner_dmabuf_retry.saturating_add(1)
                    }
                }
            }
            None => {
                self.runtime_timer_disarms = self.runtime_timer_disarms.saturating_add(1);
            }
        }
    }

    pub(crate) fn summary_line(&self, event_loop: &NativeEventLoop) -> String {
        format!(
            "event=native_wake_authority_summary runtime_timer_arms={} runtime_timer_disarms={} runtime_continuation_requests={} runtime_continuation_coalesced={} runtime_continuation_wakes={} input_backlog_continuations={} astrea_publication_continuations={} commit_timing_planning_continuations={} xwayland_continuations={} control_timeout_continuations={} stale_deadline_rearms={} past_deadline_arms={} deadline_owner_frame_scheduler={} deadline_owner_presentation_target={} deadline_owner_atomic_watchdog={} deadline_owner_explicit_sync={} deadline_owner_xwayland={} deadline_owner_cursor={} deadline_owner_control={} deadline_owner_surface_pacing={} deadline_owner_dmabuf_retry={}",
            self.runtime_timer_arms,
            self.runtime_timer_disarms,
            event_loop.continuation_requests(),
            event_loop.continuation_coalesced(),
            event_loop.continuation_wakes(),
            self.input_backlog_continuations,
            self.astrea_publication_continuations,
            self.commit_timing_planning_continuations,
            self.xwayland_continuations,
            self.control_timeout_continuations,
            self.stale_deadline_rearms,
            self.past_deadline_arms,
            self.deadline_owner_frame_scheduler,
            self.deadline_owner_presentation_target,
            self.deadline_owner_atomic_watchdog,
            self.deadline_owner_explicit_sync,
            self.deadline_owner_xwayland,
            self.deadline_owner_cursor,
            self.deadline_owner_control,
            self.deadline_owner_surface_pacing,
            self.deadline_owner_dmabuf_retry,
        )
    }
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
    if inputs.control_timeout_pending {
        continuation = continuation.insert(NativeContinuationReason::ControlTimeout);
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

    #[test]
    fn stale_same_deadline_rearm_is_counted_without_clamping() {
        let plan = NativeWakePlan {
            deadline: Some(NativeDeadline {
                owner: NativeDeadlineOwner::PresentationTarget,
                at_ns: 5,
            }),
            ..NativeWakePlan::default()
        };
        let mut metrics = NativeWakeAuthorityMetrics::default();

        metrics.observe_plan(plan, 5, Some(5));

        assert_eq!(metrics.stale_deadline_rearms, 1);
        assert_eq!(metrics.past_deadline_arms, 1);
    }
}
