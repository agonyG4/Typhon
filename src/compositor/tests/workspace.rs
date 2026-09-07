use super::*;
use std::collections::{BTreeMap, HashMap};
use std::os::unix::net::UnixStream;
use wayland_client::backend::ObjectId;
use wayland_client::globals::GlobalListContents;
use wayland_client::protocol::{wl_output as client_wl_output, wl_registry};
use wayland_protocols::ext::workspace::v1::client::{
    ext_workspace_group_handle_v1 as client_ext_workspace_group_handle_v1,
    ext_workspace_handle_v1 as client_ext_workspace_handle_v1,
    ext_workspace_manager_v1 as client_ext_workspace_manager_v1,
};

struct WorkspaceWireRecord {
    handle: client_ext_workspace_handle_v1::ExtWorkspaceHandleV1,
    id: Option<String>,
    name: Option<String>,
    coordinates: Vec<u32>,
    state: u32,
    removed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkspaceWireSnapshot {
    object_id: u32,
    name: String,
    coordinates: Vec<u32>,
    state: u32,
}

#[derive(Default)]
struct WorkspaceWireState {
    workspaces: HashMap<ObjectId, WorkspaceWireRecord>,
    done_snapshots: Vec<BTreeMap<String, WorkspaceWireSnapshot>>,
    removed_count: usize,
}

impl WorkspaceWireState {
    fn snapshot(&self) -> BTreeMap<String, WorkspaceWireSnapshot> {
        self.workspaces
            .values()
            .filter_map(|workspace| {
                Some((
                    workspace.id.clone()?,
                    WorkspaceWireSnapshot {
                        object_id: workspace.handle.id().protocol_id(),
                        name: workspace.name.clone()?,
                        coordinates: workspace.coordinates.clone(),
                        state: workspace.state,
                    },
                ))
            })
            .collect()
    }

    fn workspace(&self, id: &str) -> &WorkspaceWireRecord {
        self.workspaces
            .values()
            .find(|workspace| workspace.id.as_deref() == Some(id))
            .expect("workspace should be present")
    }

