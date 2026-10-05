use super::*;
use oblivion_one::effects::{EffectInstanceId, RenderPassKind};

fn replay_config() -> effects::EffectDebugConfig {
    effects::EffectDebugConfig::new_with_checkpoint_capture_path(
        effects::EffectDebugCaptureMode::Replay,
        effects::EffectDebugKawaseMode::Partial,
        effects::CheckpointCapturePath::FramebufferShaderCopy,
    )
}

fn root_capture(
    graph: &oblivion_one::effects::CompiledFrameGraph,
) -> &oblivion_one::effects::CompiledRenderPass {
    graph
        .passes
        .iter()
        .find(|pass| {
            pass.kind == RenderPassKind::SceneCapture
                && pass.instance == EffectInstanceId::new(31).unwrap()
                && pass.checkpoint_dependencies.is_empty()
        })
        .expect("A is the dependency-free root SceneCapture")
}

fn root_key(graph: &oblivion_one::effects::CompiledFrameGraph) -> impl std::fmt::Debug + PartialEq {
    effects::checkpoint_capture_cache_key(graph, root_capture(graph))
        .expect("root Replay capture has a semantic cache key")
}

fn root_domain_pixels(graph: &oblivion_one::effects::CompiledFrameGraph) -> u64 {
    let output = root_capture(graph).output.unwrap();
    let texture = graph
        .textures
        .iter()
        .find(|texture| texture.id == output)
        .unwrap();
    u64::from(texture.width).saturating_mul(u64::from(texture.height))
}

fn root_capture_snapshot(
    harness: &mut GlesEffectTestHarness,
    graph: &oblivion_one::effects::CompiledFrameGraph,
) -> (u64, Vec<u8>) {
    let pass = root_capture(graph);
    let key = effects::checkpoint_capture_cache_key(graph, pass).unwrap();
    let plan = graph
        .textures
        .iter()
        .find(|texture| Some(texture.id) == pass.output)
        .unwrap();
    let texture = harness
        .renderer
        .effect_runtime
        .effect_resources
        .checkpoint_capture_texture(&key)
        .expect("dependency-free Replay cache entry exists");
    let pixels = read_effect_texture_pixels(harness, &texture, plan.width, plan.height);
    (texture.id, pixels)
}

fn root_cache_needs_full_refresh(
    harness: &GlesEffectTestHarness,
    graph: &oblivion_one::effects::CompiledFrameGraph,
    frame_serial: u64,
) -> bool {
    let key = effects::checkpoint_capture_cache_key(graph, root_capture(graph)).unwrap();
    harness
        .renderer
        .effect_runtime
        .effect_resources
        .checkpoint_capture_needs_full_refresh(&key, frame_serial)
}

