use oblivion_one::compositor::{
    EffectAnchor, EffectAnchorScope, EffectBackdropReuse, EffectSceneOrder, ResolvedEffectInstance,
    ResolvedEffectScene,
};
use oblivion_one::effects::{
    BlendMode, BlendSpec, CustomFragmentSpec, DualKawaseBlurSpec, EffectAlphaMode,
    EffectDemandPlannerPolicy, EffectFailurePolicy, EffectFootprint, EffectFrameDemand,
    EffectInstanceId, EffectNode, EffectNodeId, EffectOutsets, EffectProgram, EffectProgramId,
    EffectRect, EffectRegion, EffectRegistry, EffectSampleSupportError, EffectSource,
    EffectWorkingSpace, FrameExecutionCompileOptions, FrameExecutionPlan, GraphTextureId,
    GraphTextureSource, RenderPassKind, ShaderModuleId, analyze_effect_sample_support,
    builtin_background_material_program, compile_frame_execution_variants_with_options,
    derive_effect_sample_support, dual_kawase_sampling_radius,
    plan_effect_execution_demand_with_policy, required_input_region, validate_effect_program,
};
use oblivion_one::material::{EffectiveMaterial, MaterialConfiguration};
use std::collections::HashSet;

fn material_graph(
    bounds: EffectRect,
    visible: EffectRect,
    terminal_kawase_fusion: bool,
) -> (
    oblivion_one::effects::CompiledFrameGraph,
    EffectInstanceId,
    u32,
) {
    material_graph_for(
        MaterialConfiguration::default().effective().unwrap(),
        bounds,
        visible,
        terminal_kawase_fusion,
    )
}

fn material_graph_for(
    material: EffectiveMaterial,
    bounds: EffectRect,
    visible: EffectRect,
    terminal_kawase_fusion: bool,
) -> (
    oblivion_one::effects::CompiledFrameGraph,
    EffectInstanceId,
    u32,
) {
    let validated = builtin_background_material_program(material);
    let aggregate_radius = validated.aggregate_footprint.sample_radius_x;
    let program_id = validated.program.id;
    let mut registry = EffectRegistry::empty();
    registry.insert(validated).unwrap();
    let anchor = EffectAnchor::BeforeSurface(1);
    let id = EffectInstanceId::new(1).unwrap();
    let instance = ResolvedEffectInstance {
        id,
        program: program_id,
        anchor,
        region: EffectRegion::from_rect(visible),
        target_bounds: visible,
        parameter_block: Default::default(),
        coverage: None,
        signature: 1,
        frame_demand: EffectFrameDemand::OnDamage,
        visual_group: None,
        anchor_scope: EffectAnchorScope::VisualGroup,
        backdrop_reuse: EffectBackdropReuse::Exact,
        scene_order: EffectSceneOrder::for_anchor(anchor),
    };
    let scene = ResolvedEffectScene::new(1, vec![instance]);
    let variants = compile_frame_execution_variants_with_options(
        &scene,
        &EffectRegion::from_rect(bounds),
        bounds,
        &registry,
        FrameExecutionCompileOptions {
            precomputed_window_blur: false,
            terminal_kawase_fusion,
            checkpoint_dependency_pruning: false,
        },
    )
    .unwrap();
    let FrameExecutionPlan::EffectGraph(graph) = variants.exact else {
        panic!("canonical material must compile to an effect graph");
    };
    (graph, id, aggregate_radius)
}

