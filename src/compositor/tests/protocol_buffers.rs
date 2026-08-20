use super::*;

struct RawSurfaceFocusSnapshot {
    state: RegistryTestState,
    logical_focus: Option<u32>,
    keyboard_focus: Option<u32>,
}

#[test]
fn clipboard_ready_wayland_client_can_create_data_device() {
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

    let result = create_client_data_device(&socket_path);
    stop_test_server(running, server_thread);

    result.unwrap();
}

#[test]
fn raw_wl_surface_creation_does_not_establish_keyboard_focus() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    for surface_first in [true, false] {
        let snapshot =
            exercise_raw_surface_bind_order(&socket_path, &commands, surface_first).unwrap();
        assert_eq!(
            snapshot.logical_focus, None,
            "surface_first={surface_first}"
        );
        assert_eq!(
            snapshot.keyboard_focus, None,
            "surface_first={surface_first}"
        );
        assert_eq!(
            snapshot.state.keyboard_enter_count, 0,
            "surface_first={surface_first}"
        );
        assert!(
            snapshot.state.data_device_selection_events.is_empty(),
            "surface_first={surface_first}"
        );
        assert!(
            snapshot.state.primary_selection_events.is_empty(),
            "surface_first={surface_first}"
        );
    }

    stop_controllable_test_server(commands, server_thread);
}

#[test]
#[ignore = "requires TYPHON_QT_WAYLAND_BOOTSTRAP_PROBE"]
fn qt_wayland_bootstrap_probe_survives_active_clipboard_without_focus() {
    let Ok(probe) = std::env::var("TYPHON_QT_WAYLAND_BOOTSTRAP_PROBE") else {
        return;
    };
    let socket_name = unique_socket_name();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let bridge = ScriptedClipboardBridge::with_host_selection(
        HostClipboardOfferId(123),
        vec!["text/plain".to_string(), "text/html".to_string()],
        b"host clipboard payload",
        requests,
    );
    let server =
        OwnCompositorServer::bind_with_clipboard_bridge(&socket_name, Box::new(bridge)).unwrap();
    let (running, server_thread) = spawn_test_server(server);

    let status = std::process::Command::new(probe)
        .env("XDG_RUNTIME_DIR", std::env::var("XDG_RUNTIME_DIR").unwrap())
        .env("WAYLAND_DISPLAY", &socket_name)
        .env("QT_QPA_PLATFORM", "wayland")
        .env("WAYLAND_DEBUG", "client")
        .status()
        .unwrap();

    stop_test_server(running, server_thread);
    assert!(status.success(), "Qt probe exited unsuccessfully: {status}");
}

fn exercise_raw_surface_bind_order(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    surface_first: bool,
) -> Result<RawSurfaceFocusSnapshot, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let manager: client_wl_data_device_manager::WlDataDeviceManager =
        globals.bind(&qh, 1..=3, ())?;
    let primary_manager:
        client_zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1 =
        globals.bind(&qh, 1..=1, ())?;
    let surface = surface_first.then(|| compositor.create_surface(&qh, ()));
    let _keyboard = seat.get_keyboard(&qh, ());
    let _data_device = manager.get_data_device(&seat, &qh, ());
    let _primary_device = primary_manager.get_device(&seat, &qh, ());
    let _surface = surface.unwrap_or_else(|| compositor.create_surface(&qh, ()));
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    wait_for_server_commands(commands);
    Ok(RawSurfaceFocusSnapshot {
        state,
        logical_focus: capture_focused_surface_id(commands),
        keyboard_focus: capture_keyboard_focus_surface_id(commands),
    })
}

#[test]
fn clipboard_ready_wayland_clients_transfer_selection_without_compositor_buffering() {
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
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let result = forward_clipboard_between_two_clients(&socket_path, &commands);
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    let (source_state, target_state, received) = result.unwrap();
    assert_eq!(
        target_state.data_offer_mime_types,
        ["text/plain", "text/html"]
    );
    assert_eq!(
        target_state.event_timeline,
        [
            TestWaylandEvent::DataOffer,
            TestWaylandEvent::DataOfferMime("text/plain".to_string()),
            TestWaylandEvent::DataOfferMime("text/html".to_string()),
            TestWaylandEvent::SelectionSome,
            TestWaylandEvent::KeyboardEnter {
                surface_id: target_state.keyboard_enter_surface_id.unwrap(),
            },
            TestWaylandEvent::KeyboardModifiers,
        ]
    );
    assert_eq!(source_state.data_source_send_mime_types, ["text/plain"]);
    assert_eq!(received, "clipboard payload");
}

