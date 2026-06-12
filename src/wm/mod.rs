use crate::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowId(u64);

impl WindowId {
    pub const fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedWindow {
    pub id: WindowId,
    pub app_id: String,
    pub rect: Rect,
    pub floating: bool,
    pub focused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowManager {
    bounds: (u32, u32),
    next_id: u64,
    windows: Vec<ManagedWindow>,
    focused: Option<WindowId>,
}

impl WindowManager {
    pub fn new(bounds: (u32, u32)) -> Self {
        Self {
            bounds,
            next_id: 1,
            windows: Vec::new(),
            focused: None,
        }
    }

    pub fn add_window(&mut self, app_id: impl Into<String>, rect: Rect) -> WindowId {
        let id = WindowId(self.next_id);
        self.next_id += 1;
        self.windows.push(ManagedWindow {
            id,
            app_id: app_id.into(),
            rect,
            floating: true,
            focused: false,
        });
        id
    }

    pub fn focus(&mut self, id: WindowId) {
        self.focused = None;
        for window in &mut self.windows {
            window.focused = window.id == id;
            if window.focused {
                self.focused = Some(id);
            }
        }
    }

    pub fn window(&self, id: WindowId) -> Option<&ManagedWindow> {
        self.windows.iter().find(|window| window.id == id)
    }

    pub fn move_focused_by(&mut self, dx: i32, dy: i32) {
        let (width, height) = self.bounds;
        let Some(window) = self.focused_window_mut() else {
            return;
        };
        if !window.floating {
            return;
        }

        window.rect.x = (window.rect.x + dx).clamp(0, width.saturating_sub(120) as i32);
        window.rect.y = (window.rect.y + dy).clamp(0, height.saturating_sub(120) as i32);
    }

    pub fn resize_focused_by(&mut self, dw: i32, dh: i32) {
        let (bounds_width, bounds_height) = self.bounds;
        let Some(window) = self.focused_window_mut() else {
            return;
        };
        if !window.floating {
            return;
        }

        let max_width = bounds_width.saturating_sub(window.rect.x.max(0) as u32 + 24);
        let max_height = bounds_height.saturating_sub(window.rect.y.max(0) as u32 + 24);
        window.rect.width = add_signed(window.rect.width, dw).clamp(260, max_width.max(260));
        window.rect.height = add_signed(window.rect.height, dh).clamp(160, max_height.max(160));
    }

    fn focused_window_mut(&mut self) -> Option<&mut ManagedWindow> {
        let id = self.focused?;
        self.windows.iter_mut().find(|window| window.id == id)
    }
}

fn add_signed(value: u32, delta: i32) -> u32 {
    if delta.is_negative() {
        value.saturating_sub(delta.unsigned_abs())
    } else {
        value.saturating_add(delta as u32)
    }
}
