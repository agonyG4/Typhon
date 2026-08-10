use super::*;

fn initial_offset(x: i32, y: i32) -> (i32, i32) {
    (
        render::FIRST_SURFACE_OFFSET.0 + x,
        render::FIRST_SURFACE_OFFSET.1 + y,
    )
}

fn initial_root_placement(x: i32, y: i32) -> SurfacePlacement {
    let (x, y) = initial_offset(x, y);
    SurfacePlacement::absolute_root_at(x, y)
}

#[test]
fn xdg_toplevel_move_request_accepts_serial_from_same_client_chrome_surface() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state =
        create_toplevel_request_move_from_client_chrome_surface(&socket_path, &commands).unwrap();
    let server = stop_controllable_test_server(commands, server_thread);
    let origins = render::surface_origins(server.renderable_surfaces());
    let toplevel_index = server
        .renderable_surfaces()
        .iter()
        .position(|surface| surface.width == 100 && surface.height == 80)
        .expect("toplevel should remain renderable");
    let toplevel_id = server.renderable_surfaces()[toplevel_index].surface_id;

    assert_eq!(state.pointer_surface_x, Some(12.0));
    assert_eq!(state.pointer_surface_y, Some(14.0));
    assert_eq!(
        server.state.surface_placement(toplevel_id),
        initial_root_placement(80, 60)
    );
    assert_eq!(origins[toplevel_index], initial_offset(80, 60));
}
