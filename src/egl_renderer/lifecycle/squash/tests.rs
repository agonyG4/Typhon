use super::*;
use crate::egl_renderer::tests::{
    GlesEffectTestHarness, draw_lifecycle_markers_for_test,
    lifecycle_test_lamp_sample as lamp_test_sample,
    lifecycle_test_surface_fixture as lifecycle_test_surface,
    lifecycle_transfer_texture_key as lifecycle_transfer_test_texture_key,
    test_lifecycle_identities_for_windows,
};
use oblivion_one::presentation_animation::PresentationRect;
use oblivion_one::render_backend::buffer::BufferIdAllocator;

fn sampling_test_texture_filters(gl: &glow::Context, texture: glow::Texture) -> (i32, i32) {
    unsafe {
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        let filters = (
            gl.get_tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER),
            gl.get_tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER),
        );
        gl.bind_texture(glow::TEXTURE_2D, None);
        filters
    }
}

fn prepare_two_window_sampling_draw(harness: &mut GlesEffectTestHarness) -> [glow::Texture; 2] {
    let mut lifecycle = lamp_test_sample(0.5);
    lifecycle.samples[0].effect = LifecycleEffectKind::Squash;
    let mut second_sample = lifecycle.samples[0].clone();
    second_sample.window_id = compositor::WindowId::from_raw(902).expect("second test window");
    let identities = test_lifecycle_identities_for_windows(&[
        lifecycle.samples[0].window_id,
        second_sample.window_id,
    ]);
    assert_eq!(identities[0], lifecycle.samples[0].presentation_identity);
    let second_identity = identities[1];
    second_sample.presentation_identity = second_identity;
    second_sample.root_surface_id = 2;
    second_sample.payload_id =
        compositor::PresentationRetainedVisualPayloadId::from_origin_identity(second_identity);
    assert_ne!(second_sample.payload_id, lifecycle.samples[0].payload_id);
    second_sample.visual_source.presentation_identity = second_identity;
    second_sample.visual_source.payload_id = second_sample.payload_id;
    second_sample.visual_source.resource_payload_id = second_sample.payload_id;
    second_sample.visual_source.root_surface_id = second_sample.root_surface_id;
    lifecycle.samples.push(second_sample);
    harness.renderer.lifecycle.begin_frame(&lifecycle);

    let texture_key = lifecycle_transfer_test_texture_key(4, 4);
    let mut textures = Vec::new();
    for (index, sample) in lifecycle.samples.iter().enumerate() {
        let texture = harness
            .renderer
            .effect_runtime
            .effect_resources
            .acquire(&harness.gl, texture_key)
            .expect("resolved lifecycle texture allocates");
        let texture_id = harness
            .renderer
            .effect_runtime
            .effect_resources
            .texture(&texture)
            .expect("resolved lifecycle texture is available");
        unsafe {
            harness.gl.bind_texture(glow::TEXTURE_2D, Some(texture_id));
            harness.gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                if index == 0 {
                    glow::LINEAR as i32
                } else {
                    glow::NEAREST as i32
                },
            );
            harness.gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                if index == 0 {
                    glow::LINEAR as i32
                } else {
                    glow::NEAREST as i32
                },
            );
            harness.gl.bind_texture(glow::TEXTURE_2D, None);
        }
        textures.push(texture_id);
        harness
            .renderer
            .lifecycle
            .visual_store
            .resolved_visual_resources
            .insert(
                sample.payload_id,
                LifecycleResolvedVisualResource {
                    texture,
                    source_signature: 0,
                },
            );
    }
    harness.renderer.lifecycle.rebuild_squash_commands(
        &lifecycle,
        &[],
        &[],
        1.0,
        (4, 4),
        OutputFramebufferOrigin::BottomLeft,
    );
    assert_eq!(
        harness.renderer.lifecycle.squash.commands.len(),
        2,
        "each lifecycle window contributes one textured draw"
    );
    assert!(
        harness
            .renderer
            .lifecycle
            .squash
            .commands
            .iter()
            .all(|command| command.command.sampling == SurfaceSampling::ScaledLinear)
    );
    assert_ne!(
        textures[0], textures[1],
        "the two windows must use distinct GL texture objects"
    );
    assert_ne!(
        harness.renderer.lifecycle.squash.commands[0].presentation_identity,
        harness.renderer.lifecycle.squash.commands[1].presentation_identity,
        "the two draws must belong to distinct lifecycle windows"
    );
    assert_eq!(
        sampling_test_texture_filters(&harness.gl, textures[0]),
        (glow::LINEAR as i32, glow::LINEAR as i32)
    );
    assert_eq!(
        sampling_test_texture_filters(&harness.gl, textures[1]),
        (glow::NEAREST as i32, glow::NEAREST as i32)
    );
    [textures[0], textures[1]]
}

