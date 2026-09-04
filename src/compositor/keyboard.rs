use std::{
    env, fmt,
    fs::{self, File, OpenOptions},
    io::{self, Seek, Write},
    os::fd::AsFd,
};

use wayland_server::{Resource, WEnum, protocol::wl_keyboard};
use xkbcommon::xkb;

use super::runtime_files::unique_runtime_file_path;

const WL_KEYBOARD_REPEAT_INFO_SINCE: u32 = 4;
const XKB_EVDEV_OFFSET: u32 = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct KeyboardConfig {
    pub(super) rules: Option<String>,
    pub(super) model: Option<String>,
    pub(super) layout: String,
    pub(super) variant: Option<String>,
    pub(super) options: Option<String>,
    pub(super) repeat_rate: i32,
    pub(super) repeat_delay: i32,
}

impl Default for KeyboardConfig {
    fn default() -> Self {
        Self {
            rules: None,
            model: None,
            layout: "br".to_string(),
            variant: Some("abnt2".to_string()),
            options: None,
            repeat_rate: 25,
            repeat_delay: 600,
        }
    }
}

impl KeyboardConfig {
    pub(super) fn from_env() -> Self {
        let default = Self::default();
        Self {
            rules: optional_env("OBLIVION_ONE_XKB_RULES"),
            model: optional_env("OBLIVION_ONE_XKB_MODEL"),
            layout: optional_env("OBLIVION_ONE_XKB_LAYOUT").unwrap_or(default.layout),
            variant: optional_env("OBLIVION_ONE_XKB_VARIANT").or(default.variant),
            options: optional_env("OBLIVION_ONE_XKB_OPTIONS").or(default.options),
            repeat_rate: non_negative_env_i32("OBLIVION_ONE_XKB_REPEAT_RATE", default.repeat_rate),
            repeat_delay: non_negative_env_i32(
                "OBLIVION_ONE_XKB_REPEAT_DELAY",
                default.repeat_delay,
            ),
        }
    }

    fn minimal_us() -> Self {
        Self {
            layout: "us".to_string(),
            variant: None,
            options: None,
            ..Self::default()
        }
    }
}

fn optional_env(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.is_empty())
}

fn non_negative_env_i32(name: &str, default: i32) -> i32 {
    match env::var(name)
        .ok()
        .and_then(|value| value.parse::<i32>().ok())
    {
        Some(value) if value >= 0 => value,
        _ => default,
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct KeyboardSerializedState {
    pub(super) depressed: u32,
    pub(super) latched: u32,
    pub(super) locked: u32,
    pub(super) group: u32,
}

pub(super) struct XkbKeyboardState {
    keymap: xkb::Keymap,
    state: xkb::State,
    config: KeyboardConfig,
    serialized_keymap_v1: Vec<u8>,
}

// SAFETY: the keymap and state are uniquely owned by CompositorState and are
// accessed only by the compositor's single event thread. OwnCompositorServer
// is constructed before that thread starts and then moved into it; no XKB
// object is ever shared concurrently or accessed through a second owner.
unsafe impl Send for XkbKeyboardState {}

impl fmt::Debug for XkbKeyboardState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("XkbKeyboardState")
            .field("config", &self.config)
            .field("serialized_keymap_v1_len", &self.serialized_keymap_v1.len())
            .finish()
    }
}

impl XkbKeyboardState {
    pub(super) fn from_config(config: &KeyboardConfig) -> Result<Self, String> {
        if let Some(value) = [
            config.rules.as_deref(),
            config.model.as_deref(),
            Some(config.layout.as_str()),
            config.variant.as_deref(),
            config.options.as_deref(),
        ]
        .into_iter()
        .flatten()
        .find(|value| value.contains('\0'))
        {
            return Err(format!("RMLVO value contains NUL byte: {value:?}"));
        }

        let context = xkb::Context::new(xkb::CONTEXT_NO_ENVIRONMENT_NAMES);
        let rules = config.rules.as_deref().unwrap_or("");
        let model = config.model.as_deref().unwrap_or("");
        let variant = config.variant.as_deref().unwrap_or("");
        let options = config.options.clone();
        let keymap = xkb::Keymap::new_from_names(
            &context,
            rules,
            model,
            config.layout.as_str(),
            variant,
            options,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .ok_or_else(|| format!("libxkbcommon rejected {}", describe_config(config)))?;
        let mut serialized_keymap_v1 = keymap
            .get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1)
            .into_bytes();
        if serialized_keymap_v1.last().copied() != Some(0) {
            serialized_keymap_v1.push(0);
        }
        if serialized_keymap_v1.len() <= 1 {
            return Err(format!(
                "libxkbcommon returned an empty Text V1 keymap for {}",
                describe_config(config)
            ));
        }

        let state = xkb::State::new(&keymap);
        Ok(Self {
            keymap,
            state,
            config: config.clone(),
            serialized_keymap_v1,
        })
    }

