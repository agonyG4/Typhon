use super::*;

#[test]
fn lifecycle_output_capture_is_bottom_left_canonical_for_both_origins() {
    let mut harness = GlesEffectTestHarness::new(8, 6);
    harness.install_texture_backed_output();
    let target = harness
        .renderer
        .effect_runtime
        .effect_resources
        .acquire(&harness.gl, lifecycle_transfer_test_texture_key(8, 6))
        .expect("lifecycle capture texture allocates");
    let rect = PresentationRect::new(0.0, 0.0, 8.0, 6.0).expect("full output rectangle");
    let mut captured = Vec::new();

    for framebuffer_origin in [
        OutputFramebufferOrigin::BottomLeft,
        OutputFramebufferOrigin::TopLeftScanout,
    ] {
        fill_lifecycle_transfer_test_pattern(&harness, framebuffer_origin);
        clear_effect_texture(&mut harness.renderer, &target)
            .expect("lifecycle capture texture clears");
        copy_output_region_to_texture(&mut harness.renderer, &target, rect, framebuffer_origin)
            .expect("lifecycle output region captures");
        captured.push(read_effect_texture_pixels(&mut harness, &target, 8, 6));
    }

    assert_eq!(
        captured[0], captured[1],
        "the same logical image must produce identical BottomLeft lifecycle textures"
    );
    assert_effect_test_pixel(&captured[0], 8, 0, 5, lifecycle_transfer_test_color(0, 0));
    assert_effect_test_pixel(&captured[0], 8, 0, 0, lifecycle_transfer_test_color(0, 5));
}

#[test]
fn lifecycle_output_blit_plan_clips_edges_and_keeps_texture_offsets() {
    let cases = [
        (
            PresentationRect::new(2.0, 1.0, 6.0, 6.0).unwrap(),
            GlBlitRect::new(2, 1, 8, 7),
            GlBlitRect::new(2, 1, 8, 7),
            GlBlitRect::new(0, 0, 6, 6),
        ),
        (
            PresentationRect::new(2.0, -2.0, 6.0, 6.0).unwrap(),
            GlBlitRect::new(2, 4, 8, 8),
            GlBlitRect::new(2, 0, 8, 4),
            GlBlitRect::new(0, 0, 6, 4),
        ),
        (
            PresentationRect::new(2.0, 6.0, 6.0, 6.0).unwrap(),
            GlBlitRect::new(2, 0, 8, 2),
            GlBlitRect::new(2, 6, 8, 8),
            GlBlitRect::new(0, 4, 6, 6),
        ),
        (
            PresentationRect::new(-2.0, 1.0, 6.0, 6.0).unwrap(),
            GlBlitRect::new(0, 1, 4, 7),
            GlBlitRect::new(0, 1, 4, 7),
            GlBlitRect::new(2, 0, 6, 6),
        ),
        (
            PresentationRect::new(8.0, 1.0, 6.0, 6.0).unwrap(),
            GlBlitRect::new(8, 1, 10, 7),
            GlBlitRect::new(8, 1, 10, 7),
            GlBlitRect::new(0, 0, 2, 6),
        ),
    ];

    for (rect, expected_bottom_left_output, expected_top_left_output, expected_texture) in cases {
        let bottom_left = lifecycle_output_copy_region(
            (10, 8),
            rect,
            (6, 6),
            1.0,
            OutputFramebufferOrigin::BottomLeft,
        )
        .expect("visible BottomLeft transfer plan");
        let top_left = lifecycle_output_copy_region(
            (10, 8),
            rect,
            (6, 6),
            1.0,
            OutputFramebufferOrigin::TopLeftScanout,
        )
        .expect("visible TopLeftScanout transfer plan");
        assert_eq!(bottom_left.output, expected_bottom_left_output);
        assert_eq!(bottom_left.texture, expected_texture);
        assert_eq!(top_left.output, expected_top_left_output);
        assert_eq!(
            top_left.texture,
            GlBlitRect::new(
                expected_texture.x0,
                expected_texture.y1,
                expected_texture.x1,
                expected_texture.y0,
            )
        );
    }
}

