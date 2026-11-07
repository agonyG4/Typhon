use oblivion_one::compositor::{
    EffectAnchor, EffectAnchorScope, EffectBackdropReuse, EffectSceneOrder, ResolvedEffectInstance,
    ResolvedEffectScene, VisualGroupId,
};
use oblivion_one::effects::{
    EffectCheckpointDependencyDecisionKind, EffectFrameDemand, EffectInstanceId, EffectRect,
    EffectRegion, EffectRegistry, FrameExecutionCompileOptions, FrameExecutionPlan, RenderPassKind,
    apply_effect_checkpoint_dependency_pruning, builtin_background_material_program,
    compile_frame_execution_plan, compile_frame_execution_variants_with_options,
    derive_effect_sample_support, plan_effect_checkpoint_dependency_pruning,
};
use oblivion_one::material::MaterialConfiguration;

fn two_material_variants(
    second_x: i32,
    terminal_kawase_fusion: bool,
    checkpoint_dependency_pruning: bool,
) -> oblivion_one::effects::CompiledFrameExecutionVariants {
    let (scene, registry, bounds) = two_material_scene(second_x);
    compile_frame_execution_variants_with_options(
        &scene,
        &EffectRegion::from_rect(bounds),
        bounds,
        &registry,
        FrameExecutionCompileOptions {
            precomputed_window_blur: false,
            terminal_kawase_fusion,
            checkpoint_dependency_pruning,
        },
    )
    .unwrap()
}

