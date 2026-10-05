use super::*;
use oblivion_one::effects::{EffectFrameDemand, EffectInstanceId, GraphPassId};

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
    let snapshot = EglCheckpointSceneCausalSnapshot::new(
        renderer.scene_state.current_size,
        &renderer.scene_state.commands,
        &renderer.scene_state.vertices,
        surface_signatures,
        &renderer.scene_state.presentation_opacities,
        &renderer.scene_state.presentation_visual_group_owners,
    );
    renderer
        .scene_state
        .current_checkpoint_scene_causal_snapshot = Some(snapshot);
}

fn promote_current_cache_causal_state(
    renderer: &mut GlesSceneRenderer,
    graph: &oblivion_one::effects::CompiledFrameGraph,
) {
    renderer.promote_checkpoint_cache_causal_state(graph);
}

fn cached_checkpoint_pixels(
    harness: &mut GlesEffectTestHarness,
    graph: &oblivion_one::effects::CompiledFrameGraph,
    instance_id: u64,
) -> Vec<u8> {
    let instance = oblivion_one::effects::EffectInstanceId::new(instance_id).unwrap();
    let pass = graph
        .passes
        .iter()
        .find(|pass| {
            pass.kind == RenderPassKind::SceneCapture
                && pass.instance == instance
                && !pass.checkpoint_dependencies.is_empty()
        })
        .expect("requested checkpoint capture exists");
    let key = effects::checkpoint_capture_cache_key(graph, pass)
        .expect("requested checkpoint has a semantic cache key");
    let texture_plan = graph
        .textures
        .iter()
        .find(|texture| Some(texture.id) == pass.output)
        .expect("checkpoint capture has a texture plan");
    let texture = harness
        .renderer
        .effect_runtime
        .effect_resources
        .checkpoint_capture_texture(&key)
        .expect("checkpoint texture remains cache-owned");
    read_effect_texture_pixels(harness, &texture, texture_plan.width, texture_plan.height)
}

fn fallback_checkpoint_pixels(
    graph: &oblivion_one::effects::CompiledFrameGraph,
    output_size: (u32, u32),
) -> u64 {
    graph
        .passes
        .iter()
        .filter(|pass| {
            pass.kind == RenderPassKind::SceneCapture && !pass.checkpoint_dependencies.is_empty()
        })
        .map(|pass| {
            let texture_plan = graph
                .textures
                .iter()
                .find(|texture| Some(texture.id) == pass.output)
                .expect("checkpoint capture has a texture plan");
            let (rects, _) = effects::checkpoint_update_rects_for_test(
                &graph.final_damage,
                texture_plan.domain,
                output_size,
                texture_plan,
            );
            rects.iter().fold(0_u64, |pixels, rect| {
                pixels.saturating_add(u64::from(rect.width).saturating_mul(u64::from(rect.height)))
            })
        })
        .sum()
}

fn shift_checkpoint_capture_domains(graph: &mut oblivion_one::effects::CompiledFrameGraph) {
    let outputs = graph
        .passes
        .iter()
        .filter(|pass| {
            pass.kind == RenderPassKind::SceneCapture && !pass.checkpoint_dependencies.is_empty()
        })
        .filter_map(|pass| pass.output)
        .collect::<Vec<_>>();
    for output in outputs {
        let texture = graph
            .textures
            .iter_mut()
            .find(|texture| texture.id == output)
            .expect("checkpoint capture texture exists");
        texture.domain.x -= 1;
    }
}

fn render_checkpoint_test_frame(
    harness: &mut GlesEffectTestHarness,
    graph: &oblivion_one::effects::CompiledFrameGraph,
    surface_signatures: &[EglSceneSurfaceSignature],
    repair: OutputRect,
    output_size: (u32, u32),
    region: EffectRegion,
    full: bool,
    config: effects::EffectDebugConfig,
) -> u64 {
    render_checkpoint_test_frame_with_stats(
        harness,
        graph,
        surface_signatures,
        repair,
        output_size,
        region,
        full,
        config,
    )
    .checkpoint_capture_execution_pixels
}

fn render_checkpoint_test_frame_with_stats(
    harness: &mut GlesEffectTestHarness,
    graph: &oblivion_one::effects::CompiledFrameGraph,
    surface_signatures: &[EglSceneSurfaceSignature],
    repair: OutputRect,
    output_size: (u32, u32),
    region: EffectRegion,
    full: bool,
    config: effects::EffectDebugConfig,
) -> effects::EffectExecutionStats {
    set_current_snapshot(&mut harness.renderer, surface_signatures);
    harness
        .renderer
        .effect_runtime
        .effect_resources
        .begin_checkpoint_frame();
    let demand = oblivion_one::effects::plan_effect_execution_demand_with_kawase_mode(
        graph,
        &region,
        full,
        config.kawase_mode() == effects::EffectDebugKawaseMode::Full,
    );
    let selection = effects::select_effect_execution(graph, &demand);
    effects::execute_effect_graph_with_debug_config(
        &mut harness.renderer,
        graph,
        OutputFramebufferOrigin::TopLeftScanout,
        &diagnostic_repaint_plan_for_repairs_in_size(&[repair], full, output_size),
        &demand,
        &selection,
        config,
    )
    .expect("diagnostic frame renders")
}

fn full_current_checkpoint_reference(
    fixture: NativeThreeCheckpointFixture,
    signatures: &[EglSceneSurfaceSignature],
    source_color: [u8; 4],
    config: effects::EffectDebugConfig,
) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let full_region = EffectRegion::from_rect(fixture.output_bounds);
    let current_damage = diagnostic_region(fixture.repair);
    let graph = compile_native_three_checkpoint_graph(fixture, &current_damage);
    let mut reference = GlesEffectTestHarness::new(fixture.output_size.0, fixture.output_size.1);
    reference.install_texture_backed_output();
    install_native_three_checkpoint_diagnostic_scene(&mut reference, fixture.scene);
    update_diagnostic_background_for_surface(
        &reference,
        fixture.scene.background_surface,
        fixture.repair,
        source_color,
    );
    render_checkpoint_test_frame(
        &mut reference,
        &graph,
        signatures,
        fixture.repair,
        fixture.output_size,
        full_region,
        true,
        config,
    );
    let output = read_diagnostic_pixels(&reference);
    let checkpoint_c = cached_checkpoint_pixels(&mut reference, &graph, 32);
    let checkpoint_b = cached_checkpoint_pixels(&mut reference, &graph, 33);
    (output, checkpoint_c, checkpoint_b)
}

