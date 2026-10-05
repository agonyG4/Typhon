use super::*;

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
    harness
        .renderer
        .rebuild_squash_commands(&lifecycle, &[], &[], 1.0);

    assert_eq!(harness.renderer.scene_state.squash_vertices.len(), 6);
    assert_eq!(harness.renderer.scene_state.squash_commands.len(), 1);
    assert!(matches!(
        harness.renderer.scene_state.squash_commands[0]
            .command
            .layer,
        EglDrawLayer::LifecycleResolvedVisual(_)
    ));
    assert!(harness.renderer.scene_state.lamp_vertices.is_empty());
    assert!(harness.renderer.scene_state.lamp_commands.is_empty());
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
    harness.renderer.lifecycle_samples = LifecycleFrameSnapshot::from_sample(&lifecycle).samples;
    harness.renderer.rebuild_squash_commands(
        &lifecycle,
        std::slice::from_ref(&retained_surface),
        &[],
        1.0,
    );

    harness
        .renderer
        .draw_squash_overlay(None)
        .expect("missing retained texture is a recoverable lifecycle fallback");
    assert_eq!(harness.renderer.lifecycle_render_fallbacks.failed.len(), 1);
    assert_eq!(
        harness.renderer.lifecycle_render_fallbacks.failed[0].effect,
        LifecycleEffectKind::Squash
    );
    assert!(
        harness
            .renderer
            .lifecycle_render_evidence
            .consumed
            .is_empty()
    );
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
