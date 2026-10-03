use super::*;

#[test]
fn non_occluding_fullscreen_preserves_underlays_without_exposing_input() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let (owner_surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 1280, 800).unwrap();
    owner_surface.commit();
    connection.flush().unwrap();
    let mut owner_state = RegistryTestState::default();
    queue.roundtrip(&mut owner_state).unwrap();
    commands
        .send(ServerCommand::ToggleFullscreenFocused)
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut owner_state).unwrap();
    let owner_id = capture_fullscreen_render_plan_metrics(&commands)
        .owner_root_surface_id
        .expect("fullscreen owner should be registered");
    commands
        .send(ServerCommand::SetTestEffectiveXdgWindowGeometry {
            root_surface_id: owner_id,
            geometry: XdgWindowGeometry::new(0, 0, 1280, 800),
        })
        .unwrap();
    wait_for_server_commands(&commands);
    assert_eq!(
        capture_effective_xdg_window_geometry(&commands, owner_id),
        Some(XdgWindowGeometry::new(0, 0, 1280, 800))
    );
    assert!(capture_root_window_geometry(&commands, owner_id).is_some());
    retain_live_test_connection(connection);
    commands
        .send(ServerCommand::CancelRootPresentationProperties {
            root_surface_id: owner_id,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    let eligibility = capture_fullscreen_presentation_eligibility(&commands);
    assert!(eligibility.exactly_covers_output);
    assert!(!eligibility.fully_opaque);

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
    let direct_scanout_analysis = capture_direct_scanout_scene_analysis(&commands);
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
    let owner_snapshot = layer_snapshots
        .iter()
        .find(|surface| surface.surface_id == owner_id)
        .expect("fullscreen owner should be renderable");
    assert!(metrics.fullscreen_composition_active);
    assert!(!metrics.solitary_tree_active);
    assert_eq!(metrics.fullscreen_allowed_layer_roots, 1);
    assert_eq!(metrics.fullscreen_culled_layer_roots, 1);
    assert!(presented.contains(&owner_id));
    assert!(presented.contains(&overlay_id));
    assert!(presented.contains(&background_id));
    assert!(presented.contains(&bottom_id));
    assert!(!presented.contains(&top_id));
    assert!(!metrics.wallpaper_culled);
    assert!(direct_scanout_analysis.candidate.is_none());
    assert!(
        direct_scanout_analysis
            .blockers
            .reasons()
            .contains(&DirectScanoutSceneRejection::FullscreenUnderlayVisible)
    );

    let (owner_origin_x, owner_origin_y) = owner_snapshot
        .active_scene_origin
        .expect("fullscreen owner should have an active scene origin");
    let (owner_width, owner_height) = owner_snapshot
        .active_scene_size
        .expect("fullscreen owner should have active scene geometry");
    let pointer_x = f64::from(owner_origin_x + i32::try_from(owner_width / 2).unwrap());
    let pointer_y = f64::from(owner_origin_y + i32::try_from(owner_height / 2).unwrap());
    commands
        .send(ServerCommand::PointerMotion {
            x: pointer_x,
            y: pointer_y,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    let pointer_focus = capture_pointer_focus_surface_id(&commands);
    assert_eq!(
        pointer_focus,
        Some(owner_id),
        "pointer over the preserved background should remain on the fullscreen owner; owner={owner_snapshot:?}, input={:?}, metrics={metrics:?}",
        capture_pointer_input_metrics(&commands)
    );
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
fn proven_opaque_fullscreen_still_culls_background_and_bottom() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let _owner = create_fullscreen_identity_viewport_xrgb_dmabuf(&socket_path, &commands).unwrap();
    let owner_id = capture_fullscreen_render_plan_metrics(&commands)
        .owner_root_surface_id
        .expect("fullscreen owner should be registered");
    commands
        .send(ServerCommand::SetTestEffectiveXdgWindowGeometry {
            root_surface_id: owner_id,
            geometry: XdgWindowGeometry::new(0, 0, 1280, 800),
        })
        .unwrap();
    wait_for_server_commands(&commands);
    assert_eq!(
        capture_effective_xdg_window_geometry(&commands, owner_id),
        Some(XdgWindowGeometry::new(0, 0, 1280, 800))
    );
    assert!(capture_root_window_geometry(&commands, owner_id).is_some());
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
        "opaque-fullscreen-background",
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
        "opaque-fullscreen-bottom",
        1280,
        32,
    );
    commands
        .send(ServerCommand::CancelRootPresentationProperties {
            root_surface_id: owner_id,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    commands
        .send(ServerCommand::PublishTestPresentationAt {
            frame_id: 1,
            at: AnimationTime::from_nanos(u64::MAX),
        })
        .unwrap();
    wait_for_server_commands(&commands);

    let eligibility = capture_fullscreen_presentation_eligibility(&commands);
    let metrics = capture_fullscreen_render_plan_metrics(&commands);
    let presented = capture_native_frame_surface_ids(&commands);
    let surfaces = capture_renderable_surface_snapshot(&commands);
    let direct_scanout_analysis = capture_direct_scanout_scene_analysis(&commands);
    let background_id = surfaces
        .iter()
        .find(|surface| {
            surface.parent_surface_id.is_none()
                && surface.width == 1280
                && surface.height == 800
                && surface.surface_id != owner_id
        })
        .expect("background layer should be renderable")
        .surface_id;
    let bottom_id = surfaces
        .iter()
        .find(|surface| {
            surface.parent_surface_id.is_none() && surface.width == 1280 && surface.height == 32
        })
        .expect("Bottom layer should be renderable")
        .surface_id;

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();

    assert!(eligibility.fully_opaque);
    assert!(metrics.fullscreen_composition_active);
    assert!(
        metrics.solitary_tree_active,
        "opaque fullscreen should have no composition underlays: metrics={metrics:?}, scanout_blockers={:?}",
        direct_scanout_analysis.blockers.reasons()
    );
    assert!(metrics.wallpaper_culled);
    assert_eq!(metrics.fullscreen_culled_layer_roots, 2);
    assert!(!presented.contains(&background_id));
    assert!(!presented.contains(&bottom_id));
}

#[test]
fn fullscreen_preserved_background_feedback_is_sampled_with_its_frame() {
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
    assert!(native_ids.contains(&background_id));

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
    assert!(
        client_state
            .presentation_feedback_event_log
            .contains(&(feedback_b.id().protocol_id(), "presented"))
    );
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
        owns_background,
        "preserved background must be captured by the frame"
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
