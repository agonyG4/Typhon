use super::*;

#[path = "layer_shell_lifecycle.rs"]
mod layer_shell_lifecycle;

fn connect_layer_client(
    socket_path: &PathBuf,
) -> (
    Connection,
    EventQueue<RegistryTestState>,
    QueueHandle<RegistryTestState>,
    client_wl_compositor::WlCompositor,
    client_wl_shm::WlShm,
    client_zwlr_layer_shell_v1::ZwlrLayerShellV1,
) {
    let stream = UnixStream::connect(socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    (connection, queue, qh, compositor, shm, layer_shell)
}

fn connect_layer_client_with_activation(
    socket_path: &PathBuf,
) -> (
    Connection,
    EventQueue<RegistryTestState>,
    QueueHandle<RegistryTestState>,
    client_wl_compositor::WlCompositor,
    client_wl_shm::WlShm,
    client_zwlr_layer_shell_v1::ZwlrLayerShellV1,
    client_xdg_activation_v1::XdgActivationV1,
) {
    let stream = UnixStream::connect(socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    let activation: client_xdg_activation_v1::XdgActivationV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    (
        connection,
        queue,
        qh,
        compositor,
        shm,
        layer_shell,
        activation,
    )
}

fn create_layer_surface(
    compositor: &client_wl_compositor::WlCompositor,
    layer_shell: &client_zwlr_layer_shell_v1::ZwlrLayerShellV1,
    qh: &QueueHandle<RegistryTestState>,
    layer: client_zwlr_layer_shell_v1::Layer,
    namespace: &str,
) -> (
    client_wl_surface::WlSurface,
    client_zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
) {
    let surface = compositor.create_surface(qh, ());
    let layer_surface =
        layer_shell.get_layer_surface(&surface, None, layer, namespace.to_string(), qh, ());
    (surface, layer_surface)
}

#[allow(clippy::too_many_arguments)]
fn create_mapped_layer_surface(
    connection: &Connection,
    queue: &mut EventQueue<RegistryTestState>,
    state: &mut RegistryTestState,
    compositor: &client_wl_compositor::WlCompositor,
    shm: &client_wl_shm::WlShm,
    layer_shell: &client_zwlr_layer_shell_v1::ZwlrLayerShellV1,
    qh: &QueueHandle<RegistryTestState>,
    layer: client_zwlr_layer_shell_v1::Layer,
    namespace: &str,
    width: usize,
    height: usize,
) -> (
    client_wl_surface::WlSurface,
    client_zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
) {
    let (surface, layer_surface) =
        create_layer_surface(compositor, layer_shell, qh, layer, namespace);
    layer_surface.set_size(width as u32, height as u32);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(state).unwrap();
    commit_test_buffered_surface(&surface, shm, qh, width, height).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(state).unwrap();
    (surface, layer_surface)
}

#[allow(clippy::too_many_arguments)]
fn create_layer_popup(
    connection: &Connection,
    queue: &mut EventQueue<RegistryTestState>,
    state: &mut RegistryTestState,
    compositor: &client_wl_compositor::WlCompositor,
    shm: &client_wl_shm::WlShm,
    wm_base: &client_xdg_wm_base::XdgWmBase,
    qh: &QueueHandle<RegistryTestState>,
    parent_layer: &client_zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    width: usize,
    height: usize,
) -> (client_wl_surface::WlSurface, client_xdg_popup::XdgPopup) {
    let popup_surface = compositor.create_surface(qh, ());
    let popup_xdg_surface = wm_base.get_xdg_surface(&popup_surface, qh, ());
    let positioner = wm_base.create_positioner(qh, ());
    positioner.set_size(width as i32, height as i32);
    positioner.set_anchor_rect(0, 0, 1, 1);
    let popup = popup_xdg_surface.get_popup(None, &positioner, qh, ());
    parent_layer.get_popup(&popup);
    popup_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(state).unwrap();
    commit_test_buffered_surface(&popup_surface, shm, qh, width, height).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(state).unwrap();
    (popup_surface, popup)
}

fn assert_layer_initial_configure(
    queue: &mut EventQueue<RegistryTestState>,
    state: &mut RegistryTestState,
    expected_width: u32,
    expected_height: u32,
) {
    queue.roundtrip(state).unwrap();
    assert_eq!(state.layer_surface_configure_count, 1);
    assert_eq!(state.layer_surface_width, expected_width);
    assert_eq!(state.layer_surface_height, expected_height);
}

#[test]
fn layer_shell_global_is_advertised_at_version_four_or_newer() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (running, server_thread) = spawn_test_server(server);

    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, _queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let layer_shell = globals
        .contents()
        .clone_list()
        .into_iter()
        .find(|global| global.interface == "zwlr_layer_shell_v1")
        .expect("zwlr_layer_shell_v1 global must be advertised");

    assert!(layer_shell.version >= 4);

    stop_test_server(running, server_thread);
}

#[test]
fn eclipse_overlay_gets_full_output_configure_then_maps_after_ack_and_buffer() {
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
    let surface = compositor.create_surface(&qh, ());
    let layer_surface = layer_shell.get_layer_surface(
        &surface,
        None,
        client_zwlr_layer_shell_v1::Layer::Overlay,
        "astrea-spotlight".to_string(),
        &qh,
        (),
    );
    layer_surface.set_anchor(
        client_zwlr_layer_surface_v1::Anchor::Top
            | client_zwlr_layer_surface_v1::Anchor::Bottom
            | client_zwlr_layer_surface_v1::Anchor::Left
            | client_zwlr_layer_surface_v1::Anchor::Right,
    );
    layer_surface.set_exclusive_zone(-1);
    layer_surface
        .set_keyboard_interactivity(client_zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive);
    surface.commit();
    connection.flush().unwrap();

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.layer_surface_configure_count, 1);
    assert_eq!(state.layer_surface_width, 1280);
    assert_eq!(state.layer_surface_height, 800);
    assert_eq!(capture_renderable_surface_count(&commands), 0);

    commit_test_buffered_surface(&surface, &shm, &qh, 1280, 800).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let surfaces = capture_renderable_surface_snapshot(&commands);
    assert_eq!(surfaces.len(), 1);
    assert_eq!(surfaces[0].local_x, 0);
    assert_eq!(surfaces[0].local_y, 0);
    assert_eq!(surfaces[0].width, 1280);
    assert_eq!(surfaces[0].height, 800);
    assert_eq!(
        capture_focused_surface_id(&commands),
        Some(surfaces[0].surface_id)
    );

    surface.attach(None, 0, 0);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_renderable_surface_count(&commands), 0);
    assert_eq!(capture_focused_surface_id(&commands), None);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn layer_roots_render_at_absolute_arranged_origin_without_cascade() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let (connection, mut queue, qh, compositor, shm, layer_shell) =
        connect_layer_client(&socket_path);
    let mut state = RegistryTestState::default();

    for namespace in ["absolute-a", "absolute-b", "absolute-c"] {
        let (surface, layer_surface) = create_layer_surface(
            &compositor,
            &layer_shell,
            &qh,
            client_zwlr_layer_shell_v1::Layer::Top,
            namespace,
        );
        layer_surface.set_anchor(
            client_zwlr_layer_surface_v1::Anchor::Top | client_zwlr_layer_surface_v1::Anchor::Left,
        );
        layer_surface.set_size(64, 32);
        surface.commit();
        connection.flush().unwrap();
        queue.roundtrip(&mut state).unwrap();
        commit_test_buffered_surface(&surface, &shm, &qh, 64, 32).unwrap();
        connection.flush().unwrap();
        queue.roundtrip(&mut state).unwrap();
    }

    let surfaces = capture_renderable_surface_snapshot(&commands);
    assert_eq!(surfaces.len(), 3);
    for surface in surfaces {
        assert_eq!((surface.local_x, surface.local_y), (0, 0));
        assert_eq!((surface.origin_x, surface.origin_y), (0, 0));
    }

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn quickshell_top_panel_reserves_usable_output_and_reconfigures_on_resize() {
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
    let surface = compositor.create_surface(&qh, ());
    let panel = layer_shell.get_layer_surface(
        &surface,
        None,
        client_zwlr_layer_shell_v1::Layer::Top,
        "quickshell-panel".to_string(),
        &qh,
        (),
    );
    panel.set_anchor(
        client_zwlr_layer_surface_v1::Anchor::Top
            | client_zwlr_layer_surface_v1::Anchor::Left
            | client_zwlr_layer_surface_v1::Anchor::Right,
    );
    panel.set_size(0, 32);
    panel.set_exclusive_zone(32);
    surface.commit();
    connection.flush().unwrap();

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.layer_surface_width, 1280);
    assert_eq!(state.layer_surface_height, 32);

    commit_test_buffered_surface(&surface, &shm, &qh, 1280, 32).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let usable = capture_usable_output_geometry(&commands);
    assert_eq!(usable.x, 0.0);
    assert_eq!(usable.y, 32.0);
    assert_eq!(usable.width, 1280.0);
    assert_eq!(usable.height, 768.0);

    commands
        .send(ServerCommand::SetOutputSize {
            width: 1600,
            height: 900,
        })
        .unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.layer_surface_width, 1600);
    assert_eq!(state.layer_surface_height, 32);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn zone_zero_top_surface_is_arranged_against_reserved_usable_area() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let (connection, mut queue, qh, compositor, shm, layer_shell) =
        connect_layer_client(&socket_path);

    let (panel_surface, panel) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "reserved-panel",
    );
    panel.set_anchor(
        client_zwlr_layer_surface_v1::Anchor::Top
            | client_zwlr_layer_surface_v1::Anchor::Left
            | client_zwlr_layer_surface_v1::Anchor::Right,
    );
    panel.set_size(0, 32);
    panel.set_exclusive_zone(32);
    panel_surface.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&panel_surface, &shm, &qh, 1280, 32).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let (surface, notification) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "zone-zero-top",
    );
    notification.set_anchor(
        client_zwlr_layer_surface_v1::Anchor::Top
            | client_zwlr_layer_surface_v1::Anchor::Left
            | client_zwlr_layer_surface_v1::Anchor::Right,
    );
    notification.set_size(0, 24);
    notification.set_exclusive_zone(0);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&surface, &shm, &qh, 1280, 24).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let surfaces = capture_renderable_surface_snapshot(&commands);
    assert_eq!(surfaces.len(), 2);
    assert_eq!(surfaces[0].local_y, 0);
    assert_eq!(surfaces[1].local_y, 32);
    assert_eq!(surfaces[1].height, 24);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn corner_anchored_positive_exclusive_zone_does_not_reserve_space() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let (connection, mut queue, qh, compositor, shm, layer_shell) =
        connect_layer_client(&socket_path);
    let (surface, corner) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "corner-exclusive",
    );
    corner.set_anchor(
        client_zwlr_layer_surface_v1::Anchor::Top | client_zwlr_layer_surface_v1::Anchor::Left,
    );
    corner.set_size(50, 50);
    corner.set_exclusive_zone(50);
    surface.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&surface, &shm, &qh, 50, 50).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let usable = capture_usable_output_geometry(&commands);
    assert_eq!(usable.x, 0.0);
    assert_eq!(usable.y, 0.0);
    assert_eq!(usable.width, 1280.0);
    assert_eq!(usable.height, 800.0);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn overlay_layer_renders_above_normal_xdg_window() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_client_toplevel_with_sized_shm_buffer(&socket_path, 300, 200).unwrap();
    assert_eq!(capture_renderable_surface_count(&commands), 1);

    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let overlay = layer_shell.get_layer_surface(
        &surface,
        None,
        client_zwlr_layer_shell_v1::Layer::Overlay,
        "astrea-alt-tab".to_string(),
        &qh,
        (),
    );
    overlay.set_anchor(
        client_zwlr_layer_surface_v1::Anchor::Top
            | client_zwlr_layer_surface_v1::Anchor::Bottom
            | client_zwlr_layer_surface_v1::Anchor::Left
            | client_zwlr_layer_surface_v1::Anchor::Right,
    );
    overlay.set_exclusive_zone(-1);
    surface.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&surface, &shm, &qh, 1280, 800).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let surfaces = capture_renderable_surface_snapshot(&commands);
    assert_eq!(surfaces.len(), 2);
    assert_eq!(surfaces[0].width, 300);
    assert_eq!(surfaces[0].height, 200);
    assert_eq!(surfaces[1].width, 1280);
    assert_eq!(surfaces[1].height, 800);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn exclusive_focus_arbitrates_latest_overlay_and_restores_application() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_client_toplevel_with_sized_shm_buffer(&socket_path, 300, 200).unwrap();
    let app_surface_id = capture_renderable_surface_snapshot(&commands)[0].surface_id;
    assert_eq!(capture_focused_surface_id(&commands), Some(app_surface_id));

    let (connection, mut queue, qh, compositor, shm, layer_shell) =
        connect_layer_client(&socket_path);
    let (spotlight_surface, spotlight) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Overlay,
        "astrea-spotlight",
    );
    spotlight.set_size(400, 300);
    spotlight
        .set_keyboard_interactivity(client_zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive);
    spotlight_surface.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&spotlight_surface, &shm, &qh, 400, 300).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let spotlight_surface_id = capture_renderable_surface_snapshot(&commands)[1].surface_id;
    assert_eq!(
        capture_focused_surface_id(&commands),
        Some(spotlight_surface_id)
    );

    let (alttab_surface, alttab) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Overlay,
        "astrea-alt-tab",
    );
    alttab.set_size(500, 350);
    alttab
        .set_keyboard_interactivity(client_zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive);
    alttab_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&alttab_surface, &shm, &qh, 500, 350).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let surfaces = capture_renderable_surface_snapshot(&commands);
    let alttab_surface_id = surfaces[2].surface_id;
    assert_eq!(
        capture_focused_surface_id(&commands),
        Some(alttab_surface_id)
    );

    spotlight_surface.attach(None, 0, 0);
    spotlight_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(
        capture_focused_surface_id(&commands),
        Some(alttab_surface_id)
    );

    alttab_surface.attach(None, 0, 0);
    alttab_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_focused_surface_id(&commands), Some(app_surface_id));

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn direct_wl_surface_destroy_restores_focus_from_exclusive_overlay() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_client_toplevel_with_sized_shm_buffer(&socket_path, 300, 200).unwrap();
    let app_surface_id = capture_renderable_surface_snapshot(&commands)[0].surface_id;

    let (connection, mut queue, qh, compositor, shm, layer_shell) =
        connect_layer_client(&socket_path);
    let (surface, overlay) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Overlay,
        "destroy-exclusive",
    );
    overlay.set_size(320, 240);
    overlay
        .set_keyboard_interactivity(client_zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive);
    surface.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&surface, &shm, &qh, 320, 240).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_ne!(capture_focused_surface_id(&commands), Some(app_surface_id));

    surface.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_renderable_surface_count(&commands), 1);
    assert_eq!(capture_focused_surface_id(&commands), Some(app_surface_id));

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn xdg_role_after_layer_surface_is_rejected() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (running, server_thread) = spawn_test_server(server);

    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let _layer = layer_shell.get_layer_surface(
        &surface,
        None,
        client_zwlr_layer_shell_v1::Layer::Top,
        "role-conflict".to_string(),
        &qh,
        (),
    );
    let _xdg = wm_base.get_xdg_surface(&surface, &qh, ());
    connection.flush().unwrap();

    let mut state = RegistryTestState::default();
    assert!(queue.roundtrip(&mut state).is_err());

    stop_test_server(running, server_thread);
}