#[test]
fn dependency_stability_propagates_and_fails_closed() {
    let fixture = native_three_checkpoint_fixture();
    let mut harness = GlesEffectTestHarness::new(fixture.output_size.0, fixture.output_size.1);
    install_native_three_checkpoint_diagnostic_scene(&mut harness, fixture.scene);
    let graph = compile_native_three_checkpoint_graph(fixture, &EffectRegion::empty());
    let initial_signatures = signatures(fixture.scene, None);
    set_current_snapshot(&mut harness.renderer, &initial_signatures);
    let no_history = effects::checkpoint_causal_stability_plan(&harness.renderer, &graph);
    assert!(
        no_history
            .instances
            .values()
            .all(|stability| { !stability.output_unchanged && !stability.source_unchanged })
    );
    assert!(
        no_history
            .captures
            .values()
            .all(|stability| !stability.source_unchanged)
    );

    let mut boundary_change = graph.clone();
    boundary_change
        .passes
        .iter_mut()
        .find(|pass| {
            pass.kind == RenderPassKind::SceneCapture
                && pass.instance == EffectInstanceId::new(31).unwrap()
        })
        .expect("A capture exists")
        .anchor_scope = oblivion_one::compositor::EffectAnchorScope::VisualGroup;
    let boundary_change_plan =
        effects::checkpoint_causal_stability_plan(&harness.renderer, &boundary_change);
    assert!(!boundary_change_plan.instances[&EffectInstanceId::new(31).unwrap()].source_unchanged);

    harness
        .renderer
        .effect_runtime
        .effect_resources
        .begin_checkpoint_frame();
    promote_current_cache_causal_state(&mut harness.renderer, &graph);
    harness
        .renderer
        .effect_runtime
        .effect_resources
        .begin_checkpoint_frame();

    let unchanged = effects::checkpoint_causal_stability_plan(&harness.renderer, &graph);
    assert!(
        unchanged
            .instances
            .values()
            .all(|stability| stability.output_unchanged),
        "unexpected unchanged graph stability: {:#?}",
        unchanged.instances
    );

    let original_current_scene = harness
        .renderer
        .scene_state
        .current_checkpoint_scene_causal_snapshot
        .clone()
        .expect("diagnostic scene has a current snapshot");
    let changed_capture_owners = HashMap::from([(VisualGroupId::new(101).unwrap(), 900)]);
    harness
        .renderer
        .scene_state
        .current_checkpoint_scene_causal_snapshot = Some(EglCheckpointSceneCausalSnapshot::new(
        harness.renderer.scene_state.current_size,
        &harness.renderer.scene_state.commands,
        &harness.renderer.scene_state.vertices,
        &initial_signatures,
        &harness.renderer.scene_state.presentation_opacities,
        &changed_capture_owners,
    ));
    let owner_change_plan = effects::checkpoint_causal_stability_plan(&harness.renderer, &graph);
    assert!(!owner_change_plan.instances[&EffectInstanceId::new(31).unwrap()].source_unchanged);
    harness
        .renderer
        .scene_state
        .current_checkpoint_scene_causal_snapshot = Some(original_current_scene);

    let mut renumbered = graph.clone();
    let pass_id_map = renumbered
        .passes
        .iter()
        .map(|pass| {
            (
                pass.id,
                GraphPassId::new(pass.id.get().saturating_add(100)).unwrap(),
            )
        })
        .collect::<HashMap<_, _>>();
    for pass in &mut renumbered.passes {
        pass.id = pass_id_map[&pass.id];
        pass.checkpoint_dependencies = pass
            .checkpoint_dependencies
            .iter()
            .map(|dependency| pass_id_map[dependency])
            .collect();
    }
    let renumbered_plan = effects::checkpoint_causal_stability_plan(&harness.renderer, &renumbered);
    assert!(
        renumbered_plan
            .instances
            .values()
            .all(|stability| stability.output_unchanged)
    );

    let mut semantic_change = graph.clone();
    semantic_change.instances[0].semantic_signature = semantic_change.instances[0]
        .semantic_signature
        .wrapping_add(1);
    let semantic_plan =
        effects::checkpoint_causal_stability_plan(&harness.renderer, &semantic_change);
    assert!(!semantic_plan.instances[&EffectInstanceId::new(31).unwrap()].output_unchanged);
    assert!(!semantic_plan.instances[&EffectInstanceId::new(32).unwrap()].output_unchanged);
    assert!(!semantic_plan.instances[&EffectInstanceId::new(33).unwrap()].output_unchanged);

    let mut continuous = graph.clone();
    continuous.instances[0].frame_demand = EffectFrameDemand::Continuous;
    let continuous_plan = effects::checkpoint_causal_stability_plan(&harness.renderer, &continuous);
    assert!(!continuous_plan.instances[&EffectInstanceId::new(31).unwrap()].output_unchanged);
    assert!(!continuous_plan.instances[&EffectInstanceId::new(32).unwrap()].output_unchanged);
    assert!(!continuous_plan.instances[&EffectInstanceId::new(33).unwrap()].output_unchanged);

    let mut changed_history = graph.clone();
    let c_id = EffectInstanceId::new(32).unwrap();
    let c_capture = changed_history
        .passes
        .iter_mut()
        .find(|pass| pass.kind == RenderPassKind::SceneCapture && pass.instance == c_id)
        .expect("C capture exists");
    c_capture.checkpoint_dependencies.clear();
    let history_plan =
        effects::checkpoint_causal_stability_plan(&harness.renderer, &changed_history);
    assert!(!history_plan.instances[&c_id].source_unchanged);
    assert!(!history_plan.instances[&EffectInstanceId::new(33).unwrap()].output_unchanged);

    let mut unsupported = graph.clone();
    let a_id = EffectInstanceId::new(31).unwrap();
    let mut extra_capture = unsupported
        .passes
        .iter()
        .find(|pass| pass.kind == RenderPassKind::SceneCapture && pass.instance == a_id)
        .expect("A capture exists")
        .clone();
    let next_id = unsupported
        .passes
        .iter()
        .map(|pass| pass.id.get())
        .max()
        .unwrap()
        .saturating_add(1);
    extra_capture.id = GraphPassId::new(next_id).unwrap();
    extra_capture.kind = RenderPassKind::SurfaceCapture;
    extra_capture.checkpoint_dependencies.clear();
    unsupported.passes.push(extra_capture);
    let unsupported_plan =
        effects::checkpoint_causal_stability_plan(&harness.renderer, &unsupported);
    assert!(!unsupported_plan.instances[&a_id].output_unchanged);
    assert!(!unsupported_plan.instances[&EffectInstanceId::new(32).unwrap()].output_unchanged);
    assert!(!unsupported_plan.instances[&EffectInstanceId::new(33).unwrap()].output_unchanged);

    promote_current_cache_causal_state(&mut harness.renderer, &unsupported);
    harness
        .renderer
        .effect_runtime
        .effect_resources
        .begin_checkpoint_frame();
    let unsupported_to_supported_plan =
        effects::checkpoint_causal_stability_plan(&harness.renderer, &graph);
    assert!(
        unsupported_to_supported_plan
            .instances
            .values()
            .all(|stability| !stability.output_unchanged)
    );
}

