use super::super::*;
use super::*;

pub(in crate::egl_renderer::tests) fn fill_shader_copy_output_rect(
    harness: &GlesEffectTestHarness,
    origin: OutputFramebufferOrigin,
    rect: EffectRect,
    color: [u8; 4],
) {
    let height = harness.renderer.scene_state.current_size.1;
    let physical_y = match origin {
        OutputFramebufferOrigin::BottomLeft => {
            i32::try_from(height).unwrap() - rect.y - rect.height as i32
        }
        OutputFramebufferOrigin::TopLeftScanout => rect.y,
    };
    harness.renderer.bind_active_output_framebuffer();
    unsafe {
        harness.gl.disable(glow::BLEND);
        harness.gl.enable(glow::SCISSOR_TEST);
        harness
            .gl
            .scissor(rect.x, physical_y, rect.width as i32, rect.height as i32);
        harness.gl.clear_color(
            f32::from(color[0]) / 255.0,
            f32::from(color[1]) / 255.0,
            f32::from(color[2]) / 255.0,
            f32::from(color[3]) / 255.0,
        );
        harness.gl.clear(glow::COLOR_BUFFER_BIT);
        harness.gl.disable(glow::SCISSOR_TEST);
    }
    harness.renderer.establish_ordinary_scene_state();
}

