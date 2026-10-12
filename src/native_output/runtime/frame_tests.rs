use super::{
    FrozenNativeRenderFrame, FrozenRenderSnapshotIdentity, OwnedClientCursorRenderState,
    project_egl_scene_draw_request,
};
use crate::egl_renderer::OutputDamage;
use crate::native_output::runtime::frame::{
    NativeCursorOutputArbitration, NativeCursorOutputDisposition, NativeCursorRenderMode,
    NativeCursorSchedulingPolicy, NativeFrameRenderer, ResolvedNativeFrameScene,
};
use crate::native_output::runtime::frame_scene_identity::{
    finalize_snapshot, reset_snapshot_work_counters, snapshot_work_counters, visibility_signature,
};
use crate::native_output::runtime::{NativeInputState, NativeSceneSnapshot};
use oblivion_one::compositor::{
    AnimationTime, ClientCursorRenderState, DesktopVisualState, FullscreenRenderPlanMetrics,
    PresentationRect, PresentationRetainedVisualPayloadId, RenderableSurface,
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
use oblivion_one::presentation_animation::{PresentationEngine, PresentationRetainedVisualKind};
use oblivion_one::render_backend::buffer::{BufferIdAllocator, BufferSize, CommittedSurfaceBuffer};
use oblivion_one::window_lifecycle_animation::{
    LifecycleDirection, LifecycleEffectKind, LifecycleFrameSnapshot, LifecycleSceneSample,
    LifecycleVisualGroup, LifecycleVisualSource, LifecycleVisualSourceKind, LifecycleWindowSample,
};
use std::borrow::Cow;
use std::process;
use wayland_server::Resource;
use wayland_server::protocol::wl_output;

fn assert_send<T: Send>() {}

#[test]
fn frozen_native_render_frame_is_send() {
    assert_send::<FrozenNativeRenderFrame>();
}

#[test]
fn frozen_native_render_frame_keeps_scene_visual_journal_lifecycle_and_damage_at_g1() {
    let socket_name = format!("typhon-frozen-render-g1-{}", process::id());
    let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name)
        .expect("bind compositor for frozen render isolation regression");
    server.install_native_frame_test_scene(
        vec![test_surface(
            710,
            320,
            200,
            SurfacePlacement::root_at(10, 20),
        )],
        &[(710, WindowId::from_raw(710).expect("G1 window id"))],
        None,
    );
    let scene = ResolvedNativeFrameScene::from_server_at(&server, AnimationTime::from_nanos(11))
        .into_owned();
    let render_generation = scene.render_generation;
    let scene_signature = scene.scene_identity_signature();
    let effects_generation = scene.effects.generation;
    let presentation_signature = scene.presentation.presentation_visual_signature();
    let lifecycle_signature = scene.lifecycle_snapshot.signature;
    let sync_s1 = server.record_surface_damage_for_render_snapshot_test(
        710,
        RenderableSurfaceDamage::Full,
        320,
        200,
    );
    assert_eq!(sync_s1, 1);
    let mut input_state = NativeInputState::new(1280, 800);
    input_state.cursor_x = 41.0;
    input_state.cursor_y = 52.0;

    reset_snapshot_work_counters();
    let mut frozen = FrozenNativeRenderFrame::freeze(
        1280,
        800,
        scene,
        &server,
        &input_state,
        NativeCursorRenderMode::Software,
        Some(OutputDamage::Full),
    );
    assert_eq!(
        snapshot_work_counters().snapshot_owned_clones,
        0,
        "freeze does not call snapshot_owned() and clone another NativeSceneSnapshot"
    );
    let frozen_identity = frozen.identity;
    let resource_sync_states = frozen.surface_resource_sync_states.clone();
    let resource_sync_signature = frozen.identity.surface_resource_sync_signature;
    assert_eq!(
        frozen_identity.visual_state,
        DesktopVisualState::with_cursor(41, 52)
    );
    assert_eq!(frozen_identity.lifecycle_signature, lifecycle_signature);
    assert_eq!(
        frozen_identity.surface_resource_sync_signature,
        resource_sync_signature
    );
    let frozen_sync_s1 = resource_sync_states
        .iter()
        .find(|state| state.surface_id == 710)
        .expect("G1 journal state is frozen");
    assert!(frozen_sync_s1.authoritative);
    assert_eq!(frozen_sync_s1.current_commit.0, sync_s1);

    let lifecycle_bounds =
        PresentationRect::new(10.0, 20.0, 320.0, 200.0).expect("lifecycle test bounds");
    let lifecycle_group = LifecycleVisualGroup::from_bounds(
        lifecycle_bounds,
        lifecycle_bounds,
        lifecycle_bounds,
        lifecycle_bounds,
        1280,
        800,
    )
    .expect("lifecycle test visual group");
    assert!(server.begin_lifecycle_lamp_for_test(
        WindowId::from_raw(710).expect("G1 lifecycle window id"),
        710,
        lifecycle_group,
    ));
    server.install_native_frame_test_scene(
        vec![test_surface(
            711,
            640,
            400,
            SurfacePlacement::root_at(30, 40),
        )],
        &[(711, WindowId::from_raw(711).expect("G2 window id"))],
        None,
    );
    let sync_s2 = server.record_surface_damage_for_render_snapshot_test(
        710,
        RenderableSurfaceDamage::Empty,
        320,
        200,
    );
    assert_eq!(sync_s2, sync_s1 + 1);
    input_state.cursor_x = 91.0;
    input_state.cursor_y = 92.0;

    let request = frozen
        .draw_request(Some(17))
        .expect("the frozen render projection is available once");
    assert_eq!(request.frame_id, Some(17));
    assert_eq!(
        request.visual_state,
        DesktopVisualState::with_cursor(41, 52)
    );
    assert_eq!(frozen_identity.visual_state, request.visual_state);
    assert_eq!(
        request
            .surfaces
            .iter()
            .map(|surface| surface.surface_id)
            .collect::<Vec<_>>(),
        [710]
    );
    assert_eq!(request.render_generation, Some(render_generation));
    assert_eq!(request.scene_signature, scene_signature);
    assert_eq!(request.scene_generation, effects_generation);
    assert_eq!(
        request.presentation_visual_signature,
        presentation_signature
    );
    assert_eq!(
        frozen_identity.presentation_visual_signature, request.presentation_visual_signature,
        "frozen identity and draw request share the captured signature"
    );
    assert_eq!(request.lifecycle.sampled_at, AnimationTime::from_nanos(11));
    assert!(request.lifecycle.samples.is_empty());
    assert_eq!(request.surface_resource_sync_states, resource_sync_states);
    assert_eq!(
        frozen_identity.surface_resource_sync_signature,
        resource_sync_signature
    );
    assert_eq!(request.current_damage, Some(OutputDamage::Full));
    assert_eq!(
        snapshot_work_counters().snapshot_owned_clones,
        0,
        "projection does not clone another NativeSceneSnapshot"
    );
    drop(request);

    assert!(
        frozen.draw_request(None).is_none(),
        "one frozen frame cannot replay stale damage authority"
    );
}

