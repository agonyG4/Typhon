use super::*;

#[test]
fn draw_scene_reconciles_canonical_and_empty_lifecycle_decoration_resources_together() {
    let _egl_test_lock = egl_test_lock();
    const EGL_PLATFORM_SURFACELESS_MESA: egl::Enum = 0x31dd;
    let egl = unsafe { EglInstance::load_required() }
        .expect("EGL loader is required for decoration reconciliation regression");
    let display = unsafe {
        egl.get_platform_display(
            EGL_PLATFORM_SURFACELESS_MESA,
            std::ptr::null_mut(),
            &[egl::ATTRIB_NONE],
        )
        .or_else(|_| {
            egl.get_display(egl::DEFAULT_DISPLAY)
                .ok_or(egl::Error::BadDisplay)
        })
    }
    .expect("EGL display is available");
    egl.initialize(display).expect("EGL initializes");
    egl.bind_api(egl::OPENGL_ES_API)
        .expect("EGL binds the GLES API");
    let config_attributes = [
        egl::SURFACE_TYPE,
        egl::PBUFFER_BIT,
        egl::RENDERABLE_TYPE,
        egl::OPENGL_ES3_BIT,
        egl::RED_SIZE,
        8,
        egl::GREEN_SIZE,
        8,
        egl::BLUE_SIZE,
        8,
        egl::ALPHA_SIZE,
        8,
        egl::NONE,
    ];
    let count = egl
        .matching_config_count(display, &config_attributes)
        .expect("EGL returns GLES3 pbuffer configs");
    assert!(count > 0, "EGL exposes a GLES3 pbuffer config");
    let mut configs = Vec::with_capacity(count);
    egl.choose_config(display, &config_attributes, &mut configs)
        .expect("EGL chooses a GLES3 pbuffer config");
    let config = configs[0];
    let context = create_gles_context(&egl, display, config).expect("GLES3 context creates");
    let egl_surface = egl
        .create_pbuffer_surface(
            display,
            config,
            &[egl::WIDTH, 320, egl::HEIGHT, 200, egl::NONE],
        )
        .expect("EGL pbuffer surface creates");
    egl.make_current(display, Some(egl_surface), Some(egl_surface), Some(context))
        .expect("EGL makes the GLES3 context current");

    let cursor_image = Arc::new(
        CompositorCursorImage::from_argb8888(vec![0xffff_ffff], 1, 1, 0, 0)
            .expect("test cursor image is valid"),
    );
    let mut renderer = GlesSceneRenderer::new_current(
        &egl,
        320,
        200,
        None,
        EglPartialRepaintCapabilities {
            buffer_age: false,
            partial_render_repair: false,
            swap_buffers_with_damage: false,
        },
        cursor_image,
    )
    .expect("test GLES renderer creates");

    let socket_name = format!(
        "typhon-decoration-resource-reconciliation-{}",
        std::process::id()
    );
    let mut server =
        oblivion_one::compositor::OwnCompositorServer::bind_cpu_composition(&socket_name)
            .expect("bind compositor for decoration reconciliation regression");
    let surface = RenderableSurface {
        surface_id: 603,
        x: 0,
        y: 0,
        width: 320,
        height: 200,
        placement: SurfacePlacement::root(),
        render_backend: SurfaceRenderBackend::NativeWayland,
        render_placement: None,
        visual_clip: None,
        render_target_size: None,
        generation: 1,
        commit_sequence: SurfaceCommitSequence::initial(),
        buffer: CommittedSurfaceBuffer::shm_snapshot(
            BufferIdAllocator::default()
                .allocate()
                .expect("test buffer identity"),
            BufferSize::new(320, 200).expect("test surface size"),
            vec![0xff12_3456; 320 * 200],
        ),
        viewport_source: None,
        viewport_destination: None,
        buffer_scale: 1,
        buffer_transform: wayland_server::protocol::wl_output::Transform::Normal,
        damage: RenderableSurfaceDamage::full(),
        opaque_region: SurfaceOpaqueRegion::None,
    };
    let window_id = oblivion_one::compositor::WindowId::from_raw(3).expect("test window id");
    server.install_native_frame_test_scene_with_server_decorations(
        vec![surface],
        &[(603, window_id)],
        None,
    );
    let mut resolved = crate::native_output::ResolvedNativeFrameScene::from_server_at(
        &server,
        AnimationTime::from_nanos(0),
    );
    assert_eq!(resolved.decorations.len(), 1);
    assert!(resolved.lifecycle_decorations.is_empty());
    let required_layers = resolved
        .decorations
        .iter()
        .flat_map(DecorationRenderInstance::primitives)
        .filter_map(|primitive| match primitive {
            DecorationRenderPrimitive::SolidRect { color, .. } => {
                Some(EglDrawLayer::SolidRgba(rgba_to_pixel(*color)))
            }
            DecorationRenderPrimitive::Image { asset, .. } => {
                Some(EglDrawLayer::DecorationAsset(asset.asset_id()))
            }
            DecorationRenderPrimitive::Text { .. } => None,
        })
        .collect::<Vec<_>>();
    assert!(
        required_layers
            .iter()
            .any(|layer| matches!(layer, EglDrawLayer::SolidRgba(_)))
    );

    let input_state = crate::native_output::NativeInputState::new(320, 200);
    let mut frame_renderer = crate::native_output::NativeFrameRenderer::default();
    let request = frame_renderer.egl_scene_draw_request(
        320,
        200,
        &resolved,
        &server,
        &input_state,
        crate::native_output::NativeCursorRenderMode::Hardware,
        Some(OutputDamage::Full),
    );
    let outcome = renderer
        .draw_scene(&egl, display, egl_surface, request)
        .expect("frame with canonical decoration and empty lifecycle renders");
    let (stats, commit) = match outcome {
        EglFrameOutcome::Rendered { commit, stats, .. } => (stats, Some(commit)),
        EglFrameOutcome::Skipped { stats, .. } => (stats, None),
        EglFrameOutcome::LifecycleFallback { .. } => {
            panic!("decoration-only regression frame unexpectedly requested lifecycle fallback")
        }
    };

    let resources = renderer.resources.texture_view();
    assert!(required_layers.iter().all(|layer| match layer {
        EglDrawLayer::SolidRgba(color) => {
            resources.texture_for_solid_decoration(*color).is_some()
        }
        EglDrawLayer::DecorationAsset(asset_id) => {
            resources.texture_for_decoration_asset(*asset_id).is_some()
        }
        _ => unreachable!("required decoration layers stay in the decoration domain"),
    }));
    assert_eq!(stats.missing_required_decoration_resources, 0);

    renderer.commit_presented(
        commit.expect("the initial full-damage frame renders"),
        OutputDamage::Full,
    );
    assert_eq!(renderer.scene_state.repaint_planner.history_depth(), 1);

    renderer
        .resources
        .ensure_decoration_resources(&renderer.gl, &egl, display, std::iter::empty())
        .expect("unused decoration textures are retired");
    assert!(required_layers.iter().all(|layer| match layer {
        EglDrawLayer::SolidRgba(color) => {
            renderer
                .resources
                .texture_view()
                .texture_for_solid_decoration(*color)
                .is_none()
        }
        EglDrawLayer::DecorationAsset(asset_id) => {
            renderer
                .resources
                .texture_view()
                .texture_for_decoration_asset(*asset_id)
                .is_none()
        }
        _ => unreachable!("required decoration layers stay in the decoration domain"),
    }));

    renderer.effect_runtime.effect_trace = effects::EffectExecutionTrace::enabled_for_test();
    effects::clear_effect_trace_test_events();
    let no_damage_request = frame_renderer.egl_scene_draw_request(
        320,
        200,
        &resolved,
        &server,
        &input_state,
        crate::native_output::NativeCursorRenderMode::Hardware,
        Some(OutputDamage::Empty),
    );
    let no_damage_outcome = renderer
        .draw_scene(&egl, display, egl_surface, no_damage_request)
        .expect("no-damage frame still performs pre-skip resource preparation");
    let EglFrameOutcome::Skipped { reason, stats } = no_damage_outcome else {
        panic!("unchanged scene with authoritative empty damage is skipped");
    };
    assert_eq!(reason, FrameSkipReason::NoLogicalDamage);
    assert_eq!(stats.surface_resource_candidates, resolved.surfaces.len());
    assert_eq!(stats.surface_resource_consumers, 0);
    assert_eq!(stats.surface_resource_deferred, resolved.surfaces.len());
    assert_eq!(stats.missing_required_decoration_resources, 0);
    assert!(required_layers.iter().all(|layer| {
        match layer {
            EglDrawLayer::SolidRgba(color) => renderer
                .resources
                .texture_view()
                .texture_for_solid_decoration(*color)
                .is_some(),
            EglDrawLayer::DecorationAsset(asset_id) => renderer
                .resources
                .texture_view()
                .texture_for_decoration_asset(*asset_id)
                .is_some(),
            _ => unreachable!("required decoration layers stay in the decoration domain"),
        }
    }));
    let no_damage_trace = effects::take_effect_trace_test_events();
    let phase_events = no_damage_trace
        .iter()
        .filter(|event| {
            [
                "event=effect_scene_resolve_",
                "event=effect_graph_compile_",
                "event=effect_demand_plan_",
                "event=renderer_draw_complete_",
            ]
            .iter()
            .any(|prefix| event.starts_with(prefix))
        })
        .map(|event| {
            event
                .split_whitespace()
                .next()
                .expect("frame phase event has a name")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        phase_events,
        [
            "event=effect_scene_resolve_begin",
            "event=effect_scene_resolve_end",
            "event=effect_graph_compile_begin",
            "event=effect_graph_compile_end",
        ],
        "no-damage skip occurs after scene resolve and graph selection, before demand and draw: {no_damage_trace:?}"
    );

    renderer
        .resources
        .ensure_decoration_resources(&renderer.gl, &egl, display, std::iter::empty())
        .expect("unused decoration textures are retired again before draw accounting");
    renderer.scene_state.frame_stats = GlesSceneFrameStats::default();
    renderer
        .draw_command_batch(true, None)
        .expect("scene draw tolerates a missing decoration resource");
    assert!(
        renderer
            .last_frame_stats()
            .missing_required_decoration_resources
            > 0
    );

    resolved.lifecycle = lamp_test_sample(0.5);
    renderer.lifecycle.disable_lamp_for_test();
    let request = frame_renderer.egl_scene_draw_request(
        320,
        200,
        &resolved,
        &server,
        &input_state,
        crate::native_output::NativeCursorRenderMode::Hardware,
        Some(OutputDamage::Full),
    );
    let fallback = renderer
        .draw_scene(&egl, display, egl_surface, request)
        .expect("unavailable Lamp requests a recoverable lifecycle fallback");
    let EglFrameOutcome::LifecycleFallback { fallbacks, .. } = fallback else {
        panic!("unavailable Lamp must not produce a rendered frame");
    };
    assert_eq!(fallbacks.failed.len(), 1);
    assert_eq!(fallbacks.failed[0].window_id.get(), 1);
    assert!(renderer.lifecycle.evidence().consumed.is_empty());
    assert_eq!(renderer.scene_state.repaint_planner.history_depth(), 0);

    let mut resolved_effect_lifecycle = lamp_test_sample(0.5);
    resolved_effect_lifecycle.samples[0].visual_source = LifecycleVisualSource {
        window_id: oblivion_one::compositor::WindowId::from_raw(1)
            .expect("valid lifecycle window ID"),
        root_surface_id: 1,
        presentation_identity: test_lifecycle_identity(
            oblivion_one::compositor::WindowId::from_raw(1).expect("valid lifecycle window ID"),
            1,
        ),
        payload_id:
            oblivion_one::compositor::PresentationRetainedVisualPayloadId::from_origin_identity(
                test_lifecycle_identity(
                    oblivion_one::compositor::WindowId::from_raw(1)
                        .expect("valid lifecycle window ID"),
                    1,
                ),
            ),
        kind: LifecycleVisualSourceKind::ResolvedOwnedEffects,
        effect_scene: std::sync::Arc::new(oblivion_one::compositor::ResolvedEffectScene::default()),
    };
    renderer.lifecycle.invalidate_lamp_geometry_for_test();
    renderer.lifecycle.rebuild_lamp_commands(
        &resolved_effect_lifecycle,
        &[],
        &[],
        1.0,
        (320, 200),
        OutputFramebufferOrigin::BottomLeft,
    );
    assert_eq!(renderer.lifecycle.lamp_commands().len(), 1);
    assert!(matches!(
        renderer.lifecycle.lamp_commands()[0].layer,
        EglDrawLayer::LifecycleResolvedVisual(_)
    ));
}

#[test]
fn egl_lifecycle_background_blur_renders_reverses_and_keeps_real_capture_failures_fallback() {
    let _egl_test_lock = egl_test_lock();
    const EGL_PLATFORM_SURFACELESS_MESA: egl::Enum = 0x31dd;
    let egl = unsafe { EglInstance::load_required() }.expect("EGL loader is available");
    let display = unsafe {
        egl.get_platform_display(
            EGL_PLATFORM_SURFACELESS_MESA,
            std::ptr::null_mut(),
            &[egl::ATTRIB_NONE],
        )
        .or_else(|_| {
            egl.get_display(egl::DEFAULT_DISPLAY)
                .ok_or(egl::Error::BadDisplay)
        })
    }
    .expect("surfaceless EGL display is available");
    egl.initialize(display).expect("EGL initializes");
    egl.bind_api(egl::OPENGL_ES_API).expect("EGL binds GLES");
    let config_attributes = [
        egl::SURFACE_TYPE,
        egl::PBUFFER_BIT,
        egl::RENDERABLE_TYPE,
        egl::OPENGL_ES3_BIT,
        egl::RED_SIZE,
        8,
        egl::GREEN_SIZE,
        8,
        egl::BLUE_SIZE,
        8,
        egl::ALPHA_SIZE,
        8,
        egl::NONE,
    ];
    let count = egl
        .matching_config_count(display, &config_attributes)
        .expect("EGL returns GLES3 pbuffer configs");
    assert!(count > 0);
    let mut configs = Vec::with_capacity(count);
    egl.choose_config(display, &config_attributes, &mut configs)
        .expect("EGL chooses a GLES3 pbuffer config");
    let config = configs[0];
    let context = create_gles_context(&egl, display, config).expect("GLES3 context creates");
    let egl_surface = egl
        .create_pbuffer_surface(
            display,
            config,
            &[egl::WIDTH, 320, egl::HEIGHT, 200, egl::NONE],
        )
        .expect("EGL pbuffer surface creates");
    egl.make_current(display, Some(egl_surface), Some(egl_surface), Some(context))
        .expect("EGL makes the context current");

    let cursor_image = Arc::new(
        CompositorCursorImage::from_argb8888(vec![0xffff_ffff], 1, 1, 0, 0)
            .expect("test cursor image is valid"),
    );
    let mut renderer = GlesSceneRenderer::new_current(
        &egl,
        320,
        200,
        None,
        EglPartialRepaintCapabilities {
            buffer_age: false,
            partial_render_repair: false,
            swap_buffers_with_damage: false,
        },
        cursor_image,
    )
    .expect("GLES renderer creates");
    renderer.effect_runtime.effect_trace = effects::EffectExecutionTrace::enabled_for_test();

    let mut buffer_ids = BufferIdAllocator::default();
    let background = lifecycle_test_surface(603, 0, 0, 320, 200, 0xff24_4567, &mut buffer_ids);
    let retained_window =
        lifecycle_test_surface(604, 60, 40, 180, 110, 0xffee_8844, &mut buffer_ids);
    let window_id = oblivion_one::compositor::WindowId::from_raw(1).expect("window id");
    let socket_name = format!("typhon-lifecycle-blur-egl-{}", std::process::id());
    let mut server =
        oblivion_one::compositor::OwnCompositorServer::bind_cpu_composition(&socket_name)
            .expect("test compositor binds");
    server.install_native_frame_test_scene_with_server_decorations(
        vec![background.clone()],
        &[(background.surface_id, window_id)],
        None,
    );
    let mut resolved = crate::native_output::ResolvedNativeFrameScene::from_server_at(
        &server,
        AnimationTime::from_nanos(0),
    );
    // The minimized root is absent from the ordinary scene and remains
    // available only as the retained lifecycle source.
    resolved.surfaces = std::borrow::Cow::Owned(vec![background]);
    resolved.lifecycle_surfaces = vec![retained_window];
    let blur_scene = lifecycle_blur_effect_scene(
        604,
        oblivion_one::effects::builtin_background_blur_program_id(),
    );
    resolved.lifecycle =
        lifecycle_effect_sample(0.45, LifecycleDirection::Minimize, 604, 1, blur_scene);
    let identity = resolved.lifecycle.samples[0].presentation_identity;
    let payload_id = resolved.lifecycle.samples[0].payload_id;
    let source_scene = Arc::clone(&resolved.lifecycle.samples[0].visual_source.effect_scene);

    let input_state = crate::native_output::NativeInputState::new(320, 200);
    let mut frame_renderer = crate::native_output::NativeFrameRenderer::default();
    effects::clear_effect_trace_test_events();
    let request = frame_renderer.egl_scene_draw_request(
        320,
        200,
        &resolved,
        &server,
        &input_state,
        crate::native_output::NativeCursorRenderMode::Hardware,
        Some(OutputDamage::Full),
    );
    let outcome = renderer
        .draw_scene(&egl, display, egl_surface, request)
        .expect("lifecycle blur frame draws");
    let first_evidence = match outcome {
        EglFrameOutcome::Rendered {
            commit,
            lifecycle_evidence,
            ..
        } => {
            renderer
                .scene_state
                .repaint_planner
                .commit_presented_transition(OutputDamage::Full);
            assert_eq!(commit.repaint_plan.repair_damage, OutputDamage::Full);
            lifecycle_evidence
        }
        EglFrameOutcome::LifecycleFallback { fallbacks, .. } => {
            panic!("valid lifecycle blur unexpectedly fell back: {fallbacks:?}")
        }
        EglFrameOutcome::Skipped { reason, .. } => {
            panic!("full-damage lifecycle blur was skipped: {reason:?}")
        }
    };
    assert!(first_evidence.contains(identity, payload_id, 604));
    assert!(
        renderer
            .lifecycle
            .is_visual_source_ready(payload_id, &renderer.effect_runtime)
    );
    assert!(matches!(
        renderer.lifecycle.lamp_commands(),
        [EglLampDrawCommand {
            layer: EglDrawLayer::LifecycleResolvedVisual(layer_payload),
            ..
        }] if *layer_payload == payload_id
    ));
    let first_trace = effects::take_effect_trace_test_events();
    assert!(
        first_trace
            .iter()
            .any(|event| event.starts_with("event=effect_graph_execute_end ")),
        "successful lifecycle blur execution must leave positive trace evidence: {first_trace:?}"
    );
    let phase_events = first_trace
        .iter()
        .filter(|event| {
            [
                "event=effect_scene_resolve_",
                "event=effect_graph_compile_",
                "event=effect_demand_plan_",
                "event=renderer_draw_complete_",
            ]
            .iter()
            .any(|prefix| event.starts_with(prefix))
        })
        .map(|event| {
            event
                .split_whitespace()
                .next()
                .expect("frame phase event has a name")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        phase_events,
        [
            "event=effect_scene_resolve_begin",
            "event=effect_scene_resolve_end",
            "event=effect_graph_compile_begin",
            "event=effect_graph_compile_end",
            "event=effect_demand_plan_begin",
            "event=effect_demand_plan_end",
            "event=renderer_draw_complete_begin",
            "event=renderer_draw_complete_end",
        ],
        "normal effect frames preserve top-level phase boundary ordering: {first_trace:?}"
    );
    assert!(
        !first_trace
            .iter()
            .any(|event| event.contains("InvalidCheckpointSource"))
    );

    let source_signature = renderer
        .lifecycle
        .resolved_visual_source_signature(payload_id)
        .expect("successful blur capture owns a resolved texture");
    // Canonical effects may change while the retained lifecycle payload
    // and its resolved visual stay frozen through minimize-to-restore.
    resolved.effects = ResolvedEffectScene::new(999, Vec::new());
    resolved.lifecycle.sampled_at = AnimationTime::from_nanos(2);
    resolved.lifecycle.samples[0].direction = LifecycleDirection::Restore;
    resolved.lifecycle.samples[0].progress = 0.55;
    assert!(Arc::ptr_eq(
        &source_scene,
        &resolved.lifecycle.samples[0].visual_source.effect_scene
    ));
    effects::clear_effect_trace_test_events();
    let request = frame_renderer.egl_scene_draw_request(
        320,
        200,
        &resolved,
        &server,
        &input_state,
        crate::native_output::NativeCursorRenderMode::Hardware,
        Some(OutputDamage::Full),
    );
    let reverse_outcome = renderer
        .draw_scene(&egl, display, egl_surface, request)
        .expect("reversed lifecycle blur frame draws");
    let reverse_evidence = match reverse_outcome {
        EglFrameOutcome::Rendered {
            lifecycle_evidence, ..
        } => lifecycle_evidence,
        EglFrameOutcome::LifecycleFallback { fallbacks, .. } => {
            panic!("reversed retained blur unexpectedly fell back: {fallbacks:?}")
        }
        EglFrameOutcome::Skipped { reason, .. } => {
            panic!("full-damage reversed lifecycle blur was skipped: {reason:?}")
        }
    };
    assert!(reverse_evidence.contains(identity, payload_id, 604));
    assert!(
        renderer
            .lifecycle
            .is_visual_source_ready(payload_id, &renderer.effect_runtime)
    );
    assert_eq!(
        renderer
            .lifecycle
            .resolved_visual_source_signature(payload_id)
            .expect("reversal retains the resolved visual"),
        source_signature
    );
    let reverse_trace = effects::take_effect_trace_test_events();
    assert!(
        reverse_trace
            .iter()
            .any(|event| event.starts_with("event=effect_scene_resolve_begin ")),
        "restore reversal must prove trace capture is active: {reverse_trace:?}"
    );
    assert!(
        !reverse_trace
            .iter()
            .any(|event| event.contains("effect_graph_execute"))
    );

    let failed_scene = lifecycle_blur_effect_scene(
        604,
        EffectProgramId::new(0x7fff).expect("unknown test program id is valid"),
    );
    resolved.lifecycle =
        lifecycle_effect_sample(0.4, LifecycleDirection::Minimize, 604, 2, failed_scene);
    let failed_identity = resolved.lifecycle.samples[0].presentation_identity;
    let failed_payload = resolved.lifecycle.samples[0].payload_id;
    effects::clear_effect_trace_test_events();
    let request = frame_renderer.egl_scene_draw_request(
        320,
        200,
        &resolved,
        &server,
        &input_state,
        crate::native_output::NativeCursorRenderMode::Hardware,
        Some(OutputDamage::Full),
    );
    let failure_outcome = renderer
        .draw_scene(&egl, display, egl_surface, request)
        .expect("invalid frozen effect graph is contained as lifecycle fallback");
    let EglFrameOutcome::LifecycleFallback { fallbacks, .. } = failure_outcome else {
        panic!("invalid frozen effect graph must not render a raw Lamp");
    };
    assert_eq!(fallbacks.failed.len(), 1);
    assert_eq!(fallbacks.failed[0].presentation_identity, failed_identity);
    assert_eq!(fallbacks.failed[0].payload_id, failed_payload);
    assert_eq!(
        fallbacks.failed[0].reason,
        LifecycleRenderFallbackReason::ResolvedSourceCapture
    );
    assert!(
        !renderer
            .lifecycle
            .is_visual_source_ready(failed_payload, &renderer.effect_runtime)
    );
    assert!(
        !renderer
            .lifecycle
            .evidence()
            .contains(failed_identity, failed_payload, 604)
    );
    let failure_trace = effects::take_effect_trace_test_events();
    let capture_failures = failure_trace
        .iter()
        .filter(|event| event.starts_with("event=lifecycle_source_capture_failure "))
        .collect::<Vec<_>>();
    assert_eq!(capture_failures.len(), 1, "{failure_trace:?}");
    assert!(
        capture_failures[0].contains("stage=graph_compile "),
        "invalid frozen effect graph must be classified at compile time: {capture_failures:?}"
    );
    assert!(
        capture_failures[0].contains("error=MissingProgram("),
        "trace must identify the missing-program compile failure: {capture_failures:?}"
    );
}

#[test]
fn egl_lifecycle_resolved_visual_leaves_unused_visual_bounds_transparent() {
    for framebuffer_origin in [
        OutputFramebufferOrigin::BottomLeft,
        OutputFramebufferOrigin::TopLeftScanout,
    ] {
        assert_lifecycle_resolved_visual_isolation_for_origin(framebuffer_origin);
    }
}

fn assert_lifecycle_resolved_visual_isolation_for_origin(
    framebuffer_origin: OutputFramebufferOrigin,
) {
    let mut harness = GlesEffectTestHarness::new(320, 200);
    harness.install_texture_backed_output();

    let mut buffer_ids = BufferIdAllocator::default();
    let background = lifecycle_test_surface(613, 0, 0, 320, 200, 0xffff_00ff, &mut buffer_ids);
    let mut retained_window =
        lifecycle_test_surface(614, 60, 60, 180, 100, 0x8012_7514, &mut buffer_ids);
    retained_window.placement = SurfacePlacement::absolute_root_at(0, 0);
    let window_id = oblivion_one::compositor::WindowId::from_raw(3).expect("window id");
    let socket_name = format!("typhon-lifecycle-isolation-egl-{}", std::process::id());
    let mut server =
        oblivion_one::compositor::OwnCompositorServer::bind_cpu_composition(&socket_name)
            .expect("test compositor binds");
    server.install_native_frame_test_scene_with_server_decorations(
        vec![retained_window.clone()],
        &[(retained_window.surface_id, window_id)],
        None,
    );
    let mut resolved = crate::native_output::ResolvedNativeFrameScene::from_server_at(
        &server,
        AnimationTime::from_nanos(0),
    );
    resolved.surfaces = std::borrow::Cow::Owned(vec![background]);
    resolved.effects = lifecycle_blur_effect_scene(
        613,
        oblivion_one::effects::builtin_background_blur_program_id(),
    );
    resolved.lifecycle_surfaces = vec![retained_window];
    resolved.lifecycle = lifecycle_effect_sample(
        0.45,
        LifecycleDirection::Minimize,
        614,
        1,
        lifecycle_blur_effect_scene_for_rect(
            614,
            oblivion_one::effects::builtin_background_blur_program_id(),
            EffectRect::new(60, 60, 180, 100).expect("blur owner rect"),
        ),
    );
    resolved.lifecycle.samples[0].window_id = window_id;
    resolved.lifecycle.samples[0].visual_source.window_id = window_id;
    resolved.lifecycle.samples[0].visual_group = LifecycleVisualGroup::from_bounds(
        PresentationRect::new(60.0, 60.0, 180.0, 100.0).expect("canonical client rect"),
        PresentationRect::new(40.0, 20.0, 220.0, 160.0).expect("canonical visual rect"),
        PresentationRect::new(60.0, 60.0, 180.0, 100.0).expect("presented client rect"),
        PresentationRect::new(220.0, 130.0, 60.0, 40.0).expect("anchor rect"),
        320,
        200,
    )
    .expect("expanded lifecycle visual group");
    assert_eq!(
        resolved.lifecycle.samples[0]
            .visual_group
            .presented_source_visual_rect,
        PresentationRect::new(40.0, 20.0, 220.0, 160.0).expect("expanded bounds"),
    );
    resolved.lifecycle_decorations =
        server.native_decoration_render_instances(&resolved.lifecycle_surfaces);
    assert_eq!(resolved.lifecycle_decorations.len(), 1);
    let decoration = &resolved.lifecycle_decorations[0];
    let (decoration_x, decoration_y, _, _) = decoration.scene_snapshot().bounds();
    assert!(
        decoration_x > 50,
        "fixture has unused space beside its frozen SSD"
    );
    assert!(
        decoration_y < 60,
        "fixture SSD extends above the retained client: {decoration_y}"
    );

    let payload_id = resolved.lifecycle.samples[0].payload_id;
    let input_state = crate::native_output::NativeInputState::new(320, 200);
    let mut frame_renderer = crate::native_output::NativeFrameRenderer::default();
    let request = frame_renderer.egl_scene_draw_request(
        320,
        200,
        &resolved,
        &server,
        &input_state,
        crate::native_output::NativeCursorRenderMode::Hardware,
        Some(OutputDamage::Full),
    );
    let target = EglOutputRenderTarget {
        framebuffer: harness
            .test_output_framebuffer
            .expect("output has an FBO-backed render target"),
        sampleable_texture: harness.test_output_texture,
        width: 320,
        height: 200,
        buffer_age: BufferAge::Unsupported,
        framebuffer_origin,
    };
    let outcome = harness
        .renderer
        .draw_scene_to_target(&harness.egl, harness.display, target, request)
        .expect("lifecycle isolation frame draws");
    assert!(matches!(outcome, EglFrameOutcome::Rendered { .. }));

    let texture = harness
        .renderer
        .lifecycle
        .resolved_visual_texture(payload_id)
        .expect("resolved lifecycle visual is retained")
        .clone();
    // Supply a deterministic, opaque desktop baseline to the real output
    // before re-running the retained source capture.
    harness.renderer.bind_active_output_framebuffer();
    unsafe {
        harness.gl.disable(glow::SCISSOR_TEST);
        harness.gl.disable(glow::BLEND);
        harness.gl.clear_color(1.0, 0.0, 1.0, 1.0);
        harness.gl.clear(glow::COLOR_BUFFER_BIT);
    }
    let output_before = read_effect_test_pixels(&harness.gl, 320, 200);
    let desktop_gl_y = match framebuffer_origin {
        OutputFramebufferOrigin::BottomLeft => 200 - 1 - 30,
        OutputFramebufferOrigin::TopLeftScanout => 30,
    };
    let desktop_pixel = effect_test_pixel(&output_before, 320, 50, desktop_gl_y);
    assert_eq!(desktop_pixel, [255, 0, 255, 255]);
    let source = harness
        .renderer
        .lifecycle
        .visual_sources()
        .next()
        .expect("frozen lifecycle effect source is retained")
        .clone();
    let lamp = harness.renderer.lifecycle.samples()[0];
    {
        let renderer = &mut harness.renderer;
        let mut context = LifecycleRenderContext::new(
            &harness.gl,
            &mut renderer.scene_state,
            &mut renderer.effect_runtime,
            renderer.resources.texture_view(),
        );
        renderer
            .lifecycle
            .capture_visual_source_for_test(
                &mut context,
                &source,
                lamp,
                texture.clone(),
                framebuffer_origin,
            )
            .expect("manual resolved-source recapture succeeds");
    }
    harness.renderer.bind_active_output_framebuffer();
    let output_after = read_effect_test_pixels(&harness.gl, 320, 200);
    assert_eq!(
        output_after, output_before,
        "lifecycle capture leaves output pixels unchanged"
    );

    let pixels = read_effect_texture_pixels(
        &mut harness,
        &texture,
        texture.key.width,
        texture.key.height,
    );
    // Output-space (50, 30) is within the 40,20–260,180 visual bounds,
    // but above and outside the retained client/effect owner at 60,60–240,160.
    let unused_bounds_pixel = effect_test_pixel(
        &pixels,
        texture.key.width,
        50 - 40,
        texture.key.height - 1 - (30 - 20),
    );
    assert_ne!(unused_bounds_pixel, desktop_pixel);
    assert_eq!(
        unused_bounds_pixel,
        [0, 0, 0, 0],
        "unused visual bounds must stay transparent instead of retaining the magenta desktop"
    );
    let owned_pixel = effect_test_pixel(
        &pixels,
        texture.key.width,
        100 - 40,
        texture.key.height - 1 - (100 - 20),
    );
    assert!(
        owned_pixel[0] > 25 && owned_pixel[2] > 25 && owned_pixel[1] > 0,
        "blur-owned content still includes the magenta backdrop input: {owned_pixel:?}"
    );

    // The frozen SSD's top extension is retained visual ownership even
    // though it lies above the client. The adjacent unused bounds stay clear.
    let ssd_output_x = decoration_x + 20;
    let ssd_output_y = decoration_y + (60 - decoration_y) / 2;
    assert!((40..260).contains(&ssd_output_x));
    assert!((20..60).contains(&ssd_output_y));
    let ssd_pixel = effect_test_pixel(
        &pixels,
        texture.key.width,
        ssd_output_x as u32 - 40,
        texture.key.height - 1 - (ssd_output_y as u32 - 20),
    );
    assert!(
        ssd_pixel[3] > 0,
        "frozen SSD titlebar must remain in the isolated lifecycle visual: {ssd_pixel:?}"
    );
    let unused_adjacent_pixel = effect_test_pixel(
        &pixels,
        texture.key.width,
        50 - 40,
        texture.key.height - 1 - (ssd_output_y as u32 - 20),
    );
    assert_eq!(
        unused_adjacent_pixel,
        [0, 0, 0, 0],
        "unused visual bounds beside the SSD must remain transparent"
    );
}

#[test]
fn egl_lifecycle_resolved_visual_preserves_premultiplied_alpha() {
    let mut harness = GlesEffectTestHarness::new(320, 200);
    harness.install_texture_backed_output();

    let node_source = oblivion_one::effects::EffectNodeId::new(1).unwrap();
    let program_id = EffectProgramId::new(9_002).expect("alpha test program id");
    let alpha_program = oblivion_one::effects::EffectProgram {
        id: program_id,
        nodes: vec![oblivion_one::effects::EffectNode::source(
            node_source,
            oblivion_one::effects::EffectSource::TargetContent,
        )],
        output: node_source,
        working_space: EffectWorkingSpace::OutputEncodedSrgb,
        alpha_mode: EffectAlphaMode::Preserve,
        outsets: oblivion_one::effects::EffectOutsets::ZERO,
        frame_demand: EffectFrameDemand::OnDamage,
        failure_policy: EffectFailurePolicy::Passthrough,
    };
    let mut registry = oblivion_one::effects::EffectRegistry::empty();
    registry
        .insert(oblivion_one::effects::validate_effect_program(alpha_program).unwrap())
        .expect("alpha-preserving test program registers");
    harness.renderer.set_effect_registry(registry);

    let mut buffer_ids = BufferIdAllocator::default();
    let background = lifecycle_test_surface(615, 0, 0, 320, 200, 0xff30_4050, &mut buffer_ids);
    let mut retained_window =
        lifecycle_test_surface(616, 60, 60, 180, 100, 0x8040_3030, &mut buffer_ids);
    retained_window.placement = SurfacePlacement::absolute_root_at(0, 0);
    let window_id = oblivion_one::compositor::WindowId::from_raw(1).expect("window id");
    let socket_name = format!("typhon-lifecycle-alpha-egl-{}", std::process::id());
    let mut server =
        oblivion_one::compositor::OwnCompositorServer::bind_cpu_composition(&socket_name)
            .expect("test compositor binds");
    server.install_native_frame_test_scene_with_server_decorations(
        vec![background.clone(), retained_window.clone()],
        &[
            (
                background.surface_id,
                oblivion_one::compositor::WindowId::from_raw(2).expect("background window id"),
            ),
            (retained_window.surface_id, window_id),
        ],
        None,
    );
    let mut resolved = crate::native_output::ResolvedNativeFrameScene::from_server_at(
        &server,
        AnimationTime::from_nanos(0),
    );
    resolved.surfaces = std::borrow::Cow::Owned(vec![background]);
    resolved.lifecycle_surfaces = vec![retained_window];
    let alpha_scene = lifecycle_blur_effect_scene_for_rect(
        616,
        program_id,
        EffectRect::new(60, 60, 180, 100).expect("alpha owner rect"),
    );
    resolved.lifecycle =
        lifecycle_effect_sample(0.45, LifecycleDirection::Minimize, 616, 1, alpha_scene);
    resolved.lifecycle.samples[0].visual_group = LifecycleVisualGroup::from_bounds(
        PresentationRect::new(60.0, 60.0, 180.0, 100.0).expect("canonical client rect"),
        PresentationRect::new(40.0, 20.0, 220.0, 160.0).expect("canonical visual rect"),
        PresentationRect::new(60.0, 60.0, 180.0, 100.0).expect("presented client rect"),
        PresentationRect::new(220.0, 130.0, 60.0, 40.0).expect("anchor rect"),
        320,
        200,
    )
    .expect("expanded alpha visual group");
    let payload_id = resolved.lifecycle.samples[0].payload_id;
    let input_state = crate::native_output::NativeInputState::new(320, 200);
    let mut frame_renderer = crate::native_output::NativeFrameRenderer::default();
    let request = frame_renderer.egl_scene_draw_request(
        320,
        200,
        &resolved,
        &server,
        &input_state,
        crate::native_output::NativeCursorRenderMode::Hardware,
        Some(OutputDamage::Full),
    );
    let target = EglOutputRenderTarget {
        framebuffer: harness
            .test_output_framebuffer
            .expect("output has an FBO-backed render target"),
        sampleable_texture: harness.test_output_texture,
        width: 320,
        height: 200,
        buffer_age: BufferAge::Unsupported,
        framebuffer_origin: OutputFramebufferOrigin::BottomLeft,
    };
    let outcome = harness
        .renderer
        .draw_scene_to_target(&harness.egl, harness.display, target, request)
        .expect("alpha-preserving lifecycle frame draws");
    assert!(matches!(outcome, EglFrameOutcome::Rendered { .. }));
    let texture = harness
        .renderer
        .lifecycle
        .resolved_visual_texture(payload_id)
        .expect("alpha-preserving lifecycle resource is retained")
        .clone();
    let pixels = read_effect_texture_pixels(
        &mut harness,
        &texture,
        texture.key.width,
        texture.key.height,
    );
    let pixel = effect_test_pixel(
        &pixels,
        texture.key.width,
        100 - 40,
        texture.key.height - 1 - (100 - 20),
    );
    assert!(
        pixel[3] > 0 && pixel[3] < 255,
        "lifecycle capture must retain non-opaque source alpha: {pixel:?}"
    );
    assert!(
        pixel[..3].iter().all(|channel| *channel <= pixel[3]),
        "lifecycle capture must retain premultiplied RGB: {pixel:?}"
    );
}

#[test]
fn egl_lifecycle_background_blur_renders_retained_content_on_top_left_scanout() {
    let mut harness = GlesEffectTestHarness::new(320, 200);
    harness.install_texture_backed_output();
    harness.renderer.effect_runtime.effect_trace =
        effects::EffectExecutionTrace::enabled_for_test();

    let mut buffer_ids = BufferIdAllocator::default();
    let background = lifecycle_test_surface_with_row_colors(
        613,
        0,
        0,
        320,
        200,
        [0xff30_6080, 0xff80_6040, 0xff60_3080],
        &mut buffer_ids,
    );
    let mut retained_window = lifecycle_test_surface_with_row_colors(
        614,
        60,
        40,
        180,
        110,
        [0xffee_2211, 0xff12_ea14, 0xff12_22_ee],
        &mut buffer_ids,
    );
    retained_window.placement = SurfacePlacement::absolute_root_at(0, 0);
    let window_id = oblivion_one::compositor::WindowId::from_raw(1).expect("window id");
    let socket_name = format!("typhon-lifecycle-blur-top-left-egl-{}", std::process::id());
    let mut server =
        oblivion_one::compositor::OwnCompositorServer::bind_cpu_composition(&socket_name)
            .expect("test compositor binds");
    server.install_native_frame_test_scene_with_server_decorations(
        vec![background.clone()],
        &[(background.surface_id, window_id)],
        None,
    );
    let mut resolved = crate::native_output::ResolvedNativeFrameScene::from_server_at(
        &server,
        AnimationTime::from_nanos(0),
    );
    resolved.surfaces = std::borrow::Cow::Owned(vec![background]);
    // Keep an ordinary effect graph alive while the retained lifecycle
    // source runs its own background-blur graph during this frame.
    resolved.effects = lifecycle_blur_effect_scene(
        613,
        oblivion_one::effects::builtin_background_blur_program_id(),
    );
    resolved.lifecycle_surfaces = vec![retained_window];
    resolved.lifecycle = lifecycle_effect_sample(
        0.45,
        LifecycleDirection::Minimize,
        614,
        1,
        lifecycle_blur_effect_scene(
            614,
            oblivion_one::effects::builtin_background_blur_program_id(),
        ),
    );
    let identity = resolved.lifecycle.samples[0].presentation_identity;
    let payload_id = resolved.lifecycle.samples[0].payload_id;
    let input_state = crate::native_output::NativeInputState::new(320, 200);
    let mut frame_renderer = crate::native_output::NativeFrameRenderer::default();
    let request = frame_renderer.egl_scene_draw_request(
        320,
        200,
        &resolved,
        &server,
        &input_state,
        crate::native_output::NativeCursorRenderMode::Hardware,
        Some(OutputDamage::Full),
    );
    let target = EglOutputRenderTarget {
        framebuffer: harness
            .test_output_framebuffer
            .expect("output has an FBO-backed render target"),
        sampleable_texture: harness.test_output_texture,
        width: 320,
        height: 200,
        buffer_age: BufferAge::Unsupported,
        framebuffer_origin: OutputFramebufferOrigin::TopLeftScanout,
    };
    effects::clear_effect_trace_test_events();
    let outcome = harness
        .renderer
        .draw_scene_to_target(&harness.egl, harness.display, target, request)
        .expect("native-origin lifecycle blur frame draws");
    let evidence = match outcome {
        EglFrameOutcome::Rendered {
            lifecycle_evidence, ..
        } => lifecycle_evidence,
        EglFrameOutcome::LifecycleFallback { fallbacks, .. } => {
            panic!("valid native-origin lifecycle blur fell back: {fallbacks:?}")
        }
        EglFrameOutcome::Skipped { reason, .. } => {
            panic!("full-damage native-origin lifecycle blur was skipped: {reason:?}")
        }
    };
    assert!(evidence.contains(identity, payload_id, 614));
    assert!(
        harness
            .renderer
            .lifecycle
            .is_visual_source_ready(payload_id, &harness.renderer.effect_runtime)
    );
    let trace = effects::take_effect_trace_test_events();
    let graph_begins = trace
        .iter()
        .enumerate()
        .filter(|(_, event)| event.starts_with("event=effect_graph_execute_begin "))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let graph_ends = trace
        .iter()
        .enumerate()
        .filter(|(_, event)| event.starts_with("event=effect_graph_execute_end "))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let graph_release_begins = trace
        .iter()
        .enumerate()
        .filter(|(_, event)| event.starts_with("event=effect_graph_release_begin "))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let graph_release_ends = trace
        .iter()
        .enumerate()
        .filter(|(_, event)| event.starts_with("event=effect_graph_release_end "))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    assert_eq!(
        graph_begins.len(),
        2,
        "outer and lifecycle graphs: {trace:?}"
    );
    assert_eq!(graph_ends.len(), 2, "outer and lifecycle graphs: {trace:?}");
    assert!(
        graph_begins[0] < graph_begins[1]
            && graph_begins[1] < graph_ends[0]
            && graph_ends[0] < graph_ends[1],
        "lifecycle graph execution must nest inside the outer graph phase: {trace:?}"
    );
    assert_eq!(graph_release_begins.len(), 2, "graph releases: {trace:?}");
    assert_eq!(graph_release_ends.len(), 2, "graph releases: {trace:?}");
    assert!(
        graph_ends[0] < graph_release_begins[0]
            && graph_release_begins[0] < graph_release_ends[0]
            && graph_release_ends[0] < graph_ends[1]
            && graph_ends[1] < graph_release_begins[1]
            && graph_release_begins[1] < graph_release_ends[1],
        "the nested graph must release its leases before the outer graph ends, and the outer graph releases afterward: {trace:?}"
    );
    let (checkpoint_entries, _) = harness
        .renderer
        .effect_runtime
        .effect_resources
        .checkpoint_cache_stats();
    assert_eq!(
        harness
            .renderer
            .effect_runtime
            .effect_resources
            .metrics()
            .checked_out_texture_count,
        harness.renderer.lifecycle.resolved_visual_resource_count() + checkpoint_entries,
        "both graphs release every temporary lease; only retained visuals and checkpoint cache entries remain checked out"
    );

    let texture = harness
        .renderer
        .lifecycle
        .resolved_visual_texture(payload_id)
        .expect("resolved lifecycle visual is retained")
        .clone();
    let source_commands = harness
        .renderer
        .lifecycle
        .source_commands_for_payload(payload_id)
        .expect("retained lifecycle source commands are prepared");
    assert!(
        source_commands
            .iter()
            .any(|command| command.layer == EglDrawLayer::Surface(614)),
        "the retained source includes its window surface draw command: {source_commands:?}"
    );
    let source_surface_command = source_commands
        .iter()
        .find(|command| command.layer == EglDrawLayer::Surface(614))
        .expect("retained window draw command exists");
    assert_eq!(
        source_surface_command.bounds,
        EglRect::new(60.0, 40.0, 180.0, 110.0)
    );
    let surface_texture = harness
        .renderer
        .resources
        .texture_view()
        .texture_for_surface(614)
        .expect("retained surface texture is realized");
    let surface_framebuffer = unsafe {
        harness
            .gl
            .create_framebuffer()
            .expect("surface readback framebuffer creates")
    };
    unsafe {
        harness
            .gl
            .bind_framebuffer(glow::FRAMEBUFFER, Some(surface_framebuffer));
        harness.gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(surface_texture),
            0,
        );
        assert_eq!(
            harness.gl.check_framebuffer_status(glow::FRAMEBUFFER),
            glow::FRAMEBUFFER_COMPLETE,
            "retained source readback framebuffer is complete"
        );
    }
    let surface_pixels = read_effect_test_pixels(&harness.gl, 180, 110);
    harness.renderer.establish_ordinary_scene_state();
    unsafe { harness.gl.delete_framebuffer(surface_framebuffer) };
    let source_top = effect_test_pixel(&surface_pixels, 180, 90, 5);
    let source_middle = effect_test_pixel(&surface_pixels, 180, 90, 55);
    let source_bottom = effect_test_pixel(&surface_pixels, 180, 90, 100);
    assert!(
        source_top[3] > 240 && source_middle[3] > 240 && source_bottom[3] > 240,
        "retained source test surface must be opaque: top={source_top:?}, middle={source_middle:?}, bottom={source_bottom:?}"
    );
    assert!(source_top[0] > source_top[1] && source_top[0] > source_top[2]);
    assert!(source_middle[1] > source_middle[0] && source_middle[1] > source_middle[2]);
    assert!(source_bottom[2] > source_bottom[0] && source_bottom[2] > source_bottom[1]);
    let resolved_pixels = read_effect_texture_pixels(
        &mut harness,
        &texture,
        texture.key.width,
        texture.key.height,
    );
    let sample_center_x = 90;
    let top = effect_test_pixel(
        &resolved_pixels,
        texture.key.width,
        sample_center_x,
        109 - 10,
    );
    let middle = effect_test_pixel(
        &resolved_pixels,
        texture.key.width,
        sample_center_x,
        109 - 55,
    );
    let bottom = effect_test_pixel(
        &resolved_pixels,
        texture.key.width,
        sample_center_x,
        109 - 100,
    );
    assert!(
        top[0] > top[1] && top[0] > top[2],
        "resolved visual top should contain the red retained-window band: top={top:?}, middle={middle:?}, bottom={bottom:?}"
    );
    assert!(
        middle[1] > middle[0] && middle[1] > middle[2],
        "resolved visual middle should contain the green retained-window band: {middle:?}"
    );
    assert!(
        bottom[2] > bottom[0] && bottom[2] > bottom[1],
        "resolved visual bottom should contain the blue retained-window band: {bottom:?}"
    );

    harness.renderer.bind_active_output_framebuffer();
    let output_pixels = read_effect_test_pixels(&harness.gl, 320, 200);
    let (mut red_y_sum, mut red_count, mut blue_y_sum, mut blue_count) =
        (0_u64, 0_u64, 0_u64, 0_u64);
    for y in 0..200 {
        for x in 0..320 {
            let pixel = effect_test_pixel(&output_pixels, 320, x, y);
            if pixel[0] > pixel[1].saturating_mul(2) && pixel[0] > pixel[2].saturating_mul(2) {
                red_y_sum += u64::from(y);
                red_count += 1;
            }
            if pixel[2] > pixel[0].saturating_mul(2) && pixel[2] > pixel[1].saturating_mul(2) {
                blue_y_sum += u64::from(y);
                blue_count += 1;
            }
        }
    }
    assert!(red_count > 0, "Lamp output contains the retained red band");
    assert!(
        blue_count > 0,
        "Lamp output contains the retained blue band"
    );
    assert!(
        red_y_sum * blue_count < blue_y_sum * red_count,
        "TopLeftScanout Lamp output must keep the red band above the blue band: red={red_y_sum}/{red_count}, blue={blue_y_sum}/{blue_count}"
    );
}

#[test]
fn egl_decoration_commands_emit_titlebar_and_button_primitives() {
    let socket_name = format!("typhon-floating-ssd-egl-{}", std::process::id());
    let mut server =
        oblivion_one::compositor::OwnCompositorServer::bind_cpu_composition(&socket_name)
            .expect("bind compositor for EGL decoration command regression");
    let width = 320;
    let height = 200;
    let surface = RenderableSurface {
        surface_id: 603,
        x: 0,
        y: 0,
        width,
        height,
        placement: SurfacePlacement::root(),
        render_backend: SurfaceRenderBackend::NativeWayland,
        render_placement: None,
        visual_clip: None,
        render_target_size: None,
        generation: 1,
        commit_sequence: SurfaceCommitSequence::initial(),
        buffer: CommittedSurfaceBuffer::shm_snapshot(
            BufferIdAllocator::default()
                .allocate()
                .expect("test buffer identity"),
            BufferSize::new(width, height).expect("test surface size"),
            vec![0xff12_3456; (width * height) as usize],
        ),
        viewport_source: None,
        viewport_destination: None,
        buffer_scale: 1,
        buffer_transform: wayland_server::protocol::wl_output::Transform::Normal,
        damage: RenderableSurfaceDamage::full(),
        opaque_region: SurfaceOpaqueRegion::None,
    };
    let window_id = oblivion_one::compositor::WindowId::from_raw(3).expect("test window id");
    server.install_native_frame_test_scene_with_server_decorations(
        vec![surface.clone()],
        &[(603, window_id)],
        None,
    );
    let decorations = server.native_decoration_render_instances(&[surface]);
    assert_eq!(decorations.len(), 1);
    let decoration = &decorations[0];
    let (_, _, _, height) = decoration.scene_snapshot().bounds();
    assert!(height > 200, "Floating SSD must add visible chrome height");
    let mut vertices = Vec::new();
    let mut commands = Vec::new();

    push_egl_decoration_instance(
        &mut vertices,
        &mut commands,
        1280,
        800,
        decoration,
        1.0,
        OutputFramebufferOrigin::BottomLeft,
        Some(VisualGroupId::new(1).expect("visual group id")),
    );

    assert_eq!(commands.len(), decoration.primitives().len());
    assert!(
        commands
            .iter()
            .any(|command| { matches!(command.layer, EglDrawLayer::SolidRgba(_)) })
    );
    assert!(
        commands
            .iter()
            .any(|command| { matches!(command.layer, EglDrawLayer::DecorationAsset(_)) })
    );
}
