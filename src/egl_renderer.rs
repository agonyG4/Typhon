use std::{
    collections::{HashMap, HashSet},
    error::Error,
    ffi::c_void,
    io, ptr,
    sync::Arc,
};

use glow::HasContext;
use khronos_egl as egl;
use oblivion_one::effects::{
    EffectGenerationPublisher, EffectManifest, EffectRect, EffectRegion, EffectRegistry,
    EffectRegistryGeneration, EffectWorkingSpace, FrameExecutionPlan, RegistryReloadError,
    TrustedEffectRegistry, compile_frame_execution_plan, plan_effect_execution_demand,
    reload_with_publisher,
};
use oblivion_one::{
    compositor::{
        self, DecorationRenderInstance, DecorationRenderPrimitive, DecorationSceneSnapshot,
        DesktopVisualState, RenderableSurface, SurfaceOpaqueRect, SurfaceOpaqueRegion,
        SurfaceResourceSyncState, VisualGroupId, clipped_decoration_text_geometry,
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

mod damage;
pub(crate) mod dmabuf;
mod effects;
mod geometry;
mod lifecycle;
pub(crate) mod native_fence;
mod program;
mod resources;
mod scene_state;

pub(crate) use resources::EglImageGuard;

pub(crate) use damage::{
    BufferAge, EglPartialRepaintCapabilities, FullRepaintReason, OutputDamage, OutputRect,
    PartialRepaintComplexityAction, PartialRepaintComplexityPolicy, PartialRepaintPlanner,
    RepaintMode, render_target_buffer_age,
};
use damage::{
    ClientCursorDamageState, EglOutputDamage, EglOutputDamageTracker, EglPresentedDamageState,
    RenderExecution, RepaintPlan, merge_effect_damage, resolve_effect_execution_for_repaint_plan,
    resolve_effect_execution_for_repaint_plan_with_diagnostics,
};
#[cfg(test)]
use effects::ShaderProgramCache;
use effects::{
    DamageTraceSnapshot, EffectExecutionContext, EffectExecutionTrace, EffectFailureReason,
    EffectGlResourceCache, EffectGraphMetrics, EffectRepaintProvenanceSnapshot, EffectRuntime,
    EffectRuntimeCaptureSnapshot, FrameTraceSummary, RepaintPlanTraceSnapshot, graph_metrics,
};
use effects::{EffectTextureFilter, EffectTextureFormat, EffectTextureKey, PooledEffectTexture};
use geometry::{
    EglDrawCommand, EglDrawLayer, EglLampDrawCommand, EglLampVertex, EglRect, EglTexturedVertex,
    EglUvRect, EglVisibilityDecision, MIN_VERTEX_BUFFER_BYTES, SurfaceConsumerPlan,
    SurfaceSampling, VERTEX_STRIDE, add_surface_consumers_for_command_range,
    plan_surface_consumers, push_draw_command, push_draw_command_with_uv,
    surface_sampling_for_plan,
};
use lifecycle::{
    LifecycleCaptureSnapshot, LifecycleRenderContext, LifecycleRenderState,
    LifecycleResolvedVisualResource,
};
use program::create_texture_program;
use resources::surface::SurfaceResourceInputs;
use resources::{
    RendererResourceCaptureSnapshot, RendererResourceState, ResourceTelemetry, ResourceTextureView,
};
use scene_state::{SceneCaptureSnapshot, SceneRenderState, SceneTextureSources};

pub(crate) type RendererResult<T> = Result<T, Box<dyn Error>>;
pub(crate) type EglInstance = egl::DynamicInstance<egl::EGL1_5>;
type GlTexture = <glow::Context as HasContext>::Texture;
type GlProgram = <glow::Context as HasContext>::Program;
type GlBuffer = <glow::Context as HasContext>::Buffer;
type GlVertexArray = <glow::Context as HasContext>::VertexArray;
pub(crate) type GlEglImageTargetTexture2DOes = unsafe extern "system" fn(u32, *mut c_void);
pub(crate) type EglSwapBuffersWithDamage = unsafe extern "system" fn(
    egl::EGLDisplay,
    egl::EGLSurface,
    *const egl::Int,
    egl::Int,
) -> egl::Boolean;

const EGL_BUFFER_AGE_EXT: egl::Int = 0x313d;
const MAX_LAMP_VERTICES: usize = 65_536;
const LAMP_TARGET_CELL_PIXELS: f32 = 32.0;
const LAMP_MAX_GRID_SUBDIVISIONS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeEglConfigCandidate {
    pub config_id: egl::Int,
    pub native_visual_id: u32,
    pub surface_type: egl::Int,
    pub renderable_type: egl::Int,
    pub red_size: egl::Int,
    pub green_size: egl::Int,
    pub blue_size: egl::Int,
    pub alpha_size: egl::Int,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GlesSceneFrameStats {
    pub scene_rebuilt: bool,
    pub surface_resource_candidates: usize,
    pub surface_resource_consumers: usize,
    pub surface_resource_deferred: usize,
    pub shm_upload_bytes: usize,
    pub dmabuf_imports: usize,
    pub dmabuf_reuses: usize,
    pub dmabuf_import_failures: usize,
    pub dmabuf_cache_entries: usize,
    pub dmabuf_cache_peak_entries: usize,
    pub dmabuf_cache_evictions: usize,
    pub dmabuf_current_resource_reuses: usize,
    pub dmabuf_cache_hits: usize,
    pub dmabuf_cache_misses: usize,
    pub dmabuf_cache_insertions: usize,
    pub dmabuf_cache_evictions_dead: usize,
    pub dmabuf_cache_evictions_surface_bound: usize,
    pub dmabuf_cache_evictions_surface_destroyed: usize,
    pub dmabuf_cache_max_entries_for_one_surface: usize,
    pub shm_full_resyncs: usize,
    pub repaint_mode: RepaintMode,
    pub partial_repaint_complexity_policy: PartialRepaintComplexityPolicy,
    pub partial_repaint_complexity_action: PartialRepaintComplexityAction,
    pub buffer_age: Option<u32>,
    pub current_damage_rects: usize,
    pub current_damage_pixels: u64,
    pub repair_damage_rects: usize,
    pub repair_damage_pixels: u64,
    pub scissor_passes: usize,
    pub planner_passes: usize,
    pub planner_commands_visited: usize,
    pub commands_drawable: usize,
    pub draw_command_replays: usize,
    pub commands_considered: usize,
    pub commands_executed: usize,
    pub missing_required_decoration_resources: usize,
    pub commands_rejected_outside_damage: usize,
    pub commands_rejected_outside_remaining: usize,
    pub commands_rejected_occluded: usize,
    pub opaque_rectangles_subtracted: usize,
    pub planner_early_terminations: usize,
    pub effect_fallbacks: usize,
    pub region_fragmentation_overflow_fallbacks: usize,
    pub scene_replay_work_overflow_fallbacks: usize,
    pub peak_region_piece_count: usize,
    pub texture_binds: usize,
    pub draw_calls: usize,
    pub scene_vbo_uploads: usize,
    pub scene_vbo_upload_bytes: usize,
    pub overlay_vbo_uploads: usize,
    pub overlay_vbo_upload_bytes: usize,
    pub history_depth: usize,
    pub fallback_reason: Option<FullRepaintReason>,
    pub partial_repaint_enabled: bool,
    pub contradictory_empty_damage: bool,
    pub orphan_decoration_count: u32,
    pub effect_instances_visible: usize,
    pub effect_instances_pruned: usize,
    pub effect_instances_executed: usize,
    pub effect_instances_failed: usize,
    pub render_graph_passes: usize,
    pub effect_passes_executed: usize,
    pub render_graph_peak_live_textures: usize,
    pub effect_graph_peak_live_bytes: u64,
    pub effect_capture_pixels: u64,
    pub effect_capture_pixels_executed: u64,
    pub effect_output_pixels: u64,
    pub blur_downsample_passes: usize,
    pub blur_upsample_passes: usize,
    pub effect_resource_allocations: usize,
    pub effect_resource_acquisitions: usize,
    pub effect_resource_reuses: usize,
    pub effect_resource_evictions: usize,
    pub effect_gpu_cache_bytes: u64,
    pub effect_gpu_cache_peak_bytes: u64,
    pub effect_gpu_budget_bytes: u64,
    pub effect_gpu_cached_keys: usize,
    pub effect_gpu_cached_textures: usize,
    pub effect_gpu_checked_out_textures: usize,
    pub effect_resource_allocations_total: usize,
    pub effect_resource_reuses_total: usize,
    pub effect_resource_evictions_total: usize,
    pub shader_cache_capacity: usize,
    pub shader_cache_entries: usize,
    pub shader_cache_peak_entries: usize,
    pub shader_cache_evictions_total: usize,
    pub effect_failure_reason: Option<EffectFailureReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrameSkipReason {
    NoLogicalDamage,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EglFrameOutcome {
    Skipped {
        reason: FrameSkipReason,
        stats: GlesSceneFrameStats,
    },
    Rendered {
        commit: EglSceneFrameCommit,
        stats: GlesSceneFrameStats,
        lifecycle_evidence: LifecycleRenderEvidence,
    },
    LifecycleFallback {
        stats: GlesSceneFrameStats,
        fallbacks: LifecycleRenderFallbacks,
    },
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)] // The explicit Atomic runtime consumes this after bootstrap reordering.
pub(crate) struct EglOutputRenderTarget {
    pub(crate) framebuffer: glow::Framebuffer,
    pub(crate) sampleable_texture: Option<glow::Texture>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) buffer_age: BufferAge,
    pub(crate) framebuffer_origin: OutputFramebufferOrigin,
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EglSceneFrameCommit {
    repaint_plan: RepaintPlan,
    damage_state: EglPresentedDamageState,
    scene_key: EglSceneCacheKey,
}

impl EglSceneFrameCommit {
    pub(crate) const fn repaint_plan(&self) -> &RepaintPlan {
        &self.repaint_plan
    }

    #[cfg(test)]
    pub(crate) fn empty_for_test() -> Self {
        Self {
            repaint_plan: RepaintPlan {
                render_damage: OutputDamage::Empty,
                repair_damage: OutputDamage::Empty,
                buffer_age: None,
                mode: RepaintMode::Skip,
                fallback_reason: None,
                ..RepaintPlan::default()
            },
            damage_state: EglPresentedDamageState::empty_for_test(),
            scene_key: EglSceneCacheKey {
                width: 1,
                height: 1,
                content_generation: 0,
                output_scale_key: 0,
                surface_signature_hash: 0,
                decoration_signature_hash: 0,
                popup_surface_signature_hash: 0,
                external_overlay_surface_signature_hash: 0,
                presentation_geometry_signature: 0,
                framebuffer_origin: OutputFramebufferOrigin::BottomLeft,
            },
        }
    }
}

pub struct EglSceneDrawRequest<'a> {
    pub width: u32,
    pub height: u32,
    pub surfaces: &'a [RenderableSurface],
    pub external_overlay_surface_ids: &'a [u32],
    pub popup_surface_ids: &'a [u32],
    pub content_generation: u64,
    pub(crate) frame_id: Option<u64>,
    pub(crate) render_generation: Option<u64>,
    pub(crate) scene_generation: u64,
    pub(crate) scene_signature: u64,
    pub visual_state: DesktopVisualState,
    pub output_scale: f64,
    pub decoration_instances: &'a [DecorationRenderInstance],
    pub effects: &'a compositor::ResolvedEffectScene,
    /// Compatibility-shaped cache input carrying the complete visual signature.
    pub presentation_visual_signature: u64,
    pub presentation_opacities:
        &'a [oblivion_one::presentation_animation::PresentationGroupOpacity],
    pub presentation_clips: &'a [oblivion_one::presentation_animation::PresentationGroupClip],
    pub presentation_owner_root_surface_ids: &'a [u32],
    pub client_cursor: Option<compositor::ClientCursorRenderState<'a>>,
    pub(crate) current_damage: Option<OutputDamage>,
    pub(crate) surface_resource_sync_states: Vec<SurfaceResourceSyncState>,
    pub lifecycle: &'a LifecycleSceneSample,
    pub lifecycle_surfaces: &'a [RenderableSurface],
    pub lifecycle_decorations: &'a [DecorationRenderInstance],
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

fn push_output_background_command(
    vertices: &mut Vec<EglTexturedVertex>,
    commands: &mut Vec<EglDrawCommand>,
    width: u32,
    height: u32,
    framebuffer_origin: OutputFramebufferOrigin,
) {
    push_draw_command(
        vertices,
        commands,
        EglDrawLayer::Solid(compositor::ServerFrameColor::OutputBackground),
        EglRect::new(0.0, 0.0, width as f32, height as f32),
        width,
        height,
        framebuffer_origin,
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LegacySceneScissoredPhase {
    BaseRepair(usize),
    PrepareLifecycleSources,
    RestoreRepairScissor(usize),
    Lamp(usize),
    Squash(usize),
    ExternalOverlays(usize),
}

#[derive(Debug, Clone, Copy)]
struct LegacySceneScissoredPhasePlan {
    repair_count: usize,
    index: usize,
}

impl Iterator for LegacySceneScissoredPhasePlan {
    type Item = LegacySceneScissoredPhase;

    fn next(&mut self) -> Option<Self::Item> {
        let phase = if self.index < self.repair_count {
            LegacySceneScissoredPhase::BaseRepair(self.index)
        } else if self.index == self.repair_count {
            LegacySceneScissoredPhase::PrepareLifecycleSources
        } else {
            let offset = self.index - self.repair_count - 1;
            let repair = offset / 4;
            if repair >= self.repair_count {
                return None;
            }
            match offset % 4 {
                0 => LegacySceneScissoredPhase::RestoreRepairScissor(repair),
                1 => LegacySceneScissoredPhase::Lamp(repair),
                2 => LegacySceneScissoredPhase::Squash(repair),
                _ => LegacySceneScissoredPhase::ExternalOverlays(repair),
            }
        };
        self.index = self.index.saturating_add(1);
        Some(phase)
    }
}

fn legacy_scene_scissored_phase_plan(repair_count: usize) -> LegacySceneScissoredPhasePlan {
    LegacySceneScissoredPhasePlan {
        repair_count,
        index: 0,
    }
}

#[derive(Debug)]
pub(crate) struct ReplayCaptureRegionLayout {
    pub(crate) materialization_rects: usize,
    pub(crate) execution_region: EffectRegion,
    pub(crate) execution_regions: usize,
    pub(crate) disjoint_overflowed: bool,
}

pub(crate) fn replay_capture_region_layout(
    output_rects: &[OutputRect],
) -> ReplayCaptureRegionLayout {
    let mut requested = EffectRegion::empty();
    for output_rect in output_rects {
        if let Some(rect) = EffectRect::new(
            output_rect.x,
            output_rect.y,
            output_rect.width,
            output_rect.height,
        ) {
            requested.push(rect);
        }
    }
    let disjoint = requested.disjoint_bounded();
    let execution_region = if disjoint.overflowed {
        requested
            .bounding_rect()
            .map(EffectRegion::from_rect)
            .unwrap_or_else(EffectRegion::empty)
    } else {
        disjoint.region
    };
    ReplayCaptureRegionLayout {
        materialization_rects: output_rects.len(),
        execution_regions: execution_region.rects().len(),
        disjoint_overflowed: disjoint.overflowed,
        execution_region,
    }
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

    fn execute_effect_graph_with_overlays(
        &mut self,
        graph: &oblivion_one::effects::CompiledFrameGraph,
        framebuffer_origin: OutputFramebufferOrigin,
        repaint_plan: &RepaintPlan,
        demand: &oblivion_one::effects::EffectExecutionDemand,
        selection: &effects::EffectExecutionSelection,
    ) -> RendererResult<effects::EffectExecutionStats> {
        self.execute_effect_graph_with_overlays_config(
            graph,
            framebuffer_origin,
            repaint_plan,
            demand,
            selection,
            *effects::effect_debug_config(),
            None,
        )
    }

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
        let mut prepared = {
            let mut context = self.effect_execution_context();
            effects::prepare_effect_graph_execution(
                &mut context,
                graph,
                framebuffer_origin,
                repaint_plan,
                demand,
                selection,
                debug_config,
                scene_replay_work_mode_override,
            )?
        };
        let promotes_checkpoint_cache = prepared.promotes_checkpoint_cache();
        let composition_target = prepared.composition_target();
        let mut result = {
            let mut context = self.effect_execution_context();
            effects::execute_prepared_effect_graph_core(&mut context, &mut prepared)
        };
        if result.is_ok() {
            let overlay_result = (|| {
                if self.effect_runtime.effect_trace.enabled() {
                    self.effect_runtime.effect_trace.overlay_boundary("begin");
                }
                self.draw_lifecycle_overlays(
                    prepared.overlay_rects(),
                    framebuffer_origin,
                    repaint_plan,
                )?;
                self.draw_effect_overlays(prepared.overlay_rects(), framebuffer_origin)?;
                if self.effect_runtime.effect_trace.enabled() {
                    self.effect_runtime.effect_trace.overlay_boundary("end");
                }
                self.establish_effect_composition_state(composition_target);
                Ok(())
            })();
            if let Err(error) = overlay_result {
                result = Err(error);
            }
        }
        let result = {
            let mut context = self.effect_execution_context();
            effects::finish_prepared_effect_graph_execution(&mut context, prepared, result)
        };
        if result.is_ok() && promotes_checkpoint_cache {
            self.promote_checkpoint_cache_causal_state(graph);
        }
        result
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
    pub(crate) fn establish_ordinary_scene_state(&self) {
        self.establish_scene_state_for_framebuffer(self.scene_state.active_output_framebuffer);
    }

    pub(crate) fn establish_effect_composition_state(&self, target: EffectFramebufferTarget) {
        self.establish_scene_state_for_framebuffer(target.framebuffer);
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

    fn establish_scene_state_for_framebuffer(&self, framebuffer: Option<glow::Framebuffer>) {
        self.scene_state
            .establish_scene_state_for_framebuffer(&self.gl, framebuffer);
    }

    fn presentation_opacity_for_root(
        presentation_opacities: &[oblivion_one::presentation_animation::PresentationGroupOpacity],
        owner_root: u32,
    ) -> f32 {
        presentation_opacities
            .iter()
            .find(|entry| entry.root_surface_id == owner_root)
            .map_or(1.0, |entry| entry.opacity.get() as f32)
            .clamp(0.0, 1.0)
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
        self.effect_runtime
            .effect_resources
            .begin_checkpoint_frame();
        let EglSceneDrawRequest {
            width,
            height,
            surfaces,
            external_overlay_surface_ids,
            content_generation,
            frame_id,
            render_generation,
            scene_generation,
            scene_signature,
            visual_state,
            output_scale,
            decoration_instances,
            effects,
            presentation_visual_signature,
            presentation_opacities,
            presentation_clips,
            presentation_owner_root_surface_ids,
            popup_surface_ids,
            client_cursor,
            current_damage,
            surface_resource_sync_states,
            lifecycle,
            lifecycle_surfaces,
            lifecycle_decorations,
        } = request;
        self.effect_runtime.effect_trace = self.effect_runtime.effect_trace.with_frame_context(
            frame_id,
            render_generation,
            Some(scene_generation),
            Some(scene_signature),
        );
        if !self.effect_runtime.capture_in_progress {
            self.effect_runtime
                .effect_gpu_profiler
                .collect(&self.gl, &self.effect_runtime.effect_trace);
        }
        self.effect_runtime.effect_trace.frame_boundary(
            "effect_scene_resolve",
            "begin",
            FrameTraceSummary::default(),
        );
        let width = width.max(1);
        let height = height.max(1);
        let input_damage_trace = if self.effect_runtime.effect_trace.enabled() {
            Some(DamageTraceSnapshot::from_optional(
                current_damage.as_ref(),
                width,
                height,
            ))
        } else {
            None
        };
        self.scene_state.current_framebuffer_origin = framebuffer_origin;
        self.lifecycle.begin_frame(lifecycle);
        let output_scale_key = compositor::output_scale_key(output_scale);
        let mut scaled_visual_state =
            compositor::scale_desktop_visual_state(visual_state, output_scale);
        if client_cursor.is_some() {
            scaled_visual_state.cursor = None;
        }
        self.scene_state.frame_stats = GlesSceneFrameStats::default();
        let effect_time = self.effect_runtime.effect_clock_elapsed_seconds();
        self.effect_runtime.effect_delta_seconds = if self.effect_runtime.effect_time_seconds == 0.0
        {
            0.0
        } else {
            (effect_time - self.effect_runtime.effect_time_seconds).clamp(0.0, 0.25)
        };
        self.effect_runtime.effect_time_seconds = effect_time;
        self.effect_runtime.effect_output_scale = output_scale.max(0.0) as f32;
        self.ensure_output_size(width, height)?;
        self.lifecycle
            .release_stale_visual_resources(&mut self.effect_runtime);
        self.scene_state.frame_stats.effect_instances_visible = effects
            .instances
            .iter()
            .filter(|instance| !instance.region.is_empty())
            .count();
        self.effect_runtime.effect_trace.frame_boundary(
            "effect_scene_resolve",
            "end",
            FrameTraceSummary {
                scene_generation: Some(scene_generation),
                scene_signature: Some(scene_signature),
                visible_effect_count: Some(self.scene_state.frame_stats.effect_instances_visible),
                ..FrameTraceSummary::default()
            },
        );
        self.resources.ensure_frame_resources(&self.gl)?;
        self.resources.ensure_decoration_resources(
            &self.gl,
            egl,
            egl_display,
            decoration_instances
                .iter()
                .chain(lifecycle_decorations.iter()),
        )?;
        if scaled_visual_state.cursor.is_some() {
            self.resources.ensure_cursor_resource(
                &self.gl,
                egl,
                egl_display,
                &self.cursor_image,
            )?;
        }
        {
            let mut telemetry = ResourceTelemetry::new(&mut self.scene_state.frame_stats);
            self.resources.reconcile_surface_resource_lifetimes(
                &self.gl,
                egl,
                egl_display,
                surfaces,
                lifecycle_surfaces,
                client_cursor.map(|cursor| cursor.surface),
                &mut telemetry,
            )?;
        }
        // The software client cursor remains eager: it is a small, separately
        // owned overlay path and is not part of ordinary scene realization.
        if let Some(cursor) = client_cursor.map(|cursor| cursor.surface) {
            let mut cursor_consumers = SurfaceConsumerPlan::default();
            cursor_consumers.add_surface(cursor.surface_id);
            cursor_consumers.finish();
            let mut telemetry = ResourceTelemetry::new(&mut self.scene_state.frame_stats);
            self.resources.realize_surface_resources_for_consumers(
                &self.gl,
                egl,
                egl_display,
                SurfaceResourceInputs {
                    canonical: surfaces,
                    lifecycle: lifecycle_surfaces,
                    client_cursor: Some(cursor),
                },
                &cursor_consumers,
                &surface_resource_sync_states,
                &mut telemetry,
            )?;
        }

        let (base_surfaces, overlay_surfaces) =
            split_external_overlay_surfaces(surfaces, external_overlay_surface_ids);
        let presentation_owner_roots_by_surface = surfaces
            .iter()
            .map(|surface| surface.surface_id)
            .zip(presentation_owner_root_surface_ids.iter().copied())
            .collect::<HashMap<_, _>>();
        let scene_surfaces = if external_overlay_surface_ids.is_empty() {
            surfaces
        } else {
            base_surfaces.as_slice()
        };
        let surface_signatures = egl_scene_surface_signatures(surfaces);
        let candidate_scene_key = EglSceneCacheKey::new_with_decorations_and_external_overlay_ids(
            width,
            height,
            content_generation,
            output_scale_key,
            &surface_signatures,
            presentation_visual_signature,
            external_overlay_surface_ids,
            decoration_instances,
            popup_surface_ids,
            framebuffer_origin,
        );
        let scene_changed = self.scene_state.presented_scene_key != Some(candidate_scene_key);
        let commands_changed = !self.scene_cache_is_current(
            width,
            height,
            content_generation,
            output_scale_key,
            &surface_signatures,
            external_overlay_surface_ids,
            decoration_instances,
            popup_surface_ids,
            presentation_visual_signature,
            framebuffer_origin,
        );
        let client_cursor_damage = client_cursor.map(|cursor| {
            ClientCursorDamageState::new(
                compositor::scale_logical_coordinate(
                    cursor.logical_x.saturating_add(cursor.surface.x),
                    output_scale,
                ),
                compositor::scale_logical_coordinate(
                    cursor.logical_y.saturating_add(cursor.surface.y),
                    output_scale,
                ),
                compositor::scale_logical_extent(cursor.surface.width, output_scale),
                compositor::scale_logical_extent(cursor.surface.height, output_scale),
                cursor.surface.generation,
                width,
                height,
            )
        });
        let damage_authority_available = current_damage.is_some();
        let output_damage = self.scene_state.damage_tracker.damage_for_frame(
            width,
            height,
            scene_changed,
            current_damage,
            scaled_visual_state,
            client_cursor_damage,
        );
        let output_damage = output_damage.union(
            self.lifecycle
                .damage_for_snapshot(lifecycle, (width, height), output_scale),
            width,
            height,
        );
        let (output_damage, contradictory_empty_damage) = resolve_scene_damage_authority(
            scene_changed,
            damage_authority_available,
            output_damage,
        );
        let scene_damage_trace = if self.effect_runtime.effect_trace.enabled() {
            Some(DamageTraceSnapshot::from_damage(
                &output_damage,
                width,
                height,
            ))
        } else {
            None
        };
        self.scene_state.frame_stats.contradictory_empty_damage = contradictory_empty_damage;
        let damage_state = EglOutputDamageTracker::candidate_state(
            width,
            height,
            scaled_visual_state,
            client_cursor_damage,
            &self.cursor_image,
        );

        if commands_changed {
            self.scene_state.frame_stats.scene_rebuilt = true;
            self.rebuild_scene_commands(
                width,
                height,
                scene_surfaces,
                decoration_instances,
                popup_surface_ids,
                content_generation,
                output_scale,
                output_scale_key,
                &surface_signatures,
                external_overlay_surface_ids,
                presentation_visual_signature,
                presentation_opacities,
                presentation_clips,
                &presentation_owner_roots_by_surface,
                framebuffer_origin,
            );
        }
        self.rebuild_overlay_commands(
            width,
            height,
            scaled_visual_state,
            &overlay_surfaces,
            client_cursor,
            output_scale,
            framebuffer_origin,
        );
        self.scene_state.current_checkpoint_scene_causal_snapshot =
            Some(EglCheckpointSceneCausalSnapshot::new(
                (width, height),
                &self.scene_state.commands,
                &self.scene_state.vertices,
                &surface_signatures,
                &self.scene_state.presentation_opacities,
                &self.scene_state.presentation_visual_group_owners,
            ));
        self.lifecycle.rebuild_lamp_commands(
            lifecycle,
            lifecycle_surfaces,
            lifecycle_decorations,
            output_scale,
            self.scene_state.current_size,
            framebuffer_origin,
        );
        self.lifecycle.rebuild_squash_commands(
            lifecycle,
            lifecycle_surfaces,
            lifecycle_decorations,
            output_scale,
            self.scene_state.current_size,
            framebuffer_origin,
        );
        let effect_source_damage = effect_region_from_output_damage(&output_damage, width, height);
        let output_bounds = EffectRect::new(0, 0, width, height)
            .expect("non-zero renderer dimensions must form valid effect bounds");
        self.effect_runtime.effect_trace.frame_boundary(
            "effect_graph_compile",
            "begin",
            self.effect_trace_summary(effects, None, None, None),
        );
        let execution_plan = if self.effect_runtime.failed_effect_generation
            == Some(self.effect_runtime.effect_registry_generation)
            && self.scene_state.frame_stats.effect_instances_visible != 0
        {
            self.scene_state.frame_stats.effect_fallbacks = self
                .scene_state
                .frame_stats
                .effect_fallbacks
                .saturating_add(1);
            self.scene_state.frame_stats.effect_instances_failed =
                self.scene_state.frame_stats.effect_instances_visible;
            self.scene_state.frame_stats.effect_failure_reason =
                Some(EffectFailureReason::GraphCompile);
            FrameExecutionPlan::LegacyScene
        } else {
            match compile_frame_execution_plan(
                effects,
                &effect_source_damage,
                output_bounds,
                &self.effect_runtime.effect_registry,
            ) {
                Ok(FrameExecutionPlan::LegacyScene) => FrameExecutionPlan::LegacyScene,
                Ok(FrameExecutionPlan::EffectGraph(graph)) => {
                    self.record_effect_graph_metrics(graph_metrics(&graph));
                    FrameExecutionPlan::EffectGraph(graph)
                }
                Err(_) => {
                    self.scene_state.frame_stats.effect_fallbacks = self
                        .scene_state
                        .frame_stats
                        .effect_fallbacks
                        .saturating_add(1);
                    self.scene_state.frame_stats.effect_instances_failed =
                        self.scene_state.frame_stats.effect_instances_visible;
                    self.scene_state.frame_stats.effect_failure_reason =
                        Some(EffectFailureReason::GraphCompile);
                    self.effect_runtime.failed_effect_generation =
                        Some(self.effect_runtime.effect_registry_generation);
                    FrameExecutionPlan::LegacyScene
                }
            }
        };
        if matches!(&execution_plan, FrameExecutionPlan::LegacyScene) {
            self.effect_runtime
                .effect_resources
                .clear_checkpoint_capture_cache();
        }
        let compiled_graph = match &execution_plan {
            FrameExecutionPlan::EffectGraph(graph) => Some(graph),
            FrameExecutionPlan::LegacyScene => None,
        };
        self.effect_runtime.effect_trace.frame_boundary(
            "effect_graph_compile",
            "end",
            self.effect_trace_summary(effects, None, compiled_graph, None),
        );
        let output_damage = match &execution_plan {
            FrameExecutionPlan::LegacyScene => output_damage,
            FrameExecutionPlan::EffectGraph(graph) => {
                merge_effect_damage(output_damage, &graph.final_damage, width, height)
            }
        };
        let merged_damage_trace = if self.effect_runtime.effect_trace.enabled() {
            Some(DamageTraceSnapshot::from_damage(
                &output_damage,
                width,
                height,
            ))
        } else {
            None
        };
        let (mut plan, damage_complexity_shadow_trace) =
            if self.effect_runtime.effect_trace.enabled() {
                let (plan, shadow) = self
                    .scene_state
                    .repaint_planner
                    .plan_with_damage_complexity_shadow(output_damage, buffer_age);
                (plan, Some(shadow))
            } else {
                (
                    self.scene_state
                        .repaint_planner
                        .plan(output_damage, buffer_age),
                    None,
                )
            };
        if plan.mode == RepaintMode::Skip {
            self.scene_state.frame_stats.surface_resource_candidates = surfaces.len();
            self.scene_state.frame_stats.surface_resource_deferred = surfaces.len();
            self.record_effect_resource_metrics();
            self.record_repaint_stats(&plan);
            return Ok(EglFrameOutcome::Skipped {
                reason: FrameSkipReason::NoLogicalDamage,
                stats: self.scene_state.frame_stats,
            });
        }
        let initial_repaint_trace = if self.effect_runtime.effect_trace.enabled() {
            Some(RepaintPlanTraceSnapshot::from_plan(&plan, width, height))
        } else {
            None
        };
        let demand_trace_seed = if self.effect_runtime.effect_trace.enabled() {
            compiled_graph.map(|graph| oblivion_one::effects::EffectDemandPlanStats {
                repair_rect_count: plan.repair_damage.rect_count(),
                dependency_edge_count: graph.instances.iter().fold(0, |count, instance| {
                    count.saturating_add(instance.dependencies.len())
                }),
                dependency_propagations: 0,
                max_instance_region_rect_count: 0,
                conservative_full: plan.mode == RepaintMode::Full,
                ..oblivion_one::effects::EffectDemandPlanStats::default()
            })
        } else {
            None
        };
        let mut demand_trace_begin_summary =
            self.effect_trace_summary(effects, Some(&plan), compiled_graph, None);
        demand_trace_begin_summary.demand_plan = demand_trace_seed;
        self.effect_runtime.effect_trace.frame_boundary(
            "effect_demand_plan",
            "begin",
            demand_trace_begin_summary,
        );
        let effect_execution_demand = match &execution_plan {
            FrameExecutionPlan::LegacyScene => None,
            FrameExecutionPlan::EffectGraph(graph) => {
                Some(if self.effect_runtime.effect_trace.enabled() {
                    let (demand, snapshot) =
                        resolve_effect_execution_for_repaint_plan_with_diagnostics(
                            &self.scene_state.repaint_planner,
                            graph,
                            &mut plan,
                            width,
                            height,
                        );
                    self.effect_runtime
                        .effect_trace
                        .effect_execution_resolution(|| snapshot);
                    demand
                } else {
                    resolve_effect_execution_for_repaint_plan(
                        &self.scene_state.repaint_planner,
                        graph,
                        &mut plan,
                        width,
                        height,
                    )
                })
            }
        };
        let selected_effect_count = effect_execution_demand
            .as_ref()
            .map(|demand| demand.instances.len());
        let demand_trace_stats = if self.effect_runtime.effect_trace.enabled() {
            effect_execution_demand
                .as_ref()
                .map(|demand| demand.plan_stats())
        } else {
            None
        };
        let mut demand_trace_end_summary =
            self.effect_trace_summary(effects, Some(&plan), compiled_graph, selected_effect_count);
        demand_trace_end_summary.demand_plan = demand_trace_stats;
        self.effect_runtime.effect_trace.frame_boundary(
            "effect_demand_plan",
            "end",
            demand_trace_end_summary,
        );
        self.effect_runtime
            .effect_trace
            .effect_repaint_provenance(|| {
                EffectRepaintProvenanceSnapshot::new(
                    input_damage_trace.expect("enabled effect trace must capture input damage"),
                    scene_damage_trace.expect("enabled effect trace must capture scene damage"),
                    merged_damage_trace.expect("enabled effect trace must capture merged damage"),
                    initial_repaint_trace
                        .expect("enabled effect trace must capture initial repaint"),
                    RepaintPlanTraceSnapshot::from_plan(&plan, width, height),
                    damage_complexity_shadow_trace
                        .expect("enabled effect trace must capture damage complexity shadow"),
                )
            });
        if let Some(demand) = &effect_execution_demand {
            self.scene_state.frame_stats.effect_instances_pruned = self
                .scene_state
                .frame_stats
                .effect_instances_visible
                .saturating_sub(demand.instances.len());
        }
        let repair_rects = repaint_plan_output_rects(&plan, width, height);
        let mut consumer_plan = plan_surface_consumers(&self.scene_state.commands, &repair_rects);
        self.lifecycle
            .extend_surface_consumers(&mut consumer_plan, &repair_rects);
        add_surface_consumers_for_command_range(
            &mut consumer_plan,
            &self.scene_state.cursor_commands,
            0,
            self.scene_state.cursor_commands.len(),
            &repair_rects,
        );
        let effect_selection = match (&execution_plan, &effect_execution_demand) {
            (FrameExecutionPlan::EffectGraph(graph), Some(demand)) => {
                let selection = effects::select_effect_execution(graph, demand);
                consumer_plan.extend(&effects::plan_effect_surface_consumers(
                    graph,
                    demand,
                    &selection,
                    &self.scene_state.commands,
                    &repair_rects,
                    (width, height),
                ));
                Some(selection)
            }
            _ => None,
        };
        consumer_plan.finish();
        self.scene_state.frame_stats.surface_resource_candidates = surfaces.len();
        self.scene_state.frame_stats.surface_resource_consumers = consumer_plan
            .surface_ids()
            .iter()
            .filter(|surface_id| {
                surfaces
                    .iter()
                    .any(|surface| surface.surface_id == **surface_id)
            })
            .count();
        self.scene_state.frame_stats.surface_resource_deferred = self
            .scene_state
            .frame_stats
            .surface_resource_candidates
            .saturating_sub(self.scene_state.frame_stats.surface_resource_consumers);
        let mut telemetry = ResourceTelemetry::new(&mut self.scene_state.frame_stats);
        self.resources.realize_surface_resources_for_consumers(
            &self.gl,
            egl,
            egl_display,
            SurfaceResourceInputs {
                canonical: surfaces,
                lifecycle: lifecycle_surfaces,
                client_cursor: client_cursor.map(|cursor| cursor.surface),
            },
            &consumer_plan,
            &surface_resource_sync_states,
            &mut telemetry,
        )?;
        self.effect_runtime.effect_trace.frame_boundary(
            "renderer_draw_complete",
            "begin",
            self.effect_trace_summary(effects, Some(&plan), compiled_graph, selected_effect_count),
        );
        let draw_result = match &execution_plan {
            FrameExecutionPlan::LegacyScene => self.draw_textured_layers(&plan, framebuffer_origin),
            FrameExecutionPlan::EffectGraph(graph) => {
                let demand = effect_execution_demand
                    .as_ref()
                    .expect("effect graph execution must have an execution demand");
                let selection = effect_selection
                    .as_ref()
                    .expect("effect graph execution must have an execution selection");
                match self.execute_effect_graph_with_overlays(
                    graph,
                    framebuffer_origin,
                    &plan,
                    demand,
                    selection,
                ) {
                    Ok(execution_stats) => {
                        self.scene_state.frame_stats.effect_instances_executed =
                            execution_stats.instances;
                        self.scene_state.frame_stats.effect_passes_executed =
                            execution_stats.passes;
                        self.scene_state
                            .frame_stats
                            .scene_replay_work_overflow_fallbacks =
                            execution_stats.scene_replay_work_overflow_fallbacks;
                        self.scene_state.frame_stats.blur_downsample_passes =
                            execution_stats.blur_downsamples;
                        self.scene_state.frame_stats.blur_upsample_passes =
                            execution_stats.blur_upsamples;
                        self.scene_state.frame_stats.effect_capture_pixels_executed =
                            execution_stats.capture_execution_pixels;
                        self.scene_state.frame_stats.effect_resource_acquisitions =
                            execution_stats.resource_acquisitions;
                        Ok(())
                    }
                    Err(error) => {
                        self.effect_runtime
                            .effect_resources
                            .invalidate_checkpoint_capture_contents();
                        self.scene_state.frame_stats.effect_fallbacks = self
                            .scene_state
                            .frame_stats
                            .effect_fallbacks
                            .saturating_add(1);
                        self.scene_state.frame_stats.effect_instances_failed =
                            self.scene_state.frame_stats.effect_instances_visible;
                        self.scene_state.frame_stats.effect_failure_reason =
                            Some(EffectFailureReason::from_error(error.as_ref()));
                        if self.scene_state.frame_stats.effect_failure_reason
                            == Some(EffectFailureReason::ShaderUnavailable)
                        {
                            self.effect_runtime.failed_effect_generation =
                                Some(self.effect_runtime.effect_registry_generation);
                        }
                        self.draw_textured_layers(&plan, framebuffer_origin)
                    }
                }
            }
        };
        if let Err(error) = draw_result {
            self.effect_runtime.effect_trace.frame_boundary(
                "renderer_draw_complete",
                "end",
                self.effect_trace_summary(
                    effects,
                    Some(&plan),
                    compiled_graph,
                    selected_effect_count,
                ),
            );
            self.scene_state.repaint_planner.invalidate();
            return Err(error);
        }
        self.effect_runtime.effect_trace.frame_boundary(
            "renderer_draw_complete",
            "end",
            self.effect_trace_summary(effects, Some(&plan), compiled_graph, selected_effect_count),
        );
        self.lifecycle.record_missing_evidence_fallbacks(
            f64::from(self.effect_runtime.effect_output_scale),
            self.scene_state.current_size,
        );
        if self.lifecycle.has_fallbacks() {
            self.scene_state.repaint_planner.invalidate();
            return Ok(EglFrameOutcome::LifecycleFallback {
                stats: self.scene_state.frame_stats,
                fallbacks: self.lifecycle.fallbacks(),
            });
        }
        self.record_effect_resource_metrics();
        self.record_repaint_stats(&plan);
        Ok(EglFrameOutcome::Rendered {
            commit: EglSceneFrameCommit {
                repaint_plan: plan,
                damage_state,
                scene_key: candidate_scene_key,
            },
            stats: self.scene_state.frame_stats,
            lifecycle_evidence: self.lifecycle.evidence(),
        })
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

    pub(crate) fn promote_checkpoint_cache_causal_state(
        &mut self,
        graph: &oblivion_one::effects::CompiledFrameGraph,
    ) {
        let frame_serial = self
            .effect_runtime
            .effect_resources
            .checkpoint_frame_serial();
        if let Some(state) = self.checkpoint_causal_candidate_state(Some(graph)) {
            self.effect_runtime
                .effect_resources
                .promote_checkpoint_causal_state(frame_serial, state);
        } else {
            self.effect_runtime
                .effect_resources
                .invalidate_checkpoint_causal_state();
        }
    }

    fn checkpoint_causal_candidate_state(
        &self,
        graph: Option<&oblivion_one::effects::CompiledFrameGraph>,
    ) -> Option<CheckpointCausalState> {
        self.scene_state
            .current_checkpoint_scene_causal_snapshot
            .clone()
            .map(|scene| CheckpointCausalState::new(scene, graph, &self.scene_state.commands))
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

    fn record_repaint_stats(&mut self, plan: &RepaintPlan) {
        let (width, height) = self.scene_state.current_size;
        self.scene_state.frame_stats.repaint_mode = plan.mode;
        self.scene_state
            .frame_stats
            .partial_repaint_complexity_policy = plan.complexity_policy;
        self.scene_state
            .frame_stats
            .partial_repaint_complexity_action = plan.complexity_action;
        self.scene_state.frame_stats.buffer_age = plan.buffer_age;
        self.scene_state.frame_stats.current_damage_rects = plan.render_damage.rect_count();
        self.scene_state.frame_stats.current_damage_pixels =
            plan.render_damage.pixels(width, height).unwrap_or(u64::MAX);
        self.scene_state.frame_stats.repair_damage_rects = plan.repair_damage.rect_count();
        self.scene_state.frame_stats.repair_damage_pixels =
            plan.repair_damage.pixels(width, height).unwrap_or(u64::MAX);
        self.scene_state.frame_stats.fallback_reason = plan.fallback_reason;
        self.scene_state.frame_stats.partial_repaint_enabled =
            self.scene_state.repaint_planner.partial_enabled();
        self.scene_state.frame_stats.history_depth =
            self.scene_state.repaint_planner.history_depth();
    }

    fn record_effect_graph_metrics(&mut self, metrics: EffectGraphMetrics) {
        self.scene_state.frame_stats.effect_instances_visible = metrics.instances;
        self.scene_state.frame_stats.render_graph_passes = metrics.passes;
        self.scene_state.frame_stats.render_graph_peak_live_textures = metrics.peak_live_textures;
        self.scene_state.frame_stats.effect_graph_peak_live_bytes = metrics.peak_live_bytes;
        self.scene_state.frame_stats.effect_capture_pixels = metrics.capture_pixels;
        self.scene_state.frame_stats.effect_output_pixels = metrics.output_pixels;
    }

    fn effect_trace_summary(
        &self,
        effects: &compositor::ResolvedEffectScene,
        repaint_plan: Option<&RepaintPlan>,
        graph: Option<&oblivion_one::effects::CompiledFrameGraph>,
        selected_effect_count: Option<usize>,
    ) -> FrameTraceSummary {
        FrameTraceSummary {
            scene_generation: Some(effects.generation),
            repaint_mode: repaint_plan.map(|plan| plan.mode.as_str()),
            render_damage_signature: repaint_plan
                .map(|plan| plan.render_damage.identity_signature()),
            repair_damage_signature: repaint_plan
                .map(|plan| plan.repair_damage.identity_signature()),
            visible_effect_count: Some(self.scene_state.frame_stats.effect_instances_visible),
            selected_effect_count,
            graph_pass_count: graph.map(|graph| graph.stats.passes),
            graph_texture_count: graph.map(|graph| graph.stats.textures),
            peak_live_intermediate_count: graph.map(|graph| graph.stats.peak_live_intermediates),
            ..FrameTraceSummary::default()
        }
    }

    fn record_effect_resource_metrics(&mut self) {
        let metrics = self.effect_runtime.effect_resources.metrics();
        self.scene_state.frame_stats.effect_resource_allocations = metrics.allocation_count;
        self.scene_state.frame_stats.effect_resource_reuses = metrics.reuse_count;
        self.scene_state.frame_stats.effect_resource_evictions = metrics.eviction_count;
        self.scene_state
            .frame_stats
            .effect_resource_allocations_total = metrics.allocation_count;
        self.scene_state.frame_stats.effect_resource_reuses_total = metrics.reuse_count;
        self.scene_state.frame_stats.effect_resource_evictions_total = metrics.eviction_count;
        self.scene_state.frame_stats.effect_gpu_cache_bytes = metrics.current_bytes;
        self.scene_state.frame_stats.effect_gpu_cache_peak_bytes = metrics.peak_bytes;
        self.scene_state.frame_stats.effect_gpu_budget_bytes = metrics.budget_bytes;
        self.scene_state.frame_stats.effect_gpu_cached_keys = metrics.cached_key_count;
        self.scene_state.frame_stats.effect_gpu_cached_textures = metrics.cached_texture_count;
        self.scene_state.frame_stats.effect_gpu_checked_out_textures =
            metrics.checked_out_texture_count;
        let shader_metrics = self.effect_runtime.effect_shaders.metrics();
        self.scene_state.frame_stats.shader_cache_capacity = shader_metrics.capacity;
        self.scene_state.frame_stats.shader_cache_entries = shader_metrics.resident_entries;
        self.scene_state.frame_stats.shader_cache_peak_entries = shader_metrics.peak_entries;
        self.scene_state.frame_stats.shader_cache_evictions_total = shader_metrics.eviction_count;
    }

    fn ensure_output_size(&mut self, width: u32, height: u32) -> RendererResult<()> {
        if self.scene_state.current_size == (width, height) {
            return Ok(());
        }

        self.lifecycle
            .release_all_visual_resources(&mut self.effect_runtime);
        self.scene_state.current_size = (width, height);
        self.scene_state.repaint_planner.resize((width, height));
        self.scene_state.scene_cache_key = None;
        self.effect_runtime
            .effect_resources
            .cleanup_size_history(&self.gl);
        unsafe {
            self.gl.viewport(0, 0, width as i32, height as i32);
        }
        Ok(())
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "scene-cache validation compares each render-state component explicitly"
    )]
    fn scene_cache_is_current(
        &self,
        width: u32,
        height: u32,
        content_generation: u64,
        output_scale_key: u32,
        surface_signatures: &[EglSceneSurfaceSignature],
        external_overlay_surface_ids: &[u32],
        decoration_instances: &[DecorationRenderInstance],
        popup_surface_ids: &[u32],
        presentation_geometry_signature: u64,
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> bool {
        self.scene_state.scene_cache_key.is_some_and(|key| {
            key.is_current_with_decorations_and_external_overlay_ids(
                width,
                height,
                content_generation,
                output_scale_key,
                surface_signatures,
                external_overlay_surface_ids,
                decoration_instances,
                popup_surface_ids,
                presentation_geometry_signature,
                framebuffer_origin,
            )
        })
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "hot EGL command rebuild path passes borrowed frame state directly to avoid transient config allocation"
    )]
    fn rebuild_scene_commands(
        &mut self,
        width: u32,
        height: u32,
        surfaces: &[RenderableSurface],
        decoration_instances: &[DecorationRenderInstance],
        popup_surface_ids: &[u32],
        content_generation: u64,
        output_scale: f64,
        output_scale_key: u32,
        surface_signatures: &[EglSceneSurfaceSignature],
        external_overlay_surface_ids: &[u32],
        presentation_geometry_signature: u64,
        presentation_opacities: &[oblivion_one::presentation_animation::PresentationGroupOpacity],
        presentation_clips: &[oblivion_one::presentation_animation::PresentationGroupClip],
        presentation_owner_roots_by_surface: &HashMap<u32, u32>,
        framebuffer_origin: OutputFramebufferOrigin,
    ) {
        self.scene_state.frame_stats.orphan_decoration_count =
            compositor::WindowVisualGroup::orphan_decoration_count(surfaces, decoration_instances);
        self.scene_state.vertices.clear();
        self.scene_state.commands.clear();
        self.scene_state.presentation_opacities.clear();
        self.scene_state.presentation_visual_group_opacities.clear();
        self.scene_state.presentation_visual_group_clips.clear();
        self.scene_state.presentation_visual_group_owners.clear();
        self.scene_state.scene_geometry_dirty = true;
        self.scene_state.vertices.reserve((1 + surfaces.len()) * 6);
        self.scene_state.commands.reserve(1 + surfaces.len());

        push_output_background_command(
            &mut self.scene_state.vertices,
            &mut self.scene_state.commands,
            width,
            height,
            framebuffer_origin,
        );

        let render_assignments =
            compositor::surface_render_space_assignments(surfaces, output_scale);
        for (group_index, group) in compositor::WindowVisualGroup::stack_order_with_popups(
            surfaces,
            decoration_instances,
            popup_surface_ids,
        )
        .into_iter()
        .enumerate()
        {
            let command_start = self.scene_state.commands.len();
            let visual_group = VisualGroupId::new(
                u32::try_from(group_index)
                    .unwrap_or(u32::MAX.saturating_sub(1))
                    .saturating_add(1),
            );
            for &surface_index in group.surface_indices() {
                let Some((surface, render_assignment)) = surfaces
                    .get(surface_index)
                    .zip(render_assignments.get(surface_index).cloned())
                else {
                    continue;
                };
                push_egl_surface_commands(
                    &mut self.scene_state.vertices,
                    &mut self.scene_state.commands,
                    width,
                    height,
                    surface,
                    render_assignment,
                    framebuffer_origin,
                    visual_group,
                );
            }
            if let Some(decoration_index) = group.decoration_index()
                && let Some(instance) = decoration_instances.get(decoration_index)
            {
                push_egl_decoration_instance(
                    &mut self.scene_state.vertices,
                    &mut self.scene_state.commands,
                    width,
                    height,
                    instance,
                    output_scale,
                    framebuffer_origin,
                    visual_group,
                );
            }
            let group_root = group.root_surface_id();
            let owner_root = presentation_owner_roots_by_surface
                .get(&group_root)
                .copied()
                .unwrap_or(group_root);
            let opacity = Self::presentation_opacity_for_root(presentation_opacities, owner_root);
            if let Some(visual_group) = visual_group {
                self.scene_state
                    .presentation_visual_group_owners
                    .insert(visual_group, owner_root);
                self.scene_state
                    .presentation_visual_group_opacities
                    .insert(visual_group, opacity);
                if let Some(clip) = presentation_clips
                    .iter()
                    .find(|clip| clip.root_surface_id == owner_root)
                    .and_then(|clip| clip.presented_clip)
                {
                    let scale = output_scale.max(0.01);
                    self.scene_state.presentation_visual_group_clips.insert(
                        visual_group,
                        EglRect::new(
                            (clip.x() * scale) as f32,
                            (clip.y() * scale) as f32,
                            (clip.width() * scale) as f32,
                            (clip.height() * scale) as f32,
                        ),
                    );
                }
            }
            for _ in command_start..self.scene_state.commands.len() {
                self.scene_state.presentation_opacities.push(opacity);
            }
            let presentation_clip = visual_group
                .and_then(|visual_group| {
                    self.scene_state
                        .presentation_visual_group_clips
                        .get(&visual_group)
                })
                .copied();
            for command in &mut self.scene_state.commands[command_start..] {
                command.presentation_clip = presentation_clip;
            }
            if opacity < 1.0 {
                for command in &mut self.scene_state.commands[command_start..] {
                    command.opaque_regions.clear();
                }
            }
        }

        self.scene_state.scene_cache_key = Some(
            EglSceneCacheKey::new_with_decorations_and_external_overlay_ids(
                width,
                height,
                content_generation,
                output_scale_key,
                surface_signatures,
                presentation_geometry_signature,
                external_overlay_surface_ids,
                decoration_instances,
                popup_surface_ids,
                framebuffer_origin,
            ),
        );
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "overlay command emission keeps target, overlay, cursor, scale, and origin state explicit"
    )]
    fn rebuild_overlay_commands(
        &mut self,
        width: u32,
        height: u32,
        visual_state: DesktopVisualState,
        overlay_surfaces: &[RenderableSurface],
        client_cursor: Option<compositor::ClientCursorRenderState<'_>>,
        output_scale: f64,
        framebuffer_origin: OutputFramebufferOrigin,
    ) {
        self.scene_state.cursor_vertices.clear();
        self.scene_state.cursor_commands.clear();
        self.scene_state.cursor_presentation_opacities.clear();
        self.scene_state.overlay_geometry_dirty = true;

        let render_assignments =
            compositor::surface_render_space_assignments(overlay_surfaces, output_scale);
        for (surface, render_assignment) in overlay_surfaces.iter().zip(render_assignments) {
            let command_start = self.scene_state.cursor_commands.len();
            push_egl_surface_commands(
                &mut self.scene_state.cursor_vertices,
                &mut self.scene_state.cursor_commands,
                width,
                height,
                surface,
                render_assignment,
                framebuffer_origin,
                None,
            );
            self.scene_state
                .cursor_presentation_opacities
                .extend(std::iter::repeat_n(
                    1.0,
                    self.scene_state
                        .cursor_commands
                        .len()
                        .saturating_sub(command_start),
                ));
        }

        if let Some((cursor_x, cursor_y)) = visual_state.cursor
            && let Some(cursor_size) = self.resources.texture_view().cursor_size()
        {
            let (top_left_x, top_left_y) = self.cursor_image.top_left(cursor_x, cursor_y);
            push_draw_command(
                &mut self.scene_state.cursor_vertices,
                &mut self.scene_state.cursor_commands,
                EglDrawLayer::Cursor,
                EglRect::new(
                    top_left_x as f32,
                    top_left_y as f32,
                    cursor_size.0 as f32,
                    cursor_size.1 as f32,
                ),
                width,
                height,
                framebuffer_origin,
            );
            self.scene_state.cursor_presentation_opacities.push(1.0);
        }

        if let Some(cursor) = client_cursor {
            let visual_target = compositor::SurfaceTargetRect::new(
                compositor::scale_logical_coordinate(
                    cursor.logical_x.saturating_add(cursor.surface.x),
                    output_scale,
                ),
                compositor::scale_logical_coordinate(
                    cursor.logical_y.saturating_add(cursor.surface.y),
                    output_scale,
                ),
                compositor::scale_logical_extent(cursor.surface.width, output_scale),
                compositor::scale_logical_extent(cursor.surface.height, output_scale),
            );
            let render_plan = compositor::surface_render_plan(cursor.surface, visual_target);
            let uv = EglUvRect::from_surface_uv_quad(render_plan.content_uv);
            push_draw_command_with_uv(
                &mut self.scene_state.cursor_vertices,
                &mut self.scene_state.cursor_commands,
                EglDrawLayer::Surface(cursor.surface.surface_id),
                EglRect::new(
                    render_plan.content_target.x() as f32,
                    render_plan.content_target.y() as f32,
                    render_plan.content_target.width() as f32,
                    render_plan.content_target.height() as f32,
                ),
                uv,
                surface_sampling_for_plan(
                    cursor.surface.buffer_size().width,
                    cursor.surface.buffer_size().height,
                    render_plan.content_target.x(),
                    render_plan.content_target.y(),
                    render_plan.content_target.width(),
                    render_plan.content_target.height(),
                    uv,
                ),
                width,
                height,
                framebuffer_origin,
            );
            self.scene_state.cursor_presentation_opacities.push(1.0);
        }
    }

    fn draw_textured_layers(
        &mut self,
        plan: &RepaintPlan,
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> RendererResult<()> {
        self.establish_ordinary_scene_state();
        unsafe { self.gl.clear_color(0.0, 0.0, 0.0, 1.0) };

        let execution = plan
            .render_execution(
                self.scene_state.current_size.0,
                self.scene_state.current_size.1,
                framebuffer_origin,
            )
            .ok_or_else(|| io::Error::other("repaint execution conversion failed"))?;
        match execution {
            RenderExecution::Full => {
                unsafe {
                    self.gl.disable(glow::SCISSOR_TEST);
                    self.gl.clear(glow::COLOR_BUFFER_BIT);
                }
                self.draw_command_batch(true, None)?;
                self.prepare_lifecycle_visual_sources(plan, framebuffer_origin)?;
                self.draw_lamp_overlay(None)?;
                self.draw_squash_overlay(None)?;
                self.draw_command_batch(false, None)?;
            }
            RenderExecution::Scissored {
                scissors,
                disable_scissor_after,
            } => {
                unsafe {
                    self.gl.enable(glow::SCISSOR_TEST);
                }
                let mut draw_result = Ok(());
                for phase in legacy_scene_scissored_phase_plan(scissors.len()) {
                    if draw_result.is_err() {
                        break;
                    }
                    match phase {
                        LegacySceneScissoredPhase::BaseRepair(index) => {
                            let [x, y, width, height] = scissors[index];
                            unsafe {
                                self.gl.scissor(x, y, width, height);
                                self.gl.clear(glow::COLOR_BUFFER_BIT);
                            }
                            let output_rect = gl_scissor_to_output_rect(
                                [x, y, width, height],
                                self.scene_state.current_size.1,
                                framebuffer_origin,
                            );
                            draw_result = self.draw_command_batch(true, output_rect);
                        }
                        LegacySceneScissoredPhase::PrepareLifecycleSources => {
                            draw_result =
                                self.prepare_lifecycle_visual_sources(plan, framebuffer_origin);
                        }
                        LegacySceneScissoredPhase::RestoreRepairScissor(index) => {
                            let [x, y, width, height] = scissors[index];
                            unsafe {
                                self.gl.enable(glow::SCISSOR_TEST);
                                self.gl.scissor(x, y, width, height);
                            }
                        }
                        LegacySceneScissoredPhase::Lamp(index) => {
                            let [x, y, width, height] = scissors[index];
                            let output_rect = gl_scissor_to_output_rect(
                                [x, y, width, height],
                                self.scene_state.current_size.1,
                                framebuffer_origin,
                            );
                            draw_result = self.draw_lamp_overlay(output_rect);
                        }
                        LegacySceneScissoredPhase::Squash(index) => {
                            let [x, y, width, height] = scissors[index];
                            let output_rect = gl_scissor_to_output_rect(
                                [x, y, width, height],
                                self.scene_state.current_size.1,
                                framebuffer_origin,
                            );
                            draw_result = self.draw_squash_overlay(output_rect);
                        }
                        LegacySceneScissoredPhase::ExternalOverlays(index) => {
                            let [x, y, width, height] = scissors[index];
                            let output_rect = gl_scissor_to_output_rect(
                                [x, y, width, height],
                                self.scene_state.current_size.1,
                                framebuffer_origin,
                            );
                            draw_result = self.draw_command_batch(false, output_rect);
                        }
                    }
                }
                if disable_scissor_after {
                    unsafe {
                        self.gl.disable(glow::SCISSOR_TEST);
                    }
                }
                draw_result?;
                self.scene_state.frame_stats.scissor_passes = scissors.len();
            }
        }

        unsafe {
            self.gl.disable(glow::SCISSOR_TEST);
            self.gl.bind_texture(glow::TEXTURE_2D, None);
        }
        Ok(())
    }

    fn prepare_lifecycle_visual_sources(
        &mut self,
        plan: &RepaintPlan,
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> RendererResult<()> {
        let mut context = LifecycleRenderContext::new(
            &self.gl,
            &mut self.scene_state,
            &mut self.effect_runtime,
            self.resources.texture_view(),
        );
        self.lifecycle
            .prepare_visual_sources(&mut context, plan, framebuffer_origin)
    }

    fn draw_lamp_overlay(&mut self, scissor: Option<OutputRect>) -> RendererResult<()> {
        let mut context = LifecycleRenderContext::new(
            &self.gl,
            &mut self.scene_state,
            &mut self.effect_runtime,
            self.resources.texture_view(),
        );
        self.lifecycle.draw_lamp_overlay(&mut context, scissor)
    }

    fn draw_squash_overlay(&mut self, scissor: Option<OutputRect>) -> RendererResult<()> {
        let mut context = LifecycleRenderContext::new(
            &self.gl,
            &mut self.scene_state,
            &mut self.effect_runtime,
            self.resources.texture_view(),
        );
        self.lifecycle.draw_squash_overlay(&mut context, scissor)
    }

    pub(crate) fn draw_lifecycle_overlays(
        &mut self,
        rects: &[OutputRect],
        framebuffer_origin: OutputFramebufferOrigin,
        plan: &RepaintPlan,
    ) -> RendererResult<()> {
        let mut context = LifecycleRenderContext::new(
            &self.gl,
            &mut self.scene_state,
            &mut self.effect_runtime,
            self.resources.texture_view(),
        );
        self.lifecycle
            .draw_overlays(&mut context, rects, framebuffer_origin, plan)
    }

    fn draw_effect_overlays(
        &mut self,
        rects: &[OutputRect],
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> RendererResult<()> {
        self.effect_execution_context()
            .draw_effect_overlays(rects, framebuffer_origin)
    }

    fn draw_command_batch(
        &mut self,
        scene: bool,
        scissor: Option<OutputRect>,
    ) -> RendererResult<()> {
        self.effect_execution_context()
            .draw_command_batch(scene, scissor)
    }

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

fn resolve_scene_damage_authority(
    scene_changed: bool,
    damage_authority_available: bool,
    output_damage: OutputDamage,
) -> (OutputDamage, bool) {
    if scene_changed && !damage_authority_available && output_damage == OutputDamage::Empty {
        (OutputDamage::Full, true)
    } else {
        (output_damage, false)
    }
}

#[allow(clippy::too_many_arguments)]
fn push_egl_decoration_instance(
    vertices: &mut Vec<EglTexturedVertex>,
    commands: &mut Vec<EglDrawCommand>,
    output_width: u32,
    output_height: u32,
    instance: &DecorationRenderInstance,
    output_scale: f64,
    framebuffer_origin: OutputFramebufferOrigin,
    visual_group: Option<VisualGroupId>,
) {
    let command_start = commands.len();
    for primitive in instance.primitives() {
        match primitive {
            DecorationRenderPrimitive::SolidRect { rect, color } => {
                push_egl_decoration_rect(
                    vertices,
                    commands,
                    output_width,
                    output_height,
                    EglDrawLayer::SolidRgba(rgba_to_pixel(*color)),
                    instance,
                    *rect,
                    output_scale,
                    framebuffer_origin,
                );
            }
            DecorationRenderPrimitive::Image { rect, asset } => {
                push_egl_decoration_rect(
                    vertices,
                    commands,
                    output_width,
                    output_height,
                    EglDrawLayer::DecorationAsset(asset.asset_id()),
                    instance,
                    *rect,
                    output_scale,
                    framebuffer_origin,
                );
            }
            DecorationRenderPrimitive::Text {
                rect, clip, asset, ..
            } => push_egl_decoration_text(
                vertices,
                commands,
                output_width,
                output_height,
                instance,
                *rect,
                *clip,
                asset,
                output_scale,
                framebuffer_origin,
            ),
        }
    }
    for command in &mut commands[command_start..] {
        command.visual_group = visual_group;
    }
}

#[allow(clippy::too_many_arguments)]
fn push_egl_decoration_rect(
    vertices: &mut Vec<EglTexturedVertex>,
    commands: &mut Vec<EglDrawCommand>,
    output_width: u32,
    output_height: u32,
    layer: EglDrawLayer,
    instance: &DecorationRenderInstance,
    rect: oblivion_one::compositor::DecorationRect,
    output_scale: f64,
    framebuffer_origin: OutputFramebufferOrigin,
) {
    let scale = output_scale.max(1.0) as f32;
    let (origin_x, origin_y) = instance.origin();
    let x = (origin_x.saturating_add(rect.x) as f32) * scale;
    let y = (origin_y.saturating_add(rect.y) as f32) * scale;
    push_draw_command(
        vertices,
        commands,
        layer,
        EglRect::new(x, y, rect.width as f32 * scale, rect.height as f32 * scale),
        output_width,
        output_height,
        framebuffer_origin,
    );
}

#[allow(clippy::too_many_arguments)]
fn push_egl_decoration_text(
    vertices: &mut Vec<EglTexturedVertex>,
    commands: &mut Vec<EglDrawCommand>,
    output_width: u32,
    output_height: u32,
    instance: &DecorationRenderInstance,
    rect: oblivion_one::compositor::DecorationRect,
    clip: oblivion_one::compositor::DecorationRect,
    asset: &oblivion_one::compositor::DecorationRasterAsset,
    output_scale: f64,
    framebuffer_origin: OutputFramebufferOrigin,
) {
    let scale = output_scale.max(1.0) as f32;
    let (origin_x, origin_y) = instance.origin();
    let Some(crop) = clipped_decoration_text_geometry(rect, clip) else {
        return;
    };
    let crop_rect = crop.rect;
    let uv = EglUvRect::new(crop.uv[0], crop.uv[1], crop.uv[2], crop.uv[3]);
    push_draw_command_with_uv(
        vertices,
        commands,
        EglDrawLayer::DecorationAsset(asset.asset_id()),
        EglRect::new(
            (origin_x.saturating_add(crop_rect.x) as f32) * scale,
            (origin_y.saturating_add(crop_rect.y) as f32) * scale,
            crop_rect.width as f32 * scale,
            crop_rect.height as f32 * scale,
        ),
        uv,
        SurfaceSampling::ScaledLinear,
        output_width,
        output_height,
        framebuffer_origin,
    );
}

fn rgba_to_pixel(color: [u8; 4]) -> u32 {
    (u32::from(color[3]) << 24)
        | (u32::from(color[0]) << 16)
        | (u32::from(color[1]) << 8)
        | u32::from(color[2])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EglSceneCacheKey {
    width: u32,
    height: u32,
    content_generation: u64,
    output_scale_key: u32,
    surface_signature_hash: u64,
    decoration_signature_hash: u64,
    popup_surface_signature_hash: u64,
    external_overlay_surface_signature_hash: u64,
    presentation_geometry_signature: u64,
    framebuffer_origin: OutputFramebufferOrigin,
}

impl EglSceneCacheKey {
    #[allow(dead_code)] // Retained for focused cache-key unit tests.
    fn new(
        width: u32,
        height: u32,
        content_generation: u64,
        output_scale_key: u32,
        surface_signatures: &[EglSceneSurfaceSignature],
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> Self {
        Self::new_with_presentation(
            width,
            height,
            content_generation,
            output_scale_key,
            surface_signatures,
            0,
            framebuffer_origin,
        )
    }

    fn new_with_presentation(
        width: u32,
        height: u32,
        content_generation: u64,
        output_scale_key: u32,
        surface_signatures: &[EglSceneSurfaceSignature],
        presentation_geometry_signature: u64,
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> Self {
        Self::new_with_presentation_and_external_overlay_ids(
            width,
            height,
            content_generation,
            output_scale_key,
            surface_signatures,
            &[],
            presentation_geometry_signature,
            framebuffer_origin,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "cache-key construction keeps the explicit overlay partition authority alongside scene state"
    )]
    fn new_with_presentation_and_external_overlay_ids(
        width: u32,
        height: u32,
        content_generation: u64,
        output_scale_key: u32,
        surface_signatures: &[EglSceneSurfaceSignature],
        external_overlay_surface_ids: &[u32],
        presentation_geometry_signature: u64,
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> Self {
        Self {
            width,
            height,
            content_generation,
            output_scale_key,
            surface_signature_hash: egl_scene_surface_signature_hash(surface_signatures),
            decoration_signature_hash: egl_decoration_signature_hash(&[]),
            popup_surface_signature_hash: 0,
            external_overlay_surface_signature_hash: egl_external_overlay_surface_signature_hash(
                external_overlay_surface_ids,
            ),
            presentation_geometry_signature,
            framebuffer_origin,
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "cache-key construction keeps each render-state component and overlay partition explicit"
    )]
    fn new_with_decorations_and_external_overlay_ids(
        width: u32,
        height: u32,
        content_generation: u64,
        output_scale_key: u32,
        surface_signatures: &[EglSceneSurfaceSignature],
        presentation_geometry_signature: u64,
        external_overlay_surface_ids: &[u32],
        decoration_instances: &[DecorationRenderInstance],
        popup_surface_ids: &[u32],
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> Self {
        let mut key = Self::new_with_presentation_and_external_overlay_ids(
            width,
            height,
            content_generation,
            output_scale_key,
            surface_signatures,
            external_overlay_surface_ids,
            presentation_geometry_signature,
            framebuffer_origin,
        );
        key.decoration_signature_hash = egl_decoration_signature_hash(decoration_instances);
        key.popup_surface_signature_hash = egl_popup_surface_signature_hash(popup_surface_ids);
        key
    }

    #[cfg(test)]
    fn new_with_decoration_snapshots(
        width: u32,
        height: u32,
        content_generation: u64,
        output_scale_key: u32,
        surface_signatures: &[EglSceneSurfaceSignature],
        decoration_snapshots: &[DecorationSceneSnapshot],
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> Self {
        let mut key = Self::new(
            width,
            height,
            content_generation,
            output_scale_key,
            surface_signatures,
            framebuffer_origin,
        );
        key.decoration_signature_hash =
            egl_decoration_snapshot_signature_hash(decoration_snapshots);
        key
    }

    #[cfg(test)]
    fn is_current(
        self,
        width: u32,
        height: u32,
        content_generation: u64,
        output_scale_key: u32,
        surface_signatures: &[EglSceneSurfaceSignature],
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> bool {
        self.is_current_with_decorations(
            width,
            height,
            content_generation,
            output_scale_key,
            surface_signatures,
            &[],
            &[],
            0,
            framebuffer_origin,
        )
    }

    #[cfg(test)]
    #[expect(
        clippy::too_many_arguments,
        reason = "cache-key validation keeps each render-state component explicit"
    )]
    fn is_current_with_decorations(
        self,
        width: u32,
        height: u32,
        _content_generation: u64,
        output_scale_key: u32,
        surface_signatures: &[EglSceneSurfaceSignature],
        decoration_instances: &[DecorationRenderInstance],
        popup_surface_ids: &[u32],
        presentation_geometry_signature: u64,
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> bool {
        self.is_current_with_decorations_and_external_overlay_ids(
            width,
            height,
            _content_generation,
            output_scale_key,
            surface_signatures,
            &[],
            decoration_instances,
            popup_surface_ids,
            presentation_geometry_signature,
            framebuffer_origin,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "cache-key validation keeps each render-state component and overlay partition explicit"
    )]
    fn is_current_with_decorations_and_external_overlay_ids(
        self,
        width: u32,
        height: u32,
        _content_generation: u64,
        output_scale_key: u32,
        surface_signatures: &[EglSceneSurfaceSignature],
        external_overlay_surface_ids: &[u32],
        decoration_instances: &[DecorationRenderInstance],
        popup_surface_ids: &[u32],
        presentation_geometry_signature: u64,
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> bool {
        self.width == width
            && self.height == height
            && self.output_scale_key == output_scale_key
            && self.surface_signature_hash == egl_scene_surface_signature_hash(surface_signatures)
            && self.decoration_signature_hash == egl_decoration_signature_hash(decoration_instances)
            && self.popup_surface_signature_hash
                == egl_popup_surface_signature_hash(popup_surface_ids)
            && self.external_overlay_surface_signature_hash
                == egl_external_overlay_surface_signature_hash(external_overlay_surface_ids)
            && self.presentation_geometry_signature == presentation_geometry_signature
            && self.framebuffer_origin == framebuffer_origin
    }

    #[cfg(test)]
    #[expect(
        clippy::too_many_arguments,
        reason = "test cache-key validation mirrors the production state comparison"
    )]
    fn is_current_with_decoration_snapshots(
        self,
        width: u32,
        height: u32,
        _content_generation: u64,
        output_scale_key: u32,
        surface_signatures: &[EglSceneSurfaceSignature],
        decoration_snapshots: &[DecorationSceneSnapshot],
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> bool {
        self.width == width
            && self.height == height
            && self.output_scale_key == output_scale_key
            && self.surface_signature_hash == egl_scene_surface_signature_hash(surface_signatures)
            && self.decoration_signature_hash
                == egl_decoration_snapshot_signature_hash(decoration_snapshots)
            && self.framebuffer_origin == framebuffer_origin
    }
}

fn egl_popup_surface_signature_hash(popup_surface_ids: &[u32]) -> u64 {
    if popup_surface_ids.is_empty() {
        return 0;
    }
    popup_surface_ids
        .iter()
        .fold(0xcbf2_9ce4_8422_2325, |hash, id| {
            (hash ^ u64::from(*id)).wrapping_mul(0x0000_0100_0000_01b3)
        })
}

fn egl_external_overlay_surface_signature_hash(external_overlay_surface_ids: &[u32]) -> u64 {
    if external_overlay_surface_ids.is_empty() {
        return 0;
    }
    external_overlay_surface_ids
        .iter()
        .fold(0xcbf2_9ce4_8422_2325, |hash, id| {
            (hash ^ u64::from(*id)).wrapping_mul(0x0000_0100_0000_01b3)
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EglSceneSurfaceSignature {
    surface_id: u32,
    commit_sequence: u64,
    buffer_id: u64,
    buffer_width: u32,
    buffer_height: u32,
    buffer_scale: u32,
    buffer_transform: wayland_server::protocol::wl_output::Transform,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    render_x: i32,
    render_y: i32,
    clip_x: i32,
    clip_y: i32,
    clip_width: u32,
    clip_height: u32,
    generation: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct EglCheckpointRectBits([u32; 4]);

impl EglCheckpointRectBits {
    fn from_rect(rect: EglRect) -> Self {
        Self([
            rect.x().to_bits(),
            rect.y().to_bits(),
            rect.width().to_bits(),
            rect.height().to_bits(),
        ])
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EglCheckpointPixelSourceIdentity {
    Layer(EglDrawLayer),
    Surface(EglSceneSurfaceSignature),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct EglCheckpointCommandCausalSnapshot {
    layer: EglDrawLayer,
    visual_group: Option<VisualGroupId>,
    bounds: EglCheckpointRectBits,
    opaque_regions: Vec<EglCheckpointRectBits>,
    presentation_clip: Option<EglCheckpointRectBits>,
    vertex_start: u32,
    vertex_count: u32,
    sampling: SurfaceSampling,
    presentation_opacity_bits: u32,
    presentation_owner_root: Option<u32>,
    vertices: Vec<[u32; 4]>,
    pixel_source: Option<EglCheckpointPixelSourceIdentity>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct EglCheckpointSceneCausalSnapshot {
    output_size: (u32, u32),
    commands: Vec<EglCheckpointCommandCausalSnapshot>,
    presentation_visual_group_owners: HashMap<VisualGroupId, u32>,
}

impl EglCheckpointSceneCausalSnapshot {
    fn new(
        output_size: (u32, u32),
        commands: &[EglDrawCommand],
        vertices: &[EglTexturedVertex],
        surface_signatures: &[EglSceneSurfaceSignature],
        presentation_opacities: &[f32],
        presentation_visual_group_owners: &HashMap<VisualGroupId, u32>,
    ) -> Self {
        let surface_signatures = surface_signatures
            .iter()
            .map(|signature| (signature.surface_id, *signature))
            .collect::<HashMap<_, _>>();
        let commands = commands
            .iter()
            .enumerate()
            .map(|(command_index, command)| {
                let start = usize::try_from(command.vertex_start).ok();
                let end = start.and_then(|start| {
                    usize::try_from(command.vertex_count)
                        .ok()
                        .and_then(|count| start.checked_add(count))
                });
                let command_vertices = start
                    .zip(end)
                    .and_then(|(start, end)| vertices.get(start..end))
                    .map(|vertices| {
                        vertices
                            .iter()
                            .map(|vertex| {
                                [
                                    vertex.position[0].to_bits(),
                                    vertex.position[1].to_bits(),
                                    vertex.uv[0].to_bits(),
                                    vertex.uv[1].to_bits(),
                                ]
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let vertex_range_is_valid = start
                    .zip(end)
                    .is_some_and(|(start, end)| vertices.get(start..end).is_some());
                let pixel_source = match command.layer {
                    EglDrawLayer::Surface(surface_id) => surface_signatures
                        .get(&surface_id)
                        .copied()
                        .map(EglCheckpointPixelSourceIdentity::Surface),
                    EglDrawLayer::Solid(_)
                    | EglDrawLayer::SolidRgba(_)
                    | EglDrawLayer::DecorationAsset(_) => {
                        Some(EglCheckpointPixelSourceIdentity::Layer(command.layer))
                    }
                    EglDrawLayer::LifecycleResolvedVisual(_) | EglDrawLayer::Cursor => None,
                };
                EglCheckpointCommandCausalSnapshot {
                    layer: command.layer,
                    visual_group: command.visual_group,
                    bounds: EglCheckpointRectBits::from_rect(command.bounds),
                    opaque_regions: command
                        .opaque_regions
                        .iter()
                        .copied()
                        .map(EglCheckpointRectBits::from_rect)
                        .collect(),
                    presentation_clip: command
                        .presentation_clip
                        .map(EglCheckpointRectBits::from_rect),
                    vertex_start: command.vertex_start,
                    vertex_count: command.vertex_count,
                    sampling: command.sampling,
                    presentation_opacity_bits: presentation_opacities
                        .get(command_index)
                        .copied()
                        .unwrap_or(1.0)
                        .to_bits(),
                    presentation_owner_root: command
                        .visual_group
                        .and_then(|group| presentation_visual_group_owners.get(&group).copied()),
                    vertices: if vertex_range_is_valid {
                        command_vertices
                    } else {
                        Vec::new()
                    },
                    pixel_source: pixel_source.filter(|_| vertex_range_is_valid),
                }
            })
            .collect();
        Self {
            output_size,
            commands,
            presentation_visual_group_owners: presentation_visual_group_owners.clone(),
        }
    }

    fn presentation_owner_for_visual_group(
        &self,
        visual_group: Option<VisualGroupId>,
    ) -> Option<u32> {
        visual_group.and_then(|group| self.presentation_visual_group_owners.get(&group).copied())
    }

    fn unchanged_command_prefix_len(&self, current: &Self) -> usize {
        if self.output_size != current.output_size {
            return 0;
        }
        self.commands
            .iter()
            .zip(&current.commands)
            .take_while(|(previous, current)| {
                previous.pixel_source.is_some()
                    && current.pixel_source.is_some()
                    && previous == current
            })
            .count()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CheckpointEffectCausalState {
    semantic_signature: u64,
    frame_demand: oblivion_one::effects::EffectFrameDemand,
    causal_backdrop_only: bool,
    capture_owner_root: Option<u32>,
    composition_boundary: Option<(
        usize,
        oblivion_one::compositor::EffectAnchor,
        Option<VisualGroupId>,
        oblivion_one::compositor::EffectAnchorScope,
    )>,
    dependencies: Vec<oblivion_one::effects::EffectInstanceId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CheckpointCausalState {
    scene: EglCheckpointSceneCausalSnapshot,
    effects: HashMap<oblivion_one::effects::EffectInstanceId, CheckpointEffectCausalState>,
}

impl CheckpointCausalState {
    fn new(
        scene: EglCheckpointSceneCausalSnapshot,
        graph: Option<&oblivion_one::effects::CompiledFrameGraph>,
        commands: &[EglDrawCommand],
    ) -> Self {
        let mut effects = HashMap::new();
        if let Some(graph) = graph {
            let passes_by_id = graph
                .passes
                .iter()
                .map(|pass| (pass.id, pass))
                .collect::<HashMap<_, _>>();
            for instance in &graph.instances {
                let scene_captures = graph
                    .passes
                    .iter()
                    .filter(|pass| {
                        pass.instance == instance.id
                            && pass.kind == oblivion_one::effects::RenderPassKind::SceneCapture
                    })
                    .collect::<Vec<_>>();
                let has_surface_capture = graph.passes.iter().any(|pass| {
                    pass.instance == instance.id
                        && pass.kind == oblivion_one::effects::RenderPassKind::SurfaceCapture
                });
                let capture_dependencies = scene_captures
                    .first()
                    .and_then(|pass| {
                        pass.checkpoint_dependencies
                            .iter()
                            .map(|dependency| {
                                passes_by_id
                                    .get(dependency)
                                    .map(|dependency_pass| dependency_pass.instance)
                            })
                            .collect::<Option<Vec<_>>>()
                    })
                    .unwrap_or_default();
                effects.insert(
                    instance.id,
                    CheckpointEffectCausalState {
                        semantic_signature: instance.semantic_signature,
                        frame_demand: instance.frame_demand,
                        causal_backdrop_only: scene_captures.len() == 1 && !has_surface_capture,
                        capture_owner_root: scene_captures.first().and_then(|pass| {
                            scene.presentation_owner_for_visual_group(pass.visual_group)
                        }),
                        composition_boundary: scene_captures.first().map(|pass| {
                            let (draw_end, _) = effects::composition_range(
                                commands,
                                pass.anchor,
                                pass.visual_group,
                                pass.anchor_scope,
                            );
                            (draw_end, pass.anchor, pass.visual_group, pass.anchor_scope)
                        }),
                        dependencies: capture_dependencies,
                    },
                );
            }
        }
        Self { scene, effects }
    }
}

fn split_external_overlay_surfaces(
    surfaces: &[RenderableSurface],
    external_overlay_surface_ids: &[u32],
) -> (Vec<RenderableSurface>, Vec<RenderableSurface>) {
    if external_overlay_surface_ids.is_empty() {
        return (Vec::new(), Vec::new());
    }
    surfaces
        .iter()
        .cloned()
        .partition(|surface| !external_overlay_surface_ids.contains(&surface.surface_id))
}

#[allow(clippy::too_many_arguments)]
fn push_egl_surface_commands(
    vertices: &mut Vec<EglTexturedVertex>,
    commands: &mut Vec<EglDrawCommand>,
    width: u32,
    height: u32,
    surface: &RenderableSurface,
    render_assignment: compositor::SurfaceRenderSpaceAssignment,
    framebuffer_origin: OutputFramebufferOrigin,
    visual_group: Option<VisualGroupId>,
) {
    let command_start = commands.len();
    if let Some(bounds) =
        compositor::xwayland_visual_backing_target(surface, render_assignment.visual_clip.as_ref())
    {
        push_draw_command(
            vertices,
            commands,
            EglDrawLayer::Solid(compositor::ServerFrameColor::XwaylandBacking),
            EglRect::new(
                bounds.x() as f32,
                bounds.y() as f32,
                bounds.width() as f32,
                bounds.height() as f32,
            ),
            width,
            height,
            framebuffer_origin,
        );
    }
    for render_plan in compositor::surface_render_plans_with_aperture(
        surface,
        render_assignment.target,
        render_assignment.visual_clip.as_ref(),
    ) {
        push_egl_render_plan(
            vertices,
            commands,
            width,
            height,
            surface,
            render_plan,
            framebuffer_origin,
        );
    }
    for command in &mut commands[command_start..] {
        command.visual_group = visual_group;
    }
}

fn push_egl_render_plan(
    vertices: &mut Vec<EglTexturedVertex>,
    commands: &mut Vec<EglDrawCommand>,
    width: u32,
    height: u32,
    surface: &RenderableSurface,
    render_plan: compositor::SurfaceRenderPlan,
    framebuffer_origin: OutputFramebufferOrigin,
) {
    let uv = EglUvRect::from_surface_uv_quad(render_plan.content_uv);
    let sampling = surface_sampling_for_plan(
        surface.buffer_size().width,
        surface.buffer_size().height,
        render_plan.content_target.x(),
        render_plan.content_target.y(),
        render_plan.content_target.width(),
        render_plan.content_target.height(),
        uv,
    );
    push_draw_command_with_uv(
        vertices,
        commands,
        EglDrawLayer::Surface(surface.surface_id),
        EglRect::new(
            render_plan.content_target.x() as f32,
            render_plan.content_target.y() as f32,
            render_plan.content_target.width() as f32,
            render_plan.content_target.height() as f32,
        ),
        uv,
        sampling,
        width,
        height,
        framebuffer_origin,
    );
    if let Some(command) = commands.last_mut()
        && command.layer == EglDrawLayer::Surface(surface.surface_id)
    {
        command.opaque_regions = opaque_region_output_rects(surface, render_plan);
    }
}

fn opaque_region_output_rects(
    surface: &RenderableSurface,
    render_plan: compositor::SurfaceRenderPlan,
) -> Vec<EglRect> {
    match surface.opaque_region() {
        SurfaceOpaqueRegion::None => Vec::new(),
        SurfaceOpaqueRegion::Full => vec![egl_rect_from_target(render_plan.content_target)],
        SurfaceOpaqueRegion::Partial(rects) => rects
            .iter()
            .filter_map(|rect| map_opaque_rect_to_output(surface, render_plan, *rect))
            .collect(),
    }
}

fn egl_rect_from_target(target: compositor::SurfaceTargetRect) -> EglRect {
    EglRect::new(
        target.x() as f32,
        target.y() as f32,
        target.width() as f32,
        target.height() as f32,
    )
}

fn map_opaque_rect_to_output(
    surface: &RenderableSurface,
    render_plan: compositor::SurfaceRenderPlan,
    opaque: SurfaceOpaqueRect,
) -> Option<EglRect> {
    if surface.width == 0 || surface.height == 0 {
        return None;
    }
    let visual = render_plan.visual_target;
    let logical_x = f64::from(opaque.x());
    let logical_y = f64::from(opaque.y());
    let logical_right = logical_x + f64::from(opaque.width());
    let logical_bottom = logical_y + f64::from(opaque.height());
    let scale_x = f64::from(visual.width()) / f64::from(surface.width);
    let scale_y = f64::from(visual.height()) / f64::from(surface.height);
    let left = f64::from(visual.x()) + logical_x * scale_x;
    let top = f64::from(visual.y()) + logical_y * scale_y;
    let right = f64::from(visual.x()) + logical_right * scale_x;
    let bottom = f64::from(visual.y()) + logical_bottom * scale_y;
    let clip = render_plan.content_target;
    let clipped_left = left.max(f64::from(clip.x()));
    let clipped_top = top.max(f64::from(clip.y()));
    let clipped_right = right.min(f64::from(clip.x()) + f64::from(clip.width()));
    let clipped_bottom = bottom.min(f64::from(clip.y()) + f64::from(clip.height()));
    (clipped_right > clipped_left && clipped_bottom > clipped_top).then_some(EglRect::new(
        clipped_left as f32,
        clipped_top as f32,
        (clipped_right - clipped_left) as f32,
        (clipped_bottom - clipped_top) as f32,
    ))
}

fn egl_scene_surface_signatures(surfaces: &[RenderableSurface]) -> Vec<EglSceneSurfaceSignature> {
    surfaces
        .iter()
        .map(|surface| {
            let render_placement = surface.render_placement.unwrap_or(surface.placement);
            let clip = surface.visual_clip.as_ref();
            EglSceneSurfaceSignature {
                surface_id: surface.surface_id,
                commit_sequence: surface.commit_sequence.get(),
                buffer_id: surface.buffer_id().get(),
                buffer_width: surface.buffer_size().width,
                buffer_height: surface.buffer_size().height,
                buffer_scale: surface.buffer_scale,
                buffer_transform: surface.buffer_transform,
                x: surface.x,
                y: surface.y,
                width: surface.width,
                height: surface.height,
                render_x: render_placement.local_x,
                render_y: render_placement.local_y,
                clip_x: clip.map_or(0, |clip| clip.x()),
                clip_y: clip.map_or(0, |clip| clip.y()),
                clip_width: clip.map_or(0, |clip| clip.width()),
                clip_height: clip.map_or(0, |clip| clip.height()),
                generation: surface.generation,
            }
        })
        .collect()
}

fn egl_scene_surface_signature_hash(signatures: &[EglSceneSurfaceSignature]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for signature in signatures {
        hash = fnv1a_u64(hash, u64::from(signature.surface_id));
        hash = fnv1a_u64(hash, u64::from(signature.buffer_width));
        hash = fnv1a_u64(hash, u64::from(signature.buffer_height));
        hash = fnv1a_u64(hash, u64::from(signature.buffer_scale));
        hash = fnv1a_u64(hash, signature.buffer_transform as u32 as u64);
        hash = fnv1a_u64(hash, signature.x as u32 as u64);
        hash = fnv1a_u64(hash, signature.y as u32 as u64);
        hash = fnv1a_u64(hash, u64::from(signature.width));
        hash = fnv1a_u64(hash, u64::from(signature.height));
        hash = fnv1a_u64(hash, signature.render_x as u32 as u64);
        hash = fnv1a_u64(hash, signature.render_y as u32 as u64);
        hash = fnv1a_u64(hash, signature.clip_x as u32 as u64);
        hash = fnv1a_u64(hash, signature.clip_y as u32 as u64);
        hash = fnv1a_u64(hash, u64::from(signature.clip_width));
        hash = fnv1a_u64(hash, u64::from(signature.clip_height));
    }
    hash
}

fn egl_decoration_signature_hash(instances: &[DecorationRenderInstance]) -> u64 {
    let snapshots = instances
        .iter()
        .map(DecorationRenderInstance::scene_snapshot)
        .collect::<Vec<_>>();
    egl_decoration_snapshot_signature_hash(&snapshots)
}

fn egl_decoration_snapshot_signature_hash(snapshots: &[DecorationSceneSnapshot]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for snapshot in snapshots {
        let (window_id, root_surface_id) = snapshot.identity();
        let (x, y, width, height) = snapshot.bounds();
        for value in [
            window_id.get(),
            u64::from(root_surface_id),
            x as u64,
            y as u64,
            u64::from(width),
            u64::from(height),
            snapshot.visual_signature(),
        ] {
            hash = fnv1a_u64(hash, value);
        }
    }
    hash
}

const fn fnv1a_u64(hash: u64, value: u64) -> u64 {
    (hash ^ value).wrapping_mul(0x0000_0100_0000_01b3)
}

fn ensure_vertex_buffer_capacity(
    gl: &glow::Context,
    vertex_buffer: GlBuffer,
    current_capacity: &mut usize,
    required_size: usize,
) {
    if *current_capacity >= required_size && *current_capacity > 0 {
        return;
    }
    let capacity = required_size
        .max(MIN_VERTEX_BUFFER_BYTES)
        .next_power_of_two();
    unsafe {
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(vertex_buffer));
        gl.buffer_data_size(glow::ARRAY_BUFFER, capacity as i32, glow::DYNAMIC_DRAW);
    }
    *current_capacity = capacity;
}

fn gl_scissor_to_output_rect(
    [x, y, width, height]: [i32; 4],
    output_height: u32,
    framebuffer_origin: OutputFramebufferOrigin,
) -> Option<OutputRect> {
    let top = match framebuffer_origin {
        OutputFramebufferOrigin::BottomLeft => i32::try_from(output_height)
            .ok()?
            .checked_sub(y.checked_add(height)?)?,
        OutputFramebufferOrigin::TopLeftScanout => y,
    };
    (width > 0 && height > 0).then_some(OutputRect::new(x, top, width as u32, height as u32))
}

fn output_rect_for_egl_clip(clip: EglRect) -> Option<OutputRect> {
    if !clip.x().is_finite()
        || !clip.y().is_finite()
        || !clip.width().is_finite()
        || !clip.height().is_finite()
        || clip.width() <= 0.0
        || clip.height() <= 0.0
    {
        return None;
    }
    let left = f64::from(clip.x()).floor();
    let top = f64::from(clip.y()).floor();
    let right = (f64::from(clip.x()) + f64::from(clip.width())).ceil();
    let bottom = (f64::from(clip.y()) + f64::from(clip.height())).ceil();
    let left = left.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
    let top = top.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
    let right = right.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
    let bottom = bottom.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
    let width = (i64::from(right) - i64::from(left)).clamp(0, i64::from(u32::MAX)) as u32;
    let height = (i64::from(bottom) - i64::from(top)).clamp(0, i64::from(u32::MAX)) as u32;
    (width > 0 && height > 0).then_some(OutputRect::new(left, top, width, height))
}

pub(super) fn intersect_output_rect(left: OutputRect, right: OutputRect) -> Option<OutputRect> {
    let x = i64::from(left.x).max(i64::from(right.x));
    let y = i64::from(left.y).max(i64::from(right.y));
    let right_edge = i64::from(left.x)
        .saturating_add(i64::from(left.width))
        .min(i64::from(right.x).saturating_add(i64::from(right.width)));
    let bottom_edge = i64::from(left.y)
        .saturating_add(i64::from(left.height))
        .min(i64::from(right.y).saturating_add(i64::from(right.height)));
    (right_edge > x && bottom_edge > y).then_some(OutputRect::new(
        x.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
        y.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
        (right_edge - x).clamp(0, i64::from(u32::MAX)) as u32,
        (bottom_edge - y).clamp(0, i64::from(u32::MAX)) as u32,
    ))
}

pub(crate) fn choose_native_egl_config(
    egl: &EglInstance,
    display: egl::Display,
    native_visual_id: u32,
) -> RendererResult<egl::Config> {
    let mut configs = Vec::with_capacity(egl.get_config_count(display)?);
    egl.get_configs(display, &mut configs)?;
    let candidates = configs
        .iter()
        .copied()
        .map(|config| native_egl_config_candidate(egl, display, config))
        .collect::<Result<Vec<_>, _>>()?;
    if native_egl_debug_enabled() {
        for candidate in &candidates {
            eprintln!("{}", native_egl_config_candidate_diagnostic(candidate));
        }
    }
    let selected = select_native_egl_config_candidate(&candidates, native_visual_id)?;
    configs
        .get(selected)
        .copied()
        .ok_or_else(|| io::Error::other("selected EGL config index out of range").into())
}

pub(crate) fn choose_surfaceless_egl_config(
    egl: &EglInstance,
    display: egl::Display,
    native_visual_id: u32,
) -> RendererResult<egl::Config> {
    let mut configs = Vec::with_capacity(egl.get_config_count(display)?);
    egl.get_configs(display, &mut configs)?;
    let candidates = configs
        .iter()
        .copied()
        .map(|config| native_egl_config_candidate(egl, display, config))
        .collect::<Result<Vec<_>, _>>()?;
    let selected = candidates
        .iter()
        .position(|candidate| {
            native_egl_config_candidate_matches_common(candidate, native_visual_id)
        })
        .ok_or_else(|| {
            io::Error::other(format!(
                "EGL has no GLES3-capable surfaceless config for native visual {}",
                native_visual_label(native_visual_id)
            ))
        })?;
    configs.get(selected).copied().ok_or_else(|| {
        io::Error::other("selected surfaceless EGL config index out of range").into()
    })
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
pub(crate) fn select_native_egl_visual_format(
    formats: &[u32],
    candidates: &[NativeEglConfigCandidate],
) -> RendererResult<u32> {
    formats
        .iter()
        .copied()
        .find(|format| select_native_egl_config_candidate(candidates, *format).is_ok())
        .ok_or_else(|| {
            let requested = formats
                .iter()
                .map(|format| native_visual_label(*format))
                .collect::<Vec<_>>()
                .join(", ");
            io::Error::other(format!(
                "EGL has no GLES3-capable GBM window config for requested native visuals: {requested}"
            ))
            .into()
        })
}

pub(crate) fn select_native_egl_config_candidate(
    candidates: &[NativeEglConfigCandidate],
    native_visual_id: u32,
) -> RendererResult<usize> {
    candidates
        .iter()
        .position(|candidate| native_egl_config_candidate_matches(candidate, native_visual_id))
        .ok_or_else(|| {
            io::Error::other(format!(
                "EGL has no GLES3-capable GBM window config for native visual {}",
                native_visual_label(native_visual_id)
            ))
            .into()
        })
}

fn native_egl_config_candidate_matches(
    candidate: &NativeEglConfigCandidate,
    native_visual_id: u32,
) -> bool {
    native_egl_config_candidate_matches_common(candidate, native_visual_id)
        && (candidate.surface_type & egl::WINDOW_BIT) != 0
}

fn native_egl_config_candidate_matches_common(
    candidate: &NativeEglConfigCandidate,
    native_visual_id: u32,
) -> bool {
    candidate.native_visual_id == native_visual_id
        && (candidate.renderable_type & egl::OPENGL_ES3_BIT) != 0
        && candidate.red_size >= 8
        && candidate.green_size >= 8
        && candidate.blue_size >= 8
}

fn native_egl_config_candidate(
    egl: &EglInstance,
    display: egl::Display,
    config: egl::Config,
) -> RendererResult<NativeEglConfigCandidate> {
    Ok(NativeEglConfigCandidate {
        config_id: egl.get_config_attrib(display, config, egl::CONFIG_ID)?,
        native_visual_id: egl.get_config_attrib(display, config, egl::NATIVE_VISUAL_ID)? as u32,
        surface_type: egl.get_config_attrib(display, config, egl::SURFACE_TYPE)?,
        renderable_type: egl.get_config_attrib(display, config, egl::RENDERABLE_TYPE)?,
        red_size: egl.get_config_attrib(display, config, egl::RED_SIZE)?,
        green_size: egl.get_config_attrib(display, config, egl::GREEN_SIZE)?,
        blue_size: egl.get_config_attrib(display, config, egl::BLUE_SIZE)?,
        alpha_size: egl.get_config_attrib(display, config, egl::ALPHA_SIZE)?,
    })
}

fn native_egl_debug_enabled() -> bool {
    std::env::var_os("OBLIVION_ONE_DEBUG_EGL").is_some()
}

fn native_egl_config_candidate_diagnostic(candidate: &NativeEglConfigCandidate) -> String {
    format!(
        "native EGL config config_id={} visual={} window={} gles3={} rgba={}/{}/{}/{} surface_type=0x{:x} renderable_type=0x{:x}",
        candidate.config_id,
        native_visual_label(candidate.native_visual_id),
        (candidate.surface_type & egl::WINDOW_BIT) != 0,
        (candidate.renderable_type & egl::OPENGL_ES3_BIT) != 0,
        candidate.red_size,
        candidate.green_size,
        candidate.blue_size,
        candidate.alpha_size,
        candidate.surface_type,
        candidate.renderable_type,
    )
}

pub(crate) fn native_visual_label(native_visual_id: u32) -> String {
    format!(
        "{}/0x{native_visual_id:08x}",
        native_visual_fourcc(native_visual_id)
    )
}

fn native_visual_fourcc(native_visual_id: u32) -> String {
    let bytes = native_visual_id.to_le_bytes();
    if bytes
        .iter()
        .all(|byte| byte.is_ascii_graphic() || *byte == b' ')
    {
        String::from_utf8_lossy(&bytes).into_owned()
    } else {
        "????".to_string()
    }
}

pub(crate) fn create_gles_context(
    egl: &EglInstance,
    display: egl::Display,
    config: egl::Config,
) -> RendererResult<egl::Context> {
    egl.create_context(display, config, None, gles_context_attributes())
        .map_err(|error| io::Error::other(format_gles3_context_error(&error)).into())
}

fn gles_context_attributes() -> &'static [egl::Int] {
    &[egl::CONTEXT_CLIENT_VERSION, 3, egl::NONE]
}

fn format_gles3_context_error(error: &dyn Error) -> String {
    format!("failed to create required GLES3 context: {error}")
}

pub(crate) fn load_egl_image_target_texture_2d(
    egl: &EglInstance,
) -> Option<GlEglImageTargetTexture2DOes> {
    let symbol = egl.get_proc_address("glEGLImageTargetTexture2DOES")?;
    Some(unsafe {
        std::mem::transmute::<extern "system" fn(), GlEglImageTargetTexture2DOes>(symbol)
    })
}

pub(crate) fn load_swap_buffers_with_damage(
    egl: &EglInstance,
    display: egl::Display,
) -> Option<EglSwapBuffersWithDamage> {
    let extensions = egl
        .query_string(Some(display), egl::EXTENSIONS)
        .ok()?
        .to_string_lossy();
    let mut extensions = extensions.split_ascii_whitespace();
    let has_khr = extensions
        .clone()
        .any(|extension| extension == "EGL_KHR_swap_buffers_with_damage");
    let has_ext = extensions.any(|extension| extension == "EGL_EXT_swap_buffers_with_damage");
    let symbol_name = if has_khr {
        "eglSwapBuffersWithDamageKHR"
    } else if has_ext {
        "eglSwapBuffersWithDamageEXT"
    } else {
        return None;
    };
    let symbol = egl.get_proc_address(symbol_name)?;
    Some(unsafe { std::mem::transmute::<extern "system" fn(), EglSwapBuffersWithDamage>(symbol) })
}

pub(crate) fn detect_partial_repaint_capabilities(
    egl: &EglInstance,
    display: egl::Display,
    target_lineage_available: bool,
    partial_render_repair: bool,
    swap_buffers_with_damage: bool,
) -> EglPartialRepaintCapabilities {
    let extensions = egl
        .query_string(Some(display), egl::EXTENSIONS)
        .map(|extensions| extensions.to_string_lossy().into_owned())
        .unwrap_or_default();
    EglPartialRepaintCapabilities {
        buffer_age: !buffer_age_disabled()
            && (target_lineage_available
                || extensions
                    .split_ascii_whitespace()
                    .any(|extension| extension == "EGL_EXT_buffer_age")),
        partial_render_repair,
        swap_buffers_with_damage,
    }
}

fn query_egl_buffer_age(
    egl: &EglInstance,
    display: egl::Display,
    surface: egl::Surface,
    supported: bool,
) -> BufferAge {
    if !supported {
        return BufferAge::Unsupported;
    }
    match egl.query_surface(display, surface, EGL_BUFFER_AGE_EXT) {
        Ok(age) => BufferAge::Value(age),
        Err(error) => {
            if native_egl_debug_enabled() {
                eprintln!("EGL buffer-age query failed: {error}");
            }
            BufferAge::QueryFailed
        }
    }
}

fn buffer_age_disabled() -> bool {
    std::env::var_os("OBLIVION_ONE_DISABLE_BUFFER_AGE").is_some_and(|value| value == "1")
}

pub(crate) fn egl_swap_buffers_with_damage(
    egl: &EglInstance,
    display: egl::Display,
    surface: egl::Surface,
    swap_buffers_with_damage: Option<EglSwapBuffersWithDamage>,
    damage: &EglOutputDamage,
    output_size: (u32, u32),
) -> RendererResult<()> {
    let Some(swap_buffers_with_damage) = swap_buffers_with_damage else {
        egl.swap_buffers(display, surface)?;
        return Ok(());
    };
    let Some(rects) = damage.to_egl_rects(output_size.0, output_size.1) else {
        egl.swap_buffers(display, surface)?;
        return Ok(());
    };

    let ok = unsafe {
        swap_buffers_with_damage(
            display.as_ptr(),
            surface.as_ptr(),
            rects.as_ptr(),
            rects.rect_count() as egl::Int,
        )
    };
    if ok == egl::TRUE {
        Ok(())
    } else {
        Err(egl
            .get_error()
            .map(|error| io::Error::other(format!("eglSwapBuffersWithDamage failed: {error}")))
            .unwrap_or_else(|| io::Error::other("eglSwapBuffersWithDamage failed"))
            .into())
    }
}

#[cfg(test)]
pub(super) mod tests;
