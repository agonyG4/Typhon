mod execute;
mod pipeline;
mod planning;
mod telemetry;
mod types;

#[cfg(test)]
pub(super) use execute::promote_checkpoint_cache_causal_state;
pub(in crate::egl_renderer) use pipeline::FramePipeline;
pub(crate) use types::{
    EglFrameOutcome, EglOutputRenderTarget, EglSceneDrawRequest, EglSceneFrameCommit,
    FrameSkipReason, GlesSceneFrameStats,
};
