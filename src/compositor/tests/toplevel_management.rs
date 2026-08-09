use super::super::*;
use super::support::frame_buffer_client::create_test_buffered_toplevel;
use super::support::locked_relative::{runtime_socket_path, unique_socket_name};
use super::support::registry_state::RegistryTestState;
use super::support::server_runtime::{
    ServerCommand, create_test_shm_file, spawn_controllable_test_server, spawn_test_server,
    stop_controllable_test_server, stop_test_server,
};
use super::support::window_ops::create_buffered_toplevel_then_window_commands;
use crate::astrea_shell_auth::client::astrea_shell_auth_manager_v1 as client_astrea_shell_auth_manager_v1;
use crate::astrea_toplevel_management::client::{
    astrea_toplevel_manager_v1 as client_astrea_toplevel_manager_v1,
    astrea_toplevel_v1 as client_astrea_toplevel_v1,
};
use crate::xwayland::xwm::{
    X11Geometry, X11PublishedState, X11WindowSnapshot, X11WindowTypes, XwmCommand, XwmEvent,
};
use crate::xwayland::{X11WindowHandle, XwaylandAssociationEvent, XwaylandGeneration};
use std::collections::HashMap;
use std::os::{fd::AsFd, unix::net::UnixStream};
use std::sync::mpsc;
use wayland_client::backend::ObjectId;
use wayland_client::globals::GlobalListContents;
use wayland_client::protocol::wl_registry;
use wayland_client::protocol::{wl_compositor as client_wl_compositor, wl_shm as client_wl_shm};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, globals::registry_queue_init};
use wayland_protocols::xdg::shell::client::xdg_wm_base as client_xdg_wm_base;
use wayland_protocols::xwayland::shell::v1::client::xwayland_shell_v1 as client_xwayland_shell_v1;

#[derive(Debug)]
struct ClientToplevel {
    proxy_id: ObjectId,
    identifier: Option<String>,
    app_id: Option<String>,
    title: Option<String>,
    pid: Option<u32>,
    kind: Option<u32>,
    state: Option<u32>,
    focus_serial: Option<u64>,
    last_revision: Option<u64>,
    closed: bool,
    events_after_closed: usize,
}

#[derive(Debug, Default)]
struct ToplevelClientState {
    events: Vec<&'static str>,
    manager_dones: Vec<(u32, u32, u32, u32)>,
    action_dones: Vec<(u32, u32, u32, u32)>,
    authenticated: bool,
    authentication_rejected: bool,
    handles: Vec<client_astrea_toplevel_v1::AstreaToplevelV1>,
    toplevels: HashMap<ObjectId, ClientToplevel>,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for ToplevelClientState {
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

impl Dispatch<client_astrea_shell_auth_manager_v1::AstreaShellAuthManagerV1, ()>
    for ToplevelClientState
{
    fn event(
        state: &mut Self,
        _proxy: &client_astrea_shell_auth_manager_v1::AstreaShellAuthManagerV1,
        event: client_astrea_shell_auth_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            client_astrea_shell_auth_manager_v1::Event::Authenticated => {
                state.authenticated = true;
            }
            client_astrea_shell_auth_manager_v1::Event::Rejected => {
                state.authentication_rejected = true;
            }
        }
    }
}

impl Dispatch<client_astrea_toplevel_manager_v1::AstreaToplevelManagerV1, ()>
    for ToplevelClientState
{
    fn event(
        state: &mut Self,
        _proxy: &client_astrea_toplevel_manager_v1::AstreaToplevelManagerV1,
        event: client_astrea_toplevel_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            client_astrea_toplevel_manager_v1::Event::Toplevel { id } => {
                state.events.push("toplevel");
                state.toplevels.insert(
                    id.id(),
                    ClientToplevel {
                        proxy_id: id.id(),
                        identifier: None,
                        app_id: None,
                        title: None,
                        pid: None,
                        kind: None,
                        state: None,
                        focus_serial: None,
                        last_revision: None,
                        closed: false,
                        events_after_closed: 0,
                    },
                );
                state.handles.push(id);
            }
            client_astrea_toplevel_manager_v1::Event::Done {
                revision_hi,
                revision_lo,
                total,
                flags,
            } => {
                state.events.push("manager_done");
                state
                    .manager_dones
                    .push((revision_hi, revision_lo, total, flags.into()));
            }
            client_astrea_toplevel_manager_v1::Event::Failed { .. } => {
                state.events.push("manager_failed");
            }
            client_astrea_toplevel_manager_v1::Event::ActionDone {
                token_hi,
                token_lo,
                action,
                result,
            } => {
                state.events.push("action_done");
                state
                    .action_dones
                    .push((token_hi, token_lo, action.into(), result.into()));
            }
        }
    }

