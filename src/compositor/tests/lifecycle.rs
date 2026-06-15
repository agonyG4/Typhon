use super::*;

#[test]
fn wayland_client_can_read_minimum_registry_globals() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let result = read_registry_globals(&socket_path);
    stop_test_server(running, server_thread);

    let globals = result.unwrap();
    for expected in CompositorPlan::new(socket_name).protocol_names() {
        assert!(
            globals.contains(&expected.to_string()),
            "missing {expected}"
        );
    }
}

#[test]
fn cpu_composition_registry_omits_gpu_buffer_globals() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let result = read_registry_globals(&socket_path);
    stop_test_server(running, server_thread);

    let globals = result.unwrap();
    assert!(globals.contains(&"wl_shm".to_string()));
    assert!(!globals.contains(&"zwp_linux_dmabuf_v1".to_string()));
    assert!(!globals.contains(&"wp_linux_drm_syncobj_manager_v1".to_string()));
    assert!(!globals.contains(&"wl_drm".to_string()));
}

#[test]
fn default_registry_omits_unsupported_gaming_protocol_stubs() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let result = read_registry_globals(&socket_path);
    stop_test_server(running, server_thread);

    let globals = result.unwrap();
    assert!(!globals.contains(&"zwp_relative_pointer_manager_v1".to_string()));
    assert!(!globals.contains(&"zwp_pointer_constraints_v1".to_string()));
    assert!(!globals.contains(&"zwp_idle_inhibit_manager_v1".to_string()));
}

#[test]
fn native_base_registry_can_publish_gpu_buffer_globals_after_backend_is_known() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind_native_base(&socket_name).unwrap();
    server.enable_gpu_buffer_protocols();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let result = read_registry_globals(&socket_path);
    stop_test_server(running, server_thread);

    let globals = result.unwrap();
    assert!(globals.contains(&"zwp_linux_dmabuf_v1".to_string()));
    assert!(globals.contains(&"wl_drm".to_string()));
}

#[test]
fn presentation_feedback_request_does_not_panic_server_tick() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let client = request_presentation_feedback(&socket_path).unwrap();
    thread::sleep(Duration::from_millis(20));
    running.store(false, Ordering::Relaxed);
    let result = server_thread.join();
    drop(client);

    assert!(result.is_ok());
}

#[test]
fn presentation_feedback_for_committed_buffer_is_presented_on_present_frame() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state =
        create_surface_with_presentation_feedback_and_present(&socket_path, &commands).unwrap();
    stop_controllable_test_server(commands, server_thread);

    assert_eq!(state.presentation_discarded_count, 0);
    assert_eq!(state.presentation_presented_count, 1);
    assert_eq!(
        state.presentation_kind,
        Some(
            client_wp_presentation_feedback::Kind::Vsync
                | client_wp_presentation_feedback::Kind::HwClock
        )
    );
}

#[test]
fn server_reports_pending_frame_work_for_presentation_feedback_until_present_frame() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_surface_with_unpresented_presentation_feedback(&socket_path).unwrap();
    wait_for_server_commands(&commands);

    assert!(capture_pending_frame_work(&commands));
    commands.send(ServerCommand::PresentFrame).unwrap();
    wait_for_server_commands(&commands);
    assert!(!capture_pending_frame_work(&commands));

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn committed_surfaces_from_distinct_clients_are_tracked_separately() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let count = create_two_live_client_toplevels_and_capture_surface_count(&socket_path, &commands)
        .unwrap();
    stop_controllable_test_server(commands, server_thread);

    assert_eq!(count, 2);
}
