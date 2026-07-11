use super::*;

#[test]
fn astrea_shortcuts_protocol_dispatches_registered_pressed_event() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    commands
        .send(ServerCommand::AuthorizeAstreaShellPid(std::process::id()))
        .unwrap();
    wait_for_server_commands(&commands);

    let stream = UnixStream::connect(socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let manager: client_astrea_shortcuts_manager_v1::AstreaShortcutsManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let _shortcut = manager.register_shortcut(
        "astrea-shell".to_string(),
        "spotlight_toggle".to_string(),
        "Toggle Spotlight".to_string(),
        &qh,
        (),
    );
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();

    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::EmitAstreaShortcut {
            namespace: "astrea-shell".to_string(),
            name: "spotlight_toggle".to_string(),
            phase: AstreaShortcutPhase::Pressed,
            timestamp: 42,
            reply,
        })
        .unwrap();
    assert_eq!(receiver.recv_timeout(Duration::from_secs(1)).unwrap(), 1);

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    stop_controllable_test_server(commands, server_thread);

    assert_eq!(state.astrea_shortcut_pressed_count, 1);
    assert_eq!(state.astrea_shortcut_pressed_timestamps, vec![42]);
    assert_eq!(state.astrea_shortcut_pressed_serials.len(), 1);
}

#[test]
fn astrea_shortcuts_protocol_dispatches_each_lifecycle_phase() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    commands
        .send(ServerCommand::AuthorizeAstreaShellPid(std::process::id()))
        .unwrap();
    wait_for_server_commands(&commands);

    let stream = UnixStream::connect(socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let manager: client_astrea_shortcuts_manager_v1::AstreaShortcutsManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let _shortcut = manager.register_shortcut(
        "astrea-shell".to_string(),
        "phase_test".to_string(),
        "Phase test".to_string(),
        &qh,
        (),
    );
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();

    for (phase, timestamp) in [
        (AstreaShortcutPhase::Pressed, 10),
        (AstreaShortcutPhase::Repeated, 11),
        (AstreaShortcutPhase::Released, 12),
    ] {
        let (reply, receiver) = mpsc::channel();
        commands
            .send(ServerCommand::EmitAstreaShortcut {
                namespace: "astrea-shell".to_string(),
                name: "phase_test".to_string(),
                phase,
                timestamp,
                reply,
            })
            .unwrap();
        assert_eq!(receiver.recv_timeout(Duration::from_secs(1)).unwrap(), 1);
    }

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    stop_controllable_test_server(commands, server_thread);

    assert!(matches!(
        state.astrea_shortcut_events.as_slice(),
        [
            AstreaShortcutEventRecord::Pressed { timestamp: 10, .. },
            AstreaShortcutEventRecord::Repeated { timestamp: 11, .. },
            AstreaShortcutEventRecord::Released { timestamp: 12, .. },
        ]
    ));
    assert!(
        state
            .astrea_shortcut_events
            .iter()
            .all(|event| match event {
                AstreaShortcutEventRecord::Pressed { serial, .. }
                | AstreaShortcutEventRecord::Repeated { serial, .. }
                | AstreaShortcutEventRecord::Released { serial, .. }
                | AstreaShortcutEventRecord::Cancelled { serial } => *serial != 0,
            })
    );
}

#[test]
fn astrea_shortcuts_destroyed_registration_is_not_dispatched() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    commands
        .send(ServerCommand::AuthorizeAstreaShellPid(std::process::id()))
        .unwrap();
    wait_for_server_commands(&commands);

    let stream = UnixStream::connect(socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let manager: client_astrea_shortcuts_manager_v1::AstreaShortcutsManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let shortcut = manager.register_shortcut(
        "astrea-shell".to_string(),
        "spotlight_toggle".to_string(),
        "Toggle Spotlight".to_string(),
        &qh,
        (),
    );
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();

    shortcut.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();

    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::EmitAstreaShortcut {
            namespace: "astrea-shell".to_string(),
            name: "spotlight_toggle".to_string(),
            phase: AstreaShortcutPhase::Pressed,
            timestamp: 42,
            reply,
        })
        .unwrap();
    assert_eq!(receiver.recv_timeout(Duration::from_secs(1)).unwrap(), 0);

    stop_controllable_test_server(commands, server_thread);
}

