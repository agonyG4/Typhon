use super::*;

#[test]
fn diagnostic_framebuffer_extra_scene_work_preserves_prior_pixels() {
    let mut harness = GlesEffectTestHarness::new(128, 96);
    let output_bounds = EffectRect::new(0, 0, 128, 96).expect("diagnostic output bounds");
    let target_rect = EffectRect::new(24, 20, 80, 56).expect("diagnostic target bounds");
    let repair = OutputRect::new(60, 44, 4, 4);
    let visual_group = VisualGroupId::new(9).expect("diagnostic visual group");
    install_diagnostic_scene(&mut harness, target_rect, visual_group);
    let graph = diagnostic_graph(
        target_rect,
        visual_group,
        &EffectRegion::from_rect(output_bounds),
        output_bounds,
    );
    let capture = graph
        .textures
        .iter()
        .find(|texture| texture.source == GraphTextureSource::CapturedScene)
        .expect("diagnostic blur capture texture");
    assert!(capture.domain.width >= repair.width.saturating_mul(4));
    assert!(capture.domain.height >= repair.height.saturating_mul(4));
    let config = effects::EffectDebugConfig::new(
        effects::EffectDebugCaptureMode::Framebuffer,
        effects::EffectDebugKawaseMode::Partial,
    );
    let full_plan = diagnostic_repaint_plan(repair, true);
    let full_demand = oblivion_one::effects::plan_effect_execution_demand_with_kawase_mode(
        &graph,
        &EffectRegion::from_rect(output_bounds),
        true,
        false,
    );
    let full_selection = effects::select_effect_execution(&graph, &full_demand);
    effects::execute_effect_graph_with_debug_config(
        &mut harness.renderer,
        &graph,
        OutputFramebufferOrigin::BottomLeft,
        &full_plan,
        &full_demand,
        &full_selection,
        config,
    )
    .expect("diagnostic full frame renders");
    let previous = read_diagnostic_pixels(&harness);

    let partial_plan = diagnostic_repaint_plan(repair, false);
    // The graph keeps its visible capture domain while this frame's small
    // presentation repair supplies the execution demand below.
    let partial_source_damage = EffectRegion::empty();
    let partial_graph = diagnostic_graph(
        target_rect,
        visual_group,
        &partial_source_damage,
        output_bounds,
    );
    let repair_region = diagnostic_region(repair);
    let partial_demand = oblivion_one::effects::plan_effect_execution_demand_with_kawase_mode(
        &partial_graph,
        &repair_region,
        false,
        false,
    );
    let partial_selection = effects::select_effect_execution(&partial_graph, &partial_demand);
    effects::execute_effect_graph_with_debug_config(
        &mut harness.renderer,
        &partial_graph,
        OutputFramebufferOrigin::BottomLeft,
        &partial_plan,
        &partial_demand,
        &partial_selection,
        config,
    )
    .expect("diagnostic partial frame renders");
    let after = read_diagnostic_pixels(&harness);
    let width = harness.renderer.scene_state.current_size.0;
    let height = harness.renderer.scene_state.current_size.1;
    for y in 0..height {
        for x in 0..width {
            if repair.x <= x as i32
                && (x as i32) < repair.x + repair.width as i32
                && repair.y <= y as i32
                && (y as i32) < repair.y + repair.height as i32
            {
                continue;
            }
            assert_eq!(
                diagnostic_pixel(&after, width, height, x, y),
                diagnostic_pixel(&previous, width, height, x, y),
                "outside-repair pixel changed at ({x}, {y})"
            );
        }
    }
}

