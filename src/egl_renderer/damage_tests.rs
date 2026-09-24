use super::*;

fn rect(x: i32, y: i32, width: u32, height: u32) -> OutputRect {
    OutputRect::new(x, y, width, height)
}

fn effect_rect(x: i32, y: i32, width: u32, height: u32) -> oblivion_one::effects::EffectRect {
    oblivion_one::effects::EffectRect::new(x, y, width, height).unwrap()
}

fn partial_capabilities() -> EglPartialRepaintCapabilities {
    EglPartialRepaintCapabilities {
        buffer_age: true,
        partial_render_repair: true,
        swap_buffers_with_damage: true,
    }
}

fn partial_planner(
    output_size: (u32, u32),
    capabilities: EglPartialRepaintCapabilities,
) -> PartialRepaintPlanner {
    partial_planner_with_policy(
        output_size,
        capabilities,
        PartialRepaintComplexityPolicy::Legacy,
    )
}

fn partial_planner_with_policy(
    output_size: (u32, u32),
    capabilities: EglPartialRepaintCapabilities,
    policy: PartialRepaintComplexityPolicy,
) -> PartialRepaintPlanner {
    let mut planner = PartialRepaintPlanner::new_with_policy(output_size, capabilities, policy);
    planner.partial_enabled = true;
    planner
}

fn dense_many_rect_damage() -> OutputDamage {
    OutputDamage::rects(
        100,
        100,
        [0, 11, 22]
            .into_iter()
            .flat_map(|y| [0, 11, 22].into_iter().map(move |x| rect(x, y, 10, 10))),
    )
}

fn sparse_many_rect_damage() -> OutputDamage {
    OutputDamage::rects(250, 10, (0..9).map(|index| rect(index * 30, 0, 10, 10)))
}

#[test]
fn damage_complexity_shadow_is_not_applicable_to_simple_damage() {
    let cases = [
        OutputDamage::Empty,
        OutputDamage::Full,
        OutputDamage::rects(100, 100, (0..8).map(|index| rect(index * 10, 0, 5, 5))),
    ];

    for candidate in cases {
        let shadow = DamageComplexityShadow::for_candidate(
            &candidate,
            (100, 100),
            Some(FullRepaintReason::TooManyRectangles),
        );
        assert_eq!(shadow.outcome, DamageComplexityShadowOutcome::NotApplicable);
        assert!(!shadow.applicable);
        assert_eq!(shadow, DamageComplexityShadow::not_applicable());
    }
}

#[test]
fn damage_complexity_shadow_outcomes_have_stable_names() {
    assert_eq!(
        DamageComplexityShadowOutcome::NotApplicable.as_str(),
        "not_applicable"
    );
    assert_eq!(
        DamageComplexityShadowOutcome::PartialBoundingBox.as_str(),
        "partial_bbox"
    );
    assert_eq!(
        DamageComplexityShadowOutcome::PartialManyRectangles.as_str(),
        "partial_many_rects"
    );
    assert_eq!(
        DamageComplexityShadowOutcome::FullAreaThreshold.as_str(),
        "full_area_threshold"
    );
    assert_eq!(
        DamageComplexityShadowOutcome::Unavailable.as_str(),
        "unavailable"
    );
}

#[test]
fn damage_complexity_shadow_accepts_dense_bounding_box_at_two_x_factor() {
    let candidate = dense_many_rect_damage();
    let shadow = DamageComplexityShadow::for_candidate(
        &candidate,
        (100, 100),
        Some(FullRepaintReason::TooManyRectangles),
    );

    assert_eq!(candidate.rect_count(), 9);
    assert!(shadow.applicable);
    assert_eq!(shadow.original_rects, 9);
    assert_eq!(shadow.original_pixels, 900);
    assert_eq!(shadow.bbox, Some(rect(0, 0, 32, 32)));
    assert_eq!(shadow.bbox_pixels, 1024);
    assert!(shadow.bbox_accepted);
    assert_eq!(shadow.candidate_rects, 1);
    assert_eq!(shadow.candidate_pixels, 1024);
    assert_eq!(shadow.added_pixels, 124);
    assert_eq!(
        shadow.outcome,
        DamageComplexityShadowOutcome::PartialBoundingBox
    );
    assert!(shadow.would_avoid_full);
}

#[test]
fn damage_complexity_shadow_keeps_sparse_many_rectangles() {
    let candidate = sparse_many_rect_damage();
    let shadow = DamageComplexityShadow::for_candidate(
        &candidate,
        (250, 10),
        Some(FullRepaintReason::TooManyRectangles),
    );

    assert_eq!(shadow.original_rects, 9);
    assert_eq!(shadow.original_pixels, 900);
    assert_eq!(shadow.bbox, Some(rect(0, 0, 250, 10)));
    assert_eq!(shadow.bbox_pixels, 2500);
    assert!(!shadow.bbox_accepted);
    assert_eq!(shadow.candidate_rects, 9);
    assert_eq!(shadow.candidate_pixels, 900);
    assert_eq!(shadow.added_pixels, 0);
    assert_eq!(
        shadow.outcome,
        DamageComplexityShadowOutcome::PartialManyRectangles
    );
    assert!(shadow.would_avoid_full);
}

#[test]
fn damage_complexity_shadow_is_scoped_to_rectangle_fallback_reason() {
    let shadow = DamageComplexityShadow::for_candidate(
        &dense_many_rect_damage(),
        (100, 100),
        Some(FullRepaintReason::DamageAreaThreshold),
    );

    assert_eq!(shadow.outcome, DamageComplexityShadowOutcome::NotApplicable);
    assert!(!shadow.would_avoid_full);
}

#[test]
fn damage_complexity_shadow_preserves_area_threshold_for_bbox_candidate() {
    let candidate = OutputDamage::rects(
        100,
        100,
        [0, 34, 68]
            .into_iter()
            .flat_map(|y| [0, 34, 68].into_iter().map(move |x| rect(x, y, 30, 30))),
    );
    let shadow = DamageComplexityShadow::for_candidate(
        &candidate,
        (100, 100),
        Some(FullRepaintReason::TooManyRectangles),
    );

    assert!(shadow.bbox_accepted);
    assert_eq!(shadow.candidate_pixels, 98 * 98);
    assert_eq!(
        shadow.outcome,
        DamageComplexityShadowOutcome::FullAreaThreshold
    );
    assert!(!shadow.would_avoid_full);
}

#[test]
fn damage_complexity_shadow_keeps_full_area_outcome_for_rejected_bbox() {
    // The planner receives clipped, coalesced damage, so this combination is
    // not reachable for an in-output region. Exercise the outcome precedence
    // directly with synthetic metrics to keep the area safeguard explicit.
    let shadow = DamageComplexityShadow::from_metrics_for_test(
        9,
        8_000,
        Some(rect(0, 0, 200, 100)),
        20_000,
        100,
    );

    assert!(!shadow.bbox_accepted);
    assert_eq!(shadow.candidate_pixels, 8_000);
    assert_eq!(
        shadow.outcome,
        DamageComplexityShadowOutcome::FullAreaThreshold
    );
    assert!(!shadow.would_avoid_full);
}

#[test]
fn damage_complexity_shadow_requires_more_than_eight_rectangles() {
    let candidate = OutputDamage::rects(100, 100, (0..8).map(|index| rect(index * 10, 0, 5, 5)));
    let shadow = DamageComplexityShadow::for_candidate(
        &candidate,
        (100, 100),
        Some(FullRepaintReason::TooManyRectangles),
    );

    assert_eq!(candidate.rect_count(), MAX_PARTIAL_REPAINT_RECTS);
    assert_eq!(shadow.outcome, DamageComplexityShadowOutcome::NotApplicable);
}

#[test]
fn damage_complexity_shadow_accepts_exactly_two_times_area() {
    let candidate = OutputDamage::rects(
        100,
        100,
        [
            rect(0, 0, 1, 100),
            rect(2, 0, 1, 100),
            rect(4, 0, 1, 100),
            rect(6, 0, 1, 100),
            rect(8, 0, 1, 100),
            rect(10, 0, 1, 100),
            rect(12, 0, 1, 100),
            rect(14, 0, 1, 100),
            rect(18, 0, 2, 100),
        ],
    );
    let shadow = DamageComplexityShadow::for_candidate(
        &candidate,
        (100, 100),
        Some(FullRepaintReason::TooManyRectangles),
    );

    assert_eq!(candidate.rect_count(), 9);
    assert_eq!(shadow.original_pixels, 1_000);
    assert_eq!(shadow.bbox_pixels, 2_000);
    assert_eq!(
        shadow.original_pixels * DAMAGE_COMPLEXITY_SHADOW_EXTENTS_FACTOR,
        2_000
    );
    assert!(shadow.bbox_accepted);
    assert_eq!(
        shadow.outcome,
        DamageComplexityShadowOutcome::PartialBoundingBox
    );
}

#[test]
fn damage_complexity_shadow_reports_unavailable_on_pixel_overflow() {
    let candidate = OutputDamage::Rects(vec![
        rect(0, 0, u32::MAX, u32::MAX),
        rect(0, 0, u32::MAX, u32::MAX),
        rect(0, 0, u32::MAX, u32::MAX),
        rect(0, 0, u32::MAX, u32::MAX),
        rect(0, 0, u32::MAX, u32::MAX),
        rect(0, 0, u32::MAX, u32::MAX),
        rect(0, 0, u32::MAX, u32::MAX),
        rect(0, 0, u32::MAX, u32::MAX),
        rect(0, 0, u32::MAX, u32::MAX),
    ]);
    let shadow = DamageComplexityShadow::for_candidate(
        &candidate,
        (u32::MAX, u32::MAX),
        Some(FullRepaintReason::TooManyRectangles),
    );

    assert_eq!(shadow.outcome, DamageComplexityShadowOutcome::Unavailable);
    assert!(shadow.applicable);
    assert_eq!(shadow.original_rects, 9);
    assert_eq!(shadow.original_pixels, u64::MAX);
    assert_eq!(shadow.bbox, Some(rect(0, 0, u32::MAX, u32::MAX)));
    assert_eq!(
        shadow.bbox_pixels,
        u64::from(u32::MAX) * u64::from(u32::MAX)
    );
    assert!(!shadow.would_avoid_full);
}

#[test]
fn damage_complexity_shadow_reports_unavailable_when_bbox_cannot_be_represented() {
    let mut rects: Vec<_> = (0..8).map(|index| rect(index * 2, 0, 1, 1)).collect();
    rects.push(rect(i32::MAX, 0, u32::MAX, 1));
    let candidate = OutputDamage::Rects(rects);
    let shadow = DamageComplexityShadow::for_candidate(
        &candidate,
        (u32::MAX, u32::MAX),
        Some(FullRepaintReason::TooManyRectangles),
    );

    assert_eq!(shadow.outcome, DamageComplexityShadowOutcome::Unavailable);
    assert!(shadow.applicable);
    assert_eq!(shadow.bbox, None);
    assert!(!shadow.would_avoid_full);
}

