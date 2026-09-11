mod blur;
mod capture;
mod executor;
mod metrics;
mod resources;
mod shader_cache;

#[cfg(test)]
pub(crate) use executor::{
    BLEND_STAGE_FRAGMENT_SHADER, MASK_STAGE_FRAGMENT_SHADER, NORMALIZE_FRAGMENT_SHADER,
};
pub(crate) use executor::{
    execute_effect_graph, plan_effect_surface_consumers, select_effect_execution,
};
pub(crate) use metrics::{EffectFailureReason, EffectGraphMetrics, graph_metrics};
pub(crate) use resources::EffectGlResourceCache;
pub(crate) use shader_cache::{
    ShaderProgramCache, builtin_shader_program_count, shader_cache_capacity_for_custom_shaders,
};
