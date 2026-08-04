use super::OwnCompositorServer;
use crate::compositor::window_state::ToplevelMode;
use crate::compositor::{WindowBackend, WindowId};
use crate::control_snapshots::{
    ControlWindowId, GeometrySnapshot, WindowKindSnapshot, WindowListSnapshot, WindowSnapshot,
    bounded_window_list,
};

impl OwnCompositorServer {
    pub fn control_window_snapshot(&self, id: WindowId) -> Option<WindowSnapshot> {
        let window = self.state.window(id)?;
        let (kind, x11) = match window.backend {
            WindowBackend::Xdg(_) => (WindowKindSnapshot::XdgToplevel, false),
            WindowBackend::X11(_) => (WindowKindSnapshot::X11, true),
        };
        let geometry = self
            .state
            .current_root_window_geometry(window.root_surface_id)
            .map(|geometry| GeometrySnapshot {
                x: geometry.placement.local_x,
                y: geometry.placement.local_y,
                width: geometry.width,
                height: geometry.height,
            });
        let mode = window.state.mode();
        Some(WindowSnapshot {
            id: ControlWindowId(window.id.get()),
            app_id: window.metadata.app_id.as_deref().map(|app_id| {
                crate::control_snapshots::truncate_utf8(
                    app_id,
                    crate::control_snapshots::MAX_CONTROL_NAME_BYTES,
                )
            }),
            title: crate::control_snapshots::truncate_utf8(
                window.metadata.title.as_deref().unwrap_or(""),
                crate::control_snapshots::MAX_CONTROL_TITLE_BYTES,
            ),
            pid: window.metadata.pid,
            kind,
            mapped: geometry.is_some(),
            active: self.state.focused_window_id == Some(id),
            minimized: window.state.is_minimized(),
            maximized: matches!(mode, ToplevelMode::Maximized),
            fullscreen: matches!(mode, ToplevelMode::Fullscreen),
            urgent: None,
            skip_taskbar: x11 && window.is_auxiliary_x11_role(),
            workspace: None,
            output: Some("oblivion-1".to_string()),
            geometry,
            focus_serial: None,
        })
    }

    pub fn control_window_list_snapshot(&self) -> Result<WindowListSnapshot, serde_json::Error> {
        let total = u32::try_from(self.state.desktop_windows.len()).unwrap_or(u32::MAX);
        bounded_window_list(
            total,
            self.state
                .window_stacking
                .iter()
                .rev()
                .filter_map(|id| self.control_window_snapshot(*id)),
        )
    }

    pub fn control_window_counts(&self) -> (u32, u32, u32) {
        let total = u32::try_from(self.state.desktop_windows.len()).unwrap_or(u32::MAX);
        let mapped = self
            .state
            .desktop_windows
            .values()
            .filter(|window| {
                self.state
                    .current_root_window_geometry(window.root_surface_id)
                    .is_some()
            })
            .count();
        let minimized = self
            .state
            .desktop_windows
            .values()
            .filter(|window| window.state.is_minimized())
            .count();
        (
            total,
            u32::try_from(mapped).unwrap_or(u32::MAX),
            u32::try_from(minimized).unwrap_or(u32::MAX),
        )
    }

    pub fn control_active_window_snapshot(&self) -> Option<WindowSnapshot> {
        self.state
            .focused_window_id
            .and_then(|id| self.control_window_snapshot(id))
    }
}
