use super::*;

#[test]
fn nonintersecting_explicit_xdg_geometry_posts_invalid_size_after_positive_bounds_exist() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
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
    commit_test_buffered_surface(&surface, &shm, &qh, 80, 60).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();

    xdg_surface.set_window_geometry(100, 100, 20, 20);
    surface.commit();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let observed = expect_protocol_error(
        &connection,
        "xdg_surface",
        wayland_protocols::xdg::shell::client::xdg_surface::Error::InvalidSize as u32,
    );
    assert_eq!(observed.object_id, xdg_surface.id().protocol_id());

    let server = stop_controllable_test_server(commands, server_thread);
    assert_eq!(server.state.compliance_metrics.protocol_errors_total, 1);
}

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
fn viewport_source_only_fractional_width_is_a_bad_size_error() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let viewporter: client_wp_viewporter::WpViewporter = globals.bind(&qh, 1..=1, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let viewport = viewporter.get_viewport(&surface, &qh, ());
    let buffer = TestShmBuffer::new(&shm, &qh, 4, 4).unwrap();

    viewport.set_source(0.0, 0.0, 2.5, 2.0);
    buffer.attach(&surface, 4, 4);
    surface.commit();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let observed = expect_protocol_error(
        &connection,
        "wp_viewport",
        client_wp_viewport::Error::BadSize as u32,
    );
    assert_eq!(observed.object_id, viewport.id().protocol_id());

    drop(viewport);
    drop(surface);
    drop(shm);
    drop(compositor);
    drop(globals);
    drop(queue);
    drop(qh);
    drop(connection);
    let _ = commands.send(ServerCommand::Stop);
    let server = server_thread.join().unwrap();
    assert_eq!(server.state.compliance_metrics.protocol_errors_total, 1);
}

#[test]
fn viewport_source_only_fractional_width_without_buffer_is_a_bad_size_error() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let viewporter: client_wp_viewporter::WpViewporter = globals.bind(&qh, 1..=1, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let viewport = viewporter.get_viewport(&surface, &qh, ());

    viewport.set_source(0.0, 0.0, 2.5, 2.0);
    surface.commit();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let observed = expect_protocol_error(
        &connection,
        "wp_viewport",
        client_wp_viewport::Error::BadSize as u32,
    );
    assert_eq!(observed.object_id, viewport.id().protocol_id());

    drop(viewport);
    drop(surface);
    drop(compositor);
    drop(globals);
    drop(queue);
    drop(qh);
    drop(connection);
    let _ = commands.send(ServerCommand::Stop);
    let server = server_thread.join().unwrap();
    assert_eq!(server.state.compliance_metrics.protocol_errors_total, 1);
}

#[test]
fn viewport_source_only_fractional_height_without_buffer_is_a_bad_size_error() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let viewporter: client_wp_viewporter::WpViewporter = globals.bind(&qh, 1..=1, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let viewport = viewporter.get_viewport(&surface, &qh, ());

    viewport.set_source(0.0, 0.0, 2.0, 2.5);
    surface.commit();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let observed = expect_protocol_error(
        &connection,
        "wp_viewport",
        client_wp_viewport::Error::BadSize as u32,
    );
    assert_eq!(observed.object_id, viewport.id().protocol_id());

    drop(viewport);
    drop(surface);
    drop(compositor);
    drop(globals);
    drop(queue);
    drop(qh);
    drop(connection);
    let _ = commands.send(ServerCommand::Stop);
    let server = server_thread.join().unwrap();
    assert_eq!(server.state.compliance_metrics.protocol_errors_total, 1);
}

#[test]
fn viewport_source_only_fractional_width_with_explicit_null_buffer_is_a_bad_size_error() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let viewporter: client_wp_viewporter::WpViewporter = globals.bind(&qh, 1..=1, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let viewport = viewporter.get_viewport(&surface, &qh, ());
    let buffer = TestShmBuffer::new(&shm, &qh, 4, 4).unwrap();

    buffer.attach(&surface, 4, 4);
    surface.commit();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);

    viewport.set_source(0.0, 0.0, 2.5, 2.0);
    surface.attach(None, 0, 0);
    surface.commit();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let observed = expect_protocol_error(
        &connection,
        "wp_viewport",
        client_wp_viewport::Error::BadSize as u32,
    );
    assert_eq!(observed.object_id, viewport.id().protocol_id());

    drop(viewport);
    drop(surface);
    drop(buffer);
    drop(shm);
    drop(compositor);
    drop(globals);
    drop(queue);
    drop(qh);
    drop(connection);
    let _ = commands.send(ServerCommand::Stop);
    let server = server_thread.join().unwrap();
    assert_eq!(server.state.compliance_metrics.protocol_errors_total, 1);
}