#[test]
fn lifecycle_output_capture_preserves_pixels_and_clipped_offsets() {
    let mut harness = GlesEffectTestHarness::new(10, 8);
    harness.install_texture_backed_output();
    let rects = [
        PresentationRect::new(2.0, 1.0, 6.0, 6.0).expect("fully visible rectangle"),
        PresentationRect::new(2.0, -2.0, 6.0, 6.0).expect("top-clipped rectangle"),
        PresentationRect::new(2.0, 6.0, 6.0, 6.0).expect("bottom-clipped rectangle"),
        PresentationRect::new(-2.0, 1.0, 6.0, 6.0).expect("left-clipped rectangle"),
        PresentationRect::new(8.0, 1.0, 6.0, 6.0).expect("right-clipped rectangle"),
    ];

    for framebuffer_origin in [
        OutputFramebufferOrigin::BottomLeft,
        OutputFramebufferOrigin::TopLeftScanout,
    ] {
        for rect in rects {
            let target = harness
                .renderer
                .effect_runtime
                .effect_resources
                .acquire(&harness.gl, lifecycle_transfer_test_texture_key(6, 6))
                .expect("lifecycle capture texture allocates");
            fill_lifecycle_transfer_test_pattern(&harness, framebuffer_origin);
            harness.renderer.bind_active_output_framebuffer();
            clear_effect_texture(&mut harness.renderer, &target)
                .expect("lifecycle capture texture clears");
            copy_output_region_to_texture(&mut harness.renderer, &target, rect, framebuffer_origin)
                .expect("lifecycle output region captures");

            let captured = read_effect_texture_pixels(&mut harness, &target, 6, 6);
            for texture_y in 0..6 {
                let logical_y = 5 - texture_y;
                for texture_x in 0..6 {
                    let output_x = rect.x() as i32 + texture_x as i32;
                    let output_y = rect.y() as i32 + logical_y as i32;
                    let expected = if (0..10).contains(&output_x) && (0..8).contains(&output_y) {
                        lifecycle_transfer_test_color(output_x as u32, output_y as u32)
                    } else {
                        [0, 0, 0, 0]
                    };
                    assert_effect_test_pixel(&captured, 6, texture_x, texture_y, expected);
                }
            }

            harness
                .renderer
                .effect_runtime
                .effect_resources
                .release(target)
                .expect("lifecycle capture texture releases");
        }
    }
}

