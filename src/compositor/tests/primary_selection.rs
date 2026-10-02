use super::*;
use std::{fs::File, io::Read};

#[test]
fn removing_one_primary_device_preserves_sibling_offer_and_transfer() {
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
    let manager: client_zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let device_a = manager.get_device(&seat, &qh, ());
    let device_b = manager.get_device(&seat, &qh, ());
    let source = manager.create_source(&qh, ());
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

    let serial = state
        .keyboard_enter_serial
        .expect("focused client receives a keyboard enter serial");
    device_a.set_selection(Some(&source), serial);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    let offer_for_device = |state: &RegistryTestState, device_id| {
        state
            .primary_selection_offers
            .iter()
            .rev()
            .find_map(|(id, offer)| (*id == device_id).then(|| offer.clone()).flatten())
            .expect("primary device receives the current PRIMARY offer")
    };
    let offer_a = offer_for_device(&state, device_a.id().protocol_id());
    let offer_b = offer_for_device(&state, device_b.id().protocol_id());
    let before = capture_clipboard_state(&commands);
    assert_eq!(before.clipboard_broker_offer_count, 0);
    assert_eq!(before.primary_broker_offer_count, 2);

    device_a.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let after_device_a_destroy = capture_clipboard_state(&commands);
    assert_eq!(after_device_a_destroy.clipboard_broker_offer_count, 0);
    assert_eq!(after_device_a_destroy.primary_broker_offer_count, 1);
    assert_eq!(
        after_device_a_destroy.primary_generation,
        before.primary_generation
    );
    assert_eq!(
        after_device_a_destroy.primary_mutation_epoch,
        before.primary_mutation_epoch
    );

    let (stale_read_fd, stale_write_fd) = owned_pipe().unwrap();
    offer_a.receive("text/plain".to_string(), stale_write_fd.as_fd());
    connection.flush().unwrap();
    drop(stale_write_fd);
    queue.roundtrip(&mut state).unwrap();
    let mut stale_payload = String::new();
    File::from(stale_read_fd)
        .read_to_string(&mut stale_payload)
        .unwrap();
    assert!(stale_payload.is_empty());
    assert!(state.primary_source_send_mime_types.is_empty());

    let (read_fd, write_fd) = owned_pipe().unwrap();
    offer_b.receive("text/plain".to_string(), write_fd.as_fd());
    connection.flush().unwrap();
    drop(write_fd);
    queue.roundtrip(&mut state).unwrap();
    let mut live_payload = String::new();
    File::from(read_fd)
        .read_to_string(&mut live_payload)
        .unwrap();
    assert_eq!(live_payload, "primary payload");
    assert_eq!(state.primary_source_send_mime_types, ["text/plain"]);

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn primary_selection_real_client_pipe_transfer_and_reuse() {
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
    let manager: client_zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let device = manager.get_device(&seat, &qh, ());
    let source = manager.create_source(&qh, ());
    source.offer("text/plain".to_string());
    source.offer("text/html".to_string());
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush().unwrap();

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&surface, &shm, &qh, 32, 32).unwrap();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let serial = state
        .keyboard_enter_serial
        .expect("focused keyboard enter serial");

    device.set_selection(Some(&source), serial);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(
        state.primary_selection_events,
        [false, true],
        "focus gain publishes the empty channel before its first selection"
    );
    assert_eq!(state.primary_offer_mime_types, ["text/plain", "text/html"]);

    let offer = state
        .primary_selection_offer
        .clone()
        .expect("primary offer");
    let (read_fd, write_fd) = owned_pipe().unwrap();
    offer.receive("text/plain".to_string(), write_fd.as_fd());
    connection.flush().unwrap();
    drop(write_fd);
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let mut received = String::new();
    File::from(read_fd).read_to_string(&mut received).unwrap();
    assert_eq!(received, "primary payload");
    assert_eq!(state.primary_source_send_mime_types, ["text/plain"]);

    // A live PRIMARY source may be reused; the generic broker `used` bit is
    // not the PRIMARY protocol's single-use policy.
    device.set_selection(Some(&source), serial);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert!(state.primary_selection_events.len() >= 2);

    let _server = stop_controllable_test_server(commands, server_thread);
}
