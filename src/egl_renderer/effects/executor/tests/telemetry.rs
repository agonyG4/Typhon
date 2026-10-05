use super::*;

#[test]
fn direct_framebuffer_capture_is_intentionally_conservative() {
    let instance = oblivion_one::effects::EffectInstanceId::new(1).unwrap();
    let output = GraphTextureId::new(1).unwrap();
    let domain = oblivion_one::effects::EffectRect::new(20, 30, 100, 80).unwrap();
    let pass = test_pass(
        1,
        RenderPassKind::SceneCapture,
        instance,
        Vec::new(),
        output,
        vec![GraphPassId::new(9).unwrap()],
    );
    let graph = CompiledFrameGraph {
        passes: vec![pass.clone()],
        textures: vec![test_texture(1, GraphTextureSource::CapturedScene, domain)],
        instances: vec![oblivion_one::effects::CompiledEffectInstance {
            semantic_signature: 0,
            frame_demand: oblivion_one::effects::EffectFrameDemand::OnDamage,
            id: instance,
            output_influence_region: EffectRegion::from_rect(domain),
            capture_region: EffectRegion::from_rect(domain),
            dependencies: Vec::new(),
        }],
        final_damage: EffectRegion::empty(),
        stats: Default::default(),
    };
    let demanded =
        EffectRegion::from_rect(oblivion_one::effects::EffectRect::new(40, 45, 8, 6).unwrap());
    let demand = planned_demand(instance, demanded.clone(), vec![(pass.id, demanded)]);

    assert_eq!(
        capture_execution_damage(
            &graph,
            &demand,
            &pass,
            SceneBaselineAuthority::ReplayRequired,
            *effect_debug_config(),
        ),
        EffectRegion::from_rect(domain)
    );
}

#[test]
fn replay_capture_clear_plan_covers_only_demanded_target_rectangles() {
    let target = test_texture(
        1,
        GraphTextureSource::CapturedScene,
        oblivion_one::effects::EffectRect::new(100, 50, 100, 100).unwrap(),
    );
    let damage =
        EffectRegion::from_rect(oblivion_one::effects::EffectRect::new(110, 60, 20, 20).unwrap());

    assert_eq!(
        capture_clear_rects(&damage, &target),
        vec![OutputRect::new(10, 70, 20, 20)]
    );
}

#[test]
fn capture_execution_pixels_are_physical_demanded_area() {
    let target = test_texture(
        1,
        GraphTextureSource::CapturedScene,
        oblivion_one::effects::EffectRect::new(0, 0, 100, 100).unwrap(),
    );
    let damage =
        EffectRegion::from_rect(oblivion_one::effects::EffectRect::new(10, 20, 30, 40).unwrap());
    let rects = capture_clear_rects(&damage, &target);

    assert_eq!(output_rect_pixels(&rects), 1200);
    assert_ne!(output_rect_pixels(&rects), 10000);
}

#[test]
fn capture_execution_pixels_do_not_double_count_duplicate_demand() {
    let target = test_texture(
        1,
        GraphTextureSource::CapturedScene,
        oblivion_one::effects::EffectRect::new(0, 0, 100, 100).unwrap(),
    );
    let rect = oblivion_one::effects::EffectRect::new(10, 20, 30, 40).unwrap();
    let demand = EffectRegion::from_rect(rect).union(&EffectRegion::from_rect(rect));
    let prepared = demand.disjoint_bounded();
    let rects = capture_clear_rects(&prepared.region, &target);

    assert_eq!(demand.rects().len(), 1);
    assert_eq!(output_rect_pixels(&rects), 1200);
}

#[test]
fn capture_execution_summary_uses_materialized_pixels_and_replay_indices() {
    let instance = oblivion_one::effects::EffectInstanceId::new(1).unwrap();
    let output = GraphTextureId::new(1).unwrap();
    let replay_pass = test_pass(
        1,
        RenderPassKind::SceneCapture,
        instance,
        Vec::new(),
        output,
        Vec::new(),
    );
    let layers = [
        capture::CaptureLayer::Other,
        capture::CaptureLayer::Surface(10),
        capture::CaptureLayer::Surface(20),
    ];
    let indices = capture::indices_for_capture(
        &layers,
        &[None, None, None],
        oblivion_one::compositor::EffectAnchor::BeforeSurface(20),
        false,
        None,
        oblivion_one::compositor::EffectAnchorScope::Surface,
    );
    let replay_rects = [OutputRect::new(10, 20, 30, 40)];
    let replay_pixels = output_rect_pixels(&replay_rects);
    let mut stats = EffectExecutionStats::default();
    stats.record_capture_execution(&replay_pass, false, replay_pixels, indices.len());

    let direct_pass = test_pass(
        2,
        RenderPassKind::SurfaceCapture,
        instance,
        Vec::new(),
        output,
        vec![GraphPassId::new(1).unwrap()],
    );
    let direct_rects = [full_output_rect((100, 80))];
    stats.record_capture_execution(&direct_pass, true, output_rect_pixels(&direct_rects), 0);

    assert_eq!(stats.replay_capture_execution_pixels, replay_pixels);
    assert_eq!(stats.replay_capture_commands, indices.len());
    assert_eq!(stats.framebuffer_capture_execution_pixels, 8_000);
    assert_eq!(stats.checkpoint_capture_execution_pixels, 8_000);
    assert_eq!(stats.capture_execution_pixels, replay_pixels + 8_000);
}

