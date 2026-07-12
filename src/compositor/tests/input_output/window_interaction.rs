use super::*;

#[test]
fn window_interaction_absolute_motion_targets_only_original_surface() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_with_input_capabilities(
        &socket_name,
        InputProtocolCapabilities {
            relative_pointer: true,
            ..InputProtocolCapabilities::desktop_baseline()
        },
    )
    .unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection_a = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals_a, mut queue_a) = registry_queue_init::<RegistryTestState>(&connection_a).unwrap();
    let qh_a = queue_a.handle();
    let compositor_a: client_wl_compositor::WlCompositor =
        globals_a.bind(&qh_a, 1..=6, ()).unwrap();
    let wm_base_a: client_xdg_wm_base::XdgWmBase = globals_a.bind(&qh_a, 1..=6, ()).unwrap();
    let shm_a: client_wl_shm::WlShm = globals_a.bind(&qh_a, 1..=2, ()).unwrap();
    let seat_a: client_wl_seat::WlSeat = globals_a.bind(&qh_a, 1..=7, ()).unwrap();
    let pointer_a = seat_a.get_pointer(&qh_a, ());
    let _relative_a = globals_a
        .bind::<client_zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1, _, _>(
            &qh_a,
            1..=1,
            (),
        )
        .unwrap()
        .get_relative_pointer(&pointer_a, &qh_a, ());
    let (surface_a, _xdg_surface_a, _toplevel_a) =
        create_test_buffered_toplevel(&compositor_a, &wm_base_a, &shm_a, &qh_a, 160, 120).unwrap();
    surface_a.commit();
    connection_a.flush().unwrap();

    let connection_b = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals_b, mut queue_b) = registry_queue_init::<RegistryTestState>(&connection_b).unwrap();
    let qh_b = queue_b.handle();
    let compositor_b: client_wl_compositor::WlCompositor =
        globals_b.bind(&qh_b, 1..=6, ()).unwrap();
    let wm_base_b: client_xdg_wm_base::XdgWmBase = globals_b.bind(&qh_b, 1..=6, ()).unwrap();
    let shm_b: client_wl_shm::WlShm = globals_b.bind(&qh_b, 1..=2, ()).unwrap();
    let seat_b: client_wl_seat::WlSeat = globals_b.bind(&qh_b, 1..=7, ()).unwrap();
    let pointer_b = seat_b.get_pointer(&qh_b, ());
    let (surface_b, _xdg_surface_b, _toplevel_b) =
        create_test_buffered_toplevel(&compositor_b, &wm_base_b, &shm_b, &qh_b, 160, 120).unwrap();
    surface_b.commit();
    connection_b.flush().unwrap();

    let mut state_a = RegistryTestState::default();
    let mut state_b = RegistryTestState::default();
    queue_a.roundtrip(&mut state_a).unwrap();
    queue_b.roundtrip(&mut state_b).unwrap();

    let start_x = f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0;
    let start_y = f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0;
    commands
        .send(ServerCommand::PointerMotion {
            x: start_x,
            y: start_y,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue_a.roundtrip(&mut state_a).unwrap();
    queue_b.roundtrip(&mut state_b).unwrap();
    assert_eq!(state_a.pointer_enter_count, 1);
    assert_eq!(state_b.pointer_enter_count, 0);
    let motion_a_before = state_a
        .pointer_event_log
        .iter()
        .filter(|event| **event == "motion")
        .count();
    let enter_a_before = state_a.pointer_enter_count;
    let leave_a_before = state_a.pointer_leave_count;
    let enter_b_before = state_b.pointer_enter_count;
    let leave_b_before = state_b.pointer_leave_count;

    commands
        .send(ServerCommand::BeginMove {
            x: start_x,
            y: start_y,
        })
        .unwrap();
    commands
        .send(ServerCommand::UpdateInteraction { x: 140.0, y: 140.0 })
        .unwrap();
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::SendWindowInteractionPointerMotion {
            timestamp_usec: 42_000,
            x: 140.0,
            y: 140.0,
            reply,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue_a.roundtrip(&mut state_a).unwrap();
    queue_b.roundtrip(&mut state_b).unwrap();

    assert_eq!(receiver.recv().unwrap(), 1);
    assert_eq!(
        state_a
            .pointer_event_log
            .iter()
            .filter(|event| **event == "motion")
            .count(),
        motion_a_before + 1
    );
    assert_eq!(
        state_b
            .pointer_event_log
            .iter()
            .filter(|event| **event == "motion")
            .count(),
        0
    );
    assert_eq!(state_a.pointer_surface_x, Some(20.0));
    assert_eq!(state_a.pointer_surface_y, Some(14.0));
    assert_eq!(state_a.pointer_enter_count, enter_a_before);
    assert_eq!(state_a.pointer_leave_count, leave_a_before);
    assert_eq!(state_a.relative_motion_count, 0);
    assert_eq!(state_b.pointer_enter_count, enter_b_before);
    assert_eq!(state_b.pointer_leave_count, leave_b_before);
    assert_eq!(state_b.pointer_event_log, Vec::<&'static str>::new());

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    let _ = pointer_a;
    let _ = pointer_b;
}

#[test]
fn window_interaction_motion_preserves_exact_subsurface_target() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let subcompositor: client_wl_subcompositor::WlSubcompositor =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let _pointer = seat.get_pointer(&qh, ());
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=2, ()).unwrap();

    let (parent, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    parent.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();

    let child = compositor.create_surface(&qh, ());
    let _subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    _subsurface.set_position(10, 20);
    commit_test_buffered_surface(&child, &shm, &qh, 40, 30).unwrap();
    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let x = f64::from(render::FIRST_SURFACE_OFFSET.0) + 15.0;
    let y = f64::from(render::FIRST_SURFACE_OFFSET.1) + 25.0;
    commands
        .send(ServerCommand::PointerMotion { x, y })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(
        state.pointer_enter_surface_id,
        Some(child.id().protocol_id())
    );
    let enter_count = state.pointer_enter_count;
    let leave_count = state.pointer_leave_count;
    state.pointer_motion = false;
    state.pointer_surface_x = None;
    state.pointer_surface_y = None;

    commands.send(ServerCommand::BeginMove { x, y }).unwrap();
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::SendWindowInteractionPointerMotion {
            timestamp_usec: 43_000,
            x,
            y,
            reply,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    assert_eq!(receiver.recv().unwrap(), 1);
    assert!(state.pointer_motion);
    assert_eq!(state.pointer_surface_x, Some(5.0));
    assert_eq!(state.pointer_surface_y, Some(5.0));
    assert_eq!(state.pointer_enter_count, enter_count);
    assert_eq!(state.pointer_leave_count, leave_count);

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();
}

fn run_resize_motion_coordinate_regression(
    start_local: (f64, f64),
    update_output: (f64, f64),
    expected_local: (f64, f64),
) {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let _pointer = seat.get_pointer(&qh, ());
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=2, ()).unwrap();
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    surface.commit();
    connection.flush().unwrap();

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    let start_output = (
        f64::from(render::FIRST_SURFACE_OFFSET.0) + start_local.0,
        f64::from(render::FIRST_SURFACE_OFFSET.1) + start_local.1,
    );
    commands
        .send(ServerCommand::PointerMotion {
            x: start_output.0,
            y: start_output.1,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let enter_count = state.pointer_enter_count;
    let leave_count = state.pointer_leave_count;
    state.pointer_motion = false;
    state.pointer_surface_x = None;
    state.pointer_surface_y = None;

    commands
        .send(ServerCommand::BeginResize {
            x: start_output.0,
            y: start_output.1,
        })
        .unwrap();
    let (update_reply, update_receiver) = mpsc::channel();
    commands
        .send(ServerCommand::UpdateInteractionResult {
            x: update_output.0,
            y: update_output.1,
            reply: update_reply,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    assert!(update_receiver.recv().unwrap());
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::SendWindowInteractionPointerMotion {
            timestamp_usec: 44_000,
            x: update_output.0,
            y: update_output.1,
            reply,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    assert_eq!(receiver.recv().unwrap(), 1);
    assert!(state.pointer_motion);
    assert_eq!(state.pointer_surface_x, Some(expected_local.0));
    assert_eq!(state.pointer_surface_y, Some(expected_local.1));
    assert_eq!(state.pointer_enter_count, enter_count);
    assert_eq!(state.pointer_leave_count, leave_count);

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();
}

#[test]
fn left_resize_dispatches_client_motion_using_updated_surface_origin() {
    run_resize_motion_coordinate_regression((0.0, 40.0), (52.0, 112.0), (0.0, 40.0));
}

#[test]
fn top_resize_dispatches_client_motion_using_updated_surface_origin() {
    run_resize_motion_coordinate_regression((40.0, 0.0), (112.0, 52.0), (40.0, 0.0));
}

#[test]
fn right_bottom_resize_dispatches_client_motion_without_origin_change() {
    run_resize_motion_coordinate_regression((159.0, 119.0), (251.0, 211.0), (179.0, 139.0));
}
