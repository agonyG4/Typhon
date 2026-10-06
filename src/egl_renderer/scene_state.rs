use super::*;
use glow::HasContext;

pub(in crate::egl_renderer) fn establish_scene_gl_state(
    gl: &glow::Context,
    output_size: (u32, u32),
    program: GlProgram,
    framebuffer: Option<glow::Framebuffer>,
) {
    unsafe {
        gl.bind_framebuffer(glow::FRAMEBUFFER, framebuffer);
        gl.viewport(0, 0, output_size.0 as i32, output_size.1 as i32);
        gl.use_program(Some(program));
        gl.active_texture(glow::TEXTURE0);
        gl.disable(glow::SCISSOR_TEST);
        gl.enable(glow::BLEND);
        gl.blend_func_separate(
            glow::ONE,
            glow::ONE_MINUS_SRC_ALPHA,
            glow::ONE,
            glow::ONE_MINUS_SRC_ALPHA,
        );
    }
}

/// Ordinary scene replay, composition, output, and checkpoint state.
pub(crate) struct SceneRenderState {
    pub(in crate::egl_renderer) program: GlProgram,
    pub(in crate::egl_renderer) presentation_opacity_location: Option<glow::UniformLocation>,
    pub(in crate::egl_renderer) scene_vertex_array: GlVertexArray,
    pub(in crate::egl_renderer) scene_vertex_buffer: GlBuffer,
    pub(in crate::egl_renderer) scene_vertex_buffer_capacity: usize,
    pub(in crate::egl_renderer) scene_geometry_dirty: bool,
    pub(in crate::egl_renderer) overlay_vertex_array: GlVertexArray,
    pub(in crate::egl_renderer) overlay_vertex_buffer: GlBuffer,
    pub(in crate::egl_renderer) overlay_vertex_buffer_capacity: usize,
    pub(in crate::egl_renderer) overlay_geometry_dirty: bool,
    pub(in crate::egl_renderer) current_framebuffer_origin: OutputFramebufferOrigin,
    pub(in crate::egl_renderer) current_size: (u32, u32),
    pub(in crate::egl_renderer) vertices: Vec<EglTexturedVertex>,
    pub(in crate::egl_renderer) commands: Vec<EglDrawCommand>,
    pub(in crate::egl_renderer) cursor_vertices: Vec<EglTexturedVertex>,
    pub(in crate::egl_renderer) cursor_commands: Vec<EglDrawCommand>,
    pub(in crate::egl_renderer) presentation_opacities: Vec<f32>,
    pub(in crate::egl_renderer) cursor_presentation_opacities: Vec<f32>,
    pub(in crate::egl_renderer) presentation_visual_group_opacities: HashMap<VisualGroupId, f32>,
    pub(in crate::egl_renderer) presentation_visual_group_clips: HashMap<VisualGroupId, EglRect>,
    pub(in crate::egl_renderer) presentation_visual_group_owners: HashMap<VisualGroupId, u32>,
    pub(in crate::egl_renderer) scene_visibility_plan: Vec<EglVisibilityDecision>,
    pub(in crate::egl_renderer) scene_cache_key: Option<EglSceneCacheKey>,
    pub(in crate::egl_renderer) presented_scene_key: Option<EglSceneCacheKey>,
    pub(in crate::egl_renderer) current_checkpoint_scene_causal_snapshot:
        Option<EglCheckpointSceneCausalSnapshot>,
    pub(in crate::egl_renderer) damage_tracker: EglOutputDamageTracker,
    pub(in crate::egl_renderer) repaint_planner: PartialRepaintPlanner,
    pub(in crate::egl_renderer) active_output_framebuffer: Option<glow::Framebuffer>,
    pub(in crate::egl_renderer) active_output_texture: Option<glow::Texture>,
    pub(in crate::egl_renderer) frame_stats: GlesSceneFrameStats,
    pub(in crate::egl_renderer) capture_unattenuated_visual_group: Option<VisualGroupId>,
    pub(in crate::egl_renderer) capture_unclipped_presentation_owner: Option<u32>,
}

/// Read-only scene texture lookup view. Resource ownership stays with the
/// renderer; retained effect textures are resolved against the runtime pool.
pub(in crate::egl_renderer) struct SceneTextureSources<'a> {
    pub(in crate::egl_renderer) surfaces: &'a HashMap<u32, EglSurfaceResource>,
    pub(in crate::egl_renderer) frames: &'a HashMap<compositor::ServerFrameColor, EglImageResource>,
    pub(in crate::egl_renderer) decorations: &'a HashMap<DecorationResourceKey, EglImageResource>,
    pub(in crate::egl_renderer) lifecycle: &'a HashMap<
        compositor::PresentationRetainedVisualPayloadId,
        LifecycleResolvedVisualResource,
    >,
    pub(in crate::egl_renderer) cursor: Option<&'a EglImageResource>,
}

