use super::*;

#[test]
fn wayland_client_can_create_subsurface_on_oblivion_server() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let result = create_client_subsurface(&socket_path);
    stop_test_server(running, server_thread);

    result.unwrap();
}

#[test]
fn wayland_client_subsurface_commit_tracks_parent_relative_position() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let result = create_client_toplevel_with_positioned_subsurface_buffer(&socket_path);
    let server = stop_test_server(running, server_thread);

    result.unwrap();
    let child = server
        .renderable_surfaces()
        .iter()
        .find(|surface| surface.width == 1 && surface.height == 1)
        .expect("child subsurface snapshot should be renderable");
    let parent = server
        .renderable_surfaces()
        .iter()
        .find(|surface| surface.width == 2 && surface.height == 2)
        .expect("parent toplevel snapshot should be renderable");

    assert_eq!(
        child.placement,
        SurfacePlacement::subsurface(parent.surface_id, 10, 12)
    );
}

#[test]
fn subsurface_committed_before_parent_stays_above_parent_when_parent_maps() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    create_subsurface_buffer_before_parent_buffer(&socket_path).unwrap();
    let server = stop_test_server(running, server_thread);

    let parent_index = server
        .renderable_surfaces()
        .iter()
        .position(|surface| surface.width == 2 && surface.height == 2)
        .expect("parent surface should be renderable");
    let child_index = server
        .renderable_surfaces()
        .iter()
        .position(|surface| surface.width == 1 && surface.height == 1)
        .expect("subsurface should be renderable");

    assert!(child_index > parent_index);
}

#[test]
fn wayland_surface_attach_null_unmaps_renderable_surface() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    create_toplevel_then_attach_null_buffer(&socket_path).unwrap();
    let server = stop_test_server(running, server_thread);

    assert!(server.renderable_surfaces().is_empty());
}

#[test]
fn wayland_surface_attach_null_unmaps_nested_subsurface_tree() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    create_toplevel_with_nested_subsurfaces_then_attach_null_buffer(&socket_path).unwrap();
    let server = stop_test_server(running, server_thread);

    assert!(server.renderable_surfaces().is_empty());
}
