use std::time::Duration;

const COMMIT_TIMING_REEVALUATION_INTERVAL: Duration = Duration::from_secs(1);

use super::*;

/// The timestamp carried by one `wl_surface.commit`.  It remains in the
/// advertised presentation-clock domain until the native scheduler asks for
/// a monotonic wake-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) struct CommitTimingConstraint {
    seconds: u64,
    nanoseconds: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) struct CommitTimingReadiness {
    pub(in crate::compositor) requested_not_before: CommitTimingConstraint,
    pub(in crate::compositor) selected_presentation_time_ns: u64,
    pub(in crate::compositor) release_for_render_at_ns: u64,
    pub(in crate::compositor) selected_sequence: u64,
    pub(in crate::compositor) clock_generation: u64,
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

    pub(in crate::compositor) fn monotonic_deadline_ns(self) -> Option<u64> {
        self.as_nanos().and_then(|nanos| u64::try_from(nanos).ok())
    }

    pub(in crate::compositor) fn monotonic_deadline_from_clock_sample(
        self,
        monotonic_now: PresentationTimestamp,
        realtime_now: PresentationTimestamp,
    ) -> Option<u64> {
        let target = self.as_nanos().expect("commit timing timestamps are u128");
        let monotonic_now = timestamp_as_nanos_u128(monotonic_now);
        let realtime_now = timestamp_as_nanos_u128(realtime_now);
        let monotonic_now_ns = u64::try_from(monotonic_now).unwrap_or(u64::MAX);
        let deadline = if target <= realtime_now {
            monotonic_now_ns
        } else {
            let delta = target - realtime_now;
            u64::try_from(monotonic_now.saturating_add(delta)).unwrap_or_else(|_| {
                monotonic_now_ns.saturating_add(
                    COMMIT_TIMING_REEVALUATION_INTERVAL
                        .as_nanos()
                        .min(u128::from(u64::MAX)) as u64,
                )
            })
        };
        Some(deadline)
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
        if let Some(requested) = transaction.commit_timing_request()
            && !transaction
                .commit_timing_readiness
                .is_some_and(|readiness| {
                    readiness.requested_not_before == requested
                        && client_pacing_now_ns() >= readiness.release_for_render_at_ns
                })
            && !requested.is_due(self.presentation_clock)
        {
            return false;
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
            .map(|timing| match self.presentation_clock {
                PresentationClock::Monotonic => {
                    timing.monotonic_deadline_ns().unwrap_or_else(|| {
                        now.saturating_add(COMMIT_TIMING_REEVALUATION_INTERVAL.as_nanos() as u64)
                    })
                }
                PresentationClock::Realtime => {
                    let deadline = PresentationTimestamp::from_clock(PresentationClock::Monotonic)
                        .ok()
                        .zip(PresentationTimestamp::from_clock(PresentationClock::Realtime).ok())
                        .and_then(|(mono, realtime)| {
                            timing.monotonic_deadline_from_clock_sample(mono, realtime)
                        });
                    deadline.unwrap_or_else(|| {
                        now.saturating_add(COMMIT_TIMING_REEVALUATION_INTERVAL.as_nanos() as u64)
                    })
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
            .map(|timing| match self.presentation_clock {
                PresentationClock::Monotonic => {
                    timing.monotonic_deadline_ns().unwrap_or_else(|| {
                        now.saturating_add(COMMIT_TIMING_REEVALUATION_INTERVAL.as_nanos() as u64)
                    })
                }
                PresentationClock::Realtime => {
                    let deadline = PresentationTimestamp::from_clock(PresentationClock::Monotonic)
                        .ok()
                        .zip(PresentationTimestamp::from_clock(PresentationClock::Realtime).ok())
                        .and_then(|(mono, realtime)| {
                            timing.monotonic_deadline_from_clock_sample(mono, realtime)
                        });
                    deadline.unwrap_or_else(|| {
                        now.saturating_add(COMMIT_TIMING_REEVALUATION_INTERVAL.as_nanos() as u64)
                    })
                }
            })
            .map(|target| target.max(now))
            .min()
    }

    pub(in crate::compositor) fn next_commit_timing_requested_ns(&self) -> Option<u64> {
        self.pending_surface_tree_transactions
            .iter()
            .find_map(PendingSurfaceTreeTransaction::commit_timing_request)
            .and_then(|timing| timing.monotonic_deadline_ns())
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
        let Some(requested_ns) = readiness.requested_not_before.monotonic_deadline_ns() else {
            return false;
        };
        if readiness.selected_presentation_time_ns < requested_ns {
            return false;
        }
        let Some(transaction) =
            self.pending_surface_tree_transactions
                .iter_mut()
                .find(|transaction| {
                    transaction.commit_timing_request() == Some(readiness.requested_not_before)
                })
        else {
            return false;
        };
        transaction.commit_timing_readiness = Some(readiness);
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
                .monotonic_deadline_from_clock_sample(monotonic, realtime)
                .is_some()
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
                root_surface_id: 10,
                nodes: vec![(10, commit)],
                dependencies: Vec::new(),
                commit_timing_readiness: None,
                received_at: Instant::now(),
            });
        let readiness = CommitTimingReadiness {
            requested_not_before: requested,
            selected_presentation_time_ns: now + 61_000_000_000,
            release_for_render_at_ns: now + 60_000_000_000,
            selected_sequence: 44,
            clock_generation: 7,
        };

        assert!(state.arm_commit_timing_target(readiness));
        assert!(!state.transaction_is_ready(&state.pending_surface_tree_transactions[0]));
        state.arm_commit_timing_target(CommitTimingReadiness {
            release_for_render_at_ns: 0,
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
            claims[0].readiness.selected_presentation_time_ns,
            now + 61_000_000_000
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
            constraint.monotonic_deadline_from_clock_sample(monotonic, realtime),
            Some(51_000_000_000)
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
