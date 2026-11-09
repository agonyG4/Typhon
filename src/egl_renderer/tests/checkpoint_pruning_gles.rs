use super::*;
use oblivion_one::compositor::{EffectAnchorScope, ResolvedEffectScene};
use oblivion_one::effects::{
    EffectDemandPlannerPolicy, EffectInstanceId, EffectRect, EffectRegion, EffectRegistry,
    FrameExecutionCompileOptions, FrameExecutionPlan, RenderPassKind,
    builtin_background_material_program_with_passes, compile_frame_execution_variants_with_options,
};
use oblivion_one::material::MaterialConfiguration;

fn pruning_fixture(second_x: i32) -> NativeThreeCheckpointFixture {
    let mut fixture = native_three_checkpoint_fixture();
    fixture.output_size = (800, 500);
    fixture.output_bounds = EffectRect::new(0, 0, 800, 500).unwrap();
    fixture.repair = OutputRect::new(160, 110, 40, 70);

    let a_bounds = EffectRect::new(100, 110, 100, 70).unwrap();
    let c_bounds = EffectRect::new(second_x, 110, 100, 70).unwrap();
    let b_bounds = EffectRect::new(500, 110, 100, 70).unwrap();
    fixture.effect_a.target_bounds = a_bounds;
    fixture.effect_c.target_bounds = c_bounds;
    fixture.effect_b.target_bounds = b_bounds;
    fixture.effect_a.anchor_scope = EffectAnchorScope::VisualGroup;
    fixture.effect_c.anchor_scope = EffectAnchorScope::VisualGroup;
    fixture.effect_b.anchor_scope = EffectAnchorScope::VisualGroup;
    fixture.scene.a_content_bounds = EffectRect::new(100, 110, 60, 70).unwrap();
    fixture.scene.c_content_bounds = c_bounds;
    fixture.scene.b_content_bounds = b_bounds;
    fixture
}

fn material_registry() -> EffectRegistry {
    let program = builtin_background_material_program_with_passes(
        MaterialConfiguration::default().effective().unwrap(),
        2,
    );
    let mut registry = EffectRegistry::empty();
    registry.insert(program).unwrap();
    registry
}

fn compile_pruning_graph(
    fixture: NativeThreeCheckpointFixture,
    checkpoint_dependency_pruning: bool,
) -> oblivion_one::effects::CompiledFrameGraph {
    compile_pruning_graph_for_damage(
        fixture,
        checkpoint_dependency_pruning,
        &EffectRegion::from_rect(fixture.output_bounds),
    )
}

fn compile_pruning_graph_for_damage(
    fixture: NativeThreeCheckpointFixture,
    checkpoint_dependency_pruning: bool,
    source_damage: &EffectRegion,
) -> oblivion_one::effects::CompiledFrameGraph {
    let registry = material_registry();
    let program = registry
        .get(oblivion_one::effects::builtin_background_blur_program_id())
        .expect("built-in material program is registered")
        .program
        .id;
    let scene = ResolvedEffectScene::new(
        1,
        vec![
            native_backdrop_effect_instance(fixture.effect_a, program),
            native_backdrop_effect_instance(fixture.effect_c, program),
            native_backdrop_effect_instance(fixture.effect_b, program),
        ],
    );
    let variants = compile_frame_execution_variants_with_options(
        &scene,
        source_damage,
        fixture.output_bounds,
        &registry,
        FrameExecutionCompileOptions {
            precomputed_window_blur: false,
            terminal_kawase_fusion: false,
            checkpoint_dependency_pruning,
            shell_backdrop_static: false,
        },
    )
    .expect("three-material G3a fixture compiles");
    let FrameExecutionPlan::EffectGraph(graph) = variants.exact else {
        panic!("three-material G3a fixture must compile to an effect graph");
    };
    graph
}

