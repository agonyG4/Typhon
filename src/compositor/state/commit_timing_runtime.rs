use std::time::Duration;

use super::*;
use crate::native::presentation_deadline::MonotonicTimestampNs;

const COMMIT_TIMING_REEVALUATION_INTERVAL: Duration = Duration::from_secs(1);

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
    pub(super) fn is_due_at(self, now: PresentationTimestamp) -> bool {
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

impl CompositorState {
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
