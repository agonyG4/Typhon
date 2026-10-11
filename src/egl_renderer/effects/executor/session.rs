use super::*;

/// Owns a graph's temporary bindings and pass preparation while allowing the
/// renderer to run nested lifecycle graphs between core execution and release.
#[must_use = "effect graph sessions must be finalized to release graph bindings"]
pub(in crate::egl_renderer) struct PreparedEffectGraphExecution<'a> {
    graph: &'a CompiledFrameGraph,
    framebuffer_origin: OutputFramebufferOrigin,
    repaint_plan: Option<&'a super::super::super::damage::RepaintPlan>,
    explicit_repaint_rects: Option<&'a [OutputRect]>,
    demand: &'a EffectExecutionDemand,
    selection: &'a EffectExecutionSelection,
    debug_config: EffectDebugConfig,
    scene_baseline_authority: SceneBaselineAuthority,
    scene_replay_work_mode_override: Option<SceneReplayWorkMode>,
    targets: EffectExecutionTargets,
    textures: HashMap<GraphTextureId, GraphTextureBinding>,
    checkpoint_cache_admission: CheckpointCacheAdmissionStats,
    checkpoint_preparations: HashMap<GraphTextureId, CheckpointCapturePreparation>,
    graph_scope: Option<super::super::gpu_timing::GraphTimingScope>,
    promote_checkpoint_cache: bool,
    lifecycle_trace_summary: Option<FrameTraceSummary>,
    overlay_rects: Vec<OutputRect>,
    skip_final_scene_replay: bool,
}

impl PreparedEffectGraphExecution<'_> {
    pub(in crate::egl_renderer) fn overlay_rects(&self) -> &[OutputRect] {
        &self.overlay_rects
    }

    pub(in crate::egl_renderer) const fn composition_target(&self) -> EffectFramebufferTarget {
        self.targets.composition_draw
    }

    pub(in crate::egl_renderer) const fn promotes_checkpoint_cache(&self) -> bool {
        self.promote_checkpoint_cache
    }
}

pub(super) fn prepare_checkpoint_cache_bindings(
    gl: &glow::Context,
    effect_resources: &mut super::super::resources::EffectGlResourceCache,
    graph: &CompiledFrameGraph,
    compatibility: CheckpointCacheCompatibility,
    active_output_texture_available: bool,
    debug_config: EffectDebugConfig,
) -> PreparedCheckpointCaptures {
    let candidates =
        checkpoint_capture_cache_candidates(graph, &debug_config, active_output_texture_available);
    let graph_peak_bytes = estimate_graph_peak_bytes(graph).ok();
    effect_resources.prepare_checkpoint_captures(gl, compatibility, graph_peak_bytes, &candidates)
}

pub(crate) fn checkpoint_capture_cache_candidates(
    graph: &CompiledFrameGraph,
    debug_config: &EffectDebugConfig,
    active_output_texture_available: bool,
) -> Vec<(
    super::super::resources::CheckpointCaptureCacheKey,
    oblivion_one::effects::GraphTexturePlan,
)> {
    let mut dependent_candidates = Vec::new();
    let mut replay_candidates = Vec::new();
    if debug_config.capture_mode() != EffectDebugCaptureMode::Replay {
        return dependent_candidates;
    }
    for pass in &graph.passes {
        if pass.kind != RenderPassKind::SceneCapture {
            continue;
        }
        let dependency_free = pass.checkpoint_dependencies.is_empty();
        let execution_plan = checkpoint_capture_execution_plan(
            pass.kind,
            pass.checkpoint_dependencies.len(),
            SceneBaselineAuthority::ReplayRequired,
            debug_config.capture_mode(),
            debug_config.checkpoint_capture_path(),
            active_output_texture_available,
        );
        let eligible = if dependency_free {
            execution_plan.executed == CaptureTimingMode::Replay
        } else {
            execution_plan.executed == CaptureTimingMode::FramebufferShaderCopy
        };
        if !eligible {
            continue;
        }
        let Some(key) = checkpoint_capture_cache_key(graph, pass) else {
            continue;
        };
        let Some(output) = pass.output else {
            continue;
        };
        let Some(plan) = graph.textures.iter().find(|texture| texture.id == output) else {
            continue;
        };
        let candidate = (key, plan.clone());
        if dependency_free {
            replay_candidates.push(candidate);
        } else {
            dependent_candidates.push(candidate);
        }
    }
    dependent_candidates.extend(replay_candidates);
    dependent_candidates
}

