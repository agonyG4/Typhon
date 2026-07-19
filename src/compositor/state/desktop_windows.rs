use std::num::NonZeroU64;

use super::*;

impl CompositorState {
    pub(in crate::compositor) fn toplevel_window_id(&self, surface_id: u32) -> Option<WindowId> {
        self.toplevel_surfaces
            .get(&surface_id)
            .map(|toplevel| toplevel.window_id)
    }

    pub(in crate::compositor) fn toplevel_window_state(
        &self,
        surface_id: u32,
    ) -> Option<&WindowState> {
        let window_id = self.toplevel_window_id(surface_id)?;
        self.window(window_id).map(|window| &window.state)
    }

    pub(in crate::compositor) fn toplevel_window_state_mut(
        &mut self,
        surface_id: u32,
    ) -> Option<&mut WindowState> {
        let window_id = self.toplevel_window_id(surface_id)?;
        self.window_mut(window_id).map(|window| &mut window.state)
    }

    pub(in crate::compositor) fn toplevel_window_constraints(
        &self,
        surface_id: u32,
    ) -> WindowConstraints {
        self.toplevel_window_id(surface_id)
            .and_then(|window_id| self.window(window_id))
            .map(|window| window.constraints)
            .unwrap_or_default()
    }

    pub(in crate::compositor) fn allocate_window_id(&mut self) -> io::Result<WindowId> {
        let value = NonZeroU64::new(self.next_window_id)
            .ok_or_else(|| io::Error::other(DesktopWindowError::WindowIdExhausted))?;
        self.next_window_id = self.next_window_id.checked_add(1).unwrap_or(0);
        Ok(WindowId::new(value))
    }

    pub(in crate::compositor) fn insert_desktop_window(
        &mut self,
        window: DesktopWindow,
    ) -> Result<(), DesktopWindowError> {
        if self.desktop_windows.contains_key(&window.id) {
            return Err(DesktopWindowError::DuplicateWindowId);
        }
        if let WindowBackend::X11(handle) = window.backend
            && self.window_by_x11_handle.contains_key(&handle)
        {
            return Err(DesktopWindowError::DuplicateWindowId);
        }
        if self
            .window_by_root_surface
            .contains_key(&window.root_surface_id)
        {
            return Err(DesktopWindowError::DuplicateRootSurface);
        }
        self.window_by_root_surface
            .insert(window.root_surface_id, window.id);
        if let WindowBackend::X11(handle) = window.backend {
            self.window_by_x11_handle.insert(handle, window.id);
        }
        self.window_stacking.push(window.id);
        self.desktop_windows.insert(window.id, window);
        Ok(())
    }

    pub(in crate::compositor) fn remove_desktop_window(
        &mut self,
        id: WindowId,
    ) -> Option<DesktopWindow> {
        let window = self.desktop_windows.remove(&id)?;
        self.window_by_root_surface.remove(&window.root_surface_id);
        if let WindowBackend::X11(handle) = window.backend {
            self.window_by_x11_handle.remove(&handle);
        }
        self.window_stacking.retain(|window_id| *window_id != id);
        Some(window)
    }

    pub(in crate::compositor) fn insert_x11_window(
        &mut self,
        snapshot: crate::xwayland::xwm::X11WindowSnapshot,
    ) -> Result<WindowId, X11WindowAdmissionError> {
        let generation = snapshot.handle.generation();
        if self
            .xwayland
            .client_identity
            .as_ref()
            .is_none_or(|identity| identity.generation != generation)
        {
            return Err(X11WindowAdmissionError::StaleGeneration);
        }
        if !matches!(
            self.surface_role(snapshot.surface_id),
            SurfaceRole::Xwayland
        ) {
            return Err(X11WindowAdmissionError::SurfaceNotXwayland);
        }
        if self
            .xwayland
            .surface_states
            .get(&snapshot.surface_id)
            .is_none_or(|state| state.generation != generation || state.committed_serial.is_none())
        {
            return Err(X11WindowAdmissionError::SurfaceNotAssociated);
        }
        if self.window_by_x11_handle.contains_key(&snapshot.handle) {
            return Err(X11WindowAdmissionError::DuplicateX11Window);
        }
        if self
            .window_by_root_surface
            .contains_key(&snapshot.surface_id)
        {
            return Err(X11WindowAdmissionError::DuplicateRootSurface);
        }
        let window_id = self
            .allocate_window_id()
            .map_err(|_| X11WindowAdmissionError::WindowIdExhausted)?;
        let handle = snapshot.handle;
        let geometry = snapshot.geometry;
        let requested_state = snapshot.state;
        let transient_for = snapshot.transient_for;
        self.insert_desktop_window(DesktopWindow::new_x11(window_id, snapshot))
            .map_err(|_| X11WindowAdmissionError::DuplicateRootSurface)?;
        let _ = self.set_x11_geometry(handle, geometry);
        if let Some(parent_handle) = transient_for
            && let Some(parent_id) = self.window_id_for_x11_handle(parent_handle)
            && let Some(window) = self.window_mut(window_id)
        {
            window.relationships.transient_for = Some(parent_id);
        }
        self.apply_initial_x11_state(handle, requested_state, geometry);
        Ok(window_id)
    }

