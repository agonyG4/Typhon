use super::*;

#[test]
fn locked_pointer_warp_is_ignored_while_active() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        pointer_constraints: true,
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

    let lock = constraints.lock_pointer(
        &surface,
        &pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    surface.commit();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    activate_backend_locked_pointer(&commands, &mut state, &mut queue).unwrap();
    let _ = capture_pointer_constraint_backend_requests(&commands);

    state.pointer_motion = false;
    state.pointer_surface_x = None;
    state.pointer_surface_y = None;
    state.relative_motion_count = 0;

    pointer_warp.warp_pointer(&surface, &pointer, 80.0, 60.0, serial);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    let warp_requests = capture_pointer_constraint_backend_requests(&commands);
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureLastPointerPosition(reply))
        .unwrap();
    let position_after_warp = receiver.recv().unwrap();
    assert_eq!(position_after_warp, anchor);
    assert!(!state.pointer_motion);
    assert_eq!(state.relative_motion_count, 0);
    assert!(
        !warp_requests.iter().any(|request| {
            matches!(request, PointerConstraintBackendRequest::WarpPointer { .. })
        })
    );
    assert_eq!(state.locked_count, 1);
    assert_eq!(state.unlocked_count, 0);

    let relative = RelativePointerMotion {
        dx: 9.25,
        dy: -4.5,
        dx_unaccelerated: 10.75,
        dy_unaccelerated: -6.0,
    };
    commands
        .send(ServerCommand::PointerMotionSample(PointerMotionSample {
            timestamp_usec: 77,
            absolute: Some(OutputPosition { x: 7.0, y: 8.0 }),
            relative: Some(relative),
        }))
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureLastPointerPosition(reply))
        .unwrap();
    let position_after_relative = receiver.recv().unwrap();
    assert_eq!(position_after_relative, anchor);
    assert!(!state.pointer_motion);
    assert_eq!(state.relative_motion_count, 1);
    assert_eq!(state.relative_motion_dx, Some(relative.dx));
    assert_eq!(state.relative_motion_dy, Some(relative.dy));
    assert_eq!(
        state.relative_motion_dx_unaccel,
        Some(relative.dx_unaccelerated)
    );
    assert_eq!(
        state.relative_motion_dy_unaccel,
        Some(relative.dy_unaccelerated)
    );

    lock.set_cursor_position_hint(70.0, 50.0);
    surface.commit();
    lock.destroy();
    surface.commit();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let unlock_requests = capture_pointer_constraint_backend_requests(&commands);
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureLastPointerPosition(reply))
        .unwrap();
    let position_after_unlock = receiver.recv().unwrap();

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    let expected_restore = (
        f64::from(render::FIRST_SURFACE_OFFSET.0) + 70.0,
        f64::from(render::FIRST_SURFACE_OFFSET.1) + 50.0,
    );
    assert_eq!(position_after_unlock, expected_restore);
    assert!(unlock_requests.iter().any(|request| {
        matches!(
            request,
            PointerConstraintBackendRequest::Deactivate {
                restore_position: Some(OutputPosition { x, y }),
                ..
            } if (*x, *y) == expected_restore
        )
    }));
    assert_eq!(state.locked_count, 1);
    assert_eq!(state.unlocked_count, 0);
    assert_eq!(state.relative_motion_count, 1);
}
