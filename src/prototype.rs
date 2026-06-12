use std::{
    error::Error,
    num::NonZeroU32,
    process::{Child, Command},
    sync::Arc,
};

use oblivion_one::{
    DockItem, PROTOTYPE_HEIGHT, PROTOTYPE_WIDTH, PrototypeRuntimeState, PrototypeScene, Rect, Rgba,
    app_launch_env_for,
};
use softbuffer::{Context, Surface};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalPosition},
    event::{ElementState, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{Key, NamedKey},
    window::{Window, WindowAttributes, WindowId},
};

type PrototypeResult<T> = Result<T, Box<dyn Error>>;

pub fn run_prototype(inside_de: bool) -> PrototypeResult<()> {
    let event_loop = EventLoop::new()?;
    let mut app = PrototypeApp {
        inside_de,
        ..PrototypeApp::default()
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}

#[derive(Default)]
struct PrototypeApp {
    window: Option<Arc<Window>>,
    surface: Option<Surface<Arc<Window>, Arc<Window>>>,
    scene: PrototypeScene,
    runtime: PrototypeRuntimeState,
    children: Vec<Child>,
    flash_ticks: u8,
    pointer: PhysicalPosition<f64>,
    drag: DragMode,
    inside_de: bool,
}

#[derive(Debug, Clone, Copy, Default)]
enum DragMode {
    #[default]
    None,
    Move {
        window: usize,
        start_pointer: (i32, i32),
        start_rect: Rect,
    },
    Resize {
        window: usize,
        start_pointer: (i32, i32),
        start_rect: Rect,
    },
}

impl ApplicationHandler for PrototypeApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let attributes = WindowAttributes::default()
            .with_title("Oblivion One Prototype")
            .with_inner_size(LogicalSize::new(PROTOTYPE_WIDTH, PROTOTYPE_HEIGHT))
            .with_min_inner_size(LogicalSize::new(900, 560))
            .with_transparent(false)
            .with_resizable(true);

        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                eprintln!("oblivion-one prototype: failed to create window: {error}");
                event_loop.exit();
                return;
            }
        };

        let context = match Context::new(window.clone()) {
            Ok(context) => context,
            Err(error) => {
                eprintln!("oblivion-one prototype: failed to create softbuffer context: {error}");
                event_loop.exit();
                return;
            }
        };

        let surface = match Surface::new(&context, window.clone()) {
            Ok(surface) => surface,
            Err(error) => {
                eprintln!("oblivion-one prototype: failed to create surface: {error}");
                event_loop.exit();
                return;
            }
        };

        self.surface = Some(surface);
        self.window = Some(window);
        self.request_redraw();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer = position;
                self.update_drag(position.x as i32, position.y as i32);
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                let x = self.pointer.x as i32;
                let y = self.pointer.y as i32;
                if self.handle_window_press(x, y) {
                    self.request_redraw();
                } else if let Some(item) = self.scene.dock_item_at(x, y).cloned() {
                    self.launch_item(&item);
                    self.request_redraw();
                } else {
                    self.scene.activate_at(x, y);
                    self.request_redraw();
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => {
                self.drag = DragMode::None;
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                if self.inside_de {
                    return;
                }
                match event.logical_key {
                    Key::Named(NamedKey::Escape) => event_loop.exit(),
                    Key::Named(NamedKey::ArrowLeft) => self.scene.move_active_by(-24, 0),
                    Key::Named(NamedKey::ArrowRight) => self.scene.move_active_by(24, 0),
                    Key::Named(NamedKey::ArrowUp) => self.scene.move_active_by(0, -24),
                    Key::Named(NamedKey::ArrowDown) => self.scene.move_active_by(0, 24),
                    Key::Named(NamedKey::Delete) | Key::Named(NamedKey::Backspace) => {
                        self.scene.close_active();
                    }
                    Key::Named(NamedKey::Tab) => {
                        self.scene.cycle_active();
                        self.request_redraw();
                    }
                    Key::Character(ref character) if character.eq_ignore_ascii_case("q") => {
                        event_loop.exit();
                    }
                    Key::Character(ref character) if character.eq_ignore_ascii_case("m") => {
                        self.scene.minimize_active();
                    }
                    Key::Character(ref character) if character.eq_ignore_ascii_case("f") => {
                        self.scene.toggle_maximize_active();
                    }
                    Key::Character(ref character) if character.eq_ignore_ascii_case("r") => {
                        self.scene.restore_next_minimized();
                    }
                    Key::Character(ref character) if character == "[" => {
                        self.scene.resize_active_by(-36, -24);
                    }
                    Key::Character(ref character) if character == "]" => {
                        self.scene.resize_active_by(36, 24);
                    }
                    _ => {}
                }
                self.request_redraw();
            }
            WindowEvent::Resized(size) => {
                self.scene = PrototypeScene::new(size.width.max(1), size.height.max(1));
                self.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                if let Err(error) = self.draw() {
                    eprintln!("oblivion-one prototype: draw failed: {error}");
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }
}

impl PrototypeApp {
    fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn draw(&mut self) -> PrototypeResult<()> {
        let Some(window) = &self.window else {
            return Ok(());
        };
        let Some(surface) = &mut self.surface else {
            return Ok(());
        };

        let size = window.inner_size();
        let width = NonZeroU32::new(size.width.max(1)).expect("width is clamped above zero");
        let height = NonZeroU32::new(size.height.max(1)).expect("height is clamped above zero");
        surface.resize(width, height)?;

        let mut buffer = surface.buffer_mut()?;
        let mut frame = Frame {
            pixels: &mut buffer,
            width: size.width.max(1),
            height: size.height.max(1),
        };
        render_scene(&mut frame, &self.scene, &self.runtime, self.flash_ticks);
        self.flash_ticks = self.flash_ticks.saturating_sub(1);
        buffer.present()?;
        if self.flash_ticks > 0 {
            self.request_redraw();
        }
        Ok(())
    }

    fn launch_item(&mut self, item: &DockItem) {
        if item.program().is_empty() {
            eprintln!(
                "oblivion-one prototype: `{}` has no launch command",
                item.label
            );
            return;
        }

        let current_env = std::env::vars().collect();
        let child_env = app_launch_env_for(&current_env, item.isolated_profile.then_some(item.id));
        match Command::new(item.program())
            .args(item.args())
            .envs(child_env)
            .spawn()
        {
            Ok(child) => {
                let pid = child.id();
                println!(
                    "oblivion-one prototype: launched {} as pid {} ({})",
                    item.label,
                    pid,
                    item.command.join(" ")
                );
                self.runtime.record_launch(item, pid);
                self.children.push(child);
                self.flash_ticks = 24;
            }
            Err(error) => {
                eprintln!(
                    "oblivion-one prototype: failed to launch {} via `{}`: {error}",
                    item.label,
                    item.command.join(" ")
                );
                self.flash_ticks = 12;
            }
        }
    }

    fn handle_window_press(&mut self, x: i32, y: i32) -> bool {
        let Some(index) = self.hit_window(x, y) else {
            return false;
        };
        self.scene.active_window = index;

        let rect = self.scene.windows[index].rect;
        if control_rect(rect, WindowControl::Close).contains(x, y) {
            self.scene.close_active();
            self.drag = DragMode::None;
            return true;
        }
        if control_rect(rect, WindowControl::Minimize).contains(x, y) {
            self.scene.minimize_active();
            self.drag = DragMode::None;
            return true;
        }
        if control_rect(rect, WindowControl::Maximize).contains(x, y) {
            self.scene.toggle_maximize_active();
            self.drag = DragMode::None;
            return true;
        }
        if resize_handle_rect(rect).contains(x, y) {
            self.drag = DragMode::Resize {
                window: index,
                start_pointer: (x, y),
                start_rect: rect,
            };
            return true;
        }
        if titlebar_rect(rect).contains(x, y) {
            self.drag = DragMode::Move {
                window: index,
                start_pointer: (x, y),
                start_rect: rect,
            };
            return true;
        }

        true
    }

    fn update_drag(&mut self, x: i32, y: i32) {
        match self.drag {
            DragMode::None => {}
            DragMode::Move {
                window,
                start_pointer,
                start_rect,
            } => {
                if let Some(target) = self.scene.windows.get_mut(window) {
                    if target.maximized || target.minimized {
                        return;
                    }
                    let dx = x - start_pointer.0;
                    let dy = y - start_pointer.1;
                    target.rect.x =
                        (start_rect.x + dx).clamp(0, self.scene.size.0.saturating_sub(120) as i32);
                    target.rect.y =
                        (start_rect.y + dy).clamp(40, self.scene.size.1.saturating_sub(120) as i32);
                    self.request_redraw();
                }
            }
            DragMode::Resize {
                window,
                start_pointer,
                start_rect,
            } => {
                if let Some(target) = self.scene.windows.get_mut(window) {
                    if target.maximized || target.minimized {
                        return;
                    }
                    let dx = x - start_pointer.0;
                    let dy = y - start_pointer.1;
                    let max_width = self
                        .scene
                        .size
                        .0
                        .saturating_sub(target.rect.x.max(0) as u32 + 24);
                    let max_height = self
                        .scene
                        .size
                        .1
                        .saturating_sub(target.rect.y.max(0) as u32 + 112);
                    target.rect.width =
                        add_signed(start_rect.width, dx).clamp(260, max_width.max(260));
                    target.rect.height =
                        add_signed(start_rect.height, dy).clamp(160, max_height.max(160));
                    self.request_redraw();
                }
            }
        }
    }

    fn hit_window(&self, x: i32, y: i32) -> Option<usize> {
        self.scene
            .windows
            .iter()
            .enumerate()
            .rfind(|(_, window)| !window.minimized && window.rect.contains(x, y))
            .map(|(index, _)| index)
    }
}

#[derive(Debug, Clone, Copy)]
enum WindowControl {
    Close,
    Minimize,
    Maximize,
}

struct Frame<'a> {
    pixels: &'a mut [u32],
    width: u32,
    height: u32,
}