#[test]
fn damage_complexity_shadow_rejects_unrepresentable_factor_arithmetic() {
    let shadow = DamageComplexityShadow::from_metrics_for_test(
        9,
        u64::MAX / DAMAGE_COMPLEXITY_SHADOW_EXTENTS_FACTOR + 1,
        Some(rect(0, 0, 10, 10)),
        100,
        u64::MAX,
    );

    assert_eq!(shadow.outcome, DamageComplexityShadowOutcome::Unavailable);
    assert!(!shadow.bbox_accepted);
    assert!(!shadow.would_avoid_full);
}

#[test]
fn damage_complexity_shadow_added_pixels_use_saturating_subtraction() {
    let shadow =
        DamageComplexityShadow::from_metrics_for_test(9, 100, Some(rect(0, 0, 9, 10)), 90, 1_000);

    assert!(shadow.bbox_accepted);
    assert_eq!(shadow.added_pixels, 0);
}

#[test]
fn damage_complexity_shadow_output_area_uses_checked_u64_dimensions() {
    assert_eq!(
        super::output_pixel_count((u32::MAX, u32::MAX)),
        Some(u64::from(u32::MAX) * u64::from(u32::MAX))
    );
}

#[test]
fn damage_complexity_shadow_does_not_change_actual_full_plan() {
    let mut planner = partial_planner((100, 100), partial_capabilities());
    planner.commit_presented_transition(OutputDamage::Empty);
    let candidate = dense_many_rect_damage();
    let expected = planner.plan(candidate.clone(), BufferAge::Value(1));
    let (actual, shadow) =
        planner.plan_with_damage_complexity_shadow(candidate, BufferAge::Value(1));

    assert_eq!(expected, actual);
    assert_eq!(actual.mode, RepaintMode::Full);
    assert_eq!(
        actual.fallback_reason,
        Some(FullRepaintReason::TooManyRectangles)
    );
    assert_eq!(actual.repair_damage, OutputDamage::Full);
    assert_eq!(
        shadow.outcome,
        DamageComplexityShadowOutcome::PartialBoundingBox
    );
    assert!(shadow.would_avoid_full);
}

#[test]
fn damage_complexity_shadow_analysis_is_skipped_by_normal_planning() {
    super::DAMAGE_COMPLEXITY_SHADOW_ANALYSIS_COUNT.with(|count| count.set(0));
    let mut planner = partial_planner((100, 100), partial_capabilities());
    planner.commit_presented_transition(OutputDamage::Empty);
    let candidate = dense_many_rect_damage();

    let actual = planner.plan(candidate, BufferAge::Value(1));

    assert_eq!(actual.mode, RepaintMode::Full);
    assert_eq!(
        actual.fallback_reason,
        Some(FullRepaintReason::TooManyRectangles)
    );
    super::DAMAGE_COMPLEXITY_SHADOW_ANALYSIS_COUNT.with(|count| assert_eq!(count.get(), 0));
}

fn graph_for_effect_region(
    id: u64,
    output_influence_region: oblivion_one::effects::EffectRegion,
) -> oblivion_one::effects::CompiledFrameGraph {
    oblivion_one::effects::CompiledFrameGraph {
        passes: Vec::new(),
        textures: Vec::new(),
        instances: vec![oblivion_one::effects::CompiledEffectInstance {
            id: oblivion_one::effects::EffectInstanceId::new(id).unwrap(),
            capture_region: output_influence_region.clone(),
            output_influence_region,
            dependencies: Vec::new(),
        }],
        final_damage: oblivion_one::effects::EffectRegion::empty(),
        stats: Default::default(),
    }
}

fn graph_for_effect_instances(
    instances: impl IntoIterator<Item = (u64, i32, i32, i32, i32, Vec<u64>)>,
) -> oblivion_one::effects::CompiledFrameGraph {
    graph_with_instance_regions(instances.into_iter().map(
        |(id, output_x, output_width, capture_x, capture_width, dependencies)| {
            (
                id,
                oblivion_one::effects::EffectRegion::from_rect(effect_rect(
                    output_x,
                    0,
                    output_width as u32,
                    10,
                )),
                oblivion_one::effects::EffectRegion::from_rect(effect_rect(
                    capture_x,
                    0,
                    capture_width as u32,
                    10,
                )),
                dependencies,
            )
        },
    ))
}

fn empty_effect_graph() -> oblivion_one::effects::CompiledFrameGraph {
    oblivion_one::effects::CompiledFrameGraph {
        passes: Vec::new(),
        textures: Vec::new(),
        instances: Vec::new(),
        final_damage: oblivion_one::effects::EffectRegion::empty(),
        stats: Default::default(),
    }
}

fn graph_with_instance_regions(
    instances: impl IntoIterator<
        Item = (
            u64,
            oblivion_one::effects::EffectRegion,
            oblivion_one::effects::EffectRegion,
            Vec<u64>,
        ),
    >,
) -> oblivion_one::effects::CompiledFrameGraph {
    let instances = instances
        .into_iter()
        .map(
            |(id, output_influence_region, capture_region, dependencies)| {
                oblivion_one::effects::CompiledEffectInstance {
                    id: oblivion_one::effects::EffectInstanceId::new(id).unwrap(),
                    output_influence_region,
                    capture_region,
                    dependencies: dependencies
                        .into_iter()
                        .map(|dependency| {
                            oblivion_one::effects::EffectInstanceId::new(dependency).unwrap()
                        })
                        .collect(),
                }
            },
        )
        .collect();
    oblivion_one::effects::CompiledFrameGraph {
        passes: Vec::new(),
        textures: Vec::new(),
        instances,
        final_damage: oblivion_one::effects::EffectRegion::empty(),
        stats: Default::default(),
    }
}

#[test]
fn output_damage_clips_all_edges_and_discards_empty_rectangles() {
    let damage = OutputDamage::rects(
        100,
        80,
        [
            rect(-5, 10, 10, 10),
            rect(95, 10, 10, 10),
            rect(10, -5, 10, 10),
            rect(10, 75, 10, 10),
            rect(0, 0, 0, 5),
        ],
    );
    assert_eq!(
        damage,
        OutputDamage::Rects(vec![
            rect(0, 10, 5, 10),
            rect(95, 10, 5, 10),
            rect(10, 0, 10, 5),
            rect(10, 75, 10, 5),
        ])
    );
}

#[test]
fn output_damage_coalesces_overlapping_and_touching_rectangles() {
    assert_eq!(
        OutputDamage::rects(
            100,
            100,
            [rect(5, 5, 10, 10), rect(15, 5, 5, 10), rect(8, 8, 4, 4)],
        ),
        OutputDamage::Rects(vec![rect(5, 5, 15, 10)])
    );
}

#[test]
fn effect_damage_stays_local_to_a_blur_panel() {
    let current = OutputDamage::rects(1920, 1080, [rect(10, 10, 10, 10)]);
    let effect = oblivion_one::effects::EffectRegion::from_rect(effect_rect(0, 0, 500, 60));
    let merged = merge_effect_damage(current, &effect, 1920, 1080);
    assert_ne!(merged, OutputDamage::Full);
    assert!(merged.pixels(1920, 1080).unwrap() < 1920 * 1080);
    assert!(merged.rects_slice().iter().any(|rect| rect.width <= 500));
}

#[test]
fn effect_damage_union_covers_removal_and_move_transitions() {
    let old = oblivion_one::effects::EffectRegion::from_rect(effect_rect(0, 0, 500, 60));
    let new = oblivion_one::effects::EffectRegion::from_rect(effect_rect(700, 0, 500, 60));
    let transition = oblivion_one::effects::effect_transition_damage(&old, &new);
    let merged = merge_effect_damage(OutputDamage::Empty, &transition, 1920, 1080);
    assert!(merged.rects_slice().iter().any(|rect| rect.x == 0));
    assert!(merged.rects_slice().iter().any(|rect| rect.x == 700));
}

#[test]
fn effect_damage_preserves_buffer_age_history() {
    let capabilities = partial_capabilities();
    let mut planner = partial_planner((1920, 1080), capabilities);
    let first = OutputDamage::rects(1920, 1080, [rect(0, 0, 500, 60)]);
    let first_plan = planner.plan(first.clone(), BufferAge::Value(1));
    assert_eq!(first_plan.mode, RepaintMode::Full);
    planner.commit_presented_transition(first);
    let effect = oblivion_one::effects::EffectRegion::from_rect(effect_rect(0, 0, 500, 60));
    let current = merge_effect_damage(
        OutputDamage::rects(1920, 1080, [rect(10, 10, 10, 10)]),
        &effect,
        1920,
        1080,
    );
    let plan = planner.plan(current, BufferAge::Value(2));
    assert_eq!(plan.mode, RepaintMode::Partial);
    assert!(plan.repair_damage.pixels(1920, 1080).unwrap() < 1920 * 1080);
}

#[test]
fn age_2_repair_revives_unchanged_effect() {
    let effect_region = oblivion_one::effects::EffectRegion::from_rect(effect_rect(10, 10, 20, 20));
    let graph = graph_for_effect_region(1, effect_region.clone());
    let mut planner = partial_planner((100, 80), partial_capabilities());

    let first = planner.plan(
        OutputDamage::rects(100, 80, [rect(10, 10, 20, 20)]),
        BufferAge::Value(0),
    );
    planner.commit_presented_transition(first.render_damage);

    let mut plan = planner.plan(
        OutputDamage::rects(100, 80, [rect(70, 60, 5, 5)]),
        BufferAge::Value(2),
    );
    let demand = resolve_effect_execution_for_repaint_plan(&planner, &graph, &mut plan, 100, 80);

    assert_eq!(plan.mode, RepaintMode::Partial);
    assert!(
        plan.render_damage
            .rects_slice()
            .contains(&rect(70, 60, 5, 5))
    );
    assert!(
        plan.repair_damage
            .rects_slice()
            .contains(&rect(10, 10, 20, 20))
    );
    assert!(demand.contains(oblivion_one::effects::EffectInstanceId::new(1).unwrap()));
}