    pub(in crate::compositor) fn apply_initial_x11_state(
        &mut self,
        handle: X11WindowHandle,
        state: crate::xwayland::xwm::X11PublishedState,
        geometry: crate::xwayland::xwm::X11Geometry,
    ) -> bool {
        let Some(window_id) = self.window_id_for_x11_handle(handle) else {
            return false;
        };
        let root_surface_id = self.window(window_id).map(|window| window.root_surface_id);
        let Some(root_surface_id) = root_surface_id else {
            return false;
        };
        let restore_geometry = WindowGeometry::new(
            SurfacePlacement::absolute_root_at(geometry.x, geometry.y),
            geometry.width.max(1),
            geometry.height.max(1),
        );
        let mode = if state.fullscreen {
            ToplevelMode::Fullscreen
        } else if state.maximized {
            ToplevelMode::Maximized
        } else {
            ToplevelMode::Floating
        };
        if let Some(window) = self.window_mut(window_id) {
            if mode != ToplevelMode::Floating {
                window.state.capture_restore_geometry(restore_geometry);
            }
            window.state.set_mode(mode);
        }
        let target_geometry = self.window_geometry_for_mode(mode);
        self.set_surface_placement_with_cause(
            root_surface_id,
            if mode == ToplevelMode::Floating {
                restore_geometry.placement
            } else {
                target_geometry.placement
            },
            RenderGenerationCause::WindowMode,
        );
        if state.hidden {
            if !self.minimize_desktop_window(window_id)
                && let Some(window) = self.window_mut(window_id)
            {
                window.state.mark_minimized_without_surfaces();
            }
        } else if self
            .window(window_id)
            .is_some_and(|window| window.state.is_minimized())
        {
            self.restore_minimized_desktop_window(window_id);
        }
        if mode == ToplevelMode::Fullscreen && !state.hidden {
            self.set_fullscreen_presentation_owner(root_surface_id);
        } else {
            self.clear_fullscreen_presentation_owner(root_surface_id);
        }
        if mode != ToplevelMode::Floating {
            self.queue_backend_configure(window_id, target_geometry, mode, false);
        }
        self.queue_backend_state(window_id);
        true
    }

    pub(in crate::compositor) fn window_id_for_x11_handle(
        &self,
        handle: X11WindowHandle,
    ) -> Option<WindowId> {
        self.window_by_x11_handle.get(&handle).copied()
    }

    pub(in crate::compositor) fn apply_x11_metadata_delta(
        &mut self,
        handle: X11WindowHandle,
        delta: crate::xwayland::xwm::X11MetadataDelta,
    ) -> bool {
        let Some(window_id) = self.window_id_for_x11_handle(handle) else {
            return false;
        };
        let Some(window) = self.window_mut(window_id) else {
            return false;
        };
        match delta {
            crate::xwayland::xwm::X11MetadataDelta::Title(title) => {
                window.metadata.title = title;
            }
            crate::xwayland::xwm::X11MetadataDelta::AppId(app_id) => {
                window.metadata.app_id = app_id;
            }
            crate::xwayland::xwm::X11MetadataDelta::Pid(pid) => {
                window.metadata.pid = pid;
            }
            crate::xwayland::xwm::X11MetadataDelta::Constraints(constraints) => {
                window.constraints = constraints;
            }
            crate::xwayland::xwm::X11MetadataDelta::TransientFor(_) => {}
            crate::xwayland::xwm::X11MetadataDelta::Protocols { .. } => {}
        }
        true
    }

