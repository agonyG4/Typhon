use super::*;
#[test]
fn pointer_warp_global_is_capability_gated() {
    let baseline_socket = unique_socket_name();
    let baseline_server = OwnCompositorServer::bind_with_input_capabilities(
        &baseline_socket,
        InputProtocolCapabilities::desktop_baseline(),
    )
    .unwrap();
    let baseline_path = runtime_socket_path(&baseline_socket);
    let (baseline_commands, baseline_thread) = spawn_controllable_test_server(baseline_server);
    let baseline_globals = read_registry_globals(&baseline_path).unwrap();
    baseline_commands.send(ServerCommand::Stop).unwrap();
    baseline_thread.join().unwrap();

    let warp_socket = unique_socket_name();
    let warp_server = OwnCompositorServer::bind_with_input_capabilities(
        &warp_socket,
        InputProtocolCapabilities {
            pointer_warp: true,
            ..InputProtocolCapabilities::desktop_baseline()
        },
    )
    .unwrap();
    let warp_path = runtime_socket_path(&warp_socket);
    let (warp_commands, warp_thread) = spawn_controllable_test_server(warp_server);
    let warp_globals = read_registry_globals(&warp_path).unwrap();
    warp_commands.send(ServerCommand::Stop).unwrap();
    warp_thread.join().unwrap();

    assert!(
        !baseline_globals
            .iter()
            .any(|name| name == "wp_pointer_warp_v1")
    );
    assert!(warp_globals.iter().any(|name| name == "wp_pointer_warp_v1"));
}

#[test]
fn valid_pointer_warp_moves_pointer_and_sends_absolute_motion_without_relative_motion() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        pointer_warp: true,
        relative_pointer: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=2, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let pointer = seat.get_pointer(&qh, ());
    let relative_manager: client_zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let _relative_pointer = relative_manager.get_relative_pointer(&pointer, &qh, ());
    let pointer_warp: client_wp_pointer_warp_v1::WpPointerWarpV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    surface.commit();
    connection.flush().unwrap();

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let serial = state.pointer_enter_serial.unwrap();
    state.pointer_motion = false;
    state.pointer_surface_x = None;
    state.pointer_surface_y = None;
    state.relative_motion_count = 0;

    pointer_warp.warp_pointer(&surface, &pointer, 30.0, 40.0, serial);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    let requests = capture_pointer_constraint_backend_requests(&commands);
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureLastPointerPosition(reply))
        .unwrap();
    let position = receiver.recv().unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    let expected = (
        f64::from(render::FIRST_SURFACE_OFFSET.0) + 30.0,
        f64::from(render::FIRST_SURFACE_OFFSET.1) + 40.0,
    );
    assert_eq!(position, expected);
    assert!(state.pointer_motion);
    assert_eq!(state.pointer_surface_x, Some(30.0));
    assert_eq!(state.pointer_surface_y, Some(40.0));
    assert_eq!(state.relative_motion_count, 0);
    assert!(requests.iter().any(|request| {
        matches!(
            request,
            PointerConstraintBackendRequest::WarpPointer {
                position: OutputPosition { x, y }
            } if (*x, *y) == expected
        )
    }));
}

#[test]
fn pointer_warp_rejects_stale_serial_and_out_of_surface_coordinates() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        pointer_warp: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=2, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let pointer = seat.get_pointer(&qh, ());
    let pointer_warp: client_wp_pointer_warp_v1::WpPointerWarpV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    surface.commit();
    connection.flush().unwrap();

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    let anchor = (
        f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
        f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
    );
    commands
        .send(ServerCommand::PointerMotion {
            x: anchor.0,
            y: anchor.1,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let serial = state.pointer_enter_serial.unwrap();
    state.pointer_motion = false;

    pointer_warp.warp_pointer(&surface, &pointer, 30.0, 40.0, serial.wrapping_add(1));
    pointer_warp.warp_pointer(&surface, &pointer, 9999.0, 40.0, serial);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    let requests = capture_pointer_constraint_backend_requests(&commands);
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureLastPointerPosition(reply))
        .unwrap();
    let position = receiver.recv().unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert_eq!(position, anchor);
    assert!(!state.pointer_motion);
    assert!(requests.iter().all(|request| {
        !matches!(request, PointerConstraintBackendRequest::WarpPointer { .. })
    }));
}

