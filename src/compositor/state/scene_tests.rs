use super::*;
use std::os::unix::net::UnixStream;
use std::sync::Arc;

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
