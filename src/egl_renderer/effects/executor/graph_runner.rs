use super::*;

pub(super) fn scene_advance_reason(
    pass: &CompiledRenderPass,
    scene_baseline_authority: SceneBaselineAuthority,
    framebuffer_capture: bool,
) -> Option<&'static str> {
    if scene_baseline_authority == SceneBaselineAuthority::PrecomposedFramebuffer {
        return None;
    }
    if !pass.checkpoint_dependencies.is_empty()
        && matches!(
            pass.kind,
            RenderPassKind::SceneCapture | RenderPassKind::SurfaceCapture
        )
    {
        return Some("checkpoint_dependency");
    }
    (pass.kind == RenderPassKind::SceneCapture && framebuffer_capture)
        .then_some("framebuffer_capture")
}

pub(super) fn scene_replay_work_mode(
    scene_baseline_authority: SceneBaselineAuthority,
    framebuffer_capture: bool,
) -> SceneReplayWorkMode {
    if scene_baseline_authority == SceneBaselineAuthority::PrecomposedFramebuffer
        || framebuffer_capture
    {
        SceneReplayWorkMode::GlobalBaseline
    } else {
        SceneReplayWorkMode::SuffixDemand
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn execute_graph_passes_inner(
    renderer: &mut EffectExecutionContext<'_>,
    graph: &CompiledFrameGraph,
    textures: &mut std::collections::HashMap<GraphTextureId, GraphTextureBinding>,
    targets: EffectExecutionTargets,
    framebuffer_origin: OutputFramebufferOrigin,
    repaint_plan: Option<&crate::egl_renderer::damage::RepaintPlan>,
    explicit_repaint_rects: Option<&[OutputRect]>,
    demand: &EffectExecutionDemand,
    selection: &EffectExecutionSelection,
    debug_config: EffectDebugConfig,
    scene_baseline_authority: SceneBaselineAuthority,
    scene_replay_work_mode_override: Option<SceneReplayWorkMode>,
    graph_scope: Option<super::super::gpu_timing::GraphTimingScope>,
    fusion_plan: &CaptureDownsampleFusionPlan,
    ordinary_overlay_rects: &mut Vec<OutputRect>,
) -> RendererResult<EffectExecutionStats> {
    let mut stats = EffectExecutionStats::default();
    stats.capture_downsample_fusion_candidates = fusion_plan.candidates;
    stats.capture_downsample_fusion_ineligible = fusion_plan.ineligible;
    let checkpoint_causal_stability =
        checkpoint_causal_stability_plan_for_owners(renderer.scene, renderer.runtime, graph);
    #[cfg(any(debug_assertions, test))]
    // Validity belongs to this graph execution and logical texture ID. A
    // pooled physical allocation never carries validity into this map.
    let mut valid_regions = std::collections::HashMap::<GraphTextureId, EffectRegion>::new();
    let framebuffer_capture = scene_baseline_authority == SceneBaselineAuthority::ReplayRequired
        && repaint_plan.is_some()
        && debug_config.capture_mode() == EffectDebugCaptureMode::Framebuffer;
    let repaint_rects = if let Some(rects) = explicit_repaint_rects {
        rects
    } else {
        *ordinary_overlay_rects = renderer.begin_effect_repaint(
            repaint_plan.expect("ordinary effect execution needs a repaint plan"),
            framebuffer_origin,
        )?;
        ordinary_overlay_rects.as_slice()
    };
    let scene_work = scene_replay_work_plan(
        repaint_rects,
        graph,
        selection,
        renderer.scene.current_size,
        scene_baseline_authority,
        debug_config,
    );
    let mut scene_work_state = SceneReplayWorkState::new(
        &scene_work,
        scene_replay_work_mode_override.unwrap_or_else(|| {
            scene_replay_work_mode(scene_baseline_authority, framebuffer_capture)
        }),
    );
    let scene_work_rects = &scene_work.baseline_work;
    let output_size = renderer.scene.current_size;
    let reconstruct_internal_scene_work = should_reconstruct_scene_work(
        scene_baseline_authority,
        framebuffer_capture,
        !scene_work.extra_scene_work.is_empty(),
    );
    let mut scene_valid_region = initial_scene_valid_region(scene_baseline_authority, &scene_work);
    let mut effect_valid_regions =
        std::collections::HashMap::<oblivion_one::effects::EffectInstanceId, EffectRegion>::new();
    let mut scene_work_preservation =
        if !targets.uses_separate_targets() && !scene_work.extra_scene_work.is_empty() {
            Some(capture_scene_work_preservation_context(
                renderer,
                output_size,
                framebuffer_origin,
                &scene_work.extra_scene_work,
            )?)
        } else {
            None
        };
    let execution_result = (|| -> RendererResult<EffectExecutionStats> {
        if reconstruct_internal_scene_work {
            renderer.clear_effect_scene_work(
                scene_work_rects,
                framebuffer_origin,
                targets.composition_draw,
            )?;
        }
        let mut scene_cursor = 0;
        for pass in &graph.passes {
            if !selection.executed_passes.contains(&pass.id) {
                continue;
            }
            let execution_damage = prepare_effect_execution_region(
                graph,
                pass,
                capture_execution_damage(
                    graph,
                    demand,
                    pass,
                    scene_baseline_authority,
                    debug_config,
                ),
            );
            let capture_plan = checkpoint_capture_execution_plan_for_pass(
                renderer,
                pass,
                scene_baseline_authority,
                debug_config,
            );
            if renderer.runtime.effect_trace.enabled() {
                renderer.runtime.effect_trace.execution_region(
                    pass,
                    execution_damage.input_rect_count,
                    execution_damage.region.rects().len(),
                    execution_damage.duplicate_rects_removed,
                    execution_damage.overlap_fragments_generated,
                    execution_damage.fallback,
                );
                if let Some((input_rect_count, clip_rect_count)) = pass.visible_clip_fallback {
                    renderer.runtime.effect_trace.visible_clip_fallback(
                        pass,
                        input_rect_count,
                        clip_rect_count,
                    );
                }
                let is_final_output_pass = matches!(
                    pass.kind,
                    RenderPassKind::Composite | RenderPassKind::OutputPostProcess
                );
                for fallback in demand.visible_clip_fallbacks().iter().filter(|fallback| {
                    fallback.instance == pass.instance
                        && (fallback.pass == Some(pass.id)
                            || (fallback.pass.is_none() && is_final_output_pass))
                }) {
                    renderer.runtime.effect_trace.visible_clip_fallback(
                        pass,
                        fallback.input_rect_count,
                        fallback.clip_rect_count,
                    );
                }
                renderer.runtime.effect_trace.pass_boundary(
                    "begin",
                    pass,
                    graph,
                    textures,
                    pass_trace_summary(
                        renderer,
                        graph,
                        pass,
                        demand,
                        &execution_damage.region,
                        scene_work_rects,
                        framebuffer_origin,
                        scene_baseline_authority,
                        debug_config,
                        capture_plan,
                    ),
                );
            }
            if renderer.runtime.effect_trace.enabled() {
                renderer.runtime.effect_trace.pass_boundary(
                    "resources_begin",
                    pass,
                    graph,
                    textures,
                    pass_trace_summary(
                        renderer,
                        graph,
                        pass,
                        demand,
                        &execution_damage.region,
                        scene_work_rects,
                        framebuffer_origin,
                        scene_baseline_authority,
                        debug_config,
                        capture_plan,
                    ),
                );
            }
            let resource_result = ensure_pass_textures(renderer, graph, pass, textures, &mut stats);
            if renderer.runtime.effect_trace.enabled() {
                renderer.runtime.effect_trace.pass_boundary(
                    "resources_end",
                    pass,
                    graph,
                    textures,
                    pass_trace_summary(
                        renderer,
                        graph,
                        pass,
                        demand,
                        &execution_damage.region,
                        scene_work_rects,
                        framebuffer_origin,
                        scene_baseline_authority,
                        debug_config,
                        capture_plan,
                    ),
                );
            }
            resource_result?;
            if let Some(scene_advance_reason) =
                scene_advance_reason(pass, scene_baseline_authority, framebuffer_capture)
            {
                let (draw_end, _) = composition_range(
                    &renderer.scene.commands,
                    pass.anchor,
                    pass.visual_group,
                    pass.anchor_scope,
                );
                if draw_end > scene_cursor {
                    if renderer.runtime.effect_trace.enabled() {
                        let metrics = scene_work_state.work_trace_metrics();
                        renderer.runtime.effect_trace.scene_replay_boundary(
                            "begin",
                            pass,
                            scene_advance_reason,
                            scene_cursor,
                            draw_end,
                            metrics.0,
                            metrics.1,
                            metrics.2,
                            metrics.3,
                            metrics.4,
                        );
                    }
                    renderer.draw_effect_scene_range(
                        scene_work_state.active_work(),
                        scene_cursor,
                        draw_end,
                        framebuffer_origin,
                        targets.composition_draw,
                    )?;
                    scene_valid_region =
                        scene_valid_region_after_scene_advance(scene_work_state.active_work());
                    if renderer.runtime.effect_trace.enabled() {
                        let metrics = scene_work_state.work_trace_metrics();
                        renderer.runtime.effect_trace.scene_replay_boundary(
                            "end",
                            pass,
                            scene_advance_reason,
                            scene_cursor,
                            draw_end,
                            metrics.0,
                            metrics.1,
                            metrics.2,
                            metrics.3,
                            metrics.4,
                        );
                    }
                    scene_cursor = draw_end;
                }
            }
            if matches!(
                pass.kind,
                RenderPassKind::Composite | RenderPassKind::OutputPostProcess
            ) {
                let (draw_end, next_cursor) = composition_range(
                    &renderer.scene.commands,
                    pass.anchor,
                    pass.visual_group,
                    pass.anchor_scope,
                );
                if renderer.runtime.effect_trace.enabled() {
                    let metrics = scene_work_state.work_trace_metrics();
                    renderer.runtime.effect_trace.scene_replay_boundary(
                        "begin",
                        pass,
                        "composite_advance",
                        scene_cursor,
                        draw_end,
                        metrics.0,
                        metrics.1,
                        metrics.2,
                        metrics.3,
                        metrics.4,
                    );
                }
                let active_work = scene_work_state.active_work();
                let composite_scene_replay = if should_time_composite_scene_replay(
                    pass.kind,
                    graph_scope.is_some(),
                    scene_cursor,
                    draw_end,
                    !active_work.is_empty(),
                    renderer.runtime.capture_in_progress,
                ) {
                    let work = composite_scene_replay_work(
                        scene_cursor,
                        draw_end,
                        renderer.scene.commands.len(),
                        active_work,
                        scene_work_state.pending_checkpoint_requirements(),
                    );
                    graph_scope.and_then(|scope| {
                        renderer
                            .runtime
                            .effect_gpu_profiler
                            .begin_composite_scene_replay(
                                renderer.gl,
                                scope,
                                u64::from(pass.id.get()),
                                pass.instance.get(),
                                work,
                            )
                    })
                } else {
                    None
                };
                let replay_host_start = composite_scene_replay.as_ref().map(|_| Instant::now());
                let replay_stats_before = composite_scene_replay
                    .as_ref()
                    .map(|_| renderer.scene.frame_stats);
                let draw_result = renderer.draw_effect_scene_range(
                    active_work,
                    scene_cursor,
                    draw_end,
                    framebuffer_origin,
                    targets.composition_draw,
                );
                if let Some(replay_span) = composite_scene_replay {
                    let host_cpu_ns = monotonic_elapsed_ns(replay_host_start);
                    let detail = finalize_composite_scene_replay_timing(
                        true,
                        || {
                            renderer
                                .runtime
                                .effect_gpu_profiler
                                .end_composite_scene_replay(renderer.gl, replay_span)
                        },
                        || {
                            composite_scene_replay_execution_detail(
                                replay_stats_before
                                    .expect("Composite replay timing captured before draw"),
                                renderer.scene.frame_stats,
                                host_cpu_ns,
                            )
                        },
                    );
                    if let Some(detail) = detail {
                        renderer
                            .runtime
                            .effect_gpu_profiler
                            .attach_composite_scene_replay_execution_detail(replay_span, detail);
                    }
                }
                draw_result?;
                if draw_end > scene_cursor {
                    scene_valid_region = scene_valid_region_after_scene_advance(active_work);
                }
                if renderer.runtime.effect_trace.enabled() {
                    let metrics = scene_work_state.work_trace_metrics();
                    renderer.runtime.effect_trace.scene_replay_boundary(
                        "end",
                        pass,
                        "composite_advance",
                        scene_cursor,
                        draw_end,
                        metrics.0,
                        metrics.1,
                        metrics.2,
                        metrics.3,
                        metrics.4,
                    );
                }
                scene_cursor = next_cursor.max(scene_cursor);
            }
            if is_direct_framebuffer_capture(pass, scene_baseline_authority, debug_config) {
                let required = pass_output_texture_domain(graph, pass);
                let required_effect_region =
                    checkpoint_dependency_influence_region(graph, pass, &required);
                let mut dependency_validity = Vec::new();
                for dependency in &pass.checkpoint_dependencies {
                    let Some(dependency_pass) = graph
                        .passes
                        .iter()
                        .find(|candidate| candidate.id == *dependency)
                    else {
                        continue;
                    };
                    let dependency_influence = graph
                        .instances
                        .iter()
                        .find(|instance| instance.id == dependency_pass.instance)
                        .map(|instance| instance.output_influence_region.intersect(&required))
                        .unwrap_or_else(EffectRegion::empty);
                    let dependency_valid = effect_valid_regions
                        .get(&dependency_pass.instance)
                        .cloned()
                        .unwrap_or_else(EffectRegion::empty);
                    dependency_validity.push((dependency_influence, dependency_valid));
                }
                let validity = checkpoint_source_semantic_validity(
                    &required,
                    &scene_valid_region,
                    &required_effect_region,
                    &dependency_validity,
                );
                if renderer.runtime.effect_trace.enabled() {
                    renderer.runtime.effect_trace.checkpoint_source_validity(
                        pass,
                        validity.required_rect_count,
                        validity.required_bounding_box,
                        validity.valid_rect_count,
                        validity.valid_bounding_box,
                        validity.missing.rects().len(),
                        validity
                            .missing
                            .bounding_rect()
                            .map(|rect| (rect.x, rect.y, rect.width, rect.height)),
                        effect_region_pixels(&validity.missing),
                    );
                }
                #[cfg(any(debug_assertions, test))]
                if !validity.missing.is_empty() {
                    let error = EffectExecutionInvariantError::InvalidCheckpointSource {
                        pass: pass.id,
                        missing: validity.missing,
                    };
                    renderer.runtime.effect_trace.invariant_failure(&error);
                    return Err(Box::new(error));
                }
            }
            if renderer.runtime.effect_trace.enabled() {
                renderer.runtime.effect_trace.pass_boundary(
                    "validate_begin",
                    pass,
                    graph,
                    textures,
                    pass_trace_summary(
                        renderer,
                        graph,
                        pass,
                        demand,
                        &execution_damage.region,
                        scene_work_rects,
                        framebuffer_origin,
                        scene_baseline_authority,
                        debug_config,
                        capture_plan,
                    ),
                );
            }
            let validation_result = validate_effect_pass_resources(
                renderer,
                graph,
                pass,
                textures,
                &execution_damage.region,
                framebuffer_origin,
            );
            if renderer.runtime.effect_trace.enabled() {
                renderer.runtime.effect_trace.pass_boundary(
                    "validate_end",
                    pass,
                    graph,
                    textures,
                    pass_trace_summary(
                        renderer,
                        graph,
                        pass,
                        demand,
                        &execution_damage.region,
                        scene_work_rects,
                        framebuffer_origin,
                        scene_baseline_authority,
                        debug_config,
                        capture_plan,
                    ),
                );
            }
            if let Err(error) = validation_result {
                renderer.runtime.effect_trace.invariant_failure(&error);
                return Err(Box::new(error));
            }
            #[cfg(any(debug_assertions, test))]
            if let Err(error) = validate_current_frame_input_regions(
                graph,
                pass,
                &execution_damage.region,
                &valid_regions,
            ) {
                renderer.runtime.effect_trace.invariant_failure(&error);
                return Err(Box::new(error));
            }
            if renderer.runtime.effect_trace.enabled() {
                renderer.runtime.effect_trace.pass_boundary(
                    "execute_begin",
                    pass,
                    graph,
                    textures,
                    pass_trace_summary(
                        renderer,
                        graph,
                        pass,
                        demand,
                        &execution_damage.region,
                        scene_work_rects,
                        framebuffer_origin,
                        scene_baseline_authority,
                        debug_config,
                        capture_plan,
                    ),
                );
            }
            let pass_timing = graph_scope.and_then(|scope| {
                if renderer.runtime.capture_in_progress {
                    return None;
                }
                let output_target = pass.output.and_then(|output_id| {
                    graph
                        .textures
                        .iter()
                        .find(|texture| texture.id == output_id)
                });
                let damage_bbox_pixels =
                    execution_damage.region.bounding_rect().map_or(0, |rect| {
                        u64::from(rect.width).saturating_mul(u64::from(rect.height))
                    });
                let work = PassTimingWork {
                    effect_pixels: effect_region_pixels(&execution_damage.region),
                    damage_rect_count: execution_damage.region.rects().len(),
                    damage_bbox_pixels,
                    target_width: output_target.map_or(0, |texture| texture.width),
                    target_height: output_target.map_or(0, |texture| texture.height),
                };
                let capture_metadata = capture_timing_metadata(pass, capture_plan);
                renderer.runtime.effect_gpu_profiler.begin_pass(
                    renderer.gl,
                    scope,
                    u64::from(pass.id.get()),
                    pass.instance.get(),
                    pass.kind,
                    work,
                    capture_metadata,
                )
            });
            let execute_host_start = pass_timing
                .filter(|span| span.host_timing_enabled())
                .map(|_| Instant::now());
            let execute_result = execute_pass(
                renderer,
                graph,
                textures,
                pass,
                targets,
                framebuffer_origin,
                &execution_damage.region,
                scene_baseline_authority,
                debug_config,
                capture_plan,
                &checkpoint_causal_stability,
                graph_scope.is_some(),
                fusion_plan
                    .by_capture
                    .get(&pass.id)
                    .or_else(|| fusion_plan.by_consumer.get(&pass.id))
                    .copied(),
                &mut stats,
            );
            let execute_host_ns = monotonic_elapsed_ns(execute_host_start);
            let replay_execution = execute_result.as_ref().ok().copied().flatten();
            finalize_pass_timing_and_replay_detail(
                || {
                    renderer.runtime.effect_gpu_profiler.end_pass(
                        renderer.gl,
                        pass_timing,
                        replay_execution,
                        execute_host_ns,
                    );
                },
                replay_execution,
                |detail| stats.record_replay_capture_detail(detail),
            );
            if renderer.runtime.effect_trace.enabled() {
                renderer.runtime.effect_trace.pass_boundary(
                    "execute_end",
                    pass,
                    graph,
                    textures,
                    pass_trace_summary(
                        renderer,
                        graph,
                        pass,
                        demand,
                        &execution_damage.region,
                        scene_work_rects,
                        framebuffer_origin,
                        scene_baseline_authority,
                        debug_config,
                        capture_plan,
                    ),
                );
            }
            if let Err(error) = execute_result {
                if let Some(invariant) = error.downcast_ref::<EffectExecutionInvariantError>() {
                    renderer.runtime.effect_trace.invariant_failure(invariant);
                }
                return Err(error);
            }
            if let Some(fusion) = fusion_plan.by_consumer.get(&pass.id) {
                debug_assert_eq!(fusion.consumer_pass, pass.id);
                stats.capture_downsample_fusion_executed =
                    stats.capture_downsample_fusion_executed.saturating_add(1);
                stats.capture_downsample_fusion_output_pixels = stats
                    .capture_downsample_fusion_output_pixels
                    .saturating_add(effect_region_pixels(&execution_damage.region));
            }
            if is_direct_framebuffer_capture(pass, scene_baseline_authority, debug_config) {
                let capture_satisfied = scene_work_state.mark_capture_satisfied(pass.id);
                if capture_satisfied && renderer.runtime.effect_trace.enabled() {
                    let metrics = scene_work_state.work_trace_metrics();
                    renderer.runtime.effect_trace.scene_replay_boundary(
                        "capture_satisfied",
                        pass,
                        "capture_satisfied",
                        scene_cursor,
                        scene_cursor,
                        metrics.0,
                        metrics.1,
                        metrics.2,
                        metrics.3,
                        metrics.4,
                    );
                }
            }
            if matches!(
                pass.kind,
                RenderPassKind::Composite | RenderPassKind::OutputPostProcess
            ) {
                effect_valid_regions
                    .entry(pass.instance)
                    .and_modify(|valid| *valid = valid.union(&execution_damage.region))
                    .or_insert_with(|| execution_damage.region.clone());
            }
            #[cfg(any(debug_assertions, test))]
            {
                let output_region = record_current_frame_output_region(
                    renderer,
                    graph,
                    pass,
                    &execution_damage.region,
                    scene_baseline_authority,
                    debug_config,
                )
                .map_err(|error| {
                    renderer.runtime.effect_trace.invariant_failure(&error);
                    Box::new(error) as Box<dyn std::error::Error>
                })?;
                valid_regions.insert(pass.output.expect("validated pass output"), output_region);
            }
            if renderer.runtime.effect_trace.enabled() {
                renderer.runtime.effect_trace.pass_boundary(
                    "end",
                    pass,
                    graph,
                    textures,
                    pass_trace_summary(
                        renderer,
                        graph,
                        pass,
                        demand,
                        &execution_damage.region,
                        scene_work_rects,
                        framebuffer_origin,
                        scene_baseline_authority,
                        debug_config,
                        capture_plan,
                    ),
                );
            }
            release_dead_graph_textures(
                &mut renderer.runtime.effect_resources,
                graph,
                pass.id,
                textures,
            )?;
            stats.passes = stats.passes.saturating_add(1);
        }
        if scene_work_state.pending_checkpoint_requirements() != 0 {
            let error = EffectExecutionInvariantError::PendingSceneCheckpointRequirements(
                scene_work_state.pending_capture_passes.clone(),
            );
            renderer.runtime.effect_trace.invariant_failure(&error);
            #[cfg(any(debug_assertions, test))]
            return Err(Box::new(error));
            #[cfg(not(any(debug_assertions, test)))]
            scene_work_state.force_baseline();
        }
        let final_scene_cursor_end = renderer.scene.commands.len();
        if renderer.runtime.effect_trace.enabled() {
            let metrics = scene_work_state.work_trace_metrics();
            renderer.runtime.effect_trace.final_scene_replay_boundary(
                "begin",
                scene_cursor,
                final_scene_cursor_end,
                metrics.0,
                metrics.1,
                metrics.2,
                metrics.3,
                metrics.4,
            );
        }
        renderer.draw_effect_scene_range(
            scene_work_state.active_work(),
            scene_cursor,
            final_scene_cursor_end,
            framebuffer_origin,
            targets.composition_draw,
        )?;
        if renderer.runtime.effect_trace.enabled() {
            let metrics = scene_work_state.work_trace_metrics();
            renderer.runtime.effect_trace.final_scene_replay_boundary(
                "end",
                scene_cursor,
                final_scene_cursor_end,
                metrics.0,
                metrics.1,
                metrics.2,
                metrics.3,
                metrics.4,
            );
        }
        if let Some(preservation) = scene_work_preservation.take() {
            let restore_result = restore_scene_work_preservation_context(renderer, &preservation);
            let release_result = renderer
                .runtime
                .effect_resources
                .release(preservation.texture);
            restore_result?;
            release_result?;
        }
        stats.instances = selection.executed_instances.len();
        stats.scene_replay_work_overflow_fallbacks =
            scene_work_state.normalization_overflow_fallbacks;
        Ok(stats)
    })();
    let cleanup_result = if let Some(preservation) = scene_work_preservation.take() {
        let restore_result = restore_scene_work_preservation_context(renderer, &preservation);
        let release_result = renderer
            .runtime
            .effect_resources
            .release(preservation.texture);
        restore_result.and(release_result.map_err(Into::into))
    } else {
        Ok(())
    };
    match (execution_result, cleanup_result) {
        (Ok(stats), Ok(())) => Ok(stats),
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
    }
}
