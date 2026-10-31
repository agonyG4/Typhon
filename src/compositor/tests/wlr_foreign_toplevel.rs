use super::super::*;
use super::support::locked_relative::{runtime_socket_path, unique_socket_name};
use super::support::server_runtime::{
    ServerCommand, spawn_controllable_test_server, stop_controllable_test_server,
};
use super::support::window_ops::create_buffered_toplevel_then_window_commands;
use std::os::unix::net::UnixStream;
use wayland_client::globals::GlobalListContents;
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, QueueHandle, globals::registry_queue_init};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1 as client_handle_v1,
    zwlr_foreign_toplevel_manager_v1 as client_manager_v1,
};

#[derive(Debug, Default)]
struct ForeignToplevelClientState {
    toplevels: usize,
    titles: Vec<String>,
    app_ids: Vec<String>,
    states: Vec<Vec<u8>>,
    dones: usize,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for ForeignToplevelClientState {
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

impl Dispatch<client_manager_v1::ZwlrForeignToplevelManagerV1, ()> for ForeignToplevelClientState {
    fn event(
        state: &mut Self,
        _proxy: &client_manager_v1::ZwlrForeignToplevelManagerV1,
        event: client_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if matches!(event, client_manager_v1::Event::Toplevel { .. }) {
            state.toplevels += 1;
        }
    }

    wayland_client::event_created_child!(
        ForeignToplevelClientState,
        client_manager_v1::ZwlrForeignToplevelManagerV1,
        [0 => (client_handle_v1::ZwlrForeignToplevelHandleV1, ())]
    );
}

impl Dispatch<client_handle_v1::ZwlrForeignToplevelHandleV1, ()> for ForeignToplevelClientState {
    fn event(
        state: &mut Self,
        _proxy: &client_handle_v1::ZwlrForeignToplevelHandleV1,
        event: client_handle_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            client_handle_v1::Event::Title { title } => state.titles.push(title),
            client_handle_v1::Event::AppId { app_id } => state.app_ids.push(app_id),
            client_handle_v1::Event::State { state: wire } => state.states.push(wire),
            client_handle_v1::Event::Done => state.dones += 1,
            _ => {}
        }
    }
}

#[test]
fn mapped_xdg_window_is_published_to_wlr_foreign_toplevel_clients() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let socket_path = runtime_socket_path(&socket_name);
    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) =
        registry_queue_init::<ForeignToplevelClientState>(&connection).unwrap();
    let qh = queue.handle();
    let _manager: client_manager_v1::ZwlrForeignToplevelManagerV1 =
        globals.bind(&qh, 3..=3, ()).unwrap();
    let mut state = ForeignToplevelClientState::default();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.toplevels, 0);

    let _window_client_state = create_buffered_toplevel_then_window_commands(
        &socket_path,
        &commands,
        &[] as &[ServerCommand],
    )
    .unwrap();
    queue.roundtrip(&mut state).unwrap();

    assert_eq!(state.toplevels, 1);
    assert_eq!(state.titles.len(), 1);
    assert_eq!(state.app_ids.len(), 1);
    assert!(!state.states.is_empty());
    assert!(state.dones >= 1);

    let _ = stop_controllable_test_server(commands, server_thread);
}