#[test]
fn viewport_integral_source_outside_null_buffer_does_not_emit_out_of_buffer() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let viewporter: client_wp_viewporter::WpViewporter = globals.bind(&qh, 1..=1, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let viewport = viewporter.get_viewport(&surface, &qh, ());
    let buffer = TestShmBuffer::new(&shm, &qh, 4, 4).unwrap();

    buffer.attach(&surface, 4, 4);
    surface.commit();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);

    viewport.set_source(0.0, 0.0, 100.0, 100.0);
    surface.attach(None, 0, 0);
    surface.commit();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    expect_roundtrip_alive(&connection);

    drop(viewport);
    drop(surface);
    drop(buffer);
    drop(shm);
    drop(compositor);
    drop(globals);
    drop(queue);
    drop(qh);
    drop(connection);
    let _ = commands.send(ServerCommand::Stop);
    let server = server_thread.join().unwrap();
    assert_eq!(server.state.compliance_metrics.protocol_errors_total, 0);
}

#[test]
fn viewport_fractional_source_with_destination_without_buffer_is_valid() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let viewporter: client_wp_viewporter::WpViewporter = globals.bind(&qh, 1..=1, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let viewport = viewporter.get_viewport(&surface, &qh, ());

    viewport.set_source(0.0, 0.0, 2.5, 2.5);
    viewport.set_destination(4, 4);
    surface.commit();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    expect_roundtrip_alive(&connection);

    drop(viewport);
    drop(surface);
    drop(compositor);
    drop(globals);
    drop(queue);
    drop(qh);
    drop(connection);
    let _ = commands.send(ServerCommand::Stop);
    let server = server_thread.join().unwrap();
    assert_eq!(server.state.compliance_metrics.protocol_errors_total, 0);
}

fn run_delayed_synchronized_viewport_error(
    create_replacement: bool,
) -> (bool, Option<ProtocolErrorObservation>, Option<u32>) {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
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
    let viewporter: client_wp_viewporter::WpViewporter = globals.bind(&qh, 1..=1, ()).unwrap();
    let parent = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let child = compositor.create_surface(&qh, ());
    let _subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    let viewport = viewporter.get_viewport(&child, &qh, ());
    let buffer = TestShmBuffer::new(&shm, &qh, 4, 4).unwrap();

    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();

    // The integral source is valid without a buffer. It becomes invalid only
    // when the cached child state is applied to the 4x4 buffer below.
    viewport.set_source(0.0, 0.0, 100.0, 100.0);
    child.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();

    buffer.attach(&child, 4, 4);
    child.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();

    viewport.destroy();
    let replacement = create_replacement.then(|| viewporter.get_viewport(&child, &qh, ()));
    let replacement_id = replacement
        .as_ref()
        .map(|viewport| viewport.id().protocol_id());

    parent.commit();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let read_result = loop {
        if let Some(guard) = connection.prepare_read() {
            break guard.read();
        }
        if connection
            .backend()
            .dispatch_inner_queue()
            .expect("pending client events must be dispatchable")
            > 0
        {
            continue;
        }
    };
    let roundtrip_ok = read_result.is_ok();
    let observed = connection
        .protocol_error()
        .map(|error| ProtocolErrorObservation {
            code: error.code,
            object_id: error.object_id,
            object_interface: error.object_interface.clone(),
            message: error.message.clone(),
        });

    drop(replacement);
    drop(child);
    drop(parent);
    drop(buffer);
    drop(viewporter);
    drop(shm);
    drop(subcompositor);
    drop(_toplevel);
    drop(xdg_surface);
    drop(wm_base);
    drop(compositor);
    drop(globals);
    drop(queue);
    drop(qh);
    drop(connection);
    let _ = commands.send(ServerCommand::Stop);
    let server = server_thread.join().unwrap();
    assert_eq!(server.state.compliance_metrics.protocol_errors_total, 0);

    (roundtrip_ok, observed, replacement_id)
}

