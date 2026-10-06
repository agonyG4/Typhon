use super::super::*;
use super::geometry::{
    ndc_to_output_point, output_point_to_ndc, scaled_presentation_rect,
    surface_root_for_lifecycle_surface,
};
use super::{frame_state::LifecycleFrameState, visual_store::LifecycleVisualStore};

#[derive(Debug, Clone, PartialEq)]
pub(super) struct EglSquashDrawCommand {
    pub(super) command: EglDrawCommand,
    pub(super) presentation_identity:
        oblivion_one::presentation_animation::PresentationRetainedVisualIdentity,
}

pub(super) struct SquashRenderState {
    pub(super) vertices: Vec<EglTexturedVertex>,
    pub(super) commands: Vec<EglSquashDrawCommand>,
    pub(super) vertex_array: GlVertexArray,
    pub(super) vertex_buffer: GlBuffer,
    pub(super) vertex_buffer_capacity: usize,
    pub(super) geometry_dirty: bool,
}

fn is_required_decoration_layer(layer: EglDrawLayer) -> bool {
    matches!(
        layer,
        EglDrawLayer::SolidRgba(_) | EglDrawLayer::DecorationAsset(_)
    )
}

#[cfg(test)]
mod tests;

impl SquashRenderState {
    pub(super) fn new(
        vertex_array: GlVertexArray,
        vertex_buffer: GlBuffer,
        vertex_buffer_capacity: usize,
    ) -> Self {
        Self {
            vertices: Vec::new(),
            commands: Vec::new(),
            vertex_array,
            vertex_buffer,
            vertex_buffer_capacity,
            geometry_dirty: true,
        }
    }

