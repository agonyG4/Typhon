use super::*;

fn retained_surface_signature(surface_id: u32, generation: u64) -> EglSceneSurfaceSignature {
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

fn retained_signatures(
    scene: NativeThreeCheckpointSceneSpec,
    changed: &[u32],
) -> Vec<EglSceneSurfaceSignature> {
    [
        scene.background_surface,
        scene.a_surface,
        scene.c_surface,
        scene.b_surface,
    ]
    .into_iter()
    .map(|surface_id| {
        retained_surface_signature(
            surface_id,
            if changed.contains(&surface_id) { 2 } else { 1 },
        )
    })
    .collect()
}

/// Renders one frame like the ordinary planner does, optionally letting the
/// retained output cache skip passes. Returns the retained hits.
#[allow(clippy::too_many_arguments)]
fn render_retained_test_frame(
    harness: &mut GlesEffectTestHarness,
    graph: &oblivion_one::effects::CompiledFrameGraph,
    surface_signatures: &[EglSceneSurfaceSignature],
    repair: OutputRect,
    output_size: (u32, u32),
    region: EffectRegion,
    full: bool,
    retained: bool,
    rerun_invalidated: bool,
    scene_damage: Option<EffectRegion>,
) -> (usize, usize) {
    harness.renderer.scene_state.current_scene_damage = scene_damage;
    let config = effects::EffectDebugConfig::new_with_checkpoint_capture_path(
        effects::EffectDebugCaptureMode::Replay,
        effects::EffectDebugKawaseMode::Partial,
        effects::CheckpointCapturePath::FramebufferShaderCopy,
    );
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
    harness
        .renderer
        .effect_runtime
        .effect_resources
        .begin_checkpoint_frame();
    let demand = oblivion_one::effects::plan_effect_execution_demand_with_kawase_mode(
        graph, &region, full, false,
    );
    let mut selection = effects::select_effect_execution(graph, &demand);
    let mut hits = 0;
    if retained {
        effects::plan_retained_effect_outputs(
            &harness.renderer.scene_state,
            &mut harness.renderer.effect_runtime,
            graph,
            &demand,
            &mut selection,
        );
        hits = harness
            .renderer
            .effect_runtime
            .retained_output_plan
            .as_ref()
            .map_or(0, |plan| plan.hits());
    }
    let passes = selection.executed_passes.len();
    effects::execute_effect_graph_with_debug_config(
        &mut harness.renderer,
        graph,
        OutputFramebufferOrigin::TopLeftScanout,
        &diagnostic_repaint_plan_for_repairs_in_size(&[repair], full, output_size),
        &demand,
        &selection,
        config,
    )
    .expect("retained output test frame renders");
    if rerun_invalidated {
        // A lifecycle baseline executes the same graph and selection again in
        // the same frame, possibly after an effect path invalidated the cache.
        harness
            .renderer
            .effect_runtime
            .effect_resources
            .invalidate_checkpoint_capture_contents();
        effects::execute_effect_graph_with_debug_config(
            &mut harness.renderer,
            graph,
            OutputFramebufferOrigin::TopLeftScanout,
            &diagnostic_repaint_plan_for_repairs_in_size(&[repair], full, output_size),
            &demand,
            &selection,
            config,
        )
        .expect("retained output test frame renders again");
    }
    (hits, passes)
}

struct RetainedRun {
    pixels: [Vec<u8>; 3],
    hits: [usize; 3],
    passes: [usize; 3],
    entries_after_first: usize,
}

/// Renders the same three frames as the ordinary planner would. The harness
/// holds the global EGL test lock, so each run owns the GPU alone.
fn run_retained_sequence(retained: bool, rerun_invalidated: bool) -> RetainedRun {
    let fixture = native_three_checkpoint_fixture();
    let full_region = EffectRegion::from_rect(fixture.output_bounds);
    let damage = diagnostic_region(fixture.repair);
    let mut harness = GlesEffectTestHarness::new(fixture.output_size.0, fixture.output_size.1);
    harness.install_texture_backed_output();
    install_native_three_checkpoint_diagnostic_scene(&mut harness, fixture.scene);
    update_diagnostic_background_for_surface(
        &harness,
        fixture.scene.background_surface,
        fixture.repair,
        [30, 50, 70, 255],
    );

    // Frame 1: everything renders and the cache fills.
    let first_graph = compile_native_three_checkpoint_graph(fixture, &full_region);
    let (hits_1, passes_1) = render_retained_test_frame(
        &mut harness,
        &first_graph,
        &retained_signatures(fixture.scene, &[]),
        fixture.repair,
        fixture.output_size,
        full_region.clone(),
        true,
        retained,
        false,
        None,
    );
    let pixels_1 = read_diagnostic_pixels(&harness);
    let (entries_after_first, _) = harness
        .renderer
        .effect_runtime
        .effect_resources
        .retained_effect_output_stats();

    // Frame 2: the topmost surface changes; earlier backdrops do not.
    let second_graph = compile_native_three_checkpoint_graph(fixture, &damage);
    update_diagnostic_background_for_surface(
        &harness,
        fixture.scene.b_surface,
        OutputRect::new(0, 0, 2, 2),
        [236, 28, 42, 255],
    );
    let (hits_2, passes_2) = render_retained_test_frame(
        &mut harness,
        &second_graph,
        &retained_signatures(fixture.scene, &[fixture.scene.b_surface]),
        fixture.repair,
        fixture.output_size,
        damage.clone(),
        false,
        retained,
        rerun_invalidated,
        None,
    );
    let pixels_2 = read_diagnostic_pixels(&harness);

    // Frame 3: the background below every effect changes, so nothing is
    // proven unchanged.
    update_diagnostic_background_for_surface(
        &harness,
        fixture.scene.background_surface,
        fixture.repair,
        [200, 180, 20, 255],
    );
    let (hits_3, passes_3) = render_retained_test_frame(
        &mut harness,
        &second_graph,
        &retained_signatures(
            fixture.scene,
            &[fixture.scene.b_surface, fixture.scene.background_surface],
        ),
        fixture.repair,
        fixture.output_size,
        damage,
        false,
        retained,
        false,
        None,
    );
    let pixels_3 = read_diagnostic_pixels(&harness);
    RetainedRun {
        pixels: [pixels_1, pixels_2, pixels_3],
        hits: [hits_1, hits_2, hits_3],
        passes: [passes_1, passes_2, passes_3],
        entries_after_first,
    }
}

#[test]
fn retained_outputs_match_uncached_pixels_and_fail_closed_on_backdrop_change() {
    let cached = run_retained_sequence(true, false);
    let uncached = run_retained_sequence(false, false);

    assert_eq!(
        cached.hits[0], 0,
        "nothing is retained before the first frame"
    );
    assert!(
        cached.entries_after_first > 0,
        "the first frame retains outputs"
    );
    assert!(
        cached.hits[1] > 0,
        "unchanged backdrops composite from retained outputs"
    );
    assert!(
        cached.passes[1] < uncached.passes[1],
        "retained hits skip passes: cached={:?} uncached={:?}",
        cached.passes,
        uncached.passes
    );
    assert_eq!(
        cached.hits[2], 0,
        "a changed backdrop never reuses retained pixels"
    );
    for frame in 0..3 {
        assert!(
            cached.pixels[frame] == uncached.pixels[frame],
            "frame {} {}",
            frame + 1,
            diagnostic_difference(&cached.pixels[frame], &uncached.pixels[frame])
        );
    }
}

/// The lifecycle baseline runs the main graph twice in one frame, and an
/// effect path may invalidate the caches in between. Hit instances have no
/// capture or blur passes left, so both runs must still composite from the
/// retained pixels instead of an unrendered texture.
#[test]
fn retained_outputs_survive_a_second_execution_in_the_same_frame() {
    let cached = run_retained_sequence(true, true);
    let uncached = run_retained_sequence(false, true);
    assert!(
        cached.hits[1] > 0,
        "unchanged backdrops composite from retained outputs"
    );
    for frame in 0..3 {
        assert!(
            cached.pixels[frame] == uncached.pixels[frame],
            "frame {} {}",
            frame + 1,
            diagnostic_difference(&cached.pixels[frame], &uncached.pixels[frame])
        );
    }
}

/// The three-checkpoint scene on an output wide enough that its far corner
/// lies beyond every effect's sampling footprint.
fn wide_three_checkpoint_fixture() -> NativeThreeCheckpointFixture {
    let mut fixture = native_three_checkpoint_fixture();
    fixture.output_size = (480, 360);
    fixture.output_bounds =
        EffectRect::new(0, 0, 480, 360).expect("wide three-checkpoint output bounds");
    fixture
}

/// A full repaint where only the background surface's pixels change inside
/// `changed`, as when a window below a shell blur scrolls. Returns the second
/// frame's pixels and retained hits.
fn run_surface_content_change(
    retained: bool,
    changed: OutputRect,
) -> (Vec<u8>, usize, oblivion_one::effects::CompiledFrameGraph) {
    let fixture = wide_three_checkpoint_fixture();
    let full_region = EffectRegion::from_rect(fixture.output_bounds);
    let full_repair = OutputRect::new(0, 0, fixture.output_size.0, fixture.output_size.1);
    let mut harness = GlesEffectTestHarness::new(fixture.output_size.0, fixture.output_size.1);
    harness.install_texture_backed_output();
    install_native_three_checkpoint_diagnostic_scene(&mut harness, fixture.scene);

    let first_graph = compile_native_three_checkpoint_graph(fixture, &full_region);
    render_retained_test_frame(
        &mut harness,
        &first_graph,
        &retained_signatures(fixture.scene, &[]),
        full_repair,
        fixture.output_size,
        full_region.clone(),
        true,
        retained,
        false,
        None,
    );

    let damage = diagnostic_region(changed);
    let second_graph = compile_native_three_checkpoint_graph(fixture, &damage);
    update_diagnostic_background_for_surface(
        &harness,
        fixture.scene.background_surface,
        changed,
        [236, 28, 42, 255],
    );
    let (hits, _) = render_retained_test_frame(
        &mut harness,
        &second_graph,
        &retained_signatures(fixture.scene, &[fixture.scene.background_surface]),
        full_repair,
        fixture.output_size,
        full_region,
        true,
        retained,
        false,
        Some(damage),
    );
    (read_diagnostic_pixels(&harness), hits, second_graph)
}

#[test]
fn retained_outputs_hit_when_surface_damage_misses_the_sampled_region() {
    let fixture = wide_three_checkpoint_fixture();
    let far_corner = OutputRect::new(
        fixture.output_size.0 as i32 - 4,
        fixture.output_size.1 as i32 - 4,
        4,
        4,
    );
    let (cached, hits, graph) = run_surface_content_change(true, far_corner);
    let (uncached, _, _) = run_surface_content_change(false, far_corner);
    let effect_a = graph
        .instances
        .iter()
        .find(|instance| instance.id.get() == fixture.effect_a.id)
        .expect("effect A is compiled");
    assert!(
        !effect_a
            .capture_region
            .intersects(&diagnostic_region(far_corner)),
        "the fixture's far corner must lie outside A's sampled region: {:?}",
        graph
            .instances
            .iter()
            .map(|i| (i.id, i.capture_region.bounding_rect()))
            .collect::<Vec<_>>()
    );
    assert!(
        hits > 0,
        "a backdrop whose sampled pixels saw no damage composites from retained output"
    );
    assert!(
        cached == uncached,
        "{}",
        diagnostic_difference(&cached, &uncached)
    );
}

#[test]
fn retained_outputs_miss_when_surface_damage_reaches_the_sampled_region() {
    let fixture = wide_three_checkpoint_fixture();
    let (cached, hits, _) = run_surface_content_change(true, fixture.repair);
    let (uncached, _, _) = run_surface_content_change(false, fixture.repair);
    assert_eq!(
        hits, 0,
        "damage inside every sampled region never reuses retained pixels"
    );
    assert!(
        cached == uncached,
        "{}",
        diagnostic_difference(&cached, &uncached)
    );
}

fn diagnostic_difference(left: &[u8], right: &[u8]) -> String {
    let differing = left
        .chunks_exact(4)
        .zip(right.chunks_exact(4))
        .filter(|(left, right)| left != right)
        .count();
    format!("differing_pixels={differing}")
}