fn draw_squash_sampling_pass(harness: &mut GlesEffectTestHarness) {
    let identities = harness
        .renderer
        .lifecycle
        .samples()
        .iter()
        .map(|sample| sample.presentation_identity)
        .collect::<Vec<_>>();
    draw_lifecycle_markers_for_test(harness, &identities).expect("Squash sampling draw succeeds");
}

fn assert_squash_textures_have_filter(
    gl: &glow::Context,
    textures: [glow::Texture; 2],
    expected_filter: i32,
) {
    for (index, texture) in textures.into_iter().enumerate() {
        assert_eq!(
            sampling_test_texture_filters(gl, texture),
            (expected_filter, expected_filter),
            "resolved Squash texture {index} must carry the requested min/mag filter"
        );
    }
}

fn set_squash_test_texture_filter(gl: &glow::Context, texture: glow::Texture, filter: u32) {
    unsafe {
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, filter as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, filter as i32);
        gl.bind_texture(glow::TEXTURE_2D, None);
    }
}

#[test]
fn squash_uses_linear_sampling_for_each_resolved_window_texture() {
    let mut harness = GlesEffectTestHarness::new(4, 4);
    let textures = prepare_two_window_sampling_draw(&mut harness);

    draw_squash_sampling_pass(&mut harness);

    assert_eq!(
        harness.renderer.lifecycle.evidence().consumed.len(),
        2,
        "both lifecycle textures must reach an executed GLES draw"
    );
    assert_squash_textures_have_filter(&harness.gl, textures, glow::LINEAR as i32);
}

#[test]
fn squash_tracks_filter_per_texture_across_consecutive_commands() {
    let mut harness = GlesEffectTestHarness::new(4, 4);
    let textures = prepare_two_window_sampling_draw(&mut harness);
    set_squash_test_texture_filter(&harness.gl, textures[0], glow::NEAREST);
    set_squash_test_texture_filter(&harness.gl, textures[1], glow::LINEAR);
    let first = harness.renderer.lifecycle.squash.commands[0].clone();
    let second = harness.renderer.lifecycle.squash.commands[1].clone();
    let mut first_nearest = first.clone();
    first_nearest.command.sampling = SurfaceSampling::ExactNearest;
    let mut first_linear = first;
    first_linear.command.sampling = SurfaceSampling::ScaledLinear;
    let mut second_nearest = second;
    second_nearest.command.sampling = SurfaceSampling::ExactNearest;
    harness.renderer.lifecycle.squash.commands = vec![
        first_nearest.clone(),
        first_linear,
        first_nearest,
        second_nearest,
    ];

    draw_squash_sampling_pass(&mut harness);

    assert_eq!(
        harness.renderer.lifecycle.evidence().consumed.len(),
        2,
        "both lifecycle windows must reach an executed GLES draw"
    );
    assert_eq!(
        sampling_test_texture_filters(&harness.gl, textures[0]),
        (glow::NEAREST as i32, glow::NEAREST as i32),
        "the first texture must follow its final ExactNearest command"
    );
    assert_eq!(
        sampling_test_texture_filters(&harness.gl, textures[1]),
        (glow::NEAREST as i32, glow::NEAREST as i32),
        "the second texture must receive ExactNearest after the first texture alternates modes"
    );
}

