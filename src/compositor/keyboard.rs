use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    env, fmt,
    fs::{self, File, OpenOptions},
    io::{self, Seek, Write},
    os::fd::AsFd,
    sync::atomic::{AtomicU64, Ordering},
};

use wayland_server::{Resource, WEnum, protocol::wl_keyboard};
use xkbcommon::xkb;

use super::runtime_files::unique_runtime_file_path;

mod xkb_compat {
    use std::os::raw::c_int;

    use xkbcommon::xkb;

    #[link(name = "xkbcommon")]
    unsafe extern "C" {
        fn xkb_state_update_latched_locked(
            state: *mut xkb::ffi::xkb_state,
            affect_latched_mods: u32,
            latched_mods: u32,
            affect_latched_layout: bool,
            latched_layout: i32,
            affect_locked_mods: u32,
            locked_mods: u32,
            affect_locked_layout: bool,
            locked_layout: i32,
        ) -> c_int;
    }

    pub(super) fn set_locked_layout(state: &xkb::State, index: i32) -> bool {
        // SAFETY: `state.get_raw_ptr()` points to this live compositor-thread
        // XKB state; libxkbcommon does not retain it, the state outlives this
        // call, no concurrent access exists, the index was validated before
        // entry, and this declaration matches xkbcommon.h on >= 1.10.0.
        unsafe {
            xkb_state_update_latched_locked(state.get_raw_ptr(), 0, 0, false, 0, 0, 0, true, index)
                != 0
        }
    }
}

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

    fn with_repeat_from(mut self, requested: &Self) -> Self {
        self.repeat_rate = requested.repeat_rate;
        self.repeat_delay = requested.repeat_delay;
        self
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct KeyboardLayoutState {
    pub(super) effective_index: u32,
    pub(super) locked_index: u32,
    pub(super) layouts: Vec<KeyboardLayoutEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct KeyboardLayoutEntry {
    pub(super) index: u32,
    pub(super) name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct KeyboardLayoutChange {
    pub(super) snapshot: KeyboardLayoutState,
    pub(super) changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum KeyboardLayoutError {
    Unavailable(&'static str),
    InvalidIndex { index: u32, count: u32 },
    IndexTooLarge(u32),
    FfiFailed,
}

pub(super) struct XkbKeyboardState {
    keymap: xkb::Keymap,
    physical_state: xkb::State,
    physical_pressed_keys: HashSet<u32>,
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

        let physical_state = xkb::State::new(&keymap);
        Ok(Self {
            keymap,
            physical_state,
            physical_pressed_keys: HashSet::new(),
            config: config.clone(),
            serialized_keymap_v1,
        })
    }

    pub(super) fn from_environment() -> Result<Self, String> {
        let requested = KeyboardConfig::from_env();
        let baseline = KeyboardConfig::default().with_repeat_from(&requested);
        let minimal_us = KeyboardConfig::minimal_us().with_repeat_from(&requested);
        Self::from_candidates([
            ("requested", requested),
            ("baseline", baseline),
            ("minimal us", minimal_us),
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

    pub(super) fn update_physical_key(&mut self, evdev_key: u32, pressed: bool) -> bool {
        let Some(keycode) = self.keycode_for_evdev(evdev_key) else {
            return false;
        };
        let before = self.wayland_serialized_state();
        let direction = if pressed {
            xkb::KeyDirection::Down
        } else {
            xkb::KeyDirection::Up
        };
        self.physical_state.update_key(keycode, direction);
        if pressed {
            self.physical_pressed_keys.insert(evdev_key);
        } else {
            self.physical_pressed_keys.remove(&evdev_key);
        }
        self.wayland_serialized_state() != before
    }

    pub(super) fn wayland_serialized_state(&self) -> KeyboardSerializedState {
        KeyboardSerializedState {
            depressed: self
                .physical_state
                .serialize_mods(xkb::STATE_MODS_DEPRESSED),
            latched: self.physical_state.serialize_mods(xkb::STATE_MODS_LATCHED),
            locked: self.physical_state.serialize_mods(xkb::STATE_MODS_LOCKED),
            group: self
                .physical_state
                .serialize_layout(xkb::STATE_LAYOUT_EFFECTIVE),
        }
    }

    pub(super) fn layout_snapshot(&self) -> Result<KeyboardLayoutState, KeyboardLayoutError> {
        let count = self.keymap.num_layouts();
        if count == 0 {
            return Err(KeyboardLayoutError::Unavailable("keymap has no layouts"));
        }
        let layouts = (0..count)
            .map(|index| KeyboardLayoutEntry {
                index,
                name: self.keymap.layout_get_name(index).to_string(),
            })
            .collect();
        Ok(KeyboardLayoutState {
            effective_index: self
                .physical_state
                .serialize_layout(xkb::STATE_LAYOUT_EFFECTIVE),
            locked_index: self
                .physical_state
                .serialize_layout(xkb::STATE_LAYOUT_LOCKED),
            layouts,
        })
    }

    pub(super) fn set_locked_layout(
        &mut self,
        index: u32,
    ) -> Result<KeyboardLayoutChange, KeyboardLayoutError> {
        let before = self.layout_snapshot()?;
        let count = u32::try_from(before.layouts.len()).unwrap_or(u32::MAX);
        if index >= count {
            return Err(KeyboardLayoutError::InvalidIndex { index, count });
        }
        if index == before.locked_index {
            return Ok(KeyboardLayoutChange {
                snapshot: before,
                changed: false,
            });
        }
        let index = i32::try_from(index).map_err(|_| KeyboardLayoutError::IndexTooLarge(index))?;
        if !xkb_compat::set_locked_layout(&self.physical_state, index) {
            return Err(KeyboardLayoutError::FfiFailed);
        }
        let snapshot = self.layout_snapshot()?;
        Ok(KeyboardLayoutChange {
            changed: snapshot != before,
            snapshot,
        })
    }

    pub(super) fn next_layout(&mut self) -> Result<KeyboardLayoutChange, KeyboardLayoutError> {
        let snapshot = self.layout_snapshot()?;
        let index = (snapshot.locked_index + 1) % snapshot.layouts.len() as u32;
        self.set_locked_layout(index)
    }

    pub(super) fn previous_layout(&mut self) -> Result<KeyboardLayoutChange, KeyboardLayoutError> {
        let snapshot = self.layout_snapshot()?;
        let index = (snapshot.locked_index + snapshot.layouts.len() as u32 - 1)
            % snapshot.layouts.len() as u32;
        self.set_locked_layout(index)
    }

    pub(super) fn clear_transient_key_state(&mut self) {
        let physical_keys = self.physical_pressed_keys.drain().collect::<Vec<_>>();
        for evdev_key in physical_keys {
            if let Some(keycode) = self.keycode_for_evdev(evdev_key) {
                self.physical_state
                    .update_key(keycode, xkb::KeyDirection::Up);
            }
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
        self.physical_state.led_name_is_active(name)
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

    pub(super) fn wayland_serialized_state(&self) -> Option<KeyboardSerializedState> {
        let KeyboardStateStatus::Ready(id) = self.status else {
            return None;
        };
        KEYBOARD_STATES.with(|states| {
            states
                .borrow()
                .get(&id)
                .map(XkbKeyboardState::wayland_serialized_state)
        })
    }

    pub(super) fn layout_snapshot(&mut self) -> Result<KeyboardLayoutState, KeyboardLayoutError> {
        let KeyboardStateStatus::Ready(id) = self.status else {
            return Err(KeyboardLayoutError::Unavailable(
                "keyboard state unavailable",
            ));
        };
        let snapshot = KEYBOARD_STATES.with(|states| {
            states
                .borrow()
                .get(&id)
                .map(XkbKeyboardState::layout_snapshot)
        });
        match snapshot {
            Some(snapshot) => snapshot,
            None => {
                self.status = KeyboardStateStatus::Failed;
                eprintln!(
                    "oblivion-one compositor: keyboard state was used outside its owning thread"
                );
                Err(KeyboardLayoutError::Unavailable(
                    "keyboard state unavailable",
                ))
            }
        }
    }

    pub(super) fn set_locked_layout(
        &mut self,
        index: u32,
    ) -> Result<KeyboardLayoutChange, KeyboardLayoutError> {
        let KeyboardStateStatus::Ready(id) = self.status else {
            return Err(KeyboardLayoutError::Unavailable(
                "keyboard state unavailable",
            ));
        };
        let change = KEYBOARD_STATES.with(|states| {
            states
                .borrow_mut()
                .get_mut(&id)
                .map(|state| state.set_locked_layout(index))
        });
        match change {
            Some(change) => change,
            None => {
                self.status = KeyboardStateStatus::Failed;
                eprintln!(
                    "oblivion-one compositor: keyboard state was used outside its owning thread"
                );
                Err(KeyboardLayoutError::Unavailable(
                    "keyboard state unavailable",
                ))
            }
        }
    }

    pub(super) fn next_layout(&mut self) -> Result<KeyboardLayoutChange, KeyboardLayoutError> {
        let KeyboardStateStatus::Ready(id) = self.status else {
            return Err(KeyboardLayoutError::Unavailable(
                "keyboard state unavailable",
            ));
        };
        let change = KEYBOARD_STATES.with(|states| {
            states
                .borrow_mut()
                .get_mut(&id)
                .map(XkbKeyboardState::next_layout)
        });
        match change {
            Some(change) => change,
            None => {
                self.status = KeyboardStateStatus::Failed;
                eprintln!(
                    "oblivion-one compositor: keyboard state was used outside its owning thread"
                );
                Err(KeyboardLayoutError::Unavailable(
                    "keyboard state unavailable",
                ))
            }
        }
    }

    pub(super) fn previous_layout(&mut self) -> Result<KeyboardLayoutChange, KeyboardLayoutError> {
        let KeyboardStateStatus::Ready(id) = self.status else {
            return Err(KeyboardLayoutError::Unavailable(
                "keyboard state unavailable",
            ));
        };
        let change = KEYBOARD_STATES.with(|states| {
            states
                .borrow_mut()
                .get_mut(&id)
                .map(XkbKeyboardState::previous_layout)
        });
        match change {
            Some(change) => change,
            None => {
                self.status = KeyboardStateStatus::Failed;
                eprintln!(
                    "oblivion-one compositor: keyboard state was used outside its owning thread"
                );
                Err(KeyboardLayoutError::Unavailable(
                    "keyboard state unavailable",
                ))
            }
        }
    }

    pub(super) fn update_physical_key(&mut self, evdev_key: u32, pressed: bool) -> bool {
        let KeyboardStateStatus::Ready(id) = self.status else {
            return false;
        };
        let changed = KEYBOARD_STATES.with(|states| {
            states
                .borrow_mut()
                .get_mut(&id)
                .map(|state| state.update_physical_key(evdev_key, pressed))
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

    pub(super) fn clear_transient_key_state(&mut self) {
        let KeyboardStateStatus::Ready(id) = self.status else {
            return;
        };
        let present = KEYBOARD_STATES.with(|states| {
            states.borrow_mut().get_mut(&id).is_some_and(|state| {
                state.clear_transient_key_state();
                true
            })
        });
        if !present {
            self.status = KeyboardStateStatus::Failed;
            eprintln!("oblivion-one compositor: keyboard state was used outside its owning thread");
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

    #[cfg(test)]
    pub(super) fn fail_for_test(&mut self) {
        self.status = KeyboardStateStatus::Failed;
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
            repeat_rate: 41,
            repeat_delay: 710,
            ..KeyboardConfig::default()
        };
        let state = XkbKeyboardState::from_candidates([
            ("requested", invalid.clone()),
            (
                "baseline",
                KeyboardConfig::default().with_repeat_from(&invalid),
            ),
            (
                "minimal us",
                KeyboardConfig::minimal_us().with_repeat_from(&invalid),
            ),
        ])
        .unwrap();
        assert_eq!(state.config.layout, KeyboardConfig::default().layout);
        assert_eq!(state.config.variant, KeyboardConfig::default().variant);
        assert_eq!(state.config.repeat_rate, 41);
        assert_eq!(state.config.repeat_delay, 710);
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
    fn uninitialized_keyboard_handle_initializes_on_current_thread() {
        let mut handle = KeyboardStateHandle::default();
        assert!(handle.ensure_with(|| XkbKeyboardState::from_config(&KeyboardConfig::default())));
        assert!(handle.wayland_serialized_state().is_some());
    }

    #[test]
    fn ready_keyboard_handle_fails_closed_on_a_different_thread() {
        let handle = std::thread::spawn(|| {
            let mut handle = KeyboardStateHandle::default();
            assert!(
                handle.ensure_with(|| XkbKeyboardState::from_config(&KeyboardConfig::default()))
            );
            assert!(handle.wayland_serialized_state().is_some());
            handle
        })
        .join()
        .unwrap();

        let mut handle = handle;
        assert!(!handle.ensure());
        assert!(handle.wayland_serialized_state().is_none());
    }

    #[test]
    fn layout_handle_fails_closed_on_a_different_thread() {
        let handle = std::thread::spawn(|| {
            let mut handle = KeyboardStateHandle::default();
            assert!(
                handle.ensure_with(|| XkbKeyboardState::from_config(&KeyboardConfig::default()))
            );
            handle
        })
        .join()
        .unwrap();

        let mut handle = handle;
        assert!(matches!(
            handle.layout_snapshot(),
            Err(KeyboardLayoutError::Unavailable(
                "keyboard state unavailable"
            ))
        ));
        assert!(!handle.ensure());
    }

    #[test]
    fn keyboard_state_handles_get_distinct_tls_ids() {
        let mut first = KeyboardStateHandle::default();
        let mut second = KeyboardStateHandle::default();
        assert!(first.ensure_with(|| XkbKeyboardState::from_config(&KeyboardConfig::default())));
        assert!(second.ensure_with(|| XkbKeyboardState::from_config(&KeyboardConfig::default())));
        let first_id = match &first.status {
            KeyboardStateStatus::Ready(id) => *id,
            _ => panic!("first keyboard handle did not initialize"),
        };
        let second_id = match &second.status {
            KeyboardStateStatus::Ready(id) => *id,
            _ => panic!("second keyboard handle did not initialize"),
        };
        assert_ne!(first_id, second_id);
    }

    #[test]
    fn update_physical_key_uses_xkb_offset_but_keeps_evdev_api() {
        let mut state = XkbKeyboardState::from_config(&KeyboardConfig::default()).unwrap();
        assert!(!state.update_physical_key(30, true));
        assert!(state.update_physical_key(42, true));
        assert_ne!(state.wayland_serialized_state().depressed, 0);
        assert_eq!(state.keymap_keycode_for_evdev(30), Some(38));
    }

    #[test]
    fn caps_lock_changes_xkb_locked_mask_without_manual_modifier_state() {
        let mut state = XkbKeyboardState::from_config(&KeyboardConfig::default()).unwrap();
        assert_eq!(state.wayland_serialized_state().locked, 0);
        state.update_physical_key(58, true);
        state.update_physical_key(58, false);
        assert_ne!(state.wayland_serialized_state().locked, 0);
        assert!(state.led_active(xkb::LED_NAME_CAPS));
    }

    #[test]
    fn right_alt_uses_the_keymap_level_three_modifier() {
        let mut state = XkbKeyboardState::from_config(&KeyboardConfig::default()).unwrap();
        let level_three = state.keymap.mod_get_index(xkb::MOD_NAME_ISO_LEVEL3_SHIFT);
        assert_ne!(level_three, xkb::MOD_INVALID);
        state.update_physical_key(100, true);
        assert!(
            state
                .physical_state
                .mod_index_is_active(level_three, xkb::STATE_MODS_DEPRESSED)
        );
        assert_ne!(state.wayland_serialized_state().depressed, 0);
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
        assert_eq!(state.wayland_serialized_state().group, 0);
        state.update_physical_key(56, true);
        state.update_physical_key(42, true);
        state.update_physical_key(42, false);
        state.update_physical_key(56, false);
        assert_eq!(state.wayland_serialized_state().group, 1);
    }

    #[test]
    fn runtime_layout_snapshot_comes_from_compiled_keymap() {
        let config = KeyboardConfig {
            layout: "br,us".into(),
            variant: Some("abnt2,".into()),
            ..KeyboardConfig::default()
        };
        let state = XkbKeyboardState::from_config(&config).unwrap();
        let snapshot = state.layout_snapshot().unwrap();
        assert_eq!(snapshot.effective_index, 0);
        assert_eq!(snapshot.locked_index, 0);
        assert_eq!(snapshot.layouts.len(), 2);
        assert_eq!(snapshot.layouts[0].index, 0);
        assert_eq!(snapshot.layouts[1].index, 1);
    }

    #[test]
    fn runtime_layout_set_preserves_physical_state_and_keymap_bytes() {
        let config = KeyboardConfig {
            layout: "br,us".into(),
            variant: Some("abnt2,".into()),
            ..KeyboardConfig::default()
        };
        let mut state = XkbKeyboardState::from_config(&config).unwrap();
        state.update_physical_key(42, true);
        let keymap_before = state.keymap_text_v1().to_string();
        let before = state.wayland_serialized_state();

        let change = state.set_locked_layout(1).unwrap();

        assert!(change.changed);
        assert_eq!(change.snapshot.locked_index, 1);
        assert_eq!(change.snapshot.effective_index, 1);
        assert_eq!(state.wayland_serialized_state().depressed, before.depressed);
        assert_eq!(state.wayland_serialized_state().latched, before.latched);
        assert_eq!(state.wayland_serialized_state().locked, before.locked);
        assert_eq!(state.keymap_text_v1(), keymap_before);
        assert!(state.physical_pressed_keys.contains(&42));
    }

    #[test]
    fn runtime_layout_set_preserves_held_shift_ctrl_and_right_alt() {
        let config = KeyboardConfig {
            layout: "br,us".into(),
            variant: Some("abnt2,".into()),
            ..KeyboardConfig::default()
        };
        let mut state = XkbKeyboardState::from_config(&config).unwrap();
        for evdev_key in [42, 29, 100] {
            state.update_physical_key(evdev_key, true);
        }
        let before = state.wayland_serialized_state();

        let change = state.set_locked_layout(1).unwrap();

        let after = state.wayland_serialized_state();
        assert!(change.changed);
        assert_eq!(after.depressed, before.depressed);
        assert_eq!(after.latched, before.latched);
        assert_eq!(after.locked, before.locked);
        assert_eq!(state.physical_pressed_keys.len(), 3);
        for evdev_key in [42, 29, 100] {
            state.update_physical_key(evdev_key, false);
        }
        assert_eq!(state.wayland_serialized_state().depressed, 0);
    }

    #[test]
    fn runtime_layout_navigation_wraps_from_locked_layout() {
        let config = KeyboardConfig {
            layout: "br,us".into(),
            variant: Some("abnt2,".into()),
            ..KeyboardConfig::default()
        };
        let mut state = XkbKeyboardState::from_config(&config).unwrap();
        assert_eq!(state.next_layout().unwrap().snapshot.locked_index, 1);
        assert_eq!(state.next_layout().unwrap().snapshot.locked_index, 0);
        assert_eq!(state.previous_layout().unwrap().snapshot.locked_index, 1);
    }

    #[test]
    fn runtime_layout_single_layout_operations_are_idempotent() {
        let mut state = XkbKeyboardState::from_config(&KeyboardConfig::default()).unwrap();
        assert!(!state.next_layout().unwrap().changed);
        assert!(!state.previous_layout().unwrap().changed);
        assert!(!state.set_locked_layout(0).unwrap().changed);
        assert_eq!(state.layout_snapshot().unwrap().locked_index, 0);
    }

    #[test]
    fn runtime_layout_selection_shares_authority_with_physical_group_actions() {
        let config = KeyboardConfig {
            layout: "br,us".into(),
            variant: Some("abnt2,".into()),
            options: Some("grp:alt_shift_toggle".into()),
            ..KeyboardConfig::default()
        };
        let mut state = XkbKeyboardState::from_config(&config).unwrap();
        let selected = state.set_locked_layout(1).unwrap();
        assert_eq!(selected.snapshot.locked_index, 1);
        assert_eq!(selected.snapshot.effective_index, 1);

        for (evdev_key, pressed) in [(56, true), (42, true), (42, false), (56, false)] {
            state.update_physical_key(evdev_key, pressed);
        }
        let after_physical = state.layout_snapshot().unwrap();
        assert_eq!(after_physical.locked_index, 0);
        assert_eq!(after_physical.effective_index, 0);
        assert_eq!(state.previous_layout().unwrap().snapshot.locked_index, 1);
    }

    #[test]
    fn runtime_layout_rejects_out_of_range_index_without_mutation() {
        let mut state = XkbKeyboardState::from_config(&KeyboardConfig::default()).unwrap();
        let before = state.layout_snapshot().unwrap();
        assert!(matches!(
            state.set_locked_layout(1),
            Err(KeyboardLayoutError::InvalidIndex { index: 1, count: 1 })
        ));
        assert_eq!(state.layout_snapshot().unwrap(), before);
    }

    #[test]
    fn authoritative_state_matches_reference_for_held_right_alt_across_group_switch() {
        let config = KeyboardConfig {
            layout: "us,br".into(),
            variant: Some(",abnt2".into()),
            options: Some("grp:alt_shift_toggle".into()),
            ..KeyboardConfig::default()
        };
        let mut state = XkbKeyboardState::from_config(&config).unwrap();
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let reference_keymap = xkb::Keymap::new_from_string(
            &context,
            state.keymap_text_v1().to_string(),
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .unwrap();
        let mut reference = xkb::State::new(&reference_keymap);
        let events = [
            (100, true),
            (56, true),
            (42, true),
            (42, false),
            (56, false),
        ];
        let serialize = |reference: &xkb::State| KeyboardSerializedState {
            depressed: reference.serialize_mods(xkb::STATE_MODS_DEPRESSED),
            latched: reference.serialize_mods(xkb::STATE_MODS_LATCHED),
            locked: reference.serialize_mods(xkb::STATE_MODS_LOCKED),
            group: reference.serialize_layout(xkb::STATE_LAYOUT_EFFECTIVE),
        };

        for &(evdev_key, pressed) in &events {
            state.update_physical_key(evdev_key, pressed);
            reference.update_key(
                xkb::Keycode::new(evdev_key + XKB_EVDEV_OFFSET),
                if pressed {
                    xkb::KeyDirection::Down
                } else {
                    xkb::KeyDirection::Up
                },
            );
            assert_eq!(state.wayland_serialized_state(), serialize(&reference));
        }

        state.update_physical_key(100, false);
        reference.update_key(xkb::Keycode::new(108), xkb::KeyDirection::Up);
        assert_eq!(state.wayland_serialized_state(), serialize(&reference));
    }

    #[test]
    fn visible_ctrl_shift_group_switch_matches_reference_state() {
        let config = KeyboardConfig {
            layout: "us,br".into(),
            variant: Some(",abnt2".into()),
            options: Some("grp:ctrl_shift_toggle".into()),
            ..KeyboardConfig::default()
        };
        let mut state = XkbKeyboardState::from_config(&config).unwrap();
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let reference_keymap = xkb::Keymap::new_from_string(
            &context,
            state.keymap_text_v1().to_string(),
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .unwrap();
        let mut reference = xkb::State::new(&reference_keymap);
        for evdev_key in [29, 42] {
            state.update_physical_key(evdev_key, true);
            reference.update_key(
                xkb::Keycode::new(evdev_key + XKB_EVDEV_OFFSET),
                xkb::KeyDirection::Down,
            );
        }
        let expected = KeyboardSerializedState {
            depressed: reference.serialize_mods(xkb::STATE_MODS_DEPRESSED),
            latched: reference.serialize_mods(xkb::STATE_MODS_LATCHED),
            locked: reference.serialize_mods(xkb::STATE_MODS_LOCKED),
            group: reference.serialize_layout(xkb::STATE_LAYOUT_EFFECTIVE),
        };
        assert_eq!(expected.group, 1);
        assert_eq!(state.wayland_serialized_state(), expected);
    }

    #[test]
    fn wayland_modifiers_publish_authoritative_physical_depressed_modifiers() {
        let mut state = XkbKeyboardState::from_config(&KeyboardConfig::default()).unwrap();
        let alt_index = state.keymap.mod_get_index(xkb::MOD_NAME_ALT);
        let shift_index = state.keymap.mod_get_index(xkb::MOD_NAME_SHIFT);
        assert_ne!(alt_index, xkb::MOD_INVALID);
        assert_ne!(shift_index, xkb::MOD_INVALID);

        state.update_physical_key(56, true);
        state.update_physical_key(42, true);
        let projected = state.wayland_serialized_state();
        let alt_mask = 1u32.checked_shl(alt_index).unwrap();
        let shift_mask = 1u32.checked_shl(shift_index).unwrap();
        assert_eq!(projected.depressed & alt_mask, alt_mask);
        assert_eq!(projected.depressed & shift_mask, shift_mask);

        state.update_physical_key(42, false);
        state.update_physical_key(56, false);
        assert_eq!(state.wayland_serialized_state().depressed, 0);
    }

    #[test]
    fn clearing_transient_keys_releases_authoritative_state_without_resetting_locks() {
        let mut state = XkbKeyboardState::from_config(&KeyboardConfig::default()).unwrap();
        state.update_physical_key(56, true);
        state.update_physical_key(58, true);
        let locked = state.wayland_serialized_state().locked;
        assert_ne!(locked, 0);

        state.clear_transient_key_state();

        assert_eq!(state.wayland_serialized_state().depressed, 0);
        assert_eq!(state.wayland_serialized_state().locked, locked);
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
