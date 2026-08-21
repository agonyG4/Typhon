//! Presentation-clock identities and immutable render-start deadlines.
#![allow(dead_code)] // Wired into the native runtime in Task 12.

use std::time::Duration;

use crate::native::buffering::{OpportunityLease, OpportunityLeaseReason, PresentationOpportunity};

#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MonotonicTimestampNs(u64);

impl MonotonicTimestampNs {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub(crate) fn checked_add(self, duration: Duration) -> Option<Self> {
        self.0
            .checked_add(duration_ns(duration))
            .map(MonotonicTimestampNs)
    }

    pub(crate) fn saturating_sub_duration(self, duration: Duration) -> Self {
        Self(self.0.saturating_sub(duration_ns(duration)))
    }
}

#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationTargetReason {
    ReactiveDouble,
    Normal,
    PredictedPressure,
    ProvenReadinessMiss,
    ForcedValidation,
    CommitTiming,
}

#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentationTarget {
    pub sequence: u64,
    pub presentation_time: MonotonicTimestampNs,
    pub submit_not_before: MonotonicTimestampNs,
    pub render_start_deadline: MonotonicTimestampNs,
    pub refresh_interval: Duration,
    pub reason: PresentationTargetReason,
    pub clock_generation: u64,
    pub estimated: bool,
    pub predicted_unreachable: bool,
}

impl PresentationTarget {
    pub const fn sequence(self) -> u64 {
        self.sequence
    }

    pub const fn identity(self) -> (u64, u64) {
        (self.clock_generation, self.sequence)
    }

    pub const fn render_start_deadline(self) -> MonotonicTimestampNs {
        self.render_start_deadline
    }

    pub const fn submit_not_before(self) -> MonotonicTimestampNs {
        self.submit_not_before
    }

    pub const fn predicted_unreachable(self) -> bool {
        self.predicted_unreachable
    }

    pub const fn opportunity(self) -> PresentationOpportunity {
        PresentationOpportunity::fixed_vsync(
            crate::native::buffering::PresentationOpportunityId::new(
                self.clock_generation,
                self.sequence,
            ),
            self.presentation_time,
            self.refresh_interval,
        )
    }

    pub const fn opportunity_lease(self) -> OpportunityLease {
        let reason = match self.reason {
            PresentationTargetReason::ReactiveDouble => OpportunityLeaseReason::VisualWork,
            PresentationTargetReason::Normal => OpportunityLeaseReason::VisualWork,
            PresentationTargetReason::PredictedPressure => OpportunityLeaseReason::RenderAhead,
            PresentationTargetReason::ProvenReadinessMiss => OpportunityLeaseReason::Recovery,
            PresentationTargetReason::ForcedValidation => OpportunityLeaseReason::ForcedValidation,
            PresentationTargetReason::CommitTiming => OpportunityLeaseReason::CommitTiming,
        };
        OpportunityLease::arm(self.opportunity(), reason)
    }
}

#[doc(hidden)]
#[derive(Debug, Clone, Copy)]
pub struct PresentationDeadlinePlanner {
    clock_generation: u64,
    last_presented_sequence: u64,
    last_presented_at: Option<MonotonicTimestampNs>,
    refresh_interval: Duration,
    scheduled: Option<PresentationTarget>,
    pre_render_abandoned: u64,
}

impl PresentationDeadlinePlanner {
    pub fn new(refresh_interval: Duration) -> Self {
        Self {
            clock_generation: 1,
            last_presented_sequence: 0,
            last_presented_at: None,
            refresh_interval: nonzero_refresh(refresh_interval),
            scheduled: None,
            pre_render_abandoned: 0,
        }
    }