#[test]
fn age_3_repair_revives_effect_from_two_presented_frames_ago() {
    let effect_region = oblivion_one::effects::EffectRegion::from_rect(effect_rect(10, 10, 20, 20));
    let graph = graph_for_effect_region(1, effect_region);
    let mut planner = partial_planner((100, 80), partial_capabilities());

    let first = planner.plan(
        OutputDamage::rects(100, 80, [rect(10, 10, 20, 20)]),
        BufferAge::Value(0),
    );
    planner.commit_presented_transition(first.render_damage);

    let intermediate = planner.plan(
        OutputDamage::rects(100, 80, [rect(40, 10, 5, 5)]),
        BufferAge::Value(1),
    );
    planner.commit_presented_transition(intermediate.render_damage);

    let mut plan = planner.plan(
        OutputDamage::rects(100, 80, [rect(70, 60, 5, 5)]),
        BufferAge::Value(3),
    );
    let demand = resolve_effect_execution_for_repaint_plan(&planner, &graph, &mut plan, 100, 80);

    assert_eq!(plan.mode, RepaintMode::Partial);
    assert!(
        plan.repair_damage
            .rects_slice()
            .contains(&rect(10, 10, 20, 20))
    );
    assert!(demand.contains(oblivion_one::effects::EffectInstanceId::new(1).unwrap()));
}

#[test]
fn effect_execution_repair_demand_converges_across_forward_consumers() {
    let graph = graph_for_effect_instances([
        (1, 10, 10, 10, 10, vec![]),
        (2, 30, 10, 10, 40, vec![1]),
        (3, 45, 10, 45, 25, vec![]),
        (4, 80, 10, 80, 10, vec![]),
    ]);
    let planner = partial_planner((100, 80), partial_capabilities());
    let initial_render_damage = OutputDamage::rects(100, 80, [rect(30, 0, 10, 10)]);
    let mut plan = RepaintPlan {
        render_damage: initial_render_damage.clone(),
        repair_damage: OutputDamage::rects(100, 80, [rect(30, 0, 10, 10)]),
        buffer_age: Some(2),
        mode: RepaintMode::Partial,
        fallback_reason: None,
        ..RepaintPlan::default()
    };

    let demand = resolve_effect_execution_for_repaint_plan(&planner, &graph, &mut plan, 100, 80);
    let execution_repair = merge_effect_damage(
        plan.repair_damage.clone(),
        &demand.execution_region,
        100,
        80,
    );
    planner.apply_execution_repair(&mut plan, execution_repair);

    assert!(
        plan.repair_damage
            .rects_slice()
            .iter()
            .any(|repair| repair.x <= 45 && repair.right() >= 50)
    );
    assert!(demand.contains(oblivion_one::effects::EffectInstanceId::new(1).unwrap()));
    assert!(demand.contains(oblivion_one::effects::EffectInstanceId::new(2).unwrap()));
    assert!(!demand.contains(oblivion_one::effects::EffectInstanceId::new(4).unwrap()));
    assert!(
        demand.contains(oblivion_one::effects::EffectInstanceId::new(3).unwrap()),
        "the returned demand must match the expanded final repair"
    );
    assert_eq!(plan.render_damage, initial_render_damage);
}

#[test]
fn effect_execution_repair_demand_converges_across_multiple_forward_hops() {
    let graph = graph_for_effect_instances([
        (1, 10, 10, 10, 10, vec![]),
        (2, 30, 10, 10, 40, vec![1]),
        (3, 45, 10, 45, 25, vec![2]),
        (4, 65, 10, 65, 25, vec![]),
    ]);
    let planner = partial_planner((100, 80), partial_capabilities());
    let initial_render_damage = OutputDamage::rects(100, 80, [rect(30, 0, 10, 10)]);
    let mut plan = RepaintPlan {
        render_damage: initial_render_damage.clone(),
        repair_damage: initial_render_damage.clone(),
        buffer_age: Some(2),
        mode: RepaintMode::Partial,
        fallback_reason: None,
        ..RepaintPlan::default()
    };

    let (demand, resolution) = resolve_effect_execution_for_repaint_plan_with_diagnostics(
        &planner, &graph, &mut plan, 100, 80,
    );

    assert_eq!(plan.mode, RepaintMode::Partial);
    assert_eq!(plan.fallback_reason, None);
    assert_eq!(resolution.outcome.as_str(), "converged");
    assert!(resolution.iterations_attempted >= 2);
    assert_eq!(resolution.initial_repair.pixels, 100);
    assert!(resolution.last_input_repair.pixels > resolution.initial_repair.pixels);
    assert!(resolution.last_execution_region.rects > 0);
    assert_eq!(
        resolution.last_merged_repair,
        resolution.last_applied_repair
    );
    assert_eq!(
        resolution.last_applied_repair,
        EffectExecutionRepairSnapshot::from_damage(&plan.repair_damage, (100, 80))
    );
    assert!(plan.repair_damage.rects_slice().iter().any(|repair| {
        repair.x <= 30 && repair.y <= 0 && repair.right() >= 40 && repair.bottom() >= 10
    }));
    assert!(!resolution.last_repair_changed);
    assert!(demand.contains(oblivion_one::effects::EffectInstanceId::new(1).unwrap()));
    assert!(demand.contains(oblivion_one::effects::EffectInstanceId::new(2).unwrap()));
    assert!(demand.contains(oblivion_one::effects::EffectInstanceId::new(3).unwrap()));
    assert!(demand.contains(oblivion_one::effects::EffectInstanceId::new(4).unwrap()));
    assert!(
        plan.repair_damage
            .rects_slice()
            .iter()
            .any(|repair| repair.x <= 65 && repair.right() >= 70)
    );
    assert_eq!(plan.render_damage, initial_render_damage);
}

#[test]
fn stable_effect_execution_demand_keeps_partial_repair_unchanged() {
    let graph = graph_for_effect_instances([(1, 30, 10, 30, 10, vec![])]);
    let planner = partial_planner((100, 80), partial_capabilities());
    let initial_repair = OutputDamage::rects(100, 80, [rect(30, 0, 10, 10)]);
    let mut plan = RepaintPlan {
        render_damage: OutputDamage::rects(100, 80, [rect(1, 0, 2, 2)]),
        repair_damage: initial_repair.clone(),
        buffer_age: Some(2),
        mode: RepaintMode::Partial,
        fallback_reason: None,
        ..RepaintPlan::default()
    };

    let demand = resolve_effect_execution_for_repaint_plan(&planner, &graph, &mut plan, 100, 80);

    assert_eq!(plan.mode, RepaintMode::Partial);
    assert_eq!(plan.repair_damage, initial_repair);
    assert!(demand.contains(oblivion_one::effects::EffectInstanceId::new(1).unwrap()));
}

#[test]
fn effect_execution_resolution_diagnostics_report_convergence_and_fixed_point_evidence() {
    let graph = empty_effect_graph();
    let planner = partial_planner((100, 80), partial_capabilities());
    let initial = OutputDamage::rects(100, 80, [rect(30, 0, 10, 10)]);
    let mut plan = RepaintPlan {
        render_damage: initial.clone(),
        repair_damage: initial.clone(),
        buffer_age: Some(2),
        mode: RepaintMode::Partial,
        fallback_reason: None,
        ..RepaintPlan::default()
    };

    let (demand, snapshot) = resolve_effect_execution_for_repaint_plan_with_diagnostics(
        &planner, &graph, &mut plan, 100, 80,
    );

    assert!(!demand.is_conservative_full());
    assert_eq!(snapshot.outcome.as_str(), "converged");
    assert_eq!(snapshot.demand_conservative_cause.as_str(), "none");
    assert_eq!(snapshot.iterations_attempted, 1);
    assert_eq!(snapshot.final_repaint_mode, RepaintMode::Partial);
    assert_eq!(snapshot.last_input_repair, snapshot.initial_repair);
    assert_eq!(snapshot.last_merged_repair, snapshot.last_input_repair);
    assert_eq!(snapshot.last_applied_repair, snapshot.last_input_repair);
    assert!(!snapshot.last_repair_changed);
}

#[test]
fn effect_execution_resolution_converges_when_execution_region_is_already_covered() {
    let initial_repair =
        OutputDamage::rects(8, 8, [rect(0, 0, 1, 1), rect(0, 1, 2, 1), rect(0, 2, 1, 1)]);
    let region = oblivion_one::effects::EffectRegion::from_rect(effect_rect(0, 0, 1, 1));
    let graph = graph_with_instance_regions([(1, region.clone(), region, vec![])]);
    let planner = partial_planner((8, 8), partial_capabilities());
    let mut plan = RepaintPlan {
        render_damage: initial_repair.clone(),
        repair_damage: initial_repair.clone(),
        buffer_age: Some(2),
        mode: RepaintMode::Partial,
        fallback_reason: None,
        ..RepaintPlan::default()
    };

    let (demand, snapshot) = resolve_effect_execution_for_repaint_plan_with_diagnostics(
        &planner, &graph, &mut plan, 8, 8,
    );

    let instance_id = oblivion_one::effects::EffectInstanceId::new(1).unwrap();

    assert!(demand.contains(instance_id));
    assert_eq!(snapshot.graph_instances, 1);
    assert_eq!(snapshot.dependency_edges, 0);
    assert!(snapshot.last_execution_region.rects > 0);
    assert!(snapshot.last_execution_region.pixels > 0);
    assert_eq!(snapshot.outcome.as_str(), "converged");
    assert_eq!(snapshot.iterations_attempted, 1);
    assert_eq!(plan.mode, RepaintMode::Partial);
    assert_eq!(plan.fallback_reason, None);
    assert!(!snapshot.last_repair_changed);
    assert_eq!(snapshot.last_merged_repair, snapshot.last_input_repair);
    assert_eq!(snapshot.last_applied_repair, snapshot.last_input_repair);
    assert_eq!(snapshot.final_repaint_reason, None);
    assert_eq!(plan.repair_damage, initial_repair);
}

#[test]
fn conservative_effect_execution_metadata_forces_full_repair() {
    let graph =
        graph_for_effect_instances([(1, 30, 10, 30, 10, vec![99]), (2, 80, 10, 80, 10, vec![])]);
    let planner = partial_planner((100, 80), partial_capabilities());
    let render_damage = OutputDamage::rects(100, 80, [rect(30, 0, 10, 10)]);
    let mut plan = RepaintPlan {
        render_damage: render_damage.clone(),
        repair_damage: render_damage,
        buffer_age: Some(2),
        mode: RepaintMode::Partial,
        fallback_reason: None,
        ..RepaintPlan::default()
    };

    let demand = resolve_effect_execution_for_repaint_plan(&planner, &graph, &mut plan, 100, 80);

    assert_eq!(plan.mode, RepaintMode::Full);
    assert_eq!(plan.repair_damage, OutputDamage::Full);
    assert_eq!(
        plan.fallback_reason,
        Some(FullRepaintReason::EffectExecutionConservative)
    );
    assert!(demand.is_conservative_full());
    assert!(demand.contains(oblivion_one::effects::EffectInstanceId::new(1).unwrap()));
    assert!(demand.contains(oblivion_one::effects::EffectInstanceId::new(2).unwrap()));
}

