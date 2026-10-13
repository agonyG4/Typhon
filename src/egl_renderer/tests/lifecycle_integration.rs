use super::*;

#[test]
fn egl_lifecycle_lamp_below_canonical_opaque_window_respects_painter_order() {
    let mut harness = GlesEffectTestHarness::new(320, 200);
    harness.install_texture_backed_output();

    let mut buffer_ids = BufferIdAllocator::default();
    let lower = lifecycle_test_surface(630, 0, 0, 320, 200, 0xff22_3322, &mut buffer_ids);
    let retained = lifecycle_test_surface(631, 60, 40, 180, 110, 0xffee_2211, &mut buffer_ids);
    let upper = lifecycle_test_surface(632, 100, 0, 100, 70, 0xff11_22ee, &mut buffer_ids);
    let lower_window = oblivion_one::compositor::WindowId::from_raw(1).expect("lower window");
    let retained_window = oblivion_one::compositor::WindowId::from_raw(2).expect("retained window");
    let upper_window = oblivion_one::compositor::WindowId::from_raw(3).expect("upper window");
    let socket_name = format!(
        "typhon-lifecycle-canonical-overlap-egl-{}",
        std::process::id()
    );
    let mut server =
        oblivion_one::compositor::OwnCompositorServer::bind_cpu_composition(&socket_name)
            .expect("test compositor binds");
    let windows = [
        (lower.surface_id, lower_window),
        (retained.surface_id, retained_window),
        (upper.surface_id, upper_window),
    ];
    server.install_native_frame_test_scene_with_server_decorations(
        vec![lower.clone(), retained.clone(), upper.clone()],
        &windows,
        None,
    );
    let lifecycle_decorations =
        server.native_decoration_render_instances(std::slice::from_ref(&retained));
    assert_eq!(lifecycle_decorations.len(), 1);
    server.install_native_frame_test_scene_with_server_decorations(
        vec![lower.clone(), upper.clone()],
        &windows,
        None,
    );

    let mut resolved = crate::native_output::ResolvedNativeFrameScene::from_server_at(
        &server,
        AnimationTime::from_nanos(0),
    );
    resolved.surfaces = std::borrow::Cow::Owned(vec![lower.clone(), upper.clone()]);
    resolved.surface_scene_node_ids =
        std::borrow::Cow::Owned(resolved.surface_scene_node_ids.as_ref().to_vec());
    resolved.presentation_owner_root_surface_ids = std::borrow::Cow::Owned(
        resolved
            .presentation_owner_root_surface_ids
            .as_ref()
            .to_vec(),
    );
    resolved.popup_surface_ids =
        std::borrow::Cow::Owned(resolved.popup_surface_ids.as_ref().to_vec());
    resolved.lifecycle_surfaces = vec![retained.clone()];
    resolved.lifecycle = lifecycle_effect_sample(
        0.0,
        LifecycleDirection::Minimize,
        retained.surface_id,
        1,
        ResolvedEffectScene::default(),
    );
    resolved.lifecycle.samples[0].window_id = retained_window;
    resolved.lifecycle.samples[0].visual_source.window_id = retained_window;
    resolved.lifecycle.samples[0].visual_group = LifecycleVisualGroup::from_bounds(
        PresentationRect::new(60.0, 40.0, 180.0, 110.0).expect("canonical client rect"),
        PresentationRect::new(40.0, 20.0, 220.0, 160.0).expect("canonical visual rect"),
        PresentationRect::new(60.0, 40.0, 180.0, 110.0).expect("presented client rect"),
        PresentationRect::new(220.0, 130.0, 60.0, 40.0).expect("anchor rect"),
        320,
        200,
    )
    .expect("decorated lifecycle visual group");
    resolved.lifecycle_decorations = lifecycle_decorations;
    assert_eq!(resolved.lifecycle_decorations.len(), 1);
    let (scene_band, layer_rank, stack_position, stable_order) =
        server.native_scene_root_stack_key(retained.surface_id, 1);
    resolved.lifecycle.samples[0].painter_order = LifecyclePainterOrder {
        scene_band,
        layer_rank,
        stack_position,
        stable_order: stable_order as u64,
    };
    resolved.lifecycle.samples[0].visual_source.kind = LifecycleVisualSourceKind::NoOwnedEffects;
    let canonical_groups = oblivion_one::compositor::WindowVisualGroup::stack_order_with_popups(
        resolved.surfaces.as_ref(),
        &resolved.decorations,
        resolved.popup_surface_ids.as_ref(),
    );
    let lifecycle_order = oblivion_one::compositor::window_exit_stack_order_key(
        scene_band,
        layer_rank,
        stack_position,
        true,
        usize::try_from(stable_order).unwrap_or(usize::MAX),
    );
    let insertion_group = canonical_groups
        .iter()
        .enumerate()
        .position(|(group_index, group)| {
            let (band, rank, position, _) =
                server.native_scene_root_stack_key(group.root_surface_id(), group_index);
            oblivion_one::compositor::window_exit_stack_order_key(
                band,
                rank,
                position,
                false,
                group_index,
            ) > lifecycle_order
        })
        .unwrap_or(canonical_groups.len());
    let presentation_identity = resolved.lifecycle.samples[0].presentation_identity;
    resolved.lifecycle_marker_insertions = vec![(presentation_identity, insertion_group)];
    resolved.lifecycle_snapshot =
        oblivion_one::window_lifecycle_animation::LifecycleFrameSnapshot::from_sample(
            &resolved.lifecycle,
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
        .expect("canonical overlap frame draws");
    assert!(matches!(outcome, EglFrameOutcome::Rendered { .. }));

    harness.renderer.bind_active_output_framebuffer();
    let pixels = read_effect_test_pixels(&harness.gl, 320, 200);
    let commands = &harness.renderer.scene_state.commands;
    let lower_index = commands
        .iter()
        .position(|command| command.layer == EglDrawLayer::Surface(630))
        .expect("lower canonical window command");
    let marker_command = commands
        .iter()
        .find(|command| command.layer == EglDrawLayer::LifecycleOverlay(presentation_identity))
        .expect("frame-local lifecycle marker");
    let marker_index = commands
        .iter()
        .position(|command| command.layer == EglDrawLayer::LifecycleOverlay(presentation_identity))
        .expect("frame-local lifecycle marker");
    let upper_command = commands
        .iter()
        .find(|command| command.layer == EglDrawLayer::Surface(632))
        .expect("upper canonical window command");
    let upper_index = commands
        .iter()
        .position(|command| command.layer == EglDrawLayer::Surface(632))
        .expect("upper canonical window command");
    assert!(lower_index < marker_index && marker_index < upper_index);
    let overlap_left = marker_command
        .bounds
        .x()
        .max(upper_command.bounds.x())
        .max(0.0);
    let overlap_top = marker_command
        .bounds
        .y()
        .max(upper_command.bounds.y())
        .max(0.0);
    let overlap_right = (marker_command.bounds.x() + marker_command.bounds.width())
        .min(upper_command.bounds.x() + upper_command.bounds.width())
        .min(320.0);
    let overlap_bottom = (marker_command.bounds.y() + marker_command.bounds.height())
        .min(upper_command.bounds.y() + upper_command.bounds.height())
        .min(200.0);
    assert!(
        overlap_left < overlap_right && overlap_top < overlap_bottom,
        "retained marker and upper canonical window must overlap inside the output"
    );
    let overlap_x = ((overlap_left + overlap_right) * 0.5).floor() as u32;
    let overlap_y = ((overlap_top + overlap_bottom) * 0.5).floor() as u32;
    let overlap = effect_test_pixel(&pixels, 320, overlap_x, 200 - 1 - overlap_y);
    assert!(
        overlap[2] > overlap[0].saturating_mul(2),
        "the opaque upper canonical window must win above the retained Lamp at ({overlap_x}, {overlap_y}): {overlap:?}"
    );

    let payload_id = resolved.lifecycle.samples[0].payload_id;
    let retained_surface_snapshot_id = resolved.lifecycle_surfaces[0].buffer_id();
    let lifecycle_snapshot_signature = resolved.lifecycle_snapshot.signature;
    let frozen_ssd = format!("{:?}", resolved.lifecycle_decorations[0].scene_snapshot());
    let source_signature = harness
        .renderer
        .lifecycle
        .resolved_visual_source_signature(payload_id)
        .expect("retained lifecycle source was materialized");
    let retained_texture = harness
        .renderer
        .lifecycle
        .resolved_visual_texture(payload_id)
        .expect("retained lifecycle texture exists")
        .clone();
    let retained_pixels_before = read_effect_texture_pixels(
        &mut harness,
        &retained_texture,
        retained_texture.key.width,
        retained_texture.key.height,
    );
    let canonical_surfaces = resolved.surfaces.as_ref().to_vec();
    let retained_lifecycle = resolved.lifecycle.clone();
    let retained_surfaces = resolved.lifecycle_surfaces.clone();
    let frozen_decorations = resolved.lifecycle_decorations.clone();
    drop(resolved);
    server.install_native_frame_test_scene_with_server_decorations(
        canonical_surfaces.clone(),
        &[
            (lower.surface_id, lower_window),
            (upper.surface_id, upper_window),
            (retained.surface_id, retained_window),
        ],
        None,
    );
    let mut resolved = crate::native_output::ResolvedNativeFrameScene::from_server_at(
        &server,
        AnimationTime::from_nanos(1),
    );
    resolved.surfaces = std::borrow::Cow::Owned(canonical_surfaces);
    resolved.lifecycle = retained_lifecycle;
    resolved.lifecycle_snapshot =
        oblivion_one::window_lifecycle_animation::LifecycleFrameSnapshot::from_sample(
            &resolved.lifecycle,
        );
    resolved.lifecycle_surfaces = retained_surfaces;
    resolved.lifecycle_decorations = frozen_decorations;
    set_test_lifecycle_marker_insertions(&server, &mut resolved);
    assert_eq!(
        resolved.lifecycle.samples[0].presentation_identity,
        presentation_identity
    );
    assert_eq!(resolved.lifecycle.samples[0].payload_id, payload_id);
    assert_eq!(
        resolved.lifecycle_snapshot.signature, lifecycle_snapshot_signature,
        "stack movement preserves the frozen lifecycle snapshot"
    );
    assert_eq!(
        resolved.lifecycle_surfaces[0].buffer_id(),
        retained_surface_snapshot_id
    );
    assert_eq!(
        format!("{:?}", resolved.lifecycle_decorations[0].scene_snapshot()),
        frozen_ssd,
        "stack movement keeps the frozen SSD snapshot"
    );

    let request = frame_renderer.egl_scene_draw_request(
        320,
        200,
        &resolved,
        &server,
        &input_state,
        crate::native_output::NativeCursorRenderMode::Hardware,
        Some(OutputDamage::Full),
    );
    let outcome = harness
        .renderer
        .draw_scene_to_target(&harness.egl, harness.display, target, request)
        .expect("reordered lifecycle frame draws");
    let moved_evidence = match outcome {
        EglFrameOutcome::Rendered {
            lifecycle_evidence, ..
        } => lifecycle_evidence,
        EglFrameOutcome::LifecycleFallback { fallbacks, .. } => {
            panic!("reordered lifecycle sample fell back: {fallbacks:?}")
        }
        EglFrameOutcome::Skipped { reason, .. } => {
            panic!("full reordered lifecycle frame was skipped: {reason:?}")
        }
    };
    assert!(moved_evidence.contains(presentation_identity, payload_id, retained.surface_id));
    let commands = &harness.renderer.scene_state.commands;
    let moved_marker_index = commands
        .iter()
        .position(|command| command.layer == EglDrawLayer::LifecycleOverlay(presentation_identity))
        .expect("reordered frame has one lifecycle marker");
    let moved_upper_index = commands
        .iter()
        .position(|command| command.layer == EglDrawLayer::Surface(632))
        .expect("upper canonical window remains present");
    assert!(moved_upper_index < moved_marker_index);
    harness.renderer.bind_active_output_framebuffer();
    let moved_pixels = read_effect_test_pixels(&harness.gl, 320, 200);
    let moved_overlap = effect_test_pixel(&moved_pixels, 320, overlap_x, 200 - 1 - overlap_y);
    assert!(
        moved_overlap[0] > moved_overlap[2].saturating_mul(2),
        "moving the retained owner above the upper window moves its marker above it: {moved_overlap:?}"
    );
    assert_eq!(
        harness
            .renderer
            .lifecycle
            .resolved_visual_source_signature(payload_id),
        Some(source_signature),
        "stack movement reuses the retained source"
    );
    assert_eq!(
        read_effect_texture_pixels(
            &mut harness,
            &retained_texture,
            retained_texture.key.width,
            retained_texture.key.height,
        ),
        retained_pixels_before,
        "stack movement does not recapture the retained source"
    );
}

#[test]
fn workspace_slide_blur_reads_back_translated_lower_workspace_pixels() {
    let mut harness = GlesEffectTestHarness::new(320, 200);
    harness.install_texture_backed_output();

    let mut buffer_ids = BufferIdAllocator::default();
    let mut lower = lifecycle_test_surface(710, 0, 0, 320, 200, 0xffee_1010, &mut buffer_ids);
    let lower_pixels = (0..320 * 200)
        .map(|pixel| {
            if pixel % 320 < 280 {
                0xffee_1010
            } else {
                0xff10_10ee
            }
        })
        .collect();
    lower.buffer = CommittedSurfaceBuffer::shm_snapshot(
        buffer_ids.allocate().expect("lower pixel buffer identity"),
        BufferSize::new(320, 200).expect("lower pixel buffer size"),
        lower_pixels,
    );
    let upper = lifecycle_test_surface(711, 0, 70, 120, 60, 0x4000_0000, &mut buffer_ids);
    let canonical_origins =
        oblivion_one::compositor::surface_origins(&[lower.clone(), upper.clone()]);
    let lower_window = oblivion_one::compositor::WindowId::from_raw(1).expect("lower window");
    let upper_window = oblivion_one::compositor::WindowId::from_raw(2).expect("upper window");
    let socket_name = format!("typhon-workspace-slide-backdrop-egl-{}", std::process::id());
    let mut server =
        oblivion_one::compositor::OwnCompositorServer::bind_cpu_composition(&socket_name)
            .expect("test compositor binds");
    assert!(server.set_output_size(320, 200));
    server.install_native_frame_test_scene(
        vec![lower.clone(), upper.clone()],
        &[
            (lower.surface_id, lower_window),
            (upper.surface_id, upper_window),
        ],
        None,
    );
    let target_workspace = oblivion_one::wm::WorkspaceId::new(2).expect("target workspace");
    assert!(server.set_test_window_workspace(lower_window, target_workspace));
    assert!(server.set_test_window_workspace(upper_window, target_workspace));
    let mut animation_configuration = oblivion_one::animation_control::AnimationConfiguration {
        enabled: true,
        ..oblivion_one::animation_control::AnimationConfiguration::default()
    };
    animation_configuration.overrides.insert(
        oblivion_one::animation_control::AnimationSlot::WorkspaceSwitch,
        oblivion_one::animation_control::AnimationEffect::WorkspaceSlide,
    );
    server
        .set_animation_configuration(animation_configuration)
        .expect("enable WorkspaceSlide");
    server.activate_workspace(target_workspace);
    assert!(server.workspace_presentation_active());
    std::thread::sleep(std::time::Duration::from_millis(200));

    let sampled_at = AnimationTime::monotonic_now().expect("monotonic frame time");
    let mut resolved =
        crate::native_output::ResolvedNativeFrameScene::from_server_at(&server, sampled_at);
    let lower_index = resolved
        .surfaces
        .iter()
        .position(|surface| surface.surface_id == lower.surface_id)
        .expect("translated lower workspace surface");
    let upper_index = resolved
        .surfaces
        .iter()
        .position(|surface| surface.surface_id == upper.surface_id)
        .expect("translated translucent window");
    let frame_origins = oblivion_one::compositor::surface_origins(&resolved.surfaces);
    let (lower_x, _lower_y) = frame_origins[lower_index];
    let (upper_x, upper_y) = frame_origins[upper_index];
    let lower_translation_x = lower_x - canonical_origins[0].0;
    let upper_translation_x = upper_x - canonical_origins[1].0;
    let upper_width = resolved.surfaces[upper_index].width;
    let upper_height = resolved.surfaces[upper_index].height;
    assert_ne!(lower_translation_x, 0);
    let participant = resolved
        .workspace_presentation_sample
        .as_ref()
        .expect("frame carries WorkspaceSlide sample")
        .participants
        .iter()
        .find(|participant| participant.workspace == target_workspace)
        .expect("target workspace participates in this frame");
    let expected_translation_x = participant.translation_x.round() as i32;
    assert_eq!(lower_translation_x, expected_translation_x);
    assert_eq!(upper_translation_x, expected_translation_x);
    let clip = participant.logical_cell_clip;
    let left = upper_x.max(clip.x.floor() as i32).max(0);
    let top = upper_y.max(clip.y.floor() as i32).max(0);
    let right = (upper_x + upper_width as i32)
        .min((clip.x + clip.width).ceil() as i32)
        .min(320);
    let bottom = (upper_y + upper_height as i32)
        .min((clip.y + clip.height).ceil() as i32)
        .min(200);
    assert!(
        right - left >= 16 && bottom - top >= 16,
        "visible blur window: ({left}, {top})-({right}, {bottom})"
    );

    let blur_bounds = EffectRect::new(
        i32::try_from(left).expect("visible blur x"),
        i32::try_from(top).expect("visible blur y"),
        u32::try_from(right - left).expect("visible blur width"),
        u32::try_from(bottom - top).expect("visible blur height"),
    )
    .expect("visible blur region");
    let (mut effects, registry) = moving_blur_scene_with_spec(blur_bounds, 24.0, 3, 1.0);
    let upper_anchor = oblivion_one::compositor::EffectAnchor::BeforeSurface(upper.surface_id);
    let backdrop = effects
        .instances
        .first_mut()
        .expect("backdrop blur effect instance");
    backdrop.anchor = upper_anchor;
    backdrop.scene_order = oblivion_one::compositor::EffectSceneOrder::for_anchor(upper_anchor);
    backdrop.backdrop_reuse = oblivion_one::compositor::EffectBackdropReuse::Exact;
    harness.renderer.set_effect_registry(registry);
    resolved.effects = effects;

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
        .expect("WorkspaceSlide backdrop frame renders through GLES");
    assert!(matches!(outcome, EglFrameOutcome::Rendered { .. }));

    harness.renderer.bind_active_output_framebuffer();
    let output_pixels = read_effect_test_pixels(&harness.gl, 320, 200);
    let probe_x = left + 16;
    let probe_y = top + (bottom - top) / 2;
    let blurred_pixel = effect_test_pixel(
        &output_pixels,
        320,
        probe_x as u32,
        200 - 1 - probe_y as u32,
    );
    assert!(
        blurred_pixel[0] > 60 && blurred_pixel[2] < 100,
        "translucent window at translated x={} must blur the translated red lower workspace pixel, got {blurred_pixel:?}",
        upper_x,
    );
}

fn render_lifecycle_overlap_pixel(
    effect: LifecycleEffectKind,
    retained_above: bool,
    surface_id_base: u32,
    name: &str,
    partial_repaint: bool,
) -> ([u8; 4], GlesSceneFrameStats) {
    let mut harness = if partial_repaint {
        GlesEffectTestHarness::new_with_partial_repaint(320, 200)
    } else {
        GlesEffectTestHarness::new(320, 200)
    };
    harness.install_texture_backed_output();

    let mut buffer_ids = BufferIdAllocator::default();
    let lower = lifecycle_test_surface(
        surface_id_base,
        0,
        0,
        320,
        200,
        0xff22_3322,
        &mut buffer_ids,
    );
    let retained = lifecycle_test_surface(
        surface_id_base + 1,
        60,
        40,
        180,
        110,
        0xffee_2211,
        &mut buffer_ids,
    );
    let upper = lifecycle_test_surface(
        surface_id_base + 2,
        100,
        0,
        100,
        70,
        0xff11_22ee,
        &mut buffer_ids,
    );
    let lower_window = oblivion_one::compositor::WindowId::from_raw(1).expect("lower window");
    let retained_window = oblivion_one::compositor::WindowId::from_raw(2).expect("retained");
    let upper_window = oblivion_one::compositor::WindowId::from_raw(3).expect("upper window");
    let stack = if retained_above {
        vec![
            (lower.surface_id, lower_window),
            (upper.surface_id, upper_window),
            (retained.surface_id, retained_window),
        ]
    } else {
        vec![
            (lower.surface_id, lower_window),
            (retained.surface_id, retained_window),
            (upper.surface_id, upper_window),
        ]
    };
    let socket_name = format!("typhon-lifecycle-{name}-egl-{}", std::process::id());
    let mut server =
        oblivion_one::compositor::OwnCompositorServer::bind_cpu_composition(&socket_name)
            .expect("test compositor binds");
    server.install_native_frame_test_scene_with_server_decorations(
        vec![lower.clone(), retained.clone(), upper.clone()],
        &stack,
        None,
    );
    let lifecycle_decorations = if partial_repaint {
        Vec::new()
    } else {
        server.native_decoration_render_instances(std::slice::from_ref(&retained))
    };
    server.install_native_frame_test_scene_with_server_decorations(
        vec![lower.clone(), upper.clone()],
        &stack,
        None,
    );

    let mut resolved = crate::native_output::ResolvedNativeFrameScene::from_server_at(
        &server,
        AnimationTime::from_nanos(0),
    );
    resolved.surfaces = std::borrow::Cow::Owned(vec![lower.clone(), upper.clone()]);
    resolved.lifecycle_surfaces = vec![retained.clone()];
    resolved.lifecycle = lifecycle_effect_sample(
        0.0,
        LifecycleDirection::Minimize,
        retained.surface_id,
        1,
        ResolvedEffectScene::default(),
    );
    {
        let sample = &mut resolved.lifecycle.samples[0];
        sample.window_id = retained_window;
        sample.visual_source.window_id = retained_window;
        sample.visual_source.kind = LifecycleVisualSourceKind::NoOwnedEffects;
        sample.effect = effect;
        if !partial_repaint {
            sample.visual_group = LifecycleVisualGroup::from_bounds(
                PresentationRect::new(60.0, 40.0, 180.0, 110.0).expect("canonical client rect"),
                PresentationRect::new(40.0, 20.0, 220.0, 160.0).expect("canonical visual rect"),
                PresentationRect::new(60.0, 40.0, 180.0, 110.0)
                    .expect("presented client rect"),
                PresentationRect::new(220.0, 130.0, 60.0, 40.0).expect("anchor rect"),
                320,
                200,
            )
            .expect("decorated lifecycle visual group");
        }
    }
    resolved.lifecycle_decorations = lifecycle_decorations;
    set_test_lifecycle_marker_insertions(&server, &mut resolved);
    resolved.lifecycle_snapshot =
        oblivion_one::window_lifecycle_animation::LifecycleFrameSnapshot::from_sample(
            &resolved.lifecycle,
        );
    let identity = resolved.lifecycle.samples[0].presentation_identity;
    let payload_id = resolved.lifecycle.samples[0].payload_id;

    let input_state = crate::native_output::NativeInputState::new(320, 200);
    let mut frame_renderer = crate::native_output::NativeFrameRenderer::default();
    let output_framebuffer = harness
        .test_output_framebuffer
        .expect("output has an FBO-backed render target");
    let output_texture = harness.test_output_texture;
    let target_for_age = |buffer_age| EglOutputRenderTarget {
        framebuffer: output_framebuffer,
        sampleable_texture: output_texture,
        width: 320,
        height: 200,
        buffer_age,
        framebuffer_origin: OutputFramebufferOrigin::BottomLeft,
    };
    if partial_repaint {
        let first_request = frame_renderer.egl_scene_draw_request(
            320,
            200,
            &resolved,
            &server,
            &input_state,
            crate::native_output::NativeCursorRenderMode::Hardware,
            Some(OutputDamage::Full),
        );
        let first_outcome = harness
            .renderer
            .draw_scene_to_target(
                &harness.egl,
                harness.display,
                target_for_age(BufferAge::Unsupported),
                first_request,
            )
            .expect("initial full repaint frame draws");
        let EglFrameOutcome::Rendered { commit, .. } = first_outcome else {
            panic!("initial lifecycle frame must render before partial repair");
        };
        harness
            .renderer
            .commit_presented(commit, OutputDamage::Full);
    }
    let damage = if partial_repaint {
        OutputDamage::rects(
            320,
            200,
            [
                OutputRect::new(230, 130, 30, 20),
                OutputRect::new(40, 40, 20, 20),
            ],
        )
    } else {
        OutputDamage::Full
    };
    let request = frame_renderer.egl_scene_draw_request(
        320,
        200,
        &resolved,
        &server,
        &input_state,
        crate::native_output::NativeCursorRenderMode::Hardware,
        Some(damage),
    );
    let target = target_for_age(if partial_repaint {
        BufferAge::Value(1)
    } else {
        BufferAge::Unsupported
    });
    let outcome = harness
        .renderer
        .draw_scene_to_target(&harness.egl, harness.display, target, request)
        .expect("canonical overlap frame draws");
    let (evidence, stats, repair_rect_count, repaint_mode, repaint_plan_debug) = match outcome {
        EglFrameOutcome::Rendered {
            commit,
            stats,
            lifecycle_evidence,
            ..
        } => (
            lifecycle_evidence,
            stats,
            commit.repaint_plan.repair_damage.rect_count(),
            commit.repaint_plan.mode,
            format!("{:?}", commit.repaint_plan),
        ),
        EglFrameOutcome::LifecycleFallback { fallbacks, .. } => {
            panic!("valid overlap lifecycle fell back: {fallbacks:?}")
        }
        EglFrameOutcome::Skipped { reason, .. } => {
            panic!("full overlap lifecycle frame was skipped: {reason:?}")
        }
    };
    assert!(evidence.contains(identity, payload_id, retained.surface_id));
    if partial_repaint {
        assert!(repair_rect_count > 0);
        assert_eq!(
            repaint_mode,
            crate::egl_renderer::damage::RepaintMode::Partial,
            "partial damage request produced a non-partial plan: {repaint_plan_debug}"
        );
        if matches!(
            std::env::var("TYPHON_SCENE_EXECUTION_MODE").as_deref(),
            Ok("legacy")
        ) {
            assert_eq!(stats.base_region_legacy_mode_frames, 1);
        }
    }
    let commands = &harness.renderer.scene_state.commands;
    let lower_index = commands
        .iter()
        .position(|command| command.layer == EglDrawLayer::Surface(lower.surface_id))
        .expect("lower canonical command");
    let marker_index = commands
        .iter()
        .position(|command| command.layer == EglDrawLayer::LifecycleOverlay(identity))
        .expect("canonical lifecycle marker");
    let marker_command = commands
        .iter()
        .find(|command| command.layer == EglDrawLayer::LifecycleOverlay(identity))
        .expect("canonical lifecycle marker");
    let upper_index = commands
        .iter()
        .position(|command| command.layer == EglDrawLayer::Surface(upper.surface_id))
        .expect("upper canonical command");
    let upper_command = commands
        .iter()
        .find(|command| command.layer == EglDrawLayer::Surface(upper.surface_id))
        .expect("upper canonical command");
    assert!(lower_index < marker_index);
    assert_eq!(
        marker_index < upper_index,
        !retained_above,
        "lifecycle marker must occupy the selected compositor stack position"
    );
    let overlap_left = marker_command.bounds.x().max(upper_command.bounds.x()).max(0.0);
    let overlap_top = marker_command.bounds.y().max(upper_command.bounds.y()).max(0.0);
    let overlap_right = (marker_command.bounds.x() + marker_command.bounds.width())
        .min(upper_command.bounds.x() + upper_command.bounds.width())
        .min(320.0);
    let overlap_bottom = (marker_command.bounds.y() + marker_command.bounds.height())
        .min(upper_command.bounds.y() + upper_command.bounds.height())
        .min(200.0);
    assert!(overlap_left < overlap_right && overlap_top < overlap_bottom);
    let overlap_x = ((overlap_left + overlap_right) * 0.5).floor() as u32;
    let overlap_y = ((overlap_top + overlap_bottom) * 0.5).floor() as u32;
    harness.renderer.bind_active_output_framebuffer();
    let pixels = read_effect_test_pixels(&harness.gl, 320, 200);
    let overlap = effect_test_pixel(&pixels, 320, overlap_x, 200 - 1 - overlap_y);
    if partial_repaint {
        let second_repair_overlap = effect_test_pixel(&pixels, 320, overlap_x, 200 - 1 - overlap_y);
        assert!(
            second_repair_overlap[2] > second_repair_overlap[0].saturating_mul(2),
            "the second repair must replay the upper canonical window after the lifecycle marker: {second_repair_overlap:?}"
        );
    }
    (overlap, stats)
}

#[test]
fn egl_lifecycle_squash_below_canonical_opaque_window_respects_painter_order() {
    let (overlap, _) = render_lifecycle_overlap_pixel(
        LifecycleEffectKind::Squash,
        false,
        642,
        "squash-below-canonical",
        false,
    );
    assert!(
        overlap[2] > overlap[0].saturating_mul(2),
        "the upper canonical window must win above the retained Squash: {overlap:?}"
    );
}

#[test]
fn egl_lifecycle_above_canonical_opaque_window_respects_painter_order() {
    let (overlap, _) = render_lifecycle_overlap_pixel(
        LifecycleEffectKind::Lamp,
        true,
        645,
        "above-canonical",
        false,
    );
    assert!(
        overlap[0] > overlap[2].saturating_mul(2),
        "the retained Lamp above the canonical window must win: {overlap:?}"
    );
}

#[test]
fn egl_lifecycle_marker_uses_top_level_region_replay() {
    let (overlap, stats) = render_lifecycle_overlap_pixel(
        LifecycleEffectKind::Lamp,
        false,
        648,
        "top-level-region-replay",
        false,
    );
    assert!(
        overlap[2] > overlap[0].saturating_mul(2),
        "the top-level region replay must preserve canonical/lifecycle painter order: {overlap:?}"
    );
    if matches!(
        std::env::var("TYPHON_SCENE_EXECUTION_MODE").as_deref(),
        Ok("legacy")
    ) {
        return;
    }
    assert_eq!(stats.base_region_plan_builds, 1);
    assert_eq!(stats.base_region_legacy_mode_frames, 0);
    assert_eq!(stats.base_region_legacy_fallbacks, 0);
    assert_eq!(stats.scissor_passes, 1);
}

#[test]
fn egl_lifecycle_scissored_replay_preserves_order_for_multiple_repairs() {
    let (overlap, _) = render_lifecycle_overlap_pixel(
        LifecycleEffectKind::Lamp,
        false,
        651,
        "multiple-scissored-repairs",
        true,
    );
    assert!(
        overlap[2] > overlap[0].saturating_mul(2),
        "the first repair must leave the upper canonical window above lifecycle: {overlap:?}"
    );
}

#[test]
fn egl_lifecycle_marker_replays_without_canonical_vertices_in_full_repaint() {
    let mut harness = GlesEffectTestHarness::new(320, 200);
    harness.install_texture_backed_output();

    let mut buffer_ids = BufferIdAllocator::default();
    let retained = lifecycle_test_surface(633, 60, 40, 180, 110, 0xffee_2211, &mut buffer_ids);
    let window_id = oblivion_one::compositor::WindowId::from_raw(1).expect("window id");
    let socket_name = format!("typhon-lifecycle-marker-only-egl-{}", std::process::id());
    let mut server =
        oblivion_one::compositor::OwnCompositorServer::bind_cpu_composition(&socket_name)
            .expect("test compositor binds");
    server.install_native_frame_test_scene_with_server_decorations(
        vec![retained.clone()],
        &[(retained.surface_id, window_id)],
        None,
    );
    let lifecycle_decorations =
        server.native_decoration_render_instances(std::slice::from_ref(&retained));
    server.install_native_frame_test_scene_with_server_decorations(
        Vec::new(),
        &[(retained.surface_id, window_id)],
        None,
    );

    let mut resolved = crate::native_output::ResolvedNativeFrameScene::from_server_at(
        &server,
        AnimationTime::from_nanos(0),
    );
    resolved.surfaces = std::borrow::Cow::Owned(Vec::new());
    resolved.lifecycle_surfaces = vec![retained.clone()];
    resolved.lifecycle = lifecycle_effect_sample(
        0.0,
        LifecycleDirection::Minimize,
        retained.surface_id,
        1,
        ResolvedEffectScene::default(),
    );
    resolved.lifecycle.samples[0].visual_group = LifecycleVisualGroup::from_bounds(
        PresentationRect::new(60.0, 40.0, 180.0, 110.0).expect("canonical client rect"),
        PresentationRect::new(40.0, 20.0, 220.0, 160.0).expect("canonical visual rect"),
        PresentationRect::new(60.0, 40.0, 180.0, 110.0).expect("presented client rect"),
        PresentationRect::new(220.0, 130.0, 60.0, 40.0).expect("anchor rect"),
        320,
        200,
    )
    .expect("decorated lifecycle visual group");
    resolved.lifecycle_decorations = lifecycle_decorations;
    let sample = &mut resolved.lifecycle.samples[0];
    sample.window_id = window_id;
    sample.visual_source.window_id = window_id;
    sample.visual_source.kind = LifecycleVisualSourceKind::NoOwnedEffects;
    let (scene_band, layer_rank, stack_position, stable_order) =
        server.native_scene_root_stack_key(retained.surface_id, 0);
    sample.painter_order = LifecyclePainterOrder {
        scene_band,
        layer_rank,
        stack_position,
        stable_order: stable_order as u64,
    };
    let identity = sample.presentation_identity;
    resolved.lifecycle_marker_insertions = vec![(identity, 0)];
    resolved.lifecycle_snapshot =
        oblivion_one::window_lifecycle_animation::LifecycleFrameSnapshot::from_sample(
            &resolved.lifecycle,
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
        .expect("marker-only lifecycle frame draws");
    let evidence = match outcome {
        EglFrameOutcome::Rendered {
            lifecycle_evidence, ..
        } => lifecycle_evidence,
        EglFrameOutcome::LifecycleFallback { fallbacks, .. } => {
            panic!("valid marker-only lifecycle fell back: {fallbacks:?}")
        }
        EglFrameOutcome::Skipped { reason, .. } => {
            panic!("full marker-only lifecycle repaint was skipped: {reason:?}")
        }
    };
    assert!(evidence.contains(identity, resolved.lifecycle.samples[0].payload_id, 633));
    assert!(harness.renderer.scene_state.commands.iter().all(|command| {
        !matches!(command.layer, EglDrawLayer::Surface(_))
    }));
    assert!(
        harness
            .renderer
            .scene_state
            .commands
            .iter()
            .any(|command| command.layer == EglDrawLayer::LifecycleOverlay(identity))
    );

    harness.renderer.bind_active_output_framebuffer();
    let pixels = read_effect_test_pixels(&harness.gl, 320, 200);
    let retained_red_pixel = pixels
        .chunks_exact(4)
        .find(|pixel| pixel[0] > pixel[1].saturating_mul(2));
    assert!(
        retained_red_pixel.is_some(),
        "the lifecycle marker must draw without canonical scene vertices"
    );
}

#[test]
fn egl_lifecycle_pixels_participate_in_output_postprocess() {
    let mut harness = GlesEffectTestHarness::new(320, 200);
    harness.install_texture_backed_output();

    let program_id = EffectProgramId::new(9_003).expect("postprocess test program id");
    let source = oblivion_one::effects::EffectNodeId::new(1).expect("source node id");
    let transformed = oblivion_one::effects::EffectNodeId::new(2).expect("matrix node id");
    let program = oblivion_one::effects::EffectProgram {
        id: program_id,
        nodes: vec![
            oblivion_one::effects::EffectNode::source(
                source,
                oblivion_one::effects::EffectSource::TargetContent,
            ),
            oblivion_one::effects::EffectNode::color_matrix(
                transformed,
                source,
                oblivion_one::effects::ColorMatrixSpec {
                    matrix: [
                        0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0,
                        1.0,
                    ],
                    bias: [0.0; 4],
                },
            )
            .expect("channel-swap matrix validates"),
        ],
        output: transformed,
        working_space: EffectWorkingSpace::OutputEncodedSrgb,
        alpha_mode: EffectAlphaMode::Preserve,
        outsets: oblivion_one::effects::EffectOutsets::ZERO,
        frame_demand: EffectFrameDemand::OnDamage,
        failure_policy: EffectFailurePolicy::Passthrough,
    };
    let mut registry = oblivion_one::effects::EffectRegistry::empty();
    registry
        .insert(oblivion_one::effects::validate_effect_program(program).unwrap())
        .expect("postprocess test program registers");
    harness.renderer.set_effect_registry(registry);

    let mut buffer_ids = BufferIdAllocator::default();
    let background = lifecycle_test_surface(634, 0, 0, 320, 200, 0xff00_10ee, &mut buffer_ids);
    let retained = lifecycle_test_surface(635, 60, 40, 180, 110, 0xffee_0010, &mut buffer_ids);
    let background_window = oblivion_one::compositor::WindowId::from_raw(1).expect("background");
    let retained_window = oblivion_one::compositor::WindowId::from_raw(2).expect("retained");
    let socket_name = format!(
        "typhon-lifecycle-output-postprocess-egl-{}",
        std::process::id()
    );
    let mut server =
        oblivion_one::compositor::OwnCompositorServer::bind_cpu_composition(&socket_name)
            .expect("test compositor binds");
    let windows = [
        (background.surface_id, background_window),
        (retained.surface_id, retained_window),
    ];
    server.install_native_frame_test_scene_with_server_decorations(
        vec![background.clone(), retained.clone()],
        &windows,
        None,
    );
    server.install_native_frame_test_scene_with_server_decorations(
        vec![background.clone()],
        &windows,
        None,
    );

    let mut resolved = crate::native_output::ResolvedNativeFrameScene::from_server_at(
        &server,
        AnimationTime::from_nanos(0),
    );
    resolved.surfaces = std::borrow::Cow::Owned(vec![background]);
    resolved.lifecycle_surfaces = vec![retained.clone()];
    resolved.lifecycle = lifecycle_effect_sample(
        0.0,
        LifecycleDirection::Minimize,
        retained.surface_id,
        1,
        ResolvedEffectScene::default(),
    );
    let sample = &mut resolved.lifecycle.samples[0];
    sample.window_id = retained_window;
    sample.visual_source.window_id = retained_window;
    sample.visual_source.kind = LifecycleVisualSourceKind::NoOwnedEffects;
    let (scene_band, layer_rank, stack_position, stable_order) =
        server.native_scene_root_stack_key(retained.surface_id, 1);
    sample.painter_order = LifecyclePainterOrder {
        scene_band,
        layer_rank,
        stack_position,
        stable_order: stable_order as u64,
    };
    let identity = sample.presentation_identity;
    let canonical_groups = oblivion_one::compositor::WindowVisualGroup::stack_order_with_popups(
        resolved.surfaces.as_ref(),
        &resolved.decorations,
        resolved.popup_surface_ids.as_ref(),
    );
    let lifecycle_order = oblivion_one::compositor::window_exit_stack_order_key(
        scene_band,
        layer_rank,
        stack_position,
        true,
        usize::try_from(stable_order).unwrap_or(usize::MAX),
    );
    let insertion_group = canonical_groups
        .iter()
        .enumerate()
        .position(|(group_index, group)| {
            let (band, rank, position, _) =
                server.native_scene_root_stack_key(group.root_surface_id(), group_index);
            oblivion_one::compositor::window_exit_stack_order_key(
                band,
                rank,
                position,
                false,
                group_index,
            ) > lifecycle_order
        })
        .unwrap_or(canonical_groups.len());
    resolved.lifecycle_marker_insertions = vec![(identity, insertion_group)];
    resolved.lifecycle_snapshot =
        oblivion_one::window_lifecycle_animation::LifecycleFrameSnapshot::from_sample(
            &resolved.lifecycle,
        );

    let bounds = EffectRect::new(0, 0, 320, 200).expect("postprocess output bounds");
    let anchor = oblivion_one::compositor::EffectAnchor::OutputPostProcess;
    resolved.effects = ResolvedEffectScene::new(
        1,
        vec![oblivion_one::compositor::ResolvedEffectInstance {
            id: oblivion_one::effects::EffectInstanceId::new(1).expect("effect instance id"),
            program: program_id,
            anchor,
            target_bounds: bounds,
            region: EffectRegion::from_rect(bounds),
            parameter_block: EffectParameterBlock::default(),
            coverage: None,
            signature: 1,
            frame_demand: EffectFrameDemand::OnDamage,
            visual_group: None,
            anchor_scope: oblivion_one::compositor::EffectAnchorScope::VisualGroup,
            backdrop_reuse: oblivion_one::compositor::EffectBackdropReuse::Exact,
            scene_order: oblivion_one::compositor::EffectSceneOrder::for_anchor(anchor),
        }],
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
        .expect("lifecycle postprocess frame draws");
    assert!(matches!(outcome, EglFrameOutcome::Rendered { .. }));
    harness.renderer.bind_active_output_framebuffer();
    let pixels = read_effect_test_pixels(&harness.gl, 320, 200);
    let lifecycle_pixel = effect_test_pixel(&pixels, 320, 100, 200 - 1 - 90);
    assert!(
        lifecycle_pixel[2] > lifecycle_pixel[0].saturating_mul(2)
            && lifecycle_pixel[2] > lifecycle_pixel[1].saturating_mul(2),
        "the output-postprocess channel swap must turn the retained red lifecycle pixel blue: {lifecycle_pixel:?}"
    );
}

#[test]
fn egl_canonical_backdrop_blur_captures_lifecycle_pixels_below_its_anchor() {
    let mut harness = GlesEffectTestHarness::new(320, 200);
    harness.install_texture_backed_output();

    let mut buffer_ids = BufferIdAllocator::default();
    let lower = lifecycle_test_surface(636, 0, 0, 320, 200, 0xff10_10ee, &mut buffer_ids);
    let retained = lifecycle_test_surface_with_row_colors(
        637,
        60,
        40,
        180,
        110,
        [0xffee_2211, 0xff12_ea14, 0xff12_22ee],
        &mut buffer_ids,
    );
    let upper = lifecycle_test_surface(638, 100, 70, 100, 70, 0x4000_0000, &mut buffer_ids);
    let lower_window = oblivion_one::compositor::WindowId::from_raw(1).expect("lower window");
    let retained_window = oblivion_one::compositor::WindowId::from_raw(2).expect("retained window");
    let upper_window = oblivion_one::compositor::WindowId::from_raw(3).expect("upper window");
    let socket_name = format!(
        "typhon-lifecycle-backdrop-capture-egl-{}",
        std::process::id()
    );
    let mut server =
        oblivion_one::compositor::OwnCompositorServer::bind_cpu_composition(&socket_name)
            .expect("test compositor binds");
    let windows = [
        (lower.surface_id, lower_window),
        (retained.surface_id, retained_window),
        (upper.surface_id, upper_window),
    ];
    server.install_native_frame_test_scene(
        vec![lower.clone(), retained.clone(), upper.clone()],
        &windows,
        None,
    );
    server.install_native_frame_test_scene(vec![lower.clone(), upper.clone()], &windows, None);

    let blur_bounds = EffectRect::new(100, 70, 100, 70).expect("upper blur bounds");
    let (mut backdrop_scene, registry) = moving_blur_scene_with_spec(blur_bounds, 24.0, 3, 1.0);
    let upper_anchor = oblivion_one::compositor::EffectAnchor::BeforeSurface(upper.surface_id);
    let backdrop = backdrop_scene
        .instances
        .first_mut()
        .expect("canonical backdrop blur instance");
    backdrop.anchor = upper_anchor;
    backdrop.scene_order = oblivion_one::compositor::EffectSceneOrder::for_anchor(upper_anchor);
    backdrop.backdrop_reuse = oblivion_one::compositor::EffectBackdropReuse::Exact;
    harness.renderer.set_effect_registry(registry);

    let mut resolved = crate::native_output::ResolvedNativeFrameScene::from_server_at(
        &server,
        AnimationTime::from_nanos(0),
    );
    resolved.surfaces = std::borrow::Cow::Owned(vec![lower, upper]);
    resolved.effects = backdrop_scene;
    resolved.lifecycle_surfaces = vec![retained.clone()];
    resolved.lifecycle = lifecycle_effect_sample(
        0.0,
        LifecycleDirection::Minimize,
        retained.surface_id,
        1,
        ResolvedEffectScene::default(),
    );
    let sample = &mut resolved.lifecycle.samples[0];
    sample.window_id = retained_window;
    sample.visual_source.window_id = retained_window;
    sample.visual_source.kind = LifecycleVisualSourceKind::NoOwnedEffects;
    let (scene_band, layer_rank, stack_position, stable_order) =
        server.native_scene_root_stack_key(retained.surface_id, 1);
    sample.painter_order = LifecyclePainterOrder {
        scene_band,
        layer_rank,
        stack_position,
        stable_order: stable_order as u64,
    };
    let identity = sample.presentation_identity;
    let canonical_groups = oblivion_one::compositor::WindowVisualGroup::stack_order_with_popups(
        resolved.surfaces.as_ref(),
        &resolved.decorations,
        resolved.popup_surface_ids.as_ref(),
    );
    let lifecycle_order = oblivion_one::compositor::window_exit_stack_order_key(
        scene_band,
        layer_rank,
        stack_position,
        true,
        usize::try_from(stable_order).unwrap_or(usize::MAX),
    );
    let insertion_group = canonical_groups
        .iter()
        .enumerate()
        .position(|(group_index, group)| {
            let (band, rank, position, _) =
                server.native_scene_root_stack_key(group.root_surface_id(), group_index);
            oblivion_one::compositor::window_exit_stack_order_key(
                band,
                rank,
                position,
                false,
                group_index,
            ) > lifecycle_order
        })
        .unwrap_or(canonical_groups.len());
    resolved.lifecycle_marker_insertions = vec![(identity, insertion_group)];
    resolved.lifecycle_snapshot =
        oblivion_one::window_lifecycle_animation::LifecycleFrameSnapshot::from_sample(
            &resolved.lifecycle,
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
        .expect("canonical backdrop blur frame draws");
    assert!(matches!(outcome, EglFrameOutcome::Rendered { .. }));
    let commands = &harness.renderer.scene_state.commands;
    let lower_index = commands
        .iter()
        .position(|command| command.layer == EglDrawLayer::Surface(636))
        .expect("lower canonical surface command");
    let marker_index = commands
        .iter()
        .position(|command| command.layer == EglDrawLayer::LifecycleOverlay(identity))
        .expect("canonical lifecycle marker");
    let upper_index = commands
        .iter()
        .position(|command| command.layer == EglDrawLayer::Surface(638))
        .expect("upper blur owner command");
    assert!(lower_index < marker_index && marker_index < upper_index);

    let payload_id = resolved.lifecycle.samples[0].payload_id;
    let source_texture = harness
        .renderer
        .lifecycle
        .resolved_visual_texture(payload_id)
        .expect("lifecycle source visual is captured")
        .clone();
    let source_pixels = read_effect_texture_pixels(
        &mut harness,
        &source_texture,
        source_texture.key.width,
        source_texture.key.height,
    );
    let source_top = effect_test_pixel(
        &source_pixels,
        source_texture.key.width,
        90,
        source_texture.key.height - 1 - 75,
    );
    let source_band_probes = [10, 35, 55, 75, 95].map(|row| {
        effect_test_pixel(
            &source_pixels,
            source_texture.key.width,
            90,
            source_texture.key.height - 1 - row,
        )
    });
    assert!(
        source_top[0] > source_top[1].saturating_mul(2),
        "source red-edge probe at x=90,row=75 must be red; dimensions={}x{}, samples={source_band_probes:?}",
        source_texture.key.width,
        source_texture.key.height,
    );

    harness.renderer.bind_active_output_framebuffer();
    let output_pixels = read_effect_test_pixels(&harness.gl, 320, 200);
    let blurred_lifecycle_edge = effect_test_pixel(&output_pixels, 320, 150, 200 - 1 - 75);
    assert!(
        blurred_lifecycle_edge[0] > 36 && blurred_lifecycle_edge[2] < 218,
        "the blue backdrop blur must mix the red lifecycle edge below its anchor: source={source_top:?}, output={blurred_lifecycle_edge:?}"
    );
}

#[test]
fn lifecycle_owned_effect_capture_does_not_replay_another_main_frame_marker() {
    let mut harness = GlesEffectTestHarness::new(320, 200);
    harness.install_texture_backed_output();

    let mut buffer_ids = BufferIdAllocator::default();
    let background = lifecycle_test_surface(639, 0, 0, 320, 200, 0xff00_10ee, &mut buffer_ids);
    let retained = lifecycle_test_surface(640, 60, 40, 180, 110, 0xffee_1010, &mut buffer_ids);
    let other = lifecycle_test_surface(641, 60, 40, 180, 110, 0xff10_ee10, &mut buffer_ids);
    let first_window = oblivion_one::compositor::WindowId::from_raw(1).expect("first window");
    let other_window = oblivion_one::compositor::WindowId::from_raw(2).expect("other window");
    let background_window = oblivion_one::compositor::WindowId::from_raw(3).expect("background");
    let socket_name = format!(
        "typhon-lifecycle-no-recursive-marker-egl-{}",
        std::process::id()
    );
    let mut server =
        oblivion_one::compositor::OwnCompositorServer::bind_cpu_composition(&socket_name)
            .expect("test compositor binds");
    let windows = [
        (background.surface_id, background_window),
        (retained.surface_id, first_window),
        (other.surface_id, other_window),
    ];
    server.install_native_frame_test_scene(
        vec![background.clone(), retained.clone(), other.clone()],
        &windows,
        None,
    );
    server.install_native_frame_test_scene(vec![background.clone()], &windows, None);

    let mut resolved = crate::native_output::ResolvedNativeFrameScene::from_server_at(
        &server,
        AnimationTime::from_nanos(0),
    );
    resolved.surfaces = std::borrow::Cow::Owned(vec![background]);
    resolved.lifecycle_surfaces = vec![retained.clone(), other.clone()];
    resolved.lifecycle = lifecycle_effect_sample(
        0.0,
        LifecycleDirection::Minimize,
        retained.surface_id,
        1,
        lifecycle_blur_effect_scene(
            retained.surface_id,
            oblivion_one::effects::builtin_background_blur_program_id(),
        ),
    );
    let first_identity = resolved.lifecycle.samples[0].presentation_identity;
    let first_payload = resolved.lifecycle.samples[0].payload_id;

    let other_identity = test_lifecycle_identity_sequence(other_window, 2)[1];
    let other_payload =
        oblivion_one::compositor::PresentationRetainedVisualPayloadId::from_origin_identity(
            other_identity,
        );
    let mut other_sample = lifecycle_effect_sample(
        0.0,
        LifecycleDirection::Minimize,
        other.surface_id,
        2,
        ResolvedEffectScene::default(),
    )
    .samples
    .remove(0);
    other_sample.window_id = other_window;
    other_sample.presentation_identity = other_identity;
    other_sample.payload_id = other_payload;
    other_sample.visual_source.window_id = other_window;
    other_sample.visual_source.presentation_identity = other_identity;
    other_sample.visual_source.payload_id = other_payload;
    other_sample.visual_source.resource_payload_id = other_payload;
    other_sample.visual_source.kind = LifecycleVisualSourceKind::ResolvedOwnedEffects;
    other_sample.effect = LifecycleEffectKind::Squash;
    resolved.lifecycle.samples.push(other_sample);
    set_test_lifecycle_marker_insertions(&server, &mut resolved);
    resolved.lifecycle_snapshot =
        oblivion_one::window_lifecycle_animation::LifecycleFrameSnapshot::from_sample(
            &resolved.lifecycle,
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
        .expect("nested lifecycle source capture draws");
    let evidence = match outcome {
        EglFrameOutcome::Rendered {
            lifecycle_evidence, ..
        } => lifecycle_evidence,
        EglFrameOutcome::LifecycleFallback { fallbacks, .. } => {
            panic!("valid nested lifecycle capture fell back: {fallbacks:?}")
        }
        EglFrameOutcome::Skipped { reason, .. } => {
            panic!("full nested lifecycle frame was skipped: {reason:?}")
        }
    };
    assert!(evidence.contains(first_identity, first_payload, retained.surface_id));
    assert!(evidence.contains(other_identity, other_payload, other.surface_id));

    let main_commands = &harness.renderer.scene_state.commands;
    assert!(
        main_commands
            .iter()
            .any(|command| { command.layer == EglDrawLayer::LifecycleOverlay(other_identity) })
    );
    let source_commands = harness
        .renderer
        .lifecycle
        .source_commands_for_payload(first_payload)
        .expect("retained effect source commands are prepared");
    assert!(
        source_commands
            .iter()
            .any(|command| command.layer == EglDrawLayer::Surface(retained.surface_id))
    );
    assert!(
        !source_commands
            .iter()
            .any(|command| matches!(command.layer, EglDrawLayer::LifecycleOverlay(_)))
    );

    let source_texture = harness
        .renderer
        .lifecycle
        .resolved_visual_texture(first_payload)
        .expect("retained effect source is resolved")
        .clone();
    let source_pixels = read_effect_texture_pixels(
        &mut harness,
        &source_texture,
        source_texture.key.width,
        source_texture.key.height,
    );
    let captured = effect_test_pixel(
        &source_pixels,
        source_texture.key.width,
        90,
        source_texture.key.height - 1 - 55,
    );
    assert!(
        captured[1] < captured[0].max(captured[2]),
        "the other active green lifecycle marker must not enter the retained source capture: {captured:?}"
    );
}

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
    set_test_lifecycle_marker_insertions(&server, &mut resolved);
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
        resource_payload_id:
            oblivion_one::compositor::PresentationRetainedVisualPayloadId::from_origin_identity(
                test_lifecycle_identity(
                    oblivion_one::compositor::WindowId::from_raw(1)
                        .expect("valid lifecycle window ID"),
                    1,
                ),
            ),
        kind: LifecycleVisualSourceKind::ResolvedOwnedEffects,
        effect_scene: std::sync::Arc::new(oblivion_one::compositor::ResolvedEffectScene::default()),
        render_layout: None,
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
    let windows = [
        (
            background.surface_id,
            oblivion_one::compositor::WindowId::from_raw(2).expect("background window"),
        ),
        (retained_window.surface_id, window_id),
    ];
    server.install_native_frame_test_scene_with_server_decorations(
        vec![background.clone(), retained_window.clone()],
        &windows,
        None,
    );
    server.install_native_frame_test_scene_with_server_decorations(
        vec![background.clone()],
        &windows,
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
    set_test_lifecycle_marker_insertions(&server, &mut resolved);
    resolved.lifecycle_snapshot =
        oblivion_one::window_lifecycle_animation::LifecycleFrameSnapshot::from_sample(
            &resolved.lifecycle,
        );
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
    let mut failed_lifecycle =
        lifecycle_effect_sample(0.4, LifecycleDirection::Minimize, 604, 2, failed_scene);
    // Use a fresh identity from the same engine so this is a distinct retained
    // payload from the successful capture above.
    let mut identity_engine = PresentationEngine::enabled();
    let scene_node_id = SceneNodeId::from_raw(1).expect("test scene node");
    let _first_identity = identity_engine
        .begin_retained_visual(
            scene_node_id,
            PresentationRetainedVisualKind::WindowLifecycle,
            AnimationTime::from_nanos(1),
        )
        .expect("first retained identity");
    let failed_identity = identity_engine
        .begin_retained_visual(
            scene_node_id,
            PresentationRetainedVisualKind::WindowLifecycle,
            AnimationTime::from_nanos(2),
        )
        .expect("second retained identity");
    let failed_payload =
        oblivion_one::compositor::PresentationRetainedVisualPayloadId::from_origin_identity(
            failed_identity,
        );
    failed_lifecycle.samples[0].presentation_identity = failed_identity;
    failed_lifecycle.samples[0].payload_id = failed_payload;
    failed_lifecycle.samples[0]
        .visual_source
        .presentation_identity = failed_identity;
    failed_lifecycle.samples[0].visual_source.payload_id = failed_payload;
    failed_lifecycle.samples[0].visual_source.resource_payload_id = failed_payload;
    resolved.lifecycle = failed_lifecycle;
    set_test_lifecycle_marker_insertions(&server, &mut resolved);
    resolved.lifecycle_snapshot =
        oblivion_one::window_lifecycle_animation::LifecycleFrameSnapshot::from_sample(
            &resolved.lifecycle,
        );
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
fn egl_lifecycle_lamp_freezes_ordinary_surface_content_for_the_retained_payload() {
    let mut harness = GlesEffectTestHarness::new(320, 200);
    harness.install_texture_backed_output();

    let mut buffer_ids = BufferIdAllocator::default();
    let background = lifecycle_test_surface(617, 0, 0, 320, 200, 0xff24_4567, &mut buffer_ids);
    let retained = lifecycle_test_surface(618, 60, 40, 180, 110, 0xffee_8844, &mut buffer_ids);
    let window_id = oblivion_one::compositor::WindowId::from_raw(1).expect("window id");
    let socket_name = format!(
        "typhon-lifecycle-ordinary-source-egl-{}",
        std::process::id()
    );
    let mut server =
        oblivion_one::compositor::OwnCompositorServer::bind_cpu_composition(&socket_name)
            .expect("test compositor binds");
    server.install_native_frame_test_scene_with_server_decorations(
        vec![background.clone()],
        &[
            (
                background.surface_id,
                oblivion_one::compositor::WindowId::from_raw(2).expect("background window"),
            ),
            (retained.surface_id, window_id),
        ],
        None,
    );

    let mut resolved = crate::native_output::ResolvedNativeFrameScene::from_server_at(
        &server,
        AnimationTime::from_nanos(0),
    );
    resolved.surfaces = std::borrow::Cow::Owned(vec![background]);
    resolved.lifecycle_surfaces = vec![retained];
    resolved.lifecycle = lifecycle_effect_sample(
        0.45,
        LifecycleDirection::Minimize,
        618,
        1,
        ResolvedEffectScene::default(),
    );
    resolved.lifecycle.samples[0].visual_source.kind = LifecycleVisualSourceKind::NoOwnedEffects;
    set_test_lifecycle_marker_insertions(&server, &mut resolved);
    let sample = &resolved.lifecycle.samples[0];
    let identity = sample.presentation_identity;
    let payload_id = sample.payload_id;

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
        .expect("ordinary lifecycle source frame draws");
    let evidence = match outcome {
        EglFrameOutcome::Rendered {
            lifecycle_evidence, ..
        } => lifecycle_evidence,
        EglFrameOutcome::LifecycleFallback { fallbacks, .. } => {
            panic!("valid ordinary lifecycle source fell back: {fallbacks:?}")
        }
        EglFrameOutcome::Skipped { reason, .. } => {
            panic!("full-damage ordinary lifecycle source was skipped: {reason:?}")
        }
    };

    assert!(evidence.contains(identity, payload_id, 618));
    assert!(
        harness
            .renderer
            .lifecycle
            .is_visual_source_ready(payload_id, &harness.renderer.effect_runtime),
        "ordinary lifecycle surfaces must be captured into a retained visual texture"
    );
    assert!(matches!(
        harness.renderer.lifecycle.lamp_commands(),
        [EglLampDrawCommand {
            layer: EglDrawLayer::LifecycleResolvedVisual(layer_payload),
            ..
        }] if *layer_payload == payload_id
    ));

    let source_signature = harness
        .renderer
        .lifecycle
        .resolved_visual_source_signature(payload_id)
        .expect("ordinary source capture has a signature");
    let texture = harness
        .renderer
        .lifecycle
        .resolved_visual_texture(payload_id)
        .expect("ordinary source capture retains its texture")
        .clone();
    let captured_pixels = read_effect_texture_pixels(
        &mut harness,
        &texture,
        texture.key.width,
        texture.key.height,
    );

    let resized_texture = unsafe {
        harness
            .gl
            .create_texture()
            .expect("resized output texture creates")
    };
    let resized_framebuffer = unsafe {
        harness
            .gl
            .create_framebuffer()
            .expect("resized output framebuffer creates")
    };
    unsafe {
        harness
            .gl
            .bind_texture(glow::TEXTURE_2D, Some(resized_texture));
        harness.gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MIN_FILTER,
            glow::NEAREST as i32,
        );
        harness.gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MAG_FILTER,
            glow::NEAREST as i32,
        );
        harness.gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            640,
            400,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(None),
        );
        harness
            .gl
            .bind_framebuffer(glow::FRAMEBUFFER, Some(resized_framebuffer));
        harness.gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(resized_texture),
            0,
        );
        assert_eq!(
            harness.gl.check_framebuffer_status(glow::FRAMEBUFFER),
            glow::FRAMEBUFFER_COMPLETE,
            "resized output framebuffer is complete"
        );
    }
    let resized_target = EglOutputRenderTarget {
        framebuffer: resized_framebuffer,
        sampleable_texture: Some(resized_texture),
        width: 640,
        height: 400,
        buffer_age: BufferAge::Unsupported,
        framebuffer_origin: OutputFramebufferOrigin::BottomLeft,
    };

    resolved.lifecycle.sampled_at = AnimationTime::from_nanos(2);
    resolved.lifecycle.samples[0].direction = LifecycleDirection::Restore;
    resolved.lifecycle.samples[0].progress = 0.55;
    resolved.lifecycle_surfaces.clear();
    let request = frame_renderer.egl_scene_draw_request(
        640,
        400,
        &resolved,
        &server,
        &input_state,
        crate::native_output::NativeCursorRenderMode::Hardware,
        Some(OutputDamage::Full),
    );
    let outcome = harness
        .renderer
        .draw_scene_to_target(&harness.egl, harness.display, resized_target, request)
        .expect("resized reversal draws without live surfaces");
    let reverse_evidence = match outcome {
        EglFrameOutcome::Rendered {
            lifecycle_evidence, ..
        } => lifecycle_evidence,
        EglFrameOutcome::LifecycleFallback { fallbacks, .. } => {
            panic!("retained ordinary lifecycle source fell back after resize: {fallbacks:?}")
        }
        EglFrameOutcome::Skipped { reason, .. } => {
            panic!("full-damage reversed ordinary lifecycle source was skipped: {reason:?}")
        }
    };

    assert!(reverse_evidence.contains(identity, payload_id, 618));
    assert_eq!(
        harness
            .renderer
            .lifecycle
            .resolved_visual_source_signature(payload_id),
        Some(source_signature),
        "reversal must reuse the source representation captured for its payload"
    );
    assert_eq!(
        read_effect_texture_pixels(
            &mut harness,
            &texture,
            texture.key.width,
            texture.key.height
        ),
        captured_pixels,
        "surface withdrawal must not replace frozen client content during reversal"
    );
    unsafe {
        harness
            .gl
            .bind_framebuffer(glow::FRAMEBUFFER, harness.test_output_framebuffer);
        harness
            .gl
            .bind_texture(glow::TEXTURE_2D, harness.test_output_texture);
        harness.gl.delete_framebuffer(resized_framebuffer);
        harness.gl.delete_texture(resized_texture);
    }
    harness.renderer.scene_state.active_output_framebuffer = harness.test_output_framebuffer;
    harness.renderer.scene_state.active_output_texture = harness.test_output_texture;
    harness.renderer.establish_ordinary_scene_state();
}

#[test]
fn egl_lifecycle_squash_captures_ordinary_surface_content_for_its_payload() {
    let mut harness = GlesEffectTestHarness::new(320, 200);
    harness.install_texture_backed_output();
    let mut buffer_ids = BufferIdAllocator::default();
    let background = lifecycle_test_surface(619, 0, 0, 320, 200, 0xff24_4567, &mut buffer_ids);
    let retained = lifecycle_test_surface(620, 60, 40, 180, 110, 0xffee_8844, &mut buffer_ids);
    let window_id = oblivion_one::compositor::WindowId::from_raw(1).expect("window id");
    let socket_name = format!("typhon-lifecycle-squash-egl-{}", std::process::id());
    let mut server =
        oblivion_one::compositor::OwnCompositorServer::bind_cpu_composition(&socket_name)
            .expect("test compositor binds");
    server.install_native_frame_test_scene_with_server_decorations(
        vec![background.clone()],
        &[
            (
                background.surface_id,
                oblivion_one::compositor::WindowId::from_raw(2).expect("background window"),
            ),
            (retained.surface_id, window_id),
        ],
        None,
    );
    let mut resolved = crate::native_output::ResolvedNativeFrameScene::from_server_at(
        &server,
        AnimationTime::from_nanos(0),
    );
    resolved.surfaces = std::borrow::Cow::Owned(vec![background]);
    resolved.lifecycle_surfaces = vec![retained];
    resolved.lifecycle = lifecycle_effect_sample(
        0.45,
        LifecycleDirection::Minimize,
        620,
        1,
        ResolvedEffectScene::default(),
    );
    resolved.lifecycle.samples[0].effect = LifecycleEffectKind::Squash;
    resolved.lifecycle.samples[0].visual_source.kind = LifecycleVisualSourceKind::NoOwnedEffects;
    set_test_lifecycle_marker_insertions(&server, &mut resolved);
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
        framebuffer_origin: OutputFramebufferOrigin::BottomLeft,
    };
    let outcome = harness
        .renderer
        .draw_scene_to_target(&harness.egl, harness.display, target, request)
        .expect("ordinary Squash source frame draws");
    let evidence = match outcome {
        EglFrameOutcome::Rendered {
            lifecycle_evidence, ..
        } => lifecycle_evidence,
        EglFrameOutcome::LifecycleFallback { fallbacks, .. } => {
            panic!("ordinary Squash source unexpectedly fell back: {fallbacks:?}")
        }
        EglFrameOutcome::Skipped { reason, .. } => {
            panic!("ordinary Squash source frame was skipped: {reason:?}")
        }
    };
    assert!(evidence.contains(identity, payload_id, 620));
    assert!(
        harness
            .renderer
            .lifecycle
            .is_visual_source_ready(payload_id, &harness.renderer.effect_runtime)
    );
    let texture = harness
        .renderer
        .lifecycle
        .resolved_visual_texture(payload_id)
        .expect("Squash retains a resolved visual")
        .clone();
    let captured_pixels = read_effect_texture_pixels(
        &mut harness,
        &texture,
        texture.key.width,
        texture.key.height,
    );

    let mut replacement_ids = BufferIdAllocator::default();
    resolved.lifecycle_surfaces = vec![lifecycle_test_surface(
        620,
        60,
        40,
        180,
        110,
        0xff10_2030,
        &mut replacement_ids,
    )];
    resolved.lifecycle.sampled_at = AnimationTime::from_nanos(2);
    resolved.lifecycle.samples[0].direction = LifecycleDirection::Restore;
    resolved.lifecycle.samples[0].progress = 0.55;
    let request = frame_renderer.egl_scene_draw_request(
        320,
        200,
        &resolved,
        &server,
        &input_state,
        crate::native_output::NativeCursorRenderMode::Hardware,
        Some(OutputDamage::Full),
    );
    let outcome = harness
        .renderer
        .draw_scene_to_target(&harness.egl, harness.display, target, request)
        .expect("reversed Squash source frame draws");
    let reverse_evidence = match outcome {
        EglFrameOutcome::Rendered {
            lifecycle_evidence, ..
        } => lifecycle_evidence,
        EglFrameOutcome::LifecycleFallback { fallbacks, .. } => {
            panic!("reversed Squash source unexpectedly fell back: {fallbacks:?}")
        }
        EglFrameOutcome::Skipped { reason, .. } => {
            panic!("reversed Squash source frame was skipped: {reason:?}")
        }
    };
    assert!(reverse_evidence.contains(identity, payload_id, 620));
    assert_eq!(
        read_effect_texture_pixels(
            &mut harness,
            &texture,
            texture.key.width,
            texture.key.height,
        ),
        captured_pixels,
        "Squash reversal reuses the frozen source after live buffer rotation"
    );
}

#[test]
fn egl_lifecycle_missing_root_source_fails_with_renderer_fallback() {
    let mut harness = GlesEffectTestHarness::new(320, 200);
    harness.install_texture_backed_output();
    let mut buffer_ids = BufferIdAllocator::default();
    let background = lifecycle_test_surface(621, 0, 0, 320, 200, 0xff24_4567, &mut buffer_ids);
    let window_id = oblivion_one::compositor::WindowId::from_raw(1).expect("window id");
    let socket_name = format!("typhon-lifecycle-missing-root-egl-{}", std::process::id());
    let mut server =
        oblivion_one::compositor::OwnCompositorServer::bind_cpu_composition(&socket_name)
            .expect("test compositor binds");
    server.install_native_frame_test_scene_with_server_decorations(
        vec![background.clone()],
        &[
            (
                background.surface_id,
                oblivion_one::compositor::WindowId::from_raw(2).expect("background window"),
            ),
            (622, window_id),
        ],
        None,
    );
    let mut resolved = crate::native_output::ResolvedNativeFrameScene::from_server_at(
        &server,
        AnimationTime::from_nanos(0),
    );
    resolved.surfaces = std::borrow::Cow::Owned(vec![background]);
    resolved.lifecycle_surfaces.clear();
    resolved.lifecycle = lifecycle_effect_sample(
        0.45,
        LifecycleDirection::Minimize,
        622,
        1,
        ResolvedEffectScene::default(),
    );
    resolved.lifecycle.samples[0].visual_source.kind = LifecycleVisualSourceKind::NoOwnedEffects;
    set_test_lifecycle_marker_insertions(&server, &mut resolved);
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
        framebuffer_origin: OutputFramebufferOrigin::BottomLeft,
    };
    let outcome = harness
        .renderer
        .draw_scene_to_target(&harness.egl, harness.display, target, request)
        .expect("missing lifecycle root source is contained as fallback");
    let EglFrameOutcome::LifecycleFallback { fallbacks, .. } = outcome else {
        panic!("missing lifecycle root source must not produce render evidence");
    };
    assert_eq!(fallbacks.failed.len(), 1);
    assert_eq!(fallbacks.failed[0].presentation_identity, identity);
    assert_eq!(fallbacks.failed[0].payload_id, payload_id);
    assert_eq!(fallbacks.failed[0].root_surface_id, 622);
    assert_eq!(
        fallbacks.failed[0].reason,
        LifecycleRenderFallbackReason::LifecycleResourceUnavailable
    );
    assert!(
        !harness
            .renderer
            .lifecycle
            .evidence()
            .contains(identity, payload_id, 622)
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
    set_test_lifecycle_marker_insertions(&server, &mut resolved);
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
        let target_space = crate::egl_renderer::lifecycle::LifecycleRenderTargetSpace::output(
            renderer.scene_state.current_size,
            framebuffer_origin,
        );
        let mut context = LifecycleRenderContext::new(
            &harness.gl,
            &mut renderer.scene_state,
            &mut renderer.effect_runtime,
            renderer.resources.texture_view(),
            target_space,
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
    set_test_lifecycle_marker_insertions(&server, &mut resolved);
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
    let windows = [
        (
            background.surface_id,
            oblivion_one::compositor::WindowId::from_raw(2).expect("background window"),
        ),
        (614, window_id),
    ];
    server.install_native_frame_test_scene_with_server_decorations(
        vec![background.clone(), retained_window.clone()],
        &windows,
        None,
    );
    server.install_native_frame_test_scene_with_server_decorations(
        vec![background.clone()],
        &windows,
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
    set_test_lifecycle_marker_insertions(&server, &mut resolved);
    resolved.lifecycle_snapshot =
        oblivion_one::window_lifecycle_animation::LifecycleFrameSnapshot::from_sample(
            &resolved.lifecycle,
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
    assert!(graph_begins.len() >= 2, "source and frame effect graphs: {trace:?}");
    assert_eq!(graph_ends.len(), graph_begins.len(), "graph executions: {trace:?}");
    assert_eq!(
        graph_release_begins.len(),
        graph_begins.len(),
        "graph releases: {trace:?}"
    );
    assert_eq!(
        graph_release_ends.len(),
        graph_begins.len(),
        "graph releases: {trace:?}"
    );
    for graph_index in 0..graph_begins.len() {
        assert!(
            graph_begins[graph_index] < graph_ends[graph_index]
                && graph_ends[graph_index] < graph_release_begins[graph_index]
                && graph_release_begins[graph_index] < graph_release_ends[graph_index],
            "each effect graph must finish and release its leases: {trace:?}"
        );
    }
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
    if decoration
        .primitives()
        .iter()
        .any(|primitive| matches!(primitive, DecorationRenderPrimitive::Image { .. }))
    {
        assert!(
            commands
                .iter()
                .any(|command| matches!(command.layer, EglDrawLayer::DecorationAsset(_)))
        );
    }
}

#[test]
fn lifecycle_replay_capture_matches_full_output_crop_for_translated_domain() {
    assert_lifecycle_replay_capture_matches_full_output_crop(LifecycleEffectKind::Lamp);
}

#[test]
fn squash_replay_capture_matches_full_output_crop_for_translated_domain() {
    assert_lifecycle_replay_capture_matches_full_output_crop(LifecycleEffectKind::Squash);
}

fn assert_lifecycle_replay_capture_matches_full_output_crop(effect: LifecycleEffectKind) {
    let mode = std::env::var("TYPHON_SCENE_EXECUTION_MODE").unwrap_or_else(|_| "auto".into());
    assert!(
        matches!(mode.as_str(), "auto" | "region" | "legacy"),
        "TYPHON_SCENE_EXECUTION_MODE must be auto, region, or legacy"
    );

    const OUTPUT_SIZE: (u32, u32) = (320, 200);
    // Separate RGBA8 renders can differ by a few channel values from interpolation rounding.
    const PIXEL_TOLERANCE: u8 = 3;
    let capture_domain = EffectRect::new(73, 41, 131, 89).expect("translated capture domain");

    let mut harness = GlesEffectTestHarness::new(OUTPUT_SIZE.0, OUTPUT_SIZE.1);
    harness.install_texture_backed_output();

    let mut buffer_ids = BufferIdAllocator::default();
    let mut lower = lifecycle_test_surface(640, 0, 0, 320, 200, 0xff20_2010, &mut buffer_ids);
    // SurfacePlacement offsets are added to each fixture's x/y coordinates.
    // Zero-offset absolute placements keep those coordinates authoritative.
    lower.placement = SurfacePlacement::absolute_root_at(0, 0);
    let mut retained = lifecycle_test_surface_with_row_colors(
        641,
        60,
        40,
        180,
        110,
        [0xffee_2211, 0xff12_ea14, 0xff12_22ee],
        &mut buffer_ids,
    );
    retained.placement = SurfacePlacement::absolute_root_at(0, 0);
    let mut upper = lifecycle_test_surface(642, 170, 100, 30, 20, 0xffdd_22cc, &mut buffer_ids);
    upper.placement = SurfacePlacement::absolute_root_at(0, 0);
    let lower_window = oblivion_one::compositor::WindowId::from_raw(1).expect("lower window");
    let retained_window = oblivion_one::compositor::WindowId::from_raw(2).expect("retained window");
    let upper_window = oblivion_one::compositor::WindowId::from_raw(3).expect("upper window");
    let socket_name = format!("typhon-lifecycle-capture-space-egl-{}", std::process::id());
    let mut server =
        oblivion_one::compositor::OwnCompositorServer::bind_cpu_composition(&socket_name)
            .expect("test compositor binds");
    let windows = [
        (lower.surface_id, lower_window),
        (retained.surface_id, retained_window),
        (upper.surface_id, upper_window),
    ];
    server.install_native_frame_test_scene(
        vec![lower.clone(), retained.clone(), upper.clone()],
        &windows,
        None,
    );
    server.install_native_frame_test_scene(vec![lower.clone(), upper.clone()], &windows, None);

    let mut resolved = crate::native_output::ResolvedNativeFrameScene::from_server_at(
        &server,
        AnimationTime::from_nanos(0),
    );
    resolved.surfaces = std::borrow::Cow::Owned(vec![lower, upper]);
    resolved.lifecycle_surfaces = vec![retained.clone()];
    resolved.lifecycle = lifecycle_effect_sample(
        0.0,
        LifecycleDirection::Minimize,
        retained.surface_id,
        1,
        ResolvedEffectScene::default(),
    );
    resolved.lifecycle.samples[0].window_id = retained_window;
    resolved.lifecycle.samples[0].visual_source.window_id = retained_window;
    resolved.lifecycle.samples[0].visual_source.kind = LifecycleVisualSourceKind::NoOwnedEffects;
    resolved.lifecycle.samples[0].effect = effect;
    set_test_lifecycle_marker_insertions(&server, &mut resolved);
    let identity = resolved.lifecycle.samples[0].presentation_identity;
    resolved.lifecycle_snapshot =
        oblivion_one::window_lifecycle_animation::LifecycleFrameSnapshot::from_sample(
            &resolved.lifecycle,
        );

    let input_state = crate::native_output::NativeInputState::new(OUTPUT_SIZE.0, OUTPUT_SIZE.1);
    let output_framebuffer = harness
        .test_output_framebuffer
        .expect("output has an FBO-backed render target");
    let output_target = |framebuffer_origin| EglOutputRenderTarget {
        framebuffer: output_framebuffer,
        sampleable_texture: harness.test_output_texture,
        width: OUTPUT_SIZE.0,
        height: OUTPUT_SIZE.1,
        buffer_age: BufferAge::Unsupported,
        framebuffer_origin,
    };

    for framebuffer_origin in [
        OutputFramebufferOrigin::BottomLeft,
        OutputFramebufferOrigin::TopLeftScanout,
    ] {
        let mut frame_renderer = crate::native_output::NativeFrameRenderer::default();
        let request = frame_renderer.egl_scene_draw_request(
            OUTPUT_SIZE.0,
            OUTPUT_SIZE.1,
            &resolved,
            &server,
            &input_state,
            crate::native_output::NativeCursorRenderMode::Hardware,
            Some(OutputDamage::Full),
        );
        let outcome = harness
            .renderer
            .draw_scene_to_target(
                &harness.egl,
                harness.display,
                output_target(framebuffer_origin),
                request,
            )
            .expect("full-output lifecycle reference renders");
        assert!(matches!(outcome, EglFrameOutcome::Rendered { .. }));

        let commands = &harness.renderer.scene_state.commands;
        let lower_index = commands
            .iter()
            .position(|command| command.layer == EglDrawLayer::Surface(640))
            .expect("lower surface command");
        let marker_index = commands
            .iter()
            .position(|command| command.layer == EglDrawLayer::LifecycleOverlay(identity))
            .expect("lifecycle marker command");
        let upper_index = commands
            .iter()
            .position(|command| command.layer == EglDrawLayer::Surface(642))
            .expect("ordinary surface after the lifecycle marker");
        assert!(lower_index < marker_index && marker_index < upper_index);
        let capture_command_indices = (lower_index..=upper_index).collect::<Vec<_>>();
        assert!(capture_command_indices.contains(&marker_index));
        assert!(capture_command_indices.contains(&upper_index));

        unsafe {
            harness
                .gl
                .bind_framebuffer(glow::FRAMEBUFFER, Some(output_framebuffer));
        }
        let reference_pixels = read_effect_test_pixels(&harness.gl, OUTPUT_SIZE.0, OUTPUT_SIZE.1);

        let capture_texture = unsafe {
            harness
                .gl
                .create_texture()
                .expect("capture texture creates")
        };
        let capture_framebuffer = unsafe {
            harness
                .gl
                .create_framebuffer()
                .expect("capture framebuffer creates")
        };
        unsafe {
            harness
                .gl
                .bind_texture(glow::TEXTURE_2D, Some(capture_texture));
            harness.gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::NEAREST as i32,
            );
            harness.gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::NEAREST as i32,
            );
            harness.gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                capture_domain.width as i32,
                capture_domain.height as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            harness
                .gl
                .bind_framebuffer(glow::FRAMEBUFFER, Some(capture_framebuffer));
            harness.gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(capture_texture),
                0,
            );
            assert_eq!(
                harness.gl.check_framebuffer_status(glow::FRAMEBUFFER),
                glow::FRAMEBUFFER_COMPLETE,
                "translated lifecycle capture target is complete"
            );
            harness.gl.viewport(
                0,
                0,
                capture_domain.width as i32,
                capture_domain.height as i32,
            );
            harness.gl.disable(glow::SCISSOR_TEST);
            harness.gl.enable(glow::BLEND);
            harness
                .gl
                .blend_equation_separate(glow::FUNC_ADD, glow::FUNC_ADD);
            harness.gl.blend_func_separate(
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
            );
            harness.gl.clear_color(0.0, 0.0, 0.0, 0.0);
            harness.gl.clear(glow::COLOR_BUFFER_BIT);
        }

        harness.renderer.scene_state.current_framebuffer_origin = framebuffer_origin;
        harness.renderer.effect_runtime.capture_in_progress = true;
        let capture_program = harness.renderer.effect_runtime.capture_program;
        let capture_output_size = harness
            .renderer
            .effect_runtime
            .capture_uniform_location(&harness.gl, "u_capture_output_size");
        let capture_domain_uniform = harness
            .renderer
            .effect_runtime
            .capture_uniform_location(&harness.gl, "u_capture_domain");
        let capture_origin = harness
            .renderer
            .effect_runtime
            .capture_uniform_location(&harness.gl, "u_capture_origin_bottom_left");
        unsafe {
            harness.gl.use_program(Some(capture_program));
            if let Some(location) = capture_output_size.as_ref() {
                harness.gl.uniform_2_f32(
                    Some(location),
                    OUTPUT_SIZE.0 as f32,
                    OUTPUT_SIZE.1 as f32,
                );
            }
            if let Some(location) = capture_domain_uniform.as_ref() {
                harness.gl.uniform_4_f32(
                    Some(location),
                    capture_domain.x as f32,
                    capture_domain.y as f32,
                    capture_domain.width as f32,
                    capture_domain.height as f32,
                );
            }
            if let Some(location) = capture_origin.as_ref() {
                harness.gl.uniform_1_i32(
                    Some(location),
                    i32::from(framebuffer_origin == OutputFramebufferOrigin::BottomLeft),
                );
            }
        }

        let renderer = &mut harness.renderer;
        let (lifecycle_resources, lifecycle_dispatch) = renderer.lifecycle.replay_dispatch_parts();
        let texture_sources = SceneTextureSources {
            ordinary: renderer.resources.texture_view(),
            lifecycle: lifecycle_resources,
        };
        let draw_result = {
            let mut context = EffectExecutionContext::new_with_lifecycle_dispatch(
                &harness.gl,
                &mut renderer.scene_state,
                &mut renderer.effect_runtime,
                texture_sources,
                crate::egl_renderer::lifecycle::LifecycleRenderTargetSpace::output(
                    OUTPUT_SIZE,
                    framebuffer_origin,
                ),
                Some(lifecycle_dispatch),
            );
            context.draw_capture_commands_for_regions(
                &capture_command_indices,
                &[OutputRect::new(
                    capture_domain.x,
                    capture_domain.y,
                    capture_domain.width,
                    capture_domain.height,
                )],
                capture_domain,
                (capture_domain.width, capture_domain.height),
                framebuffer_origin,
                false,
            )
        };
        harness.renderer.effect_runtime.capture_in_progress = false;
        draw_result.expect("selected lifecycle command range replays into the capture target");

        let replay_pixels =
            read_effect_test_pixels(&harness.gl, capture_domain.width, capture_domain.height);
        for capture_y in 0..capture_domain.height {
            let local_y_from_top = capture_domain.height - 1 - capture_y;
            let output_logical_y = capture_domain.y as u32 + local_y_from_top;
            let reference_y = match framebuffer_origin {
                OutputFramebufferOrigin::BottomLeft => OUTPUT_SIZE.1 - 1 - output_logical_y,
                OutputFramebufferOrigin::TopLeftScanout => output_logical_y,
            };
            for capture_x in 0..capture_domain.width {
                let reference = effect_test_pixel(
                    &reference_pixels,
                    OUTPUT_SIZE.0,
                    capture_domain.x as u32 + capture_x,
                    reference_y,
                );
                let replay =
                    effect_test_pixel(&replay_pixels, capture_domain.width, capture_x, capture_y);
                for channel in 0..4 {
                    assert!(
                        reference[channel].abs_diff(replay[channel]) <= PIXEL_TOLERANCE,
                        "full output crop and translated replay differ for {effect:?} at output ({}, {}) / capture ({capture_x}, {capture_y}) for {framebuffer_origin:?}: reference={reference:?}, replay={replay:?}, mode={mode}",
                        capture_domain.x as u32 + capture_x,
                        output_logical_y,
                    );
                }
            }
        }

        for (logical_y, dominant_channel) in [(45_u32, 0), (85_u32, 1), (125_u32, 2)] {
            let local_y = logical_y - capture_domain.y as u32;
            let capture_y = capture_domain.height - 1 - local_y;
            let reference_y = match framebuffer_origin {
                OutputFramebufferOrigin::BottomLeft => OUTPUT_SIZE.1 - 1 - logical_y,
                OutputFramebufferOrigin::TopLeftScanout => logical_y,
            };
            let reference = effect_test_pixel(&reference_pixels, OUTPUT_SIZE.0, 90, reference_y);
            let replay =
                effect_test_pixel(&replay_pixels, capture_domain.width, 90 - 73, capture_y);
            for (representation, pixel) in [("reference", reference), ("replay", replay)] {
                for other_channel in 0..3 {
                    if other_channel == dominant_channel {
                        continue;
                    }
                    assert!(
                        pixel[dominant_channel] > pixel[other_channel].saturating_mul(2),
                        "logical row {logical_y} must preserve its red/green/blue band in the {representation} for {effect:?} / {framebuffer_origin:?}: {pixel:?}"
                    );
                }
            }
        }

        unsafe {
            harness.gl.delete_framebuffer(capture_framebuffer);
            harness.gl.delete_texture(capture_texture);
        }
    }
}

#[path = "lifecycle_precapture_integration.rs"]
mod lifecycle_precapture_integration;
