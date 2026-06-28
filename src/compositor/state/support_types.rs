use super::*;

pub(crate) fn coalesce_output_row_rects(rects: Vec<OutputRect>) -> Vec<OutputRect> {
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
    pub(crate) fn desired_visible(&self) -> bool {
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

pub(crate) fn pointer_debug_log(message: impl AsRef<str>) {
    if std::env::var_os("TYPHON_POINTER_DEBUG").is_some() {
        eprintln!("typhon pointer: {}", message.as_ref());
    }
}

impl RelativeMotionDebugState {
    pub(crate) fn note_dispatch(&mut self, message: String) {
        self.dispatch_total = self.dispatch_total.saturating_add(1);
        pointer_debug_log(message);
    }

    pub(crate) fn note_drop(&mut self, reason: impl Into<String>) {
        self.pending_drop_count = self.pending_drop_count.saturating_add(1);
        self.pending_drop_reason = Some(reason.into());
        self.flush_drops(false);
    }

    pub(crate) fn should_log_route_snapshot(&mut self) -> bool {
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

    pub(crate) fn flush_drops(&mut self, force: bool) {
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

pub(crate) fn wayland_resource_client_label(resource: &impl Resource) -> String {
    resource
        .client()
        .map(|client| format!("{:?}", client.id()))
        .unwrap_or_else(|| "unknown".to_string())
}
