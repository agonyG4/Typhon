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

fn start_workspace_pointer_constraint_test() -> (
    Sender<ServerCommand>,
    JoinHandle<OwnCompositorServer>,
    WorkspacePointerConstraintFixture,
) {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        pointer_constraints: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let fixture = workspace_pointer_constraint_fixture(&socket_path, &commands);
    (commands, server_thread, fixture)
}

struct WorkspacePointerConstraintFixture {
    connection: Connection,
    queue: EventQueue<RegistryTestState>,
    state: RegistryTestState,
    compositor: client_wl_compositor::WlCompositor,
    subcompositor: client_wl_subcompositor::WlSubcompositor,
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
    let subcompositor: client_wl_subcompositor::WlSubcompositor =
        globals.bind(&qh, 1..=1, ()).unwrap();
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
        compositor,
        subcompositor,
        constraints,
        pointer,
        surface,
        _other_surface: other_surface,
    }
}

#[test]
fn compositor_move_suspends_active_persistent_locked_pointer_without_hint_warp() {
    let (commands, server_thread, mut fixture) = start_workspace_pointer_constraint_test();
    let qh = fixture.queue.handle();

    let lock = fixture.constraints.lock_pointer(
        &fixture.surface,
        &fixture.pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    lock.set_cursor_position_hint(70.0, 50.0);
    fixture.surface.commit();
    fixture.connection.flush().unwrap();
    wait_for_server_commands(&commands);
    fixture.queue.roundtrip(&mut fixture.state).unwrap();
    let constraint_id = capture_pointer_constraint_ids(&commands)
        .into_iter()
        .next()
        .expect("persistent lock should be registered");
    let backend_id = activate_locked_backend(&commands, &mut fixture.state, &mut fixture.queue);
    assert_eq!(fixture.state.locked_count, 1);

    let ids_before_unrelated = capture_pointer_constraint_ids(&commands);
    let unrelated = fixture.constraints.lock_pointer(
        &fixture._other_surface,
        &fixture.pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Oneshot,
        &qh,
        (),
    );
    fixture._other_surface.commit();
    fixture.connection.flush().unwrap();
    wait_for_server_commands(&commands);
    fixture.queue.roundtrip(&mut fixture.state).unwrap();
    let unrelated_constraint_id = capture_pointer_constraint_ids(&commands)
        .into_iter()
        .find(|id| !ids_before_unrelated.contains(id))
        .expect("other root's one-shot constraint should be registered");
    let unrelated_before = capture_pointer_constraint_snapshot(&commands, unrelated_constraint_id)
        .expect("other root's constraint should remain registered");
    assert!(unrelated_before.committed);
    assert!(!unrelated_before.defunct);

    let root_surface_id =
        capture_xdg_role_snapshot(&commands, fixture.surface.id().protocol_id()).surface_id;
    let (start_x, start_y) = capture_last_pointer_position(&commands);
    let pointer_before = (start_x, start_y);
    assert!(capture_cursor_hidden_by_pointer_lock(&commands));
    assert!(
        !capture_interaction_cursor_state(&commands).visible,
        "the active lock should hide the client cursor before takeover"
    );
    let root_x_before = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .find(|surface| surface.surface_id == root_surface_id)
        .expect("root should be renderable")
        .origin_x;

    commands
        .send(ServerCommand::BeginMove {
            x: start_x,
            y: start_y,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    let interaction = capture_window_interaction_debug_snapshot(&commands)
        .expect("interaction should begin while the persistent lock is active");
    assert_eq!(interaction.root_surface_id, root_surface_id);
    assert_eq!(interaction.kind, WindowInteractionKind::Move);

    let cursor = capture_interaction_cursor_state(&commands);
    assert!(cursor.override_active);
    assert!(
        cursor.visible,
        "interaction cursor must override the lock-hidden cursor"
    );
    assert_eq!(
        (cursor.pointer_x, cursor.pointer_y),
        pointer_before,
        "takeover must preserve the current logical position instead of applying the hint"
    );
    let requests = capture_pointer_constraint_backend_requests(&commands);
    assert!(requests.iter().any(|request| matches!(
        request,
        PointerConstraintBackendRequest::Deactivate {
            id,
            restore_position: None,
            restore_origin: None,
        } if *id == backend_id
    )));
    assert!(requests.iter().any(|request| matches!(
        request,
        PointerConstraintBackendRequest::ApplyCursorVisibility { visible: true }
    )));
    fixture.queue.roundtrip(&mut fixture.state).unwrap();
    assert_eq!(fixture.state.unlocked_count, 1);

    commands
        .send(ServerCommand::PointerConstraintBackendDeactivated(
            backend_id,
        ))
        .unwrap();
    wait_for_server_commands(&commands);
    let during_interaction_requests = capture_pointer_constraint_backend_requests(&commands);
    assert!(during_interaction_requests.iter().all(|request| !matches!(
        request,
        PointerConstraintBackendRequest::ActivateLocked { .. }
    )));

    commands
        .send(ServerCommand::PointerMotion {
            x: start_x + 35.0,
            y: start_y + 25.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    commands.send(ServerCommand::PresentFrame).unwrap();
    wait_for_server_commands(&commands);
    let root_x_after = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .find(|surface| surface.surface_id == root_surface_id)
        .expect("moved root should remain renderable")
        .origin_x;
    assert_ne!(
        root_x_after, root_x_before,
        "physical pointer motion must move the window"
    );

    commands.send(ServerCommand::EndInteraction).unwrap();
    wait_for_server_commands(&commands);
    let resumed_requests = capture_pointer_constraint_backend_requests(&commands);
    assert!(resumed_requests.iter().any(|request| matches!(
        request,
        PointerConstraintBackendRequest::ActivateLocked { id } if *id == backend_id
    )));
    let unrelated_after = capture_pointer_constraint_snapshot(&commands, unrelated_constraint_id)
        .expect("other root's constraint should remain registered");
    assert!(unrelated_after.committed);
    assert!(
        !unrelated_after.defunct,
        "interaction suspension must be root-scoped"
    );

    commands
        .send(ServerCommand::PointerConstraintBackendActivated(backend_id))
        .unwrap();
    wait_for_server_commands(&commands);
    fixture.queue.roundtrip(&mut fixture.state).unwrap();
    assert_eq!(fixture.state.locked_count, 2);
    assert!(
        capture_pointer_constraint_snapshot(&commands, constraint_id)
            .expect("persistent lock remains registered")
            .active
    );
    let cursor_after_reactivation = capture_interaction_cursor_state(&commands);
    assert!(!cursor_after_reactivation.override_active);
    assert!(capture_cursor_hidden_by_pointer_lock(&commands));
    assert!(
        !cursor_after_reactivation.visible,
        "the reactivated client lock should restore its normal hidden-cursor policy"
    );

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();
    let _ = unrelated;
}

#[test]
fn compositor_move_defers_persistent_confined_pointer_reactivation_until_interaction_ends() {
    let (commands, server_thread, mut fixture) = start_workspace_pointer_constraint_test();
    let qh = fixture.queue.handle();
    let confine = fixture.constraints.confine_pointer(
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
    assert_eq!(fixture.state.confined_count, 1);

    let root_surface_id =
        capture_xdg_role_snapshot(&commands, fixture.surface.id().protocol_id()).surface_id;
    let (start_x, start_y) = capture_last_pointer_position(&commands);
    let root_x_before = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .find(|surface| surface.surface_id == root_surface_id)
        .expect("root should be renderable")
        .origin_x;
    commands
        .send(ServerCommand::BeginMove {
            x: start_x,
            y: start_y,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    assert!(capture_window_interaction_debug_snapshot(&commands).is_some());
    let requests = capture_pointer_constraint_backend_requests(&commands);
    assert!(requests.iter().any(|request| matches!(
        request,
        PointerConstraintBackendRequest::Deactivate { id, .. } if *id == backend_id
    )));
    fixture.queue.roundtrip(&mut fixture.state).unwrap();
    assert_eq!(fixture.state.unconfined_count, 1);

    commands
        .send(ServerCommand::PointerConstraintBackendDeactivated(
            backend_id,
        ))
        .unwrap();
    wait_for_server_commands(&commands);

    let updated_region = fixture.compositor.create_region(&qh, ());
    updated_region.add(0, 0, 160, 120);
    confine.set_region(Some(&updated_region));
    fixture.surface.commit();
    fixture.connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let during_interaction_requests = capture_pointer_constraint_backend_requests(&commands);
    assert!(
        during_interaction_requests.iter().all(|request| !matches!(
            request,
            PointerConstraintBackendRequest::ActivateConfined { .. }
        )),
        "confinement must not reacquire pointer authority during interaction"
    );
    let snapshot = capture_pointer_constraint_snapshot(&commands, constraint_id)
        .expect("persistent confinement remains registered");
    assert!(!snapshot.active);
    assert!(!snapshot.backend_pending);
    assert!(!snapshot.defunct);

    commands
        .send(ServerCommand::PointerMotion {
            x: start_x + 35.0,
            y: start_y + 25.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    commands.send(ServerCommand::PresentFrame).unwrap();
    wait_for_server_commands(&commands);
    let root_x_after = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .find(|surface| surface.surface_id == root_surface_id)
        .expect("moved root should remain renderable")
        .origin_x;
    assert_ne!(
        root_x_after, root_x_before,
        "pointer motion must move the window"
    );

    commands.send(ServerCommand::EndInteraction).unwrap();
    wait_for_server_commands(&commands);
    let resumed_requests = capture_pointer_constraint_backend_requests(&commands);
    let resumed_id = resumed_requests.iter().find_map(|request| match request {
        PointerConstraintBackendRequest::ActivateConfined { id, .. } => Some(*id),
        _ => None,
    });
    assert_eq!(resumed_id, Some(backend_id));
    commands
        .send(ServerCommand::PointerConstraintBackendActivated(backend_id))
        .unwrap();
    wait_for_server_commands(&commands);
    fixture.queue.roundtrip(&mut fixture.state).unwrap();
    assert_eq!(fixture.state.confined_count, 2);
    assert!(
        capture_pointer_constraint_snapshot(&commands, constraint_id)
            .expect("persistent confinement remains registered")
            .active
    );

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();
}

#[test]
fn compositor_move_terminates_active_oneshot_pointer_constraint() {
    let (commands, server_thread, mut fixture) = start_workspace_pointer_constraint_test();
    let qh = fixture.queue.handle();
    let lock = fixture.constraints.lock_pointer(
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
        .expect("one-shot lock should be registered");
    let backend_id = activate_locked_backend(&commands, &mut fixture.state, &mut fixture.queue);

    let (x, y) = capture_last_pointer_position(&commands);
    commands.send(ServerCommand::BeginMove { x, y }).unwrap();
    wait_for_server_commands(&commands);
    assert!(capture_window_interaction_debug_snapshot(&commands).is_some());
    let snapshot = capture_pointer_constraint_snapshot(&commands, constraint_id)
        .expect("one-shot constraint should remain until backend settlement");
    assert!(!snapshot.active);
    assert!(snapshot.defunct);
    assert!(
        capture_pointer_constraint_backend_requests(&commands)
            .iter()
            .any(|request| matches!(
                request,
                PointerConstraintBackendRequest::Deactivate { id, .. } if *id == backend_id
            ))
    );
    fixture.queue.roundtrip(&mut fixture.state).unwrap();
    assert_eq!(fixture.state.unlocked_count, 1);

    commands
        .send(ServerCommand::PointerConstraintBackendDeactivated(
            backend_id,
        ))
        .unwrap();
    wait_for_server_commands(&commands);
    commands.send(ServerCommand::EndInteraction).unwrap();
    wait_for_server_commands(&commands);
    assert!(
        capture_pointer_constraint_backend_requests(&commands)
            .iter()
            .all(|request| !matches!(
                request,
                PointerConstraintBackendRequest::ActivateLocked { .. }
            ))
    );
    assert!(
        capture_pointer_constraint_snapshot(&commands, constraint_id)
            .expect("one-shot constraint should remain terminal")
            .defunct
    );

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();
    let _ = lock;
}

#[test]
fn compositor_move_cancels_pending_persistent_lock_activation_and_allows_later_resume() {
    let (commands, server_thread, mut fixture) = start_workspace_pointer_constraint_test();
    let qh = fixture.queue.handle();
    let lock = fixture.constraints.lock_pointer(
        &fixture.surface,
        &fixture.pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    lock.set_cursor_position_hint(70.0, 50.0);
    fixture.surface.commit();
    fixture.connection.flush().unwrap();
    wait_for_server_commands(&commands);
    fixture.queue.roundtrip(&mut fixture.state).unwrap();
    let constraint_id = capture_pointer_constraint_ids(&commands)
        .into_iter()
        .next()
        .expect("persistent lock should be registered");
    let activation_id = capture_pointer_constraint_backend_requests(&commands)
        .into_iter()
        .find_map(|request| match request {
            PointerConstraintBackendRequest::ActivateLocked { id } => Some(id),
            _ => None,
        })
        .expect("locked activation should be pending");
    assert!(
        capture_pointer_constraint_snapshot(&commands, constraint_id)
            .expect("pending lock remains registered")
            .backend_pending
    );

    let pointer_before = capture_last_pointer_position(&commands);
    commands
        .send(ServerCommand::BeginMove {
            x: pointer_before.0,
            y: pointer_before.1,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    assert!(capture_window_interaction_debug_snapshot(&commands).is_some());
    let snapshot = capture_pointer_constraint_snapshot(&commands, constraint_id)
        .expect("persistent constraint remains registered after takeover");
    assert!(snapshot.committed);
    assert!(!snapshot.active);
    assert!(!snapshot.backend_pending);
    assert!(!snapshot.defunct);
    assert_eq!(capture_last_pointer_position(&commands), pointer_before);
    assert!(
        capture_pointer_constraint_backend_requests(&commands)
            .iter()
            .all(|request| !matches!(
                request,
                PointerConstraintBackendRequest::ActivateLocked { .. }
            ))
    );

    commands
        .send(ServerCommand::PointerConstraintBackendActivated(
            activation_id,
        ))
        .unwrap();
    wait_for_server_commands(&commands);
    let stale_snapshot = capture_pointer_constraint_snapshot(&commands, constraint_id)
        .expect("persistent constraint remains eligible");
    assert!(!stale_snapshot.active);
    assert!(!stale_snapshot.backend_pending);
    assert!(
        capture_pointer_constraint_backend_requests(&commands)
            .iter()
            .all(|request| !matches!(
                request,
                PointerConstraintBackendRequest::ActivateLocked { .. }
            ))
    );

    commands.send(ServerCommand::EndInteraction).unwrap();
    wait_for_server_commands(&commands);
    assert!(
        capture_pointer_constraint_backend_requests(&commands)
            .iter()
            .any(|request| matches!(
                request,
                PointerConstraintBackendRequest::ActivateLocked { id } if *id == activation_id
            ))
    );
    commands
        .send(ServerCommand::PointerConstraintBackendActivated(
            activation_id,
        ))
        .unwrap();
    wait_for_server_commands(&commands);
    assert!(
        capture_pointer_constraint_snapshot(&commands, constraint_id)
            .expect("persistent lock remains registered")
            .active
    );

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();
}

#[test]
fn compositor_move_terminates_active_oneshot_confined_pointer() {
    let (commands, server_thread, mut fixture) = start_workspace_pointer_constraint_test();
    let qh = fixture.queue.handle();
    let confine = fixture.constraints.confine_pointer(
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
        .expect("one-shot confinement should be registered");
    let backend_id = activate_confined_backend(&commands, &mut fixture.state, &mut fixture.queue);

    let (x, y) = capture_last_pointer_position(&commands);
    commands.send(ServerCommand::BeginMove { x, y }).unwrap();
    wait_for_server_commands(&commands);
    assert!(capture_window_interaction_debug_snapshot(&commands).is_some());
    let snapshot = capture_pointer_constraint_snapshot(&commands, constraint_id)
        .expect("one-shot confinement should remain until backend settlement");
    assert!(!snapshot.active);
    assert!(snapshot.defunct);
    assert!(
        capture_pointer_constraint_backend_requests(&commands)
            .iter()
            .any(|request| matches!(
                request,
                PointerConstraintBackendRequest::Deactivate { id, .. } if *id == backend_id
            ))
    );
    fixture.queue.roundtrip(&mut fixture.state).unwrap();
    assert_eq!(fixture.state.unconfined_count, 1);

    commands
        .send(ServerCommand::PointerConstraintBackendDeactivated(
            backend_id,
        ))
        .unwrap();
    wait_for_server_commands(&commands);
    commands.send(ServerCommand::EndInteraction).unwrap();
    wait_for_server_commands(&commands);
    assert!(
        capture_pointer_constraint_backend_requests(&commands)
            .iter()
            .all(|request| !matches!(
                request,
                PointerConstraintBackendRequest::ActivateConfined { .. }
            ))
    );
    assert!(
        capture_pointer_constraint_snapshot(&commands, constraint_id)
            .expect("one-shot confinement should remain terminal")
            .defunct
    );

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();
    let _ = confine;
}

#[test]
fn compositor_move_cancels_pending_oneshot_lock_without_hint_warp() {
    let (commands, server_thread, mut fixture) = start_workspace_pointer_constraint_test();
    let qh = fixture.queue.handle();
    let lock = fixture.constraints.lock_pointer(
        &fixture.surface,
        &fixture.pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Oneshot,
        &qh,
        (),
    );
    lock.set_cursor_position_hint(70.0, 50.0);
    fixture.surface.commit();
    fixture.connection.flush().unwrap();
    wait_for_server_commands(&commands);
    fixture.queue.roundtrip(&mut fixture.state).unwrap();
    let constraint_id = capture_pointer_constraint_ids(&commands)
        .into_iter()
        .next()
        .expect("one-shot lock should be registered");
    let activation_id = capture_pointer_constraint_backend_requests(&commands)
        .into_iter()
        .find_map(|request| match request {
            PointerConstraintBackendRequest::ActivateLocked { id } => Some(id),
            _ => None,
        })
        .expect("one-shot activation should be pending");
    let pointer_before = capture_last_pointer_position(&commands);

    commands
        .send(ServerCommand::BeginMove {
            x: pointer_before.0,
            y: pointer_before.1,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    assert!(capture_window_interaction_debug_snapshot(&commands).is_some());
    assert_eq!(
        capture_last_pointer_position(&commands),
        pointer_before,
        "canceling a pending one-shot lock must not apply its compatibility hint"
    );
    let snapshot = capture_pointer_constraint_snapshot(&commands, constraint_id)
        .expect("one-shot constraint should remain eligible");
    assert!(snapshot.committed);
    assert!(!snapshot.active);
    assert!(!snapshot.backend_pending);
    assert!(!snapshot.defunct);
    fixture.queue.roundtrip(&mut fixture.state).unwrap();
    assert_eq!(fixture.state.locked_count, 0);
    assert_eq!(fixture.state.unlocked_count, 0);
    assert!(
        capture_pointer_constraint_backend_requests(&commands)
            .iter()
            .all(|request| !matches!(
                request,
                PointerConstraintBackendRequest::ActivateLocked { id }
                    | PointerConstraintBackendRequest::Deactivate { id, .. }
                    if *id == activation_id
            ))
    );

    commands
        .send(ServerCommand::PointerConstraintBackendActivated(
            activation_id,
        ))
        .unwrap();
    wait_for_server_commands(&commands);
    fixture.queue.roundtrip(&mut fixture.state).unwrap();
    let stale_snapshot = capture_pointer_constraint_snapshot(&commands, constraint_id)
        .expect("one-shot constraint should remain registered");
    assert!(stale_snapshot.committed);
    assert!(!stale_snapshot.active);
    assert!(!stale_snapshot.backend_pending);
    assert!(!stale_snapshot.defunct);
    assert_eq!(fixture.state.locked_count, 0);
    assert_eq!(fixture.state.unlocked_count, 0);
    assert!(capture_window_interaction_debug_snapshot(&commands).is_some());
    assert!(
        capture_pointer_constraint_backend_requests(&commands)
            .iter()
            .all(|request| !matches!(
                request,
                PointerConstraintBackendRequest::ActivateLocked { .. }
            ))
    );

    commands.send(ServerCommand::EndInteraction).unwrap();
    wait_for_server_commands(&commands);
    let resumed_activation_id = capture_pointer_constraint_backend_requests(&commands)
        .into_iter()
        .find_map(|request| match request {
            PointerConstraintBackendRequest::ActivateLocked { id } => Some(id),
            _ => None,
        })
        .expect("eligible one-shot lock should queue activation after interaction");
    assert_eq!(resumed_activation_id, activation_id);
    assert!(
        capture_pointer_constraint_snapshot(&commands, constraint_id)
            .expect("one-shot constraint remains eligible")
            .backend_pending
    );
    commands
        .send(ServerCommand::PointerConstraintBackendActivated(
            resumed_activation_id,
        ))
        .unwrap();
    wait_for_server_commands(&commands);
    fixture.queue.roundtrip(&mut fixture.state).unwrap();
    assert_eq!(fixture.state.locked_count, 1);
    let activated_snapshot = capture_pointer_constraint_snapshot(&commands, constraint_id)
        .expect("one-shot constraint should be active");
    assert!(activated_snapshot.committed);
    assert!(activated_snapshot.active);
    assert!(!activated_snapshot.backend_pending);
    assert!(!activated_snapshot.defunct);

    let (x, y) = capture_last_pointer_position(&commands);
    commands.send(ServerCommand::BeginMove { x, y }).unwrap();
    wait_for_server_commands(&commands);
    assert!(capture_window_interaction_debug_snapshot(&commands).is_some());
    let deactivation = capture_pointer_constraint_backend_requests(&commands)
        .into_iter()
        .find(|request| {
            matches!(
                request,
                PointerConstraintBackendRequest::Deactivate { id, .. }
                    if *id == resumed_activation_id
            )
        })
        .expect("active one-shot lock should queue backend deactivation");
    assert!(matches!(
        deactivation,
        PointerConstraintBackendRequest::Deactivate {
            restore_position: None,
            restore_origin: None,
            ..
        }
    ));
    let terminal_snapshot = capture_pointer_constraint_snapshot(&commands, constraint_id)
        .expect("one-shot constraint remains registered");
    assert!(!terminal_snapshot.active);
    assert!(!terminal_snapshot.backend_pending);
    assert!(terminal_snapshot.defunct);
    fixture.queue.roundtrip(&mut fixture.state).unwrap();
    assert_eq!(fixture.state.unlocked_count, 1);
    commands
        .send(ServerCommand::PointerConstraintBackendDeactivated(
            resumed_activation_id,
        ))
        .unwrap();
    wait_for_server_commands(&commands);
    commands.send(ServerCommand::EndInteraction).unwrap();
    wait_for_server_commands(&commands);
    assert!(
        capture_pointer_constraint_backend_requests(&commands)
            .iter()
            .all(|request| !matches!(
                request,
                PointerConstraintBackendRequest::ActivateLocked { .. }
            ))
    );
    assert!(
        capture_pointer_constraint_snapshot(&commands, constraint_id)
            .expect("one-shot constraint should remain terminal")
            .defunct
    );

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();
    let _ = lock;
}

#[test]
fn compositor_move_cancels_pending_oneshot_confine_and_allows_later_resume() {
    let (commands, server_thread, mut fixture) = start_workspace_pointer_constraint_test();
    let qh = fixture.queue.handle();
    let confine = fixture.constraints.confine_pointer(
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
        .expect("one-shot confinement should be registered");
    let activation_id = capture_pointer_constraint_backend_requests(&commands)
        .into_iter()
        .find_map(|request| match request {
            PointerConstraintBackendRequest::ActivateConfined { id, .. } => Some(id),
            _ => None,
        })
        .expect("one-shot confinement activation should be pending");
    let pointer_before = capture_last_pointer_position(&commands);

    commands
        .send(ServerCommand::BeginMove {
            x: pointer_before.0,
            y: pointer_before.1,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    assert!(capture_window_interaction_debug_snapshot(&commands).is_some());
    assert_eq!(capture_last_pointer_position(&commands), pointer_before);
    let snapshot = capture_pointer_constraint_snapshot(&commands, constraint_id)
        .expect("one-shot confinement should remain eligible");
    assert!(snapshot.committed);
    assert!(!snapshot.active);
    assert!(!snapshot.backend_pending);
    assert!(!snapshot.defunct);
    fixture.queue.roundtrip(&mut fixture.state).unwrap();
    assert_eq!(fixture.state.confined_count, 0);
    assert_eq!(fixture.state.unconfined_count, 0);
    assert!(
        capture_pointer_constraint_backend_requests(&commands)
            .iter()
            .all(|request| !matches!(
                request,
                PointerConstraintBackendRequest::ActivateConfined { id, .. }
                    | PointerConstraintBackendRequest::Deactivate { id, .. }
                    if *id == activation_id
            ))
    );

    commands
        .send(ServerCommand::PointerConstraintBackendActivated(
            activation_id,
        ))
        .unwrap();
    wait_for_server_commands(&commands);
    fixture.queue.roundtrip(&mut fixture.state).unwrap();
    let stale_snapshot = capture_pointer_constraint_snapshot(&commands, constraint_id)
        .expect("one-shot confinement should remain registered");
    assert!(!stale_snapshot.active);
    assert!(!stale_snapshot.backend_pending);
    assert!(!stale_snapshot.defunct);
    assert_eq!(fixture.state.confined_count, 0);
    assert_eq!(fixture.state.unconfined_count, 0);
    assert!(capture_window_interaction_debug_snapshot(&commands).is_some());
    assert!(
        capture_pointer_constraint_backend_requests(&commands)
            .iter()
            .all(|request| !matches!(
                request,
                PointerConstraintBackendRequest::ActivateConfined { .. }
            ))
    );

    commands.send(ServerCommand::EndInteraction).unwrap();
    wait_for_server_commands(&commands);
    let resumed_activation_id = capture_pointer_constraint_backend_requests(&commands)
        .into_iter()
        .find_map(|request| match request {
            PointerConstraintBackendRequest::ActivateConfined { id, .. } => Some(id),
            _ => None,
        })
        .expect("eligible one-shot confinement should queue activation after interaction");
    assert_eq!(resumed_activation_id, activation_id);
    commands
        .send(ServerCommand::PointerConstraintBackendActivated(
            resumed_activation_id,
        ))
        .unwrap();
    wait_for_server_commands(&commands);
    fixture.queue.roundtrip(&mut fixture.state).unwrap();
    assert_eq!(fixture.state.confined_count, 1);
    let activated_snapshot = capture_pointer_constraint_snapshot(&commands, constraint_id)
        .expect("one-shot confinement should be active");
    assert!(activated_snapshot.active);
    assert!(!activated_snapshot.backend_pending);
    assert!(!activated_snapshot.defunct);

    let (x, y) = capture_last_pointer_position(&commands);
    commands.send(ServerCommand::BeginMove { x, y }).unwrap();
    wait_for_server_commands(&commands);
    let terminal_snapshot = capture_pointer_constraint_snapshot(&commands, constraint_id)
        .expect("one-shot confinement remains registered");
    assert!(!terminal_snapshot.active);
    assert!(!terminal_snapshot.backend_pending);
    assert!(terminal_snapshot.defunct);
    fixture.queue.roundtrip(&mut fixture.state).unwrap();
    assert_eq!(fixture.state.unconfined_count, 1);
    commands
        .send(ServerCommand::PointerConstraintBackendDeactivated(
            resumed_activation_id,
        ))
        .unwrap();
    wait_for_server_commands(&commands);
    commands.send(ServerCommand::EndInteraction).unwrap();
    wait_for_server_commands(&commands);
    assert!(
        capture_pointer_constraint_backend_requests(&commands)
            .iter()
            .all(|request| !matches!(
                request,
                PointerConstraintBackendRequest::ActivateConfined { .. }
            ))
    );

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();
    let _ = confine;
}

#[test]
fn compositor_move_leaves_inactive_oneshot_constraint_on_the_same_root_eligible() {
    let (commands, server_thread, mut fixture) = start_workspace_pointer_constraint_test();
    let qh = fixture.queue.handle();
    let active_lock = fixture.constraints.lock_pointer(
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
    let active_lock_id = capture_pointer_constraint_ids(&commands)
        .into_iter()
        .next()
        .expect("persistent lock should be registered");
    let active_backend_id =
        activate_locked_backend(&commands, &mut fixture.state, &mut fixture.queue);

    let child = fixture.compositor.create_surface(&qh, ());
    let _subsurface = fixture
        .subcompositor
        .get_subsurface(&child, &fixture.surface, &qh, ());
    let inactive_lock = fixture.constraints.lock_pointer(
        &child,
        &fixture.pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Oneshot,
        &qh,
        (),
    );
    child.commit();
    fixture.surface.commit();
    fixture.connection.flush().unwrap();
    wait_for_server_commands(&commands);
    fixture.queue.roundtrip(&mut fixture.state).unwrap();
    let inactive_constraint_id = capture_pointer_constraint_ids(&commands)
        .into_iter()
        .find(|id| *id != active_lock_id)
        .expect("child one-shot constraint should be registered");
    let inactive_before = capture_pointer_constraint_snapshot(&commands, inactive_constraint_id)
        .expect("child constraint should remain registered");
    assert!(inactive_before.committed);
    assert!(!inactive_before.active);
    assert!(!inactive_before.backend_pending);
    assert!(!inactive_before.defunct);

    let (x, y) = capture_last_pointer_position(&commands);
    commands.send(ServerCommand::BeginMove { x, y }).unwrap();
    wait_for_server_commands(&commands);
    assert!(capture_window_interaction_debug_snapshot(&commands).is_some());
    let requests = capture_pointer_constraint_backend_requests(&commands);
    assert!(requests.iter().any(|request| matches!(
        request,
        PointerConstraintBackendRequest::Deactivate { id, .. }
            if *id == active_backend_id
    )));
    assert!(requests.iter().all(|request| !matches!(
        request,
        PointerConstraintBackendRequest::Deactivate { id, .. }
            if id.constraint_id == inactive_constraint_id
    )));
    let inactive_after = capture_pointer_constraint_snapshot(&commands, inactive_constraint_id)
        .expect("inactive child constraint should remain registered");
    assert!(inactive_after.committed);
    assert!(!inactive_after.active);
    assert!(!inactive_after.backend_pending);
    assert!(
        !inactive_after.defunct,
        "interaction takeover must not terminate an inactive one-shot constraint"
    );

    fixture.queue.roundtrip(&mut fixture.state).unwrap();
    assert_eq!(fixture.state.unlocked_count, 1);
    commands
        .send(ServerCommand::PointerConstraintBackendDeactivated(
            active_backend_id,
        ))
        .unwrap();
    wait_for_server_commands(&commands);
    active_lock.destroy();
    fixture.surface.commit();
    fixture.connection.flush().unwrap();
    wait_for_server_commands(&commands);
    fixture.queue.roundtrip(&mut fixture.state).unwrap();

    commands.send(ServerCommand::EndInteraction).unwrap();
    wait_for_server_commands(&commands);
    let resumed_activation_id = capture_pointer_constraint_backend_requests(&commands)
        .into_iter()
        .find_map(|request| match request {
            PointerConstraintBackendRequest::ActivateLocked { id } => Some(id),
            _ => None,
        })
        .expect("inactive one-shot lock should become eligible after interaction ends");
    assert_eq!(resumed_activation_id.constraint_id, inactive_constraint_id);
    commands
        .send(ServerCommand::PointerConstraintBackendActivated(
            resumed_activation_id,
        ))
        .unwrap();
    wait_for_server_commands(&commands);
    fixture.queue.roundtrip(&mut fixture.state).unwrap();
    let resumed_snapshot = capture_pointer_constraint_snapshot(&commands, inactive_constraint_id)
        .expect("inactive one-shot lock should remain registered");
    assert!(resumed_snapshot.active);
    assert!(!resumed_snapshot.backend_pending);
    assert!(!resumed_snapshot.defunct);
    assert_eq!(fixture.state.locked_count, 2);

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();
    let _ = inactive_lock;
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
