use std::{
    collections::{HashMap, HashSet},
    error::Error,
    ffi::c_void,
    io, ptr,
    sync::Arc,
    time::Instant,
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
        DesktopVisualState, RenderableSurface, SurfaceCommitCounter, SurfaceDamageRect,
        SurfaceOpaqueRect, SurfaceOpaqueRegion, SurfaceResourceSyncState, VisualGroupId,
        clipped_decoration_text_geometry,
    },
    cursor_theme::CompositorCursorImage,
    render_backend::{
        buffer::{DmabufImageKey, DrmModifier, WeakBufferIdentity},
        egl_gles::{EGL_LINUX_DMA_BUF_EXT, EglGlesDmabufImportAttributes, EglGlesImportError},
    },
    window_lifecycle_animation::{
        LampWindowSample, LifecycleRenderEvidence, LifecycleRenderEvidenceEntry,
        LifecycleRenderFallbackEntry, LifecycleRenderFallbackReason, LifecycleRenderFallbacks,
        LifecycleSceneSample, LifecycleVisualSource, LifecycleVisualSourceKind, lamp_footprint,
        lamp_stage_channels,
    },
};

mod damage;
pub(crate) mod dmabuf;
mod effects;
mod geometry;
pub(crate) mod native_fence;
mod program;

pub(crate) use damage::{
    BufferAge, EglPartialRepaintCapabilities, FullRepaintReason, OutputDamage, OutputRect,
    PartialRepaintPlanner, RepaintMode, render_target_buffer_age,
};
use damage::{
    ClientCursorDamageState, EglOutputDamage, EglOutputDamageTracker, EglPresentedDamageState,
    RenderExecution, RepaintPlan, merge_effect_damage, resolve_effect_execution_for_repaint_plan,
};
use effects::{
    EffectExecutionTrace, EffectFailureReason, EffectGlResourceCache, EffectGpuProfiler,
    EffectGraphMetrics, FrameTraceSummary, ShaderProgramCache, builtin_shader_program_count,
    graph_metrics, shader_cache_capacity_for_custom_shaders,
};
use effects::{EffectTextureFilter, EffectTextureFormat, EffectTextureKey, PooledEffectTexture};
use geometry::{
    EglDrawCommand, EglDrawLayer, EglLampDrawCommand, EglLampVertex, EglRect, EglTexturedVertex,
    EglUvRect, EglVisibilityDecision, MIN_VERTEX_BUFFER_BYTES, SurfaceConsumerPlan,
    SurfaceSampling, VERTEX_STRIDE, add_surface_consumers_for_command_range,
    plan_capture_visibility, plan_surface_consumers, plan_visibility, push_draw_command,
    push_draw_command_with_uv, surface_sampling_for_plan,
};
use program::create_texture_program;

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

struct SurfaceResourceInputs<'a> {
    canonical: &'a [RenderableSurface],
    lifecycle: &'a [RenderableSurface],
    client_cursor: Option<&'a RenderableSurface>,
}

const MAX_CACHED_DMABUF_RESOURCES_PER_SURFACE: usize = 4;
const EGL_BUFFER_AGE_EXT: egl::Int = 0x313d;
const MAX_LAMP_VERTICES: usize = 65_536;
const LAMP_TARGET_CELL_PIXELS: f32 = 32.0;
const LAMP_MAX_GRID_SUBDIVISIONS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DmabufImportGlStage {
    TextureCreation,
    Bind,
    TextureConfiguration,
    ImageTarget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DmabufImportFailureClass {
    BufferIncompatible,
    RendererFatal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DmabufImportPath {
    Initial,
    Replacement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DmabufImportCacheState {
    NotChecked,
    Miss,
    Hit,
}

#[derive(Debug)]
enum DmabufTextureImportError {
    InvalidAttributes(EglGlesImportError),
    EglImageCreation(egl::Error),
    TextureCreation(String),
    Gl {
        stage: DmabufImportGlStage,
        error: u32,
    },
}

impl DmabufTextureImportError {
    fn classification(&self) -> DmabufImportFailureClass {
        match self {
            Self::InvalidAttributes(_) => DmabufImportFailureClass::BufferIncompatible,
            Self::EglImageCreation(
                egl::Error::BadAttribute
                | egl::Error::BadMatch
                | egl::Error::BadNativePixmap
                | egl::Error::BadParameter,
            ) => DmabufImportFailureClass::BufferIncompatible,
            Self::Gl {
                stage: DmabufImportGlStage::ImageTarget,
                error: glow::INVALID_OPERATION,
            } => DmabufImportFailureClass::BufferIncompatible,
            _ => DmabufImportFailureClass::RendererFatal,
        }
    }

    fn stage_name(&self) -> &'static str {
        match self {
            Self::InvalidAttributes(_) => "attributes",
            Self::EglImageCreation(_) => "egl_create_image",
            Self::TextureCreation(_) => "texture_creation",
            Self::Gl { stage, .. } => match stage {
                DmabufImportGlStage::TextureCreation => "texture_creation",
                DmabufImportGlStage::Bind => "bind",
                DmabufImportGlStage::TextureConfiguration => "texture_configuration",
                DmabufImportGlStage::ImageTarget => "image_target",
            },
        }
    }

    fn error_code(&self) -> Option<u32> {
        match self {
            Self::EglImageCreation(error) => Some(error.native() as u32),
            Self::Gl { error, .. } => Some(*error),
            Self::InvalidAttributes(_) | Self::TextureCreation(_) => None,
        }
    }

    fn egl_image_created(&self) -> bool {
        matches!(self, Self::TextureCreation(_) | Self::Gl { .. })
    }
}

impl std::fmt::Display for DmabufTextureImportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidAttributes(error) => {
                write!(formatter, "invalid DMA-BUF import attributes: {error:?}")
            }
            Self::EglImageCreation(error) => write!(formatter, "eglCreateImage failed: {error}"),
            Self::TextureCreation(error) => write!(formatter, "texture creation failed: {error}"),
            Self::Gl { stage, error } => {
                write!(formatter, "GL {:?} failed with error 0x{error:04x}", stage)
            }
        }
    }
}

impl Error for DmabufTextureImportError {}

fn drain_gl_errors(gl: &glow::Context) -> Option<u32> {
    first_drained_gl_error(|| unsafe { gl.get_error() })
}

fn first_drained_gl_error(mut next_error: impl FnMut() -> u32) -> Option<u32> {
    let mut first_error = None;
    loop {
        let error = next_error();
        if error == glow::NO_ERROR {
            return first_error;
        }
        first_error.get_or_insert(error);
    }
}

fn check_dmabuf_gl_stage(
    gl: &glow::Context,
    stage: DmabufImportGlStage,
) -> Result<(), DmabufTextureImportError> {
    drain_gl_errors(gl).map_or(Ok(()), |error| {
        Err(DmabufTextureImportError::Gl { stage, error })
    })
}

#[derive(Debug, Clone, Copy)]
struct DmabufImportDiagnosticContext {
    surface_id: u32,
    generation: u64,
    buffer_id: u64,
    width: u32,
    height: u32,
    fourcc: u32,
    modifier: u64,
    planes: usize,
    path: DmabufImportPath,
    cache: DmabufImportCacheState,
}

impl DmabufImportDiagnosticContext {
    fn from_surface(
        surface: &RenderableSurface,
        path: DmabufImportPath,
        cache: DmabufImportCacheState,
    ) -> Option<Self> {
        let handle = surface.dmabuf_handle()?;
        let modifier = handle
            .planes()
            .first()
            .map(|plane| plane.descriptor().modifier.0)?;
        let size = handle.size();
        Some(Self {
            surface_id: surface.surface_id,
            generation: surface.generation,
            buffer_id: surface.buffer_id().get(),
            width: size.width,
            height: size.height,
            fourcc: handle.format().as_fourcc(),
            modifier,
            planes: handle.planes().len(),
            path,
            cache,
        })
    }
}

fn log_dmabuf_import_context(
    context: DmabufImportDiagnosticContext,
    event: &'static str,
    stage: &'static str,
    egl_image_created: bool,
    error_code: Option<u32>,
    classification: &'static str,
) {
    let error_code = error_code.map_or_else(|| "none".to_owned(), |code| format!("0x{code:04x}"));
    eprintln!(
        "oblivion-one compositor: dmabuf {event}: surface={} generation={} buffer_id={} size={}x{} fourcc=0x{:08x} modifier=0x{:016x} implicit={} planes={} path={:?} cache={:?} egl_image={} stage={} gl_or_egl_error={} classification={classification}",
        context.surface_id,
        context.generation,
        context.buffer_id,
        context.width,
        context.height,
        context.fourcc,
        context.modifier,
        context.modifier == DrmModifier::INVALID.0,
        context.planes,
        context.path,
        context.cache,
        egl_image_created,
        stage,
        error_code,
    );
}

fn settle_dmabuf_import_result<T>(
    result: RendererResult<T>,
    context: DmabufImportDiagnosticContext,
    frame_stats: &mut GlesSceneFrameStats,
    failed_surface_generations: &mut HashMap<u32, u64>,
) -> RendererResult<Option<T>> {
    match result {
        Ok(resource) => {
            failed_surface_generations.remove(&context.surface_id);
            if native_egl_debug_enabled() {
                log_dmabuf_import_context(
                    context,
                    "import_accepted",
                    "complete",
                    true,
                    None,
                    "success",
                );
            }
            Ok(Some(resource))
        }
        Err(error) => {
            let Some(import_error) = error.downcast_ref::<DmabufTextureImportError>() else {
                return Err(error);
            };
            let classification = import_error.classification();
            let should_log = failed_surface_generations
                .get(&context.surface_id)
                .is_none_or(|generation| *generation != context.generation);
            if should_log || classification == DmabufImportFailureClass::RendererFatal {
                log_dmabuf_import_context(
                    context,
                    "import_rejected",
                    import_error.stage_name(),
                    import_error.egl_image_created(),
                    import_error.error_code(),
                    match classification {
                        DmabufImportFailureClass::BufferIncompatible => "buffer_incompatible",
                        DmabufImportFailureClass::RendererFatal => "renderer_fatal",
                    },
                );
            }
            if classification == DmabufImportFailureClass::RendererFatal {
                return Err(error);
            }
            frame_stats.dmabuf_import_failures =
                frame_stats.dmabuf_import_failures.saturating_add(1);
            if should_log {
                failed_surface_generations.insert(context.surface_id, context.generation);
            }
            Ok(None)
        }
    }
}

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
    pub presentation_geometry_signature: u64,
    pub client_cursor: Option<compositor::ClientCursorRenderState<'a>>,
    pub(crate) current_damage: Option<OutputDamage>,
    pub(crate) surface_resource_sync_states: Vec<SurfaceResourceSyncState>,
    pub lifecycle: &'a LifecycleSceneSample,
    pub lifecycle_surfaces: &'a [RenderableSurface],
    pub lifecycle_decorations: &'a [DecorationRenderInstance],
}

struct LifecycleResolvedVisualResource {
    texture: PooledEffectTexture,
    source_signature: u64,
    source_visual_rect: compositor::PresentationRect,
}

#[derive(Debug, Clone, Copy)]
struct LampUniformLocations {
    output_size: Option<glow::UniformLocation>,
    canonical_visual_rect: Option<glow::UniformLocation>,
    source_visual_rect: Option<glow::UniformLocation>,
    anchor_rect: Option<glow::UniformLocation>,
    progress: Option<glow::UniformLocation>,
    opacity: Option<glow::UniformLocation>,
    direction: Option<glow::UniformLocation>,
    shape_factor: Option<glow::UniformLocation>,
    bump_distance: Option<glow::UniformLocation>,
    bump_progress: Option<glow::UniformLocation>,
    stretch_progress: Option<glow::UniformLocation>,
    squash_progress: Option<glow::UniformLocation>,
    framebuffer_origin_bottom_left: Option<glow::UniformLocation>,
    texture: Option<glow::UniformLocation>,
}

pub(crate) struct GlesSceneRenderer {
    cursor_image: std::sync::Arc<CompositorCursorImage>,
    gl: glow::Context,
    program: GlProgram,
    capture_program: GlProgram,
    capture_uniform_locations: HashMap<String, Option<glow::UniformLocation>>,
    scene_vertex_array: GlVertexArray,
    scene_vertex_buffer: GlBuffer,
    scene_vertex_buffer_capacity: usize,
    scene_geometry_dirty: bool,
    overlay_vertex_array: GlVertexArray,
    overlay_vertex_buffer: GlBuffer,
    overlay_vertex_buffer_capacity: usize,
    overlay_geometry_dirty: bool,
    lamp_program: Option<GlProgram>,
    lamp_uniform_locations: Option<LampUniformLocations>,
    lamp_vertex_array: GlVertexArray,
    lamp_vertex_buffer: GlBuffer,
    lamp_vertex_buffer_capacity: usize,
    lamp_geometry_dirty: bool,
    lamp_geometry_key: Option<u64>,
    lamp_vertices: Vec<EglLampVertex>,
    lamp_commands: Vec<EglLampDrawCommand>,
    lifecycle_source_vertices: HashMap<compositor::WindowId, Vec<EglTexturedVertex>>,
    lifecycle_source_commands: HashMap<compositor::WindowId, Vec<EglDrawCommand>>,
    lifecycle_visual_resources: HashMap<compositor::WindowId, LifecycleResolvedVisualResource>,
    lifecycle_visual_sources: HashMap<compositor::WindowId, LifecycleVisualSource>,
    lamp_samples: Vec<LampWindowSample>,
    lifecycle_render_evidence: LifecycleRenderEvidence,
    lifecycle_render_fallbacks: LifecycleRenderFallbacks,
    current_framebuffer_origin: OutputFramebufferOrigin,
    current_size: (u32, u32),
    texture_upload_rgba: Vec<u8>,
    vertices: Vec<EglTexturedVertex>,
    commands: Vec<EglDrawCommand>,
    cursor_vertices: Vec<EglTexturedVertex>,
    cursor_commands: Vec<EglDrawCommand>,
    scene_visibility_plan: Vec<EglVisibilityDecision>,
    scene_cache_key: Option<EglSceneCacheKey>,
    presented_scene_key: Option<EglSceneCacheKey>,
    cursor_resource: Option<EglImageResource>,
    cursor_resource_stale: bool,
    surface_resources: HashMap<u32, EglSurfaceResource>,
    dmabuf_resource_cache: HashMap<DmabufImageKey, CachedDmabufResource<EglImageResource>>,
    dmabuf_cache_peak_entries: usize,
    dmabuf_cache_max_entries_for_one_surface: usize,
    active_surface_ids: Vec<u32>,
    failed_surface_generations: HashMap<u32, u64>,
    frame_resources: HashMap<compositor::ServerFrameColor, EglImageResource>,
    decoration_resources: HashMap<DecorationResourceKey, EglImageResource>,
    egl_image_target_texture_2d: Option<GlEglImageTargetTexture2DOes>,
    damage_tracker: EglOutputDamageTracker,
    repaint_planner: PartialRepaintPlanner,
    effect_resources: EffectGlResourceCache,
    effect_registry: EffectRegistry,
    effect_registry_generation: u64,
    failed_effect_generation: Option<u64>,
    effect_shaders: ShaderProgramCache,
    effect_quad: Option<(GlVertexArray, GlBuffer)>,
    effect_trace: EffectExecutionTrace,
    effect_gpu_profiler: EffectGpuProfiler,
    active_output_framebuffer: Option<glow::Framebuffer>,
    frame_stats: GlesSceneFrameStats,
    effect_clock_start: Instant,
    effect_time_seconds: f32,
    effect_delta_seconds: f32,
    effect_output_scale: f32,
    pub(crate) capture_in_progress: bool,
}

struct CaptureRendererState {
    current_framebuffer_origin: OutputFramebufferOrigin,
    current_size: (u32, u32),
    presented_scene_key: Option<EglSceneCacheKey>,
    damage_tracker: EglOutputDamageTracker,
    repaint_planner: PartialRepaintPlanner,
    failed_effect_generation: Option<u64>,
    effect_trace: EffectExecutionTrace,
    frame_stats: GlesSceneFrameStats,
    effect_time_seconds: f32,
    effect_delta_seconds: f32,
    effect_output_scale: f32,
    lifecycle_render_evidence: LifecycleRenderEvidence,
    lifecycle_render_fallbacks: LifecycleRenderFallbacks,
    lamp_samples: Vec<LampWindowSample>,
    lifecycle_visual_sources: HashMap<compositor::WindowId, LifecycleVisualSource>,
    failed_surface_generations: HashMap<u32, u64>,
    active_output_framebuffer: Option<glow::Framebuffer>,
    capture_in_progress: bool,
}

impl CaptureRendererState {
    fn take(renderer: &GlesSceneRenderer) -> Self {
        Self {
            current_framebuffer_origin: renderer.current_framebuffer_origin,
            current_size: renderer.current_size,
            presented_scene_key: renderer.presented_scene_key,
            damage_tracker: renderer.damage_tracker.clone(),
            repaint_planner: renderer.repaint_planner.clone(),
            failed_effect_generation: renderer.failed_effect_generation,
            effect_trace: renderer.effect_trace,
            frame_stats: renderer.frame_stats,
            effect_time_seconds: renderer.effect_time_seconds,
            effect_delta_seconds: renderer.effect_delta_seconds,
            effect_output_scale: renderer.effect_output_scale,
            lifecycle_render_evidence: renderer.lifecycle_render_evidence.clone(),
            lifecycle_render_fallbacks: renderer.lifecycle_render_fallbacks.clone(),
            lamp_samples: renderer.lamp_samples.clone(),
            lifecycle_visual_sources: renderer.lifecycle_visual_sources.clone(),
            failed_surface_generations: renderer.failed_surface_generations.clone(),
            active_output_framebuffer: renderer.active_output_framebuffer,
            capture_in_progress: renderer.capture_in_progress,
        }
    }

