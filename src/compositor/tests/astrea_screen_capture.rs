use super::*;

use std::os::unix::net::UnixStream;

#[derive(Default)]
struct ScreenCaptureTestState {
    authenticated: bool,
    rejected: bool,
    failed: Vec<String>,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for ScreenCaptureTestState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_astrea_shell_auth_manager_v1::AstreaShellAuthManagerV1, ()>
    for ScreenCaptureTestState
{
    fn event(
        state: &mut Self,
        _proxy: &client_astrea_shell_auth_manager_v1::AstreaShellAuthManagerV1,
        event: client_astrea_shell_auth_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            client_astrea_shell_auth_manager_v1::Event::Authenticated => state.authenticated = true,
            client_astrea_shell_auth_manager_v1::Event::Rejected => state.rejected = true,
        }
    }
}

impl Dispatch<client_astrea_screen_capture_manager_v1::AstreaScreenCaptureManagerV1, ()>
    for ScreenCaptureTestState
{
    fn event(
        _state: &mut Self,
        _proxy: &client_astrea_screen_capture_manager_v1::AstreaScreenCaptureManagerV1,
        _event: client_astrea_screen_capture_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_astrea_screen_capture_v1::AstreaScreenCaptureV1, ()>
    for ScreenCaptureTestState
{
    fn event(
        state: &mut Self,
        _proxy: &client_astrea_screen_capture_v1::AstreaScreenCaptureV1,
        event: client_astrea_screen_capture_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            client_astrea_screen_capture_v1::Event::Ready { fd, .. } => drop(fd),
            client_astrea_screen_capture_v1::Event::Failed { reason } => state.failed.push(reason),
        }
    }
}

impl Dispatch<client_wl_output::WlOutput, ()> for ScreenCaptureTestState {
    fn event(
        _state: &mut Self,
        _proxy: &client_wl_output::WlOutput,
        _event: client_wl_output::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

fn pending_capture_state(commands: &Sender<ServerCommand>) -> bool {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CapturePendingAstreaScreenCapture(reply))
        .unwrap();
    wait_for_server_commands(commands);
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should return pending capture state")
}

fn capture_pending_output(commands: &Sender<ServerCommand>) -> Option<(OutputId, bool)> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CapturePendingAstreaScreenCaptureOutput(
            reply,
        ))
        .unwrap();
    wait_for_server_commands(commands);
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report the logical output held by a pending capture")
}

