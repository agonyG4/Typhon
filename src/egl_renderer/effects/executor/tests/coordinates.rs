use super::*;
use crate::egl_renderer::{EglRect, SurfaceSampling};

fn target(
    source: GraphTextureSource,
    domain: oblivion_one::effects::EffectRect,
    width: u32,
    height: u32,
) -> oblivion_one::effects::GraphTexturePlan {
    oblivion_one::effects::GraphTexturePlan {
        id: GraphTextureId::new(1).unwrap(),
        source,
        width,
        height,
        domain,
        working_space: oblivion_one::effects::EffectWorkingSpace::LinearSrgb,
        origin: oblivion_one::effects::GraphTextureOrigin::BottomLeft,
        first_use: None,
        last_use: None,
    }
}

#[test]
fn local_damage_translates_and_scales_into_an_offset_domain() {
    let target = target(
        GraphTextureSource::Intermediate,
        oblivion_one::effects::EffectRect::new(100, 50, 100, 100).unwrap(),
        50,
        50,
    );
    let logical_rect = oblivion_one::effects::EffectRect::new(110, 60, 20, 20).unwrap();
    let coverage = logical_rect_to_physical_coverage(logical_rect, &target).unwrap();
    assert_eq!(
        coverage,
        oblivion_one::effects::GraphTexturePhysicalRect {
            left: 5,
            top: 5,
            right: 15,
            bottom: 15,
        }
    );
    let rect = effect_rect_to_texture_rect(
        logical_rect,
        &target,
        OutputFramebufferOrigin::TopLeftScanout,
    )
    .unwrap();
    assert_eq!(rect, OutputRect::new(5, 35, 10, 10));
}

#[test]
fn executor_scissor_preserves_shared_coverage_for_framebuffer_y_origins() {
    let target = target(
        GraphTextureSource::Output,
        oblivion_one::effects::EffectRect::new(145, 148, 1112, 873).unwrap(),
        556,
        437,
    );
    let logical_rect = oblivion_one::effects::EffectRect::new(647, 901, 610, 120).unwrap();
    let coverage = logical_rect_to_physical_coverage(logical_rect, &target).unwrap();
    assert_eq!(
        coverage,
        oblivion_one::effects::GraphTexturePhysicalRect {
            left: 251,
            top: 376,
            right: 556,
            bottom: 437,
        }
    );
    assert_eq!(
        effect_rect_to_texture_rect(logical_rect, &target, OutputFramebufferOrigin::BottomLeft,)
            .unwrap(),
        OutputRect::new(251, 0, 305, 61)
    );
    assert_eq!(
        effect_rect_to_texture_rect(
            logical_rect,
            &target,
            OutputFramebufferOrigin::TopLeftScanout,
        )
        .unwrap(),
        OutputRect::new(251, 376, 305, 61)
    );
}

#[test]
fn effect_target_flip_y_matches_logical_destination_contract() {
    let cases = [
        (false, OutputFramebufferOrigin::BottomLeft, 0.0),
        (true, OutputFramebufferOrigin::BottomLeft, 0.0),
        (true, OutputFramebufferOrigin::TopLeftScanout, 1.0),
    ];
    for (output_is_framebuffer, origin, logical_top_a_uv_y) in cases {
        let flip = effect_target_requires_logical_y_flip(output_is_framebuffer, origin);
        let logical_top_v_uv_y = if flip {
            1.0 - logical_top_a_uv_y
        } else {
            logical_top_a_uv_y
        };
        assert_eq!(logical_top_v_uv_y, 0.0);
    }
}

#[test]
fn effect_input_flip_y_matches_graph_texture_sample_contract() {
    assert!(effect_input_requires_sample_y_flip(
        oblivion_one::effects::GraphTextureOrigin::BottomLeft,
    ));
    let sample_y = |logical_y: f32| {
        if effect_input_requires_sample_y_flip(
            oblivion_one::effects::GraphTextureOrigin::BottomLeft,
        ) {
            1.0 - logical_y
        } else {
            logical_y
        }
    };
    assert_eq!(sample_y(0.0), 1.0);
    assert_eq!(sample_y(1.0), 0.0);
}

#[test]
fn texture_feedback_alias_is_rejected_even_when_pool_keys_match() {
    let error = validate_no_texture_feedback(Some(17), [17].into_iter()).unwrap_err();
    assert_eq!(
        error,
        EffectExecutionInvariantError::FeedbackTextureAlias {
            input: 17,
            output: 17,
        }
    );

    validate_no_texture_feedback(Some(17), [18].into_iter())
        .expect("different physical resources are safe to sample");
}

