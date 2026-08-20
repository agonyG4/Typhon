use super::*;
use std::{fs::File, io::Read};

#[test]
fn data_control_real_client_sources_transfer_to_clipboard_and_primary_targets() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let source_connection =
        Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (source_globals, mut source_queue) =
        registry_queue_init::<RegistryTestState>(&source_connection).unwrap();
    let source_qh = source_queue.handle();
    let source_seat: client_wl_seat::WlSeat = source_globals.bind(&source_qh, 1..=7, ()).unwrap();
    let source_manager: client_ext_data_control_manager_v1::ExtDataControlManagerV1 =
        source_globals.bind(&source_qh, 1..=1, ()).unwrap();
    let source_device = source_manager.get_data_device(&source_seat, &source_qh, ());
    let clipboard_source = source_manager.create_data_source(&source_qh, ());
    clipboard_source.offer("text/plain".to_string());
    clipboard_source.offer("text/html".to_string());
    let primary_source = source_manager.create_data_source(&source_qh, ());
    primary_source.offer("text/plain".to_string());
    source_connection.flush().unwrap();
    let mut source_state = RegistryTestState::default();
    source_queue.roundtrip(&mut source_state).unwrap();
    assert_eq!(source_state.data_control_selection_events, vec![false]);
    assert_eq!(
        source_state.data_control_primary_selection_events,
        vec![false]
    );

    let target_connection =
        Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (target_globals, mut target_queue) =
        registry_queue_init::<RegistryTestState>(&target_connection).unwrap();
    let target_qh = target_queue.handle();
    let compositor: client_wl_compositor::WlCompositor =
        target_globals.bind(&target_qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase =
        target_globals.bind(&target_qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = target_globals.bind(&target_qh, 1..=1, ()).unwrap();
    let target_seat: client_wl_seat::WlSeat = target_globals.bind(&target_qh, 1..=7, ()).unwrap();
    let _target_keyboard = target_seat.get_keyboard(&target_qh, ());
    let data_manager: client_wl_data_device_manager::WlDataDeviceManager =
        target_globals.bind(&target_qh, 1..=3, ()).unwrap();
    let _target_data_device = data_manager.get_data_device(&target_seat, &target_qh, ());
    let primary_manager: client_zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1 =
        target_globals.bind(&target_qh, 1..=1, ()).unwrap();
    let _target_primary_device = primary_manager.get_device(&target_seat, &target_qh, ());
    let (target_surface, target_xdg_surface, _target_toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &target_qh, 160, 120).unwrap();
    target_surface.commit();
    target_connection.flush().unwrap();
    let mut target_state = RegistryTestState::default();
    target_queue.roundtrip(&mut target_state).unwrap();
    commit_registered_initial_xdg_test_buffer(&target_xdg_surface);
    target_connection.flush().unwrap();
    wait_for_server_commands(&commands);
    target_queue.roundtrip(&mut target_state).unwrap();
    source_device.set_selection(Some(&clipboard_source));
    source_device.set_primary_selection(Some(&primary_source));
    source_connection.flush().unwrap();
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    target_queue.roundtrip(&mut target_state).unwrap();

    assert_eq!(
        target_state.data_offer_mime_types,
        ["text/plain", "text/html"]
    );
    assert_eq!(target_state.primary_offer_mime_types, ["text/plain"]);
    assert!(
        source_state
            .data_control_selection_events
            .ends_with(&[false, true])
    );
    assert!(
        source_state
            .data_control_primary_selection_events
            .ends_with(&[false, true])
    );

    let clipboard_offer = target_state
        .data_device_selection_offer
        .clone()
        .expect("normal clipboard target should receive a data-control offer");
    let (clipboard_read_fd, clipboard_write_fd) = owned_pipe().unwrap();
    clipboard_offer.receive("text/plain".to_string(), clipboard_write_fd.as_fd());
    target_connection.flush().unwrap();
    drop(clipboard_write_fd);
    wait_for_server_commands(&commands);
    target_queue.roundtrip(&mut target_state).unwrap();
    source_queue.roundtrip(&mut source_state).unwrap();
    let mut clipboard_received = String::new();
    File::from(clipboard_read_fd)
        .read_to_string(&mut clipboard_received)
        .unwrap();
    assert_eq!(clipboard_received, "data-control payload");
    assert_eq!(
        source_state.data_control_source_send_mime_types,
        ["text/plain"]
    );

    let primary_offer = target_state
        .primary_selection_offer
        .clone()
        .expect("primary target should receive a data-control offer");
    let (primary_read_fd, primary_write_fd) = owned_pipe().unwrap();
    primary_offer.receive("text/plain".to_string(), primary_write_fd.as_fd());
    target_connection.flush().unwrap();
    drop(primary_write_fd);
    wait_for_server_commands(&commands);
    target_queue.roundtrip(&mut target_state).unwrap();
    source_queue.roundtrip(&mut source_state).unwrap();
    let mut primary_received = String::new();
    File::from(primary_read_fd)
        .read_to_string(&mut primary_received)
        .unwrap();
    assert_eq!(primary_received, "data-control payload");
    assert_eq!(
        source_state.data_control_source_send_mime_types,
        ["text/plain", "text/plain"]
    );

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn data_control_source_is_rejected_when_reused() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let manager: client_ext_data_control_manager_v1::ExtDataControlManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let device = manager.get_data_device(&seat, &qh, ());
    let source = manager.create_data_source(&qh, ());
    source.offer("text/plain".to_string());
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();

    device.set_selection(Some(&source));
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    device.set_selection(Some(&source));
    connection.flush().unwrap();
    assert!(
        connection.roundtrip().is_err(),
        "reusing a data-control source must disconnect the client with UsedSource"
    );

    stop_test_server(running, server_thread);
}

#[test]
fn normal_clipboard_source_transfers_to_data_control_target() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let source_connection =
        Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (source_globals, mut source_queue) =
        registry_queue_init::<RegistryTestState>(&source_connection).unwrap();
    let source_qh = source_queue.handle();
    let compositor: client_wl_compositor::WlCompositor =
        source_globals.bind(&source_qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase =
        source_globals.bind(&source_qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = source_globals.bind(&source_qh, 1..=1, ()).unwrap();
    let seat: client_wl_seat::WlSeat = source_globals.bind(&source_qh, 1..=7, ()).unwrap();
    let _keyboard = seat.get_keyboard(&source_qh, ());
    let manager: client_wl_data_device_manager::WlDataDeviceManager =
        source_globals.bind(&source_qh, 1..=3, ()).unwrap();
    let device = manager.get_data_device(&seat, &source_qh, ());
    let source = manager.create_data_source(&source_qh, ());
    source.offer("text/plain".to_string());
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &source_qh, 160, 120).unwrap();
    surface.commit();
    source_connection.flush().unwrap();
    let mut source_state = RegistryTestState::default();
    source_queue.roundtrip(&mut source_state).unwrap();
    commands
        .send(ServerCommand::KeyboardKey {
            key: 30,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    let serial = source_state.keyboard_key_serial.expect("keyboard serial");
    device.set_selection(Some(&source), serial);
    source_connection.flush().unwrap();
    source_connection.roundtrip().unwrap();

    let target_connection =
        Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (target_globals, mut target_queue) =
        registry_queue_init::<RegistryTestState>(&target_connection).unwrap();
    let target_qh = target_queue.handle();
    let target_seat: client_wl_seat::WlSeat = target_globals.bind(&target_qh, 1..=7, ()).unwrap();
    let target_manager: client_ext_data_control_manager_v1::ExtDataControlManagerV1 =
        target_globals.bind(&target_qh, 1..=1, ()).unwrap();
    let _target_device = target_manager.get_data_device(&target_seat, &target_qh, ());
    target_connection.flush().unwrap();
    let mut target_state = RegistryTestState::default();
    target_queue.roundtrip(&mut target_state).unwrap();

    assert_eq!(target_state.data_control_selection_events, vec![true]);
    assert_eq!(
        target_state.data_control_primary_selection_events,
        vec![false]
    );
    assert_eq!(target_state.data_control_offer_mime_types, ["text/plain"]);
    let offer = target_state
        .data_control_clipboard_offer
        .clone()
        .expect("data-control target should receive the normal clipboard offer");
    let (read_fd, write_fd) = owned_pipe().unwrap();
    offer.receive("text/plain".to_string(), write_fd.as_fd());
    target_connection.flush().unwrap();
    drop(write_fd);
    target_queue.roundtrip(&mut target_state).unwrap();
    source_queue.roundtrip(&mut source_state).unwrap();
    let mut received = String::new();
    File::from(read_fd).read_to_string(&mut received).unwrap();
    assert_eq!(received, "clipboard payload");
    assert_eq!(source_state.data_source_send_mime_types, ["text/plain"]);

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn primary_source_transfers_to_data_control_target() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let source_connection =
        Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (source_globals, mut source_queue) =
        registry_queue_init::<RegistryTestState>(&source_connection).unwrap();
    let source_qh = source_queue.handle();
    let compositor: client_wl_compositor::WlCompositor =
        source_globals.bind(&source_qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase =
        source_globals.bind(&source_qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = source_globals.bind(&source_qh, 1..=1, ()).unwrap();
    let seat: client_wl_seat::WlSeat = source_globals.bind(&source_qh, 1..=7, ()).unwrap();
    let _keyboard = seat.get_keyboard(&source_qh, ());
    let manager: client_zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1 =
        source_globals.bind(&source_qh, 1..=1, ()).unwrap();
    let device = manager.get_device(&seat, &source_qh, ());
    let source = manager.create_source(&source_qh, ());
    source.offer("text/plain".to_string());
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &source_qh, 160, 120).unwrap();
    surface.commit();
    source_connection.flush().unwrap();
    let mut source_state = RegistryTestState::default();
    source_queue.roundtrip(&mut source_state).unwrap();
    commands
        .send(ServerCommand::KeyboardKey {
            key: 30,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    let serial = source_state.keyboard_key_serial.expect("keyboard serial");
    device.set_selection(Some(&source), serial);
    source_connection.flush().unwrap();
    source_connection.roundtrip().unwrap();

    let target_connection =
        Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (target_globals, mut target_queue) =
        registry_queue_init::<RegistryTestState>(&target_connection).unwrap();
    let target_qh = target_queue.handle();
    let target_seat: client_wl_seat::WlSeat = target_globals.bind(&target_qh, 1..=7, ()).unwrap();
    let target_manager: client_ext_data_control_manager_v1::ExtDataControlManagerV1 =
        target_globals.bind(&target_qh, 1..=1, ()).unwrap();
    let _target_device = target_manager.get_data_device(&target_seat, &target_qh, ());
    target_connection.flush().unwrap();
    let mut target_state = RegistryTestState::default();
    target_queue.roundtrip(&mut target_state).unwrap();

    assert_eq!(target_state.data_control_selection_events, vec![false]);
    assert_eq!(
        target_state.data_control_primary_selection_events,
        vec![true]
    );
    assert_eq!(target_state.data_control_offer_mime_types, ["text/plain"]);
    let offer = target_state
        .data_control_primary_offer
        .clone()
        .expect("data-control target should receive the primary offer");
    let (read_fd, write_fd) = owned_pipe().unwrap();
    offer.receive("text/plain".to_string(), write_fd.as_fd());
    target_connection.flush().unwrap();
    drop(write_fd);
    target_queue.roundtrip(&mut target_state).unwrap();
    source_queue.roundtrip(&mut source_state).unwrap();
    let mut received = String::new();
    File::from(read_fd).read_to_string(&mut received).unwrap();
    assert_eq!(received, "primary payload");
    assert_eq!(source_state.primary_source_send_mime_types, ["text/plain"]);

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn host_clipboard_bridge_transfers_to_data_control_target() {
    let socket_name = unique_socket_name();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let bridge = ScriptedClipboardBridge::with_host_selection(
        HostClipboardOfferId(101),
        vec!["text/plain".to_string(), "text/html".to_string()],
        b"host clipboard payload",
        Arc::clone(&requests),
    );
    let server =
        OwnCompositorServer::bind_with_clipboard_bridge(&socket_name, Box::new(bridge)).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let manager: client_ext_data_control_manager_v1::ExtDataControlManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let _device = manager.get_data_device(&seat, &qh, ());
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();

    assert_eq!(state.data_control_selection_events, vec![true]);
    assert_eq!(state.data_control_primary_selection_events, vec![false]);
    assert_eq!(
        state.data_control_offer_mime_types,
        ["text/plain", "text/html"]
    );
    let offer = state
        .data_control_clipboard_offer
        .clone()
        .expect("data-control target should receive the host offer");
    let (read_fd, write_fd) = owned_pipe().unwrap();
    offer.receive("text/plain".to_string(), write_fd.as_fd());
    connection.flush().unwrap();
    drop(write_fd);
    queue.roundtrip(&mut state).unwrap();
    let mut received = String::new();
    File::from(read_fd).read_to_string(&mut received).unwrap();
    assert_eq!(received, "host clipboard payload");
    assert_eq!(
        requests.lock().unwrap().as_slice(),
        &[(HostClipboardOfferId(101), "text/plain".to_string())]
    );

    let _server = stop_controllable_test_server(commands, server_thread);
}
