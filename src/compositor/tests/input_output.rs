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
fn locked_relative_motion_is_followed_by_source_pointer_frame() {
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

    let (state, _relative_id) =
        run_locked_relative_motion_falls_back_to_same_client_pointer_resource(
            &socket_path,
            &commands,
            PointerMotionSample {
                timestamp_usec: 178,
                absolute: None,
                relative: Some(RelativePointerMotion {
                    dx: 6.0,
                    dy: -3.0,
                    dx_unaccelerated: 6.0,
                    dy_unaccelerated: -3.0,
                }),
            },
        )
        .unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert_eq!(state.relative_motion_count, 1);
    assert!(
        state
            .pointer_event_log
            .windows(2)
            .any(|events| events == ["relative", "frame"])
    );
}

#[test]
fn locked_relative_motion_dispatches_sdl_pending_delta_without_button_frame() {
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

    let (state, _relative_id) =
        run_locked_relative_motion_falls_back_to_same_client_pointer_resource(
            &socket_path,
            &commands,
            PointerMotionSample {
                timestamp_usec: 179,
                absolute: None,
                relative: Some(RelativePointerMotion {
                    dx: 2.0,
                    dy: 2.0,
                    dx_unaccelerated: 2.0,
                    dy_unaccelerated: 2.0,
                }),
            },
        )
        .unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert_eq!(state.relative_motion_count, 1);
    assert_eq!(state.sdl_pending_relative_motion_count, 0);
    assert_eq!(state.sdl_camera_motion_count, 1);
    assert!(!state.pointer_button);
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
        [PointerConstraintBackendRequest::ActivateLocked { .. }]
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
        [PointerConstraintBackendRequest::ActivateLocked { .. }]
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
fn locked_relative_motion_exact_match_does_not_duplicate_to_same_client_resources() {
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
    assert_ne!(relative_a_id, relative_b_id);
    assert_eq!(state.relative_motion_count, 2);
    assert_eq!(actual_ids, expected_ids);
}

#[test]
fn locked_relative_motion_routes_to_different_same_client_pointer_resource() {
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
    assert!(state.relative_motion_resource_ids.contains(&relative_a_id));
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
    assert_eq!(other_state.pointer_frame_count, 0);
    assert_eq!(other_state.pointer_enter_count, 0);
}

#[test]
fn locked_relative_motion_ignores_destroyed_same_client_relative_pointer() {
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
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 5..=5, ()).unwrap();
    let pointer_a = seat.get_pointer(&qh, ());
    let pointer_b = seat.get_pointer(&qh, ());
    let relative_manager: client_zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
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

    let stale_relative = relative_manager.get_relative_pointer(&pointer_a, &qh, ());
    let live_relative = relative_manager.get_relative_pointer(&pointer_b, &qh, ());
    let stale_id = stale_relative.id().protocol_id();
    let live_id = live_relative.id().protocol_id();
    let live_pointer_id = pointer_b.id().protocol_id();
    stale_relative.destroy();
    let _lock = constraints.lock_pointer(
        &surface,
        &pointer_b,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    activate_backend_locked_pointer(&commands, &mut state, &mut queue).unwrap();
    clear_locked_relative_motion_observations(&mut state);

    commands
        .send(ServerCommand::PointerMotionSample(PointerMotionSample {
            timestamp_usec: 707,
            absolute: None,
            relative: Some(RelativePointerMotion {
                dx: 4.0,
                dy: 5.0,
                dx_unaccelerated: 4.0,
                dy_unaccelerated: 5.0,
            }),
        }))
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert_eq!(state.relative_motion_count, 1);
    assert_eq!(state.relative_motion_resource_ids, vec![live_id]);
    assert!(!state.relative_motion_resource_ids.contains(&stale_id));
    assert_eq!(state.pointer_frame_resource_ids, vec![live_pointer_id]);
}

#[test]
fn locked_relative_motion_dispatches_to_all_same_client_pointer_resources() {
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

    let (state, expected_ids) = run_locked_relative_motion_dispatches_to_all_same_client_resources(
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

    let mut expected_ids = expected_ids;
    let mut actual_ids = state.relative_motion_resource_ids;
    expected_ids.sort_unstable();
    actual_ids.sort_unstable();
    assert_eq!(actual_ids, expected_ids);
    assert_eq!(actual_ids.len(), 2);
    actual_ids.dedup();
    assert_eq!(actual_ids.len(), 2);
}

#[test]
fn locked_relative_motion_shared_source_pointer_gets_one_frame() {
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

    let result =
        run_locked_relative_motion_shared_source_pointer_frames(&socket_path, &commands).unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    let mut actual_ids = result.state.relative_motion_resource_ids;
    let mut expected_ids = result.relative_ids;
    actual_ids.sort_unstable();
    expected_ids.sort_unstable();
    assert_eq!(actual_ids, expected_ids);
    assert_eq!(result.state.pointer_frame_resource_ids, result.pointer_ids);
}

#[test]
fn locked_relative_motion_different_same_client_source_pointers_each_get_frame() {
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

    let result =
        run_locked_relative_motion_different_source_pointer_frames(&socket_path, &commands)
            .unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    let mut actual_relative_ids = result.state.relative_motion_resource_ids;
    let mut expected_relative_ids = result.relative_ids;
    actual_relative_ids.sort_unstable();
    expected_relative_ids.sort_unstable();
    assert_eq!(actual_relative_ids, expected_relative_ids);

    let mut actual_pointer_ids = result.state.pointer_frame_resource_ids;
    let mut expected_pointer_ids = result.pointer_ids;
    actual_pointer_ids.sort_unstable();
    expected_pointer_ids.sort_unstable();
    assert_eq!(actual_pointer_ids, expected_pointer_ids);
}

#[test]
fn locked_constraint_activation_anchor_survives_intervening_cursor_move() {
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
    let moved = (anchor.0 + 70.0, anchor.1 + 40.0);
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
    let requests = capture_pointer_constraint_backend_requests(&commands);
    let backend_id = requests
        .iter()
        .find_map(|request| match request {
            PointerConstraintBackendRequest::ActivateLocked { id, .. } => Some(*id),
            _ => None,
        })
        .expect("expected locked backend activation request");

    commands
        .send(ServerCommand::PointerMotion {
            x: moved.0,
            y: moved.1,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    commands
        .send(ServerCommand::PointerConstraintBackendActivated(backend_id))
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
    assert_ne!(position, moved);
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
fn locked_pointer_button_transitions_do_not_clear_relative_motion_route() {
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

    for (index, command) in [
        ServerCommand::PointerButton {
            button: 273,
            pressed: true,
        },
        ServerCommand::PointerButton {
            button: 273,
            pressed: false,
        },
        ServerCommand::PointerButton {
            button: 272,
            pressed: true,
        },
        ServerCommand::PointerButton {
            button: 272,
            pressed: false,
        },
    ]
    .into_iter()
    .enumerate()
    {
        commands.send(command).unwrap();
        wait_for_server_commands(&commands);
        commands
            .send(ServerCommand::PointerMotionSample(PointerMotionSample {
                timestamp_usec: 800 + index as u64,
                absolute: None,
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
        assert_eq!(state.locked_count, 1);
        assert_eq!(state.unlocked_count, 0);
        assert_eq!(state.relative_motion_count, index + 1);
    }

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();
}

#[test]
fn locked_relative_motion_does_not_wait_for_button_frame() {
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
    clear_locked_relative_motion_observations(&mut state);

    for index in 0..3 {
        commands
            .send(ServerCommand::PointerMotionSample(PointerMotionSample {
                timestamp_usec: 900 + index,
                absolute: None,
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
    }
    assert_eq!(state.relative_motion_count, 3);
    assert_eq!(state.sdl_pending_relative_motion_count, 0);
    assert_eq!(state.sdl_camera_motion_count, 3);

    commands
        .send(ServerCommand::PointerButton {
            button: 272,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
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

    assert_eq!(state.sdl_pending_relative_motion_count, 0);
    assert_eq!(state.sdl_camera_motion_count, 3);
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
            .filter(|request| matches!(
                request,
                PointerConstraintBackendRequest::ActivateLocked { .. }
            ))
            .count(),
        1
    );
}

#[test]
fn pending_oneshot_locked_destroy_removes_queued_activation() {
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
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=2, ()).unwrap();
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
        client_zwp_pointer_constraints_v1::Lifetime::Oneshot,
        &qh,
        (),
    );
    lock.set_cursor_position_hint(70.0, 50.0);
    surface.commit();
    lock.destroy();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    let requests = capture_pointer_constraint_backend_requests(&commands);
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert!(
        requests.iter().all(|request| !matches!(
            request,
            PointerConstraintBackendRequest::ActivateLocked { .. }
        )),
        "destroyed pending oneshot lock must not leave queued activation: {requests:?}"
    );
    assert_eq!(state.locked_count, 0);
    assert_eq!(state.unlocked_count, 0);
}

#[test]
fn pending_persistent_locked_destroy_removes_queued_activation() {
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
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=2, ()).unwrap();
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
    lock.destroy();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    let requests = capture_pointer_constraint_backend_requests(&commands);
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert!(
        requests.iter().all(|request| !matches!(
            request,
            PointerConstraintBackendRequest::ActivateLocked { .. }
        )),
        "destroyed pending persistent lock must not leave queued activation: {requests:?}"
    );
    assert_eq!(state.locked_count, 0);
    assert_eq!(state.unlocked_count, 0);
}

#[test]
fn pending_confined_destroy_removes_queued_activation() {
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
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=2, ()).unwrap();
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

    let confined = constraints.confine_pointer(
        &surface,
        &pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    confined.destroy();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    let requests = capture_pointer_constraint_backend_requests(&commands);
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert!(
        requests.iter().all(|request| !matches!(
            request,
            PointerConstraintBackendRequest::ActivateConfined { .. }
        )),
        "destroyed pending confinement must not leave queued activation: {requests:?}"
    );
    assert_eq!(state.confined_count, 0);
    assert_eq!(state.unconfined_count, 0);
}

#[test]
fn pending_oneshot_committed_hint_destroy_warps_without_activation() {
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
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=2, ()).unwrap();
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
        client_zwp_pointer_constraints_v1::Lifetime::Oneshot,
        &qh,
        (),
    );
    lock.set_cursor_position_hint(70.0, 50.0);
    surface.commit();
    lock.destroy();
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
        f64::from(render::FIRST_SURFACE_OFFSET.0) + 70.0,
        f64::from(render::FIRST_SURFACE_OFFSET.1) + 50.0,
    );
    assert_eq!(position, expected);
    assert!(requests.iter().any(|request| {
        matches!(
            request,
            PointerConstraintBackendRequest::WarpPointer {
                position: OutputPosition { x, y }
            } if (*x, *y) == expected
        )
    }));
    assert!(requests.iter().all(|request| {
        !matches!(
            request,
            PointerConstraintBackendRequest::ActivateLocked { .. }
        )
    }));
    assert_eq!(state.locked_count, 0);
    assert_eq!(state.unlocked_count, 0);
}

#[test]
fn pending_oneshot_uncommitted_hint_destroy_does_not_warp() {
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
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=2, ()).unwrap();
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
        client_zwp_pointer_constraints_v1::Lifetime::Oneshot,
        &qh,
        (),
    );
    lock.set_cursor_position_hint(70.0, 50.0);
    lock.destroy();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);

    let requests = capture_pointer_constraint_backend_requests(&commands);
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureLastPointerPosition(reply))
        .unwrap();
    let position = receiver.recv().unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert_eq!(position, anchor);
    assert!(requests.iter().all(|request| {
        !matches!(request, PointerConstraintBackendRequest::WarpPointer { .. })
    }));
}

#[test]
fn pending_oneshot_invalid_hint_destroy_does_not_warp() {
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
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=2, ()).unwrap();
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
        client_zwp_pointer_constraints_v1::Lifetime::Oneshot,
        &qh,
        (),
    );
    lock.set_cursor_position_hint(9999.0, 50.0);
    surface.commit();
    lock.destroy();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);

    let requests = capture_pointer_constraint_backend_requests(&commands);
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureLastPointerPosition(reply))
        .unwrap();
    let position = receiver.recv().unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert_eq!(position, anchor);
    assert!(requests.iter().all(|request| {
        !matches!(request, PointerConstraintBackendRequest::WarpPointer { .. })
    }));
}

