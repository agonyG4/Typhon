use super::*;
use crate::egl_renderer::lifecycle::LifecycleRenderTargetSpace;
use crate::egl_renderer::tests::{
    GlesEffectTestHarness, lifecycle_test_lamp_sample as lamp_test_sample,
    lifecycle_transfer_assert_pixel as assert_effect_test_pixel,
    lifecycle_transfer_color as lifecycle_transfer_test_color,
    lifecycle_transfer_fill_pattern as fill_lifecycle_transfer_test_pattern,
    lifecycle_transfer_read_pixels as read_effect_texture_pixels,
    lifecycle_transfer_texture_key as lifecycle_transfer_test_texture_key,
    test_lifecycle_identity_sequence,
};
use oblivion_one::core::SceneNodeId;
use oblivion_one::presentation_animation::{
    AnimationTime, PresentationEngine, PresentationRect, PresentationRetainedVisualKind,
};
use oblivion_one::render_backend::buffer::BufferIdAllocator;
use oblivion_one::window_lifecycle_animation::LifecycleVisualGroup;

fn clear_test_target(harness: &mut GlesEffectTestHarness, target: &PooledEffectTexture) {
    let renderer = &mut harness.renderer;
    let target_space = LifecycleRenderTargetSpace::output(
        renderer.scene_state.current_size,
        renderer.scene_state.current_framebuffer_origin,
    );
    let mut context = LifecycleRenderContext::new(
        &harness.gl,
        &mut renderer.scene_state,
        &mut renderer.effect_runtime,
        renderer.resources.texture_view(),
        target_space,
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
    let target_space =
        LifecycleRenderTargetSpace::output(renderer.scene_state.current_size, framebuffer_origin);
    let mut context = LifecycleRenderContext::new(
        &harness.gl,
        &mut renderer.scene_state,
        &mut renderer.effect_runtime,
        renderer.resources.texture_view(),
        target_space,
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
    let next_identity = test_lifecycle_identity_sequence(first_lamp.window_id, 2)[1];
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
fn retained_cascaded_source_commands_keep_admission_origin_and_clip_after_isolation() {
    let mut buffer_ids = BufferIdAllocator::default();
    let mut first = crate::egl_renderer::tests::lifecycle_test_surface_fixture(
        8,
        0,
        0,
        8,
        8,
        0xff10_2030,
        &mut buffer_ids,
    );
    first.placement = compositor::SurfacePlacement::absolute_root_at(5, 7);
    let mut second = crate::egl_renderer::tests::lifecycle_test_surface_fixture(
        9,
        0,
        0,
        8,
        8,
        0xff20_3040,
        &mut buffer_ids,
    );
    second.placement = compositor::SurfacePlacement::absolute_root_at(25, 27);
    let mut target = crate::egl_renderer::tests::lifecycle_test_surface_fixture(
        1,
        2,
        3,
        120,
        90,
        0xff30_4050,
        &mut buffer_ids,
    );
    target.placement = compositor::SurfacePlacement::root_at(11, 13);
    let mut child = crate::egl_renderer::tests::lifecycle_test_surface_fixture(
        2,
        4,
        6,
        60,
        40,
        0xff40_5060,
        &mut buffer_ids,
    );
    child.placement = compositor::SurfacePlacement::subsurface(1, 23, 29);
    child.visual_clip = Some(compositor::SurfaceVisualAperture::logical_only(
        compositor::SurfaceTargetRect::new(4, 5, 24, 16),
    ));

    let full_scene = vec![first, second, target.clone(), child.clone()];
    let retained = vec![target.clone(), child.clone()];
    let presentation_generations = [(8, 1), (9, 1), (1, 1), (2, 1)].into();
    let layout = std::sync::Arc::new(
        compositor::FrozenLifecycleRenderLayout::capture(
            &full_scene,
            &retained,
            &presentation_generations,
        )
        .expect("full scene supplies exact retained render layout"),
    );
    let full_assignments = compositor::surface_render_space_assignments(&full_scene, 1.0);
    let expected_root = full_assignments[2].clone();
    let expected_child = full_assignments[3].clone();
    assert_eq!(
        (expected_root.target.x(), expected_root.target.y()),
        (149, 152)
    );

    let mut lifecycle = lamp_test_sample(0.5);
    let root_rect = PresentationRect::new(
        f64::from(expected_root.target.x()),
        f64::from(expected_root.target.y()),
        f64::from(expected_root.target.width()),
        f64::from(expected_root.target.height()),
    )
    .expect("root source rectangle");
    let anchor = PresentationRect::new(300.0, 300.0, 32.0, 32.0).expect("anchor");
    let visual_group =
        LifecycleVisualGroup::from_bounds(root_rect, root_rect, root_rect, anchor, 400, 400)
            .expect("lifecycle source group");
    lifecycle.samples[0].visual_group = visual_group;
    lifecycle.samples[0].visual_source.render_layout = Some(layout);

    let mut store = LifecycleVisualStore::default();
    store.rebuild_source_commands(
        &lifecycle,
        &retained,
        &[],
        1.0,
        (400, 400),
        OutputFramebufferOrigin::BottomLeft,
    );
    let commands = store
        .source_commands
        .get(&lifecycle.samples[0].payload_id)
        .expect("source commands captured");
    let root_command = commands
        .iter()
        .find(|command| command.layer == EglDrawLayer::Surface(1))
        .expect("root surface command");
    assert_eq!(
        root_command.bounds,
        EglRect::new(149.0, 152.0, 120.0, 90.0),
        "retained capture must use the full-scene CascadedWindow origin"
    );
    let expected_child_plans = compositor::surface_render_plans_with_aperture(
        &child,
        expected_child.target,
        expected_child.visual_clip.as_ref(),
    );
    let child_commands = commands
        .iter()
        .filter(|command| command.layer == EglDrawLayer::Surface(2))
        .collect::<Vec<_>>();
    assert_eq!(child_commands.len(), expected_child_plans.len());
    for (command, plan) in child_commands.into_iter().zip(expected_child_plans) {
        assert_eq!(
            command.bounds,
            EglRect::new(
                plan.content_target.x() as f32,
                plan.content_target.y() as f32,
                plan.content_target.width() as f32,
                plan.content_target.height() as f32,
            ),
            "retained child clipping must match the admission-time scene"
        );
    }
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
            },
        );
        store.resolved_visual_resources.insert(
            stale_payload,
            LifecycleResolvedVisualResource {
                texture: stale_texture,
                source_signature: 2,
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
fn minimized_snapshot_survives_idle_frames_and_is_reused_by_restore_payload() {
    let mut harness = GlesEffectTestHarness::new(16, 16);
    let minimized = lamp_test_sample(0.5);
    let minimized_sample = &minimized.samples[0];
    let source_resource_id = minimized_sample.visual_source.resource_payload_id;
    let restore_identity = test_lifecycle_identity_sequence(minimized_sample.window_id, 2)[1];
    let restore_payload_id =
        compositor::PresentationRetainedVisualPayloadId::from_origin_identity(restore_identity);
    let second_snapshot_id = compositor::PresentationRetainedVisualPayloadId::from_origin_identity(
        test_lifecycle_identity_sequence(minimized_sample.window_id, 3)[2],
    );
    let mut restore = minimized.clone();
    restore.samples[0].presentation_identity = restore_identity;
    restore.samples[0].payload_id = restore_payload_id;
    restore.samples[0].direction = LifecycleDirection::Restore;
    restore.samples[0].visual_source.presentation_identity = restore_identity;
    restore.samples[0].visual_source.payload_id = restore_payload_id;
    restore.samples[0].visual_source.resource_payload_id = source_resource_id;
    restore.minimized_visual_resource_payload_ids = vec![second_snapshot_id];

    let texture_key = lifecycle_transfer_test_texture_key(16, 16);
    let baseline_leases = harness
        .renderer
        .effect_runtime
        .effect_resources
        .metrics()
        .checked_out_texture_count;
    let texture = harness
        .renderer
        .effect_runtime
        .effect_resources
        .acquire(&harness.gl, texture_key)
        .expect("minimize capture texture allocates");
    let texture_id = texture.id;
    let second_texture = harness
        .renderer
        .effect_runtime
        .effect_resources
        .acquire(&harness.gl, texture_key)
        .expect("second minimized snapshot texture allocates");
    {
        let renderer = &mut harness.renderer;
        let store = &mut renderer.lifecycle.visual_store;
        store.resolved_visual_resources.insert(
            source_resource_id,
            LifecycleResolvedVisualResource {
                texture,
                source_signature: 1,
            },
        );
        store.resolved_visual_resources.insert(
            second_snapshot_id,
            LifecycleResolvedVisualResource {
                texture: second_texture,
                source_signature: 2,
            },
        );

        let idle_minimized = LifecycleSceneSample {
            sampled_at: AnimationTime::from_nanos(0),
            samples: Vec::new(),
            minimized_visual_resource_payload_ids: vec![source_resource_id, second_snapshot_id],
        };
        for _ in 0..3 {
            store.begin_frame(&idle_minimized);
            store.release_stale(&mut renderer.effect_runtime);
            assert!(
                store
                    .resolved_visual_resources
                    .contains_key(&source_resource_id)
            );
        }

        store.begin_frame(&restore);
        store.rebuild_source_commands(
            &restore,
            &[],
            &[],
            1.0,
            (16, 16),
            OutputFramebufferOrigin::BottomLeft,
        );
        assert!(!store.source_commands.contains_key(&restore_payload_id));
        store.release_stale(&mut renderer.effect_runtime);
        assert_eq!(store.resolved_visual_resources.len(), 2);
        assert_eq!(
            store.resolved_visual_resources[&source_resource_id]
                .texture
                .id,
            texture_id,
            "fresh restore owns a new lifecycle identity but samples the existing minimize texture"
        );
        assert!(
            store
                .resolved_visual_resources
                .contains_key(&second_snapshot_id)
        );

        let mut frame = LifecycleFrameState::default();
        frame.begin(&restore);
        let target_space = LifecycleRenderTargetSpace::output(
            renderer.scene_state.current_size,
            OutputFramebufferOrigin::BottomLeft,
        );
        let mut context = LifecycleRenderContext::new(
            &harness.gl,
            &mut renderer.scene_state,
            &mut renderer.effect_runtime,
            renderer.resources.texture_view(),
            target_space,
        );
        store
            .prepare_lifecycle_visual_sources(
                &mut frame,
                &mut context,
                &RepaintPlan::default(),
                OutputFramebufferOrigin::BottomLeft,
            )
            .expect("restore resolves the already-owned minimized texture");
        drop(context);
        assert!(frame.fallbacks.failed.is_empty());
        assert!(store.is_ready(source_resource_id, &renderer.effect_runtime));
        assert!(!store.captured_payloads.contains(&restore_payload_id));
        assert_eq!(
            renderer
                .effect_runtime
                .effect_resources
                .metrics()
                .checked_out_texture_count,
            baseline_leases + 2,
            "restore does not allocate a second resolved visual texture"
        );
    }

    let store = &mut harness.renderer.lifecycle.visual_store;
    store.begin_frame(&LifecycleSceneSample {
        sampled_at: AnimationTime::from_nanos(0),
        samples: Vec::new(),
        minimized_visual_resource_payload_ids: vec![second_snapshot_id],
    });
    store.release_stale(&mut harness.renderer.effect_runtime);
    assert!(
        !store
            .resolved_visual_resources
            .contains_key(&source_resource_id)
    );
    assert!(
        store
            .resolved_visual_resources
            .contains_key(&second_snapshot_id)
    );

    store.begin_frame(&LifecycleSceneSample {
        sampled_at: AnimationTime::from_nanos(0),
        samples: Vec::new(),
        minimized_visual_resource_payload_ids: Vec::new(),
    });
    store.release_stale(&mut harness.renderer.effect_runtime);
    assert!(store.resolved_visual_resources.is_empty());
    assert_eq!(
        harness
            .renderer
            .effect_runtime
            .effect_resources
            .metrics()
            .checked_out_texture_count,
        baseline_leases,
        "the texture returns to the pool after minimized and restore references retire"
    );
}

#[test]
fn missing_minimized_snapshot_skips_animation_source_without_recapture() {
    let mut harness = GlesEffectTestHarness::new(16, 16);
    let mut restore = lamp_test_sample(0.5);
    let minimize_payload_id = restore.samples[0].visual_source.resource_payload_id;
    let restore_identity = test_lifecycle_identity_sequence(restore.samples[0].window_id, 2)[1];
    let restore_payload_id =
        compositor::PresentationRetainedVisualPayloadId::from_origin_identity(restore_identity);
    restore.samples[0].presentation_identity = restore_identity;
    restore.samples[0].payload_id = restore_payload_id;
    restore.samples[0].direction = LifecycleDirection::Restore;
    restore.samples[0].visual_source.presentation_identity = restore_identity;
    restore.samples[0].visual_source.payload_id = restore_payload_id;
    restore.samples[0].visual_source.resource_payload_id = minimize_payload_id;
    restore.minimized_visual_resource_payload_ids.clear();

    let mut frame = LifecycleFrameState::default();
    frame.begin(&restore);
    let renderer = &mut harness.renderer;
    let store = &mut renderer.lifecycle.visual_store;
    store.begin_frame(&restore);
    let target_space = LifecycleRenderTargetSpace::output(
        renderer.scene_state.current_size,
        OutputFramebufferOrigin::BottomLeft,
    );
    let mut context = LifecycleRenderContext::new(
        &harness.gl,
        &mut renderer.scene_state,
        &mut renderer.effect_runtime,
        renderer.resources.texture_view(),
        target_space,
    );
    store
        .prepare_lifecycle_visual_sources(
            &mut frame,
            &mut context,
            &RepaintPlan::default(),
            OutputFramebufferOrigin::BottomLeft,
        )
        .expect("missing optional snapshot is an animation fallback");
    drop(context);

    assert_eq!(frame.fallbacks.failed.len(), 1);
    assert_eq!(
        frame.fallbacks.failed[0].reason,
        LifecycleRenderFallbackReason::MinimizedSnapshotUnavailable
    );
    assert!(store.resolved_visual_resources.is_empty());
    assert!(!store.captured_payloads.contains(&restore_payload_id));
    assert!(store.source_commands.get(&restore_payload_id).is_none());
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
