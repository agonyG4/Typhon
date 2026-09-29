use super::*;

struct ActiveWireDrag {
    connection: Connection,
    queue: EventQueue<RegistryTestState>,
    state: RegistryTestState,
    origin: client_wl_surface::WlSurface,
    icon: client_wl_surface::WlSurface,
    _origin_xdg: client_xdg_surface::XdgSurface,
    _origin_toplevel: client_xdg_toplevel::XdgToplevel,
    icon_shm: client_wl_shm::WlShm,
    _pointer: client_wl_pointer::WlPointer,
    device: client_wl_data_device::WlDataDevice,
}

impl ActiveWireDrag {
    fn start_again(&mut self, commands: &Sender<ServerCommand>) {
        commands
            .send(ServerCommand::PointerButton {
                button: 0x110,
                pressed: false,
            })
            .unwrap();
        wait_for_server_commands(commands);
        self.queue.roundtrip(&mut self.state).unwrap();
        commands
            .send(ServerCommand::PointerButton {
                button: 0x110,
                pressed: true,
            })
            .unwrap();
        wait_for_server_commands(commands);
        self.queue.roundtrip(&mut self.state).unwrap();
        let serial = self
            .state
            .pointer_button_serial
            .expect("replacement drag must use a fresh pointer press serial");
        self.device
            .start_drag(None, &self.origin, Some(&self.icon), serial);
        self.connection.flush().unwrap();
        wait_for_server_commands(commands);
        self.queue.roundtrip(&mut self.state).unwrap();
    }
}

struct OverlayLayerClient {
    _connection: Connection,
    _surface: client_wl_surface::WlSurface,
    _layer_surface: client_zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
}