#[test]
fn owned_client_cursor_keeps_the_frozen_commit_and_placement() {
    let socket_name = format!("typhon-frozen-cursor-commit-{}", process::id());
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name)
        .expect("bind compositor for frozen client cursor regression");
    let mut allocator = BufferIdAllocator::default();
    let commit_c1 = allocator.allocate().expect("C1 buffer identity");
    let commit_c2 = allocator.allocate().expect("C2 buffer identity");
    let c1_buffer_id = commit_c1.id();
    let c2_buffer_id = commit_c2.id();
    let mut c1 = test_surface(720, 32, 32, SurfacePlacement::root_at(0, 0));
    c1.generation = 4;
    c1.commit_sequence = SurfaceCommitSequence(8);
    c1.buffer = CommittedSurfaceBuffer::shm_snapshot(
        commit_c1,
        BufferSize::new(32, 32).expect("C1 buffer size"),
        vec![0x11; 32 * 32],
    );
    let cursor = ClientCursorRenderState {
        surface: &c1,
        scene_node_id: oblivion_one::core::SceneNodeId::from_raw(720)
            .expect("client cursor scene node"),
        logical_x: 101,
        logical_y: 202,
        hotspot_x: 3,
        hotspot_y: 5,
    };
    let owned = OwnedClientCursorRenderState::from_render_state(cursor);
    let scene = ResolvedNativeFrameScene::from_server_at(&server, AnimationTime::from_nanos(12))
        .into_owned();
    let mut frozen = FrozenNativeRenderFrame::from_snapshot_parts(
        1280,
        800,
        scene,
        DesktopVisualState::wallpaper_only(),
        Some(owned),
        Vec::new(),
        None,
    );

    c1.generation = 5;
    c1.commit_sequence = SurfaceCommitSequence(9);
    c1.buffer = CommittedSurfaceBuffer::shm_snapshot(
        commit_c2,
        BufferSize::new(32, 32).expect("C2 buffer size"),
        vec![0x22; 32 * 32],
    );
    let request = frozen
        .draw_request(None)
        .expect("cursor snapshot projection is available");
    let projected = request
        .client_cursor
        .expect("C1 client cursor remains visible");

    assert_eq!(projected.surface.buffer_id(), c1_buffer_id);
    assert_eq!(projected.surface.generation, 4);
    assert_eq!(projected.surface.commit_sequence, SurfaceCommitSequence(8));
    assert_eq!((projected.logical_x, projected.logical_y), (101, 202));
    assert_eq!((projected.hotspot_x, projected.hotspot_y), (3, 5));
    assert_ne!(projected.surface.buffer_id(), c1.buffer_id());
    assert_eq!(c1.buffer_id(), c2_buffer_id);
}

