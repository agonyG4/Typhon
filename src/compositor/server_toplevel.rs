use std::collections::BTreeMap;

use super::*;

impl CompositorState {
    pub(crate) fn has_pending_astrea_toplevel_publication(&self) -> bool {
        self.astrea_toplevel_publisher.has_pending_publication()
    }

    pub(crate) fn reconcile_astrea_toplevels(
        &mut self,
        display: &DisplayHandle,
    ) -> AstreaToplevelPublicationSummary {
        let needs_full = self.astrea_toplevel_publisher.needs_full_reconciliation();
        let dirty_ids = self.astrea_toplevel_publisher.dirty_window_ids();
        let dirty_snapshots = dirty_ids
            .into_iter()
            .map(|window_id| (window_id, self.astrea_toplevel_snapshot(window_id)))
            .collect();
        let collection = if needs_full {
            match self.collect_astrea_toplevels() {
                Ok(collection) => Some(collection),
                Err(()) => {
                    if self.astrea_toplevel_publisher.has_active_transaction() {
                        // An uncollectable follow-up target must not strand the
                        // already-frozen transaction. Complete the committed
                        // publication and fail closed for the follow-up.
                        return self.astrea_toplevel_publisher.reconcile(
                            display,
                            None,
                            BTreeMap::new(),
                        );
                    }
                    self.astrea_toplevel_publisher.fail_all_managers();
                    self.astrea_toplevel_publisher
                        .clear_failed_collection_state();
                    return AstreaToplevelPublicationSummary {
                        revision: self.astrea_toplevel_publisher.revision,
                        manager_count: self.astrea_toplevel_publisher.manager_count(),
                        ..AstreaToplevelPublicationSummary::default()
                    };
                }
            }
        } else {
            None
        };
        self.astrea_toplevel_publisher
            .reconcile(display, collection, dirty_snapshots)
    }

    pub(in crate::compositor) fn mark_astrea_toplevel_dirty(&mut self, window_id: WindowId) {
        self.astrea_toplevel_publisher.mark_window_dirty(window_id);
    }

    pub(in crate::compositor) fn mark_astrea_toplevel_removed(&mut self, window_id: WindowId) {
        self.astrea_toplevel_publisher
            .mark_window_removed(window_id);
    }

    pub(in crate::compositor) fn mark_astrea_toplevel_structure_dirty(&mut self) {
        self.astrea_toplevel_publisher.mark_structure_dirty();
    }

    pub(in crate::compositor) fn remove_astrea_toplevel_client(&mut self, client_id: &ClientId) {
        self.astrea_shell_authenticated_clients.remove(client_id);
        self.astrea_toplevel_authorized_clients.remove(client_id);
        self.astrea_toplevel_publisher.remove_client(client_id);
    }
}

impl OwnCompositorServer {
    pub fn has_pending_astrea_toplevel_publication(&self) -> bool {
        self.state.has_pending_astrea_toplevel_publication()
    }

    pub(crate) fn publish_astrea_toplevel_updates(&mut self) -> AstreaToplevelPublicationSummary {
        let display = self.display.handle();
        self.state.reconcile_astrea_toplevels(&display)
    }

    pub fn send_keyboard_key(&mut self, key: u32, pressed: bool) {
        self.state.send_keyboard_key(key, pressed);
        self.publish_astrea_toplevel_updates();
        let _ = self.display.flush_clients();
    }

    pub fn send_pointer_motion(&mut self, x: f64, y: f64) {
        self.state.send_pointer_motion(x, y);
        self.publish_astrea_toplevel_updates();
        let _ = self.display.flush_clients();
    }

    pub fn update_pointer_position_without_client_dispatch(&mut self, x: f64, y: f64) -> bool {
        self.state
            .update_pointer_position_without_client_dispatch(x, y)
    }

    pub fn send_pointer_motion_sample(&mut self, sample: PointerMotionSample) {
        self.state.send_pointer_motion_sample(sample);
        self.publish_astrea_toplevel_updates();
        let _ = self.display.flush_clients();
    }

    pub fn send_window_interaction_pointer_motion(
        &mut self,
        timestamp_usec: u64,
        x: f64,
        y: f64,
    ) -> usize {
        let dispatched = self
            .state
            .send_window_interaction_pointer_motion(timestamp_usec, x, y);
        self.publish_astrea_toplevel_updates();
        let _ = self.display.flush_clients();
        dispatched
    }

    pub fn send_pointer_button(&mut self, button: u32, pressed: bool) {
        self.state.send_pointer_button(button, pressed);
        self.publish_astrea_toplevel_updates();
        let _ = self.display.flush_clients();
    }

    pub fn minimize_focused_window(&mut self) -> bool {
        let minimized = self.state.minimize_focused_window();
        self.publish_astrea_toplevel_updates();
        let _ = self.display.flush_clients();
        minimized
    }

    pub fn close_focused_window(&mut self) -> bool {
        let closed = self.state.close_focused_window();
        self.publish_astrea_toplevel_updates();
        let _ = self.display.flush_clients();
        closed
    }

    pub fn restore_next_minimized_window(&mut self) -> bool {
        let restored = self.state.restore_next_minimized_window();
        self.publish_astrea_toplevel_updates();
        let _ = self.display.flush_clients();
        restored
    }

    pub fn activate_window(&mut self, surface_id: u32) -> bool {
        let activated = self.state.activate_root_window(surface_id);
        self.publish_astrea_toplevel_updates();
        let _ = self.display.flush_clients();
        activated
    }

    pub fn toggle_maximize_focused_window(&mut self) -> bool {
        let changed = self.state.toggle_maximize_focused_window();
        self.publish_astrea_toplevel_updates();
        let _ = self.display.flush_clients();
        changed
    }

    pub fn decoration_theme_status(&self) -> (String, String, u32, u64, String, Option<String>) {
        self.state.decoration_theme_status()
    }

    pub fn decoration_theme_list(&self) -> Vec<String> {
        self.state.available_decoration_themes()
    }

    pub fn set_decoration_theme(&mut self, name: &str) -> Result<(), String> {
        let result = self.state.set_decoration_theme(name);
        let _ = self.display.flush_clients();
        result
    }

    pub fn reload_decoration_theme(&mut self) -> Result<(), String> {
        let result = self.state.reload_decoration_theme();
        let _ = self.display.flush_clients();
        result
    }

    pub fn toggle_fullscreen_focused_window(&mut self) -> bool {
        let changed = self.state.toggle_fullscreen_focused_window();
        self.publish_astrea_toplevel_updates();
        let _ = self.display.flush_clients();
        changed
    }
}
