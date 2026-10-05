use super::*;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct EffectExecutionStats {
    pub instances: usize,
    pub passes: usize,
    pub scene_captures: usize,
    /// Physical target-texture pixels covered by executed capture regions.
    /// This is execution work, not the allocated texture area.
    pub capture_execution_pixels: u64,
    pub scene_capture_execution_pixels: u64,
    pub surface_capture_execution_pixels: u64,
    pub replay_capture_execution_pixels: u64,
    pub framebuffer_capture_execution_pixels: u64,
    pub framebuffer_shader_copy_capture_execution_pixels: u64,
    pub checkpoint_capture_execution_pixels: u64,
    pub replay_capture_passes: usize,
    pub framebuffer_capture_passes: usize,
    pub checkpoint_capture_passes: usize,
    pub replay_capture_commands: usize,
    pub checkpoint_dependency_edges: usize,
    pub replay_capture_materialization_rects: usize,
    pub replay_capture_execution_regions: usize,
    pub replay_capture_disjoint_overflows: usize,
    pub scene_replay_work_overflow_fallbacks: usize,
    pub replay_capture_command_region_pairs: usize,
    pub replay_capture_scene_scan_pairs: usize,
    pub replay_capture_planner_commands_visited: usize,
    pub replay_capture_planner_commands_drawable: usize,
    pub replay_capture_commands_executed: usize,
    pub replay_capture_draw_calls: usize,
    pub replay_capture_host_cpu_ns: u64,
    pub replay_capture_selection_cpu_ns: u64,
    pub replay_capture_visibility_cpu_ns: u64,
    pub replay_capture_draw_submit_cpu_ns: u64,
    pub checkpoint_cache_hits: usize,
    pub checkpoint_cache_full_refreshes: usize,
    pub checkpoint_cache_zero_copy_hits: usize,
    pub checkpoint_cache_update_pixels: u64,
    pub checkpoint_cache_domain_pixels: u64,
    pub checkpoint_cache_saved_pixels: u64,
    /// Dependency-free Replay SceneCapture counters. Hits count consecutive
    /// cache populations; full refreshes count full-domain physical replays.
    pub replay_capture_cache_hits: usize,
    pub replay_capture_cache_full_refreshes: usize,
    /// Zero-copy hits are a subset of replay_capture_cache_hits.
    pub replay_capture_cache_zero_copy_hits: usize,
    /// Physical pixels written by dependency-free Replay cache refreshes.
    pub replay_capture_cache_update_pixels: u64,
    /// Capture-domain pixels considered by dependency-free persistent Replay.
    pub replay_capture_cache_domain_pixels: u64,
    /// Domain pixels minus physical update pixels for dependency-free Replay.
    pub replay_capture_cache_saved_pixels: u64,
    pub checkpoint_causal_proven_unchanged: usize,
    pub checkpoint_causal_unproven: usize,
    pub checkpoint_causal_dependency_changed: usize,
    pub checkpoint_causal_scene_prefix_changed: usize,
    pub checkpoint_cache_entries: usize,
    pub checkpoint_cache_bytes: u64,
    pub checkpoint_cache_admission: CheckpointCacheAdmissionStats,
    pub capture_downsample_fusion_candidates: usize,
    pub capture_downsample_fusion_executed: usize,
    pub capture_downsample_fusion_elided_capture_pixels: u64,
    pub capture_downsample_fusion_output_pixels: u64,
    pub capture_downsample_fusion_ineligible: usize,
    pub blur_downsamples: usize,
    pub blur_upsamples: usize,
    pub composites: usize,
    pub resource_acquisitions: usize,
}

