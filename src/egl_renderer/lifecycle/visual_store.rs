use super::super::*;
use super::frame_state::LifecycleFrameState;
use super::geometry::scaled_presentation_rect;

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
pub(super) struct LifecycleSourceCaptureFailure {
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

pub(in crate::egl_renderer) struct LifecycleResolvedVisualResource {
    pub(super) texture: PooledEffectTexture,
    pub(super) source_signature: u64,
    pub(super) source_visual_rect: compositor::PresentationRect,
}

impl LifecycleResolvedVisualResource {
    pub(in crate::egl_renderer) fn pooled_texture(&self) -> &PooledEffectTexture {
        &self.texture
    }
}

#[derive(Default)]
pub(super) struct LifecycleVisualStore {
    pub(super) source_vertices:
        HashMap<compositor::PresentationRetainedVisualPayloadId, Vec<EglTexturedVertex>>,
    pub(super) source_commands:
        HashMap<compositor::PresentationRetainedVisualPayloadId, Vec<EglDrawCommand>>,
    pub(super) resolved_visual_resources:
        HashMap<compositor::PresentationRetainedVisualPayloadId, LifecycleResolvedVisualResource>,
    pub(super) visual_sources: HashMap<
        oblivion_one::presentation_animation::PresentationRetainedVisualIdentity,
        LifecycleVisualSource,
    >,
}

impl LifecycleVisualStore {
    pub(super) fn is_ready(
        &self,
        payload_id: compositor::PresentationRetainedVisualPayloadId,
        runtime: &EffectRuntime,
    ) -> bool {
        self.resolved_visual_resources
            .get(&payload_id)
            .is_some_and(|resource| {
                runtime
                    .effect_resources
                    .texture(&resource.texture)
                    .is_some()
            })
    }

    pub(super) fn begin_frame(&mut self, snapshot: &LifecycleSceneSample) {
        self.visual_sources.clear();
        self.visual_sources.extend(
            snapshot
                .samples
                .iter()
                .map(|sample| sample.visual_source.clone())
                .map(|source| (source.presentation_identity, source)),
        );
    }

    pub(super) fn extend_surface_consumers(
        &self,
        consumer_plan: &mut SurfaceConsumerPlan,
        repairs: &[OutputRect],
    ) {
        for commands in self.source_commands.values() {
            consumer_plan.extend(&plan_surface_consumers(commands, repairs));
        }
    }

    pub(super) fn release_stale(&mut self, runtime: &mut EffectRuntime) {
        let live_payload_ids = self
            .visual_sources
            .values()
            .map(|source| source.payload_id)
            .collect::<HashSet<_>>();
        let stale = self
            .resolved_visual_resources
            .keys()
            .copied()
            .filter(|payload_id| !live_payload_ids.contains(payload_id))
            .collect::<Vec<_>>();
        for payload_id in stale {
            if let Some(resource) = self.resolved_visual_resources.remove(&payload_id) {
                let _ = runtime.effect_resources.release(resource.texture);
            }
        }
        self.source_vertices
            .retain(|payload_id, _| live_payload_ids.contains(payload_id));
        self.source_commands
            .retain(|payload_id, _| live_payload_ids.contains(payload_id));
    }

