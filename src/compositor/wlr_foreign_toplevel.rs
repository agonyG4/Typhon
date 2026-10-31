//! `zwlr_foreign_toplevel_manager_v1` mirror of the Astrea toplevel snapshots.
//!
//! xdg-desktop-portal-hyprland requires this global before it starts a
//! ScreenCast session, and taskbars use it for window lists.

use std::collections::BTreeMap;

use wayland_protocols_wlr::foreign_toplevel::v1::server::{
    zwlr_foreign_toplevel_handle_v1, zwlr_foreign_toplevel_manager_v1,
};
use wayland_server::backend::ObjectId;

use super::toplevel_publication::{AstreaToplevelSnapshot, AstreaToplevelStates};
use super::*;

const HANDLE_PARENT_SINCE: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
struct PublishedToplevel {
    title: String,
    app_id: String,
    states: AstreaToplevelStates,
}

impl PublishedToplevel {
    fn from_snapshot(snapshot: &AstreaToplevelSnapshot) -> Self {
        Self {
            title: snapshot.title.clone(),
            app_id: snapshot.app_id.clone(),
            states: snapshot.states,
        }
    }
}

#[derive(Debug)]
pub(in crate::compositor) struct WlrToplevelManagerBinding {
    manager: zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1,
    client_id: ClientId,
    handles: BTreeMap<
        WindowId,
        (
            zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1,
            PublishedToplevel,
        ),
    >,
}

#[derive(Debug)]
pub struct WlrToplevelHandleData {
    window_id: WindowId,
}

fn wire_states(states: AstreaToplevelStates) -> Vec<u8> {
    use zwlr_foreign_toplevel_handle_v1::State;
    [
        (AstreaToplevelStates::MAXIMIZED, State::Maximized),
        (AstreaToplevelStates::MINIMIZED, State::Minimized),
        (AstreaToplevelStates::ACTIVE, State::Activated),
        (AstreaToplevelStates::FULLSCREEN, State::Fullscreen),
    ]
    .into_iter()
    .filter(|(flag, _)| states.contains(*flag))
    .flat_map(|(_, state)| (state as u32).to_ne_bytes())
    .collect()
}

fn send_published(
    handle: &zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1,
    previous: Option<&PublishedToplevel>,
    next: &PublishedToplevel,
) {
    if previous.is_none_or(|previous| previous.title != next.title) {
        handle.title(next.title.clone());
    }
    if previous.is_none_or(|previous| previous.app_id != next.app_id) {
        handle.app_id(next.app_id.clone());
    }
    if previous.is_none_or(|previous| previous.states != next.states) {
        handle.state(wire_states(next.states));
    }
    if previous.is_none() && handle.version() >= HANDLE_PARENT_SINCE {
        handle.parent(None);
    }
    handle.done();
}

impl GlobalDispatch<zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1, ()>
    for CompositorState
{
    fn bind(
        state: &mut Self,
        handle: &DisplayHandle,
        client: &Client,
        resource: New<zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        let manager = data_init.init(resource, ());
        let mut binding = WlrToplevelManagerBinding {
            manager,
            client_id: client.id(),
            handles: BTreeMap::new(),
        };
        if let Ok(collection) = state.collect_astrea_toplevels() {
            state.sync_wlr_toplevel_binding(handle, client, &mut binding, &collection.snapshots);
        }
        state.wlr_toplevel_managers.push(binding);
    }
}

impl Dispatch<zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1, ()>
    for CompositorState
{
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1,
        request: zwlr_foreign_toplevel_manager_v1::Request,
        _data: &(),
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if matches!(request, zwlr_foreign_toplevel_manager_v1::Request::Stop) {
            state.remove_wlr_toplevel_manager(&resource.id());
            resource.finished();
        }
    }

    fn destroyed(
        state: &mut Self,
        _client_id: ClientId,
        resource: &zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1,
        _data: &(),
    ) {
        state.remove_wlr_toplevel_manager(&resource.id());
    }
}

impl Dispatch<zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1, WlrToplevelHandleData>
    for CompositorState
{
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1,
        request: zwlr_foreign_toplevel_handle_v1::Request,
        data: &WlrToplevelHandleData,
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        use zwlr_foreign_toplevel_handle_v1::Request;
        let window_id = data.window_id;
        let maximized = || {
            state
                .astrea_toplevel_snapshot(window_id)
                .is_some_and(|snapshot| snapshot.states.contains(AstreaToplevelStates::MAXIMIZED))
        };
        match request {
            Request::Activate { .. } => {
                let _ = state.activate_desktop_window_action(window_id);
            }
            Request::Close => {
                let _ = state.close_desktop_window_outcome(window_id);
            }
            Request::SetMinimized => {
                let _ = state.minimize_desktop_window_outcome(window_id);
            }
            Request::UnsetMinimized => {
                let _ = state.restore_minimized_desktop_window_outcome(window_id);
            }
            Request::SetMaximized if !maximized() => {
                state.toggle_maximize_desktop_window(window_id);
            }
            Request::UnsetMaximized if maximized() => {
                state.toggle_maximize_desktop_window(window_id);
            }
            // Fullscreen and minimize-rectangle hints are optional for
            // foreign clients; Typhon keeps those under its own policy.
            _ => {}
        }
    }
}

impl CompositorState {
    fn sync_wlr_toplevel_binding(
        &self,
        display: &DisplayHandle,
        client: &Client,
        binding: &mut WlrToplevelManagerBinding,
        snapshots: &BTreeMap<WindowId, AstreaToplevelSnapshot>,
    ) {
        binding.handles.retain(|window_id, (handle, _)| {
            let keep = snapshots.contains_key(window_id);
            if !keep {
                handle.closed();
            }
            keep
        });
        for (window_id, snapshot) in snapshots {
            let next = PublishedToplevel::from_snapshot(snapshot);
            if let Some((handle, previous)) = binding.handles.get_mut(window_id) {
                if *previous != next {
                    send_published(handle, Some(previous), &next);
                    *previous = next;
                }
                continue;
            }
            let Ok(handle) = client
                .create_resource::<
                    zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1,
                    WlrToplevelHandleData,
                    CompositorState,
                >(
                    display,
                    binding.manager.version(),
                    WlrToplevelHandleData {
                        window_id: *window_id,
                    },
                )
            else {
                continue;
            };
            binding.manager.toplevel(&handle);
            for output in self.output_resources.iter().filter(|output| {
                output.client_id == binding.client_id && output.resource.is_alive()
            }) {
                handle.output_enter(&output.resource);
            }
            send_published(&handle, None, &next);
            binding.handles.insert(*window_id, (handle, next));
        }
    }

    /// Bring every bound manager up to date with the current toplevel set.
    pub(in crate::compositor) fn sync_wlr_foreign_toplevels(&mut self, display: &DisplayHandle) {
        if self.wlr_toplevel_managers.is_empty() {
            return;
        }
        let Ok(collection) = self.collect_astrea_toplevels() else {
            return;
        };
        let mut managers = std::mem::take(&mut self.wlr_toplevel_managers);
        managers.retain(|binding| binding.manager.is_alive());
        for binding in &mut managers {
            let Some(client) = binding.manager.client() else {
                continue;
            };
            self.sync_wlr_toplevel_binding(display, &client, binding, &collection.snapshots);
        }
        self.wlr_toplevel_managers = managers;
    }

    fn remove_wlr_toplevel_manager(&mut self, manager_id: &ObjectId) {
        self.wlr_toplevel_managers
            .retain(|binding| binding.manager.id() != *manager_id);
    }
}