#[cfg(test)]
pub(crate) fn execute_effect_graph(
    renderer: &mut super::super::super::GlesSceneRenderer,
    graph: &CompiledFrameGraph,
    framebuffer_origin: OutputFramebufferOrigin,
    repaint_plan: &RepaintPlan,
    demand: &EffectExecutionDemand,
    selection: &EffectExecutionSelection,
) -> RendererResult<EffectExecutionStats> {
    renderer.execute_effect_graph_with_overlays_config(
        graph,
        framebuffer_origin,
        repaint_plan,
        demand,
        selection,
        *effect_debug_config(),
        None,
    )
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn execute_effect_graph_with_debug_config(
    renderer: &mut super::super::super::GlesSceneRenderer,
    graph: &CompiledFrameGraph,
    framebuffer_origin: OutputFramebufferOrigin,
    repaint_plan: &RepaintPlan,
    demand: &EffectExecutionDemand,
    selection: &EffectExecutionSelection,
    debug_config: EffectDebugConfig,
) -> RendererResult<EffectExecutionStats> {
    renderer.execute_effect_graph_with_overlays_config(
        graph,
        framebuffer_origin,
        repaint_plan,
        demand,
        selection,
        debug_config,
        None,
    )
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn execute_effect_graph_with_debug_config_and_scene_replay_mode(
    renderer: &mut super::super::super::GlesSceneRenderer,
    graph: &CompiledFrameGraph,
    framebuffer_origin: OutputFramebufferOrigin,
    repaint_plan: &RepaintPlan,
    demand: &EffectExecutionDemand,
    selection: &EffectExecutionSelection,
    debug_config: EffectDebugConfig,
    scene_replay_work_mode: SceneReplayWorkMode,
) -> RendererResult<EffectExecutionStats> {
    renderer.execute_effect_graph_with_overlays_config(
        graph,
        framebuffer_origin,
        repaint_plan,
        demand,
        selection,
        debug_config,
        Some(scene_replay_work_mode),
    )
}

pub(crate) fn execute_effect_graph_for_lifecycle(
    renderer: &mut EffectExecutionContext<'_>,
    graph: &CompiledFrameGraph,
    targets: EffectExecutionTargets,
    framebuffer_origin: OutputFramebufferOrigin,
    repaint_rects: &[OutputRect],
    demand: &EffectExecutionDemand,
    selection: &EffectExecutionSelection,
) -> RendererResult<EffectExecutionStats> {
    renderer.disable_lifecycle_replay();
    let mut prepared = prepare_lifecycle_effect_graph_execution(
        renderer,
        graph,
        targets,
        framebuffer_origin,
        repaint_rects,
        demand,
        selection,
        false,
    )?;
    let result = execute_prepared_effect_graph_core(renderer, &mut prepared);
    if result.is_ok() {
        renderer.establish_effect_composition_state(targets.composition_draw);
    }
    finish_prepared_effect_graph_execution(renderer, prepared, result)
}

pub(crate) fn execute_precomputed_backdrop_build(
    renderer: &mut EffectExecutionContext<'_>,
    build: &CompiledPrecomputedBackdropBuild,
    capture_texture: PooledEffectTexture,
    timing_scope: Option<super::super::gpu_timing::GraphTimingScope>,
    framebuffer_origin: OutputFramebufferOrigin,
) -> RendererResult<(EffectExecutionStats, PooledEffectTexture)> {
    let mut owned_textures = vec![capture_texture];
    let capture_pixels =
        u64::from(build.capture.width).saturating_mul(u64::from(build.capture.height));
    let mut stats = EffectExecutionStats {
        passes: 1,
        scene_captures: 1,
        capture_execution_pixels: capture_pixels,
        scene_capture_execution_pixels: capture_pixels,
        replay_capture_execution_pixels: capture_pixels,
        replay_capture_passes: 1,
        resource_acquisitions: 1,
        ..EffectExecutionStats::default()
    };

    let execution_result = (|| {
        if build.blur.passes != 2
            || build.capture.working_space
                != oblivion_one::effects::EffectWorkingSpace::OutputEncodedSrgb
            || build.working_space != oblivion_one::effects::EffectWorkingSpace::LinearSrgb
        {
            return Err(io::Error::other(
                "precomputed backdrop build does not match the canonical two-pass working spaces",
            )
            .into());
        }

        let mut input_layout = FullscreenTextureLayout {
            width: build.capture.width,
            height: build.capture.height,
            origin: oblivion_one::effects::GraphTextureOrigin::BottomLeft,
        };
        let mut input_is_linear = false;
        let mut output_index = 0;
        for (stage_index, stage) in build.stages.iter().copied().enumerate().skip(1) {
            let (kind, width, height) = match stage {
                oblivion_one::effects::PrecomputedBackdropBuildStage::KawaseDownsample {
                    width,
                    height,
                    ..
                } => (RenderPassKind::DualKawaseDownsample, width, height),
                oblivion_one::effects::PrecomputedBackdropBuildStage::KawaseUpsample {
                    width,
                    height,
                    ..
                } => (RenderPassKind::DualKawaseUpsample, width, height),
                oblivion_one::effects::PrecomputedBackdropBuildStage::SceneCaptureReplay => {
                    return Err(io::Error::other(
                        "precomputed backdrop build has an unexpected capture stage",
                    )
                    .into());
                }
            };
            let input_texture = renderer
                .runtime
                .effect_resources
                .texture(&owned_textures[output_index])
                .ok_or_else(|| io::Error::other("precomputed backdrop input is not realized"))?;
            let output = renderer.runtime.effect_resources.acquire(
                renderer.gl,
                EffectTextureKey::new(
                    width,
                    height,
                    EffectTextureFormat::Rgba16Float,
                    EffectTextureFilter::Linear,
                    build.working_space,
                ),
            )?;
            owned_textures.push(output);
            output_index = owned_textures.len() - 1;
            let output_layout = FullscreenTextureLayout {
                width,
                height,
                origin: oblivion_one::effects::GraphTextureOrigin::BottomLeft,
            };
            let pixels = u64::from(width).saturating_mul(u64::from(height));
            let work = PassTimingWork {
                effect_pixels: pixels,
                damage_rect_count: 1,
                damage_bbox_pixels: pixels,
                target_width: width,
                target_height: height,
            };
            let span = timing_scope.and_then(|scope| {
                renderer.begin_precomputed_build_stage(scope, stage_index, kind, work, None)
            });
            let shader_variant =
                u8::from(kind == RenderPassKind::DualKawaseDownsample && input_is_linear);
            let targets = EffectExecutionTargets::ordinary(EffectFramebufferTarget::new(
                renderer.scene.active_output_framebuffer,
            ));
            let draw_result = super::execute_kawase_texture_stage(
                renderer,
                input_texture,
                input_layout,
                &owned_textures[output_index],
                output_layout,
                kind,
                build.blur.radius,
                shader_variant,
                framebuffer_origin,
                None,
                targets,
            );
            renderer.end_precomputed_build_stage(span);
            draw_result?;

            stats.passes = stats.passes.saturating_add(1);
            stats.resource_acquisitions = stats.resource_acquisitions.saturating_add(1);
            match kind {
                RenderPassKind::DualKawaseDownsample => {
                    stats.blur_downsamples = stats.blur_downsamples.saturating_add(1);
                }
                RenderPassKind::DualKawaseUpsample => {
                    stats.blur_upsamples = stats.blur_upsamples.saturating_add(1);
                }
                _ => unreachable!(),
            }
            input_layout = output_layout;
            input_is_linear = true;
        }
        Ok(output_index)
    })();

    renderer.end_precomputed_build_scope(timing_scope);
    match execution_result {
        Ok(final_index) => {
            match release_precomputed_owned(owned_textures, Some(final_index), |texture| {
                renderer.runtime.effect_resources.release(texture)
            }) {
                Ok(Some(texture)) => Ok((stats, texture)),
                Ok(None) => {
                    Err(io::Error::other("precomputed backdrop output was not retained").into())
                }
                Err(error) => Err(error.into()),
            }
        }
        Err(error) => {
            let _ = release_precomputed_owned(owned_textures, None, |texture| {
                renderer.runtime.effect_resources.release(texture)
            });
            Err(error)
        }
    }
}

fn release_precomputed_owned<T, E: Copy>(
    textures: Vec<T>,
    retained_index: Option<usize>,
    mut release: impl FnMut(T) -> Result<(), E>,
) -> Result<Option<T>, E> {
    let mut retained = None;
    let mut first_error = None;
    for (index, texture) in textures.into_iter().enumerate() {
        if Some(index) == retained_index {
            retained = Some(texture);
        } else if let Err(error) = release(texture) {
            first_error.get_or_insert(error);
        }
    }
    if let Some(error) = first_error {
        if let Some(texture) = retained.take() {
            let _ = release(texture);
        }
        Err(error)
    } else {
        Ok(retained)
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_effect_graph_execution<'a>(
    context: &mut EffectExecutionContext<'_>,
    graph: &'a CompiledFrameGraph,
    framebuffer_origin: OutputFramebufferOrigin,
    repaint_plan: &'a crate::egl_renderer::damage::RepaintPlan,
    demand: &'a EffectExecutionDemand,
    selection: &'a EffectExecutionSelection,
    debug_config: EffectDebugConfig,
    scene_replay_work_mode_override: Option<SceneReplayWorkMode>,
) -> RendererResult<PreparedEffectGraphExecution<'a>> {
    prepare_effect_graph_execution_with_external_bindings(
        context,
        graph,
        framebuffer_origin,
        repaint_plan,
        demand,
        selection,
        debug_config,
        scene_replay_work_mode_override,
        HashMap::new(),
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_effect_graph_execution_with_external_bindings<'a>(
    context: &mut EffectExecutionContext<'_>,
    graph: &'a CompiledFrameGraph,
    framebuffer_origin: OutputFramebufferOrigin,
    repaint_plan: &'a crate::egl_renderer::damage::RepaintPlan,
    demand: &'a EffectExecutionDemand,
    selection: &'a EffectExecutionSelection,
    debug_config: EffectDebugConfig,
    scene_replay_work_mode_override: Option<SceneReplayWorkMode>,
    external_bindings: HashMap<GraphTextureId, GraphTextureBinding>,
) -> RendererResult<PreparedEffectGraphExecution<'a>> {
    let compatibility = CheckpointCacheCompatibility {
        output_size: context.scene.current_size,
        framebuffer_origin_top_left: framebuffer_origin == OutputFramebufferOrigin::TopLeftScanout,
        effect_registry_generation: context.runtime.effect_registry_generation,
    };
    let cached = prepare_checkpoint_cache_bindings(
        context.gl,
        &mut context.runtime.effect_resources,
        graph,
        compatibility,
        context.scene.active_output_texture.is_some(),
        debug_config,
    );
    let trace_summary = effect_trace_summary(context, graph, Some(repaint_plan), selection);
    context
        .runtime
        .effect_trace
        .frame_boundary("effect_resource_sync", "begin", trace_summary);
    context
        .runtime
        .effect_trace
        .frame_boundary("effect_graph_execute", "begin", trace_summary);
    let mut textures = cached.bindings;
    textures.extend(external_bindings);
    Ok(PreparedEffectGraphExecution {
        graph,
        framebuffer_origin,
        repaint_plan: Some(repaint_plan),
        explicit_repaint_rects: None,
        demand,
        selection,
        debug_config,
        scene_baseline_authority: SceneBaselineAuthority::ReplayRequired,
        scene_replay_work_mode_override,
        targets: EffectExecutionTargets::ordinary(EffectFramebufferTarget::new(
            context.scene.active_output_framebuffer,
        )),
        textures,
        checkpoint_cache_admission: cached.admission,
        checkpoint_preparations: cached.preparations,
        graph_scope: None,
        promote_checkpoint_cache: true,
        lifecycle_trace_summary: None,
        overlay_rects: Vec::new(),
        skip_final_scene_replay: false,
    })
}

pub(crate) fn prepare_lifecycle_effect_graph_execution<'a>(
    context: &mut EffectExecutionContext<'_>,
    graph: &'a CompiledFrameGraph,
    targets: EffectExecutionTargets,
    framebuffer_origin: OutputFramebufferOrigin,
    repaint_rects: &'a [OutputRect],
    demand: &'a EffectExecutionDemand,
    selection: &'a EffectExecutionSelection,
    skip_final_scene_replay: bool,
) -> RendererResult<PreparedEffectGraphExecution<'a>> {
    let trace_summary = FrameTraceSummary {
        selected_effect_count: Some(selection.executed_instances.len()),
        graph_pass_count: Some(graph.stats.passes),
        graph_texture_count: Some(graph.stats.textures),
        peak_live_intermediate_count: Some(graph.stats.peak_live_intermediates),
        ..FrameTraceSummary::default()
    };
    context
        .runtime
        .effect_trace
        .frame_boundary("effect_resource_sync", "begin", trace_summary);
    context
        .runtime
        .effect_trace
        .frame_boundary("effect_graph_execute", "begin", trace_summary);
    Ok(PreparedEffectGraphExecution {
        graph,
        framebuffer_origin,
        repaint_plan: None,
        explicit_repaint_rects: Some(repaint_rects),
        demand,
        selection,
        debug_config: *effect_debug_config(),
        scene_baseline_authority: SceneBaselineAuthority::PrecomposedFramebuffer,
        scene_replay_work_mode_override: None,
        targets,
        textures: HashMap::new(),
        checkpoint_cache_admission: CheckpointCacheAdmissionStats::default(),
        checkpoint_preparations: HashMap::new(),
        graph_scope: None,
        promote_checkpoint_cache: false,
        lifecycle_trace_summary: Some(trace_summary),
        overlay_rects: Vec::new(),
        skip_final_scene_replay,
    })
}

