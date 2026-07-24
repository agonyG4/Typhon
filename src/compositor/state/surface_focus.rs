use super::*;

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
        let changed = !self
            .focused_surface
            .as_ref()
            .is_some_and(|current| same_surface_resource(current, &surface));
        if changed {
            self.focus_generation = self.focus_generation.wrapping_add(1);
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
}

pub(in crate::compositor) fn focus_debug_log(message: impl FnOnce() -> String) {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ENABLED.get_or_init(|| std::env::var_os("OBLIVION_ONE_FOCUS_DEBUG").is_some()) {
        eprintln!("oblivion-one focus: {}", message());
    }
}
