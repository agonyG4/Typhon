use super::*;
use std::num::NonZeroU64;
use std::os::unix::net::UnixStream;
use std::sync::Arc;

use crate::xwayland::xwm::{X11Geometry, X11PublishedState, X11WindowSnapshot, X11WindowTypes};
use crate::xwayland::{X11WindowHandle, XwaylandGeneration};

fn test_client(display: &mut wayland_server::Display<CompositorState>) -> wayland_server::Client {
    let mut display_handle = display.handle();
    let (server_end, _peer) = UnixStream::pair().expect("test client socket");
    display_handle
        .insert_client(server_end, Arc::new(()))
        .expect("test client")
}

fn test_surface_for_client(
    state: &mut CompositorState,
    display: &mut wayland_server::Display<CompositorState>,
    client: &wayland_server::Client,
) -> u32 {
    let display_handle = display.handle();
    let surface =
        state.test_create_unmapped_surface_resource_at_version(client, &display_handle, 1);
    compositor_surface_id(&surface)
}

fn test_surface(state: &mut CompositorState) -> (wayland_server::Display<CompositorState>, u32) {
    let mut display = wayland_server::Display::<CompositorState>::new().expect("test display");
    let client = test_client(&mut display);
    let surface_id = test_surface_for_client(state, &mut display, &client);
    (display, surface_id)
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
fn subsurface_role_uses_parent_surface_and_detaches_only_live_edge() {
    let mut state = CompositorState::default();
    let mut display = wayland_server::Display::<CompositorState>::new().expect("test display");
    let client = test_client(&mut display);
    let parent_id = test_surface_for_client(&mut state, &mut display, &client);
    let child_id = test_surface_for_client(&mut state, &mut display, &client);
    state
        .assign_surface_role(child_id, SurfaceRole::Subsurface { parent_id })
        .expect("subsurface role");
    let parent_node = state.scene_node_id_for_surface(parent_id).unwrap();

    assert_eq!(
        state
            .scene_node_metadata_for_surface(child_id)
            .unwrap()
            .visual_parent,
        Some(parent_node)
    );
    state.deactivate_role_instance(child_id);
    let metadata = state.scene_node_metadata_for_surface(child_id).unwrap();
    assert_eq!(metadata.role, SceneRole::Subsurface);
    assert_eq!(metadata.visual_parent, None);
    drop(display);
}

#[test]
fn role_rollback_restores_unassigned_scene_metadata() {
    let mut state = CompositorState::default();
    let (display, surface_id) = test_surface(&mut state);
    state
        .assign_surface_role(surface_id, SurfaceRole::DragIcon)
        .expect("drag icon role");
    state.deactivate_role_instance(surface_id);
    state.rollback_surface_role_reservation(surface_id, SurfaceRole::DragIcon);
    let metadata = state.scene_node_metadata_for_surface(surface_id).unwrap();

    assert_eq!(metadata.role, SceneRole::UnassignedSurface);
    assert_eq!(
        metadata.domain,
        SceneDomainAssignment::Inherit {
            fallback: SceneDomain::Content
        }
    );
    drop(display);
}

#[test]
fn input_surface_roles_receive_input_domain_without_render_changes() {
    let mut state = CompositorState::default();
    let mut display = wayland_server::Display::<CompositorState>::new().expect("test display");
    let client = test_client(&mut display);
    let cursor_id = test_surface_for_client(&mut state, &mut display, &client);
    let drag_id = test_surface_for_client(&mut state, &mut display, &client);
    state
        .assign_surface_role(cursor_id, SurfaceRole::Cursor)
        .expect("cursor role");
    state
        .assign_surface_role(drag_id, SurfaceRole::DragIcon)
        .expect("drag icon role");

    assert_eq!(
        state
            .scene_node_metadata_for_surface(cursor_id)
            .unwrap()
            .domain,
        SceneDomainAssignment::Explicit(SceneDomain::Input)
    );
    assert_eq!(
        state
            .scene_node_metadata_for_surface(drag_id)
            .unwrap()
            .domain,
        SceneDomainAssignment::Explicit(SceneDomain::Input)
    );
    drop(display);
}

#[test]
fn active_scene_projection_keeps_canonical_identity_across_visibility() {
    let mut state = CompositorState::default();
    let (display, surface_id) = test_surface(&mut state);
    state.test_publish_surface(surface_id, 32, 32, SurfacePlacement::root());
    state.rebuild_active_scene_view();
    let node = state.active_scene_node_for_surface(surface_id).unwrap();

    assert_eq!(
        state.active_scene_surface_index_for_node(node),
        state.active_scene_surface_index(surface_id)
    );
    state.test_unmap_surface(surface_id);
    state.rebuild_active_scene_view();
    assert!(state.active_scene_node_for_surface(surface_id).is_none());
    assert!(state.scene_node_id_for_surface(surface_id).is_some());
    state.test_map_surface(surface_id, 32, 32, SurfacePlacement::root());
    state.rebuild_active_scene_view();
    assert_eq!(state.active_scene_node_for_surface(surface_id), Some(node));
    drop(display);
}

#[test]
fn desktop_window_has_stable_group_and_decoration_nodes() {
    let mut state = CompositorState::default();
    let (_display, surface_id) = test_surface(&mut state);
    state
        .assign_surface_role(surface_id, SurfaceRole::XdgToplevel)
        .expect("toplevel role");
    let window_id = WindowId::from_raw(17).expect("window id");
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
        state
            .scene_registry
            .metadata(decoration)
            .unwrap()
            .visual_parent,
        Some(group)
    );

    state.ensure_window_scene_nodes(window_id, surface_id);
    assert_eq!(
        state
            .scene_registry
            .node_for_source(SceneSource::WindowGroup(window_id)),
        Some(group)
    );
    assert_eq!(
        state
            .scene_registry
            .node_for_source(SceneSource::ServerDecoration(window_id)),
        Some(decoration)
    );

    state
        .remove_desktop_window(window_id)
        .expect("remove window");
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
    let mut display = wayland_server::Display::<CompositorState>::new().expect("test display");
    let client = test_client(&mut display);
    let surface_a = test_surface_for_client(&mut state, &mut display, &client);
    let surface_b = test_surface_for_client(&mut state, &mut display, &client);
    state
        .assign_surface_role(surface_a, SurfaceRole::Xwayland)
        .expect("first xwayland role");
    state
        .assign_surface_role(surface_b, SurfaceRole::Xwayland)
        .expect("replacement xwayland role");
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let snapshot = test_x11_snapshot(generation, surface_a);
    let handle = snapshot.handle;
    let window_id = WindowId::from_raw(18).expect("window id");
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
    drop(display);
}
