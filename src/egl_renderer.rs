use std::{
    collections::{HashMap, HashSet},
    error::Error,
    ffi::c_void,
    io, ptr,
    sync::Arc,
};

use glow::HasContext;
use khronos_egl as egl;
#[cfg(test)]
use oblivion_one::compositor::{DecorationRenderPrimitive, DecorationSceneSnapshot};
use oblivion_one::effects::{
    EffectGenerationPublisher, EffectManifest, EffectRect, EffectRegion, EffectRegistry,
    EffectRegistryGeneration, EffectWorkingSpace, FrameExecutionPlan, RegistryReloadError,
    TrustedEffectRegistry, compile_frame_execution_plan, plan_effect_execution_demand,
    reload_with_publisher,
};
use oblivion_one::{
    compositor::{
        self, DecorationRenderInstance, DesktopVisualState, RenderableSurface, VisualGroupId,
    },
    cursor_theme::CompositorCursorImage,
    window_lifecycle_animation::{
        LifecycleEffectKind, LifecycleFrameSample, LifecycleFrameSnapshot, LifecycleRenderEvidence,
        LifecycleRenderEvidenceEntry, LifecycleRenderFallbackEntry, LifecycleRenderFallbackReason,
        LifecycleRenderFallbacks, LifecycleSceneSample, LifecycleVisualGroup,
        LifecycleVisualSource, LifecycleVisualSourceKind, lamp_motion_channels,
        lifecycle_visual_transition_bounds, visible_lifecycle_samples,
    },
};

mod checkpoint;
mod damage;
pub(crate) mod dmabuf;
mod effects;
mod egl_support;
mod frame;
mod geometry;
mod lifecycle;
pub(crate) mod native_fence;
mod program;
mod resources;
mod scene;
mod scene_state;

pub(in crate::egl_renderer) use checkpoint::{
    CheckpointCausalState, EglCheckpointSceneCausalSnapshot,
};
#[cfg(test)]
pub(crate) use effects::replay_capture_region_layout;
pub(crate) use egl_support::{
    EglSwapBuffersWithDamage, choose_native_egl_config, choose_surfaceless_egl_config,
    create_gles_context, detect_partial_repaint_capabilities, egl_swap_buffers_with_damage,
    load_egl_image_target_texture_2d, load_swap_buffers_with_damage, native_visual_label,
};
#[cfg(test)]
pub(in crate::egl_renderer) use egl_support::{
    NativeEglConfigCandidate, format_gles3_context_error, gles_context_attributes,
    native_egl_config_candidate_matches, native_egl_config_candidate_matches_common,
    select_native_egl_config_candidate, select_native_egl_visual_format,
};
pub(in crate::egl_renderer) use egl_support::{native_egl_debug_enabled, query_egl_buffer_age};
#[cfg(test)]
pub(in crate::egl_renderer) use frame::{
    LegacySceneScissoredPhase, legacy_scene_scissored_phase_plan,
};
pub(in crate::egl_renderer) use frame::{
    resolve_scene_damage_authority, split_external_overlay_surfaces,
};
pub(in crate::egl_renderer) use geometry::{
    ensure_vertex_buffer_capacity, gl_scissor_to_output_rect, intersect_output_rect,
    output_rect_for_egl_clip,
};
pub(in crate::egl_renderer) use scene::{
    EglSceneCacheKey, EglSceneSurfaceSignature, egl_scene_surface_signatures,
    push_egl_decoration_instance, push_egl_surface_commands, push_output_background_command,
    rgba_to_pixel,
};

pub(crate) use frame::{
    EglFrameOutcome, EglOutputRenderTarget, EglSceneDrawRequest, EglSceneFrameCommit,
    FrameSkipReason, GlesSceneFrameStats,
};
pub(crate) use resources::EglImageGuard;

