use super::*;
use crate::compositor::DecorationRenderInstance;
use crate::compositor::decoration::types::DecorationPreference;
use crate::presentation_animation::{
    PresentationRetainedVisualIdentity, PresentationRetainedVisualKind,
    PresentationTransactionMemberKind,
};
use crate::render_backend::buffer::{BufferIdAllocator, BufferSize, CommittedSurfaceBuffer};
use crate::window_lifecycle_animation::{
    LampWindowSample, LifecycleFrameSnapshot, LifecycleRenderEvidence,
    LifecycleRenderEvidenceEntry, LifecycleRenderFallbackEntry, LifecycleRenderFallbackReason,
    LifecycleTransitionRequest, LifecycleVisualGroup,
};

fn lifecycle_decoration(
    window_id: WindowId,
    root_surface_id: u32,
    visual_signature: u8,
) -> DecorationRenderInstance {
    DecorationRenderInstance::test_solid(
        window_id,
        root_surface_id,
        0,
        0,
        832,
        640,
        [visual_signature, 0, 0, 255],
    )
}

fn visual_group(visual_rect: PresentationRect) -> LifecycleVisualGroup {
    LifecycleVisualGroup::from_bounds(
        rect(400.0, 100.0, 800.0, 600.0),
        visual_rect,
        rect(200.0, 160.0, 960.0, 720.0),
        rect(1500.0, 500.0, 64.0, 64.0),
        1920,
        1080,
    )
    .expect("valid test visual group")
}

fn ssd_test_surface(surface_id: u32) -> RenderableSurface {
    let buffer_id = BufferIdAllocator::default()
        .allocate()
        .expect("test buffer identity");
    RenderableSurface {
        surface_id,
        x: 0,
        y: 0,
        width: 300,
        height: 200,
        placement: SurfacePlacement::root(),
        render_backend: SurfaceRenderBackend::NativeWayland,
        render_placement: None,
        visual_clip: None,
        render_target_size: None,
        generation: 1,
        commit_sequence: SurfaceCommitSequence::initial(),
        buffer: CommittedSurfaceBuffer::shm_snapshot(
            buffer_id,
            BufferSize::new(300, 200).expect("test buffer size"),
            vec![0xff12_3456; 300 * 200],
        ),
        viewport_source: None,
        viewport_destination: None,
        buffer_scale: 1,
        buffer_transform: wayland_server::protocol::wl_output::Transform::Normal,
        damage: RenderableSurfaceDamage::Full,
    }
}

