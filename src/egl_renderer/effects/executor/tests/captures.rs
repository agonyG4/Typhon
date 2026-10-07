use super::*;

#[test]
fn framebuffer_capture_orders_scene_advance_by_capture_policy() {
    let instance = oblivion_one::effects::EffectInstanceId::new(1).unwrap();
    let output = GraphTextureId::new(2).unwrap();
    let ordinary_capture = test_pass(
        1,
        RenderPassKind::SceneCapture,
        instance,
        Vec::new(),
        output,
        Vec::new(),
    );
    assert_eq!(
        scene_advance_reason(
            &ordinary_capture,
            SceneBaselineAuthority::ReplayRequired,
            true
        ),
        Some("framebuffer_capture")
    );
    assert_eq!(
        scene_advance_reason(
            &ordinary_capture,
            SceneBaselineAuthority::ReplayRequired,
            false
        ),
        None
    );
    assert_eq!(
        scene_advance_reason(
            &ordinary_capture,
            SceneBaselineAuthority::PrecomposedFramebuffer,
            false,
        ),
        None
    );

    let checkpoint_capture = test_pass(
        3,
        RenderPassKind::SceneCapture,
        instance,
        Vec::new(),
        output,
        vec![GraphPassId::new(2).unwrap()],
    );
    assert_eq!(
        scene_advance_reason(
            &checkpoint_capture,
            SceneBaselineAuthority::ReplayRequired,
            false,
        ),
        Some("checkpoint_dependency")
    );

    let surface_capture = test_pass(
        4,
        RenderPassKind::SurfaceCapture,
        instance,
        Vec::new(),
        output,
        Vec::new(),
    );
    assert_eq!(
        scene_advance_reason(
            &surface_capture,
            SceneBaselineAuthority::ReplayRequired,
            true
        ),
        None
    );
}

#[test]
fn lifecycle_builtin_background_blur_accepts_the_planned_precomposed_baseline() {
    let visible = oblivion_one::effects::EffectRect::new(700, 300, 220, 120).unwrap();
    let explicit_region = EffectRegion::from_rect(visible);
    let graph = compile_builtin_background_blur(
        explicit_region.clone(),
        &explicit_region,
        oblivion_one::effects::EffectRect::new(0, 0, 1920, 1080).unwrap(),
    );
    let capture = graph
        .passes
        .iter()
        .find(|pass| pass.kind == RenderPassKind::SceneCapture)
        .expect("background blur must start with a backdrop capture");
    let demand =
        oblivion_one::effects::plan_effect_execution_demand(&graph, &explicit_region, true);
    let selection = select_effect_execution(&graph, &demand);
    let repaint_rects = [OutputRect::new(700, 300, 220, 120)];
    let work = scene_replay_work_plan(
        &repaint_rects,
        &graph,
        &selection,
        (1920, 1080),
        SceneBaselineAuthority::PrecomposedFramebuffer,
        *effect_debug_config(),
    );
    let required = pass_output_texture_domain(&graph, capture);
    let planned_baseline = output_rects_to_effect_region(&work.baseline_work);

    assert!(is_direct_framebuffer_capture(
        capture,
        SceneBaselineAuthority::PrecomposedFramebuffer,
        *effect_debug_config()
    ));
    assert!(capture.checkpoint_dependencies.is_empty());
    assert!(!required.subtract(&explicit_region).is_empty());
    assert!(required.subtract(&planned_baseline).is_empty());
    assert!(!work.extra_scene_work.is_empty());

    // Before lifecycle execution had a precomposed baseline authority,
    // this first direct capture started with empty scene validity and
    // could not request a replay because it has no dependencies.
    let empty_baseline_validity = checkpoint_source_semantic_validity(
        &required,
        &EffectRegion::empty(),
        &EffectRegion::empty(),
        &[],
    );
    assert_eq!(empty_baseline_validity.missing, required);

    // Lifecycle capture already has this exact planner-owned domain in
    // the framebuffer; unrelated framebuffer pixels remain invalid.
    let scene_valid =
        initial_scene_valid_region(SceneBaselineAuthority::PrecomposedFramebuffer, &work);
    assert_eq!(scene_valid, planned_baseline);
    assert!(!scene_valid.contains_point(0, 0));
    let validity =
        checkpoint_source_semantic_validity(&required, &scene_valid, &EffectRegion::empty(), &[]);
    assert!(
        validity.missing.is_empty(),
        "precomposed backdrop pixels in the planner-owned domain must be valid: {:?}",
        validity.missing
    );
}

