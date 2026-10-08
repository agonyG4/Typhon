use super::frame::NativeCursorOutputArbitration;
use super::frame::NativeCursorOutputDisposition;
use crate::native_output::presentation::trace::{
    CursorEpochPresentationTrace, PresentationTransactionTraceRing,
};
use crate::native_output::presentation::transaction::OutputTransactionId;
use oblivion_one::compositor::PointerInputMetricsSnapshot;
use oblivion_one::core::OutputId;
use std::{collections::VecDeque, fmt::Write as _, path::PathBuf};

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
    pub(crate) groups_inspected: u64,
    pub(crate) surfaces_inspected: u64,
    pub(crate) cpu_ns: u64,
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
            groups_inspected: self
                .groups_inspected
                .saturating_sub(before.groups_inspected),
            surfaces_inspected: self
                .surfaces_inspected
                .saturating_sub(before.surfaces_inspected),
            cpu_ns: self.cpu_ns.saturating_sub(before.cpu_ns),
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
            groups_inspected: snapshot.groups_inspected,
            surfaces_inspected: snapshot.surfaces_inspected,
            cpu_ns: snapshot.cpu_ns,
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
    pub(crate) superseded_by_epoch: Option<u64>,
    pub(crate) effect_scene_resolution_ns: Option<u64>,
    pub(crate) effect_scene_visible_instances: Option<usize>,
    /// CPU time spent creating and retaining this sampled record.
    pub(crate) trace_write_cpu_ns: Option<u64>,
}

#[derive(Debug)]
pub(crate) struct NativeCursorResponsivenessTrace {
    enabled: bool,
    capacity: usize,
    sample_stride: u64,
    records: VecDeque<CursorResponsivenessSample>,
    next_sample_id: u64,
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
            next_sample_id: 1,
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

