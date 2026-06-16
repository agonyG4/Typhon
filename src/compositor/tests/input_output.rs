use super::*;

#[test]
fn pointer_motion_sample_preserves_relative_deltas_in_compositor_state() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let relative = RelativePointerMotion {
        dx: 4.0,
        dy: -2.0,
        dx_unaccelerated: 7.0,
        dy_unaccelerated: -3.5,
    };

    server.send_pointer_motion_sample(PointerMotionSample {
        timestamp_usec: 123_456,
        absolute: Some(OutputPosition { x: 40.0, y: 20.0 }),
        relative: Some(relative),
    });

    assert_eq!(server.state.last_pointer_x, 40.0);
    assert_eq!(server.state.last_pointer_y, 20.0);
    assert_eq!(server.state.last_relative_pointer_motion, Some(relative));
    assert_eq!(server.state.last_pointer_motion_usec, Some(123_456));
}

#[test]
fn relative_pointer_resource_receives_timestamped_motion_for_focused_surface() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        relative_pointer: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let relative = RelativePointerMotion {
        dx: 3.25,
        dy: -1.5,
        dx_unaccelerated: 4.75,
        dy_unaccelerated: -2.25,
    };

    let state = create_focused_toplevel_and_receive_relative_pointer_motion(
        &socket_path,
        &commands,
        PointerMotionSample {
            timestamp_usec: 0x1_0000_0002,
            absolute: None,
            relative: Some(relative),
        },
    )
    .unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert_eq!(state.relative_motion_count, 1);
    assert_eq!(state.relative_motion_utime, Some(0x1_0000_0002));
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
}

#[test]
fn focused_relative_pointer_receives_raw_delta_before_lock() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        relative_pointer: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let relative = RelativePointerMotion {
        dx: 2.0,
        dy: -5.0,
        dx_unaccelerated: 2.0,
        dy_unaccelerated: -5.0,
    };

    let state = create_focused_toplevel_and_receive_relative_pointer_motion(
        &socket_path,
        &commands,
        PointerMotionSample {
            timestamp_usec: 77,
            absolute: None,
            relative: Some(relative),
        },
    )
    .unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert_eq!(state.relative_motion_count, 1);
    assert_eq!(state.relative_motion_dx, Some(relative.dx));
    assert!(!state.pointer_motion);
}

#[test]
fn locked_pointer_constraint_suppresses_absolute_motion_but_keeps_relative_motion() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        relative_pointer: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let relative = RelativePointerMotion {
        dx: 9.0,
        dy: -8.0,
        dx_unaccelerated: 11.0,
        dy_unaccelerated: -10.0,
    };

    let state = create_locked_focused_toplevel_and_receive_pointer_motion_sample(
        &socket_path,
        &commands,
        PointerMotionSample {
            timestamp_usec: 77,
            absolute: Some(OutputPosition { x: 160.0, y: 90.0 }),
            relative: Some(relative),
        },
    )
    .unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert_eq!(state.relative_motion_count, 1);
    assert_eq!(state.relative_motion_dx, Some(relative.dx));
    assert_eq!(
        state.relative_motion_dx_unaccel,
        Some(relative.dx_unaccelerated)
    );
    assert!(!state.pointer_motion);
    assert_eq!(state.pointer_surface_x, None);
    assert_eq!(state.pointer_surface_y, None);
}

#[test]
fn protocol_lock_waits_for_backend_activation_then_suppresses_absolute_motion() {
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
    let relative = RelativePointerMotion {
        dx: 5.0,
        dy: -6.0,
        dx_unaccelerated: 5.0,
        dy_unaccelerated: -6.0,
    };

    let (state, requests) = request_lock_activate_and_receive_pointer_motion_sample(
        &socket_path,
        &commands,
        PointerMotionSample {
            timestamp_usec: 99,
            absolute: Some(OutputPosition { x: 200.0, y: 200.0 }),
            relative: Some(relative),
        },
    )
    .unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert!(matches!(
        requests.as_slice(),
        [PointerConstraintBackendRequest::ActivateLocked(_)]
    ));
    assert_eq!(state.locked_count, 1);
    assert_eq!(state.relative_motion_count, 1);
    assert_eq!(state.relative_motion_utime, Some(99));
    assert_eq!(state.relative_motion_dx, Some(relative.dx));
    assert_eq!(
        state.relative_motion_dx_unaccel,
        Some(relative.dx_unaccelerated)
    );
    assert!(!state.pointer_motion);
}

