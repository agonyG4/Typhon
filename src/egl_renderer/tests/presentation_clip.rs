use super::*;

fn clear_pbuffer(harness: &GlesEffectTestHarness) {
    unsafe {
        harness.gl.disable(glow::SCISSOR_TEST);
        harness.gl.clear_color(0.0, 0.0, 0.0, 0.0);
        harness.gl.clear(glow::COLOR_BUFFER_BIT);
    }
}

#[test]
fn real_gles_scene_clip_and_same_owner_capture_bypass() {
    let mut harness = GlesEffectTestHarness::new(4, 1);
    let pixel = rgba_to_pixel([255, 0, 0, 255]);
    let texture = create_effect_test_texture(&harness.gl, 1, 1, &[255, 0, 0, 255]);
    let resource = EglImageResource {
        texture,
        size: (1, 1),
        generation: 0,
        egl_image: None,
    };
    harness
        .renderer
        .decoration_resources
        .insert(DecorationResourceKey::Solid(pixel), resource);

    let group = oblivion_one::compositor::VisualGroupId::new(1).expect("visual group");
    harness.renderer.vertices.clear();
    harness.renderer.commands.clear();
    push_draw_command(
        &mut harness.renderer.vertices,
        &mut harness.renderer.commands,
        EglDrawLayer::SolidRgba(pixel),
        EglRect::new(0.0, 0.0, 4.0, 1.0),
        4,
        1,
        OutputFramebufferOrigin::BottomLeft,
    );
    harness.renderer.commands[0].visual_group = Some(group);
    harness.renderer.commands[0].presentation_clip = Some(EglRect::new(1.0, 0.0, 2.0, 1.0));
    harness
        .renderer
        .presentation_visual_group_owners
        .insert(group, 7);
    harness.renderer.presentation_opacities = vec![1.0];

    clear_pbuffer(&harness);
    harness
        .renderer
        .draw_command_batch_with_visibility(true, None, false)
        .expect("clipped scene draw succeeds");
    let clipped = read_effect_test_pixels(&harness.gl, 4, 1);
    assert_effect_test_pixel(&clipped, 4, 0, 0, [0, 0, 0, 0]);
    assert_effect_test_pixel(&clipped, 4, 1, 0, [255, 0, 0, 255]);
    assert_effect_test_pixel(&clipped, 4, 2, 0, [255, 0, 0, 255]);
    assert_effect_test_pixel(&clipped, 4, 3, 0, [0, 0, 0, 0]);

    clear_pbuffer(&harness);
    harness.renderer.capture_unclipped_presentation_owner = Some(7);
    harness
        .renderer
        .draw_command_batch_with_visibility(true, None, false)
        .expect("same-owner unclipped capture draw succeeds");
    harness.renderer.capture_unclipped_presentation_owner = None;
    let captured = read_effect_test_pixels(&harness.gl, 4, 1);
    for x in 0..4 {
        assert_effect_test_pixel(&captured, 4, x, 0, [255, 0, 0, 255]);
    }

    clear_pbuffer(&harness);
    harness.renderer.capture_unclipped_presentation_owner = Some(8);
    harness
        .renderer
        .draw_command_batch_with_visibility(true, None, false)
        .expect("other-owner capture draw succeeds");
    harness.renderer.capture_unclipped_presentation_owner = None;
    let other_owner = read_effect_test_pixels(&harness.gl, 4, 1);
    assert_effect_test_pixel(&other_owner, 4, 0, 0, [0, 0, 0, 0]);
    assert_effect_test_pixel(&other_owner, 4, 1, 0, [255, 0, 0, 255]);
    assert_effect_test_pixel(&other_owner, 4, 2, 0, [255, 0, 0, 255]);
    assert_effect_test_pixel(&other_owner, 4, 3, 0, [0, 0, 0, 0]);

    unsafe { harness.gl.delete_texture(texture) };
}

