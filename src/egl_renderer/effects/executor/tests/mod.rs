use super::*;
use crate::egl_renderer::{EglRect, SurfaceSampling, replay_capture_region_layout};

fn test_texture(
    id: u16,
    source: GraphTextureSource,
    domain: oblivion_one::effects::EffectRect,
) -> oblivion_one::effects::GraphTexturePlan {
    oblivion_one::effects::GraphTexturePlan {
        id: GraphTextureId::new(id).unwrap(),
        source,
        width: domain.width,
        height: domain.height,
        domain,
        working_space: oblivion_one::effects::EffectWorkingSpace::LinearSrgb,
        origin: oblivion_one::effects::GraphTextureOrigin::BottomLeft,
        first_use: None,
        last_use: None,
    }
}

fn test_pass(
    id: u16,
    kind: RenderPassKind,
    instance: oblivion_one::effects::EffectInstanceId,
    inputs: Vec<GraphTextureId>,
    output: GraphTextureId,
    checkpoint_dependencies: Vec<GraphPassId>,
) -> CompiledRenderPass {
    CompiledRenderPass {
        id: GraphPassId::new(id).unwrap(),
        kind,
        inputs,
        output: Some(output),
        damage: EffectRegion::empty(),
        instance,
        anchor: oblivion_one::compositor::EffectAnchor::OutputPostProcess,
        blur_radius: None,
        stage: None,
        fused_stages: Vec::new(),
        parameter_block: oblivion_one::effects::EffectParameterBlock::default(),
        coverage: None,
        alpha_mode: oblivion_one::effects::EffectAlphaMode::Preserve,
        encode_output: false,
        color_conversion: EffectColorConversion::None,
        checkpoint_dependencies,
        visual_group: None,
        anchor_scope: oblivion_one::compositor::EffectAnchorScope::VisualGroup,
        visible_clip_fallback: None,
    }
}

fn compile_builtin_background_blur(
    region: EffectRegion,
    source_damage: &EffectRegion,
    output_bounds: oblivion_one::effects::EffectRect,
) -> CompiledFrameGraph {
    let program = oblivion_one::effects::builtin_background_blur_program_id();
    let anchor = oblivion_one::compositor::EffectAnchor::BeforeSurface(1);
    let scene = oblivion_one::compositor::ResolvedEffectScene::new(
        1,
        vec![oblivion_one::compositor::ResolvedEffectInstance {
            id: oblivion_one::effects::EffectInstanceId::new(1).unwrap(),
            program,
            anchor,
            target_bounds: region.bounding_rect().unwrap(),
            region,
            parameter_block: oblivion_one::effects::EffectParameterBlock::default(),
            coverage: None,
            signature: 1,
            frame_demand: oblivion_one::effects::EffectFrameDemand::OnDamage,
            visual_group: None,
            anchor_scope: oblivion_one::compositor::EffectAnchorScope::VisualGroup,
            scene_order: oblivion_one::compositor::EffectSceneOrder::for_anchor(anchor),
        }],
    );
    let registry = oblivion_one::effects::EffectRegistry::with_builtin_background_blur();
    let oblivion_one::effects::FrameExecutionPlan::EffectGraph(graph) =
        oblivion_one::effects::compile_frame_execution_plan(
            &scene,
            source_damage,
            output_bounds,
            &registry,
        )
        .unwrap()
    else {
        panic!("builtin background blur must compile to an effect graph");
    };
    graph
}

fn planned_demand(
    instance: oblivion_one::effects::EffectInstanceId,
    output_region: EffectRegion,
    passes: Vec<(GraphPassId, EffectRegion)>,
) -> EffectExecutionDemand {
    let mut demand = EffectExecutionDemand::new(
        vec![oblivion_one::effects::EffectInstanceExecutionDemand {
            id: instance,
            presentation_output_region: output_region.clone(),
            output_region: output_region.clone(),
        }],
        output_region,
    );
    demand.passes = passes
        .into_iter()
        .map(
            |(id, output_region)| oblivion_one::effects::EffectPassExecutionDemand {
                id,
                output_region,
            },
        )
        .collect();
    demand
}