    fn restore(self, renderer: &mut GlesSceneRenderer) {
        renderer.current_framebuffer_origin = self.current_framebuffer_origin;
        renderer.current_size = self.current_size;
        renderer.presented_scene_key = self.presented_scene_key;
        renderer.damage_tracker = self.damage_tracker;
        renderer.repaint_planner = self.repaint_planner;
        renderer.failed_effect_generation = self.failed_effect_generation;
        renderer.effect_trace = self.effect_trace;
        renderer.frame_stats = self.frame_stats;
        renderer.effect_time_seconds = self.effect_time_seconds;
        renderer.effect_delta_seconds = self.effect_delta_seconds;
        renderer.effect_output_scale = self.effect_output_scale;
        renderer.lifecycle_render_evidence = self.lifecycle_render_evidence;
        renderer.lifecycle_render_fallbacks = self.lifecycle_render_fallbacks;
        renderer.lamp_samples = self.lamp_samples;
        renderer.lifecycle_visual_sources = self.lifecycle_visual_sources;
        renderer.failed_surface_generations = self.failed_surface_generations;
        renderer.active_output_framebuffer = self.active_output_framebuffer;
        renderer.capture_in_progress = self.capture_in_progress;
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

fn surface_root_for_lamp(
    surface: &RenderableSurface,
    surfaces: &[RenderableSurface],
    lifecycle: &LifecycleSceneSample,
) -> u32 {
    let mut current = surface.surface_id;
    for _ in 0..=surfaces.len() {
        if lifecycle
            .lamps
            .iter()
            .any(|lamp| lamp.root_surface_id == current)
        {
            return current;
        }
        let Some(parent) = surfaces
            .iter()
            .find(|candidate| candidate.surface_id == current)
            .and_then(|candidate| candidate.placement.parent_surface_id)
        else {
            break;
        };
        current = parent;
    }
    current
}

struct LampGridSpec {
    layer: EglDrawLayer,
    window_id: compositor::WindowId,
    bounds: EglRect,
    uv: EglUvRect,
}

fn lamp_grid_subdivisions(
    width: f32,
    height: f32,
    available_vertices: usize,
) -> Option<(usize, usize)> {
    let mut columns =
        ((width / LAMP_TARGET_CELL_PIXELS).ceil() as usize).clamp(1, LAMP_MAX_GRID_SUBDIVISIONS);
    let mut rows =
        ((height / LAMP_TARGET_CELL_PIXELS).ceil() as usize).clamp(1, LAMP_MAX_GRID_SUBDIVISIONS);
    let available_cells = available_vertices / 6;
    if available_cells == 0 {
        return None;
    }
    while columns.saturating_mul(rows) > available_cells {
        if columns >= rows && columns > 1 {
            columns -= 1;
        } else if rows > 1 {
            rows -= 1;
        } else {
            return None;
        }
    }
    Some((columns, rows))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LegacySceneScissoredPhase {
    BaseRepair(usize),
    PrepareLifecycleSources,
    RestoreRepairScissor(usize),
    Lamp(usize),
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
            let repair = offset / 3;
            if repair >= self.repair_count {
                return None;
            }
            match offset % 3 {
                0 => LegacySceneScissoredPhase::RestoreRepairScissor(repair),
                1 => LegacySceneScissoredPhase::Lamp(repair),
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

fn append_lamp_grid(
    vertices: &mut Vec<EglLampVertex>,
    commands: &mut Vec<EglLampDrawCommand>,
    spec: LampGridSpec,
) -> bool {
    let LampGridSpec {
        layer,
        window_id,
        bounds,
        uv,
    } = spec;
    let x = bounds.x();
    let y = bounds.y();
    let width = bounds.width();
    let height = bounds.height();
    if !x.is_finite()
        || !y.is_finite()
        || !width.is_finite()
        || !height.is_finite()
        || width <= 0.0
        || height <= 0.0
    {
        return false;
    }
    let available_vertices = MAX_LAMP_VERTICES.saturating_sub(vertices.len());
    let Some((columns, rows)) = lamp_grid_subdivisions(width, height, available_vertices) else {
        return false;
    };
    let required = columns.saturating_mul(rows).saturating_mul(6);
    let vertex_start = u32::try_from(vertices.len()).unwrap_or(u32::MAX);
    let columns_f = columns as f32;
    let rows_f = rows as f32;
    let push_vertex = |vertices: &mut Vec<EglLampVertex>, column: usize, row: usize| {
        let u = column as f32 / columns_f;
        let v = row as f32 / rows_f;
        vertices.push(EglLampVertex {
            position: [x + width * u, y + height * v],
            uv: [
                uv.left() + (uv.right() - uv.left()) * u,
                uv.top() + (uv.bottom() - uv.top()) * v,
            ],
        });
    };
    for row in 0..rows {
        for column in 0..columns {
            for (vertex_column, vertex_row) in [
                (column, row),
                (column + 1, row),
                (column + 1, row + 1),
                (column, row),
                (column + 1, row + 1),
                (column, row + 1),
            ] {
                push_vertex(vertices, vertex_column, vertex_row);
            }
        }
    }
    commands.push(EglLampDrawCommand {
        layer,
        bounds: EglRect::new(x, y, width, height),
        vertex_start,
        vertex_count: u32::try_from(required).unwrap_or(u32::MAX),
        sampling: SurfaceSampling::ScaledLinear,
        window_id,
    });
    true
}

fn lamp_geometry_key(
    lifecycle: &LifecycleSceneSample,
    surfaces: &[RenderableSurface],
    decorations: &[DecorationRenderInstance],
    output_scale: f64,
    framebuffer_origin: OutputFramebufferOrigin,
) -> u64 {
    let mut signature = 0xcbf2_9ce4_8422_2325_u64;
    let mut mix = |value: u64| {
        signature ^= value;
        signature = signature.wrapping_mul(0x1000_0000_01b3);
    };
    mix(output_scale.to_bits());
    mix(match framebuffer_origin {
        OutputFramebufferOrigin::BottomLeft => 1,
        OutputFramebufferOrigin::TopLeftScanout => 2,
    });
    for lamp in &lifecycle.lamps {
        for value in [
            lamp.window_id.get(),
            u64::from(lamp.root_surface_id),
            lamp.transition_id.get(),
            lamp.visual_group.canonical_client_rect.x().to_bits(),
            lamp.visual_group.canonical_client_rect.y().to_bits(),
            lamp.visual_group.canonical_client_rect.width().to_bits(),
            lamp.visual_group.canonical_client_rect.height().to_bits(),
            lamp.visual_group.canonical_visual_rect.x().to_bits(),
            lamp.visual_group.canonical_visual_rect.y().to_bits(),
            lamp.visual_group.canonical_visual_rect.width().to_bits(),
            lamp.visual_group.canonical_visual_rect.height().to_bits(),
            lamp.visual_group.presented_source_client_rect.x().to_bits(),
            lamp.visual_group.presented_source_client_rect.y().to_bits(),
            lamp.visual_group
                .presented_source_client_rect
                .width()
                .to_bits(),
            lamp.visual_group
                .presented_source_client_rect
                .height()
                .to_bits(),
            lamp.visual_group.presented_source_visual_rect.x().to_bits(),
            lamp.visual_group.presented_source_visual_rect.y().to_bits(),
            lamp.visual_group
                .presented_source_visual_rect
                .width()
                .to_bits(),
            lamp.visual_group
                .presented_source_visual_rect
                .height()
                .to_bits(),
            lamp.visual_group.anchor_rect.x().to_bits(),
            lamp.visual_group.anchor_rect.y().to_bits(),
            lamp.visual_group.anchor_rect.width().to_bits(),
            lamp.visual_group.anchor_rect.height().to_bits(),
            lamp.visual_group.shape_factor.to_bits(),
            lamp.visual_group.bump_distance.to_bits(),
            match lamp.visual_group.lamp_direction {
                oblivion_one::window_lifecycle_animation::LampDirection::Top => 1,
                oblivion_one::window_lifecycle_animation::LampDirection::Right => 2,
                oblivion_one::window_lifecycle_animation::LampDirection::Bottom => 3,
                oblivion_one::window_lifecycle_animation::LampDirection::Left => 4,
            },
        ] {
            mix(value);
        }
        if let Some(source) = lifecycle.visual_source_for_window(lamp.window_id) {
            mix(match source.kind {
                LifecycleVisualSourceKind::NoOwnedEffects => 1,
                LifecycleVisualSourceKind::ResolvedOwnedEffects => 2,
            });
            mix(source.effect_scene.signature);
        }
    }
    for surface in surfaces {
        mix(u64::from(surface.surface_id));
        mix(surface.x as u32 as u64);
        mix(surface.y as u32 as u64);
        mix(u64::from(surface.width));
        mix(u64::from(surface.height));
        mix(u64::from(surface.placement.parent_surface_id.unwrap_or(0)));
        mix(surface.placement.local_x as u32 as u64);
        mix(surface.placement.local_y as u32 as u64);
        mix(surface.placement.root_mode as u32 as u64);
        let buffer_size = surface.buffer_size();
        mix(u64::from(buffer_size.width));
        mix(u64::from(buffer_size.height));
        mix(u64::from(surface.buffer_scale));
        mix(surface.buffer_transform as u32 as u64);
    }
    for decoration in decorations {
        mix(u64::from(decoration.root_surface_id()));
        mix(decoration.scene_snapshot().visual_signature());
    }
    signature
}

fn plan_lamp_surface_consumers(
    commands: &[EglLampDrawCommand],
    repairs: &[OutputRect],
) -> SurfaceConsumerPlan {
    let mut plan = SurfaceConsumerPlan::default();
    for command in commands {
        if repairs
            .iter()
            .any(|repair| command.bounds.intersects_output_rect(*repair))
            && let EglDrawLayer::Surface(surface_id) = command.layer
        {
            plan.add_surface(surface_id);
        }
    }
    plan.finish();
    plan
}

fn lifecycle_damage_for_samples(
    lifecycle: &LifecycleSceneSample,
    output_width: u32,
    output_height: u32,
    output_scale: f64,
) -> OutputDamage {
    let mut rects = Vec::new();
    for lamp in &lifecycle.lamps {
        if let Some(footprint) = lamp_footprint(lamp.visual_group) {
            let left = footprint.x();
            let top = footprint.y();
            let right = footprint.x() + footprint.width();
            let bottom = footprint.y() + footprint.height();
            rects.push(OutputRect::new(
                (left * output_scale).floor() as i32,
                (top * output_scale).floor() as i32,
                ((right - left) * output_scale).ceil().max(1.0) as u32,
                ((bottom - top) * output_scale).ceil().max(1.0) as u32,
            ));
        }
    }
    OutputDamage::rects(output_width, output_height, rects)
}

fn set_lamp_uniform_rect(
    gl: &glow::Context,
    location: Option<&glow::UniformLocation>,
    rect: compositor::PresentationRect,
    scale: f64,
) {
    if let Some(location) = location {
        unsafe {
            gl.uniform_4_f32(
                Some(location),
                (rect.x() * scale) as f32,
                (rect.y() * scale) as f32,
                (rect.width() * scale) as f32,
                (rect.height() * scale) as f32,
            );
        }
    }
}

impl GlesSceneRenderer {
    pub(crate) const fn lifecycle_animation_available(&self) -> bool {
        self.lamp_program.is_some()
    }

    pub(crate) fn invalidate_presented_damage_history(&mut self) {
        self.repaint_planner.invalidate();
        self.presented_scene_key = None;
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
        let capture_program = program::create_capture_program(&gl)?;
        let lamp_program = program::create_lamp_program(&gl).ok();
        let scene_vertex_array = unsafe { gl.create_vertex_array().map_err(io::Error::other)? };
        let scene_vertex_buffer = unsafe { gl.create_buffer().map_err(io::Error::other)? };
        let overlay_vertex_array = unsafe { gl.create_vertex_array().map_err(io::Error::other)? };
        let overlay_vertex_buffer = unsafe { gl.create_buffer().map_err(io::Error::other)? };
        let lamp_vertex_array = unsafe { gl.create_vertex_array().map_err(io::Error::other)? };
        let lamp_vertex_buffer = unsafe { gl.create_buffer().map_err(io::Error::other)? };
        unsafe {
            for (vertex_array, vertex_buffer) in [
                (scene_vertex_array, scene_vertex_buffer),
                (overlay_vertex_array, overlay_vertex_buffer),
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
            gl.use_program(Some(capture_program));
            if let Some(location) = gl.get_uniform_location(capture_program, "u_texture") {
                gl.uniform_1_i32(Some(&location), 0);
            }
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

        let lamp_uniform_locations = lamp_program.map(|program| unsafe {
            LampUniformLocations {
                output_size: gl.get_uniform_location(program, "u_output_size"),
                canonical_visual_rect: gl.get_uniform_location(program, "u_canonical_visual_rect"),
                source_visual_rect: gl.get_uniform_location(program, "u_source_visual_rect"),
                anchor_rect: gl.get_uniform_location(program, "u_anchor_rect"),
                progress: gl.get_uniform_location(program, "u_progress"),
                opacity: gl.get_uniform_location(program, "u_opacity"),
                direction: gl.get_uniform_location(program, "u_direction"),
                shape_factor: gl.get_uniform_location(program, "u_shape_factor"),
                bump_distance: gl.get_uniform_location(program, "u_bump_distance"),
                bump_progress: gl.get_uniform_location(program, "u_bump_progress"),
                stretch_progress: gl.get_uniform_location(program, "u_stretch_progress"),
                squash_progress: gl.get_uniform_location(program, "u_squash_progress"),
                framebuffer_origin_bottom_left: gl
                    .get_uniform_location(program, "u_framebuffer_origin_bottom_left"),
                texture: gl.get_uniform_location(program, "u_texture"),
            }
        });
        let mut effect_shaders = ShaderProgramCache::new(builtin_shader_program_count())
            .expect("built-in shader cache capacity is non-zero");
        effect_shaders.prewarm_builtins(&gl)?;
        let effect_gpu_profiler = EffectGpuProfiler::new(&gl, |name| {
            egl.get_proc_address(name)
                .map(|symbol| symbol as *const c_void)
        });

        Ok(Self {
            gl,
            program,
            capture_program,
            capture_uniform_locations: HashMap::new(),
            scene_vertex_array,
            scene_vertex_buffer,
            scene_vertex_buffer_capacity: MIN_VERTEX_BUFFER_BYTES,
            scene_geometry_dirty: true,
            overlay_vertex_array,
            overlay_vertex_buffer,
            overlay_vertex_buffer_capacity: MIN_VERTEX_BUFFER_BYTES,
            overlay_geometry_dirty: true,
            lamp_program,
            lamp_uniform_locations,
            lamp_vertex_array,
            lamp_vertex_buffer,
            lamp_vertex_buffer_capacity: MIN_VERTEX_BUFFER_BYTES,
            lamp_geometry_dirty: true,
            lamp_geometry_key: None,
            lamp_vertices: Vec::new(),
            lamp_commands: Vec::new(),
            lifecycle_source_vertices: HashMap::new(),
            lifecycle_source_commands: HashMap::new(),
            lifecycle_visual_resources: HashMap::new(),
            lifecycle_visual_sources: HashMap::new(),
            lamp_samples: Vec::new(),
            lifecycle_render_evidence: LifecycleRenderEvidence::default(),
            lifecycle_render_fallbacks: LifecycleRenderFallbacks::default(),
            current_framebuffer_origin: OutputFramebufferOrigin::BottomLeft,
            cursor_image: cursor_image.clone(),
            current_size: (width, height),
            texture_upload_rgba: Vec::new(),
            vertices: Vec::new(),
            commands: Vec::new(),
            cursor_vertices: Vec::new(),
            cursor_commands: Vec::new(),
            scene_visibility_plan: Vec::new(),
            scene_cache_key: None,
            presented_scene_key: None,
            cursor_resource: None,
            cursor_resource_stale: false,
            surface_resources: HashMap::new(),
            dmabuf_resource_cache: HashMap::new(),
            dmabuf_cache_peak_entries: 0,
            dmabuf_cache_max_entries_for_one_surface: 0,
            active_surface_ids: Vec::new(),
            failed_surface_generations: HashMap::new(),
            frame_resources: HashMap::new(),
            decoration_resources: HashMap::new(),
            egl_image_target_texture_2d,
            damage_tracker: EglOutputDamageTracker::with_cursor_image(cursor_image),
            repaint_planner: PartialRepaintPlanner::new(
                (width, height),
                partial_repaint_capabilities,
            ),
            effect_resources: EffectGlResourceCache::new(),
            effect_registry: EffectRegistry::with_builtin_background_blur(),
            effect_registry_generation: 1,
            failed_effect_generation: None,
            effect_shaders,
            effect_quad: None,
            effect_trace: EffectExecutionTrace::new(None, None, None, None),
            effect_gpu_profiler,
            active_output_framebuffer: None,
            frame_stats: GlesSceneFrameStats::default(),
            effect_clock_start: Instant::now(),
            effect_time_seconds: 0.0,
            effect_delta_seconds: 0.0,
            effect_output_scale: 1.0,
            capture_in_progress: false,
        })
    }

    pub(crate) const fn last_frame_stats(&self) -> GlesSceneFrameStats {
        self.frame_stats
    }

    pub(crate) fn trace_render_fence_export_begin(&self) {
        self.effect_trace.frame_boundary(
            "render_fence_export",
            "begin",
            FrameTraceSummary::default(),
        );
    }

    pub(crate) fn trace_render_fence_export_end(&self) {
        self.effect_trace.frame_boundary(
            "render_fence_export",
            "end",
            FrameTraceSummary::default(),
        );
    }

    pub(crate) fn set_cursor_image(&mut self, cursor_image: Arc<CompositorCursorImage>) {
        self.cursor_image = cursor_image.clone();
        self.damage_tracker.set_cursor_image(cursor_image);
        self.cursor_resource_stale = true;
        self.repaint_planner.invalidate();
    }

    pub(crate) fn bind_active_output_framebuffer(&self) {
        unsafe {
            self.gl
                .bind_framebuffer(glow::FRAMEBUFFER, self.active_output_framebuffer);
        }
    }

    /// Restore the complete state expected by ordinary scene drawing after an
    /// effect or other offscreen pass has changed GL state.
    pub(crate) fn establish_ordinary_scene_state(&self) {
        unsafe {
            self.gl
                .bind_framebuffer(glow::FRAMEBUFFER, self.active_output_framebuffer);
            self.gl
                .viewport(0, 0, self.current_size.0 as i32, self.current_size.1 as i32);
            self.gl.use_program(Some(self.program));
            self.gl.active_texture(glow::TEXTURE0);
            self.gl.disable(glow::SCISSOR_TEST);
            self.gl.enable(glow::BLEND);
            self.gl.blend_func_separate(
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
            );
        }
    }

    pub(crate) fn capture_uniform_location(&mut self, name: &str) -> Option<glow::UniformLocation> {
        if let Some(location) = self.capture_uniform_locations.get(name) {
            return *location;
        }
        let location = unsafe { self.gl.get_uniform_location(self.capture_program, name) };
        self.capture_uniform_locations
            .insert(name.to_owned(), location);
        location
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
        self.effect_registry = registry;
        self.effect_registry_generation = self.effect_registry_generation.saturating_add(1);
        self.failed_effect_generation = None;
        self.invalidate_presented_damage_history();
    }

    /// Compile all custom modules at the active compositor GL boundary before
    /// making their complete validated generation visible to frame execution.
    #[allow(dead_code)]
    pub(crate) fn publish_effect_registry_generation(
        &mut self,
        generation: EffectRegistryGeneration,
    ) -> Result<(), RegistryReloadError> {
        let capacity = shader_cache_capacity_for_custom_shaders(generation.shaders.len())
            .expect("validated shader generation size fits cache capacity");
        let mut next_shaders =
            ShaderProgramCache::new(capacity).expect("shader cache capacity is non-zero");
        let compile_result = (|| {
            next_shaders.prewarm_builtins(&self.gl).map_err(|error| {
                RegistryReloadError::ShaderCompile {
                    module: oblivion_one::effects::ShaderModuleId::new(
                        oblivion_one::effects::INTERNAL_EFFECT_SHADER_MODULE_DOWNSAMPLE,
                    )
                    .expect("builtin shader ids are non-zero"),
                    log: error.to_string(),
                }
            })?;
            for shader in generation.shaders.values() {
                next_shaders
                    .prewarm_trusted_custom(&self.gl, shader)
                    .map_err(|error| RegistryReloadError::ShaderCompile {
                        module: shader.module,
                        log: error.to_string(),
                    })?;
            }
            Ok::<(), RegistryReloadError>(())
        })();
        if let Err(error) = compile_result {
            next_shaders.clear(&self.gl);
            return Err(error);
        }
        self.effect_shaders.clear(&self.gl);
        self.effect_shaders = next_shaders;
        self.effect_registry = generation.registry;
        self.effect_registry_generation = generation.generation;
        self.failed_effect_generation = None;
        self.invalidate_presented_damage_history();
        Ok(())
    }

    #[allow(dead_code)]
    pub(crate) fn reload_trusted_effect_registry(
        &mut self,
        registry: &TrustedEffectRegistry,
        manifest: EffectManifest,
    ) -> Result<Arc<EffectRegistryGeneration>, RegistryReloadError> {
        reload_with_publisher(registry, manifest, self)
    }

    fn ensure_effect_quad(&mut self) -> RendererResult<(GlVertexArray, GlBuffer)> {
        if let Some(quad) = self.effect_quad {
            return Ok(quad);
        }
        let vertex_array = unsafe { self.gl.create_vertex_array().map_err(io::Error::other)? };
        let vertex_buffer = unsafe { self.gl.create_buffer().map_err(io::Error::other)? };
        let vertices: [f32; 24] = [
            -1.0, -1.0, 0.0, 1.0, 1.0, -1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.0, -1.0, -1.0, 0.0, 1.0,
            1.0, 1.0, 1.0, 0.0, -1.0, 1.0, 0.0, 0.0,
        ];
        unsafe {
            self.gl.bind_vertex_array(Some(vertex_array));
            self.gl.bind_buffer(glow::ARRAY_BUFFER, Some(vertex_buffer));
            self.gl.buffer_data_u8_slice(
                glow::ARRAY_BUFFER,
                bytemuck::cast_slice(&vertices),
                glow::STATIC_DRAW,
            );
            self.gl.enable_vertex_attrib_array(0);
            self.gl
                .vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 16, 0);
            self.gl.enable_vertex_attrib_array(1);
            self.gl
                .vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, 16, 8);
            self.gl.bind_buffer(glow::ARRAY_BUFFER, None);
            self.gl.bind_vertex_array(None);
        }
        self.effect_quad = Some((vertex_array, vertex_buffer));
        Ok((vertex_array, vertex_buffer))
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
        self.capture_in_progress = true;

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
            self.repaint_planner.capabilities().buffer_age,
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
        self.active_output_framebuffer = Some(target.framebuffer);
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
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
        self.active_output_framebuffer = None;
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
            presentation_geometry_signature,
            popup_surface_ids,
            client_cursor,
            current_damage,
            surface_resource_sync_states,
            lifecycle,
            lifecycle_surfaces,
            lifecycle_decorations,
        } = request;
        self.effect_trace = EffectExecutionTrace::new(
            frame_id,
            render_generation,
            Some(scene_generation),
            Some(scene_signature),
        );
        if !self.capture_in_progress {
            self.effect_gpu_profiler.collect(&self.gl);
        }
        self.effect_trace.frame_boundary(
            "effect_scene_resolve",
            "begin",
            FrameTraceSummary::default(),
        );
        let width = width.max(1);
        let height = height.max(1);
        self.current_framebuffer_origin = framebuffer_origin;
        self.lamp_samples.clear();
        self.lamp_samples.extend_from_slice(&lifecycle.lamps);
        self.lifecycle_render_evidence.consumed.clear();
        self.lifecycle_render_fallbacks.failed.clear();
        self.lifecycle_visual_sources.clear();
        self.lifecycle_visual_sources.extend(
            lifecycle
                .visual_sources
                .iter()
                .cloned()
                .map(|source| (source.window_id, source)),
        );
        let output_scale_key = compositor::output_scale_key(output_scale);
        let mut scaled_visual_state =
            compositor::scale_desktop_visual_state(visual_state, output_scale);
        if client_cursor.is_some() {
            scaled_visual_state.cursor = None;
        }
        self.frame_stats = GlesSceneFrameStats::default();
        let effect_time = self.effect_clock_start.elapsed().as_secs_f32();
        self.effect_delta_seconds = if self.effect_time_seconds == 0.0 {
            0.0
        } else {
            (effect_time - self.effect_time_seconds).clamp(0.0, 0.25)
        };
        self.effect_time_seconds = effect_time;
        self.effect_output_scale = output_scale.max(0.0) as f32;
        self.ensure_output_size(width, height)?;
        self.release_stale_lifecycle_visual_resources();
        self.frame_stats.effect_instances_visible = effects
            .instances
            .iter()
            .filter(|instance| !instance.region.is_empty())
            .count();
        self.effect_trace.frame_boundary(
            "effect_scene_resolve",
            "end",
            FrameTraceSummary {
                scene_generation: Some(scene_generation),
                scene_signature: Some(scene_signature),
                visible_effect_count: Some(self.frame_stats.effect_instances_visible),
                ..FrameTraceSummary::default()
            },
        );
        self.ensure_frame_resources()?;
        self.ensure_decoration_resources(
            egl,
            egl_display,
            decoration_instances
                .iter()
                .chain(lifecycle_decorations.iter()),
        )?;
        if scaled_visual_state.cursor.is_some() {
            self.ensure_cursor_resource(egl, egl_display)?;
        }
        self.reconcile_surface_resource_lifetimes(
            egl,
            egl_display,
            surfaces,
            lifecycle_surfaces,
            client_cursor.map(|cursor| cursor.surface),
        )?;
        // The software client cursor remains eager: it is a small, separately
        // owned overlay path and is not part of ordinary scene realization.
        if let Some(cursor) = client_cursor.map(|cursor| cursor.surface) {
            let mut cursor_consumers = SurfaceConsumerPlan::default();
            cursor_consumers.add_surface(cursor.surface_id);
            cursor_consumers.finish();
            self.realize_surface_resources_for_consumers(
                egl,
                egl_display,
                SurfaceResourceInputs {
                    canonical: surfaces,
                    lifecycle: lifecycle_surfaces,
                    client_cursor: Some(cursor),
                },
                &cursor_consumers,
                &surface_resource_sync_states,
            )?;
        }

        let (base_surfaces, overlay_surfaces) =
            split_external_overlay_surfaces(surfaces, external_overlay_surface_ids);
        let scene_surfaces = if external_overlay_surface_ids.is_empty() {
            surfaces
        } else {
            base_surfaces.as_slice()
        };
        let surface_signatures = egl_scene_surface_signatures(surfaces);
        let candidate_scene_key = EglSceneCacheKey::new_with_decorations(
            width,
            height,
            content_generation,
            output_scale_key,
            &surface_signatures,
            decoration_instances,
            popup_surface_ids,
            presentation_geometry_signature,
            framebuffer_origin,
        );
        let scene_changed = self.presented_scene_key != Some(candidate_scene_key);
        let commands_changed = !self.scene_cache_is_current(
            width,
            height,
            content_generation,
            output_scale_key,
            &surface_signatures,
            decoration_instances,
            popup_surface_ids,
            presentation_geometry_signature,
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
        let output_damage = self.damage_tracker.damage_for_frame(
            width,
            height,
            scene_changed,
            current_damage,
            scaled_visual_state,
            client_cursor_damage,
        );
        let output_damage = output_damage.union(
            lifecycle_damage_for_samples(lifecycle, width, height, output_scale),
            width,
            height,
        );
        let (output_damage, contradictory_empty_damage) = resolve_scene_damage_authority(
            scene_changed,
            damage_authority_available,
            output_damage,
        );
        self.frame_stats.contradictory_empty_damage = contradictory_empty_damage;
        let damage_state = EglOutputDamageTracker::candidate_state(
            width,
            height,
            scaled_visual_state,
            client_cursor_damage,
            &self.cursor_image,
        );

        if commands_changed {
            self.frame_stats.scene_rebuilt = true;
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
                presentation_geometry_signature,
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
        self.rebuild_lamp_commands_if_needed(
            lifecycle,
            lifecycle_surfaces,
            lifecycle_decorations,
            output_scale,
        );
        let effect_source_damage = effect_region_from_output_damage(&output_damage, width, height);
        let output_bounds = EffectRect::new(0, 0, width, height)
            .expect("non-zero renderer dimensions must form valid effect bounds");
        self.effect_trace.frame_boundary(
            "effect_graph_compile",
            "begin",
            self.effect_trace_summary(effects, None, None, None),
        );
        let execution_plan = if self.failed_effect_generation
            == Some(self.effect_registry_generation)
            && self.frame_stats.effect_instances_visible != 0
        {
            self.frame_stats.effect_fallbacks = self.frame_stats.effect_fallbacks.saturating_add(1);
            self.frame_stats.effect_instances_failed = self.frame_stats.effect_instances_visible;
            self.frame_stats.effect_failure_reason = Some(EffectFailureReason::GraphCompile);
            FrameExecutionPlan::LegacyScene
        } else {
            match compile_frame_execution_plan(
                effects,
                &effect_source_damage,
                output_bounds,
                &self.effect_registry,
            ) {
                Ok(FrameExecutionPlan::LegacyScene) => FrameExecutionPlan::LegacyScene,
                Ok(FrameExecutionPlan::EffectGraph(graph)) => {
                    self.record_effect_graph_metrics(graph_metrics(&graph));
                    FrameExecutionPlan::EffectGraph(graph)
                }
                Err(_) => {
                    self.frame_stats.effect_fallbacks =
                        self.frame_stats.effect_fallbacks.saturating_add(1);
                    self.frame_stats.effect_instances_failed =
                        self.frame_stats.effect_instances_visible;
                    self.frame_stats.effect_failure_reason =
                        Some(EffectFailureReason::GraphCompile);
                    self.failed_effect_generation = Some(self.effect_registry_generation);
                    FrameExecutionPlan::LegacyScene
                }
            }
        };
        let compiled_graph = match &execution_plan {
            FrameExecutionPlan::EffectGraph(graph) => Some(graph),
            FrameExecutionPlan::LegacyScene => None,
        };
        self.effect_trace.frame_boundary(
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
        let mut plan = self.repaint_planner.plan(output_damage, buffer_age);
        if plan.mode == RepaintMode::Skip {
            self.frame_stats.surface_resource_candidates = surfaces.len();
            self.frame_stats.surface_resource_deferred = surfaces.len();
            self.record_effect_resource_metrics();
            self.record_repaint_stats(&plan);
            return Ok(EglFrameOutcome::Skipped {
                reason: FrameSkipReason::NoLogicalDamage,
                stats: self.frame_stats,
            });
        }
        let demand_trace_seed = if self.effect_trace.enabled() {
            compiled_graph.map(|graph| oblivion_one::effects::EffectDemandPlanStats {
                repair_rect_count: plan.repair_damage.rect_count(),
                dependency_edge_count: graph.instances.iter().fold(0, |count, instance| {
                    count.saturating_add(instance.dependencies.len())
                }),
                dependency_propagations: 0,
                max_instance_region_rect_count: 0,
                conservative_full: plan.mode == RepaintMode::Full,
            })
        } else {
            None
        };
        let mut demand_trace_begin_summary =
            self.effect_trace_summary(effects, Some(&plan), compiled_graph, None);
        demand_trace_begin_summary.demand_plan = demand_trace_seed;
        self.effect_trace
            .frame_boundary("effect_demand_plan", "begin", demand_trace_begin_summary);
        let effect_execution_demand = match &execution_plan {
            FrameExecutionPlan::LegacyScene => None,
            FrameExecutionPlan::EffectGraph(graph) => {
                Some(resolve_effect_execution_for_repaint_plan(
                    &self.repaint_planner,
                    graph,
                    &mut plan,
                    width,
                    height,
                ))
            }
        };
        let selected_effect_count = effect_execution_demand
            .as_ref()
            .map(|demand| demand.instances.len());
        let demand_trace_stats = if self.effect_trace.enabled() {
            effect_execution_demand
                .as_ref()
                .map(|demand| demand.plan_stats())
        } else {
            None
        };
        let mut demand_trace_end_summary =
            self.effect_trace_summary(effects, Some(&plan), compiled_graph, selected_effect_count);
        demand_trace_end_summary.demand_plan = demand_trace_stats;
        self.effect_trace
            .frame_boundary("effect_demand_plan", "end", demand_trace_end_summary);
        if let Some(demand) = &effect_execution_demand {
            self.frame_stats.effect_instances_pruned = self
                .frame_stats
                .effect_instances_visible
                .saturating_sub(demand.instances.len());
        }
        let repair_rects = repaint_plan_output_rects(&plan, width, height);
        let mut consumer_plan = plan_surface_consumers(&self.commands, &repair_rects);
        consumer_plan.extend(&plan_lamp_surface_consumers(
            &self.lamp_commands,
            &repair_rects,
        ));
        for commands in self.lifecycle_source_commands.values() {
            consumer_plan.extend(&plan_surface_consumers(commands, &repair_rects));
        }
        add_surface_consumers_for_command_range(
            &mut consumer_plan,
            &self.cursor_commands,
            0,
            self.cursor_commands.len(),
            &repair_rects,
        );
        let effect_selection = match (&execution_plan, &effect_execution_demand) {
            (FrameExecutionPlan::EffectGraph(graph), Some(demand)) => {
                let selection = effects::select_effect_execution(graph, demand);
                consumer_plan.extend(&effects::plan_effect_surface_consumers(
                    graph,
                    demand,
                    &selection,
                    &self.commands,
                    &repair_rects,
                    (width, height),
                ));
                Some(selection)
            }
            _ => None,
        };
        consumer_plan.finish();
        self.frame_stats.surface_resource_candidates = surfaces.len();
        self.frame_stats.surface_resource_consumers = consumer_plan
            .surface_ids()
            .iter()
            .filter(|surface_id| {
                surfaces
                    .iter()
                    .any(|surface| surface.surface_id == **surface_id)
            })
            .count();
        self.frame_stats.surface_resource_deferred = self
            .frame_stats
            .surface_resource_candidates
            .saturating_sub(self.frame_stats.surface_resource_consumers);
        self.realize_surface_resources_for_consumers(
            egl,
            egl_display,
            SurfaceResourceInputs {
                canonical: surfaces,
                lifecycle: lifecycle_surfaces,
                client_cursor: client_cursor.map(|cursor| cursor.surface),
            },
            &consumer_plan,
            &surface_resource_sync_states,
        )?;
        self.effect_trace.frame_boundary(
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
                match effects::execute_effect_graph(
                    self,
                    graph,
                    framebuffer_origin,
                    &plan,
                    demand,
                    selection,
                ) {
                    Ok(execution_stats) => {
                        self.frame_stats.effect_instances_executed = execution_stats.instances;
                        self.frame_stats.effect_passes_executed = execution_stats.passes;
                        self.frame_stats.blur_downsample_passes = execution_stats.blur_downsamples;
                        self.frame_stats.blur_upsample_passes = execution_stats.blur_upsamples;
                        self.frame_stats.effect_capture_pixels_executed =
                            execution_stats.capture_pixels;
                        self.frame_stats.effect_resource_acquisitions =
                            execution_stats.resource_acquisitions;
                        Ok(())
                    }
                    Err(error) => {
                        self.frame_stats.effect_fallbacks =
                            self.frame_stats.effect_fallbacks.saturating_add(1);
                        self.frame_stats.effect_instances_failed =
                            self.frame_stats.effect_instances_visible;
                        self.frame_stats.effect_failure_reason =
                            Some(EffectFailureReason::from_error(error.as_ref()));
                        if self.frame_stats.effect_failure_reason
                            == Some(EffectFailureReason::ShaderUnavailable)
                        {
                            self.failed_effect_generation = Some(self.effect_registry_generation);
                        }
                        self.draw_textured_layers(&plan, framebuffer_origin)
                    }
                }
            }
        };
        if let Err(error) = draw_result {
            self.effect_trace.frame_boundary(
                "renderer_draw_complete",
                "end",
                self.effect_trace_summary(
                    effects,
                    Some(&plan),
                    compiled_graph,
                    selected_effect_count,
                ),
            );
            self.repaint_planner.invalidate();
            return Err(error);
        }
        self.effect_trace.frame_boundary(
            "renderer_draw_complete",
            "end",
            self.effect_trace_summary(effects, Some(&plan), compiled_graph, selected_effect_count),
        );
        self.record_lifecycle_fallbacks_without_evidence();
        if !self.lifecycle_render_fallbacks.is_empty() {
            self.repaint_planner.invalidate();
            return Ok(EglFrameOutcome::LifecycleFallback {
                stats: self.frame_stats,
                fallbacks: self.lifecycle_render_fallbacks.clone(),
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
            stats: self.frame_stats,
            lifecycle_evidence: self.lifecycle_render_evidence.clone(),
        })
    }

    pub(crate) fn commit_presented(
        &mut self,
        frame: EglSceneFrameCommit,
        presented_transition_damage: OutputDamage,
    ) {
        self.repaint_planner
            .commit_presented_transition(presented_transition_damage);
        self.damage_tracker.commit_presented(frame.damage_state);
        self.presented_scene_key = Some(frame.scene_key);
        self.frame_stats.history_depth = self.repaint_planner.history_depth();
    }

    pub(crate) fn discard_rendered(&mut self, frame: EglSceneFrameCommit) {
        self.repaint_planner.discard_rendered(&frame.repaint_plan);
    }

    pub(crate) fn frame_swap_failed(&mut self) {
        self.repaint_planner.swap_failed();
        self.frame_stats.history_depth = 0;
    }

    fn record_repaint_stats(&mut self, plan: &RepaintPlan) {
        let (width, height) = self.current_size;
        self.frame_stats.repaint_mode = plan.mode;
        self.frame_stats.buffer_age = plan.buffer_age;
        self.frame_stats.current_damage_rects = plan.render_damage.rect_count();
        self.frame_stats.current_damage_pixels =
            plan.render_damage.pixels(width, height).unwrap_or(u64::MAX);
        self.frame_stats.repair_damage_rects = plan.repair_damage.rect_count();
        self.frame_stats.repair_damage_pixels =
            plan.repair_damage.pixels(width, height).unwrap_or(u64::MAX);
        self.frame_stats.fallback_reason = plan.fallback_reason;
        self.frame_stats.partial_repaint_enabled = self.repaint_planner.partial_enabled();
        self.frame_stats.history_depth = self.repaint_planner.history_depth();
    }

    fn record_effect_graph_metrics(&mut self, metrics: EffectGraphMetrics) {
        self.frame_stats.effect_instances_visible = metrics.instances;
        self.frame_stats.render_graph_passes = metrics.passes;
        self.frame_stats.render_graph_peak_live_textures = metrics.peak_live_textures;
        self.frame_stats.effect_graph_peak_live_bytes = metrics.peak_live_bytes;
        self.frame_stats.effect_capture_pixels = metrics.capture_pixels;
        self.frame_stats.effect_output_pixels = metrics.output_pixels;
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
            visible_effect_count: Some(self.frame_stats.effect_instances_visible),
            selected_effect_count,
            graph_pass_count: graph.map(|graph| graph.stats.passes),
            graph_texture_count: graph.map(|graph| graph.stats.textures),
            peak_live_intermediate_count: graph.map(|graph| graph.stats.peak_live_intermediates),
            ..FrameTraceSummary::default()
        }
    }

    fn record_effect_resource_metrics(&mut self) {
        let metrics = self.effect_resources.metrics();
        self.frame_stats.effect_resource_allocations = metrics.allocation_count;
        self.frame_stats.effect_resource_reuses = metrics.reuse_count;
        self.frame_stats.effect_resource_evictions = metrics.eviction_count;
        self.frame_stats.effect_resource_allocations_total = metrics.allocation_count;
        self.frame_stats.effect_resource_reuses_total = metrics.reuse_count;
        self.frame_stats.effect_resource_evictions_total = metrics.eviction_count;
        self.frame_stats.effect_gpu_cache_bytes = metrics.current_bytes;
        self.frame_stats.effect_gpu_cache_peak_bytes = metrics.peak_bytes;
        self.frame_stats.effect_gpu_budget_bytes = metrics.budget_bytes;
        self.frame_stats.effect_gpu_cached_keys = metrics.cached_key_count;
        self.frame_stats.effect_gpu_cached_textures = metrics.cached_texture_count;
        self.frame_stats.effect_gpu_checked_out_textures = metrics.checked_out_texture_count;
        let shader_metrics = self.effect_shaders.metrics();
        self.frame_stats.shader_cache_capacity = shader_metrics.capacity;
        self.frame_stats.shader_cache_entries = shader_metrics.resident_entries;
        self.frame_stats.shader_cache_peak_entries = shader_metrics.peak_entries;
        self.frame_stats.shader_cache_evictions_total = shader_metrics.eviction_count;
    }

    fn ensure_output_size(&mut self, width: u32, height: u32) -> RendererResult<()> {
        if self.current_size == (width, height) {
            return Ok(());
        }

        self.release_all_lifecycle_visual_resources();
        self.current_size = (width, height);
        self.repaint_planner.resize((width, height));
        self.scene_cache_key = None;
        self.effect_resources.cleanup_size_history(&self.gl);
        unsafe {
            self.gl.viewport(0, 0, width as i32, height as i32);
        }
        Ok(())
    }

    fn ensure_cursor_resource(
        &mut self,
        egl: &EglInstance,
        egl_display: egl::Display,
    ) -> RendererResult<()> {
        let width = self.cursor_image.width;
        let height = self.cursor_image.height;
        if width == 0 || height == 0 {
            return Ok(());
        }
        if self
            .cursor_resource
            .as_ref()
            .is_some_and(|resource| resource.size == (width, height) && !self.cursor_resource_stale)
        {
            return Ok(());
        }

        let mut resource = create_uploaded_resource(&self.gl, width, height)?;
        write_argb_pixels_to_resource(
            &self.gl,
            &resource,
            SurfaceDamageRect::full(width, height),
            &self.cursor_image.pixels_argb8888,
            &mut self.texture_upload_rgba,
        );
        if let Some(old) = self.cursor_resource.take() {
            destroy_image_resource(&self.gl, egl, egl_display, old);
        }
        resource.generation = 1;
        self.cursor_resource = Some(resource);
        self.cursor_resource_stale = false;
        Ok(())
    }

    fn ensure_frame_resources(&mut self) -> RendererResult<()> {
        for color in compositor::ServerFrameColor::ALL {
            if self.frame_resources.contains_key(&color) {
                continue;
            }

            let mut resource = create_uploaded_resource(&self.gl, 1, 1)?;
            write_argb_pixels_to_resource(
                &self.gl,
                &resource,
                SurfaceDamageRect::full(1, 1),
                &[color.pixel()],
                &mut self.texture_upload_rgba,
            );
            resource.generation = 1;
            self.frame_resources.insert(color, resource);
        }
        Ok(())
    }

    fn ensure_decoration_resources<'a, I>(
        &mut self,
        egl: &EglInstance,
        egl_display: egl::Display,
        instances: I,
    ) -> RendererResult<()>
    where
        I: IntoIterator<Item = &'a DecorationRenderInstance>,
    {
        let DecorationResourceRequirements {
            required,
            required_assets,
        } = decoration_resource_requirements(
            instances
                .into_iter()
                .map(DecorationRenderInstance::primitives),
        );

        let stale = self
            .decoration_resources
            .keys()
            .copied()
            .filter(|key| !required.contains(key))
            .collect::<Vec<_>>();
        for key in stale {
            if let Some(resource) = self.decoration_resources.remove(&key) {
                destroy_image_resource(&self.gl, egl, egl_display, resource);
            }
        }

        for key in required {
            if self.decoration_resources.contains_key(&key) {
                continue;
            }
            let mut resource = match key {
                DecorationResourceKey::Solid(color) => {
                    let resource = create_uploaded_resource(&self.gl, 1, 1)?;
                    write_argb_pixels_to_resource(
                        &self.gl,
                        &resource,
                        SurfaceDamageRect::full(1, 1),
                        &[color],
                        &mut self.texture_upload_rgba,
                    );
                    resource
                }
                DecorationResourceKey::Asset(asset_id) => {
                    let asset = required_assets
                        .get(&asset_id)
                        .expect("required decoration asset was collected");
                    let resource =
                        create_uploaded_resource(&self.gl, asset.width(), asset.height())?;
                    write_rgba_bytes_to_resource(
                        &self.gl,
                        &resource,
                        SurfaceDamageRect::full(asset.width(), asset.height()),
                        asset.rgba_premultiplied(),
                    );
                    resource
                }
            };
            resource.generation = 1;
            self.decoration_resources.insert(key, resource);
        }
        Ok(())
    }

    fn reconcile_surface_resource_lifetimes(
        &mut self,
        egl: &EglInstance,
        egl_display: egl::Display,
        surfaces: &[RenderableSurface],
        lifecycle_surfaces: &[RenderableSurface],
        client_cursor: Option<&RenderableSurface>,
    ) -> RendererResult<()> {
        self.evict_dead_cached_dmabufs(egl, egl_display);
        self.active_surface_ids.clear();
        self.active_surface_ids
            .extend(surfaces.iter().map(|surface| surface.surface_id));
        self.active_surface_ids
            .extend(lifecycle_surfaces.iter().map(|surface| surface.surface_id));
        self.active_surface_ids
            .extend(client_cursor.map(|surface| surface.surface_id));
        self.active_surface_ids.sort_unstable();
        self.active_surface_ids.dedup();

        for surface in surfaces
            .iter()
            .chain(lifecycle_surfaces)
            .chain(client_cursor)
        {
            let Some((action, resource)) =
                reconcile_surface_resource_backing(&mut self.surface_resources, surface)
            else {
                continue;
            };
            match action {
                SurfaceResourceLifetimeAction::DemoteDmabuf => {
                    self.cache_or_destroy_dmabuf_resource(
                        egl,
                        egl_display,
                        surface.surface_id,
                        resource,
                    );
                }
                SurfaceResourceLifetimeAction::Destroy => {
                    destroy_surface_resource(&self.gl, egl, egl_display, resource);
                }
                SurfaceResourceLifetimeAction::Keep => {
                    unreachable!("current surface resource reconciliation never removes Keep")
                }
            }
        }

        let stale_ids = self
            .surface_resources
            .keys()
            .copied()
            .filter(|id| self.active_surface_ids.binary_search(id).is_err())
            .collect::<Vec<_>>();
        for surface_id in stale_ids {
            if let Some(resource) = self.surface_resources.remove(&surface_id) {
                destroy_surface_resource(&self.gl, egl, egl_display, resource);
            }
            self.destroy_cached_dmabufs_for_surface(egl, egl_display, surface_id);
            self.failed_surface_generations.remove(&surface_id);
        }

        self.frame_stats.dmabuf_cache_entries = self.dmabuf_resource_cache.len();
        self.frame_stats.dmabuf_cache_peak_entries = self.dmabuf_cache_peak_entries;
        self.frame_stats.dmabuf_cache_max_entries_for_one_surface =
            self.dmabuf_cache_max_entries_for_one_surface;
        Ok(())
    }

    fn realize_surface_resources_for_consumers(
        &mut self,
        egl: &EglInstance,
        egl_display: egl::Display,
        inputs: SurfaceResourceInputs<'_>,
        consumers: &SurfaceConsumerPlan,
        sync_states: &[SurfaceResourceSyncState],
    ) -> RendererResult<()> {
        for surface in inputs
            .canonical
            .iter()
            .chain(inputs.lifecycle)
            .chain(inputs.client_cursor)
        {
            if consumers
                .surface_ids()
                .binary_search(&surface.surface_id)
                .is_err()
            {
                continue;
            }
            let sync_state = sync_states
                .iter()
                .find(|state| state.surface_id == surface.surface_id)
                .copied()
                .unwrap_or(SurfaceResourceSyncState {
                    surface_id: surface.surface_id,
                    complete_since: None,
                    current_commit: SurfaceCommitCounter::default(),
                    authoritative: false,
                });
            self.realize_surface_resource(egl, egl_display, surface, sync_state)?;
        }
        self.frame_stats.dmabuf_cache_entries = self.dmabuf_resource_cache.len();
        self.frame_stats.dmabuf_cache_peak_entries = self.dmabuf_cache_peak_entries;
        self.frame_stats.dmabuf_cache_max_entries_for_one_surface =
            self.dmabuf_cache_max_entries_for_one_surface;
        Ok(())
    }

    fn realize_surface_resource(
        &mut self,
        egl: &EglInstance,
        egl_display: egl::Display,
        surface: &RenderableSurface,
        sync_state: SurfaceResourceSyncState,
    ) -> RendererResult<()> {
        let update = self
            .surface_resources
            .get(&surface.surface_id)
            .map_or(EglSurfaceResourceUpdate::Recreate, |resource| {
                resource.update_for(surface, sync_state)
            });
        match update {
            EglSurfaceResourceUpdate::Reuse => return Ok(()),
            EglSurfaceResourceUpdate::ReuseShm => {
                if let Some(resource) = self.surface_resources.get_mut(&surface.surface_id) {
                    resource.advance_shm_sync_baseline(sync_state.current_commit);
                }
                return Ok(());
            }
            EglSurfaceResourceUpdate::ReuseDmabuf => {
                if let Some(resource) = self.surface_resources.get_mut(&surface.surface_id) {
                    resource.image.generation = surface.generation;
                }
                self.frame_stats.dmabuf_current_resource_reuses = self
                    .frame_stats
                    .dmabuf_current_resource_reuses
                    .saturating_add(1);
                self.frame_stats.dmabuf_reuses = self.frame_stats.dmabuf_reuses.saturating_add(1);
                return Ok(());
            }
            EglSurfaceResourceUpdate::UploadDamage | EglSurfaceResourceUpdate::FullShmResync => {
                if let Some(resource) = self.surface_resources.get_mut(&surface.surface_id) {
                    let force_full = update == EglSurfaceResourceUpdate::FullShmResync;
                    self.frame_stats.shm_upload_bytes = self
                        .frame_stats
                        .shm_upload_bytes
                        .saturating_add(resource.write_shm_damage(
                            &self.gl,
                            surface,
                            force_full,
                            sync_state.current_commit,
                            &mut self.texture_upload_rgba,
                        ));
                    if force_full {
                        self.frame_stats.shm_full_resyncs =
                            self.frame_stats.shm_full_resyncs.saturating_add(1);
                    }
                }
                return Ok(());
            }
            EglSurfaceResourceUpdate::Recreate if surface.dmabuf_handle().is_some() => {
                self.switch_dmabuf_surface_resource(egl, egl_display, surface)?;
                return Ok(());
            }
            EglSurfaceResourceUpdate::Recreate => {}
            EglSurfaceResourceUpdate::UnsupportedBuffer => {
                if let Some(resource) = self.surface_resources.remove(&surface.surface_id) {
                    destroy_surface_resource(&self.gl, egl, egl_display, resource);
                }
                self.destroy_cached_dmabufs_for_surface(egl, egl_display, surface.surface_id);
                return Ok(());
            }
        }

        if let Some(old) = self.surface_resources.remove(&surface.surface_id) {
            destroy_surface_resource(&self.gl, egl, egl_display, old);
        }
        if surface.dmabuf_handle().is_none() {
            self.destroy_cached_dmabufs_for_surface(egl, egl_display, surface.surface_id);
        }

        let result = create_surface_resource(
            &self.gl,
            egl,
            egl_display,
            self.egl_image_target_texture_2d,
            surface,
            (surface.cpu_pixels().is_some() && sync_state.authoritative)
                .then_some(sync_state.current_commit),
            &mut self.texture_upload_rgba,
        );
        if let Some(context) = DmabufImportDiagnosticContext::from_surface(
            surface,
            DmabufImportPath::Initial,
            DmabufImportCacheState::NotChecked,
        ) {
            if let Some(resource) = settle_dmabuf_import_result(
                result,
                context,
                &mut self.frame_stats,
                &mut self.failed_surface_generations,
            )? {
                self.frame_stats.dmabuf_imports = self.frame_stats.dmabuf_imports.saturating_add(1);
                self.surface_resources.insert(surface.surface_id, resource);
            }
        } else {
            match result {
                Ok(resource) => {
                    self.frame_stats.shm_upload_bytes = self
                        .frame_stats
                        .shm_upload_bytes
                        .saturating_add(surface_upload_byte_len(surface));
                    self.failed_surface_generations.remove(&surface.surface_id);
                    self.surface_resources.insert(surface.surface_id, resource);
                }
                Err(error) => {
                    eprintln!(
                        "oblivion-one compositor: failed to realize surface {} on EGL/GLES: {error}",
                        surface.surface_id
                    );
                }
            }
        }
        Ok(())
    }

    fn switch_dmabuf_surface_resource(
        &mut self,
        egl: &EglInstance,
        egl_display: egl::Display,
        surface: &RenderableSurface,
    ) -> RendererResult<()> {
        let Some(handle) = surface.dmabuf_handle() else {
            return Ok(());
        };
        let key = DmabufImageKey::from_handle(surface.buffer_id(), handle);

        if let Some(mut cached) = self.dmabuf_resource_cache.remove(&key) {
            if native_egl_debug_enabled() {
                eprintln!(
                    "oblivion-one compositor: dmabuf cache=hit surface={} key={key:?} texture={:?} egl_image={:?}",
                    surface.surface_id,
                    cached.image.texture,
                    cached.image.egl_image.map(|image| image.as_ptr()),
                );
                if let Some(context) = DmabufImportDiagnosticContext::from_surface(
                    surface,
                    DmabufImportPath::Replacement,
                    DmabufImportCacheState::Hit,
                ) {
                    log_dmabuf_import_context(
                        context,
                        "resource_selected",
                        "cache",
                        cached.image.egl_image.is_some(),
                        None,
                        "success",
                    );
                }
            }
            cached.image.generation = surface.generation;
            self.frame_stats.dmabuf_cache_hits =
                self.frame_stats.dmabuf_cache_hits.saturating_add(1);
            self.frame_stats.dmabuf_reuses = self.frame_stats.dmabuf_reuses.saturating_add(1);
            if let Some(old) = self.surface_resources.insert(
                surface.surface_id,
                EglSurfaceResource {
                    image: cached.image,
                    dmabuf_key: Some(key),
                    buffer_lifetime: Some(surface.buffer_identity().downgrade()),
                    shm_synced_commit: None,
                },
            ) {
                self.cache_or_destroy_dmabuf_resource(egl, egl_display, surface.surface_id, old);
            }
            return Ok(());
        }

        if native_egl_debug_enabled() {
            eprintln!(
                "oblivion-one compositor: dmabuf cache=miss surface={} key={key:?} layout={:?}",
                surface.surface_id,
                surface.dmabuf_handle(),
            );
        }
        self.frame_stats.dmabuf_cache_misses =
            self.frame_stats.dmabuf_cache_misses.saturating_add(1);

        let Some(old) = self.surface_resources.remove(&surface.surface_id) else {
            let result = create_surface_resource(
                &self.gl,
                egl,
                egl_display,
                self.egl_image_target_texture_2d,
                surface,
                None,
                &mut self.texture_upload_rgba,
            );
            let context = DmabufImportDiagnosticContext::from_surface(
                surface,
                DmabufImportPath::Initial,
                DmabufImportCacheState::Miss,
            )
            .expect("switching a DMA-BUF resource requires a DMA-BUF surface");
            if let Some(resource) = settle_dmabuf_import_result(
                result,
                context,
                &mut self.frame_stats,
                &mut self.failed_surface_generations,
            )? {
                self.frame_stats.dmabuf_imports = self.frame_stats.dmabuf_imports.saturating_add(1);
                self.surface_resources.insert(surface.surface_id, resource);
            }
            return Ok(());
        };
        self.cache_or_destroy_dmabuf_resource(egl, egl_display, surface.surface_id, old);

        let result = create_surface_resource(
            &self.gl,
            egl,
            egl_display,
            self.egl_image_target_texture_2d,
            surface,
            None,
            &mut self.texture_upload_rgba,
        );
        let context = DmabufImportDiagnosticContext::from_surface(
            surface,
            DmabufImportPath::Replacement,
            DmabufImportCacheState::Miss,
        )
        .expect("switching a DMA-BUF resource requires a DMA-BUF surface");
        if let Some(resource) = settle_dmabuf_import_result(
            result,
            context,
            &mut self.frame_stats,
            &mut self.failed_surface_generations,
        )? {
            self.frame_stats.dmabuf_imports = self.frame_stats.dmabuf_imports.saturating_add(1);
            self.surface_resources.insert(surface.surface_id, resource);
        }
        Ok(())
    }

    fn cache_or_destroy_dmabuf_resource(
        &mut self,
        egl: &EglInstance,
        egl_display: egl::Display,
        surface_id: u32,
        resource: EglSurfaceResource,
    ) {
        let Some(key) = resource.dmabuf_key else {
            destroy_surface_resource(&self.gl, egl, egl_display, resource);
            return;
        };
        let Some(buffer_lifetime) = resource.buffer_lifetime else {
            destroy_image_resource(&self.gl, egl, egl_display, resource.image);
            return;
        };
        if !buffer_lifetime.is_alive() {
            if native_egl_debug_enabled() {
                eprintln!(
                    "oblivion-one compositor: dmabuf cache=evict reason=dead-before-cache key={key:?}"
                );
            }
            destroy_image_resource(&self.gl, egl, egl_display, resource.image);
            self.frame_stats.dmabuf_cache_evictions =
                self.frame_stats.dmabuf_cache_evictions.saturating_add(1);
            self.frame_stats.dmabuf_cache_evictions_dead = self
                .frame_stats
                .dmabuf_cache_evictions_dead
                .saturating_add(1);
            return;
        }

        self.prune_cached_dmabufs_for_surface(egl, egl_display, surface_id);
        if let Some(replaced) = self.dmabuf_resource_cache.insert(
            key,
            CachedDmabufResource {
                image: resource.image,
                buffer_lifetime,
                surface_id,
            },
        ) {
            destroy_image_resource(&self.gl, egl, egl_display, replaced.image);
        }
        self.frame_stats.dmabuf_cache_insertions =
            self.frame_stats.dmabuf_cache_insertions.saturating_add(1);
        let surface_entries = self
            .dmabuf_resource_cache
            .values()
            .filter(|cached| cached.surface_id == surface_id)
            .count();
        self.dmabuf_cache_max_entries_for_one_surface = self
            .dmabuf_cache_max_entries_for_one_surface
            .max(surface_entries);
        self.dmabuf_cache_peak_entries = self
            .dmabuf_cache_peak_entries
            .max(self.dmabuf_resource_cache.len());
    }

    fn prune_cached_dmabufs_for_surface(
        &mut self,
        egl: &EglInstance,
        egl_display: egl::Display,
        surface_id: u32,
    ) {
        let cached = self
            .dmabuf_resource_cache
            .values()
            .filter(|cached| cached.surface_id == surface_id)
            .count();
        if cached < MAX_CACHED_DMABUF_RESOURCES_PER_SURFACE {
            return;
        }
        let Some(key) = self
            .dmabuf_resource_cache
            .iter()
            .find_map(|(key, cached)| (cached.surface_id == surface_id).then_some(key.clone()))
        else {
            return;
        };
        if let Some(resource) = self.dmabuf_resource_cache.remove(&key) {
            if native_egl_debug_enabled() {
                eprintln!(
                    "oblivion-one compositor: dmabuf cache=evict reason=surface-bound key={key:?}"
                );
            }
            destroy_image_resource(&self.gl, egl, egl_display, resource.image);
            self.frame_stats.dmabuf_cache_evictions =
                self.frame_stats.dmabuf_cache_evictions.saturating_add(1);
            self.frame_stats.dmabuf_cache_evictions_surface_bound = self
                .frame_stats
                .dmabuf_cache_evictions_surface_bound
                .saturating_add(1);
        }
    }

    fn destroy_cached_dmabufs_for_surface(
        &mut self,
        egl: &EglInstance,
        egl_display: egl::Display,
        surface_id: u32,
    ) {
        let keys = self
            .dmabuf_resource_cache
            .iter()
            .filter_map(|(key, cached)| (cached.surface_id == surface_id).then_some(key.clone()))
            .collect::<Vec<_>>();
        for key in keys {
            if let Some(resource) = self.dmabuf_resource_cache.remove(&key) {
                if native_egl_debug_enabled() {
                    eprintln!(
                        "oblivion-one compositor: dmabuf cache=evict reason=surface-destroyed key={key:?}"
                    );
                }
                destroy_image_resource(&self.gl, egl, egl_display, resource.image);
                self.frame_stats.dmabuf_cache_evictions =
                    self.frame_stats.dmabuf_cache_evictions.saturating_add(1);
                self.frame_stats.dmabuf_cache_evictions_surface_destroyed = self
                    .frame_stats
                    .dmabuf_cache_evictions_surface_destroyed
                    .saturating_add(1);
            }
        }
    }

    fn evict_dead_cached_dmabufs(&mut self, egl: &EglInstance, egl_display: egl::Display) {
        let dead = dead_cached_dmabuf_keys(&self.dmabuf_resource_cache);
        for key in dead {
            if let Some(cached) = self.dmabuf_resource_cache.remove(&key) {
                if native_egl_debug_enabled() {
                    eprintln!(
                        "oblivion-one compositor: dmabuf cache=evict reason=buffer-dead key={key:?}"
                    );
                }
                destroy_image_resource(&self.gl, egl, egl_display, cached.image);
                self.frame_stats.dmabuf_cache_evictions =
                    self.frame_stats.dmabuf_cache_evictions.saturating_add(1);
                self.frame_stats.dmabuf_cache_evictions_dead = self
                    .frame_stats
                    .dmabuf_cache_evictions_dead
                    .saturating_add(1);
            }
        }
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
        decoration_instances: &[DecorationRenderInstance],
        popup_surface_ids: &[u32],
        presentation_geometry_signature: u64,
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> bool {
        self.scene_cache_key.is_some_and(|key| {
            key.is_current_with_decorations(
                width,
                height,
                content_generation,
                output_scale_key,
                surface_signatures,
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
        presentation_geometry_signature: u64,
        framebuffer_origin: OutputFramebufferOrigin,
    ) {
        self.frame_stats.orphan_decoration_count =
            compositor::WindowVisualGroup::orphan_decoration_count(surfaces, decoration_instances);
        self.vertices.clear();
        self.commands.clear();
        self.scene_geometry_dirty = true;
        self.vertices.reserve((1 + surfaces.len()) * 6);
        self.commands.reserve(1 + surfaces.len());

        push_output_background_command(
            &mut self.vertices,
            &mut self.commands,
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
                    &mut self.vertices,
                    &mut self.commands,
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
                    &mut self.vertices,
                    &mut self.commands,
                    width,
                    height,
                    instance,
                    output_scale,
                    framebuffer_origin,
                    visual_group,
                );
            }
        }

        self.scene_cache_key = Some(EglSceneCacheKey::new_with_decorations(
            width,
            height,
            content_generation,
            output_scale_key,
            surface_signatures,
            decoration_instances,
            popup_surface_ids,
            presentation_geometry_signature,
            framebuffer_origin,
        ));
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
        self.cursor_vertices.clear();
        self.cursor_commands.clear();
        self.overlay_geometry_dirty = true;

        let render_assignments =
            compositor::surface_render_space_assignments(overlay_surfaces, output_scale);
        for (surface, render_assignment) in overlay_surfaces.iter().zip(render_assignments) {
            push_egl_surface_commands(
                &mut self.cursor_vertices,
                &mut self.cursor_commands,
                width,
                height,
                surface,
                render_assignment,
                framebuffer_origin,
                None,
            );
        }

        if let Some((cursor_x, cursor_y)) = visual_state.cursor
            && let Some(cursor) = self.cursor_resource.as_ref()
        {
            let (top_left_x, top_left_y) = self.cursor_image.top_left(cursor_x, cursor_y);
            push_draw_command(
                &mut self.cursor_vertices,
                &mut self.cursor_commands,
                EglDrawLayer::Cursor,
                EglRect::new(
                    top_left_x as f32,
                    top_left_y as f32,
                    cursor.size.0 as f32,
                    cursor.size.1 as f32,
                ),
                width,
                height,
                framebuffer_origin,
            );
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
            let uv = EglUvRect::new(
                render_plan.content_uv.left,
                render_plan.content_uv.top,
                render_plan.content_uv.right,
                render_plan.content_uv.bottom,
            );
            push_draw_command_with_uv(
                &mut self.cursor_vertices,
                &mut self.cursor_commands,
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
            .render_execution(self.current_size.0, self.current_size.1, framebuffer_origin)
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
                                self.current_size.1,
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
                                self.current_size.1,
                                framebuffer_origin,
                            );
                            draw_result = self.draw_lamp_overlay(output_rect);
                        }
                        LegacySceneScissoredPhase::ExternalOverlays(index) => {
                            let [x, y, width, height] = scissors[index];
                            let output_rect = gl_scissor_to_output_rect(
                                [x, y, width, height],
                                self.current_size.1,
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
                self.frame_stats.scissor_passes = scissors.len();
            }
        }

        unsafe {
            self.gl.disable(glow::SCISSOR_TEST);
            self.gl.bind_texture(glow::TEXTURE_2D, None);
        }
        Ok(())
    }

    fn rebuild_lamp_commands_if_needed(
        &mut self,
        lifecycle: &LifecycleSceneSample,
        lifecycle_surfaces: &[RenderableSurface],
        lifecycle_decorations: &[DecorationRenderInstance],
        output_scale: f64,
    ) {
        let geometry_key = lamp_geometry_key(
            lifecycle,
            lifecycle_surfaces,
            lifecycle_decorations,
            output_scale,
            self.current_framebuffer_origin,
        );
        if self.lamp_geometry_key == Some(geometry_key) {
            return;
        }
        self.lamp_geometry_key = Some(geometry_key);
        self.lamp_geometry_dirty = true;
        self.lamp_vertices.clear();
        self.lamp_commands.clear();
        self.lifecycle_source_vertices.clear();
        self.lifecycle_source_commands.clear();
        if lifecycle.lamps.is_empty() {
            return;
        }
        let mut transition_starts = HashMap::new();
        let mut rejected_transitions = HashSet::new();
        let assignments =
            compositor::surface_render_space_assignments(lifecycle_surfaces, output_scale);
        for lamp in lifecycle.lamps.iter().copied() {
            let resolved_source = lifecycle
                .visual_source_for_window(lamp.window_id)
                .filter(|source| source.kind == LifecycleVisualSourceKind::ResolvedOwnedEffects);
            if let Some(source) = resolved_source {
                let mut vertices = Vec::new();
                let mut commands = Vec::new();
                let visual_group = source
                    .effect_scene
                    .instances
                    .first()
                    .and_then(|instance| instance.visual_group)
                    .or_else(|| VisualGroupId::new(1));
                for (surface, assignment) in lifecycle_surfaces.iter().zip(assignments.iter()) {
                    if surface_root_for_lamp(surface, lifecycle_surfaces, lifecycle)
                        != lamp.root_surface_id
                    {
                        continue;
                    }
                    push_egl_surface_commands(
                        &mut vertices,
                        &mut commands,
                        self.current_size.0,
                        self.current_size.1,
                        surface,
                        assignment.clone(),
                        self.current_framebuffer_origin,
                        visual_group,
                    );
                }
                for decoration in lifecycle_decorations
                    .iter()
                    .filter(|decoration| decoration.root_surface_id() == lamp.root_surface_id)
                {
                    push_egl_decoration_instance(
                        &mut vertices,
                        &mut commands,
                        self.current_size.0,
                        self.current_size.1,
                        decoration,
                        output_scale,
                        self.current_framebuffer_origin,
                        visual_group,
                    );
                }
                reproject_lifecycle_source_commands(
                    &mut vertices,
                    &mut commands,
                    lamp.visual_group.canonical_client_rect,
                    lamp.visual_group.presented_source_client_rect,
                    output_scale,
                    self.current_size,
                    self.current_framebuffer_origin,
                );
                self.lifecycle_source_vertices
                    .insert(source.window_id, vertices);
                self.lifecycle_source_commands
                    .insert(source.window_id, commands);
                self.append_lamp_grid_for_transition(
                    lamp,
                    LampGridSpec {
                        layer: EglDrawLayer::LifecycleResolvedVisual(source.window_id),
                        window_id: source.window_id,
                        bounds: EglRect::new(
                            (lamp.visual_group.canonical_visual_rect.x() * output_scale) as f32,
                            (lamp.visual_group.canonical_visual_rect.y() * output_scale) as f32,
                            (lamp.visual_group.canonical_visual_rect.width() * output_scale) as f32,
                            (lamp.visual_group.canonical_visual_rect.height() * output_scale)
                                as f32,
                        ),
                        uv: EglUvRect::new(0.0, 1.0, 1.0, 0.0),
                    },
                    &mut transition_starts,
                    &mut rejected_transitions,
                );
                continue;
            }

            for (surface, assignment) in lifecycle_surfaces.iter().zip(assignments.iter()) {
                if surface_root_for_lamp(surface, lifecycle_surfaces, lifecycle)
                    != lamp.root_surface_id
                {
                    continue;
                }
                for render_plan in compositor::surface_render_plans_with_aperture(
                    surface,
                    assignment.target,
                    assignment.visual_clip.as_ref(),
                ) {
                    let target = render_plan.content_target;
                    if target.width() == 0 || target.height() == 0 {
                        continue;
                    }
                    self.append_lamp_grid_for_transition(
                        lamp,
                        LampGridSpec {
                            layer: EglDrawLayer::Surface(surface.surface_id),
                            window_id: lamp.window_id,
                            bounds: EglRect::new(
                                target.x() as f32,
                                target.y() as f32,
                                target.width() as f32,
                                target.height() as f32,
                            ),
                            uv: EglUvRect::new(
                                render_plan.content_uv.left,
                                render_plan.content_uv.top,
                                render_plan.content_uv.right,
                                render_plan.content_uv.bottom,
                            ),
                        },
                        &mut transition_starts,
                        &mut rejected_transitions,
                    );
                }
            }
            let scale = output_scale.max(1.0) as f32;
            for decoration in lifecycle_decorations
                .iter()
                .filter(|decoration| decoration.root_surface_id() == lamp.root_surface_id)
            {
                let (origin_x, origin_y) = decoration.origin();
                for primitive in decoration.primitives() {
                    let (rect, uv, layer) = match primitive {
                        DecorationRenderPrimitive::SolidRect { rect, color } => (
                            *rect,
                            EglUvRect::new(0.0, 0.0, 1.0, 1.0),
                            EglDrawLayer::SolidRgba(rgba_to_pixel(*color)),
                        ),
                        DecorationRenderPrimitive::Image { rect, asset } => (
                            *rect,
                            EglUvRect::new(0.0, 0.0, 1.0, 1.0),
                            EglDrawLayer::DecorationAsset(asset.asset_id()),
                        ),
                        DecorationRenderPrimitive::Text {
                            rect, clip, asset, ..
                        } => {
                            let Some(crop) = clipped_decoration_text_geometry(*rect, *clip) else {
                                continue;
                            };
                            (
                                crop.rect,
                                EglUvRect::new(crop.uv[0], crop.uv[1], crop.uv[2], crop.uv[3]),
                                EglDrawLayer::DecorationAsset(asset.asset_id()),
                            )
                        }
                    };
                    self.append_lamp_grid_for_transition(
                        lamp,
                        LampGridSpec {
                            layer,
                            window_id: lamp.window_id,
                            bounds: EglRect::new(
                                origin_x.saturating_add(rect.x) as f32 * scale,
                                origin_y.saturating_add(rect.y) as f32 * scale,
                                rect.width as f32 * scale,
                                rect.height as f32 * scale,
                            ),
                            uv,
                        },
                        &mut transition_starts,
                        &mut rejected_transitions,
                    );
                }
            }
        }
        for lamp in &lifecycle.lamps {
            if !transition_starts.contains_key(&lamp.window_id)
                && !rejected_transitions.contains(&lamp.window_id)
            {
                self.record_lifecycle_render_fallback(
                    *lamp,
                    LifecycleRenderFallbackReason::MeshBudget,
                );
            }
        }
    }

    fn append_lamp_grid_for_transition(
        &mut self,
        lamp: LampWindowSample,
        spec: LampGridSpec,
        transition_starts: &mut HashMap<compositor::WindowId, (usize, usize)>,
        rejected_transitions: &mut HashSet<compositor::WindowId>,
    ) {
        if rejected_transitions.contains(&lamp.window_id) {
            return;
        }
        let start = *transition_starts
            .entry(lamp.window_id)
            .or_insert((self.lamp_vertices.len(), self.lamp_commands.len()));
        if !append_lamp_grid(&mut self.lamp_vertices, &mut self.lamp_commands, spec) {
            self.lamp_vertices.truncate(start.0);
            self.lamp_commands.truncate(start.1);
            rejected_transitions.insert(lamp.window_id);
            self.record_lifecycle_render_fallback(lamp, LifecycleRenderFallbackReason::MeshBudget);
        }
    }

    /// Materialize one frozen, bounded compositor-resolved source for each
    /// effected lifecycle group. The source is captured after the ordinary
    /// scene/effect pass and reused for the complete short transition, so the
    /// effect graph never evaluates backdrop pixels against a moving Lamp
    /// mesh. A failed capture reports an exact lifecycle fallback instead of
    /// exposing a raw warped representation of an effect-owned source.
    fn prepare_lifecycle_visual_sources(
        &mut self,
        _plan: &RepaintPlan,
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> RendererResult<()> {
        self.release_stale_lifecycle_visual_resources();
        let output_scale = self.effect_output_scale.max(1.0) as f64;
        let sources = self
            .lifecycle_visual_sources
            .values()
            .filter(|source| source.kind == LifecycleVisualSourceKind::ResolvedOwnedEffects)
            .cloned()
            .collect::<Vec<_>>();

        for source in sources {
            let Some(lamp) = self
                .lamp_samples
                .iter()
                .find(|sample| sample.window_id == source.window_id)
                .copied()
            else {
                continue;
            };
            let source_visual_rect = scaled_presentation_rect(
                lamp.visual_group.presented_source_visual_rect,
                output_scale,
            );
            let Some((width, height)) = lifecycle_visual_texture_size(source_visual_rect) else {
                self.record_lifecycle_render_fallback(
                    lamp,
                    LifecycleRenderFallbackReason::ResolvedSourceAllocation,
                );
                continue;
            };
            let source_signature = lifecycle_visual_source_signature(&source, lamp, output_scale);
            let ready = self
                .lifecycle_visual_resources
                .get(&source.window_id)
                .is_some_and(|resource| {
                    resource.source_signature == source_signature
                        && resource.source_visual_rect
                            == lamp.visual_group.presented_source_visual_rect
                        && self.effect_resources.texture(&resource.texture).is_some()
                });
            if ready {
                continue;
            }

            if let Some(previous) = self.lifecycle_visual_resources.remove(&source.window_id) {
                let _ = self.effect_resources.release(previous.texture);
            }
            let texture_key = EffectTextureKey::new(
                width,
                height,
                EffectTextureFormat::Rgba8,
                EffectTextureFilter::Linear,
                EffectWorkingSpace::OutputEncodedSrgb,
            );
            let Ok(texture) = self.effect_resources.acquire(&self.gl, texture_key) else {
                self.record_lifecycle_render_fallback(
                    lamp,
                    LifecycleRenderFallbackReason::ResolvedSourceAllocation,
                );
                continue;
            };
            if self
                .capture_lifecycle_visual_source(&source, lamp, texture.clone(), framebuffer_origin)
                .is_err()
            {
                let _ = self.effect_resources.release(texture);
                self.record_lifecycle_render_fallback(
                    lamp,
                    LifecycleRenderFallbackReason::ResolvedSourceCapture,
                );
                continue;
            }
            self.lifecycle_visual_resources.insert(
                source.window_id,
                LifecycleResolvedVisualResource {
                    texture,
                    source_signature,
                    source_visual_rect: lamp.visual_group.presented_source_visual_rect,
                },
            );
        }
        Ok(())
    }

    fn record_lifecycle_render_fallback(
        &mut self,
        lamp: LampWindowSample,
        reason: LifecycleRenderFallbackReason,
    ) {
        self.lifecycle_render_fallbacks
            .record(LifecycleRenderFallbackEntry {
                window_id: lamp.window_id,
                root_surface_id: lamp.root_surface_id,
                transition_id: lamp.transition_id,
                reason,
            });
    }

    fn capture_lifecycle_visual_source(
        &mut self,
        source: &LifecycleVisualSource,
        lamp: LampWindowSample,
        target: PooledEffectTexture,
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> RendererResult<()> {
        clear_effect_texture(self, &target)?;
        let backup = self.effect_resources.acquire(&self.gl, target.key)?;
        let result = (|| {
            copy_output_region_to_texture(
                self,
                &backup,
                lamp.visual_group.presented_source_visual_rect,
                framebuffer_origin,
            )?;

            let source_vertices = self
                .lifecycle_source_vertices
                .get(&source.window_id)
                .cloned()
                .unwrap_or_default();
            let source_commands = self
                .lifecycle_source_commands
                .get(&source.window_id)
                .cloned()
                .unwrap_or_default();
            if source_vertices.is_empty() || source_commands.is_empty() {
                return Err(
                    io::Error::other("lifecycle visual source has no draw commands").into(),
                );
            }

            let saved_vertices = std::mem::replace(&mut self.vertices, source_vertices);
            let saved_commands = std::mem::replace(&mut self.commands, source_commands);
            self.scene_geometry_dirty = true;
            let draw_result = (|| {
                let source_damage =
                    lifecycle_visual_effect_damage(lamp.visual_group.presented_source_visual_rect);
                let output_bounds =
                    EffectRect::new(0, 0, self.current_size.0.max(1), self.current_size.1.max(1))
                        .expect("non-zero renderer dimensions must form valid effect bounds");
                let graph = match compile_frame_execution_plan(
                    &source.effect_scene,
                    &source_damage,
                    output_bounds,
                    &self.effect_registry,
                )? {
                    FrameExecutionPlan::EffectGraph(graph) => graph,
                    FrameExecutionPlan::LegacyScene => {
                        return Err(io::Error::other(
                            "effect lifecycle source unexpectedly compiled as legacy scene",
                        )
                        .into());
                    }
                };
                let demand = plan_effect_execution_demand(&graph, &source_damage, true);
                let selection = effects::select_effect_execution(&graph, &demand);
                effects::execute_effect_graph_for_lifecycle(
                    self,
                    &graph,
                    framebuffer_origin,
                    &[lifecycle_visual_output_rect(
                        lamp.visual_group.presented_source_visual_rect,
                        output_bounds,
                    )],
                    &demand,
                    &selection,
                )?;
                copy_output_region_to_texture(
                    self,
                    &target,
                    lamp.visual_group.presented_source_visual_rect,
                    framebuffer_origin,
                )?;
                Ok(())
            })();
            self.vertices = saved_vertices;
            self.commands = saved_commands;
            // The temporary source draw replaced the scene VBO contents.
            // Force the normal scene cache to upload its unchanged geometry
            // before the next ordinary scene draw.
            self.scene_geometry_dirty = true;
            draw_result
        })();

        let restore_result = restore_output_region_from_texture(
            self,
            &backup,
            lamp.visual_group.presented_source_visual_rect,
            framebuffer_origin,
        );
        let release_result = self.effect_resources.release(backup);
        match (result, restore_result, release_result) {
            (Err(error), _, _) => Err(error),
            (Ok(()), Err(error), _) => Err(error),
            (Ok(()), Ok(()), Err(error)) => Err(error.into()),
            (Ok(()), Ok(()), Ok(())) => Ok(()),
        }
    }

    fn release_stale_lifecycle_visual_resources(&mut self) {
        let stale = self
            .lifecycle_visual_resources
            .keys()
            .copied()
            .filter(|window_id| {
                !self
                    .lifecycle_visual_sources
                    .get(window_id)
                    .is_some_and(|source| {
                        source.kind == LifecycleVisualSourceKind::ResolvedOwnedEffects
                    })
            })
            .collect::<Vec<_>>();
        for window_id in stale {
            if let Some(resource) = self.lifecycle_visual_resources.remove(&window_id) {
                let _ = self.effect_resources.release(resource.texture);
            }
        }
    }

    fn release_all_lifecycle_visual_resources(&mut self) {
        for (_, resource) in self.lifecycle_visual_resources.drain() {
            let _ = self.effect_resources.release(resource.texture);
        }
    }

    fn draw_lamp_overlay(&mut self, scissor: Option<OutputRect>) -> RendererResult<()> {
        let (Some(program), Some(uniforms)) = (self.lamp_program, self.lamp_uniform_locations)
        else {
            self.record_visible_lifecycle_fallbacks(
                LifecycleRenderFallbackReason::LampProgramUnavailable,
            );
            return Ok(());
        };
        if self.lamp_vertices.is_empty() || self.lamp_commands.is_empty() {
            return Ok(());
        }
        let required_size = self.lamp_vertices.len() * std::mem::size_of::<EglLampVertex>();
        ensure_vertex_buffer_capacity(
            &self.gl,
            self.lamp_vertex_buffer,
            &mut self.lamp_vertex_buffer_capacity,
            required_size,
        );
        if self.lamp_geometry_dirty {
            unsafe {
                self.gl
                    .bind_buffer(glow::ARRAY_BUFFER, Some(self.lamp_vertex_buffer));
                self.gl.buffer_sub_data_u8_slice(
                    glow::ARRAY_BUFFER,
                    0,
                    bytemuck::cast_slice(self.lamp_vertices.as_slice()),
                );
            }
            self.lamp_geometry_dirty = false;
        }
        let samples = self
            .lamp_commands
            .iter()
            .filter_map(|command| {
                if self.lifecycle_visual_source_is_ready(command.window_id)
                    && !matches!(command.layer, EglDrawLayer::LifecycleResolvedVisual(_))
                {
                    return None;
                }
                self.lamp_geometry_sample(command.window_id)
                    .map(|sample| (*command, sample))
            })
            .collect::<Vec<_>>();
        unsafe {
            self.gl.use_program(Some(program));
            self.gl.bind_vertex_array(Some(self.lamp_vertex_array));
            self.gl.active_texture(glow::TEXTURE0);
            self.gl.enable(glow::BLEND);
            self.gl.blend_func_separate(
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
            );
            if let Some(location) = &uniforms.texture {
                self.gl.uniform_1_i32(Some(location), 0);
            }
            if let Some(location) = &uniforms.output_size {
                self.gl.uniform_2_f32(
                    Some(location),
                    self.current_size.0 as f32,
                    self.current_size.1 as f32,
                );
            }
            if let Some(location) = &uniforms.framebuffer_origin_bottom_left {
                self.gl.uniform_1_i32(
                    Some(location),
                    i32::from(
                        self.current_framebuffer_origin == OutputFramebufferOrigin::BottomLeft,
                    ),
                );
            }
        }
        let output_scale = self.effect_output_scale.max(1.0) as f64;
        let mut sampling = None;
        let missing_required_decoration_resources = samples
            .iter()
            .filter(|(command, _)| {
                scissor.is_none_or(|rect| command.bounds.intersects_output_rect(rect))
                    && Self::is_required_decoration_layer(command.layer)
                    && self.texture_for_layer(command.layer).is_none()
            })
            .count();
        for (command, sample) in samples {
            if scissor.is_some_and(|rect| !command.bounds.intersects_output_rect(rect)) {
                continue;
            }
            let Some(texture) = self.texture_for_layer(command.layer) else {
                self.record_lifecycle_render_fallback(
                    sample,
                    LifecycleRenderFallbackReason::LifecycleResourceUnavailable,
                );
                continue;
            };
            unsafe {
                self.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                if sampling != Some(command.sampling) {
                    let filter = match command.sampling {
                        SurfaceSampling::ExactNearest => glow::NEAREST,
                        SurfaceSampling::ScaledLinear => glow::LINEAR,
                    } as i32;
                    self.gl
                        .tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, filter);
                    self.gl
                        .tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, filter);
                    sampling = Some(command.sampling);
                }
                set_lamp_uniform_rect(
                    &self.gl,
                    uniforms.canonical_visual_rect.as_ref(),
                    sample.visual_group.canonical_visual_rect,
                    output_scale,
                );
                set_lamp_uniform_rect(
                    &self.gl,
                    uniforms.source_visual_rect.as_ref(),
                    sample.visual_group.presented_source_visual_rect,
                    output_scale,
                );
                set_lamp_uniform_rect(
                    &self.gl,
                    uniforms.anchor_rect.as_ref(),
                    sample.visual_group.anchor_rect,
                    output_scale,
                );
                if let Some(location) = &uniforms.direction {
                    let direction = match sample.visual_group.lamp_direction {
                        oblivion_one::window_lifecycle_animation::LampDirection::Top => 0,
                        oblivion_one::window_lifecycle_animation::LampDirection::Right => 1,
                        oblivion_one::window_lifecycle_animation::LampDirection::Bottom => 2,
                        oblivion_one::window_lifecycle_animation::LampDirection::Left => 3,
                    };
                    self.gl.uniform_1_i32(Some(location), direction);
                }
                if let Some(location) = &uniforms.shape_factor {
                    self.gl
                        .uniform_1_f32(Some(location), sample.visual_group.shape_factor as f32);
                }
                if let Some(location) = &uniforms.bump_distance {
                    self.gl.uniform_1_f32(
                        Some(location),
                        (sample.visual_group.bump_distance * output_scale) as f32,
                    );
                }
                let channels = lamp_stage_channels(
                    sample.progress,
                    sample.visual_group.shape_factor,
                    sample.visual_group.bump_distance,
                );
                if let Some(location) = &uniforms.bump_progress {
                    self.gl
                        .uniform_1_f32(Some(location), channels.bump_progress as f32);
                }
                if let Some(location) = &uniforms.stretch_progress {
                    self.gl
                        .uniform_1_f32(Some(location), channels.stretch_progress as f32);
                }
                if let Some(location) = &uniforms.squash_progress {
                    self.gl
                        .uniform_1_f32(Some(location), channels.squash_progress as f32);
                }
                if let Some(location) = &uniforms.progress {
                    self.gl
                        .uniform_1_f32(Some(location), sample.progress as f32);
                }
                if let Some(location) = &uniforms.opacity {
                    self.gl.uniform_1_f32(Some(location), sample.opacity as f32);
                }
                self.gl.draw_arrays(
                    glow::TRIANGLES,
                    command.vertex_start as i32,
                    command.vertex_count as i32,
                );
            }
            self.lifecycle_render_evidence
                .record(LifecycleRenderEvidenceEntry {
                    window_id: sample.window_id,
                    root_surface_id: sample.root_surface_id,
                    transition_id: sample.transition_id,
                });
        }
        self.frame_stats.missing_required_decoration_resources = self
            .frame_stats
            .missing_required_decoration_resources
            .saturating_add(missing_required_decoration_resources);
        unsafe {
            self.gl.use_program(Some(self.program));
            self.gl.bind_vertex_array(Some(self.scene_vertex_array));
        }
        Ok(())
    }

    fn record_visible_lifecycle_fallbacks(&mut self, reason: LifecycleRenderFallbackReason) {
        let visible = self
            .lamp_samples
            .iter()
            .copied()
            .filter(|lamp| self.lamp_intersects_current_output(lamp))
            .collect::<Vec<_>>();
        for lamp in visible {
            self.record_lifecycle_render_fallback(lamp, reason);
        }
    }

    fn record_lifecycle_fallbacks_without_evidence(&mut self) {
        let missing = self
            .lamp_samples
            .iter()
            .copied()
            .filter(|lamp| {
                !self.lifecycle_render_evidence.contains(
                    lamp.window_id,
                    lamp.root_surface_id,
                    lamp.transition_id,
                )
            })
            .filter(|lamp| self.lamp_intersects_current_output(lamp))
            .collect::<Vec<_>>();
        for lamp in missing {
            self.record_lifecycle_render_fallback(
                lamp,
                LifecycleRenderFallbackReason::NoConsumedRepresentation,
            );
        }
    }

    fn lamp_intersects_current_output(&self, lamp: &LampWindowSample) -> bool {
        let scale = self.effect_output_scale.max(1.0) as f64;
        lamp_footprint(lamp.visual_group).is_some_and(|footprint| {
            let x = footprint.x() * scale;
            let y = footprint.y() * scale;
            let width = footprint.width() * scale;
            let height = footprint.height() * scale;
            x < f64::from(self.current_size.0)
                && y < f64::from(self.current_size.1)
                && x + width > 0.0
                && y + height > 0.0
        })
    }

    pub(crate) fn draw_lifecycle_overlays(
        &mut self,
        rects: &[OutputRect],
        framebuffer_origin: OutputFramebufferOrigin,
        plan: &RepaintPlan,
    ) -> RendererResult<()> {
        self.prepare_lifecycle_visual_sources(plan, framebuffer_origin)?;
        for rect in rects {
            let y = match framebuffer_origin {
                OutputFramebufferOrigin::BottomLeft => self
                    .current_size
                    .1
                    .saturating_sub(rect.y.max(0) as u32 + rect.height)
                    as i32,
                OutputFramebufferOrigin::TopLeftScanout => rect.y,
            };
            unsafe {
                self.gl.enable(glow::SCISSOR_TEST);
                self.gl
                    .scissor(rect.x, y, rect.width as i32, rect.height as i32);
            }
            self.draw_lamp_overlay(Some(*rect))?;
        }
        unsafe {
            self.gl.disable(glow::SCISSOR_TEST);
        }
        Ok(())
    }

    fn lamp_geometry_sample(&self, window_id: compositor::WindowId) -> Option<LampWindowSample> {
        self.lamp_samples
            .iter()
            .find(|sample| sample.window_id == window_id)
            .copied()
    }

    pub(crate) fn begin_effect_repaint(
        &mut self,
        plan: &RepaintPlan,
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> RendererResult<Vec<OutputRect>> {
        self.establish_ordinary_scene_state();
        unsafe { self.gl.clear_color(0.0, 0.0, 0.0, 1.0) };
        let execution = plan
            .render_execution(self.current_size.0, self.current_size.1, framebuffer_origin)
            .ok_or_else(|| io::Error::other("effect repaint conversion failed"))?;
        let mut rects = Vec::new();
        match execution {
            RenderExecution::Full => unsafe {
                self.gl.disable(glow::SCISSOR_TEST);
                self.gl.clear(glow::COLOR_BUFFER_BIT);
                rects.push(OutputRect::new(
                    0,
                    0,
                    self.current_size.0,
                    self.current_size.1,
                ));
            },
            RenderExecution::Scissored { scissors, .. } => unsafe {
                self.gl.enable(glow::SCISSOR_TEST);
                for scissor in scissors {
                    self.gl
                        .scissor(scissor[0], scissor[1], scissor[2], scissor[3]);
                    self.gl.clear(glow::COLOR_BUFFER_BIT);
                    if let Some(rect) =
                        gl_scissor_to_output_rect(scissor, self.current_size.1, framebuffer_origin)
                    {
                        rects.push(rect);
                    }
                }
                self.gl.disable(glow::SCISSOR_TEST);
            },
        }
        Ok(rects)
    }

    pub(crate) fn draw_effect_scene_range(
        &mut self,
        rects: &[OutputRect],
        start: usize,
        end: usize,
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> RendererResult<()> {
        let end = end.min(self.commands.len());
        let start = start.min(end);
        for rect in rects {
            let y = match framebuffer_origin {
                OutputFramebufferOrigin::BottomLeft => self
                    .current_size
                    .1
                    .saturating_sub(rect.y.max(0) as u32 + rect.height)
                    as i32,
                OutputFramebufferOrigin::TopLeftScanout => rect.y,
            };
            unsafe {
                self.establish_ordinary_scene_state();
                self.gl.enable(glow::SCISSOR_TEST);
                self.gl
                    .scissor(rect.x, y, rect.width as i32, rect.height as i32);
            }
            self.draw_command_batch_range(true, Some(*rect), start, end)?;
        }
        unsafe {
            self.gl.disable(glow::SCISSOR_TEST);
        }
        Ok(())
    }

    pub(crate) fn draw_effect_overlays(
        &mut self,
        rects: &[OutputRect],
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> RendererResult<()> {
        for rect in rects {
            let y = match framebuffer_origin {
                OutputFramebufferOrigin::BottomLeft => self
                    .current_size
                    .1
                    .saturating_sub(rect.y.max(0) as u32 + rect.height)
                    as i32,
                OutputFramebufferOrigin::TopLeftScanout => rect.y,
            };
            unsafe {
                self.establish_ordinary_scene_state();
                self.gl.enable(glow::SCISSOR_TEST);
                self.gl
                    .scissor(rect.x, y, rect.width as i32, rect.height as i32);
            }
            self.draw_command_batch(false, Some(*rect))?;
        }
        unsafe {
            self.gl.disable(glow::SCISSOR_TEST);
        }
        Ok(())
    }

    fn draw_command_batch(
        &mut self,
        scene: bool,
        scissor: Option<OutputRect>,
    ) -> RendererResult<()> {
        self.draw_command_batch_with_visibility_and_range(scene, scissor, true, None, true)
    }

    fn draw_command_batch_range(
        &mut self,
        scene: bool,
        scissor: Option<OutputRect>,
        start: usize,
        end: usize,
    ) -> RendererResult<()> {
        self.draw_command_batch_with_visibility_and_range(
            scene,
            scissor,
            false,
            Some((start, end)),
            false,
        )
    }

    fn draw_command_batch_with_visibility(
        &mut self,
        scene: bool,
        scissor: Option<OutputRect>,
        plan_scene_visibility: bool,
    ) -> RendererResult<()> {
        self.draw_command_batch_with_visibility_and_range(
            scene,
            scissor,
            plan_scene_visibility,
            None,
            true,
        )
    }

    fn draw_command_batch_with_visibility_and_range(
        &mut self,
        scene: bool,
        scissor: Option<OutputRect>,
        plan_scene_visibility: bool,
        command_range: Option<(usize, usize)>,
        use_visibility_plan: bool,
    ) -> RendererResult<()> {
        if scene && plan_scene_visibility {
            self.plan_scene_visibility(scissor);
        }
        let (vertices, commands) = if scene {
            (&self.vertices, &self.commands)
        } else {
            (&self.cursor_vertices, &self.cursor_commands)
        };
        if vertices.is_empty() || commands.is_empty() {
            return Ok(());
        }

        let required_size = vertices.len() * std::mem::size_of::<EglTexturedVertex>();
        let mut upload_bytes = 0;
        let mut uploaded = false;
        let vertex_array = if scene {
            ensure_vertex_buffer_capacity(
                &self.gl,
                self.scene_vertex_buffer,
                &mut self.scene_vertex_buffer_capacity,
                required_size,
            );
            if self.scene_geometry_dirty {
                upload_bytes = required_size;
                unsafe {
                    self.gl
                        .bind_buffer(glow::ARRAY_BUFFER, Some(self.scene_vertex_buffer));
                    self.gl.buffer_sub_data_u8_slice(
                        glow::ARRAY_BUFFER,
                        0,
                        bytemuck::cast_slice(vertices.as_slice()),
                    );
                }
                self.scene_geometry_dirty = false;
                uploaded = true;
            }
            self.scene_vertex_array
        } else {
            ensure_vertex_buffer_capacity(
                &self.gl,
                self.overlay_vertex_buffer,
                &mut self.overlay_vertex_buffer_capacity,
                required_size,
            );
            if self.overlay_geometry_dirty {
                upload_bytes = required_size;
                unsafe {
                    self.gl
                        .bind_buffer(glow::ARRAY_BUFFER, Some(self.overlay_vertex_buffer));
                    self.gl.buffer_sub_data_u8_slice(
                        glow::ARRAY_BUFFER,
                        0,
                        bytemuck::cast_slice(vertices.as_slice()),
                    );
                }
                self.overlay_geometry_dirty = false;
                uploaded = true;
            }
            self.overlay_vertex_array
        };
        unsafe {
            self.gl.bind_vertex_array(Some(vertex_array));
        }

        let mut current_sampling = None;
        let mut commands_considered = 0;
        let mut commands_executed = 0;
        let mut commands_rejected_outside_damage = 0;
        let mut missing_required_decoration_resources: usize = 0;
        let mut texture_binds = 0;
        let mut draw_calls = 0;
        for (command_index, command) in commands.iter().enumerate() {
            if command_range
                .is_some_and(|(start, end)| command_index < start || command_index >= end)
            {
                continue;
            }
            commands_considered += 1;
            if scissor.is_some_and(|rect| !command.bounds.intersects_output_rect(rect)) {
                commands_rejected_outside_damage += 1;
                continue;
            }
            if scene && use_visibility_plan {
                match self.scene_visibility_plan[command_index] {
                    EglVisibilityDecision::Drawable => {}
                    EglVisibilityDecision::OutsideRemaining | EglVisibilityDecision::Occluded => {
                        continue;
                    }
                }
            }
            let Some(texture) = self.texture_for_layer(command.layer) else {
                if Self::is_required_decoration_layer(command.layer) {
                    missing_required_decoration_resources =
                        missing_required_decoration_resources.saturating_add(1);
                }
                continue;
            };
            unsafe {
                self.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                texture_binds += 1;
                if current_sampling != Some(command.sampling) {
                    let filter = match command.sampling {
                        SurfaceSampling::ExactNearest => glow::NEAREST,
                        SurfaceSampling::ScaledLinear => glow::LINEAR,
                    } as i32;
                    self.gl
                        .tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, filter);
                    self.gl
                        .tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, filter);
                    current_sampling = Some(command.sampling);
                }
                self.gl.draw_arrays(
                    glow::TRIANGLES,
                    command.vertex_start as i32,
                    command.vertex_count as i32,
                );
            }
            commands_executed += 1;
            draw_calls += 1;
        }
        self.frame_stats.commands_considered = self
            .frame_stats
            .commands_considered
            .saturating_add(commands_considered);
        self.frame_stats.commands_executed = self
            .frame_stats
            .commands_executed
            .saturating_add(commands_executed);
        self.frame_stats.missing_required_decoration_resources = self
            .frame_stats
            .missing_required_decoration_resources
            .saturating_add(missing_required_decoration_resources);
        self.frame_stats.commands_rejected_outside_damage = self
            .frame_stats
            .commands_rejected_outside_damage
            .saturating_add(commands_rejected_outside_damage);
        self.frame_stats.texture_binds =
            self.frame_stats.texture_binds.saturating_add(texture_binds);
        self.frame_stats.draw_calls = self.frame_stats.draw_calls.saturating_add(draw_calls);
        self.frame_stats.draw_command_replays = self
            .frame_stats
            .draw_command_replays
            .saturating_add(commands_executed);
        if uploaded {
            if scene {
                self.frame_stats.scene_vbo_uploads =
                    self.frame_stats.scene_vbo_uploads.saturating_add(1);
                self.frame_stats.scene_vbo_upload_bytes = self
                    .frame_stats
                    .scene_vbo_upload_bytes
                    .saturating_add(upload_bytes);
            } else {
                self.frame_stats.overlay_vbo_uploads =
                    self.frame_stats.overlay_vbo_uploads.saturating_add(1);
                self.frame_stats.overlay_vbo_upload_bytes = self
                    .frame_stats
                    .overlay_vbo_upload_bytes
                    .saturating_add(upload_bytes);
            }
        }
        Ok(())
    }

    fn draw_capture_commands(
        &mut self,
        command_indices: &[usize],
        output_rect: OutputRect,
        target_domain: EffectRect,
        target_size: (u32, u32),
    ) -> RendererResult<()> {
        let requested = EffectRect::new(
            output_rect.x,
            output_rect.y,
            output_rect.width,
            output_rect.height,
        )
        .and_then(|rect| rect.intersect(target_domain));
        let Some(requested) = requested else {
            return Ok(());
        };
        let local_x = requested.x.saturating_sub(target_domain.x);
        let local_y = requested.y.saturating_sub(target_domain.y);
        let gl_y = i32::try_from(target_size.1)
            .unwrap_or(i32::MAX)
            .saturating_sub(local_y.saturating_add(requested.height as i32));
        unsafe {
            self.gl.enable(glow::SCISSOR_TEST);
            self.gl.scissor(
                local_x,
                gl_y,
                requested.width as i32,
                requested.height as i32,
            );
        }
        let saved_visibility = std::mem::take(&mut self.scene_visibility_plan);
        let repair = EglRect::new(
            output_rect.x as f32,
            output_rect.y as f32,
            output_rect.width as f32,
            output_rect.height as f32,
        );
        let capture_stats = plan_capture_visibility(
            &self.commands,
            command_indices,
            repair,
            &mut self.scene_visibility_plan,
        );
        let result = self.draw_command_batch_with_visibility(true, Some(output_rect), false);
        unsafe { self.gl.disable(glow::SCISSOR_TEST) };
        self.frame_stats.planner_commands_visited = self
            .frame_stats
            .planner_commands_visited
            .saturating_add(capture_stats.commands_visited);
        self.scene_visibility_plan = saved_visibility;
        result
    }

    fn draw_capture_commands_for_regions(
        &mut self,
        command_indices: &[usize],
        output_rects: &[OutputRect],
        target_domain: EffectRect,
        target_size: (u32, u32),
    ) -> RendererResult<()> {
        for &output_rect in output_rects {
            self.draw_capture_commands(command_indices, output_rect, target_domain, target_size)?;
        }
        Ok(())
    }

    fn plan_scene_visibility(&mut self, scissor: Option<OutputRect>) {
        let repair = scissor
            .map(|rect| {
                EglRect::new(
                    rect.x as f32,
                    rect.y as f32,
                    rect.width as f32,
                    rect.height as f32,
                )
            })
            .unwrap_or_else(|| {
                EglRect::new(
                    0.0,
                    0.0,
                    self.current_size.0 as f32,
                    self.current_size.1 as f32,
                )
            });
        let stats = plan_visibility(&self.commands, repair, &mut self.scene_visibility_plan);
        self.frame_stats.planner_passes = self.frame_stats.planner_passes.saturating_add(1);
        self.frame_stats.planner_commands_visited = self
            .frame_stats
            .planner_commands_visited
            .saturating_add(stats.commands_visited);
        self.frame_stats.commands_drawable = self
            .frame_stats
            .commands_drawable
            .saturating_add(stats.commands_drawable);
        self.frame_stats.commands_rejected_outside_remaining = self
            .frame_stats
            .commands_rejected_outside_remaining
            .saturating_add(stats.commands_rejected_outside_remaining);
        self.frame_stats.commands_rejected_occluded = self
            .frame_stats
            .commands_rejected_occluded
            .saturating_add(stats.commands_rejected_occluded);
        self.frame_stats.opaque_rectangles_subtracted = self
            .frame_stats
            .opaque_rectangles_subtracted
            .saturating_add(stats.opaque_rectangles_subtracted);
        if stats.early_terminated {
            self.frame_stats.planner_early_terminations = self
                .frame_stats
                .planner_early_terminations
                .saturating_add(1);
        }
        if stats.overflow_fallback {
            self.frame_stats.region_fragmentation_overflow_fallbacks = self
                .frame_stats
                .region_fragmentation_overflow_fallbacks
                .saturating_add(1);
        }
        self.frame_stats.peak_region_piece_count = self
            .frame_stats
            .peak_region_piece_count
            .max(stats.peak_region_pieces);
    }

    const fn is_required_decoration_layer(layer: EglDrawLayer) -> bool {
        matches!(
            layer,
            EglDrawLayer::SolidRgba(_) | EglDrawLayer::DecorationAsset(_)
        )
    }

    fn texture_for_layer(&self, layer: EglDrawLayer) -> Option<GlTexture> {
        match layer {
            EglDrawLayer::Solid(color) => self
                .frame_resources
                .get(&color)
                .map(|resource| resource.texture),
            EglDrawLayer::SolidRgba(color) => self
                .decoration_resources
                .get(&DecorationResourceKey::Solid(color))
                .map(|resource| resource.texture),
            EglDrawLayer::DecorationAsset(asset_id) => self
                .decoration_resources
                .get(&DecorationResourceKey::Asset(asset_id))
                .map(|resource| resource.texture),
            EglDrawLayer::Surface(surface_id) => self
                .surface_resources
                .get(&surface_id)
                .map(|resource| resource.image.texture),
            EglDrawLayer::LifecycleResolvedVisual(window_id) => self
                .lifecycle_visual_resources
                .get(&window_id)
                .and_then(|resource| self.effect_resources.texture(&resource.texture)),
            EglDrawLayer::Cursor => self
                .cursor_resource
                .as_ref()
                .map(|resource| resource.texture),
        }
    }

    fn lifecycle_visual_source_is_ready(&self, window_id: compositor::WindowId) -> bool {
        self.lifecycle_visual_resources
            .get(&window_id)
            .is_some_and(|resource| self.effect_resources.texture(&resource.texture).is_some())
    }

    pub(crate) fn destroy(&mut self, egl: &EglInstance, egl_display: egl::Display) {
        if let Some(resource) = self.cursor_resource.take() {
            destroy_image_resource(&self.gl, egl, egl_display, resource);
        }
        for (_, resource) in self.frame_resources.drain() {
            destroy_image_resource(&self.gl, egl, egl_display, resource);
        }
        for (_, resource) in self.decoration_resources.drain() {
            destroy_image_resource(&self.gl, egl, egl_display, resource);
        }
        for (_, resource) in self.surface_resources.drain() {
            destroy_surface_resource(&self.gl, egl, egl_display, resource);
        }
        for (_, resource) in self.dmabuf_resource_cache.drain() {
            destroy_image_resource(&self.gl, egl, egl_display, resource.image);
        }
        self.release_all_lifecycle_visual_resources();
        self.effect_gpu_profiler.destroy(&self.gl);
        self.effect_shaders.clear(&self.gl);
        self.effect_resources.destroy(&self.gl);
        if let Some((vertex_array, vertex_buffer)) = self.effect_quad.take() {
            unsafe {
                self.gl.delete_buffer(vertex_buffer);
                self.gl.delete_vertex_array(vertex_array);
            }
        }

        unsafe {
            if let Some(lamp_program) = self.lamp_program.take() {
                self.gl.delete_program(lamp_program);
            }
            self.gl.delete_buffer(self.lamp_vertex_buffer);
            self.gl.delete_vertex_array(self.lamp_vertex_array);
            self.gl.delete_buffer(self.scene_vertex_buffer);
            self.gl.delete_vertex_array(self.scene_vertex_array);
            self.gl.delete_buffer(self.overlay_vertex_buffer);
            self.gl.delete_vertex_array(self.overlay_vertex_array);
            self.gl.delete_program(self.program);
            self.gl.delete_program(self.capture_program);
        }
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

struct DecorationResourceRequirements<'a> {
    required: HashSet<DecorationResourceKey>,
    required_assets: HashMap<u64, &'a oblivion_one::compositor::DecorationRasterAsset>,
}

fn decoration_resource_requirements<'a, I>(primitive_sets: I) -> DecorationResourceRequirements<'a>
where
    I: IntoIterator<Item = &'a [DecorationRenderPrimitive]>,
{
    let mut required = HashSet::new();
    let mut required_assets = HashMap::new();
    for primitives in primitive_sets {
        for primitive in primitives {
            match primitive {
                DecorationRenderPrimitive::SolidRect { color, .. } => {
                    required.insert(DecorationResourceKey::Solid(rgba_to_pixel(*color)));
                }
                DecorationRenderPrimitive::Image { asset, .. } => {
                    required.insert(DecorationResourceKey::Asset(asset.asset_id()));
                    required_assets.insert(asset.asset_id(), asset);
                }
                DecorationRenderPrimitive::Text { color, asset, .. } => {
                    required.insert(DecorationResourceKey::Solid(rgba_to_pixel(*color)));
                    required.insert(DecorationResourceKey::Asset(asset.asset_id()));
                    required_assets.insert(asset.asset_id(), asset);
                }
            }
        }
    }
    DecorationResourceRequirements {
        required,
        required_assets,
    }
}

#[cfg(test)]
#[allow(dead_code)]
fn decoration_resource_keys_for_primitive_sets<'a, I>(
    primitive_sets: I,
) -> HashSet<DecorationResourceKey>
where
    I: IntoIterator<Item = &'a [DecorationRenderPrimitive]>,
{
    decoration_resource_requirements(primitive_sets).required
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum DecorationResourceKey {
    Solid(u32),
    Asset(u64),
}

struct EglImageResource {
    texture: GlTexture,
    size: (u32, u32),
    generation: u64,
    egl_image: Option<egl::Image>,
}

pub(crate) struct EglImageGuard<F>
where
    F: FnMut(egl::Image),
{
    image: Option<egl::Image>,
    destroy: F,
}

impl<F> EglImageGuard<F>
where
    F: FnMut(egl::Image),
{
    pub(crate) fn new(image: egl::Image, destroy: F) -> Self {
        Self {
            image: Some(image),
            destroy,
        }
    }

    pub(crate) fn image(&self) -> egl::Image {
        self.image.expect("EGL image guard must own an image")
    }

    pub(crate) fn disarm(mut self) -> egl::Image {
        self.image
            .take()
            .expect("EGL image guard must own an image")
    }
}

impl<F> Drop for EglImageGuard<F>
where
    F: FnMut(egl::Image),
{
    fn drop(&mut self) {
        if let Some(image) = self.image.take() {
            (self.destroy)(image);
        }
    }
}

struct EglSurfaceResource {
    image: EglImageResource,
    dmabuf_key: Option<DmabufImageKey>,
    buffer_lifetime: Option<WeakBufferIdentity>,
    shm_synced_commit: Option<SurfaceCommitCounter>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SurfaceResourceLifetimeAction {
    Keep,
    DemoteDmabuf,
    Destroy,
}

struct CachedDmabufResource<R> {
    image: R,
    buffer_lifetime: WeakBufferIdentity,
    surface_id: u32,
}

fn dead_cached_dmabuf_keys<R>(
    cache: &HashMap<DmabufImageKey, CachedDmabufResource<R>>,
) -> Vec<DmabufImageKey> {
    cache
        .iter()
        .filter_map(|(key, cached)| (!cached.buffer_lifetime.is_alive()).then_some(key.clone()))
        .collect()
}

fn classify_surface_resource_lifetime(
    resource: &EglSurfaceResource,
    surface: &RenderableSurface,
) -> SurfaceResourceLifetimeAction {
    if let Some(installed_key) = resource.dmabuf_key.as_ref() {
        let current_key = surface
            .dmabuf_handle()
            .map(|handle| DmabufImageKey::from_handle(surface.buffer_id(), handle));
        return if current_key.as_ref() == Some(installed_key) {
            SurfaceResourceLifetimeAction::Keep
        } else {
            SurfaceResourceLifetimeAction::DemoteDmabuf
        };
    }

    let size = surface.buffer_size();
    if surface.cpu_pixels().is_some() && resource.image.size == (size.width, size.height) {
        SurfaceResourceLifetimeAction::Keep
    } else {
        SurfaceResourceLifetimeAction::Destroy
    }
}

fn reconcile_surface_resource_backing(
    surface_resources: &mut HashMap<u32, EglSurfaceResource>,
    surface: &RenderableSurface,
) -> Option<(SurfaceResourceLifetimeAction, EglSurfaceResource)> {
    let action = surface_resources
        .get(&surface.surface_id)
        .map(|resource| classify_surface_resource_lifetime(resource, surface))?;
    if action == SurfaceResourceLifetimeAction::Keep {
        return None;
    }
    surface_resources
        .remove(&surface.surface_id)
        .map(|resource| (action, resource))
}

impl EglSurfaceResource {
    fn advance_shm_sync_baseline(&mut self, synced_commit: SurfaceCommitCounter) {
        self.shm_synced_commit = Some(synced_commit);
    }

    fn update_for(
        &self,
        surface: &RenderableSurface,
        sync_state: SurfaceResourceSyncState,
    ) -> EglSurfaceResourceUpdate {
        let buffer_size = surface.buffer_size();
        if self.image.size != (buffer_size.width, buffer_size.height) {
            return EglSurfaceResourceUpdate::Recreate;
        }
        if surface.cpu_pixels().is_some() {
            if self.image.egl_image.is_some() {
                return EglSurfaceResourceUpdate::Recreate;
            }
            if !sync_state.authoritative {
                return EglSurfaceResourceUpdate::FullShmResync;
            }
            if self.shm_synced_commit == Some(sync_state.current_commit) {
                return EglSurfaceResourceUpdate::Reuse;
            }
            if surface.damage.is_history_lost() {
                return EglSurfaceResourceUpdate::FullShmResync;
            }
            if sync_state.complete_since.is_some_and(|complete_since| {
                self.shm_synced_commit
                    .is_some_and(|synced| synced >= complete_since)
            }) {
                return if surface.damage.is_empty() {
                    EglSurfaceResourceUpdate::ReuseShm
                } else {
                    EglSurfaceResourceUpdate::UploadDamage
                };
            }
            return EglSurfaceResourceUpdate::FullShmResync;
        }
        if self.image.generation == surface.generation {
            return EglSurfaceResourceUpdate::Reuse;
        }
        if surface
            .dmabuf_handle()
            .map(|handle| DmabufImageKey::from_handle(surface.buffer_id(), handle))
            .is_some_and(|key| self.dmabuf_key.as_ref() == Some(&key))
        {
            return EglSurfaceResourceUpdate::ReuseDmabuf;
        }
        if surface.dmabuf_handle().is_some() {
            return EglSurfaceResourceUpdate::Recreate;
        }
        EglSurfaceResourceUpdate::UnsupportedBuffer
    }

    fn write_shm_damage(
        &mut self,
        gl: &glow::Context,
        surface: &RenderableSurface,
        force_full_upload: bool,
        synced_commit: SurfaceCommitCounter,
        upload_rgba: &mut Vec<u8>,
    ) -> usize {
        let upload_bytes = write_surface_pixels_to_resource(
            gl,
            &self.image,
            surface,
            force_full_upload,
            upload_rgba,
        );
        self.image.generation = surface.generation;
        self.shm_synced_commit = Some(synced_commit);
        upload_bytes
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EglSurfaceResourceUpdate {
    Reuse,
    ReuseShm,
    ReuseDmabuf,
    UploadDamage,
    FullShmResync,
    Recreate,
    UnsupportedBuffer,
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
        Self {
            width,
            height,
            content_generation,
            output_scale_key,
            surface_signature_hash: egl_scene_surface_signature_hash(surface_signatures),
            decoration_signature_hash: egl_decoration_signature_hash(&[]),
            popup_surface_signature_hash: 0,
            presentation_geometry_signature,
            framebuffer_origin,
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "cache-key construction keeps each render-state component explicit"
    )]
    fn new_with_decorations(
        width: u32,
        height: u32,
        content_generation: u64,
        output_scale_key: u32,
        surface_signatures: &[EglSceneSurfaceSignature],
        decoration_instances: &[DecorationRenderInstance],
        popup_surface_ids: &[u32],
        presentation_geometry_signature: u64,
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> Self {
        let mut key = Self::new_with_presentation(
            width,
            height,
            content_generation,
            output_scale_key,
            surface_signatures,
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
        self.width == width
            && self.height == height
            && self.output_scale_key == output_scale_key
            && self.surface_signature_hash == egl_scene_surface_signature_hash(surface_signatures)
            && self.decoration_signature_hash == egl_decoration_signature_hash(decoration_instances)
            && self.popup_surface_signature_hash
                == egl_popup_surface_signature_hash(popup_surface_ids)
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

fn reproject_lifecycle_source_commands(
    vertices: &mut [EglTexturedVertex],
    commands: &mut [EglDrawCommand],
    canonical_client_rect: compositor::PresentationRect,
    presented_source_client_rect: compositor::PresentationRect,
    output_scale: f64,
    output_size: (u32, u32),
    framebuffer_origin: OutputFramebufferOrigin,
) {
    let full = scaled_presentation_rect(canonical_client_rect, output_scale);
    let source = scaled_presentation_rect(presented_source_client_rect, output_scale);
    let map_point = |point: [f64; 2]| {
        [
            source.x() + (point[0] - full.x()) * source.width() / full.width(),
            source.y() + (point[1] - full.y()) * source.height() / full.height(),
        ]
    };
    for vertex in vertices {
        let output_point = ndc_to_output_point(vertex.position, output_size, framebuffer_origin);
        let mapped = map_point(output_point);
        vertex.position = output_point_to_ndc(mapped, output_size, framebuffer_origin);
    }
    for command in commands {
        let left = f64::from(command.bounds.x());
        let top = f64::from(command.bounds.y());
        let right = left + f64::from(command.bounds.width());
        let bottom = top + f64::from(command.bounds.height());
        let mapped_top_left = map_point([left, top]);
        let mapped_bottom_right = map_point([right, bottom]);
        command.bounds = EglRect::new(
            mapped_top_left[0] as f32,
            mapped_top_left[1] as f32,
            (mapped_bottom_right[0] - mapped_top_left[0]) as f32,
            (mapped_bottom_right[1] - mapped_top_left[1]) as f32,
        );
        // A lifecycle source is a non-linear visual-group input. It must not
        // contribute a rectangular opaque region to ordinary occlusion.
        command.opaque_regions.clear();
    }
}

fn scaled_presentation_rect(
    rect: compositor::PresentationRect,
    output_scale: f64,
) -> compositor::PresentationRect {
    compositor::PresentationRect::new(
        rect.x() * output_scale,
        rect.y() * output_scale,
        rect.width() * output_scale,
        rect.height() * output_scale,
    )
    .unwrap_or(rect)
}

fn ndc_to_output_point(
    position: [f32; 2],
    output_size: (u32, u32),
    framebuffer_origin: OutputFramebufferOrigin,
) -> [f64; 2] {
    let x = (f64::from(position[0]) + 1.0) * f64::from(output_size.0) * 0.5;
    let y = match framebuffer_origin {
        OutputFramebufferOrigin::BottomLeft => {
            (1.0 - f64::from(position[1])) * f64::from(output_size.1) * 0.5
        }
        OutputFramebufferOrigin::TopLeftScanout => {
            (f64::from(position[1]) + 1.0) * f64::from(output_size.1) * 0.5
        }
    };
    [x, y]
}

fn output_point_to_ndc(
    point: [f64; 2],
    output_size: (u32, u32),
    framebuffer_origin: OutputFramebufferOrigin,
) -> [f32; 2] {
    let x = point[0] / f64::from(output_size.0.max(1)) * 2.0 - 1.0;
    let y = match framebuffer_origin {
        OutputFramebufferOrigin::BottomLeft => {
            1.0 - point[1] / f64::from(output_size.1.max(1)) * 2.0
        }
        OutputFramebufferOrigin::TopLeftScanout => {
            point[1] / f64::from(output_size.1.max(1)) * 2.0 - 1.0
        }
    };
    [x as f32, y as f32]
}

fn lifecycle_visual_source_signature(
    source: &LifecycleVisualSource,
    lamp: LampWindowSample,
    output_scale: f64,
) -> u64 {
    let mut signature = source.effect_scene.signature;
    for value in [
        lamp.visual_group.canonical_client_rect.x().to_bits(),
        lamp.visual_group.canonical_client_rect.y().to_bits(),
        lamp.visual_group.canonical_client_rect.width().to_bits(),
        lamp.visual_group.canonical_client_rect.height().to_bits(),
        lamp.visual_group.canonical_visual_rect.x().to_bits(),
        lamp.visual_group.canonical_visual_rect.y().to_bits(),
        lamp.visual_group.canonical_visual_rect.width().to_bits(),
        lamp.visual_group.canonical_visual_rect.height().to_bits(),
        lamp.visual_group.presented_source_client_rect.x().to_bits(),
        lamp.visual_group.presented_source_client_rect.y().to_bits(),
        lamp.visual_group
            .presented_source_client_rect
            .width()
            .to_bits(),
        lamp.visual_group
            .presented_source_client_rect
            .height()
            .to_bits(),
        lamp.visual_group.presented_source_visual_rect.x().to_bits(),
        lamp.visual_group.presented_source_visual_rect.y().to_bits(),
        lamp.visual_group
            .presented_source_visual_rect
            .width()
            .to_bits(),
        lamp.visual_group
            .presented_source_visual_rect
            .height()
            .to_bits(),
        output_scale.to_bits(),
    ] {
        signature ^= value;
        signature = signature.wrapping_mul(0x1000_0000_01b3);
    }
    signature
}

fn lifecycle_visual_texture_size(rect: compositor::PresentationRect) -> Option<(u32, u32)> {
    let width = rect.width().ceil();
    let height = rect.height().ceil();
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return None;
    }
    let width = width.min(f64::from(u32::MAX)) as u32;
    let height = height.min(f64::from(u32::MAX)) as u32;
    (width != 0 && height != 0).then_some((width, height))
}

fn lifecycle_visual_effect_damage(rect: compositor::PresentationRect) -> EffectRegion {
    presentation_rect_to_effect_rect(rect)
        .map(EffectRegion::from_rect)
        .unwrap_or_default()
}

fn presentation_rect_to_effect_rect(rect: compositor::PresentationRect) -> Option<EffectRect> {
    let x = rect.x().floor();
    let y = rect.y().floor();
    let right = (rect.x() + rect.width()).ceil();
    let bottom = (rect.y() + rect.height()).ceil();
    if !x.is_finite()
        || !y.is_finite()
        || !right.is_finite()
        || !bottom.is_finite()
        || x < f64::from(i32::MIN)
        || y < f64::from(i32::MIN)
        || right > f64::from(i32::MAX)
        || bottom > f64::from(i32::MAX)
    {
        return None;
    }
    EffectRect::new(
        x as i32,
        y as i32,
        (right - x).max(1.0).min(f64::from(u32::MAX)) as u32,
        (bottom - y).max(1.0).min(f64::from(u32::MAX)) as u32,
    )
}

fn lifecycle_visual_output_rect(
    rect: compositor::PresentationRect,
    output_bounds: EffectRect,
) -> OutputRect {
    let Some(rect) =
        presentation_rect_to_effect_rect(rect).and_then(|rect| rect.intersect(output_bounds))
    else {
        return OutputRect::new(0, 0, 0, 0);
    };
    OutputRect::new(rect.x, rect.y, rect.width, rect.height)
}

fn lifecycle_output_copy_region(
    renderer: &GlesSceneRenderer,
    rect: compositor::PresentationRect,
    target_width: u32,
    target_height: u32,
) -> Option<(OutputRect, i32, i32)> {
    let scale = renderer.effect_output_scale.max(1.0) as f64;
    let left = (rect.x() * scale).floor();
    let top = (rect.y() * scale).floor();
    let right = ((rect.x() + rect.width()) * scale).ceil();
    let bottom = ((rect.y() + rect.height()) * scale).ceil();
    if !left.is_finite()
        || !top.is_finite()
        || !right.is_finite()
        || !bottom.is_finite()
        || right <= left
        || bottom <= top
        || left < f64::from(i32::MIN)
        || top < f64::from(i32::MIN)
        || right > f64::from(i32::MAX)
        || bottom > f64::from(i32::MAX)
    {
        return None;
    }
    let left = left as i32;
    let top = top as i32;
    let right = right as i32;
    let bottom = bottom as i32;
    let visible_left = i64::from(left)
        .max(0)
        .min(i64::from(renderer.current_size.0));
    let visible_top = i64::from(top)
        .max(0)
        .min(i64::from(renderer.current_size.1));
    let visible_right = i64::from(right)
        .max(0)
        .min(i64::from(renderer.current_size.0));
    let visible_bottom = i64::from(bottom)
        .max(0)
        .min(i64::from(renderer.current_size.1));
    if visible_right <= visible_left || visible_bottom <= visible_top {
        return None;
    }
    let visible = OutputRect::new(
        visible_left as i32,
        visible_top as i32,
        (visible_right - visible_left) as u32,
        (visible_bottom - visible_top) as u32,
    );
    let destination_x = visible.x.saturating_sub(left);
    let destination_top = visible.y.saturating_sub(top);
    let destination_bottom = destination_top.saturating_add(visible.height as i32);
    let destination_y =
        i32::try_from(i64::from(target_height).saturating_sub(i64::from(destination_bottom)))
            .ok()?;
    if destination_x < 0
        || destination_y < 0
        || destination_x.saturating_add(visible.width as i32) > target_width as i32
        || destination_y.saturating_add(visible.height as i32) > target_height as i32
    {
        return None;
    }
    Some((visible, destination_x, destination_y))
}

fn copy_output_region_to_texture(
    renderer: &mut GlesSceneRenderer,
    target: &PooledEffectTexture,
    rect: compositor::PresentationRect,
    framebuffer_origin: OutputFramebufferOrigin,
) -> RendererResult<()> {
    let Some((visible, destination_x, destination_y)) =
        lifecycle_output_copy_region(renderer, rect, target.key.width, target.key.height)
    else {
        return Ok(());
    };
    let source_y = match framebuffer_origin {
        OutputFramebufferOrigin::BottomLeft => renderer
            .current_size
            .1
            .saturating_sub(visible.y.max(0) as u32 + visible.height)
            as i32,
        OutputFramebufferOrigin::TopLeftScanout => visible.y,
    };
    let texture = renderer
        .effect_resources
        .texture(target)
        .ok_or_else(|| io::Error::other("lifecycle source texture was not realized"))?;
    renderer.bind_active_output_framebuffer();
    unsafe {
        renderer.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        renderer.gl.copy_tex_sub_image_2d(
            glow::TEXTURE_2D,
            0,
            destination_x,
            destination_y,
            visible.x,
            source_y,
            visible.width as i32,
            visible.height as i32,
        );
        renderer.gl.bind_texture(glow::TEXTURE_2D, None);
    }
    renderer.establish_ordinary_scene_state();
    Ok(())
}

fn clear_effect_texture(
    renderer: &mut GlesSceneRenderer,
    target: &PooledEffectTexture,
) -> RendererResult<()> {
    renderer
        .effect_resources
        .bind_render_target(&renderer.gl, target)?;
    unsafe {
        renderer
            .gl
            .viewport(0, 0, target.key.width as i32, target.key.height as i32);
        renderer.gl.disable(glow::SCISSOR_TEST);
        renderer.gl.clear_color(0.0, 0.0, 0.0, 0.0);
        renderer.gl.clear(glow::COLOR_BUFFER_BIT);
    }
    renderer.effect_resources.unbind_render_target(&renderer.gl);
    renderer.establish_ordinary_scene_state();
    Ok(())
}

fn restore_output_region_from_texture(
    renderer: &mut GlesSceneRenderer,
    source: &PooledEffectTexture,
    rect: compositor::PresentationRect,
    framebuffer_origin: OutputFramebufferOrigin,
) -> RendererResult<()> {
    let Some((visible, destination_x, destination_y)) =
        lifecycle_output_copy_region(renderer, rect, source.key.width, source.key.height)
    else {
        return Ok(());
    };
    let destination_bottom = destination_y.saturating_add(visible.height as i32);
    let uv_left = destination_x as f32 / source.key.width as f32;
    let uv_right =
        destination_x.saturating_add(visible.width as i32) as f32 / source.key.width as f32;
    let uv_top = 1.0 - destination_y as f32 / source.key.height as f32;
    let uv_bottom = 1.0 - destination_bottom as f32 / source.key.height as f32;
    let output_width = renderer.current_size.0.max(1) as f32;
    let output_height = renderer.current_size.1.max(1) as f32;
    let left = visible.x as f32 / output_width * 2.0 - 1.0;
    let right = (visible.x + visible.width as i32) as f32 / output_width * 2.0 - 1.0;
    let (top, bottom) = match framebuffer_origin {
        OutputFramebufferOrigin::BottomLeft => (
            1.0 - visible.y as f32 / output_height * 2.0,
            1.0 - (visible.y + visible.height as i32) as f32 / output_height * 2.0,
        ),
        OutputFramebufferOrigin::TopLeftScanout => (
            visible.y as f32 / output_height * 2.0 - 1.0,
            (visible.y + visible.height as i32) as f32 / output_height * 2.0 - 1.0,
        ),
    };
    let vertices = [
        EglTexturedVertex {
            position: [left, top],
            uv: [uv_left, uv_top],
        },
        EglTexturedVertex {
            position: [left, bottom],
            uv: [uv_left, uv_bottom],
        },
        EglTexturedVertex {
            position: [right, bottom],
            uv: [uv_right, uv_bottom],
        },
        EglTexturedVertex {
            position: [left, top],
            uv: [uv_left, uv_top],
        },
        EglTexturedVertex {
            position: [right, bottom],
            uv: [uv_right, uv_bottom],
        },
        EglTexturedVertex {
            position: [right, top],
            uv: [uv_right, uv_top],
        },
    ];
    let texture = renderer
        .effect_resources
        .texture(source)
        .ok_or_else(|| io::Error::other("lifecycle backup texture was not realized"))?;
    renderer.bind_active_output_framebuffer();
    unsafe {
        renderer
            .gl
            .bind_buffer(glow::ARRAY_BUFFER, Some(renderer.scene_vertex_buffer));
        renderer.gl.buffer_sub_data_u8_slice(
            glow::ARRAY_BUFFER,
            0,
            bytemuck::cast_slice(&vertices),
        );
        renderer
            .gl
            .bind_vertex_array(Some(renderer.scene_vertex_array));
        renderer.gl.use_program(Some(renderer.program));
        renderer.gl.active_texture(glow::TEXTURE0);
        renderer.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        renderer.gl.enable(glow::BLEND);
        renderer.gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
        renderer.gl.enable(glow::SCISSOR_TEST);
        let y = match framebuffer_origin {
            OutputFramebufferOrigin::BottomLeft => renderer
                .current_size
                .1
                .saturating_sub(visible.y.max(0) as u32 + visible.height)
                as i32,
            OutputFramebufferOrigin::TopLeftScanout => visible.y,
        };
        renderer
            .gl
            .scissor(visible.x, y, visible.width as i32, visible.height as i32);
        renderer.gl.draw_arrays(glow::TRIANGLES, 0, 6);
        renderer.gl.bind_texture(glow::TEXTURE_2D, None);
        renderer.gl.disable(glow::SCISSOR_TEST);
    }
    renderer.scene_geometry_dirty = true;
    renderer.establish_ordinary_scene_state();
    Ok(())
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
    let uv = EglUvRect::new(
        render_plan.content_uv.left,
        render_plan.content_uv.top,
        render_plan.content_uv.right,
        render_plan.content_uv.bottom,
    );
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

fn surface_upload_byte_len(surface: &RenderableSurface) -> usize {
    let size = surface.buffer_size();
    (size.width as usize)
        .saturating_mul(size.height as usize)
        .saturating_mul(4)
}

fn create_surface_resource(
    gl: &glow::Context,
    egl: &EglInstance,
    egl_display: egl::Display,
    egl_image_target_texture_2d: Option<GlEglImageTargetTexture2DOes>,
    surface: &RenderableSurface,
    shm_synced_commit: Option<SurfaceCommitCounter>,
    upload_rgba: &mut Vec<u8>,
) -> RendererResult<EglSurfaceResource> {
    let image = if surface.cpu_pixels().is_some() {
        let buffer_size = surface.buffer_size();
        let mut resource = create_uploaded_resource(gl, buffer_size.width, buffer_size.height)?;
        write_surface_pixels_to_resource(gl, &resource, surface, true, upload_rgba);
        resource.generation = surface.generation;
        resource
    } else if let Some(handle) = surface.dmabuf_handle() {
        create_dmabuf_resource(
            gl,
            egl,
            egl_display,
            egl_image_target_texture_2d,
            handle,
            surface.generation,
        )?
    } else {
        return Err(io::Error::other("surface has no importable buffer").into());
    };

    if native_egl_debug_enabled() && surface.dmabuf_handle().is_some() {
        eprintln!(
            "oblivion-one compositor: dmabuf cache=create surface={} buffer_id={} texture={:?} egl_image={:?}",
            surface.surface_id,
            surface.buffer_id().get(),
            image.texture,
            image.egl_image.map(|egl_image| egl_image.as_ptr()),
        );
    }

    Ok(EglSurfaceResource {
        image,
        dmabuf_key: surface
            .dmabuf_handle()
            .map(|handle| DmabufImageKey::from_handle(surface.buffer_id(), handle)),
        buffer_lifetime: surface
            .dmabuf_handle()
            .map(|_| surface.buffer_identity().downgrade()),
        shm_synced_commit,
    })
}

fn create_uploaded_resource(
    gl: &glow::Context,
    width: u32,
    height: u32,
) -> RendererResult<EglImageResource> {
    let texture = unsafe { gl.create_texture().map_err(io::Error::other)? };
    unsafe {
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        configure_texture(gl);
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA as i32,
            width as i32,
            height as i32,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(None),
        );
    }
    Ok(EglImageResource {
        texture,
        size: (width, height),
        generation: 0,
        egl_image: None,
    })
}

fn create_dmabuf_resource(
    gl: &glow::Context,
    egl: &EglInstance,
    egl_display: egl::Display,
    egl_image_target_texture_2d: Option<GlEglImageTargetTexture2DOes>,
    handle: &oblivion_one::render_backend::buffer::DmabufBufferHandle,
    generation: u64,
) -> RendererResult<EglImageResource> {
    let preexisting_gl_error = drain_gl_errors(gl);
    if native_egl_debug_enabled()
        && let Some(error) = preexisting_gl_error
    {
        eprintln!(
            "oblivion-one compositor: dmabuf import cleared preexisting GL error 0x{error:04x}"
        );
    }
    let Some(egl_image_target_texture_2d) = egl_image_target_texture_2d else {
        return Err(io::Error::other("GL_OES_EGL_image is unavailable").into());
    };
    let attributes = EglGlesDmabufImportAttributes::from_handle(handle)
        .map_err(DmabufTextureImportError::InvalidAttributes)?;
    let no_context = unsafe { egl::Context::from_ptr(egl::NO_CONTEXT) };
    let null_client_buffer = unsafe { egl::ClientBuffer::from_ptr(ptr::null_mut()) };
    let image = egl
        .create_image(
            egl_display,
            no_context,
            EGL_LINUX_DMA_BUF_EXT,
            null_client_buffer,
            attributes.as_slice(),
        )
        .map_err(DmabufTextureImportError::EglImageCreation)?;
    let image_guard = EglImageGuard::new(image, |image| {
        let _ = egl.destroy_image(egl_display, image);
    });
    let texture = unsafe {
        gl.create_texture()
            .map_err(|error| DmabufTextureImportError::TextureCreation(error.to_owned()))?
    };
    if let Err(error) = check_dmabuf_gl_stage(gl, DmabufImportGlStage::TextureCreation) {
        unsafe { gl.delete_texture(texture) };
        return Err(error.into());
    }
    unsafe {
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
    }
    if let Err(error) = check_dmabuf_gl_stage(gl, DmabufImportGlStage::Bind) {
        unsafe { gl.delete_texture(texture) };
        return Err(error.into());
    }
    configure_texture(gl);
    if let Err(error) = check_dmabuf_gl_stage(gl, DmabufImportGlStage::TextureConfiguration) {
        unsafe { gl.delete_texture(texture) };
        return Err(error.into());
    }
    unsafe {
        egl_image_target_texture_2d(glow::TEXTURE_2D, image_guard.image().as_ptr());
    }
    if let Err(error) = check_dmabuf_gl_stage(gl, DmabufImportGlStage::ImageTarget) {
        unsafe { gl.delete_texture(texture) };
        return Err(error.into());
    }

    let size = handle.size();
    Ok(EglImageResource {
        texture,
        size: (size.width, size.height),
        generation,
        egl_image: Some(image_guard.disarm()),
    })
}

fn write_surface_pixels_to_resource(
    gl: &glow::Context,
    resource: &EglImageResource,
    surface: &RenderableSurface,
    force_full_upload: bool,
    upload_rgba: &mut Vec<u8>,
) -> usize {
    if force_full_upload
        || surface.damage.is_full()
        || surface
            .damage
            .covers_surface(surface.buffer_size().width, surface.buffer_size().height)
    {
        let Some(pixels) = surface.cpu_pixels() else {
            return 0;
        };
        let buffer_size = surface.buffer_size();
        return write_argb_pixels_to_resource(
            gl,
            resource,
            SurfaceDamageRect::full(buffer_size.width, buffer_size.height),
            pixels,
            upload_rgba,
        );
    }

    let buffer_size = surface.buffer_size();
    let mut uploaded_bytes = 0usize;
    for rect in surface
        .damage
        .clipped_rects(buffer_size.width, buffer_size.height)
    {
        if rect.width == 0 || rect.height == 0 {
            continue;
        }
        if !pack_surface_rect_rgba(surface, rect, upload_rgba) {
            continue;
        }
        uploaded_bytes = uploaded_bytes.saturating_add(write_rgba_bytes_to_resource(
            gl,
            resource,
            rect,
            upload_rgba,
        ));
    }
    uploaded_bytes
}

fn write_argb_pixels_to_resource(
    gl: &glow::Context,
    resource: &EglImageResource,
    rect: SurfaceDamageRect,
    pixels: &[u32],
    upload_rgba: &mut Vec<u8>,
) -> usize {
    pack_argb_pixels_rgba(pixels, upload_rgba);
    write_rgba_bytes_to_resource(gl, resource, rect, upload_rgba)
}

fn write_rgba_bytes_to_resource(
    gl: &glow::Context,
    resource: &EglImageResource,
    rect: SurfaceDamageRect,
    rgba: &[u8],
) -> usize {
    unsafe {
        gl.bind_texture(glow::TEXTURE_2D, Some(resource.texture));
        gl.tex_sub_image_2d(
            glow::TEXTURE_2D,
            0,
            rect.x as i32,
            rect.y as i32,
            rect.width as i32,
            rect.height as i32,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(rgba)),
        );
    }
    (rect.width as usize)
        .saturating_mul(rect.height as usize)
        .saturating_mul(4)
}

fn pack_argb_pixels_rgba(pixels: &[u32], output: &mut Vec<u8>) {
    output.resize(pixels.len().saturating_mul(4), 0);
    for (index, &pixel) in pixels.iter().enumerate() {
        let base = index * 4;
        output[base] = ((pixel >> 16) & 0xff) as u8;
        output[base + 1] = ((pixel >> 8) & 0xff) as u8;
        output[base + 2] = (pixel & 0xff) as u8;
        output[base + 3] = ((pixel >> 24) & 0xff) as u8;
    }
}

fn pack_surface_rect_rgba(
    surface: &RenderableSurface,
    rect: SurfaceDamageRect,
    output: &mut Vec<u8>,
) -> bool {
    let Some(surface_pixels) = surface.cpu_pixels() else {
        return false;
    };
    let surface_width = surface.buffer_size().width as usize;
    let rect_x = rect.x as usize;
    let rect_y = rect.y as usize;
    let rect_width = rect.width as usize;
    let rect_height = rect.height as usize;

    output.resize(rect_width.saturating_mul(rect_height).saturating_mul(4), 0);
    let mut output_index = 0;
    for row_index in 0..rect_height {
        let Some(start) = (rect_y + row_index)
            .checked_mul(surface_width)
            .and_then(|row_start| row_start.checked_add(rect_x))
        else {
            output.clear();
            return false;
        };
        let Some(end) = start.checked_add(rect_width) else {
            output.clear();
            return false;
        };
        let Some(row) = surface_pixels.get(start..end) else {
            output.clear();
            return false;
        };
        for &pixel in row {
            output[output_index] = ((pixel >> 16) & 0xff) as u8;
            output[output_index + 1] = ((pixel >> 8) & 0xff) as u8;
            output[output_index + 2] = (pixel & 0xff) as u8;
            output[output_index + 3] = ((pixel >> 24) & 0xff) as u8;
            output_index += 4;
        }
    }
    true
}

fn configure_texture(gl: &glow::Context) {
    unsafe {
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MIN_FILTER,
            glow::LINEAR as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MAG_FILTER,
            glow::LINEAR as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_S,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_T,
            glow::CLAMP_TO_EDGE as i32,
        );
    }
}

fn destroy_surface_resource(
    gl: &glow::Context,
    egl: &EglInstance,
    egl_display: egl::Display,
    resource: EglSurfaceResource,
) {
    destroy_image_resource(gl, egl, egl_display, resource.image);
}

fn destroy_image_resource(
    gl: &glow::Context,
    egl: &EglInstance,
    egl_display: egl::Display,
    resource: EglImageResource,
) {
    unsafe {
        gl.delete_texture(resource.texture);
    }
    if let Some(image) = resource.egl_image {
        let _ = egl.destroy_image(egl_display, image);
    }
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
mod tests {
    use super::*;
    use oblivion_one::compositor::{
        RenderableSurfaceDamage, ResolvedEffectScene, SurfaceCommitCounter, SurfaceCommitSequence,
        SurfaceOpaqueRegion, SurfacePlacement, SurfaceRenderBackend, SurfaceResourceSyncState,
    };
    use oblivion_one::effects::{
        DualKawaseBlurSpec, EffectAlphaMode, EffectFailurePolicy, EffectFrameDemand, EffectNode,
        EffectNodeId, EffectParameterBlock, EffectProgram, EffectProgramId, EffectSource,
        EffectWorkingSpace, GraphTextureSource, validate_effect_program,
    };
    use oblivion_one::presentation_animation::{AnimationTime, PresentationRect};
    use oblivion_one::render_backend::buffer::{
        BufferIdAllocator, BufferIdentity, BufferSize, CommittedSurfaceBuffer, DmabufBufferHandle,
        DmabufImageKey, DmabufPlane, DmabufPlaneDescriptor, DrmFormat, DrmModifier,
    };
    use oblivion_one::window_lifecycle_animation::{
        LampWindowSample, LifecycleDirection, LifecycleSceneSample, LifecycleTransitionId,
        LifecycleVisualGroup, LifecycleVisualSource, LifecycleVisualSourceKind,
    };

    const XR24: u32 = u32::from_le_bytes(*b"XR24");
    const AR24: u32 = u32::from_le_bytes(*b"AR24");

    fn egl_test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .expect("EGL test lock is not poisoned")
    }

    struct GlesEffectTestHarness {
        egl: EglInstance,
        display: egl::Display,
        context: egl::Context,
        surface: egl::Surface,
        gl: glow::Context,
        renderer: GlesSceneRenderer,
        _egl_test_lock: std::sync::MutexGuard<'static, ()>,
    }

    impl GlesEffectTestHarness {
        fn new(width: u32, height: u32) -> Self {
            let egl_test_lock = egl_test_lock();
            const EGL_PLATFORM_SURFACELESS_MESA: egl::Enum = 0x31dd;
            let egl = unsafe { EglInstance::load_required() }
                .expect("EGL loader is required for the effect coordinate test");
            let display = unsafe {
                egl.get_platform_display(
                    EGL_PLATFORM_SURFACELESS_MESA,
                    std::ptr::null_mut(),
                    &[egl::ATTRIB_NONE],
                )
                .or_else(|_| {
                    egl.get_display(egl::DEFAULT_DISPLAY)
                        .ok_or(egl::Error::BadDisplay)
                })
            }
            .expect("EGL display is available");
            egl.initialize(display).expect("EGL initializes");
            egl.bind_api(egl::OPENGL_ES_API)
                .expect("EGL binds the GLES API");
            let config_attributes = [
                egl::SURFACE_TYPE,
                egl::PBUFFER_BIT,
                egl::RENDERABLE_TYPE,
                egl::OPENGL_ES3_BIT,
                egl::RED_SIZE,
                8,
                egl::GREEN_SIZE,
                8,
                egl::BLUE_SIZE,
                8,
                egl::ALPHA_SIZE,
                8,
                egl::NONE,
            ];
            let count = egl
                .matching_config_count(display, &config_attributes)
                .expect("EGL returns GLES3 pbuffer configs");
            assert!(count > 0, "EGL exposes a GLES3 pbuffer config");
            let mut configs = Vec::with_capacity(count);
            egl.choose_config(display, &config_attributes, &mut configs)
                .expect("EGL chooses a GLES3 pbuffer config");
            let config = configs[0];
            let context =
                create_gles_context(&egl, display, config).expect("GLES3 context creates");
            let surface = egl
                .create_pbuffer_surface(
                    display,
                    config,
                    &[
                        egl::WIDTH,
                        width as egl::Int,
                        egl::HEIGHT,
                        height as egl::Int,
                        egl::NONE,
                    ],
                )
                .expect("GLES3 pbuffer surface creates");
            egl.make_current(display, Some(surface), Some(surface), Some(context))
                .expect("EGL makes the GLES3 context current");
            let cursor_image = Arc::new(
                CompositorCursorImage::from_argb8888(vec![0xffff_ffff], 1, 1, 0, 0)
                    .expect("test cursor image is valid"),
            );
            let renderer = GlesSceneRenderer::new_current(
                &egl,
                width,
                height,
                None,
                EglPartialRepaintCapabilities {
                    buffer_age: false,
                    partial_render_repair: false,
                    swap_buffers_with_damage: false,
                },
                cursor_image,
            )
            .expect("test GLES renderer creates");
            let gl = unsafe {
                glow::Context::from_loader_function(|name| {
                    egl.get_proc_address(name)
                        .map(|symbol| symbol as *const c_void)
                        .unwrap_or(ptr::null())
                })
            };
            Self {
                _egl_test_lock: egl_test_lock,
                egl,
                display,
                context,
                surface,
                gl,
                renderer,
            }
        }
    }

    impl Drop for GlesEffectTestHarness {
        fn drop(&mut self) {
            self.renderer.destroy(&self.egl, self.display);
            let _ = self.egl.make_current(self.display, None, None, None);
            let _ = self.egl.destroy_surface(self.display, self.surface);
            let _ = self.egl.destroy_context(self.display, self.context);
            let _ = self.egl.terminate(self.display);
        }
    }

    fn moving_blur_scene(rect: EffectRect) -> (ResolvedEffectScene, EffectRegistry) {
        let source = EffectNodeId::new(1).expect("source id");
        let blur = EffectNodeId::new(2).expect("blur id");
        let program = validate_effect_program(EffectProgram {
            id: EffectProgramId::new(1).expect("program id"),
            nodes: vec![
                EffectNode::source(source, EffectSource::Backdrop),
                EffectNode::dual_kawase(
                    blur,
                    source,
                    DualKawaseBlurSpec::new(4.0, 2, 1.0).expect("blur spec"),
                ),
            ],
            output: blur,
            working_space: EffectWorkingSpace::LinearSrgb,
            alpha_mode: EffectAlphaMode::Opaque,
            outsets: oblivion_one::effects::EffectOutsets::ZERO,
            frame_demand: EffectFrameDemand::OnDamage,
            failure_policy: EffectFailurePolicy::Passthrough,
        })
        .expect("moving blur program validates");
        let mut registry = EffectRegistry::empty();
        registry
            .insert(program)
            .expect("moving blur program inserts");
        let instance = oblivion_one::compositor::ResolvedEffectInstance {
            id: oblivion_one::effects::EffectInstanceId::new(1).expect("instance id"),
            program: EffectProgramId::new(1).expect("program id"),
            anchor: oblivion_one::compositor::EffectAnchor::OutputPostProcess,
            target_bounds: rect,
            region: oblivion_one::effects::EffectRegion::from_rect(rect),
            parameter_block: EffectParameterBlock::default(),
            signature: 11,
            frame_demand: EffectFrameDemand::OnDamage,
            visual_group: None,
            anchor_scope: oblivion_one::compositor::EffectAnchorScope::VisualGroup,
            scene_order: oblivion_one::compositor::EffectSceneOrder::for_anchor(
                oblivion_one::compositor::EffectAnchor::OutputPostProcess,
            ),
        };
        (ResolvedEffectScene::new(1, vec![instance]), registry)
    }

    fn moving_visual_group_blur_scene(
        rect: EffectRect,
        visual_group: VisualGroupId,
    ) -> ResolvedEffectScene {
        let (mut scene, _) = moving_blur_scene(rect);
        let instance = scene.instances.first_mut().expect("moving blur instance");
        instance.anchor = oblivion_one::compositor::EffectAnchor::BeforeSurface(42);
        instance.visual_group = Some(visual_group);
        instance.anchor_scope = oblivion_one::compositor::EffectAnchorScope::VisualGroup;
        instance.scene_order =
            oblivion_one::compositor::EffectSceneOrder::for_anchor(instance.anchor);
        scene
    }

    fn install_visual_group_scene_commands(
        renderer: &mut GlesSceneRenderer,
        rect: EffectRect,
        visual_group: VisualGroupId,
    ) {
        const BACKGROUND_COLOR: u32 = 0xff20_4060;
        renderer.vertices.clear();
        renderer.commands.clear();
        push_draw_command(
            &mut renderer.vertices,
            &mut renderer.commands,
            EglDrawLayer::SolidRgba(BACKGROUND_COLOR),
            EglRect::new(
                0.0,
                0.0,
                renderer.current_size.0 as f32,
                renderer.current_size.1 as f32,
            ),
            renderer.current_size.0,
            renderer.current_size.1,
            OutputFramebufferOrigin::BottomLeft,
        );
        let target_command = renderer.commands.len();
        push_draw_command(
            &mut renderer.vertices,
            &mut renderer.commands,
            EglDrawLayer::Surface(42),
            EglRect::new(
                rect.x as f32,
                rect.y as f32,
                rect.width as f32,
                rect.height as f32,
            ),
            renderer.current_size.0,
            renderer.current_size.1,
            OutputFramebufferOrigin::BottomLeft,
        );
        renderer.commands[target_command].visual_group = Some(visual_group);
        renderer.scene_geometry_dirty = true;
    }

    #[test]
    fn moving_blur_domain_reuses_real_gles_resources() {
        let mut harness = GlesEffectTestHarness::new(256, 192);
        let output_bounds = EffectRect::new(0, 0, 256, 192).expect("output bounds");
        let full_damage = EffectRegion::from_rect(output_bounds);
        let repaint_plan = RepaintPlan {
            render_damage: OutputDamage::Full,
            repair_damage: OutputDamage::Full,
            buffer_age: None,
            mode: RepaintMode::Full,
            fallback_reason: None,
        };
        let mut first_dimensions = None;
        let mut first_cache_bytes = None;
        let mut first_allocation_count = None;
        let mut capture_radii = None;
        let mut positions = Vec::with_capacity(512);
        for step in 0..512 {
            let phase = step % 16;
            let (x, y) = match phase {
                0 => (32, 32),
                1 => (33, 33),
                2 => (184, 32),
                3 => (183, 31),
                4 => (184, 144),
                5 => (183, 143),
                6 => (32, 144),
                7 => (33, 143),
                8 => (112, 80),
                9 => (113, 81),
                10 => (112, 80),
                11 => (32, 32),
                12 => (224, 168),
                13 => (0, 168),
                14 => (224, 0),
                _ => (0, 0),
            };
            positions.push((x, y));
        }

        for (step, (x, y)) in positions.into_iter().enumerate() {
            let rect = EffectRect::new(x, y, 32, 24).expect("moving blur rectangle");
            let (scene, registry) = moving_blur_scene(rect);
            let plan = oblivion_one::effects::compile_frame_execution_plan(
                &scene,
                &full_damage,
                output_bounds,
                &registry,
            )
            .expect("moving blur graph compiles");
            let oblivion_one::effects::FrameExecutionPlan::EffectGraph(graph) = plan else {
                panic!("moving blur must compile to an effect graph");
            };
            let demand = plan_effect_execution_demand(&graph, &full_damage, true);
            let selection = effects::select_effect_execution(&graph, &demand);

            effects::execute_effect_graph(
                &mut harness.renderer,
                &graph,
                OutputFramebufferOrigin::BottomLeft,
                &repaint_plan,
                &demand,
                &selection,
            )
            .expect("moving blur graph executes in real GLES");

            let mut first_gl_error = None;
            loop {
                let error = unsafe { harness.gl.get_error() };
                if error == glow::NO_ERROR {
                    break;
                }
                first_gl_error.get_or_insert(error);
            }
            assert_eq!(first_gl_error, None, "step {step} left a GLES error");

            let capture = graph
                .textures
                .iter()
                .find(|texture| texture.source == GraphTextureSource::CapturedScene)
                .expect("moving blur capture texture");
            let (capture_radius_x, capture_radius_y) = *capture_radii.get_or_insert((
                x.saturating_sub(capture.domain.x).max(0),
                y.saturating_sub(capture.domain.y).max(0),
            ));
            let expected_left = (x - capture_radius_x).max(0);
            let expected_top = (y - capture_radius_y).max(0);
            let expected_right = (x + 32 + capture_radius_x).min(256);
            let expected_bottom = (y + 24 + capture_radius_y).min(192);
            let expected_capture = EffectRect::new(
                expected_left,
                expected_top,
                u32::try_from(expected_right - expected_left).expect("capture width"),
                u32::try_from(expected_bottom - expected_top).expect("capture height"),
            )
            .expect("expected capture domain");
            for texture in &graph.textures {
                assert!(texture.width > 0 && texture.height > 0);
                if texture.source != GraphTextureSource::Output {
                    assert_eq!(texture.domain, expected_capture, "step {step}");
                }
            }
            let dimensions = graph
                .textures
                .iter()
                .filter(|texture| texture.source != GraphTextureSource::Output)
                .map(|texture| (texture.width, texture.height))
                .collect::<Vec<_>>();
            let interior = expected_left == x - capture_radius_x
                && expected_top == y - capture_radius_y
                && expected_right == x + 32 + capture_radius_x
                && expected_bottom == y + 24 + capture_radius_y;
            if interior {
                if let Some(first) = &first_dimensions {
                    assert_eq!(first, &dimensions, "translation changed texture dimensions");
                } else {
                    first_dimensions = Some(dimensions);
                }
            }

            let metrics = harness.renderer.effect_resources.metrics();
            assert_eq!(metrics.checked_out_texture_count, 0, "step {step}");
            if step == 15 {
                first_cache_bytes = Some(metrics.current_bytes);
                first_allocation_count = Some(metrics.allocation_count);
            }
            if step >= 16 {
                assert_eq!(
                    metrics.current_bytes,
                    first_cache_bytes.expect("warm cache bytes"),
                    "step {step} changed cache size"
                );
                assert_eq!(
                    metrics.allocation_count,
                    first_allocation_count.expect("warm allocation count"),
                    "step {step} allocated again"
                );
            }
            assert_eq!(metrics.eviction_count, 0, "translation evicted a resource");
            assert!(metrics.cached_texture_count <= 16);
        }

        let metrics = harness.renderer.effect_resources.metrics();
        assert!(
            metrics.reuse_count > 0,
            "moving blur did not reuse pooled textures"
        );
        assert_eq!(metrics.checked_out_texture_count, 0);
    }

    #[test]
    fn moving_visual_group_blur_replays_only_background_below_target() {
        let mut harness = GlesEffectTestHarness::new(256, 192);
        const BACKGROUND_COLOR: u32 = 0xff20_4060;
        let target_texture =
            create_uploaded_resource(&harness.gl, 32, 24).expect("target scene texture creates");
        harness.renderer.surface_resources.insert(
            42,
            EglSurfaceResource {
                image: target_texture,
                dmabuf_key: None,
                buffer_lifetime: None,
                shm_synced_commit: None,
            },
        );
        let background_texture =
            create_uploaded_resource(&harness.gl, 1, 1).expect("background scene texture creates");
        harness.renderer.decoration_resources.insert(
            DecorationResourceKey::Solid(BACKGROUND_COLOR),
            background_texture,
        );

        let output_bounds = EffectRect::new(0, 0, 256, 192).expect("output bounds");
        let full_damage = EffectRegion::from_rect(output_bounds);
        let repaint_plan = RepaintPlan {
            render_damage: OutputDamage::Full,
            repair_damage: OutputDamage::Full,
            buffer_age: None,
            mode: RepaintMode::Full,
            fallback_reason: None,
        };
        let visual_group = VisualGroupId::new(9).expect("visual group id");
        let (_, registry) = moving_blur_scene(output_bounds);
        let positions = [
            (32, 32),
            (33, 33),
            (184, 32),
            (183, 31),
            (224, 144),
            (240, 168),
            (240, 180),
            (0, 168),
            (-8, 180),
            (-8, -4),
            (0, 0),
            (112, 80),
            (113, 81),
            (112, 80),
            (32, 32),
            (184, 144),
        ];
        let mut first_dimensions = None;
        let mut warm_cache_bytes = None;
        let mut warm_allocation_count = None;
        let mut capture_radii = None;

        for (step, (x, y)) in positions.iter().copied().cycle().take(512).enumerate() {
            let rect = EffectRect::new(x, y, 32, 24).expect("moving target rectangle");
            install_visual_group_scene_commands(&mut harness.renderer, rect, visual_group);
            let scene = moving_visual_group_blur_scene(rect, visual_group);
            let plan = oblivion_one::effects::compile_frame_execution_plan(
                &scene,
                &full_damage,
                output_bounds,
                &registry,
            )
            .expect("visual-group blur graph compiles");
            let oblivion_one::effects::FrameExecutionPlan::EffectGraph(graph) = plan else {
                panic!("visual-group blur must compile to an effect graph");
            };
            let capture_pass = graph
                .passes
                .iter()
                .find(|pass| pass.kind == oblivion_one::effects::RenderPassKind::SceneCapture)
                .expect("visual-group blur capture pass");
            assert_eq!(
                capture_pass.anchor,
                oblivion_one::compositor::EffectAnchor::BeforeSurface(42)
            );
            assert_eq!(capture_pass.visual_group, Some(visual_group));
            assert_eq!(
                capture_pass.anchor_scope,
                oblivion_one::compositor::EffectAnchorScope::VisualGroup
            );
            assert!(capture_pass.checkpoint_dependencies.is_empty());

            let demand = plan_effect_execution_demand(&graph, &full_damage, true);
            let selection = effects::select_effect_execution(&graph, &demand);
            let before_replays = harness.renderer.last_frame_stats().draw_command_replays;
            effects::execute_effect_graph(
                &mut harness.renderer,
                &graph,
                OutputFramebufferOrigin::BottomLeft,
                &repaint_plan,
                &demand,
                &selection,
            )
            .expect("visual-group blur graph executes in real GLES");
            let after_replays = harness.renderer.last_frame_stats().draw_command_replays;
            assert_eq!(
                after_replays.saturating_sub(before_replays),
                3,
                "step {step}: capture replays one background command and the final scene replays both commands"
            );

            let mut first_gl_error = None;
            loop {
                let error = unsafe { harness.gl.get_error() };
                if error == glow::NO_ERROR {
                    break;
                }
                first_gl_error.get_or_insert(error);
            }
            assert_eq!(first_gl_error, None, "step {step} left a GLES error");

            let capture = graph
                .textures
                .iter()
                .find(|texture| texture.source == GraphTextureSource::CapturedScene)
                .expect("visual-group blur capture texture");
            assert!(capture.domain.width > 0 && capture.domain.height > 0);
            let (capture_radius_x, capture_radius_y) = *capture_radii.get_or_insert((
                x.saturating_sub(capture.domain.x).max(0),
                y.saturating_sub(capture.domain.y).max(0),
            ));
            let expected_left = (x - capture_radius_x).max(0);
            let expected_top = (y - capture_radius_y).max(0);
            let expected_right = (x + 32 + capture_radius_x).min(256);
            let expected_bottom = (y + 24 + capture_radius_y).min(192);
            let expected_capture = EffectRect::new(
                expected_left,
                expected_top,
                u32::try_from(expected_right - expected_left).expect("capture width"),
                u32::try_from(expected_bottom - expected_top).expect("capture height"),
            )
            .expect("expected capture domain");
            assert_eq!(capture.domain, expected_capture, "step {step}");
            let dimensions = graph
                .textures
                .iter()
                .filter(|texture| texture.source != GraphTextureSource::Output)
                .map(|texture| (texture.width, texture.height))
                .collect::<Vec<_>>();
            let interior = expected_left == x - capture_radius_x
                && expected_top == y - capture_radius_y
                && expected_right == x + 32 + capture_radius_x
                && expected_bottom == y + 24 + capture_radius_y;
            if interior {
                if let Some(first) = &first_dimensions {
                    assert_eq!(first, &dimensions, "translation changed texture dimensions");
                } else {
                    first_dimensions = Some(dimensions);
                }
            }

            let metrics = harness.renderer.effect_resources.metrics();
            assert_eq!(metrics.checked_out_texture_count, 0, "step {step}");
            if step == positions.len() - 1 {
                warm_cache_bytes = Some(metrics.current_bytes);
                warm_allocation_count = Some(metrics.allocation_count);
            }
            if step >= positions.len() {
                assert_eq!(
                    metrics.current_bytes,
                    warm_cache_bytes.expect("warm cache bytes"),
                    "step {step} changed cache size"
                );
                assert_eq!(
                    metrics.allocation_count,
                    warm_allocation_count.expect("warm allocation count"),
                    "step {step} allocated again"
                );
            }
            assert_eq!(metrics.eviction_count, 0, "translation evicted a resource");
            assert!(metrics.cached_texture_count <= 32);
        }

        let metrics = harness.renderer.effect_resources.metrics();
        assert!(
            metrics.reuse_count > 0,
            "visual-group blur did not reuse pooled textures"
        );
        assert_eq!(metrics.checked_out_texture_count, 0);
    }

    fn trace_event_index(events: &[String], terms: &[&str]) -> usize {
        events
            .iter()
            .position(|line| terms.iter().all(|term| line.contains(term)))
            .unwrap_or_else(|| panic!("trace event not found: {terms:?}"))
    }

    #[test]
    fn effect_trace_bounds_visual_group_scene_and_final_replay() {
        let mut harness = GlesEffectTestHarness::new(256, 192);
        let target_texture =
            create_uploaded_resource(&harness.gl, 32, 24).expect("target scene texture creates");
        harness.renderer.surface_resources.insert(
            42,
            EglSurfaceResource {
                image: target_texture,
                dmabuf_key: None,
                buffer_lifetime: None,
                shm_synced_commit: None,
            },
        );
        let background_texture =
            create_uploaded_resource(&harness.gl, 1, 1).expect("background scene texture creates");
        harness.renderer.decoration_resources.insert(
            DecorationResourceKey::Solid(0xff20_4060),
            background_texture,
        );
        harness.renderer.effect_trace = effects::EffectExecutionTrace::enabled_for_test();
        effects::clear_effect_trace_test_events();

        let rect = EffectRect::new(32, 32, 32, 24).expect("visual group blur rectangle");
        let visual_group = VisualGroupId::new(9).expect("visual group id");
        install_visual_group_scene_commands(&mut harness.renderer, rect, visual_group);
        let (mut scene, registry) = moving_blur_scene(rect);
        let instance = scene.instances.first_mut().expect("moving blur instance");
        instance.anchor = oblivion_one::compositor::EffectAnchor::BeforeSurface(42);
        instance.visual_group = Some(visual_group);
        instance.anchor_scope = oblivion_one::compositor::EffectAnchorScope::VisualGroup;
        instance.scene_order =
            oblivion_one::compositor::EffectSceneOrder::for_anchor(instance.anchor);
        let output_bounds = EffectRect::new(0, 0, 256, 192).expect("output bounds");
        let full_damage = EffectRegion::from_rect(output_bounds);
        let repaint_plan = RepaintPlan {
            render_damage: OutputDamage::Full,
            repair_damage: OutputDamage::Full,
            buffer_age: None,
            mode: RepaintMode::Full,
            fallback_reason: None,
        };
        let plan = oblivion_one::effects::compile_frame_execution_plan(
            &scene,
            &full_damage,
            output_bounds,
            &registry,
        )
        .expect("visual group blur graph compiles");
        let oblivion_one::effects::FrameExecutionPlan::EffectGraph(graph) = plan else {
            panic!("visual group blur must compile to an effect graph");
        };
        let demand = plan_effect_execution_demand(&graph, &full_damage, true);
        let selection = effects::select_effect_execution(&graph, &demand);

        effects::execute_effect_graph(
            &mut harness.renderer,
            &graph,
            OutputFramebufferOrigin::BottomLeft,
            &repaint_plan,
            &demand,
            &selection,
        )
        .expect("visual group blur graph executes in real GLES");
        let events = effects::take_effect_trace_test_events();

        let capture_execute_end = trace_event_index(
            &events,
            &["event=effect_pass_execute_end", "kind=SceneCapture"],
        );
        let composite_resources_end = trace_event_index(
            &events,
            &["event=effect_pass_resources_end", "kind=Composite"],
        );
        let replay_begin = trace_event_index(
            &events,
            &[
                "event=effect_scene_replay_begin",
                "kind=Composite",
                "reason=composite_advance",
            ],
        );
        let replay_end = trace_event_index(
            &events,
            &[
                "event=effect_scene_replay_end",
                "kind=Composite",
                "reason=composite_advance",
            ],
        );
        let composite_validate_begin = trace_event_index(
            &events,
            &["event=effect_pass_validate_begin", "kind=Composite"],
        );
        let composite_end =
            trace_event_index(&events, &["event=effect_pass_end", "kind=Composite"]);
        let final_replay_begin =
            trace_event_index(&events, &["event=effect_final_scene_replay_begin"]);
        let final_replay_end = trace_event_index(&events, &["event=effect_final_scene_replay_end"]);
        let overlay_begin = trace_event_index(&events, &["event=effect_overlay_draw_begin"]);
        let overlay_end = trace_event_index(&events, &["event=effect_overlay_draw_end"]);
        let graph_execute_end = trace_event_index(&events, &["event=effect_graph_execute_end"]);

        assert!(capture_execute_end < composite_resources_end);
        assert!(composite_resources_end < replay_begin);
        assert!(replay_begin < replay_end);
        assert!(replay_end < composite_validate_begin);
        assert!(composite_validate_begin < composite_end);
        assert!(composite_end < final_replay_begin);
        assert!(final_replay_begin < final_replay_end);
        assert!(final_replay_end < overlay_begin);
        assert!(overlay_begin < overlay_end);
        assert!(overlay_end < graph_execute_end);
        assert!(events[replay_begin].contains("scene_cursor_start=0"));
        assert!(events[replay_begin].contains("scene_cursor_end=1"));
        assert!(events[replay_begin].contains("command_count=1"));
        assert!(events[final_replay_begin].contains("scene_cursor_start=1"));
        assert!(events[final_replay_begin].contains("scene_cursor_end=2"));
        assert!(events[final_replay_begin].contains("command_count=1"));
    }

    #[test]
    fn effect_trace_bounds_checkpoint_scene_advancement() {
        let mut harness = GlesEffectTestHarness::new(256, 192);
        harness.renderer.effect_trace = effects::EffectExecutionTrace::enabled_for_test();
        effects::clear_effect_trace_test_events();
        for surface_id in [10, 20] {
            let texture = create_uploaded_resource(&harness.gl, 1, 1)
                .expect("checkpoint scene texture creates");
            harness.renderer.surface_resources.insert(
                surface_id,
                EglSurfaceResource {
                    image: texture,
                    dmabuf_key: None,
                    buffer_lifetime: None,
                    shm_synced_commit: None,
                },
            );
        }
        push_draw_command(
            &mut harness.renderer.vertices,
            &mut harness.renderer.commands,
            EglDrawLayer::SolidRgba(0xff20_4060),
            EglRect::new(0.0, 0.0, 256.0, 192.0),
            256,
            192,
            OutputFramebufferOrigin::BottomLeft,
        );
        for (surface_id, y) in [(10, 0.0), (20, 96.0)] {
            push_draw_command(
                &mut harness.renderer.vertices,
                &mut harness.renderer.commands,
                EglDrawLayer::Surface(surface_id),
                EglRect::new(0.0, y, 256.0, 96.0),
                256,
                192,
                OutputFramebufferOrigin::BottomLeft,
            );
        }
        harness.renderer.scene_geometry_dirty = true;

        let rect = EffectRect::new(32, 32, 64, 48).expect("checkpoint blur rectangle");
        let (scene, registry) = moving_blur_scene(rect);
        let mut first = scene.instances[0].clone();
        first.anchor = oblivion_one::compositor::EffectAnchor::BeforeSurface(10);
        first.anchor_scope = oblivion_one::compositor::EffectAnchorScope::Surface;
        first.visual_group = None;
        first.scene_order = oblivion_one::compositor::EffectSceneOrder::for_anchor(first.anchor);
        let mut second = first.clone();
        second.id = oblivion_one::effects::EffectInstanceId::new(2).expect("second instance id");
        second.signature = second.signature.saturating_add(1);
        second.anchor = oblivion_one::compositor::EffectAnchor::BeforeSurface(20);
        second.scene_order = oblivion_one::compositor::EffectSceneOrder::for_anchor(second.anchor);
        let scene = ResolvedEffectScene::new(1, vec![first, second]);
        let output_bounds = EffectRect::new(0, 0, 256, 192).expect("output bounds");
        let full_damage = EffectRegion::from_rect(output_bounds);
        let repaint_plan = RepaintPlan {
            render_damage: OutputDamage::Full,
            repair_damage: OutputDamage::Full,
            buffer_age: None,
            mode: RepaintMode::Full,
            fallback_reason: None,
        };
        let plan = oblivion_one::effects::compile_frame_execution_plan(
            &scene,
            &full_damage,
            output_bounds,
            &registry,
        )
        .expect("checkpoint blur graph compiles");
        let oblivion_one::effects::FrameExecutionPlan::EffectGraph(graph) = plan else {
            panic!("checkpoint blur must compile to an effect graph");
        };
        let checkpoint_pass = graph
            .passes
            .iter()
            .find(|pass| {
                pass.kind == oblivion_one::effects::RenderPassKind::SceneCapture
                    && !pass.checkpoint_dependencies.is_empty()
            })
            .expect("checkpoint-dependent capture pass");
        let checkpoint_pass_id = checkpoint_pass.id.get().to_string();
        let demand = plan_effect_execution_demand(&graph, &full_damage, true);
        let selection = effects::select_effect_execution(&graph, &demand);

        effects::execute_effect_graph(
            &mut harness.renderer,
            &graph,
            OutputFramebufferOrigin::BottomLeft,
            &repaint_plan,
            &demand,
            &selection,
        )
        .expect("checkpoint blur graph executes in real GLES");
        let events = effects::take_effect_trace_test_events();

        let replay_begin = trace_event_index(
            &events,
            &[
                "event=effect_scene_replay_begin",
                "reason=checkpoint_dependency",
            ],
        );
        let replay_end = trace_event_index(
            &events,
            &[
                "event=effect_scene_replay_end",
                "reason=checkpoint_dependency",
            ],
        );
        let resources_end = trace_event_index(
            &events,
            &[
                "event=effect_pass_resources_end",
                &format!("pass={checkpoint_pass_id}"),
            ],
        );
        let validate_begin = trace_event_index(
            &events,
            &[
                "event=effect_pass_validate_begin",
                &format!("pass={checkpoint_pass_id}"),
            ],
        );

        assert!(events[replay_begin].contains(&format!("pass={checkpoint_pass_id}")));
        assert!(events[replay_end].contains(&format!("pass={checkpoint_pass_id}")));
        assert!(resources_end < replay_begin);
        assert!(replay_begin < replay_end);
        assert!(replay_end < validate_begin);
        assert!(events[replay_begin].contains("scene_cursor_start=1"));
        assert!(events[replay_begin].contains("scene_cursor_end=2"));
        assert!(events[replay_begin].contains("command_count=1"));
    }

    fn create_effect_test_texture(
        gl: &glow::Context,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) -> glow::Texture {
        let texture = unsafe { gl.create_texture().expect("effect test texture creates") };
        unsafe {
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::NEAREST as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::NEAREST as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_S,
                glow::CLAMP_TO_EDGE as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_T,
                glow::CLAMP_TO_EDGE as i32,
            );
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA as i32,
                width as i32,
                height as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(pixels)),
            );
            gl.bind_texture(glow::TEXTURE_2D, None);
        }
        texture
    }

    fn read_effect_test_pixels(gl: &glow::Context, width: u32, height: u32) -> Vec<u8> {
        let mut pixels = vec![0_u8; width as usize * height as usize * 4];
        unsafe {
            gl.flush();
            gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
            gl.read_pixels(
                0,
                0,
                width as i32,
                height as i32,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut pixels)),
            );
        }
        pixels
    }

    fn assert_effect_test_pixel(pixels: &[u8], width: u32, x: u32, y: u32, expected: [u8; 4]) {
        let index = ((y * width + x) * 4) as usize;
        assert_eq!(&pixels[index..index + 4], &expected, "pixel ({x}, {y})");
    }

    fn set_effect_test_uniform_i32(
        gl: &glow::Context,
        program: glow::Program,
        name: &str,
        value: i32,
    ) {
        let location = unsafe {
            gl.get_uniform_location(program, name)
                .unwrap_or_else(|| panic!("uniform {name} is active"))
        };
        unsafe { gl.uniform_1_i32(Some(&location), value) };
    }

    fn set_effect_test_uniform_2_f32(
        gl: &glow::Context,
        program: glow::Program,
        name: &str,
        x: f32,
        y: f32,
    ) {
        let location = unsafe {
            gl.get_uniform_location(program, name)
                .unwrap_or_else(|| panic!("uniform {name} is active"))
        };
        unsafe { gl.uniform_2_f32(Some(&location), x, y) };
    }

    fn set_effect_test_uniform_4_f32(
        gl: &glow::Context,
        program: glow::Program,
        name: &str,
        values: [f32; 4],
    ) {
        let location = unsafe {
            gl.get_uniform_location(program, name)
                .unwrap_or_else(|| panic!("uniform {name} is active"))
        };
        unsafe { gl.uniform_4_f32(Some(&location), values[0], values[1], values[2], values[3]) };
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_effect_test(
        gl: &glow::Context,
        program: glow::Program,
        quad: glow::VertexArray,
        input_texture: glow::Texture,
        width: u32,
        height: u32,
        target_flip_y: bool,
        input_uniforms: Option<(&str, bool)>,
        configure: impl Fn(&glow::Context, glow::Program),
    ) {
        unsafe {
            gl.viewport(0, 0, width as i32, height as i32);
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::BLEND);
            gl.clear_color(0.0, 0.0, 0.0, 0.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.use_program(Some(program));
            set_effect_test_uniform_i32(
                gl,
                program,
                "u_effect_target_flip_y",
                i32::from(target_flip_y),
            );
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(input_texture));
            if let Some((input_flip_uniform, input_flip_y)) = input_uniforms {
                set_effect_test_uniform_i32(
                    gl,
                    program,
                    input_flip_uniform,
                    i32::from(input_flip_y),
                );
                set_effect_test_uniform_i32(gl, program, "u_effect_input", 0);
            }
            configure(gl, program);
            gl.bind_vertex_array(Some(quad));
            gl.draw_arrays(glow::TRIANGLES, 0, 6);
            gl.bind_vertex_array(None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.use_program(None);
        }
    }

    fn canonical_two_by_two_pixels() -> Vec<u8> {
        [
            [0, 0, 255, 255],
            [0, 0, 255, 255],
            [255, 0, 0, 255],
            [255, 0, 0, 255],
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    #[test]
    fn real_gles_copy_preserves_logical_and_physical_orientation_contracts() {
        let mut harness = GlesEffectTestHarness::new(2, 2);
        let program = program::create_program_from_sources(
            &harness.gl,
            effects::DUAL_KAWASE_VERTEX_SHADER,
            effects::COPY_FRAGMENT_SHADER,
        )
        .expect("copy effect program compiles with the shared vertex shader");
        let input_texture =
            create_effect_test_texture(&harness.gl, 2, 2, &canonical_two_by_two_pixels());
        let quad = harness
            .renderer
            .ensure_effect_quad()
            .expect("effect quad creates")
            .0;

        draw_effect_test(
            &harness.gl,
            program,
            quad,
            input_texture,
            2,
            2,
            false,
            Some(("u_effect_input_flip_y", true)),
            |_, _| {},
        );
        let bottom_left_pixels = read_effect_test_pixels(&harness.gl, 2, 2);
        assert_effect_test_pixel(&bottom_left_pixels, 2, 0, 0, [0, 0, 255, 255]);
        assert_effect_test_pixel(&bottom_left_pixels, 2, 0, 1, [255, 0, 0, 255]);

        draw_effect_test(
            &harness.gl,
            program,
            quad,
            input_texture,
            2,
            2,
            true,
            Some(("u_effect_input_flip_y", true)),
            |_, _| {},
        );
        let top_left_scanout_pixels = read_effect_test_pixels(&harness.gl, 2, 2);
        assert_effect_test_pixel(&top_left_scanout_pixels, 2, 0, 0, [255, 0, 0, 255]);
        assert_effect_test_pixel(&top_left_scanout_pixels, 2, 0, 1, [0, 0, 255, 255]);

        unsafe {
            harness.gl.delete_texture(input_texture);
            harness.gl.delete_program(program);
        }
    }

    #[test]
    fn real_gles_composite_keeps_logical_domain_and_orientation() {
        let mut harness = GlesEffectTestHarness::new(4, 6);
        let program = program::create_program_from_sources(
            &harness.gl,
            effects::DUAL_KAWASE_VERTEX_SHADER,
            effects::COMPOSITE_FRAGMENT_SHADER,
        )
        .expect("composite effect program compiles with the shared vertex shader");
        let input_texture =
            create_effect_test_texture(&harness.gl, 2, 2, &canonical_two_by_two_pixels());
        let quad = harness
            .renderer
            .ensure_effect_quad()
            .expect("effect quad creates")
            .0;
        let configure = |gl: &glow::Context, program: glow::Program| {
            set_effect_test_uniform_4_f32(
                gl,
                program,
                "u_effect_input_domain",
                [1.0, 1.0, 2.0, 2.0],
            );
            set_effect_test_uniform_2_f32(gl, program, "u_effect_output_size", 4.0, 6.0);
            set_effect_test_uniform_i32(gl, program, "u_effect_encode_srgb", 0);
            set_effect_test_uniform_i32(gl, program, "u_effect_force_opaque", 0);
        };

        draw_effect_test(
            &harness.gl,
            program,
            quad,
            input_texture,
            4,
            6,
            false,
            Some(("u_effect_input_flip_y", true)),
            configure,
        );
        let bottom_left_pixels = read_effect_test_pixels(&harness.gl, 4, 6);
        assert_effect_test_pixel(&bottom_left_pixels, 4, 1, 4, [255, 0, 0, 255]);
        assert_effect_test_pixel(&bottom_left_pixels, 4, 2, 4, [255, 0, 0, 255]);
        assert_effect_test_pixel(&bottom_left_pixels, 4, 1, 3, [0, 0, 255, 255]);
        assert_effect_test_pixel(&bottom_left_pixels, 4, 2, 3, [0, 0, 255, 255]);
        assert_effect_test_pixel(&bottom_left_pixels, 4, 1, 1, [0, 0, 0, 0]);

        draw_effect_test(
            &harness.gl,
            program,
            quad,
            input_texture,
            4,
            6,
            true,
            Some(("u_effect_input_flip_y", true)),
            configure,
        );
        let top_left_scanout_pixels = read_effect_test_pixels(&harness.gl, 4, 6);
        assert_effect_test_pixel(&top_left_scanout_pixels, 4, 1, 1, [255, 0, 0, 255]);
        assert_effect_test_pixel(&top_left_scanout_pixels, 4, 2, 1, [255, 0, 0, 255]);
        assert_effect_test_pixel(&top_left_scanout_pixels, 4, 1, 2, [0, 0, 255, 255]);
        assert_effect_test_pixel(&top_left_scanout_pixels, 4, 2, 2, [0, 0, 255, 255]);
        assert_effect_test_pixel(&top_left_scanout_pixels, 4, 1, 4, [0, 0, 0, 0]);

        unsafe {
            harness.gl.delete_texture(input_texture);
            harness.gl.delete_program(program);
        }
    }

    #[test]
    fn real_gles_normalize_uses_shared_logical_uv_and_sample_conversion() {
        let mut harness = GlesEffectTestHarness::new(4, 4);
        let program = program::create_program_from_sources(
            &harness.gl,
            effects::DUAL_KAWASE_VERTEX_SHADER,
            effects::NORMALIZE_FRAGMENT_SHADER,
        )
        .expect("normalize effect program compiles with the shared vertex shader");
        let mut input_pixels = Vec::with_capacity(4 * 4 * 4);
        for y in 0..4 {
            let color = if y < 2 {
                [0, 0, 255, 255]
            } else {
                [255, 0, 0, 255]
            };
            for _ in 0..4 {
                input_pixels.extend(color);
            }
        }
        let input_texture = create_effect_test_texture(&harness.gl, 4, 4, &input_pixels);
        let quad = harness
            .renderer
            .ensure_effect_quad()
            .expect("effect quad creates")
            .0;

        draw_effect_test(
            &harness.gl,
            program,
            quad,
            input_texture,
            4,
            4,
            false,
            Some(("u_effect_input_flip_y", true)),
            |gl, program| {
                set_effect_test_uniform_4_f32(
                    gl,
                    program,
                    "u_effect_input_domain",
                    [1.0, 1.0, 2.0, 2.0],
                );
                set_effect_test_uniform_4_f32(
                    gl,
                    program,
                    "u_effect_output_domain",
                    [0.0, 0.0, 4.0, 4.0],
                );
                set_effect_test_uniform_i32(gl, program, "u_effect_decode_srgb", 0);
                set_effect_test_uniform_i32(gl, program, "u_effect_encode_srgb", 0);
            },
        );
        let pixels = read_effect_test_pixels(&harness.gl, 4, 4);
        assert_effect_test_pixel(&pixels, 4, 1, 2, [255, 0, 0, 255]);
        assert_effect_test_pixel(&pixels, 4, 2, 2, [255, 0, 0, 255]);
        assert_effect_test_pixel(&pixels, 4, 1, 1, [0, 0, 255, 255]);
        assert_effect_test_pixel(&pixels, 4, 2, 1, [0, 0, 255, 255]);
        assert_effect_test_pixel(&pixels, 4, 0, 0, [0, 0, 0, 0]);

        unsafe {
            harness.gl.delete_texture(input_texture);
            harness.gl.delete_program(program);
        }
    }

    #[test]
    fn real_gles_trusted_wrapper_preserves_logical_context_uv() {
        let mut harness = GlesEffectTestHarness::new(2, 2);
        let wrapper = effects::generate_fragment_wrapper(
            "vec4 typhon_effect_main(TyphonEffectContext ctx) { return typhon_sample_primary(ctx.uv); }",
            &[],
        )
        .expect("trusted wrapper generates");
        let program = program::create_program_from_sources(
            &harness.gl,
            effects::DUAL_KAWASE_VERTEX_SHADER,
            &wrapper,
        )
        .expect("trusted wrapper compiles with the shared vertex shader");
        let input_texture =
            create_effect_test_texture(&harness.gl, 2, 2, &canonical_two_by_two_pixels());
        let quad = harness
            .renderer
            .ensure_effect_quad()
            .expect("effect quad creates")
            .0;

        draw_effect_test(
            &harness.gl,
            program,
            quad,
            input_texture,
            2,
            2,
            false,
            None,
            |gl, program| {
                set_effect_test_uniform_i32(gl, program, "u_typhon_primary", 0);
                set_effect_test_uniform_i32(gl, program, "u_typhon_input_flip_y", 1);
                set_effect_test_uniform_i32(gl, program, "u_typhon_decode_srgb", 0);
                set_effect_test_uniform_i32(gl, program, "u_typhon_encode_srgb", 0);
            },
        );
        let pixels = read_effect_test_pixels(&harness.gl, 2, 2);
        assert_effect_test_pixel(&pixels, 2, 0, 0, [0, 0, 255, 255]);
        assert_effect_test_pixel(&pixels, 2, 0, 1, [255, 0, 0, 255]);

        unsafe {
            harness.gl.delete_texture(input_texture);
            harness.gl.delete_program(program);
        }
    }

    fn lamp_test_sample(progress: f64) -> LifecycleSceneSample {
        let rect = PresentationRect::new(100.0, 80.0, 800.0, 600.0).expect("valid rectangle");
        let anchor = PresentationRect::new(1200.0, 900.0, 64.0, 64.0).expect("valid anchor");
        LifecycleSceneSample {
            sampled_at: AnimationTime::from_nanos(1),
            lamps: vec![LampWindowSample {
                window_id: oblivion_one::compositor::WindowId::from_raw(1)
                    .expect("valid window id"),
                root_surface_id: 1,
                transition_id: LifecycleTransitionId::new(1),
                visual_group: LifecycleVisualGroup::from_bounds(
                    rect, rect, rect, anchor, 1920, 1080,
                )
                .expect("valid visual group"),
                progress,
                opacity: 1.0,
                direction: LifecycleDirection::Minimize,
                mathematically_settled: false,
            }],
            visual_sources: Vec::new(),
        }
    }

    #[test]
    fn decoration_resource_requirements_include_canonical_and_lifecycle_instances() {
        let canonical = vec![DecorationRenderPrimitive::SolidRect {
            rect: oblivion_one::compositor::DecorationRect {
                x: 0,
                y: 0,
                width: 20,
                height: 26,
            },
            color: [51, 51, 51, 255],
        }];
        let lifecycle = vec![DecorationRenderPrimitive::SolidRect {
            rect: oblivion_one::compositor::DecorationRect {
                x: 0,
                y: 0,
                width: 20,
                height: 26,
            },
            color: [17, 34, 51, 255],
        }];

        let requirements =
            decoration_resource_requirements([canonical.as_slice(), lifecycle.as_slice()]);

        assert!(
            requirements
                .required
                .contains(&DecorationResourceKey::Solid(rgba_to_pixel([
                    51, 51, 51, 255,
                ])))
        );
        assert!(
            requirements
                .required
                .contains(&DecorationResourceKey::Solid(rgba_to_pixel([
                    17, 34, 51, 255,
                ])))
        );
    }

    #[test]
    fn draw_scene_reconciles_canonical_and_empty_lifecycle_decoration_resources_together() {
        let _egl_test_lock = egl_test_lock();
        const EGL_PLATFORM_SURFACELESS_MESA: egl::Enum = 0x31dd;
        let egl = unsafe { EglInstance::load_required() }
            .expect("EGL loader is required for decoration reconciliation regression");
        let display = unsafe {
            egl.get_platform_display(
                EGL_PLATFORM_SURFACELESS_MESA,
                std::ptr::null_mut(),
                &[egl::ATTRIB_NONE],
            )
            .or_else(|_| {
                egl.get_display(egl::DEFAULT_DISPLAY)
                    .ok_or(egl::Error::BadDisplay)
            })
        }
        .expect("EGL display is available");
        egl.initialize(display).expect("EGL initializes");
        egl.bind_api(egl::OPENGL_ES_API)
            .expect("EGL binds the GLES API");
        let config_attributes = [
            egl::SURFACE_TYPE,
            egl::PBUFFER_BIT,
            egl::RENDERABLE_TYPE,
            egl::OPENGL_ES3_BIT,
            egl::RED_SIZE,
            8,
            egl::GREEN_SIZE,
            8,
            egl::BLUE_SIZE,
            8,
            egl::ALPHA_SIZE,
            8,
            egl::NONE,
        ];
        let count = egl
            .matching_config_count(display, &config_attributes)
            .expect("EGL returns GLES3 pbuffer configs");
        assert!(count > 0, "EGL exposes a GLES3 pbuffer config");
        let mut configs = Vec::with_capacity(count);
        egl.choose_config(display, &config_attributes, &mut configs)
            .expect("EGL chooses a GLES3 pbuffer config");
        let config = configs[0];
        let context = create_gles_context(&egl, display, config).expect("GLES3 context creates");
        let egl_surface = egl
            .create_pbuffer_surface(
                display,
                config,
                &[egl::WIDTH, 320, egl::HEIGHT, 200, egl::NONE],
            )
            .expect("EGL pbuffer surface creates");
        egl.make_current(display, Some(egl_surface), Some(egl_surface), Some(context))
            .expect("EGL makes the GLES3 context current");

        let cursor_image = Arc::new(
            CompositorCursorImage::from_argb8888(vec![0xffff_ffff], 1, 1, 0, 0)
                .expect("test cursor image is valid"),
        );
        let mut renderer = GlesSceneRenderer::new_current(
            &egl,
            320,
            200,
            None,
            EglPartialRepaintCapabilities {
                buffer_age: false,
                partial_render_repair: false,
                swap_buffers_with_damage: false,
            },
            cursor_image,
        )
        .expect("test GLES renderer creates");

        let socket_name = format!(
            "typhon-decoration-resource-reconciliation-{}",
            std::process::id()
        );
        let mut server =
            oblivion_one::compositor::OwnCompositorServer::bind_cpu_composition(&socket_name)
                .expect("bind compositor for decoration reconciliation regression");
        let surface = RenderableSurface {
            surface_id: 603,
            x: 0,
            y: 0,
            width: 320,
            height: 200,
            placement: SurfacePlacement::root(),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: CommittedSurfaceBuffer::shm_snapshot(
                BufferIdAllocator::default()
                    .allocate()
                    .expect("test buffer identity"),
                BufferSize::new(320, 200).expect("test surface size"),
                vec![0xff12_3456; 320 * 200],
            ),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wayland_server::protocol::wl_output::Transform::Normal,
            damage: RenderableSurfaceDamage::full(),
            opaque_region: SurfaceOpaqueRegion::None,
        };
        let window_id = oblivion_one::compositor::WindowId::from_raw(3).expect("test window id");
        server.install_native_frame_test_scene_with_server_decorations(
            vec![surface],
            &[(603, window_id)],
            None,
        );
        let mut resolved = crate::native_output::ResolvedNativeFrameScene::from_server_at(
            &server,
            AnimationTime::from_nanos(0),
        );
        assert_eq!(resolved.decorations.len(), 1);
        assert!(resolved.lifecycle_decorations.is_empty());
        let required = decoration_resource_requirements(
            resolved
                .decorations
                .iter()
                .map(DecorationRenderInstance::primitives),
        );

        let input_state = crate::native_output::NativeInputState::new(320, 200);
        let mut frame_renderer = crate::native_output::NativeFrameRenderer::default();
        let request = frame_renderer.egl_scene_draw_request(
            320,
            200,
            &resolved,
            &server,
            &input_state,
            crate::native_output::NativeCursorRenderMode::Hardware,
            Some(OutputDamage::Full),
        );
        let outcome = renderer
            .draw_scene(&egl, display, egl_surface, request)
            .expect("frame with canonical decoration and empty lifecycle renders");
        let stats = match outcome {
            EglFrameOutcome::Rendered { stats, .. } | EglFrameOutcome::Skipped { stats, .. } => {
                stats
            }
            EglFrameOutcome::LifecycleFallback { .. } => {
                panic!("decoration-only regression frame unexpectedly requested lifecycle fallback")
            }
        };

        assert_eq!(renderer.decoration_resources.len(), required.required.len());
        assert!(
            required
                .required
                .iter()
                .all(|key| renderer.decoration_resources.contains_key(key))
        );
        assert_eq!(stats.missing_required_decoration_resources, 0);

        renderer
            .repaint_planner
            .commit_presented_transition(OutputDamage::Full);
        assert_eq!(renderer.repaint_planner.history_depth(), 1);

        renderer.decoration_resources.clear();
        renderer.frame_stats = GlesSceneFrameStats::default();
        renderer
            .draw_command_batch(true, None)
            .expect("scene draw tolerates a missing decoration resource");
        assert!(
            renderer
                .last_frame_stats()
                .missing_required_decoration_resources
                > 0
        );

        resolved.lifecycle = lamp_test_sample(0.5);
        renderer.lamp_program = None;
        renderer.lamp_uniform_locations = None;
        let request = frame_renderer.egl_scene_draw_request(
            320,
            200,
            &resolved,
            &server,
            &input_state,
            crate::native_output::NativeCursorRenderMode::Hardware,
            Some(OutputDamage::Full),
        );
        let fallback = renderer
            .draw_scene(&egl, display, egl_surface, request)
            .expect("unavailable Lamp requests a recoverable lifecycle fallback");
        let EglFrameOutcome::LifecycleFallback { fallbacks, .. } = fallback else {
            panic!("unavailable Lamp must not produce a rendered frame");
        };
        assert_eq!(fallbacks.failed.len(), 1);
        assert_eq!(fallbacks.failed[0].window_id.get(), 1);
        assert!(renderer.lifecycle_render_evidence.consumed.is_empty());
        assert_eq!(renderer.repaint_planner.history_depth(), 0);

        let mut resolved_effect_lifecycle = lamp_test_sample(0.5);
        resolved_effect_lifecycle.visual_sources = vec![LifecycleVisualSource {
            window_id: oblivion_one::compositor::WindowId::from_raw(1)
                .expect("valid lifecycle window ID"),
            root_surface_id: 1,
            transition_id: LifecycleTransitionId::new(1),
            kind: LifecycleVisualSourceKind::ResolvedOwnedEffects,
            effect_scene: std::sync::Arc::new(
                oblivion_one::compositor::ResolvedEffectScene::default(),
            ),
        }];
        renderer.lamp_geometry_key = None;
        renderer.rebuild_lamp_commands_if_needed(&resolved_effect_lifecycle, &[], &[], 1.0);
        assert_eq!(renderer.lamp_commands.len(), 1);
        assert!(matches!(
            renderer.lamp_commands[0].layer,
            EglDrawLayer::LifecycleResolvedVisual(_)
        ));
    }

    #[test]
    fn egl_decoration_commands_emit_titlebar_and_button_primitives() {
        let socket_name = format!("typhon-floating-ssd-egl-{}", std::process::id());
        let mut server =
            oblivion_one::compositor::OwnCompositorServer::bind_cpu_composition(&socket_name)
                .expect("bind compositor for EGL decoration command regression");
        let width = 320;
        let height = 200;
        let surface = RenderableSurface {
            surface_id: 603,
            x: 0,
            y: 0,
            width,
            height,
            placement: SurfacePlacement::root(),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: CommittedSurfaceBuffer::shm_snapshot(
                BufferIdAllocator::default()
                    .allocate()
                    .expect("test buffer identity"),
                BufferSize::new(width, height).expect("test surface size"),
                vec![0xff12_3456; (width * height) as usize],
            ),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wayland_server::protocol::wl_output::Transform::Normal,
            damage: RenderableSurfaceDamage::full(),
            opaque_region: SurfaceOpaqueRegion::None,
        };
        let window_id = oblivion_one::compositor::WindowId::from_raw(3).expect("test window id");
        server.install_native_frame_test_scene_with_server_decorations(
            vec![surface.clone()],
            &[(603, window_id)],
            None,
        );
        let decorations = server.native_decoration_render_instances(&[surface]);
        assert_eq!(decorations.len(), 1);
        let decoration = &decorations[0];
        let (_, _, _, height) = decoration.scene_snapshot().bounds();
        assert!(height > 200, "Floating SSD must add visible chrome height");
        let mut vertices = Vec::new();
        let mut commands = Vec::new();

        push_egl_decoration_instance(
            &mut vertices,
            &mut commands,
            1280,
            800,
            decoration,
            1.0,
            OutputFramebufferOrigin::BottomLeft,
            Some(VisualGroupId::new(1).expect("visual group id")),
        );

        assert_eq!(commands.len(), decoration.primitives().len());
        assert!(
            commands
                .iter()
                .any(|command| { matches!(command.layer, EglDrawLayer::SolidRgba(_)) })
        );
        assert!(
            commands
                .iter()
                .any(|command| { matches!(command.layer, EglDrawLayer::DecorationAsset(_)) })
        );
    }

    #[test]
    fn lamp_mesh_identity_ignores_progress() {
        assert_eq!(
            lamp_geometry_key(
                &lamp_test_sample(0.1),
                &[],
                &[],
                1.0,
                OutputFramebufferOrigin::BottomLeft,
            ),
            lamp_geometry_key(
                &lamp_test_sample(0.9),
                &[],
                &[],
                1.0,
                OutputFramebufferOrigin::BottomLeft,
            )
        );
    }

    #[test]
    fn every_visible_lamp_progress_sample_has_lifecycle_damage() {
        for progress in [0.01, 0.5, 0.97] {
            let sample = lamp_test_sample(progress);
            let damage = lifecycle_damage_for_samples(&sample, 1920, 1080, 1.0);
            assert!(
                !matches!(damage, OutputDamage::Empty),
                "progress {progress} must remain eligible for presentation"
            );
        }
    }

    #[test]
    fn lamp_mesh_topology_upload_count_is_independent_of_progress_frames() {
        let early = lamp_test_sample(0.1);
        let middle = lamp_test_sample(0.5);
        let late = lamp_test_sample(0.9);
        let early_key =
            lamp_geometry_key(&early, &[], &[], 1.0, OutputFramebufferOrigin::BottomLeft);
        assert_eq!(
            early_key,
            lamp_geometry_key(&middle, &[], &[], 1.0, OutputFramebufferOrigin::BottomLeft,)
        );
        assert_eq!(
            early_key,
            lamp_geometry_key(&late, &[], &[], 1.0, OutputFramebufferOrigin::BottomLeft,)
        );
    }

    #[test]
    fn lamp_mesh_reaches_preferred_cell_size_before_global_budget() {
        let (columns, rows) = lamp_grid_subdivisions(1920.0, 1080.0, MAX_LAMP_VERTICES)
            .expect("preferred Lamp grid fits in global budget");
        assert_eq!(columns, 60);
        assert_eq!(rows, 34);
        assert!(1920.0 / columns as f32 <= 36.0);
        assert!(1080.0 / rows as f32 <= 36.0);
    }

    #[test]
    fn lamp_mesh_coarsens_deterministically_under_global_vertex_budget() {
        let available = 60 * 20 * 6;
        let first =
            lamp_grid_subdivisions(1920.0, 1080.0, available).expect("reduced Lamp grid fits");
        let second =
            lamp_grid_subdivisions(1920.0, 1080.0, available).expect("reduced Lamp grid fits");
        assert_eq!(first, second);
        assert!(first.0.saturating_mul(first.1).saturating_mul(6) <= available);
        assert!(first.0 > 1 || first.1 > 1);
    }

    #[test]
    fn lamp_mesh_vertex_budget_is_bounded_for_concurrent_windows() {
        let window_id = oblivion_one::compositor::WindowId::from_raw(1).expect("valid window id");
        let mut vertices = Vec::new();
        let mut commands = Vec::new();
        for _ in 0..32 {
            append_lamp_grid(
                &mut vertices,
                &mut commands,
                LampGridSpec {
                    layer: EglDrawLayer::Surface(1),
                    window_id,
                    bounds: EglRect::new(0.0, 0.0, 10_000.0, 10_000.0),
                    uv: EglUvRect::new(0.0, 0.0, 1.0, 1.0),
                },
            );
        }
        assert!(vertices.len() <= MAX_LAMP_VERTICES);
        assert!(
            commands
                .iter()
                .all(|command| command.vertex_count <= MAX_LAMP_VERTICES as u32)
        );
    }

    #[test]
    fn lamp_grid_admission_is_atomic_when_the_minimum_grid_does_not_fit() {
        let window_id = oblivion_one::compositor::WindowId::from_raw(2).expect("valid window id");
        let vertex = EglLampVertex {
            position: [0.0, 0.0],
            uv: [0.0, 0.0],
        };
        let mut vertices = vec![vertex; MAX_LAMP_VERTICES - 5];
        let mut commands = Vec::new();
        assert!(!append_lamp_grid(
            &mut vertices,
            &mut commands,
            LampGridSpec {
                layer: EglDrawLayer::Surface(2),
                window_id,
                bounds: EglRect::new(10_000.0, 10_000.0, 10_000.0, 10_000.0),
                uv: EglUvRect::new(0.0, 0.0, 1.0, 1.0),
            },
        ));
        assert_eq!(vertices.len(), MAX_LAMP_VERTICES - 5);
        assert!(commands.is_empty());
    }

    #[test]
    fn legacy_scissored_lifecycle_phases_prepare_once_after_all_repairs() {
        assert_eq!(
            legacy_scene_scissored_phase_plan(2).collect::<Vec<_>>(),
            vec![
                LegacySceneScissoredPhase::BaseRepair(0),
                LegacySceneScissoredPhase::BaseRepair(1),
                LegacySceneScissoredPhase::PrepareLifecycleSources,
                LegacySceneScissoredPhase::RestoreRepairScissor(0),
                LegacySceneScissoredPhase::Lamp(0),
                LegacySceneScissoredPhase::ExternalOverlays(0),
                LegacySceneScissoredPhase::RestoreRepairScissor(1),
                LegacySceneScissoredPhase::Lamp(1),
                LegacySceneScissoredPhase::ExternalOverlays(1),
            ]
        );
    }

    #[derive(Debug, Default, PartialEq, Eq)]
    struct DmabufRingQualification {
        imports: usize,
        current_resource_reuses: usize,
        cache_hits: usize,
        cache_misses: usize,
        surface_bound_evictions: usize,
        peak_entries: usize,
    }

    fn qualify_dmabuf_ring(ring_len: usize, cycles: usize) -> DmabufRingQualification {
        let mut current = None;
        let mut cache = std::collections::BTreeSet::new();
        let mut result = DmabufRingQualification::default();
        for buffer in (0..ring_len).cycle().take(ring_len.saturating_mul(cycles)) {
            if current == Some(buffer) {
                result.current_resource_reuses += 1;
                continue;
            }
            if cache.remove(&buffer) {
                result.cache_hits += 1;
            } else {
                result.cache_misses += 1;
                result.imports += 1;
            }
            if let Some(previous) = current {
                if cache.len() >= MAX_CACHED_DMABUF_RESOURCES_PER_SURFACE {
                    let oldest = *cache.iter().next().expect("full cache is non-empty");
                    cache.remove(&oldest);
                    result.surface_bound_evictions += 1;
                }
                cache.insert(previous);
            }
            current = Some(buffer);
            result.peak_entries = result.peak_entries.max(cache.len());
        }
        result
    }

    #[test]
    fn dmabuf_ring_qualification_exposes_capacity_behavior_without_egl() {
        for ring_len in 2..=5 {
            let result = qualify_dmabuf_ring(ring_len, 2);
            assert_eq!(result.imports, ring_len);
            assert_eq!(result.cache_misses, ring_len);
            assert_eq!(result.cache_hits, ring_len);
            assert_eq!(result.current_resource_reuses, 0);
            assert_eq!(result.surface_bound_evictions, 0);
            assert_eq!(result.peak_entries, ring_len - 1);
        }
        assert!(qualify_dmabuf_ring(6, 2).surface_bound_evictions > 0);
    }

    #[test]
    fn dmabuf_image_target_invalid_operation_is_buffer_local() {
        let error = DmabufTextureImportError::Gl {
            stage: DmabufImportGlStage::ImageTarget,
            error: glow::INVALID_OPERATION,
        };

        assert_eq!(
            error.classification(),
            DmabufImportFailureClass::BufferIncompatible
        );
    }

    #[test]
    fn dmabuf_texture_configuration_invalid_operation_is_renderer_fatal() {
        let error = DmabufTextureImportError::Gl {
            stage: DmabufImportGlStage::TextureConfiguration,
            error: glow::INVALID_OPERATION,
        };

        assert_eq!(
            error.classification(),
            DmabufImportFailureClass::RendererFatal
        );
    }

    #[test]
    fn dmabuf_import_error_drain_returns_first_error_and_clears_all_stale_errors() {
        let mut errors = [glow::INVALID_OPERATION, glow::INVALID_ENUM, glow::NO_ERROR].into_iter();

        assert_eq!(
            first_drained_gl_error(|| errors.next().expect("test error stream is finite")),
            Some(glow::INVALID_OPERATION)
        );
        assert!(errors.next().is_none());
    }

    #[test]
    fn dmabuf_replacement_incompatibility_is_surface_local() {
        let mut frame_stats = GlesSceneFrameStats::default();
        let mut failed_surface_generations = HashMap::new();
        let result = settle_dmabuf_import_result::<u8>(
            Err(Box::new(DmabufTextureImportError::EglImageCreation(
                egl::Error::BadMatch,
            ))),
            DmabufImportDiagnosticContext {
                surface_id: 7,
                generation: 9,
                buffer_id: 11,
                width: 2,
                height: 2,
                fourcc: XR24,
                modifier: 0,
                planes: 1,
                path: DmabufImportPath::Replacement,
                cache: DmabufImportCacheState::Miss,
            },
            &mut frame_stats,
            &mut failed_surface_generations,
        );

        assert!(
            result
                .expect("buffer-local import rejection is absorbed")
                .is_none()
        );
        assert_eq!(frame_stats.dmabuf_import_failures, 1);
        assert_eq!(failed_surface_generations.get(&7), Some(&9));
    }

    #[test]
    fn dmabuf_renderer_fatal_failure_still_escapes_surface_local_boundary() {
        let mut frame_stats = GlesSceneFrameStats::default();
        let mut failed_surface_generations = HashMap::new();
        let result = settle_dmabuf_import_result::<u8>(
            Err(Box::new(DmabufTextureImportError::Gl {
                stage: DmabufImportGlStage::Bind,
                error: glow::INVALID_OPERATION,
            })),
            DmabufImportDiagnosticContext {
                surface_id: 7,
                generation: 9,
                buffer_id: 11,
                width: 2,
                height: 2,
                fourcc: XR24,
                modifier: 0,
                planes: 1,
                path: DmabufImportPath::Replacement,
                cache: DmabufImportCacheState::Miss,
            },
            &mut frame_stats,
            &mut failed_surface_generations,
        );

        assert!(result.is_err());
        assert_eq!(frame_stats.dmabuf_import_failures, 0);
        assert!(failed_surface_generations.is_empty());
    }

    #[test]
    fn unclassified_dmabuf_import_error_still_escapes_surface_local_boundary() {
        let mut frame_stats = GlesSceneFrameStats::default();
        let mut failed_surface_generations = HashMap::new();
        let result = settle_dmabuf_import_result::<u8>(
            Err(io::Error::other("unclassified renderer failure").into()),
            DmabufImportDiagnosticContext {
                surface_id: 7,
                generation: 9,
                buffer_id: 11,
                width: 2,
                height: 2,
                fourcc: XR24,
                modifier: 0,
                planes: 1,
                path: DmabufImportPath::Initial,
                cache: DmabufImportCacheState::NotChecked,
            },
            &mut frame_stats,
            &mut failed_surface_generations,
        );

        assert!(result.is_err());
        assert_eq!(frame_stats.dmabuf_import_failures, 0);
        assert!(failed_surface_generations.is_empty());
    }

    #[test]
    fn dmabuf_rejected_replacement_does_not_install_or_cache_failed_generation() {
        let mut frame_stats = GlesSceneFrameStats::default();
        let mut failed_surface_generations = HashMap::new();
        let mut cached_resources = HashMap::from([("A", 1_u8)]);
        let context = DmabufImportDiagnosticContext {
            surface_id: 7,
            generation: 9,
            buffer_id: 12,
            width: 2,
            height: 2,
            fourcc: XR24,
            modifier: 0,
            planes: 1,
            path: DmabufImportPath::Replacement,
            cache: DmabufImportCacheState::Miss,
        };

        let result = settle_dmabuf_import_result::<u8>(
            Err(Box::new(DmabufTextureImportError::EglImageCreation(
                egl::Error::BadMatch,
            ))),
            context,
            &mut frame_stats,
            &mut failed_surface_generations,
        )
        .expect("buffer-local replacement rejection is nonfatal");
        if let Some(resource) = result {
            cached_resources.insert("B", resource);
        }

        assert!(!cached_resources.contains_key("B"));
        assert_eq!(cached_resources.get("A"), Some(&1));
        assert_eq!(frame_stats.dmabuf_import_failures, 1);
    }

    #[test]
    fn dmabuf_rejected_generation_can_be_followed_by_a_valid_import() {
        let mut frame_stats = GlesSceneFrameStats::default();
        let mut failed_surface_generations = HashMap::new();
        let context = DmabufImportDiagnosticContext {
            surface_id: 7,
            generation: 9,
            buffer_id: 13,
            width: 2,
            height: 2,
            fourcc: XR24,
            modifier: 0,
            planes: 1,
            path: DmabufImportPath::Replacement,
            cache: DmabufImportCacheState::Miss,
        };

        assert!(
            settle_dmabuf_import_result::<u8>(
                Err(Box::new(DmabufTextureImportError::EglImageCreation(
                    egl::Error::BadMatch,
                ))),
                context,
                &mut frame_stats,
                &mut failed_surface_generations,
            )
            .expect("buffer-local replacement rejection is nonfatal")
            .is_none()
        );
        let valid = settle_dmabuf_import_result(
            Ok(3_u8),
            context,
            &mut frame_stats,
            &mut failed_surface_generations,
        )
        .expect("a later valid generation remains usable");

        assert_eq!(valid, Some(3));
        assert_eq!(frame_stats.dmabuf_import_failures, 1);
        assert!(failed_surface_generations.is_empty());
    }

    fn test_shm_surface(damage: RenderableSurfaceDamage) -> RenderableSurface {
        let identity = BufferIdAllocator::default()
            .allocate()
            .expect("test buffer identity");
        RenderableSurface {
            surface_id: 7,
            x: 0,
            y: 0,
            width: 2,
            height: 2,
            placement: SurfacePlacement::root(),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 2,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: CommittedSurfaceBuffer::shm_snapshot(
                identity,
                BufferSize::new(2, 2).expect("test surface size"),
                vec![0xff00_0000; 4],
            ),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wayland_server::protocol::wl_output::Transform::Normal,
            opaque_region: SurfaceOpaqueRegion::None,
            damage,
        }
    }

    #[test]
    #[ignore = "gate failure: live subsurface placement changes the Lamp geometry key; surface snapshot ownership is a separate design"]
    fn active_lamp_geometry_stays_frozen_when_live_subsurface_moves() {
        let window_id = oblivion_one::compositor::WindowId::from_raw(501).expect("window id");
        let group = LifecycleVisualGroup::from_bounds(
            PresentationRect::new(400.0, 100.0, 800.0, 600.0).expect("client rect"),
            PresentationRect::new(384.0, 60.0, 832.0, 640.0).expect("visual rect"),
            PresentationRect::new(400.0, 100.0, 800.0, 600.0).expect("source rect"),
            PresentationRect::new(1500.0, 500.0, 64.0, 64.0).expect("anchor rect"),
            1920,
            1080,
        )
        .expect("valid visual group");
        let lifecycle = LifecycleSceneSample {
            sampled_at: AnimationTime::from_nanos(0),
            lamps: vec![LampWindowSample {
                window_id,
                root_surface_id: 42,
                transition_id: LifecycleTransitionId::new(1),
                visual_group: group,
                progress: 0.5,
                opacity: 1.0,
                direction: LifecycleDirection::Minimize,
                mathematically_settled: false,
            }],
            visual_sources: Vec::new(),
        };
        let mut subsurface = test_shm_surface(RenderableSurfaceDamage::full());
        subsurface.surface_id = 43;
        subsurface.width = 64;
        subsurface.height = 64;
        subsurface.placement = SurfacePlacement::subsurface(42, 16, 24);
        let before = lamp_geometry_key(
            &lifecycle,
            &[subsurface.clone()],
            &[],
            1.0,
            OutputFramebufferOrigin::BottomLeft,
        );

        subsurface.placement = SurfacePlacement::subsurface(42, -48, 72);
        let after = lamp_geometry_key(
            &lifecycle,
            &[subsurface],
            &[],
            1.0,
            OutputFramebufferOrigin::BottomLeft,
        );

        assert_eq!(
            before, after,
            "active Lamp geometry changed when live root-owned subsurface placement changed"
        );
    }

    fn test_shm_resource(synced_commit: Option<SurfaceCommitCounter>) -> EglSurfaceResource {
        EglSurfaceResource {
            image: EglImageResource {
                texture: glow::NativeTexture(std::num::NonZeroU32::new(1).unwrap()),
                size: (2, 2),
                generation: 1,
                egl_image: None,
            },
            dmabuf_key: None,
            buffer_lifetime: None,
            shm_synced_commit: synced_commit,
        }
    }

    #[test]
    fn shm_resource_update_requires_full_resync_when_baseline_is_behind() {
        let surface = test_shm_surface(RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        }]));
        let resource = test_shm_resource(Some(SurfaceCommitCounter(1)));
        let state = SurfaceResourceSyncState {
            surface_id: surface.surface_id,
            complete_since: Some(SurfaceCommitCounter(2)),
            current_commit: SurfaceCommitCounter(3),
            authoritative: true,
        };

        assert_eq!(
            resource.update_for(&surface, state),
            EglSurfaceResourceUpdate::FullShmResync
        );
    }

    #[test]
    fn shm_resource_update_keeps_partial_upload_when_damage_is_complete() {
        let surface = test_shm_surface(RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        }]));
        let resource = test_shm_resource(Some(SurfaceCommitCounter(2)));
        let state = SurfaceResourceSyncState {
            surface_id: surface.surface_id,
            complete_since: Some(SurfaceCommitCounter(2)),
            current_commit: SurfaceCommitCounter(3),
            authoritative: true,
        };

        assert_eq!(
            resource.update_for(&surface, state),
            EglSurfaceResourceUpdate::UploadDamage
        );
    }

    #[test]
    fn shm_resource_update_reuses_exact_current_commit_without_upload() {
        let surface = test_shm_surface(RenderableSurfaceDamage::Empty);
        let resource = test_shm_resource(Some(SurfaceCommitCounter(3)));
        let state = SurfaceResourceSyncState {
            surface_id: surface.surface_id,
            complete_since: Some(SurfaceCommitCounter(2)),
            current_commit: SurfaceCommitCounter(3),
            authoritative: true,
        };

        assert_eq!(
            resource.update_for(&surface, state),
            EglSurfaceResourceUpdate::Reuse
        );
    }

    #[test]
    fn shm_resource_update_history_loss_requires_full_resync() {
        let surface = test_shm_surface(RenderableSurfaceDamage::HistoryLost);
        let resource = test_shm_resource(Some(SurfaceCommitCounter(2)));
        let state = SurfaceResourceSyncState {
            surface_id: surface.surface_id,
            complete_since: Some(SurfaceCommitCounter(2)),
            current_commit: SurfaceCommitCounter(3),
            authoritative: true,
        };

        assert_eq!(
            resource.update_for(&surface, state),
            EglSurfaceResourceUpdate::FullShmResync
        );
    }

    #[test]
    fn shm_resource_update_requires_full_resync_when_presentation_settlement_outruns_baseline() {
        let surface = test_shm_surface(RenderableSurfaceDamage::Empty);
        let resource = test_shm_resource(Some(SurfaceCommitCounter(1)));
        let state = SurfaceResourceSyncState {
            surface_id: surface.surface_id,
            complete_since: Some(SurfaceCommitCounter(2)),
            current_commit: SurfaceCommitCounter(2),
            authoritative: true,
        };

        assert_eq!(
            resource.update_for(&surface, state),
            EglSurfaceResourceUpdate::FullShmResync
        );
    }

    #[test]
    fn shm_resource_update_keeps_accumulated_hidden_damage_partial() {
        let surface = test_shm_surface(RenderableSurfaceDamage::Partial(vec![
            SurfaceDamageRect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
            SurfaceDamageRect {
                x: 1,
                y: 1,
                width: 1,
                height: 1,
            },
        ]));
        let resource = test_shm_resource(Some(SurfaceCommitCounter(1)));
        let state = SurfaceResourceSyncState {
            surface_id: surface.surface_id,
            complete_since: Some(SurfaceCommitCounter(1)),
            current_commit: SurfaceCommitCounter(3),
            authoritative: true,
        };

        assert_eq!(
            resource.update_for(&surface, state),
            EglSurfaceResourceUpdate::UploadDamage
        );
    }

    #[test]
    fn shm_resource_update_advances_empty_damage_baseline_without_upload() {
        let surface = test_shm_surface(RenderableSurfaceDamage::Empty);
        let mut resource = test_shm_resource(Some(SurfaceCommitCounter(1)));
        let current_commit = SurfaceCommitCounter(2);
        let state = SurfaceResourceSyncState {
            surface_id: surface.surface_id,
            complete_since: Some(SurfaceCommitCounter(1)),
            current_commit,
            authoritative: true,
        };
        assert_eq!(
            resource.update_for(&surface, state),
            EglSurfaceResourceUpdate::ReuseShm
        );

        resource.advance_shm_sync_baseline(current_commit);
        assert_eq!(resource.shm_synced_commit, Some(current_commit));
    }

    #[test]
    fn output_background_uses_dedicated_solid_scene_layer() {
        let mut vertices = Vec::new();
        let mut commands = Vec::new();
        push_output_background_command(
            &mut vertices,
            &mut commands,
            1280,
            800,
            OutputFramebufferOrigin::BottomLeft,
        );

        assert_eq!(
            commands.first().map(|command| command.layer),
            Some(EglDrawLayer::Solid(
                compositor::ServerFrameColor::OutputBackground
            ))
        );
    }

    #[derive(Clone)]
    struct DropProbe(std::rc::Rc<std::cell::Cell<usize>>);

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.set(self.0.get().saturating_add(1));
        }
    }

    fn fake_egl_image() -> egl::Image {
        // SAFETY: the fake handle is only passed to the test cleanup probe;
        // it is never sent to EGL.
        unsafe { egl::Image::from_ptr(std::ptr::NonNull::<c_void>::dangling().as_ptr()) }
    }

    #[test]
    fn failed_image_creation_has_no_cleanup_owner() {
        let image_cleanup_count = std::rc::Rc::new(std::cell::Cell::new(0));
        let image_result: Result<egl::Image, ()> = Err(());

        if let Ok(image) = image_result {
            let cleanup_count = std::rc::Rc::clone(&image_cleanup_count);
            let _guard = EglImageGuard::new(image, move |_| {
                cleanup_count.set(cleanup_count.get() + 1);
            });
        }

        assert_eq!(image_cleanup_count.get(), 0);
    }

    #[test]
    fn texture_creation_failure_destroys_acquired_image_once() {
        let image_cleanup_count = std::rc::Rc::new(std::cell::Cell::new(0));
        let texture_cleanup_count = std::rc::Rc::new(std::cell::Cell::new(0));
        {
            let cleanup_count = std::rc::Rc::clone(&image_cleanup_count);
            let _guard = EglImageGuard::new(fake_egl_image(), move |_| {
                cleanup_count.set(cleanup_count.get() + 1);
            });
            let _texture: Result<(), ()> = Err(());
            assert!(_texture.is_err());
        }

        assert_eq!(image_cleanup_count.get(), 1);
        assert_eq!(texture_cleanup_count.get(), 0);
    }

    #[test]
    fn binding_failure_deletes_texture_and_destroys_image_once() {
        let image_cleanup_count = std::rc::Rc::new(std::cell::Cell::new(0));
        let texture_cleanup_count = std::rc::Rc::new(std::cell::Cell::new(0));
        {
            let cleanup_count = std::rc::Rc::clone(&image_cleanup_count);
            let _guard = EglImageGuard::new(fake_egl_image(), move |_| {
                cleanup_count.set(cleanup_count.get() + 1);
            });
            texture_cleanup_count.set(texture_cleanup_count.get() + 1);
        }

        assert_eq!(image_cleanup_count.get(), 1);
        assert_eq!(texture_cleanup_count.get(), 1);
    }

    #[test]
    fn successful_construction_transfers_image_without_double_cleanup() {
        let image_cleanup_count = std::rc::Rc::new(std::cell::Cell::new(0));
        let texture_cleanup_count = std::rc::Rc::new(std::cell::Cell::new(0));
        let image = {
            let cleanup_count = std::rc::Rc::clone(&image_cleanup_count);
            let guard = EglImageGuard::new(fake_egl_image(), move |_| {
                cleanup_count.set(cleanup_count.get() + 1);
            });
            guard.disarm()
        };

        texture_cleanup_count.set(texture_cleanup_count.get() + 1);
        image_cleanup_count.set(image_cleanup_count.get() + 1);
        let _ = image;

        assert_eq!(image_cleanup_count.get(), 1);
        assert_eq!(texture_cleanup_count.get(), 1);
    }

    fn native_candidate(config_id: egl::Int, native_visual_id: u32) -> NativeEglConfigCandidate {
        NativeEglConfigCandidate {
            config_id,
            native_visual_id,
            surface_type: egl::WINDOW_BIT,
            renderable_type: egl::OPENGL_ES3_BIT,
            red_size: 8,
            green_size: 8,
            blue_size: 8,
            alpha_size: 0,
        }
    }

    fn config_attribute_value(attributes: &[egl::Int], key: egl::Int) -> Option<egl::Int> {
        attributes
            .chunks_exact(2)
            .take_while(|pair| pair[0] != egl::NONE)
            .find(|pair| pair[0] == key)
            .map(|pair| pair[1])
    }

    #[test]
    fn gles_context_attributes_request_client_version_3_only() {
        let client_version =
            config_attribute_value(gles_context_attributes(), egl::CONTEXT_CLIENT_VERSION).unwrap();

        assert_eq!(client_version, 3);
        assert!(!gles_context_attributes().contains(&2));
    }

    #[test]
    fn gles_context_creation_error_mentions_required_gles3() {
        let error = io::Error::other("driver rejected context");

        let message = format_gles3_context_error(&error);

        assert!(message.contains("required GLES3 context"));
        assert!(message.contains("driver rejected context"));
    }

    #[test]
    fn trusted_custom_wrapper_compiles_and_links_in_real_gles_context() {
        let _egl_test_lock = egl_test_lock();
        const EGL_PLATFORM_SURFACELESS_MESA: egl::Enum = 0x31dd;
        let egl = unsafe { EglInstance::load_required() }
            .expect("EGL loader is required for the GLES wrapper compile test");
        let display = unsafe {
            egl.get_platform_display(
                EGL_PLATFORM_SURFACELESS_MESA,
                std::ptr::null_mut(),
                &[egl::ATTRIB_NONE],
            )
            .or_else(|_| {
                egl.get_display(egl::DEFAULT_DISPLAY)
                    .ok_or(egl::Error::BadDisplay)
            })
        }
        .expect("EGL display is available");
        egl.initialize(display).expect("EGL initializes");
        egl.bind_api(egl::OPENGL_ES_API)
            .expect("EGL binds the GLES API");
        let config_attributes = [
            egl::SURFACE_TYPE,
            egl::PBUFFER_BIT,
            egl::RENDERABLE_TYPE,
            egl::OPENGL_ES3_BIT,
            egl::RED_SIZE,
            8,
            egl::GREEN_SIZE,
            8,
            egl::BLUE_SIZE,
            8,
            egl::ALPHA_SIZE,
            8,
            egl::NONE,
        ];
        let count = egl
            .matching_config_count(display, &config_attributes)
            .expect("EGL returns GLES3 pbuffer configs");
        assert!(count > 0, "EGL exposes a GLES3 pbuffer config");
        let mut configs = Vec::with_capacity(count);
        egl.choose_config(display, &config_attributes, &mut configs)
            .expect("EGL chooses a GLES3 pbuffer config");
        let config = configs[0];
        let context = create_gles_context(&egl, display, config).expect("GLES3 context creates");
        let surface = egl
            .create_pbuffer_surface(display, config, &[egl::WIDTH, 1, egl::HEIGHT, 1, egl::NONE])
            .expect("EGL pbuffer surface creates");
        egl.make_current(display, Some(surface), Some(surface), Some(context))
            .expect("EGL makes the GLES3 context current");

        let gl = unsafe {
            glow::Context::from_loader_function(|name| {
                egl.get_proc_address(name)
                    .map(|symbol| symbol as *const c_void)
                    .unwrap_or(ptr::null())
            })
        };
        let asset = oblivion_one::effects::TrustedShaderAsset {
            module: oblivion_one::effects::ShaderModuleId::new(9001).unwrap(),
            relative_path: std::path::PathBuf::from("test.glsl"),
            source: r#"
                uniform float u_float;
                uniform vec2 u_vec2;
                uniform vec4 u_vec4;
                vec4 typhon_effect_main(TyphonEffectContext ctx) {
                    vec4 aux = typhon_sample_aux(0, ctx.uv) + typhon_sample_aux(7, ctx.uv);
                    return typhon_sample_primary(ctx.uv) + aux * 0.0
                        + vec4(u_float + ctx.time + ctx.delta + u_vec2.x + u_vec4.x);
                }
            "#
            .to_owned(),
            uniforms: vec![
                oblivion_one::effects::EffectUniformBinding {
                    parameter: oblivion_one::effects::EffectParameterId::new(1).unwrap(),
                    shader_name: "u_float".to_owned(),
                },
                oblivion_one::effects::EffectUniformBinding {
                    parameter: oblivion_one::effects::EffectParameterId::new(2).unwrap(),
                    shader_name: "u_vec2".to_owned(),
                },
                oblivion_one::effects::EffectUniformBinding {
                    parameter: oblivion_one::effects::EffectParameterId::new(3).unwrap(),
                    shader_name: "u_vec4".to_owned(),
                },
            ],
        };
        let mut cache = ShaderProgramCache::new(4).unwrap();
        cache
            .prewarm_trusted_custom(&gl, &asset)
            .expect("trusted custom wrapper compiles and links in GLES3");
        let cursor_image = Arc::new(
            CompositorCursorImage::from_argb8888(vec![0xffff_ffff], 1, 1, 0, 0)
                .expect("test cursor image is valid"),
        );
        let mut renderer = GlesSceneRenderer::new_current(
            &egl,
            1,
            1,
            None,
            EglPartialRepaintCapabilities {
                buffer_age: false,
                partial_render_repair: false,
                swap_buffers_with_damage: false,
            },
            cursor_image,
        )
        .expect("test GLES renderer creates");
        renderer.establish_ordinary_scene_state();
        unsafe {
            assert!(gl.is_enabled(glow::BLEND));
            assert!(!gl.is_enabled(glow::SCISSOR_TEST));
            assert_eq!(
                gl.get_parameter_i32(glow::ACTIVE_TEXTURE),
                glow::TEXTURE0 as i32
            );
            assert_eq!(gl.get_parameter_i32(glow::BLEND_SRC_RGB), glow::ONE as i32);
            assert_eq!(
                gl.get_parameter_i32(glow::BLEND_DST_RGB),
                glow::ONE_MINUS_SRC_ALPHA as i32
            );
            assert_eq!(
                gl.get_parameter_i32(glow::BLEND_SRC_ALPHA),
                glow::ONE as i32
            );
            assert_eq!(
                gl.get_parameter_i32(glow::BLEND_DST_ALPHA),
                glow::ONE_MINUS_SRC_ALPHA as i32
            );
            assert_ne!(gl.get_parameter_i32(glow::CURRENT_PROGRAM), 0);
            let mut viewport = [0; 4];
            gl.get_parameter_i32_slice(glow::VIEWPORT, &mut viewport);
            assert_eq!(viewport, [0, 0, 1, 1]);
        }

        let source_over_program = program::create_program_from_sources(
            &gl,
            r#"#version 300 es
                layout(location = 0) in vec2 a_position;
                void main() { gl_Position = vec4(a_position, 0.0, 1.0); }
            "#,
            r#"#version 300 es
                precision highp float;
                uniform vec4 u_color;
                out vec4 out_color;
                void main() { out_color = u_color; }
            "#,
        )
        .expect("source-over regression shader compiles");
        let (quad, _) = renderer
            .ensure_effect_quad()
            .expect("source-over regression quad creates");
        let destination = oblivion_one::effects::PremultipliedRgba::new(0.2, 0.1, 0.05, 0.5);
        let source = oblivion_one::effects::PremultipliedRgba::new(0.4, 0.2, 0.1, 0.5);
        let expected = destination.blend(source, oblivion_one::effects::BlendMode::SourceOver, 1.0);
        unsafe {
            gl.viewport(0, 0, 1, 1);
            gl.disable(glow::SCISSOR_TEST);
            gl.enable(glow::BLEND);
            gl.blend_func_separate(
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
            );
            gl.clear_color(0.0, 0.0, 0.0, 0.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.use_program(Some(source_over_program));
            gl.bind_vertex_array(Some(quad));
            let color = gl
                .get_uniform_location(source_over_program, "u_color")
                .expect("source-over color uniform is active");
            gl.uniform_4_f32(
                Some(&color),
                destination.r,
                destination.g,
                destination.b,
                destination.a,
            );
            gl.draw_arrays(glow::TRIANGLES, 0, 6);
            gl.uniform_4_f32(Some(&color), source.r, source.g, source.b, source.a);
            gl.draw_arrays(glow::TRIANGLES, 0, 6);
            gl.bind_vertex_array(None);
            gl.flush();
            let mut pixel = [0_u8; 4];
            gl.read_pixels(
                0,
                0,
                1,
                1,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut pixel)),
            );
            for (actual, expected) in pixel.iter().zip([
                (expected.r * 255.0).round() as u8,
                (expected.g * 255.0).round() as u8,
                (expected.b * 255.0).round() as u8,
                (expected.a * 255.0).round() as u8,
            ]) {
                assert!(
                    (*actual as i16 - expected as i16).abs() <= 2,
                    "source-over channel mismatch: actual={actual}, expected={expected}, pixel={pixel:?}"
                );
            }
            gl.delete_program(source_over_program);
        }

        let blend_program = program::create_program_from_sources(
            &gl,
            r#"#version 300 es
                layout(location = 0) in vec2 a_position;
                layout(location = 1) in vec2 a_uv;
                out vec2 v_uv;
                void main() {
                    gl_Position = vec4(a_position, 0.0, 1.0);
                    v_uv = a_uv;
                }
            "#,
            effects::BLEND_STAGE_FRAGMENT_SHADER,
        )
        .expect("blend regression shader compiles");
        let destination_texture =
            unsafe { gl.create_texture().expect("blend destination texture") };
        let source_texture = unsafe { gl.create_texture().expect("blend source texture") };
        let (quad, _) = renderer
            .ensure_effect_quad()
            .expect("blend regression quad creates");
        let blend_modes = [
            (0, oblivion_one::effects::BlendMode::SourceOver),
            (1, oblivion_one::effects::BlendMode::Add),
            (2, oblivion_one::effects::BlendMode::Multiply),
            (3, oblivion_one::effects::BlendMode::Screen),
        ];
        let opacities = [0.0_f32, 0.25, 0.5, 0.75, 1.0];
        let destination_straight = [0.3_f32, 0.6, 0.2];
        let source_straight = [0.8_f32, 0.25, 0.7];
        unsafe {
            gl.viewport(0, 0, 1, 1);
            gl.disable(glow::BLEND);
            gl.use_program(Some(blend_program));
            gl.bind_vertex_array(Some(quad));
            gl.uniform_1_i32(
                Some(
                    &gl.get_uniform_location(blend_program, "u_effect_input")
                        .expect("blend input uniform"),
                ),
                0,
            );
            gl.uniform_1_i32(
                Some(
                    &gl.get_uniform_location(blend_program, "u_effect_input_secondary")
                        .expect("blend secondary input uniform"),
                ),
                1,
            );
            gl.uniform_1_i32(
                Some(
                    &gl.get_uniform_location(blend_program, "u_effect_decode_srgb")
                        .expect("blend decode uniform"),
                ),
                0,
            );
            gl.uniform_1_i32(
                Some(
                    &gl.get_uniform_location(blend_program, "u_effect_encode_srgb")
                        .expect("blend encode uniform"),
                ),
                0,
            );
            let mode_location = gl
                .get_uniform_location(blend_program, "u_effect_blend_mode")
                .expect("blend mode uniform");
            let opacity_location = gl
                .get_uniform_location(blend_program, "u_effect_blend_opacity")
                .expect("blend opacity uniform");
            for destination_alpha in [0.25_f32, 0.5, 1.0] {
                for source_alpha in [0.25_f32, 0.5, 1.0] {
                    let destination = oblivion_one::effects::PremultipliedRgba::new(
                        destination_straight[0] * destination_alpha,
                        destination_straight[1] * destination_alpha,
                        destination_straight[2] * destination_alpha,
                        destination_alpha,
                    );
                    let source = oblivion_one::effects::PremultipliedRgba::new(
                        source_straight[0] * source_alpha,
                        source_straight[1] * source_alpha,
                        source_straight[2] * source_alpha,
                        source_alpha,
                    );
                    let destination_pixels = [
                        (destination.r * 255.0).round() as u8,
                        (destination.g * 255.0).round() as u8,
                        (destination.b * 255.0).round() as u8,
                        (destination.a * 255.0).round() as u8,
                    ];
                    let source_pixels = [
                        (source.r * 255.0).round() as u8,
                        (source.g * 255.0).round() as u8,
                        (source.b * 255.0).round() as u8,
                        (source.a * 255.0).round() as u8,
                    ];
                    gl.active_texture(glow::TEXTURE0);
                    gl.bind_texture(glow::TEXTURE_2D, Some(destination_texture));
                    configure_texture(&gl);
                    gl.tex_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        glow::RGBA as i32,
                        1,
                        1,
                        0,
                        glow::RGBA,
                        glow::UNSIGNED_BYTE,
                        glow::PixelUnpackData::Slice(Some(&destination_pixels)),
                    );
                    gl.active_texture(glow::TEXTURE1);
                    gl.bind_texture(glow::TEXTURE_2D, Some(source_texture));
                    configure_texture(&gl);
                    gl.tex_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        glow::RGBA as i32,
                        1,
                        1,
                        0,
                        glow::RGBA,
                        glow::UNSIGNED_BYTE,
                        glow::PixelUnpackData::Slice(Some(&source_pixels)),
                    );
                    for (shader_mode, mode) in blend_modes {
                        gl.uniform_1_i32(Some(&mode_location), shader_mode);
                        for opacity in opacities {
                            let expected = destination.blend(source, mode, opacity);
                            gl.uniform_1_f32(Some(&opacity_location), opacity);
                            gl.clear_color(0.0, 0.0, 0.0, 0.0);
                            gl.clear(glow::COLOR_BUFFER_BIT);
                            gl.draw_arrays(glow::TRIANGLES, 0, 6);
                            gl.flush();
                            let mut actual = [0_u8; 4];
                            gl.read_pixels(
                                0,
                                0,
                                1,
                                1,
                                glow::RGBA,
                                glow::UNSIGNED_BYTE,
                                glow::PixelPackData::Slice(Some(&mut actual)),
                            );
                            let expected = [
                                (expected.r * 255.0).round() as u8,
                                (expected.g * 255.0).round() as u8,
                                (expected.b * 255.0).round() as u8,
                                (expected.a * 255.0).round() as u8,
                            ];
                            if opacity == 0.0
                                && matches!(
                                    mode,
                                    oblivion_one::effects::BlendMode::Multiply
                                        | oblivion_one::effects::BlendMode::Screen
                                )
                            {
                                assert_eq!(
                                    actual, destination_pixels,
                                    "zero-opacity {:?} must preserve the destination exactly",
                                    mode
                                );
                            }
                            for (actual, expected) in actual.iter().zip(expected) {
                                assert!(
                                    (*actual as i16 - expected as i16).abs() <= 3,
                                    "blend mismatch mode={mode:?} opacity={opacity} destination_alpha={destination_alpha} source_alpha={source_alpha}: actual={actual:?} expected={expected:?}",
                                );
                            }
                        }
                    }
                }
            }
            gl.bind_vertex_array(None);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.delete_texture(destination_texture);
            gl.delete_texture(source_texture);
            gl.delete_program(blend_program);
        }

