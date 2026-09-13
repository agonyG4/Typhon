use std::{collections::HashMap, ffi::OsStr, sync::OnceLock};

use oblivion_one::effects::{
    CompiledFrameGraph, CompiledRenderPass, GraphTextureId, GraphTextureSource, RenderPassKind,
};

use super::resources::PooledEffectTexture;

const TRACE_ENV: &str = "TYPHON_EFFECT_EXEC_TRACE";
const MAX_TRACE_INPUTS: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TraceConfig {
    enabled: bool,
}

impl TraceConfig {
    pub(crate) fn from_env(value: Option<&OsStr>) -> Self {
        Self {
            enabled: value == Some(OsStr::new("1")),
        }
    }

    pub(crate) const fn enabled(self) -> bool {
        self.enabled
    }
}

fn trace_config() -> TraceConfig {
    TraceConfig::from_env(std::env::var_os(TRACE_ENV).as_deref())
}

fn tracing_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| trace_config().enabled())
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FrameTraceSummary {
    pub(crate) render_generation: Option<u64>,
    pub(crate) scene_generation: Option<u64>,
    pub(crate) scene_signature: Option<u64>,
    pub(crate) repaint_mode: Option<&'static str>,
    pub(crate) render_damage_signature: Option<u64>,
    pub(crate) repair_damage_signature: Option<u64>,
    pub(crate) visible_effect_count: Option<usize>,
    pub(crate) selected_effect_count: Option<usize>,
    pub(crate) graph_pass_count: Option<usize>,
    pub(crate) graph_texture_count: Option<usize>,
    pub(crate) peak_live_intermediate_count: Option<usize>,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct PassTraceFields<'a> {
    pub(crate) boundary: &'static str,
    pub(crate) pass_id: u64,
    pub(crate) instance_id: u64,
    pub(crate) kind: &'static str,
    pub(crate) input_ids: &'a [GraphTextureId],
    pub(crate) output_id: Option<u64>,
    pub(crate) capture_mode: Option<&'static str>,
    pub(crate) capture_commands: Option<usize>,
}

#[derive(Debug, Default)]
pub(crate) struct PassTraceSummary {
    pub(crate) target_flip_y: bool,
    pub(crate) input_flip_y: bool,
    pub(crate) damage_rect_count: usize,
    pub(crate) damage_bounding_box: Option<(i32, i32, u32, u32)>,
    pub(crate) capture_mode: Option<&'static str>,
    pub(crate) capture_command_count: Option<usize>,
    pub(crate) read_framebuffer: Option<String>,
    pub(crate) draw_framebuffer: Option<String>,
    pub(crate) scratch_fbo_complete: Option<bool>,
}