#[test]
fn later_overlapping_surface_change_zero_copies_earlier_checkpoints() {
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
    update_diagnostic_background_for_surface(
        &incremental,
        fixture.scene.background_surface,
        fixture.repair,
        [30, 50, 70, 255],
    );
    let first_graph = compile_native_three_checkpoint_graph(fixture, &full_region);
    let initial_signatures = signatures(fixture.scene, None);
    set_current_snapshot(&mut incremental.renderer, &initial_signatures);
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
    let previous = read_diagnostic_pixels(&incremental);
    let cached_c_before = cached_checkpoint_pixels(&mut incremental, &first_graph, 32);
    let cached_b_before = cached_checkpoint_pixels(&mut incremental, &first_graph, 33);

    update_diagnostic_background_for_surface(
        &incremental,
        fixture.scene.b_surface,
        OutputRect::new(0, 0, 2, 2),
        [236, 28, 42, 255],
    );
    let current_damage = diagnostic_region(fixture.repair);
    let current_graph = compile_native_three_checkpoint_graph(fixture, &current_damage);
    let current_signatures = signatures(fixture.scene, Some(fixture.scene.b_surface));
    set_current_snapshot(&mut incremental.renderer, &current_signatures);
    incremental
        .renderer
        .effect_runtime
        .effect_resources
        .begin_checkpoint_frame();
    let causal_plan =
        effects::checkpoint_causal_stability_plan(&incremental.renderer, &current_graph);
    let c_capture = current_graph
        .passes
        .iter()
        .find(|pass| {
            pass.kind == RenderPassKind::SceneCapture
                && pass.instance == oblivion_one::effects::EffectInstanceId::new(32).unwrap()
                && !pass.checkpoint_dependencies.is_empty()
        })
        .expect("C is a persistent checkpoint");
    assert!(causal_plan.captures[&c_capture.id].source_unchanged);
    let c_domain = current_graph
        .textures
        .iter()
        .find(|texture| Some(texture.id) == c_capture.output)
        .expect("C checkpoint output plan");
    assert!(
        !current_graph
            .final_damage
            .intersect_rect(c_domain.domain)
            .is_empty(),
        "later B damage overlaps the earlier C checkpoint domain"
    );
    let fallback_pixels = fallback_checkpoint_pixels(&current_graph, fixture.output_size);
    assert!(
        fallback_pixels > 0,
        "the conservative fallback has physical work"
    );

    let update_pixels = execute_diagnostic_frame_with_origin(
        &mut incremental,
        &current_graph,
        &diagnostic_repaint_plan_for_repairs_in_size(&[fixture.repair], false, fixture.output_size),
        current_damage,
        false,
        incremental_config,
        OutputFramebufferOrigin::TopLeftScanout,
    );
    assert_eq!(update_pixels, 0);
    assert!(update_pixels < fallback_pixels);
    assert_eq!(
        cached_checkpoint_pixels(&mut incremental, &current_graph, 32),
        cached_c_before,
        "earlier checkpoint texture stays unchanged"
    );
    assert_eq!(
        cached_checkpoint_pixels(&mut incremental, &current_graph, 33),
        cached_b_before,
        "B's backdrop source excludes its later surface command"
    );
    let actual = read_diagnostic_pixels(&incremental);

    incremental
        .renderer
        .discard_rendered(EglSceneFrameCommit::empty_for_test());
    let c_key = effects::checkpoint_capture_cache_key(&current_graph, c_capture)
        .expect("C checkpoint cache key remains stable");
    assert!(
        incremental
            .renderer
            .effect_runtime
            .effect_resources
            .checkpoint_capture_needs_full_refresh(
                &c_key,
                incremental
                    .renderer
                    .effect_runtime
                    .effect_resources
                    .checkpoint_frame_serial(),
            )
    );
    set_current_snapshot(&mut incremental.renderer, &initial_signatures);
    let restored_state_plan =
        effects::checkpoint_causal_stability_plan(&incremental.renderer, &current_graph);
    assert!(!restored_state_plan.captures[&c_capture.id].source_unchanged);
    incremental.renderer.frame_swap_failed();
    let swap_failed_plan =
        effects::checkpoint_causal_stability_plan(&incremental.renderer, &current_graph);
    assert!(!swap_failed_plan.captures[&c_capture.id].source_unchanged);
    drop(current_graph);
    drop(first_graph);
    drop(incremental);

    let mut uncached_reference =
        GlesEffectTestHarness::new(fixture.output_size.0, fixture.output_size.1);
    uncached_reference.install_texture_backed_output();
    install_native_three_checkpoint_diagnostic_scene(&mut uncached_reference, fixture.scene);
    update_diagnostic_background_for_surface(
        &uncached_reference,
        fixture.scene.b_surface,
        OutputRect::new(0, 0, 2, 2),
        [236, 28, 42, 255],
    );
    let reference_graph = compile_native_three_checkpoint_graph(fixture, &full_region);
    uncached_reference
        .renderer
        .effect_runtime
        .effect_resources
        .begin_checkpoint_frame();
    execute_diagnostic_frame_with_origin(
        &mut uncached_reference,
        &reference_graph,
        &diagnostic_repaint_plan_for_repairs_in_size(&[fixture.repair], true, fixture.output_size),
        full_region,
        true,
        full_capture_config,
        OutputFramebufferOrigin::TopLeftScanout,
    );
    let full_current = read_diagnostic_pixels(&uncached_reference);
    let (outside, inside) = diagnostic_matrix_mismatch_counts_for_origin(
        &actual,
        &previous,
        &full_current,
        fixture.output_size.0,
        fixture.output_size.1,
        &[fixture.repair],
        0,
        OutputFramebufferOrigin::TopLeftScanout,
    );
    assert_eq!(inside, 0, "incremental output matches full-current repair");
    assert_eq!(outside, 0, "incremental output is stable outside repair");
}

