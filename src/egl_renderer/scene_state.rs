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
    pub(in crate::egl_renderer) ordinary: ResourceTextureView<'a>,
    pub(in crate::egl_renderer) lifecycle: &'a HashMap<
        compositor::PresentationRetainedVisualPayloadId,
        LifecycleResolvedVisualResource,
    >,
}

impl SceneTextureSources<'_> {
    pub(in crate::egl_renderer) fn texture_for_layer(
        &self,
        layer: EglDrawLayer,
        effects: &EffectGlResourceCache,
    ) -> Option<glow::Texture> {
        match layer {
            EglDrawLayer::Solid(color) => self.ordinary.texture_for_frame(color),
            EglDrawLayer::SolidRgba(color) => self.ordinary.texture_for_solid_decoration(color),
            EglDrawLayer::DecorationAsset(asset_id) => {
                self.ordinary.texture_for_decoration_asset(asset_id)
            }
            EglDrawLayer::Surface(surface_id) => self.ordinary.texture_for_surface(surface_id),
            EglDrawLayer::LifecycleResolvedVisual(payload_id) => self
                .lifecycle
                .get(&payload_id)
                .and_then(|resource| effects.texture(resource.pooled_texture())),
            EglDrawLayer::Cursor => self.ordinary.cursor_texture(),
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
    pub(in crate::egl_renderer) fn presentation_opacity_for_root(
        presentation_opacities: &[oblivion_one::presentation_animation::PresentationGroupOpacity],
        owner_root: u32,
    ) -> f32 {
        presentation_opacities
            .iter()
            .find(|entry| entry.root_surface_id == owner_root)
            .map_or(1.0, |entry| entry.opacity.get() as f32)
            .clamp(0.0, 1.0)
    }

    pub(in crate::egl_renderer) fn scene_cache_is_current(
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
        self.scene_cache_key.is_some_and(|key| {
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
    pub(in crate::egl_renderer) fn rebuild_scene_commands(
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
        self.frame_stats.orphan_decoration_count =
            compositor::WindowVisualGroup::orphan_decoration_count(surfaces, decoration_instances);
        self.vertices.clear();
        self.commands.clear();
        self.presentation_opacities.clear();
        self.presentation_visual_group_opacities.clear();
        self.presentation_visual_group_clips.clear();
        self.presentation_visual_group_owners.clear();
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
            let command_start = self.commands.len();
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
            let group_root = group.root_surface_id();
            let owner_root = presentation_owner_roots_by_surface
                .get(&group_root)
                .copied()
                .unwrap_or(group_root);
            let opacity = Self::presentation_opacity_for_root(presentation_opacities, owner_root);
            if let Some(visual_group) = visual_group {
                self.presentation_visual_group_owners
                    .insert(visual_group, owner_root);
                self.presentation_visual_group_opacities
                    .insert(visual_group, opacity);
                if let Some(clip) = presentation_clips
                    .iter()
                    .find(|clip| clip.root_surface_id == owner_root)
                    .and_then(|clip| clip.presented_clip)
                {
                    let scale = output_scale.max(0.01);
                    self.presentation_visual_group_clips.insert(
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
            for _ in command_start..self.commands.len() {
                self.presentation_opacities.push(opacity);
            }
            let presentation_clip = visual_group
                .and_then(|visual_group| self.presentation_visual_group_clips.get(&visual_group))
                .copied();
            for command in &mut self.commands[command_start..] {
                command.presentation_clip = presentation_clip;
            }
            if opacity < 1.0 {
                for command in &mut self.commands[command_start..] {
                    command.opaque_regions.clear();
                }
            }
        }

        self.scene_cache_key = Some(
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
    pub(in crate::egl_renderer) fn rebuild_overlay_commands(
        &mut self,
        width: u32,
        height: u32,
        visual_state: DesktopVisualState,
        overlay_surfaces: &[RenderableSurface],
        client_cursor: Option<compositor::ClientCursorRenderState<'_>>,
        output_scale: f64,
        framebuffer_origin: OutputFramebufferOrigin,
        cursor_image: &CompositorCursorImage,
        cursor_size: Option<(u32, u32)>,
    ) {
        self.cursor_vertices.clear();
        self.cursor_commands.clear();
        self.cursor_presentation_opacities.clear();
        self.overlay_geometry_dirty = true;

        let render_assignments =
            compositor::surface_render_space_assignments(overlay_surfaces, output_scale);
        for (surface, render_assignment) in overlay_surfaces.iter().zip(render_assignments) {
            let command_start = self.cursor_commands.len();
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
            self.cursor_presentation_opacities
                .extend(std::iter::repeat_n(
                    1.0,
                    self.cursor_commands.len().saturating_sub(command_start),
                ));
        }

        if let Some((cursor_x, cursor_y)) = visual_state.cursor
            && let Some(cursor_size) = cursor_size
        {
            let (top_left_x, top_left_y) = cursor_image.top_left(cursor_x, cursor_y);
            push_draw_command(
                &mut self.cursor_vertices,
                &mut self.cursor_commands,
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
            self.cursor_presentation_opacities.push(1.0);
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
            self.cursor_presentation_opacities.push(1.0);
        }
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