#[cfg(test)]
pub(crate) fn format_pass_trace_line(fields: PassTraceFields<'_>) -> String {
    let input_ids = fields
        .input_ids
        .iter()
        .take(MAX_TRACE_INPUTS)
        .map(|id| id.get().to_string())
        .collect::<Vec<_>>()
        .join(",");
    let output_id = fields
        .output_id
        .map_or_else(|| "none".to_owned(), |id| id.to_string());
    let capture_mode = fields.capture_mode.unwrap_or("none");
    let capture_commands = fields
        .capture_commands
        .map_or_else(|| "none".to_owned(), |count| count.to_string());
    format!(
        "event=effect_pass_{} pass={} instance={} kind={} inputs={} output={} capture_mode={} capture_commands={}",
        fields.boundary,
        fields.pass_id,
        fields.instance_id,
        fields.kind,
        input_ids,
        output_id,
        capture_mode,
        capture_commands,
    )
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct EffectExecutionTrace {
    enabled: bool,
    frame_id: Option<u64>,
    render_generation: Option<u64>,
    scene_generation: Option<u64>,
    scene_signature: Option<u64>,
}

impl EffectExecutionTrace {
    pub(crate) fn new(
        frame_id: Option<u64>,
        render_generation: Option<u64>,
        scene_generation: Option<u64>,
        scene_signature: Option<u64>,
    ) -> Self {
        Self {
            enabled: tracing_enabled(),
            frame_id,
            render_generation,
            scene_generation,
            scene_signature,
        }
    }

    #[cfg(test)]
    pub(crate) const fn disabled_for_test() -> Self {
        Self {
            enabled: false,
            frame_id: None,
            render_generation: None,
            scene_generation: None,
            scene_signature: None,
        }
    }

    pub(crate) const fn enabled(self) -> bool {
        self.enabled
    }

    pub(crate) fn event<F>(&self, make_line: F)
    where
        F: FnOnce() -> String,
    {
        if self.enabled {
            eprintln!("typhon effect: {}", make_line());
        }
    }

    pub(crate) fn frame_boundary(
        &self,
        phase: &'static str,
        boundary: &'static str,
        summary: FrameTraceSummary,
    ) {
        self.event(|| {
            format!(
                "event={phase}_{boundary} frame_id={} render_generation={} scene_generation={} scene_signature={} repaint_mode={} render_damage={} repair_damage={} visible_effects={} selected_effects={} graph_passes={} graph_textures={} peak_live_intermediates={}",
                optional_u64(self.frame_id),
                optional_u64(summary.render_generation.or(self.render_generation)),
                optional_u64(summary.scene_generation.or(self.scene_generation)),
                optional_u64(summary.scene_signature.or(self.scene_signature)),
                summary.repaint_mode.unwrap_or("unknown"),
                optional_u64(summary.render_damage_signature),
                optional_u64(summary.repair_damage_signature),
                optional_usize(summary.visible_effect_count),
                optional_usize(summary.selected_effect_count),
                optional_usize(summary.graph_pass_count),
                optional_usize(summary.graph_texture_count),
                optional_usize(summary.peak_live_intermediate_count),
            )
        });
    }

    pub(crate) fn pass_boundary(
        &self,
        boundary: &'static str,
        pass: &CompiledRenderPass,
        graph: &CompiledFrameGraph,
        resources: &HashMap<GraphTextureId, PooledEffectTexture>,
        summary: PassTraceSummary,
    ) {
        self.event(|| {
            let input_ids = pass
                .inputs
                .iter()
                .take(MAX_TRACE_INPUTS)
                .map(|id| id.get().to_string())
                .collect::<Vec<_>>()
                .join(",");
            let input_details = pass
                .inputs
                .iter()
                .take(MAX_TRACE_INPUTS)
                .filter_map(|id| graph_texture_trace(graph, *id, resources))
                .collect::<Vec<_>>()
                .join(";");
            let output_details = pass
                .output
                .and_then(|id| graph_texture_trace(graph, id, resources))
                .unwrap_or_else(|| "none".to_owned());
            let bbox = summary.damage_bounding_box.map_or_else(
                || "none".to_owned(),
                |(x, y, width, height)| format!("{x},{y},{width},{height}"),
            );
            format!(
                "event=effect_pass_{boundary} frame_id={} pass={} instance={} kind={} anchor={:?} anchor_scope={:?} visual_group={} inputs={} input_details={} output={} target_flip_y={} input_flip_y={} damage_rects={} damage_bbox={} checkpoints={} capture_mode={} capture_commands={} read_fbo={} draw_fbo={} scratch_fbo_complete={}",
                optional_u64(self.frame_id),
                pass.id.get(),
                pass.instance.get(),
                render_pass_kind_name(pass.kind),
                pass.anchor,
                pass.anchor_scope,
                pass.visual_group.map_or(0, |group| group.get()),
                input_ids,
                input_details,
                output_details,
                summary.target_flip_y,
                summary.input_flip_y,
                summary.damage_rect_count,
                bbox,
                pass.checkpoint_dependencies.len(),
                summary.capture_mode.unwrap_or("none"),
                summary
                    .capture_command_count
                    .map_or_else(|| "none".to_owned(), |count| count.to_string()),
                summary
                    .read_framebuffer
                    .as_deref()
                    .unwrap_or("none"),
                summary
                    .draw_framebuffer
                    .as_deref()
                    .unwrap_or("none"),
                summary
                    .scratch_fbo_complete
                    .map_or_else(|| "unknown".to_owned(), |complete| complete.to_string()),
            )
        });
    }

    pub(crate) fn invariant_failure(&self, error: &dyn std::error::Error) {
        self.event(|| format!("event=effect_invariant_failure invariant={error}"));
    }
}

fn render_pass_kind_name(kind: RenderPassKind) -> &'static str {
    match kind {
        RenderPassKind::SceneCapture => "SceneCapture",
        RenderPassKind::SurfaceCapture => "SurfaceCapture",
        RenderPassKind::NormalizeInput => "NormalizeInput",
        RenderPassKind::DualKawaseDownsample => "DualKawaseDownsample",
        RenderPassKind::DualKawaseUpsample => "DualKawaseUpsample",
        RenderPassKind::Fragment => "Fragment",
        RenderPassKind::Blend => "Blend",
        RenderPassKind::Mask => "Mask",
        RenderPassKind::Composite => "Composite",
        RenderPassKind::OutputPostProcess => "OutputPostProcess",
    }
}