fn frozen_client_cursor_identity_for_placement(
    server: &OwnCompositorServer,
    surface: &RenderableSurface,
    logical_x: i32,
    logical_y: i32,
    hotspot_x: i32,
    hotspot_y: i32,
) -> FrozenRenderSnapshotIdentity {
    let state = ClientCursorRenderState {
        surface,
        scene_node_id: oblivion_one::core::SceneNodeId::from_raw(729)
            .expect("client cursor scene node"),
        logical_x,
        logical_y,
        hotspot_x,
        hotspot_y,
    };
    let scene = ResolvedNativeFrameScene::from_server_at(server, AnimationTime::from_nanos(12))
        .into_owned();
    FrozenNativeRenderFrame::from_snapshot_parts(
        1280,
        800,
        scene,
        DesktopVisualState::wallpaper_only(),
        Some(OwnedClientCursorRenderState::from_render_state(state)),
        Vec::new(),
        None,
    )
    .identity
}

#[test]
fn frozen_identity_distinguishes_client_cursor_position() {
    let socket_name = format!("typhon-frozen-cursor-position-{}", process::id());
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name)
        .expect("bind compositor for frozen cursor position identity regression");
    let surface = test_surface(729, 32, 32, SurfacePlacement::root_at(0, 0));

    let position_a = frozen_client_cursor_identity_for_placement(&server, &surface, 10, 20, 3, 5);
    let position_b = frozen_client_cursor_identity_for_placement(&server, &surface, 11, 20, 3, 5);

    assert_eq!(
        position_a.client_cursor.unwrap().buffer_id,
        position_b.client_cursor.unwrap().buffer_id,
        "both frames refer to the same committed cursor content"
    );
    assert_ne!(position_a, position_b);
}

#[test]
fn frozen_identity_distinguishes_client_cursor_hotspot() {
    let socket_name = format!("typhon-frozen-cursor-hotspot-{}", process::id());
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name)
        .expect("bind compositor for frozen cursor hotspot identity regression");
    let surface = test_surface(729, 32, 32, SurfacePlacement::root_at(0, 0));

    let hotspot_a = frozen_client_cursor_identity_for_placement(&server, &surface, 10, 20, 3, 5);
    let hotspot_b = frozen_client_cursor_identity_for_placement(&server, &surface, 10, 20, 4, 5);

    assert_eq!(
        hotspot_a.client_cursor.unwrap().buffer_id,
        hotspot_b.client_cursor.unwrap().buffer_id,
        "both frames refer to the same committed cursor content"
    );
    assert_ne!(hotspot_a, hotspot_b);
}

#[test]
fn freezing_a_render_frame_does_not_capture_or_complete_frame_callbacks() {
    let socket_name = format!("typhon-frozen-callback-ownership-{}", process::id());
    let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name)
        .expect("bind compositor for frozen frame callback regression");
    server.install_native_frame_test_scene(
        vec![test_surface(725, 320, 200, SurfacePlacement::root_at(0, 0))],
        &[(
            725,
            WindowId::from_raw(725).expect("callback test window id"),
        )],
        None,
    );
    let (callback, _client_peer) = server.test_queue_visible_frame_callback(725);
    assert!(server.has_pending_frame_callbacks());

    let scene = ResolvedNativeFrameScene::from_server_at(&server, AnimationTime::from_nanos(14))
        .into_owned();
    let input_state = NativeInputState::new(1280, 800);
    let frozen = FrozenNativeRenderFrame::freeze(
        1280,
        800,
        scene,
        &server,
        &input_state,
        NativeCursorRenderMode::Hardware,
        Some(OutputDamage::Full),
    );

    assert!(server.has_pending_frame_callbacks());
    assert!(callback.is_alive());
    drop(frozen);
    assert!(server.has_pending_frame_callbacks());
    assert!(callback.is_alive());
}

