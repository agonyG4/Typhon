use super::*;

#[test]
fn dependency_free_replay_cache_telemetry_distinguishes_hits_and_full_updates() {
    let mut stats = EffectExecutionStats::default();
    stats.record_dependency_free_replay_cache_decision(false, true, false, 100, 100);
    stats.record_dependency_free_replay_cache_decision(true, false, true, 100, 0);
    stats.record_dependency_free_replay_cache_decision(true, true, false, 100, 100);

    assert_eq!(stats.checkpoint_cache_hits, 2);
    assert_eq!(stats.checkpoint_cache_full_refreshes, 1);
    assert_eq!(stats.checkpoint_cache_zero_copy_hits, 1);
    assert_eq!(stats.checkpoint_cache_domain_pixels, 300);
    assert_eq!(stats.checkpoint_cache_update_pixels, 200);
    assert_eq!(stats.checkpoint_cache_saved_pixels, 100);
    assert_eq!(stats.replay_capture_cache_hits, 2);
    assert_eq!(stats.replay_capture_cache_full_refreshes, 2);
    assert_eq!(stats.replay_capture_cache_zero_copy_hits, 1);
    assert_eq!(stats.replay_capture_cache_domain_pixels, 300);
    assert_eq!(stats.replay_capture_cache_update_pixels, 200);
    assert_eq!(stats.replay_capture_cache_saved_pixels, 100);
}

#[test]
fn eligible_churning_checkpoint_immediately_followed_by_first_kawase_plans_fusion() {
    let graph = fusion_test_graph();
    let capture = graph
        .passes
        .iter()
        .find(|pass| pass.kind == RenderPassKind::SceneCapture)
        .unwrap();
    let consumer = graph
        .passes
        .iter()
        .find(|pass| {
            pass.kind == RenderPassKind::DualKawaseDownsample && pass.instance == capture.instance
        })
        .unwrap();
    assert_eq!(graph.passes[1].id, consumer.id);
    let capture_texture = capture.output.unwrap();
    let mut selection = EffectExecutionSelection::default();
    selection.executed_passes = vec![capture.id, consumer.id];
    let mut preparations = std::collections::HashMap::new();
    preparations.insert(
        capture_texture,
        super::super::super::resources::CheckpointCapturePreparation {
            newly_admitted: true,
            identity_churn: true,
        },
    );
    let debug_config =
        EffectDebugConfig::from_env_values_with_checkpoint_capture_path(None, None, None);

    let plan = plan_capture_downsample_fusions(
        &graph,
        &selection,
        &preparations,
        true,
        (1920, 1080),
        OutputFramebufferOrigin::BottomLeft,
        SceneBaselineAuthority::ReplayRequired,
        debug_config,
    );

    assert_eq!(plan.candidates, 1);
    assert_eq!(plan.ineligible, 0);
    let fusion = plan.by_capture.get(&capture.id).unwrap();
    assert_eq!(fusion.consumer_pass, consumer.id);
    assert_eq!(fusion.capture_texture, capture_texture);
    assert_eq!(
        fusion.capture_domain,
        graph
            .textures
            .iter()
            .find(|texture| texture.id == capture_texture)
            .unwrap()
            .domain
    );
}