#[test]
fn gles_framebuffer_blit_and_shader_copy_capture_pixels_match_at_edges() {
    let mut harness = GlesEffectTestHarness::new(64, 48);
    harness.install_texture_backed_output();
    fill_shader_copy_test_pattern(&harness);
    let domains = [
        EffectRect::new(0, 0, 16, 12).expect("top-left edge domain"),
        EffectRect::new(0, 36, 16, 12).expect("bottom-left edge domain"),
        EffectRect::new(48, 8, 16, 12).expect("right edge domain"),
        EffectRect::new(20, 8, 16, 12).expect("interior domain"),
    ];
    for origin in [
        OutputFramebufferOrigin::BottomLeft,
        OutputFramebufferOrigin::TopLeftScanout,
    ] {
        for domain in domains {
            let blit_target_plan = GraphTexturePlan {
                id: GraphTextureId::new(1).expect("capture target id"),
                source: GraphTextureSource::CapturedScene,
                width: domain.width,
                height: domain.height,
                domain,
                working_space: EffectWorkingSpace::OutputEncodedSrgb,
                origin: oblivion_one::effects::GraphTextureOrigin::BottomLeft,
                first_use: None,
                last_use: None,
            };
            let shader_target_plan = GraphTexturePlan {
                id: GraphTextureId::new(2).expect("shader-copy target id"),
                ..blit_target_plan.clone()
            };
            let blit_target = harness
                .renderer
                .effect_runtime
                .effect_resources
                .acquire_plan(&harness.gl, &blit_target_plan)
                .expect("blit capture target acquires");
            let shader_target = harness
                .renderer
                .effect_runtime
                .effect_resources
                .acquire_plan(&harness.gl, &shader_target_plan)
                .expect("shader-copy capture target acquires");
            effects::capture_output_region_to_graph_texture(
                &mut harness.renderer,
                &blit_target,
                &blit_target_plan,
                origin,
            )
            .expect("framebuffer blit capture succeeds");
            let blit_pixels =
                read_effect_texture_pixels(&mut harness, &blit_target, domain.width, domain.height);
            let output_texture = harness
                .renderer
                .scene_state
                .active_output_texture
                .expect("shader-copy test output texture");
            effects::capture_output_region_to_graph_texture_shader_copy(
                &mut harness.renderer,
                &shader_target,
                &shader_target_plan,
                origin,
                output_texture,
            )
            .expect("shader copy capture succeeds");
            let shader_pixels = read_effect_texture_pixels(
                &mut harness,
                &shader_target,
                domain.width,
                domain.height,
            );
            assert_eq!(
                blit_pixels, shader_pixels,
                "capture pixels differ for {origin:?} domain {domain:?}"
            );
            harness
                .renderer
                .effect_runtime
                .effect_resources
                .release(blit_target)
                .expect("blit capture target releases");
            harness
                .renderer
                .effect_runtime
                .effect_resources
                .release(shader_target)
                .expect("shader-copy capture target releases");
        }
    }
}
#[test]
fn gles_checkpoint_single_rect_update_matches_full_reference_for_both_origins() {
    let dirty = EffectRect::new(16, 11, 7, 5).unwrap();
    for origin in [
        OutputFramebufferOrigin::BottomLeft,
        OutputFramebufferOrigin::TopLeftScanout,
    ] {
        assert_shader_copy_partial_update_matches_full_reference(origin, &[dirty]);
    }
}

#[test]
fn gles_checkpoint_disjoint_updates_preserve_the_gap_for_both_origins() {
    let dirty = [
        EffectRect::new(15, 10, 6, 5).unwrap(),
        EffectRect::new(35, 25, 4, 7).unwrap(),
    ];
    for origin in [
        OutputFramebufferOrigin::BottomLeft,
        OutputFramebufferOrigin::TopLeftScanout,
    ] {
        assert_shader_copy_partial_update_matches_full_reference(origin, &dirty);
    }
}