#[test]
fn lifecycle_blur_expansion_does_not_clear_the_authoritative_backdrop() {
    let visible = oblivion_one::effects::EffectRect::new(700, 300, 220, 120).unwrap();
    let explicit_region = EffectRegion::from_rect(visible);
    let graph = compile_builtin_background_blur(
        explicit_region.clone(),
        &explicit_region,
        oblivion_one::effects::EffectRect::new(0, 0, 1920, 1080).unwrap(),
    );
    let demand =
        oblivion_one::effects::plan_effect_execution_demand(&graph, &explicit_region, true);
    let selection = select_effect_execution(&graph, &demand);
    let work = scene_replay_work_plan(
        &[OutputRect::new(700, 300, 220, 120)],
        &graph,
        &selection,
        (1920, 1080),
        SceneBaselineAuthority::PrecomposedFramebuffer,
        *effect_debug_config(),
    );
    assert!(!work.extra_scene_work.is_empty());

    let reconstruct_internal_scene_work = should_reconstruct_scene_work(
        SceneBaselineAuthority::PrecomposedFramebuffer,
        false,
        !work.extra_scene_work.is_empty(),
    );
    assert!(
        !reconstruct_internal_scene_work,
        "expanded blur work must preserve the already-composed framebuffer baseline"
    );
    assert!(should_reconstruct_scene_work(
        SceneBaselineAuthority::ReplayRequired,
        false,
        !work.extra_scene_work.is_empty(),
    ));
    assert!(should_reconstruct_scene_work(
        SceneBaselineAuthority::ReplayRequired,
        true,
        false,
    ));
    assert!(initial_scene_valid_region(SceneBaselineAuthority::ReplayRequired, &work,).is_empty());
}

#[test]
fn precomposed_base_scene_does_not_satisfy_missing_effect_dependencies() {
    let required =
        EffectRegion::from_rect(oblivion_one::effects::EffectRect::new(100, 100, 20, 20).unwrap());
    let validity = checkpoint_source_semantic_validity(
        &required,
        &required,
        &required,
        &[(required.clone(), EffectRegion::empty())],
    );

    assert_eq!(validity.missing, required);
}

#[test]
fn checkpoint_capture_execution_plan_uses_production_default_when_output_is_sampleable() {
    let config = EffectDebugConfig::from_env_values_with_checkpoint_capture_path(None, None, None);
    let plan = checkpoint_capture_execution_plan(
        RenderPassKind::SceneCapture,
        1,
        SceneBaselineAuthority::ReplayRequired,
        config.capture_mode(),
        config.checkpoint_capture_path(),
        true,
    );

    assert_eq!(
        plan.requested,
        Some(CheckpointCapturePath::FramebufferShaderCopy)
    );
    assert_eq!(plan.executed, CaptureTimingMode::FramebufferShaderCopy);
    assert_eq!(plan.fallback_reason, None);
}

#[test]
fn checkpoint_capture_execution_plan_production_default_falls_back_without_output_texture() {
    let config = EffectDebugConfig::from_env_values_with_checkpoint_capture_path(None, None, None);
    let plan = checkpoint_capture_execution_plan(
        RenderPassKind::SceneCapture,
        1,
        SceneBaselineAuthority::ReplayRequired,
        config.capture_mode(),
        config.checkpoint_capture_path(),
        false,
    );

    assert_eq!(
        plan.requested,
        Some(CheckpointCapturePath::FramebufferShaderCopy)
    );
    assert_eq!(
        plan.executed,
        CaptureTimingMode::FramebufferBlit,
        "shader-copy must fall back to framebuffer blit when the output is not sampleable"
    );
    assert_eq!(
        plan.fallback_reason,
        Some(CapturePathFallbackReason::NoSampleableOutputTexture)
    );
}

#[test]
fn checkpoint_capture_execution_plan_honors_explicit_blit_with_sampleable_output() {
    let config = EffectDebugConfig::from_env_values_with_checkpoint_capture_path(
        None,
        None,
        Some(std::ffi::OsStr::new("blit")),
    );
    let plan = checkpoint_capture_execution_plan(
        RenderPassKind::SceneCapture,
        1,
        SceneBaselineAuthority::ReplayRequired,
        config.capture_mode(),
        config.checkpoint_capture_path(),
        true,
    );

    assert_eq!(plan.requested, Some(CheckpointCapturePath::FramebufferBlit));
    assert_eq!(plan.executed, CaptureTimingMode::FramebufferBlit);
    assert_eq!(plan.fallback_reason, None);
}

#[test]
fn checkpoint_capture_production_default_does_not_widen_other_capture_paths() {
    let replay_default =
        EffectDebugConfig::from_env_values_with_checkpoint_capture_path(None, None, None);
    let framebuffer_debug = EffectDebugConfig::from_env_values_with_checkpoint_capture_path(
        Some(std::ffi::OsStr::new("framebuffer")),
        None,
        None,
    );
    let cases = [
        (
            RenderPassKind::SceneCapture,
            0,
            SceneBaselineAuthority::ReplayRequired,
            replay_default,
            CaptureTimingMode::Replay,
        ),
        (
            RenderPassKind::SceneCapture,
            0,
            SceneBaselineAuthority::PrecomposedFramebuffer,
            replay_default,
            CaptureTimingMode::FramebufferBlit,
        ),
        (
            RenderPassKind::SceneCapture,
            0,
            SceneBaselineAuthority::ReplayRequired,
            framebuffer_debug,
            CaptureTimingMode::FramebufferBlit,
        ),
        (
            RenderPassKind::SurfaceCapture,
            1,
            SceneBaselineAuthority::ReplayRequired,
            replay_default,
            CaptureTimingMode::FramebufferBlit,
        ),
    ];

    for (kind, checkpoint_count, scene_baseline_authority, config, expected_mode) in cases {
        let plan = checkpoint_capture_execution_plan(
            kind,
            checkpoint_count,
            scene_baseline_authority,
            config.capture_mode(),
            config.checkpoint_capture_path(),
            true,
        );

        assert_eq!(plan.requested, None);
        assert_eq!(plan.executed, expected_mode);
        assert_eq!(plan.fallback_reason, None);
    }
}