    pub(super) fn release_all(&mut self, runtime: &mut EffectRuntime) {
        for (_, resource) in self.resolved_visual_resources.drain() {
            let _ = runtime.effect_resources.release(resource.texture);
        }
    }
}

impl LifecycleVisualStore {
    pub(super) fn prepare_lifecycle_visual_sources(
        &mut self,
        frame: &mut LifecycleFrameState,
        context: &mut LifecycleRenderContext<'_>,
        _plan: &RepaintPlan,
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> RendererResult<()> {
        self.release_stale(context.effect_runtime);
        let output_scale = context.effect_runtime.effect_output_scale.max(1.0) as f64;
        let sources = self
            .visual_sources
            .values()
            .filter(|source| source.kind == LifecycleVisualSourceKind::ResolvedOwnedEffects)
            .cloned()
            .collect::<Vec<_>>();

        for source in sources {
            let Some(lamp) = frame
                .samples
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
                frame.record_fallback(
                    lamp,
                    LifecycleRenderFallbackReason::ResolvedSourceAllocation,
                );
                continue;
            };
            let source_signature = lifecycle_visual_source_signature(&source, lamp, output_scale);
            let ready = self
                .resolved_visual_resources
                .get(&source.payload_id)
                .is_some_and(|resource| {
                    resource.source_signature == source_signature
                        && resource.source_visual_rect
                            == lamp.visual_group.presented_source_visual_rect
                        && context
                            .effect_runtime
                            .effect_resources
                            .texture(&resource.texture)
                            .is_some()
                });
            if ready {
                continue;
            }

            if let Some(previous) = self.resolved_visual_resources.remove(&source.payload_id) {
                let _ = context
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
            let texture = match context
                .effect_runtime
                .effect_resources
                .acquire(&context.gl, texture_key)
            {
                Ok(texture) => texture,
                Err(error) => {
                    Self::record_source_capture_failure(
                        context,
                        &LifecycleSourceCaptureFailure::new(
                            LifecycleSourceCaptureFailureStage::TargetAllocation,
                            error,
                        ),
                    );
                    frame.record_fallback(
                        lamp,
                        LifecycleRenderFallbackReason::ResolvedSourceAllocation,
                    );
                    continue;
                }
            };
            if let Err(error) = self.capture_visual_source(
                context,
                &source,
                lamp,
                texture.clone(),
                framebuffer_origin,
            ) {
                Self::record_source_capture_failure(context, &error);
                if let Err(release_error) = context.effect_runtime.effect_resources.release(texture)
                {
                    Self::record_source_capture_failure(
                        context,
                        &LifecycleSourceCaptureFailure::new(
                            LifecycleSourceCaptureFailureStage::TargetRelease,
                            Box::new(release_error),
                        ),
                    );
                }
                frame.record_fallback(lamp, LifecycleRenderFallbackReason::ResolvedSourceCapture);
                continue;
            }
            self.resolved_visual_resources.insert(
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

    pub(super) fn capture_visual_source(
        &self,
        context: &mut LifecycleRenderContext<'_>,
        source: &LifecycleVisualSource,
        lamp: LifecycleFrameSample,
        target: PooledEffectTexture,
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> Result<(), LifecycleSourceCaptureFailure> {
        clear_effect_texture(context, &target).map_err(|error| {
            LifecycleSourceCaptureFailure::new(
                LifecycleSourceCaptureFailureStage::TargetClear,
                error,
            )
        })?;
        let scratch_key = EffectTextureKey::new(
            context.scene_state.current_size.0.max(1),
            context.scene_state.current_size.1.max(1),
            EffectTextureFormat::Rgba8,
            EffectTextureFilter::Linear,
            EffectWorkingSpace::OutputEncodedSrgb,
        );
        let scratch = context
            .effect_runtime
            .effect_resources
            .acquire(&context.gl, scratch_key)
            .map_err(|error| {
                LifecycleSourceCaptureFailure::new(
                    LifecycleSourceCaptureFailureStage::ScratchAllocation,
                    error,
                )
            })?;
        let result = (|| {
            clear_effect_texture(context, &scratch).map_err(|error| {
                LifecycleSourceCaptureFailure::new(
                    LifecycleSourceCaptureFailureStage::ScratchClear,
                    error,
                )
            })?;
            let scratch_framebuffer = context
                .effect_runtime
                .effect_resources
                .bind_lifecycle_composition_target(&context.gl, &scratch)
                .map_err(|error| {
                    LifecycleSourceCaptureFailure::new(
                        LifecycleSourceCaptureFailureStage::ScratchTarget,
                        error,
                    )
                })?;
            let targets = EffectExecutionTargets {
                baseline_read: EffectFramebufferTarget::new(
                    context.scene_state.active_output_framebuffer,
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
                .source_vertices
                .get(&source.payload_id)
                .cloned()
                .unwrap_or_default();
            let source_commands = self
                .source_commands
                .get(&source.payload_id)
                .cloned()
                .unwrap_or_default();
            if source_vertices.is_empty() || source_commands.is_empty() {
                return Err(LifecycleSourceCaptureFailure::new(
                    LifecycleSourceCaptureFailureStage::MissingSourceCommands,
                    io::Error::other("lifecycle visual source has no draw commands"),
                ));
            }

            let saved_vertices =
                std::mem::replace(&mut context.scene_state.vertices, source_vertices);
            let saved_commands =
                std::mem::replace(&mut context.scene_state.commands, source_commands);
            context.scene_state.scene_geometry_dirty = true;
            let draw_result = (|| {
                let source_damage =
                    lifecycle_visual_effect_damage(lamp.visual_group.presented_source_visual_rect);
                let output_bounds = EffectRect::new(
                    0,
                    0,
                    context.scene_state.current_size.0.max(1),
                    context.scene_state.current_size.1.max(1),
                )
                .expect("non-zero renderer dimensions must form valid effect bounds");
                let graph = match compile_frame_execution_plan(
                    &source.effect_scene,
                    &source_damage,
                    output_bounds,
                    &context.effect_runtime.effect_registry,
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
                    let mut execution_context =
                        context.effect_execution_context(&self.resolved_visual_resources);
                    effects::execute_effect_graph_for_lifecycle(
                        &mut execution_context,
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
                    context,
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
            context.scene_state.vertices = saved_vertices;
            context.scene_state.commands = saved_commands;
            // The temporary source draw replaced the scene VBO contents.
            // Force the normal scene cache to upload its unchanged geometry
            // before the next ordinary scene draw.
            context.scene_state.scene_geometry_dirty = true;
            draw_result
        })();
        context.establish_ordinary_scene_state();
        let release_result = context
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
                Self::record_source_capture_failure(context, &release_error);
                Err(error)
            }
            (Err(error), Ok(())) => Err(error),
            (Ok(()), Err(error)) => Err(error),
            (Ok(()), Ok(())) => Ok(()),
        }
    }
}

impl LifecycleVisualStore {
    fn record_source_capture_failure(
        context: &mut LifecycleRenderContext<'_>,
        failure: &LifecycleSourceCaptureFailure,
    ) {
        context.effect_runtime.effect_trace.event(|| {
            format!(
                "event=lifecycle_source_capture_failure stage={} error={}",
                failure.stage.as_str(),
                failure.source,
            )
        });
    }
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

fn clear_effect_texture(
    context: &mut LifecycleRenderContext<'_>,
    target: &PooledEffectTexture,
) -> RendererResult<()> {
    let result = (|| {
        context
            .effect_runtime
            .effect_resources
            .bind_render_target(context.gl, target)?;
        unsafe {
            context
                .gl
                .viewport(0, 0, target.key.width as i32, target.key.height as i32);
            context.gl.disable(glow::SCISSOR_TEST);
            context.gl.clear_color(0.0, 0.0, 0.0, 0.0);
            context.gl.clear(glow::COLOR_BUFFER_BIT);
        }
        context
            .effect_runtime
            .effect_resources
            .unbind_render_target(context.gl);
        Ok(())
    })();
    context.establish_ordinary_scene_state();
    result
}

fn copy_framebuffer_region_to_texture(
    context: &mut LifecycleRenderContext<'_>,
    target: &PooledEffectTexture,
    rect: compositor::PresentationRect,
    framebuffer_origin: OutputFramebufferOrigin,
    source: EffectFramebufferTarget,
    restore_target: EffectFramebufferTarget,
) -> RendererResult<()> {
    let Some(plan) = lifecycle_output_copy_region(
        context.scene_state.current_size,
        rect,
        (target.key.width, target.key.height),
        f64::from(context.effect_runtime.effect_output_scale),
        framebuffer_origin,
    ) else {
        return Ok(());
    };
    let result = (|| {
        let draw_framebuffer = context
            .effect_runtime
            .effect_resources
            .bind_draw_target(context.gl, target)?;
        if source.framebuffer == Some(draw_framebuffer) {
            return Err(io::Error::other("lifecycle source and texture targets alias").into());
        }
        unsafe {
            context.gl.disable(glow::SCISSOR_TEST);
            context
                .gl
                .bind_framebuffer(glow::READ_FRAMEBUFFER, source.framebuffer);
            context
                .gl
                .bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(draw_framebuffer));
            context.gl.blit_framebuffer(
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
    context.establish_effect_composition_state(restore_target);
    result
}

#[cfg(test)]
mod tests;