pub(crate) fn execute_prepared_effect_graph_core(
    context: &mut EffectExecutionContext<'_>,
    prepared: &mut PreparedEffectGraphExecution<'_>,
) -> RendererResult<EffectExecutionStats> {
    let fusion_plan = plan_capture_downsample_fusions(
        prepared.graph,
        prepared.selection,
        &prepared.checkpoint_preparations,
        context.scene.active_output_texture.is_some(),
        context.scene.current_size,
        prepared.framebuffer_origin,
        prepared.scene_baseline_authority,
        prepared.debug_config,
    );
    let graph_scope = (!context.runtime.capture_in_progress)
        .then(|| {
            context
                .runtime
                .effect_gpu_profiler
                .begin_graph(context.gl, context.runtime.effect_trace.frame_id())
        })
        .flatten();
    prepared.graph_scope = graph_scope;
    let mut result = execute_graph_passes_inner(
        context,
        prepared.graph,
        &mut prepared.textures,
        prepared.targets,
        prepared.framebuffer_origin,
        prepared.repaint_plan,
        prepared.explicit_repaint_rects,
        prepared.demand,
        prepared.selection,
        prepared.debug_config,
        prepared.scene_baseline_authority,
        prepared.scene_replay_work_mode_override,
        graph_scope,
        &fusion_plan,
        &mut prepared.overlay_rects,
        prepared.skip_final_scene_replay,
    );
    if let Ok(stats) = &mut result {
        stats.checkpoint_cache_admission = prepared.checkpoint_cache_admission;
    }
    result
}