#[test]
fn astrea_shell_duplicate_registration_cancels_old_owner_and_dispatches_once() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    commands
        .send(ServerCommand::AuthorizeAstreaShellPid(std::process::id()))
        .unwrap();
    wait_for_server_commands(&commands);

    let stream = UnixStream::connect(socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let manager: client_astrea_shortcuts_manager_v1::AstreaShortcutsManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let _old_owner = manager.register_shortcut(
        "astrea-shell".to_string(),
        "spotlight_toggle".to_string(),
        "Old Spotlight".to_string(),
        &qh,
        (),
    );
    let _new_owner = manager.register_shortcut(
        "astrea-shell".to_string(),
        "spotlight_toggle".to_string(),
        "New Spotlight".to_string(),
        &qh,
        (),
    );
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();

    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::EmitAstreaShortcut {
            namespace: "astrea-shell".to_string(),
            name: "spotlight_toggle".to_string(),
            phase: AstreaShortcutPhase::Pressed,
            timestamp: 42,
            reply,
        })
        .unwrap();
    assert_eq!(receiver.recv_timeout(Duration::from_secs(1)).unwrap(), 1);

    queue.roundtrip(&mut state).unwrap();

    drop(queue);
    drop(connection);
    wait_for_server_commands(&commands);

    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::EmitAstreaShortcut {
            namespace: "astrea-shell".to_string(),
            name: "spotlight_toggle".to_string(),
            phase: AstreaShortcutPhase::Pressed,
            timestamp: 43,
            reply,
        })
        .unwrap();
    assert_eq!(receiver.recv_timeout(Duration::from_secs(1)).unwrap(), 0);

    stop_controllable_test_server(commands, server_thread);

    assert_eq!(state.astrea_shortcut_cancelled_count, 1);
    assert_eq!(state.astrea_shortcut_cancelled_serials.len(), 1);
    assert_ne!(state.astrea_shortcut_cancelled_serials[0], 0);
    assert_eq!(state.astrea_shortcut_pressed_count, 1);
    assert_eq!(state.astrea_shortcut_pressed_timestamps, vec![42]);
}

#[test]
fn astrea_shell_unauthorized_duplicate_does_not_evict_owner() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    commands
        .send(ServerCommand::AuthorizeAstreaShellPid(std::process::id()))
        .unwrap();
    wait_for_server_commands(&commands);

    let stream = UnixStream::connect(socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let manager: client_astrea_shortcuts_manager_v1::AstreaShortcutsManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let _authorized_owner = manager.register_shortcut(
        "astrea-shell".to_string(),
        "spotlight_toggle".to_string(),
        "Authorized Spotlight".to_string(),
        &qh,
        (),
    );
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();

    commands
        .send(ServerCommand::ClearAstreaShellAuthorization)
        .unwrap();
    wait_for_server_commands(&commands);

    let _unauthorized_duplicate = manager.register_shortcut(
        "astrea-shell".to_string(),
        "spotlight_toggle".to_string(),
        "Unauthorized Spotlight".to_string(),
        &qh,
        (),
    );
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.astrea_shortcut_cancelled_count, 1);
    assert_ne!(state.astrea_shortcut_cancelled_serials[0], 0);

    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::EmitAstreaShortcut {
            namespace: "astrea-shell".to_string(),
            name: "spotlight_toggle".to_string(),
            phase: AstreaShortcutPhase::Pressed,
            timestamp: 44,
            reply,
        })
        .unwrap();
    assert_eq!(receiver.recv_timeout(Duration::from_secs(1)).unwrap(), 1);
    queue.roundtrip(&mut state).unwrap();

    stop_controllable_test_server(commands, server_thread);

    assert_eq!(state.astrea_shortcut_pressed_count, 1);
    assert_eq!(state.astrea_shortcut_pressed_timestamps, vec![44]);
}

