#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_ids_are_nonzero_and_wrap_to_one() {
        let mut ids = NativeOutputFrameIdSequence::new(u64::MAX);
        assert_eq!(ids.next().get(), u64::MAX);
        assert_eq!(ids.next().get(), 1);
    }

    #[test]
    fn bounded_samples_report_nearest_rank_percentiles() {
        let mut samples = BoundedSamples::<4>::default();
        for sample in [40, 10, 30, 20, 50] {
            samples.record(sample);
        }
        assert_eq!(samples.len(), 4);
        assert_eq!(samples.percentiles(), (20, 50, 50));
    }

    #[test]
    fn refresh_misses_use_documented_half_interval_tolerance() {
        let mut misses = RefreshMissBuckets::default();
        for interval in [9_000, 9_001, 15_000, 15_001, 21_000, 21_001] {
            misses.record(interval, 6_000);
        }
        assert_eq!(misses.on_time, 1);
        assert_eq!(misses.missed_1x, 2);
        assert_eq!(misses.missed_2x, 2);
        assert_eq!(misses.missed_3x_or_more, 1);
    }

    #[test]
    fn pacing_line_is_compact_and_prefixed() {
        let line = pacing_line(
            "wait_for_buffer",
            &[PacingField::u64("frame_id", 7), PacingField::none("ready")],
        );
        assert_eq!(
            line,
            "typhon pacing: event=wait_for_buffer frame_id=7 ready=none"
        );
    }

    #[test]
    fn snapshot_fields_use_stable_slot_values_only() {
        let fields = snapshot_fields(NativeScanoutBufferSnapshot {
            backend: super::super::scanout::NativeScanoutKind::AtomicEglGbmExplicit,
            capacity: None,
            current: None,
            pending: None,
            ready: None,
            free_count: None,
            gbm_surface_has_free_buffers: Some(false),
        });
        assert_eq!(
            pacing_line("decision", &fields),
            "typhon pacing: event=decision backend=atomic-egl-gbm-explicit capacity=none current=none pending=none ready=none free_count=none gbm_surface_has_free_buffers=false"
        );
    }
}
use super::scanout::NativeScanoutBufferSnapshot;
use oblivion_one::native::adaptive_buffering::{
    AdaptiveBufferingMode, FenceTimestampQuality, ProvenDeadlineMiss,
};
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct NativeOutputFrameId(u64);

impl NativeOutputFrameId {
    pub(crate) const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug)]
pub(crate) struct NativeOutputFrameIdSequence {
    next: u64,
}

impl NativeOutputFrameIdSequence {
    pub(crate) const fn new(next: u64) -> Self {
        Self { next }
    }

    pub(crate) fn next(&mut self) -> NativeOutputFrameId {
        let id = NativeOutputFrameId(self.next.max(1));
        self.next = id.0.checked_add(1).unwrap_or(1);
        id
    }
}

#[derive(Debug)]
pub(crate) struct BoundedSamples<const N: usize> {
    values: VecDeque<u64>,
}

impl<const N: usize> Default for BoundedSamples<N> {
    fn default() -> Self {
        Self {
            values: VecDeque::with_capacity(N),
        }
    }
}

impl<const N: usize> BoundedSamples<N> {
    pub(crate) fn record(&mut self, value: u64) {
        if N == 0 {
            return;
        }
        if self.values.len() == N {
            self.values.pop_front();
        }
        self.values.push_back(value);
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.values.len()
    }

