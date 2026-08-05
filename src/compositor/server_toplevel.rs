use super::*;

impl CompositorState {
    pub(crate) fn reconcile_astrea_toplevels(
        &mut self,
        display: &DisplayHandle,
    ) -> AstreaToplevelPublicationSummary {
        let collection = self.collect_astrea_toplevels();
        self.astrea_toplevel_publisher
            .reconcile(display, collection)
    }

    pub(in crate::compositor) fn remove_astrea_toplevel_client(&mut self, client_id: &ClientId) {
        self.astrea_toplevel_publisher.remove_client(client_id);
    }
}

impl OwnCompositorServer {
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

    pub fn toggle_fullscreen_focused_window(&mut self) -> bool {
        let changed = self.state.toggle_fullscreen_focused_window();
        self.publish_astrea_toplevel_updates();
        let _ = self.display.flush_clients();
        changed
    }
}
