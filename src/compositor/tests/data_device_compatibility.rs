use super::*;

#[test]
fn active_clipboard_source_reuse_is_tolerated_then_clear_retires_it() {
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
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let _keyboard = seat.get_keyboard(&qh, ());
    let manager: client_wl_data_device_manager::WlDataDeviceManager =
        globals.bind(&qh, 1..=3, ()).unwrap();
    let device = manager.get_data_device(&seat, &qh, ());
    let source = manager.create_data_source(&qh, ());
    source.offer("text/plain".to_string());
    let (surface, xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    surface.commit();
    connection.flush().unwrap();

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commit_registered_initial_xdg_test_buffer(&xdg_surface);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    commands
        .send(ServerCommand::KeyboardKey {
            key: 30,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let first_serial = state.keyboard_key_serial.expect("focused key serial");
    device.set_selection(Some(&source), first_serial);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.data_device_selection_events, [false, true]);

    commands
        .send(ServerCommand::KeyboardKey {
            key: 31,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let repeated_serial = state.keyboard_key_serial.expect("fresh focused key serial");
    device.set_selection(Some(&source), repeated_serial);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue
        .roundtrip(&mut state)
        .expect("reselecting the active clipboard source must be recoverable");
    assert_eq!(state.data_device_selection_events, [false, true]);

    commands
        .send(ServerCommand::KeyboardKey {
            key: 32,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let clear_serial = state.keyboard_key_serial.expect("fresh focused key serial");
    device.set_selection(None, clear_serial);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.data_device_selection_events, [false, true, false]);

    commands
        .send(ServerCommand::KeyboardKey {
            key: 33,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let retired_serial = state.keyboard_key_serial.expect("fresh focused key serial");
    device.set_selection(Some(&source), retired_serial);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let observed = expect_protocol_error(
        &connection,
        "wl_data_device",
        client_wl_data_device::Error::UsedSource as u32,
    );
    assert_eq!(observed.object_id, device.id().protocol_id());

    let server = stop_controllable_test_server(commands, server_thread);
    assert_eq!(server.state.compliance_metrics.protocol_errors_total, 1);
    assert_eq!(
        server
            .state
            .compliance_metrics
            .lifecycle_active_clipboard_source_reuse_total,
        1
    );
}

#[test]
fn drag_source_reuse_remains_a_wire_protocol_error() {
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
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let _pointer = seat.get_pointer(&qh, ());
    let manager: client_wl_data_device_manager::WlDataDeviceManager =
        globals.bind(&qh, 1..=3, ()).unwrap();
    let device = manager.get_data_device(&seat, &qh, ());
    let (origin, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
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
    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let serial = state.pointer_button_serial.expect("pointer press serial");

    let source = manager.create_data_source(&qh, ());
    source.offer("text/plain".to_string());
    source.set_actions(client_wl_data_device_manager::DndAction::Copy);
    device.start_drag(Some(&source), &origin, None, serial);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue
        .roundtrip(&mut state)
        .expect("the first drag use must succeed");

    device.start_drag(Some(&source), &origin, None, serial);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let observed = expect_protocol_error(
        &connection,
        "wl_data_device",
        client_wl_data_device::Error::UsedSource as u32,
    );
    assert_eq!(observed.object_id, device.id().protocol_id());

    let server = stop_controllable_test_server(commands, server_thread);
    assert_eq!(server.state.compliance_metrics.protocol_errors_total, 1);
    assert_eq!(
        server
            .state
            .compliance_metrics
            .lifecycle_active_clipboard_source_reuse_total,
        0
    );
}
