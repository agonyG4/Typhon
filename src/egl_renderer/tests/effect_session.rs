use super::*;

#[test]
fn overlay_failure_finalizes_graph_leases_without_promoting_checkpoint_state() {
    let mut harness = GlesEffectTestHarness::new(320, 200);
    harness.install_texture_backed_output();
    harness.renderer.effect_runtime.effect_trace =
        effects::EffectExecutionTrace::enabled_for_test();
    effects::clear_effect_trace_test_events();

    let output_bounds = EffectRect::new(0, 0, 320, 200).expect("output bounds");
    let full_damage = EffectRegion::from_rect(output_bounds);
    install_diagnostic_scene(
        &mut harness,
        EffectRect::new(60, 40, 180, 110).expect("owner bounds"),
        VisualGroupId::new(1).expect("owner visual group"),
    );
    let scene = lifecycle_blur_effect_scene(
        42,
        oblivion_one::effects::builtin_background_blur_program_id(),
    );
    let registry = oblivion_one::effects::EffectRegistry::with_builtin_background_blur();
    let oblivion_one::effects::FrameExecutionPlan::EffectGraph(mut graph) =
        oblivion_one::effects::compile_frame_execution_plan(
            &scene,
            &full_damage,
            output_bounds,
            &registry,
        )
        .expect("lifecycle blur graph compiles")
    else {
        panic!("lifecycle blur must compile to an effect graph");
    };
    // Extend temporary texture lifetimes through the overlay boundary, rather
    // than allowing last-use retirement inside the core. This isolates the
    // session's error finalization contract from per-pass early reclamation.
    for texture in &mut graph.textures {
        texture.last_use = None;
    }
    harness.renderer.set_effect_registry(registry);
    let demand = plan_effect_execution_demand(&graph, &full_damage, true);
    let selection = effects::select_effect_execution(&graph, &demand);
    let repaint_plan = RepaintPlan {
        render_damage: OutputDamage::Full,
        repair_damage: OutputDamage::Full,
        buffer_age: None,
        mode: RepaintMode::Full,
        fallback_reason: None,
        ..RepaintPlan::default()
    };
    let debug_config = effects::EffectDebugConfig::new(
        effects::EffectDebugCaptureMode::Framebuffer,
        effects::EffectDebugKawaseMode::Partial,
    );

    // Seed a prior causal baseline so a mistaken promotion after this failed
    // frame would be observable for the following frame.
    harness
        .renderer
        .scene_state
        .current_checkpoint_scene_causal_snapshot = Some(EglCheckpointSceneCausalSnapshot::new(
        harness.renderer.scene_state.current_size,
        &harness.renderer.scene_state.commands,
        &harness.renderer.scene_state.vertices,
        &[],
        &harness.renderer.scene_state.presentation_opacities,
        &harness
            .renderer
            .scene_state
            .presentation_visual_group_owners,
    ));
    let seed_frame_serial = harness
        .renderer
        .effect_runtime
        .effect_resources
        .checkpoint_frame_serial();
    harness
        .renderer
        .promote_checkpoint_cache_causal_state(&graph);
    assert!(
        harness
            .renderer
            .effect_runtime
            .effect_resources
            .checkpoint_causal_state_for_frame(seed_frame_serial + 1)
            .is_some()
    );
    let frame_serial = harness
        .renderer
        .effect_runtime
        .effect_resources
        .begin_checkpoint_frame();
    assert_eq!(frame_serial, seed_frame_serial + 1);

    let checked_out_before = harness
        .renderer
        .effect_runtime
        .effect_resources
        .metrics()
        .checked_out_texture_count;
    let mut prepared = {
        let mut context = harness.renderer.effect_execution_context();
        effects::prepare_effect_graph_execution(
            &mut context,
            &graph,
            OutputFramebufferOrigin::BottomLeft,
            &repaint_plan,
            &demand,
            &selection,
            debug_config,
            None,
        )
        .expect("graph preparation succeeds")
    };
    let promotes_checkpoint_cache = prepared.promotes_checkpoint_cache();
    assert!(promotes_checkpoint_cache);

    let core_result = {
        let mut context = harness.renderer.effect_execution_context();
        effects::execute_prepared_effect_graph_core(&mut context, &mut prepared)
    };
    core_result.expect("graph core succeeds before the simulated overlay failure");
    let checked_out_during_overlay = harness
        .renderer
        .effect_runtime
        .effect_resources
        .metrics()
        .checked_out_texture_count;
    assert!(
        checked_out_during_overlay > checked_out_before,
        "prepared graph textures remain checked out across overlay composition"
    );

    // Model draw_lifecycle_overlays returning an error after its trace begin.
    harness
        .renderer
        .effect_runtime
        .effect_trace
        .overlay_boundary("begin");
    let overlay_error = std::io::Error::other("injected lifecycle overlay failure");
    let execution_result: RendererResult<effects::EffectExecutionStats> = Err(overlay_error.into());
    let result = {
        let mut context = harness.renderer.effect_execution_context();
        effects::finish_prepared_effect_graph_execution(&mut context, prepared, execution_result)
    };
    assert_eq!(
        result
            .expect_err("overlay error is preserved through graph finalization")
            .to_string(),
        "injected lifecycle overlay failure"
    );

    // Finalization never promotes causal state. The failed session leaves the
    // previous baseline too old to authorize the following frame.
    assert!(
        harness
            .renderer
            .effect_runtime
            .effect_resources
            .checkpoint_causal_state_for_frame(frame_serial + 1)
            .is_none()
    );
    assert_eq!(
        harness
            .renderer
            .effect_runtime
            .effect_resources
            .metrics()
            .checked_out_texture_count,
        checked_out_before,
        "finalization releases every graph-owned texture after overlay failure"
    );

    let events = effects::take_effect_trace_test_events();
    let overlay_begin = trace_event_index(&events, &["event=effect_overlay_draw_begin"]);
    let execute_end = trace_event_index(&events, &["event=effect_graph_execute_end"]);
    let release_begin = trace_event_index(&events, &["event=effect_graph_release_begin"]);
    let release_end = trace_event_index(&events, &["event=effect_graph_release_end"]);
    assert!(
        overlay_begin < execute_end && execute_end < release_begin && release_begin < release_end,
        "failed overlays remain inside graph execution, before resource release: {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|event| event.starts_with("event=effect_overlay_draw_end ")),
        "failed lifecycle overlay has no matching overlay end: {events:?}"
    );
}
