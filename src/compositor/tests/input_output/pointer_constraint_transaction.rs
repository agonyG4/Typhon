use super::*;
use crate::compositor::state_data::{InputRegionOp, InputRegionRect};

fn activate_locked_backend(
    commands: &Sender<ServerCommand>,
    state: &mut RegistryTestState,
    queue: &mut EventQueue<RegistryTestState>,
) -> PointerConstraintBackendId {
    let requests = capture_pointer_constraint_backend_requests(commands);
    let id = requests
        .iter()
        .find_map(|request| match request {
            PointerConstraintBackendRequest::ActivateLocked { id, .. } => Some(*id),
            _ => None,
        })
        .expect("expected locked pointer activation request");
    commands
        .send(ServerCommand::PointerConstraintBackendActivated(id))
        .unwrap();
    wait_for_server_commands(commands);
    queue.roundtrip(state).unwrap();
    assert_eq!(state.locked_count, 1);
    id
}

fn activate_confined_backend(
    commands: &Sender<ServerCommand>,
    state: &mut RegistryTestState,
    queue: &mut EventQueue<RegistryTestState>,
) -> PointerConstraintBackendId {
    let requests = capture_pointer_constraint_backend_requests(commands);
    let id = requests
        .iter()
        .find_map(|request| match request {
            PointerConstraintBackendRequest::ActivateConfined { id, .. } => Some(*id),
            _ => None,
        })
        .expect("expected confined pointer activation request");
    commands
        .send(ServerCommand::PointerConstraintBackendActivated(id))
        .unwrap();
    wait_for_server_commands(commands);
    queue.roundtrip(state).unwrap();
    assert_eq!(state.confined_count, 1);
    id
}

#[test]
fn active_locked_destroy_without_surface_commit_keeps_current_routing() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        pointer_constraints: true,
        relative_pointer: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=2, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let relative_manager: client_zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let pointer = seat.get_pointer(&qh, ());
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();

    surface.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let _relative_pointer = relative_manager.get_relative_pointer(&pointer, &qh, ());
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    let lock = constraints.lock_pointer(
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
    queue.roundtrip(&mut state).unwrap();
    let backend_id = activate_locked_backend(&commands, &mut state, &mut queue);

    state.locked_count = 0;
    state.unlocked_count = 0;
    state.pointer_motion = false;
    state.relative_motion_count = 0;
    lock.destroy();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    let before_commit_requests = capture_pointer_constraint_backend_requests(&commands);
    let snapshot = capture_pointer_constraint_snapshot(&commands, backend_id.constraint_id)
        .expect("destroyed protocol resource must retain current constraint ownership");
    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 300.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 250.0,
        })
        .unwrap();
    commands
        .send(ServerCommand::PointerMotionSample(PointerMotionSample {
            timestamp_usec: 1,
            absolute: None,
            relative: Some(RelativePointerMotion {
                dx: 4.0,
                dy: -2.0,
                dx_unaccelerated: 4.0,
                dy_unaccelerated: -2.0,
            }),
        }))
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    assert!(before_commit_requests.iter().all(|request| !matches!(
        request,
        PointerConstraintBackendRequest::Deactivate { .. }
    )));
    assert!(snapshot.committed);
    assert!(!state.pointer_motion, "absolute motion must remain locked");
    assert_eq!(state.relative_motion_count, 1);
    assert_eq!(state.unlocked_count, 0);

    surface.commit();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let after_commit_requests = capture_pointer_constraint_backend_requests(&commands);
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert!(after_commit_requests.iter().any(|request| matches!(
        request,
        PointerConstraintBackendRequest::Deactivate { id, .. } if *id == backend_id
    )));
}

