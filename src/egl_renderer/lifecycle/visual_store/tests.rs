use super::*;
use crate::egl_renderer::tests::{
    GlesEffectTestHarness, lifecycle_test_lamp_sample as lamp_test_sample,
    lifecycle_transfer_assert_pixel as assert_effect_test_pixel,
    lifecycle_transfer_color as lifecycle_transfer_test_color,
    lifecycle_transfer_fill_pattern as fill_lifecycle_transfer_test_pattern,
    lifecycle_transfer_read_pixels as read_effect_texture_pixels,
    lifecycle_transfer_texture_key as lifecycle_transfer_test_texture_key, test_lifecycle_identity,
};
use oblivion_one::core::SceneNodeId;
use oblivion_one::presentation_animation::{
    AnimationTime, PresentationEngine, PresentationRect, PresentationRetainedVisualKind,
};

fn clear_test_target(harness: &mut GlesEffectTestHarness, target: &PooledEffectTexture) {
    let renderer = &mut harness.renderer;
    let mut context = LifecycleRenderContext::new(
        &harness.gl,
        &mut renderer.scene_state,
        &mut renderer.effect_runtime,
        renderer.resources.texture_view(),
    );
    clear_effect_texture(&mut context, target).expect("lifecycle capture texture clears");
}

fn copy_test_output_region(
    harness: &mut GlesEffectTestHarness,
    target: &PooledEffectTexture,
    rect: PresentationRect,
    framebuffer_origin: OutputFramebufferOrigin,
) {
    let renderer = &mut harness.renderer;
    let output = EffectFramebufferTarget::new(renderer.scene_state.active_output_framebuffer);
    let mut context = LifecycleRenderContext::new(
        &harness.gl,
        &mut renderer.scene_state,
        &mut renderer.effect_runtime,
        renderer.resources.texture_view(),
    );
    copy_framebuffer_region_to_texture(
        &mut context,
        target,
        rect,
        framebuffer_origin,
        output,
        output,
    )
    .expect("lifecycle output region captures");
}

#[test]
fn frozen_visual_signature_survives_reversal_but_changes_for_fresh_payload() {
    let first_sample = lamp_test_sample(0.5);
    let first_lamp = LifecycleFrameSnapshot::from_sample(&first_sample).samples[0];
    let first_source = &first_sample.samples[0].visual_source;
    let next_identity = test_lifecycle_identity(first_lamp.window_id, 2);
    let mut reversed_lamp = first_lamp;
    reversed_lamp.presentation_identity = next_identity;
    let reversed_source = LifecycleVisualSource {
        presentation_identity: next_identity,
        ..first_source.clone()
    };
    assert_eq!(
        lifecycle_visual_source_signature(&reversed_source, reversed_lamp, 1.0),
        lifecycle_visual_source_signature(first_source, first_lamp, 1.0),
        "owner revisions do not invalidate the same immutable payload"
    );

    let fresh_payload =
        compositor::PresentationRetainedVisualPayloadId::from_origin_identity(next_identity);
    let fresh_lamp = LifecycleFrameSample {
        presentation_identity: next_identity,
        payload_id: fresh_payload,
        ..first_lamp
    };
    let fresh_source = LifecycleVisualSource {
        presentation_identity: next_identity,
        payload_id: fresh_payload,
        ..first_source.clone()
    };
    assert_ne!(
        lifecycle_visual_source_signature(&fresh_source, fresh_lamp, 1.0),
        lifecycle_visual_source_signature(first_source, first_lamp, 1.0),
        "a new lifecycle payload cannot alias the previous resolved visual"
    );
}

