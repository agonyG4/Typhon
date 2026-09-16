use super::*;
use crate::native_output::pacing::NativeBufferingMetrics;
use oblivion_one::compositor::SurfaceDamagePresentation;
use oblivion_one::control_snapshots::BufferingPerformanceSnapshot;
use oblivion_one::native::adaptive_buffering::{AdaptiveBufferingController, RenderPrediction};

#[allow(clippy::too_many_arguments)]
pub(super) fn build_render_begin_fields(
    frame_id: Option<NativeOutputFrameId>,
    predictive_attempt_id: Option<PredictiveO1AttemptId>,
    render_generation: u64,
    render_observed_at_ns: u64,
    render_ahead: bool,
    buffering: &AdaptiveBufferingController,
    overlap_required_ns: u64,
    pre_render_abandoned: u64,
    prediction: &RenderPrediction,
    refresh_interval: Duration,
    buffer_snapshot: NativeScanoutBufferSnapshot,
) -> Vec<PacingField> {
    let mut fields = vec![
        frame_id_field(frame_id),
        PacingField::option_u64(
            "predictive_attempt_id",
            predictive_attempt_id.map(|attempt| attempt.get()),
        ),
        PacingField::none("output_frame_id"),
        PacingField::u64("render_generation", render_generation),
        PacingField::u64("render_observed_at_ns", render_observed_at_ns),
        PacingField::bool("render_ahead", render_ahead),
        PacingField::str("buffering_mode", buffering.mode().as_str()),
        PacingField::u64(
            "o1_future_primary_credit",
            u64::from(buffering.future_primary_credit()),
        ),
        PacingField::u64("o1_extra_credit_grants", buffering.extra_credit_grants()),
        PacingField::u64("o1_extra_credit_revokes", buffering.extra_credit_revokes()),
        PacingField::u64("o1_overlap_required_ns", overlap_required_ns),
        PacingField::u64("o1_pre_render_abandoned", pre_render_abandoned),
        PacingField::u64("prediction_ewma_ns", prediction.ewma_render_ns),
        PacingField::u64(
            "prediction_upper_deviation_ns",
            prediction.upper_render_deviation_ns,
        ),
        PacingField::u64("prediction_p90_ns", prediction.p90_recent_render_ns),
        PacingField::u64("prediction_render_risk_ns", prediction.render_risk_ns),
        PacingField::u64(
            "prediction_independent_total_cost_ns",
            prediction.independent_total_cost_ns,
        ),
        PacingField::u64(
            "prediction_warm_paired_total_cost_ns",
            prediction.warm_paired_total_cost_ns,
        ),
        PacingField::u64(
            "prediction_independent_p90_floor_ns",
            prediction.independent_p90_floor_ns,
        ),
        PacingField::u64(
            "prediction_worker_non_ioctl_lead_ns",
            prediction.worker_non_ioctl_lead_ns,
        ),
        PacingField::usize(
            "prediction_miss_recovery_remaining",
            prediction.miss_recovery_remaining,
        ),
        PacingField::u64(
            "prediction_worker_queue_residency_ns",
            prediction.p95_worker_queue_residency_ns,
        ),
        PacingField::u64(
            "prediction_worker_submit_wake_ns",
            prediction.p95_wake_lateness_ns,
        ),
        PacingField::u64(
            "prediction_worker_pre_submit_ns",
            prediction.p95_worker_pre_submit_ns,
        ),
        PacingField::u64(
            "prediction_worker_dispatch_ns",
            prediction.p95_worker_dispatch_ns,
        ),
        PacingField::u64("prediction_atomic_ioctl_ns", prediction.p95_atomic_ioctl_ns),
        PacingField::u64(
            "prediction_kms_dispatch_budget_ns",
            prediction.kms_dispatch_budget_ns,
        ),
        PacingField::u64(
            "prediction_kms_apply_guard_ns",
            prediction.kms_apply_guard_ns,
        ),
        PacingField::u64("prediction_kms_total_lead_ns", prediction.kms_total_lead_ns),
        PacingField::u64(
            "prediction_paired_service_p95_ns",
            prediction.paired_service_p95_ns,
        ),
        PacingField::usize(
            "prediction_paired_service_samples",
            prediction.paired_service_samples,
        ),
        PacingField::str(
            "prediction_estimator_mode",
            prediction.estimator_mode.as_str(),
        ),
        PacingField::u64(
            "main_event_loop_wake_guard_ns",
            prediction.main_event_loop_wake_guard_ns,
        ),
        PacingField::u64("predicted_total_cost_ns", prediction.total_cost_ns),
        PacingField::u64("refresh_interval_ns", duration_ns(refresh_interval)),
        PacingField::bool("idle_wake_guard", prediction.idle_wake_guard),
    ];
    fields.extend(snapshot_fields(buffer_snapshot));
    fields
}