#[test]
fn screen_capture_requires_exact_authenticated_client_and_accepts_current_output() {
    let socket_name = unique_socket_name();
    let capability_path =
        crate::compositor::astrea_shell_capability::test_capability_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let capability = std::fs::read_to_string(&capability_path).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let socket_path = runtime_socket_path(&socket_name);

    let connection_a = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals_a, mut queue_a) =
        registry_queue_init::<ScreenCaptureTestState>(&connection_a).unwrap();
    let qh_a = queue_a.handle();
    let auth_a = globals_a
        .bind::<client_astrea_shell_auth_manager_v1::AstreaShellAuthManagerV1, _, _>(
            &qh_a,
            1..=1,
            (),
        )
        .unwrap();
    let manager_a = globals_a
        .bind::<client_astrea_screen_capture_manager_v1::AstreaScreenCaptureManagerV1, _, _>(
            &qh_a,
            1..=1,
            (),
        )
        .unwrap();
    let output_a = globals_a
        .bind::<client_wl_output::WlOutput, _, _>(&qh_a, 1..=4, ())
        .unwrap();
    let mut state_a = ScreenCaptureTestState::default();
    auth_a.authenticate(capability.trim().to_string());
    connection_a.flush().unwrap();
    queue_a.roundtrip(&mut state_a).unwrap();
    assert!(state_a.authenticated);
    assert!(!state_a.rejected);

    let first = manager_a.capture_output(&output_a, &qh_a, ());
    connection_a.flush().unwrap();
    wait_for_server_commands(&commands);
    queue_a.roundtrip(&mut state_a).unwrap();
    assert!(pending_capture_state(&commands));

    let connection_b = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals_b, queue_b) =
        registry_queue_init::<ScreenCaptureTestState>(&connection_b).unwrap();
    let qh_b = queue_b.handle();
    let manager_b = globals_b
        .bind::<client_astrea_screen_capture_manager_v1::AstreaScreenCaptureManagerV1, _, _>(
            &qh_b,
            1..=1,
            (),
        )
        .unwrap();
    let output_b = globals_b
        .bind::<client_wl_output::WlOutput, _, _>(&qh_b, 1..=4, ())
        .unwrap();
    let _second_client_capture = manager_b.capture_output(&output_b, &qh_b, ());
    connection_b.flush().unwrap();
    wait_for_server_commands(&commands);
    let error = expect_protocol_error(
        &connection_b,
        "astrea_screen_capture_manager_v1",
        crate::astrea_screen_capture::server::astrea_screen_capture_manager_v1::Error::Unauthorized
            as u32,
    );
    assert_eq!(error.object_id, manager_b.id().protocol_id());
    drop(connection_b);
    drop(queue_b);
    drop(globals_b);
    drop(qh_b);

    let busy = manager_a.capture_output(&output_a, &qh_a, ());
    connection_a.flush().unwrap();
    wait_for_server_commands(&commands);
    queue_a.roundtrip(&mut state_a).unwrap();
    assert_eq!(state_a.failed, vec![String::from("busy")]);
    assert!(pending_capture_state(&commands));
    busy.destroy();
    connection_a.flush().unwrap();
    wait_for_server_commands(&commands);
    first.destroy();
    connection_a.flush().unwrap();
    wait_for_server_commands(&commands);
    queue_a.roundtrip(&mut state_a).unwrap();
    assert!(!pending_capture_state(&commands));

    drop(connection_a);
    drop(queue_a);
    drop(globals_a);
    drop(qh_a);
    let _ = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn screen_capture_rejects_removed_output_and_reports_output_gone() {
    let socket_name = unique_socket_name();
    let capability_path =
        crate::compositor::astrea_shell_capability::test_capability_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let capability = std::fs::read_to_string(&capability_path).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let socket_path = runtime_socket_path(&socket_name);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<ScreenCaptureTestState>(&connection).unwrap();
    let qh = queue.handle();
    let auth = globals
        .bind::<client_astrea_shell_auth_manager_v1::AstreaShellAuthManagerV1, _, _>(&qh, 1..=1, ())
        .unwrap();
    let manager = globals
        .bind::<client_astrea_screen_capture_manager_v1::AstreaScreenCaptureManagerV1, _, _>(
            &qh,
            1..=1,
            (),
        )
        .unwrap();
    let output = globals
        .bind::<client_wl_output::WlOutput, _, _>(&qh, 1..=4, ())
        .unwrap();
    let mut state = ScreenCaptureTestState::default();
    auth.authenticate(capability.trim().to_string());
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let pending = manager.capture_output(&output, &qh, ());
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert!(pending_capture_state(&commands));

    commands.send(ServerCommand::WithdrawLogicalOutput).unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.failed, vec![String::from("output_gone")]);
    assert!(!pending_capture_state(&commands));
    pending.destroy();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);

    let _stale = manager.capture_output(&output, &qh, ());
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let error = expect_protocol_error(
        &connection,
        "astrea_screen_capture_manager_v1",
        crate::astrea_screen_capture::server::astrea_screen_capture_manager_v1::Error::InvalidOutput
            as u32,
    );
    assert_eq!(error.object_id, manager.id().protocol_id());

    drop(connection);
    drop(queue);
    drop(globals);
    drop(qh);
    let _ = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn screen_capture_survives_release_of_the_request_output_binding() {
    let socket_name = unique_socket_name();
    let capability_path =
        crate::compositor::astrea_shell_capability::test_capability_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let capability = std::fs::read_to_string(&capability_path).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let socket_path = runtime_socket_path(&socket_name);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<ScreenCaptureTestState>(&connection).unwrap();
    let qh = queue.handle();
    let auth = globals
        .bind::<client_astrea_shell_auth_manager_v1::AstreaShellAuthManagerV1, _, _>(&qh, 1..=1, ())
        .unwrap();
    let manager = globals
        .bind::<client_astrea_screen_capture_manager_v1::AstreaScreenCaptureManagerV1, _, _>(
            &qh,
            1..=1,
            (),
        )
        .unwrap();
    let output = globals
        .bind::<client_wl_output::WlOutput, _, _>(&qh, 1..=4, ())
        .unwrap();
    let mut state = ScreenCaptureTestState::default();
    auth.authenticate(capability.trim().to_string());
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert!(state.authenticated);

    let pending = manager.capture_output(&output, &qh, ());
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let (logical_output_id, current_before_release) = capture_pending_output(&commands)
        .expect("capture admission should resolve the output binding");
    assert!(current_before_release);

    output.release();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert!(connection.roundtrip().is_ok());
    assert!(state.failed.is_empty());
    assert_eq!(
        capture_pending_output(&commands),
        Some((logical_output_id, true)),
        "binding release must not invalidate the logical output held by the capture"
    );

    commands.send(ServerCommand::WithdrawLogicalOutput).unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.failed, vec![String::from("output_gone")]);
    assert!(!pending_capture_state(&commands));
    pending.destroy();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);

    drop((connection, queue, globals, qh));
    let _ = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn screen_capture_disconnect_cleans_pending_request() {
    let socket_name = unique_socket_name();
    let capability_path =
        crate::compositor::astrea_shell_capability::test_capability_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let capability = std::fs::read_to_string(&capability_path).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let socket_path = runtime_socket_path(&socket_name);

    {
        let connection =
            Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
        let (globals, mut queue) =
            registry_queue_init::<ScreenCaptureTestState>(&connection).unwrap();
        let qh = queue.handle();
        let auth = globals
            .bind::<client_astrea_shell_auth_manager_v1::AstreaShellAuthManagerV1, _, _>(
                &qh,
                1..=1,
                (),
            )
            .unwrap();
        let manager = globals
            .bind::<client_astrea_screen_capture_manager_v1::AstreaScreenCaptureManagerV1, _, _>(
                &qh,
                1..=1,
                (),
            )
            .unwrap();
        let output = globals
            .bind::<client_wl_output::WlOutput, _, _>(&qh, 1..=4, ())
            .unwrap();
        let mut state = ScreenCaptureTestState::default();
        auth.authenticate(capability.trim().to_string());
        connection.flush().unwrap();
        queue.roundtrip(&mut state).unwrap();
        let _capture = manager.capture_output(&output, &qh, ());
        connection.flush().unwrap();
        wait_for_server_commands(&commands);
        queue.roundtrip(&mut state).unwrap();
        assert!(pending_capture_state(&commands));
    }

    wait_for_server_commands(&commands);
    assert!(!pending_capture_state(&commands));
    let server = stop_controllable_test_server(commands, server_thread);
    assert!(server.state.astrea_screen_captures.is_empty());
}

