use crate::{PROTOTYPE_HEIGHT, PROTOTYPE_WIDTH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgba(pub u8, pub u8, pub u8, pub u8);

impl Rgba {
    pub const fn to_argb(self) -> u32 {
        let Self(red, green, blue, alpha) = self;
        ((alpha as u32) << 24) | ((red as u32) << 16) | ((green as u32) << 8) | blue as u32
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn contains(self, x: i32, y: i32) -> bool {
        let right = self.x + self.width as i32;
        let bottom = self.y + self.height as i32;
        x >= self.x && x < right && y >= self.y && y < bottom
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrototypeWindow {
    pub id: &'static str,
    pub title: &'static str,
    pub subtitle: &'static str,
    pub rect: Rect,
    pub restore_rect: Option<Rect>,
    pub minimized: bool,
    pub maximized: bool,
    pub accent: Rgba,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockItem {
    pub id: &'static str,
    pub label: &'static str,
    pub command: &'static [&'static str],
    pub accent: Rgba,
    pub isolated_profile: bool,
}

impl DockItem {
    pub fn program(&self) -> &'static str {
        self.command.first().copied().unwrap_or("")
    }

    pub fn args(&self) -> &'static [&'static str] {
        self.command.get(1..).unwrap_or(&[])
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchRecord {
    pub app_id: &'static str,
    pub label: &'static str,
    pub pid: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrototypeRuntimeState {
    pub launches: Vec<LaunchRecord>,
}

impl PrototypeRuntimeState {
    pub fn record_launch(&mut self, item: &DockItem, pid: u32) {
        self.launches.push(LaunchRecord {
            app_id: item.id,
            label: item.label,
            pid,
        });
    }

    pub fn launch_count_for(&self, app_id: &str) -> usize {
        self.launches
            .iter()
            .filter(|record| record.app_id == app_id)
            .count()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrototypeScene {
    pub size: (u32, u32),
    pub active_window: usize,
    pub windows: Vec<PrototypeWindow>,
    pub dock_items: Vec<DockItem>,
}

impl PrototypeScene {
    pub fn new(width: u32, height: u32) -> Self {
        let scale_x = width as f32 / PROTOTYPE_WIDTH as f32;
        let scale_y = height as f32 / PROTOTYPE_HEIGHT as f32;
        let scale = scale_x.min(scale_y).max(0.65);

        Self {
            size: (width, height),
            active_window: 0,
            windows: vec![
                PrototypeWindow {
                    id: "explorer",
                    title: "Explorer",
                    subtitle: "Arquivos, mídia e ações contextuais",
                    rect: scaled_rect(140, 120, 520, 360, scale),
                    restore_rect: None,
                    minimized: false,
                    maximized: false,
                    accent: Rgba(10, 132, 255, 255),
                },
                PrototypeWindow {
                    id: "settings",
                    title: "Settings",
                    subtitle: "Borealis controls and system identity",
                    rect: scaled_rect(560, 170, 500, 340, scale),
                    restore_rect: None,
                    minimized: false,
                    maximized: false,
                    accent: Rgba(118, 214, 255, 255),
                },
                PrototypeWindow {
                    id: "spotlight",
                    title: "Spotlight",
                    subtitle: "Search, launch, actions",
                    rect: scaled_rect(360, 72, 560, 92, scale),
                    restore_rect: None,
                    minimized: false,
                    maximized: false,
                    accent: Rgba(215, 186, 125, 255),
                },
            ],
            dock_items: vec![
                DockItem {
                    id: "terminal",
                    label: "Terminal",
                    command: &["kitty", "--class", "OblivionOneTerminal"],
                    accent: Rgba(10, 132, 255, 255),
                    isolated_profile: false,
                },
                DockItem {
                    id: "explorer",
                    label: "Explorer",
                    command: &[
                        "quickshell",
                        "-p",
                        "/home/agony/.local/share/Astrea/Apps/Explorer/Main.qml",
                    ],
                    accent: Rgba(39, 201, 63, 255),
                    isolated_profile: false,
                },
                DockItem {
                    id: "browser",
                    label: "Browser",
                    command: &[
                        "sh",
                        "-lc",
                        "exec brave --user-data-dir=\"$OBLIVION_ONE_APP_DIR/brave-profile\" --ozone-platform=wayland --enable-features=UseOzonePlatform --use-gl=egl-angle --use-angle=opengles --disable-features=Vulkan --disable-vulkan --new-window about:blank",
                    ],
                    accent: Rgba(255, 189, 46, 255),
                    isolated_profile: true,
                },
                DockItem {
                    id: "music",
                    label: "Music",
                    command: &[
                        "sh",
                        "-lc",
                        "exec spotify --ozone-platform=wayland --enable-features=UseOzonePlatform",
                    ],
                    accent: Rgba(255, 95, 87, 255),
                    isolated_profile: true,
                },
                DockItem {
                    id: "settings",
                    label: "Settings",
                    command: &[
                        "quickshell",
                        "-p",
                        "/home/agony/.local/share/Astrea/Apps/Settings/main.qml",
                    ],
                    accent: Rgba(176, 138, 255, 255),
                    isolated_profile: false,
                },
            ],
        }
    }

    pub fn cycle_active(&mut self) {
        if !self.windows.is_empty() {
            let start = self.active_window;
            for offset in 1..=self.windows.len() {
                let index = (start + offset) % self.windows.len();
                if !self.windows[index].minimized {
                    self.active_window = index;
                    break;
                }
            }
        }
    }

    pub fn activate_at(&mut self, x: i32, y: i32) -> Option<&'static str> {
        let index = self
            .windows
            .iter()
            .rposition(|window| !window.minimized && window.rect.contains(x, y))?;
        self.active_window = index;
        Some(self.windows[index].id)
    }

    pub fn active_window(&self) -> Option<&PrototypeWindow> {
        self.windows.get(self.active_window)
    }

    pub fn active_window_mut(&mut self) -> Option<&mut PrototypeWindow> {
        self.windows.get_mut(self.active_window)
    }

    pub fn move_active_by(&mut self, dx: i32, dy: i32) {
        let (width, height) = self.size;
        let Some(window) = self.active_window_mut() else {
            return;
        };
        if window.minimized || window.maximized {
            return;
        }
        window.rect.x = (window.rect.x + dx).clamp(0, width.saturating_sub(120) as i32);
        window.rect.y = (window.rect.y + dy).clamp(40, height.saturating_sub(120) as i32);
    }

    pub fn resize_active_by(&mut self, dw: i32, dh: i32) {
        let (scene_width, scene_height) = self.size;
        let Some(window) = self.active_window_mut() else {
            return;
        };
        if window.minimized || window.maximized {
            return;
        }

        let max_width = scene_width.saturating_sub(window.rect.x.max(0) as u32 + 24);
        let max_height = scene_height.saturating_sub(window.rect.y.max(0) as u32 + 112);
        window.rect.width = add_signed(window.rect.width, dw).clamp(260, max_width.max(260));
        window.rect.height = add_signed(window.rect.height, dh).clamp(160, max_height.max(160));
    }

    pub fn minimize_active(&mut self) {
        if let Some(window) = self.active_window_mut() {
            window.minimized = true;
        }
        self.cycle_active();
    }

    pub fn restore_next_minimized(&mut self) -> Option<&'static str> {
        let index = self.windows.iter().position(|window| window.minimized)?;
        self.windows[index].minimized = false;
        self.active_window = index;
        Some(self.windows[index].id)
    }

    pub fn toggle_maximize_active(&mut self) {
        let (width, height) = self.size;
        let Some(window) = self.active_window_mut() else {
            return;
        };
        if window.minimized {
            return;
        }

        if window.maximized {
            if let Some(rect) = window.restore_rect.take() {
                window.rect = rect;
            }
            window.maximized = false;
        } else {
            window.restore_rect = Some(window.rect);
            window.rect = Rect::new(24, 52, width.saturating_sub(48), height.saturating_sub(152));
            window.maximized = true;
        }
    }

    pub fn close_active(&mut self) -> Option<&'static str> {
        if self.windows.is_empty() {
            return None;
        }
        let removed = self.windows.remove(self.active_window);
        if self.active_window >= self.windows.len() {
            self.active_window = self.windows.len().saturating_sub(1);
        }
        Some(removed.id)
    }

    pub fn topbar_rect(&self) -> Rect {
        Rect::new(0, 0, self.size.0, 36)
    }

    pub fn dock_rect(&self) -> Rect {
        let width = (self.dock_items.len() as u32 * 72).min(self.size.0.saturating_sub(48));
        let x = ((self.size.0 - width) / 2) as i32;
        let y = self.size.1.saturating_sub(84) as i32;
        Rect::new(x, y, width, 64)
    }

    pub fn dock_item_rect(&self, index: usize) -> Option<Rect> {
        if index >= self.dock_items.len() {
            return None;
        }
        let dock = self.dock_rect();
        Some(Rect::new(
            dock.x + 16 + index as i32 * 72,
            dock.y + 9,
            46,
            46,
        ))
    }

    pub fn dock_item_at(&self, x: i32, y: i32) -> Option<&DockItem> {
        self.dock_items
            .iter()
            .enumerate()
            .find(|(index, _)| {
                self.dock_item_rect(*index)
                    .is_some_and(|rect| rect.contains(x, y))
            })
            .map(|(_, item)| item)
    }
}

impl Default for PrototypeScene {
    fn default() -> Self {
        Self::new(PROTOTYPE_WIDTH, PROTOTYPE_HEIGHT)
    }
}

fn scaled_rect(x: i32, y: i32, width: u32, height: u32, scale: f32) -> Rect {
    Rect::new(
        (x as f32 * scale) as i32,
        (y as f32 * scale) as i32,
        (width as f32 * scale) as u32,
        (height as f32 * scale) as u32,
    )
}

fn add_signed(value: u32, delta: i32) -> u32 {
    if delta.is_negative() {
        value.saturating_sub(delta.unsigned_abs())
    } else {
        value.saturating_add(delta as u32)
    }
}
