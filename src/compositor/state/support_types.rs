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

#[derive(Debug, Clone)]
pub(crate) enum ClientCursorChoice {
    Hidden {
        pointer: wl_pointer::WlPointer,
    },
    Surface(ActiveClientCursor),
    Shape {
        pointer: wl_pointer::WlPointer,
        shape: u32,
    },
}

impl ClientCursorChoice {
    pub(crate) fn pointer(&self) -> &wl_pointer::WlPointer {
        match self {
            Self::Hidden { pointer } | Self::Shape { pointer, .. } => pointer,
            Self::Surface(active) => &active.pointer,
        }
    }

    pub(crate) fn surface(&self) -> Option<&ActiveClientCursor> {
        match self {
            Self::Surface(active) => Some(active),
            Self::Hidden { .. } | Self::Shape { .. } => None,
        }
    }

    pub(crate) fn is_hidden(&self) -> bool {
        matches!(self, Self::Hidden { .. })
    }

    pub(crate) fn is_same_as(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Hidden { pointer: left }, Self::Hidden { pointer: right }) => {
                same_wayland_resource(left, right)
            }
            (Self::Surface(left), Self::Surface(right)) => {
                same_wayland_resource(&left.pointer, &right.pointer)
                    && left.surface_id == right.surface_id
                    && left.hotspot_x == right.hotspot_x
                    && left.hotspot_y == right.hotspot_y
            }
            (
                Self::Shape {
                    pointer: left,
                    shape: left_shape,
                },
                Self::Shape {
                    pointer: right,
                    shape: right_shape,
                },
            ) => same_wayland_resource(left, right) && left_shape == right_shape,
            _ => false,
        }
    }
}

pub(in crate::compositor) fn pointer_debug_log(message: impl AsRef<str>) {
    crate::pointer_debug::log(message);
}

pub(in crate::compositor) fn pointer_debug_enabled() -> bool {
    crate::pointer_debug::enabled()
}

#[cfg(test)]
fn pointer_debug_message<T>(enabled: bool, message: impl FnOnce() -> T) -> Option<T> {
    enabled.then(message)
}

pub(in crate::compositor) fn pointer_debug_log_lazy(message: impl FnOnce() -> String) {
    crate::pointer_debug::log_lazy(message);
}

#[allow(clippy::too_many_arguments)]
pub(in crate::compositor) fn cursor_geometry_debug_message(
    event: &str,
    client: &str,
    surface_id: u32,
    source: &str,
    buffer_width: u32,
    buffer_height: u32,
    buffer_scale: u32,
    transform: wl_output::Transform,
    logical_width: u32,
    logical_height: u32,
    hotspot: Option<(i32, i32)>,
    viewport_destination: Option<BufferSize>,
    output_scale: f64,
) -> String {
    format!(
        "cursor surface commit event={event} client={client} surface={surface_id} source={source} buffer={buffer_width}x{buffer_height} buffer_scale={buffer_scale} transform={transform:?} logical={logical_width}x{logical_height} hotspot={hotspot:?} viewport_destination={viewport_destination:?} output_scale={output_scale:.3}"
    )
}

impl RelativeMotionDebugState {
    pub(in crate::compositor) fn note_dispatch(&mut self, message: impl FnOnce() -> String) {
        self.dispatch_total = self.dispatch_total.saturating_add(1);
        pointer_debug_log_lazy(message);
    }

    pub(in crate::compositor) fn note_drop(&mut self, reason: impl Into<String>) {
        self.pending_drop_count = self.pending_drop_count.saturating_add(1);
        self.pending_drop_reason = Some(reason.into());
        self.flush_drops(false);
    }

    pub(in crate::compositor) fn should_log_route_snapshot(&mut self) -> bool {
        if !pointer_debug_enabled() {
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
        let timestamp_ns = crate::native::event_loop::monotonic_now_ns().unwrap_or_default();
        resize_debug_log(|| {
            format!(
                "timestamp_ns={timestamp_ns} input_hardware_timestamp_usec={} event={event} interaction_id={} resize_interaction_id={} root={} serial={} sequence={} resizing={} geometry={geometry:?} outstanding_count={} acked_uncaptured_count={} queued_latest={:?} final_pending={:?} captured_count={} preview_active={}",
                self.last_pointer_motion_usec
                    .map_or_else(|| "none".to_string(), |timestamp| timestamp.to_string()),
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

#[cfg(test)]
mod pointer_debug_tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn disabled_pointer_debug_does_not_format_locked_motion_messages() {
        let formatted = Cell::new(false);

        let message = pointer_debug_message(false, || {
            formatted.set(true);
            "locked motion".to_string()
        });

        assert_eq!(message, None);
        assert!(!formatted.get());
    }

    #[test]
    fn enabled_cursor_geometry_diagnostic_contains_scaling_inputs() {
        let message = cursor_geometry_debug_message(
            "published",
            "client-7",
            42,
            "shm",
            48,
            64,
            2,
            wl_output::Transform::_90,
            32,
            24,
            Some((3, 4)),
            None,
            1.25,
        );

        assert!(message.contains("client=client-7"));
        assert!(message.contains("buffer=48x64"));
        assert!(message.contains("buffer_scale=2"));
        assert!(message.contains("logical=32x24"));
        assert!(message.contains("hotspot=Some((3, 4))"));
        assert!(message.contains("output_scale=1.250"));
    }
}