#[test]
fn gles_checkpoint_empty_update_does_not_change_a_valid_texture() {
    for origin in [
        OutputFramebufferOrigin::BottomLeft,
        OutputFramebufferOrigin::TopLeftScanout,
    ] {
        let mut harness = GlesEffectTestHarness::new(64, 48);
        harness.install_texture_backed_output();
        fill_shader_copy_test_pattern(&harness);
        let domain = EffectRect::new(11, 7, 39, 31).unwrap();
        let target_plan = GraphTexturePlan {
            id: GraphTextureId::new(1).unwrap(),
            source: GraphTextureSource::CapturedScene,
            width: domain.width,
            height: domain.height,
            domain,
            working_space: EffectWorkingSpace::OutputEncodedSrgb,
            origin: oblivion_one::effects::GraphTextureOrigin::BottomLeft,
            first_use: None,
            last_use: None,
        };
        let target = harness
            .renderer
            .effect_runtime
            .effect_resources
            .acquire_plan(&harness.gl, &target_plan)
            .expect("cache target acquires");
        let output_texture = harness.renderer.scene_state.active_output_texture.unwrap();
        effects::capture_output_region_to_graph_texture_shader_copy(
            &mut harness.renderer,
            &target,
            &target_plan,
            origin,
            output_texture,
        )
        .expect("initial full capture succeeds");
        let before = read_effect_texture_pixels(&mut harness, &target, domain.width, domain.height);

        effects::capture_output_rects_to_graph_texture_shader_copy(
            &mut harness.renderer,
            &target,
            &target_plan,
            origin,
            output_texture,
            &[],
        )
        .expect("empty update performs no shader-copy draw");
        let after = read_effect_texture_pixels(&mut harness, &target, domain.width, domain.height);
        assert_eq!(
            before, after,
            "empty update changed the cache for {origin:?}"
        );
        harness
            .renderer
            .effect_runtime
            .effect_resources
            .release(target)
            .expect("cache target releases");
    }
}
#[test]
fn replay_capture_clears_translated_texture_scissor_in_gles() {
    let mut harness = GlesEffectTestHarness::new(64, 64);
    let (graph, target, materialization_region, output_rect) = translated_capture_test_graph();
    let pass_id = graph.passes[0].id;
    let instance = graph.passes[0].instance;
    let mut demand = oblivion_one::effects::EffectExecutionDemand::new(
        vec![EffectInstanceExecutionDemand {
            id: instance,
            presentation_output_region: materialization_region.clone(),
            output_region: materialization_region.clone(),
        }],
        materialization_region.clone(),
    );
    demand.passes = vec![EffectPassExecutionDemand {
        id: pass_id,
        output_region: materialization_region.clone(),
    }];
    let selection = effects::select_effect_execution(&graph, &demand);
    let plan = diagnostic_repaint_plan_for_repairs_in_size(&[output_rect], false, (64, 64));

    for origin in [
        OutputFramebufferOrigin::BottomLeft,
        OutputFramebufferOrigin::TopLeftScanout,
    ] {
        let poisoned = harness
            .renderer
            .effect_runtime
            .effect_resources
            .acquire_plan(&harness.gl, &target)
            .expect("translated capture poison texture acquires");
        harness
            .renderer
            .effect_runtime
            .effect_resources
            .bind_render_target(&harness.gl, &poisoned)
            .expect("translated capture poison target binds");
        unsafe {
            harness.gl.disable(glow::SCISSOR_TEST);
            harness.gl.disable(glow::BLEND);
            harness.gl.clear_color(1.0, 0.0, 1.0, 1.0);
            harness.gl.clear(glow::COLOR_BUFFER_BIT);
        }
        harness
            .renderer
            .effect_runtime
            .effect_resources
            .unbind_render_target(&harness.gl);
        harness
            .renderer
            .effect_runtime
            .effect_resources
            .release(poisoned)
            .expect("translated capture poison texture releases");

        effects::execute_effect_graph_with_debug_config(
            &mut harness.renderer,
            &graph,
            origin,
            &plan,
            &demand,
            &selection,
            effects::EffectDebugConfig::new(
                effects::EffectDebugCaptureMode::Replay,
                effects::EffectDebugKawaseMode::Partial,
            ),
        )
        .expect("translated capture graph renders");

        let inspected = harness
            .renderer
            .effect_runtime
            .effect_resources
            .acquire_plan(&harness.gl, &target)
            .expect("translated capture inspection texture acquires");
        harness
            .renderer
            .effect_runtime
            .effect_resources
            .bind_render_target(&harness.gl, &inspected)
            .expect("translated capture inspection target binds");
        let mut pixels = vec![0_u8; 8 * 8 * 4];
        unsafe {
            harness.gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
            harness.gl.read_pixels(
                0,
                0,
                8,
                8,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut pixels)),
            );
        }
        harness
            .renderer
            .effect_runtime
            .effect_resources
            .unbind_render_target(&harness.gl);
        harness
            .renderer
            .effect_runtime
            .effect_resources
            .release(inspected)
            .expect("translated capture inspection texture releases");

        for y in 0..8 {
            for x in 0..8 {
                let index = ((y * 8 + x) * 4) as usize;
                let actual: [u8; 4] = pixels[index..index + 4]
                    .try_into()
                    .expect("translated capture pixel has four channels");
                let expected = if (6..8).contains(&x) && (0..2).contains(&y) {
                    [0, 0, 0, 0]
                } else {
                    [255, 0, 255, 255]
                };
                assert_eq!(
                    actual, expected,
                    "translated capture clear mismatch at local texture ({x}, {y}) for {origin:?}; output rect {output_rect:?}"
                );
            }
        }
    }
}
