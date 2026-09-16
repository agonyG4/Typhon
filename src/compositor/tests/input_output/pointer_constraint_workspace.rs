use super::*;

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

struct WorkspacePointerConstraintFixture {
    connection: Connection,
    queue: EventQueue<RegistryTestState>,
    state: RegistryTestState,
    constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1,
    pointer: client_wl_pointer::WlPointer,
    surface: client_wl_surface::WlSurface,
    _other_surface: client_wl_surface::WlSurface,
}

fn workspace_pointer_constraint_fixture(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> WorkspacePointerConstraintFixture {
    let stream = UnixStream::connect(socket_path).unwrap();
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

    let (other_surface, _other_xdg_surface, _other_toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    other_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let other_snapshot = capture_xdg_role_snapshot(commands, other_surface.id().protocol_id());
    let other_window_id = capture_window_id_for_surface(commands, other_snapshot.surface_id)
        .expect("second workspace window should be registered");
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::MoveWindowToWorkspace {
            window_id: other_window_id,
            workspace: 2,
            reply,
        })
        .unwrap();
    wait_for_server_commands(commands);
    assert!(receiver.recv().unwrap());

    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
        })
        .unwrap();
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state).unwrap();

    WorkspacePointerConstraintFixture {
        connection,
        queue,
        state,
        constraints,
        pointer,
        surface,
        _other_surface: other_surface,
    }
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
    let subcompositor: client_wl_subcompositor::WlSubcompositor =
        globals.bind(&qh, 1..=1, ()).unwrap();
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
fn persistent_locked_pointer_releases_when_workspace_owner_departs() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        pointer_constraints: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let mut fixture = workspace_pointer_constraint_fixture(&socket_path, &commands);
    let qh = fixture.queue.handle();

    let _lock = fixture.constraints.lock_pointer(
        &fixture.surface,
        &fixture.pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    fixture.surface.commit();
    fixture.connection.flush().unwrap();
    wait_for_server_commands(&commands);
    fixture.queue.roundtrip(&mut fixture.state).unwrap();
    let constraint_id = capture_pointer_constraint_ids(&commands)
        .into_iter()
        .next()
        .expect("persistent lock should be registered");
    let old_focus = capture_pointer_focus_surface_id(&commands);
    assert!(old_focus.is_some());
    let backend_id = activate_locked_backend(&commands, &mut fixture.state, &mut fixture.queue);

    commands
        .send(ServerCommand::ActivateWorkspace { workspace: 2 })
        .unwrap();
    wait_for_server_commands(&commands);

    let snapshot = capture_pointer_constraint_snapshot(&commands, constraint_id)
        .expect("persistent constraint remains registered");
    assert!(!snapshot.active);
    assert!(!snapshot.backend_pending);
    assert!(!snapshot.defunct);
    assert_ne!(capture_pointer_focus_surface_id(&commands), old_focus);
    assert!(capture_pending_locked_pointer_reveal(&commands));
    assert!(capture_cursor_hidden_by_pointer_lock(&commands));
    assert!(
        capture_pointer_constraint_backend_requests(&commands)
            .iter()
            .any(|request| {
                matches!(
                    request,
                    PointerConstraintBackendRequest::Deactivate { id, .. } if *id == backend_id
                )
            })
    );

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();
}

