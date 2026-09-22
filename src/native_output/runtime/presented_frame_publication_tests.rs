use super::presentation_worker::{promote_immediate_and_publish, promote_pageflip_and_publish};
use super::scene_history::{NativeFrameSceneSnapshot, NativeSceneHistory};
use super::*;
use oblivion_one::compositor::{
    AnimationTime, OwnCompositorServer, PresentationFrameSnapshot, PresentationRect,
    PresentedFramePublication, PresentedLifecycleScene, RenderableSurface, RenderableSurfaceDamage,
    SurfaceCommitSequence, SurfaceOpaqueRegion, SurfacePlacement, SurfaceRenderBackend, WindowId,
};
use oblivion_one::core::{OutputId, SceneNodeId};
use oblivion_one::presentation_animation::{
    PresentationEngine, PresentationRetainedVisualIdentity, PresentationRetainedVisualKind,
};
use oblivion_one::render_backend::buffer::{BufferIdAllocator, BufferSize, CommittedSurfaceBuffer};
use oblivion_one::window_lifecycle_animation::{
    LifecycleDirection, LifecycleFrameLamp, LifecycleFrameSnapshot, LifecycleVisualGroup,
};
use std::process;
use wayland_server::protocol::wl_output;

const FROZEN_ROOT: u32 = 415;
const CURRENT_ROOT: u32 = 416;

fn retained_identity(window_id: WindowId, raw: u64) -> PresentationRetainedVisualIdentity {
    PresentationEngine::enabled()
        .begin_retained_visual(
            SceneNodeId::from_raw(window_id.get()).expect("test scene node"),
            PresentationRetainedVisualKind::WindowLifecycle,
            AnimationTime::from_nanos(raw),
        )
        .expect("test retained lifecycle identity")
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> PresentationRect {
    PresentationRect::new(x, y, width, height).expect("valid test rectangle")
}

fn test_surface(surface_id: u32) -> RenderableSurface {
    let identity = BufferIdAllocator::default()
        .allocate()
        .expect("test buffer identity");
    RenderableSurface {
        surface_id,
        x: 0,
        y: 0,
        width: 64,
        height: 48,
        placement: SurfacePlacement::root(),
        render_backend: SurfaceRenderBackend::NativeWayland,
        render_placement: None,
        visual_clip: None,
        render_target_size: None,
        generation: 1,
        commit_sequence: SurfaceCommitSequence::initial(),
        buffer: CommittedSurfaceBuffer::shm_snapshot(
            identity,
            BufferSize::new(64, 48).expect("test surface size"),
            vec![0; 64 * 48],
        ),
        viewport_source: None,
        viewport_destination: None,
        buffer_scale: 1,
        buffer_transform: wl_output::Transform::Normal,
        opaque_region: SurfaceOpaqueRegion::None,
        damage: RenderableSurfaceDamage::Full,
    }
}

fn snapshot(output_id: OutputId, frame_id: u64) -> NativeFrameSceneSnapshot {
    NativeFrameSceneSnapshot {
        output_id,
        frame_id,
        render_generation: frame_id,
        scene: NativeSceneSnapshot::from_surfaces_with_scene_nodes(&[], &[], Vec::new(), &[]),
        cursor_damage: NativeCursorDamageBounds::default(),
        presentation: PresentationFrameSnapshot::empty_for_output(output_id),
        lifecycle: LifecycleFrameSnapshot::default(),
    }
}

fn snapshot_with_root(
    output_id: OutputId,
    frame_id: u64,
    root_surface_id: u32,
) -> NativeFrameSceneSnapshot {
    let mut frame = snapshot(output_id, frame_id);
    let surfaces = [test_surface(root_surface_id)];
    let scene_node_ids = [SceneNodeId::from_raw(root_surface_id as u64).expect("scene node")];
    frame.scene = NativeSceneSnapshot::from_surfaces_with_scene_nodes(
        &surfaces,
        &scene_node_ids,
        Vec::new(),
        &[],
    );
    frame
}

fn old_physical_lifecycle_lamp(root_surface_id: u32) -> LifecycleFrameSnapshot {
    let window_id = WindowId::from_raw(root_surface_id as u64).expect("lifecycle window id");
    let visual_group = LifecycleVisualGroup::from_bounds(
        rect(40.0, 40.0, 160.0, 120.0),
        rect(40.0, 40.0, 160.0, 120.0),
        rect(40.0, 40.0, 160.0, 120.0),
        rect(600.0, 800.0, 80.0, 20.0),
        1920,
        1080,
    )
    .expect("valid lifecycle group");
    let mut snapshot = LifecycleFrameSnapshot {
        sampled_at: Some(AnimationTime::from_nanos(1)),
        lamps: vec![LifecycleFrameLamp {
            window_id,
            root_surface_id,
            presentation_identity: retained_identity(window_id, 1),
            visual_group,
            progress: 0.5,
            opacity: 1.0,
            mathematically_settled: false,
            direction: LifecycleDirection::Minimize,
        }],
        signature: 0,
    };
    snapshot.refresh_signature();
    snapshot
}

#[test]
fn pageflip_promotion_publishes_presentation_and_lifecycle_together() {
    let socket_name = format!("typhon-presented-frame-pageflip-{}", process::id());
    let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name)
        .expect("bind compositor for pageflip publication regression");
    let output_id = server.native_output_id().expect("test output id");
    let mut history = NativeSceneHistory::new(snapshot(output_id, 1));

    assert!(history.replace_ready(snapshot(output_id, 2)));
    assert!(history.queue_submission(23));
    assert!(promote_pageflip_and_publish(&mut history, 23, &mut server));

    assert_eq!(server.presented_presentation_frame_id(), 2);
    assert_eq!(server.presented_lifecycle_frame_id(), 2);
    let physical = history.presented_snapshot().expect("promoted frame");
    assert_eq!(
        server.presented_presentation_snapshot_for_test(),
        Some(&physical.presentation)
    );
    assert_eq!(
        server.presented_lifecycle_snapshot_for_test().lamps,
        physical.lifecycle.lamps
    );
}

