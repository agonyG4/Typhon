use super::*;

#[test]
fn egl_lifecycle_capture_consumes_the_compositor_precapture_source_after_withdrawal() {
    let mut harness = GlesEffectTestHarness::new(320, 200);
    harness.install_texture_backed_output();

    let mut buffer_ids = BufferIdAllocator::default();
    let background = lifecycle_test_surface(615, 0, 0, 320, 200, 0xff24_4567, &mut buffer_ids);
    let retained = lifecycle_test_surface(616, 60, 40, 180, 110, 0xffee_8844, &mut buffer_ids);
    let window_id = oblivion_one::compositor::WindowId::from_raw(1).expect("window id");
    let root_surface_id = retained.surface_id;
    let socket_name = format!("typhon-lifecycle-precapture-egl-{}", std::process::id());
    let mut server =
        oblivion_one::compositor::OwnCompositorServer::bind_cpu_composition(&socket_name)
            .expect("test compositor binds");
    server.install_native_frame_test_scene(
        vec![background.clone(), retained.clone()],
        &[(root_surface_id, window_id)],
        None,
    );

    let source_rect =
        PresentationRect::new(60.0, 40.0, 180.0, 110.0).expect("lifecycle source geometry");
    let anchor_rect =
        PresentationRect::new(8.0, 8.0, 24.0, 12.0).expect("lifecycle anchor geometry");
    let visual_group = oblivion_one::window_lifecycle_animation::LifecycleVisualGroup::from_bounds(
        source_rect,
        source_rect,
        source_rect,
        anchor_rect,
        320,
        200,
    )
    .expect("lifecycle visual group");
    assert!(server.begin_lifecycle_lamp_for_test(window_id, root_surface_id, visual_group,));

    let admitted = server.lifecycle_scene_sample_at(
        AnimationTime::monotonic_now().expect("lifecycle admission time"),
    );
    let admitted_sample = admitted.samples.first().expect("admitted lifecycle sample");
    let presentation_identity = admitted_sample.presentation_identity;
    let payload_id = admitted_sample.payload_id;
    assert_eq!(server.lifecycle_renderable_surfaces(&admitted).len(), 1);

    server.retire_xwayland_attachment_for_test(root_surface_id);
    let batch_id = server.take_frame_batch_for_render_for_test(616);
    let at = AnimationTime::monotonic_now().expect("renderer sample time");
    let mut resolved = crate::native_output::ResolvedNativeFrameScene::from_server_at(&server, at);
    assert!(
        resolved
            .surfaces
            .iter()
            .all(|surface| surface.surface_id != root_surface_id)
    );
    assert_eq!(resolved.lifecycle_surfaces.len(), 1);
    assert_eq!(
        resolved.lifecycle_surfaces[0].buffer_id(),
        retained.buffer_id()
    );
    resolved.lifecycle.samples[0].visual_source.kind = LifecycleVisualSourceKind::NoOwnedEffects;

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
    let evidence = match harness
        .renderer
        .draw_scene_to_target(&harness.egl, harness.display, target, request)
        .expect("withdrawn source still reaches first lifecycle capture")
    {
        EglFrameOutcome::Rendered {
            lifecycle_evidence, ..
        } => lifecycle_evidence,
        EglFrameOutcome::LifecycleFallback { fallbacks, .. } => {
            panic!("admission-time compositor source fell back: {fallbacks:?}")
        }
        EglFrameOutcome::Skipped { reason, .. } => {
            panic!("full-damage lifecycle capture was skipped: {reason:?}")
        }
    };

    assert!(evidence.contains(presentation_identity, payload_id, root_surface_id));
    assert!(
        harness
            .renderer
            .lifecycle
            .is_visual_source_ready(payload_id, &harness.renderer.effect_runtime)
    );
    assert!(matches!(
        harness.renderer.lifecycle.lamp_commands(),
        [EglLampDrawCommand {
            layer: EglDrawLayer::LifecycleResolvedVisual(layer_payload),
            ..
        }] if *layer_payload == payload_id
    ));

    let rendered_snapshot =
        oblivion_one::window_lifecycle_animation::LifecycleFrameSnapshot::qualified_from_sample(
            &resolved.lifecycle,
            &evidence,
        );
    drop(resolved);
    assert_eq!(
        server.settle_lifecycle_precapture_for_rendered_frame(batch_id, &rendered_snapshot),
        0,
        "SHM capture has no DMA-BUF release obligations"
    );
    assert!(server.lifecycle_renderable_surfaces(&admitted).is_empty());
}