#[test]
fn persistent_confined_pointer_releases_when_workspace_owner_departs() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        pointer_constraints: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let mut fixture = workspace_pointer_constraint_fixture(&socket_path, &commands);
    let qh = fixture.queue.handle();

    let _confine = fixture.constraints.confine_pointer(
        &fixture.surface,
        &fixture.pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    fixture.surface.commit();
    fixture.connection.flush().unwrap();
    wait_for_server_commands(&commands);
    fixture.queue.roundtrip(&mut fixture.state).unwrap();
    let constraint_id = capture_pointer_constraint_ids(&commands)
        .into_iter()
        .next()
        .expect("persistent confinement should be registered");
    let backend_id = activate_confined_backend(&commands, &mut fixture.state, &mut fixture.queue);

    commands
        .send(ServerCommand::ActivateWorkspace { workspace: 2 })
        .unwrap();
    wait_for_server_commands(&commands);

    let snapshot = capture_pointer_constraint_snapshot(&commands, constraint_id)
        .expect("persistent constraint remains registered");
    assert!(!snapshot.active);
    assert!(!snapshot.backend_pending);
    assert!(!snapshot.defunct);
    assert_eq!(capture_pointer_focus_surface_id(&commands), None);
    let deactivation_requests = capture_pointer_constraint_backend_requests(&commands);
    assert!(deactivation_requests.iter().any(|request| {
        matches!(
            request,
            PointerConstraintBackendRequest::Deactivate { id, .. } if *id == backend_id
        )
    }));

    commands
        .send(ServerCommand::PointerConstraintBackendDeactivated(
            backend_id,
        ))
        .unwrap();
    wait_for_server_commands(&commands);
    fixture.queue.roundtrip(&mut fixture.state).unwrap();
    commands
        .send(ServerCommand::ActivateWorkspace { workspace: 1 })
        .unwrap();
    wait_for_server_commands(&commands);
    assert!(
        capture_pointer_constraint_backend_requests(&commands)
            .iter()
            .any(|request| {
                matches!(
                    request,
                    PointerConstraintBackendRequest::ActivateConfined { id, .. }
                        if id.constraint_id == constraint_id
                )
            })
    );

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();
}

#[test]
fn pending_locked_activation_is_canceled_when_workspace_owner_departs() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        pointer_constraints: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let mut fixture = workspace_pointer_constraint_fixture(&socket_path, &commands);
    let qh = fixture.queue.handle();

    let _lock = fixture.constraints.lock_pointer(
        &fixture.surface,
        &fixture.pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    fixture.surface.commit();
    fixture.connection.flush().unwrap();
    wait_for_server_commands(&commands);
    fixture.queue.roundtrip(&mut fixture.state).unwrap();
    let activation = capture_pointer_constraint_backend_requests(&commands)
        .into_iter()
        .find_map(|request| match request {
            PointerConstraintBackendRequest::ActivateLocked { id } => Some(id),
            _ => None,
        })
        .expect("locked activation should be pending");
    let constraint_id = capture_pointer_constraint_ids(&commands)
        .into_iter()
        .next()
        .expect("pending lock should be registered");

    commands
        .send(ServerCommand::ActivateWorkspace { workspace: 2 })
        .unwrap();
    wait_for_server_commands(&commands);
    let snapshot = capture_pointer_constraint_snapshot(&commands, constraint_id)
        .expect("persistent constraint remains registered");
    assert!(!snapshot.active);
    assert!(!snapshot.backend_pending);
    assert!(!snapshot.defunct);
    assert_eq!(capture_pointer_focus_surface_id(&commands), None);

    commands
        .send(ServerCommand::PointerConstraintBackendActivated(activation))
        .unwrap();
    wait_for_server_commands(&commands);
    let late_snapshot = capture_pointer_constraint_snapshot(&commands, constraint_id)
        .expect("canceled persistent constraint remains registered");
    assert!(!late_snapshot.active);
    assert!(!late_snapshot.backend_pending);
    assert!(
        capture_pointer_constraint_backend_requests(&commands)
            .iter()
            .all(|request| {
                !matches!(request, PointerConstraintBackendRequest::Deactivate { .. })
            })
    );

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();
}