#[test]
fn pointer_warp_rejects_a_stale_enter_serial_after_pointer_focus_moves() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        pointer_warp: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=2, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let pointer = seat.get_pointer(&qh, ());
    let pointer_warp: client_wp_pointer_warp_v1::WpPointerWarpV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let (surface_a, _xdg_surface_a, _toplevel_a) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    let (surface_b, _xdg_surface_b, _toplevel_b) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    surface_a.commit();
    surface_b.commit();
    connection.flush().unwrap();

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let surface_a_serial = state.pointer_enter_serial.unwrap();

    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0 + render::SURFACE_CASCADE_STEP) + 20.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1 + render::SURFACE_CASCADE_STEP) + 14.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    state.pointer_motion = false;
    state.pointer_surface_x = None;
    state.pointer_surface_y = None;

    pointer_warp.warp_pointer(&surface_b, &pointer, 30.0, 40.0, surface_a_serial);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    let requests = capture_pointer_constraint_backend_requests(&commands);
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureLastPointerPosition(reply))
        .unwrap();
    let position = receiver.recv().unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    let expected = (
        f64::from(render::FIRST_SURFACE_OFFSET.0 + render::SURFACE_CASCADE_STEP) + 30.0,
        f64::from(render::FIRST_SURFACE_OFFSET.1 + render::SURFACE_CASCADE_STEP) + 40.0,
    );
    let original = (
        f64::from(render::FIRST_SURFACE_OFFSET.0 + render::SURFACE_CASCADE_STEP) + 20.0,
        f64::from(render::FIRST_SURFACE_OFFSET.1 + render::SURFACE_CASCADE_STEP) + 14.0,
    );
    assert_eq!(position, original);
    assert!(!state.pointer_motion);
    assert_eq!(state.pointer_surface_x, None);
    assert_eq!(state.pointer_surface_y, None);
    assert!(!requests.iter().any(|request| {
        matches!(
            request,
            PointerConstraintBackendRequest::WarpPointer {
                position: OutputPosition { x, y }
            } if (*x, *y) == expected
        )
    }));
}

#[test]
fn cursor_restore_from_different_same_client_pointer_with_stale_serial_is_ignored() {
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
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let pointer_a = seat.get_pointer(&qh, ());
    let pointer_b = seat.get_pointer(&qh, ());
    let surface = compositor.create_surface(&qh, ());
    let cursor_surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
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

    let pointer_a_serial = state
        .pointer_enter_serials
        .iter()
        .rev()
        .find_map(|(pointer_id, serial)| {
            (*pointer_id == pointer_a.id().protocol_id()).then_some(*serial)
        })
        .expect("pointer A should have an enter serial");
    let pointer_b_serial = state
        .pointer_enter_serials
        .iter()
        .rev()
        .find_map(|(pointer_id, serial)| {
            (*pointer_id == pointer_b.id().protocol_id()).then_some(*serial)
        })
        .expect("pointer B should have an enter serial");
    assert_ne!(pointer_a_serial, pointer_b_serial);

    for _ in 0..20 {
        commands
            .send(ServerCommand::PointerButton {
                button: 0x110,
                pressed: true,
            })
            .unwrap();
        commands
            .send(ServerCommand::PointerButton {
                button: 0x110,
                pressed: false,
            })
            .unwrap();
    }
    wait_for_server_commands(&commands);

    pointer_a.set_cursor(pointer_a_serial, None, 0, 0);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let hide_requests = capture_pointer_constraint_backend_requests(&commands);
    assert!(hide_requests.iter().any(|request| {
        matches!(
            request,
            PointerConstraintBackendRequest::ApplyCursorVisibility { visible: false }
        )
    }));

    pointer_b.set_cursor(pointer_a_serial, Some(&cursor_surface), 1, 1);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let stale_restore_requests = capture_pointer_constraint_backend_requests(&commands);
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert!(!stale_restore_requests.iter().any(|request| {
        matches!(
            request,
            PointerConstraintBackendRequest::ApplyCursorVisibility { visible: true }
        )
    }));
}