#[test]
fn delayed_synchronized_viewport_error_is_not_attributed_to_replacement() {
    let (roundtrip_ok, observed, replacement_id) = run_delayed_synchronized_viewport_error(true);

    if let (Some(observed), Some(replacement_id)) = (&observed, replacement_id) {
        assert_ne!(
            observed.object_id, replacement_id,
            "a cached V1 violation must not target replacement V2"
        );
    }
    assert!(
        roundtrip_ok,
        "destroyed V1 must not turn a delayed mapping error into an error on V2: {observed:?}"
    );
    assert!(observed.is_none());
}

#[test]
fn delayed_synchronized_viewport_error_without_replacement_is_not_surface_error() {
    let (roundtrip_ok, observed, replacement_id) = run_delayed_synchronized_viewport_error(false);

    assert!(replacement_id.is_none());
    assert!(
        roundtrip_ok,
        "destroyed V1 must not be replaced by wl_surface.invalid_size: {observed:?}"
    );
    assert!(observed.is_none());
}

#[test]
fn synchronized_child_out_of_buffer_is_rejected_when_cached_state_is_applied() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let subcompositor: client_wl_subcompositor::WlSubcompositor =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let viewporter: client_wp_viewporter::WpViewporter = globals.bind(&qh, 1..=1, ()).unwrap();
    let parent = compositor.create_surface(&qh, ());
    let child = compositor.create_surface(&qh, ());
    let _subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    let viewport = viewporter.get_viewport(&child, &qh, ());
    let buffer = TestShmBuffer::new(&shm, &qh, 4, 4).unwrap();

    // The integral source is admitted to the synchronized cache without a
    // concrete buffer. Bounds validation belongs at the parent application
    // boundary, while the owning viewport resource is still live.
    viewport.set_source(0.0, 0.0, 100.0, 100.0);
    buffer.attach(&child, 4, 4);
    child.commit();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    parent.commit();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let observed = expect_protocol_error(
        &connection,
        "wp_viewport",
        client_wp_viewport::Error::OutOfBuffer as u32,
    );
    assert_eq!(observed.object_id, viewport.id().protocol_id());

    drop(viewport);
    drop(child);
    drop(parent);
    drop(buffer);
    drop(shm);
    drop(viewporter);
    drop(subcompositor);
    drop(compositor);
    drop(globals);
    drop(queue);
    drop(qh);
    drop(connection);
    let _ = commands.send(ServerCommand::Stop);
    let server = server_thread.join().unwrap();
    assert_eq!(server.state.compliance_metrics.protocol_errors_total, 1);
}

#[test]
fn synchronized_child_fractional_viewport_is_rejected_when_cached_state_is_applied() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let subcompositor: client_wl_subcompositor::WlSubcompositor =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let viewporter: client_wp_viewporter::WpViewporter = globals.bind(&qh, 1..=1, ()).unwrap();
    let parent = compositor.create_surface(&qh, ());
    let child = compositor.create_surface(&qh, ());
    let _subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    let viewport = viewporter.get_viewport(&child, &qh, ());
    let buffer = TestShmBuffer::new(&shm, &qh, 4, 4).unwrap();

    // Reject the invalid effective state at the parent application boundary,
    // while the child commit still owns the live viewport resource.
    viewport.set_source(0.0, 0.0, 2.5, 2.0);
    buffer.attach(&child, 4, 4);
    child.commit();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    parent.commit();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let observed = expect_protocol_error(
        &connection,
        "wp_viewport",
        client_wp_viewport::Error::BadSize as u32,
    );
    assert_eq!(observed.object_id, viewport.id().protocol_id());

    drop(child);
    drop(parent);
    drop(buffer);
    drop(viewporter);
    drop(shm);
    drop(subcompositor);
    drop(compositor);
    drop(globals);
    drop(queue);
    drop(qh);
    drop(connection);
    let _ = commands.send(ServerCommand::Stop);
    let server = server_thread.join().unwrap();
    assert_eq!(server.state.compliance_metrics.protocol_errors_total, 1);
}