fn render_scene(
    frame: &mut Frame<'_>,
    scene: &PrototypeScene,
    runtime: &PrototypeRuntimeState,
    flash_ticks: u8,
) {
    frame.clear(Rgba(8, 10, 14, 255));
    draw_wallpaper(frame);

    frame.fill_rounded(scene.topbar_rect(), 16, Rgba(24, 28, 36, 238));
    frame.fill_rect(Rect::new(22, 11, 12, 12), Rgba(255, 95, 87, 255));
    frame.fill_rect(Rect::new(44, 11, 12, 12), Rgba(255, 189, 46, 255));
    frame.fill_rect(Rect::new(66, 11, 12, 12), Rgba(39, 201, 63, 255));
    frame.fill_rect(
        Rect::new((frame.width.saturating_sub(220)) as i32, 13, 150, 7),
        Rgba(164, 176, 198, 170),
    );
    draw_minimized_strip(frame, scene);

    for (index, window) in scene.windows.iter().enumerate() {
        if window.minimized {
            continue;
        }
        let is_active = index == scene.active_window;
        let shadow = Rect::new(
            window.rect.x + 14,
            window.rect.y + 18,
            window.rect.width,
            window.rect.height,
        );
        frame.fill_rounded(shadow, 24, Rgba(0, 0, 0, 80));

        let body = if is_active {
            Rgba(28, 33, 44, 246)
        } else {
            Rgba(22, 25, 33, 224)
        };
        frame.fill_rounded(window.rect, 18, body);
        frame.stroke_rounded(
            window.rect,
            18,
            if is_active {
                window.accent
            } else {
                Rgba(82, 91, 110, 180)
            },
            if is_active { 2 } else { 1 },
        );

        let titlebar = titlebar_rect(window.rect);
        frame.fill_rounded(titlebar, 18, Rgba(255, 255, 255, 18));
        draw_traffic_lights(frame, window.rect.x + 16, window.rect.y + 15);
        draw_window_content(frame, window.rect, window.accent, is_active);
        draw_resize_handle(frame, window.rect, is_active);
    }

    draw_dock(frame, scene, runtime, flash_ticks);
}