#[test]
fn frozen_draw_request_matches_the_synchronous_request_projection() {
    let socket_name = format!("typhon-frozen-render-equivalence-{}", process::id());
    let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name)
        .expect("bind compositor for frozen request equivalence regression");
    server.install_native_frame_test_scene_with_server_decorations(
        vec![test_surface(
            730,
            320,
            200,
            SurfacePlacement::root_at(40, 50),
        )],
        &[(730, WindowId::from_raw(730).expect("equivalence window id"))],
        None,
    );
    let at = AnimationTime::from_nanos(13);
    let old_scene = ResolvedNativeFrameScene::from_server_at(&server, at);
    let frozen_scene = ResolvedNativeFrameScene::from_server_at(&server, at).into_owned();
    let input_state = NativeInputState::new(1280, 800);
    let mut renderer = NativeFrameRenderer::default();
    let synchronous_seed = renderer.egl_scene_draw_request(
        1280,
        800,
        &old_scene,
        &server,
        &input_state,
        NativeCursorRenderMode::Software,
        Some(OutputDamage::Full),
    );

    let mut cursor_surface = test_surface(731, 32, 32, SurfacePlacement::root_at(0, 0));
    cursor_surface.generation = 19;
    cursor_surface.commit_sequence = SurfaceCommitSequence(27);
    let cursor_node_id =
        oblivion_one::core::SceneNodeId::from_raw(731).expect("equivalence cursor scene node");
    let cursor_logical_position = (113, 227);
    let cursor_hotspot = (4, 7);
    let synchronous_cursor = ClientCursorRenderState {
        surface: &cursor_surface,
        scene_node_id: cursor_node_id,
        logical_x: cursor_logical_position.0,
        logical_y: cursor_logical_position.1,
        hotspot_x: cursor_hotspot.0,
        hotspot_y: cursor_hotspot.1,
    };
    let presentation_visual_signature = synchronous_seed.presentation_visual_signature;
    let visual_state = synchronous_seed.visual_state;
    let surface_resource_sync_states = synchronous_seed.surface_resource_sync_states;
    // The compatibility builder supplies a borrowed ClientCursorRenderState
    // to this same projection helper. Provide one directly so the equivalence
    // check exercises a visible software cursor without Wayland client setup.
    let old = project_egl_scene_draw_request(
        1280,
        800,
        &old_scene,
        presentation_visual_signature,
        visual_state,
        Some(synchronous_cursor),
        Some(OutputDamage::Full),
        surface_resource_sync_states.clone(),
        None,
    );
    let mut frozen = FrozenNativeRenderFrame::from_snapshot_parts(
        1280,
        800,
        frozen_scene,
        visual_state,
        Some(OwnedClientCursorRenderState::from_render_state(
            synchronous_cursor,
        )),
        surface_resource_sync_states,
        Some(OutputDamage::Full),
    );
    let frozen_identity = frozen.identity;
    let current = frozen
        .draw_request(None)
        .expect("frozen request projection is available");

    assert_eq!(current.surfaces.len(), old.surfaces.len());
    for (left, right) in current.surfaces.iter().zip(old.surfaces) {
        assert_eq!(left.surface_id, right.surface_id);
        assert_eq!(left.buffer_id(), right.buffer_id());
        assert_eq!(left.generation, right.generation);
        assert_eq!(left.commit_sequence, right.commit_sequence);
    }
    assert_eq!(
        current.external_overlay_surface_ids,
        old.external_overlay_surface_ids
    );
    assert_eq!(current.popup_surface_ids, old.popup_surface_ids);
    assert_eq!(current.content_generation, old.content_generation);
    assert_eq!(current.render_generation, old.render_generation);
    assert_eq!(current.scene_generation, old.scene_generation);
    assert_eq!(current.scene_signature, old.scene_signature);
    assert_eq!(current.visual_state, old.visual_state);
    assert_eq!(
        current.decoration_instances.len(),
        old.decoration_instances.len()
    );
    assert_eq!(
        current
            .decoration_instances
            .iter()
            .map(|item| item.scene_snapshot().identity())
            .collect::<Vec<_>>(),
        old.decoration_instances
            .iter()
            .map(|item| item.scene_snapshot().identity())
            .collect::<Vec<_>>(),
    );
    assert_eq!(current.effects.signature, old.effects.signature);
    assert_eq!(current.effects.generation, old.effects.generation);
    assert_eq!(
        current.presentation_visual_signature,
        old.presentation_visual_signature
    );
    assert_eq!(current.presentation_opacities, old.presentation_opacities);
    assert_eq!(current.presentation_clips, old.presentation_clips);
    assert_eq!(
        current.presentation_owner_root_surface_ids,
        old.presentation_owner_root_surface_ids,
    );
    assert_eq!(
        current.surface_resource_sync_states,
        old.surface_resource_sync_states
    );
    assert_eq!(current.lifecycle.sampled_at, old.lifecycle.sampled_at);
    assert_eq!(
        current.lifecycle_surfaces.len(),
        old.lifecycle_surfaces.len()
    );
    assert_eq!(
        current.lifecycle_decorations.len(),
        old.lifecycle_decorations.len()
    );
    assert_eq!(current.current_damage, old.current_damage);
    assert_eq!(
        frozen_identity.presentation_visual_signature,
        current.presentation_visual_signature
    );
    let current_cursor = current
        .client_cursor
        .expect("frozen request contains the software client cursor");
    let synchronous_cursor = old
        .client_cursor
        .expect("synchronous request contains the software client cursor");
    assert_eq!(
        current_cursor.surface.surface_id,
        synchronous_cursor.surface.surface_id
    );
    assert_eq!(
        current_cursor.scene_node_id,
        synchronous_cursor.scene_node_id
    );
    assert_eq!(
        current_cursor.surface.generation,
        synchronous_cursor.surface.generation
    );
    assert_eq!(
        current_cursor.surface.commit_sequence,
        synchronous_cursor.surface.commit_sequence
    );
    assert_eq!(
        current_cursor.surface.buffer_id(),
        synchronous_cursor.surface.buffer_id()
    );
    assert_eq!(
        (current_cursor.logical_x, current_cursor.logical_y),
        (synchronous_cursor.logical_x, synchronous_cursor.logical_y)
    );
    assert_eq!(
        (current_cursor.hotspot_x, current_cursor.hotspot_y),
        (synchronous_cursor.hotspot_x, synchronous_cursor.hotspot_y)
    );
    assert_eq!(current_cursor.surface.surface_id, 731);
    assert_eq!(current_cursor.scene_node_id, cursor_node_id);
    assert_eq!(current_cursor.surface.generation, 19);
    assert_eq!(
        current_cursor.surface.commit_sequence,
        SurfaceCommitSequence(27)
    );
    assert_eq!(
        current_cursor.surface.buffer_id(),
        cursor_surface.buffer_id()
    );
    assert_eq!(
        (current_cursor.logical_x, current_cursor.logical_y),
        cursor_logical_position
    );
    assert_eq!(
        (current_cursor.hotspot_x, current_cursor.hotspot_y),
        cursor_hotspot
    );
}

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
fn resolved_native_frame_scene_carries_workspace_presentation_sample() {
    let socket_name = format!("typhon-workspace-frame-sample-{}", process::id());
    let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name)
        .expect("bind compositor for workspace-frame sample regression");
    server
        .set_animation_configuration(oblivion_one::animation_control::AnimationConfiguration {
            enabled: true,
            overrides: std::collections::BTreeMap::from([(
                oblivion_one::animation_control::AnimationSlot::WorkspaceSwitch,
                oblivion_one::animation_control::AnimationEffect::WorkspaceSlide,
            )]),
            ..oblivion_one::animation_control::AnimationConfiguration::default()
        })
        .expect("enable workspace slide for frame-scene regression");
    server.install_native_frame_test_scene(
        vec![test_surface(
            691,
            320,
            200,
            SurfacePlacement::absolute_root_at(20, 30),
        )],
        &[],
        None,
    );
    let target = oblivion_one::wm::WorkspaceId::new(2).expect("workspace id");
    assert!(matches!(
        server.activate_workspace(target),
        oblivion_one::wm::WorkspaceSwitchOutcome::Changed { current, .. }
            if current == target
    ));

    let resolved = ResolvedNativeFrameScene::from_server(&server);
    let workspace_sample = resolved
        .workspace_presentation_sample
        .as_ref()
        .expect("active workspace transition is sampled in the frame scene");

    assert_eq!(workspace_sample.target_workspace, target);
    assert_eq!(resolved.surface_ids().collect::<Vec<_>>(), [691]);
}