#[test]
fn viewport_source_one_fixed_unit_outside_buffer_is_out_of_buffer() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let viewporter: client_wp_viewporter::WpViewporter = globals.bind(&qh, 1..=1, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let viewport = viewporter.get_viewport(&surface, &qh, ());
    let buffer = TestShmBuffer::new(&shm, &qh, 4, 4).unwrap();

    viewport.set_source(0.0, 0.0, 4.0 + 1.0 / 256.0, 4.0);
    viewport.set_destination(4, 4);
    buffer.attach(&surface, 4, 4);
    surface.commit();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let observed = expect_protocol_error(
        &connection,
        "wp_viewport",
        client_wp_viewport::Error::OutOfBuffer as u32,
    );
    assert_eq!(observed.object_id, viewport.id().protocol_id());

    drop(viewport);
    drop(surface);
    drop(shm);
    drop(compositor);
    drop(globals);
    drop(queue);
    drop(qh);
    drop(connection);
    let _ = commands.send(ServerCommand::Stop);
    let server = server_thread.join().unwrap();
    assert_eq!(server.state.compliance_metrics.protocol_errors_total, 1);
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
    let shm_a: client_wl_shm::WlShm = globals_a.bind(&qh_a, 1..=1, ()).unwrap();
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
    commit_test_buffered_surface(&origin, &shm_a, &qh_a, 32, 32).unwrap();
    connection_a.flush().unwrap();
    wait_for_server_commands(&commands);
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
    let record = server
        .state
        .protocol_error_trace
        .records()
        .next()
        .expect("wl_surface protocol error should be attributed");
    assert_eq!(record.interface, ProtocolErrorInterface::CoreSurface);
    assert_eq!(record.category, ProtocolErrorCategory::Wire);
    assert!(!record.reason.is_empty());
    assert!(record.peer_pid.is_some());
    assert_eq!(
        record.error_code,
        Some(client_wl_surface::Error::InvalidScale as u32)
    );
    assert_eq!(record.resource_id, Some(surface_a_id));
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
fn surface_destroy_with_live_role_uses_canonical_teardown() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let (surface, xdg_surface, toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 64, 48).unwrap();
    let protocol_surface_id = surface.id().protocol_id();
    surface.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commit_registered_initial_xdg_test_buffer(&xdg_surface);
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let internal_surface_id = resolve_internal_surface_id(&commands, protocol_surface_id);
    assert!(
        capture_surface_buffer_ownership(&commands, internal_surface_id).current_surface_buffer
    );

    surface.destroy();
    connection.flush().unwrap();
    connection
        .roundtrip()
        .expect("surface-first toplevel teardown keeps the client connected");

    // Both role resources can be destroyed after the surface has already
    // gone away. They must remain inert and must not repeat the teardown.
    toplevel.set_title("late request on retired role".to_string());
    connection.flush().unwrap();
    connection
        .roundtrip()
        .expect("late toplevel requests are inert after surface teardown");
    toplevel.destroy();
    xdg_surface.destroy();
    connection.flush().unwrap();
    connection
        .roundtrip()
        .expect("late role destruction remains idempotent");

    let mut server = stop_controllable_test_server(commands, server_thread);
    assert_eq!(server.state.compliance_metrics.protocol_errors_total, 0);
    assert_eq!(
        server
            .state
            .compliance_metrics
            .lifecycle_surface_destroy_with_role_total,
        1
    );
    assert!(
        !server
            .state
            .surface_resources
            .contains_key(&internal_surface_id)
    );
    assert!(
        !server
            .state
            .toplevel_surfaces
            .contains_key(&internal_surface_id)
    );
    assert!(
        !server
            .state
            .xdg_surface_resources
            .contains_key(&internal_surface_id)
    );
    assert!(
        !server
            .state
            .xdg_surface_wm_bases
            .contains_key(&internal_surface_id)
    );
    assert!(
        !server
            .state
            .current_surface_buffers
            .contains_key(&internal_surface_id)
    );
    assert!(
        !server
            .state
            .renderable_surfaces
            .iter()
            .any(|surface| surface.surface_id == internal_surface_id)
    );
    assert!(
        server
            .state
            .focused_surface
            .as_ref()
            .is_none_or(|surface| compositor_surface_id(surface) != internal_surface_id)
    );
    assert!(
        server
            .state
            .keyboard_surface
            .as_ref()
            .is_none_or(|surface| compositor_surface_id(surface) != internal_surface_id)
    );
    assert!(
        server
            .state
            .pointer_surface
            .as_ref()
            .is_none_or(|surface| compositor_surface_id(surface) != internal_surface_id)
    );
    let repeated = server
        .state
        .teardown_surface_resource(internal_surface_id, SurfaceTeardownReason::ExplicitDestroy);
    assert_eq!(repeated.removed_resource, false);
    assert_eq!(repeated.removed_renderables, 0);
}