    pub(super) fn destroy_gl_resources(&mut self, gl: &glow::Context) {
        unsafe {
            gl.delete_buffer(self.vertex_buffer);
            gl.delete_vertex_array(self.vertex_array);
        }
    }
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

impl SquashRenderState {
    pub(super) fn rebuild_commands(
        &mut self,
        lifecycle: &LifecycleSceneSample,
        lifecycle_surfaces: &[RenderableSurface],
        lifecycle_decorations: &[DecorationRenderInstance],
        output_scale: f64,
        output_size: (u32, u32),
        framebuffer_origin: OutputFramebufferOrigin,
        frame: &mut LifecycleFrameState,
    ) {
        self.vertices.clear();
        self.commands.clear();
        self.geometry_dirty = true;
        let assignments =
            compositor::surface_render_space_assignments(lifecycle_surfaces, output_scale);
        for sample in lifecycle
            .samples
            .iter()
            .filter(|sample| sample.effect == LifecycleEffectKind::Squash)
        {
            let initial_command_count = self.commands.len();
            if sample.visual_source.root_surface_id != sample.root_surface_id
                || sample.visual_source.payload_id != sample.payload_id
                || sample.visual_source.presentation_identity != sample.presentation_identity
            {
                frame.record_fallback(
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
                    frame.record_fallback(
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
                    output_size.0,
                    output_size.1,
                    framebuffer_origin,
                );
                self.append_batch(sample.presentation_identity, vertices, commands);
            } else {
                for (surface, assignment) in lifecycle_surfaces.iter().zip(assignments.iter()) {
                    if surface_root_for_lifecycle_surface(surface, lifecycle_surfaces, lifecycle)
                        != sample.root_surface_id
                    {
                        continue;
                    }
                    let mut vertices = Vec::new();
                    let mut commands = Vec::new();
                    push_egl_surface_commands(
                        &mut vertices,
                        &mut commands,
                        output_size.0,
                        output_size.1,
                        surface,
                        assignment.clone(),
                        framebuffer_origin,
                        None,
                    );
                    if transform_squash_geometry(
                        &mut vertices,
                        &mut commands,
                        sample.visual_group,
                        sample.progress,
                        output_scale,
                        output_size,
                        framebuffer_origin,
                    ) {
                        self.append_batch(sample.presentation_identity, vertices, commands);
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
                        output_size.0,
                        output_size.1,
                        decoration,
                        output_scale,
                        framebuffer_origin,
                        None,
                    );
                    if transform_squash_geometry(
                        &mut vertices,
                        &mut commands,
                        sample.visual_group,
                        sample.progress,
                        output_scale,
                        output_size,
                        framebuffer_origin,
                    ) {
                        self.append_batch(sample.presentation_identity, vertices, commands);
                    }
                }
            }
            if self.commands.len() == initial_command_count {
                frame.record_fallback(
                    frame_sample,
                    LifecycleRenderFallbackReason::NoConsumedRepresentation,
                );
            }
        }
    }

    fn append_batch(
        &mut self,
        presentation_identity: oblivion_one::presentation_animation::PresentationRetainedVisualIdentity,
        vertices: Vec<EglTexturedVertex>,
        commands: Vec<EglDrawCommand>,
    ) {
        let vertex_offset = u32::try_from(self.vertices.len()).unwrap_or(u32::MAX);
        self.vertices.extend(vertices);
        self.commands
            .extend(commands.into_iter().map(|mut command| {
                command.vertex_start = command.vertex_start.saturating_add(vertex_offset);
                EglSquashDrawCommand {
                    command,
                    presentation_identity,
                }
            }));
    }

    pub(super) fn draw_overlay(
        &mut self,
        frame: &mut LifecycleFrameState,
        visual_store: &LifecycleVisualStore,
        context: &mut LifecycleRenderContext<'_>,
        scissor: Option<OutputRect>,
    ) -> RendererResult<()> {
        if self.vertices.is_empty() || self.commands.is_empty() {
            return Ok(());
        }
        let required_size = self.vertices.len() * std::mem::size_of::<EglTexturedVertex>();
        ensure_vertex_buffer_capacity(
            &context.gl,
            self.vertex_buffer,
            &mut self.vertex_buffer_capacity,
            required_size,
        );
        if self.geometry_dirty {
            unsafe {
                context
                    .gl
                    .bind_buffer(glow::ARRAY_BUFFER, Some(self.vertex_buffer));
                context.gl.buffer_sub_data_u8_slice(
                    glow::ARRAY_BUFFER,
                    0,
                    bytemuck::cast_slice(self.vertices.as_slice()),
                );
            }
            self.geometry_dirty = false;
        }
        let active = frame
            .samples
            .iter()
            .filter(|sample| sample.effect == LifecycleEffectKind::Squash)
            .copied()
            .collect::<Vec<_>>();
        let mut draw_calls = 0_usize;
        let mut texture_binds = 0_usize;
        let mut missing_required_decoration_resources = 0_usize;
        unsafe {
            context.gl.use_program(Some(context.scene_state.program));
            context.gl.bind_vertex_array(Some(self.vertex_array));
            context.gl.active_texture(glow::TEXTURE0);
            context.gl.enable(glow::BLEND);
            context.gl.blend_func_separate(
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
            );
        }
        let mut current_sampling = None;
        for sample in active {
            let commands = self
                .commands
                .iter()
                .filter(|entry| entry.presentation_identity == sample.presentation_identity)
                .filter(|entry| {
                    scissor.is_none_or(|rect| entry.command.bounds.intersects_output_rect(rect))
                })
                .collect::<Vec<_>>();
            if commands.is_empty() {
                continue;
            }
            if commands.iter().any(|entry| {
                context
                    .texture_for_layer(entry.command.layer, &visual_store.resolved_visual_resources)
                    .is_none()
            }) {
                if commands
                    .iter()
                    .any(|entry| is_required_decoration_layer(entry.command.layer))
                {
                    missing_required_decoration_resources =
                        missing_required_decoration_resources.saturating_add(1);
                }
                frame.record_fallback(
                    sample,
                    LifecycleRenderFallbackReason::LifecycleResourceUnavailable,
                );
                continue;
            }
            for entry in commands {
                let command = &entry.command;
                let Some(texture) = context
                    .texture_for_layer(command.layer, &visual_store.resolved_visual_resources)
                else {
                    continue;
                };
                unsafe {
                    context.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                    texture_binds = texture_binds.saturating_add(1);
                    if current_sampling != Some(command.sampling) {
                        let filter = match command.sampling {
                            SurfaceSampling::ExactNearest => glow::NEAREST,
                            SurfaceSampling::ScaledLinear => glow::LINEAR,
                        } as i32;
                        context.gl.tex_parameter_i32(
                            glow::TEXTURE_2D,
                            glow::TEXTURE_MIN_FILTER,
                            filter,
                        );
                        context.gl.tex_parameter_i32(
                            glow::TEXTURE_2D,
                            glow::TEXTURE_MAG_FILTER,
                            filter,
                        );
                        current_sampling = Some(command.sampling);
                    }
                    if let Some(location) = &context.scene_state.presentation_opacity_location {
                        context
                            .gl
                            .uniform_1_f32(Some(location), sample.effect_opacity as f32);
                    }
                    context.gl.draw_arrays(
                        glow::TRIANGLES,
                        command.vertex_start as i32,
                        command.vertex_count as i32,
                    );
                }
                draw_calls = draw_calls.saturating_add(1);
                frame.evidence.record(LifecycleRenderEvidenceEntry {
                    window_id: sample.window_id,
                    root_surface_id: sample.root_surface_id,
                    presentation_identity: sample.presentation_identity,
                    payload_id: sample.payload_id,
                });
            }
        }
        context.scene_state.frame_stats.draw_calls = context
            .scene_state
            .frame_stats
            .draw_calls
            .saturating_add(draw_calls);
        context.scene_state.frame_stats.commands_executed = context
            .scene_state
            .frame_stats
            .commands_executed
            .saturating_add(draw_calls);
        context.scene_state.frame_stats.texture_binds = context
            .scene_state
            .frame_stats
            .texture_binds
            .saturating_add(texture_binds);
        context
            .scene_state
            .frame_stats
            .missing_required_decoration_resources = context
            .scene_state
            .frame_stats
            .missing_required_decoration_resources
            .saturating_add(missing_required_decoration_resources);
        unsafe {
            context.gl.use_program(Some(context.scene_state.program));
            context
                .gl
                .bind_vertex_array(Some(context.scene_state.scene_vertex_array));
        }
        Ok(())
    }
}
