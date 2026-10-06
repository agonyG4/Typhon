use super::*;

#[test]
fn native_faithful_full_kawase_dock_control() {
    let fixture = native_dock_fixture();
    let config = effects::EffectDebugConfig::new(
        effects::EffectDebugCaptureMode::Replay,
        effects::EffectDebugKawaseMode::Full,
    );
    let (_, previous, candidate, events) = render_native_stacked_candidate(fixture, config, None);
    assert!(events.iter().any(|line| {
        line.contains("event=effect_pass_execute_end")
            && line.contains("kind=SceneCapture")
            && line.contains("checkpoints=1")
            && line.contains("capture_mode=framebuffer_blit")
            && line.contains("backdrop_capture_policy=replay")
            && line.contains("kawase_execution_policy=full")
    }));
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
        "Full Kawase Dock candidate differs from reference"
    );
    assert_eq!(
        outside, 0,
        "Full Kawase Dock candidate changed outside repair"
    );

    let (_, _, poison_a, _) =
        render_native_stacked_candidate(fixture, config, Some([1.0, 0.0, 1.0, 1.0]));
    let (_, _, poison_b, _) =
        render_native_stacked_candidate(fixture, config, Some([0.0, 1.0, 0.0, 1.0]));
    assert_native_poison_independence(
        fixture,
        &poison_a,
        &poison_b,
        &full_current_reference,
        "Full Kawase Dock",
    );
}

#[test]
fn native_faithful_stacked_replay_baseline_matches_suffix_demand() {
    assert_native_baseline_matches_suffix_demand(native_dock_fixture(), "native Dock");
}