#[test]
fn built_in_effect_shaders_keep_logical_and_sample_uv_spaces_separate() {
    for shader in [
        COPY_FRAGMENT_SHADER,
        NORMALIZE_FRAGMENT_SHADER,
        COMPOSITE_FRAGMENT_SHADER,
        FRAGMENT_STAGE_FRAGMENT_SHADER,
        MASK_STAGE_FRAGMENT_SHADER,
        BLEND_STAGE_FRAGMENT_SHADER,
    ] {
        assert!(shader.contains("uniform int u_effect_input_flip_y;"));
        assert!(shader.contains("vec2 typhon_effect_sample_uv(vec2 logical_uv)"));
    }
    assert!(
        NORMALIZE_FRAGMENT_SHADER.contains("output_position = u_effect_output_domain.xy + v_uv")
    );
    assert!(COMPOSITE_FRAGMENT_SHADER.contains("output_position = v_uv * u_effect_output_size"));
    assert!(!NORMALIZE_FRAGMENT_SHADER.contains("texture(u_effect_input, input_uv)"));
    assert!(!COMPOSITE_FRAGMENT_SHADER.contains("texture(u_effect_input, input_uv)"));
}

#[test]
fn composite_shader_applies_presentation_opacity_after_effect_alpha_mode() {
    let force_opaque = COMPOSITE_FRAGMENT_SHADER
        .find("if (u_effect_force_opaque != 0)")
        .expect("composite shader must preserve opaque effect semantics");
    let presentation_opacity = COMPOSITE_FRAGMENT_SHADER
        .find("result *= clamp(u_presentation_opacity, 0.0, 1.0)")
        .expect("composite shader must apply presentation opacity");
    assert!(
        force_opaque < presentation_opacity,
        "PresentationOpacity must be the outermost effect alpha operation"
    );
    assert!(
        COMPOSITE_FRAGMENT_SHADER.contains("out_color = typhon_sanitize_premultiplied(result);")
    );
}

#[test]
fn every_fullscreen_pass_family_uses_the_shared_sample_orientation_contract() {
    for shader in [
        blur::DUAL_KAWASE_DOWNSAMPLE_SHADER,
        blur::DUAL_KAWASE_DOWNSAMPLE_LINEAR_SHADER,
        blur::DUAL_KAWASE_UPSAMPLE_SHADER,
    ] {
        assert!(shader.contains("uniform int u_effect_input_flip_y;"));
        assert!(shader.contains("typhon_effect_sample_uv(v_uv)"));
    }
}

#[test]
fn translating_a_domain_does_not_change_storage_orientation() {
    let first = target(
        GraphTextureSource::Intermediate,
        oblivion_one::effects::EffectRect::new(20, 30, 80, 40).unwrap(),
        40,
        20,
    );
    let moved = target(
        GraphTextureSource::Intermediate,
        oblivion_one::effects::EffectRect::new(700, 500, 80, 40).unwrap(),
        40,
        20,
    );
    assert_eq!(first.origin, moved.origin);
    assert_eq!(
        effect_input_requires_sample_y_flip(first.origin),
        effect_input_requires_sample_y_flip(moved.origin)
    );
}

#[test]
fn direct_capture_transfer_selects_and_normalizes_each_output_origin() {
    let domain = oblivion_one::effects::EffectRect::new(8, 10, 20, 20).unwrap();

    let bottom_left = plan_graph_texture_capture(
        (100, 100),
        domain,
        (20, 20),
        OutputFramebufferOrigin::BottomLeft,
    )
    .expect("bottom-left capture domain should be valid");
    assert_eq!(
        bottom_left.source,
        GlBlitRect {
            x0: 8,
            y0: 70,
            x1: 28,
            y1: 90,
        }
    );
    assert_eq!(
        bottom_left.destination,
        GlBlitRect {
            x0: 0,
            y0: 0,
            x1: 20,
            y1: 20,
        }
    );

    let top_left = plan_graph_texture_capture(
        (100, 100),
        domain,
        (20, 20),
        OutputFramebufferOrigin::TopLeftScanout,
    )
    .expect("top-left capture domain should be valid");
    assert_eq!(
        top_left.source,
        GlBlitRect {
            x0: 8,
            y0: 10,
            x1: 28,
            y1: 30,
        }
    );
    assert_eq!(
        top_left.destination,
        GlBlitRect {
            x0: 0,
            y0: 20,
            x1: 20,
            y1: 0,
        }
    );
}