#[test]
fn clipboard_source_disconnect_clears_focused_target_selection() {
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
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let result = disconnect_clipboard_source_after_target_offer(&socket_path, &commands);
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    let ClipboardDisconnectResult {
        target_state,
        clipboard_state,
    } = result.unwrap();
    assert_eq!(
        target_state.data_device_selection_events,
        vec![true, false],
        "target should receive its offer followed by exactly one selection clear"
    );
    assert!(
        target_state.data_device_selection_offer.is_none(),
        "target should receive selection(None) when the source client disconnects"
    );
    assert_eq!(
        clipboard_state,
        ClipboardStateSnapshot {
            active_source: false,
            source_count: 0,
            offer_count: 0,
        },
        "disconnect should leave no active clipboard source or offer state"
    );
}

#[test]
fn host_bridge_selection_is_offered_to_focused_wayland_client() {
    let socket_name = unique_socket_name();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let bridge = ScriptedClipboardBridge::with_host_selection(
        HostClipboardOfferId(99),
        vec!["text/plain".to_string(), "text/html".to_string()],
        b"host clipboard payload",
        Arc::clone(&requests),
    );
    let server =
        OwnCompositorServer::bind_with_clipboard_bridge(&socket_name, Box::new(bridge)).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let result = receive_host_clipboard_from_bridge(&socket_path, &commands);
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    let (target_state, received) = result.unwrap();
    assert_eq!(
        target_state.data_offer_mime_types,
        ["text/plain", "text/html"]
    );
    assert_eq!(received, "host clipboard payload");
    assert_eq!(
        requests.lock().unwrap().as_slice(),
        &[(HostClipboardOfferId(99), "text/plain".to_string())]
    );
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

fn stage4_feedback_capabilities(
    drm_device: u64,
    output_generation: u64,
    primary_plane_id: u32,
    formats: impl IntoIterator<Item = (u32, u64)>,
) -> DirectScanoutFeedbackCapabilities {
    DirectScanoutFeedbackCapabilities::new(
        drm_device,
        output_generation,
        primary_plane_id,
        formats
            .into_iter()
            .map(|(format, modifier)| DirectScanoutFormatCapability { format, modifier })
            .collect(),
    )
}

fn configured_stage4_feedback_server(
    capabilities: Option<DirectScanoutFeedbackCapabilities>,
) -> (String, u64, u64) {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    let main_device: u64 = 0x1122_3344_5566_7788;
    let scanout_device: u64 = 0x8877_6655_4433_2211;
    server.set_dmabuf_feedback_with_scanout_capabilities(
        EglGlesDmabufFeedback::with_scanout_tranche(
            [EglGlesDmabufFormat::new(
                DrmFormat::Xrgb8888,
                DrmModifier(0),
            )],
            [
                EglGlesDmabufFormat::new(DrmFormat::Argb8888, DrmModifier::LINEAR),
                EglGlesDmabufFormat::new(DrmFormat::Xrgb8888, DrmModifier::LINEAR),
            ],
        ),
        Some(main_device),
        Some("/dev/dri/renderD128".to_string()),
        capabilities,
    );
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);
    let state = request_dmabuf_default_feedback(&socket_path).unwrap();
    stop_test_server(running, server_thread);
    (
        socket_name,
        state.dmabuf_feedback_tranche_scanout.len() as u64,
        scanout_device,
    )
}

#[test]
fn scanout_tranche_precedes_renderer_tranche() {
    let capabilities = stage4_feedback_capabilities(
        0x8877_6655_4433_2211,
        1,
        42,
        [(DrmFormat::Xrgb8888.as_fourcc(), 0)],
    );
    let (_socket, tranche_count, _device) = configured_stage4_feedback_server(Some(capabilities));

    assert_eq!(tranche_count, 2);
}

