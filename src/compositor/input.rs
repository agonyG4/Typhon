use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{self, Seek, Write},
    os::fd::AsFd,
    time::{SystemTime, UNIX_EPOCH},
};

use wayland_server::{
    Resource, WEnum,
    protocol::{wl_keyboard, wl_pointer, wl_surface},
};

use super::unique_runtime_file_path;

const WL_POINTER_FRAME_SINCE: u32 = 5;
const WL_KEYBOARD_REPEAT_INFO_SINCE: u32 = 4;
const XKB_SHIFT_MASK: u32 = 1 << 0;
const XKB_LOCK_MASK: u32 = 1 << 1;
const XKB_CONTROL_MASK: u32 = 1 << 2;
const XKB_ALT_MASK: u32 = 1 << 3;
const XKB_SUPER_MASK: u32 = 1 << 6;

#[derive(Debug, Clone)]
pub(super) struct InputSerial {
    pub(super) serial: u32,
    pub(super) surface: wl_surface::WlSurface,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct OutputPosition {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OutputRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl OutputRect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Option<Self> {
        if !x.is_finite()
            || !y.is_finite()
            || !width.is_finite()
            || !height.is_finite()
            || width <= 0.0
            || height <= 0.0
        {
            return None;
        }
        Some(Self {
            x,
            y,
            width,
            height,
        })
    }

    pub fn closest_point(self, position: OutputPosition) -> OutputPosition {
        let max_x = self.x + self.width - 1.0;
        let max_y = self.y + self.height - 1.0;
        OutputPosition {
            x: position.x.clamp(self.x, max_x),
            y: position.y.clamp(self.y, max_y),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutputRegion {
    pub rects: Vec<OutputRect>,
}

impl OutputRegion {
    pub fn from_rect(rect: OutputRect) -> Self {
        Self { rects: vec![rect] }
    }

    pub fn closest_point(&self, position: OutputPosition) -> OutputPosition {
        let Some(first) = self.rects.first().copied() else {
            return position;
        };
        let mut closest = first.closest_point(position);
        let mut closest_distance = output_distance_squared(position, closest);
        for rect in self.rects.iter().copied().skip(1) {
            let candidate = rect.closest_point(position);
            let distance = output_distance_squared(position, candidate);
            if distance < closest_distance {
                closest = candidate;
                closest_distance = distance;
            }
        }
        closest
    }
}

fn output_distance_squared(left: OutputPosition, right: OutputPosition) -> f64 {
    let dx = left.x - right.x;
    let dy = left.y - right.y;
    dx.mul_add(dx, dy * dy)
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RelativePointerMotion {
    pub dx: f64,
    pub dy: f64,
    pub dx_unaccelerated: f64,
    pub dy_unaccelerated: f64,
}

impl RelativePointerMotion {
    pub fn is_zero(self) -> bool {
        self.dx == 0.0
            && self.dy == 0.0
            && self.dx_unaccelerated == 0.0
            && self.dy_unaccelerated == 0.0
    }

    pub fn from_absolute_delta(dx: f64, dy: f64) -> Option<Self> {
        if dx == 0.0 && dy == 0.0 {
            return None;
        }
        Some(Self {
            dx,
            dy,
            dx_unaccelerated: dx,
            dy_unaccelerated: dy,
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PointerMotionSample {
    pub timestamp_usec: u64,
    pub absolute: Option<OutputPosition>,
    pub relative: Option<RelativePointerMotion>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum PointerConstraintMode {
    None,
    Confined,
    Locked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerConstraintLifetime {
    Oneshot,
    Persistent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PointerConstraintBackendId {
    pub constraint_id: u64,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PointerConstraintBackendRequest {
    ActivateLocked {
        id: PointerConstraintBackendId,
        anchor: OutputPosition,
    },
    ActivateConfined {
        id: PointerConstraintBackendId,
        region: OutputRegion,
    },
    UpdateConfinedRegion {
        id: PointerConstraintBackendId,
        region: OutputRegion,
    },
    Deactivate {
        id: PointerConstraintBackendId,
        restore_position: Option<OutputPosition>,
    },
    WarpPointer {
        position: OutputPosition,
    },
    ApplyCursorVisibility {
        visible: bool,
    },
}

impl PointerConstraintBackendRequest {
    pub const fn id(&self) -> Option<PointerConstraintBackendId> {
        match self {
            Self::ActivateLocked { id, .. }
            | Self::ActivateConfined { id, .. }
            | Self::UpdateConfinedRegion { id, .. } => Some(*id),
            Self::Deactivate { id, .. } => Some(*id),
            Self::WarpPointer { .. } | Self::ApplyCursorVisibility { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub struct PointerConstraintState {
    mode: PointerConstraintMode,
    surface_id: Option<u32>,
}

impl Default for PointerConstraintState {
    fn default() -> Self {
        Self {
            mode: PointerConstraintMode::None,
            surface_id: None,
        }
    }
}

impl PointerConstraintState {
    #[allow(dead_code)]
    pub fn activate(&mut self, mode: PointerConstraintMode, surface_id: u32) {
        self.mode = mode;
        self.surface_id = Some(surface_id);
    }

    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.mode = PointerConstraintMode::None;
        self.surface_id = None;
    }

    #[allow(dead_code)]
    pub const fn mode(self) -> PointerConstraintMode {
        self.mode
    }

    #[allow(dead_code)]
    pub fn filters_absolute_motion(self, surface_id: u32) -> bool {
        self.surface_id == Some(surface_id) && matches!(self.mode, PointerConstraintMode::Locked)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct KeyboardModifierState {
    shift_left: bool,
    shift_right: bool,
    control_left: bool,
    control_right: bool,
    alt_left: bool,
    alt_right: bool,
    super_left: bool,
    super_right: bool,
    caps_lock_pressed: bool,
    caps_locked: bool,
}

impl KeyboardModifierState {
    pub(super) fn update_key(&mut self, key: u32, pressed: bool) -> bool {
        let before = self.serialized_state();
        match key {
            29 => self.control_left = pressed,
            97 => self.control_right = pressed,
            42 => self.shift_left = pressed,
            54 => self.shift_right = pressed,
            56 => self.alt_left = pressed,
            100 => self.alt_right = pressed,
            125 => self.super_left = pressed,
            126 => self.super_right = pressed,
            58 => {
                if pressed && !self.caps_lock_pressed {
                    self.caps_locked = !self.caps_locked;
                }
                self.caps_lock_pressed = pressed;
            }
            _ => {}
        }
        self.serialized_state() != before
    }

    pub(super) fn mods_depressed(self) -> u32 {
        let mut mods = 0;
        if self.shift_left || self.shift_right {
            mods |= XKB_SHIFT_MASK;
        }
        if self.control_left || self.control_right {
            mods |= XKB_CONTROL_MASK;
        }
        if self.alt_left || self.alt_right {
            mods |= XKB_ALT_MASK;
        }
        if self.super_left || self.super_right {
            mods |= XKB_SUPER_MASK;
        }
        mods
    }

    pub(super) fn mods_locked(self) -> u32 {
        if self.caps_locked { XKB_LOCK_MASK } else { 0 }
    }

    fn serialized_state(self) -> (u32, u32) {
        (self.mods_depressed(), self.mods_locked())
    }
}

pub(super) fn send_pointer_frame_if_supported(pointer: &wl_pointer::WlPointer) {
    if pointer.version() >= WL_POINTER_FRAME_SINCE {
        let _ = pointer.send_event(wl_pointer::Event::Frame);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct KeyboardLayoutConfig {
    layout: String,
    variant: Option<String>,
    options: Option<String>,
}

impl Default for KeyboardLayoutConfig {
    fn default() -> Self {
        Self {
            layout: "br".to_string(),
            variant: Some("abnt2".to_string()),
            options: None,
        }
    }
}

impl KeyboardLayoutConfig {
    fn from_env() -> Self {
        let default = Self::default();
        Self {
            layout: env::var("OBLIVION_ONE_XKB_LAYOUT")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or(default.layout),
            variant: env::var("OBLIVION_ONE_XKB_VARIANT")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .or(default.variant),
            options: env::var("OBLIVION_ONE_XKB_OPTIONS")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .or(default.options),
        }
    }
}

fn xkb_symbols_include(config: &KeyboardLayoutConfig) -> String {
    let mut include = format!("pc+{}", sanitize_xkb_atom(&config.layout, "br"));
    if let Some(variant) = &config.variant {
        include.push('(');
        include.push_str(&sanitize_xkb_atom(variant, "abnt2"));
        include.push(')');
    }
    include.push_str("+inet(evdev)");
    if let Some(options) = &config.options {
        for option in options.split(',') {
            let option = sanitize_xkb_option(option);
            if !option.is_empty() {
                include.push('+');
                include.push_str(&option);
            }
        }
    }
    include
}

fn keymap_contents(config: &KeyboardLayoutConfig) -> String {
    let symbols = xkb_symbols_include(config);
    format!(
        r#"xkb_keymap {{
xkb_keycodes "evdev+aliases(qwerty)" {{
    include "evdev+aliases(qwerty)"
}};
xkb_types "complete" {{
    include "complete"
}};
xkb_compatibility "complete" {{
    include "complete"
}};
xkb_symbols "{symbols}" {{
    include "{symbols}"
}};
xkb_geometry "pc(pc105)" {{
    include "pc(pc105)"
}};
}};
"#
    )
}

fn sanitize_xkb_atom(value: &str, fallback: &str) -> String {
    let value = value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
        .collect::<String>();
    if value.is_empty() {
        fallback.to_string()
    } else {
        value
    }
}

fn sanitize_xkb_option(value: &str) -> String {
    value
        .trim()
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | ':' | '.')
        })
        .collect()
}

pub(super) fn send_keyboard_initial_state(keyboard: &wl_keyboard::WlKeyboard) {
    match keymap_file() {
        Ok((file, size)) => {
            let _ = keyboard.send_event(wl_keyboard::Event::Keymap {
                format: WEnum::Value(wl_keyboard::KeymapFormat::XkbV1),
                fd: file.as_fd(),
                size,
            });
        }
        Err(error) => {
            eprintln!("oblivion-one compositor: failed to create keyboard keymap: {error}");
        }
    }

    if keyboard.version() >= WL_KEYBOARD_REPEAT_INFO_SINCE {
        let _ = keyboard.send_event(wl_keyboard::Event::RepeatInfo {
            rate: 25,
            delay: 600,
        });
    }
}

fn keymap_file() -> io::Result<(File, u32)> {
    let path = unique_runtime_file_path("oblivion-one-keymap");
    let keymap = keymap_contents(&KeyboardLayoutConfig::from_env());
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)?;
    let _ = fs::remove_file(&path);
    file.write_all(keymap.as_bytes())?;
    file.write_all(&[0])?;
    file.flush()?;
    file.rewind()?;
    let size = keymap
        .len()
        .checked_add(1)
        .and_then(|size| u32::try_from(size).ok())
        .ok_or_else(|| io::Error::other("keyboard keymap is too large"))?;
    Ok((file, size))
}

pub(super) fn wayland_event_time() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u32)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_keyboard_layout_prefers_abnt2_for_native_session() {
        let config = KeyboardLayoutConfig::default();

        assert_eq!(config.layout, "br");
        assert_eq!(config.variant.as_deref(), Some("abnt2"));
        assert_eq!(xkb_symbols_include(&config), "pc+br(abnt2)+inet(evdev)");
    }

    #[test]
    fn keyboard_layout_config_builds_xkb_include_with_options() {
        let config = KeyboardLayoutConfig {
            layout: "us".to_string(),
            variant: None,
            options: Some("compose:ralt".to_string()),
        };

        assert_eq!(
            xkb_symbols_include(&config),
            "pc+us+inet(evdev)+compose:ralt"
        );
        assert!(keymap_contents(&config).contains("include \"pc+us+inet(evdev)+compose:ralt\""));
    }

    #[test]
    fn relative_pointer_motion_uses_absolute_delta_for_both_tracks() {
        let motion = RelativePointerMotion::from_absolute_delta(4.0, -2.5).unwrap();

        assert_eq!(motion.dx, 4.0);
        assert_eq!(motion.dy, -2.5);
        assert_eq!(motion.dx_unaccelerated, 4.0);
        assert_eq!(motion.dy_unaccelerated, -2.5);
        assert!(RelativePointerMotion::from_absolute_delta(0.0, 0.0).is_none());
    }

    #[test]
    fn relative_pointer_motion_detects_zero_across_both_tracks() {
        assert!(RelativePointerMotion::default().is_zero());
        assert!(
            !RelativePointerMotion {
                dx: 0.0,
                dy: 0.0,
                dx_unaccelerated: 0.25,
                dy_unaccelerated: 0.0,
            }
            .is_zero()
        );
    }

    #[test]
    fn pointer_constraint_locked_surface_filters_absolute_motion() {
        let mut state = PointerConstraintState::default();

        state.activate(PointerConstraintMode::Confined, 42);
        assert!(!state.filters_absolute_motion(42));
        state.activate(PointerConstraintMode::Locked, 42);

        assert!(state.filters_absolute_motion(42));
        assert!(!state.filters_absolute_motion(7));
        state.clear();
        assert_eq!(state.mode(), PointerConstraintMode::None);
    }
}
