use super::frame::{
    NativeCursorOutputArbitration, NativeCursorOutputDisposition, NativeCursorPrimaryState,
    NativeCursorSchedulingPolicy,
};
use super::planner::CursorServiceOpportunity;
use crate::native_output::presentation::trace::{
    CursorEpochPresentationTrace, PresentationTransactionTraceRing,
};
use crate::native_output::presentation::transaction::OutputTransactionId;
use oblivion_one::compositor::PointerInputMetricsSnapshot;
use oblivion_one::core::OutputId;
use std::{
    collections::{HashMap, VecDeque},
    fmt::Write as _,
    path::PathBuf,
};

const DEFAULT_CAPACITY: usize = 32_768;
const MAX_CAPACITY: usize = 65_536;
const DEFAULT_SAMPLE_STRIDE: u64 = 16;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CursorHitTestCounters {
    pub(crate) calls: u64,
    pub(crate) cache_hits: u64,
    pub(crate) cache_misses: u64,
    pub(crate) full_scene_scans: u64,
    pub(crate) owner_locality_fast_hits: u64,
    pub(crate) certificate_hits: u64,
    pub(crate) certificate_misses: u64,
    pub(crate) certificate_builds: u64,
    pub(crate) certificate_rejections: u64,
    pub(crate) groups_inspected: u64,
    pub(crate) surfaces_inspected: u64,
    pub(crate) cpu_ns: u64,
    pub(crate) pointer_hit_generation_invalidations: u64,
    pub(crate) interaction_topology_invalidations: u64,
    pub(crate) visual_stack_groups_cache_rebuilds: u64,
}

impl CursorHitTestCounters {
    fn delta_since(self, before: Self) -> Self {
        Self {
            calls: self.calls.saturating_sub(before.calls),
            cache_hits: self.cache_hits.saturating_sub(before.cache_hits),
            cache_misses: self.cache_misses.saturating_sub(before.cache_misses),
            full_scene_scans: self
                .full_scene_scans
                .saturating_sub(before.full_scene_scans),
            owner_locality_fast_hits: self
                .owner_locality_fast_hits
                .saturating_sub(before.owner_locality_fast_hits),
            certificate_hits: self
                .certificate_hits
                .saturating_sub(before.certificate_hits),
            certificate_misses: self
                .certificate_misses
                .saturating_sub(before.certificate_misses),
            certificate_builds: self
                .certificate_builds
                .saturating_sub(before.certificate_builds),
            certificate_rejections: self
                .certificate_rejections
                .saturating_sub(before.certificate_rejections),
            groups_inspected: self
                .groups_inspected
                .saturating_sub(before.groups_inspected),
            surfaces_inspected: self
                .surfaces_inspected
                .saturating_sub(before.surfaces_inspected),
            cpu_ns: self.cpu_ns.saturating_sub(before.cpu_ns),
            pointer_hit_generation_invalidations: self
                .pointer_hit_generation_invalidations
                .saturating_sub(before.pointer_hit_generation_invalidations),
            interaction_topology_invalidations: self
                .interaction_topology_invalidations
                .saturating_sub(before.interaction_topology_invalidations),
            visual_stack_groups_cache_rebuilds: self
                .visual_stack_groups_cache_rebuilds
                .saturating_sub(before.visual_stack_groups_cache_rebuilds),
        }
    }
}

