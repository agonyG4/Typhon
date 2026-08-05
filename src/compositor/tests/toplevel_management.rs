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
use std::os::unix::net::UnixStream;
use wayland_client::globals::GlobalListContents;
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, QueueHandle, globals::registry_queue_init};

#[derive(Debug, Default)]
struct ToplevelClientState {
    events: Vec<&'static str>,
    manager_dones: Vec<(u32, u32, u32, u32)>,
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
            client_astrea_toplevel_manager_v1::Event::Toplevel { id: _ } => {
                state.events.push("toplevel");
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
        _proxy: &client_astrea_toplevel_v1::AstreaToplevelV1,
        event: client_astrea_toplevel_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        state.events.push(match event {
            client_astrea_toplevel_v1::Event::Identifier { .. } => "identifier",
            client_astrea_toplevel_v1::Event::AppId { .. } => "app_id",
            client_astrea_toplevel_v1::Event::Title { .. } => "title",
            client_astrea_toplevel_v1::Event::Pid { .. } => "pid",
            client_astrea_toplevel_v1::Event::Kind { .. } => "kind",
            client_astrea_toplevel_v1::Event::State { .. } => "state",
            client_astrea_toplevel_v1::Event::FocusSerial { .. } => "focus_serial",
            client_astrea_toplevel_v1::Event::Done { .. } => "handle_done",
            client_astrea_toplevel_v1::Event::Closed => "closed",
        });
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
    assert_eq!(state.manager_dones.len(), 2);
    assert_eq!(state.manager_dones[1].2, 1);

    let _ = stop_controllable_test_server(commands, server_thread);
}