#[test]
fn astrea_shell_disconnect_removes_owner_and_reconnect_can_register_again() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    commands
        .send(ServerCommand::AuthorizeAstreaShellPid(std::process::id()))
        .unwrap();
    wait_for_server_commands(&commands);

    {
        let stream = UnixStream::connect(&socket_path).unwrap();
        let connection = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
        let qh = queue.handle();
        let manager: client_astrea_shortcuts_manager_v1::AstreaShortcutsManagerV1 =
            globals.bind(&qh, 1..=1, ()).unwrap();
        let _shortcut = manager.register_shortcut(
            "astrea-shell".to_string(),
            "spotlight_toggle".to_string(),
            "Toggle Spotlight".to_string(),
            &qh,
            (),
        );
        connection.flush().unwrap();
        queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    }
    wait_for_server_commands(&commands);

    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::EmitAstreaShortcut {
            namespace: "astrea-shell".to_string(),
            name: "spotlight_toggle".to_string(),
            phase: AstreaShortcutPhase::Pressed,
            timestamp: 42,
            reply,
        })
        .unwrap();
    assert_eq!(receiver.recv_timeout(Duration::from_secs(1)).unwrap(), 0);

    let stream = UnixStream::connect(socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let manager: client_astrea_shortcuts_manager_v1::AstreaShortcutsManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let _shortcut = manager.register_shortcut(
        "astrea-shell".to_string(),
        "spotlight_toggle".to_string(),
        "Toggle Spotlight".to_string(),
        &qh,
        (),
    );
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();

    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::EmitAstreaShortcut {
            namespace: "astrea-shell".to_string(),
            name: "spotlight_toggle".to_string(),
            phase: AstreaShortcutPhase::Pressed,
            timestamp: 43,
            reply,
        })
        .unwrap();
    assert_eq!(receiver.recv_timeout(Duration::from_secs(1)).unwrap(), 1);

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    stop_controllable_test_server(commands, server_thread);

    assert_eq!(state.astrea_shortcut_pressed_count, 1);
    assert_eq!(state.astrea_shortcut_pressed_timestamps, vec![43]);
}

#[test]
fn astrea_shell_ownership_is_independent_per_shortcut_name() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    commands
        .send(ServerCommand::AuthorizeAstreaShellPid(std::process::id()))
        .unwrap();
    wait_for_server_commands(&commands);

    let first_stream = UnixStream::connect(&socket_path).unwrap();
    let first_connection = Connection::from_socket(first_stream).unwrap();
    let (first_globals, mut first_queue) =
        registry_queue_init::<RegistryTestState>(&first_connection).unwrap();
    let first_qh = first_queue.handle();
    let first_manager: client_astrea_shortcuts_manager_v1::AstreaShortcutsManagerV1 =
        first_globals.bind(&first_qh, 1..=1, ()).unwrap();
    let _spotlight = first_manager.register_shortcut(
        "astrea-shell".to_string(),
        "spotlight_toggle".to_string(),
        "Spotlight".to_string(),
        &first_qh,
        (),
    );
    let _alt_tab = first_manager.register_shortcut(
        "astrea-shell".to_string(),
        "alt_tab_next".to_string(),
        "Alt-Tab".to_string(),
        &first_qh,
        (),
    );
    first_connection.flush().unwrap();
    first_queue
        .roundtrip(&mut RegistryTestState::default())
        .unwrap();

    let second_stream = UnixStream::connect(socket_path).unwrap();
    let second_connection = Connection::from_socket(second_stream).unwrap();
    let (second_globals, mut second_queue) =
        registry_queue_init::<RegistryTestState>(&second_connection).unwrap();
    let second_qh = second_queue.handle();
    let second_manager: client_astrea_shortcuts_manager_v1::AstreaShortcutsManagerV1 =
        second_globals.bind(&second_qh, 1..=1, ()).unwrap();
    let _new_spotlight = second_manager.register_shortcut(
        "astrea-shell".to_string(),
        "spotlight_toggle".to_string(),
        "New Spotlight".to_string(),
        &second_qh,
        (),
    );
    second_connection.flush().unwrap();
    let mut second_state = RegistryTestState::default();
    second_queue.roundtrip(&mut second_state).unwrap();

    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::EmitAstreaShortcut {
            namespace: "astrea-shell".to_string(),
            name: "spotlight_toggle".to_string(),
            phase: AstreaShortcutPhase::Pressed,
            timestamp: 50,
            reply,
        })
        .unwrap();
    assert_eq!(receiver.recv_timeout(Duration::from_secs(1)).unwrap(), 1);
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::EmitAstreaShortcut {
            namespace: "astrea-shell".to_string(),
            name: "alt_tab_next".to_string(),
            phase: AstreaShortcutPhase::Pressed,
            timestamp: 51,
            reply,
        })
        .unwrap();
    assert_eq!(receiver.recv_timeout(Duration::from_secs(1)).unwrap(), 1);

    let mut first_state = RegistryTestState::default();
    first_queue.roundtrip(&mut first_state).unwrap();
    second_queue.roundtrip(&mut second_state).unwrap();
    stop_controllable_test_server(commands, server_thread);

    assert_eq!(first_state.astrea_shortcut_cancelled_count, 1);
    assert_eq!(second_state.astrea_shortcut_pressed_timestamps, vec![50]);
    assert_eq!(first_state.astrea_shortcut_pressed_timestamps, vec![51]);
}

