use super::*;

#[test]
fn releasing_an_entered_output_binding_does_not_leave_the_physical_output() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let output: client_wl_output::WlOutput = globals.bind(&qh, 1..=4, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    assign_test_toplevel(&globals, &qh, &surface).unwrap();

    let mut state = RegistryTestState::default();
    commit_test_buffered_surface_after_initial_configure(
        &surface,
        &shm,
        &qh,
        &connection,
        &mut queue,
        &mut state,
        32,
        32,
    )
    .unwrap();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.surface_enter_count, 1);

    let (reply, snapshot) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureOutputLifecycle {
            surface_id: surface.id().protocol_id(),
            reply,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    let before_release = snapshot.recv().unwrap();
    assert_eq!(before_release.output_binding_ids.len(), 1);
    assert_eq!(before_release.entered_binding_count, 1);
    assert_eq!(before_release.physical_output_ids.len(), 1);

    output.release();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    assert!(connection.roundtrip().is_ok());
    assert_eq!(state.surface_leave_count, 0);
    let (reply, snapshot) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureOutputLifecycle {
            surface_id: surface.id().protocol_id(),
            reply,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    let after_release = snapshot.recv().unwrap();
    assert!(after_release.output_binding_ids.is_empty());
    assert!(
        after_release.physical_output_ids.contains(
            before_release
                .physical_output_ids
                .iter()
                .next()
                .expect("the surface starts on the physical output")
        )
    );
    assert_eq!(
        after_release.logical_output_ids, before_release.logical_output_ids,
        "forgetting a client binding must preserve the logical output"
    );
    assert_eq!(after_release.entered_binding_count, 0);
    assert_eq!(
        after_release.surface_leave_events,
        before_release.surface_leave_events
    );
    assert!(after_release.membership_invariants_valid);

    drop((connection, queue, globals, qh, surface));
    let _ = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn releasing_one_of_two_output_bindings_preserves_the_other_and_rebind_enters_once() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let first_output: client_wl_output::WlOutput = globals.bind(&qh, 1..=4, ()).unwrap();
    let second_output: client_wl_output::WlOutput = globals.bind(&qh, 1..=4, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    assign_test_toplevel(&globals, &qh, &surface).unwrap();

    let mut state = RegistryTestState::default();
    commit_test_buffered_surface_after_initial_configure(
        &surface,
        &shm,
        &qh,
        &connection,
        &mut queue,
        &mut state,
        32,
        32,
    )
    .unwrap();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.surface_enter_count, 2);
    assert_eq!(
        state.surface_enter_output_ids,
        vec![
            first_output.id().protocol_id(),
            second_output.id().protocol_id()
        ]
    );

    let released_binding_id = first_output.id();
    let released_protocol_id = released_binding_id.protocol_id();
    first_output.release();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.surface_leave_count, 0);

    let (reply, snapshot) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureOutputLifecycle {
            surface_id: surface.id().protocol_id(),
            reply,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    let after_first_release = snapshot.recv().unwrap();
    assert_eq!(after_first_release.output_binding_ids.len(), 1);
    assert_eq!(after_first_release.entered_binding_count, 1);
    assert_eq!(after_first_release.physical_output_ids.len(), 1);
    assert!(after_first_release.membership_invariants_valid);

    let enter_count_before_rebind = state.surface_enter_count;
    let replacement_output: client_wl_output::WlOutput = globals.bind(&qh, 1..=4, ()).unwrap();
    let replacement_binding_id = replacement_output.id();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(replacement_binding_id.protocol_id(), released_protocol_id);
    assert_ne!(replacement_binding_id, released_binding_id);
    assert_eq!(state.surface_enter_count, enter_count_before_rebind + 1);
    assert_eq!(state.surface_leave_count, 0);
    assert_eq!(
        state.surface_enter_output_ids.last(),
        Some(&replacement_output.id().protocol_id()),
        "the replacement binding gets its own enter even if the numeric ID was reused"
    );

    let (reply, snapshot) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureOutputLifecycle {
            surface_id: surface.id().protocol_id(),
            reply,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    let snapshot = snapshot.recv().unwrap();
    assert_eq!(snapshot.output_binding_ids.len(), 2);
    assert_eq!(snapshot.entered_binding_count, 2);
    assert_eq!(snapshot.physical_output_ids.len(), 1);
    assert!(snapshot.membership_invariants_valid);
    assert_eq!(snapshot.output_binding_ids.len(), 2);
    assert_eq!(snapshot.entered_binding_count, 2);
    assert_eq!(state.surface_leave_count, 0);

    drop((
        connection,
        queue,
        globals,
        qh,
        surface,
        first_output,
        second_output,
        replacement_output,
    ));
    let _ = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn logical_output_withdrawal_sends_leave_before_retiring_its_binding() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let first_output: client_wl_output::WlOutput = globals.bind(&qh, 1..=4, ()).unwrap();
    let second_output: client_wl_output::WlOutput = globals.bind(&qh, 1..=4, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    assign_test_toplevel(&globals, &qh, &surface).unwrap();

    let mut state = RegistryTestState::default();
    commit_test_buffered_surface_after_initial_configure(
        &surface,
        &shm,
        &qh,
        &connection,
        &mut queue,
        &mut state,
        32,
        32,
    )
    .unwrap();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.surface_enter_count, 2);
    assert_eq!(
        state.surface_enter_output_ids,
        vec![
            first_output.id().protocol_id(),
            second_output.id().protocol_id()
        ]
    );

    commands.send(ServerCommand::WithdrawLogicalOutput).unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.surface_leave_count, 2);
    assert_eq!(
        state.surface_leave_output_ids,
        vec![
            first_output.id().protocol_id(),
            second_output.id().protocol_id()
        ],
        "the leave event must reference the live binding that entered"
    );

    let (reply, snapshot) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureOutputLifecycle {
            surface_id: surface.id().protocol_id(),
            reply,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    let snapshot = snapshot.recv().unwrap();
    assert!(snapshot.output_binding_ids.is_empty());
    assert!(snapshot.logical_output_ids.is_empty());
    assert!(snapshot.physical_output_ids.is_empty());
    assert_eq!(snapshot.entered_binding_count, 0);
    assert_eq!(snapshot.surface_leave_events, 2);
    assert!(snapshot.membership_invariants_valid);

    drop((
        connection,
        queue,
        globals,
        qh,
        surface,
        first_output,
        second_output,
    ));
    let _ = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn client_disconnect_forgets_output_bindings_without_withdrawing_the_output() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let output: client_wl_output::WlOutput = globals.bind(&qh, 1..=4, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    assign_test_toplevel(&globals, &qh, &surface).unwrap();

    let mut state = RegistryTestState::default();
    commit_test_buffered_surface_after_initial_configure(
        &surface,
        &shm,
        &qh,
        &connection,
        &mut queue,
        &mut state,
        32,
        32,
    )
    .unwrap();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.surface_enter_count, 1);
    let leave_count_before_disconnect = state.surface_leave_count;
    let surface_id = surface.id().protocol_id();
    let _entered_output_id = output.id().protocol_id();

    drop((connection, queue, globals, qh, surface, output));
    wait_for_server_commands(&commands);

    let (reply, snapshot) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureOutputLifecycle { surface_id, reply })
        .unwrap();
    wait_for_server_commands(&commands);
    let snapshot = snapshot.recv().unwrap();
    assert!(snapshot.output_binding_ids.is_empty());
    assert_eq!(snapshot.logical_output_ids.len(), 1);
    assert_eq!(
        snapshot.surface_leave_events,
        leave_count_before_disconnect as u64
    );
    assert!(snapshot.membership_invariants_valid);

    let _ = stop_controllable_test_server(commands, server_thread);
}