fn multi_material_graph(
    bounds: EffectRect,
    visible_regions: &[EffectRect],
) -> oblivion_one::effects::CompiledFrameGraph {
    let validated =
        builtin_background_material_program(MaterialConfiguration::default().effective().unwrap());
    let program_id = validated.program.id;
    let mut registry = EffectRegistry::empty();
    registry.insert(validated).unwrap();
    let instances = visible_regions
        .iter()
        .copied()
        .enumerate()
        .map(|(index, visible)| {
            let surface = u32::try_from(index + 1).unwrap();
            let anchor = EffectAnchor::BeforeSurface(surface);
            ResolvedEffectInstance {
                id: EffectInstanceId::new(u64::from(surface)).unwrap(),
                program: program_id,
                anchor,
                region: EffectRegion::from_rect(visible),
                target_bounds: visible,
                parameter_block: Default::default(),
                coverage: None,
                signature: u64::from(surface),
                frame_demand: EffectFrameDemand::OnDamage,
                visual_group: None,
                anchor_scope: EffectAnchorScope::VisualGroup,
                backdrop_reuse: EffectBackdropReuse::Exact,
                scene_order: EffectSceneOrder::for_anchor(anchor),
            }
        })
        .collect();
    let scene = ResolvedEffectScene::new(1, instances);
    let variants = compile_frame_execution_variants_with_options(
        &scene,
        &EffectRegion::from_rect(bounds),
        bounds,
        &registry,
        FrameExecutionCompileOptions {
            precomputed_window_blur: false,
            terminal_kawase_fusion: true,
            checkpoint_dependency_pruning: false,
        },
    )
    .unwrap();
    let FrameExecutionPlan::EffectGraph(graph) = variants.exact else {
        panic!("canonical materials must compile to an effect graph");
    };
    graph
}

fn program_graph(
    program: EffectProgram,
    bounds: EffectRect,
    visible: EffectRect,
    target_bounds: EffectRect,
    anchor: EffectAnchor,
) -> (oblivion_one::effects::CompiledFrameGraph, EffectInstanceId) {
    let validated = validate_effect_program(program).unwrap();
    let program_id = validated.program.id;
    let mut registry = EffectRegistry::empty();
    registry.insert(validated).unwrap();
    let id = EffectInstanceId::new(1).unwrap();
    let instance = ResolvedEffectInstance {
        id,
        program: program_id,
        anchor,
        region: EffectRegion::from_rect(visible),
        target_bounds,
        parameter_block: Default::default(),
        coverage: None,
        signature: 1,
        frame_demand: EffectFrameDemand::OnDamage,
        visual_group: None,
        anchor_scope: EffectAnchorScope::VisualGroup,
        backdrop_reuse: EffectBackdropReuse::Exact,
        scene_order: EffectSceneOrder::for_anchor(anchor),
    };
    let scene = ResolvedEffectScene::new(1, vec![instance]);
    let variants = compile_frame_execution_variants_with_options(
        &scene,
        &EffectRegion::from_rect(bounds),
        bounds,
        &registry,
        FrameExecutionCompileOptions::default(),
    )
    .unwrap();
    let FrameExecutionPlan::EffectGraph(graph) = variants.exact else {
        panic!("effect program must compile to an effect graph");
    };
    (graph, id)
}

fn blur_graph(
    bounds: EffectRect,
    visible: EffectRect,
    radius: f32,
    passes: u8,
    scale: f32,
) -> (
    oblivion_one::effects::CompiledFrameGraph,
    EffectInstanceId,
    u32,
) {
    let source = EffectNodeId::new(1).unwrap();
    let blur = EffectNodeId::new(2).unwrap();
    let validated = validate_effect_program(EffectProgram {
        id: EffectProgramId::new(1).unwrap(),
        nodes: vec![
            EffectNode::source(source, EffectSource::Backdrop),
            EffectNode::dual_kawase(
                blur,
                source,
                DualKawaseBlurSpec::new(radius, passes, scale).unwrap(),
            ),
        ],
        output: blur,
        working_space: EffectWorkingSpace::LinearSrgb,
        alpha_mode: EffectAlphaMode::Opaque,
        outsets: Default::default(),
        frame_demand: EffectFrameDemand::OnDamage,
        failure_policy: EffectFailurePolicy::Passthrough,
    })
    .unwrap();
    let aggregate_radius = validated.aggregate_footprint.sample_radius_x;
    let program_id = validated.program.id;
    let mut registry = EffectRegistry::empty();
    registry.insert(validated).unwrap();
    let anchor = EffectAnchor::BeforeSurface(1);
    let id = EffectInstanceId::new(1).unwrap();
    let instance = ResolvedEffectInstance {
        id,
        program: program_id,
        anchor,
        region: EffectRegion::from_rect(visible),
        target_bounds: visible,
        parameter_block: Default::default(),
        coverage: None,
        signature: 1,
        frame_demand: EffectFrameDemand::OnDamage,
        visual_group: None,
        anchor_scope: EffectAnchorScope::VisualGroup,
        backdrop_reuse: EffectBackdropReuse::Exact,
        scene_order: EffectSceneOrder::for_anchor(anchor),
    };
    let scene = ResolvedEffectScene::new(1, vec![instance]);
    let variants = compile_frame_execution_variants_with_options(
        &scene,
        &EffectRegion::from_rect(bounds),
        bounds,
        &registry,
        FrameExecutionCompileOptions::default(),
    )
    .unwrap();
    let FrameExecutionPlan::EffectGraph(graph) = variants.exact else {
        panic!("Dual Kawase must compile to an effect graph");
    };
    (graph, id, aggregate_radius)
}

