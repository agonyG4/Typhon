use super::super::workspace_protocol::{
    WorkspaceGroupResourceData, WorkspaceHandleResourceData, WorkspaceManagerResourceData,
};
use super::super::*;
use wayland_protocols::ext::workspace::v1::server::{
    ext_workspace_group_handle_v1, ext_workspace_handle_v1, ext_workspace_manager_v1,
};

impl GlobalDispatch<ext_workspace_manager_v1::ExtWorkspaceManagerV1, ()> for CompositorState {
    fn bind(
        state: &mut Self,
        handle: &DisplayHandle,
        client: &Client,
        resource: New<ext_workspace_manager_v1::ExtWorkspaceManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        let manager = data_init.init(
            resource,
            WorkspaceManagerResourceData {
                client_id: client.id(),
            },
        );
        state.bind_workspace_manager(handle, client, manager);
    }
}

impl Dispatch<ext_workspace_manager_v1::ExtWorkspaceManagerV1, WorkspaceManagerResourceData>
    for CompositorState
{
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &ext_workspace_manager_v1::ExtWorkspaceManagerV1,
        request: ext_workspace_manager_v1::Request,
        data: &WorkspaceManagerResourceData,
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if data.client_id != client.id() {
            return;
        }
        match request {
            ext_workspace_manager_v1::Request::Commit => {
                state.commit_workspace_transaction(&resource.id(), &client.id());
            }
            ext_workspace_manager_v1::Request::Stop => {
                state.stop_workspace_manager(&resource.id(), &client.id());
            }
            _ => {}
        }
    }

    fn destroyed(
        state: &mut Self,
        _client_id: ClientId,
        resource: &ext_workspace_manager_v1::ExtWorkspaceManagerV1,
        _data: &WorkspaceManagerResourceData,
    ) {
        state.remove_workspace_manager(&resource.id());
    }
}

impl Dispatch<ext_workspace_group_handle_v1::ExtWorkspaceGroupHandleV1, WorkspaceGroupResourceData>
    for CompositorState
{
    fn request(
        _state: &mut Self,
        client: &Client,
        _resource: &ext_workspace_group_handle_v1::ExtWorkspaceGroupHandleV1,
        _request: ext_workspace_group_handle_v1::Request,
        data: &WorkspaceGroupResourceData,
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if data.client_id != client.id() {
            return;
        }
        // Typhon intentionally advertises no group mutation capability.
        // Unsupported requests are protocol no-ops as required by ext-workspace-v1.
    }

    fn destroyed(
        state: &mut Self,
        _client_id: ClientId,
        resource: &ext_workspace_group_handle_v1::ExtWorkspaceGroupHandleV1,
        data: &WorkspaceGroupResourceData,
    ) {
        state.remove_workspace_group(&data.manager_id, &resource.id());
    }
}

impl Dispatch<ext_workspace_handle_v1::ExtWorkspaceHandleV1, WorkspaceHandleResourceData>
    for CompositorState
{
    fn request(
        state: &mut Self,
        client: &Client,
        _resource: &ext_workspace_handle_v1::ExtWorkspaceHandleV1,
        request: ext_workspace_handle_v1::Request,
        data: &WorkspaceHandleResourceData,
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if data.client_id != client.id() {
            return;
        }
        if matches!(request, ext_workspace_handle_v1::Request::Activate) {
            state.queue_workspace_activation(&data.manager_id, &data.client_id, data.workspace);
        }
    }

    fn destroyed(
        state: &mut Self,
        _client_id: ClientId,
        resource: &ext_workspace_handle_v1::ExtWorkspaceHandleV1,
        data: &WorkspaceHandleResourceData,
    ) {
        state.remove_workspace_handle(&data.manager_id, &Resource::id(resource));
    }
}