impl EffectExecutionStats {
    pub(super) fn capture_timing_summary(&self) -> CaptureExecutionTimingSummary {
        CaptureExecutionTimingSummary {
            capture_execution_pixels: self.capture_execution_pixels,
            scene_capture_execution_pixels: self.scene_capture_execution_pixels,
            surface_capture_execution_pixels: self.surface_capture_execution_pixels,
            replay_capture_execution_pixels: self.replay_capture_execution_pixels,
            framebuffer_capture_execution_pixels: self.framebuffer_capture_execution_pixels,
            framebuffer_shader_copy_capture_execution_pixels: self
                .framebuffer_shader_copy_capture_execution_pixels,
            checkpoint_capture_execution_pixels: self.checkpoint_capture_execution_pixels,
            replay_capture_passes: self.replay_capture_passes,
            framebuffer_capture_passes: self.framebuffer_capture_passes,
            checkpoint_capture_passes: self.checkpoint_capture_passes,
            replay_capture_commands: self.replay_capture_commands,
            checkpoint_dependency_edges: self.checkpoint_dependency_edges,
            replay_capture_materialization_rects: self.replay_capture_materialization_rects,
            replay_capture_execution_regions: self.replay_capture_execution_regions,
            replay_capture_disjoint_overflows: self.replay_capture_disjoint_overflows,
            replay_capture_command_region_pairs: self.replay_capture_command_region_pairs,
            replay_capture_scene_scan_pairs: self.replay_capture_scene_scan_pairs,
            replay_capture_planner_commands_visited: self.replay_capture_planner_commands_visited,
            replay_capture_planner_commands_drawable: self.replay_capture_planner_commands_drawable,
            replay_capture_commands_executed: self.replay_capture_commands_executed,
            replay_capture_draw_calls: self.replay_capture_draw_calls,
            replay_capture_host_cpu_ns: self.replay_capture_host_cpu_ns,
            replay_capture_selection_cpu_ns: self.replay_capture_selection_cpu_ns,
            replay_capture_visibility_cpu_ns: self.replay_capture_visibility_cpu_ns,
            replay_capture_draw_submit_cpu_ns: self.replay_capture_draw_submit_cpu_ns,
            checkpoint_cache_hits: self.checkpoint_cache_hits,
            checkpoint_cache_full_refreshes: self.checkpoint_cache_full_refreshes,
            checkpoint_cache_zero_copy_hits: self.checkpoint_cache_zero_copy_hits,
            checkpoint_cache_update_pixels: self.checkpoint_cache_update_pixels,
            checkpoint_cache_domain_pixels: self.checkpoint_cache_domain_pixels,
            checkpoint_cache_saved_pixels: self.checkpoint_cache_saved_pixels,
            replay_capture_cache_hits: self.replay_capture_cache_hits,
            replay_capture_cache_full_refreshes: self.replay_capture_cache_full_refreshes,
            replay_capture_cache_zero_copy_hits: self.replay_capture_cache_zero_copy_hits,
            replay_capture_cache_update_pixels: self.replay_capture_cache_update_pixels,
            replay_capture_cache_domain_pixels: self.replay_capture_cache_domain_pixels,
            replay_capture_cache_saved_pixels: self.replay_capture_cache_saved_pixels,
            checkpoint_causal_proven_unchanged: self.checkpoint_causal_proven_unchanged,
            checkpoint_causal_unproven: self.checkpoint_causal_unproven,
            checkpoint_causal_dependency_changed: self.checkpoint_causal_dependency_changed,
            checkpoint_causal_scene_prefix_changed: self.checkpoint_causal_scene_prefix_changed,
            checkpoint_cache_entries: self.checkpoint_cache_entries,
            checkpoint_cache_bytes: self.checkpoint_cache_bytes,
            checkpoint_cache_admission: self.checkpoint_cache_admission,
            capture_downsample_fusion_candidates: self.capture_downsample_fusion_candidates,
            capture_downsample_fusion_executed: self.capture_downsample_fusion_executed,
            capture_downsample_fusion_elided_capture_pixels: self
                .capture_downsample_fusion_elided_capture_pixels,
            capture_downsample_fusion_output_pixels: self.capture_downsample_fusion_output_pixels,
            capture_downsample_fusion_ineligible: self.capture_downsample_fusion_ineligible,
        }
    }