fn region_pixels(region: &EffectRegion) -> HashSet<(i32, i32)> {
    region
        .rects()
        .iter()
        .flat_map(|rect| {
            (rect.y..rect.bottom()).flat_map(move |y| (rect.x..rect.right()).map(move |x| (x, y)))
        })
        .collect()
}

fn add_region_pixels(into: &mut HashSet<(i32, i32)>, region: &EffectRegion) {
    into.extend(region_pixels(region));
}

fn trace_pixel_sources(
    graph: &oblivion_one::effects::CompiledFrameGraph,
    pass_index: usize,
    output_region: &EffectRegion,
    backdrop: &mut HashSet<(i32, i32)>,
    target: &mut HashSet<(i32, i32)>,
) {
    let pass = &graph.passes[pass_index];
    match pass.kind {
        RenderPassKind::SceneCapture => {
            add_region_pixels(backdrop, output_region);
            return;
        }
        RenderPassKind::SurfaceCapture => {
            add_region_pixels(target, output_region);
            return;
        }
        _ => {}
    }
    let output_id = pass.output.expect("compiled pass output");
    let output = graph
        .textures
        .iter()
        .find(|texture| texture.id == output_id)
        .unwrap();
    for input_id in &pass.inputs {
        let input = graph
            .textures
            .iter()
            .find(|texture| texture.id == *input_id)
            .unwrap();
        let required = required_input_region(pass, output_region, output, input)
            .expect("compiled pass has sampling metadata");
        let producer_index = graph
            .passes
            .iter()
            .position(|producer| producer.output == Some(input.id))
            .expect("ordinary input has a producer");
        trace_pixel_sources(graph, producer_index, &required, backdrop, target);
    }
}

fn pixel_oracle(
    graph: &oblivion_one::effects::CompiledFrameGraph,
    instance_id: EffectInstanceId,
    presentation_region: &EffectRegion,
) -> (HashSet<(i32, i32)>, HashSet<(i32, i32)>) {
    let final_pass_index = graph
        .passes
        .iter()
        .position(|pass| {
            pass.instance == instance_id
                && matches!(
                    pass.kind,
                    RenderPassKind::Composite | RenderPassKind::OutputPostProcess
                )
        })
        .unwrap();
    let mut backdrop = HashSet::new();
    let mut target = HashSet::new();
    for (x, y) in region_pixels(presentation_region) {
        trace_pixel_sources(
            graph,
            final_pass_index,
            &EffectRegion::from_rect(EffectRect::new(x, y, 1, 1).unwrap()),
            &mut backdrop,
            &mut target,
        );
    }
    (backdrop, target)
}

fn assert_planner_parity(
    graph: &oblivion_one::effects::CompiledFrameGraph,
    instance_id: EffectInstanceId,
    bounds: EffectRect,
    support: &oblivion_one::effects::EffectSampleSupportEnvelope,
) {
    for policy in [
        EffectDemandPlannerPolicy::Legacy,
        EffectDemandPlannerPolicy::Regional,
    ] {
        let demand = plan_effect_execution_demand_with_policy(
            graph,
            &EffectRegion::from_rect(bounds),
            false,
            false,
            policy,
        );
        for pass in graph.passes.iter().filter(|pass| {
            pass.instance == instance_id
                && matches!(
                    pass.kind,
                    RenderPassKind::SceneCapture | RenderPassKind::SurfaceCapture
                )
        }) {
            let planned = demand
                .passes
                .iter()
                .find(|planned| planned.id == pass.id)
                .unwrap();
            let expected = match pass.kind {
                RenderPassKind::SceneCapture => &support.backdrop_region,
                RenderPassKind::SurfaceCapture => &support.target_region,
                _ => unreachable!(),
            };
            assert_eq!(
                &planned.output_region, expected,
                "{policy:?} {:?}",
                pass.kind
            );
        }
    }
}