    pub(super) fn from_environment() -> Option<Self> {
        let requested = KeyboardConfig::from_env();
        let candidates = [
            ("requested", requested),
            ("baseline", KeyboardConfig::default()),
            ("minimal us", KeyboardConfig::minimal_us()),
        ];
        for (label, config) in candidates {
            match Self::from_config(&config) {
                Ok(state) => return Some(state),
                Err(error) => eprintln!(
                    "oblivion-one compositor: failed {label} keyboard configuration ({}): {error}",
                    describe_config(&config)
                ),
            }
        }
        None
    }

    pub(super) fn update_key(&mut self, evdev_key: u32, pressed: bool) -> bool {
        let Some(keycode) = self.keycode_for_evdev(evdev_key) else {
            return false;
        };
        let before = self.serialized_state();
        let direction = if pressed {
            xkb::KeyDirection::Down
        } else {
            xkb::KeyDirection::Up
        };
        self.state.update_key(keycode, direction);
        self.serialized_state() != before
    }

    pub(super) fn serialized_state(&self) -> KeyboardSerializedState {
        KeyboardSerializedState {
            depressed: self.state.serialize_mods(xkb::STATE_MODS_DEPRESSED),
            latched: self.state.serialize_mods(xkb::STATE_MODS_LATCHED),
            locked: self.state.serialize_mods(xkb::STATE_MODS_LOCKED),
            group: self.state.serialize_layout(xkb::STATE_LAYOUT_EFFECTIVE),
        }
    }

    pub(super) fn keymap_file(&self) -> io::Result<(File, u32)> {
        let path = unique_runtime_file_path("oblivion-one-keymap");
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)?;
        let _ = fs::remove_file(&path);
        file.write_all(&self.serialized_keymap_v1)?;
        file.flush()?;
        file.rewind()?;
        let size = u32::try_from(self.serialized_keymap_v1.len())
            .map_err(|_| io::Error::other("keyboard keymap is too large"))?;
        Ok((file, size))
    }

    pub(super) fn send_initial_state(&self, keyboard: &wl_keyboard::WlKeyboard) {
        match self.keymap_file() {
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
                rate: self.config.repeat_rate,
                delay: self.config.repeat_delay,
            });
        }
    }

    #[cfg(test)]
    fn keymap_text_v1(&self) -> &str {
        std::str::from_utf8(&self.serialized_keymap_v1[..self.serialized_keymap_v1.len() - 1])
            .expect("libxkbcommon Text V1 is UTF-8")
    }

    #[cfg(test)]
    fn keymap_keycode_for_evdev(&self, evdev_key: u32) -> Option<u32> {
        self.keycode_for_evdev(evdev_key).map(Into::into)
    }

    #[cfg(test)]
    fn led_active(&self, name: &str) -> bool {
        self.state.led_name_is_active(name)
    }

    fn keycode_for_evdev(&self, evdev_key: u32) -> Option<xkb::Keycode> {
        let keycode = evdev_key.checked_add(XKB_EVDEV_OFFSET)?;
        if !xkb::keycode_is_legal_ext(keycode) {
            return None;
        }
        let min = self.keymap.min_keycode().into();
        let max = self.keymap.max_keycode().into();
        (min..=max)
            .contains(&keycode)
            .then(|| xkb::Keycode::new(keycode))
    }
}

