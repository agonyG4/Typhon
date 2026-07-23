//! Absolute-deadline frame scheduling for the native compositor runtime.

use crate::native::kms::KmsBackendKind;
use crate::native::presentation_deadline::{MonotonicTimestampNs, PresentationTarget};
use std::time::Duration;

const DEFAULT_PAGE_FLIP_WATCHDOG_NS: u64 = 1_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulerDecision {
    Idle,
    Render,
    RenderAhead,
    SubmitReady,
    SubmitReadyLate,
    ReadyTargetInvalidated,
    CompleteProtocolOnly,
    WaitForRefresh,
    WaitForBuffer,
    WaitForPageFlip,
    PageFlipWatchdogExpired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulerState {
    Idle,
    VisualWorkQueued,
    ProtocolWorkQueued,
    RefreshDeadlineArmed,
    PageFlipPending,
    ReadyFrameQueued,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeOutputPacingMode {
    ReactiveDouble,
    PredictiveTriple,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchedulerCapabilities {
    kms_backend: KmsBackendKind,
    primary_plane_in_fence: bool,
    explicit_output_swapchain: bool,
}

#[doc(hidden)]
#[derive(Debug, Clone, Copy)]
pub struct SchedulerFrameContext {
    pub pacing_mode: NativeOutputPacingMode,
    pub capabilities: SchedulerCapabilities,
    pub presentation_target: Option<PresentationTarget>,
    pub predicted_total_cost: Duration,
    pub now: MonotonicTimestampNs,
    pub render_target_available: bool,
    pub render_ahead_allowed: bool,
    pub ready_frame_present: bool,
    pub ready_target_current: bool,
}

impl SchedulerCapabilities {
    pub const fn for_backend(kms_backend: KmsBackendKind) -> Self {
        Self {
            kms_backend,
            primary_plane_in_fence: false,
            explicit_output_swapchain: false,
        }
    }

    pub const fn explicit_atomic(
        primary_plane_in_fence: bool,
        explicit_output_swapchain: bool,
    ) -> Self {
        Self::for_backend(KmsBackendKind::Atomic)
            .with_primary_plane_in_fence(primary_plane_in_fence)
            .with_explicit_output_swapchain(explicit_output_swapchain)
    }

    pub const fn legacy() -> Self {
        Self::for_backend(KmsBackendKind::Legacy)
    }

    pub const fn with_primary_plane_in_fence(mut self, available: bool) -> Self {
        self.primary_plane_in_fence = available;
        self
    }

    pub const fn with_explicit_output_swapchain(mut self, available: bool) -> Self {
        self.explicit_output_swapchain = available;
        self
    }

    pub const fn render_ahead_allowed(self) -> bool {
        matches!(self.kms_backend, KmsBackendKind::Atomic)
            && self.primary_plane_in_fence
            && self.explicit_output_swapchain
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageFlipCompletionResult {
    Completed { submitted_at_ns: u64 },
    Mismatched { expected: u64 },
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeFrameScheduler {
    refresh_interval_ns: u64,
    anchor_ns: u64,
    visual_work_queued: bool,
    protocol_work_queued: bool,
    pending_page_flip_token: Option<u64>,
    pending_page_flip_submitted_at_ns: Option<u64>,
    ready_frame_queued: bool,
    ready_target: Option<PresentationTarget>,
    refresh_deadline_ns: Option<u64>,
    watchdog_deadline_ns: Option<u64>,
    watchdog_interval_ns: u64,
    watchdog_timeout_count: u64,
    watchdog_reported: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PresentationCadenceMetrics {
    presentations: u64,
    sequence_gaps: u64,
    last_sequence: Option<u32>,
    last_timestamp_us: Option<u64>,
    last_interval_us: Option<u64>,
    last_sequence_delta: Option<u32>,
    logical_sequence: u64,
    last_logical_sequence_delta: Option<u64>,
    timestamp_fallback_active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentationCadenceSample {
    pub presentations: u64,
    pub interval_us: Option<u64>,
    pub sequence_delta: Option<u32>,
    pub sequence_gap: bool,
    pub logical_sequence: u64,
    pub logical_sequence_delta: Option<u64>,
    pub timestamp_sequence_fallback: bool,
    pub estimated_hz_millihz: Option<u64>,
    pub sequence_gaps: u64,
}

impl PresentationCadenceMetrics {
    pub fn record(&mut self, sequence: u32, timestamp_us: u64) -> PresentationCadenceSample {
        self.record_with_refresh(sequence, timestamp_us, 0)
    }

    pub fn record_with_refresh(
        &mut self,
        sequence: u32,
        timestamp_us: u64,
        refresh_interval_us: u64,
    ) -> PresentationCadenceSample {
        self.presentations = self.presentations.saturating_add(1);
        let interval_us = self
            .last_timestamp_us
            .map(|last| timestamp_us.saturating_sub(last));
        let sequence_delta = self.last_sequence.map(|last| sequence.wrapping_sub(last));
        let logical_sequence_delta = interval_us.map(|interval| {
            if refresh_interval_us == 0 {
                1
            } else {
                interval
                    .saturating_add(refresh_interval_us / 2)
                    .checked_div(refresh_interval_us)
                    .unwrap_or(1)
                    .max(1)
            }
        });
        if let Some(delta) = logical_sequence_delta {
            self.logical_sequence = self.logical_sequence.saturating_add(delta);
        } else {
            self.logical_sequence = 1;
        }
        let sequence_gap = logical_sequence_delta.is_some_and(|delta| delta > 1);
        if sequence_gap {
            self.sequence_gaps = self.sequence_gaps.saturating_add(1);
        }
        self.last_sequence = Some(sequence);
        self.last_timestamp_us = Some(timestamp_us);
        self.last_interval_us = interval_us;
        self.last_sequence_delta = sequence_delta;
        self.last_logical_sequence_delta = logical_sequence_delta;
        self.timestamp_fallback_active = sequence == 0;
        PresentationCadenceSample {
            presentations: self.presentations,
            interval_us,
            sequence_delta,
            sequence_gap,
            logical_sequence: self.logical_sequence,
            logical_sequence_delta,
            timestamp_sequence_fallback: self.timestamp_fallback_active,
            estimated_hz_millihz: interval_us
                .filter(|interval| *interval > 0)
                .map(|interval| 1_000_000_000 / interval),
            sequence_gaps: self.sequence_gaps,
        }
    }

    pub const fn presentations(&self) -> u64 {
        self.presentations
    }

    pub const fn sequence_gaps(&self) -> u64 {
        self.sequence_gaps
    }

    pub const fn logical_sequence(&self) -> u64 {
        self.logical_sequence
    }

    pub const fn timestamp_fallback_active(&self) -> bool {
        self.timestamp_fallback_active
    }
}

impl NativeFrameScheduler {
    pub fn new(refresh_hz: u32, anchor_ns: u64) -> Self {
        Self::with_watchdog(refresh_hz, anchor_ns, DEFAULT_PAGE_FLIP_WATCHDOG_NS)
    }

    fn with_watchdog(refresh_hz: u32, anchor_ns: u64, watchdog_interval_ns: u64) -> Self {
        let refresh_hz = if refresh_hz == 0 {
            60
        } else {
            refresh_hz.clamp(30, 360)
        };
        Self {
            refresh_interval_ns: 1_000_000_000 / u64::from(refresh_hz),
            anchor_ns,
            visual_work_queued: false,
            protocol_work_queued: false,
            pending_page_flip_token: None,
            pending_page_flip_submitted_at_ns: None,
            ready_frame_queued: false,
            ready_target: None,
            refresh_deadline_ns: None,
            watchdog_deadline_ns: None,
            watchdog_interval_ns: watchdog_interval_ns.max(1),
            watchdog_timeout_count: 0,
            watchdog_reported: false,
        }
    }

    pub fn refresh_interval_ns(&self) -> u64 {
        self.refresh_interval_ns
    }

    /// Return the next refresh boundary used by the output scheduler.  Cursor
    /// arbitration uses this clock rather than an input-relative delay, so a
    /// cursor request cannot win a race merely because it arrived first.
    pub fn next_refresh_deadline_ns(&self, now_ns: u64) -> u64 {
        self.first_boundary_after(now_ns)
    }

    pub fn queue_visual_work(&mut self) {
        self.visual_work_queued = true;
        self.protocol_work_queued = false;
        self.refresh_deadline_ns = None;
    }

    pub fn queue_protocol_work(&mut self, now_ns: u64) {
        if self.visual_work_queued {
            return;
        }
        self.protocol_work_queued = true;
        if self.pending_page_flip_token.is_none() && self.refresh_deadline_ns.is_none() {
            self.refresh_deadline_ns = Some(self.first_boundary_after(now_ns));
        }
    }

    pub fn decision(&mut self, now_ns: u64) -> SchedulerDecision {
        self.decision_with_render_target(now_ns, true)
    }

    pub fn decision_with_render_target(
        &mut self,
        now_ns: u64,
        render_target_available: bool,
    ) -> SchedulerDecision {
        self.decision_with_context(SchedulerFrameContext {
            pacing_mode: NativeOutputPacingMode::PredictiveTriple,
            capabilities: SchedulerCapabilities::legacy(),
            presentation_target: None,
            predicted_total_cost: Duration::ZERO,
            now: MonotonicTimestampNs::new(now_ns),
            render_target_available,
            render_ahead_allowed: false,
            ready_frame_present: self.ready_frame_queued,
            ready_target_current: true,
        })
    }

    pub fn decision_with_context(&mut self, context: SchedulerFrameContext) -> SchedulerDecision {
        let now_ns = context.now.get();
        let _predicted_total_cost = context.predicted_total_cost;
        if context.pacing_mode == NativeOutputPacingMode::ReactiveDouble {
            if self.pending_page_flip_token.is_some() {
                if self.visual_work_queued {
                    return SchedulerDecision::WaitForBuffer;
                }
                if self
                    .watchdog_deadline_ns
                    .is_some_and(|deadline| now_ns >= deadline)
                {
                    if !self.watchdog_reported {
                        self.watchdog_timeout_count = self.watchdog_timeout_count.saturating_add(1);
                        self.watchdog_reported = true;
                    }
                    return SchedulerDecision::PageFlipWatchdogExpired;
                }
                return SchedulerDecision::WaitForPageFlip;
            }
            if self.ready_frame_queued || context.ready_frame_present {
                return SchedulerDecision::ReadyTargetInvalidated;
            }
            if self.visual_work_queued {
                return SchedulerDecision::Render;
            }
        }
        if self.pending_page_flip_token.is_some() {
            if self.visual_work_queued {
                if context.ready_frame_present || self.ready_frame_queued {
                    return SchedulerDecision::WaitForPageFlip;
                }
                if !context.render_target_available {
                    return SchedulerDecision::WaitForBuffer;
                }
                if !context.render_ahead_allowed
                    || !context.capabilities.render_ahead_allowed()
                    || context.presentation_target.is_none()
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
            if self
                .watchdog_deadline_ns
                .is_some_and(|deadline| now_ns >= deadline)
            {
                if !self.watchdog_reported {
                    self.watchdog_timeout_count = self.watchdog_timeout_count.saturating_add(1);
                    self.watchdog_reported = true;
                }
                return SchedulerDecision::PageFlipWatchdogExpired;
            }
            return SchedulerDecision::WaitForPageFlip;
        }
        if self.ready_frame_queued {
            if self.ready_target.is_some() && !context.ready_target_current {
                return SchedulerDecision::ReadyTargetInvalidated;
            }
            if self
                .ready_target
                .is_some_and(|target| now_ns < target.submit_not_before().get())
            {
                return SchedulerDecision::WaitForRefresh;
            }
            if self
                .ready_target
                .is_some_and(|target| now_ns >= target.presentation_time.get())
            {
                // The userspace boundary was missed. Submit the same frame and target
                // identity now; the pageflip remains responsible for recording lateness.
                return SchedulerDecision::SubmitReadyLate;
            }
            return SchedulerDecision::SubmitReady;
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
        } else {
            SchedulerDecision::Idle
        }
    }

    pub fn note_async_submission(&mut self, token: u64, now_ns: u64) -> Result<(), &'static str> {
        if token == 0 {
            return Err("page flip token must be nonzero");
        }
        if self.pending_page_flip_token.is_some() {
            return Err("page flip already pending");
        }
        self.visual_work_queued = false;
        self.protocol_work_queued = false;
        self.refresh_deadline_ns = None;
        self.pending_page_flip_token = Some(token);
        self.pending_page_flip_submitted_at_ns = Some(now_ns);
        self.watchdog_deadline_ns = Some(now_ns.saturating_add(self.watchdog_interval_ns));
        self.watchdog_reported = false;
        Ok(())
    }

    pub fn note_render_ahead_ready(&mut self) {
        self.note_ready_frame(None);
    }

    pub fn note_ready_frame(&mut self, target: Option<PresentationTarget>) {
        self.visual_work_queued = false;
        self.protocol_work_queued = false;
        self.refresh_deadline_ns = None;
        self.ready_frame_queued = true;
        self.ready_target = target;
    }

    pub fn note_ready_submission(&mut self, token: u64, now_ns: u64) -> Result<(), &'static str> {
        self.note_async_submission(token, now_ns)?;
        self.ready_frame_queued = false;
        self.ready_target = None;
        Ok(())
    }

    pub fn discard_ready_frame(&mut self) {
        self.ready_frame_queued = false;
        self.ready_target = None;
    }

    pub fn note_page_flip_completion(
        &mut self,
        token: u64,
        observed_ns: u64,
    ) -> PageFlipCompletionResult {
        let Some(expected) = self.pending_page_flip_token else {
            return PageFlipCompletionResult::Stale;
        };
        if token != expected {
            return PageFlipCompletionResult::Mismatched { expected };
        }
        self.pending_page_flip_token = None;
        let submitted_at_ns = self
            .pending_page_flip_submitted_at_ns
            .take()
            .unwrap_or(observed_ns);
        self.watchdog_deadline_ns = None;
        self.watchdog_reported = false;
        self.anchor_ns = observed_ns;
        if self.protocol_work_queued && !self.visual_work_queued {
            self.refresh_deadline_ns = Some(self.first_boundary_after(observed_ns));
        }
        PageFlipCompletionResult::Completed { submitted_at_ns }
    }

    pub fn complete_protocol_only(&mut self) {
        self.protocol_work_queued = false;
        self.refresh_deadline_ns = None;
    }

    pub fn note_immediate_completion(&mut self) {
        self.visual_work_queued = false;
        self.protocol_work_queued = false;
        self.ready_frame_queued = false;
        self.ready_target = None;
        self.refresh_deadline_ns = None;
    }

    /// A libseat disable revokes KMS access. Pending presentation is deliberately
    /// abandoned and rebuilt after output recovery rather than drained on a revoked fd.
    pub fn abandon_for_session_suspend(&mut self) {
        self.pending_page_flip_token = None;
        self.pending_page_flip_submitted_at_ns = None;
        self.ready_frame_queued = false;
        self.ready_target = None;
        self.refresh_deadline_ns = None;
        self.watchdog_deadline_ns = None;
        self.watchdog_reported = false;
    }

    pub fn next_deadline_ns(&self) -> Option<u64> {
        if self.pending_page_flip_token.is_some() {
            self.watchdog_deadline_ns
        } else if self.ready_frame_queued {
            Some(
                self.ready_target
                    .map_or(self.anchor_ns, |target| target.submit_not_before().get()),
            )
        } else {
            self.refresh_deadline_ns
        }
    }

    pub fn visual_work_queued(&self) -> bool {
        self.visual_work_queued
    }

    pub fn protocol_work_queued(&self) -> bool {
        self.protocol_work_queued
    }

    pub fn page_flip_pending(&self) -> bool {
        self.pending_page_flip_token.is_some()
    }

    pub fn ready_frame_queued(&self) -> bool {
        self.ready_frame_queued
    }

    pub fn ready_target(&self) -> Option<PresentationTarget> {
        self.ready_target
    }

    pub fn pending_page_flip_token(&self) -> Option<u64> {
        self.pending_page_flip_token
    }

    pub fn watchdog_timeout_count(&self) -> u64 {
        self.watchdog_timeout_count
    }

    /// Explicit Atomic output has a total commit arbiter that owns the kernel
    /// watchdog. Keep the scheduler's pacing state pending without creating a
    /// second timeout owner for the same commit.
    pub fn defer_page_flip_watchdog_to_atomic_arbiter(&mut self) {
        self.watchdog_deadline_ns = None;
        self.watchdog_reported = false;
    }

    pub fn state(&self) -> SchedulerState {
        if self.pending_page_flip_token.is_some() {
            SchedulerState::PageFlipPending
        } else if self.ready_frame_queued {
            SchedulerState::ReadyFrameQueued
        } else if self.visual_work_queued {
            SchedulerState::VisualWorkQueued
        } else if self.refresh_deadline_ns.is_some() {
            SchedulerState::RefreshDeadlineArmed
        } else if self.protocol_work_queued {
            SchedulerState::ProtocolWorkQueued
        } else {
            SchedulerState::Idle
        }
    }

    fn first_boundary_after(&self, now_ns: u64) -> u64 {
        let elapsed = now_ns.saturating_sub(self.anchor_ns);
        let intervals = elapsed / self.refresh_interval_ns + 1;
        self.anchor_ns
            .saturating_add(intervals.saturating_mul(self.refresh_interval_ns))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::presentation_deadline::PresentationTargetReason;

    fn render_ahead_context(
        now_ns: u64,
        render_target_available: bool,
        ready_frame_present: bool,
    ) -> SchedulerFrameContext {
        SchedulerFrameContext {
            pacing_mode: NativeOutputPacingMode::PredictiveTriple,
            capabilities: SchedulerCapabilities::explicit_atomic(true, true),
            presentation_target: Some(PresentationTarget {
                sequence: 2,
                presentation_time: MonotonicTimestampNs::new(now_ns.saturating_add(1_000_000)),
                submit_not_before: MonotonicTimestampNs::new(now_ns),
                render_start_deadline: MonotonicTimestampNs::new(now_ns),
                refresh_interval: Duration::from_millis(10),
                reason: PresentationTargetReason::PredictedPressure,
                clock_generation: 1,
                estimated: false,
                predicted_unreachable: false,
            }),
            predicted_total_cost: Duration::from_millis(10),
            now: MonotonicTimestampNs::new(now_ns),
            render_target_available,
            render_ahead_allowed: true,
            ready_frame_present,
            ready_target_current: true,
        }
    }

    fn reactive_context(now_ns: u64) -> SchedulerFrameContext {
        let mut context = render_ahead_context(now_ns, true, false);
        context.pacing_mode = NativeOutputPacingMode::ReactiveDouble;
        context.presentation_target = context.presentation_target.map(|mut target| {
            target.render_start_deadline = MonotonicTimestampNs::new(now_ns + 100_000_000);
            target.submit_not_before = MonotonicTimestampNs::new(now_ns + 100_000_000);
            target
        });
        context.predicted_total_cost = Duration::from_millis(100);
        context.render_ahead_allowed = false;
        context
    }

    #[derive(Debug, Clone, Copy)]
    struct PreTripleReferenceModel {
        visual_work: bool,
        protocol_work: bool,
        pageflip_pending: bool,
        render_target_available: bool,
        ready_frame: bool,
        protocol_deadline_reached: bool,
        watchdog_expired: bool,
    }

    impl PreTripleReferenceModel {
        fn decision(self) -> SchedulerDecision {
            if self.pageflip_pending {
                if self.visual_work {
                    return if self.render_target_available {
                        SchedulerDecision::RenderAhead
                    } else {
                        SchedulerDecision::WaitForBuffer
                    };
                }
                return if self.watchdog_expired {
                    SchedulerDecision::PageFlipWatchdogExpired
                } else {
                    SchedulerDecision::WaitForPageFlip
                };
            }
            if self.visual_work {
                SchedulerDecision::Render
            } else if self.ready_frame {
                SchedulerDecision::SubmitReady
            } else if self.protocol_work {
                if self.protocol_deadline_reached {
                    SchedulerDecision::CompleteProtocolOnly
                } else {
                    SchedulerDecision::WaitForRefresh
                }
            } else {
                SchedulerDecision::Idle
            }
        }
    }

    #[test]
    fn reactive_double_matches_pretriple_idle_visual_decision_despite_prediction() {
        let reference = PreTripleReferenceModel {
            visual_work: true,
            protocol_work: false,
            pageflip_pending: false,
            render_target_available: true,
            ready_frame: false,
            protocol_deadline_reached: false,
            watchdog_expired: false,
        };
        let mut scheduler = NativeFrameScheduler::new(165, 0);
        scheduler.queue_visual_work();

        assert_eq!(
            scheduler.decision_with_context(reactive_context(10)),
            reference.decision()
        );
    }

    #[test]
    fn reactive_double_matches_physical_double_buffer_pressure_while_pending() {
        let reference = PreTripleReferenceModel {
            visual_work: true,
            protocol_work: false,
            pageflip_pending: true,
            render_target_available: false,
            ready_frame: false,
            protocol_deadline_reached: false,
            watchdog_expired: false,
        };
        let mut scheduler = NativeFrameScheduler::new(165, 0);
        scheduler.note_async_submission(41, 1).unwrap();
        scheduler.queue_visual_work();

        assert_eq!(
            scheduler.decision_with_context(reactive_context(2)),
            reference.decision()
        );
    }

    #[test]
    fn reactive_double_renders_queued_work_immediately_after_pageflip() {
        let mut scheduler = NativeFrameScheduler::new(165, 0);
        scheduler.note_async_submission(41, 1).unwrap();
        scheduler.queue_visual_work();
        assert_eq!(
            scheduler.note_page_flip_completion(41, 6_060_606),
            PageFlipCompletionResult::Completed { submitted_at_ns: 1 }
        );

        assert_eq!(
            scheduler.decision_with_context(reactive_context(6_060_606)),
            SchedulerDecision::Render
        );
    }

    #[test]
    fn reactive_double_conservative_prediction_and_logging_delay_do_not_change_decision() {
        let mut baseline = NativeFrameScheduler::new(165, 0);
        baseline.queue_visual_work();
        let mut delayed = baseline;

        assert_eq!(
            baseline.decision_with_context(reactive_context(1)),
            SchedulerDecision::Render
        );
        assert_eq!(
            delayed.decision_with_context(reactive_context(100_000_001)),
            SchedulerDecision::Render
        );
    }

    fn assert_deadlines_do_not_drift(refresh_hz: u32) {
        let mut scheduler = NativeFrameScheduler::new(refresh_hz, 1_000);
        let interval = scheduler.refresh_interval_ns();

        for boundary in 1..=10 {
            let queued_at = 1_000 + (boundary - 1) * interval + interval / 3;
            scheduler.queue_protocol_work(queued_at);
            assert_eq!(
                scheduler.next_deadline_ns(),
                Some(1_000 + boundary * interval)
            );
            assert_eq!(
                scheduler.decision(1_000 + boundary * interval),
                SchedulerDecision::CompleteProtocolOnly
            );
            scheduler.complete_protocol_only();
        }
    }

    #[test]
    fn sixty_hz_deadlines_do_not_accumulate_execution_time_drift() {
        assert_deadlines_do_not_drift(60);
    }

    #[test]
    fn one_sixty_five_hz_deadlines_do_not_accumulate_execution_time_drift() {
        assert_deadlines_do_not_drift(165);
    }

    #[test]
    fn two_forty_hz_deadlines_do_not_accumulate_execution_time_drift() {
        assert_deadlines_do_not_drift(240);
    }

    #[test]
    fn missed_deadline_advances_to_first_future_boundary() {
        let mut scheduler = NativeFrameScheduler::new(60, 0);
        let interval = scheduler.refresh_interval_ns();

        scheduler.queue_protocol_work(interval * 4 + 7);

        assert_eq!(scheduler.next_deadline_ns(), Some(interval * 5));
    }

    #[test]
    fn idle_has_no_refresh_deadline() {
        let scheduler = NativeFrameScheduler::new(60, 0);

        assert_eq!(scheduler.next_deadline_ns(), None);
    }

    #[test]
    fn protocol_work_arms_one_deadline_and_does_not_duplicate_completion() {
        let mut scheduler = NativeFrameScheduler::new(60, 0);
        scheduler.queue_protocol_work(1);
        let deadline = scheduler.next_deadline_ns().unwrap();

        scheduler.queue_protocol_work(deadline - 1);
        assert_eq!(scheduler.next_deadline_ns(), Some(deadline));
        assert_eq!(
            scheduler.decision(deadline - 1),
            SchedulerDecision::WaitForRefresh
        );
        assert_eq!(
            scheduler.decision(deadline),
            SchedulerDecision::CompleteProtocolOnly
        );
        scheduler.complete_protocol_only();
        assert_eq!(scheduler.decision(deadline), SchedulerDecision::Idle);
        assert_eq!(scheduler.next_deadline_ns(), None);
    }

    #[test]
    fn visual_work_without_pending_flip_renders() {
        let mut scheduler = NativeFrameScheduler::new(60, 0);
        scheduler.queue_visual_work();

        assert_eq!(scheduler.decision(0), SchedulerDecision::Render);
    }

    #[test]
    fn visual_work_during_pending_flip_is_queued() {
        let mut scheduler = NativeFrameScheduler::new(60, 0);
        scheduler.queue_visual_work();
        scheduler.note_async_submission(41, 10).unwrap();
        scheduler.queue_visual_work();

        assert_eq!(
            scheduler.decision_with_context(render_ahead_context(20, true, false)),
            SchedulerDecision::RenderAhead
        );
        assert!(scheduler.visual_work_queued());
    }

    #[test]
    fn scheduler_can_render_ahead_with_pending_pageflip() {
        let mut scheduler = NativeFrameScheduler::new(165, 0);
        scheduler.queue_visual_work();
        scheduler.note_async_submission(41, 10).unwrap();
        scheduler.queue_visual_work();

        assert_eq!(
            scheduler.decision_with_context(render_ahead_context(11, true, false)),
            SchedulerDecision::RenderAhead
        );
    }

    #[test]
    fn pending_pageflip_blocks_second_kms_submit_until_completion() {
        let mut scheduler = NativeFrameScheduler::new(165, 0);
        scheduler.queue_visual_work();
        scheduler.note_async_submission(41, 10).unwrap();

        assert_eq!(
            scheduler.note_async_submission(42, 11),
            Err("page flip already pending")
        );
    }

    #[test]
    fn session_suspend_retires_exact_pending_token_and_late_completion_is_stale() {
        let mut scheduler = NativeFrameScheduler::new(165, 0);
        scheduler.note_async_submission(52, 10).unwrap();

        scheduler.abandon_for_session_suspend();

        assert_eq!(scheduler.pending_page_flip_token(), None);
        assert_eq!(
            scheduler.note_page_flip_completion(52, 20),
            PageFlipCompletionResult::Stale
        );
    }

    #[test]
    fn ready_frame_submits_immediately_after_expected_pageflip() {
        let mut scheduler = NativeFrameScheduler::new(165, 0);
        scheduler.queue_visual_work();
        scheduler.note_async_submission(41, 10).unwrap();
        scheduler.queue_visual_work();
        assert_eq!(
            scheduler.decision_with_context(render_ahead_context(11, true, false)),
            SchedulerDecision::RenderAhead
        );
        scheduler.note_render_ahead_ready();
        assert!(scheduler.ready_frame_queued());

        assert_eq!(
            scheduler.note_page_flip_completion(41, 20),
            PageFlipCompletionResult::Completed {
                submitted_at_ns: 10
            }
        );

        assert_eq!(scheduler.decision(20), SchedulerDecision::SubmitReady);
        scheduler.note_ready_submission(42, 20).unwrap();
        assert_eq!(scheduler.pending_page_flip_token(), Some(42));
        assert!(!scheduler.ready_frame_queued());
    }

    #[test]
    fn ready_frame_for_next_refresh_submits_immediately() {
        let mut scheduler = NativeFrameScheduler::new(165, 0);
        scheduler.note_ready_frame(Some(PresentationTarget {
            sequence: 1,
            presentation_time: MonotonicTimestampNs::new(6_060_606),
            submit_not_before: MonotonicTimestampNs::new(100),
            render_start_deadline: MonotonicTimestampNs::new(0),
            refresh_interval: Duration::from_nanos(6_060_606),
            reason: PresentationTargetReason::Normal,
            clock_generation: 1,
            estimated: false,
            predicted_unreachable: false,
        }));

        assert_eq!(scheduler.decision(101), SchedulerDecision::SubmitReady);
    }

    #[test]
    fn ready_frame_for_n_plus_two_waits_until_the_previous_boundary() {
        let mut scheduler = NativeFrameScheduler::new(165, 0);
        let target = PresentationTarget {
            sequence: 2,
            presentation_time: MonotonicTimestampNs::new(18_181_818),
            submit_not_before: MonotonicTimestampNs::new(6_160_606),
            render_start_deadline: MonotonicTimestampNs::new(5_000_000),
            refresh_interval: Duration::from_nanos(6_060_606),
            reason: PresentationTargetReason::PredictedPressure,
            clock_generation: 1,
            estimated: false,
            predicted_unreachable: false,
        };
        scheduler.note_ready_frame(Some(target));

        assert_eq!(scheduler.next_deadline_ns(), Some(6_160_606));
        assert_eq!(
            scheduler.decision(6_160_605),
            SchedulerDecision::WaitForRefresh
        );
        assert_eq!(
            scheduler.decision(6_160_606),
            SchedulerDecision::SubmitReady
        );
        assert_eq!(scheduler.ready_target(), Some(target));
    }

    #[test]
    fn expired_ready_target_uses_explicit_late_submit_transition() {
        let mut scheduler = NativeFrameScheduler::new(165, 0);
        scheduler.note_ready_frame(Some(PresentationTarget {
            sequence: 2,
            presentation_time: MonotonicTimestampNs::new(100),
            submit_not_before: MonotonicTimestampNs::new(50),
            render_start_deadline: MonotonicTimestampNs::new(0),
            refresh_interval: Duration::from_millis(6),
            reason: PresentationTargetReason::PredictedPressure,
            clock_generation: 1,
            estimated: false,
            predicted_unreachable: false,
        }));

        assert_eq!(scheduler.decision(101), SchedulerDecision::SubmitReadyLate);
        assert_eq!(scheduler.ready_target().unwrap().sequence, 2);
    }

    #[test]
    fn invalidated_ready_target_cannot_be_submitted() {
        let mut scheduler = NativeFrameScheduler::new(165, 0);
        let mut context = render_ahead_context(10, true, true);
        let target = context.presentation_target.expect("test target");
        context.ready_target_current = false;
        scheduler.note_ready_frame(Some(target));

        assert_eq!(
            scheduler.decision_with_context(context),
            SchedulerDecision::ReadyTargetInvalidated
        );
    }

    #[test]
    fn ready_frame_is_never_replaced_by_newer_visual_work() {
        let mut scheduler = NativeFrameScheduler::new(165, 0);
        scheduler.queue_visual_work();
        scheduler.note_async_submission(41, 10).unwrap();
        scheduler.queue_visual_work();
        scheduler.note_render_ahead_ready();
        scheduler.queue_visual_work();

        assert_eq!(
            scheduler.decision_with_context(render_ahead_context(12, true, true)),
            SchedulerDecision::WaitForPageFlip
        );

        assert!(scheduler.ready_frame_queued());
        assert!(scheduler.visual_work_queued());
    }

    #[test]
    fn queued_visual_generation_cannot_replace_ready_target_identity() {
        let mut scheduler = NativeFrameScheduler::new(165, 0);
        let target = render_ahead_context(10, true, false)
            .presentation_target
            .map(|mut target| {
                target.submit_not_before = MonotonicTimestampNs::new(100);
                target
            })
            .expect("test target");
        scheduler.note_ready_frame(Some(target));
        scheduler.queue_visual_work();

        assert_eq!(scheduler.ready_target(), Some(target));
        assert!(scheduler.ready_frame_queued());
        assert_eq!(scheduler.decision(10), SchedulerDecision::WaitForRefresh);
    }

    #[test]
    fn bounded_pipeline_never_accumulates_unbounded_frames() {
        let mut scheduler = NativeFrameScheduler::new(165, 0);
        scheduler.queue_visual_work();
        scheduler.note_async_submission(41, 10).unwrap();

        scheduler.queue_visual_work();
        assert_eq!(
            scheduler.decision_with_context(render_ahead_context(11, true, false)),
            SchedulerDecision::RenderAhead
        );
        scheduler.note_render_ahead_ready();
        for now in 12..100 {
            scheduler.queue_visual_work();
            assert_eq!(
                scheduler.decision_with_context(render_ahead_context(now, true, true)),
                SchedulerDecision::WaitForPageFlip
            );
            assert!(scheduler.ready_frame_queued());
        }
    }

    #[test]
    fn no_free_render_target_waits_without_clearing_visual_work() {
        let mut scheduler = NativeFrameScheduler::new(165, 0);
        scheduler.queue_visual_work();
        scheduler.note_async_submission(41, 10).unwrap();
        scheduler.queue_visual_work();

        assert_eq!(
            scheduler.decision_with_context(render_ahead_context(11, false, false)),
            SchedulerDecision::WaitForBuffer
        );
        assert!(scheduler.visual_work_queued());
    }

    #[test]
    fn pending_render_ahead_requires_policy_capability_and_target() {
        let mut scheduler = NativeFrameScheduler::new(60, 0);
        scheduler.note_async_submission(41, 1).unwrap();
        scheduler.queue_visual_work();
        let mut context = render_ahead_context(2, true, false);

        context.render_ahead_allowed = false;
        assert_eq!(
            scheduler.decision_with_context(context),
            SchedulerDecision::WaitForPageFlip
        );
        context.render_ahead_allowed = true;
        context.capabilities = SchedulerCapabilities::legacy();
        assert_eq!(
            scheduler.decision_with_context(context),
            SchedulerDecision::WaitForPageFlip
        );
        context.capabilities = SchedulerCapabilities::explicit_atomic(true, true);
        context.presentation_target = None;
        assert_eq!(
            scheduler.decision_with_context(context),
            SchedulerDecision::WaitForPageFlip
        );
    }

    #[test]
    fn target_deadline_arms_without_busy_rendering() {
        let mut scheduler = NativeFrameScheduler::new(60, 0);
        scheduler.note_async_submission(41, 1).unwrap();
        scheduler.queue_visual_work();
        let mut context = render_ahead_context(2, true, false);
        context
            .presentation_target
            .as_mut()
            .unwrap()
            .render_start_deadline = MonotonicTimestampNs::new(3);

        assert_eq!(
            scheduler.decision_with_context(context),
            SchedulerDecision::WaitForRefresh
        );
    }

    #[test]
    fn page_flip_completion_finishes_exactly_once() {
        let mut scheduler = NativeFrameScheduler::new(60, 0);
        scheduler.queue_visual_work();
        scheduler.note_async_submission(41, 10).unwrap();

        assert_eq!(
            scheduler.note_page_flip_completion(41, 20),
            PageFlipCompletionResult::Completed {
                submitted_at_ns: 10
            }
        );
        assert_eq!(
            scheduler.note_page_flip_completion(41, 21),
            PageFlipCompletionResult::Stale
        );
        assert!(!scheduler.page_flip_pending());
    }

    #[test]
    fn spurious_drm_readiness_does_not_finish_frame() {
        let mut scheduler = NativeFrameScheduler::new(60, 0);

        assert_eq!(
            scheduler.note_page_flip_completion(41, 20),
            PageFlipCompletionResult::Stale
        );
    }

    #[test]
    fn queued_work_after_page_flip_is_renderable_immediately() {
        let mut scheduler = NativeFrameScheduler::new(60, 0);
        scheduler.queue_visual_work();
        scheduler.note_async_submission(41, 10).unwrap();
        scheduler.queue_visual_work();
        assert_eq!(
            scheduler.note_page_flip_completion(41, 20),
            PageFlipCompletionResult::Completed {
                submitted_at_ns: 10
            }
        );

        assert_eq!(scheduler.decision(20), SchedulerDecision::Render);
    }

    #[test]
    fn watchdog_never_fabricates_completion() {
        let mut scheduler = NativeFrameScheduler::with_watchdog(60, 0, 100);
        scheduler.queue_visual_work();
        scheduler.note_async_submission(41, 10).unwrap();

        assert_eq!(scheduler.decision(109), SchedulerDecision::WaitForPageFlip);
        assert_eq!(
            scheduler.decision(110),
            SchedulerDecision::PageFlipWatchdogExpired
        );
        assert!(scheduler.page_flip_pending());
        assert_eq!(scheduler.watchdog_timeout_count(), 1);
        assert_eq!(
            scheduler.note_page_flip_completion(41, 111),
            PageFlipCompletionResult::Completed {
                submitted_at_ns: 10
            }
        );
    }

    #[test]
    fn immediate_scanout_completes_without_page_flip_state() {
        let mut scheduler = NativeFrameScheduler::new(60, 0);
        scheduler.queue_visual_work();
        scheduler.note_immediate_completion();

        assert_eq!(scheduler.decision(0), SchedulerDecision::Idle);
        assert!(!scheduler.page_flip_pending());
        assert!(!scheduler.visual_work_queued());
    }

    #[test]
    fn second_async_submission_is_rejected() {
        let mut scheduler = NativeFrameScheduler::new(60, 0);
        scheduler.queue_visual_work();
        scheduler.note_async_submission(41, 1).unwrap();

        assert_eq!(
            scheduler.note_async_submission(42, 2),
            Err("page flip already pending")
        );
    }

    #[test]
    fn mismatched_page_flip_token_keeps_pending_frame() {
        let mut scheduler = NativeFrameScheduler::new(60, 0);
        scheduler.queue_visual_work();
        scheduler.note_async_submission(41, 1).unwrap();

        assert_eq!(
            scheduler.note_page_flip_completion(42, 2),
            PageFlipCompletionResult::Mismatched { expected: 41 }
        );
        assert!(scheduler.page_flip_pending());
        assert_eq!(scheduler.pending_page_flip_token(), Some(41));
    }

    #[test]
    fn stale_completion_cannot_finish_next_submission() {
        let mut scheduler = NativeFrameScheduler::new(60, 0);
        scheduler.queue_visual_work();
        scheduler.note_async_submission(41, 1).unwrap();
        assert_eq!(
            scheduler.note_page_flip_completion(41, 2),
            PageFlipCompletionResult::Completed { submitted_at_ns: 1 }
        );
        scheduler.queue_visual_work();
        scheduler.note_async_submission(42, 3).unwrap();

        assert_eq!(
            scheduler.note_page_flip_completion(41, 4),
            PageFlipCompletionResult::Mismatched { expected: 42 }
        );
        assert_eq!(scheduler.pending_page_flip_token(), Some(42));
    }

    #[test]
    fn presentation_interval_uses_drm_timestamp_and_sequence() {
        let mut metrics = PresentationCadenceMetrics::default();
        assert_eq!(
            metrics
                .record_with_refresh(10, 1_000_000, 6_060)
                .interval_us,
            None
        );

        let sample = metrics.record_with_refresh(11, 1_006_060, 6_060);

        assert_eq!(sample.interval_us, Some(6_060));
        assert_eq!(sample.sequence_delta, Some(1));
        assert_eq!(sample.logical_sequence_delta, Some(1));
        assert_eq!(sample.estimated_hz_millihz, Some(165_016));
        assert!(!sample.sequence_gap);
    }

    #[test]
    fn sequence_gap_metric_detects_missed_presentations() {
        let mut metrics = PresentationCadenceMetrics::default();
        metrics.record_with_refresh(10, 1_000_000, 6_060);

        let sample = metrics.record_with_refresh(13, 1_018_180, 6_060);

        assert_eq!(sample.sequence_delta, Some(3));
        assert!(sample.sequence_gap);
        assert_eq!(sample.sequence_gaps, 1);
        assert_eq!(metrics.sequence_gaps(), 1);
    }

    #[test]
    fn zero_drm_sequence_uses_timestamp_logical_sequence() {
        let mut metrics = PresentationCadenceMetrics::default();
        metrics.record_with_refresh(0, 1_000_000, 6_060);
        let sample = metrics.record_with_refresh(0, 1_018_181, 6_060);

        assert!(sample.timestamp_sequence_fallback);
        assert_eq!(sample.logical_sequence_delta, Some(3));
        assert_eq!(sample.logical_sequence, 4);
        assert!(sample.sequence_gap);
    }

    #[test]
    fn timestamp_cadence_classifies_one_two_and_three_refresh_intervals() {
        let mut metrics = PresentationCadenceMetrics::default();
        metrics.record_with_refresh(0, 0, 6_060);
        assert_eq!(
            metrics
                .record_with_refresh(0, 6_060, 6_060)
                .logical_sequence_delta,
            Some(1)
        );
        assert_eq!(
            metrics
                .record_with_refresh(0, 18_181, 6_060)
                .logical_sequence_delta,
            Some(2)
        );
        assert_eq!(
            metrics
                .record_with_refresh(0, 36_362, 6_060)
                .logical_sequence_delta,
            Some(3)
        );
    }

    #[test]
    fn one_thousand_frame_low_load_cadence_has_no_refresh_gaps_or_target_early_path() {
        let refresh_us: u64 = 6_060;
        let mut metrics = PresentationCadenceMetrics::default();
        for frame in 0..1_000 {
            let sample = metrics.record_with_refresh(0, frame * refresh_us, refresh_us);
            if frame > 0 {
                assert_eq!(sample.logical_sequence_delta, Some(1));
                assert!(!sample.sequence_gap);
            }
            assert!(sample.timestamp_sequence_fallback);
        }
        assert_eq!(metrics.sequence_gaps(), 0);
        assert_eq!(metrics.logical_sequence(), 1_000);
    }
}
