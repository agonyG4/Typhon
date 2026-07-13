//! Presentation-clock identities and immutable render-start deadlines.
#![allow(dead_code)] // Wired into the native runtime in Task 12.

use std::time::Duration;

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
    Normal,
    PredictedPressure,
    ProvenReadinessMiss,
    ForcedValidation,
}

#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentationTarget {
    pub sequence: u64,
    pub presentation_time: MonotonicTimestampNs,
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

    pub const fn predicted_unreachable(self) -> bool {
        self.predicted_unreachable
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
}

impl PresentationDeadlinePlanner {
    pub fn new(refresh_interval: Duration) -> Self {
        Self {
            clock_generation: 1,
            last_presented_sequence: 0,
            last_presented_at: None,
            refresh_interval: nonzero_refresh(refresh_interval),
            scheduled: None,
        }
    }

    pub fn note_presented(&mut self, logical_sequence: u64, presented_at: MonotonicTimestampNs) {
        self.last_presented_sequence = logical_sequence;
        self.last_presented_at = Some(presented_at);
        self.scheduled = None;
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
        );
        self.scheduled = Some(target);
        Some(target)
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

    fn make_target(
        &self,
        sequence: u64,
        presentation_time: MonotonicTimestampNs,
        predicted_total_cost: Duration,
        reason: PresentationTargetReason,
        estimated: bool,
        predicted_unreachable: bool,
    ) -> PresentationTarget {
        PresentationTarget {
            sequence,
            presentation_time,
            render_start_deadline: presentation_time.saturating_sub_duration(predicted_total_cost),
            refresh_interval: self.refresh_interval,
            reason,
            clock_generation: self.clock_generation,
            estimated,
            predicted_unreachable,
        }
    }
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
        planner.note_presented(7, MonotonicTimestampNs::new(70_000_000));
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
        planner.note_presented(7, MonotonicTimestampNs::new(70_000_000));
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
        planner.note_presented(7, MonotonicTimestampNs::new(70_000_000));
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
        planner.note_presented(7, MonotonicTimestampNs::new(70_000_000));
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
}