    wayland_client::event_created_child!(
        ToplevelClientState,
        client_astrea_toplevel_manager_v1::AstreaToplevelManagerV1,
        [0 => (client_astrea_toplevel_v1::AstreaToplevelV1, ())]
    );
}

impl Dispatch<client_astrea_toplevel_v1::AstreaToplevelV1, ()> for ToplevelClientState {
    fn event(
        state: &mut Self,
        proxy: &client_astrea_toplevel_v1::AstreaToplevelV1,
        event: client_astrea_toplevel_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let handle = state
            .toplevels
            .get_mut(&proxy.id())
            .expect("handle must be announced before handle events");
        if handle.closed {
            handle.events_after_closed = handle.events_after_closed.saturating_add(1);
        }
        let name = match event {
            client_astrea_toplevel_v1::Event::Identifier { identifier } => {
                handle.identifier = Some(identifier);
                "identifier"
            }
            client_astrea_toplevel_v1::Event::AppId { app_id } => {
                handle.app_id = Some(app_id);
                "app_id"
            }
            client_astrea_toplevel_v1::Event::Title { title } => {
                handle.title = Some(title);
                "title"
            }
            client_astrea_toplevel_v1::Event::Pid { pid } => {
                handle.pid = Some(pid);
                "pid"
            }
            client_astrea_toplevel_v1::Event::Kind { kind } => {
                handle.kind = Some(kind.into());
                "kind"
            }
            client_astrea_toplevel_v1::Event::State { state: value } => {
                handle.state = Some(value.into());
                "state"
            }
            client_astrea_toplevel_v1::Event::FocusSerial {
                serial_hi,
                serial_lo,
            } => {
                handle.focus_serial = Some(crate::compositor::toplevel_publication::join_u64(
                    serial_hi, serial_lo,
                ));
                "focus_serial"
            }
            client_astrea_toplevel_v1::Event::Done {
                revision_hi,
                revision_lo,
            } => {
                handle.last_revision = Some(crate::compositor::toplevel_publication::join_u64(
                    revision_hi,
                    revision_lo,
                ));
                "handle_done"
            }
            client_astrea_toplevel_v1::Event::Closed => {
                handle.closed = true;
                "closed"
            }
        };
        state.events.push(name);
    }
}