    pub(super) fn record_replay_capture_detail(&mut self, detail: ReplayCaptureExecutionDetail) {
        self.replay_capture_materialization_rects = self
            .replay_capture_materialization_rects
            .saturating_add(detail.materialization_rects);
        self.replay_capture_execution_regions = self
            .replay_capture_execution_regions
            .saturating_add(detail.execution_regions);
        self.replay_capture_disjoint_overflows = self
            .replay_capture_disjoint_overflows
            .saturating_add(usize::from(detail.disjoint_overflowed));
        self.replay_capture_command_region_pairs = self
            .replay_capture_command_region_pairs
            .saturating_add(detail.command_region_pairs);
        self.replay_capture_scene_scan_pairs = self
            .replay_capture_scene_scan_pairs
            .saturating_add(detail.scene_scan_pairs);
        self.replay_capture_planner_commands_visited = self
            .replay_capture_planner_commands_visited
            .saturating_add(detail.planner_commands_visited);
        self.replay_capture_planner_commands_drawable = self
            .replay_capture_planner_commands_drawable
            .saturating_add(detail.planner_commands_drawable);
        self.replay_capture_commands_executed = self
            .replay_capture_commands_executed
            .saturating_add(detail.commands_executed);
        self.replay_capture_draw_calls = self
            .replay_capture_draw_calls
            .saturating_add(detail.draw_calls);
        self.replay_capture_host_cpu_ns = self
            .replay_capture_host_cpu_ns
            .saturating_add(detail.host_cpu_ns);
        self.replay_capture_selection_cpu_ns = self
            .replay_capture_selection_cpu_ns
            .saturating_add(detail.selection_cpu_ns);
        self.replay_capture_visibility_cpu_ns = self
            .replay_capture_visibility_cpu_ns
            .saturating_add(detail.visibility_cpu_ns);
        self.replay_capture_draw_submit_cpu_ns = self
            .replay_capture_draw_submit_cpu_ns
            .saturating_add(detail.draw_submit_cpu_ns);
    }

    pub(super) fn record_capture_execution(
        &mut self,
        pass: &CompiledRenderPass,
        direct_capture: bool,
        physical_pixels: u64,
        replay_commands: usize,
    ) {
        self.record_capture_execution_with_mode(
            pass,
            if direct_capture {
                CaptureTimingMode::FramebufferBlit
            } else {
                CaptureTimingMode::Replay
            },
            physical_pixels,
            replay_commands,
        );
    }

    pub(super) fn record_capture_execution_with_mode(
        &mut self,
        pass: &CompiledRenderPass,
        mode: CaptureTimingMode,
        physical_pixels: u64,
        replay_commands: usize,
    ) {
        self.capture_execution_pixels = self
            .capture_execution_pixels
            .saturating_add(physical_pixels);
        match pass.kind {
            RenderPassKind::SceneCapture => {
                self.scene_capture_execution_pixels = self
                    .scene_capture_execution_pixels
                    .saturating_add(physical_pixels);
            }
            RenderPassKind::SurfaceCapture => {
                self.surface_capture_execution_pixels = self
                    .surface_capture_execution_pixels
                    .saturating_add(physical_pixels);
            }
            _ => return,
        }
        match mode {
            CaptureTimingMode::Replay => {
                self.replay_capture_execution_pixels = self
                    .replay_capture_execution_pixels
                    .saturating_add(physical_pixels);
                self.replay_capture_passes = self.replay_capture_passes.saturating_add(1);
                self.replay_capture_commands =
                    self.replay_capture_commands.saturating_add(replay_commands);
            }
            CaptureTimingMode::FramebufferBlit => {
                self.framebuffer_capture_execution_pixels = self
                    .framebuffer_capture_execution_pixels
                    .saturating_add(physical_pixels);
                self.framebuffer_capture_passes = self.framebuffer_capture_passes.saturating_add(1);
            }
            CaptureTimingMode::FramebufferShaderCopy => {
                self.framebuffer_capture_execution_pixels = self
                    .framebuffer_capture_execution_pixels
                    .saturating_add(physical_pixels);
                self.framebuffer_shader_copy_capture_execution_pixels = self
                    .framebuffer_shader_copy_capture_execution_pixels
                    .saturating_add(physical_pixels);
                self.framebuffer_capture_passes = self.framebuffer_capture_passes.saturating_add(1);
            }
        }
        let checkpoint_count = pass.checkpoint_dependencies.len();
        if checkpoint_count > 0 {
            self.checkpoint_capture_execution_pixels = self
                .checkpoint_capture_execution_pixels
                .saturating_add(physical_pixels);
            self.checkpoint_capture_passes = self.checkpoint_capture_passes.saturating_add(1);
            self.checkpoint_dependency_edges = self
                .checkpoint_dependency_edges
                .saturating_add(checkpoint_count);
        }
    }

