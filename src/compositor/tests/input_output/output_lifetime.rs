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

    let advertised_outputs = || {
        globals.contents().with_list(|list| {
            list.iter()
                .filter(|global| global.interface == "wl_output")
                .count()
        })
    };
    assert_eq!(advertised_outputs(), 1);

    commands.send(ServerCommand::WithdrawLogicalOutput).unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(
        advertised_outputs(),
        0,
        "withdrawal must remove the wl_output global from the registry"
    );
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

#[test]
fn surfaces_enter_and_leave_additional_outputs_by_layout_intersection() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let native_output: client_wl_output::WlOutput = globals.bind(&qh, 1..=4, ()).unwrap();
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

    let add_output = |name, position| {
        let (reply, output_id) = mpsc::channel();
        commands
            .send(ServerCommand::AddLogicalOutput {
                name,
                position,
                size: (1280, 800),
                reply,
            })
            .unwrap();
        wait_for_server_commands(&commands);
        output_id.recv().unwrap().expect("additional output id")
    };
    let bind_newest_output = |queue: &mut EventQueue<RegistryTestState>,
                              state: &mut RegistryTestState| {
        queue.roundtrip(state).unwrap();
        let name = globals.contents().with_list(|list| {
            list.iter()
                .filter(|global| global.interface == "wl_output")
                .map(|global| global.name)
                .max()
                .unwrap()
        });
        let output: client_wl_output::WlOutput = globals.registry().bind(name, 4, &qh, ());
        queue.roundtrip(state).unwrap();
        output
    };

    // An output to the right of the native one: the surface does not overlap it.
    let right_id = add_output("DP-2", (1280, 0));
    let right_output = bind_newest_output(&mut queue, &mut state);
    assert_eq!(state.surface_enter_count, 1);

    // An output mirroring the native area: the surface overlaps it as well.
    let mirror_id = add_output("HDMI-A-1", (0, 0));
    let mirror_output = bind_newest_output(&mut queue, &mut state);
    assert_eq!(state.surface_enter_count, 2);
    assert_eq!(
        state.surface_enter_output_ids,
        vec![
            native_output.id().protocol_id(),
            mirror_output.id().protocol_id()
        ]
    );
    assert_eq!(
        globals.contents().with_list(|list| list
            .iter()
            .filter(|global| global.interface == "wl_output")
            .count()),
        3
    );

    commands
        .send(ServerCommand::WithdrawLogicalOutputId(mirror_id))
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.surface_leave_count, 1);
    assert_eq!(
        state.surface_leave_output_ids,
        vec![mirror_output.id().protocol_id()]
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
    assert_eq!(snapshot.logical_output_ids.len(), 2);
    assert!(snapshot.logical_output_ids.contains(&right_id));
    assert!(!snapshot.physical_output_ids.contains(&right_id));
    assert_eq!(snapshot.physical_output_ids.len(), 1);
    assert_eq!(snapshot.entered_binding_count, 1);
    assert!(snapshot.membership_invariants_valid);

    drop((
        connection,
        queue,
        globals,
        qh,
        surface,
        native_output,
        right_output,
        mirror_output,
    ));
    let _ = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn each_output_shows_its_own_workspace_and_activation_follows_ownership() {
    use crate::wm::{WorkspaceId, WorkspaceSwitchOutcome};
    let workspace = |raw| WorkspaceId::new(raw).unwrap();
    let mut server = OwnCompositorServer::bind(&unique_socket_name()).unwrap();
    let native = server.focused_output().expect("native output is attached");
    assert_eq!(server.output_workspace(native), Some(workspace(1)));

    let right = server
        .add_logical_output("DP-2", (1280, 0), (1920, 1080), 1.0, 60)
        .unwrap();
    assert_eq!(server.output_workspace(right), Some(workspace(2)));
    assert_eq!(server.focused_output(), Some(native));
    assert_eq!(server.active_workspace(), workspace(1));

    // Workspace 2 is already on screen: only output focus moves.
    assert_eq!(
        server.activate_workspace(workspace(2)),
        WorkspaceSwitchOutcome::FocusedOutput {
            previous: workspace(1),
            current: workspace(2),
        }
    );
    assert_eq!(server.focused_output(), Some(right));
    assert_eq!(server.output_workspace(native), Some(workspace(1)));

    // Workspace 3 belongs to the native output, so it switches there.
    assert_eq!(
        server.activate_workspace(workspace(3)),
        WorkspaceSwitchOutcome::Changed {
            previous: workspace(1),
            current: workspace(3),
        }
    );
    assert_eq!(server.focused_output(), Some(native));
    assert_eq!(server.output_workspace(right), Some(workspace(2)));

    server.withdraw_logical_output(right);
    assert_eq!(server.output_workspace(right), None);
    assert_eq!(server.focused_output(), Some(native));
    assert_eq!(server.active_workspace(), workspace(3));
    // Workspace 2 now belongs to the native output and switches it.
    assert!(server.activate_workspace(workspace(2)).is_changed());
    assert_eq!(server.output_workspace(native), Some(workspace(2)));
}