pub(super) fn build_buffering_performance_snapshot(
    buffering: NativeBufferingMetrics,
    adaptive_buffering: &AdaptiveBufferingController,
    pre_render_abandoned: u64,
    prediction: &RenderPrediction,
) -> BufferingPerformanceSnapshot {
    BufferingPerformanceSnapshot {
        reactive_double_frames: buffering.reactive_double_frames,
        predictive_triple_frames: buffering.predictive_triple_frames,
        future_primary_credit: adaptive_buffering.future_primary_credit(),
        extra_credit_grants: adaptive_buffering.extra_credit_grants(),
        extra_credit_revokes: adaptive_buffering.extra_credit_revokes(),
        o1_credit2_useful_hits: buffering.o1_credit2_useful_hits,
        o1_credit2_unnecessary_hits: buffering.o1_credit2_unnecessary_hits,
        o1_credit2_ineffective_misses: buffering.o1_credit2_ineffective_misses,
        o1_credit2_granted_not_consumed: buffering.o1_credit2_granted_not_consumed,
        o1_credit2_drain_events: buffering.o1_credit2_drain_events,
        o1_credit2_refill_suppressed_while_draining: buffering
            .o1_credit2_refill_suppressed_while_draining,
        pre_render_abandoned,
        predicted_independent_render_ready_service_ns: prediction
            .main_event_loop_wake_guard_ns
            .saturating_add(prediction.render_risk_ns),
        predicted_independent_kms_lead_ns: prediction.kms_total_lead_ns,
        predicted_independent_total_service_ns: prediction.independent_total_cost_ns,
        predicted_warm_paired_total_service_ns: prediction.warm_paired_total_cost_ns,
        predicted_independent_p90_floor_ns: prediction.independent_p90_floor_ns,
        predicted_worker_non_ioctl_lead_ns: prediction.worker_non_ioctl_lead_ns,
        predicted_miss_recovery_remaining: prediction.miss_recovery_remaining as u64,
        predicted_total_service_ns: prediction.total_cost_ns,
        last_overlap_required_ns: adaptive_buffering.last_overlap_required_ns(),
        positive_overlap_observations: adaptive_buffering.positive_overlap_observations(),
        nonpositive_overlap_observations: adaptive_buffering.nonpositive_overlap_observations(),
        render_ahead_attempts: buffering.render_ahead_attempts,
        render_ahead_ready: buffering.render_ahead_ready,
        ready_submits: buffering.ready_submits,
        triple_entries_predicted: buffering.triple_entries_predicted,
        triple_entries_render_miss: buffering.triple_entries_render_miss,
        triple_entries_submit_miss: buffering.triple_entries_submit_miss,
        triple_entries_presentation_miss: buffering.triple_entries_presentation_miss,
        triple_exits: buffering.triple_exits,
        ready_pull_in_attempts: buffering.ready_pull_in_attempts,
        ready_pull_in_successes: buffering.ready_pull_in_successes,
        ready_pull_in_rejected_too_late: buffering.ready_pull_in_rejected_too_late,
        ready_pull_in_rejected_owned: buffering.ready_pull_in_rejected_owned,
        ready_pull_in_rejected_identity: buffering.ready_pull_in_rejected_identity,
        ready_pull_in_advanced_intervals: buffering.ready_pull_in_advanced_intervals,
    }
}