fn describe_config(config: &KeyboardConfig) -> String {
    format!(
        "rules={:?}, model={:?}, layout={:?}, variant={:?}, options={:?}",
        config.rules, config.model, config.layout, config.variant, config.options
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn default_keyboard_config_preserves_native_defaults() {
        let config = KeyboardConfig::default();
        assert_eq!(config.layout, "br");
        assert_eq!(config.variant.as_deref(), Some("abnt2"));
        assert_eq!(config.options, None);
        assert_eq!(config.repeat_rate, 25);
        assert_eq!(config.repeat_delay, 600);
    }

    #[test]
    fn rmlvo_compiles_multiple_layouts_without_manual_include_syntax() {
        let config = KeyboardConfig {
            layout: "br,us".into(),
            variant: Some("abnt2,".into()),
            options: Some("grp:alt_shift_toggle".into()),
            ..KeyboardConfig::default()
        };
        let state = XkbKeyboardState::from_config(&config).unwrap();
        assert!(state.keymap_text_v1().starts_with("xkb_keymap"));
        assert_eq!(state.keymap.num_layouts(), 2);
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        assert!(
            xkb::Keymap::new_from_string(
                &context,
                state.keymap_text_v1().to_string(),
                xkb::KEYMAP_FORMAT_TEXT_V1,
                xkb::KEYMAP_COMPILE_NO_FLAGS,
            )
            .is_some()
        );
    }

    #[test]
    fn update_key_uses_xkb_offset_but_keeps_evdev_api() {
        let mut state = XkbKeyboardState::from_config(&KeyboardConfig::default()).unwrap();
        assert!(!state.update_key(30, true));
        assert!(state.update_key(42, true));
        assert_eq!(state.keymap_keycode_for_evdev(30), Some(38));
    }

    #[test]
    fn caps_lock_changes_xkb_locked_mask_without_manual_modifier_state() {
        let mut state = XkbKeyboardState::from_config(&KeyboardConfig::default()).unwrap();
        assert_eq!(state.serialized_state().locked, 0);
        state.update_key(58, true);
        state.update_key(58, false);
        assert_ne!(state.serialized_state().locked, 0);
        assert!(state.led_active(xkb::LED_NAME_CAPS));
    }

    #[test]
    fn right_alt_uses_the_keymap_level_three_modifier() {
        let mut state = XkbKeyboardState::from_config(&KeyboardConfig::default()).unwrap();
        let level_three = state.keymap.mod_get_index(xkb::MOD_NAME_ISO_LEVEL3_SHIFT);
        assert_ne!(level_three, xkb::MOD_INVALID);
        state.update_key(100, true);
        assert!(
            state
                .state
                .mod_index_is_active(level_three, xkb::STATE_MODS_DEPRESSED)
        );
        assert_ne!(state.serialized_state().depressed, 0);
    }

    #[test]
    fn layout_switching_option_updates_effective_group() {
        let config = KeyboardConfig {
            layout: "br,us".into(),
            variant: Some("abnt2,".into()),
            options: Some("grp:alt_shift_toggle".into()),
            ..KeyboardConfig::default()
        };
        let mut state = XkbKeyboardState::from_config(&config).unwrap();
        assert_eq!(state.serialized_state().group, 0);
        state.update_key(56, true);
        state.update_key(42, true);
        state.update_key(42, false);
        state.update_key(56, false);
        assert_eq!(state.serialized_state().group, 1);
    }

    #[test]
    fn nul_in_rmlvo_is_rejected_before_binding_call() {
        let config = KeyboardConfig {
            layout: "us\0".into(),
            ..KeyboardConfig::default()
        };
        let error = XkbKeyboardState::from_config(&config).unwrap_err();
        assert!(error.contains("NUL"));
    }

    #[test]
    fn negative_repeat_values_fall_back_to_defaults() {
        let _guard = ENV_LOCK.lock().unwrap();
        let previous_rate = env::var_os("OBLIVION_ONE_XKB_REPEAT_RATE");
        let previous_delay = env::var_os("OBLIVION_ONE_XKB_REPEAT_DELAY");
        // SAFETY: the test serializes environment access through the shared lock.
        unsafe {
            env::set_var("OBLIVION_ONE_XKB_REPEAT_RATE", "-1");
            env::set_var("OBLIVION_ONE_XKB_REPEAT_DELAY", "-2");
        }
        let config = KeyboardConfig::from_env();
        // SAFETY: the test serializes environment access through the shared lock.
        unsafe {
            match previous_rate {
                Some(value) => env::set_var("OBLIVION_ONE_XKB_REPEAT_RATE", value),
                None => env::remove_var("OBLIVION_ONE_XKB_REPEAT_RATE"),
            }
            match previous_delay {
                Some(value) => env::set_var("OBLIVION_ONE_XKB_REPEAT_DELAY", value),
                None => env::remove_var("OBLIVION_ONE_XKB_REPEAT_DELAY"),
            }
        }
        assert_eq!(config.repeat_rate, 25);
        assert_eq!(config.repeat_delay, 600);
    }
}