#[test]
fn authorized_manager_receives_explicit_empty_initial_done() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    server.authorize_astrea_shell_pid(std::process::id());
    let (running, server_thread) = spawn_test_server(server);
    let socket_path = runtime_socket_path(&socket_name);
    let connection = Connection::from_socket(UnixStream::connect(socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<ToplevelClientState>(&connection).unwrap();
    let qh = queue.handle();
    let _manager: client_astrea_toplevel_manager_v1::AstreaToplevelManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    connection.flush().unwrap();
    let mut state = ToplevelClientState::default();
    queue.roundtrip(&mut state).unwrap();

    assert_eq!(state.events, ["manager_done"]);
    assert_eq!(state.manager_dones, [(0, 0, 0, 0)]);

    let _ = stop_test_server(running, server_thread);
}

#[test]
fn one_hundred_empty_manager_lifecycles_release_publication_ownership() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    server.authorize_astrea_shell_pid(std::process::id());
    let (running, server_thread) = spawn_test_server(server);
    let socket_path = runtime_socket_path(&socket_name);
    let connection = Connection::from_socket(UnixStream::connect(socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<ToplevelClientState>(&connection).unwrap();
    let qh = queue.handle();
    let mut state = ToplevelClientState::default();

    for _ in 0..100 {
        let manager: client_astrea_toplevel_manager_v1::AstreaToplevelManagerV1 =
            globals.bind(&qh, 1..=1, ()).unwrap();
        connection.flush().unwrap();
        queue.roundtrip(&mut state).unwrap();
        manager.destroy();
        connection.flush().unwrap();
        queue.roundtrip(&mut state).unwrap();
    }

    assert!(state.handles.is_empty());
    let _ = stop_test_server(running, server_thread);
}

#[test]
fn one_hundred_manager_lifecycles_retain_closed_handles_until_destroyed() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    server.authorize_astrea_shell_pid(std::process::id());
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let socket_path = runtime_socket_path(&socket_name);
    let _window_client_state = create_buffered_toplevel_then_window_commands(
        &socket_path,
        &commands,
        &[] as &[ServerCommand],
    )
    .unwrap();
    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<ToplevelClientState>(&connection).unwrap();
    let qh = queue.handle();
    let mut state = ToplevelClientState::default();

    for expected in 1..=100 {
        let manager: client_astrea_toplevel_manager_v1::AstreaToplevelManagerV1 =
            globals.bind(&qh, 1..=1, ()).unwrap();
        connection.flush().unwrap();
        queue.roundtrip(&mut state).unwrap();
        assert_eq!(state.handles.len(), expected);
        manager.destroy();
        connection.flush().unwrap();
        queue.roundtrip(&mut state).unwrap();
        assert!(state.toplevels.values().all(|handle| handle.closed));
    }

    for handle in &state.handles {
        handle.destroy();
    }
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert!(
        state
            .toplevels
            .values()
            .all(|handle| handle.events_after_closed == 0)
    );

    let returned = stop_controllable_test_server(commands, server_thread);
    assert_eq!(
        returned
            .state
            .astrea_toplevel_publisher
            .metrics
            .retired_handles,
        0
    );
    assert_eq!(
        returned
            .state
            .astrea_toplevel_publisher
            .metrics
            .active_handles,
        0
    );
}

#[test]
fn same_uid_client_without_supervised_identity_is_rejected() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let (running, server_thread) = spawn_test_server(server);
    let socket_path = runtime_socket_path(&socket_name);
    let connection = Connection::from_socket(UnixStream::connect(socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<ToplevelClientState>(&connection).unwrap();
    let qh = queue.handle();
    let _manager: client_astrea_toplevel_manager_v1::AstreaToplevelManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    connection.flush().unwrap();
    let mut state = ToplevelClientState::default();
    assert!(queue.roundtrip(&mut state).is_err());
    assert!(state.handles.is_empty());

    let _ = stop_test_server(running, server_thread);
}

fn authenticate_toplevel_client(
    socket_name: &str,
    connection: &Connection,
    globals: &wayland_client::globals::GlobalList,
    qh: &QueueHandle<ToplevelClientState>,
    queue: &mut wayland_client::EventQueue<ToplevelClientState>,
    state: &mut ToplevelClientState,
) {
    let capability_path = std::env::temp_dir().join(format!(
        ".oblivion-one-test-capability-{}-{}",
        std::process::id(),
        socket_name
    ));
    let capability = std::fs::read_to_string(capability_path).unwrap();
    let auth = globals
        .bind::<client_astrea_shell_auth_manager_v1::AstreaShellAuthManagerV1, _, _>(qh, 1..=1, ())
        .unwrap();
    auth.authenticate(capability.trim().to_string());
    connection.flush().unwrap();
    queue.roundtrip(state).unwrap();
    assert!(state.authenticated);
    assert!(!state.authentication_rejected);
}

fn assert_supervised_pid_action_rejected(
    socket_path: &std::path::Path,
    request: fn(&client_astrea_toplevel_v1::AstreaToplevelV1, u32, u32),
    token: u32,
) {
    let connection = Connection::from_socket(UnixStream::connect(socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<ToplevelClientState>(&connection).unwrap();
    let qh = queue.handle();
    let _manager: client_astrea_toplevel_manager_v1::AstreaToplevelManagerV1 =
        globals.bind(&qh, 2..=2, ()).unwrap();
    connection.flush().unwrap();
    let mut state = ToplevelClientState::default();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.handles.len(), 1);

    request(&state.handles[0], 0, token);
    connection.flush().unwrap();
    assert!(queue.roundtrip(&mut state).is_err());
    assert!(state.action_dones.is_empty());
}

#[test]
fn supervised_pid_only_client_can_read_but_cannot_mutate_v2() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    server.authorize_astrea_shell_pid(std::process::id());
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let socket_path = runtime_socket_path(&socket_name);
    let _window_client_state = create_buffered_toplevel_then_window_commands(
        &socket_path,
        &commands,
        &[] as &[ServerCommand],
    )
    .unwrap();

    let read_only_connection =
        Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (read_only_globals, mut read_only_queue) =
        registry_queue_init::<ToplevelClientState>(&read_only_connection).unwrap();
    let read_only_qh = read_only_queue.handle();
    let _read_only_manager: client_astrea_toplevel_manager_v1::AstreaToplevelManagerV1 =
        read_only_globals.bind(&read_only_qh, 1..=1, ()).unwrap();
    read_only_connection.flush().unwrap();
    let mut read_only_state = ToplevelClientState::default();
    read_only_queue.roundtrip(&mut read_only_state).unwrap();
    assert_eq!(read_only_state.handles.len(), 1);
    let initial_state = read_only_state.toplevels.values().next().unwrap().state;

    let requests: [fn(&client_astrea_toplevel_v1::AstreaToplevelV1, u32, u32); 4] = [
        |handle, high, low| handle.activate(high, low),
        |handle, high, low| handle.minimize(high, low),
        |handle, high, low| handle.restore(high, low),
        |handle, high, low| handle.close(high, low),
    ];
    for (token, request) in requests.into_iter().enumerate() {
        assert_supervised_pid_action_rejected(&socket_path, request, token as u32 + 101);
    }

    read_only_queue.roundtrip(&mut read_only_state).unwrap();
    assert_eq!(
        read_only_state.toplevels.values().next().unwrap().state,
        initial_state
    );

    let _ = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn second_same_uid_client_cannot_borrow_capability_authentication() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    server.authorize_astrea_shell_pid(std::process::id());
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let socket_path = runtime_socket_path(&socket_name);
    let _window_client_state = create_buffered_toplevel_then_window_commands(
        &socket_path,
        &commands,
        &[] as &[ServerCommand],
    )
    .unwrap();

    let authenticated_connection =
        Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (authenticated_globals, mut authenticated_queue) =
        registry_queue_init::<ToplevelClientState>(&authenticated_connection).unwrap();
    let authenticated_qh = authenticated_queue.handle();
    let mut authenticated_state = ToplevelClientState::default();
    authenticate_toplevel_client(
        &socket_name,
        &authenticated_connection,
        &authenticated_globals,
        &authenticated_qh,
        &mut authenticated_queue,
        &mut authenticated_state,
    );
    let _authenticated_manager: client_astrea_toplevel_manager_v1::AstreaToplevelManagerV1 =
        authenticated_globals
            .bind(&authenticated_qh, 2..=2, ())
            .unwrap();
    authenticated_connection.flush().unwrap();
    authenticated_queue
        .roundtrip(&mut authenticated_state)
        .unwrap();

    let unauthenticated_connection =
        Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (unauthenticated_globals, mut unauthenticated_queue) =
        registry_queue_init::<ToplevelClientState>(&unauthenticated_connection).unwrap();
    let unauthenticated_qh = unauthenticated_queue.handle();
    let _unauthenticated_manager: client_astrea_toplevel_manager_v1::AstreaToplevelManagerV1 =
        unauthenticated_globals
            .bind(&unauthenticated_qh, 2..=2, ())
            .unwrap();
    unauthenticated_connection.flush().unwrap();
    let mut unauthenticated_state = ToplevelClientState::default();
    unauthenticated_queue
        .roundtrip(&mut unauthenticated_state)
        .unwrap();
    assert!(!unauthenticated_state.authenticated);
    assert_eq!(unauthenticated_state.handles.len(), 1);

    authenticated_state.handles[0].activate(0, 201);
    authenticated_connection.flush().unwrap();
    authenticated_queue
        .roundtrip(&mut authenticated_state)
        .unwrap();
    assert_eq!(authenticated_state.action_dones, [(0, 201, 0, 1)]);

    unauthenticated_state.handles[0].minimize(0, 202);
    unauthenticated_connection.flush().unwrap();
    assert!(
        unauthenticated_queue
            .roundtrip(&mut unauthenticated_state)
            .is_err()
    );
    assert!(unauthenticated_state.action_dones.is_empty());

    let _ = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn disconnect_reconnect_does_not_transfer_capability_authentication() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    server.authorize_astrea_shell_pid(std::process::id());
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let socket_path = runtime_socket_path(&socket_name);
    let _window_client_state = create_buffered_toplevel_then_window_commands(
        &socket_path,
        &commands,
        &[] as &[ServerCommand],
    )
    .unwrap();

    {
        let connection =
            Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
        let (globals, mut queue) = registry_queue_init::<ToplevelClientState>(&connection).unwrap();
        let qh = queue.handle();
        let mut state = ToplevelClientState::default();
        authenticate_toplevel_client(
            &socket_name,
            &connection,
            &globals,
            &qh,
            &mut queue,
            &mut state,
        );
        let _manager: client_astrea_toplevel_manager_v1::AstreaToplevelManagerV1 =
            globals.bind(&qh, 2..=2, ()).unwrap();
        connection.flush().unwrap();
        queue.roundtrip(&mut state).unwrap();
        assert_eq!(state.handles.len(), 1);
    }

    let (barrier_reply, barrier_receiver) = mpsc::channel();
    commands
        .send(ServerCommand::Barrier(barrier_reply))
        .unwrap();
    barrier_receiver.recv().unwrap();

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<ToplevelClientState>(&connection).unwrap();
    let qh = queue.handle();
    let _manager: client_astrea_toplevel_manager_v1::AstreaToplevelManagerV1 =
        globals.bind(&qh, 2..=2, ()).unwrap();
    connection.flush().unwrap();
    let mut state = ToplevelClientState::default();
    queue.roundtrip(&mut state).unwrap();
    assert!(!state.authenticated);
    assert_eq!(state.handles.len(), 1);

    state.handles[0].restore(0, 301);
    connection.flush().unwrap();
    assert!(queue.roundtrip(&mut state).is_err());
    assert!(state.action_dones.is_empty());

    let _ = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn revision_helpers_are_available_to_protocol_tests() {
    let values = [0, 1, u32::MAX as u64, u32::MAX as u64 + 1, u64::MAX];
    for value in values {
        let (high, low) = crate::compositor::toplevel_publication::split_u64(value);
        assert_eq!(
            crate::compositor::toplevel_publication::join_u64(high, low),
            value
        );
    }
}

#[test]
fn pointer_motion_without_window_state_change_does_not_scan_or_publish_toplevels() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let _ = server.publish_astrea_toplevel_updates();
    let scans = server
        .state
        .astrea_toplevel_publisher
        .metrics
        .full_reconciliations;
    let revision = server.state.astrea_toplevel_publisher.revision;

    for index in 0..1_000 {
        server.send_pointer_motion(index as f64, 34.0);
    }

    assert_eq!(
        server
            .state
            .astrea_toplevel_publisher
            .metrics
            .full_reconciliations,
        scans
    );
    assert_eq!(server.state.astrea_toplevel_publisher.revision, revision);
}

#[test]
fn mapped_xdg_window_is_announced_after_initial_enumeration() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    server.authorize_astrea_shell_pid(std::process::id());
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let socket_path = runtime_socket_path(&socket_name);
    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<ToplevelClientState>(&connection).unwrap();
    let qh = queue.handle();
    let _manager: client_astrea_toplevel_manager_v1::AstreaToplevelManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    connection.flush().unwrap();
    let mut state = ToplevelClientState::default();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.events, ["manager_done"]);

    let _window_client_state = create_buffered_toplevel_then_window_commands(
        &socket_path,
        &commands,
        &[] as &[ServerCommand],
    )
    .unwrap();
    queue.roundtrip(&mut state).unwrap();

    assert_eq!(
        state.events,
        [
            "manager_done",
            "toplevel",
            "identifier",
            "app_id",
            "title",
            "pid",
            "kind",
            "state",
            "focus_serial",
            "handle_done",
            "manager_done",
        ]
    );
    let handle = state.toplevels.values().next().expect("one toplevel");
    assert_eq!(handle.identifier.as_deref(), Some("1"));
    assert!(handle.app_id.is_some());
    assert!(handle.title.is_some());
    assert!(handle.pid.is_some());
    assert_eq!(handle.kind, Some(0));
    assert!(handle.state.is_some());
    assert!(handle.focus_serial.is_some());
    assert!(handle.last_revision.is_some());
    assert_eq!(
        handle.last_revision,
        Some(crate::compositor::toplevel_publication::join_u64(
            state.manager_dones[1].0,
            state.manager_dones[1].1,
        ))
    );
    assert_eq!(state.manager_dones.len(), 2);
    assert_eq!(state.manager_dones[1].2, 1);

    let _ = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn authorized_v2_exact_xdg_actions_complete_on_the_manager() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    server.authorize_astrea_shell_pid(std::process::id());
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let socket_path = runtime_socket_path(&socket_name);
    let _window_client_state = create_buffered_toplevel_then_window_commands(
        &socket_path,
        &commands,
        &[] as &[ServerCommand],
    )
    .unwrap();

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<ToplevelClientState>(&connection).unwrap();
    let qh = queue.handle();
    let mut state = ToplevelClientState::default();
    authenticate_toplevel_client(
        &socket_name,
        &connection,
        &globals,
        &qh,
        &mut queue,
        &mut state,
    );
    let _manager: client_astrea_toplevel_manager_v1::AstreaToplevelManagerV1 =
        globals.bind(&qh, 2..=2, ()).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.handles.len(), 1);

    let handle = state.handles[0].clone();
    handle.activate(0, 1);
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    handle.minimize(0, 2);
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    handle.minimize(0, 3);
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    handle.restore(0, 4);
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    handle.close(0, 5);
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    assert_eq!(
        state.action_dones,
        [
            (0, 1, 0, 1),
            (0, 2, 1, 0),
            (0, 3, 1, 1),
            (0, 4, 2, 0),
            (0, 5, 3, 0),
        ]
    );
    assert_eq!(
        state
            .events
            .iter()
            .filter(|event| **event == "action_done")
            .count(),
        5
    );
    assert!(!state.toplevels.values().next().unwrap().closed);

    let _ = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn authorized_v2_exact_managed_x11_actions_complete_on_the_manager() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    server.authorize_astrea_shell_pid(std::process::id());
    let generation = XwaylandGeneration::new(std::num::NonZeroU64::new(1).unwrap());
    let (server_stream, xwayland_stream) = UnixStream::pair().unwrap();
    server
        .insert_xwayland_client(server_stream, generation)
        .unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let xwayland_connection = Connection::from_socket(xwayland_stream).unwrap();
    let (xwayland_globals, mut xwayland_queue) =
        registry_queue_init::<RegistryTestState>(&xwayland_connection).unwrap();
    let xwayland_qh = xwayland_queue.handle();
    let xwayland_shell: client_xwayland_shell_v1::XwaylandShellV1 =
        xwayland_globals.bind(&xwayland_qh, 1..=1, ()).unwrap();
    let compositor: client_wl_compositor::WlCompositor =
        xwayland_globals.bind(&xwayland_qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = xwayland_globals.bind(&xwayland_qh, 1..=1, ()).unwrap();
    let surface = compositor.create_surface(&xwayland_qh, ());
    let xwayland_surface = xwayland_shell.get_xwayland_surface(&surface, &xwayland_qh, ());
    xwayland_surface.set_serial(0x1111_2222, 0x3333_4444);
    surface.commit();
    xwayland_connection.flush().unwrap();
    xwayland_queue
        .roundtrip(&mut RegistryTestState::default())
        .unwrap();

    let file = create_test_shm_file(&[0xffff_ffff, 0xff10_1010, 0xff20_2020, 0xff30_3030]).unwrap();
    let pool = shm.create_pool(file.as_fd(), 16, &xwayland_qh, ());
    let buffer = pool.create_buffer(
        0,
        2,
        2,
        8,
        client_wl_shm::Format::Argb8888,
        &xwayland_qh,
        (),
    );
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    xwayland_connection.flush().unwrap();
    xwayland_queue
        .roundtrip(&mut RegistryTestState::default())
        .unwrap();

    let (association_reply, association_receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureXwaylandAssociationEvents(
            association_reply,
        ))
        .unwrap();
    let surface_id = association_receiver
        .recv()
        .unwrap()
        .into_iter()
        .find_map(|event| match event {
            XwaylandAssociationEvent::Committed { surface_id, .. } => Some(surface_id),
            XwaylandAssociationEvent::Removed { .. } => None,
        })
        .expect("Xwayland surface association");
    let x11_handle = X11WindowHandle::new(generation, 900);
    let snapshot = X11WindowSnapshot {
        handle: x11_handle,
        surface_id,
        kind: DesktopWindowKind::Managed,
        window_types: X11WindowTypes::default(),
        override_redirect: false,
        geometry: X11Geometry {
            x: 0,
            y: 0,
            width: 2,
            height: 2,
        },
        metadata: WindowMetadata {
            app_id: Some("managed-x11-action-test".into()),
            title: Some("Managed X11 action test".into()),
            pid: None,
        },
        constraints: WindowConstraints::default(),
        state: X11PublishedState::default(),
        transient_for: None,
        supports_delete: true,
        supports_take_focus: true,
        accepts_input: Some(true),
        window_role: None,
        startup_id: None,
        user_time: None,
        urgency: false,
        supports_sync_request: false,
        sync_counter: None,
    };
    let (ready_reply, ready_receiver) = mpsc::channel();
    commands
        .send(ServerCommand::ApplyXwaylandWindowEvent {
            event: Box::new(XwmEvent::WindowReady(snapshot)),
            reply: ready_reply,
        })
        .unwrap();
    ready_receiver.recv().unwrap();

    let manager_connection =
        Connection::from_socket(UnixStream::connect(runtime_socket_path(&socket_name)).unwrap())
            .unwrap();
    let (globals, mut queue) =
        registry_queue_init::<ToplevelClientState>(&manager_connection).unwrap();
    let qh = queue.handle();
    let mut state = ToplevelClientState::default();
    authenticate_toplevel_client(
        &socket_name,
        &manager_connection,
        &globals,
        &qh,
        &mut queue,
        &mut state,
    );
    let _manager: client_astrea_toplevel_manager_v1::AstreaToplevelManagerV1 =
        globals.bind(&qh, 2..=2, ()).unwrap();
    manager_connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.handles.len(), 1);
    assert_eq!(state.toplevels.values().next().unwrap().kind, Some(1));

    let handle = state.handles[0].clone();
    handle.activate(0, 1);
    manager_connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    handle.minimize(0, 2);
    manager_connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    handle.restore(0, 3);
    manager_connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    handle.close(0, 4);
    manager_connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    assert_eq!(
        state.action_dones,
        [(0, 1, 0, 1), (0, 2, 1, 0), (0, 3, 2, 0), (0, 4, 3, 0)]
    );
    assert!(!state.toplevels.values().next().unwrap().closed);
    let (backend_reply, backend_receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureXwaylandBackendCommands(backend_reply))
        .unwrap();
    assert!(
        backend_receiver
            .recv()
            .unwrap()
            .iter()
            .any(|command| matches!(command, XwmCommand::Close(window) if *window == x11_handle))
    );

    let _ = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn authorized_v1_handle_cannot_invoke_v2_action() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    server.authorize_astrea_shell_pid(std::process::id());
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let socket_path = runtime_socket_path(&socket_name);
    let _window_client_state = create_buffered_toplevel_then_window_commands(
        &socket_path,
        &commands,
        &[] as &[ServerCommand],
    )
    .unwrap();

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<ToplevelClientState>(&connection).unwrap();
    let qh = queue.handle();
    let _manager: client_astrea_toplevel_manager_v1::AstreaToplevelManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    connection.flush().unwrap();
    let mut state = ToplevelClientState::default();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.handles.len(), 1);

    // wayland-backend rejects a since=2 request before Typhon dispatches it
    // when the handle was bound at version 1.
    state.handles[0].activate(0, 99);
    connection.flush().unwrap();
    assert!(queue.roundtrip(&mut state).is_err());
    assert!(state.action_dones.is_empty());

    let _ = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn authorized_v2_stale_handle_reports_unavailable_without_reservation() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    server.authorize_astrea_shell_pid(std::process::id());
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let socket_path = runtime_socket_path(&socket_name);

    let manager_connection =
        Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (manager_globals, mut manager_queue) =
        registry_queue_init::<ToplevelClientState>(&manager_connection).unwrap();
    let manager_qh = manager_queue.handle();
    let mut manager_state = ToplevelClientState::default();
    authenticate_toplevel_client(
        &socket_name,
        &manager_connection,
        &manager_globals,
        &manager_qh,
        &mut manager_queue,
        &mut manager_state,
    );
    let _manager: client_astrea_toplevel_manager_v1::AstreaToplevelManagerV1 =
        manager_globals.bind(&manager_qh, 2..=2, ()).unwrap();
    manager_connection.flush().unwrap();
    manager_queue.roundtrip(&mut manager_state).unwrap();

    let window_connection =
        Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (window_globals, mut window_queue) =
        registry_queue_init::<RegistryTestState>(&window_connection).unwrap();
    let window_qh = window_queue.handle();
    let compositor: client_wl_compositor::WlCompositor =
        window_globals.bind(&window_qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase =
        window_globals.bind(&window_qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = window_globals.bind(&window_qh, 1..=1, ()).unwrap();
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &window_qh, 300, 200).unwrap();
    surface.commit();
    window_connection.flush().unwrap();
    window_queue
        .roundtrip(&mut RegistryTestState::default())
        .unwrap();
    manager_queue.roundtrip(&mut manager_state).unwrap();
    assert_eq!(manager_state.handles.len(), 1);

    surface.attach(None, 0, 0);
    surface.commit();
    window_connection.flush().unwrap();
    window_queue
        .roundtrip(&mut RegistryTestState::default())
        .unwrap();
    manager_queue.roundtrip(&mut manager_state).unwrap();

    let handle = manager_state.handles[0].clone();
    assert!(manager_state.toplevels.values().next().unwrap().closed);
    assert!(manager_state.action_dones.is_empty());

    handle.close(0, 42);
    manager_connection.flush().unwrap();
    manager_queue.roundtrip(&mut manager_state).unwrap();
    assert_eq!(manager_state.action_dones, [(0, 42, 3, 2)]);
    assert!(manager_state.toplevels.values().next().unwrap().closed);

    let _ = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn manager_destruction_closes_child_handles_once() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    server.authorize_astrea_shell_pid(std::process::id());
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let socket_path = runtime_socket_path(&socket_name);
    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<ToplevelClientState>(&connection).unwrap();
    let qh = queue.handle();
    let manager: client_astrea_toplevel_manager_v1::AstreaToplevelManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    connection.flush().unwrap();
    let mut state = ToplevelClientState::default();
    queue.roundtrip(&mut state).unwrap();

    let _window_client_state = create_buffered_toplevel_then_window_commands(
        &socket_path,
        &commands,
        &[] as &[ServerCommand],
    )
    .unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.handles.len(), 1);

    manager.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(
        state
            .events
            .iter()
            .filter(|event| **event == "closed")
            .count(),
        1
    );
    let child = state.toplevels.values().next().expect("one child");
    assert_eq!(child.proxy_id, state.handles[0].id());
    assert!(child.closed);
    assert_eq!(child.events_after_closed, 0);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(
        state
            .events
            .iter()
            .filter(|event| **event == "closed")
            .count(),
        1
    );

    state.handles[0].destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let _ = stop_controllable_test_server(commands, server_thread);
}