#[test]
fn pending_persistent_hint_destroy_is_not_reinterpreted_as_oneshot_warp() {
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
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=2, ()).unwrap();
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
    lock.set_cursor_position_hint(70.0, 50.0);
    surface.commit();
    lock.destroy();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);

    let requests = capture_pointer_constraint_backend_requests(&commands);
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureLastPointerPosition(reply))
        .unwrap();
    let position = receiver.recv().unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert_eq!(position, anchor);
    assert!(requests.iter().all(|request| {
        !matches!(request, PointerConstraintBackendRequest::WarpPointer { .. })
    }));
}

#[test]
fn active_lock_then_oneshot_warp_fallback_does_not_block_next_lock() {
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
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=2, ()).unwrap();
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

    let lock_a = constraints.lock_pointer(
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

    lock_a.destroy();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let deactivate_a = capture_pointer_constraint_backend_requests(&commands);
    assert!(
        deactivate_a.iter().any(|request| {
            matches!(request, PointerConstraintBackendRequest::Deactivate { .. })
        })
    );

    let lock_b = constraints.lock_pointer(
        &surface,
        &pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Oneshot,
        &qh,
        (),
    );
    lock_b.set_cursor_position_hint(70.0, 50.0);
    surface.commit();
    lock_b.destroy();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let fallback_b = capture_pointer_constraint_backend_requests(&commands);
    assert!(
        fallback_b.iter().any(|request| {
            matches!(request, PointerConstraintBackendRequest::WarpPointer { .. })
        })
    );
    assert!(fallback_b.iter().all(|request| {
        !matches!(
            request,
            PointerConstraintBackendRequest::ActivateLocked { .. }
        )
    }));

    let _lock_c = constraints.lock_pointer(
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
    let activate_c = capture_pointer_constraint_backend_requests(&commands);
    let backend_id_c = activate_c
        .iter()
        .find_map(|request| match request {
            PointerConstraintBackendRequest::ActivateLocked { id, .. } => Some(*id),
            _ => None,
        })
        .expect("persistent lock C should queue backend activation");
    commands
        .send(ServerCommand::PointerConstraintBackendActivated(
            backend_id_c,
        ))
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert_eq!(state.locked_count, 2);
}

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
fn pointer_warp_accepts_same_client_enter_serial_for_target_surface() {
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
    assert_eq!(position, expected);
    assert!(state.pointer_motion);
    assert_eq!(state.pointer_surface_x, Some(30.0));
    assert_eq!(state.pointer_surface_y, Some(40.0));
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
    let (surface, xdg_surface, _toplevel) =
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

    let result = create_toplevel_then_set_and_commit_cursor_surface(
        &socket_path,
        &commands,
        false,
        false,
        None,
    )
    .unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(result.renderable_count, 1);
}

#[test]
fn valid_cursor_surface_commit_exposes_hotspot_adjusted_overlay_snapshot() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let result = create_toplevel_then_set_and_commit_cursor_surface(
        &socket_path,
        &commands,
        false,
        false,
        None,
    );
    let _server = stop_controllable_test_server(commands, server_thread);

    let cursor = result
        .unwrap()
        .cursor
        .expect("valid committed cursor should have an overlay snapshot");
    assert_eq!(cursor.logical_x, render::FIRST_SURFACE_OFFSET.0 + 19);
    assert_eq!(cursor.logical_y, render::FIRST_SURFACE_OFFSET.1 + 13);
    assert_eq!((cursor.width, cursor.height), (24, 24));
}