#[test]
fn squash_reapplies_filter_after_an_earlier_pass_changes_texture_state() {
    let mut harness = GlesEffectTestHarness::new(4, 4);
    let textures = prepare_two_window_sampling_draw(&mut harness);
    for texture in textures {
        set_squash_test_texture_filter(&harness.gl, texture, glow::NEAREST);
    }
    for command in &mut harness.renderer.lifecycle.squash.commands {
        command.command.sampling = SurfaceSampling::ScaledLinear;
    }

    draw_squash_sampling_pass(&mut harness);

    assert_squash_textures_have_filter(&harness.gl, textures, glow::LINEAR as i32);
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> PresentationRect {
    PresentationRect::new(x, y, width, height).expect("valid test rectangle")
}

fn squash_sample(
    progress: f64,
    canonical_client: PresentationRect,
    canonical_visual: PresentationRect,
    presented_source_client: PresentationRect,
    anchor: PresentationRect,
) -> LifecycleSceneSample {
    let mut lifecycle = lamp_test_sample(progress);
    let sample = &mut lifecycle.samples[0];
    sample.effect = LifecycleEffectKind::Squash;
    sample.visual_group = LifecycleVisualGroup::from_effect_bounds(
        LifecycleEffectKind::Squash,
        canonical_client,
        canonical_visual,
        presented_source_client,
        anchor,
        1600,
        1200,
    )
    .expect("valid Squash visual group");
    lifecycle
}

fn assert_rect_close(actual: EglRect, expected: PresentationRect) {
    assert!(
        (f64::from(actual.x()) - expected.x()).abs() < 1e-3,
        "x: actual {}, expected {}",
        actual.x(),
        expected.x()
    );
    assert!(
        (f64::from(actual.y()) - expected.y()).abs() < 1e-3,
        "y: actual {}, expected {}",
        actual.y(),
        expected.y()
    );
    assert!(
        (f64::from(actual.width()) - expected.width()).abs() < 1e-3,
        "width: actual {}, expected {}",
        actual.width(),
        expected.width()
    );
    assert!(
        (f64::from(actual.height()) - expected.height()).abs() < 1e-3,
        "height: actual {}, expected {}",
        actual.height(),
        expected.height()
    );
}

fn transformed_from_canonical(
    group: LifecycleVisualGroup,
    primitive: PresentationRect,
    progress: f64,
) -> PresentationRect {
    let current = oblivion_one::window_lifecycle_animation::squash_client_rect_at_progress(
        group.presented_source_client_rect,
        group.anchor_rect,
        progress,
    )
    .expect("finite current Squash client rectangle");
    oblivion_one::window_lifecycle_animation::squash_transform_rect(
        group.canonical_client_rect,
        current,
        primitive,
    )
    .expect("finite transformed test primitive")
}

fn source_command_for_surface(
    harness: &GlesEffectTestHarness,
    payload_id: compositor::PresentationRetainedVisualPayloadId,
    surface_id: u32,
) -> &EglDrawCommand {
    &harness.renderer.lifecycle.visual_store.source_commands[&payload_id]
        .iter()
        .find(|command| matches!(command.layer, EglDrawLayer::Surface(id) if id == surface_id))
        .expect("lifecycle source surface command")
}

fn resolved_visual_command<'a>(
    harness: &'a GlesEffectTestHarness,
    lifecycle: &LifecycleSceneSample,
) -> &'a EglDrawCommand {
    let sample = &lifecycle.samples[0];
    &harness
        .renderer
        .lifecycle
        .squash
        .commands
        .iter()
        .find(|entry| {
            entry.presentation_identity == sample.presentation_identity
                && matches!(entry.command.layer, EglDrawLayer::LifecycleResolvedVisual(id) if id == sample.payload_id)
        })
        .expect("Squash resolved visual command")
        .command
}

#[test]
fn squash_resolved_source_is_one_quad_and_does_not_use_lamp_mesh() {
    let mut harness = GlesEffectTestHarness::new(320, 200);
    let mut lifecycle = lamp_test_sample(0.5);
    let sample = &mut lifecycle.samples[0];
    let source = PresentationRect::new(100.0, 80.0, 160.0, 100.0).expect("source client rectangle");
    let anchor = PresentationRect::new(240.0, 140.0, 48.0, 32.0).expect("taskbar anchor rectangle");
    sample.effect = LifecycleEffectKind::Squash;
    sample.visual_group = LifecycleVisualGroup::from_effect_bounds(
        LifecycleEffectKind::Squash,
        source,
        source,
        source,
        anchor,
        320,
        200,
    )
    .expect("valid Squash visual group");
    sample.visual_source.kind = LifecycleVisualSourceKind::ResolvedOwnedEffects;
    harness.renderer.lifecycle.rebuild_squash_commands(
        &lifecycle,
        &[],
        &[],
        1.0,
        (320, 200),
        OutputFramebufferOrigin::BottomLeft,
    );

    assert_eq!(harness.renderer.lifecycle.squash.vertices.len(), 6);
    assert_eq!(harness.renderer.lifecycle.squash.commands.len(), 1);
    assert!(matches!(
        harness.renderer.lifecycle.squash.commands[0].command.layer,
        EglDrawLayer::LifecycleResolvedVisual(_)
    ));
    assert!(harness.renderer.lifecycle.lamp.vertices.is_empty());
    assert!(harness.renderer.lifecycle.lamp.commands.is_empty());
}

