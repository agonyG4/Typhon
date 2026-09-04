use super::*;
use crate::wm::{
    LayoutMembership, WindowManagementState, WorkspaceId, WorkspaceLocation, WorkspaceSwitchOutcome,
};
use crate::xwayland::xwm::{
    X11DecorationHints, X11FrameExtents, X11Geometry, X11MetadataDelta, X11MotifDecorationHint,
    X11PublishedState, X11StackMode, X11WindowSnapshot, X11WindowType, X11WindowTypes,
};
use crate::xwayland::{X11WindowHandle, XwaylandGeneration};
use std::num::NonZeroU64;

fn x11_snapshot(generation: XwaylandGeneration, xid: u32, surface_id: u32) -> X11WindowSnapshot {
    X11WindowSnapshot {
        handle: X11WindowHandle::new(generation, xid),
        surface_id,
        kind: DesktopWindowKind::Managed,
        window_types: X11WindowTypes::default(),
        decoration_hints: Default::default(),
        override_redirect: false,
        geometry: X11Geometry {
            x: 10,
            y: 20,
            width: 800,
            height: 600,
        },
        metadata: WindowMetadata {
            app_id: Some("TyphonApp".into()),
            title: Some("Typhon Window".into()),
            pid: Some(42),
        },
        constraints: WindowConstraints::default(),
        state: X11PublishedState::default(),
        transient_for: None,
        supports_delete: true,
        supports_take_focus: true,
        accepts_input: Some(true),
        window_role: None,
        startup_id: None,
        user_time: None,
        urgency: false,
        supports_sync_request: false,
        sync_counter: None,
    }
}

fn insert_x11(state: &mut CompositorState, snapshot: X11WindowSnapshot) -> WindowId {
    let id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_x11(id, snapshot))
        .expect("X11 window");
    id
}

#[test]
fn window_id_is_nonzero_monotonic_and_not_reused() {
    let mut state = CompositorState::new(None);
    let first = state.allocate_window_id().expect("first id");
    let second = state.allocate_window_id().expect("second id");
    assert!(first.get() != 0);
    assert!(second > first);
    assert_ne!(first, second);
}

#[test]
fn xdg_toplevel_creation_builds_one_role_and_one_desktop_window() {
    let mut state = CompositorState::new(None);
    let id = state.allocate_window_id().expect("window id");
    let window = DesktopWindow::new_xdg(id, 41);
    state.insert_desktop_window(window).expect("insert window");
    assert_eq!(state.desktop_windows.len(), 1);
    assert_eq!(state.window_by_root_surface.get(&41), Some(&id));
    assert_eq!(
        state.window(id).expect("window").management,
        Some(crate::wm::WindowManagementState::new(
            crate::wm::WorkspaceLocation::Regular(crate::wm::WorkspaceId::new(1).unwrap()),
        ))
    );
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
fn managed_x11_toplevel_joins_the_active_workspace_as_floating() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let id = insert_x11(&mut state, x11_snapshot(generation, 200, 201));

    let management = state
        .window(id)
        .expect("window")
        .management
        .expect("managed X11 window has management state");
    assert_eq!(management.regular_workspace().unwrap().get(), 1);
    assert_eq!(management.layout(), LayoutMembership::Floating);
}

#[test]
fn admitted_x11_window_preserves_decoration_hints_without_changing_management() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(2).expect("generation"));
    let mut snapshot = x11_snapshot(generation, 201, 202);
    snapshot.decoration_hints = X11DecorationHints {
        motif: X11MotifDecorationHint::Undecorated,
        gtk_frame_extents: Some(X11FrameExtents {
            left: 1,
            right: 2,
            top: 3,
            bottom: 4,
        }),
    };
    let expected_hints = snapshot.decoration_hints.clone();
    let expected_types = snapshot.window_types.clone();
    let id = insert_x11(&mut state, snapshot);
    let window = state.window(id).expect("window");

    assert_eq!(window.x11_decoration_hints, expected_hints);
    assert_eq!(window.kind, DesktopWindowKind::Managed);
    assert!(window.management.is_some());
    assert_eq!(window.x11_window_types, expected_types);
}

#[test]
fn restoring_a_tiled_x11_window_uses_the_layout_plan_instead_of_floating_geometry() {
    let mut state = CompositorState::new(None);
    let snapshot = x11_snapshot(
        XwaylandGeneration::new(NonZeroU64::new(9).expect("generation")),
        253,
        253,
    );
    let id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_x11(id, snapshot))
        .expect("window");
    state.focused_window_id = Some(id);
    let floating = WindowGeometry::new(SurfacePlacement::absolute_root_at(37, 49), 640, 480);
    state.install_x11_visual_geometry(253, floating);

    assert!(state.toggle_focused_window_layout());
    let location = state
        .window(id)
        .expect("window")
        .management
        .expect("management")
        .location();
    let expected = state
        .tiled_layout
        .calculate(
            location,
            state.layout_root_rect(),
            &[crate::wm::layout::LayoutWindowSnapshot::new(id)],
        )
        .expect("layout plan")
        .target_for_window(id)
        .expect("tiled target");
    let expected = WindowGeometry::new(
        SurfacePlacement::absolute_root_at(expected.tile().x(), expected.tile().y()),
        expected.tile().width(),
        expected.tile().height(),
    );

    assert!(state.set_root_window_mode(253, ToplevelMode::Fullscreen));
    assert!(state.restore_normal_root_window(253));
    assert_eq!(
        state.window(id).expect("window").state.mode(),
        ToplevelMode::Normal
    );
    assert_eq!(
        state.current_visual_root_window_geometry(253),
        Some(expected)
    );
}

#[test]
fn workspace_visibility_keeps_inactive_window_mapped_and_unminimized() {
    let mut state = CompositorState::new(None);
    let first = state.allocate_window_id().expect("first window id");
    let second = state.allocate_window_id().expect("second window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(first, 401))
        .expect("first window");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(second, 402))
        .expect("second window");
    let workspace_two = WorkspaceId::new(2).expect("workspace two");
    state.window_mut(second).expect("second window").management = Some(WindowManagementState::new(
        WorkspaceLocation::Regular(workspace_two),
    ));

    assert!(state.window_is_visible_in_active_scene(first));
    assert!(!state.window_is_visible_in_active_scene(second));
    assert!(
        !state
            .window(second)
            .expect("second window")
            .state
            .is_minimized()
    );
    assert_eq!(
        state.window(second).expect("second window").root_surface_id,
        402
    );

    assert_eq!(
        state.activate_workspace(workspace_two),
        WorkspaceSwitchOutcome::Changed {
            previous: WorkspaceId::new(1).unwrap(),
            current: workspace_two,
        }
    );
    assert!(!state.window_is_visible_in_active_scene(first));
    assert!(state.window_is_visible_in_active_scene(second));
}

#[test]
fn workspace_switch_advances_scene_generation_once_and_noop_does_not_advance() {
    let mut state = CompositorState::new(None);
    let workspace_two = WorkspaceId::new(2).unwrap();
    let before = state.scene_render_generation;

    assert!(matches!(
        state.activate_workspace(workspace_two),
        WorkspaceSwitchOutcome::Changed { .. }
    ));
    assert_eq!(state.scene_render_generation, before + 1);
    assert_eq!(
        state.render_generation_cause(),
        RenderGenerationCause::WorkspaceSwitch
    );

    let after_switch = state.scene_render_generation;
    assert_eq!(
        state.activate_workspace(workspace_two),
        WorkspaceSwitchOutcome::NoChange
    );
    assert_eq!(state.scene_render_generation, after_switch);
}

