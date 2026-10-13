use super::super::*;
use super::{frame_state::LifecycleFrameState, visual_store::LifecycleVisualStore};

pub(super) const MAX_LAMP_VERTICES: usize = 65_536;
pub(super) const LAMP_TARGET_CELL_PIXELS: f32 = 32.0;
pub(super) const LAMP_MAX_GRID_SUBDIVISIONS: usize = 64;

#[derive(Debug, Clone, Copy)]
pub(super) struct LampUniformLocations {
    pub(super) output_size: Option<glow::UniformLocation>,
    pub(super) canonical_visual_rect: Option<glow::UniformLocation>,
    pub(super) source_visual_rect: Option<glow::UniformLocation>,
    pub(super) sink_rect: Option<glow::UniformLocation>,
    pub(super) progress: Option<glow::UniformLocation>,
    pub(super) opacity: Option<glow::UniformLocation>,
    pub(super) direction: Option<glow::UniformLocation>,
    pub(super) shape_factor: Option<glow::UniformLocation>,
    pub(super) bump_distance: Option<glow::UniformLocation>,
    pub(super) contraction_progress: Option<glow::UniformLocation>,
    pub(super) translation_progress: Option<glow::UniformLocation>,
    pub(super) retreat_progress: Option<glow::UniformLocation>,
    pub(super) framebuffer_origin_bottom_left: Option<glow::UniformLocation>,
    pub(super) capture_target: Option<glow::UniformLocation>,
    pub(super) capture_domain: Option<glow::UniformLocation>,
    pub(super) texture: Option<glow::UniformLocation>,
}

pub(super) struct LampRenderState {
    pub(super) program: Option<GlProgram>,
    pub(super) uniform_locations: Option<LampUniformLocations>,
    pub(super) vertex_array: GlVertexArray,
    pub(super) vertex_buffer: GlBuffer,
    pub(super) vertex_buffer_capacity: usize,
    pub(super) geometry_dirty: bool,
    pub(super) geometry_key: Option<u64>,
    pub(super) vertices: Vec<EglLampVertex>,
    pub(super) commands: Vec<EglLampDrawCommand>,
}

fn is_required_decoration_layer(layer: EglDrawLayer) -> bool {
    matches!(
        layer,
        EglDrawLayer::SolidRgba(_) | EglDrawLayer::DecorationAsset(_)
    )
}

#[cfg(test)]
mod geometry_tests;
#[cfg(test)]
mod tests;

impl LampUniformLocations {
    pub(super) fn query(gl: &glow::Context, program: GlProgram) -> Self {
        unsafe {
            Self {
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
                capture_target: gl.get_uniform_location(program, "u_capture_target"),
                capture_domain: gl.get_uniform_location(program, "u_capture_domain"),
                texture: gl.get_uniform_location(program, "u_texture"),
            }
        }
    }
}

impl LampRenderState {
    pub(super) fn new(
        program: Option<GlProgram>,
        uniform_locations: Option<LampUniformLocations>,
        vertex_array: GlVertexArray,
        vertex_buffer: GlBuffer,
        vertex_buffer_capacity: usize,
    ) -> Self {
        Self {
            program,
            uniform_locations,
            vertex_array,
            vertex_buffer,
            vertex_buffer_capacity,
            geometry_dirty: true,
            geometry_key: None,
            vertices: Vec::new(),
            commands: Vec::new(),
        }
    }

    pub(super) fn destroy_gl_resources(&mut self, gl: &glow::Context) {
        unsafe {
            if let Some(program) = self.program.take() {
                gl.delete_program(program);
            }
            gl.delete_buffer(self.vertex_buffer);
            gl.delete_vertex_array(self.vertex_array);
        }
    }

    pub(super) fn extend_surface_consumers(
        &self,
        consumer_plan: &mut SurfaceConsumerPlan,
        repairs: &[OutputRect],
    ) {
        consumer_plan.extend(&plan_lamp_surface_consumers(&self.commands, repairs));
    }
}