#[test]
fn active_confined_destroy_without_surface_commit_keeps_current_routing() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        pointer_constraints: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=2, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let pointer = seat.get_pointer(&qh, ());
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();

    surface.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    let region = compositor.create_region(&qh, ());
    region.add(0, 0, 160, 120);
    let confined = constraints.confine_pointer(
        &surface,
        &pointer,
        Some(&region),
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    surface.commit();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let backend_id = activate_confined_backend(&commands, &mut state, &mut queue);

    state.confined_count = 0;
    state.unconfined_count = 0;
    confined.destroy();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    let before_commit_requests = capture_pointer_constraint_backend_requests(&commands);
    let snapshot = capture_pointer_constraint_snapshot(&commands, backend_id.constraint_id)
        .expect("destroyed protocol resource must retain current constraint ownership");
    assert!(before_commit_requests.iter().all(|request| !matches!(
        request,
        PointerConstraintBackendRequest::Deactivate { .. }
    )));
    assert!(snapshot.committed);
    assert_eq!(state.unconfined_count, 0);

    surface.commit();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let after_commit_requests = capture_pointer_constraint_backend_requests(&commands);
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert!(after_commit_requests.iter().any(|request| matches!(
        request,
        PointerConstraintBackendRequest::Deactivate { id, .. } if *id == backend_id
    )));
}

#[test]
fn destroyed_pending_locked_activation_cannot_complete_as_ghost_lock() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        pointer_constraints: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=2, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let pointer = seat.get_pointer(&qh, ());
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    surface.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    let lock = constraints.lock_pointer(
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
    queue.roundtrip(&mut state).unwrap();
    let activation = capture_pointer_constraint_backend_requests(&commands)
        .into_iter()
        .find_map(|request| match request {
            PointerConstraintBackendRequest::ActivateLocked { id, .. } => Some(id),
            _ => None,
        })
        .expect("expected pending locked activation");
    lock.destroy();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let canceled_requests = capture_pointer_constraint_backend_requests(&commands);
    commands
        .send(ServerCommand::PointerConstraintBackendActivated(activation))
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let ids = capture_pointer_constraint_ids(&commands);
    let snapshot = capture_pointer_constraint_snapshot(&commands, activation.constraint_id)
        .expect("the committed topology remains owned until its removal commit");
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert!(canceled_requests.is_empty());
    assert_eq!(state.locked_count, 0);
    assert!(ids.contains(&activation.constraint_id));
    assert!(snapshot.committed);
}

#[test]
fn create_and_destroy_before_first_effective_commit_has_no_activation() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        pointer_constraints: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=2, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let pointer = seat.get_pointer(&qh, ());
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();

    surface.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    let lock = constraints.lock_pointer(
        &surface,
        &pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    lock.destroy();
    surface.commit();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let requests = capture_pointer_constraint_backend_requests(&commands);
    let ids = capture_pointer_constraint_ids(&commands);
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert!(requests.iter().all(|request| !matches!(
        request,
        PointerConstraintBackendRequest::ActivateLocked { .. }
    )));
    assert_eq!(state.locked_count, 0);
    assert!(ids.is_empty());
}

struct SynchronizedConstraintFixture {
    connection: Connection,
    queue: EventQueue<RegistryTestState>,
    compositor: client_wl_compositor::WlCompositor,
    constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1,
    pointer: client_wl_pointer::WlPointer,
    parent: client_wl_surface::WlSurface,
    child: client_wl_surface::WlSurface,
    _xdg_surface: client_xdg_surface::XdgSurface,
    _toplevel: client_xdg_toplevel::XdgToplevel,
    _subsurface: client_wl_subsurface::WlSubsurface,
}