    pub fn note_presented(&mut self, presented_at: MonotonicTimestampNs) -> u64 {
        let logical_sequence = self
            .last_presented_at
            .map(|previous| {
                let elapsed = presented_at.get().saturating_sub(previous.get());
                let refresh_ns = duration_ns(self.refresh_interval).max(1);
                let intervals = elapsed
                    .saturating_add(refresh_ns / 2)
                    .checked_div(refresh_ns)
                    .unwrap_or(1)
                    .max(1);
                self.last_presented_sequence.saturating_add(intervals)
            })
            .unwrap_or_else(|| self.last_presented_sequence.saturating_add(1));
        self.last_presented_sequence = logical_sequence;
        self.last_presented_at = Some(presented_at);
        self.scheduled = None;
        logical_sequence
    }

    pub fn plan_normal(
        &mut self,
        now: MonotonicTimestampNs,
        predicted_total_cost: Duration,
    ) -> Option<PresentationTarget> {
        let ready_at = now.checked_add(predicted_total_cost)?;
        let (sequence, presentation_time, estimated) = self.earliest_reachable(ready_at)?;
        let target = self.make_target(
            sequence,
            presentation_time,
            predicted_total_cost,
            PresentationTargetReason::Normal,
            estimated,
            false,
            if estimated {
                MonotonicTimestampNs::new(0)
            } else {
                submit_not_before(presentation_time, self.refresh_interval)
            },
        );
        self.scheduled = Some(target);
        Some(target)
    }

    /// Select the first refresh target which is both reachable after the
    /// predicted render cost and not earlier than a surface Commit Timing
    /// request.  The request is therefore a presentation lower bound, not a
    /// timer which releases work at the last possible instant.
    pub fn plan_not_before(
        &mut self,
        now: MonotonicTimestampNs,
        requested_target: MonotonicTimestampNs,
        predicted_total_cost: Duration,
    ) -> Option<PresentationTarget> {
        let ready_at = now.checked_add(predicted_total_cost)?;
        let eligible_at = MonotonicTimestampNs::new(ready_at.get().max(requested_target.get()));
        let (sequence, presentation_time, estimated) = self.earliest_reachable(eligible_at)?;
        let target = self.make_target(
            sequence,
            presentation_time,
            predicted_total_cost,
            PresentationTargetReason::CommitTiming,
            estimated,
            false,
            if estimated {
                now
            } else {
                submit_not_before(presentation_time, self.refresh_interval)
            },
        );
        self.scheduled = Some(target);
        Some(target)
    }

    pub fn reactive_target(
        &self,
        now: MonotonicTimestampNs,
        predicted_total_cost: Duration,
    ) -> Option<PresentationTarget> {
        let ready_at = now.checked_add(predicted_total_cost)?;
        let (sequence, presentation_time, estimated) = self.earliest_reachable(ready_at)?;
        Some(PresentationTarget {
            sequence,
            presentation_time,
            submit_not_before: now,
            // Reactive Double starts rendering as soon as the normal frame
            // opportunity arrives. The reachable target is accounting
            // metadata and must not turn into a render-start gate.
            render_start_deadline: now,
            refresh_interval: self.refresh_interval,
            reason: PresentationTargetReason::ReactiveDouble,
            clock_generation: self.clock_generation,
            estimated,
            predicted_unreachable: false,
        })
    }

    /// Select the first reachable refresh opportunity strictly after an
    /// already-owned future target. The lower bound is an ownership/order
    /// constraint, not a render-start deadline.
    pub fn reactive_target_after(
        &self,
        now: MonotonicTimestampNs,
        predicted_total_cost: Duration,
        lower_bound: PresentationTarget,
    ) -> Option<PresentationTarget> {
        let ready_at = now.checked_add(predicted_total_cost)?;
        let (sequence, presentation_time, estimated) =
            self.earliest_reachable_after(ready_at, lower_bound)?;

        Some(PresentationTarget {
            sequence,
            presentation_time,
            submit_not_before: now,
            render_start_deadline: now,
            refresh_interval: self.refresh_interval,
            reason: PresentationTargetReason::ReactiveDouble,
            clock_generation: self.clock_generation,
            estimated,
            predicted_unreachable: false,
        })
    }