#[test]
fn moving_window_family_preserves_layout_and_keeps_active_workspace() {
    let mut state = CompositorState::new(None);
    let parent = state.allocate_window_id().expect("parent window id");
    let child = state.allocate_window_id().expect("child window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(parent, 411))
        .expect("parent window");
    let mut child_window = DesktopWindow::new_xdg(child, 412);
    child_window.relationships.parent = Some(parent);
    state
        .insert_desktop_window(child_window)
        .expect("child window");
    state.window_mut(parent).unwrap().management = Some(
        WindowManagementState::new(WorkspaceLocation::Regular(WorkspaceId::new(1).unwrap()))
            .with_layout(LayoutMembership::Tiled),
    );
    state.window_mut(child).unwrap().management = Some(
        WindowManagementState::new(WorkspaceLocation::Regular(WorkspaceId::new(1).unwrap()))
            .with_layout(LayoutMembership::Floating),
    );

    let workspace_two = WorkspaceId::new(2).unwrap();
    // Moving a focused transient must canonicalize to the top-level family
    // root before collecting descendants.
    assert!(state.move_window_family_to_workspace(child, workspace_two));
    assert_eq!(
        state.workspace_manager.active_workspace(),
        WorkspaceId::new(1).unwrap()
    );
    assert_eq!(
        state
            .window(parent)
            .unwrap()
            .management
            .unwrap()
            .regular_workspace(),
        Some(workspace_two)
    );
    assert_eq!(
        state
            .window(child)
            .unwrap()
            .management
            .unwrap()
            .regular_workspace(),
        Some(workspace_two)
    );
    assert_eq!(
        state.window(parent).unwrap().management.unwrap().layout(),
        LayoutMembership::Tiled
    );
    assert_eq!(
        state.window(child).unwrap().management.unwrap().layout(),
        LayoutMembership::Floating
    );
}

#[test]
fn moving_a_tiled_window_migrates_its_tree_to_the_destination_location() {
    let mut state = CompositorState::new(None);
    let id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(id, 413))
        .expect("window");
    state.focused_window_id = Some(id);
    assert!(state.toggle_focused_window_layout());

    let source = WorkspaceLocation::Regular(WorkspaceId::new(1).expect("source workspace"));
    let destination = WorkspaceId::new(2).expect("destination workspace");
    assert!(state.move_window_family_to_workspace(id, destination));
    assert!(
        !state
            .tiled_layout
            .tree(source)
            .is_some_and(|tree| tree.contains_window(id))
    );
    assert!(
        state
            .tiled_layout
            .tree(WorkspaceLocation::Regular(destination))
            .is_some_and(|tree| tree.contains_window(id))
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
}

#[test]
fn removing_parent_preserves_child_workspace_membership() {
    let mut state = CompositorState::new(None);
    let parent = state.allocate_window_id().expect("parent window id");
    let child = state.allocate_window_id().expect("child window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(parent, 421))
        .expect("parent window");
    let mut child_window = DesktopWindow::new_xdg(child, 422);
    child_window.relationships.parent = Some(parent);
    state
        .insert_desktop_window(child_window)
        .expect("child window");
    let workspace_two = WorkspaceId::new(2).unwrap();
    state.window_mut(child).unwrap().management = Some(
        WindowManagementState::new(WorkspaceLocation::Regular(workspace_two))
            .with_layout(LayoutMembership::Tiled),
    );

    state.remove_desktop_window(parent).expect("removed parent");
    let child_window = state.window(child).expect("child remains");
    assert_eq!(child_window.relationships.parent, None);
    assert_eq!(
        child_window.management.unwrap().regular_workspace(),
        Some(workspace_two)
    );
    assert_eq!(
        child_window.management.unwrap().layout(),
        LayoutMembership::Tiled
    );
}

#[test]
fn auxiliary_x11_window_has_no_independent_workspace_membership() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let mut snapshot = x11_snapshot(generation, 202, 203);
    snapshot.kind = DesktopWindowKind::OverrideRedirect;
    let id = insert_x11(&mut state, snapshot);

    assert_eq!(state.window(id).expect("window").management, None);
}

#[test]
fn auxiliary_x11_scene_band_inherits_canonical_special_or_regular_owner() {
    let mut state = CompositorState::new(None);
    let special = state.allocate_window_id().expect("special window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(special, 204))
        .expect("special window");
    state
        .window_mut(special)
        .expect("special window")
        .management = Some(
        WindowManagementState::new(WorkspaceLocation::Special(
            crate::wm::SpecialWorkspaceId::DEFAULT,
        ))
        .with_layout(LayoutMembership::Tiled),
    );
    let special_aux = insert_x11(
        &mut state,
        x11_snapshot(
            XwaylandGeneration::new(NonZeroU64::new(3).unwrap()),
            205,
            206,
        ),
    );
    state
        .window_mut(special_aux)
        .expect("special auxiliary")
        .relationships
        .parent = Some(special);

    let regular = state.allocate_window_id().expect("regular window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(regular, 207))
        .expect("regular window");
    let regular_aux = insert_x11(
        &mut state,
        x11_snapshot(
            XwaylandGeneration::new(NonZeroU64::new(4).unwrap()),
            208,
            209,
        ),
    );
    state
        .window_mut(regular_aux)
        .expect("regular auxiliary")
        .relationships
        .parent = Some(regular);

    assert_eq!(
        state.scene_work_owner_for_window(special_aux),
        SceneWorkOwner::Location(WorkspaceLocation::Special(
            crate::wm::SpecialWorkspaceId::DEFAULT,
        ))
    );
    assert_eq!(
        state
            .renderable_root_stack_key(
                state
                    .window(special_aux)
                    .expect("special auxiliary")
                    .root_surface_id,
                0,
            )
            .0,
        4
    );
    assert_eq!(
        state
            .renderable_root_stack_key(
                state
                    .window(regular_aux)
                    .expect("regular auxiliary")
                    .root_surface_id,
                0,
            )
            .0,
        3
    );
}

#[test]
fn scene_work_index_migrates_auxiliary_owner_when_canonical_parent_changes() {
    let mut state = CompositorState::new(None);
    let regular = state.allocate_window_id().expect("regular window id");
    let special = state.allocate_window_id().expect("special window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(regular, 214))
        .expect("regular window");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(special, 215))
        .expect("special window");
    state
        .window_mut(special)
        .expect("special window")
        .management = Some(WindowManagementState::new(WorkspaceLocation::Special(
        crate::wm::SpecialWorkspaceId::DEFAULT,
    )));
    let mut auxiliary_snapshot = x11_snapshot(
        XwaylandGeneration::new(NonZeroU64::new(5).unwrap()),
        216,
        217,
    );
    auxiliary_snapshot.kind = DesktopWindowKind::OverrideRedirect;
    let auxiliary = insert_x11(&mut state, auxiliary_snapshot);
    state
        .window_mut(auxiliary)
        .expect("auxiliary window")
        .relationships
        .parent = Some(regular);
    state.active_fifo_barriers.insert(
        217,
        ActiveFifoBarrier {
            surface_generation: 1,
            fifo_barrier_generation: FifoBarrierGeneration::new(1),
            commit_sequence: SurfaceCommitSequence::initial(),
            fallback_deadline_ns: u64::MAX,
        },
    );

    state.reconcile_workspace_inheritance();
    assert_eq!(
        state.scene_work_prepare_count(WorkspaceLocation::Regular(WorkspaceId::new(1).unwrap(),)),
        1
    );
    assert_eq!(
        state.scene_work_prepare_count(WorkspaceLocation::Special(
            crate::wm::SpecialWorkspaceId::DEFAULT,
        )),
        0
    );
    assert_eq!(state.window(auxiliary).unwrap().management, None);

    state
        .window_mut(auxiliary)
        .expect("auxiliary window")
        .relationships
        .parent = Some(special);
    state.reconcile_workspace_inheritance();

    assert_eq!(
        state.scene_work_prepare_count(WorkspaceLocation::Regular(WorkspaceId::new(1).unwrap(),)),
        0
    );
    assert_eq!(
        state.scene_work_prepare_count(WorkspaceLocation::Special(
            crate::wm::SpecialWorkspaceId::DEFAULT,
        )),
        1
    );
    assert_eq!(state.window(auxiliary).unwrap().management, None);
}

#[test]
fn default_special_toggle_preserves_regular_selection_and_updates_scene() {
    let mut state = CompositorState::new(None);
    let id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(id, 431))
        .expect("window");
    state.window_mut(id).expect("window").management = Some(WindowManagementState::new(
        WorkspaceLocation::Special(crate::wm::SpecialWorkspaceId::DEFAULT),
    ));
    let regular = state.active_workspace();
    let backend_commands_before = state.backend_commands.len();
    assert_eq!(
        state.toggle_default_special_workspace(),
        crate::wm::SpecialWorkspaceToggleOutcome::Opened {
            id: crate::wm::SpecialWorkspaceId::DEFAULT,
        }
    );
    assert_eq!(state.active_workspace(), regular);
    assert!(state.active_scene_surfaces().is_empty());
    assert_eq!(state.backend_commands.len(), backend_commands_before);
    assert_eq!(
        state.toggle_default_special_workspace(),
        crate::wm::SpecialWorkspaceToggleOutcome::Closed {
            id: crate::wm::SpecialWorkspaceId::DEFAULT,
        }
    );
    assert!(state.active_scene_surfaces().is_empty());
    assert_eq!(state.backend_commands.len(), backend_commands_before);
}

#[test]
fn focused_family_moves_between_special_and_current_regular_without_geometry_change() {
    let mut state = CompositorState::new(None);
    let id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(id, 432))
        .expect("window");
    assert!(state.window(id).unwrap().is_workspace_managed());
    assert!(state.window(id).unwrap().management.is_some());
    state.focused_window_id = Some(id);
    assert_eq!(state.focused_window_id, Some(id));
    assert_eq!(
        state.window(id).unwrap().management.unwrap().location(),
        WorkspaceLocation::Regular(WorkspaceId::new(1).unwrap())
    );
    let before_commands = state.backend_commands.len();

    assert!(state.move_focused_window_to_or_from_special_workspace());
    assert_eq!(
        state.window(id).unwrap().management.unwrap().location(),
        WorkspaceLocation::Special(crate::wm::SpecialWorkspaceId::DEFAULT)
    );
    assert_eq!(
        state.workspace_manager.visible_special_workspace(),
        None,
        "moving to Special is silent while the overlay is hidden"
    );
    assert_eq!(state.backend_commands.len(), before_commands);

    state
        .workspace_manager
        .toggle_special_workspace(crate::wm::SpecialWorkspaceId::DEFAULT);
    state.focused_window_id = Some(id);
    assert!(state.move_focused_window_to_or_from_special_workspace());
    assert_eq!(
        state.window(id).unwrap().management.unwrap().location(),
        WorkspaceLocation::Regular(WorkspaceId::new(1).unwrap())
    );
}

#[test]
fn moving_x11_family_to_special_queues_typed_clear_workspace() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(2).unwrap());
    let id = insert_x11(&mut state, x11_snapshot(generation, 240, 241));
    state.focused_window_id = Some(id);

    assert!(state.move_focused_window_to_or_from_special_workspace());
    assert!(matches!(
        state.backend_commands.last(),
        Some(crate::compositor::window_backend::WindowBackendCommand::ClearWorkspace {
            window
        }) if *window == id
    ));
}

