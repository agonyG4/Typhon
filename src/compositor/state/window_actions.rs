use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WindowActionOutcome {
    Changed,
    NoChange,
    Unavailable,
}

impl CompositorState {
    pub(in crate::compositor) fn toggle_focused_window_layout(&mut self) -> bool {
        self.toggle_focused_tiled_layout()
    }

    pub(in crate::compositor) fn activate_desktop_window_action(
        &mut self,
        window_id: WindowId,
    ) -> WindowActionOutcome {
        match self.activate_desktop_window(window_id, WindowFocusReason::ShellActivation) {
            WindowActivationOutcome::Changed => WindowActionOutcome::Changed,
            WindowActivationOutcome::NoChange => WindowActionOutcome::NoChange,
            WindowActivationOutcome::Unavailable => WindowActionOutcome::Unavailable,
        }
    }

    pub(in crate::compositor) fn close_desktop_window_outcome(
        &mut self,
        window_id: WindowId,
    ) -> WindowActionOutcome {
        let Some(window) = self.window(window_id).cloned() else {
            return WindowActionOutcome::Unavailable;
        };
        if window.kind != DesktopWindowKind::Managed || !window.is_normal_x11_role() {
            return WindowActionOutcome::Unavailable;
        }
        match window.backend {
            WindowBackend::X11(_) => {
                self.backend_commands.push(
                    crate::compositor::window_backend::WindowBackendCommand::Close {
                        window: window_id,
                    },
                );
                WindowActionOutcome::Changed
            }
            WindowBackend::Xdg(_) => self
                .toplevel_surfaces
                .get(&window.root_surface_id)
                .and_then(|role| role.toplevel.send_event(xdg_toplevel::Event::Close).ok())
                .map_or(WindowActionOutcome::Unavailable, |_| {
                    WindowActionOutcome::Changed
                }),
        }
    }

    pub(in crate::compositor) fn minimize_desktop_window_outcome(
        &mut self,
        window_id: WindowId,
    ) -> WindowActionOutcome {
        let Some(window) = self.window(window_id) else {
            return WindowActionOutcome::Unavailable;
        };
        if window.kind != DesktopWindowKind::Managed || !window.is_normal_x11_role() {
            return WindowActionOutcome::Unavailable;
        }
        if window.state.is_minimized() {
            return WindowActionOutcome::NoChange;
        }
        if self.minimize_desktop_window(window_id) {
            WindowActionOutcome::Changed
        } else {
            WindowActionOutcome::Unavailable
        }
    }

    pub(in crate::compositor) fn restore_minimized_desktop_window_outcome(
        &mut self,
        window_id: WindowId,
    ) -> WindowActionOutcome {
        let Some(window) = self.window(window_id) else {
            return WindowActionOutcome::Unavailable;
        };
        if window.kind != DesktopWindowKind::Managed || !window.is_normal_x11_role() {
            return WindowActionOutcome::Unavailable;
        }
        if !window.state.is_minimized() {
            return WindowActionOutcome::NoChange;
        }
        if self.restore_minimized_desktop_window(window_id) {
            WindowActionOutcome::Changed
        } else {
            WindowActionOutcome::Unavailable
        }
    }
}