    /// Replan a scheduled target which has fallen behind an already-owned
    /// future target. A later protocol lower bound remains a valid lower
    /// bound, so preserving the target reason is safe.
    pub fn plan_successor_after(
        &mut self,
        lower_bound: PresentationTarget,
        now: MonotonicTimestampNs,
        predicted_total_cost: Duration,
        reason: PresentationTargetReason,
    ) -> Option<PresentationTarget> {
        if !self.is_current(lower_bound) {
            return None;
        }
        let ready_at = now.checked_add(predicted_total_cost)?;
        let (sequence, presentation_time, estimated) =
            self.earliest_reachable_after(ready_at, lower_bound)?;
        let submit_not_before = if estimated || reason == PresentationTargetReason::ReactiveDouble {
            now
        } else {
            submit_not_before(presentation_time, self.refresh_interval)
        };
        let target = self.make_target(
            sequence,
            presentation_time,
            predicted_total_cost,
            reason,
            estimated,
            false,
            submit_not_before,
        );
        self.scheduled = Some(target);
        Some(target)
    }

    pub const fn scheduled_target(&self) -> Option<PresentationTarget> {
        self.scheduled
    }

    pub fn clear_scheduled_target(&mut self) {
        self.scheduled = None;
    }

    /// End a target that has not entered rendering.  The caller must allocate
    /// a new target identity; this method never retargets the old value.
    pub fn abandon_scheduled_target(&mut self) -> Option<PresentationTarget> {
        let abandoned = self.scheduled.take();
        if abandoned.is_some() {
            self.pre_render_abandoned = self.pre_render_abandoned.saturating_add(1);
        }
        abandoned
    }

    pub const fn pre_render_abandoned(&self) -> u64 {
        self.pre_render_abandoned
    }

    pub fn plan_render_ahead(
        &mut self,
        pending: PresentationTarget,
        now: MonotonicTimestampNs,
        predicted_total_cost: Duration,
        reason: PresentationTargetReason,
    ) -> Option<PresentationTarget> {
        if !self.is_current(pending) {
            return None;
        }
        let sequence = pending.sequence.checked_add(1)?;
        let presentation_time = pending
            .presentation_time
            .checked_add(pending.refresh_interval)?;
        let ready_at = now.checked_add(predicted_total_cost)?;
        let unreachable = ready_at > presentation_time;
        if unreachable
            && !matches!(
                reason,
                PresentationTargetReason::ProvenReadinessMiss
                    | PresentationTargetReason::ForcedValidation
            )
        {
            return None;
        }
        let target = self.make_target(
            sequence,
            presentation_time,
            predicted_total_cost,
            reason,
            pending.estimated,
            unreachable,
            pending
                .presentation_time
                .checked_add(SUBMIT_NOT_BEFORE_GUARD)
                .unwrap_or(pending.presentation_time),
        );
        self.scheduled = Some(target);
        Some(target)
    }

    pub fn reschedule_earlier(
        &mut self,
        target: PresentationTarget,
        predicted_total_cost: Duration,
    ) -> PresentationTarget {
        if !self.is_current(target) {
            return target;
        }
        let candidate = target
            .presentation_time
            .saturating_sub_duration(predicted_total_cost);
        let updated = PresentationTarget {
            render_start_deadline: candidate.min(target.render_start_deadline),
            ..target
        };
        self.scheduled = Some(updated);
        updated
    }

    pub fn invalidate(&mut self, refresh_interval: Duration) {
        self.clock_generation = self.clock_generation.checked_add(1).unwrap_or(1);
        self.last_presented_sequence = 0;
        self.last_presented_at = None;
        self.refresh_interval = nonzero_refresh(refresh_interval);
        self.scheduled = None;
    }

    pub const fn is_current(&self, target: PresentationTarget) -> bool {
        target.clock_generation == self.clock_generation
    }