#[test]
fn xdg_wm_base_destroy_remains_fatal_while_inert_xdg_surface_is_live() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base_id = wm_base.id().protocol_id();
    let surface = compositor.create_surface(&qh, ());
    let protocol_surface_id = surface.id().protocol_id();
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    let internal_surface_id = resolve_internal_surface_id(&commands, protocol_surface_id);

    surface.destroy();
    connection.flush().unwrap();
    connection
        .roundtrip()
        .expect("surface-first recovery keeps the client alive");

    wm_base.destroy();
    connection.flush().unwrap();
    let observed = expect_protocol_error(
        &connection,
        "xdg_wm_base",
        client_xdg_wm_base::Error::DefunctSurfaces as u32,
    );
    assert_eq!(observed.object_id, wm_base_id);

    let server = stop_controllable_test_server(commands, server_thread);
    assert_eq!(server.state.compliance_metrics.protocol_errors_total, 1);
    assert_eq!(
        server
            .state
            .compliance_metrics
            .lifecycle_surface_destroy_with_role_total,
        1
    );
    assert!(
        !server
            .state
            .surface_resources
            .contains_key(&internal_surface_id)
    );
    assert!(
        !server
            .state
            .xdg_surface_wm_bases
            .contains_key(&internal_surface_id)
    );
}

#[test]
fn surface_destroy_with_live_subsurface_tears_down_and_leaves_role_inert() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
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
    let (parent, parent_xdg_surface, _parent_toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 64, 48).unwrap();
    let child = compositor.create_surface(&qh, ());
    let child_protocol_id = child.id().protocol_id();
    let parent_protocol_id = parent.id().protocol_id();
    let child_subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    child_subsurface.set_position(12, 8);

    parent.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commit_registered_initial_xdg_test_buffer(&parent_xdg_surface);
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    child_subsurface.set_desync();
    commit_test_buffered_surface(&child, &shm, &qh, 32, 24).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);
    let child_id = resolve_internal_surface_id(&commands, child_protocol_id);
    let parent_id = resolve_internal_surface_id(&commands, parent_protocol_id);
    assert!(capture_surface_buffer_ownership(&commands, child_id).current_surface_buffer);
    assert_eq!(
        capture_surface_role_state(&commands, child_id),
        ("subsurface".to_string(), true)
    );

    child.destroy();
    connection.flush().unwrap();
    connection
        .roundtrip()
        .expect("surface-first subsurface teardown keeps the client connected");
    child_subsurface.place_above(&parent);
    connection.flush().unwrap();
    connection
        .roundtrip()
        .expect("late subsurface requests are inert after surface teardown");
    child_subsurface.destroy();
    connection.flush().unwrap();
    connection
        .roundtrip()
        .expect("late subsurface destruction remains idempotent");
    let stack = capture_subsurface_stack_state(&commands, parent_id);
    for surface_ids in [
        stack.committed.as_ref(),
        stack.latched.as_ref(),
        stack.pending.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        assert!(
            !surface_ids.contains(&child_id),
            "surface-first cleanup must not resurrect the child in a stack"
        );
    }

    let server = stop_controllable_test_server(commands, server_thread);
    assert_eq!(server.state.compliance_metrics.protocol_errors_total, 0);
    assert_eq!(
        server
            .state
            .compliance_metrics
            .lifecycle_surface_destroy_with_role_total,
        1
    );
    assert!(!server.state.surface_resources.contains_key(&child_id));
    assert!(!server.state.surface_role_lifecycles.contains_key(&child_id));
    assert_eq!(server.state.subsurface_transactions.parent(child_id), None);
    assert!(!server.state.current_surface_buffers.contains_key(&child_id));
    assert!(!server.state.active_dmabuf_buffers.contains_key(&child_id));
    assert!(
        !server
            .state
            .renderable_surfaces
            .iter()
            .any(|surface| surface.surface_id == child_id)
    );
    assert!(
        server
            .state
            .focused_surface
            .as_ref()
            .is_none_or(|surface| compositor_surface_id(surface) != child_id)
    );
    assert!(
        server
            .state
            .keyboard_surface
            .as_ref()
            .is_none_or(|surface| compositor_surface_id(surface) != child_id)
    );
    assert!(
        server
            .state
            .pointer_surface
            .as_ref()
            .is_none_or(|surface| compositor_surface_id(surface) != child_id)
    );
}