#[test]
fn late_created_pointer_inherits_existing_pointer_focus() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_late_pointer_after_focus(&socket_path, &commands).unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert_eq!(state.pointer_enter_count, 2);
    assert_eq!(state.pointer_enter_frame_count, 2);
}

#[test]
fn late_created_relative_pointer_receives_locked_motion() {
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
    let relative = RelativePointerMotion {
        dx: 13.0,
        dy: -7.0,
        dx_unaccelerated: 13.0,
        dy_unaccelerated: -7.0,
    };

    let (state, requests) = late_pointer_lock_activate_and_receive_relative_motion(
        &socket_path,
        &commands,
        PointerMotionSample {
            timestamp_usec: 1234,
            absolute: None,
            relative: Some(relative),
        },
    )
    .unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert!(matches!(
        requests.as_slice(),
        [PointerConstraintBackendRequest::ActivateLocked(_)]
    ));
    assert_eq!(state.pointer_enter_count, 2);
    assert_eq!(state.pointer_enter_frame_count, 2);
    assert_eq!(state.locked_count, 1);
    assert_eq!(state.relative_motion_count, 1);
    assert_eq!(state.relative_motion_dx, Some(relative.dx));
    assert_eq!(state.relative_motion_dy, Some(relative.dy));
    assert_eq!(
        state.relative_motion_dx_unaccel,
        Some(relative.dx_unaccelerated)
    );
    assert!(!state.pointer_motion);
    let enter_before_locked = state
        .pointer_event_log
        .windows(2)
        .any(|events| events == ["frame", "locked"]);
    assert!(enter_before_locked);
}

#[test]
fn lock_activation_repairs_missing_source_pointer_enter() {
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
    let relative = RelativePointerMotion {
        dx: -4.0,
        dy: 8.0,
        dx_unaccelerated: -4.0,
        dy_unaccelerated: 8.0,
    };

    let state = lock_activation_repairs_missing_source_pointer_enter_state(
        &socket_path,
        &commands,
        PointerMotionSample {
            timestamp_usec: 5678,
            absolute: None,
            relative: Some(relative),
        },
    )
    .unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert_eq!(state.locked_count, 1);
    assert_eq!(state.relative_motion_count, 1);
    assert_eq!(state.relative_motion_dx, Some(relative.dx));
    assert!(state.pointer_enter_count >= 3);
    assert!(
        state
            .pointer_event_log
            .windows(2)
            .any(|events| events == ["frame", "locked"])
    );
}

#[test]
fn relative_motion_is_not_broadcast_to_other_client() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        relative_pointer: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let other_state = relative_motion_for_focused_client_is_not_broadcast_to_other_client(
        &socket_path,
        &commands,
    )
    .unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert_eq!(other_state.relative_motion_count, 0);
    assert_eq!(other_state.pointer_enter_count, 0);
}

#[test]
fn locked_relative_motion_survives_stale_absolute_hit_test() {
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
    let relative = RelativePointerMotion {
        dx: 6.0,
        dy: -3.0,
        dx_unaccelerated: 6.0,
        dy_unaccelerated: -3.0,
    };

    let state = locked_relative_motion_survives_stale_hit_test(
        &socket_path,
        &commands,
        PointerMotionSample {
            timestamp_usec: 101,
            absolute: Some(OutputPosition { x: 200.0, y: 200.0 }),
            relative: Some(relative),
        },
    )
    .unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert_eq!(state.locked_count, 1);
    assert_eq!(state.relative_motion_count, 1);
    assert_eq!(state.relative_motion_dx, Some(relative.dx));
    assert!(!state.pointer_motion);
}

