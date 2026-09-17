mod blur;
mod capture;
mod executor;
mod gpu_timing;
mod metrics;
mod resources;
mod shader_cache;
mod trace;

#[cfg(test)]
pub(crate) use blur::{DUAL_KAWASE_DOWNSAMPLE_LINEAR_SHADER, DUAL_KAWASE_VERTEX_SHADER};
#[cfg(test)]
pub(crate) use executor::{
    BLEND_STAGE_FRAGMENT_SHADER, EffectPassBlendMode, MASK_STAGE_FRAGMENT_SHADER,
    NORMALIZE_FRAGMENT_SHADER, establish_effect_pass_blend_state,
};
#[cfg(test)]
pub(crate) const COPY_FRAGMENT_SHADER: &str = executor::COPY_FRAGMENT_SHADER;
#[cfg(test)]
pub(crate) const COMPOSITE_FRAGMENT_SHADER: &str = executor::COMPOSITE_FRAGMENT_SHADER;
#[cfg(test)]
pub(crate) use executor::plan_effect_surface_consumers_with_debug_config;
#[cfg(test)]
pub(crate) use executor::{
    SceneCaptureMaterializationPolicy, execute_effect_graph_with_debug_config,
    execute_effect_graph_with_debug_config_and_materialization_policy,
};
pub(crate) use executor::{
    execute_effect_graph, execute_effect_graph_for_lifecycle, plan_effect_surface_consumers,
    select_effect_execution,
};
pub(crate) use gpu_timing::EffectGpuProfiler;
pub(crate) use metrics::{EffectFailureReason, EffectGraphMetrics, graph_metrics};
pub(crate) use resources::{
    EffectGlResourceCache, EffectTextureFilter, EffectTextureFormat, EffectTextureKey,
    PooledEffectTexture,
};
#[cfg(test)]
pub(crate) use shader_cache::ShaderProgramKey;
#[cfg(test)]
pub(crate) use shader_cache::generate_fragment_wrapper;
pub(crate) use shader_cache::{
    ShaderProgramCache, builtin_shader_program_count, shader_cache_capacity_for_custom_shaders,
};
pub(crate) use trace::{
    EffectDebugCaptureMode, EffectDebugConfig, EffectDebugKawaseMode, EffectExecutionTrace,
    FrameTraceSummary, PassTraceSummary, effect_debug_config,
};
#[cfg(test)]
pub(crate) use trace::{
    clear_test_events as clear_effect_trace_test_events,
    take_test_events as take_effect_trace_test_events,
};
