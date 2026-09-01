use super::{NativeFrameScheduler, SchedulerDecision};
use crate::native::presentation_deadline::{MonotonicTimestampNs, PresentationTarget};
use std::time::Duration;

#[derive(Debug, Clone, Copy)]
pub struct ExplicitAtomicSchedulerContext {
    pub now: MonotonicTimestampNs,
    pub predicted_total_cost: Duration,
    pub presentation_target: Option<PresentationTarget>,
    pub render_ahead_allowed: bool,
    pub worker_queue_available: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulerPreparedPrimary {
    None,
    Rendering,
    Ready { target: PresentationTarget },
}

pub trait PresentationPipelineView {
    fn future_primary_limit(&self) -> u8;
    fn kernel_commit_occupied(&self) -> bool;
    fn kernel_primary_submitted(&self) -> bool;
    fn worker_commit_occupied(&self) -> bool;
    fn worker_primary_queued(&self) -> bool;
    fn prepared_primary(&self) -> SchedulerPreparedPrimary;
    fn free_compositor_slots(&self) -> u8;
    fn future_primary_depth(&self) -> u8;
    fn direct_active(&self) -> bool;
    fn triple_capable(&self) -> bool;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineWaitReason {
    RefreshDeadline,
    NoFreeSlot,
    PreparedFrameExists,
    FuturePrimaryDepthFull,
    WorkerQueueOccupied,
    KernelCommitPending,
    RenderFence,
    DirectSteadyState,
    CompatibilityPath,
    TripleCapabilityUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulerWakeDeadlineKind {
    RenderStart,
    SubmitNotBefore,
    ProtocolRefresh,
    PageFlipWatchdog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchedulerWakeDeadline {
    pub kind: SchedulerWakeDeadlineKind,
    pub at_ns: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExplicitAtomicSchedulerDecision {
    pub action: SchedulerDecision,
    pub wait_reason: Option<PipelineWaitReason>,
    pub wake_deadline: Option<SchedulerWakeDeadline>,
}

fn ready_submit_decision(now_ns: u64, target: Option<PresentationTarget>) -> SchedulerDecision {
    if target.is_some_and(|target| now_ns < target.submit_not_before().get()) {
        SchedulerDecision::WaitForRefresh
    } else if target.is_some_and(|target| now_ns >= target.presentation_time.get()) {
        SchedulerDecision::SubmitReadyLate
    } else {
        SchedulerDecision::SubmitReady
    }
}

fn ready_worker_admission_decision(
    now_ns: u64,
    target: Option<PresentationTarget>,
) -> SchedulerDecision {
    if target.is_some_and(|target| now_ns >= target.presentation_time.get()) {
        SchedulerDecision::SubmitReadyLate
    } else {
        SchedulerDecision::SubmitReady
    }
}

pub fn apply_atomic_commit_lane_guard(
    decision: SchedulerDecision,
    atomic_commit_pending: bool,
    can_queue_worker_next: bool,
) -> SchedulerDecision {
    if atomic_commit_pending
        && !can_queue_worker_next
        && matches!(
            decision,
            SchedulerDecision::SubmitReady | SchedulerDecision::SubmitReadyLate
        )
    {
        SchedulerDecision::WaitForPageFlip
    } else {
        decision
    }
}

pub const fn rendered_primary_must_wait_for_lane(
    render_ahead: bool,
    atomic_commit_pending: bool,
    can_queue_worker_next: bool,
) -> bool {
    render_ahead || (atomic_commit_pending && !can_queue_worker_next)
}

impl NativeFrameScheduler {
    pub fn decision_with_pipeline(
        &mut self,
        context: ExplicitAtomicSchedulerContext,
        pipeline: &impl PresentationPipelineView,
    ) -> SchedulerDecision {
        let now_ns = context.now.get();
        let _predicted_total_cost = context.predicted_total_cost;
        let prepared = pipeline.prepared_primary();
        let ready_target = match prepared {
            SchedulerPreparedPrimary::Ready { target } => Some(target),
            SchedulerPreparedPrimary::None | SchedulerPreparedPrimary::Rendering => None,
        };

        if pipeline.worker_primary_queued() {
            if self.visual_work_queued
                && matches!(prepared, SchedulerPreparedPrimary::None)
                && context.render_ahead_allowed
                && pipeline.triple_capable()
                && context.presentation_target.is_some()
                && pipeline.future_primary_depth() < pipeline.future_primary_limit()
                && pipeline.free_compositor_slots() > 0
                && !pipeline.direct_active()
            {
                if context
                    .presentation_target
                    .is_some_and(|target| now_ns < target.render_start_deadline.get())
                {
                    return SchedulerDecision::WaitForRefresh;
                }
                return SchedulerDecision::RenderAhead;
            }
            return SchedulerDecision::WaitForWorkerQueue;
        }
        if pipeline.kernel_primary_submitted() {
            if !self.visual_work_queued && self.page_flip_watchdog_expired(now_ns) {
                return SchedulerDecision::PageFlipWatchdogExpired;
            }
            if matches!(prepared, SchedulerPreparedPrimary::Ready { .. })
                && context.worker_queue_available
                && context.render_ahead_allowed
                && pipeline.triple_capable()
            {
                return ready_worker_admission_decision(now_ns, ready_target);
            }
            if !matches!(prepared, SchedulerPreparedPrimary::None) {
                return SchedulerDecision::WaitForPageFlip;
            }
            if !self.visual_work_queued {
                return SchedulerDecision::WaitForPageFlip;
            }
            if pipeline.free_compositor_slots() == 0 {
                return SchedulerDecision::WaitForBuffer;
            }
            if !context.render_ahead_allowed {
                return SchedulerDecision::WaitForBuffer;
            }
            if !pipeline.triple_capable()
                || context.presentation_target.is_none()
                || pipeline.future_primary_depth() >= pipeline.future_primary_limit()
                || pipeline.direct_active()
            {
                return SchedulerDecision::WaitForPageFlip;
            }
            if context
                .presentation_target
                .is_some_and(|target| now_ns < target.render_start_deadline.get())
            {
                return SchedulerDecision::WaitForRefresh;
            }
            return SchedulerDecision::RenderAhead;
        }

        if matches!(prepared, SchedulerPreparedPrimary::Ready { .. }) {
            if pipeline.kernel_commit_occupied() {
                return SchedulerDecision::WaitForPageFlip;
            }
            return ready_submit_decision(now_ns, ready_target);
        }
        if matches!(prepared, SchedulerPreparedPrimary::Rendering) {
            return SchedulerDecision::WaitForBuffer;
        }
        if self.visual_work_queued {
            if context
                .presentation_target
                .is_some_and(|target| now_ns < target.render_start_deadline.get())
            {
                return SchedulerDecision::WaitForRefresh;
            }
            return SchedulerDecision::Render;
        }
        if self.protocol_work_queued {
            let deadline = match self.refresh_deadline_ns {
                Some(deadline) => deadline,
                None => {
                    let deadline = self.first_boundary_after(now_ns);
                    self.refresh_deadline_ns = Some(deadline);
                    deadline
                }
            };
            if now_ns >= deadline {
                SchedulerDecision::CompleteProtocolOnly
            } else {
                SchedulerDecision::WaitForRefresh
            }
        } else if pipeline.worker_commit_occupied() {
            SchedulerDecision::WaitForWorkerQueue
        } else {
            SchedulerDecision::Idle
        }
    }

    fn page_flip_watchdog_expired(&mut self, now_ns: u64) -> bool {
        if !self
            .watchdog_deadline_ns
            .is_some_and(|deadline| now_ns >= deadline)
        {
            return false;
        }
        if !self.watchdog_reported {
            self.watchdog_timeout_count = self.watchdog_timeout_count.saturating_add(1);
            self.watchdog_reported = true;
        }
        true
    }

    pub fn decision_with_pipeline_diagnostics(
        &mut self,
        context: ExplicitAtomicSchedulerContext,
        pipeline: &impl PresentationPipelineView,
    ) -> ExplicitAtomicSchedulerDecision {
        let action = self.decision_with_pipeline(context, pipeline);
        let wait_reason = match action {
            SchedulerDecision::WaitForRefresh => Some(PipelineWaitReason::RefreshDeadline),
            SchedulerDecision::WaitForWorkerQueue => Some(PipelineWaitReason::WorkerQueueOccupied),
            SchedulerDecision::WaitForBuffer if pipeline.kernel_primary_submitted() => {
                Some(PipelineWaitReason::KernelCommitPending)
            }
            SchedulerDecision::WaitForBuffer
                if !matches!(pipeline.prepared_primary(), SchedulerPreparedPrimary::None) =>
            {
                Some(PipelineWaitReason::PreparedFrameExists)
            }
            SchedulerDecision::WaitForBuffer => Some(PipelineWaitReason::NoFreeSlot),
            SchedulerDecision::WaitForPageFlip
                if pipeline.future_primary_depth() >= pipeline.future_primary_limit() =>
            {
                Some(PipelineWaitReason::FuturePrimaryDepthFull)
            }
            SchedulerDecision::WaitForPageFlip if pipeline.direct_active() => {
                Some(PipelineWaitReason::DirectSteadyState)
            }
            SchedulerDecision::WaitForPageFlip if !pipeline.triple_capable() => {
                Some(PipelineWaitReason::TripleCapabilityUnavailable)
            }
            SchedulerDecision::WaitForPageFlip
                if !matches!(pipeline.prepared_primary(), SchedulerPreparedPrimary::None) =>
            {
                Some(PipelineWaitReason::PreparedFrameExists)
            }
            SchedulerDecision::WaitForPageFlip => Some(PipelineWaitReason::KernelCommitPending),
            SchedulerDecision::Idle
            | SchedulerDecision::Render
            | SchedulerDecision::RenderAhead
            | SchedulerDecision::SubmitReady
            | SchedulerDecision::SubmitReadyLate
            | SchedulerDecision::ReadyTargetInvalidated
            | SchedulerDecision::CompleteProtocolOnly
            | SchedulerDecision::PageFlipWatchdogExpired => None,
        };
        let wake_deadline = match action {
            SchedulerDecision::WaitForRefresh => {
                if let Some(target) = ready_target_for_pipeline(pipeline) {
                    Some(SchedulerWakeDeadline {
                        kind: SchedulerWakeDeadlineKind::SubmitNotBefore,
                        at_ns: target.submit_not_before().get(),
                    })
                } else if self.visual_work_queued {
                    context
                        .presentation_target
                        .map(|target| SchedulerWakeDeadline {
                            kind: SchedulerWakeDeadlineKind::RenderStart,
                            at_ns: target.render_start_deadline.get(),
                        })
                } else if self.protocol_work_queued {
                    self.refresh_deadline_ns.map(|at_ns| SchedulerWakeDeadline {
                        kind: SchedulerWakeDeadlineKind::ProtocolRefresh,
                        at_ns,
                    })
                } else {
                    None
                }
            }
            SchedulerDecision::WaitForBuffer | SchedulerDecision::WaitForPageFlip => self
                .pending_page_flip_token
                .and(self.watchdog_deadline_ns)
                .map(|at_ns| SchedulerWakeDeadline {
                    kind: SchedulerWakeDeadlineKind::PageFlipWatchdog,
                    at_ns,
                }),
            SchedulerDecision::Idle
            | SchedulerDecision::Render
            | SchedulerDecision::RenderAhead
            | SchedulerDecision::SubmitReady
            | SchedulerDecision::SubmitReadyLate
            | SchedulerDecision::ReadyTargetInvalidated
            | SchedulerDecision::CompleteProtocolOnly
            | SchedulerDecision::PageFlipWatchdogExpired => None,
            SchedulerDecision::WaitForWorkerQueue => None,
        };
        ExplicitAtomicSchedulerDecision {
            action,
            wait_reason,
            wake_deadline,
        }
    }
}

fn ready_target_for_pipeline(
    pipeline: &impl PresentationPipelineView,
) -> Option<PresentationTarget> {
    match pipeline.prepared_primary() {
        SchedulerPreparedPrimary::Ready { target } => Some(target),
        SchedulerPreparedPrimary::None | SchedulerPreparedPrimary::Rendering => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::presentation_deadline::PresentationTargetReason;

    #[derive(Debug, Clone, Copy)]
    struct TestPipeline {
        future_primary_limit: u8,
        kernel_commit_occupied: bool,
        kernel_primary_submitted: bool,
        worker_commit_occupied: bool,
        worker_primary_queued: bool,
        prepared: SchedulerPreparedPrimary,
        free_compositor_slots: u8,
        future_primary_depth: u8,
        direct_active: bool,
        triple_capable: bool,
    }

    impl Default for TestPipeline {
        fn default() -> Self {
            Self {
                future_primary_limit: 2,
                kernel_commit_occupied: false,
                kernel_primary_submitted: false,
                worker_commit_occupied: false,
                worker_primary_queued: false,
                prepared: SchedulerPreparedPrimary::None,
                free_compositor_slots: 1,
                future_primary_depth: 0,
                direct_active: false,
                triple_capable: true,
            }
        }
    }

    impl PresentationPipelineView for TestPipeline {
        fn future_primary_limit(&self) -> u8 {
            self.future_primary_limit
        }

        fn kernel_commit_occupied(&self) -> bool {
            self.kernel_commit_occupied
        }

        fn kernel_primary_submitted(&self) -> bool {
            self.kernel_primary_submitted
        }

        fn worker_commit_occupied(&self) -> bool {
            self.worker_commit_occupied
        }

        fn worker_primary_queued(&self) -> bool {
            self.worker_primary_queued
        }

        fn prepared_primary(&self) -> SchedulerPreparedPrimary {
            self.prepared
        }

        fn free_compositor_slots(&self) -> u8 {
            self.free_compositor_slots
        }

        fn future_primary_depth(&self) -> u8 {
            self.future_primary_depth
        }

        fn direct_active(&self) -> bool {
            self.direct_active
        }

        fn triple_capable(&self) -> bool {
            self.triple_capable
        }
    }

    fn target(render_start_deadline: u64, submit_not_before: u64) -> PresentationTarget {
        PresentationTarget {
            sequence: 1,
            presentation_time: MonotonicTimestampNs::new(submit_not_before + 1),
            submit_not_before: MonotonicTimestampNs::new(submit_not_before),
            render_start_deadline: MonotonicTimestampNs::new(render_start_deadline),
            refresh_interval: Duration::from_millis(6),
            reason: PresentationTargetReason::PredictedPressure,
            clock_generation: 1,
            estimated: false,
            predicted_unreachable: false,
        }
    }

    fn context(
        now_ns: u64,
        presentation_target: Option<PresentationTarget>,
    ) -> ExplicitAtomicSchedulerContext {
        ExplicitAtomicSchedulerContext {
            now: MonotonicTimestampNs::new(now_ns),
            predicted_total_cost: Duration::from_millis(2),
            presentation_target,
            render_ahead_allowed: true,
            worker_queue_available: true,
        }
    }

    #[test]
    fn expired_visual_target_cannot_poll_a_worker_blocked_pipeline() {
        let mut scheduler = NativeFrameScheduler::new(165, 0);
        scheduler.queue_visual_work();
        let pipeline = TestPipeline {
            worker_primary_queued: true,
            ..TestPipeline::default()
        };

        let mut scheduler_context = context(5_000_000, Some(target(4_000_000, 6_000_000)));
        scheduler_context.render_ahead_allowed = false;
        let decision = scheduler.decision_with_pipeline_diagnostics(scheduler_context, &pipeline);

        assert_eq!(decision.action, SchedulerDecision::WaitForWorkerQueue);
        assert_eq!(
            decision.wait_reason,
            Some(PipelineWaitReason::WorkerQueueOccupied)
        );
        assert_eq!(decision.wake_deadline, None);
    }

    #[test]
    fn pageflip_blocked_pipeline_ignores_obsolete_visual_deadline() {
        let mut scheduler = NativeFrameScheduler::new(165, 0);
        scheduler.note_async_submission(41, 1).unwrap();
        let pipeline = TestPipeline {
            kernel_primary_submitted: true,
            ..TestPipeline::default()
        };

        let decision = scheduler.decision_with_pipeline_diagnostics(
            context(5_000_000, Some(target(4_000_000, 6_000_000))),
            &pipeline,
        );

        assert_eq!(decision.action, SchedulerDecision::WaitForPageFlip);
        assert_eq!(
            decision.wake_deadline,
            Some(SchedulerWakeDeadline {
                kind: SchedulerWakeDeadlineKind::PageFlipWatchdog,
                at_ns: 1_000_000_001,
            })
        );
    }

    #[test]
    fn expired_pageflip_watchdog_is_terminal_before_rearming() {
        let mut scheduler = NativeFrameScheduler::with_watchdog(165, 0, 100);
        scheduler.note_async_submission(41, 1).unwrap();
        let pipeline = TestPipeline {
            kernel_primary_submitted: true,
            ..TestPipeline::default()
        };

        let decision = scheduler.decision_with_pipeline_diagnostics(context(101, None), &pipeline);

        assert_eq!(decision.action, SchedulerDecision::PageFlipWatchdogExpired);
        assert_eq!(decision.wake_deadline, None);
        assert_eq!(scheduler.watchdog_timeout_count(), 1);
    }

    #[test]
    fn wait_for_refresh_keeps_exact_render_start_deadline() {
        let mut scheduler = NativeFrameScheduler::new(165, 0);
        scheduler.queue_visual_work();
        let pipeline = TestPipeline::default();

        let decision = scheduler.decision_with_pipeline_diagnostics(
            context(4_000_000, Some(target(5_000_000, 6_000_000))),
            &pipeline,
        );

        assert_eq!(decision.action, SchedulerDecision::WaitForRefresh);
        assert_eq!(
            decision.wake_deadline,
            Some(SchedulerWakeDeadline {
                kind: SchedulerWakeDeadlineKind::RenderStart,
                at_ns: 5_000_000,
            })
        );
    }

    #[test]
    fn ready_frame_uses_submit_boundary_instead_of_render_start() {
        let mut scheduler = NativeFrameScheduler::new(165, 0);
        let ready_target = target(3_000_000, 5_000_000);
        scheduler.note_ready_frame(Some(ready_target));
        let pipeline = TestPipeline {
            prepared: SchedulerPreparedPrimary::Ready {
                target: ready_target,
            },
            ..TestPipeline::default()
        };

        let decision = scheduler
            .decision_with_pipeline_diagnostics(context(4_000_000, Some(ready_target)), &pipeline);

        assert_eq!(decision.action, SchedulerDecision::WaitForRefresh);
        assert_eq!(
            decision.wake_deadline,
            Some(SchedulerWakeDeadline {
                kind: SchedulerWakeDeadlineKind::SubmitNotBefore,
                at_ns: 5_000_000,
            })
        );
    }

    #[test]
    fn actionable_scheduler_decisions_have_no_rediscovery_deadline() {
        let mut render = NativeFrameScheduler::new(165, 0);
        render.queue_visual_work();
        assert_eq!(
            render
                .decision_with_pipeline_diagnostics(
                    context(5_000_000, Some(target(4_000_000, 6_000_000))),
                    &TestPipeline::default(),
                )
                .wake_deadline,
            None
        );

        let mut render_ahead = NativeFrameScheduler::new(165, 0);
        render_ahead.note_async_submission(41, 1).unwrap();
        render_ahead.queue_visual_work();
        let worker_pipeline = TestPipeline {
            worker_primary_queued: true,
            ..TestPipeline::default()
        };
        assert_eq!(
            render_ahead
                .decision_with_pipeline_diagnostics(
                    context(5_000_000, Some(target(4_000_000, 6_000_000))),
                    &worker_pipeline,
                )
                .wake_deadline,
            None
        );

        for now_ns in [5_000_000, 5_000_002] {
            let mut ready = NativeFrameScheduler::new(165, 0);
            let ready_target = target(3_000_000, 5_000_000);
            ready.note_ready_frame(Some(ready_target));
            let decision = ready.decision_with_pipeline_diagnostics(
                context(now_ns, Some(ready_target)),
                &TestPipeline {
                    prepared: SchedulerPreparedPrimary::Ready {
                        target: ready_target,
                    },
                    ..TestPipeline::default()
                },
            );
            assert_eq!(decision.wake_deadline, None);
        }

        let mut protocol = NativeFrameScheduler::new(165, 0);
        protocol.queue_protocol_work(0);
        assert_eq!(
            protocol
                .decision_with_pipeline_diagnostics(
                    context(6_060_606, None),
                    &TestPipeline::default(),
                )
                .wake_deadline,
            None
        );
    }
}