fn begin_source_less_wire_drag(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<ActiveWireDrag, Box<dyn std::error::Error>> {
    let connection = Connection::from_socket(UnixStream::connect(socket_path)?)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let pointer = seat.get_pointer(&qh, ());
    let manager: client_wl_data_device_manager::WlDataDeviceManager =
        globals.bind(&qh, 1..=3, ())?;
    let device = manager.get_data_device(&seat, &qh, ());
    let (origin, origin_xdg, origin_toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120)?;
    let icon = compositor.create_surface(&qh, ());

    origin.commit();
    connection.flush()?;
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commit_registered_initial_xdg_test_buffer(&origin_xdg);
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    commit_test_buffered_surface(&icon, &shm, &qh, 21, 15)?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;

    focus_root_window(commands, origin.id().protocol_id());
    set_focused_root_visual_geometry(
        commands,
        SurfacePlacement::absolute_root_at(80, 60),
        160,
        120,
    );
    let pointer_position = (100.0, 80.0);
    commands.send(ServerCommand::PointerMotion {
        x: pointer_position.0,
        y: pointer_position.1,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerButton {
        button: 0x110,
        pressed: true,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    let serial = state
        .pointer_button_serial
        .expect("drag must use a real pointer press serial");

    device.start_drag(None, &origin, Some(&icon), serial);
    connection.flush()?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    Ok(ActiveWireDrag {
        connection,
        queue,
        state,
        origin,
        icon,
        _origin_xdg: origin_xdg,
        _origin_toplevel: origin_toplevel,
        icon_shm: shm,
        _pointer: pointer,
        device,
    })
}

fn create_overlay_layer_client(
    socket_path: &PathBuf,
) -> Result<OverlayLayerClient, Box<dyn std::error::Error>> {
    let connection = Connection::from_socket(UnixStream::connect(socket_path)?)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 = globals.bind(&qh, 4..=4, ())?;
    let surface = compositor.create_surface(&qh, ());
    let layer_surface = layer_shell.get_layer_surface(
        &surface,
        None,
        client_zwlr_layer_shell_v1::Layer::Overlay,
        "drag-icon-overlay-test".to_string(),
        &qh,
        (),
    );
    layer_surface.set_anchor(
        client_zwlr_layer_surface_v1::Anchor::Top | client_zwlr_layer_surface_v1::Anchor::Left,
    );
    layer_surface.set_size(100, 100);
    layer_surface.set_exclusive_zone(-1);
    surface.commit();
    connection.flush()?;
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commit_test_buffered_surface(&surface, &shm, &qh, 100, 100)?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;

    Ok(OverlayLayerClient {
        _connection: connection,
        _surface: surface,
        _layer_surface: layer_surface,
    })
}

fn settle_fullscreen_presentation(commands: &Sender<ServerCommand>) {
    let owner_root_surface_id = capture_fullscreen_render_plan_metrics(commands)
        .owner_root_surface_id
        .expect("fullscreen owner should be registered");
    commands
        .send(ServerCommand::CancelRootPresentationProperties {
            root_surface_id: owner_root_surface_id,
        })
        .unwrap();
    commands
        .send(ServerCommand::PublishTestPresentationAt {
            frame_id: 1,
            at: crate::presentation_animation::AnimationTime::from_nanos(u64::MAX),
        })
        .unwrap();
    wait_for_server_commands(commands);
}

#[test]
fn active_drag_icon_orders_above_application_and_layer_overlay() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let _drag = begin_source_less_wire_drag(&socket_path, &commands).unwrap();
    let _overlay = create_overlay_layer_client(&socket_path).unwrap();
    wait_for_server_commands(&commands);
    let surfaces = capture_renderable_surface_snapshot(&commands);
    let application_position = surfaces
        .iter()
        .position(|surface| (surface.width, surface.height) == (160, 120));
    let overlay_position = surfaces
        .iter()
        .position(|surface| (surface.width, surface.height) == (100, 100));
    let icon_position = surfaces
        .iter()
        .position(|surface| (surface.width, surface.height) == (21, 15));
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(
        application_position.is_some(),
        "origin missing from scene: {surfaces:?}"
    );
    assert!(
        overlay_position.is_some(),
        "overlay missing from scene: {surfaces:?}"
    );
    assert!(
        icon_position.is_some(),
        "icon missing from scene: {surfaces:?}"
    );
    let application_position = application_position.unwrap();
    let overlay_position = overlay_position.unwrap();
    let icon_position = icon_position.unwrap();

    assert!(application_position < overlay_position);
    assert!(
        overlay_position < icon_position,
        "the active DragIcon must be ordered above a layer-shell overlay"
    );
}

#[test]
fn active_drag_icon_survives_dominant_fullscreen_composition() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let _drag = begin_source_less_wire_drag(&socket_path, &commands).unwrap();
    let icon_id = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .find(|surface| (surface.width, surface.height) == (21, 15))
        .expect("active DragIcon must remain in the raw scene")
        .surface_id;
    let _fullscreen =
        create_fullscreen_identity_viewport_xrgb_dmabuf(&socket_path, &commands).unwrap();
    settle_fullscreen_presentation(&commands);
    let metrics = capture_fullscreen_render_plan_metrics(&commands);
    let presented_ids = capture_native_frame_surface_ids(&commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(metrics.fullscreen_composition_active);
    assert_eq!(
        metrics.fullscreen_above_reason,
        Some(FullscreenAboveFullscreenReason::DragIcon)
    );
    assert!(
        presented_ids.contains(&icon_id),
        "a visible DragIcon must remain in dominant fullscreen composition"
    );
}

#[test]
fn active_drag_icon_rejects_direct_scanout_with_overlay_visible() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let _drag = begin_source_less_wire_drag(&socket_path, &commands).unwrap();
    let _fullscreen =
        create_fullscreen_identity_viewport_xrgb_dmabuf(&socket_path, &commands).unwrap();
    settle_fullscreen_presentation(&commands);
    let result = capture_direct_scanout_candidate(&commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(
        result.unwrap_err(),
        DirectScanoutSceneRejection::OverlayVisible
    );
}

#[test]
fn cancelling_drag_hides_icon_and_preserves_its_permanent_role() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let mut drag = begin_source_less_wire_drag(&socket_path, &commands).unwrap();
    let initial_icon = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .find(|surface| (surface.width, surface.height) == (21, 15))
        .expect("active DragIcon must be visible before cancellation");
    let icon_id = initial_icon.surface_id;
    cancel_active_drag_for_test(&commands);
    let icon_still_rendered = capture_renderable_surface_snapshot(&commands)
        .iter()
        .any(|surface| surface.surface_id == icon_id);
    let active_icon = capture_active_drag_icon_surface(&commands);
    let role_state = capture_surface_role_state(&commands, icon_id);
    assert!(
        !icon_still_rendered,
        "terminal drag cleanup must hide its icon"
    );
    assert_eq!(
        active_icon, None,
        "terminal drag cleanup must clear active icon ownership"
    );
    assert_eq!(role_state, ("drag_icon".to_string(), false));

    drag.start_again(&commands);
    let reactivated_icon = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .find(|surface| surface.surface_id == icon_id)
        .expect("same-role DragIcon reuse must adopt retained committed content");
    let active_role_state = capture_surface_role_state(&commands, icon_id);
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(reactivated_icon.buffer_id, initial_icon.buffer_id);
    assert_eq!(active_role_state, ("drag_icon".to_string(), true));
}

#[test]
fn null_attachment_hides_active_icon_without_ending_drag() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let mut drag = begin_source_less_wire_drag(&socket_path, &commands).unwrap();
    let icon_id = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .find(|surface| (surface.width, surface.height) == (21, 15))
        .expect("active DragIcon must be visible before NULL attachment")
        .surface_id;
    drag.icon.attach(None, 0, 0);
    drag.icon.commit();
    drag.connection.flush().unwrap();
    drag.queue.roundtrip(&mut drag.state).unwrap();
    wait_for_server_commands(&commands);
    let hidden = !capture_renderable_surface_snapshot(&commands)
        .iter()
        .any(|surface| surface.surface_id == icon_id);
    let active_after_null = capture_active_drag_icon_surface(&commands);
    let role_after_null = capture_surface_role_state(&commands, icon_id);

    let qh = drag.queue.handle();
    commit_test_buffered_surface(&drag.icon, &drag.icon_shm, &qh, 21, 15).unwrap();
    drag.connection.flush().unwrap();
    drag.queue.roundtrip(&mut drag.state).unwrap();
    wait_for_server_commands(&commands);
    let remapped = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .find(|surface| surface.surface_id == icon_id);
    let active_after_remap = capture_active_drag_icon_surface(&commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(
        hidden,
        "NULL attachment must hide committed DragIcon pixels"
    );
    assert_eq!(active_after_null, Some(icon_id));
    assert_eq!(role_after_null, ("drag_icon".to_string(), true));
    assert!(
        remapped.is_some(),
        "a later buffer must remap during the same drag"
    );
    assert_eq!(active_after_remap, Some(icon_id));
}