#[test]
fn earlier_scene_source_change_uses_the_conservative_checkpoint_update() {
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
    let initial_signatures = signatures(fixture.scene, None);
    set_current_snapshot(&mut incremental.renderer, &initial_signatures);
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
    let previous = read_diagnostic_pixels(&incremental);

    update_diagnostic_background_for_surface(
        &incremental,
        fixture.scene.background_surface,
        fixture.repair,
        [236, 28, 42, 255],
    );
    let current_damage = diagnostic_region(fixture.repair);
    let current_graph = compile_native_three_checkpoint_graph(fixture, &current_damage);
    let current_signatures = signatures(fixture.scene, Some(fixture.scene.background_surface));
    set_current_snapshot(&mut incremental.renderer, &current_signatures);
    incremental
        .renderer
        .effect_runtime
        .effect_resources
        .begin_checkpoint_frame();
    let causal_plan =
        effects::checkpoint_causal_stability_plan(&incremental.renderer, &current_graph);
    for instance_id in [32, 33] {
        let capture = current_graph
            .passes
            .iter()
            .find(|pass| {
                pass.kind == RenderPassKind::SceneCapture
                    && pass.instance == EffectInstanceId::new(instance_id).unwrap()
                    && !pass.checkpoint_dependencies.is_empty()
            })
            .expect("dependent checkpoint capture exists");
        assert!(
            !causal_plan.captures[&capture.id].source_unchanged,
            "earlier scene source change affects checkpoint {instance_id}"
        );
    }
    let fallback_pixels = fallback_checkpoint_pixels(&current_graph, fixture.output_size);
    assert!(
        fallback_pixels > 0,
        "the conservative fallback has physical work"
    );

    let update_pixels = execute_diagnostic_frame_with_origin(
        &mut incremental,
        &current_graph,
        &diagnostic_repaint_plan_for_repairs_in_size(&[fixture.repair], false, fixture.output_size),
        current_damage,
        false,
        incremental_config,
        OutputFramebufferOrigin::TopLeftScanout,
    );
    assert_eq!(update_pixels, fallback_pixels);
    let actual = read_diagnostic_pixels(&incremental);
    drop(current_graph);
    drop(first_graph);
    drop(incremental);

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
    let reference_graph = compile_native_three_checkpoint_graph(fixture, &full_region);
    uncached_reference
        .renderer
        .effect_runtime
        .effect_resources
        .begin_checkpoint_frame();
    execute_diagnostic_frame_with_origin(
        &mut uncached_reference,
        &reference_graph,
        &diagnostic_repaint_plan_for_repairs_in_size(&[fixture.repair], true, fixture.output_size),
        full_region,
        true,
        full_capture_config,
        OutputFramebufferOrigin::TopLeftScanout,
    );
    let full_current = read_diagnostic_pixels(&uncached_reference);
    let (outside, inside) = diagnostic_matrix_mismatch_counts_for_origin(
        &actual,
        &previous,
        &full_current,
        fixture.output_size.0,
        fixture.output_size.1,
        &[fixture.repair],
        0,
        OutputFramebufferOrigin::TopLeftScanout,
    );
    assert_eq!(
        inside, 0,
        "incremental output matches the full-current reference"
    );
    assert_eq!(outside, 0, "incremental output is stable outside repair");
}

#[test]
fn unpresented_intermediate_render_cannot_reuse_stale_presented_checkpoint_pixels() {
    let fixture = native_three_checkpoint_fixture();
    let config = effects::EffectDebugConfig::new_with_checkpoint_capture_path(
        effects::EffectDebugCaptureMode::Replay,
        effects::EffectDebugKawaseMode::Partial,
        effects::CheckpointCapturePath::FramebufferShaderCopy,
    );
    let full_region = EffectRegion::from_rect(fixture.output_bounds);
    let current_damage = diagnostic_region(fixture.repair);
    let initial_signatures = signatures(fixture.scene, None);
    let intermediate_signatures = signatures(fixture.scene, Some(fixture.scene.background_surface));
    let mut incremental = GlesEffectTestHarness::new(fixture.output_size.0, fixture.output_size.1);
    incremental.install_texture_backed_output();
    install_native_three_checkpoint_diagnostic_scene(&mut incremental, fixture.scene);
    update_diagnostic_background_for_surface(
        &incremental,
        fixture.scene.background_surface,
        fixture.repair,
        [30, 50, 70, 255],
    );

    let presented_graph = compile_native_three_checkpoint_graph(fixture, &full_region);
    render_checkpoint_test_frame(
        &mut incremental,
        &presented_graph,
        &initial_signatures,
        fixture.repair,
        fixture.output_size,
        full_region.clone(),
        true,
        config,
    );
    incremental
        .renderer
        .commit_presented(EglSceneFrameCommit::empty_for_test(), OutputDamage::Empty);

    update_diagnostic_background_for_surface(
        &incremental,
        fixture.scene.background_surface,
        fixture.repair,
        [220, 24, 36, 255],
    );
    let render_a_graph = compile_native_three_checkpoint_graph(fixture, &current_damage);
    render_checkpoint_test_frame(
        &mut incremental,
        &render_a_graph,
        &intermediate_signatures,
        fixture.repair,
        fixture.output_size,
        current_damage.clone(),
        false,
        config,
    );
    // Render A remains unpresented; its textures now contain Y.

    update_diagnostic_background_for_surface(
        &incremental,
        fixture.scene.background_surface,
        fixture.repair,
        [30, 50, 70, 255],
    );
    let render_b_graph = compile_native_three_checkpoint_graph(fixture, &current_damage);
    let fallback_pixels = fallback_checkpoint_pixels(&render_b_graph, fixture.output_size);
    assert!(fallback_pixels > 0);
    let update_stats = render_checkpoint_test_frame_with_stats(
        &mut incremental,
        &render_b_graph,
        &initial_signatures,
        fixture.repair,
        fixture.output_size,
        current_damage,
        false,
        config,
    );
    assert_eq!(
        update_stats.checkpoint_capture_execution_pixels, fallback_pixels,
        "B must update from its current X source instead of zero-copying A's Y cache"
    );
    assert_eq!(update_stats.checkpoint_cache_zero_copy_hits, 0);
    assert!(update_stats.checkpoint_causal_unproven > 0);

    let actual_output = read_diagnostic_pixels(&incremental);
    let actual_checkpoint_c = cached_checkpoint_pixels(&mut incremental, &render_b_graph, 32);
    let actual_checkpoint_b = cached_checkpoint_pixels(&mut incremental, &render_b_graph, 33);
    drop(incremental);
    let (reference_output, reference_checkpoint_c, reference_checkpoint_b) =
        full_current_checkpoint_reference(fixture, &initial_signatures, [30, 50, 70, 255], config);
    assert_eq!(actual_output, reference_output);
    assert_eq!(actual_checkpoint_c, reference_checkpoint_c);
    assert_eq!(actual_checkpoint_b, reference_checkpoint_b);
}

