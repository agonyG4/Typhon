use std::time::Duration;

use super::*;

/// The timestamp carried by one `wl_surface.commit`.  It remains in the
/// advertised presentation-clock domain until the native scheduler asks for
/// a monotonic wake-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) struct CommitTimingConstraint {
    seconds: u64,
    nanoseconds: u32,
}

impl CommitTimingConstraint {
    pub(in crate::compositor) fn from_protocol(seconds: u64, nanoseconds: u32) -> Option<Self> {
        if nanoseconds >= 1_000_000_000 {
            None
        } else {
            Some(Self {
                seconds,
                nanoseconds,
            })
        }
    }

    #[cfg(test)]
    pub(in crate::compositor) const fn seconds(self) -> u64 {
        self.seconds
    }

    pub(in crate::compositor) fn as_nanos(self) -> Option<u64> {
        let seconds = self.seconds.checked_mul(1_000_000_000)?;
        seconds.checked_add(self.nanoseconds as u64)
    }

    pub(in crate::compositor) fn is_due(self, clock: PresentationClock) -> bool {
        PresentationTimestamp::from_clock(clock)
            .ok()
            .and_then(PresentationTimestamp::as_nanos)
            .is_some_and(|now| self.as_nanos().is_some_and(|target| now >= target))
    }

    pub(in crate::compositor) fn monotonic_deadline_ns(self) -> Option<u64> {
        self.as_nanos()
    }