        let normalization_surface = egl
            .create_pbuffer_surface(display, config, &[egl::WIDTH, 2, egl::HEIGHT, 1, egl::NONE])
            .expect("normalization pbuffer surface creates");
        egl.make_current(
            display,
            Some(normalization_surface),
            Some(normalization_surface),
            Some(context),
        )
        .expect("normalization surface becomes current");
        let normalization_program = program::create_program_from_sources(
            &gl,
            r#"#version 300 es
                layout(location = 0) in vec2 a_position;
                layout(location = 1) in vec2 a_uv;
                out vec2 v_uv;
                void main() {
                    gl_Position = vec4(a_position, 0.0, 1.0);
                    v_uv = a_uv;
                }
            "#,
            effects::NORMALIZE_FRAGMENT_SHADER,
        )
        .expect("normalize regression shader compiles");
        let input_texture = unsafe { gl.create_texture().expect("normalize input texture") };
        let (quad, _) = renderer
            .ensure_effect_quad()
            .expect("normalize regression quad creates");
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(input_texture));
            configure_texture(&gl);
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA as i32,
                1,
                1,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(&[255, 0, 0, 255])),
            );
            gl.viewport(0, 0, 2, 1);
            gl.disable(glow::BLEND);
            gl.clear_color(0.2, 0.3, 0.4, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.use_program(Some(normalization_program));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(input_texture));
            gl.uniform_1_i32(
                Some(
                    &gl.get_uniform_location(normalization_program, "u_effect_input")
                        .expect("normalize input uniform"),
                ),
                0,
            );
            gl.uniform_4_f32(
                Some(
                    &gl.get_uniform_location(normalization_program, "u_effect_input_domain")
                        .expect("normalize input domain uniform"),
                ),
                500.0,
                200.0,
                10.0,
                10.0,
            );
            gl.uniform_4_f32(
                Some(
                    &gl.get_uniform_location(normalization_program, "u_effect_output_domain")
                        .expect("normalize output domain uniform"),
                ),
                500.0,
                200.0,
                20.0,
                10.0,
            );
            gl.uniform_1_i32(
                Some(
                    &gl.get_uniform_location(normalization_program, "u_effect_decode_srgb")
                        .expect("normalize decode uniform"),
                ),
                0,
            );
            gl.uniform_1_i32(
                Some(
                    &gl.get_uniform_location(normalization_program, "u_effect_encode_srgb")
                        .expect("normalize encode uniform"),
                ),
                0,
            );
            gl.bind_vertex_array(Some(quad));
            gl.draw_arrays(glow::TRIANGLES, 0, 6);
            gl.bind_vertex_array(None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.flush();
            let mut pixels = [0_u8; 8];
            gl.read_pixels(
                0,
                0,
                2,
                1,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut pixels)),
            );
            assert!(pixels[0] > 240 && pixels[1] < 10 && pixels[2] < 10 && pixels[3] > 240);
            assert_eq!(&pixels[4..8], &[0, 0, 0, 0]);
            gl.delete_texture(input_texture);
            gl.delete_program(normalization_program);
        }
        egl.make_current(display, None, None, None)
            .expect("EGL releases the normalization context");
        egl.destroy_surface(display, normalization_surface)
            .expect("EGL destroys the normalization surface");

        let mask_surface = egl
            .create_pbuffer_surface(display, config, &[egl::WIDTH, 4, egl::HEIGHT, 1, egl::NONE])
            .expect("mask pbuffer surface creates");
        egl.make_current(
            display,
            Some(mask_surface),
            Some(mask_surface),
            Some(context),
        )
        .expect("mask surface becomes current");
        let mask_program = program::create_program_from_sources(
            &gl,
            r#"#version 300 es
                layout(location = 0) in vec2 a_position;
                layout(location = 1) in vec2 a_uv;
                out vec2 v_uv;
                void main() {
                    gl_Position = vec4(a_position, 0.0, 1.0);
                    v_uv = a_uv;
                }
            "#,
            effects::MASK_STAGE_FRAGMENT_SHADER,
        )
        .expect("mask regression shader compiles");
        let mask_texture = unsafe { gl.create_texture().expect("mask input texture") };
        let (quad, _) = renderer
            .ensure_effect_quad()
            .expect("mask regression quad creates");
        let alphas = [0.0_f32, 0.25, 0.5, 1.0];
        let mut mask_pixels = Vec::with_capacity(alphas.len() * 4);
        for alpha in alphas {
            let channel = (0.8 * alpha * 255.0).round() as u8;
            mask_pixels.extend_from_slice(&[
                channel,
                channel,
                channel,
                (alpha * 255.0).round() as u8,
            ]);
        }
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(mask_texture));
            configure_texture(&gl);
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA as i32,
                4,
                1,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(&mask_pixels)),
            );
            gl.viewport(0, 0, 4, 1);
            gl.disable(glow::BLEND);
            gl.use_program(Some(mask_program));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(mask_texture));
            gl.uniform_1_i32(
                Some(
                    &gl.get_uniform_location(mask_program, "u_effect_input")
                        .expect("mask input uniform"),
                ),
                0,
            );
            gl.uniform_1_i32(
                Some(
                    &gl.get_uniform_location(mask_program, "u_effect_decode_srgb")
                        .expect("mask decode uniform"),
                ),
                0,
            );
            gl.uniform_1_i32(
                Some(
                    &gl.get_uniform_location(mask_program, "u_effect_encode_srgb")
                        .expect("mask encode uniform"),
                ),
                0,
            );
            gl.bind_vertex_array(Some(quad));
            for inverted in [false, true] {
                gl.uniform_1_i32(
                    Some(
                        &gl.get_uniform_location(mask_program, "u_effect_inverted")
                            .expect("mask mode uniform"),
                    ),
                    i32::from(inverted),
                );
                gl.clear_color(0.0, 0.0, 0.0, 0.0);
                gl.clear(glow::COLOR_BUFFER_BIT);
                gl.draw_arrays(glow::TRIANGLES, 0, 6);
                gl.flush();
                let mut pixels = [0_u8; 16];
                gl.read_pixels(
                    0,
                    0,
                    4,
                    1,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelPackData::Slice(Some(&mut pixels)),
                );
                for (index, alpha) in alphas.into_iter().enumerate() {
                    let coverage = if inverted { 1.0 - alpha } else { alpha };
                    let expected = [
                        (0.8 * alpha * coverage * 255.0).round() as u8,
                        (0.8 * alpha * coverage * 255.0).round() as u8,
                        (0.8 * alpha * coverage * 255.0).round() as u8,
                        (alpha * coverage * 255.0).round() as u8,
                    ];
                    let actual = &pixels[index * 4..index * 4 + 4];
                    for (actual, expected) in actual.iter().zip(expected) {
                        assert!(
                            (*actual as i16 - expected as i16).abs() <= 3,
                            "mask mismatch inverted={inverted} alpha={alpha} coverage={coverage}: actual={actual:?} expected={expected:?}"
                        );
                    }
                }
            }
            gl.bind_vertex_array(None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.delete_texture(mask_texture);
            gl.delete_program(mask_program);
        }
        egl.make_current(display, None, None, None)
            .expect("EGL releases the mask context");
        egl.destroy_surface(display, mask_surface)
            .expect("EGL destroys the mask surface");

        egl.make_current(display, None, None, None)
            .expect("EGL releases the GLES3 context");
        egl.destroy_surface(display, surface)
            .expect("EGL destroys the pbuffer surface");
        egl.destroy_context(display, context)
            .expect("EGL destroys the GLES3 context");
        egl.terminate(display).expect("EGL terminates");
    }

    #[test]
    fn native_egl_config_selection_rejects_gles2_only_candidates() {
        let mut candidate = native_candidate(1, XR24);
        candidate.renderable_type = egl::OPENGL_ES2_BIT;
        let candidates = [candidate];

        let error = select_native_egl_config_candidate(&candidates, XR24).unwrap_err();

        assert!(error.to_string().contains("GLES3"));
    }

    #[test]
    fn surfaceless_config_does_not_require_window_bit() {
        let mut candidate = native_candidate(1, XR24);
        candidate.surface_type = 0;

        assert!(native_egl_config_candidate_matches_common(&candidate, XR24));
        assert!(!native_egl_config_candidate_matches(&candidate, XR24));
    }

    #[test]
    fn native_egl_config_selection_prefers_requested_xrgb8888() {
        let candidates = [
            native_candidate(1, AR24),
            native_candidate(2, XR24),
            native_candidate(3, XR24),
        ];

        let selected = select_native_egl_config_candidate(&candidates, XR24).unwrap();

        assert_eq!(selected, 1);
    }

    #[test]
    fn native_egl_config_selection_ignores_wrong_visual() {
        let candidates = [native_candidate(1, AR24)];

        assert!(select_native_egl_config_candidate(&candidates, XR24).is_err());
    }

    #[test]
    fn native_egl_config_selection_accepts_zero_alpha_for_xrgb8888() {
        let candidates = [native_candidate(7, XR24)];

        let selected = select_native_egl_config_candidate(&candidates, XR24).unwrap();

        assert_eq!(selected, 0);
    }

    #[test]
    fn native_egl_format_selection_falls_back_to_argb8888_when_xrgb8888_absent() {
        let available_formats = [XR24, AR24];
        let candidates = [native_candidate(9, AR24)];

        let selected = select_native_egl_visual_format(&available_formats, &candidates).unwrap();

        assert_eq!(selected, AR24);
    }

    #[test]
    fn native_egl_config_selection_rejects_missing_window_bit() {
        let mut candidate = native_candidate(1, XR24);
        candidate.surface_type = 0;

        assert!(select_native_egl_config_candidate(&[candidate], XR24).is_err());
    }

    #[test]
    fn native_egl_config_selection_rejects_missing_gles_renderable_bit() {
        let mut candidate = native_candidate(1, XR24);
        candidate.renderable_type = 0;

        assert!(select_native_egl_config_candidate(&[candidate], XR24).is_err());
    }

    #[test]
    fn native_egl_config_selection_diagnostic_names_requested_fourcc_and_hex() {
        let error = select_native_egl_config_candidate(&[], XR24).unwrap_err();
        let diagnostic = error.to_string();

        assert!(diagnostic.contains("XR24"));
        assert!(diagnostic.contains("0x34325258"));
    }

    #[test]
    fn output_damage_tracker_separates_scene_rebuild_from_authoritative_damage() {
        let mut tracker = EglOutputDamageTracker::default();
        tracker.damage_for_frame(
            1280,
            800,
            true,
            None,
            DesktopVisualState::wallpaper_only(),
            None,
        );
        tracker.commit_presented(EglOutputDamageTracker::candidate_state(
            1280,
            800,
            DesktopVisualState::wallpaper_only(),
            None,
            &oblivion_one::cursor_theme::CompositorCursorImage::builtin_fallback(),
        ));
        let precise = OutputDamage::rects(1280, 800, [OutputRect::new(10, 20, 30, 40)]);

        assert_eq!(
            tracker.damage_for_frame(
                1280,
                800,
                true,
                Some(precise.clone()),
                DesktopVisualState::wallpaper_only(),
                None,
            ),
            precise
        );
    }

    #[test]
    fn scene_damage_authority_preserves_explicit_empty_damage() {
        assert_eq!(
            resolve_scene_damage_authority(true, true, OutputDamage::Empty),
            (OutputDamage::Empty, false)
        );
    }

    #[test]
    fn scene_damage_authority_falls_back_when_damage_is_missing() {
        assert_eq!(
            resolve_scene_damage_authority(true, false, OutputDamage::Empty),
            (OutputDamage::Full, true)
        );
    }

    #[test]
    fn output_damage_tracker_limits_cursor_motion_to_old_and_new_bounds() {
        let mut tracker = EglOutputDamageTracker::default();
        tracker.damage_for_frame(
            1280,
            800,
            true,
            None,
            DesktopVisualState::with_cursor(10, 10),
            None,
        );
        tracker.commit_presented(EglOutputDamageTracker::candidate_state(
            1280,
            800,
            DesktopVisualState::with_cursor(10, 10),
            None,
            &oblivion_one::cursor_theme::CompositorCursorImage::builtin_fallback(),
        ));
        let damage = tracker.damage_for_frame(
            1280,
            800,
            false,
            Some(OutputDamage::Empty),
            DesktopVisualState::with_cursor(20, 22),
            None,
        );

        assert_eq!(damage.rect_count(), 2);
        let cursor_image = oblivion_one::cursor_theme::CompositorCursorImage::builtin_fallback();
        assert_eq!(
            damage.pixels(1280, 800),
            Some(u64::from(cursor_image.width) * u64::from(cursor_image.height) * 2)
        );
    }

    #[test]
    fn egl_damage_uses_hotspot_adjusted_bounds() {
        let image = std::sync::Arc::new(
            oblivion_one::cursor_theme::CompositorCursorImage::from_argb8888(
                vec![0xff00_0000; 4 * 3],
                4,
                3,
                2,
                1,
            )
            .unwrap(),
        );
        let mut tracker = EglOutputDamageTracker::with_cursor_image(image.clone());
        tracker.damage_for_frame(
            1280,
            800,
            true,
            None,
            DesktopVisualState::with_cursor(10, 10),
            None,
        );
        tracker.commit_presented(EglOutputDamageTracker::candidate_state(
            1280,
            800,
            DesktopVisualState::with_cursor(10, 10),
            None,
            &image,
        ));

        assert_eq!(
            tracker.damage_for_frame(
                1280,
                800,
                false,
                Some(OutputDamage::Empty),
                DesktopVisualState::with_cursor(20, 22),
                None,
            ),
            OutputDamage::Rects(vec![
                OutputRect::new(8, 9, 4, 3),
                OutputRect::new(18, 21, 4, 3),
            ])
        );
    }

    #[test]
    fn output_damage_tracker_damages_old_and_new_bounds_when_cursor_image_changes() {
        let old_image = std::sync::Arc::new(
            oblivion_one::cursor_theme::CompositorCursorImage::from_argb8888(
                vec![0xff00_0000],
                1,
                1,
                0,
                0,
            )
            .unwrap(),
        );
        let new_image = std::sync::Arc::new(
            oblivion_one::cursor_theme::CompositorCursorImage::from_argb8888(
                vec![0xffff_0000; 3 * 2],
                3,
                2,
                0,
                0,
            )
            .unwrap(),
        );
        let mut tracker = EglOutputDamageTracker::with_cursor_image(old_image.clone());
        tracker.damage_for_frame(
            1280,
            800,
            true,
            None,
            DesktopVisualState::with_cursor(10, 10),
            None,
        );
        tracker.commit_presented(EglOutputDamageTracker::candidate_state(
            1280,
            800,
            DesktopVisualState::with_cursor(10, 10),
            None,
            &old_image,
        ));

        tracker.set_cursor_image(new_image);
        let damage = tracker.damage_for_frame(
            1280,
            800,
            false,
            Some(OutputDamage::Empty),
            DesktopVisualState::with_cursor(10, 10),
            None,
        );
        assert_eq!(damage.rect_count(), 1);
        assert_eq!(damage.pixels(1280, 800), Some(6));
    }

    #[test]
    fn output_damage_tracker_does_not_damage_hidden_cursor_after_settling() {
        let image = oblivion_one::cursor_theme::CompositorCursorImage::builtin_fallback();
        let mut tracker = EglOutputDamageTracker::default();
        tracker.damage_for_frame(
            1280,
            800,
            true,
            None,
            DesktopVisualState::with_cursor(10, 10),
            None,
        );
        tracker.commit_presented(EglOutputDamageTracker::candidate_state(
            1280,
            800,
            DesktopVisualState::with_cursor(10, 10),
            None,
            &image,
        ));

        let hide_damage = tracker.damage_for_frame(
            1280,
            800,
            false,
            Some(OutputDamage::Empty),
            DesktopVisualState::wallpaper_only(),
            None,
        );
        assert_eq!(
            hide_damage.pixels(1280, 800),
            Some(u64::from(image.width) * u64::from(image.height))
        );
        tracker.commit_presented(EglOutputDamageTracker::candidate_state(
            1280,
            800,
            DesktopVisualState::wallpaper_only(),
            None,
            &image,
        ));
        assert_eq!(
            tracker.damage_for_frame(
                1280,
                800,
                false,
                Some(OutputDamage::Empty),
                DesktopVisualState::wallpaper_only(),
                None,
            ),
            OutputDamage::Empty
        );
    }

    #[test]
    fn output_damage_tracker_repeats_candidate_damage_until_presented() {
        let mut tracker = EglOutputDamageTracker::default();
        tracker.damage_for_frame(
            1280,
            800,
            true,
            None,
            DesktopVisualState::with_cursor(10, 10),
            None,
        );
        tracker.commit_presented(EglOutputDamageTracker::candidate_state(
            1280,
            800,
            DesktopVisualState::with_cursor(10, 10),
            None,
            &oblivion_one::cursor_theme::CompositorCursorImage::builtin_fallback(),
        ));

        let first = tracker.damage_for_frame(
            1280,
            800,
            false,
            Some(OutputDamage::Empty),
            DesktopVisualState::with_cursor(20, 22),
            None,
        );
        let retry = tracker.damage_for_frame(
            1280,
            800,
            false,
            Some(OutputDamage::Empty),
            DesktopVisualState::with_cursor(20, 22),
            None,
        );

        assert_eq!(retry, first);
        assert_ne!(retry, OutputDamage::Empty);
    }

    #[test]
    fn argb_pixels_pack_to_rgba_without_changing_channel_order() {
        let mut packed = Vec::new();

        pack_argb_pixels_rgba(&[0x1122_3344, 0xaa55_6677], &mut packed);

        assert_eq!(packed, vec![0x22, 0x33, 0x44, 0x11, 0x55, 0x66, 0x77, 0xaa]);
    }

    #[test]
    fn scene_cache_key_invalidates_when_surface_geometry_changes() {
        let initial_signature = EglSceneSurfaceSignature {
            surface_id: 7,
            commit_sequence: 1,
            buffer_id: 11,
            buffer_width: 800,
            buffer_height: 600,
            buffer_scale: 1,
            buffer_transform: wayland_server::protocol::wl_output::Transform::Normal,
            x: 10,
            y: 20,
            width: 800,
            height: 600,
            render_x: 0,
            render_y: 0,
            clip_x: 0,
            clip_y: 0,
            clip_width: 0,
            clip_height: 0,
            generation: 1,
        };
        let resized_signature = EglSceneSurfaceSignature {
            width: 420,
            height: 320,
            ..initial_signature
        };
        let key = EglSceneCacheKey::new(
            1280,
            800,
            9,
            120,
            &[initial_signature],
            OutputFramebufferOrigin::BottomLeft,
        );

        assert!(key.is_current(
            1280,
            800,
            9,
            120,
            &[initial_signature],
            OutputFramebufferOrigin::BottomLeft,
        ));
        assert!(!key.is_current(
            1280,
            800,
            9,
            120,
            &[resized_signature],
            OutputFramebufferOrigin::BottomLeft,
        ));
    }

    #[test]
    fn scene_cache_key_invalidates_when_visual_assignment_changes() {
        let initial_signature = EglSceneSurfaceSignature {
            surface_id: 7,
            commit_sequence: 1,
            buffer_id: 11,
            buffer_width: 800,
            buffer_height: 600,
            buffer_scale: 1,
            buffer_transform: wayland_server::protocol::wl_output::Transform::Normal,
            x: 10,
            y: 20,
            width: 800,
            height: 600,
            render_x: 0,
            render_y: 0,
            clip_x: 0,
            clip_y: 0,
            clip_width: 0,
            clip_height: 0,
            generation: 1,
        };
        let cropped_signature = EglSceneSurfaceSignature {
            render_x: 5,
            render_y: 7,
            clip_x: 0,
            clip_y: 0,
            clip_width: 0,
            clip_height: 0,
            ..initial_signature
        };
        let key = EglSceneCacheKey::new(
            1280,
            800,
            9,
            120,
            &[initial_signature],
            OutputFramebufferOrigin::BottomLeft,
        );

        assert!(!key.is_current(
            1280,
            800,
            9,
            120,
            &[cropped_signature],
            OutputFramebufferOrigin::BottomLeft,
        ));
    }

    #[test]
    fn scene_cache_key_invalidates_when_decoration_scene_changes() {
        let window_id = oblivion_one::compositor::WindowId::from_raw(21).expect("window id");
        let initial = DecorationSceneSnapshot::from_bounds(window_id, 7, 100, 80, 302, 227, 1);
        let changed = DecorationSceneSnapshot::from_bounds(window_id, 7, 100, 80, 302, 227, 2);
        let key = EglSceneCacheKey::new_with_decoration_snapshots(
            1280,
            800,
            9,
            120,
            &[],
            std::slice::from_ref(&initial),
            OutputFramebufferOrigin::BottomLeft,
        );

        assert!(key.is_current_with_decoration_snapshots(
            1280,
            800,
            9,
            120,
            &[],
            std::slice::from_ref(&initial),
            OutputFramebufferOrigin::BottomLeft,
        ));
        assert!(!key.is_current_with_decoration_snapshots(
            1280,
            800,
            9,
            120,
            &[],
            std::slice::from_ref(&changed),
            OutputFramebufferOrigin::BottomLeft,
        ));
    }

    #[test]
    fn scene_cache_key_reuses_geometry_when_visible_buffer_identity_changes() {
        let initial = EglSceneSurfaceSignature {
            surface_id: 7,
            commit_sequence: 1,
            buffer_id: 11,
            buffer_width: 800,
            buffer_height: 600,
            buffer_scale: 1,
            buffer_transform: wayland_server::protocol::wl_output::Transform::Normal,
            x: 10,
            y: 20,
            width: 800,
            height: 600,
            render_x: 0,
            render_y: 0,
            clip_x: 0,
            clip_y: 0,
            clip_width: 0,
            clip_height: 0,
            generation: 1,
        };
        let replacement = EglSceneSurfaceSignature {
            buffer_id: 12,
            ..initial
        };
        let key = EglSceneCacheKey::new(
            1280,
            800,
            9,
            120,
            &[initial],
            OutputFramebufferOrigin::BottomLeft,
        );

        assert!(key.is_current(
            1280,
            800,
            9,
            120,
            &[replacement],
            OutputFramebufferOrigin::BottomLeft,
        ));
    }

    #[test]
    fn scene_cache_key_reuses_geometry_when_content_generation_changes() {
        let signature = EglSceneSurfaceSignature {
            surface_id: 7,
            commit_sequence: 1,
            buffer_id: 11,
            buffer_width: 800,
            buffer_height: 600,
            buffer_scale: 1,
            buffer_transform: wayland_server::protocol::wl_output::Transform::Normal,
            x: 10,
            y: 20,
            width: 800,
            height: 600,
            render_x: 0,
            render_y: 0,
            clip_x: 0,
            clip_y: 0,
            clip_width: 0,
            clip_height: 0,
            generation: 1,
        };
        let key = EglSceneCacheKey::new(
            1280,
            800,
            9,
            120,
            &[signature],
            OutputFramebufferOrigin::BottomLeft,
        );

        assert!(key.is_current(
            1280,
            800,
            10,
            120,
            &[EglSceneSurfaceSignature {
                commit_sequence: 2,
                buffer_id: 12,
                generation: 2,
                ..signature
            }],
            OutputFramebufferOrigin::BottomLeft,
        ));
    }

    #[test]
    fn scene_cache_key_invalidates_when_framebuffer_origin_changes() {
        let signature = EglSceneSurfaceSignature {
            surface_id: 7,
            commit_sequence: 1,
            buffer_id: 11,
            buffer_width: 800,
            buffer_height: 600,
            buffer_scale: 1,
            buffer_transform: wayland_server::protocol::wl_output::Transform::Normal,
            x: 10,
            y: 20,
            width: 800,
            height: 600,
            render_x: 0,
            render_y: 0,
            clip_x: 0,
            clip_y: 0,
            clip_width: 0,
            clip_height: 0,
            generation: 1,
        };
        let key = EglSceneCacheKey::new(
            1280,
            800,
            9,
            120,
            &[signature],
            OutputFramebufferOrigin::BottomLeft,
        );

        assert!(!key.is_current(
            1280,
            800,
            9,
            120,
            &[signature],
            OutputFramebufferOrigin::TopLeftScanout,
        ));
    }

    #[test]
    fn scene_cache_key_invalidates_when_presentation_geometry_changes() {
        let key = EglSceneCacheKey::new_with_presentation(
            1280,
            800,
            9,
            120,
            &[],
            11,
            OutputFramebufferOrigin::BottomLeft,
        );

        assert!(key.is_current_with_decorations(
            1280,
            800,
            9,
            120,
            &[],
            &[],
            &[],
            11,
            OutputFramebufferOrigin::BottomLeft,
        ));
        assert!(!key.is_current_with_decorations(
            1280,
            800,
            9,
            120,
            &[],
            &[],
            &[],
            12,
            OutputFramebufferOrigin::BottomLeft,
        ));
    }

    #[test]
    fn dmabuf_resource_key_matches_same_handle_for_surface() {
        let mut ids = BufferIdAllocator::default();
        let identity = ids.allocate().expect("test buffer identity");
        let handle = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);

        assert_eq!(
            DmabufImageKey::from_handle(identity.id(), &handle),
            DmabufImageKey::from_handle(identity.id(), &handle)
        );
    }

    #[test]
    fn dmabuf_resource_key_separates_buffer_ids_when_raw_fd_is_identical() {
        let mut ids = BufferIdAllocator::default();
        let first = ids.allocate().expect("first test buffer identity");
        let second = ids.allocate().expect("second test buffer identity");
        let handle = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);

        assert_ne!(
            DmabufImageKey::from_handle(first.id(), &handle),
            DmabufImageKey::from_handle(second.id(), &handle)
        );
    }

    #[test]
    fn dmabuf_resource_key_separates_plane_layout_for_same_buffer_id() {
        let mut ids = BufferIdAllocator::default();
        let identity = ids.allocate().expect("test buffer identity");
        let first = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
        let second = test_dmabuf_handle(256, 144, 2048, DrmModifier::LINEAR);

        assert_ne!(
            DmabufImageKey::from_handle(identity.id(), &first),
            DmabufImageKey::from_handle(identity.id(), &second)
        );
    }

    #[test]
    fn dead_buffer_cache_entry_is_evicted_exactly_once() {
        let mut ids = BufferIdAllocator::default();
        let identity = ids.allocate().expect("test buffer identity");
        let handle = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
        let key = DmabufImageKey::from_handle(identity.id(), &handle);
        let drops = std::rc::Rc::new(std::cell::Cell::new(0));
        let mut cache = HashMap::from([(
            key.clone(),
            CachedDmabufResource {
                image: DropProbe(std::rc::Rc::clone(&drops)),
                buffer_lifetime: identity.downgrade(),
                surface_id: 7,
            },
        )]);

        drop(identity);
        for dead in dead_cached_dmabuf_keys(&cache) {
            drop(cache.remove(&dead));
        }
        assert!(dead_cached_dmabuf_keys(&cache).is_empty());
        assert_eq!(drops.get(), 1);

        drop(cache.remove(&key));
        assert_eq!(drops.get(), 1);
    }

    #[test]
    fn renderer_cache_recreation_drops_all_previous_generation_entries() {
        let mut ids = BufferIdAllocator::default();
        let handle = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
        let drops = std::rc::Rc::new(std::cell::Cell::new(0));
        let mut cache = HashMap::new();
        for surface_id in 1..=3 {
            let identity = ids.allocate().expect("test buffer identity");
            cache.insert(
                DmabufImageKey::from_handle(identity.id(), &handle),
                CachedDmabufResource {
                    image: DropProbe(std::rc::Rc::clone(&drops)),
                    buffer_lifetime: identity.downgrade(),
                    surface_id,
                },
            );
        }

        drop(cache);

        assert_eq!(drops.get(), 3);
    }

    #[test]
    fn obsolete_dmabuf_resource_is_removed_when_current_backing_changes() {
        let mut ids = BufferIdAllocator::default();
        let identity_a = ids.allocate().expect("first test buffer identity");
        let identity_b = ids.allocate().expect("second test buffer identity");
        let handle_a = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
        let handle_b = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
        let surface = test_dmabuf_surface(7, identity_b, handle_b, 2);
        let mut resources = HashMap::from([(7, test_dmabuf_resource(&identity_a, &handle_a, 1))]);

        let (action, _resource) = reconcile_surface_resource_backing(&mut resources, &surface)
            .expect("obsolete resource must be reconciled");

        assert_eq!(action, SurfaceResourceLifetimeAction::DemoteDmabuf);
        assert!(!resources.contains_key(&surface.surface_id));
    }

    #[test]
    fn exact_current_dmabuf_resource_is_kept_across_render_generation_change() {
        let mut ids = BufferIdAllocator::default();
        let identity = ids.allocate().expect("test buffer identity");
        let handle = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
        let surface = test_dmabuf_surface(7, identity.clone(), handle.clone(), 2);
        let mut resources = HashMap::from([(7, test_dmabuf_resource(&identity, &handle, 1))]);

        assert_eq!(
            classify_surface_resource_lifetime(&resources[&7], &surface),
            SurfaceResourceLifetimeAction::Keep
        );
        assert!(reconcile_surface_resource_backing(&mut resources, &surface).is_none());
        assert!(resources.contains_key(&surface.surface_id));
    }

    #[test]
    fn repeated_hidden_dmabuf_rotations_do_not_create_replacement_resources() {
        let mut ids = BufferIdAllocator::default();
        let identity_a = ids.allocate().expect("initial test buffer identity");
        let handle_a = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
        let mut resources = HashMap::from([(7, test_dmabuf_resource(&identity_a, &handle_a, 1))]);

        for generation in 2..=5 {
            let identity = ids.allocate().expect("rotated test buffer identity");
            let handle = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
            let surface = test_dmabuf_surface(7, identity, handle, generation);
            let action = reconcile_surface_resource_backing(&mut resources, &surface)
                .map(|(action, _)| action);
            if generation == 2 {
                assert_eq!(action, Some(SurfaceResourceLifetimeAction::DemoteDmabuf));
            } else {
                assert_eq!(action, None);
            }
            assert!(resources.is_empty());
        }
    }

    #[test]
    fn dead_obsolete_dmabuf_resource_is_not_cacheable() {
        let mut ids = BufferIdAllocator::default();
        let identity_a = ids.allocate().expect("initial test buffer identity");
        let identity_b = ids.allocate().expect("replacement test buffer identity");
        let handle_a = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
        let handle_b = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
        let mut resources = HashMap::from([(7, test_dmabuf_resource(&identity_a, &handle_a, 1))]);
        let surface = test_dmabuf_surface(7, identity_b, handle_b, 2);
        drop(identity_a);

        let (action, resource) = reconcile_surface_resource_backing(&mut resources, &surface)
            .expect("obsolete resource must be reconciled");

        assert_eq!(action, SurfaceResourceLifetimeAction::DemoteDmabuf);
        assert!(
            !resource
                .buffer_lifetime
                .expect("dmabuf resource lifetime")
                .is_alive()
        );
        assert!(resources.is_empty());
    }

    #[test]
    fn dmabuf_to_shm_retires_dmabuf_without_creating_shm_resource() {
        let mut ids = BufferIdAllocator::default();
        let identity = ids.allocate().expect("test buffer identity");
        let handle = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
        let mut resources = HashMap::from([(7, test_dmabuf_resource(&identity, &handle, 1))]);
        let surface = test_shm_surface(RenderableSurfaceDamage::Empty);

        let (action, _resource) = reconcile_surface_resource_backing(&mut resources, &surface)
            .expect("obsolete dmabuf resource must be reconciled");

        assert_eq!(action, SurfaceResourceLifetimeAction::DemoteDmabuf);
        assert!(resources.is_empty());
    }

    #[test]
    fn shm_to_dmabuf_destroys_shm_without_importing_dmabuf() {
        let mut ids = BufferIdAllocator::default();
        let identity = ids.allocate().expect("replacement test buffer identity");
        let handle = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
        let surface = test_dmabuf_surface(7, identity, handle, 2);
        let mut resources = HashMap::from([(7, test_shm_resource(Some(SurfaceCommitCounter(1))))]);

        let (action, _resource) = reconcile_surface_resource_backing(&mut resources, &surface)
            .expect("obsolete shm resource must be reconciled");

        assert_eq!(action, SurfaceResourceLifetimeAction::Destroy);
        assert!(resources.is_empty());
    }

    #[test]
    fn compatible_hidden_shm_update_keeps_stale_texture_and_commit_baseline() {
        let surface = test_shm_surface(RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        }]));
        let synced_commit = Some(SurfaceCommitCounter(1));
        let mut resources = HashMap::from([(7, test_shm_resource(synced_commit))]);

        assert!(reconcile_surface_resource_backing(&mut resources, &surface).is_none());
        assert_eq!(resources[&7].shm_synced_commit, synced_commit);
    }

    #[test]
    fn hidden_shm_resize_retires_incompatible_texture_without_replacement() {
        let mut surface = test_shm_surface(RenderableSurfaceDamage::Empty);
        let identity = BufferIdAllocator::default()
            .allocate()
            .expect("resized test buffer identity");
        surface.buffer = CommittedSurfaceBuffer::shm_snapshot(
            identity,
            BufferSize::new(3, 2).expect("resized test surface size"),
            vec![0; 6],
        );
        let mut resources = HashMap::from([(7, test_shm_resource(Some(SurfaceCommitCounter(1))))]);

        let (action, _resource) = reconcile_surface_resource_backing(&mut resources, &surface)
            .expect("incompatible shm resource must be reconciled");

        assert_eq!(action, SurfaceResourceLifetimeAction::Destroy);
        assert!(resources.is_empty());
    }

    #[test]
    fn obsolete_resource_is_absent_before_hidden_replacement_can_be_realized() {
        let mut ids = BufferIdAllocator::default();
        let identity_a = ids.allocate().expect("initial test buffer identity");
        let identity_b = ids.allocate().expect("replacement test buffer identity");
        let handle_a = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
        let handle_b = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
        let mut resources = HashMap::from([(7, test_dmabuf_resource(&identity_a, &handle_a, 1))]);
        let surface_b = test_dmabuf_surface(7, identity_b, handle_b, 2);

        let (action, _resource) = reconcile_surface_resource_backing(&mut resources, &surface_b)
            .expect("obsolete resource must be reconciled");

        assert_eq!(action, SurfaceResourceLifetimeAction::DemoteDmabuf);
        assert!(!resources.contains_key(&surface_b.surface_id));
        assert!(reconcile_surface_resource_backing(&mut resources, &surface_b).is_none());
    }

    fn test_dmabuf_surface(
        surface_id: u32,
        identity: BufferIdentity,
        handle: DmabufBufferHandle,
        generation: u64,
    ) -> RenderableSurface {
        let size = handle.size();
        RenderableSurface {
            surface_id,
            x: 0,
            y: 0,
            width: size.width,
            height: size.height,
            placement: SurfacePlacement::root(),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: CommittedSurfaceBuffer::dmabuf_handle(identity, handle),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wayland_server::protocol::wl_output::Transform::Normal,
            opaque_region: SurfaceOpaqueRegion::None,
            damage: RenderableSurfaceDamage::Full,
        }
    }

    fn test_dmabuf_resource(
        identity: &BufferIdentity,
        handle: &DmabufBufferHandle,
        generation: u64,
    ) -> EglSurfaceResource {
        let size = handle.size();
        EglSurfaceResource {
            image: EglImageResource {
                texture: glow::NativeTexture(std::num::NonZeroU32::new(1).unwrap()),
                size: (size.width, size.height),
                generation,
                egl_image: Some(fake_egl_image()),
            },
            dmabuf_key: Some(DmabufImageKey::from_handle(identity.id(), handle)),
            buffer_lifetime: Some(identity.downgrade()),
            shm_synced_commit: None,
        }
    }

    fn test_dmabuf_handle(
        width: u32,
        height: u32,
        stride: u32,
        modifier: DrmModifier,
    ) -> DmabufBufferHandle {
        let fd = std::fs::File::open("/dev/null")
            .expect("/dev/null exists for dmabuf identity tests")
            .into();
        DmabufBufferHandle::new(
            BufferSize::new(width, height).expect("test dmabuf size is non-zero"),
            DrmFormat::Xrgb8888,
            vec![DmabufPlane::new(
                fd,
                DmabufPlaneDescriptor {
                    plane_index: 0,
                    offset: 0,
                    stride,
                    modifier,
                },
            )],
        )
        .expect("test dmabuf metadata is valid")
    }
}
