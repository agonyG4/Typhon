use super::*;

use crate::wm::layout::TiledResizeHandle;
use crate::wm::{LayoutMembership, WindowManagementState, WorkspaceId, WorkspaceLocation};

fn test_renderable_surface(surface_id: u32, width: u32, height: u32) -> RenderableSurface {
    let identity = BufferIdAllocator::default()
        .allocate()
        .expect("test buffer identity");
    RenderableSurface {
        surface_id,
        x: 0,
        y: 0,
        width,
        height,
        placement: SurfacePlacement::root(),
        render_backend: SurfaceRenderBackend::NativeWayland,
        render_placement: None,
        visual_clip: None,
        render_target_size: None,
        generation: 1,
        commit_sequence: SurfaceCommitSequence::initial(),
        buffer: crate::render_backend::buffer::CommittedSurfaceBuffer::shm_snapshot(
            identity,
            BufferSize::new(width, height).expect("test buffer size"),
            vec![0; width as usize * height as usize],
        ),
        viewport_source: None,
        viewport_destination: None,
        buffer_scale: 1,
        buffer_transform: wayland_server::protocol::wl_output::Transform::Normal,
        damage: RenderableSurfaceDamage::Full,
    }
}

#[test]
fn tiled_dwindle_reflow_uses_the_kde_layout_policy_curve() {
    let mut state = CompositorState::new(None);
    assert!(state.set_output_size(1_920, 1_080));
    let location = WorkspaceLocation::Regular(WorkspaceId::new(1).expect("workspace"));
    let first = state.allocate_window_id().expect("first window id");
    let second = state.allocate_window_id().expect("second window id");

    state
        .insert_desktop_window(DesktopWindow::new_xdg(first, 250))
        .expect("first window");
    state.window_mut(first).expect("first window").management =
        Some(WindowManagementState::new(location).with_layout(LayoutMembership::Tiled));
    state
        .tiled_layout
        .insert(location, first, crate::wm::layout::InsertHint::default())
        .expect("first tiled insert");
    state.append_renderable_surface(test_renderable_surface(250, 16, 16));
    state.install_toplevel_visual_geometry(
        250,
        WindowGeometry::new(SurfacePlacement::absolute_root_at(37, 49), 640, 480),
    );
    assert!(state.reflow_tiled_location(location));
    state.cancel_presentation_geometry_for_root(250);

    state
        .insert_desktop_window(DesktopWindow::new_xdg(second, 251))
        .expect("second window");
    state.window_mut(second).expect("second window").management =
        Some(WindowManagementState::new(location).with_layout(LayoutMembership::Tiled));
    state
        .tiled_layout
        .insert(location, second, crate::wm::layout::InsertHint::default())
        .expect("second tiled insert");

    assert!(state.reflow_tiled_location(location));
    assert_eq!(
        state.presentation_animator.track_curve(
            state
                .presentation_scene_node_id_for_root(250)
                .expect("group node")
        ),
        Some(
            PresentationAnimationPolicy::kde().curve_for(PresentationAnimationKind::LayoutReflow,)
        )
    );
}