#[test]
fn effect_execution_resolution_freezes_metadata_cause_before_full_demand_recompute() {
    let graph =
        graph_for_effect_instances([(1, 30, 10, 30, 10, vec![99]), (2, 80, 10, 80, 10, vec![])]);
    let planner = partial_planner((100, 80), partial_capabilities());
    let initial = OutputDamage::rects(100, 80, [rect(30, 0, 10, 10)]);
    let mut plan = RepaintPlan {
        render_damage: initial.clone(),
        repair_damage: initial,
        buffer_age: Some(2),
        mode: RepaintMode::Partial,
        fallback_reason: None,
        ..RepaintPlan::default()
    };

    let (demand, snapshot) = resolve_effect_execution_for_repaint_plan_with_diagnostics(
        &planner, &graph, &mut plan, 100, 80,
    );

    assert!(demand.is_conservative_full());
    assert_eq!(snapshot.outcome.as_str(), "demand_conservative");
    assert_eq!(
        snapshot.demand_conservative_cause.as_str(),
        "graph_metadata_incomplete"
    );
    assert_eq!(
        snapshot
            .graph_metadata_issue
            .map(|issue| issue.kind.as_str()),
        Some("dependency_missing_or_not_unique")
    );
    assert_eq!(snapshot.final_repaint_mode, RepaintMode::Full);
    assert_eq!(
        snapshot.final_repaint_reason,
        Some(FullRepaintReason::EffectExecutionConservative)
    );
    assert_eq!(snapshot.iterations_attempted, 1);
}

#[test]
fn effect_execution_resolution_separates_repaint_policy_full_from_conservatism() {
    let graph = graph_with_instance_regions([
        (
            1,
            oblivion_one::effects::EffectRegion::from_rect(effect_rect(10, 0, 10, 10)),
            oblivion_one::effects::EffectRegion::from_rect(effect_rect(10, 0, 10, 10)),
            vec![],
        ),
        (
            2,
            oblivion_one::effects::EffectRegion::from_rect(effect_rect(30, 0, 10, 10)),
            oblivion_one::effects::EffectRegion::from_rect(effect_rect(0, 0, 80, 80)),
            vec![1],
        ),
    ]);
    let planner = partial_planner((100, 80), partial_capabilities());
    let initial = OutputDamage::rects(100, 80, [rect(30, 0, 10, 10)]);
    let mut plan = RepaintPlan {
        render_damage: initial.clone(),
        repair_damage: initial,
        buffer_age: Some(2),
        mode: RepaintMode::Partial,
        fallback_reason: None,
        ..RepaintPlan::default()
    };

    let (_demand, snapshot) = resolve_effect_execution_for_repaint_plan_with_diagnostics(
        &planner, &graph, &mut plan, 100, 80,
    );

    assert_eq!(snapshot.outcome.as_str(), "repaint_policy_full");
    assert_eq!(snapshot.demand_conservative_cause.as_str(), "none");
    assert!(snapshot.attribution_available);
    assert!(snapshot.last_selected_output_region.pixels > 0);
    assert!(snapshot.last_capture_work_region.pixels > 0);
    assert_eq!(snapshot.last_capture_work_instances, 1);
    assert_eq!(snapshot.attribution_union_coalesces, 0);
    assert_eq!(
        snapshot.last_output_only_repaint_mode,
        Some(RepaintMode::Partial)
    );
    assert_eq!(snapshot.last_output_only_repaint_reason, None);
    assert_eq!(
        snapshot.last_output_only_applied_repair.kind,
        EffectExecutionRepairKind::Rects
    );
    assert!(snapshot.initial_repair.pixels < snapshot.last_output_only_applied_repair.pixels);
    assert!(snapshot.last_output_only_applied_repair.pixels < snapshot.last_merged_repair.pixels);
    assert_eq!(
        snapshot.final_repaint_reason,
        Some(FullRepaintReason::DamageAreaThreshold)
    );
    assert_eq!(plan.mode, RepaintMode::Full);
    assert_eq!(
        plan.fallback_reason,
        Some(FullRepaintReason::DamageAreaThreshold)
    );
    assert_eq!(snapshot.last_applied_repair.kind.as_str(), "full");
    assert!(snapshot.last_merged_repair.pixels >= 6_000);
}

#[test]
fn effect_execution_resolution_freezes_attribution_before_full_demand_recompute() {
    let graph = graph_with_instance_regions([
        (
            1,
            oblivion_one::effects::EffectRegion::from_rect(effect_rect(10, 0, 10, 10)),
            oblivion_one::effects::EffectRegion::from_rect(effect_rect(10, 0, 10, 10)),
            vec![],
        ),
        (
            2,
            oblivion_one::effects::EffectRegion::from_rect(effect_rect(30, 0, 10, 10)),
            oblivion_one::effects::EffectRegion::from_rect(effect_rect(0, 0, 80, 80)),
            vec![1],
        ),
        (
            3,
            oblivion_one::effects::EffectRegion::from_rect(effect_rect(90, 0, 10, 10)),
            oblivion_one::effects::EffectRegion::empty(),
            vec![],
        ),
    ]);
    let planner = partial_planner((100, 80), partial_capabilities());
    let initial = OutputDamage::rects(100, 80, [rect(30, 0, 10, 10)]);
    let mut plan = RepaintPlan {
        render_damage: initial.clone(),
        repair_damage: initial,
        buffer_age: Some(2),
        mode: RepaintMode::Partial,
        fallback_reason: None,
        ..RepaintPlan::default()
    };

    let (demand, snapshot) = resolve_effect_execution_for_repaint_plan_with_diagnostics(
        &planner, &graph, &mut plan, 100, 80,
    );

    assert_eq!(
        snapshot.outcome,
        EffectExecutionResolutionOutcome::RepaintPolicyFull
    );
    assert_eq!(plan.mode, RepaintMode::Full);
    assert_eq!(demand.instances.len(), 3);
    assert!(snapshot.attribution_available);
    assert_eq!(snapshot.last_selected_output_region.rects, 2);
    assert_eq!(snapshot.last_selected_output_region.pixels, 200);
    assert_eq!(
        snapshot.last_output_only_repaint_mode,
        Some(RepaintMode::Partial)
    );
    assert_eq!(snapshot.last_output_only_repaint_reason, None);
}

#[test]
fn effect_execution_attribution_omits_capture_work_without_dependencies() {
    let graph = graph_for_effect_instances([(1, 30, 10, 0, 80, vec![])]);
    let planner = partial_planner((100, 80), partial_capabilities());
    let initial = OutputDamage::rects(100, 80, [rect(30, 0, 10, 10)]);
    let mut plan = RepaintPlan {
        render_damage: initial.clone(),
        repair_damage: initial,
        buffer_age: Some(2),
        mode: RepaintMode::Partial,
        fallback_reason: None,
        ..RepaintPlan::default()
    };

    let (_demand, snapshot) = resolve_effect_execution_for_repaint_plan_with_diagnostics(
        &planner, &graph, &mut plan, 100, 80,
    );

    assert!(snapshot.attribution_available);
    assert_eq!(
        snapshot.last_capture_work_region.kind,
        EffectExecutionRepairKind::Empty
    );
    assert_eq!(snapshot.last_capture_work_instances, 0);
    assert_eq!(
        snapshot.last_output_only_merged_repair,
        snapshot.last_merged_repair
    );
    assert_eq!(snapshot.last_output_only_repaint_mode, Some(plan.mode));
    assert_eq!(
        snapshot.last_output_only_repaint_reason,
        plan.fallback_reason
    );
}

#[test]
fn effect_execution_attribution_does_not_assign_cause_to_covered_capture_work() {
    let graph = graph_with_instance_regions([
        (
            1,
            oblivion_one::effects::EffectRegion::from_rect(effect_rect(4, 4, 5, 5)),
            oblivion_one::effects::EffectRegion::empty(),
            vec![],
        ),
        (
            2,
            oblivion_one::effects::EffectRegion::from_rect(effect_rect(8, 8, 5, 5)),
            oblivion_one::effects::EffectRegion::from_rect(effect_rect(10, 10, 10, 10)),
            vec![1],
        ),
    ]);
    let planner = partial_planner((100, 80), partial_capabilities());
    let initial = OutputDamage::rects(100, 80, [rect(0, 0, 20, 20)]);
    let mut plan = RepaintPlan {
        render_damage: initial.clone(),
        repair_damage: initial,
        buffer_age: Some(2),
        mode: RepaintMode::Partial,
        fallback_reason: None,
        ..RepaintPlan::default()
    };

    let (_demand, snapshot) = resolve_effect_execution_for_repaint_plan_with_diagnostics(
        &planner, &graph, &mut plan, 100, 80,
    );

    assert!(snapshot.attribution_available);
    assert_eq!(snapshot.last_capture_work_instances, 1);
    assert!(snapshot.last_capture_work_region.pixels > 0);
    assert_eq!(
        snapshot.outcome,
        EffectExecutionResolutionOutcome::Converged
    );
    assert_eq!(
        snapshot.last_output_only_merged_repair,
        snapshot.last_input_repair
    );
    assert_eq!(snapshot.last_merged_repair, snapshot.last_input_repair);
    assert_eq!(
        snapshot.last_output_only_repaint_mode,
        Some(RepaintMode::Partial)
    );
    assert_eq!(snapshot.last_output_only_repaint_reason, None);
    assert_eq!(plan.mode, RepaintMode::Partial);
    assert_eq!(plan.fallback_reason, None);
}

#[test]
fn effect_execution_resolution_reports_budget_exhaustion_without_overwriting_cause() {
    let graph = graph_for_effect_instances([
        (1, 10, 10, 10, 10, vec![]),
        (2, 30, 10, 10, 40, vec![1]),
        (3, 45, 10, 45, 25, vec![2]),
        (4, 65, 10, 65, 25, vec![]),
    ]);
    let planner = partial_planner((100, 80), partial_capabilities());
    let initial = OutputDamage::rects(100, 80, [rect(30, 0, 10, 10)]);
    let mut plan = RepaintPlan {
        render_damage: initial.clone(),
        repair_damage: initial,
        buffer_age: Some(2),
        mode: RepaintMode::Partial,
        fallback_reason: None,
        ..RepaintPlan::default()
    };

    let (_demand, snapshot) = resolve_effect_execution_for_repaint_plan_with_iteration_budget(
        &planner, &graph, &mut plan, 100, 80, 1,
    );

    assert_eq!(snapshot.outcome.as_str(), "iteration_exhausted");
    assert_eq!(snapshot.demand_conservative_cause.as_str(), "none");
    assert_eq!(snapshot.iterations_attempted, 1);
    assert_eq!(snapshot.max_iterations, 1);
    assert_eq!(
        snapshot.final_repaint_reason,
        Some(FullRepaintReason::EffectExecutionConservative)
    );
    assert!(snapshot.last_repair_changed);
    assert_eq!(
        snapshot.last_repair_changed,
        snapshot.last_applied_repair != snapshot.last_input_repair
    );
    assert_eq!(snapshot.last_merged_repair, snapshot.last_applied_repair);
    assert!(snapshot.last_merged_repair.pixels >= snapshot.last_input_repair.pixels);
}

