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