#[test]
fn nested_dwindle_reflow_installs_one_atomic_three_window_transaction() {
    let mut state = CompositorState::new(None);
    assert!(state.set_output_size(1_920, 1_080));
    let roots = [270, 271, 272];
    let windows = [
        WindowId::from_raw(70).expect("first window id"),
        WindowId::from_raw(71).expect("second window id"),
        WindowId::from_raw(72).expect("third window id"),
    ];
    state.install_native_frame_test_scene(
        roots
            .into_iter()
            .map(|root| test_renderable_surface(root, 640, 480))
            .collect(),
        &[
            (roots[0], windows[0]),
            (roots[1], windows[1]),
            (roots[2], windows[2]),
        ],
        None,
    );

    let previous = [
        WindowGeometry::new(SurfacePlacement::absolute_root_at(40, 40), 640, 480),
        WindowGeometry::new(SurfacePlacement::absolute_root_at(700, 40), 640, 480),
        WindowGeometry::new(SurfacePlacement::absolute_root_at(40, 540), 640, 480),
    ];
    let targets = [
        WindowGeometry::new(SurfacePlacement::absolute_root_at(80, 80), 700, 500),
        WindowGeometry::new(SurfacePlacement::absolute_root_at(760, 80), 700, 500),
        WindowGeometry::new(SurfacePlacement::absolute_root_at(80, 600), 700, 500),
    ];
    for index in 0..roots.len() {
        state.install_toplevel_visual_geometry(roots[index], previous[index]);
    }

    state.begin_layout_reflow_batch();
    state.begin_layout_reflow_batch();
    for index in 0..roots.len() {
        state.animate_toplevel_visual_geometry(
            roots[index],
            previous[index],
            targets[index],
            PresentationAnimationKind::LayoutReflow,
        );
    }
    assert_eq!(state.presentation_animator.active_count(), 0);
    assert!(!state.finish_layout_reflow_batch());
    assert_eq!(state.presentation_animator.active_count(), 0);
    let _ = state.finish_layout_reflow_batch();
    assert_eq!(state.presentation_animator.active_count(), roots.len());

    let nodes = roots
        .into_iter()
        .map(|root| {
            state
                .presentation_scene_node_id_for_root(root)
                .expect("group node")
        })
        .collect::<Vec<_>>();
    let transactions = nodes
        .iter()
        .map(|node| {
            state
                .presentation_animator
                .track_transaction(*node)
                .expect("transaction")
        })
        .collect::<Vec<_>>();
    assert_eq!(transactions[0], transactions[1]);
    assert_eq!(transactions[1], transactions[2]);
    let revisions = nodes
        .iter()
        .map(|node| {
            state
                .presentation_animator
                .track_revision(*node)
                .expect("revision")
        })
        .collect::<Vec<_>>();
    assert_ne!(revisions[0], revisions[1]);
    assert_ne!(revisions[1], revisions[2]);
    assert_eq!(
        state
            .presentation_animator
            .track_started_at_for_scene_node(nodes[0]),
        state
            .presentation_animator
            .track_started_at_for_scene_node(nodes[1])
    );
    assert_eq!(
        state
            .presentation_animator
            .track_started_at_for_scene_node(nodes[1]),
        state
            .presentation_animator
            .track_started_at_for_scene_node(nodes[2])
    );

    let layout_generation = state.layout_generation;
    let configure_serial = state.next_configure_serial;
    let canonical_placements = state
        .renderable_surfaces
        .iter()
        .map(|surface| (surface.surface_id, surface.placement))
        .collect::<Vec<_>>();
    for nanos in [0, 8_000_000, 32_000_000, 96_000_000] {
        let _ = state.presentation_scene_sample_at(AnimationTime::from_nanos(nanos));
    }
    assert_eq!(state.layout_generation, layout_generation);
    assert_eq!(state.next_configure_serial, configure_serial);
    assert_eq!(
        state
            .renderable_surfaces
            .iter()
            .map(|surface| (surface.surface_id, surface.placement))
            .collect::<Vec<_>>(),
        canonical_placements
    );
}

#[test]
fn pending_geometry_mutation_preserves_first_start_and_final_target() {
    let scene_node_id = crate::core::SceneNodeId::from_raw(2_700).expect("scene node");
    let first = crate::presentation_animation::PresentationGeometryMutation::new(
        scene_node_id,
        crate::presentation_animation::PresentationRect::new(0.0, 0.0, 100.0, 100.0)
            .expect("first start"),
        crate::presentation_animation::PresentationRect::new(100.0, 0.0, 100.0, 100.0)
            .expect("first target"),
        PresentationAnimationPolicy::kde().curve_for(PresentationAnimationKind::LayoutReflow),
    );
    let second = crate::presentation_animation::PresentationGeometryMutation::new(
        scene_node_id,
        first.target,
        crate::presentation_animation::PresentationRect::new(200.0, 0.0, 100.0, 100.0)
            .expect("second target"),
        PresentationAnimationPolicy::kde().curve_for(PresentationAnimationKind::LayoutReflow),
    );
    let mut pending = super::active_scene::PendingPresentationGeometryTransaction {
        started_at: crate::presentation_animation::AnimationTime::from_nanos(10),
        members: Vec::new(),
    };

    pending.upsert_geometry_mutation(first);
    pending.upsert_geometry_mutation(second);

    assert_eq!(pending.members.len(), 1);
    assert_eq!(pending.members[0].scene_node_id(), Some(scene_node_id));
    assert_eq!(pending.members[0].start, first.start);
    assert_eq!(pending.members[0].target, second.target);
    assert_eq!(
        pending.started_at,
        crate::presentation_animation::AnimationTime::from_nanos(10)
    );
}

#[test]
fn presentation_sampling_requires_allocated_output_identity() {
    let state = CompositorState::default();
    assert!(state.native_output_id().is_none());
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        state.presentation_scene_sample_at(
            crate::presentation_animation::AnimationTime::from_nanos(0),
        );
    }));
    assert!(result.is_err());
}

#[test]
fn presentation_sampling_uses_the_explicitly_allocated_output_identity() {
    let mut state = CompositorState::default();
    let output_id = state
        .ensure_native_output_id()
        .expect("test output identity");
    let sample = state
        .presentation_scene_sample_at(crate::presentation_animation::AnimationTime::from_nanos(0));
    assert_eq!(sample.output_id, output_id);
}

