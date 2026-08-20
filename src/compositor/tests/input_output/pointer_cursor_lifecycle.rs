use super::*;

#[test]
fn pointer_release_preserves_client_hidden_cursor_until_focus_changes() {
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
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let pointer = seat.get_pointer(&qh, ());
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush().unwrap();

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&surface, &shm, &qh, 32, 32).unwrap();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    pointer.set_cursor(state.pointer_enter_serial.unwrap(), None, 0, 0);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let hide_requests = capture_pointer_constraint_backend_requests(&commands);
    assert!(hide_requests.iter().any(|request| {
        matches!(
            request,
            PointerConstraintBackendRequest::ApplyCursorVisibility { visible: false }
        )
    }));

    pointer.release();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let release_requests = capture_pointer_constraint_backend_requests(&commands);

    toplevel.destroy();
    xdg_surface.destroy();
    surface.destroy();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let focus_loss_requests = capture_pointer_constraint_backend_requests(&commands);
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert!(
        !release_requests.iter().any(|request| {
            matches!(
                request,
                PointerConstraintBackendRequest::ApplyCursorVisibility { visible: true }
            )
        }),
        "releasing the pointer must not reveal the compositor cursor: {release_requests:?}"
    );
    assert!(focus_loss_requests.iter().any(|request| {
        matches!(
            request,
            PointerConstraintBackendRequest::ApplyCursorVisibility { visible: true }
        )
    }));
}
