use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn execute_capture(
    renderer: &mut EffectExecutionContext<'_>,
    graph: &CompiledFrameGraph,
    textures: &std::collections::HashMap<GraphTextureId, GraphTextureBinding>,
    pass: &CompiledRenderPass,
    targets: EffectExecutionTargets,
    framebuffer_origin: OutputFramebufferOrigin,
    execution_damage: &EffectRegion,
    scene_baseline_authority: SceneBaselineAuthority,
    debug_config: EffectDebugConfig,
    capture_plan: CheckpointCaptureExecutionPlan,
    causal_stability: &CheckpointCausalStabilityPlan,
    host_timing_enabled: bool,
    fusion: Option<CaptureDownsampleFusion>,
    stats: &mut EffectExecutionStats,
) -> RendererResult<Option<ReplayCaptureExecutionDetail>> {
    let output = pass
        .output
        .ok_or_else(|| io::Error::other("capture pass has no output texture"))?;
    let target = textures
        .get(&output)
        .ok_or_else(|| io::Error::other("capture output texture is not allocated"))?;
    let target_plan = graph_texture(graph, output)?;
    let direct_capture =
        is_direct_framebuffer_capture(pass, scene_baseline_authority, debug_config);
    let replay_host_timing_enabled =
        replay_capture_host_timing_enabled(host_timing_enabled, direct_capture);
    let host_start = replay_host_timing_enabled.then(Instant::now);
    let persistent_capture_mode = persistent_scene_capture_mode(
        pass,
        target,
        scene_baseline_authority,
        debug_config,
        capture_plan,
    );
    let checkpoint_cache_key =
        persistent_capture_mode.and_then(|_| checkpoint_capture_cache_key(graph, pass));
    let dependency_free_replay_cache =
        persistent_capture_mode == Some(PersistentSceneCaptureMode::DependencyFreeReplayCapture);
    let frame_serial = renderer.runtime.effect_resources.checkpoint_frame_serial();
    let checkpoint_full_refresh = checkpoint_cache_key.as_ref().is_some_and(|key| {
        renderer
            .runtime
            .effect_resources
            .checkpoint_capture_needs_full_refresh(key, frame_serial)
    });
    if let Some(fusion) = fusion {
        let Some(key) = checkpoint_cache_key.as_ref() else {
            return Err(io::Error::other(
                "fused checkpoint capture has no persistent cache identity",
            )
            .into());
        };
        if fusion.capture_pass != pass.id
            || fusion.capture_texture != output
            || !direct_capture
            || !target.is_checkpoint_cache()
            || pass.checkpoint_dependencies.is_empty()
            || capture_plan.executed != CaptureTimingMode::FramebufferShaderCopy
            || !checkpoint_full_refresh
        {
            return Err(io::Error::other("fused checkpoint capture failed validation").into());
        }
        stats.record_capture_execution_with_mode(pass, capture_plan.executed, 0, 0);
        stats.capture_downsample_fusion_elided_capture_pixels = stats
            .capture_downsample_fusion_elided_capture_pixels
            .saturating_add(
                u64::from(target_plan.width).saturating_mul(u64::from(target_plan.height)),
            );
        debug_assert!(
            renderer
                .runtime
                .effect_resources
                .checkpoint_capture_needs_full_refresh(key, frame_serial)
        );
        return Ok(None);
    }
    let causal_capture = causal_stability.captures.get(&pass.id);
    let causal_zero_copy = checkpoint_cache_key.is_some()
        && !checkpoint_full_refresh
        && causal_capture.is_some_and(|stability| stability.source_unchanged);
    if checkpoint_cache_key.is_some()
        && !checkpoint_full_refresh
        && renderer
            .runtime
            .effect_gpu_profiler
            .cache_telemetry_enabled()
    {
        if causal_zero_copy {
            stats.checkpoint_causal_proven_unchanged =
                stats.checkpoint_causal_proven_unchanged.saturating_add(1);
        } else {
            stats.checkpoint_causal_unproven = stats.checkpoint_causal_unproven.saturating_add(1);
            match causal_capture.map(|stability| stability.unproven_reason) {
                Some(Some(CheckpointCausalUnprovenReason::ScenePrefixChanged)) => {
                    stats.checkpoint_causal_scene_prefix_changed = stats
                        .checkpoint_causal_scene_prefix_changed
                        .saturating_add(1);
                }
                Some(Some(CheckpointCausalUnprovenReason::DependencyChanged)) => {
                    stats.checkpoint_causal_dependency_changed =
                        stats.checkpoint_causal_dependency_changed.saturating_add(1);
                }
                _ => {}
            }
        }
    }
    let materialization = if direct_capture {
        checkpoint_cache_key.as_ref().map(|_| {
            if checkpoint_full_refresh {
                capture_materialization_plan(
                    &EffectRegion::from_rect(target_plan.domain),
                    Some(target_plan.domain),
                    renderer.scene.current_size,
                )
            } else if causal_zero_copy {
                CaptureMaterializationPlan {
                    region: EffectRegion::empty(),
                    output_rects: Vec::new(),
                }
            } else {
                checkpoint_update_materialization_plan(
                    &graph.final_damage,
                    target_plan.domain,
                    renderer.scene.current_size,
                )
            }
        })
    } else if dependency_free_replay_cache {
        Some(if causal_zero_copy {
            CaptureMaterializationPlan {
                region: EffectRegion::empty(),
                output_rects: Vec::new(),
            }
        } else {
            capture_materialization_plan(
                &EffectRegion::from_rect(target_plan.domain),
                Some(target_plan.domain),
                renderer.scene.current_size,
            )
        })
    } else {
        Some(capture_materialization_plan(
            execution_damage,
            Some(target_plan.domain),
            renderer.scene.current_size,
        ))
    };
    let capture_rects = if direct_capture && checkpoint_cache_key.is_none() {
        vec![full_output_rect((target_plan.width, target_plan.height))]
    } else {
        materialization
            .as_ref()
            .expect("capture materialization plan is required")
            .output_rects
            .clone()
    };
    let capture_texture_rects = materialized_target_rects(
        direct_capture,
        checkpoint_cache_key.is_some(),
        materialization.as_ref(),
        &capture_rects,
        target_plan,
    );
    let physical_pixels = output_rect_pixels(&capture_rects);
    if let Some(materialization) = materialization.as_ref()
        && renderer.runtime.effect_trace.enabled()
    {
        renderer.runtime.effect_trace.capture_materialization(
            pass,
            materialization.region.rects().len(),
            materialization
                .region
                .bounding_rect()
                .map(|rect| (rect.x, rect.y, rect.width, rect.height)),
            materialization.output_rects.len(),
            output_rect_pixels(&capture_rects),
        );
    }
    if dependency_free_replay_cache && causal_zero_copy {
        stats.record_capture_execution_with_mode(pass, CaptureTimingMode::Replay, 0, 0);
        if let Some(key) = checkpoint_cache_key.as_ref() {
            renderer
                .runtime
                .effect_resources
                .mark_checkpoint_capture_populated(key, frame_serial);
            if renderer
                .runtime
                .effect_gpu_profiler
                .cache_telemetry_enabled()
            {
                let domain_pixels =
                    u64::from(target_plan.width).saturating_mul(u64::from(target_plan.height));
                stats.record_dependency_free_replay_cache_decision(
                    true,
                    false,
                    true,
                    domain_pixels,
                    0,
                );
            }
        }
        return Ok(None);
    }
    if direct_capture {
        stats.record_capture_execution_with_mode(pass, capture_plan.executed, physical_pixels, 0);
        match capture_plan.executed {
            CaptureTimingMode::FramebufferBlit => {
                capture_output_region_to_graph_texture_with_targets(
                    renderer.gl,
                    &mut renderer.runtime.effect_resources,
                    renderer.scene.current_size,
                    renderer.scene.program,
                    target,
                    target_plan,
                    framebuffer_origin,
                    targets,
                )?;
            }
            CaptureTimingMode::FramebufferShaderCopy => {
                if let Some(key) = checkpoint_cache_key.as_ref() {
                    renderer
                        .runtime
                        .effect_resources
                        .invalidate_checkpoint_capture(key);
                }
                if physical_pixels != 0 {
                    let output_texture = renderer.scene.active_output_texture.ok_or_else(|| {
                        io::Error::other("shader-copy capture has no sampleable output texture")
                    })?;
                    capture_output_rects_to_graph_texture_shader_copy_with_targets(
                        renderer.gl,
                        renderer.runtime,
                        renderer.scene.current_size,
                        renderer.scene.program,
                        target,
                        target_plan,
                        framebuffer_origin,
                        output_texture,
                        &capture_texture_rects,
                        targets,
                    )?;
                }
                if let Some(key) = checkpoint_cache_key.as_ref() {
                    renderer
                        .runtime
                        .effect_resources
                        .mark_checkpoint_capture_populated(key, frame_serial);
                    if renderer
                        .runtime
                        .effect_gpu_profiler
                        .cache_telemetry_enabled()
                    {
                        let domain_pixels = u64::from(target_plan.width)
                            .saturating_mul(u64::from(target_plan.height));
                        stats.checkpoint_cache_domain_pixels = stats
                            .checkpoint_cache_domain_pixels
                            .saturating_add(domain_pixels);
                        stats.checkpoint_cache_update_pixels = stats
                            .checkpoint_cache_update_pixels
                            .saturating_add(physical_pixels);
                        stats.checkpoint_cache_saved_pixels = stats
                            .checkpoint_cache_saved_pixels
                            .saturating_add(domain_pixels.saturating_sub(physical_pixels));
                        if checkpoint_full_refresh {
                            stats.checkpoint_cache_full_refreshes =
                                stats.checkpoint_cache_full_refreshes.saturating_add(1);
                        } else {
                            stats.checkpoint_cache_hits =
                                stats.checkpoint_cache_hits.saturating_add(1);
                            if physical_pixels == 0 {
                                stats.checkpoint_cache_zero_copy_hits =
                                    stats.checkpoint_cache_zero_copy_hits.saturating_add(1);
                            }
                        }
                    }
                }
            }
            CaptureTimingMode::Replay => unreachable!("direct capture selected replay timing"),
        }
        return Ok(None);
    }
    if dependency_free_replay_cache && let Some(key) = checkpoint_cache_key.as_ref() {
        // A failed bind, clear, selection, or draw must never leave the
        // partially rewritten Replay texture authoritative.
        renderer
            .runtime
            .effect_resources
            .invalidate_checkpoint_capture(key);
    }
    renderer
        .runtime
        .effect_resources
        .bind_render_target(renderer.gl, target)?;
    unsafe {
        renderer
            .gl
            .viewport(0, 0, target_plan.width as i32, target_plan.height as i32);
        renderer.gl.disable(glow::SCISSOR_TEST);
        renderer.gl.disable(glow::BLEND);
        renderer.gl.clear_color(0.0, 0.0, 0.0, 0.0);
        for rect in &capture_texture_rects {
            renderer.gl.enable(glow::SCISSOR_TEST);
            renderer
                .gl
                .scissor(rect.x, rect.y, rect.width as i32, rect.height as i32);
            renderer.gl.clear(glow::COLOR_BUFFER_BIT);
        }
        renderer.gl.disable(glow::SCISSOR_TEST);
        establish_effect_pass_blend_state(renderer.gl, capture_blend_mode());
        renderer
            .gl
            .use_program(Some(renderer.runtime.capture_program));
        if let Some(location) = renderer.capture_uniform_location("u_capture_output_size") {
            renderer.gl.uniform_2_f32(
                Some(&location),
                renderer.scene.current_size.0.max(1) as f32,
                renderer.scene.current_size.1.max(1) as f32,
            );
        }
        if let Some(location) = renderer.capture_uniform_location("u_capture_domain") {
            renderer.gl.uniform_4_f32(
                Some(&location),
                target_plan.domain.x as f32,
                target_plan.domain.y as f32,
                target_plan.domain.width.max(1) as f32,
                target_plan.domain.height.max(1) as f32,
            );
        }
        if let Some(location) = renderer.capture_uniform_location("u_capture_origin_bottom_left") {
            renderer.gl.uniform_1_i32(
                Some(&location),
                i32::from(matches!(
                    framebuffer_origin,
                    OutputFramebufferOrigin::BottomLeft
                )),
            );
        }
    }
    let selection_start = replay_host_timing_enabled.then(Instant::now);
    let layers = renderer
        .scene
        .commands
        .iter()
        .map(|command| match command.layer {
            EglDrawLayer::Surface(id) => capture::CaptureLayer::Surface(id),
            _ => capture::CaptureLayer::Other,
        })
        .collect::<Vec<_>>();
    let visual_groups = renderer
        .scene
        .commands
        .iter()
        .map(|command| command.visual_group)
        .collect::<Vec<_>>();
    let indices = capture::indices_for_capture(
        &layers,
        &visual_groups,
        pass.anchor,
        pass.kind == RenderPassKind::SurfaceCapture,
        pass.visual_group,
        pass.anchor_scope,
    );
    let selection_cpu_ns = monotonic_elapsed_ns(selection_start);
    stats.record_capture_execution(pass, false, physical_pixels, indices.len());
    let scissors = materialization
        .as_ref()
        .expect("replay capture has a materialization plan")
        .output_rects
        .clone();
    renderer.scene.capture_unattenuated_visual_group = pass.visual_group;
    renderer.scene.capture_unclipped_presentation_owner =
        renderer.presentation_owner_for_visual_group(pass.visual_group);
    #[cfg(test)]
    let inject_replay_failure = dependency_free_replay_cache
        && std::mem::replace(
            &mut renderer.runtime.fail_next_dependency_free_replay_capture,
            false,
        );
    #[cfg(not(test))]
    let inject_replay_failure = false;
    let draw_result = if inject_replay_failure {
        Err(io::Error::other("injected dependency-free Replay capture failure").into())
    } else {
        renderer.draw_capture_commands_for_regions(
            &indices,
            &scissors,
            target_plan.domain,
            (target_plan.width, target_plan.height),
            replay_host_timing_enabled,
        )
    };
    renderer.scene.capture_unattenuated_visual_group = None;
    renderer.scene.capture_unclipped_presentation_owner = None;
    let mut detail = draw_result?;
    renderer
        .runtime
        .effect_resources
        .unbind_render_target(renderer.gl);
    renderer.establish_effect_composition_state(targets.composition_draw);
    restore_output_viewport(renderer.gl, renderer.scene.current_size);
    establish_effect_pass_blend_state(renderer.gl, EffectPassBlendMode::Replace);
    detail.execution_pixels = physical_pixels;
    detail.selection_cpu_ns = selection_cpu_ns;
    detail.host_cpu_ns = monotonic_elapsed_ns(host_start);
    if dependency_free_replay_cache && let Some(key) = checkpoint_cache_key.as_ref() {
        renderer
            .runtime
            .effect_resources
            .mark_checkpoint_capture_populated(key, frame_serial);
        if renderer
            .runtime
            .effect_gpu_profiler
            .cache_telemetry_enabled()
        {
            let domain_pixels =
                u64::from(target_plan.width).saturating_mul(u64::from(target_plan.height));
            stats.record_dependency_free_replay_cache_decision(
                !checkpoint_full_refresh,
                physical_pixels == domain_pixels,
                false,
                domain_pixels,
                physical_pixels,
            );
        }
    }
    Ok(Some(detail))
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
pub(super) struct GraphTextureCaptureBlit {
    source: GlBlitRect,
    destination: GlBlitRect,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct SceneWorkPreservationPlan {
    transfers: Vec<GraphTextureCaptureBlit>,
    pixels: u64,
}

impl GraphTextureCaptureBlit {
    const fn inverse(self) -> Self {
        Self {
            source: self.destination,
            destination: self.source,
        }
    }

    fn pixels(self) -> u64 {
        let width =
            (i64::from(self.destination.x1) - i64::from(self.destination.x0)).unsigned_abs();
        let height =
            (i64::from(self.destination.y1) - i64::from(self.destination.y0)).unsigned_abs();
        width.saturating_mul(height)
    }
}

impl SceneWorkPreservationPlan {
    fn from_extra_scene_work(
        extra_scene_work: &[OutputRect],
        output_size: (u32, u32),
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> Self {
        let transfers = extra_scene_work
            .iter()
            .filter_map(|rect| {
                scene_work_preservation_blit_rects(*rect, output_size, framebuffer_origin)
            })
            .collect::<Vec<_>>();
        let pixels = transfers.iter().copied().fold(0u64, |total, transfer| {
            total.saturating_add(transfer.pixels())
        });
        Self { transfers, pixels }
    }
}

pub(super) fn scene_work_preservation_blit_rects(
    rect: OutputRect,
    output_size: (u32, u32),
    framebuffer_origin: OutputFramebufferOrigin,
) -> Option<GraphTextureCaptureBlit> {
    let left = i64::from(rect.x).clamp(0, i64::from(output_size.0));
    let top = i64::from(rect.y).clamp(0, i64::from(output_size.1));
    let right = (i64::from(rect.x) + i64::from(rect.width)).clamp(0, i64::from(output_size.0));
    let bottom = (i64::from(rect.y) + i64::from(rect.height)).clamp(0, i64::from(output_size.1));
    if right <= left || bottom <= top {
        return None;
    }
    let left = i32::try_from(left).ok()?;
    let top = i32::try_from(top).ok()?;
    let right = i32::try_from(right).ok()?;
    let bottom = i32::try_from(bottom).ok()?;
    let height = i32::try_from(output_size.1).ok()?;
    let canonical_texture_low_y = height - bottom;
    let canonical_texture_high_y = height - top;
    let (source, destination) = match framebuffer_origin {
        OutputFramebufferOrigin::BottomLeft => {
            let transfer = GlBlitRect::new(
                left,
                canonical_texture_low_y,
                right,
                canonical_texture_high_y,
            );
            (transfer, transfer)
        }
        OutputFramebufferOrigin::TopLeftScanout => (
            GlBlitRect::new(left, top, right, bottom),
            GlBlitRect::new(
                left,
                canonical_texture_high_y,
                right,
                canonical_texture_low_y,
            ),
        ),
    };
    Some(GraphTextureCaptureBlit {
        source,
        destination,
    })
}

pub(crate) struct SceneWorkPreservation {
    pub(super) texture: PooledEffectTexture,
    plan: SceneWorkPreservationPlan,
}

impl SceneWorkPreservation {
    #[cfg(test)]
    pub(crate) fn transfer_count(&self) -> usize {
        self.plan.transfers.len()
    }

    #[cfg(test)]
    pub(crate) fn preserved_pixels(&self) -> u64 {
        self.plan.pixels
    }

    #[cfg(test)]
    pub(crate) fn release(
        self,
        renderer: &mut super::super::super::GlesSceneRenderer,
    ) -> RendererResult<()> {
        renderer
            .effect_runtime
            .effect_resources
            .release(self.texture)?;
        Ok(())
    }
}

pub(super) fn capture_scene_work_preservation_context(
    renderer: &mut EffectExecutionContext<'_>,
    output_size: (u32, u32),
    framebuffer_origin: OutputFramebufferOrigin,
    extra_scene_work: &[OutputRect],
) -> RendererResult<SceneWorkPreservation> {
    let plan = SceneWorkPreservationPlan::from_extra_scene_work(
        extra_scene_work,
        output_size,
        framebuffer_origin,
    );
    let key = EffectTextureKey::new(
        output_size.0,
        output_size.1,
        EffectTextureFormat::Rgba8,
        EffectTextureFilter::Nearest,
        oblivion_one::effects::EffectWorkingSpace::OutputEncodedSrgb,
    );
    let texture = renderer
        .runtime
        .effect_resources
        .acquire(renderer.gl, key)?;
    let result = (|| {
        let output_framebuffer = renderer.scene.active_output_framebuffer;
        renderer.bind_active_output_framebuffer();
        let draw_framebuffer = renderer
            .runtime
            .effect_resources
            .bind_draw_target(renderer.gl, &texture)?;
        unsafe {
            renderer.gl.disable(glow::SCISSOR_TEST);
            renderer
                .gl
                .bind_framebuffer(glow::READ_FRAMEBUFFER, output_framebuffer);
            renderer
                .gl
                .bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(draw_framebuffer));
            for transfer in &plan.transfers {
                renderer.gl.blit_framebuffer(
                    transfer.source.x0,
                    transfer.source.y0,
                    transfer.source.x1,
                    transfer.source.y1,
                    transfer.destination.x0,
                    transfer.destination.y0,
                    transfer.destination.x1,
                    transfer.destination.y1,
                    glow::COLOR_BUFFER_BIT,
                    glow::NEAREST,
                );
            }
        }
        Ok::<(), Box<dyn std::error::Error>>(())
    })();
    renderer.establish_ordinary_scene_state();
    match result {
        Ok(()) => {
            renderer.runtime.effect_trace.scene_work_preservation(
                "capture",
                plan.transfers.len(),
                plan.pixels,
                u64::from(output_size.0).saturating_mul(u64::from(output_size.1)),
            );
            Ok(SceneWorkPreservation { texture, plan })
        }
        Err(error) => {
            let _ = renderer.runtime.effect_resources.release(texture);
            Err(error)
        }
    }
}

pub(super) fn restore_scene_work_preservation_context(
    renderer: &mut EffectExecutionContext<'_>,
    preservation: &SceneWorkPreservation,
) -> RendererResult<()> {
    let output_framebuffer = renderer.scene.active_output_framebuffer;
    let read_framebuffer = renderer
        .runtime
        .effect_resources
        .bind_read_target(renderer.gl, &preservation.texture)?;
    unsafe {
        renderer.gl.disable(glow::SCISSOR_TEST);
        renderer
            .gl
            .bind_framebuffer(glow::READ_FRAMEBUFFER, Some(read_framebuffer));
        renderer
            .gl
            .bind_framebuffer(glow::DRAW_FRAMEBUFFER, output_framebuffer);
        for transfer in preservation.plan.transfers.iter().copied() {
            let transfer = transfer.inverse();
            renderer.gl.blit_framebuffer(
                transfer.source.x0,
                transfer.source.y0,
                transfer.source.x1,
                transfer.source.y1,
                transfer.destination.x0,
                transfer.destination.y0,
                transfer.destination.x1,
                transfer.destination.y1,
                glow::COLOR_BUFFER_BIT,
                glow::NEAREST,
            );
        }
    }
    renderer.establish_ordinary_scene_state();
    renderer.runtime.effect_trace.scene_work_preservation(
        "restore",
        preservation.plan.transfers.len(),
        preservation.plan.pixels,
        u64::from(renderer.scene.current_size.0)
            .saturating_mul(u64::from(renderer.scene.current_size.1)),
    );
    Ok(())
}

/// Effect coordinate contract:
///
/// * logical domains are top-left, integer output-space rectangles;
/// * graph textures use bottom-left physical storage;
/// * the framebuffer origin describes the active output image only;
/// * fullscreen vertex UVs describe the logical destination and are flipped
///   only when drawing directly to a top-left scanout framebuffer;
/// * sampled graph inputs are converted independently using their storage
///   origin; and
/// * translating a logical domain must never change either orientation flag.
///
/// Scene replay uses `u_capture_domain` plus
/// `u_capture_origin_bottom_left`. Direct framebuffer capture uses the
/// explicit READ/DRAW blit mapping. Kawase, normalization, and final
/// composite passes use the same independent target/input rules here.
pub(super) fn effect_target_requires_logical_y_flip(
    output_is_framebuffer: bool,
    framebuffer_origin: OutputFramebufferOrigin,
) -> bool {
    output_is_framebuffer && framebuffer_origin == OutputFramebufferOrigin::TopLeftScanout
}

/// Returns whether logical UVs must be converted to physical sampling UVs for
/// the graph texture's canonical storage origin.
pub(super) fn effect_input_requires_sample_y_flip(
    input_origin: oblivion_one::effects::GraphTextureOrigin,
) -> bool {
    matches!(
        input_origin,
        oblivion_one::effects::GraphTextureOrigin::BottomLeft
    )
}

pub(super) fn plan_graph_texture_capture(
    output_size: (u32, u32),
    domain: oblivion_one::effects::EffectRect,
    target_size: (u32, u32),
    framebuffer_origin: OutputFramebufferOrigin,
) -> Option<GraphTextureCaptureBlit> {
    if target_size.0 == 0 || target_size.1 == 0 || domain.x < 0 || domain.y < 0 {
        return None;
    }
    let domain_right = i64::from(domain.x).checked_add(i64::from(domain.width))?;
    let domain_bottom = i64::from(domain.y).checked_add(i64::from(domain.height))?;
    if domain_right > i64::from(output_size.0) || domain_bottom > i64::from(output_size.1) {
        return None;
    }
    let source_y = match framebuffer_origin {
        OutputFramebufferOrigin::BottomLeft => {
            i64::from(output_size.1).checked_sub(domain_bottom)?
        }
        OutputFramebufferOrigin::TopLeftScanout => i64::from(domain.y),
    };
    let source = GlBlitRect {
        x0: domain.x,
        y0: i32::try_from(source_y).ok()?,
        x1: i32::try_from(domain_right).ok()?,
        y1: i32::try_from(source_y.checked_add(i64::from(domain.height))?).ok()?,
    };
    let target_width = i32::try_from(target_size.0).ok()?;
    let target_height = i32::try_from(target_size.1).ok()?;
    let destination = match framebuffer_origin {
        OutputFramebufferOrigin::BottomLeft => GlBlitRect {
            x0: 0,
            y0: 0,
            x1: target_width,
            y1: target_height,
        },
        OutputFramebufferOrigin::TopLeftScanout => GlBlitRect {
            x0: 0,
            y0: target_height,
            x1: target_width,
            y1: 0,
        },
    };
    Some(GraphTextureCaptureBlit {
        source,
        destination,
    })
}

#[cfg(test)]
pub(super) fn shader_copy_source_texel(
    output_size: (u32, u32),
    domain: oblivion_one::effects::EffectRect,
    target_size: (u32, u32),
    framebuffer_origin: OutputFramebufferOrigin,
    destination: (u32, u32),
) -> Option<(i32, i32)> {
    let transfer =
        plan_graph_texture_capture(output_size, domain, target_size, framebuffer_origin)?;
    if destination.0 >= target_size.0 || destination.1 >= target_size.1 {
        return None;
    }
    let logical_y = target_size.1.checked_sub(destination.1)?.checked_sub(1)?;
    let source_x = domain.x.checked_add(i32::try_from(destination.0).ok()?)?;
    let source_y = match framebuffer_origin {
        OutputFramebufferOrigin::BottomLeft => i32::try_from(output_size.1)
            .ok()?
            .checked_sub(1)?
            .checked_sub(domain.y)?
            .checked_sub(i32::try_from(logical_y).ok()?)?,
        OutputFramebufferOrigin::TopLeftScanout => {
            domain.y.checked_add(i32::try_from(logical_y).ok()?)?
        }
    };
    let source = (source_x, source_y);
    (source.0 >= transfer.source.x0
        && source.0 < transfer.source.x1
        && source.1 >= transfer.source.y0
        && source.1 < transfer.source.y1)
        .then_some(source)
}

/// Capture a logical output domain into a graph texture with the canonical
/// `GraphTextureOrigin::BottomLeft` orientation.
#[cfg(test)]
pub(crate) fn capture_output_region_to_graph_texture_context(
    renderer: &mut EffectExecutionContext<'_>,
    target: &PooledEffectTexture,
    target_plan: &oblivion_one::effects::GraphTexturePlan,
    framebuffer_origin: OutputFramebufferOrigin,
) -> RendererResult<()> {
    let output = EffectFramebufferTarget::new(renderer.scene.active_output_framebuffer);
    capture_output_region_to_graph_texture_with_targets(
        renderer.gl,
        &mut renderer.runtime.effect_resources,
        renderer.scene.current_size,
        renderer.scene.program,
        target,
        target_plan,
        framebuffer_origin,
        EffectExecutionTargets::ordinary(output),
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn capture_output_region_to_graph_texture_with_targets(
    gl: &glow::Context,
    effect_resources: &mut super::super::resources::EffectGlResourceCache,
    output_size: (u32, u32),
    scene_program: glow::Program,
    target: &PooledEffectTexture,
    target_plan: &oblivion_one::effects::GraphTexturePlan,
    framebuffer_origin: OutputFramebufferOrigin,
    targets: EffectExecutionTargets,
) -> RendererResult<()> {
    if target_plan.origin != oblivion_one::effects::GraphTextureOrigin::BottomLeft {
        return Err(io::Error::other("direct capture target is not bottom-left oriented").into());
    }
    let transfer = plan_graph_texture_capture(
        output_size,
        target_plan.domain,
        (target_plan.width, target_plan.height),
        framebuffer_origin,
    )
    .ok_or_else(|| io::Error::other("direct capture domain is outside the output"))?;
    let result = (|| {
        let draw_framebuffer = effect_resources.bind_draw_target(gl, target)?;
        if targets.baseline_read.framebuffer == Some(draw_framebuffer)
            || target_plan.source == GraphTextureSource::Output
        {
            return Err(
                Box::new(EffectExecutionInvariantError::InvalidFramebufferBlitTargets)
                    as Box<dyn std::error::Error>,
            );
        }
        unsafe {
            gl.disable(glow::SCISSOR_TEST);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, targets.baseline_read.framebuffer);
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(draw_framebuffer));
            gl.blit_framebuffer(
                transfer.source.x0,
                transfer.source.y0,
                transfer.source.x1,
                transfer.source.y1,
                transfer.destination.x0,
                transfer.destination.y0,
                transfer.destination.x1,
                transfer.destination.y1,
                glow::COLOR_BUFFER_BIT,
                glow::NEAREST,
            );
        }
        Ok(())
    })();
    super::super::super::scene_state::establish_scene_gl_state(
        gl,
        output_size,
        scene_program,
        targets.composition_draw.framebuffer,
    );
    result
}

/// Copy the active output image into the same pooled graph texture used by the
/// framebuffer-blit path. The output image is sampled as an integer texel
/// source, while the graph target remains bottom-left oriented and local to
/// the capture domain.
#[cfg(test)]
pub(crate) fn capture_output_region_to_graph_texture_shader_copy_context(
    renderer: &mut EffectExecutionContext<'_>,
    target: &PooledEffectTexture,
    target_plan: &oblivion_one::effects::GraphTexturePlan,
    framebuffer_origin: OutputFramebufferOrigin,
    output_texture: glow::Texture,
) -> RendererResult<()> {
    let full_target = full_output_rect((target_plan.width, target_plan.height));
    capture_output_rects_to_graph_texture_shader_copy_context(
        renderer,
        target,
        target_plan,
        framebuffer_origin,
        output_texture,
        std::slice::from_ref(&full_target),
    )
}

/// Update only the supplied bottom-left-local rectangles in a graph capture
/// target. The capture shader still samples the active output using the same
/// output-space mapping as a full-domain capture.
#[cfg(test)]
pub(crate) fn capture_output_rects_to_graph_texture_shader_copy_context(
    renderer: &mut EffectExecutionContext<'_>,
    target: &PooledEffectTexture,
    target_plan: &oblivion_one::effects::GraphTexturePlan,
    framebuffer_origin: OutputFramebufferOrigin,
    output_texture: glow::Texture,
    target_rects: &[OutputRect],
) -> RendererResult<()> {
    let output = EffectFramebufferTarget::new(renderer.scene.active_output_framebuffer);
    capture_output_rects_to_graph_texture_shader_copy_with_targets(
        renderer.gl,
        renderer.runtime,
        renderer.scene.current_size,
        renderer.scene.program,
        target,
        target_plan,
        framebuffer_origin,
        output_texture,
        target_rects,
        EffectExecutionTargets::ordinary(output),
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn capture_output_rects_to_graph_texture_shader_copy_with_targets(
    gl: &glow::Context,
    runtime: &mut EffectRuntime,
    output_size: (u32, u32),
    scene_program: glow::Program,
    target: &PooledEffectTexture,
    target_plan: &oblivion_one::effects::GraphTexturePlan,
    framebuffer_origin: OutputFramebufferOrigin,
    output_texture: glow::Texture,
    target_rects: &[OutputRect],
    targets: EffectExecutionTargets,
) -> RendererResult<()> {
    if target_plan.origin != oblivion_one::effects::GraphTextureOrigin::BottomLeft {
        return Err(io::Error::other("direct capture target is not bottom-left oriented").into());
    }
    plan_graph_texture_capture(
        output_size,
        target_plan.domain,
        (target_plan.width, target_plan.height),
        framebuffer_origin,
    )
    .ok_or_else(|| io::Error::other("direct capture domain is outside the output"))?;
    let result = (|| {
        let draw_framebuffer = runtime.effect_resources.bind_draw_target(gl, target)?;
        // This is a hard guard against sampling from the image attached to the
        // current draw framebuffer. The output framebuffer is never bound as
        // DRAW for this path; only the pooled graph target is.
        if targets.baseline_read.framebuffer == Some(draw_framebuffer)
            || target_plan.source == GraphTextureSource::Output
        {
            return Err(
                Box::new(EffectExecutionInvariantError::InvalidFramebufferBlitTargets)
                    as Box<dyn std::error::Error>,
            );
        }
        let (vertex_array, _) = runtime.ensure_effect_quad(gl)?;
        unsafe {
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(draw_framebuffer));
            gl.viewport(0, 0, target_plan.width as i32, target_plan.height as i32);
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::BLEND);
            gl.use_program(Some(runtime.capture_copy_program));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(output_texture));
            if let Some(location) = runtime.capture_copy_uniform_location(gl, "u_output_texture") {
                gl.uniform_1_i32(Some(&location), 0);
            }
            if let Some(location) =
                runtime.capture_copy_uniform_location(gl, "u_capture_output_size")
            {
                gl.uniform_2_f32(Some(&location), output_size.0 as f32, output_size.1 as f32);
            }
            if let Some(location) = runtime.capture_copy_uniform_location(gl, "u_capture_domain") {
                gl.uniform_4_f32(
                    Some(&location),
                    target_plan.domain.x as f32,
                    target_plan.domain.y as f32,
                    target_plan.domain.width as f32,
                    target_plan.domain.height as f32,
                );
            }
            if let Some(location) =
                runtime.capture_copy_uniform_location(gl, "u_capture_target_size")
            {
                gl.uniform_2_f32(
                    Some(&location),
                    target_plan.width as f32,
                    target_plan.height as f32,
                );
            }
            if let Some(location) =
                runtime.capture_copy_uniform_location(gl, "u_capture_origin_bottom_left")
            {
                gl.uniform_1_i32(
                    Some(&location),
                    i32::from(matches!(
                        framebuffer_origin,
                        OutputFramebufferOrigin::BottomLeft
                    )),
                );
            }
            gl.bind_vertex_array(Some(vertex_array));
            gl.enable(glow::SCISSOR_TEST);
            for rect in target_rects {
                let right = i64::from(rect.x) + i64::from(rect.width);
                let bottom = i64::from(rect.y) + i64::from(rect.height);
                if rect.x < 0
                    || rect.y < 0
                    || right > i64::from(target_plan.width)
                    || bottom > i64::from(target_plan.height)
                {
                    return Err(io::Error::other(
                        "shader-copy update rectangle exceeds capture target",
                    )
                    .into());
                }
                gl.scissor(rect.x, rect.y, rect.width as i32, rect.height as i32);
                gl.draw_arrays(glow::TRIANGLES, 0, 6);
            }
            gl.disable(glow::SCISSOR_TEST);
        }
        Ok(())
    })();
    super::super::super::scene_state::establish_scene_gl_state(
        gl,
        output_size,
        scene_program,
        targets.composition_draw.framebuffer,
    );
    result
}

#[cfg(test)]
#[path = "tests/coordinates.rs"]
mod coordinate_tests;

#[cfg(test)]
#[path = "tests/capture_preservation.rs"]
mod preservation_tests;
