use super::super::*;
use super::*;

pub(in crate::egl_renderer::tests) fn execute_diagnostic_frame(
    harness: &mut GlesEffectTestHarness,
    graph: &oblivion_one::effects::CompiledFrameGraph,
    plan: &RepaintPlan,
    region: EffectRegion,
    conservative_full: bool,
    config: effects::EffectDebugConfig,
) -> u64 {
    execute_diagnostic_frame_with_origin(
        harness,
        graph,
        plan,
        region,
        conservative_full,
        config,
        OutputFramebufferOrigin::BottomLeft,
    )
}

pub(in crate::egl_renderer::tests) fn execute_diagnostic_frame_with_origin(
    harness: &mut GlesEffectTestHarness,
    graph: &oblivion_one::effects::CompiledFrameGraph,
    plan: &RepaintPlan,
    region: EffectRegion,
    conservative_full: bool,
    config: effects::EffectDebugConfig,
    framebuffer_origin: OutputFramebufferOrigin,
) -> u64 {
    execute_diagnostic_frame_with_origin_and_scene_replay_mode(
        harness,
        graph,
        plan,
        region,
        conservative_full,
        config,
        framebuffer_origin,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub(in crate::egl_renderer::tests) fn execute_diagnostic_frame_with_origin_and_scene_replay_mode(
    harness: &mut GlesEffectTestHarness,
    graph: &oblivion_one::effects::CompiledFrameGraph,
    plan: &RepaintPlan,
    region: EffectRegion,
    conservative_full: bool,
    config: effects::EffectDebugConfig,
    framebuffer_origin: OutputFramebufferOrigin,
    scene_replay_work_mode: Option<effects::SceneReplayWorkMode>,
) -> u64 {
    let demand = oblivion_one::effects::plan_effect_execution_demand_with_kawase_mode(
        graph,
        &region,
        conservative_full,
        config.kawase_mode() == effects::EffectDebugKawaseMode::Full,
    );
    let selection = effects::select_effect_execution(graph, &demand);
    let stats = match scene_replay_work_mode {
        Some(mode) => effects::execute_effect_graph_with_debug_config_and_scene_replay_mode(
            &mut harness.renderer,
            graph,
            framebuffer_origin,
            plan,
            &demand,
            &selection,
            config,
            mode,
        ),
        None => effects::execute_effect_graph_with_debug_config(
            &mut harness.renderer,
            graph,
            framebuffer_origin,
            plan,
            &demand,
            &selection,
            config,
        ),
    }
    .expect("diagnostic frame renders");
    stats.checkpoint_capture_execution_pixels
}

pub(in crate::egl_renderer::tests) fn render_native_stacked_candidate(
    fixture: NativeStackedDiagnosticFixture,
    config: effects::EffectDebugConfig,
    poison: Option<[f32; 4]>,
) -> (
    oblivion_one::effects::CompiledFrameGraph,
    Vec<u8>,
    Vec<u8>,
    Vec<String>,
) {
    render_native_stacked_candidate_with_mode(fixture, config, poison, None)
}

pub(in crate::egl_renderer::tests) fn render_native_stacked_candidate_with_mode(
    fixture: NativeStackedDiagnosticFixture,
    config: effects::EffectDebugConfig,
    poison: Option<[f32; 4]>,
    scene_replay_work_mode: Option<effects::SceneReplayWorkMode>,
) -> (
    oblivion_one::effects::CompiledFrameGraph,
    Vec<u8>,
    Vec<u8>,
    Vec<String>,
) {
    let mut harness = GlesEffectTestHarness::new(fixture.output_size.0, fixture.output_size.1);
    if config.checkpoint_capture_path() == effects::CheckpointCapturePath::FramebufferShaderCopy {
        harness.install_texture_backed_output();
    }
    install_native_stacked_diagnostic_scene(&mut harness, fixture.scene);
    let graph = compile_native_stacked_graph(fixture, &EffectRegion::empty());
    let a_id = oblivion_one::effects::EffectInstanceId::new(fixture.effect_a.id)
        .expect("native A instance id");
    let b_id = oblivion_one::effects::EffectInstanceId::new(fixture.effect_b.id)
        .expect("native B instance id");
    let captures = graph
        .passes
        .iter()
        .filter(|pass| pass.kind == RenderPassKind::SceneCapture)
        .collect::<Vec<_>>();
    assert_eq!(
        captures.len(),
        2,
        "native stacked graph has A and B captures"
    );
    let a_capture = captures
        .iter()
        .copied()
        .find(|pass| pass.instance == a_id)
        .expect("native A scene capture");
    let b_capture = captures
        .iter()
        .copied()
        .find(|pass| pass.instance == b_id)
        .expect("native B scene capture");
    let a_composite = graph
        .passes
        .iter()
        .find(|pass| pass.instance == a_id && pass.kind == RenderPassKind::Composite)
        .expect("native A composite pass");
    assert_eq!(
        b_capture.checkpoint_dependencies,
        vec![a_composite.id],
        "native B dependency is compiled from earlier A"
    );
    let a_texture = graph
        .textures
        .iter()
        .find(|texture| Some(texture.id) == a_capture.output)
        .expect("native A capture texture");
    let b_texture = graph
        .textures
        .iter()
        .find(|texture| Some(texture.id) == b_capture.output)
        .expect("native B capture texture");
    if fixture.effect_b.id == 2 {
        assert_eq!(
            a_texture.domain,
            EffectRect::new(66, 120, 1000, 937).expect("native A capture domain")
        );
        assert_eq!(
            b_texture.domain,
            EffectRect::new(762, 976, 396, 104).expect("native B capture domain")
        );
    }
    if fixture.effect_b.id == 12 {
        assert_eq!(
            b_texture.domain,
            EffectRect::new(0, 0, 120, 65).expect("native TopBar capture domain")
        );
    }
    let a_instance = graph
        .instances
        .iter()
        .find(|instance| instance.id == a_id)
        .expect("native A compiled instance");
    let b_required_region = EffectRegion::from_rect(b_texture.domain);
    let dependency_required_region =
        b_required_region.intersect(&a_instance.output_influence_region);
    assert!(
        !dependency_required_region.is_empty(),
        "native B required region intersects A influence"
    );

    execute_diagnostic_frame_with_origin_and_scene_replay_mode(
        &mut harness,
        &graph,
        &diagnostic_repaint_plan_for_repairs_in_size(&[fixture.repair], true, fixture.output_size),
        EffectRegion::from_rect(fixture.output_bounds),
        true,
        config,
        OutputFramebufferOrigin::TopLeftScanout,
        scene_replay_work_mode,
    );
    let previous = read_diagnostic_pixels(&harness);

    update_diagnostic_background_for_surface(
        &harness,
        fixture.scene.background_surface,
        fixture.repair,
        [236, 28, 42, 255],
    );
    if let Some(poison) = poison {
        poison_diagnostic_output(&harness, poison);
    }
    harness.renderer.effect_runtime.effect_trace =
        effects::EffectExecutionTrace::enabled_for_test();
    effects::clear_effect_trace_test_events();
    let repair_region = diagnostic_region(fixture.repair);
    let demand = oblivion_one::effects::plan_effect_execution_demand_with_kawase_mode(
        &graph,
        &repair_region,
        false,
        config.kawase_mode() == effects::EffectDebugKawaseMode::Full,
    );
    let a_output_demand = demand
        .output_region(a_id)
        .expect("native A is demanded by B checkpoint")
        .clone();
    assert!(
        dependency_required_region
            .subtract(&a_output_demand)
            .is_empty(),
        "A semantically valid Composite demand covers B's exact dependency region"
    );
    let a_composite_output = demand
        .pass_output_region(a_composite.id)
        .expect("native A Composite output is semantically valid");
    assert!(
        dependency_required_region
            .subtract(a_composite_output)
            .is_empty(),
        "A Composite output exactly covers B's required dependency region"
    );
    let selection = effects::select_effect_execution(&graph, &demand);
    let consumer_plan = effects::plan_effect_surface_consumers_with_debug_config(
        &graph,
        &demand,
        &selection,
        &harness.renderer.scene_state.commands,
        &[fixture.repair],
        fixture.output_size,
        config,
    );
    for surface_id in [
        fixture.scene.background_surface,
        fixture.scene.a_surface,
        fixture.scene.b_surface,
    ] {
        assert!(
            consumer_plan.surface_ids().contains(&surface_id),
            "native expanded scene work consumes Surface({surface_id})"
        );
    }
    execute_diagnostic_frame_with_origin_and_scene_replay_mode(
        &mut harness,
        &graph,
        &diagnostic_repaint_plan_for_repairs_in_size(&[fixture.repair], false, fixture.output_size),
        repair_region,
        false,
        config,
        OutputFramebufferOrigin::TopLeftScanout,
        scene_replay_work_mode,
    );
    let candidate = read_diagnostic_pixels(&harness);
    let events = effects::take_effect_trace_test_events();
    let a_capture_id = a_capture.id.get().to_string();
    let a_capture_event = events
        .iter()
        .find(|line| {
            line.contains("event=effect_pass_execute_end")
                && line.contains("kind=SceneCapture")
                && line.contains(&format!("pass={a_capture_id}"))
        })
        .expect("native A execute trace event");
    assert!(a_capture_event.contains("checkpoints=0"));
    assert!(a_capture_event.contains("capture_mode=replay"));
    assert!(a_capture_event.contains("backdrop_capture_policy=replay"));
    let checkpoint_id = b_capture.id.get().to_string();
    let capture_event = events
        .iter()
        .find(|line| {
            line.contains("event=effect_pass_execute_end")
                && line.contains("kind=SceneCapture")
                && line.contains(&format!("pass={checkpoint_id}"))
        })
        .expect("native B execute trace event");
    assert!(capture_event.contains("checkpoints=1"));
    let expected_capture_mode = match config.checkpoint_capture_path() {
        effects::CheckpointCapturePath::FramebufferBlit => "framebuffer_blit",
        effects::CheckpointCapturePath::FramebufferShaderCopy => "framebuffer_shader_copy",
    };
    let expected_requested_path = config.checkpoint_capture_path().as_str();
    assert!(capture_event.contains(&format!("capture_mode={expected_capture_mode}")));
    assert!(capture_event.contains(&format!("requested_capture_path={expected_requested_path}")));
    assert!(capture_event.contains(&format!("executed_capture_path={expected_capture_mode}")));
    assert!(capture_event.contains("fallback_reason=none"));
    assert!(capture_event.contains("backdrop_capture_policy=replay"));
    let expected_kawase_policy = match config.kawase_mode() {
        effects::EffectDebugKawaseMode::Full => "full",
        effects::EffectDebugKawaseMode::Partial => "partial",
    };
    assert!(capture_event.contains(&format!("kawase_execution_policy={expected_kawase_policy}")));
    assert!(capture_event.contains("framebuffer_origin=top_left_scanout"));
    let validity_event = events
        .iter()
        .find(|line| {
            line.contains("event=effect_checkpoint_source_validity")
                && line.contains(&format!("pass={checkpoint_id}"))
        })
        .expect("native B checkpoint source validity trace event");
    assert!(validity_event.contains("missing_pixels=0"));

    let a_composite_id = a_composite.id.get().to_string();
    let a_replay_begin = trace_event_index(
        &events,
        &[
            "event=effect_scene_replay_begin",
            "kind=Composite",
            "reason=composite_advance",
            &format!("pass={a_composite_id}"),
        ],
    );
    let b_replay_begin = trace_event_index(
        &events,
        &[
            "event=effect_scene_replay_begin",
            "reason=checkpoint_dependency",
            &format!("pass={checkpoint_id}"),
        ],
    );
    assert!(a_replay_begin < b_replay_begin);
    assert!(events[a_replay_begin].contains("scene_cursor_start=0"));
    assert!(events[a_replay_begin].contains("scene_cursor_end=1"));
    assert!(events[a_replay_begin].contains("command_count=1"));
    assert!(events[b_replay_begin].contains("scene_cursor_start=1"));
    assert!(events[b_replay_begin].contains("scene_cursor_end=2"));
    assert!(events[b_replay_begin].contains("command_count=1"));

    (graph, previous, candidate, events)
}

pub(in crate::egl_renderer::tests) fn render_native_three_checkpoint_candidate_with_mode(
    fixture: NativeThreeCheckpointFixture,
    config: effects::EffectDebugConfig,
    scene_replay_work_mode: Option<effects::SceneReplayWorkMode>,
) -> (
    oblivion_one::effects::CompiledFrameGraph,
    Vec<u8>,
    Vec<u8>,
    Vec<String>,
) {
    let mut harness = GlesEffectTestHarness::new(fixture.output_size.0, fixture.output_size.1);
    if config.checkpoint_capture_path() == effects::CheckpointCapturePath::FramebufferShaderCopy {
        harness.install_texture_backed_output();
    }
    install_native_three_checkpoint_diagnostic_scene(&mut harness, fixture.scene);
    let graph = compile_native_three_checkpoint_graph(fixture, &EffectRegion::empty());
    let a_id = oblivion_one::effects::EffectInstanceId::new(fixture.effect_a.id)
        .expect("three-checkpoint A instance id");
    let c_id = oblivion_one::effects::EffectInstanceId::new(fixture.effect_c.id)
        .expect("three-checkpoint C instance id");
    let b_id = oblivion_one::effects::EffectInstanceId::new(fixture.effect_b.id)
        .expect("three-checkpoint B instance id");
    let captures = graph
        .passes
        .iter()
        .filter(|pass| pass.kind == RenderPassKind::SceneCapture)
        .collect::<Vec<_>>();
    assert_eq!(
        captures.len(),
        3,
        "three-checkpoint graph has A, C, and B captures"
    );
    let a_capture = captures
        .iter()
        .copied()
        .find(|pass| pass.instance == a_id)
        .expect("three-checkpoint A scene capture");
    let c_capture = captures
        .iter()
        .copied()
        .find(|pass| pass.instance == c_id)
        .expect("three-checkpoint C scene capture");
    let b_capture = captures
        .iter()
        .copied()
        .find(|pass| pass.instance == b_id)
        .expect("three-checkpoint B scene capture");
    assert!(a_capture.checkpoint_dependencies.is_empty());
    assert!(!c_capture.checkpoint_dependencies.is_empty());
    assert!(b_capture.checkpoint_dependencies.len() >= 2);
    assert_ne!(a_capture.anchor, c_capture.anchor);
    if fixture.same_anchor {
        assert_eq!(c_capture.anchor, b_capture.anchor);
    } else {
        assert_ne!(c_capture.anchor, b_capture.anchor);
    }

    let command_positions = [
        fixture.scene.a_surface,
        fixture.scene.c_surface,
        fixture.scene.b_surface,
    ]
    .map(|surface_id| {
        harness
            .renderer
            .scene_state
            .commands
            .iter()
            .position(|command| command.layer == EglDrawLayer::Surface(surface_id))
            .expect("three-checkpoint surface command")
    });
    assert!(
        command_positions[0] < command_positions[1] && command_positions[1] < command_positions[2],
        "three-checkpoint composition ranges must be distinct: {command_positions:?}"
    );
    assert!(command_positions[1] > command_positions[0]);
    assert!(command_positions[2] > command_positions[1]);

    execute_diagnostic_frame_with_origin_and_scene_replay_mode(
        &mut harness,
        &graph,
        &diagnostic_repaint_plan_for_repairs_in_size(&[fixture.repair], true, fixture.output_size),
        EffectRegion::from_rect(fixture.output_bounds),
        true,
        config,
        OutputFramebufferOrigin::TopLeftScanout,
        scene_replay_work_mode,
    );
    let previous = read_diagnostic_pixels(&harness);
    update_diagnostic_background_for_surface(
        &harness,
        fixture.scene.background_surface,
        fixture.repair,
        [236, 28, 42, 255],
    );
    harness.renderer.effect_runtime.effect_trace =
        effects::EffectExecutionTrace::enabled_for_test();
    effects::clear_effect_trace_test_events();

    let repair_region = diagnostic_region(fixture.repair);
    let demand = oblivion_one::effects::plan_effect_execution_demand_with_kawase_mode(
        &graph,
        &repair_region,
        false,
        config.kawase_mode() == effects::EffectDebugKawaseMode::Full,
    );
    let selection = effects::select_effect_execution(&graph, &demand);
    let consumer_plan = effects::plan_effect_surface_consumers_with_debug_config(
        &graph,
        &demand,
        &selection,
        &harness.renderer.scene_state.commands,
        &[fixture.repair],
        fixture.output_size,
        config,
    );
    for surface_id in [
        fixture.scene.background_surface,
        fixture.scene.a_surface,
        fixture.scene.c_surface,
        fixture.scene.b_surface,
    ] {
        assert!(
            consumer_plan.surface_ids().contains(&surface_id),
            "three-checkpoint expanded work consumes Surface({surface_id})"
        );
    }
    execute_diagnostic_frame_with_origin_and_scene_replay_mode(
        &mut harness,
        &graph,
        &diagnostic_repaint_plan_for_repairs_in_size(&[fixture.repair], false, fixture.output_size),
        repair_region,
        false,
        config,
        OutputFramebufferOrigin::TopLeftScanout,
        scene_replay_work_mode,
    );
    let candidate = read_diagnostic_pixels(&harness);
    let events = effects::take_effect_trace_test_events();
    let validity_events = events
        .iter()
        .filter(|line| line.contains("event=effect_checkpoint_source_validity"))
        .collect::<Vec<_>>();
    assert!(!validity_events.is_empty());
    assert!(
        validity_events
            .iter()
            .all(|line| line.contains("missing_pixels=0"))
    );
    for capture in [c_capture, b_capture] {
        let pass = capture.id.get().to_string();
        let event = events
            .iter()
            .find(|line| {
                line.contains("event=effect_pass_execute_end")
                    && line.contains("kind=SceneCapture")
                    && line.contains(&format!("pass={pass}"))
            })
            .expect("three-checkpoint framebuffer capture trace event");
        let expected_capture_mode = match config.checkpoint_capture_path() {
            effects::CheckpointCapturePath::FramebufferBlit => "framebuffer_blit",
            effects::CheckpointCapturePath::FramebufferShaderCopy => "framebuffer_shader_copy",
        };
        assert!(event.contains(&format!("capture_mode={expected_capture_mode}")));
        assert!(event.contains(&format!(
            "requested_capture_path={}",
            config.checkpoint_capture_path().as_str()
        )));
        assert!(event.contains(&format!("executed_capture_path={expected_capture_mode}")));
        assert!(event.contains("fallback_reason=none"));
        assert!(event.contains("backdrop_capture_policy=replay"));
        assert!(event.contains("kawase_execution_policy=partial"));
    }

    (graph, previous, candidate, events)
}

pub(in crate::egl_renderer::tests) fn trace_field_u64(line: &str, field: &str) -> u64 {
    line.split_whitespace()
        .find_map(|part| part.strip_prefix(&format!("{field}=")))
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| panic!("trace field {field} missing from {line}"))
}

pub(in crate::egl_renderer::tests) fn assert_native_three_checkpoint_replay_evidence(
    fixture: NativeThreeCheckpointFixture,
    baseline: &[u8],
    baseline_events: &[String],
    suffix: &[u8],
    suffix_events: &[String],
) {
    let mismatches = baseline
        .iter()
        .zip(suffix)
        .filter(|(expected, actual)| expected.abs_diff(**actual) > 2)
        .count();
    assert_eq!(
        mismatches, 0,
        "three-checkpoint baseline and suffix pixels differ"
    );
    for events in [baseline_events, suffix_events] {
        let validity_events = events
            .iter()
            .filter(|line| line.contains("event=effect_checkpoint_source_validity"))
            .collect::<Vec<_>>();
        assert!(!validity_events.is_empty());
        assert!(
            validity_events
                .iter()
                .all(|line| line.contains("missing_pixels=0"))
        );
    }

    let graph = compile_native_three_checkpoint_graph(fixture, &EffectRegion::empty());
    let ids = [
        fixture.effect_a.id,
        fixture.effect_c.id,
        fixture.effect_b.id,
    ]
    .map(|id| oblivion_one::effects::EffectInstanceId::new(id).unwrap());
    let captures = ids.map(|instance| {
        graph
            .passes
            .iter()
            .find(|pass| pass.kind == RenderPassKind::SceneCapture && pass.instance == instance)
            .expect("three-checkpoint capture pass")
    });
    let c_id = captures[1].id.get().to_string();
    let b_id = captures[2].id.get().to_string();
    let c_begin = suffix_events
        .iter()
        .find(|line| {
            line.contains("event=effect_scene_replay_begin")
                && line.contains("reason=checkpoint_dependency")
                && line.contains(&format!("pass={c_id}"))
        })
        .expect("C checkpoint replay boundary");
    assert!(trace_field_u64(c_begin, "pending_checkpoint_requirements") >= 2);
    let c_satisfied = suffix_events
        .iter()
        .find(|line| {
            line.contains("event=effect_scene_replay_capture_satisfied")
                && line.contains(&format!("pass={c_id}"))
        })
        .expect("C capture-satisfied trace event");
    let b_begin = suffix_events
        .iter()
        .find(|line| {
            line.contains("event=effect_scene_replay_begin")
                && line.contains("reason=checkpoint_dependency")
                && line.contains(&format!("pass={b_id}"))
        })
        .expect("B checkpoint replay boundary");
    assert!(trace_field_u64(b_begin, "pending_checkpoint_requirements") >= 1);
    assert!(
        trace_field_u64(c_satisfied, "active_work_pixels")
            < trace_field_u64(c_begin, "active_work_pixels")
    );
    let b_satisfied = suffix_events
        .iter()
        .find(|line| {
            line.contains("event=effect_scene_replay_capture_satisfied")
                && line.contains(&format!("pass={b_id}"))
        })
        .expect("B capture-satisfied trace event");
    assert_eq!(
        trace_field_u64(b_satisfied, "pending_checkpoint_requirements"),
        0
    );
    let final_replay = suffix_events
        .iter()
        .find(|line| line.contains("event=effect_final_scene_replay_begin"))
        .expect("final ordinary scene replay boundary");
    assert_eq!(
        trace_field_u64(final_replay, "pending_checkpoint_requirements"),
        0
    );
    assert_eq!(
        trace_field_u64(final_replay, "active_work_pixels"),
        u64::from(fixture.repair.width) * u64::from(fixture.repair.height)
    );
}

pub(in crate::egl_renderer::tests) fn assert_native_same_anchor_replay_evidence(
    fixture: NativeThreeCheckpointFixture,
    baseline: &[u8],
    suffix: &[u8],
    suffix_events: &[String],
) {
    let mismatches = baseline
        .iter()
        .zip(suffix)
        .filter(|(expected, actual)| expected.abs_diff(**actual) > 2)
        .count();
    assert_eq!(
        mismatches, 0,
        "same-anchor baseline and suffix pixels differ"
    );
    let graph = compile_native_three_checkpoint_graph(fixture, &EffectRegion::empty());
    let capture_passes = [fixture.effect_c.id, fixture.effect_b.id].map(|id| {
        let instance = oblivion_one::effects::EffectInstanceId::new(id).unwrap();
        graph
            .passes
            .iter()
            .find(|pass| pass.kind == RenderPassKind::SceneCapture && pass.instance == instance)
            .expect("same-anchor capture pass")
    });
    assert_eq!(capture_passes[0].anchor, capture_passes[1].anchor);
    assert!(!capture_passes[0].checkpoint_dependencies.is_empty());
    assert!(!capture_passes[1].checkpoint_dependencies.is_empty());
    let capture_ids = capture_passes.map(|pass| pass.id.get().to_string());
    let later_capture_id = capture_ids[1].clone();
    let satisfied = capture_ids.map(|id| {
        suffix_events
            .iter()
            .find(|line| {
                line.contains("event=effect_scene_replay_capture_satisfied")
                    && line.contains(&format!("pass={id}"))
            })
            .expect("same-anchor capture-satisfied trace event")
    });
    assert_eq!(
        trace_field_u64(satisfied[0], "pending_checkpoint_requirements"),
        1
    );
    assert_eq!(
        trace_field_u64(satisfied[1], "pending_checkpoint_requirements"),
        0
    );
    assert_eq!(
        trace_field_u64(satisfied[0], "scene_cursor_start"),
        trace_field_u64(satisfied[1], "scene_cursor_start")
    );
    assert_eq!(
        trace_field_u64(satisfied[0], "scene_cursor_end"),
        trace_field_u64(satisfied[1], "scene_cursor_end")
    );
    let validity_events = suffix_events
        .iter()
        .filter(|line| line.contains("event=effect_checkpoint_source_validity"))
        .collect::<Vec<_>>();
    assert!(!validity_events.is_empty());
    assert!(
        validity_events
            .iter()
            .all(|line| line.contains("missing_pixels=0"))
    );
    assert!(
        validity_events
            .iter()
            .any(|line| line.contains(&format!("pass={later_capture_id}")))
    );
}

pub(in crate::egl_renderer::tests) fn assert_native_baseline_matches_suffix_demand(
    fixture: NativeStackedDiagnosticFixture,
    label: &str,
) {
    let config = effects::EffectDebugConfig::new(
        effects::EffectDebugCaptureMode::Replay,
        effects::EffectDebugKawaseMode::Partial,
    );
    let (_, _, baseline, baseline_events) = render_native_stacked_candidate_with_mode(
        fixture,
        config,
        None,
        Some(effects::SceneReplayWorkMode::GlobalBaseline),
    );
    let (_, _, suffix, suffix_events) = render_native_stacked_candidate_with_mode(
        fixture,
        config,
        None,
        Some(effects::SceneReplayWorkMode::SuffixDemand),
    );
    let mismatches = baseline
        .iter()
        .zip(&suffix)
        .filter(|(expected, actual)| expected.abs_diff(**actual) > 2)
        .count();
    assert_eq!(
        mismatches, 0,
        "{label} baseline and suffix replay pixels differ"
    );

    for (mode, events) in [("baseline", &baseline_events), ("suffix", &suffix_events)] {
        let validity_events = events
            .iter()
            .filter(|line| line.contains("event=effect_checkpoint_source_validity"))
            .collect::<Vec<_>>();
        assert!(
            !validity_events.is_empty(),
            "{label} {mode} produced no checkpoint validity events"
        );
        assert!(
            validity_events
                .iter()
                .all(|line| line.contains("missing_pixels=0")),
            "{label} {mode} checkpoint validity event reported missing pixels: {validity_events:?}"
        );
    }

    let baseline_replays = baseline_events
        .iter()
        .filter(|line| line.contains("event=effect_scene_replay_"))
        .collect::<Vec<_>>();
    assert!(
        !baseline_replays.is_empty(),
        "{label} baseline has no replay intervals"
    );
    assert!(
        baseline_replays
            .iter()
            .all(|line| line.contains("saved_pixels=0")),
        "{label} baseline unexpectedly saved replay pixels: {baseline_replays:?}"
    );

    let suffix_replays = suffix_events
        .iter()
        .filter(|line| line.contains("event=effect_scene_replay_"))
        .collect::<Vec<_>>();
    assert!(
        suffix_replays.iter().any(|line| {
            line.split_whitespace()
                .find_map(|field| field.strip_prefix("saved_pixels="))
                .and_then(|pixels| pixels.parse::<u64>().ok())
                .is_some_and(|pixels| pixels > 0)
        }),
        "{label} suffix produced no replay interval with saved pixels: {suffix_replays:?}"
    );
}

pub(in crate::egl_renderer::tests) fn render_native_stacked_full_reference(
    fixture: NativeStackedDiagnosticFixture,
    config: effects::EffectDebugConfig,
) -> Vec<u8> {
    let mut harness = GlesEffectTestHarness::new(fixture.output_size.0, fixture.output_size.1);
    install_native_stacked_diagnostic_scene(&mut harness, fixture.scene);
    let graph =
        compile_native_stacked_graph(fixture, &EffectRegion::from_rect(fixture.output_bounds));
    update_diagnostic_background_for_surface(
        &harness,
        fixture.scene.background_surface,
        fixture.repair,
        [236, 28, 42, 255],
    );
    execute_diagnostic_frame_with_origin(
        &mut harness,
        &graph,
        &diagnostic_repaint_plan_for_repairs_in_size(&[fixture.repair], true, fixture.output_size),
        EffectRegion::from_rect(fixture.output_bounds),
        true,
        config,
        OutputFramebufferOrigin::TopLeftScanout,
    );
    read_diagnostic_pixels(&harness)
}

pub(in crate::egl_renderer::tests) fn assert_native_poison_independence(
    fixture: NativeStackedDiagnosticFixture,
    poison_a: &[u8],
    poison_b: &[u8],
    full_current_reference: &[u8],
    label: &str,
) {
    for y in fixture.repair.y as u32..(fixture.repair.y as u32 + fixture.repair.height) {
        for x in fixture.repair.x as u32..(fixture.repair.x as u32 + fixture.repair.width) {
            assert_eq!(
                diagnostic_pixel_for_origin(
                    poison_a,
                    fixture.output_size.0,
                    fixture.output_size.1,
                    x,
                    y,
                    OutputFramebufferOrigin::TopLeftScanout,
                ),
                diagnostic_pixel_for_origin(
                    poison_b,
                    fixture.output_size.0,
                    fixture.output_size.1,
                    x,
                    y,
                    OutputFramebufferOrigin::TopLeftScanout,
                ),
                "{label} output depends on poison at ({x}, {y})"
            );
            assert_eq!(
                diagnostic_pixel_for_origin(
                    poison_a,
                    fixture.output_size.0,
                    fixture.output_size.1,
                    x,
                    y,
                    OutputFramebufferOrigin::TopLeftScanout,
                ),
                diagnostic_pixel_for_origin(
                    full_current_reference,
                    fixture.output_size.0,
                    fixture.output_size.1,
                    x,
                    y,
                    OutputFramebufferOrigin::TopLeftScanout,
                ),
                "{label} poison differs from full reference at ({x}, {y})"
            );
        }
    }
}

pub(in crate::egl_renderer::tests) fn diagnostic_region(rect: OutputRect) -> EffectRegion {
    diagnostic_region_for_repairs(&[rect])
}

pub(in crate::egl_renderer::tests) fn diagnostic_region_for_repairs(
    repairs: &[OutputRect],
) -> EffectRegion {
    let mut region = EffectRegion::empty();
    for repair in repairs {
        region.push(
            EffectRect::new(repair.x, repair.y, repair.width, repair.height)
                .expect("diagnostic repair region"),
        );
    }
    region
}

pub(in crate::egl_renderer::tests) fn diagnostic_pixel_is_inside(
    rect: OutputRect,
    x: u32,
    y: u32,
) -> bool {
    rect.x <= x as i32
        && (x as i32) < rect.x + rect.width as i32
        && rect.y <= y as i32
        && (y as i32) < rect.y + rect.height as i32
}

pub(in crate::egl_renderer::tests) fn diagnostic_pixel_is_inside_repairs(
    repairs: &[OutputRect],
    x: u32,
    y: u32,
) -> bool {
    repairs
        .iter()
        .copied()
        .any(|repair| diagnostic_pixel_is_inside(repair, x, y))
}

pub(in crate::egl_renderer::tests) fn diagnostic_matrix_mismatch_counts(
    actual: &[u8],
    previous: &[u8],
    full_reference: &[u8],
    width: u32,
    height: u32,
    repairs: &[OutputRect],
    tolerance: u8,
) -> (usize, usize) {
    let mut outside = 0;
    let mut inside = 0;
    for y in 0..height {
        for x in 0..width {
            let expected = if diagnostic_pixel_is_inside_repairs(repairs, x, y) {
                diagnostic_pixel(full_reference, width, height, x, y)
            } else {
                diagnostic_pixel(previous, width, height, x, y)
            };
            let actual_pixel = diagnostic_pixel(actual, width, height, x, y);
            let mismatch = actual_pixel
                .iter()
                .zip(expected)
                .any(|(actual, expected)| actual.abs_diff(expected) > tolerance);
            if mismatch {
                if diagnostic_pixel_is_inside_repairs(repairs, x, y) {
                    inside += 1;
                } else {
                    outside += 1;
                }
            }
        }
    }
    (outside, inside)
}

#[allow(clippy::too_many_arguments)]
pub(in crate::egl_renderer::tests) fn assert_diagnostic_matrix_pixels(
    actual: &[u8],
    previous: &[u8],
    full_reference: &[u8],
    width: u32,
    height: u32,
    repair: OutputRect,
    tolerance: u8,
    label: &str,
) {
    for y in 0..height {
        for x in 0..width {
            let expected = if diagnostic_pixel_is_inside(repair, x, y) {
                diagnostic_pixel(full_reference, width, height, x, y)
            } else {
                diagnostic_pixel(previous, width, height, x, y)
            };
            let actual_pixel = diagnostic_pixel(actual, width, height, x, y);
            for (channel, (actual, expected)) in actual_pixel.iter().zip(expected).enumerate() {
                assert!(
                    actual.abs_diff(expected) <= tolerance,
                    "{label} pixel mismatch at ({x}, {y}) channel {channel}: actual={}, expected={}, tolerance={tolerance}",
                    actual,
                    expected,
                );
            }
        }
    }
}