    pub(super) fn record_dependency_free_replay_cache_decision(
        &mut self,
        consecutive_cache_hit: bool,
        full_refresh: bool,
        zero_copy: bool,
        domain_pixels: u64,
        update_pixels: u64,
    ) {
        if consecutive_cache_hit {
            self.checkpoint_cache_hits = self.checkpoint_cache_hits.saturating_add(1);
            self.replay_capture_cache_hits = self.replay_capture_cache_hits.saturating_add(1);
        } else {
            self.checkpoint_cache_full_refreshes =
                self.checkpoint_cache_full_refreshes.saturating_add(1);
        }
        if zero_copy {
            self.checkpoint_cache_zero_copy_hits =
                self.checkpoint_cache_zero_copy_hits.saturating_add(1);
            self.replay_capture_cache_zero_copy_hits =
                self.replay_capture_cache_zero_copy_hits.saturating_add(1);
        }
        if full_refresh {
            self.replay_capture_cache_full_refreshes =
                self.replay_capture_cache_full_refreshes.saturating_add(1);
        }
        self.checkpoint_cache_domain_pixels = self
            .checkpoint_cache_domain_pixels
            .saturating_add(domain_pixels);
        self.checkpoint_cache_update_pixels = self
            .checkpoint_cache_update_pixels
            .saturating_add(update_pixels);
        self.checkpoint_cache_saved_pixels = self
            .checkpoint_cache_saved_pixels
            .saturating_add(domain_pixels.saturating_sub(update_pixels));
        self.replay_capture_cache_domain_pixels = self
            .replay_capture_cache_domain_pixels
            .saturating_add(domain_pixels);
        self.replay_capture_cache_update_pixels = self
            .replay_capture_cache_update_pixels
            .saturating_add(update_pixels);
        self.replay_capture_cache_saved_pixels = self
            .replay_capture_cache_saved_pixels
            .saturating_add(domain_pixels.saturating_sub(update_pixels));
    }
}