#[test]
fn unpresented_immediately_previous_render_can_zero_copy_unchanged_checkpoints() {
    let fixture = native_three_checkpoint_fixture();
    let config = effects::EffectDebugConfig::new_with_checkpoint_capture_path(
        effects::EffectDebugCaptureMode::Replay,
        effects::EffectDebugKawaseMode::Partial,
        effects::CheckpointCapturePath::FramebufferShaderCopy,
    );
    let full_region = EffectRegion::from_rect(fixture.output_bounds);
    let current_damage = diagnostic_region(fixture.repair);
    let initial_signatures = signatures(fixture.scene, None);
    let intermediate_signatures = signatures(fixture.scene, Some(fixture.scene.background_surface));
    let mut incremental = GlesEffectTestHarness::new(fixture.output_size.0, fixture.output_size.1);
    incremental.install_texture_backed_output();
    install_native_three_checkpoint_diagnostic_scene(&mut incremental, fixture.scene);
    update_diagnostic_background_for_surface(
        &incremental,
        fixture.scene.background_surface,
        fixture.repair,
        [30, 50, 70, 255],
    );

    let presented_graph = compile_native_three_checkpoint_graph(fixture, &full_region);
    render_checkpoint_test_frame(
        &mut incremental,
        &presented_graph,
        &initial_signatures,
        fixture.repair,
        fixture.output_size,
        full_region.clone(),
        true,
        config,
    );
    incremental
        .renderer
        .commit_presented(EglSceneFrameCommit::empty_for_test(), OutputDamage::Empty);

    update_diagnostic_background_for_surface(
        &incremental,
        fixture.scene.background_surface,
        fixture.repair,
        [220, 24, 36, 255],
    );
    let render_a_graph = compile_native_three_checkpoint_graph(fixture, &current_damage);
    let render_a_pixels = render_checkpoint_test_frame(
        &mut incremental,
        &render_a_graph,
        &intermediate_signatures,
        fixture.repair,
        fixture.output_size,
        current_damage.clone(),
        false,
        config,
    );
    assert!(render_a_pixels > 0);
    // Render A remains unpresented. Render B sees the same Y as A.

    let render_b_graph = compile_native_three_checkpoint_graph(fixture, &current_damage);
    let fallback_pixels = fallback_checkpoint_pixels(&render_b_graph, fixture.output_size);
    assert!(fallback_pixels > 0);
    let update_stats = render_checkpoint_test_frame_with_stats(
        &mut incremental,
        &render_b_graph,
        &intermediate_signatures,
        fixture.repair,
        fixture.output_size,
        current_damage,
        false,
        config,
    );
    assert_eq!(update_stats.checkpoint_capture_execution_pixels, 0);
    assert!(update_stats.checkpoint_cache_zero_copy_hits > 0);
    assert!(update_stats.checkpoint_causal_proven_unchanged > 0);

    let actual_output = read_diagnostic_pixels(&incremental);
    let actual_checkpoint_c = cached_checkpoint_pixels(&mut incremental, &render_b_graph, 32);
    let actual_checkpoint_b = cached_checkpoint_pixels(&mut incremental, &render_b_graph, 33);
    drop(incremental);
    let (reference_output, reference_checkpoint_c, reference_checkpoint_b) =
        full_current_checkpoint_reference(
            fixture,
            &intermediate_signatures,
            [220, 24, 36, 255],
            config,
        );
    let max_output_delta = actual_output
        .iter()
        .zip(&reference_output)
        .map(|(actual, expected)| actual.abs_diff(*expected))
        .max()
        .unwrap_or_default();
    assert!(
        max_output_delta <= 1,
        "unpresented render differs from the full-current reference by up to {max_output_delta} channel values"
    );
    let max_checkpoint_c_delta = actual_checkpoint_c
        .iter()
        .zip(&reference_checkpoint_c)
        .map(|(actual, expected)| actual.abs_diff(*expected))
        .max()
        .unwrap_or_default();
    assert!(
        max_checkpoint_c_delta <= 1,
        "checkpoint C differs from the full-current reference by up to {max_checkpoint_c_delta} channel values"
    );
    let max_checkpoint_b_delta = actual_checkpoint_b
        .iter()
        .zip(&reference_checkpoint_b)
        .map(|(actual, expected)| actual.abs_diff(*expected))
        .max()
        .unwrap_or_default();
    assert!(
        max_checkpoint_b_delta <= 1,
        "checkpoint B differs from the full-current reference by up to {max_checkpoint_b_delta} channel values"
    );
}