#[test]
fn shader_copy_maps_each_capture_pixel_like_framebuffer_blit() {
    let output_size = (1920, 1080);
    let domains = [
        oblivion_one::effects::EffectRect::new(762, 976, 396, 104).unwrap(),
        oblivion_one::effects::EffectRect::new(0, 0, 120, 65).unwrap(),
        oblivion_one::effects::EffectRect::new(1610, 0, 310, 65).unwrap(),
        oblivion_one::effects::EffectRect::new(700, 400, 320, 180).unwrap(),
        oblivion_one::effects::EffectRect::new(0, 0, 1920, 1).unwrap(),
        oblivion_one::effects::EffectRect::new(0, 1079, 1920, 1).unwrap(),
        oblivion_one::effects::EffectRect::new(0, 0, 1, 1080).unwrap(),
        oblivion_one::effects::EffectRect::new(1919, 0, 1, 1080).unwrap(),
    ];

    for origin in [
        OutputFramebufferOrigin::BottomLeft,
        OutputFramebufferOrigin::TopLeftScanout,
    ] {
        for domain in domains {
            let target_size = (domain.width, domain.height);
            let transfer = plan_graph_texture_capture(output_size, domain, target_size, origin)
                .expect("edge capture domain is valid");
            for destination_y in 0..target_size.1 {
                for destination_x in 0..target_size.0 {
                    let expected_y = if transfer.destination.y0 < transfer.destination.y1 {
                        transfer.source.y0 + destination_y as i32
                    } else {
                        transfer.source.y1 - 1 - destination_y as i32
                    };
                    assert_eq!(
                        shader_copy_source_texel(
                            output_size,
                            domain,
                            target_size,
                            origin,
                            (destination_x, destination_y),
                        ),
                        Some((transfer.source.x0 + destination_x as i32, expected_y,)),
                        "shader mapping mismatch for {origin:?}, domain {domain:?}, destination ({destination_x}, {destination_y})"
                    );
                }
            }
        }
    }
}

#[test]
fn disjoint_damage_rectangles_preserve_their_hole() {
    let target = target(
        GraphTextureSource::Intermediate,
        oblivion_one::effects::EffectRect::new(0, 0, 100, 100).unwrap(),
        25,
        25,
    );
    let mut damage = EffectRegion::empty();
    damage.push(oblivion_one::effects::EffectRect::new(0, 0, 20, 20).unwrap());
    damage.push(oblivion_one::effects::EffectRect::new(80, 80, 20, 20).unwrap());
    let rects =
        effect_damage_to_texture_rects(&damage, &target, OutputFramebufferOrigin::TopLeftScanout);
    assert_eq!(rects.len(), 2);
}

#[test]
fn composition_position_keeps_before_target_effects_below_the_target() {
    let layers = [
        capture::CaptureLayer::Other,
        capture::CaptureLayer::Surface(10),
        capture::CaptureLayer::Surface(20),
        capture::CaptureLayer::Other,
    ];
    assert_eq!(
        composition_position(
            &layers,
            oblivion_one::compositor::EffectAnchor::BeforeSurface(20),
        ),
        2
    );
    assert_eq!(
        composition_position(
            &layers,
            oblivion_one::compositor::EffectAnchor::AfterSurface(20),
        ),
        3
    );
}

