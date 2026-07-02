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
fn default_registry_hides_color_management_until_renderer_supports_transforms() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let result = read_registry_globals(&socket_path);
    stop_test_server(running, server_thread);

    let globals = result.unwrap();
    assert!(!globals.contains(&"wp_color_manager_v1".to_string()));
}

#[test]
fn default_registry_advertises_core_clipboard_only() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let result = read_registry_globals(&socket_path);
    stop_test_server(running, server_thread);

    let globals = result.unwrap();
    assert!(globals.contains(&"wl_data_device_manager".to_string()));
    assert!(!globals.contains(&"zwp_primary_selection_device_manager_v1".to_string()));
    assert!(!globals.contains(&"ext_data_control_manager_v1".to_string()));
}

#[test]
fn clipboard_ready_registry_advertises_only_core_clipboard_selection() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_with_selection_capabilities(
        &socket_name,
        SelectionProtocolCapabilities {
            clipboard: true,
            primary_selection: false,
            data_control: false,
        },
    )
    .unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let result = read_registry_globals(&socket_path);
    stop_test_server(running, server_thread);

    let globals = result.unwrap();
    assert!(globals.contains(&"wl_data_device_manager".to_string()));
    assert!(!globals.contains(&"zwp_primary_selection_device_manager_v1".to_string()));
    assert!(!globals.contains(&"ext_data_control_manager_v1".to_string()));
}

#[test]
fn native_and_cpu_base_registries_share_core_clipboard_policy() {
    let cpu_socket_name = unique_socket_name();
    let cpu_server = OwnCompositorServer::bind_cpu_composition(&cpu_socket_name).unwrap();
    let cpu_socket_path = runtime_socket_path(&cpu_socket_name);
    let (cpu_running, cpu_server_thread) = spawn_test_server(cpu_server);

    let cpu_result = read_registry_globals(&cpu_socket_path);
    stop_test_server(cpu_running, cpu_server_thread);

    let native_socket_name = unique_socket_name();
    let native_server = OwnCompositorServer::bind_native_base(&native_socket_name).unwrap();
    let native_socket_path = runtime_socket_path(&native_socket_name);
    let (native_running, native_server_thread) = spawn_test_server(native_server);

    let native_result = read_registry_globals(&native_socket_path);
    stop_test_server(native_running, native_server_thread);

    for globals in [cpu_result.unwrap(), native_result.unwrap()] {
        assert!(globals.contains(&"wl_data_device_manager".to_string()));
        assert!(!globals.contains(&"zwp_primary_selection_device_manager_v1".to_string()));
        assert!(!globals.contains(&"ext_data_control_manager_v1".to_string()));
    }
}

#[test]
fn default_registry_does_not_publish_duplicate_globals() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let result = read_registry_globals(&socket_path);
    stop_test_server(running, server_thread);

    let globals = result.unwrap();
    for name in &globals {
        assert_eq!(
            globals
                .iter()
                .filter(|candidate| *candidate == name)
                .count(),
            1,
            "duplicated global {name}"
        );
    }
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
fn deferred_renderer_registry_omits_gpu_buffer_globals_until_renderer_enables_them() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_with_capabilities(
        &socket_name,
        false,
        InputProtocolCapabilities::nested_winit(),
        SelectionProtocolCapabilities::core_clipboard(),
        RendererProtocolCapabilities::unsupported(),
    )
    .unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let result = read_registry_globals(&socket_path);
    stop_test_server(running, server_thread);

    let globals = result.unwrap();
    assert!(globals.contains(&"wl_shm".to_string()));
    assert!(!globals.contains(&"zwp_linux_dmabuf_v1".to_string()));
    assert!(!globals.contains(&"wp_linux_drm_syncobj_manager_v1".to_string()));
    assert!(!globals.contains(&"wl_drm".to_string()));

    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind_with_capabilities(
        &socket_name,
        false,
        InputProtocolCapabilities::nested_winit(),
        SelectionProtocolCapabilities::core_clipboard(),
        RendererProtocolCapabilities::unsupported(),
    )
    .unwrap();
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

    let state = create_surface_with_presentation_feedback_and_present(
        &socket_path,
        &commands,
        ServerCommand::PresentFrame,
    )
    .unwrap();
    stop_controllable_test_server(commands, server_thread);

    assert_eq!(state.presentation_discarded_count, 0);
    assert_eq!(state.presentation_presented_count, 1);
    assert_eq!(
        state.presentation_kind,
        Some(client_wp_presentation_feedback::Kind::empty())
    );
}

#[test]
fn presentation_feedback_uses_injected_kernel_metadata_once() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    server.set_presentation_clock(PresentationClock::Monotonic);
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let presentation = FramePresentation::synchronized(
        PresentationClock::Monotonic,
        0xfedc_ba98,
        999_999,
        u32::MAX,
    )
    .unwrap();

    let state = create_surface_with_presentation_feedback_and_present(
        &socket_path,
        &commands,
        ServerCommand::FinishFrameWithPresentation(presentation),
    )
    .unwrap();
    stop_controllable_test_server(commands, server_thread);

    assert_eq!(
        state.presentation_clock_id,
        Some(libc::CLOCK_MONOTONIC as u32)
    );
    assert_eq!(
        state.presentation_timestamp,
        Some((0, 0xfedc_ba98, 999_999_000))
    );
    assert_eq!(state.presentation_sequence, Some((0, u32::MAX)));
    assert_eq!(state.presentation_presented_count, 1);
    assert_eq!(
        state.presentation_kind,
        Some(client_wp_presentation_feedback::Kind::Vsync)
    );
}

#[test]
fn presentation_global_advertises_configured_realtime_clock() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    server.set_presentation_clock(PresentationClock::Realtime);
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let presentation =
        FramePresentation::synchronized(PresentationClock::Realtime, 1, 0, 1).unwrap();

    let state = create_surface_with_presentation_feedback_and_present(
        &socket_path,
        &commands,
        ServerCommand::FinishFrameWithPresentation(presentation),
    )
    .unwrap();
    stop_controllable_test_server(commands, server_thread);

    assert_eq!(
        state.presentation_clock_id,
        Some(libc::CLOCK_REALTIME as u32)
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

#[test]
fn abrupt_client_disconnect_removes_mapped_toplevel_surface() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    server.enable_cleanup_live_disconnected_test_clients();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let client = LiveTestClient::connect(&socket_path).unwrap();
    let surface = client
        .create_toplevel_surface("oblivion.disconnect-test", 64, 48)
        .unwrap();
    let surface_id = surface.id().protocol_id();
    wait_for_server_commands(&commands);
    assert_eq!(capture_renderable_surface_count(&commands), 1);
    assert!(
        capture_xdg_role_snapshot(&commands, surface_id).toplevel_registered,
        "mapped toplevel should be registered before disconnect"
    );

    drop(surface);
    drop(client);
    thread::sleep(Duration::from_millis(20));
    wait_for_server_commands(&commands);

    assert_eq!(capture_renderable_surface_count(&commands), 0);
    assert!(!capture_xdg_role_snapshot(&commands, surface_id).surface_registered);
    assert!(!capture_xdg_role_snapshot(&commands, surface_id).toplevel_registered);
    assert_eq!(capture_focused_surface_id(&commands), None);

    stop_controllable_test_server(commands, server_thread);
}