fn test_surface_signature(surface_id: u32, generation: u64) -> EglSceneSurfaceSignature {
    EglSceneSurfaceSignature {
        surface_id,
        commit_sequence: generation,
        buffer_id: u64::from(surface_id) * 100 + generation,
        buffer_width: 160,
        buffer_height: 120,
        buffer_scale: 1,
        buffer_transform: wayland_server::protocol::wl_output::Transform::Normal,
        x: 0,
        y: 0,
        width: 160,
        height: 120,
        render_x: 0,
        render_y: 0,
        clip_x: 0,
        clip_y: 0,
        clip_width: 160,
        clip_height: 120,
        generation,
    }
}

fn fixture_signatures(
    fixture: NativeThreeCheckpointFixture,
    changed_surface: Option<u32>,
) -> Vec<EglSceneSurfaceSignature> {
    [
        fixture.scene.background_surface,
        fixture.scene.a_surface,
        fixture.scene.c_surface,
        fixture.scene.b_surface,
    ]
    .into_iter()
    .map(|surface_id| {
        test_surface_signature(
            surface_id,
            if changed_surface == Some(surface_id) {
                2
            } else {
                1
            },
        )
    })
    .collect()
}

fn set_fixture_snapshot(
    harness: &mut GlesEffectTestHarness,
    surface_signatures: &[EglSceneSurfaceSignature],
) {
    let snapshot = EglCheckpointSceneCausalSnapshot::new(
        harness.renderer.scene_state.current_size,
        &harness.renderer.scene_state.commands,
        &harness.renderer.scene_state.vertices,
        surface_signatures,
        &harness.renderer.scene_state.presentation_opacities,
        &harness
            .renderer
            .scene_state
            .presentation_visual_group_owners,
    );
    harness
        .renderer
        .scene_state
        .current_checkpoint_scene_causal_snapshot = Some(snapshot);
}

fn capture_pass(
    graph: &oblivion_one::effects::CompiledFrameGraph,
    instance: EffectInstanceId,
) -> &oblivion_one::effects::CompiledRenderPass {
    graph
        .passes
        .iter()
        .find(|pass| pass.instance == instance && pass.kind == RenderPassKind::SceneCapture)
        .expect("requested scene capture exists")
}

fn execute_frame(
    harness: &mut GlesEffectTestHarness,
    graph: &oblivion_one::effects::CompiledFrameGraph,
    fixture: NativeThreeCheckpointFixture,
    signatures: &[EglSceneSurfaceSignature],
    region: EffectRegion,
    full: bool,
    planner: EffectDemandPlannerPolicy,
) -> effects::EffectExecutionStats {
    set_fixture_snapshot(harness, signatures);
    harness
        .renderer
        .effect_runtime
        .effect_resources
        .begin_checkpoint_frame();
    let config = effects::EffectDebugConfig::new_with_checkpoint_capture_path(
        effects::EffectDebugCaptureMode::Replay,
        effects::EffectDebugKawaseMode::Partial,
        effects::CheckpointCapturePath::FramebufferShaderCopy,
    );
    let caller_conservative_full = full && planner == EffectDemandPlannerPolicy::Legacy;
    let demand = oblivion_one::effects::plan_effect_execution_demand_with_policy(
        graph,
        &region,
        caller_conservative_full,
        false,
        planner,
    );
    if planner == EffectDemandPlannerPolicy::Regional {
        assert!(
            !demand.plan_stats().regional_fallback,
            "regional planner fallback: {:?}",
            demand.plan_stats().regional_fallback_reason
        );
    }
    let selection = effects::select_effect_execution(graph, &demand);
    effects::execute_effect_graph_with_debug_config(
        &mut harness.renderer,
        graph,
        OutputFramebufferOrigin::TopLeftScanout,
        &diagnostic_repaint_plan_for_repairs_in_size(&[fixture.repair], full, fixture.output_size),
        &demand,
        &selection,
        config,
    )
    .expect("ReplayRequired GLES frame executes")
}

fn seed_harness(
    fixture: NativeThreeCheckpointFixture,
) -> (
    GlesEffectTestHarness,
    Vec<u8>,
    Vec<EglSceneSurfaceSignature>,
) {
    seed_harness_with_policy(fixture, false)
}

