use oblivion_one::compositor::{
    EffectAnchor, EffectAnchorScope, EffectBackdropReuse, EffectSceneOrder, ResolvedEffectInstance,
    ResolvedEffectScene,
};
use oblivion_one::effects::{
    CompiledFrameGraph, DualKawaseBlurSpec, EffectAlphaMode, EffectDamagePlan, EffectFailurePolicy,
    EffectFootprint, EffectFrameDemand, EffectInstanceId, EffectNode, EffectNodeId, EffectProgram,
    EffectProgramId, EffectRect, EffectRegion, EffectRegistry, EffectSource, EffectWorkingSpace,
    FrameExecutionCompileOptions, FrameExecutionPlan, GraphTextureSource, RenderPassKind,
    builtin_background_material_program, compile_frame_execution_plan,
    compile_frame_execution_variants_with_options, dual_kawase_sampling_radius, plan_effect_damage,
    required_input_region, validate_effect_program,
};
use oblivion_one::material::{EffectiveMaterial, MaterialConfiguration};

fn blur_graph(
    domain: EffectRect,
    output_bounds: EffectRect,
    source_damage: &EffectRegion,
    radius: f32,
    passes: u8,
    scale: f32,
) -> (CompiledFrameGraph, EffectFootprint, EffectDamagePlan) {
    let source = EffectNodeId::new(1).unwrap();
    let blur = EffectNodeId::new(2).unwrap();
    let program = EffectProgram {
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
    };
    let validated = validate_effect_program(program).unwrap();
    let footprint = validated.aggregate_footprint;
    let mut registry = EffectRegistry::empty();
    registry.insert(validated).unwrap();
    let anchor = EffectAnchor::BeforeSurface(1);
    let instance = ResolvedEffectInstance {
        composite_opacity: None,
        id: EffectInstanceId::new(1).unwrap(),
        program: EffectProgramId::new(1).unwrap(),
        anchor,
        region: EffectRegion::from_rect(domain),
        target_bounds: domain,
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
    let damage = plan_effect_damage(
        footprint,
        &EffectRegion::from_rect(domain),
        source_damage,
        output_bounds,
    );
    let FrameExecutionPlan::EffectGraph(graph) =
        compile_frame_execution_plan(&scene, source_damage, output_bounds, &registry).unwrap()
    else {
        panic!("Dual Kawase must compile to a render graph");
    };
    (graph, footprint, damage)
}

fn canonical_material_graph(
    domain: EffectRect,
    output_bounds: EffectRect,
    source_damage: &EffectRegion,
) -> (CompiledFrameGraph, EffectFootprint, EffectDamagePlan) {
    let material = EffectiveMaterial {
        blur: 0.42,
        saturation: 0.31,
        noise: 0.3,
        blur_passes: 3,
    };
    let validated = builtin_background_material_program(material);
    let footprint = validated.aggregate_footprint;
    let program_id = validated.program.id;
    let mut registry = EffectRegistry::empty();
    registry.insert(validated).unwrap();
    let anchor = EffectAnchor::BeforeSurface(1);
    let instance = ResolvedEffectInstance {
        composite_opacity: None,
        id: EffectInstanceId::new(1).unwrap(),
        program: program_id,
        anchor,
        region: EffectRegion::from_rect(domain),
        target_bounds: domain,
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
    let damage = plan_effect_damage(
        footprint,
        &EffectRegion::from_rect(domain),
        source_damage,
        output_bounds,
    );
    let FrameExecutionPlan::EffectGraph(graph) =
        compile_frame_execution_plan(&scene, source_damage, output_bounds, &registry).unwrap()
    else {
        panic!("built-in background material must compile to a render graph");
    };
    (graph, footprint, damage)
}

fn source_dependency_for_output_pixel(
    graph: &CompiledFrameGraph,
    pixel: EffectRect,
) -> EffectRegion {
    let mut texture_id = graph
        .textures
        .iter()
        .find(|texture| texture.source == GraphTextureSource::Output)
        .expect("compiled output texture")
        .id;
    let mut demanded = EffectRegion::from_rect(pixel);

    for pass in graph.passes.iter().rev() {
        if pass.output != Some(texture_id) {
            continue;
        }
        if let Some(input_id) = pass.inputs.first().copied() {
            let output = graph
                .textures
                .iter()
                .find(|texture| texture.id == texture_id)
                .expect("compiled pass output texture");
            let input = graph
                .textures
                .iter()
                .find(|texture| texture.id == input_id)
                .expect("compiled pass input texture");
            demanded = required_input_region(pass, &demanded, output, input)
                .expect("compiled pass has reverse dependency metadata");
            texture_id = input_id;
        } else {
            assert_eq!(pass.kind, RenderPassKind::SceneCapture);
            return demanded;
        }
    }
    panic!("output pixel did not trace back to a scene capture");
}

fn pass_accurate_output_influences(
    graph: &CompiledFrameGraph,
    visible_region: &EffectRegion,
    source_repairs: &[EffectRegion],
) -> Vec<EffectRegion> {
    let mut influences = vec![EffectRegion::empty(); source_repairs.len()];
    for visible in visible_region.rects() {
        for y in visible.y..visible.bottom() {
            let mut run_starts = vec![None; source_repairs.len()];
            for x in visible.x..visible.right() {
                let pixel = EffectRect::new(x, y, 1, 1).unwrap();
                let required_source = source_dependency_for_output_pixel(graph, pixel);
                for (index, source_repair) in source_repairs.iter().enumerate() {
                    let affected = required_source.intersects(source_repair);
                    match (run_starts[index], affected) {
                        (None, true) => run_starts[index] = Some(x),
                        (Some(start), false) => {
                            influences[index]
                                .push(EffectRect::new(start, y, (x - start) as u32, 1).unwrap());
                            run_starts[index] = None;
                        }
                        _ => {}
                    }
                }
            }
            for (index, run_start) in run_starts.into_iter().enumerate() {
                if let Some(start) = run_start {
                    influences[index].push(
                        EffectRect::new(start, y, (visible.right() - start) as u32, 1).unwrap(),
                    );
                }
            }
        }
    }
    influences
}

fn region_contains_pixel(region: &EffectRegion, x: i32, y: i32) -> bool {
    region
        .rects()
        .iter()
        .any(|rect| x >= rect.x && x < rect.right() && y >= rect.y && y < rect.bottom())
}

fn region_pixel_count(region: &EffectRegion) -> u64 {
    region.rects().iter().fold(0, |total, rect| {
        total + u64::from(rect.width) * u64::from(rect.height)
    })
}

fn region_pixels_missing_from(expected: &EffectRegion, actual: &EffectRegion) -> Vec<(i32, i32)> {
    expected
        .rects()
        .iter()
        .flat_map(|rect| {
            (rect.y..rect.bottom()).flat_map(move |y| {
                (rect.x..rect.right())
                    .filter_map(move |x| (!region_contains_pixel(actual, x, y)).then_some((x, y)))
            })
        })
        .collect()
}

#[test]
fn dual_kawase_damage_footprint_covers_compiled_graph_oracle_for_101_by_75_fixture() {
    let domain = EffectRect::new(19, 13, 101, 75).unwrap();
    let visible_region = EffectRegion::from_rect(domain);
    let source_damage = EffectRegion::from_rect(EffectRect::new(20, 14, 3, 2).unwrap());
    let (graph, footprint, plan) = blur_graph(domain, domain, &source_damage, 5.36, 2, 1.0);

    let mut dimensions = vec![
        graph
            .textures
            .iter()
            .find(|texture| texture.source == GraphTextureSource::CapturedScene)
            .expect("captured source texture"),
    ];
    dimensions.extend(graph.passes.iter().filter_map(|pass| {
        matches!(
            pass.kind,
            RenderPassKind::DualKawaseDownsample | RenderPassKind::DualKawaseUpsample
        )
        .then(|| {
            graph
                .textures
                .iter()
                .find(|texture| texture.id == pass.output.unwrap())
                .expect("Kawase output texture")
        })
    }));
    assert_eq!(
        dimensions
            .iter()
            .map(|texture| (texture.width, texture.height))
            .collect::<Vec<_>>(),
        vec![(101, 75), (51, 38), (26, 19), (51, 38), (101, 75)]
    );

    let oracle = pass_accurate_output_influences(&graph, &visible_region, &[source_damage.clone()]);
    let oracle = &oracle[0];

    // Keep a durable red-side regression for the aggregate formula that
    // preceded this fix. It is computed from that formula rather than baked
    // in as a fixture-specific radius.
    let legacy_radius = (f64::from(5.36_f32) * 2.0 * 3.0_f64).ceil() as u32;
    let legacy_plan = plan_effect_damage(
        EffectFootprint::symmetric(legacy_radius),
        &visible_region,
        &source_damage,
        domain,
    );
    let legacy_missing = region_pixels_missing_from(oracle, &legacy_plan.output_damage);
    let legacy_missing_bounds: Option<(i32, i32, i32, i32)> =
        legacy_missing.iter().fold(None, |bounds, (x, y)| {
            Some(match bounds {
                None => (*x, *y, *x + 1, *y + 1),
                Some((left, top, right, bottom)) => (
                    left.min(*x),
                    top.min(*y),
                    right.max(*x + 1),
                    bottom.max(*y + 1),
                ),
            })
        });
    eprintln!(
        "legacy Dual Kawase undercoverage: aggregate_radius={legacy_radius} planned={:?} oracle={:?} missing_pixels={} missing_bounds={legacy_missing_bounds:?}",
        legacy_plan.output_damage.bounding_rect(),
        oracle.bounding_rect(),
        legacy_missing.len(),
    );
    assert!(
        !legacy_missing.is_empty(),
        "the pre-fix aggregate unexpectedly covers the pass-accurate oracle"
    );

    let missing = region_pixels_missing_from(oracle, &plan.output_damage);
    let missing_bounds: Option<(i32, i32, i32, i32)> =
        missing.iter().fold(None, |bounds, (x, y)| {
            Some(match bounds {
                None => (*x, *y, *x + 1, *y + 1),
                Some((left, top, right, bottom)) => (
                    left.min(*x),
                    top.min(*y),
                    right.max(*x + 1),
                    bottom.max(*y + 1),
                ),
            })
        });
    eprintln!(
        "Dual Kawase oracle fixture: aggregate_radius={} planned={:?} oracle={:?} oracle_pixels={} missing_pixels={} missing_bounds={missing_bounds:?}",
        footprint.sample_radius_x,
        plan.output_damage.bounding_rect(),
        oracle.bounding_rect(),
        region_pixel_count(&oracle),
        missing.len(),
    );
    assert!(
        missing.is_empty(),
        "aggregate planner omitted {} pass-accurate output pixels; missing bounds {missing_bounds:?}",
        missing.len(),
    );
}

#[test]
fn canonical_3_by_2_repair_plans_all_visible_effect_output_damage() {
    let output_bounds = EffectRect::new(0, 0, 161, 121).unwrap();
    let domain = EffectRect::new(19, 13, 101, 75).unwrap();
    let visible = EffectRegion::from_rect(domain);
    let source_repair = EffectRegion::from_rect(EffectRect::new(20, 14, 3, 2).unwrap());
    let (graph, footprint, plan) = canonical_material_graph(domain, output_bounds, &source_repair);

    assert_eq!(footprint.sample_radius_x, 101);
    assert_eq!(plan.output_damage.bounding_rect(), Some(domain));
    assert_eq!(plan.capture_region.bounding_rect(), Some(output_bounds));
    assert_eq!(plan.source_query_region, plan.capture_region);
    assert_eq!(graph.final_damage.bounding_rect(), Some(domain));
    eprintln!(
        "canonical damage plan: source_repair={:?} aggregate_radius={} output_damage={:?} capture_region={:?} final_damage={:?}",
        source_repair.bounding_rect(),
        footprint.sample_radius_x,
        plan.output_damage.bounding_rect(),
        plan.capture_region.bounding_rect(),
        graph.final_damage.bounding_rect(),
    );
    let oracle = pass_accurate_output_influences(&graph, &visible, &[source_repair]);
    assert!(region_pixels_missing_from(&oracle[0], &plan.output_damage).is_empty());
}

fn representative_repairs(domain: EffectRect) -> Vec<EffectRegion> {
    let width = domain.width.min(3);
    let height = domain.height.min(2);
    let center = EffectRect::new(
        domain.x + (domain.width / 2) as i32,
        domain.y + (domain.height / 2) as i32,
        1,
        1,
    )
    .unwrap();
    let top_left = EffectRect::new(domain.x, domain.y, 1, 1).unwrap();
    let bottom_right = EffectRect::new(domain.right() - 1, domain.bottom() - 1, 1, 1).unwrap();
    let small = EffectRect::new(
        domain.x + ((domain.width - width) / 2) as i32,
        domain.y + ((domain.height - height) / 2) as i32,
        width,
        height,
    )
    .unwrap();
    let near_edge =
        EffectRect::new(domain.right() - width as i32, domain.y, width, height).unwrap();
    [center, top_left, bottom_right, small, near_edge]
        .into_iter()
        .map(EffectRegion::from_rect)
        .collect()
}

fn assert_oracle_is_inside_damage(
    graph: &CompiledFrameGraph,
    domain: EffectRect,
    repairs: &[EffectRegion],
    footprint: EffectFootprint,
    output_bounds: EffectRect,
    context: &str,
) {
    let visible = EffectRegion::from_rect(domain);
    let oracles = pass_accurate_output_influences(graph, &visible, repairs);
    for (source_repair, oracle) in repairs.iter().zip(oracles.iter()) {
        let plan = plan_effect_damage(footprint, &visible, source_repair, output_bounds);
        let missing = region_pixels_missing_from(oracle, &plan.output_damage);
        assert!(
            missing.is_empty(),
            "{context}: repair={:?} footprint={} planned={:?} oracle={:?} missing={} first_missing={:?}",
            source_repair.bounding_rect(),
            footprint.sample_radius_x,
            plan.output_damage.bounding_rect(),
            oracle.bounding_rect(),
            missing.len(),
            missing.first(),
        );
    }
}

#[test]
fn aggregate_footprint_covers_adversarial_compiled_blur_graphs() {
    let small_domain = EffectRect::new(145, 148, 9, 7).unwrap();
    for passes in 1..=4 {
        for scale in [1.0, 0.75, 0.5, 0.25, 0.125, 0.0625] {
            for radius in [0.125, 1.0, 2.0, 5.36, 8.0, 10.0] {
                let repairs = representative_repairs(small_domain);
                let (graph, footprint, _) = blur_graph(
                    small_domain,
                    small_domain,
                    &repairs[0],
                    radius,
                    passes,
                    scale,
                );
                assert_oracle_is_inside_damage(
                    &graph,
                    small_domain,
                    &repairs,
                    footprint,
                    small_domain,
                    &format!("9x7 passes={passes} scale={scale} radius={radius}"),
                );
            }
        }
    }

    let dimension_cases = [
        (EffectRect::new(0, 0, 1, 1).unwrap(), 4, 0.125, 10.0),
        (EffectRect::new(1, 3, 7, 5).unwrap(), 3, 0.0625, 8.0),
        (EffectRect::new(145, 148, 2, 3).unwrap(), 1, 0.75, 1.0),
        (EffectRect::new(0, 0, 320, 180).unwrap(), 1, 1.0, 0.125),
        (EffectRect::new(0, 0, 101, 74).unwrap(), 2, 0.75, 2.0),
        (EffectRect::new(145, 148, 100, 75).unwrap(), 3, 0.5, 5.36),
        (EffectRect::new(19, 13, 101, 75).unwrap(), 2, 1.0, 5.36),
        (EffectRect::new(0, 0, 321, 181).unwrap(), 4, 0.125, 10.0),
    ];
    for (domain, passes, scale, radius) in dimension_cases {
        let repairs = representative_repairs(domain);
        let (graph, footprint, _) = blur_graph(domain, domain, &repairs[0], radius, passes, scale);
        assert_oracle_is_inside_damage(
            &graph,
            domain,
            &repairs,
            footprint,
            domain,
            &format!(
                "{}x{} at ({},{}) passes={passes} scale={scale} radius={radius}",
                domain.width, domain.height, domain.x, domain.y
            ),
        );
    }
}

#[test]
fn capture_and_source_query_cover_complete_visible_output_dependencies() {
    let output_bounds = EffectRect::new(0, 0, 640, 480).unwrap();
    let visible = EffectRect::new(270, 200, 101, 75).unwrap();
    let no_source_damage = EffectRegion::empty();

    // A full-output captured texture is the independent source extent, so the
    // reverse oracle can report any texels omitted by the candidate capture.
    let (full_source_graph, footprint, _) = blur_graph(
        output_bounds,
        output_bounds,
        &no_source_damage,
        5.36,
        2,
        1.0,
    );
    let plan = plan_effect_damage(
        footprint,
        &EffectRegion::from_rect(visible),
        &no_source_damage,
        output_bounds,
    );
    assert_eq!(plan.source_query_region, plan.capture_region);
    for y in visible.y..visible.bottom() {
        for x in visible.x..visible.right() {
            let required = source_dependency_for_output_pixel(
                &full_source_graph,
                EffectRect::new(x, y, 1, 1).unwrap(),
            );
            for rect in required.rects() {
                for source_y in rect.y..rect.bottom() {
                    for source_x in rect.x..rect.right() {
                        assert!(
                            region_contains_pixel(&plan.capture_region, source_x, source_y),
                            "capture {:?} omitted source texel ({source_x},{source_y}) required for output ({x},{y})",
                            plan.capture_region.bounding_rect(),
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn expanded_capture_keeps_overlapping_backdrop_checkpoint_dependency() {
    let output_bounds = EffectRect::new(0, 0, 400, 120).unwrap();
    let first_region = EffectRect::new(60, 20, 30, 30).unwrap();
    let second_region = EffectRect::new(160, 20, 30, 30).unwrap();
    let source = EffectNodeId::new(1).unwrap();
    let blur = EffectNodeId::new(2).unwrap();
    let validated = validate_effect_program(EffectProgram {
        id: EffectProgramId::new(1).unwrap(),
        nodes: vec![
            EffectNode::source(source, EffectSource::Backdrop),
            EffectNode::dual_kawase(blur, source, DualKawaseBlurSpec::new(5.36, 2, 1.0).unwrap()),
        ],
        output: blur,
        working_space: EffectWorkingSpace::LinearSrgb,
        alpha_mode: EffectAlphaMode::Opaque,
        outsets: Default::default(),
        frame_demand: EffectFrameDemand::OnDamage,
        failure_policy: EffectFailurePolicy::Passthrough,
    })
    .unwrap();
    let footprint = validated.aggregate_footprint;
    let mut registry = EffectRegistry::empty();
    registry.insert(validated).unwrap();

    let make_instance = |id: u64, region: EffectRect| {
        let anchor = EffectAnchor::BeforeSurface(u32::try_from(id).unwrap());
        ResolvedEffectInstance {
            composite_opacity: None,
            id: EffectInstanceId::new(id).unwrap(),
            program: EffectProgramId::new(1).unwrap(),
            anchor,
            region: EffectRegion::from_rect(region),
            target_bounds: region,
            parameter_block: Default::default(),
            coverage: None,
            signature: id,
            frame_demand: EffectFrameDemand::OnDamage,
            visual_group: None,
            anchor_scope: EffectAnchorScope::VisualGroup,
            backdrop_reuse: EffectBackdropReuse::Exact,
            scene_order: EffectSceneOrder {
                group_order: 0,
                surface_order: u32::try_from(id).unwrap(),
                phase: 0,
            },
        }
    };
    let scene = ResolvedEffectScene::new(
        1,
        vec![
            make_instance(1, first_region),
            make_instance(2, second_region),
        ],
    );
    let source_damage = EffectRegion::from_rect(output_bounds);
    let FrameExecutionPlan::EffectGraph(graph) =
        compile_frame_execution_plan(&scene, &source_damage, output_bounds, &registry).unwrap()
    else {
        panic!("backdrop blurs must compile to an effect graph");
    };

    let first_id = EffectInstanceId::new(1).unwrap();
    let second_id = EffectInstanceId::new(2).unwrap();
    let first = graph
        .instances
        .iter()
        .find(|instance| instance.id == first_id)
        .expect("earlier backdrop instance");
    let second = graph
        .instances
        .iter()
        .find(|instance| instance.id == second_id)
        .expect("later backdrop instance");
    let second_plan = plan_effect_damage(
        footprint,
        &EffectRegion::from_rect(second_region),
        &source_damage,
        output_bounds,
    );
    assert_eq!(second_plan.source_query_region, second_plan.capture_region);
    assert!(
        second
            .capture_region
            .intersects(&first.output_influence_region)
    );
    assert_eq!(second.dependencies, vec![first_id]);
}

#[test]
fn footprint_change_preserves_builtin_terminal_fusion_topology() {
    let bounds = EffectRect::new(0, 0, 2_400, 600).unwrap();
    let material = EffectiveMaterial {
        blur: 0.42,
        saturation: 0.31,
        noise: 0.3,
        blur_passes: 3,
    };
    let validated = builtin_background_material_program(material);
    let program_id = validated.program.id;
    let mut registry = EffectRegistry::empty();
    registry.insert(validated).unwrap();
    let instances = (0..15)
        .map(|index| {
            let surface = index + 1;
            let anchor = EffectAnchor::BeforeSurface(surface);
            let x = 20 + i32::try_from(index).unwrap() * 110;
            let region = EffectRect::new(x, 40, 90, 60).unwrap();
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
    let source_damage = EffectRegion::from_rect(bounds);
    let compile = |terminal_kawase_fusion| {
        let variants = compile_frame_execution_variants_with_options(
            &scene,
            &source_damage,
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
            panic!("built-in background materials must compile to an effect graph");
        };
        graph
    };
    let staged = compile(false);
    let fused = compile(true);

    assert_eq!(staged.stats.passes, 105);
    assert_eq!(fused.stats.passes, 75);
    assert_eq!(fused.stats.terminal_blur_fusions, 15);
    assert_eq!(fused.stats.terminal_blur_passes_elided, 30);
}

#[test]
fn built_in_material_footprint_radii_follow_the_documented_bound() {
    let default_material = MaterialConfiguration::default().effective().unwrap();
    let maximum_material = EffectiveMaterial {
        blur: 1.0,
        saturation: 1.0,
        noise: 1.0,
        blur_passes: 3,
    };
    let fixture_radius = dual_kawase_sampling_radius(5.36, 2, 1.0).unwrap();
    let default_radius =
        dual_kawase_sampling_radius(2.0 + default_material.blur * 8.0, 2, 1.0).unwrap();
    let maximum_radius =
        dual_kawase_sampling_radius(2.0 + maximum_material.blur * 8.0, 2, 1.0).unwrap();

    assert_eq!(fixture_radius, 101);
    assert_eq!(default_radius, 111);
    assert_eq!(maximum_radius, 142);
    eprintln!(
        "Dual Kawase radii (old -> new): fixture 33 -> {fixture_radius}, default 39 -> {default_radius}, maximum 60 -> {maximum_radius}"
    );
}
