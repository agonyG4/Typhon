use super::*;

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
            NativePerfField::str("configured_policy", format!("{configured_policy:?}")),
            NativePerfField::str("effective_mode", format!("{pacing_mode:?}")),
            NativePerfField::str("capability", format!("{:?}", pipeline.triple_capability)),
            NativePerfField::str("current_primary", format!("{:?}", pipeline.current_primary)),
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
            NativePerfField::str("force_unavailable", format!("{force_unavailable:?}")),
            NativePerfField::bool("terminal_ownership_valid", terminal_ownership_valid),
        ]
    });
}