#[test]
fn scanout_tranche_uses_selected_drm_device() {
    let capabilities = stage4_feedback_capabilities(
        0x8877_6655_4433_2211,
        1,
        42,
        [(DrmFormat::Xrgb8888.as_fourcc(), 0)],
    );
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    let main_device: u64 = 0x1122_3344_5566_7788;
    let scanout_device: u64 = 0x8877_6655_4433_2211;
    server.set_dmabuf_feedback_with_scanout_capabilities(
        EglGlesDmabufFeedback::with_scanout_tranche(
            [EglGlesDmabufFormat::new(
                DrmFormat::Xrgb8888,
                DrmModifier(0),
            )],
            [EglGlesDmabufFormat::new(
                DrmFormat::Argb8888,
                DrmModifier::LINEAR,
            )],
        ),
        Some(main_device),
        Some("/dev/dri/renderD128".to_string()),
        Some(capabilities),
    );
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);
    let state = request_dmabuf_default_feedback(&socket_path).unwrap();
    stop_test_server(running, server_thread);

    assert_eq!(
        state.dmabuf_feedback_tranche_targets[0],
        scanout_device.to_ne_bytes()
    );
    assert_eq!(
        state.dmabuf_feedback_tranche_targets[1],
        main_device.to_ne_bytes()
    );
}

#[test]
fn forced_compat_feedback_has_no_target_device_mismatch() {
    let capabilities = stage4_feedback_capabilities(
        0x8877_6655_4433_2211,
        1,
        42,
        [(DrmFormat::Xrgb8888.as_fourcc(), 0)],
    );
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    let main_device: u64 = 0x1122_3344_5566_7788;
    server.set_dmabuf_feedback_with_scanout_capabilities_and_target(
        EglGlesDmabufFeedback::with_scanout_tranche(
            [EglGlesDmabufFormat::new(
                DrmFormat::Xrgb8888,
                DrmModifier(0),
            )],
            [EglGlesDmabufFormat::new(
                DrmFormat::Argb8888,
                DrmModifier::LINEAR,
            )],
        ),
        Some(main_device),
        Some("/dev/dri/renderD128".to_string()),
        Some(capabilities),
        Some(main_device),
    );
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);
    let state = request_dmabuf_default_feedback(&socket_path).unwrap();
    stop_test_server(running, server_thread);

    assert_eq!(state.dmabuf_feedback_tranche_targets.len(), 2);
    assert!(
        state
            .dmabuf_feedback_tranche_targets
            .iter()
            .all(|target| *target == main_device.to_ne_bytes())
    );
}

#[test]
fn scanout_tranche_excludes_renderer_only_formats() {
    let capabilities = stage4_feedback_capabilities(
        0x8877_6655_4433_2211,
        1,
        42,
        [(DrmFormat::Xrgb8888.as_fourcc(), 0)],
    );
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    server.set_dmabuf_feedback_with_scanout_capabilities(
        EglGlesDmabufFeedback::with_scanout_tranche(
            [
                EglGlesDmabufFormat::new(DrmFormat::Xrgb8888, DrmModifier(0)),
                EglGlesDmabufFormat::new(DrmFormat::Argb8888, DrmModifier::LINEAR),
            ],
            [EglGlesDmabufFormat::new(
                DrmFormat::Argb8888,
                DrmModifier::LINEAR,
            )],
        ),
        Some(0x1122),
        Some("/dev/dri/renderD128".to_string()),
        Some(capabilities),
    );
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);
    let state = request_dmabuf_default_feedback(&socket_path).unwrap();
    stop_test_server(running, server_thread);

    assert_eq!(state.dmabuf_feedback_tranche_indices[0], vec![0, 0]);
    assert_eq!(state.dmabuf_feedback_tranche_indices[1], vec![1, 0]);
}