#[test]
fn cursor_surface_null_attachment_removes_overlay_without_mapping_client_content() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let result = create_toplevel_then_set_and_commit_cursor_surface(
        &socket_path,
        &commands,
        true,
        false,
        None,
    );
    let _server = stop_controllable_test_server(commands, server_thread);

    let result = result.unwrap();
    assert_eq!(result.renderable_count, 1);
    assert_eq!(result.cursor, None);
}

#[test]
fn visible_cursor_surface_frame_callback_waits_for_frame_completion() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let result = create_toplevel_then_set_and_commit_cursor_surface(
        &socket_path,
        &commands,
        false,
        true,
        None,
    );
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(result.unwrap().callback_state, Some((true, false)));
}

#[test]
fn active_client_cursor_motion_advances_overlay_generation_without_mapping_surface() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let x = f64::from(render::FIRST_SURFACE_OFFSET.0) + 40.0;
    let y = f64::from(render::FIRST_SURFACE_OFFSET.1) + 30.0;

    let result = create_toplevel_then_set_and_commit_cursor_surface(
        &socket_path,
        &commands,
        false,
        false,
        Some((x, y)),
    );
    let _server = stop_controllable_test_server(commands, server_thread);

    let result = result.unwrap();
    assert_eq!(result.renderable_count, 1);
    assert_eq!(result.cause, RenderGenerationCause::CursorMotion);
    assert_eq!(result.cursor.unwrap().logical_x, x.round() as i32 - 1);
}

#[test]
fn client_cursor_hotspot_hide_reselect_and_destroy_transitions_are_isolated() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let snapshots = exercise_client_cursor_state_transitions(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    let initial = snapshots.initial.unwrap();
    let hotspot_changed = snapshots.hotspot_changed.unwrap();
    assert_eq!(hotspot_changed.surface_id, initial.surface_id);
    assert_eq!(hotspot_changed.logical_x, initial.logical_x - 4);
    assert_eq!(hotspot_changed.logical_y, initial.logical_y - 6);
    assert_eq!(snapshots.hidden, None);
    assert_eq!(snapshots.reselected.unwrap().surface_id, initial.surface_id);
    assert_eq!(snapshots.destroyed, None);
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