#[test]
fn capture_timing_metadata_uses_execution_authority() {
    let instance = oblivion_one::effects::EffectInstanceId::new(1).unwrap();
    let output = GraphTextureId::new(2).unwrap();
    let replay_pass = test_pass(
        1,
        RenderPassKind::SceneCapture,
        instance,
        Vec::new(),
        output,
        Vec::new(),
    );
    let replay_plan = checkpoint_capture_execution_plan(
        replay_pass.kind,
        replay_pass.checkpoint_dependencies.len(),
        SceneBaselineAuthority::ReplayRequired,
        EffectDebugCaptureMode::Replay,
        CheckpointCapturePath::FramebufferBlit,
        false,
    );
    let replay_metadata =
        capture_timing_metadata(&replay_pass, replay_plan).expect("capture metadata");
    assert_eq!(replay_metadata.mode, CaptureTimingMode::Replay);
    assert_eq!(replay_metadata.checkpoint_count, 0);

    let checkpoint_pass = test_pass(
        3,
        RenderPassKind::SceneCapture,
        instance,
        Vec::new(),
        output,
        vec![GraphPassId::new(2).unwrap()],
    );
    let checkpoint_plan = checkpoint_capture_execution_plan(
        checkpoint_pass.kind,
        checkpoint_pass.checkpoint_dependencies.len(),
        SceneBaselineAuthority::ReplayRequired,
        EffectDebugCaptureMode::Replay,
        CheckpointCapturePath::FramebufferBlit,
        false,
    );
    let checkpoint_metadata = capture_timing_metadata(&checkpoint_pass, checkpoint_plan)
        .expect("checkpoint capture metadata");
    assert_eq!(checkpoint_metadata.mode, CaptureTimingMode::FramebufferBlit);
    assert_eq!(checkpoint_metadata.checkpoint_count, 1);
}

#[test]
fn framebuffer_scene_work_includes_selected_backdrop_capture_domains() {
    let instance = oblivion_one::effects::EffectInstanceId::new(1).unwrap();
    let capture_id = GraphTextureId::new(1).unwrap();
    let capture_domain = oblivion_one::effects::EffectRect::new(40, 30, 60, 50).unwrap();
    let pass = test_pass(
        1,
        RenderPassKind::SceneCapture,
        instance,
        Vec::new(),
        capture_id,
        Vec::new(),
    );
    let graph = CompiledFrameGraph {
        passes: vec![pass.clone()],
        textures: vec![test_texture(
            1,
            GraphTextureSource::CapturedScene,
            capture_domain,
        )],
        instances: Vec::new(),
        final_damage: EffectRegion::empty(),
        stats: Default::default(),
    };
    let selection = EffectExecutionSelection {
        executed_passes: vec![pass.id],
        ..EffectExecutionSelection::default()
    };
    let config = EffectDebugConfig::new(
        EffectDebugCaptureMode::Framebuffer,
        EffectDebugKawaseMode::Partial,
    );
    let regions = scene_replay_work_plan(
        &[OutputRect::new(5, 6, 7, 8)],
        &graph,
        &selection,
        (200, 150),
        SceneBaselineAuthority::ReplayRequired,
        config,
    );
    let work = &regions.baseline_work;

    assert!(work.contains(&OutputRect::new(5, 6, 7, 8)));
    assert!(work.contains(&OutputRect::new(40, 30, 60, 50)));
    assert!(!regions.extra_scene_work.is_empty());
    assert!(
        regions
            .extra_scene_work
            .iter()
            .all(|rect| { subtract_output_rect(*rect, OutputRect::new(5, 6, 7, 8)).len() == 1 })
    );
    assert!(work.len() <= MAX_EFFECT_REGION_RECTS);

    let coalesced = scene_replay_work_plan(
        &[OutputRect::new(5, 6, 50, 40)],
        &graph,
        &selection,
        (200, 150),
        SceneBaselineAuthority::ReplayRequired,
        config,
    )
    .baseline_work;
    assert_eq!(coalesced.len(), 3);
    for (index, first) in coalesced.iter().enumerate() {
        for second in coalesced.iter().skip(index + 1) {
            assert!(
                subtract_output_rect(*first, *second).len() == 1,
                "scene-work scissors must not overlap: {first:?} and {second:?}"
            );
        }
    }
}

