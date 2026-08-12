use std::time::Duration;

const COMMIT_TIMING_REEVALUATION_INTERVAL: Duration = Duration::from_secs(1);

use super::*;
use crate::native::presentation_deadline::MonotonicTimestampNs;

/// The timestamp carried by one `wl_surface.commit`.  It remains in the
/// advertised presentation-clock domain until the native scheduler asks for
/// a monotonic wake-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommitTimingConstraint {
    pub seconds: u64,
    pub nanoseconds: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommitTimingClockSample {
    pub monotonic_before: PresentationTimestamp,
    pub monotonic_after: PresentationTimestamp,
    pub presentation_now: PresentationTimestamp,
    pub presentation_clock: PresentationClock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommitTimingSchedulerDeadline {
    pub monotonic_not_before: MonotonicTimestampNs,
    pub recheck_at: MonotonicTimestampNs,
    pub is_representable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommitTimingClockMappingMetadata {
    pub sample: CommitTimingClockSample,
    pub monotonic_not_before: MonotonicTimestampNs,
    pub is_representable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommitTimingReadiness {
    pub transaction_id: SurfaceTreeTransactionId,
    pub requested_not_before: CommitTimingConstraint,
    pub selected_monotonic_presentation_time: MonotonicTimestampNs,
    pub release_for_render_at: MonotonicTimestampNs,
    pub selected_sequence: u64,
    pub clock_generation: u64,
    pub clock_mapping: CommitTimingClockMappingMetadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommitTimingPlanningCandidate {
    pub transaction_id: SurfaceTreeTransactionId,
    pub requested_not_before: CommitTimingConstraint,
    pub monotonic_not_before: MonotonicTimestampNs,
    pub recheck_at: MonotonicTimestampNs,
    pub clock_mapping: CommitTimingClockMappingMetadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) struct CommitTimingTargetClaim {
    pub(in crate::compositor) surface_id: u32,
    pub(in crate::compositor) surface_generation: u64,
    pub(in crate::compositor) commit_sequence: SurfaceCommitSequence,
    pub(in crate::compositor) readiness: CommitTimingReadiness,
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

    pub(in crate::compositor) const fn ordering_key(self) -> (u64, u32) {
        (self.seconds, self.nanoseconds)
    }

    pub const fn protocol_seconds(self) -> (u32, u32) {
        ((self.seconds >> 32) as u32, self.seconds as u32)
    }

    pub const fn nanoseconds(self) -> u32 {
        self.nanoseconds
    }

    pub(in crate::compositor) fn as_nanos(self) -> Option<u128> {
        Some(
            u128::from(self.seconds)
                .saturating_mul(1_000_000_000)
                .saturating_add(u128::from(self.nanoseconds)),
        )
    }

    pub(in crate::compositor) fn is_due(self, clock: PresentationClock) -> bool {
        PresentationTimestamp::from_clock(clock)
            .ok()
            .is_some_and(|now| self.is_due_at(now))
    }

    fn is_due_at(self, now: PresentationTimestamp) -> bool {
        let (now_hi, now_lo) = now.protocol_seconds();
        let (target_hi, target_lo) = ((self.seconds >> 32) as u32, self.seconds as u32);
        (now_hi, now_lo, now.nanoseconds()) >= (target_hi, target_lo, self.nanoseconds)
    }

    pub fn scheduler_deadline(
        self,
        sample: CommitTimingClockSample,
    ) -> CommitTimingSchedulerDeadline {
        let monotonic_now = timestamp_as_nanos_u128(sample.monotonic_after);
        let requested = self.as_nanos().expect("commit timing timestamps are u128");
        let mapped = match sample.presentation_clock {
            PresentationClock::Monotonic => requested,
            PresentationClock::Realtime => {
                let presentation_now = timestamp_as_nanos_u128(sample.presentation_now);
                if requested <= presentation_now {
                    monotonic_now
                } else {
                    monotonic_now.saturating_add(requested - presentation_now)
                }
            }
        };
        let monotonic_now_ns = u64::try_from(monotonic_now).unwrap_or(u64::MAX);
        let recheck_at = MonotonicTimestampNs::new(
            monotonic_now_ns.saturating_add(
                COMMIT_TIMING_REEVALUATION_INTERVAL
                    .as_nanos()
                    .min(u128::from(u64::MAX)) as u64,
            ),
        );
        let is_representable = u64::try_from(mapped).is_ok();
        CommitTimingSchedulerDeadline {
            monotonic_not_before: MonotonicTimestampNs::new(
                u64::try_from(mapped).unwrap_or(u64::MAX),
            ),
            recheck_at,
            is_representable,
        }
    }
}

fn timestamp_as_nanos_u128(timestamp: PresentationTimestamp) -> u128 {
    let (seconds_hi, seconds_lo) = timestamp.protocol_seconds();
    let seconds = (u128::from(seconds_hi) << 32) | u128::from(seconds_lo);
    seconds
        .saturating_mul(1_000_000_000)
        .saturating_add(u128::from(timestamp.nanoseconds()))
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
    pub(in crate::compositor) commit_timing_readiness: Option<CommitTimingReadiness>,
}

impl CapturedSurfacePacing {
    pub(in crate::compositor) const fn from_pending(pending: PendingSurfacePacingState) -> Self {
        Self {
            fifo_set_barrier: pending.fifo_set_barrier,
            fifo_wait_barrier: pending.fifo_wait_barrier,
            fifo_wait_ignored_for_synchronized_subsurface: false,
            commit_timing: pending.commit_timing,
            commit_timing_readiness: None,
        }
    }

    pub(in crate::compositor) const fn is_boundary(self) -> bool {
        self.fifo_set_barrier
            || (self.fifo_wait_barrier && !self.fifo_wait_ignored_for_synchronized_subsurface)
            || self.commit_timing.is_some()
    }
}

fn fifo_wait_blocks(pacing: CapturedSurfacePacing, barrier_active: bool) -> bool {
    pacing.fifo_wait_barrier
        && !pacing.fifo_wait_ignored_for_synchronized_subsurface
        && barrier_active
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
    pub queue_resource_exhaustions: u64,
    pub transactions_blocked_by_timing: u64,
    pub released_for_predicted_target: u64,
    pub released_by_conservative_fallback: u64,
    pub early_presentation_violations: u64,
    pub realtime_plans_created: u64,
    pub realtime_mappings_revalidated: u64,
    pub realtime_backward_jump_replans: u64,
    pub realtime_forward_jump_already_due_releases: u64,
    pub stale_planner_transaction_ids: u64,
    pub equal_timestamp_independent_plans: u64,
    pub pre_submit_timing_deferrals: u64,
    pub queue_admission_resource_exhaustion: u64,
    pub commit_timing_candidates_planned: u64,
    pub commit_timing_candidates_already_armed: u64,
    pub commit_timing_multi_root_plans: u64,
    pub commit_timing_frame_claims_checked: u64,
    pub commit_timing_unsampled_targets_ignored_for_submit: u64,
    pub commit_timing_realtime_conservative_samples: u64,
    pub commit_timing_realtime_resample_deferrals: u64,
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
        let surface_generation = self
            .surface_presentation_generations
            .get(&surface_id)
            .copied()
            .unwrap_or_default();
        if !pacing.fifo_set_barrier {
            if let Some(readiness) = pacing.commit_timing_readiness {
                self.active_commit_timing_targets
                    .entry(surface_id)
                    .or_default()
                    .push((surface_generation, commit_sequence, readiness));
            }
            return;
        }
        self.next_fifo_barrier_generation = self
            .next_fifo_barrier_generation
            .checked_add(1)
            .unwrap_or(1);
        let generation = FifoBarrierGeneration::new(self.next_fifo_barrier_generation);
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
        if let Some(readiness) = pacing.commit_timing_readiness {
            self.active_commit_timing_targets
                .entry(surface_id)
                .or_default()
                .push((surface_generation, commit_sequence, readiness));
        }
    }

    pub(in crate::compositor) fn transaction_is_ready(
        &self,
        transaction: &PendingSurfaceTreeTransaction,
    ) -> bool {
        if let Some(requested) = transaction.commit_timing_request() {
            if transaction
                .commit_timing_readiness
                .is_some_and(|readiness| {
                    readiness.transaction_id != transaction.id
                        || readiness.requested_not_before != requested
                })
            {
                return false;
            }
            if !requested.is_due(self.presentation_clock)
                && !transaction
                    .commit_timing_readiness
                    .is_some_and(|readiness| {
                        client_pacing_now_ns() >= readiness.release_for_render_at.get()
                            && self.commit_timing_readiness_is_safe(readiness)
                    })
            {
                return false;
            }
        }
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
            if fifo_wait_blocks(
                commit.pacing,
                self.active_fifo_barriers.contains_key(surface_id),
            ) {
                return false;
            }
        }
        true
    }

    fn current_commit_timing_clock_sample(&self) -> Option<CommitTimingClockSample> {
        let monotonic_before =
            PresentationTimestamp::from_clock(PresentationClock::Monotonic).ok()?;
        let presentation_now = PresentationTimestamp::from_clock(self.presentation_clock).ok()?;
        let monotonic_after = if self.presentation_clock == PresentationClock::Realtime {
            PresentationTimestamp::from_clock(PresentationClock::Monotonic).ok()?
        } else {
            monotonic_before
        };
        Some(CommitTimingClockSample {
            monotonic_before,
            monotonic_after,
            presentation_now,
            presentation_clock: self.presentation_clock,
        })
    }

    fn commit_timing_readiness_is_safe(&self, readiness: CommitTimingReadiness) -> bool {
        let Some(sample) = self.current_commit_timing_clock_sample() else {
            return false;
        };
        if readiness.clock_mapping.sample.presentation_clock != self.presentation_clock {
            return false;
        }
        if sample.presentation_clock == PresentationClock::Realtime {
            return matches!(
                revalidate_commit_timing_readiness(readiness, sample),
                CommitTimingRevalidation::Keep | CommitTimingRevalidation::AlreadyDue
            );
        }
        let deadline = readiness.requested_not_before.scheduler_deadline(sample);
        deadline.is_representable
            && deadline.monotonic_not_before <= readiness.selected_monotonic_presentation_time
    }

    pub(in crate::compositor) fn revalidate_pending_commit_timing_targets(&mut self) {
        let Some(sample) = self.current_commit_timing_clock_sample() else {
            return;
        };
        if sample.presentation_clock != PresentationClock::Realtime {
            return;
        }
        let mut backward_jump_replans = 0;
        let mut mappings_revalidated = 0;
        let mut forward_jump_releases = 0;
        for transaction in &mut self.pending_surface_tree_transactions {
            let Some(readiness) = transaction.commit_timing_readiness else {
                continue;
            };
            if readiness.clock_mapping.sample.presentation_clock != PresentationClock::Realtime {
                continue;
            }
            mappings_revalidated += 1;
            match revalidate_commit_timing_readiness(readiness, sample) {
                CommitTimingRevalidation::Replan => {
                    transaction.commit_timing_readiness = None;
                    backward_jump_replans += 1;
                }
                CommitTimingRevalidation::AlreadyDue => {
                    forward_jump_releases += 1;
                }
                CommitTimingRevalidation::Keep => {}
            }
        }
        self.surface_pacing_metrics.realtime_mappings_revalidated = self
            .surface_pacing_metrics
            .realtime_mappings_revalidated
            .saturating_add(mappings_revalidated);
        self.surface_pacing_metrics.realtime_backward_jump_replans = self
            .surface_pacing_metrics
            .realtime_backward_jump_replans
            .saturating_add(backward_jump_replans);
        self.surface_pacing_metrics
            .realtime_forward_jump_already_due_releases = self
            .surface_pacing_metrics
            .realtime_forward_jump_already_due_releases
            .saturating_add(forward_jump_releases);
    }

    pub(in crate::compositor) fn next_surface_pacing_deadline_ns(&self) -> Option<u64> {
        let now = client_pacing_now_ns();
        let mut deadline = self
            .active_fifo_barriers
            .values()
            .map(|barrier| barrier.fallback_deadline_ns)
            .min();
        let timing_deadline = self.next_commit_timing_deadline_ns();
        if let Some(target) = timing_deadline {
            deadline = Some(deadline.map_or(target, |current| current.min(target)));
        }
        deadline
    }

    pub(in crate::compositor) fn next_commit_timing_deadline_ns(&self) -> Option<u64> {
        let now = client_pacing_now_ns();
        let sample = self.current_commit_timing_clock_sample();
        let mut candidate_deadline = None;
        for (index, transaction) in self.pending_surface_tree_transactions.iter().enumerate() {
            if self.pending_surface_tree_transactions[..index]
                .iter()
                .any(|previous| previous.root_surface_id == transaction.root_surface_id)
            {
                continue;
            }
            let deadline = transaction
                .commit_timing_readiness
                .map(|readiness| readiness.release_for_render_at.get())
                .or_else(|| {
                    let requested = transaction.commit_timing_request()?;
                    let deadline = requested.scheduler_deadline(sample?);
                    Some(if deadline.is_representable {
                        deadline.monotonic_not_before.get()
                    } else {
                        deadline.recheck_at.get()
                    })
                });
            if let Some(deadline) = deadline {
                candidate_deadline =
                    Some(candidate_deadline.map_or(deadline, |current: u64| current.min(deadline)));
            }
        }
        let candidate_deadline = candidate_deadline?;
        Some(candidate_deadline.max(now))
    }

    pub(in crate::compositor) fn commit_timing_planning_candidates(
        &mut self,
    ) -> Vec<CommitTimingPlanningCandidate> {
        self.revalidate_pending_commit_timing_targets();
        let Some(sample) = self.current_commit_timing_clock_sample() else {
            return Vec::new();
        };
        if sample.presentation_clock == PresentationClock::Realtime {
            self.surface_pacing_metrics
                .commit_timing_realtime_conservative_samples = self
                .surface_pacing_metrics
                .commit_timing_realtime_conservative_samples
                .saturating_add(1);
        }
        let mut candidates = Vec::new();
        let mut already_armed = 0u64;
        for (index, transaction) in self.pending_surface_tree_transactions.iter().enumerate() {
            if self.pending_surface_tree_transactions[..index]
                .iter()
                .any(|previous| previous.root_surface_id == transaction.root_surface_id)
            {
                continue;
            }
            if transaction.commit_timing_readiness.is_some() {
                already_armed = already_armed.saturating_add(1);
                continue;
            }
            if let Some(candidate) =
                self.commit_timing_planning_candidate_for_id_with_sample(transaction.id, sample)
            {
                candidates.push(candidate);
            }
        }
        self.surface_pacing_metrics
            .commit_timing_candidates_already_armed = self
            .surface_pacing_metrics
            .commit_timing_candidates_already_armed
            .saturating_add(already_armed);
        if candidates.len() > 1 {
            self.surface_pacing_metrics.commit_timing_multi_root_plans = self
                .surface_pacing_metrics
                .commit_timing_multi_root_plans
                .saturating_add(1);
        }
        candidates.sort_by_key(|candidate| {
            (
                candidate.monotonic_not_before,
                candidate.transaction_id.get(),
            )
        });
        candidates
    }

    pub(in crate::compositor) fn commit_timing_planning_candidate_for_id(
        &self,
        transaction_id: SurfaceTreeTransactionId,
    ) -> Option<CommitTimingPlanningCandidate> {
        let sample = self.current_commit_timing_clock_sample()?;
        self.commit_timing_planning_candidate_for_id_with_sample(transaction_id, sample)
    }

    fn commit_timing_planning_candidate_for_id_with_sample(
        &self,
        transaction_id: SurfaceTreeTransactionId,
        sample: CommitTimingClockSample,
    ) -> Option<CommitTimingPlanningCandidate> {
        let index = self
            .pending_surface_tree_transactions
            .iter()
            .position(|transaction| transaction.id == transaction_id)?;
        let transaction = &self.pending_surface_tree_transactions[index];
        if self.pending_surface_tree_transactions[..index]
            .iter()
            .any(|previous| previous.root_surface_id == transaction.root_surface_id)
        {
            return None;
        }
        let requested_not_before = transaction.commit_timing_request()?;
        let deadline = requested_not_before.scheduler_deadline(sample);
        let clock_mapping = CommitTimingClockMappingMetadata {
            sample,
            monotonic_not_before: deadline.monotonic_not_before,
            is_representable: deadline.is_representable,
        };
        Some(CommitTimingPlanningCandidate {
            transaction_id,
            requested_not_before,
            monotonic_not_before: deadline.monotonic_not_before,
            recheck_at: deadline.recheck_at,
            clock_mapping,
        })
    }

    pub(in crate::compositor) fn has_pending_commit_timing(&self) -> bool {
        self.pending_surface_tree_transactions
            .iter()
            .any(|transaction| transaction.commit_timing_request().is_some())
    }

    pub(in crate::compositor) fn arm_commit_timing_target(
        &mut self,
        readiness: CommitTimingReadiness,
    ) -> bool {
        if readiness.clock_mapping.sample.presentation_clock != self.presentation_clock
            || (readiness.clock_mapping.is_representable
                && readiness.selected_monotonic_presentation_time
                    < readiness.clock_mapping.monotonic_not_before)
        {
            return false;
        }
        let Some(transaction_index) = self
            .pending_surface_tree_transactions
            .iter()
            .position(|transaction| transaction.id == readiness.transaction_id)
        else {
            self.surface_pacing_metrics.stale_planner_transaction_ids = self
                .surface_pacing_metrics
                .stale_planner_transaction_ids
                .saturating_add(1);
            return false;
        };
        let root_surface_id =
            self.pending_surface_tree_transactions[transaction_index].root_surface_id;
        if self.pending_surface_tree_transactions[..transaction_index]
            .iter()
            .any(|previous| previous.root_surface_id == root_surface_id)
        {
            self.surface_pacing_metrics.stale_planner_transaction_ids = self
                .surface_pacing_metrics
                .stale_planner_transaction_ids
                .saturating_add(1);
            return false;
        }
        if self.pending_surface_tree_transactions[transaction_index].commit_timing_request()
            != Some(readiness.requested_not_before)
        {
            self.surface_pacing_metrics.stale_planner_transaction_ids = self
                .surface_pacing_metrics
                .stale_planner_transaction_ids
                .saturating_add(1);
            return false;
        }
        let has_independent_equal_timestamp = self
            .pending_surface_tree_transactions
            .iter()
            .enumerate()
            .any(|(index, transaction)| {
                index != transaction_index
                    && transaction.root_surface_id != root_surface_id
                    && transaction.commit_timing_request() == Some(readiness.requested_not_before)
            });
        if has_independent_equal_timestamp {
            self.surface_pacing_metrics
                .equal_timestamp_independent_plans = self
                .surface_pacing_metrics
                .equal_timestamp_independent_plans
                .saturating_add(1);
        }
        if readiness.clock_mapping.sample.presentation_clock == PresentationClock::Realtime {
            self.surface_pacing_metrics.realtime_plans_created = self
                .surface_pacing_metrics
                .realtime_plans_created
                .saturating_add(1);
        }
        self.pending_surface_tree_transactions[transaction_index].commit_timing_readiness =
            Some(readiness);
        self.surface_pacing_metrics.commit_timing_candidates_planned = self
            .surface_pacing_metrics
            .commit_timing_candidates_planned
            .saturating_add(1);
        true
    }

    pub(in crate::compositor) fn invalidate_pending_commit_timing_targets(&mut self) {
        for transaction in &mut self.pending_surface_tree_transactions {
            transaction.commit_timing_readiness = None;
        }
    }

    pub(in crate::compositor) fn commit_timing_claims_for_frame(
        &self,
        surface_ids: impl IntoIterator<Item = u32>,
    ) -> Vec<CommitTimingTargetClaim> {
        surface_ids
            .into_iter()
            .flat_map(|surface_id| {
                self.active_commit_timing_targets
                    .get(&surface_id)
                    .into_iter()
                    .flat_map(|claims| claims.iter())
                    .map(move |(surface_generation, commit_sequence, readiness)| {
                        CommitTimingTargetClaim {
                            surface_id,
                            surface_generation: *surface_generation,
                            commit_sequence: *commit_sequence,
                            readiness: *readiness,
                        }
                    })
            })
            .collect()
    }

    pub(in crate::compositor) fn commit_timing_submission_is_safe_for_batch(
        &mut self,
        batch_id: CompositorFrameBatchId,
        planned_monotonic_presentation_time: MonotonicTimestampNs,
        clock_generation: u64,
    ) -> bool {
        let Some(claims) = self
            .frame_batches
            .get(&batch_id)
            .map(|batch| batch.commit_timing_target_claims.clone())
        else {
            return false;
        };
        if claims.is_empty() {
            let unsampled = self
                .active_commit_timing_targets
                .values()
                .map(Vec::len)
                .sum::<usize>();
            self.surface_pacing_metrics
                .commit_timing_unsampled_targets_ignored_for_submit = self
                .surface_pacing_metrics
                .commit_timing_unsampled_targets_ignored_for_submit
                .saturating_add(unsampled as u64);
            return true;
        }
        let Some(sample) = self.current_commit_timing_clock_sample() else {
            return false;
        };
        self.surface_pacing_metrics
            .commit_timing_frame_claims_checked = self
            .surface_pacing_metrics
            .commit_timing_frame_claims_checked
            .saturating_add(claims.len() as u64);
        if sample.presentation_clock == PresentationClock::Realtime {
            self.surface_pacing_metrics
                .commit_timing_realtime_conservative_samples = self
                .surface_pacing_metrics
                .commit_timing_realtime_conservative_samples
                .saturating_add(1);
        }
        let current_monotonic = MonotonicTimestampNs::new(
            u64::try_from(timestamp_as_nanos_u128(sample.monotonic_after)).unwrap_or(u64::MAX),
        );
        let effective_monotonic_presentation_time =
            planned_monotonic_presentation_time.max(current_monotonic);
        let mut realtime_deferral = false;
        let safe = claims.iter().all(|claim| {
            let readiness = claim.readiness;
            if readiness.clock_generation != clock_generation
                || readiness.clock_mapping.sample.presentation_clock != self.presentation_clock
                || effective_monotonic_presentation_time
                    < readiness.selected_monotonic_presentation_time
            {
                realtime_deferral |= sample.presentation_clock == PresentationClock::Realtime;
                return false;
            }
            let deadline = readiness.requested_not_before.scheduler_deadline(sample);
            let claim_safe = deadline.is_representable
                && effective_monotonic_presentation_time >= deadline.monotonic_not_before;
            realtime_deferral |=
                !claim_safe && sample.presentation_clock == PresentationClock::Realtime;
            claim_safe
        });
        if !safe {
            self.surface_pacing_metrics.pre_submit_timing_deferrals = self
                .surface_pacing_metrics
                .pre_submit_timing_deferrals
                .saturating_add(1);
        }
        if realtime_deferral {
            self.surface_pacing_metrics
                .commit_timing_realtime_resample_deferrals = self
                .surface_pacing_metrics
                .commit_timing_realtime_resample_deferrals
                .saturating_add(1);
        }
        safe
    }

    pub(in crate::compositor) fn complete_commit_timing_claim(
        &mut self,
        claim: CommitTimingTargetClaim,
        presentation: FramePresentation,
    ) {
        let comparable = presentation.clock == self.presentation_clock;
        if comparable
            && !claim
                .readiness
                .requested_not_before
                .is_due_at(presentation.timestamp)
        {
            self.surface_pacing_metrics.early_presentation_violations = self
                .surface_pacing_metrics
                .early_presentation_violations
                .saturating_add(1);
            let (actual_seconds_hi, actual_seconds_lo) = presentation.timestamp.protocol_seconds();
            let (requested_seconds_hi, requested_seconds_lo) =
                claim.readiness.requested_not_before.protocol_seconds();
            client_pacing_log(
                "early_commit_timing_presentation",
                &[
                    (
                        "transaction_id",
                        claim.readiness.transaction_id.get().to_string(),
                    ),
                    ("requested_seconds_hi", requested_seconds_hi.to_string()),
                    ("requested_seconds_lo", requested_seconds_lo.to_string()),
                    (
                        "requested_nanoseconds",
                        claim.readiness.requested_not_before.nanoseconds.to_string(),
                    ),
                    ("presentation_clock", format!("{:?}", presentation.clock)),
                    (
                        "selected_monotonic_presentation_time",
                        claim
                            .readiness
                            .selected_monotonic_presentation_time
                            .get()
                            .to_string(),
                    ),
                    ("actual_seconds_hi", actual_seconds_hi.to_string()),
                    ("actual_seconds_lo", actual_seconds_lo.to_string()),
                    (
                        "actual_nanoseconds",
                        presentation.timestamp.nanoseconds().to_string(),
                    ),
                    (
                        "clock_generation",
                        claim.readiness.clock_generation.to_string(),
                    ),
                ],
            );
            debug_assert!(
                claim
                    .readiness
                    .requested_not_before
                    .is_due_at(presentation.timestamp),
                "Commit Timing frame was presented before its requested timestamp"
            );
        }
        self.discard_commit_timing_claim(claim);
    }

    pub(in crate::compositor) fn discard_commit_timing_claim(
        &mut self,
        claim: CommitTimingTargetClaim,
    ) {
        let mut remove_surface_entry = false;
        if let Some(active) = self.active_commit_timing_targets.get_mut(&claim.surface_id)
            && let Some(index) =
                active
                    .iter()
                    .position(|(surface_generation, commit_sequence, readiness)| {
                        *surface_generation == claim.surface_generation
                            && *commit_sequence == claim.commit_sequence
                            && *readiness == claim.readiness
                    })
        {
            active.remove(index);
            remove_surface_entry = active.is_empty();
        }
        if remove_surface_entry {
            self.active_commit_timing_targets.remove(&claim.surface_id);
        }
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
            FifoBarrierClearReason::Presented => {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommitTimingRevalidation {
    Keep,
    Replan,
    AlreadyDue,
}

fn revalidate_commit_timing_readiness(
    readiness: CommitTimingReadiness,
    sample: CommitTimingClockSample,
) -> CommitTimingRevalidation {
    if sample.presentation_clock != PresentationClock::Realtime
        || readiness.clock_mapping.sample.presentation_clock != PresentationClock::Realtime
    {
        return CommitTimingRevalidation::Keep;
    }
    let deadline = readiness.requested_not_before.scheduler_deadline(sample);
    if !deadline.is_representable
        || deadline.monotonic_not_before > readiness.selected_monotonic_presentation_time
    {
        return CommitTimingRevalidation::Replan;
    }
    if readiness
        .requested_not_before
        .is_due_at(sample.presentation_now)
        && !readiness
            .requested_not_before
            .is_due_at(readiness.clock_mapping.sample.presentation_now)
    {
        return CommitTimingRevalidation::AlreadyDue;
    }
    CommitTimingRevalidation::Keep
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_keeps_both_seconds_words_without_truncation() {
        let constraint = CommitTimingConstraint::from_protocol(0x1234_5678_9abc_def0, 7).unwrap();
        assert_eq!(constraint.seconds(), 0x1234_5678_9abc_def0);
        assert!(constraint.as_nanos().is_some());
    }

    #[test]
    fn maximum_protocol_timestamp_remains_representable_for_re_evaluation() {
        let constraint = CommitTimingConstraint::from_protocol(u64::MAX, 999_999_999).unwrap();
        assert!(constraint.as_nanos().is_some());
        let monotonic = PresentationTimestamp::from_microseconds(1, 0).unwrap();
        let realtime = PresentationTimestamp::from_microseconds(0, 0).unwrap();
        assert!(
            constraint
                .scheduler_deadline(CommitTimingClockSample {
                    monotonic_before: monotonic,
                    monotonic_after: monotonic,
                    presentation_now: realtime,
                    presentation_clock: PresentationClock::Realtime,
                })
                .is_representable
        );
    }

    #[test]
    fn selected_timing_target_releases_before_presentation_time_and_remains_owned() {
        let mut state = CompositorState::default();
        let now = client_pacing_now_ns();
        let seconds = now / 1_000_000_000 + 60;
        let requested = CommitTimingConstraint::from_protocol(seconds, 0).unwrap();
        let mut commit = empty_cached_subsurface_commit();
        commit.pacing.commit_timing = Some(requested);
        state
            .pending_surface_tree_transactions
            .push(PendingSurfaceTreeTransaction {
                id: SurfaceTreeTransactionId::new(1),
                root_surface_id: 10,
                nodes: vec![(10, commit)],
                dependencies: Vec::new(),
                commit_timing_readiness: None,
                received_at: Instant::now(),
            });
        let candidate = state
            .commit_timing_planning_candidates()
            .into_iter()
            .next()
            .expect("timed transaction should produce a planning candidate");
        let readiness = CommitTimingReadiness {
            transaction_id: SurfaceTreeTransactionId::new(1),
            requested_not_before: requested,
            selected_monotonic_presentation_time: MonotonicTimestampNs::new(now + 61_000_000_000),
            release_for_render_at: MonotonicTimestampNs::new(now + 60_000_000_000),
            selected_sequence: 44,
            clock_generation: 7,
            clock_mapping: candidate.clock_mapping,
        };

        assert!(state.arm_commit_timing_target(readiness));
        assert!(!state.transaction_is_ready(&state.pending_surface_tree_transactions[0]));
        state.arm_commit_timing_target(CommitTimingReadiness {
            release_for_render_at: MonotonicTimestampNs::new(0),
            ..readiness
        });
        assert!(state.transaction_is_ready(&state.pending_surface_tree_transactions[0]));

        let pacing = CapturedSurfacePacing {
            commit_timing: Some(requested),
            commit_timing_readiness: Some(readiness),
            ..CapturedSurfacePacing::default()
        };
        state.apply_captured_surface_pacing(10, SurfaceCommitSequence::initial(), pacing);
        let claims = state.commit_timing_claims_for_frame([10]);
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].readiness.selected_sequence, 44);
        assert_eq!(
            claims[0].readiness.selected_monotonic_presentation_time,
            MonotonicTimestampNs::new(now + 61_000_000_000)
        );
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
            constraint
                .scheduler_deadline(CommitTimingClockSample {
                    monotonic_before: monotonic,
                    monotonic_after: monotonic,
                    presentation_now: realtime,
                    presentation_clock: PresentationClock::Realtime,
                })
                .monotonic_not_before
                .get(),
            51_000_000_000
        );
    }

    #[test]
    fn realtime_clock_sample_maps_future_protocol_time_into_monotonic_scheduler_time() {
        let constraint = CommitTimingConstraint::from_protocol(1000, 50_000_000).unwrap();
        let deadline = constraint.scheduler_deadline(CommitTimingClockSample {
            monotonic_before: PresentationTimestamp::from_microseconds(100, 0).unwrap(),
            monotonic_after: PresentationTimestamp::from_microseconds(100, 0).unwrap(),
            presentation_now: PresentationTimestamp::from_microseconds(1000, 0).unwrap(),
            presentation_clock: PresentationClock::Realtime,
        });

        assert_eq!(deadline.monotonic_not_before.get(), 100_050_000_000);
        assert!(deadline.is_representable);
    }

    #[test]
    fn realtime_mapping_uses_the_post_sample_monotonic_boundary() {
        let constraint = CommitTimingConstraint::from_protocol(1000, 16_667_000).unwrap();
        let monotonic_before = PresentationTimestamp::from_microseconds(100, 0).unwrap();
        let monotonic_after = PresentationTimestamp::from_microseconds(100, 100).unwrap();
        let deadline = constraint.scheduler_deadline(CommitTimingClockSample {
            monotonic_before,
            monotonic_after,
            presentation_now: PresentationTimestamp::from_microseconds(1000, 50).unwrap(),
            presentation_clock: PresentationClock::Realtime,
        });

        assert_eq!(deadline.monotonic_not_before.get(), 100_016_717_000);
        assert!(deadline.monotonic_not_before.get() > 100_016_617_000);
    }

    #[test]
    fn realtime_mapping_rounds_a_target_up_to_the_next_refresh_boundary() {
        let requested = CommitTimingConstraint::from_protocol(1000, 16_667_000).unwrap();
        let sample = CommitTimingClockSample {
            monotonic_before: PresentationTimestamp::from_microseconds(100, 0).unwrap(),
            monotonic_after: PresentationTimestamp::from_microseconds(100, 100).unwrap(),
            presentation_now: PresentationTimestamp::from_microseconds(1000, 50).unwrap(),
            presentation_clock: PresentationClock::Realtime,
        };
        let deadline = requested.scheduler_deadline(sample);
        let mut planner = crate::native::presentation_deadline::PresentationDeadlinePlanner::new(
            Duration::from_nanos(16_666_667),
        );
        planner.note_presented(MonotonicTimestampNs::new(100_000_000_000));
        let target = planner
            .plan_not_before(
                MonotonicTimestampNs::new(100_000_000_000),
                deadline.monotonic_not_before,
                Duration::ZERO,
            )
            .unwrap();

        assert!(target.presentation_time >= deadline.monotonic_not_before);
        assert_eq!(target.sequence, 2);
    }

    #[test]
    fn realtime_clock_sample_treats_already_due_target_as_immediate() {
        let constraint = CommitTimingConstraint::from_protocol(999, 999_999_999).unwrap();
        let deadline = constraint.scheduler_deadline(CommitTimingClockSample {
            monotonic_before: PresentationTimestamp::from_microseconds(100, 0).unwrap(),
            monotonic_after: PresentationTimestamp::from_microseconds(100, 0).unwrap(),
            presentation_now: PresentationTimestamp::from_microseconds(1000, 0).unwrap(),
            presentation_clock: PresentationClock::Realtime,
        });

        assert_eq!(deadline.monotonic_not_before.get(), 100_000_000_000);
    }

    #[test]
    fn monotonic_clock_sample_keeps_the_protocol_timestamp_in_scheduler_domain() {
        let constraint = CommitTimingConstraint::from_protocol(101, 50_000_000).unwrap();
        let deadline = constraint.scheduler_deadline(CommitTimingClockSample {
            monotonic_before: PresentationTimestamp::from_microseconds(100, 0).unwrap(),
            monotonic_after: PresentationTimestamp::from_microseconds(100, 0).unwrap(),
            presentation_now: PresentationTimestamp::from_microseconds(100, 0).unwrap(),
            presentation_clock: PresentationClock::Monotonic,
        });

        assert_eq!(deadline.monotonic_not_before.get(), 101_050_000_000);
        assert!(deadline.is_representable);
    }

    #[test]
    fn realtime_backward_jump_invalidates_an_old_selected_target() {
        let requested = CommitTimingConstraint::from_protocol(1001, 0).unwrap();
        let initial_sample = CommitTimingClockSample {
            monotonic_before: PresentationTimestamp::from_microseconds(100, 0).unwrap(),
            monotonic_after: PresentationTimestamp::from_microseconds(100, 0).unwrap(),
            presentation_now: PresentationTimestamp::from_microseconds(1000, 0).unwrap(),
            presentation_clock: PresentationClock::Realtime,
        };
        let initial_deadline = requested.scheduler_deadline(initial_sample);
        let readiness = CommitTimingReadiness {
            transaction_id: SurfaceTreeTransactionId::new(1),
            requested_not_before: requested,
            selected_monotonic_presentation_time: initial_deadline.monotonic_not_before,
            release_for_render_at: MonotonicTimestampNs::new(100_500_000_000),
            selected_sequence: 1,
            clock_generation: 1,
            clock_mapping: CommitTimingClockMappingMetadata {
                sample: initial_sample,
                monotonic_not_before: initial_deadline.monotonic_not_before,
                is_representable: initial_deadline.is_representable,
            },
        };
        let backward_sample = CommitTimingClockSample {
            monotonic_before: PresentationTimestamp::from_microseconds(100, 500_000).unwrap(),
            monotonic_after: PresentationTimestamp::from_microseconds(100, 500_000).unwrap(),
            presentation_now: PresentationTimestamp::from_microseconds(999, 500_000).unwrap(),
            presentation_clock: PresentationClock::Realtime,
        };

        assert_eq!(
            revalidate_commit_timing_readiness(readiness, backward_sample),
            CommitTimingRevalidation::Replan
        );
    }

    #[test]
    fn realtime_forward_jump_makes_the_original_constraint_already_due() {
        let requested = CommitTimingConstraint::from_protocol(1001, 0).unwrap();
        let initial_sample = CommitTimingClockSample {
            monotonic_before: PresentationTimestamp::from_microseconds(100, 0).unwrap(),
            monotonic_after: PresentationTimestamp::from_microseconds(100, 0).unwrap(),
            presentation_now: PresentationTimestamp::from_microseconds(1000, 0).unwrap(),
            presentation_clock: PresentationClock::Realtime,
        };
        let initial_deadline = requested.scheduler_deadline(initial_sample);
        let readiness = CommitTimingReadiness {
            transaction_id: SurfaceTreeTransactionId::new(1),
            requested_not_before: requested,
            selected_monotonic_presentation_time: initial_deadline.monotonic_not_before,
            release_for_render_at: MonotonicTimestampNs::new(100_500_000_000),
            selected_sequence: 1,
            clock_generation: 1,
            clock_mapping: CommitTimingClockMappingMetadata {
                sample: initial_sample,
                monotonic_not_before: initial_deadline.monotonic_not_before,
                is_representable: initial_deadline.is_representable,
            },
        };
        let forward_sample = CommitTimingClockSample {
            monotonic_before: PresentationTimestamp::from_microseconds(100, 500_000).unwrap(),
            monotonic_after: PresentationTimestamp::from_microseconds(100, 500_000).unwrap(),
            presentation_now: PresentationTimestamp::from_microseconds(1002, 0).unwrap(),
            presentation_clock: PresentationClock::Realtime,
        };

        assert_eq!(
            revalidate_commit_timing_readiness(readiness, forward_sample),
            CommitTimingRevalidation::AlreadyDue
        );
    }

    #[test]
    fn repeated_realtime_backward_jumps_replan_with_finite_progress() {
        let requested = CommitTimingConstraint::from_protocol(1001, 0).unwrap();
        let initial_sample = CommitTimingClockSample {
            monotonic_before: PresentationTimestamp::from_microseconds(100, 0).unwrap(),
            monotonic_after: PresentationTimestamp::from_microseconds(100, 0).unwrap(),
            presentation_now: PresentationTimestamp::from_microseconds(1000, 0).unwrap(),
            presentation_clock: PresentationClock::Realtime,
        };
        let initial_deadline = requested.scheduler_deadline(initial_sample);
        let mut readiness = CommitTimingReadiness {
            transaction_id: SurfaceTreeTransactionId::new(1),
            requested_not_before: requested,
            selected_monotonic_presentation_time: initial_deadline.monotonic_not_before,
            release_for_render_at: MonotonicTimestampNs::new(100_500_000_000),
            selected_sequence: 1,
            clock_generation: 1,
            clock_mapping: CommitTimingClockMappingMetadata {
                sample: initial_sample,
                monotonic_not_before: initial_deadline.monotonic_not_before,
                is_representable: initial_deadline.is_representable,
            },
        };
        for (monotonic_seconds, realtime_seconds) in [(100, 999), (101, 998)] {
            let sample = CommitTimingClockSample {
                monotonic_before: PresentationTimestamp::from_microseconds(monotonic_seconds, 0)
                    .unwrap(),
                monotonic_after: PresentationTimestamp::from_microseconds(monotonic_seconds, 0)
                    .unwrap(),
                presentation_now: PresentationTimestamp::from_microseconds(realtime_seconds, 0)
                    .unwrap(),
                presentation_clock: PresentationClock::Realtime,
            };
            assert_eq!(
                revalidate_commit_timing_readiness(readiness, sample),
                CommitTimingRevalidation::Replan
            );
            let deadline = requested.scheduler_deadline(sample);
            assert!(deadline.is_representable);
            readiness.selected_monotonic_presentation_time = deadline.monotonic_not_before;
            readiness.clock_mapping = CommitTimingClockMappingMetadata {
                sample,
                monotonic_not_before: deadline.monotonic_not_before,
                is_representable: deadline.is_representable,
            };
        }
    }

    #[test]
    fn maximum_realtime_protocol_target_keeps_a_finite_recheck_deadline() {
        let constraint = CommitTimingConstraint::from_protocol(u64::MAX, 999_999_999).unwrap();
        let deadline = constraint.scheduler_deadline(CommitTimingClockSample {
            monotonic_before: PresentationTimestamp::from_microseconds(100, 0).unwrap(),
            monotonic_after: PresentationTimestamp::from_microseconds(100, 0).unwrap(),
            presentation_now: PresentationTimestamp::from_microseconds(1000, 0).unwrap(),
            presentation_clock: PresentationClock::Realtime,
        });

        assert!(!deadline.is_representable);
        assert!(deadline.recheck_at.get() > 100_000_000_000);
    }

    #[test]
    fn equal_timestamps_in_independent_root_heads_are_all_planned() {
        let mut state = CompositorState::default();
        let requested = CommitTimingConstraint::from_protocol(1, 0).unwrap();
        let mut first = empty_cached_subsurface_commit();
        first.pacing.commit_timing = Some(requested);
        let mut second = empty_cached_subsurface_commit();
        second.pacing.commit_timing = Some(requested);
        let first_id = state.allocate_surface_tree_transaction_id();
        let second_id = state.allocate_surface_tree_transaction_id();
        state.pending_surface_tree_transactions.extend([
            PendingSurfaceTreeTransaction {
                id: first_id,
                root_surface_id: 1,
                nodes: vec![(1, first)],
                dependencies: Vec::new(),
                commit_timing_readiness: None,
                received_at: Instant::now(),
            },
            PendingSurfaceTreeTransaction {
                id: second_id,
                root_surface_id: 2,
                nodes: vec![(2, second)],
                dependencies: Vec::new(),
                commit_timing_readiness: None,
                received_at: Instant::now(),
            },
        ]);

        let candidates = state.commit_timing_planning_candidates();
        assert_eq!(candidates.len(), 2);
        assert_eq!(
            candidates
                .iter()
                .map(|candidate| candidate.transaction_id)
                .collect::<Vec<_>>(),
            vec![first_id, second_id]
        );
        for candidate in candidates {
            assert!(state.arm_commit_timing_target(CommitTimingReadiness {
                transaction_id: candidate.transaction_id,
                requested_not_before: requested,
                selected_monotonic_presentation_time: candidate.monotonic_not_before,
                release_for_render_at: MonotonicTimestampNs::new(0),
                selected_sequence: 1,
                clock_generation: 1,
                clock_mapping: candidate.clock_mapping,
            }));
        }
        assert_eq!(
            state
                .surface_pacing_metrics
                .equal_timestamp_independent_plans,
            2
        );
        assert!(
            state
                .pending_surface_tree_transactions
                .iter()
                .all(|transaction| { transaction.commit_timing_readiness.is_some() })
        );
        assert!(state.commit_timing_planning_candidates().is_empty());
        state.invalidate_pending_commit_timing_targets();
        assert_eq!(state.commit_timing_planning_candidates().len(), 2);
    }

    #[test]
    fn same_root_equal_timestamp_keeps_the_second_transaction_ordered() {
        let mut state = CompositorState::default();
        let requested = CommitTimingConstraint::from_protocol(1, 0).unwrap();
        let mut first = empty_cached_subsurface_commit();
        first.pacing.commit_timing = Some(requested);
        let mut second = empty_cached_subsurface_commit();
        second.pacing.commit_timing = Some(requested);
        let first_id = state.allocate_surface_tree_transaction_id();
        let second_id = state.allocate_surface_tree_transaction_id();
        state.pending_surface_tree_transactions.extend([
            PendingSurfaceTreeTransaction {
                id: first_id,
                root_surface_id: 4,
                nodes: vec![(4, first)],
                dependencies: Vec::new(),
                commit_timing_readiness: None,
                received_at: Instant::now(),
            },
            PendingSurfaceTreeTransaction {
                id: second_id,
                root_surface_id: 4,
                nodes: vec![(4, second)],
                dependencies: Vec::new(),
                commit_timing_readiness: None,
                received_at: Instant::now(),
            },
        ]);

        assert_eq!(
            state
                .commit_timing_planning_candidates()
                .into_iter()
                .map(|candidate| candidate.transaction_id)
                .collect::<Vec<_>>(),
            vec![first_id]
        );
        assert!(
            state
                .commit_timing_planning_candidate_for_id(second_id)
                .is_none()
        );
        state.pending_surface_tree_transactions.remove(0);
        assert_eq!(
            state
                .commit_timing_planning_candidates()
                .into_iter()
                .map(|candidate| candidate.transaction_id)
                .collect::<Vec<_>>(),
            vec![second_id]
        );
    }

    #[test]
    fn submission_safety_is_scoped_to_the_prepared_frame_batch() {
        let mut state = CompositorState::default();
        let sample = state
            .current_commit_timing_clock_sample()
            .expect("test clock sample should be available");
        let readiness = CommitTimingReadiness {
            transaction_id: SurfaceTreeTransactionId::new(1),
            requested_not_before: CommitTimingConstraint::from_protocol(0, 0).unwrap(),
            selected_monotonic_presentation_time: MonotonicTimestampNs::new(u64::MAX),
            release_for_render_at: MonotonicTimestampNs::new(u64::MAX),
            selected_sequence: 1,
            clock_generation: 1,
            clock_mapping: CommitTimingClockMappingMetadata {
                sample,
                monotonic_not_before: MonotonicTimestampNs::new(0),
                is_representable: true,
            },
        };
        let claim = CommitTimingTargetClaim {
            surface_id: 10,
            surface_generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            readiness,
        };
        let blocked_batch = CompositorFrameBatchId::new(
            std::num::NonZeroU64::new(1).expect("test batch ID is nonzero"),
        );
        let unrelated_batch = CompositorFrameBatchId::new(
            std::num::NonZeroU64::new(2).expect("test batch ID is nonzero"),
        );
        let empty_batch = |frame_id, claims| CompositorFrameBatch {
            frame_id,
            callbacks: Vec::new(),
            callback_commit_ns: None,
            callback_render_completed_ns: None,
            callback_settlement: FrameCallbackSettlement::default(),
            callback_terminal_ownership_checked: false,
            presentation_feedbacks: Vec::new(),
            shm_buffer_releases: Vec::new(),
            dmabuf_releases_to_complete_on_present: Vec::new(),
            fifo_barrier_claims: Vec::new(),
            commit_timing_target_claims: claims,
        };
        state
            .frame_batches
            .insert(blocked_batch, empty_batch(1, vec![claim]));
        state
            .frame_batches
            .insert(unrelated_batch, empty_batch(2, Vec::new()));

        assert!(!state.commit_timing_submission_is_safe_for_batch(
            blocked_batch,
            MonotonicTimestampNs::new(0),
            1,
        ));
        assert!(state.commit_timing_submission_is_safe_for_batch(
            unrelated_batch,
            MonotonicTimestampNs::new(0),
            1,
        ));
    }

    #[test]
    fn non_evictable_queue_rejects_an_ordinary_incoming_transaction() {
        let mut state = CompositorState::default();
        let requested = CommitTimingConstraint::from_protocol(
            client_pacing_now_ns() / 1_000_000_000 + 3_600,
            0,
        )
        .unwrap();
        for index in 0..8 {
            let mut commit = empty_cached_subsurface_commit();
            commit.pacing.commit_timing = Some(requested);
            state
                .pending_surface_tree_transactions
                .push(PendingSurfaceTreeTransaction {
                    id: SurfaceTreeTransactionId::new(index as u64 + 1),
                    root_surface_id: 9,
                    nodes: vec![(9, commit)],
                    dependencies: Vec::new(),
                    commit_timing_readiness: None,
                    received_at: Instant::now(),
                });
        }

        state.queue_waiting_surface_tree(
            9,
            vec![(9, empty_cached_subsurface_commit())],
            Vec::new(),
        );

        assert_eq!(state.pending_surface_tree_transactions.len(), 8);
        assert_eq!(state.take_client_resource_exhaustions(), vec![9]);
        assert_eq!(
            state
                .surface_pacing_metrics
                .queue_admission_resource_exhaustion,
            1
        );
    }

    #[test]
    fn fallback_is_refresh_aware_but_finite() {
        assert_eq!(fifo_forward_progress_deadline(10, 60_000_000), 75_000_010);
        assert_eq!(fifo_forward_progress_deadline(10, 6_060_606), 34_000_010);
    }

    #[test]
    fn synchronized_wait_capture_remains_authoritative_after_mode_changes() {
        let ignored_at_commit = CapturedSurfacePacing {
            fifo_wait_barrier: true,
            fifo_wait_ignored_for_synchronized_subsurface: true,
            ..CapturedSurfacePacing::default()
        };
        let applied_at_commit = CapturedSurfacePacing {
            fifo_wait_barrier: true,
            fifo_wait_ignored_for_synchronized_subsurface: false,
            ..CapturedSurfacePacing::default()
        };

        assert!(!fifo_wait_blocks(ignored_at_commit, true));
        assert!(fifo_wait_blocks(applied_at_commit, true));
    }
}
