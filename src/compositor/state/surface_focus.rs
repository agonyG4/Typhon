use super::*;

#[expect(
    dead_code,
    reason = "approved focus policy reasons are consumed by later input paths"
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WindowFocusReason {
    PointerEnter,
    PointerPress,
    ShellActivation,
    KeyboardNavigation,
    Restore,
}

impl WindowFocusReason {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::PointerEnter => "pointer-enter",
            Self::PointerPress => "pointer-press",
            Self::ShellActivation => "shell-activation",
            Self::KeyboardNavigation => "keyboard-navigation",
            Self::Restore => "restore",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WindowActivationOutcome {
    Accepted,
    NoChange,
    Unavailable,
}

impl CompositorState {
    pub(in crate::compositor) fn focus_surface(&mut self, surface: wl_surface::WlSurface) {
        self.set_desktop_focus(surface, "focus");
    }

    pub(in crate::compositor) fn set_desktop_focus(
        &mut self,
        surface: wl_surface::WlSurface,
        reason: &'static str,
    ) {
        let old_surface_id = self.focused_surface.as_ref().map(compositor_surface_id);
        let new_surface_id = compositor_surface_id(&surface);
        let old_window_id = self.focused_window_id;
        let new_window_id =
            self.window_id_for_surface(self.root_surface_id_for_surface(new_surface_id));
        let desktop_window_changed = old_window_id != new_window_id;
        let changed = !self
            .focused_surface
            .as_ref()
            .is_some_and(|current| same_surface_resource(current, &surface));
        if changed {
            self.focus_generation = advance_nonzero_serial(self.focus_generation);
            pointer_debug_log(format!(
                "focus change reason={} old={:?} new={}",
                reason, old_surface_id, new_surface_id
            ));
            focus_debug_log(|| {
                format!("focus_enter reason={reason} old={old_surface_id:?} new={new_surface_id}")
            });
        }
        self.focused_surface = Some(surface.clone());
        self.focused_window_id = self.update_desktop_focus_window(new_surface_id, changed);
        let focus_generation = self.focus_generation;
        if desktop_window_changed
            && let Some(window_id) = self.focused_window_id
            && self.astrea_toplevel_snapshot(window_id).is_some()
            && let Some(window) = self.window_mut(window_id)
        {
            window.last_focus_serial = focus_generation;
        }
        if desktop_window_changed {
            if let Some(window_id) = old_window_id {
                self.mark_astrea_toplevel_dirty(window_id);
            }
            if let Some(window_id) = self.focused_window_id {
                self.mark_astrea_toplevel_dirty(window_id);
            }
        }
        self.ensure_keyboard_focus(&surface);
        crate::xwayland::trace::emit("focus_wayland_keyboard", || {
            crate::xwayland::trace::TraceFields::new()
                .field("source", "compositor")
                .field("surface_id", new_surface_id)
                .field("focus_generation", self.focus_generation)
                .field("changed", changed)
        });
        self.apply_pending_pointer_constraint_state_for_surface(new_surface_id);
        if !self
            .layer_surfaces
            .contains_key(&self.root_surface_id_for_surface(new_surface_id))
        {
            self.last_application_keyboard_focus = Some(surface);
        }
    }

    pub(in crate::compositor) fn focused_client_id(&self) -> Option<ClientId> {
        self.focused_surface
            .as_ref()
            .and_then(Resource::client)
            .map(|client| client.id())
    }

    pub(in crate::compositor) fn client_has_focus(&self, client_id: &ClientId) -> bool {
        self.focused_client_id()
            .as_ref()
            .is_some_and(|focused_client_id| focused_client_id == client_id)
    }

    pub(in crate::compositor) fn assign_focus_serial_if_needed(&mut self, window_id: WindowId) {
        if self.focused_window_id != Some(window_id)
            || self.astrea_toplevel_snapshot(window_id).is_none()
        {
            return;
        }
        let focus_generation = self.focus_generation;
        if let Some(window) = self.window_mut(window_id)
            && window.last_focus_serial == 0
        {
            window.last_focus_serial = focus_generation;
        }
    }
}

pub(crate) const fn advance_nonzero_serial(value: u64) -> u64 {
    let next = value.wrapping_add(1);
    if next == 0 { 1 } else { next }
}

pub(in crate::compositor) fn focus_debug_log(message: impl FnOnce() -> String) {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ENABLED.get_or_init(|| std::env::var_os("OBLIVION_ONE_FOCUS_DEBUG").is_some()) {
        eprintln!("oblivion-one focus: {}", message());
    }
}

#[cfg(test)]
mod tests {
    use super::advance_nonzero_serial;

    #[test]
    fn focus_serial_never_uses_zero_when_wrapping() {
        assert_eq!(advance_nonzero_serial(0), 1);
        assert_eq!(advance_nonzero_serial(u64::MAX - 1), u64::MAX);
        assert_eq!(advance_nonzero_serial(u64::MAX), 1);
    }
}