#[test]
fn initial_full_resolution_does_not_count_a_partial_iteration() {
    let graph = empty_effect_graph();
    let planner = partial_planner((100, 80), partial_capabilities());
    let mut plan = planner.full_plan(OutputDamage::Full, Some(2), FullRepaintReason::ForcedFull);

    let (_demand, snapshot) = resolve_effect_execution_for_repaint_plan_with_diagnostics(
        &planner, &graph, &mut plan, 100, 80,
    );

    assert_eq!(snapshot.outcome.as_str(), "initial_full");
    assert_eq!(
        snapshot.demand_conservative_cause.as_str(),
        "caller_conservative_full"
    );
    assert_eq!(snapshot.iterations_attempted, 0);
    assert_eq!(
        snapshot.final_repaint_reason,
        Some(FullRepaintReason::ForcedFull)
    );
}

#[test]
fn ordinary_resolver_does_not_construct_trace_only_resolution_snapshots() {
    let graph = empty_effect_graph();
    let planner = partial_planner((100, 80), partial_capabilities());
    let initial = OutputDamage::rects(100, 80, [rect(30, 0, 10, 10)]);
    let mut plan = RepaintPlan {
        render_damage: initial.clone(),
        repair_damage: initial,
        buffer_age: Some(2),
        mode: RepaintMode::Partial,
        fallback_reason: None,
        ..RepaintPlan::default()
    };
    let before = EFFECT_EXECUTION_RESOLUTION_SNAPSHOT_BUILDS.with(Cell::get);
    let attribution_before = EFFECT_EXECUTION_ATTRIBUTION_BUILDS.with(Cell::get);

    resolve_effect_execution_for_repaint_plan(&planner, &graph, &mut plan, 100, 80);

    let after = EFFECT_EXECUTION_RESOLUTION_SNAPSHOT_BUILDS.with(Cell::get);
    let attribution_after = EFFECT_EXECUTION_ATTRIBUTION_BUILDS.with(Cell::get);
    assert_eq!(after, before);
    assert_eq!(attribution_after, attribution_before);
}

#[test]
fn threshold_full_recomputes_effect_demand_from_full_repair() {
    let graph = graph_with_instance_regions([
        (
            1,
            oblivion_one::effects::EffectRegion::from_rect(effect_rect(10, 0, 10, 10)),
            oblivion_one::effects::EffectRegion::from_rect(effect_rect(10, 0, 10, 10)),
            vec![],
        ),
        (
            2,
            oblivion_one::effects::EffectRegion::from_rect(effect_rect(30, 0, 10, 10)),
            oblivion_one::effects::EffectRegion::from_rect(effect_rect(0, 0, 80, 80)),
            vec![1],
        ),
        (
            3,
            oblivion_one::effects::EffectRegion::from_rect(effect_rect(90, 0, 10, 10)),
            oblivion_one::effects::EffectRegion::from_rect(effect_rect(90, 0, 10, 10)),
            vec![],
        ),
    ]);
    let planner = partial_planner((100, 80), partial_capabilities());
    let render_damage = OutputDamage::rects(100, 80, [rect(30, 0, 10, 10)]);
    let mut plan = RepaintPlan {
        render_damage: render_damage.clone(),
        repair_damage: render_damage,
        buffer_age: Some(2),
        mode: RepaintMode::Partial,
        fallback_reason: None,
        ..RepaintPlan::default()
    };

    let demand = resolve_effect_execution_for_repaint_plan(&planner, &graph, &mut plan, 100, 80);

    assert_eq!(plan.mode, RepaintMode::Full);
    assert_eq!(plan.repair_damage, OutputDamage::Full);
    assert_eq!(
        plan.fallback_reason,
        Some(FullRepaintReason::DamageAreaThreshold)
    );
    assert!(demand.is_conservative_full());
    assert!(demand.contains(oblivion_one::effects::EffectInstanceId::new(3).unwrap()));
    assert_eq!(
        plan.render_damage,
        OutputDamage::rects(100, 80, [rect(30, 0, 10, 10)])
    );
}

#[test]
fn full_repaint_keeps_all_visible_effects_live() {
    let effect_region = oblivion_one::effects::EffectRegion::from_rect(effect_rect(10, 10, 20, 20));
    let graph = graph_for_effect_region(1, effect_region);
    let mut planner = partial_planner((100, 80), partial_capabilities());
    let first = planner.plan(
        OutputDamage::rects(100, 80, [rect(10, 10, 20, 20)]),
        BufferAge::Value(0),
    );
    planner.commit_presented_transition(first.render_damage);

    let mut plan = planner.plan(
        OutputDamage::rects(100, 80, [rect(70, 60, 5, 5)]),
        BufferAge::Value(0),
    );
    let demand = resolve_effect_execution_for_repaint_plan(&planner, &graph, &mut plan, 100, 80);

    assert_eq!(plan.mode, RepaintMode::Full);
    assert_eq!(plan.repair_damage, OutputDamage::Full);
    assert!(demand.is_conservative_full());
    assert!(demand.contains(oblivion_one::effects::EffectInstanceId::new(1).unwrap()));
}

#[test]
fn full_repaint_reason_for_effect_conservatism_has_stable_telemetry_name() {
    assert_eq!(
        FullRepaintReason::EffectExecutionConservative.histogram_index(),
        12
    );
    assert_eq!(
        FullRepaintReason::EffectExecutionConservative.as_str(),
        "effect_execution_conservative"
    );
}

#[test]
fn output_damage_converts_top_left_rectangles_for_gl_and_egl() {
    let damage = OutputDamage::rects(100, 80, [rect(4, 7, 9, 11)]);
    assert_eq!(
        damage
            .to_gl_scissors(100, 80, OutputFramebufferOrigin::BottomLeft)
            .unwrap(),
        vec![[4, 62, 9, 11]]
    );
    assert_eq!(
        damage.to_egl_rects(100, 80).unwrap().as_slice(),
        &[4, 62, 9, 11]
    );
}

#[test]
fn output_damage_converts_one_pixel_rectangles_at_every_edge() {
    let damage = OutputDamage::rects(
        8,
        6,
        [
            rect(0, 0, 1, 1),
            rect(0, 5, 1, 1),
            rect(7, 2, 1, 1),
            rect(3, 0, 1, 1),
        ],
    );
    assert_eq!(
        damage
            .to_gl_scissors(8, 6, OutputFramebufferOrigin::BottomLeft)
            .unwrap(),
        vec![[0, 5, 1, 1], [0, 0, 1, 1], [7, 3, 1, 1], [3, 5, 1, 1]]
    );
}

#[test]
fn first_frame_and_unsupported_buffer_age_force_full_repaint() {
    let current = OutputDamage::rects(100, 80, [rect(2, 3, 4, 5)]);
    let mut planner = partial_planner((100, 80), partial_capabilities());
    assert_eq!(
        planner.plan(current.clone(), BufferAge::Value(1)).mode,
        RepaintMode::Full
    );
    let mut unsupported = partial_planner(
        (100, 80),
        EglPartialRepaintCapabilities {
            buffer_age: false,
            partial_render_repair: true,
            swap_buffers_with_damage: true,
        },
    );
    assert_eq!(
        unsupported.plan(current, BufferAge::Unsupported).mode,
        RepaintMode::Full
    );
}

#[test]
fn software_buffer_age_uses_output_presentation_serials() {
    assert_eq!(software_buffer_age(10, None), BufferAge::Value(0));
    assert_eq!(software_buffer_age(10, Some(9)), BufferAge::Value(2));
    assert_eq!(software_buffer_age(10, Some(8)), BufferAge::Value(3));
    assert_eq!(software_buffer_age(10, Some(10)), BufferAge::Value(1));
    assert_eq!(software_buffer_age(10, Some(11)), BufferAge::Value(-1));
}

#[test]
fn pending_presentation_does_not_invalidate_unrelated_slot_age() {
    assert_eq!(render_target_buffer_age(10, Some(8)), BufferAge::Value(3));
    assert_eq!(render_target_buffer_age(10, Some(8)), BufferAge::Value(3));
}

#[test]
fn explicit_render_repair_does_not_require_egl_swap_damage() {
    let first = OutputDamage::Full;
    let current = OutputDamage::rects(100, 80, [rect(20, 20, 3, 3)]);
    let mut planner = partial_planner(
        (100, 80),
        EglPartialRepaintCapabilities {
            buffer_age: true,
            partial_render_repair: true,
            swap_buffers_with_damage: false,
        },
    );
    let first_plan = planner.plan(first, BufferAge::Value(0));
    planner.commit_presented_transition(first_plan.render_damage.clone());

    assert_eq!(
        planner.plan(current, BufferAge::Value(1)).mode,
        RepaintMode::Partial
    );
}

#[test]
fn execution_repair_preserves_partial_area_threshold() {
    let planner = partial_planner((100, 80), partial_capabilities());
    let mut plan = RepaintPlan {
        render_damage: OutputDamage::rects(100, 80, [rect(1, 1, 2, 2)]),
        repair_damage: OutputDamage::rects(100, 80, [rect(1, 1, 2, 2)]),
        buffer_age: Some(2),
        mode: RepaintMode::Partial,
        fallback_reason: None,
        ..RepaintPlan::default()
    };

    planner.apply_execution_repair(
        &mut plan,
        OutputDamage::rects(100, 80, [rect(0, 0, 80, 80)]),
    );

    assert_eq!(plan.mode, RepaintMode::Full);
    assert_eq!(plan.repair_damage, OutputDamage::Full);
    assert_eq!(
        plan.fallback_reason,
        Some(FullRepaintReason::DamageAreaThreshold)
    );
}

#[test]
fn effect_execution_repair_keeps_structured_many_rectangle_plan_partial() {
    let mut planner = partial_planner_with_policy(
        (100, 100),
        partial_capabilities(),
        PartialRepaintComplexityPolicy::StructuredExperimental,
    );
    planner.commit_presented_transition(OutputDamage::Empty);
    let candidate = OutputDamage::rects(
        100,
        100,
        (0..6).flat_map(|y| (0..6).map(move |x| rect(x * 14, y * 14, 4, 4))),
    );
    let mut plan = planner.plan(candidate.clone(), BufferAge::Value(1));
    assert_eq!(plan.mode, RepaintMode::Partial);
    assert_eq!(plan.repair_damage, candidate);
    assert_eq!(plan.repair_damage.rect_count(), 36);

    let graph = empty_effect_graph();
    resolve_effect_execution_for_repaint_plan(&planner, &graph, &mut plan, 100, 100);

    assert_eq!(plan.mode, RepaintMode::Partial);
    assert_eq!(plan.repair_damage.rect_count(), 36);
    assert_eq!(
        plan.complexity_action,
        PartialRepaintComplexityAction::StructuredManyRectangles
    );
}