#[test]
fn invalid_zero_width_without_opposite_horizontal_anchors_is_rejected() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (running, server_thread) = spawn_test_server(server);
    let (connection, mut queue, qh, compositor, _shm, layer_shell) =
        connect_layer_client(&socket_path);
    let (surface, layer_surface) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "invalid-width",
    );
    layer_surface.set_anchor(client_zwlr_layer_surface_v1::Anchor::Left);
    layer_surface.set_size(0, 32);
    surface.commit();
    connection.flush().unwrap();

    assert!(queue.roundtrip(&mut RegistryTestState::default()).is_err());
    stop_test_server(running, server_thread);
}

#[test]
fn invalid_zero_height_without_opposite_vertical_anchors_is_rejected() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (running, server_thread) = spawn_test_server(server);
    let (connection, mut queue, qh, compositor, _shm, layer_shell) =
        connect_layer_client(&socket_path);
    let (surface, layer_surface) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "invalid-height",
    );
    layer_surface.set_anchor(client_zwlr_layer_surface_v1::Anchor::Top);
    layer_surface.set_size(32, 0);
    surface.commit();
    connection.flush().unwrap();

    assert!(queue.roundtrip(&mut RegistryTestState::default()).is_err());
    stop_test_server(running, server_thread);
}

#[test]
fn zero_size_with_opposite_anchors_is_valid() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (running, server_thread) = spawn_test_server(server);
    let (connection, mut queue, qh, compositor, _shm, layer_shell) =
        connect_layer_client(&socket_path);
    let (surface, layer_surface) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "valid-zero-size",
    );
    layer_surface.set_anchor(
        client_zwlr_layer_surface_v1::Anchor::Top
            | client_zwlr_layer_surface_v1::Anchor::Bottom
            | client_zwlr_layer_surface_v1::Anchor::Left
            | client_zwlr_layer_surface_v1::Anchor::Right,
    );
    layer_surface.set_size(0, 0);
    surface.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    assert_layer_initial_configure(&mut queue, &mut state, 1280, 800);
    stop_test_server(running, server_thread);
}