fn duration_ns(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn finish_no_primary_work(
    perf: NativePerfLogger,
    server: &mut OwnCompositorServer,
    frame_scheduler: &mut NativeFrameScheduler,
    output_damage: &NativeOutputDamage,
    skipped_input_repaints: usize,
    tick_us: u64,
    pageflip_pending_at_tick: bool,
    input_drain_us: u64,
    raw_input_events: usize,
    coalesced_input_events: usize,
    pageflip_drain_us: u64,
    pageflip_completed: bool,
    present_us: u64,
    kms_backend: &KmsBackendSelection,
    scanout: &NativeScanoutBackend,
    drm_file_generation: u64,
    render_generation: u64,
    render_cause: &'static str,
    surface_damage: Option<SurfaceDamagePresentation>,
    owns_frame_batch: bool,
) -> bool {
    perf.log("native.frame_skip", || {
        let mut fields = output_damage.fields().to_vec();
        fields.extend([
            NativePerfField::str("reason", "no_logical_damage"),
            NativePerfField::usize("skipped_input_repaints", skipped_input_repaints),
            NativePerfField::u64("tick_us", tick_us),
            NativePerfField::bool("pageflip_pending_at_tick", pageflip_pending_at_tick),
            NativePerfField::u64("input_drain_us", input_drain_us),
            NativePerfField::usize("raw_input_events", raw_input_events),
            NativePerfField::usize("coalesced_input_events", coalesced_input_events),
            NativePerfField::u64("pageflip_drain_us", pageflip_drain_us),
            NativePerfField::bool("pageflip_completed", pageflip_completed),
            NativePerfField::u64("present_us", present_us),
            NativePerfField::str("kms_backend", kms_backend.effective_kind().as_str()),
            NativePerfField::u64(
                "pageflip_token",
                scanout.pending_page_flip_token().unwrap_or(0),
            ),
            NativePerfField::u64("backend_generation", drm_file_generation),
            NativePerfField::u64("render_generation", render_generation),
            NativePerfField::str("render_cause", render_cause),
            NativePerfField::bool("pending_frame_work", owns_frame_batch),
        ]);
        fields
    });
    let completed_work = server.settle_no_visual_change_work(surface_damage, owns_frame_batch);
    if completed_work {
        let no_visual_change_start = Instant::now();
        perf.log("native.no_visual_change", || {
            vec![
                NativePerfField::str("reason", "no_visual_change"),
                NativePerfField::u64("elapsed_us", elapsed_micros(no_visual_change_start)),
                NativePerfField::usize("surfaces", server.renderable_surfaces().len()),
                NativePerfField::u64("render_generation", server.render_generation()),
            ]
        });
        frame_scheduler.note_immediate_completion();
    }
    completed_work
}

#[allow(clippy::too_many_arguments)]
pub(super) fn log_native_frame(
    perf: NativePerfLogger,
    paint_stats: &NativePaintStats,
    output_damage: &NativeOutputDamage,
    index: u64,
    render_ahead: bool,
    mode_label: &str,
    cursor_render_mode: NativeCursorRenderMode,
    refresh_hz: u32,
    surfaces: usize,
    render_generation: u64,
    scene_changed: bool,
    render_cause: &'static str,
    tick_us: u64,
    pageflip_pending_at_tick: bool,
    input_drain_us: u64,
    raw_input_events: usize,
    coalesced_input_events: usize,
    pageflip_drain_us: u64,
    pageflip_completed: bool,
    present_us: u64,
    repaint_present_us: u64,
    render_ahead_ready: bool,
    acquire_ready_to_render_submit_us: u64,
    cpu_user_us: u64,
    cpu_system_us: u64,
    pending_frame_work: bool,
    redraw_requested: bool,
    skipped_input_repaints: usize,
    accepted: usize,
) {
    perf.log("native.frame", || {
        let mut fields = paint_stats.fields().to_vec();
        fields.extend(output_damage.fields());
        fields.extend([
            NativePerfField::u64("index", index),
            NativePerfField::str(
                "phase",
                if render_ahead {
                    "render-ahead"
                } else {
                    "repaint"
                },
            ),
            NativePerfField::str("mode", mode_label),
            NativePerfField::str("cursor", cursor_render_mode.as_str()),
            NativePerfField::u64("refresh_hz", u64::from(refresh_hz)),
            NativePerfField::usize("surfaces", surfaces),
            NativePerfField::u64("render_generation", render_generation),
            NativePerfField::bool("render_changed", scene_changed),
            NativePerfField::str("render_cause", render_cause),
            NativePerfField::u64("tick_us", tick_us),
            NativePerfField::bool("pageflip_pending_at_tick", pageflip_pending_at_tick),
            NativePerfField::u64("input_drain_us", input_drain_us),
            NativePerfField::usize("raw_input_events", raw_input_events),
            NativePerfField::usize("coalesced_input_events", coalesced_input_events),
            NativePerfField::u64("pageflip_drain_us", pageflip_drain_us),
            NativePerfField::bool("pageflip_completed", pageflip_completed),
            NativePerfField::u64("present_us", present_us),
            NativePerfField::u64("repaint_present_us", repaint_present_us),
            NativePerfField::bool("render_ahead", render_ahead),
            NativePerfField::bool("render_ahead_ready", render_ahead_ready),
            NativePerfField::u64(
                "acquire_ready_to_render_submit_us",
                acquire_ready_to_render_submit_us,
            ),
            NativePerfField::u64("cpu_user_us", cpu_user_us),
            NativePerfField::u64("cpu_system_us", cpu_system_us),
            NativePerfField::bool("pending_frame_work", pending_frame_work),
            NativePerfField::bool("redraw_requested", redraw_requested),
            NativePerfField::usize("skipped_input_repaints", skipped_input_repaints),
            NativePerfField::usize("accepted_clients", accepted),
        ]);
        fields
    });
}

pub(super) struct PipelineSchedulingDiagnostics {
    pub(super) scheduled_target: Option<PresentationTarget>,
    pub(super) render_ahead_allowed: bool,
    pub(super) worker_queue_available: bool,
}

impl PipelineSchedulingDiagnostics {
    pub(super) const fn new(
        scheduled_target: Option<PresentationTarget>,
        render_ahead_allowed: bool,
        worker_queue_available: bool,
    ) -> Self {
        Self {
            scheduled_target,
            render_ahead_allowed,
            worker_queue_available,
        }
    }
}

pub(super) fn note_same_buffer_suppressed(perf: NativePerfLogger) -> bool {
    perf.log("native.direct_scanout", || {
        vec![NativePerfField::str("transition", "same_buffer_suppressed")]
    });
    true
}

pub(super) fn log_output_pipeline_snapshot(
    perf: NativePerfLogger,
    configured_policy: AdaptiveTripleBufferPolicy,
    pacing_mode: NativeOutputPacingMode,
    pipeline: &OutputPipelineSnapshot,
    scheduling: PipelineSchedulingDiagnostics,
    force_unavailable: Option<TripleCapabilityBlocker>,
    terminal_ownership_valid: bool,
) {
    perf.log("native.presentation_pipeline", || {
        vec![
            NativePerfField::str("configured_policy", configured_policy.as_str()),
            NativePerfField::str("effective_mode", pacing_mode.as_str()),
            NativePerfField::str("capability", pipeline.triple_capability.as_str()),
            NativePerfField::str(
                "current_primary",
                format!("{:?}", pipeline.presented_planes.primary),
            ),
            NativePerfField::str(
                "kernel_submitted",
                format!("{:?}", pipeline.kernel_submitted),
            ),
            NativePerfField::str(
                "worker_queued_next",
                format!("{:?}", pipeline.worker_queued_next),
            ),
            NativePerfField::str("prepared", format!("{:?}", pipeline.prepared)),
            NativePerfField::str(
                "scheduled_target",
                format!("{:?}", scheduling.scheduled_target),
            ),
            NativePerfField::bool("render_ahead_allowed", scheduling.render_ahead_allowed),
            NativePerfField::bool("worker_queue_available", scheduling.worker_queue_available),
            NativePerfField::u64(
                "future_primary_depth",
                u64::from(pipeline.future_primary_depth()),
            ),
            NativePerfField::u64(
                "free_compositor_slots",
                u64::from(pipeline.free_compositor_slots),
            ),
            NativePerfField::bool("direct_active", pipeline.direct_active()),
            NativePerfField::str(
                "force_unavailable",
                force_unavailable.map_or("none", TripleCapabilityBlocker::as_str),
            ),
            NativePerfField::bool("terminal_ownership_valid", terminal_ownership_valid),
        ]
    });
}

#[cfg(test)]
mod tests {
    use super::build_buffering_performance_snapshot;
    use crate::native_output::pacing::NativeBufferingMetrics;
    use oblivion_one::native::adaptive_buffering::{
        AdaptiveBufferingController, AdaptiveRenderJournal, AdaptiveTripleBufferPolicy,
    };
    use std::time::Duration;

    #[test]
    fn buffering_snapshot_builder_preserves_metric_sources() {
        let buffering = NativeBufferingMetrics {
            reactive_double_frames: 1,
            predictive_triple_frames: 2,
            render_ahead_attempts: 3,
            render_ahead_ready: 4,
            ready_submits: 5,
            triple_entries_predicted: 6,
            triple_entries_render_miss: 7,
            triple_entries_submit_miss: 8,
            triple_entries_presentation_miss: 9,
            triple_exits: 10,
            o1_credit2_useful_hits: 11,
            o1_credit2_unnecessary_hits: 12,
            o1_credit2_ineffective_misses: 13,
            o1_credit2_granted_not_consumed: 14,
            o1_credit2_drain_events: 15,
            o1_credit2_refill_suppressed_while_draining: 16,
            ready_pull_in_attempts: 17,
            ready_pull_in_successes: 18,
            ready_pull_in_rejected_too_late: 19,
            ready_pull_in_rejected_owned: 20,
            ready_pull_in_rejected_identity: 21,
            ready_pull_in_advanced_intervals: 22,
        };
        let adaptive = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Auto);
        let prediction = AdaptiveRenderJournal::default()
            .prediction_with_kms_guard(Duration::from_millis(10), 100_000);

        let snapshot = build_buffering_performance_snapshot(buffering, &adaptive, 23, &prediction);

        assert_eq!(snapshot.reactive_double_frames, 1);
        assert_eq!(snapshot.predictive_triple_frames, 2);
        assert_eq!(
            snapshot.future_primary_credit,
            adaptive.future_primary_credit()
        );
        assert_eq!(snapshot.extra_credit_grants, adaptive.extra_credit_grants());
        assert_eq!(
            snapshot.extra_credit_revokes,
            adaptive.extra_credit_revokes()
        );
        assert_eq!(snapshot.o1_credit2_useful_hits, 11);
        assert_eq!(snapshot.o1_credit2_unnecessary_hits, 12);
        assert_eq!(snapshot.o1_credit2_ineffective_misses, 13);
        assert_eq!(snapshot.o1_credit2_granted_not_consumed, 14);
        assert_eq!(snapshot.o1_credit2_drain_events, 15);
        assert_eq!(snapshot.o1_credit2_refill_suppressed_while_draining, 16);
        assert_eq!(snapshot.pre_render_abandoned, 23);
        assert_eq!(
            snapshot.predicted_independent_render_ready_service_ns,
            prediction
                .main_event_loop_wake_guard_ns
                .saturating_add(prediction.render_risk_ns)
        );
        assert_eq!(
            snapshot.predicted_independent_kms_lead_ns,
            prediction.kms_total_lead_ns
        );
        assert_eq!(
            snapshot.predicted_independent_total_service_ns,
            prediction.independent_total_cost_ns
        );
        assert_eq!(
            snapshot.predicted_warm_paired_total_service_ns,
            prediction.warm_paired_total_cost_ns
        );
        assert_eq!(
            snapshot.predicted_independent_p90_floor_ns,
            prediction.independent_p90_floor_ns
        );
        assert_eq!(
            snapshot.predicted_worker_non_ioctl_lead_ns,
            prediction.worker_non_ioctl_lead_ns
        );
        assert_eq!(
            snapshot.predicted_miss_recovery_remaining,
            prediction.miss_recovery_remaining as u64
        );
        assert_eq!(
            snapshot.predicted_total_service_ns,
            prediction.total_cost_ns
        );
        assert_eq!(
            snapshot.last_overlap_required_ns,
            adaptive.last_overlap_required_ns()
        );
        assert_eq!(
            snapshot.positive_overlap_observations,
            adaptive.positive_overlap_observations()
        );
        assert_eq!(
            snapshot.nonpositive_overlap_observations,
            adaptive.nonpositive_overlap_observations()
        );
        assert_eq!(snapshot.render_ahead_attempts, 3);
        assert_eq!(snapshot.render_ahead_ready, 4);
        assert_eq!(snapshot.ready_submits, 5);
        assert_eq!(snapshot.triple_entries_predicted, 6);
        assert_eq!(snapshot.triple_entries_render_miss, 7);
        assert_eq!(snapshot.triple_entries_submit_miss, 8);
        assert_eq!(snapshot.triple_entries_presentation_miss, 9);
        assert_eq!(snapshot.triple_exits, 10);
        assert_eq!(snapshot.ready_pull_in_attempts, 17);
        assert_eq!(snapshot.ready_pull_in_successes, 18);
        assert_eq!(snapshot.ready_pull_in_rejected_too_late, 19);
        assert_eq!(snapshot.ready_pull_in_rejected_owned, 20);
        assert_eq!(snapshot.ready_pull_in_rejected_identity, 21);
        assert_eq!(snapshot.ready_pull_in_advanced_intervals, 22);
    }
}