    pub(crate) fn percentiles(&self) -> (u64, u64, u64) {
        let mut values: Vec<_> = self.values.iter().copied().collect();
        values.sort_unstable();
        let percentile = |percent: usize| {
            if values.is_empty() {
                return 0;
            }
            let rank = (percent * values.len()).div_ceil(100).max(1);
            values[rank - 1]
        };
        (percentile(50), percentile(95), percentile(99))
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RefreshMissBuckets {
    pub(crate) on_time: u64,
    pub(crate) missed_1x: u64,
    pub(crate) missed_2x: u64,
    pub(crate) missed_3x_or_more: u64,
}

impl RefreshMissBuckets {
    pub(crate) fn record(&mut self, elapsed_us: u64, refresh_interval_us: u64) {
        if refresh_interval_us == 0 {
            return;
        }
        let twice = elapsed_us.saturating_mul(2);
        if twice <= refresh_interval_us.saturating_mul(3) {
            self.on_time += 1;
        } else if twice <= refresh_interval_us.saturating_mul(5) {
            self.missed_1x += 1;
        } else if twice <= refresh_interval_us.saturating_mul(7) {
            self.missed_2x += 1;
        } else {
            self.missed_3x_or_more += 1;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PacingField {
    key: &'static str,
    value: String,
}

impl PacingField {
    pub(crate) fn str(key: &'static str, value: impl Into<String>) -> Self {
        Self {
            key,
            value: value.into(),
        }
    }
    pub(crate) fn u64(key: &'static str, value: u64) -> Self {
        Self::str(key, value.to_string())
    }
    pub(crate) fn usize(key: &'static str, value: usize) -> Self {
        Self::str(key, value.to_string())
    }
    pub(crate) fn bool(key: &'static str, value: bool) -> Self {
        Self::str(key, if value { "true" } else { "false" })
    }
    pub(crate) fn option_usize(key: &'static str, value: Option<usize>) -> Self {
        value.map_or_else(|| Self::none(key), |v| Self::usize(key, v))
    }
    pub(crate) fn option_bool(key: &'static str, value: Option<bool>) -> Self {
        value.map_or_else(|| Self::none(key), |v| Self::bool(key, v))
    }
    pub(crate) fn none(key: &'static str) -> Self {
        Self::str(key, "none")
    }
}

pub(crate) fn pacing_line(event: &str, fields: &[PacingField]) -> String {
    let mut line = format!("typhon pacing: event={event}");
    for field in fields {
        line.push(' ');
        line.push_str(field.key);
        line.push('=');
        line.push_str(&field.value);
    }
    line
}

pub(crate) fn snapshot_fields(snapshot: NativeScanoutBufferSnapshot) -> Vec<PacingField> {
    vec![
        PacingField::str("backend", snapshot.backend.metric_name()),
        PacingField::option_usize("capacity", snapshot.capacity),
        PacingField::option_usize("current", snapshot.current),
        PacingField::option_usize("pending", snapshot.pending),
        PacingField::option_usize("ready", snapshot.ready),
        PacingField::option_usize("free_count", snapshot.free_count),
        PacingField::option_bool(
            "gbm_surface_has_free_buffers",
            snapshot.gbm_surface_has_free_buffers,
        ),
    ]
}

pub(crate) fn frame_id_field(frame_id: Option<NativeOutputFrameId>) -> PacingField {
    frame_id.map_or_else(
        || PacingField::none("frame_id"),
        |id| PacingField::u64("frame_id", id.get()),
    )
}

const PACING_SAMPLE_CAPACITY: usize = 4096;

#[derive(Debug)]
pub(crate) struct NativeFramePacing {
    enabled: bool,
    ids: NativeOutputFrameIdSequence,
    pub(crate) active: Option<NativeOutputFrameId>,
    pub(crate) active_queued_ns: Option<u64>,
    pub(crate) pending: Option<NativeOutputFrameId>,
    pub(crate) ready: Option<NativeOutputFrameId>,
    pub(crate) render_ahead_attempts: u64,
    pub(crate) render_ahead_successes: u64,
    pub(crate) wait_for_buffer_count: u64,
    pub(crate) ready_submit_count: u64,
    pub(crate) adaptive_triple_entries_predicted: u64,
    pub(crate) adaptive_triple_entries_proven_render_miss: u64,
    pub(crate) adaptive_triple_entries_proven_submit_miss: u64,
    pub(crate) adaptive_triple_exits: u64,
    pub(crate) sync_file_info_exact: u64,
    pub(crate) sync_file_info_approximate: u64,
    wake_lateness: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    slot_hold: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    ready_age: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    target_error: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    atomic_submit: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    pageflip_intervals: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    commit_to_present: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    misses: RefreshMissBuckets,
    last_pageflip_ns: Option<u64>,
}

impl NativeFramePacing {
    pub(crate) fn from_env() -> Self {
        Self {
            enabled: std::env::var("TYPHON_FRAME_PACING_DEBUG")
                .ok()
                .is_some_and(|value| super::perf::native_perf_log_value_enabled(&value)),
            ids: NativeOutputFrameIdSequence::new(1),
            active: None,
            active_queued_ns: None,
            pending: None,
            ready: None,
            render_ahead_attempts: 0,
            render_ahead_successes: 0,
            wait_for_buffer_count: 0,
            ready_submit_count: 0,
            adaptive_triple_entries_predicted: 0,
            adaptive_triple_entries_proven_render_miss: 0,
            adaptive_triple_entries_proven_submit_miss: 0,
            adaptive_triple_exits: 0,
            sync_file_info_exact: 0,
            sync_file_info_approximate: 0,
            wake_lateness: BoundedSamples::default(),
            slot_hold: BoundedSamples::default(),
            ready_age: BoundedSamples::default(),
            target_error: BoundedSamples::default(),
            atomic_submit: BoundedSamples::default(),
            pageflip_intervals: BoundedSamples::default(),
            commit_to_present: BoundedSamples::default(),
            misses: RefreshMissBuckets::default(),
            last_pageflip_ns: None,
        }
    }

    pub(crate) const fn enabled(&self) -> bool {
        self.enabled
    }
    pub(crate) fn queue_visual(&mut self, now_ns: u64, render_generation: u64) {
        if !self.enabled {
            return;
        }
        if self.active.is_some() {
            return;
        }
        let id = self.ids.next();
        self.active = Some(id);
        self.active_queued_ns = Some(now_ns);
        self.log(
            "visual_queued",
            vec![
                PacingField::u64("frame_id", id.get()),
                PacingField::u64("render_generation", render_generation),
            ],
        );
    }
    pub(crate) fn log(&self, event: &str, fields: Vec<PacingField>) {
        if self.enabled {
            println!("{}", pacing_line(event, &fields));
        }
    }
    pub(crate) fn note_submit(&mut self, token: u64, now_ns: u64, ready_submit: bool) {
        if !self.enabled {
            return;
        }
        let id = if ready_submit {
            self.ready.take()
        } else {
            self.active.take()
        };
        if ready_submit {
            self.ready_submit_count += 1;
        }
        self.pending = id;
        self.active_queued_ns = None;
        self.log(
            "submit",
            vec![
                frame_id_field(id),
                PacingField::u64("pageflip_token", token),
                PacingField::u64("submit_ns", now_ns),
                PacingField::bool("ready_submit", ready_submit),
            ],
        );
    }
    pub(crate) fn note_render_ahead_ready(&mut self, now_ns: u64) {
        if !self.enabled {
            return;
        }
        self.render_ahead_successes += 1;
        self.ready = self.active.take();
        self.active_queued_ns = None;
        self.log(
            "ready_queued",
            vec![
                frame_id_field(self.ready),
                PacingField::u64("render_end_ns", now_ns),
            ],
        );
    }
    pub(crate) fn note_pageflip(
        &mut self,
        now_ns: u64,
        submitted_at_ns: u64,
        token: u64,
        refresh_interval_us: u64,
    ) {
        if !self.enabled {
            return;
        }
        if let Some(last) = self.last_pageflip_ns {
            let us = now_ns.saturating_sub(last) / 1_000;
            self.pageflip_intervals.record(us);
            self.misses.record(us, refresh_interval_us);
        }
        self.last_pageflip_ns = Some(now_ns);
        let commit_us = now_ns.saturating_sub(submitted_at_ns) / 1_000;
        self.commit_to_present.record(commit_us);
        let id = self.pending.take();
        self.log(
            "pageflip_complete",
            vec![
                frame_id_field(id),
                PacingField::u64("pageflip_token", token),
                PacingField::u64("pageflip_complete_ns", now_ns),
                PacingField::u64("commit_to_present_us", commit_us),
            ],
        );
    }
    pub(crate) fn last_pageflip_ns(&self) -> Option<u64> {
        self.last_pageflip_ns
    }
    pub(crate) fn note_wake_lateness(&mut self, lateness_ns: u64) {
        if self.enabled {
            self.wake_lateness.record(lateness_ns / 1_000);
        }
    }
    pub(crate) fn note_explicit_present(
        &mut self,
        target_ns: u64,
        presented_ns: u64,
        composite_started_ns: u64,
        rendered_ns: u64,
        submit_started_ns: u64,
        submit_returned_ns: u64,
    ) {
        if !self.enabled {
            return;
        }
        self.target_error
            .record(presented_ns.abs_diff(target_ns) / 1_000);
        self.slot_hold
            .record(submit_returned_ns.saturating_sub(composite_started_ns) / 1_000);
        self.ready_age
            .record(submit_started_ns.saturating_sub(rendered_ns) / 1_000);
        self.atomic_submit
            .record(submit_returned_ns.saturating_sub(submit_started_ns) / 1_000);
    }
    pub(crate) fn note_adaptive_transition(
        &mut self,
        before: AdaptiveBufferingMode,
        after: AdaptiveBufferingMode,
        miss: Option<ProvenDeadlineMiss>,
    ) {
        if !self.enabled || before == after {
            return;
        }
        match (before, after, miss) {
            (AdaptiveBufferingMode::Double, AdaptiveBufferingMode::Triple, None) => {
                self.adaptive_triple_entries_predicted += 1;
            }
            (
                AdaptiveBufferingMode::Double,
                AdaptiveBufferingMode::Triple,
                Some(ProvenDeadlineMiss::AtomicSubmit),
            ) => self.adaptive_triple_entries_proven_submit_miss += 1,
            (AdaptiveBufferingMode::Double, AdaptiveBufferingMode::Triple, Some(_)) => {
                self.adaptive_triple_entries_proven_render_miss += 1;
            }
            (AdaptiveBufferingMode::Triple, AdaptiveBufferingMode::Double, _) => {
                self.adaptive_triple_exits += 1;
            }
            _ => {}
        }
    }
    pub(crate) fn note_fence_timestamp_quality(&mut self, quality: FenceTimestampQuality) {
        if !self.enabled {
            return;
        }
        match quality {
            FenceTimestampQuality::ExactSyncFile => self.sync_file_info_exact += 1,
            FenceTimestampQuality::ObservedApproximate => self.sync_file_info_approximate += 1,
        }
    }
    pub(crate) fn summary_line(&self) -> String {
        let (pf50, pf95, pf99) = self.pageflip_intervals.percentiles();
        let (cp50, cp95, cp99) = self.commit_to_present.percentiles();
        let (wake50, wake95, wake99) = self.wake_lateness.percentiles();
        let (slot50, slot95, slot99) = self.slot_hold.percentiles();
        let (ready50, ready95, ready99) = self.ready_age.percentiles();
        let (target50, target95, target99) = self.target_error.percentiles();
        let (submit50, submit95, submit99) = self.atomic_submit.percentiles();
        pacing_line(
            "summary",
            &[
                PacingField::u64("render_ahead_attempts", self.render_ahead_attempts),
                PacingField::u64("render_ahead_successes", self.render_ahead_successes),
                PacingField::u64("wait_for_buffer_count", self.wait_for_buffer_count),
                PacingField::u64("ready_submit_count", self.ready_submit_count),
                PacingField::u64(
                    "adaptive_triple_entries_predicted",
                    self.adaptive_triple_entries_predicted,
                ),
                PacingField::u64(
                    "adaptive_triple_entries_proven_render_miss",
                    self.adaptive_triple_entries_proven_render_miss,
                ),
                PacingField::u64(
                    "adaptive_triple_entries_proven_submit_miss",
                    self.adaptive_triple_entries_proven_submit_miss,
                ),
                PacingField::u64("adaptive_triple_exits", self.adaptive_triple_exits),
                PacingField::u64("sync_file_info_exact", self.sync_file_info_exact),
                PacingField::u64(
                    "sync_file_info_approximate",
                    self.sync_file_info_approximate,
                ),
                PacingField::u64("presentation_target_sequence_mutations", 0),
                PacingField::u64("scheduler_wakeup_lateness_p50_us", wake50),
                PacingField::u64("scheduler_wakeup_lateness_p95_us", wake95),
                PacingField::u64("scheduler_wakeup_lateness_p99_us", wake99),
                PacingField::u64("slot_hold_p50_us", slot50),
                PacingField::u64("slot_hold_p95_us", slot95),
                PacingField::u64("slot_hold_p99_us", slot99),
                PacingField::u64("ready_age_p50_us", ready50),
                PacingField::u64("ready_age_p95_us", ready95),
                PacingField::u64("ready_age_p99_us", ready99),
                PacingField::u64("target_error_p50_us", target50),
                PacingField::u64("target_error_p95_us", target95),
                PacingField::u64("target_error_p99_us", target99),
                PacingField::u64("atomic_submit_p50_us", submit50),
                PacingField::u64("atomic_submit_p95_us", submit95),
                PacingField::u64("atomic_submit_p99_us", submit99),
                PacingField::u64("pageflip_interval_p50_us", pf50),
                PacingField::u64("pageflip_interval_p95_us", pf95),
                PacingField::u64("pageflip_interval_p99_us", pf99),
                PacingField::u64("commit_to_present_p50_us", cp50),
                PacingField::u64("commit_to_present_p95_us", cp95),
                PacingField::u64("commit_to_present_p99_us", cp99),
                PacingField::u64("missed_refresh_1x", self.misses.missed_1x),
                PacingField::u64("missed_refresh_2x", self.misses.missed_2x),
                PacingField::u64("missed_refresh_3x_or_more", self.misses.missed_3x_or_more),
            ],
        )
    }
}