#[test]
fn replay_capture_detail_distinguishes_candidate_planning_from_scene_scans() {
    let mut stats = EffectExecutionStats::default();
    stats.record_replay_capture_detail(ReplayCaptureExecutionDetail {
        materialization_rects: 2,
        execution_regions: 2,
        candidate_commands: 3,
        command_region_pairs: 5,
        scene_commands_total: 10,
        scene_scan_pairs: 20,
        planner_commands_visited: 5,
        planner_commands_drawable: 4,
        commands_considered: 20,
        commands_executed: 3,
        draw_calls: 3,
        ..Default::default()
    });

    let summary = stats.capture_timing_summary();

    assert_eq!(summary.replay_capture_materialization_rects, 2);
    assert_eq!(summary.replay_capture_execution_regions, 2);
    assert_eq!(summary.replay_capture_command_region_pairs, 5);
    assert_eq!(summary.replay_capture_scene_scan_pairs, 20);
    assert_eq!(summary.replay_capture_planner_commands_visited, 5);
    assert_eq!(summary.replay_capture_planner_commands_drawable, 4);
    assert_eq!(summary.replay_capture_commands_executed, 3);
    assert_eq!(summary.replay_capture_draw_calls, 3);
    assert_ne!(summary.replay_capture_command_region_pairs, 3 * 2);
    assert_eq!(stats.replay_capture_commands, 0);
}

#[test]
fn replay_capture_detail_keeps_host_phases_synthetic_and_fixed_size() {
    let mut stats = EffectExecutionStats::default();
    stats.record_replay_capture_detail(ReplayCaptureExecutionDetail {
        host_cpu_ns: 101,
        selection_cpu_ns: 11,
        visibility_cpu_ns: 22,
        draw_submit_cpu_ns: 33,
        commands_executed: 1,
        draw_calls: 1,
        ..Default::default()
    });

    let summary = stats.capture_timing_summary();

    assert_eq!(summary.replay_capture_host_cpu_ns, 101);
    assert_eq!(summary.replay_capture_selection_cpu_ns, 11);
    assert_eq!(summary.replay_capture_visibility_cpu_ns, 22);
    assert_eq!(summary.replay_capture_draw_submit_cpu_ns, 33);
}

#[test]
fn checkpoint_cache_timing_uses_bounded_frame_aggregates() {
    let admission = CheckpointCacheAdmissionStats {
        candidates_total: 9,
        candidates_considered: 9,
        resident_candidates: 7,
        skipped_budget: 2,
        skipped_hard_budget: 1,
        skipped_checkpoint_budget: 1,
        skipped_budget_bytes: 6_291_456,
        skipped_hard_budget_bytes: 3_145_728,
        skipped_checkpoint_budget_bytes: 3_145_728,
        graph_peak_known: true,
        graph_peak_bytes: 12_000_000,
        base_checked_out_bytes: 31_285_016,
        hard_budget_bytes: 128 * 1024 * 1024,
        checkpoint_cache_soft_budget_bytes: 64 * 1024 * 1024,
        checkpoint_cache_bytes_at_admission: 60_000_000,
        budget_bytes: 128 * 1024 * 1024,
        additional_budget_needed_for_all_candidates_bytes: 6_291_456,
        additional_budget_needed_known: true,
        additional_checkpoint_budget_needed_for_all_candidates_bytes: 5_000_000,
        additional_checkpoint_budget_needed_known: true,
        ..Default::default()
    };
    let stats = EffectExecutionStats {
        checkpoint_cache_hits: 4,
        checkpoint_cache_full_refreshes: 1,
        checkpoint_cache_zero_copy_hits: 2,
        checkpoint_cache_update_pixels: 120,
        checkpoint_cache_domain_pixels: 2_000,
        checkpoint_cache_saved_pixels: 1_880,
        checkpoint_cache_entries: 5,
        checkpoint_cache_bytes: 80_000,
        checkpoint_cache_admission: admission,
        ..Default::default()
    };

    let summary = stats.capture_timing_summary();

    assert_eq!(summary.checkpoint_cache_hits, 4);
    assert_eq!(summary.checkpoint_cache_full_refreshes, 1);
    assert_eq!(summary.checkpoint_cache_zero_copy_hits, 2);
    assert_eq!(summary.checkpoint_cache_update_pixels, 120);
    assert_eq!(summary.checkpoint_cache_domain_pixels, 2_000);
    assert_eq!(summary.checkpoint_cache_saved_pixels, 1_880);
    assert_eq!(summary.checkpoint_cache_entries, 5);
    assert_eq!(summary.checkpoint_cache_bytes, 80_000);
    assert_eq!(summary.checkpoint_cache_admission, admission);
}

#[test]
fn fused_checkpoint_reports_zero_physical_capture_pixels_in_timing_summary() {
    let mut capture = test_pass(
        1,
        RenderPassKind::SceneCapture,
        oblivion_one::effects::EffectInstanceId::new(1).unwrap(),
        Vec::new(),
        GraphTextureId::new(2).unwrap(),
        Vec::new(),
    );
    capture
        .checkpoint_dependencies
        .push(GraphPassId::new(3).unwrap());
    let mut stats = EffectExecutionStats::default();
    stats.record_capture_execution_with_mode(
        &capture,
        CaptureTimingMode::FramebufferShaderCopy,
        0,
        0,
    );
    stats.capture_downsample_fusion_candidates = 1;
    stats.capture_downsample_fusion_executed = 1;
    stats.capture_downsample_fusion_elided_capture_pixels = 1_073_600;
    stats.capture_downsample_fusion_output_pixels = 240_000;

    let summary = stats.capture_timing_summary();

    assert_eq!(summary.framebuffer_shader_copy_capture_execution_pixels, 0);
    assert_eq!(summary.checkpoint_capture_execution_pixels, 0);
    assert_eq!(summary.capture_downsample_fusion_candidates, 1);
    assert_eq!(summary.capture_downsample_fusion_executed, 1);
    assert_eq!(
        summary.capture_downsample_fusion_elided_capture_pixels,
        1_073_600
    );
    assert_eq!(summary.capture_downsample_fusion_output_pixels, 240_000);
}