#[test]
fn special_overlay_focus_wins_over_regular_application_focus() {
    let mut state = CompositorState::new(None);
    let regular = state.allocate_window_id().expect("regular window id");
    let special = state.allocate_window_id().expect("special window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(regular, 451))
        .expect("regular window");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(special, 452))
        .expect("special window");
    state.window_mut(special).unwrap().management = Some(WindowManagementState::new(
        WorkspaceLocation::Special(crate::wm::SpecialWorkspaceId::DEFAULT),
    ));
    state.window_mut(regular).unwrap().last_focus_serial = 100;
    state.window_mut(special).unwrap().last_focus_serial = 1;
    state
        .workspace_manager
        .toggle_special_workspace(crate::wm::SpecialWorkspaceId::DEFAULT);

    assert_eq!(state.topmost_renderable_toplevel_window_id(), Some(special));
}

#[test]
fn closing_special_restores_the_best_visible_regular_focus_candidate() {
    let mut state = CompositorState::new(None);
    let regular = state.allocate_window_id().expect("regular window id");
    let special = state.allocate_window_id().expect("special window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(regular, 461))
        .expect("regular window");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(special, 462))
        .expect("special window");
    state.window_mut(special).unwrap().management = Some(WindowManagementState::new(
        WorkspaceLocation::Special(crate::wm::SpecialWorkspaceId::DEFAULT),
    ));
    state.focused_window_id = Some(special);
    state
        .workspace_manager
        .toggle_special_workspace(crate::wm::SpecialWorkspaceId::DEFAULT);

    let _ = state.toggle_default_special_workspace();

    assert_eq!(state.topmost_renderable_toplevel_window_id(), Some(regular));
}

#[test]
fn surface_lookup_resolves_stable_window_identity() {
    let mut state = CompositorState::new(None);
    let id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(id, 7))
        .expect("insert window");
    assert_eq!(state.window_id_for_surface(7), Some(id));
}

#[test]
fn metadata_updates_do_not_touch_backend_protocol_state() {
    let mut state = CompositorState::new(None);
    let id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(id, 7))
        .expect("insert window");
    let backend = state.window(id).expect("window").backend;
    state.window_mut(id).expect("window").metadata.title = Some("Typhon".into());
    assert_eq!(state.window(id).expect("window").backend, backend);
}

#[test]
fn destroying_xdg_role_removes_window_and_reverse_index_atomically() {
    let mut state = CompositorState::new(None);
    let id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(id, 7))
        .expect("insert window");
    assert!(state.remove_desktop_window(id).is_some());
    assert!(state.window(id).is_none());
    assert!(state.window_id_for_surface(7).is_none());
}

