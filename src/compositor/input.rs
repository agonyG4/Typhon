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
    file.flush()?;
    file.rewind()?;
    Ok((file, keymap.len() as u32))
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
}