fn seed_harness_with_policy(
    fixture: NativeThreeCheckpointFixture,
    pruning: bool,
) -> (
    GlesEffectTestHarness,
    Vec<u8>,
    Vec<EglSceneSurfaceSignature>,
) {
    let mut harness = GlesEffectTestHarness::new(fixture.output_size.0, fixture.output_size.1);
    harness.install_texture_backed_output();
    install_native_three_checkpoint_diagnostic_scene(&mut harness, fixture.scene);
    harness.renderer.effect_runtime.effect_registry = material_registry();
    let graph = compile_pruning_graph(fixture, pruning);
    let signatures = fixture_signatures(fixture, None);
    execute_frame(
        &mut harness,
        &graph,
        fixture,
        &signatures,
        EffectRegion::from_rect(fixture.output_bounds),
        true,
        EffectDemandPlannerPolicy::Regional,
    );
    let pixels = read_diagnostic_pixels(&harness);
    harness
        .renderer
        .promote_checkpoint_cache_causal_state(&graph);
    (harness, pixels, signatures)
}

#[test]
fn gles_cold_warm_and_dependency_identity_transition_match_full_reference() {
    let fixture = pruning_fixture(270);
    let raw_graph = compile_pruning_graph(fixture, false);
    let pruned_graph = compile_pruning_graph(fixture, true);
    let consumer = EffectInstanceId::new(32).unwrap();
    assert_eq!(
        capture_pass(&raw_graph, consumer)
            .checkpoint_dependencies
            .len(),
        1
    );
    assert!(
        capture_pass(&pruned_graph, consumer)
            .checkpoint_dependencies
            .is_empty()
    );
    assert_eq!(
        raw_graph
            .instances
            .iter()
            .find(|instance| instance.id == consumer)
            .unwrap()
            .dependencies
            .len(),
        1
    );
    assert!(
        pruned_graph
            .instances
            .iter()
            .find(|instance| instance.id == consumer)
            .unwrap()
            .dependencies
            .is_empty()
    );

    let (raw_pixels, raw_cold_stats) = {
        let mut harness = GlesEffectTestHarness::new(fixture.output_size.0, fixture.output_size.1);
        harness.install_texture_backed_output();
        install_native_three_checkpoint_diagnostic_scene(&mut harness, fixture.scene);
        harness.renderer.effect_runtime.effect_registry = material_registry();
        let signatures = fixture_signatures(fixture, None);
        let stats = execute_frame(
            &mut harness,
            &raw_graph,
            fixture,
            &signatures,
            EffectRegion::from_rect(fixture.output_bounds),
            true,
            EffectDemandPlannerPolicy::Regional,
        );
        assert!(stats.framebuffer_shader_copy_capture_execution_pixels > 0);
        assert!(stats.replay_capture_passes > 0);
        assert_eq!(stats.checkpoint_dependency_edges, 1);
        (read_diagnostic_pixels(&harness), stats)
    };

    let (pruned_cold_pixels, pruned_cold_stats) = {
        let mut harness = GlesEffectTestHarness::new(fixture.output_size.0, fixture.output_size.1);
        harness.install_texture_backed_output();
        install_native_three_checkpoint_diagnostic_scene(&mut harness, fixture.scene);
        harness.renderer.effect_runtime.effect_registry = material_registry();
        let signatures = fixture_signatures(fixture, None);
        let stats = execute_frame(
            &mut harness,
            &pruned_graph,
            fixture,
            &signatures,
            EffectRegion::from_rect(fixture.output_bounds),
            true,
            EffectDemandPlannerPolicy::Regional,
        );
        assert_eq!(stats.framebuffer_shader_copy_capture_execution_pixels, 0);
        assert!(stats.replay_capture_passes >= 3);
        assert_eq!(stats.checkpoint_dependency_edges, 0);
        (read_diagnostic_pixels(&harness), stats)
    };
    assert_eq!(raw_pixels, pruned_cold_pixels);

    let (mut harness, transitioned_seed_pixels, signatures) = seed_harness(fixture);
    assert_eq!(transitioned_seed_pixels, raw_pixels);
    let old_key =
        effects::checkpoint_capture_cache_key(&raw_graph, capture_pass(&raw_graph, consumer))
            .expect("dependent checkpoint has a semantic key");
    let new_key =
        effects::checkpoint_capture_cache_key(&pruned_graph, capture_pass(&pruned_graph, consumer))
            .expect("dependency-free checkpoint has a semantic key");
    assert_ne!(old_key, new_key);
    assert!(
        harness
            .renderer
            .effect_runtime
            .effect_resources
            .checkpoint_capture_texture(&old_key)
            .is_some()
    );
    assert!(
        harness
            .renderer
            .effect_runtime
            .effect_resources
            .checkpoint_capture_texture(&new_key)
            .is_none()
    );

    let transition_stats = execute_frame(
        &mut harness,
        &pruned_graph,
        fixture,
        &signatures,
        EffectRegion::from_rect(fixture.output_bounds),
        true,
        EffectDemandPlannerPolicy::Regional,
    );
    let transitioned_pixels = read_diagnostic_pixels(&harness);
    assert_eq!(transitioned_pixels, pruned_cold_pixels);
    assert!(transition_stats.replay_capture_cache_full_refreshes > 0);
    assert!(transition_stats.replay_capture_cache_update_pixels > 0);
    assert!(
        harness
            .renderer
            .effect_runtime
            .effect_resources
            .checkpoint_capture_texture(&new_key)
            .is_some()
    );
    assert_eq!(pruned_cold_stats.capture_downsample_fusion_candidates, 0);
    assert_eq!(pruned_cold_stats.capture_downsample_fusion_executed, 0);
    assert_eq!(
        pruned_cold_stats.capture_downsample_fusion_elided_capture_pixels,
        0
    );
    harness
        .renderer
        .promote_checkpoint_cache_causal_state(&pruned_graph);

    let warm_stats = execute_frame(
        &mut harness,
        &pruned_graph,
        fixture,
        &signatures,
        EffectRegion::from_rect(fixture.output_bounds),
        true,
        EffectDemandPlannerPolicy::Regional,
    );
    assert!(warm_stats.replay_capture_cache_zero_copy_hits > 0);
    assert_eq!(read_diagnostic_pixels(&harness), pruned_cold_pixels);
    eprintln!(
        "g3a_gles cold off: dependency_edges={} checkpoint_capture_passes={} checkpoint_capture_pixels={} framebuffer_capture_passes={} framebuffer_shader_copy_pixels={} replay_capture_passes={} replay_capture_pixels={} fusion_candidates={} fusion_executed={} fusion_elided_pixels={}; cold on: dependency_edges={} checkpoint_capture_passes={} checkpoint_capture_pixels={} framebuffer_capture_passes={} framebuffer_shader_copy_pixels={} replay_capture_passes={} replay_capture_pixels={} fusion_candidates={} fusion_executed={} fusion_elided_pixels={}; transition full_refreshes={} update_pixels={} warm_zero_copy_hits={} cache_entries={} cache_bytes={}",
        raw_cold_stats.checkpoint_dependency_edges,
        raw_cold_stats.checkpoint_capture_passes,
        raw_cold_stats.checkpoint_capture_execution_pixels,
        raw_cold_stats.framebuffer_capture_passes,
        raw_cold_stats.framebuffer_shader_copy_capture_execution_pixels,
        raw_cold_stats.replay_capture_passes,
        raw_cold_stats.replay_capture_execution_pixels,
        raw_cold_stats.capture_downsample_fusion_candidates,
        raw_cold_stats.capture_downsample_fusion_executed,
        raw_cold_stats.capture_downsample_fusion_elided_capture_pixels,
        pruned_cold_stats.checkpoint_dependency_edges,
        pruned_cold_stats.checkpoint_capture_passes,
        pruned_cold_stats.checkpoint_capture_execution_pixels,
        pruned_cold_stats.framebuffer_capture_passes,
        pruned_cold_stats.framebuffer_shader_copy_capture_execution_pixels,
        pruned_cold_stats.replay_capture_passes,
        pruned_cold_stats.replay_capture_execution_pixels,
        pruned_cold_stats.capture_downsample_fusion_candidates,
        pruned_cold_stats.capture_downsample_fusion_executed,
        pruned_cold_stats.capture_downsample_fusion_elided_capture_pixels,
        transition_stats.replay_capture_cache_full_refreshes,
        transition_stats.replay_capture_cache_update_pixels,
        warm_stats.replay_capture_cache_zero_copy_hits,
        warm_stats.checkpoint_cache_entries,
        warm_stats.checkpoint_cache_bytes,
    );
}