#[test]
fn parent_relationship_uses_window_id_not_surface_id() {
    let mut state = CompositorState::new(None);
    let parent = state.allocate_window_id().expect("parent id");
    let child = state.allocate_window_id().expect("child id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(parent, 100))
        .expect("insert parent");
    let mut child_window = DesktopWindow::new_xdg(child, 200);
    child_window.relationships.parent = Some(parent);
    state
        .insert_desktop_window(child_window)
        .expect("insert child");
    assert_eq!(
        state.window(child).expect("child").relationships.parent,
        Some(parent)
    );
}

#[test]
fn failed_role_creation_leaves_no_partial_desktop_window() {
    let mut state = CompositorState::new(None);
    let first = state.allocate_window_id().expect("first id");
    let second = state.allocate_window_id().expect("second id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(first, 9))
        .expect("insert first");
    let result = state.insert_desktop_window(DesktopWindow::new_xdg(second, 9));
    assert_eq!(result, Err(DesktopWindowError::DuplicateRootSurface));
    assert!(state.window(second).is_none());
    assert_eq!(state.window_id_for_surface(9), Some(first));
}

#[test]
fn window_stacking_uses_stable_ids() {
    let mut state = CompositorState::new(None);
    let first = state.allocate_window_id().expect("first id");
    let second = state.allocate_window_id().expect("second id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(first, 10))
        .expect("insert first");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(second, 20))
        .expect("insert second");

    assert_eq!(state.window_stacking, vec![first, second]);
    assert!(state.raise_window_id(first));
    assert_eq!(state.window_stacking, vec![second, first]);
    assert_eq!(state.window(first).expect("first").root_surface_id, 10);
}

#[test]
fn normal_window_geometry_is_independent_of_stacking_order() {
    let mut state = CompositorState::new(None);
    let a = state.allocate_window_id().expect("A window id");
    let b = state.allocate_window_id().expect("B window id");
    let c = state.allocate_window_id().expect("C window id");
    for (window_id, surface_id) in [(a, 401), (b, 402), (c, 403)] {
        state
            .insert_desktop_window(DesktopWindow::new_xdg(window_id, surface_id))
            .expect("insert XDG window");
    }

    let placements = [401, 402, 403].map(|surface_id| state.surface_placement(surface_id));
    assert!(
        placements
            .iter()
            .all(|placement| placement.root_mode == crate::compositor::RootPlacementMode::Absolute)
    );
    assert_ne!(placements[0], placements[1]);
    assert_ne!(placements[1], placements[2]);

    let initial = [a, b, c].map(|id| state.desktop_window_frame(id).expect("frame"));
    assert!(state.raise_window_id(a));
    assert_eq!(state.window_stacking, vec![b, c, a]);
    assert_eq!(
        initial,
        [a, b, c].map(|id| state.desktop_window_frame(id).expect("frame"))
    );
    assert!(state.raise_window_id(b));
    assert_eq!(state.window_stacking, vec![c, a, b]);
    assert_eq!(
        initial,
        [a, b, c].map(|id| state.desktop_window_frame(id).expect("frame"))
    );

    for index in 0..100 {
        assert!(state.raise_window_id(if index % 2 == 0 { a } else { b }));
        assert_eq!(
            initial,
            [a, b, c].map(|id| state.desktop_window_frame(id).expect("frame"))
        );
    }
}

#[test]
fn closing_xdg_window_does_not_reflow_survivors_or_new_window_placement() {
    let mut state = CompositorState::new(None);
    let a = state.allocate_window_id().expect("A window id");
    let b = state.allocate_window_id().expect("B window id");
    let c = state.allocate_window_id().expect("C window id");
    for (window_id, surface_id) in [(a, 411), (b, 412), (c, 413)] {
        state
            .insert_desktop_window(DesktopWindow::new_xdg(window_id, surface_id))
            .expect("insert XDG window");
    }

    let a_before = state.desktop_window_frame(a).expect("A frame");
    let c_before = state.desktop_window_frame(c).expect("C frame");
    assert!(state.remove_desktop_window(b).is_some());
    assert_eq!(state.desktop_window_frame(a), Some(a_before));
    assert_eq!(state.desktop_window_frame(c), Some(c_before));

    let d = state.allocate_window_id().expect("D window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(d, 414))
        .expect("insert XDG D");
    assert_eq!(state.desktop_window_frame(a), Some(a_before));
    assert_eq!(state.desktop_window_frame(c), Some(c_before));
    assert_ne!(state.desktop_window_frame(d), Some(a_before));
    assert_ne!(state.desktop_window_frame(d), Some(c_before));
}

#[test]
fn repeated_xdg_creation_reuses_bounded_initial_placement() {
    let mut state = CompositorState::new(None);
    let usable = state.usable_output_geometry();
    let min_x = usable.x as i32;
    let min_y = usable.y as i32;
    let max_x = (usable.x + (usable.width - 800.0).max(0.0)) as i32;
    let max_y = (usable.y + (usable.height - 600.0).max(0.0)) as i32;
    let mut first_frame = None;
    let mut maximum_x = min_x;
    let mut maximum_y = min_y;

    for surface_id in 500..600 {
        let window_id = state.allocate_window_id().expect("window id");
        state
            .insert_desktop_window(DesktopWindow::new_xdg(window_id, surface_id))
            .expect("insert XDG window");
        let frame = state.desktop_window_frame(window_id).expect("frame");
        assert!(frame.0 >= min_x && frame.0 <= max_x);
        assert!(frame.1 >= min_y && frame.1 <= max_y);
        assert!(i64::from(frame.0) + i64::from(frame.2) <= (usable.x + usable.width) as i64);
        assert!(i64::from(frame.1) + i64::from(frame.3) <= (usable.y + usable.height) as i64);
        maximum_x = maximum_x.max(frame.0);
        maximum_y = maximum_y.max(frame.1);
        if let Some(first_frame) = first_frame {
            assert_eq!(frame, first_frame, "released placement must be reusable");
        } else {
            first_frame = Some(frame);
        }
        assert!(state.remove_desktop_window(window_id).is_some());
    }

    assert!(maximum_x <= max_x);
    assert!(maximum_y <= max_y);
}

#[test]
fn ready_x11_event_creates_one_desktop_window() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let snapshot = x11_snapshot(generation, 100, 50);
    let id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_x11(id, snapshot.clone()))
        .expect("insert X11 window");

    assert_eq!(state.desktop_windows.len(), 1);
    assert_eq!(state.window_id_for_surface(50), Some(id));
    assert_eq!(state.window_id_for_x11_handle(snapshot.handle), Some(id));
    assert_eq!(
        state.window(id).expect("window").metadata.title.as_deref(),
        Some("Typhon Window")
    );
}

#[test]
fn duplicate_ready_event_is_rejected_without_duplicate_window() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let snapshot = x11_snapshot(generation, 101, 51);
    let first = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_x11(first, snapshot.clone()))
        .expect("insert X11 window");
    let second = state.allocate_window_id().expect("window id");

    assert_eq!(
        state.insert_desktop_window(DesktopWindow::new_x11(second, snapshot)),
        Err(DesktopWindowError::DuplicateWindowId)
    );
    assert_eq!(state.desktop_windows.len(), 1);
}

#[test]
fn destroyed_x11_window_removes_surface_index_focus_and_interaction() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let snapshot = x11_snapshot(generation, 102, 52);
    let id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_x11(id, snapshot.clone()))
        .expect("insert X11 window");

    assert!(state.remove_desktop_window(id).is_some());
    assert!(state.window_id_for_surface(snapshot.surface_id).is_none());
    assert!(state.window_id_for_x11_handle(snapshot.handle).is_none());
    assert!(state.window_stacking.is_empty());
}

#[test]
fn old_generation_destroy_cannot_remove_new_generation_window() {
    let mut state = CompositorState::new(None);
    let old = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let new = XwaylandGeneration::new(NonZeroU64::new(2).unwrap());
    let old_snapshot = x11_snapshot(old, 103, 53);
    let new_snapshot = x11_snapshot(new, 103, 54);
    let id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_x11(id, new_snapshot.clone()))
        .expect("insert new X11 window");

    assert!(
        state
            .window_id_for_x11_handle(old_snapshot.handle)
            .is_none()
    );
    assert!(state.window(id).is_some());
    assert_eq!(
        state.window_id_for_surface(new_snapshot.surface_id),
        Some(id)
    );
}

#[test]
fn x11_metadata_delta_updates_generic_metadata() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let snapshot = x11_snapshot(generation, 104, 55);
    let id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_x11(id, snapshot.clone()))
        .expect("insert X11 window");

    assert!(state.apply_x11_metadata_delta(
        snapshot.handle,
        crate::xwayland::xwm::X11MetadataDelta::Title(Some("Updated".into()))
    ));
    assert_eq!(
        state.window(id).expect("window").metadata.title.as_deref(),
        Some("Updated")
    );
}

#[test]
fn x11_kind_delta_reclassifies_existing_window_as_override_redirect() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let snapshot = x11_snapshot(generation, 105, 56);
    let id = insert_x11(&mut state, snapshot.clone());

    assert!(state.apply_x11_metadata_delta(
        snapshot.handle,
        X11MetadataDelta::Kind(DesktopWindowKind::OverrideRedirect)
    ));
    let window = state.window(id).expect("window");
    assert_eq!(window.kind, DesktopWindowKind::OverrideRedirect);
    assert_eq!(window.x11_role, Some(X11DesktopRole::OverrideRedirect));
    assert_eq!(window.management, None);
    assert!(state.x11_client_lists().0.is_empty());
}

