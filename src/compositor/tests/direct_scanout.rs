use super::*;

#[test]
fn fullscreen_identity_viewport_xrgb_dmabuf_is_direct_scanout_candidate() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_fullscreen_identity_viewport_xrgb_dmabuf(&socket_path, &commands).unwrap();
    assert!(state.toplevel_has_state(client_xdg_toplevel::State::Fullscreen));
    let eligibility = capture_fullscreen_presentation_eligibility(&commands);
    assert!(eligibility.eligible);
    let candidate = capture_direct_scanout_candidate(&commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(candidate.buffer_size, BufferSize::new(1280, 800).unwrap());
    assert_eq!(candidate.output_size, BufferSize::new(1280, 800).unwrap());
    assert_eq!(candidate.buffer_size, candidate.output_size);
    assert_ne!(candidate.surface_id, 0);
    assert_eq!(candidate.surface_id, candidate.root_surface_id);
    assert!(candidate.generation > 0);
    assert!(candidate.commit_sequence.get() > 0);
    assert!(candidate.viewport_identity_metadata_present);
}

#[test]
fn fullscreen_cropped_viewport_is_rejected_before_direct_scanout_import() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let _state = create_fullscreen_viewport_xrgb_dmabuf(
        &socket_path,
        &commands,
        Some((1.0 / 256.0, 0.0, 1280.0, 800.0)),
        Some((1280, 800)),
    )
    .unwrap();
    let rejection = capture_direct_scanout_candidate(&commands).unwrap_err();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(
        rejection,
        DirectScanoutSceneRejection::ViewportSourceNonIdentity
    );
}

#[test]
fn fullscreen_scaled_viewport_is_rejected_before_direct_scanout_import() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let _state =
        create_fullscreen_viewport_xrgb_dmabuf(&socket_path, &commands, None, Some((1279, 800)))
            .unwrap();
    set_focused_root_visual_geometry(
        &commands,
        SurfacePlacement::absolute_root_at(0, 0),
        1280,
        800,
    );
    let rejection = capture_direct_scanout_candidate(&commands).unwrap_err();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(
        rejection,
        DirectScanoutSceneRejection::ViewportDestinationNonIdentity
    );
}
