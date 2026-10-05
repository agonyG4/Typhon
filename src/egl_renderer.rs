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
pub(crate) mod native_fence;
mod program;
mod scene_state;

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
use program::create_texture_program;
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

struct LifecycleResolvedVisualResource {
    texture: PooledEffectTexture,
    source_signature: u64,
    source_visual_rect: compositor::PresentationRect,
}

#[derive(Clone, Copy, Debug)]
enum LifecycleSourceCaptureFailureStage {
    TargetAllocation,
    TargetClear,
    TargetRelease,
    ScratchAllocation,
    ScratchClear,
    ScratchTarget,
    MissingSourceCommands,
    GraphCompile,
    EffectExecution,
    TargetCopy,
    ScratchRelease,
}

impl LifecycleSourceCaptureFailureStage {
    const fn as_str(self) -> &'static str {
        match self {
            Self::TargetAllocation => "target_allocation",
            Self::TargetClear => "target_clear",
            Self::TargetRelease => "target_release",
            Self::ScratchAllocation => "scratch_allocation",
            Self::ScratchClear => "scratch_clear",
            Self::ScratchTarget => "scratch_target",
            Self::MissingSourceCommands => "missing_source_commands",
            Self::GraphCompile => "graph_compile",
            Self::EffectExecution => "effect_execution",
            Self::TargetCopy => "target_copy",
            Self::ScratchRelease => "scratch_release",
        }
    }
}

#[derive(Debug)]
struct LifecycleSourceCaptureFailure {
    stage: LifecycleSourceCaptureFailureStage,
    source: Box<dyn Error>,
}