#[test]
fn real_gles_clip_intersects_frame_damage_scissor() {
    let mut harness = GlesEffectTestHarness::new(4, 1);
    let pixel = rgba_to_pixel([0, 255, 0, 255]);
    let texture = create_effect_test_texture(&harness.gl, 1, 1, &[0, 255, 0, 255]);
    let resource = EglImageResource {
        texture,
        size: (1, 1),
        generation: 0,
        egl_image: None,
    };
    harness
        .renderer
        .decoration_resources
        .insert(DecorationResourceKey::Solid(pixel), resource);
    let group = oblivion_one::compositor::VisualGroupId::new(1).expect("visual group");
    harness.renderer.vertices.clear();
    harness.renderer.commands.clear();
    push_draw_command(
        &mut harness.renderer.vertices,
        &mut harness.renderer.commands,
        EglDrawLayer::SolidRgba(pixel),
        EglRect::new(0.0, 0.0, 4.0, 1.0),
        4,
        1,
        OutputFramebufferOrigin::BottomLeft,
    );
    harness.renderer.commands[0].visual_group = Some(group);
    harness.renderer.commands[0].presentation_clip = Some(EglRect::new(1.0, 0.0, 2.0, 1.0));
    harness.renderer.presentation_opacities = vec![1.0];

    clear_pbuffer(&harness);
    harness
        .renderer
        .draw_command_batch_with_visibility_and_range(
            true,
            Some(OutputRect::new(2, 0, 2, 1)),
            false,
            None,
            false,
        )
        .expect("damage and Clip scissor draw succeeds");
    let pixels = read_effect_test_pixels(&harness.gl, 4, 1);
    assert_effect_test_pixel(&pixels, 4, 0, 0, [0, 0, 0, 0]);
    assert_effect_test_pixel(&pixels, 4, 1, 0, [0, 0, 0, 0]);
    assert_effect_test_pixel(&pixels, 4, 2, 0, [0, 255, 0, 255]);
    assert_effect_test_pixel(&pixels, 4, 3, 0, [0, 0, 0, 0]);

    unsafe { harness.gl.delete_texture(texture) };
}

#[test]
fn popup_visual_group_inherits_window_group_clip_from_its_presentation_owner() {
    let mut harness = GlesEffectTestHarness::new(16, 16);
    let root = test_shm_surface(RenderableSurfaceDamage::full());
    let mut popup = root.clone();
    popup.surface_id = 8;
    popup.placement = oblivion_one::compositor::SurfacePlacement::subsurface(7, 0, 0);
    let surfaces = vec![root, popup];
    let signatures = egl_scene_surface_signatures(&surfaces);
    let owner_root = 701;
    let scene_node_id = oblivion_one::core::SceneNodeId::from_raw(1).expect("scene node");
    let rect = oblivion_one::presentation_animation::PresentationClipRect::new(2.0, 3.0, 6.0, 7.0)
        .expect("WindowGroup clip");
    let clip = oblivion_one::presentation_animation::PresentationGroupClip::with_scene_node(
        scene_node_id,
        owner_root,
        oblivion_one::presentation_animation::PresentationClip::Rect(rect),
        Some(rect),
        None,
    );

    harness.renderer.rebuild_scene_commands(
        16,
        16,
        &surfaces,
        &[],
        &[8],
        1,
        1.0,
        1,
        &signatures,
        &[],
        0,
        &[],
        &[clip],
        &HashMap::from([(7, owner_root), (8, owner_root)]),
        OutputFramebufferOrigin::BottomLeft,
    );

    let root_command = harness
        .renderer
        .commands
        .iter()
        .find(|command| command.layer == EglDrawLayer::Surface(7))
        .expect("toplevel command");
    let popup_command = harness
        .renderer
        .commands
        .iter()
        .find(|command| command.layer == EglDrawLayer::Surface(8))
        .expect("popup command");
    let expected = EglRect::new(2.0, 3.0, 6.0, 7.0);
    assert_ne!(root_command.visual_group, popup_command.visual_group);
    assert_eq!(root_command.presentation_clip, Some(expected));
    assert_eq!(popup_command.presentation_clip, Some(expected));
}