fn two_material_scene(second_x: i32) -> (ResolvedEffectScene, EffectRegistry, EffectRect) {
    let bounds = EffectRect::new(0, 0, 800, 500).unwrap();
    let validated =
        builtin_background_material_program(MaterialConfiguration::default().effective().unwrap());
    let program_id = validated.program.id;
    let mut registry = EffectRegistry::empty();
    registry.insert(validated).unwrap();
    let instances = [100, second_x]
        .into_iter()
        .enumerate()
        .map(|(index, x)| {
            let surface = u32::try_from(index + 1).unwrap();
            let anchor = EffectAnchor::BeforeSurface(surface);
            let region = EffectRect::new(x, 110, 100, 70).unwrap();
            ResolvedEffectInstance {
                composite_opacity: None,
                id: EffectInstanceId::new(u64::from(surface)).unwrap(),
                program: program_id,
                anchor,
                region: EffectRegion::from_rect(region),
                target_bounds: region,
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
    (scene, registry, bounds)
}

fn two_material_graph(second_x: i32) -> oblivion_one::effects::CompiledFrameGraph {
    let variants = two_material_variants(second_x, false, false);
    let FrameExecutionPlan::EffectGraph(graph) = variants.exact else {
        panic!("two canonical materials must compile to an effect graph");
    };
    graph
}

fn precomputed_isolation_scene() -> (ResolvedEffectScene, EffectRegistry, EffectRect) {
    let bounds = EffectRect::new(0, 0, 800, 500).unwrap();
    let validated =
        builtin_background_material_program(MaterialConfiguration::default().effective().unwrap());
    let program_id = validated.program.id;
    let mut registry = EffectRegistry::empty();
    registry.insert(validated).unwrap();
    let instances = [(100, false), (270, true), (500, true)]
        .into_iter()
        .enumerate()
        .map(|(index, (x, precomputed_candidate))| {
            let surface = u32::try_from(index + 1).unwrap();
            let anchor = EffectAnchor::BeforeSurface(surface);
            let region = EffectRect::new(x, 110, 100, 70).unwrap();
            ResolvedEffectInstance {
                composite_opacity: None,
                id: EffectInstanceId::new(u64::from(surface)).unwrap(),
                program: program_id,
                anchor,
                region: EffectRegion::from_rect(region),
                target_bounds: region,
                parameter_block: Default::default(),
                coverage: None,
                signature: u64::from(surface),
                frame_demand: EffectFrameDemand::OnDamage,
                visual_group: precomputed_candidate.then(|| VisualGroupId::new(surface).unwrap()),
                anchor_scope: EffectAnchorScope::VisualGroup,
                backdrop_reuse: if precomputed_candidate {
                    EffectBackdropReuse::PrecomputedWindowCandidate
                } else {
                    EffectBackdropReuse::Exact
                },
                scene_order: EffectSceneOrder {
                    group_order: 0,
                    surface_order: surface,
                    phase: 0,
                },
            }
        })
        .collect();
    (ResolvedEffectScene::new(1, instances), registry, bounds)
}

fn scene_capture_dependencies(
    graph: &oblivion_one::effects::CompiledFrameGraph,
    consumer: EffectInstanceId,
) -> &[oblivion_one::effects::GraphPassId] {
    &graph
        .passes
        .iter()
        .find(|pass| pass.instance == consumer && pass.kind == RenderPassKind::SceneCapture)
        .unwrap()
        .checkpoint_dependencies
}

#[test]
fn sampled_support_disjointness_prunes_both_checkpoint_representations() {
    let mut graph = two_material_graph(270);
    let producer = graph.instances[0].id;
    let consumer = graph.instances[1].id;
    assert_eq!(graph.instances[1].dependencies, vec![producer]);
    assert_eq!(scene_capture_dependencies(&graph, consumer).len(), 1);

    let support = derive_effect_sample_support(&graph, consumer).unwrap();
    assert!(
        !graph.instances[0]
            .output_influence_region
            .intersects(&support.backdrop_region)
    );

    let plan = plan_effect_checkpoint_dependency_pruning(&graph);
    assert_eq!(plan.stats.edges_before, 1);
    assert_eq!(plan.stats.edges_retained_intersecting, 0);
    assert_eq!(plan.stats.edges_pruned, 1);
    assert_eq!(plan.stats.edges_ineligible, 0);
    assert_eq!(plan.stats.edges_unresolved, 0);
    assert_eq!(plan.stats.edges_after, 0);
    assert_eq!(plan.stats.consumers_became_dependency_free, 1);
    assert_eq!(
        plan.decisions[0].kind,
        EffectCheckpointDependencyDecisionKind::PrunedSampleSupportDisjoint
    );

    apply_effect_checkpoint_dependency_pruning(&mut graph, &plan).unwrap();
    assert!(graph.instances[1].dependencies.is_empty());
    assert!(scene_capture_dependencies(&graph, consumer).is_empty());
}

#[test]
fn sampled_support_overlap_retains_the_checkpoint_edge() {
    let graph = two_material_graph(250);
    let producer = graph.instances[0].id;
    let consumer = graph.instances[1].id;
    let support = derive_effect_sample_support(&graph, consumer).unwrap();
    assert!(
        graph.instances[0]
            .output_influence_region
            .intersects(&support.backdrop_region)
    );

    let plan = plan_effect_checkpoint_dependency_pruning(&graph);
    assert_eq!(plan.stats.edges_before, 1);
    assert_eq!(plan.stats.edges_retained_intersecting, 1);
    assert_eq!(plan.stats.edges_pruned, 0);
    assert_eq!(
        plan.decisions[0].kind,
        EffectCheckpointDependencyDecisionKind::RetainedSampledSupportIntersection
    );
    assert_eq!(graph.instances[1].dependencies, vec![producer]);
}

#[test]
fn direct_frame_execution_plan_keeps_the_conservative_dependency_graph() {
    let (scene, registry, bounds) = two_material_scene(270);
    let plan =
        compile_frame_execution_plan(&scene, &EffectRegion::from_rect(bounds), bounds, &registry)
            .unwrap();
    let FrameExecutionPlan::EffectGraph(graph) = plan else {
        panic!("two canonical materials must compile to an effect graph");
    };
    assert_eq!(graph.instances[1].dependencies, vec![graph.instances[0].id]);
}

#[test]
fn unsupported_consumer_topologies_retain_all_edges_as_ineligible() {
    let baseline = two_material_graph(270);
    let consumer = baseline.instances[1].id;
    let capture_index = baseline
        .passes
        .iter()
        .position(|pass| pass.instance == consumer && pass.kind == RenderPassKind::SceneCapture)
        .unwrap();

    let mut multiple_scene_captures = baseline.clone();
    multiple_scene_captures
        .passes
        .push(multiple_scene_captures.passes[capture_index].clone());

    let mut scene_and_surface_capture = baseline.clone();
    let mut surface_capture = scene_and_surface_capture.passes[capture_index].clone();
    surface_capture.kind = RenderPassKind::SurfaceCapture;
    scene_and_surface_capture.passes.push(surface_capture);

    let mut continuous_demand = baseline.clone();
    continuous_demand.instances[1].frame_demand = EffectFrameDemand::Continuous;

    for mut graph in [
        multiple_scene_captures,
        scene_and_surface_capture,
        continuous_demand,
    ] {
        let original = graph.clone();
        let plan = plan_effect_checkpoint_dependency_pruning(&graph);
        assert_eq!(plan.stats.edges_before, 1);
        assert_eq!(plan.stats.edges_pruned, 0);
        assert_eq!(plan.stats.edges_ineligible, 1);
        assert_eq!(plan.stats.edges_unresolved, 0);
        assert!(plan.stats.accounting_is_valid());
        apply_effect_checkpoint_dependency_pruning(&mut graph, &plan).unwrap();
        assert_eq!(graph, original);
    }
}

#[test]
fn malformed_dependency_metadata_is_retained_and_classified_unresolved() {
    let baseline = two_material_graph(270);
    let producer = baseline.instances[0].id;
    let consumer = baseline.instances[1].id;

    let mut pass_list_mismatch = baseline.clone();
    pass_list_mismatch
        .passes
        .iter_mut()
        .find(|pass| pass.instance == consumer && pass.kind == RenderPassKind::SceneCapture)
        .unwrap()
        .checkpoint_dependencies
        .clear();

    let mut missing_producer_final_pass = baseline.clone();
    missing_producer_final_pass.passes.retain(|pass| {
        pass.instance != producer
            || !matches!(
                pass.kind,
                RenderPassKind::Composite | RenderPassKind::OutputPostProcess
            )
    });

    let mut ambiguous_producer_final_pass = baseline.clone();
    let producer_final = ambiguous_producer_final_pass
        .passes
        .iter()
        .find(|pass| {
            pass.instance == producer
                && matches!(
                    pass.kind,
                    RenderPassKind::Composite | RenderPassKind::OutputPostProcess
                )
        })
        .unwrap()
        .clone();
    ambiguous_producer_final_pass.passes.push(producer_final);

    let mut unavailable_sample_support = baseline.clone();
    unavailable_sample_support.passes.retain(|pass| {
        pass.instance != consumer
            || !matches!(
                pass.kind,
                RenderPassKind::Composite | RenderPassKind::OutputPostProcess
            )
    });

    for mut graph in [
        pass_list_mismatch,
        missing_producer_final_pass,
        ambiguous_producer_final_pass,
        unavailable_sample_support,
    ] {
        let original = graph.clone();
        let plan = plan_effect_checkpoint_dependency_pruning(&graph);
        assert_eq!(plan.stats.edges_before, 1);
        assert_eq!(plan.stats.edges_pruned, 0);
        assert_eq!(plan.stats.edges_unresolved, 1);
        assert_eq!(plan.stats.edges_ineligible, 0);
        assert!(plan.stats.accounting_is_valid());
        apply_effect_checkpoint_dependency_pruning(&mut graph, &plan).unwrap();
        assert_eq!(graph, original);
    }
}

#[test]
fn terminal_fusion_on_and_off_produce_the_same_pruning_decisions() {
    let unfused = two_material_variants(270, false, true);
    let fused = two_material_variants(270, true, true);

    assert_eq!(
        unfused.checkpoint_dependency_pruning.decisions,
        fused.checkpoint_dependency_pruning.decisions
    );
    assert_eq!(
        unfused.checkpoint_dependency_pruning.stats.edges_pruned,
        fused.checkpoint_dependency_pruning.stats.edges_pruned
    );
}

#[test]
fn policy_off_and_on_preserve_every_non_dependency_graph_field() {
    let off = two_material_variants(270, true, false);
    let on = two_material_variants(270, true, true);
    let FrameExecutionPlan::EffectGraph(mut off_graph) = off.exact else {
        panic!("policy-off graph must be exact");
    };
    let FrameExecutionPlan::EffectGraph(mut on_graph) = on.exact else {
        panic!("policy-on graph must be exact");
    };
    assert_eq!(off_graph.instances[1].dependencies.len(), 1);
    assert!(on_graph.instances[1].dependencies.is_empty());
    assert_eq!(off_graph.passes.len(), on_graph.passes.len());
    assert_eq!(off_graph.textures.len(), on_graph.textures.len());

    for graph in [&mut off_graph, &mut on_graph] {
        for instance in &mut graph.instances {
            instance.dependencies.clear();
        }
        for pass in &mut graph.passes {
            if pass.kind == RenderPassKind::SceneCapture {
                pass.checkpoint_dependencies.clear();
            }
        }
    }
    assert_eq!(off_graph, on_graph);
}

#[test]
fn precomputed_eligibility_uses_the_raw_exact_graph_before_g3a_pruning() {
    let (scene, registry, bounds) = precomputed_isolation_scene();
    let options = |checkpoint_dependency_pruning| FrameExecutionCompileOptions {
        precomputed_window_blur: true,
        terminal_kawase_fusion: false,
        checkpoint_dependency_pruning,
    };
    let raw = compile_frame_execution_variants_with_options(
        &scene,
        &EffectRegion::from_rect(bounds),
        bounds,
        &registry,
        options(false),
    )
    .unwrap();
    let raw_graph = match &raw.exact {
        FrameExecutionPlan::EffectGraph(graph) => graph,
        FrameExecutionPlan::LegacyScene => panic!("three materials must compile"),
    };
    assert_eq!(raw.eligible_instances, 2);
    assert_eq!(
        raw_graph.instances[1].dependencies,
        vec![raw_graph.instances[0].id]
    );
    let raw_support = derive_effect_sample_support(raw_graph, raw_graph.instances[1].id).unwrap();
    assert!(
        !raw_graph.instances[0]
            .output_influence_region
            .intersects(&raw_support.backdrop_region)
    );
    assert!(raw.precomputed_backdrop.is_none());
    assert!(raw.precomputed_graph.is_none());

    let pruned = compile_frame_execution_variants_with_options(
        &scene,
        &EffectRegion::from_rect(bounds),
        bounds,
        &registry,
        options(true),
    )
    .unwrap();
    let pruned_graph = match &pruned.exact {
        FrameExecutionPlan::EffectGraph(graph) => graph,
        FrameExecutionPlan::LegacyScene => panic!("three materials must compile"),
    };
    assert!(pruned_graph.instances[1].dependencies.is_empty());
    assert!(scene_capture_dependencies(pruned_graph, pruned_graph.instances[1].id).is_empty());
    assert!(pruned.precomputed_backdrop.is_none());
    assert!(pruned.precomputed_graph.is_none());
}