#[test]
fn locked_relative_motion_exact_match_does_not_suppress_same_client_resources() {
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
    let relative = RelativePointerMotion {
        dx: 2.0,
        dy: 4.0,
        dx_unaccelerated: 2.0,
        dy_unaccelerated: 4.0,
    };

    let (state, relative_a_id, relative_b_id) =
        run_locked_relative_motion_targets_exact_source_pointer(
            &socket_path,
            &commands,
            PointerMotionSample {
                timestamp_usec: 202,
                absolute: None,
                relative: Some(relative),
            },
        )
        .unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    let mut actual_ids = state.relative_motion_resource_ids;
    actual_ids.sort_unstable();
    let mut expected_ids = vec![relative_a_id, relative_b_id];
    expected_ids.sort_unstable();
    assert_eq!(state.relative_motion_count, 2);
    assert_eq!(actual_ids, expected_ids);
}

#[test]
fn locked_relative_motion_falls_back_to_same_client_pointer_resource() {
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
    let relative = RelativePointerMotion {
        dx: -7.0,
        dy: 3.0,
        dx_unaccelerated: -7.0,
        dy_unaccelerated: 3.0,
    };

    let (state, relative_a_id) =
        run_locked_relative_motion_falls_back_to_same_client_pointer_resource(
            &socket_path,
            &commands,
            PointerMotionSample {
                timestamp_usec: 303,
                absolute: Some(OutputPosition { x: 180.0, y: 180.0 }),
                relative: Some(relative),
            },
        )
        .unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert_eq!(state.locked_count, 1);
    assert_eq!(state.relative_motion_count, 1);
    assert_eq!(state.relative_motion_resource_ids, vec![relative_a_id]);
    assert_eq!(state.relative_motion_dx, Some(relative.dx));
    assert!(!state.pointer_motion);
}

#[test]
fn locked_relative_motion_fallback_does_not_cross_clients() {
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

    let (locked_state, other_state) =
        run_locked_relative_motion_fallback_does_not_cross_clients(&socket_path, &commands)
            .unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert_eq!(locked_state.locked_count, 1);
    assert_eq!(other_state.relative_motion_count, 0);
    assert_eq!(other_state.pointer_enter_count, 0);
}

#[test]
fn locked_relative_motion_dispatches_to_all_same_client_resources() {
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
    let relative = RelativePointerMotion {
        dx: 8.0,
        dy: -4.0,
        dx_unaccelerated: 8.0,
        dy_unaccelerated: -4.0,
    };

    let (state, mut expected_ids) =
        run_locked_relative_motion_dispatches_to_all_same_client_resources(
            &socket_path,
            &commands,
            PointerMotionSample {
                timestamp_usec: 606,
                absolute: None,
                relative: Some(relative),
            },
        )
        .unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    expected_ids.sort_unstable();
    let mut actual_ids = state.relative_motion_resource_ids;
    actual_ids.sort_unstable();
    assert_eq!(actual_ids, expected_ids);
    assert_eq!(actual_ids.len(), 2);
}

#[test]
fn multi_client_pointer_constraints_remain_independent() {
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

    let (id_a, _id_b) =
        run_multi_client_pointer_constraints_remain_independent(&socket_path, &commands).unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert_ne!(id_a.constraint_id, 0);
}

#[test]
fn locked_relative_motion_survives_surface_tree_churn() {
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
    let relative = RelativePointerMotion {
        dx: 11.0,
        dy: -5.0,
        dx_unaccelerated: 11.0,
        dy_unaccelerated: -5.0,
    };

    let state = run_locked_relative_motion_survives_surface_tree_churn(
        &socket_path,
        &commands,
        PointerMotionSample {
            timestamp_usec: 303,
            absolute: None,
            relative: Some(relative),
        },
    )
    .unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert_eq!(state.locked_count, 1);
    assert_eq!(state.relative_motion_count, 1);
    assert_eq!(state.relative_motion_dx, Some(relative.dx));
}