#[test]
fn fixed_size_with_opposite_anchors_centers_without_margin_shift() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let (connection, mut queue, qh, compositor, shm, layer_shell) =
        connect_layer_client(&socket_path);
    let (surface, layer_surface) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "fixed-opposite-centers",
    );
    layer_surface.set_anchor(
        client_zwlr_layer_surface_v1::Anchor::Top
            | client_zwlr_layer_surface_v1::Anchor::Bottom
            | client_zwlr_layer_surface_v1::Anchor::Left
            | client_zwlr_layer_surface_v1::Anchor::Right,
    );
    layer_surface.set_margin(10, 20, 30, 40);
    layer_surface.set_size(100, 50);
    surface.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&surface, &shm, &qh, 100, 50).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let surfaces = capture_renderable_surface_snapshot(&commands);
    assert_eq!((surfaces[0].local_x, surfaces[0].local_y), (590, 375));
    assert_eq!((surfaces[0].origin_x, surfaces[0].origin_y), (590, 375));

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn zero_size_with_opposite_anchors_stretches_using_margins() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let (connection, mut queue, qh, compositor, shm, layer_shell) =
        connect_layer_client(&socket_path);
    let (surface, layer_surface) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "zero-opposite-stretches",
    );
    layer_surface.set_anchor(
        client_zwlr_layer_surface_v1::Anchor::Top
            | client_zwlr_layer_surface_v1::Anchor::Bottom
            | client_zwlr_layer_surface_v1::Anchor::Left
            | client_zwlr_layer_surface_v1::Anchor::Right,
    );
    layer_surface.set_margin(10, 20, 30, 40);
    layer_surface.set_size(0, 0);
    surface.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(
        (state.layer_surface_width, state.layer_surface_height),
        (1220, 760)
    );
    commit_test_buffered_surface(&surface, &shm, &qh, 1220, 760).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let surfaces = capture_renderable_surface_snapshot(&commands);
    assert_eq!((surfaces[0].local_x, surfaces[0].local_y), (40, 10));
    assert_eq!((surfaces[0].width, surfaces[0].height), (1220, 760));

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn buffer_commit_before_initial_configure_is_rejected() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (running, server_thread) = spawn_test_server(server);
    let (connection, mut queue, qh, compositor, shm, layer_shell) =
        connect_layer_client(&socket_path);
    let (surface, layer_surface) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "pre-configure-buffer",
    );
    layer_surface.set_anchor(
        client_zwlr_layer_surface_v1::Anchor::Top
            | client_zwlr_layer_surface_v1::Anchor::Left
            | client_zwlr_layer_surface_v1::Anchor::Right,
    );
    layer_surface.set_size(0, 32);
    commit_test_buffered_surface(&surface, &shm, &qh, 1280, 32).unwrap();
    connection.flush().unwrap();

    assert!(queue.roundtrip(&mut RegistryTestState::default()).is_err());
    stop_test_server(running, server_thread);
}

