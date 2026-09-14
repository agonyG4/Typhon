use std::{collections::HashMap, ffi::OsStr, sync::OnceLock};

#[cfg(test)]
use std::cell::RefCell;

use oblivion_one::effects::{
    CompiledFrameGraph, CompiledRenderPass, EffectDemandPlanStats, GraphTextureId,
    GraphTextureSource, RenderPassKind,
};

use super::resources::PooledEffectTexture;

const TRACE_ENV: &str = "TYPHON_EFFECT_EXEC_TRACE";
const MAX_TRACE_INPUTS: usize = 8;

#[cfg(test)]
thread_local! {
    static TEST_EVENTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

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
    pub(crate) demand_plan: Option<EffectDemandPlanStats>,
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
    pub(crate) framebuffer_origin: Option<&'static str>,
    pub(crate) damage_rect_count: usize,
    pub(crate) damage_bounding_box: Option<(i32, i32, u32, u32)>,
    pub(crate) capture_mode: Option<&'static str>,
    pub(crate) capture_command_count: Option<usize>,
    pub(crate) read_framebuffer: Option<String>,
    pub(crate) draw_framebuffer: Option<String>,
    pub(crate) scratch_fbo_present: Option<bool>,
    pub(crate) conservative_pass_demand: bool,
    pub(crate) conservative_pass_demand_kind: &'static str,
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

    #[cfg(test)]
    pub(crate) const fn enabled_for_test() -> Self {
        Self {
            enabled: true,
            frame_id: None,
            render_generation: None,
            scene_generation: None,
            scene_signature: None,
        }
    }

    pub(crate) const fn enabled(self) -> bool {
        self.enabled
    }

    pub(crate) const fn frame_id(self) -> Option<u64> {
        self.frame_id
    }

    pub(crate) fn event<F>(&self, make_line: F)
    where
        F: FnOnce() -> String,
    {
        if self.enabled {
            let line = make_line();
            #[cfg(test)]
            TEST_EVENTS.with(|events| events.borrow_mut().push(line.clone()));
            eprintln!("typhon effect: {line}");
        }
    }

    pub(crate) fn scene_replay_boundary(
        &self,
        boundary: &'static str,
        pass: &CompiledRenderPass,
        reason: &'static str,
        scene_cursor_start: usize,
        scene_cursor_end: usize,
    ) {
        self.event(|| {
            format!(
                "event=effect_scene_replay_{boundary} frame_id={} pass={} kind={} reason={reason} scene_cursor_start={scene_cursor_start} scene_cursor_end={scene_cursor_end} command_count={}",
                optional_u64(self.frame_id),
                pass.id.get(),
                render_pass_kind_name(pass.kind),
                scene_cursor_end.saturating_sub(scene_cursor_start),
            )
        });
    }

    pub(crate) fn final_scene_replay_boundary(
        &self,
        boundary: &'static str,
        scene_cursor_start: usize,
        scene_cursor_end: usize,
    ) {
        self.event(|| {
            format!(
                "event=effect_final_scene_replay_{boundary} frame_id={} scene_cursor_start={scene_cursor_start} scene_cursor_end={scene_cursor_end} command_count={}",
                optional_u64(self.frame_id),
                scene_cursor_end.saturating_sub(scene_cursor_start),
            )
        });
    }

    pub(crate) fn overlay_boundary(&self, boundary: &'static str) {
        self.event(|| {
            format!(
                "event=effect_overlay_draw_{boundary} frame_id={}",
                optional_u64(self.frame_id),
            )
        });
    }

    pub(crate) fn frame_boundary(
        &self,
        phase: &'static str,
        boundary: &'static str,
        summary: FrameTraceSummary,
    ) {
        self.event(|| {
            format!(
                "event={phase}_{boundary} frame_id={} render_generation={} scene_generation={} scene_signature={} repaint_mode={} render_damage={} repair_damage={} visible_effects={} selected_effects={} graph_passes={} graph_textures={} peak_live_intermediates={} repair_rect_count={} dependency_edge_count={} dependency_propagations={} max_instance_region_rect_count={} region_representation_overflows={} visible_clip_fallbacks={} work_region_bbox_coalesces={} conservative_full={} pass_count_selected={} partial_pass_count={} full_domain_pass_count={} pass_dependency_propagations={} max_pass_region_rect_count={} pass_conservative_fallbacks={}",
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
                summary
                    .demand_plan
                    .map(|stats| stats.repair_rect_count)
                    .map_or_else(|| "unknown".to_owned(), |count| count.to_string()),
                summary
                    .demand_plan
                    .map(|stats| stats.dependency_edge_count)
                    .map_or_else(|| "unknown".to_owned(), |count| count.to_string()),
                summary
                    .demand_plan
                    .map(|stats| stats.dependency_propagations)
                    .map_or_else(|| "unknown".to_owned(), |count| count.to_string()),
                summary
                    .demand_plan
                    .map(|stats| stats.max_instance_region_rect_count)
                    .map_or_else(|| "unknown".to_owned(), |count| count.to_string()),
                summary
                    .demand_plan
                    .map(|stats| stats.region_representation_overflows)
                    .map_or_else(|| "unknown".to_owned(), |count| count.to_string()),
                summary
                    .demand_plan
                    .map(|stats| stats.visible_clip_fallbacks)
                    .map_or_else(|| "unknown".to_owned(), |count| count.to_string()),
                summary
                    .demand_plan
                    .map(|stats| stats.work_region_bbox_coalesces)
                    .map_or_else(|| "unknown".to_owned(), |count| count.to_string()),
                summary.demand_plan.map_or_else(
                    || "unknown".to_owned(),
                    |stats| stats.conservative_full.to_string(),
                ),
                summary
                    .demand_plan
                    .map(|stats| stats.pass_count_selected)
                    .map_or_else(|| "unknown".to_owned(), |count| count.to_string()),
                summary
                    .demand_plan
                    .map(|stats| stats.partial_pass_count)
                    .map_or_else(|| "unknown".to_owned(), |count| count.to_string()),
                summary
                    .demand_plan
                    .map(|stats| stats.full_domain_pass_count)
                    .map_or_else(|| "unknown".to_owned(), |count| count.to_string()),
                summary
                    .demand_plan
                    .map(|stats| stats.pass_dependency_propagations)
                    .map_or_else(|| "unknown".to_owned(), |count| count.to_string()),
                summary
                    .demand_plan
                    .map(|stats| stats.max_pass_region_rect_count)
                    .map_or_else(|| "unknown".to_owned(), |count| count.to_string()),
                summary
                    .demand_plan
                    .map(|stats| stats.pass_conservative_fallbacks)
                    .map_or_else(|| "unknown".to_owned(), |count| count.to_string()),
            )
        });
    }

    pub(crate) fn visible_clip_fallback(
        &self,
        pass: &CompiledRenderPass,
        input_rect_count: usize,
        clip_rect_count: usize,
    ) {
        self.event(|| {
            format!(
                "event=region_representation_overflow frame_id={} instance={} pass={} input_rect_count={} clip_rect_count={} fallback=output_influence visible_clip_fallback=true",
                optional_u64(self.frame_id),
                pass.instance.get(),
                pass.id.get(),
                input_rect_count,
                clip_rect_count,
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
                "event=effect_pass_{boundary} frame_id={} pass={} instance={} kind={} anchor={:?} anchor_scope={:?} visual_group={} inputs={} input_details={} output={} framebuffer_origin={} target_flip_y={} input_flip_y={} damage_rects={} damage_bbox={} checkpoints={} capture_mode={} conservative_pass_demand={} conservative_pass_demand_kind={} capture_commands={} read_fbo={} draw_fbo={} scratch_fbo_present={}",
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
                summary.framebuffer_origin.unwrap_or("unknown"),
                summary.target_flip_y,
                summary.input_flip_y,
                summary.damage_rect_count,
                bbox,
                pass.checkpoint_dependencies.len(),
                summary.capture_mode.unwrap_or("none"),
                summary.conservative_pass_demand,
                summary.conservative_pass_demand_kind,
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
                    .scratch_fbo_present
                    .map_or_else(|| "unknown".to_owned(), |complete| complete.to_string()),
            )
        });
    }

    pub(crate) fn invariant_failure(&self, error: &dyn std::error::Error) {
        self.event(|| format!("event=effect_invariant_failure invariant={error}"));
    }
}

