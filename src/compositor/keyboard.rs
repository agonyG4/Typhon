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

    pub(super) fn update_latched_locked(
        state: &xkb::State,
        affect_locked_mods: u32,
        locked_mods: u32,
        index: i32,
    ) -> Option<bool> {
        // SAFETY: `state.get_raw_ptr()` points to this live compositor-thread
        // XKB state; libxkbcommon does not retain it, the state outlives this
        // call, no concurrent access exists, the index was validated before
        // entry, and this declaration matches xkbcommon.h on >= 1.10.0.
        unsafe {
            let result = xkb_state_update_latched_locked(
                state.get_raw_ptr(),
                0,
                0,
                false,
                0,
                affect_locked_mods,
                locked_mods,
                true,
                index,
            );
            (result >= 0).then_some(result != 0)
        }
    }

    pub(super) fn set_locked_layout(state: &xkb::State, index: i32) -> bool {
        update_latched_locked(state, 0, 0, index).is_some_and(|changed| changed)
    }
}

const WL_KEYBOARD_REPEAT_INFO_SINCE: u32 = 4;
const XKB_EVDEV_OFFSET: u32 = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyboardConfig {
    pub rules: Option<String>,
    pub model: Option<String>,
    pub layout: String,
    pub variant: Option<String>,
    pub options: Option<String>,
    pub repeat_rate: i32,
    pub repeat_delay: i32,
    pub default_layout_index: u32,
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
            default_layout_index: 0,
        }
    }
}

impl KeyboardConfig {
    pub fn select_startup(
        persisted: Option<Self>,
    ) -> Result<
        (
            Self,
            crate::control_snapshots::KeyboardConfigurationSource,
            bool,
        ),
        String,
    > {
        XkbKeyboardState::select_startup(persisted)
    }

    pub(crate) fn validate_structure(&self) -> Result<(), String> {
        const MAX_FIELD_BYTES: usize = 4096;
        const MAX_CONFIGURATION_BYTES: usize = 16 * 1024;
        let fields = [
            ("rules", self.rules.as_deref()),
            ("model", self.model.as_deref()),
            ("layout", Some(self.layout.as_str())),
            ("variant", self.variant.as_deref()),
            ("options", self.options.as_deref()),
        ];
        if self.layout.is_empty() {
            return Err("keyboard layout must not be empty".to_string());
        }
        if self.repeat_rate < 0 || self.repeat_delay < 0 {
            return Err("keyboard repeat values must not be negative".to_string());
        }
        let mut total = 0usize;
        for (name, value) in fields {
            if let Some(value) = value {
                if value.contains('\0') {
                    return Err(format!("keyboard {name} contains NUL byte"));
                }
                if value.len() > MAX_FIELD_BYTES {
                    return Err(format!("keyboard {name} exceeds {MAX_FIELD_BYTES} bytes"));
                }
                total = total.saturating_add(value.len());
            }
        }
        total = total.saturating_add(std::mem::size_of::<i32>() * 2 + std::mem::size_of::<u32>());
        if total > MAX_CONFIGURATION_BYTES {
            return Err("keyboard configuration exceeds the bounded size".to_string());
        }
        Ok(())
    }

    pub(crate) fn has_rmlvo_difference(&self, other: &Self) -> bool {
        self.rules != other.rules
            || self.model != other.model
            || self.layout != other.layout
            || self.variant != other.variant
            || self.options != other.options
    }