#[test]
fn squash_missing_retained_texture_falls_back_without_render_evidence() {
    let mut harness = GlesEffectTestHarness::new(320, 200);
    let mut lifecycle = lamp_test_sample(0.5);
    let sample = &mut lifecycle.samples[0];
    let source = PresentationRect::new(100.0, 80.0, 160.0, 100.0).expect("source client rectangle");
    let anchor = PresentationRect::new(240.0, 140.0, 48.0, 32.0).expect("taskbar anchor rectangle");
    sample.effect = LifecycleEffectKind::Squash;
    sample.visual_group = LifecycleVisualGroup::from_effect_bounds(
        LifecycleEffectKind::Squash,
        source,
        source,
        source,
        anchor,
        320,
        200,
    )
    .expect("valid Squash visual group");
    let identity = sample.presentation_identity;
    let retained_surface = lifecycle_test_surface(
        1,
        100,
        80,
        160,
        100,
        0xffee_8844,
        &mut BufferIdAllocator::default(),
    );
    harness.renderer.lifecycle.begin_frame(&lifecycle);
    harness.renderer.lifecycle.rebuild_squash_commands(
        &lifecycle,
        std::slice::from_ref(&retained_surface),
        &[],
        1.0,
        (320, 200),
        OutputFramebufferOrigin::BottomLeft,
    );

    draw_lifecycle_markers_for_test(&mut harness, &[identity])
        .expect("missing retained texture is a recoverable lifecycle fallback");
    let fallbacks = harness.renderer.lifecycle.fallbacks();
    assert_eq!(fallbacks.failed.len(), 1);
    assert_eq!(fallbacks.failed[0].effect, LifecycleEffectKind::Squash);
    assert!(harness.renderer.lifecycle.evidence().consumed.is_empty());
}

#[test]
fn squash_affine_quad_preserves_uvs_for_both_framebuffer_origins() {
    let source = PresentationRect::new(100.0, 80.0, 160.0, 100.0).expect("source client rectangle");
    let anchor = PresentationRect::new(240.0, 140.0, 48.0, 32.0).expect("taskbar anchor rectangle");
    let group = LifecycleVisualGroup::from_effect_bounds(
        LifecycleEffectKind::Squash,
        source,
        source,
        source,
        anchor,
        320,
        200,
    )
    .expect("valid Squash visual group");
    for origin in [
        OutputFramebufferOrigin::BottomLeft,
        OutputFramebufferOrigin::TopLeftScanout,
    ] {
        let mut vertices = Vec::new();
        let mut commands = Vec::new();
        push_draw_command(
            &mut vertices,
            &mut commands,
            EglDrawLayer::Surface(7),
            EglRect::new(110.0, 90.0, 40.0, 30.0),
            320,
            200,
            origin,
        );
        let original_uvs = vertices.iter().map(|vertex| vertex.uv).collect::<Vec<_>>();
        let original_first = ndc_to_output_point(vertices[0].position, (320, 200), origin);

        assert!(transform_squash_geometry(
            &mut vertices,
            &mut commands,
            group,
            0.5,
            1.0,
            (320, 200),
            origin,
        ));
        assert_eq!(
            vertices.iter().map(|vertex| vertex.uv).collect::<Vec<_>>(),
            original_uvs
        );
        let transformed_first = ndc_to_output_point(vertices[0].position, (320, 200), origin);
        let current = oblivion_one::window_lifecycle_animation::squash_client_rect_at_progress(
            source, anchor, 0.5,
        )
        .expect("finite midpoint client rectangle");
        let expected = [
            current.x() + (original_first[0] - source.x()) * current.width() / source.width(),
            current.y() + (original_first[1] - source.y()) * current.height() / source.height(),
        ];
        assert!((transformed_first[0] - expected[0]).abs() < 1e-4);
        assert!((transformed_first[1] - expected[1]).abs() < 1e-4);
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].vertex_count, 6);
    }
}

