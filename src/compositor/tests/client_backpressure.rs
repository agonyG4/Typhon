use super::*;

#[test]
fn temporarily_unresponsive_client_survives_event_backlog() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, thread) = spawn_test_server(server);
    let connection = Connection::from_socket(UnixStream::connect(socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let _output: client_wl_output::WlOutput = globals.bind(&queue.handle(), 2..=4, ()).unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    let initial_modes = state.output_mode_count;
    let server = stop_test_server(running, thread);
    let output = &server.state.output_resources[0].resource;

    // Simulate a game blocking its UI thread while the compositor keeps
    // producing events. Exceed both the socket capacity and the 4 KiB
    // libwayland default, but remain below the bounded 1 MiB policy.
    const EVENTS: usize = 16_384;
    for _ in 0..EVENTS {
        output.mode(wl_output::Mode::Current, 1280, 720, 60_000);
    }
    let (running, thread) = spawn_test_server(server);
    let result = queue.roundtrip(&mut state);
    stop_test_server(running, thread);
    assert!(
        result.is_ok(),
        "event backlog disconnected the client: {result:?}"
    );
    assert_eq!(state.output_mode_count, initial_modes + EVENTS);
}
