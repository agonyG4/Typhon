use super::*;

mod frame_state;
mod geometry;
mod lamp;
mod squash;
mod visual_store;

use frame_state::{LifecycleCaptureFrameSnapshot, LifecycleFrameState};
use lamp::LampRenderState;
use squash::SquashRenderState;
use visual_store::LifecycleVisualStore;

pub(super) use frame_state::LifecycleCaptureSnapshot;
use lamp::LampUniformLocations;
pub(super) use visual_store::LifecycleResolvedVisualResource;

/// Renderer-side lifecycle execution owner. Semantic lifecycle identities and
/// animation state remain owned by the compositor and presentation engine.
pub(super) struct LifecycleRenderState {
    frame: LifecycleFrameState,
    visual_store: LifecycleVisualStore,
    lamp: LampRenderState,
    squash: SquashRenderState,
}

impl LifecycleRenderState {
    pub(super) fn new(
        gl: &glow::Context,
        lamp_program: Option<GlProgram>,
        lamp_vertex_array: GlVertexArray,
        lamp_vertex_buffer: GlBuffer,
        squash_vertex_array: GlVertexArray,
        squash_vertex_buffer: GlBuffer,
        vertex_buffer_capacity: usize,
    ) -> Self {
        let lamp_uniform_locations =
            lamp_program.map(|program| LampUniformLocations::query(gl, program));
        Self {
            frame: LifecycleFrameState::default(),
            visual_store: LifecycleVisualStore::default(),
            lamp: LampRenderState::new(
                lamp_program,
                lamp_uniform_locations,
                lamp_vertex_array,
                lamp_vertex_buffer,
                vertex_buffer_capacity,
            ),
            squash: SquashRenderState::new(
                squash_vertex_array,
                squash_vertex_buffer,
                vertex_buffer_capacity,
            ),
        }
    }

    pub(super) const fn lamp_available(&self) -> bool {
        self.lamp.program.is_some()
    }

    pub(super) fn capture_snapshot(&self) -> LifecycleCaptureSnapshot {
        LifecycleCaptureSnapshot {
            frame: LifecycleCaptureFrameSnapshot::take(&self.frame),
            visual_sources: self.visual_store.visual_sources.clone(),
        }
    }

    pub(super) fn restore_capture_snapshot(&mut self, snapshot: LifecycleCaptureSnapshot) {
        snapshot.frame.restore(&mut self.frame);
        self.visual_store.visual_sources = snapshot.visual_sources;
    }