#[test]
fn default_material_support_is_tighter_than_aggregate_capture() {
    let bounds = EffectRect::new(0, 0, 1920, 1080).unwrap();
    let visible = EffectRect::new(700, 400, 240, 140).unwrap();
    let (graph, instance_id, aggregate_radius) = material_graph(bounds, visible, true);
    let instance = graph
        .instances
        .iter()
        .find(|instance| instance.id == instance_id)
        .unwrap();

    assert_eq!(aggregate_radius, 111);
    assert_eq!(dual_kawase_sampling_radius(5.36, 2, 1.0).unwrap(), 101);
    let support = derive_effect_sample_support(&graph, instance_id).unwrap();
    assert_eq!(
        support.presentation_region,
        instance.output_influence_region
    );
    assert!(
        support
            .backdrop_region
            .subtract(&instance.capture_region)
            .is_empty()
    );
    assert_ne!(support.backdrop_region, instance.capture_region);
    let presentation = support.presentation_region.bounding_rect().unwrap();
    let aggregate = instance.capture_region.bounding_rect().unwrap();
    let sampled = support.backdrop_region.bounding_rect().unwrap();
    let halo = (
        u32::try_from(i64::from(presentation.x) - i64::from(sampled.x)).unwrap(),
        u32::try_from(i64::from(sampled.right()) - i64::from(presentation.right())).unwrap(),
        u32::try_from(i64::from(presentation.y) - i64::from(sampled.y)).unwrap(),
        u32::try_from(i64::from(sampled.bottom()) - i64::from(presentation.bottom())).unwrap(),
    );
    let aggregate_pixels = region_pixels(&instance.capture_region).len();
    let sampled_pixels = region_pixels(&support.backdrop_region).len();
    assert!(sampled_pixels < aggregate_pixels);
    eprintln!(
        "default Material aggregate={aggregate:?} ({aggregate_pixels} px), sampled={sampled:?} ({sampled_pixels} px), halo L/R/T/B={}/{}/{}/{}, coalesces={}",
        halo.0, halo.1, halo.2, halo.3, support.known_representation_coalesces,
    );
}

#[test]
fn fixture_and_maximum_material_keep_aggregate_bounds_and_contain_sampled_support() {
    let bounds = EffectRect::new(0, 0, 1920, 1080).unwrap();
    let visible = EffectRect::new(720, 390, 180, 120).unwrap();
    for (radius, expected_aggregate) in [(5.36, 101), (10.0, 142)] {
        let (graph, id, aggregate) = blur_graph(bounds, visible, radius, 2, 1.0);
        assert_eq!(aggregate, expected_aggregate);
        let instance = graph
            .instances
            .iter()
            .find(|instance| instance.id == id)
            .unwrap();
        let support = derive_effect_sample_support(&graph, id).unwrap();
        assert!(
            support
                .backdrop_region
                .subtract(&instance.capture_region)
                .is_empty()
        );
        assert_ne!(support.backdrop_region, instance.capture_region);
        eprintln!(
            "Dual Kawase radius={radius} aggregate={} capture={:?}, sampled={:?} ({} px)",
            aggregate,
            instance.capture_region.bounding_rect(),
            support.backdrop_region.bounding_rect(),
            region_pixels(&support.backdrop_region).len(),
        );
    }

    let maximum = EffectiveMaterial {
        blur: 1.0,
        saturation: 1.0,
        noise: 1.0,
    };
    let (graph, id, aggregate) = material_graph_for(maximum, bounds, visible, true);
    assert_eq!(aggregate, 142);
    let instance = graph
        .instances
        .iter()
        .find(|instance| instance.id == id)
        .unwrap();
    let support = derive_effect_sample_support(&graph, id).unwrap();
    assert!(
        support
            .backdrop_region
            .subtract(&instance.capture_region)
            .is_empty()
    );
    assert_ne!(support.backdrop_region, instance.capture_region);
    eprintln!(
        "maximum Material aggregate={aggregate} capture={:?}, sampled={:?} ({} px)",
        instance.capture_region.bounding_rect(),
        support.backdrop_region.bounding_rect(),
        region_pixels(&support.backdrop_region).len(),
    );
}