#[test]
fn effect_execution_repair_promotes_structured_plan_at_area_threshold() {
    let mut planner = partial_planner_with_policy(
        (100, 100),
        partial_capabilities(),
        PartialRepaintComplexityPolicy::StructuredExperimental,
    );
    planner.commit_presented_transition(OutputDamage::Empty);
    let mut plan = planner.plan(
        OutputDamage::rects(100, 100, [rect(1, 1, 2, 2)]),
        BufferAge::Value(1),
    );
    assert_eq!(plan.mode, RepaintMode::Partial);

    let full_effect_region =
        oblivion_one::effects::EffectRegion::from_rect(effect_rect(0, 0, 90, 90));
    let graph = graph_with_instance_regions([
        (
            1,
            full_effect_region.clone(),
            full_effect_region.clone(),
            vec![],
        ),
        (2, full_effect_region.clone(), full_effect_region, vec![1]),
    ]);
    resolve_effect_execution_for_repaint_plan(&planner, &graph, &mut plan, 100, 100);

    assert_eq!(plan.mode, RepaintMode::Full);
    assert_eq!(plan.repair_damage, OutputDamage::Full);
    assert_eq!(
        plan.fallback_reason,
        Some(FullRepaintReason::DamageAreaThreshold)
    );
    assert_eq!(
        plan.complexity_action,
        PartialRepaintComplexityAction::StructuredAreaFull
    );
}

#[test]
fn effect_execution_repair_promotes_structured_plan_above_safety_bound() {
    let planner = partial_planner_with_policy(
        (258, 100),
        partial_capabilities(),
        PartialRepaintComplexityPolicy::StructuredExperimental,
    );
    let initial = OutputDamage::rects(258, 100, [rect(0, 0, 1, 1)]);
    let mut plan = RepaintPlan {
        render_damage: initial.clone(),
        repair_damage: initial,
        buffer_age: Some(1),
        mode: RepaintMode::Partial,
        fallback_reason: None,
        complexity_policy: PartialRepaintComplexityPolicy::StructuredExperimental,
        complexity_action: PartialRepaintComplexityAction::NotApplicable,
    };
    let grew_beyond_bound = OutputDamage::Rects(
        (0..=oblivion_one::effects::MAX_EFFECT_REGION_RECTS)
            .map(|index| rect((index * 2) as i32, 0, 1, 1))
            .collect(),
    );

    planner.apply_execution_repair(&mut plan, grew_beyond_bound);

    assert_eq!(plan.mode, RepaintMode::Full);
    assert_eq!(plan.repair_damage, OutputDamage::Full);
    assert_eq!(
        plan.fallback_reason,
        Some(FullRepaintReason::TooManyRectangles)
    );
    assert_eq!(
        plan.complexity_action,
        PartialRepaintComplexityAction::StructuredSafetyFull
    );
}

#[test]
fn structured_policy_keeps_conservative_effect_execution_authoritative() {
    let planner = partial_planner_with_policy(
        (100, 80),
        partial_capabilities(),
        PartialRepaintComplexityPolicy::StructuredExperimental,
    );
    let render_damage = OutputDamage::rects(100, 80, [rect(30, 0, 10, 10)]);
    let mut plan = RepaintPlan {
        render_damage: render_damage.clone(),
        repair_damage: render_damage,
        buffer_age: Some(1),
        mode: RepaintMode::Partial,
        fallback_reason: None,
        complexity_policy: PartialRepaintComplexityPolicy::StructuredExperimental,
        complexity_action: PartialRepaintComplexityAction::NotApplicable,
    };
    let graph = graph_for_effect_instances([(1, 30, 10, 10, 10, vec![99])]);

    let demand = resolve_effect_execution_for_repaint_plan(&planner, &graph, &mut plan, 100, 80);

    assert!(demand.is_conservative_full());
    assert_eq!(plan.mode, RepaintMode::Full);
    assert_eq!(plan.repair_damage, OutputDamage::Full);
    assert_eq!(
        plan.fallback_reason,
        Some(FullRepaintReason::EffectExecutionConservative)
    );
}

#[test]
fn buffer_age_beyond_three_slot_history_forces_full_repaint() {
    let mut planner = partial_planner((100, 80), partial_capabilities());
    for x in [1, 10, 20] {
        let plan = planner.plan(
            OutputDamage::rects(100, 80, [rect(x, 1, 2, 2)]),
            BufferAge::Value(1),
        );
        planner.commit_presented_transition(plan.render_damage.clone());
    }

    let unsupported = planner.plan(
        OutputDamage::rects(100, 80, [rect(30, 1, 2, 2)]),
        BufferAge::Value(4),
    );

    assert_eq!(unsupported.mode, RepaintMode::Full);
    assert_eq!(
        unsupported.fallback_reason,
        Some(FullRepaintReason::InsufficientHistory)
    );
}

#[test]
fn empty_logical_damage_requires_full_repair_when_history_is_invalid() {
    let mut planner = partial_planner((100, 80), partial_capabilities());

    let plan = planner.plan(OutputDamage::Empty, BufferAge::Value(0));
    assert_eq!(plan.mode, RepaintMode::Full);
    assert_eq!(
        plan.fallback_reason,
        Some(FullRepaintReason::FirstFrameOrInvalidated)
    );
    assert_eq!(planner.history_depth(), 0);
}

#[test]
fn usable_ages_accumulate_only_required_logical_damage() {
    let first = OutputDamage::rects(100, 80, [rect(1, 1, 3, 3)]);
    let second = OutputDamage::rects(100, 80, [rect(20, 20, 3, 3)]);
    let third = OutputDamage::rects(100, 80, [rect(40, 40, 3, 3)]);
    let mut planner = partial_planner((100, 80), partial_capabilities());
    let plan = planner.plan(first, BufferAge::Value(0));
    planner.commit_presented_transition(plan.render_damage.clone());
    let plan = planner.plan(second.clone(), BufferAge::Value(1));
    assert_eq!(plan.repair_damage, second);
    planner.commit_presented_transition(plan.render_damage.clone());
    let plan = planner.plan(third, BufferAge::Value(2));
    assert_eq!(
        plan.repair_damage,
        OutputDamage::Rects(vec![rect(40, 40, 3, 3), rect(20, 20, 3, 3)])
    );
    planner.commit_presented_transition(plan.render_damage.clone());
    let fourth = OutputDamage::rects(100, 80, [rect(60, 60, 3, 3)]);
    assert_eq!(
        planner
            .plan(fourth, BufferAge::Value(3))
            .repair_damage
            .rect_count(),
        3
    );
}

#[test]
fn invalid_age_history_and_resize_force_full_repaint() {
    let current = OutputDamage::rects(100, 80, [rect(2, 3, 4, 5)]);
    let mut planner = partial_planner((100, 80), partial_capabilities());
    let first = planner.plan(OutputDamage::Full, BufferAge::Value(0));
    planner.commit_presented_transition(first.render_damage.clone());
    assert_eq!(
        planner.plan(current.clone(), BufferAge::Value(0)).mode,
        RepaintMode::Full
    );
    assert_eq!(
        planner.plan(current.clone(), BufferAge::Value(9)).mode,
        RepaintMode::Full
    );
    planner.resize((120, 80));
    assert_eq!(
        planner.plan(current, BufferAge::Value(1)).mode,
        RepaintMode::Full
    );
}

#[test]
fn failed_swap_does_not_advance_history_and_empty_stays_empty() {
    let current = OutputDamage::rects(100, 80, [rect(2, 3, 4, 5)]);
    let mut planner = partial_planner((100, 80), partial_capabilities());
    let first = planner.plan(OutputDamage::Full, BufferAge::Value(0));
    planner.commit_presented_transition(first.render_damage.clone());
    let failed = planner.plan(current, BufferAge::Value(1));
    planner.swap_failed();
    assert_eq!(planner.history_depth(), 0);
    assert_eq!(
        planner
            .plan(OutputDamage::Empty, BufferAge::Value(1))
            .render_damage,
        OutputDamage::Empty
    );
    assert_eq!(failed.render_damage.rect_count(), 1);
}

#[test]
fn rendered_candidate_does_not_advance_history_until_matching_commit() {
    let mut planner = partial_planner((100, 80), partial_capabilities());
    let candidate = planner.plan(OutputDamage::Full, BufferAge::Value(0));

    assert_eq!(planner.history_depth(), 0);
    planner.commit_presented_transition(candidate.render_damage.clone());
    assert_eq!(planner.history_depth(), 1);
}

#[test]
fn discarded_rendered_candidate_does_not_advance_or_invalidate_history() {
    let mut planner = partial_planner((100, 80), partial_capabilities());
    let presented = planner.plan(
        OutputDamage::rects(100, 80, [rect(1, 2, 2, 2)]),
        BufferAge::Value(0),
    );
    planner.commit_presented_transition(presented.render_damage.clone());
    let discarded = planner.plan(
        OutputDamage::rects(100, 80, [rect(4, 5, 6, 7)]),
        BufferAge::Value(2),
    );

    planner.discard_rendered(&discarded);

    assert_eq!(planner.history_depth(), 1);
    assert_eq!(
        planner
            .plan(
                OutputDamage::rects(100, 80, [rect(20, 21, 2, 2)]),
                BufferAge::Value(2),
            )
            .repair_damage
            .rect_count(),
        2
    );
}

#[test]
fn render_ahead_keeps_presented_history_until_pageflip_and_repairs_all_changes() {
    let first_damage = OutputDamage::rects(100, 80, [rect(1, 2, 2, 2)]);
    let render_ahead_damage = OutputDamage::rects(100, 80, [rect(20, 21, 2, 2)]);
    let current_damage = OutputDamage::rects(100, 80, [rect(20, 21, 2, 2), rect(40, 41, 2, 2)]);
    let mut planner = partial_planner((100, 80), partial_capabilities());

    let presented = planner.plan(first_damage.clone(), BufferAge::Value(0));
    planner.commit_presented_transition(presented.render_damage.clone());

    // B is rendered while A is presented, but its damage is not presented
    // history until B's pageflip is confirmed.
    let render_ahead = planner.plan(render_ahead_damage, BufferAge::Value(1));
    assert_eq!(planner.history_depth(), 1);
    assert_eq!(render_ahead.mode, RepaintMode::Partial);

    // C's scene damage includes the B change because B never became the
    // confirmed scene. C's slot is older, so repair also needs A's history.
    let render_ahead_ready = planner.plan(current_damage, BufferAge::Value(2));
    assert_eq!(render_ahead_ready.mode, RepaintMode::Partial);
    for required in first_damage
        .rects_slice()
        .iter()
        .chain(render_ahead_ready.render_damage.rects_slice())
    {
        assert!(
            render_ahead_ready
                .repair_damage
                .rects_slice()
                .contains(required),
            "partial repair omitted required visual difference {required:?}"
        );
    }
    planner.commit_presented_transition(render_ahead_ready.render_damage.clone());
    assert_eq!(planner.history_depth(), 2);
}

