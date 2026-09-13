mod blur;
mod capture;
mod executor;
mod metrics;
mod resources;
mod shader_cache;

#[cfg(test)]
pub(crate) use blur::DUAL_KAWASE_VERTEX_SHADER;
#[cfg(test)]
pub(crate) use executor::{
    BLEND_STAGE_FRAGMENT_SHADER, MASK_STAGE_FRAGMENT_SHADER, NORMALIZE_FRAGMENT_SHADER,
};
#[cfg(test)]
pub(crate) const COPY_FRAGMENT_SHADER: &str = executor::COPY_FRAGMENT_SHADER;
#[cfg(test)]
pub(crate) const COMPOSITE_FRAGMENT_SHADER: &str = executor::COMPOSITE_FRAGMENT_SHADER;
pub(crate) use executor::{
    execute_effect_graph, execute_effect_graph_for_lifecycle, plan_effect_surface_consumers,
    select_effect_execution,
};
pub(crate) use metrics::{EffectFailureReason, EffectGraphMetrics, graph_metrics};
pub(crate) use resources::{
    EffectGlResourceCache, EffectTextureFilter, EffectTextureFormat, EffectTextureKey,
    PooledEffectTexture,
};
#[cfg(test)]
pub(crate) use shader_cache::generate_fragment_wrapper;
pub(crate) use shader_cache::{
    ShaderProgramCache, builtin_shader_program_count, shader_cache_capacity_for_custom_shaders,
};
