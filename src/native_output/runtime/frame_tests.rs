use crate::native_output::runtime::frame::{
    NativeCursorOutputArbitration, NativeCursorOutputDisposition, NativeCursorRenderMode,
    NativeFrameRenderer, ResolvedNativeFrameScene,
};
use crate::native_output::runtime::frame_scene_identity::{
    finalize_snapshot, reset_snapshot_work_counters, snapshot_work_counters, visibility_signature,
};
use crate::native_output::runtime::{NativeInputState, NativeSceneSnapshot};
use oblivion_one::compositor::{
    AnimationTime, FullscreenRenderPlanMetrics, PresentationRect, RenderableSurface,
    RenderableSurfaceDamage, ResolvedEffectScene, SurfaceCommitSequence, SurfaceOpaqueRegion,
    SurfacePlacement, SurfaceRenderBackend, WindowId,
};
use oblivion_one::compositor::{
    EffectAnchor, EffectAnchorScope, EffectSceneOrder, OwnCompositorServer, ResolvedEffectInstance,
};
use oblivion_one::compositor::{PresentationFrameSnapshot, PresentationSceneSample};
use oblivion_one::effects::{
    EffectFrameDemand, EffectInstanceId, EffectParameterBlock, EffectProgramId, EffectRect,
    EffectRegion,
};
use oblivion_one::render_backend::buffer::{BufferIdAllocator, BufferSize, CommittedSurfaceBuffer};
use oblivion_one::window_lifecycle_animation::{LifecycleFrameSnapshot, LifecycleSceneSample};
use std::borrow::Cow;
use std::process;
use wayland_server::protocol::wl_output;

fn test_surface(
    surface_id: u32,
    width: u32,
    height: u32,
    placement: SurfacePlacement,
) -> RenderableSurface {
    let identity = BufferIdAllocator::default()
        .allocate()
        .expect("test buffer identity");
    RenderableSurface {
        surface_id,
        x: 0,
        y: 0,
        width,
        height,
        placement,
        render_backend: SurfaceRenderBackend::NativeWayland,
        render_placement: None,
        visual_clip: None,
        render_target_size: None,
        generation: 1,
        commit_sequence: SurfaceCommitSequence::initial(),
        buffer: CommittedSurfaceBuffer::shm_snapshot(
            identity,
            BufferSize::new(width, height).expect("test surface size"),
            vec![0; width as usize * height as usize],
        ),
        viewport_source: None,
        viewport_destination: None,
        buffer_scale: 1,
        buffer_transform: wl_output::Transform::Normal,
        opaque_region: SurfaceOpaqueRegion::None,
        damage: RenderableSurfaceDamage::Full,
    }
}

#[test]
fn resolved_native_frame_scene_excludes_culled_transition_owner() {
    let socket_name = format!("typhon-frame-membership-{}", process::id());
    let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name)
        .expect("bind compositor for frame-membership regression");
    let rear = test_surface(601, 320, 200, SurfacePlacement::root_at(100, 100));
    let owner = test_surface(602, 1280, 800, SurfacePlacement::absolute_root_at(0, 0));
    let rear_rect =
        PresentationRect::new(100.0, 100.0, 320.0, 200.0).expect("rear presentation rect");
    let settled_rear_rect =
        PresentationRect::new(140.0, 100.0, 320.0, 200.0).expect("settled rear presentation rect");
    server.install_native_frame_test_scene(
        vec![rear, owner],
        &[
            (601, WindowId::from_raw(1).expect("rear window id")),
            (602, WindowId::from_raw(2).expect("fullscreen window id")),
        ],
        Some(602),
    );
    server.start_test_presentation_transition(
        601,
        rear_rect,
        settled_rear_rect,
        AnimationTime::from_nanos(0),
    );

    let resolved =
        ResolvedNativeFrameScene::from_server_at(&server, AnimationTime::from_nanos(2_000_000));

    assert_eq!(resolved.surface_ids().collect::<Vec<_>>(), [602]);
    assert!(resolved.presentation.transform_for_root(601).is_none());
    assert_eq!(
        resolved
            .presentation_snapshot
            .presented_windows
            .iter()
            .map(|window| window.root_surface_id())
            .collect::<Vec<_>>(),
        [602]
    );
}

