use oblivion_one::{
    compositor::{self, DecorationRenderInstance, RenderableSurface, SurfaceResourceSyncState},
    window_lifecycle_animation::{
        LifecycleRenderEvidence, LifecycleRenderFallbacks, LifecycleSceneSample,
    },
};

use super::super::{
    EglSceneCacheKey, OutputFramebufferOrigin,
    damage::{
        BufferAge, EglPresentedDamageState, FullRepaintReason, OutputDamage,
        PartialRepaintComplexityAction, PartialRepaintComplexityPolicy, RepaintMode, RepaintPlan,
    },
    effects::EffectFailureReason,
};

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
#[allow(dead_code)]
pub(crate) struct EglOutputRenderTarget {
    pub(crate) framebuffer: glow::Framebuffer,
    pub(crate) sampleable_texture: Option<glow::Texture>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) buffer_age: BufferAge,
    pub(crate) framebuffer_origin: OutputFramebufferOrigin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EglSceneFrameCommit {
    pub(in crate::egl_renderer) repaint_plan: RepaintPlan,
    pub(in crate::egl_renderer) damage_state: EglPresentedDamageState,
    pub(in crate::egl_renderer) scene_key: EglSceneCacheKey,
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
    pub visual_state: compositor::DesktopVisualState,
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
