use super::frame::ResolvedNativeFrameScene;
use super::frame_scene_identity::filter_surface_scene_nodes;
use super::scene_history::{NativeFrameSceneSnapshot, NativeSceneHistory};
use super::*;
use oblivion_one::compositor::{
    AnimationTime, DecorationSceneSnapshot, PresentationFrameSnapshot, RenderableSurfaceDamage,
    SceneNodeId, SurfaceCommitSequence, SurfaceOpaqueRegion, SurfacePlacement,
    SurfaceRenderBackend,
};
use oblivion_one::core::OutputId;
use oblivion_one::render_backend::buffer::{BufferIdAllocator, BufferSize, CommittedSurfaceBuffer};
use oblivion_one::window_lifecycle_animation::LifecycleFrameSnapshot;
use wayland_server::protocol::wl_output;

fn node(raw: u64) -> SceneNodeId {
    SceneNodeId::from_raw(raw).expect("test scene node id")
}

fn surface(surface_id: u32, width: u32, height: u32) -> RenderableSurface {
    let buffer_id = BufferIdAllocator::default()
        .allocate()
        .expect("test buffer identity");
    RenderableSurface {
        surface_id,
        x: 0,
        y: 0,
        width,
        height,
        placement: SurfacePlacement::root_at(0, 0),
        render_backend: SurfaceRenderBackend::NativeWayland,
        render_placement: None,
        visual_clip: None,
        render_target_size: None,
        generation: 1,
        commit_sequence: SurfaceCommitSequence::initial(),
        buffer: CommittedSurfaceBuffer::shm_snapshot(
            buffer_id,
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

fn frame_snapshot(
    frame_id: u64,
    surface_id: u32,
    scene_node_id: SceneNodeId,
) -> NativeFrameSceneSnapshot {
    let surface = surface(surface_id, 32, 32);
    NativeFrameSceneSnapshot {
        output_id: OutputId::from_raw(1).expect("test output id"),
        frame_id,
        render_generation: frame_id,
        scene: NativeSceneSnapshot::from_surfaces_with_scene_nodes(
            std::slice::from_ref(&surface),
            std::slice::from_ref(&scene_node_id),
            Vec::new(),
            &[],
        ),
        cursor_damage: NativeCursorDamageBounds::default(),
        presentation: PresentationFrameSnapshot::empty(),
        lifecycle: LifecycleFrameSnapshot::default(),
    }
}

#[test]
fn filtering_active_surfaces_keeps_scene_node_identity_paired() {
    let surfaces = vec![surface(901, 10, 10), surface(902, 20, 20)];
    let (surfaces, nodes) = filter_surface_scene_nodes(
        std::borrow::Cow::Owned(surfaces),
        std::borrow::Cow::Owned(vec![node(7), node(8)]),
        |surface| surface.surface_id == 902,
    );

    assert_eq!(
        surfaces
            .iter()
            .map(|surface| surface.surface_id)
            .collect::<Vec<_>>(),
        [902]
    );
    assert_eq!(nodes.as_ref(), &[node(8)]);
}

#[test]
fn resolved_frame_freezes_the_active_scene_node_projection() {
    let socket_name = format!("typhon-frame-scene-identity-{}", std::process::id());
    let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name)
        .expect("bind compositor for frame identity regression");
    server.install_native_frame_test_scene(
        vec![surface(903, 32, 32)],
        &[(903, WindowId::from_raw(17).expect("test window id"))],
        None,
    );
    let expected = server.active_scene_surface_scene_nodes_in_order().to_vec();
    let resolved = ResolvedNativeFrameScene::from_server_at(&server, AnimationTime::from_nanos(0));

    assert_eq!(
        resolved.surfaces.len(),
        resolved.surface_scene_node_ids.len()
    );
    assert_eq!(
        resolved.surface_scene_node_ids.as_ref(),
        expected.as_slice()
    );
    assert_eq!(
        resolved
            .snapshot
            .surfaces
            .iter()
            .map(|surface| surface.scene_node_id)
            .collect::<Vec<_>>(),
        expected
    );

    let owned = resolved.into_owned();
    assert_eq!(owned.surface_scene_node_ids.as_ref(), expected.as_slice());
}

#[test]
fn direct_scanout_snapshot_retains_resolved_scene_node_identity() {
    let socket_name = format!("typhon-direct-scene-identity-{}", std::process::id());
    let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name)
        .expect("bind compositor for direct-scanout identity regression");
    server.install_native_frame_test_scene(
        vec![surface(911, 32, 32)],
        &[(911, WindowId::from_raw(19).expect("test window id"))],
        None,
    );
    let resolved = ResolvedNativeFrameScene::from_server_at(&server, AnimationTime::from_nanos(0));
    let direct_snapshot = resolved.snapshot_owned();

    assert_eq!(direct_snapshot.surfaces.len(), 1);
    assert_eq!(
        direct_snapshot.surfaces[0].scene_node_id,
        resolved.surface_scene_node_ids[0]
    );
}

#[test]
fn scene_node_identity_is_evidence_not_pixel_signature() {
    let rendered = surface(904, 32, 32);
    let first = NativeSceneSnapshot::from_surfaces_with_scene_nodes(
        std::slice::from_ref(&rendered),
        &[node(11)],
        Vec::new(),
        &[],
    );
    let second = NativeSceneSnapshot::from_surfaces_with_scene_nodes(
        std::slice::from_ref(&rendered),
        &[node(12)],
        Vec::new(),
        &[],
    );

    assert_eq!(first.identity_signature(), second.identity_signature());
    assert_ne!(
        first.surfaces[0].scene_node_id,
        second.surfaces[0].scene_node_id
    );
}

#[test]
fn scene_node_identity_does_not_create_visual_damage() {
    let rendered = surface(907, 32, 32);
    let first = NativeSceneSnapshot::from_surfaces_with_scene_nodes(
        std::slice::from_ref(&rendered),
        &[node(41)],
        Vec::new(),
        &[],
    );
    let second = NativeSceneSnapshot::from_surfaces_with_scene_nodes(
        std::slice::from_ref(&rendered),
        &[node(42)],
        Vec::new(),
        &[],
    );
    let cursor = NativeClientCursorDamageState {
        surface_id: 908,
        scene_node_id: node(43),
        generation: 1,
        hotspot_x: 0,
        hotspot_y: 0,
        rect: None,
    };
    let mut cursor_with_other_node = cursor;
    cursor_with_other_node.scene_node_id = node(44);

    let damage = native_output_damage_for_scene_snapshots(
        64,
        64,
        &first,
        &second,
        NativeCursorDamageBounds {
            previous_client: Some(cursor),
            client: Some(cursor_with_other_node),
            ..NativeCursorDamageBounds::default()
        },
    );
    assert_eq!(damage.summary().kind, NativeDamageKind::Empty);
}

#[test]
fn only_physical_promotion_replaces_presented_scene_node_evidence() {
    let first_node = node(21);
    let second_node = node(22);
    let mut history = NativeSceneHistory::new(frame_snapshot(1, 901, first_node));

    assert_eq!(
        history.presented_scene_node_for_surface(901),
        Some(first_node),
    );
    history.replace_ready(frame_snapshot(2, 901, second_node));
    assert_eq!(
        history.presented_scene_node_for_surface(901),
        Some(first_node),
    );
    assert!(history.queue_submission(77));
    assert_eq!(
        history.submitted_scene_node_for_surface(77, 901),
        Some(second_node),
    );
    assert_eq!(
        history.presented_scene_node_for_surface(901),
        Some(first_node),
    );
    assert!(history.promote_pageflip(77));
    assert_eq!(
        history.presented_scene_node_for_surface(901),
        Some(second_node),
    );
}

#[test]
fn discarded_or_wrong_pageflip_evidence_never_becomes_presented() {
    let first_node = node(51);
    let second_node = node(52);
    let mut history = NativeSceneHistory::new(frame_snapshot(1, 909, first_node));

    history.replace_ready(frame_snapshot(2, 909, second_node));
    history.discard_ready();
    assert_eq!(
        history.presented_scene_node_for_surface(909),
        Some(first_node)
    );

    history.replace_ready(frame_snapshot(2, 909, second_node));
    assert!(history.queue_submission(88));
    assert!(!history.promote_pageflip(89));
    assert_eq!(
        history.presented_scene_node_for_surface(909),
        Some(first_node)
    );
    assert!(history.discard_submission(88));
    assert_eq!(
        history.presented_scene_node_for_surface(909),
        Some(first_node)
    );
}

#[test]
fn physical_helpers_expose_decoration_and_cursor_scene_nodes() {
    let window_id = WindowId::from_raw(23).expect("test window id");
    let decoration_node = node(31);
    let cursor_node = node(32);
    let surface = surface(905, 32, 32);
    let decoration = DecorationSceneSnapshot::from_bounds_with_scene_node(
        decoration_node,
        window_id,
        surface.surface_id,
        0,
        0,
        32,
        32,
        1,
    );
    let cursor = NativeClientCursorDamageState {
        surface_id: 906,
        scene_node_id: cursor_node,
        generation: 1,
        hotspot_x: 0,
        hotspot_y: 0,
        rect: None,
    };
    let snapshot = NativeFrameSceneSnapshot {
        output_id: OutputId::from_raw(1).expect("test output id"),
        frame_id: 1,
        render_generation: 1,
        scene: NativeSceneSnapshot::from_surfaces_with_scene_nodes(
            std::slice::from_ref(&surface),
            &[node(33)],
            vec![decoration],
            &[],
        ),
        cursor_damage: NativeCursorDamageBounds {
            client: Some(cursor),
            ..NativeCursorDamageBounds::default()
        },
        presentation: PresentationFrameSnapshot::empty(),
        lifecycle: LifecycleFrameSnapshot::default(),
    };
    let history = NativeSceneHistory::new(snapshot);

    assert_eq!(
        history.presented_decoration_scene_node(window_id),
        Some(decoration_node)
    );
    assert_eq!(
        history.presented_client_cursor_scene_node(),
        Some(cursor_node)
    );
}