        let trace_start_cpu_ns = super::pointer_timing::thread_cpu_time_ns();
        let timing = capture()?;
        let sample_id = self.next_sample_id;
        self.next_sample_id = self.next_sample_id.saturating_add(1);
        self.sampled_batches = self.sampled_batches.saturating_add(1);
        if self.records.len() == self.capacity {
            self.records.pop_front();
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
            superseded_by_epoch: None,
            effect_scene_resolution_ns: None,
            effect_scene_visible_instances: None,
            trace_write_cpu_ns: None,
        });
        let elapsed_cpu_ns = trace_start_cpu_ns
            .zip(super::pointer_timing::thread_cpu_time_ns())
            .map(|(start, end)| end.saturating_sub(start));
        if let Some(sample) = self.sample_mut(sample_id) {
            sample.trace_write_cpu_ns = elapsed_cpu_ns;
        }
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
    }

    pub(crate) fn note_cursor_epoch(&mut self, output_id: OutputId, epoch: u64) {
        if !self.enabled {
            return;
        }
        for sample in self.records.iter_mut().rev() {
            if sample.output_id != Some(output_id) {
                continue;
            }
            if let Some(previous_epoch) = sample.cursor_epoch {
                if epoch > previous_epoch {
                    sample.superseded_by_epoch = Some(epoch);
                }
                break;
            }
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
        self.records
            .iter()
            .find(|sample| sample.sample_id == sample_id)
    }

    fn sample_mut(&mut self, sample_id: u64) -> Option<&mut CursorResponsivenessSample> {
        self.records
            .iter_mut()
            .find(|sample| sample.sample_id == sample_id)
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
            "{{\"type\":\"cumulative_counter_deltas\",\"hit_test_calls\":{},\"hit_test_cache_hits\":{},\"hit_test_cache_misses\":{},\"hit_test_full_scene_scans\":{},\"hit_test_owner_locality_fast_hits\":{},\"hit_test_groups_inspected\":{},\"hit_test_surfaces_inspected\":{},\"hit_test_cpu_ns\":{},\"arbitration_response_windows_opened\":{},\"arbitration_changes_coalesced\":{},\"arbitration_plane_delta_plans\":{},\"arbitration_plane_delta_submissions\":{},\"arbitration_deferred_for_primary\":{},\"arbitration_piggybacked\":{},\"arbitration_idle_hardware_updates\":{},\"arbitration_idle_software_updates\":{}}}",
            hit_test_delta.calls,
            hit_test_delta.cache_hits,
            hit_test_delta.cache_misses,
            hit_test_delta.full_scene_scans,
            hit_test_delta.owner_locality_fast_hits,
            hit_test_delta.groups_inspected,
            hit_test_delta.surfaces_inspected,
            hit_test_delta.cpu_ns,
            arbitration_delta.response_windows_opened,
            arbitration_delta.changes_coalesced,
            arbitration_delta.plane_delta_plans,
            arbitration_delta.plane_delta_submissions,
            arbitration_delta.plane_delta_deferred_for_primary,
            arbitration_delta.cursor_state_piggybacked,
            arbitration_delta.idle_hardware_updates,
            arbitration_delta.idle_software_updates,
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
        let mut missing_transaction = 0u64;
        let mut missing_submit = 0u64;
        let mut missing_pageflip = 0u64;
        for sample in &self.records {
            let Some(attribution) = self.indexed_attribution(sample, &attribution_index) else {
                missing_transaction = missing_transaction.saturating_add(1);
                continue;
            };
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
    let _ = writeln!(
        output,
        "{{\"type\":\"sample\",\"sample_id\":{},\"raw_motion_events\":{},\"coalesced_motion_events\":{},\"source_oldest_timestamp_usec\":{},\"source_newest_timestamp_usec\":{},\"input_batch_materialized_at_ns\":{},\"dispatch_started_at_ns\":{},\"dispatch_completed_at_ns\":{},\"dispatch_duration_ns\":{},\"input_batch_to_dispatch_complete_ns\":{},\"dispatch_to_arbitration_ns\":{},\"dispatch_complete_to_arbitration_ns\":{},\"hit_test_calls\":{},\"hit_test_cache_hits\":{},\"hit_test_cache_misses\":{},\"hit_test_full_scene_scans\":{},\"hit_test_owner_locality_fast_hits\":{},\"hit_test_groups_inspected\":{},\"hit_test_surfaces_inspected\":{},\"hit_test_cpu_ns\":{},\"effect_scene_resolution_ns\":{},\"effect_scene_visible_instances\":{},\"output_id\":{},\"cursor_epoch\":{},\"arbitration_cursor_epoch\":{},\"arbitration_at_ns\":{},\"disposition\":\"{}\",\"superseded_by_epoch\":{},\"trace_write_cpu_ns\":{},\"cursor_transaction_id\":{},\"cursor_transaction_built_at_ns\":{},\"worker_queued_at_ns\":{},\"kms_submit_started_at_ns\":{},\"kms_submit_returned_at_ns\":{},\"pageflip_observed_at_ns\":{},\"kms_queue_wait_ns\":{},\"kms_submit_duration_ns\":{},\"submit_return_to_pageflip_observation_ns\":{},\"pageflip_timestamp_kind\":\"compositor_observed_not_optical\"}}",
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
            |value| value.groups_inspected.to_string()
        ),
        sample.hit_test_delta.map_or_else(
            || "null".to_owned(),
            |value| value.surfaces_inspected.to_string()
        ),
        json_number(hit_test_cpu_ns),
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
        json_number(kms_queue_wait_ns),
        json_number(kms_submit_duration_ns),
        json_number(submit_return_to_pageflip_ns),
    );
}

fn json_number(value: Option<u64>) -> String {
    value.map_or_else(|| "null".to_owned(), |number| number.to_string())
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
    use super::{CursorHitTestCounters, CursorMotionBatchTiming, NativeCursorResponsivenessTrace};
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
        let id = trace
            .record_motion_batch(1, 1, || Some(timing()))
            .expect("stride one retains each nonempty batch");
        trace.complete_motion_batch(
            id,
            output_id(),
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

        let attribution = trace
            .cursor_transaction_attribution(sample_id, &presentation)
            .expect("matching output and cursor epoch have a transaction");
        assert_eq!(attribution.transaction_id, transaction_id);
        assert_eq!(attribution.kms_submit_started_at_ns, Some(220));
        assert_eq!(attribution.kms_submit_returned_at_ns, Some(230));
        assert_eq!(attribution.pageflip_observed_at_ns, Some(240));
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
            NativeCursorSchedulingPolicy::Auto,
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
