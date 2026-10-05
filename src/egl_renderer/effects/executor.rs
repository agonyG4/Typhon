//! Effect execution façade. Sessions prepare bindings, run the graph core,
//! and explicitly finalize those bindings. The renderer composes lifecycle and
//! ordinary overlays between core execution and finalization, with the outer
//! session still owning its checked-out resources and logical trace phase.
//!
//! Scene replay uses a temporary concrete context over authoritative owners;
//! neither sessions nor contexts own renderer resources or retained visuals.

use std::{collections::HashMap, io, time::Instant};

use glow::HasContext;
use oblivion_one::effects::{
    CompiledFrameGraph, CompiledRenderPass, EffectAlphaMode, EffectColorConversion,
    EffectExecutionDemand, EffectFrameDemand, EffectInstanceId, EffectNodeKind, EffectRect,
    EffectRegion, GraphPassId, GraphTextureId, GraphTextureSource,
    INTERNAL_EFFECT_SHADER_MODULE_BLEND, INTERNAL_EFFECT_SHADER_MODULE_FRAGMENT,
    INTERNAL_EFFECT_SHADER_MODULE_MASK, MAX_EFFECT_REGION_RECTS, RenderPassKind, ShaderModuleId,
    logical_rect_to_physical_coverage,
};

use super::super::damage::{OutputDamage, RepaintPlan};
use super::super::geometry::{
    EglDrawCommand, EglDrawLayer, EglTexturedVertex, EglVisibilityDecision, SurfaceConsumerPlan,
    SurfaceSampling, add_surface_consumers_for_capture_indices,
    add_surface_consumers_for_command_range, plan_capture_visibility,
};
use super::super::{
    EffectExecutionTargets, EffectFramebufferTarget, EglRect, GlesSceneFrameStats,
    OutputFramebufferOrigin, OutputRect, RendererResult, SceneRenderState, SceneTextureSources,
    VisualGroupId, ensure_vertex_buffer_capacity, intersect_output_rect, output_rect_for_egl_clip,
    replay_capture_region_layout,
};
use super::gpu_timing::{
    CaptureExecutionTimingSummary, CaptureTimingMetadata, CaptureTimingMode,
    CompositeSceneReplayExecutionDetail, CompositeSceneReplayWork, PassTimingWork,
    ReplayCaptureExecutionDetail,
};
use super::{
    CapturePathFallbackReason, CheckpointCapturePath, EffectDebugCaptureMode, EffectDebugConfig,
    EffectDebugKawaseMode, EffectRuntime, FrameTraceSummary, PassTraceSummary, blur, capture,
    effect_debug_config,
    resources::{
        CheckpointCacheAdmissionStats, CheckpointCacheCompatibility, CheckpointCapturePreparation,
        EffectTextureFilter, EffectTextureFormat, EffectTextureKey, GraphTextureBinding,
        PooledEffectTexture, PreparedCheckpointCaptures, checkpoint_capture_cache_key,
        estimate_graph_peak_bytes, release_dead_graph_textures,
    },
    shader_cache::{ShaderProgramCache, ShaderProgramKey},
};

mod context;
pub(in crate::egl_renderer) use context::EffectExecutionContext;
mod selection;
#[cfg(test)]
pub(crate) use selection::plan_effect_surface_consumers_with_debug_config;
pub(crate) use selection::{
    EffectExecutionSelection, plan_effect_surface_consumers, select_effect_execution,
};
mod session;
#[cfg(test)]
pub(crate) use session::checkpoint_capture_cache_candidates;
pub(crate) use session::execute_effect_graph_for_lifecycle;
#[cfg(test)]
pub(crate) use session::{
    execute_effect_graph, execute_effect_graph_with_debug_config,
    execute_effect_graph_with_debug_config_and_scene_replay_mode,
};
pub(in crate::egl_renderer) use session::{
    execute_prepared_effect_graph_core, finish_prepared_effect_graph_execution,
    prepare_effect_graph_execution,
};
mod graph_runner;
use graph_runner::*;
mod planning;
use planning::*;
mod validation;
pub(crate) use validation::EffectExecutionInvariantError;
use validation::*;
mod telemetry;
pub(crate) use telemetry::EffectExecutionStats;
pub(in crate::egl_renderer) use telemetry::composition_range;
use telemetry::*;
mod pass_exec;
use pass_exec::*;
pub(crate) use pass_exec::{EffectPassBlendMode, establish_effect_pass_blend_state};
mod capture_exec;
#[cfg(test)]
use capture_exec::SceneWorkPreservation;
use capture_exec::*;
mod replay;
pub(crate) use replay::SceneReplayWorkMode;
use replay::*;
mod checkpoint;
use checkpoint::*;
#[cfg(test)]
pub(crate) use checkpoint::{checkpoint_causal_stability_plan, checkpoint_update_rects_for_test};