#[test]
fn oneshot_locked_pointer_does_not_reactivate_after_workspace_departure() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        pointer_constraints: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let mut fixture = workspace_pointer_constraint_fixture(&socket_path, &commands);
    let qh = fixture.queue.handle();

    let _lock = fixture.constraints.lock_pointer(
        &fixture.surface,
        &fixture.pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Oneshot,
        &qh,
        (),
    );
    fixture.surface.commit();
    fixture.connection.flush().unwrap();
    wait_for_server_commands(&commands);
    fixture.queue.roundtrip(&mut fixture.state).unwrap();
    let constraint_id = capture_pointer_constraint_ids(&commands)
        .into_iter()
        .next()
        .expect("oneshot lock should be registered");
    let backend_id = activate_locked_backend(&commands, &mut fixture.state, &mut fixture.queue);

    commands
        .send(ServerCommand::ActivateWorkspace { workspace: 2 })
        .unwrap();
    wait_for_server_commands(&commands);
    let snapshot = capture_pointer_constraint_snapshot(&commands, constraint_id)
        .expect("oneshot constraint remains until backend settlement");
    assert!(!snapshot.active);
    assert!(snapshot.defunct);
    let requests = capture_pointer_constraint_backend_requests(&commands);
    assert!(requests.iter().any(|request| {
        matches!(
            request,
            PointerConstraintBackendRequest::Deactivate { id, .. } if *id == backend_id
        )
    }));

    commands
        .send(ServerCommand::PointerConstraintBackendDeactivated(
            backend_id,
        ))
        .unwrap();
    wait_for_server_commands(&commands);
    fixture.queue.roundtrip(&mut fixture.state).unwrap();
    commands
        .send(ServerCommand::ActivateWorkspace { workspace: 1 })
        .unwrap();
    wait_for_server_commands(&commands);
    let return_requests = capture_pointer_constraint_backend_requests(&commands);
    assert!(return_requests.iter().all(|request| {
        !matches!(
            request,
            PointerConstraintBackendRequest::ActivateLocked { .. }
        )
    }));
    let return_snapshot = capture_pointer_constraint_snapshot(&commands, constraint_id)
        .expect("oneshot constraint remains committed for its existing lifetime");
    assert!(return_snapshot.defunct);

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();
}

#[test]
fn activating_current_workspace_preserves_valid_locked_pointer_constraint() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        pointer_constraints: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let mut fixture = workspace_pointer_constraint_fixture(&socket_path, &commands);
    let qh = fixture.queue.handle();

    let _lock = fixture.constraints.lock_pointer(
        &fixture.surface,
        &fixture.pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    fixture.surface.commit();
    fixture.connection.flush().unwrap();
    wait_for_server_commands(&commands);
    fixture.queue.roundtrip(&mut fixture.state).unwrap();
    let constraint_id = capture_pointer_constraint_ids(&commands)
        .into_iter()
        .next()
        .expect("persistent lock should be registered");
    let _backend_id = activate_locked_backend(&commands, &mut fixture.state, &mut fixture.queue);

    commands
        .send(ServerCommand::ActivateWorkspace { workspace: 1 })
        .unwrap();
    wait_for_server_commands(&commands);
    let snapshot = capture_pointer_constraint_snapshot(&commands, constraint_id)
        .expect("valid constraint remains registered");
    assert!(snapshot.active);
    assert!(capture_pointer_focus_surface_id(&commands).is_some());
    assert!(
        capture_pointer_constraint_backend_requests(&commands)
            .iter()
            .all(|request| {
                !matches!(request, PointerConstraintBackendRequest::Deactivate { .. })
            })
    );

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();
}

#[test]
fn child_surface_constraint_releases_with_departing_application_root() {
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
    let mut state = RegistryTestState::default();
    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 40.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 40.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    fixture.queue.roundtrip(&mut state).unwrap();
    let child_surface_id =
        capture_xdg_role_snapshot(&commands, fixture.child.id().protocol_id()).surface_id;
    assert_eq!(
        capture_pointer_scene_hit(
            &commands,
            f64::from(render::FIRST_SURFACE_OFFSET.0) + 40.0,
            f64::from(render::FIRST_SURFACE_OFFSET.1) + 40.0,
        )
        .0,
        Some(child_surface_id),
        "pointer should hit the constrained child surface"
    );
    assert_eq!(
        capture_pointer_focus_surface_id(&commands),
        Some(child_surface_id),
        "pointer focus should be the constrained child surface"
    );

    let qh = fixture.queue.handle();
    let _lock = fixture.constraints.lock_pointer(
        &fixture.child,
        &fixture.pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    fixture.child.commit();
    fixture.parent.commit();
    fixture.connection.flush().unwrap();
    wait_for_server_commands(&commands);
    fixture.queue.roundtrip(&mut state).unwrap();
    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 40.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 40.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    fixture.queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);
    let constraint_id = capture_pointer_constraint_ids(&commands)
        .into_iter()
        .next()
        .expect("child lock should be registered");
    let backend_id = capture_pointer_constraint_backend_requests(&commands)
        .into_iter()
        .find_map(|request| match request {
            PointerConstraintBackendRequest::ActivateLocked { id, .. } => Some(id),
            _ => None,
        })
        .expect("expected child lock activation request");
    commands
        .send(ServerCommand::PointerConstraintBackendActivated(backend_id))
        .unwrap();
    wait_for_server_commands(&commands);
    fixture.queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.locked_count, 1);
    assert!(capture_pointer_focus_surface_id(&commands).is_some());

    commands
        .send(ServerCommand::ActivateWorkspace { workspace: 2 })
        .unwrap();
    wait_for_server_commands(&commands);
    let snapshot = capture_pointer_constraint_snapshot(&commands, constraint_id)
        .expect("persistent child constraint remains registered");
    assert!(!snapshot.active);
    assert!(!snapshot.backend_pending);
    assert!(!snapshot.defunct);
    assert!(
        capture_pointer_constraint_backend_requests(&commands)
            .iter()
            .any(|request| {
                matches!(
                    request,
                    PointerConstraintBackendRequest::Deactivate { id, .. } if *id == backend_id
                )
            })
    );

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();
}