pub(super) fn effect_trace_summary(
    renderer: &EffectExecutionContext<'_>,
    graph: &CompiledFrameGraph,
    repaint_plan: Option<&crate::egl_renderer::damage::RepaintPlan>,
    selection: &EffectExecutionSelection,
) -> FrameTraceSummary {
    FrameTraceSummary {
        repaint_mode: repaint_plan.map(|plan| plan.mode.as_str()),
        render_damage_signature: repaint_plan.map(|plan| plan.render_damage.identity_signature()),
        repair_damage_signature: repaint_plan.map(|plan| plan.repair_damage.identity_signature()),
        visible_effect_count: Some(renderer.scene.frame_stats.effect_instances_visible),
        selected_effect_count: Some(selection.executed_instances.len()),
        graph_pass_count: Some(graph.stats.passes),
        graph_texture_count: Some(graph.stats.textures),
        peak_live_intermediate_count: Some(graph.stats.peak_live_intermediates),
        ..FrameTraceSummary::default()
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn pass_trace_summary(
    renderer: &EffectExecutionContext<'_>,
    graph: &CompiledFrameGraph,
    pass: &CompiledRenderPass,
    demand: &EffectExecutionDemand,
    execution_damage: &EffectRegion,
    scene_work_rects: &[OutputRect],
    framebuffer_origin: OutputFramebufferOrigin,
    scene_baseline_authority: SceneBaselineAuthority,
    debug_config: EffectDebugConfig,
    capture_plan: CheckpointCaptureExecutionPlan,
) -> PassTraceSummary {
    if !renderer.runtime.effect_trace.enabled() {
        return PassTraceSummary::default();
    }

    let input_flip_y = pass
        .inputs
        .first()
        .and_then(|input| graph.textures.iter().find(|texture| texture.id == *input))
        .is_some_and(|texture| effect_input_requires_sample_y_flip(texture.origin));
    let output_plan = pass
        .output
        .and_then(|output| graph.textures.iter().find(|texture| texture.id == output));
    let output_is_framebuffer =
        output_plan.is_some_and(|texture| texture.source == GraphTextureSource::Output);
    let target_flip_y =
        effect_target_requires_logical_y_flip(output_is_framebuffer, framebuffer_origin);
    let direct_capture =
        is_direct_framebuffer_capture(pass, scene_baseline_authority, debug_config);
    let conservative_pass_demand = direct_capture
        || demand.is_conservative_full()
        || demand.instance_is_conservative_full(pass.instance);
    let debug_full_kawase = debug_config.kawase_mode() == EffectDebugKawaseMode::Full
        && matches!(
            pass.kind,
            RenderPassKind::DualKawaseDownsample | RenderPassKind::DualKawaseUpsample
        );
    let conservative_pass_demand_kind = if direct_capture {
        "direct_framebuffer_capture"
    } else if debug_full_kawase {
        "debug_full_kawase"
    } else if conservative_pass_demand
        && matches!(
            pass.kind,
            RenderPassKind::Composite | RenderPassKind::OutputPostProcess
        )
    {
        "final_output_constrained"
    } else if conservative_pass_demand {
        "internal_full_domain"
    } else {
        "precise"
    };
    let capture_mode = matches!(
        pass.kind,
        RenderPassKind::SceneCapture | RenderPassKind::SurfaceCapture
    )
    .then_some(capture_plan.executed.as_str());
    let capture_command_count = if capture_mode == Some("replay") {
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
        Some(
            capture::indices_for_capture(
                &layers,
                &visual_groups,
                pass.anchor,
                pass.kind == RenderPassKind::SurfaceCapture,
                pass.visual_group,
                pass.anchor_scope,
            )
            .len(),
        )
    } else {
        None
    };
    let damage_bounding_box = execution_damage
        .bounding_rect()
        .map(|rect| (rect.x, rect.y, rect.width, rect.height));
    PassTraceSummary {
        target_flip_y,
        input_flip_y,
        framebuffer_origin: Some(match framebuffer_origin {
            OutputFramebufferOrigin::BottomLeft => "bottom_left",
            OutputFramebufferOrigin::TopLeftScanout => "top_left_scanout",
        }),
        damage_rect_count: execution_damage.rects().len(),
        damage_bounding_box,
        capture_mode,
        requested_capture_path: capture_plan.requested.map(CheckpointCapturePath::as_str),
        executed_capture_path: matches!(
            pass.kind,
            RenderPassKind::SceneCapture | RenderPassKind::SurfaceCapture
        )
        .then_some(capture_plan.executed.as_str()),
        capture_fallback_reason: capture_plan
            .fallback_reason
            .map(CapturePathFallbackReason::as_str),
        capture_command_count,
        read_framebuffer: direct_capture.then(|| {
            renderer.scene.active_output_framebuffer.map_or_else(
                || "default".to_owned(),
                |framebuffer| format!("{framebuffer:?}"),
            )
        }),
        draw_framebuffer: direct_capture
            .then(|| {
                renderer
                    .runtime
                    .effect_resources
                    .scratch_framebuffer_identity()
            })
            .flatten(),
        scratch_fbo_present: direct_capture.then(|| {
            renderer
                .runtime
                .effect_resources
                .scratch_framebuffer_identity()
                .is_some()
        }),
        conservative_pass_demand,
        conservative_pass_demand_kind,
        backdrop_capture_policy: Some(debug_config.capture_mode().as_str()),
        kawase_execution_policy: Some(debug_config.kawase_mode().as_str()),
        scene_work_damage_rects: scene_work_rects.len(),
        scene_work_damage_bounding_box: output_rects_bounding_box(scene_work_rects),
    }
}

pub(super) fn ensure_pass_textures(
    renderer: &mut EffectExecutionContext<'_>,
    graph: &CompiledFrameGraph,
    pass: &CompiledRenderPass,
    textures: &mut std::collections::HashMap<GraphTextureId, GraphTextureBinding>,
    stats: &mut EffectExecutionStats,
) -> RendererResult<()> {
    for texture_id in pass.inputs.iter().copied().chain(pass.output) {
        let plan = graph_texture(graph, texture_id)?;
        if plan.source == GraphTextureSource::Output || textures.contains_key(&texture_id) {
            continue;
        }
        let realized = renderer
            .runtime
            .effect_resources
            .acquire_plan(renderer.gl, plan)?;
        textures.insert(texture_id, GraphTextureBinding::transient(realized));
        stats.resource_acquisitions = stats.resource_acquisitions.saturating_add(1);
    }
    Ok(())
}

pub(super) fn composition_position(
    layers: &[capture::CaptureLayer],
    anchor: oblivion_one::compositor::EffectAnchor,
) -> usize {
    match anchor {
        oblivion_one::compositor::EffectAnchor::BeforeSurface(surface_id)
        | oblivion_one::compositor::EffectAnchor::ReplaceSurface(surface_id) => layers
            .iter()
            .position(|layer| *layer == capture::CaptureLayer::Surface(surface_id))
            .unwrap_or(layers.len()),
        oblivion_one::compositor::EffectAnchor::AfterSurface(surface_id) => layers
            .iter()
            .rposition(|layer| *layer == capture::CaptureLayer::Surface(surface_id))
            .map_or(layers.len(), |index| index.saturating_add(1)),
        oblivion_one::compositor::EffectAnchor::OutputPostProcess => layers.len(),
    }
}

pub(in crate::egl_renderer) fn composition_range(
    commands: &[crate::egl_renderer::geometry::EglDrawCommand],
    anchor: oblivion_one::compositor::EffectAnchor,
    visual_group: Option<oblivion_one::compositor::VisualGroupId>,
    anchor_scope: oblivion_one::compositor::EffectAnchorScope,
) -> (usize, usize) {
    if anchor_scope == oblivion_one::compositor::EffectAnchorScope::VisualGroup {
        let Some(visual_group) = visual_group else {
            return (commands.len(), commands.len());
        };
        let Some(start) = commands
            .iter()
            .position(|command| command.visual_group == Some(visual_group))
        else {
            return (commands.len(), commands.len());
        };
        let end = commands
            .iter()
            .rposition(|command| command.visual_group == Some(visual_group))
            .map_or(start, |index| index.saturating_add(1));
        return match anchor {
            oblivion_one::compositor::EffectAnchor::BeforeSurface(_) => (start, start),
            oblivion_one::compositor::EffectAnchor::ReplaceSurface(_) => (start, end),
            oblivion_one::compositor::EffectAnchor::AfterSurface(_) => (end, end),
            oblivion_one::compositor::EffectAnchor::OutputPostProcess => {
                (commands.len(), commands.len())
            }
        };
    }
    let position = composition_position(
        &commands
            .iter()
            .map(|command| match command.layer {
                EglDrawLayer::Surface(id) => capture::CaptureLayer::Surface(id),
                _ => capture::CaptureLayer::Other,
            })
            .collect::<Vec<_>>(),
        anchor,
    );
    match anchor {
        oblivion_one::compositor::EffectAnchor::BeforeSurface(_) => (position, position),
        oblivion_one::compositor::EffectAnchor::ReplaceSurface(_) => {
            (position, position.saturating_add(1))
        }
        oblivion_one::compositor::EffectAnchor::AfterSurface(_) => (position, position),
        oblivion_one::compositor::EffectAnchor::OutputPostProcess => {
            (commands.len(), commands.len())
        }
    }
}

pub(super) fn finalize_pass_timing_and_replay_detail(
    finish_timing: impl FnOnce(),
    replay_execution: Option<ReplayCaptureExecutionDetail>,
    mut record_detail: impl FnMut(ReplayCaptureExecutionDetail),
) {
    finish_timing();
    if let Some(detail) = replay_execution {
        record_detail(detail);
    }
}

pub(super) fn finalize_composite_scene_replay_timing(
    timing_allocated: bool,
    finish_timing: impl FnOnce() -> bool,
    build_detail: impl FnOnce() -> CompositeSceneReplayExecutionDetail,
) -> Option<CompositeSceneReplayExecutionDetail> {
    if timing_allocated && finish_timing() {
        Some(build_detail())
    } else {
        None
    }
}
