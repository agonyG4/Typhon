use super::*;
use glow::HasContext;

/// Temporary borrowed access to the renderer's authoritative scene and effect
/// owners. It never owns or clones GL resources, scene commands, or caches.
pub(in crate::egl_renderer) struct EffectExecutionContext<'a> {
    pub(super) gl: &'a glow::Context,
    pub(super) scene: &'a mut SceneRenderState,
    pub(super) runtime: &'a mut EffectRuntime,
    sources: SceneTextureSources<'a>,
}

impl<'a> EffectExecutionContext<'a> {
    pub(in crate::egl_renderer) fn new(
        gl: &'a glow::Context,
        scene: &'a mut SceneRenderState,
        runtime: &'a mut EffectRuntime,
        sources: SceneTextureSources<'a>,
    ) -> Self {
        Self {
            gl,
            scene,
            runtime,
            sources,
        }
    }

    pub(super) fn texture_for_layer(&self, layer: EglDrawLayer) -> Option<glow::Texture> {
        self.sources
            .texture_for_layer(layer, &self.runtime.effect_resources)
    }

    pub(super) fn capture_uniform_location(&mut self, name: &str) -> Option<glow::UniformLocation> {
        self.runtime.capture_uniform_location(self.gl, name)
    }

    pub(super) fn ensure_effect_quad(
        &mut self,
    ) -> RendererResult<(glow::VertexArray, glow::Buffer)> {
        self.runtime.ensure_effect_quad(self.gl)
    }

    pub(super) fn establish_scene_state_for_framebuffer(
        &self,
        framebuffer: Option<glow::Framebuffer>,
    ) {
        super::super::super::scene_state::establish_scene_gl_state(
            self.gl,
            self.scene.current_size,
            self.scene.program,
            framebuffer,
        );
    }

    pub(super) fn establish_ordinary_scene_state(&self) {
        self.establish_scene_state_for_framebuffer(self.scene.active_output_framebuffer);
    }

    pub(super) fn establish_effect_composition_state(&self, target: EffectFramebufferTarget) {
        self.establish_scene_state_for_framebuffer(target.framebuffer);
    }

    pub(super) fn bind_active_output_framebuffer(&self) {
        unsafe {
            self.gl
                .bind_framebuffer(glow::FRAMEBUFFER, self.scene.active_output_framebuffer);
        }
    }

    pub(super) fn presentation_opacity_for_visual_group(
        &self,
        visual_group: Option<VisualGroupId>,
    ) -> f32 {
        self.scene
            .presentation_opacity_for_visual_group(visual_group)
    }

    pub(super) fn presentation_clip_for_visual_group(
        &self,
        visual_group: Option<VisualGroupId>,
    ) -> Option<EglRect> {
        self.scene.presentation_clip_for_visual_group(visual_group)
    }

    pub(super) fn presentation_owner_for_visual_group(
        &self,
        visual_group: Option<VisualGroupId>,
    ) -> Option<u32> {
        self.scene.presentation_owner_for_visual_group(visual_group)
    }
}

