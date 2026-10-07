use super::*;
use crate::egl_renderer::tests::{
    GlesEffectTestHarness, lifecycle_test_lamp_sample as lamp_test_sample,
    lifecycle_test_surface_fixture as lifecycle_test_surface,
};
use oblivion_one::presentation_animation::PresentationRect;
use oblivion_one::render_backend::buffer::BufferIdAllocator;

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

fn command_for_surface(harness: &GlesEffectTestHarness, surface_id: u32) -> &EglDrawCommand {
    &harness
        .renderer
        .lifecycle
        .squash
        .commands
        .iter()
        .find(|entry| matches!(entry.command.layer, EglDrawLayer::Surface(id) if id == surface_id))
        .expect("Squash surface command")
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

    harness
        .renderer
        .draw_squash_overlay(None)
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
        harness.renderer.lifecycle.rebuild_squash_commands(
            &lifecycle,
            std::slice::from_ref(&surface),
            &[],
            1.0,
            (1600, 1200),
            origin,
        );

        assert_rect_close(command_for_surface(&harness, 1).bounds, presented);
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
    let canonical_subsurface = rect(260.0, 200.0, 200.0, 100.0);

    for progress in [0.0, 0.5] {
        let lifecycle = squash_sample(progress, canonical, canonical, presented, anchor);
        let mut harness = GlesEffectTestHarness::new(1600, 1200);
        harness.renderer.lifecycle.rebuild_squash_commands(
            &lifecycle,
            &surfaces,
            &[],
            1.0,
            (1600, 1200),
            OutputFramebufferOrigin::BottomLeft,
        );

        assert_rect_close(
            command_for_surface(&harness, 2).bounds,
            transformed_from_canonical(
                lifecycle.samples[0].visual_group,
                canonical_subsurface,
                progress,
            ),
        );
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
        harness.renderer.lifecycle.rebuild_squash_commands(
            &lifecycle,
            std::slice::from_ref(&surface),
            &[],
            1.0,
            (1600, 1200),
            OutputFramebufferOrigin::BottomLeft,
        );
        let command = command_for_surface(&harness, 1);
        assert_eq!(command.sampling, SurfaceSampling::ScaledLinear);
        assert_eq!(
            harness
                .renderer
                .lifecycle
                .squash
                .vertices
                .iter()
                .map(|vertex| vertex.uv)
                .collect::<Vec<_>>(),
            ordinary_uvs
        );
    }
}

#[test]
fn squash_keeps_exact_nearest_for_a_pixel_aligned_one_to_one_source_endpoint() {
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
    harness.renderer.lifecycle.rebuild_squash_commands(
        &lifecycle,
        std::slice::from_ref(&surface),
        &[],
        1.0,
        (1600, 1200),
        OutputFramebufferOrigin::BottomLeft,
    );

    assert_eq!(
        command_for_surface(&harness, 1).sampling,
        SurfaceSampling::ExactNearest
    );
}