#[test]
fn x11_client_lists_follow_identity_and_generic_stacking() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let first = x11_snapshot(generation, 107, 58);
    let second = x11_snapshot(generation, 108, 59);
    let mut popup = x11_snapshot(generation, 109, 60);
    popup.kind = DesktopWindowKind::OverrideRedirect;
    let first_id = state.allocate_window_id().expect("first window id");
    let popup_id = state.allocate_window_id().expect("popup window id");
    let second_id = state.allocate_window_id().expect("second window id");
    state
        .insert_desktop_window(DesktopWindow::new_x11(first_id, first.clone()))
        .expect("insert first X11 window");
    state
        .insert_desktop_window(DesktopWindow::new_x11(popup_id, popup))
        .expect("insert override-redirect X11 window");
    state
        .insert_desktop_window(DesktopWindow::new_x11(second_id, second.clone()))
        .expect("insert second X11 window");

    let (client_list, stacking) = state.x11_client_lists();
    assert_eq!(client_list, vec![first.handle, second.handle]);
    assert_eq!(stacking, vec![first.handle, second.handle]);

    assert!(state.raise_window_id(first_id));
    let (_, stacking) = state.x11_client_lists();
    assert_eq!(stacking, vec![second.handle, first.handle]);
}

#[test]
fn popup_menu_is_rendered_but_not_a_desktop_client() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let parent = x11_snapshot(generation, 201, 201);
    let parent_id = insert_x11(&mut state, parent.clone());
    let mut popup = x11_snapshot(generation, 202, 202);
    popup.window_types = X11WindowTypes::new(vec![X11WindowType::PopupMenu]);
    popup.transient_for = Some(parent.handle);
    let popup_id = insert_x11(&mut state, popup.clone());

    assert_eq!(
        state.window(popup_id).unwrap().x11_role,
        Some(X11DesktopRole::AuxiliaryPopup)
    );
    assert_eq!(state.x11_client_lists().0, vec![parent.handle]);
    assert_eq!(
        state.focus_desktop_window(popup_id, WindowFocusReason::ShellActivation),
        WindowFocusOutcome::Unavailable
    );
    assert!(state.x11_focus_request_allowed(parent.handle));
    assert!(state.window(parent_id).unwrap().is_normal_x11_role());
}

#[test]
fn unsupported_leading_window_type_does_not_hide_popup_semantics() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let parent = x11_snapshot(generation, 211, 211);
    insert_x11(&mut state, parent.clone());
    let mut popup = x11_snapshot(generation, 212, 212);
    popup.window_types = X11WindowTypes::new(vec![
        X11WindowType::Other(0xfeed_beef),
        X11WindowType::PopupMenu,
    ]);
    popup.transient_for = Some(parent.handle);
    let popup_id = insert_x11(&mut state, popup);

    assert_eq!(
        state.window(popup_id).unwrap().x11_role,
        Some(X11DesktopRole::AuxiliaryPopup)
    );
    assert_eq!(
        state.focus_desktop_window(popup_id, WindowFocusReason::ShellActivation),
        WindowFocusOutcome::Unavailable
    );
    assert_eq!(state.x11_client_lists().0, vec![parent.handle]);
}

#[test]
fn normal_x11_windows_use_compositor_placement_but_popups_keep_client_position() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let first = x11_snapshot(generation, 213, 213);
    let second = x11_snapshot(generation, 214, 214);
    let first_id = insert_x11(&mut state, first.clone());
    let second_id = insert_x11(&mut state, second.clone());

    assert_eq!(
        state.window(first_id).unwrap().x11_placement_policy,
        Some(X11PlacementPolicy::CompositorManaged)
    );
    assert_eq!(
        state.window(second_id).unwrap().x11_placement_policy,
        Some(X11PlacementPolicy::CompositorManaged)
    );
    assert_eq!(
        state.surface_placement(first.surface_id),
        SurfacePlacement::absolute_root_at(
            crate::compositor::render::FIRST_SURFACE_OFFSET.0,
            crate::compositor::render::FIRST_SURFACE_OFFSET.1,
        )
    );
    assert_eq!(
        state.surface_placement(second.surface_id).root_mode,
        crate::compositor::RootPlacementMode::Absolute
    );
    assert_ne!(
        state.surface_placement(second.surface_id),
        state.surface_placement(first.surface_id)
    );

    let mut popup = x11_snapshot(generation, 215, 215);
    popup.window_types = X11WindowTypes::new(vec![X11WindowType::PopupMenu]);
    popup.transient_for = Some(first.handle);
    let popup_id = insert_x11(&mut state, popup.clone());
    assert!(state.set_x11_geometry(popup.handle, popup.geometry));
    assert_eq!(
        state.window(popup_id).unwrap().x11_placement_policy,
        Some(X11PlacementPolicy::ClientPositioned)
    );
    assert_eq!(
        state.surface_placement(popup.surface_id),
        SurfacePlacement::absolute_root_at(popup.geometry.x, popup.geometry.y)
    );
}

#[test]
fn normal_x11_windows_have_distinct_stable_absolute_frame_geometry() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let first = x11_snapshot(generation, 219, 219);
    let second = x11_snapshot(generation, 220, 220);
    let first_id = insert_x11(&mut state, first.clone());
    let second_id = insert_x11(&mut state, second.clone());

    let first_frame = state.window(first_id).unwrap().x11_geometry.unwrap().frame;
    let second_frame = state.window(second_id).unwrap().x11_geometry.unwrap().frame;
    assert_eq!(
        first_frame.placement.root_mode,
        crate::compositor::RootPlacementMode::Absolute
    );
    assert_eq!(
        second_frame.placement.root_mode,
        crate::compositor::RootPlacementMode::Absolute
    );
    assert_ne!(first_frame.placement, second_frame.placement);
    assert_eq!(
        state.surface_placement(first.surface_id),
        first_frame.placement
    );
    assert_eq!(
        state.surface_placement(second.surface_id),
        second_frame.placement
    );

    assert!(state.raise_window_id(first_id));
    assert_eq!(
        state.window(first_id).unwrap().x11_geometry.unwrap().frame,
        first_frame
    );
    assert_eq!(
        state.window(second_id).unwrap().x11_geometry.unwrap().frame,
        second_frame
    );
}

#[test]
fn managed_x11_placement_avoids_existing_xdg_frames() {
    let mut state = CompositorState::new(None);
    let xdg_a = state.allocate_window_id().expect("xdg window id");
    let xdg_b = state.allocate_window_id().expect("xdg window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(xdg_a, 301))
        .expect("insert xdg A");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(xdg_b, 302))
        .expect("insert xdg B");

    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let x11 = x11_snapshot(generation, 303, 303);
    let x11_id = insert_x11(&mut state, x11.clone());
    let frame = state.window(x11_id).unwrap().x11_geometry.unwrap().frame;

    assert_ne!(
        frame.placement,
        SurfacePlacement::absolute_root_at(
            crate::compositor::render::FIRST_SURFACE_OFFSET.0,
            crate::compositor::render::FIRST_SURFACE_OFFSET.1,
        )
    );
    assert_ne!(
        frame.placement,
        SurfacePlacement::absolute_root_at(
            crate::compositor::render::FIRST_SURFACE_OFFSET.0
                + crate::compositor::render::SURFACE_CASCADE_STEP,
            crate::compositor::render::FIRST_SURFACE_OFFSET.1
                + crate::compositor::render::SURFACE_CASCADE_STEP,
        )
    );
}