fn draw_wallpaper(frame: &mut Frame<'_>) {
    let width = frame.width.max(1);
    let height = frame.height.max(1);
    for y in 0..height {
        for x in 0..width {
            let blue = 26 + ((x * 26 / width) as u8);
            let green = 17 + ((y * 20 / height) as u8);
            let red = 9 + (((x + y) * 12 / (width + height)) as u8);
            frame.set(x as i32, y as i32, Rgba(red, green, blue, 255));
        }
    }

    frame.fill_ellipse(Rect::new(120, 110, 520, 260), Rgba(10, 132, 255, 24));
    frame.fill_ellipse(Rect::new(760, 210, 420, 300), Rgba(215, 186, 125, 22));
}

fn draw_traffic_lights(frame: &mut Frame<'_>, x: i32, y: i32) {
    frame.fill_ellipse(Rect::new(x, y, 12, 12), Rgba(255, 95, 87, 255));
    frame.fill_ellipse(Rect::new(x + 20, y, 12, 12), Rgba(255, 189, 46, 255));
    frame.fill_ellipse(Rect::new(x + 40, y, 12, 12), Rgba(39, 201, 63, 255));
}

fn draw_resize_handle(frame: &mut Frame<'_>, rect: Rect, active: bool) {
    let color = if active {
        Rgba(230, 238, 255, 90)
    } else {
        Rgba(230, 238, 255, 42)
    };
    let handle = resize_handle_rect(rect);
    frame.fill_rect(Rect::new(handle.x + 12, handle.y + 20, 16, 3), color);
    frame.fill_rect(Rect::new(handle.x + 20, handle.y + 12, 3, 16), color);
}