fn fusion_test_graph() -> CompiledFrameGraph {
    let region =
        EffectRegion::from_rect(oblivion_one::effects::EffectRect::new(260, 180, 37, 29).unwrap());
    let mut graph = compile_builtin_background_blur(
        region.clone(),
        &region,
        oblivion_one::effects::EffectRect::new(0, 0, 1920, 1080).unwrap(),
    );
    let capture_id = graph
        .passes
        .iter()
        .find(|pass| pass.kind == RenderPassKind::SceneCapture)
        .unwrap()
        .id;
    graph
        .passes
        .iter_mut()
        .find(|pass| pass.id == capture_id)
        .unwrap()
        .checkpoint_dependencies
        .push(GraphPassId::new(u16::MAX).unwrap());
    graph
}

fn fusion_test_plan(
    graph: &CompiledFrameGraph,
    selected_passes: Vec<GraphPassId>,
    preparation: CheckpointCapturePreparation,
    active_output_texture_available: bool,
    debug_config: EffectDebugConfig,
) -> CaptureDownsampleFusionPlan {
    let capture = graph
        .passes
        .iter()
        .find(|pass| {
            matches!(
                pass.kind,
                RenderPassKind::SceneCapture | RenderPassKind::SurfaceCapture
            )
        })
        .unwrap();
    let capture_texture = capture.output.unwrap();
    let selection = EffectExecutionSelection {
        executed_passes: selected_passes,
        ..EffectExecutionSelection::default()
    };
    let preparations = HashMap::from([(capture_texture, preparation)]);
    plan_capture_downsample_fusions(
        graph,
        &selection,
        &preparations,
        active_output_texture_available,
        (1920, 1080),
        OutputFramebufferOrigin::BottomLeft,
        SceneBaselineAuthority::ReplayRequired,
        debug_config,
    )
}

fn all_graph_passes_selected(graph: &CompiledFrameGraph) -> Vec<GraphPassId> {
    graph.passes.iter().map(|pass| pass.id).collect()
}

fn assert_fusion_ineligible(graph: &CompiledFrameGraph) {
    let config = EffectDebugConfig::from_env_values_with_checkpoint_capture_path(None, None, None);
    let plan = fusion_test_plan(
        graph,
        all_graph_passes_selected(graph),
        CheckpointCapturePreparation {
            newly_admitted: true,
            identity_churn: true,
        },
        true,
        config,
    );
    assert!(
        plan.by_capture.is_empty(),
        "unexpected fusion plan: {plan:?}"
    );
}

fn test_checkpoint_requirement(pass: u16, rects: &[OutputRect]) -> SceneCheckpointRequirement {
    SceneCheckpointRequirement {
        capture_pass: GraphPassId::new(pass).expect("test checkpoint pass id"),
        region: rects.to_vec(),
    }
}

fn test_scene_replay_plan(
    presentation: &[OutputRect],
    requirements: &[SceneCheckpointRequirement],
) -> SceneReplayWorkPlan {
    SceneReplayWorkPlan::new(presentation.to_vec(), requirements.to_vec(), (100, 100))
}

fn assert_same_output_region(actual: &[OutputRect], expected: &[OutputRect]) {
    let actual = output_rects_to_effect_region(actual);
    let expected = output_rects_to_effect_region(expected);
    assert!(
        actual.subtract(&expected).is_empty(),
        "actual work contains pixels outside expected work: {actual:?} vs {expected:?}"
    );
    assert!(
        expected.subtract(&actual).is_empty(),
        "expected work contains pixels outside actual work: {expected:?} vs {actual:?}"
    );
}

fn output_work_pixels(rects: &[OutputRect]) -> u64 {
    rects.iter().fold(0, |pixels, rect| {
        pixels.saturating_add(u64::from(rect.width) * u64::from(rect.height))
    })
}

mod captures;
mod planning;
mod resource_lifetime;
mod telemetry;
mod validation;