#[test]
fn squash_raw_root_starts_at_presented_geometry_for_both_framebuffer_origins() {
    let canonical = rect(100.0, 80.0, 800.0, 600.0);
    let presented = rect(240.0, 150.0, 600.0, 450.0);
    let anchor = rect(900.0, 700.0, 56.0, 40.0);
    let lifecycle = squash_sample(0.0, canonical, canonical, presented, anchor);

    for origin in [
        OutputFramebufferOrigin::BottomLeft,
        OutputFramebufferOrigin::TopLeftScanout,
    ] {
        let mut harness = GlesEffectTestHarness::new(1600, 1200);
        let mut surface = lifecycle_test_surface(
            1,
            100,
            80,
            800,
            600,
            0xffee_8844,
            &mut BufferIdAllocator::default(),
        );
        surface.placement = compositor::SurfacePlacement::absolute_root_at(0, 0);
        let sample = &lifecycle.samples[0];
        harness.renderer.lifecycle.rebuild_visual_source_commands(
            &lifecycle,
            std::slice::from_ref(&surface),
            &[],
            1.0,
            (1600, 1200),
            origin,
        );
        assert_rect_close(
            source_command_for_surface(&harness, sample.payload_id, 1).bounds,
            presented,
        );
        harness.renderer.lifecycle.rebuild_squash_commands(
            &lifecycle,
            std::slice::from_ref(&surface),
            &[],
            1.0,
            (1600, 1200),
            origin,
        );

        assert_rect_close(
            resolved_visual_command(&harness, &lifecycle).bounds,
            presented,
        );
    }
}

#[test]
fn squash_raw_subsurface_uses_canonical_affine_mapping_at_start_and_midflight() {
    let canonical = rect(100.0, 80.0, 800.0, 600.0);
    let presented = rect(240.0, 150.0, 600.0, 450.0);
    let anchor = rect(900.0, 700.0, 56.0, 40.0);
    let mut root = lifecycle_test_surface(
        1,
        100,
        80,
        800,
        600,
        0xffee_8844,
        &mut BufferIdAllocator::default(),
    );
    root.placement = compositor::SurfacePlacement::absolute_root_at(0, 0);
    let mut subsurface = lifecycle_test_surface(
        2,
        0,
        0,
        200,
        100,
        0xff44_88ee,
        &mut BufferIdAllocator::default(),
    );
    subsurface.placement = compositor::SurfacePlacement::subsurface(1, 160, 120);
    let surfaces = [root, subsurface];
    for progress in [0.0, 0.5] {
        let lifecycle = squash_sample(progress, canonical, canonical, presented, anchor);
        let mut harness = GlesEffectTestHarness::new(1600, 1200);
        let sample = &lifecycle.samples[0];
        harness.renderer.lifecycle.rebuild_visual_source_commands(
            &lifecycle,
            &surfaces,
            &[],
            1.0,
            (1600, 1200),
            OutputFramebufferOrigin::BottomLeft,
        );
        assert_rect_close(
            source_command_for_surface(&harness, sample.payload_id, 2).bounds,
            rect(360.0, 240.0, 150.0, 75.0),
        );
        harness.renderer.lifecycle.rebuild_squash_commands(
            &lifecycle,
            &surfaces,
            &[],
            1.0,
            (1600, 1200),
            OutputFramebufferOrigin::BottomLeft,
        );

        let target = oblivion_one::window_lifecycle_animation::squash_visual_rect_at_progress(
            sample.visual_group.presented_source_client_rect,
            sample.visual_group.anchor_rect,
            sample.visual_group.presented_source_visual_rect,
            progress,
        );
        let target = target.expect("finite resolved Squash target");
        assert_rect_close(resolved_visual_command(&harness, &lifecycle).bounds, target);
    }
}