impl EffectExecutionContext<'_> {
    pub(super) fn begin_effect_repaint(
        &mut self,
        plan: &RepaintPlan,
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> RendererResult<Vec<OutputRect>> {
        self.scene
            .begin_effect_repaint(self.gl, plan, framebuffer_origin)
    }

    pub(super) fn clear_effect_scene_work(
        &mut self,
        rects: &[OutputRect],
        framebuffer_origin: OutputFramebufferOrigin,
        composition_draw: EffectFramebufferTarget,
    ) -> RendererResult<()> {
        self.scene
            .clear_effect_scene_work(self.gl, rects, framebuffer_origin, composition_draw)
    }

    pub(in crate::egl_renderer) fn draw_effect_scene_range(
        &mut self,
        rects: &[OutputRect],
        start: usize,
        end: usize,
        framebuffer_origin: OutputFramebufferOrigin,
        composition_draw: EffectFramebufferTarget,
    ) -> RendererResult<()> {
        let end = end.min(self.scene.commands.len());
        let start = start.min(end);
        for rect in rects {
            let y = match framebuffer_origin {
                OutputFramebufferOrigin::BottomLeft => self
                    .scene
                    .current_size
                    .1
                    .saturating_sub(rect.y.max(0) as u32 + rect.height)
                    as i32,
                OutputFramebufferOrigin::TopLeftScanout => rect.y,
            };
            unsafe {
                self.establish_effect_composition_state(composition_draw);
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

    pub(in crate::egl_renderer) fn draw_effect_overlays(
        &mut self,
        rects: &[OutputRect],
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> RendererResult<()> {
        for rect in rects {
            let y = match framebuffer_origin {
                OutputFramebufferOrigin::BottomLeft => self
                    .scene
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

    pub(in crate::egl_renderer) fn draw_command_batch(
        &mut self,
        scene: bool,
        scissor: Option<OutputRect>,
    ) -> RendererResult<()> {
        self.draw_command_batch_with_visibility_and_range(scene, scissor, true, None, true)
    }

    pub(super) fn draw_command_batch_range(
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

    pub(in crate::egl_renderer) fn draw_command_batch_with_visibility(
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

    pub(in crate::egl_renderer) fn draw_command_batch_with_visibility_and_range(
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
        let opacity_location = if self.runtime.capture_in_progress
            || self.scene.capture_unattenuated_visual_group.is_some()
        {
            self.capture_uniform_location("u_opacity")
        } else {
            self.scene.presentation_opacity_location
        };
        let (vertices, commands) = if scene {
            (&self.scene.vertices, &self.scene.commands)
        } else {
            (&self.scene.cursor_vertices, &self.scene.cursor_commands)
        };
        let presentation_opacities = if scene {
            &self.scene.presentation_opacities
        } else {
            &self.scene.cursor_presentation_opacities
        };
        if vertices.is_empty() || commands.is_empty() {
            return Ok(());
        }

        let required_size = vertices.len() * std::mem::size_of::<EglTexturedVertex>();
        let mut upload_bytes = 0;
        let mut uploaded = false;
        let vertex_array = if scene {
            ensure_vertex_buffer_capacity(
                self.gl,
                self.scene.scene_vertex_buffer,
                &mut self.scene.scene_vertex_buffer_capacity,
                required_size,
            );
            if self.scene.scene_geometry_dirty {
                upload_bytes = required_size;
                unsafe {
                    self.gl
                        .bind_buffer(glow::ARRAY_BUFFER, Some(self.scene.scene_vertex_buffer));
                    self.gl.buffer_sub_data_u8_slice(
                        glow::ARRAY_BUFFER,
                        0,
                        bytemuck::cast_slice(vertices.as_slice()),
                    );
                }
                self.scene.scene_geometry_dirty = false;
                uploaded = true;
            }
            self.scene.scene_vertex_array
        } else {
            ensure_vertex_buffer_capacity(
                self.gl,
                self.scene.overlay_vertex_buffer,
                &mut self.scene.overlay_vertex_buffer_capacity,
                required_size,
            );
            if self.scene.overlay_geometry_dirty {
                upload_bytes = required_size;
                unsafe {
                    self.gl
                        .bind_buffer(glow::ARRAY_BUFFER, Some(self.scene.overlay_vertex_buffer));
                    self.gl.buffer_sub_data_u8_slice(
                        glow::ARRAY_BUFFER,
                        0,
                        bytemuck::cast_slice(vertices.as_slice()),
                    );
                }
                self.scene.overlay_geometry_dirty = false;
                uploaded = true;
            }
            self.scene.overlay_vertex_array
        };
        unsafe {
            self.gl.bind_vertex_array(Some(vertex_array));
        }

        let mut current_sampling = None;
        let initial_scissor = scissor;
        let mut current_scissor = scissor;
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
            let bypass_presentation_clip = self
                .scene
                .capture_unclipped_presentation_owner
                .is_some_and(|owner| {
                    command
                        .visual_group
                        .and_then(|group| self.scene.presentation_visual_group_owners.get(&group))
                        .is_some_and(|command_owner| *command_owner == owner)
                });
            let presentation_clip = if bypass_presentation_clip {
                None
            } else {
                command.presentation_clip
            };
            let effective_scissor = if let Some(clip) = presentation_clip {
                let Some(clip) = output_rect_for_egl_clip(clip) else {
                    continue;
                };
                let Some(effective) =
                    scissor.map_or(Some(clip), |damage| intersect_output_rect(damage, clip))
                else {
                    continue;
                };
                Some(effective)
            } else {
                scissor
            };
            if current_scissor != effective_scissor {
                self.set_scene_scissor(effective_scissor);
                current_scissor = effective_scissor;
            }
            if effective_scissor.is_some_and(|rect| !command.bounds.intersects_output_rect(rect)) {
                commands_rejected_outside_damage += 1;
                continue;
            }
            if scene && use_visibility_plan {
                match self.scene.scene_visibility_plan[command_index] {
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
                if let Some(location) = opacity_location.as_ref() {
                    let presentation_opacity =
                        if self.scene.capture_unattenuated_visual_group == command.visual_group {
                            1.0
                        } else {
                            presentation_opacities
                                .get(command_index)
                                .copied()
                                .unwrap_or(1.0)
                        };
                    self.gl.uniform_1_f32(Some(location), presentation_opacity);
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
        if current_scissor != initial_scissor {
            self.set_scene_scissor(initial_scissor);
        }
        self.scene.frame_stats.commands_considered = self
            .scene
            .frame_stats
            .commands_considered
            .saturating_add(commands_considered);
        self.scene.frame_stats.commands_executed = self
            .scene
            .frame_stats
            .commands_executed
            .saturating_add(commands_executed);
        self.scene.frame_stats.missing_required_decoration_resources = self
            .scene
            .frame_stats
            .missing_required_decoration_resources
            .saturating_add(missing_required_decoration_resources);
        self.scene.frame_stats.commands_rejected_outside_damage = self
            .scene
            .frame_stats
            .commands_rejected_outside_damage
            .saturating_add(commands_rejected_outside_damage);
        self.scene.frame_stats.texture_binds = self
            .scene
            .frame_stats
            .texture_binds
            .saturating_add(texture_binds);
        self.scene.frame_stats.draw_calls =
            self.scene.frame_stats.draw_calls.saturating_add(draw_calls);
        self.scene.frame_stats.draw_command_replays = self
            .scene
            .frame_stats
            .draw_command_replays
            .saturating_add(commands_executed);
        if uploaded {
            if scene {
                self.scene.frame_stats.scene_vbo_uploads =
                    self.scene.frame_stats.scene_vbo_uploads.saturating_add(1);
                self.scene.frame_stats.scene_vbo_upload_bytes = self
                    .scene
                    .frame_stats
                    .scene_vbo_upload_bytes
                    .saturating_add(upload_bytes);
            } else {
                self.scene.frame_stats.overlay_vbo_uploads =
                    self.scene.frame_stats.overlay_vbo_uploads.saturating_add(1);
                self.scene.frame_stats.overlay_vbo_upload_bytes = self
                    .scene
                    .frame_stats
                    .overlay_vbo_upload_bytes
                    .saturating_add(upload_bytes);
            }
        }
        Ok(())
    }

    pub(super) fn set_scene_scissor(&self, scissor: Option<OutputRect>) {
        self.scene.set_scene_scissor(self.gl, scissor)
    }

    pub(super) fn draw_capture_commands(
        &mut self,
        command_indices: &[usize],
        output_rect: OutputRect,
        target_domain: EffectRect,
        target_size: (u32, u32),
        host_timing_enabled: bool,
    ) -> RendererResult<ReplayCaptureExecutionDetail> {
        let requested = EffectRect::new(
            output_rect.x,
            output_rect.y,
            output_rect.width,
            output_rect.height,
        )
        .and_then(|rect| rect.intersect(target_domain));
        let Some(requested) = requested else {
            return Ok(ReplayCaptureExecutionDetail::default());
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
        let saved_visibility = std::mem::take(&mut self.scene.scene_visibility_plan);
        let repair = EglRect::new(
            output_rect.x as f32,
            output_rect.y as f32,
            output_rect.width as f32,
            output_rect.height as f32,
        );
        let visibility_start = host_timing_enabled.then(Instant::now);
        let unclipped_visual_groups = self
            .scene
            .capture_unclipped_presentation_owner
            .map(|owner| {
                self.scene
                    .presentation_visual_group_owners
                    .iter()
                    .filter_map(|(group, command_owner)| {
                        (*command_owner == owner).then_some(*group)
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let capture_stats = plan_capture_visibility(
            &self.scene.commands,
            command_indices,
            repair,
            &unclipped_visual_groups,
            &mut self.scene.scene_visibility_plan,
        );
        let visibility_cpu_ns = monotonic_elapsed_ns(visibility_start);
        let commands_considered_before = self.scene.frame_stats.commands_considered;
        let commands_executed_before = self.scene.frame_stats.commands_executed;
        let draw_calls_before = self.scene.frame_stats.draw_calls;
        let draw_submit_start = host_timing_enabled.then(Instant::now);
        let result = self.draw_command_batch_with_visibility(true, Some(output_rect), false);
        let draw_submit_cpu_ns = monotonic_elapsed_ns(draw_submit_start);
        unsafe { self.gl.disable(glow::SCISSOR_TEST) };
        self.scene.frame_stats.planner_commands_visited = self
            .scene
            .frame_stats
            .planner_commands_visited
            .saturating_add(capture_stats.commands_visited);
        self.scene.scene_visibility_plan = saved_visibility;
        let detail = ReplayCaptureExecutionDetail {
            planner_commands_visited: capture_stats.commands_visited,
            planner_commands_drawable: capture_stats.commands_drawable,
            commands_considered: self
                .scene
                .frame_stats
                .commands_considered
                .saturating_sub(commands_considered_before),
            commands_executed: self
                .scene
                .frame_stats
                .commands_executed
                .saturating_sub(commands_executed_before),
            draw_calls: self
                .scene
                .frame_stats
                .draw_calls
                .saturating_sub(draw_calls_before),
            visibility_cpu_ns,
            draw_submit_cpu_ns,
            ..Default::default()
        };
        result.map(|()| detail)
    }

    pub(super) fn draw_capture_commands_for_regions(
        &mut self,
        command_indices: &[usize],
        output_rects: &[OutputRect],
        target_domain: EffectRect,
        target_size: (u32, u32),
        host_timing_enabled: bool,
    ) -> RendererResult<ReplayCaptureExecutionDetail> {
        let layout = replay_capture_region_layout(output_rects);
        let mut detail = ReplayCaptureExecutionDetail {
            execution_pixels: 0,
            materialization_rects: layout.materialization_rects,
            execution_regions: layout.execution_regions,
            disjoint_overflowed: layout.disjoint_overflowed,
            candidate_commands: command_indices.len(),
            scene_commands_total: self.scene.commands.len(),
            ..Default::default()
        };
        for output_rect in layout
            .execution_region
            .rects()
            .iter()
            .copied()
            .map(|rect| OutputRect::new(rect.x, rect.y, rect.width, rect.height))
        {
            let region_detail = self.draw_capture_commands(
                command_indices,
                output_rect,
                target_domain,
                target_size,
                host_timing_enabled,
            )?;
            detail.planner_commands_visited = detail
                .planner_commands_visited
                .saturating_add(region_detail.planner_commands_visited);
            detail.planner_commands_drawable = detail
                .planner_commands_drawable
                .saturating_add(region_detail.planner_commands_drawable);
            detail.commands_considered = detail
                .commands_considered
                .saturating_add(region_detail.commands_considered);
            detail.commands_executed = detail
                .commands_executed
                .saturating_add(region_detail.commands_executed);
            detail.draw_calls = detail.draw_calls.saturating_add(region_detail.draw_calls);
            detail.visibility_cpu_ns = detail
                .visibility_cpu_ns
                .saturating_add(region_detail.visibility_cpu_ns);
            detail.draw_submit_cpu_ns = detail
                .draw_submit_cpu_ns
                .saturating_add(region_detail.draw_submit_cpu_ns);
        }
        detail.command_region_pairs = detail.planner_commands_visited;
        detail.scene_scan_pairs = detail.commands_considered;
        Ok(detail)
    }

    pub(super) fn plan_scene_visibility(&mut self, scissor: Option<OutputRect>) {
        self.scene.plan_scene_visibility(scissor)
    }

    const fn is_required_decoration_layer(layer: EglDrawLayer) -> bool {
        matches!(
            layer,
            EglDrawLayer::SolidRgba(_) | EglDrawLayer::DecorationAsset(_)
        )
    }
}