fn draw_minimized_strip(frame: &mut Frame<'_>, scene: &PrototypeScene) {
    let minimized: Vec<_> = scene
        .windows
        .iter()
        .filter(|window| window.minimized)
        .collect();
    if minimized.is_empty() {
        return;
    }

    let width = (minimized.len() as u32 * 112).min(scene.size.0.saturating_sub(48));
    let strip = Rect::new(24, scene.size.1.saturating_sub(156) as i32, width, 44);
    frame.fill_rounded(strip, 16, Rgba(24, 28, 36, 210));
    for (index, window) in minimized.iter().enumerate() {
        let x = strip.x + 12 + index as i32 * 112;
        frame.fill_rounded(Rect::new(x, strip.y + 9, 88, 26), 10, window.accent);
        frame.fill_rect(
            Rect::new(x + 14, strip.y + 20, 38, 4),
            Rgba(255, 255, 255, 150),
        );
    }
}

fn draw_window_content(frame: &mut Frame<'_>, rect: Rect, accent: Rgba, active: bool) {
    let content_y = rect.y + 62;
    let alpha = if active { 210 } else { 130 };
    frame.fill_rect(
        Rect::new(rect.x + 28, content_y, rect.width.saturating_sub(56), 16),
        Rgba(accent.0, accent.1, accent.2, alpha),
    );
    frame.fill_rect(
        Rect::new(
            rect.x + 28,
            content_y + 38,
            rect.width.saturating_sub(96),
            10,
        ),
        Rgba(228, 234, 244, 120),
    );
    frame.fill_rect(
        Rect::new(
            rect.x + 28,
            content_y + 62,
            rect.width.saturating_sub(146),
            10,
        ),
        Rgba(228, 234, 244, 88),
    );

    let card_width = (rect.width.saturating_sub(76)) / 3;
    for column in 0..3 {
        let x = rect.x + 28 + (column * (card_width as i32 + 10));
        let y = rect.y + rect.height as i32 - 118;
        frame.fill_rounded(
            Rect::new(x, y, card_width, 76),
            12,
            Rgba(255, 255, 255, if active { 26 } else { 16 }),
        );
        frame.fill_rect(
            Rect::new(x + 14, y + 18, card_width.saturating_sub(28), 8),
            Rgba(accent.0, accent.1, accent.2, 150),
        );
        frame.fill_rect(
            Rect::new(x + 14, y + 42, card_width.saturating_sub(44), 8),
            Rgba(230, 236, 247, 82),
        );
    }
}