#[test]
fn workspace_translation_moves_frame_geometry_without_changing_client_extent() {
    let mut surfaces = vec![
        test_surface(901, 320, 200, SurfacePlacement::absolute_root_at(20, 30)),
        test_surface(902, 640, 360, SurfacePlacement::absolute_root_at(40, 50)),
    ];
    let presented_size = BufferSize::new(720, 405).expect("presented render target size");
    surfaces[1].render_target_size = Some(presented_size);
    let offsets = std::collections::HashMap::from([(901, (-480.0, 0.0)), (902, (1440.0, 0.0))]);

    let translated =
        super::translate_workspace_frame_surfaces(Cow::Owned(surfaces), &[901, 902], &offsets);

    assert_eq!(
        oblivion_one::compositor::surface_origins(translated.as_ref()),
        [(-460, 30), (1480, 50)]
    );
    assert_eq!((translated[0].width, translated[0].height), (320, 200));
    assert_eq!((translated[1].width, translated[1].height), (640, 360));
    assert_eq!(translated[1].render_target_size, Some(presented_size));
}

#[test]
fn workspace_cell_clip_intersects_an_existing_presentation_clip() {
    let output_id = oblivion_one::core::OutputId::from_raw(1).expect("test output id");
    let root_surface_id = 912;
    let scene_node_id = oblivion_one::core::SceneNodeId::from_raw(912).expect("scene node id");
    let existing_clip =
        oblivion_one::presentation_animation::PresentationClipRect::new(10.0, 10.0, 50.0, 50.0)
            .expect("existing clip");
    let workspace_clip =
        oblivion_one::presentation_animation::PresentationClipRect::new(40.0, 0.0, 50.0, 80.0)
            .expect("workspace cell clip");
    let mut presentation = PresentationSceneSample::empty_for_output(
        output_id,
        AnimationTime::from_nanos(1),
        oblivion_one::compositor::PresentationSampleTimeSource::ZeroFallback,
    );
    presentation.clips.push(
        oblivion_one::presentation_animation::PresentationGroupClip {
            scene_node_id,
            root_surface_id,
            clip: oblivion_one::presentation_animation::PresentationClip::Rect(existing_clip),
            presented_clip: Some(existing_clip),
            transition: None,
        },
    );

    super::merge_workspace_presentation_clips(
        &mut presentation,
        &std::collections::HashMap::from([(root_surface_id, (scene_node_id, workspace_clip))]),
    );

    assert_eq!(presentation.clips.len(), 1);
    assert_eq!(
        presentation.clips[0].presented_clip,
        Some(
            oblivion_one::presentation_animation::PresentationClipRect::new(
                40.0, 10.0, 20.0, 50.0,
            )
            .expect("intersection clip")
        )
    );
}

