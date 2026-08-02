use super::{
    OwnCompositorServer, WindowInteractionButtonRelease, WindowInteractionReleaseContext,
    WindowInteractionReleaseDebugRecord, WindowInteractionReleaseMetrics,
};

impl OwnCompositorServer {
    pub fn pointer_ownership_is_clear(&self) -> bool {
        self.state.held_pointer_buttons.is_empty()
            && self.state.last_pointer_press.is_none()
            && self.state.implicit_pointer_grab.is_none()
    }

    pub fn window_interaction_release_metrics(&self) -> WindowInteractionReleaseMetrics {
        self.state.window_interaction_release_metrics()
    }

    pub fn window_interaction_release_debug_records(
        &self,
    ) -> Vec<WindowInteractionReleaseDebugRecord> {
        self.state.window_interaction_release_debug_records()
    }

    pub fn end_window_interaction_for_button(
        &mut self,
        button: u32,
    ) -> WindowInteractionButtonRelease {
        let ended = self.state.end_window_interaction_for_button(button);
        let _ = self.display.flush_clients();
        ended
    }

    pub fn send_client_owned_trigger_release(
        &mut self,
        context: WindowInteractionReleaseContext,
    ) -> bool {
        let forwarded = self.state.send_client_owned_trigger_release(context);
        let _ = self.display.flush_clients();
        forwarded
    }
}