fn draw_dock(
    frame: &mut Frame<'_>,
    scene: &PrototypeScene,
    runtime: &PrototypeRuntimeState,
    flash_ticks: u8,
) {
    let dock = scene.dock_rect();
    frame.fill_rounded(
        Rect::new(dock.x + 8, dock.y + 8, dock.width, dock.height),
        28,
        Rgba(0, 0, 0, 92),
    );
    frame.fill_rounded(dock, 28, Rgba(30, 35, 45, 226));
    frame.stroke_rounded(dock, 28, Rgba(255, 255, 255, 34), 1);

    let item_size = 46;
    for (index, item) in scene.dock_items.iter().enumerate() {
        let Some(rect) = scene.dock_item_rect(index) else {
            continue;
        };
        let x = rect.x;
        let y = rect.y;
        let launches = runtime.launch_count_for(item.id);
        let pulse = flash_ticks > 0 && launches > 0;
        let mut color = item.accent;
        if pulse {
            color = Rgba(
                color.0.saturating_add(24),
                color.1.saturating_add(24),
                color.2.saturating_add(24),
                255,
            );
        }
        frame.fill_rounded(Rect::new(x, y, item_size, item_size), 12, color);
        frame.fill_rect(Rect::new(x + 12, y + 16, 22, 5), Rgba(255, 255, 255, 160));
        frame.fill_rect(Rect::new(x + 12, y + 27, 14, 5), Rgba(255, 255, 255, 110));
        if launches > 0 {
            frame.fill_ellipse(
                Rect::new(x + 19, dock.y + dock.height as i32 - 9, 8, 8),
                Rgba(235, 242, 255, 230),
            );
            if launches > 1 {
                frame.fill_rect(
                    Rect::new(x + 32, dock.y + dock.height as i32 - 7, 10, 4),
                    Rgba(235, 242, 255, 160),
                );
            }
        }
    }
}

impl Frame<'_> {
    fn clear(&mut self, color: Rgba) {
        self.pixels.fill(color.to_argb());
    }

    fn set(&mut self, x: i32, y: i32, color: Rgba) {
        if x < 0 || y < 0 {
            return;
        }
        let x = x as u32;
        let y = y as u32;
        if x >= self.width || y >= self.height {
            return;
        }
        let index = (y * self.width + x) as usize;
        self.pixels[index] = blend(self.pixels[index], color);
    }

    fn fill_rect(&mut self, rect: Rect, color: Rgba) {
        let left = rect.x.max(0) as u32;
        let top = rect.y.max(0) as u32;
        let right = (rect.x + rect.width as i32).clamp(0, self.width as i32) as u32;
        let bottom = (rect.y + rect.height as i32).clamp(0, self.height as i32) as u32;

        for y in top..bottom {
            let row = (y * self.width) as usize;
            for x in left..right {
                let index = row + x as usize;
                self.pixels[index] = blend(self.pixels[index], color);
            }
        }
    }

    fn fill_rounded(&mut self, rect: Rect, radius: i32, color: Rgba) {
        let left = rect.x;
        let top = rect.y;
        let right = rect.x + rect.width as i32;
        let bottom = rect.y + rect.height as i32;
        let radius = radius.max(0);

        for y in top..bottom {
            for x in left..right {
                if rounded_contains(x, y, rect, radius) {
                    self.set(x, y, color);
                }
            }
        }
    }

    fn stroke_rounded(&mut self, rect: Rect, radius: i32, color: Rgba, width: i32) {
        for inset in 0..width.max(1) {
            let inset_rect = Rect::new(
                rect.x + inset,
                rect.y + inset,
                rect.width.saturating_sub((inset * 2) as u32),
                rect.height.saturating_sub((inset * 2) as u32),
            );
            self.stroke_rounded_once(inset_rect, radius.saturating_sub(inset), color);
        }
    }

    fn stroke_rounded_once(&mut self, rect: Rect, radius: i32, color: Rgba) {
        let left = rect.x;
        let top = rect.y;
        let right = rect.x + rect.width as i32 - 1;
        let bottom = rect.y + rect.height as i32 - 1;

        for x in left..=right {
            if rounded_contains(x, top, rect, radius) {
                self.set(x, top, color);
            }
            if rounded_contains(x, bottom, rect, radius) {
                self.set(x, bottom, color);
            }
        }
        for y in top..=bottom {
            if rounded_contains(left, y, rect, radius) {
                self.set(left, y, color);
            }
            if rounded_contains(right, y, rect, radius) {
                self.set(right, y, color);
            }
        }
    }

    fn fill_ellipse(&mut self, rect: Rect, color: Rgba) {
        let rx = rect.width as f32 / 2.0;
        let ry = rect.height as f32 / 2.0;
        let cx = rect.x as f32 + rx;
        let cy = rect.y as f32 + ry;

        for y in rect.y..rect.y + rect.height as i32 {
            for x in rect.x..rect.x + rect.width as i32 {
                let dx = (x as f32 + 0.5 - cx) / rx.max(1.0);
                let dy = (y as f32 + 0.5 - cy) / ry.max(1.0);
                if dx * dx + dy * dy <= 1.0 {
                    self.set(x, y, color);
                }
            }
        }
    }
}