    pub(super) fn texture_sources<'a>(
        &'a self,
        surfaces: &'a HashMap<u32, EglSurfaceResource>,
        frames: &'a HashMap<compositor::ServerFrameColor, EglImageResource>,
        decorations: &'a HashMap<DecorationResourceKey, EglImageResource>,
        cursor: Option<&'a EglImageResource>,
    ) -> SceneTextureSources<'a> {
        SceneTextureSources {
            surfaces,
            frames,
            decorations,
            lifecycle: &self.visual_store.resolved_visual_resources,
            cursor,
        }
    }

    pub(super) fn begin_frame(&mut self, snapshot: &LifecycleSceneSample) {
        self.frame.begin(snapshot);
        self.visual_store.begin_frame(snapshot);
    }

    pub(super) fn release_stale_visual_resources(&mut self, runtime: &mut EffectRuntime) {
        self.visual_store.release_stale(runtime);
    }

    pub(super) fn release_all_visual_resources(&mut self, runtime: &mut EffectRuntime) {
        self.visual_store.release_all(runtime);
    }

    pub(super) fn prepare_visual_sources(
        &mut self,
        context: &mut LifecycleRenderContext<'_>,
        plan: &RepaintPlan,
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> RendererResult<()> {
        self.visual_store.prepare_lifecycle_visual_sources(
            &mut self.frame,
            context,
            plan,
            framebuffer_origin,
        )
    }

    pub(super) fn rebuild_lamp_commands(
        &mut self,
        snapshot: &LifecycleSceneSample,
        surfaces: &[RenderableSurface],
        decorations: &[DecorationRenderInstance],
        output_scale: f64,
        output_size: (u32, u32),
        framebuffer_origin: OutputFramebufferOrigin,
    ) {
        self.lamp.rebuild_commands(
            snapshot,
            surfaces,
            decorations,
            output_scale,
            output_size,
            framebuffer_origin,
            &mut self.frame,
            &mut self.visual_store,
        );
    }

    pub(super) fn draw_lamp_overlay(
        &mut self,
        context: &mut LifecycleRenderContext<'_>,
        scissor: Option<OutputRect>,
    ) -> RendererResult<()> {
        self.lamp
            .draw_overlay(&mut self.frame, &self.visual_store, context, scissor)
    }

    pub(super) fn rebuild_squash_commands(
        &mut self,
        snapshot: &LifecycleSceneSample,
        surfaces: &[RenderableSurface],
        decorations: &[DecorationRenderInstance],
        output_scale: f64,
        output_size: (u32, u32),
        framebuffer_origin: OutputFramebufferOrigin,
    ) {
        self.squash.rebuild_commands(
            snapshot,
            surfaces,
            decorations,
            output_scale,
            output_size,
            framebuffer_origin,
            &mut self.frame,
        );
    }

    pub(super) fn draw_squash_overlay(
        &mut self,
        context: &mut LifecycleRenderContext<'_>,
        scissor: Option<OutputRect>,
    ) -> RendererResult<()> {
        self.squash
            .draw_overlay(&mut self.frame, &self.visual_store, context, scissor)
    }

    pub(super) fn extend_surface_consumers(
        &self,
        consumer_plan: &mut SurfaceConsumerPlan,
        repairs: &[OutputRect],
    ) {
        self.lamp.extend_surface_consumers(consumer_plan, repairs);
        self.visual_store
            .extend_surface_consumers(consumer_plan, repairs);
    }

    pub(super) fn record_missing_evidence_fallbacks(
        &mut self,
        output_scale: f64,
        output_size: (u32, u32),
    ) {
        self.frame
            .record_missing_evidence_fallbacks(output_scale, output_size);
    }

    pub(super) fn has_fallbacks(&self) -> bool {
        !self.frame.fallbacks.is_empty()
    }

    pub(super) fn evidence(&self) -> LifecycleRenderEvidence {
        self.frame.evidence.clone()
    }

    #[cfg(test)]
    pub(super) fn samples(&self) -> &[LifecycleFrameSample] {
        &self.frame.samples
    }

    #[cfg(test)]
    pub(super) fn lamp_commands(&self) -> &[EglLampDrawCommand] {
        &self.lamp.commands
    }

    #[cfg(test)]
    pub(super) fn lamp_uniforms_present_for_test(&self) -> [bool; 7] {
        let Some(uniforms) = self.lamp.uniform_locations else {
            return [false; 7];
        };
        [
            uniforms.canonical_visual_rect.is_some(),
            uniforms.source_visual_rect.is_some(),
            uniforms.sink_rect.is_some(),
            uniforms.progress.is_some(),
            uniforms.contraction_progress.is_some(),
            uniforms.translation_progress.is_some(),
            uniforms.retreat_progress.is_some(),
        ]
    }

    #[cfg(test)]
    pub(super) fn disable_lamp_for_test(&mut self) {
        self.lamp.program = None;
        self.lamp.uniform_locations = None;
    }

    #[cfg(test)]
    pub(super) fn invalidate_lamp_geometry_for_test(&mut self) {
        self.lamp.geometry_key = None;
    }

    pub(super) fn fallbacks(&self) -> LifecycleRenderFallbacks {
        self.frame.fallbacks.clone()
    }

    pub(super) fn damage_for_snapshot(
        &self,
        snapshot: &LifecycleSceneSample,
        output_size: (u32, u32),
        output_scale: f64,
    ) -> OutputDamage {
        LifecycleFrameState::damage_for_snapshot(snapshot, output_size, output_scale)
    }

    #[cfg(test)]
    pub(super) fn is_visual_source_ready(
        &self,
        payload_id: compositor::PresentationRetainedVisualPayloadId,
        runtime: &EffectRuntime,
    ) -> bool {
        self.visual_store.is_ready(payload_id, runtime)
    }

    #[cfg(test)]
    pub(super) fn resolved_visual_texture(
        &self,
        payload_id: compositor::PresentationRetainedVisualPayloadId,
    ) -> Option<&PooledEffectTexture> {
        self.visual_store
            .resolved_visual_resources
            .get(&payload_id)
            .map(|resource| &resource.texture)
    }

    #[cfg(test)]
    pub(super) fn resolved_visual_source_signature(
        &self,
        payload_id: compositor::PresentationRetainedVisualPayloadId,
    ) -> Option<u64> {
        self.visual_store
            .resolved_visual_resources
            .get(&payload_id)
            .map(|resource| resource.source_signature)
    }

    #[cfg(test)]
    pub(super) fn resolved_visual_resource_count(&self) -> usize {
        self.visual_store.resolved_visual_resources.len()
    }

    #[cfg(test)]
    pub(super) fn visual_sources(&self) -> impl Iterator<Item = &LifecycleVisualSource> {
        self.visual_store.visual_sources.values()
    }

    #[cfg(test)]
    pub(super) fn source_commands_for_payload(
        &self,
        payload_id: compositor::PresentationRetainedVisualPayloadId,
    ) -> Option<&[EglDrawCommand]> {
        self.visual_store
            .source_commands
            .get(&payload_id)
            .map(Vec::as_slice)
    }

    #[cfg(test)]
    pub(super) fn capture_visual_source_for_test(
        &mut self,
        context: &mut LifecycleRenderContext<'_>,
        source: &LifecycleVisualSource,
        sample: LifecycleFrameSample,
        target: PooledEffectTexture,
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> RendererResult<()> {
        self.visual_store
            .capture_visual_source(context, source, sample, target, framebuffer_origin)
            .map_err(|error| io::Error::other(format!("{error:?}")).into())
    }

    pub(super) fn draw_overlays(
        &mut self,
        context: &mut LifecycleRenderContext<'_>,
        rects: &[OutputRect],
        framebuffer_origin: OutputFramebufferOrigin,
        plan: &RepaintPlan,
    ) -> RendererResult<()> {
        self.prepare_visual_sources(context, plan, framebuffer_origin)?;
        for rect in rects {
            let y = match framebuffer_origin {
                OutputFramebufferOrigin::BottomLeft => context
                    .scene_state
                    .current_size
                    .1
                    .saturating_sub(rect.y.max(0) as u32 + rect.height)
                    as i32,
                OutputFramebufferOrigin::TopLeftScanout => rect.y,
            };
            unsafe {
                context.gl.enable(glow::SCISSOR_TEST);
                context
                    .gl
                    .scissor(rect.x, y, rect.width as i32, rect.height as i32);
            }
            self.draw_lamp_overlay(context, Some(*rect))?;
            self.draw_squash_overlay(context, Some(*rect))?;
        }
        unsafe {
            context.gl.disable(glow::SCISSOR_TEST);
        }
        Ok(())
    }

    pub(super) fn destroy_gl_resources(&mut self, gl: &glow::Context) {
        self.lamp.destroy_gl_resources(gl);
        self.squash.destroy_gl_resources(gl);
    }
}

/// Stack-only access to renderer domains used while producing lifecycle
/// pixels. The lifecycle owner and its retained caches are never carried here.
pub(super) struct LifecycleRenderContext<'a> {
    pub(super) gl: &'a glow::Context,
    pub(super) scene_state: &'a mut SceneRenderState,
    pub(super) effect_runtime: &'a mut EffectRuntime,
    pub(super) surface_resources: &'a HashMap<u32, EglSurfaceResource>,
    pub(super) frame_resources: &'a HashMap<compositor::ServerFrameColor, EglImageResource>,
    pub(super) decoration_resources: &'a HashMap<DecorationResourceKey, EglImageResource>,
    pub(super) cursor_resource: Option<&'a EglImageResource>,
}