fn synchronized_constraint_fixture(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> SynchronizedConstraintFixture {
    let stream = UnixStream::connect(socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=2, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let pointer = seat.get_pointer(&qh, ());
    let (parent, xdg_surface, toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();

    let child = compositor.create_surface(&qh, ());
    let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    subsurface.set_position(0, 0);
    commit_test_buffered_surface(&child, &shm, &qh, 160, 120).unwrap();
    parent.commit();
    connection.flush().unwrap();
    wait_for_server_commands(commands);
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();

    SynchronizedConstraintFixture {
        connection,
        queue,
        compositor,
        constraints,
        pointer,
        parent,
        child,
        _xdg_surface: xdg_surface,
        _toplevel: toplevel,
        _subsurface: subsurface,
    }
}

#[test]
fn captured_synchronized_install_remains_already_constrained() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        pointer_constraints: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let mut fixture = synchronized_constraint_fixture(&socket_path, &commands);
    let qh = fixture.queue.handle();
    let lock_a = fixture.constraints.lock_pointer(
        &fixture.child,
        &fixture.pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    fixture.child.commit();
    fixture.connection.flush().unwrap();
    wait_for_server_commands(&commands);

    let _lock_b = fixture.constraints.lock_pointer(
        &fixture.child,
        &fixture.pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    fixture.connection.flush().unwrap();
    let result = fixture.connection.roundtrip();
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();
    drop(lock_a);

    assert!(result.is_err(), "captured install must reject a second lock");
    let error = fixture
        .connection
        .protocol_error()
        .expect("AlreadyConstrained must be retained on the client connection");
    assert_eq!(error.object_interface, "zwp_pointer_constraints_v1");
    assert_eq!(error.code, 1);
}

#[test]
fn delayed_removal_does_not_transfer_a_hint_to_constraint_b() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        pointer_constraints: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let mut fixture = synchronized_constraint_fixture(&socket_path, &commands);
    let qh = fixture.queue.handle();
    let lock_a = fixture.constraints.lock_pointer(
        &fixture.child,
        &fixture.pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    lock_a.set_cursor_position_hint(11.0, 12.0);
    lock_a.destroy();
    fixture.child.commit();
    fixture.connection.flush().unwrap();
    wait_for_server_commands(&commands);

    let _lock_b = fixture.constraints.lock_pointer(
        &fixture.child,
        &fixture.pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    fixture.connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let b_id = *capture_pointer_constraint_ids(&commands)
        .first()
        .expect("constraint B must be registered");

    fixture.parent.commit();
    fixture.connection.flush().unwrap();
    wait_for_server_commands(&commands);
    fixture.queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    let before_b_commit = capture_pointer_constraint_snapshot(&commands, b_id)
        .expect("constraint B must remain registered before its install commit");
    assert_eq!(before_b_commit.committed_cursor_position_hint, None);

    let hint_b = (41.0, 42.0);
    _lock_b.set_cursor_position_hint(hint_b.0, hint_b.1);
    fixture.child.commit();
    fixture.parent.commit();
    fixture.connection.flush().unwrap();
    wait_for_server_commands(&commands);
    fixture.queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    let after_b_commit = capture_pointer_constraint_snapshot(&commands, b_id)
        .expect("constraint B must be current after its install commit");
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert_eq!(after_b_commit.committed_cursor_position_hint, Some(hint_b));
    assert_ne!(after_b_commit.committed_cursor_position_hint, Some((11.0, 12.0)));
}

#[test]
fn delayed_removal_does_not_transfer_a_region_to_constraint_b() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        pointer_constraints: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let mut fixture = synchronized_constraint_fixture(&socket_path, &commands);
    let qh = fixture.queue.handle();
    let region_a = fixture.compositor.create_region(&qh, ());
    region_a.add(1, 2, 10, 11);
    let lock_a = fixture.constraints.lock_pointer(
        &fixture.child,
        &fixture.pointer,
        Some(&region_a),
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    lock_a.destroy();
    fixture.child.commit();
    fixture.connection.flush().unwrap();
    wait_for_server_commands(&commands);

    let region_b = fixture.compositor.create_region(&qh, ());
    region_b.add(30, 31, 12, 13);
    let _lock_b = fixture.constraints.confine_pointer(
        &fixture.child,
        &fixture.pointer,
        Some(&region_b),
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    fixture.connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let b_id = *capture_pointer_constraint_ids(&commands)
        .first()
        .expect("constraint B must be registered");

    fixture.parent.commit();
    fixture.connection.flush().unwrap();
    wait_for_server_commands(&commands);
    fixture.queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    let before_b_commit = capture_pointer_constraint_snapshot(&commands, b_id)
        .expect("constraint B must remain registered before its install commit");
    assert_eq!(before_b_commit.committed_region, SurfaceInputRegion::Default);

    fixture.child.commit();
    fixture.parent.commit();
    fixture.connection.flush().unwrap();
    wait_for_server_commands(&commands);
    fixture.queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    let after_b_commit = capture_pointer_constraint_snapshot(&commands, b_id)
        .expect("constraint B must be current after its install commit");
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert_eq!(after_b_commit.committed_region, SurfaceInputRegion::Custom(vec![
        InputRegionOp::Add(InputRegionRect::new(30, 31, 12, 13).unwrap()),
    ]));
    assert_ne!(after_b_commit.committed_region, SurfaceInputRegion::Custom(vec![
        InputRegionOp::Add(InputRegionRect::new(1, 2, 10, 11).unwrap()),
    ]));
}