    fn handle(&self, id: &str) -> client_ext_workspace_handle_v1::ExtWorkspaceHandleV1 {
        self.workspace(id).handle.clone()
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for WorkspaceWireState {
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

impl Dispatch<client_wl_output::WlOutput, ()> for WorkspaceWireState {
    fn event(
        _state: &mut Self,
        _proxy: &client_wl_output::WlOutput,
        _event: client_wl_output::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_ext_workspace_manager_v1::ExtWorkspaceManagerV1, ()> for WorkspaceWireState {
    fn event(
        state: &mut Self,
        _proxy: &client_ext_workspace_manager_v1::ExtWorkspaceManagerV1,
        event: client_ext_workspace_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            client_ext_workspace_manager_v1::Event::Workspace { workspace } => {
                state.workspaces.insert(
                    workspace.id(),
                    WorkspaceWireRecord {
                        handle: workspace,
                        id: None,
                        name: None,
                        coordinates: Vec::new(),
                        state: 0,
                        removed: false,
                    },
                );
            }
            client_ext_workspace_manager_v1::Event::Done => {
                state.done_snapshots.push(state.snapshot());
            }
            client_ext_workspace_manager_v1::Event::WorkspaceGroup { .. }
            | client_ext_workspace_manager_v1::Event::Finished => {}
            _ => {}
        }
    }

    wayland_client::event_created_child!(
        WorkspaceWireState,
        client_ext_workspace_manager_v1::ExtWorkspaceManagerV1,
        [
            0 => (client_ext_workspace_group_handle_v1::ExtWorkspaceGroupHandleV1, ()),
            1 => (client_ext_workspace_handle_v1::ExtWorkspaceHandleV1, ())
        ]
    );
}

impl Dispatch<client_ext_workspace_group_handle_v1::ExtWorkspaceGroupHandleV1, ()>
    for WorkspaceWireState
{
    fn event(
        _state: &mut Self,
        _proxy: &client_ext_workspace_group_handle_v1::ExtWorkspaceGroupHandleV1,
        _event: client_ext_workspace_group_handle_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_ext_workspace_handle_v1::ExtWorkspaceHandleV1, ()> for WorkspaceWireState {
    fn event(
        state: &mut Self,
        proxy: &client_ext_workspace_handle_v1::ExtWorkspaceHandleV1,
        event: client_ext_workspace_handle_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let workspace = state
            .workspaces
            .get_mut(&proxy.id())
            .expect("workspace must be announced before its details");
        match event {
            client_ext_workspace_handle_v1::Event::Id { id } => workspace.id = Some(id),
            client_ext_workspace_handle_v1::Event::Name { name } => workspace.name = Some(name),
            client_ext_workspace_handle_v1::Event::Coordinates { coordinates } => {
                workspace.coordinates = coordinates
                    .chunks_exact(4)
                    .map(|coordinate| u32::from_ne_bytes(coordinate.try_into().unwrap()))
                    .collect();
            }
            client_ext_workspace_handle_v1::Event::State { state: value } => {
                workspace.state = value.into();
            }
            client_ext_workspace_handle_v1::Event::Removed => {
                workspace.removed = true;
                state.removed_count += 1;
            }
            client_ext_workspace_handle_v1::Event::Capabilities { .. } => {}
            _ => {}
        }
    }
}

#[test]
fn wire_visibility_is_atomic_and_independent_of_astrea_manager() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<WorkspaceWireState>(&connection).unwrap();
    let qh = queue.handle();
    let manager: client_ext_workspace_manager_v1::ExtWorkspaceManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    connection.flush().unwrap();

    let mut state = WorkspaceWireState::default();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.done_snapshots.len(), 1);
    assert_eq!(state.workspaces.len(), 10);
    for (index, workspace_id) in (1..=10)
        .map(|id| format!("typhon.workspace.{id}"))
        .enumerate()
    {
        let workspace = state.workspace(&workspace_id);
        let expected_name = (index + 1).to_string();
        assert_eq!(workspace.name.as_deref(), Some(expected_name.as_str()));
        assert_eq!(workspace.coordinates, vec![index as u32]);
        if index == 0 {
            assert_eq!(workspace.state, 1);
        } else {
            assert_eq!(workspace.state, 4);
        }
    }

    let mut app = LiveTestClient::connect(&socket_path).unwrap();
    let surface = app
        .create_toplevel_surface("workspace-wire-test", 32, 32)
        .unwrap();
    app.commit_surface(&surface, 32, 32).unwrap();
    queue.roundtrip(&mut state).unwrap();

    let done_before_move = state.done_snapshots.len();
    commands
        .send(ServerCommand::MoveFocusedWindowToWorkspace { workspace: 3 })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.done_snapshots.len(), done_before_move + 1);
    let workspace_three_object = state.workspace("typhon.workspace.3").handle.id();
    assert_eq!(state.workspace("typhon.workspace.3").state, 0);
    assert_eq!(state.workspace("typhon.workspace.1").state, 1);

    let occupied_connection =
        Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (occupied_globals, mut occupied_queue) =
        registry_queue_init::<WorkspaceWireState>(&occupied_connection).unwrap();
    let _occupied_manager: client_ext_workspace_manager_v1::ExtWorkspaceManagerV1 =
        occupied_globals
            .bind(&occupied_queue.handle(), 1..=1, ())
            .unwrap();
    occupied_connection.flush().unwrap();
    let mut occupied_state = WorkspaceWireState::default();
    occupied_queue.roundtrip(&mut occupied_state).unwrap();
    assert_eq!(occupied_state.done_snapshots.len(), 1);
    assert_eq!(occupied_state.workspace("typhon.workspace.3").state, 0);
    assert_eq!(
        occupied_state.workspace("typhon.workspace.3").handle.id(),
        workspace_three_object
    );

    let workspace_three = state.handle("typhon.workspace.3");
    workspace_three.activate();
    manager.commit();
    connection.flush().unwrap();
    let done_before_workspace_three_activation = state.done_snapshots.len();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(
        state.done_snapshots.len(),
        done_before_workspace_three_activation + 1
    );
    assert_eq!(state.workspace("typhon.workspace.3").state, 1);

    commands
        .send(ServerCommand::MoveFocusedWindowToOrFromSpecialWorkspace)
        .unwrap();
    wait_for_server_commands(&commands);
    let done_before_special = state.done_snapshots.len();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.done_snapshots.len(), done_before_special + 1);
    assert_eq!(state.workspace("typhon.workspace.3").state, 1);

    commands
        .send(ServerCommand::ToggleDefaultSpecialWorkspace)
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    let workspace_one = state.handle("typhon.workspace.1");
    workspace_one.activate();
    manager.commit();
    connection.flush().unwrap();
    let done_before_special_visibility_check = state.done_snapshots.len();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(
        state.done_snapshots.len(),
        done_before_special_visibility_check + 1
    );
    assert_eq!(state.workspace("typhon.workspace.1").state, 1);
    assert_eq!(state.workspace("typhon.workspace.3").state, 4);

    commands
        .send(ServerCommand::MoveFocusedWindowToWorkspace { workspace: 3 })
        .unwrap();
    wait_for_server_commands(&commands);
    let done_before_special_restore = state.done_snapshots.len();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.done_snapshots.len(), done_before_special_restore + 1);
    assert_eq!(state.workspace("typhon.workspace.3").state, 0);
    assert_eq!(
        state.workspace("typhon.workspace.3").handle.id(),
        workspace_three_object
    );

    commands
        .send(ServerCommand::ToggleDefaultSpecialWorkspace)
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    workspace_three.activate();
    manager.commit();
    connection.flush().unwrap();
    let done_before_restored_workspace_activation = state.done_snapshots.len();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(
        state.done_snapshots.len(),
        done_before_restored_workspace_activation + 1
    );
    assert_eq!(state.workspace("typhon.workspace.3").state, 1);

    commands.send(ServerCommand::MinimizeFocused).unwrap();
    wait_for_server_commands(&commands);
    assert_eq!(state.workspace("typhon.workspace.3").state, 1);

    workspace_one.activate();
    manager.commit();
    connection.flush().unwrap();
    let done_before_workspace_one_activation = state.done_snapshots.len();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(
        state.done_snapshots.len(),
        done_before_workspace_one_activation + 1
    );
    assert_eq!(state.workspace("typhon.workspace.1").state, 1);
    assert_eq!(state.workspace("typhon.workspace.3").state, 0);

    assert_eq!(
        state.workspace("typhon.workspace.3").handle.id(),
        workspace_three_object
    );

    app.unmap_surface(&surface).unwrap();
    let done_before_unmap = state.done_snapshots.len();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.done_snapshots.len(), done_before_unmap + 1);
    assert_eq!(state.workspace("typhon.workspace.3").state, 4);

    let workspace_five = state.handle("typhon.workspace.5");
    workspace_five.activate();
    manager.commit();
    connection.flush().unwrap();
    let done_before_activation = state.done_snapshots.len();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.done_snapshots.len(), done_before_activation + 1);
    assert_eq!(state.workspace("typhon.workspace.5").state, 1);
    assert_eq!(state.workspace("typhon.workspace.1").state, 4);
    assert_eq!(state.workspace("typhon.workspace.3").state, 4);

    workspace_one.activate();
    manager.commit();
    connection.flush().unwrap();
    let done_before_empty_workspace_deactivation = state.done_snapshots.len();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(
        state.done_snapshots.len(),
        done_before_empty_workspace_deactivation + 1
    );
    assert_eq!(state.workspace("typhon.workspace.1").state, 1);
    assert_eq!(state.workspace("typhon.workspace.5").state, 4);
    assert_eq!(
        state.workspace("typhon.workspace.3").handle.id(),
        workspace_three_object
    );
    assert_eq!(state.removed_count, 0);

    let _ = stop_controllable_test_server(commands, server_thread);
}