    pub(crate) fn apply_environment_overrides(mut self) -> (Self, bool) {
        let layout_override = env_override("OBLIVION_ONE_XKB_LAYOUT");
        let variant_override = env_override("OBLIVION_ONE_XKB_VARIANT");
        let (layout, variant) = match (layout_override.as_deref(), variant_override.as_deref()) {
            (None, None) => (self.layout.clone(), self.variant.clone()),
            (None, Some(variant)) => (self.layout.clone(), Some(variant.to_string())),
            (Some(layout), variant) => (layout.to_string(), variant.map(str::to_string)),
        };
        self.rules = optional_env("OBLIVION_ONE_XKB_RULES").or(self.rules);
        self.model = optional_env("OBLIVION_ONE_XKB_MODEL").or(self.model);
        self.layout = layout;
        self.variant = variant;
        self.options = optional_env("OBLIVION_ONE_XKB_OPTIONS").or(self.options);
        self.repeat_rate = non_negative_env_i32("OBLIVION_ONE_XKB_REPEAT_RATE", self.repeat_rate);
        self.repeat_delay =
            non_negative_env_i32("OBLIVION_ONE_XKB_REPEAT_DELAY", self.repeat_delay);
        self.default_layout_index = non_negative_env_u32(
            "OBLIVION_ONE_XKB_DEFAULT_LAYOUT_INDEX",
            self.default_layout_index,
        );
        let active = [
            "OBLIVION_ONE_XKB_RULES",
            "OBLIVION_ONE_XKB_MODEL",
            "OBLIVION_ONE_XKB_LAYOUT",
            "OBLIVION_ONE_XKB_VARIANT",
            "OBLIVION_ONE_XKB_OPTIONS",
            "OBLIVION_ONE_XKB_REPEAT_RATE",
            "OBLIVION_ONE_XKB_REPEAT_DELAY",
            "OBLIVION_ONE_XKB_DEFAULT_LAYOUT_INDEX",
        ]
        .into_iter()
        .any(|name| env::var_os(name).is_some());
        (self, active)
    }

    pub(super) fn from_env() -> Self {
        Self::default().apply_environment_overrides().0
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
        self.default_layout_index = requested.default_layout_index;
        self
    }
}

const DEFAULT_LAYOUT: &str = "br";
const DEFAULT_VARIANT: &str = "abnt2";

#[cfg(test)]
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