#[test]
fn replay_scene_work_includes_selected_checkpoint_framebuffer_domains() {
    let instance = oblivion_one::effects::EffectInstanceId::new(1).unwrap();
    let capture_id = GraphTextureId::new(1).unwrap();
    let checkpoint_dependency = GraphPassId::new(9).unwrap();
    let capture_domain = oblivion_one::effects::EffectRect::new(762, 976, 396, 104).unwrap();
    let pass = test_pass(
        1,
        RenderPassKind::SceneCapture,
        instance,
        Vec::new(),
        capture_id,
        vec![checkpoint_dependency],
    );
    let graph = CompiledFrameGraph {
        passes: vec![pass.clone()],
        textures: vec![test_texture(
            1,
            GraphTextureSource::CapturedScene,
            capture_domain,
        )],
        instances: Vec::new(),
        final_damage: EffectRegion::empty(),
        stats: Default::default(),
    };
    let selection = EffectExecutionSelection {
        executed_passes: vec![pass.id],
        ..EffectExecutionSelection::default()
    };
    let config = EffectDebugConfig::new(
        EffectDebugCaptureMode::Replay,
        EffectDebugKawaseMode::Partial,
    );

    let regions = scene_replay_work_plan(
        &[OutputRect::new(24, 20, 8, 8)],
        &graph,
        &selection,
        (1920, 1080),
        SceneBaselineAuthority::ReplayRequired,
        config,
    );

    assert!(
        regions
            .baseline_work
            .iter()
            .any(|rect| rect.x <= capture_domain.x
                && rect.y <= capture_domain.y
                && rect.x + rect.width as i32 >= capture_domain.right()
                && rect.y + rect.height as i32 >= capture_domain.bottom()),
        "replay checkpoint capture must expand internal scene work to its exact source domain: {:?}",
        regions.baseline_work
    );
    assert!(!regions.extra_scene_work.is_empty());
}

#[test]
fn replay_scene_work_includes_topbar_checkpoint_framebuffer_domain() {
    let instance = oblivion_one::effects::EffectInstanceId::new(1).unwrap();
    let capture_id = GraphTextureId::new(1).unwrap();
    let checkpoint_dependency = GraphPassId::new(9).unwrap();
    let capture_domain = oblivion_one::effects::EffectRect::new(0, 0, 120, 65).unwrap();
    let pass = test_pass(
        1,
        RenderPassKind::SceneCapture,
        instance,
        Vec::new(),
        capture_id,
        vec![checkpoint_dependency],
    );
    let graph = CompiledFrameGraph {
        passes: vec![pass.clone()],
        textures: vec![test_texture(
            1,
            GraphTextureSource::CapturedScene,
            capture_domain,
        )],
        instances: Vec::new(),
        final_damage: EffectRegion::empty(),
        stats: Default::default(),
    };
    let selection = EffectExecutionSelection {
        executed_passes: vec![pass.id],
        ..EffectExecutionSelection::default()
    };
    let regions = scene_replay_work_plan(
        &[OutputRect::new(10, 10, 4, 4)],
        &graph,
        &selection,
        (1920, 1080),
        SceneBaselineAuthority::ReplayRequired,
        EffectDebugConfig::new(
            EffectDebugCaptureMode::Replay,
            EffectDebugKawaseMode::Partial,
        ),
    );

    assert!(regions.baseline_work.iter().any(|rect| {
        rect.x <= capture_domain.x
            && rect.y <= capture_domain.y
            && rect.x + rect.width as i32 >= capture_domain.right()
            && rect.y + rect.height as i32 >= capture_domain.bottom()
    }));
    assert!(!regions.extra_scene_work.is_empty());
}

