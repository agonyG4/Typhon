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
    let mut prepared = prepare_lifecycle_effect_graph_execution(
        renderer,
        graph,
        targets,
        framebuffer_origin,
        repaint_rects,
        demand,
        selection,
    )?;
    let result = execute_prepared_effect_graph_core(renderer, &mut prepared);
    if result.is_ok() {
        renderer.establish_effect_composition_state(targets.composition_draw);
    }
    finish_prepared_effect_graph_execution(renderer, prepared, result)
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
        textures: cached.bindings,
        checkpoint_cache_admission: cached.admission,
        checkpoint_preparations: cached.preparations,
        graph_scope: None,
        promote_checkpoint_cache: true,
        lifecycle_trace_summary: None,
        overlay_rects: Vec::new(),
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