#[test]
fn buffer_commit_before_configure_ack_is_rejected() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (running, server_thread) = spawn_test_server(server);
    let (connection, mut queue, qh, compositor, shm, layer_shell) =
        connect_layer_client(&socket_path);
    let (surface, layer_surface) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "pre-ack-buffer",
    );
    layer_surface.set_anchor(
        client_zwlr_layer_surface_v1::Anchor::Top
            | client_zwlr_layer_surface_v1::Anchor::Left
            | client_zwlr_layer_surface_v1::Anchor::Right,
    );
    layer_surface.set_size(0, 32);
    surface.commit();
    connection.flush().unwrap();
    // Flush the configure from the server but intentionally do not dispatch it,
    // so the client has not sent ack_configure.
    connection.roundtrip().unwrap();
    commit_test_buffered_surface(&surface, &shm, &qh, 1280, 32).unwrap();
    connection.flush().unwrap();

    assert!(queue.roundtrip(&mut RegistryTestState::default()).is_err());
    stop_test_server(running, server_thread);
}

#[test]
fn stale_ack_after_reconfigure_cannot_authorize_mapping() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let (connection, mut queue, qh, compositor, shm, layer_shell) =
        connect_layer_client(&socket_path);
    let (surface, layer_surface) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "stale-ack",
    );
    layer_surface.set_anchor(
        client_zwlr_layer_surface_v1::Anchor::Top
            | client_zwlr_layer_surface_v1::Anchor::Left
            | client_zwlr_layer_surface_v1::Anchor::Right,
    );
    layer_surface.set_size(0, 32);
    surface.commit();
    connection.flush().unwrap();

    let mut state = RegistryTestState {
        suppress_layer_surface_ack: true,
        ..RegistryTestState::default()
    };
    queue.roundtrip(&mut state).unwrap();
    let stale_serial = state.layer_surface_configure_serials[0];
    layer_surface.ack_configure(stale_serial);
    commit_test_buffered_surface(&surface, &shm, &qh, 1280, 32).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_renderable_surface_count(&commands), 1);

    commands
        .send(ServerCommand::SetOutputSize {
            width: 1600,
            height: 900,
        })
        .unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.layer_surface_configure_count, 2);
    layer_surface.ack_configure(stale_serial);
    commit_test_buffered_surface(&surface, &shm, &qh, 1600, 32).unwrap();
    connection.flush().unwrap();

    assert!(queue.roundtrip(&mut state).is_err());
    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn remap_after_null_buffer_requires_fresh_configure_ack() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (running, server_thread) = spawn_test_server(server);
    let (connection, mut queue, qh, compositor, shm, layer_shell) =
        connect_layer_client(&socket_path);
    let (surface, layer_surface) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "remap-lifecycle",
    );
    layer_surface.set_anchor(
        client_zwlr_layer_surface_v1::Anchor::Top
            | client_zwlr_layer_surface_v1::Anchor::Left
            | client_zwlr_layer_surface_v1::Anchor::Right,
    );
    layer_surface.set_size(0, 32);
    surface.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&surface, &shm, &qh, 1280, 32).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    surface.attach(None, 0, 0);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    commit_test_buffered_surface(&surface, &shm, &qh, 1280, 32).unwrap();
    connection.flush().unwrap();
    assert!(queue.roundtrip(&mut state).is_err());
    stop_test_server(running, server_thread);
}