#[test]
fn replay_partial_whole_graph_with_linear_post_blur_sampling_is_poison_independent() {
    let output_bounds = EffectRect::new(0, 0, 512, 384).expect("diagnostic output bounds");
    let target_rect = EffectRect::new(145, 148, 321, 181).expect("diagnostic target bounds");
    let repair = OutputRect::new(292, 220, 5, 3);
    let visual_group = VisualGroupId::new(9).expect("diagnostic visual group");
    let config = effects::EffectDebugConfig::new(
        effects::EffectDebugCaptureMode::Replay,
        effects::EffectDebugKawaseMode::Partial,
    );

    let render_candidate = |poison: [f32; 4]| {
        let mut candidate = GlesEffectTestHarness::new(512, 384);
        install_diagnostic_scene(&mut candidate, target_rect, visual_group);
        install_custom_linear_neighbor_shader(&mut candidate);
        let graph = diagnostic_graph_with_linear_neighbor_stage(
            target_rect,
            visual_group,
            &EffectRegion::empty(),
            output_bounds,
        );
        execute_diagnostic_frame(
            &mut candidate,
            &graph,
            &diagnostic_repaint_plan_for_repairs_in_size(&[repair], true, (512, 384)),
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
            "custom diagnostic candidate must reuse a pooled texture"
        );
        execute_diagnostic_frame(
            &mut candidate,
            &graph,
            &diagnostic_repaint_plan_for_repairs_in_size(&[repair], false, (512, 384)),
            diagnostic_region(repair),
            false,
            config,
        );
        (previous, read_diagnostic_pixels(&candidate))
    };

    let (previous_a, actual_a) = render_candidate([1.0, 0.0, 1.0, 1.0]);
    let (previous_b, actual_b) = render_candidate([0.0, 1.0, 0.0, 1.0]);

    let mut reference = GlesEffectTestHarness::new(512, 384);
    install_diagnostic_scene(&mut reference, target_rect, visual_group);
    install_custom_linear_neighbor_shader(&mut reference);
    let reference_graph = diagnostic_graph_with_linear_neighbor_stage(
        target_rect,
        visual_group,
        &EffectRegion::from_rect(output_bounds),
        output_bounds,
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
    let (outside_a, inside_a) = diagnostic_matrix_mismatch_counts(
        &actual_a,
        &previous_a,
        &full_reference,
        512,
        384,
        &[repair],
        2,
    );
    let (outside_b, inside_b) = diagnostic_matrix_mismatch_counts(
        &actual_b,
        &previous_b,
        &full_reference,
        512,
        384,
        &[repair],
        2,
    );
    assert_eq!(
        outside_a, 0,
        "linear post-blur poison A changed outside repair"
    );
    assert_eq!(
        inside_a, 0,
        "linear post-blur poison A differs from reference"
    );
    assert_eq!(
        outside_b, 0,
        "linear post-blur poison B changed outside repair"
    );
    assert_eq!(
        inside_b, 0,
        "linear post-blur poison B differs from reference"
    );
    assert_eq!(
        actual_a, actual_b,
        "linear post-blur output depends on poison"
    );
}

#[test]
fn replay_partial_poison_sweep_finds_no_uninitialized_capture_samples() {
    let cases = [
        (
            (128, 96),
            EffectRect::new(24, 20, 80, 56).unwrap(),
            OutputRect::new(60, 44, 4, 4),
            1,
            1.0,
        ),
        (
            (256, 192),
            EffectRect::new(97, 61, 121, 87).unwrap(),
            OutputRect::new(151, 103, 5, 3),
            2,
            0.5,
        ),
        (
            (512, 384),
            EffectRect::new(145, 148, 321, 181).unwrap(),
            OutputRect::new(292, 220, 5, 3),
            3,
            0.75,
        ),
        (
            (1920, 1080),
            EffectRect::new(145, 148, 1112, 873).unwrap(),
            OutputRect::new(647, 901, 12, 10),
            2,
            0.5,
        ),
        (
            (512, 384),
            EffectRect::new(0, 0, 512, 384).unwrap(),
            OutputRect::new(257, 193, 3, 3),
            4,
            0.5,
        ),
        (
            (513, 385),
            EffectRect::new(17, 19, 479, 347).unwrap(),
            OutputRect::new(256, 192, 3, 3),
            4,
            0.0625,
        ),
        (
            (511, 383),
            EffectRect::new(3, 5, 503, 371).unwrap(),
            OutputRect::new(253, 191, 5, 3),
            3,
            0.9375,
        ),
    ];
    let visual_group = VisualGroupId::new(9).expect("diagnostic visual group");
    let config = effects::EffectDebugConfig::new(
        effects::EffectDebugCaptureMode::Replay,
        effects::EffectDebugKawaseMode::Partial,
    );

    for (output_size, target_rect, repair, passes, scale) in cases {
        let output_bounds =
            EffectRect::new(0, 0, output_size.0, output_size.1).expect("diagnostic output bounds");
        let mut reference = GlesEffectTestHarness::new(output_size.0, output_size.1);
        install_diagnostic_scene(&mut reference, target_rect, visual_group);
        let reference_graph = diagnostic_graph_with_spec(
            target_rect,
            visual_group,
            &EffectRegion::empty(),
            output_bounds,
            4.0,
            passes,
            scale,
        );
        execute_diagnostic_frame(
            &mut reference,
            &reference_graph,
            &diagnostic_repaint_plan_for_repairs_in_size(&[repair], true, output_size),
            EffectRegion::from_rect(output_bounds),
            true,
            config,
        );
        let full_reference = read_diagnostic_pixels(&reference);
        drop(reference);
        let mut outputs = Vec::new();
        for poison in [[1.0, 0.0, 1.0, 1.0], [0.0, 1.0, 1.0, 1.0]] {
            let mut candidate = GlesEffectTestHarness::new(output_size.0, output_size.1);
            install_diagnostic_scene(&mut candidate, target_rect, visual_group);
            let graph = diagnostic_graph_with_spec(
                target_rect,
                visual_group,
                &EffectRegion::empty(),
                output_bounds,
                4.0,
                passes,
                scale,
            );
            execute_diagnostic_frame(
                &mut candidate,
                &graph,
                &diagnostic_repaint_plan_for_repairs_in_size(&[repair], true, output_size),
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
                "poison sweep candidate must reuse a pooled texture"
            );
            execute_diagnostic_frame(
                &mut candidate,
                &graph,
                &diagnostic_repaint_plan_for_repairs_in_size(&[repair], false, output_size),
                diagnostic_region(repair),
                false,
                config,
            );
            let actual = read_diagnostic_pixels(&candidate);
            let (outside, inside) = diagnostic_matrix_mismatch_counts(
                &actual,
                &previous,
                &full_reference,
                output_size.0,
                output_size.1,
                &[repair],
                2,
            );
            assert_eq!(
                outside, 0,
                "poison sweep changed outside repair for size={output_size:?} target={target_rect:?} passes={passes} scale={scale}"
            );
            assert_eq!(
                inside, 0,
                "poison sweep changed repair for size={output_size:?} target={target_rect:?} passes={passes} scale={scale}"
            );
            outputs.push(actual);
        }
        assert_eq!(
            outputs[0], outputs[1],
            "poison sweep output depends on pooled contents for size={output_size:?} target={target_rect:?} passes={passes} scale={scale}"
        );
    }
}

#[test]
fn replay_partial_fragmented_capture_materialization_is_a_non_regression() {
    let output_bounds = EffectRect::new(0, 0, 512, 384).expect("diagnostic output bounds");
    let target_rect = EffectRect::new(180, 130, 100, 80).expect("diagnostic target bounds");
    let repairs = [
        OutputRect::new(208, 150, 4, 4),
        OutputRect::new(248, 190, 4, 4),
    ];
    let visual_group = VisualGroupId::new(9).expect("diagnostic visual group");
    let config = effects::EffectDebugConfig::new(
        effects::EffectDebugCaptureMode::Replay,
        effects::EffectDebugKawaseMode::Partial,
    );

    let mut reference = GlesEffectTestHarness::new(512, 384);
    install_diagnostic_scene(&mut reference, target_rect, visual_group);
    let reference_graph = diagnostic_graph(
        target_rect,
        visual_group,
        &EffectRegion::from_rect(output_bounds),
        output_bounds,
    );
    execute_diagnostic_frame(
        &mut reference,
        &reference_graph,
        &diagnostic_repaint_plan_for_repairs_in_size(&repairs, true, (512, 384)),
        EffectRegion::from_rect(output_bounds),
        true,
        config,
    );
    let full_reference = read_diagnostic_pixels(&reference);
    drop(reference);

    let mut candidate = GlesEffectTestHarness::new(512, 384);
    install_diagnostic_scene(&mut candidate, target_rect, visual_group);
    let graph = diagnostic_graph(
        target_rect,
        visual_group,
        &EffectRegion::empty(),
        output_bounds,
    );
    execute_diagnostic_frame(
        &mut candidate,
        &graph,
        &diagnostic_repaint_plan_for_repairs_in_size(&repairs, true, (512, 384)),
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
            .poison_cached_textures(&candidate.gl, [1.0, 0.0, 1.0, 1.0])
            > 0,
        "fragmented materialization candidate must reuse a pooled texture"
    );
    execute_diagnostic_frame(
        &mut candidate,
        &graph,
        &diagnostic_repaint_plan_for_repairs_in_size(&repairs, false, (512, 384)),
        diagnostic_region_for_repairs(&repairs),
        false,
        config,
    );
    let actual = read_diagnostic_pixels(&candidate);
    let (outside, inside) = diagnostic_matrix_mismatch_counts(
        &actual,
        &previous,
        &full_reference,
        512,
        384,
        &repairs,
        2,
    );
    assert_eq!(
        outside, 0,
        "fragmented replay changed pixels outside repair"
    );
    assert_eq!(inside, 0, "fragmented replay differs from full reference");
}

#[test]
fn replay_partial_translated_capture_domain_reuses_pool_without_stale_content() {
    let output_bounds = EffectRect::new(0, 0, 256, 192).expect("diagnostic output bounds");
    let rect_a = EffectRect::new(96, 72, 64, 48).expect("first blur bounds");
    let rect_b = EffectRect::new(112, 80, 64, 48).expect("translated blur bounds");
    let repairs = [
        OutputRect::new(rect_a.x, rect_a.y, rect_a.width, rect_a.height),
        OutputRect::new(rect_b.x, rect_b.y, rect_b.width, rect_b.height),
    ];
    let visual_group = VisualGroupId::new(9).expect("diagnostic visual group");
    let config = effects::EffectDebugConfig::new(
        effects::EffectDebugCaptureMode::Replay,
        effects::EffectDebugKawaseMode::Partial,
    );

    let mut candidate = GlesEffectTestHarness::new(256, 192);
    install_diagnostic_scene(&mut candidate, rect_a, visual_group);
    let graph_a = diagnostic_graph(rect_a, visual_group, &EffectRegion::empty(), output_bounds);
    execute_diagnostic_frame(
        &mut candidate,
        &graph_a,
        &diagnostic_repaint_plan_for_repairs_in_size(&repairs, true, (256, 192)),
        EffectRegion::from_rect(output_bounds),
        true,
        config,
    );
    let previous = read_diagnostic_pixels(&candidate);

    install_diagnostic_scene(&mut candidate, rect_b, visual_group);
    let graph_b = diagnostic_graph(rect_b, visual_group, &EffectRegion::empty(), output_bounds);
    let capture_a = graph_a
        .textures
        .iter()
        .find(|texture| texture.source == GraphTextureSource::CapturedScene)
        .expect("first capture texture");
    let capture_b = graph_b
        .textures
        .iter()
        .find(|texture| texture.source == GraphTextureSource::CapturedScene)
        .expect("translated capture texture");
    assert_ne!(capture_a.domain, capture_b.domain);
    assert_eq!(
        (capture_a.width, capture_a.height),
        (capture_b.width, capture_b.height)
    );
    assert!(
        candidate
            .renderer
            .effect_runtime
            .effect_resources
            .poison_cached_textures(&candidate.gl, [0.0, 1.0, 1.0, 1.0])
            > 0,
        "translated candidate must reuse a pooled texture"
    );

    execute_diagnostic_frame(
        &mut candidate,
        &graph_b,
        &diagnostic_repaint_plan_for_repairs_in_size(&repairs, false, (256, 192)),
        diagnostic_region_for_repairs(&repairs),
        false,
        config,
    );
    let actual = read_diagnostic_pixels(&candidate);
    drop(candidate);

    let mut reference = GlesEffectTestHarness::new(256, 192);
    install_diagnostic_scene(&mut reference, rect_b, visual_group);
    execute_diagnostic_frame(
        &mut reference,
        &graph_b,
        &diagnostic_repaint_plan_for_repairs_in_size(&repairs, true, (256, 192)),
        EffectRegion::from_rect(output_bounds),
        true,
        config,
    );
    let full_reference = read_diagnostic_pixels(&reference);
    let (outside, inside) = diagnostic_matrix_mismatch_counts(
        &actual,
        &previous,
        &full_reference,
        256,
        192,
        &repairs,
        2,
    );
    assert_eq!(
        outside, 0,
        "translated replay changed pixels outside repair"
    );
    assert_eq!(inside, 0, "translated replay differs from full reference");
}

#[test]
fn moving_blur_domain_reuses_real_gles_resources() {
    let mut harness = GlesEffectTestHarness::new(256, 192);
    let output_bounds = EffectRect::new(0, 0, 256, 192).expect("output bounds");
    let full_damage = EffectRegion::from_rect(output_bounds);
    let repaint_plan = RepaintPlan {
        render_damage: OutputDamage::Full,
        repair_damage: OutputDamage::Full,
        buffer_age: None,
        mode: RepaintMode::Full,
        fallback_reason: None,
        ..RepaintPlan::default()
    };
    let mut first_dimensions = None;
    let mut first_cache_bytes = None;
    let mut first_allocation_count = None;
    let mut capture_radii = None;
    let mut positions = Vec::with_capacity(512);
    for step in 0..512 {
        let phase = step % 16;
        let (x, y) = match phase {
            0 => (32, 32),
            1 => (33, 33),
            2 => (184, 32),
            3 => (183, 31),
            4 => (184, 144),
            5 => (183, 143),
            6 => (32, 144),
            7 => (33, 143),
            8 => (112, 80),
            9 => (113, 81),
            10 => (112, 80),
            11 => (32, 32),
            12 => (224, 168),
            13 => (0, 168),
            14 => (224, 0),
            _ => (0, 0),
        };
        positions.push((x, y));
    }

    for (step, (x, y)) in positions.into_iter().enumerate() {
        let rect = EffectRect::new(x, y, 32, 24).expect("moving blur rectangle");
        let (scene, registry) = moving_blur_scene(rect);
        let plan = oblivion_one::effects::compile_frame_execution_plan(
            &scene,
            &full_damage,
            output_bounds,
            &registry,
        )
        .expect("moving blur graph compiles");
        let oblivion_one::effects::FrameExecutionPlan::EffectGraph(graph) = plan else {
            panic!("moving blur must compile to an effect graph");
        };
        let demand = plan_effect_execution_demand(&graph, &full_damage, true);
        let selection = effects::select_effect_execution(&graph, &demand);

        effects::execute_effect_graph(
            &mut harness.renderer,
            &graph,
            OutputFramebufferOrigin::BottomLeft,
            &repaint_plan,
            &demand,
            &selection,
        )
        .expect("moving blur graph executes in real GLES");

        let mut first_gl_error = None;
        loop {
            let error = unsafe { harness.gl.get_error() };
            if error == glow::NO_ERROR {
                break;
            }
            first_gl_error.get_or_insert(error);
        }
        assert_eq!(first_gl_error, None, "step {step} left a GLES error");

        let capture = graph
            .textures
            .iter()
            .find(|texture| texture.source == GraphTextureSource::CapturedScene)
            .expect("moving blur capture texture");
        let (capture_radius_x, capture_radius_y) = *capture_radii.get_or_insert((
            x.saturating_sub(capture.domain.x).max(0),
            y.saturating_sub(capture.domain.y).max(0),
        ));
        let expected_left = (x - capture_radius_x).max(0);
        let expected_top = (y - capture_radius_y).max(0);
        let expected_right = (x + 32 + capture_radius_x).min(256);
        let expected_bottom = (y + 24 + capture_radius_y).min(192);
        let expected_capture = EffectRect::new(
            expected_left,
            expected_top,
            u32::try_from(expected_right - expected_left).expect("capture width"),
            u32::try_from(expected_bottom - expected_top).expect("capture height"),
        )
        .expect("expected capture domain");
        for texture in &graph.textures {
            assert!(texture.width > 0 && texture.height > 0);
            if texture.source != GraphTextureSource::Output {
                assert_eq!(texture.domain, expected_capture, "step {step}");
            }
        }
        let dimensions = graph
            .textures
            .iter()
            .filter(|texture| texture.source != GraphTextureSource::Output)
            .map(|texture| (texture.width, texture.height))
            .collect::<Vec<_>>();
        let interior = expected_left == x - capture_radius_x
            && expected_top == y - capture_radius_y
            && expected_right == x + 32 + capture_radius_x
            && expected_bottom == y + 24 + capture_radius_y;
        if interior {
            if let Some(first) = &first_dimensions {
                assert_eq!(first, &dimensions, "translation changed texture dimensions");
            } else {
                first_dimensions = Some(dimensions);
            }
        }

        let metrics = harness.renderer.effect_runtime.effect_resources.metrics();
        assert_eq!(metrics.checked_out_texture_count, 0, "step {step}");
        if step == 15 {
            first_cache_bytes = Some(metrics.current_bytes);
            first_allocation_count = Some(metrics.allocation_count);
        }
        if step >= 16 {
            assert_eq!(
                metrics.current_bytes,
                first_cache_bytes.expect("warm cache bytes"),
                "step {step} changed cache size"
            );
            assert_eq!(
                metrics.allocation_count,
                first_allocation_count.expect("warm allocation count"),
                "step {step} allocated again"
            );
        }
        assert_eq!(metrics.eviction_count, 0, "translation evicted a resource");
        assert!(metrics.cached_texture_count <= 16);
    }

    let metrics = harness.renderer.effect_runtime.effect_resources.metrics();
    assert!(
        metrics.reuse_count > 0,
        "moving blur did not reuse pooled textures"
    );
    assert_eq!(metrics.checked_out_texture_count, 0);
}

#[test]
fn moving_visual_group_blur_replays_only_background_below_target() {
    let mut harness = GlesEffectTestHarness::new(256, 192);
    const BACKGROUND_COLOR: u32 = 0xff20_4060;
    harness
        .renderer
        .resources
        .test_create_surface_texture(&harness.gl, 42, 32, 24, None)
        .expect("target scene texture creates");
    harness
        .renderer
        .resources
        .test_create_decoration_texture(
            &harness.gl,
            EglDrawLayer::SolidRgba(BACKGROUND_COLOR),
            1,
            1,
            None,
        )
        .expect("background scene texture creates");

    let output_bounds = EffectRect::new(0, 0, 256, 192).expect("output bounds");
    let full_damage = EffectRegion::from_rect(output_bounds);
    let repaint_plan = RepaintPlan {
        render_damage: OutputDamage::Full,
        repair_damage: OutputDamage::Full,
        buffer_age: None,
        mode: RepaintMode::Full,
        fallback_reason: None,
        ..RepaintPlan::default()
    };
    let visual_group = VisualGroupId::new(9).expect("visual group id");
    let (_, registry) = moving_blur_scene(output_bounds);
    let positions = [
        (32, 32),
        (33, 33),
        (184, 32),
        (183, 31),
        (224, 144),
        (240, 168),
        (240, 180),
        (0, 168),
        (-8, 180),
        (-8, -4),
        (0, 0),
        (112, 80),
        (113, 81),
        (112, 80),
        (32, 32),
        (184, 144),
    ];
    let mut first_dimensions = None;
    let mut warm_cache_bytes = None;
    let mut warm_allocation_count = None;
    let mut capture_radii = None;

    for (step, (x, y)) in positions.iter().copied().cycle().take(512).enumerate() {
        let rect = EffectRect::new(x, y, 32, 24).expect("moving target rectangle");
        install_visual_group_scene_commands(&mut harness.renderer, rect, visual_group);
        let scene = moving_visual_group_blur_scene(rect, visual_group);
        let plan = oblivion_one::effects::compile_frame_execution_plan(
            &scene,
            &full_damage,
            output_bounds,
            &registry,
        )
        .expect("visual-group blur graph compiles");
        let oblivion_one::effects::FrameExecutionPlan::EffectGraph(graph) = plan else {
            panic!("visual-group blur must compile to an effect graph");
        };
        let capture_pass = graph
            .passes
            .iter()
            .find(|pass| pass.kind == oblivion_one::effects::RenderPassKind::SceneCapture)
            .expect("visual-group blur capture pass");
        assert_eq!(
            capture_pass.anchor,
            oblivion_one::compositor::EffectAnchor::BeforeSurface(42)
        );
        assert_eq!(capture_pass.visual_group, Some(visual_group));
        assert_eq!(
            capture_pass.anchor_scope,
            oblivion_one::compositor::EffectAnchorScope::VisualGroup
        );
        assert!(capture_pass.checkpoint_dependencies.is_empty());

        let demand = plan_effect_execution_demand(&graph, &full_damage, true);
        let selection = effects::select_effect_execution(&graph, &demand);
        let before_replays = harness.renderer.last_frame_stats().draw_command_replays;
        effects::execute_effect_graph(
            &mut harness.renderer,
            &graph,
            OutputFramebufferOrigin::BottomLeft,
            &repaint_plan,
            &demand,
            &selection,
        )
        .expect("visual-group blur graph executes in real GLES");
        let after_replays = harness.renderer.last_frame_stats().draw_command_replays;
        let expected_replays = match effects::effect_debug_config().capture_mode() {
            effects::EffectDebugCaptureMode::Replay => 3,
            effects::EffectDebugCaptureMode::Framebuffer => 2,
        };
        assert_eq!(
            after_replays.saturating_sub(before_replays),
            expected_replays,
            "step {step}: capture policy {:?} must keep scene ordering without replaying the framebuffer capture",
            effects::effect_debug_config().capture_mode(),
        );

        let mut first_gl_error = None;
        loop {
            let error = unsafe { harness.gl.get_error() };
            if error == glow::NO_ERROR {
                break;
            }
            first_gl_error.get_or_insert(error);
        }
        assert_eq!(first_gl_error, None, "step {step} left a GLES error");

        let capture = graph
            .textures
            .iter()
            .find(|texture| texture.source == GraphTextureSource::CapturedScene)
            .expect("visual-group blur capture texture");
        assert!(capture.domain.width > 0 && capture.domain.height > 0);
        let (capture_radius_x, capture_radius_y) = *capture_radii.get_or_insert((
            x.saturating_sub(capture.domain.x).max(0),
            y.saturating_sub(capture.domain.y).max(0),
        ));
        let expected_left = (x - capture_radius_x).max(0);
        let expected_top = (y - capture_radius_y).max(0);
        let expected_right = (x + 32 + capture_radius_x).min(256);
        let expected_bottom = (y + 24 + capture_radius_y).min(192);
        let expected_capture = EffectRect::new(
            expected_left,
            expected_top,
            u32::try_from(expected_right - expected_left).expect("capture width"),
            u32::try_from(expected_bottom - expected_top).expect("capture height"),
        )
        .expect("expected capture domain");
        assert_eq!(capture.domain, expected_capture, "step {step}");
        let dimensions = graph
            .textures
            .iter()
            .filter(|texture| texture.source != GraphTextureSource::Output)
            .map(|texture| (texture.width, texture.height))
            .collect::<Vec<_>>();
        let interior = expected_left == x - capture_radius_x
            && expected_top == y - capture_radius_y
            && expected_right == x + 32 + capture_radius_x
            && expected_bottom == y + 24 + capture_radius_y;
        if interior {
            if let Some(first) = &first_dimensions {
                assert_eq!(first, &dimensions, "translation changed texture dimensions");
            } else {
                first_dimensions = Some(dimensions);
            }
        }

        let metrics = harness.renderer.effect_runtime.effect_resources.metrics();
        assert_eq!(metrics.checked_out_texture_count, 0, "step {step}");
        if step == positions.len() - 1 {
            warm_cache_bytes = Some(metrics.current_bytes);
            warm_allocation_count = Some(metrics.allocation_count);
        }
        if step >= positions.len() {
            assert_eq!(
                metrics.current_bytes,
                warm_cache_bytes.expect("warm cache bytes"),
                "step {step} changed cache size"
            );
            assert_eq!(
                metrics.allocation_count,
                warm_allocation_count.expect("warm allocation count"),
                "step {step} allocated again"
            );
        }
        assert_eq!(metrics.eviction_count, 0, "translation evicted a resource");
        assert!(metrics.cached_texture_count <= 32);
    }

    let metrics = harness.renderer.effect_runtime.effect_resources.metrics();
    assert!(
        metrics.reuse_count > 0,
        "visual-group blur did not reuse pooled textures"
    );
    assert_eq!(metrics.checked_out_texture_count, 0);
}

#[test]
fn effect_trace_bounds_visual_group_scene_and_final_replay() {
    let mut harness = GlesEffectTestHarness::new(256, 192);
    harness
        .renderer
        .resources
        .test_create_surface_texture(&harness.gl, 42, 32, 24, None)
        .expect("target scene texture creates");
    harness
        .renderer
        .resources
        .test_create_decoration_texture(
            &harness.gl,
            EglDrawLayer::SolidRgba(0xff20_4060),
            1,
            1,
            None,
        )
        .expect("background scene texture creates");
    harness.renderer.effect_runtime.effect_trace =
        effects::EffectExecutionTrace::enabled_for_test();
    effects::clear_effect_trace_test_events();

    let rect = EffectRect::new(32, 32, 32, 24).expect("visual group blur rectangle");
    let visual_group = VisualGroupId::new(9).expect("visual group id");
    install_visual_group_scene_commands(&mut harness.renderer, rect, visual_group);
    let (mut scene, registry) = moving_blur_scene(rect);
    let instance = scene.instances.first_mut().expect("moving blur instance");
    instance.anchor = oblivion_one::compositor::EffectAnchor::BeforeSurface(42);
    instance.visual_group = Some(visual_group);
    instance.anchor_scope = oblivion_one::compositor::EffectAnchorScope::VisualGroup;
    instance.scene_order = oblivion_one::compositor::EffectSceneOrder::for_anchor(instance.anchor);
    let output_bounds = EffectRect::new(0, 0, 256, 192).expect("output bounds");
    let full_damage = EffectRegion::from_rect(output_bounds);
    let repaint_plan = RepaintPlan {
        render_damage: OutputDamage::Full,
        repair_damage: OutputDamage::Full,
        buffer_age: None,
        mode: RepaintMode::Full,
        fallback_reason: None,
        ..RepaintPlan::default()
    };
    let plan = oblivion_one::effects::compile_frame_execution_plan(
        &scene,
        &full_damage,
        output_bounds,
        &registry,
    )
    .expect("visual group blur graph compiles");
    let oblivion_one::effects::FrameExecutionPlan::EffectGraph(graph) = plan else {
        panic!("visual group blur must compile to an effect graph");
    };
    let demand = plan_effect_execution_demand(&graph, &full_damage, true);
    let selection = effects::select_effect_execution(&graph, &demand);

    effects::execute_effect_graph(
        &mut harness.renderer,
        &graph,
        OutputFramebufferOrigin::BottomLeft,
        &repaint_plan,
        &demand,
        &selection,
    )
    .expect("visual group blur graph executes in real GLES");
    let events = effects::take_effect_trace_test_events();

    let capture_execute_end = trace_event_index(
        &events,
        &["event=effect_pass_execute_end", "kind=SceneCapture"],
    );
    let expected_capture_mode = match effects::effect_debug_config().capture_mode() {
        effects::EffectDebugCaptureMode::Replay => "replay",
        effects::EffectDebugCaptureMode::Framebuffer => "framebuffer_blit",
    };
    let capture_execute_end_event = &events[capture_execute_end];
    assert!(capture_execute_end_event.contains(&format!("capture_mode={expected_capture_mode}")));
    assert!(capture_execute_end_event.contains(&format!(
        "backdrop_capture_policy={}",
        effects::effect_debug_config().capture_mode().as_str(),
    )));
    assert!(capture_execute_end_event.contains(&format!(
        "kawase_execution_policy={}",
        effects::effect_debug_config().kawase_mode().as_str(),
    )));
    let composite_resources_end = trace_event_index(
        &events,
        &["event=effect_pass_resources_end", "kind=Composite"],
    );
    let replay_begin = trace_event_index(
        &events,
        &[
            "event=effect_scene_replay_begin",
            "kind=Composite",
            "reason=composite_advance",
        ],
    );
    let replay_end = trace_event_index(
        &events,
        &[
            "event=effect_scene_replay_end",
            "kind=Composite",
            "reason=composite_advance",
        ],
    );
    let composite_validate_begin = trace_event_index(
        &events,
        &["event=effect_pass_validate_begin", "kind=Composite"],
    );
    let composite_end = trace_event_index(&events, &["event=effect_pass_end", "kind=Composite"]);
    let final_replay_begin = trace_event_index(&events, &["event=effect_final_scene_replay_begin"]);
    let final_replay_end = trace_event_index(&events, &["event=effect_final_scene_replay_end"]);
    let overlay_begin = trace_event_index(&events, &["event=effect_overlay_draw_begin"]);
    let overlay_end = trace_event_index(&events, &["event=effect_overlay_draw_end"]);
    let graph_execute_end = trace_event_index(&events, &["event=effect_graph_execute_end"]);

    let framebuffer_capture_advance = if matches!(
        effects::effect_debug_config().capture_mode(),
        effects::EffectDebugCaptureMode::Framebuffer
    ) {
        let begin = trace_event_index(
            &events,
            &[
                "event=effect_scene_replay_begin",
                "kind=SceneCapture",
                "reason=framebuffer_capture",
            ],
        );
        let end = trace_event_index(
            &events,
            &[
                "event=effect_scene_replay_end",
                "kind=SceneCapture",
                "reason=framebuffer_capture",
            ],
        );
        assert!(begin < end);
        assert!(end < capture_execute_end);
        Some((begin, end))
    } else {
        None
    };

    assert!(capture_execute_end < composite_resources_end);
    assert!(composite_resources_end < replay_begin);
    assert!(replay_begin < replay_end);
    assert!(replay_end < composite_validate_begin);
    assert!(composite_validate_begin < composite_end);
    assert!(composite_end < final_replay_begin);
    assert!(final_replay_begin < final_replay_end);
    assert!(final_replay_end < overlay_begin);
    assert!(overlay_begin < overlay_end);
    assert!(overlay_end < graph_execute_end);
    let expected_composite_cursor = if framebuffer_capture_advance.is_some() {
        (
            "scene_cursor_start=1",
            "scene_cursor_end=1",
            "command_count=0",
        )
    } else {
        (
            "scene_cursor_start=0",
            "scene_cursor_end=1",
            "command_count=1",
        )
    };
    assert!(events[replay_begin].contains(expected_composite_cursor.0));
    assert!(events[replay_begin].contains(expected_composite_cursor.1));
    assert!(events[replay_begin].contains(expected_composite_cursor.2));
    assert!(events[final_replay_begin].contains("scene_cursor_start=1"));
    assert!(events[final_replay_begin].contains("scene_cursor_end=2"));
    assert!(events[final_replay_begin].contains("command_count=1"));
}

#[test]
fn effect_trace_bounds_checkpoint_scene_advancement() {
    let mut harness = GlesEffectTestHarness::new(256, 192);
    harness.renderer.effect_runtime.effect_trace =
        effects::EffectExecutionTrace::enabled_for_test();
    effects::clear_effect_trace_test_events();
    for surface_id in [10, 20] {
        harness
            .renderer
            .resources
            .test_create_surface_texture(&harness.gl, surface_id, 1, 1, None)
            .expect("checkpoint scene texture creates");
    }
    push_draw_command(
        &mut harness.renderer.scene_state.vertices,
        &mut harness.renderer.scene_state.commands,
        EglDrawLayer::SolidRgba(0xff20_4060),
        EglRect::new(0.0, 0.0, 256.0, 192.0),
        256,
        192,
        OutputFramebufferOrigin::BottomLeft,
    );
    for (surface_id, y) in [(10, 0.0), (20, 96.0)] {
        push_draw_command(
            &mut harness.renderer.scene_state.vertices,
            &mut harness.renderer.scene_state.commands,
            EglDrawLayer::Surface(surface_id),
            EglRect::new(0.0, y, 256.0, 96.0),
            256,
            192,
            OutputFramebufferOrigin::BottomLeft,
        );
    }
    harness.renderer.scene_state.scene_geometry_dirty = true;

    let rect = EffectRect::new(32, 32, 64, 48).expect("checkpoint blur rectangle");
    let (scene, registry) = moving_blur_scene(rect);
    let mut first = scene.instances[0].clone();
    first.anchor = oblivion_one::compositor::EffectAnchor::BeforeSurface(10);
    first.anchor_scope = oblivion_one::compositor::EffectAnchorScope::Surface;
    first.visual_group = None;
    first.scene_order = oblivion_one::compositor::EffectSceneOrder::for_anchor(first.anchor);
    let mut second = first.clone();
    second.id = oblivion_one::effects::EffectInstanceId::new(2).expect("second instance id");
    second.signature = second.signature.saturating_add(1);
    second.anchor = oblivion_one::compositor::EffectAnchor::BeforeSurface(20);
    second.scene_order = oblivion_one::compositor::EffectSceneOrder::for_anchor(second.anchor);
    let scene = ResolvedEffectScene::new(1, vec![first, second]);
    let output_bounds = EffectRect::new(0, 0, 256, 192).expect("output bounds");
    let full_damage = EffectRegion::from_rect(output_bounds);
    let repaint_plan = RepaintPlan {
        render_damage: OutputDamage::Full,
        repair_damage: OutputDamage::Full,
        buffer_age: None,
        mode: RepaintMode::Full,
        fallback_reason: None,
        ..RepaintPlan::default()
    };
    let plan = oblivion_one::effects::compile_frame_execution_plan(
        &scene,
        &full_damage,
        output_bounds,
        &registry,
    )
    .expect("checkpoint blur graph compiles");
    let oblivion_one::effects::FrameExecutionPlan::EffectGraph(graph) = plan else {
        panic!("checkpoint blur must compile to an effect graph");
    };
    let checkpoint_pass = graph
        .passes
        .iter()
        .find(|pass| {
            pass.kind == oblivion_one::effects::RenderPassKind::SceneCapture
                && !pass.checkpoint_dependencies.is_empty()
        })
        .expect("checkpoint-dependent capture pass");
    let checkpoint_pass_id = checkpoint_pass.id.get().to_string();
    let demand = plan_effect_execution_demand(&graph, &full_damage, true);
    let selection = effects::select_effect_execution(&graph, &demand);

    effects::execute_effect_graph(
        &mut harness.renderer,
        &graph,
        OutputFramebufferOrigin::BottomLeft,
        &repaint_plan,
        &demand,
        &selection,
    )
    .expect("checkpoint blur graph executes in real GLES");
    let events = effects::take_effect_trace_test_events();

    let replay_begin = trace_event_index(
        &events,
        &[
            "event=effect_scene_replay_begin",
            "reason=checkpoint_dependency",
        ],
    );
    let replay_end = trace_event_index(
        &events,
        &[
            "event=effect_scene_replay_end",
            "reason=checkpoint_dependency",
        ],
    );
    let resources_end = trace_event_index(
        &events,
        &[
            "event=effect_pass_resources_end",
            &format!("pass={checkpoint_pass_id}"),
        ],
    );
    let validate_begin = trace_event_index(
        &events,
        &[
            "event=effect_pass_validate_begin",
            &format!("pass={checkpoint_pass_id}"),
        ],
    );

    assert!(events[replay_begin].contains(&format!("pass={checkpoint_pass_id}")));
    assert!(events[replay_end].contains(&format!("pass={checkpoint_pass_id}")));
    assert!(resources_end < replay_begin);
    assert!(replay_begin < replay_end);
    assert!(replay_end < validate_begin);
    assert!(events[replay_begin].contains("scene_cursor_start=1"));
    assert!(events[replay_begin].contains("scene_cursor_end=2"));
    assert!(events[replay_begin].contains("command_count=1"));
}