#[test]
fn moving_constrained_window_family_off_active_workspace_releases_pointer_route() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        pointer_constraints: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let mut fixture = workspace_pointer_constraint_fixture(&socket_path, &commands);
    let qh = fixture.queue.handle();

    let _lock = fixture.constraints.lock_pointer(
        &fixture.surface,
        &fixture.pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    fixture.surface.commit();
    fixture.connection.flush().unwrap();
    wait_for_server_commands(&commands);
    fixture.queue.roundtrip(&mut fixture.state).unwrap();
    let constraint_id = capture_pointer_constraint_ids(&commands)
        .into_iter()
        .next()
        .expect("persistent lock should be registered");
    let backend_id = activate_locked_backend(&commands, &mut fixture.state, &mut fixture.queue);
    let window_id = capture_focused_window_id(&commands).expect("surface should own focus");
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::MoveWindowToWorkspace {
            window_id,
            workspace: 2,
            reply,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    assert!(receiver.recv().unwrap());

    let snapshot = capture_pointer_constraint_snapshot(&commands, constraint_id)
        .expect("persistent constraint remains registered");
    assert!(!snapshot.active);
    assert!(!snapshot.backend_pending);
    assert!(!snapshot.defunct);
    assert_eq!(capture_pointer_focus_surface_id(&commands), None);
    assert!(
        capture_pointer_constraint_backend_requests(&commands)
            .iter()
            .any(|request| {
                matches!(
                    request,
                    PointerConstraintBackendRequest::Deactivate { id, .. } if *id == backend_id
                )
            })
    );

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();
}

#[test]
fn closing_special_workspace_releases_constraint_owned_by_departing_window() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        pointer_constraints: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let mut fixture = workspace_pointer_constraint_fixture(&socket_path, &commands);
    let qh = fixture.queue.handle();

    let _lock = fixture.constraints.lock_pointer(
        &fixture.surface,
        &fixture.pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    fixture.surface.commit();
    fixture.connection.flush().unwrap();
    wait_for_server_commands(&commands);
    fixture.queue.roundtrip(&mut fixture.state).unwrap();
    let constraint_id = capture_pointer_constraint_ids(&commands)
        .into_iter()
        .next()
        .expect("persistent lock should be registered");
    let backend_id = activate_locked_backend(&commands, &mut fixture.state, &mut fixture.queue);

    commands
        .send(ServerCommand::ToggleDefaultSpecialWorkspace)
        .unwrap();
    wait_for_server_commands(&commands);
    commands
        .send(ServerCommand::MoveFocusedWindowToOrFromSpecialWorkspace)
        .unwrap();
    wait_for_server_commands(&commands);
    assert!(
        capture_pointer_constraint_snapshot(&commands, constraint_id)
            .expect("constraint remains registered")
            .active
    );

    commands
        .send(ServerCommand::ToggleDefaultSpecialWorkspace)
        .unwrap();
    wait_for_server_commands(&commands);
    let snapshot = capture_pointer_constraint_snapshot(&commands, constraint_id)
        .expect("persistent constraint remains registered");
    assert!(!snapshot.active);
    assert!(!snapshot.backend_pending);
    assert!(!snapshot.defunct);
    assert_eq!(capture_pointer_focus_surface_id(&commands), None);
    assert!(
        capture_pointer_constraint_backend_requests(&commands)
            .iter()
            .any(|request| {
                matches!(
                    request,
                    PointerConstraintBackendRequest::Deactivate { id, .. } if *id == backend_id
                )
            })
    );

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();
}

#[test]
fn minimizing_locked_window_releases_pointer_route_before_focus_reconciliation() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        pointer_constraints: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let mut fixture = workspace_pointer_constraint_fixture(&socket_path, &commands);
    let qh = fixture.queue.handle();

    let _lock = fixture.constraints.lock_pointer(
        &fixture.surface,
        &fixture.pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    fixture.surface.commit();
    fixture.connection.flush().unwrap();
    wait_for_server_commands(&commands);
    fixture.queue.roundtrip(&mut fixture.state).unwrap();
    let constraint_id = capture_pointer_constraint_ids(&commands)
        .into_iter()
        .next()
        .expect("persistent lock should be registered");
    let backend_id = activate_locked_backend(&commands, &mut fixture.state, &mut fixture.queue);

    commands.send(ServerCommand::MinimizeFocused).unwrap();
    wait_for_server_commands(&commands);
    let snapshot = capture_pointer_constraint_snapshot(&commands, constraint_id)
        .expect("persistent constraint remains registered");
    assert!(!snapshot.active);
    assert!(!snapshot.backend_pending);
    assert!(!snapshot.defunct);
    assert_eq!(capture_pointer_focus_surface_id(&commands), None);
    assert!(
        capture_pointer_constraint_backend_requests(&commands)
            .iter()
            .any(|request| {
                matches!(
                    request,
                    PointerConstraintBackendRequest::Deactivate { id, .. } if *id == backend_id
                )
            })
    );

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();
}

#[test]
fn minimizing_confined_window_releases_pointer_route() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        pointer_constraints: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let mut fixture = workspace_pointer_constraint_fixture(&socket_path, &commands);
    let qh = fixture.queue.handle();

    let _confine = fixture.constraints.confine_pointer(
        &fixture.surface,
        &fixture.pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    fixture.surface.commit();
    fixture.connection.flush().unwrap();
    wait_for_server_commands(&commands);
    fixture.queue.roundtrip(&mut fixture.state).unwrap();
    let constraint_id = capture_pointer_constraint_ids(&commands)
        .into_iter()
        .next()
        .expect("persistent confinement should be registered");
    let backend_id = activate_confined_backend(&commands, &mut fixture.state, &mut fixture.queue);

    commands.send(ServerCommand::MinimizeFocused).unwrap();
    wait_for_server_commands(&commands);
    let snapshot = capture_pointer_constraint_snapshot(&commands, constraint_id)
        .expect("persistent constraint remains registered");
    assert!(!snapshot.active);
    assert!(!snapshot.backend_pending);
    assert!(!snapshot.defunct);
    assert_eq!(capture_pointer_focus_surface_id(&commands), None);
    assert!(
        capture_pointer_constraint_backend_requests(&commands)
            .iter()
            .any(|request| {
                matches!(
                    request,
                    PointerConstraintBackendRequest::Deactivate { id, .. } if *id == backend_id
                )
            })
    );

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();
}

#[test]
fn ordinary_locked_motion_keeps_visible_constraint_active() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        pointer_constraints: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let mut fixture = workspace_pointer_constraint_fixture(&socket_path, &commands);
    let qh = fixture.queue.handle();

    let _lock = fixture.constraints.lock_pointer(
        &fixture.surface,
        &fixture.pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    fixture.surface.commit();
    fixture.connection.flush().unwrap();
    wait_for_server_commands(&commands);
    fixture.queue.roundtrip(&mut fixture.state).unwrap();
    let constraint_id = capture_pointer_constraint_ids(&commands)
        .into_iter()
        .next()
        .expect("persistent lock should be registered");
    let _backend_id = activate_locked_backend(&commands, &mut fixture.state, &mut fixture.queue);
    let _ = capture_pointer_constraint_backend_requests(&commands);

    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 1000.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 1000.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    let snapshot = capture_pointer_constraint_snapshot(&commands, constraint_id)
        .expect("visible constraint remains registered");
    assert!(snapshot.active);
    assert!(
        capture_pointer_constraint_backend_requests(&commands)
            .iter()
            .all(|request| {
                !matches!(request, PointerConstraintBackendRequest::Deactivate { .. })
            })
    );

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();
}
