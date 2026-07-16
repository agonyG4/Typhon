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
        self.insert_desktop_window(DesktopWindow::new_x11(window_id, snapshot))
            .map_err(|_| X11WindowAdmissionError::DuplicateRootSurface)?;
        Ok(window_id)
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

    pub(in crate::compositor) fn raise_window_id(&mut self, id: WindowId) -> bool {
        if !self.desktop_windows.contains_key(&id) {
            return false;
        }
        self.window_stacking.retain(|window_id| *window_id != id);
        self.window_stacking.push(id);
        true
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
