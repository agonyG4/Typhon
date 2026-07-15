use super::*;
use std::{fs::File, sync::Arc};
use wayland_server::{Client, Display};

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
                        || self.phase == ReferenceDndPhase::Ask)
                    && actions <= 7
                    && (preferred == 0
                        || (preferred.count_ones() == 1 && actions & preferred != 0)) =>
            {
                self.destination_actions = Some(actions);
                if self.phase == ReferenceDndPhase::Dragging {
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
                } else if preferred != 0 {
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
        self.state
            .active_drag
            .as_ref()
            .and_then(|drag| drag.offer.clone())
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
                let x = if self
                    .state
                    .active_drag
                    .as_ref()
                    .is_some_and(|drag| drag.source.is_some())
                {
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
                    self.state.update_drag_actions(&offer, actions, preferred);
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
                    self.state.update_drag_actions(&offer, 1 | 2, 1);
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
            .and_then(|drag| drag.offer.as_ref())
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
            source_attached: active.is_some_and(|drag| drag.source.is_some()),
            target_present: active.is_some_and(|drag| drag.target_surface.is_some()),
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