#[test]
fn window_preview_capture_is_advertised_to_shell_clients_as_version_two() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let (running, server_thread) = spawn_test_server(server);
    let connection =
        Connection::from_socket(UnixStream::connect(runtime_socket_path(&socket_name)).unwrap())
            .unwrap();
    let (globals, queue) = registry_queue_init::<ScreenCaptureTestState>(&connection).unwrap();
    let result = globals
        .bind::<client_astrea_screen_capture_manager_v1::AstreaScreenCaptureManagerV1, _, _>(
            &queue.handle(),
            2..=2,
            (),
        );
    running.store(false, Ordering::SeqCst);
    server_thread.join().unwrap();
    assert!(
        result.is_ok(),
        "window previews require screen capture manager version 2"
    );
}

#[test]
fn window_preview_requests_authenticate_the_exact_client_and_validate_targets() {
    let socket_name = unique_socket_name();
    let capability_path =
        crate::compositor::astrea_shell_capability::test_capability_path(&socket_name);
    let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    server.install_native_frame_test_scene(
        Vec::new(),
        &[(42, WindowId::from_raw(42).unwrap())],
        None,
    );
    let capability = std::fs::read_to_string(capability_path).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let socket_path = runtime_socket_path(&socket_name);
    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<ScreenCaptureTestState>(&connection).unwrap();
    let qh = queue.handle();
    let auth = globals
        .bind::<client_astrea_shell_auth_manager_v1::AstreaShellAuthManagerV1, _, _>(&qh, 1..=1, ())
        .unwrap();
    let manager = globals
        .bind::<client_astrea_screen_capture_manager_v1::AstreaScreenCaptureManagerV1, _, _>(
            &qh,
            2..=2,
            (),
        )
        .unwrap();
    let mut state = ScreenCaptureTestState::default();
    auth.authenticate(capability.trim().to_string());
    queue.roundtrip(&mut state).unwrap();
    assert!(state.authenticated);
    let _unknown = manager.capture_window("999999".into(), 320, 200, &qh, ());
    let _invalid_size = manager.capture_window("42".into(), 1025, 200, &qh, ());
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.failed, vec!["window_gone", "invalid_size"]);
    assert!(!pending_capture_state(&commands));
    let accepted = manager.capture_window("42".into(), 320, 200, &qh, ());
    queue.roundtrip(&mut state).unwrap();
    assert!(pending_capture_state(&commands));
    let _busy = manager.capture_window("42".into(), 320, 200, &qh, ());
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.failed.last().map(String::as_str), Some("busy"));
    accepted.destroy();
    queue.roundtrip(&mut state).unwrap();
    assert!(!pending_capture_state(&commands));

    let foreign = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (foreign_globals, mut foreign_queue) =
        registry_queue_init::<ScreenCaptureTestState>(&foreign).unwrap();
    let foreign_manager = foreign_globals
        .bind::<client_astrea_screen_capture_manager_v1::AstreaScreenCaptureManagerV1, _, _>(
            &foreign_queue.handle(),
            2..=2,
            (),
        )
        .unwrap();
    let _unauthorized =
        foreign_manager.capture_window("42".into(), 320, 200, &foreign_queue.handle(), ());
    assert!(
        foreign_queue
            .roundtrip(&mut ScreenCaptureTestState::default())
            .is_err()
    );
    assert!(!pending_capture_state(&commands));
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();
}
