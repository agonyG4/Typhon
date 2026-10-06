use khronos_egl as egl;

use super::super::{
    EglInstance, RendererResult, lifecycle::LifecycleRenderState, resources::RendererResourceState,
    scene_state::SceneRenderState,
};
use super::{planning::FramePlanOutcome, types::EglSceneDrawRequest};
use oblivion_one::cursor_theme::CompositorCursorImage;

/// One call's coordinator over the persistent renderer domains.
/// It is constructed at the renderer façade and never escapes `render`.
pub(in crate::egl_renderer) struct FramePipeline<'a> {
    pub(super) gl: &'a glow::Context,
    pub(super) cursor_image: &'a CompositorCursorImage,
    pub(super) scene: &'a mut SceneRenderState,
    pub(super) lifecycle: &'a mut LifecycleRenderState,
    pub(super) effects: &'a mut super::super::effects::EffectRuntime,
    pub(super) resources: &'a mut RendererResourceState,
}

impl<'a> FramePipeline<'a> {
    pub(in crate::egl_renderer) fn new(
        gl: &'a glow::Context,
        cursor_image: &'a CompositorCursorImage,
        scene: &'a mut SceneRenderState,
        lifecycle: &'a mut LifecycleRenderState,
        effects: &'a mut super::super::effects::EffectRuntime,
        resources: &'a mut RendererResourceState,
    ) -> Self {
        Self {
            gl,
            cursor_image,
            scene,
            lifecycle,
            effects,
            resources,
        }
    }

    pub(in crate::egl_renderer) fn render(
        mut self,
        egl: &EglInstance,
        egl_display: egl::Display,
        mut request: EglSceneDrawRequest<'_>,
        buffer_age: super::super::damage::BufferAge,
        framebuffer_origin: super::super::OutputFramebufferOrigin,
    ) -> RendererResult<super::types::EglFrameOutcome> {
        self.effects.effect_resources.begin_checkpoint_frame();

        let begun = self.begin_frame(&request, framebuffer_origin)?;
        self.prepare_resources(egl, egl_display, &request, &begun)?;
        let prepared_scene = self.prepare_scene(&mut request, &begun, framebuffer_origin);
        let mut planned = match self.plan_frame(&request, prepared_scene, buffer_age)? {
            FramePlanOutcome::Skipped(outcome) => return Ok(outcome),
            FramePlanOutcome::Planned(planned) => planned,
        };

        self.plan_effect_demand(&request, &mut planned);
        let consumers = self.plan_consumers(&request, &mut planned);
        self.realize_consumers(egl, egl_display, &request, &planned, &consumers)?;
        self.execute_frame(&request, &planned, framebuffer_origin)?;
        Ok(self.settle_frame(planned))
    }
}
