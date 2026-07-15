use super::*;

#[test]
fn xdg_role_after_subsurface_is_rejected_and_healthy_client_survives() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection_a = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals_a, _queue_a) = registry_queue_init::<RegistryTestState>(&connection_a).unwrap();
    let qh_a = _queue_a.handle();
    let compositor_a: client_wl_compositor::WlCompositor =
        globals_a.bind(&qh_a, 1..=6, ()).unwrap();
    let subcompositor_a: client_wl_subcompositor::WlSubcompositor =
        globals_a.bind(&qh_a, 1..=1, ()).unwrap();
    let wm_base_a: client_xdg_wm_base::XdgWmBase = globals_a.bind(&qh_a, 1..=6, ()).unwrap();
    let parent = compositor_a.create_surface(&qh_a, ());
    let child = compositor_a.create_surface(&qh_a, ());
    let subsurface = subcompositor_a.get_subsurface(&child, &parent, &qh_a, ());

    let connection_b = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals_b, _queue_b) = registry_queue_init::<RegistryTestState>(&connection_b).unwrap();
    let qh_b = _queue_b.handle();
    let compositor_b: client_wl_compositor::WlCompositor =
        globals_b.bind(&qh_b, 1..=6, ()).unwrap();
    let _surface_b = compositor_b.create_surface(&qh_b, ());
    connection_b.roundtrip().unwrap();

    let _xdg_surface = wm_base_a.get_xdg_surface(&child, &qh_a, ());
    connection_a.flush().unwrap();
    wait_for_server_commands(&commands);
    let observed = expect_protocol_error(
        &connection_a,
        "xdg_wm_base",
        client_xdg_wm_base::Error::Role as u32,
    );
    assert_eq!(observed.object_id, wm_base_a.id().protocol_id());
    expect_roundtrip_alive(&connection_b);

    drop(subsurface);
    drop(child);
    drop(parent);
    drop(wm_base_a);
    drop(subcompositor_a);
    drop(compositor_a);
    drop(globals_a);
    drop(_queue_a);
    drop(qh_a);
    drop(connection_a);
    drop(compositor_b);
    drop(globals_b);
    drop(_queue_b);
    drop(qh_b);
    drop(connection_b);
    wait_for_server_commands(&commands);
    let server = stop_controllable_test_server(commands, server_thread);
    assert_eq!(server.state.compliance_metrics.protocol_errors_total, 1);
    assert_eq!(
        server.state.compliance_metrics.client_state_leaks_detected,
        0
    );
}

#[test]
fn xdg_role_after_cursor_surface_is_rejected_and_healthy_client_survives() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection_a = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals_a, mut queue_a) = registry_queue_init::<RegistryTestState>(&connection_a).unwrap();
    let qh_a = queue_a.handle();
    let compositor_a: client_wl_compositor::WlCompositor =
        globals_a.bind(&qh_a, 1..=6, ()).unwrap();
    let wm_base_a: client_xdg_wm_base::XdgWmBase = globals_a.bind(&qh_a, 1..=6, ()).unwrap();
    let seat_a: client_wl_seat::WlSeat = globals_a.bind(&qh_a, 1..=7, ()).unwrap();
    let pointer_a = seat_a.get_pointer(&qh_a, ());
    let origin = compositor_a.create_surface(&qh_a, ());
    let cursor_surface = compositor_a.create_surface(&qh_a, ());
    let origin_xdg = wm_base_a.get_xdg_surface(&origin, &qh_a, ());
    let origin_toplevel = origin_xdg.get_toplevel(&qh_a, ());
    origin.commit();
    connection_a.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue_a.roundtrip(&mut state).unwrap();
    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 20.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue_a.roundtrip(&mut state).unwrap();
    let enter_serial = state
        .pointer_enter_serial
        .expect("cursor surface needs pointer focus");
    pointer_a.set_cursor(enter_serial, Some(&cursor_surface), 0, 0);
    connection_a.flush().unwrap();
    wait_for_server_commands(&commands);

    let connection_b = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals_b, _queue_b) = registry_queue_init::<RegistryTestState>(&connection_b).unwrap();
    let qh_b = _queue_b.handle();
    let compositor_b: client_wl_compositor::WlCompositor =
        globals_b.bind(&qh_b, 1..=6, ()).unwrap();
    let _surface_b = compositor_b.create_surface(&qh_b, ());
    connection_b.roundtrip().unwrap();

    let _xdg_surface = wm_base_a.get_xdg_surface(&cursor_surface, &qh_a, ());
    connection_a.flush().unwrap();
    wait_for_server_commands(&commands);
    let observed = expect_protocol_error(
        &connection_a,
        "xdg_wm_base",
        client_xdg_wm_base::Error::Role as u32,
    );
    assert_eq!(observed.object_id, wm_base_a.id().protocol_id());
    expect_roundtrip_alive(&connection_b);

    drop(origin_toplevel);
    drop(origin_xdg);
    drop(pointer_a);
    drop(seat_a);
    drop(cursor_surface);
    drop(origin);
    drop(wm_base_a);
    drop(compositor_a);
    drop(globals_a);
    drop(queue_a);
    drop(qh_a);
    drop(connection_a);
    drop(compositor_b);
    drop(globals_b);
    drop(_queue_b);
    drop(qh_b);
    drop(connection_b);
    wait_for_server_commands(&commands);
    let server = stop_controllable_test_server(commands, server_thread);
    assert_eq!(server.state.compliance_metrics.protocol_errors_total, 1);
    assert_eq!(
        server.state.compliance_metrics.client_state_leaks_detected,
        0
    );
}