pub(crate) fn finish_prepared_effect_graph_execution(
    context: &mut EffectExecutionContext<'_>,
    mut prepared: PreparedEffectGraphExecution<'_>,
    mut execution_result: RendererResult<EffectExecutionStats>,
) -> RendererResult<EffectExecutionStats> {
    let graph_timing_finished = context
        .runtime
        .effect_gpu_profiler
        .end_graph(context.gl, prepared.graph_scope);
    if graph_timing_finished && let Ok(stats) = &mut execution_result {
        let (entries, bytes) = context.runtime.effect_resources.checkpoint_cache_stats();
        stats.checkpoint_cache_entries = entries;
        stats.checkpoint_cache_bytes = bytes;
        context
            .runtime
            .effect_gpu_profiler
            .attach_capture_execution_summary(prepared.graph_scope, stats.capture_timing_summary());
    }

    let trace_summary = prepared.lifecycle_trace_summary.unwrap_or_else(|| {
        effect_trace_summary(
            context,
            prepared.graph,
            prepared.repaint_plan,
            prepared.selection,
        )
    });
    context
        .runtime
        .effect_trace
        .frame_boundary("effect_graph_execute", "end", trace_summary);
    context
        .runtime
        .effect_trace
        .frame_boundary("effect_resource_sync", "end", trace_summary);
    if execution_result.is_err() {
        if prepared.promote_checkpoint_cache {
            context.establish_ordinary_scene_state();
        } else {
            context.establish_effect_composition_state(prepared.targets.composition_draw);
        }
        unsafe { context.gl.bind_texture(glow::TEXTURE_2D, None) };
    }
    context
        .runtime
        .effect_trace
        .frame_boundary("effect_graph_release", "begin", trace_summary);
    let release_result = context
        .runtime
        .effect_resources
        .release_graph(std::mem::take(&mut prepared.textures));
    context
        .runtime
        .effect_trace
        .frame_boundary("effect_graph_release", "end", trace_summary);
    match (execution_result, release_result) {
        (Ok(stats), Ok(())) => Ok(stats),
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
    }
}

