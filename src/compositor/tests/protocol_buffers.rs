use super::*;
use crate::compositor::DmabufKmsPreferredState;

struct RawSurfaceFocusSnapshot {
    state: RegistryTestState,
    logical_focus: Option<u32>,
    keyboard_focus: Option<u32>,
}

#[derive(Clone, Copy)]
enum SelectionOfferRequest {
    SetActions,
    Finish,
    ReceiveInvalidMime,
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
    assert_eq!(
        source_state.data_source_send_mime_types,
        ["text/plain", "text/html"]
    );
    assert_eq!(received, ["clipboard payload", "clipboard payload"]);
}

#[test]
fn clipboard_selection_accept_then_receive_survives_exact_firefox_mime() {
    let (transfer, dnd_state) = run_clipboard_selection_accept_case("text/plain;charset=utf-8");

    assert_eq!(
        transfer.target_state.data_offer_mime_types,
        ["text/plain;charset=utf-8"]
    );
    assert_eq!(
        transfer.source_state.data_source_send_mime_types,
        ["text/plain;charset=utf-8"]
    );
    assert_eq!(transfer.received, ["clipboard payload"]);
    assert_eq!(
        transfer.selection_generation_after_accept,
        transfer.selection_generation_before_accept
    );
    assert_eq!(
        transfer.selection_generation_after_receive,
        transfer.selection_generation_before_accept
    );
    assert!(
        transfer
            .source_state
            .data_source_target_mime_types
            .is_empty()
    );
    assert_eq!(dnd_state.offer_phase, None);
    assert_eq!(dnd_state.active_phase, None);
    assert_eq!(dnd_state.offer_action_events, 0);
    assert_eq!(dnd_state.source_action_events, 0);
}

#[test]
fn clipboard_selection_accept_with_unoffered_mime_is_ignored() {
    let (transfer, dnd_state) = run_clipboard_selection_accept_case("application/x-unoffered");

    assert_eq!(transfer.received, ["clipboard payload"]);
    assert_eq!(
        transfer.source_state.data_source_send_mime_types,
        ["text/plain;charset=utf-8"]
    );
    assert_eq!(
        transfer.selection_generation_after_accept,
        transfer.selection_generation_before_accept
    );
    assert!(
        transfer
            .source_state
            .data_source_target_mime_types
            .is_empty()
    );
    assert_eq!(dnd_state.offer_phase, None);
    assert_eq!(dnd_state.active_phase, None);
    assert_eq!(dnd_state.offer_action_events, 0);
    assert_eq!(dnd_state.source_action_events, 0);
}

fn run_clipboard_selection_accept_case(
    accept_mime_type: &str,
) -> (ClipboardSelectionAcceptTransfer, DndActionSnapshot) {
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

    let transfer =
        forward_clipboard_with_selection_accept(&socket_path, &commands, accept_mime_type).unwrap();
    let dnd_state = capture_dnd_action_snapshot(&commands, transfer.offer_protocol_id)
        .expect("selection offer must remain tracked without DnD state");

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    (transfer, dnd_state)
}

#[test]
fn clipboard_selection_offer_keeps_set_actions_and_finish_fatal() {
    let set_actions = selection_offer_request(SelectionOfferRequest::SetActions)
        .expect("selection offer set_actions must remain fatal");
    assert_eq!(
        set_actions.code,
        client_wl_data_offer::Error::InvalidOffer as u32
    );
    assert_eq!(
        set_actions.message,
        "selection offer cannot negotiate drag-and-drop actions"
    );

    let finish = selection_offer_request(SelectionOfferRequest::Finish)
        .expect("selection offer finish must remain fatal");
    assert_eq!(
        finish.code,
        client_wl_data_offer::Error::InvalidFinish as u32
    );
    assert_eq!(
        finish.message,
        "data offer finish was not preceded by a valid drop"
    );
}

#[test]
fn clipboard_selection_receive_with_unoffered_mime_still_does_not_transfer() {
    assert!(selection_offer_request(SelectionOfferRequest::ReceiveInvalidMime).is_none());
}