#[test]
fn presentation_journal_uses_actual_predecessor_after_render_ahead() {
    let a_to_b = OutputDamage::rects(100, 80, [rect(10, 10, 4, 4)]);
    let b_to_c = OutputDamage::rects(100, 80, [rect(40, 10, 4, 4)]);
    let a_to_c = OutputDamage::rects(100, 80, [rect(70, 10, 4, 4)]);
    let mut planner = partial_planner((100, 80), partial_capabilities());

    let a = planner.plan(OutputDamage::Full, BufferAge::Value(0));
    planner.commit_presented_transition(a.render_damage.clone());

    let b = planner.plan(a_to_b, BufferAge::Value(1));
    let _c = planner.plan(a_to_c, BufferAge::Value(2));

    // B is confirmed before C even though C was rendered while A was still
    // presented. The journal is presentation-domain state, so C's entry must
    // be the transition from the actually presented B scene to C.
    planner.commit_presented_transition(b.render_damage.clone());
    planner.commit_presented_transition(b_to_c.clone());

    assert_eq!(planner.history.front(), Some(&b_to_c));
}

#[test]
fn presentation_domain_journal_clears_b_only_pixels_from_reused_slot() {
    let output_size = (12, 1);
    let mut planner = partial_planner(output_size, partial_capabilities());
    let mut slots = [vec![0_u8; 12], vec![0_u8; 12], vec![0_u8; 12]];
    let mut last_presented_serial = [None::<u64>; 3];
    let mut presentation_serial = 0_u64;

    let apply_plan =
        |slot: &mut [u8], reference: &[u8], plan: &RepaintPlan| match &plan.repair_damage {
            OutputDamage::Empty => {}
            OutputDamage::Full => slot.copy_from_slice(reference),
            OutputDamage::Rects(rects) => {
                for damage in rects {
                    let start = usize::try_from(damage.x.max(0)).unwrap();
                    let end = start
                        .saturating_add(damage.width as usize)
                        .min(reference.len());
                    slot[start..end].copy_from_slice(&reference[start..end]);
                }
            }
        };

    let present_baseline = |planner: &mut PartialRepaintPlanner,
                            slot: &mut [u8],
                            last_serial: &mut Option<u64>,
                            serial: &mut u64,
                            transition: OutputDamage| {
        let plan = planner.plan(OutputDamage::Full, BufferAge::Value(0));
        apply_plan(slot, &[0; 12], &plan);
        assert_eq!(slot, &[0; 12]);
        *serial += 1;
        *last_serial = Some(*serial);
        planner.commit_presented_transition(transition);
    };

    // Warm all three slots with a known A scene so the later age=2 repair is
    // a genuine reused-slot path, not first-use full repaint fallback.
    present_baseline(
        &mut planner,
        &mut slots[0],
        &mut last_presented_serial[0],
        &mut presentation_serial,
        OutputDamage::Full,
    );
    present_baseline(
        &mut planner,
        &mut slots[1],
        &mut last_presented_serial[1],
        &mut presentation_serial,
        OutputDamage::Empty,
    );
    present_baseline(
        &mut planner,
        &mut slots[2],
        &mut last_presented_serial[2],
        &mut presentation_serial,
        OutputDamage::Empty,
    );

    let mut b = vec![0_u8; 12];
    b[4] = 1; // B-only titlebar/button pixel.
    let a_to_b = OutputDamage::rects(12, 1, [rect(4, 0, 1, 1)]);
    let b_plan = planner.plan(a_to_b.clone(), BufferAge::Value(3));
    apply_plan(&mut slots[0], &b, &b_plan);
    assert_eq!(slots[0], b);

    let mut c = b.clone();
    c[4] = 0; // B-only pixel must be erased by B→C.
    c[7] = 2; // C-only content.
    let a_to_c = OutputDamage::rects(12, 1, [rect(7, 0, 1, 1)]);
    let c_plan = planner.plan(a_to_c, BufferAge::Value(2));
    apply_plan(&mut slots[1], &c, &c_plan);
    assert_eq!(slots[1], c);

    // The actual presentation order is A→B→C, even though C was repaired
    // while A remained presented. The two transitions, rather than C's
    // render-time A→C damage, now define the presentation journal.
    presentation_serial += 1;
    last_presented_serial[0] = Some(presentation_serial);
    planner.commit_presented_transition(a_to_b);
    presentation_serial += 1;
    last_presented_serial[1] = Some(presentation_serial);
    let b_to_c = OutputDamage::rects(12, 1, [rect(4, 0, 1, 1), rect(7, 0, 1, 1)]);
    planner.commit_presented_transition(b_to_c.clone());

    let mut d = c.clone();
    d[8] = 3;
    let c_to_d = OutputDamage::rects(12, 1, [rect(8, 0, 1, 1)]);
    let age = presentation_serial
        .saturating_sub(last_presented_serial[0].unwrap())
        .saturating_add(1);
    assert_eq!(age, 2);
    let d_plan = planner.plan(c_to_d, BufferAge::Value(age as i32));
    assert!(
        d_plan
            .repair_damage
            .rects_slice()
            .contains(&rect(4, 0, 1, 1))
    );
    apply_plan(&mut slots[0], &d, &d_plan);

    assert_eq!(slots[0], d);
    assert_eq!(
        slots[0][4], 0,
        "B-only pixel resurfaced from the reused slot"
    );
}

#[test]
fn presentation_domain_journal_clears_b_only_resize_edge_from_reused_slot() {
    let output_size = (64, 1);
    let mut planner = partial_planner(output_size, partial_capabilities());
    let mut slots = [vec![0_u8; 64], vec![0_u8; 64], vec![0_u8; 64]];
    let mut last_serial = [None::<u64>; 3];
    let mut serial = 0_u64;
    let apply = |slot: &mut [u8], reference: &[u8], plan: &RepaintPlan| match &plan.repair_damage {
        OutputDamage::Empty => {}
        OutputDamage::Full => slot.copy_from_slice(reference),
        OutputDamage::Rects(rects) => {
            for rect in rects {
                let start = usize::try_from(rect.x.max(0)).unwrap();
                let end = start
                    .saturating_add(rect.width as usize)
                    .min(reference.len());
                slot[start..end].copy_from_slice(&reference[start..end]);
            }
        }
    };
    for slot_index in 0..3 {
        let plan = planner.plan(OutputDamage::Full, BufferAge::Value(0));
        apply(&mut slots[slot_index], &[0; 64], &plan);
        serial += 1;
        last_serial[slot_index] = Some(serial);
        planner.commit_presented_transition(if slot_index == 0 {
            OutputDamage::Full
        } else {
            OutputDamage::Empty
        });
    }

    // A=1200px, B=1750px, C=1300px in the real geometry test. These byte
    // ranges model the B-only right titlebar edge and resize-exposed frame.
    let mut b = vec![0_u8; 64];
    for pixel in &mut b[20..36] {
        *pixel = 1;
    }
    let b_damage = OutputDamage::rects(64, 1, [rect(20, 0, 16, 1)]);
    let b_plan = planner.plan(b_damage.clone(), BufferAge::Value(3));
    apply(&mut slots[0], &b, &b_plan);

    let mut c = vec![0_u8; 64];
    for pixel in &mut c[20..32] {
        *pixel = 2;
    }
    let a_to_c = OutputDamage::rects(64, 1, [rect(20, 0, 12, 1)]);
    let c_plan = planner.plan(a_to_c, BufferAge::Value(2));
    apply(&mut slots[1], &c, &c_plan);
    assert_eq!(slots[1], c);

    serial += 1;
    last_serial[0] = Some(serial);
    planner.commit_presented_transition(b_damage);
    serial += 1;
    last_serial[1] = Some(serial);
    let b_to_c = OutputDamage::rects(64, 1, [rect(20, 0, 16, 1)]);
    planner.commit_presented_transition(b_to_c);

    let mut d = c.clone();
    d[40] = 3;
    let d_plan = planner.plan(
        OutputDamage::rects(64, 1, [rect(40, 0, 1, 1)]),
        BufferAge::Value(
            serial
                .saturating_sub(last_serial[0].unwrap())
                .saturating_add(1) as i32,
        ),
    );
    apply(&mut slots[0], &d, &d_plan);

    assert_eq!(slots[0], d);
    assert!(slots[0][32..36].iter().all(|pixel| *pixel == 0));
}

#[test]
fn two_rendered_candidates_can_coexist_before_one_is_committed() {
    let mut planner = partial_planner((100, 80), partial_capabilities());
    let first = planner.plan(OutputDamage::Full, BufferAge::Value(0));
    let second = planner.plan(
        OutputDamage::rects(100, 80, [rect(8, 9, 3, 3)]),
        BufferAge::Value(0),
    );

    assert_eq!(planner.history_depth(), 0);
    planner.discard_rendered(&first);
    planner.commit_presented_transition(second.render_damage.clone());
    assert_eq!(planner.history_depth(), 1);
}

#[test]
fn policy_falls_back_for_many_rectangles_or_near_full_area() {
    let mut planner = partial_planner((100, 100), partial_capabilities());
    let first = planner.plan(OutputDamage::Full, BufferAge::Value(0));
    planner.commit_presented_transition(first.render_damage.clone());
    let many = OutputDamage::Rects(
        (0..=MAX_PARTIAL_REPAINT_RECTS)
            .map(|index| rect((index * 3) as i32, 1, 1, 1))
            .collect(),
    );
    assert_eq!(
        planner.plan(many, BufferAge::Value(1)).mode,
        RepaintMode::Full
    );
    let near_full = OutputDamage::rects(100, 100, [rect(0, 0, 90, 90)]);
    assert_eq!(
        planner.plan(near_full, BufferAge::Value(1)).mode,
        RepaintMode::Full
    );
}