fn rounded_contains(x: i32, y: i32, rect: Rect, radius: i32) -> bool {
    if radius <= 0 {
        return rect.contains(x, y);
    }

    let left = rect.x;
    let top = rect.y;
    let right = rect.x + rect.width as i32 - 1;
    let bottom = rect.y + rect.height as i32 - 1;
    let radius = radius
        .min((rect.width / 2) as i32)
        .min((rect.height / 2) as i32);

    let cx = if x < left + radius {
        left + radius
    } else if x > right - radius {
        right - radius
    } else {
        x
    };
    let cy = if y < top + radius {
        top + radius
    } else if y > bottom - radius {
        bottom - radius
    } else {
        y
    };

    let dx = x - cx;
    let dy = y - cy;
    dx * dx + dy * dy <= radius * radius
}

fn titlebar_rect(rect: Rect) -> Rect {
    Rect::new(rect.x, rect.y, rect.width, 42)
}

fn resize_handle_rect(rect: Rect) -> Rect {
    Rect::new(
        rect.x + rect.width as i32 - 34,
        rect.y + rect.height as i32 - 34,
        30,
        30,
    )
}

fn control_rect(rect: Rect, control: WindowControl) -> Rect {
    let offset = match control {
        WindowControl::Close => 0,
        WindowControl::Minimize => 20,
        WindowControl::Maximize => 40,
    };
    Rect::new(rect.x + 16 + offset, rect.y + 15, 14, 14)
}

fn add_signed(value: u32, delta: i32) -> u32 {
    if delta.is_negative() {
        value.saturating_sub(delta.unsigned_abs())
    } else {
        value.saturating_add(delta as u32)
    }
}

fn blend(base: u32, overlay: Rgba) -> u32 {
    if overlay.3 == 255 {
        return overlay.to_argb();
    }
    if overlay.3 == 0 {
        return base;
    }

    let alpha = overlay.3 as u32;
    let inv_alpha = 255 - alpha;
    let base_red = (base >> 16) & 0xff;
    let base_green = (base >> 8) & 0xff;
    let base_blue = base & 0xff;

    let red = (overlay.0 as u32 * alpha + base_red * inv_alpha) / 255;
    let green = (overlay.1 as u32 * alpha + base_green * inv_alpha) / 255;
    let blue = (overlay.2 as u32 * alpha + base_blue * inv_alpha) / 255;

    0xff00_0000 | (red << 16) | (green << 8) | blue
}