#[test]
fn churning_checkpoint_fusion_materializes_then_recovers_zero_copy_when_stable() {
    let fixture = native_three_checkpoint_fixture();
    let config = effects::EffectDebugConfig::new_with_checkpoint_capture_path(
        effects::EffectDebugCaptureMode::Replay,
        effects::EffectDebugKawaseMode::Partial,
        effects::CheckpointCapturePath::FramebufferShaderCopy,
    );
    let full_region = EffectRegion::from_rect(fixture.output_bounds);
    let current_damage = diagnostic_region(fixture.repair);
    let initial_signatures = signatures(fixture.scene, None);
    let seed_graph = compile_native_three_checkpoint_graph(fixture, &full_region);
    let mut first_churning_graph = compile_native_three_checkpoint_graph(fixture, &current_damage);
    shift_checkpoint_capture_domains(&mut first_churning_graph);
    let mut second_churning_graph = first_churning_graph.clone();
    shift_checkpoint_capture_domains(&mut second_churning_graph);

    let mut incremental = GlesEffectTestHarness::new(fixture.output_size.0, fixture.output_size.1);
    incremental.install_texture_backed_output();
    install_native_three_checkpoint_diagnostic_scene(&mut incremental, fixture.scene);
    update_diagnostic_background_for_surface(
        &incremental,
        fixture.scene.background_surface,
        fixture.repair,
        [30, 50, 70, 255],
    );

    // Seed each capture family with the preceding layout. The moved
    // layout below creates new exact cache keys in those same families.
    render_checkpoint_test_frame(
        &mut incremental,
        &seed_graph,
        &initial_signatures,
        fixture.repair,
        fixture.output_size,
        full_region.clone(),
        true,
        config,
    );
    incremental
        .renderer
        .commit_presented(EglSceneFrameCommit::empty_for_test(), OutputDamage::Empty);

    update_diagnostic_background_for_surface(
        &incremental,
        fixture.scene.background_surface,
        fixture.repair,
        [220, 24, 36, 255],
    );
    let changed_signatures = signatures(fixture.scene, Some(fixture.scene.background_surface));

    let capture_a = first_churning_graph
        .passes
        .iter()
        .find(|pass| {
            pass.kind == RenderPassKind::SceneCapture
                && pass.instance == EffectInstanceId::new(33).unwrap()
                && !pass.checkpoint_dependencies.is_empty()
        })
        .unwrap();
    let seed_capture_a = seed_graph
        .passes
        .iter()
        .find(|pass| pass.id == capture_a.id)
        .unwrap();
    assert_ne!(
        effects::checkpoint_capture_cache_key(&seed_graph, seed_capture_a),
        effects::checkpoint_capture_cache_key(&first_churning_graph, capture_a),
        "movement changes the exact persistent cache key"
    );
    let key_a = effects::checkpoint_capture_cache_key(&first_churning_graph, capture_a)
        .expect("first churning identity has a persistent cache key");

    let frame_a = render_checkpoint_test_frame_with_stats(
        &mut incremental,
        &first_churning_graph,
        &changed_signatures,
        fixture.repair,
        fixture.output_size,
        current_damage.clone(),
        false,
        config,
    );
    assert!(frame_a.capture_downsample_fusion_candidates > 0);
    assert!(frame_a.capture_downsample_fusion_executed > 0);
    assert_eq!(frame_a.checkpoint_capture_execution_pixels, 0);
    assert!(frame_a.capture_downsample_fusion_elided_capture_pixels > 0);
    let frame_a_serial = incremental
        .renderer
        .effect_runtime
        .effect_resources
        .checkpoint_frame_serial();
    assert!(
        incremental
            .renderer
            .effect_runtime
            .effect_resources
            .checkpoint_capture_needs_full_refresh(&key_a, frame_a_serial.saturating_add(1))
    );

    // Move again before the first identity can stabilize. The new key
    // in frame B must also fuse and remain unpublished.
    let capture_b = second_churning_graph
        .passes
        .iter()
        .find(|pass| pass.id == capture_a.id)
        .expect("same logical capture exists in the second layout");
    let key_b = effects::checkpoint_capture_cache_key(&second_churning_graph, capture_b)
        .expect("second churning identity has a persistent cache key");
    assert_ne!(key_a, key_b);
    let frame_b = render_checkpoint_test_frame_with_stats(
        &mut incremental,
        &second_churning_graph,
        &changed_signatures,
        fixture.repair,
        fixture.output_size,
        current_damage.clone(),
        false,
        config,
    );
    assert!(frame_b.capture_downsample_fusion_candidates > 0);
    assert!(frame_b.capture_downsample_fusion_executed > 0);
    assert_eq!(frame_b.checkpoint_capture_execution_pixels, 0);
    assert!(frame_b.capture_downsample_fusion_elided_capture_pixels > 0);
    let frame_b_serial = incremental
        .renderer
        .effect_runtime
        .effect_resources
        .checkpoint_frame_serial();
    assert!(
        incremental
            .renderer
            .effect_runtime
            .effect_resources
            .checkpoint_capture_needs_full_refresh(&key_b, frame_b_serial.saturating_add(1))
    );

    // Once the identity stops moving, the ordinary path materializes
    // it. The following unchanged frame can then use causal zero-copy.
    let frame_c = render_checkpoint_test_frame_with_stats(
        &mut incremental,
        &second_churning_graph,
        &changed_signatures,
        fixture.repair,
        fixture.output_size,
        current_damage.clone(),
        false,
        config,
    );
    assert_eq!(frame_c.capture_downsample_fusion_executed, 0);
    assert!(frame_c.checkpoint_capture_execution_pixels > 0);
    let frame_c_serial = incremental
        .renderer
        .effect_runtime
        .effect_resources
        .checkpoint_frame_serial();
    assert!(
        !incremental
            .renderer
            .effect_runtime
            .effect_resources
            .checkpoint_capture_needs_full_refresh(&key_b, frame_c_serial.saturating_add(1))
    );

    set_current_snapshot(&mut incremental.renderer, &changed_signatures);
    incremental
        .renderer
        .effect_runtime
        .effect_resources
        .begin_checkpoint_frame();
    let frame_d_demand = oblivion_one::effects::plan_effect_execution_demand_with_kawase_mode(
        &second_churning_graph,
        &current_damage,
        true,
        config.kawase_mode() == effects::EffectDebugKawaseMode::Full,
    );
    let frame_d_selection =
        effects::select_effect_execution(&second_churning_graph, &frame_d_demand);
    let frame_d_repaint =
        diagnostic_repaint_plan_for_repairs_in_size(&[fixture.repair], false, fixture.output_size);
    let frame_d = effects::execute_effect_graph_with_debug_config(
        &mut incremental.renderer,
        &second_churning_graph,
        OutputFramebufferOrigin::TopLeftScanout,
        &frame_d_repaint,
        &frame_d_demand,
        &frame_d_selection,
        config,
    )
    .expect("frame C executes stable checkpoint graph passes");
    assert_eq!(frame_d.capture_downsample_fusion_executed, 0);
    assert!(
        frame_d.checkpoint_cache_zero_copy_hits > 0,
        "zero-copy misses: hits={} cache_hits={} update_pixels={} proven={} unproven={} dependency_changed={} scene_prefix_changed={}",
        frame_d.checkpoint_cache_zero_copy_hits,
        frame_d.checkpoint_cache_hits,
        frame_d.checkpoint_cache_update_pixels,
        frame_d.checkpoint_causal_proven_unchanged,
        frame_d.checkpoint_causal_unproven,
        frame_d.checkpoint_causal_dependency_changed,
        frame_d.checkpoint_causal_scene_prefix_changed
    );
    assert!(frame_d.checkpoint_causal_proven_unchanged > 0);
}

