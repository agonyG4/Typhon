use super::*;

use std::{os::unix::net::UnixStream, sync::mpsc, time::Duration};

#[derive(Default)]
struct AuthTestState {
    authenticated: bool,
    rejected: bool,
    shortcut_pressed_count: usize,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for AuthTestState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_astrea_shell_auth_manager_v1::AstreaShellAuthManagerV1, ()> for AuthTestState {
    fn event(
        state: &mut Self,
        _proxy: &client_astrea_shell_auth_manager_v1::AstreaShellAuthManagerV1,
        event: client_astrea_shell_auth_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            client_astrea_shell_auth_manager_v1::Event::Authenticated => state.authenticated = true,
            client_astrea_shell_auth_manager_v1::Event::Rejected => state.rejected = true,
        }
    }
}

impl Dispatch<client_astrea_shortcuts_manager_v1::AstreaShortcutsManagerV1, ()> for AuthTestState {
    fn event(
        _state: &mut Self,
        _proxy: &client_astrea_shortcuts_manager_v1::AstreaShortcutsManagerV1,
        _event: client_astrea_shortcuts_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_astrea_shortcut_v1::AstreaShortcutV1, ()> for AuthTestState {
    fn event(
        state: &mut Self,
        _proxy: &client_astrea_shortcut_v1::AstreaShortcutV1,
        event: client_astrea_shortcut_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if matches!(event, client_astrea_shortcut_v1::Event::Pressed { .. }) {
            state.shortcut_pressed_count += 1;
        }
    }
}

#[test]
fn capability_authenticates_exact_wayland_client_for_private_shortcuts() {
    let socket_name = format!("typhon-shell-auth-{}", std::process::id());
    let capability_path = std::env::temp_dir().join(format!(
        ".oblivion-one-test-capability-{}-{}",
        std::process::id(),
        socket_name
    ));
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let capability = std::fs::read_to_string(&capability_path).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let stream = UnixStream::connect(runtime_socket_path(&socket_name)).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<AuthTestState>(&connection).unwrap();
    let qh = queue.handle();
    let auth = globals
        .bind::<client_astrea_shell_auth_manager_v1::AstreaShellAuthManagerV1, _, _>(&qh, 1..=1, ())
        .unwrap();
    auth.authenticate(capability.trim().to_string());
    connection.flush().unwrap();

    let mut state = AuthTestState::default();
    queue.roundtrip(&mut state).unwrap();
    assert!(state.authenticated);
    assert!(!state.rejected);

    let manager: client_astrea_shortcuts_manager_v1::AstreaShortcutsManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let _shortcut = manager.register_shortcut(
        "astrea-shell".to_string(),
        "authenticated_test".to_string(),
        "Authenticated test".to_string(),
        &qh,
        (),
    );
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::EmitAstreaShortcut {
            namespace: "astrea-shell".to_string(),
            name: "authenticated_test".to_string(),
            phase: AstreaShortcutPhase::Pressed,
            timestamp: 7,
            reply,
        })
        .unwrap();
    assert_eq!(receiver.recv_timeout(Duration::from_secs(1)).unwrap(), 1);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.shortcut_pressed_count, 1);

    drop(connection);
    let _ = stop_controllable_test_server(commands, server_thread);
}
