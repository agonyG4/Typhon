use crate::native_output::kms_worker::KmsCommitWorkerTransport;
use oblivion_one::native::event_loop::NativeEventLoop;
pub(crate) use oblivion_one::native::event_loop::{
    NativeContinuationReason, NativeContinuationReasons,
};
use oblivion_one::native::scheduler::{SchedulerWakeDeadline, SchedulerWakeDeadlineKind};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum NativePageflipTimeoutOwner {
    #[default]
    MainThread,
    KmsWorker,
}

impl From<KmsCommitWorkerTransport> for NativePageflipTimeoutOwner {
    fn from(transport: KmsCommitWorkerTransport) -> Self {
        match transport {
            KmsCommitWorkerTransport::Synchronous => Self::MainThread,
            KmsCommitWorkerTransport::Worker => Self::KmsWorker,
        }
    }
}

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

const DEADLINE_OWNER_COUNT: usize = 9;

const fn deadline_owner_index(owner: NativeDeadlineOwner) -> usize {
    match owner {
        NativeDeadlineOwner::FrameScheduler => 0,
        NativeDeadlineOwner::PresentationTarget => 1,
        NativeDeadlineOwner::AtomicCommitWatchdog => 2,
        NativeDeadlineOwner::ExplicitSyncFallback => 3,
        NativeDeadlineOwner::XwaylandTimeout => 4,
        NativeDeadlineOwner::CursorResponse => 5,
        NativeDeadlineOwner::ControlTimeout => 6,
        NativeDeadlineOwner::SurfacePacing => 7,
        NativeDeadlineOwner::DmabufRetry => 8,
    }
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
    pub(crate) stale_deadline_arms_by_owner: [u64; DEADLINE_OWNER_COUNT],
    pub(crate) past_deadline_arms_by_owner: [u64; DEADLINE_OWNER_COUNT],
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
        fired_deadline_ns: Option<u64>,
    ) {
        match plan.deadline {
            Some(deadline) => {
                self.runtime_timer_arms = self.runtime_timer_arms.saturating_add(1);
                if deadline.at_ns <= now_ns {
                    self.past_deadline_arms = self.past_deadline_arms.saturating_add(1);
                    let owner = deadline_owner_index(deadline.owner);
                    self.past_deadline_arms_by_owner[owner] =
                        self.past_deadline_arms_by_owner[owner].saturating_add(1);
                }
                if fired_deadline_ns
                    .or(previously_armed)
                    .is_some_and(|previous| previous == deadline.at_ns)
                    && deadline.at_ns <= now_ns
                {
                    self.stale_deadline_rearms = self.stale_deadline_rearms.saturating_add(1);
                    let owner = deadline_owner_index(deadline.owner);
                    self.stale_deadline_arms_by_owner[owner] =
                        self.stale_deadline_arms_by_owner[owner].saturating_add(1);
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

    pub(crate) fn observe_plan_after_wake(
        &mut self,
        plan: NativeWakePlan,
        now_ns: u64,
        event_loop: &mut NativeEventLoop,
    ) {
        let fired_deadline_ns = event_loop.take_fired_deadline_ns();
        self.observe_plan(
            plan,
            now_ns,
            event_loop.armed_deadline_ns(),
            fired_deadline_ns,
        );
    }

    pub(crate) fn summary_line(&self, event_loop: &NativeEventLoop) -> String {
        format!(
            "event=native_wake_authority_summary runtime_timer_arms={} runtime_timer_disarms={} runtime_continuation_requests={} runtime_continuation_coalesced={} runtime_continuation_wakes={} input_backlog_continuations={} astrea_publication_continuations={} commit_timing_planning_continuations={} xwayland_continuations={} control_timeout_continuations={} stale_deadline_rearms={} past_deadline_arms={} stale_frame_scheduler={} stale_presentation_target={} stale_atomic_watchdog={} stale_explicit_sync={} stale_xwayland={} stale_cursor={} stale_control={} stale_surface_pacing={} stale_dmabuf_retry={} past_frame_scheduler={} past_presentation_target={} past_atomic_watchdog={} past_explicit_sync={} past_xwayland={} past_cursor={} past_control={} past_surface_pacing={} past_dmabuf_retry={} deadline_owner_frame_scheduler={} deadline_owner_presentation_target={} deadline_owner_atomic_watchdog={} deadline_owner_explicit_sync={} deadline_owner_xwayland={} deadline_owner_cursor={} deadline_owner_control={} deadline_owner_surface_pacing={} deadline_owner_dmabuf_retry={}",
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
            self.stale_deadline_arms_by_owner
                [deadline_owner_index(NativeDeadlineOwner::FrameScheduler)],
            self.stale_deadline_arms_by_owner
                [deadline_owner_index(NativeDeadlineOwner::PresentationTarget)],
            self.stale_deadline_arms_by_owner
                [deadline_owner_index(NativeDeadlineOwner::AtomicCommitWatchdog)],
            self.stale_deadline_arms_by_owner
                [deadline_owner_index(NativeDeadlineOwner::ExplicitSyncFallback)],
            self.stale_deadline_arms_by_owner
                [deadline_owner_index(NativeDeadlineOwner::XwaylandTimeout)],
            self.stale_deadline_arms_by_owner
                [deadline_owner_index(NativeDeadlineOwner::CursorResponse)],
            self.stale_deadline_arms_by_owner
                [deadline_owner_index(NativeDeadlineOwner::ControlTimeout)],
            self.stale_deadline_arms_by_owner
                [deadline_owner_index(NativeDeadlineOwner::SurfacePacing)],
            self.stale_deadline_arms_by_owner
                [deadline_owner_index(NativeDeadlineOwner::DmabufRetry)],
            self.past_deadline_arms_by_owner
                [deadline_owner_index(NativeDeadlineOwner::FrameScheduler)],
            self.past_deadline_arms_by_owner
                [deadline_owner_index(NativeDeadlineOwner::PresentationTarget)],
            self.past_deadline_arms_by_owner
                [deadline_owner_index(NativeDeadlineOwner::AtomicCommitWatchdog)],
            self.past_deadline_arms_by_owner
                [deadline_owner_index(NativeDeadlineOwner::ExplicitSyncFallback)],
            self.past_deadline_arms_by_owner
                [deadline_owner_index(NativeDeadlineOwner::XwaylandTimeout)],
            self.past_deadline_arms_by_owner
                [deadline_owner_index(NativeDeadlineOwner::CursorResponse)],
            self.past_deadline_arms_by_owner
                [deadline_owner_index(NativeDeadlineOwner::ControlTimeout)],
            self.past_deadline_arms_by_owner
                [deadline_owner_index(NativeDeadlineOwner::SurfacePacing)],
            self.past_deadline_arms_by_owner
                [deadline_owner_index(NativeDeadlineOwner::DmabufRetry)],
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

pub(crate) fn scheduler_deadline_for_timeout_owner(
    deadline: Option<SchedulerWakeDeadline>,
    owner: NativePageflipTimeoutOwner,
) -> Option<NativeDeadline> {
    deadline
        .filter(|deadline| {
            owner == NativePageflipTimeoutOwner::MainThread
                || deadline.kind != SchedulerWakeDeadlineKind::PageFlipWatchdog
        })
        .map(native_deadline_from_scheduler)
}

pub(crate) fn atomic_commit_watchdog_deadline_for_timeout_owner(
    deadline_ns: Option<u64>,
    owner: NativePageflipTimeoutOwner,
) -> Option<u64> {
    (owner == NativePageflipTimeoutOwner::MainThread)
        .then_some(deadline_ns)
        .flatten()
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
    use crate::native_output::runtime::NativeCursorOutputArbitration;

    fn cursor_wake_plan(
        arbitration: &NativeCursorOutputArbitration,
        now_ns: u64,
    ) -> NativeWakePlan {
        build_native_wake_plan(NativeWakePlanInputs {
            now_ns,
            cursor_response_deadline_ns: arbitration.wake_deadline_ns(now_ns),
            ..NativeWakePlanInputs::default()
        })
    }

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
    fn future_cursor_response_deadline_is_selected_for_wake() {
        let mut arbitration = NativeCursorOutputArbitration::default();
        arbitration.request(1, 0, 200);

        assert_eq!(
            cursor_wake_plan(&arbitration, 100).deadline,
            Some(NativeDeadline {
                owner: NativeDeadlineOwner::CursorResponse,
                at_ns: 200,
            })
        );
    }

    #[test]
    fn matured_cursor_debt_is_not_reinstalled_as_a_timer() {
        let mut arbitration = NativeCursorOutputArbitration::default();
        arbitration.request(1, 0, 100);
        let mut metrics = NativeWakeAuthorityMetrics::default();

        for _ in 0..128 {
            let plan = cursor_wake_plan(&arbitration, 100);
            metrics.observe_plan(plan, 100, None, None);
            assert!(arbitration.pending());
        }

        assert_eq!(metrics.runtime_timer_arms, 0);
        assert_eq!(metrics.past_deadline_arms, 0);
        assert_eq!(metrics.stale_deadline_rearms, 0);
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

        metrics.observe_plan(plan, 5, Some(5), None);

        assert_eq!(metrics.stale_deadline_rearms, 1);
        assert_eq!(metrics.past_deadline_arms, 1);
    }

    #[test]
    fn stale_and_past_deadline_arms_are_attributed_to_their_owner() {
        let plan = NativeWakePlan {
            deadline: Some(NativeDeadline {
                owner: NativeDeadlineOwner::CursorResponse,
                at_ns: 5,
            }),
            ..NativeWakePlan::default()
        };
        let mut metrics = NativeWakeAuthorityMetrics::default();

        metrics.observe_plan(plan, 5, Some(5), None);

        assert_eq!(
            metrics.stale_deadline_arms_by_owner
                [deadline_owner_index(NativeDeadlineOwner::CursorResponse)],
            1
        );
        assert_eq!(
            metrics.past_deadline_arms_by_owner
                [deadline_owner_index(NativeDeadlineOwner::CursorResponse)],
            1
        );
        assert_eq!(
            metrics.stale_deadline_arms_by_owner
                [deadline_owner_index(NativeDeadlineOwner::PresentationTarget)],
            0
        );
    }

    #[test]
    fn real_timer_wake_preserves_fired_identity_for_the_next_plan() {
        let mut event_loop = NativeEventLoop::new().unwrap();
        let fired_at_ns = oblivion_one::native::event_loop::monotonic_now_ns().unwrap();
        event_loop.arm_deadline(Some(fired_at_ns)).unwrap();
        let wakeup = event_loop.wait().unwrap();
        assert!(wakeup.reasons.timer());
        assert_eq!(event_loop.armed_deadline_ns(), None);
        assert_eq!(event_loop.fired_deadline_ns(), Some(fired_at_ns));

        let same_expired_plan = NativeWakePlan {
            deadline: Some(NativeDeadline {
                owner: NativeDeadlineOwner::PresentationTarget,
                at_ns: fired_at_ns,
            }),
            ..NativeWakePlan::default()
        };
        let mut metrics = NativeWakeAuthorityMetrics::default();
        let now_ns = fired_at_ns.saturating_add(1);
        metrics.observe_plan_after_wake(same_expired_plan, now_ns, &mut event_loop);
        assert_eq!(event_loop.fired_deadline_ns(), None);
        event_loop
            .arm_deadline(same_expired_plan.deadline.map(|deadline| deadline.at_ns))
            .unwrap();

        assert_eq!(metrics.stale_deadline_rearms, 1);

        let future_plan = NativeWakePlan {
            deadline: Some(NativeDeadline {
                owner: NativeDeadlineOwner::PresentationTarget,
                at_ns: now_ns.saturating_add(1_000_000),
            }),
            ..NativeWakePlan::default()
        };
        metrics.observe_plan_after_wake(future_plan, now_ns, &mut event_loop);
        assert_eq!(metrics.stale_deadline_rearms, 1);
    }

    #[test]
    fn worker_transport_has_exclusive_pageflip_timeout_authority() {
        let owner = NativePageflipTimeoutOwner::from(KmsCommitWorkerTransport::Worker);
        let scheduler_watchdog = Some(SchedulerWakeDeadline {
            kind: SchedulerWakeDeadlineKind::PageFlipWatchdog,
            at_ns: 2_000,
        });

        assert_eq!(
            scheduler_deadline_for_timeout_owner(scheduler_watchdog, owner,),
            None
        );
        assert_eq!(
            atomic_commit_watchdog_deadline_for_timeout_owner(Some(1_000), owner,),
            None
        );

        let plan =
            build_native_wake_plan(NativeWakePlanInputs {
                scheduler_deadline: scheduler_deadline_for_timeout_owner(scheduler_watchdog, owner),
                atomic_commit_watchdog_deadline_ns:
                    atomic_commit_watchdog_deadline_for_timeout_owner(Some(1_000), owner),
                explicit_sync_fallback_deadline_ns: Some(3_000),
                ..NativeWakePlanInputs::default()
            });
        assert_eq!(
            plan.deadline,
            Some(NativeDeadline {
                owner: NativeDeadlineOwner::ExplicitSyncFallback,
                at_ns: 3_000,
            })
        );
    }

    #[test]
    fn synchronous_transport_keeps_pageflip_watchdog_authority() {
        let owner = NativePageflipTimeoutOwner::from(KmsCommitWorkerTransport::Synchronous);
        let scheduler_watchdog = scheduler_deadline_for_timeout_owner(
            Some(SchedulerWakeDeadline {
                kind: SchedulerWakeDeadlineKind::PageFlipWatchdog,
                at_ns: 2_000,
            }),
            owner,
        );

        assert_eq!(
            scheduler_watchdog,
            Some(NativeDeadline {
                owner: NativeDeadlineOwner::FrameScheduler,
                at_ns: 2_000,
            })
        );
        assert_eq!(
            atomic_commit_watchdog_deadline_for_timeout_owner(Some(1_000), owner,),
            Some(1_000)
        );

        let plan = build_native_wake_plan(NativeWakePlanInputs {
            scheduler_deadline: scheduler_watchdog,
            atomic_commit_watchdog_deadline_ns: Some(1_000),
            ..NativeWakePlanInputs::default()
        });
        assert_eq!(
            plan.deadline,
            Some(NativeDeadline {
                owner: NativeDeadlineOwner::AtomicCommitWatchdog,
                at_ns: 1_000,
            })
        );
    }

    #[test]
    fn worker_transport_preserves_non_pageflip_scheduler_deadlines() {
        let owner = NativePageflipTimeoutOwner::from(KmsCommitWorkerTransport::Worker);
        assert_eq!(
            scheduler_deadline_for_timeout_owner(
                Some(SchedulerWakeDeadline {
                    kind: SchedulerWakeDeadlineKind::RenderStart,
                    at_ns: 2_000,
                }),
                owner,
            ),
            Some(NativeDeadline {
                owner: NativeDeadlineOwner::PresentationTarget,
                at_ns: 2_000,
            })
        );
    }
}