impl LifecycleSourceCaptureFailure {
    fn new(stage: LifecycleSourceCaptureFailureStage, source: impl Into<Box<dyn Error>>) -> Self {
        Self {
            stage,
            source: source.into(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct LampUniformLocations {
    output_size: Option<glow::UniformLocation>,
    canonical_visual_rect: Option<glow::UniformLocation>,
    source_visual_rect: Option<glow::UniformLocation>,
    sink_rect: Option<glow::UniformLocation>,
    progress: Option<glow::UniformLocation>,
    opacity: Option<glow::UniformLocation>,
    direction: Option<glow::UniformLocation>,
    shape_factor: Option<glow::UniformLocation>,
    bump_distance: Option<glow::UniformLocation>,
    contraction_progress: Option<glow::UniformLocation>,
    translation_progress: Option<glow::UniformLocation>,
    retreat_progress: Option<glow::UniformLocation>,
    framebuffer_origin_bottom_left: Option<glow::UniformLocation>,
    texture: Option<glow::UniformLocation>,
}

pub(crate) struct GlesSceneRenderer {
    cursor_image: std::sync::Arc<CompositorCursorImage>,
    gl: glow::Context,
    effect_runtime: EffectRuntime,
    scene_state: SceneRenderState,
    lifecycle_source_vertices:
        HashMap<compositor::PresentationRetainedVisualPayloadId, Vec<EglTexturedVertex>>,
    lifecycle_source_commands:
        HashMap<compositor::PresentationRetainedVisualPayloadId, Vec<EglDrawCommand>>,
    lifecycle_visual_resources:
        HashMap<compositor::PresentationRetainedVisualPayloadId, LifecycleResolvedVisualResource>,
    lifecycle_visual_sources: HashMap<
        oblivion_one::presentation_animation::PresentationRetainedVisualIdentity,
        LifecycleVisualSource,
    >,
    lifecycle_samples: Vec<LifecycleFrameSample>,
    lifecycle_render_evidence: LifecycleRenderEvidence,
    lifecycle_render_fallbacks: LifecycleRenderFallbacks,
    texture_upload_rgba: Vec<u8>,
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
}

struct CaptureRendererState {
    scene: SceneCaptureSnapshot,
    effect_runtime: EffectRuntimeCaptureSnapshot,
    lifecycle_render_evidence: LifecycleRenderEvidence,
    lifecycle_render_fallbacks: LifecycleRenderFallbacks,
    lifecycle_samples: Vec<LifecycleFrameSample>,
    lifecycle_visual_sources: HashMap<
        oblivion_one::presentation_animation::PresentationRetainedVisualIdentity,
        LifecycleVisualSource,
    >,
    failed_surface_generations: HashMap<u32, u64>,
}

impl CaptureRendererState {
    fn take(renderer: &GlesSceneRenderer) -> Self {
        Self {
            scene: SceneCaptureSnapshot::take(&renderer.scene_state),
            effect_runtime: EffectRuntimeCaptureSnapshot::take(&renderer.effect_runtime),
            lifecycle_render_evidence: renderer.lifecycle_render_evidence.clone(),
            lifecycle_render_fallbacks: renderer.lifecycle_render_fallbacks.clone(),
            lifecycle_samples: renderer.lifecycle_samples.clone(),
            lifecycle_visual_sources: renderer.lifecycle_visual_sources.clone(),
            failed_surface_generations: renderer.failed_surface_generations.clone(),
        }
    }

    fn restore(self, renderer: &mut GlesSceneRenderer) {
        self.scene.restore(&mut renderer.scene_state);
        self.effect_runtime.restore(&mut renderer.effect_runtime);
        renderer.lifecycle_render_evidence = self.lifecycle_render_evidence;
        renderer.lifecycle_render_fallbacks = self.lifecycle_render_fallbacks;
        renderer.lifecycle_samples = self.lifecycle_samples;
        renderer.lifecycle_visual_sources = self.lifecycle_visual_sources;
        renderer.failed_surface_generations = self.failed_surface_generations;
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
            .samples
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
    presentation_identity: oblivion_one::presentation_animation::PresentationRetainedVisualIdentity,
    bounds: EglRect,
    uv: EglUvRect,
}

#[derive(Debug, Clone, PartialEq)]
struct EglSquashDrawCommand {
    command: EglDrawCommand,
    presentation_identity: oblivion_one::presentation_animation::PresentationRetainedVisualIdentity,
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

fn append_lamp_grid(
    vertices: &mut Vec<EglLampVertex>,
    commands: &mut Vec<EglLampDrawCommand>,
    spec: LampGridSpec,
) -> bool {
    let LampGridSpec {
        layer,
        presentation_identity,
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
            uv: uv.sample(u, v),
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
        presentation_identity,
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
    for lamp in lifecycle
        .samples
        .iter()
        .filter(|sample| sample.effect == LifecycleEffectKind::Lamp)
    {
        for value in [
            lamp.window_id.get(),
            u64::from(lamp.root_surface_id),
            lamp.presentation_identity.scene_node_id().get(),
            match lamp.presentation_identity.kind() {
                oblivion_one::presentation_animation::PresentationRetainedVisualKind::WindowLifecycle => 1,
                oblivion_one::presentation_animation::PresentationRetainedVisualKind::WindowExit => 2,
            },
            lamp.presentation_identity.transaction_id().get(),
            lamp.presentation_identity.revision_id().get(),
            lamp.payload_id.get(),
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
            lamp.visual_group.portal_rect.x().to_bits(),
            lamp.visual_group.portal_rect.y().to_bits(),
            lamp.visual_group.portal_rect.width().to_bits(),
            lamp.visual_group.portal_rect.height().to_bits(),
            lamp.visual_group.sink_rect.x().to_bits(),
            lamp.visual_group.sink_rect.y().to_bits(),
            lamp.visual_group.sink_rect.width().to_bits(),
            lamp.visual_group.sink_rect.height().to_bits(),
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
        let source = &lamp.visual_source;
        mix(match source.kind {
            LifecycleVisualSourceKind::NoOwnedEffects => 1,
            LifecycleVisualSourceKind::ResolvedOwnedEffects => 2,
        });
        mix(source.effect_scene.signature);
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
        match surface.render_placement {
            Some(placement) => {
                mix(1);
                mix(u64::from(placement.parent_surface_id.unwrap_or(0)));
                mix(placement.local_x as u32 as u64);
                mix(placement.local_y as u32 as u64);
                mix(placement.root_mode as u32 as u64);
            }
            None => mix(0),
        }
        match surface.render_target_size {
            Some(size) => {
                mix(1);
                mix(u64::from(size.width));
                mix(u64::from(size.height));
            }
            None => mix(0),
        }
        match &surface.visual_clip {
            Some(aperture) => {
                mix(1);
                let logical_target = aperture.logical_target();
                mix(logical_target.x() as u32 as u64);
                mix(logical_target.y() as u32 as u64);
                mix(u64::from(logical_target.width()));
                mix(u64::from(logical_target.height()));
                if let Some(content_target) = aperture.committed_content_target() {
                    mix(1);
                    mix(content_target.x() as u32 as u64);
                    mix(content_target.y() as u32 as u64);
                    mix(u64::from(content_target.width()));
                    mix(u64::from(content_target.height()));
                } else {
                    mix(0);
                }
                mix(aperture.committed_extent_regions().len() as u64);
                for extent in aperture.committed_extent_regions() {
                    mix(extent.x() as u32 as u64);
                    mix(extent.y() as u32 as u64);
                    mix(u64::from(extent.width()));
                    mix(u64::from(extent.height()));
                }
            }
            None => mix(0),
        }
        let buffer_size = surface.buffer_size();
        mix(u64::from(buffer_size.width));
        mix(u64::from(buffer_size.height));
        mix(u64::from(surface.buffer_scale));
        mix(surface.buffer_transform as u32 as u64);
        match surface.viewport_source {
            Some(source) => {
                mix(1);
                mix(source.x.to_bits());
                mix(source.y.to_bits());
                mix(source.width.to_bits());
                mix(source.height.to_bits());
            }
            None => mix(0),
        }
        match surface.viewport_destination {
            Some(destination) => {
                mix(1);
                mix(u64::from(destination.width));
                mix(u64::from(destination.height));
            }
            None => mix(0),
        }
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
    for sample in &lifecycle.samples {
        if let Some(footprint) =
            lifecycle_visual_transition_bounds(sample.effect, sample.visual_group, sample.progress)
        {
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

fn lifecycle_visual_source_for_lamp(
    lifecycle: &LifecycleSceneSample,
    lamp: LifecycleFrameSample,
) -> Option<&LifecycleVisualSource> {
    lifecycle
        .visual_source_for_identity(lamp.presentation_identity)
        .filter(|source| {
            source.root_surface_id == lamp.root_surface_id && source.payload_id == lamp.payload_id
        })
}

fn lifecycle_draw_layer_matches_payload(
    layer: EglDrawLayer,
    payload_id: compositor::PresentationRetainedVisualPayloadId,
    source_kind: LifecycleVisualSourceKind,
) -> bool {
    match (source_kind, layer) {
        (
            LifecycleVisualSourceKind::ResolvedOwnedEffects,
            EglDrawLayer::LifecycleResolvedVisual(layer_payload_id),
        ) => layer_payload_id == payload_id,
        (LifecycleVisualSourceKind::ResolvedOwnedEffects, _) => false,
        (LifecycleVisualSourceKind::NoOwnedEffects, EglDrawLayer::LifecycleResolvedVisual(_)) => {
            false
        }
        (LifecycleVisualSourceKind::NoOwnedEffects, _) => true,
    }
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
        self.scene_state.lamp_program.is_some()
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

        let lamp_uniform_locations = lamp_program.map(|program| unsafe {
            LampUniformLocations {
                output_size: gl.get_uniform_location(program, "u_output_size"),
                canonical_visual_rect: gl.get_uniform_location(program, "u_canonical_visual_rect"),
                source_visual_rect: gl.get_uniform_location(program, "u_source_visual_rect"),
                sink_rect: gl.get_uniform_location(program, "u_sink_rect"),
                progress: gl.get_uniform_location(program, "u_progress"),
                opacity: gl.get_uniform_location(program, "u_opacity"),
                direction: gl.get_uniform_location(program, "u_direction"),
                shape_factor: gl.get_uniform_location(program, "u_shape_factor"),
                bump_distance: gl.get_uniform_location(program, "u_bump_distance"),
                contraction_progress: gl.get_uniform_location(program, "u_contraction_progress"),
                translation_progress: gl.get_uniform_location(program, "u_translation_progress"),
                retreat_progress: gl.get_uniform_location(program, "u_retreat_progress"),
                framebuffer_origin_bottom_left: gl
                    .get_uniform_location(program, "u_framebuffer_origin_bottom_left"),
                texture: gl.get_uniform_location(program, "u_texture"),
            }
        });
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
                lamp_program,
                lamp_uniform_locations,
                lamp_vertex_array,
                lamp_vertex_buffer,
                lamp_vertex_buffer_capacity: MIN_VERTEX_BUFFER_BYTES,
                lamp_geometry_dirty: true,
                lamp_geometry_key: None,
                lamp_vertices: Vec::new(),
                lamp_commands: Vec::new(),
                squash_vertices: Vec::new(),
                squash_commands: Vec::new(),
                squash_vertex_array,
                squash_vertex_buffer,
                squash_vertex_buffer_capacity: MIN_VERTEX_BUFFER_BYTES,
                squash_geometry_dirty: true,
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
            lifecycle_source_vertices: HashMap::new(),
            lifecycle_source_commands: HashMap::new(),
            lifecycle_visual_resources: HashMap::new(),
            lifecycle_visual_sources: HashMap::new(),
            lifecycle_samples: Vec::new(),
            lifecycle_render_evidence: LifecycleRenderEvidence::default(),
            lifecycle_render_fallbacks: LifecycleRenderFallbacks::default(),
            cursor_image,
            texture_upload_rgba: Vec::new(),
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
        })
    }

    pub(in crate::egl_renderer) fn effect_execution_context(
        &mut self,
    ) -> EffectExecutionContext<'_> {
        EffectExecutionContext::new(
            &self.gl,
            &mut self.scene_state,
            &mut self.effect_runtime,
            SceneTextureSources {
                surfaces: &self.surface_resources,
                frames: &self.frame_resources,
                decorations: &self.decoration_resources,
                lifecycle: &self.lifecycle_visual_resources,
                cursor: self.cursor_resource.as_ref(),
            },
        )
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
        self.cursor_resource_stale = true;
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
        self.lifecycle_samples.clear();
        self.lifecycle_samples
            .extend(LifecycleFrameSnapshot::from_sample(lifecycle).samples);
        self.lifecycle_render_evidence.consumed.clear();
        self.lifecycle_render_fallbacks.failed.clear();
        self.lifecycle_visual_sources.clear();
        self.lifecycle_visual_sources.extend(
            lifecycle
                .samples
                .iter()
                .map(|sample| sample.visual_source.clone())
                .map(|source| (source.presentation_identity, source)),
        );
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
        self.release_stale_lifecycle_visual_resources();
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
            lifecycle_damage_for_samples(lifecycle, width, height, output_scale),
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
        self.rebuild_lamp_commands_if_needed(
            lifecycle,
            lifecycle_surfaces,
            lifecycle_decorations,
            output_scale,
        );
        self.rebuild_squash_commands(
            lifecycle,
            lifecycle_surfaces,
            lifecycle_decorations,
            output_scale,
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
        consumer_plan.extend(&plan_lamp_surface_consumers(
            &self.scene_state.lamp_commands,
            &repair_rects,
        ));
        for commands in self.lifecycle_source_commands.values() {
            consumer_plan.extend(&plan_surface_consumers(commands, &repair_rects));
        }
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
        self.record_lifecycle_fallbacks_without_evidence();
        if !self.lifecycle_render_fallbacks.is_empty() {
            self.scene_state.repaint_planner.invalidate();
            return Ok(EglFrameOutcome::LifecycleFallback {
                stats: self.scene_state.frame_stats,
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
            stats: self.scene_state.frame_stats,
            lifecycle_evidence: self.lifecycle_render_evidence.clone(),
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

        self.release_all_lifecycle_visual_resources();
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

        self.scene_state.frame_stats.dmabuf_cache_entries = self.dmabuf_resource_cache.len();
        self.scene_state.frame_stats.dmabuf_cache_peak_entries = self.dmabuf_cache_peak_entries;
        self.scene_state
            .frame_stats
            .dmabuf_cache_max_entries_for_one_surface =
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
        self.scene_state.frame_stats.dmabuf_cache_entries = self.dmabuf_resource_cache.len();
        self.scene_state.frame_stats.dmabuf_cache_peak_entries = self.dmabuf_cache_peak_entries;
        self.scene_state
            .frame_stats
            .dmabuf_cache_max_entries_for_one_surface =
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
                self.scene_state.frame_stats.dmabuf_current_resource_reuses = self
                    .scene_state
                    .frame_stats
                    .dmabuf_current_resource_reuses
                    .saturating_add(1);
                self.scene_state.frame_stats.dmabuf_reuses =
                    self.scene_state.frame_stats.dmabuf_reuses.saturating_add(1);
                return Ok(());
            }
            EglSurfaceResourceUpdate::UploadDamage | EglSurfaceResourceUpdate::FullShmResync => {
                if let Some(resource) = self.surface_resources.get_mut(&surface.surface_id) {
                    let force_full = update == EglSurfaceResourceUpdate::FullShmResync;
                    self.scene_state.frame_stats.shm_upload_bytes = self
                        .scene_state
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
                        self.scene_state.frame_stats.shm_full_resyncs = self
                            .scene_state
                            .frame_stats
                            .shm_full_resyncs
                            .saturating_add(1);
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
                &mut self.scene_state.frame_stats,
                &mut self.failed_surface_generations,
            )? {
                self.scene_state.frame_stats.dmabuf_imports = self
                    .scene_state
                    .frame_stats
                    .dmabuf_imports
                    .saturating_add(1);
                self.surface_resources.insert(surface.surface_id, resource);
            }
        } else {
            match result {
                Ok(resource) => {
                    self.scene_state.frame_stats.shm_upload_bytes = self
                        .scene_state
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
            self.scene_state.frame_stats.dmabuf_cache_hits = self
                .scene_state
                .frame_stats
                .dmabuf_cache_hits
                .saturating_add(1);
            self.scene_state.frame_stats.dmabuf_reuses =
                self.scene_state.frame_stats.dmabuf_reuses.saturating_add(1);
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
        self.scene_state.frame_stats.dmabuf_cache_misses = self
            .scene_state
            .frame_stats
            .dmabuf_cache_misses
            .saturating_add(1);

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
                &mut self.scene_state.frame_stats,
                &mut self.failed_surface_generations,
            )? {
                self.scene_state.frame_stats.dmabuf_imports = self
                    .scene_state
                    .frame_stats
                    .dmabuf_imports
                    .saturating_add(1);
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
            &mut self.scene_state.frame_stats,
            &mut self.failed_surface_generations,
        )? {
            self.scene_state.frame_stats.dmabuf_imports = self
                .scene_state
                .frame_stats
                .dmabuf_imports
                .saturating_add(1);
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
            self.scene_state.frame_stats.dmabuf_cache_evictions = self
                .scene_state
                .frame_stats
                .dmabuf_cache_evictions
                .saturating_add(1);
            self.scene_state.frame_stats.dmabuf_cache_evictions_dead = self
                .scene_state
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
        self.scene_state.frame_stats.dmabuf_cache_insertions = self
            .scene_state
            .frame_stats
            .dmabuf_cache_insertions
            .saturating_add(1);
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
            self.scene_state.frame_stats.dmabuf_cache_evictions = self
                .scene_state
                .frame_stats
                .dmabuf_cache_evictions
                .saturating_add(1);
            self.scene_state
                .frame_stats
                .dmabuf_cache_evictions_surface_bound = self
                .scene_state
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
                self.scene_state.frame_stats.dmabuf_cache_evictions = self
                    .scene_state
                    .frame_stats
                    .dmabuf_cache_evictions
                    .saturating_add(1);
                self.scene_state
                    .frame_stats
                    .dmabuf_cache_evictions_surface_destroyed = self
                    .scene_state
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
                self.scene_state.frame_stats.dmabuf_cache_evictions = self
                    .scene_state
                    .frame_stats
                    .dmabuf_cache_evictions
                    .saturating_add(1);
                self.scene_state.frame_stats.dmabuf_cache_evictions_dead = self
                    .scene_state
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
            && let Some(cursor) = self.cursor_resource.as_ref()
        {
            let (top_left_x, top_left_y) = self.cursor_image.top_left(cursor_x, cursor_y);
            push_draw_command(
                &mut self.scene_state.cursor_vertices,
                &mut self.scene_state.cursor_commands,
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
            self.scene_state.current_framebuffer_origin,
        );
        if self.scene_state.lamp_geometry_key == Some(geometry_key) {
            return;
        }
        self.scene_state.lamp_geometry_key = Some(geometry_key);
        self.scene_state.lamp_geometry_dirty = true;
        self.scene_state.lamp_vertices.clear();
        self.scene_state.lamp_commands.clear();
        if !lifecycle
            .samples
            .iter()
            .any(|sample| sample.effect == LifecycleEffectKind::Lamp)
        {
            return;
        }
        let mut transition_starts = HashMap::new();
        let mut rejected_transitions = HashSet::new();
        let assignments =
            compositor::surface_render_space_assignments(lifecycle_surfaces, output_scale);
        for lamp in LifecycleFrameSnapshot::from_sample(lifecycle)
            .samples
            .into_iter()
            .filter(|sample| sample.effect == LifecycleEffectKind::Lamp)
        {
            let Some(source) = lifecycle_visual_source_for_lamp(lifecycle, lamp) else {
                self.record_lifecycle_render_fallback(
                    lamp,
                    LifecycleRenderFallbackReason::NoConsumedRepresentation,
                );
                continue;
            };
            if source.root_surface_id != lamp.root_surface_id
                || source.payload_id != lamp.payload_id
            {
                self.record_lifecycle_render_fallback(
                    lamp,
                    LifecycleRenderFallbackReason::NoConsumedRepresentation,
                );
                continue;
            }
            let resolved_source =
                (source.kind == LifecycleVisualSourceKind::ResolvedOwnedEffects).then_some(source);
            if let Some(source) = resolved_source {
                if !self
                    .lifecycle_source_vertices
                    .contains_key(&source.payload_id)
                {
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
                            self.scene_state.current_size.0,
                            self.scene_state.current_size.1,
                            surface,
                            assignment.clone(),
                            self.scene_state.current_framebuffer_origin,
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
                            self.scene_state.current_size.0,
                            self.scene_state.current_size.1,
                            decoration,
                            output_scale,
                            self.scene_state.current_framebuffer_origin,
                            visual_group,
                        );
                    }
                    reproject_lifecycle_source_commands(
                        &mut vertices,
                        &mut commands,
                        lamp.visual_group.canonical_client_rect,
                        lamp.visual_group.presented_source_client_rect,
                        output_scale,
                        self.scene_state.current_size,
                        self.scene_state.current_framebuffer_origin,
                    );
                    self.lifecycle_source_vertices
                        .insert(source.payload_id, vertices);
                    self.lifecycle_source_commands
                        .insert(source.payload_id, commands);
                }
                self.append_lamp_grid_for_transition(
                    lamp,
                    LampGridSpec {
                        layer: EglDrawLayer::LifecycleResolvedVisual(source.payload_id),
                        presentation_identity: lamp.presentation_identity,
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
                            presentation_identity: lamp.presentation_identity,
                            bounds: EglRect::new(
                                target.x() as f32,
                                target.y() as f32,
                                target.width() as f32,
                                target.height() as f32,
                            ),
                            uv: EglUvRect::from_surface_uv_quad(render_plan.content_uv),
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
                            presentation_identity: lamp.presentation_identity,
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
        for lamp in LifecycleFrameSnapshot::from_sample(lifecycle)
            .samples
            .into_iter()
            .filter(|sample| sample.effect == LifecycleEffectKind::Lamp)
        {
            if !transition_starts.contains_key(&lamp.presentation_identity)
                && !rejected_transitions.contains(&lamp.presentation_identity)
            {
                self.record_lifecycle_render_fallback(
                    lamp,
                    LifecycleRenderFallbackReason::MeshBudget,
                );
            }
        }
    }

    fn rebuild_squash_commands(
        &mut self,
        lifecycle: &LifecycleSceneSample,
        lifecycle_surfaces: &[RenderableSurface],
        lifecycle_decorations: &[DecorationRenderInstance],
        output_scale: f64,
    ) {
        self.scene_state.squash_vertices.clear();
        self.scene_state.squash_commands.clear();
        self.scene_state.squash_geometry_dirty = true;
        let assignments =
            compositor::surface_render_space_assignments(lifecycle_surfaces, output_scale);
        for sample in lifecycle
            .samples
            .iter()
            .filter(|sample| sample.effect == LifecycleEffectKind::Squash)
        {
            let initial_command_count = self.scene_state.squash_commands.len();
            if sample.visual_source.root_surface_id != sample.root_surface_id
                || sample.visual_source.payload_id != sample.payload_id
                || sample.visual_source.presentation_identity != sample.presentation_identity
            {
                self.record_lifecycle_render_fallback(
                    LifecycleFrameSample {
                        window_id: sample.window_id,
                        root_surface_id: sample.root_surface_id,
                        presentation_identity: sample.presentation_identity,
                        payload_id: sample.payload_id,
                        visual_group: sample.visual_group,
                        effect: sample.effect,
                        progress: sample.progress,
                        effect_opacity: sample.effect_opacity,
                        mathematically_settled: sample.mathematically_settled,
                        direction: sample.direction,
                    },
                    LifecycleRenderFallbackReason::NoConsumedRepresentation,
                );
                continue;
            }
            let frame_sample = LifecycleFrameSample {
                window_id: sample.window_id,
                root_surface_id: sample.root_surface_id,
                presentation_identity: sample.presentation_identity,
                payload_id: sample.payload_id,
                visual_group: sample.visual_group,
                effect: sample.effect,
                progress: sample.progress,
                effect_opacity: sample.effect_opacity,
                mathematically_settled: sample.mathematically_settled,
                direction: sample.direction,
            };

            if sample.visual_source.kind == LifecycleVisualSourceKind::ResolvedOwnedEffects {
                let Some(target_rect) =
                    oblivion_one::window_lifecycle_animation::squash_visual_rect_at_progress(
                        sample.visual_group.presented_source_client_rect,
                        sample.visual_group.anchor_rect,
                        sample.visual_group.presented_source_visual_rect,
                        sample.progress,
                    )
                else {
                    self.record_lifecycle_render_fallback(
                        frame_sample,
                        LifecycleRenderFallbackReason::NoConsumedRepresentation,
                    );
                    continue;
                };
                let target = scaled_presentation_rect(target_rect, output_scale);
                let mut vertices = Vec::new();
                let mut commands = Vec::new();
                push_draw_command(
                    &mut vertices,
                    &mut commands,
                    EglDrawLayer::LifecycleResolvedVisual(sample.payload_id),
                    EglRect::new(
                        target.x() as f32,
                        target.y() as f32,
                        target.width() as f32,
                        target.height() as f32,
                    ),
                    self.scene_state.current_size.0,
                    self.scene_state.current_size.1,
                    self.scene_state.current_framebuffer_origin,
                );
                self.append_squash_batch(sample.presentation_identity, vertices, commands);
            } else {
                for (surface, assignment) in lifecycle_surfaces.iter().zip(assignments.iter()) {
                    if surface_root_for_lamp(surface, lifecycle_surfaces, lifecycle)
                        != sample.root_surface_id
                    {
                        continue;
                    }
                    let mut vertices = Vec::new();
                    let mut commands = Vec::new();
                    push_egl_surface_commands(
                        &mut vertices,
                        &mut commands,
                        self.scene_state.current_size.0,
                        self.scene_state.current_size.1,
                        surface,
                        assignment.clone(),
                        self.scene_state.current_framebuffer_origin,
                        None,
                    );
                    if transform_squash_geometry(
                        &mut vertices,
                        &mut commands,
                        sample.visual_group,
                        sample.progress,
                        output_scale,
                        self.scene_state.current_size,
                        self.scene_state.current_framebuffer_origin,
                    ) {
                        self.append_squash_batch(sample.presentation_identity, vertices, commands);
                    }
                }
                for decoration in lifecycle_decorations
                    .iter()
                    .filter(|decoration| decoration.root_surface_id() == sample.root_surface_id)
                {
                    let mut vertices = Vec::new();
                    let mut commands = Vec::new();
                    push_egl_decoration_instance(
                        &mut vertices,
                        &mut commands,
                        self.scene_state.current_size.0,
                        self.scene_state.current_size.1,
                        decoration,
                        output_scale,
                        self.scene_state.current_framebuffer_origin,
                        None,
                    );
                    if transform_squash_geometry(
                        &mut vertices,
                        &mut commands,
                        sample.visual_group,
                        sample.progress,
                        output_scale,
                        self.scene_state.current_size,
                        self.scene_state.current_framebuffer_origin,
                    ) {
                        self.append_squash_batch(sample.presentation_identity, vertices, commands);
                    }
                }
            }
            if self.scene_state.squash_commands.len() == initial_command_count {
                self.record_lifecycle_render_fallback(
                    frame_sample,
                    LifecycleRenderFallbackReason::NoConsumedRepresentation,
                );
            }
        }
    }

    fn append_squash_batch(
        &mut self,
        presentation_identity: oblivion_one::presentation_animation::PresentationRetainedVisualIdentity,
        vertices: Vec<EglTexturedVertex>,
        commands: Vec<EglDrawCommand>,
    ) {
        let vertex_offset =
            u32::try_from(self.scene_state.squash_vertices.len()).unwrap_or(u32::MAX);
        self.scene_state.squash_vertices.extend(vertices);
        self.scene_state
            .squash_commands
            .extend(commands.into_iter().map(|mut command| {
                command.vertex_start = command.vertex_start.saturating_add(vertex_offset);
                EglSquashDrawCommand {
                    command,
                    presentation_identity,
                }
            }));
    }

    fn append_lamp_grid_for_transition(
        &mut self,
        lamp: LifecycleFrameSample,
        spec: LampGridSpec,
        transition_starts: &mut HashMap<
            oblivion_one::presentation_animation::PresentationRetainedVisualIdentity,
            (usize, usize),
        >,
        rejected_transitions: &mut HashSet<
            oblivion_one::presentation_animation::PresentationRetainedVisualIdentity,
        >,
    ) {
        if rejected_transitions.contains(&lamp.presentation_identity) {
            return;
        }
        let start = *transition_starts
            .entry(lamp.presentation_identity)
            .or_insert((
                self.scene_state.lamp_vertices.len(),
                self.scene_state.lamp_commands.len(),
            ));
        if !append_lamp_grid(
            &mut self.scene_state.lamp_vertices,
            &mut self.scene_state.lamp_commands,
            spec,
        ) {
            self.scene_state.lamp_vertices.truncate(start.0);
            self.scene_state.lamp_commands.truncate(start.1);
            rejected_transitions.insert(lamp.presentation_identity);
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
        let output_scale = self.effect_runtime.effect_output_scale.max(1.0) as f64;
        let sources = self
            .lifecycle_visual_sources
            .values()
            .filter(|source| source.kind == LifecycleVisualSourceKind::ResolvedOwnedEffects)
            .cloned()
            .collect::<Vec<_>>();

        for source in sources {
            let Some(lamp) = self
                .lifecycle_samples
                .iter()
                .find(|sample| {
                    sample.presentation_identity == source.presentation_identity
                        && sample.root_surface_id == source.root_surface_id
                        && sample.payload_id == source.payload_id
                })
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
                .get(&source.payload_id)
                .is_some_and(|resource| {
                    resource.source_signature == source_signature
                        && resource.source_visual_rect
                            == lamp.visual_group.presented_source_visual_rect
                        && self
                            .effect_runtime
                            .effect_resources
                            .texture(&resource.texture)
                            .is_some()
                });
            if ready {
                continue;
            }

            if let Some(previous) = self.lifecycle_visual_resources.remove(&source.payload_id) {
                let _ = self
                    .effect_runtime
                    .effect_resources
                    .release(previous.texture);
            }
            let texture_key = EffectTextureKey::new(
                width,
                height,
                EffectTextureFormat::Rgba8,
                EffectTextureFilter::Linear,
                EffectWorkingSpace::OutputEncodedSrgb,
            );
            let texture = match self
                .effect_runtime
                .effect_resources
                .acquire(&self.gl, texture_key)
            {
                Ok(texture) => texture,
                Err(error) => {
                    self.record_lifecycle_source_capture_failure(
                        &LifecycleSourceCaptureFailure::new(
                            LifecycleSourceCaptureFailureStage::TargetAllocation,
                            error,
                        ),
                    );
                    self.record_lifecycle_render_fallback(
                        lamp,
                        LifecycleRenderFallbackReason::ResolvedSourceAllocation,
                    );
                    continue;
                }
            };
            if let Err(error) = self.capture_lifecycle_visual_source(
                &source,
                lamp,
                texture.clone(),
                framebuffer_origin,
            ) {
                self.record_lifecycle_source_capture_failure(&error);
                if let Err(release_error) = self.effect_runtime.effect_resources.release(texture) {
                    self.record_lifecycle_source_capture_failure(
                        &LifecycleSourceCaptureFailure::new(
                            LifecycleSourceCaptureFailureStage::TargetRelease,
                            Box::new(release_error),
                        ),
                    );
                }
                self.record_lifecycle_render_fallback(
                    lamp,
                    LifecycleRenderFallbackReason::ResolvedSourceCapture,
                );
                continue;
            }
            self.lifecycle_visual_resources.insert(
                source.payload_id,
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
        lamp: LifecycleFrameSample,
        reason: LifecycleRenderFallbackReason,
    ) {
        self.lifecycle_render_fallbacks
            .record(LifecycleRenderFallbackEntry {
                window_id: lamp.window_id,
                root_surface_id: lamp.root_surface_id,
                presentation_identity: lamp.presentation_identity,
                payload_id: lamp.payload_id,
                effect: lamp.effect,
                reason,
            });
    }

    fn record_lifecycle_source_capture_failure(&self, failure: &LifecycleSourceCaptureFailure) {
        self.effect_runtime.effect_trace.event(|| {
            format!(
                "event=lifecycle_source_capture_failure stage={} error={}",
                failure.stage.as_str(),
                failure.source,
            )
        });
    }

    fn capture_lifecycle_visual_source(
        &mut self,
        source: &LifecycleVisualSource,
        lamp: LifecycleFrameSample,
        target: PooledEffectTexture,
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> Result<(), LifecycleSourceCaptureFailure> {
        clear_effect_texture(self, &target).map_err(|error| {
            LifecycleSourceCaptureFailure::new(
                LifecycleSourceCaptureFailureStage::TargetClear,
                error,
            )
        })?;
        let scratch_key = EffectTextureKey::new(
            self.scene_state.current_size.0.max(1),
            self.scene_state.current_size.1.max(1),
            EffectTextureFormat::Rgba8,
            EffectTextureFilter::Linear,
            EffectWorkingSpace::OutputEncodedSrgb,
        );
        let scratch = self
            .effect_runtime
            .effect_resources
            .acquire(&self.gl, scratch_key)
            .map_err(|error| {
                LifecycleSourceCaptureFailure::new(
                    LifecycleSourceCaptureFailureStage::ScratchAllocation,
                    error,
                )
            })?;
        let result = (|| {
            clear_effect_texture(self, &scratch).map_err(|error| {
                LifecycleSourceCaptureFailure::new(
                    LifecycleSourceCaptureFailureStage::ScratchClear,
                    error,
                )
            })?;
            let scratch_framebuffer = self
                .effect_runtime
                .effect_resources
                .bind_lifecycle_composition_target(&self.gl, &scratch)
                .map_err(|error| {
                    LifecycleSourceCaptureFailure::new(
                        LifecycleSourceCaptureFailureStage::ScratchTarget,
                        error,
                    )
                })?;
            let targets = EffectExecutionTargets {
                baseline_read: EffectFramebufferTarget::new(
                    self.scene_state.active_output_framebuffer,
                ),
                composition_draw: EffectFramebufferTarget::new(Some(scratch_framebuffer)),
            };
            if !targets.uses_separate_targets() {
                return Err(LifecycleSourceCaptureFailure::new(
                    LifecycleSourceCaptureFailureStage::ScratchTarget,
                    io::Error::other("lifecycle scratch aliases the active output framebuffer"),
                ));
            }

            let source_vertices = self
                .lifecycle_source_vertices
                .get(&source.payload_id)
                .cloned()
                .unwrap_or_default();
            let source_commands = self
                .lifecycle_source_commands
                .get(&source.payload_id)
                .cloned()
                .unwrap_or_default();
            if source_vertices.is_empty() || source_commands.is_empty() {
                return Err(LifecycleSourceCaptureFailure::new(
                    LifecycleSourceCaptureFailureStage::MissingSourceCommands,
                    io::Error::other("lifecycle visual source has no draw commands"),
                ));
            }

            let saved_vertices = std::mem::replace(&mut self.scene_state.vertices, source_vertices);
            let saved_commands = std::mem::replace(&mut self.scene_state.commands, source_commands);
            self.scene_state.scene_geometry_dirty = true;
            let draw_result = (|| {
                let source_damage =
                    lifecycle_visual_effect_damage(lamp.visual_group.presented_source_visual_rect);
                let output_bounds = EffectRect::new(
                    0,
                    0,
                    self.scene_state.current_size.0.max(1),
                    self.scene_state.current_size.1.max(1),
                )
                .expect("non-zero renderer dimensions must form valid effect bounds");
                let graph = match compile_frame_execution_plan(
                    &source.effect_scene,
                    &source_damage,
                    output_bounds,
                    &self.effect_runtime.effect_registry,
                )
                .map_err(|error| {
                    LifecycleSourceCaptureFailure::new(
                        LifecycleSourceCaptureFailureStage::GraphCompile,
                        error,
                    )
                })? {
                    FrameExecutionPlan::EffectGraph(graph) => graph,
                    FrameExecutionPlan::LegacyScene => {
                        return Err(LifecycleSourceCaptureFailure::new(
                            LifecycleSourceCaptureFailureStage::GraphCompile,
                            io::Error::other(
                                "effect lifecycle source unexpectedly compiled as legacy scene",
                            ),
                        ));
                    }
                };
                let demand = plan_effect_execution_demand(&graph, &source_damage, true);
                let selection = effects::select_effect_execution(&graph, &demand);
                {
                    let mut context = self.effect_execution_context();
                    effects::execute_effect_graph_for_lifecycle(
                        &mut context,
                        &graph,
                        targets,
                        framebuffer_origin,
                        &[lifecycle_visual_output_rect(
                            lamp.visual_group.presented_source_visual_rect,
                            output_bounds,
                        )],
                        &demand,
                        &selection,
                    )
                }
                .map_err(|error| {
                    LifecycleSourceCaptureFailure::new(
                        LifecycleSourceCaptureFailureStage::EffectExecution,
                        error,
                    )
                })?;
                copy_framebuffer_region_to_texture(
                    self,
                    &target,
                    lamp.visual_group.presented_source_visual_rect,
                    framebuffer_origin,
                    targets.composition_draw,
                    targets.composition_draw,
                )
                .map_err(|error| {
                    LifecycleSourceCaptureFailure::new(
                        LifecycleSourceCaptureFailureStage::TargetCopy,
                        error,
                    )
                })?;
                Ok(())
            })();
            self.scene_state.vertices = saved_vertices;
            self.scene_state.commands = saved_commands;
            // The temporary source draw replaced the scene VBO contents.
            // Force the normal scene cache to upload its unchanged geometry
            // before the next ordinary scene draw.
            self.scene_state.scene_geometry_dirty = true;
            draw_result
        })();
        self.establish_ordinary_scene_state();
        let release_result = self
            .effect_runtime
            .effect_resources
            .release(scratch)
            .map_err(|error| {
                LifecycleSourceCaptureFailure::new(
                    LifecycleSourceCaptureFailureStage::ScratchRelease,
                    Box::new(error),
                )
            });
        match (result, release_result) {
            (Err(error), Err(release_error)) => {
                self.record_lifecycle_source_capture_failure(&release_error);
                Err(error)
            }
            (Err(error), Ok(())) => Err(error),
            (Ok(()), Err(error)) => Err(error),
            (Ok(()), Ok(())) => Ok(()),
        }
    }

    fn release_stale_lifecycle_visual_resources(&mut self) {
        let live_payload_ids = self
            .lifecycle_visual_sources
            .values()
            .map(|source| source.payload_id)
            .collect::<HashSet<_>>();
        let stale = self
            .lifecycle_visual_resources
            .keys()
            .copied()
            .filter(|payload_id| !live_payload_ids.contains(payload_id))
            .collect::<Vec<_>>();
        for payload_id in stale {
            if let Some(resource) = self.lifecycle_visual_resources.remove(&payload_id) {
                let _ = self
                    .effect_runtime
                    .effect_resources
                    .release(resource.texture);
            }
        }
        self.lifecycle_source_vertices
            .retain(|payload_id, _| live_payload_ids.contains(payload_id));
        self.lifecycle_source_commands
            .retain(|payload_id, _| live_payload_ids.contains(payload_id));
    }

    fn release_all_lifecycle_visual_resources(&mut self) {
        for (_, resource) in self.lifecycle_visual_resources.drain() {
            let _ = self
                .effect_runtime
                .effect_resources
                .release(resource.texture);
        }
    }

    fn draw_lamp_overlay(&mut self, scissor: Option<OutputRect>) -> RendererResult<()> {
        let (Some(program), Some(uniforms)) = (
            self.scene_state.lamp_program,
            self.scene_state.lamp_uniform_locations,
        ) else {
            self.record_visible_lifecycle_fallbacks(
                LifecycleRenderFallbackReason::LampProgramUnavailable,
            );
            return Ok(());
        };
        if self.scene_state.lamp_vertices.is_empty() || self.scene_state.lamp_commands.is_empty() {
            return Ok(());
        }
        let required_size =
            self.scene_state.lamp_vertices.len() * std::mem::size_of::<EglLampVertex>();
        ensure_vertex_buffer_capacity(
            &self.gl,
            self.scene_state.lamp_vertex_buffer,
            &mut self.scene_state.lamp_vertex_buffer_capacity,
            required_size,
        );
        if self.scene_state.lamp_geometry_dirty {
            unsafe {
                self.gl.bind_buffer(
                    glow::ARRAY_BUFFER,
                    Some(self.scene_state.lamp_vertex_buffer),
                );
                self.gl.buffer_sub_data_u8_slice(
                    glow::ARRAY_BUFFER,
                    0,
                    bytemuck::cast_slice(self.scene_state.lamp_vertices.as_slice()),
                );
            }
            self.scene_state.lamp_geometry_dirty = false;
        }
        let samples = self
            .scene_state
            .lamp_commands
            .iter()
            .filter_map(|command| {
                let sample = self.lamp_geometry_sample(command.presentation_identity)?;
                let source = self
                    .lifecycle_visual_sources
                    .get(&command.presentation_identity)?;
                if source.root_surface_id != sample.root_surface_id
                    || source.payload_id != sample.payload_id
                    || !lifecycle_draw_layer_matches_payload(
                        command.layer,
                        sample.payload_id,
                        source.kind,
                    )
                {
                    return None;
                }
                if self.lifecycle_visual_source_is_ready(sample.payload_id)
                    && !matches!(command.layer, EglDrawLayer::LifecycleResolvedVisual(_))
                {
                    return None;
                }
                Some((*command, sample))
            })
            .collect::<Vec<_>>();
        unsafe {
            self.gl.use_program(Some(program));
            self.gl
                .bind_vertex_array(Some(self.scene_state.lamp_vertex_array));
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
                    self.scene_state.current_size.0 as f32,
                    self.scene_state.current_size.1 as f32,
                );
            }
            if let Some(location) = &uniforms.framebuffer_origin_bottom_left {
                self.gl.uniform_1_i32(
                    Some(location),
                    i32::from(
                        self.scene_state.current_framebuffer_origin
                            == OutputFramebufferOrigin::BottomLeft,
                    ),
                );
            }
        }
        let output_scale = self.effect_runtime.effect_output_scale.max(1.0) as f64;
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
                    uniforms.sink_rect.as_ref(),
                    sample.visual_group.sink_rect,
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
                let channels =
                    lamp_motion_channels(sample.progress, sample.visual_group.bump_distance);
                if let Some(location) = &uniforms.contraction_progress {
                    self.gl
                        .uniform_1_f32(Some(location), channels.contraction_progress as f32);
                }
                if let Some(location) = &uniforms.translation_progress {
                    self.gl
                        .uniform_1_f32(Some(location), channels.translation_progress as f32);
                }
                if let Some(location) = &uniforms.retreat_progress {
                    self.gl
                        .uniform_1_f32(Some(location), channels.retreat_progress as f32);
                }
                if let Some(location) = &uniforms.progress {
                    self.gl
                        .uniform_1_f32(Some(location), channels.temporal_progress as f32);
                }
                if let Some(location) = &uniforms.opacity {
                    self.gl
                        .uniform_1_f32(Some(location), sample.effect_opacity as f32);
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
                    presentation_identity: sample.presentation_identity,
                    payload_id: sample.payload_id,
                });
        }
        self.scene_state
            .frame_stats
            .missing_required_decoration_resources = self
            .scene_state
            .frame_stats
            .missing_required_decoration_resources
            .saturating_add(missing_required_decoration_resources);
        unsafe {
            self.gl.use_program(Some(self.scene_state.program));
            self.gl
                .bind_vertex_array(Some(self.scene_state.scene_vertex_array));
        }
        Ok(())
    }

    fn draw_squash_overlay(&mut self, scissor: Option<OutputRect>) -> RendererResult<()> {
        if self.scene_state.squash_vertices.is_empty()
            || self.scene_state.squash_commands.is_empty()
        {
            return Ok(());
        }
        let required_size =
            self.scene_state.squash_vertices.len() * std::mem::size_of::<EglTexturedVertex>();
        ensure_vertex_buffer_capacity(
            &self.gl,
            self.scene_state.squash_vertex_buffer,
            &mut self.scene_state.squash_vertex_buffer_capacity,
            required_size,
        );
        if self.scene_state.squash_geometry_dirty {
            unsafe {
                self.gl.bind_buffer(
                    glow::ARRAY_BUFFER,
                    Some(self.scene_state.squash_vertex_buffer),
                );
                self.gl.buffer_sub_data_u8_slice(
                    glow::ARRAY_BUFFER,
                    0,
                    bytemuck::cast_slice(self.scene_state.squash_vertices.as_slice()),
                );
            }
            self.scene_state.squash_geometry_dirty = false;
        }
        let active = self
            .lifecycle_samples
            .iter()
            .filter(|sample| sample.effect == LifecycleEffectKind::Squash)
            .copied()
            .collect::<Vec<_>>();
        let mut draw_calls = 0_usize;
        let mut texture_binds = 0_usize;
        let mut missing_required_decoration_resources = 0_usize;
        unsafe {
            self.gl.use_program(Some(self.scene_state.program));
            self.gl
                .bind_vertex_array(Some(self.scene_state.squash_vertex_array));
            self.gl.active_texture(glow::TEXTURE0);
            self.gl.enable(glow::BLEND);
            self.gl.blend_func_separate(
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
            );
        }
        let mut current_sampling = None;
        for sample in active {
            let commands = self
                .scene_state
                .squash_commands
                .iter()
                .filter(|entry| entry.presentation_identity == sample.presentation_identity)
                .filter(|entry| {
                    scissor.is_none_or(|rect| entry.command.bounds.intersects_output_rect(rect))
                })
                .collect::<Vec<_>>();
            if commands.is_empty() {
                continue;
            }
            if commands
                .iter()
                .any(|entry| self.texture_for_layer(entry.command.layer).is_none())
            {
                if commands
                    .iter()
                    .any(|entry| Self::is_required_decoration_layer(entry.command.layer))
                {
                    missing_required_decoration_resources =
                        missing_required_decoration_resources.saturating_add(1);
                }
                self.record_lifecycle_render_fallback(
                    sample,
                    LifecycleRenderFallbackReason::LifecycleResourceUnavailable,
                );
                continue;
            }
            for entry in commands {
                let command = &entry.command;
                let Some(texture) = self.texture_for_layer(command.layer) else {
                    continue;
                };
                unsafe {
                    self.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                    texture_binds = texture_binds.saturating_add(1);
                    if current_sampling != Some(command.sampling) {
                        let filter = match command.sampling {
                            SurfaceSampling::ExactNearest => glow::NEAREST,
                            SurfaceSampling::ScaledLinear => glow::LINEAR,
                        } as i32;
                        self.gl.tex_parameter_i32(
                            glow::TEXTURE_2D,
                            glow::TEXTURE_MIN_FILTER,
                            filter,
                        );
                        self.gl.tex_parameter_i32(
                            glow::TEXTURE_2D,
                            glow::TEXTURE_MAG_FILTER,
                            filter,
                        );
                        current_sampling = Some(command.sampling);
                    }
                    if let Some(location) = &self.scene_state.presentation_opacity_location {
                        self.gl
                            .uniform_1_f32(Some(location), sample.effect_opacity as f32);
                    }
                    self.gl.draw_arrays(
                        glow::TRIANGLES,
                        command.vertex_start as i32,
                        command.vertex_count as i32,
                    );
                }
                draw_calls = draw_calls.saturating_add(1);
                self.lifecycle_render_evidence
                    .record(LifecycleRenderEvidenceEntry {
                        window_id: sample.window_id,
                        root_surface_id: sample.root_surface_id,
                        presentation_identity: sample.presentation_identity,
                        payload_id: sample.payload_id,
                    });
            }
        }
        self.scene_state.frame_stats.draw_calls = self
            .scene_state
            .frame_stats
            .draw_calls
            .saturating_add(draw_calls);
        self.scene_state.frame_stats.commands_executed = self
            .scene_state
            .frame_stats
            .commands_executed
            .saturating_add(draw_calls);
        self.scene_state.frame_stats.texture_binds = self
            .scene_state
            .frame_stats
            .texture_binds
            .saturating_add(texture_binds);
        self.scene_state
            .frame_stats
            .missing_required_decoration_resources = self
            .scene_state
            .frame_stats
            .missing_required_decoration_resources
            .saturating_add(missing_required_decoration_resources);
        unsafe {
            self.gl.use_program(Some(self.scene_state.program));
            self.gl
                .bind_vertex_array(Some(self.scene_state.scene_vertex_array));
        }
        Ok(())
    }

    fn record_visible_lifecycle_fallbacks(&mut self, reason: LifecycleRenderFallbackReason) {
        let visible = visible_lifecycle_samples(
            &self.lifecycle_samples,
            f64::from(self.effect_runtime.effect_output_scale),
            self.scene_state.current_size.0,
            self.scene_state.current_size.1,
        )
        .collect::<Vec<_>>();
        for sample in visible {
            self.record_lifecycle_render_fallback(sample, reason);
        }
    }

    fn record_lifecycle_fallbacks_without_evidence(&mut self) {
        let missing = visible_lifecycle_samples(
            &self.lifecycle_samples,
            f64::from(self.effect_runtime.effect_output_scale),
            self.scene_state.current_size.0,
            self.scene_state.current_size.1,
        )
        .filter(|sample| {
            !self.lifecycle_render_evidence.contains(
                sample.presentation_identity,
                sample.payload_id,
                sample.root_surface_id,
            )
        })
        .collect::<Vec<_>>();
        for sample in missing {
            self.record_lifecycle_render_fallback(
                sample,
                LifecycleRenderFallbackReason::NoConsumedRepresentation,
            );
        }
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
                    .scene_state
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
            self.draw_squash_overlay(Some(*rect))?;
        }
        unsafe {
            self.gl.disable(glow::SCISSOR_TEST);
        }
        Ok(())
    }

    fn lamp_geometry_sample(
        &self,
        presentation_identity: oblivion_one::presentation_animation::PresentationRetainedVisualIdentity,
    ) -> Option<LifecycleFrameSample> {
        self.lifecycle_samples
            .iter()
            .find(|sample| sample.presentation_identity == presentation_identity)
            .copied()
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

    fn lifecycle_visual_source_is_ready(
        &self,
        payload_id: compositor::PresentationRetainedVisualPayloadId,
    ) -> bool {
        self.lifecycle_visual_resources
            .get(&payload_id)
            .is_some_and(|resource| {
                self.effect_runtime
                    .effect_resources
                    .texture(&resource.texture)
                    .is_some()
            })
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
            EglDrawLayer::LifecycleResolvedVisual(payload_id) => self
                .lifecycle_visual_resources
                .get(&payload_id)
                .and_then(|resource| {
                    self.effect_runtime
                        .effect_resources
                        .texture(&resource.texture)
                }),
            EglDrawLayer::Cursor => self
                .cursor_resource
                .as_ref()
                .map(|resource| resource.texture),
        }
    }

    const fn is_required_decoration_layer(layer: EglDrawLayer) -> bool {
        matches!(
            layer,
            EglDrawLayer::SolidRgba(_) | EglDrawLayer::DecorationAsset(_)
        )
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
        self.effect_runtime.destroy_persistent_resources(&self.gl);

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

fn transform_squash_geometry(
    vertices: &mut [EglTexturedVertex],
    commands: &mut [EglDrawCommand],
    visual_group: LifecycleVisualGroup,
    progress: f64,
    output_scale: f64,
    output_size: (u32, u32),
    framebuffer_origin: OutputFramebufferOrigin,
) -> bool {
    let source_client =
        scaled_presentation_rect(visual_group.presented_source_client_rect, output_scale);
    let Some(current_client) =
        oblivion_one::window_lifecycle_animation::squash_client_rect_at_progress(
            visual_group.presented_source_client_rect,
            visual_group.anchor_rect,
            progress,
        )
    else {
        return false;
    };
    let current_client = scaled_presentation_rect(current_client, output_scale);
    if !source_client.x().is_finite()
        || !source_client.y().is_finite()
        || !source_client.width().is_finite()
        || !source_client.height().is_finite()
        || source_client.width() <= 0.0
        || source_client.height() <= 0.0
    {
        return false;
    }
    let map_point = |point: [f64; 2]| {
        [
            current_client.x()
                + (point[0] - source_client.x()) * current_client.width() / source_client.width(),
            current_client.y()
                + (point[1] - source_client.y()) * current_client.height() / source_client.height(),
        ]
    };
    for vertex in vertices {
        let point = ndc_to_output_point(vertex.position, output_size, framebuffer_origin);
        vertex.position = output_point_to_ndc(map_point(point), output_size, framebuffer_origin);
    }
    let map_rect = |rect: EglRect| {
        let source_rect = compositor::PresentationRect::new(
            f64::from(rect.x()),
            f64::from(rect.y()),
            f64::from(rect.width()),
            f64::from(rect.height()),
        )?;
        let transformed = oblivion_one::window_lifecycle_animation::squash_transform_rect(
            source_client,
            current_client,
            source_rect,
        )?;
        Some(EglRect::new(
            transformed.x() as f32,
            transformed.y() as f32,
            transformed.width() as f32,
            transformed.height() as f32,
        ))
    };
    for command in commands {
        let Some(bounds) = map_rect(command.bounds) else {
            return false;
        };
        command.bounds = bounds;
        command.presentation_clip = match command.presentation_clip {
            Some(clip) => match map_rect(clip) {
                Some(clip) => Some(clip),
                None => return false,
            },
            None => None,
        };
        command.opaque_regions = command
            .opaque_regions
            .iter()
            .filter_map(|rect| map_rect(*rect))
            .collect();
    }
    true
}

fn lifecycle_visual_source_signature(
    source: &LifecycleVisualSource,
    lamp: LifecycleFrameSample,
    output_scale: f64,
) -> u64 {
    let mut signature = source.effect_scene.signature ^ source.payload_id.get();
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct GlBlitRect {
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
}

impl GlBlitRect {
    const fn new(x0: i32, y0: i32, x1: i32, y1: i32) -> Self {
        Self { x0, y0, x1, y1 }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LifecycleOutputBlitPlan {
    output: GlBlitRect,
    texture: GlBlitRect,
}

fn lifecycle_output_copy_region(
    output_size: (u32, u32),
    rect: compositor::PresentationRect,
    texture_size: (u32, u32),
    output_scale: f64,
    framebuffer_origin: OutputFramebufferOrigin,
) -> Option<LifecycleOutputBlitPlan> {
    if output_size.0 == 0 || output_size.1 == 0 || texture_size.0 == 0 || texture_size.1 == 0 {
        return None;
    }
    let scale = output_scale.max(1.0);
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
    let visible_left = i64::from(left).max(0).min(i64::from(output_size.0));
    let visible_top = i64::from(top).max(0).min(i64::from(output_size.1));
    let visible_right = i64::from(right).max(0).min(i64::from(output_size.0));
    let visible_bottom = i64::from(bottom).max(0).min(i64::from(output_size.1));
    if visible_right <= visible_left || visible_bottom <= visible_top {
        return None;
    }

    let visible_left = i32::try_from(visible_left).ok()?;
    let visible_top = i32::try_from(visible_top).ok()?;
    let visible_right = i32::try_from(visible_right).ok()?;
    let visible_bottom = i32::try_from(visible_bottom).ok()?;
    let output_height = i32::try_from(output_size.1).ok()?;
    let texture_width = i32::try_from(texture_size.0).ok()?;
    let texture_height = i32::try_from(texture_size.1).ok()?;
    let texture_left = visible_left.checked_sub(left)?;
    let texture_right = texture_left.checked_add(visible_right.checked_sub(visible_left)?)?;
    let logical_texture_top = visible_top.checked_sub(top)?;
    let logical_texture_bottom =
        logical_texture_top.checked_add(visible_bottom.checked_sub(visible_top)?)?;
    let texture_low_y = texture_height.checked_sub(logical_texture_bottom)?;
    let texture_high_y = texture_height.checked_sub(logical_texture_top)?;
    if texture_left < 0
        || texture_right > texture_width
        || texture_low_y < 0
        || texture_high_y > texture_height
        || texture_right <= texture_left
        || texture_high_y <= texture_low_y
    {
        return None;
    }

    let output = match framebuffer_origin {
        OutputFramebufferOrigin::BottomLeft => GlBlitRect::new(
            visible_left,
            output_height.checked_sub(visible_bottom)?,
            visible_right,
            output_height.checked_sub(visible_top)?,
        ),
        OutputFramebufferOrigin::TopLeftScanout => {
            GlBlitRect::new(visible_left, visible_top, visible_right, visible_bottom)
        }
    };
    let texture = match framebuffer_origin {
        OutputFramebufferOrigin::BottomLeft => {
            GlBlitRect::new(texture_left, texture_low_y, texture_right, texture_high_y)
        }
        OutputFramebufferOrigin::TopLeftScanout => {
            GlBlitRect::new(texture_left, texture_high_y, texture_right, texture_low_y)
        }
    };
    Some(LifecycleOutputBlitPlan { output, texture })
}

#[cfg(test)]
fn copy_output_region_to_texture(
    renderer: &mut GlesSceneRenderer,
    target: &PooledEffectTexture,
    rect: compositor::PresentationRect,
    framebuffer_origin: OutputFramebufferOrigin,
) -> RendererResult<()> {
    let output = EffectFramebufferTarget::new(renderer.scene_state.active_output_framebuffer);
    copy_framebuffer_region_to_texture(renderer, target, rect, framebuffer_origin, output, output)
}

fn copy_framebuffer_region_to_texture(
    renderer: &mut GlesSceneRenderer,
    target: &PooledEffectTexture,
    rect: compositor::PresentationRect,
    framebuffer_origin: OutputFramebufferOrigin,
    source: EffectFramebufferTarget,
    restore_target: EffectFramebufferTarget,
) -> RendererResult<()> {
    let Some(plan) = lifecycle_output_copy_region(
        renderer.scene_state.current_size,
        rect,
        (target.key.width, target.key.height),
        f64::from(renderer.effect_runtime.effect_output_scale),
        framebuffer_origin,
    ) else {
        return Ok(());
    };
    let result = (|| {
        let draw_framebuffer = renderer
            .effect_runtime
            .effect_resources
            .bind_draw_target(&renderer.gl, target)?;
        if source.framebuffer == Some(draw_framebuffer) {
            return Err(io::Error::other("lifecycle source and texture targets alias").into());
        }
        unsafe {
            renderer.gl.disable(glow::SCISSOR_TEST);
            renderer
                .gl
                .bind_framebuffer(glow::READ_FRAMEBUFFER, source.framebuffer);
            renderer
                .gl
                .bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(draw_framebuffer));
            renderer.gl.blit_framebuffer(
                plan.output.x0,
                plan.output.y0,
                plan.output.x1,
                plan.output.y1,
                plan.texture.x0,
                plan.texture.y0,
                plan.texture.x1,
                plan.texture.y1,
                glow::COLOR_BUFFER_BIT,
                glow::NEAREST,
            );
        }
        Ok(())
    })();
    renderer.establish_effect_composition_state(restore_target);
    result
}

fn clear_effect_texture(
    renderer: &mut GlesSceneRenderer,
    target: &PooledEffectTexture,
) -> RendererResult<()> {
    let result = (|| {
        renderer
            .effect_runtime
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
        renderer
            .effect_runtime
            .effect_resources
            .unbind_render_target(&renderer.gl);
        Ok(())
    })();
    renderer.establish_ordinary_scene_state();
    result
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
mod tests;
