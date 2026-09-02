#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

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
    fn long_idle_gap_is_not_classified_as_an_active_refresh_miss() {
        assert!(is_active_refresh_interval(18_181, 6_060));
        assert!(!is_active_refresh_interval(60_000, 6_060));
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

    #[test]
    fn verbose_trace_drops_when_full_without_blocking() {
        let (sender, _receiver) = sync_channel(1);
        let sink = NativeTraceSink {
            sender,
            dropped: Arc::new(AtomicU64::new(0)),
        };
        sink.send("queued".to_string());
        let started = Instant::now();
        sink.send("dropped".to_string());

        assert_eq!(sink.dropped_entries(), 1);
        assert!(started.elapsed().as_millis() < 50);
    }

    #[test]
    fn reactive_double_metrics_never_report_predictive_or_ready_work() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.queue_visual(1, 1);
        pacing.note_render_started(NativeOutputPacingMode::ReactiveDouble, false);
        pacing.note_submit(41, 2, false, NativeOutputPacingMode::ReactiveDouble);

        assert_eq!(pacing.reactive_double_frames, 1);
        assert_eq!(pacing.reactive_double_immediate_submits, 1);
        assert_eq!(pacing.predictive_render_ahead_attempts, 0);
        assert_eq!(pacing.predictive_render_ahead_ready, 0);
        assert_eq!(pacing.predictive_ready_submits, 0);
        assert_eq!(pacing.normal_ready_wait_count, 0);
    }

    #[test]
    fn predictive_ready_count_cannot_exceed_attempt_count() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.queue_visual(1, 1);
        pacing.note_render_started(NativeOutputPacingMode::PredictiveTriple, true);
        pacing.note_ready_frame(2, true);
        pacing.note_submit(41, 3, true, NativeOutputPacingMode::PredictiveTriple);

        assert_eq!(pacing.predictive_render_ahead_attempts, 1);
        assert_eq!(pacing.predictive_render_ahead_ready, 1);
        assert_eq!(pacing.predictive_ready_submits, 1);
        assert!(pacing.predictive_render_ahead_ready <= pacing.predictive_render_ahead_attempts);
    }

    #[test]
    fn worker_pacing_submit_records_pending_only_after_success() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.queue_visual(1, 1);

        assert!(pacing.pending.is_none());
        pacing.note_worker_submit(41, 3, false, NativeOutputPacingMode::ReactiveDouble);
        assert!(pacing.pending.is_some());
    }

    #[test]
    fn worker_submit_settles_reserved_frame_after_active_becomes_ready() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.queue_visual(1, 1);
        let reserved = pacing
            .reserve_worker_submission(false)
            .expect("worker reservation should be available");

        pacing.note_ready_frame(2, true);
        assert_eq!(pacing.worker_submission_frame_id(true), reserved);

        pacing
            .note_worker_submit_exact(
                reserved,
                41,
                3,
                false,
                NativeOutputPacingMode::PredictiveTriple,
            )
            .expect("the immutable worker reservation should settle once");
        assert!(pacing.ready.is_none());
        assert_eq!(pacing.pending.map(NativeOutputFrameId::get), reserved);
    }

    #[test]
    fn worker_cancel_settles_reserved_frame_after_active_becomes_ready() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.queue_visual(1, 1);
        let reserved = pacing
            .reserve_worker_submission(false)
            .expect("worker reservation should be available");

        pacing.note_ready_frame(2, false);
        assert!(pacing.cancel_worker_submission(reserved, false));
        assert!(pacing.active.is_none());
        assert!(pacing.ready.is_none());
        assert!(pacing.ready_waiting_started_ns.is_none());
    }

    #[test]
    fn stale_worker_reservation_cannot_settle_or_remove_newer_frame() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.queue_visual(1, 1);
        let stale = pacing
            .reserve_worker_submission(false)
            .expect("worker reservation should be available");
        assert!(pacing.cancel_worker_submission(stale, false));

        pacing.queue_visual(2, 2);
        let current = pacing.worker_submission_frame_id(false);
        assert_ne!(stale, current);
        assert!(
            pacing
                .note_worker_submit_exact(
                    stale,
                    41,
                    3,
                    false,
                    NativeOutputPacingMode::ReactiveDouble,
                )
                .is_err()
        );
        assert_eq!(pacing.worker_submission_frame_id(false), current);
        assert!(pacing.pending.is_none());
    }

    #[test]
    fn worker_reservation_settles_exactly_once() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.queue_visual(1, 1);
        let reserved = pacing
            .reserve_worker_submission(false)
            .expect("worker reservation should be available");
        pacing
            .note_worker_submit_exact(
                reserved,
                41,
                2,
                false,
                NativeOutputPacingMode::ReactiveDouble,
            )
            .unwrap();

        assert!(
            pacing
                .note_worker_submit_exact(
                    reserved,
                    42,
                    3,
                    false,
                    NativeOutputPacingMode::ReactiveDouble,
                )
                .is_err()
        );
        assert_eq!(pacing.pending.map(NativeOutputFrameId::get), reserved);
    }

    #[test]
    fn unreserved_worker_submission_does_not_disturb_active_pacing() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.queue_visual(1, 1);
        let active = pacing.worker_submission_frame_id(false);

        assert!(pacing.cancel_worker_submission(None, true));
        pacing
            .note_worker_submit_exact(None, 41, 2, true, NativeOutputPacingMode::ReactiveDouble)
            .expect("a compatibility job without a pacing reservation is valid");

        assert_eq!(pacing.worker_submission_frame_id(false), active);
        assert!(pacing.ready.is_none());
        assert!(pacing.pending.is_none());
    }

    #[test]
    fn rejected_worker_submission_clears_active_identity_and_ready_timing() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.queue_visual(1, 1);
        let active = pacing.reserve_worker_submission(false).unwrap();
        assert!(pacing.cancel_worker_submission(active, false));
        assert!(pacing.active.is_none());
        assert!(pacing.active_queued_ns.is_none());

        pacing.queue_visual(2, 2);
        pacing.note_ready_frame(3, false);
        let ready = pacing.reserve_worker_submission(true).unwrap();
        assert!(pacing.cancel_worker_submission(ready, true));
        assert!(pacing.ready.is_none());
        assert!(pacing.ready_waiting_started_ns.is_none());
    }

    #[test]
    fn rejected_worker_submission_does_not_leave_pending_presentation_state() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.queue_visual(1, 1);
        pacing.note_worker_submit(41, 2, false, NativeOutputPacingMode::ReactiveDouble);
        assert!(pacing.pending.is_some());
        assert!(pacing.abandon_pending_submission(41));
        assert!(pacing.pending.is_none());
    }

    #[test]
    fn ready_worker_submit_records_wait_duration_before_clearing_timing() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.queue_visual(1, 1);
        pacing.note_ready_frame(1_000, false);
        let reserved = pacing.reserve_worker_submission(true).unwrap();

        pacing
            .note_worker_submit_exact(
                reserved,
                41,
                51_000,
                true,
                NativeOutputPacingMode::PredictiveTriple,
            )
            .unwrap();

        assert_eq!(pacing.ready_waiting_for_target.percentiles(), (50, 50, 50));
        assert!(pacing.ready_waiting_started_ns.is_none());
    }

    #[test]
    fn pacing_summary_exports_reactive_and_deadline_owner_counters() {
        let summary = NativeFramePacing::from_env().summary_line(0);
        for field in [
            "reactive_double_frames=0",
            "reactive_double_immediate_submits=0",
            "reactive_double_actual_misses=0",
            "predictive_render_ahead_attempts=0",
            "predictive_render_ahead_ready=0",
            "predictive_ready_submits=0",
            "normal_ready_wait_count=0",
            "scheduled_normal_target_count=0",
            "expired_deadline_wait_count=0",
            "repeated_immediate_timer_wake_count=0",
            "multiple_deadline_owner_violation_count=0",
            "active_pageflip_interval_p50_us=0",
            "active_pageflip_interval_p95_us=0",
            "active_pageflip_interval_p99_us=0",
            "adaptive_triple_entries_proven_presentation_miss=0",
        ] {
            assert!(summary.contains(field), "missing summary field {field}");
        }
    }

    #[test]
    fn pacing_summary_exports_exact_pipeline_wait_reasons() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.note_pipeline_wait(PipelineWaitReason::FuturePrimaryDepthFull);
        pacing.note_pipeline_wait(PipelineWaitReason::KernelCommitPending);

        let summary = pacing.summary_line(0);
        assert!(summary.contains("pipeline_wait_future_primary_depth_full=1"));
        assert!(summary.contains("pipeline_wait_kernel_commit_pending=1"));
        assert!(summary.contains("pipeline_wait_direct_steady_state=0"));
    }

    #[test]
    fn presentation_miss_entry_has_a_dedicated_adaptive_metric() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;

        pacing.note_adaptive_transition(
            AdaptiveBufferingMode::Double,
            AdaptiveBufferingMode::Triple,
            Some(ProvenDeadlineMiss::KmsApplyGuard),
        );

        assert_eq!(pacing.adaptive_triple_entries_predicted, 0);
        assert_eq!(pacing.adaptive_triple_entries_proven_render_miss, 0);
        assert_eq!(pacing.adaptive_triple_entries_proven_submit_miss, 0);
        assert_eq!(pacing.adaptive_triple_entries_proven_presentation_miss, 1);
    }

    #[test]
    fn deadline_state_stress_has_no_expired_wait_or_immediate_wake_loop() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        for frame in 0..1_000_u64 {
            let now = frame * 6_060_606;
            pacing.note_deadline_state(SchedulerDecision::Render, now, None, None, false, false);
        }

        assert_eq!(pacing.expired_deadline_wait_count, 0);
        assert_eq!(pacing.repeated_immediate_timer_wake_count, 0);
        assert_eq!(pacing.multiple_deadline_owner_violation_count, 0);
    }

    #[test]
    fn deadline_diagnostics_count_each_forbidden_state() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.note_deadline_state(
            SchedulerDecision::WaitForRefresh,
            10,
            None,
            Some(9),
            true,
            true,
        );
        pacing.note_deadline_state(
            SchedulerDecision::WaitForRefresh,
            10,
            None,
            Some(9),
            true,
            true,
        );

        assert_eq!(pacing.expired_deadline_wait_count, 2);
        assert_eq!(pacing.repeated_immediate_timer_wake_count, 1);
        assert_eq!(pacing.multiple_deadline_owner_violation_count, 2);
    }

    #[test]
    fn active_pageflip_percentiles_exclude_idle_gaps() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        for now_ns in [6_060_000, 12_121_000, 24_241_000, 84_241_000, 90_301_000] {
            pacing.note_pageflip(now_ns, now_ns, 1, 6_060);
        }

        let timing = pacing.timing_metrics();
        assert_eq!(timing.active_pageflip_interval, (6_061, 12_120, 12_120));
        assert_eq!(pacing.idle_intervals_excluded, 1);
        assert_eq!(timing.pageflip_interval, (6_061, 60_000, 60_000));
    }

    #[test]
    fn content_clock_summary_exposes_bounded_stage_and_attribution_metrics() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        let mut callback_metrics = oblivion_one::compositor::FrameCallbackMetrics::default();
        callback_metrics.last_callback_admission_to_next_commit_ns = Some(500_000);
        callback_metrics.callback_admission_to_next_commit_samples = 1;
        pacing.note_callback_metrics(callback_metrics, 6_060_606);
        pacing.note_explicit_present(ExplicitPresentationObservation {
            planned_sequence: 4,
            actual_sequence: 2,
            target_ns: 1_018_181_818,
            presented_ns: 1_012_121_212,
            composite_started_ns: 1_010_000_000,
            rendered_ns: 1_011_000_000,
            submit_started_ns: 1_011_100_000,
            submit_returned_ns: 1_011_300_000,
            reactive_double: true,
            target_reason: oblivion_one::native::presentation_deadline::PresentationTargetReason::ReactiveDouble,
            previous_primary_sequence: Some(1),
            client_commit_ns: Some(1_009_500_000),
            callback_reaction_ns: Some(500_000),
            callback_admission_ns: None,
            refresh_interval_ns: 6_060_606,
            render_missed: false,
            submit_missed: false,
            kms_slipped: false,
        });

        let summary = pacing.content_summary_line();
        for field in [
            "event=native_content_frame_clock_summary",
            "primary_present_interval_p50_us=0",
            "callback_admission_to_next_commit_p50_us=500",
            "client_commit_to_render_start_p50_us=500",
            "render_start_to_ready_p50_us=1000",
            "ready_to_submit_p50_us=100",
            "submit_to_pageflip_p50_us=821",
            "selected_target_distance_intervals_p50=3",
            "actual_primary_distance_intervals_p50=1",
            "reactive_target_late_by_intervals=1",
            "fast_client_samples=1",
            "content_attribution_target_limited=1",
            "prediction_total_cost_ns=0",
        ] {
            assert!(summary.contains(field), "missing content field {field}");
        }
    }
}
use super::scanout::NativeScanoutBufferSnapshot;
use oblivion_one::compositor::FrameCallbackMetrics;
use oblivion_one::native::adaptive_buffering::{
    AdaptiveBufferingMode, FenceTimestampQuality, ProvenDeadlineMiss, RenderPrediction,
};
use oblivion_one::native::presentation_deadline::PresentationTargetReason;
use oblivion_one::native::scheduler::{
    NativeOutputPacingMode, PipelineWaitReason, SchedulerDecision,
};
use std::collections::VecDeque;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
    mpsc::{SyncSender, sync_channel},
};
use std::thread;
#[path = "pacing_o1.rs"]
mod pacing_o1;
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

