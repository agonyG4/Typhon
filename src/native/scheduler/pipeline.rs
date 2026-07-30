use super::{NativeFrameScheduler, NativeOutputPacingMode, SchedulerDecision};
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
    fn pacing_mode(&self) -> NativeOutputPacingMode;
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
pub struct ExplicitAtomicSchedulerDecision {
    pub action: SchedulerDecision,
    pub wait_reason: Option<PipelineWaitReason>,
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
            if pipeline.pacing_mode() == NativeOutputPacingMode::PredictiveTriple
                && self.visual_work_queued
                && matches!(prepared, SchedulerPreparedPrimary::None)
                && context.render_ahead_allowed
                && pipeline.triple_capable()
                && context.presentation_target.is_some()
                && pipeline.future_primary_depth() < 2
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
        if pipeline.pacing_mode() == NativeOutputPacingMode::ReactiveDouble {
            if pipeline.kernel_primary_submitted() {
                return if self.visual_work_queued {
                    SchedulerDecision::WaitForBuffer
                } else {
                    SchedulerDecision::WaitForPageFlip
                };
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
                return SchedulerDecision::Render;
            }
        }

        if pipeline.kernel_primary_submitted() {
            if matches!(prepared, SchedulerPreparedPrimary::Ready { .. })
                && context.worker_queue_available
                && context.render_ahead_allowed
                && pipeline.triple_capable()
            {
                return ready_submit_decision(now_ns, ready_target);
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
            if !context.render_ahead_allowed
                || !pipeline.triple_capable()
                || context.presentation_target.is_none()
                || pipeline.future_primary_depth() >= 2
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
            SchedulerDecision::WaitForPageFlip if pipeline.future_primary_depth() >= 2 => {
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
        ExplicitAtomicSchedulerDecision {
            action,
            wait_reason,
        }
    }
}