#[test]
fn synchronous_modeset_candidate_is_a_client_free_transition_projection() {
    let socket_name = format!("typhon-frame-modeset-candidate-{}", process::id());
    let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name)
        .expect("bind compositor for synchronous modeset projection");
    let surfaces = vec![
        test_surface(631, 320, 200, SurfacePlacement::root_at(80, 70)),
        test_surface(632, 1280, 800, SurfacePlacement::absolute_root_at(0, 0)),
    ];
    server.install_native_frame_test_scene(
        surfaces,
        &[
            (631, WindowId::from_raw(631).expect("surface window id")),
            (632, WindowId::from_raw(632).expect("fullscreen window id")),
        ],
        None,
    );
    let (callback, _client_peer) = server.test_queue_visible_frame_callback(631);
    assert!(server.has_pending_frame_callbacks());
    let ordinary = ResolvedNativeFrameScene::from_server_at(&server, AnimationTime::from_nanos(7));
    assert_eq!(ordinary.surface_ids().count(), 2);

    let candidate = ResolvedNativeFrameScene::for_synchronous_output_reconfiguration(&server);

    assert_eq!(candidate.surface_ids().count(), 0);
    assert!(candidate.decorations.is_empty());
    assert!(candidate.effects.instances.is_empty());
    assert!(candidate.presentation.windows.is_empty());
    assert!(candidate.lifecycle.samples.is_empty());
    assert!(candidate.lifecycle_surfaces.is_empty());
    assert!(candidate.window_exit_evidence.is_empty());
    assert!(server.has_pending_frame_callbacks());
    assert!(callback.is_alive());
}

#[test]
fn restore_filters_canonical_surfaces_from_the_frame_lifecycle_sample() {
    let socket_name = format!("typhon-frame-restore-suppression-{}", process::id());
    let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name)
        .expect("bind compositor for Restore suppression regression");
    let root_surface_id = 621;
    let window_id = WindowId::from_raw(621).expect("restore window id");
    server.install_native_frame_test_scene(
        vec![test_surface(
            root_surface_id,
            320,
            200,
            SurfacePlacement::root_at(30, 40),
        )],
        &[(root_surface_id, window_id)],
        None,
    );
    let visual = PresentationRect::new(30.0, 40.0, 320.0, 200.0).unwrap();
    let anchor = PresentationRect::new(600.0, 400.0, 32.0, 32.0).unwrap();
    let visual_group = LifecycleVisualGroup::from_bounds(visual, visual, visual, anchor, 1280, 800)
        .expect("valid restore visual group");
    let at = AnimationTime::from_nanos(42);
    let identity = PresentationEngine::enabled()
        .begin_retained_visual(
            oblivion_one::core::SceneNodeId::from_raw(window_id.get())
                .expect("test lifecycle scene node"),
            PresentationRetainedVisualKind::WindowLifecycle,
            at,
        )
        .expect("test retained lifecycle identity");
    let lifecycle = LifecycleSceneSample {
        sampled_at: at,
        samples: vec![LifecycleWindowSample {
            window_id,
            root_surface_id,
            presentation_identity: identity,
            payload_id: PresentationRetainedVisualPayloadId::from_origin_identity(identity),
            painter_order: Default::default(),
            visual_group,
            visual_source: LifecycleVisualSource {
                window_id,
                root_surface_id,
                presentation_identity: identity,
                payload_id: PresentationRetainedVisualPayloadId::from_origin_identity(identity),
                resource_payload_id: PresentationRetainedVisualPayloadId::from_origin_identity(
                    identity,
                ),
                kind: LifecycleVisualSourceKind::NoOwnedEffects,
                effect_scene: std::sync::Arc::new(ResolvedEffectScene::default()),
                render_layout: None,
            },
            effect: LifecycleEffectKind::Lamp,
            progress: 0.5,
            effect_opacity: 1.0,
            direction: LifecycleDirection::Restore,
            mathematically_settled: true,
        }],
        minimized_visual_resource_payload_ids: Vec::new(),
    };
    let resolved = ResolvedNativeFrameScene::from_server_at_with_lifecycle(
        &server,
        at,
        oblivion_one::compositor::PresentationSampleTimeSource::MonotonicFallback,
        lifecycle,
    );

    assert_eq!(resolved.lifecycle.samples.len(), 1);
    assert_eq!(
        resolved.lifecycle.samples[0].direction,
        LifecycleDirection::Restore
    );
    assert_eq!(resolved.lifecycle.sampled_at, at);
    assert_eq!(
        resolved.lifecycle.samples[0].root_surface_id,
        root_surface_id
    );
    assert!(
        !resolved
            .surface_ids()
            .any(|surface_id| surface_id == root_surface_id)
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
            presentation_owner_root_surface_ids: Cow::Borrowed(&[]),
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
            canonical_scene_evidence: None,
            scene_identity_signature: 0,
            workspace_presentation_sample: None,
            effects: ResolvedEffectScene::default(),
            effect_scene_resolution_ns: None,
            effect_scene_visible_instances: 0,
            presentation: PresentationSceneSample::empty_for_output(
                output_id,
                AnimationTime::from_nanos(0),
                oblivion_one::compositor::PresentationSampleTimeSource::ZeroFallback,
            ),
            presentation_snapshot: PresentationFrameSnapshot::empty_for_output(output_id),
            lifecycle: LifecycleSceneSample {
                sampled_at: AnimationTime::from_nanos(0),
                samples: Vec::new(),
                minimized_visual_resource_payload_ids: Vec::new(),
            },
            lifecycle_marker_insertions: Vec::new(),
            lifecycle_surfaces: Vec::new(),
            lifecycle_decorations: Vec::new(),
            lifecycle_snapshot: LifecycleFrameSnapshot::default(),
            window_exit_evidence: Vec::new(),
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
            coverage: None,
            signature: 17,
            frame_demand: EffectFrameDemand::OnDamage,
            visual_group: None,
            anchor_scope: EffectAnchorScope::VisualGroup,
            backdrop_reuse: oblivion_one::compositor::EffectBackdropReuse::Exact,
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
    assert_eq!(arbitration.deadline_ns(), Some(100));
    assert_eq!(arbitration.pending_since_ns(), Some(1));
    assert_eq!(arbitration.pending_age_ns(120), Some(119));

    arbitration.consume_submitted_epoch(11, 220, 300);
    assert!(!arbitration.pending());
}

