use super::*;
use crate::xwayland::xwm::{X11Geometry, X11PublishedState, X11WindowSnapshot};
use crate::xwayland::{X11WindowHandle, XwaylandGeneration};
use std::num::NonZeroU64;

fn x11_snapshot(generation: XwaylandGeneration, xid: u32, surface_id: u32) -> X11WindowSnapshot {
    X11WindowSnapshot {
        handle: X11WindowHandle::new(generation, xid),
        surface_id,
        kind: DesktopWindowKind::Managed,
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
        sync_counter: None,
    }
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
fn x11_client_lists_follow_identity_and_generic_stacking() {
    let mut state = CompositorState::new(None);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let first = x11_snapshot(generation, 107, 58);
    let second = x11_snapshot(generation, 108, 59);
    let first_id = state.allocate_window_id().expect("first window id");
    let second_id = state.allocate_window_id().expect("second window id");
    state
        .insert_desktop_window(DesktopWindow::new_x11(first_id, first.clone()))
        .expect("insert first X11 window");
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
                first: Some(crate::xwayland::xwm::X11StateAtom::Maximized),
                second: None,
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
        state.maximized_window_geometry().placement
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
        ToplevelMode::Floating,
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
