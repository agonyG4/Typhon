use super::*;

use crate::wm::layout::TiledResizeHandle;
use crate::wm::{LayoutMembership, WindowManagementState, WorkspaceId, WorkspaceLocation};

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