#[test]
fn stale_visual_eviction_releases_only_unreferenced_payload_leases() {
    let mut harness = GlesEffectTestHarness::new(16, 16);
    let live_snapshot = lamp_test_sample(0.5);
    let live_source = live_snapshot.samples[0].visual_source.clone();
    let live_payload = live_source.payload_id;
    let mut identity_engine = PresentationEngine::enabled();
    let _first_identity = identity_engine
        .begin_retained_visual(
            SceneNodeId::from_raw(2).expect("first test scene node"),
            PresentationRetainedVisualKind::WindowLifecycle,
            AnimationTime::from_nanos(1),
        )
        .expect("first retained identity allocates");
    let stale_identity = identity_engine
        .begin_retained_visual(
            SceneNodeId::from_raw(3).expect("stale test scene node"),
            PresentationRetainedVisualKind::WindowLifecycle,
            AnimationTime::from_nanos(2),
        )
        .expect("second retained identity allocates");
    let stale_payload =
        compositor::PresentationRetainedVisualPayloadId::from_origin_identity(stale_identity);
    let texture_key = lifecycle_transfer_test_texture_key(16, 16);
    let baseline_leases = harness
        .renderer
        .effect_runtime
        .effect_resources
        .metrics()
        .checked_out_texture_count;
    let live_texture = harness
        .renderer
        .effect_runtime
        .effect_resources
        .acquire(&harness.gl, texture_key)
        .expect("live lifecycle visual texture allocates");
    let stale_texture = harness
        .renderer
        .effect_runtime
        .effect_resources
        .acquire(&harness.gl, texture_key)
        .expect("stale lifecycle visual texture allocates");
    let source_rect = live_snapshot.samples[0]
        .visual_group
        .presented_source_visual_rect;

    {
        let store = &mut harness.renderer.lifecycle.visual_store;
        store.source_vertices.insert(live_payload, Vec::new());
        store.source_vertices.insert(stale_payload, Vec::new());
        store.source_commands.insert(live_payload, Vec::new());
        store.source_commands.insert(stale_payload, Vec::new());
        store.resolved_visual_resources.insert(
            live_payload,
            LifecycleResolvedVisualResource {
                texture: live_texture,
                source_signature: 1,
                source_visual_rect: source_rect,
            },
        );
        store.resolved_visual_resources.insert(
            stale_payload,
            LifecycleResolvedVisualResource {
                texture: stale_texture,
                source_signature: 2,
                source_visual_rect: source_rect,
            },
        );
        store.begin_frame(&live_snapshot);
        store.release_stale(&mut harness.renderer.effect_runtime);

        assert!(store.resolved_visual_resources.contains_key(&live_payload));
        assert!(!store.resolved_visual_resources.contains_key(&stale_payload));
        assert!(store.source_vertices.contains_key(&live_payload));
        assert!(!store.source_vertices.contains_key(&stale_payload));
        assert!(store.source_commands.contains_key(&live_payload));
        assert!(!store.source_commands.contains_key(&stale_payload));
    }

    assert_eq!(
        harness
            .renderer
            .effect_runtime
            .effect_resources
            .metrics()
            .checked_out_texture_count,
        baseline_leases + 1,
        "eviction returns only the stale retained visual lease"
    );
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
fn capture_snapshot_restores_transient_lifecycle_state_without_replacing_caches_or_leases() {
    let mut harness = GlesEffectTestHarness::new(8, 6);
    let lifecycle = lamp_test_sample(0.5);
    let sample = LifecycleFrameSnapshot::from_sample(&lifecycle).samples[0];
    let payload_id = sample.payload_id;
    let source = lifecycle.samples[0].visual_source.clone();
    let source_rect = sample.visual_group.presented_source_visual_rect;
    let texture = harness
        .renderer
        .effect_runtime
        .effect_resources
        .acquire(&harness.gl, lifecycle_transfer_test_texture_key(8, 6))
        .expect("persistent lifecycle texture allocates");
    let texture_id = texture.id;

    let owner = &mut harness.renderer.lifecycle;
    owner.begin_frame(&lifecycle);
    owner.visual_store.source_vertices.insert(
        payload_id,
        vec![EglTexturedVertex {
            position: [0.25, -0.5],
            uv: [0.75, 0.125],
        }],
    );
    owner
        .visual_store
        .source_commands
        .insert(payload_id, Vec::new());
    owner.visual_store.resolved_visual_resources.insert(
        payload_id,
        LifecycleResolvedVisualResource {
            texture,
            source_signature: 17,
            source_visual_rect: source_rect,
        },
    );
    owner
        .frame
        .evidence
        .consumed
        .push(LifecycleRenderEvidenceEntry {
            window_id: sample.window_id,
            root_surface_id: sample.root_surface_id,
            presentation_identity: sample.presentation_identity,
            payload_id,
        });
    let source_vertices_address = &owner.visual_store.source_vertices as *const _;
    let source_commands_address = &owner.visual_store.source_commands as *const _;
    let resources_address = &owner.visual_store.resolved_visual_resources as *const _;
    let resource_address =
        &owner.visual_store.resolved_visual_resources[&payload_id].texture as *const _;
    let original_vertex = owner.visual_store.source_vertices[&payload_id][0];
    let snapshot = owner.capture_snapshot();

    owner.frame.samples.clear();
    owner.frame.evidence.consumed.clear();
    owner.frame.fallbacks.failed.clear();
    owner.visual_store.visual_sources.clear();
    owner.restore_capture_snapshot(snapshot);

    assert_eq!(owner.frame.samples, vec![sample]);
    assert_eq!(owner.frame.evidence.consumed.len(), 1);
    assert!(owner.frame.fallbacks.failed.is_empty());
    assert_eq!(
        owner.visual_store.visual_sources[&sample.presentation_identity],
        source
    );
    let restored_vertex = owner.visual_store.source_vertices[&payload_id][0];
    assert_eq!(restored_vertex.position, original_vertex.position);
    assert_eq!(restored_vertex.uv, original_vertex.uv);
    assert!(owner.visual_store.source_commands[&payload_id].is_empty());
    assert_eq!(
        &owner.visual_store.source_vertices as *const _,
        source_vertices_address
    );
    assert_eq!(
        &owner.visual_store.source_commands as *const _,
        source_commands_address
    );
    assert_eq!(
        &owner.visual_store.resolved_visual_resources as *const _,
        resources_address
    );
    assert_eq!(
        &owner.visual_store.resolved_visual_resources[&payload_id].texture as *const _,
        resource_address
    );
    assert_eq!(
        owner.visual_store.resolved_visual_resources[&payload_id]
            .texture
            .id,
        texture_id
    );

    let resource = owner
        .visual_store
        .resolved_visual_resources
        .remove(&payload_id)
        .expect("lifecycle lease remains owned");
    harness
        .renderer
        .effect_runtime
        .effect_resources
        .release(resource.texture)
        .expect("lifecycle lease releases through the authoritative pool");
}

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
        clear_test_target(&mut harness, &target);
        copy_test_output_region(&mut harness, &target, rect, framebuffer_origin);
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
            clear_test_target(&mut harness, &target);
            copy_test_output_region(&mut harness, &target, rect, framebuffer_origin);

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