pub(crate) use damage::{
    BufferAge, EglPartialRepaintCapabilities, FullRepaintReason, OutputDamage, OutputRect,
    PartialRepaintPlanner, RepaintMode, render_target_buffer_age,
};
use damage::{EglOutputDamage, EglOutputDamageTracker, RenderExecution, RepaintPlan};
#[cfg(test)]
pub(crate) use damage::{PartialRepaintComplexityAction, PartialRepaintComplexityPolicy};
#[cfg(test)]
use effects::ShaderProgramCache;
use effects::{
    DamageTraceSnapshot, EffectExecutionContext, EffectExecutionTrace, EffectFailureReason,
    EffectGlResourceCache, EffectRepaintProvenanceSnapshot, EffectRuntime,
    EffectRuntimeCaptureSnapshot, FrameTraceSummary, RepaintPlanTraceSnapshot,
};
use effects::{EffectTextureFilter, EffectTextureFormat, EffectTextureKey, PooledEffectTexture};
use geometry::{
    EglDrawCommand, EglDrawLayer, EglLampDrawCommand, EglLampVertex, EglRect, EglTexturedVertex,
    EglUvRect, EglVisibilityDecision, MIN_VERTEX_BUFFER_BYTES, SurfaceConsumerPlan,
    SurfaceSampling, VERTEX_STRIDE, plan_surface_consumers, push_draw_command,
    push_draw_command_with_uv, surface_sampling_for_plan,
};
use lifecycle::{
    LifecycleCaptureSnapshot, LifecycleRenderContext, LifecycleRenderState,
    LifecycleResolvedVisualResource,
};
use program::create_texture_program;
use resources::{RendererResourceCaptureSnapshot, RendererResourceState, ResourceTextureView};
use scene_state::{SceneCaptureSnapshot, SceneRenderState, SceneTextureSources};