fn ssd_test_state(surface_id: u32) -> (CompositorState, WindowId) {
    let mut state = CompositorState::new(None);
    let window_id = state.allocate_window_id().expect("test window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(window_id, surface_id))
        .expect("test XDG window");
    let mut decoration_state = WindowDecorationState::new();
    decoration_state.set_preference(DecorationPreference::ServerSide);
    decoration_state.apply_configured_mode(DecorationMode::ServerSide);
    state
        .xdg_decoration_states
        .insert(surface_id, decoration_state);
    state.append_renderable_surface(ssd_test_surface(surface_id));
    state.rebuild_active_scene_view();
    (state, window_id)
}

fn lifecycle_decoration_signature(
    state: &CompositorState,
    root_surface_id: u32,
    surfaces: &[RenderableSurface],
    at: AnimationTime,
) -> Option<u64> {
    let sample = state.lifecycle_scene_sample_at(at);
    state
        .lifecycle_decoration_render_instances(&sample, surfaces)
        .into_iter()
        .find(|decoration| decoration.root_surface_id() == root_surface_id)
        .map(|decoration| decoration.scene_snapshot().visual_signature())
}

fn settle_lifecycle_transition(state: &mut CompositorState, window_id: WindowId) {
    let now = AnimationTime::monotonic_now().expect("monotonic test time");
    let scene_node_id =
        crate::core::SceneNodeId::from_raw(window_id.get()).expect("test SceneNodeId");
    let active = state
        .window_lifecycle_animator
        .sample(scene_node_id, now)
        .expect("active lifecycle transition");
    assert!(
        state
            .window_lifecycle_animator
            .snap_to_endpoint(active.presentation_identity, now,)
    );
    let sample = state.lifecycle_scene_sample_at(now);
    let evidence = LifecycleRenderEvidence::from_consumed([LifecycleRenderEvidenceEntry {
        window_id,
        root_surface_id: active.root_surface_id,
        presentation_identity: active.presentation_identity,
    }]);
    let snapshot = LifecycleFrameSnapshot::qualified_from_sample(&sample, &evidence);
    state.publish_presented_lifecycle(1, &snapshot);
}

fn lifecycle_request(
    presentation_engine: &mut crate::presentation_animation::PresentationEngine,
    window_id: WindowId,
    root_surface_id: u32,
    source_rect: PresentationRect,
    anchor_rect: PresentationRect,
    direction: LifecycleDirection,
) -> LifecycleTransitionRequest {
    let visual_group = LifecycleVisualGroup::from_bounds(
        source_rect,
        source_rect,
        source_rect,
        anchor_rect,
        1920,
        1080,
    )
    .expect("valid test visual group");
    lifecycle_request_with_group(
        presentation_engine,
        window_id,
        root_surface_id,
        visual_group,
        direction,
    )
}

fn lifecycle_request_with_group(
    presentation_engine: &mut crate::presentation_animation::PresentationEngine,
    window_id: WindowId,
    root_surface_id: u32,
    visual_group: LifecycleVisualGroup,
    direction: LifecycleDirection,
) -> LifecycleTransitionRequest {
    let scene_node_id =
        crate::core::SceneNodeId::from_raw(window_id.get()).expect("test SceneNodeId");
    let presentation_identity = presentation_engine
        .begin_retained_visual(
            scene_node_id,
            crate::presentation_animation::PresentationRetainedVisualKind::WindowLifecycle,
            AnimationTime::from_nanos(0),
        )
        .expect("test retained identity");
    LifecycleTransitionRequest {
        presentation_identity,
        window_id,
        root_surface_id,
        visual_group,
        direction,
        resolved_effect_scene: ResolvedEffectScene::default(),
    }
}

#[test]
fn xwayland_backing_replacement_preserves_frozen_lifecycle_identity_and_root() {
    let mut state = CompositorState::new(None);
    state.lifecycle_animation_renderer_available = Some(true);
    let generation = crate::xwayland::XwaylandGeneration::new(
        std::num::NonZeroU64::new(38).expect("generation"),
    );
    let root_a = 381;
    let root_b = 382;
    let snapshot = super::super::desktop_window_tests::x11_snapshot(generation, 3_801, root_a);
    let handle = snapshot.handle;
    let window_id = super::super::desktop_window_tests::insert_x11(&mut state, snapshot);
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("WindowGroup scene node");
    let source = rect(100.0, 80.0, 800.0, 600.0);
    let anchor = rect(1200.0, 900.0, 64.0, 64.0);
    let visual_group =
        LifecycleVisualGroup::from_bounds(source, source, source, anchor, 1920, 1080)
            .expect("lifecycle visual group");

    state.begin_lifecycle_minimize(
        window_id,
        root_a,
        Some(source),
        Some(source),
        Some(visual_group),
        ResolvedEffectScene::default(),
        Vec::new(),
    );
    let identity = state
        .window_lifecycle_animator
        .identity(scene_node_id)
        .expect("active lifecycle identity");
    assert_eq!(identity.scene_node_id(), scene_node_id);
    assert_eq!(
        identity.kind(),
        PresentationRetainedVisualKind::WindowLifecycle
    );
    assert_eq!(
        state
            .presentation_animator
            .transaction_record(identity.transaction_id())
            .expect("retained lifecycle transaction")
            .members()[0]
            .kind(),
        PresentationTransactionMemberKind::RetainedVisual(
            PresentationRetainedVisualKind::WindowLifecycle
        )
    );

    let frozen = LifecycleFrameSnapshot::from_sample(
        &state.lifecycle_scene_sample_at(AnimationTime::monotonic_now().expect("monotonic time")),
    );
    assert_eq!(state.attach_x11_surface(handle, root_b), Ok(Some(root_a)));
    assert_eq!(
        state.scene_node_id_for_window_group(window_id),
        Some(scene_node_id)
    );
    assert_eq!(
        state
            .window(window_id)
            .expect("replacement XWayland window")
            .root_surface_id,
        root_b
    );
    let active = state
        .window_lifecycle_animator
        .sample(
            scene_node_id,
            AnimationTime::monotonic_now().expect("monotonic time"),
        )
        .expect("frozen lifecycle transition remains active");
    assert_eq!(active.presentation_identity, identity);
    assert_eq!(active.root_surface_id, root_a);
    assert_eq!(frozen.lamps[0].presentation_identity, identity);
    assert_eq!(frozen.lamps[0].root_surface_id, root_a);
    let current_sample = state.lifecycle_scene_sample_at(AnimationTime::monotonic_now().unwrap());
    let qualified = LifecycleFrameSnapshot::qualified_from_sample(
        &current_sample,
        &LifecycleRenderEvidence::from_consumed([LifecycleRenderEvidenceEntry {
            window_id,
            root_surface_id: root_a,
            presentation_identity: identity,
        }]),
    );
    assert_eq!(qualified.lamps.len(), 1);
    assert_eq!(qualified.lamps[0].root_surface_id, root_a);
    assert_eq!(qualified.lamps[0].presentation_identity, identity);

    state.lifecycle_render_suppressed_roots.insert(root_a);
    state.lifecycle_cancel_window(window_id);
    assert!(!state.lifecycle_render_suppressed_roots.contains(&root_a));
    assert!(
        state
            .presentation_animator
            .transaction_record(identity.transaction_id())
            .is_none()
    );
}

fn commit_geometry_track(state: &mut CompositorState, scene_node_id: crate::core::SceneNodeId) {
    let now = AnimationTime::monotonic_now().expect("monotonic time");
    state
        .presentation_animator
        .commit(
            crate::presentation_animation::PresentationTransactionRequest::geometry(
                now,
                vec![
                    crate::presentation_animation::PresentationGeometryMutation::new(
                        scene_node_id,
                        rect(0.0, 0.0, 80.0, 60.0),
                        rect(10.0, 0.0, 80.0, 60.0),
                        crate::presentation_animation::AnimationCurve::easing(
                            std::time::Duration::from_millis(200),
                            crate::presentation_animation::EasingCurve::Linear,
                        ),
                    ),
                ],
            ),
        )
        .expect("active Geometry transaction");
}

fn exhaust_identity_allocator(
    state: &mut CompositorState,
    scene_node_id: crate::core::SceneNodeId,
    transaction_ids: bool,
) {
    state.presentation_animator.set_next_ids_for_test(
        if transaction_ids {
            std::num::NonZeroU64::MAX
        } else {
            std::num::NonZeroU64::new(700).expect("nonzero test ID")
        },
        if transaction_ids {
            std::num::NonZeroU64::new(700).expect("nonzero test ID")
        } else {
            std::num::NonZeroU64::MAX
        },
    );
    let marker = state
        .presentation_animator
        .begin_retained_visual(
            scene_node_id,
            crate::presentation_animation::PresentationRetainedVisualKind::WindowLifecycle,
            AnimationTime::from_nanos(1),
        )
        .expect("allocate final identity in selected namespace");
    assert!(
        state
            .presentation_animator
            .retire_retained_visual_exact(marker)
    );
}

fn historical_identity(window_id: WindowId, raw: u64) -> PresentationRetainedVisualIdentity {
    PresentationRetainedVisualIdentity::new(
        crate::core::SceneNodeId::from_raw(window_id.get()).expect("test SceneNodeId"),
        crate::presentation_animation::PresentationRetainedVisualKind::WindowLifecycle,
        crate::presentation_animation::PresentationTransactionId::from_raw(raw)
            .expect("test transaction ID"),
        crate::presentation_animation::PresentationRevisionId::from_raw(raw)
            .expect("test revision ID"),
    )
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> PresentationRect {
    PresentationRect::new(x, y, width, height).expect("valid test rectangle")
}

#[test]
fn unconsumed_endpoint_pageflip_does_not_retire_lifecycle_transition() {
    let window_id = WindowId::from_raw(301).expect("valid window ID");
    let mut state = CompositorState {
        lifecycle_animation_renderer_available: Some(true),
        ..Default::default()
    };
    let source = rect(20.0, 20.0, 200.0, 150.0);
    let anchor = rect(500.0, 500.0, 40.0, 40.0);
    let transition_id = state
        .window_lifecycle_animator
        .start_or_reverse(
            lifecycle_request(
                &mut state.presentation_animator,
                window_id,
                301,
                source,
                anchor,
                LifecycleDirection::Minimize,
            ),
            AnimationTime::from_nanos(0),
            1.0,
        )
        .expect("Lamp transition starts");
    let endpoint = state
        .window_lifecycle_animator
        .sample_scene(AnimationTime::from_nanos(280_000_000));
    assert_eq!(state.presentation_animator.transaction_count(), 1);

    state.publish_presented_lifecycle(1, &LifecycleFrameSnapshot::default());
    assert_eq!(state.window_lifecycle_animator.active_count(), 1);

    let evidence = LifecycleRenderEvidence::from_consumed([LifecycleRenderEvidenceEntry {
        window_id,
        root_surface_id: 301,
        presentation_identity: transition_id,
    }]);
    let qualified = LifecycleFrameSnapshot::qualified_from_sample(&endpoint, &evidence);
    state.publish_presented_lifecycle(2, &qualified);
    assert_eq!(state.window_lifecycle_animator.active_count(), 0);
    assert_eq!(state.presentation_animator.transaction_count(), 0);
}

#[test]
fn off_output_transition_settles_without_pageflip_and_does_not_block_scheduler() {
    let window_id = WindowId::from_raw(302).expect("valid window ID");
    let mut state = CompositorState {
        output_size: OutputSize::new(100, 100),
        lifecycle_animation_renderer_available: Some(true),
        ..Default::default()
    };
    let transition_id = state
        .window_lifecycle_animator
        .start_or_reverse(
            lifecycle_request(
                &mut state.presentation_animator,
                window_id,
                302,
                rect(300.0, 300.0, 100.0, 100.0),
                rect(500.0, 500.0, 20.0, 20.0),
                LifecycleDirection::Restore,
            ),
            AnimationTime::from_nanos(0),
            1.0,
        )
        .expect("Lamp transition starts");
    state.lifecycle_render_suppressed_roots.insert(302);
    assert!(!state.lifecycle_animation_has_pending_visible());
    assert!(state.settle_lifecycle_no_visual_change());
    assert_eq!(state.window_lifecycle_animator.active_count(), 0);
    assert_eq!(state.presentation_animator.transaction_count(), 0);
    assert_eq!(state.presented_lifecycle_frame_id(), 0);
    assert!(!state.lifecycle_render_suppressed_roots.contains(&302));
    assert!(!state.lifecycle_animation_has_pending_visible());
    assert!(
        state
            .window_lifecycle_animator
            .acknowledge(transition_id, true)
            .is_none()
    );
}

#[test]
fn exact_invisible_lamp_endpoint_can_settle_without_visual_change() {
    let window_id = WindowId::from_raw(309).expect("valid window ID");
    let mut state = CompositorState {
        output_size: OutputSize::new(100, 100),
        lifecycle_animation_renderer_available: Some(true),
        ..Default::default()
    };
    let transition_id = state
        .window_lifecycle_animator
        .start_or_reverse(
            lifecycle_request(
                &mut state.presentation_animator,
                window_id,
                309,
                rect(10.0, 10.0, 60.0, 60.0),
                rect(20.0, 20.0, 20.0, 20.0),
                LifecycleDirection::Minimize,
            ),
            AnimationTime::from_nanos(0),
            1.0,
        )
        .expect("Lamp transition starts");
    assert!(state.window_lifecycle_animator.snap_to_endpoint(
        transition_id,
        AnimationTime::monotonic_now().expect("monotonic time"),
    ));

    assert!(!state.lifecycle_animation_has_pending_visible());
    assert!(state.settle_lifecycle_no_visual_change());
    assert_eq!(state.window_lifecycle_animator.active_count(), 0);
    assert_eq!(state.presentation_animator.transaction_count(), 0);
    assert_eq!(state.presented_lifecycle_frame_id(), 0);
}

#[test]
fn old_visible_physical_lamp_prevents_no_visual_settlement() {
    let window_id = WindowId::from_raw(303).expect("valid window ID");
    let mut state = CompositorState {
        output_size: OutputSize::new(100, 100),
        lifecycle_animation_renderer_available: Some(true),
        ..Default::default()
    };
    state
        .window_lifecycle_animator
        .start_or_reverse(
            lifecycle_request(
                &mut state.presentation_animator,
                window_id,
                303,
                rect(300.0, 300.0, 100.0, 100.0),
                rect(500.0, 500.0, 20.0, 20.0),
                LifecycleDirection::Minimize,
            ),
            AnimationTime::from_nanos(0),
            1.0,
        )
        .expect("Lamp transition starts");
    assert_eq!(state.presentation_animator.transaction_count(), 1);
    let old = LifecycleSceneSample {
        sampled_at: AnimationTime::from_nanos(0),
        lamps: vec![LampWindowSample {
            window_id,
            root_surface_id: 303,
            presentation_identity: historical_identity(window_id, 99),
            visual_group: LifecycleVisualGroup::from_bounds(
                rect(0.0, 0.0, 80.0, 80.0),
                rect(0.0, 0.0, 80.0, 80.0),
                rect(0.0, 0.0, 80.0, 80.0),
                rect(20.0, 20.0, 20.0, 20.0),
                100,
                100,
            )
            .expect("valid visual group"),
            progress: 0.5,
            opacity: 1.0,
            direction: LifecycleDirection::Minimize,
            mathematically_settled: false,
        }],
        visual_sources: Vec::new(),
    };
    state.presented_lifecycle = LifecycleFrameSnapshot::from_sample(&old);
    assert!(!state.settle_lifecycle_no_visual_change());
    assert!(!state.settle_lifecycle_no_visual_change());
    assert_eq!(state.window_lifecycle_animator.active_count(), 1);
    assert!(state.lifecycle_animation_has_pending_visible());
    assert_eq!(
        state.direct_scanout_scene_candidate().unwrap_err(),
        DirectScanoutSceneRejection::LifecycleAnimation
    );
}

#[test]
fn lifecycle_render_fallback_preserves_confirmed_physical_lamp_until_replacement() {
    let window_id = WindowId::from_raw(308).expect("valid window ID");
    let root_surface_id = 308;
    let mut state = CompositorState {
        output_size: OutputSize::new(100, 100),
        lifecycle_animation_renderer_available: Some(true),
        ..Default::default()
    };
    let transition_id = state
        .window_lifecycle_animator
        .start_or_reverse(
            lifecycle_request(
                &mut state.presentation_animator,
                window_id,
                root_surface_id,
                rect(0.0, 0.0, 80.0, 80.0),
                rect(20.0, 20.0, 20.0, 20.0),
                LifecycleDirection::Minimize,
            ),
            AnimationTime::from_nanos(0),
            1.0,
        )
        .expect("Lamp transition starts");
    let physical = LifecycleSceneSample {
        sampled_at: AnimationTime::from_nanos(100_000_000),
        lamps: vec![LampWindowSample {
            window_id,
            root_surface_id,
            presentation_identity: transition_id,
            visual_group: LifecycleVisualGroup::from_bounds(
                rect(0.0, 0.0, 80.0, 80.0),
                rect(0.0, 0.0, 80.0, 80.0),
                rect(0.0, 0.0, 80.0, 80.0),
                rect(20.0, 20.0, 20.0, 20.0),
                100,
                100,
            )
            .expect("valid visual group"),
            progress: 0.5,
            opacity: 1.0,
            direction: LifecycleDirection::Minimize,
            mathematically_settled: false,
        }],
        visual_sources: Vec::new(),
    };
    state.publish_presented_lifecycle(1, &LifecycleFrameSnapshot::from_sample(&physical));
    let confirmed = state.presented_lifecycle.clone();
    state
        .lifecycle_render_suppressed_roots
        .insert(root_surface_id);

    assert!(
        state.apply_lifecycle_render_fallback(LifecycleRenderFallbackEntry {
            window_id,
            root_surface_id,
            presentation_identity: transition_id,
            reason: LifecycleRenderFallbackReason::LampProgramUnavailable,
        })
    );
    assert_eq!(state.window_lifecycle_animator.active_count(), 0);
    assert!(
        state
            .presentation_animator
            .transaction_record(transition_id.transaction_id())
            .is_none()
    );
    assert_eq!(state.presentation_animator.transaction_count(), 0);
    assert_eq!(state.presented_lifecycle, confirmed);
    assert_eq!(state.presented_lifecycle_frame_id, 1);
    assert!(state.lifecycle_animation_has_pending_visible());
    assert!(state.has_unowned_frame_work());

    state.publish_presented_lifecycle_with_replacements(
        2,
        &LifecycleFrameSnapshot::default(),
        &[root_surface_id],
        true,
    );
    assert!(state.presented_lifecycle.lamps.is_empty());
    assert_eq!(state.presented_lifecycle_frame_id, 2);
    assert!(!state.lifecycle_animation_has_pending_visible());
}

#[test]
fn canonical_presentation_replaces_old_physical_lamp_after_logical_cancel() {
    let window_id = WindowId::from_raw(304).expect("valid window ID");
    let mut state = CompositorState {
        output_size: OutputSize::new(100, 100),
        lifecycle_animation_renderer_available: Some(true),
        ..Default::default()
    };
    let old = LifecycleSceneSample {
        sampled_at: AnimationTime::from_nanos(0),
        lamps: vec![LampWindowSample {
            window_id,
            root_surface_id: 304,
            presentation_identity: historical_identity(window_id, 100),
            visual_group: LifecycleVisualGroup::from_bounds(
                rect(0.0, 0.0, 80.0, 80.0),
                rect(0.0, 0.0, 80.0, 80.0),
                rect(0.0, 0.0, 80.0, 80.0),
                rect(20.0, 20.0, 20.0, 20.0),
                100,
                100,
            )
            .expect("valid visual group"),
            progress: 0.5,
            opacity: 1.0,
            direction: LifecycleDirection::Minimize,
            mathematically_settled: false,
        }],
        visual_sources: Vec::new(),
    };
    state.presented_lifecycle = LifecycleFrameSnapshot::from_sample(&old);
    state.publish_presented_lifecycle_with_replacements(2, &Default::default(), &[304], true);
    assert!(state.presented_lifecycle.lamps.is_empty());
}

#[test]
fn rendered_replacement_clears_absent_physical_lamp_without_acknowledging_active_transition() {
    let window_id = WindowId::from_raw(305).expect("valid window ID");
    let mut state = CompositorState {
        output_size: OutputSize::new(100, 100),
        lifecycle_animation_renderer_available: Some(true),
        ..Default::default()
    };
    state
        .window_lifecycle_animator
        .start_or_reverse(
            lifecycle_request(
                &mut state.presentation_animator,
                window_id,
                305,
                rect(0.0, 0.0, 80.0, 80.0),
                rect(20.0, 20.0, 20.0, 20.0),
                LifecycleDirection::Minimize,
            ),
            AnimationTime::from_nanos(0),
            1.0,
        )
        .expect("Lamp transition starts");
    let physical = state
        .window_lifecycle_animator
        .sample_scene(AnimationTime::from_nanos(100_000_000));
    state.presented_lifecycle = LifecycleFrameSnapshot::from_sample(&physical);
    state.publish_presented_lifecycle_with_replacements(2, &Default::default(), &[], true);
    assert!(state.presented_lifecycle.lamps.is_empty());
    assert_eq!(state.window_lifecycle_animator.active_count(), 1);
    assert_eq!(
        state
            .window_lifecycle_animator
            .sample_scene(AnimationTime::from_nanos(100_000_000))
            .lamps
            .len(),
        1
    );
}

#[test]
fn runtime_slot_change_snaps_minimize_and_restore_but_retains_physical_ownership() {
    let directory = std::env::temp_dir().join(format!(
        "typhon-lifecycle-policy-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock")
            .as_nanos()
    ));
    std::fs::create_dir(&directory).expect("create animation configuration directory");
    let mut state = CompositorState {
        lifecycle_animation_renderer_available: Some(true),
        ..Default::default()
    };
    state.animation_control = crate::animation_control::AnimationControlState::from_store(
        crate::animation_control::AnimationConfigurationStore::new(directory.clone())
            .expect("create animation configuration store"),
    );
    let window_id = WindowId::from_raw(306).expect("valid window ID");
    let minimize_id = state
        .window_lifecycle_animator
        .start_or_reverse(
            lifecycle_request(
                &mut state.presentation_animator,
                window_id,
                306,
                rect(0.0, 0.0, 80.0, 80.0),
                rect(200.0, 200.0, 20.0, 20.0),
                LifecycleDirection::Minimize,
            ),
            AnimationTime::from_nanos(0),
            1.0,
        )
        .expect("minimize starts");
    let mut candidate = state.animation_control.configuration().clone();
    candidate
        .overrides
        .insert(AnimationSlot::WindowMinimize, AnimationEffect::None);
    state
        .set_animation_configuration(candidate)
        .expect("runtime policy mutation persists");
    let minimize = state
        .window_lifecycle_animator
        .sample(minimize_id.scene_node_id(), AnimationTime::from_nanos(0))
        .expect("snapped minimize remains owned");
    assert_eq!(minimize.presentation_identity, minimize_id);
    assert_eq!(minimize.progress, 1.0);
    assert!(minimize.mathematically_settled);
    let minimize_sample = state.lifecycle_scene_sample_at(AnimationTime::from_nanos(0));
    let minimize_frame = LifecycleFrameSnapshot::qualified_from_sample(
        &minimize_sample,
        &LifecycleRenderEvidence::from_consumed([LifecycleRenderEvidenceEntry {
            window_id,
            root_surface_id: 306,
            presentation_identity: minimize_id,
        }]),
    );
    state.publish_presented_lifecycle(1, &minimize_frame);
    assert_eq!(state.window_lifecycle_animator.active_count(), 0);

    let restore_id = state
        .window_lifecycle_animator
        .start_or_reverse(
            lifecycle_request(
                &mut state.presentation_animator,
                window_id,
                306,
                rect(0.0, 0.0, 80.0, 80.0),
                rect(200.0, 200.0, 20.0, 20.0),
                LifecycleDirection::Restore,
            ),
            AnimationTime::from_nanos(0),
            1.0,
        )
        .expect("restore starts");
    state.lifecycle_render_suppressed_roots.insert(306);
    let mut candidate = state.animation_control.configuration().clone();
    candidate
        .overrides
        .insert(AnimationSlot::WindowRestore, AnimationEffect::None);
    state
        .set_animation_configuration(candidate)
        .expect("restore policy mutation persists");
    let restore = state
        .window_lifecycle_animator
        .sample(restore_id.scene_node_id(), AnimationTime::from_nanos(0))
        .expect("snapped restore remains owned");
    assert_eq!(restore.presentation_identity, restore_id);
    assert_eq!(restore.progress, 0.0);
    assert!(state.lifecycle_render_suppressed_roots.contains(&306));
    let restore_sample = state.lifecycle_scene_sample_at(AnimationTime::from_nanos(0));
    let restore_frame = LifecycleFrameSnapshot::qualified_from_sample(
        &restore_sample,
        &LifecycleRenderEvidence::from_consumed([LifecycleRenderEvidenceEntry {
            window_id,
            root_surface_id: 306,
            presentation_identity: restore_id,
        }]),
    );
    state.publish_presented_lifecycle(2, &restore_frame);
    assert_eq!(state.window_lifecycle_animator.active_count(), 0);
    assert!(!state.lifecycle_render_suppressed_roots.contains(&306));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn stale_lifecycle_render_fallback_cannot_cancel_a_reversal() {
    let window_id = WindowId::from_raw(307).expect("valid window ID");
    let mut state = CompositorState {
        lifecycle_animation_renderer_available: Some(true),
        ..Default::default()
    };
    let mut request = |direction| {
        lifecycle_request(
            &mut state.presentation_animator,
            window_id,
            307,
            rect(0.0, 0.0, 80.0, 80.0),
            rect(200.0, 200.0, 20.0, 20.0),
            direction,
        )
    };
    let old_id = state
        .window_lifecycle_animator
        .start_or_reverse(
            request(LifecycleDirection::Minimize),
            AnimationTime::from_nanos(0),
            1.0,
        )
        .expect("minimize starts");
    let new_id = state
        .window_lifecycle_animator
        .start_or_reverse(
            request(LifecycleDirection::Restore),
            AnimationTime::from_nanos(100_000_000),
            1.0,
        )
        .expect("restore reverses");
    assert_ne!(old_id, new_id);
    assert!(
        state
            .presentation_animator
            .retire_retained_visual_exact(old_id)
    );
    assert!(
        state
            .presentation_animator
            .transaction_record(new_id.transaction_id())
            .is_some()
    );

    assert!(
        !state.apply_lifecycle_render_fallback(LifecycleRenderFallbackEntry {
            window_id,
            root_surface_id: 307,
            presentation_identity: old_id,
            reason: LifecycleRenderFallbackReason::LampProgramUnavailable,
        })
    );
    assert!(
        state
            .presentation_animator
            .transaction_record(new_id.transaction_id())
            .is_some()
    );
    assert_eq!(
        state
            .window_lifecycle_animator
            .sample(
                new_id.scene_node_id(),
                AnimationTime::from_nanos(100_000_000)
            )
            .expect("new transition survives")
            .presentation_identity,
        new_id
    );
}

#[test]
fn failed_minimize_install_rolls_back_identity_without_taking_over_previous_state() {
    let (mut state, window_id) = ssd_test_state(405);
    state.lifecycle_animation_renderer_available = Some(true);
    let root_surface_id = 405;
    let group = visual_group(rect(384.0, 60.0, 832.0, 640.0));
    let retained_decoration = lifecycle_decoration(window_id, root_surface_id, 0x61);
    let replacement_decoration = lifecycle_decoration(window_id, root_surface_id, 0x62);

    state.begin_lifecycle_restore(
        window_id,
        root_surface_id,
        Some(group),
        ResolvedEffectScene::default(),
        vec![retained_decoration.clone()],
    );
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("window group SceneNode");
    let old_identity = state
        .window_lifecycle_animator
        .identity(scene_node_id)
        .expect("active restore identity");
    let now = AnimationTime::monotonic_now().expect("test monotonic time");
    state
        .presentation_animator
        .commit(
            crate::presentation_animation::PresentationTransactionRequest::geometry(
                now,
                vec![
                    crate::presentation_animation::PresentationGeometryMutation::new(
                        scene_node_id,
                        rect(0.0, 0.0, 80.0, 60.0),
                        rect(10.0, 0.0, 80.0, 60.0),
                        crate::presentation_animation::AnimationCurve::easing(
                            std::time::Duration::from_millis(200),
                            crate::presentation_animation::EasingCurve::Linear,
                        ),
                    ),
                ],
            ),
        )
        .expect("active Geometry transaction");
    state
        .lifecycle_render_suppressed_roots
        .insert(root_surface_id);
    state
        .lifecycle_decorations
        .insert(root_surface_id, retained_decoration.clone());
    state.window_lifecycle_animator.fail_next_start_for_test();

    state.begin_lifecycle_minimize(
        window_id,
        root_surface_id,
        Some(rect(400.0, 100.0, 800.0, 600.0)),
        Some(rect(400.0, 100.0, 800.0, 600.0)),
        Some(group),
        ResolvedEffectScene::default(),
        vec![replacement_decoration],
    );

    assert_eq!(state.window_lifecycle_animator.active_count(), 1);
    assert_eq!(
        state.window_lifecycle_animator.identity(scene_node_id),
        Some(old_identity)
    );
    assert!(
        state
            .presentation_animator
            .has_geometry_track(scene_node_id)
    );
    assert_eq!(state.presentation_animator.transaction_count(), 2);
    assert!(
        state
            .presentation_animator
            .transaction_record(old_identity.transaction_id())
            .is_some()
    );
    assert!(
        state
            .lifecycle_render_suppressed_roots
            .contains(&root_surface_id)
    );
    assert_eq!(
        state.lifecycle_decorations[&root_surface_id]
            .scene_snapshot()
            .visual_signature(),
        retained_decoration.scene_snapshot().visual_signature()
    );
}

#[test]
fn exhausted_identity_namespace_does_not_cancel_geometry_or_add_lifecycle_state() {
    for transaction_ids in [true, false] {
        let surface_id = if transaction_ids { 406 } else { 407 };
        let (mut state, window_id) = ssd_test_state(surface_id);
        state.lifecycle_animation_renderer_available = Some(true);
        let scene_node_id = state
            .scene_node_id_for_window_group(window_id)
            .expect("window group SceneNode");
        commit_geometry_track(&mut state, scene_node_id);
        exhaust_identity_allocator(&mut state, scene_node_id, transaction_ids);
        assert_eq!(state.presentation_animator.transaction_count(), 1);

        let group = visual_group(rect(384.0, 60.0, 832.0, 640.0));
        state.begin_lifecycle_minimize(
            window_id,
            surface_id,
            Some(rect(400.0, 100.0, 800.0, 600.0)),
            Some(rect(400.0, 100.0, 800.0, 600.0)),
            Some(group),
            ResolvedEffectScene::default(),
            vec![lifecycle_decoration(window_id, surface_id, 0x71)],
        );

        assert_eq!(state.window_lifecycle_animator.active_count(), 0);
        assert_eq!(state.presentation_animator.transaction_count(), 1);
        assert!(
            state
                .presentation_animator
                .has_geometry_track(scene_node_id)
        );
        assert!(
            !state
                .lifecycle_render_suppressed_roots
                .contains(&surface_id)
        );
        assert!(!state.lifecycle_decorations.contains_key(&surface_id));
    }
}

#[test]
fn missing_window_group_scene_node_does_not_create_lifecycle_identity_or_side_effects() {
    let window_id = WindowId::from_raw(408).expect("window id");
    let root_surface_id = 408;
    let mut state = CompositorState {
        lifecycle_animation_renderer_available: Some(true),
        ..Default::default()
    };
    let group = visual_group(rect(384.0, 60.0, 832.0, 640.0));

    state.begin_lifecycle_minimize(
        window_id,
        root_surface_id,
        Some(rect(400.0, 100.0, 800.0, 600.0)),
        Some(rect(400.0, 100.0, 800.0, 600.0)),
        Some(group),
        ResolvedEffectScene::default(),
        vec![lifecycle_decoration(window_id, root_surface_id, 0x72)],
    );
    state.begin_lifecycle_restore(
        window_id,
        root_surface_id,
        Some(group),
        ResolvedEffectScene::default(),
        vec![lifecycle_decoration(window_id, root_surface_id, 0x73)],
    );

    assert_eq!(state.window_lifecycle_animator.active_count(), 0);
    assert_eq!(state.presentation_animator.transaction_count(), 0);
    assert!(
        !state
            .lifecycle_render_suppressed_roots
            .contains(&root_surface_id)
    );
    assert!(!state.lifecycle_decorations.contains_key(&root_surface_id));
}

#[test]
fn cancel_teardown_and_renderer_unavailable_retire_exact_members() {
    let (mut cancelled, cancelled_window) = ssd_test_state(409);
    cancelled.lifecycle_animation_renderer_available = Some(true);
    let root_surface_id = 409;
    cancelled.begin_lifecycle_restore(
        cancelled_window,
        root_surface_id,
        Some(visual_group(rect(384.0, 60.0, 832.0, 640.0))),
        ResolvedEffectScene::default(),
        Vec::new(),
    );
    let cancelled_node = cancelled
        .scene_node_id_for_window_group(cancelled_window)
        .expect("window group SceneNode");
    let cancelled_identity = cancelled
        .window_lifecycle_animator
        .identity(cancelled_node)
        .expect("active restore");
    cancelled.lifecycle_cancel_window(cancelled_window);
    assert_eq!(cancelled.window_lifecycle_animator.active_count(), 0);
    assert!(
        cancelled
            .presentation_animator
            .transaction_record(cancelled_identity.transaction_id())
            .is_none()
    );

    let (mut torn_down, teardown_window) = ssd_test_state(410);
    torn_down.lifecycle_animation_renderer_available = Some(true);
    torn_down.begin_lifecycle_restore(
        teardown_window,
        410,
        Some(visual_group(rect(384.0, 60.0, 832.0, 640.0))),
        ResolvedEffectScene::default(),
        Vec::new(),
    );
    let teardown_node = torn_down
        .scene_node_id_for_window_group(teardown_window)
        .expect("window group SceneNode");
    let teardown_identity = torn_down
        .window_lifecycle_animator
        .identity(teardown_node)
        .expect("active restore");
    assert!(torn_down.remove_desktop_window(teardown_window).is_some());
    assert_eq!(torn_down.window_lifecycle_animator.active_count(), 0);
    assert!(
        torn_down
            .presentation_animator
            .transaction_record(teardown_identity.transaction_id())
            .is_none()
    );

    let (mut unavailable, unavailable_window) = ssd_test_state(411);
    unavailable.lifecycle_animation_renderer_available = Some(true);
    unavailable.begin_lifecycle_restore(
        unavailable_window,
        411,
        Some(visual_group(rect(384.0, 60.0, 832.0, 640.0))),
        ResolvedEffectScene::default(),
        Vec::new(),
    );
    let unavailable_node = unavailable
        .scene_node_id_for_window_group(unavailable_window)
        .expect("window group SceneNode");
    let unavailable_identity = unavailable
        .window_lifecycle_animator
        .identity(unavailable_node)
        .expect("active restore");
    unavailable.set_lifecycle_animation_renderer_available(false);
    assert_eq!(unavailable.window_lifecycle_animator.active_count(), 0);
    assert!(
        unavailable
            .presentation_animator
            .transaction_record(unavailable_identity.transaction_id())
            .is_none()
    );
}

#[test]
fn fresh_restore_freezes_ssd_until_physical_settlement() {
    let (mut state, window_id) = ssd_test_state(401);
    state.lifecycle_animation_renderer_available = Some(true);
    let root_surface_id = 401;
    let group = visual_group(rect(384.0, 60.0, 832.0, 640.0));
    let decoration_a = state
        .native_decoration_render_instances_for_scale(&state.renderable_surfaces, 1.0)
        .into_iter()
        .next()
        .expect("authoritative SSD snapshot A");

    state.begin_lifecycle_restore(
        window_id,
        root_surface_id,
        Some(group),
        ResolvedEffectScene::default(),
        vec![decoration_a.clone()],
    );

    assert_eq!(
        lifecycle_decoration_signature(
            &state,
            root_surface_id,
            &state.renderable_surfaces,
            AnimationTime::monotonic_now().unwrap(),
        ),
        Some(decoration_a.scene_snapshot().visual_signature())
    );

    state.focused_window_id = Some(window_id);
    let decoration_b = state
        .native_decoration_render_instances_for_scale(&state.renderable_surfaces, 1.0)
        .into_iter()
        .next()
        .expect("live SSD snapshot B");
    assert_ne!(
        decoration_a.scene_snapshot().visual_signature(),
        decoration_b.scene_snapshot().visual_signature()
    );
    assert_eq!(
        lifecycle_decoration_signature(
            &state,
            root_surface_id,
            &state.renderable_surfaces,
            AnimationTime::monotonic_now().unwrap(),
        ),
        Some(decoration_a.scene_snapshot().visual_signature())
    );

    settle_lifecycle_transition(&mut state, window_id);
    assert!(!state.lifecycle_decorations.contains_key(&root_surface_id));
}

#[test]
fn reversal_preserves_existing_frozen_ssd_snapshot() {
    let (mut state, window_id) = ssd_test_state(402);
    state.lifecycle_animation_renderer_available = Some(true);
    let root_surface_id = 402;
    let group_a = visual_group(rect(384.0, 60.0, 832.0, 640.0));
    let group_b = visual_group(rect(384.0, 20.0, 832.0, 680.0));
    let decoration_a = lifecycle_decoration(window_id, root_surface_id, 0x31);
    let decoration_b = lifecycle_decoration(window_id, root_surface_id, 0x32);

    state.begin_lifecycle_minimize(
        window_id,
        root_surface_id,
        Some(rect(400.0, 100.0, 800.0, 600.0)),
        Some(rect(400.0, 100.0, 800.0, 600.0)),
        Some(group_a),
        ResolvedEffectScene::default(),
        vec![decoration_a.clone()],
    );
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("window group SceneNode");
    let old_identity = state
        .window_lifecycle_animator
        .identity(scene_node_id)
        .expect("minimize identity");
    let before = state
        .window_lifecycle_animator
        .sample(
            scene_node_id,
            AnimationTime::monotonic_now().expect("monotonic time"),
        )
        .expect("minimize sample");
    state
        .lifecycle_decorations
        .insert(root_surface_id, decoration_a.clone());
    state.begin_lifecycle_restore(
        window_id,
        root_surface_id,
        Some(group_b),
        ResolvedEffectScene::default(),
        vec![decoration_b],
    );
    let new_identity = state
        .window_lifecycle_animator
        .identity(scene_node_id)
        .expect("reversed restore identity");
    let after = state
        .window_lifecycle_animator
        .sample(
            scene_node_id,
            AnimationTime::monotonic_now().expect("monotonic time"),
        )
        .expect("restore sample");

    assert_eq!(after.presentation_identity, new_identity);
    assert_eq!(new_identity.scene_node_id(), scene_node_id);
    assert_ne!(new_identity.transaction_id(), old_identity.transaction_id());
    assert_ne!(new_identity.revision_id(), old_identity.revision_id());
    assert!((after.progress - before.progress).abs() < 0.01);
    assert_eq!(after.visual_group, group_a);
    assert!(
        state
            .presentation_animator
            .transaction_record(old_identity.transaction_id())
            .is_none()
    );
    assert!(
        state
            .presentation_animator
            .transaction_record(new_identity.transaction_id())
            .is_some()
    );

    assert_eq!(
        lifecycle_decoration_signature(
            &state,
            root_surface_id,
            &[],
            AnimationTime::monotonic_now().unwrap(),
        ),
        Some(decoration_a.scene_snapshot().visual_signature())
    );
    assert_eq!(
        state
            .window_lifecycle_animator
            .visual_group(scene_node_id)
            .expect("reversed transition")
            .canonical_visual_rect,
        group_a.canonical_visual_rect
    );
}

#[test]
fn later_independent_restore_replaces_settled_ssd_snapshot() {
    let (mut state, window_id) = ssd_test_state(403);
    state.lifecycle_animation_renderer_available = Some(true);
    let root_surface_id = 403;
    let group = visual_group(rect(384.0, 60.0, 832.0, 640.0));
    let decoration_a = lifecycle_decoration(window_id, root_surface_id, 0x41);
    let decoration_b = lifecycle_decoration(window_id, root_surface_id, 0x42);

    state.begin_lifecycle_restore(
        window_id,
        root_surface_id,
        Some(group),
        ResolvedEffectScene::default(),
        vec![decoration_a],
    );
    settle_lifecycle_transition(&mut state, window_id);
    assert!(!state.lifecycle_decorations.contains_key(&root_surface_id));

    state.begin_lifecycle_restore(
        window_id,
        root_surface_id,
        Some(group),
        ResolvedEffectScene::default(),
        vec![decoration_b.clone()],
    );
    assert_eq!(
        lifecycle_decoration_signature(
            &state,
            root_surface_id,
            &[],
            AnimationTime::monotonic_now().unwrap(),
        ),
        Some(decoration_b.scene_snapshot().visual_signature())
    );
}

#[test]
fn fresh_csd_restore_does_not_synthesize_frozen_ssd() {
    let (mut state, window_id) = ssd_test_state(404);
    state.lifecycle_animation_renderer_available = Some(true);
    let root_surface_id = 404;
    let stale_ssd = lifecycle_decoration(window_id, root_surface_id, 0x51);
    state
        .lifecycle_decorations
        .insert(root_surface_id, stale_ssd);

    state.begin_lifecycle_restore(
        window_id,
        root_surface_id,
        Some(visual_group(rect(400.0, 100.0, 800.0, 600.0))),
        ResolvedEffectScene::default(),
        Vec::new(),
    );

    assert!(!state.lifecycle_decorations.contains_key(&root_surface_id));
    let sample = state.lifecycle_scene_sample_at(AnimationTime::monotonic_now().unwrap());
    assert!(
        state
            .lifecycle_decoration_render_instances(&sample, &[])
            .is_empty()
    );
}