#[test]
fn capture_downsample_fusion_fails_closed_for_ineligible_cache_and_graph_cases() {
    let base = fusion_test_graph();
    let capture = base
        .passes
        .iter()
        .find(|pass| pass.kind == RenderPassKind::SceneCapture)
        .unwrap();
    let consumer = base
        .passes
        .iter()
        .find(|pass| {
            pass.kind == RenderPassKind::DualKawaseDownsample && pass.instance == capture.instance
        })
        .unwrap();
    let default_config =
        EffectDebugConfig::from_env_values_with_checkpoint_capture_path(None, None, None);

    for preparation in [
        CheckpointCapturePreparation {
            newly_admitted: true,
            identity_churn: false,
        },
        CheckpointCapturePreparation {
            newly_admitted: false,
            identity_churn: false,
        },
    ] {
        let plan = fusion_test_plan(
            &base,
            all_graph_passes_selected(&base),
            preparation,
            true,
            default_config,
        );
        assert!(plan.by_capture.is_empty());
    }

    let mut root_replay = base.clone();
    root_replay
        .passes
        .iter_mut()
        .find(|pass| pass.id == capture.id)
        .unwrap()
        .checkpoint_dependencies
        .clear();
    assert_fusion_ineligible(&root_replay);

    let blit_config = EffectDebugConfig::from_env_values_with_checkpoint_capture_path(
        None,
        None,
        Some(std::ffi::OsStr::new("blit")),
    );
    let blit = fusion_test_plan(
        &base,
        all_graph_passes_selected(&base),
        CheckpointCapturePreparation {
            newly_admitted: true,
            identity_churn: true,
        },
        true,
        blit_config,
    );
    assert!(blit.by_capture.is_empty());

    let no_output = fusion_test_plan(
        &base,
        all_graph_passes_selected(&base),
        CheckpointCapturePreparation {
            newly_admitted: true,
            identity_churn: true,
        },
        false,
        default_config,
    );
    assert!(no_output.by_capture.is_empty());

    let mut surface_capture = base.clone();
    surface_capture
        .passes
        .iter_mut()
        .find(|pass| pass.id == capture.id)
        .unwrap()
        .kind = RenderPassKind::SurfaceCapture;
    assert_fusion_ineligible(&surface_capture);

    let mut multiple_consumers = base.clone();
    let mut extra_consumer = consumer.clone();
    extra_consumer.id = GraphPassId::new(u16::MAX - 1).unwrap();
    multiple_consumers.passes.push(extra_consumer);
    assert_fusion_ineligible(&multiple_consumers);

    let mut different_instance = base.clone();
    different_instance
        .passes
        .iter_mut()
        .find(|pass| pass.id == consumer.id)
        .unwrap()
        .instance = oblivion_one::effects::EffectInstanceId::new(2).unwrap();
    assert_fusion_ineligible(&different_instance);

    let mut multiple_inputs = base.clone();
    multiple_inputs
        .passes
        .iter_mut()
        .find(|pass| pass.id == consumer.id)
        .unwrap()
        .inputs
        .push(GraphTextureId::new(u16::MAX - 4).unwrap());
    assert_fusion_ineligible(&multiple_inputs);

    let mut non_kawase = base.clone();
    non_kawase
        .passes
        .iter_mut()
        .find(|pass| pass.id == consumer.id)
        .unwrap()
        .kind = RenderPassKind::NormalizeInput;
    assert_fusion_ineligible(&non_kawase);

    let mut framebuffer_output = base.clone();
    let output = consumer.output.unwrap();
    framebuffer_output
        .textures
        .iter_mut()
        .find(|texture| texture.id == output)
        .unwrap()
        .source = GraphTextureSource::Output;
    assert_fusion_ineligible(&framebuffer_output);

    let mut non_first_kawase = base.clone();
    let first_downsample = test_pass(
        u16::MAX - 2,
        RenderPassKind::DualKawaseDownsample,
        capture.instance,
        Vec::new(),
        GraphTextureId::new(u16::MAX - 2).unwrap(),
        Vec::new(),
    );
    non_first_kawase.passes.insert(0, first_downsample);
    assert_fusion_ineligible(&non_first_kawase);

    let mut intervening_pass = base.clone();
    let capture_index = intervening_pass
        .passes
        .iter()
        .position(|pass| pass.id == capture.id)
        .unwrap();
    let unrelated_texture = intervening_pass
        .textures
        .iter()
        .find(|texture| texture.id != capture.output.unwrap())
        .unwrap()
        .id;
    let intervening = test_pass(
        u16::MAX - 3,
        RenderPassKind::NormalizeInput,
        capture.instance,
        vec![unrelated_texture],
        GraphTextureId::new(u16::MAX - 3).unwrap(),
        Vec::new(),
    );
    intervening_pass
        .passes
        .insert(capture_index + 1, intervening);
    assert_fusion_ineligible(&intervening_pass);

    let unselected_consumer = fusion_test_plan(
        &base,
        vec![capture.id],
        CheckpointCapturePreparation {
            newly_admitted: true,
            identity_churn: true,
        },
        true,
        default_config,
    );
    assert!(unselected_consumer.by_capture.is_empty());

    let unselected_capture = fusion_test_plan(
        &base,
        vec![consumer.id],
        CheckpointCapturePreparation {
            newly_admitted: true,
            identity_churn: true,
        },
        true,
        default_config,
    );
    assert!(unselected_capture.by_capture.is_empty());

    let mut scaled_capture = base.clone();
    let capture_texture = capture.output.unwrap();
    scaled_capture
        .textures
        .iter_mut()
        .find(|texture| texture.id == capture_texture)
        .unwrap()
        .width += 1;
    assert_fusion_ineligible(&scaled_capture);

    let mut wrong_source = base.clone();
    wrong_source
        .textures
        .iter_mut()
        .find(|texture| texture.id == capture.output.unwrap())
        .unwrap()
        .source = GraphTextureSource::CapturedTarget;
    assert_fusion_ineligible(&wrong_source);

    let mut wrong_working_space = base.clone();
    wrong_working_space
        .textures
        .iter_mut()
        .find(|texture| texture.id == capture.output.unwrap())
        .unwrap()
        .working_space = oblivion_one::effects::EffectWorkingSpace::LinearSrgb;
    assert_fusion_ineligible(&wrong_working_space);
}