#[test]
fn nested_layout_batch_merges_duplicate_geometry_before_outer_commit() {
    let mut state = CompositorState::new(None);
    let root_surface_id = 2_701;
    let window_id = WindowId::from_raw(2_701).expect("window id");
    state.install_native_frame_test_scene(
        vec![test_renderable_surface(root_surface_id, 100, 100)],
        &[(root_surface_id, window_id)],
        None,
    );
    let start = WindowGeometry::new(SurfacePlacement::absolute_root_at(0, 0), 100, 100);
    let middle = WindowGeometry::new(SurfacePlacement::absolute_root_at(100, 0), 100, 100);
    let target = WindowGeometry::new(SurfacePlacement::absolute_root_at(200, 0), 100, 100);
    state.install_toplevel_visual_geometry(root_surface_id, start);

    state.begin_layout_reflow_batch();
    state.begin_layout_reflow_batch();
    state.animate_toplevel_visual_geometry(
        root_surface_id,
        start,
        middle,
        PresentationAnimationKind::LayoutReflow,
    );
    state.animate_toplevel_visual_geometry(
        root_surface_id,
        middle,
        target,
        PresentationAnimationKind::LayoutReflow,
    );
    assert_eq!(state.presentation_animator.active_count(), 0);
    assert_eq!(state.presentation_animator.transaction_count(), 0);

    assert!(!state.finish_layout_reflow_batch());
    assert_eq!(state.presentation_animator.active_count(), 0);
    assert_eq!(state.presentation_animator.transaction_count(), 0);

    let _ = state.finish_layout_reflow_batch();
    let scene_node_id = state
        .presentation_scene_node_id_for_root(root_surface_id)
        .expect("window group scene node");
    assert_eq!(state.presentation_animator.active_count(), 1);
    assert_eq!(state.presentation_animator.transaction_count(), 1);
    assert_eq!(
        state
            .presentation_animator
            .track_revision(scene_node_id)
            .expect("one geometry revision")
            .get(),
        1
    );
    let started_at = state
        .presentation_animator
        .track_started_at_for_scene_node(scene_node_id)
        .expect("track start");
    assert_eq!(
        state
            .presentation_animator
            .sample_at_transition_start_for_scene_node(scene_node_id)
            .expect("start sample")
            .rect,
        crate::presentation_animation::PresentationRect::new(0.0, 0.0, 100.0, 100.0)
            .expect("start rect")
    );
    assert_eq!(
        state
            .presentation_animator
            .sample_for_scene_node(
                scene_node_id,
                crate::presentation_animation::AnimationTime::from_nanos(
                    started_at.as_nanos() + 1_000_000_000,
                ),
            )
            .expect("settled sample")
            .rect,
        crate::presentation_animation::PresentationRect::new(200.0, 0.0, 100.0, 100.0)
            .expect("target rect")
    );
}

#[test]
fn net_zero_layout_batch_elides_presentation_work_and_scanout_blocker() {
    let mut state = CompositorState::new(None);
    assert!(state.set_output_size(1_920, 1_080));
    let root_surface_id = 2_703;
    let window_id = WindowId::from_raw(2_703).expect("window id");
    state.install_native_frame_test_scene(
        vec![test_renderable_surface(root_surface_id, 1_920, 1_080)],
        &[(root_surface_id, window_id)],
        None,
    );
    let start = WindowGeometry::new(SurfacePlacement::absolute_root_at(0, 0), 1_920, 1_080);
    let middle = WindowGeometry::new(SurfacePlacement::absolute_root_at(80, 60), 1_760, 960);
    state.install_toplevel_visual_geometry(root_surface_id, start);
    let scene_node_id = state
        .presentation_scene_node_id_for_root(root_surface_id)
        .expect("window group scene node");
    assert_eq!(state.presentation_animator.active_count(), 0);
    assert!(!state.presentation_animation_has_pending_visible());
    assert!(
        !state
            .direct_scanout_scene_blockers()
            .reasons()
            .contains(&DirectScanoutSceneRejection::AnimationTransform)
    );

    state.begin_layout_reflow_batch();
    state.animate_toplevel_visual_geometry(
        root_surface_id,
        start,
        middle,
        PresentationAnimationKind::LayoutReflow,
    );
    state.animate_toplevel_visual_geometry(
        root_surface_id,
        middle,
        start,
        PresentationAnimationKind::LayoutReflow,
    );
    assert_eq!(state.presentation_animator.active_count(), 0);

    let _ = state.finish_layout_reflow_batch();
    assert_eq!(state.presentation_animator.active_count(), 0);
    assert_eq!(state.presentation_animator.transaction_count(), 0);
    assert!(!state.presentation_animator.has_track(scene_node_id));
    assert!(
        !state
            .presentation_animator
            .has_pending_visible(&[scene_node_id])
    );
    assert!(!state.presentation_animation_has_pending_visible());
    assert!(
        !state
            .direct_scanout_scene_blockers()
            .reasons()
            .contains(&DirectScanoutSceneRejection::AnimationTransform)
    );
}