fn surface_signature(surface_id: u32, generation: u64) -> EglSceneSurfaceSignature {
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

fn signatures(
    scene: NativeThreeCheckpointSceneSpec,
    changed_surface: Option<u32>,
) -> Vec<EglSceneSurfaceSignature> {
    [
        scene.background_surface,
        scene.a_surface,
        scene.c_surface,
        scene.b_surface,
    ]
    .into_iter()
    .map(|surface_id| {
        surface_signature(
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

fn set_current_snapshot(
    renderer: &mut GlesSceneRenderer,
    surface_signatures: &[EglSceneSurfaceSignature],
) {
    renderer
        .scene_state
        .current_checkpoint_scene_causal_snapshot = Some(EglCheckpointSceneCausalSnapshot::new(
        renderer.scene_state.current_size,
        &renderer.scene_state.commands,
        &renderer.scene_state.vertices,
        surface_signatures,
        &renderer.scene_state.presentation_opacities,
        &renderer.scene_state.presentation_visual_group_owners,
    ));
}

fn render_root_frame(
    harness: &mut GlesEffectTestHarness,
    fixture: NativeThreeCheckpointFixture,
    graph: &oblivion_one::effects::CompiledFrameGraph,
    surface_signatures: &[EglSceneSurfaceSignature],
    region: &EffectRegion,
    full: bool,
) -> RendererResult<effects::EffectExecutionStats> {
    set_current_snapshot(&mut harness.renderer, surface_signatures);
    harness
        .renderer
        .effect_runtime
        .effect_resources
        .begin_checkpoint_frame();
    let config = replay_config();
    let demand = oblivion_one::effects::plan_effect_execution_demand_with_kawase_mode(
        graph, region, full, false,
    );
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
}

fn new_harness(
    fixture: NativeThreeCheckpointFixture,
    background_color: [u8; 4],
) -> GlesEffectTestHarness {
    let mut harness = GlesEffectTestHarness::new(fixture.output_size.0, fixture.output_size.1);
    harness.install_texture_backed_output();
    install_native_three_checkpoint_diagnostic_scene(&mut harness, fixture.scene);
    update_diagnostic_background_for_surface(
        &harness,
        fixture.scene.background_surface,
        fixture.repair,
        background_color,
    );
    harness
}

fn full_replay_reference(
    fixture: NativeThreeCheckpointFixture,
    background_color: [u8; 4],
    surface_signatures: &[EglSceneSurfaceSignature],
) -> (Vec<u8>, Vec<u8>) {
    full_replay_reference_with_patch(fixture, background_color, None, surface_signatures)
}

fn full_replay_reference_with_patch(
    fixture: NativeThreeCheckpointFixture,
    background_color: [u8; 4],
    patched_background_color: Option<[u8; 4]>,
    surface_signatures: &[EglSceneSurfaceSignature],
) -> (Vec<u8>, Vec<u8>) {
    let mut reference = new_harness(fixture, background_color);
    if let Some(patched_background_color) = patched_background_color {
        update_diagnostic_background_for_surface(
            &reference,
            fixture.scene.background_surface,
            fixture.repair,
            patched_background_color,
        );
    }
    let full_region = EffectRegion::from_rect(fixture.output_bounds);
    let graph = compile_native_three_checkpoint_graph(fixture, &EffectRegion::empty());
    render_root_frame(
        &mut reference,
        fixture,
        &graph,
        surface_signatures,
        &full_region,
        true,
    )
    .expect("full Replay reference renders");
    let output = read_diagnostic_pixels(&reference);
    let (_, capture) = root_capture_snapshot(&mut reference, &graph);
    (output, capture)
}

fn assert_pixel_buffers_match(actual: &[u8], expected: &[u8]) {
    if actual == expected {
        return;
    }
    assert_eq!(actual.len(), expected.len(), "pixel buffer lengths differ");
    let mismatches = actual
        .chunks_exact(4)
        .zip(expected.chunks_exact(4))
        .enumerate()
        .filter(|(_, (actual, expected))| actual != expected)
        .collect::<Vec<_>>();
    let (first_index, (first_actual, first_expected)) = mismatches[0];
    panic!(
        "{} pixels differ; first pixel index {first_index}: actual={first_actual:?}, expected={first_expected:?}",
        mismatches.len()
    );
}

#[test]
fn dependency_free_replay_cache_first_population_replays_full_domain() {
    let fixture = native_three_checkpoint_fixture();
    let surface_signatures = signatures(fixture.scene, None);
    let mut harness = new_harness(fixture, [30, 50, 70, 255]);
    let full_region = EffectRegion::from_rect(fixture.output_bounds);
    let graph = compile_native_three_checkpoint_graph(fixture, &EffectRegion::empty());

    let stats = render_root_frame(
        &mut harness,
        fixture,
        &graph,
        &surface_signatures,
        &full_region,
        true,
    )
    .expect("first full Replay refresh succeeds");

    let frame_serial = harness
        .renderer
        .effect_runtime
        .effect_resources
        .checkpoint_frame_serial();
    assert_eq!(
        stats.replay_capture_execution_pixels,
        root_domain_pixels(&graph)
    );
    assert!(stats.replay_capture_commands > 0);
    assert!(
        !root_cache_needs_full_refresh(&harness, &graph, frame_serial + 1),
        "the complete first Replay population is current"
    );
}

#[test]
fn dependency_free_replay_unchanged_source_zero_copies_and_matches_full_output() {
    let fixture = native_three_checkpoint_fixture();
    let surface_signatures = signatures(fixture.scene, None);
    let mut harness = new_harness(fixture, [30, 50, 70, 255]);
    let full_region = EffectRegion::from_rect(fixture.output_bounds);
    let graph = compile_native_three_checkpoint_graph(fixture, &EffectRegion::empty());

    render_root_frame(
        &mut harness,
        fixture,
        &graph,
        &surface_signatures,
        &full_region,
        true,
    )
    .expect("initial full Replay population succeeds");
    let (texture_id_before, pixels_before) = root_capture_snapshot(&mut harness, &graph);
    let stats = render_root_frame(
        &mut harness,
        fixture,
        &graph,
        &surface_signatures,
        &full_region,
        true,
    )
    .expect("unchanged Replay cache hit succeeds");

    assert_eq!(stats.replay_capture_execution_pixels, 0);
    assert_eq!(stats.replay_capture_commands, 0);
    assert_eq!(stats.replay_capture_draw_calls, 0);
    assert_eq!(stats.replay_capture_scene_scan_pairs, 0);
    assert_eq!(stats.replay_capture_command_region_pairs, 0);
    assert_eq!(stats.replay_capture_planner_commands_visited, 0);
    let (texture_id_after, pixels_after) = root_capture_snapshot(&mut harness, &graph);
    assert_eq!(texture_id_after, texture_id_before);
    assert_eq!(pixels_after, pixels_before);

    let actual_output = read_diagnostic_pixels(&harness);
    drop(harness);
    let (reference_output, reference_capture) =
        full_replay_reference(fixture, [30, 50, 70, 255], &surface_signatures);
    assert_pixel_buffers_match(&actual_output, &reference_output);
    assert_pixel_buffers_match(&pixels_after, &reference_capture);
}

#[test]
fn dependency_free_replay_changed_source_refreshes_entire_capture_domain() {
    let fixture = native_three_checkpoint_fixture();
    let initial_signatures = signatures(fixture.scene, None);
    let changed_signatures = signatures(fixture.scene, Some(fixture.scene.background_surface));
    let mut harness = new_harness(fixture, [30, 50, 70, 255]);
    let full_region = EffectRegion::from_rect(fixture.output_bounds);
    let graph = compile_native_three_checkpoint_graph(fixture, &EffectRegion::empty());
    render_root_frame(
        &mut harness,
        fixture,
        &graph,
        &initial_signatures,
        &full_region,
        true,
    )
    .expect("initial Replay population succeeds");

    update_diagnostic_background_for_surface(
        &harness,
        fixture.scene.background_surface,
        fixture.repair,
        [220, 24, 36, 255],
    );
    let current_damage = diagnostic_region(fixture.repair);
    let changed_graph = compile_native_three_checkpoint_graph(fixture, &current_damage);
    assert_eq!(root_key(&graph), root_key(&changed_graph));
    let stats = render_root_frame(
        &mut harness,
        fixture,
        &changed_graph,
        &changed_signatures,
        &current_damage,
        false,
    )
    .expect("changed source forces a complete Replay refresh");

    assert_eq!(
        stats.replay_capture_execution_pixels,
        root_domain_pixels(&changed_graph)
    );
    let (_, actual_capture) = root_capture_snapshot(&mut harness, &changed_graph);
    drop(harness);
    let (_, reference_capture) = full_replay_reference_with_patch(
        fixture,
        [30, 50, 70, 255],
        Some([220, 24, 36, 255]),
        &changed_signatures,
    );
    assert_pixel_buffers_match(&actual_capture, &reference_capture);
}

#[test]
fn dependency_free_replay_without_causal_baseline_refreshes_entire_domain() {
    let fixture = native_three_checkpoint_fixture();
    let surface_signatures = signatures(fixture.scene, None);
    let mut harness = new_harness(fixture, [30, 50, 70, 255]);
    let full_region = EffectRegion::from_rect(fixture.output_bounds);
    let current_damage = diagnostic_region(fixture.repair);
    let graph = compile_native_three_checkpoint_graph(fixture, &current_damage);
    render_root_frame(
        &mut harness,
        fixture,
        &graph,
        &surface_signatures,
        &full_region,
        true,
    )
    .expect("initial Replay population succeeds");

    harness
        .renderer
        .effect_runtime
        .effect_resources
        .invalidate_checkpoint_causal_state();
    let stats = render_root_frame(
        &mut harness,
        fixture,
        &graph,
        &surface_signatures,
        &current_damage,
        false,
    )
    .expect("missing causal baseline forces a complete Replay refresh");

    assert_eq!(
        stats.replay_capture_execution_pixels,
        root_domain_pixels(&graph)
    );
}

#[test]
fn dependency_free_replay_domain_change_requires_full_new_population() {
    let fixture = native_three_checkpoint_fixture();
    let surface_signatures = signatures(fixture.scene, None);
    let mut harness = new_harness(fixture, [30, 50, 70, 255]);
    let full_region = EffectRegion::from_rect(fixture.output_bounds);
    let first_graph = compile_native_three_checkpoint_graph(fixture, &EffectRegion::empty());
    render_root_frame(
        &mut harness,
        fixture,
        &first_graph,
        &surface_signatures,
        &full_region,
        true,
    )
    .expect("initial Replay population succeeds");

    let mut changed_fixture = fixture;
    changed_fixture.effect_a.target_bounds.x += 1;
    let changed_graph =
        compile_native_three_checkpoint_graph(changed_fixture, &EffectRegion::empty());
    assert_ne!(root_key(&first_graph), root_key(&changed_graph));
    let changed_region = EffectRegion::from_rect(changed_fixture.output_bounds);
    let stats = render_root_frame(
        &mut harness,
        changed_fixture,
        &changed_graph,
        &surface_signatures,
        &changed_region,
        true,
    )
    .expect("changed capture identity receives a full Replay population");

    assert_eq!(
        stats.replay_capture_execution_pixels,
        root_domain_pixels(&changed_graph)
    );
    assert!(
        root_cache_needs_full_refresh(
            &harness,
            &changed_graph,
            harness
                .renderer
                .effect_runtime
                .effect_resources
                .checkpoint_frame_serial()
                + 1
        ) == false
    );
}

#[test]
fn dependency_free_replay_cannot_reuse_stale_presented_cache_state() {
    let fixture = native_three_checkpoint_fixture();
    let initial_signatures = signatures(fixture.scene, None);
    let changed_signatures = signatures(fixture.scene, Some(fixture.scene.background_surface));
    let mut harness = new_harness(fixture, [30, 50, 70, 255]);
    let full_region = EffectRegion::from_rect(fixture.output_bounds);
    let current_damage = diagnostic_region(fixture.repair);
    let graph = compile_native_three_checkpoint_graph(fixture, &full_region);

    render_root_frame(
        &mut harness,
        fixture,
        &graph,
        &initial_signatures,
        &full_region,
        true,
    )
    .expect("presented P=X populates the cache");
    harness
        .renderer
        .commit_presented(EglSceneFrameCommit::empty_for_test(), OutputDamage::Empty);

    update_diagnostic_background_for_surface(
        &harness,
        fixture.scene.background_surface,
        fixture.repair,
        [220, 24, 36, 255],
    );
    render_root_frame(
        &mut harness,
        fixture,
        &graph,
        &changed_signatures,
        &current_damage,
        false,
    )
    .expect("unpresented A=Y renders into the physical cache");

    update_diagnostic_background_for_surface(
        &harness,
        fixture.scene.background_surface,
        fixture.repair,
        [30, 50, 70, 255],
    );
    let stats = render_root_frame(
        &mut harness,
        fixture,
        &graph,
        &initial_signatures,
        &current_damage,
        false,
    )
    .expect("B=X refreshes from its changed current source");

    assert_eq!(
        stats.replay_capture_execution_pixels,
        root_domain_pixels(&graph)
    );
    let (actual_output, actual_capture) = {
        let output = read_diagnostic_pixels(&harness);
        let (_, capture) = root_capture_snapshot(&mut harness, &graph);
        (output, capture)
    };
    drop(harness);
    let mut reference = new_harness(fixture, [30, 50, 70, 255]);
    render_root_frame(
        &mut reference,
        fixture,
        &graph,
        &initial_signatures,
        &full_region,
        true,
    )
    .expect("reference P=X populates the cache");
    reference
        .renderer
        .commit_presented(EglSceneFrameCommit::empty_for_test(), OutputDamage::Empty);
    update_diagnostic_background_for_surface(
        &reference,
        fixture.scene.background_surface,
        fixture.repair,
        [220, 24, 36, 255],
    );
    render_root_frame(
        &mut reference,
        fixture,
        &graph,
        &changed_signatures,
        &current_damage,
        false,
    )
    .expect("reference A=Y renders the changed source");
    update_diagnostic_background_for_surface(
        &reference,
        fixture.scene.background_surface,
        fixture.repair,
        [30, 50, 70, 255],
    );
    let stats = render_root_frame(
        &mut reference,
        fixture,
        &graph,
        &initial_signatures,
        &current_damage,
        false,
    )
    .expect("reference B=X performs a full source refresh");
    assert_eq!(
        stats.replay_capture_execution_pixels,
        root_domain_pixels(&graph)
    );
    let reference_output = read_diagnostic_pixels(&reference);
    let (_, reference_capture) = root_capture_snapshot(&mut reference, &graph);
    assert_pixel_buffers_match(&actual_output, &reference_output);
    assert_pixel_buffers_match(&actual_capture, &reference_capture);
}

#[test]
fn dependency_free_replay_can_use_immediately_previous_unpresented_cache_state() {
    let fixture = native_three_checkpoint_fixture();
    let initial_signatures = signatures(fixture.scene, None);
    let changed_signatures = signatures(fixture.scene, Some(fixture.scene.background_surface));
    let mut harness = new_harness(fixture, [30, 50, 70, 255]);
    let full_region = EffectRegion::from_rect(fixture.output_bounds);
    let current_damage = diagnostic_region(fixture.repair);
    let graph = compile_native_three_checkpoint_graph(fixture, &full_region);

    render_root_frame(
        &mut harness,
        fixture,
        &graph,
        &initial_signatures,
        &full_region,
        true,
    )
    .expect("presented P=X populates the cache");
    harness
        .renderer
        .commit_presented(EglSceneFrameCommit::empty_for_test(), OutputDamage::Empty);

    update_diagnostic_background_for_surface(
        &harness,
        fixture.scene.background_surface,
        fixture.repair,
        [220, 24, 36, 255],
    );
    render_root_frame(
        &mut harness,
        fixture,
        &graph,
        &changed_signatures,
        &current_damage,
        false,
    )
    .expect("unpresented A=Y writes its physical Replay cache");
    let (texture_id_before, pixels_before) = root_capture_snapshot(&mut harness, &graph);

    let stats = render_root_frame(
        &mut harness,
        fixture,
        &graph,
        &changed_signatures,
        &current_damage,
        false,
    )
    .expect("B=Y can reuse immediately preceding unpresented A=Y");

    assert_eq!(stats.replay_capture_execution_pixels, 0);
    assert_eq!(stats.replay_capture_commands, 0);
    assert_eq!(stats.replay_capture_draw_calls, 0);
    assert_eq!(stats.replay_capture_scene_scan_pairs, 0);
    assert_eq!(stats.replay_capture_command_region_pairs, 0);
    assert_eq!(stats.replay_capture_planner_commands_visited, 0);
    let (texture_id_after, pixels_after) = root_capture_snapshot(&mut harness, &graph);
    assert_eq!(texture_id_after, texture_id_before);
    assert_eq!(pixels_after, pixels_before);
    let actual_output = read_diagnostic_pixels(&harness);
    drop(harness);
    let mut reference = new_harness(fixture, [30, 50, 70, 255]);
    render_root_frame(
        &mut reference,
        fixture,
        &graph,
        &initial_signatures,
        &full_region,
        true,
    )
    .expect("reference P=X populates the cache");
    reference
        .renderer
        .commit_presented(EglSceneFrameCommit::empty_for_test(), OutputDamage::Empty);
    update_diagnostic_background_for_surface(
        &reference,
        fixture.scene.background_surface,
        fixture.repair,
        [220, 24, 36, 255],
    );
    render_root_frame(
        &mut reference,
        fixture,
        &graph,
        &changed_signatures,
        &current_damage,
        false,
    )
    .expect("reference A=Y renders the changed source");
    let root_key = effects::checkpoint_capture_cache_key(&graph, root_capture(&graph))
        .expect("reference root capture cache key");
    reference
        .renderer
        .effect_runtime
        .effect_resources
        .invalidate_checkpoint_capture(&root_key);
    let stats = render_root_frame(
        &mut reference,
        fixture,
        &graph,
        &changed_signatures,
        &current_damage,
        false,
    )
    .expect("reference B=Y forces a complete Replay refresh");
    assert_eq!(
        stats.replay_capture_execution_pixels,
        root_domain_pixels(&graph)
    );
    let reference_output = read_diagnostic_pixels(&reference);
    let (_, reference_capture) = root_capture_snapshot(&mut reference, &graph);
    assert_pixel_buffers_match(&actual_output, &reference_output);
    assert_pixel_buffers_match(&pixels_after, &reference_capture);
}

#[test]
fn dependency_free_replay_discard_requires_full_refresh() {
    let fixture = native_three_checkpoint_fixture();
    let surface_signatures = signatures(fixture.scene, None);
    let mut harness = new_harness(fixture, [30, 50, 70, 255]);
    let full_region = EffectRegion::from_rect(fixture.output_bounds);
    let current_damage = diagnostic_region(fixture.repair);
    let graph = compile_native_three_checkpoint_graph(fixture, &full_region);
    render_root_frame(
        &mut harness,
        fixture,
        &graph,
        &surface_signatures,
        &full_region,
        true,
    )
    .expect("initial Replay population succeeds");

    harness
        .renderer
        .discard_rendered(EglSceneFrameCommit::empty_for_test());
    let stats = render_root_frame(
        &mut harness,
        fixture,
        &graph,
        &surface_signatures,
        &current_damage,
        false,
    )
    .expect("discarded cache history forces a full Replay refresh");

    assert_eq!(
        stats.replay_capture_execution_pixels,
        root_domain_pixels(&graph)
    );
}

#[test]
fn failed_dependency_free_replay_refresh_leaves_cache_invalid() {
    let fixture = native_three_checkpoint_fixture();
    let initial_signatures = signatures(fixture.scene, None);
    let changed_signatures = signatures(fixture.scene, Some(fixture.scene.background_surface));
    let mut harness = new_harness(fixture, [30, 50, 70, 255]);
    let full_region = EffectRegion::from_rect(fixture.output_bounds);
    let current_damage = diagnostic_region(fixture.repair);
    let graph = compile_native_three_checkpoint_graph(fixture, &full_region);
    render_root_frame(
        &mut harness,
        fixture,
        &graph,
        &initial_signatures,
        &full_region,
        true,
    )
    .expect("initial Replay population succeeds");

    update_diagnostic_background_for_surface(
        &harness,
        fixture.scene.background_surface,
        fixture.repair,
        [220, 24, 36, 255],
    );
    harness
        .renderer
        .effect_runtime
        .fail_next_dependency_free_replay_capture = true;
    assert!(
        render_root_frame(
            &mut harness,
            fixture,
            &graph,
            &changed_signatures,
            &current_damage,
            false,
        )
        .is_err()
    );

    let failed_frame_serial = harness
        .renderer
        .effect_runtime
        .effect_resources
        .checkpoint_frame_serial();
    assert!(root_cache_needs_full_refresh(
        &harness,
        &graph,
        failed_frame_serial
    ));
    let next = render_root_frame(
        &mut harness,
        fixture,
        &graph,
        &changed_signatures,
        &current_damage,
        false,
    )
    .expect("the next frame retries a full Replay refresh");
    assert_eq!(
        next.replay_capture_execution_pixels,
        root_domain_pixels(&graph)
    );
}

#[test]
fn dependency_free_replay_zero_copy_preserves_complete_effect_output() {
    let fixture = native_three_checkpoint_fixture();
    let surface_signatures = signatures(fixture.scene, None);
    let mut harness = new_harness(fixture, [30, 50, 70, 255]);
    let full_region = EffectRegion::from_rect(fixture.output_bounds);
    let graph = compile_native_three_checkpoint_graph(fixture, &EffectRegion::empty());
    render_root_frame(
        &mut harness,
        fixture,
        &graph,
        &surface_signatures,
        &full_region,
        true,
    )
    .expect("initial Replay population succeeds");
    let second = render_root_frame(
        &mut harness,
        fixture,
        &graph,
        &surface_signatures,
        &full_region,
        true,
    )
    .expect("unchanged root source zero-copies");
    assert_eq!(second.replay_capture_execution_pixels, 0);

    let output = read_diagnostic_pixels(&harness);
    let (_, capture) = root_capture_snapshot(&mut harness, &graph);
    drop(harness);
    let mut reference = new_harness(fixture, [30, 50, 70, 255]);
    render_root_frame(
        &mut reference,
        fixture,
        &graph,
        &surface_signatures,
        &full_region,
        true,
    )
    .expect("full reference first population succeeds");
    let root_key = effects::checkpoint_capture_cache_key(&graph, root_capture(&graph))
        .expect("reference root capture cache key");
    reference
        .renderer
        .effect_runtime
        .effect_resources
        .invalidate_checkpoint_capture(&root_key);
    let full_refresh = render_root_frame(
        &mut reference,
        fixture,
        &graph,
        &surface_signatures,
        &full_region,
        true,
    )
    .expect("forced full Replay reference succeeds");
    assert_eq!(
        full_refresh.replay_capture_execution_pixels,
        root_domain_pixels(&graph)
    );
    let reference_output = read_diagnostic_pixels(&reference);
    let (_, reference_capture) = root_capture_snapshot(&mut reference, &graph);
    assert_pixel_buffers_match(&output, &reference_output);
    assert_pixel_buffers_match(&capture, &reference_capture);
}
