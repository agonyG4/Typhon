use super::*;

#[test]
fn layer_popup_renders_above_top_parent_and_gets_input_first() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let _pointer = seat.get_pointer(&qh, ());
    let mut state = RegistryTestState::default();

    let (_parent_surface, parent_layer) = create_mapped_layer_surface(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "popup-parent-top",
        200,
        120,
    );
    let (_popup_surface, _popup) = create_layer_popup(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &wm_base,
        &qh,
        &parent_layer,
        60,
        40,
    );
    let surfaces = capture_renderable_surface_snapshot(&commands);
    assert_eq!(surfaces.len(), 2);
    let popup_surface_id = surfaces[1].surface_id;
    assert_eq!(surfaces[1].surface_id, popup_surface_id);
    assert_eq!(surfaces[1].parent_surface_id, Some(surfaces[0].surface_id));
    let popup_output_x = surfaces[1].origin_x + 5;
    let popup_output_y = surfaces[1].origin_y + 5;

    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(popup_output_x),
            y: f64::from(popup_output_y),
        })
        .unwrap();
    wait_for_server_commands(&commands);
    assert_eq!(
        capture_pointer_focus_surface_id(&commands),
        Some(popup_surface_id)
    );

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn duplicate_layer_popup_association_is_rejected() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (running, server_thread) = spawn_test_server(server);
    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let mut state = RegistryTestState::default();
    let (_parent_surface, parent_layer) = create_mapped_layer_surface(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "popup-parent-duplicate",
        200,
        120,
    );
    let popup_surface = compositor.create_surface(&qh, ());
    let popup_xdg_surface = wm_base.get_xdg_surface(&popup_surface, &qh, ());
    let positioner = wm_base.create_positioner(&qh, ());
    positioner.set_size(60, 40);
    positioner.set_anchor_rect(0, 0, 1, 1);
    let popup = popup_xdg_surface.get_popup(None, &positioner, &qh, ());
    parent_layer.get_popup(&popup);
    parent_layer.get_popup(&popup);
    connection.flush().unwrap();

    assert!(queue.roundtrip(&mut state).is_err());
    stop_test_server(running, server_thread);
}