    fn earliest_reachable(
        &self,
        ready_at: MonotonicTimestampNs,
    ) -> Option<(u64, MonotonicTimestampNs, bool)> {
        let refresh_ns = duration_ns(self.refresh_interval);
        let Some(anchor) = self.last_presented_at else {
            return ready_at
                .checked_add(self.refresh_interval)
                .map(|time| (self.last_presented_sequence.saturating_add(1), time, true));
        };
        let delta = ready_at.get().saturating_sub(anchor.get());
        let intervals = delta.div_ceil(refresh_ns).max(1);
        let sequence = self.last_presented_sequence.checked_add(intervals)?;
        let presentation_time = anchor
            .get()
            .checked_add(intervals.checked_mul(refresh_ns)?)
            .map(MonotonicTimestampNs)?;
        Some((sequence, presentation_time, false))
    }

    fn earliest_reachable_after(
        &self,
        ready_at: MonotonicTimestampNs,
        lower_bound: PresentationTarget,
    ) -> Option<(u64, MonotonicTimestampNs, bool)> {
        if !self.is_current(lower_bound) {
            return self.earliest_reachable(ready_at);
        }
        let base = self.earliest_reachable(ready_at)?;
        let refresh_ns = duration_ns(self.refresh_interval).max(1);
        let mut sequence = lower_bound.sequence.checked_add(1)?;
        let mut presentation_time = lower_bound
            .presentation_time
            .checked_add(self.refresh_interval)?;
        if presentation_time < ready_at {
            let intervals = (ready_at.get() - presentation_time.get()).div_ceil(refresh_ns);
            sequence = sequence.checked_add(intervals)?;
            presentation_time = MonotonicTimestampNs::new(
                presentation_time
                    .get()
                    .checked_add(intervals.checked_mul(refresh_ns)?)?,
            );
        }
        if base.1 > presentation_time || (base.1 == presentation_time && base.0 > sequence) {
            Some((base.0, base.1, base.2 || lower_bound.estimated))
        } else {
            Some((sequence, presentation_time, base.2 || lower_bound.estimated))
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn make_target(
        &self,
        sequence: u64,
        presentation_time: MonotonicTimestampNs,
        predicted_total_cost: Duration,
        reason: PresentationTargetReason,
        estimated: bool,
        predicted_unreachable: bool,
        submit_not_before: MonotonicTimestampNs,
    ) -> PresentationTarget {
        PresentationTarget {
            sequence,
            presentation_time,
            submit_not_before,
            render_start_deadline: presentation_time.saturating_sub_duration(predicted_total_cost),
            refresh_interval: self.refresh_interval,
            reason,
            clock_generation: self.clock_generation,
            estimated,
            predicted_unreachable,
        }
    }
}

const SUBMIT_NOT_BEFORE_GUARD: Duration = Duration::from_micros(100);

fn submit_not_before(
    presentation_time: MonotonicTimestampNs,
    refresh_interval: Duration,
) -> MonotonicTimestampNs {
    presentation_time
        .saturating_sub_duration(refresh_interval)
        .checked_add(SUBMIT_NOT_BEFORE_GUARD)
        .unwrap_or(presentation_time)
}

fn duration_ns(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

fn nonzero_refresh(refresh_interval: Duration) -> Duration {
    if refresh_interval.is_zero() {
        Duration::from_nanos(1)
    } else {
        refresh_interval
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const REFRESH_NS: u64 = 10_000_000;

    #[test]
    fn pending_frame_targets_exactly_the_following_sequence() {
        let mut planner = PresentationDeadlinePlanner::new(Duration::from_nanos(REFRESH_NS));
        assert_eq!(
            planner.note_presented(MonotonicTimestampNs::new(70_000_000)),
            1
        );
        let pending = planner
            .plan_normal(
                MonotonicTimestampNs::new(71_000_000),
                Duration::from_millis(2),
            )
            .unwrap();

        let ready = planner
            .plan_render_ahead(
                pending,
                MonotonicTimestampNs::new(72_000_000),
                Duration::from_millis(2),
                PresentationTargetReason::PredictedPressure,
            )
            .unwrap();

        assert_eq!(ready.sequence(), pending.sequence() + 1);
        assert!(!ready.predicted_unreachable());
    }

    #[test]
    fn predictive_render_ahead_rejects_an_unreachable_next_target() {
        let mut planner = PresentationDeadlinePlanner::new(Duration::from_nanos(REFRESH_NS));
        planner.note_presented(MonotonicTimestampNs::new(70_000_000));
        let pending = planner
            .plan_normal(
                MonotonicTimestampNs::new(71_000_000),
                Duration::from_millis(2),
            )
            .unwrap();

        assert!(
            planner
                .plan_render_ahead(
                    pending,
                    MonotonicTimestampNs::new(78_000_001),
                    Duration::from_millis(12),
                    PresentationTargetReason::PredictedPressure,
                )
                .is_none()
        );
    }

    #[test]
    fn proven_miss_attempts_only_pending_plus_one_when_unreachable() {
        let mut planner = PresentationDeadlinePlanner::new(Duration::from_nanos(REFRESH_NS));
        planner.note_presented(MonotonicTimestampNs::new(70_000_000));
        let pending = planner
            .plan_normal(
                MonotonicTimestampNs::new(71_000_000),
                Duration::from_millis(2),
            )
            .unwrap();
        let recovery = planner
            .plan_render_ahead(
                pending,
                MonotonicTimestampNs::new(78_000_001),
                Duration::from_millis(12),
                PresentationTargetReason::ProvenReadinessMiss,
            )
            .unwrap();

        assert_eq!(recovery.sequence(), pending.sequence() + 1);
        assert!(recovery.predicted_unreachable());
    }

    #[test]
    fn changed_estimate_only_moves_an_armed_deadline_earlier() {
        let mut planner = PresentationDeadlinePlanner::new(Duration::from_nanos(REFRESH_NS));
        planner.note_presented(MonotonicTimestampNs::new(70_000_000));
        let original = planner
            .plan_normal(
                MonotonicTimestampNs::new(71_000_000),
                Duration::from_millis(2),
            )
            .unwrap();

        let earlier = planner.reschedule_earlier(original, Duration::from_millis(4));
        let not_later = planner.reschedule_earlier(earlier, Duration::from_millis(1));

        assert_eq!(earlier.identity(), original.identity());
        assert!(earlier.render_start_deadline() < original.render_start_deadline());
        assert_eq!(
            not_later.render_start_deadline(),
            earlier.render_start_deadline()
        );
    }

    #[test]
    fn clock_generation_change_invalidates_old_targets() {
        let mut planner = PresentationDeadlinePlanner::new(Duration::from_nanos(REFRESH_NS));
        let target = planner
            .plan_normal(MonotonicTimestampNs::new(1), Duration::from_millis(2))
            .unwrap();

        planner.invalidate(Duration::from_nanos(REFRESH_NS));

        assert!(!planner.is_current(target));
    }

    #[test]
    fn overtaken_target_is_abandoned_before_a_new_successor_is_allocated() {
        let mut planner = PresentationDeadlinePlanner::new(Duration::from_nanos(REFRESH_NS));
        planner.note_presented(MonotonicTimestampNs::new(70_000_000));
        let original = planner
            .plan_normal(
                MonotonicTimestampNs::new(71_000_000),
                Duration::from_millis(2),
            )
            .unwrap();

        assert_eq!(planner.abandon_scheduled_target(), Some(original));
        let replacement = planner
            .plan_successor_after(
                PresentationTarget {
                    sequence: original.sequence + 1,
                    presentation_time: MonotonicTimestampNs::new(90_000_000),
                    ..original
                },
                MonotonicTimestampNs::new(95_000_000),
                Duration::from_millis(2),
                PresentationTargetReason::PredictedPressure,
            )
            .unwrap();

        assert_ne!(replacement.identity(), original.identity());
        assert_eq!(planner.pre_render_abandoned(), 1);
        assert_eq!(planner.scheduled_target(), Some(replacement));
    }

    #[test]
    fn presented_sequence_is_derived_from_timestamp_intervals() {
        let mut planner = PresentationDeadlinePlanner::new(Duration::from_nanos(6_060_606));

        assert_eq!(
            planner.note_presented(MonotonicTimestampNs::new(6_060_606)),
            1
        );
        assert_eq!(
            planner.note_presented(MonotonicTimestampNs::new(18_181_818)),
            3
        );
        assert_eq!(
            planner.note_presented(MonotonicTimestampNs::new(36_363_636)),
            6
        );
    }

    #[test]
    fn normal_target_submission_boundary_is_immediate_for_n_plus_one_only() {
        let mut planner = PresentationDeadlinePlanner::new(Duration::from_nanos(REFRESH_NS));
        planner.note_presented(MonotonicTimestampNs::new(70_000_000));

        let next = planner
            .plan_normal(
                MonotonicTimestampNs::new(71_000_000),
                Duration::from_millis(2),
            )
            .unwrap();
        assert_eq!(next.sequence, 2);
        assert!(next.submit_not_before().get() < 71_000_000);

        let later = planner
            .plan_normal(
                MonotonicTimestampNs::new(71_000_000),
                Duration::from_millis(12),
            )
            .unwrap();
        assert_eq!(later.sequence, 3);
        assert!(later.submit_not_before().get() > 71_000_000);
        assert_eq!(later.submit_not_before().get(), 80_100_000);
    }

    #[test]
    fn reactive_target_is_non_gating_reachable_metadata() {
        let mut planner = PresentationDeadlinePlanner::new(Duration::from_nanos(REFRESH_NS));
        assert_eq!(
            planner.note_presented(MonotonicTimestampNs::new(70_000_000)),
            1
        );

        let target = planner
            .reactive_target(
                MonotonicTimestampNs::new(75_000_000),
                Duration::from_millis(2),
            )
            .unwrap();

        assert_eq!(target.sequence, 2);
        assert_eq!(target.presentation_time.get(), 80_000_000);
        assert_eq!(target.render_start_deadline.get(), 75_000_000);
        assert_eq!(target.submit_not_before().get(), 75_000_000);
        assert_eq!(target.reason, PresentationTargetReason::ReactiveDouble);
        assert_eq!(planner.scheduled_target(), None);
    }

    #[test]
    fn reactive_target_selects_first_reachable_opportunity_after_a_late_wake() {
        let mut planner = PresentationDeadlinePlanner::new(Duration::from_nanos(REFRESH_NS));
        planner.note_presented(MonotonicTimestampNs::new(70_000_000));

        let target = planner
            .reactive_target(
                MonotonicTimestampNs::new(95_000_000),
                Duration::from_millis(2),
            )
            .unwrap();

        assert_eq!(target.sequence, 4);
        assert_eq!(target.presentation_time.get(), 100_000_000);
        assert_eq!(target.submit_not_before().get(), 95_000_000);
        assert_eq!(target.render_start_deadline.get(), 95_000_000);
        assert_eq!(planner.scheduled_target(), None);
    }

    #[test]
    fn reactive_target_after_preserves_strict_order_with_a_future_target() {
        let mut planner = PresentationDeadlinePlanner::new(Duration::from_nanos(10_000_000));
        planner.note_presented(MonotonicTimestampNs::new(70_000_000));
        let lower_bound = planner
            .reactive_target(
                MonotonicTimestampNs::new(75_000_000),
                Duration::from_millis(2),
            )
            .unwrap();

        let target = planner
            .reactive_target_after(
                MonotonicTimestampNs::new(75_000_000),
                Duration::from_millis(2),
                lower_bound,
            )
            .unwrap();

        assert_eq!(target.sequence, 3);
        assert_eq!(target.presentation_time.get(), 90_000_000);
        assert_eq!(target.render_start_deadline.get(), 75_000_000);
        assert_eq!(target.submit_not_before().get(), 75_000_000);
    }

    #[test]
    fn reactive_target_after_skips_multiple_unreachable_opportunities() {
        let mut planner = PresentationDeadlinePlanner::new(Duration::from_nanos(10_000_000));
        planner.note_presented(MonotonicTimestampNs::new(70_000_000));
        let lower_bound = planner
            .reactive_target(
                MonotonicTimestampNs::new(75_000_000),
                Duration::from_millis(2),
            )
            .unwrap();

        let target = planner
            .reactive_target_after(
                MonotonicTimestampNs::new(95_000_000),
                Duration::from_millis(2),
                lower_bound,
            )
            .unwrap();

        assert_eq!(target.sequence, 4);
        assert_eq!(target.presentation_time.get(), 100_000_000);
    }

    #[test]
    fn one_thousand_reactive_frames_never_intentionally_target_n_plus_two() {
        let mut planner = PresentationDeadlinePlanner::new(Duration::from_nanos(6_060_606));
        let mut presented_at = MonotonicTimestampNs::new(0);
        for expected_sequence in 1..=1_000 {
            let target = planner
                .reactive_target(presented_at, Duration::ZERO)
                .unwrap();
            assert_eq!(target.sequence, expected_sequence);
            assert_eq!(target.reason, PresentationTargetReason::ReactiveDouble);
            assert_eq!(planner.scheduled_target(), None);
            presented_at = target.presentation_time;
            assert_eq!(planner.note_presented(presented_at), expected_sequence);
        }
    }

    #[test]
    fn commit_timing_chooses_first_refresh_not_before_requested_target() {
        for refresh in [60, 120, 165] {
            let interval = Duration::from_nanos(1_000_000_000 / refresh);
            let mut planner = PresentationDeadlinePlanner::new(interval);
            planner.note_presented(MonotonicTimestampNs::new(1_000_000_000));
            let requested =
                MonotonicTimestampNs::new(1_000_000_000 + interval.as_nanos() as u64 + 1);
            let target = planner
                .plan_not_before(
                    MonotonicTimestampNs::new(1_000_000_000 + 1_000),
                    requested,
                    Duration::from_micros(500),
                )
                .unwrap();
            assert!(target.presentation_time.get() >= requested.get());
            assert_eq!(target.reason, PresentationTargetReason::CommitTiming);
        }
    }

    #[test]
    fn commit_timing_accounts_for_render_cost_without_presenting_early() {
        let interval = Duration::from_millis(10);
        let mut planner = PresentationDeadlinePlanner::new(interval);
        planner.note_presented(MonotonicTimestampNs::new(100_000_000));
        let target = planner
            .plan_not_before(
                MonotonicTimestampNs::new(101_000_000),
                MonotonicTimestampNs::new(105_000_000),
                Duration::from_millis(7),
            )
            .unwrap();

        assert_eq!(target.presentation_time.get(), 110_000_000);
        assert!(target.render_start_deadline().get() <= 103_000_000);
    }

    #[test]
    fn commit_timing_past_target_is_immediately_render_eligible() {
        let mut planner = PresentationDeadlinePlanner::new(Duration::from_millis(10));
        planner.note_presented(MonotonicTimestampNs::new(100_000_000));
        let target = planner
            .plan_not_before(
                MonotonicTimestampNs::new(121_000_000),
                MonotonicTimestampNs::new(90_000_000),
                Duration::from_millis(1),
            )
            .unwrap();
        assert_eq!(target.presentation_time.get(), 130_000_000);
    }

    #[test]
    fn commit_timing_target_is_invalidated_with_the_clock_generation() {
        let mut planner = PresentationDeadlinePlanner::new(Duration::from_millis(10));
        let target = planner
            .plan_not_before(
                MonotonicTimestampNs::new(1),
                MonotonicTimestampNs::new(2),
                Duration::from_nanos(1),
            )
            .unwrap();
        planner.invalidate(Duration::from_millis(10));
        assert!(!planner.is_current(target));
    }

    #[test]
    fn commit_timing_handles_large_timestamp_without_overflow() {
        let mut planner = PresentationDeadlinePlanner::new(Duration::from_nanos(1));
        planner.note_presented(MonotonicTimestampNs::new(u64::MAX - 100));
        let target = planner
            .plan_not_before(
                MonotonicTimestampNs::new(u64::MAX - 10),
                MonotonicTimestampNs::new(u64::MAX - 5),
                Duration::from_nanos(1),
            )
            .unwrap();
        assert!(target.presentation_time.get() >= u64::MAX - 5);
    }
}
