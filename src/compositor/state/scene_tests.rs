use super::*;
use std::os::unix::net::UnixStream;
use std::num::NonZeroU64;
use std::sync::Arc;

use crate::xwayland::xwm::{X11Geometry, X11PublishedState, X11WindowSnapshot, X11WindowTypes};
use crate::xwayland::{X11WindowHandle, XwaylandGeneration};

fn test_surface(state: &mut CompositorState) -> (wayland_server::Display<CompositorState>, u32) {
    let display = wayland_server::Display::<CompositorState>::new().expect("test display");
    let mut display_handle = display.handle();
    let (server_end, _peer) = UnixStream::pair().expect("test client socket");
    let client = display_handle
        .insert_client(server_end, Arc::new(()))
        .expect("test client");
    let surface =
        state.test_create_unmapped_surface_resource_at_version(&client, &display_handle, 1);
    (display, compositor_surface_id(&surface))
}

fn test_x11_snapshot(generation: XwaylandGeneration, surface_id: u32) -> X11WindowSnapshot {
    X11WindowSnapshot {
        handle: X11WindowHandle::new(generation, 0x100),
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
        metadata: WindowMetadata::default(),
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

#[test]
fn surface_scene_identity_follows_wl_surface_lifetime() {
    let mut state = CompositorState::default();
    let (display, surface_id) = test_surface(&mut state);
    let node = state
        .scene_node_id_for_surface(surface_id)
        .expect("surface scene node");

    state.unregister_surface_resource(surface_id);

    assert!(state.scene_node_id_for_surface(surface_id).is_none());
    let (_replacement_display, replacement_id) = test_surface(&mut state);
    let replacement_node = state
        .scene_node_id_for_surface(replacement_id)
        .expect("replacement scene node");
    assert_ne!(node, replacement_node);
    drop(display);
}

#[test]
fn role_metadata_changes_without_replacing_surface_scene_identity() {
    let mut state = CompositorState::default();
    let (_display, surface_id) = test_surface(&mut state);
    let node = state
        .scene_node_id_for_surface(surface_id)
        .expect("surface scene node");

    state
        .assign_surface_role(surface_id, SurfaceRole::Xwayland)
        .expect("xwayland role");

    let metadata = state
        .scene_node_metadata_for_surface(surface_id)
        .expect("scene metadata");
    assert_eq!(metadata.id, node);
    assert_eq!(metadata.role, SceneRole::ClientSurface);
}

#[test]
fn desktop_window_has_stable_group_and_decoration_nodes() {
    let mut state = CompositorState::default();
    let (_display, surface_id) = test_surface(&mut state);
    state
        .assign_surface_role(surface_id, SurfaceRole::XdgToplevel)
        .expect("toplevel role");
    let window_id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(window_id, surface_id))
        .expect("desktop window");

    let group = state
        .scene_registry
        .node_for_source(SceneSource::WindowGroup(window_id))
        .expect("window group");
    let decoration = state
        .scene_registry
        .node_for_source(SceneSource::ServerDecoration(window_id))
        .expect("server decoration");
    assert_ne!(group, decoration);
    assert_eq!(
        state
            .scene_node_metadata_for_surface(surface_id)
            .unwrap()
            .visual_parent,
        Some(group)
    );
    assert_eq!(
        state.scene_registry.metadata(decoration).unwrap().visual_parent,
        Some(group)
    );

    state.remove_desktop_window(window_id).expect("remove window");
    assert!(
        state
            .scene_registry
            .node_for_source(SceneSource::WindowGroup(window_id))
            .is_none()
    );
    assert_eq!(
        state
            .scene_node_metadata_for_surface(surface_id)
            .unwrap()
            .visual_parent,
        None
    );
}

#[test]
fn xwayland_surface_replacement_preserves_window_group_identity() {
    let mut state = CompositorState::default();
    let (display_a, surface_a) = test_surface(&mut state);
    let (display_b, surface_b) = test_surface(&mut state);
    state
        .assign_surface_role(surface_a, SurfaceRole::Xwayland)
        .expect("first xwayland role");
    state
        .assign_surface_role(surface_b, SurfaceRole::Xwayland)
        .expect("replacement xwayland role");
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let snapshot = test_x11_snapshot(generation, surface_a);
    let handle = snapshot.handle;
    let window_id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_x11(window_id, snapshot))
        .expect("xwayland window");
    let group = state
        .scene_registry
        .node_for_source(SceneSource::WindowGroup(window_id))
        .expect("window group");
    let node_a = state.scene_node_id_for_surface(surface_a).unwrap();
    let node_b = state.scene_node_id_for_surface(surface_b).unwrap();

    assert_eq!(
        state
            .scene_node_metadata_for_surface(surface_a)
            .unwrap()
            .visual_parent,
        Some(group)
    );
    assert_eq!(
        state.attach_x11_surface(handle, surface_b),
        Ok(Some(surface_a))
    );
    assert_eq!(
        state
            .scene_node_metadata_for_surface(surface_a)
            .unwrap()
            .visual_parent,
        None
    );
    assert_eq!(
        state
            .scene_node_metadata_for_surface(surface_b)
            .unwrap()
            .visual_parent,
        Some(group)
    );
    assert_eq!(
        state
            .scene_registry
            .node_for_source(SceneSource::WindowGroup(window_id)),
        Some(group)
    );
    assert_ne!(node_a, node_b);
    drop(display_a);
    drop(display_b);
}