#[test]
fn replay_detail_is_aggregated_once_after_pass_timing_finalization() {
    use std::cell::RefCell;

    let mut stats = EffectExecutionStats::default();
    let detail = ReplayCaptureExecutionDetail {
        execution_regions: 2,
        commands_executed: 3,
        ..Default::default()
    };
    let events = RefCell::new(Vec::new());

    finalize_pass_timing_and_replay_detail(
        || events.borrow_mut().push("gpu_end"),
        Some(detail),
        |detail| {
            events.borrow_mut().push("aggregate");
            stats.record_replay_capture_detail(detail);
        },
    );

    assert_eq!(*events.borrow(), ["gpu_end", "aggregate"]);
    assert_eq!(stats.replay_capture_execution_regions, 2);
    assert_eq!(stats.replay_capture_commands_executed, 3);
}

#[test]
fn composite_scene_replay_finalization_builds_detail_after_gpu_end() {
    use std::cell::RefCell;

    let events = RefCell::new(Vec::new());

    events.borrow_mut().push("draw_returns");
    let detail = finalize_composite_scene_replay_timing(
        true,
        || {
            events.borrow_mut().push("gpu_end");
            true
        },
        || {
            events.borrow_mut().push("detail_build");
            CompositeSceneReplayExecutionDetail::default()
        },
    );
    assert!(detail.is_some());
    events.borrow_mut().push("detail_attach");

    assert_eq!(
        *events.borrow(),
        ["draw_returns", "gpu_end", "detail_build", "detail_attach"]
    );
}

#[test]
fn composite_scene_replay_finalization_skips_profiler_work_without_span() {
    let detail = finalize_composite_scene_replay_timing(
        false,
        || panic!("disabled profiler must not issue GPU END"),
        || panic!("disabled profiler must not build execution detail"),
    );
    assert_eq!(detail, None);
}

#[test]
fn composite_scene_replay_eligibility_matches_contract() {
    let cases = [
        (
            "valid Composite replay",
            RenderPassKind::Composite,
            true,
            10,
            20,
            true,
            false,
            true,
        ),
        (
            "non-Composite pass",
            RenderPassKind::OutputPostProcess,
            true,
            10,
            20,
            true,
            false,
            false,
        ),
        (
            "no graph timing scope",
            RenderPassKind::Composite,
            false,
            10,
            20,
            true,
            false,
            false,
        ),
        (
            "empty command range",
            RenderPassKind::Composite,
            true,
            10,
            10,
            true,
            false,
            false,
        ),
        (
            "reversed command range",
            RenderPassKind::Composite,
            true,
            10,
            9,
            true,
            false,
            false,
        ),
        (
            "empty active work",
            RenderPassKind::Composite,
            true,
            10,
            20,
            false,
            false,
            false,
        ),
        (
            "capture in progress",
            RenderPassKind::Composite,
            true,
            10,
            20,
            true,
            true,
            false,
        ),
    ];

    for (
        name,
        pass_kind,
        graph_timing_active,
        scene_cursor,
        draw_end,
        has_active_work,
        capture_in_progress,
        expected,
    ) in cases
    {
        assert_eq!(
            should_time_composite_scene_replay(
                pass_kind,
                graph_timing_active,
                scene_cursor,
                draw_end,
                has_active_work,
                capture_in_progress,
            ),
            expected,
            "{name}"
        );
    }
}

#[test]
fn composite_scene_replay_execution_detail_uses_saturating_frame_stat_deltas() {
    let mut before = GlesSceneFrameStats::default();
    before.commands_considered = 10;
    before.commands_executed = 8;
    before.draw_calls = 7;
    before.texture_binds = 6;
    before.scene_vbo_uploads = 5;
    before.scene_vbo_upload_bytes = 4_000;
    let mut after = before;
    after.commands_considered = 15;
    after.commands_executed = 9;
    after.draw_calls = 10;
    after.texture_binds = 12;
    after.scene_vbo_uploads = 6;
    after.scene_vbo_upload_bytes = 5_024;

    let detail = composite_scene_replay_execution_detail(before, after, 123);

    assert_eq!(detail.host_cpu_ns, 123);
    assert_eq!(detail.commands_considered, 5);
    assert_eq!(detail.commands_executed, 1);
    assert_eq!(detail.draw_calls, 3);
    assert_eq!(detail.texture_binds, 6);
    assert_eq!(detail.scene_vbo_uploads, 1);
    assert_eq!(detail.scene_vbo_upload_bytes, 1_024);
}

#[test]
fn replay_failure_does_not_aggregate_detail() {
    use std::cell::RefCell;

    let mut stats = EffectExecutionStats::default();
    let events = RefCell::new(Vec::new());

    finalize_pass_timing_and_replay_detail(
        || events.borrow_mut().push("gpu_end"),
        None,
        |detail| {
            events.borrow_mut().push("aggregate");
            stats.record_replay_capture_detail(detail);
        },
    );

    assert_eq!(*events.borrow(), ["gpu_end"]);
    assert_eq!(stats, EffectExecutionStats::default());
}

#[test]
fn replay_host_timing_gate_is_disabled_without_active_gpu_profiling() {
    assert!(!replay_capture_host_timing_enabled(false, false));
    assert!(!replay_capture_host_timing_enabled(true, true));
    assert!(replay_capture_host_timing_enabled(true, false));
}