pub(crate) type RendererResult<T> = Result<T, Box<dyn Error>>;
pub(crate) type EglInstance = egl::DynamicInstance<egl::EGL1_5>;
type GlTexture = <glow::Context as HasContext>::Texture;
type GlProgram = <glow::Context as HasContext>::Program;
type GlBuffer = <glow::Context as HasContext>::Buffer;
type GlVertexArray = <glow::Context as HasContext>::VertexArray;
pub(crate) type GlEglImageTargetTexture2DOes = unsafe extern "system" fn(u32, *mut c_void);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputFramebufferOrigin {
    BottomLeft,
    TopLeftScanout,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EffectFramebufferTarget {
    pub(crate) framebuffer: Option<glow::Framebuffer>,
}

impl EffectFramebufferTarget {
    pub(crate) const fn new(framebuffer: Option<glow::Framebuffer>) -> Self {
        Self { framebuffer }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EffectExecutionTargets {
    pub(crate) baseline_read: EffectFramebufferTarget,
    pub(crate) composition_draw: EffectFramebufferTarget,
}

impl EffectExecutionTargets {
    pub(crate) const fn ordinary(output: EffectFramebufferTarget) -> Self {
        Self {
            baseline_read: output,
            composition_draw: output,
        }
    }

    pub(crate) fn uses_separate_targets(self) -> bool {
        self.baseline_read.framebuffer != self.composition_draw.framebuffer
    }
}

pub(crate) struct GlesSceneRenderer {
    cursor_image: std::sync::Arc<CompositorCursorImage>,
    gl: glow::Context,
    effect_runtime: EffectRuntime,
    scene_state: SceneRenderState,
    lifecycle: LifecycleRenderState,
    resources: RendererResourceState,
}

struct CaptureRendererState {
    scene: SceneCaptureSnapshot,
    effect_runtime: EffectRuntimeCaptureSnapshot,
    lifecycle: LifecycleCaptureSnapshot,
    resources: RendererResourceCaptureSnapshot,
}

impl CaptureRendererState {
    fn take(renderer: &GlesSceneRenderer) -> Self {
        Self {
            scene: SceneCaptureSnapshot::take(&renderer.scene_state),
            effect_runtime: EffectRuntimeCaptureSnapshot::take(&renderer.effect_runtime),
            lifecycle: renderer.lifecycle.capture_snapshot(),
            resources: renderer.resources.capture_snapshot(),
        }
    }

    fn restore(self, renderer: &mut GlesSceneRenderer) {
        self.scene.restore(&mut renderer.scene_state);
        self.effect_runtime.restore(&mut renderer.effect_runtime);
        renderer.lifecycle.restore_capture_snapshot(self.lifecycle);
        renderer.resources.restore_capture_snapshot(self.resources);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GlesRendererInfo {
    pub vendor: String,
    pub renderer: String,
    pub version: String,
}

impl GlesSceneRenderer {
    pub(crate) const fn lifecycle_animation_available(&self) -> bool {
        self.lifecycle.lamp_available()
    }

    /// Squash reuses the mandatory textured scene program. Renderer creation
    /// fails if that program cannot be compiled and linked.
    pub(crate) const fn squash_animation_available(&self) -> bool {
        true
    }

    pub(crate) fn invalidate_presented_damage_history(&mut self) {
        self.scene_state.repaint_planner.invalidate();
        self.scene_state.presented_scene_key = None;
        self.effect_runtime
            .effect_resources
            .invalidate_checkpoint_capture_contents();
    }

    pub(crate) fn new_current(
        egl: &EglInstance,
        width: u32,
        height: u32,
        egl_image_target_texture_2d: Option<GlEglImageTargetTexture2DOes>,
        partial_repaint_capabilities: EglPartialRepaintCapabilities,
        cursor_image: std::sync::Arc<CompositorCursorImage>,
    ) -> RendererResult<Self> {
        let gl = unsafe {
            glow::Context::from_loader_function(|name| {
                egl.get_proc_address(name)
                    .map(|symbol| symbol as *const c_void)
                    .unwrap_or(ptr::null())
            })
        };
        let program = create_texture_program(&gl)?;
        let effect_programs = EffectRuntime::create_programs(&gl)?;
        let lamp_program = program::create_lamp_program(&gl).ok();
        let scene_vertex_array = unsafe { gl.create_vertex_array().map_err(io::Error::other)? };
        let scene_vertex_buffer = unsafe { gl.create_buffer().map_err(io::Error::other)? };
        let overlay_vertex_array = unsafe { gl.create_vertex_array().map_err(io::Error::other)? };
        let overlay_vertex_buffer = unsafe { gl.create_buffer().map_err(io::Error::other)? };
        let squash_vertex_array = unsafe { gl.create_vertex_array().map_err(io::Error::other)? };
        let squash_vertex_buffer = unsafe { gl.create_buffer().map_err(io::Error::other)? };
        let lamp_vertex_array = unsafe { gl.create_vertex_array().map_err(io::Error::other)? };
        let lamp_vertex_buffer = unsafe { gl.create_buffer().map_err(io::Error::other)? };
        unsafe {
            for (vertex_array, vertex_buffer) in [
                (scene_vertex_array, scene_vertex_buffer),
                (overlay_vertex_array, overlay_vertex_buffer),
                (squash_vertex_array, squash_vertex_buffer),
            ] {
                gl.bind_vertex_array(Some(vertex_array));
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(vertex_buffer));
                gl.buffer_data_size(
                    glow::ARRAY_BUFFER,
                    MIN_VERTEX_BUFFER_BYTES as i32,
                    glow::DYNAMIC_DRAW,
                );
                gl.enable_vertex_attrib_array(0);
                gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, VERTEX_STRIDE, 0);
                gl.enable_vertex_attrib_array(1);
                gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, VERTEX_STRIDE, 8);
            }
            gl.bind_vertex_array(Some(lamp_vertex_array));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(lamp_vertex_buffer));
            gl.buffer_data_size(
                glow::ARRAY_BUFFER,
                MIN_VERTEX_BUFFER_BYTES as i32,
                glow::STATIC_DRAW,
            );
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, VERTEX_STRIDE, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, VERTEX_STRIDE, 8);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.bind_vertex_array(None);
            gl.use_program(Some(program));
            if let Some(location) = gl.get_uniform_location(program, "u_texture") {
                gl.uniform_1_i32(Some(&location), 0);
            }
            if let Some(location) = gl.get_uniform_location(program, "u_opacity") {
                gl.uniform_1_f32(Some(&location), 1.0);
            }
            effect_programs.initialize_uniforms(&gl);
            if let Some(lamp_program) = lamp_program {
                gl.use_program(Some(lamp_program));
                if let Some(location) = gl.get_uniform_location(lamp_program, "u_texture") {
                    gl.uniform_1_i32(Some(&location), 0);
                }
            }
            gl.use_program(Some(program));
            gl.enable(glow::BLEND);
            gl.blend_func_separate(
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
            );
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.viewport(0, 0, width as i32, height as i32);
        }

        let lifecycle = LifecycleRenderState::new(
            &gl,
            lamp_program,
            lamp_vertex_array,
            lamp_vertex_buffer,
            squash_vertex_array,
            squash_vertex_buffer,
            MIN_VERTEX_BUFFER_BYTES,
        );
        let effect_runtime = EffectRuntime::new(&gl, egl, effect_programs)?;
        let presentation_opacity_location =
            unsafe { gl.get_uniform_location(program, "u_opacity") };
        Ok(Self {
            gl,
            effect_runtime,
            scene_state: SceneRenderState {
                program,
                presentation_opacity_location,
                scene_vertex_array,
                scene_vertex_buffer,
                scene_vertex_buffer_capacity: MIN_VERTEX_BUFFER_BYTES,
                scene_geometry_dirty: true,
                overlay_vertex_array,
                overlay_vertex_buffer,
                overlay_vertex_buffer_capacity: MIN_VERTEX_BUFFER_BYTES,
                overlay_geometry_dirty: true,
                current_framebuffer_origin: OutputFramebufferOrigin::BottomLeft,
                current_size: (width, height),
                vertices: Vec::new(),
                commands: Vec::new(),
                cursor_vertices: Vec::new(),
                cursor_commands: Vec::new(),
                presentation_opacities: Vec::new(),
                cursor_presentation_opacities: Vec::new(),
                presentation_visual_group_opacities: HashMap::new(),
                presentation_visual_group_clips: HashMap::new(),
                presentation_visual_group_owners: HashMap::new(),
                scene_visibility_plan: Vec::new(),
                scene_cache_key: None,
                presented_scene_key: None,
                current_checkpoint_scene_causal_snapshot: None,
                damage_tracker: EglOutputDamageTracker::with_cursor_image(cursor_image.clone()),
                repaint_planner: PartialRepaintPlanner::new_configured(
                    (width, height),
                    partial_repaint_capabilities,
                ),
                active_output_framebuffer: None,
                active_output_texture: None,
                frame_stats: GlesSceneFrameStats::default(),
                capture_unattenuated_visual_group: None,
                capture_unclipped_presentation_owner: None,
            },
            lifecycle,
            cursor_image,
            resources: RendererResourceState::new(egl_image_target_texture_2d),
        })
    }

    #[cfg(test)]
    pub(in crate::egl_renderer) fn effect_execution_context(
        &mut self,
    ) -> EffectExecutionContext<'_> {
        let Self {
            gl,
            scene_state,
            effect_runtime,
            lifecycle,
            resources,
            ..
        } = self;
        let texture_sources = lifecycle.texture_sources(resources.texture_view());
        EffectExecutionContext::new(gl, scene_state, effect_runtime, texture_sources)
    }

    #[cfg(test)]
    pub(crate) fn draw_squash_overlay(
        &mut self,
        scissor: Option<OutputRect>,
    ) -> RendererResult<()> {
        let Self {
            gl,
            scene_state,
            effect_runtime,
            lifecycle,
            resources,
            ..
        } = self;
        let mut context =
            LifecycleRenderContext::new(gl, scene_state, effect_runtime, resources.texture_view());
        lifecycle.draw_squash_overlay(&mut context, scissor)
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(in crate::egl_renderer) fn execute_effect_graph_with_overlays_config(
        &mut self,
        graph: &oblivion_one::effects::CompiledFrameGraph,
        framebuffer_origin: OutputFramebufferOrigin,
        repaint_plan: &RepaintPlan,
        demand: &oblivion_one::effects::EffectExecutionDemand,
        selection: &effects::EffectExecutionSelection,
        debug_config: effects::EffectDebugConfig,
        scene_replay_work_mode_override: Option<effects::SceneReplayWorkMode>,
    ) -> RendererResult<effects::EffectExecutionStats> {
        let Self {
            gl,
            cursor_image,
            scene_state,
            lifecycle,
            effect_runtime,
            resources,
        } = self;
        frame::FramePipeline::new(
            gl,
            cursor_image,
            scene_state,
            lifecycle,
            effect_runtime,
            resources,
        )
        .execute_effect_graph_with_overlays_config(
            graph,
            framebuffer_origin,
            repaint_plan,
            demand,
            selection,
            debug_config,
            scene_replay_work_mode_override,
        )
    }

    pub(crate) const fn last_frame_stats(&self) -> GlesSceneFrameStats {
        self.scene_state.frame_stats
    }

    pub(crate) fn trace_render_fence_export_begin(&self) {
        self.effect_runtime.effect_trace.frame_boundary(
            "render_fence_export",
            "begin",
            FrameTraceSummary::default(),
        );
    }

    pub(crate) fn trace_render_fence_export_end(&self) {
        self.effect_runtime.effect_trace.frame_boundary(
            "render_fence_export",
            "end",
            FrameTraceSummary::default(),
        );
    }

    pub(crate) fn set_cursor_image(&mut self, cursor_image: Arc<CompositorCursorImage>) {
        self.cursor_image = cursor_image.clone();
        self.scene_state
            .damage_tracker
            .set_cursor_image(cursor_image);
        self.resources.mark_cursor_stale();
        self.scene_state.repaint_planner.invalidate();
    }

    /// Restore the complete state expected by ordinary scene drawing after an
    /// effect or other offscreen pass has changed GL state.
    #[cfg(test)]
    pub(crate) fn establish_ordinary_scene_state(&self) {
        self.establish_scene_state_for_framebuffer(self.scene_state.active_output_framebuffer);
    }

    #[cfg(test)]
    fn bind_active_output_framebuffer(&self) {
        unsafe {
            self.gl.bind_framebuffer(
                glow::FRAMEBUFFER,
                self.scene_state.active_output_framebuffer,
            );
        }
    }

    #[cfg(test)]
    fn presentation_clip_for_visual_group(
        &self,
        visual_group: Option<VisualGroupId>,
    ) -> Option<EglRect> {
        visual_group.and_then(|group| {
            self.scene_state
                .presentation_visual_group_clips
                .get(&group)
                .copied()
        })
    }

    #[cfg(test)]
    fn ensure_effect_quad(&mut self) -> RendererResult<(GlVertexArray, GlBuffer)> {
        self.effect_runtime.ensure_effect_quad(&self.gl)
    }

    #[cfg(test)]
    fn begin_effect_repaint(
        &mut self,
        plan: &RepaintPlan,
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> RendererResult<Vec<OutputRect>> {
        self.scene_state
            .begin_effect_repaint(&self.gl, plan, framebuffer_origin)
    }

    #[cfg(test)]
    fn establish_scene_state_for_framebuffer(&self, framebuffer: Option<glow::Framebuffer>) {
        self.scene_state
            .establish_scene_state_for_framebuffer(&self.gl, framebuffer);
    }

    pub(crate) fn renderer_info(&self) -> GlesRendererInfo {
        GlesRendererInfo {
            vendor: unsafe { self.gl.get_parameter_string(glow::VENDOR) },
            renderer: unsafe { self.gl.get_parameter_string(glow::RENDERER) },
            version: unsafe { self.gl.get_parameter_string(glow::VERSION) },
        }
    }

    #[allow(dead_code)] // Populated by the trusted registry reload boundary.
    pub(crate) fn set_effect_registry(&mut self, registry: EffectRegistry) {
        self.effect_runtime.set_registry(registry);
        self.invalidate_presented_damage_history();
    }

    /// Compile all custom modules at the active compositor GL boundary before
    /// making their complete validated generation visible to frame execution.
    #[allow(dead_code)]
    pub(crate) fn publish_effect_registry_generation(
        &mut self,
        generation: EffectRegistryGeneration,
    ) -> Result<(), RegistryReloadError> {
        self.effect_runtime
            .publish_registry_generation(&self.gl, generation)?;
        self.invalidate_presented_damage_history();
        Ok(())
    }

    /// Replace the built-in-only or already-compiled trusted program table for
    /// a material update. Material generations do not add shader modules, so
    /// the existing compiled shader cache remains valid.
    pub(crate) fn publish_material_effect_generation(
        &mut self,
        generation: &EffectRegistryGeneration,
    ) {
        self.effect_runtime.publish_material_generation(generation);
        self.invalidate_presented_damage_history();
    }

    #[allow(dead_code)]
    pub(crate) fn reload_trusted_effect_registry(
        &mut self,
        registry: &TrustedEffectRegistry,
        manifest: EffectManifest,
    ) -> Result<Arc<EffectRegistryGeneration>, RegistryReloadError> {
        reload_with_publisher(registry, manifest, self)
    }

    /// Render the current compositor scene into a private GLES target and
    /// return normalized top-left RGBA bytes. This operation deliberately
    /// never creates or touches a physical output buffer.
    pub(crate) fn capture_scene(
        &mut self,
        egl: &EglInstance,
        egl_display: egl::Display,
        mut request: EglSceneDrawRequest<'_>,
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> RendererResult<Vec<u8>> {
        let dimensions = crate::native_output::screen_capture::CaptureDimensions::checked(
            request.width,
            request.height,
        )?;
        let snapshot = CaptureRendererState::take(self);
        self.effect_runtime.capture_in_progress = true;

        request.current_damage = Some(OutputDamage::Full);
        request.client_cursor = None;
        request.visual_state.cursor = None;

        let result = (|| {
            let texture = unsafe { self.gl.create_texture().map_err(io::Error::other)? };
            let framebuffer = match unsafe { self.gl.create_framebuffer() } {
                Ok(framebuffer) => framebuffer,
                Err(error) => {
                    unsafe { self.gl.delete_texture(texture) };
                    return Err(io::Error::other(error).into());
                }
            };
            let setup_result = (|| {
                unsafe {
                    self.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                    self.gl.tex_parameter_i32(
                        glow::TEXTURE_2D,
                        glow::TEXTURE_MIN_FILTER,
                        glow::NEAREST as i32,
                    );
                    self.gl.tex_parameter_i32(
                        glow::TEXTURE_2D,
                        glow::TEXTURE_MAG_FILTER,
                        glow::NEAREST as i32,
                    );
                    self.gl.tex_parameter_i32(
                        glow::TEXTURE_2D,
                        glow::TEXTURE_WRAP_S,
                        glow::CLAMP_TO_EDGE as i32,
                    );
                    self.gl.tex_parameter_i32(
                        glow::TEXTURE_2D,
                        glow::TEXTURE_WRAP_T,
                        glow::CLAMP_TO_EDGE as i32,
                    );
                    self.gl.tex_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        glow::RGBA8 as i32,
                        dimensions.width as i32,
                        dimensions.height as i32,
                        0,
                        glow::RGBA,
                        glow::UNSIGNED_BYTE,
                        glow::PixelUnpackData::Slice(None),
                    );
                    self.gl
                        .bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
                    self.gl.framebuffer_texture_2d(
                        glow::FRAMEBUFFER,
                        glow::COLOR_ATTACHMENT0,
                        glow::TEXTURE_2D,
                        Some(texture),
                        0,
                    );
                }
                let status = unsafe { self.gl.check_framebuffer_status(glow::FRAMEBUFFER) };
                if status != glow::FRAMEBUFFER_COMPLETE {
                    return Err(io::Error::other(format!(
                        "screenshot framebuffer is incomplete: 0x{status:04x}"
                    ))
                    .into());
                }
                Ok::<(), Box<dyn Error>>(())
            })();

            let draw_result = setup_result.and_then(|()| {
                self.draw_scene_to_target(
                    egl,
                    egl_display,
                    EglOutputRenderTarget {
                        framebuffer,
                        sampleable_texture: None,
                        width: dimensions.width,
                        height: dimensions.height,
                        buffer_age: BufferAge::Value(0),
                        framebuffer_origin,
                    },
                    request,
                )
                .and_then(|outcome| match outcome {
                    EglFrameOutcome::Rendered { commit, stats, .. } => {
                        self.discard_rendered(commit);
                        if stats.dmabuf_import_failures > 0 {
                            return Err(io::Error::other(
                                "screenshot scene contains a GLES-incompatible dmabuf",
                            )
                            .into());
                        }
                        if stats.missing_required_decoration_resources > 0 {
                            return Err(io::Error::other(
                                "screenshot scene is missing a required decoration resource",
                            )
                            .into());
                        }
                        let mut readback = vec![0_u8; dimensions.payload_len];
                        unsafe {
                            self.gl
                                .bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
                            self.gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
                            self.gl.flush();
                            self.gl.read_pixels(
                                0,
                                0,
                                dimensions.width as i32,
                                dimensions.height as i32,
                                glow::RGBA,
                                glow::UNSIGNED_BYTE,
                                glow::PixelPackData::Slice(Some(&mut readback)),
                            );
                        }
                        crate::native_output::screen_capture::normalize_rgba_readback(
                            readback,
                            dimensions.width,
                            dimensions.height,
                            framebuffer_origin,
                        )
                        .map_err(Into::into)
                    }
                    EglFrameOutcome::Skipped { .. } => {
                        Err(io::Error::other("screenshot scene render produced no frame").into())
                    }
                    EglFrameOutcome::LifecycleFallback { .. } => Err(io::Error::other(
                        "screenshot scene render fell back during lifecycle rendering",
                    )
                    .into()),
                })
            });

            unsafe {
                self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                self.gl.delete_framebuffer(framebuffer);
                self.gl.delete_texture(texture);
            }
            draw_result
        })();

        snapshot.restore(self);
        result
    }

    pub(crate) fn draw_scene(
        &mut self,
        egl: &EglInstance,
        egl_display: egl::Display,
        egl_surface: egl::Surface,
        request: EglSceneDrawRequest<'_>,
    ) -> RendererResult<EglFrameOutcome> {
        let buffer_age = query_egl_buffer_age(
            egl,
            egl_display,
            egl_surface,
            self.scene_state.repaint_planner.capabilities().buffer_age,
        );
        self.draw_scene_with_buffer_age(
            egl,
            egl_display,
            request,
            buffer_age,
            OutputFramebufferOrigin::BottomLeft,
        )
    }

    #[allow(dead_code)] // Wired by the explicit Atomic runtime integration task.
    pub(crate) fn draw_scene_to_target(
        &mut self,
        egl: &EglInstance,
        egl_display: egl::Display,
        target: EglOutputRenderTarget,
        mut request: EglSceneDrawRequest<'_>,
    ) -> RendererResult<EglFrameOutcome> {
        request.width = target.width;
        request.height = target.height;
        let previous_output_framebuffer = self.scene_state.active_output_framebuffer;
        let previous_output_texture = self.scene_state.active_output_texture;
        self.scene_state.active_output_framebuffer = Some(target.framebuffer);
        self.scene_state.active_output_texture = target.sampleable_texture;
        unsafe {
            self.gl
                .bind_framebuffer(glow::FRAMEBUFFER, Some(target.framebuffer));
            self.gl
                .viewport(0, 0, target.width as i32, target.height as i32);
        }
        let result = self.draw_scene_with_buffer_age(
            egl,
            egl_display,
            request,
            target.buffer_age,
            target.framebuffer_origin,
        );
        unsafe {
            self.gl
                .bind_framebuffer(glow::FRAMEBUFFER, previous_output_framebuffer);
        }
        self.scene_state.active_output_framebuffer = previous_output_framebuffer;
        self.scene_state.active_output_texture = previous_output_texture;
        result
    }

    fn draw_scene_with_buffer_age(
        &mut self,
        egl: &EglInstance,
        egl_display: egl::Display,
        request: EglSceneDrawRequest<'_>,
        buffer_age: BufferAge,
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> RendererResult<EglFrameOutcome> {
        let Self {
            gl,
            cursor_image,
            scene_state,
            lifecycle,
            effect_runtime,
            resources,
        } = self;
        frame::FramePipeline::new(
            gl,
            cursor_image,
            scene_state,
            lifecycle,
            effect_runtime,
            resources,
        )
        .render(egl, egl_display, request, buffer_age, framebuffer_origin)
    }

    pub(crate) fn commit_presented(
        &mut self,
        frame: EglSceneFrameCommit,
        presented_transition_damage: OutputDamage,
    ) {
        self.scene_state
            .repaint_planner
            .commit_presented_transition(presented_transition_damage);
        self.scene_state
            .damage_tracker
            .commit_presented(frame.damage_state);
        self.scene_state.presented_scene_key = Some(frame.scene_key);
        self.scene_state.frame_stats.history_depth =
            self.scene_state.repaint_planner.history_depth();
    }

    #[cfg(test)]
    pub(crate) fn promote_checkpoint_cache_causal_state(
        &mut self,
        graph: &oblivion_one::effects::CompiledFrameGraph,
    ) {
        let Self {
            scene_state,
            effect_runtime,
            ..
        } = self;
        frame::promote_checkpoint_cache_causal_state(scene_state, effect_runtime, graph);
    }

    pub(crate) fn discard_rendered(&mut self, frame: EglSceneFrameCommit) {
        self.scene_state
            .repaint_planner
            .discard_rendered(&frame.repaint_plan);
        self.effect_runtime
            .effect_resources
            .invalidate_checkpoint_capture_contents();
    }

    pub(crate) fn frame_swap_failed(&mut self) {
        self.scene_state.repaint_planner.swap_failed();
        self.scene_state.frame_stats.history_depth = 0;
    }

    #[cfg(test)]
    fn draw_command_batch(
        &mut self,
        scene: bool,
        scissor: Option<OutputRect>,
    ) -> RendererResult<()> {
        self.effect_execution_context()
            .draw_command_batch(scene, scissor)
    }

    #[cfg(test)]
    #[cfg(test)]
    fn draw_command_batch_with_visibility(
        &mut self,
        scene: bool,
        scissor: Option<OutputRect>,
        plan_scene_visibility: bool,
    ) -> RendererResult<()> {
        self.effect_execution_context()
            .draw_command_batch_with_visibility(scene, scissor, plan_scene_visibility)
    }

    #[cfg(test)]
    #[cfg(test)]
    fn draw_command_batch_with_visibility_and_range(
        &mut self,
        scene: bool,
        scissor: Option<OutputRect>,
        plan_scene_visibility: bool,
        command_range: Option<(usize, usize)>,
        use_visibility_plan: bool,
    ) -> RendererResult<()> {
        self.effect_execution_context()
            .draw_command_batch_with_visibility_and_range(
                scene,
                scissor,
                plan_scene_visibility,
                command_range,
                use_visibility_plan,
            )
    }

    pub(crate) fn destroy(&mut self, egl: &EglInstance, egl_display: egl::Display) {
        self.resources.destroy(&self.gl, egl, egl_display);
        self.lifecycle
            .release_all_visual_resources(&mut self.effect_runtime);
        self.effect_runtime.destroy_persistent_resources(&self.gl);

        self.lifecycle.destroy_gl_resources(&self.gl);
        self.scene_state.destroy_gl_resources(&self.gl);
        self.effect_runtime.destroy_programs(&self.gl);
    }
}

fn effect_region_from_output_damage(
    damage: &OutputDamage,
    width: u32,
    height: u32,
) -> EffectRegion {
    match damage {
        OutputDamage::Empty => EffectRegion::empty(),
        OutputDamage::Full => EffectRegion::from_rect(
            EffectRect::new(0, 0, width, height).expect("renderer dimensions are valid"),
        ),
        OutputDamage::Rects(rects) => {
            let mut region = EffectRegion::empty();
            for rect in rects {
                if let Some(effect_rect) = EffectRect::new(rect.x, rect.y, rect.width, rect.height)
                {
                    region.push(effect_rect);
                }
            }
            region
        }
    }
}

fn repaint_plan_output_rects(plan: &RepaintPlan, width: u32, height: u32) -> Vec<OutputRect> {
    match plan.mode {
        RepaintMode::Skip => Vec::new(),
        RepaintMode::Full => vec![OutputRect::new(0, 0, width, height)],
        RepaintMode::Partial => match &plan.repair_damage {
            OutputDamage::Empty => Vec::new(),
            OutputDamage::Full => vec![OutputRect::new(0, 0, width, height)],
            OutputDamage::Rects(rects) => rects.clone(),
        },
    }
}

impl EffectGenerationPublisher for GlesSceneRenderer {
    fn publish_effect_generation(
        &mut self,
        generation: EffectRegistryGeneration,
    ) -> Result<(), RegistryReloadError> {
        self.publish_effect_registry_generation(generation)
    }
}

#[cfg(test)]
pub(super) mod tests;