#[test]
fn resolved_native_frame_scene_filters_culled_effects_and_restores_them() {
    let socket_name = format!("typhon-frame-effect-{}", process::id());
    let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name)
        .expect("bind compositor for fullscreen effect regression");
    let panel = test_surface(611, 320, 200, SurfacePlacement::root_at(80, 70));
    let owner = test_surface(612, 1280, 800, SurfacePlacement::absolute_root_at(0, 0));
    let windows = &[
        (611, WindowId::from_raw(11).expect("panel window id")),
        (612, WindowId::from_raw(12).expect("fullscreen window id")),
    ];
    server.install_native_frame_test_scene(vec![panel, owner], windows, None);
    assert!(server.install_native_frame_test_effect(
        611,
        EffectAnchor::BeforeSurface(611),
        oblivion_one::effects::builtin_background_blur_program_id(),
        EffectRegion::from_rect(EffectRect::new(80, 70, 320, 200).unwrap()),
    ));

    let normal = ResolvedNativeFrameScene::from_server_at(&server, AnimationTime::from_nanos(0));
    assert_eq!(normal.surface_ids().collect::<Vec<_>>(), [611, 612]);
    assert_eq!(normal.effects.instances.len(), 1);
    assert_eq!(
        normal.effects.instances[0].anchor,
        EffectAnchor::BeforeSurface(611)
    );

    server.install_native_frame_test_scene(
        vec![
            test_surface(611, 320, 200, SurfacePlacement::root_at(80, 70)),
            test_surface(612, 1280, 800, SurfacePlacement::absolute_root_at(0, 0)),
        ],
        windows,
        Some(612),
    );
    let fullscreen =
        ResolvedNativeFrameScene::from_server_at(&server, AnimationTime::from_nanos(0));
    assert_eq!(fullscreen.surface_ids().collect::<Vec<_>>(), [612]);
    assert!(fullscreen.effects.instances.is_empty());

    server.install_native_frame_test_scene(
        vec![
            test_surface(611, 320, 200, SurfacePlacement::root_at(80, 70)),
            test_surface(612, 1280, 800, SurfacePlacement::absolute_root_at(0, 0)),
        ],
        windows,
        None,
    );
    let restored = ResolvedNativeFrameScene::from_server_at(&server, AnimationTime::from_nanos(0));
    assert_eq!(restored.surface_ids().collect::<Vec<_>>(), [611, 612]);
    assert_eq!(restored.effects.instances.len(), 1);
    assert_eq!(
        restored.effects.instances[0].anchor,
        EffectAnchor::BeforeSurface(611)
    );
}

#[test]
fn resolved_native_frame_scene_and_egl_request_retain_floating_ssd() {
    let socket_name = format!("typhon-floating-ssd-frame-{}", process::id());
    let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name)
        .expect("bind compositor for Floating SSD frame regression");
    server.install_native_frame_test_scene_with_server_decorations(
        vec![test_surface(
            603,
            320,
            200,
            SurfacePlacement::root_at(40, 50),
        )],
        &[(603, WindowId::from_raw(3).expect("test window id"))],
        None,
    );

    let resolved = ResolvedNativeFrameScene::from_server_at(&server, AnimationTime::from_nanos(0));

    assert_eq!(resolved.surface_ids().collect::<Vec<_>>(), [603]);
    assert_eq!(resolved.decorations.len(), 1);
    let decoration = &resolved.decorations[0];
    assert_eq!(decoration.root_surface_id(), 603);
    let (_, _, width, height) = decoration.scene_snapshot().bounds();
    assert_eq!(width, 320);
    assert!(height > 200, "Floating SSD must add visible chrome height");
    assert!(resolved.lifecycle_decorations.is_empty());

    let mut renderer = NativeFrameRenderer::default();
    let input_state = NativeInputState::new(1280, 800);
    let request = renderer.egl_scene_draw_request(
        1280,
        800,
        &resolved,
        &server,
        &input_state,
        NativeCursorRenderMode::Hardware,
        None,
    );
    assert_eq!(request.decoration_instances.len(), 1);
    assert_eq!(request.decoration_instances[0].root_surface_id(), 603);
}

