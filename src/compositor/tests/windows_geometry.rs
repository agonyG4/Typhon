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
fn first_renderable_uses_persistent_committed_geometry_after_geometry_only_commit() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());

    xdg_surface.set_window_geometry(16, 10, 300, 200);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    wait_for_server_commands(&commands);

    assert_eq!(
        capture_committed_window_geometry(&commands),
        Some(XdgWindowGeometry::new(16, 10, 300, 200))
    );
    assert!(capture_renderable_surface_snapshot(&commands).is_empty());

    commit_test_buffered_surface(&surface, &shm, &qh, 300, 200).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    wait_for_server_commands(&commands);
    let surfaces = capture_renderable_surface_snapshot(&commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let root = surfaces
        .iter()
        .find(|surface| surface.parent_surface_id.is_none())
        .expect("first root renderable should be published");
    assert_eq!((root.origin_x, root.origin_y), initial_offset(-16, -10));
}

#[test]
fn real_resize_lifecycle_preserves_frame_to_content_offset() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let snapshots =
        capture_csd_consecutive_resize_regression_snapshots(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    for snapshot in [
        &snapshots.first_final,
        &snapshots.second_preview,
        &snapshots.second_final,
        &snapshots.third_preview,
    ] {
        let visual = snapshot.visual.expect("resize lifecycle visual geometry");
        let window_geometry = snapshot
            .window_geometry
            .expect("resize lifecycle committed geometry");
        let root = snapshot
            .surfaces
            .iter()
            .find(|surface| surface.parent_surface_id.is_none())
            .expect("resize lifecycle root renderable");
        assert_eq!(
            (root.origin_x, root.origin_y),
            (
                visual.local_x - window_geometry.x,
                visual.local_y - window_geometry.y,
            )
        );
    }
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