#[test]
fn fused_downsample_failure_keeps_checkpoint_invalid_and_restores_renderer_state() {
    let fixture = native_three_checkpoint_fixture();
    let config = effects::EffectDebugConfig::new_with_checkpoint_capture_path(
        effects::EffectDebugCaptureMode::Replay,
        effects::EffectDebugKawaseMode::Partial,
        effects::CheckpointCapturePath::FramebufferShaderCopy,
    );
    let full_region = EffectRegion::from_rect(fixture.output_bounds);
    let current_damage = diagnostic_region(fixture.repair);
    let signatures = signatures(fixture.scene, None);
    let seed_graph = compile_native_three_checkpoint_graph(fixture, &full_region);
    let mut churning_graph = compile_native_three_checkpoint_graph(fixture, &current_damage);
    shift_checkpoint_capture_domains(&mut churning_graph);

    let capture = churning_graph
        .passes
        .iter()
        .find(|pass| {
            pass.kind == RenderPassKind::SceneCapture
                && pass.instance == EffectInstanceId::new(33).unwrap()
                && !pass.checkpoint_dependencies.is_empty()
        })
        .unwrap();
    let cache_key = effects::checkpoint_capture_cache_key(&churning_graph, capture).unwrap();
    let mut harness = GlesEffectTestHarness::new(fixture.output_size.0, fixture.output_size.1);
    harness.install_texture_backed_output();
    install_native_three_checkpoint_diagnostic_scene(&mut harness, fixture.scene);

    render_checkpoint_test_frame(
        &mut harness,
        &seed_graph,
        &signatures,
        fixture.repair,
        fixture.output_size,
        full_region,
        true,
        config,
    );

    let fused_shader_key = effects::ShaderProgramKey::new(
        oblivion_one::effects::ShaderModuleId::new(
            oblivion_one::effects::INTERNAL_EFFECT_SHADER_MODULE_DOWNSAMPLE,
        )
        .unwrap(),
        2,
        oblivion_one::effects::EffectWorkingSpace::LinearSrgb,
    );
    harness
        .renderer
        .effect_runtime
        .effect_shaders
        .force_uniform_missing_for_test(fused_shader_key, "u_effect_capture_origin_bottom_left")
        .unwrap();

    set_current_snapshot(&mut harness.renderer, &signatures);
    harness
        .renderer
        .effect_runtime
        .effect_resources
        .begin_checkpoint_frame();
    let demand = oblivion_one::effects::plan_effect_execution_demand_with_kawase_mode(
        &churning_graph,
        &current_damage,
        false,
        config.kawase_mode() == effects::EffectDebugKawaseMode::Full,
    );
    let selection = effects::select_effect_execution(&churning_graph, &demand);
    let repaint =
        diagnostic_repaint_plan_for_repairs_in_size(&[fixture.repair], false, fixture.output_size);
    let failed_result = effects::execute_effect_graph_with_debug_config(
        &mut harness.renderer,
        &churning_graph,
        OutputFramebufferOrigin::TopLeftScanout,
        &repaint,
        &demand,
        &selection,
        config,
    );
    assert!(
        failed_result.is_err(),
        "the injected fused uniform failure must fail the graph"
    );

    unsafe {
        assert_eq!(
            harness.renderer.gl.get_parameter_i32(glow::ACTIVE_TEXTURE),
            glow::TEXTURE0 as i32
        );
        assert_eq!(
            harness
                .renderer
                .gl
                .get_parameter_i32(glow::TEXTURE_BINDING_2D),
            0
        );
        assert!(harness.renderer.gl.is_enabled(glow::BLEND));
        assert!(!harness.renderer.gl.is_enabled(glow::SCISSOR_TEST));
        let mut viewport = [0; 4];
        harness
            .renderer
            .gl
            .get_parameter_i32_slice(glow::VIEWPORT, &mut viewport);
        assert_eq!(
            viewport,
            [
                0,
                0,
                fixture.output_size.0 as i32,
                fixture.output_size.1 as i32,
            ]
        );
    }

    let failed_serial = harness
        .renderer
        .effect_runtime
        .effect_resources
        .checkpoint_frame_serial();
    assert!(
        harness
            .renderer
            .effect_runtime
            .effect_resources
            .checkpoint_capture_needs_full_refresh(&cache_key, failed_serial)
    );

    let retry = render_checkpoint_test_frame_with_stats(
        &mut harness,
        &churning_graph,
        &signatures,
        fixture.repair,
        fixture.output_size,
        current_damage,
        false,
        config,
    );
    assert_eq!(retry.capture_downsample_fusion_executed, 0);
    assert!(retry.checkpoint_capture_execution_pixels > 0);
    assert_eq!(retry.checkpoint_cache_zero_copy_hits, 0);
    let retry_serial = harness
        .renderer
        .effect_runtime
        .effect_resources
        .checkpoint_frame_serial();
    assert!(
        !harness
            .renderer
            .effect_runtime
            .effect_resources
            .checkpoint_capture_needs_full_refresh(&cache_key, retry_serial.saturating_add(1))
    );
}