impl SceneTextureSources<'_> {
    pub(in crate::egl_renderer) fn texture_for_layer(
        &self,
        layer: EglDrawLayer,
        effects: &EffectGlResourceCache,
    ) -> Option<glow::Texture> {
        match layer {
            EglDrawLayer::Solid(color) => self.frames.get(&color).map(|resource| resource.texture),
            EglDrawLayer::SolidRgba(color) => self
                .decorations
                .get(&DecorationResourceKey::Solid(color))
                .map(|resource| resource.texture),
            EglDrawLayer::DecorationAsset(asset_id) => self
                .decorations
                .get(&DecorationResourceKey::Asset(asset_id))
                .map(|resource| resource.texture),
            EglDrawLayer::Surface(surface_id) => self
                .surfaces
                .get(&surface_id)
                .map(|resource| resource.image.texture),
            EglDrawLayer::LifecycleResolvedVisual(payload_id) => self
                .lifecycle
                .get(&payload_id)
                .and_then(|resource| effects.texture(resource.pooled_texture())),
            EglDrawLayer::Cursor => self.cursor.map(|resource| resource.texture),
        }
    }
}

/// Transient output/checkpoint state intentionally restored after screenshots.
/// Command buffers, GL geometry, and resource caches are never copied here.
pub(in crate::egl_renderer) struct SceneCaptureSnapshot {
    current_framebuffer_origin: OutputFramebufferOrigin,
    current_size: (u32, u32),
    presented_scene_key: Option<EglSceneCacheKey>,
    current_checkpoint_scene_causal_snapshot: Option<EglCheckpointSceneCausalSnapshot>,
    damage_tracker: EglOutputDamageTracker,
    repaint_planner: PartialRepaintPlanner,
    frame_stats: GlesSceneFrameStats,
    active_output_framebuffer: Option<glow::Framebuffer>,
    active_output_texture: Option<glow::Texture>,
    capture_unattenuated_visual_group: Option<VisualGroupId>,
    capture_unclipped_presentation_owner: Option<u32>,
}

impl SceneCaptureSnapshot {
    pub(in crate::egl_renderer) fn take(scene: &SceneRenderState) -> Self {
        Self {
            current_framebuffer_origin: scene.current_framebuffer_origin,
            current_size: scene.current_size,
            presented_scene_key: scene.presented_scene_key,
            current_checkpoint_scene_causal_snapshot: scene
                .current_checkpoint_scene_causal_snapshot
                .clone(),
            damage_tracker: scene.damage_tracker.clone(),
            repaint_planner: scene.repaint_planner.clone(),
            frame_stats: scene.frame_stats,
            active_output_framebuffer: scene.active_output_framebuffer,
            active_output_texture: scene.active_output_texture,
            capture_unattenuated_visual_group: scene.capture_unattenuated_visual_group,
            capture_unclipped_presentation_owner: scene.capture_unclipped_presentation_owner,
        }
    }

    pub(in crate::egl_renderer) fn restore(self, scene: &mut SceneRenderState) {
        scene.current_framebuffer_origin = self.current_framebuffer_origin;
        scene.current_size = self.current_size;
        scene.presented_scene_key = self.presented_scene_key;
        scene.current_checkpoint_scene_causal_snapshot =
            self.current_checkpoint_scene_causal_snapshot;
        scene.damage_tracker = self.damage_tracker;
        scene.repaint_planner = self.repaint_planner;
        scene.frame_stats = self.frame_stats;
        scene.active_output_framebuffer = self.active_output_framebuffer;
        scene.active_output_texture = self.active_output_texture;
        scene.capture_unattenuated_visual_group = self.capture_unattenuated_visual_group;
        scene.capture_unclipped_presentation_owner = self.capture_unclipped_presentation_owner;
    }
}

impl SceneRenderState {
    pub(in crate::egl_renderer) fn establish_scene_state_for_framebuffer(
        &self,
        gl: &glow::Context,
        framebuffer: Option<glow::Framebuffer>,
    ) {
        establish_scene_gl_state(gl, self.current_size, self.program, framebuffer);
    }

    pub(in crate::egl_renderer) fn presentation_opacity_for_visual_group(
        &self,
        visual_group: Option<VisualGroupId>,
    ) -> f32 {
        visual_group
            .and_then(|group| {
                self.presentation_visual_group_opacities
                    .get(&group)
                    .copied()
            })
            .unwrap_or(1.0)
            .clamp(0.0, 1.0)
    }