#[test]
fn effect_surface_consumer_plan_keeps_capture_only_source() {
    let instance = oblivion_one::effects::EffectInstanceId::new(1).unwrap();
    let pass_id = GraphPassId::new(1).unwrap();
    let output = GraphTextureId::new(1).unwrap();
    let full = oblivion_one::effects::EffectRect::new(0, 0, 100, 100).unwrap();
    let pass = CompiledRenderPass {
        id: pass_id,
        kind: RenderPassKind::SceneCapture,
        inputs: Vec::new(),
        output: Some(output),
        damage: EffectRegion::empty(),
        instance,
        anchor: oblivion_one::compositor::EffectAnchor::BeforeSurface(2),
        blur_radius: None,
        stage: None,
        fused_stages: Vec::new(),
        parameter_block: oblivion_one::effects::EffectParameterBlock::default(),
        alpha_mode: oblivion_one::effects::EffectAlphaMode::Preserve,
        encode_output: false,
        color_conversion: EffectColorConversion::None,
        checkpoint_dependencies: vec![pass_id],
        visual_group: None,
        anchor_scope: oblivion_one::compositor::EffectAnchorScope::Surface,
        visible_clip_fallback: None,
    };
    let graph = CompiledFrameGraph {
        passes: vec![pass],
        textures: vec![oblivion_one::effects::GraphTexturePlan {
            id: output,
            source: GraphTextureSource::Intermediate,
            width: 100,
            height: 100,
            domain: full,
            working_space: oblivion_one::effects::EffectWorkingSpace::LinearSrgb,
            origin: oblivion_one::effects::GraphTextureOrigin::BottomLeft,
            first_use: None,
            last_use: None,
        }],
        instances: vec![oblivion_one::effects::CompiledEffectInstance {
            semantic_signature: 0,
            frame_demand: oblivion_one::effects::EffectFrameDemand::OnDamage,
            id: instance,
            output_influence_region: EffectRegion::from_rect(full),
            capture_region: EffectRegion::from_rect(full),
            dependencies: Vec::new(),
        }],
        final_damage: EffectRegion::empty(),
        stats: Default::default(),
    };
    let demand = EffectExecutionDemand::new(
        vec![oblivion_one::effects::EffectInstanceExecutionDemand {
            id: instance,
            presentation_output_region: EffectRegion::from_rect(full),
            output_region: EffectRegion::from_rect(full),
        }],
        EffectRegion::from_rect(full),
    );
    let selection = select_effect_execution(&graph, &demand);
    let command = |layer| EglDrawCommand {
        layer,
        visual_group: None,
        bounds: EglRect::new(0.0, 0.0, 100.0, 100.0),
        opaque_regions: Vec::new(),
        presentation_clip: None,
        vertex_start: 0,
        vertex_count: 6,
        sampling: SurfaceSampling::ExactNearest,
    };
    let commands = vec![
        command(EglDrawLayer::Surface(1)),
        EglDrawCommand {
            opaque_regions: vec![EglRect::new(0.0, 0.0, 100.0, 100.0)],
            ..command(EglDrawLayer::Surface(2))
        },
    ];

    let plan = plan_effect_surface_consumers(
        &graph,
        &demand,
        &selection,
        &commands,
        &[OutputRect::new(0, 0, 100, 100)],
        (100, 100),
    );

    assert!(plan.surface_ids().contains(&1));
}

#[test]
fn effect_surface_consumer_plan_uses_precise_capture_demand() {
    let instance = oblivion_one::effects::EffectInstanceId::new(1).unwrap();
    let output = GraphTextureId::new(1).unwrap();
    let domain = oblivion_one::effects::EffectRect::new(0, 0, 100, 100).unwrap();
    let pass = test_pass(
        1,
        RenderPassKind::SceneCapture,
        instance,
        Vec::new(),
        output,
        Vec::new(),
    );
    let graph = CompiledFrameGraph {
        passes: vec![pass.clone()],
        textures: vec![test_texture(1, GraphTextureSource::CapturedScene, domain)],
        instances: vec![oblivion_one::effects::CompiledEffectInstance {
            semantic_signature: 0,
            frame_demand: oblivion_one::effects::EffectFrameDemand::OnDamage,
            id: instance,
            output_influence_region: EffectRegion::from_rect(domain),
            capture_region: EffectRegion::from_rect(domain),
            dependencies: Vec::new(),
        }],
        final_damage: EffectRegion::empty(),
        stats: Default::default(),
    };
    let demanded =
        EffectRegion::from_rect(oblivion_one::effects::EffectRect::new(0, 0, 20, 20).unwrap());
    let demand = planned_demand(instance, demanded.clone(), vec![(pass.id, demanded)]);
    let selection = select_effect_execution(&graph, &demand);
    let command = |layer, x| EglDrawCommand {
        layer,
        visual_group: None,
        bounds: EglRect::new(x, 0.0, 20.0, 20.0),
        opaque_regions: Vec::new(),
        presentation_clip: None,
        vertex_start: 0,
        vertex_count: 6,
        sampling: SurfaceSampling::ExactNearest,
    };
    let commands = vec![
        command(EglDrawLayer::Surface(1), 0.0),
        command(EglDrawLayer::Surface(2), 60.0),
    ];

    let plan = plan_effect_surface_consumers(
        &graph,
        &demand,
        &selection,
        &commands,
        &[OutputRect::new(0, 0, 20, 20)],
        (100, 100),
    );

    assert!(plan.surface_ids().contains(&1));
    assert!(!plan.surface_ids().contains(&2));
}