#[test]
fn invalid_scale_is_a_wire_error_and_does_not_disconnect_another_client() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection_a = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals_a, _queue_a) = registry_queue_init::<RegistryTestState>(&connection_a).unwrap();
    let qh_a = _queue_a.handle();
    let compositor_a: client_wl_compositor::WlCompositor =
        globals_a.bind(&qh_a, 1..=6, ()).unwrap();
    let surface_a = compositor_a.create_surface(&qh_a, ());
    let surface_a_id = surface_a.id().protocol_id();
    surface_a.set_buffer_scale(0);
    connection_a.flush().unwrap();

    let connection_b = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals_b, _queue_b) = registry_queue_init::<RegistryTestState>(&connection_b).unwrap();
    let qh_b = _queue_b.handle();
    let compositor_b: client_wl_compositor::WlCompositor =
        globals_b.bind(&qh_b, 1..=6, ()).unwrap();
    let surface_b = compositor_b.create_surface(&qh_b, ());
    let surface_b_id = surface_b.id().protocol_id();
    connection_b.roundtrip().unwrap();

    let observed = expect_protocol_error(
        &connection_a,
        "wl_surface",
        client_wl_surface::Error::InvalidScale as u32,
    );
    assert_eq!(observed.object_id, surface_a_id);
    drop(surface_a);
    drop(compositor_a);
    drop(globals_a);
    drop(_queue_a);
    drop(qh_a);
    drop(connection_a);
    expect_roundtrip_alive(&connection_b);
    wait_for_server_commands(&commands);
    wait_for_server_commands(&commands);
    let remaining_surface_count = capture_surface_resource_count(&commands);

    let server = stop_controllable_test_server(commands, server_thread);
    assert_eq!(
        surface_a_id, surface_b_id,
        "Wayland object ids are client-local"
    );
    assert_eq!(
        remaining_surface_count, 1,
        "client A cleanup must not remove client B's internal surface"
    );
    assert_eq!(server.state.surface_client_ids.len(), 1);
    assert_eq!(server.state.compliance_metrics.protocol_errors_total, 1);
}

#[test]
fn invalid_subsurface_sibling_is_a_wire_error_and_does_not_disconnect_another_client() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection_a = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals_a, _queue_a) = registry_queue_init::<RegistryTestState>(&connection_a).unwrap();
    let qh_a = _queue_a.handle();
    let compositor_a: client_wl_compositor::WlCompositor =
        globals_a.bind(&qh_a, 1..=6, ()).unwrap();
    let subcompositor_a: client_wl_subcompositor::WlSubcompositor =
        globals_a.bind(&qh_a, 1..=1, ()).unwrap();
    let parent = compositor_a.create_surface(&qh_a, ());
    let child = compositor_a.create_surface(&qh_a, ());
    let unrelated = compositor_a.create_surface(&qh_a, ());
    let subsurface = subcompositor_a.get_subsurface(&child, &parent, &qh_a, ());
    subsurface.place_above(&unrelated);
    connection_a.flush().unwrap();

    let connection_b = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals_b, _queue_b) = registry_queue_init::<RegistryTestState>(&connection_b).unwrap();
    let qh_b = _queue_b.handle();
    let compositor_b: client_wl_compositor::WlCompositor =
        globals_b.bind(&qh_b, 1..=6, ()).unwrap();
    let _surface_b = compositor_b.create_surface(&qh_b, ());
    connection_b.roundtrip().unwrap();

    let observed = expect_protocol_error(
        &connection_a,
        "wl_subsurface",
        client_wl_subsurface::Error::BadSurface as u32,
    );
    let expected_object_id = subsurface.id().protocol_id();
    assert_eq!(observed.object_id, expected_object_id);
    drop(subsurface);
    drop(unrelated);
    drop(child);
    drop(parent);
    drop(subcompositor_a);
    drop(compositor_a);
    drop(globals_a);
    drop(_queue_a);
    drop(qh_a);
    drop(connection_a);
    expect_roundtrip_alive(&connection_b);
    wait_for_server_commands(&commands);
    wait_for_server_commands(&commands);
    let remaining_surface_count = capture_surface_resource_count(&commands);

    let server = stop_controllable_test_server(commands, server_thread);
    assert_eq!(remaining_surface_count, 1);
    assert_eq!(server.state.surface_resources.len(), 1);
}