impl From<PointerInputMetricsSnapshot> for CursorHitTestCounters {
    fn from(snapshot: PointerInputMetricsSnapshot) -> Self {
        Self {
            calls: snapshot.calls,
            cache_hits: snapshot.cache_hits,
            cache_misses: snapshot.cache_misses,
            full_scene_scans: snapshot.full_scene_scans,
            owner_locality_fast_hits: snapshot.owner_locality_fast_hits,
            certificate_hits: snapshot.certificate_hits,
            certificate_misses: snapshot.certificate_misses,
            certificate_builds: snapshot.certificate_builds,
            certificate_rejections: snapshot.certificate_rejections,
            groups_inspected: snapshot.groups_inspected,
            surfaces_inspected: snapshot.surfaces_inspected,
            cpu_ns: snapshot.cpu_ns,
            pointer_hit_generation_invalidations: snapshot.pointer_hit_generation_invalidations,
            interaction_topology_invalidations: snapshot.interaction_topology_invalidations,
            visual_stack_groups_cache_rebuilds: snapshot.visual_stack_groups_cache_rebuilds,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct CursorArbitrationCounters {
    response_windows_opened: u64,
    changes_coalesced: u64,
    plane_delta_plans: u64,
    plane_delta_submissions: u64,
    plane_delta_deferred_for_primary: u64,
    cursor_state_piggybacked: u64,
    idle_hardware_updates: u64,
    idle_software_updates: u64,
    ready_primary_piggybacks: u64,
    unready_primary_bypass_decisions: u64,
    kms_busy_retries: u64,
    cursor_service_target_misses: u64,
}

impl CursorArbitrationCounters {
    fn snapshot(arbitration: &NativeCursorOutputArbitration) -> Self {
        Self {
            response_windows_opened: arbitration.response_windows_opened(),
            changes_coalesced: arbitration.changes_coalesced(),
            plane_delta_plans: arbitration.plane_delta_plans(),
            plane_delta_submissions: arbitration.plane_delta_submissions(),
            plane_delta_deferred_for_primary: arbitration.plane_delta_deferred_for_primary(),
            cursor_state_piggybacked: arbitration.cursor_state_piggybacked(),
            idle_hardware_updates: arbitration.idle_hardware_updates(),
            idle_software_updates: arbitration.idle_software_updates(),
            ready_primary_piggybacks: arbitration.ready_primary_piggybacks,
            unready_primary_bypass_decisions: arbitration.unready_primary_bypass_decisions,
            kms_busy_retries: arbitration.kms_busy_retries,
            cursor_service_target_misses: arbitration.cursor_service_target_misses,
        }
    }

    fn delta_since(self, before: Self) -> Self {
        Self {
            response_windows_opened: self
                .response_windows_opened
                .saturating_sub(before.response_windows_opened),
            changes_coalesced: self
                .changes_coalesced
                .saturating_sub(before.changes_coalesced),
            plane_delta_plans: self
                .plane_delta_plans
                .saturating_sub(before.plane_delta_plans),
            plane_delta_submissions: self
                .plane_delta_submissions
                .saturating_sub(before.plane_delta_submissions),
            plane_delta_deferred_for_primary: self
                .plane_delta_deferred_for_primary
                .saturating_sub(before.plane_delta_deferred_for_primary),
            cursor_state_piggybacked: self
                .cursor_state_piggybacked
                .saturating_sub(before.cursor_state_piggybacked),
            idle_hardware_updates: self
                .idle_hardware_updates
                .saturating_sub(before.idle_hardware_updates),
            idle_software_updates: self
                .idle_software_updates
                .saturating_sub(before.idle_software_updates),
            ready_primary_piggybacks: self
                .ready_primary_piggybacks
                .saturating_sub(before.ready_primary_piggybacks),
            unready_primary_bypass_decisions: self
                .unready_primary_bypass_decisions
                .saturating_sub(before.unready_primary_bypass_decisions),
            kms_busy_retries: self
                .kms_busy_retries
                .saturating_sub(before.kms_busy_retries),
            cursor_service_target_misses: self
                .cursor_service_target_misses
                .saturating_sub(before.cursor_service_target_misses),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct PointerMotionEventStats {
    pub(super) count: u32,
    pub(super) oldest_timestamp_usec: Option<u64>,
    pub(super) newest_timestamp_usec: Option<u64>,
}

pub(super) fn pointer_motion_event_stats(
    events: &[crate::native_output::input::NativeHardwareInputEvent],
) -> PointerMotionEventStats {
    events
        .iter()
        .fold(PointerMotionEventStats::default(), |mut stats, event| {
            if let crate::native_output::input::NativeHardwareInputEvent::PointerMotion(sample) =
                event
            {
                stats.count = stats.count.saturating_add(1);
                stats.oldest_timestamp_usec = Some(
                    stats
                        .oldest_timestamp_usec
                        .map_or(sample.timestamp_usec, |oldest| {
                            oldest.min(sample.timestamp_usec)
                        }),
                );
                stats.newest_timestamp_usec = Some(
                    stats
                        .newest_timestamp_usec
                        .map_or(sample.timestamp_usec, |newest| {
                            newest.max(sample.timestamp_usec)
                        }),
                );
            }
            stats
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CursorMotionBatchTiming {
    pub(crate) input_batch_materialized_at_ns: u64,
    pub(crate) dispatch_started_at_ns: u64,
    pub(crate) source_oldest_timestamp_usec: Option<u64>,
    pub(crate) source_newest_timestamp_usec: Option<u64>,
    pub(crate) hit_test_before: CursorHitTestCounters,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CursorKmsAttribution {
    pub(crate) output_id: OutputId,
    pub(crate) cursor_epoch: u64,
    pub(crate) transaction_id: OutputTransactionId,
    pub(crate) transaction_built_at_ns: u64,
    pub(crate) worker_queued_at_ns: Option<u64>,
    pub(crate) kms_submit_started_at_ns: Option<u64>,
    pub(crate) kms_submit_returned_at_ns: Option<u64>,
    /// This is a compositor/kernel pageflip observation, not optical visibility.
    pub(crate) pageflip_observed_at_ns: Option<u64>,
    pub(crate) pageflip_sequence: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CursorResponsivenessSample {
    pub(crate) sample_id: u64,
    pub(crate) raw_motion_events: u32,
    pub(crate) coalesced_motion_events: u32,
    pub(crate) source_oldest_timestamp_usec: Option<u64>,
    pub(crate) source_newest_timestamp_usec: Option<u64>,
    pub(crate) input_batch_materialized_at_ns: u64,
    pub(crate) dispatch_started_at_ns: u64,
    pub(crate) dispatch_completed_at_ns: Option<u64>,
    hit_test_before: CursorHitTestCounters,
    pub(crate) hit_test_delta: Option<CursorHitTestCounters>,
    pub(crate) output_id: Option<OutputId>,
    /// Epoch created by this input batch after processing its coalesced events.
    pub(crate) cursor_epoch: Option<u64>,
    pub(crate) arbitration_cursor_epoch: Option<u64>,
    pub(crate) arbitration_at_ns: Option<u64>,
    pub(crate) disposition: Option<NativeCursorOutputDisposition>,
    pub(crate) policy: Option<&'static str>,
    pub(crate) arbitration_reason: Option<&'static str>,
    pub(crate) cursor_pending_since_ns: Option<u64>,
    pub(crate) cursor_age_at_arbitration_ns: Option<u64>,
    pub(crate) cursor_overdue_age_ns: Option<u64>,
    pub(crate) cursor_service_deadline_ns: Option<u64>,
    pub(crate) cursor_target_service_deadline_ns: Option<u64>,
    pub(crate) cursor_prepare_budget_ns: Option<u64>,
    pub(crate) cursor_dispatch_budget_ns: Option<u64>,
    pub(crate) cursor_apply_guard_ns: Option<u64>,
    pub(crate) cursor_total_lead_ns: Option<u64>,
    pub(crate) cursor_retry_not_before_ns: Option<u64>,
    pub(crate) primary_ready_carrier: Option<bool>,
    pub(crate) primary_target_owned: Option<bool>,
    pub(crate) primary_worker_queued: Option<bool>,
    pub(crate) primary_kernel_submitted: Option<bool>,
    pub(crate) independent_kms_admission: Option<bool>,
    pub(crate) cursor_target_sequence: Option<u64>,
    pub(crate) cursor_target_presentation_ns: Option<u64>,
    pub(crate) cursor_worker_wake_ns: Option<u64>,
    pub(crate) cursor_commit_complete_deadline_ns: Option<u64>,
    pub(crate) superseded_by_epoch: Option<u64>,
    pub(crate) effect_scene_resolution_ns: Option<u64>,
    pub(crate) effect_scene_visible_instances: Option<usize>,
    /// CPU time from sampled timing capture through ring insertion and the
    /// O(1) newest-record lookup. This excludes per-batch counters updated
    /// before sampling, input processing around this method, and writing this
    /// measured value plus its aggregate after the ending clock read.
    pub(crate) trace_write_cpu_ns: Option<u64>,
}

#[derive(Debug)]
pub(crate) struct NativeCursorResponsivenessTrace {
    enabled: bool,
    capacity: usize,
    sample_stride: u64,
    records: VecDeque<CursorResponsivenessSample>,
    /// Next usable ID. `None` means the u64 ID space was exhausted.
    next_sample_id: Option<u64>,
    /// Highest retained sample ID with a cursor epoch for each output.
    /// Kept in sync with FIFO retention so supersession never scans history.
    latest_epoch_sample_by_output: HashMap<OutputId, u64>,
    pub(crate) motion_batches: u64,
    pub(crate) sampled_batches: u64,
    pub(crate) raw_motion_events: u64,
    pub(crate) coalesced_motion_events: u64,
    pub(crate) evicted_samples: u64,
    effect_scene_resolutions: u64,
    effect_scene_duration_total_ns: u64,
    arbitration_piggyback_primary: u64,
    arbitration_defer_for_primary: u64,
    arbitration_plane_delta: u64,
    arbitration_software_overlay: u64,
    arbitration_noop: u64,
    trace_write_cpu_total_ns: u64,
    trace_path: Option<PathBuf>,
    hit_test_baseline: CursorHitTestCounters,
    arbitration_baseline: CursorArbitrationCounters,
}

impl NativeCursorResponsivenessTrace {
    pub(crate) fn from_env(hit_test_baseline: Option<PointerInputMetricsSnapshot>) -> Self {
        let enabled = crate::pointer_debug::cursor_responsiveness_trace_enabled();
        let capacity = std::env::var("TYPHON_CURSOR_RESPONSIVENESS_TRACE_CAPACITY")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(DEFAULT_CAPACITY)
            .clamp(1, MAX_CAPACITY);
        let trace_path = enabled
            .then(|| std::env::var_os("TYPHON_CURSOR_RESPONSIVENESS_TRACE_FILE"))
            .flatten()
            .map(PathBuf::from);
        let mut trace = Self::new_with_path(enabled, capacity, DEFAULT_SAMPLE_STRIDE, trace_path);
        if let Some(hit_test_baseline) = hit_test_baseline {
            trace.hit_test_baseline = hit_test_baseline.into();
        }
        trace
    }

    fn new(enabled: bool, capacity: usize, sample_stride: u64) -> Self {
        Self::new_with_path(enabled, capacity, sample_stride, None)
    }

    fn new_with_path(
        enabled: bool,
        capacity: usize,
        sample_stride: u64,
        trace_path: Option<PathBuf>,
    ) -> Self {
        let capacity = capacity.clamp(1, MAX_CAPACITY);
        Self {
            enabled,
            capacity,
            sample_stride: sample_stride.max(1),
            records: VecDeque::with_capacity(if enabled { capacity } else { 0 }),
            next_sample_id: Some(1),
            latest_epoch_sample_by_output: HashMap::new(),
            motion_batches: 0,
            sampled_batches: 0,
            raw_motion_events: 0,
            coalesced_motion_events: 0,
            evicted_samples: 0,
            effect_scene_resolutions: 0,
            effect_scene_duration_total_ns: 0,
            arbitration_piggyback_primary: 0,
            arbitration_defer_for_primary: 0,
            arbitration_plane_delta: 0,
            arbitration_software_overlay: 0,
            arbitration_noop: 0,
            trace_write_cpu_total_ns: 0,
            trace_path,
            hit_test_baseline: CursorHitTestCounters::default(),
            arbitration_baseline: CursorArbitrationCounters::default(),
        }
    }

    pub(crate) const fn enabled(&self) -> bool {
        self.enabled
    }

    /// Count every motion batch and lazily capture one record at the configured stride.
    pub(crate) fn record_motion_batch(
        &mut self,
        raw_motion_events: u32,
        coalesced_motion_events: u32,
        capture: impl FnOnce() -> Option<CursorMotionBatchTiming>,
    ) -> Option<u64> {
        if !self.enabled || raw_motion_events == 0 {
            return None;
        }
        self.motion_batches = self.motion_batches.saturating_add(1);
        self.raw_motion_events = self
            .raw_motion_events
            .saturating_add(u64::from(raw_motion_events));
        self.coalesced_motion_events = self
            .coalesced_motion_events
            .saturating_add(u64::from(coalesced_motion_events));
        if self.motion_batches % self.sample_stride != 0 {
            return None;
        }

        let sample_id = self.next_sample_id?;
        let trace_start_cpu_ns = super::pointer_timing::thread_cpu_time_ns();
        let timing = capture()?;
        self.next_sample_id = sample_id.checked_add(1);
        self.sampled_batches = self.sampled_batches.saturating_add(1);
        if self.records.len() == self.capacity {
            if let Some(evicted) = self.records.pop_front()
                && let Some(output_id) = evicted.output_id
                && evicted.cursor_epoch.is_some()
                && self.latest_epoch_sample_by_output.get(&output_id) == Some(&evicted.sample_id)
            {
                // The evicted record was the highest retained ID for this
                // output. FIFO order guarantees no older retained record can
                // replace it; newer incomplete records have no epoch entry.
                self.latest_epoch_sample_by_output.remove(&output_id);
            }
            self.evicted_samples = self.evicted_samples.saturating_add(1);
        }
        self.records.push_back(CursorResponsivenessSample {
            sample_id,
            raw_motion_events,
            coalesced_motion_events,
            source_oldest_timestamp_usec: timing.source_oldest_timestamp_usec,
            source_newest_timestamp_usec: timing.source_newest_timestamp_usec,
            input_batch_materialized_at_ns: timing.input_batch_materialized_at_ns,
            dispatch_started_at_ns: timing.dispatch_started_at_ns,
            dispatch_completed_at_ns: None,
            hit_test_before: timing.hit_test_before,
            hit_test_delta: None,
            output_id: None,
            cursor_epoch: None,
            arbitration_cursor_epoch: None,
            arbitration_at_ns: None,
            disposition: None,
            policy: None,
            arbitration_reason: None,
            cursor_pending_since_ns: None,
            cursor_age_at_arbitration_ns: None,
            cursor_overdue_age_ns: None,
            cursor_service_deadline_ns: None,
            cursor_target_service_deadline_ns: None,
            cursor_prepare_budget_ns: None,
            cursor_dispatch_budget_ns: None,
            cursor_apply_guard_ns: None,
            cursor_total_lead_ns: None,
            cursor_retry_not_before_ns: None,
            primary_ready_carrier: None,
            primary_target_owned: None,
            primary_worker_queued: None,
            primary_kernel_submitted: None,
            independent_kms_admission: None,
            cursor_target_sequence: None,
            cursor_target_presentation_ns: None,
            cursor_worker_wake_ns: None,
            cursor_commit_complete_deadline_ns: None,
            superseded_by_epoch: None,
            effect_scene_resolution_ns: None,
            effect_scene_visible_instances: None,
            trace_write_cpu_ns: None,
        });
        // VecDeque::back_mut is the O(1) lookup needed to attach the measured
        // write cost. Take the ending timestamp only after that lookup.
        let sample = self
            .records
            .back_mut()
            .expect("a newly retained sample is the ring's newest record");
        let elapsed_cpu_ns = trace_start_cpu_ns
            .zip(super::pointer_timing::thread_cpu_time_ns())
            .map(|(start, end)| end.saturating_sub(start));
        sample.trace_write_cpu_ns = elapsed_cpu_ns;
        if let Some(elapsed_cpu_ns) = elapsed_cpu_ns {
            self.trace_write_cpu_total_ns =
                self.trace_write_cpu_total_ns.saturating_add(elapsed_cpu_ns);
        }
        Some(sample_id)
    }

    pub(crate) fn complete_motion_batch(
        &mut self,
        sample_id: u64,
        output_id: OutputId,
        cursor_epoch: Option<u64>,
        dispatch_completed_at_ns: u64,
        hit_test_after: CursorHitTestCounters,
    ) {
        if !self.enabled {
            return;
        }
        if let Some(cursor_epoch) = cursor_epoch {
            self.note_cursor_epoch(output_id, cursor_epoch);
        }
        let Some(sample) = self.sample_mut(sample_id) else {
            return;
        };
        sample.output_id = Some(output_id);
        sample.cursor_epoch = cursor_epoch;
        sample.dispatch_completed_at_ns = Some(dispatch_completed_at_ns);
        sample.hit_test_delta = Some(hit_test_after.delta_since(sample.hit_test_before));
        if cursor_epoch.is_some() {
            self.latest_epoch_sample_by_output
                .entry(output_id)
                .and_modify(|latest| *latest = (*latest).max(sample_id))
                .or_insert(sample_id);
        }
    }

    pub(crate) fn note_cursor_epoch(&mut self, output_id: OutputId, epoch: u64) {
        if !self.enabled {
            return;
        }
        let Some(sample_id) = self.latest_epoch_sample_by_output.get(&output_id).copied() else {
            return;
        };
        let Some(sample) = self.sample_mut(sample_id) else {
            // Be conservative if future changes break index synchronization.
            self.latest_epoch_sample_by_output.remove(&output_id);
            return;
        };
        if let Some(previous_epoch) = sample.cursor_epoch
            && epoch > previous_epoch
        {
            sample.superseded_by_epoch = Some(epoch);
        }
    }

    pub(crate) fn record_arbitration(
        &mut self,
        sample_id: Option<u64>,
        output_id: OutputId,
        cursor_epoch: Option<u64>,
        at_ns: u64,
        disposition: NativeCursorOutputDisposition,
    ) {
        if !self.enabled {
            return;
        }
        match disposition {
            NativeCursorOutputDisposition::PiggybackPrimary => {
                self.arbitration_piggyback_primary =
                    self.arbitration_piggyback_primary.saturating_add(1)
            }
            NativeCursorOutputDisposition::DeferForPrimary => {
                self.arbitration_defer_for_primary =
                    self.arbitration_defer_for_primary.saturating_add(1)
            }
            NativeCursorOutputDisposition::SubmitPlaneDelta => {
                self.arbitration_plane_delta = self.arbitration_plane_delta.saturating_add(1)
            }
            NativeCursorOutputDisposition::SoftwareOverlay => {
                self.arbitration_software_overlay =
                    self.arbitration_software_overlay.saturating_add(1)
            }
            NativeCursorOutputDisposition::Noop => {
                self.arbitration_noop = self.arbitration_noop.saturating_add(1)
            }
        }
        if let Some(cursor_epoch) = cursor_epoch {
            self.note_cursor_epoch(output_id, cursor_epoch);
        }
        let Some(sample_id) = sample_id else {
            return;
        };
        let Some(sample) = self.sample_mut(sample_id) else {
            return;
        };
        sample.arbitration_cursor_epoch = cursor_epoch;
        sample.arbitration_at_ns = Some(at_ns);
        sample.disposition = Some(disposition);
        if let Some(cursor_epoch) = cursor_epoch
            && sample
                .cursor_epoch
                .is_some_and(|sample_epoch| cursor_epoch > sample_epoch)
        {
            sample.superseded_by_epoch = Some(cursor_epoch);
        }
    }

    pub(crate) fn record_arbitration_evidence(
        &mut self,
        sample_id: Option<u64>,
        output_id: OutputId,
        cursor_epoch: Option<u64>,
        at_ns: u64,
        policy: NativeCursorSchedulingPolicy,
        arbitration: &NativeCursorOutputArbitration,
        primary: NativeCursorPrimaryState,
        opportunity: Option<CursorServiceOpportunity>,
        disposition: NativeCursorOutputDisposition,
    ) {
        if !self.enabled {
            return;
        }
        let sample_id = sample_id.or_else(|| {
            (cursor_epoch.is_some() || arbitration.pending())
                .then(|| self.latest_epoch_sample_by_output.get(&output_id).copied())
                .flatten()
        });
        self.record_arbitration(sample_id, output_id, cursor_epoch, at_ns, disposition);
        let Some(sample) = sample_id.and_then(|sample_id| self.sample_mut(sample_id)) else {
            return;
        };
        sample.policy = Some(policy.as_str());
        sample.arbitration_reason = Some(arbitration_reason(policy, primary, disposition));
        sample.cursor_pending_since_ns = arbitration.pending_since_ns();
        sample.cursor_age_at_arbitration_ns = arbitration.pending_age_ns(at_ns);
        sample.cursor_service_deadline_ns = arbitration.deadline_ns();
        sample.cursor_overdue_age_ns = arbitration
            .deadline_ns()
            .filter(|deadline| *deadline <= at_ns)
            .map(|deadline| at_ns.saturating_sub(deadline));
        sample.cursor_retry_not_before_ns = arbitration.retry_not_before_ns();
        sample.primary_ready_carrier = Some(primary.ready_primary_carrier_attachable);
        sample.primary_target_owned = Some(primary.primary_target_owned);
        sample.primary_worker_queued = Some(primary.primary_worker_queued);
        sample.primary_kernel_submitted = Some(primary.primary_kernel_submitted);
        sample.independent_kms_admission = Some(primary.independent_kms_admission);
        if let Some(opportunity) = opportunity {
            sample.cursor_target_sequence = Some(opportunity.target.sequence);
            sample.cursor_target_presentation_ns = Some(opportunity.target.presentation_time.get());
            sample.cursor_worker_wake_ns = Some(opportunity.submit_window.worker_wake_at_ns());
            sample.cursor_commit_complete_deadline_ns =
                Some(opportunity.submit_window.commit_complete_deadline_ns());
            sample.cursor_target_service_deadline_ns = Some(opportunity.service_deadline_ns);
            sample.cursor_prepare_budget_ns = Some(opportunity.prepare_budget_ns);
            sample.cursor_dispatch_budget_ns = Some(opportunity.dispatch_budget_ns);
            sample.cursor_apply_guard_ns = Some(opportunity.apply_guard_ns);
            sample.cursor_total_lead_ns = Some(opportunity.total_lead_ns);
        }
    }

    pub(crate) fn record_effect_scene_resolution(
        &mut self,
        sample_id: Option<u64>,
        duration_ns: u64,
        visible_instances: usize,
    ) {
        if !self.enabled {
            return;
        }
        self.effect_scene_resolutions = self.effect_scene_resolutions.saturating_add(1);
        self.effect_scene_duration_total_ns = self
            .effect_scene_duration_total_ns
            .saturating_add(duration_ns);
        if let Some(sample) = sample_id.and_then(|sample_id| self.sample_mut(sample_id)) {
            if sample.superseded_by_epoch.is_none() {
                sample.effect_scene_resolution_ns = Some(duration_ns);
                sample.effect_scene_visible_instances = Some(visible_instances);
            }
        }
    }

    pub(crate) fn sample(&self, sample_id: u64) -> Option<&CursorResponsivenessSample> {
        let oldest_id = self.records.front()?.sample_id;
        let index = usize::try_from(sample_id.checked_sub(oldest_id)?).ok()?;
        self.records
            .get(index)
            .filter(|sample| sample.sample_id == sample_id)
    }

    fn sample_mut(&mut self, sample_id: u64) -> Option<&mut CursorResponsivenessSample> {
        let oldest_id = self.records.front()?.sample_id;
        let index = usize::try_from(sample_id.checked_sub(oldest_id)?).ok()?;
        self.records
            .get_mut(index)
            .filter(|sample| sample.sample_id == sample_id)
    }

    pub(crate) fn cursor_transaction_attribution(
        &self,
        sample_id: u64,
        presentation_trace: &PresentationTransactionTraceRing,
    ) -> Option<CursorKmsAttribution> {
        let index = cursor_transaction_attribution_index(presentation_trace);
        let sample = self.sample(sample_id)?;
        self.indexed_attribution(sample, &index)
    }

    fn indexed_attribution(
        &self,
        sample: &CursorResponsivenessSample,
        index: &std::collections::HashMap<(OutputId, u64), Vec<CursorKmsAttribution>>,
    ) -> Option<CursorKmsAttribution> {
        let output_id = sample.output_id?;
        let cursor_epoch = sample.cursor_epoch?;
        if sample.arbitration_cursor_epoch != Some(cursor_epoch) {
            return None;
        }
        let arbitration_at_ns = sample.arbitration_at_ns?;
        index
            .get(&(output_id, cursor_epoch))?
            .iter()
            .find(|attribution| attribution.transaction_built_at_ns >= arbitration_at_ns)
            .copied()
    }

    pub(crate) fn dump(
        &self,
        presentation_trace: &PresentationTransactionTraceRing,
        hit_test_after: PointerInputMetricsSnapshot,
        arbitration_after: &NativeCursorOutputArbitration,
    ) {
        if !self.enabled {
            return;
        }
        let contents = self.format_dump(presentation_trace, hit_test_after, arbitration_after);
        if let Some(path) = self.trace_path.as_ref() {
            let write_result = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::write(path, contents.as_bytes()));
            if let Err(error) = write_result {
                eprintln!(
                    "typhon_cursor_responsiveness_dump_failed path={} error={error}",
                    path.display()
                );
            }
        } else {
            eprintln!("{contents}");
        }
    }

    fn format_dump(
        &self,
        presentation_trace: &PresentationTransactionTraceRing,
        hit_test_after: PointerInputMetricsSnapshot,
        arbitration_after: &NativeCursorOutputArbitration,
    ) -> String {
        let attribution_index = cursor_transaction_attribution_index(presentation_trace);
        let mut output = String::new();
        let hit_test_delta =
            CursorHitTestCounters::from(hit_test_after).delta_since(self.hit_test_baseline);
        let arbitration_delta = CursorArbitrationCounters::snapshot(arbitration_after)
            .delta_since(self.arbitration_baseline);
        let coalescing_ratio = if self.raw_motion_events == 0 {
            0.0
        } else {
            self.raw_motion_events
                .saturating_sub(self.coalesced_motion_events) as f64
                / self.raw_motion_events as f64
        };
        let _ = writeln!(
            output,
            "{{\"type\":\"summary\",\"capacity\":{},\"stride\":{},\"motion_batches\":{},\"sampled_batches\":{},\"retained_samples\":{},\"evicted_samples\":{},\"raw_motion_events\":{},\"coalesced_motion_events\":{},\"coalescing_ratio\":{:.6},\"effect_scene_resolutions\":{},\"effect_scene_duration_total_ns\":{},\"arbitration_piggyback_primary\":{},\"arbitration_defer_for_primary\":{},\"arbitration_plane_delta\":{},\"arbitration_software_overlay\":{},\"arbitration_noop\":{},\"trace_write_cpu_total_ns\":{},\"presentation_events_retained\":{},\"presentation_events_evicted\":{}}}",
            self.capacity,
            self.sample_stride,
            self.motion_batches,
            self.sampled_batches,
            self.records.len(),
            self.evicted_samples,
            self.raw_motion_events,
            self.coalesced_motion_events,
            coalescing_ratio,
            self.effect_scene_resolutions,
            self.effect_scene_duration_total_ns,
            self.arbitration_piggyback_primary,
            self.arbitration_defer_for_primary,
            self.arbitration_plane_delta,
            self.arbitration_software_overlay,
            self.arbitration_noop,
            self.trace_write_cpu_total_ns,
            presentation_trace.events().count(),
            presentation_trace.dropped(),
        );
        let _ = writeln!(
            output,
            "{{\"type\":\"cumulative_counter_deltas\",\"hit_test_calls\":{},\"hit_test_cache_hits\":{},\"hit_test_cache_misses\":{},\"hit_test_full_scene_scans\":{},\"hit_test_owner_locality_fast_hits\":{},\"hit_test_certificate_hits\":{},\"hit_test_certificate_misses\":{},\"hit_test_certificate_builds\":{},\"hit_test_certificate_rejections\":{},\"hit_test_groups_inspected\":{},\"hit_test_surfaces_inspected\":{},\"hit_test_cpu_ns\":{},\"hit_test_generation_invalidations\":{},\"hit_test_topology_invalidations\":{},\"hit_test_visual_stack_groups_cache_rebuilds\":{},\"arbitration_response_windows_opened\":{},\"arbitration_changes_coalesced\":{},\"arbitration_plane_delta_plans\":{},\"arbitration_plane_delta_submissions\":{},\"arbitration_deferred_for_primary\":{},\"arbitration_piggybacked\":{},\"arbitration_idle_hardware_updates\":{},\"arbitration_idle_software_updates\":{},\"ready_primary_piggybacks\":{},\"unready_primary_bypass_decisions\":{},\"kms_busy_retries\":{},\"cursor_service_target_misses\":{}}}",
            hit_test_delta.calls,
            hit_test_delta.cache_hits,
            hit_test_delta.cache_misses,
            hit_test_delta.full_scene_scans,
            hit_test_delta.owner_locality_fast_hits,
            hit_test_delta.certificate_hits,
            hit_test_delta.certificate_misses,
            hit_test_delta.certificate_builds,
            hit_test_delta.certificate_rejections,
            hit_test_delta.groups_inspected,
            hit_test_delta.surfaces_inspected,
            hit_test_delta.cpu_ns,
            hit_test_delta.pointer_hit_generation_invalidations,
            hit_test_delta.interaction_topology_invalidations,
            hit_test_delta.visual_stack_groups_cache_rebuilds,
            arbitration_delta.response_windows_opened,
            arbitration_delta.changes_coalesced,
            arbitration_delta.plane_delta_plans,
            arbitration_delta.plane_delta_submissions,
            arbitration_delta.plane_delta_deferred_for_primary,
            arbitration_delta.cursor_state_piggybacked,
            arbitration_delta.idle_hardware_updates,
            arbitration_delta.idle_software_updates,
            arbitration_delta.ready_primary_piggybacks,
            arbitration_delta.unready_primary_bypass_decisions,
            arbitration_delta.kms_busy_retries,
            arbitration_delta.cursor_service_target_misses,
        );
        let cursor_transactions = presentation_trace
            .cursor_epoch_transactions()
            .collect::<Vec<_>>();
        let _ = writeln!(
            output,
            "{{\"type\":\"presentation_trace_retention\",\"cursor_epoch_transactions_retained\":{},\"cursor_epoch_transactions_evicted\":{}}}",
            cursor_transactions.len(),
            presentation_trace.cursor_epoch_transactions_dropped(),
        );
        let _ = writeln!(
            output,
            "{{\"type\":\"cursor_transaction_event_counts_retained\",\"built\":{},\"worker_queued\":{},\"kms_submit_started\":{},\"kms_submit_returned\":{},\"pageflip_observed\":{}}}",
            cursor_transactions.len(),
            cursor_transactions
                .iter()
                .filter(|transaction| transaction.worker_queued_at_ns.is_some())
                .count(),
            cursor_transactions
                .iter()
                .filter(|transaction| transaction.kms_submit_started_at_ns.is_some())
                .count(),
            cursor_transactions
                .iter()
                .filter(|transaction| transaction.kms_submit_returned_at_ns.is_some())
                .count(),
            cursor_transactions
                .iter()
                .filter(|transaction| transaction.pageflip_observed_at_ns.is_some())
                .count(),
        );
        append_percentile_summary(
            &mut output,
            "dispatch_duration_ns",
            self.records.iter().filter_map(|sample| {
                sample
                    .dispatch_completed_at_ns
                    .map(|end| end.saturating_sub(sample.dispatch_started_at_ns))
            }),
        );
        append_percentile_summary(
            &mut output,
            "input_batch_to_dispatch_complete_ns",
            self.records.iter().filter_map(|sample| {
                sample
                    .dispatch_completed_at_ns
                    .map(|end| end.saturating_sub(sample.input_batch_materialized_at_ns))
            }),
        );
        append_percentile_summary(
            &mut output,
            "dispatch_to_arbitration_ns",
            self.records.iter().filter_map(|sample| {
                sample
                    .arbitration_at_ns
                    .map(|at| at.saturating_sub(sample.dispatch_started_at_ns))
            }),
        );
        append_percentile_summary(
            &mut output,
            "dispatch_complete_to_arbitration_ns",
            self.records.iter().filter_map(|sample| {
                sample
                    .dispatch_completed_at_ns
                    .zip(sample.arbitration_at_ns)
                    .map(|(completed, arbitrated)| arbitrated.saturating_sub(completed))
            }),
        );
        append_percentile_summary(
            &mut output,
            "hit_test_cpu_ns",
            self.records
                .iter()
                .filter_map(|sample| sample.hit_test_delta.map(|delta| delta.cpu_ns)),
        );
        append_percentile_summary(
            &mut output,
            "effect_scene_resolution_ns",
            self.records
                .iter()
                .filter_map(|sample| sample.effect_scene_resolution_ns),
        );
        append_percentile_summary(
            &mut output,
            "trace_write_cpu_ns",
            self.records
                .iter()
                .filter_map(|sample| sample.trace_write_cpu_ns),
        );
        let mut kms_queue = Vec::new();
        let mut kms_submit = Vec::new();
        let mut submit_to_pageflip = Vec::new();
        let mut arbitration_to_build = Vec::new();
        let mut arbitration_to_worker_queue = Vec::new();
        let mut missing_transaction = 0u64;
        let mut missing_submit = 0u64;
        let mut missing_pageflip = 0u64;
        for sample in &self.records {
            let Some(attribution) = self.indexed_attribution(sample, &attribution_index) else {
                missing_transaction = missing_transaction.saturating_add(1);
                continue;
            };
            if let Some(arbitration_at) = sample.arbitration_at_ns {
                arbitration_to_build.push(
                    attribution
                        .transaction_built_at_ns
                        .saturating_sub(arbitration_at),
                );
                if let Some(queued) = attribution.worker_queued_at_ns {
                    arbitration_to_worker_queue.push(queued.saturating_sub(arbitration_at));
                }
            }
            if let (Some(queued), Some(started)) = (
                attribution.worker_queued_at_ns,
                attribution.kms_submit_started_at_ns,
            ) {
                kms_queue.push(started.saturating_sub(queued));
            }
            if let (Some(started), Some(returned)) = (
                attribution.kms_submit_started_at_ns,
                attribution.kms_submit_returned_at_ns,
            ) {
                kms_submit.push(returned.saturating_sub(started));
            } else {
                missing_submit = missing_submit.saturating_add(1);
            }
            if let (Some(returned), Some(pageflip)) = (
                attribution.kms_submit_returned_at_ns,
                attribution.pageflip_observed_at_ns,
            ) {
                submit_to_pageflip.push(pageflip.saturating_sub(returned));
            } else {
                missing_pageflip = missing_pageflip.saturating_add(1);
            }
        }
        append_percentile_summary(&mut output, "kms_queue_wait_ns", kms_queue.into_iter());
        append_percentile_summary(
            &mut output,
            "arbitration_to_transaction_build_ns",
            arbitration_to_build.into_iter(),
        );
        append_percentile_summary(
            &mut output,
            "arbitration_to_worker_queue_ns",
            arbitration_to_worker_queue.into_iter(),
        );
        append_percentile_summary(
            &mut output,
            "kms_submit_duration_ns",
            kms_submit.into_iter(),
        );
        append_percentile_summary(
            &mut output,
            "submit_return_to_pageflip_observation_ns",
            submit_to_pageflip.into_iter(),
        );
        let _ = writeln!(
            output,
            "{{\"type\":\"attribution_counts\",\"missing_cursor_epoch_transaction\":{},\"missing_kms_submit_pair\":{},\"missing_pageflip_observation\":{}}}",
            missing_transaction, missing_submit, missing_pageflip,
        );
        for sample in &self.records {
            append_sample_jsonl(
                &mut output,
                *sample,
                self.indexed_attribution(sample, &attribution_index),
            );
        }
        output
    }
}

fn append_percentile_summary(output: &mut String, name: &str, values: impl Iterator<Item = u64>) {
    let mut values = values.collect::<Vec<_>>();
    values.sort_unstable();
    let p50 = percentile(&values, 50);
    let p95 = percentile(&values, 95);
    let p99 = percentile(&values, 99);
    let _ = writeln!(
        output,
        "{{\"type\":\"percentiles\",\"metric\":\"{name}\",\"count\":{},\"p50_ns\":{},\"p95_ns\":{},\"p99_ns\":{}}}",
        values.len(),
        p50.map_or_else(|| "null".to_owned(), |value| value.to_string()),
        p95.map_or_else(|| "null".to_owned(), |value| value.to_string()),
        p99.map_or_else(|| "null".to_owned(), |value| value.to_string()),
    );
}

fn percentile(sorted_values: &[u64], percentile: usize) -> Option<u64> {
    if sorted_values.is_empty() {
        return None;
    }
    let rank = sorted_values
        .len()
        .saturating_mul(percentile)
        .saturating_add(99)
        / 100;
    sorted_values.get(rank.saturating_sub(1)).copied()
}

fn cursor_transaction_attribution_index(
    presentation_trace: &PresentationTransactionTraceRing,
) -> std::collections::HashMap<(OutputId, u64), Vec<CursorKmsAttribution>> {
    let mut by_epoch =
        std::collections::HashMap::<(OutputId, u64), Vec<CursorKmsAttribution>>::new();
    for transaction in presentation_trace.cursor_epoch_transactions() {
        let attribution = CursorKmsAttribution::from(transaction);
        by_epoch
            .entry((attribution.output_id, attribution.cursor_epoch))
            .or_default()
            .push(attribution);
    }
    for candidates in by_epoch.values_mut() {
        candidates.sort_unstable_by_key(|attribution| {
            (
                attribution.transaction_built_at_ns,
                attribution.transaction_id.get(),
            )
        });
    }
    by_epoch
}

impl From<CursorEpochPresentationTrace> for CursorKmsAttribution {
    fn from(transaction: CursorEpochPresentationTrace) -> Self {
        Self {
            output_id: transaction.output_id,
            cursor_epoch: transaction.cursor_epoch,
            transaction_id: transaction.transaction_id,
            transaction_built_at_ns: transaction.transaction_built_at_ns,
            worker_queued_at_ns: transaction.worker_queued_at_ns,
            kms_submit_started_at_ns: transaction.kms_submit_started_at_ns,
            kms_submit_returned_at_ns: transaction.kms_submit_returned_at_ns,
            pageflip_observed_at_ns: transaction.pageflip_observed_at_ns,
            pageflip_sequence: transaction.pageflip_sequence,
        }
    }
}

fn append_sample_jsonl(
    output: &mut String,
    sample: CursorResponsivenessSample,
    attribution: Option<CursorKmsAttribution>,
) {
    let dispatch_duration_ns = sample
        .dispatch_completed_at_ns
        .map(|end| end.saturating_sub(sample.dispatch_started_at_ns));
    let input_batch_to_dispatch_complete_ns = sample
        .dispatch_completed_at_ns
        .map(|end| end.saturating_sub(sample.input_batch_materialized_at_ns));
    let dispatch_to_arbitration_ns = sample
        .arbitration_at_ns
        .map(|at| at.saturating_sub(sample.dispatch_started_at_ns));
    let dispatch_complete_to_arbitration_ns = sample
        .dispatch_completed_at_ns
        .zip(sample.arbitration_at_ns)
        .map(|(completed, arbitrated)| arbitrated.saturating_sub(completed));
    let hit_test_cpu_ns = sample.hit_test_delta.map(|delta| delta.cpu_ns);
    let kms_queue_wait_ns = attribution.and_then(|value| {
        value
            .worker_queued_at_ns
            .zip(value.kms_submit_started_at_ns)
            .map(|(queued, started)| started.saturating_sub(queued))
    });
    let kms_submit_duration_ns = attribution.and_then(|value| {
        value
            .kms_submit_started_at_ns
            .zip(value.kms_submit_returned_at_ns)
            .map(|(started, returned)| returned.saturating_sub(started))
    });
    let submit_return_to_pageflip_ns = attribution.and_then(|value| {
        value
            .kms_submit_returned_at_ns
            .zip(value.pageflip_observed_at_ns)
            .map(|(returned, pageflip)| pageflip.saturating_sub(returned))
    });
    let pending_age_at_worker_queue_ns = sample.cursor_pending_since_ns.and_then(|pending_since| {
        attribution
            .and_then(|value| value.worker_queued_at_ns)
            .map(|queued| queued.saturating_sub(pending_since))
    });
    let pending_age_at_transaction_built_ns =
        sample.cursor_pending_since_ns.and_then(|pending_since| {
            attribution.map(|value| value.transaction_built_at_ns.saturating_sub(pending_since))
        });
    let pending_age_at_kms_submit_ns = sample.cursor_pending_since_ns.and_then(|pending_since| {
        attribution
            .and_then(|value| value.kms_submit_started_at_ns)
            .map(|submitted| submitted.saturating_sub(pending_since))
    });
    let pending_age_at_pageflip_ns = sample.cursor_pending_since_ns.and_then(|pending_since| {
        attribution
            .and_then(|value| value.pageflip_observed_at_ns)
            .map(|pageflip| pageflip.saturating_sub(pending_since))
    });
    let _ = writeln!(
        output,
        "{{\"type\":\"sample\",\"sample_id\":{},\"raw_motion_events\":{},\"coalesced_motion_events\":{},\"source_oldest_timestamp_usec\":{},\"source_newest_timestamp_usec\":{},\"input_batch_materialized_at_ns\":{},\"dispatch_started_at_ns\":{},\"dispatch_completed_at_ns\":{},\"dispatch_duration_ns\":{},\"input_batch_to_dispatch_complete_ns\":{},\"dispatch_to_arbitration_ns\":{},\"dispatch_complete_to_arbitration_ns\":{},\"hit_test_calls\":{},\"hit_test_cache_hits\":{},\"hit_test_cache_misses\":{},\"hit_test_full_scene_scans\":{},\"hit_test_owner_locality_fast_hits\":{},\"hit_test_certificate_hits\":{},\"hit_test_certificate_misses\":{},\"hit_test_certificate_builds\":{},\"hit_test_certificate_rejections\":{},\"hit_test_groups_inspected\":{},\"hit_test_surfaces_inspected\":{},\"hit_test_cpu_ns\":{},\"hit_test_generation_invalidations\":{},\"hit_test_topology_invalidations\":{},\"hit_test_visual_stack_groups_cache_rebuilds\":{},\"effect_scene_resolution_ns\":{},\"effect_scene_visible_instances\":{},\"output_id\":{},\"cursor_epoch\":{},\"arbitration_cursor_epoch\":{},\"arbitration_at_ns\":{},\"disposition\":\"{}\",\"superseded_by_epoch\":{},\"trace_write_cpu_ns\":{},\"cursor_transaction_id\":{},\"cursor_transaction_built_at_ns\":{},\"worker_queued_at_ns\":{},\"kms_submit_started_at_ns\":{},\"kms_submit_returned_at_ns\":{},\"pageflip_observed_at_ns\":{},\"actual_pageflip_sequence\":{},\"kms_queue_wait_ns\":{},\"kms_submit_duration_ns\":{},\"submit_return_to_pageflip_observation_ns\":{},\"pageflip_timestamp_kind\":\"compositor_observed_not_optical\"}}",
        sample.sample_id,
        sample.raw_motion_events,
        sample.coalesced_motion_events,
        json_number(sample.source_oldest_timestamp_usec),
        json_number(sample.source_newest_timestamp_usec),
        sample.input_batch_materialized_at_ns,
        sample.dispatch_started_at_ns,
        json_number(sample.dispatch_completed_at_ns),
        json_number(dispatch_duration_ns),
        json_number(input_batch_to_dispatch_complete_ns),
        json_number(dispatch_to_arbitration_ns),
        json_number(dispatch_complete_to_arbitration_ns),
        sample
            .hit_test_delta
            .map_or_else(|| "null".to_owned(), |value| value.calls.to_string()),
        sample
            .hit_test_delta
            .map_or_else(|| "null".to_owned(), |value| value.cache_hits.to_string()),
        sample
            .hit_test_delta
            .map_or_else(|| "null".to_owned(), |value| value.cache_misses.to_string()),
        sample.hit_test_delta.map_or_else(
            || "null".to_owned(),
            |value| value.full_scene_scans.to_string()
        ),
        sample.hit_test_delta.map_or_else(
            || "null".to_owned(),
            |value| value.owner_locality_fast_hits.to_string()
        ),
        sample.hit_test_delta.map_or_else(
            || "null".to_owned(),
            |value| value.certificate_hits.to_string()
        ),
        sample.hit_test_delta.map_or_else(
            || "null".to_owned(),
            |value| value.certificate_misses.to_string()
        ),
        sample.hit_test_delta.map_or_else(
            || "null".to_owned(),
            |value| value.certificate_builds.to_string()
        ),
        sample.hit_test_delta.map_or_else(
            || "null".to_owned(),
            |value| value.certificate_rejections.to_string()
        ),
        sample.hit_test_delta.map_or_else(
            || "null".to_owned(),
            |value| value.groups_inspected.to_string()
        ),
        sample.hit_test_delta.map_or_else(
            || "null".to_owned(),
            |value| value.surfaces_inspected.to_string()
        ),
        json_number(hit_test_cpu_ns),
        sample.hit_test_delta.map_or_else(
            || "null".to_owned(),
            |value| value.pointer_hit_generation_invalidations.to_string()
        ),
        sample.hit_test_delta.map_or_else(
            || "null".to_owned(),
            |value| value.interaction_topology_invalidations.to_string()
        ),
        sample.hit_test_delta.map_or_else(
            || "null".to_owned(),
            |value| value.visual_stack_groups_cache_rebuilds.to_string()
        ),
        json_number(sample.effect_scene_resolution_ns),
        json_number(
            sample
                .effect_scene_visible_instances
                .map(|value| value as u64)
        ),
        json_number(sample.output_id.map(OutputId::get)),
        json_number(sample.cursor_epoch),
        json_number(sample.arbitration_cursor_epoch),
        json_number(sample.arbitration_at_ns),
        sample.disposition.map_or("unobserved", disposition_name),
        json_number(sample.superseded_by_epoch),
        json_number(sample.trace_write_cpu_ns),
        json_number(attribution.map(|value| value.transaction_id.get())),
        json_number(attribution.map(|value| value.transaction_built_at_ns)),
        json_number(attribution.and_then(|value| value.worker_queued_at_ns)),
        json_number(attribution.and_then(|value| value.kms_submit_started_at_ns)),
        json_number(attribution.and_then(|value| value.kms_submit_returned_at_ns)),
        json_number(attribution.and_then(|value| value.pageflip_observed_at_ns)),
        json_number(attribution.and_then(|value| value.pageflip_sequence)),
        json_number(kms_queue_wait_ns),
        json_number(kms_submit_duration_ns),
        json_number(submit_return_to_pageflip_ns),
    );
    let _ = writeln!(
        output,
        "{{\"type\":\"arbitration_evidence\",\"sample_id\":{},\"policy\":{},\"reason\":{},\"pending_since_ns\":{},\"cursor_age_at_arbitration_ns\":{},\"service_deadline_ns\":{},\"overdue_age_ns\":{},\"target_service_deadline_ns\":{},\"retry_not_before_ns\":{},\"primary_ready_carrier\":{},\"primary_target_owned\":{},\"primary_worker_queued\":{},\"primary_kernel_submitted\":{},\"independent_kms_admission\":{},\"cursor_target_sequence\":{},\"cursor_target_presentation_ns\":{},\"worker_wake_ns\":{},\"commit_complete_deadline_ns\":{},\"cursor_prepare_budget_ns\":{},\"cursor_dispatch_budget_ns\":{},\"cursor_apply_guard_ns\":{},\"cursor_total_lead_ns\":{},\"cursor_age_at_transaction_build_ns\":{},\"cursor_age_at_worker_queue_ns\":{},\"cursor_age_at_kms_submit_ns\":{},\"cursor_age_at_pageflip_observation_ns\":{}}}",
        sample.sample_id,
        json_string(sample.policy),
        json_string(sample.arbitration_reason),
        json_number(sample.cursor_pending_since_ns),
        json_number(sample.cursor_age_at_arbitration_ns),
        json_number(sample.cursor_service_deadline_ns),
        json_number(sample.cursor_overdue_age_ns),
        json_number(sample.cursor_target_service_deadline_ns),
        json_number(sample.cursor_retry_not_before_ns),
        json_bool(sample.primary_ready_carrier),
        json_bool(sample.primary_target_owned),
        json_bool(sample.primary_worker_queued),
        json_bool(sample.primary_kernel_submitted),
        json_bool(sample.independent_kms_admission),
        json_number(sample.cursor_target_sequence),
        json_number(sample.cursor_target_presentation_ns),
        json_number(sample.cursor_worker_wake_ns),
        json_number(sample.cursor_commit_complete_deadline_ns),
        json_number(sample.cursor_prepare_budget_ns),
        json_number(sample.cursor_dispatch_budget_ns),
        json_number(sample.cursor_apply_guard_ns),
        json_number(sample.cursor_total_lead_ns),
        json_number(pending_age_at_transaction_built_ns),
        json_number(pending_age_at_worker_queue_ns),
        json_number(pending_age_at_kms_submit_ns),
        json_number(pending_age_at_pageflip_ns),
    );
}

fn json_number(value: Option<u64>) -> String {
    value.map_or_else(|| "null".to_owned(), |number| number.to_string())
}

fn json_bool(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "true",
        Some(false) => "false",
        None => "null",
    }
}

fn json_string(value: Option<&'static str>) -> String {
    value.map_or_else(|| "null".to_owned(), |value| format!("\"{value}\""))
}

const fn arbitration_reason(
    policy: NativeCursorSchedulingPolicy,
    primary: NativeCursorPrimaryState,
    disposition: NativeCursorOutputDisposition,
) -> &'static str {
    match (policy, disposition) {
        (NativeCursorSchedulingPolicy::Piggyback, _) => "legacy_piggyback_policy",
        (_, NativeCursorOutputDisposition::SoftwareOverlay) => "software_required",
        (_, NativeCursorOutputDisposition::PiggybackPrimary) => "ready_primary_carrier",
        (_, NativeCursorOutputDisposition::DeferForPrimary) if primary.primary_target_owned => {
            "primary_target_owned"
        }
        (_, NativeCursorOutputDisposition::DeferForPrimary)
            if !primary.independent_kms_admission =>
        {
            "kms_lane_busy"
        }
        (_, NativeCursorOutputDisposition::DeferForPrimary) => "waiting_for_service_window",
        (_, NativeCursorOutputDisposition::SubmitPlaneDelta) => "independent_service_due",
        (_, NativeCursorOutputDisposition::Noop) => "no_pending_cursor",
    }
}

const fn disposition_name(disposition: NativeCursorOutputDisposition) -> &'static str {
    match disposition {
        NativeCursorOutputDisposition::PiggybackPrimary => "piggyback_primary",
        NativeCursorOutputDisposition::DeferForPrimary => "defer_for_primary",
        NativeCursorOutputDisposition::SubmitPlaneDelta => "submit_plane_delta",
        NativeCursorOutputDisposition::SoftwareOverlay => "software_overlay",
        NativeCursorOutputDisposition::Noop => "noop",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CursorHitTestCounters, CursorMotionBatchTiming, DEFAULT_CAPACITY, DEFAULT_SAMPLE_STRIDE,
        NativeCursorResponsivenessTrace,
    };
    use crate::native_output::input::{
        KeyboardDeviceId, NativeHardwareInputEvent, NativeKeyboardInputEvent, PointerMotionSample,
        RelativeMotion,
    };
    use crate::native_output::presentation::trace::{
        PresentationTransactionEvent, PresentationTransactionTraceRing,
    };
    use crate::native_output::presentation::transaction::OutputTransactionId;
    use crate::native_output::runtime::frame::{
        NativeCursorOutputArbitration, NativeCursorOutputDisposition, NativeCursorSchedulingPolicy,
        cursor_output_disposition_at_deadline,
    };
    use oblivion_one::compositor::PointerInputMetricsSnapshot;
    use oblivion_one::core::OutputId;
    use std::num::NonZeroU64;

    fn output_id() -> OutputId {
        OutputId::from_raw(1).unwrap()
    }

    fn other_output_id() -> OutputId {
        OutputId::from_raw(2).unwrap()
    }

    #[test]
    fn motion_batch_trace_counts_pointer_events_only() {
        let events = [
            NativeHardwareInputEvent::PointerMotion(PointerMotionSample::relative(
                100,
                RelativeMotion::accelerated_only(1.0, 0.0),
            )),
            NativeHardwareInputEvent::Keyboard(NativeKeyboardInputEvent::SourceRemoved {
                device: KeyboardDeviceId::from_raw(1).unwrap(),
            }),
            NativeHardwareInputEvent::PointerMotion(PointerMotionSample::relative(
                80,
                RelativeMotion::accelerated_only(0.0, 1.0),
            )),
        ];

        assert_eq!(
            super::pointer_motion_event_stats(&events),
            super::PointerMotionEventStats {
                count: 2,
                oldest_timestamp_usec: Some(80),
                newest_timestamp_usec: Some(100),
            }
        );
    }

    fn timing() -> CursorMotionBatchTiming {
        CursorMotionBatchTiming {
            input_batch_materialized_at_ns: 100,
            dispatch_started_at_ns: 110,
            source_oldest_timestamp_usec: Some(1_000),
            source_newest_timestamp_usec: Some(1_500),
            hit_test_before: CursorHitTestCounters::default(),
        }
    }

    fn sample(trace: &mut NativeCursorResponsivenessTrace, epoch: u64) -> u64 {
        sample_on_output(trace, output_id(), epoch)
    }

    fn sample_on_output(
        trace: &mut NativeCursorResponsivenessTrace,
        output_id: OutputId,
        epoch: u64,
    ) -> u64 {
        let id = trace
            .record_motion_batch(1, 1, || Some(timing()))
            .expect("stride one retains each nonempty batch");
        trace.complete_motion_batch(
            id,
            output_id,
            Some(epoch),
            200,
            CursorHitTestCounters::default(),
        );
        id
    }

    #[test]
    fn disabled_trace_skips_lazy_capture() {
        let mut trace = NativeCursorResponsivenessTrace::new(false, 4, 1);
        let id = trace.record_motion_batch(1, 1, || panic!("disabled trace captured timing"));
        assert_eq!(id, None);
        assert_eq!(trace.motion_batches, 0);
    }

    #[test]
    fn empty_batches_do_not_allocate_motion_samples() {
        let mut trace = NativeCursorResponsivenessTrace::new(true, 4, 1);
        let id = trace.record_motion_batch(0, 0, || panic!("empty batch captured timing"));
        assert_eq!(id, None);
        assert_eq!(trace.motion_batches, 0);
        assert!(trace.records.is_empty());
    }

    #[test]
    fn hit_invalidation_deltas_are_present_in_cursor_responsiveness_jsonl() {
        let mut trace = NativeCursorResponsivenessTrace::new(true, 4, 1);
        let sample_id = trace
            .record_motion_batch(1, 1, || Some(timing()))
            .expect("sample retained");
        trace.complete_motion_batch(
            sample_id,
            output_id(),
            Some(1),
            200,
            CursorHitTestCounters {
                pointer_hit_generation_invalidations: 2,
                interaction_topology_invalidations: 3,
                visual_stack_groups_cache_rebuilds: 4,
                certificate_hits: 5,
                certificate_misses: 6,
                certificate_builds: 7,
                certificate_rejections: 8,
                ..CursorHitTestCounters::default()
            },
        );
        let hit_test_after = PointerInputMetricsSnapshot {
            pointer_hit_generation_invalidations: 2,
            interaction_topology_invalidations: 3,
            visual_stack_groups_cache_rebuilds: 4,
            certificate_hits: 5,
            certificate_misses: 6,
            certificate_builds: 7,
            certificate_rejections: 8,
            ..PointerInputMetricsSnapshot::default()
        };
        let output = trace.format_dump(
            &PresentationTransactionTraceRing::new(4),
            hit_test_after,
            &NativeCursorOutputArbitration::default(),
        );

        assert!(output.contains("\"hit_test_generation_invalidations\":2"));
        assert!(output.contains("\"hit_test_topology_invalidations\":3"));
        assert!(output.contains("\"hit_test_visual_stack_groups_cache_rebuilds\":4"));
        assert!(output.contains("\"hit_test_certificate_hits\":5"));
        assert!(output.contains("\"hit_test_certificate_misses\":6"));
        assert!(output.contains("\"hit_test_certificate_builds\":7"));
        assert!(output.contains("\"hit_test_certificate_rejections\":8"));
    }

    #[test]
    fn ring_retains_only_capacity_and_counts_evictions() {
        let mut trace = NativeCursorResponsivenessTrace::new(true, 2, 1);
        for epoch in 1..=3 {
            sample(&mut trace, epoch);
        }

        assert_eq!(trace.records.len(), 2);
        assert_eq!(trace.motion_batches, 3);
        assert_eq!(trace.sampled_batches, 3);
        assert_eq!(trace.raw_motion_events, 3);
        assert_eq!(trace.coalesced_motion_events, 3);
        assert_eq!(trace.evicted_samples, 1);
    }

    #[test]
    fn sample_lookup_returns_none_for_an_empty_ring_and_indexes_multiple_samples() {
        let mut trace = NativeCursorResponsivenessTrace::new(true, 4, 1);
        assert!(trace.sample(1).is_none());
        assert!(trace.sample_mut(1).is_none());

        let first = sample(&mut trace, 1);
        let second = sample(&mut trace, 2);
        let third = sample(&mut trace, 3);

        assert_eq!(
            trace.sample(first).map(|sample| sample.cursor_epoch),
            Some(Some(1))
        );
        assert_eq!(
            trace.sample(second).map(|sample| sample.cursor_epoch),
            Some(Some(2))
        );
        assert_eq!(
            trace.sample(third).map(|sample| sample.cursor_epoch),
            Some(Some(3))
        );
    }

    #[test]
    fn sample_lookup_tracks_oldest_and_newest_after_full_capacity_eviction() {
        let mut trace = NativeCursorResponsivenessTrace::new(true, 3, 1);
        let evicted = sample(&mut trace, 1);
        let oldest = sample(&mut trace, 2);
        let middle = sample(&mut trace, 3);
        let newest = sample(&mut trace, 4);

        assert!(trace.sample(evicted).is_none());
        assert!(trace.sample_mut(evicted).is_none());
        assert_eq!(
            trace.sample(oldest).map(|sample| sample.sample_id),
            Some(oldest)
        );
        assert_eq!(
            trace.sample(middle).map(|sample| sample.sample_id),
            Some(middle)
        );
        assert_eq!(
            trace.sample(newest).map(|sample| sample.sample_id),
            Some(newest)
        );
        assert_eq!(trace.records.len(), 3);
        assert_eq!(trace.evicted_samples, 1);
    }

    #[test]
    fn sample_mut_changes_oldest_and_newest_retained_records() {
        let mut trace = NativeCursorResponsivenessTrace::new(true, 3, 1);
        let oldest = sample(&mut trace, 1);
        sample(&mut trace, 2);
        let newest = sample(&mut trace, 3);

        trace.sample_mut(oldest).unwrap().raw_motion_events = 101;
        trace.sample_mut(newest).unwrap().raw_motion_events = 303;

        assert_eq!(trace.sample(oldest).unwrap().raw_motion_events, 101);
        assert_eq!(trace.sample(newest).unwrap().raw_motion_events, 303);
    }

    #[test]
    fn sample_ids_do_not_repeat_after_u64_saturation() {
        let mut trace = NativeCursorResponsivenessTrace::new(true, 2, 1);
        trace.next_sample_id = Some(u64::MAX);

        assert_eq!(
            trace.record_motion_batch(1, 1, || Some(timing())),
            Some(u64::MAX)
        );
        assert_eq!(trace.record_motion_batch(1, 1, || Some(timing())), None);
    }

    #[test]
    fn cursor_epoch_updates_the_newest_sample_for_its_output() {
        let mut trace = NativeCursorResponsivenessTrace::new(true, 32, 1);
        let older_output_one = sample_on_output(&mut trace, output_id(), 1);
        for epoch in 1..=15 {
            sample_on_output(&mut trace, other_output_id(), epoch);
        }
        let newest_output_one = sample_on_output(&mut trace, output_id(), 2);

        trace.note_cursor_epoch(output_id(), 3);

        assert_eq!(
            trace.sample(newest_output_one).unwrap().superseded_by_epoch,
            Some(3)
        );
        assert_eq!(
            trace.sample(older_output_one).unwrap().superseded_by_epoch,
            Some(2)
        );
    }

    #[test]
    fn arbitration_evidence_keeps_original_pending_age_on_latest_sample() {
        let mut trace = NativeCursorResponsivenessTrace::new(true, 4, 1);
        let sample_id = sample(&mut trace, 1);
        let mut arbitration = NativeCursorOutputArbitration::default();
        arbitration.request_hardware(1, 100, 200);
        arbitration.request_hardware(2, 220, 300);

        trace.record_arbitration_evidence(
            None,
            output_id(),
            Some(2),
            250,
            NativeCursorSchedulingPolicy::Auto,
            &arbitration,
            super::super::frame::NativeCursorPrimaryState {
                independent_kms_admission: true,
                ..Default::default()
            },
            None,
            NativeCursorOutputDisposition::SubmitPlaneDelta,
        );

        let sample = trace.sample(sample_id).unwrap();
        assert_eq!(sample.cursor_pending_since_ns, Some(100));
        assert_eq!(sample.cursor_age_at_arbitration_ns, Some(150));
        assert_eq!(sample.cursor_service_deadline_ns, Some(200));
        assert_eq!(sample.cursor_overdue_age_ns, Some(50));
        assert_eq!(sample.superseded_by_epoch, Some(2));

        let mut jsonl = String::new();
        super::append_sample_jsonl(&mut jsonl, *sample, None);
        assert!(jsonl.contains("\"overdue_age_ns\":50"));

        let sample = trace.sample_mut(sample_id).unwrap();
        sample.cursor_prepare_budget_ns = Some(400_000);
        sample.cursor_dispatch_budget_ns = Some(250_000);
        sample.cursor_apply_guard_ns = Some(1_100_000);
        sample.cursor_total_lead_ns = Some(1_750_000);
        let mut jsonl = String::new();
        super::append_sample_jsonl(&mut jsonl, *sample, None);
        assert!(jsonl.contains("\"cursor_prepare_budget_ns\":400000"));
        assert!(jsonl.contains("\"cursor_dispatch_budget_ns\":250000"));
        assert!(jsonl.contains("\"cursor_apply_guard_ns\":1100000"));
        assert!(jsonl.contains("\"cursor_total_lead_ns\":1750000"));
    }

    #[test]
    fn cursor_opportunity_trace_records_the_selected_timing_budgets() {
        use crate::native_output::presentation::cursor_timing::CursorTimingDomain;
        use crate::native_output::presentation::kms_timing::KmsModeTiming;
        use crate::native_output::runtime::planner::plan_cursor_service_opportunity;
        use oblivion_one::native::presentation_deadline::{
            MonotonicTimestampNs, PresentationDeadlinePlanner,
        };
        use std::time::Duration;

        const REFRESH_NS: u64 = 10_000_000;
        let now = MonotonicTimestampNs::new(1_000_000_000);
        let mut planner = PresentationDeadlinePlanner::new(Duration::from_nanos(REFRESH_NS));
        planner.note_presented(now);
        let timing = CursorTimingDomain::new(
            KmsModeTiming::from_mode(&drm_sys::drm_mode_modeinfo::default(), REFRESH_NS),
            2,
        );
        let opportunity = plan_cursor_service_opportunity(&planner, &timing, now, None, 250_000)
            .expect("cursor opportunity");
        let mut trace = NativeCursorResponsivenessTrace::new(true, 4, 1);
        let sample_id = sample(&mut trace, 1);
        let mut arbitration = NativeCursorOutputArbitration::default();
        arbitration.request_hardware_for_opportunity(1, now.get(), opportunity);

        trace.record_arbitration_evidence(
            Some(sample_id),
            output_id(),
            Some(1),
            now.get(),
            NativeCursorSchedulingPolicy::Auto,
            &arbitration,
            super::super::frame::NativeCursorPrimaryState::default(),
            Some(opportunity),
            NativeCursorOutputDisposition::SubmitPlaneDelta,
        );

        let sample = trace.sample(sample_id).unwrap();
        assert_eq!(
            sample.cursor_prepare_budget_ns,
            Some(opportunity.prepare_budget_ns)
        );
        assert_eq!(
            sample.cursor_dispatch_budget_ns,
            Some(opportunity.dispatch_budget_ns)
        );
        assert_eq!(
            sample.cursor_apply_guard_ns,
            Some(opportunity.apply_guard_ns)
        );
        assert_eq!(sample.cursor_total_lead_ns, Some(opportunity.total_lead_ns));
        let mut jsonl = String::new();
        super::append_sample_jsonl(&mut jsonl, *sample, None);
        assert!(jsonl.contains(&format!(
            "\"cursor_total_lead_ns\":{}",
            opportunity.total_lead_ns
        )));
    }

    #[test]
    fn cursor_epoch_index_does_not_refer_to_an_evicted_sample() {
        let mut trace = NativeCursorResponsivenessTrace::new(true, 2, 1);
        let evicted_output_one = sample_on_output(&mut trace, output_id(), 1);
        sample_on_output(&mut trace, other_output_id(), 2);
        let retained_output_two = sample_on_output(&mut trace, other_output_id(), 3);

        trace.note_cursor_epoch(output_id(), 4);

        assert!(trace.sample(evicted_output_one).is_none());
        assert_eq!(
            trace
                .sample(retained_output_two)
                .unwrap()
                .superseded_by_epoch,
            None
        );
    }

    #[test]
    #[ignore = "manual capacity-scaling benchmark; reports timings without thresholds"]
    fn benchmark_lookup_cost_across_retained_sample_capacities() {
        use std::time::Instant;

        const ITERATIONS: u64 = 20_000;
        for capacity in [16, 1_024, 16_384, 32_768] {
            let mut trace = NativeCursorResponsivenessTrace::new(true, capacity, 1);
            let mut newest_id = 0;
            for epoch in 1..=capacity as u64 {
                newest_id = sample(&mut trace, epoch);
            }

            let started = Instant::now();
            for _ in 0..ITERATIONS {
                std::hint::black_box(trace.sample(newest_id).map(|sample| sample.sample_id));
            }
            let sample_ns = started.elapsed().as_nanos() / u128::from(ITERATIONS);

            let started = Instant::now();
            for _ in 0..ITERATIONS {
                let sample = trace
                    .sample_mut(newest_id)
                    .expect("newest record remains retained");
                sample.raw_motion_events ^= 1;
                std::hint::black_box(sample.raw_motion_events);
            }
            let sample_mut_ns = started.elapsed().as_nanos() / u128::from(ITERATIONS);

            let started = Instant::now();
            for epoch in 0..ITERATIONS {
                trace.note_cursor_epoch(output_id(), capacity as u64 + epoch + 1);
            }
            let note_epoch_ns = started.elapsed().as_nanos() / u128::from(ITERATIONS);

            eprintln!(
                "cursor trace retained={capacity} iterations={ITERATIONS} sample_ns/op={sample_ns} sample_mut_ns/op={sample_mut_ns} note_cursor_epoch_ns/op={note_epoch_ns}"
            );
        }
    }

    #[test]
    #[ignore = "manual trace-off/on instrumentation-cost benchmark; reports timings without thresholds"]
    fn benchmark_trace_disabled_vs_enabled_motion_batch_cost() {
        use std::time::Instant;

        const MOTION_BATCHES: u64 = 1_000_000;
        let mut disabled =
            NativeCursorResponsivenessTrace::new(false, DEFAULT_CAPACITY, DEFAULT_SAMPLE_STRIDE);
        let started = Instant::now();
        for _ in 0..MOTION_BATCHES {
            let trace = std::hint::black_box(&mut disabled);
            std::hint::black_box(trace.record_motion_batch(1, 1, || {
                panic!("disabled trace must not capture motion timing")
            }));
        }
        let disabled_ns_per_batch = started.elapsed().as_nanos() / u128::from(MOTION_BATCHES);
        assert_eq!(disabled.motion_batches, 0);

        let mut enabled =
            NativeCursorResponsivenessTrace::new(true, DEFAULT_CAPACITY, DEFAULT_SAMPLE_STRIDE);
        let started = Instant::now();
        for batch in 1..=MOTION_BATCHES {
            let sample_id = enabled.record_motion_batch(1, 1, || Some(timing()));
            if let Some(sample_id) = sample_id {
                enabled.complete_motion_batch(
                    sample_id,
                    output_id(),
                    Some(batch),
                    batch,
                    CursorHitTestCounters::default(),
                );
            }
            std::hint::black_box(sample_id);
        }
        let enabled_ns_per_batch = started.elapsed().as_nanos() / u128::from(MOTION_BATCHES);

        eprintln!(
            "cursor trace motion batches={MOTION_BATCHES} sample_stride={} capacity={} disabled_ns/batch={disabled_ns_per_batch} enabled_ns/batch={enabled_ns_per_batch} added_ns/batch={} samples={} evicted_samples={} trace_write_cpu_total_ns={}",
            DEFAULT_SAMPLE_STRIDE,
            DEFAULT_CAPACITY,
            enabled_ns_per_batch.saturating_sub(disabled_ns_per_batch),
            enabled.sampled_batches,
            enabled.evicted_samples,
            enabled.trace_write_cpu_total_ns,
        );
    }

    #[test]
    fn matching_epoch_transaction_and_pageflip_are_attributed() {
        let mut trace = NativeCursorResponsivenessTrace::new(true, 4, 1);
        let sample_id = sample(&mut trace, 9);
        let transaction_id = OutputTransactionId::new(NonZeroU64::new(5).unwrap());
        trace.record_arbitration(
            Some(sample_id),
            output_id(),
            Some(9),
            205,
            NativeCursorOutputDisposition::SubmitPlaneDelta,
        );
        let mut presentation = PresentationTransactionTraceRing::new(8);
        presentation.push(PresentationTransactionEvent::CursorEpochTransactionBuilt {
            output_id: output_id(),
            cursor_epoch: 9,
            transaction_id,
            timestamp_ns: 210,
        });
        presentation.push(PresentationTransactionEvent::KmsSubmitStarted {
            transaction_id,
            timestamp_ns: 220,
        });
        presentation.push(PresentationTransactionEvent::KmsSubmitReturned {
            transaction_id,
            timestamp_ns: 230,
        });
        presentation.push(PresentationTransactionEvent::PageflipPresented {
            transaction_id,
            timestamp_ns: 240,
        });
        presentation.record_cursor_pageflip_sequence(transaction_id, 241);

        let attribution = trace
            .cursor_transaction_attribution(sample_id, &presentation)
            .expect("matching output and cursor epoch have a transaction");
        assert_eq!(attribution.transaction_id, transaction_id);
        assert_eq!(attribution.kms_submit_started_at_ns, Some(220));
        assert_eq!(attribution.kms_submit_returned_at_ns, Some(230));
        assert_eq!(attribution.pageflip_observed_at_ns, Some(240));
        assert_eq!(attribution.pageflip_sequence, Some(241));
        let mut jsonl = String::new();
        super::append_sample_jsonl(
            &mut jsonl,
            *trace.sample(sample_id).unwrap(),
            Some(attribution),
        );
        assert!(jsonl.contains("\"actual_pageflip_sequence\":241"));
    }

    #[test]
    fn mismatched_epoch_or_output_is_not_attributed() {
        let mut trace = NativeCursorResponsivenessTrace::new(true, 4, 1);
        let sample_id = sample(&mut trace, 9);
        let transaction_id = OutputTransactionId::new(NonZeroU64::new(6).unwrap());
        trace.record_arbitration(
            Some(sample_id),
            output_id(),
            Some(9),
            205,
            NativeCursorOutputDisposition::SubmitPlaneDelta,
        );
        let mut wrong_output = PresentationTransactionTraceRing::new(8);
        wrong_output.push(PresentationTransactionEvent::CursorEpochTransactionBuilt {
            output_id: other_output_id(),
            cursor_epoch: 9,
            transaction_id,
            timestamp_ns: 210,
        });
        assert!(
            trace
                .cursor_transaction_attribution(sample_id, &wrong_output)
                .is_none()
        );

        let mut wrong_epoch = PresentationTransactionTraceRing::new(8);
        wrong_epoch.push(PresentationTransactionEvent::CursorEpochTransactionBuilt {
            output_id: output_id(),
            cursor_epoch: 10,
            transaction_id,
            timestamp_ns: 210,
        });
        assert!(
            trace
                .cursor_transaction_attribution(sample_id, &wrong_epoch)
                .is_none()
        );

        let mut mismatched_arbitration = NativeCursorResponsivenessTrace::new(true, 4, 1);
        let mismatched_sample_id = sample(&mut mismatched_arbitration, 9);
        mismatched_arbitration.record_arbitration(
            Some(mismatched_sample_id),
            output_id(),
            Some(10),
            205,
            NativeCursorOutputDisposition::SubmitPlaneDelta,
        );
        let mut matching_transaction = PresentationTransactionTraceRing::new(8);
        matching_transaction.push(PresentationTransactionEvent::CursorEpochTransactionBuilt {
            output_id: output_id(),
            cursor_epoch: 9,
            transaction_id,
            timestamp_ns: 210,
        });
        assert!(
            mismatched_arbitration
                .cursor_transaction_attribution(mismatched_sample_id, &matching_transaction)
                .is_none()
        );
    }

    #[test]
    fn transaction_built_before_arbitration_is_not_attributed() {
        let mut trace = NativeCursorResponsivenessTrace::new(true, 4, 1);
        let sample_id = sample(&mut trace, 9);
        let transaction_id = OutputTransactionId::new(NonZeroU64::new(7).unwrap());
        trace.record_arbitration(
            Some(sample_id),
            output_id(),
            Some(9),
            215,
            NativeCursorOutputDisposition::SubmitPlaneDelta,
        );
        let mut presentation = PresentationTransactionTraceRing::new(8);
        presentation.push(PresentationTransactionEvent::CursorEpochTransactionBuilt {
            output_id: output_id(),
            cursor_epoch: 9,
            transaction_id,
            timestamp_ns: 210,
        });

        assert!(
            trace
                .cursor_transaction_attribution(sample_id, &presentation)
                .is_none()
        );
    }

    #[test]
    fn cursor_history_keeps_pageflip_after_general_events_are_evicted() {
        let mut trace = NativeCursorResponsivenessTrace::new(true, 4, 1);
        let sample_id = sample(&mut trace, 9);
        let transaction_id = OutputTransactionId::new(NonZeroU64::new(8).unwrap());
        trace.record_arbitration(
            Some(sample_id),
            output_id(),
            Some(9),
            205,
            NativeCursorOutputDisposition::SubmitPlaneDelta,
        );
        let mut presentation = PresentationTransactionTraceRing::new(3);
        presentation.push(PresentationTransactionEvent::CursorEpochTransactionBuilt {
            output_id: output_id(),
            cursor_epoch: 9,
            transaction_id,
            timestamp_ns: 210,
        });
        presentation.push(PresentationTransactionEvent::KmsSubmitStarted {
            transaction_id,
            timestamp_ns: 220,
        });
        presentation.push(PresentationTransactionEvent::KmsSubmitReturned {
            transaction_id,
            timestamp_ns: 230,
        });
        presentation.push(PresentationTransactionEvent::PageflipPresented {
            transaction_id,
            timestamp_ns: 240,
        });
        presentation.push(PresentationTransactionEvent::KmsSubmitStarted {
            transaction_id,
            timestamp_ns: 250,
        });
        let attribution = trace
            .cursor_transaction_attribution(sample_id, &presentation)
            .expect("compact cursor history survives unrelated event eviction");
        assert_eq!(attribution.pageflip_observed_at_ns, Some(240));
    }

    #[test]
    fn evicted_cursor_transaction_is_not_attributed() {
        let mut trace = NativeCursorResponsivenessTrace::new(true, 4, 1);
        let sample_id = sample(&mut trace, 9);
        let transaction_id = OutputTransactionId::new(NonZeroU64::new(8).unwrap());
        trace.record_arbitration(
            Some(sample_id),
            output_id(),
            Some(9),
            205,
            NativeCursorOutputDisposition::SubmitPlaneDelta,
        );
        let mut presentation = PresentationTransactionTraceRing::new(1);
        presentation.push(PresentationTransactionEvent::CursorEpochTransactionBuilt {
            output_id: output_id(),
            cursor_epoch: 9,
            transaction_id,
            timestamp_ns: 210,
        });
        presentation.push(PresentationTransactionEvent::CursorEpochTransactionBuilt {
            output_id: output_id(),
            cursor_epoch: 10,
            transaction_id: OutputTransactionId::new(NonZeroU64::new(9).unwrap()),
            timestamp_ns: 220,
        });

        assert!(
            trace
                .cursor_transaction_attribution(sample_id, &presentation)
                .is_none()
        );
    }

    #[test]
    fn newer_desired_epoch_marks_prior_sample_superseded() {
        let mut trace = NativeCursorResponsivenessTrace::new(true, 4, 1);
        let prior_id = sample(&mut trace, 11);
        trace.note_cursor_epoch(output_id(), 12);

        assert_eq!(
            trace.sample(prior_id).unwrap().superseded_by_epoch,
            Some(12)
        );
    }

    #[test]
    fn records_primary_piggyback_and_plane_delta_dispositions() {
        let mut trace = NativeCursorResponsivenessTrace::new(true, 4, 1);
        let primary_sample = sample(&mut trace, 20);
        let plane_delta_sample = sample(&mut trace, 21);
        let mut primary_arbitration = NativeCursorOutputArbitration::default();
        primary_arbitration.request_hardware(20, 0, 10);
        let primary_disposition = cursor_output_disposition_at_deadline(
            &mut primary_arbitration,
            NativeCursorSchedulingPolicy::Piggyback,
            10,
            true,
            true,
        );
        let mut plane_delta_arbitration = NativeCursorOutputArbitration::default();
        plane_delta_arbitration.request_hardware(21, 0, 10);
        let plane_delta_disposition = cursor_output_disposition_at_deadline(
            &mut plane_delta_arbitration,
            NativeCursorSchedulingPolicy::Auto,
            10,
            false,
            true,
        );
        assert_eq!(
            primary_disposition,
            NativeCursorOutputDisposition::PiggybackPrimary
        );
        assert_eq!(
            plane_delta_disposition,
            NativeCursorOutputDisposition::SubmitPlaneDelta
        );
        trace.record_arbitration(
            Some(primary_sample),
            output_id(),
            Some(20),
            300,
            primary_disposition,
        );
        trace.record_arbitration(
            Some(plane_delta_sample),
            output_id(),
            Some(21),
            310,
            plane_delta_disposition,
        );

        assert_eq!(
            trace.sample(primary_sample).unwrap().disposition,
            Some(NativeCursorOutputDisposition::PiggybackPrimary)
        );
        assert_eq!(
            trace.sample(plane_delta_sample).unwrap().disposition,
            Some(NativeCursorOutputDisposition::SubmitPlaneDelta)
        );
    }
}