#[test]
fn effect_surface_consumer_plan_framebuffer_scene_work_includes_upper_surface() {
    let instance = oblivion_one::effects::EffectInstanceId::new(1).unwrap();
    let output = GraphTextureId::new(1).unwrap();
    let capture_domain = oblivion_one::effects::EffectRect::new(0, 0, 100, 100).unwrap();
    let mut pass = test_pass(
        1,
        RenderPassKind::SceneCapture,
        instance,
        Vec::new(),
        output,
        Vec::new(),
    );
    pass.anchor = oblivion_one::compositor::EffectAnchor::BeforeSurface(2);
    pass.anchor_scope = oblivion_one::compositor::EffectAnchorScope::Surface;
    let graph = CompiledFrameGraph {
        passes: vec![pass.clone()],
        textures: vec![test_texture(
            1,
            GraphTextureSource::CapturedScene,
            capture_domain,
        )],
        instances: vec![oblivion_one::effects::CompiledEffectInstance {
            semantic_signature: 0,
            frame_demand: oblivion_one::effects::EffectFrameDemand::OnDamage,
            id: instance,
            output_influence_region: EffectRegion::from_rect(capture_domain),
            capture_region: EffectRegion::from_rect(capture_domain),
            dependencies: Vec::new(),
        }],
        final_damage: EffectRegion::empty(),
        stats: Default::default(),
    };
    let demand = EffectExecutionDemand::new(
        vec![oblivion_one::effects::EffectInstanceExecutionDemand {
            id: instance,
            presentation_output_region: EffectRegion::from_rect(capture_domain),
            output_region: EffectRegion::from_rect(capture_domain),
        }],
        EffectRegion::from_rect(capture_domain),
    );
    let selection = select_effect_execution(&graph, &demand);
    let command = |layer, x, width| EglDrawCommand {
        layer,
        visual_group: None,
        bounds: EglRect::new(x, 0.0, width, 20.0),
        opaque_regions: Vec::new(),
        presentation_clip: None,
        vertex_start: 0,
        vertex_count: 6,
        sampling: SurfaceSampling::ExactNearest,
    };
    let commands = vec![
        command(EglDrawLayer::Surface(1), 0.0, 20.0),
        command(EglDrawLayer::Surface(2), 60.0, 20.0),
    ];
    let config = super::super::super::trace::EffectDebugConfig::new(
        super::super::super::trace::EffectDebugCaptureMode::Framebuffer,
        super::super::super::trace::EffectDebugKawaseMode::Partial,
    );

    let plan = plan_effect_surface_consumers_with_debug_config(
        &graph,
        &demand,
        &selection,
        &commands,
        &[OutputRect::new(0, 0, 4, 4)],
        (100, 100),
        config,
    );

    assert!(plan.surface_ids().contains(&1));
    assert!(plan.surface_ids().contains(&2));
}

#[test]
fn effect_surface_consumer_plan_replay_checkpoint_uses_checkpoint_scene_work() {
    let instance = oblivion_one::effects::EffectInstanceId::new(1).unwrap();
    let output = GraphTextureId::new(1).unwrap();
    let capture_domain = oblivion_one::effects::EffectRect::new(0, 0, 100, 100).unwrap();
    let pass = test_pass(
        1,
        RenderPassKind::SceneCapture,
        instance,
        Vec::new(),
        output,
        vec![GraphPassId::new(9).unwrap()],
    );
    let graph = CompiledFrameGraph {
        passes: vec![pass.clone()],
        textures: vec![test_texture(
            1,
            GraphTextureSource::CapturedScene,
            capture_domain,
        )],
        instances: vec![oblivion_one::effects::CompiledEffectInstance {
            semantic_signature: 0,
            frame_demand: oblivion_one::effects::EffectFrameDemand::OnDamage,
            id: instance,
            output_influence_region: EffectRegion::from_rect(capture_domain),
            capture_region: EffectRegion::from_rect(capture_domain),
            dependencies: Vec::new(),
        }],
        final_damage: EffectRegion::empty(),
        stats: Default::default(),
    };
    let demand = EffectExecutionDemand::new(
        vec![oblivion_one::effects::EffectInstanceExecutionDemand {
            id: instance,
            presentation_output_region: EffectRegion::from_rect(capture_domain),
            output_region: EffectRegion::from_rect(capture_domain),
        }],
        EffectRegion::from_rect(capture_domain),
    );
    let selection = select_effect_execution(&graph, &demand);
    let command = |layer, x| EglDrawCommand {
        layer,
        visual_group: None,
        bounds: EglRect::new(x, 0.0, 20.0, 20.0),
        opaque_regions: Vec::new(),
        presentation_clip: None,
        vertex_start: 0,
        vertex_count: 6,
        sampling: SurfaceSampling::ExactNearest,
    };
    let commands = vec![
        command(EglDrawLayer::Surface(1), 0.0),
        command(EglDrawLayer::Surface(2), 60.0),
    ];
    let plan = plan_effect_surface_consumers_with_debug_config(
        &graph,
        &demand,
        &selection,
        &commands,
        &[OutputRect::new(0, 0, 4, 4)],
        (100, 100),
        EffectDebugConfig::new(
            EffectDebugCaptureMode::Replay,
            EffectDebugKawaseMode::Partial,
        ),
    );

    assert!(plan.surface_ids().contains(&1));
    assert!(plan.surface_ids().contains(&2));
}

