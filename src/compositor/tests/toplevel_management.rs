use super::super::*;
use super::support::locked_relative::{runtime_socket_path, unique_socket_name};
use super::support::server_runtime::{
    ServerCommand, spawn_controllable_test_server, spawn_test_server,
    stop_controllable_test_server, stop_test_server,
};
use super::support::window_ops::create_buffered_toplevel_then_window_commands;
use crate::astrea_toplevel_management::client::{
    astrea_toplevel_manager_v1 as client_astrea_toplevel_manager_v1,
    astrea_toplevel_v1 as client_astrea_toplevel_v1,
};
use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use wayland_client::backend::ObjectId;
use wayland_client::globals::GlobalListContents;
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, globals::registry_queue_init};

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