#[test]
fn repeated_pending_mutations_retarget_active_track_at_batch_start() {
    let mut state = CompositorState::new(None);
    let root_surface_id = 2_702;
    let window_id = WindowId::from_raw(2_702).expect("window id");
    state.install_native_frame_test_scene(
        vec![test_renderable_surface(root_surface_id, 100, 100)],
        &[(root_surface_id, window_id)],
        None,
    );
    let start = WindowGeometry::new(SurfacePlacement::absolute_root_at(0, 0), 100, 100);
    let active_target = WindowGeometry::new(SurfacePlacement::absolute_root_at(100, 0), 100, 100);
    let pending_middle = WindowGeometry::new(SurfacePlacement::absolute_root_at(150, 0), 100, 100);
    let pending_target = WindowGeometry::new(SurfacePlacement::absolute_root_at(250, 0), 100, 100);
    state.install_toplevel_visual_geometry(root_surface_id, start);
    state.animate_toplevel_visual_geometry(
        root_surface_id,
        start,
        active_target,
        PresentationAnimationKind::LayoutReflow,
    );
    let scene_node_id = state
        .presentation_scene_node_id_for_root(root_surface_id)
        .expect("window group scene node");
    let previous_transaction = state
        .presentation_animator
        .track_transaction(scene_node_id)
        .expect("active transaction");

    state.begin_layout_reflow_batch();
    let batch_started_at = state.layout_animation_epoch.expect("batch start");
    let expected = state
        .presentation_animator
        .sample_for_scene_node(scene_node_id, batch_started_at)
        .expect("active sample at batch start");
    state.animate_toplevel_visual_geometry(
        root_surface_id,
        active_target,
        pending_middle,
        PresentationAnimationKind::LayoutReflow,
    );
    state.animate_toplevel_visual_geometry(
        root_surface_id,
        pending_middle,
        pending_target,
        PresentationAnimationKind::LayoutReflow,
    );
    let _ = state.finish_layout_reflow_batch();

    assert_eq!(state.presentation_animator.active_count(), 1);
    assert_eq!(state.presentation_animator.transaction_count(), 1);
    assert_ne!(
        state.presentation_animator.track_transaction(scene_node_id),
        Some(previous_transaction)
    );
    let retargeted_start = state
        .presentation_animator
        .sample_at_transition_start_for_scene_node(scene_node_id)
        .expect("retargeted start");
    assert_eq!(retargeted_start.rect, expected.rect);
    assert_eq!(retargeted_start.velocity, expected.velocity);
    let started_at = state
        .presentation_animator
        .track_started_at_for_scene_node(scene_node_id)
        .expect("retargeted timestamp");
    assert_eq!(started_at, batch_started_at);
    assert_eq!(
        state
            .presentation_animator
            .sample_for_scene_node(
                scene_node_id,
                crate::presentation_animation::AnimationTime::from_nanos(
                    started_at.as_nanos() + 1_000_000_000,
                ),
            )
            .expect("retargeted settled sample")
            .rect,
        crate::presentation_animation::PresentationRect::new(250.0, 0.0, 100.0, 100.0)
            .expect("final target rect")
    );
}

#[test]
fn tiled_dwindle_reflow_starts_from_pre_mutation_geometry_without_visual_history() {
    let mut state = CompositorState::new(None);
    assert!(state.set_output_size(1_920, 1_080));
    let location = WorkspaceLocation::Regular(WorkspaceId::new(1).expect("workspace"));
    let window_id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(window_id, 260))
        .expect("window");
    state.window_mut(window_id).expect("window").management =
        Some(WindowManagementState::new(location).with_layout(LayoutMembership::Tiled));
    state
        .tiled_layout
        .insert(
            location,
            window_id,
            crate::wm::layout::InsertHint::default(),
        )
        .expect("tiled insert");
    state.append_renderable_surface(test_renderable_surface(260, 640, 480));
    let source = WindowGeometry::new(SurfacePlacement::absolute_root_at(37, 49), 640, 480);
    state.surface_placements.insert(260, source.placement);
    state.renderable_surfaces[0].placement = source.placement;
    state.install_toplevel_visual_geometry(260, source);
    state.toplevel_visual_geometries.remove(&260);

    assert!(state.reflow_tiled_location(location));
    let start = state
        .presentation_animator
        .sample_at_transition_start_for_scene_node(
            state
                .presentation_scene_node_id_for_root(260)
                .expect("group node"),
        )
        .expect("layout transition should be active");
    let expected = state
        .presentation_rect_for_geometry(260, source)
        .expect("source presentation rect");
    assert_eq!(start.rect, expected);
}