#[test]
fn scene_identity_and_damage_reuse_finalized_snapshot() {
    let socket_name = format!("typhon-c2a-snapshot-{}", process::id());
    let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name)
        .expect("bind compositor for C2a snapshot regression");
    server.install_native_frame_test_scene(
        vec![test_surface(701, 320, 200, SurfacePlacement::root_at(0, 0))],
        &[(701, WindowId::from_raw(1).expect("test window id"))],
        None,
    );
    reset_snapshot_work_counters();
    let resolved = ResolvedNativeFrameScene::from_server_at(&server, AnimationTime::from_nanos(0));
    let construction_counters = snapshot_work_counters();
    assert_eq!(construction_counters.snapshot_finalizations, 1);
    assert_eq!(construction_counters.identity_computations, 1);
    let mut expected_signature = resolved.snapshot_ref().identity_signature();
    expected_signature ^= resolved.effects.signature;
    expected_signature = expected_signature.wrapping_mul(0x1000_0000_01b3);
    assert_eq!(resolved.scene_identity_signature(), expected_signature);
    reset_snapshot_work_counters();

    let _current = resolved.snapshot_ref();
    let _identity = resolved.scene_identity_signature();

    let counters = snapshot_work_counters();
    assert_eq!(counters.snapshot_finalizations, 0);
    assert_eq!(counters.snapshot_owned_clones, 0);
    assert_eq!(counters.identity_computations, 0);

    let owned = resolved.snapshot_owned();
    assert_eq!(owned, *resolved.snapshot_ref());
    assert_eq!(snapshot_work_counters().snapshot_owned_clones, 1);
}

#[test]
fn snapshot_ref_preserves_constructor_popup_ids_and_order() {
    fn assert_popup_ids(popup_surface_ids: &[u32]) {
        let output_id = oblivion_one::core::OutputId::from_raw(1).expect("test output id");
        let surfaces: &[RenderableSurface] = &[];
        let resolved = ResolvedNativeFrameScene {
            surfaces: Cow::Borrowed(surfaces),
            surface_scene_node_ids: Cow::Borrowed(&[]),
            decorations: Vec::new(),
            popup_surface_ids: Cow::Borrowed(popup_surface_ids),
            external_overlay_surface_ids: Vec::new(),
            render_generation: 1,
            visibility: FullscreenRenderPlanMetrics::default(),
            snapshot: NativeSceneSnapshot::from_surfaces_with_popup_ids(
                surfaces,
                Vec::new(),
                popup_surface_ids,
            ),
            scene_identity_signature: 0,
            effects: ResolvedEffectScene::default(),
            presentation: PresentationSceneSample::empty_for_output(
                output_id,
                AnimationTime::from_nanos(0),
                oblivion_one::compositor::PresentationSampleTimeSource::ZeroFallback,
            ),
            presentation_snapshot: PresentationFrameSnapshot::empty_for_output(output_id),
            lifecycle: LifecycleSceneSample {
                sampled_at: AnimationTime::from_nanos(0),
                lamps: Vec::new(),
                visual_sources: Vec::new(),
            },
            lifecycle_surfaces: Vec::new(),
            lifecycle_decorations: Vec::new(),
            lifecycle_snapshot: LifecycleFrameSnapshot::default(),
        };
        assert_eq!(resolved.snapshot_ref().popup_surface_ids, popup_surface_ids);
    }

    assert_popup_ids(&[]);
    assert_popup_ids(&[701, 702, 701]);
}

