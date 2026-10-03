use super::*;
use std::{fs::File, sync::Arc};
use wayland_server::{Client, Display};

#[test]
fn v3_source_can_offer_mime_after_start_drag_before_target_enter() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
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
    let source_shm: client_wl_shm::WlShm = source_globals.bind(&source_qh, 1..=1, ()).unwrap();
    let source_seat: client_wl_seat::WlSeat = source_globals.bind(&source_qh, 1..=7, ()).unwrap();
    let _source_pointer = source_seat.get_pointer(&source_qh, ());
    let source_manager: client_wl_data_device_manager::WlDataDeviceManager =
        source_globals.bind(&source_qh, 1..=3, ()).unwrap();
    let source_device = source_manager.get_data_device(&source_seat, &source_qh, ());
    let (source_surface, source_xdg_surface, _source_toplevel) = create_test_buffered_toplevel(
        &source_compositor,
        &source_wm_base,
        &source_shm,
        &source_qh,
        160,
        120,
    )
    .unwrap();
    let source = source_manager.create_data_source(&source_qh, ());
    source.set_actions(client_wl_data_device_manager::DndAction::Copy);
    source_surface.commit();
    source_connection.flush().unwrap();
    let mut source_state = RegistryTestState::default();
    source_queue.roundtrip(&mut source_state).unwrap();
    commit_registered_initial_xdg_test_buffer(&source_xdg_surface);
    source_connection.flush().unwrap();
    source_queue.roundtrip(&mut source_state).unwrap();

    let target_connection =
        Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (target_globals, mut target_queue) =
        registry_queue_init::<RegistryTestState>(&target_connection).unwrap();
    let target_qh = target_queue.handle();
    let target_compositor: client_wl_compositor::WlCompositor =
        target_globals.bind(&target_qh, 1..=6, ()).unwrap();
    let target_wm_base: client_xdg_wm_base::XdgWmBase =
        target_globals.bind(&target_qh, 1..=6, ()).unwrap();
    let target_shm: client_wl_shm::WlShm = target_globals.bind(&target_qh, 1..=1, ()).unwrap();
    let target_seat: client_wl_seat::WlSeat = target_globals.bind(&target_qh, 1..=7, ()).unwrap();
    let target_manager: client_wl_data_device_manager::WlDataDeviceManager =
        target_globals.bind(&target_qh, 1..=3, ()).unwrap();
    let _target_device = target_manager.get_data_device(&target_seat, &target_qh, ());
    let (target_surface, target_xdg_surface, _target_toplevel) = create_test_buffered_toplevel(
        &target_compositor,
        &target_wm_base,
        &target_shm,
        &target_qh,
        160,
        120,
    )
    .unwrap();
    target_surface.commit();
    target_connection.flush().unwrap();
    let mut target_state = RegistryTestState::default();
    target_queue.roundtrip(&mut target_state).unwrap();
    commit_registered_initial_xdg_test_buffer(&target_xdg_surface);
    target_connection.flush().unwrap();
    target_queue.roundtrip(&mut target_state).unwrap();

    focus_root_window(&commands, target_surface.id().protocol_id());
    set_focused_root_visual_geometry(
        &commands,
        SurfacePlacement::absolute_root_at(300, 200),
        160,
        120,
    );
    focus_root_window(&commands, source_surface.id().protocol_id());
    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 20.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    let serial = source_state
        .pointer_button_serial
        .expect("source drag must use the real pointer press serial");

    source_device.start_drag(Some(&source), &source_surface, None, serial);
    source_connection.flush().unwrap();
    wait_for_server_commands(&commands);
    source_queue
        .roundtrip(&mut source_state)
        .expect("v3 drag must remain alive after start_drag");
    target_queue.roundtrip(&mut target_state).unwrap();
    assert_eq!(source_state.data_source_cancelled_count, 0);

    source.offer("text/plain".to_string());
    source.offer("text/plain".to_string());
    source_connection.flush().unwrap();
    source_queue
        .roundtrip(&mut source_state)
        .expect("wl_data_source.offer after start_drag must remain connected");
    assert_eq!(source_state.data_source_cancelled_count, 0);

    commands
        .send(ServerCommand::PointerMotion { x: 320.0, y: 220.0 })
        .unwrap();
    wait_for_server_commands(&commands);
    target_queue.roundtrip(&mut target_state).unwrap();
    source_queue.roundtrip(&mut source_state).unwrap();
    assert_eq!(target_state.data_device_enter_count, 1);
    assert_eq!(target_state.data_offer_mime_types, vec!["text/plain"]);

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn v3_source_set_actions_after_start_drag_remains_invalid_source() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let _pointer = seat.get_pointer(&qh, ());
    let manager: client_wl_data_device_manager::WlDataDeviceManager =
        globals.bind(&qh, 1..=3, ()).unwrap();
    let device = manager.get_data_device(&seat, &qh, ());
    let (origin, xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    let source = manager.create_data_source(&qh, ());
    origin.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commit_registered_initial_xdg_test_buffer(&xdg_surface);
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 20.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let serial = state.pointer_button_serial.expect("pointer press serial");

    source.offer("text/plain".to_string());
    source.set_actions(client_wl_data_device_manager::DndAction::Copy);
    device.start_drag(Some(&source), &origin, None, serial);
    source.set_actions(client_wl_data_device_manager::DndAction::Move);
    connection.flush().unwrap();
    assert!(queue.roundtrip(&mut state).is_err());

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn source_less_wire_drag_with_icon_reserves_a_permanent_drag_icon_role() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let pointer = seat.get_pointer(&qh, ());
    let manager: client_wl_data_device_manager::WlDataDeviceManager =
        globals.bind(&qh, 1..=3, ()).unwrap();
    let device = manager.get_data_device(&seat, &qh, ());
    let (origin, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    let icon = compositor.create_surface(&qh, ());

    origin.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();

    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 20.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let serial = state
        .pointer_button_serial
        .expect("drag must use a real pointer press serial");

    // A source-less drag without an icon is a valid Core request.  The
    // request must begin without manufacturing a data offer or requiring a
    // role on an icon surface.
    device.start_drag(None, &origin, None, serial);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue
        .roundtrip(&mut state)
        .expect("source-less drag without an icon must remain connected");

    device.start_drag(None, &origin, Some(&icon), serial);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);

    // A drag icon is a permanent role even when this is a source-less drag.
    let icon_xdg = wm_base.get_xdg_surface(&icon, &qh, ());
    let _ = icon_xdg;
    connection.flush().unwrap();
    assert!(queue.roundtrip(&mut state).is_err());

    let _server = stop_controllable_test_server(commands, server_thread);
    let _ = pointer;
}