#[test]
fn invalid_shm_pool_size_is_a_wire_error_and_does_not_disconnect_another_client() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection_a = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals_a, _queue_a) = registry_queue_init::<RegistryTestState>(&connection_a).unwrap();
    let qh_a = _queue_a.handle();
    let compositor_a: client_wl_compositor::WlCompositor =
        globals_a.bind(&qh_a, 1..=6, ()).unwrap();
    let _surface_a = compositor_a.create_surface(&qh_a, ());
    let shm_a: client_wl_shm::WlShm = globals_a.bind(&qh_a, 1..=2, ()).unwrap();
    let shm_object_id = shm_a.id().protocol_id();
    let file = create_test_shm_file(&[]).unwrap();
    let _pool = shm_a.create_pool(file.as_fd(), 0, &qh_a, ());
    connection_a.flush().unwrap();

    let connection_b = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals_b, _queue_b) = registry_queue_init::<RegistryTestState>(&connection_b).unwrap();
    let qh_b = _queue_b.handle();
    let compositor_b: client_wl_compositor::WlCompositor =
        globals_b.bind(&qh_b, 1..=6, ()).unwrap();
    let _surface_b = compositor_b.create_surface(&qh_b, ());
    connection_b.roundtrip().unwrap();

    let observed = expect_protocol_error(
        &connection_a,
        "wl_shm",
        client_wl_shm::Error::InvalidStride as u32,
    );
    assert_eq!(observed.object_id, shm_object_id);
    drop(_pool);
    drop(shm_a);
    drop(compositor_a);
    drop(globals_a);
    drop(_queue_a);
    drop(qh_a);
    drop(connection_a);
    expect_roundtrip_alive(&connection_b);
    wait_for_server_commands(&commands);
    wait_for_server_commands(&commands);
    let remaining_surface_count = capture_surface_resource_count(&commands);

    let server = stop_controllable_test_server(commands, server_thread);
    assert_eq!(remaining_surface_count, 1);
    assert_eq!(server.state.surface_resources.len(), 1);
}