mod shaders;
pub(crate) use shaders::{
    BLEND_STAGE_FRAGMENT_SHADER, COMPOSITE_FRAGMENT_SHADER, COPY_FRAGMENT_SHADER,
    FRAGMENT_STAGE_FRAGMENT_SHADER, MASK_STAGE_FRAGMENT_SHADER, NORMALIZE_FRAGMENT_SHADER,
};

#[cfg(test)]
pub(crate) fn capture_scene_work_preservation(
    renderer: &mut super::super::GlesSceneRenderer,
    output_size: (u32, u32),
    framebuffer_origin: OutputFramebufferOrigin,
    extra_scene_work: &[OutputRect],
) -> RendererResult<SceneWorkPreservation> {
    let mut context = renderer.effect_execution_context();
    capture_scene_work_preservation_context(
        &mut context,
        output_size,
        framebuffer_origin,
        extra_scene_work,
    )
}

#[cfg(test)]
pub(crate) fn restore_scene_work_preservation(
    renderer: &mut super::super::GlesSceneRenderer,
    preservation: &SceneWorkPreservation,
) -> RendererResult<()> {
    let mut context = renderer.effect_execution_context();
    restore_scene_work_preservation_context(&mut context, preservation)
}

#[cfg(test)]
pub(crate) fn capture_output_region_to_graph_texture(
    renderer: &mut super::super::GlesSceneRenderer,
    target: &PooledEffectTexture,
    target_plan: &oblivion_one::effects::GraphTexturePlan,
    framebuffer_origin: OutputFramebufferOrigin,
) -> RendererResult<()> {
    let mut context = renderer.effect_execution_context();
    capture_output_region_to_graph_texture_context(
        &mut context,
        target,
        target_plan,
        framebuffer_origin,
    )
}

#[cfg(test)]
pub(crate) fn capture_output_region_to_graph_texture_shader_copy(
    renderer: &mut super::super::GlesSceneRenderer,
    target: &PooledEffectTexture,
    target_plan: &oblivion_one::effects::GraphTexturePlan,
    framebuffer_origin: OutputFramebufferOrigin,
    output_texture: glow::Texture,
) -> RendererResult<()> {
    let mut context = renderer.effect_execution_context();
    capture_output_region_to_graph_texture_shader_copy_context(
        &mut context,
        target,
        target_plan,
        framebuffer_origin,
        output_texture,
    )
}

#[cfg(test)]
pub(crate) fn capture_output_rects_to_graph_texture_shader_copy(
    renderer: &mut super::super::GlesSceneRenderer,
    target: &PooledEffectTexture,
    target_plan: &oblivion_one::effects::GraphTexturePlan,
    framebuffer_origin: OutputFramebufferOrigin,
    output_texture: glow::Texture,
    target_rects: &[OutputRect],
) -> RendererResult<()> {
    let mut context = renderer.effect_execution_context();
    capture_output_rects_to_graph_texture_shader_copy_context(
        &mut context,
        target,
        target_plan,
        framebuffer_origin,
        output_texture,
        target_rects,
    )
}

#[cfg(test)]
mod tests;