#[test]
fn locked_pointer_destroy_clears_active_routing() {
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
    let relative_manager: client_zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let _relative_pointer = relative_manager.get_relative_pointer(&pointer, &qh, ());
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
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 20.0,
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

    lock.destroy();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    state.pointer_motion = false;
    state.pointer_surface_x = None;
    state.pointer_surface_y = None;
    commands
        .send(ServerCommand::PointerMotionSample(PointerMotionSample {
            timestamp_usec: 404,
            absolute: Some(OutputPosition { x: 55.0, y: 66.0 }),
            relative: Some(RelativePointerMotion {
                dx: 1.0,
                dy: 1.0,
                dx_unaccelerated: 1.0,
                dy_unaccelerated: 1.0,
            }),
        }))
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert!(state.pointer_motion);
    assert_eq!(state.relative_motion_count, 1);
}

#[test]
fn same_surface_lock_from_different_pointer_resource_is_rejected() {
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
    let pointer_a = seat.get_pointer(&qh, ());
    let pointer_b = seat.get_pointer(&qh, ());
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
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 20.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    let _lock_a = constraints.lock_pointer(
        &surface,
        &pointer_a,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    let _lock_b = constraints.lock_pointer(
        &surface,
        &pointer_b,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    connection.flush().unwrap();
    wait_for_server_commands(&commands);

    let requests = capture_pointer_constraint_backend_requests(&commands);
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert_eq!(
        requests
            .iter()
            .filter(|request| matches!(request, PointerConstraintBackendRequest::ActivateLocked(_)))
            .count(),
        1
    );
}

#[test]
fn cursor_restore_from_different_same_client_pointer_makes_host_cursor_visible() {
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
    let compatibility_restore_requests = capture_pointer_constraint_backend_requests(&commands);
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert!(compatibility_restore_requests.iter().any(|request| {
        matches!(
            request,
            PointerConstraintBackendRequest::ApplyCursorVisibility { visible: true }
        )
    }));
}

#[test]
fn pointer_leave_releases_held_button_before_leave() {
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
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    let release_before_leave = state
        .pointer_event_log
        .windows(3)
        .any(|events| events == ["button_released", "frame", "leave"]);
    assert!(
        release_before_leave,
        "event log: {:?}",
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
            PointerConstraintBackendRequest::ActivateLocked(id) => Some(*id),
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
            PointerConstraintBackendRequest::ActivateLocked(id) => Some(*id),
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

#[test]
fn idle_inhibit_capability_registers_protocol_and_tracks_inhibitor() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        idle_inhibit: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let inhibited =
        create_idle_inhibitor_for_surface_and_capture_state(&socket_path, &commands).unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert!(inhibited);
}

#[test]
fn wayland_client_surface_commit_sends_output_enter() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let state = create_client_surface_and_wait_for_enter(&socket_path);
    stop_test_server(running, server_thread);

    let state = state.unwrap();
    assert!(state.surface_enter_count >= 1);
}

#[test]
fn wayland_surface_offset_request_updates_rendered_surface_offset() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    create_client_surface_with_buffer_offset(&socket_path).unwrap();
    let server = stop_test_server(running, server_thread);

    let surface = server
        .renderable_surfaces()
        .iter()
        .find(|surface| surface.width == 40 && surface.height == 30)
        .expect("surface should be rendered");
    assert_eq!((surface.x, surface.y), (5, 7));
}

#[test]
fn wayland_client_receives_output_and_seat_capabilities() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let state = bind_output_and_seat(&socket_path);
    stop_test_server(running, server_thread);

    let state = state.unwrap();
    assert!(state.output_done);
    assert!(state.seat_has_pointer);
    assert!(state.seat_has_keyboard);
}

#[test]
fn wl_output_v1_bind_receives_only_v1_events() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let state = bind_output_at_version(&socket_path, 1);
    stop_test_server(running, server_thread);

    let state = state.unwrap();
    assert_eq!(state.output_mode_count, 1);
    assert!(!state.output_done);
    assert_eq!(state.output_scale_count, 0);
    assert!(!state.output_name);
    assert!(!state.output_description);
}

#[test]
fn wl_seat_v1_bind_does_not_receive_name() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let state = bind_seat_at_version(&socket_path, 1);
    stop_test_server(running, server_thread);

    let state = state.unwrap();
    assert!(state.seat_has_pointer);
    assert!(state.seat_has_keyboard);
    assert!(!state.seat_name);
}