#[test]
fn scanout_tranche_excludes_unsupported_modifiers() {
    let capabilities = stage4_feedback_capabilities(
        0x8877_6655_4433_2211,
        1,
        42,
        [(DrmFormat::Xrgb8888.as_fourcc(), 7)],
    );
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    server.set_dmabuf_feedback_with_scanout_capabilities(
        EglGlesDmabufFeedback::with_scanout_tranche(
            [EglGlesDmabufFormat::new(
                DrmFormat::Xrgb8888,
                DrmModifier(8),
            )],
            [EglGlesDmabufFormat::new(
                DrmFormat::Xrgb8888,
                DrmModifier::LINEAR,
            )],
        ),
        Some(0x1122),
        Some("/dev/dri/renderD128".to_string()),
        Some(capabilities),
    );
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);
    let state = request_dmabuf_default_feedback(&socket_path).unwrap();
    stop_test_server(running, server_thread);

    assert!(!state.dmabuf_feedback_tranche_scanout[0]);
}

#[test]
fn scanout_tranche_excludes_unimplemented_transform_and_scaling_paths() {
    let capabilities = stage4_feedback_capabilities(
        0x8877_6655_4433_2211,
        1,
        42,
        [(DrmFormat::Xrgb8888.as_fourcc(), 0)],
    );

    assert_eq!(capabilities.formats.len(), 1);
    assert_eq!(
        capabilities.formats[0].format,
        DrmFormat::Xrgb8888.as_fourcc()
    );
    assert_eq!(capabilities.formats[0].modifier, 0);
}

#[test]
fn renderer_tranche_remains_available_after_scanout_tranche() {
    let capabilities = stage4_feedback_capabilities(
        0x8877_6655_4433_2211,
        1,
        42,
        [(DrmFormat::Xrgb8888.as_fourcc(), 0)],
    );
    let (_socket, tranche_count, _device) = configured_stage4_feedback_server(Some(capabilities));

    assert_eq!(tranche_count, 2);
}

#[test]
fn empty_scanout_capability_set_omits_scanout_tranche() {
    let (_socket, tranche_count, _device) = configured_stage4_feedback_server(Some(
        stage4_feedback_capabilities(0x8877_6655_4433_2211, 1, 42, []),
    ));

    assert_eq!(tranche_count, 1);
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
    assert!(!state.wl_drm_authenticated);
}

#[test]
fn wl_shm_pool_resize_growth_enables_buffer_above_initial_size() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let result = create_toplevel_with_resized_shm_pool_buffer(&socket_path, 32, 16);
    let server = stop_test_server(running, server_thread);

    result.unwrap();
    assert_eq!(server.renderable_surfaces().len(), 1);
    let surface = &server.renderable_surfaces()[0];
    assert_eq!(surface.width, 2);
    assert_eq!(surface.height, 2);
    assert_eq!(
        surface.cpu_pixels(),
        Some(vec![0xff55_0000, 0xff00_5500, 0xff00_0055, 0xff55_5555].as_slice())
    );
}

#[test]
fn wl_shm_pool_resize_to_same_size_remains_usable() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let result = create_toplevel_with_resized_shm_pool_buffer(&socket_path, 16, 0);
    let server = stop_test_server(running, server_thread);

    result.unwrap();
    assert_eq!(server.renderable_surfaces().len(), 1);
}

#[test]
fn wl_shm_pool_resize_shrink_is_rejected() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let result = resize_shm_pool_to_invalid_size(&socket_path, 8);
    stop_test_server(running, server_thread);

    assert!(result.is_err());
}

#[test]
fn wl_shm_create_pool_zero_size_is_rejected() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let result = create_shm_pool_with_invalid_size(&socket_path, 0);
    stop_test_server(running, server_thread);

    assert!(result.is_err());
}

#[test]
fn wl_shm_create_pool_negative_size_is_rejected() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let result = create_shm_pool_with_invalid_size(&socket_path, -1);
    stop_test_server(running, server_thread);

    assert!(result.is_err());
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
    assert!(!state.wl_drm_authenticated);
}

#[test]
fn wl_drm_authentication_is_rejected_without_magic_authentication_contract() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let result = request_wl_drm_authentication(&socket_path);
    stop_test_server(running, server_thread);

    assert!(result.is_err());
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