fn selection_offer_request(request: SelectionOfferRequest) -> Option<ProtocolErrorObservation> {
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

    let source_connection =
        Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (source_globals, mut source_queue) =
        registry_queue_init::<RegistryTestState>(&source_connection).unwrap();
    let source_qh = source_queue.handle();
    let source_compositor: client_wl_compositor::WlCompositor =
        source_globals.bind(&source_qh, 1..=6, ()).unwrap();
    let source_wm_base: client_xdg_wm_base::XdgWmBase =
        source_globals.bind(&source_qh, 1..=6, ()).unwrap();
    let source_seat: client_wl_seat::WlSeat = source_globals.bind(&source_qh, 1..=7, ()).unwrap();
    let source_shm: client_wl_shm::WlShm = source_globals.bind(&source_qh, 1..=1, ()).unwrap();
    let source_manager: client_wl_data_device_manager::WlDataDeviceManager =
        source_globals.bind(&source_qh, 1..=3, ()).unwrap();
    let _source_keyboard = source_seat.get_keyboard(&source_qh, ());
    let source = source_manager.create_data_source(&source_qh, ());
    source.offer("text/plain;charset=utf-8".to_string());
    let source_device = source_manager.get_data_device(&source_seat, &source_qh, ());
    let source_surface = source_compositor.create_surface(&source_qh, ());
    let source_xdg_surface = source_wm_base.get_xdg_surface(&source_surface, &source_qh, ());
    let _source_toplevel = source_xdg_surface.get_toplevel(&source_qh, ());
    source_surface.commit();
    source_connection.flush().unwrap();

    let mut source_state = RegistryTestState::default();
    source_queue.roundtrip(&mut source_state).unwrap();
    commit_test_buffered_surface(&source_surface, &source_shm, &source_qh, 32, 32).unwrap();
    source_connection.flush().unwrap();
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    let serial = source_state
        .keyboard_enter_serial
        .expect("clipboard source must receive keyboard focus");
    source_device.set_selection(Some(&source), serial);
    source_connection.flush().unwrap();
    source_connection.roundtrip().unwrap();

    let target_connection =
        Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (target_globals, mut target_queue) =
        registry_queue_init::<RegistryTestState>(&target_connection).unwrap();
    let target_qh = target_queue.handle();
    let target_compositor: client_wl_compositor::WlCompositor =
        target_globals.bind(&target_qh, 1..=6, ()).unwrap();
    let target_wm_base: client_xdg_wm_base::XdgWmBase =
        target_globals.bind(&target_qh, 1..=6, ()).unwrap();
    let target_seat: client_wl_seat::WlSeat = target_globals.bind(&target_qh, 1..=7, ()).unwrap();
    let target_shm: client_wl_shm::WlShm = target_globals.bind(&target_qh, 1..=1, ()).unwrap();
    let target_manager: client_wl_data_device_manager::WlDataDeviceManager =
        target_globals.bind(&target_qh, 1..=3, ()).unwrap();
    let _target_keyboard = target_seat.get_keyboard(&target_qh, ());
    let _target_device = target_manager.get_data_device(&target_seat, &target_qh, ());
    let target_surface = target_compositor.create_surface(&target_qh, ());
    let target_xdg_surface = target_wm_base.get_xdg_surface(&target_surface, &target_qh, ());
    let _target_toplevel = target_xdg_surface.get_toplevel(&target_qh, ());
    target_surface.commit();
    target_connection.flush().unwrap();

    let mut target_state = RegistryTestState::default();
    target_queue.roundtrip(&mut target_state).unwrap();
    commit_test_buffered_surface(&target_surface, &target_shm, &target_qh, 32, 32).unwrap();
    target_connection.flush().unwrap();
    wait_for_server_commands(&commands);
    target_queue.roundtrip(&mut target_state).unwrap();
    let offer = target_state
        .data_device_selection_offer
        .as_ref()
        .expect("target must receive a clipboard selection offer");

    let observed = match request {
        SelectionOfferRequest::SetActions => {
            offer.set_actions(
                client_wl_data_device_manager::DndAction::Copy,
                client_wl_data_device_manager::DndAction::Copy,
            );
            target_connection.flush().unwrap();
            wait_for_server_commands(&commands);
            Some(expect_protocol_error(
                &target_connection,
                "wl_data_offer",
                client_wl_data_offer::Error::InvalidOffer as u32,
            ))
        }
        SelectionOfferRequest::Finish => {
            offer.finish();
            target_connection.flush().unwrap();
            wait_for_server_commands(&commands);
            Some(expect_protocol_error(
                &target_connection,
                "wl_data_offer",
                client_wl_data_offer::Error::InvalidFinish as u32,
            ))
        }
        SelectionOfferRequest::ReceiveInvalidMime => {
            let (read_fd, write_fd) = owned_pipe().unwrap();
            offer.receive("application/x-unoffered".to_string(), write_fd.as_fd());
            target_connection.flush().unwrap();
            drop(write_fd);
            target_connection
                .roundtrip()
                .expect("invalid selection receive MIME must not disconnect the client");
            let mut payload = Vec::new();
            File::from(read_fd).read_to_end(&mut payload).unwrap();
            assert!(payload.is_empty());
            source_queue.roundtrip(&mut source_state).unwrap();
            assert!(source_state.data_source_send_mime_types.is_empty());
            None
        }
    };

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();
    drop(source_queue);
    drop(source_connection);

    observed
}