#[test]
fn late_presentation_of_older_render_cannot_rewind_checkpoint_cache_causality() {
    let fixture = native_three_checkpoint_fixture();
    let config = effects::EffectDebugConfig::new_with_checkpoint_capture_path(
        effects::EffectDebugCaptureMode::Replay,
        effects::EffectDebugKawaseMode::Partial,
        effects::CheckpointCapturePath::FramebufferShaderCopy,
    );
    let full_region = EffectRegion::from_rect(fixture.output_bounds);
    let current_damage = diagnostic_region(fixture.repair);
    let initial_signatures = signatures(fixture.scene, None);
    let render_a_signatures = signatures(fixture.scene, Some(fixture.scene.background_surface));
    let mut render_b_signatures = render_a_signatures.clone();
    let changed_background = render_b_signatures
        .iter_mut()
        .find(|signature| signature.surface_id == fixture.scene.background_surface)
        .expect("background surface signature exists");
    changed_background.commit_sequence = 3;
    changed_background.buffer_id = u64::from(fixture.scene.background_surface) * 100 + 3;
    changed_background.generation = 3;
    let mut incremental = GlesEffectTestHarness::new(fixture.output_size.0, fixture.output_size.1);
    incremental.install_texture_backed_output();
    install_native_three_checkpoint_diagnostic_scene(&mut incremental, fixture.scene);

    let presented_graph = compile_native_three_checkpoint_graph(fixture, &full_region);
    render_checkpoint_test_frame(
        &mut incremental,
        &presented_graph,
        &initial_signatures,
        fixture.repair,
        fixture.output_size,
        full_region.clone(),
        true,
        config,
    );
    incremental
        .renderer
        .commit_presented(EglSceneFrameCommit::empty_for_test(), OutputDamage::Empty);

    update_diagnostic_background_for_surface(
        &incremental,
        fixture.scene.background_surface,
        fixture.repair,
        [220, 24, 36, 255],
    );
    let render_a_graph = compile_native_three_checkpoint_graph(fixture, &current_damage);
    render_checkpoint_test_frame(
        &mut incremental,
        &render_a_graph,
        &render_a_signatures,
        fixture.repair,
        fixture.output_size,
        current_damage.clone(),
        false,
        config,
    );
    let late_render_a_commit = EglSceneFrameCommit::empty_for_test();

    update_diagnostic_background_for_surface(
        &incremental,
        fixture.scene.background_surface,
        fixture.repair,
        [170, 40, 210, 255],
    );
    let render_b_graph = compile_native_three_checkpoint_graph(fixture, &current_damage);
    render_checkpoint_test_frame(
        &mut incremental,
        &render_b_graph,
        &render_b_signatures,
        fixture.repair,
        fixture.output_size,
        current_damage.clone(),
        false,
        config,
    );

    incremental
        .renderer
        .commit_presented(late_render_a_commit, OutputDamage::Empty);
    set_current_snapshot(&mut incremental.renderer, &render_b_signatures);
    incremental
        .renderer
        .effect_runtime
        .effect_resources
        .begin_checkpoint_frame();
    let next_plan =
        effects::checkpoint_causal_stability_plan(&incremental.renderer, &render_b_graph);
    let b_capture = render_b_graph
        .passes
        .iter()
        .find(|pass| {
            pass.kind == RenderPassKind::SceneCapture
                && pass.instance == EffectInstanceId::new(32).unwrap()
                && !pass.checkpoint_dependencies.is_empty()
        })
        .expect("B frame includes the persistent checkpoint");
    assert!(
        next_plan.captures[&b_capture.id].source_unchanged,
        "the next comparison must use render B's cache causal baseline"
    );
}

#[test]
fn checkpoint_causal_invalidation_and_serial_gaps_fail_closed() {
    let fixture = native_three_checkpoint_fixture();
    let mut harness = GlesEffectTestHarness::new(fixture.output_size.0, fixture.output_size.1);
    install_native_three_checkpoint_diagnostic_scene(&mut harness, fixture.scene);
    let graph = compile_native_three_checkpoint_graph(
        fixture,
        &EffectRegion::from_rect(fixture.output_bounds),
    );
    let surface_signatures = signatures(fixture.scene, None);
    set_current_snapshot(&mut harness.renderer, &surface_signatures);
    harness
        .renderer
        .effect_runtime
        .effect_resources
        .begin_checkpoint_frame();
    promote_current_cache_causal_state(&mut harness.renderer, &graph);

    let capture = graph
        .passes
        .iter()
        .find(|pass| {
            pass.kind == RenderPassKind::SceneCapture
                && pass.instance == EffectInstanceId::new(32).unwrap()
                && !pass.checkpoint_dependencies.is_empty()
        })
        .expect("checkpoint capture exists");
    harness
        .renderer
        .effect_runtime
        .effect_resources
        .begin_checkpoint_frame();
    let adjacent = effects::checkpoint_causal_stability_plan(&harness.renderer, &graph);
    assert!(adjacent.captures[&capture.id].source_unchanged);

    harness
        .renderer
        .discard_rendered(EglSceneFrameCommit::empty_for_test());
    let discarded = effects::checkpoint_causal_stability_plan(&harness.renderer, &graph);
    assert!(!discarded.captures[&capture.id].source_unchanged);

    promote_current_cache_causal_state(&mut harness.renderer, &graph);
    harness
        .renderer
        .effect_runtime
        .effect_resources
        .begin_checkpoint_frame();
    assert!(
        effects::checkpoint_causal_stability_plan(&harness.renderer, &graph).captures[&capture.id]
            .source_unchanged
    );
    harness
        .renderer
        .effect_runtime
        .effect_resources
        .clear_checkpoint_capture_cache();
    let cleared = effects::checkpoint_causal_stability_plan(&harness.renderer, &graph);
    assert!(!cleared.captures[&capture.id].source_unchanged);

    promote_current_cache_causal_state(&mut harness.renderer, &graph);
    harness
        .renderer
        .effect_runtime
        .effect_resources
        .begin_checkpoint_frame();
    harness
        .renderer
        .effect_runtime
        .effect_resources
        .begin_checkpoint_frame();
    let skipped_serial = effects::checkpoint_causal_stability_plan(&harness.renderer, &graph);
    assert!(!skipped_serial.captures[&capture.id].source_unchanged);
}