#[test]
fn diagnostic_debug_matrix_preserves_partial_framebuffer_pixels() {
    let output_bounds = EffectRect::new(0, 0, 128, 96).expect("diagnostic output bounds");
    let target_rect = EffectRect::new(24, 20, 80, 56).expect("diagnostic target bounds");
    let repair = OutputRect::new(60, 44, 4, 4);
    let visual_group = VisualGroupId::new(9).expect("diagnostic visual group");
    let policies = [
        (
            "replay+partial",
            effects::EffectDebugConfig::new(
                effects::EffectDebugCaptureMode::Replay,
                effects::EffectDebugKawaseMode::Partial,
            ),
        ),
        (
            "replay+full",
            effects::EffectDebugConfig::new(
                effects::EffectDebugCaptureMode::Replay,
                effects::EffectDebugKawaseMode::Full,
            ),
        ),
        (
            "framebuffer+partial",
            effects::EffectDebugConfig::new(
                effects::EffectDebugCaptureMode::Framebuffer,
                effects::EffectDebugKawaseMode::Partial,
            ),
        ),
        (
            "framebuffer+full",
            effects::EffectDebugConfig::new(
                effects::EffectDebugCaptureMode::Framebuffer,
                effects::EffectDebugKawaseMode::Full,
            ),
        ),
    ];

    for (label, config) in policies {
        let mut candidate = GlesEffectTestHarness::new(128, 96);
        install_diagnostic_scene(&mut candidate, target_rect, visual_group);
        let full_source_damage = EffectRegion::from_rect(output_bounds);
        let candidate_graph = diagnostic_graph(
            target_rect,
            visual_group,
            &full_source_damage,
            output_bounds,
        );
        let full_plan = diagnostic_repaint_plan(repair, true);
        execute_diagnostic_frame(
            &mut candidate,
            &candidate_graph,
            &full_plan,
            EffectRegion::from_rect(output_bounds),
            true,
            config,
        );
        let previous = read_diagnostic_pixels(&candidate);
        let partial_plan = diagnostic_repaint_plan(repair, false);
        let partial_source_damage = EffectRegion::empty();
        let partial_graph = diagnostic_graph(
            target_rect,
            visual_group,
            &partial_source_damage,
            output_bounds,
        );
        let capture = partial_graph
            .textures
            .iter()
            .find(|texture| texture.source == GraphTextureSource::CapturedScene)
            .expect("diagnostic partial blur capture texture");
        assert!(
            capture.domain.width >= repair.width.saturating_mul(4),
            "{label}"
        );
        assert!(
            capture.domain.height >= repair.height.saturating_mul(4),
            "{label}"
        );
        execute_diagnostic_frame(
            &mut candidate,
            &partial_graph,
            &partial_plan,
            diagnostic_region(repair),
            false,
            config,
        );
        let actual = read_diagnostic_pixels(&candidate);
        drop(candidate);

        let mut reference = GlesEffectTestHarness::new(128, 96);
        install_diagnostic_scene(&mut reference, target_rect, visual_group);
        let reference_graph = diagnostic_graph(
            target_rect,
            visual_group,
            &full_source_damage,
            output_bounds,
        );
        execute_diagnostic_frame(
            &mut reference,
            &reference_graph,
            &full_plan,
            EffectRegion::from_rect(output_bounds),
            true,
            config,
        );
        let full_reference = read_diagnostic_pixels(&reference);
        assert_diagnostic_matrix_pixels(
            &actual,
            &previous,
            &full_reference,
            128,
            96,
            repair,
            2,
            label,
        );
    }
}