    pub(in crate::compositor) fn filter_x11_geometry(
        &self,
        handle: X11WindowHandle,
        requested: crate::xwayland::xwm::X11Geometry,
    ) -> Option<crate::xwayland::xwm::X11Geometry> {
        let window_id = self.window_id_for_x11_handle(handle)?;
        let window = self.window(window_id)?;
        let width = requested
            .width
            .max(1)
            .max(window.constraints.min_width.unwrap_or(1))
            .min(window.constraints.max_width.unwrap_or(u32::MAX));
        let height = requested
            .height
            .max(1)
            .max(window.constraints.min_height.unwrap_or(1))
            .min(window.constraints.max_height.unwrap_or(u32::MAX));
        Some(crate::xwayland::xwm::X11Geometry {
            width,
            height,
            ..requested
        })
    }

    pub(in crate::compositor) fn set_x11_geometry(
        &mut self,
        handle: X11WindowHandle,
        geometry: crate::xwayland::xwm::X11Geometry,
    ) -> bool {
        let Some(window_id) = self.window_id_for_x11_handle(handle) else {
            return false;
        };
        let Some(filtered) = self.filter_x11_geometry(handle, geometry) else {
            return false;
        };
        let root_surface_id = self.window(window_id).map(|window| window.root_surface_id);
        let Some(root_surface_id) = root_surface_id else {
            return false;
        };
        self.set_surface_placement_with_cause(
            root_surface_id,
            SurfacePlacement::absolute_root_at(filtered.x, filtered.y),
            RenderGenerationCause::WindowMove,
        );
        true
    }

    pub(in crate::compositor) fn apply_x11_published_state(
        &mut self,
        handle: X11WindowHandle,
        state: crate::xwayland::xwm::X11PublishedState,
    ) -> bool {
        let Some(window_id) = self.window_id_for_x11_handle(handle) else {
            return false;
        };
        let mode = if state.fullscreen {
            ToplevelMode::Fullscreen
        } else if state.maximized {
            ToplevelMode::Maximized
        } else {
            ToplevelMode::Floating
        };
        let Some(root_surface_id) = self.window(window_id).map(|window| window.root_surface_id)
        else {
            return false;
        };
        let minimized = self
            .window(window_id)
            .is_some_and(|window| window.state.is_minimized());
        if let Some(window) = self.window_mut(window_id) {
            window.state.set_mode(mode);
        }
        if state.hidden && !minimized {
            let _ = self.minimize_desktop_window(window_id);
        } else if !state.hidden && minimized {
            let _ = self.restore_minimized_desktop_window(window_id);
        }
        let placement = match mode {
            ToplevelMode::Fullscreen => self.fullscreen_window_geometry().placement,
            ToplevelMode::Maximized => self.maximized_window_geometry().placement,
            ToplevelMode::Floating => self.surface_placement(root_surface_id),
        };
        self.set_surface_placement_with_cause(
            root_surface_id,
            placement,
            RenderGenerationCause::WindowMode,
        );
        true
    }

    pub(in crate::compositor) fn apply_x11_state_request(
        &mut self,
        handle: X11WindowHandle,
        request: crate::xwayland::xwm::X11StateRequest,
    ) -> Option<crate::xwayland::xwm::X11PublishedState> {
        let window_id = self.window_id_for_x11_handle(handle)?;
        let window = self.window(window_id)?;
        let mut state = crate::xwayland::xwm::X11PublishedState {
            fullscreen: window.state.mode() == ToplevelMode::Fullscreen,
            maximized: window.state.mode() == ToplevelMode::Maximized,
            hidden: window.state.is_minimized(),
            activated: self.focused_window_id == Some(window_id),
        };
        let mut maximized_horizontal = state.maximized;
        let mut maximized_vertical = state.maximized;
        for atom in [request.first, request.second].into_iter().flatten() {
            let value = match atom {
                crate::xwayland::xwm::X11StateAtom::Fullscreen => &mut state.fullscreen,
                crate::xwayland::xwm::X11StateAtom::MaximizedHorizontal => {
                    &mut maximized_horizontal
                }
                crate::xwayland::xwm::X11StateAtom::MaximizedVertical => &mut maximized_vertical,
                crate::xwayland::xwm::X11StateAtom::Hidden => &mut state.hidden,
            };
            *value = crate::xwayland::xwm::ewmh::apply_state_action(*value, request.action);
        }
        state.maximized = crate::xwayland::xwm::ewmh::aggregate_maximize(
            maximized_horizontal,
            maximized_vertical,
        );
        self.apply_x11_published_state(handle, state);
        Some(state)
    }