#[test]
fn managed_x11_placement_stays_visible_when_overlap_is_unavoidable() {
    let mut state = CompositorState::new(None);
    assert!(state.set_output_size(1_920, 1_080));
    let xdg = state.allocate_window_id().expect("xdg window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(xdg, 304))
        .expect("insert occupying xdg window");

    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let mut steam = x11_snapshot(generation, 305, 305);
    steam.geometry.width = 1_898;
    steam.geometry.height = 1_013;
    let steam_id = insert_x11(&mut state, steam);
    let frame = state.window(steam_id).unwrap().x11_geometry.unwrap().frame;
    let usable = state.usable_output_geometry();

    assert!(frame.placement.local_x >= usable.x as i32);
    assert!(frame.placement.local_y >= usable.y as i32);
    assert!(
        i64::from(frame.placement.local_x) + i64::from(frame.width)
            <= (usable.x + usable.width) as i64
    );
    assert!(
        i64::from(frame.placement.local_y) + i64::from(frame.height)
            <= (usable.y + usable.height) as i64
    );

    assert!(state.set_root_window_mode(305, ToplevelMode::Fullscreen));
    assert!(state.restore_normal_root_window(305));
    assert_eq!(state.surface_placement(305), frame.placement);
}

#[test]
fn managed_x11_placement_reuses_a_free_hole_not_an_occupied_slot() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let _first = insert_x11(&mut state, x11_snapshot(generation, 304, 304));
    let middle = insert_x11(&mut state, x11_snapshot(generation, 305, 305));
    let third = insert_x11(&mut state, x11_snapshot(generation, 306, 306));
    let third_frame = state.window(third).unwrap().x11_geometry.unwrap().frame;

    state.remove_desktop_window(middle);
    let fourth = insert_x11(&mut state, x11_snapshot(generation, 307, 307));
    let fourth_frame = state.window(fourth).unwrap().x11_geometry.unwrap().frame;

    assert_ne!(
        fourth_frame.placement, third_frame.placement,
        "new managed windows must not reuse a rectangle still occupied by a survivor"
    );
}

#[test]
fn dialog_remains_a_managed_client() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let parent = x11_snapshot(generation, 203, 203);
    insert_x11(&mut state, parent.clone());
    let mut dialog = x11_snapshot(generation, 204, 204);
    dialog.window_types = X11WindowTypes::new(vec![X11WindowType::Dialog]);
    dialog.transient_for = Some(parent.handle);
    insert_x11(&mut state, dialog.clone());
    assert_eq!(
        state.x11_client_lists().0,
        vec![parent.handle, dialog.handle]
    );
    assert_eq!(state.window_stacking.len(), 2);
}

#[test]
fn generic_transient_is_parent_relative_floating() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let parent = x11_snapshot(generation, 221, 221);
    insert_x11(&mut state, parent.clone());
    let mut transient = x11_snapshot(generation, 222, 222);
    transient.transient_for = Some(parent.handle);
    let transient_id = insert_x11(&mut state, transient.clone());

    assert_eq!(
        state.window(transient_id).unwrap().x11_role,
        Some(X11DesktopRole::Dialog)
    );
    assert_eq!(
        state.window(transient_id).unwrap().x11_placement_policy,
        Some(X11PlacementPolicy::ParentRelative)
    );
    assert_eq!(
        state.surface_placement(transient.surface_id),
        SurfacePlacement::absolute_root_at(transient.geometry.x, transient.geometry.y)
    );
}

#[test]
fn normal_type_with_transient_parent_is_parent_relative_floating() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let parent = x11_snapshot(generation, 223, 223);
    insert_x11(&mut state, parent.clone());
    let mut transient = x11_snapshot(generation, 224, 224);
    transient.window_types = X11WindowTypes::new(vec![X11WindowType::Normal]);
    transient.transient_for = Some(parent.handle);
    let transient_id = insert_x11(&mut state, transient);

    assert_eq!(
        state.window(transient_id).unwrap().x11_role,
        Some(X11DesktopRole::Dialog)
    );
    assert_eq!(
        state.window(transient_id).unwrap().x11_placement_policy,
        Some(X11PlacementPolicy::ParentRelative)
    );
}

#[test]
fn dialog_without_transient_is_floating_before_parent_metadata_arrives() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let mut dialog = x11_snapshot(generation, 225, 225);
    dialog.window_types = X11WindowTypes::new(vec![X11WindowType::Dialog]);
    let dialog_id = insert_x11(&mut state, dialog);

    assert_eq!(
        state.window(dialog_id).unwrap().x11_role,
        Some(X11DesktopRole::Dialog)
    );
    assert_eq!(
        state.window(dialog_id).unwrap().x11_placement_policy,
        Some(X11PlacementPolicy::ParentRelative)
    );
}

#[test]
fn late_popup_to_toplevel_reclassification_migrates_to_managed_placement() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let mut popup = x11_snapshot(generation, 226, 226);
    popup.window_types = X11WindowTypes::new(vec![X11WindowType::PopupMenu]);
    popup.geometry.x = 300;
    popup.geometry.y = 400;
    let popup_id = insert_x11(&mut state, popup.clone());
    assert!(state.set_x11_geometry(popup.handle, popup.geometry));
    assert_eq!(
        state.surface_placement(popup.surface_id),
        SurfacePlacement::absolute_root_at(300, 400)
    );

    assert!(state.apply_x11_metadata_delta(
        popup.handle,
        X11MetadataDelta::WindowTypes(X11WindowTypes::new(vec![X11WindowType::Normal]))
    ));
    assert_eq!(
        state.window(popup_id).unwrap().x11_placement_policy,
        Some(X11PlacementPolicy::CompositorManaged)
    );
    assert_eq!(
        state.surface_placement(popup.surface_id),
        SurfacePlacement::absolute_root_at(
            crate::compositor::render::FIRST_SURFACE_OFFSET.0,
            crate::compositor::render::FIRST_SURFACE_OFFSET.1,
        )
    );
}

#[test]
fn transient_family_raise_preserves_parent_below_child() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let parent = x11_snapshot(generation, 205, 205);
    let parent_id = insert_x11(&mut state, parent.clone());
    let unrelated = insert_x11(&mut state, x11_snapshot(generation, 206, 206));
    let mut popup = x11_snapshot(generation, 207, 207);
    popup.window_types = X11WindowTypes::new(vec![X11WindowType::Menu]);
    popup.transient_for = Some(parent.handle);
    let popup_id = insert_x11(&mut state, popup);

    assert!(state.raise_window_id(parent_id));
    assert_eq!(state.window_stacking, vec![unrelated, parent_id, popup_id]);
    assert!(state.raise_window_id(popup_id));
    assert_eq!(state.window_stacking, vec![unrelated, parent_id, popup_id]);
}

#[test]
fn raising_one_transient_sibling_does_not_raise_the_other_sibling() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let parent = x11_snapshot(generation, 216, 216);
    let parent_id = insert_x11(&mut state, parent.clone());
    let mut first = x11_snapshot(generation, 217, 217);
    first.window_types = X11WindowTypes::new(vec![X11WindowType::PopupMenu]);
    first.transient_for = Some(parent.handle);
    let first_id = insert_x11(&mut state, first);
    let mut second = x11_snapshot(generation, 218, 218);
    second.window_types = X11WindowTypes::new(vec![X11WindowType::PopupMenu]);
    second.transient_for = Some(parent.handle);
    let second_id = insert_x11(&mut state, second);

    assert_eq!(state.window_stacking, vec![parent_id, first_id, second_id]);
    assert!(state.raise_window_id(first_id));
    assert_eq!(state.window_stacking, vec![parent_id, second_id, first_id]);
}

#[test]
fn popup_layer_stays_above_normal_window_when_normal_window_is_raised() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let mut popup = x11_snapshot(generation, 219, 219);
    popup.window_types = X11WindowTypes::new(vec![X11WindowType::PopupMenu]);
    let popup_id = insert_x11(&mut state, popup);
    let normal_id = insert_x11(&mut state, x11_snapshot(generation, 220, 220));

    assert_eq!(state.window_stacking, vec![normal_id, popup_id]);
    assert!(state.raise_window_id(normal_id));
    assert_eq!(state.window_stacking, vec![normal_id, popup_id]);
}

#[test]
fn x11_stack_request_moves_one_window_without_reordering_siblings() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let first = x11_snapshot(generation, 221, 221);
    let second = x11_snapshot(generation, 222, 222);
    let first_id = insert_x11(&mut state, first.clone());
    let second_id = insert_x11(&mut state, second.clone());

    assert!(state.apply_x11_stack_request(first.handle, Some(second.handle), X11StackMode::Above,));
    assert_eq!(state.window_stacking, vec![second_id, first_id]);
    assert!(!state.apply_x11_stack_request(first.handle, None, X11StackMode::Above,));
}

