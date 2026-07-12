use super::*;

pub(in crate::compositor) fn coalesce_output_row_rects(rects: Vec<OutputRect>) -> Vec<OutputRect> {
    let mut coalesced: Vec<OutputRect> = Vec::new();
    for rect in rects {
        if let Some(last) = coalesced.last_mut()
            && last.x == rect.x
            && last.width == rect.width
            && (last.y + last.height) == rect.y
        {
            last.height += rect.height;
            continue;
        }
        coalesced.push(rect);
    }
    coalesced
}

#[derive(Debug, Clone)]
pub(crate) struct CursorVisibilityState {
    pub(crate) client_hidden_pointer: Option<wl_pointer::WlPointer>,
    pub(crate) client_cursor_pointer: Option<wl_pointer::WlPointer>,
    pub(crate) lock_hidden_constraint_id: Option<u64>,
    pub(crate) visible: bool,
}

impl Default for CursorVisibilityState {
    fn default() -> Self {
        Self {
            client_hidden_pointer: None,
            client_cursor_pointer: None,
            lock_hidden_constraint_id: None,
            visible: true,
        }
    }
}

impl CursorVisibilityState {
    pub(in crate::compositor) fn desired_visible(&self) -> bool {
        self.client_hidden_pointer.is_none()
            && self.client_cursor_pointer.is_none()
            && self.lock_hidden_constraint_id.is_none()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PointerEnterSerial {
    pub(crate) pointer: wl_pointer::WlPointer,
    pub(crate) surface: wl_surface::WlSurface,
    pub(crate) serial: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct ActiveClientCursor {
    pub(crate) pointer: wl_pointer::WlPointer,
    pub(crate) surface_id: u32,
    pub(crate) hotspot_x: i32,
    pub(crate) hotspot_y: i32,
}

pub(in crate::compositor) fn pointer_debug_log(message: impl AsRef<str>) {
    if std::env::var_os("TYPHON_POINTER_DEBUG").is_some() {
        eprintln!("typhon pointer: {}", message.as_ref());
    }
}

impl RelativeMotionDebugState {
    pub(in crate::compositor) fn note_dispatch(&mut self, message: String) {
        self.dispatch_total = self.dispatch_total.saturating_add(1);
        pointer_debug_log(message);
    }

    pub(in crate::compositor) fn note_drop(&mut self, reason: impl Into<String>) {
        self.pending_drop_count = self.pending_drop_count.saturating_add(1);
        self.pending_drop_reason = Some(reason.into());
        self.flush_drops(false);
    }

    pub(in crate::compositor) fn should_log_route_snapshot(&mut self) -> bool {
        if std::env::var_os("TYPHON_POINTER_DEBUG").is_none() {
            return false;
        }
        let now = Instant::now();
        let should_log = self
            .last_route_snapshot_log
            .is_none_or(|last| now.duration_since(last) >= std::time::Duration::from_millis(500));
        if should_log {
            self.last_route_snapshot_log = Some(now);
        }
        should_log
    }

    pub(in crate::compositor) fn flush_drops(&mut self, force: bool) {
        let Some(reason) = self.pending_drop_reason.take() else {
            return;
        };
        let count = self.pending_drop_count;
        self.pending_drop_count = 0;
        let now = Instant::now();
        let should_log = force
            || self.last_drop_log.is_none_or(|last| {
                now.duration_since(last) >= std::time::Duration::from_millis(500)
            });
        if !should_log {
            self.pending_drop_reason = Some(reason);
            self.pending_drop_count = count;
            return;
        }
        self.last_drop_log = Some(now);
        if count > 1 {
            pointer_debug_log(format!("relative motion drop reason ({count}x): {reason}"));
        } else {
            pointer_debug_log(format!("relative motion drop reason: {reason}"));
        }
    }
}

pub(in crate::compositor) fn wayland_resource_client_label(resource: &impl Resource) -> String {
    resource
        .client()
        .map(|client| format!("{:?}", client.id()))
        .unwrap_or_else(|| "unknown".to_string())
}

impl CompositorState {
    #[allow(clippy::too_many_arguments)]
    pub(in crate::compositor) fn resize_flow_debug_event(
        &self,
        event: &str,
        surface_id: u32,
        interaction_id: Option<WindowInteractionId>,
        serial: Option<u32>,
        sequence: Option<u64>,
        resizing: bool,
        geometry: Option<WindowGeometry>,
    ) {
        let flow = self.resize_configure_flows.get(&surface_id);
        let active_window = self
            .window_interaction_debug_snapshot()
            .filter(|snapshot| snapshot.root_surface_id == surface_id);
        let flow_state = flow.map(|flow| {
            (
                flow.active_interaction_id(),
                flow.outstanding_count(),
                flow.acked_uncaptured_count(),
                flow.captured_count(),
                flow.queued_latest(),
                flow.final_pending(),
            )
        });
        let interaction_id = interaction_id.or_else(|| {
            active_window.map(|snapshot| WindowInteractionId::new(snapshot.interaction_id))
        });
        resize_debug_log(|| {
            format!(
                "event={event} interaction_id={} resize_interaction_id={} root={} serial={} sequence={} resizing={} geometry={geometry:?} outstanding_count={} acked_uncaptured_count={} queued_latest={:?} final_pending={:?} captured_count={} preview_active={}",
                interaction_id.map_or_else(|| "none".to_string(), |id| id.get().to_string()),
                flow_state
                    .and_then(|state| state.0)
                    .map_or_else(|| "none".to_string(), |id| id.get().to_string()),
                surface_id,
                serial.map_or_else(|| "none".to_string(), |serial| serial.to_string()),
                sequence.map_or_else(|| "none".to_string(), |sequence| sequence.to_string()),
                resizing,
                flow_state.map_or(0, |state| state.1),
                flow_state.map_or(0, |state| state.2),
                flow_state.and_then(|state| state.4),
                flow_state.and_then(|state| state.5),
                flow_state.map_or(0, |state| state.3),
                self.active_toplevel_resizes.contains_key(&surface_id),
            )
        });
    }
}