#[test]
fn unmapping_exclusive_panel_rearranges_remaining_layer_surfaces() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let (connection, mut queue, qh, compositor, shm, layer_shell) =
        connect_layer_client(&socket_path);
    let mut state = RegistryTestState::default();

    let (panel_surface, panel) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "reserved-panel",
    );
    panel.set_anchor(
        client_zwlr_layer_surface_v1::Anchor::Top
            | client_zwlr_layer_surface_v1::Anchor::Left
            | client_zwlr_layer_surface_v1::Anchor::Right,
    );
    panel.set_size(0, 32);
    panel.set_exclusive_zone(32);
    panel_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&panel_surface, &shm, &qh, 1280, 32).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let (notification_surface, notification) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "notification",
    );
    notification.set_anchor(
        client_zwlr_layer_surface_v1::Anchor::Top
            | client_zwlr_layer_surface_v1::Anchor::Left
            | client_zwlr_layer_surface_v1::Anchor::Right,
    );
    notification.set_size(0, 24);
    notification_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&notification_surface, &shm, &qh, 1280, 24).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(
        capture_renderable_surface_snapshot(&commands)[1].local_y,
        32
    );

    panel_surface.attach(None, 0, 0);
    panel_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let surfaces = capture_renderable_surface_snapshot(&commands);
    assert_eq!(surfaces.len(), 1);
    assert_eq!(surfaces[0].local_y, 0);
    assert_eq!(capture_usable_output_geometry(&commands).y, 0.0);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn ondemand_layer_surface_maps_without_focus_steal_and_click_focuses() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_client_toplevel_with_sized_shm_buffer(&socket_path, 300, 200).unwrap();
    let app_surface_id = capture_renderable_surface_snapshot(&commands)[0].surface_id;
    assert_eq!(capture_focused_surface_id(&commands), Some(app_surface_id));

    let (connection, mut queue, qh, compositor, shm, layer_shell) =
        connect_layer_client(&socket_path);
    let (surface, layer_surface) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Overlay,
        "ondemand",
    );
    layer_surface.set_anchor(
        client_zwlr_layer_surface_v1::Anchor::Top | client_zwlr_layer_surface_v1::Anchor::Left,
    );
    layer_surface.set_size(200, 120);
    layer_surface
        .set_keyboard_interactivity(client_zwlr_layer_surface_v1::KeyboardInteractivity::OnDemand);
    surface.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&surface, &shm, &qh, 200, 120).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let layer_surface_id = capture_renderable_surface_snapshot(&commands)[1].surface_id;
    assert_eq!(capture_focused_surface_id(&commands), Some(app_surface_id));

    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0 + render::SURFACE_CASCADE_STEP) + 10.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1 + render::SURFACE_CASCADE_STEP) + 10.0,
        })
        .unwrap();
    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    assert_eq!(
        capture_focused_surface_id(&commands),
        Some(layer_surface_id)
    );

    surface.attach(None, 0, 0);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_focused_surface_id(&commands), Some(app_surface_id));

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn xdg_activation_focuses_mapped_ondemand_layer_surface() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_client_toplevel_with_sized_shm_buffer(&socket_path, 300, 200).unwrap();
    let app_surface_id = capture_renderable_surface_snapshot(&commands)[0].surface_id;
    assert_eq!(capture_focused_surface_id(&commands), Some(app_surface_id));

    let (connection, mut queue, qh, compositor, shm, layer_shell, activation) =
        connect_layer_client_with_activation(&socket_path);
    let (surface, layer_surface) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Overlay,
        "ondemand-activation",
    );
    layer_surface.set_anchor(
        client_zwlr_layer_surface_v1::Anchor::Top | client_zwlr_layer_surface_v1::Anchor::Left,
    );
    layer_surface.set_size(200, 120);
    layer_surface
        .set_keyboard_interactivity(client_zwlr_layer_surface_v1::KeyboardInteractivity::OnDemand);
    surface.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&surface, &shm, &qh, 200, 120).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let layer_surface_id = capture_renderable_surface_snapshot(&commands)[1].surface_id;
    assert_eq!(capture_focused_surface_id(&commands), Some(app_surface_id));

    let activation_token = activation.get_activation_token(&qh, ());
    activation_token.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let token = state
        .activation_token_done
        .take()
        .expect("activation token should be committed");
    activation.activate(token, &surface);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    assert_eq!(
        capture_focused_surface_id(&commands),
        Some(layer_surface_id)
    );

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn set_layer_reorders_only_after_surface_commit() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_client_toplevel_with_sized_shm_buffer(&socket_path, 300, 200).unwrap();
    let (connection, mut queue, qh, compositor, shm, layer_shell) =
        connect_layer_client(&socket_path);
    let (surface, layer_surface) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "set-layer",
    );
    layer_surface.set_size(200, 120);
    surface.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&surface, &shm, &qh, 200, 120).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let before = capture_renderable_surface_snapshot(&commands);
    let app_surface_id = before[0].surface_id;
    let layer_surface_id = before[1].surface_id;

    layer_surface.set_layer(client_zwlr_layer_shell_v1::Layer::Bottom);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let pending = capture_renderable_surface_snapshot(&commands);
    assert_eq!(pending[0].surface_id, app_surface_id);
    assert_eq!(pending[1].surface_id, layer_surface_id);

    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let committed = capture_renderable_surface_snapshot(&commands);
    assert_eq!(committed[0].surface_id, layer_surface_id);
    assert_eq!(committed[1].surface_id, app_surface_id);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn arranged_geometry_drives_configure_after_left_reservation() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let (connection, mut queue, qh, compositor, shm, layer_shell) =
        connect_layer_client(&socket_path);
    let mut state = RegistryTestState::default();

    let (left_surface, left_panel) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "left-reservation",
    );
    left_panel.set_anchor(
        client_zwlr_layer_surface_v1::Anchor::Left
            | client_zwlr_layer_surface_v1::Anchor::Top
            | client_zwlr_layer_surface_v1::Anchor::Bottom,
    );
    left_panel.set_size(100, 0);
    left_panel.set_exclusive_zone(100);
    left_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&left_surface, &shm, &qh, 100, 800).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let (top_surface, top_panel) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "top-after-left",
    );
    top_panel.set_anchor(
        client_zwlr_layer_surface_v1::Anchor::Top
            | client_zwlr_layer_surface_v1::Anchor::Left
            | client_zwlr_layer_surface_v1::Anchor::Right,
    );
    top_panel.set_size(0, 32);
    top_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    assert_eq!(state.layer_surface_width, 1180);
    assert_eq!(state.layer_surface_height, 32);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn reservation_removal_expands_and_reconfigures_affected_surface() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let (connection, mut queue, qh, compositor, shm, layer_shell) =
        connect_layer_client(&socket_path);
    let mut state = RegistryTestState::default();

    let (left_surface, left_panel) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "left-reservation-remove",
    );
    left_panel.set_anchor(
        client_zwlr_layer_surface_v1::Anchor::Left
            | client_zwlr_layer_surface_v1::Anchor::Top
            | client_zwlr_layer_surface_v1::Anchor::Bottom,
    );
    left_panel.set_size(100, 0);
    left_panel.set_exclusive_zone(100);
    left_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&left_surface, &shm, &qh, 100, 800).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let (top_surface, top_panel) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "top-expands",
    );
    top_panel.set_anchor(
        client_zwlr_layer_surface_v1::Anchor::Top
            | client_zwlr_layer_surface_v1::Anchor::Left
            | client_zwlr_layer_surface_v1::Anchor::Right,
    );
    top_panel.set_size(0, 32);
    top_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.layer_surface_width, 1180);
    let configure_count = state.layer_surface_configure_count;
    commit_test_buffered_surface(&top_surface, &shm, &qh, 1180, 32).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    left_surface.attach(None, 0, 0);
    left_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    assert!(state.layer_surface_configure_count > configure_count);
    assert_eq!(state.layer_surface_width, 1280);
    assert_eq!(state.layer_surface_height, 32);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn placement_only_margin_change_does_not_send_duplicate_size_configure() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let (connection, mut queue, qh, compositor, shm, layer_shell) =
        connect_layer_client(&socket_path);
    let (surface, layer_surface) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "margin-placement-only",
    );
    layer_surface.set_anchor(client_zwlr_layer_surface_v1::Anchor::Top);
    layer_surface.set_size(200, 32);
    surface.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&surface, &shm, &qh, 200, 32).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let configure_count = state.layer_surface_configure_count;

    layer_surface.set_margin(12, 0, 0, 0);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.layer_surface_configure_count, configure_count);
    assert_eq!(
        capture_renderable_surface_snapshot(&commands)[0].local_y,
        12
    );

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn acking_older_configure_uses_its_exact_geometry_snapshot() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let (connection, mut queue, qh, compositor, shm, layer_shell) =
        connect_layer_client(&socket_path);
    let (surface, layer_surface) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "snapshot-ack",
    );
    layer_surface.set_anchor(
        client_zwlr_layer_surface_v1::Anchor::Top
            | client_zwlr_layer_surface_v1::Anchor::Left
            | client_zwlr_layer_surface_v1::Anchor::Right,
    );
    layer_surface.set_size(0, 32);
    surface.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState {
        suppress_layer_surface_ack: true,
        ..RegistryTestState::default()
    };
    queue.roundtrip(&mut state).unwrap();
    let (serial_a, width_a, height_a) = state.layer_surface_configures[0];
    assert_eq!((width_a, height_a), (1280, 32));

    commands
        .send(ServerCommand::SetOutputSize {
            width: 900,
            height: 700,
        })
        .unwrap();
    queue.roundtrip(&mut state).unwrap();
    let (_serial_b, width_b, height_b) = state.layer_surface_configures[1];
    assert_eq!((width_b, height_b), (900, 32));

    layer_surface.ack_configure(serial_a);
    commit_test_buffered_surface(&surface, &shm, &qh, width_a as usize, height_a as usize).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let surfaces = capture_renderable_surface_snapshot(&commands);
    assert_eq!(surfaces[0].width, width_a);
    assert_eq!(surfaces[0].height, height_a);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn newer_configure_does_not_clear_unconsumed_acked_snapshot() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let (connection, mut queue, qh, compositor, shm, layer_shell) =
        connect_layer_client(&socket_path);
    let (surface, layer_surface) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "acked-before-newer-configure",
    );
    layer_surface.set_anchor(
        client_zwlr_layer_surface_v1::Anchor::Top
            | client_zwlr_layer_surface_v1::Anchor::Left
            | client_zwlr_layer_surface_v1::Anchor::Right,
    );
    layer_surface.set_size(0, 32);
    surface.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState {
        suppress_layer_surface_ack: true,
        ..RegistryTestState::default()
    };
    queue.roundtrip(&mut state).unwrap();
    let (serial_a, width_a, height_a) = state.layer_surface_configures[0];
    layer_surface.ack_configure(serial_a);
    connection.flush().unwrap();
    connection.roundtrip().unwrap();

    commands
        .send(ServerCommand::SetOutputSize {
            width: 900,
            height: 700,
        })
        .unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.layer_surface_configures.len(), 2);
    assert_eq!(state.layer_surface_configures[1].1, 900);

    commit_test_buffered_surface(&surface, &shm, &qh, width_a as usize, height_a as usize).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let surfaces = capture_renderable_surface_snapshot(&commands);
    assert_eq!(surfaces.len(), 1);
    assert_eq!(surfaces[0].width, width_a);
    assert_eq!(surfaces[0].height, height_a);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn mapped_layer_surface_can_commit_old_buffer_while_new_configure_is_pending() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let (connection, mut queue, qh, compositor, shm, layer_shell) =
        connect_layer_client(&socket_path);
    let mut state = RegistryTestState::default();
    let (surface, layer_surface) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "mapped-old-buffer-before-new-ack",
    );
    layer_surface.set_anchor(
        client_zwlr_layer_surface_v1::Anchor::Top
            | client_zwlr_layer_surface_v1::Anchor::Left
            | client_zwlr_layer_surface_v1::Anchor::Right,
    );
    layer_surface.set_size(0, 32);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&surface, &shm, &qh, 1280, 32).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    state.suppress_layer_surface_ack = true;
    commands
        .send(ServerCommand::SetOutputSize {
            width: 900,
            height: 700,
        })
        .unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.layer_surface_width, 900);
    commit_test_buffered_surface(&surface, &shm, &qh, 1280, 32).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    assert_eq!(capture_renderable_surface_count(&commands), 1);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn acking_latest_configure_maps_and_repaints_without_stale_configure_error() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let (connection, mut queue, qh, compositor, shm, layer_shell) =
        connect_layer_client(&socket_path);
    let (surface, layer_surface) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "quickshell-latest-ack-repaint",
    );
    layer_surface.set_anchor(
        client_zwlr_layer_surface_v1::Anchor::Top
            | client_zwlr_layer_surface_v1::Anchor::Left
            | client_zwlr_layer_surface_v1::Anchor::Right,
    );
    layer_surface.set_size(0, 32);
    surface.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState {
        suppress_layer_surface_ack: true,
        ..RegistryTestState::default()
    };
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.layer_surface_configures[0].1, 1280);

    commands
        .send(ServerCommand::SetOutputSize {
            width: 900,
            height: 700,
        })
        .unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.layer_surface_configures.len(), 2);
    let (serial_b, width_b, height_b) = state.layer_surface_configures[1];
    assert_eq!((width_b, height_b), (900, 32));

    layer_surface.ack_configure(serial_b);
    commit_test_buffered_surface(&surface, &shm, &qh, width_b as usize, height_b as usize).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_renderable_surface_count(&commands), 1);

    commit_test_buffered_surface(&surface, &shm, &qh, width_b as usize, height_b as usize).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_renderable_surface_count(&commands), 1);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}
