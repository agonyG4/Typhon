use super::*;

#[test]
fn dominant_fullscreen_culls_chrome_keeps_overlay_and_shares_input_policy() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_buffered_toplevel_then_toggle_fullscreen(&socket_path, &commands).unwrap();
    let owner_id = capture_fullscreen_render_plan_metrics(&commands)
        .owner_root_surface_id
        .expect("fullscreen owner should be registered");

    let (connection, mut queue, qh, compositor, shm, layer_shell) =
        connect_layer_client(&socket_path);
    let mut client_state = RegistryTestState::default();
    let (_background_surface, _) = create_mapped_layer_surface(
        &connection,
        &mut queue,
        &mut client_state,
        &compositor,
        &shm,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Background,
        "fullscreen-background",
        1280,
        800,
    );
    let (_bottom_surface, _) = create_mapped_layer_surface(
        &connection,
        &mut queue,
        &mut client_state,
        &compositor,
        &shm,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Bottom,
        "fullscreen-bottom",
        1280,
        32,
    );

    let (top_surface, top) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "fullscreen-top",
    );
    top.set_anchor(
        client_zwlr_layer_surface_v1::Anchor::Top
            | client_zwlr_layer_surface_v1::Anchor::Left
            | client_zwlr_layer_surface_v1::Anchor::Right,
    );
    top.set_size(0, 32);
    top_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut client_state).unwrap();
    commit_test_buffered_surface(&top_surface, &shm, &qh, 1280, 32).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut client_state).unwrap();

    let (overlay_surface, overlay) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Overlay,
        "fullscreen-overlay",
    );
    overlay.set_anchor(
        client_zwlr_layer_surface_v1::Anchor::Top | client_zwlr_layer_surface_v1::Anchor::Left,
    );
    overlay.set_size(100, 100);
    overlay.set_exclusive_zone(-1);
    overlay_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut client_state).unwrap();
    commit_test_buffered_surface(&overlay_surface, &shm, &qh, 100, 100).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut client_state).unwrap();

    commands
        .send(ServerCommand::PublishTestPresentationAt {
            frame_id: 1,
            at: AnimationTime::from_nanos(u64::MAX),
        })
        .unwrap();
    wait_for_server_commands(&commands);

    let metrics = capture_fullscreen_render_plan_metrics(&commands);
    let presented = capture_native_frame_surface_ids(&commands);
    let layer_snapshots = capture_renderable_surface_snapshot(&commands);
    let background_id = layer_snapshots
        .iter()
        .find(|surface| {
            surface.parent_surface_id.is_none()
                && surface.width == 1280
                && surface.height == 800
                && surface.surface_id != owner_id
        })
        .expect("background layer should be renderable")
        .surface_id;
    let overlay_id = layer_snapshots
        .iter()
        .find(|surface| {
            surface.parent_surface_id.is_none() && surface.width == 100 && surface.height == 100
        })
        .expect("overlay layer should be renderable")
        .surface_id;
    let top_id = layer_snapshots
        .iter()
        .find(|surface| {
            surface.parent_surface_id.is_none()
                && surface.width == 1280
                && surface.height == 32
                && surface.local_y == 0
        })
        .expect("Top layer should be renderable")
        .surface_id;
    let bottom_id = layer_snapshots
        .iter()
        .find(|surface| {
            surface.parent_surface_id.is_none()
                && surface.width == 1280
                && surface.height == 32
                && surface.surface_id != top_id
        })
        .expect("Bottom layer should be renderable")
        .surface_id;
    assert!(metrics.fullscreen_composition_active);
    assert!(!metrics.solitary_tree_active);
    assert_eq!(metrics.fullscreen_allowed_layer_roots, 1);
    assert_eq!(metrics.fullscreen_culled_layer_roots, 3);
    assert!(presented.contains(&owner_id));
    assert!(presented.contains(&overlay_id));
    assert!(!presented.contains(&background_id));
    assert!(!presented.contains(&bottom_id));
    assert!(!presented.contains(&top_id));

    commands
        .send(ServerCommand::PointerMotion { x: 200.0, y: 16.0 })
        .unwrap();
    wait_for_server_commands(&commands);
    assert_eq!(capture_pointer_focus_surface_id(&commands), Some(owner_id));
    commands
        .send(ServerCommand::PointerMotion { x: 50.0, y: 16.0 })
        .unwrap();
    wait_for_server_commands(&commands);
    assert_eq!(
        capture_pointer_focus_surface_id(&commands),
        Some(overlay_id)
    );

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn fullscreen_culled_background_feedback_waits_for_sampled_frame() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_buffered_toplevel_then_toggle_fullscreen(&socket_path, &commands).unwrap();
    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    let presentation: client_wp_presentation::WpPresentation =
        globals.bind(&qh, 1..=2, ()).unwrap();
    let mut client_state = RegistryTestState::default();
    let (background_surface, _) = create_mapped_layer_surface(
        &connection,
        &mut queue,
        &mut client_state,
        &compositor,
        &shm,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Background,
        "fullscreen-feedback-background",
        1280,
        800,
    );
    let feedback_a = presentation.feedback(&background_surface, &qh, ());
    commit_test_buffered_surface(&background_surface, &shm, &qh, 1280, 800).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut client_state).unwrap();
    let feedback_b = presentation.feedback(&background_surface, &qh, ());
    commit_test_buffered_surface(&background_surface, &shm, &qh, 1280, 800).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut client_state).unwrap();

    let (overlay_surface, overlay) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Overlay,
        "fullscreen-feedback-overlay",
    );
    overlay.set_anchor(
        client_zwlr_layer_surface_v1::Anchor::Top | client_zwlr_layer_surface_v1::Anchor::Left,
    );
    overlay.set_size(100, 100);
    overlay.set_exclusive_zone(-1);
    overlay_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut client_state).unwrap();
    let overlay_feedback = presentation.feedback(&overlay_surface, &qh, ());
    commit_test_buffered_surface(&overlay_surface, &shm, &qh, 100, 100).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut client_state).unwrap();
    wait_for_server_commands(&commands);

    commands
        .send(ServerCommand::PublishTestPresentationAt {
            frame_id: 1,
            at: AnimationTime::from_nanos(u64::MAX),
        })
        .unwrap();
    wait_for_server_commands(&commands);
    let native_ids = capture_native_frame_surface_ids(&commands);
    let snapshots = capture_renderable_surface_snapshot(&commands);
    let background_id = snapshots
        .iter()
        .find(|surface| {
            surface.parent_surface_id.is_none() && surface.width == 1280 && surface.height == 800
        })
        .expect("background layer should be renderable")
        .surface_id;
    assert!(!native_ids.contains(&background_id));

    let (batch_reply, batch_receiver) = std::sync::mpsc::channel();
    commands
        .send(ServerCommand::CaptureNativeFrameBatch {
            frame_id: 2,
            reply: batch_reply,
        })
        .unwrap();
    let batch_id = batch_receiver.recv_timeout(Duration::from_secs(1)).unwrap();
    let (surface_ids_reply, surface_ids_receiver) = std::sync::mpsc::channel();
    commands
        .send(ServerCommand::CaptureFrameBatchSurfaceIds {
            batch_id,
            reply: surface_ids_reply,
        })
        .unwrap();
    let batch_surface_ids = surface_ids_receiver
        .recv_timeout(Duration::from_secs(1))
        .unwrap();
    let owns_background = batch_surface_ids.contains(&background_id);
    commands
        .send(ServerCommand::CompleteFrameBatchNow {
            frame_id: 2,
            batch_id,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut client_state).unwrap();
    assert!(!capture_frame_eligible_presentation_feedback_work(
        &commands
    ));

    commands
        .send(ServerCommand::ToggleFullscreenFocused)
        .unwrap();
    wait_for_server_commands(&commands);
    commands
        .send(ServerCommand::PublishTestPresentationAt {
            frame_id: 3,
            at: AnimationTime::from_nanos(u64::MAX),
        })
        .unwrap();
    wait_for_server_commands(&commands);
    assert!(capture_native_frame_surface_ids(&commands).contains(&background_id));

    let (retry_reply, retry_receiver) = std::sync::mpsc::channel();
    commands
        .send(ServerCommand::CaptureNativeFrameBatch {
            frame_id: 3,
            reply: retry_reply,
        })
        .unwrap();
    let retry_batch = retry_receiver.recv_timeout(Duration::from_secs(1)).unwrap();
    commands
        .send(ServerCommand::CompleteFrameBatchNow {
            frame_id: 3,
            batch_id: retry_batch,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut client_state).unwrap();
    let presented_count = client_state.presentation_presented_count;
    let discarded_count = client_state.presentation_discarded_count;
    assert!(
        client_state
            .presentation_feedback_event_log
            .contains(&(overlay_feedback.id().protocol_id(), "presented"))
    );

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();

    assert!(
        !owns_background,
        "culled background must not be captured by the frame"
    );
    assert_eq!(presented_count, 2);
    assert_eq!(discarded_count, 1);
    assert!(
        client_state
            .presentation_feedback_event_log
            .contains(&(feedback_a.id().protocol_id(), "discarded"))
    );
    assert!(
        client_state
            .presentation_feedback_event_log
            .contains(&(feedback_b.id().protocol_id(), "presented"))
    );
}
