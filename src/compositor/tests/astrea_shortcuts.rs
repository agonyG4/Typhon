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
            timestamp: 42,
            reply,
        })
        .unwrap();
    assert_eq!(receiver.recv_timeout(Duration::from_secs(1)).unwrap(), 0);

    stop_controllable_test_server(commands, server_thread);
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
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();

    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::EmitAstreaShortcut {
            namespace: "astrea-shell".to_string(),
            name: "spotlight_toggle".to_string(),
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