#[test]
fn effect_selection_follows_direct_presentation_and_graph_dependencies() {
    use crate::egl_renderer::damage::{
        EglPartialRepaintCapabilities, OutputDamage, PartialRepaintPlanner, RepaintMode,
        RepaintPlan, resolve_effect_execution_for_repaint_plan,
    };

    let first = oblivion_one::effects::EffectInstanceId::new(1).unwrap();
    let second = oblivion_one::effects::EffectInstanceId::new(2).unwrap();
    let third = oblivion_one::effects::EffectInstanceId::new(3).unwrap();
    let unrelated = oblivion_one::effects::EffectInstanceId::new(4).unwrap();
    let pass = |id, instance, input, output| CompiledRenderPass {
        id: GraphPassId::new(id).unwrap(),
        kind: RenderPassKind::Fragment,
        inputs: vec![input],
        output: Some(output),
        damage: EffectRegion::empty(),
        instance,
        anchor: oblivion_one::compositor::EffectAnchor::OutputPostProcess,
        blur_radius: None,
        stage: None,
        fused_stages: Vec::new(),
        parameter_block: oblivion_one::effects::EffectParameterBlock::default(),
        alpha_mode: oblivion_one::effects::EffectAlphaMode::Preserve,
        encode_output: false,
        color_conversion: EffectColorConversion::None,
        checkpoint_dependencies: Vec::new(),
        visual_group: None,
        anchor_scope: oblivion_one::compositor::EffectAnchorScope::VisualGroup,
        visible_clip_fallback: None,
    };
    let texture = |id| oblivion_one::effects::GraphTexturePlan {
        id: GraphTextureId::new(id).unwrap(),
        source: GraphTextureSource::Intermediate,
        width: 10,
        height: 10,
        domain: oblivion_one::effects::EffectRect::new(0, 0, 10, 10).unwrap(),
        working_space: oblivion_one::effects::EffectWorkingSpace::LinearSrgb,
        origin: oblivion_one::effects::GraphTextureOrigin::BottomLeft,
        first_use: None,
        last_use: None,
    };
    let instance = |id, output_x, capture_x, capture_width, dependencies| {
        oblivion_one::effects::CompiledEffectInstance {
            semantic_signature: 0,
            frame_demand: oblivion_one::effects::EffectFrameDemand::OnDamage,
            id,
            output_influence_region: EffectRegion::from_rect(
                oblivion_one::effects::EffectRect::new(output_x, 0, 10, 10).unwrap(),
            ),
            capture_region: EffectRegion::from_rect(
                oblivion_one::effects::EffectRect::new(capture_x, 0, capture_width, 10).unwrap(),
            ),
            dependencies,
        }
    };
    let graph = CompiledFrameGraph {
        passes: vec![
            pass(
                1,
                first,
                GraphTextureId::new(1).unwrap(),
                GraphTextureId::new(2).unwrap(),
            ),
            pass(
                2,
                second,
                GraphTextureId::new(3).unwrap(),
                GraphTextureId::new(4).unwrap(),
            ),
            pass(
                3,
                third,
                GraphTextureId::new(5).unwrap(),
                GraphTextureId::new(6).unwrap(),
            ),
            pass(
                4,
                unrelated,
                GraphTextureId::new(7).unwrap(),
                GraphTextureId::new(8).unwrap(),
            ),
        ],
        textures: (1..=8).map(texture).collect(),
        instances: vec![
            instance(first, 10, 10, 10, Vec::new()),
            instance(second, 30, 10, 40, vec![first]),
            instance(third, 45, 45, 25, vec![second]),
            instance(unrelated, 80, 80, 10, Vec::new()),
        ],
        final_damage: EffectRegion::empty(),
        stats: Default::default(),
    };
    let planner = PartialRepaintPlanner::new(
        (100, 80),
        EglPartialRepaintCapabilities {
            buffer_age: true,
            partial_render_repair: true,
            swap_buffers_with_damage: true,
        },
    );
    let initial_damage = OutputDamage::rects(100, 80, [OutputRect::new(30, 0, 10, 10)]);
    let mut repaint_plan = RepaintPlan {
        render_damage: initial_damage.clone(),
        repair_damage: initial_damage.clone(),
        buffer_age: Some(2),
        mode: RepaintMode::Partial,
        fallback_reason: None,
        ..RepaintPlan::default()
    };
    let demand =
        resolve_effect_execution_for_repaint_plan(&planner, &graph, &mut repaint_plan, 100, 80);

    let dependency_demand = demand
        .instances
        .iter()
        .find(|instance| instance.id == first)
        .expect("required earlier dependency demand");
    let presented_demand = demand
        .instances
        .iter()
        .find(|instance| instance.id == second)
        .expect("directly presented effect demand");
    assert!(dependency_demand.presentation_output_region.is_empty());
    assert!(!dependency_demand.output_region.is_empty());
    assert!(!presented_demand.presentation_output_region.is_empty());

    let selection = select_effect_execution(&graph, &demand);

    assert_eq!(selection.executed_instances, vec![first, second]);
    assert_eq!(
        selection.executed_passes,
        vec![GraphPassId::new(1).unwrap(), GraphPassId::new(2).unwrap(),]
    );
    assert!(!selection.executed_instances.contains(&unrelated));
    assert_eq!(repaint_plan.mode, RepaintMode::Partial);
    assert_eq!(repaint_plan.repair_damage, initial_damage);
}