#[test]
fn focused_regular_window_toggles_layout_without_recreating_or_moving_workspace_membership() {
    let mut state = CompositorState::new(None);
    let id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(id, 250))
        .expect("window");
    state.focused_window_id = Some(id);

    let location = state
        .window(id)
        .expect("window")
        .management
        .expect("management")
        .location();
    assert!(state.toggle_focused_window_layout());
    assert_eq!(
        state
            .window(id)
            .expect("window")
            .management
            .expect("management")
            .layout(),
        LayoutMembership::Tiled
    );
    assert_eq!(
        state
            .window(id)
            .expect("window")
            .management
            .expect("management")
            .location(),
        location
    );
    assert_eq!(state.focused_window_id, Some(id));
    assert!(
        state
            .tiled_layout
            .tree(location)
            .is_some_and(|tree| tree.contains_window(id))
    );

    assert!(state.toggle_focused_window_layout());
    assert_eq!(
        state
            .window(id)
            .expect("window")
            .management
            .expect("management")
            .layout(),
        LayoutMembership::Floating
    );
    assert_eq!(state.window(id).expect("window").id, id);
    assert!(state.tiled_layout.tree(location).is_none());
}

#[test]
fn special_window_can_toggle_to_tiled_without_changing_special_membership() {
    let mut state = CompositorState::new(None);
    let id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(id, 251))
        .expect("window");
    let location = WorkspaceLocation::Special(crate::wm::SpecialWorkspaceId::DEFAULT);
    state.window_mut(id).expect("window").management = Some(WindowManagementState::new(location));
    state.focused_window_id = Some(id);

    assert!(state.toggle_focused_window_layout());
    let management = state
        .window(id)
        .expect("window")
        .management
        .expect("management");
    assert_eq!(management.location(), location);
    assert_eq!(management.layout(), LayoutMembership::Tiled);
    assert_eq!(
        state.workspace_manager.active_workspace(),
        WorkspaceId::new(1).unwrap()
    );
    assert_eq!(state.workspace_manager.visible_special_workspace(), None);
}

#[test]
fn tiled_to_floating_restores_the_last_floating_geometry() {
    let mut state = CompositorState::new(None);
    let id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(id, 252))
        .expect("window");
    state.focused_window_id = Some(id);
    let floating = WindowGeometry::new(SurfacePlacement::absolute_root_at(37, 49), 640, 480);
    state.install_toplevel_visual_geometry(252, floating);

    assert!(state.toggle_focused_window_layout());
    assert_eq!(
        state.window(id).expect("window").floating_geometry,
        Some(floating)
    );

    let tiled_layout_geometry =
        WindowGeometry::new(SurfacePlacement::absolute_root_at(3, 5), 300, 200);
    state.install_toplevel_visual_geometry(252, tiled_layout_geometry);
    assert!(state.toggle_focused_window_layout());

    assert_eq!(
        state.current_visual_root_window_geometry(252),
        Some(floating)
    );
    assert_eq!(state.focused_window_id, Some(id));
}

#[test]
fn prepared_tiled_detach_is_atomic_when_tree_ownership_is_invalid() {
    let mut state = CompositorState::new(None);
    let id = state.allocate_window_id().expect("window id");
    let location = WorkspaceLocation::Regular(WorkspaceId::new(1).expect("workspace"));
    state
        .insert_desktop_window(DesktopWindow::new_xdg(id, 253))
        .expect("window");
    state.window_mut(id).expect("window").management =
        Some(WindowManagementState::new(location).with_layout(LayoutMembership::Tiled));
    state
        .window_mut(id)
        .expect("window")
        .state
        .set_mode(ToplevelMode::Maximized);

    assert!(state.prepare_tiled_detach(id).is_none());
    assert_eq!(
        state.window(id).expect("window").state.mode(),
        ToplevelMode::Maximized
    );
    assert_eq!(
        state
            .window(id)
            .expect("window")
            .management
            .expect("management")
            .layout(),
        LayoutMembership::Tiled
    );
    assert!(state.tiled_layout.tree(location).is_none());
}

