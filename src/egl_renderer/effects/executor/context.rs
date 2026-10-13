use super::super::gpu_timing::{
    CaptureTimingMetadata, GraphTimingScope, PassTimingSpan, PassTimingWork,
};
use super::*;
use glow::HasContext;

#[derive(Clone, Copy)]
enum RegionPlanScissorMode {
    Framebuffer(OutputFramebufferOrigin),
    CaptureTarget {
        framebuffer_origin: OutputFramebufferOrigin,
        target_domain: EffectRect,
        target_size: (u32, u32),
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CaptureTargetGlScissor {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

/// Convert a logical top-left output-space clip to a capture-local GL scissor.
/// The offscreen capture framebuffer always uses GL's bottom-left origin, so
/// its own height and the translated capture domain determine the conversion;
/// the output framebuffer origin does not.
fn capture_target_gl_scissor_for_output_clip(
    output_clip: OutputRect,
    capture_domain: OutputRect,
    target_size: (u32, u32),
) -> Option<CaptureTargetGlScissor> {
    let clipped_output = intersect_output_rect(output_clip, capture_domain)?;
    let capture_local_top_left = OutputRect::new(
        clipped_output.x.checked_sub(capture_domain.x)?,
        clipped_output.y.checked_sub(capture_domain.y)?,
        clipped_output.width,
        clipped_output.height,
    );
    let target_bounds = OutputRect::new(0, 0, target_size.0, target_size.1);
    let capture_local_top_left = intersect_output_rect(capture_local_top_left, target_bounds)?;
    let target_height = i32::try_from(target_size.1).ok()?;
    let height = i32::try_from(capture_local_top_left.height).ok()?;
    let y = target_height.checked_sub(capture_local_top_left.y.checked_add(height)?)?;
    Some(CaptureTargetGlScissor {
        x: capture_local_top_left.x,
        y,
        width: i32::try_from(capture_local_top_left.width).ok()?,
        height,
    })
}

/// Temporary borrowed access to the renderer's authoritative scene and effect
/// owners. It never owns or clones GL resources, scene commands, or caches.
pub(in crate::egl_renderer) struct EffectExecutionContext<'a> {
    pub(super) gl: &'a glow::Context,
    pub(super) scene: &'a mut SceneRenderState,
    pub(super) runtime: &'a mut EffectRuntime,
    sources: SceneTextureSources<'a>,
    pub(super) output_target_space: crate::egl_renderer::lifecycle::LifecycleRenderTargetSpace,
    lifecycle_dispatch: Option<crate::egl_renderer::lifecycle::LifecycleReplayDispatcher<'a>>,
}

impl<'a> EffectExecutionContext<'a> {
    pub(in crate::egl_renderer) fn new(
        gl: &'a glow::Context,
        scene: &'a mut SceneRenderState,
        runtime: &'a mut EffectRuntime,
        sources: SceneTextureSources<'a>,
        output_target_space: crate::egl_renderer::lifecycle::LifecycleRenderTargetSpace,
    ) -> Self {
        Self::new_with_lifecycle_dispatch(gl, scene, runtime, sources, output_target_space, None)
    }

    pub(in crate::egl_renderer) fn new_with_lifecycle_dispatch(
        gl: &'a glow::Context,
        scene: &'a mut SceneRenderState,
        runtime: &'a mut EffectRuntime,
        sources: SceneTextureSources<'a>,
        output_target_space: crate::egl_renderer::lifecycle::LifecycleRenderTargetSpace,
        lifecycle_dispatch: Option<crate::egl_renderer::lifecycle::LifecycleReplayDispatcher<'a>>,
    ) -> Self {
        scene.ensure_capture_command_metadata();
        Self {
            gl,
            scene,
            runtime,
            sources,
            output_target_space,
            lifecycle_dispatch,
        }
    }

    pub(super) fn disable_lifecycle_replay(&mut self) {
        self.lifecycle_dispatch = None;
    }

    fn dispatch_lifecycle_overlay(
        &mut self,
        identity: oblivion_one::presentation_animation::PresentationRetainedVisualIdentity,
        scissor: Option<OutputRect>,
        target_space: crate::egl_renderer::lifecycle::LifecycleRenderTargetSpace,
    ) -> RendererResult<()> {
        let Some(dispatcher) = self.lifecycle_dispatch.as_mut() else {
            return Ok(());
        };
        let result = {
            let mut lifecycle_context = crate::egl_renderer::lifecycle::LifecycleRenderContext::new(
                self.gl,
                self.scene,
                self.runtime,
                self.sources.ordinary,
                target_space,
            );
            dispatcher.draw_marker(&mut lifecycle_context, identity, scissor)
        };
        let resume_program = match target_space {
            crate::egl_renderer::lifecycle::LifecycleRenderTargetSpace::Output { .. } => {
                self.scene.program
            }
            crate::egl_renderer::lifecycle::LifecycleRenderTargetSpace::Capture { .. } => {
                self.runtime.capture_program
            }
        };
        unsafe {
            self.gl.use_program(Some(resume_program));
            self.gl.active_texture(glow::TEXTURE0);
            self.gl
                .bind_vertex_array(Some(self.scene.scene_vertex_array));
            self.gl.enable(glow::BLEND);
            self.gl.blend_func_separate(
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
            );
        }
        result
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

    pub(in crate::egl_renderer) fn begin_precomputed_build_scope(
        &mut self,
    ) -> Option<GraphTimingScope> {
        self.runtime
            .effect_gpu_profiler
            .begin_precomputed_build(self.gl, self.runtime.effect_trace.frame_id())
    }

    pub(in crate::egl_renderer) fn begin_precomputed_build_stage(
        &mut self,
        scope: GraphTimingScope,
        stage_index: usize,
        kind: RenderPassKind,
        work: PassTimingWork,
        capture: Option<CaptureTimingMetadata>,
    ) -> Option<PassTimingSpan> {
        self.runtime.effect_gpu_profiler.begin_precomputed_stage(
            self.gl,
            scope,
            stage_index,
            kind,
            work,
            capture,
        )
    }

    pub(in crate::egl_renderer) fn end_precomputed_build_stage(
        &mut self,
        span: Option<PassTimingSpan>,
    ) {
        self.runtime
            .effect_gpu_profiler
            .end_pass(self.gl, span, None, 0);
    }

    pub(in crate::egl_renderer) fn end_precomputed_build_scope(
        &mut self,
        scope: Option<GraphTimingScope>,
    ) -> bool {
        self.runtime.effect_gpu_profiler.end_graph(self.gl, scope)
    }

    pub(in crate::egl_renderer) fn replay_precomputed_backdrop_capture(
        &mut self,
        capture_texture: &PooledEffectTexture,
        anchor: oblivion_one::compositor::EffectAnchor,
        visual_group: Option<VisualGroupId>,
        anchor_scope: oblivion_one::compositor::EffectAnchorScope,
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> RendererResult<Option<GraphTimingScope>> {
        let output_size = self.scene.current_size;
        let capture_pixels = u64::from(output_size.0).saturating_mul(u64::from(output_size.1));
        let capture_work = PassTimingWork {
            effect_pixels: capture_pixels,
            damage_rect_count: 1,
            damage_bbox_pixels: capture_pixels,
            target_width: output_size.0,
            target_height: output_size.1,
        };
        let (prefix_end, _) = composition_range_with_metadata(
            &self.scene.commands,
            &self.scene.capture_layers,
            &self.scene.capture_visual_groups,
            anchor,
            visual_group,
            anchor_scope,
        );
        let full_output = [OutputRect::new(0, 0, output_size.0, output_size.1)];
        let timing_scope = self.begin_precomputed_build_scope();
        let capture_span = timing_scope.and_then(|scope| {
            self.begin_precomputed_build_stage(
                scope,
                0,
                RenderPassKind::SceneCapture,
                capture_work,
                Some(CaptureTimingMetadata {
                    mode: super::super::gpu_timing::CaptureTimingMode::Replay,
                    checkpoint_count: 0,
                }),
            )
        });
        let capture_result: RendererResult<()> = (|| {
            let framebuffer = self
                .runtime
                .effect_resources
                .bind_draw_target(self.gl, capture_texture)?;
            self.establish_scene_state_for_framebuffer(Some(framebuffer));
            unsafe {
                self.gl.disable(glow::SCISSOR_TEST);
                self.gl.disable(glow::BLEND);
                self.gl.clear_color(0.0, 0.0, 0.0, 1.0);
                self.gl.clear(glow::COLOR_BUFFER_BIT);
            }
            self.draw_effect_scene_range(
                &full_output,
                0,
                prefix_end,
                framebuffer_origin,
                EffectFramebufferTarget::new(Some(framebuffer)),
            )
        })();
        self.runtime.effect_resources.unbind_render_target(self.gl);
        self.establish_scene_state_for_framebuffer(self.scene.active_output_framebuffer);
        self.end_precomputed_build_stage(capture_span);
        match capture_result {
            Ok(()) => Ok(timing_scope),
            Err(error) => {
                self.end_precomputed_build_scope(timing_scope);
                Err(error)
            }
        }
    }
}

impl EffectExecutionContext<'_> {
    /// Execute an ordinary or effect scene plan command-first. Plans contain
    /// only command and repair indices; commands and textures remain owned by
    /// their existing renderer state.
    pub(in crate::egl_renderer) fn draw_scene_region_plan(&mut self) -> RendererResult<()> {
        self.draw_command_region_plan(
            false,
            RegionPlanScissorMode::Framebuffer(self.output_target_space.framebuffer_origin()),
        )
    }

    fn draw_effect_region_plan(
        &mut self,
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> RendererResult<()> {
        self.draw_command_region_plan(true, RegionPlanScissorMode::Framebuffer(framebuffer_origin))
    }

    fn draw_capture_region_plan(
        &mut self,
        framebuffer_origin: OutputFramebufferOrigin,
        target_domain: EffectRect,
        target_size: (u32, u32),
    ) -> RendererResult<()> {
        self.draw_command_region_plan(
            true,
            RegionPlanScissorMode::CaptureTarget {
                framebuffer_origin,
                target_domain,
                target_size,
            },
        )
    }

    fn draw_command_region_plan(
        &mut self,
        effect_plan: bool,
        scissor_mode: RegionPlanScissorMode,
    ) -> RendererResult<()> {
        let capture_plan = matches!(scissor_mode, RegionPlanScissorMode::CaptureTarget { .. });
        let target_space = match scissor_mode {
            RegionPlanScissorMode::Framebuffer(origin) => {
                crate::egl_renderer::lifecycle::LifecycleRenderTargetSpace::output(
                    self.output_target_space.output_size(),
                    origin,
                )
            }
            RegionPlanScissorMode::CaptureTarget {
                framebuffer_origin,
                target_domain,
                target_size,
            } => crate::egl_renderer::lifecycle::LifecycleRenderTargetSpace::capture(
                self.output_target_space.output_size(),
                OutputRect::new(
                    target_domain.x,
                    target_domain.y,
                    target_domain.width,
                    target_domain.height,
                ),
                target_size,
                framebuffer_origin,
            ),
        };
        let plan_is_executable = if effect_plan {
            self.scene
                .effect_region_plan
                .is_executable_for(&self.scene.commands)
        } else {
            self.scene
                .scene_region_plan
                .is_executable_for(&self.scene.commands)
        };
        if !plan_is_executable {
            return Err(std::io::Error::other("scene region plan is stale").into());
        }
        let planned_command_count = if effect_plan {
            self.scene.effect_region_plan.commands.len()
        } else {
            self.scene.scene_region_plan.commands.len()
        };
        let opacity_location = if self.runtime.capture_in_progress
            || self.scene.capture_unattenuated_visual_group.is_some()
        {
            self.capture_uniform_location("u_opacity")
        } else {
            self.scene.presentation_opacity_location
        };
        let (outer_commands_visited, command_repair_tests, planned_commands, clip_pieces) =
            if effect_plan {
                let stats = &self.scene.effect_region_plan.stats;
                (
                    stats.outer_commands_visited,
                    stats.command_repair_tests,
                    stats.planned_commands,
                    stats.clip_pieces,
                )
            } else {
                let stats = &self.scene.scene_region_plan.stats;
                (
                    stats.outer_commands_visited,
                    stats.command_repair_tests,
                    stats.planned_commands,
                    stats.clip_pieces,
                )
            };
        if capture_plan {
            self.scene
                .frame_stats
                .replay_capture_actual_scene_commands_visited = self
                .scene
                .frame_stats
                .replay_capture_actual_scene_commands_visited
                .saturating_add(outer_commands_visited);
            self.scene
                .frame_stats
                .replay_capture_actual_command_region_tests = self
                .scene
                .frame_stats
                .replay_capture_actual_command_region_tests
                .saturating_add(command_repair_tests);
            self.scene
                .frame_stats
                .replay_capture_actual_planned_commands = self
                .scene
                .frame_stats
                .replay_capture_actual_planned_commands
                .saturating_add(planned_commands);
            self.scene.frame_stats.replay_capture_actual_clip_pieces = self
                .scene
                .frame_stats
                .replay_capture_actual_clip_pieces
                .saturating_add(clip_pieces);
        } else if effect_plan {
            self.scene.frame_stats.effect_scene_replay_commands_visited = self
                .scene
                .frame_stats
                .effect_scene_replay_commands_visited
                .saturating_add(outer_commands_visited);
            self.scene
                .frame_stats
                .effect_scene_replay_command_region_tests = self
                .scene
                .frame_stats
                .effect_scene_replay_command_region_tests
                .saturating_add(command_repair_tests);
            self.scene.frame_stats.effect_scene_replay_planned_commands = self
                .scene
                .frame_stats
                .effect_scene_replay_planned_commands
                .saturating_add(planned_commands);
            self.scene.frame_stats.effect_scene_replay_clip_pieces = self
                .scene
                .frame_stats
                .effect_scene_replay_clip_pieces
                .saturating_add(clip_pieces);
        }
        if self.scene.vertices.is_empty() || planned_command_count == 0 {
            if !effect_plan {
                self.scene.frame_stats.base_region_texture_binds = 0;
                self.scene.frame_stats.base_region_draw_calls = 0;
            }
            return Ok(());
        }
        if capture_plan {
            self.scene
                .frame_stats
                .replay_capture_actual_scene_commands_visited = self
                .scene
                .frame_stats
                .replay_capture_actual_scene_commands_visited
                .saturating_add(planned_command_count);
        } else if effect_plan {
            self.scene.frame_stats.effect_scene_replay_commands_visited = self
                .scene
                .frame_stats
                .effect_scene_replay_commands_visited
                .saturating_add(planned_command_count);
        }
        let required_size = self.scene.vertices.len() * std::mem::size_of::<EglTexturedVertex>();
        ensure_vertex_buffer_capacity(
            self.gl,
            self.scene.scene_vertex_buffer,
            &mut self.scene.scene_vertex_buffer_capacity,
            required_size,
        );
        let mut uploaded = false;
        if self.scene.scene_geometry_dirty {
            unsafe {
                self.gl
                    .bind_buffer(glow::ARRAY_BUFFER, Some(self.scene.scene_vertex_buffer));
                self.gl.buffer_sub_data_u8_slice(
                    glow::ARRAY_BUFFER,
                    0,
                    bytemuck::cast_slice(self.scene.vertices.as_slice()),
                );
            }
            self.scene.scene_geometry_dirty = false;
            uploaded = true;
        }
        unsafe {
            self.gl
                .bind_vertex_array(Some(self.scene.scene_vertex_array));
        }

        let mut commands_considered = 0usize;
        let mut commands_executed = 0usize;
        let mut commands_rejected_outside_damage = 0usize;
        let mut missing_required_decoration_resources = 0usize;
        let mut texture_binds = 0usize;
        let mut draw_calls = 0usize;
        for planned_index in 0..planned_command_count {
            let planned = if effect_plan {
                self.scene.effect_region_plan.commands[planned_index]
            } else {
                self.scene.scene_region_plan.commands[planned_index]
            };
            let command = self.scene.commands[planned.command_index].clone();
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
            let output_clip = match presentation_clip {
                Some(clip) => match output_rect_for_egl_clip(clip) {
                    Some(clip) => Some(clip),
                    None => continue,
                },
                None => None,
            };
            let has_active_clip = (0..planned.clips_len).any(|clip_offset| {
                let clip_index = planned.clips_start + clip_offset;
                let repair = if effect_plan {
                    let repair_index =
                        self.scene.effect_region_plan.clip_repair_indices[clip_index];
                    self.scene.effect_region_plan.repairs[repair_index]
                } else {
                    let repair_index = self.scene.scene_region_plan.clip_repair_indices[clip_index];
                    self.scene.scene_region_plan.repairs[repair_index]
                };
                output_clip
                    .map_or(Some(repair), |clip| intersect_output_rect(repair, clip))
                    .is_some_and(|clip| command.bounds.intersects_output_rect(clip))
            });
            if !has_active_clip {
                commands_rejected_outside_damage += planned.clips_len;
                continue;
            }
            if let EglDrawLayer::LifecycleOverlay(identity) = command.layer {
                for clip_offset in 0..planned.clips_len {
                    let clip_index = planned.clips_start + clip_offset;
                    let repair = if effect_plan {
                        let repair_index =
                            self.scene.effect_region_plan.clip_repair_indices[clip_index];
                        self.scene.effect_region_plan.repairs[repair_index]
                    } else {
                        let repair_index =
                            self.scene.scene_region_plan.clip_repair_indices[clip_index];
                        self.scene.scene_region_plan.repairs[repair_index]
                    };
                    let effective_scissor = match output_clip {
                        Some(clip) => intersect_output_rect(repair, clip),
                        None => Some(repair),
                    };
                    let Some(effective_scissor) = effective_scissor else {
                        continue;
                    };
                    self.set_scissor_for_target_space(Some(effective_scissor), target_space);
                    if command.bounds.intersects_output_rect(effective_scissor) {
                        self.dispatch_lifecycle_overlay(
                            identity,
                            Some(effective_scissor),
                            target_space,
                        )?;
                    }
                }
                continue;
            }
            let Some(texture) = self.texture_for_layer(command.layer) else {
                if Self::is_required_decoration_layer(command.layer) {
                    missing_required_decoration_resources += 1;
                }
                continue;
            };

            unsafe {
                self.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                texture_binds += 1;
                let filter = match command.sampling {
                    SurfaceSampling::ExactNearest => glow::NEAREST,
                    SurfaceSampling::ScaledLinear => glow::LINEAR,
                } as i32;
                // Texture min/mag filters belong to the bound texture, so set
                // them for every command binding.
                self.gl
                    .tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, filter);
                self.gl
                    .tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, filter);
                if let Some(location) = opacity_location.as_ref() {
                    let presentation_opacity =
                        if self.scene.capture_unattenuated_visual_group == command.visual_group {
                            1.0
                        } else {
                            self.scene
                                .presentation_opacities
                                .get(planned.command_index)
                                .copied()
                                .unwrap_or(1.0)
                        };
                    self.gl.uniform_1_f32(Some(location), presentation_opacity);
                }
            }

            for clip_offset in 0..planned.clips_len {
                let clip_index = planned.clips_start + clip_offset;
                let repair = if effect_plan {
                    let repair_index =
                        self.scene.effect_region_plan.clip_repair_indices[clip_index];
                    self.scene.effect_region_plan.repairs[repair_index]
                } else {
                    let repair_index = self.scene.scene_region_plan.clip_repair_indices[clip_index];
                    self.scene.scene_region_plan.repairs[repair_index]
                };
                let effective_scissor = match output_clip {
                    Some(clip) => intersect_output_rect(repair, clip),
                    None => Some(repair),
                };
                let Some(effective_scissor) = effective_scissor else {
                    continue;
                };
                self.set_scissor_for_target_space(Some(effective_scissor), target_space);
                if !command.bounds.intersects_output_rect(effective_scissor) {
                    commands_rejected_outside_damage += 1;
                    continue;
                }
                unsafe {
                    self.gl.draw_arrays(
                        glow::TRIANGLES,
                        command.vertex_start as i32,
                        command.vertex_count as i32,
                    );
                }
                commands_executed += 1;
                draw_calls += 1;
            }
        }
        self.set_scissor_for_target_space(None, target_space);
        unsafe {
            self.gl.bind_texture(glow::TEXTURE_2D, None);
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
        self.scene.frame_stats.commands_rejected_outside_damage = self
            .scene
            .frame_stats
            .commands_rejected_outside_damage
            .saturating_add(commands_rejected_outside_damage);
        self.scene.frame_stats.missing_required_decoration_resources = self
            .scene
            .frame_stats
            .missing_required_decoration_resources
            .saturating_add(missing_required_decoration_resources);
        self.scene.frame_stats.texture_binds = self
            .scene
            .frame_stats
            .texture_binds
            .saturating_add(texture_binds);
        if capture_plan {
            self.scene.frame_stats.replay_capture_actual_texture_binds = self
                .scene
                .frame_stats
                .replay_capture_actual_texture_binds
                .saturating_add(texture_binds);
            self.scene.frame_stats.replay_capture_actual_draw_calls = self
                .scene
                .frame_stats
                .replay_capture_actual_draw_calls
                .saturating_add(draw_calls);
        } else if effect_plan {
            self.scene.frame_stats.effect_scene_replay_texture_binds = self
                .scene
                .frame_stats
                .effect_scene_replay_texture_binds
                .saturating_add(texture_binds);
            self.scene.frame_stats.effect_scene_replay_draw_calls = self
                .scene
                .frame_stats
                .effect_scene_replay_draw_calls
                .saturating_add(draw_calls);
        } else {
            self.scene.frame_stats.base_region_texture_binds = texture_binds;
            self.scene.frame_stats.base_region_draw_calls = draw_calls;
        }
        self.scene.frame_stats.draw_calls =
            self.scene.frame_stats.draw_calls.saturating_add(draw_calls);
        self.scene.frame_stats.draw_command_replays = self
            .scene
            .frame_stats
            .draw_command_replays
            .saturating_add(commands_executed);
        if uploaded {
            self.scene.frame_stats.scene_vbo_uploads =
                self.scene.frame_stats.scene_vbo_uploads.saturating_add(1);
            self.scene.frame_stats.scene_vbo_upload_bytes = self
                .scene
                .frame_stats
                .scene_vbo_upload_bytes
                .saturating_add(required_size);
        }
        Ok(())
    }

    fn set_scene_scissor_with_origin(
        &self,
        scissor: Option<OutputRect>,
        framebuffer_origin: OutputFramebufferOrigin,
    ) {
        unsafe {
            if let Some(rect) = scissor {
                let y = match framebuffer_origin {
                    OutputFramebufferOrigin::BottomLeft => self
                        .scene
                        .current_size
                        .1
                        .saturating_sub(rect.y.max(0) as u32 + rect.height)
                        as i32,
                    OutputFramebufferOrigin::TopLeftScanout => rect.y,
                };
                self.gl.enable(glow::SCISSOR_TEST);
                self.gl
                    .scissor(rect.x, y, rect.width as i32, rect.height as i32);
            } else {
                self.gl.disable(glow::SCISSOR_TEST);
            }
        }
    }

    fn set_scissor_for_target_space(
        &self,
        output_scissor: Option<OutputRect>,
        target_space: crate::egl_renderer::lifecycle::LifecycleRenderTargetSpace,
    ) -> bool {
        match (output_scissor, target_space) {
            (
                Some(output_scissor),
                crate::egl_renderer::lifecycle::LifecycleRenderTargetSpace::Output {
                    framebuffer_origin,
                    ..
                },
            ) => {
                self.set_scene_scissor_with_origin(Some(output_scissor), framebuffer_origin);
                true
            }
            (
                Some(output_scissor),
                crate::egl_renderer::lifecycle::LifecycleRenderTargetSpace::Capture {
                    capture_domain,
                    target_size,
                    ..
                },
            ) => self.set_capture_target_scissor(output_scissor, capture_domain, target_size),
            (None, target_space) => {
                self.set_scene_scissor_with_origin(None, target_space.framebuffer_origin());
                false
            }
        }
    }

    fn set_capture_target_scissor(
        &self,
        output_rect: OutputRect,
        capture_domain: OutputRect,
        target_size: (u32, u32),
    ) -> bool {
        let Some(scissor) =
            capture_target_gl_scissor_for_output_clip(output_rect, capture_domain, target_size)
        else {
            unsafe { self.gl.disable(glow::SCISSOR_TEST) };
            return false;
        };
        unsafe {
            self.gl.enable(glow::SCISSOR_TEST);
            self.gl
                .scissor(scissor.x, scissor.y, scissor.width, scissor.height);
        }
        true
    }

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
        let force_legacy = crate::egl_renderer::geometry::scene_execution_mode()
            == crate::egl_renderer::geometry::SceneExecutionMode::Legacy;
        let (plan_fallback, plan_executable, attempted_commands_visited, attempted_region_tests) =
            if force_legacy {
                (true, false, 0, 0)
            } else {
                self.scene.effect_region_plan.build_range(
                    &self.scene.commands,
                    rects,
                    start,
                    end,
                    OcclusionPolicy::Disabled,
                );
                let plan = &self.scene.effect_region_plan;
                self.scene.frame_stats.effect_scene_replay_plan_builds = self
                    .scene
                    .frame_stats
                    .effect_scene_replay_plan_builds
                    .saturating_add(1);
                self.scene.frame_stats.effect_scene_replay_input_rects = self
                    .scene
                    .frame_stats
                    .effect_scene_replay_input_rects
                    .saturating_add(plan.stats.input_rects);
                self.scene.frame_stats.effect_scene_replay_normalized_rects = self
                    .scene
                    .frame_stats
                    .effect_scene_replay_normalized_rects
                    .saturating_add(plan.stats.normalized_rects);
                (
                    plan.fallback.is_some(),
                    plan.is_executable_for(&self.scene.commands),
                    plan.stats.outer_commands_visited,
                    plan.stats.command_repair_tests,
                )
            };
        if !plan_fallback && plan_executable {
            self.establish_effect_composition_state(composition_draw);
            return self.draw_effect_region_plan(framebuffer_origin);
        }
        self.scene.frame_stats.effect_scene_replay_legacy_fallbacks = self
            .scene
            .frame_stats
            .effect_scene_replay_legacy_fallbacks
            .saturating_add(1);
        let texture_binds_before = self.scene.frame_stats.texture_binds;
        let draw_calls_before = self.scene.frame_stats.draw_calls;
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
        let legacy_scan_pairs = self.scene.commands.len().saturating_mul(rects.len());
        let command_region_tests = end.saturating_sub(start).saturating_mul(rects.len());
        self.scene.frame_stats.effect_scene_replay_commands_visited = self
            .scene
            .frame_stats
            .effect_scene_replay_commands_visited
            .saturating_add(attempted_commands_visited)
            .saturating_add(legacy_scan_pairs);
        self.scene
            .frame_stats
            .effect_scene_replay_command_region_tests = self
            .scene
            .frame_stats
            .effect_scene_replay_command_region_tests
            .saturating_add(attempted_region_tests)
            .saturating_add(command_region_tests);
        self.scene.frame_stats.effect_scene_replay_texture_binds = self
            .scene
            .frame_stats
            .effect_scene_replay_texture_binds
            .saturating_add(
                self.scene
                    .frame_stats
                    .texture_binds
                    .saturating_sub(texture_binds_before),
            );
        self.scene.frame_stats.effect_scene_replay_draw_calls = self
            .scene
            .frame_stats
            .effect_scene_replay_draw_calls
            .saturating_add(
                self.scene
                    .frame_stats
                    .draw_calls
                    .saturating_sub(draw_calls_before),
            );
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
        self.draw_command_batch_with_visibility_and_range_in_target_space(
            scene,
            scissor,
            plan_scene_visibility,
            command_range,
            use_visibility_plan,
            self.output_target_space,
        )
    }

    fn draw_command_batch_with_visibility_and_range_in_target_space(
        &mut self,
        scene: bool,
        scissor: Option<OutputRect>,
        plan_scene_visibility: bool,
        command_range: Option<(usize, usize)>,
        use_visibility_plan: bool,
        target_space: crate::egl_renderer::lifecycle::LifecycleRenderTargetSpace,
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
        let (vertices_len, commands_len) = if scene {
            (self.scene.vertices.len(), self.scene.commands.len())
        } else {
            (
                self.scene.cursor_vertices.len(),
                self.scene.cursor_commands.len(),
            )
        };
        if vertices_len == 0 || commands_len == 0 {
            return Ok(());
        }

        let required_size = vertices_len * std::mem::size_of::<EglTexturedVertex>();
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
                        bytemuck::cast_slice(self.scene.vertices.as_slice()),
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
                        bytemuck::cast_slice(self.scene.cursor_vertices.as_slice()),
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

        let initial_scissor = scissor;
        let mut current_scissor = scissor;
        let mut commands_considered = 0;
        let mut commands_executed = 0;
        let mut commands_rejected_outside_damage = 0;
        let mut missing_required_decoration_resources: usize = 0;
        let mut texture_binds = 0;
        let mut draw_calls = 0;
        for command_index in 0..commands_len {
            if command_range
                .is_some_and(|(start, end)| command_index < start || command_index >= end)
            {
                continue;
            }
            let Some(command) = (if scene {
                self.scene.commands.get(command_index)
            } else {
                self.scene.cursor_commands.get(command_index)
            })
            .cloned() else {
                continue;
            };
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
                self.set_scissor_for_target_space(effective_scissor, target_space);
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
            if let EglDrawLayer::LifecycleOverlay(identity) = command.layer {
                self.dispatch_lifecycle_overlay(identity, effective_scissor, target_space)?;
                continue;
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
                let filter = match command.sampling {
                    SurfaceSampling::ExactNearest => glow::NEAREST,
                    SurfaceSampling::ScaledLinear => glow::LINEAR,
                } as i32;
                self.gl
                    .tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, filter);
                self.gl
                    .tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, filter);
                if let Some(location) = opacity_location.as_ref() {
                    let presentation_opacity =
                        if self.scene.capture_unattenuated_visual_group == command.visual_group {
                            1.0
                        } else {
                            if scene {
                                self.scene.presentation_opacities.get(command_index)
                            } else {
                                self.scene.cursor_presentation_opacities.get(command_index)
                            }
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
            self.set_scissor_for_target_space(initial_scissor, target_space);
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

    pub(super) fn draw_capture_commands(
        &mut self,
        command_indices: &[usize],
        output_rect: OutputRect,
        target_domain: EffectRect,
        target_size: (u32, u32),
        framebuffer_origin: OutputFramebufferOrigin,
        host_timing_enabled: bool,
    ) -> RendererResult<ReplayCaptureExecutionDetail> {
        let target_space = crate::egl_renderer::lifecycle::LifecycleRenderTargetSpace::capture(
            self.output_target_space.output_size(),
            OutputRect::new(
                target_domain.x,
                target_domain.y,
                target_domain.width,
                target_domain.height,
            ),
            target_size,
            framebuffer_origin,
        );
        if !self.set_scissor_for_target_space(Some(output_rect), target_space) {
            return Ok(ReplayCaptureExecutionDetail::default());
        }
        let saved_visibility = std::mem::take(&mut self.scene.scene_visibility_plan);
        let repair = EglRect::new(
            output_rect.x as f32,
            output_rect.y as f32,
            output_rect.width as f32,
            output_rect.height as f32,
        );
        let visibility_start = host_timing_enabled.then(Instant::now);
        let capture_stats = plan_capture_visibility(
            &self.scene.commands,
            command_indices,
            repair,
            &self.scene.capture_unclipped_visual_groups,
            &mut self.scene.scene_visibility_plan,
        );
        let visibility_cpu_ns = monotonic_elapsed_ns(visibility_start);
        let commands_considered_before = self.scene.frame_stats.commands_considered;
        let commands_executed_before = self.scene.frame_stats.commands_executed;
        let draw_calls_before = self.scene.frame_stats.draw_calls;
        let draw_submit_start = host_timing_enabled.then(Instant::now);
        let result = self.draw_command_batch_with_visibility_and_range_in_target_space(
            true,
            Some(output_rect),
            false,
            None,
            true,
            target_space,
        );
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

    pub(in crate::egl_renderer) fn draw_capture_commands_for_regions(
        &mut self,
        command_indices: &[usize],
        output_rects: &[OutputRect],
        target_domain: EffectRect,
        target_size: (u32, u32),
        framebuffer_origin: OutputFramebufferOrigin,
        host_timing_enabled: bool,
    ) -> RendererResult<ReplayCaptureExecutionDetail> {
        let layout = replay_capture_region_layout(output_rects);
        self.scene.capture_execution_rects.clear();
        self.scene
            .capture_execution_rects
            .reserve(layout.execution_regions);
        for rect in layout.execution_region.rects() {
            self.scene.capture_execution_rects.push(OutputRect::new(
                rect.x,
                rect.y,
                rect.width,
                rect.height,
            ));
        }
        self.scene.capture_unclipped_visual_groups.clear();
        if let Some(owner) = self.scene.capture_unclipped_presentation_owner {
            self.scene.capture_unclipped_visual_groups.extend(
                self.scene
                    .presentation_visual_group_owners
                    .iter()
                    .filter_map(|(group, command_owner)| {
                        (*command_owner == owner).then_some(*group)
                    }),
            );
        }
        let visibility_start = host_timing_enabled.then(Instant::now);
        let force_legacy = crate::egl_renderer::geometry::scene_execution_mode()
            == crate::egl_renderer::geometry::SceneExecutionMode::Legacy;
        let (
            plan_fallback,
            plan_executable,
            actual_region_tests,
            drawable_pairs,
            plan_commands_visited,
        ) = if force_legacy {
            (true, false, 0, 0, 0)
        } else {
            self.scene.effect_region_plan.build_capture_selected(
                &self.scene.commands,
                &self.scene.capture_execution_rects,
                command_indices,
                &self.scene.capture_unclipped_visual_groups,
            );
            let plan = &self.scene.effect_region_plan;
            self.scene.frame_stats.replay_capture_region_plan_builds = self
                .scene
                .frame_stats
                .replay_capture_region_plan_builds
                .saturating_add(1);
            self.scene.frame_stats.replay_capture_region_input_rects = self
                .scene
                .frame_stats
                .replay_capture_region_input_rects
                .saturating_add(plan.stats.input_rects);
            self.scene
                .frame_stats
                .replay_capture_region_normalized_rects = self
                .scene
                .frame_stats
                .replay_capture_region_normalized_rects
                .saturating_add(plan.stats.normalized_rects);
            (
                plan.fallback.is_some(),
                plan.is_executable_for(&self.scene.commands),
                plan.stats.command_repair_tests,
                plan.stats
                    .repairs
                    .iter()
                    .map(|repair| repair.commands_drawable)
                    .sum::<usize>(),
                plan.stats.outer_commands_visited,
            )
        };
        let visibility_cpu_ns = monotonic_elapsed_ns(visibility_start);
        let mut detail = ReplayCaptureExecutionDetail {
            execution_pixels: 0,
            materialization_rects: layout.materialization_rects,
            execution_regions: layout.execution_regions,
            disjoint_overflowed: layout.disjoint_overflowed,
            candidate_commands: command_indices.len(),
            scene_commands_total: self.scene.commands.len(),
            command_region_pairs: command_indices
                .len()
                .saturating_mul(layout.execution_regions),
            scene_scan_pairs: self
                .scene
                .commands
                .len()
                .saturating_mul(layout.execution_regions),
            visibility_cpu_ns,
            ..Default::default()
        };
        if !plan_fallback && plan_executable {
            let commands_considered_before = self.scene.frame_stats.commands_considered;
            let commands_executed_before = self.scene.frame_stats.commands_executed;
            let draw_calls_before = self.scene.frame_stats.draw_calls;
            let draw_submit_start = host_timing_enabled.then(Instant::now);
            self.draw_capture_region_plan(framebuffer_origin, target_domain, target_size)?;
            detail.planner_commands_visited = actual_region_tests;
            detail.planner_commands_drawable = drawable_pairs;
            detail.commands_considered = self
                .scene
                .frame_stats
                .commands_considered
                .saturating_sub(commands_considered_before);
            detail.commands_executed = self
                .scene
                .frame_stats
                .commands_executed
                .saturating_sub(commands_executed_before);
            detail.draw_calls = self
                .scene
                .frame_stats
                .draw_calls
                .saturating_sub(draw_calls_before);
            detail.draw_submit_cpu_ns = monotonic_elapsed_ns(draw_submit_start);
            self.scene.frame_stats.planner_commands_visited = self
                .scene
                .frame_stats
                .planner_commands_visited
                .saturating_add(actual_region_tests);
            self.scene.frame_stats.commands_drawable = self
                .scene
                .frame_stats
                .commands_drawable
                .saturating_add(drawable_pairs);
            return Ok(detail);
        }

        self.scene
            .frame_stats
            .replay_capture_region_legacy_fallbacks = self
            .scene
            .frame_stats
            .replay_capture_region_legacy_fallbacks
            .saturating_add(1);
        let texture_binds_before = self.scene.frame_stats.texture_binds;
        for rect_index in 0..self.scene.capture_execution_rects.len() {
            let output_rect = self.scene.capture_execution_rects[rect_index];
            let region_detail = self.draw_capture_commands(
                command_indices,
                output_rect,
                target_domain,
                target_size,
                framebuffer_origin,
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
        self.scene
            .frame_stats
            .replay_capture_actual_scene_commands_visited = self
            .scene
            .frame_stats
            .replay_capture_actual_scene_commands_visited
            .saturating_add(plan_commands_visited)
            .saturating_add(
                command_indices
                    .len()
                    .saturating_mul(layout.execution_regions),
            )
            .saturating_add(
                self.scene
                    .commands
                    .len()
                    .saturating_mul(layout.execution_regions),
            );
        self.scene
            .frame_stats
            .replay_capture_actual_command_region_tests = self
            .scene
            .frame_stats
            .replay_capture_actual_command_region_tests
            .saturating_add(actual_region_tests)
            .saturating_add(detail.planner_commands_visited)
            .saturating_add(
                command_indices
                    .len()
                    .saturating_mul(layout.execution_regions),
            );
        self.scene.frame_stats.replay_capture_actual_texture_binds = self
            .scene
            .frame_stats
            .replay_capture_actual_texture_binds
            .saturating_add(
                self.scene
                    .frame_stats
                    .texture_binds
                    .saturating_sub(texture_binds_before),
            );
        self.scene.frame_stats.replay_capture_actual_draw_calls = self
            .scene
            .frame_stats
            .replay_capture_actual_draw_calls
            .saturating_add(detail.draw_calls);
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

#[cfg(test)]
mod capture_target_scissor_tests {
    use super::*;

    #[test]
    fn capture_target_scissor_maps_origin_and_translated_full_domains() {
        let origin_domain = OutputRect::new(0, 0, 10, 8);
        assert_eq!(
            capture_target_gl_scissor_for_output_clip(origin_domain, origin_domain, (10, 8),),
            Some(CaptureTargetGlScissor {
                x: 0,
                y: 0,
                width: 10,
                height: 8,
            })
        );

        let translated_domain = OutputRect::new(73, 41, 131, 89);
        assert_eq!(
            capture_target_gl_scissor_for_output_clip(
                translated_domain,
                translated_domain,
                (131, 89),
            ),
            Some(CaptureTargetGlScissor {
                x: 0,
                y: 0,
                width: 131,
                height: 89,
            })
        );
    }

    #[test]
    fn capture_target_scissor_maps_each_domain_edge_to_local_gl_coordinates() {
        let domain = OutputRect::new(73, 41, 131, 89);
        let cases = [
            (
                "left edge",
                OutputRect::new(73, 49, 5, 4),
                CaptureTargetGlScissor {
                    x: 0,
                    y: 77,
                    width: 5,
                    height: 4,
                },
            ),
            (
                "right edge",
                OutputRect::new(199, 49, 5, 4),
                CaptureTargetGlScissor {
                    x: 126,
                    y: 77,
                    width: 5,
                    height: 4,
                },
            ),
            (
                "top edge",
                OutputRect::new(77, 41, 5, 1),
                CaptureTargetGlScissor {
                    x: 4,
                    y: 88,
                    width: 5,
                    height: 1,
                },
            ),
            (
                "bottom edge",
                OutputRect::new(77, 129, 5, 1),
                CaptureTargetGlScissor {
                    x: 4,
                    y: 0,
                    width: 5,
                    height: 1,
                },
            ),
        ];

        for (edge, output_clip, expected) in cases {
            assert_eq!(
                capture_target_gl_scissor_for_output_clip(output_clip, domain, (131, 89)),
                Some(expected),
                "unexpected GL scissor at the {edge}"
            );
        }
    }

    #[test]
    fn capture_target_scissor_preserves_half_open_one_pixel_intersections() {
        let domain = OutputRect::new(73, 41, 131, 89);
        assert_eq!(
            capture_target_gl_scissor_for_output_clip(
                OutputRect::new(203, 58, 8, 2),
                domain,
                (131, 89),
            ),
            Some(CaptureTargetGlScissor {
                x: 130,
                y: 70,
                width: 1,
                height: 2,
            })
        );
        assert_eq!(
            capture_target_gl_scissor_for_output_clip(
                OutputRect::new(80, 129, 3, 4),
                domain,
                (131, 89),
            ),
            Some(CaptureTargetGlScissor {
                x: 7,
                y: 0,
                width: 3,
                height: 1,
            })
        );
        assert_eq!(
            capture_target_gl_scissor_for_output_clip(
                OutputRect::new(204, 50, 1, 1),
                domain,
                (131, 89),
            ),
            None,
            "a clip starting at the half-open right edge has no intersection"
        );
    }

    #[test]
    fn capture_target_scissor_converts_the_presentation_clipped_repair() {
        let domain = OutputRect::new(20, 30, 40, 30);
        let repair = OutputRect::new(24, 34, 28, 22);
        let presentation_clip = OutputRect::new(30, 40, 18, 10);
        let effective_clip = intersect_output_rect(repair, presentation_clip)
            .expect("presentation clip intersects the repair");

        assert_eq!(
            capture_target_gl_scissor_for_output_clip(effective_clip, domain, (40, 30)),
            Some(CaptureTargetGlScissor {
                x: 10,
                y: 10,
                width: 18,
                height: 10,
            })
        );
    }
}