#[test]
fn terminal_fusion_preserves_sampled_support_for_even_odd_translated_and_edge_domains() {
    let fixtures = [
        (
            EffectRect::new(0, 0, 320, 180).unwrap(),
            EffectRect::new(72, 44, 90, 62).unwrap(),
        ),
        (
            EffectRect::new(19, 13, 101, 74).unwrap(),
            EffectRect::new(43, 31, 40, 30).unwrap(),
        ),
        (
            EffectRect::new(-17, 29, 101, 75).unwrap(),
            EffectRect::new(-17, 29, 35, 24).unwrap(),
        ),
        (
            EffectRect::new(0, 0, 321, 181).unwrap(),
            EffectRect::new(251, 131, 70, 50).unwrap(),
        ),
    ];
    for (bounds, visible) in fixtures {
        let (staged_graph, id, _) = material_graph(bounds, visible, false);
        let (fused_graph, _, _) = material_graph(bounds, visible, true);
        let staged = derive_effect_sample_support(&staged_graph, id).unwrap();
        let fused = derive_effect_sample_support(&fused_graph, id).unwrap();
        assert_eq!(staged.presentation_region, fused.presentation_region);
        assert_eq!(staged.backdrop_region, fused.backdrop_region);
        assert_eq!(staged.target_region, fused.target_region);
    }
}

#[test]
fn sampled_support_is_contained_for_odd_tiny_translated_and_scaled_domains() {
    let dimensions = [
        (1, 1),
        (2, 3),
        (7, 5),
        (9, 7),
        (101, 74),
        (101, 75),
        (321, 181),
    ];
    let scales = [1.0, 0.75, 0.5, 0.25];
    let radii = [0.25, 5.36, 10.0];
    for (width, height) in dimensions {
        let bounds = EffectRect::new(-19, 31, width, height).unwrap();
        let visible = if width <= 9 || height <= 7 {
            bounds
        } else {
            let visible_width = width.min(40);
            let visible_height = height.min(30);
            EffectRect::new(
                bounds.x + i32::try_from((width - visible_width) / 2).unwrap(),
                bounds.y + i32::try_from((height - visible_height) / 2).unwrap(),
                visible_width,
                visible_height,
            )
            .unwrap()
        };
        for passes in 1..=4 {
            for scale in scales {
                for radius in radii {
                    let (graph, id, _) = blur_graph(bounds, visible, radius, passes, scale);
                    let instance = graph
                        .instances
                        .iter()
                        .find(|instance| instance.id == id)
                        .unwrap();
                    let support = derive_effect_sample_support(&graph, id).unwrap();
                    assert!(
                        support
                            .backdrop_region
                            .subtract(&instance.capture_region)
                            .is_empty(),
                        "support escaped capture for {width}x{height}, passes={passes}, scale={scale}, radius={radius}"
                    );
                }
            }
        }
    }
}

#[test]
fn reverse_support_matches_an_independent_per_output_pixel_oracle() {
    let bounds = EffectRect::new(0, 0, 96, 72).unwrap();
    let visible = EffectRect::new(31, 24, 18, 12).unwrap();
    let (graph, id, _) = blur_graph(bounds, visible, 1.0, 2, 0.75);
    let support = derive_effect_sample_support(&graph, id).unwrap();
    let (backdrop, target) = pixel_oracle(&graph, id, &support.presentation_region);
    assert_eq!(region_pixels(&support.backdrop_region), backdrop);
    assert!(target.is_empty());
}