#[test]
fn squash_frozen_ssd_uses_the_same_canonical_affine_mapping_as_client_content() {
    let canonical = rect(100.0, 80.0, 800.0, 600.0);
    let canonical_visual = rect(80.0, 40.0, 840.0, 680.0);
    let presented = rect(240.0, 150.0, 600.0, 450.0);
    let anchor = rect(900.0, 700.0, 56.0, 40.0);

    for progress in [0.0, 0.5] {
        let lifecycle = squash_sample(progress, canonical, canonical_visual, presented, anchor);
        let sample = &lifecycle.samples[0];
        let source_ssd = rect(80.0, 40.0, 200.0, 60.0);
        let expected = transformed_from_canonical(sample.visual_group, source_ssd, progress);
        if progress == 0.0 {
            assert_eq!(expected, rect(225.0, 120.0, 150.0, 45.0));
        }
        let mut vertices = Vec::new();
        let mut commands = Vec::new();
        push_draw_command(
            &mut vertices,
            &mut commands,
            EglDrawLayer::SolidRgba(0xff33_3333),
            EglRect::new(80.0, 40.0, 200.0, 60.0),
            1600,
            1200,
            OutputFramebufferOrigin::BottomLeft,
        );
        assert!(transform_squash_geometry(
            &mut vertices,
            &mut commands,
            sample.visual_group,
            progress,
            1.0,
            (1600, 1200),
            OutputFramebufferOrigin::BottomLeft,
        ));

        assert_rect_close(commands[0].bounds, expected);
    }
}

#[test]
fn squash_resolved_visual_stays_in_presented_source_space() {
    let canonical = rect(100.0, 80.0, 800.0, 600.0);
    let canonical_visual = rect(80.0, 40.0, 840.0, 680.0);
    let presented = rect(240.0, 150.0, 600.0, 450.0);
    let anchor = rect(900.0, 700.0, 56.0, 40.0);
    let mut lifecycle = squash_sample(0.0, canonical, canonical_visual, presented, anchor);
    lifecycle.samples[0].visual_source.kind = LifecycleVisualSourceKind::ResolvedOwnedEffects;
    let expected = lifecycle.samples[0]
        .visual_group
        .presented_source_visual_rect;
    let mut harness = GlesEffectTestHarness::new(1600, 1200);
    harness.renderer.lifecycle.rebuild_squash_commands(
        &lifecycle,
        &[],
        &[],
        1.0,
        (1600, 1200),
        OutputFramebufferOrigin::BottomLeft,
    );

    let command = &harness.renderer.lifecycle.squash.commands[0].command;
    assert!(matches!(
        command.layer,
        EglDrawLayer::LifecycleResolvedVisual(_)
    ));
    assert_rect_close(command.bounds, expected);
    assert_eq!(command.sampling, SurfaceSampling::ScaledLinear);
}

#[test]
fn squash_damage_contains_presented_visual_when_canonical_geometry_differs() {
    let canonical = rect(100.0, 80.0, 800.0, 600.0);
    let canonical_visual = rect(80.0, 40.0, 840.0, 680.0);
    let presented = rect(240.0, 150.0, 600.0, 450.0);
    let anchor = rect(900.0, 700.0, 56.0, 40.0);
    let lifecycle = squash_sample(0.0, canonical, canonical_visual, presented, anchor);
    let group = lifecycle.samples[0].visual_group;
    assert_ne!(
        group.canonical_client_rect,
        group.presented_source_client_rect
    );
    let damage = LifecycleFrameState::damage_for_snapshot(&lifecycle, (1600, 1200), 1.0);
    let OutputDamage::Rects(rects) = damage else {
        panic!("expected bounded lifecycle damage");
    };
    let visual = group.presented_source_visual_rect;
    assert!(rects.iter().any(|damage| {
        f64::from(damage.x) <= visual.x()
            && f64::from(damage.y) <= visual.y()
            && f64::from(damage.x) + f64::from(damage.width) >= visual.x() + visual.width()
            && f64::from(damage.y) + f64::from(damage.height) >= visual.y() + visual.height()
    }));
}