#[test]
fn busy_retry_moves_transport_retry_without_sliding_cursor_service_deadline() {
    let mut arbitration = NativeCursorOutputArbitration::default();
    arbitration.request_hardware(1, 100, 200);

    for (busy_at_ns, retry_not_before_ns) in [(500, 600), (700, 800), (900, 1_000)] {
        arbitration.note_kms_busy_retry();
        arbitration.set_retry_not_before(retry_not_before_ns);

        assert_eq!(arbitration.deadline_ns(), Some(200));
        assert_eq!(arbitration.pending_since_ns(), Some(100));
        assert_eq!(
            arbitration.pending_age_ns(busy_at_ns),
            Some(busy_at_ns - 100)
        );
        assert_eq!(arbitration.retry_not_before_ns(), Some(retry_not_before_ns));
        assert!(!arbitration.due(retry_not_before_ns - 1));
        assert!(arbitration.due(retry_not_before_ns));
        assert_eq!(
            arbitration.wake_deadline_ns(busy_at_ns),
            Some(retry_not_before_ns)
        );
        assert_eq!(arbitration.wake_deadline_ns(retry_not_before_ns), None);
    }
    assert_eq!(arbitration.kms_busy_retries, 3);
}

#[test]
fn matured_cursor_deadline_remains_due_without_requesting_a_past_wake() {
    let mut arbitration = NativeCursorOutputArbitration::default();
    arbitration.request_hardware(1, 10, 20);

    assert_eq!(arbitration.wake_deadline_ns(19), Some(20));
    assert!(arbitration.due(20));
    assert_eq!(arbitration.wake_deadline_ns(20), None);
    assert_eq!(arbitration.wake_deadline_ns(100), None);
    assert!(arbitration.pending());
}