#[test]
fn finalized_snapshot_contains_all_dynamic_metadata_fields() {
    let effect_region = EffectRegion::from_rect(EffectRect::new(20, 30, 40, 50).unwrap());
    let effects = ResolvedEffectScene::new(
        9,
        vec![ResolvedEffectInstance {
            id: EffectInstanceId::new(1).unwrap(),
            program: EffectProgramId::new(2).unwrap(),
            anchor: EffectAnchor::BeforeSurface(701),
            region: effect_region.clone(),
            target_bounds: effect_region.bounding_rect().unwrap(),
            parameter_block: EffectParameterBlock::default(),
            signature: 17,
            frame_demand: EffectFrameDemand::OnDamage,
            visual_group: None,
            anchor_scope: EffectAnchorScope::VisualGroup,
            scene_order: EffectSceneOrder::for_anchor(EffectAnchor::BeforeSurface(701)),
        }],
    );
    let visibility = FullscreenRenderPlanMetrics {
        fullscreen_active: true,
        owner_root_surface_id: Some(701),
        fullscreen_composition_active: true,
        fullscreen_transition_pending: false,
        solitary_tree_active: false,
        culled_surface_count: 3,
        wallpaper_culled: true,
        visible_overlay_count: 2,
        fullscreen_allowed_application_roots: 1,
        fullscreen_allowed_layer_roots: 1,
        fullscreen_culled_application_roots: 1,
        fullscreen_culled_layer_roots: 1,
        fullscreen_above_reason: None,
        rejection: None,
    };

    let (snapshot, cached_signature) = finalize_snapshot(
        NativeSceneSnapshot::from_surfaces_with_popup_ids(&[], Vec::new(), &[701, 702]),
        &[703],
        visibility,
        &effects,
    );

    assert_eq!(snapshot.popup_surface_ids, [701, 702]);
    assert_eq!(snapshot.external_overlay_surface_ids, [703]);
    assert_eq!(
        snapshot.visibility_signature,
        visibility_signature(visibility)
    );
    assert_eq!(snapshot.effect_damage, effect_region);
    assert_eq!(snapshot.effect_identity_signature, effects.signature);
    let mut expected_signature = snapshot.identity_signature();
    expected_signature ^= effects.signature;
    expected_signature = expected_signature.wrapping_mul(0x1000_0000_01b3);
    assert_eq!(cached_signature, expected_signature);
}

#[test]
fn presentation_animation_samples_keep_distinct_frame_local_snapshots() {
    let socket_name = format!("typhon-c2a-animation-{}", process::id());
    let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name)
        .expect("bind compositor for C2a animation regression");
    server.install_native_frame_test_scene(
        vec![test_surface(702, 320, 200, SurfacePlacement::root_at(0, 0))],
        &[(702, WindowId::from_raw(2).expect("test window id"))],
        None,
    );
    server.start_test_presentation_transition(
        702,
        PresentationRect::new(0.0, 0.0, 320.0, 200.0).expect("animation start"),
        PresentationRect::new(100.0, 0.0, 320.0, 200.0).expect("animation target"),
        AnimationTime::from_nanos(0),
    );

    reset_snapshot_work_counters();
    let first =
        ResolvedNativeFrameScene::from_server_at(&server, AnimationTime::from_nanos(250_000));
    let first_counters = snapshot_work_counters();
    reset_snapshot_work_counters();
    let second =
        ResolvedNativeFrameScene::from_server_at(&server, AnimationTime::from_nanos(750_000));
    let second_counters = snapshot_work_counters();

    assert_eq!(first.render_generation, second.render_generation);
    assert_ne!(
        first.snapshot_ref().surfaces[0].bounds,
        second.snapshot_ref().surfaces[0].bounds
    );
    assert_ne!(
        first.scene_identity_signature(),
        second.scene_identity_signature()
    );
    assert_eq!(first_counters.snapshot_finalizations, 1);
    assert_eq!(second_counters.snapshot_finalizations, 1);
    assert_eq!(first_counters.identity_computations, 1);
    assert_eq!(second_counters.identity_computations, 1);
}

#[test]
fn stale_atomic_cursor_debt_is_cleared_without_clearing_software_work() {
    let mut arbitration = NativeCursorOutputArbitration::default();

    arbitration.request_hardware(7, 1_000, 2_000);
    arbitration.set_software_overlay_pending(true);
    arbitration.reconcile_hardware_cursor_liveness(false);

    assert!(arbitration.pending());
    assert_eq!(
        arbitration.disposition(2_000, false, false),
        NativeCursorOutputDisposition::SoftwareOverlay
    );
}

#[test]
fn cursor_submit_consumes_only_exact_queued_epoch() {
    let mut arbitration = NativeCursorOutputArbitration::default();
    arbitration.request(10, 1, 100);
    arbitration.request(11, 2, 100);

    arbitration.consume_submitted_epoch(10, 120, 200);

    assert!(arbitration.pending());
    assert_eq!(arbitration.desired_epoch(), 11);
    assert_eq!(arbitration.deadline_ns(), Some(200));

    arbitration.consume_submitted_epoch(11, 220, 300);
    assert!(!arbitration.pending());
}