#[test]
fn sourced_wayland_drag_hands_pointer_routing_to_dnd_and_restores_after_drop() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let _pointer = seat.get_pointer(&qh, ());
    let manager: client_wl_data_device_manager::WlDataDeviceManager =
        globals.bind(&qh, 1..=3, ()).unwrap();
    let device = manager.get_data_device(&seat, &qh, ());
    let (surface, xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    let source = manager.create_data_source(&qh, ());
    source.offer("text/plain".to_string());
    source.set_actions(client_wl_data_device_manager::DndAction::Copy);
    surface.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commit_registered_initial_xdg_test_buffer(&xdg_surface);
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let surface_id = surface.id().protocol_id();
    focus_root_window(&commands, surface_id);
    let pointer_x = f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0;
    let pointer_y = f64::from(render::FIRST_SURFACE_OFFSET.1) + 20.0;
    commands
        .send(ServerCommand::PointerMotion {
            x: pointer_x,
            y: pointer_y,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let serial = state
        .pointer_button_serial
        .expect("start_drag must use a real pointer press serial");
    let pointer_motion_count_before_drag = state.pointer_motion_count;
    let pointer_enter_count_before_drag = state.pointer_enter_count;
    state.event_timeline.clear();

    device.start_drag(Some(&source), &surface, None, serial);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    let mut failures = Vec::new();
    let start_timeline = state.event_timeline.clone();
    let pointer_leave_at = start_timeline.iter().position(|event| {
        matches!(event, TestWaylandEvent::PointerLeave { surface_id: id } if *id == surface_id)
    });
    let data_offer_at = start_timeline
        .iter()
        .position(|event| matches!(event, TestWaylandEvent::DataOffer));
    let data_enter_at = start_timeline.iter().position(|event| {
        matches!(event, TestWaylandEvent::DataDeviceEnter { surface_id: id } if *id == surface_id)
    });
    if state.pointer_leave_count == 0 || pointer_leave_at.is_none() {
        failures.push("start_drag did not retire normal wl_pointer focus".to_string());
    }
    if state.data_device_enter_count != 1 || data_enter_at.is_none() {
        failures.push(format!(
            "start_drag did not immediately enter the current DnD target (count {})",
            state.data_device_enter_count
        ));
    }
    if let (Some(leave), Some(enter)) = (pointer_leave_at, data_enter_at) {
        if leave >= enter {
            failures.push("wl_pointer.leave did not precede wl_data_device.enter".to_string());
        }
    }
    if let (Some(offer), Some(enter)) = (data_offer_at, data_enter_at) {
        if offer >= enter {
            failures.push("wl_data_device.data_offer did not precede enter".to_string());
        }
    }
    if start_timeline
        .iter()
        .any(|event| matches!(event, TestWaylandEvent::PointerMotion))
    {
        failures.push("start_drag required or emitted ordinary pointer motion".to_string());
    }
    if state.pointer_motion_count != pointer_motion_count_before_drag {
        failures.push("start_drag emitted ordinary wl_pointer.motion".to_string());
    }
    if state.pointer_enter_count != pointer_enter_count_before_drag
        || state.pointer_enter_surface_id.is_some()
    {
        failures.push("normal pointer focus remained active during the DnD grab".to_string());
    }
    if state.data_device_enter_count > 0
        && (state.data_device_enter_surface_id != Some(surface_id)
            || state.data_device_enter_x != Some(20.0)
            || state.data_device_enter_y != Some(20.0))
    {
        failures.push(format!(
            "initial DnD enter did not use current surface-local coordinates: surface={:?} xy=({:?},{:?})",
            state.data_device_enter_surface_id, state.data_device_enter_x, state.data_device_enter_y
        ));
    }

    // Keep checking the rest of the wire contract against the old path too:
    // if the initial target was missing, this motion only lets the test
    // negotiate and reach the independent motion/release regressions below.
    if state.data_device_drag_offer.is_none() {
        commands
            .send(ServerCommand::PointerMotion {
                x: pointer_x + 1.0,
                y: pointer_y + 1.0,
            })
            .unwrap();
        wait_for_server_commands(&commands);
        queue.roundtrip(&mut state).unwrap();
    }
    let offer = state
        .data_device_drag_offer
        .clone()
        .expect("the sourced drag must eventually create a destination offer");
    let enter_serial = state
        .data_device_enter_serial
        .expect("the destination must receive an enter serial");
    offer.accept(enter_serial, Some("text/plain".to_string()));
    offer.set_actions(
        client_wl_data_device_manager::DndAction::Copy,
        client_wl_data_device_manager::DndAction::Copy,
    );
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    let data_motion_count_before = state.data_device_motion_count;
    let pointer_motion_count_before = state.pointer_motion_count;
    let pointer_enter_count_before = state.pointer_enter_count;
    state.event_timeline.clear();
    commands
        .send(ServerCommand::PointerMotion {
            x: pointer_x + 4.0,
            y: pointer_y + 4.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    if state.data_device_motion_count != data_motion_count_before + 1 {
        failures.push("physical motion did not reach wl_data_device.motion".to_string());
    }
    if state.pointer_motion_count != pointer_motion_count_before {
        failures.push("native DnD motion also reached wl_pointer.motion".to_string());
    }
    if state.pointer_enter_count != pointer_enter_count_before {
        failures.push("normal pointer focus was re-entered during the DnD grab".to_string());
    }
    if state.event_timeline.iter().any(|event| {
        matches!(
            event,
            TestWaylandEvent::PointerMotion | TestWaylandEvent::PointerEnter { .. }
        )
    }) {
        failures.push("normal pointer motion/focus events leaked during DnD motion".to_string());
    }

    let axis_count_before = state.pointer_axis_times.len();
    let frame_count_before_axis = state.pointer_frame_count;
    state.event_timeline.clear();
    commands
        .send(ServerCommand::PointerAxisFrame(PointerAxisFrame {
            timestamp_usec: 123_456_000,
            source: PointerAxisSource::Wheel,
            horizontal: PointerAxisComponent::absent(),
            vertical: PointerAxisComponent {
                continuous: Some(12.0),
                value120: None,
                discrete: Some(1),
                stopped: false,
            },
        }))
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    if state.pointer_axis_times.len() != axis_count_before {
        failures.push("wl_pointer.axis leaked through the native DnD route".to_string());
    }
    if state.pointer_frame_count != frame_count_before_axis {
        failures.push("a suppressed axis event emitted a normal pointer frame".to_string());
    }
    if state.event_timeline.iter().any(|event| {
        matches!(
            event,
            TestWaylandEvent::PointerAxis
                | TestWaylandEvent::PointerAxisSource
                | TestWaylandEvent::PointerAxisDiscrete
                | TestWaylandEvent::PointerAxisValue120
                | TestWaylandEvent::PointerAxisStop
                | TestWaylandEvent::PointerFrame
        )
    }) {
        failures.push("normal pointer axis/frame events appeared in the wire timeline".to_string());
    }

    state.event_timeline.clear();
    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: false,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let terminal_timeline = state.event_timeline.clone();
    let drop_at = terminal_timeline
        .iter()
        .position(|event| matches!(event, TestWaylandEvent::DataDeviceDrop));
    let leave_at = terminal_timeline
        .iter()
        .position(|event| matches!(event, TestWaylandEvent::DataDeviceLeave));
    let pointer_enter_at = terminal_timeline.iter().position(|event| {
        matches!(event, TestWaylandEvent::PointerEnter { surface_id: id } if *id == surface_id)
    });
    if terminal_timeline
        .iter()
        .any(|event| matches!(event, TestWaylandEvent::PointerButtonReleased))
    {
        failures.push(
            "ordinary wl_pointer.button(RELEASED) preceded the DnD terminal event".to_string(),
        );
    }
    if state.data_device_drop_count != 1 || drop_at.is_none() {
        failures.push(format!(
            "physical release did not produce exactly one wl_data_device.drop (count {})",
            state.data_device_drop_count
        ));
    }
    if state.data_device_leave_count != 1 || leave_at.is_none() {
        failures.push(format!(
            "successful drop did not send one terminal wl_data_device.leave (count {})",
            state.data_device_leave_count
        ));
    }
    if let (Some(drop), Some(leave), Some(pointer_enter)) = (drop_at, leave_at, pointer_enter_at) {
        if !(drop < leave && leave < pointer_enter) {
            failures.push(
                "terminal event order was not drop < DnD leave < pointer re-enter".to_string(),
            );
        }
    } else {
        failures.push(
            "terminal timeline lacked drop, DnD leave, or normal pointer re-enter".to_string(),
        );
    }
    if state.pointer_enter_surface_id != Some(surface_id) {
        failures.push(
            "normal pointer focus was not restored to the surface under the pointer".to_string(),
        );
    }
    if state.data_source_dnd_drop_performed_count != 1 {
        failures.push("source did not receive exactly one dnd_drop_performed".to_string());
    }

    let (read_fd, write_fd) = owned_pipe().unwrap();
    offer.receive("text/plain".to_string(), write_fd.as_fd());
    connection.flush().unwrap();
    drop(write_fd);
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let mut payload = String::new();
    File::from(read_fd).read_to_string(&mut payload).unwrap();
    assert_eq!(payload, "clipboard payload");
    offer.finish();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    if state.data_source_dnd_finished_count != 1 {
        failures.push(
            "retained dropped offer did not complete with exactly one dnd_finished".to_string(),
        );
    }
    if state.data_source_cancelled_count != 0 {
        failures.push("successful native DnD spuriously cancelled its source".to_string());
    }
    assert!(
        failures.is_empty(),
        "wire-level DnD ownership failures: {failures:#?}"
    );

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn sourced_wayland_drag_cancellation_keeps_pointer_routing_until_release() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        relative_pointer: true,
        pointer_constraints: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let pointer = seat.get_pointer(&qh, ());
    let relative_manager: client_zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let _relative_pointer = relative_manager.get_relative_pointer(&pointer, &qh, ());
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let manager: client_wl_data_device_manager::WlDataDeviceManager =
        globals.bind(&qh, 1..=3, ()).unwrap();
    let device = manager.get_data_device(&seat, &qh, ());
    let (surface, xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    let source = manager.create_data_source(&qh, ());
    source.offer("text/plain".to_string());
    source.set_actions(client_wl_data_device_manager::DndAction::Copy);
    surface.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commit_registered_initial_xdg_test_buffer(&xdg_surface);
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let surface_id = surface.id().protocol_id();
    focus_root_window(&commands, surface_id);
    let pointer_x = f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0;
    let pointer_y = f64::from(render::FIRST_SURFACE_OFFSET.1) + 20.0;
    commands
        .send(ServerCommand::PointerMotion {
            x: pointer_x,
            y: pointer_y,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    let _confined = constraints.confine_pointer(
        &surface,
        &pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    surface.commit();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let initial_constraint_requests = capture_pointer_constraint_backend_requests(&commands);
    let constraint_id = initial_constraint_requests
        .iter()
        .find_map(|request| match request {
            PointerConstraintBackendRequest::ActivateConfined { id, .. } => Some(*id),
            _ => None,
        })
        .expect("confined pointer should request backend activation");
    commands
        .send(ServerCommand::PointerConstraintBackendActivated(
            constraint_id,
        ))
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.confined_count, 1);

    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let serial = state
        .pointer_button_serial
        .expect("start_drag must use a real pointer press serial");
    state.event_timeline.clear();

    device.start_drag(Some(&source), &surface, None, serial);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    let start_timeline = state.event_timeline.clone();
    let pointer_leave_at = start_timeline.iter().position(|event| {
        matches!(event, TestWaylandEvent::PointerLeave { surface_id: id } if *id == surface_id)
    });
    let data_enter_at = start_timeline.iter().position(|event| {
        matches!(event, TestWaylandEvent::DataDeviceEnter { surface_id: id } if *id == surface_id)
    });
    assert!(
        matches!((pointer_leave_at, data_enter_at), (Some(leave), Some(enter)) if leave < enter),
        "native DnD handoff must send pointer leave before immediate data-device enter: {start_timeline:#?}"
    );
    assert_eq!(state.unconfined_count, 1);
    let handoff_constraint_requests = capture_pointer_constraint_backend_requests(&commands);
    assert!(handoff_constraint_requests.iter().any(|request| {
        matches!(request, PointerConstraintBackendRequest::Deactivate { id, .. } if *id == constraint_id)
    }));
    let offer = state
        .data_device_drag_offer
        .clone()
        .expect("initiating surface should immediately receive its DnD offer");

    state.event_timeline.clear();
    commands
        .send(ServerCommand::PointerButton {
            button: 0x111,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    commands
        .send(ServerCommand::PointerButton {
            button: 0x111,
            pressed: false,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert!(
        state.event_timeline.iter().all(|event| !matches!(
            event,
            TestWaylandEvent::PointerButtonPressed | TestWaylandEvent::PointerButtonReleased
        )),
        "additional physical buttons must not create normal pointer events: {:#?}",
        state.event_timeline
    );

    let relative_motion_count_during_dnd = state.relative_motion_count;
    state.event_timeline.clear();
    commands
        .send(ServerCommand::PointerMotionSample(PointerMotionSample {
            timestamp_usec: 0x1_0000_0003,
            absolute: None,
            relative: Some(RelativePointerMotion {
                dx: 1.25,
                dy: -0.75,
                dx_unaccelerated: 1.5,
                dy_unaccelerated: -1.0,
            }),
        }))
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(
        state.relative_motion_count,
        relative_motion_count_during_dnd
    );
    assert!(
        state.event_timeline.is_empty(),
        "relative pointer motion leaked while native DnD owned routing: {:#?}",
        state.event_timeline
    );

    state.event_timeline.clear();
    offer.destroy();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.data_device_leave_count, 1);
    assert_eq!(state.data_source_cancelled_count, 1);
    assert_eq!(state.data_device_drop_count, 0);
    assert!(
        state
            .event_timeline
            .iter()
            .any(|event| matches!(event, TestWaylandEvent::DataDeviceLeave))
    );
    assert!(
        !state
            .event_timeline
            .iter()
            .any(|event| matches!(event, TestWaylandEvent::DataDeviceDrop))
    );

    let pointer_enter_count_after_handoff = state.pointer_enter_count;
    let pointer_motion_count_after_handoff = state.pointer_motion_count;
    let pointer_axis_count_after_handoff = state.pointer_axis_times.len();
    let relative_motion_count_after_handoff = state.relative_motion_count;
    let data_motion_count_after_handoff = state.data_device_motion_count;
    state.event_timeline.clear();
    commands
        .send(ServerCommand::PointerMotionSample(PointerMotionSample {
            timestamp_usec: 0x1_0000_0002,
            absolute: Some(OutputPosition {
                x: pointer_x + 12.0,
                y: pointer_y + 12.0,
            }),
            relative: Some(RelativePointerMotion {
                dx: 3.5,
                dy: -2.25,
                dx_unaccelerated: 4.0,
                dy_unaccelerated: -3.0,
            }),
        }))
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    commands
        .send(ServerCommand::PointerAxisFrame(PointerAxisFrame {
            timestamp_usec: 0x1_0000_1000,
            source: PointerAxisSource::Wheel,
            horizontal: PointerAxisComponent::absent(),
            vertical: PointerAxisComponent {
                continuous: Some(8.0),
                value120: None,
                discrete: Some(1),
                stopped: false,
            },
        }))
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert!(
        state.event_timeline.is_empty(),
        "pointer or DnD routing leaked after cancellation while BTN_LEFT remained held: {:#?}",
        state.event_timeline
    );
    assert_eq!(state.pointer_enter_count, pointer_enter_count_after_handoff);
    assert_eq!(
        state.pointer_motion_count,
        pointer_motion_count_after_handoff
    );
    assert_eq!(
        state.pointer_axis_times.len(),
        pointer_axis_count_after_handoff
    );
    assert_eq!(
        state.relative_motion_count,
        relative_motion_count_after_handoff
    );
    assert_eq!(
        state.data_device_motion_count,
        data_motion_count_after_handoff
    );
    assert_eq!(state.unconfined_count, 1);
    assert!(
        !capture_pointer_constraint_backend_requests(&commands)
            .iter()
            .any(|request| matches!(
                request,
                PointerConstraintBackendRequest::ActivateConfined { .. }
            ))
    );

    state.event_timeline.clear();
    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: false,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let terminal_timeline = state.event_timeline.clone();
    assert!(terminal_timeline.iter().any(|event| {
        matches!(event, TestWaylandEvent::PointerEnter { surface_id: id } if *id == surface_id)
    }));
    assert!(
        !terminal_timeline
            .iter()
            .any(|event| matches!(event, TestWaylandEvent::PointerButtonReleased))
    );
    assert!(
        !terminal_timeline
            .iter()
            .any(|event| matches!(event, TestWaylandEvent::DataDeviceDrop))
    );
    assert_eq!(state.data_device_drop_count, 0);
    assert_eq!(state.data_source_cancelled_count, 1);
    let terminal_constraint_requests = capture_pointer_constraint_backend_requests(&commands);
    let reactivated_constraint_id = terminal_constraint_requests
        .iter()
        .find_map(|request| match request {
            PointerConstraintBackendRequest::ActivateConfined { id, .. } => Some(*id),
            _ => None,
        })
        .expect(
            "normal pointer focus should make the persistent constraint eligible after release",
        );
    commands
        .send(ServerCommand::PointerConstraintBackendActivated(
            reactivated_constraint_id,
        ))
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.confined_count, 2);

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn source_less_wayland_drag_hands_off_pointer_and_stays_private_to_initiator() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let pointer = seat.get_pointer(&qh, ());
    let manager: client_wl_data_device_manager::WlDataDeviceManager =
        globals.bind(&qh, 1..=3, ()).unwrap();
    let device = manager.get_data_device(&seat, &qh, ());
    let (origin, origin_xdg, _origin_toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    origin.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commit_registered_initial_xdg_test_buffer(&origin_xdg);
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let other_connection =
        Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (other_globals, mut other_queue) =
        registry_queue_init::<RegistryTestState>(&other_connection).unwrap();
    let other_qh = other_queue.handle();
    let other_compositor: client_wl_compositor::WlCompositor =
        other_globals.bind(&other_qh, 1..=6, ()).unwrap();
    let other_wm_base: client_xdg_wm_base::XdgWmBase =
        other_globals.bind(&other_qh, 1..=6, ()).unwrap();
    let other_shm: client_wl_shm::WlShm = other_globals.bind(&other_qh, 1..=1, ()).unwrap();
    let other_seat: client_wl_seat::WlSeat = other_globals.bind(&other_qh, 1..=7, ()).unwrap();
    let other_pointer = other_seat.get_pointer(&other_qh, ());
    let other_manager: client_wl_data_device_manager::WlDataDeviceManager =
        other_globals.bind(&other_qh, 1..=3, ()).unwrap();
    let _other_device = other_manager.get_data_device(&other_seat, &other_qh, ());
    let (other_surface, other_xdg, _other_toplevel) = create_test_buffered_toplevel(
        &other_compositor,
        &other_wm_base,
        &other_shm,
        &other_qh,
        160,
        120,
    )
    .unwrap();
    other_surface.commit();
    other_connection.flush().unwrap();
    let mut other_state = RegistryTestState::default();
    other_queue.roundtrip(&mut other_state).unwrap();
    commit_registered_initial_xdg_test_buffer(&other_xdg);
    other_connection.flush().unwrap();
    other_queue.roundtrip(&mut other_state).unwrap();

    focus_root_window(&commands, other_surface.id().protocol_id());
    set_focused_root_visual_geometry(
        &commands,
        SurfacePlacement::absolute_root_at(300, 200),
        160,
        120,
    );
    focus_root_window(&commands, origin.id().protocol_id());
    let origin_id = origin.id().protocol_id();
    let pointer_x = f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0;
    let pointer_y = f64::from(render::FIRST_SURFACE_OFFSET.1) + 20.0;
    commands
        .send(ServerCommand::PointerMotion {
            x: pointer_x,
            y: pointer_y,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    other_queue.roundtrip(&mut other_state).unwrap();
    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let serial = state
        .pointer_button_serial
        .expect("source-less drag must use a real pointer press serial");
    let pointer_motion_count_before_drag = state.pointer_motion_count;
    state.event_timeline.clear();
    other_state.event_timeline.clear();

    device.start_drag(None, &origin, None, serial);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    other_queue.roundtrip(&mut other_state).unwrap();

    let mut failures = Vec::new();
    let start_timeline = state.event_timeline.clone();
    let pointer_leave_at = start_timeline.iter().position(|event| {
        matches!(event, TestWaylandEvent::PointerLeave { surface_id } if *surface_id == origin_id)
    });
    let data_enter_at = start_timeline.iter().position(|event| {
        matches!(event, TestWaylandEvent::DataDeviceEnter { surface_id } if *surface_id == origin_id)
    });
    if state.data_device_enter_count != 1 || data_enter_at.is_none() {
        failures.push(
            "source-less start_drag did not immediately enter the initiating client".to_string(),
        );
    }
    if start_timeline
        .iter()
        .any(|event| matches!(event, TestWaylandEvent::DataOffer))
    {
        failures.push("source-less DnD unexpectedly created a data offer".to_string());
    }
    if let (Some(leave), Some(enter)) = (pointer_leave_at, data_enter_at) {
        if leave >= enter {
            failures.push("source-less wl_pointer.leave did not precede DnD enter".to_string());
        }
    } else {
        failures.push("source-less start_drag did not hand focus from pointer to DnD".to_string());
    }
    if state.pointer_motion_count != pointer_motion_count_before_drag {
        failures.push("source-less start_drag emitted ordinary pointer motion".to_string());
    }
    if state.data_device_drag_offer.is_some() || !state.data_offer_mime_types.is_empty() {
        failures.push("source-less DnD exposed a data offer".to_string());
    }

    if state.data_device_enter_count == 0 {
        commands
            .send(ServerCommand::PointerMotion {
                x: pointer_x + 1.0,
                y: pointer_y + 1.0,
            })
            .unwrap();
        wait_for_server_commands(&commands);
        queue.roundtrip(&mut state).unwrap();
    }
    let pointer_motion_count_before_move = state.pointer_motion_count;
    let data_motion_count_before_move = state.data_device_motion_count;
    state.event_timeline.clear();
    commands
        .send(ServerCommand::PointerMotion {
            x: pointer_x + 4.0,
            y: pointer_y + 4.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    if state.pointer_motion_count != pointer_motion_count_before_move {
        failures.push("source-less DnD motion also reached wl_pointer.motion".to_string());
    }
    if state.data_device_motion_count != data_motion_count_before_move + 1 {
        failures
            .push("source-less drag motion did not reach the initiating data device".to_string());
    }
    if state
        .event_timeline
        .iter()
        .any(|event| matches!(event, TestWaylandEvent::PointerMotion))
    {
        failures.push("ordinary pointer motion leaked during source-less DnD".to_string());
    }

    commands
        .send(ServerCommand::PointerMotion { x: 320.0, y: 220.0 })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    other_queue.roundtrip(&mut other_state).unwrap();
    if other_state.data_device_enter_count != 0 || other_state.data_device_motion_count != 0 {
        failures.push("source-less DnD leaked to another client's data device".to_string());
    }
    if other_state.pointer_enter_count != 0 || other_state.pointer_motion_count != 0 {
        failures.push(
            "normal pointer events reached the foreign client during source-less DnD".to_string(),
        );
    }

    commands
        .send(ServerCommand::PointerMotion {
            x: pointer_x + 30.0,
            y: pointer_y + 30.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    other_queue.roundtrip(&mut other_state).unwrap();
    if state.data_device_enter_count != 2 {
        failures.push(format!(
            "returning to the initiating surface did not restore source-less DnD focus (count {})",
            state.data_device_enter_count
        ));
    }
    if state.data_device_enter_surface_id != Some(origin_id) {
        failures.push("source-less DnD focus left the initiating surface".to_string());
    }
    if other_state.data_device_enter_count != 0 {
        failures.push("another client received a source-less data-device enter".to_string());
    }

    state.event_timeline.clear();
    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: false,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let terminal_timeline = state.event_timeline.clone();
    let drop_at = terminal_timeline
        .iter()
        .position(|event| matches!(event, TestWaylandEvent::DataDeviceDrop));
    let leave_at = terminal_timeline
        .iter()
        .position(|event| matches!(event, TestWaylandEvent::DataDeviceLeave));
    let pointer_enter_at = terminal_timeline.iter().position(|event| {
        matches!(event, TestWaylandEvent::PointerEnter { surface_id } if *surface_id == origin_id)
    });
    if terminal_timeline
        .iter()
        .any(|event| matches!(event, TestWaylandEvent::PointerButtonReleased))
    {
        failures
            .push("source-less terminal release leaked wl_pointer.button(RELEASED)".to_string());
    }
    if state.data_device_drop_count != 1 || state.data_device_leave_count != 2 {
        failures.push(format!(
            "source-less release must produce one drop and terminal leave (drop {}, leave {})",
            state.data_device_drop_count, state.data_device_leave_count
        ));
    }
    if let (Some(drop), Some(leave), Some(pointer_enter)) = (drop_at, leave_at, pointer_enter_at) {
        if !(drop < leave && leave < pointer_enter) {
            failures.push(
                "source-less terminal order was not drop < leave < pointer re-enter".to_string(),
            );
        }
    } else {
        failures.push(
            "source-less terminal timeline lacked drop, leave, or pointer re-enter".to_string(),
        );
    }
    if state.pointer_enter_surface_id != Some(origin_id) {
        failures.push("pointer focus was not restored to the current scene target".to_string());
    }
    if other_state.data_device_enter_count != 0 {
        failures
            .push("source-less drag exposed its data-device focus to another client".to_string());
    }
    assert!(
        failures.is_empty(),
        "source-less wire DnD failures: {failures:#?}"
    );

    let _server = stop_controllable_test_server(commands, server_thread);
    let _ = pointer;
    let _ = other_pointer;
}

#[test]
fn destroying_active_drag_icon_keeps_drag_session_and_client_alive() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let _pointer = seat.get_pointer(&qh, ());
    let manager: client_wl_data_device_manager::WlDataDeviceManager =
        globals.bind(&qh, 1..=3, ()).unwrap();
    let device = manager.get_data_device(&seat, &qh, ());
    let (origin, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    let icon = compositor.create_surface(&qh, ());

    origin.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();

    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 20.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let serial = state
        .pointer_button_serial
        .expect("drag must use a real pointer press serial");

    device.start_drag(None, &origin, Some(&icon), serial);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue
        .roundtrip(&mut state)
        .expect("drag with an icon must start before icon destruction");

    icon.destroy();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let destroy_result = queue.roundtrip(&mut state);

    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 48.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 36.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    let motion_result = queue.roundtrip(&mut state);

    let server = stop_controllable_test_server(commands, server_thread);
    assert!(
        destroy_result.is_ok(),
        "destroying a live DragIcon wl_surface must not post defunct_role_object: {destroy_result:?}"
    );
    assert!(
        motion_result.is_ok(),
        "pointer motion after icon destruction must leave the client connected: {motion_result:?}"
    );
    let drag = server
        .state
        .active_drag
        .as_ref()
        .expect("destroying only the icon must not terminate the drag");
    assert!(
        drag.icon_surface.is_none(),
        "the active drag must release its destroyed icon resource"
    );
}

#[test]
fn start_drag_publishes_a_precommitted_drag_icon_buffer() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let _pointer = seat.get_pointer(&qh, ());
    let manager: client_wl_data_device_manager::WlDataDeviceManager =
        globals.bind(&qh, 1..=3, ()).unwrap();
    let device = manager.get_data_device(&seat, &qh, ());
    let (origin, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    let icon = compositor.create_surface(&qh, ());

    origin.commit();
    commit_test_buffered_surface(&icon, &shm, &qh, 19, 13).unwrap();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);
    let icon_before_drag_is_rendered = capture_renderable_surface_snapshot(&commands)
        .iter()
        .any(|surface| (surface.width, surface.height) == (19, 13));

    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 20.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let serial = state
        .pointer_button_serial
        .expect("drag must use a real pointer press serial");

    device.start_drag(None, &origin, Some(&icon), serial);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue
        .roundtrip(&mut state)
        .expect("drag with a precommitted icon must start");
    let icon_after_drag = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .find(|surface| (surface.width, surface.height) == (19, 13));

    let _server = stop_controllable_test_server(commands, server_thread);
    assert!(
        !icon_before_drag_is_rendered,
        "an unassigned surface's precommitted buffer must remain unpublished before start_drag"
    );
    let icon_after_drag = icon_after_drag.expect(
        "start_drag must immediately publish the icon's already-committed buffer into the render scene",
    );
    assert_eq!((icon_after_drag.width, icon_after_drag.height), (19, 13));
    assert!(
        icon_after_drag.pixel_checksum.is_some(),
        "the adopted icon must contain its committed SHM pixels"
    );
}

#[test]
fn active_drag_icon_publishes_and_replaces_committed_buffers() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let _pointer = seat.get_pointer(&qh, ());
    let manager: client_wl_data_device_manager::WlDataDeviceManager =
        globals.bind(&qh, 1..=3, ()).unwrap();
    let device = manager.get_data_device(&seat, &qh, ());
    let (origin, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    let icon = compositor.create_surface(&qh, ());

    origin.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 20.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let serial = state
        .pointer_button_serial
        .expect("drag must use a real pointer press serial");

    device.start_drag(None, &origin, Some(&icon), serial);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue
        .roundtrip(&mut state)
        .expect("drag with an empty icon must start");

    commit_test_buffered_surface(&icon, &shm, &qh, 21, 14).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);
    let first_icon_buffer = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .find(|surface| (surface.width, surface.height) == (21, 14));

    commit_test_buffered_surface(&icon, &shm, &qh, 23, 16).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);
    let replacement_icon_buffer = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .find(|surface| (surface.width, surface.height) == (23, 16));

    let _server = stop_controllable_test_server(commands, server_thread);
    let first_icon_buffer = first_icon_buffer.expect(
        "an active DragIcon commit must publish its SHM buffer through normal surface publication",
    );
    let replacement_icon_buffer = replacement_icon_buffer
        .expect("a later active DragIcon commit must replace the published buffer");
    assert_ne!(
        first_icon_buffer.buffer_id,
        replacement_icon_buffer.buffer_id
    );
    assert!(replacement_icon_buffer.pixel_checksum.is_some());
}

#[test]
fn active_drag_icon_tracks_pointer_and_committed_surface_offset() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let _pointer = seat.get_pointer(&qh, ());
    let manager: client_wl_data_device_manager::WlDataDeviceManager =
        globals.bind(&qh, 1..=3, ()).unwrap();
    let device = manager.get_data_device(&seat, &qh, ());
    let (origin, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    let icon = compositor.create_surface(&qh, ());

    origin.commit();
    commit_test_buffered_surface(&icon, &shm, &qh, 25, 17).unwrap();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    let pointer_a = (
        f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
        f64::from(render::FIRST_SURFACE_OFFSET.1) + 20.0,
    );
    commands
        .send(ServerCommand::PointerMotion {
            x: pointer_a.0,
            y: pointer_a.1,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let serial = state
        .pointer_button_serial
        .expect("drag must use a real pointer press serial");

    device.start_drag(None, &origin, Some(&icon), serial);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue
        .roundtrip(&mut state)
        .expect("drag with a precommitted icon must start");
    let initial = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .find(|surface| (surface.width, surface.height) == (25, 17));

    icon.offset(7, -5);
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);
    let pending_offset = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .find(|surface| (surface.width, surface.height) == (25, 17));

    icon.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);
    let committed_offset = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .find(|surface| (surface.width, surface.height) == (25, 17));

    let pointer_b = (pointer_a.0 + 60.0, pointer_a.1 + 40.0);
    commands
        .send(ServerCommand::PointerMotion {
            x: pointer_b.0,
            y: pointer_b.1,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let moved = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .find(|surface| (surface.width, surface.height) == (25, 17));
    let move_cause = capture_render_generation_cause(&commands);

    let _server = stop_controllable_test_server(commands, server_thread);
    let initial = initial.expect("precommitted active icon must be rendered");
    let pending_offset = pending_offset.expect("icon remains visible with an uncommitted offset");
    let committed_offset = committed_offset.expect("icon remains visible after offset commit");
    let moved = moved.expect("visible icon remains rendered while the pointer moves");

    assert_eq!(
        (initial.origin_x, initial.origin_y),
        (pointer_a.0 as i32, pointer_a.1 as i32)
    );
    assert_eq!((initial.content_x, initial.content_y), (0, 0));
    assert_eq!(
        (pending_offset.origin_x, pending_offset.origin_y),
        (initial.origin_x, initial.origin_y),
        "wl_surface.offset must remain pending until surface.commit"
    );
    assert_eq!((pending_offset.content_x, pending_offset.content_y), (0, 0));
    assert_eq!(
        (committed_offset.content_x, committed_offset.content_y),
        (7, -5)
    );
    assert_eq!(
        (committed_offset.origin_x, committed_offset.origin_y),
        (pointer_a.0 as i32 + 7, pointer_a.1 as i32 - 5)
    );
    assert_eq!(
        (moved.origin_x, moved.origin_y),
        (pointer_b.0 as i32 + 7, pointer_b.1 as i32 - 5),
        "pointer motion must move the icon root while preserving its committed offset"
    );
    assert_eq!(moved.buffer_id, committed_offset.buffer_id);
    assert_eq!(moved.generation, committed_offset.generation);
    assert_eq!(moved.commit_sequence, committed_offset.commit_sequence);
    assert_eq!(move_cause, RenderGenerationCause::SurfacePlacement);
}

#[test]
fn active_drag_icon_input_region_never_receives_pointer_hit() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let _pointer = seat.get_pointer(&qh, ());
    let manager: client_wl_data_device_manager::WlDataDeviceManager =
        globals.bind(&qh, 1..=3, ()).unwrap();
    let device = manager.get_data_device(&seat, &qh, ());
    let (origin, origin_xdg, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    let icon = compositor.create_surface(&qh, ());

    origin.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commit_registered_initial_xdg_test_buffer(&origin_xdg);
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    focus_root_window(&commands, origin.id().protocol_id());
    set_focused_root_visual_geometry(
        &commands,
        SurfacePlacement::absolute_root_at(80, 60),
        160,
        120,
    );

    commit_test_buffered_surface(&icon, &shm, &qh, 25, 17).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let underlying_surface_id = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .find(|surface| (surface.width, surface.height) == (160, 120))
        .expect("underlying application surface must be visible")
        .surface_id;
    let pointer = (100.0, 80.0);
    commands
        .send(ServerCommand::PointerMotion {
            x: pointer.0,
            y: pointer.1,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let serial = state
        .pointer_button_serial
        .expect("drag must use a real pointer press serial");

    device.start_drag(None, &origin, Some(&icon), serial);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue
        .roundtrip(&mut state)
        .expect("drag with a committed icon must start");
    let icon_id = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .find(|surface| (surface.width, surface.height) == (25, 17))
        .expect("active DragIcon must be visible")
        .surface_id;
    let default_region_hit = capture_pointer_scene_hit(&commands, pointer.0, pointer.1).0;
    let default_region_acceptance = capture_surface_input_acceptance(&commands, icon_id, 0.0, 0.0);

    let region = compositor.create_region(&qh, ());
    region.add(0, 0, 25, 17);
    icon.set_input_region(Some(&region));
    icon.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);
    let explicit_region_hit = capture_pointer_scene_hit(&commands, pointer.0, pointer.1).0;
    let explicit_region_acceptance = capture_surface_input_acceptance(&commands, icon_id, 0.0, 0.0);

    let _server = stop_controllable_test_server(commands, server_thread);
    assert_ne!(icon_id, underlying_surface_id);
    assert_eq!(
        default_region_hit,
        Some(underlying_surface_id),
        "the default infinite DragIcon input region must not intercept the pointer"
    );
    assert!(
        !default_region_acceptance,
        "the default infinite DragIcon input region must be ignored"
    );
    assert_eq!(
        explicit_region_hit,
        Some(underlying_surface_id),
        "an explicitly-set DragIcon input region must be ignored"
    );
    assert!(
        !explicit_region_acceptance,
        "an explicitly-set DragIcon input region must be ignored"
    );
}

#[test]
fn v3_source_set_actions_then_selection_is_a_wire_protocol_error() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let pointer = seat.get_pointer(&qh, ());
    let manager: client_wl_data_device_manager::WlDataDeviceManager =
        globals.bind(&qh, 1..=3, ()).unwrap();
    let device = manager.get_data_device(&seat, &qh, ());
    let (origin, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    origin.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();

    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 20.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let serial = state.pointer_button_serial.expect("pointer press serial");

    let source = manager.create_data_source(&qh, ());
    source.offer("text/plain".to_string());
    source.set_actions(client_wl_data_device_manager::DndAction::Copy);
    device.set_selection(Some(&source), serial);
    connection.flush().unwrap();
    assert!(queue.roundtrip(&mut state).is_err());

    let _server = stop_controllable_test_server(commands, server_thread);
    let _ = pointer;
}

#[test]
fn clipboard_selection_after_unrelated_input_uses_a_fresh_mutation_epoch() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let _keyboard = seat.get_keyboard(&qh, ());
    let manager: client_wl_data_device_manager::WlDataDeviceManager =
        globals.bind(&qh, 1..=3, ()).unwrap();
    let device = manager.get_data_device(&seat, &qh, ());
    let source = manager.create_data_source(&qh, ());
    source.offer("text/plain".to_string());
    let replacement = manager.create_data_source(&qh, ());
    replacement.offer("text/plain".to_string());
    let (surface, xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    surface.commit();
    connection.flush().unwrap();

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commit_registered_initial_xdg_test_buffer(&xdg_surface);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    commands
        .send(ServerCommand::KeyboardKey {
            key: 30,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let old_serial = state.keyboard_key_serial.expect("focused key serial");

    device.set_selection(Some(&source), old_serial);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.data_device_selection_events, [false, true]);
    let committed_epoch = capture_clipboard_state(&commands).mutation_epoch;

    for key in 31..50 {
        commands
            .send(ServerCommand::KeyboardKey { key, pressed: true })
            .unwrap();
        wait_for_server_commands(&commands);
        queue.roundtrip(&mut state).unwrap();
    }
    assert_eq!(
        capture_clipboard_state(&commands).mutation_epoch,
        committed_epoch,
        "ordinary input events must not allocate selection mutation epochs"
    );

    device.set_selection(Some(&replacement), old_serial);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    assert_eq!(
        state.data_device_selection_events,
        [false, true, true],
        "a focused client may replace its clipboard after unrelated input churn"
    );
    assert_eq!(
        state.data_source_cancelled_count, 1,
        "replacing the active source cancels it exactly once"
    );
    assert!(capture_clipboard_state(&commands).mutation_epoch > committed_epoch);

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn client_teardown_retires_its_core_primary_and_data_control_offers() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
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
    let source_shm: client_wl_shm::WlShm = source_globals.bind(&source_qh, 1..=1, ()).unwrap();
    let source_seat: client_wl_seat::WlSeat = source_globals.bind(&source_qh, 1..=7, ()).unwrap();
    let _source_keyboard = source_seat.get_keyboard(&source_qh, ());
    let source_manager: client_wl_data_device_manager::WlDataDeviceManager =
        source_globals.bind(&source_qh, 1..=3, ()).unwrap();
    let source_device = source_manager.get_data_device(&source_seat, &source_qh, ());
    let source_primary_manager: client_zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1 =
        source_globals.bind(&source_qh, 1..=1, ()).unwrap();
    let source_primary_device = source_primary_manager.get_device(&source_seat, &source_qh, ());
    let clipboard_source = source_manager.create_data_source(&source_qh, ());
    clipboard_source.offer("text/plain".to_string());
    let primary_source = source_primary_manager.create_source(&source_qh, ());
    primary_source.offer("text/plain".to_string());
    let (source_surface, source_xdg_surface, _source_toplevel) = create_test_buffered_toplevel(
        &source_compositor,
        &source_wm_base,
        &source_shm,
        &source_qh,
        160,
        120,
    )
    .unwrap();
    source_surface.commit();
    source_connection.flush().unwrap();
    let mut source_state = RegistryTestState::default();
    source_queue.roundtrip(&mut source_state).unwrap();
    commit_registered_initial_xdg_test_buffer(&source_xdg_surface);
    source_connection.flush().unwrap();
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    let source_serial = source_state
        .keyboard_enter_serial
        .expect("source owner is initially focused");
    source_device.set_selection(Some(&clipboard_source), source_serial);
    source_primary_device.set_selection(Some(&primary_source), source_serial);
    source_connection.flush().unwrap();
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();

    let selection_before_disconnect = {
        let target_connection =
            Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
        let (target_globals, mut target_queue) =
            registry_queue_init::<RegistryTestState>(&target_connection).unwrap();
        let target_qh = target_queue.handle();
        let target_compositor: client_wl_compositor::WlCompositor =
            target_globals.bind(&target_qh, 1..=6, ()).unwrap();
        let target_wm_base: client_xdg_wm_base::XdgWmBase =
            target_globals.bind(&target_qh, 1..=6, ()).unwrap();
        let target_shm: client_wl_shm::WlShm = target_globals.bind(&target_qh, 1..=1, ()).unwrap();
        let target_seat: client_wl_seat::WlSeat =
            target_globals.bind(&target_qh, 1..=7, ()).unwrap();
        let _target_keyboard = target_seat.get_keyboard(&target_qh, ());
        let target_data_manager: client_wl_data_device_manager::WlDataDeviceManager =
            target_globals.bind(&target_qh, 1..=3, ()).unwrap();
        let _target_device = target_data_manager.get_data_device(&target_seat, &target_qh, ());
        let target_primary_manager: client_zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1 =
            target_globals.bind(&target_qh, 1..=1, ()).unwrap();
        let _target_primary_device =
            target_primary_manager.get_device(&target_seat, &target_qh, ());
        let target_data_control_manager: client_ext_data_control_manager_v1::ExtDataControlManagerV1 =
            target_globals.bind(&target_qh, 1..=1, ()).unwrap();
        let _target_control_device =
            target_data_control_manager.get_data_device(&target_seat, &target_qh, ());
        let (target_surface, target_xdg_surface, _target_toplevel) = create_test_buffered_toplevel(
            &target_compositor,
            &target_wm_base,
            &target_shm,
            &target_qh,
            160,
            120,
        )
        .unwrap();
        target_surface.commit();
        target_connection.flush().unwrap();
        let mut target_state = RegistryTestState::default();
        target_queue.roundtrip(&mut target_state).unwrap();
        commit_registered_initial_xdg_test_buffer(&target_xdg_surface);
        target_connection.flush().unwrap();
        focus_root_window(&commands, target_surface.id().protocol_id());
        wait_for_server_commands(&commands);
        target_queue.roundtrip(&mut target_state).unwrap();
        source_queue.roundtrip(&mut source_state).unwrap();

        let before_disconnect = capture_clipboard_state(&commands);
        assert_eq!(before_disconnect.clipboard_broker_offer_count, 2);
        assert_eq!(before_disconnect.primary_broker_offer_count, 2);
        assert!(target_state.data_device_selection_offer.is_some());
        assert!(target_state.primary_selection_offer.is_some());
        assert!(target_state.data_control_clipboard_offer.is_some());
        assert!(target_state.data_control_primary_offer.is_some());
        before_disconnect
    };

    wait_for_server_commands(&commands);
    let after_disconnect = capture_clipboard_state(&commands);
    assert_eq!(after_disconnect.clipboard_broker_offer_count, 0);
    assert_eq!(after_disconnect.primary_broker_offer_count, 0);
    assert!(after_disconnect.active_source);
    assert_eq!(
        after_disconnect.generation,
        selection_before_disconnect.generation
    );
    assert_eq!(
        after_disconnect.mutation_epoch,
        selection_before_disconnect.mutation_epoch
    );
    assert_eq!(
        after_disconnect.primary_generation,
        selection_before_disconnect.primary_generation
    );
    assert_eq!(
        after_disconnect.primary_mutation_epoch,
        selection_before_disconnect.primary_mutation_epoch
    );

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn removing_one_core_device_preserves_sibling_offers_and_transfers() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let _keyboard = seat.get_keyboard(&qh, ());
    let manager: client_wl_data_device_manager::WlDataDeviceManager =
        globals.bind(&qh, 1..=3, ()).unwrap();
    let device_a = manager.get_data_device(&seat, &qh, ());
    let device_b = manager.get_data_device(&seat, &qh, ());
    let source = manager.create_data_source(&qh, ());
    source.offer("text/plain".to_string());
    let (surface, xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    surface.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commit_registered_initial_xdg_test_buffer(&xdg_surface);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    let serial = state
        .keyboard_enter_serial
        .expect("focused client receives a keyboard enter serial");
    device_a.set_selection(Some(&source), serial);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    let offer_for_device = |state: &RegistryTestState, device_id| {
        state
            .data_device_selection_offers
            .iter()
            .rev()
            .find_map(|(id, offer)| (*id == device_id).then(|| offer.clone()).flatten())
            .expect("core data device receives the current clipboard offer")
    };
    let offer_a = offer_for_device(&state, device_a.id().protocol_id());
    let offer_b = offer_for_device(&state, device_b.id().protocol_id());
    let before = capture_clipboard_state(&commands);
    assert_eq!(before.clipboard_broker_offer_count, 2);

    device_a.release();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let after_device_a_destroy = capture_clipboard_state(&commands);
    assert_eq!(after_device_a_destroy.clipboard_broker_offer_count, 1);
    assert_eq!(after_device_a_destroy.primary_broker_offer_count, 0);
    assert_eq!(after_device_a_destroy.generation, before.generation);
    assert_eq!(after_device_a_destroy.mutation_epoch, before.mutation_epoch);

    let (stale_read_fd, stale_write_fd) = owned_pipe().unwrap();
    offer_a.receive("text/plain".to_string(), stale_write_fd.as_fd());
    connection.flush().unwrap();
    drop(stale_write_fd);
    queue.roundtrip(&mut state).unwrap();
    let mut stale_payload = String::new();
    File::from(stale_read_fd)
        .read_to_string(&mut stale_payload)
        .unwrap();
    assert!(stale_payload.is_empty());
    assert!(state.data_source_send_mime_types.is_empty());

    let (read_fd, write_fd) = owned_pipe().unwrap();
    offer_b.receive("text/plain".to_string(), write_fd.as_fd());
    connection.flush().unwrap();
    drop(write_fd);
    queue.roundtrip(&mut state).unwrap();
    let mut live_payload = String::new();
    File::from(read_fd)
        .read_to_string(&mut live_payload)
        .unwrap();
    assert_eq!(live_payload, "clipboard payload");
    assert_eq!(state.data_source_send_mime_types, ["text/plain"]);

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn destroying_selection_offers_retires_only_the_broker_offer() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let _keyboard = seat.get_keyboard(&qh, ());
    let data_manager: client_wl_data_device_manager::WlDataDeviceManager =
        globals.bind(&qh, 1..=3, ()).unwrap();
    let data_device = data_manager.get_data_device(&seat, &qh, ());
    let primary_manager: client_zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let primary_device = primary_manager.get_device(&seat, &qh, ());
    let data_control_manager: client_ext_data_control_manager_v1::ExtDataControlManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let _data_control_device = data_control_manager.get_data_device(&seat, &qh, ());
    let clipboard_source = data_manager.create_data_source(&qh, ());
    clipboard_source.offer("text/plain".to_string());
    let primary_source = primary_manager.create_source(&qh, ());
    primary_source.offer("text/plain".to_string());
    let (surface, xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    surface.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commit_registered_initial_xdg_test_buffer(&xdg_surface);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    let serial = state
        .keyboard_enter_serial
        .expect("the focused client receives a keyboard enter serial");
    data_device.set_selection(Some(&clipboard_source), serial);
    primary_device.set_selection(Some(&primary_source), serial);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    let clipboard_offer = state
        .data_device_selection_offer
        .clone()
        .expect("core data device receives the clipboard offer");
    let primary_offer = state
        .primary_selection_offer
        .clone()
        .expect("primary device receives the PRIMARY offer");
    let control_clipboard_offer = state
        .data_control_clipboard_offer
        .clone()
        .expect("data-control device receives the clipboard offer");
    let control_primary_offer = state
        .data_control_primary_offer
        .clone()
        .expect("data-control device receives the PRIMARY offer");
    let before = capture_clipboard_state(&commands);
    assert_eq!(before.clipboard_broker_offer_count, 2);
    assert_eq!(before.primary_broker_offer_count, 2);

    clipboard_offer.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let after_core_destroy = capture_clipboard_state(&commands);
    assert_eq!(after_core_destroy.clipboard_broker_offer_count, 1);
    assert_eq!(after_core_destroy.primary_broker_offer_count, 2);

    primary_offer.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let after_primary_destroy = capture_clipboard_state(&commands);
    assert_eq!(after_primary_destroy.clipboard_broker_offer_count, 1);
    assert_eq!(after_primary_destroy.primary_broker_offer_count, 1);

    control_clipboard_offer.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let after_control_clipboard_destroy = capture_clipboard_state(&commands);
    assert_eq!(
        after_control_clipboard_destroy.clipboard_broker_offer_count,
        0
    );
    assert_eq!(
        after_control_clipboard_destroy.primary_broker_offer_count,
        1
    );

    control_primary_offer.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let after_control_destroy = capture_clipboard_state(&commands);
    assert_eq!(after_control_destroy.clipboard_broker_offer_count, 0);
    assert_eq!(after_control_destroy.primary_broker_offer_count, 0);
    for after in [
        after_core_destroy,
        after_primary_destroy,
        after_control_destroy,
    ] {
        assert_eq!(after.generation, before.generation);
        assert_eq!(after.mutation_epoch, before.mutation_epoch);
        assert_eq!(after.primary_generation, before.primary_generation);
        assert_eq!(after.primary_mutation_epoch, before.primary_mutation_epoch);
    }

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn focus_transfer_retires_old_core_offers_without_mutating_selections() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
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
    let source_shm: client_wl_shm::WlShm = source_globals.bind(&source_qh, 1..=1, ()).unwrap();
    let source_seat: client_wl_seat::WlSeat = source_globals.bind(&source_qh, 1..=7, ()).unwrap();
    let _source_keyboard = source_seat.get_keyboard(&source_qh, ());
    let source_manager: client_wl_data_device_manager::WlDataDeviceManager =
        source_globals.bind(&source_qh, 1..=3, ()).unwrap();
    let source_device = source_manager.get_data_device(&source_seat, &source_qh, ());
    let source_primary_manager: client_zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1 =
        source_globals.bind(&source_qh, 1..=1, ()).unwrap();
    let source_primary_device = source_primary_manager.get_device(&source_seat, &source_qh, ());
    let source_data: client_ext_data_control_manager_v1::ExtDataControlManagerV1 =
        source_globals.bind(&source_qh, 1..=1, ()).unwrap();
    let _source_control_device = source_data.get_data_device(&source_seat, &source_qh, ());
    let source = source_manager.create_data_source(&source_qh, ());
    source.offer("text/plain".to_string());
    let replacement = source_manager.create_data_source(&source_qh, ());
    replacement.offer("text/plain".to_string());
    let source_primary = source_primary_manager.create_source(&source_qh, ());
    source_primary.offer("text/plain".to_string());
    let (source_surface, source_xdg_surface, _source_toplevel) = create_test_buffered_toplevel(
        &source_compositor,
        &source_wm_base,
        &source_shm,
        &source_qh,
        160,
        120,
    )
    .unwrap();
    source_surface.commit();
    source_connection.flush().unwrap();
    let mut source_state = RegistryTestState::default();
    source_queue.roundtrip(&mut source_state).unwrap();
    commit_registered_initial_xdg_test_buffer(&source_xdg_surface);
    source_connection.flush().unwrap();
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();

    let serial = source_state
        .keyboard_enter_serial
        .expect("keyboard enter serial should be visible to the focused client");
    source_device.set_selection(Some(&source), serial);
    source_connection.flush().unwrap();
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    let old_offer = source_state
        .data_device_selection_offer
        .clone()
        .expect("focused owner receives the current selection offer");
    assert_eq!(source_state.data_device_selection_events, [false, true]);
    assert_eq!(source_state.data_control_selection_events, [false, true]);
    source_primary_device.set_selection(Some(&source_primary), serial);
    source_connection.flush().unwrap();
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    let old_primary_offer = source_state
        .primary_selection_offer
        .clone()
        .expect("focused owner receives the current PRIMARY offer");
    assert_eq!(source_state.primary_selection_events, [false, true]);
    assert_eq!(
        source_state.data_control_primary_selection_events,
        [false, true]
    );
    let selection_before_focus = capture_clipboard_state(&commands);

    let target_connection =
        Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (target_globals, mut target_queue) =
        registry_queue_init::<RegistryTestState>(&target_connection).unwrap();
    let target_qh = target_queue.handle();
    let target_compositor: client_wl_compositor::WlCompositor =
        target_globals.bind(&target_qh, 1..=6, ()).unwrap();
    let target_wm_base: client_xdg_wm_base::XdgWmBase =
        target_globals.bind(&target_qh, 1..=6, ()).unwrap();
    let target_shm: client_wl_shm::WlShm = target_globals.bind(&target_qh, 1..=1, ()).unwrap();
    let target_seat: client_wl_seat::WlSeat = target_globals.bind(&target_qh, 1..=7, ()).unwrap();
    let _target_keyboard = target_seat.get_keyboard(&target_qh, ());
    let target_manager: client_wl_data_device_manager::WlDataDeviceManager =
        target_globals.bind(&target_qh, 1..=3, ()).unwrap();
    let _target_device = target_manager.get_data_device(&target_seat, &target_qh, ());
    let target_primary_manager: client_zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1 =
        target_globals.bind(&target_qh, 1..=1, ()).unwrap();
    let _target_primary_device = target_primary_manager.get_device(&target_seat, &target_qh, ());
    let (target_surface, target_xdg_surface, _target_toplevel) = create_test_buffered_toplevel(
        &target_compositor,
        &target_wm_base,
        &target_shm,
        &target_qh,
        160,
        120,
    )
    .unwrap();
    target_surface.commit();
    target_connection.flush().unwrap();
    let mut target_state = RegistryTestState::default();
    target_queue.roundtrip(&mut target_state).unwrap();
    commit_registered_initial_xdg_test_buffer(&target_xdg_surface);
    target_connection.flush().unwrap();
    focus_root_window(&commands, target_surface.id().protocol_id());
    wait_for_server_commands(&commands);
    target_queue.roundtrip(&mut target_state).unwrap();
    source_queue.roundtrip(&mut source_state).unwrap();

    assert_eq!(target_state.data_device_selection_events, [true]);
    assert_eq!(
        source_state.data_device_selection_events,
        [false, true, false]
    );
    assert_eq!(target_state.primary_selection_events, [true]);
    assert_eq!(source_state.primary_selection_events, [false, true, false]);
    assert_eq!(
        capture_clipboard_state(&commands).generation,
        selection_before_focus.generation,
        "focus-only publication must preserve the canonical selection generation"
    );
    assert_eq!(
        capture_clipboard_state(&commands).mutation_epoch,
        selection_before_focus.mutation_epoch,
        "focus-only publication must not allocate a selection mutation epoch"
    );
    let selection_after_focus = capture_clipboard_state(&commands);
    assert_eq!(
        selection_after_focus.primary_generation, selection_before_focus.primary_generation,
        "focus-only publication must preserve the PRIMARY selection generation"
    );
    assert_eq!(
        selection_after_focus.primary_mutation_epoch, selection_before_focus.primary_mutation_epoch,
        "focus-only publication must not allocate a PRIMARY mutation epoch"
    );
    assert_eq!(
        source_state.data_control_selection_events,
        [false, true],
        "focus transfer must not republish a data-control mutation"
    );
    assert_eq!(
        source_state.data_control_primary_selection_events,
        [false, true],
        "focus transfer must not republish a PRIMARY data-control mutation"
    );

    let (read_fd, write_fd) = owned_pipe().unwrap();
    old_offer.receive("text/plain".to_string(), write_fd.as_fd());
    source_connection.flush().unwrap();
    drop(write_fd);
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    let mut stale_payload = String::new();
    File::from(read_fd)
        .read_to_string(&mut stale_payload)
        .unwrap();
    assert!(stale_payload.is_empty());
    assert!(source_state.data_source_send_mime_types.is_empty());

    let (primary_read_fd, primary_write_fd) = owned_pipe().unwrap();
    old_primary_offer.receive("text/plain".to_string(), primary_write_fd.as_fd());
    source_connection.flush().unwrap();
    drop(primary_write_fd);
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    let mut stale_primary_payload = String::new();
    File::from(primary_read_fd)
        .read_to_string(&mut stale_primary_payload)
        .unwrap();
    assert!(stale_primary_payload.is_empty());
    assert!(source_state.primary_source_send_mime_types.is_empty());

    source_device.set_selection(Some(&replacement), serial);
    source_connection.flush().unwrap();
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    target_queue.roundtrip(&mut target_state).unwrap();
    assert_eq!(target_state.data_device_selection_events, [true]);
    assert_eq!(
        capture_clipboard_state(&commands).generation,
        selection_before_focus.generation
    );
    assert_eq!(source_state.data_control_selection_events, [false, true]);

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn v3_start_drag_without_set_actions_is_a_wire_protocol_error() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let pointer = seat.get_pointer(&qh, ());
    let manager: client_wl_data_device_manager::WlDataDeviceManager =
        globals.bind(&qh, 1..=3, ()).unwrap();
    let device = manager.get_data_device(&seat, &qh, ());
    let (origin, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    origin.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();

    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 20.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let serial = state.pointer_button_serial.expect("pointer press serial");

    let source = manager.create_data_source(&qh, ());
    source.offer("text/plain".to_string());
    device.start_drag(Some(&source), &origin, None, serial);
    connection.flush().unwrap();
    assert!(queue.roundtrip(&mut state).is_err());

    let _server = stop_controllable_test_server(commands, server_thread);
    let _ = pointer;
}

#[test]
fn pre_v3_source_can_start_drag_without_set_actions() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let pointer = seat.get_pointer(&qh, ());
    let manager: client_wl_data_device_manager::WlDataDeviceManager =
        globals.bind(&qh, 2..=2, ()).unwrap();
    let device = manager.get_data_device(&seat, &qh, ());
    let (origin, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    origin.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();

    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 20.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let serial = state.pointer_button_serial.expect("pointer press serial");

    let source = manager.create_data_source(&qh, ());
    source.offer("text/plain".to_string());
    device.start_drag(Some(&source), &origin, None, serial);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue
        .roundtrip(&mut state)
        .expect("v2 drag must not require the v3 set_actions request");

    let _server = stop_controllable_test_server(commands, server_thread);
    let _ = pointer;
}

#[test]
fn sourced_wire_drag_target_disconnect_after_drop_cancels_once() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
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
    let source_shm: client_wl_shm::WlShm = source_globals.bind(&source_qh, 1..=1, ()).unwrap();
    let source_seat: client_wl_seat::WlSeat = source_globals.bind(&source_qh, 1..=7, ()).unwrap();
    let source_pointer = source_seat.get_pointer(&source_qh, ());
    let source_manager: client_wl_data_device_manager::WlDataDeviceManager =
        source_globals.bind(&source_qh, 1..=3, ()).unwrap();
    let source_device = source_manager.get_data_device(&source_seat, &source_qh, ());
    let (source_surface, source_xdg_surface, _source_toplevel) = create_test_buffered_toplevel(
        &source_compositor,
        &source_wm_base,
        &source_shm,
        &source_qh,
        160,
        120,
    )
    .unwrap();
    let source = source_manager.create_data_source(&source_qh, ());
    source.offer("text/plain".to_string());
    source.set_actions(
        client_wl_data_device_manager::DndAction::Copy
            | client_wl_data_device_manager::DndAction::Move,
    );
    source_surface.commit();
    source_connection.flush().unwrap();
    let mut source_state = RegistryTestState::default();
    source_queue.roundtrip(&mut source_state).unwrap();
    commit_registered_initial_xdg_test_buffer(&source_xdg_surface);
    source_connection.flush().unwrap();
    source_queue.roundtrip(&mut source_state).unwrap();

    let target_connection =
        Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (target_globals, mut target_queue) =
        registry_queue_init::<RegistryTestState>(&target_connection).unwrap();
    let target_qh = target_queue.handle();
    let target_compositor: client_wl_compositor::WlCompositor =
        target_globals.bind(&target_qh, 1..=6, ()).unwrap();
    let target_wm_base: client_xdg_wm_base::XdgWmBase =
        target_globals.bind(&target_qh, 1..=6, ()).unwrap();
    let target_shm: client_wl_shm::WlShm = target_globals.bind(&target_qh, 1..=1, ()).unwrap();
    let target_seat: client_wl_seat::WlSeat = target_globals.bind(&target_qh, 1..=7, ()).unwrap();
    let target_manager: client_wl_data_device_manager::WlDataDeviceManager =
        target_globals.bind(&target_qh, 1..=3, ()).unwrap();
    let target_device = target_manager.get_data_device(&target_seat, &target_qh, ());
    let (target_surface, target_xdg_surface, _target_toplevel) = create_test_buffered_toplevel(
        &target_compositor,
        &target_wm_base,
        &target_shm,
        &target_qh,
        160,
        120,
    )
    .unwrap();
    target_surface.commit();
    target_connection.flush().unwrap();
    let mut target_state = RegistryTestState::default();
    target_queue.roundtrip(&mut target_state).unwrap();
    commit_registered_initial_xdg_test_buffer(&target_xdg_surface);
    target_connection.flush().unwrap();
    target_queue.roundtrip(&mut target_state).unwrap();

    let target_surface_id = target_surface.id().protocol_id();
    let source_surface_id = source_surface.id().protocol_id();
    focus_root_window(&commands, target_surface_id);
    set_focused_root_visual_geometry(
        &commands,
        SurfacePlacement::absolute_root_at(300, 200),
        160,
        120,
    );
    focus_root_window(&commands, source_surface_id);

    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 20.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    let serial = source_state
        .pointer_button_serial
        .expect("source drag must use the real pointer press serial");

    source_device.start_drag(Some(&source), &source_surface, None, serial);
    source_connection.flush().unwrap();
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    target_queue.roundtrip(&mut target_state).unwrap();
    assert!(source_state.data_source_actions.is_empty());

    commands
        .send(ServerCommand::PointerMotion { x: 320.0, y: 220.0 })
        .unwrap();
    wait_for_server_commands(&commands);
    target_queue.roundtrip(&mut target_state).unwrap();
    source_queue.roundtrip(&mut source_state).unwrap();
    assert_eq!(target_state.data_device_enter_count, 1);
    assert_eq!(target_state.data_offer_mime_types, vec!["text/plain"]);
    assert_eq!(target_state.data_offer_source_actions, vec![1 | 2]);
    assert!(target_state.data_offer_actions.is_empty());
    assert!(source_state.data_source_actions.is_empty());

    let offer = target_state
        .data_device_drag_offer
        .clone()
        .expect("target must receive a DnD offer");
    let enter_serial = target_state
        .data_device_enter_serial
        .expect("target must receive an enter serial");
    offer.accept(enter_serial, Some("text/plain".to_string()));
    offer.set_actions(
        client_wl_data_device_manager::DndAction::Copy
            | client_wl_data_device_manager::DndAction::Move,
        client_wl_data_device_manager::DndAction::Move,
    );
    target_connection.flush().unwrap();
    wait_for_server_commands(&commands);
    target_queue.roundtrip(&mut target_state).unwrap();
    source_queue.roundtrip(&mut source_state).unwrap();
    assert_eq!(target_state.data_offer_actions, vec![2]);
    assert_eq!(source_state.data_source_actions, vec![2]);

    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: false,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    target_queue.roundtrip(&mut target_state).unwrap();
    source_queue.roundtrip(&mut source_state).unwrap();
    assert_eq!(target_state.data_device_drop_count, 1);
    assert_eq!(source_state.data_source_dnd_drop_performed_count, 1);

    drop(target_state);
    drop(target_queue);
    drop(target_connection);
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    assert_eq!(source_state.data_source_cancelled_count, 1);
    assert_eq!(source_state.data_source_dnd_finished_count, 0);
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    assert_eq!(source_state.data_source_cancelled_count, 1);

    let _server = stop_controllable_test_server(commands, server_thread);
    let _ = source_pointer;
    let _ = target_device;
}

#[test]
fn sourced_wire_drag_accept_rejects_unoffered_mime() {
    let protocol_error = run_sourced_wire_drag_accept_probe(DndAcceptProbe::UnofferedMime);
    assert_eq!(
        protocol_error.code,
        client_wl_data_offer::Error::InvalidOffer as u32
    );
    assert_eq!(
        protocol_error.message,
        "data offer MIME type was not offered"
    );
}

#[test]
fn sourced_wire_drag_accept_after_finish_is_fatal() {
    let protocol_error = run_sourced_wire_drag_accept_probe(DndAcceptProbe::AfterFinish);
    assert_eq!(
        protocol_error.code,
        client_wl_data_offer::Error::InvalidOffer as u32
    );
}

#[derive(Clone, Copy)]
enum DndAcceptProbe {
    UnofferedMime,
    AfterFinish,
}

fn run_sourced_wire_drag_accept_probe(probe: DndAcceptProbe) -> ProtocolErrorObservation {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
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
    let source_shm: client_wl_shm::WlShm = source_globals.bind(&source_qh, 1..=1, ()).unwrap();
    let source_seat: client_wl_seat::WlSeat = source_globals.bind(&source_qh, 1..=7, ()).unwrap();
    let source_pointer = source_seat.get_pointer(&source_qh, ());
    let source_manager: client_wl_data_device_manager::WlDataDeviceManager =
        source_globals.bind(&source_qh, 1..=3, ()).unwrap();
    let source_device = source_manager.get_data_device(&source_seat, &source_qh, ());
    let (source_surface, source_xdg_surface, _source_toplevel) = create_test_buffered_toplevel(
        &source_compositor,
        &source_wm_base,
        &source_shm,
        &source_qh,
        160,
        120,
    )
    .unwrap();
    let source = source_manager.create_data_source(&source_qh, ());
    source.offer("text/plain".to_string());
    source.set_actions(
        client_wl_data_device_manager::DndAction::Copy
            | client_wl_data_device_manager::DndAction::Move,
    );
    source_surface.commit();
    source_connection.flush().unwrap();
    let mut source_state = RegistryTestState::default();
    source_queue.roundtrip(&mut source_state).unwrap();
    commit_registered_initial_xdg_test_buffer(&source_xdg_surface);
    source_connection.flush().unwrap();
    source_queue.roundtrip(&mut source_state).unwrap();

    let target_connection =
        Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (target_globals, mut target_queue) =
        registry_queue_init::<RegistryTestState>(&target_connection).unwrap();
    let target_qh = target_queue.handle();
    let target_compositor: client_wl_compositor::WlCompositor =
        target_globals.bind(&target_qh, 1..=6, ()).unwrap();
    let target_wm_base: client_xdg_wm_base::XdgWmBase =
        target_globals.bind(&target_qh, 1..=6, ()).unwrap();
    let target_shm: client_wl_shm::WlShm = target_globals.bind(&target_qh, 1..=1, ()).unwrap();
    let target_seat: client_wl_seat::WlSeat = target_globals.bind(&target_qh, 1..=7, ()).unwrap();
    let target_manager: client_wl_data_device_manager::WlDataDeviceManager =
        target_globals.bind(&target_qh, 1..=3, ()).unwrap();
    let target_device = target_manager.get_data_device(&target_seat, &target_qh, ());
    let (target_surface, target_xdg_surface, _target_toplevel) = create_test_buffered_toplevel(
        &target_compositor,
        &target_wm_base,
        &target_shm,
        &target_qh,
        160,
        120,
    )
    .unwrap();
    target_surface.commit();
    target_connection.flush().unwrap();
    let mut target_state = RegistryTestState::default();
    target_queue.roundtrip(&mut target_state).unwrap();
    commit_registered_initial_xdg_test_buffer(&target_xdg_surface);
    target_connection.flush().unwrap();
    target_queue.roundtrip(&mut target_state).unwrap();

    let target_surface_id = target_surface.id().protocol_id();
    let source_surface_id = source_surface.id().protocol_id();
    focus_root_window(&commands, target_surface_id);
    set_focused_root_visual_geometry(
        &commands,
        SurfacePlacement::absolute_root_at(300, 200),
        160,
        120,
    );
    focus_root_window(&commands, source_surface_id);
    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 20.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    let serial = source_state
        .pointer_button_serial
        .expect("source drag must use the real pointer press serial");

    source_device.start_drag(Some(&source), &source_surface, None, serial);
    source_connection.flush().unwrap();
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    target_queue.roundtrip(&mut target_state).unwrap();
    commands
        .send(ServerCommand::PointerMotion { x: 320.0, y: 220.0 })
        .unwrap();
    wait_for_server_commands(&commands);
    target_queue.roundtrip(&mut target_state).unwrap();
    source_queue.roundtrip(&mut source_state).unwrap();

    let offer = target_state
        .data_device_drag_offer
        .clone()
        .expect("target must receive a DnD offer");
    let enter_serial = target_state
        .data_device_enter_serial
        .expect("target must receive an enter serial");

    match probe {
        DndAcceptProbe::UnofferedMime => {
            offer.accept(enter_serial, Some("application/x-unoffered".to_string()));
        }
        DndAcceptProbe::AfterFinish => {
            offer.accept(enter_serial, Some("text/plain".to_string()));
            offer.set_actions(
                client_wl_data_device_manager::DndAction::Copy,
                client_wl_data_device_manager::DndAction::Copy,
            );
            target_connection.flush().unwrap();
            wait_for_server_commands(&commands);
            target_queue.roundtrip(&mut target_state).unwrap();
            source_queue.roundtrip(&mut source_state).unwrap();

            commands
                .send(ServerCommand::PointerButton {
                    button: 0x110,
                    pressed: false,
                })
                .unwrap();
            wait_for_server_commands(&commands);
            target_queue.roundtrip(&mut target_state).unwrap();
            source_queue.roundtrip(&mut source_state).unwrap();
            assert_eq!(target_state.data_device_drop_count, 1);

            offer.finish();
            target_connection.flush().unwrap();
            wait_for_server_commands(&commands);
            target_queue.roundtrip(&mut target_state).unwrap();
            source_queue.roundtrip(&mut source_state).unwrap();
            assert_eq!(source_state.data_source_dnd_finished_count, 1);

            offer.accept(enter_serial, Some("text/plain".to_string()));
        }
    }
    target_connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let protocol_error = expect_protocol_error(
        &target_connection,
        "wl_data_offer",
        client_wl_data_offer::Error::InvalidOffer as u32,
    );
    let _server = stop_controllable_test_server(commands, server_thread);
    let _ = source_pointer;
    let _ = target_device;
    protocol_error
}

#[test]
fn sourced_wire_drag_post_drop_set_actions_preserves_frozen_action() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
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
    let source_shm: client_wl_shm::WlShm = source_globals.bind(&source_qh, 1..=1, ()).unwrap();
    let source_seat: client_wl_seat::WlSeat = source_globals.bind(&source_qh, 1..=7, ()).unwrap();
    let source_pointer = source_seat.get_pointer(&source_qh, ());
    let source_manager: client_wl_data_device_manager::WlDataDeviceManager =
        source_globals.bind(&source_qh, 1..=3, ()).unwrap();
    let source_device = source_manager.get_data_device(&source_seat, &source_qh, ());
    let (source_surface, source_xdg_surface, _source_toplevel) = create_test_buffered_toplevel(
        &source_compositor,
        &source_wm_base,
        &source_shm,
        &source_qh,
        160,
        120,
    )
    .unwrap();
    let source = source_manager.create_data_source(&source_qh, ());
    source.offer("text/plain".to_string());
    source.set_actions(
        client_wl_data_device_manager::DndAction::Copy
            | client_wl_data_device_manager::DndAction::Move,
    );
    source_surface.commit();
    source_connection.flush().unwrap();
    let mut source_state = RegistryTestState::default();
    source_queue.roundtrip(&mut source_state).unwrap();
    commit_registered_initial_xdg_test_buffer(&source_xdg_surface);
    source_connection.flush().unwrap();
    source_queue.roundtrip(&mut source_state).unwrap();

    let target_connection =
        Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (target_globals, mut target_queue) =
        registry_queue_init::<RegistryTestState>(&target_connection).unwrap();
    let target_qh = target_queue.handle();
    let target_compositor: client_wl_compositor::WlCompositor =
        target_globals.bind(&target_qh, 1..=6, ()).unwrap();
    let target_wm_base: client_xdg_wm_base::XdgWmBase =
        target_globals.bind(&target_qh, 1..=6, ()).unwrap();
    let target_shm: client_wl_shm::WlShm = target_globals.bind(&target_qh, 1..=1, ()).unwrap();
    let target_seat: client_wl_seat::WlSeat = target_globals.bind(&target_qh, 1..=7, ()).unwrap();
    let target_manager: client_wl_data_device_manager::WlDataDeviceManager =
        target_globals.bind(&target_qh, 1..=3, ()).unwrap();
    let target_device = target_manager.get_data_device(&target_seat, &target_qh, ());
    let (target_surface, target_xdg_surface, _target_toplevel) = create_test_buffered_toplevel(
        &target_compositor,
        &target_wm_base,
        &target_shm,
        &target_qh,
        160,
        120,
    )
    .unwrap();
    target_surface.commit();
    target_connection.flush().unwrap();
    let mut target_state = RegistryTestState::default();
    target_queue.roundtrip(&mut target_state).unwrap();
    commit_registered_initial_xdg_test_buffer(&target_xdg_surface);
    target_connection.flush().unwrap();
    target_queue.roundtrip(&mut target_state).unwrap();

    let target_surface_id = target_surface.id().protocol_id();
    let source_surface_id = source_surface.id().protocol_id();
    focus_root_window(&commands, target_surface_id);
    set_focused_root_visual_geometry(
        &commands,
        SurfacePlacement::absolute_root_at(300, 200),
        160,
        120,
    );
    focus_root_window(&commands, source_surface_id);

    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 20.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    let serial = source_state
        .pointer_button_serial
        .expect("source drag must use the real pointer press serial");

    source_device.start_drag(Some(&source), &source_surface, None, serial);
    source_connection.flush().unwrap();
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    target_queue.roundtrip(&mut target_state).unwrap();
    assert!(source_state.data_source_actions.is_empty());

    commands
        .send(ServerCommand::PointerMotion { x: 320.0, y: 220.0 })
        .unwrap();
    wait_for_server_commands(&commands);
    target_queue.roundtrip(&mut target_state).unwrap();
    source_queue.roundtrip(&mut source_state).unwrap();
    assert_eq!(target_state.data_device_enter_count, 1);
    assert_eq!(target_state.data_offer_mime_types, vec!["text/plain"]);
    assert_eq!(target_state.data_offer_source_actions, vec![1 | 2]);
    assert!(target_state.data_offer_actions.is_empty());
    assert!(source_state.data_source_actions.is_empty());

    let offer = target_state
        .data_device_drag_offer
        .clone()
        .expect("target must receive a DnD offer");
    let enter_serial = target_state
        .data_device_enter_serial
        .expect("target must receive an enter serial");
    offer.accept(enter_serial, Some("text/plain".to_string()));
    offer.set_actions(
        client_wl_data_device_manager::DndAction::Copy
            | client_wl_data_device_manager::DndAction::Move,
        client_wl_data_device_manager::DndAction::Move,
    );
    target_connection.flush().unwrap();
    wait_for_server_commands(&commands);
    target_queue.roundtrip(&mut target_state).unwrap();
    source_queue.roundtrip(&mut source_state).unwrap();
    assert_eq!(target_state.data_offer_actions, vec![2]);
    assert_eq!(source_state.data_source_actions, vec![2]);

    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: false,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    target_queue.roundtrip(&mut target_state).unwrap();
    source_queue.roundtrip(&mut source_state).unwrap();
    assert_eq!(target_state.data_device_drop_count, 1);
    assert_eq!(source_state.data_source_dnd_drop_performed_count, 1);

    let offer_id = offer.id().protocol_id();
    let before = capture_dnd_action_snapshot(&commands, offer_id)
        .expect("the dropped offer must remain tracked until finish");
    assert_eq!(before.offer_phase, Some(DragOfferPhase::Dropped));
    assert_eq!(
        before.active_phase,
        Some(DragSessionPhase::DroppedAwaitingFinish)
    );
    assert_eq!(before.offer_selected_action, Some(2));
    assert_eq!(before.offer_destination_actions, Some(1 | 2));
    assert_eq!(before.offer_preferred_action, 2);
    assert_eq!(before.active_selected_action, Some(2));
    assert_eq!(before.active_last_offer_action, Some(2));
    assert_eq!(before.active_last_source_action, Some(2));

    // set_actions remains a valid request until finish. Choosing Copy here
    // must not rewrite the Move action already frozen by the normal drop.
    offer.set_actions(
        client_wl_data_device_manager::DndAction::Copy
            | client_wl_data_device_manager::DndAction::Move,
        client_wl_data_device_manager::DndAction::Copy,
    );
    target_connection.flush().unwrap();
    wait_for_server_commands(&commands);
    assert!(
        target_queue.roundtrip(&mut target_state).is_ok(),
        "valid post-drop set_actions must not disconnect the destination"
    );
    source_queue.roundtrip(&mut source_state).unwrap();
    assert_eq!(target_state.data_offer_actions, vec![2]);
    assert_eq!(source_state.data_source_actions, vec![2]);

    let after = capture_dnd_action_snapshot(&commands, offer_id)
        .expect("the dropped offer must remain tracked after set_actions");
    assert_eq!(after.offer_phase, before.offer_phase);
    assert_eq!(after.active_phase, before.active_phase);
    assert_eq!(after.offer_selected_action, before.offer_selected_action);
    assert_eq!(
        after.offer_destination_actions,
        before.offer_destination_actions
    );
    assert_eq!(after.offer_preferred_action, before.offer_preferred_action);
    assert_eq!(after.active_selected_action, before.active_selected_action);
    assert_eq!(
        after.active_last_offer_action,
        before.active_last_offer_action
    );
    assert_eq!(
        after.active_last_source_action,
        before.active_last_source_action
    );
    assert_eq!(after.offer_action_events, before.offer_action_events);
    assert_eq!(after.source_action_events, before.source_action_events);

    offer.finish();
    target_connection.flush().unwrap();
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    assert_eq!(source_state.data_source_actions, vec![2]);
    assert_eq!(source_state.data_source_dnd_finished_count, 1);
    assert_eq!(source_state.data_source_cancelled_count, 0);

    // finish is terminal for all offer requests except destroy.
    offer.set_actions(
        client_wl_data_device_manager::DndAction::Copy
            | client_wl_data_device_manager::DndAction::Move,
        client_wl_data_device_manager::DndAction::Move,
    );
    target_connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let protocol_error = expect_protocol_error(
        &target_connection,
        "wl_data_offer",
        client_wl_data_offer::Error::InvalidOffer as u32,
    );
    assert_eq!(
        protocol_error.message,
        "selection offer cannot negotiate drag-and-drop actions"
    );

    drop(target_state);
    drop(target_queue);
    drop(target_connection);
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    assert_eq!(source_state.data_source_cancelled_count, 0);
    assert_eq!(source_state.data_source_dnd_finished_count, 1);

    let _server = stop_controllable_test_server(commands, server_thread);
    let _ = source_pointer;
    let _ = target_device;
}

#[test]
fn sourced_wire_drag_target_disconnect_before_drop_cancels_once() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
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
    let source_shm: client_wl_shm::WlShm = source_globals.bind(&source_qh, 1..=1, ()).unwrap();
    let source_seat: client_wl_seat::WlSeat = source_globals.bind(&source_qh, 1..=7, ()).unwrap();
    let source_pointer = source_seat.get_pointer(&source_qh, ());
    let source_manager: client_wl_data_device_manager::WlDataDeviceManager =
        source_globals.bind(&source_qh, 1..=3, ()).unwrap();
    let source_device = source_manager.get_data_device(&source_seat, &source_qh, ());
    let (source_surface, source_xdg_surface, _source_toplevel) = create_test_buffered_toplevel(
        &source_compositor,
        &source_wm_base,
        &source_shm,
        &source_qh,
        160,
        120,
    )
    .unwrap();
    let source = source_manager.create_data_source(&source_qh, ());
    source.offer("text/plain".to_string());
    source.set_actions(
        client_wl_data_device_manager::DndAction::Copy
            | client_wl_data_device_manager::DndAction::Move,
    );
    source_surface.commit();
    source_connection.flush().unwrap();
    let mut source_state = RegistryTestState::default();
    source_queue.roundtrip(&mut source_state).unwrap();
    commit_registered_initial_xdg_test_buffer(&source_xdg_surface);
    source_connection.flush().unwrap();
    source_queue.roundtrip(&mut source_state).unwrap();

    let target_connection =
        Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (target_globals, mut target_queue) =
        registry_queue_init::<RegistryTestState>(&target_connection).unwrap();
    let target_qh = target_queue.handle();
    let target_compositor: client_wl_compositor::WlCompositor =
        target_globals.bind(&target_qh, 1..=6, ()).unwrap();
    let target_wm_base: client_xdg_wm_base::XdgWmBase =
        target_globals.bind(&target_qh, 1..=6, ()).unwrap();
    let target_shm: client_wl_shm::WlShm = target_globals.bind(&target_qh, 1..=1, ()).unwrap();
    let target_seat: client_wl_seat::WlSeat = target_globals.bind(&target_qh, 1..=7, ()).unwrap();
    let target_manager: client_wl_data_device_manager::WlDataDeviceManager =
        target_globals.bind(&target_qh, 1..=3, ()).unwrap();
    let target_device = target_manager.get_data_device(&target_seat, &target_qh, ());
    let (target_surface, target_xdg_surface, _target_toplevel) = create_test_buffered_toplevel(
        &target_compositor,
        &target_wm_base,
        &target_shm,
        &target_qh,
        160,
        120,
    )
    .unwrap();
    target_surface.commit();
    target_connection.flush().unwrap();
    let mut target_state = RegistryTestState::default();
    target_queue.roundtrip(&mut target_state).unwrap();
    commit_registered_initial_xdg_test_buffer(&target_xdg_surface);
    target_connection.flush().unwrap();
    target_queue.roundtrip(&mut target_state).unwrap();

    let target_surface_id = target_surface.id().protocol_id();
    let source_surface_id = source_surface.id().protocol_id();
    focus_root_window(&commands, target_surface_id);
    set_focused_root_visual_geometry(
        &commands,
        SurfacePlacement::absolute_root_at(300, 200),
        160,
        120,
    );
    focus_root_window(&commands, source_surface_id);

    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 20.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    let serial = source_state
        .pointer_button_serial
        .expect("source drag must use the real pointer press serial");

    source_device.start_drag(Some(&source), &source_surface, None, serial);
    source_connection.flush().unwrap();
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    target_queue.roundtrip(&mut target_state).unwrap();
    assert!(source_state.data_source_actions.is_empty());

    commands
        .send(ServerCommand::PointerMotion { x: 320.0, y: 220.0 })
        .unwrap();
    wait_for_server_commands(&commands);
    target_queue.roundtrip(&mut target_state).unwrap();
    source_queue.roundtrip(&mut source_state).unwrap();
    assert_eq!(target_state.data_device_enter_count, 1);
    assert_eq!(target_state.data_offer_mime_types, vec!["text/plain"]);
    assert_eq!(target_state.data_offer_source_actions, vec![1 | 2]);
    assert!(target_state.data_offer_actions.is_empty());
    assert!(source_state.data_source_actions.is_empty());

    let offer = target_state
        .data_device_drag_offer
        .clone()
        .expect("target must receive a DnD offer");
    let enter_serial = target_state
        .data_device_enter_serial
        .expect("target must receive an enter serial");
    offer.accept(enter_serial, Some("text/plain".to_string()));
    offer.set_actions(
        client_wl_data_device_manager::DndAction::Copy
            | client_wl_data_device_manager::DndAction::Move,
        client_wl_data_device_manager::DndAction::Move,
    );
    target_connection.flush().unwrap();
    wait_for_server_commands(&commands);
    target_queue.roundtrip(&mut target_state).unwrap();
    source_queue.roundtrip(&mut source_state).unwrap();
    assert_eq!(target_state.data_offer_actions, vec![2]);
    assert_eq!(source_state.data_source_actions, vec![2]);

    drop(target_state);
    drop(target_queue);
    drop(target_connection);
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    assert_eq!(source_state.data_source_cancelled_count, 1);
    assert_eq!(source_state.data_source_dnd_finished_count, 0);
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    assert_eq!(source_state.data_source_cancelled_count, 1);

    let _server = stop_controllable_test_server(commands, server_thread);
    let _ = source_pointer;
    let _ = target_device;
}

#[test]
fn sourced_wire_drag_offer_destroy_after_drop_cancels_once() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
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
    let source_shm: client_wl_shm::WlShm = source_globals.bind(&source_qh, 1..=1, ()).unwrap();
    let source_seat: client_wl_seat::WlSeat = source_globals.bind(&source_qh, 1..=7, ()).unwrap();
    let source_pointer = source_seat.get_pointer(&source_qh, ());
    let source_manager: client_wl_data_device_manager::WlDataDeviceManager =
        source_globals.bind(&source_qh, 1..=3, ()).unwrap();
    let source_device = source_manager.get_data_device(&source_seat, &source_qh, ());
    let (source_surface, source_xdg_surface, _source_toplevel) = create_test_buffered_toplevel(
        &source_compositor,
        &source_wm_base,
        &source_shm,
        &source_qh,
        160,
        120,
    )
    .unwrap();
    let source = source_manager.create_data_source(&source_qh, ());
    source.offer("text/plain".to_string());
    source.set_actions(
        client_wl_data_device_manager::DndAction::Copy
            | client_wl_data_device_manager::DndAction::Move,
    );
    source_surface.commit();
    source_connection.flush().unwrap();
    let mut source_state = RegistryTestState::default();
    source_queue.roundtrip(&mut source_state).unwrap();
    commit_registered_initial_xdg_test_buffer(&source_xdg_surface);
    source_connection.flush().unwrap();
    source_queue.roundtrip(&mut source_state).unwrap();

    let target_connection =
        Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (target_globals, mut target_queue) =
        registry_queue_init::<RegistryTestState>(&target_connection).unwrap();
    let target_qh = target_queue.handle();
    let target_compositor: client_wl_compositor::WlCompositor =
        target_globals.bind(&target_qh, 1..=6, ()).unwrap();
    let target_wm_base: client_xdg_wm_base::XdgWmBase =
        target_globals.bind(&target_qh, 1..=6, ()).unwrap();
    let target_shm: client_wl_shm::WlShm = target_globals.bind(&target_qh, 1..=1, ()).unwrap();
    let target_seat: client_wl_seat::WlSeat = target_globals.bind(&target_qh, 1..=7, ()).unwrap();
    let target_manager: client_wl_data_device_manager::WlDataDeviceManager =
        target_globals.bind(&target_qh, 1..=3, ()).unwrap();
    let target_device = target_manager.get_data_device(&target_seat, &target_qh, ());
    let (target_surface, target_xdg_surface, _target_toplevel) = create_test_buffered_toplevel(
        &target_compositor,
        &target_wm_base,
        &target_shm,
        &target_qh,
        160,
        120,
    )
    .unwrap();
    target_surface.commit();
    target_connection.flush().unwrap();
    let mut target_state = RegistryTestState::default();
    target_queue.roundtrip(&mut target_state).unwrap();
    commit_registered_initial_xdg_test_buffer(&target_xdg_surface);
    target_connection.flush().unwrap();
    target_queue.roundtrip(&mut target_state).unwrap();

    let target_surface_id = target_surface.id().protocol_id();
    let source_surface_id = source_surface.id().protocol_id();
    focus_root_window(&commands, target_surface_id);
    set_focused_root_visual_geometry(
        &commands,
        SurfacePlacement::absolute_root_at(300, 200),
        160,
        120,
    );
    focus_root_window(&commands, source_surface_id);

    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 20.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    let serial = source_state
        .pointer_button_serial
        .expect("source drag must use the real pointer press serial");

    source_device.start_drag(Some(&source), &source_surface, None, serial);
    source_connection.flush().unwrap();
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    target_queue.roundtrip(&mut target_state).unwrap();
    assert!(source_state.data_source_actions.is_empty());

    commands
        .send(ServerCommand::PointerMotion { x: 320.0, y: 220.0 })
        .unwrap();
    wait_for_server_commands(&commands);
    target_queue.roundtrip(&mut target_state).unwrap();
    source_queue.roundtrip(&mut source_state).unwrap();
    assert_eq!(target_state.data_device_enter_count, 1);
    assert_eq!(target_state.data_offer_mime_types, vec!["text/plain"]);
    assert_eq!(target_state.data_offer_source_actions, vec![1 | 2]);
    assert!(target_state.data_offer_actions.is_empty());
    assert!(source_state.data_source_actions.is_empty());

    let offer = target_state
        .data_device_drag_offer
        .clone()
        .expect("target must receive a DnD offer");
    let enter_serial = target_state
        .data_device_enter_serial
        .expect("target must receive an enter serial");
    offer.accept(enter_serial, Some("text/plain".to_string()));
    offer.set_actions(
        client_wl_data_device_manager::DndAction::Copy
            | client_wl_data_device_manager::DndAction::Move,
        client_wl_data_device_manager::DndAction::Move,
    );
    target_connection.flush().unwrap();
    wait_for_server_commands(&commands);
    target_queue.roundtrip(&mut target_state).unwrap();
    source_queue.roundtrip(&mut source_state).unwrap();
    assert_eq!(target_state.data_offer_actions, vec![2]);
    assert_eq!(source_state.data_source_actions, vec![2]);

    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: false,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    target_queue.roundtrip(&mut target_state).unwrap();
    source_queue.roundtrip(&mut source_state).unwrap();
    assert_eq!(target_state.data_device_drop_count, 1);
    assert_eq!(source_state.data_source_dnd_drop_performed_count, 1);

    offer.destroy();
    target_connection.flush().unwrap();
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    assert_eq!(source_state.data_source_cancelled_count, 1);
    assert_eq!(source_state.data_source_dnd_finished_count, 0);
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    assert_eq!(source_state.data_source_cancelled_count, 1);

    let _server = stop_controllable_test_server(commands, server_thread);
    let _ = source_pointer;
    let _ = target_device;
}

#[test]
fn sourced_wire_drag_target_disconnect_while_ask_is_unresolved_cancels_once() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
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
    let source_shm: client_wl_shm::WlShm = source_globals.bind(&source_qh, 1..=1, ()).unwrap();
    let source_seat: client_wl_seat::WlSeat = source_globals.bind(&source_qh, 1..=7, ()).unwrap();
    let source_pointer = source_seat.get_pointer(&source_qh, ());
    let source_manager: client_wl_data_device_manager::WlDataDeviceManager =
        source_globals.bind(&source_qh, 1..=3, ()).unwrap();
    let source_device = source_manager.get_data_device(&source_seat, &source_qh, ());
    let (source_surface, source_xdg_surface, _source_toplevel) = create_test_buffered_toplevel(
        &source_compositor,
        &source_wm_base,
        &source_shm,
        &source_qh,
        160,
        120,
    )
    .unwrap();
    let source = source_manager.create_data_source(&source_qh, ());
    source.offer("text/plain".to_string());
    source.set_actions(
        client_wl_data_device_manager::DndAction::Copy
            | client_wl_data_device_manager::DndAction::Move
            | client_wl_data_device_manager::DndAction::Ask,
    );
    source_surface.commit();
    source_connection.flush().unwrap();
    let mut source_state = RegistryTestState::default();
    source_queue.roundtrip(&mut source_state).unwrap();
    commit_registered_initial_xdg_test_buffer(&source_xdg_surface);
    source_connection.flush().unwrap();
    source_queue.roundtrip(&mut source_state).unwrap();

    let target_connection =
        Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (target_globals, mut target_queue) =
        registry_queue_init::<RegistryTestState>(&target_connection).unwrap();
    let target_qh = target_queue.handle();
    let target_compositor: client_wl_compositor::WlCompositor =
        target_globals.bind(&target_qh, 1..=6, ()).unwrap();
    let target_wm_base: client_xdg_wm_base::XdgWmBase =
        target_globals.bind(&target_qh, 1..=6, ()).unwrap();
    let target_shm: client_wl_shm::WlShm = target_globals.bind(&target_qh, 1..=1, ()).unwrap();
    let target_seat: client_wl_seat::WlSeat = target_globals.bind(&target_qh, 1..=7, ()).unwrap();
    let target_manager: client_wl_data_device_manager::WlDataDeviceManager =
        target_globals.bind(&target_qh, 1..=3, ()).unwrap();
    let target_device = target_manager.get_data_device(&target_seat, &target_qh, ());
    let (target_surface, target_xdg_surface, _target_toplevel) = create_test_buffered_toplevel(
        &target_compositor,
        &target_wm_base,
        &target_shm,
        &target_qh,
        160,
        120,
    )
    .unwrap();
    target_surface.commit();
    target_connection.flush().unwrap();
    let mut target_state = RegistryTestState::default();
    target_queue.roundtrip(&mut target_state).unwrap();
    commit_registered_initial_xdg_test_buffer(&target_xdg_surface);
    target_connection.flush().unwrap();
    target_queue.roundtrip(&mut target_state).unwrap();

    let target_surface_id = target_surface.id().protocol_id();
    let source_surface_id = source_surface.id().protocol_id();
    focus_root_window(&commands, target_surface_id);
    set_focused_root_visual_geometry(
        &commands,
        SurfacePlacement::absolute_root_at(300, 200),
        160,
        120,
    );
    focus_root_window(&commands, source_surface_id);
    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 20.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    let serial = source_state
        .pointer_button_serial
        .expect("source drag must use the real pointer press serial");
    source_device.start_drag(Some(&source), &source_surface, None, serial);
    source_connection.flush().unwrap();
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    target_queue.roundtrip(&mut target_state).unwrap();

    commands
        .send(ServerCommand::PointerMotion { x: 320.0, y: 220.0 })
        .unwrap();
    wait_for_server_commands(&commands);
    target_queue.roundtrip(&mut target_state).unwrap();
    source_queue.roundtrip(&mut source_state).unwrap();
    let offer = target_state
        .data_device_drag_offer
        .clone()
        .expect("target must receive an ASK-capable DnD offer");
    let enter_serial = target_state
        .data_device_enter_serial
        .expect("target must receive an enter serial");
    offer.accept(enter_serial, Some("text/plain".to_string()));
    offer.set_actions(
        client_wl_data_device_manager::DndAction::Copy
            | client_wl_data_device_manager::DndAction::Move
            | client_wl_data_device_manager::DndAction::Ask,
        client_wl_data_device_manager::DndAction::Ask,
    );
    target_connection.flush().unwrap();
    wait_for_server_commands(&commands);
    target_queue.roundtrip(&mut target_state).unwrap();
    source_queue.roundtrip(&mut source_state).unwrap();
    assert_eq!(target_state.data_offer_actions, vec![4]);
    assert_eq!(source_state.data_source_actions, vec![4]);

    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: false,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    target_queue.roundtrip(&mut target_state).unwrap();
    source_queue.roundtrip(&mut source_state).unwrap();
    assert_eq!(target_state.data_device_drop_count, 1);
    assert_eq!(source_state.data_source_dnd_drop_performed_count, 1);

    drop(target_state);
    drop(target_queue);
    drop(target_connection);
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    assert_eq!(source_state.data_source_cancelled_count, 1);
    assert_eq!(source_state.data_source_dnd_finished_count, 0);
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    assert_eq!(source_state.data_source_cancelled_count, 1);

    let _server = stop_controllable_test_server(commands, server_thread);
    let _ = source_pointer;
    let _ = target_device;
}

#[test]
fn sourced_wire_drag_ask_resolves_to_copy_before_finished() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
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
    let source_shm: client_wl_shm::WlShm = source_globals.bind(&source_qh, 1..=1, ()).unwrap();
    let source_seat: client_wl_seat::WlSeat = source_globals.bind(&source_qh, 1..=7, ()).unwrap();
    let source_pointer = source_seat.get_pointer(&source_qh, ());
    let source_manager: client_wl_data_device_manager::WlDataDeviceManager =
        source_globals.bind(&source_qh, 1..=3, ()).unwrap();
    let source_device = source_manager.get_data_device(&source_seat, &source_qh, ());
    let (source_surface, source_xdg_surface, _source_toplevel) = create_test_buffered_toplevel(
        &source_compositor,
        &source_wm_base,
        &source_shm,
        &source_qh,
        160,
        120,
    )
    .unwrap();
    let source = source_manager.create_data_source(&source_qh, ());
    source.offer("text/plain".to_string());
    source.set_actions(
        client_wl_data_device_manager::DndAction::Copy
            | client_wl_data_device_manager::DndAction::Move
            | client_wl_data_device_manager::DndAction::Ask,
    );
    source_surface.commit();
    source_connection.flush().unwrap();
    let mut source_state = RegistryTestState::default();
    source_queue.roundtrip(&mut source_state).unwrap();
    commit_registered_initial_xdg_test_buffer(&source_xdg_surface);
    source_connection.flush().unwrap();
    source_queue.roundtrip(&mut source_state).unwrap();

    let target_connection =
        Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (target_globals, mut target_queue) =
        registry_queue_init::<RegistryTestState>(&target_connection).unwrap();
    let target_qh = target_queue.handle();
    let target_compositor: client_wl_compositor::WlCompositor =
        target_globals.bind(&target_qh, 1..=6, ()).unwrap();
    let target_wm_base: client_xdg_wm_base::XdgWmBase =
        target_globals.bind(&target_qh, 1..=6, ()).unwrap();
    let target_shm: client_wl_shm::WlShm = target_globals.bind(&target_qh, 1..=1, ()).unwrap();
    let target_seat: client_wl_seat::WlSeat = target_globals.bind(&target_qh, 1..=7, ()).unwrap();
    let target_manager: client_wl_data_device_manager::WlDataDeviceManager =
        target_globals.bind(&target_qh, 1..=3, ()).unwrap();
    let target_device = target_manager.get_data_device(&target_seat, &target_qh, ());
    let (target_surface, target_xdg_surface, _target_toplevel) = create_test_buffered_toplevel(
        &target_compositor,
        &target_wm_base,
        &target_shm,
        &target_qh,
        160,
        120,
    )
    .unwrap();
    target_surface.commit();
    target_connection.flush().unwrap();
    let mut target_state = RegistryTestState::default();
    target_queue.roundtrip(&mut target_state).unwrap();
    commit_registered_initial_xdg_test_buffer(&target_xdg_surface);
    target_connection.flush().unwrap();
    target_queue.roundtrip(&mut target_state).unwrap();

    let target_surface_id = target_surface.id().protocol_id();
    let source_surface_id = source_surface.id().protocol_id();
    focus_root_window(&commands, target_surface_id);
    set_focused_root_visual_geometry(
        &commands,
        SurfacePlacement::absolute_root_at(300, 200),
        160,
        120,
    );
    focus_root_window(&commands, source_surface_id);
    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 20.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    let serial = source_state
        .pointer_button_serial
        .expect("source drag must use the real pointer press serial");
    source_device.start_drag(Some(&source), &source_surface, None, serial);
    source_connection.flush().unwrap();
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    target_queue.roundtrip(&mut target_state).unwrap();

    commands
        .send(ServerCommand::PointerMotion { x: 320.0, y: 220.0 })
        .unwrap();
    wait_for_server_commands(&commands);
    target_queue.roundtrip(&mut target_state).unwrap();
    source_queue.roundtrip(&mut source_state).unwrap();
    let offer = target_state
        .data_device_drag_offer
        .clone()
        .expect("target must receive an ASK-capable DnD offer");
    let enter_serial = target_state
        .data_device_enter_serial
        .expect("target must receive an enter serial");
    offer.accept(enter_serial, Some("text/plain".to_string()));
    offer.set_actions(
        client_wl_data_device_manager::DndAction::Copy
            | client_wl_data_device_manager::DndAction::Move
            | client_wl_data_device_manager::DndAction::Ask,
        client_wl_data_device_manager::DndAction::Ask,
    );
    target_connection.flush().unwrap();
    wait_for_server_commands(&commands);
    target_queue.roundtrip(&mut target_state).unwrap();
    source_queue.roundtrip(&mut source_state).unwrap();
    assert_eq!(target_state.data_offer_actions, vec![4]);
    assert_eq!(source_state.data_source_actions, vec![4]);

    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: false,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    target_queue.roundtrip(&mut target_state).unwrap();
    source_queue.roundtrip(&mut source_state).unwrap();
    assert_eq!(target_state.data_device_drop_count, 1);
    assert_eq!(source_state.data_source_dnd_drop_performed_count, 1);

    // ASK is resolved after drop.  The offer action remains frozen; only the
    // source receives the final concrete action immediately before finished.
    offer.set_actions(
        client_wl_data_device_manager::DndAction::Copy
            | client_wl_data_device_manager::DndAction::Move,
        client_wl_data_device_manager::DndAction::Copy,
    );
    target_connection.flush().unwrap();
    wait_for_server_commands(&commands);
    target_queue.roundtrip(&mut target_state).unwrap();
    source_queue.roundtrip(&mut source_state).unwrap();
    assert_eq!(target_state.data_offer_actions, vec![4]);
    assert_eq!(source_state.data_source_actions, vec![4]);

    offer.finish();
    target_connection.flush().unwrap();
    wait_for_server_commands(&commands);
    source_queue.roundtrip(&mut source_state).unwrap();
    assert_eq!(source_state.data_source_actions, vec![4, 1]);
    assert_eq!(source_state.data_source_dnd_finished_count, 1);
    offer.destroy();
    target_connection.flush().unwrap();
    wait_for_server_commands(&commands);

    let _server = stop_controllable_test_server(commands, server_thread);
    let _ = source_pointer;
    let _ = target_device;
}

#[derive(Debug, Clone, Copy)]
enum DndModelOp {
    CreateSource,
    OfferMime,
    SetSourceActions(u32),
    StartDrag(bool),
    Enter,
    Leave,
    Accept(bool),
    SetDestinationActions(u32, u32),
    Receive,
    Drop,
    ResolveAsk,
    Finish,
    DestroyOffer,
    DestroySource,
    DisconnectSource,
    DisconnectTarget,
    Cancel,
    Suspend,
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReferenceDndPhase {
    Idle,
    Dragging,
    Dropped,
    Ask,
    Finished,
    Cancelled,
}

#[derive(Debug, Clone)]
struct ReferenceDndModel {
    source_client_available: bool,
    source_alive: bool,
    source_used: bool,
    actions_set: bool,
    source_actions: u32,
    mime_offered: bool,
    active: bool,
    source_attached: bool,
    target_present: bool,
    target_available: bool,
    offer_alive: bool,
    accepted_mime: bool,
    destination_actions: Option<u32>,
    selected_action: u32,
    phase: ReferenceDndPhase,
    terminal_events: u32,
    duplicate_terminal_attempts: u32,
    source_cancelled_events: u64,
    source_finished_events: u64,
    offer_action_events: u64,
    source_action_events: u64,
    last_action_event: Option<u32>,
}

impl Default for ReferenceDndModel {
    fn default() -> Self {
        Self {
            source_client_available: true,
            source_alive: false,
            source_used: false,
            actions_set: false,
            source_actions: 0,
            mime_offered: false,
            active: false,
            source_attached: false,
            target_present: false,
            target_available: true,
            offer_alive: false,
            accepted_mime: false,
            destination_actions: None,
            selected_action: 0,
            phase: ReferenceDndPhase::Idle,
            terminal_events: 0,
            duplicate_terminal_attempts: 0,
            source_cancelled_events: 0,
            source_finished_events: 0,
            offer_action_events: 0,
            source_action_events: 0,
            last_action_event: None,
        }
    }
}

impl ReferenceDndModel {
    fn terminate(&mut self, phase: ReferenceDndPhase) {
        if !self.active {
            if self.phase == ReferenceDndPhase::Finished
                || self.phase == ReferenceDndPhase::Cancelled
            {
                self.duplicate_terminal_attempts += 1;
            }
            return;
        }
        self.active = false;
        self.phase = phase;
        self.terminal_events += 1;
        if self.source_attached {
            match phase {
                ReferenceDndPhase::Cancelled => self.source_cancelled_events += 1,
                ReferenceDndPhase::Finished => self.source_finished_events += 1,
                _ => {}
            }
        }
        self.offer_alive = false;
        self.target_present = false;
        self.accepted_mime = false;
        self.destination_actions = None;
        self.selected_action = 0;
    }

    fn apply(&mut self, op: DndModelOp) {
        match op {
            DndModelOp::CreateSource if self.source_client_available && !self.source_alive => {
                self.source_alive = true;
                self.source_used = false;
                self.actions_set = false;
                self.source_actions = 0;
                self.mime_offered = false;
            }
            DndModelOp::OfferMime if self.source_alive && !self.source_used => {
                self.mime_offered = true;
            }
            DndModelOp::SetSourceActions(actions)
                if self.source_alive && !self.source_used && !self.actions_set && actions <= 7 =>
            {
                self.source_actions = actions;
                self.actions_set = true;
            }
            DndModelOp::StartDrag(with_source)
                if !self.active
                    && (!with_source
                        || (self.source_alive && !self.source_used && self.actions_set)) =>
            {
                self.active = true;
                self.source_attached = with_source;
                if with_source {
                    self.source_used = true;
                }
                self.offer_alive = false;
                self.target_present = false;
                self.accepted_mime = false;
                self.destination_actions = None;
                self.selected_action = 0;
                self.last_action_event = None;
                self.phase = ReferenceDndPhase::Dragging;
            }
            DndModelOp::Enter
                if self.active
                    && self.phase == ReferenceDndPhase::Dragging
                    && self.target_available =>
            {
                self.offer_alive = self.source_attached;
                self.target_present = true;
                self.accepted_mime = false;
                self.destination_actions = None;
                self.selected_action = 0;
                self.last_action_event = None;
            }
            DndModelOp::Leave if self.active && self.phase == ReferenceDndPhase::Dragging => {
                self.offer_alive = false;
                self.target_present = false;
                self.accepted_mime = false;
                self.destination_actions = None;
                self.selected_action = 0;
                self.last_action_event = None;
            }
            DndModelOp::Accept(accepted)
                if self.active && self.offer_alive && self.phase == ReferenceDndPhase::Dragging =>
            {
                self.accepted_mime = accepted && self.mime_offered;
            }
            DndModelOp::SetDestinationActions(actions, preferred)
                if self.active
                    && self.offer_alive
                    && (self.phase == ReferenceDndPhase::Dragging
                        || self.phase == ReferenceDndPhase::Ask
                        || self.phase == ReferenceDndPhase::Dropped)
                    && actions <= 7
                    && (preferred == 0
                        || (preferred.count_ones() == 1 && actions & preferred != 0)) =>
            {
                if self.phase == ReferenceDndPhase::Dragging {
                    self.destination_actions = Some(actions);
                    let common = self.source_actions & actions;
                    self.selected_action = if preferred != 0 && common & preferred != 0 {
                        preferred
                    } else if common & 1 != 0 {
                        1
                    } else if common & 2 != 0 {
                        2
                    } else if common & 4 != 0 {
                        4
                    } else {
                        0
                    };
                    if self.source_attached && self.last_action_event != Some(self.selected_action)
                    {
                        self.last_action_event = Some(self.selected_action);
                        self.offer_action_events += 1;
                        self.source_action_events += 1;
                    }
                } else if self.phase == ReferenceDndPhase::Ask {
                    self.destination_actions = Some(actions);
                    let common = self.source_actions & actions;
                    if common & preferred != 0 {
                        self.selected_action = preferred;
                    }
                }
            }
            DndModelOp::Receive
                if self.active
                    && self.offer_alive
                    && self.accepted_mime
                    && (self.phase == ReferenceDndPhase::Dragging
                        || self.phase == ReferenceDndPhase::Dropped
                        || self.phase == ReferenceDndPhase::Ask) => {}
            DndModelOp::Drop if self.active && self.phase == ReferenceDndPhase::Dragging => {
                if !self.source_attached && self.target_present {
                    self.phase = ReferenceDndPhase::Finished;
                    self.active = false;
                    self.terminal_events += 1;
                } else if !self.source_attached
                    || !self.offer_alive
                    || !self.accepted_mime
                    || self.selected_action == 0
                {
                    self.terminate(ReferenceDndPhase::Cancelled);
                } else if self.selected_action == 4 {
                    self.phase = ReferenceDndPhase::Ask;
                } else {
                    self.phase = ReferenceDndPhase::Dropped;
                }
            }
            DndModelOp::Drop
                if !self.active
                    && self.offer_alive
                    && matches!(
                        self.phase,
                        ReferenceDndPhase::Finished | ReferenceDndPhase::Cancelled
                    ) =>
            {
                self.duplicate_terminal_attempts += 1;
            }
            DndModelOp::ResolveAsk
                if self.active && self.phase == ReferenceDndPhase::Ask && self.offer_alive =>
            {
                let common = self.source_actions & self.destination_actions.unwrap_or_default();
                self.selected_action = if common & 1 != 0 {
                    1
                } else if common & 2 != 0 {
                    2
                } else {
                    0
                };
            }
            DndModelOp::Finish
                if self.active
                    && self.offer_alive
                    && self.accepted_mime
                    && ((self.phase == ReferenceDndPhase::Dropped
                        && self.selected_action != 0)
                        || (self.phase == ReferenceDndPhase::Ask
                            && matches!(self.selected_action, 1 | 2))) =>
            {
                let was_ask = self.phase == ReferenceDndPhase::Ask;
                self.terminate(ReferenceDndPhase::Finished);
                if self.source_attached && was_ask {
                    self.source_action_events += 1;
                }
                self.offer_alive = true;
            }
            DndModelOp::Finish => {
                if !self.active
                    && self.offer_alive
                    && matches!(
                        self.phase,
                        ReferenceDndPhase::Finished | ReferenceDndPhase::Cancelled
                    )
                {
                    self.duplicate_terminal_attempts += 1;
                }
            }
            DndModelOp::DestroyOffer if self.offer_alive => {
                if self.active {
                    self.terminate(ReferenceDndPhase::Cancelled);
                } else {
                    self.offer_alive = false;
                }
            }
            DndModelOp::DestroySource | DndModelOp::DisconnectSource => {
                if self.active && self.source_attached {
                    self.terminate(ReferenceDndPhase::Cancelled);
                }
                self.source_alive = false;
                self.source_used = false;
                self.actions_set = false;
                self.source_actions = 0;
                self.mime_offered = false;
                self.target_available = false;
                self.source_client_available = false;
            }
            DndModelOp::DisconnectTarget => {
                self.target_available = false;
                if self.active && self.offer_alive {
                    self.terminate(ReferenceDndPhase::Cancelled);
                }
            }
            DndModelOp::Cancel => {
                if self.active {
                    self.terminate(ReferenceDndPhase::Cancelled);
                } else if matches!(
                    self.phase,
                    ReferenceDndPhase::Finished | ReferenceDndPhase::Cancelled
                ) {
                    self.duplicate_terminal_attempts += 1;
                }
            }
            DndModelOp::Suspend | DndModelOp::Shutdown => {
                if self.active {
                    self.terminate(ReferenceDndPhase::Cancelled);
                }
            }
            _ => {}
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProductionDndSnapshot {
    source_alive: bool,
    source_used: bool,
    actions_set: bool,
    source_actions: u32,
    mime_offered: bool,
    active: bool,
    source_attached: bool,
    target_present: bool,
    target_available: bool,
    source_client_available: bool,
    offer_alive: bool,
    accepted_mime: bool,
    destination_actions: Option<u32>,
    selected_action: u32,
    phase: Option<DragSessionPhase>,
    terminal_events: u32,
    duplicate_terminal_attempts: u32,
    source_cancelled_events: u64,
    source_finished_events: u64,
    offer_action_events: u64,
    source_action_events: u64,
    drag_icon_live: bool,
    orphaned_resources: u64,
}

struct ProductionDndClient {
    client: Client,
    _peer: UnixStream,
}

struct ProductionDndModel {
    display: Display<CompositorState>,
    state: CompositorState,
    clients: Vec<ProductionDndClient>,
    origin: wl_surface::WlSurface,
    target: wl_surface::WlSurface,
    same_client_target: wl_surface::WlSurface,
    target_device: wl_data_device::WlDataDevice,
    same_client_device: wl_data_device::WlDataDevice,
    target_available: bool,
    source_client_available: bool,
    source: Option<wl_data_source::WlDataSource>,
    source_client_index: usize,
    target_client_index: usize,
}

impl ProductionDndModel {
    fn new() -> Self {
        let display = Display::<CompositorState>::new().expect("production model display");
        let mut state = CompositorState::new(None);
        let clients = (0..1)
            .map(|_| {
                let (server_end, peer) = UnixStream::pair().expect("production model client");
                let client = display
                    .handle()
                    .insert_client(server_end, Arc::new(()))
                    .expect("production model insert client");
                ProductionDndClient {
                    client,
                    _peer: peer,
                }
            })
            .collect::<Vec<_>>();
        let origin = state.test_create_surface_resource(
            &clients[0].client,
            &display.handle(),
            40,
            40,
            SurfacePlacement::absolute_root_at(1000, 1000),
        );
        let target = state.test_create_surface_resource(
            &clients[0].client,
            &display.handle(),
            40,
            40,
            SurfacePlacement::absolute_root_at(0, 0),
        );
        let same_client_target = state.test_create_surface_resource(
            &clients[0].client,
            &display.handle(),
            40,
            40,
            SurfacePlacement::absolute_root_at(200, 0),
        );
        let target_device = state.test_create_data_device(&clients[0].client, &display.handle());
        let same_client_device =
            state.test_create_data_device(&clients[0].client, &display.handle());
        Self {
            display,
            state,
            clients,
            origin,
            target,
            same_client_target,
            target_device,
            same_client_device,
            target_available: true,
            source_client_available: true,
            source: None,
            source_client_index: 0,
            target_client_index: 0,
        }
    }

    fn current_offer(&self) -> Option<wl_data_offer::WlDataOffer> {
        self.state.active_drag.as_ref().and_then(|drag| {
            drag.target
                .as_ref()
                .and_then(ActiveDragTarget::wayland_offer)
                .cloned()
        })
    }

    fn apply(&mut self, op: DndModelOp) {
        match op {
            DndModelOp::CreateSource if self.source_client_available && self.source.is_none() => {
                let source = self.state.test_create_data_source(
                    &self.clients[self.source_client_index].client,
                    &self.display.handle(),
                );
                self.source = Some(source);
            }
            DndModelOp::OfferMime => {
                if let Some(source) = self.source.clone() {
                    self.state
                        .offer_data_source_mime_type(&source, "text/plain".to_string());
                }
            }
            DndModelOp::SetSourceActions(actions) if actions <= 7 => {
                if let Some(source) = self.source.clone()
                    && let Some(binding) = self.state.data_sources.get_mut(&source.id())
                    && binding.use_state == DataSourceUse::Unused
                    && !binding.actions_set
                {
                    binding.actions = actions;
                    binding.actions_set = true;
                    self.state.source_drag_actions_changed(&source, actions);
                }
            }
            DndModelOp::StartDrag(with_source) if self.state.active_drag.is_none() => {
                let source = if with_source {
                    let Some(source) = self.source.clone() else {
                        return;
                    };
                    let Some(binding) = self.state.data_sources.get_mut(&source.id()) else {
                        return;
                    };
                    if !binding.actions_set || binding.use_state != DataSourceUse::Unused {
                        return;
                    }
                    binding.use_state = DataSourceUse::DragSource;
                    Some(source)
                } else {
                    None
                };
                self.state
                    .begin_drag_session(source, self.origin.clone(), None, 1);
            }
            DndModelOp::Enter if self.state.active_drag.is_some() => {
                if !self.target_available {
                    return;
                }
                let x = if self.state.active_drag.as_ref().is_some_and(|drag| {
                    matches!(drag.origin, ActiveDragOrigin::WaylandSource { .. })
                }) {
                    10.0
                } else {
                    210.0
                };
                self.state.update_drag_target_at(x, 10.0);
            }
            DndModelOp::Leave => self.state.leave_drag_target(),
            DndModelOp::Accept(accepted) => {
                if let Some(offer) = self.current_offer() {
                    self.state.update_drag_acceptance(
                        &offer,
                        accepted.then_some("text/plain".to_string()),
                    );
                }
            }
            DndModelOp::SetDestinationActions(actions, preferred) if actions <= 7 => {
                if let Some(offer) = self.current_offer() {
                    self.state
                        .apply_drag_offer_actions(&offer, actions, preferred);
                }
            }
            DndModelOp::Receive => {
                if let Some(offer) = self.current_offer() {
                    let fd = File::open("/dev/null")
                        .expect("production model receive fd")
                        .into();
                    self.state.receive_clipboard_offer(
                        &offer,
                        &self.clients[self.target_client_index].client.id(),
                        0,
                        "text/plain".to_string(),
                        fd,
                    );
                }
            }
            DndModelOp::Drop => self.state.drop_active_drag(),
            DndModelOp::ResolveAsk => {
                if let Some(offer) = self.current_offer()
                    && self.state.active_drag.as_ref().is_some_and(|drag| {
                        drag.phase == DragSessionPhase::DroppedAwaitingAskResolution
                    })
                {
                    self.state.apply_drag_offer_actions(&offer, 1 | 2, 1);
                }
            }
            DndModelOp::Finish => {
                if let Some(offer) = self.current_offer() {
                    let _ = self.state.finish_drag_offer(&offer);
                }
            }
            DndModelOp::DestroyOffer => {
                if let Some(offer) = self.current_offer() {
                    self.state.destroy_data_offer(&offer);
                }
            }
            DndModelOp::DestroySource => {
                if let Some(source) = self.source.clone() {
                    self.state.remove_data_source(&source);
                    self.source = None;
                }
            }
            DndModelOp::DisconnectSource => {
                if !self.source_client_available {
                    return;
                }
                let id = self.clients[self.source_client_index].client.id();
                self.state.teardown_client_resources(&id);
                self.source = None;
                self.target_available = false;
                self.source_client_available = false;
            }
            DndModelOp::DisconnectTarget => {
                self.target_available = false;
                self.state.remove_data_device(&self.target_device);
            }
            DndModelOp::Cancel => self.state.cancel_drag_session("explicit_cancel"),
            DndModelOp::Suspend | DndModelOp::Shutdown => {
                self.state.cancel_drag_session("production_model")
            }
            _ => {}
        }
    }

    fn snapshot(&self) -> ProductionDndSnapshot {
        let active = self.state.active_drag.as_ref();
        let source_binding = self
            .source
            .as_ref()
            .and_then(|source| self.state.data_sources.get(&source.id()));
        let offer_binding = active
            .and_then(|drag| {
                drag.target
                    .as_ref()
                    .and_then(ActiveDragTarget::wayland_offer)
            })
            .and_then(|offer| self.state.data_offers.get(&offer.id()))
            .or_else(|| {
                self.state
                    .data_offers
                    .values()
                    .find(|offer| offer.kind == DataOfferKind::DragAndDrop)
            });
        let metrics = self.state.compliance_metrics;
        let terminal_events =
            metrics.dnd_sessions_finished as u32 + metrics.dnd_sessions_cancelled as u32;
        ProductionDndSnapshot {
            source_alive: source_binding.is_some(),
            source_used: source_binding
                .is_some_and(|source| source.use_state != DataSourceUse::Unused),
            actions_set: source_binding.is_some_and(|source| source.actions_set),
            source_actions: source_binding.map_or(0, |source| source.actions),
            mime_offered: source_binding.is_some_and(|source| !source.mime_types.is_empty()),
            active: active.is_some(),
            source_attached: active
                .is_some_and(|drag| matches!(drag.origin, ActiveDragOrigin::WaylandSource { .. })),
            target_present: active.is_some_and(|drag| drag.target.is_some()),
            target_available: self.target_available,
            source_client_available: self.source_client_available,
            offer_alive: offer_binding.is_some(),
            accepted_mime: active.is_some_and(|drag| drag.accepted_mime.is_some()),
            destination_actions: active.and_then(|drag| drag.destination_actions),
            selected_action: active.map_or(0, |drag| drag.selected_action),
            phase: active
                .map(|drag| drag.phase)
                .or(metrics.dnd_last_terminal_phase),
            terminal_events,
            duplicate_terminal_attempts: metrics.dnd_duplicate_terminal_attempts as u32,
            source_cancelled_events: metrics.dnd_source_cancelled_events,
            source_finished_events: metrics.dnd_source_finished_events,
            offer_action_events: metrics.dnd_offer_action_events,
            source_action_events: metrics.dnd_source_action_events,
            drag_icon_live: active.and_then(|drag| drag.icon_surface.as_ref()).is_some(),
            orphaned_resources: metrics.dnd_orphaned_resources_detected,
        }
    }
}

fn assert_dnd_snapshot_matches(
    reference: &ReferenceDndModel,
    actual: &ProductionDndSnapshot,
    context: &str,
) {
    assert_eq!(reference.active, actual.active, "{context} active");
    assert_eq!(
        reference.source_alive, actual.source_alive,
        "{context} source"
    );
    assert_eq!(
        reference.source_used, actual.source_used,
        "{context} source use"
    );
    assert_eq!(
        reference.actions_set, actual.actions_set,
        "{context} source actions"
    );
    assert_eq!(
        reference.source_actions, actual.source_actions,
        "{context} action mask"
    );
    assert_eq!(
        reference.mime_offered, actual.mime_offered,
        "{context} MIME"
    );
    assert_eq!(reference.offer_alive, actual.offer_alive, "{context} offer");
    assert_eq!(
        reference.target_present, actual.target_present,
        "{context} target"
    );
    assert_eq!(
        reference.target_available, actual.target_available,
        "{context} target availability"
    );
    assert_eq!(
        reference.source_client_available, actual.source_client_available,
        "{context} source client availability"
    );
    assert_eq!(
        reference.accepted_mime, actual.accepted_mime,
        "{context} MIME acceptance"
    );
    assert_eq!(
        reference.destination_actions, actual.destination_actions,
        "{context} destination actions"
    );
    assert_eq!(
        reference.selected_action, actual.selected_action,
        "{context} selected action"
    );
    let expected_phase = match reference.phase {
        ReferenceDndPhase::Idle => None,
        ReferenceDndPhase::Dragging => Some(DragSessionPhase::Dragging),
        ReferenceDndPhase::Dropped => Some(DragSessionPhase::DroppedAwaitingFinish),
        ReferenceDndPhase::Ask => Some(DragSessionPhase::DroppedAwaitingAskResolution),
        ReferenceDndPhase::Finished => Some(DragSessionPhase::Finished),
        ReferenceDndPhase::Cancelled => Some(DragSessionPhase::Cancelled),
    };
    assert_eq!(expected_phase, actual.phase, "{context} phase");
    assert_eq!(
        reference.terminal_events, actual.terminal_events,
        "{context} terminal events"
    );
    assert_eq!(
        reference.duplicate_terminal_attempts, actual.duplicate_terminal_attempts,
        "{context} duplicate terminal count"
    );
    assert_eq!(
        reference.source_cancelled_events, actual.source_cancelled_events,
        "{context} source cancelled events"
    );
    assert_eq!(
        reference.source_finished_events, actual.source_finished_events,
        "{context} source finished events"
    );
    assert_eq!(
        reference.offer_action_events, actual.offer_action_events,
        "{context} offer action events"
    );
    assert_eq!(
        reference.source_action_events, actual.source_action_events,
        "{context} source action events"
    );
    assert!(!actual.drag_icon_live, "{context} unexpected drag icon");
    assert_eq!(
        actual.orphaned_resources, 0,
        "{context} orphaned DnD resources"
    );
}

#[test]
fn dnd_model_comparison_rejects_intentional_divergence() {
    let reference = ReferenceDndModel::default();
    let production = ProductionDndModel::new();
    let mut actual = production.snapshot();
    actual.active = true;
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            assert_dnd_snapshot_matches(&reference, &actual, "intentional divergence");
        }))
        .is_err()
    );
}

#[test]
fn dnd_production_state_seeded_model_runs_100_000_transitions() {
    const SEED: u64 = 0xdad5_0000_0042;
    let mut random = SEED;
    let mut reference = ReferenceDndModel::default();
    let mut production = ProductionDndModel::new();

    for operation in 0..100_000_u32 {
        random = random
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        let op = match (random >> 32) % 19 {
            0 => DndModelOp::CreateSource,
            1 => DndModelOp::OfferMime,
            2 => DndModelOp::SetSourceActions((random as u32) & 7),
            3 => DndModelOp::StartDrag(random & 1 != 0),
            4 => DndModelOp::Enter,
            5 => DndModelOp::Leave,
            6 => DndModelOp::Accept(random & 1 != 0),
            7 => DndModelOp::SetDestinationActions((random as u32) & 7, (random >> 8) as u32 & 7),
            8 => DndModelOp::Receive,
            9 => DndModelOp::Drop,
            10 => DndModelOp::ResolveAsk,
            11 => DndModelOp::Finish,
            12 => DndModelOp::DestroyOffer,
            13 => DndModelOp::DestroySource,
            14 => DndModelOp::DisconnectSource,
            15 => DndModelOp::DisconnectTarget,
            16 => DndModelOp::Cancel,
            17 => DndModelOp::Suspend,
            _ => DndModelOp::Shutdown,
        };
        reference.apply(op);
        production.apply(op);
        let actual = production.snapshot();
        assert_dnd_snapshot_matches(
            &reference,
            &actual,
            &format!("seed={SEED:#x} operation={operation}"),
        );
    }
}

#[test]
fn removing_unrelated_same_client_data_device_keeps_drag_offer_alive() {
    let mut model = ProductionDndModel::new();
    model.apply(DndModelOp::CreateSource);
    model.apply(DndModelOp::OfferMime);
    model.apply(DndModelOp::SetSourceActions(1));
    model.apply(DndModelOp::StartDrag(true));
    model.apply(DndModelOp::Enter);

    let offer = model
        .current_offer()
        .expect("the active drag has a Wayland target offer");
    let binding = model
        .state
        .data_offers
        .get(&offer.id())
        .expect("the active drag offer has a protocol binding");
    assert_eq!(binding.target_id, model.target_device.id().protocol_id());
    assert_eq!(
        model.same_client_device.client().unwrap().id(),
        model.target_device.client().unwrap().id()
    );

    model.state.remove_data_device(&model.same_client_device);

    assert!(model.state.active_drag.is_some());
    assert!(model.state.data_offers.contains_key(&offer.id()));
    assert_eq!(model.state.compliance_metrics.dnd_sessions_cancelled, 0);
}
