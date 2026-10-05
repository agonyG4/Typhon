use super::*;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct EffectExecutionSelection {
    pub(crate) executed_passes: Vec<GraphPassId>,
    pub(crate) executed_instances: Vec<oblivion_one::effects::EffectInstanceId>,
    pub(crate) acquired_texture_ids: Vec<GraphTextureId>,
}

pub(crate) fn select_effect_execution(
    graph: &CompiledFrameGraph,
    demand: &EffectExecutionDemand,
) -> EffectExecutionSelection {
    let mut selection = EffectExecutionSelection::default();
    for pass in &graph.passes {
        if !demand.is_conservative_full() && !demand.contains(pass.instance) {
            continue;
        }
        if demand.has_pass_plan()
            && demand
                .pass_output_region(pass.id)
                .is_none_or(EffectRegion::is_empty)
        {
            continue;
        }
        selection.executed_passes.push(pass.id);
        if !selection.executed_instances.contains(&pass.instance) {
            selection.executed_instances.push(pass.instance);
        }
        for texture_id in pass.inputs.iter().copied().chain(pass.output) {
            let is_output = graph
                .textures
                .iter()
                .find(|texture| texture.id == texture_id)
                .is_some_and(|texture| texture.source == GraphTextureSource::Output);
            if !is_output && !selection.acquired_texture_ids.contains(&texture_id) {
                selection.acquired_texture_ids.push(texture_id);
            }
        }
    }
    selection
}

pub(crate) fn plan_effect_surface_consumers(
    graph: &CompiledFrameGraph,
    demand: &EffectExecutionDemand,
    selection: &EffectExecutionSelection,
    commands: &[EglDrawCommand],
    repaint_rects: &[OutputRect],
    output_size: (u32, u32),
) -> SurfaceConsumerPlan {
    plan_effect_surface_consumers_with_debug_config(
        graph,
        demand,
        selection,
        commands,
        repaint_rects,
        output_size,
        *effect_debug_config(),
    )
}

pub(crate) fn plan_effect_surface_consumers_with_debug_config(
    graph: &CompiledFrameGraph,
    demand: &EffectExecutionDemand,
    selection: &EffectExecutionSelection,
    commands: &[EglDrawCommand],
    repaint_rects: &[OutputRect],
    output_size: (u32, u32),
    debug_config: EffectDebugConfig,
) -> SurfaceConsumerPlan {
    let mut plan = SurfaceConsumerPlan::default();
    let scene_work = scene_replay_work_plan(
        repaint_rects,
        graph,
        selection,
        output_size,
        SceneBaselineAuthority::ReplayRequired,
        debug_config,
    );
    let mut scene_work_state = SceneReplayWorkState::new(
        &scene_work,
        scene_replay_work_mode(
            SceneBaselineAuthority::ReplayRequired,
            debug_config.capture_mode() == EffectDebugCaptureMode::Framebuffer,
        ),
    );
    let mut scene_cursor = 0;
    let layers = commands
        .iter()
        .map(|command| match command.layer {
            EglDrawLayer::Surface(id) => capture::CaptureLayer::Surface(id),
            _ => capture::CaptureLayer::Other,
        })
        .collect::<Vec<_>>();
    let visual_groups = commands
        .iter()
        .map(|command| command.visual_group)
        .collect::<Vec<_>>();

    for pass in &graph.passes {
        if !selection.executed_passes.contains(&pass.id) {
            continue;
        }
        if scene_advance_reason(
            pass,
            SceneBaselineAuthority::ReplayRequired,
            debug_config.capture_mode() == EffectDebugCaptureMode::Framebuffer,
        )
        .is_some()
        {
            let (draw_end, _) =
                composition_range(commands, pass.anchor, pass.visual_group, pass.anchor_scope);
            add_surface_consumers_for_command_range(
                &mut plan,
                commands,
                scene_cursor,
                draw_end,
                scene_work_state.active_work(),
            );
            scene_cursor = scene_cursor.max(draw_end.min(commands.len()));
        }
        if matches!(
            pass.kind,
            RenderPassKind::Composite | RenderPassKind::OutputPostProcess
        ) {
            let (draw_end, next_cursor) =
                composition_range(commands, pass.anchor, pass.visual_group, pass.anchor_scope);
            add_surface_consumers_for_command_range(
                &mut plan,
                commands,
                scene_cursor,
                draw_end,
                scene_work_state.active_work(),
            );
            scene_cursor = scene_cursor.max(next_cursor.min(commands.len()));
        }
        if matches!(
            pass.kind,
            RenderPassKind::SceneCapture | RenderPassKind::SurfaceCapture
        ) {
            let indices = capture::indices_for_capture(
                &layers,
                &visual_groups,
                pass.anchor,
                pass.kind == RenderPassKind::SurfaceCapture,
                pass.visual_group,
                pass.anchor_scope,
            );
            let execution_damage = prepare_effect_execution_region(
                graph,
                pass,
                capture_execution_damage(
                    graph,
                    demand,
                    pass,
                    SceneBaselineAuthority::ReplayRequired,
                    debug_config,
                ),
            );
            let target_domain = pass
                .output
                .and_then(|output| graph.textures.iter().find(|texture| texture.id == output))
                .map(|texture| texture.domain);
            let capture_rects =
                capture_materialization_plan(&execution_damage.region, target_domain, output_size)
                    .output_rects;
            add_surface_consumers_for_capture_indices(
                &mut plan,
                commands,
                &indices,
                &capture_rects,
            );
            scene_work_state.mark_capture_satisfied(pass.id);
        }
    }
    let trailing_scene_work = finalize_surface_consumer_trailing_work(&mut scene_work_state);
    add_surface_consumers_for_command_range(
        &mut plan,
        commands,
        scene_cursor,
        commands.len(),
        &trailing_scene_work,
    );
    plan.finish();
    plan
}