#[test]
fn surface_destroy_with_live_xdg_popup_tears_down_and_leaves_role_inert() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let (parent, parent_xdg_surface, _parent_toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 64, 48).unwrap();
    parent.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commit_registered_initial_xdg_test_buffer(&parent_xdg_surface);
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let popup_surface = compositor.create_surface(&qh, ());
    let popup_protocol_surface_id = popup_surface.id().protocol_id();
    let popup_xdg_surface = wm_base.get_xdg_surface(&popup_surface, &qh, ());
    let positioner = wm_base.create_positioner(&qh, ());
    positioner.set_size(24, 18);
    positioner.set_anchor_rect(0, 0, 1, 1);
    let popup = popup_xdg_surface.get_popup(Some(&parent_xdg_surface), &positioner, &qh, ());
    popup_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&popup_surface, &shm, &qh, 24, 18).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    positioner.destroy();
    let popup_internal_surface_id =
        resolve_internal_surface_id(&commands, popup_protocol_surface_id);
    assert!(
        capture_surface_buffer_ownership(&commands, popup_internal_surface_id)
            .current_surface_buffer
    );
    assert_eq!(
        capture_surface_role_state(&commands, popup_internal_surface_id),
        ("xdg_popup".to_string(), true)
    );

    popup_surface.destroy();
    connection.flush().unwrap();
    connection
        .roundtrip()
        .expect("surface-first popup teardown keeps the client connected");
    popup.destroy();
    popup_xdg_surface.destroy();
    connection.flush().unwrap();
    connection
        .roundtrip()
        .expect("late popup role destruction remains idempotent");
    let server = stop_controllable_test_server(commands, server_thread);
    assert_eq!(server.state.compliance_metrics.protocol_errors_total, 0);
    assert_eq!(
        server
            .state
            .compliance_metrics
            .lifecycle_surface_destroy_with_role_total,
        1
    );
    assert!(
        !server
            .state
            .surface_resources
            .contains_key(&popup_internal_surface_id)
    );
    assert!(
        !server
            .state
            .popup_surfaces
            .contains_key(&popup_internal_surface_id)
    );
    assert!(
        !server
            .state
            .current_surface_buffers
            .contains_key(&popup_internal_surface_id)
    );
    assert!(
        !server
            .state
            .renderable_surfaces
            .iter()
            .any(|renderable| renderable.surface_id == popup_internal_surface_id)
    );
    assert!(
        server
            .state
            .focused_surface
            .as_ref()
            .is_none_or(|focused| compositor_surface_id(focused) != popup_internal_surface_id)
    );
    assert!(
        server
            .state
            .keyboard_surface
            .as_ref()
            .is_none_or(|focused| compositor_surface_id(focused) != popup_internal_surface_id)
    );
    assert!(
        server
            .state
            .pointer_surface
            .as_ref()
            .is_none_or(|focused| compositor_surface_id(focused) != popup_internal_surface_id)
    );
}

#[test]
fn surface_destroy_with_live_cursor_role_does_not_post_defunct_role_object() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let pointer = seat.get_pointer(&qh, ());
    let (origin, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 80, 60).unwrap();
    let cursor_surface = compositor.create_surface(&qh, ());

    origin.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();

    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 20.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let enter_serial = state
        .pointer_enter_serial
        .expect("cursor role assignment requires pointer focus");

    pointer.set_cursor(enter_serial, Some(&cursor_surface), 0, 0);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    cursor_surface.destroy();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let destroy_result = connection.roundtrip();

    let server = stop_controllable_test_server(commands, server_thread);
    assert!(
        destroy_result.is_ok(),
        "destroying a live Cursor wl_surface must not post defunct_role_object: {destroy_result:?}"
    );
    assert_eq!(server.state.compliance_metrics.protocol_errors_total, 0);
}

#[test]
fn xdg_surface_destroy_with_live_toplevel_cleans_role_and_leaves_resource_inert() {
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
    let toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush().unwrap();

    xdg_surface.destroy();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    toplevel.set_title("late request on retired role".to_string());
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    expect_roundtrip_alive(&connection);
    toplevel.destroy();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    expect_roundtrip_alive(&connection);
    surface.commit();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    expect_roundtrip_alive(&connection);

    let server = stop_controllable_test_server(commands, server_thread);
    assert_eq!(server.state.compliance_metrics.protocol_errors_total, 0);
    assert_eq!(
        server
            .state
            .compliance_metrics
            .lifecycle_xdg_surface_destroy_with_role_total,
        1
    );
    assert!(
        !server
            .state
            .toplevel_surfaces
            .contains_key(&surface.id().protocol_id())
    );
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
