use super::*;

#[test]
fn unknown_restore_before_first_buffer_uses_implicit_committed_geometry() {
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

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);
    let root_surface_id = capture_sole_toplevel_root_surface_id(&commands)
        .expect("unmapped toplevel root is registered");

    state.suppress_xdg_surface_ack = true;
    state.suppress_xdg_surface_commit = true;
    assert!(start_unknown_restore_before_first_buffer(
        &commands,
        root_surface_id
    ));
    queue.roundtrip(&mut state).unwrap();
    assert_eq!((state.toplevel_width, state.toplevel_height), (0, 0));
    let configure_serial = *state
        .surface_configure_serials
        .last()
        .expect("unspecified normal restore configure");
    xdg_surface.ack_configure(configure_serial);
    commit_test_buffered_surface(&surface, &shm, &qh, 800, 600).unwrap();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    assert!(!capture_pending_normal_restore(&commands, root_surface_id));
    assert_eq!(capture_committed_window_geometry(&commands), None);
    let restored = capture_root_window_geometry(&commands, root_surface_id)
        .expect("implicit response geometry finalizes the restore");
    assert_eq!((restored.width, restored.height), (800, 600));

    stop_controllable_test_server(commands, server_thread);
}

#[test]
fn unknown_restore_mapping_only_response_uses_new_implicit_logical_size() {
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
    let viewporter: client_wp_viewporter::WpViewporter = globals.bind(&qh, 1..=1, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let viewport = viewporter.get_viewport(&surface, &qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush().unwrap();

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    viewport.set_destination(80, 60);
    commit_test_buffered_surface(&surface, &shm, &qh, 80, 60).unwrap();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    let root_surface_id = capture_focused_surface_id(&commands)
        .expect("mapped implicit-geometry toplevel is focused");
    state.suppress_xdg_surface_ack = true;
    state.suppress_xdg_surface_commit = true;
    assert!(begin_unknown_normal_restore(&commands, root_surface_id));
    queue.roundtrip(&mut state).unwrap();
    assert_eq!((state.toplevel_width, state.toplevel_height), (0, 0));
    let configure_serial = *state
        .surface_configure_serials
        .last()
        .expect("unspecified normal restore configure");
    xdg_surface.ack_configure(configure_serial);

    // The response is a mapping-only root commit: no new buffer and no
    // set_window_geometry request. Its viewport destination changes the
    // committed logical extent from 80x60 to 800x600.
    viewport.set_destination(800, 600);
    surface.commit();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    assert!(!capture_pending_normal_restore(&commands, root_surface_id));
    assert_eq!(capture_committed_window_geometry(&commands), None);
    let restored = capture_root_window_geometry(&commands, root_surface_id)
        .expect("mapping-only response uses its committed viewport mapping");
    assert_eq!((restored.width, restored.height), (800, 600));

    stop_controllable_test_server(commands, server_thread);
}

#[test]
fn synchronized_subsurface_response_publishes_restore_from_complete_committed_tree() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let subcompositor: client_wl_subcompositor::WlSubcompositor =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush().unwrap();

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    let child = compositor.create_surface(&qh, ());
    let subsurface = subcompositor.get_subsurface(&child, &surface, &qh, ());
    subsurface.set_position(350, -20);
    let child_initial = TestShmBuffer::new(&shm, &qh, 200, 150).unwrap();
    child_initial.attach(&child, 200, 150);
    child.commit();
    let root_initial = TestShmBuffer::new(&shm, &qh, 400, 300).unwrap();
    root_initial.attach(&surface, 400, 300);
    surface.commit();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    let root_surface_id = capture_focused_surface_id(&commands)
        .expect("mapped XDG root with synchronized child is focused");
    state.suppress_xdg_surface_ack = true;
    state.suppress_xdg_surface_commit = true;
    assert!(begin_unknown_normal_restore(&commands, root_surface_id));
    queue.roundtrip(&mut state).unwrap();
    let configure_serial = *state
        .surface_configure_serials
        .last()
        .expect("unspecified normal restore configure");
    xdg_surface.ack_configure(configure_serial);

    // Synchronized child changes are latched first; the root response commit
    // publishes the child position, child size, and root size as one tree.
    subsurface.set_position(400, -20);
    let child_response = TestShmBuffer::new(&shm, &qh, 250, 150).unwrap();
    child_response.attach(&child, 250, 150);
    child.commit();
    let root_response = TestShmBuffer::new(&shm, &qh, 500, 400).unwrap();
    root_response.attach(&surface, 500, 400);
    surface.commit();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    assert!(!capture_pending_normal_restore(&commands, root_surface_id));
    assert_eq!(capture_committed_window_geometry(&commands), None);
    let restored = capture_root_window_geometry(&commands, root_surface_id)
        .expect("complete committed surface tree finalizes the restore");
    assert_eq!((restored.width, restored.height), (650, 420));

    stop_controllable_test_server(commands, server_thread);
}

#[test]
fn explicit_window_geometry_persists_across_unknown_restore_response_commit() {
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

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    xdg_surface.set_window_geometry(10, 10, 520, 410);
    commit_test_buffered_surface(&surface, &shm, &qh, 80, 60).unwrap();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    let root_surface_id =
        capture_focused_surface_id(&commands).expect("explicit-geometry toplevel is mapped");
    state.suppress_xdg_surface_ack = true;
    state.suppress_xdg_surface_commit = true;
    assert!(begin_unknown_normal_restore(&commands, root_surface_id));
    queue.roundtrip(&mut state).unwrap();
    let configure_serial = *state
        .surface_configure_serials
        .last()
        .expect("unspecified normal restore configure");
    xdg_surface.ack_configure(configure_serial);
    // No repeated set_window_geometry request is sent on this commit.
    commit_test_buffered_surface(&surface, &shm, &qh, 96, 72).unwrap();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    assert!(!capture_pending_normal_restore(&commands, root_surface_id));
    assert_eq!(
        capture_committed_window_geometry(&commands),
        Some(XdgWindowGeometry::new(10, 10, 520, 410))
    );
    let restored = capture_root_window_geometry(&commands, root_surface_id)
        .expect("persistent explicit geometry finalizes the restore");
    assert_eq!((restored.width, restored.height), (520, 410));

    stop_controllable_test_server(commands, server_thread);
}