#[test]
fn layer_parent_unmap_destroy_and_popup_destroy_cleanup_popup_state() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let mut state = RegistryTestState::default();
    let (parent_surface, parent_layer) = create_mapped_layer_surface(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "popup-parent-unmap",
        200,
        120,
    );
    let (_popup_surface, popup) = create_layer_popup(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &wm_base,
        &qh,
        &parent_layer,
        60,
        40,
    );
    assert_eq!(capture_renderable_surface_count(&commands), 2);

    popup.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_renderable_surface_count(&commands), 1);

    let (_popup_surface, _popup) = create_layer_popup(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &wm_base,
        &qh,
        &parent_layer,
        60,
        40,
    );
    assert_eq!(capture_renderable_surface_count(&commands), 2);
    parent_surface.attach(None, 0, 0);
    parent_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_renderable_surface_count(&commands), 0);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn layer_parent_unmap_dismisses_popup_but_late_popup_destroy_is_idempotent() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let mut state = RegistryTestState::default();

    let (parent_surface, parent_layer) = create_mapped_layer_surface(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "popup-parent-unmap",
        200,
        80,
    );
    let (popup_surface, popup) = create_layer_popup(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &wm_base,
        &qh,
        &parent_layer,
        64,
        40,
    );
    let popup_surface_id = popup_surface.id().protocol_id();
    assert_eq!(capture_renderable_surface_count(&commands), 2);

    parent_surface.attach(None, 0, 0);
    parent_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let snapshot = capture_xdg_role_snapshot(&commands, popup_surface_id);
    assert_eq!(snapshot.popup_count, 1);
    assert_eq!(snapshot.popup_node_count, 1);
    assert_eq!(capture_renderable_surface_count(&commands), 0);

    popup.destroy();
    popup_surface.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let snapshot = capture_xdg_role_snapshot(&commands, popup_surface_id);
    assert_eq!(snapshot.popup_count, 0);
    assert_eq!(snapshot.popup_node_count, 0);
    assert!(!snapshot.popup_grab_active);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn parent_layer_change_restacks_popup_with_parent() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    create_client_toplevel_with_sized_shm_buffer(&socket_path, 300, 200).unwrap();
    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let mut state = RegistryTestState::default();
    let (parent_surface, parent_layer) = create_mapped_layer_surface(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "popup-parent-layer-change",
        200,
        120,
    );
    let (_popup_surface, _popup) = create_layer_popup(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &wm_base,
        &qh,
        &parent_layer,
        60,
        40,
    );
    let popup_surface_id = capture_renderable_surface_snapshot(&commands)
        .last()
        .map(|surface| surface.surface_id)
        .unwrap();
    assert_eq!(
        capture_renderable_surface_snapshot(&commands)
            .last()
            .map(|surface| surface.surface_id),
        Some(popup_surface_id)
    );

    parent_layer.set_layer(client_zwlr_layer_shell_v1::Layer::Bottom);
    parent_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let surfaces = capture_renderable_surface_snapshot(&commands);
    assert_eq!(surfaces[1].surface_id, popup_surface_id);
    assert_eq!(surfaces[2].width, 300);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn layer_surface_shm_render_unmap_and_remap_use_normal_scene_lifecycle() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let (connection, mut queue, qh, compositor, shm, layer_shell) =
        connect_layer_client(&socket_path);
    let mut state = RegistryTestState::default();
    let (surface, layer_surface) = create_mapped_layer_surface(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "layer-shm-render",
        64,
        32,
    );
    assert_eq!(capture_renderable_surface_count(&commands), 1);

    surface.attach(None, 0, 0);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_renderable_surface_count(&commands), 0);

    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&surface, &shm, &qh, 64, 32).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_renderable_surface_count(&commands), 1);
    layer_surface.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_renderable_surface_count(&commands), 0);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn layer_surface_frame_callback_and_presentation_feedback_publish_normally() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
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
    let mut state = RegistryTestState::default();
    let (surface, _layer_surface) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Overlay,
        "layer-frame-presentation",
    );
    _layer_surface.set_size(32, 32);
    surface.frame(&qh, ());
    presentation.feedback(&surface, &qh, ());
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&surface, &shm, &qh, 32, 32).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    commands.send(ServerCommand::PresentFrame).unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert!(state.frame_done);
    assert_eq!(state.presentation_presented_count, 1);
    assert_eq!(state.presentation_discarded_count, 0);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn layer_surface_dmabuf_import_and_reuse_publish_normally() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 =
        globals.bind(&qh, 3..=3, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    let first_buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff11_1111).unwrap();
    let second_buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff22_2222).unwrap();
    let (surface, _layer_surface) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "layer-dmabuf",
    );
    _layer_surface.set_size(2, 2);
    surface.set_buffer_scale(1);
    surface.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();

    surface.attach(Some(&first_buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let first_snapshot = capture_renderable_surface_snapshot(&commands);
    assert_eq!(first_snapshot.len(), 1);

    surface.attach(Some(&second_buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let second_snapshot = capture_renderable_surface_snapshot(&commands);
    assert_eq!(second_snapshot.len(), 1);
    assert_ne!(first_snapshot[0].buffer_id, second_snapshot[0].buffer_id);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn layer_surface_explicit_sync_waits_then_publishes_and_destroy_pending_is_safe() {
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
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 =
        globals.bind(&qh, 3..=3, ()).unwrap();
    let syncobj: client_wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    let (surface, _layer_surface) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "layer-explicit-sync",
    );
    _layer_surface.set_size(2, 2);
    let sync_surface = syncobj.get_surface(&surface, &qh, ());
    let acquire_timeline_fd = acquire_timeline.export_timeline_fd().unwrap();
    let release_timeline_fd = release_timeline.export_timeline_fd().unwrap();
    let sync_acquire_timeline = syncobj.import_timeline(acquire_timeline_fd.as_fd(), &qh, ());
    let sync_release_timeline = syncobj.import_timeline(release_timeline_fd.as_fd(), &qh, ());
    let first_buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff44_4444).unwrap();
    let second_buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff55_5555).unwrap();
    surface.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();

    acquire_timeline.signal_point(1).unwrap();
    sync_surface.set_acquire_point(&sync_acquire_timeline, 0, 1);
    sync_surface.set_release_point(&sync_release_timeline, 0, 2);
    surface.attach(Some(&first_buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_renderable_surface_count(&commands), 1);

    sync_surface.set_acquire_point(&sync_acquire_timeline, 0, 3);
    sync_surface.set_release_point(&sync_release_timeline, 0, 4);
    surface.attach(Some(&second_buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let blocked = capture_renderable_surface_snapshot(&commands);
    assert_eq!(blocked.len(), 1);
    acquire_timeline.signal_point(3).unwrap();
    commands.send(ServerCommand::PresentFrame).unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let published = capture_renderable_surface_snapshot(&commands);
    assert_ne!(blocked[0].buffer_id, published[0].buffer_id);

    sync_surface.set_acquire_point(&sync_acquire_timeline, 0, 5);
    sync_surface.set_release_point(&sync_release_timeline, 0, 6);
    surface.attach(Some(&first_buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    surface.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_renderable_surface_count(&commands), 0);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn layer_root_publishes_synchronized_subsurface_transaction() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let subcompositor: client_wl_subcompositor::WlSubcompositor =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    let mut state = RegistryTestState::default();
    let (parent, _layer_surface) = create_mapped_layer_surface(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "layer-subsurface",
        64,
        32,
    );
    let child = compositor.create_surface(&qh, ());
    let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    subsurface.set_position(10, 12);
    commit_test_buffered_surface(&child, &shm, &qh, 16, 8).unwrap();
    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let surfaces = capture_renderable_surface_snapshot(&commands);
    assert_eq!(surfaces.len(), 2);
    assert_eq!(surfaces[1].parent_surface_id, Some(surfaces[0].surface_id));
    assert_eq!(surfaces[1].local_x, 10);
    assert_eq!(surfaces[1].local_y, 12);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn arrangement_change_advances_render_generation_without_buffer_special_case() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let (connection, mut queue, qh, compositor, shm, layer_shell) =
        connect_layer_client(&socket_path);
    let mut state = RegistryTestState::default();
    let (surface, layer_surface) = create_mapped_layer_surface(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "layer-arrangement-damage",
        200,
        32,
    );
    let before = capture_render_generation(&commands);
    let before_y = capture_renderable_surface_snapshot(&commands)[0].local_y;

    layer_surface.set_anchor(client_zwlr_layer_surface_v1::Anchor::Top);
    layer_surface.set_margin(12, 0, 0, 0);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let after = capture_render_generation(&commands);
    let after_y = capture_renderable_surface_snapshot(&commands)[0].local_y;
    assert!(after > before);
    assert_ne!(before_y, after_y);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}