#[derive(Debug)]
pub(crate) struct BoundedSignedSamples<const N: usize> {
    values: VecDeque<i64>,
}

impl<const N: usize> Default for BoundedSignedSamples<N> {
    fn default() -> Self {
        Self {
            values: VecDeque::with_capacity(N),
        }
    }
}

impl<const N: usize> BoundedSignedSamples<N> {
    pub(crate) fn record(&mut self, value: i64) {
        if N == 0 {
            return;
        }
        if self.values.len() == N {
            self.values.pop_front();
        }
        self.values.push_back(value);
    }

    pub(crate) fn percentiles(&self) -> (i64, i64, i64) {
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

fn is_active_refresh_interval(elapsed_us: u64, refresh_interval_us: u64) -> bool {
    refresh_interval_us != 0 && elapsed_us <= refresh_interval_us.saturating_mul(4)
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
    pub(crate) fn i64(key: &'static str, value: i64) -> Self {
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
const TARGET_TIMESTAMP_TOLERANCE_NS: u64 = 100_000;
const TRACE_QUEUE_CAPACITY: usize = 2_048;
const CONTENT_ATTRIBUTION_COUNT: usize = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContentCadenceAttribution {
    CallbackHandoffLimited,
    ClientLimited,
    TargetLimited,
    RenderLimited,
    SubmitLimited,
    KmsLimited,
    TargetHit,
}

impl ContentCadenceAttribution {
    const fn index(self) -> usize {
        match self {
            Self::CallbackHandoffLimited => 0,
            Self::ClientLimited => 1,
            Self::TargetLimited => 2,
            Self::RenderLimited => 3,
            Self::SubmitLimited => 4,
            Self::KmsLimited => 5,
            Self::TargetHit => 6,
        }
    }
}

pub(crate) fn classify_content_frame(
    callback_handoff_limited: bool,
    callback_reaction_ns: Option<u64>,
    fast_client_threshold_ns: u64,
    selected_target_distance: u64,
    actual_primary_distance: u64,
    target_was_feasible: bool,
    render_missed: bool,
    submit_missed: bool,
    kms_slipped: bool,
) -> ContentCadenceAttribution {
    if callback_handoff_limited {
        ContentCadenceAttribution::CallbackHandoffLimited
    } else if callback_reaction_ns.is_some_and(|reaction| reaction > fast_client_threshold_ns) {
        ContentCadenceAttribution::ClientLimited
    } else if target_was_feasible && selected_target_distance > 1 && actual_primary_distance == 1 {
        ContentCadenceAttribution::TargetLimited
    } else if render_missed {
        ContentCadenceAttribution::RenderLimited
    } else if submit_missed {
        ContentCadenceAttribution::SubmitLimited
    } else if kms_slipped {
        ContentCadenceAttribution::KmsLimited
    } else {
        ContentCadenceAttribution::TargetHit
    }
}

#[derive(Debug, Clone, Copy)]
struct WorkerPacingReservation {
    frame_id: NativeOutputFrameId,
}

#[derive(Debug)]
struct NativeTraceSink {
    sender: SyncSender<String>,
    dropped: Arc<AtomicU64>,
}

impl NativeTraceSink {
    fn new() -> Self {
        let (sender, receiver) = sync_channel(TRACE_QUEUE_CAPACITY);
        let _ = thread::Builder::new()
            .name("typhon-frame-pacing-trace".to_string())
            .spawn(move || {
                while let Ok(line) = receiver.recv() {
                    println!("{line}");
                }
            });
        Self {
            sender,
            dropped: Arc::new(AtomicU64::new(0)),
        }
    }

    fn send(&self, line: String) {
        if self.sender.try_send(line).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn dropped_entries(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

#[derive(Debug)]
pub(crate) struct NativeFramePacing {
    enabled: bool,
    summary_enabled: bool,
    trace: Option<NativeTraceSink>,
    ids: NativeOutputFrameIdSequence,
    pub(crate) active: Option<NativeOutputFrameId>,
    pub(crate) active_queued_ns: Option<u64>,
    active_queued_frame_id: Option<NativeOutputFrameId>,
    pub(crate) pending: Option<NativeOutputFrameId>,
    pending_token: Option<u64>,
    pub(crate) ready: Option<NativeOutputFrameId>,
    ready_waiting_frame_id: Option<NativeOutputFrameId>,
    worker_reservation: Option<WorkerPacingReservation>,
    pub(crate) render_ahead_attempts: u64,
    pub(crate) render_ahead_successes: u64,
    pub(crate) wait_for_buffer_count: u64,
    pub(crate) ready_submit_count: u64,
    pub(crate) reactive_double_frames: u64,
    pub(crate) reactive_double_immediate_submits: u64,
    pub(crate) reactive_double_actual_misses: u64,
    pub(crate) predictive_triple_frames: u64,
    pub(crate) predictive_render_ahead_attempts: u64,
    pub(crate) predictive_render_ahead_ready: u64,
    pub(crate) predictive_ready_submits: u64,
    pub(crate) normal_ready_wait_count: u64,
    pub(crate) scheduled_normal_target_count: u64,
    pub(crate) expired_deadline_wait_count: u64,
    pub(crate) repeated_immediate_timer_wake_count: u64,
    pub(crate) multiple_deadline_owner_violation_count: u64,
    pub(crate) adaptive_triple_entries_predicted: u64,
    pub(crate) adaptive_triple_entries_proven_render_miss: u64,
    pub(crate) adaptive_triple_entries_proven_submit_miss: u64,
    pub(crate) adaptive_triple_entries_proven_presentation_miss: u64,
    pub(crate) adaptive_triple_exits: u64,
    pub(crate) o1_credit2_useful_hits: u64,
    pub(crate) o1_credit2_unnecessary_hits: u64,
    pub(crate) o1_credit2_ineffective_misses: u64,
    pub(crate) o1_credit2_granted_not_consumed: u64,
    pub(crate) o1_credit2_drain_events: u64,
    pub(crate) o1_credit2_refill_suppressed_while_draining: u64,
    o1_credit2_pending_grant: bool,
    pub(crate) sync_file_info_exact: u64,
    pub(crate) sync_file_info_approximate: u64,
    pipeline_waits: [u64; 10],
    wake_lateness: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    slot_hold: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    ready_age: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    target_error: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    target_error_signed: BoundedSignedSamples<PACING_SAMPLE_CAPACITY>,
    target_interval_distance: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    ready_waiting_for_target: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    atomic_submit: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    pageflip_intervals: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    active_pageflip_intervals: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    commit_to_present: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    callback_admission_to_next_commit: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    client_commit_to_render_start: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    render_start_to_ready: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    ready_to_submit: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    submit_to_pageflip: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    selected_target_distance_intervals: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    actual_primary_distance_intervals: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    reactive_target_early_by_intervals: u64,
    predictive_target_early_by_intervals: u64,
    reactive_target_late_by_intervals: u64,
    predictive_target_late_by_intervals: u64,
    fast_client_samples: u64,
    slow_client_samples: u64,
    content_attribution: [u64; CONTENT_ATTRIBUTION_COUNT],
    last_callback_reaction_sample_count: u64,
    last_prediction: Option<RenderPrediction>,
    last_primary_sequence: Option<u64>,
    misses: RefreshMissBuckets,
    last_pageflip_ns: Option<u64>,
    idle_intervals_excluded: u64,
    early_presentation_count: u64,
    late_presentation_count: u64,
    ready_waiting_for_target_count: u64,
    ready_waiting_started_ns: Option<u64>,
    last_immediate_timer_deadline: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ExplicitPresentationObservation {
    pub(crate) planned_sequence: u64,
    pub(crate) actual_sequence: u64,
    pub(crate) target_ns: u64,
    pub(crate) presented_ns: u64,
    pub(crate) composite_started_ns: u64,
    pub(crate) rendered_ns: u64,
    pub(crate) submit_started_ns: u64,
    pub(crate) submit_returned_ns: u64,
    pub(crate) reactive_double: bool,
    pub(crate) target_reason: PresentationTargetReason,
    pub(crate) previous_primary_sequence: Option<u64>,
    pub(crate) client_commit_ns: Option<u64>,
    pub(crate) callback_reaction_ns: Option<u64>,
    pub(crate) callback_admission_ns: Option<u64>,
    pub(crate) refresh_interval_ns: u64,
    pub(crate) render_missed: bool,
    pub(crate) submit_missed: bool,
    pub(crate) kms_slipped: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct NativeBufferingMetrics {
    pub(crate) reactive_double_frames: u64,
    pub(crate) predictive_triple_frames: u64,
    pub(crate) render_ahead_attempts: u64,
    pub(crate) render_ahead_ready: u64,
    pub(crate) ready_submits: u64,
    pub(crate) triple_entries_predicted: u64,
    pub(crate) triple_entries_render_miss: u64,
    pub(crate) triple_entries_submit_miss: u64,
    pub(crate) triple_entries_presentation_miss: u64,
    pub(crate) triple_exits: u64,
    pub(crate) o1_credit2_useful_hits: u64,
    pub(crate) o1_credit2_unnecessary_hits: u64,
    pub(crate) o1_credit2_ineffective_misses: u64,
    pub(crate) o1_credit2_granted_not_consumed: u64,
    pub(crate) o1_credit2_drain_events: u64,
    pub(crate) o1_credit2_refill_suppressed_while_draining: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct NativePacingTimingMetrics {
    pub(crate) wake_lateness: (u64, u64, u64),
    pub(crate) target_error: (u64, u64, u64),
    pub(crate) pageflip_interval: (u64, u64, u64),
    pub(crate) active_pageflip_interval: (u64, u64, u64),
    pub(crate) commit_to_present: (u64, u64, u64),
    pub(crate) missed_refresh_1x: u64,
    pub(crate) missed_refresh_2x: u64,
    pub(crate) missed_refresh_3x_or_more: u64,
}

impl NativeFramePacing {
    pub(crate) fn from_env() -> Self {
        let summary_enabled = std::env::var("TYPHON_FRAME_PACING_DEBUG")
            .ok()
            .is_some_and(|value| super::perf::native_perf_log_value_enabled(&value));
        let trace_enabled = std::env::var("TYPHON_FRAME_PACING_TRACE")
            .ok()
            .is_some_and(|value| super::perf::native_perf_log_value_enabled(&value));
        Self {
            enabled: true,
            summary_enabled: summary_enabled || trace_enabled,
            trace: trace_enabled.then(NativeTraceSink::new),
            ids: NativeOutputFrameIdSequence::new(1),
            active: None,
            active_queued_ns: None,
            active_queued_frame_id: None,
            pending: None,
            pending_token: None,
            ready: None,
            ready_waiting_frame_id: None,
            worker_reservation: None,
            render_ahead_attempts: 0,
            render_ahead_successes: 0,
            wait_for_buffer_count: 0,
            ready_submit_count: 0,
            reactive_double_frames: 0,
            reactive_double_immediate_submits: 0,
            reactive_double_actual_misses: 0,
            predictive_triple_frames: 0,
            predictive_render_ahead_attempts: 0,
            predictive_render_ahead_ready: 0,
            predictive_ready_submits: 0,
            normal_ready_wait_count: 0,
            scheduled_normal_target_count: 0,
            expired_deadline_wait_count: 0,
            repeated_immediate_timer_wake_count: 0,
            multiple_deadline_owner_violation_count: 0,
            adaptive_triple_entries_predicted: 0,
            adaptive_triple_entries_proven_render_miss: 0,
            adaptive_triple_entries_proven_submit_miss: 0,
            adaptive_triple_entries_proven_presentation_miss: 0,
            adaptive_triple_exits: 0,
            o1_credit2_useful_hits: 0,
            o1_credit2_unnecessary_hits: 0,
            o1_credit2_ineffective_misses: 0,
            o1_credit2_granted_not_consumed: 0,
            o1_credit2_drain_events: 0,
            o1_credit2_refill_suppressed_while_draining: 0,
            o1_credit2_pending_grant: false,
            sync_file_info_exact: 0,
            sync_file_info_approximate: 0,
            pipeline_waits: [0; 10],
            wake_lateness: BoundedSamples::default(),
            slot_hold: BoundedSamples::default(),
            ready_age: BoundedSamples::default(),
            target_error: BoundedSamples::default(),
            target_error_signed: BoundedSignedSamples::default(),
            target_interval_distance: BoundedSamples::default(),
            ready_waiting_for_target: BoundedSamples::default(),
            atomic_submit: BoundedSamples::default(),
            pageflip_intervals: BoundedSamples::default(),
            active_pageflip_intervals: BoundedSamples::default(),
            commit_to_present: BoundedSamples::default(),
            callback_admission_to_next_commit: BoundedSamples::default(),
            client_commit_to_render_start: BoundedSamples::default(),
            render_start_to_ready: BoundedSamples::default(),
            ready_to_submit: BoundedSamples::default(),
            submit_to_pageflip: BoundedSamples::default(),
            selected_target_distance_intervals: BoundedSamples::default(),
            actual_primary_distance_intervals: BoundedSamples::default(),
            reactive_target_early_by_intervals: 0,
            predictive_target_early_by_intervals: 0,
            reactive_target_late_by_intervals: 0,
            predictive_target_late_by_intervals: 0,
            fast_client_samples: 0,
            slow_client_samples: 0,
            content_attribution: [0; CONTENT_ATTRIBUTION_COUNT],
            last_callback_reaction_sample_count: 0,
            last_prediction: None,
            last_primary_sequence: None,
            misses: RefreshMissBuckets::default(),
            last_pageflip_ns: None,
            idle_intervals_excluded: 0,
            early_presentation_count: 0,
            late_presentation_count: 0,
            ready_waiting_for_target_count: 0,
            ready_waiting_started_ns: None,
            last_immediate_timer_deadline: None,
        }
    }

    pub(crate) const fn summary_enabled(&self) -> bool {
        self.summary_enabled
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
        self.active_queued_frame_id = Some(id);
        self.log(
            "visual_queued",
            vec![
                PacingField::u64("frame_id", id.get()),
                PacingField::u64("render_generation", render_generation),
            ],
        );
    }
    pub(crate) fn log(&self, event: &str, fields: Vec<PacingField>) {
        if let Some(trace) = &self.trace {
            trace.send(pacing_line(event, &fields));
        }
    }

    pub(crate) fn note_prediction(&mut self, prediction: RenderPrediction) {
        if self.enabled {
            self.last_prediction = Some(prediction);
        }
    }

    pub(crate) fn note_callback_metrics(
        &mut self,
        metrics: FrameCallbackMetrics,
        refresh_interval_ns: u64,
    ) {
        if !self.enabled
            || metrics.callback_admission_to_next_commit_samples
                <= self.last_callback_reaction_sample_count
        {
            return;
        }
        self.last_callback_reaction_sample_count =
            metrics.callback_admission_to_next_commit_samples;
        let Some(reaction_ns) = metrics.last_callback_admission_to_next_commit_ns else {
            return;
        };
        self.callback_admission_to_next_commit
            .record(reaction_ns / 1_000);
        let fast_threshold_ns = (refresh_interval_ns / 2).min(2_000_000);
        if reaction_ns <= fast_threshold_ns {
            self.fast_client_samples = self.fast_client_samples.saturating_add(1);
        } else {
            self.slow_client_samples = self.slow_client_samples.saturating_add(1);
        }
    }
    pub(crate) fn note_render_started(
        &mut self,
        pacing_mode: NativeOutputPacingMode,
        render_ahead: bool,
    ) {
        if !self.enabled {
            return;
        }
        match (pacing_mode, render_ahead) {
            (NativeOutputPacingMode::ReactiveDouble, false) => {
                self.reactive_double_frames += 1;
            }
            (NativeOutputPacingMode::PredictiveTriple, true) => {
                self.predictive_triple_frames += 1;
                self.render_ahead_attempts += 1;
                self.predictive_render_ahead_attempts += 1;
            }
            (NativeOutputPacingMode::ReactiveDouble, true) => {
                self.multiple_deadline_owner_violation_count += 1;
            }
            (NativeOutputPacingMode::PredictiveTriple, false) => {
                self.predictive_triple_frames += 1;
            }
        }
    }
    pub(crate) fn note_submit(
        &mut self,
        token: u64,
        now_ns: u64,
        ready_submit: bool,
        pacing_mode: NativeOutputPacingMode,
    ) {
        if !self.enabled {
            return;
        }
        let id = if ready_submit {
            self.ready.take()
        } else {
            self.active.take()
        };
        if !ready_submit {
            self.clear_active_worker_timing(id);
        }
        self.note_submit_frame(id, token, now_ns, ready_submit, pacing_mode);
    }

    fn note_submit_frame(
        &mut self,
        id: Option<NativeOutputFrameId>,
        token: u64,
        now_ns: u64,
        ready_submit: bool,
        pacing_mode: NativeOutputPacingMode,
    ) {
        if ready_submit {
            self.ready_submit_count += 1;
            match pacing_mode {
                NativeOutputPacingMode::PredictiveTriple => self.predictive_ready_submits += 1,
                NativeOutputPacingMode::ReactiveDouble => self.normal_ready_wait_count += 1,
            }
            if self.ready_waiting_frame_id == id {
                if let Some(started_at) = self.ready_waiting_started_ns.take() {
                    self.ready_waiting_for_target
                        .record(now_ns.saturating_sub(started_at) / 1_000);
                }
                self.ready_waiting_frame_id = None;
            }
        }
        if pacing_mode == NativeOutputPacingMode::ReactiveDouble && !ready_submit {
            self.reactive_double_immediate_submits += 1;
        }
        self.pending = id;
        self.pending_token = id.map(|_| token);
        if !ready_submit && self.active_queued_frame_id == id {
            self.active_queued_ns = None;
            self.active_queued_frame_id = None;
        }
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

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn note_worker_submit(
        &mut self,
        token: u64,
        now_ns: u64,
        ready_submit: bool,
        pacing_mode: NativeOutputPacingMode,
    ) {
        self.note_submit(token, now_ns, ready_submit, pacing_mode);
    }

    pub(crate) fn worker_submission_frame_id(&self, ready_submit: bool) -> Option<u64> {
        self.worker_submission_frame(ready_submit)
            .map(|id| id.get())
    }

    pub(crate) fn reserve_worker_submission(
        &mut self,
        ready_submit: bool,
    ) -> Result<Option<u64>, &'static str> {
        if !self.enabled {
            return Ok(self.worker_submission_frame_id(ready_submit));
        }
        if self.worker_reservation.is_some() {
            return Err("worker pacing reservation is already queued");
        }
        let Some(frame_id) = self.worker_submission_frame(ready_submit) else {
            return Ok(None);
        };
        self.worker_reservation = Some(WorkerPacingReservation { frame_id });
        Ok(Some(frame_id.get()))
    }

    fn worker_submission_frame(&self, ready_submit: bool) -> Option<NativeOutputFrameId> {
        if ready_submit {
            self.ready
        } else {
            self.active
        }
    }

    fn clear_active_worker_timing(&mut self, frame_id: Option<NativeOutputFrameId>) {
        if self.active_queued_frame_id == frame_id {
            self.active_queued_frame_id = None;
            self.active_queued_ns = None;
        }
    }

    fn clear_ready_waiting_timing(&mut self, frame_id: Option<NativeOutputFrameId>) {
        if self.ready_waiting_frame_id == frame_id {
            self.ready_waiting_frame_id = None;
            self.ready_waiting_started_ns = None;
        }
    }

    fn take_worker_submission_frame(
        &mut self,
        expected: Option<u64>,
    ) -> Result<Option<NativeOutputFrameId>, &'static str> {
        let Some(expected) = expected else {
            return Ok(None);
        };

        let Some(reservation) = self.worker_reservation else {
            return Err("worker pacing frame identity does not match queued state");
        };
        if reservation.frame_id.get() == expected {
            self.worker_reservation = None;
            if self.active == Some(reservation.frame_id) {
                self.active = None;
            }
            if self.ready == Some(reservation.frame_id) {
                self.ready = None;
            }
            self.clear_active_worker_timing(Some(reservation.frame_id));
            return Ok(Some(reservation.frame_id));
        }
        Err("worker pacing frame identity does not match queued state")
    }

    pub(crate) fn cancel_worker_submission(
        &mut self,
        expected: Option<u64>,
        ready_submit: bool,
    ) -> bool {
        if !self.enabled {
            return true;
        }
        if expected.is_none() {
            return true;
        }
        let current = match self.take_worker_submission_frame(expected) {
            Ok(current) => current,
            Err(_) => return false,
        };
        if current.is_none() {
            return false;
        }
        self.clear_ready_waiting_timing(current);
        self.log(
            "worker_submit_cancelled",
            vec![
                frame_id_field(current),
                PacingField::bool("ready_submit", ready_submit),
            ],
        );
        true
    }

    pub(crate) fn note_worker_submit_exact(
        &mut self,
        expected: Option<u64>,
        token: u64,
        now_ns: u64,
        ready_submit: bool,
        pacing_mode: NativeOutputPacingMode,
    ) -> Result<(), &'static str> {
        if !self.enabled {
            return Ok(());
        }
        if expected.is_none() {
            return Ok(());
        }
        let id = self.take_worker_submission_frame(expected)?;
        self.note_submit_frame(id, token, now_ns, ready_submit, pacing_mode);
        Ok(())
    }

    pub(crate) fn abandon_pending_submission(&mut self, token: u64) -> bool {
        if self.pending_token != Some(token) {
            return false;
        }
        self.pending = None;
        self.pending_token = None;
        self.log(
            "worker_submit_abandoned",
            vec![PacingField::u64("pageflip_token", token)],
        );
        true
    }
    pub(crate) fn note_render_ahead_ready(&mut self, now_ns: u64) {
        self.note_ready_frame(now_ns, true);
    }
    pub(crate) fn note_ready_frame(&mut self, now_ns: u64, waits_for_target: bool) {
        if !self.enabled {
            return;
        }
        let ready = self.active.take();
        if waits_for_target {
            self.render_ahead_successes += 1;
            self.predictive_render_ahead_ready += 1;
            self.ready_waiting_started_ns = None;
            self.ready_waiting_frame_id = None;
        } else {
            self.normal_ready_wait_count += 1;
            self.ready_waiting_for_target_count += 1;
            self.ready_waiting_started_ns = Some(now_ns);
            self.ready_waiting_frame_id = ready;
        }
        self.ready = ready;
        self.active_queued_frame_id = None;
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
            if is_active_refresh_interval(us, refresh_interval_us) {
                self.active_pageflip_intervals.record(us);
                self.misses.record(us, refresh_interval_us);
            } else {
                self.idle_intervals_excluded = self.idle_intervals_excluded.saturating_add(1);
            }
        }
        self.last_pageflip_ns = Some(now_ns);
        let commit_us = now_ns.saturating_sub(submitted_at_ns) / 1_000;
        self.commit_to_present.record(commit_us);
        let id = self.pending.take();
        self.pending_token = None;
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
    pub(crate) fn note_deadline_state(
        &mut self,
        decision: SchedulerDecision,
        now_ns: u64,
        scheduler_deadline: Option<u64>,
        visual_deadline: Option<u64>,
        ready_frame_present: bool,
        timer_wake: bool,
    ) {
        if !self.enabled {
            return;
        }
        if visual_deadline.is_some() && ready_frame_present {
            self.multiple_deadline_owner_violation_count += 1;
        }
        let deadline = match (scheduler_deadline, visual_deadline) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (Some(deadline), None) | (None, Some(deadline)) => Some(deadline),
            (None, None) => None,
        };
        if decision == SchedulerDecision::WaitForRefresh
            && deadline.is_some_and(|deadline| deadline <= now_ns)
        {
            self.expired_deadline_wait_count += 1;
        }
        if timer_wake && deadline.is_some_and(|deadline| deadline <= now_ns) {
            if self.last_immediate_timer_deadline == deadline {
                self.repeated_immediate_timer_wake_count += 1;
            }
            self.last_immediate_timer_deadline = deadline;
        } else {
            self.last_immediate_timer_deadline = None;
        }
    }
    pub(crate) fn note_explicit_present(&mut self, observation: ExplicitPresentationObservation) {
        if !self.enabled {
            return;
        }
        let refresh_interval_ns = observation.refresh_interval_ns.max(1);
        let previous_primary_sequence = observation
            .previous_primary_sequence
            .or(self.last_primary_sequence);
        let selected_target_distance = previous_primary_sequence
            .map(|previous| observation.planned_sequence.saturating_sub(previous))
            .unwrap_or_default();
        let actual_primary_distance = previous_primary_sequence
            .map(|previous| observation.actual_sequence.saturating_sub(previous))
            .unwrap_or_default();
        if previous_primary_sequence.is_some() {
            self.selected_target_distance_intervals
                .record(selected_target_distance);
            self.actual_primary_distance_intervals
                .record(actual_primary_distance);
        }
        self.last_primary_sequence = Some(observation.actual_sequence);
        if let Some(client_commit_ns) = observation.client_commit_ns
            && observation.composite_started_ns >= client_commit_ns
        {
            self.client_commit_to_render_start.record(
                observation
                    .composite_started_ns
                    .saturating_sub(client_commit_ns)
                    / 1_000,
            );
        }
        self.render_start_to_ready.record(
            observation
                .rendered_ns
                .saturating_sub(observation.composite_started_ns)
                / 1_000,
        );
        self.ready_to_submit.record(
            observation
                .submit_started_ns
                .saturating_sub(observation.rendered_ns)
                / 1_000,
        );
        self.submit_to_pageflip.record(
            observation
                .presented_ns
                .saturating_sub(observation.submit_returned_ns)
                / 1_000,
        );
        let target_distance = observation
            .target_ns
            .abs_diff(observation.presented_ns)
            .div_ceil(refresh_interval_ns);
        let target_is_early = observation.target_ns <= observation.presented_ns;
        match (observation.target_reason, target_is_early) {
            (PresentationTargetReason::ReactiveDouble, true) => {
                self.reactive_target_early_by_intervals = self
                    .reactive_target_early_by_intervals
                    .saturating_add(target_distance);
            }
            (PresentationTargetReason::ReactiveDouble, false) => {
                self.reactive_target_late_by_intervals = self
                    .reactive_target_late_by_intervals
                    .saturating_add(target_distance);
            }
            (_, true) => {
                self.predictive_target_early_by_intervals = self
                    .predictive_target_early_by_intervals
                    .saturating_add(target_distance);
            }
            (_, false) => {
                self.predictive_target_late_by_intervals = self
                    .predictive_target_late_by_intervals
                    .saturating_add(target_distance);
            }
        }
        let target_was_feasible = selected_target_distance > 1 && actual_primary_distance == 1;
        let fast_client_threshold_ns = (refresh_interval_ns / 2).min(2_000_000);
        let callback_handoff_limited = observation
            .callback_admission_ns
            .zip(observation.client_commit_ns)
            .zip(self.last_pageflip_ns)
            .is_some_and(|((admission_ns, commit_ns), previous_pageflip_ns)| {
                admission_ns >= previous_pageflip_ns.saturating_add(refresh_interval_ns)
                    && commit_ns > previous_pageflip_ns.saturating_add(refresh_interval_ns)
                    && observation
                        .callback_reaction_ns
                        .is_some_and(|reaction| reaction <= fast_client_threshold_ns)
            });
        let attribution = classify_content_frame(
            callback_handoff_limited,
            observation.callback_reaction_ns,
            fast_client_threshold_ns,
            selected_target_distance,
            actual_primary_distance,
            target_was_feasible,
            observation.render_missed,
            observation.submit_missed,
            observation.kms_slipped,
        );
        let attribution_index = attribution.index();
        self.content_attribution[attribution_index] =
            self.content_attribution[attribution_index].saturating_add(1);
        let signed_error_ns = if observation.presented_ns >= observation.target_ns {
            i64::try_from(
                observation
                    .presented_ns
                    .saturating_sub(observation.target_ns),
            )
            .unwrap_or(i64::MAX)
        } else {
            -i64::try_from(
                observation
                    .target_ns
                    .saturating_sub(observation.presented_ns),
            )
            .unwrap_or(i64::MAX)
        };
        self.target_error
            .record(signed_error_ns.unsigned_abs() / 1_000);
        self.target_error_signed.record(signed_error_ns / 1_000);
        self.target_interval_distance.record(
            observation
                .planned_sequence
                .abs_diff(observation.actual_sequence),
        );
        if observation.reactive_double && observation.actual_sequence > observation.planned_sequence
        {
            self.reactive_double_actual_misses = self
                .reactive_double_actual_misses
                .saturating_add(observation.actual_sequence - observation.planned_sequence);
        }
        if signed_error_ns < -(TARGET_TIMESTAMP_TOLERANCE_NS as i64) {
            self.early_presentation_count += 1;
        } else if signed_error_ns > TARGET_TIMESTAMP_TOLERANCE_NS as i64 {
            self.late_presentation_count += 1;
        }
        self.slot_hold.record(
            observation
                .submit_returned_ns
                .saturating_sub(observation.composite_started_ns)
                / 1_000,
        );
        self.ready_age.record(
            observation
                .submit_started_ns
                .saturating_sub(observation.rendered_ns)
                / 1_000,
        );
        self.atomic_submit.record(
            observation
                .submit_returned_ns
                .saturating_sub(observation.submit_started_ns)
                / 1_000,
        );
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
                Some(ProvenDeadlineMiss::KmsDispatch),
            ) => self.adaptive_triple_entries_proven_submit_miss += 1,
            (
                AdaptiveBufferingMode::Double,
                AdaptiveBufferingMode::Triple,
                Some(ProvenDeadlineMiss::KmsApplyGuard),
            ) => self.adaptive_triple_entries_proven_presentation_miss += 1,
            (AdaptiveBufferingMode::Double, AdaptiveBufferingMode::Triple, Some(_)) => {
                self.adaptive_triple_entries_proven_render_miss += 1;
            }
            (AdaptiveBufferingMode::Triple, AdaptiveBufferingMode::Double, _) => {
                self.adaptive_triple_exits += 1;
            }
            _ => {}
        }
    }
    pub(crate) fn note_pipeline_wait(&mut self, reason: PipelineWaitReason) {
        if !self.enabled {
            return;
        }
        let index = match reason {
            PipelineWaitReason::RefreshDeadline => 0,
            PipelineWaitReason::NoFreeSlot => 1,
            PipelineWaitReason::PreparedFrameExists => 2,
            PipelineWaitReason::FuturePrimaryDepthFull => 3,
            PipelineWaitReason::WorkerQueueOccupied => 4,
            PipelineWaitReason::KernelCommitPending => 5,
            PipelineWaitReason::RenderFence => 6,
            PipelineWaitReason::DirectSteadyState => 7,
            PipelineWaitReason::CompatibilityPath => 8,
            PipelineWaitReason::TripleCapabilityUnavailable => 9,
        };
        self.pipeline_waits[index] = self.pipeline_waits[index].saturating_add(1);
    }

    pub(crate) const fn buffering_metrics(&self) -> NativeBufferingMetrics {
        NativeBufferingMetrics {
            reactive_double_frames: self.reactive_double_frames,
            predictive_triple_frames: self.predictive_triple_frames,
            render_ahead_attempts: self.render_ahead_attempts,
            render_ahead_ready: self.predictive_render_ahead_ready,
            ready_submits: self.ready_submit_count,
            triple_entries_predicted: self.adaptive_triple_entries_predicted,
            triple_entries_render_miss: self.adaptive_triple_entries_proven_render_miss,
            triple_entries_submit_miss: self.adaptive_triple_entries_proven_submit_miss,
            triple_entries_presentation_miss: self.adaptive_triple_entries_proven_presentation_miss,
            triple_exits: self.adaptive_triple_exits,
            o1_credit2_useful_hits: self.o1_credit2_useful_hits,
            o1_credit2_unnecessary_hits: self.o1_credit2_unnecessary_hits,
            o1_credit2_ineffective_misses: self.o1_credit2_ineffective_misses,
            o1_credit2_granted_not_consumed: self.o1_credit2_granted_not_consumed,
            o1_credit2_drain_events: self.o1_credit2_drain_events,
            o1_credit2_refill_suppressed_while_draining: self
                .o1_credit2_refill_suppressed_while_draining,
        }
    }

    pub(crate) fn timing_metrics(&self) -> NativePacingTimingMetrics {
        NativePacingTimingMetrics {
            wake_lateness: self.wake_lateness.percentiles(),
            target_error: self.target_error.percentiles(),
            pageflip_interval: self.pageflip_intervals.percentiles(),
            active_pageflip_interval: self.active_pageflip_intervals.percentiles(),
            commit_to_present: self.commit_to_present.percentiles(),
            missed_refresh_1x: self.misses.missed_1x,
            missed_refresh_2x: self.misses.missed_2x,
            missed_refresh_3x_or_more: self.misses.missed_3x_or_more,
        }
    }

    pub(crate) fn content_summary_line(&self) -> String {
        let (primary50, primary95, primary99) = self.active_pageflip_intervals.percentiles();
        let (callback50, callback95, callback99) =
            self.callback_admission_to_next_commit.percentiles();
        let (client50, client95, client99) = self.client_commit_to_render_start.percentiles();
        let (render50, render95, render99) = self.render_start_to_ready.percentiles();
        let (ready50, ready95, ready99) = self.ready_to_submit.percentiles();
        let (submit50, submit95, submit99) = self.submit_to_pageflip.percentiles();
        let (selected50, selected95, selected99) =
            self.selected_target_distance_intervals.percentiles();
        let (actual50, actual95, actual99) = self.actual_primary_distance_intervals.percentiles();
        let prediction = self.last_prediction;
        pacing_line(
            "native_content_frame_clock_summary",
            &[
                PacingField::u64("primary_present_interval_p50_us", primary50),
                PacingField::u64("primary_present_interval_p95_us", primary95),
                PacingField::u64("primary_present_interval_p99_us", primary99),
                PacingField::u64("callback_admission_to_next_commit_p50_us", callback50),
                PacingField::u64("callback_admission_to_next_commit_p95_us", callback95),
                PacingField::u64("callback_admission_to_next_commit_p99_us", callback99),
                PacingField::u64("client_commit_to_render_start_p50_us", client50),
                PacingField::u64("client_commit_to_render_start_p95_us", client95),
                PacingField::u64("client_commit_to_render_start_p99_us", client99),
                PacingField::u64("render_start_to_ready_p50_us", render50),
                PacingField::u64("render_start_to_ready_p95_us", render95),
                PacingField::u64("render_start_to_ready_p99_us", render99),
                PacingField::u64("ready_to_submit_p50_us", ready50),
                PacingField::u64("ready_to_submit_p95_us", ready95),
                PacingField::u64("ready_to_submit_p99_us", ready99),
                PacingField::u64("submit_to_pageflip_p50_us", submit50),
                PacingField::u64("submit_to_pageflip_p95_us", submit95),
                PacingField::u64("submit_to_pageflip_p99_us", submit99),
                PacingField::u64("selected_target_distance_intervals_p50", selected50),
                PacingField::u64("selected_target_distance_intervals_p95", selected95),
                PacingField::u64("selected_target_distance_intervals_p99", selected99),
                PacingField::u64("actual_primary_distance_intervals_p50", actual50),
                PacingField::u64("actual_primary_distance_intervals_p95", actual95),
                PacingField::u64("actual_primary_distance_intervals_p99", actual99),
                PacingField::u64(
                    "reactive_target_early_by_intervals",
                    self.reactive_target_early_by_intervals,
                ),
                PacingField::u64(
                    "predictive_target_early_by_intervals",
                    self.predictive_target_early_by_intervals,
                ),
                PacingField::u64(
                    "reactive_target_late_by_intervals",
                    self.reactive_target_late_by_intervals,
                ),
                PacingField::u64(
                    "predictive_target_late_by_intervals",
                    self.predictive_target_late_by_intervals,
                ),
                PacingField::u64("fast_client_samples", self.fast_client_samples),
                PacingField::u64("slow_client_samples", self.slow_client_samples),
                PacingField::u64(
                    "content_attribution_callback_handoff_limited",
                    self.content_attribution
                        [ContentCadenceAttribution::CallbackHandoffLimited.index()],
                ),
                PacingField::u64(
                    "content_attribution_client_limited",
                    self.content_attribution[ContentCadenceAttribution::ClientLimited.index()],
                ),
                PacingField::u64(
                    "content_attribution_target_limited",
                    self.content_attribution[ContentCadenceAttribution::TargetLimited.index()],
                ),
                PacingField::u64(
                    "content_attribution_render_limited",
                    self.content_attribution[ContentCadenceAttribution::RenderLimited.index()],
                ),
                PacingField::u64(
                    "content_attribution_submit_limited",
                    self.content_attribution[ContentCadenceAttribution::SubmitLimited.index()],
                ),
                PacingField::u64(
                    "content_attribution_kms_limited",
                    self.content_attribution[ContentCadenceAttribution::KmsLimited.index()],
                ),
                PacingField::u64(
                    "content_attribution_target_hit",
                    self.content_attribution[ContentCadenceAttribution::TargetHit.index()],
                ),
                PacingField::u64(
                    "prediction_ewma_render_ns",
                    prediction.map_or(0, |value| value.ewma_render_ns),
                ),
                PacingField::u64(
                    "prediction_upper_render_deviation_ns",
                    prediction.map_or(0, |value| value.upper_render_deviation_ns),
                ),
                PacingField::u64(
                    "prediction_p90_recent_render_ns",
                    prediction.map_or(0, |value| value.p90_recent_render_ns),
                ),
                PacingField::u64(
                    "prediction_render_risk_ns",
                    prediction.map_or(0, |value| value.render_risk_ns),
                ),
                PacingField::u64(
                    "prediction_p95_wake_lateness_ns",
                    prediction.map_or(0, |value| value.p95_wake_lateness_ns),
                ),
                PacingField::u64(
                    "prediction_p95_worker_queue_residency_ns",
                    prediction.map_or(0, |value| value.p95_worker_queue_residency_ns),
                ),
                PacingField::u64(
                    "prediction_p95_worker_pre_submit_ns",
                    prediction.map_or(0, |value| value.p95_worker_pre_submit_ns),
                ),
                PacingField::u64(
                    "prediction_p95_worker_dispatch_ns",
                    prediction.map_or(0, |value| value.p95_worker_dispatch_ns),
                ),
                PacingField::u64(
                    "prediction_p95_atomic_ioctl_ns",
                    prediction.map_or(0, |value| value.p95_atomic_ioctl_ns),
                ),
                PacingField::u64(
                    "prediction_p95_atomic_submit_ns",
                    prediction.map_or(0, |value| value.p95_atomic_submit_ns),
                ),
                PacingField::u64(
                    "prediction_p95_target_slip_ns",
                    prediction.map_or(0, |value| value.p95_target_slip_ns),
                ),
                PacingField::u64(
                    "prediction_kms_dispatch_budget_ns",
                    prediction.map_or(0, |value| value.kms_dispatch_budget_ns),
                ),
                PacingField::u64(
                    "prediction_kms_apply_guard_ns",
                    prediction.map_or(0, |value| value.kms_apply_guard_ns),
                ),
                PacingField::u64(
                    "prediction_kms_total_lead_ns",
                    prediction.map_or(0, |value| value.kms_total_lead_ns),
                ),
                PacingField::u64(
                    "prediction_total_cost_ns",
                    prediction.map_or(0, |value| value.total_cost_ns),
                ),
                PacingField::bool(
                    "prediction_idle_wake_guard",
                    prediction.is_some_and(|value| value.idle_wake_guard),
                ),
            ],
        )
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
    pub(crate) fn summary_line(&self, compositor_trace_dropped_entries: u64) -> String {
        let (pf50, pf95, pf99) = self.pageflip_intervals.percentiles();
        let (active_pf50, active_pf95, active_pf99) = self.active_pageflip_intervals.percentiles();
        let (cp50, cp95, cp99) = self.commit_to_present.percentiles();
        let (wake50, wake95, wake99) = self.wake_lateness.percentiles();
        let (slot50, slot95, slot99) = self.slot_hold.percentiles();
        let (ready50, ready95, ready99) = self.ready_age.percentiles();
        let (target50, target95, target99) = self.target_error.percentiles();
        let (target_signed50, target_signed95, target_signed99) =
            self.target_error_signed.percentiles();
        let (target_distance50, target_distance95, target_distance99) =
            self.target_interval_distance.percentiles();
        let (ready_wait50, ready_wait95, ready_wait99) =
            self.ready_waiting_for_target.percentiles();
        let (submit50, submit95, submit99) = self.atomic_submit.percentiles();
        pacing_line(
            "summary",
            &[
                PacingField::u64("render_ahead_attempts", self.render_ahead_attempts),
                PacingField::u64("render_ahead_successes", self.render_ahead_successes),
                PacingField::u64("wait_for_buffer_count", self.wait_for_buffer_count),
                PacingField::u64("ready_submit_count", self.ready_submit_count),
                PacingField::u64("reactive_double_frames", self.reactive_double_frames),
                PacingField::u64(
                    "reactive_double_immediate_submits",
                    self.reactive_double_immediate_submits,
                ),
                PacingField::u64(
                    "reactive_double_actual_misses",
                    self.reactive_double_actual_misses,
                ),
                PacingField::u64(
                    "predictive_render_ahead_attempts",
                    self.predictive_render_ahead_attempts,
                ),
                PacingField::u64(
                    "predictive_render_ahead_ready",
                    self.predictive_render_ahead_ready,
                ),
                PacingField::u64("predictive_ready_submits", self.predictive_ready_submits),
                PacingField::u64("normal_ready_wait_count", self.normal_ready_wait_count),
                PacingField::u64(
                    "scheduled_normal_target_count",
                    self.scheduled_normal_target_count,
                ),
                PacingField::u64(
                    "expired_deadline_wait_count",
                    self.expired_deadline_wait_count,
                ),
                PacingField::u64(
                    "repeated_immediate_timer_wake_count",
                    self.repeated_immediate_timer_wake_count,
                ),
                PacingField::u64(
                    "multiple_deadline_owner_violation_count",
                    self.multiple_deadline_owner_violation_count,
                ),
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
                PacingField::u64(
                    "adaptive_triple_entries_proven_presentation_miss",
                    self.adaptive_triple_entries_proven_presentation_miss,
                ),
                PacingField::u64("adaptive_triple_exits", self.adaptive_triple_exits),
                PacingField::u64("pipeline_wait_refresh_deadline", self.pipeline_waits[0]),
                PacingField::u64("pipeline_wait_no_free_slot", self.pipeline_waits[1]),
                PacingField::u64(
                    "pipeline_wait_prepared_frame_exists",
                    self.pipeline_waits[2],
                ),
                PacingField::u64(
                    "pipeline_wait_future_primary_depth_full",
                    self.pipeline_waits[3],
                ),
                PacingField::u64(
                    "pipeline_wait_worker_queue_occupied",
                    self.pipeline_waits[4],
                ),
                PacingField::u64(
                    "pipeline_wait_kernel_commit_pending",
                    self.pipeline_waits[5],
                ),
                PacingField::u64("pipeline_wait_render_fence", self.pipeline_waits[6]),
                PacingField::u64("pipeline_wait_direct_steady_state", self.pipeline_waits[7]),
                PacingField::u64("pipeline_wait_compatibility_path", self.pipeline_waits[8]),
                PacingField::u64(
                    "pipeline_wait_triple_capability_unavailable",
                    self.pipeline_waits[9],
                ),
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
                PacingField::i64("target_error_signed_p50_us", target_signed50),
                PacingField::i64("target_error_signed_p95_us", target_signed95),
                PacingField::i64("target_error_signed_p99_us", target_signed99),
                PacingField::u64("target_interval_distance_p50", target_distance50),
                PacingField::u64("target_interval_distance_p95", target_distance95),
                PacingField::u64("target_interval_distance_p99", target_distance99),
                PacingField::u64("early_presentation_count", self.early_presentation_count),
                PacingField::u64("late_presentation_count", self.late_presentation_count),
                PacingField::u64(
                    "ready_waiting_for_target_count",
                    self.ready_waiting_for_target_count,
                ),
                PacingField::u64("ready_waiting_for_target_us_p50", ready_wait50),
                PacingField::u64("ready_waiting_for_target_us_p95", ready_wait95),
                PacingField::u64("ready_waiting_for_target_us_p99", ready_wait99),
                PacingField::u64(
                    "verbose_trace_dropped_entries",
                    self.trace
                        .as_ref()
                        .map_or(0, NativeTraceSink::dropped_entries)
                        .saturating_add(compositor_trace_dropped_entries),
                ),
                PacingField::u64("atomic_submit_p50_us", submit50),
                PacingField::u64("atomic_submit_p95_us", submit95),
                PacingField::u64("atomic_submit_p99_us", submit99),
                PacingField::u64("pageflip_interval_p50_us", pf50),
                PacingField::u64("pageflip_interval_p95_us", pf95),
                PacingField::u64("pageflip_interval_p99_us", pf99),
                PacingField::u64("active_pageflip_interval_p50_us", active_pf50),
                PacingField::u64("active_pageflip_interval_p95_us", active_pf95),
                PacingField::u64("active_pageflip_interval_p99_us", active_pf99),
                PacingField::u64("commit_to_present_p50_us", cp50),
                PacingField::u64("commit_to_present_p95_us", cp95),
                PacingField::u64("commit_to_present_p99_us", cp99),
                PacingField::u64("missed_refresh_1x", self.misses.missed_1x),
                PacingField::u64("missed_refresh_2x", self.misses.missed_2x),
                PacingField::u64("missed_refresh_3x_or_more", self.misses.missed_3x_or_more),
                PacingField::u64("idle_intervals_excluded", self.idle_intervals_excluded),
            ],
        )
    }
}
