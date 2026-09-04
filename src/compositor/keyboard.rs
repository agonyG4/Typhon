use std::{
    cell::RefCell,
    collections::HashMap,
    env, fmt,
    fs::{self, File, OpenOptions},
    io::{self, Seek, Write},
    os::fd::AsFd,
    sync::atomic::{AtomicU64, Ordering},
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
            layout: DEFAULT_LAYOUT.to_string(),
            variant: Some(DEFAULT_VARIANT.to_string()),
            options: None,
            repeat_rate: 25,
            repeat_delay: 600,
        }
    }
}

impl KeyboardConfig {
    pub(super) fn from_env() -> Self {
        let default = Self::default();
        let layout_override = env_override("OBLIVION_ONE_XKB_LAYOUT");
        let variant_override = env_override("OBLIVION_ONE_XKB_VARIANT");
        let (layout, variant) =
            resolve_layout_variant(layout_override.as_deref(), variant_override.as_deref());
        Self {
            rules: optional_env("OBLIVION_ONE_XKB_RULES"),
            model: optional_env("OBLIVION_ONE_XKB_MODEL"),
            layout,
            variant,
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

const DEFAULT_LAYOUT: &str = "br";
const DEFAULT_VARIANT: &str = "abnt2";

fn resolve_layout_variant(
    layout_override: Option<&str>,
    variant_override: Option<&str>,
) -> (String, Option<String>) {
    let layout = layout_override.unwrap_or(DEFAULT_LAYOUT).to_string();
    let variant = match variant_override {
        Some(variant) => Some(variant.to_string()),
        None if layout_override.is_none() => Some(DEFAULT_VARIANT.to_string()),
        None => None,
    };
    (layout, variant)
}

fn env_override(name: &str) -> Option<String> {
    env::var(name).ok()
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

#[derive(Debug, Default)]
pub(super) struct KeyboardStateHandle {
    status: KeyboardStateStatus,
}

#[derive(Debug, Default)]
enum KeyboardStateStatus {
    #[default]
    Uninitialized,
    Ready(u64),
    Failed,
}

static NEXT_KEYBOARD_STATE_ID: AtomicU64 = AtomicU64::new(1);

// xkbcommon intentionally keeps its keymap and state wrappers !Send and !Sync.
// Store the actual objects in the owning compositor thread's TLS and keep only
// a sendable identity in CompositorState. The identity is created lazily after
// a test/runtime server has reached its compositor thread; using it elsewhere
// fails closed instead of moving an XKB object across threads.
thread_local! {
    static KEYBOARD_STATES: RefCell<HashMap<u64, XkbKeyboardState>> =
        RefCell::new(HashMap::new());
}

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

    pub(super) fn from_environment() -> Result<Self, String> {
        let requested = KeyboardConfig::from_env();
        Self::from_candidates([
            ("requested", requested),
            ("baseline", KeyboardConfig::default()),
            ("minimal us", KeyboardConfig::minimal_us()),
        ])
    }

    fn from_candidates<'a>(
        candidates: impl IntoIterator<Item = (&'a str, KeyboardConfig)>,
    ) -> Result<Self, String> {
        let mut failures = Vec::new();
        for (label, config) in candidates {
            match Self::from_config(&config) {
                Ok(state) => return Ok(state),
                Err(error) => {
                    eprintln!(
                        "oblivion-one compositor: failed {label} keyboard configuration ({}): {error}",
                        describe_config(&config)
                    );
                    failures.push(format!("{label}: {error}"));
                }
            }
        }
        Err(format!(
            "all XKB keyboard configurations failed: {}",
            failures.join("; ")
        ))
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

    pub(super) fn send_initial_state(&self, keyboard: &wl_keyboard::WlKeyboard) -> bool {
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
                return false;
            }
        }

        if keyboard.version() >= WL_KEYBOARD_REPEAT_INFO_SINCE {
            let _ = keyboard.send_event(wl_keyboard::Event::RepeatInfo {
                rate: self.config.repeat_rate,
                delay: self.config.repeat_delay,
            });
        }
        true
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

impl KeyboardStateHandle {
    pub(super) fn ensure(&mut self) -> bool {
        self.ensure_with(XkbKeyboardState::from_environment)
    }

    fn ensure_with<F>(&mut self, initialize: F) -> bool
    where
        F: FnOnce() -> Result<XkbKeyboardState, String>,
    {
        match self.status {
            KeyboardStateStatus::Failed => return false,
            KeyboardStateStatus::Ready(id) => {
                let present = KEYBOARD_STATES.with(|states| states.borrow().contains_key(&id));
                if !present {
                    self.status = KeyboardStateStatus::Failed;
                    eprintln!(
                        "oblivion-one compositor: keyboard state was used outside its owning thread"
                    );
                }
                return present;
            }
            KeyboardStateStatus::Uninitialized => {}
        }

        match initialize() {
            Ok(state) => {
                let id = NEXT_KEYBOARD_STATE_ID.fetch_add(1, Ordering::Relaxed);
                KEYBOARD_STATES.with(|states| {
                    states.borrow_mut().insert(id, state);
                });
                self.status = KeyboardStateStatus::Ready(id);
                true
            }
            Err(error) => {
                self.status = KeyboardStateStatus::Failed;
                eprintln!("oblivion-one compositor: keyboard initialization disabled: {error}");
                false
            }
        }
    }

    pub(super) fn serialized_state(&self) -> Option<KeyboardSerializedState> {
        let KeyboardStateStatus::Ready(id) = self.status else {
            return None;
        };
        KEYBOARD_STATES.with(|states| {
            states
                .borrow()
                .get(&id)
                .map(XkbKeyboardState::serialized_state)
        })
    }

    pub(super) fn update_key(&mut self, evdev_key: u32, pressed: bool) -> bool {
        let KeyboardStateStatus::Ready(id) = self.status else {
            return false;
        };
        let changed = KEYBOARD_STATES.with(|states| {
            states
                .borrow_mut()
                .get_mut(&id)
                .map(|state| state.update_key(evdev_key, pressed))
        });
        match changed {
            Some(changed) => changed,
            None => {
                self.status = KeyboardStateStatus::Failed;
                eprintln!(
                    "oblivion-one compositor: keyboard state was used outside its owning thread"
                );
                false
            }
        }
    }

    pub(super) fn send_initial_state(&self, keyboard: &wl_keyboard::WlKeyboard) -> bool {
        let KeyboardStateStatus::Ready(id) = self.status else {
            return false;
        };
        KEYBOARD_STATES.with(|states| {
            states
                .borrow()
                .get(&id)
                .is_some_and(|state| state.send_initial_state(keyboard))
        })
    }
}

impl Drop for KeyboardStateHandle {
    fn drop(&mut self) {
        if let KeyboardStateStatus::Ready(id) = self.status {
            KEYBOARD_STATES.with(|states| {
                states.borrow_mut().remove(&id);
            });
        }
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
    fn layout_variant_resolution_distinguishes_absent_and_empty_overrides() {
        assert_eq!(
            resolve_layout_variant(None, None),
            ("br".to_string(), Some("abnt2".to_string()))
        );
        assert_eq!(
            resolve_layout_variant(Some("us"), None),
            ("us".to_string(), None)
        );
        assert_eq!(
            resolve_layout_variant(Some("br"), Some("abnt2")),
            ("br".to_string(), Some("abnt2".to_string()))
        );
        assert_eq!(
            resolve_layout_variant(Some("br,us"), Some("abnt2,")),
            ("br,us".to_string(), Some("abnt2,".to_string()))
        );
        assert_eq!(
            resolve_layout_variant(Some("br"), Some("")),
            ("br".to_string(), Some(String::new()))
        );
    }

    #[test]
    fn layout_only_us_override_compiles_without_inherited_variant() {
        let (layout, variant) = resolve_layout_variant(Some("us"), None);
        let config = KeyboardConfig {
            layout,
            variant,
            ..KeyboardConfig::default()
        };
        let state = XkbKeyboardState::from_config(&config).unwrap();
        assert_eq!(state.config.layout, "us");
        assert_eq!(state.config.variant, None);
    }

    #[test]
    fn explicitly_empty_variant_compiles_without_a_variant() {
        let (layout, variant) = resolve_layout_variant(Some("br"), Some(""));
        let config = KeyboardConfig {
            layout,
            variant,
            ..KeyboardConfig::default()
        };
        let state = XkbKeyboardState::from_config(&config).unwrap();
        assert_eq!(state.config.layout, "br");
        assert_eq!(state.config.variant.as_deref(), Some(""));
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
    fn invalid_requested_configuration_uses_the_fallback_chain() {
        let invalid = KeyboardConfig {
            layout: "invalid\0".into(),
            ..KeyboardConfig::default()
        };
        let state = XkbKeyboardState::from_candidates([
            ("requested", invalid),
            ("baseline", KeyboardConfig::default()),
            ("minimal us", KeyboardConfig::minimal_us()),
        ])
        .unwrap();
        assert_eq!(state.config, KeyboardConfig::default());
    }

    #[test]
    fn all_failed_configurations_are_reported() {
        let invalid = KeyboardConfig {
            layout: "invalid\0".into(),
            ..KeyboardConfig::default()
        };
        let error = XkbKeyboardState::from_candidates([
            ("requested", invalid.clone()),
            ("baseline", invalid.clone()),
            ("minimal us", invalid),
        ])
        .unwrap_err();
        assert!(error.contains("all XKB keyboard configurations failed"));
    }

    #[test]
    fn permanent_keyboard_initialization_failure_is_not_retried() {
        let mut handle = KeyboardStateHandle::default();
        let mut attempts = 0;
        assert!(!handle.ensure_with(|| {
            attempts += 1;
            Err("deterministic test failure".to_string())
        }));
        assert!(!handle.ensure_with(|| {
            attempts += 1;
            panic!("permanent failure must not retry initialization")
        }));
        assert_eq!(attempts, 1);
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