#[test]
fn wayland_client_receives_updated_output_mode_after_resize() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = bind_output_then_set_output_size(&socket_path, &commands, 1600, 900);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert!(state.output_done);
    assert!(state.output_mode_count >= 2);
    assert_eq!(state.output_width, 1600);
    assert_eq!(state.output_height, 900);
}

#[test]
fn wayland_client_receives_selected_output_refresh() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = bind_output_then_set_output_refresh(&socket_path, &commands, 165);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert!(state.output_mode_count >= 2);
    assert_eq!(state.output_refresh_millihertz, 165_000);
}

#[test]
fn output_resize_preserves_selected_refresh() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state =
        bind_output_then_set_output_refresh_and_size(&socket_path, &commands, 165, 1600, 900);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert!(state.output_mode_count >= 3);
    assert_eq!(state.output_width, 1600);
    assert_eq!(state.output_height, 900);
    assert_eq!(state.output_refresh_millihertz, 165_000);
}

#[test]
fn wayland_client_receives_fractional_scale_updates_after_output_scale_change() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_fractional_scale_surface_then_set_output_scale(&socket_path, &commands, 1.5);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert_eq!(state.fractional_preferred_scales, vec![120, 180]);
}

#[test]
fn duplicate_fractional_scale_surface_does_not_disconnect_client() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let state = create_duplicate_fractional_scale_surface(&socket_path);
    stop_test_server(running, server_thread);

    let state = state.unwrap();
    assert_eq!(state.fractional_preferred_scales, vec![120, 120]);
}

#[test]
fn wayland_client_receives_keyboard_keymap_when_requested() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let state = request_keyboard_from_seat(&socket_path);
    stop_test_server(running, server_thread);

    let state = state.unwrap();
    assert!(state.keyboard_keymap);
    assert!(state.keyboard_repeat_info);
}

#[test]
fn wl_keyboard_v1_bind_does_not_receive_repeat_info() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let state = request_keyboard_from_seat_at_version(&socket_path, 1);
    stop_test_server(running, server_thread);

    let state = state.unwrap();
    assert!(state.keyboard_keymap);
    assert!(!state.keyboard_repeat_info);
}

#[test]
fn wayland_client_receives_keyboard_key_from_nested_input_bridge() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_focused_toplevel_and_receive_key(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert!(state.keyboard_keymap);
    assert!(state.keyboard_key);
}

#[test]
fn mapped_toplevel_receives_keyboard_focus_before_first_key() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_focused_toplevel_without_keypress(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert_eq!(state.keyboard_enter_count, 1);
    assert!(!state.keyboard_key);
    assert_eq!(
        state
            .keyboard_event_log
            .iter()
            .filter(|event| **event == "keyboard_modifiers")
            .count(),
        1
    );
}

#[test]
fn tab_after_initial_focus_is_only_a_key_event() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_focused_toplevel_then_press_tab(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert_eq!(state.keyboard_enter_count, 1);
    assert_eq!(state.keyboard_keys, vec![15]);
}