fn render_partial_source_change(
    fixture: NativeThreeCheckpointFixture,
    planner: EffectDemandPlannerPolicy,
    pruning: bool,
) -> (Vec<u8>, Vec<u8>, effects::EffectExecutionStats) {
    let source_damage = EffectRegion::from_rect(
        EffectRect::new(
            fixture.repair.x as i32,
            fixture.repair.y as i32,
            fixture.repair.width,
            fixture.repair.height,
        )
        .unwrap(),
    );
    let graph = compile_pruning_graph_for_damage(fixture, pruning, &source_damage);
    let consumer = EffectInstanceId::new(32).unwrap();
    let edge_count = graph
        .instances
        .iter()
        .find(|instance| instance.id == consumer)
        .unwrap()
        .dependencies
        .len();
    if fixture.effect_c.target_bounds.x == 270 {
        assert_eq!(edge_count, usize::from(!pruning));
    } else {
        assert_eq!(edge_count, 1);
    }

    let (mut harness, seed_pixels, _) = seed_harness_with_policy(fixture, pruning);
    update_diagnostic_background_for_surface(
        &harness,
        fixture.scene.background_surface,
        fixture.repair,
        [236, 28, 42, 255],
    );
    let changed_signatures = fixture_signatures(fixture, Some(fixture.scene.background_surface));
    let repair = EffectRect::new(
        fixture.repair.x as i32,
        fixture.repair.y as i32,
        fixture.repair.width,
        fixture.repair.height,
    )
    .unwrap();
    let stats = execute_frame(
        &mut harness,
        &graph,
        fixture,
        &changed_signatures,
        EffectRegion::from_rect(repair),
        false,
        planner,
    );
    let changed_pixels = read_diagnostic_pixels(&harness);
    assert!(
        stats.checkpoint_causal_scene_prefix_changed > 0,
        "changed source was not observed in checkpoint causality; stats={stats:#?}"
    );
    assert!(
        stats.replay_capture_cache_full_refreshes > 0,
        "changed source did not force a full replay-cache refresh; stats={stats:#?}"
    );
    assert!(
        stats.replay_capture_cache_update_pixels > 0,
        "changed source did not repopulate replay-cache pixels; stats={stats:#?}"
    );
    (seed_pixels, changed_pixels, stats)
}