#[test]
fn structured_repaint_36_and_60_rects_reach_scene_replay_state() {
    use crate::egl_renderer::damage::{
        BufferAge, EglPartialRepaintCapabilities, OutputDamage, PartialRepaintComplexityPolicy,
        PartialRepaintPlanner, RepaintMode,
    };

    let output_size = (400, 300);
    for rect_count in [36, 60] {
        let candidate = OutputDamage::rects(
            output_size.0,
            output_size.1,
            (0..rect_count).map(|index| {
                let x = (index % 10) * 30;
                let y = (index / 10) * 30;
                OutputRect::new(x as i32, y as i32, 2, 2)
            }),
        );
        assert_eq!(candidate.rect_count(), rect_count);
        let mut planner = PartialRepaintPlanner::new_with_policy(
            output_size,
            EglPartialRepaintCapabilities {
                buffer_age: true,
                partial_render_repair: true,
                swap_buffers_with_damage: true,
            },
            PartialRepaintComplexityPolicy::StructuredExperimental,
        );
        planner.commit_presented_transition(OutputDamage::Empty);
        let plan = planner.plan(candidate, BufferAge::Value(1));
        assert_eq!(plan.mode, RepaintMode::Partial);
        assert_eq!(plan.repair_damage.rect_count(), rect_count);

        let presentation =
            crate::egl_renderer::repaint_plan_output_rects(&plan, output_size.0, output_size.1);
        assert_eq!(presentation.len(), rect_count);
        assert_ne!(presentation, vec![full_output_rect(output_size)]);

        let work_plan = SceneReplayWorkPlan::new(presentation.clone(), Vec::new(), output_size);
        assert_eq!(work_plan.baseline_work, presentation);
        let work_state = SceneReplayWorkState::new(&work_plan, SceneReplayWorkMode::SuffixDemand);
        assert_eq!(work_state.active_work().len(), rect_count);
        assert_eq!(work_state.normalization_overflow_fallbacks, 0);
    }
}

#[test]
fn scene_replay_normalization_counts_fragmentation_overflow_fallbacks() {
    use crate::egl_renderer::damage::{
        BufferAge, EglPartialRepaintCapabilities, OutputDamage, PartialRepaintComplexityAction,
        PartialRepaintComplexityPolicy, PartialRepaintPlanner, RepaintMode,
    };

    let output_size = (386, 2);
    let candidate = OutputDamage::rects(
        output_size.0,
        output_size.1,
        (0..MAX_EFFECT_REGION_RECTS).map(|index| OutputRect::new((index * 3) as i32, 0, 1, 1)),
    );
    let mut planner = PartialRepaintPlanner::new_with_policy(
        output_size,
        EglPartialRepaintCapabilities {
            buffer_age: true,
            partial_render_repair: true,
            swap_buffers_with_damage: true,
        },
        PartialRepaintComplexityPolicy::StructuredExperimental,
    );
    planner.commit_presented_transition(OutputDamage::Empty);
    let plan = planner.plan(candidate, BufferAge::Value(1));
    assert_eq!(plan.mode, RepaintMode::Partial);
    assert_eq!(plan.repair_damage.rect_count(), MAX_EFFECT_REGION_RECTS);
    assert_eq!(
        plan.complexity_action,
        PartialRepaintComplexityAction::StructuredManyRectangles
    );

    let presentation =
        crate::egl_renderer::repaint_plan_output_rects(&plan, output_size.0, output_size.1);
    let requirement = test_checkpoint_requirement(1, &[OutputRect::new(384, 0, 1, 1)]);
    let work_plan = SceneReplayWorkPlan::new(presentation, vec![requirement], output_size);
    assert_eq!(work_plan.baseline_work, vec![full_output_rect(output_size)]);
    assert_eq!(work_plan.normalization_overflow_fallbacks, 1);

    let work_state = SceneReplayWorkState::new(&work_plan, SceneReplayWorkMode::SuffixDemand);
    assert_eq!(work_state.active_work(), &[full_output_rect(output_size)]);
    assert_eq!(work_state.normalization_overflow_fallbacks, 2);
}

#[test]
fn scene_replay_work_one_checkpoint_expires_after_capture() {
    let presentation = OutputRect::new(0, 0, 10, 10);
    let checkpoint = OutputRect::new(20, 0, 10, 10);
    let requirement = test_checkpoint_requirement(1, &[checkpoint]);
    let plan = test_scene_replay_plan(&[presentation], &[requirement]);
    let mut state = SceneReplayWorkState::new(&plan, SceneReplayWorkMode::SuffixDemand);

    assert_same_output_region(state.active_work(), &[presentation, checkpoint]);
    state.mark_capture_satisfied(GraphPassId::new(1).unwrap());
    assert_same_output_region(state.active_work(), &[presentation]);
    assert_eq!(state.pending_checkpoint_requirements(), 0);
}

#[test]
fn scene_replay_work_two_checkpoints_expires_as_a_suffix() {
    let presentation = OutputRect::new(0, 0, 10, 10);
    let checkpoint_a = OutputRect::new(20, 0, 10, 10);
    let checkpoint_b = OutputRect::new(40, 0, 10, 10);
    let requirements = [
        test_checkpoint_requirement(1, &[checkpoint_a]),
        test_checkpoint_requirement(2, &[checkpoint_b]),
    ];
    let plan = test_scene_replay_plan(&[presentation], &requirements);
    let mut state = SceneReplayWorkState::new(&plan, SceneReplayWorkMode::SuffixDemand);

    assert_eq!(output_work_pixels(state.active_work()), 300);
    state.mark_capture_satisfied(GraphPassId::new(1).unwrap());
    assert_same_output_region(state.active_work(), &[presentation, checkpoint_b]);
    assert_eq!(output_work_pixels(state.active_work()), 200);
    state.mark_capture_satisfied(GraphPassId::new(2).unwrap());
    assert_same_output_region(state.active_work(), &[presentation]);
    assert_eq!(output_work_pixels(state.active_work()), 100);
}