#[test]
fn surface_consumer_plan_matches_scene_replay_work_state() {
    let instance_a = oblivion_one::effects::EffectInstanceId::new(1).unwrap();
    let instance_b = oblivion_one::effects::EffectInstanceId::new(2).unwrap();
    let output_a = GraphTextureId::new(1).unwrap();
    let output_b = GraphTextureId::new(2).unwrap();
    let mut capture_a = test_pass(
        1,
        RenderPassKind::SceneCapture,
        instance_a,
        Vec::new(),
        output_a,
        vec![GraphPassId::new(90).unwrap()],
    );
    capture_a.anchor = oblivion_one::compositor::EffectAnchor::BeforeSurface(20);
    capture_a.anchor_scope = oblivion_one::compositor::EffectAnchorScope::Surface;
    let mut capture_b = test_pass(
        2,
        RenderPassKind::SurfaceCapture,
        instance_b,
        Vec::new(),
        output_b,
        vec![GraphPassId::new(91).unwrap()],
    );
    capture_b.anchor = oblivion_one::compositor::EffectAnchor::BeforeSurface(30);
    capture_b.anchor_scope = oblivion_one::compositor::EffectAnchorScope::Surface;
    let graph = CompiledFrameGraph {
        passes: vec![capture_a.clone(), capture_b.clone()],
        textures: vec![
            test_texture(
                1,
                GraphTextureSource::CapturedScene,
                oblivion_one::effects::EffectRect::new(20, 0, 10, 10).unwrap(),
            ),
            test_texture(
                2,
                GraphTextureSource::CapturedTarget,
                oblivion_one::effects::EffectRect::new(40, 0, 10, 10).unwrap(),
            ),
        ],
        instances: Vec::new(),
        final_damage: EffectRegion::empty(),
        stats: Default::default(),
    };
    let selection = EffectExecutionSelection {
        executed_passes: vec![capture_a.id, capture_b.id],
        ..EffectExecutionSelection::default()
    };
    let commands = vec![
        EglDrawCommand {
            layer: EglDrawLayer::SolidRgba(0xff10_2030),
            visual_group: None,
            bounds: EglRect::new(0.0, 0.0, 10.0, 10.0),
            opaque_regions: Vec::new(),
            presentation_clip: None,
            vertex_start: 0,
            vertex_count: 6,
            sampling: SurfaceSampling::ExactNearest,
        },
        EglDrawCommand {
            layer: EglDrawLayer::Surface(20),
            visual_group: None,
            bounds: EglRect::new(20.0, 0.0, 10.0, 10.0),
            opaque_regions: Vec::new(),
            presentation_clip: None,
            vertex_start: 0,
            vertex_count: 6,
            sampling: SurfaceSampling::ExactNearest,
        },
        EglDrawCommand {
            layer: EglDrawLayer::Surface(30),
            visual_group: None,
            bounds: EglRect::new(40.0, 0.0, 10.0, 10.0),
            opaque_regions: Vec::new(),
            presentation_clip: None,
            vertex_start: 0,
            vertex_count: 6,
            sampling: SurfaceSampling::ExactNearest,
        },
    ];
    let demand = EffectExecutionDemand::new(Vec::new(), EffectRegion::empty());
    let plan = scene_replay_work_plan(
        &[OutputRect::new(0, 0, 10, 10)],
        &graph,
        &selection,
        (100, 100),
        SceneBaselineAuthority::ReplayRequired,
        EffectDebugConfig::new(
            EffectDebugCaptureMode::Replay,
            EffectDebugKawaseMode::Partial,
        ),
    );
    let mut expected_state = SceneReplayWorkState::new(&plan, SceneReplayWorkMode::SuffixDemand);
    expected_state.mark_capture_satisfied(capture_a.id);
    assert_same_output_region(
        expected_state.active_work(),
        &[
            OutputRect::new(0, 0, 10, 10),
            OutputRect::new(40, 0, 10, 10),
        ],
    );

    let consumer_plan = plan_effect_surface_consumers_with_debug_config(
        &graph,
        &demand,
        &selection,
        &commands,
        &[OutputRect::new(0, 0, 10, 10)],
        (100, 100),
        EffectDebugConfig::new(
            EffectDebugCaptureMode::Replay,
            EffectDebugKawaseMode::Partial,
        ),
    );
    assert!(consumer_plan.surface_ids().contains(&30));
    assert!(
        !consumer_plan.surface_ids().contains(&20),
        "surface A must not be planned after its checkpoint has retired"
    );
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "pending scene checkpoint requirements")]
fn surface_consumer_plan_trailing_pending_requirements_expose_invariant_failure() {
    let presentation = OutputRect::new(0, 0, 10, 10);
    let checkpoint_a = OutputRect::new(20, 0, 10, 10);
    let checkpoint_b = OutputRect::new(40, 0, 10, 10);
    let requirements = [
        test_checkpoint_requirement(1, &[checkpoint_a]),
        test_checkpoint_requirement(2, &[checkpoint_b]),
    ];
    let plan = test_scene_replay_plan(&[presentation], &requirements);
    let mut state = SceneReplayWorkState::new(&plan, SceneReplayWorkMode::SuffixDemand);
    state.mark_capture_satisfied(GraphPassId::new(1).unwrap());

    let _ = finalize_surface_consumer_trailing_work(&mut state);
}

#[cfg(not(debug_assertions))]
#[test]
fn surface_consumer_plan_trailing_pending_requirements_use_baseline_work() {
    let presentation = OutputRect::new(0, 0, 10, 10);
    let checkpoint_a = OutputRect::new(20, 0, 10, 10);
    let checkpoint_b = OutputRect::new(40, 0, 10, 10);
    let requirements = [
        test_checkpoint_requirement(1, &[checkpoint_a]),
        test_checkpoint_requirement(2, &[checkpoint_b]),
    ];
    let plan = test_scene_replay_plan(&[presentation], &requirements);
    let mut state = SceneReplayWorkState::new(&plan, SceneReplayWorkMode::SuffixDemand);
    state.mark_capture_satisfied(GraphPassId::new(1).unwrap());

    let trailing = finalize_surface_consumer_trailing_work(&mut state);

    assert_same_output_region(&trailing, &[presentation, checkpoint_a, checkpoint_b]);

    let command = |surface_id, rect: OutputRect| EglDrawCommand {
        layer: EglDrawLayer::Surface(surface_id),
        visual_group: None,
        bounds: EglRect::new(
            rect.x as f32,
            rect.y as f32,
            rect.width as f32,
            rect.height as f32,
        ),
        opaque_regions: Vec::new(),
        presentation_clip: None,
        vertex_start: 0,
        vertex_count: 6,
        sampling: SurfaceSampling::ExactNearest,
    };
    let commands = vec![
        command(10, presentation),
        command(20, checkpoint_a),
        command(30, checkpoint_b),
    ];
    let mut trailing_plan = SurfaceConsumerPlan::default();
    add_surface_consumers_for_command_range(
        &mut trailing_plan,
        &commands,
        0,
        commands.len(),
        &trailing,
    );
    trailing_plan.finish();
    assert_eq!(
        trailing_plan.surface_ids(),
        &[10, 20, 30],
        "pending checkpoint requirements must keep every baseline surface in the trailing plan"
    );
}

