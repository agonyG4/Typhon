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
        if self
            .window_by_root_surface
            .contains_key(&window.root_surface_id)
        {
            return Err(DesktopWindowError::DuplicateRootSurface);
        }
        self.window_by_root_surface
            .insert(window.root_surface_id, window.id);
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
        self.window_stacking.retain(|window_id| *window_id != id);
        Some(window)
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
