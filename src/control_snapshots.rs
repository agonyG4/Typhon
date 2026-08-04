use serde::{Deserialize, Deserializer, Serialize};

pub const MAX_CONTROL_OUTPUTS: usize = 32;
pub const MAX_CONTROL_WINDOWS: usize = 4096;
pub const MAX_CONTROL_DOCTOR_CHECKS: usize = 128;
pub const MAX_CONTROL_NAME_BYTES: usize = 256;
pub const MAX_CONTROL_TITLE_BYTES: usize = 1024;
pub const MAX_CONTROL_DETAIL_BYTES: usize = 4096;
pub const SNAPSHOT_RESPONSE_OVERHEAD_BYTES: usize = 8192;
pub const MAX_WINDOWS_SNAPSHOT_BYTES: usize =
    crate::control::MAX_RESPONSE_BYTES.saturating_sub(SNAPSHOT_RESPONSE_OVERHEAD_BYTES);

fn deserialize_required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum ControlSessionState {
    Active,
    Suspended,
    Recovering,
    Failed,
}

impl ControlSessionState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Suspended => "suspended",
            Self::Recovering => "recovering",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum DoctorSeverity {
    Ok,
    Warning,
    Error,
}

impl DoctorSeverity {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum WindowKindSnapshot {
    XdgToplevel,
    X11,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum FeatureState {
    Unavailable,
    Configured,
    Available,
    Active,
    Degraded,
}

impl FeatureState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::Configured => "configured",
            Self::Available => "available",
            Self::Active => "active",
            Self::Degraded => "degraded",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum CursorBackendSnapshot {
    Hardware,
    Software,
    Hidden,
    Unavailable,
}

impl CursorBackendSnapshot {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hardware => "hardware",
            Self::Software => "software",
            Self::Hidden => "hidden",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum CursorConfigSource {
    Default,
    Config,
    Control,
}

impl CursorConfigSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Config => "config",
            Self::Control => "control",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum CursorPersistenceSnapshot {
    Saved,
    Missing,
    Invalid,
    Insecure,
    WriteFailed,
}

impl CursorPersistenceSnapshot {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Saved => "saved",
            Self::Missing => "missing",
            Self::Invalid => "invalid",
            Self::Insecure => "insecure",
            Self::WriteFailed => "write_failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum CursorAssetSource {
    SystemTheme,
    BuiltinFallback,
}

impl CursorAssetSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SystemTheme => "system_theme",
            Self::BuiltinFallback => "builtin_fallback",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct ControlWindowId(pub u64);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct VersionSnapshot {
    pub protocol_version: u32,
    pub compositor_name: String,
    pub compositor_version: String,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub git_commit: Option<String>,
    pub build_profile: String,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub rustc_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct XwaylandStatusSnapshot {
    pub configured: bool,
    pub state: String,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub generation: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct ControlStatusSnapshot {
    pub endpoint_active: bool,
    pub client_count: u32,
    pub accepted: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct StatusSnapshot {
    pub instance: String,
    pub wayland_display: String,
    pub uptime_ms: u64,
    pub session_state: ControlSessionState,
    pub shutdown_state: String,
    pub output_count: u32,
    pub mapped_window_count: u32,
    pub minimized_window_count: u32,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub active_window: Option<ControlWindowId>,
    pub xwayland: XwaylandStatusSnapshot,
    pub control: ControlStatusSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct DoctorCheck {
    pub id: String,
    pub severity: DoctorSeverity,
    pub summary: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct DoctorSnapshot {
    pub healthy: bool,
    pub checks: Vec<DoctorCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct ModeSnapshot {
    pub width: u32,
    pub height: u32,
    pub refresh_millihz: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct PositionSnapshot {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct PhysicalSizeSnapshot {
    pub width_mm: u32,
    pub height_mm: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct FeatureStateSnapshot {
    pub state: FeatureState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct OutputSnapshot {
    pub id: String,
    pub name: String,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub make: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub model: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub serial: Option<String>,
    pub enabled: bool,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub current_mode: Option<ModeSnapshot>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub physical_size_mm: Option<PhysicalSizeSnapshot>,
    pub scale_milli: u32,
    pub transform: String,
    pub position: PositionSnapshot,
    pub focused: bool,
    pub backend: String,
    pub vrr: FeatureStateSnapshot,
    pub direct_scanout: FeatureStateSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct OutputListSnapshot {
    pub outputs: Vec<OutputSnapshot>,
    pub total: u32,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct GeometrySnapshot {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct WindowSnapshot {
    pub id: ControlWindowId,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub app_id: Option<String>,
    pub title: String,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub pid: Option<u32>,
    pub kind: WindowKindSnapshot,
    pub mapped: bool,
    pub active: bool,
    pub minimized: bool,
    pub maximized: bool,
    pub fullscreen: bool,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub urgent: Option<bool>,
    pub skip_taskbar: bool,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub workspace: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub output: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub geometry: Option<GeometrySnapshot>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub focus_serial: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct WindowListSnapshot {
    pub windows: Vec<WindowSnapshot>,
    pub total: u32,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct ActiveWindowSnapshot {
    #[serde(deserialize_with = "deserialize_required_option")]
    pub window: Option<WindowSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct CursorSnapshot {
    pub desired_theme: String,
    pub desired_size_px: u32,
    pub active_theme: String,
    pub active_size_px: u32,
    pub generation: u64,
    pub backend: CursorBackendSnapshot,
    pub source: CursorConfigSource,
    pub persistence: CursorPersistenceSnapshot,
    pub asset_source: CursorAssetSource,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum AstreactlResult {
    Version(VersionSnapshot),
    Status(StatusSnapshot),
    Doctor(DoctorSnapshot),
    Outputs(OutputListSnapshot),
    Windows(WindowListSnapshot),
    ActiveWindow(ActiveWindowSnapshot),
    Cursor(CursorSnapshot),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotBudget {
    remaining: usize,
}

impl SnapshotBudget {
    pub const fn windows() -> Self {
        Self {
            remaining: MAX_WINDOWS_SNAPSHOT_BYTES,
        }
    }

    pub const fn remaining(self) -> usize {
        self.remaining
    }

    pub fn reserve(&mut self, bytes: usize) -> bool {
        if bytes > self.remaining {
            return false;
        }
        self.remaining -= bytes;
        true
    }
}

pub fn bounded_window_list<I>(
    total: u32,
    snapshots: I,
) -> Result<WindowListSnapshot, serde_json::Error>
where
    I: IntoIterator<Item = WindowSnapshot>,
{
    let mut budget = SnapshotBudget::windows();
    let _ = budget.reserve(2);
    let mut windows = Vec::new();
    let mut truncated = false;
    for (index, window) in snapshots.into_iter().enumerate() {
        if index >= MAX_CONTROL_WINDOWS {
            truncated = true;
            break;
        }
        let object_bytes = serde_json::to_vec(&window)?;
        let separator_bytes = usize::from(!windows.is_empty());
        if !budget.reserve(object_bytes.len() + separator_bytes) {
            truncated = true;
            break;
        }
        windows.push(window);
    }
    Ok(WindowListSnapshot {
        windows,
        total,
        truncated,
    })
}

pub fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_objects_are_deserializable_and_require_all_fields() {
        let result = serde_json::from_value::<StatusSnapshot>(serde_json::json!({}));
        assert!(result.is_err());
    }

    #[test]
    fn worst_case_window_list_is_bounded_before_response_encoding() {
        let window = WindowSnapshot {
            id: ControlWindowId(1),
            app_id: Some("a".repeat(MAX_CONTROL_NAME_BYTES)),
            title: "😀".repeat(MAX_CONTROL_TITLE_BYTES / 4),
            pid: Some(u32::MAX),
            kind: WindowKindSnapshot::X11,
            mapped: true,
            active: true,
            minimized: false,
            maximized: false,
            fullscreen: false,
            urgent: Some(true),
            skip_taskbar: false,
            workspace: Some("w".repeat(MAX_CONTROL_NAME_BYTES)),
            output: Some("o".repeat(MAX_CONTROL_NAME_BYTES)),
            geometry: Some(GeometrySnapshot {
                x: i32::MAX,
                y: i32::MIN,
                width: u32::MAX,
                height: u32::MAX,
            }),
            focus_serial: Some(u64::MAX),
        };
        let snapshot =
            bounded_window_list(4096, std::iter::repeat_n(window, MAX_CONTROL_WINDOWS + 1))
                .unwrap();
        assert!(snapshot.truncated);
        assert_eq!(snapshot.total, 4096);
        assert!(snapshot.windows.len() < MAX_CONTROL_WINDOWS);
        let result = serde_json::to_value(snapshot).unwrap();
        let response = crate::control::ControlResponse::success(1, result);
        let encoded = crate::control::encode_response(&response).unwrap();
        assert!(encoded.len() < crate::control::MAX_RESPONSE_BYTES);
    }

    #[test]
    fn truncation_preserves_utf8_boundaries() {
        assert_eq!(truncate_utf8("abc", 2), "ab");
        assert_eq!(truncate_utf8("éé", 3), "é");
        assert_eq!(truncate_utf8("😀x", 3), "");
    }

    #[test]
    fn feature_state_names_are_stable_and_exhaustive() {
        let states = [
            (FeatureState::Unavailable, "unavailable"),
            (FeatureState::Configured, "configured"),
            (FeatureState::Available, "available"),
            (FeatureState::Active, "active"),
            (FeatureState::Degraded, "degraded"),
        ];
        for (state, expected) in states {
            assert_eq!(state.as_str(), expected);
        }
    }
}