#[test]
fn checkpoint_source_validity_uses_exact_regions_not_bounding_boxes() {
    let required =
        EffectRegion::from_rect(oblivion_one::effects::EffectRect::new(100, 100, 20, 20).unwrap());
    let valid =
        EffectRegion::from_rect(oblivion_one::effects::EffectRect::new(100, 100, 20, 8).unwrap())
            .union(&EffectRegion::from_rect(
                oblivion_one::effects::EffectRect::new(100, 112, 20, 8).unwrap(),
            ));

    let validity = checkpoint_source_validity(&required, &valid);

    assert_eq!(validity.required_rect_count, 1);
    assert_eq!(validity.valid_rect_count, 2);
    assert_eq!(validity.required_bounding_box, Some((100, 100, 20, 20)));
    assert_eq!(validity.valid_bounding_box, Some((100, 100, 20, 20)));
    assert_eq!(
        validity.missing.rects(),
        &[oblivion_one::effects::EffectRect::new(100, 108, 20, 4).unwrap()]
    );
    assert_eq!(effect_region_pixels(&validity.missing), 80);
}

#[test]
fn checkpoint_semantic_validity_does_not_mask_dependency_gaps_with_base_scene() {
    let required = EffectRegion::from_rect(
        oblivion_one::effects::EffectRect::new(762, 976, 396, 104).unwrap(),
    );
    let dependency_influence = EffectRegion::from_rect(
        oblivion_one::effects::EffectRect::new(786, 1000, 348, 56).unwrap(),
    );
    let partial_dependency_output =
        EffectRegion::from_rect(oblivion_one::effects::EffectRect::new(900, 1010, 8, 8).unwrap());
    let validity = checkpoint_source_semantic_validity(
        &required,
        &required.subtract(&dependency_influence),
        &dependency_influence,
        &[(dependency_influence.clone(), partial_dependency_output)],
    );

    assert!(!validity.missing.is_empty());
    assert!(validity.missing.contains_point(800, 1004));
    assert!(!validity.missing.contains_point(902, 1012));
}

#[test]
fn checkpoint_semantic_validity_does_not_mask_missing_overlapping_dependency() {
    let required =
        EffectRegion::from_rect(oblivion_one::effects::EffectRect::new(100, 100, 20, 20).unwrap());
    let dependency_a_influence = required.clone();
    let dependency_c_influence = required.clone();
    let dependency_a_valid = required.clone();
    let dependency_c_valid = EffectRegion::empty();
    let dependency_influence = dependency_a_influence
        .union(&dependency_c_influence)
        .intersect(&required);
    let validity = checkpoint_source_semantic_validity(
        &required,
        &EffectRegion::empty(),
        &dependency_influence,
        &[
            (dependency_a_influence, dependency_a_valid),
            (dependency_c_influence, dependency_c_valid),
        ],
    );

    assert!(validity.missing.contains_point(110, 110));
    assert_eq!(effect_region_pixels(&validity.missing), 400);
}

#[test]
fn checkpoint_semantic_validity_accepts_fully_valid_overlapping_dependencies() {
    let required =
        EffectRegion::from_rect(oblivion_one::effects::EffectRect::new(100, 100, 20, 20).unwrap());
    let dependency_a_influence = required.clone();
    let dependency_c_influence = required.clone();
    let dependency_a_valid = required.clone();
    let dependency_c_valid = required.clone();
    let dependency_influence = dependency_a_influence
        .union(&dependency_c_influence)
        .intersect(&required);
    let validity = checkpoint_source_semantic_validity(
        &required,
        &EffectRegion::empty(),
        &dependency_influence,
        &[
            (dependency_a_influence, dependency_a_valid),
            (dependency_c_influence, dependency_c_valid),
        ],
    );

    assert!(validity.missing.is_empty());
}

#[test]
fn checkpoint_semantic_validity_accepts_disjoint_dependency_coverage() {
    let required =
        EffectRegion::from_rect(oblivion_one::effects::EffectRect::new(100, 100, 40, 20).unwrap());
    let dependency_a_influence =
        EffectRegion::from_rect(oblivion_one::effects::EffectRect::new(100, 100, 20, 20).unwrap());
    let dependency_c_influence =
        EffectRegion::from_rect(oblivion_one::effects::EffectRect::new(120, 100, 20, 20).unwrap());
    let dependency_influence = dependency_a_influence
        .union(&dependency_c_influence)
        .intersect(&required);
    let validity = checkpoint_source_semantic_validity(
        &required,
        &EffectRegion::empty(),
        &dependency_influence,
        &[
            (dependency_a_influence.clone(), dependency_a_influence),
            (dependency_c_influence.clone(), dependency_c_influence),
        ],
    );

    assert!(validity.missing.is_empty());
}