    pub(in crate::compositor) fn raise_window_id(&mut self, id: WindowId) -> bool {
        if !self.desktop_windows.contains_key(&id) {
            return false;
        }
        self.window_stacking.retain(|window_id| *window_id != id);
        self.window_stacking.push(id);
        true
    }

    pub(in crate::compositor) fn x11_client_lists(
        &self,
    ) -> (
        Vec<crate::xwayland::X11WindowHandle>,
        Vec<crate::xwayland::X11WindowHandle>,
    ) {
        let mut client_list = self
            .desktop_windows
            .values()
            .filter_map(|window| match window.backend {
                WindowBackend::X11(handle) => Some((window.id, handle)),
                WindowBackend::Xdg(_) => None,
            })
            .collect::<Vec<_>>();
        client_list.sort_by_key(|(id, _)| *id);
        let client_list = client_list
            .iter()
            .map(|(_, handle)| *handle)
            .collect::<Vec<_>>();
        let stacking = self
            .window_stacking
            .iter()
            .filter_map(|id| self.window(*id))
            .filter_map(|window| match window.backend {
                WindowBackend::X11(handle) => Some(handle),
                WindowBackend::Xdg(_) => None,
            })
            .collect::<Vec<_>>();
        (client_list, stacking)
    }

    pub(in crate::compositor) fn queue_backend_configure(
        &mut self,
        window_id: WindowId,
        geometry: WindowGeometry,
        mode: ToplevelMode,
        resizing: bool,
    ) {
        self.backend_commands.push(
            crate::compositor::window_backend::WindowBackendCommand::Configure {
                window: window_id,
                geometry,
                mode,
                resizing,
            },
        );
    }

    pub(in crate::compositor) fn queue_backend_activation(
        &mut self,
        window_id: WindowId,
        activated: bool,
    ) {
        if self
            .window(window_id)
            .is_some_and(|window| matches!(window.backend, WindowBackend::X11(_)))
        {
            self.backend_commands.push(
                crate::compositor::window_backend::WindowBackendCommand::SetActivated {
                    window: window_id,
                    activated,
                },
            );
        }
    }

    pub(in crate::compositor) fn update_desktop_focus_window(
        &mut self,
        new_surface_id: u32,
        changed: bool,
    ) -> Option<WindowId> {
        let old_window_id = self.focused_window_id;
        let new_window_id =
            self.window_id_for_surface(self.root_surface_id_for_surface(new_surface_id));
        if changed {
            if let Some(window_id) = old_window_id {
                self.queue_backend_activation(window_id, false);
            }
            if let Some(window_id) = new_window_id {
                self.queue_backend_activation(window_id, true);
            }
        }
        new_window_id
    }

    pub(in crate::compositor) fn queue_backend_state(&mut self, window_id: WindowId) {
        let Some(window) = self.window(window_id) else {
            return;
        };
        if matches!(window.backend, WindowBackend::X11(_)) {
            self.backend_commands.push(
                crate::compositor::window_backend::WindowBackendCommand::PublishState {
                    window: window_id,
                    mode: window.state.mode(),
                    minimized: window.state.is_minimized(),
                    activated: self.focused_window_id == Some(window_id),
                },
            );
        }
    }

    pub(in crate::compositor) fn take_backend_commands(
        &mut self,
    ) -> Vec<crate::compositor::window_backend::WindowBackendCommand> {
        std::mem::take(&mut self.backend_commands)
    }

    pub(in crate::compositor) fn window_id_for_surface(&self, surface_id: u32) -> Option<WindowId> {
        self.window_by_root_surface.get(&surface_id).copied()
    }

    pub(in crate::compositor) fn window(&self, id: WindowId) -> Option<&DesktopWindow> {
        self.desktop_windows.get(&id)
    }

    pub(in crate::compositor) fn window_mut(&mut self, id: WindowId) -> Option<&mut DesktopWindow> {
        self.desktop_windows.get_mut(&id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) enum X11WindowAdmissionError {
    StaleGeneration,
    SurfaceNotXwayland,
    SurfaceNotAssociated,
    DuplicateX11Window,
    DuplicateRootSurface,
    WindowIdExhausted,
}