#[test]
fn squash_rescales_exact_nearest_surface_commands_with_linear_sampling_and_keeps_uvs() {
    let canonical = rect(100.0, 80.0, 800.0, 600.0);
    let surface = lifecycle_test_surface(
        1,
        100,
        80,
        800,
        600,
        0xffee_8844,
        &mut BufferIdAllocator::default(),
    );

    for anchor in [
        rect(900.0, 700.0, 56.0, 40.0),
        rect(900.0, 700.0, 200.0, 400.0),
    ] {
        let current = oblivion_one::window_lifecycle_animation::squash_client_rect_at_progress(
            canonical, anchor, 0.5,
        )
        .expect("finite sampled Squash client rectangle");
        let scale_x = current.width() / canonical.width();
        let scale_y = current.height() / canonical.height();
        assert!(scale_x < 1.0 && scale_y < 1.0, "both cases downscale");
        if anchor.width() == 200.0 {
            assert!(
                (scale_x - scale_y).abs() > 0.01,
                "second case is non-uniform"
            );
        }

        let assignments =
            compositor::surface_render_space_assignments(std::slice::from_ref(&surface), 1.0);
        let mut ordinary_vertices = Vec::new();
        let mut ordinary_commands = Vec::new();
        push_egl_surface_commands(
            &mut ordinary_vertices,
            &mut ordinary_commands,
            1600,
            1200,
            &surface,
            assignments[0].clone(),
            OutputFramebufferOrigin::BottomLeft,
            None,
        );
        assert_eq!(ordinary_commands[0].sampling, SurfaceSampling::ExactNearest);
        let ordinary_uvs = ordinary_vertices
            .iter()
            .map(|vertex| vertex.uv)
            .collect::<Vec<_>>();

        let lifecycle = squash_sample(0.5, canonical, canonical, canonical, anchor);
        let mut harness = GlesEffectTestHarness::new(1600, 1200);
        let sample = &lifecycle.samples[0];
        harness.renderer.lifecycle.rebuild_visual_source_commands(
            &lifecycle,
            std::slice::from_ref(&surface),
            &[],
            1.0,
            (1600, 1200),
            OutputFramebufferOrigin::BottomLeft,
        );
        let source_command = source_command_for_surface(&harness, sample.payload_id, 1);
        assert_eq!(source_command.sampling, SurfaceSampling::ExactNearest);
        let source_start = source_command.vertex_start as usize;
        let source_end = source_start + source_command.vertex_count as usize;
        let source_uvs = harness.renderer.lifecycle.visual_store.source_vertices
            [&sample.payload_id][source_start..source_end]
            .iter()
            .map(|vertex| vertex.uv)
            .collect::<Vec<_>>();
        assert_eq!(source_uvs, ordinary_uvs);

        harness.renderer.lifecycle.rebuild_squash_commands(
            &lifecycle,
            std::slice::from_ref(&surface),
            &[],
            1.0,
            (1600, 1200),
            OutputFramebufferOrigin::BottomLeft,
        );
        let command = resolved_visual_command(&harness, &lifecycle);
        assert_eq!(command.sampling, SurfaceSampling::ScaledLinear);
    }
}

#[test]
fn squash_captures_one_to_one_source_nearest_and_samples_retained_visual_linearly() {
    let source = rect(100.0, 80.0, 800.0, 600.0);
    let anchor = rect(900.0, 700.0, 56.0, 40.0);
    let lifecycle = squash_sample(0.0, source, source, source, anchor);
    let surface = lifecycle_test_surface(
        1,
        100,
        80,
        800,
        600,
        0xffee_8844,
        &mut BufferIdAllocator::default(),
    );
    let mut harness = GlesEffectTestHarness::new(1600, 1200);
    let sample = &lifecycle.samples[0];
    harness.renderer.lifecycle.rebuild_visual_source_commands(
        &lifecycle,
        std::slice::from_ref(&surface),
        &[],
        1.0,
        (1600, 1200),
        OutputFramebufferOrigin::BottomLeft,
    );
    assert_eq!(
        source_command_for_surface(&harness, sample.payload_id, 1).sampling,
        SurfaceSampling::ExactNearest
    );
    harness.renderer.lifecycle.rebuild_squash_commands(
        &lifecycle,
        std::slice::from_ref(&surface),
        &[],
        1.0,
        (1600, 1200),
        OutputFramebufferOrigin::BottomLeft,
    );

    assert_eq!(
        resolved_visual_command(&harness, &lifecycle).sampling,
        SurfaceSampling::ScaledLinear
    );
}