#[test]
fn checkpoint_dependency_region_requires_prior_effect_influence_coverage() {
    let earlier = oblivion_one::effects::EffectInstanceId::new(1).unwrap();
    let later = oblivion_one::effects::EffectInstanceId::new(2).unwrap();
    let earlier_output = GraphTextureId::new(1).unwrap();
    let later_output = GraphTextureId::new(2).unwrap();
    let earlier_pass = test_pass(
        9,
        RenderPassKind::SceneCapture,
        earlier,
        Vec::new(),
        earlier_output,
        Vec::new(),
    );
    let later_pass = test_pass(
        10,
        RenderPassKind::SceneCapture,
        later,
        Vec::new(),
        later_output,
        vec![earlier_pass.id],
    );
    let earlier_influence = oblivion_one::effects::EffectRect::new(786, 1000, 348, 56).unwrap();
    let later_capture = oblivion_one::effects::EffectRect::new(762, 976, 396, 104).unwrap();
    let graph = CompiledFrameGraph {
        passes: vec![earlier_pass, later_pass.clone()],
        textures: vec![
            test_texture(1, GraphTextureSource::CapturedScene, earlier_influence),
            test_texture(2, GraphTextureSource::CapturedScene, later_capture),
        ],
        instances: vec![
            oblivion_one::effects::CompiledEffectInstance {
                semantic_signature: 0,
                frame_demand: oblivion_one::effects::EffectFrameDemand::OnDamage,
                id: earlier,
                output_influence_region: EffectRegion::from_rect(earlier_influence),
                capture_region: EffectRegion::from_rect(earlier_influence),
                dependencies: Vec::new(),
            },
            oblivion_one::effects::CompiledEffectInstance {
                semantic_signature: 0,
                frame_demand: oblivion_one::effects::EffectFrameDemand::OnDamage,
                id: later,
                output_influence_region: EffectRegion::from_rect(later_capture),
                capture_region: EffectRegion::from_rect(later_capture),
                dependencies: vec![earlier],
            },
        ],
        final_damage: EffectRegion::empty(),
        stats: Default::default(),
    };

    assert_eq!(
        checkpoint_dependency_influence_region(
            &graph,
            &graph.passes[1],
            &EffectRegion::from_rect(later_capture),
        ),
        EffectRegion::from_rect(earlier_influence)
    );
}

#[test]
fn effect_pass_blend_modes_are_explicit_for_each_pass_family() {
    for kind in [
        RenderPassKind::NormalizeInput,
        RenderPassKind::DualKawaseDownsample,
        RenderPassKind::DualKawaseUpsample,
        RenderPassKind::Fragment,
        RenderPassKind::Blend,
        RenderPassKind::Mask,
    ] {
        assert_eq!(
            effect_pass_blend_mode(
                kind,
                false,
                oblivion_one::effects::EffectAlphaMode::Preserve,
                1.0,
                false,
            ),
            EffectPassBlendMode::Replace,
            "internal pass {kind:?} must replace its target"
        );
    }
    assert_eq!(
        capture_blend_mode(),
        EffectPassBlendMode::PremultipliedSourceOver
    );
    assert_eq!(
        effect_pass_blend_mode(
            RenderPassKind::Composite,
            true,
            oblivion_one::effects::EffectAlphaMode::Opaque,
            1.0,
            false,
        ),
        EffectPassBlendMode::Replace
    );
    assert_eq!(
        effect_pass_blend_mode(
            RenderPassKind::OutputPostProcess,
            true,
            oblivion_one::effects::EffectAlphaMode::Preserve,
            1.0,
            false,
        ),
        EffectPassBlendMode::PremultipliedSourceOver
    );
    for opacity in [0.5, 0.0] {
        assert_eq!(
            effect_pass_blend_mode(
                RenderPassKind::Composite,
                true,
                oblivion_one::effects::EffectAlphaMode::Opaque,
                opacity,
                false,
            ),
            EffectPassBlendMode::PremultipliedSourceOver,
            "opaque final output with opacity {opacity} must source-over"
        );
    }
    assert_eq!(
        effect_pass_blend_mode(
            RenderPassKind::Composite,
            true,
            oblivion_one::effects::EffectAlphaMode::Opaque,
            1.0,
            true,
        ),
        EffectPassBlendMode::PremultipliedSourceOver
    );
}

#[test]
fn builtin_background_blur_uses_source_over_below_opaque_owner_opacity() {
    let registry = oblivion_one::effects::EffectRegistry::with_builtin_background_blur();
    let program = registry
        .get(oblivion_one::effects::builtin_background_blur_program_id())
        .expect("builtin background blur must be registered");
    assert_eq!(
        program.program.alpha_mode,
        oblivion_one::effects::EffectAlphaMode::Opaque
    );
    assert_eq!(
        effect_pass_blend_mode(
            RenderPassKind::Composite,
            true,
            program.program.alpha_mode,
            0.5,
            false,
        ),
        EffectPassBlendMode::PremultipliedSourceOver
    );
    assert_eq!(
        effect_pass_blend_mode(
            RenderPassKind::Composite,
            true,
            program.program.alpha_mode,
            1.0,
            false,
        ),
        EffectPassBlendMode::Replace
    );
}