#[test]
fn gles_partial_source_changes_match_under_both_planners_for_disjoint_and_overlapping_support() {
    for planner in [
        EffectDemandPlannerPolicy::Legacy,
        EffectDemandPlannerPolicy::Regional,
    ] {
        // The overlap case sits closer to A than before: the half-texel radius
        // shrinks the sampling reach (about 76px instead of 108px), so 260 no
        // longer reaches A's content. 220 keeps the reach-overlap classification.
        for (second_x, should_prune) in [(270, true), (220, false)] {
            let fixture = pruning_fixture(second_x);
            let (raw_seed, raw_after, raw_stats) =
                render_partial_source_change(fixture, planner, false);
            let (pruned_seed, pruned_after, pruned_stats) =
                render_partial_source_change(fixture, planner, true);
            assert_eq!(raw_seed, pruned_seed);
            assert_eq!(raw_after, pruned_after);
            if should_prune {
                assert_eq!(pruned_stats.checkpoint_dependency_edges, 0);
            } else {
                assert_eq!(
                    pruned_stats.checkpoint_dependency_edges,
                    raw_stats.checkpoint_dependency_edges
                );
            }
            assert!(
                pruned_stats.checkpoint_dependency_edges <= raw_stats.checkpoint_dependency_edges
            );
        }
    }
}