#[test]
fn prepared_tiled_detach_handles_the_only_leaf() {
    let mut state = CompositorState::new(None);
    assert!(state.set_output_size(1_920, 1_080));
    let id = state.allocate_window_id().expect("window id");
    let location = WorkspaceLocation::Regular(WorkspaceId::new(1).expect("workspace"));
    state
        .insert_desktop_window(DesktopWindow::new_xdg(id, 254))
        .expect("window");
    state.window_mut(id).expect("window").management =
        Some(WindowManagementState::new(location).with_layout(LayoutMembership::Tiled));
    state
        .tiled_layout
        .insert(location, id, crate::wm::layout::InsertHint::default())
        .expect("tiled insert");

    let prepared = state.prepare_tiled_detach(id).expect("detach preparation");
    assert!(state.commit_prepared_tiled_detach(prepared, None));
    assert!(state.tiled_layout.tree(location).is_none());
    assert_eq!(
        state
            .window(id)
            .expect("window")
            .management
            .expect("management")
            .layout(),
        LayoutMembership::Floating
    );
}

#[test]
fn impossible_live_constraint_update_auto_floats_the_culprit_without_partial_layout() {
    let mut state = CompositorState::new(None);
    let first = state.allocate_window_id().expect("first window id");
    let second = state.allocate_window_id().expect("second window id");
    for (window_id, surface_id) in [(first, 260), (second, 261)] {
        state
            .insert_desktop_window(DesktopWindow::new_xdg(window_id, surface_id))
            .expect("window");
        state.window_mut(window_id).expect("window").management = Some(
            WindowManagementState::new(WorkspaceLocation::Regular(
                WorkspaceId::new(1).expect("workspace"),
            ))
            .with_layout(LayoutMembership::Tiled),
        );
        state
            .tiled_layout
            .insert(
                WorkspaceLocation::Regular(WorkspaceId::new(1).expect("workspace")),
                window_id,
                crate::wm::layout::InsertHint::default(),
            )
            .expect("tiled insert");
    }
    state
        .window_mut(first)
        .expect("culprit")
        .constraints
        .min_width = Some(2_000);

    assert!(state.reconcile_tiled_constraints(first));
    assert_eq!(
        state
            .window(first)
            .expect("culprit")
            .management
            .expect("management")
            .layout(),
        LayoutMembership::Floating
    );
    assert_eq!(
        state
            .window(second)
            .expect("survivor")
            .management
            .expect("management")
            .layout(),
        LayoutMembership::Tiled
    );
    assert_eq!(state.resize_flow_metrics.tiled_constraint_auto_floats, 1);
}

#[test]
fn migration_fallback_prepares_floating_restore_before_commit() {
    let mut state = CompositorState::new(None);
    let incoming = state.allocate_window_id().expect("incoming id");
    let existing = state.allocate_window_id().expect("existing id");
    for (window_id, surface_id) in [(incoming, 270), (existing, 271)] {
        state
            .insert_desktop_window(DesktopWindow::new_xdg(window_id, surface_id))
            .expect("window");
    }
    let source = WorkspaceLocation::Regular(WorkspaceId::new(1).expect("source"));
    let destination = WorkspaceLocation::Regular(WorkspaceId::new(2).expect("destination"));
    for (window_id, location) in [(incoming, source), (existing, destination)] {
        state.window_mut(window_id).expect("window").management =
            Some(WindowManagementState::new(location).with_layout(LayoutMembership::Tiled));
        state
            .tiled_layout
            .insert(
                location,
                window_id,
                crate::wm::layout::InsertHint::default(),
            )
            .expect("tree insert");
    }
    state
        .window_mut(incoming)
        .expect("incoming")
        .constraints
        .min_width = Some(4_000);

    let prepared = state
        .migrate_tiled_layouts(&[(incoming, source, destination)])
        .expect("migration preparation succeeds with incoming fallback");
    assert_eq!(prepared.fallback_windows, vec![incoming]);
    assert!(
        state
            .tiled_layout
            .tree(source)
            .is_some_and(|tree| tree.contains_window(incoming))
    );
    assert!(
        state
            .tiled_layout
            .tree(destination)
            .is_some_and(|tree| tree.contains_window(existing))
    );

    state.commit_prepared_tiled_migration(&prepared);
    assert!(
        state
            .tiled_layout
            .tree(destination)
            .is_some_and(|tree| !tree.contains_window(incoming))
    );
    assert_eq!(
        state
            .window(incoming)
            .expect("incoming")
            .management
            .unwrap()
            .layout(),
        LayoutMembership::Floating
    );
    assert!(
        state
            .window(incoming)
            .expect("incoming")
            .floating_geometry
            .is_some()
    );
    state.window_mut(incoming).expect("incoming").management =
        Some(WindowManagementState::new(destination).with_layout(LayoutMembership::Floating));
    state.apply_prepared_tiled_migration(prepared);
    assert!(state.tiled_floating_restores.contains_key(&incoming));
    assert!(state.tiled_layout_dirty.contains(&destination));
    assert!(matches!(
        state.activate_workspace(WorkspaceId::new(2).unwrap()),
        crate::wm::WorkspaceSwitchOutcome::Changed { .. }
    ));
    assert!(!state.tiled_floating_restores.contains_key(&incoming));
    assert_eq!(
        state.current_visual_root_window_geometry(270),
        state.window(incoming).expect("incoming").floating_geometry
    );
}