fn graph_texture_trace(
    graph: &CompiledFrameGraph,
    id: GraphTextureId,
    resources: &HashMap<GraphTextureId, PooledEffectTexture>,
) -> Option<String> {
    let texture = graph.textures.iter().find(|texture| texture.id == id)?;
    let physical = resources
        .get(&id)
        .map_or_else(|| "output".to_owned(), |texture| texture.id.to_string());
    let source = match texture.source {
        GraphTextureSource::Output => "output",
        GraphTextureSource::CapturedScene => "captured_scene",
        GraphTextureSource::CapturedTarget => "captured_target",
        GraphTextureSource::Intermediate => "intermediate",
        GraphTextureSource::Static(_) => "static",
    };
    Some(format!(
        "id={}:physical={physical}:source={source}:domain={},{} {}x{}:dims={}x{}:origin=bottom_left",
        id.get(),
        texture.domain.x,
        texture.domain.y,
        texture.domain.width,
        texture.domain.height,
        texture.width,
        texture.height,
    ))
}

fn optional_u64(value: Option<u64>) -> String {
    value.map_or_else(|| "unknown".to_owned(), |value| value.to_string())
}

fn optional_usize(value: Option<usize>) -> String {
    value.map_or_else(|| "unknown".to_owned(), |value| value.to_string())
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    #[test]
    fn trace_gate_requires_exact_one() {
        assert!(!TraceConfig::from_env(None).enabled());
        assert!(!TraceConfig::from_env(Some(OsStr::new(""))).enabled());
        assert!(!TraceConfig::from_env(Some(OsStr::new("0"))).enabled());
        assert!(!TraceConfig::from_env(Some(OsStr::new("true"))).enabled());
        assert!(TraceConfig::from_env(Some(OsStr::new("1"))).enabled());
    }

    #[test]
    fn disabled_trace_does_not_evaluate_event_formatter() {
        let trace = EffectExecutionTrace::disabled_for_test();
        let formatted = Cell::new(false);

        trace.event(|| {
            formatted.set(true);
            "must not be built".to_owned()
        });

        assert!(!formatted.get());
    }

    #[test]
    fn pass_trace_line_is_bounded_and_source_free() {
        let line = format_pass_trace_line(PassTraceFields {
            boundary: "begin",
            pass_id: 7,
            instance_id: 11,
            kind: "DualKawaseDownsample",
            input_ids: &[
                GraphTextureId::new(2).unwrap(),
                GraphTextureId::new(3).unwrap(),
                GraphTextureId::new(4).unwrap(),
                GraphTextureId::new(5).unwrap(),
                GraphTextureId::new(6).unwrap(),
                GraphTextureId::new(7).unwrap(),
                GraphTextureId::new(8).unwrap(),
                GraphTextureId::new(9).unwrap(),
                GraphTextureId::new(10).unwrap(),
            ],
            output_id: Some(12),
            capture_mode: Some("replay"),
            capture_commands: Some(13),
        });

        assert!(line.contains("event=effect_pass_begin"));
        assert!(line.contains("pass=7"));
        assert!(line.contains("inputs=2,3,4,5,6,7,8"));
        assert!(!line.contains("shader"));
        assert!(!line.contains("scene_commands"));
    }
}
