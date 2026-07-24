use super::*;

#[derive(Debug, Clone, Copy)]
pub(super) struct ProtocolCycleMetrics {
    pub(super) skipped_input_repaints: usize,
    pub(super) tick_us: u64,
    pub(super) pageflip_pending_at_tick: bool,
    pub(super) input_drain_us: u64,
    pub(super) raw_input_events: usize,
    pub(super) coalesced_input_events: usize,
    pub(super) pageflip_drain_us: u64,
    pub(super) pageflip_completed: bool,
    pub(super) present_us: u64,
    pub(super) render_generation: u64,
    pub(super) effective_render_target_available: bool,
    pub(super) scene_changed: bool,
    pub(super) pending_frame_work: bool,
    pub(super) redraw_requested: bool,
}

impl ProtocolCycleMetrics {
    pub(super) const fn from_cycle(
        cycle: &NativeCycleState,
        render_generation: u64,
        effective_render_target_available: bool,
        scene_changed: bool,
        pending_frame_work: bool,
        redraw_requested: bool,
    ) -> Self {
        Self {
            skipped_input_repaints: cycle.skipped_input_repaints,
            tick_us: cycle.tick_us,
            pageflip_pending_at_tick: cycle.pageflip_pending_at_tick,
            input_drain_us: cycle.input_drain_us,
            raw_input_events: cycle.raw_input_events,
            coalesced_input_events: cycle.coalesced_input_events,
            pageflip_drain_us: cycle.pageflip_drain_us,
            pageflip_completed: cycle.pageflip_completed,
            present_us: cycle.present_us,
            render_generation,
            effective_render_target_available,
            scene_changed,
            pending_frame_work,
            redraw_requested,
        }
    }
}

pub(super) fn complete_protocol_only_tick(
    server: &mut OwnCompositorServer,
    frame_scheduler: &mut NativeFrameScheduler,
    perf: NativePerfLogger,
    metrics: ProtocolCycleMetrics,
) {
    perf.log("native.frame_skip", || {
        vec![
            NativePerfField::str("reason", "frame_callback_no_damage"),
            NativePerfField::usize("skipped_input_repaints", metrics.skipped_input_repaints),
            NativePerfField::u64("tick_us", metrics.tick_us),
            NativePerfField::bool("pageflip_pending_at_tick", metrics.pageflip_pending_at_tick),
            NativePerfField::u64("input_drain_us", metrics.input_drain_us),
            NativePerfField::usize("raw_input_events", metrics.raw_input_events),
            NativePerfField::usize("coalesced_input_events", metrics.coalesced_input_events),
            NativePerfField::u64("pageflip_drain_us", metrics.pageflip_drain_us),
            NativePerfField::bool("pageflip_completed", metrics.pageflip_completed),
            NativePerfField::u64("present_us", metrics.present_us),
            NativePerfField::u64("render_generation", metrics.render_generation),
        ]
    });
    let finish_frame_start = Instant::now();
    let output_time = server.frame_callback_time_for_output();
    let protocol_completion = server.complete_protocol_only_frame_tick(output_time);
    frame_scheduler.complete_protocol_only();
    perf.log("native.finish_frame", || {
        vec![
            NativePerfField::str("reason", "frame_callback_no_damage"),
            NativePerfField::str("completion", format!("{protocol_completion:?}")),
            NativePerfField::u64("elapsed_us", elapsed_micros(finish_frame_start)),
            NativePerfField::usize("surfaces", server.renderable_surfaces().len()),
            NativePerfField::u64("render_generation", server.render_generation()),
        ]
    });
}

pub(super) fn log_wait_for_presentation(
    frame_pacing: &mut NativeFramePacing,
    scanout: &NativeScanoutBackend,
    perf: NativePerfLogger,
    scheduler_decision: SchedulerDecision,
    metrics: ProtocolCycleMetrics,
) -> NativeResult<()> {
    let snapshot = scanout.buffer_snapshot();
    let event = if scheduler_decision == SchedulerDecision::WaitForBuffer {
        frame_pacing.wait_for_buffer_count += 1;
        "wait_for_buffer"
    } else {
        "wait_for_pageflip"
    };
    let now_ns = monotonic_now_ns()?;
    let mut fields = vec![
        frame_id_field(frame_pacing.active),
        PacingField::bool("pageflip_pending", scanout.page_flip_pending()),
        PacingField::u64(
            "time_since_last_pageflip_us",
            frame_pacing
                .last_pageflip_ns()
                .map_or(0, |last| now_ns.saturating_sub(last) / 1_000),
        ),
        PacingField::u64(
            "time_since_visual_queued_us",
            frame_pacing
                .active_queued_ns
                .map_or(0, |queued| now_ns.saturating_sub(queued) / 1_000),
        ),
    ];
    fields.extend(snapshot_fields(snapshot));
    frame_pacing.log(event, fields);
    perf.log("native.frame_skip", || {
        vec![
            NativePerfField::str(
                "reason",
                if scheduler_decision == SchedulerDecision::WaitForBuffer {
                    "render_target_unavailable"
                } else {
                    "pageflip_pending"
                },
            ),
            NativePerfField::usize("skipped_input_repaints", metrics.skipped_input_repaints),
            NativePerfField::u64("tick_us", metrics.tick_us),
            NativePerfField::bool("pageflip_pending_at_tick", metrics.pageflip_pending_at_tick),
            NativePerfField::bool(
                "render_target_available",
                metrics.effective_render_target_available,
            ),
            NativePerfField::u64("input_drain_us", metrics.input_drain_us),
            NativePerfField::usize("raw_input_events", metrics.raw_input_events),
            NativePerfField::usize("coalesced_input_events", metrics.coalesced_input_events),
            NativePerfField::u64("pageflip_drain_us", metrics.pageflip_drain_us),
            NativePerfField::bool("pageflip_completed", metrics.pageflip_completed),
            NativePerfField::u64("present_us", metrics.present_us),
            NativePerfField::u64("render_generation", metrics.render_generation),
            NativePerfField::bool("render_changed", metrics.scene_changed),
            NativePerfField::bool("pending_frame_work", metrics.pending_frame_work),
            NativePerfField::bool("redraw_requested", metrics.redraw_requested),
        ]
    });
    Ok(())
}

pub(super) fn log_no_visual_work(perf: NativePerfLogger, metrics: ProtocolCycleMetrics) {
    perf.log("native.frame_skip", || {
        vec![
            NativePerfField::str("reason", "input_forwarded_no_visual"),
            NativePerfField::usize("skipped_input_repaints", metrics.skipped_input_repaints),
            NativePerfField::u64("tick_us", metrics.tick_us),
            NativePerfField::bool("pageflip_pending_at_tick", metrics.pageflip_pending_at_tick),
            NativePerfField::u64("input_drain_us", metrics.input_drain_us),
            NativePerfField::usize("raw_input_events", metrics.raw_input_events),
            NativePerfField::usize("coalesced_input_events", metrics.coalesced_input_events),
            NativePerfField::u64("pageflip_drain_us", metrics.pageflip_drain_us),
            NativePerfField::bool("pageflip_completed", metrics.pageflip_completed),
            NativePerfField::u64("present_us", metrics.present_us),
            NativePerfField::u64("render_generation", metrics.render_generation),
        ]
    });
}