pub(in crate::egl_renderer::tests) fn assert_shader_copy_partial_update_matches_full_reference(
    origin: OutputFramebufferOrigin,
    dirty_rects: &[EffectRect],
) {
    let mut harness = GlesEffectTestHarness::new(64, 48);
    harness.install_texture_backed_output();
    fill_shader_copy_test_pattern(&harness);
    let domain = EffectRect::new(11, 7, 39, 31).unwrap();
    let target_plan = GraphTexturePlan {
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
    let reference_plan = GraphTexturePlan {
        id: GraphTextureId::new(2).expect("reference target id"),
        ..target_plan.clone()
    };
    let target = harness
        .renderer
        .effect_runtime
        .effect_resources
        .acquire_plan(&harness.gl, &target_plan)
        .expect("persistent capture texture acquires");
    let reference = harness
        .renderer
        .effect_runtime
        .effect_resources
        .acquire_plan(&harness.gl, &reference_plan)
        .expect("full reference texture acquires");
    let output_texture = harness
        .renderer
        .scene_state
        .active_output_texture
        .expect("shader-copy test output texture");

    effects::capture_output_region_to_graph_texture_shader_copy(
        &mut harness.renderer,
        &target,
        &target_plan,
        origin,
        output_texture,
    )
    .expect("first cache population copies the full checkpoint");
    effects::capture_output_region_to_graph_texture(
        &mut harness.renderer,
        &reference,
        &reference_plan,
        origin,
    )
    .expect("initial full-current reference capture succeeds");
    let previous = read_effect_texture_pixels(&mut harness, &target, domain.width, domain.height);
    let initial_reference =
        read_effect_texture_pixels(&mut harness, &reference, domain.width, domain.height);
    assert_eq!(
        previous, initial_reference,
        "first population must cover the full domain"
    );

    let mut damage = EffectRegion::empty();
    for (index, rect) in dirty_rects.iter().copied().enumerate() {
        damage = damage.union(&EffectRegion::from_rect(rect));
        fill_shader_copy_output_rect(
            &harness,
            origin,
            rect,
            [201, 31 + index as u8 * 17, 89, 255],
        );
    }
    let (output_rects, target_rects) =
        effects::checkpoint_update_rects_for_test(&damage, domain, (64, 48), &target_plan);
    let expected_copied_pixels = dirty_rects.iter().fold(0u64, |sum, rect| {
        sum.saturating_add(u64::from(rect.width).saturating_mul(u64::from(rect.height)))
    });
    let physically_copied_pixels = output_rects.iter().fold(0u64, |sum, rect| {
        sum.saturating_add(u64::from(rect.width).saturating_mul(u64::from(rect.height)))
    });
    assert_eq!(physically_copied_pixels, expected_copied_pixels);
    assert_eq!(target_rects.len(), dirty_rects.len());

    effects::capture_output_rects_to_graph_texture_shader_copy(
        &mut harness.renderer,
        &target,
        &target_plan,
        origin,
        output_texture,
        &target_rects,
    )
    .expect("partial checkpoint shader-copy update succeeds");
    effects::capture_output_region_to_graph_texture(
        &mut harness.renderer,
        &reference,
        &reference_plan,
        origin,
    )
    .expect("full-current reference capture succeeds");
    let incremental =
        read_effect_texture_pixels(&mut harness, &target, domain.width, domain.height);
    let full_current =
        read_effect_texture_pixels(&mut harness, &reference, domain.width, domain.height);
    for y in 0..domain.height {
        for x in 0..domain.width {
            let offset = ((y * domain.width + x) * 4) as usize;
            let changed = target_rects.iter().any(|rect| {
                let right = rect.x as u32 + rect.width;
                let bottom = rect.y as u32 + rect.height;
                x >= rect.x as u32 && x < right && y >= rect.y as u32 && y < bottom
            });
            let expected = if changed {
                &full_current[offset..offset + 4]
            } else {
                &previous[offset..offset + 4]
            };
            assert_eq!(
                &incremental[offset..offset + 4],
                expected,
                "partial update differs at local texture pixel ({x}, {y}) for {origin:?}"
            );
        }
    }

    harness
        .renderer
        .effect_runtime
        .effect_resources
        .release(target)
        .expect("incrementally updated texture releases");
    harness
        .renderer
        .effect_runtime
        .effect_resources
        .release(reference)
        .expect("reference texture releases");
}

pub(in crate::egl_renderer::tests) fn diagnostic_pixel(
    pixels: &[u8],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
) -> [u8; 4] {
    let physical_y = height.saturating_sub(y).saturating_sub(1);
    let index = ((physical_y * width + x) * 4) as usize;
    pixels[index..index + 4]
        .try_into()
        .expect("diagnostic pixel has four channels")
}

pub(in crate::egl_renderer::tests) fn diagnostic_pixel_for_origin(
    pixels: &[u8],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    origin: OutputFramebufferOrigin,
) -> [u8; 4] {
    let physical_y = match origin {
        OutputFramebufferOrigin::BottomLeft => height.saturating_sub(y).saturating_sub(1),
        OutputFramebufferOrigin::TopLeftScanout => y,
    };
    let index = ((physical_y * width + x) * 4) as usize;
    pixels[index..index + 4]
        .try_into()
        .expect("diagnostic pixel has four channels")
}

#[allow(clippy::too_many_arguments)]
pub(in crate::egl_renderer::tests) fn diagnostic_matrix_mismatch_counts_for_origin(
    actual: &[u8],
    previous: &[u8],
    full_reference: &[u8],
    width: u32,
    height: u32,
    repairs: &[OutputRect],
    tolerance: u8,
    origin: OutputFramebufferOrigin,
) -> (usize, usize) {
    let mut outside = 0;
    let mut inside = 0;
    for y in 0..height {
        for x in 0..width {
            let inside_repair = diagnostic_pixel_is_inside_repairs(repairs, x, y);
            let expected = if inside_repair {
                diagnostic_pixel_for_origin(full_reference, width, height, x, y, origin)
            } else {
                diagnostic_pixel_for_origin(previous, width, height, x, y, origin)
            };
            let actual_pixel = diagnostic_pixel_for_origin(actual, width, height, x, y, origin);
            if actual_pixel
                .iter()
                .zip(expected)
                .any(|(actual, expected)| actual.abs_diff(expected) > tolerance)
            {
                if inside_repair {
                    inside += 1;
                } else {
                    outside += 1;
                }
            }
        }
    }
    (outside, inside)
}

pub(in crate::egl_renderer::tests) fn translated_capture_test_graph() -> (
    CompiledFrameGraph,
    GraphTexturePlan,
    EffectRegion,
    OutputRect,
) {
    let instance =
        oblivion_one::effects::EffectInstanceId::new(1).expect("translated capture instance id");
    let pass_id = oblivion_one::effects::GraphPassId::new(1).expect("translated capture pass id");
    let texture_id = GraphTextureId::new(1).expect("translated capture texture id");
    let domain = EffectRect::new(56, 56, 8, 8).expect("translated capture domain");
    let target = GraphTexturePlan {
        id: texture_id,
        source: GraphTextureSource::CapturedScene,
        width: domain.width,
        height: domain.height,
        domain,
        working_space: EffectWorkingSpace::OutputEncodedSrgb,
        origin: oblivion_one::effects::GraphTextureOrigin::BottomLeft,
        first_use: None,
        last_use: None,
    };
    let pass = CompiledRenderPass {
        id: pass_id,
        kind: RenderPassKind::SceneCapture,
        inputs: Vec::new(),
        output: Some(texture_id),
        damage: EffectRegion::empty(),
        instance,
        anchor: oblivion_one::compositor::EffectAnchor::OutputPostProcess,
        blur_radius: None,
        stage: None,
        fused_stages: Vec::new(),
        parameter_block: EffectParameterBlock::default(),
        alpha_mode: EffectAlphaMode::Opaque,
        encode_output: false,
        color_conversion: EffectColorConversion::None,
        checkpoint_dependencies: Vec::new(),
        visual_group: None,
        anchor_scope: oblivion_one::compositor::EffectAnchorScope::Surface,
        visible_clip_fallback: None,
    };
    let materialization =
        EffectRect::new(62, 62, 2, 2).expect("translated capture materialization region");
    let materialization_region = EffectRegion::from_rect(materialization);
    let graph = CompiledFrameGraph {
        passes: vec![pass],
        textures: vec![target.clone()],
        instances: vec![oblivion_one::effects::CompiledEffectInstance {
            semantic_signature: 0,
            frame_demand: oblivion_one::effects::EffectFrameDemand::OnDamage,
            id: instance,
            output_influence_region: EffectRegion::from_rect(domain),
            capture_region: EffectRegion::from_rect(domain),
            dependencies: Vec::new(),
        }],
        final_damage: EffectRegion::empty(),
        stats: Default::default(),
    };
    (
        graph,
        target,
        materialization_region,
        OutputRect::new(62, 62, 2, 2),
    )
}