fn non_negative_env_u32(name: &str, default: u32) -> u32 {
    match env::var(name)
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
    {
        Some(value) => value,
        None => default,
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
    pending_configuration: Option<PendingKeyboardConfiguration>,
    configuration_generation: u64,
}

pub(super) struct PreparedKeyboardConfiguration {
    pub(super) config: KeyboardConfig,
    pub(super) keymap: xkb::Keymap,
    pub(super) serialized_keymap_v1: Vec<u8>,
}

struct PendingKeyboardConfiguration {
    config: KeyboardConfig,
    candidate: Option<PreparedKeyboardConfiguration>,
    persisted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyboardConfigurationPreparation {
    NoOp,
    PersistOnly,
    RepeatOnly,
    DefaultOnly,
    RepeatAndDefault,
    Full,
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
        config.validate_structure()?;
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
        if keymap.num_layouts() == 0 {
            return Err(format!(
                "libxkbcommon returned no layouts for {}",
                describe_config(config)
            ));
        }
        if config.default_layout_index >= keymap.num_layouts() {
            return Err(format!(
                "default layout index {} is out of range for {} layouts",
                config.default_layout_index,
                keymap.num_layouts()
            ));
        }
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
        let default_layout = i32::try_from(config.default_layout_index)
            .map_err(|_| "default layout index is not representable".to_string())?;
        if xkb_compat::update_latched_locked(&physical_state, 0, 0, default_layout).is_none() {
            return Err("initial keyboard layout update failed".to_string());
        }
        Ok(Self {
            keymap,
            physical_state,
            physical_pressed_keys: HashSet::new(),
            config: config.clone(),
            serialized_keymap_v1,
            pending_configuration: None,
            configuration_generation: 1,
        })
    }

    fn compile_candidate(config: &KeyboardConfig) -> Result<PreparedKeyboardConfiguration, String> {
        let context = xkb::Context::new(xkb::CONTEXT_NO_ENVIRONMENT_NAMES);
        let keymap = xkb::Keymap::new_from_names(
            &context,
            config.rules.as_deref().unwrap_or(""),
            config.model.as_deref().unwrap_or(""),
            &config.layout,
            config.variant.as_deref().unwrap_or(""),
            config.options.clone(),
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .ok_or_else(|| format!("libxkbcommon rejected {}", describe_config(config)))?;
        if keymap.num_layouts() == 0 {
            return Err("compiled keyboard keymap has no layouts".to_string());
        }
        if config.default_layout_index >= keymap.num_layouts() {
            return Err(format!(
                "default layout index {} is out of range for {} layouts",
                config.default_layout_index,
                keymap.num_layouts()
            ));
        }
        let mut serialized_keymap_v1 = keymap
            .get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1)
            .into_bytes();
        if serialized_keymap_v1.last().copied() != Some(0) {
            serialized_keymap_v1.push(0);
        }
        if serialized_keymap_v1.len() <= 1 || serialized_keymap_v1.len() > 1024 * 1024 {
            return Err("compiled keyboard keymap has an invalid Text V1 size".to_string());
        }
        if serialized_keymap_v1[..serialized_keymap_v1.len() - 1].contains(&0) {
            return Err("compiled keyboard keymap contains an embedded NUL".to_string());
        }
        Ok(PreparedKeyboardConfiguration {
            config: config.clone(),
            keymap,
            serialized_keymap_v1,
        })
    }

    pub(super) fn configuration(&self) -> KeyboardConfig {
        self.config.clone()
    }

    pub(super) fn configuration_generation(&self) -> u64 {
        self.configuration_generation
    }

    pub(super) fn prepare_configuration(
        &mut self,
        config: KeyboardConfig,
    ) -> Result<KeyboardConfigurationPreparation, String> {
        if self.pending_configuration.is_some() {
            return Err("keyboard configuration mutation is busy".to_string());
        }
        config.validate_structure()?;
        if config == self.config {
            return Ok(KeyboardConfigurationPreparation::NoOp);
        }

        let rmlvo_changed = config.has_rmlvo_difference(&self.config);
        let repeat_changed = config.repeat_rate != self.config.repeat_rate
            || config.repeat_delay != self.config.repeat_delay;
        let default_changed = config.default_layout_index != self.config.default_layout_index;
        let preparation = if rmlvo_changed {
            let candidate = Self::compile_candidate(&config)?;
            self.pending_configuration = Some(PendingKeyboardConfiguration {
                config,
                candidate: Some(candidate),
                persisted: false,
            });
            KeyboardConfigurationPreparation::Full
        } else if default_changed {
            if config.default_layout_index >= self.keymap.num_layouts() {
                return Err(format!(
                    "default layout index {} is out of range for {} layouts",
                    config.default_layout_index,
                    self.keymap.num_layouts()
                ));
            }
            self.pending_configuration = Some(PendingKeyboardConfiguration {
                config,
                candidate: None,
                persisted: false,
            });
            if repeat_changed {
                KeyboardConfigurationPreparation::RepeatAndDefault
            } else {
                KeyboardConfigurationPreparation::DefaultOnly
            }
        } else {
            debug_assert!(repeat_changed);
            self.pending_configuration = Some(PendingKeyboardConfiguration {
                config,
                candidate: None,
                persisted: false,
            });
            KeyboardConfigurationPreparation::RepeatOnly
        };
        Ok(preparation)
    }

    pub(super) fn prepare_persistence_only(
        &mut self,
        config: KeyboardConfig,
    ) -> Result<KeyboardConfigurationPreparation, String> {
        if self.pending_configuration.is_some() {
            return Err("keyboard configuration mutation is busy".to_string());
        }
        config.validate_structure()?;
        if config != self.config {
            return Err("keyboard active configuration changed unexpectedly".to_string());
        }
        self.pending_configuration = Some(PendingKeyboardConfiguration {
            config,
            candidate: None,
            persisted: false,
        });
        Ok(KeyboardConfigurationPreparation::PersistOnly)
    }

    pub(super) fn has_pending_configuration(&self) -> bool {
        self.pending_configuration.is_some()
    }

    pub(super) fn pending_configuration_is_persisted(&self) -> bool {
        self.pending_configuration
            .as_ref()
            .is_some_and(|pending| pending.persisted)
    }

    pub(super) fn mark_pending_configuration_persisted(&mut self) -> Result<(), String> {
        let Some(pending) = self.pending_configuration.as_mut() else {
            return Err("keyboard configuration transaction is not pending".to_string());
        };
        pending.persisted = true;
        Ok(())
    }

    pub(super) fn abort_prepared_configuration(&mut self) -> bool {
        self.pending_configuration.take().is_some()
    }

    pub(super) fn commit_prepared_configuration(&mut self) -> Result<bool, String> {
        let Some(pending) = self.pending_configuration.as_ref() else {
            return Err("keyboard configuration transaction is not pending".to_string());
        };
        if !pending.persisted {
            return Err("keyboard configuration persistence is not complete".to_string());
        }
        if !self.physical_pressed_keys.is_empty() {
            return Err("keyboard configuration commit requires a quiescent keyboard".to_string());
        }
        let pending = self
            .pending_configuration
            .take()
            .expect("pending keyboard configuration checked above");
        let PendingKeyboardConfiguration {
            config,
            candidate,
            persisted: _,
        } = pending;
        if let Some(candidate) = candidate {
            debug_assert_eq!(candidate.config, config);
            let locked_names = self.locked_modifier_names();
            let new_state = xkb::State::new(&candidate.keymap);
            let locked_mask = locked_names.into_iter().fold(0u32, |mask, name| {
                let index = candidate.keymap.mod_get_index(&name);
                if index < 32 {
                    mask | (1u32 << index)
                } else {
                    mask
                }
            });
            let layout = i32::try_from(config.default_layout_index)
                .map_err(|_| "default layout index is not representable".to_string())?;
            let affect_locked_mods = if candidate.keymap.num_mods() >= 32 {
                u32::MAX
            } else {
                (1u32 << candidate.keymap.num_mods()) - 1
            };
            if xkb_compat::update_latched_locked(
                &new_state,
                affect_locked_mods,
                locked_mask,
                layout,
            )
            .is_none()
            {
                return Err("replacement keyboard state update failed".to_string());
            }
            self.keymap = candidate.keymap;
            self.physical_state = new_state;
            self.serialized_keymap_v1 = candidate.serialized_keymap_v1;
        } else if config.default_layout_index != self.config.default_layout_index {
            let layout = i32::try_from(config.default_layout_index)
                .map_err(|_| "default layout index is not representable".to_string())?;
            if !xkb_compat::set_locked_layout(&self.physical_state, layout) {
                return Err("keyboard default layout update failed".to_string());
            }
        }
        let changed = config != self.config;
        if changed {
            self.config = config;
            self.configuration_generation = self.configuration_generation.saturating_add(1);
        }
        Ok(changed)
    }

    fn locked_modifier_names(&self) -> Vec<String> {
        (0..self.keymap.num_mods())
            .filter(|index| {
                self.physical_state
                    .mod_index_is_active(*index, xkb::STATE_MODS_LOCKED)
            })
            .map(|index| self.keymap.mod_get_name(index).to_string())
            .collect()
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

    pub fn select_startup(
        persisted: Option<KeyboardConfig>,
    ) -> Result<
        (
            KeyboardConfig,
            crate::control_snapshots::KeyboardConfigurationSource,
            bool,
        ),
        String,
    > {
        let default = KeyboardConfig::default();
        let persisted_base = persisted.clone().unwrap_or_else(|| default.clone());
        let (environment, environment_override_active) =
            persisted_base.clone().apply_environment_overrides();
        let (default_environment, _) = default.clone().apply_environment_overrides();
        let environment_index_fallback = (environment_override_active
            && env::var_os("OBLIVION_ONE_XKB_DEFAULT_LAYOUT_INDEX").is_none())
        .then(|| KeyboardConfig {
            default_layout_index: 0,
            ..environment.clone()
        });
        let mut candidates = Vec::new();
        candidates.push((
            environment,
            if environment_override_active {
                crate::control_snapshots::KeyboardConfigurationSource::Environment
            } else if persisted.is_some() {
                crate::control_snapshots::KeyboardConfigurationSource::Persisted
            } else {
                crate::control_snapshots::KeyboardConfigurationSource::Default
            },
        ));
        if let Some(environment_index_fallback) = environment_index_fallback {
            candidates.push((
                environment_index_fallback,
                crate::control_snapshots::KeyboardConfigurationSource::Environment,
            ));
        }
        if environment_override_active {
            if persisted.is_some() {
                candidates.push((
                    persisted_base,
                    crate::control_snapshots::KeyboardConfigurationSource::Persisted,
                ));
            }
            candidates.push((
                default_environment,
                crate::control_snapshots::KeyboardConfigurationSource::Environment,
            ));
        }
        candidates.push((
            default.clone(),
            crate::control_snapshots::KeyboardConfigurationSource::Default,
        ));
        candidates.push((
            KeyboardConfig::minimal_us().with_repeat_from(&default),
            crate::control_snapshots::KeyboardConfigurationSource::Default,
        ));

        let mut failures = Vec::new();
        for (config, source) in candidates {
            match Self::from_config(&config) {
                Ok(state) => {
                    drop(state);
                    return Ok((config, source, environment_override_active));
                }
                Err(error) => failures.push(format!("{}: {error}", describe_config(&config))),
            }
        }
        Err(format!(
            "all startup keyboard configurations failed: {}",
            failures.join("; ")
        ))
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
        let before_wayland = self.wayland_serialized_state();
        let index = i32::try_from(index).map_err(|_| KeyboardLayoutError::IndexTooLarge(index))?;
        if !xkb_compat::set_locked_layout(&self.physical_state, index) {
            return Err(KeyboardLayoutError::FfiFailed);
        }
        let snapshot = self.layout_snapshot()?;
        let after_wayland = self.wayland_serialized_state();
        Ok(KeyboardLayoutChange {
            changed: before_wayland != after_wayland,
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

    pub(super) fn send_keymap(&self, keyboard: &wl_keyboard::WlKeyboard) -> bool {
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

        true
    }

    pub(super) fn send_repeat_info(&self, keyboard: &wl_keyboard::WlKeyboard) {
        if keyboard.version() >= WL_KEYBOARD_REPEAT_INFO_SINCE {
            let _ = keyboard.send_event(wl_keyboard::Event::RepeatInfo {
                rate: self.config.repeat_rate,
                delay: self.config.repeat_delay,
            });
        }
    }

    pub(super) fn send_initial_state(&self, keyboard: &wl_keyboard::WlKeyboard) -> bool {
        if !self.send_keymap(keyboard) {
            return false;
        }
        self.send_repeat_info(keyboard);
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

    pub(super) fn ensure_with_config(&mut self, config: KeyboardConfig) -> bool {
        self.ensure_with(|| XkbKeyboardState::from_config(&config))
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

    pub(super) fn send_keymap(&self, keyboard: &wl_keyboard::WlKeyboard) -> bool {
        let KeyboardStateStatus::Ready(id) = self.status else {
            return false;
        };
        KEYBOARD_STATES.with(|states| {
            states
                .borrow()
                .get(&id)
                .is_some_and(|state| state.send_keymap(keyboard))
        })
    }

    pub(super) fn send_repeat_info(&self, keyboard: &wl_keyboard::WlKeyboard) {
        let KeyboardStateStatus::Ready(id) = self.status else {
            return;
        };
        KEYBOARD_STATES.with(|states| {
            if let Some(state) = states.borrow().get(&id) {
                state.send_repeat_info(keyboard);
            }
        });
    }

    pub(super) fn configuration(&self) -> Option<KeyboardConfig> {
        let KeyboardStateStatus::Ready(id) = self.status else {
            return None;
        };
        KEYBOARD_STATES.with(|states| {
            states
                .borrow()
                .get(&id)
                .map(XkbKeyboardState::configuration)
        })
    }

    pub(super) fn configuration_generation(&self) -> Option<u64> {
        let KeyboardStateStatus::Ready(id) = self.status else {
            return None;
        };
        KEYBOARD_STATES.with(|states| {
            states
                .borrow()
                .get(&id)
                .map(XkbKeyboardState::configuration_generation)
        })
    }

    pub(super) fn prepare_configuration(
        &mut self,
        config: KeyboardConfig,
    ) -> Result<KeyboardConfigurationPreparation, String> {
        let KeyboardStateStatus::Ready(id) = self.status else {
            return Err("keyboard state unavailable".to_string());
        };
        let result = KEYBOARD_STATES.with(|states| {
            states
                .borrow_mut()
                .get_mut(&id)
                .map(|state| state.prepare_configuration(config))
        });
        match result {
            Some(result) => result,
            None => {
                self.status = KeyboardStateStatus::Failed;
                Err("keyboard state unavailable".to_string())
            }
        }
    }

    pub(super) fn prepare_persistence_only(
        &mut self,
        config: KeyboardConfig,
    ) -> Result<KeyboardConfigurationPreparation, String> {
        let KeyboardStateStatus::Ready(id) = self.status else {
            return Err("keyboard state unavailable".to_string());
        };
        let result = KEYBOARD_STATES.with(|states| {
            states
                .borrow_mut()
                .get_mut(&id)
                .map(|state| state.prepare_persistence_only(config))
        });
        match result {
            Some(result) => result,
            None => {
                self.status = KeyboardStateStatus::Failed;
                Err("keyboard state unavailable".to_string())
            }
        }
    }

    pub(super) fn has_pending_configuration(&self) -> bool {
        let KeyboardStateStatus::Ready(id) = self.status else {
            return false;
        };
        KEYBOARD_STATES.with(|states| {
            states
                .borrow()
                .get(&id)
                .is_some_and(XkbKeyboardState::has_pending_configuration)
        })
    }

    pub(super) fn pending_configuration_is_persisted(&self) -> bool {
        let KeyboardStateStatus::Ready(id) = self.status else {
            return false;
        };
        KEYBOARD_STATES.with(|states| {
            states
                .borrow()
                .get(&id)
                .is_some_and(XkbKeyboardState::pending_configuration_is_persisted)
        })
    }

    pub(super) fn mark_pending_configuration_persisted(&mut self) -> Result<(), String> {
        let KeyboardStateStatus::Ready(id) = self.status else {
            return Err("keyboard state unavailable".to_string());
        };
        KEYBOARD_STATES.with(|states| {
            states.borrow_mut().get_mut(&id).map_or_else(
                || Err("keyboard state unavailable".to_string()),
                XkbKeyboardState::mark_pending_configuration_persisted,
            )
        })
    }

    pub(super) fn abort_prepared_configuration(&mut self) -> bool {
        let KeyboardStateStatus::Ready(id) = self.status else {
            return false;
        };
        KEYBOARD_STATES.with(|states| {
            states
                .borrow_mut()
                .get_mut(&id)
                .is_some_and(XkbKeyboardState::abort_prepared_configuration)
        })
    }

    pub(super) fn keyboard_physical_keys_quiescent(&self) -> bool {
        let KeyboardStateStatus::Ready(id) = self.status else {
            return false;
        };
        KEYBOARD_STATES.with(|states| {
            states
                .borrow()
                .get(&id)
                .is_some_and(|state| state.physical_pressed_keys.is_empty())
        })
    }

    pub(super) fn commit_prepared_configuration(&mut self) -> Result<bool, String> {
        let KeyboardStateStatus::Ready(id) = self.status else {
            return Err("keyboard state unavailable".to_string());
        };
        KEYBOARD_STATES.with(|states| {
            states.borrow_mut().get_mut(&id).map_or_else(
                || Err("keyboard state unavailable".to_string()),
                XkbKeyboardState::commit_prepared_configuration,
            )
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
        assert_eq!(config.default_layout_index, 0);
    }

    #[test]
    fn keyboard_config_json_preserves_nullable_rmlvo_fields_and_default_index() {
        let value = crate::keyboard_persistence::KeyboardConfigurationValue {
            rules: None,
            model: Some(String::new()),
            layout: "br,us".to_string(),
            variant: Some("abnt2,".to_string()),
            options: None,
            repeat_rate: 40,
            repeat_delay: 300,
            default_layout_index: 1,
        };
        let encoded = serde_json::to_value(&value).expect("keyboard config JSON");
        assert_eq!(encoded["model"], serde_json::json!(""));
        assert_eq!(encoded["variant"], serde_json::json!("abnt2,"));
        assert_eq!(encoded["defaultLayoutIndex"], serde_json::json!(1));

        let decoded: crate::keyboard_persistence::KeyboardConfigurationValue =
            serde_json::from_value(encoded).expect("keyboard config JSON round trip");
        assert_eq!(decoded, value);
    }

    #[test]
    fn keyboard_config_json_rejects_unknown_fields_and_wrong_version() {
        let unknown = serde_json::json!({
            "version": 1,
            "rules": null,
            "model": null,
            "layout": "us",
            "variant": null,
            "options": null,
            "repeatRate": 25,
            "repeatDelay": 600,
            "defaultLayoutIndex": 0,
            "compiledKeymap": "must not persist"
        });
        assert!(
            serde_json::from_value::<crate::keyboard_persistence::KeyboardConfigurationDocument>(
                unknown
            )
            .is_err()
        );

        let wrong_version = serde_json::json!({
            "version": 2,
            "rules": null,
            "model": null,
            "layout": "us",
            "variant": null,
            "options": null,
            "repeatRate": 25,
            "repeatDelay": 600,
            "defaultLayoutIndex": 0
        });
        let parsed = serde_json::from_value::<
            crate::keyboard_persistence::KeyboardConfigurationDocument,
        >(wrong_version)
        .expect("wrong version remains syntactically readable");
        assert!(parsed.into_config().is_err());
    }

    #[test]
    fn runtime_configuration_candidate_is_validated_before_active_swap() {
        let mut state = XkbKeyboardState::from_config(&KeyboardConfig::default()).unwrap();
        let active_bytes = state.keymap_text_v1().to_string();
        let invalid = KeyboardConfig {
            layout: "not-a-real-layout".to_string(),
            variant: None,
            ..KeyboardConfig::default()
        };
        assert!(state.prepare_configuration(invalid).is_err());
        assert!(!state.has_pending_configuration());
        assert_eq!(state.keymap_text_v1(), active_bytes);

        let replacement = KeyboardConfig {
            layout: "us".to_string(),
            variant: None,
            ..KeyboardConfig::default()
        };
        assert!(matches!(
            state.prepare_configuration(replacement.clone()),
            Ok(KeyboardConfigurationPreparation::Full)
        ));
        assert_eq!(state.configuration(), KeyboardConfig::default());
        state.mark_pending_configuration_persisted().unwrap();
        assert!(state.commit_prepared_configuration().unwrap());
        assert_eq!(state.configuration(), replacement);
        assert_ne!(state.keymap_text_v1(), active_bytes);
        assert!(!state.has_pending_configuration());
    }

    #[test]
    fn startup_restoration_applies_the_persisted_default_layout_index() {
        let config = KeyboardConfig {
            layout: "br,us".to_string(),
            variant: Some("abnt2,".to_string()),
            default_layout_index: 1,
            ..KeyboardConfig::default()
        };
        let state = XkbKeyboardState::from_config(&config).unwrap();
        assert_eq!(state.layout_snapshot().unwrap().locked_index, 1);
    }

    #[test]
    fn invalid_variant_options_and_default_index_leave_active_keymap_untouched() {
        let mut state = XkbKeyboardState::from_config(&KeyboardConfig::default()).unwrap();
        let active = state.keymap_text_v1().to_string();
        for candidate in [
            KeyboardConfig {
                layout: "us".to_string(),
                variant: Some("not-a-real-variant".to_string()),
                ..KeyboardConfig::default()
            },
            KeyboardConfig {
                options: Some("grp:\0".to_string()),
                ..KeyboardConfig::default()
            },
            KeyboardConfig {
                default_layout_index: 1,
                ..KeyboardConfig::default()
            },
        ] {
            assert!(state.prepare_configuration(candidate).is_err());
            assert_eq!(state.keymap_text_v1(), active);
            assert_eq!(state.configuration(), KeyboardConfig::default());
        }
    }

    #[test]
    fn repeat_and_default_only_configuration_changes_do_not_recompile() {
        let mut state = XkbKeyboardState::from_config(&KeyboardConfig {
            layout: "br,us".to_string(),
            variant: Some("abnt2,".to_string()),
            ..KeyboardConfig::default()
        })
        .unwrap();
        let keymap_before = state.keymap_text_v1().to_string();
        let same = state.configuration();
        assert!(matches!(
            state.prepare_persistence_only(same),
            Ok(KeyboardConfigurationPreparation::PersistOnly)
        ));
        state.mark_pending_configuration_persisted().unwrap();
        assert!(!state.commit_prepared_configuration().unwrap());
        assert_eq!(state.keymap_text_v1(), keymap_before);

        let repeat = KeyboardConfig {
            repeat_rate: 40,
            repeat_delay: 300,
            ..state.configuration()
        };
        assert!(matches!(
            state.prepare_configuration(repeat),
            Ok(KeyboardConfigurationPreparation::RepeatOnly)
        ));
        state.mark_pending_configuration_persisted().unwrap();
        assert!(state.commit_prepared_configuration().unwrap());
        assert_eq!(state.keymap_text_v1(), keymap_before);

        let default_layout = KeyboardConfig {
            default_layout_index: 1,
            ..state.configuration()
        };
        assert!(matches!(
            state.prepare_configuration(default_layout),
            Ok(KeyboardConfigurationPreparation::DefaultOnly)
        ));
        state.mark_pending_configuration_persisted().unwrap();
        assert!(state.commit_prepared_configuration().unwrap());
        assert_eq!(state.keymap_text_v1(), keymap_before);
        assert_eq!(state.layout_snapshot().unwrap().locked_index, 1);
    }

    #[test]
    fn full_configuration_commit_migrates_named_locks_and_clears_transient_state() {
        let mut state = XkbKeyboardState::from_config(&KeyboardConfig::default()).unwrap();
        state.update_physical_key(58, true);
        state.update_physical_key(58, false);
        state.update_physical_key(42, true);
        assert!(state.led_active(xkb::LED_NAME_CAPS));

        let replacement = KeyboardConfig {
            layout: "us".to_string(),
            variant: None,
            ..KeyboardConfig::default()
        };
        assert!(matches!(
            state.prepare_configuration(replacement),
            Ok(KeyboardConfigurationPreparation::Full)
        ));
        state.mark_pending_configuration_persisted().unwrap();
        assert!(state.commit_prepared_configuration().is_err());
        state.update_physical_key(42, false);
        assert!(state.commit_prepared_configuration().unwrap());
        assert!(state.led_active(xkb::LED_NAME_CAPS));
        let serialized = state.wayland_serialized_state();
        assert_eq!(serialized.depressed, 0);
        assert_eq!(serialized.latched, 0);
        assert!(state.physical_pressed_keys.is_empty());
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

    #[test]
    fn startup_environment_layout_override_retries_persisted_index_at_zero() {
        let _guard = ENV_LOCK.lock().unwrap();
        let previous_layout = env::var_os("OBLIVION_ONE_XKB_LAYOUT");
        let previous_variant = env::var_os("OBLIVION_ONE_XKB_VARIANT");
        let previous_default_index = env::var_os("OBLIVION_ONE_XKB_DEFAULT_LAYOUT_INDEX");
        // SAFETY: the test serializes process-wide environment changes.
        unsafe {
            env::set_var("OBLIVION_ONE_XKB_LAYOUT", "us");
            env::remove_var("OBLIVION_ONE_XKB_VARIANT");
            env::remove_var("OBLIVION_ONE_XKB_DEFAULT_LAYOUT_INDEX");
        }
        let persisted = KeyboardConfig {
            layout: "br,us".to_string(),
            variant: Some("abnt2,".to_string()),
            default_layout_index: 1,
            ..KeyboardConfig::default()
        };
        let (selected, source, environment_active) =
            KeyboardConfig::select_startup(Some(persisted)).expect("startup configuration");
        // SAFETY: restore the process-wide environment after the serialized test.
        unsafe {
            match previous_layout {
                Some(value) => env::set_var("OBLIVION_ONE_XKB_LAYOUT", value),
                None => env::remove_var("OBLIVION_ONE_XKB_LAYOUT"),
            }
            match previous_variant {
                Some(value) => env::set_var("OBLIVION_ONE_XKB_VARIANT", value),
                None => env::remove_var("OBLIVION_ONE_XKB_VARIANT"),
            }
            match previous_default_index {
                Some(value) => env::set_var("OBLIVION_ONE_XKB_DEFAULT_LAYOUT_INDEX", value),
                None => env::remove_var("OBLIVION_ONE_XKB_DEFAULT_LAYOUT_INDEX"),
            }
        }
        assert_eq!(selected.layout, "us");
        assert_eq!(selected.variant, None);
        assert_eq!(selected.default_layout_index, 0);
        assert_eq!(
            source,
            crate::control_snapshots::KeyboardConfigurationSource::Environment
        );
        assert!(environment_active);
    }
}