#[cfg(test)]
pub(crate) fn clear_test_events() {
    TEST_EVENTS.with(|events| events.borrow_mut().clear());
}

#[cfg(test)]
pub(crate) fn take_test_events() -> Vec<String> {
    TEST_EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut()))
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
    let source = match texture.source {
        GraphTextureSource::Output => "output",
        GraphTextureSource::CapturedScene => "captured_scene",
        GraphTextureSource::CapturedTarget => "captured_target",
        GraphTextureSource::Intermediate => "intermediate",
        GraphTextureSource::Static(_) => "static",
    };
    let physical = match texture.source {
        GraphTextureSource::Output => "output".to_owned(),
        _ => resources
            .get(&id)
            .map_or_else(|| "unrealized".to_owned(), |texture| texture.id.to_string()),
    };
    let origin = match texture.origin {
        oblivion_one::effects::GraphTextureOrigin::BottomLeft => "bottom_left",
    };
    Some(format!(
        "id={}:physical={physical}:source={source}:domain={},{} {}x{}:dims={}x{}:origin={origin}",
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
    use oblivion_one::compositor::{EffectAnchor, EffectAnchorScope};
    use oblivion_one::effects::{
        EffectAlphaMode, EffectColorConversion, EffectInstanceId, EffectRegion, EffectWorkingSpace,
        GraphTextureOrigin, GraphTexturePlan, RenderGraphCompileStats, RenderPassKind,
    };

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

    #[test]
    fn unrealized_graph_textures_are_not_reported_as_output() {
        let output_id = GraphTextureId::new(1).expect("output texture id");
        let capture_id = GraphTextureId::new(2).expect("capture texture id");
        let domain =
            oblivion_one::effects::EffectRect::new(5, 6, 10, 11).expect("trace texture domain");
        let graph = CompiledFrameGraph {
            passes: Vec::new(),
            textures: vec![
                GraphTexturePlan {
                    id: output_id,
                    source: GraphTextureSource::Output,
                    width: 100,
                    height: 80,
                    domain,
                    working_space: EffectWorkingSpace::OutputEncodedSrgb,
                    origin: GraphTextureOrigin::BottomLeft,
                    first_use: None,
                    last_use: None,
                },
                GraphTexturePlan {
                    id: capture_id,
                    source: GraphTextureSource::CapturedScene,
                    width: 10,
                    height: 11,
                    domain,
                    working_space: EffectWorkingSpace::OutputEncodedSrgb,
                    origin: GraphTextureOrigin::BottomLeft,
                    first_use: None,
                    last_use: None,
                },
            ],
            instances: Vec::new(),
            final_damage: EffectRegion::empty(),
            stats: RenderGraphCompileStats::default(),
        };
        let resources = HashMap::new();

        let output_line = graph_texture_trace(&graph, output_id, &resources).unwrap();
        let capture_line = graph_texture_trace(&graph, capture_id, &resources).unwrap();

        assert!(output_line.contains("physical=output:source=output"));
        assert!(capture_line.contains("physical=unrealized:source=captured_scene"));
        assert!(!capture_line.contains("physical=output"));
    }

    #[test]
    fn demand_plan_trace_line_includes_bounded_planner_stats() {
        let trace = EffectExecutionTrace::enabled_for_test();
        let mut summary = FrameTraceSummary::default();
        summary.demand_plan = Some(oblivion_one::effects::EffectDemandPlanStats {
            repair_rect_count: 71,
            dependency_edge_count: 6,
            dependency_propagations: 6,
            max_instance_region_rect_count: 72,
            region_representation_overflows: 3,
            visible_clip_fallbacks: 2,
            work_region_bbox_coalesces: 1,
            conservative_full: false,
            ..Default::default()
        });

        clear_test_events();
        trace.frame_boundary("effect_demand_plan", "end", summary);
        let line = take_test_events().pop().expect("demand trace event");

        assert!(line.contains("repair_rect_count=71"));
        assert!(line.contains("dependency_edge_count=6"));
        assert!(line.contains("dependency_propagations=6"));
        assert!(line.contains("max_instance_region_rect_count=72"));
        assert!(line.contains("region_representation_overflows=3"));
        assert!(line.contains("visible_clip_fallbacks=2"));
        assert!(line.contains("work_region_bbox_coalesces=1"));
        assert!(line.contains("conservative_full=false"));
    }

    #[test]
    fn visible_clip_fallback_trace_identifies_authoritative_output() {
        let trace = EffectExecutionTrace::enabled_for_test();
        let pass = CompiledRenderPass {
            id: oblivion_one::effects::GraphPassId::new(9).unwrap(),
            kind: RenderPassKind::Composite,
            inputs: Vec::new(),
            output: None,
            damage: EffectRegion::empty(),
            instance: EffectInstanceId::new(4).unwrap(),
            anchor: EffectAnchor::OutputPostProcess,
            blur_radius: None,
            stage: None,
            fused_stages: Vec::new(),
            parameter_block: oblivion_one::effects::EffectParameterBlock::default(),
            alpha_mode: EffectAlphaMode::Preserve,
            encode_output: false,
            color_conversion: EffectColorConversion::None,
            checkpoint_dependencies: Vec::new(),
            visual_group: None,
            anchor_scope: EffectAnchorScope::VisualGroup,
            visible_clip_fallback: None,
        };

        clear_test_events();
        trace.visible_clip_fallback(&pass, 970, 33);
        let line = take_test_events()
            .pop()
            .expect("visible clip fallback event");

        assert!(line.contains("event=region_representation_overflow"));
        assert!(line.contains("frame_id=unknown"));
        assert!(line.contains("instance=4"));
        assert!(line.contains("pass=9"));
        assert!(line.contains("input_rect_count=970"));
        assert!(line.contains("clip_rect_count=33"));
        assert!(line.contains("fallback=output_influence"));
        assert!(line.contains("visible_clip_fallback=true"));
    }
}
