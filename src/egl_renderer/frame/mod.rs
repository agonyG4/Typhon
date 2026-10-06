mod execute;
mod pipeline;
mod planning;
mod telemetry;
mod types;

#[cfg(test)]
pub(super) use execute::promote_checkpoint_cache_causal_state;
#[cfg(test)]
pub(in crate::egl_renderer) use execute::{
    LegacySceneScissoredPhase, legacy_scene_scissored_phase_plan,
};
pub(in crate::egl_renderer) use pipeline::FramePipeline;
pub(in crate::egl_renderer) use planning::{
    resolve_scene_damage_authority, split_external_overlay_surfaces,
};
pub(crate) use types::{
    EglFrameOutcome, EglOutputRenderTarget, EglSceneDrawRequest, EglSceneFrameCommit,
    FrameSkipReason, GlesSceneFrameStats,
};