impl<'a> LifecycleRenderContext<'a> {
    pub(super) fn new(
        gl: &'a glow::Context,
        scene_state: &'a mut SceneRenderState,
        effect_runtime: &'a mut EffectRuntime,
        surface_resources: &'a HashMap<u32, EglSurfaceResource>,
        frame_resources: &'a HashMap<compositor::ServerFrameColor, EglImageResource>,
        decoration_resources: &'a HashMap<DecorationResourceKey, EglImageResource>,
        cursor_resource: Option<&'a EglImageResource>,
    ) -> Self {
        Self {
            gl,
            scene_state,
            effect_runtime,
            surface_resources,
            frame_resources,
            decoration_resources,
            cursor_resource,
        }
    }

    pub(super) fn effect_execution_context<'b>(
        &'b mut self,
        lifecycle_resources: &'b HashMap<
            compositor::PresentationRetainedVisualPayloadId,
            LifecycleResolvedVisualResource,
        >,
    ) -> EffectExecutionContext<'b> {
        EffectExecutionContext::new(
            self.gl,
            self.scene_state,
            self.effect_runtime,
            SceneTextureSources {
                surfaces: self.surface_resources,
                frames: self.frame_resources,
                decorations: self.decoration_resources,
                lifecycle: lifecycle_resources,
                cursor: self.cursor_resource,
            },
        )
    }

    pub(super) fn texture_for_layer(
        &self,
        layer: EglDrawLayer,
        lifecycle_resources: &HashMap<
            compositor::PresentationRetainedVisualPayloadId,
            LifecycleResolvedVisualResource,
        >,
    ) -> Option<GlTexture> {
        SceneTextureSources {
            surfaces: self.surface_resources,
            frames: self.frame_resources,
            decorations: self.decoration_resources,
            lifecycle: lifecycle_resources,
            cursor: self.cursor_resource,
        }
        .texture_for_layer(layer, &self.effect_runtime.effect_resources)
    }

    pub(super) fn establish_ordinary_scene_state(&mut self) {
        let framebuffer = self.scene_state.active_output_framebuffer;
        self.scene_state
            .establish_scene_state_for_framebuffer(self.gl, framebuffer);
    }

    pub(super) fn establish_effect_composition_state(&mut self, target: EffectFramebufferTarget) {
        self.scene_state
            .establish_scene_state_for_framebuffer(self.gl, target.framebuffer);
    }
}