#[test]
fn immediate_promotion_publishes_presentation_and_lifecycle_together() {
    let socket_name = format!("typhon-presented-frame-immediate-{}", process::id());
    let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name)
        .expect("bind compositor for immediate publication regression");
    let output_id = server.native_output_id().expect("test output id");
    let mut history = NativeSceneHistory::new(snapshot(output_id, 1));

    let physical_snapshot = snapshot(output_id, 2);
    assert!(history.replace_ready(physical_snapshot));
    assert!(promote_immediate_and_publish(&mut history, &mut server));

    assert_eq!(server.presented_presentation_frame_id(), 2);
    assert_eq!(server.presented_lifecycle_frame_id(), 2);
    let physical = history.presented_snapshot().expect("promoted frame");
    assert_eq!(
        server.presented_presentation_snapshot_for_test(),
        Some(&physical.presentation)
    );
    assert_eq!(
        server.presented_lifecycle_snapshot_for_test().lamps,
        physical.lifecycle.lamps
    );
}

#[test]
fn pageflip_uses_the_promoted_snapshot_roots_after_canonical_scene_changes() {
    let socket_name = format!("typhon-presented-frame-root-race-{}", process::id());
    let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name)
        .expect("bind compositor for frozen-root publication regression");
    let output_id = server.native_output_id().expect("test output id");
    let old_lifecycle = old_physical_lifecycle_lamp(FROZEN_ROOT);
    let initial_presentation = PresentationFrameSnapshot::empty_for_output(output_id);
    server.publish_presented_frame(PresentedFramePublication {
        frame_id: 1,
        presentation: &initial_presentation,
        lifecycle: &old_lifecycle,
        lifecycle_scene: PresentedLifecycleScene::Initial,
    });
    assert_eq!(
        server.presented_lifecycle_snapshot_for_test(),
        &old_lifecycle
    );

    let mut history = NativeSceneHistory::new(snapshot(output_id, 1));
    let frozen = snapshot_with_root(output_id, 2, FROZEN_ROOT);
    let frozen_roots = frozen
        .scene
        .surfaces
        .iter()
        .map(|surface| surface.visual_root_surface_id)
        .collect::<Vec<_>>();
    assert!(history.replace_ready(frozen));
    assert!(history.queue_submission(24));

    server.install_native_frame_test_scene(vec![test_surface(CURRENT_ROOT)], &[], None);
    assert_eq!(
        server.renderable_surfaces()[0].surface_id,
        CURRENT_ROOT,
        "canonical compositor scene changed after frame B was frozen"
    );
    assert_eq!(frozen_roots, [FROZEN_ROOT]);
    assert_eq!(
        server.presented_lifecycle_snapshot_for_test().lamps.len(),
        1
    );

    assert!(promote_pageflip_and_publish(&mut history, 24, &mut server));

    assert_eq!(server.presented_presentation_frame_id(), 2);
    assert_eq!(server.presented_lifecycle_frame_id(), 2);
    let physical = history.presented_snapshot().expect("promoted frame");
    assert_eq!(
        physical.scene.surfaces[0].visual_root_surface_id,
        FROZEN_ROOT
    );
    assert_eq!(
        server.presented_presentation_snapshot_for_test(),
        Some(&physical.presentation)
    );
    assert_eq!(
        server.presented_lifecycle_snapshot_for_test().lamps,
        physical.lifecycle.lamps,
        "exact replacement publication retires the old physical Lamp fallback"
    );
}

#[test]
fn combined_test_presentation_helper_publishes_both_ledgers_together() {
    let socket_name = format!("typhon-presented-frame-test-helper-{}", process::id());
    let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name)
        .expect("bind compositor for combined test publication regression");
    server.publish_test_presentation_at(31, AnimationTime::from_nanos(0));

    assert_eq!(server.presented_presentation_frame_id(), 31);
    assert_eq!(server.presented_lifecycle_frame_id(), 31);
    assert!(server.presented_presentation_snapshot_for_test().is_some());
}
