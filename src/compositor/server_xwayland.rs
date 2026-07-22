use super::*;
use crate::xwayland::trace::{self, TraceFields};
use crate::xwayland::xwm::XwmAssociationEvent;

impl OwnCompositorServer {
    pub fn insert_xwayland_client(
        &mut self,
        stream: std::os::unix::net::UnixStream,
        generation: XwaylandGeneration,
    ) -> io::Result<XwaylandClientIdentity> {
        let mut handle = self.display.handle();
        let client = handle.insert_client(
            stream,
            Arc::new(TyphonClientData {
                disconnected_clients: self.disconnected_clients.clone(),
                client_pids: self.client_pids.clone(),
            }),
        )?;
        let identity = XwaylandClientIdentity {
            client_id: client.id(),
            generation,
        };
        if let Ok(mut active) = self.xwayland_global_data.active.lock() {
            *active = Some(identity.clone());
        }
        self.state.xwayland.client_identity = Some(identity.clone());
        Ok(identity)
    }

    pub fn revoke_xwayland_generation(&mut self, generation: XwaylandGeneration) {
        let revoke = self
            .xwayland_global_data
            .active
            .lock()
            .ok()
            .is_some_and(|active| {
                active
                    .as_ref()
                    .is_some_and(|identity| identity.generation == generation)
            });
        if revoke {
            if let Ok(mut active) = self.xwayland_global_data.active.lock() {
                *active = None;
            }
            if self
                .state
                .xwayland
                .client_identity
                .as_ref()
                .is_some_and(|identity| identity.generation == generation)
            {
                self.state.xwayland.client_identity = None;
            }
            self.state.clear_xwayland_generation(generation);
        }
    }

    pub fn take_xwayland_shell_bind_events(&mut self) -> Vec<XwaylandClientIdentity> {
        self.xwayland_global_data
            .bind_events
            .lock()
            .map(|mut events| std::mem::take(&mut *events))
            .unwrap_or_default()
    }

    pub fn take_xwayland_client_disconnect_events(&mut self) -> Vec<XwaylandClientIdentity> {
        std::mem::take(&mut self.xwayland_disconnects)
    }

    pub fn take_xwayland_association_events(&mut self) -> Vec<XwaylandAssociationEvent> {
        self.state.take_xwayland_association_events()
    }

    pub fn apply_xwayland_association_event(&mut self, event: XwmAssociationEvent) {
        match event {
            XwmAssociationEvent::Associated {
                generation,
                window,
                surface_id,
            } => {
                if window.generation() != generation {
                    return;
                }
                if !self.state.can_attach_x11_surface(window, surface_id) {
                    return;
                }
                let old_surface_id = self
                    .state
                    .window_id_for_x11_handle(window)
                    .and_then(|window_id| self.state.window(window_id))
                    .and_then(|desktop_window| desktop_window.x11_surface_id);
                let was_focused = self
                    .state
                    .window_id_for_x11_handle(window)
                    .is_some_and(|window_id| self.state.focused_window_id == Some(window_id));
                if let Some(old_surface_id) = old_surface_id
                    && old_surface_id != surface_id
                {
                    self.state.withdraw_xwayland_surface_content(old_surface_id);
                }
                let Ok(replaced_surface_id) = self.state.attach_x11_surface(window, surface_id)
                else {
                    return;
                };
                trace::emit("xwayland_surface_attached", || {
                    TraceFields::new()
                        .field("source", "compositor")
                        .field("xid", window.xid())
                        .field("generation", generation.get())
                        .optional("old_surface_id", replaced_surface_id)
                        .field("surface_id", surface_id)
                        .field("replacement", replaced_surface_id.is_some())
                });
                let _ = self
                    .state
                    .adopt_current_xwayland_surface_content(surface_id);
                if was_focused && let Some(surface) = self.state.surface_resource_by_id(surface_id)
                {
                    self.state.focus_surface(surface);
                }
                self.state.refresh_pointer_focus_at_last_position();
            }
            XwmAssociationEvent::Removed {
                generation,
                window,
                surface_id,
            } => {
                if window.generation() != generation
                    || self.state.window_id_for_surface(surface_id).is_none()
                {
                    return;
                }
                self.state.withdraw_xwayland_surface_content(surface_id);
                self.state.detach_x11_surface(surface_id);
                self.state.refresh_pointer_focus_at_last_position();
                trace::emit("xwayland_surface_detached", || {
                    TraceFields::new()
                        .field("source", "compositor")
                        .field("xid", window.xid())
                        .field("generation", generation.get())
                        .field("surface_id", surface_id)
                        .field("replacement", false)
                });
            }
        }
    }

    pub fn take_xwayland_buffer_ready_events(
        &mut self,
    ) -> Vec<crate::compositor::XwaylandSurfaceCommitObserved> {
        self.state.take_xwayland_buffer_ready_events()
    }

    pub fn take_xwayland_buffer_level_events(&mut self) -> Vec<(XwaylandGeneration, u32)> {
        self.state.take_xwayland_buffer_level_events()
    }

    pub(crate) fn xwayland_resize_commit_floor(
        &self,
        handle: X11WindowHandle,
    ) -> Option<(
        std::num::NonZeroU64,
        crate::compositor::SurfaceCommitSequence,
    )> {
        self.state.xwayland_resize_commit_floor(handle)
    }

    #[cfg(test)]
    pub(crate) fn current_surface_buffer_id(&self, surface_id: u32) -> Option<BufferId> {
        self.state
            .current_surface_buffers
            .get(&surface_id)
            .map(|pending| pending.data.buffer_id())
    }

    pub(super) fn remove_x11_desktop_window(&mut self, handle: X11WindowHandle) -> bool {
        let Some(window_id) = self.state.window_id_for_x11_handle(handle) else {
            return false;
        };
        let was_focused = self.state.focused_window_id == Some(window_id);
        let parent_id = self
            .state
            .window(window_id)
            .and_then(|window| window.relationships.transient_for);
        let root_surface_id = self
            .state
            .window(window_id)
            .map(|window| window.root_surface_id);
        if let Some(root_surface_id) = root_surface_id {
            let _ = self
                .state
                .withdraw_xwayland_surface_content(root_surface_id);
        }
        let removed = self.state.remove_desktop_window(window_id).is_some();
        if removed && was_focused {
            self.state.focused_window_id = None;
            self.state.focused_surface = None;
            self.state.clear_keyboard_focus();
            if let Some(parent_id) = parent_id {
                if !self.state.focus_desktop_window(parent_id) {
                    let _ = self.state.focus_topmost_renderable_toplevel();
                }
            } else {
                let _ = self.state.focus_topmost_renderable_toplevel();
            }
        }
        removed
    }

    pub(super) fn sync_xwayland_client_lists(&self) -> XwmCommand {
        let (client_list, stacking) = self.state.x11_client_lists();
        XwmCommand::SyncClientLists {
            client_list,
            stacking,
        }
    }
}