#[test]
fn wayland_client_receives_control_modifier_before_modified_key() {
    const CONTROL_MODIFIER_MASK: u32 = 1 << 2;

    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_focused_toplevel_and_receive_ctrl_modified_key(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert_eq!(state.keyboard_keys, vec![29, 30]);
    assert!(
        state
            .keyboard_mods_depressed
            .contains(&CONTROL_MODIFIER_MASK)
    );
}

#[test]
fn repeated_keyboard_input_on_same_surface_does_not_resend_enter() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_focused_toplevel_and_receive_two_keys(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert_eq!(state.keyboard_enter_count, 1);
    assert_eq!(state.keyboard_leave_count, 0);
}

#[test]
fn keyboard_input_ignores_resources_from_other_wayland_clients() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state =
        create_decoy_keyboard_then_focused_toplevel_and_receive_key(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert!(state.keyboard_keymap);
    assert!(state.keyboard_key);
}

#[test]
fn wayland_client_receives_pointer_motion_from_nested_input_bridge() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_focused_toplevel_and_receive_pointer_motion(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert!(state.pointer_enter);
    assert!(state.pointer_motion);
}

#[test]
fn wayland_client_receives_pointer_axis_from_nested_input_bridge() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_focused_toplevel_and_receive_pointer_axis(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert!(state.pointer_enter);
    assert_eq!(state.pointer_vertical_axis, Some(15.0));
    assert_eq!(state.pointer_enter_frame_count, 1);
    assert_eq!(state.pointer_frame_count, 3);
}

#[test]
fn pointer_click_on_same_surface_does_not_resend_enter_before_leave() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state =
        create_toplevel_then_click_and_move_pointer_on_same_surface(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert_eq!(state.pointer_enter_count, 1);
    assert_eq!(state.pointer_leave_count, 0);
}

#[test]
fn pointer_cursor_surface_commit_is_not_rendered_as_client_content() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let renderable_count =
        create_toplevel_then_set_and_commit_cursor_surface(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(renderable_count.unwrap(), 1);
}

#[test]
fn wl_pointer_v1_motion_does_not_receive_frame() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_focused_toplevel_and_receive_pointer_motion_at_seat_version(
        &socket_path,
        &commands,
        1,
    );
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert!(state.pointer_enter);
    assert!(state.pointer_motion);
    assert_eq!(state.pointer_frame_count, 0);
}

#[test]
fn wayland_pointer_motion_uses_surface_local_coordinates() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state =
        create_buffered_toplevel_and_receive_surface_local_pointer_motion(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert_eq!(state.pointer_surface_x, Some(20.0));
    assert_eq!(state.pointer_surface_y, Some(14.0));
}

#[test]
fn pointer_click_skips_subsurface_with_empty_input_region() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state =
        create_toplevel_with_empty_input_subsurface_and_click_overlap(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert_eq!(state.pointer_button_surface_id, state.parent_surface_id);
}

#[test]
fn pointer_click_hits_subsurface_when_input_region_contains_point() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_toplevel_with_custom_input_subsurface_and_click_overlap(
        &socket_path,
        &commands,
        Some((0, 0, 160, 120)),
    );
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert_eq!(state.pointer_button_surface_id, state.child_surface_id);
}

#[test]
fn pointer_release_uses_press_surface_when_surface_appears_under_cursor() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_toplevel_then_map_subsurface_before_button_release(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert_eq!(
        state.pointer_button_surface_ids,
        vec![
            state.parent_surface_id.unwrap(),
            state.parent_surface_id.unwrap()
        ]
    );
}

#[test]
fn subsurface_place_above_changes_pointer_target_after_parent_commit() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_overlapping_subsurfaces_then_place_above_after_parent_commit(
        &socket_path,
        &commands,
    );
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert_eq!(state.pointer_button_surface_id, state.child_surface_id);
    assert!(state.second_child_surface_id.is_some());
    assert!(state.pointer_leave_count >= 1);
    assert!(state.pointer_enter_count >= 2);
}

#[test]
fn subsurface_place_below_parent_excludes_child_from_top_hit() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_subsurface_below_parent_and_click_overlap(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert_eq!(state.pointer_button_surface_id, state.parent_surface_id);
}

#[test]
fn pointer_enter_v5_is_followed_by_frame() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_pointer_enter_with_v5_pointer(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert_eq!(state.pointer_enter_count, 1);
    assert_eq!(state.pointer_enter_frame_count, 1);
    assert_eq!(state.pointer_enter_without_frame_count, 0);
}

#[test]
fn xkb_v1_keymap_is_nul_terminated() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_focused_toplevel_and_receive_key(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert!(state.keyboard_keymap);
    assert_eq!(
        state.keyboard_keymap_bytes.len(),
        state.keyboard_keymap_size as usize
    );
    assert_eq!(state.keyboard_keymap_bytes.last(), Some(&0));
    assert!(
        String::from_utf8_lossy(
            &state.keyboard_keymap_bytes[..state.keyboard_keymap_bytes.len() - 1]
        )
        .contains("xkb_keymap")
    );
}