#[test]
fn auto_prefers_ready_carriers_and_never_steals_owned_or_busy_lanes() {
    use super::NativeCursorPrimaryState;

    let mut ready_arbitration = NativeCursorOutputArbitration::default();
    ready_arbitration.request_hardware(1, 10, 20);
    assert_eq!(
        super::cursor_output_disposition_for_primary_state(
            &mut ready_arbitration,
            NativeCursorSchedulingPolicy::Auto,
            20,
            NativeCursorPrimaryState {
                ready_primary_carrier_attachable: true,
                primary_scene_work_pending: true,
                independent_kms_admission: false,
                ..Default::default()
            },
            true,
        ),
        NativeCursorOutputDisposition::PiggybackPrimary
    );

    let mut queued_carrier_arbitration = NativeCursorOutputArbitration::default();
    queued_carrier_arbitration.request_hardware(3, 10, 20);
    assert_eq!(
        super::cursor_output_disposition_for_primary_state(
            &mut queued_carrier_arbitration,
            NativeCursorSchedulingPolicy::Auto,
            20,
            NativeCursorPrimaryState {
                primary_scene_work_pending: true,
                ready_primary_carrier_attachable: true,
                primary_worker_queued: true,
                independent_kms_admission: false,
                ..Default::default()
            },
            true,
        ),
        NativeCursorOutputDisposition::PiggybackPrimary
    );

    for (target_owned, lane_available) in [(true, true), (false, false)] {
        let mut arbitration = NativeCursorOutputArbitration::default();
        arbitration.request_hardware(2, 10, 20);
        assert_eq!(
            super::cursor_output_disposition_for_primary_state(
                &mut arbitration,
                NativeCursorSchedulingPolicy::Auto,
                20,
                NativeCursorPrimaryState {
                    primary_scene_work_pending: true,
                    primary_target_owned: target_owned,
                    independent_kms_admission: lane_available,
                    ..Default::default()
                },
                true,
            ),
            NativeCursorOutputDisposition::DeferForPrimary
        );
    }
}

#[test]
fn auto_ready_carrier_before_service_deadline_uses_latest_cursor_without_plane_delta() {
    use super::NativeCursorPrimaryState;

    let mut arbitration = NativeCursorOutputArbitration::default();
    arbitration.request_hardware(1, 10, 100);
    for epoch in 2..=32 {
        arbitration.request_hardware(epoch, 10 + epoch, 100);
    }

    assert_eq!(arbitration.desired_epoch(), 32);
    assert_eq!(
        super::cursor_output_disposition_for_primary_state(
            &mut arbitration,
            NativeCursorSchedulingPolicy::Auto,
            99,
            NativeCursorPrimaryState {
                primary_scene_work_pending: true,
                ready_primary_carrier_attachable: true,
                independent_kms_admission: false,
                ..Default::default()
            },
            true,
        ),
        NativeCursorOutputDisposition::PiggybackPrimary
    );
    assert_eq!(arbitration.plane_delta_plans(), 0);
    assert_eq!(arbitration.ready_primary_piggybacks, 1);
}

#[test]
fn auto_never_admits_cursor_ahead_of_queued_or_submitted_primary_even_if_lane_snapshot_is_stale() {
    use super::NativeCursorPrimaryState;

    for primary_state in [
        NativeCursorPrimaryState {
            primary_worker_queued: true,
            independent_kms_admission: true,
            ..Default::default()
        },
        NativeCursorPrimaryState {
            primary_kernel_submitted: true,
            independent_kms_admission: true,
            ..Default::default()
        },
    ] {
        let mut arbitration = NativeCursorOutputArbitration::default();
        arbitration.request_hardware(8, 10, 20);
        assert_eq!(
            super::cursor_output_disposition_for_primary_state(
                &mut arbitration,
                NativeCursorSchedulingPolicy::Auto,
                20,
                primary_state,
                true,
            ),
            NativeCursorOutputDisposition::DeferForPrimary
        );
    }

    let mut arbitration = NativeCursorOutputArbitration::default();
    arbitration.request_hardware(9, 10, 20);
    assert_eq!(
        super::cursor_output_disposition_for_primary_state(
            &mut arbitration,
            NativeCursorSchedulingPolicy::Auto,
            20,
            NativeCursorPrimaryState {
                // Even a stale positive carrier snapshot cannot make a
                // kernel-submitted primary mutable after freeze.
                ready_primary_carrier_attachable: true,
                primary_kernel_submitted: true,
                independent_kms_admission: true,
                ..Default::default()
            },
            true,
        ),
        NativeCursorOutputDisposition::DeferForPrimary
    );
}

#[test]
fn auto_unready_scene_bypasses_after_deadline_and_keeps_latest_epoch() {
    use super::NativeCursorPrimaryState;

    let mut arbitration = NativeCursorOutputArbitration::default();
    arbitration.request_hardware(1, 100, 200);
    for epoch in 2..=1_000 {
        arbitration.request_hardware(epoch, 100 + epoch, 200);
    }
    assert_eq!(arbitration.desired_epoch(), 1_000);
    assert_eq!(arbitration.pending_since_ns(), Some(100));
    assert_eq!(arbitration.changes_coalesced(), 999);
    assert_eq!(
        super::cursor_output_disposition_for_primary_state(
            &mut arbitration,
            NativeCursorSchedulingPolicy::Auto,
            200,
            NativeCursorPrimaryState {
                primary_scene_work_pending: true,
                independent_kms_admission: true,
                ..Default::default()
            },
            true,
        ),
        NativeCursorOutputDisposition::SubmitPlaneDelta
    );
    assert_eq!(arbitration.plane_delta_plans(), 1);
    assert_eq!(arbitration.unready_primary_bypass_decisions, 1);
}