#[test]
fn real_gles_blur_effects_keep_kernel_source_and_clip_final_contribution() {
    let mut harness = GlesEffectTestHarness::new(64, 64);
    let rect = EffectRect::new(16, 16, 16, 16).expect("blur target rect");
    let visual_group = oblivion_one::compositor::VisualGroupId::new(9).expect("visual group");
    let owner_root = 42;
    let clip = EglRect::new(24.0, 16.0, 8.0, 16.0);
    let clip_pixel = rgba_to_pixel([32, 64, 96, 255]);
    let background = create_effect_test_texture(&harness.gl, 1, 1, &[32, 64, 96, 255]);
    harness.renderer.decoration_resources.insert(
        DecorationResourceKey::Solid(clip_pixel),
        EglImageResource {
            texture: background,
            size: (1, 1),
            generation: 0,
            egl_image: None,
        },
    );
    let surface_pixels = [255, 0, 0, 255].repeat(16 * 16);
    let surface = create_effect_test_texture(&harness.gl, 16, 16, &surface_pixels);
    harness.renderer.surface_resources.insert(
        owner_root,
        EglSurfaceResource {
            image: EglImageResource {
                texture: surface,
                size: (16, 16),
                generation: 0,
                egl_image: None,
            },
            dmabuf_key: None,
            buffer_lifetime: None,
            shm_synced_commit: None,
        },
    );
    install_visual_group_scene_commands(&mut harness.renderer, rect, visual_group);
    let target_command = harness
        .renderer
        .commands
        .iter_mut()
        .find(|command| command.layer == EglDrawLayer::Surface(owner_root))
        .expect("target command");
    target_command.presentation_clip = Some(clip);
    harness
        .renderer
        .presentation_visual_group_clips
        .insert(visual_group, clip);
    harness
        .renderer
        .presentation_visual_group_owners
        .insert(visual_group, owner_root);
    assert_eq!(
        harness
            .renderer
            .presentation_clip_for_visual_group(Some(visual_group)),
        Some(clip)
    );
    assert_eq!(
        harness.renderer.presentation_clip_for_visual_group(None),
        None
    );

    let output_bounds = EffectRect::new(0, 0, 64, 64).expect("output bounds");
    let full_damage = EffectRegion::from_rect(output_bounds);
    let repaint_plan = RepaintPlan {
        render_damage: OutputDamage::Full,
        repair_damage: OutputDamage::Full,
        buffer_age: None,
        mode: RepaintMode::Full,
        fallback_reason: None,
    };
    for anchor in [
        oblivion_one::compositor::EffectAnchor::BeforeSurface(owner_root),
        oblivion_one::compositor::EffectAnchor::AfterSurface(owner_root),
        oblivion_one::compositor::EffectAnchor::ReplaceSurface(owner_root),
    ] {
        let (mut scene, registry) = moving_blur_scene(rect);
        let instance = scene
            .instances
            .first_mut()
            .expect("background blur instance");
        instance.anchor = anchor;
        instance.visual_group = Some(visual_group);
        instance.anchor_scope = oblivion_one::compositor::EffectAnchorScope::VisualGroup;
        instance.scene_order = oblivion_one::compositor::EffectSceneOrder::for_anchor(anchor);
        let plan = oblivion_one::effects::compile_frame_execution_plan(
            &scene,
            &full_damage,
            output_bounds,
            &registry,
        )
        .expect("blur graph compiles");
        let oblivion_one::effects::FrameExecutionPlan::EffectGraph(graph) = plan else {
            panic!("background blur compiles to an effect graph");
        };
        let captured = graph
            .textures
            .iter()
            .find(|texture| texture.source == GraphTextureSource::CapturedScene)
            .expect("blur captures its source");
        assert!(
            captured.domain.x < 24,
            "Clip must not truncate blur kernel input for {anchor:?}"
        );

        let demand = plan_effect_execution_demand(&graph, &full_damage, true);
        let selection = effects::select_effect_execution(&graph, &demand);
        clear_pbuffer(&harness);
        effects::execute_effect_graph(
            &mut harness.renderer,
            &graph,
            OutputFramebufferOrigin::BottomLeft,
            &repaint_plan,
            &demand,
            &selection,
        )
        .expect("clipped blur graph executes in real GLES");
        let pixels = read_effect_test_pixels(&harness.gl, 64, 64);
        assert_effect_test_pixel(&pixels, 64, 20, 20, [32, 64, 96, 255]);
        assert_ne!(
            &pixels[((20 * 64 + 28) * 4) as usize..((20 * 64 + 28) * 4 + 4) as usize],
            &[32, 64, 96, 255],
            "the clipped effect remains visible inside its mask for {anchor:?}"
        );
    }

    unsafe {
        harness.gl.delete_texture(background);
        harness.gl.delete_texture(surface);
    }
}