#[test]
fn astrea_shortcuts_nonmatching_registration_is_not_dispatched() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    commands
        .send(ServerCommand::AuthorizeAstreaShellPid(std::process::id()))
        .unwrap();
    wait_for_server_commands(&commands);

    let stream = UnixStream::connect(socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let manager: client_astrea_shortcuts_manager_v1::AstreaShortcutsManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let _shortcut = manager.register_shortcut(
        "astrea-shell".to_string(),
        "alt_tab_next".to_string(),
        "AltTab Next".to_string(),
        &qh,
        (),
    );
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();

    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::EmitAstreaShortcut {
            namespace: "astrea-shell".to_string(),
            name: "spotlight_toggle".to_string(),
            phase: AstreaShortcutPhase::Pressed,
            timestamp: 42,
            reply,
        })
        .unwrap();
    assert_eq!(receiver.recv_timeout(Duration::from_secs(1)).unwrap(), 0);

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    stop_controllable_test_server(commands, server_thread);

    assert_eq!(state.astrea_shortcut_pressed_count, 0);
}

#[test]
fn astrea_shell_shortcuts_require_authorized_client_pid() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let stream = UnixStream::connect(socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let manager: client_astrea_shortcuts_manager_v1::AstreaShortcutsManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let _shortcut = manager.register_shortcut(
        "astrea-shell".to_string(),
        "spotlight_toggle".to_string(),
        "Toggle Spotlight".to_string(),
        &qh,
        (),
    );
    connection.flush().unwrap();
    let mut registration_state = RegistryTestState::default();
    queue.roundtrip(&mut registration_state).unwrap();
    assert_eq!(registration_state.astrea_shortcut_cancelled_count, 1);
    assert_ne!(registration_state.astrea_shortcut_cancelled_serials[0], 0);

    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::EmitAstreaShortcut {
            namespace: "astrea-shell".to_string(),
            name: "spotlight_toggle".to_string(),
            phase: AstreaShortcutPhase::Pressed,
            timestamp: 42,
            reply,
        })
        .unwrap();
    assert_eq!(receiver.recv_timeout(Duration::from_secs(1)).unwrap(), 0);

    stop_controllable_test_server(commands, server_thread);
}

#[test]
fn astrea_shell_shortcuts_allow_authorized_shell_descendant_pid() {
    let mut authorized = HashSet::new();
    authorized.insert(10);

    assert!(astrea_shell_pid_is_authorized_with_lookup(
        30,
        &authorized,
        |pid| match pid {
            30 => Some(20),
            20 => Some(10),
            _ => None,
        },
    ));

    assert!(!astrea_shell_pid_is_authorized_with_lookup(
        30,
        &authorized,
        |pid| match pid {
            30 => Some(20),
            20 => Some(1),
            _ => None,
        },
    ));

    assert!(!astrea_shell_pid_is_authorized_with_lookup(
        30,
        &authorized,
        Some,
    ));
}

#[test]
fn astrea_shell_shortcuts_allow_authorized_shell_uid_when_process_is_reparented() {
    let authorized_pids = HashSet::new();
    let authorized_uids = HashSet::from([1000]);

    assert!(astrea_shell_identity_is_authorized_with_lookup(
        30,
        1000,
        &authorized_pids,
        &authorized_uids,
        |_| None,
    ));
    assert!(!astrea_shell_identity_is_authorized_with_lookup(
        30,
        1001,
        &authorized_pids,
        &authorized_uids,
        |_| None,
    ));
}