#[test]
fn failed_migration_preparation_has_no_membership_or_tree_side_effect() {
    let mut state = CompositorState::new(None);
    let incoming = state.allocate_window_id().expect("incoming id");
    let existing = state.allocate_window_id().expect("existing id");
    for (window_id, surface_id) in [(incoming, 280), (existing, 281)] {
        state
            .insert_desktop_window(DesktopWindow::new_xdg(window_id, surface_id))
            .expect("window");
    }
    let source = WorkspaceLocation::Regular(WorkspaceId::new(1).expect("source"));
    let destination = WorkspaceLocation::Regular(WorkspaceId::new(2).expect("destination"));
    for (window_id, location) in [(incoming, source), (existing, destination)] {
        state.window_mut(window_id).expect("window").management =
            Some(WindowManagementState::new(location).with_layout(LayoutMembership::Tiled));
        state
            .tiled_layout
            .insert(
                location,
                window_id,
                crate::wm::layout::InsertHint::default(),
            )
            .expect("tree insert");
    }
    state
        .window_mut(existing)
        .expect("existing")
        .constraints
        .min_width = Some(4_000);

    assert!(
        state
            .migrate_tiled_layouts(&[(incoming, source, destination)])
            .is_none()
    );
    assert!(
        state
            .tiled_layout
            .tree(source)
            .is_some_and(|tree| tree.contains_window(incoming))
    );
    assert!(
        state
            .tiled_layout
            .tree(destination)
            .is_some_and(|tree| tree.contains_window(existing))
    );
    assert_eq!(
        state
            .window(incoming)
            .expect("incoming")
            .management
            .unwrap()
            .location(),
        source
    );
    assert_eq!(
        state
            .window(existing)
            .expect("existing")
            .management
            .unwrap()
            .layout(),
        LayoutMembership::Tiled
    );
}

#[test]
fn tiled_snapshots_follow_tree_membership_not_global_desktop_windows() {
    let mut state = CompositorState::new(None);
    let active = WorkspaceLocation::Regular(WorkspaceId::new(1).unwrap());
    let hidden = WorkspaceLocation::Regular(WorkspaceId::new(2).unwrap());
    let mut active_ids = Vec::new();
    for surface_id in 290..293 {
        let id = state.allocate_window_id().expect("window id");
        state
            .insert_desktop_window(DesktopWindow::new_xdg(id, surface_id))
            .expect("window");
        state.window_mut(id).expect("window").management =
            Some(WindowManagementState::new(active).with_layout(LayoutMembership::Tiled));
        state
            .tiled_layout
            .insert(active, id, crate::wm::layout::InsertHint::default())
            .expect("active tree insert");
        active_ids.push(id);
    }
    for surface_id in 300..330 {
        let id = state.allocate_window_id().expect("window id");
        state
            .insert_desktop_window(DesktopWindow::new_xdg(id, surface_id))
            .expect("window");
        state.window_mut(id).expect("window").management =
            Some(WindowManagementState::new(hidden).with_layout(LayoutMembership::Tiled));
    }

    let snapshots = state.layout_snapshots(active);
    assert_eq!(snapshots.len(), active_ids.len());
    assert!(
        snapshots
            .iter()
            .all(|snapshot| active_ids.contains(&snapshot.window))
    );
}