#[test]
fn capture_materialization_plan_keeps_region_and_raster_rects_authoritative() {
    let domain = oblivion_one::effects::EffectRect::new(100, 80, 120, 90).unwrap();
    let mut execution =
        EffectRegion::from_rect(oblivion_one::effects::EffectRect::new(108, 88, 12, 10).unwrap());
    execution.push(oblivion_one::effects::EffectRect::new(180, 132, 8, 7).unwrap());

    let exact = capture_materialization_plan(&execution, Some(domain), (320, 240));
    assert_eq!(exact.region, execution);
    assert_eq!(exact.output_rects.len(), 2);
    let target = test_texture(1, GraphTextureSource::CapturedScene, domain);
    assert_eq!(
        exact.texture_rects(&target),
        vec![
            OutputRect::new(8, 72, 12, 10),
            OutputRect::new(80, 31, 8, 7)
        ]
    );
}

#[test]
fn uncached_direct_shader_copy_keeps_full_domain_materialization() {
    let domain = oblivion_one::effects::EffectRect::new(100, 80, 120, 90).unwrap();
    let damage =
        EffectRegion::from_rect(oblivion_one::effects::EffectRect::new(108, 88, 12, 10).unwrap());
    let partial = capture_materialization_plan(&damage, Some(domain), (320, 240));
    let target = test_texture(1, GraphTextureSource::CapturedScene, domain);

    assert_eq!(
        materialized_target_rects(true, false, Some(&partial), &partial.output_rects, &target),
        vec![full_output_rect((target.width, target.height))],
        "ordinary direct captures must remain full-domain when caching is unavailable"
    );
    assert_eq!(
        materialized_target_rects(true, true, Some(&partial), &partial.output_rects, &target),
        partial.texture_rects(&target),
        "persistent checkpoints may use the bounded dirty rectangles"
    );
}

#[test]
fn checkpoint_update_materialization_keeps_disjoint_damage_and_empty_hits() {
    let domain = oblivion_one::effects::EffectRect::new(100, 80, 64, 40).unwrap();
    let first = oblivion_one::effects::EffectRect::new(102, 83, 8, 6).unwrap();
    let second = oblivion_one::effects::EffectRect::new(130, 100, 5, 7).unwrap();
    let damage = EffectRegion::from_rect(first).union(&EffectRegion::from_rect(second));

    let update = checkpoint_update_materialization_plan(&damage, domain, (320, 240));
    let target = test_texture(1, GraphTextureSource::CapturedScene, domain);

    assert_eq!(
        update.output_rects,
        vec![
            OutputRect::new(102, 83, 8, 6),
            OutputRect::new(130, 100, 5, 7),
        ]
    );
    assert_eq!(output_rect_pixels(&update.output_rects), 83);
    assert_eq!(
        update.texture_rects(&target),
        vec![OutputRect::new(2, 31, 8, 6), OutputRect::new(30, 13, 5, 7)]
    );
    assert!(update.output_rects.iter().all(|rect| {
        let right = i64::from(rect.x) + i64::from(rect.width);
        let bottom = i64::from(rect.y) + i64::from(rect.height);
        !(i64::from(rect.x) < 135 && right > 110 && i64::from(rect.y) < 100 && bottom > 89)
    }));

    let empty = checkpoint_update_materialization_plan(&EffectRegion::empty(), domain, (320, 240));
    assert!(empty.region.is_empty());
    assert!(empty.output_rects.is_empty());
    assert!(empty.texture_rects(&target).is_empty());
}

#[test]
fn replay_capture_region_layout_reports_materialization_and_execution_regions() {
    let output_rects = [
        OutputRect::new(10, 20, 30, 40),
        OutputRect::new(80, 90, 12, 14),
    ];

    let layout = replay_capture_region_layout(&output_rects);

    assert_eq!(layout.materialization_rects, 2);
    assert_eq!(layout.execution_regions, 2);
    assert!(!layout.disjoint_overflowed);
    assert_eq!(layout.execution_region.rects().len(), 2);
}

#[test]
fn replay_capture_region_layout_reports_bounded_overflow_and_bbox_fallback() {
    let mut output_rects = Vec::new();
    for index in 0..oblivion_one::effects::MAX_EFFECT_REGION_RECTS.saturating_sub(1) {
        output_rects.push(OutputRect::new(0, index as i32 * 2, 1_000, 1));
    }
    output_rects.push(OutputRect::new(
        0,
        0,
        1_000,
        (oblivion_one::effects::MAX_EFFECT_REGION_RECTS as u32)
            .saturating_mul(2)
            .saturating_sub(1),
    ));

    let layout = replay_capture_region_layout(&output_rects);

    assert_eq!(layout.materialization_rects, output_rects.len());
    assert!(layout.disjoint_overflowed);
    assert_eq!(layout.execution_regions, 1);
    assert_eq!(layout.execution_region.rects().len(), 1);
    assert_eq!(
        layout.execution_region.bounding_rect(),
        Some(
            oblivion_one::effects::EffectRect::new(
                0,
                0,
                1_000,
                (oblivion_one::effects::MAX_EFFECT_REGION_RECTS as u32)
                    .saturating_mul(2)
                    .saturating_sub(1),
            )
            .unwrap(),
        )
    );
}