#[test]
fn clipboard_receive_repeats_exact_firefox_mimes_and_rejects_retired_sources() {
    for destroy_new_offer_first in [false, true] {
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

        let result = exercise_clipboard_receive_replacement_and_disconnect(
            &socket_path,
            &commands,
            destroy_new_offer_first,
        );
        commands.send(ServerCommand::Stop).unwrap();
        server_thread.join().unwrap();

        let snapshot = result.unwrap();
        assert_eq!(
            snapshot.advertised_mime_types,
            [
                "text/plain;charset=utf-8",
                "UTF8_STRING",
                "text/plain",
                "STRING",
            ],
            "Wayland MIME strings must be advertised without normalization"
        );
        assert_eq!(
            snapshot.original_payloads,
            [
                b"original source payload".to_vec(),
                b"original source payload".to_vec(),
            ],
            "repeated MIME receives on one offer must reach its original source"
        );
        assert_eq!(
            snapshot.current_payloads,
            [
                b"replacement source payload".to_vec(),
                b"replacement source payload".to_vec(),
            ],
            "one current offer must transfer multiple advertised MIME types"
        );
        assert!(snapshot.stale_payload.is_empty());
        assert!(snapshot.payload_after_source_disconnect.is_empty());
        assert_eq!(
            snapshot.original_source_send_mime_types,
            ["text/plain;charset=utf-8", "text/plain"]
        );
        assert_eq!(
            snapshot.replacement_source_send_mime_types,
            ["text/plain;charset=utf-8", "text/plain"]
        );
        assert_eq!(snapshot.replacement_send_count_after_stale_receive, 2);
        assert!(snapshot.selection_active_after_old_source_disconnect);
        assert!(!snapshot.selection_active_after_current_source_disconnect);
        assert!(!snapshot.final_clipboard_state.active_source);
        assert_eq!(
            snapshot.final_clipboard_state.clipboard_broker_offer_count,
            0
        );
        assert_eq!(snapshot.final_clipboard_state.offer_count, 0);
    }
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
            generation: 2,
            mutation_epoch: 2,
            primary_generation: 0,
            primary_mutation_epoch: 0,
            clipboard_broker_offer_count: 0,
            primary_broker_offer_count: 0,
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
    server.activate_surface_scanout_hint(1);
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);
    let state = request_dmabuf_surface_feedback(&socket_path).unwrap();
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
fn default_feedback_stays_renderer_only_when_scanout_capabilities_exist() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
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
        Some(0x1122),
        Some("/dev/dri/renderD128".to_string()),
        Some(stage4_feedback_capabilities(
            0x8877_6655_4433_2211,
            1,
            42,
            [(DrmFormat::Xrgb8888.as_fourcc(), 0)],
        )),
    );
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);
    let state = request_dmabuf_default_feedback(&socket_path).unwrap();
    stop_test_server(running, server_thread);

    assert_eq!(state.dmabuf_feedback_tranche_scanout, vec![false]);
}

