use super::*;

#[test]
fn wayland_client_can_create_data_device_on_oblivion_server() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let result = create_client_data_device(&socket_path);
    stop_test_server(running, server_thread);

    result.unwrap();
}

#[test]
fn wayland_client_dmabuf_create_returns_buffer_for_advertised_format() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let state = create_dmabuf_candidate_and_expect_created(&socket_path);
    stop_test_server(running, server_thread);

    let state = state.unwrap();
    assert!(state.dmabuf_modifier);
    assert!(state.dmabuf_created);
    assert!(!state.dmabuf_failed);
}

#[test]
fn wayland_client_receives_dmabuf_v4_default_feedback() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let state = request_dmabuf_default_feedback(&socket_path);
    stop_test_server(running, server_thread);

    let state = state.unwrap();
    assert!(state.dmabuf_feedback_main_device);
    assert!(state.dmabuf_feedback_format_table);
    assert!(state.dmabuf_feedback_tranche_formats);
    assert!(state.dmabuf_feedback_done);
}

#[test]
fn wayland_client_receives_configured_renderer_dmabuf_feedback() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    let main_device = 0x1122_3344_5566_7788;
    let main_device_path = "/dev/dri/renderD128".to_string();
    server.set_dmabuf_feedback(
        EglGlesDmabufFeedback::from_formats([EglGlesDmabufFormat::new(
            DrmFormat::Xrgb8888,
            DrmModifier::LINEAR,
        )]),
        Some(main_device),
        Some(main_device_path),
    );
    assert_eq!(server.state.dmabuf_main_device, main_device);
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let state = request_dmabuf_default_feedback(&socket_path);
    stop_test_server(running, server_thread);

    let state = state.unwrap();
    assert!(state.dmabuf_feedback_done);
    assert_eq!(state.dmabuf_feedback_format_table_size, 16);
}

#[test]
fn dmabuf_feedback_replacement_clears_stale_main_device_identity() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    server.set_dmabuf_feedback(
        EglGlesDmabufFeedback::linear_argb_xrgb(),
        Some(0x1122_3344_5566_7788),
        Some("/dev/dri/renderD128".to_string()),
    );

    server.set_dmabuf_feedback(EglGlesDmabufFeedback::new(Vec::new()), None, None);

    assert_eq!(server.state.dmabuf_main_device, 0);
    assert_eq!(server.state.dmabuf_main_device_path, None);
    assert!(server.state.dmabuf_feedback.formats().is_empty());
}

#[test]
fn wayland_client_receives_wl_drm_compatibility_events() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    server.set_dmabuf_feedback(
        EglGlesDmabufFeedback::linear_argb_xrgb(),
        Some(0x1122_3344_5566_7788),
        Some("/dev/dri/renderD128".to_string()),
    );
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let state = request_wl_drm_capabilities(&socket_path);
    stop_test_server(running, server_thread);

    let state = state.unwrap();
    assert!(state.wl_drm_device);
    assert!(state.wl_drm_capabilities);
    assert!(state.wl_drm_format);
    assert!(state.wl_drm_authenticated);
}

#[test]
fn wl_drm_v1_bind_does_not_receive_capabilities() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    server.set_dmabuf_feedback(
        EglGlesDmabufFeedback::linear_argb_xrgb(),
        Some(0x1122_3344_5566_7788),
        Some("/dev/dri/renderD128".to_string()),
    );
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let state = request_wl_drm_at_version(&socket_path, 1);
    stop_test_server(running, server_thread);

    let state = state.unwrap();
    assert!(state.wl_drm_device);
    assert!(!state.wl_drm_capabilities);
    assert!(state.wl_drm_format);
    assert!(state.wl_drm_authenticated);
}

#[test]
fn wayland_client_receives_linux_drm_syncobj_global_when_device_supports_it() {
    if test_syncobj_device().is_none() {
        return;
    }

    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let globals = read_registry_globals(&socket_path);
    stop_test_server(running, server_thread);

    assert!(
        globals
            .unwrap()
            .iter()
            .any(|global| global == "wp_linux_drm_syncobj_manager_v1")
    );
}

#[test]
fn wayland_client_rejects_invalid_syncobj_timeline_fd() {
    if test_syncobj_device().is_none() {
        return;
    }

    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let result = import_invalid_syncobj_timeline(&socket_path);
    stop_test_server(running, server_thread);

    assert!(result.is_err());
}

#[test]
fn wayland_client_rejects_syncobj_point_after_surface_destroy() {
    let Some(timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };

    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let result = set_syncobj_acquire_after_surface_destroy(&socket_path, &timeline);
    stop_test_server(running, server_thread);

    assert!(result.is_err());
}

#[test]
fn wayland_client_syncobj_dmabuf_release_signals_release_point_after_present() {
    let Some(acquire_timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };
    let Some(release_timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };

    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_syncobj_dmabuf_surface_and_present(
        &socket_path,
        &commands,
        &acquire_timeline,
        &release_timeline,
    );
    let _server = stop_controllable_test_server(commands, server_thread);

    state.unwrap();
    assert!(release_timeline.point_signaled(2).unwrap());
}