#[test]
fn get_touch_without_advertised_touch_capability_is_a_wire_error() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection_a = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals_a, _queue_a) = registry_queue_init::<RegistryTestState>(&connection_a).unwrap();
    let qh_a = _queue_a.handle();
    let seat_a: client_wl_seat::WlSeat = globals_a.bind(&qh_a, 1..=7, ()).unwrap();
    let seat_id = seat_a.id().protocol_id();
    seat_a.get_touch(&qh_a, ());
    connection_a.flush().unwrap();

    let connection_b = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals_b, _queue_b) = registry_queue_init::<RegistryTestState>(&connection_b).unwrap();
    let qh_b = _queue_b.handle();
    let seat_b: client_wl_seat::WlSeat = globals_b.bind(&qh_b, 1..=7, ()).unwrap();
    connection_b.roundtrip().unwrap();

    let observed = expect_protocol_error(
        &connection_a,
        "wl_seat",
        client_wl_seat::Error::MissingCapability as u32,
    );
    assert_eq!(observed.object_id, seat_id);
    expect_roundtrip_alive(&connection_b);
    assert_eq!(seat_b.version(), 7);

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn invalid_data_source_action_mask_is_a_wire_error() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, _queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = _queue.handle();
    let manager: client_wl_data_device_manager::WlDataDeviceManager =
        globals.bind(&qh, 1..=3, ()).unwrap();
    let source = manager.create_data_source(&qh, ());
    let source_id = source.id().protocol_id();
    let message = wayland_backend::protocol::Message {
        sender_id: source.id(),
        opcode: 2,
        args: wayland_backend::smallvec::smallvec![wayland_backend::protocol::Argument::Uint(8)],
    };
    connection
        .backend()
        .send_request(message, None, None)
        .unwrap();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);

    let observed = expect_protocol_error(
        &connection,
        "wl_data_source",
        client_wl_data_source::Error::InvalidActionMask as u32,
    );
    assert_eq!(observed.object_id, source_id);

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn xdg_buffer_before_initial_configure_is_a_wire_error() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, _queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = _queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=2, ()).unwrap();
    let file = create_test_shm_file(&[0xff20_3040]).unwrap();
    let pool = shm.create_pool(file.as_fd(), 4, &qh, ());
    let buffer = pool.create_buffer(0, 1, 1, 4, client_wl_shm::Format::Argb8888, &qh, ());
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let xdg_surface_id = xdg_surface.id().protocol_id();
    surface.attach(Some(&buffer), 0, 0);
    surface.commit();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);

    let observed = expect_protocol_error(
        &connection,
        "xdg_surface",
        client_xdg_surface::Error::UnconfiguredBuffer as u32,
    );
    assert_eq!(observed.object_id, xdg_surface_id);

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn unknown_xdg_configure_ack_is_a_wire_error() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, _queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = _queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let xdg_surface_id = xdg_surface.id().protocol_id();
    surface.commit();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    xdg_surface.ack_configure(0xffff_fffe);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);

    let observed = expect_protocol_error(
        &connection,
        "xdg_surface",
        client_xdg_surface::Error::InvalidSerial as u32,
    );
    assert_eq!(observed.object_id, xdg_surface_id);

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn wm_base_destroy_with_live_xdg_surfaces_posts_defunct_surfaces() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, _queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = _queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base_id = wm_base.id().protocol_id();
    let surface = compositor.create_surface(&qh, ());
    let _xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    connection.roundtrip().unwrap();

    wm_base.destroy();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);

    let observed = expect_protocol_error(
        &connection,
        "xdg_wm_base",
        client_xdg_wm_base::Error::DefunctSurfaces as u32,
    );
    assert_eq!(observed.object_id, wm_base_id);

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn role_switch_after_role_object_destroy_is_rejected() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 1..=4, ()).unwrap();
    let layer_shell_id = layer_shell.id().protocol_id();

    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();

    toplevel.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let _layer_surface = layer_shell.get_layer_surface(
        &surface,
        None,
        client_zwlr_layer_shell_v1::Layer::Top,
        "role-switch".to_string(),
        &qh,
        (),
    );
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let observed = expect_protocol_error(
        &connection,
        "zwlr_layer_shell_v1",
        wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Error::Role as u32,
    );
    assert_eq!(observed.object_id, layer_shell_id);

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn surface_destroy_with_live_role_posts_defunct_role_object() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, _queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = _queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let surface_id = surface.id().protocol_id();
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush().unwrap();

    surface.destroy();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let observed = expect_protocol_error(
        &connection,
        "wl_surface",
        client_wl_surface::Error::DefunctRoleObject as u32,
    );
    assert_eq!(observed.object_id, surface_id);

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn xdg_surface_destroy_with_live_role_posts_defunct_role_object() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, _queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = _queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let xdg_surface_id = xdg_surface.id().protocol_id();
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush().unwrap();

    xdg_surface.destroy();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let observed = expect_protocol_error(
        &connection,
        "xdg_surface",
        client_xdg_surface::Error::DefunctRoleObject as u32,
    );
    assert_eq!(observed.object_id, xdg_surface_id);

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn attach_nonzero_offset_v4_preserves_legacy_semantics() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, _queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = _queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=4, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    assert_eq!(surface.version(), 4);
    surface.attach(None, 12, -7);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    expect_roundtrip_alive(&connection);

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn attach_nonzero_offset_v5_posts_invalid_offset() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, _queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = _queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=5, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    assert_eq!(surface.version(), 5);
    surface.attach(None, 12, -7);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let observed = expect_protocol_error(
        &connection,
        "wl_surface",
        client_wl_surface::Error::InvalidOffset as u32,
    );
    assert_eq!(observed.object_id, surface.id().protocol_id());

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn invalid_transform_posts_invalid_transform() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, _queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = _queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let message = wayland_backend::protocol::Message {
        sender_id: surface.id(),
        opcode: 7,
        args: wayland_backend::smallvec::smallvec![wayland_backend::protocol::Argument::Int(99)],
    };
    connection
        .backend()
        .send_request(message, None, None)
        .unwrap();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let observed = expect_protocol_error(
        &connection,
        "wl_surface",
        client_wl_surface::Error::InvalidTransform as u32,
    );
    assert_eq!(observed.object_id, surface.id().protocol_id());

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn transformed_scaled_nonintegral_buffer_posts_invalid_size() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, _queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = _queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=2, ()).unwrap();
    let file = create_test_shm_file(&[0xffff_ffff; 3]).unwrap();
    let pool = shm.create_pool(file.as_fd(), 12, &qh, ());
    let buffer = pool.create_buffer(0, 3, 1, 12, client_wl_shm::Format::Argb8888, &qh, ());
    let surface = compositor.create_surface(&qh, ());
    surface.set_buffer_scale(2);
    surface.attach(Some(&buffer), 0, 0);
    surface.commit();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let observed = expect_protocol_error(
        &connection,
        "wl_surface",
        client_wl_surface::Error::InvalidSize as u32,
    );
    assert_eq!(observed.object_id, surface.id().protocol_id());

    let _server = stop_controllable_test_server(commands, server_thread);
}