#[test]
fn blend_propagates_each_source_and_custom_fragment_uses_declared_footprint() {
    let bounds = EffectRect::new(0, 0, 400, 240).unwrap();
    let visible = EffectRect::new(150, 90, 46, 34).unwrap();
    let backdrop = EffectNodeId::new(1).unwrap();
    let target = EffectNodeId::new(2).unwrap();
    let blur = EffectNodeId::new(3).unwrap();
    let blend = EffectNodeId::new(4).unwrap();
    let blend_program = EffectProgram {
        id: EffectProgramId::new(3).unwrap(),
        nodes: vec![
            EffectNode::source(backdrop, EffectSource::Backdrop),
            EffectNode::dual_kawase(
                blur,
                backdrop,
                DualKawaseBlurSpec::new(1.0, 1, 1.0).unwrap(),
            ),
            EffectNode::source(target, EffectSource::TargetContent),
            EffectNode::blend(
                blend,
                vec![blur, target],
                BlendSpec::new(BlendMode::SourceOver, 0.75).unwrap(),
            ),
        ],
        output: blend,
        working_space: EffectWorkingSpace::LinearSrgb,
        alpha_mode: EffectAlphaMode::Preserve,
        outsets: EffectOutsets::ZERO,
        frame_demand: EffectFrameDemand::OnDamage,
        failure_policy: EffectFailurePolicy::Passthrough,
    };
    let (graph, id) = program_graph(
        blend_program,
        bounds,
        visible,
        visible,
        EffectAnchor::BeforeSurface(1),
    );
    assert!(
        graph
            .passes
            .iter()
            .any(|pass| pass.kind == RenderPassKind::NormalizeInput)
    );
    let support = derive_effect_sample_support(&graph, id).unwrap();
    assert!(!support.backdrop_region.is_empty());
    assert!(!support.target_region.is_empty());
    let (backdrop_pixels, target_pixels) = pixel_oracle(&graph, id, &support.presentation_region);
    assert_eq!(region_pixels(&support.backdrop_region), backdrop_pixels);
    assert_eq!(region_pixels(&support.target_region), target_pixels);
    assert_planner_parity(&graph, id, bounds, &support);

    let backdrop = EffectNodeId::new(1).unwrap();
    let custom = EffectNodeId::new(2).unwrap();
    let custom_program = EffectProgram {
        id: EffectProgramId::new(4).unwrap(),
        nodes: vec![
            EffectNode::source(backdrop, EffectSource::Backdrop),
            EffectNode::custom_fragment(
                custom,
                backdrop,
                CustomFragmentSpec {
                    shader: ShaderModuleId::new(4).unwrap(),
                    declared_footprint: EffectFootprint {
                        sample_radius_x: 4,
                        sample_radius_y: 7,
                        output_outsets: EffectOutsets::ZERO,
                    },
                    uniforms: Vec::new(),
                    auxiliary_inputs: Vec::new(),
                },
            )
            .unwrap(),
        ],
        output: custom,
        working_space: EffectWorkingSpace::LinearSrgb,
        alpha_mode: EffectAlphaMode::Opaque,
        outsets: EffectOutsets::ZERO,
        frame_demand: EffectFrameDemand::OnDamage,
        failure_policy: EffectFailurePolicy::Passthrough,
    };
    let (graph, id) = program_graph(
        custom_program,
        bounds,
        visible,
        visible,
        EffectAnchor::BeforeSurface(1),
    );
    let support = derive_effect_sample_support(&graph, id).unwrap();
    assert_ne!(support.backdrop_region, support.presentation_region);
    let (backdrop_pixels, target_pixels) = pixel_oracle(&graph, id, &support.presentation_region);
    assert_eq!(region_pixels(&support.backdrop_region), backdrop_pixels);
    assert!(target_pixels.is_empty());
}

#[test]
fn presentation_seed_includes_program_output_outsets() {
    let bounds = EffectRect::new(0, 0, 240, 160).unwrap();
    let visible = EffectRect::new(80, 60, 40, 30).unwrap();
    let source = EffectNodeId::new(1).unwrap();
    let custom = EffectNodeId::new(2).unwrap();
    let program = EffectProgram {
        id: EffectProgramId::new(5).unwrap(),
        nodes: vec![
            EffectNode::source(source, EffectSource::Backdrop),
            EffectNode::custom_fragment(
                custom,
                source,
                CustomFragmentSpec {
                    shader: ShaderModuleId::new(5).unwrap(),
                    declared_footprint: EffectFootprint::ZERO,
                    uniforms: Vec::new(),
                    auxiliary_inputs: Vec::new(),
                },
            )
            .unwrap(),
        ],
        output: custom,
        working_space: EffectWorkingSpace::LinearSrgb,
        alpha_mode: EffectAlphaMode::Opaque,
        outsets: EffectOutsets {
            top: 5,
            right: 9,
            bottom: 3,
            left: 7,
        },
        frame_demand: EffectFrameDemand::OnDamage,
        failure_policy: EffectFailurePolicy::Passthrough,
    };
    let (graph, id) = program_graph(
        program,
        bounds,
        visible,
        visible,
        EffectAnchor::BeforeSurface(1),
    );
    let instance = graph
        .instances
        .iter()
        .find(|instance| instance.id == id)
        .unwrap();
    let support = derive_effect_sample_support(&graph, id).unwrap();
    assert_eq!(
        support.presentation_region,
        instance.output_influence_region
    );
    assert_ne!(
        support.presentation_region,
        EffectRegion::from_rect(visible)
    );
    let (backdrop_pixels, target_pixels) = pixel_oracle(&graph, id, &support.presentation_region);
    assert_eq!(region_pixels(&support.backdrop_region), backdrop_pixels);
    assert!(target_pixels.is_empty());
}

