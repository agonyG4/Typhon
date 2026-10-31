use super::*;
use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_manager_v1 as client_zxdg_output_manager_v1;

#[test]
fn xdg_output_manager_reports_logical_geometry_and_wl_output_done_at_v3() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let output: client_wl_output::WlOutput = globals.bind(&qh, 4..=4, ()).unwrap();
    let manager: client_zxdg_output_manager_v1::ZxdgOutputManagerV1 =
        globals.bind(&qh, 3..=3, ()).unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    state.output_event_log.clear();

    let _xdg_output = manager.get_xdg_output(&output, &qh, ());
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let _ = stop_test_server(running, server_thread);
    assert_eq!(
        state.output_event_log,
        [
            "logical_position",
            "logical_size",
            "xdg_name",
            "xdg_description",
            "wl_output.done",
        ]
    );
    assert_eq!(state.xdg_logical_position, Some((0, 0)));
    assert_eq!(state.xdg_logical_size, Some((1280, 800)));
    assert_eq!(state.xdg_name.as_deref(), Some("Oblivion-1"));
    assert_eq!(
        state.xdg_description.as_deref(),
        Some("Typhon native output")
    );
}

#[test]
fn wl_output_reports_unknown_physical_size_without_connector_metadata() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let (running, server_thread) = spawn_test_server(server);
    let stream = UnixStream::connect(runtime_socket_path(&socket_name)).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let _output: client_wl_output::WlOutput = globals.bind(&queue.handle(), 4..=4, ()).unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    let _ = stop_test_server(running, server_thread);
    assert_eq!(state.output_physical_size, (0, 0));
}

#[test]
fn wl_output_reports_connector_millimetres_independent_of_pixel_mode() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    assert!(server.set_output_physical_size_mm(Some((600, 340))));
    assert!(!server.set_output_physical_size_mm(Some((600, 340))));
    server.set_output_size(1024, 768);
    let (running, server_thread) = spawn_test_server(server);
    let stream = UnixStream::connect(runtime_socket_path(&socket_name)).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let _output: client_wl_output::WlOutput = globals.bind(&queue.handle(), 4..=4, ()).unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    let _ = stop_test_server(running, server_thread);
    assert_eq!(state.output_physical_size, (600, 340));
    assert_eq!((state.output_width, state.output_height), (1024, 768));
}
