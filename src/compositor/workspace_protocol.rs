use super::*;
use wayland_protocols::ext::workspace::v1::server::{
    ext_workspace_group_handle_v1, ext_workspace_handle_v1, ext_workspace_manager_v1,
};

const MAX_WORKSPACE_MANAGERS: usize = 32;
const MAX_WORKSPACE_MANAGERS_PER_CLIENT: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspaceProtocolSnapshotItem {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) coordinates: Vec<u32>,
    pub(crate) active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspaceProtocolSnapshot {
    pub(crate) workspaces: Vec<WorkspaceProtocolSnapshotItem>,
}

impl WorkspaceProtocolSnapshot {
    pub(crate) fn from_workspace_ids(
        workspaces: impl IntoIterator<Item = crate::wm::WorkspaceId>,
        active: crate::wm::WorkspaceId,
    ) -> Self {
        Self {
            workspaces: workspaces
                .into_iter()
                .enumerate()
                .map(|(index, workspace)| WorkspaceProtocolSnapshotItem {
                    id: format!("typhon.workspace.{}", workspace.get()),
                    name: workspace.to_string(),
                    coordinates: vec![index as u32],
                    active: workspace == active,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct WorkspaceRequestTransaction {
    activation: Option<crate::wm::WorkspaceId>,
}

impl WorkspaceRequestTransaction {
    pub(crate) fn request_activation(&mut self, workspace: crate::wm::WorkspaceId, valid: bool) {
        if valid {
            self.activation = Some(workspace);
        }
    }

    pub(crate) fn take_activation(&mut self) -> Option<crate::wm::WorkspaceId> {
        self.activation.take()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct WorkspaceManagerResourceData {
    pub(crate) client_id: ClientId,
}

#[derive(Debug, Clone)]
pub(crate) struct WorkspaceGroupResourceData {
    pub(crate) manager_id: ObjectId,
}

#[derive(Debug, Clone)]
pub(crate) struct WorkspaceHandleResourceData {
    pub(crate) manager_id: ObjectId,
    pub(crate) client_id: ClientId,
    pub(crate) workspace: crate::wm::WorkspaceId,
}

#[derive(Debug)]
struct WorkspaceGroupBinding {
    resource: ext_workspace_group_handle_v1::ExtWorkspaceGroupHandleV1,
    entered_outputs: HashSet<ObjectId>,
}

#[derive(Debug)]
struct WorkspaceHandleBinding {
    resource: ext_workspace_handle_v1::ExtWorkspaceHandleV1,
    workspace: crate::wm::WorkspaceId,
}

#[derive(Debug)]
struct WorkspaceManagerBinding {
    resource: ext_workspace_manager_v1::ExtWorkspaceManagerV1,
    client_id: ClientId,
    group: WorkspaceGroupBinding,
    handles: Vec<WorkspaceHandleBinding>,
    transaction: WorkspaceRequestTransaction,
}

#[derive(Debug, Default)]
pub(crate) struct WorkspaceProtocolState {
    managers: HashMap<ObjectId, WorkspaceManagerBinding>,
}

impl WorkspaceProtocolState {
    pub(crate) fn manager_count(&self) -> usize {
        self.managers.len()
    }

    pub(crate) fn manager_count_for_client(&self, client_id: &ClientId) -> usize {
        self.managers
            .values()
            .filter(|binding| binding.client_id == *client_id)
            .count()
    }

    pub(crate) fn can_bind(&self, client_id: &ClientId) -> bool {
        self.manager_count() < MAX_WORKSPACE_MANAGERS
            && self.manager_count_for_client(client_id) < MAX_WORKSPACE_MANAGERS_PER_CLIENT
    }

    pub(crate) fn bind_manager(
        &mut self,
        display: &DisplayHandle,
        client: &Client,
        manager: ext_workspace_manager_v1::ExtWorkspaceManagerV1,
        workspaces: Vec<crate::wm::WorkspaceId>,
        active: crate::wm::WorkspaceId,
        outputs: &[wl_output::WlOutput],
    ) {
        let manager_id = manager.id();
        let client_id = client.id();
        let Ok(group) = client.create_resource::<
            ext_workspace_group_handle_v1::ExtWorkspaceGroupHandleV1,
            WorkspaceGroupResourceData,
            CompositorState,
        >(
            display,
            manager.version(),
            WorkspaceGroupResourceData {
                manager_id: manager_id.clone(),
            },
        ) else {
            return;
        };

        let snapshot = WorkspaceProtocolSnapshot::from_workspace_ids(workspaces, active);
        let _ = manager.send_event(ext_workspace_manager_v1::Event::WorkspaceGroup {
            workspace_group: group.clone(),
        });
        let _ = group.send_event(ext_workspace_group_handle_v1::Event::Capabilities {
            capabilities: WEnum::Value(ext_workspace_group_handle_v1::GroupCapabilities::empty()),
        });

        let mut handles = Vec::new();
        for item in &snapshot.workspaces {
            let Some(workspace) = item
                .id
                .strip_prefix("typhon.workspace.")
                .and_then(|value| value.parse::<u32>().ok())
                .and_then(crate::wm::WorkspaceId::new)
            else {
                continue;
            };
            let Ok(handle) = client.create_resource::<
                ext_workspace_handle_v1::ExtWorkspaceHandleV1,
                WorkspaceHandleResourceData,
                CompositorState,
            >(
                display,
                manager.version(),
                WorkspaceHandleResourceData {
                    manager_id: manager_id.clone(),
                    client_id: client_id.clone(),
                    workspace,
                },
            ) else {
                continue;
            };
            let _ = manager.send_event(ext_workspace_manager_v1::Event::Workspace {
                workspace: handle.clone(),
            });
            let _ = handle.send_event(ext_workspace_handle_v1::Event::Id {
                id: item.id.clone(),
            });
            let _ = handle.send_event(ext_workspace_handle_v1::Event::Name {
                name: item.name.clone(),
            });
            let _ = handle.send_event(ext_workspace_handle_v1::Event::Coordinates {
                coordinates: item
                    .coordinates
                    .iter()
                    .flat_map(|coordinate| coordinate.to_ne_bytes())
                    .collect(),
            });
            let state = if item.active {
                ext_workspace_handle_v1::State::Active
            } else {
                ext_workspace_handle_v1::State::empty()
            };
            let _ = handle.send_event(ext_workspace_handle_v1::Event::State {
                state: WEnum::Value(state),
            });
            let _ = handle.send_event(ext_workspace_handle_v1::Event::Capabilities {
                capabilities: WEnum::Value(
                    ext_workspace_handle_v1::WorkspaceCapabilities::Activate,
                ),
            });
            let _ = group.send_event(ext_workspace_group_handle_v1::Event::WorkspaceEnter {
                workspace: handle.clone(),
            });
            handles.push(WorkspaceHandleBinding {
                resource: handle,
                workspace,
            });
        }

        let mut group_binding = WorkspaceGroupBinding {
            resource: group,
            entered_outputs: HashSet::new(),
        };
        for output in outputs.iter().filter(|output| {
            output
                .client()
                .is_some_and(|output_client| output_client.id() == client_id)
        }) {
            Self::send_output_enter(&mut group_binding, output);
        }

        let _ = manager.send_event(ext_workspace_manager_v1::Event::Done);
        self.managers.insert(
            manager_id,
            WorkspaceManagerBinding {
                resource: manager,
                client_id,
                group: group_binding,
                handles,
                transaction: WorkspaceRequestTransaction::default(),
            },
        );
    }

    pub(crate) fn queue_activation(
        &mut self,
        manager_id: &ObjectId,
        client_id: &ClientId,
        workspace: crate::wm::WorkspaceId,
        valid: bool,
    ) {
        let Some(binding) = self.managers.get_mut(manager_id) else {
            return;
        };
        if binding.client_id != *client_id {
            return;
        }
        binding.transaction.request_activation(workspace, valid);
    }

    pub(crate) fn take_activation(
        &mut self,
        manager_id: &ObjectId,
        client_id: &ClientId,
    ) -> Option<crate::wm::WorkspaceId> {
        let binding = self.managers.get_mut(manager_id)?;
        if binding.client_id != *client_id {
            return None;
        }
        binding.transaction.take_activation()
    }

    pub(crate) fn workspace_is_known(
        &self,
        manager_id: &ObjectId,
        workspace: crate::wm::WorkspaceId,
    ) -> bool {
        self.managers.get(manager_id).is_some_and(|binding| {
            binding
                .handles
                .iter()
                .any(|handle| handle.workspace == workspace)
        })
    }

    pub(crate) fn publish_state(&mut self, active: crate::wm::WorkspaceId) {
        self.managers
            .retain(|_, binding| binding.resource.is_alive());
        for binding in self.managers.values_mut() {
            binding.handles.retain(|handle| handle.resource.is_alive());
            for handle in &binding.handles {
                let state = if handle.workspace == active {
                    ext_workspace_handle_v1::State::Active
                } else {
                    ext_workspace_handle_v1::State::empty()
                };
                let _ = handle
                    .resource
                    .send_event(ext_workspace_handle_v1::Event::State {
                        state: WEnum::Value(state),
                    });
            }
            let _ = binding
                .resource
                .send_event(ext_workspace_manager_v1::Event::Done);
        }
    }

    pub(crate) fn output_enter(&mut self, output: &wl_output::WlOutput) {
        let Some(output_client) = output.client() else {
            return;
        };
        for binding in self.managers.values_mut() {
            if binding.client_id == output_client.id() {
                Self::send_output_enter(&mut binding.group, output);
            }
        }
    }

    pub(crate) fn output_leave(&mut self, output: &wl_output::WlOutput) {
        let Some(output_client) = output.client() else {
            return;
        };
        let output_id = output.id();
        for binding in self.managers.values_mut() {
            if binding.client_id != output_client.id() {
                continue;
            }
            if binding.group.entered_outputs.remove(&output_id) {
                let _ = binding.group.resource.send_event(
                    ext_workspace_group_handle_v1::Event::OutputLeave {
                        output: output.clone(),
                    },
                );
            }
        }
    }

    fn send_output_enter(group: &mut WorkspaceGroupBinding, output: &wl_output::WlOutput) {
        if !group.resource.is_alive()
            || !output.is_alive()
            || !group.entered_outputs.insert(output.id())
        {
            return;
        }
        let _ = group
            .resource
            .send_event(ext_workspace_group_handle_v1::Event::OutputEnter {
                output: output.clone(),
            });
    }

    pub(crate) fn stop_manager(&mut self, manager_id: &ObjectId, client_id: &ClientId) {
        let Some(binding) = self.managers.remove(manager_id) else {
            return;
        };
        if binding.client_id != *client_id || !binding.resource.is_alive() {
            return;
        }
        let _ = binding
            .resource
            .send_event(ext_workspace_manager_v1::Event::Finished);
    }

    pub(crate) fn remove_manager(&mut self, manager_id: &ObjectId) {
        self.managers.remove(manager_id);
    }

    pub(crate) fn remove_group(&mut self, manager_id: &ObjectId, group_id: &ObjectId) {
        let Some(binding) = self.managers.get_mut(manager_id) else {
            return;
        };
        if binding.group.resource.id() == *group_id {
            binding.group.entered_outputs.clear();
        }
    }

    pub(crate) fn remove_handle(&mut self, manager_id: &ObjectId, handle_id: &ObjectId) {
        if let Some(binding) = self.managers.get_mut(manager_id) {
            binding
                .handles
                .retain(|handle| Resource::id(&handle.resource) != *handle_id);
        }
    }

    pub(crate) fn remove_client(&mut self, client_id: &ClientId) {
        self.managers
            .retain(|_, binding| binding.client_id != *client_id);
    }
}

impl CompositorState {
    pub(in crate::compositor) fn bind_workspace_manager(
        &mut self,
        display: &DisplayHandle,
        client: &Client,
        manager: ext_workspace_manager_v1::ExtWorkspaceManagerV1,
    ) {
        if !self.workspace_protocol.can_bind(&client.id()) {
            return;
        }
        self.workspace_protocol.bind_manager(
            display,
            client,
            manager,
            self.workspace_manager.workspaces().collect(),
            self.workspace_manager.active_workspace(),
            &self.output_resources,
        );
    }

    pub(in crate::compositor) fn queue_workspace_activation(
        &mut self,
        manager_id: &ObjectId,
        client_id: &ClientId,
        workspace: crate::wm::WorkspaceId,
    ) {
        let valid = self.workspace_manager.contains(workspace)
            && self
                .workspace_protocol
                .workspace_is_known(manager_id, workspace);
        self.workspace_protocol
            .queue_activation(manager_id, client_id, workspace, valid);
    }

    pub(in crate::compositor) fn commit_workspace_transaction(
        &mut self,
        manager_id: &ObjectId,
        client_id: &ClientId,
    ) {
        let Some(workspace) = self
            .workspace_protocol
            .take_activation(manager_id, client_id)
        else {
            return;
        };
        let _ = self.activate_workspace(workspace);
    }

    pub(in crate::compositor) fn publish_workspace_state(&mut self) {
        self.workspace_protocol
            .publish_state(self.workspace_manager.active_workspace());
    }

    pub(in crate::compositor) fn publish_workspace_output_enter(
        &mut self,
        output: &wl_output::WlOutput,
    ) {
        self.workspace_protocol.output_enter(output);
    }

    pub(in crate::compositor) fn publish_workspace_output_leave(
        &mut self,
        output: &wl_output::WlOutput,
    ) {
        self.workspace_protocol.output_leave(output);
    }

    pub(in crate::compositor) fn stop_workspace_manager(
        &mut self,
        manager_id: &ObjectId,
        client_id: &ClientId,
    ) {
        self.workspace_protocol.stop_manager(manager_id, client_id);
    }

    pub(in crate::compositor) fn remove_workspace_manager(&mut self, manager_id: &ObjectId) {
        self.workspace_protocol.remove_manager(manager_id);
    }

    pub(in crate::compositor) fn remove_workspace_group(
        &mut self,
        manager_id: &ObjectId,
        group_id: &ObjectId,
    ) {
        self.workspace_protocol.remove_group(manager_id, group_id);
    }

    pub(in crate::compositor) fn remove_workspace_handle(
        &mut self,
        manager_id: &ObjectId,
        handle_id: &ObjectId,
    ) {
        self.workspace_protocol.remove_handle(manager_id, handle_id);
    }

    pub(in crate::compositor) fn remove_workspace_client(&mut self, client_id: &ClientId) {
        self.workspace_protocol.remove_client(client_id);
    }
}