#[test]
fn both_demand_planners_match_complete_sampled_capture_support() {
    let bounds = EffectRect::new(0, 0, 640, 360).unwrap();
    let visible = EffectRect::new(210, 130, 120, 80).unwrap();
    let (graph, id, _) = material_graph(bounds, visible, true);
    let support = derive_effect_sample_support(&graph, id).unwrap();
    assert_planner_parity(&graph, id, bounds, &support);
}

#[test]
fn aggregate_checkpoint_edges_report_sample_support_pruning_candidates() {
    let bounds = EffectRect::new(0, 0, 800, 500).unwrap();
    let first_region = EffectRect::new(100, 110, 100, 70).unwrap();
    let second_region = EffectRect::new(270, 110, 100, 70).unwrap();
    let graph = multi_material_graph(bounds, &[first_region, second_region]);
    assert_eq!(graph.instances[1].dependencies, vec![graph.instances[0].id]);

    let analysis = analyze_effect_sample_support(&graph);
    assert_eq!(analysis.aggregate_checkpoint_dependency_count, 1);
    assert_eq!(analysis.sample_support_checkpoint_dependency_count, 0);
    assert_eq!(analysis.prunable_checkpoint_candidates, 1);
    assert_eq!(analysis.unresolved_checkpoint_dependency_count, 0);
}