    pub(in crate::egl_renderer) fn presentation_clip_for_visual_group(
        &self,
        visual_group: Option<VisualGroupId>,
    ) -> Option<EglRect> {
        visual_group.and_then(|group| self.presentation_visual_group_clips.get(&group).copied())
    }

    pub(in crate::egl_renderer) fn presentation_owner_for_visual_group(
        &self,
        visual_group: Option<VisualGroupId>,
    ) -> Option<u32> {
        visual_group.and_then(|group| self.presentation_visual_group_owners.get(&group).copied())
    }

    pub(in crate::egl_renderer) fn begin_effect_repaint(
        &mut self,
        gl: &glow::Context,
        plan: &RepaintPlan,
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> RendererResult<Vec<OutputRect>> {
        self.establish_scene_state_for_framebuffer(gl, self.active_output_framebuffer);
        unsafe { gl.clear_color(0.0, 0.0, 0.0, 1.0) };
        let execution = plan
            .render_execution(self.current_size.0, self.current_size.1, framebuffer_origin)
            .ok_or_else(|| io::Error::other("effect repaint conversion failed"))?;
        let mut rects = Vec::new();
        match execution {
            RenderExecution::Full => unsafe {
                gl.disable(glow::SCISSOR_TEST);
                gl.clear(glow::COLOR_BUFFER_BIT);
                rects.push(OutputRect::new(
                    0,
                    0,
                    self.current_size.0,
                    self.current_size.1,
                ));
            },
            RenderExecution::Scissored { scissors, .. } => unsafe {
                gl.enable(glow::SCISSOR_TEST);
                for scissor in scissors {
                    gl.scissor(scissor[0], scissor[1], scissor[2], scissor[3]);
                    gl.clear(glow::COLOR_BUFFER_BIT);
                    if let Some(rect) =
                        gl_scissor_to_output_rect(scissor, self.current_size.1, framebuffer_origin)
                    {
                        rects.push(rect);
                    }
                }
                gl.disable(glow::SCISSOR_TEST);
            },
        }
        Ok(rects)
    }

    pub(in crate::egl_renderer) fn clear_effect_scene_work(
        &mut self,
        gl: &glow::Context,
        rects: &[OutputRect],
        framebuffer_origin: OutputFramebufferOrigin,
        composition_draw: EffectFramebufferTarget,
    ) -> RendererResult<()> {
        self.establish_scene_state_for_framebuffer(gl, composition_draw.framebuffer);
        unsafe {
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
            gl.enable(glow::SCISSOR_TEST);
            for rect in rects {
                let y = match framebuffer_origin {
                    OutputFramebufferOrigin::BottomLeft => self
                        .current_size
                        .1
                        .saturating_sub(rect.y.max(0) as u32 + rect.height)
                        as i32,
                    OutputFramebufferOrigin::TopLeftScanout => rect.y,
                };
                gl.scissor(rect.x, y, rect.width as i32, rect.height as i32);
                gl.clear(glow::COLOR_BUFFER_BIT);
            }
            gl.disable(glow::SCISSOR_TEST);
        }
        Ok(())
    }

    pub(in crate::egl_renderer) fn set_scene_scissor(
        &self,
        gl: &glow::Context,
        scissor: Option<OutputRect>,
    ) {
        unsafe {
            if let Some(rect) = scissor {
                let y = match self.current_framebuffer_origin {
                    OutputFramebufferOrigin::BottomLeft => self
                        .current_size
                        .1
                        .saturating_sub(rect.y.max(0) as u32 + rect.height)
                        as i32,
                    OutputFramebufferOrigin::TopLeftScanout => rect.y,
                };
                gl.enable(glow::SCISSOR_TEST);
                gl.scissor(rect.x, y, rect.width as i32, rect.height as i32);
            } else {
                gl.disable(glow::SCISSOR_TEST);
            }
        }
    }

    pub(in crate::egl_renderer) fn plan_scene_visibility(&mut self, scissor: Option<OutputRect>) {
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
        let stats =
            geometry::plan_visibility(&self.commands, repair, &mut self.scene_visibility_plan);
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
}

impl SceneRenderState {
    pub(in crate::egl_renderer) fn destroy_gl_resources(&mut self, gl: &glow::Context) {
        unsafe {
            gl.delete_buffer(self.scene_vertex_buffer);
            gl.delete_vertex_array(self.scene_vertex_array);
            gl.delete_buffer(self.overlay_vertex_buffer);
            gl.delete_vertex_array(self.overlay_vertex_array);
            gl.delete_program(self.program);
        }
    }
}