#[test]
fn checkpoint_backdrop_capture_preserves_before_surface_exclusion() {
    let command = |layer| EglDrawCommand {
        layer,
        visual_group: None,
        bounds: EglRect::new(0.0, 0.0, 80.0, 60.0),
        opaque_regions: Vec::new(),
        presentation_clip: None,
        vertex_start: 0,
        vertex_count: 6,
        sampling: SurfaceSampling::ExactNearest,
    };
    let commands = vec![
        command(EglDrawLayer::SolidRgba(0xff22_2222)),
        command(EglDrawLayer::Surface(10)),
        command(EglDrawLayer::Surface(20)),
    ];
    let anchor = oblivion_one::compositor::EffectAnchor::BeforeSurface(20);
    let (draw_end, _) = composition_range(
        &commands,
        anchor,
        None,
        oblivion_one::compositor::EffectAnchorScope::Surface,
    );
    assert_eq!(draw_end, 2, "checkpoint scene must stop before target");

    let layers = commands
        .iter()
        .map(|command| match command.layer {
            EglDrawLayer::Surface(id) => capture::CaptureLayer::Surface(id),
            _ => capture::CaptureLayer::Other,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        capture::indices_for_capture(
            &layers,
            &[None; 3],
            anchor,
            false,
            None,
            oblivion_one::compositor::EffectAnchorScope::Surface,
        ),
        vec![0, 1],
        "direct checkpoint capture must not include target surface"
    );
}

#[test]
fn effect_anchor_scope_selects_exact_surface_or_complete_visual_group_range() {
    let group = oblivion_one::compositor::VisualGroupId::new(7).unwrap();
    let command = |layer| EglDrawCommand {
        layer,
        visual_group: Some(group),
        bounds: EglRect::new(0.0, 0.0, 80.0, 60.0),
        opaque_regions: Vec::new(),
        presentation_clip: None,
        vertex_start: 0,
        vertex_count: 6,
        sampling: SurfaceSampling::ExactNearest,
    };
    let commands = vec![
        command(EglDrawLayer::Surface(1)),
        command(EglDrawLayer::Surface(10)),
        command(EglDrawLayer::Surface(20)),
    ];
    let child_anchor = oblivion_one::compositor::EffectAnchor::BeforeSurface(20);

    assert_eq!(
        composition_range(
            &commands,
            child_anchor,
            Some(group),
            oblivion_one::compositor::EffectAnchorScope::Surface,
        ),
        (2, 2)
    );
    assert_eq!(
        composition_range(
            &commands,
            child_anchor,
            Some(group),
            oblivion_one::compositor::EffectAnchorScope::VisualGroup,
        ),
        (0, 0)
    );
    assert_eq!(
        composition_range(
            &commands,
            oblivion_one::compositor::EffectAnchor::ReplaceSurface(20),
            Some(group),
            oblivion_one::compositor::EffectAnchorScope::VisualGroup,
        ),
        (0, 3)
    );
    assert_eq!(
        composition_range(
            &commands,
            oblivion_one::compositor::EffectAnchor::AfterSurface(20),
            Some(group),
            oblivion_one::compositor::EffectAnchorScope::VisualGroup,
        ),
        (3, 3)
    );
    assert_eq!(
        capture::indices_for_capture(
            &commands
                .iter()
                .map(|command| match command.layer {
                    EglDrawLayer::Surface(id) => capture::CaptureLayer::Surface(id),
                    _ => capture::CaptureLayer::Other,
                })
                .collect::<Vec<_>>(),
            &commands
                .iter()
                .map(|command| command.visual_group)
                .collect::<Vec<_>>(),
            child_anchor,
            false,
            Some(group),
            oblivion_one::compositor::EffectAnchorScope::Surface,
        ),
        vec![0, 1]
    );
}

#[test]
fn before_surface_visual_group_execution_keeps_root_and_decoration_commands() {
    let group = oblivion_one::compositor::VisualGroupId::new(8).unwrap();
    let command = |layer| EglDrawCommand {
        layer,
        visual_group: Some(group),
        bounds: EglRect::new(0.0, 0.0, 80.0, 60.0),
        opaque_regions: Vec::new(),
        presentation_clip: None,
        vertex_start: 0,
        vertex_count: 6,
        sampling: SurfaceSampling::ExactNearest,
    };
    let commands = vec![
        command(EglDrawLayer::SolidRgba(0xff00_0000)),
        command(EglDrawLayer::Surface(42)),
        command(EglDrawLayer::SolidRgba(0xff33_3333)),
    ];
    let anchor = oblivion_one::compositor::EffectAnchor::BeforeSurface(42);

    assert_eq!(
        composition_range(
            &commands,
            anchor,
            Some(group),
            oblivion_one::compositor::EffectAnchorScope::VisualGroup,
        ),
        (0, 0)
    );
    assert_eq!(
        composition_range(
            &commands,
            oblivion_one::compositor::EffectAnchor::ReplaceSurface(42),
            Some(group),
            oblivion_one::compositor::EffectAnchorScope::VisualGroup,
        ),
        (0, 3)
    );
    assert_eq!(
        composition_range(
            &commands,
            oblivion_one::compositor::EffectAnchor::AfterSurface(42),
            Some(group),
            oblivion_one::compositor::EffectAnchorScope::VisualGroup,
        ),
        (3, 3)
    );
    assert_eq!(
        capture::indices_for_capture(
            &commands
                .iter()
                .map(|command| match command.layer {
                    EglDrawLayer::Surface(id) => capture::CaptureLayer::Surface(id),
                    _ => capture::CaptureLayer::Other,
                })
                .collect::<Vec<_>>(),
            &commands
                .iter()
                .map(|command| command.visual_group)
                .collect::<Vec<_>>(),
            anchor,
            true,
            Some(group),
            oblivion_one::compositor::EffectAnchorScope::VisualGroup,
        ),
        vec![0, 1, 2]
    );
}