#[test]
fn malformed_graph_metadata_returns_typed_fail_closed_errors() {
    let bounds = EffectRect::new(0, 0, 320, 200).unwrap();
    let visible = EffectRect::new(110, 80, 60, 40).unwrap();
    let (graph, id, _) = blur_graph(bounds, visible, 1.0, 2, 1.0);

    assert_eq!(
        derive_effect_sample_support(&graph, EffectInstanceId::new(99).unwrap()),
        Err(EffectSampleSupportError::InstanceMissing(
            EffectInstanceId::new(99).unwrap()
        ))
    );

    let mut ambiguous_instance = graph.clone();
    ambiguous_instance
        .instances
        .push(ambiguous_instance.instances[0].clone());
    assert_eq!(
        derive_effect_sample_support(&ambiguous_instance, id),
        Err(EffectSampleSupportError::InstanceAmbiguous(id))
    );

    let mut missing_final = graph.clone();
    missing_final.passes.pop();
    assert_eq!(
        derive_effect_sample_support(&missing_final, id),
        Err(EffectSampleSupportError::FinalPassMissing(id))
    );

    let mut ambiguous_final = graph.clone();
    let final_pass = ambiguous_final.passes.last().unwrap().clone();
    ambiguous_final.passes.push(final_pass);
    assert_eq!(
        derive_effect_sample_support(&ambiguous_final, id),
        Err(EffectSampleSupportError::FinalPassAmbiguous(id))
    );

    let mut missing_texture = graph.clone();
    missing_texture.passes.last_mut().unwrap().inputs[0] = GraphTextureId::new(60_000).unwrap();
    assert!(matches!(
        derive_effect_sample_support(&missing_texture, id),
        Err(EffectSampleSupportError::TextureMissing(_))
    ));

    let mut missing_output = graph.clone();
    missing_output.passes.last_mut().unwrap().output = None;
    assert_eq!(
        derive_effect_sample_support(&missing_output, id),
        Err(EffectSampleSupportError::PassOutputMissing(
            graph.passes.last().unwrap().id
        ))
    );

    let mut missing_producer = graph.clone();
    let capture = missing_producer
        .passes
        .iter()
        .position(|pass| pass.kind == RenderPassKind::SceneCapture)
        .unwrap();
    missing_producer.passes.remove(capture);
    assert!(matches!(
        derive_effect_sample_support(&missing_producer, id),
        Err(EffectSampleSupportError::TextureProducerMissing(_))
    ));

    let mut ambiguous_producer = graph.clone();
    let capture = ambiguous_producer
        .passes
        .iter()
        .find(|pass| pass.kind == RenderPassKind::SceneCapture)
        .unwrap()
        .clone();
    ambiguous_producer.passes.push(capture);
    assert!(matches!(
        derive_effect_sample_support(&ambiguous_producer, id),
        Err(EffectSampleSupportError::TextureProducerAmbiguous(_))
    ));

    let mut invalid_order = graph.clone();
    invalid_order.passes.swap(0, 1);
    assert!(matches!(
        derive_effect_sample_support(&invalid_order, id),
        Err(EffectSampleSupportError::ProducerOrderingInvalid { .. })
    ));

    let mut duplicate_texture = graph.clone();
    duplicate_texture
        .textures
        .push(duplicate_texture.textures[0].clone());
    assert!(matches!(
        derive_effect_sample_support(&duplicate_texture, id),
        Err(EffectSampleSupportError::TextureLookupAmbiguous(_))
    ));

    let mut precomputed = graph.clone();
    let capture_texture = precomputed
        .textures
        .iter_mut()
        .find(|texture| texture.source == GraphTextureSource::CapturedScene)
        .unwrap();
    capture_texture.source = GraphTextureSource::PrecomputedBackdrop;
    assert!(matches!(
        derive_effect_sample_support(&precomputed, id),
        Err(EffectSampleSupportError::ExternalPrecomputedBackdrop { .. })
    ));

    let mut undercovered = graph.clone();
    undercovered.instances[0].capture_region = EffectRegion::empty();
    assert!(matches!(
        derive_effect_sample_support(&undercovered, id),
        Err(EffectSampleSupportError::SampledBackdropOutsideAggregateCapture { .. })
    ));

    let mut cross_instance = multi_material_graph(bounds, &[visible, visible]);
    let first_id = EffectInstanceId::new(1).unwrap();
    let second_id = EffectInstanceId::new(2).unwrap();
    let first_final_input = cross_instance
        .passes
        .iter()
        .find(|pass| {
            pass.instance == first_id
                && matches!(
                    pass.kind,
                    RenderPassKind::Composite | RenderPassKind::OutputPostProcess
                )
        })
        .unwrap()
        .inputs[0];
    let second_final = cross_instance
        .passes
        .iter_mut()
        .find(|pass| {
            pass.instance == second_id
                && matches!(
                    pass.kind,
                    RenderPassKind::Composite | RenderPassKind::OutputPostProcess
                )
        })
        .unwrap();
    second_final.inputs[0] = first_final_input;
    assert!(matches!(
        derive_effect_sample_support(&cross_instance, second_id),
        Err(EffectSampleSupportError::CrossInstanceProducer { .. })
    ));

    let (mut invalid_terminal, id, _) = material_graph(bounds, visible, true);
    let final_pass = invalid_terminal.passes.last_mut().unwrap();
    final_pass
        .terminal_kawase
        .as_mut()
        .unwrap()
        .virtual_full_resolution
        .width = 0;
    assert!(matches!(
        derive_effect_sample_support(&invalid_terminal, id),
        Err(EffectSampleSupportError::RequiredInputRegionUnavailable { .. })
    ));
}

#[test]
fn analysis_does_not_mutate_compiled_graph_metadata() {
    let bounds = EffectRect::new(0, 0, 640, 360).unwrap();
    let visible = EffectRect::new(200, 140, 120, 80).unwrap();
    let (graph, id, _) = material_graph(bounds, visible, true);
    let before = graph.clone();
    let _ = derive_effect_sample_support(&graph, id).unwrap();
    let _ = analyze_effect_sample_support(&graph);
    assert_eq!(graph, before);
}