#[test]
fn scene_replay_work_overlapping_checkpoints_keep_pending_overlap() {
    let checkpoint_a = OutputRect::new(0, 0, 20, 20);
    let checkpoint_b = OutputRect::new(10, 0, 20, 20);
    let requirements = [
        test_checkpoint_requirement(1, &[checkpoint_a]),
        test_checkpoint_requirement(2, &[checkpoint_b]),
    ];
    let plan = test_scene_replay_plan(&[], &requirements);
    let mut state = SceneReplayWorkState::new(&plan, SceneReplayWorkMode::SuffixDemand);

    state.mark_capture_satisfied(GraphPassId::new(1).unwrap());
    assert_same_output_region(state.active_work(), &[checkpoint_b]);
    assert!(state.active_work().iter().any(|rect| {
        rect.x <= 10
            && rect.y <= 0
            && rect.x + rect.width as i32 >= 30
            && rect.y + rect.height as i32 >= 20
    }));
}

#[test]
fn scene_replay_work_same_anchor_uses_pass_lifetime_not_cursor_position() {
    let checkpoint_a = OutputRect::new(0, 0, 10, 10);
    let checkpoint_b = OutputRect::new(20, 0, 10, 10);
    let requirements = [
        test_checkpoint_requirement(10, &[checkpoint_a]),
        test_checkpoint_requirement(11, &[checkpoint_b]),
    ];
    let plan = test_scene_replay_plan(&[], &requirements);
    let mut state = SceneReplayWorkState::new(&plan, SceneReplayWorkMode::SuffixDemand);

    let scene_cursor = 10;
    state.mark_capture_satisfied(GraphPassId::new(10).unwrap());
    assert_eq!(scene_cursor, 10);
    assert_eq!(state.pending_checkpoint_requirements(), 1);
    assert_same_output_region(state.active_work(), &[checkpoint_b]);

    state.mark_capture_satisfied(GraphPassId::new(11).unwrap());
    assert_eq!(state.pending_checkpoint_requirements(), 0);
    assert!(state.active_work().is_empty());
}

#[test]
fn scene_replay_work_presentation_overlap_creates_no_duplicate_fragment() {
    let presentation = OutputRect::new(0, 0, 20, 20);
    let requirement = test_checkpoint_requirement(1, &[OutputRect::new(5, 5, 5, 5)]);
    let plan = test_scene_replay_plan(&[presentation], &[requirement]);
    let mut state = SceneReplayWorkState::new(&plan, SceneReplayWorkMode::SuffixDemand);

    assert_eq!(state.active_work(), &[presentation]);
    state.mark_capture_satisfied(GraphPassId::new(1).unwrap());
    assert_eq!(state.active_work(), &[presentation]);
}

#[test]
fn scene_replay_work_without_checkpoints_is_presentation_only() {
    let presentation = OutputRect::new(12, 14, 18, 16);
    let plan = test_scene_replay_plan(&[presentation], &[]);
    let state = SceneReplayWorkState::new(&plan, SceneReplayWorkMode::SuffixDemand);

    assert_eq!(state.active_work(), &[presentation]);
    assert_eq!(state.pending_checkpoint_requirements(), 0);
}

#[test]
fn scene_replay_work_global_baseline_is_stable_after_capture() {
    let presentation = OutputRect::new(0, 0, 10, 10);
    let checkpoint_a = OutputRect::new(20, 0, 10, 10);
    let checkpoint_b = OutputRect::new(40, 0, 10, 10);
    let requirements = [
        test_checkpoint_requirement(1, &[checkpoint_a]),
        test_checkpoint_requirement(2, &[checkpoint_b]),
    ];
    let plan = test_scene_replay_plan(&[presentation], &requirements);
    let mut state = SceneReplayWorkState::new(&plan, SceneReplayWorkMode::GlobalBaseline);

    let baseline = state.active_work().to_vec();
    state.mark_capture_satisfied(GraphPassId::new(1).unwrap());
    state.mark_capture_satisfied(GraphPassId::new(2).unwrap());
    assert_eq!(state.active_work(), baseline.as_slice());
}

#[test]
fn scene_replay_work_has_strict_reduction_after_an_earlier_capture() {
    let presentation = OutputRect::new(0, 0, 20, 20);
    let checkpoint_a = OutputRect::new(30, 0, 20, 20);
    let checkpoint_b = OutputRect::new(60, 0, 20, 20);
    let requirements = [
        test_checkpoint_requirement(1, &[checkpoint_a]),
        test_checkpoint_requirement(2, &[checkpoint_b]),
    ];
    let plan = test_scene_replay_plan(&[presentation], &requirements);
    let mut state = SceneReplayWorkState::new(&plan, SceneReplayWorkMode::SuffixDemand);
    let initial_pixels = output_work_pixels(state.active_work());

    state.mark_capture_satisfied(GraphPassId::new(1).unwrap());

    assert!(output_work_pixels(state.active_work()) < initial_pixels);
    assert_same_output_region(state.active_work(), &[presentation, checkpoint_b]);
}

#[test]
fn scene_valid_region_replaces_expired_work_after_scene_advance() {
    let presentation = OutputRect::new(0, 0, 10, 10);
    let expired = OutputRect::new(20, 0, 10, 10);
    let pending = OutputRect::new(40, 0, 10, 10);
    let valid = scene_valid_region_after_scene_advance(&[presentation, pending]);

    assert!(valid.contains_point(5, 5));
    assert!(valid.contains_point(45, 5));
    assert!(!valid.contains_point(25, 5));
    assert!(
        valid
            .subtract(&output_rects_to_effect_region(&[presentation, expired]))
            .contains_point(45, 5)
    );
}
