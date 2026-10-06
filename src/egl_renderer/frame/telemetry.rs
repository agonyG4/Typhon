use oblivion_one::compositor::ResolvedEffectScene;

use super::super::{
    FrameTraceSummary,
    damage::RepaintPlan,
    effects::{EffectGraphMetrics, EffectRuntime},
    scene_state::SceneRenderState,
};

pub(super) fn record_repaint_stats(scene: &mut SceneRenderState, plan: &RepaintPlan) {
    let (width, height) = scene.current_size;
    scene.frame_stats.repaint_mode = plan.mode;
    scene.frame_stats.partial_repaint_complexity_policy = plan.complexity_policy;
    scene.frame_stats.partial_repaint_complexity_action = plan.complexity_action;
    scene.frame_stats.buffer_age = plan.buffer_age;
    scene.frame_stats.current_damage_rects = plan.render_damage.rect_count();
    scene.frame_stats.current_damage_pixels =
        plan.render_damage.pixels(width, height).unwrap_or(u64::MAX);
    scene.frame_stats.repair_damage_rects = plan.repair_damage.rect_count();
    scene.frame_stats.repair_damage_pixels =
        plan.repair_damage.pixels(width, height).unwrap_or(u64::MAX);
    scene.frame_stats.fallback_reason = plan.fallback_reason;
    scene.frame_stats.partial_repaint_enabled = scene.repaint_planner.partial_enabled();
    scene.frame_stats.history_depth = scene.repaint_planner.history_depth();
}

pub(super) fn record_effect_graph_metrics(
    scene: &mut SceneRenderState,
    metrics: EffectGraphMetrics,
) {
    scene.frame_stats.effect_instances_visible = metrics.instances;
    scene.frame_stats.render_graph_passes = metrics.passes;
    scene.frame_stats.render_graph_peak_live_textures = metrics.peak_live_textures;
    scene.frame_stats.effect_graph_peak_live_bytes = metrics.peak_live_bytes;
    scene.frame_stats.effect_capture_pixels = metrics.capture_pixels;
    scene.frame_stats.effect_output_pixels = metrics.output_pixels;
}

pub(super) fn effect_trace_summary(
    scene: &SceneRenderState,
    effects: &ResolvedEffectScene,
    repaint_plan: Option<&RepaintPlan>,
    graph: Option<&oblivion_one::effects::CompiledFrameGraph>,
    selected_effect_count: Option<usize>,
) -> FrameTraceSummary {
    FrameTraceSummary {
        scene_generation: Some(effects.generation),
        repaint_mode: repaint_plan.map(|plan| plan.mode.as_str()),
        render_damage_signature: repaint_plan.map(|plan| plan.render_damage.identity_signature()),
        repair_damage_signature: repaint_plan.map(|plan| plan.repair_damage.identity_signature()),
        visible_effect_count: Some(scene.frame_stats.effect_instances_visible),
        selected_effect_count,
        graph_pass_count: graph.map(|graph| graph.stats.passes),
        graph_texture_count: graph.map(|graph| graph.stats.textures),
        peak_live_intermediate_count: graph.map(|graph| graph.stats.peak_live_intermediates),
        ..FrameTraceSummary::default()
    }
}

pub(super) fn record_effect_resource_metrics(
    scene: &mut SceneRenderState,
    runtime: &EffectRuntime,
) {
    let metrics = runtime.effect_resources.metrics();
    scene.frame_stats.effect_resource_allocations = metrics.allocation_count;
    scene.frame_stats.effect_resource_reuses = metrics.reuse_count;
    scene.frame_stats.effect_resource_evictions = metrics.eviction_count;
    scene.frame_stats.effect_resource_allocations_total = metrics.allocation_count;
    scene.frame_stats.effect_resource_reuses_total = metrics.reuse_count;
    scene.frame_stats.effect_resource_evictions_total = metrics.eviction_count;
    scene.frame_stats.effect_gpu_cache_bytes = metrics.current_bytes;
    scene.frame_stats.effect_gpu_cache_peak_bytes = metrics.peak_bytes;
    scene.frame_stats.effect_gpu_budget_bytes = metrics.budget_bytes;
    scene.frame_stats.effect_gpu_cached_keys = metrics.cached_key_count;
    scene.frame_stats.effect_gpu_cached_textures = metrics.cached_texture_count;
    scene.frame_stats.effect_gpu_checked_out_textures = metrics.checked_out_texture_count;
    let shader_metrics = runtime.effect_shaders.metrics();
    scene.frame_stats.shader_cache_capacity = shader_metrics.capacity;
    scene.frame_stats.shader_cache_entries = shader_metrics.resident_entries;
    scene.frame_stats.shader_cache_peak_entries = shader_metrics.peak_entries;
    scene.frame_stats.shader_cache_evictions_total = shader_metrics.eviction_count;
}