#[test]
fn raising_x11_window_queues_backend_restack() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let first = x11_snapshot(generation, 227, 227);
    let second = x11_snapshot(generation, 228, 228);
    let first_id = insert_x11(&mut state, first);
    let _second_id = insert_x11(&mut state, second);
    let _ = state.take_backend_commands();

    assert!(state.raise_window_id(first_id));
    assert!(state.take_backend_commands().iter().any(|command| matches!(
        command,
        crate::compositor::window_backend::WindowBackendCommand::RestackExact { windows }
            if windows.contains(&first_id)
    )));
}

#[test]
fn transient_child_cannot_be_stacked_below_parent() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let parent = x11_snapshot(generation, 229, 229);
    let parent_id = insert_x11(&mut state, parent.clone());
    let mut child = x11_snapshot(generation, 230, 230);
    child.transient_for = Some(parent.handle);
    let child_id = insert_x11(&mut state, child.clone());

    let _ = state.apply_x11_stack_request(child.handle, Some(parent.handle), X11StackMode::Below);
    assert_eq!(state.window_stacking, vec![parent_id, child_id]);
}

#[test]
fn dynamic_transient_for_rebuilds_family_and_rejects_cycles() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let parent = x11_snapshot(generation, 208, 208);
    let parent_id = insert_x11(&mut state, parent.clone());
    let child = x11_snapshot(generation, 209, 209);
    let child_id = insert_x11(&mut state, child.clone());
    assert!(state.apply_x11_metadata_delta(
        child.handle,
        X11MetadataDelta::TransientFor(Some(parent.handle))
    ));
    assert_eq!(
        state.window(child_id).unwrap().relationships.transient_for,
        Some(parent_id)
    );
    assert!(state.apply_x11_metadata_delta(
        parent.handle,
        X11MetadataDelta::TransientFor(Some(child.handle))
    ));
    assert_eq!(
        state.window(parent_id).unwrap().relationships.transient_for,
        None
    );
}

#[test]
fn dynamic_transient_for_reorders_child_above_parent_immediately() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let parent = x11_snapshot(generation, 210, 210);
    let parent_id = insert_x11(&mut state, parent.clone());
    let child = x11_snapshot(generation, 211, 211);
    let child_id = insert_x11(&mut state, child.clone());
    state.window_stacking.reverse();

    assert!(state.apply_x11_metadata_delta(
        child.handle,
        X11MetadataDelta::TransientFor(Some(parent.handle))
    ));
    assert_eq!(state.window_stacking, vec![parent_id, child_id]);
}

#[test]
fn transient_reorder_anchors_at_highest_family_position() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let parent = x11_snapshot(generation, 214, 214);
    let parent_id = insert_x11(&mut state, parent.clone());
    let unrelated_id = insert_x11(&mut state, x11_snapshot(generation, 215, 215));
    let child = x11_snapshot(generation, 216, 216);
    let child_id = insert_x11(&mut state, child.clone());
    state.window_stacking = vec![child_id, unrelated_id, parent_id];

    assert!(state.apply_x11_metadata_delta(
        child.handle,
        X11MetadataDelta::TransientFor(Some(parent.handle))
    ));
    assert_eq!(
        state.window_stacking,
        vec![unrelated_id, parent_id, child_id]
    );
}

#[test]
fn admitting_missing_transient_parent_reorders_existing_child() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let parent = x11_snapshot(generation, 212, 212);
    let child_handle = X11WindowHandle::new(generation, 213);
    let mut child = x11_snapshot(generation, child_handle.xid(), 213);
    child.transient_for = Some(parent.handle);
    let child_id = insert_x11(&mut state, child);
    assert_eq!(state.window_stacking, vec![child_id]);

    let parent_id = insert_x11(&mut state, parent);
    assert_eq!(state.window_stacking, vec![parent_id, child_id]);
}

#[test]
fn background_x11_activation_request_is_denied() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let first = x11_snapshot(generation, 109, 60);
    let second = x11_snapshot(generation, 110, 61);
    let first_id = state.allocate_window_id().expect("first window id");
    let second_id = state.allocate_window_id().expect("second window id");
    state
        .insert_desktop_window(DesktopWindow::new_x11(first_id, first.clone()))
        .expect("insert first X11 window");
    state
        .insert_desktop_window(DesktopWindow::new_x11(second_id, second.clone()))
        .expect("insert second X11 window");
    state.focused_window_id = Some(first_id);

    assert!(!state.x11_focus_request_allowed(second.handle));
    assert!(state.x11_focus_request_allowed(first.handle));
}

#[test]
fn x11_fullscreen_uses_output_geometry_and_maximize_publishes_both_axes() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let snapshot = x11_snapshot(generation, 111, 62);
    let id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_x11(id, snapshot.clone()))
        .expect("insert X11 window");

    let maximized = state
        .apply_x11_state_request(
            snapshot.handle,
            crate::xwayland::xwm::X11StateRequest {
                action: crate::xwayland::xwm::X11StateAction::Add,
                first: Some(crate::xwayland::xwm::X11StateAtom::MaximizedHorizontal),
                second: Some(crate::xwayland::xwm::X11StateAtom::MaximizedVertical),
            },
        )
        .expect("maximized state");
    assert!(maximized.maximized);
    assert_eq!(
        state.window(id).expect("window").state.mode(),
        ToplevelMode::Maximized
    );
    assert_eq!(
        state.surface_placement(62),
        state
            .window_geometry_for_surface_mode(62, ToplevelMode::Maximized)
            .placement
    );

    let fullscreen = state
        .apply_x11_state_request(
            snapshot.handle,
            crate::xwayland::xwm::X11StateRequest {
                action: crate::xwayland::xwm::X11StateAction::Add,
                first: Some(crate::xwayland::xwm::X11StateAtom::Fullscreen),
                second: None,
            },
        )
        .expect("fullscreen state");
    assert!(fullscreen.fullscreen);
    assert_eq!(
        state.surface_placement(62),
        state.fullscreen_window_geometry().placement
    );
    let fullscreen_geometry = state.fullscreen_window_geometry();
    assert!(state.take_backend_commands().iter().any(|command| matches!(
        command,
        crate::compositor::window_backend::WindowBackendCommand::Configure {
            window,
            geometry,
            mode: ToplevelMode::Fullscreen,
            resizing: false,
        } if *window == id && *geometry == fullscreen_geometry
    )));
}

#[test]
fn pre_map_fullscreen_snapshot_enters_fullscreen_on_admission() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let mut snapshot = x11_snapshot(generation, 113, 64);
    snapshot.state.fullscreen = true;
    let id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_x11(id, snapshot.clone()))
        .expect("insert X11 window");

    assert!(state.apply_initial_x11_state(snapshot.handle, snapshot.state, snapshot.geometry));
    assert_eq!(
        state.window(id).expect("window").state.mode(),
        ToplevelMode::Fullscreen
    );
    assert_eq!(
        state.surface_placement(snapshot.surface_id),
        state.fullscreen_window_geometry().placement
    );
    assert!(state.take_backend_commands().iter().any(|command| matches!(
        command,
        crate::compositor::window_backend::WindowBackendCommand::Configure {
            window,
            mode: ToplevelMode::Fullscreen,
            resizing: false,
            ..
        } if *window == id
    )));
    assert!(state.restore_normal_root_window(snapshot.surface_id));
    assert_eq!(
        state.window(id).expect("window").state.mode(),
        ToplevelMode::Normal
    );
    assert_eq!(
        state.surface_placement(snapshot.surface_id),
        SurfacePlacement::absolute_root_at(
            crate::compositor::render::FIRST_SURFACE_OFFSET.0,
            crate::compositor::render::FIRST_SURFACE_OFFSET.1,
        )
    );
}