#[test]
fn replay_partial_whole_graph_is_independent_of_poisoned_capture_contents() {
    let output_bounds = EffectRect::new(0, 0, 512, 384).expect("diagnostic output bounds");
    let target_rect = EffectRect::new(145, 148, 321, 181).expect("diagnostic target bounds");
    let repair = OutputRect::new(292, 220, 5, 3);
    let visual_group = VisualGroupId::new(9).expect("diagnostic visual group");
    let config = effects::EffectDebugConfig::new(
        effects::EffectDebugCaptureMode::Replay,
        effects::EffectDebugKawaseMode::Partial,
    );

    let render_poisoned_candidate = |poison: [f32; 4]| {
        let mut candidate = GlesEffectTestHarness::new(512, 384);
        install_diagnostic_scene(&mut candidate, target_rect, visual_group);
        let graph = diagnostic_graph_with_spec(
            target_rect,
            visual_group,
            &EffectRegion::empty(),
            output_bounds,
            4.0,
            2,
            0.5,
        );
        let full_plan = diagnostic_repaint_plan_for_repairs_in_size(&[repair], true, (512, 384));
        execute_diagnostic_frame(
            &mut candidate,
            &graph,
            &full_plan,
            EffectRegion::from_rect(output_bounds),
            true,
            config,
        );
        let previous = read_diagnostic_pixels(&candidate);
        assert!(
            candidate
                .renderer
                .effect_runtime
                .effect_resources
                .poison_cached_textures(&candidate.gl, poison)
                > 0,
            "diagnostic candidate must reuse a pooled texture"
        );

        let partial_graph = &graph;
        let partial_demand = oblivion_one::effects::plan_effect_execution_demand_with_kawase_mode(
            partial_graph,
            &diagnostic_region(repair),
            false,
            false,
        );
        let partial_capture = partial_graph
            .passes
            .iter()
            .find(|pass| pass.kind == oblivion_one::effects::RenderPassKind::SceneCapture)
            .expect("partial diagnostic capture pass");
        let partial_composite = partial_graph
            .passes
            .iter()
            .find(|pass| pass.kind == oblivion_one::effects::RenderPassKind::Composite)
            .expect("partial diagnostic composite pass");
        assert!(
            partial_demand
                .pass_output_region(partial_capture.id)
                .is_some()
        );
        assert!(
            partial_demand
                .pass_output_region(partial_composite.id)
                .is_some()
        );
        let partial_plan =
            diagnostic_repaint_plan_for_repairs_in_size(&[repair], false, (512, 384));
        execute_diagnostic_frame(
            &mut candidate,
            partial_graph,
            &partial_plan,
            diagnostic_region(repair),
            false,
            config,
        );
        (previous, read_diagnostic_pixels(&candidate))
    };

    let (previous_a, actual_a) = render_poisoned_candidate([1.0, 0.0, 1.0, 1.0]);
    let (previous_b, actual_b) = render_poisoned_candidate([0.0, 1.0, 0.0, 1.0]);

    let mut reference = GlesEffectTestHarness::new(512, 384);
    install_diagnostic_scene(&mut reference, target_rect, visual_group);
    let reference_graph = diagnostic_graph_with_spec(
        target_rect,
        visual_group,
        &EffectRegion::from_rect(output_bounds),
        output_bounds,
        4.0,
        2,
        0.5,
    );
    execute_diagnostic_frame(
        &mut reference,
        &reference_graph,
        &diagnostic_repaint_plan_for_repairs_in_size(&[repair], true, (512, 384)),
        EffectRegion::from_rect(output_bounds),
        true,
        config,
    );
    let full_reference = read_diagnostic_pixels(&reference);

    assert_diagnostic_matrix_pixels(
        &actual_a,
        &previous_a,
        &full_reference,
        512,
        384,
        repair,
        2,
        "replay+partial poison A",
    );
    assert_diagnostic_matrix_pixels(
        &actual_b,
        &previous_b,
        &full_reference,
        512,
        384,
        repair,
        2,
        "replay+partial poison B",
    );
    for (index, (poison_a, poison_b)) in actual_a.iter().zip(&actual_b).enumerate() {
        assert_eq!(
            poison_a, poison_b,
            "replay+partial output depends on pooled poison at byte {index}"
        );
    }
}

#[test]
fn native_faithful_topbar_checkpoint_replay_partial_matches_full_current_reference() {
    let fixture = native_topbar_fixture();
    let config = effects::EffectDebugConfig::new(
        effects::EffectDebugCaptureMode::Replay,
        effects::EffectDebugKawaseMode::Partial,
    );
    let (_, previous, candidate, events) = render_native_stacked_candidate(fixture, config, None);
    assert_scene_work_preservation_trace_events(&events, "native TopBar");
    let full_current_reference = render_native_stacked_full_reference(fixture, config);
    let (outside, inside) = diagnostic_matrix_mismatch_counts_for_origin(
        &candidate,
        &previous,
        &full_current_reference,
        fixture.output_size.0,
        fixture.output_size.1,
        &[fixture.repair],
        2,
        OutputFramebufferOrigin::TopLeftScanout,
    );
    assert_eq!(
        inside, 0,
        "native TopBar candidate differs from full reference"
    );
    assert_eq!(outside, 0, "native TopBar candidate changed outside repair");

    let (_, _, poison_a, _) =
        render_native_stacked_candidate(fixture, config, Some([1.0, 0.0, 1.0, 1.0]));
    let (_, _, poison_b, _) =
        render_native_stacked_candidate(fixture, config, Some([0.0, 1.0, 0.0, 1.0]));
    assert_native_poison_independence(
        fixture,
        &poison_a,
        &poison_b,
        &full_current_reference,
        "native TopBar",
    );
}