#[cfg(test)]
mod precomputed_build_ownership_tests {
    use super::release_precomputed_owned;

    #[test]
    fn successful_build_releases_intermediates_and_returns_one_frame_owner() {
        let mut released = Vec::new();
        let owner = release_precomputed_owned(vec![10, 11, 12, 13, 14], Some(4), |id| {
            released.push(id);
            Ok::<(), ()>(())
        })
        .unwrap();

        assert_eq!(owner, Some(14));
        assert_eq!(released, [10, 11, 12, 13]);
    }

    #[test]
    fn partial_build_failure_releases_each_allocated_texture_once() {
        let mut released = Vec::new();
        let owner = release_precomputed_owned(vec![20, 21, 22], None, |id| {
            released.push(id);
            Ok::<(), ()>(())
        })
        .unwrap();

        assert_eq!(owner, None);
        assert_eq!(released, [20, 21, 22]);
    }

    #[test]
    fn cleanup_attempts_every_texture_and_releases_retained_output_on_error() {
        let mut released = Vec::new();
        let result = release_precomputed_owned(vec![30, 31, 32], Some(2), |id| {
            released.push(id);
            (id != 30).then_some(()).ok_or(())
        });

        assert_eq!(result, Err(()));
        assert_eq!(released, [30, 31, 32]);
    }
}