    pub(in crate::compositor) fn monotonic_deadline_from_clock_sample(
        self,
        monotonic_now: PresentationTimestamp,
        realtime_now: PresentationTimestamp,
    ) -> Option<u64> {
        let target = self.as_nanos()?;
        let monotonic_now = monotonic_now.as_nanos()?;
        let realtime_now = realtime_now.as_nanos()?;
        Some(if target <= realtime_now {
            monotonic_now
        } else {
            monotonic_now.checked_add(target - realtime_now)?
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::compositor) struct PendingSurfacePacingState {
    pub(in crate::compositor) fifo_set_barrier: bool,
    pub(in crate::compositor) fifo_wait_barrier: bool,
    pub(in crate::compositor) commit_timing: Option<CommitTimingConstraint>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::compositor) struct CapturedSurfacePacing {
    pub(in crate::compositor) fifo_set_barrier: bool,
    pub(in crate::compositor) fifo_wait_barrier: bool,
    pub(in crate::compositor) fifo_wait_ignored_for_synchronized_subsurface: bool,
    pub(in crate::compositor) commit_timing: Option<CommitTimingConstraint>,
}

impl CapturedSurfacePacing {
    pub(in crate::compositor) const fn from_pending(pending: PendingSurfacePacingState) -> Self {
        Self {
            fifo_set_barrier: pending.fifo_set_barrier,
            fifo_wait_barrier: pending.fifo_wait_barrier,
            fifo_wait_ignored_for_synchronized_subsurface: false,
            commit_timing: pending.commit_timing,
        }
    }

    pub(in crate::compositor) const fn is_boundary(self) -> bool {
        self.fifo_set_barrier
            || (self.fifo_wait_barrier && !self.fifo_wait_ignored_for_synchronized_subsurface)
            || self.commit_timing.is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(in crate::compositor) struct FifoBarrierGeneration(u64);

impl FifoBarrierGeneration {
    pub(in crate::compositor) const fn new(value: u64) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) enum FifoBarrierClearReason {
    Presented,
    LatchingDeadline,
    ForwardProgressFallback,
    SurfaceTeardown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) struct FifoBarrierClaim {
    pub(in crate::compositor) surface_id: u32,
    pub(in crate::compositor) surface_generation: u64,
    pub(in crate::compositor) fifo_barrier_generation: FifoBarrierGeneration,
    pub(in crate::compositor) commit_sequence: SurfaceCommitSequence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) struct ActiveFifoBarrier {
    pub(in crate::compositor) surface_generation: u64,
    pub(in crate::compositor) fifo_barrier_generation: FifoBarrierGeneration,
    pub(in crate::compositor) commit_sequence: SurfaceCommitSequence,
    pub(in crate::compositor) fallback_deadline_ns: u64,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SurfacePacingMetrics {
    pub barriers_captured: u64,
    pub barriers_activated: u64,
    pub waits_captured: u64,
    pub waits_blocked: u64,
    pub waits_ignored_for_synchronized_subsurfaces: u64,
    pub barriers_cleared_by_presentation: u64,
    pub barriers_cleared_by_fallback: u64,
    pub stale_barrier_clear_attempts: u64,
    pub pacing_protected_transactions: u64,
    pub timestamps_accepted: u64,
    pub past_targets: u64,
    pub future_targets: u64,
    pub timing_protocol_errors: u64,
    pub transactions_blocked_by_timing: u64,
    pub released_for_predicted_target: u64,
    pub released_by_conservative_fallback: u64,
    pub early_presentation_violations: u64,
}

pub(in crate::compositor) const FIFO_FORWARD_PROGRESS_FALLBACK: Duration =
    Duration::from_millis(34);

pub(in crate::compositor) fn fifo_forward_progress_deadline(now_ns: u64, refresh_ns: u32) -> u64 {
    let refresh_ns = u64::from(refresh_ns.max(1));
    let refresh_aware = refresh_ns.saturating_mul(5).div_ceil(4);
    now_ns.saturating_add(refresh_aware.max(FIFO_FORWARD_PROGRESS_FALLBACK.as_nanos() as u64))
}

impl CompositorState {
    pub(in crate::compositor) fn set_pending_fifo_barrier(&mut self, surface_id: u32) {
        if let Some(surface) = self.surface_resource_by_id(surface_id)
            && let Some(data) = surface.data::<SurfaceData>()
        {
            data.set_pending_fifo_barrier();
            self.surface_pacing_metrics.barriers_captured = self
                .surface_pacing_metrics
                .barriers_captured
                .saturating_add(1);
        }
    }

    pub(in crate::compositor) fn set_pending_fifo_wait(&mut self, surface_id: u32) {
        if let Some(surface) = self.surface_resource_by_id(surface_id)
            && let Some(data) = surface.data::<SurfaceData>()
        {
            data.set_pending_fifo_wait();
            self.surface_pacing_metrics.waits_captured =
                self.surface_pacing_metrics.waits_captured.saturating_add(1);
        }
    }

    pub(in crate::compositor) fn set_pending_commit_timing(
        &mut self,
        surface_id: u32,
        timestamp: CommitTimingConstraint,
    ) -> bool {
        let Some(surface) = self.surface_resource_by_id(surface_id) else {
            return false;
        };
        let Some(data) = surface.data::<SurfaceData>() else {
            return false;
        };
        if !data.set_pending_commit_timing(timestamp) {
            return false;
        }
        self.surface_pacing_metrics.timestamps_accepted = self
            .surface_pacing_metrics
            .timestamps_accepted
            .saturating_add(1);
        if timestamp.is_due(self.presentation_clock) {
            self.surface_pacing_metrics.past_targets =
                self.surface_pacing_metrics.past_targets.saturating_add(1);
        } else {
            self.surface_pacing_metrics.future_targets =
                self.surface_pacing_metrics.future_targets.saturating_add(1);
        }
        true
    }

    pub(in crate::compositor) fn remove_fifo_resource(
        &mut self,
        resource: &wp_fifo_v1::WpFifoV1,
        surface_id: u32,
    ) {
        if self
            .fifo_resources
            .get(&surface_id)
            .is_some_and(|existing| existing.id() == resource.id())
        {
            self.fifo_resources.remove(&surface_id);
        }
    }

    pub(in crate::compositor) fn remove_commit_timer_resource(
        &mut self,
        resource: &wp_commit_timer_v1::WpCommitTimerV1,
        surface_id: u32,
    ) {
        if self
            .commit_timer_resources
            .get(&surface_id)
            .is_some_and(|existing| existing.id() == resource.id())
        {
            self.commit_timer_resources.remove(&surface_id);
        }
    }

    pub(in crate::compositor) fn apply_captured_surface_pacing(
        &mut self,
        surface_id: u32,
        commit_sequence: SurfaceCommitSequence,
        pacing: CapturedSurfacePacing,
    ) {
        if pacing.is_boundary() {
            // The constraint is intentionally retained in the transaction,
            // but this metric records that it reached the ordered model.
            self.surface_pacing_metrics.pacing_protected_transactions = self
                .surface_pacing_metrics
                .pacing_protected_transactions
                .saturating_add(1);
        }
        if !pacing.fifo_set_barrier {
            return;
        }
        self.next_fifo_barrier_generation = self
            .next_fifo_barrier_generation
            .checked_add(1)
            .unwrap_or(1);
        let generation = FifoBarrierGeneration::new(self.next_fifo_barrier_generation);
        let surface_generation = self
            .surface_presentation_generations
            .get(&surface_id)
            .copied()
            .unwrap_or_default();
        let active = ActiveFifoBarrier {
            surface_generation,
            fifo_barrier_generation: generation,
            commit_sequence,
            fallback_deadline_ns: fifo_forward_progress_deadline(
                client_pacing_now_ns(),
                self.output_refresh.presentation_refresh_nsec(),
            ),
        };
        self.active_fifo_barriers.insert(surface_id, active);
        self.surface_pacing_metrics.barriers_activated = self
            .surface_pacing_metrics
            .barriers_activated
            .saturating_add(1);
    }

    pub(in crate::compositor) fn transaction_is_ready(
        &self,
        transaction: &PendingSurfaceTreeTransaction,
    ) -> bool {
        self.surface_tree_parts_ready(&transaction.nodes, &transaction.dependencies)
    }

    pub(in crate::compositor) fn surface_tree_parts_ready(
        &self,
        nodes: &[(u32, CachedSubsurfaceCommit)],
        dependencies: &[SurfaceTreeAcquireDependency],
    ) -> bool {
        if !dependencies
            .iter()
            .all(|dependency| dependency.state == PendingAcquireState::Ready)
        {
            return false;
        }
        for (surface_id, commit) in nodes {
            let effective_sync = self
                .subsurface_transactions
                .is_effectively_synchronized(*surface_id);
            if commit.pacing.fifo_wait_barrier
                && !commit.pacing.fifo_wait_ignored_for_synchronized_subsurface
                && !effective_sync
                && self.active_fifo_barriers.contains_key(surface_id)
            {
                return false;
            }
            if let Some(timestamp) = commit.pacing.commit_timing
                && !timestamp.is_due(self.presentation_clock)
            {
                return false;
            }
        }
        true
    }

    pub(in crate::compositor) fn next_surface_pacing_deadline_ns(&self) -> Option<u64> {
        let now = client_pacing_now_ns();
        let mut deadline = self
            .active_fifo_barriers
            .values()
            .map(|barrier| barrier.fallback_deadline_ns)
            .min();
        let timing_deadline = self
            .pending_surface_tree_transactions
            .iter()
            .flat_map(|transaction| transaction.nodes.iter())
            .filter_map(|(_, commit)| commit.pacing.commit_timing)
            .filter_map(|timing| match self.presentation_clock {
                PresentationClock::Monotonic => timing.monotonic_deadline_ns(),
                PresentationClock::Realtime => {
                    let mono =
                        PresentationTimestamp::from_clock(PresentationClock::Monotonic).ok()?;
                    let realtime =
                        PresentationTimestamp::from_clock(PresentationClock::Realtime).ok()?;
                    timing.monotonic_deadline_from_clock_sample(mono, realtime)
                }
            })
            .map(|target| target.max(now))
            .min();
        if let Some(target) = timing_deadline {
            deadline = Some(deadline.map_or(target, |current| current.min(target)));
        }
        deadline
    }

    pub(in crate::compositor) fn next_commit_timing_deadline_ns(&self) -> Option<u64> {
        let now = client_pacing_now_ns();
        self.pending_surface_tree_transactions
            .iter()
            .flat_map(|transaction| transaction.nodes.iter())
            .filter_map(|(_, commit)| commit.pacing.commit_timing)
            .filter_map(|timing| match self.presentation_clock {
                PresentationClock::Monotonic => timing.monotonic_deadline_ns(),
                PresentationClock::Realtime => {
                    let mono =
                        PresentationTimestamp::from_clock(PresentationClock::Monotonic).ok()?;
                    let realtime =
                        PresentationTimestamp::from_clock(PresentationClock::Realtime).ok()?;
                    timing.monotonic_deadline_from_clock_sample(mono, realtime)
                }
            })
            .map(|target| target.max(now))
            .min()
    }

    pub(in crate::compositor) fn progress_surface_pacing(&mut self, now_ns: u64) {
        let expired = self
            .active_fifo_barriers
            .iter()
            .filter_map(|(surface_id, barrier)| {
                (barrier.fallback_deadline_ns <= now_ns).then_some((*surface_id, *barrier))
            })
            .collect::<Vec<_>>();
        for (surface_id, barrier) in &expired {
            self.clear_fifo_barrier_claim(
                FifoBarrierClaim {
                    surface_id: *surface_id,
                    surface_generation: barrier.surface_generation,
                    fifo_barrier_generation: barrier.fifo_barrier_generation,
                    commit_sequence: barrier.commit_sequence,
                },
                FifoBarrierClearReason::ForwardProgressFallback,
            );
        }
        if !expired.is_empty() || !self.pending_surface_tree_transactions.is_empty() {
            self.commit_ready_surface_tree_transactions();
        }
    }

    pub(in crate::compositor) fn fifo_claims_for_frame(
        &self,
        surface_ids: impl IntoIterator<Item = u32>,
    ) -> Vec<FifoBarrierClaim> {
        surface_ids
            .into_iter()
            .filter_map(|surface_id| {
                let barrier = self.active_fifo_barriers.get(&surface_id)?;
                Some(FifoBarrierClaim {
                    surface_id,
                    surface_generation: barrier.surface_generation,
                    fifo_barrier_generation: barrier.fifo_barrier_generation,
                    commit_sequence: barrier.commit_sequence,
                })
            })
            .collect()
    }

    pub(in crate::compositor) fn clear_fifo_barrier_claim(
        &mut self,
        claim: FifoBarrierClaim,
        reason: FifoBarrierClearReason,
    ) {
        let matches = self
            .active_fifo_barriers
            .get(&claim.surface_id)
            .is_some_and(|active| {
                active.surface_generation == claim.surface_generation
                    && active.fifo_barrier_generation == claim.fifo_barrier_generation
                    && active.commit_sequence == claim.commit_sequence
            });
        if !matches {
            self.surface_pacing_metrics.stale_barrier_clear_attempts = self
                .surface_pacing_metrics
                .stale_barrier_clear_attempts
                .saturating_add(1);
            return;
        }
        self.active_fifo_barriers.remove(&claim.surface_id);
        match reason {
            FifoBarrierClearReason::Presented | FifoBarrierClearReason::LatchingDeadline => {
                self.surface_pacing_metrics.barriers_cleared_by_presentation = self
                    .surface_pacing_metrics
                    .barriers_cleared_by_presentation
                    .saturating_add(1);
            }
            FifoBarrierClearReason::ForwardProgressFallback
            | FifoBarrierClearReason::SurfaceTeardown => {
                self.surface_pacing_metrics.barriers_cleared_by_fallback = self
                    .surface_pacing_metrics
                    .barriers_cleared_by_fallback
                    .saturating_add(1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_keeps_both_seconds_words_without_truncation() {
        let constraint = CommitTimingConstraint::from_protocol(0x1234_5678_9abc_def0, 7).unwrap();
        assert_eq!(constraint.seconds(), 0x1234_5678_9abc_def0);
        assert_eq!(constraint.as_nanos(), None);
    }

    #[test]
    fn invalid_nanoseconds_are_rejected() {
        assert!(CommitTimingConstraint::from_protocol(1, 1_000_000_000).is_none());
    }

    #[test]
    fn realtime_mapping_never_wakes_before_the_target() {
        let constraint = CommitTimingConstraint::from_protocol(101, 0).unwrap();
        let monotonic = PresentationTimestamp::from_microseconds(50, 0).unwrap();
        let realtime = PresentationTimestamp::from_microseconds(100, 0).unwrap();
        assert_eq!(
            constraint.monotonic_deadline_from_clock_sample(monotonic, realtime),
            Some(51_000_000_000)
        );
    }

    #[test]
    fn fallback_is_refresh_aware_but_finite() {
        assert_eq!(fifo_forward_progress_deadline(10, 60_000_000), 75_000_010);
        assert_eq!(fifo_forward_progress_deadline(10, 6_060_606), 34_000_010);
    }
}