#[test]
fn native_faithful_topbar_baseline_matches_suffix_demand() {
    assert_native_baseline_matches_suffix_demand(native_topbar_fixture(), "native TopBar");
}

#[test]
fn native_faithful_topbar_blit_and_shader_copy_are_pixel_equivalent() {
    let fixture = native_topbar_fixture();
    let blit_config = effects::EffectDebugConfig::new(
        effects::EffectDebugCaptureMode::Replay,
        effects::EffectDebugKawaseMode::Partial,
    );
    let shader_config = effects::EffectDebugConfig::new_with_checkpoint_capture_path(
        effects::EffectDebugCaptureMode::Replay,
        effects::EffectDebugKawaseMode::Partial,
        effects::CheckpointCapturePath::FramebufferShaderCopy,
    );
    let (_, _, blit_pixels, _) = render_native_stacked_candidate(fixture, blit_config, None);
    let (_, _, shader_pixels, shader_events) =
        render_native_stacked_candidate(fixture, shader_config, None);
    assert_eq!(blit_pixels, shader_pixels, "TopBar A/B pixels differ");
    assert!(shader_events.iter().any(|line| {
        line.contains("kind=SceneCapture")
            && line.contains("checkpoints=1")
            && line.contains("executed_capture_path=framebuffer_shader_copy")
    }));
    assert!(shader_events.iter().any(|line| {
        line.contains("event=effect_checkpoint_source_validity")
            && line.contains("missing_pixels=0")
    }));
}

#[test]
fn native_three_checkpoint_suffix_demand_replays_pending_suffix() {
    let fixture = native_three_checkpoint_fixture();
    let config = effects::EffectDebugConfig::new(
        effects::EffectDebugCaptureMode::Replay,
        effects::EffectDebugKawaseMode::Partial,
    );
    let (_, _, baseline, baseline_events) = render_native_three_checkpoint_candidate_with_mode(
        fixture,
        config,
        Some(effects::SceneReplayWorkMode::GlobalBaseline),
    );
    let (_, _, suffix, suffix_events) = render_native_three_checkpoint_candidate_with_mode(
        fixture,
        config,
        Some(effects::SceneReplayWorkMode::SuffixDemand),
    );

    assert_native_three_checkpoint_replay_evidence(
        fixture,
        &baseline,
        &baseline_events,
        &suffix,
        &suffix_events,
    );
}

#[test]
fn native_same_anchor_suffix_demand_retains_later_checkpoint() {
    let fixture = native_same_anchor_fixture();
    let config = effects::EffectDebugConfig::new(
        effects::EffectDebugCaptureMode::Replay,
        effects::EffectDebugKawaseMode::Partial,
    );
    let (_, _, baseline, _) = render_native_three_checkpoint_candidate_with_mode(
        fixture,
        config,
        Some(effects::SceneReplayWorkMode::GlobalBaseline),
    );
    let (_, _, suffix, suffix_events) = render_native_three_checkpoint_candidate_with_mode(
        fixture,
        config,
        Some(effects::SceneReplayWorkMode::SuffixDemand),
    );

    assert_native_same_anchor_replay_evidence(fixture, &baseline, &suffix, &suffix_events);
}

#[test]
fn native_faithful_stacked_dock_checkpoint_replay_partial_matches_full_current_reference() {
    let fixture = native_dock_fixture();
    let config = effects::EffectDebugConfig::new(
        effects::EffectDebugCaptureMode::Replay,
        effects::EffectDebugKawaseMode::Partial,
    );
    let (_, previous, candidate, events) = render_native_stacked_candidate(fixture, config, None);
    assert_scene_work_preservation_trace_events(&events, "native Dock");
    let full_current_reference = render_native_stacked_full_reference(fixture, config);
    let (outside, inside) = diagnostic_matrix_mismatch_counts_for_origin(
        &candidate,
        &previous,
        &full_current_reference,
        fixture.output_size.0,
        fixture.output_size.1,
        &[fixture.repair],
        2,
        OutputFramebufferOrigin::TopLeftScanout,
    );
    assert_eq!(
        inside, 0,
        "native Dock candidate differs from full reference"
    );
    assert_eq!(outside, 0, "native Dock candidate changed outside repair");

    let (_, _, poison_a, _) =
        render_native_stacked_candidate(fixture, config, Some([1.0, 0.0, 1.0, 1.0]));
    let (_, _, poison_b, _) =
        render_native_stacked_candidate(fixture, config, Some([0.0, 1.0, 0.0, 1.0]));
    assert_native_poison_independence(
        fixture,
        &poison_a,
        &poison_b,
        &full_current_reference,
        "native Dock",
    );
}