#[test]
fn active_tiled_resize_migration_commits_inside_one_outer_layout_batch() {
    let mut state = CompositorState::new(None);
    let first = state.allocate_window_id().expect("first window id");
    let second = state.allocate_window_id().expect("second window id");
    for (window_id, surface_id) in [(first, 330), (second, 331)] {
        state
            .insert_desktop_window(DesktopWindow::new_xdg(window_id, surface_id))
            .expect("window");
    }
    let source = WorkspaceLocation::Regular(WorkspaceId::new(1).expect("source"));
    let destination = WorkspaceLocation::Regular(WorkspaceId::new(2).expect("destination"));
    for window_id in [first, second] {
        state.window_mut(window_id).expect("window").management =
            Some(WindowManagementState::new(source).with_layout(LayoutMembership::Tiled));
        state
            .tiled_layout
            .insert(source, window_id, crate::wm::layout::InsertHint::default())
            .expect("tree insert");
    }
    state.focused_window_id = Some(first);

    let root = state.layout_root_rect();
    let snapshots = [
        crate::wm::layout::LayoutWindowSnapshot::new(first),
        crate::wm::layout::LayoutWindowSnapshot::new(second),
    ];
    let solution = state
        .tiled_layout
        .calculate(source, root, &snapshots)
        .expect("initial solution");
    let edges = ResizeEdges::new(false, false, false, true);
    let handle = TiledResizeHandle::from_solution(
        state.tiled_layout.tree(source).expect("source tree"),
        &solution,
        first,
        crate::wm::layout::ResizeEdges::new(false, false, false, true),
    )
    .expect("resize handle");
    let interaction_id = WindowInteractionId::new(900);
    let preparation = TiledResizePreparation {
        location: source,
        edges,
        handle,
        solution,
    };
    state.window_interaction = Some(WindowInteraction {
        id: interaction_id,
        window_id: first,
        root_surface_id: 330,
        kind: WindowInteractionKind::Resize(edges),
        source: WindowInteractionSource::NativeBinding,
        trigger_button: None,
        trigger_serial: None,
        pointer_motion_surface_id: None,
        start_pointer_x: 0.0,
        start_pointer_y: 0.0,
        start_placement: SurfacePlacement::absolute_root_at(0, 0),
        start_width: 800,
        start_height: 600,
        drag_committed: true,
        resize_interaction_id: Some(ResizeInteractionId::new(900)),
        tiled_resize: true,
        decoration_owned: false,
    });
    state.install_tiled_resize_session(
        interaction_id,
        ResizeInteractionId::new(900),
        first,
        &preparation,
    );
    state.pending_tiled_resize = Some(PendingTiledResize {
        interaction_id,
        window_id: first,
        location: source,
        horizontal_requested_ratio: Some(0.6),
        vertical_requested_ratio: None,
    });
    let identity = BufferIdAllocator::default()
        .allocate()
        .expect("test buffer identity");
    state.append_renderable_surface(RenderableSurface {
        surface_id: 330,
        x: 0,
        y: 0,
        width: 800,
        height: 600,
        placement: SurfacePlacement::root(),
        render_backend: SurfaceRenderBackend::NativeWayland,
        render_placement: None,
        visual_clip: None,
        render_target_size: None,
        generation: 1,
        commit_sequence: SurfaceCommitSequence::initial(),
        buffer: crate::render_backend::buffer::CommittedSurfaceBuffer::shm_snapshot(
            identity,
            BufferSize::new(800, 600).expect("test buffer size"),
            vec![0; 800 * 600],
        ),
        buffer_scale: 1,
        buffer_transform: wayland_server::protocol::wl_output::Transform::Normal,
        viewport_source: None,
        viewport_destination: None,
        damage: RenderableSurfaceDamage::Full,
    });
    let old_visual = WindowGeometry::new(SurfacePlacement::absolute_root_at(20, 30), 800, 600);
    state.install_toplevel_visual_geometry(330, old_visual);

    let before_prepare_render = state.render_generation;
    let before_prepare_layout = state.layout_generation;
    let prepared = state
        .migrate_tiled_layouts(&[(first, source, destination)])
        .expect("prepare succeeds");
    assert_eq!(state.render_generation, before_prepare_render);
    assert_eq!(state.layout_generation, before_prepare_layout);
    assert!(state.window_interaction.is_some());
    assert!(state.pending_tiled_resize.is_some());
    assert!(
        state
            .tiled_layout
            .tree(source)
            .is_some_and(|tree| tree.contains_window(first))
    );
    drop(prepared);

    let before_migration_render = state.render_generation;
    let before_migration_layout = state.layout_generation;
    assert!(state.move_focused_window_to_workspace(WorkspaceId::new(2).unwrap()));

    assert!(state.window_interaction.is_none());
    assert!(state.tiled_resize_session.is_none());
    assert!(state.pending_tiled_resize.is_none());
    assert_eq!(state.render_generation, before_migration_render + 1);
    assert_eq!(
        state.layout_generation.get(),
        before_migration_layout.get() + 1
    );
    assert_eq!(
        state.render_generation_cause(),
        RenderGenerationCause::LayoutReflow
    );
    assert!(
        state
            .tiled_layout
            .tree(source)
            .is_some_and(|tree| !tree.contains_window(first) && tree.contains_window(second))
    );
    assert!(
        state
            .tiled_layout
            .tree(destination)
            .is_some_and(|tree| tree.contains_window(first))
    );
    assert_eq!(
        state
            .window(first)
            .expect("migrated window")
            .management
            .unwrap()
            .location(),
        destination
    );
    assert_eq!(
        state
            .backend_commands
            .iter()
            .filter(|command| {
                matches!(
                    command,
                    crate::compositor::window_backend::WindowBackendCommand::FinalizeResize { .. }
                )
            })
            .count(),
        1
    );
}