#[test]
fn pre_map_maximized_snapshot_uses_usable_output_geometry() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let mut snapshot = x11_snapshot(generation, 114, 65);
    snapshot.state.maximized = true;
    snapshot.state.fullscreen = true;
    let id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_x11(id, snapshot.clone()))
        .expect("insert X11 window");

    assert!(state.apply_initial_x11_state(snapshot.handle, snapshot.state, snapshot.geometry));
    assert_eq!(
        state.window(id).expect("window").state.mode(),
        ToplevelMode::Fullscreen
    );

    let mut maximized_snapshot = snapshot;
    maximized_snapshot.handle = X11WindowHandle::new(generation, 115);
    maximized_snapshot.surface_id = 66;
    maximized_snapshot.state.fullscreen = false;
    let maximized_id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_x11(
            maximized_id,
            maximized_snapshot.clone(),
        ))
        .expect("insert maximized X11 window");
    assert!(state.apply_initial_x11_state(
        maximized_snapshot.handle,
        maximized_snapshot.state,
        maximized_snapshot.geometry
    ));
    assert_eq!(
        state.surface_placement(maximized_snapshot.surface_id),
        state
            .window_geometry_for_surface_mode(
                maximized_snapshot.surface_id,
                ToplevelMode::Maximized,
            )
            .placement
    );
}

#[test]
fn x11_resize_queues_a_typed_backend_command() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let snapshot = x11_snapshot(generation, 112, 63);
    let id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_x11(id, snapshot))
        .expect("insert X11 window");

    state.queue_backend_configure(
        id,
        WindowGeometry::new(SurfacePlacement::root_at(30, 40), 1024, 768),
        ToplevelMode::Normal,
        true,
    );
    let commands = state.take_backend_commands();
    assert_eq!(commands.len(), 1);
    assert!(matches!(
        commands[0],
        crate::compositor::window_backend::WindowBackendCommand::Configure {
            window,
            resizing: true,
            ..
        } if window == id
    ));
}

#[test]
fn override_redirect_window_is_excluded_from_normal_window_cycle() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let mut snapshot = x11_snapshot(generation, 105, 56);
    snapshot.kind = DesktopWindowKind::OverrideRedirect;
    let id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_x11(id, snapshot))
        .expect("insert override-redirect window");

    assert_eq!(
        state.window(id).expect("window").kind,
        DesktopWindowKind::OverrideRedirect
    );
    assert!(!state.window(id).expect("window").state.is_minimized());
}

#[test]
fn x11_configure_request_is_filtered_by_generic_constraints() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let snapshot = x11_snapshot(generation, 106, 57);
    let id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_x11(id, snapshot.clone()))
        .expect("insert X11 window");
    state.window_mut(id).expect("window").constraints = WindowConstraints {
        min_width: Some(400),
        min_height: Some(300),
        max_width: Some(1000),
        max_height: Some(900),
        ..WindowConstraints::default()
    };

    let filtered = state.filter_x11_geometry(
        snapshot.handle,
        X11Geometry {
            x: -20,
            y: 30,
            width: 1200,
            height: 100,
        },
    );
    assert_eq!(
        filtered,
        Some(X11Geometry {
            x: -20,
            y: 30,
            width: 1000,
            height: 300,
        })
    );
}

#[test]
fn x11_published_state_updates_generic_window_state() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let snapshot = x11_snapshot(generation, 107, 58);
    let id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_x11(id, snapshot.clone()))
        .expect("insert X11 window");

    assert!(state.apply_x11_published_state(
        snapshot.handle,
        X11PublishedState {
            fullscreen: true,
            maximized: false,
            hidden: false,
            activated: true,
        }
    ));
    assert_eq!(
        state.window(id).expect("window").state.mode(),
        ToplevelMode::Fullscreen
    );
}

#[test]
fn same_window_focus_refresh_does_not_requeue_backend_activation() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let id = insert_x11(&mut state, x11_snapshot(generation, 301, 301));
    state.focused_window_id = Some(id);
    let _ = state.take_backend_commands();

    assert_eq!(state.update_desktop_focus_window(301, true), Some(id));
    assert!(state.take_backend_commands().is_empty());
}

#[test]
fn pointer_enter_focus_rejects_auxiliary_x11_windows() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let mut popup = x11_snapshot(generation, 302, 302);
    popup.window_types = X11WindowTypes::new(vec![X11WindowType::PopupMenu]);
    let popup_id = insert_x11(&mut state, popup);

    assert_eq!(
        state.focus_desktop_window(popup_id, WindowFocusReason::PointerEnter),
        WindowFocusOutcome::Unavailable
    );
}

#[test]
fn pointer_press_activation_is_a_noop_for_focused_topmost_window() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let id = insert_x11(&mut state, x11_snapshot(generation, 303, 303));
    state.focused_window_id = Some(id);
    let _ = state.take_backend_commands();

    assert_eq!(
        state.activate_desktop_window(id, WindowFocusReason::PointerPress),
        WindowActivationOutcome::Unavailable
    );
    assert!(state.take_backend_commands().is_empty());
}

#[test]
fn pointer_press_activation_restores_minimized_window() {
    let mut state = CompositorState::new(None);
    let id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(id, 304))
        .expect("insert window");
    state
        .window_mut(id)
        .expect("window")
        .state
        .mark_minimized_without_surfaces();

    let outcome = state.activate_desktop_window(id, WindowFocusReason::PointerPress);

    assert_eq!(outcome, WindowActivationOutcome::Unavailable);
    assert!(state.window(id).expect("window").state.is_minimized());
    assert_eq!(state.focused_window_id, None);
}

#[test]
fn exact_window_action_outcomes_distinguish_unavailable_and_no_change() {
    let mut state = CompositorState::new(None);
    let id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(id, 306))
        .expect("insert window");

    assert_eq!(
        state.minimize_desktop_window_outcome(WindowId::new(NonZeroU64::new(999).unwrap())),
        WindowActionOutcome::Unavailable
    );
    assert_eq!(
        state.restore_minimized_desktop_window_outcome(id),
        WindowActionOutcome::NoChange
    );

    state
        .window_mut(id)
        .expect("window")
        .state
        .mark_minimized_without_surfaces();
    assert_eq!(
        state.minimize_desktop_window_outcome(id),
        WindowActionOutcome::NoChange
    );
}

#[test]
fn exact_x11_close_uses_the_existing_backend_close_command() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let id = insert_x11(&mut state, x11_snapshot(generation, 307, 307));
    let _ = state.take_backend_commands();

    assert_eq!(
        state.close_desktop_window_outcome(id),
        WindowActionOutcome::Changed
    );
    assert!(matches!(
        state.take_backend_commands().as_slice(),
        [crate::compositor::window_backend::WindowBackendCommand::Close { window }] if *window == id
    ));
}

#[test]
fn raise_window_id_is_a_noop_when_the_window_family_is_already_topmost() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let id = insert_x11(&mut state, x11_snapshot(generation, 305, 305));
    let _ = state.take_backend_commands();

    assert!(!state.raise_window_id(id));
    assert!(state.take_backend_commands().is_empty());
}

#[test]
fn already_topmost_transient_family_does_not_queue_duplicate_restack() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let parent = x11_snapshot(generation, 306, 306);
    let parent_id = insert_x11(&mut state, parent.clone());
    let mut dialog = x11_snapshot(generation, 307, 307);
    dialog.window_types = X11WindowTypes::new(vec![X11WindowType::Dialog]);
    dialog.transient_for = Some(parent.handle);
    let dialog_id = insert_x11(&mut state, dialog);
    let _ = state.take_backend_commands();

    assert_eq!(state.window_stacking, vec![parent_id, dialog_id]);
    assert!(state.raise_window_id(parent_id));
    assert!(state.take_backend_commands().is_empty());
}