#[test]
fn native_faithful_stacked_dock_blit_and_shader_copy_are_pixel_equivalent() {
    let fixture = native_dock_fixture();
    let blit_config = effects::EffectDebugConfig::new(
        effects::EffectDebugCaptureMode::Replay,
        effects::EffectDebugKawaseMode::Partial,
    );
    let shader_config = effects::EffectDebugConfig::new_with_checkpoint_capture_path(
        effects::EffectDebugCaptureMode::Replay,
        effects::EffectDebugKawaseMode::Partial,
        effects::CheckpointCapturePath::FramebufferShaderCopy,
    );
    let (_, _, blit_pixels, _) = render_native_stacked_candidate(fixture, blit_config, None);
    let (_, _, shader_pixels, shader_events) =
        render_native_stacked_candidate(fixture, shader_config, None);
    assert_eq!(blit_pixels, shader_pixels, "Dock A/B pixels differ");
    assert!(shader_events.iter().any(|line| {
        line.contains("kind=SceneCapture")
            && line.contains("checkpoints=1")
            && line.contains("executed_capture_path=framebuffer_shader_copy")
    }));
    assert!(shader_events.iter().any(|line| {
        line.contains("event=effect_checkpoint_source_validity")
            && line.contains("missing_pixels=0")
    }));
}

#[test]
fn native_faithful_stacked_dock_production_default_uses_shader_copy() {
    let fixture = native_dock_fixture();
    let config =
        effects::EffectDebugConfig::from_env_values_with_checkpoint_capture_path(None, None, None);
    assert_eq!(
        config.checkpoint_capture_path(),
        effects::CheckpointCapturePath::FramebufferShaderCopy
    );

    let (_, _, _, events) = render_native_stacked_candidate(fixture, config, None);
    let checkpoint_event = events
        .iter()
        .find(|line| {
            line.contains("event=effect_pass_execute_end")
                && line.contains("kind=SceneCapture")
                && line.contains("checkpoints=1")
        })
        .expect("production-default checkpoint capture trace event");
    assert!(checkpoint_event.contains("requested_capture_path=shader-copy"));
    assert!(checkpoint_event.contains("executed_capture_path=framebuffer_shader_copy"));
    assert!(checkpoint_event.contains("fallback_reason=none"));
}

#[test]
fn native_three_checkpoint_blit_and_shader_copy_preserve_dependencies() {
    let fixture = native_three_checkpoint_fixture();
    let blit_config = effects::EffectDebugConfig::new(
        effects::EffectDebugCaptureMode::Replay,
        effects::EffectDebugKawaseMode::Partial,
    );
    let shader_config = effects::EffectDebugConfig::new_with_checkpoint_capture_path(
        effects::EffectDebugCaptureMode::Replay,
        effects::EffectDebugKawaseMode::Partial,
        effects::CheckpointCapturePath::FramebufferShaderCopy,
    );
    let (_, _, blit_pixels, _) =
        render_native_three_checkpoint_candidate_with_mode(fixture, blit_config, None);
    let (_, _, shader_pixels, shader_events) =
        render_native_three_checkpoint_candidate_with_mode(fixture, shader_config, None);
    assert_eq!(
        blit_pixels, shader_pixels,
        "multi-dependency A/B pixels differ"
    );
    let validity_events = shader_events
        .iter()
        .filter(|line| line.contains("event=effect_checkpoint_source_validity"))
        .collect::<Vec<_>>();
    assert!(!validity_events.is_empty());
    assert!(
        validity_events
            .iter()
            .all(|line| line.contains("missing_pixels=0"))
    );
    assert!(shader_events.iter().any(|line| {
        line.contains("executed_capture_path=framebuffer_shader_copy")
            && line.contains("checkpoints=2")
    }));
}

