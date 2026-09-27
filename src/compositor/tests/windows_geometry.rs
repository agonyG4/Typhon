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

    // The request is committed, but remains compatibility state until a
    // positive committed tree exists; its raw dimensions are not effective.
    assert_eq!(capture_committed_window_geometry(&commands), None);
    assert!(capture_renderable_surface_snapshot(&commands).is_empty());

    commit_test_buffered_surface(&surface, &shm, &qh, 300, 200).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    wait_for_server_commands(&commands);
    assert_eq!(
        capture_committed_window_geometry(&commands),
        Some(XdgWindowGeometry::new(16, 10, 284, 190))
    );
    commands
        .send(ServerCommand::CancelFocusedPresentationTransition)
        .unwrap();
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
fn same_commit_buffer_and_geometry_clamp_against_new_content() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());

    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();

    xdg_surface.set_window_geometry(10, 10, 1_000, 800);
    commit_test_buffered_surface(&surface, &shm, &qh, 600, 500).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    wait_for_server_commands(&commands);

    assert_eq!(
        capture_committed_window_geometry(&commands),
        Some(XdgWindowGeometry::new(10, 10, 590, 490)),
        "the request must clamp against the 600x500 buffer committed alongside it"
    );

    stop_controllable_test_server(commands, server_thread);
}

#[test]
fn mapping_only_commit_clamps_against_its_new_logical_size() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let viewporter: client_wp_viewporter::WpViewporter = globals.bind(&qh, 1..=1, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let viewport = viewporter.get_viewport(&surface, &qh, ());
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    commit_test_buffered_surface(&surface, &shm, &qh, 300, 200).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();

    xdg_surface.set_window_geometry(10, 10, 1_000, 800);
    viewport.set_destination(600, 500);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    wait_for_server_commands(&commands);

    assert_eq!(
        capture_committed_window_geometry(&commands),
        Some(XdgWindowGeometry::new(10, 10, 590, 490))
    );
    let root = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .find(|surface| surface.parent_surface_id.is_none())
        .expect("root renderable");
    assert_eq!((root.width, root.height), (600, 500));

    stop_controllable_test_server(commands, server_thread);
}

#[test]
fn real_wayland_geometry_before_first_buffer_waits_for_bounds_and_keeps_latest_request() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());

    surface.commit();
    connection.flush().unwrap();
    let mut client_state = RegistryTestState::default();
    queue.roundtrip(&mut client_state).unwrap();

    xdg_surface.set_window_geometry(1, 1, 40, 30);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut client_state).unwrap();
    assert_eq!(capture_effective_xdg_window_geometry(&commands, 1), None);

    xdg_surface.set_window_geometry(10, 10, 520, 410);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut client_state).unwrap();
    assert_eq!(capture_effective_xdg_window_geometry(&commands, 1), None);

    commit_test_buffered_surface(&surface, &shm, &qh, 80, 60).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut client_state).unwrap();
    wait_for_server_commands(&commands);

    assert_eq!(
        capture_effective_xdg_window_geometry(&commands, 1),
        Some(XdgWindowGeometry::new(10, 10, 70, 50))
    );
    stop_controllable_test_server(commands, server_thread);
}

#[test]
fn never_explicit_root_geometry_includes_a_real_negative_position_subsurface() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let subcompositor: client_wl_subcompositor::WlSubcompositor =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let root = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&root, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let child = compositor.create_surface(&qh, ());
    let subsurface = subcompositor.get_subsurface(&child, &root, &qh, ());
    subsurface.set_position(-20, -10);

    root.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    commit_test_buffered_surface(&child, &shm, &qh, 200, 100).unwrap();
    commit_test_buffered_surface(&root, &shm, &qh, 400, 300).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    wait_for_server_commands(&commands);

    let root_id = capture_focused_surface_id(&commands).expect("mapped XDG root");
    let authority =
        capture_xdg_root_placement_authority(&commands, root_id).expect("root placement authority");
    let frame = authority
        .logical_window_geometry
        .expect("complete implicit frame geometry");
    assert_eq!((frame.width, frame.height), (420, 310));
    assert_eq!(authority.committed_window_geometry, None);
    assert_eq!(
        authority.resolved_render_origin,
        (frame.placement.local_x + 20, frame.placement.local_y + 10)
    );

    let surface_snapshots = capture_renderable_surface_snapshot(&commands);
    let child_snapshot = surface_snapshots
        .into_iter()
        .find(|surface| surface.parent_surface_id.is_some())
        .expect("real subsurface renderable");
    assert_eq!(
        (child_snapshot.origin_x, child_snapshot.origin_y),
        (frame.placement.local_x, frame.placement.local_y),
        "the negative child origin aligns with the canonical frame top-left"
    );

    stop_controllable_test_server(commands, server_thread);
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

    let (state, interaction) =
        create_toplevel_request_move_from_client_chrome_surface(&socket_path, &commands, false)
            .unwrap();
    let server = stop_controllable_test_server(commands, server_thread);
    let surfaces = server.renderable_surfaces();
    let origins = render::surface_origins(surfaces);
    let toplevel_index = surfaces
        .iter()
        .position(|surface| surface.width == 100 && surface.height == 80)
        .expect("toplevel should remain renderable");
    let toplevel_id = surfaces[toplevel_index].surface_id;
    let chrome_id = surfaces
        .iter()
        .find(|surface| surface.width == 120 && surface.height == 20)
        .expect("CSD surface should remain renderable")
        .surface_id;

    assert_eq!(state.pointer_surface_x, Some(12.0));
    assert_eq!(state.pointer_surface_y, Some(14.0));
    let interaction = interaction.expect("the CSD-owned press must authorize move");
    assert_eq!(interaction.root_surface_id, toplevel_id);
    assert_eq!(interaction.pointer_motion_surface_id, Some(chrome_id));
    assert!(matches!(interaction.kind, WindowInteractionKind::Move));
    assert_eq!(
        server.state.surface_placement(toplevel_id),
        initial_root_placement(80, 60)
    );
    assert_eq!(origins[toplevel_index], initial_offset(80, 60));
}

#[test]
fn xdg_toplevel_resize_request_accepts_serial_from_same_client_chrome_surface() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let (state, interaction) =
        create_toplevel_request_move_from_client_chrome_surface(&socket_path, &commands, true)
            .unwrap();
    let server = stop_controllable_test_server(commands, server_thread);
    let surfaces = server.renderable_surfaces();
    let toplevel_id = surfaces
        .iter()
        .find(|surface| surface.width == 100 && surface.height == 80)
        .expect("toplevel should remain renderable")
        .surface_id;
    let chrome_id = surfaces
        .iter()
        .find(|surface| surface.width == 120 && surface.height == 20)
        .expect("CSD surface should remain renderable")
        .surface_id;
    let interaction = interaction.expect("the CSD-owned press must authorize resize");

    assert_eq!(state.pointer_surface_x, Some(12.0));
    assert_eq!(state.pointer_surface_y, Some(14.0));
    assert_eq!(interaction.root_surface_id, toplevel_id);
    assert_eq!(interaction.pointer_motion_surface_id, Some(chrome_id));
    assert!(matches!(interaction.kind, WindowInteractionKind::Resize(_)));
}