struct LampGridSpec {
    layer: EglDrawLayer,
    presentation_identity: oblivion_one::presentation_animation::PresentationRetainedVisualIdentity,
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
    _source_kind: LifecycleVisualSourceKind,
) -> bool {
    matches!(layer, EglDrawLayer::LifecycleResolvedVisual(layer_payload_id) if layer_payload_id == payload_id)
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

impl LampRenderState {
    pub(super) fn rebuild_commands(
        &mut self,
        lifecycle: &LifecycleSceneSample,
        lifecycle_surfaces: &[RenderableSurface],
        lifecycle_decorations: &[DecorationRenderInstance],
        output_scale: f64,
        _output_size: (u32, u32),
        framebuffer_origin: OutputFramebufferOrigin,
        frame: &mut LifecycleFrameState,
    ) {
        let geometry_key = lamp_geometry_key(
            lifecycle,
            lifecycle_surfaces,
            lifecycle_decorations,
            output_scale,
            framebuffer_origin,
        );
        if self.geometry_key == Some(geometry_key) {
            return;
        }
        self.geometry_key = Some(geometry_key);
        self.geometry_dirty = true;
        self.vertices.clear();
        self.commands.clear();
        if !lifecycle
            .samples
            .iter()
            .any(|sample| sample.effect == LifecycleEffectKind::Lamp)
        {
            return;
        }
        let mut transition_starts = HashMap::new();
        let mut rejected_transitions = HashSet::new();
        for lamp in LifecycleFrameSnapshot::from_sample(lifecycle)
            .samples
            .into_iter()
            .filter(|sample| sample.effect == LifecycleEffectKind::Lamp)
        {
            let Some(source) = lifecycle_visual_source_for_lamp(lifecycle, lamp) else {
                frame.record_fallback(
                    lamp,
                    LifecycleRenderFallbackReason::NoConsumedRepresentation,
                );
                continue;
            };
            if source.root_surface_id != lamp.root_surface_id
                || source.payload_id != lamp.payload_id
            {
                frame.record_fallback(
                    lamp,
                    LifecycleRenderFallbackReason::NoConsumedRepresentation,
                );
                continue;
            }
            self.append_grid_for_transition(
                frame,
                lamp,
                LampGridSpec {
                    layer: EglDrawLayer::LifecycleResolvedVisual(source.resource_payload_id),
                    presentation_identity: lamp.presentation_identity,
                    bounds: EglRect::new(
                        (lamp.visual_group.canonical_visual_rect.x() * output_scale) as f32,
                        (lamp.visual_group.canonical_visual_rect.y() * output_scale) as f32,
                        (lamp.visual_group.canonical_visual_rect.width() * output_scale) as f32,
                        (lamp.visual_group.canonical_visual_rect.height() * output_scale) as f32,
                    ),
                    uv: EglUvRect::new(0.0, 1.0, 1.0, 0.0),
                },
                &mut transition_starts,
                &mut rejected_transitions,
            );
        }
        for lamp in LifecycleFrameSnapshot::from_sample(lifecycle)
            .samples
            .into_iter()
            .filter(|sample| sample.effect == LifecycleEffectKind::Lamp)
        {
            if !transition_starts.contains_key(&lamp.presentation_identity)
                && !rejected_transitions.contains(&lamp.presentation_identity)
            {
                frame.record_fallback(lamp, LifecycleRenderFallbackReason::MeshBudget);
            }
        }
    }

    fn append_grid_for_transition(
        &mut self,
        frame: &mut LifecycleFrameState,
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
            .or_insert((self.vertices.len(), self.commands.len()));
        if !append_lamp_grid(&mut self.vertices, &mut self.commands, spec) {
            self.vertices.truncate(start.0);
            self.commands.truncate(start.1);
            rejected_transitions.insert(lamp.presentation_identity);
            frame.record_fallback(lamp, LifecycleRenderFallbackReason::MeshBudget);
        }
    }

    pub(super) fn draw_overlay_for_sample(
        &mut self,
        frame: &mut LifecycleFrameState,
        visual_store: &LifecycleVisualStore,
        context: &mut LifecycleRenderContext<'_>,
        scissor: Option<OutputRect>,
        sample: LifecycleFrameSample,
    ) -> RendererResult<()> {
        self.draw_overlay_matching(
            frame,
            visual_store,
            context,
            scissor,
            Some(sample.presentation_identity),
        )
    }

    fn draw_overlay_matching(
        &mut self,
        frame: &mut LifecycleFrameState,
        visual_store: &LifecycleVisualStore,
        context: &mut LifecycleRenderContext<'_>,
        scissor: Option<OutputRect>,
        presentation_identity: Option<
            oblivion_one::presentation_animation::PresentationRetainedVisualIdentity,
        >,
    ) -> RendererResult<()> {
        let (Some(program), Some(uniforms)) = (self.program, self.uniform_locations) else {
            let visible_samples = frame.visible_samples(
                f64::from(context.effect_runtime.effect_output_scale),
                context.target_space.output_size(),
            );
            for sample in visible_samples.into_iter().filter(|sample| {
                sample.effect == LifecycleEffectKind::Lamp
                    && presentation_identity
                        .is_none_or(|identity| sample.presentation_identity == identity)
            }) {
                frame.record_fallback(
                    sample,
                    LifecycleRenderFallbackReason::LampProgramUnavailable,
                );
            }
            return Ok(());
        };
        if self.vertices.is_empty() || self.commands.is_empty() {
            return Ok(());
        }
        let required_size = self.vertices.len() * std::mem::size_of::<EglLampVertex>();
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
        let samples = self
            .commands
            .iter()
            .filter_map(|command| {
                if presentation_identity
                    .is_some_and(|identity| command.presentation_identity != identity)
                {
                    return None;
                }
                let sample = frame.sample_for_identity(command.presentation_identity)?;
                let source = visual_store
                    .visual_sources
                    .get(&command.presentation_identity)?;
                if source.root_surface_id != sample.root_surface_id
                    || source.payload_id != sample.payload_id
                    || !lifecycle_draw_layer_matches_payload(
                        command.layer,
                        source.resource_payload_id,
                        source.kind,
                    )
                {
                    return None;
                }
                Some((*command, sample))
            })
            .collect::<Vec<_>>();
        unsafe {
            context.gl.use_program(Some(program));
            context.gl.bind_vertex_array(Some(self.vertex_array));
            context.gl.active_texture(glow::TEXTURE0);
            context.gl.enable(glow::BLEND);
            context.gl.blend_func_separate(
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
            );
            if let Some(location) = &uniforms.texture {
                context.gl.uniform_1_i32(Some(location), 0);
            }
            if let Some(location) = &uniforms.output_size {
                let output_size = context.target_space.output_size();
                context.gl.uniform_2_f32(
                    Some(location),
                    output_size.0 as f32,
                    output_size.1 as f32,
                );
            }
            if let Some(location) = &uniforms.framebuffer_origin_bottom_left {
                context.gl.uniform_1_i32(
                    Some(location),
                    i32::from(
                        context.target_space.framebuffer_origin()
                            == OutputFramebufferOrigin::BottomLeft,
                    ),
                );
            }
            if let Some(location) = &uniforms.capture_target {
                context.gl.uniform_1_i32(
                    Some(location),
                    i32::from(matches!(
                        context.target_space,
                        super::LifecycleRenderTargetSpace::Capture { .. }
                    )),
                );
            }
            if let (
                Some(location),
                super::LifecycleRenderTargetSpace::Capture { capture_domain, .. },
            ) = (&uniforms.capture_domain, context.target_space)
            {
                context.gl.uniform_4_f32(
                    Some(location),
                    capture_domain.x as f32,
                    capture_domain.y as f32,
                    capture_domain.width.max(1) as f32,
                    capture_domain.height.max(1) as f32,
                );
            }
        }
        let output_scale = context.effect_runtime.effect_output_scale.max(1.0) as f64;
        let mut current_texture_sampling = None;
        let missing_required_decoration_resources = samples
            .iter()
            .filter(|(command, _)| {
                scissor.is_none_or(|rect| command.bounds.intersects_output_rect(rect))
                    && is_required_decoration_layer(command.layer)
                    && context
                        .texture_for_layer(command.layer, &visual_store.resolved_visual_resources)
                        .is_none()
            })
            .count();
        for (command, sample) in samples {
            if scissor.is_some_and(|rect| !command.bounds.intersects_output_rect(rect)) {
                continue;
            }
            let Some(texture) =
                context.texture_for_layer(command.layer, &visual_store.resolved_visual_resources)
            else {
                frame.record_fallback(
                    sample,
                    LifecycleRenderFallbackReason::LifecycleResourceUnavailable,
                );
                continue;
            };
            unsafe {
                context.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                let texture_sampling = (texture, command.sampling);
                if current_texture_sampling != Some(texture_sampling) {
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
                    current_texture_sampling = Some(texture_sampling);
                }
                set_lamp_uniform_rect(
                    &context.gl,
                    uniforms.canonical_visual_rect.as_ref(),
                    sample.visual_group.canonical_visual_rect,
                    output_scale,
                );
                set_lamp_uniform_rect(
                    &context.gl,
                    uniforms.source_visual_rect.as_ref(),
                    sample.visual_group.presented_source_visual_rect,
                    output_scale,
                );
                set_lamp_uniform_rect(
                    &context.gl,
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
                    context.gl.uniform_1_i32(Some(location), direction);
                }
                if let Some(location) = &uniforms.shape_factor {
                    context
                        .gl
                        .uniform_1_f32(Some(location), sample.visual_group.shape_factor as f32);
                }
                if let Some(location) = &uniforms.bump_distance {
                    context.gl.uniform_1_f32(
                        Some(location),
                        (sample.visual_group.bump_distance * output_scale) as f32,
                    );
                }
                let channels =
                    lamp_motion_channels(sample.progress, sample.visual_group.bump_distance);
                if let Some(location) = &uniforms.contraction_progress {
                    context
                        .gl
                        .uniform_1_f32(Some(location), channels.contraction_progress as f32);
                }
                if let Some(location) = &uniforms.translation_progress {
                    context
                        .gl
                        .uniform_1_f32(Some(location), channels.translation_progress as f32);
                }
                if let Some(location) = &uniforms.retreat_progress {
                    context
                        .gl
                        .uniform_1_f32(Some(location), channels.retreat_progress as f32);
                }
                if let Some(location) = &uniforms.progress {
                    context
                        .gl
                        .uniform_1_f32(Some(location), channels.temporal_progress as f32);
                }
                if let Some(location) = &uniforms.opacity {
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
            frame.evidence.record(LifecycleRenderEvidenceEntry {
                window_id: sample.window_id,
                root_surface_id: sample.root_surface_id,
                presentation_identity: sample.presentation_identity,
                payload_id: sample.payload_id,
            });
        }
        context
            .scene_state
            .frame_stats
            .missing_required_decoration_resources = context
            .scene_state
            .frame_stats
            .missing_required_decoration_resources
            .saturating_add(missing_required_decoration_resources);
        Ok(())
    }
}