#[test]
fn native_three_checkpoint_incremental_cache_matches_full_capture_reference() {
    let fixture = native_three_checkpoint_fixture();
    let incremental_config = effects::EffectDebugConfig::new_with_checkpoint_capture_path(
        effects::EffectDebugCaptureMode::Replay,
        effects::EffectDebugKawaseMode::Partial,
        effects::CheckpointCapturePath::FramebufferShaderCopy,
    );
    let full_capture_config = effects::EffectDebugConfig::new(
        effects::EffectDebugCaptureMode::Framebuffer,
        effects::EffectDebugKawaseMode::Partial,
    );
    let full_region = EffectRegion::from_rect(fixture.output_bounds);

    let mut incremental = GlesEffectTestHarness::new(fixture.output_size.0, fixture.output_size.1);
    incremental.install_texture_backed_output();
    install_native_three_checkpoint_diagnostic_scene(&mut incremental, fixture.scene);
    let first_graph = compile_native_three_checkpoint_graph(fixture, &full_region);
    incremental
        .renderer
        .effect_runtime
        .effect_resources
        .begin_checkpoint_frame();
    execute_diagnostic_frame_with_origin(
        &mut incremental,
        &first_graph,
        &diagnostic_repaint_plan_for_repairs_in_size(&[fixture.repair], true, fixture.output_size),
        full_region.clone(),
        true,
        incremental_config,
        OutputFramebufferOrigin::TopLeftScanout,
    );
    drop(first_graph);
    assert_eq!(
        incremental
            .renderer
            .effect_runtime
            .effect_resources
            .checkpoint_cache_stats()
            .0,
        3,
        "the root replay checkpoint and two dependent checkpoints are cached"
    );
    let previous = read_diagnostic_pixels(&incremental);

    update_diagnostic_background_for_surface(
        &incremental,
        fixture.scene.background_surface,
        fixture.repair,
        [236, 28, 42, 255],
    );
    let current_damage = diagnostic_region(fixture.repair);
    let current_graph = compile_native_three_checkpoint_graph(fixture, &current_damage);
    let current_demand = oblivion_one::effects::plan_effect_execution_demand_with_kawase_mode(
        &current_graph,
        &current_damage,
        false,
        false,
    );
    let current_selection = effects::select_effect_execution(&current_graph, &current_demand);
    for instance in [31, 32, 33] {
        assert!(
            current_selection
                .executed_instances
                .contains(&oblivion_one::effects::EffectInstanceId::new(instance).unwrap()),
            "stack dependency instance {instance} remains selected"
        );
    }
    incremental
        .renderer
        .effect_runtime
        .effect_resources
        .begin_checkpoint_frame();
    execute_diagnostic_frame_with_origin(
        &mut incremental,
        &current_graph,
        &diagnostic_repaint_plan_for_repairs_in_size(&[fixture.repair], false, fixture.output_size),
        current_damage,
        false,
        incremental_config,
        OutputFramebufferOrigin::TopLeftScanout,
    );
    let actual = read_diagnostic_pixels(&incremental);
    assert_eq!(
        incremental
            .renderer
            .effect_runtime
            .effect_resources
            .checkpoint_cache_stats()
            .0,
        3,
        "the root replay entry coexists with semantic C and B checkpoint entries"
    );

    let cache_passes = current_graph
        .passes
        .iter()
        .filter(|pass| {
            pass.kind == RenderPassKind::SceneCapture && !pass.checkpoint_dependencies.is_empty()
        })
        .collect::<Vec<_>>();
    assert_eq!(cache_passes.len(), 2);
    let incremental_checkpoint_snapshots = cache_passes
        .iter()
        .map(|pass| {
            let key = effects::checkpoint_capture_cache_key(&current_graph, pass).unwrap();
            let texture_plan = current_graph
                .textures
                .iter()
                .find(|texture| Some(texture.id) == pass.output)
                .unwrap();
            let texture = incremental
                .renderer
                .effect_runtime
                .effect_resources
                .checkpoint_capture_texture(&key)
                .expect("incremental checkpoint texture remains cache-owned");
            let pixels = read_effect_texture_pixels(
                &mut incremental,
                &texture,
                texture_plan.width,
                texture_plan.height,
            );
            (
                key,
                pass.instance.get(),
                texture_plan.width,
                texture_plan.height,
                pixels,
            )
        })
        .collect::<Vec<_>>();
    assert_ne!(
        &incremental_checkpoint_snapshots[0].0,
        &incremental_checkpoint_snapshots[1].0
    );
    drop(cache_passes);
    drop(current_graph);
    drop(incremental);

    let mut full_refresh = GlesEffectTestHarness::new(fixture.output_size.0, fixture.output_size.1);
    full_refresh.install_texture_backed_output();
    install_native_three_checkpoint_diagnostic_scene(&mut full_refresh, fixture.scene);
    update_diagnostic_background_for_surface(
        &full_refresh,
        fixture.scene.background_surface,
        fixture.repair,
        [236, 28, 42, 255],
    );
    let reference_graph = compile_native_three_checkpoint_graph(fixture, &full_region);
    full_refresh
        .renderer
        .effect_runtime
        .effect_resources
        .begin_checkpoint_frame();
    execute_diagnostic_frame_with_origin(
        &mut full_refresh,
        &reference_graph,
        &diagnostic_repaint_plan_for_repairs_in_size(&[fixture.repair], true, fixture.output_size),
        full_region.clone(),
        true,
        incremental_config,
        OutputFramebufferOrigin::TopLeftScanout,
    );
    for (key, instance, width, height, incremental_checkpoint) in &incremental_checkpoint_snapshots
    {
        let reference_texture = full_refresh
            .renderer
            .effect_runtime
            .effect_resources
            .checkpoint_capture_texture(key)
            .expect("full-current checkpoint was fully populated");
        let reference_checkpoint =
            read_effect_texture_pixels(&mut full_refresh, &reference_texture, *width, *height);
        let deltas = incremental_checkpoint
            .iter()
            .zip(&reference_checkpoint)
            .enumerate()
            .filter_map(|(index, (actual, expected))| {
                (actual != expected).then_some((index, actual.abs_diff(*expected)))
            })
            .collect::<Vec<_>>();
        let max_delta = deltas.iter().map(|(_, delta)| *delta).max().unwrap_or(0);
        assert!(
            max_delta <= 1,
            "checkpoint for instance {instance} differs from its full refresh: {} channel mismatches, max delta {max_delta}, first mismatches {:?} ({}x{}; RGBA8 tolerance 1 LSB)",
            deltas.len(),
            &deltas[..deltas.len().min(8)],
            width,
            height
        );
    }
    drop(full_refresh);

    let mut uncached_reference =
        GlesEffectTestHarness::new(fixture.output_size.0, fixture.output_size.1);
    uncached_reference.install_texture_backed_output();
    install_native_three_checkpoint_diagnostic_scene(&mut uncached_reference, fixture.scene);
    update_diagnostic_background_for_surface(
        &uncached_reference,
        fixture.scene.background_surface,
        fixture.repair,
        [236, 28, 42, 255],
    );
    let uncached_graph = compile_native_three_checkpoint_graph(fixture, &full_region);
    uncached_reference
        .renderer
        .effect_runtime
        .effect_resources
        .begin_checkpoint_frame();
    execute_diagnostic_frame_with_origin(
        &mut uncached_reference,
        &uncached_graph,
        &diagnostic_repaint_plan_for_repairs_in_size(&[fixture.repair], true, fixture.output_size),
        full_region,
        true,
        full_capture_config,
        OutputFramebufferOrigin::TopLeftScanout,
    );
    let full_current_reference = read_diagnostic_pixels(&uncached_reference);
    drop(uncached_reference);
    let (outside, inside) = diagnostic_matrix_mismatch_counts_for_origin(
        &actual,
        &previous,
        &full_current_reference,
        fixture.output_size.0,
        fixture.output_size.1,
        &[fixture.repair],
        0,
        OutputFramebufferOrigin::TopLeftScanout,
    );
    assert_eq!(
        inside, 0,
        "incremental stack differs from full-current reference"
    );
    assert_eq!(
        outside, 0,
        "incremental stack changed pixels outside repair"
    );
}