#[test]
fn kms_preferred_feedback_filters_renderer_pair_from_table_and_hint_fallback() {
    let safe = EglGlesDmabufFormat::new(DrmFormat::Xrgb8888, DrmModifier(0));
    let selected_renderer_formats = [safe];
    let policy_state = DmabufKmsPreferredState {
        requested: "force",
        effective: true,
        renderer_pairs_raw: 2,
        kms_presentable_pairs: 1,
        renderer_pairs_advertised: 1,
        renderer_pairs_removed: 1,
        reason: "same-FOURCC KMS-compatible modifiers selected",
    };
    let capabilities = stage4_feedback_capabilities(
        0x8877_6655_4433_2211,
        1,
        42,
        [(DrmFormat::Xrgb8888.as_fourcc(), 0)],
    );
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    server.set_dmabuf_feedback_with_scanout_capabilities_and_target_and_kms_preferred(
        EglGlesDmabufFeedback::with_scanout_tranche([safe], selected_renderer_formats),
        Some(0x1122),
        Some("/dev/dri/renderD128".to_string()),
        Some(capabilities),
        None,
        policy_state,
    );
    assert!(
        server
            .state
            .dmabuf_feedback
            .advertises(DrmFormat::Xrgb8888, DrmModifier(0))
    );
    assert!(
        !server
            .state
            .dmabuf_feedback
            .advertises(DrmFormat::Xrgb8888, DrmModifier(1))
    );
    assert_eq!(server.state.dmabuf_feedback.format_table_formats().len(), 1);

    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);
    let default_state = request_dmabuf_default_feedback(&socket_path).unwrap();
    stop_test_server(running, server_thread);
    assert_eq!(default_state.dmabuf_feedback_tranche_scanout, vec![false]);

    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    let capabilities = stage4_feedback_capabilities(
        0x8877_6655_4433_2211,
        1,
        42,
        [(DrmFormat::Xrgb8888.as_fourcc(), 0)],
    );
    server.set_dmabuf_feedback_with_scanout_capabilities_and_target_and_kms_preferred(
        EglGlesDmabufFeedback::with_scanout_tranche([safe], [safe]),
        Some(0x1122),
        Some("/dev/dri/renderD128".to_string()),
        Some(capabilities),
        None,
        policy_state,
    );
    server.activate_surface_scanout_hint(1);
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);
    let hinted_state = request_dmabuf_surface_feedback(&socket_path).unwrap();
    stop_test_server(running, server_thread);

    assert_eq!(
        hinted_state.dmabuf_feedback_tranche_scanout,
        vec![true, false]
    );
}

#[test]
fn surface_feedback_without_hint_stays_renderer_only() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
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
        Some(0x1122),
        Some("/dev/dri/renderD128".to_string()),
        Some(stage4_feedback_capabilities(
            0x8877_6655_4433_2211,
            1,
            42,
            [(DrmFormat::Xrgb8888.as_fourcc(), 0)],
        )),
    );
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);
    let state = request_dmabuf_surface_feedback(&socket_path).unwrap();
    stop_test_server(running, server_thread);

    assert_eq!(state.dmabuf_feedback_tranche_scanout, vec![false]);
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
    server.activate_surface_scanout_hint(1);
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);
    let state = request_dmabuf_surface_feedback(&socket_path).unwrap();
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
    server.activate_surface_scanout_hint(1);
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);
    let state = request_dmabuf_surface_feedback(&socket_path).unwrap();
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
    server.activate_surface_scanout_hint(1);
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);
    let state = request_dmabuf_surface_feedback(&socket_path).unwrap();
    stop_test_server(running, server_thread);

    assert_eq!(state.dmabuf_feedback_tranche_indices[0], vec![1, 0]);
    assert_eq!(state.dmabuf_feedback_tranche_indices[1], vec![0, 0]);
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
    server.activate_surface_scanout_hint(1);
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);
    let state = request_dmabuf_surface_feedback(&socket_path).unwrap();
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
