use super::*;
use oblivion_one::native::adaptive_buffering::{AdaptiveBufferingController, RenderPrediction};

#[allow(clippy::too_many_arguments)]
pub(super) fn build_render_begin_fields(
    frame_id: Option<NativeOutputFrameId>,
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
    pending_frame_work: bool,
) {
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
            NativePerfField::bool("pending_frame_work", pending_frame_work),
        ]);
        fields
    });
    if pending_frame_work {
        let finish_frame_start = Instant::now();
        server.finish_frame();
        perf.log("native.finish_frame", || {
            vec![
                NativePerfField::str("reason", "empty_visible_damage"),
                NativePerfField::u64("elapsed_us", elapsed_micros(finish_frame_start)),
                NativePerfField::usize("surfaces", server.renderable_surfaces().len()),
                NativePerfField::u64("render_generation", server.render_generation()),
            ]
        });
    }
    frame_scheduler.note_immediate_completion();
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