#[test]
fn implicit_pointer_grab_keeps_focus_and_button_state_outside_surface_until_release() {
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
    let _pointer = seat.get_pointer(&qh, ());
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    surface.commit();
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
            button: 273,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    commands
        .send(ServerCommand::PointerMotion {
            x: -100.0,
            y: -100.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    assert_eq!(
        state.pointer_leave_count, 0,
        "{:?}",
        state.pointer_event_log
    );
    assert_eq!(
        state
            .pointer_event_log
            .iter()
            .filter(|event| **event == "button_released")
            .count(),
        0,
        "{:?}",
        state.pointer_event_log
    );
    assert!(state.pointer_motion);
    assert_eq!(
        state.pointer_surface_x,
        Some(-100.0 - f64::from(render::FIRST_SURFACE_OFFSET.0))
    );
    assert_eq!(
        state.pointer_surface_y,
        Some(-100.0 - f64::from(render::FIRST_SURFACE_OFFSET.1))
    );

    commands
        .send(ServerCommand::PointerButton {
            button: 273,
            pressed: false,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    let release_index = state
        .pointer_event_log
        .iter()
        .position(|event| *event == "button_released")
        .expect("real release should be delivered");
    let leave_index = state
        .pointer_event_log
        .iter()
        .position(|event| *event == "leave")
        .expect("post-grab refocus should send leave after final release");
    assert!(
        release_index < leave_index,
        "event log: {:?}",
        state.pointer_event_log
    );
    assert_eq!(
        state
            .pointer_event_log
            .iter()
            .filter(|event| **event == "button_released")
            .count(),
        1,
        "{:?}",
        state.pointer_event_log
    );
}

#[test]
fn implicit_pointer_grab_ends_only_after_last_button_release() {
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
    let _pointer = seat.get_pointer(&qh, ());
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    surface.commit();
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
            button: 273,
            pressed: true,
        })
        .unwrap();
    commands
        .send(ServerCommand::PointerButton {
            button: 272,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    commands
        .send(ServerCommand::PointerMotion {
            x: -100.0,
            y: -100.0,
        })
        .unwrap();
    commands
        .send(ServerCommand::PointerButton {
            button: 273,
            pressed: false,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    assert_eq!(
        state.pointer_leave_count, 0,
        "{:?}",
        state.pointer_event_log
    );
    assert_eq!(
        state
            .pointer_event_log
            .iter()
            .filter(|event| **event == "button_released")
            .count(),
        1,
        "{:?}",
        state.pointer_event_log
    );

    commands
        .send(ServerCommand::PointerButton {
            button: 272,
            pressed: false,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert_eq!(
        state
            .pointer_event_log
            .iter()
            .filter(|event| **event == "button_released")
            .count(),
        2,
        "{:?}",
        state.pointer_event_log
    );
    assert_eq!(
        state.pointer_leave_count, 1,
        "{:?}",
        state.pointer_event_log
    );
}

#[test]
fn implicit_pointer_grab_preserves_client_hidden_cursor_outside_surface() {
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
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    surface.commit();
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

    commands
        .send(ServerCommand::PointerButton {
            button: 273,
            pressed: true,
        })
        .unwrap();
    commands
        .send(ServerCommand::PointerMotion {
            x: -100.0,
            y: -100.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let mid_grab_requests = capture_pointer_constraint_backend_requests(&commands);
    assert!(
        !mid_grab_requests.iter().any(|request| {
            matches!(
                request,
                PointerConstraintBackendRequest::ApplyCursorVisibility { visible: true }
            )
        }),
        "{mid_grab_requests:?}"
    );

    commands
        .send(ServerCommand::PointerButton {
            button: 273,
            pressed: false,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let post_grab_requests = capture_pointer_constraint_backend_requests(&commands);
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert!(post_grab_requests.iter().any(|request| {
        matches!(
            request,
            PointerConstraintBackendRequest::ApplyCursorVisibility { visible: true }
        )
    }));
}

#[test]
fn implicit_grab_with_visible_cursor_still_updates_absolute_position() {
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
    let _pointer = seat.get_pointer(&qh, ());
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    surface.commit();
    connection.flush().unwrap();

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 20.0,
        })
        .unwrap();
    commands
        .send(ServerCommand::PointerButton {
            button: 272,
            pressed: true,
        })
        .unwrap();
    commands
        .send(ServerCommand::PointerMotion {
            x: -123.0,
            y: -45.0,
        })
        .unwrap();
    commands
        .send(ServerCommand::PointerButton {
            button: 272,
            pressed: false,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureLastPointerPosition(reply))
        .unwrap();
    let position = receiver.recv().unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert_eq!(position, (-123.0, -45.0));
    assert_eq!(
        state.pointer_leave_count, 1,
        "{:?}",
        state.pointer_event_log
    );
}

#[test]
fn implicit_pointer_grab_surface_destroy_clears_grab_without_stuck_button() {
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
    let _pointer = seat.get_pointer(&qh, ());
    let (surface, xdg_surface, toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    surface.commit();
    connection.flush().unwrap();

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 20.0,
        })
        .unwrap();
    commands
        .send(ServerCommand::PointerButton {
            button: 273,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    toplevel.destroy();
    xdg_surface.destroy();
    surface.destroy();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    let server = server_thread.join().unwrap();

    assert!(
        server.state.implicit_pointer_grab.is_none(),
        "{:?}",
        server.state.implicit_pointer_grab
    );
    assert!(server.state.held_pointer_buttons.is_empty());
    assert!(server.state.last_pointer_press.is_none());
    assert_eq!(
        state
            .pointer_event_log
            .iter()
            .filter(|event| **event == "button_released")
            .count(),
        0,
        "{:?}",
        state.pointer_event_log
    );
}

#[test]
fn backend_reported_deactivation_does_not_queue_duplicate_release() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        pointer_constraints: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let pointer = seat.get_pointer(&qh, ());
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush().unwrap();

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commands
        .send(ServerCommand::PointerMotion { x: 42.0, y: 48.0 })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    let _lock = constraints.lock_pointer(
        &surface,
        &pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let requests = capture_pointer_constraint_backend_requests(&commands);
    let backend_id = requests
        .iter()
        .find_map(|request| match request {
            PointerConstraintBackendRequest::ActivateLocked { id, .. } => Some(*id),
            _ => None,
        })
        .expect("expected locked backend activation request");
    commands
        .send(ServerCommand::PointerConstraintBackendActivated(backend_id))
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let _ = capture_pointer_constraint_backend_requests(&commands);

    commands
        .send(ServerCommand::PointerConstraintBackendDeactivated(
            backend_id,
        ))
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let deactivation_requests = capture_pointer_constraint_backend_requests(&commands);

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert!(
        !deactivation_requests
            .iter()
            .any(|request| matches!(request, PointerConstraintBackendRequest::Deactivate { .. }))
    );
    assert_eq!(state.unlocked_count, 1);
}

#[test]
fn confined_pointer_activation_queues_backend_region_for_mapped_surface() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        pointer_constraints: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
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
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 40, 30).unwrap();
    surface.commit();
    connection.flush().unwrap();

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 10.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 10.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    let _confine = constraints.confine_pointer(
        &surface,
        &pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let requests = capture_pointer_constraint_backend_requests(&commands);

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    let region = requests
        .iter()
        .find_map(|request| match request {
            PointerConstraintBackendRequest::ActivateConfined { region, .. } => Some(region),
            _ => None,
        })
        .expect("expected confined backend activation request with region");
    assert_eq!(region.rects.len(), 1);
    let rect = region.rects[0];
    assert_eq!(rect.x, f64::from(render::FIRST_SURFACE_OFFSET.0));
    assert_eq!(rect.y, f64::from(render::FIRST_SURFACE_OFFSET.1));
    assert_eq!(rect.width, 40.0);
    assert_eq!(rect.height, 30.0);
}

#[test]
fn confined_pointer_motion_beyond_window_border_clamps_without_leave_or_unconfined() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        pointer_constraints: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
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
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 80, 60).unwrap();
    surface.commit();
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

    let _confine = constraints.confine_pointer(
        &surface,
        &pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let requests = capture_pointer_constraint_backend_requests(&commands);
    let backend_id = requests
        .iter()
        .find_map(|request| match request {
            PointerConstraintBackendRequest::ActivateConfined { id, .. } => Some(*id),
            _ => None,
        })
        .expect("expected confined backend activation request");
    commands
        .send(ServerCommand::PointerConstraintBackendActivated(backend_id))
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.confined_count, 1);

    let leave_before = state.pointer_leave_count;
    let unconfined_before = state.unconfined_count;
    state.pointer_motion = false;
    state.pointer_surface_x = None;
    state.pointer_surface_y = None;
    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 500.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 500.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert_eq!(state.pointer_leave_count, leave_before);
    assert_eq!(state.unconfined_count, unconfined_before);
    assert!(state.pointer_motion);
    assert_eq!(state.pointer_surface_x, Some(79.0));
    assert_eq!(state.pointer_surface_y, Some(59.0));
}

#[test]
fn confined_pointer_activation_region_intersects_constraint_and_input_regions() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        pointer_constraints: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
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
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 100, 80).unwrap();
    let input_region = compositor.create_region(&qh, ());
    input_region.add(20, 10, 30, 20);
    surface.set_input_region(Some(&input_region));
    surface.commit();
    connection.flush().unwrap();

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 25.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 15.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    let constraint_region = compositor.create_region(&qh, ());
    constraint_region.add(10, 5, 40, 30);
    let _confine = constraints.confine_pointer(
        &surface,
        &pointer,
        Some(&constraint_region),
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let requests = capture_pointer_constraint_backend_requests(&commands);

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    let region = requests
        .iter()
        .find_map(|request| match request {
            PointerConstraintBackendRequest::ActivateConfined { region, .. } => Some(region),
            _ => None,
        })
        .expect("expected confined backend activation request with region");
    assert_eq!(region.rects.len(), 1);
    let rect = region.rects[0];
    assert_eq!(rect.x, f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0);
    assert_eq!(rect.y, f64::from(render::FIRST_SURFACE_OFFSET.1) + 10.0);
    assert_eq!(rect.width, 30.0);
    assert_eq!(rect.height, 20.0);
}

#[test]
fn confined_pointer_set_region_updates_backend_only_after_surface_commit() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        pointer_constraints: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
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
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 100, 80).unwrap();
    surface.commit();
    connection.flush().unwrap();

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 25.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 15.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    let confine = constraints.confine_pointer(
        &surface,
        &pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let requests = capture_pointer_constraint_backend_requests(&commands);
    let backend_id = requests
        .iter()
        .find_map(|request| match request {
            PointerConstraintBackendRequest::ActivateConfined { id, .. } => Some(*id),
            _ => None,
        })
        .expect("expected confined backend activation request");
    commands
        .send(ServerCommand::PointerConstraintBackendActivated(backend_id))
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let _ = capture_pointer_constraint_backend_requests(&commands);

    let update_region = compositor.create_region(&qh, ());
    update_region.add(30, 20, 10, 10);
    confine.set_region(Some(&update_region));
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let before_commit = capture_pointer_constraint_backend_requests(&commands);
    assert!(before_commit.is_empty(), "requests: {before_commit:?}");

    surface.commit();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let after_commit = capture_pointer_constraint_backend_requests(&commands);

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    let region = after_commit
        .iter()
        .find_map(|request| match request {
            PointerConstraintBackendRequest::UpdateConfinedRegion { region, .. } => Some(region),
            _ => None,
        })
        .expect("expected committed confined region update");
    assert_eq!(region.rects.len(), 1);
    let rect = region.rects[0];
    assert_eq!(rect.x, f64::from(render::FIRST_SURFACE_OFFSET.0) + 30.0);
    assert_eq!(rect.y, f64::from(render::FIRST_SURFACE_OFFSET.1) + 20.0);
    assert_eq!(rect.width, 10.0);
    assert_eq!(rect.height, 10.0);
}

#[test]
fn locked_pointer_destroy_restores_committed_cursor_position_hint() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        pointer_constraints: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
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
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    surface.commit();
    connection.flush().unwrap();

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    let lock = constraints.lock_pointer(
        &surface,
        &pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    activate_backend_locked_pointer(&commands, &mut state, &mut queue).unwrap();

    lock.set_cursor_position_hint(9.0, 11.0);
    surface.commit();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    lock.destroy();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let deactivation_requests = capture_pointer_constraint_backend_requests(&commands);
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureLastPointerPosition(reply))
        .unwrap();
    let position = receiver.recv().unwrap();

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert_eq!(
        position,
        (
            f64::from(render::FIRST_SURFACE_OFFSET.0) + 9.0,
            f64::from(render::FIRST_SURFACE_OFFSET.1) + 11.0
        )
    );
    assert!(deactivation_requests.iter().any(|request| {
        matches!(
            request,
            PointerConstraintBackendRequest::Deactivate {
                restore_position: Some(OutputPosition { x, y }),
                ..
            } if *x == f64::from(render::FIRST_SURFACE_OFFSET.0) + 9.0
                && *y == f64::from(render::FIRST_SURFACE_OFFSET.1) + 11.0
        )
    }));
}

#[test]
fn locked_pointer_unlock_without_hint_restores_exact_activation_anchor() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        pointer_constraints: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
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
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    surface.commit();
    connection.flush().unwrap();

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    let anchor = (
        f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
        f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
    );
    commands
        .send(ServerCommand::PointerMotion {
            x: anchor.0,
            y: anchor.1,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    let lock = constraints.lock_pointer(
        &surface,
        &pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    activate_backend_locked_pointer(&commands, &mut state, &mut queue).unwrap();
    commands
        .send(ServerCommand::PointerMotionSample(PointerMotionSample {
            timestamp_usec: 44,
            absolute: Some(OutputPosition { x: 1.0, y: 1.0 }),
            relative: Some(RelativePointerMotion {
                dx: 900.0,
                dy: -700.0,
                dx_unaccelerated: 900.0,
                dy_unaccelerated: -700.0,
            }),
        }))
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    lock.destroy();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let deactivation_requests = capture_pointer_constraint_backend_requests(&commands);
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureLastPointerPosition(reply))
        .unwrap();
    let position = receiver.recv().unwrap();

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert_eq!(position, anchor);
    assert!(deactivation_requests.iter().any(|request| {
        matches!(
            request,
            PointerConstraintBackendRequest::Deactivate {
                restore_position: Some(OutputPosition { x, y }),
                ..
            } if (*x, *y) == anchor
        )
    }));
}

#[test]
fn pending_uncommitted_cursor_hint_is_not_used_on_unlock() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        pointer_constraints: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
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
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    surface.commit();
    connection.flush().unwrap();

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    let anchor = (
        f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
        f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
    );
    commands
        .send(ServerCommand::PointerMotion {
            x: anchor.0,
            y: anchor.1,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    let lock = constraints.lock_pointer(
        &surface,
        &pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    activate_backend_locked_pointer(&commands, &mut state, &mut queue).unwrap();

    lock.set_cursor_position_hint(9.0, 11.0);
    lock.destroy();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureLastPointerPosition(reply))
        .unwrap();
    let position = receiver.recv().unwrap();

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert_eq!(position, anchor);
}

#[test]
fn locked_unlock_does_not_reveal_committed_hint_before_followup_warp() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        pointer_constraints: true,
        pointer_warp: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
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
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let pointer_warp: client_wp_pointer_warp_v1::WpPointerWarpV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    surface.commit();
    connection.flush().unwrap();

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    let origin_x = f64::from(render::FIRST_SURFACE_OFFSET.0);
    let origin_y = f64::from(render::FIRST_SURFACE_OFFSET.1);
    commands
        .send(ServerCommand::PointerMotion {
            x: origin_x + 30.0,
            y: origin_y,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let serial = state.pointer_enter_serial.unwrap();

    let lock = constraints.lock_pointer(
        &surface,
        &pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    activate_backend_locked_pointer(&commands, &mut state, &mut queue).unwrap();
    let _ = capture_pointer_constraint_backend_requests(&commands);

    lock.set_cursor_position_hint(120.0, 0.0);
    surface.commit();
    lock.destroy();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let unlock_requests = capture_pointer_constraint_backend_requests(&commands);
    assert!(unlock_requests.iter().any(|request| {
        matches!(
            request,
            PointerConstraintBackendRequest::Deactivate {
                restore_position: Some(OutputPosition { x, y }),
                ..
            } if (*x, *y) == (origin_x + 120.0, origin_y)
        )
    }));
    assert!(!unlock_requests.iter().any(|request| matches!(
        request,
        PointerConstraintBackendRequest::ApplyCursorVisibility { visible: true }
    )));

    pointer_warp.warp_pointer(&surface, &pointer, 30.0, 0.0, serial);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let warp_requests = capture_pointer_constraint_backend_requests(&commands);
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    let warp_index = warp_requests.iter().position(|request| {
        matches!(
            request,
            PointerConstraintBackendRequest::WarpPointer {
                position: OutputPosition { x, y }
            } if (*x, *y) == (origin_x + 30.0, origin_y)
        )
    });
    let visible_index = warp_requests.iter().position(|request| {
        matches!(
            request,
            PointerConstraintBackendRequest::ApplyCursorVisibility { visible: true }
        )
    });
    assert!(warp_index.is_some());
    assert!(visible_index.is_some());
    assert!(warp_index.unwrap() < visible_index.unwrap());
}

#[test]
fn locked_unlock_reveals_committed_hint_after_dispatch_fallback_without_warp() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        pointer_constraints: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
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
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    surface.commit();
    connection.flush().unwrap();

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    let origin_x = f64::from(render::FIRST_SURFACE_OFFSET.0);
    let origin_y = f64::from(render::FIRST_SURFACE_OFFSET.1);
    commands
        .send(ServerCommand::PointerMotion {
            x: origin_x + 30.0,
            y: origin_y,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    let lock = constraints.lock_pointer(
        &surface,
        &pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    activate_backend_locked_pointer(&commands, &mut state, &mut queue).unwrap();
    let _ = capture_pointer_constraint_backend_requests(&commands);

    lock.set_cursor_position_hint(120.0, 0.0);
    surface.commit();
    lock.destroy();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let unlock_requests = capture_pointer_constraint_backend_requests(&commands);
    assert!(!unlock_requests.iter().any(|request| matches!(
        request,
        PointerConstraintBackendRequest::ApplyCursorVisibility { visible: true }
    )));

    for _ in 0..4 {
        wait_for_server_commands(&commands);
    }
    let fallback_requests = capture_pointer_constraint_backend_requests(&commands);
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureLastPointerPosition(reply))
        .unwrap();
    let final_position = receiver.recv().unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert!(fallback_requests.iter().any(|request| {
        matches!(
            request,
            PointerConstraintBackendRequest::ApplyCursorVisibility { visible: true }
        )
    }));
    assert_eq!(final_position, (origin_x + 120.0, origin_y));
}

#[test]
fn locked_unlock_set_cursor_none_keeps_builtin_cursor_hidden() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        pointer_constraints: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
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
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    surface.commit();
    connection.flush().unwrap();

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 30.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1),
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let serial = state.pointer_enter_serial.unwrap();

    let lock = constraints.lock_pointer(
        &surface,
        &pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    activate_backend_locked_pointer(&commands, &mut state, &mut queue).unwrap();
    let _ = capture_pointer_constraint_backend_requests(&commands);

    lock.destroy();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    pointer.set_cursor(serial, None, 0, 0);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let requests = capture_pointer_constraint_backend_requests(&commands);
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert!(!requests.iter().any(|request| matches!(
        request,
        PointerConstraintBackendRequest::ApplyCursorVisibility { visible: true }
    )));
}

#[test]
fn invalid_cursor_position_hint_cannot_teleport_pointer() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        pointer_constraints: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
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
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    surface.commit();
    connection.flush().unwrap();

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    let anchor = (
        f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
        f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
    );
    commands
        .send(ServerCommand::PointerMotion {
            x: anchor.0,
            y: anchor.1,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    let lock = constraints.lock_pointer(
        &surface,
        &pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    activate_backend_locked_pointer(&commands, &mut state, &mut queue).unwrap();

    lock.set_cursor_position_hint(f64::NAN, f64::INFINITY);
    surface.commit();
    lock.destroy();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureLastPointerPosition(reply))
        .unwrap();
    let position = receiver.recv().unwrap();

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert_eq!(position, anchor);
}

#[test]
fn pointer_release_deactivates_locked_constraint() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        relative_pointer: true,
        pointer_constraints: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let pointer = seat.get_pointer(&qh, ());
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush().unwrap();

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commands
        .send(ServerCommand::PointerMotion { x: 42.0, y: 48.0 })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let _lock = constraints.lock_pointer(
        &surface,
        &pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    activate_backend_locked_pointer(&commands, &mut state, &mut queue).unwrap();

    pointer.release();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert_eq!(state.unlocked_count, 0);
    assert_eq!(state.locked_count, 1);
}

#[test]
fn backend_pointer_constraint_failure_marks_constraint_defunct() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        pointer_constraints: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let pointer = seat.get_pointer(&qh, ());
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush().unwrap();

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commands
        .send(ServerCommand::PointerMotion { x: 42.0, y: 48.0 })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let _lock = constraints.lock_pointer(
        &surface,
        &pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Oneshot,
        &qh,
        (),
    );
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let requests = capture_pointer_constraint_backend_requests(&commands);
    let backend_id = requests
        .iter()
        .find_map(|request| match request {
            PointerConstraintBackendRequest::ActivateLocked { id, .. } => Some(*id),
            _ => None,
        })
        .unwrap();
    commands
        .send(ServerCommand::PointerConstraintBackendFailed(backend_id))
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert_eq!(state.locked_count, 0);
}