#[test]
fn partial_repaint_can_be_disabled_by_force_full_policy() {
    let mut planner = PartialRepaintPlanner::new((100, 80), partial_capabilities());
    planner.partial_enabled = false;
    let first = planner.plan(OutputDamage::Full, BufferAge::Value(0));
    planner.commit_presented_transition(first.render_damage.clone());

    let plan = planner.plan(
        OutputDamage::rects(100, 80, [rect(4, 7, 9, 11)]),
        BufferAge::Value(1),
    );

    assert_eq!(plan.mode, RepaintMode::Full);
}

#[test]
fn render_execution_plan_clears_each_partial_scissor_and_restores_state() {
    let plan = RepaintPlan {
        render_damage: OutputDamage::rects(100, 80, [rect(4, 7, 9, 11)]),
        repair_damage: OutputDamage::rects(100, 80, [rect(4, 7, 9, 11), rect(30, 40, 5, 6)]),
        buffer_age: Some(2),
        mode: RepaintMode::Partial,
        fallback_reason: None,
        ..RepaintPlan::default()
    };

    assert_eq!(
        plan.render_execution(100, 80, OutputFramebufferOrigin::BottomLeft)
            .unwrap(),
        RenderExecution::Scissored {
            scissors: vec![[4, 62, 9, 11], [30, 34, 5, 6]],
            disable_scissor_after: true,
        }
    );
    assert_eq!(plan.swap_damage(), &plan.repair_damage);
}

#[test]
fn skipped_plan_has_no_gl_execution() {
    let plan = RepaintPlan {
        render_damage: OutputDamage::Empty,
        repair_damage: OutputDamage::Empty,
        buffer_age: Some(1),
        mode: RepaintMode::Skip,
        fallback_reason: None,
        ..RepaintPlan::default()
    };

    assert_eq!(
        plan.render_execution(100, 80, OutputFramebufferOrigin::BottomLeft),
        None
    );
}

#[test]
fn successful_swap_records_logical_damage_instead_of_expanded_repair() {
    let mut planner = partial_planner((100, 80), partial_capabilities());
    let initial = planner.plan(
        OutputDamage::rects(100, 80, [rect(1, 1, 2, 2)]),
        BufferAge::Value(0),
    );
    planner.commit_presented_transition(initial.render_damage.clone());
    let second = planner.plan(
        OutputDamage::rects(100, 80, [rect(20, 20, 2, 2)]),
        BufferAge::Value(2),
    );
    planner.commit_presented_transition(second.render_damage.clone());

    let third = planner.plan(
        OutputDamage::rects(100, 80, [rect(40, 40, 2, 2)]),
        BufferAge::Value(2),
    );
    assert_eq!(third.repair_damage.rect_count(), 2);
    assert!(
        !third
            .repair_damage
            .rects_slice()
            .contains(&rect(1, 1, 2, 2))
    );
}

#[test]
fn full_damage_conversion_and_checked_area_are_explicit() {
    assert_eq!(
        OutputDamage::Full
            .to_gl_scissors(8, 6, OutputFramebufferOrigin::BottomLeft)
            .unwrap(),
        vec![[0, 0, 8, 6]]
    );
    let overflowing = OutputDamage::Rects(vec![
        rect(0, 0, u32::MAX, u32::MAX),
        rect(0, 0, u32::MAX, u32::MAX),
    ]);
    assert_eq!(overflowing.pixels(u32::MAX, u32::MAX), None);
}

#[test]
fn full_current_damage_wins_and_surface_invalidation_forces_full() {
    let mut planner = partial_planner((100, 80), partial_capabilities());
    let first = planner.plan(OutputDamage::Full, BufferAge::Value(0));
    planner.commit_presented_transition(first.render_damage.clone());
    assert_eq!(
        planner.plan(OutputDamage::Full, BufferAge::Value(1)).mode,
        RepaintMode::Full
    );
    planner.invalidate();
    let partial = OutputDamage::rects(100, 80, [rect(1, 2, 3, 4)]);
    assert_eq!(
        planner.plan(partial, BufferAge::Value(1)).fallback_reason,
        Some(FullRepaintReason::FirstFrameOrInvalidated)
    );
}

#[test]
fn invalidated_history_cannot_skip_an_empty_return_to_composition() {
    let mut planner = partial_planner((100, 80), partial_capabilities());
    planner.invalidate();

    let plan = planner.plan(OutputDamage::Empty, BufferAge::Value(3));

    assert_eq!(plan.mode, RepaintMode::Full);
    assert_eq!(
        plan.fallback_reason,
        Some(FullRepaintReason::FirstFrameOrInvalidated)
    );
}

#[test]
fn histories_are_isolated_per_planner_surface() {
    let mut first = partial_planner((100, 80), partial_capabilities());
    let second = partial_planner((100, 80), partial_capabilities());
    let plan = first.plan(OutputDamage::Full, BufferAge::Value(0));
    first.commit_presented_transition(plan.render_damage.clone());

    assert_eq!(first.history_depth(), 1);
    assert_eq!(second.history_depth(), 0);
}

#[test]
fn triple_buffer_swapchain_oracle_matches_full_reference() {
    let mut planner = partial_planner((12, 1), partial_capabilities());
    let mut buffers = [vec![0u8; 12], vec![0u8; 12], vec![0u8; 12]];
    let mut last_presented = [None::<u32>; 3];
    let serial = std::cell::Cell::new(0u32);
    let mut observed_ages = Vec::new();

    let mut present = |planner: &mut PartialRepaintPlanner,
                       buffer_index: usize,
                       reference: &[u8],
                       logical: OutputDamage,
                       fail_swap: bool| {
        let age = last_presented[buffer_index]
            .map(|last| serial.get().saturating_sub(last).saturating_add(1))
            .unwrap_or(0);
        observed_ages.push(age);
        let plan = planner.plan(logical, BufferAge::Value(age as i32));
        assert_ne!(plan.mode, RepaintMode::Skip);
        match &plan.repair_damage {
            OutputDamage::Empty => panic!("rendered oracle plan cannot be empty"),
            OutputDamage::Full => buffers[buffer_index].copy_from_slice(reference),
            OutputDamage::Rects(rects) => {
                for rect in rects {
                    let start = usize::try_from(rect.x.max(0)).unwrap();
                    let end = start
                        .saturating_add(rect.width as usize)
                        .min(reference.len());
                    buffers[buffer_index][start..end].copy_from_slice(&reference[start..end]);
                }
            }
        }
        if fail_swap {
            planner.swap_failed();
            return;
        }
        assert_eq!(buffers[buffer_index], reference);
        serial.set(serial.get().saturating_add(1));
        last_presented[buffer_index] = Some(serial.get());
        planner.commit_presented_transition(plan.render_damage.clone());
    };

    let mut reference = vec![0u8; 12];
    reference[1] = 1;
    present(&mut planner, 0, &reference, OutputDamage::Full, false);

    reference[4] = 2;
    present(
        &mut planner,
        1,
        &reference,
        OutputDamage::rects(12, 1, [rect(4, 0, 1, 1)]),
        false,
    );

    reference[7] = 3;
    present(
        &mut planner,
        2,
        &reference,
        OutputDamage::rects(12, 1, [rect(7, 0, 1, 1)]),
        false,
    );

    reference[8] = 4;
    present(
        &mut planner,
        2,
        &reference,
        OutputDamage::rects(12, 1, [rect(8, 0, 1, 1)]),
        false,
    );

    let serial_before_skip = serial.get();
    let skipped = planner.plan(OutputDamage::Empty, BufferAge::Value(3));
    assert_eq!(skipped.mode, RepaintMode::Skip);
    assert_eq!(serial.get(), serial_before_skip);

    reference[1] = 0;
    reference[2] = 5;
    present(
        &mut planner,
        1,
        &reference,
        OutputDamage::rects(12, 1, [rect(1, 0, 2, 1)]),
        false,
    );

    reference[10] = 6;
    present(
        &mut planner,
        2,
        &reference,
        OutputDamage::rects(12, 1, [rect(10, 0, 1, 1)]),
        true,
    );
    assert_eq!(serial.get(), serial_before_skip + 1);
    present(
        &mut planner,
        2,
        &reference,
        OutputDamage::rects(12, 1, [rect(10, 0, 1, 1)]),
        false,
    );

    planner.resize((16, 1));
    let resized_reference = vec![9u8; 16];
    let resized = planner.plan(OutputDamage::Full, BufferAge::Value(0));
    assert_eq!(resized.mode, RepaintMode::Full);
    let mut resized_buffer = vec![0u8; 16];
    resized_buffer.copy_from_slice(&resized_reference);
    assert_eq!(resized_buffer, resized_reference);
    planner.commit_presented_transition(resized.render_damage.clone());
    assert!(observed_ages.contains(&1));
    assert!(observed_ages.contains(&2));
    assert!(observed_ages.contains(&3));
}

#[test]
fn logical_top_damage_maps_to_origin_specific_gl_rows() {
    let damage = OutputDamage::rects(100, 80, [rect(4, 0, 9, 11)]);

    assert_eq!(
        damage
            .to_gl_scissors(
                100,
                80,
                crate::egl_renderer::OutputFramebufferOrigin::BottomLeft
            )
            .unwrap(),
        vec![[4, 69, 9, 11]]
    );
    assert_eq!(
        damage
            .to_gl_scissors(
                100,
                80,
                crate::egl_renderer::OutputFramebufferOrigin::TopLeftScanout,
            )
            .unwrap(),
        vec![[4, 0, 9, 11]]
    );
}

#[test]
fn logical_bottom_damage_maps_to_origin_specific_gl_rows() {
    let damage = OutputDamage::rects(100, 80, [rect(4, 69, 9, 11)]);

    assert_eq!(
        damage
            .to_gl_scissors(
                100,
                80,
                crate::egl_renderer::OutputFramebufferOrigin::BottomLeft
            )
            .unwrap(),
        vec![[4, 0, 9, 11]]
    );
    assert_eq!(
        damage
            .to_gl_scissors(
                100,
                80,
                crate::egl_renderer::OutputFramebufferOrigin::TopLeftScanout,
            )
            .unwrap(),
        vec![[4, 69, 9, 11]]
    );
}

#[test]
fn partial_render_execution_uses_scanout_damage_rows() {
    let plan = RepaintPlan {
        render_damage: OutputDamage::rects(100, 80, [rect(4, 0, 9, 11)]),
        repair_damage: OutputDamage::rects(100, 80, [rect(4, 0, 9, 11)]),
        buffer_age: Some(2),
        mode: RepaintMode::Partial,
        fallback_reason: None,
        ..RepaintPlan::default()
    };

    assert_eq!(
        plan.render_execution(
            100,
            80,
            crate::egl_renderer::OutputFramebufferOrigin::TopLeftScanout,
        )
        .unwrap(),
        RenderExecution::Scissored {
            scissors: vec![[4, 0, 9, 11]],
            disable_scissor_after: true,
        }
    );
}
