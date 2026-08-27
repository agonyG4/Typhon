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
    #[serde(deserialize_with = "deserialize_required_option")]
    pub detail: Option<String>,
}

#[cfg(test)]
mod doctor_schema_tests {
    use super::DoctorCheck;
    use serde_json::json;

    #[test]
    fn doctor_check_requires_the_detail_field_on_the_wire() {
        let result = serde_json::from_value::<DoctorCheck>(json!({
            "id": "cursor.configuration",
            "severity": "ok",
            "summary": "ready"
        }));

        assert!(result.is_err());
    }
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct DecorationThemeSnapshot {
    pub selected_theme: String,
    pub active_theme: String,
    pub schema_version: u32,
    pub generation: u64,
    pub source: String,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct DecorationThemeListSnapshot {
    pub themes: Vec<String>,
    pub selected_theme: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct WallpaperDescriptorSnapshot {
    pub kind: String,
    pub source_kind: String,
    #[serde(default)]
    pub origin: String,
    pub fit: String,
    pub scope: String,
    pub logical_id: String,
    pub source: String,
    #[serde(default)]
    pub resolved_source: String,
    #[serde(default)]
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct WallpaperSnapshot {
    pub configured: Option<WallpaperDescriptorSnapshot>,
    pub factory_default: WallpaperDescriptorSnapshot,
    pub effective: WallpaperDescriptorSnapshot,
    pub state: String,
    pub fallback: String,
    pub generation: u64,
    pub error_code: String,
    pub last_error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct WallpaperListSnapshot {
    pub wallpapers: Vec<WallpaperDescriptorSnapshot>,
    pub snapshot: WallpaperSnapshot,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct TimingSummarySnapshot {
    pub count: u64,
    pub total_us: u64,
    pub last_us: u64,
    pub mean_us: u64,
    pub p50_us: u64,
    pub p95_us: u64,
    pub p99_us: u64,
    pub max_us: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct SignedTimingSummarySnapshot {
    pub count: u64,
    pub total_us: i64,
    pub last_us: i64,
    pub mean_us: i64,
    pub p50_us: i64,
    pub p95_us: i64,
    pub p99_us: i64,
    pub min_us: i64,
    pub max_us: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct RepaintPerformanceSnapshot {
    pub skip_frames: u64,
    pub partial_frames: u64,
    pub full_frames: u64,
    pub buffer_age_buckets: std::collections::BTreeMap<String, u64>,
    pub partial_repair_pixels: u64,
    pub full_output_pixels: u64,
    pub full_repaint_reasons: std::collections::BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct BufferingPerformanceSnapshot {
    pub reactive_double_frames: u64,
    pub predictive_triple_frames: u64,
    pub future_primary_credit: u8,
    pub extra_credit_grants: u64,
    pub extra_credit_revokes: u64,
    pub o1_credit2_useful_hits: u64,
    pub o1_credit2_unnecessary_hits: u64,
    pub o1_credit2_ineffective_misses: u64,
    pub o1_credit2_granted_not_consumed: u64,
    pub o1_credit2_drain_events: u64,
    pub o1_credit2_refill_suppressed_while_draining: u64,
    pub pre_render_abandoned: u64,
    pub predicted_render_ready_service_ns: u64,
    pub predicted_kms_lead_ns: u64,
    pub predicted_total_service_ns: u64,
    pub last_overlap_required_ns: u64,
    pub positive_overlap_observations: u64,
    pub nonpositive_overlap_observations: u64,
    pub render_ahead_attempts: u64,
    pub render_ahead_ready: u64,
    pub ready_submits: u64,
    pub triple_entries_predicted: u64,
    pub triple_entries_render_miss: u64,
    pub triple_entries_submit_miss: u64,
    pub triple_entries_presentation_miss: u64,
    pub triple_exits: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct WorkerTimingPerformanceSnapshot {
    pub submit_wake_lateness: SignedTimingSummarySnapshot,
    pub pre_submit_duration: TimingSummarySnapshot,
    pub dispatch_duration: TimingSummarySnapshot,
    pub ioctl_duration: TimingSummarySnapshot,
    pub queue_residency: TimingSummarySnapshot,
    pub submit_earliness: SignedTimingSummarySnapshot,
    pub submit_return_earliness: SignedTimingSummarySnapshot,
    pub submit_ack_delay: TimingSummarySnapshot,
    pub pageflip_ack_delay: TimingSummarySnapshot,
    pub test_only_duration: TimingSummarySnapshot,
    pub dispatch_budget_us: u64,
    pub late_before_ioctl: u64,
    pub late_after_ioctl: u64,
    pub test_only_count: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct KmsPerformanceSnapshot {
    pub mode_refresh_interval_ns: u64,
    pub mode_blanking_interval_ns: Option<u64>,
    pub base_apply_guard_ns: u64,
    pub adaptive_apply_guard_ns: u64,
    pub total_apply_guard_ns: u64,
    pub target_hits: u64,
    pub pre_render_unreachable: u64,
    pub render_readiness_misses: u64,
    pub dispatch_misses: u64,
    pub apply_guard_misses: u64,
    pub worker_jobs_enqueued: u64,
    pub worker_jobs_submitted: u64,
    pub worker_jobs_rejected: u64,
    pub worker_late_wakeups: u64,
    pub worker_submit_duration_max_us: u64,
    pub worker_queue_residency_max_us: u64,
    pub worker_queue_depth_max: u64,
    pub worker_timing: WorkerTimingPerformanceSnapshot,
    pub main_loop_wake_lateness_p50_us: u64,
    pub main_loop_wake_lateness_p95_us: u64,
    pub main_loop_wake_lateness_p99_us: u64,
    pub main_loop_target_slip_p50_us: u64,
    pub main_loop_target_slip_p95_us: u64,
    pub main_loop_target_slip_p99_us: u64,
    pub pageflip_interval_p50_us: u64,
    pub pageflip_interval_p95_us: u64,
    pub pageflip_interval_p99_us: u64,
    pub commit_to_present_p50_us: u64,
    pub commit_to_present_p95_us: u64,
    pub commit_to_present_p99_us: u64,
    pub missed_refresh_1x: u64,
    pub missed_refresh_2x: u64,
    pub missed_refresh_3x_or_more: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct ResourceEfficiencyPerformanceSnapshot {
    pub native_cycles: u64,
    pub input_ready: u64,
    pub raw_input_events: u64,
    pub coalesced_input_events: u64,
    pub pointer_samples: u64,
    pub primary_scene_attempts: u64,
    pub primary_scene_renders: u64,
    pub primary_scene_submits: u64,
    pub cursor_only_opportunities: u64,
    pub cursor_only_submits: u64,
    pub protocol_only_completions: u64,
    pub pure_input_completions: u64,
    pub input_only_cycles: u64,
    pub wayland_read_dispatch_cycles: u64,
    pub server_tick_calls: u64,
    pub client_flushes: u64,
    pub hit_test_locality: u64,
    pub hit_test_full_scans: u64,
    pub xwayland_sync_requests: u64,
    pub xwayland_reconciliations: u64,
    pub xwayland_unchanged_skips: u64,
    pub xwayland_environment_materializations: u64,
    pub pacing_progressions: u64,
    pub acquire_prepare_runs: u64,
    pub acquire_prepare_skips: u64,
    pub explicit_sync_service_runs: u64,
    pub frame_prepare_runs: u64,
    pub surface_pacing_service_runs: u64,
    pub commit_timing_planning_replans: u64,
    pub presentation_planning_runs: u64,
    pub presentation_planning_skips: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct PerformanceSnapshot {
    pub compositor_cpu_render: TimingSummarySnapshot,
    pub repaint: RepaintPerformanceSnapshot,
    pub buffering: BufferingPerformanceSnapshot,
    pub kms: KmsPerformanceSnapshot,
    pub resource_efficiency: ResourceEfficiencyPerformanceSnapshot,
    pub timing_scopes: std::collections::BTreeMap<String, TimingSummarySnapshot>,
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
    Performance(Box<PerformanceSnapshot>),
    DecorationTheme(DecorationThemeSnapshot),
    DecorationThemes(DecorationThemeListSnapshot),
    Wallpaper(WallpaperSnapshot),
    WallpaperList(WallpaperListSnapshot),
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
    fn performance_snapshot_round_trips_resource_efficiency_field() {
        let timing = serde_json::json!({
            "count": 0,
            "totalUs": 0,
            "lastUs": 0,
            "meanUs": 0,
            "p50Us": 0,
            "p95Us": 0,
            "p99Us": 0,
            "maxUs": 0,
        });
        let signed_timing = serde_json::json!({
            "count": 0,
            "totalUs": 0,
            "lastUs": 0,
            "meanUs": 0,
            "p50Us": 0,
            "p95Us": 0,
            "p99Us": 0,
            "minUs": 0,
            "maxUs": 0,
        });
        let value = serde_json::json!({
            "compositorCpuRender": timing,
            "repaint": {
                "skipFrames": 0,
                "partialFrames": 0,
                "fullFrames": 0,
                "bufferAgeBuckets": {},
                "partialRepairPixels": 0,
                "fullOutputPixels": 0,
                "fullRepaintReasons": {},
            },
            "buffering": {
                "reactiveDoubleFrames": 0,
                "predictiveTripleFrames": 0,
                "futurePrimaryCredit": 0,
                "extraCreditGrants": 0,
                "extraCreditRevokes": 0,
                "o1Credit2UsefulHits": 0,
                "o1Credit2UnnecessaryHits": 0,
                "o1Credit2IneffectiveMisses": 0,
                "o1Credit2GrantedNotConsumed": 0,
                "o1Credit2DrainEvents": 0,
                "o1Credit2RefillSuppressedWhileDraining": 0,
                "preRenderAbandoned": 0,
                "predictedRenderReadyServiceNs": 0,
                "predictedKmsLeadNs": 0,
                "predictedTotalServiceNs": 0,
                "lastOverlapRequiredNs": 0,
                "positiveOverlapObservations": 0,
                "nonpositiveOverlapObservations": 0,
                "renderAheadAttempts": 0,
                "renderAheadReady": 0,
                "readySubmits": 0,
                "tripleEntriesPredicted": 0,
                "tripleEntriesRenderMiss": 0,
                "tripleEntriesSubmitMiss": 0,
                "tripleEntriesPresentationMiss": 0,
                "tripleExits": 0,
            },
            "kms": {
                "modeRefreshIntervalNs": 0,
                "modeBlankingIntervalNs": null,
                "baseApplyGuardNs": 0,
                "adaptiveApplyGuardNs": 0,
                "totalApplyGuardNs": 0,
                "targetHits": 0,
                "preRenderUnreachable": 0,
                "renderReadinessMisses": 0,
                "dispatchMisses": 0,
                "applyGuardMisses": 0,
                "workerJobsEnqueued": 0,
                "workerJobsSubmitted": 0,
                "workerJobsRejected": 0,
                "workerLateWakeups": 0,
                "workerSubmitDurationMaxUs": 0,
                "workerQueueResidencyMaxUs": 0,
                "workerQueueDepthMax": 0,
                "workerTiming": {
                    "submitWakeLateness": signed_timing,
                    "preSubmitDuration": timing,
                    "dispatchDuration": timing,
                    "ioctlDuration": timing,
                    "queueResidency": timing,
                    "submitEarliness": signed_timing,
                    "submitReturnEarliness": signed_timing,
                    "submitAckDelay": timing,
                    "pageflipAckDelay": timing,
                    "testOnlyDuration": timing,
                    "dispatchBudgetUs": 0,
                    "lateBeforeIoctl": 0,
                    "lateAfterIoctl": 0,
                    "testOnlyCount": 0,
                },
                "mainLoopWakeLatenessP50Us": 0,
                "mainLoopWakeLatenessP95Us": 0,
                "mainLoopWakeLatenessP99Us": 0,
                "mainLoopTargetSlipP50Us": 0,
                "mainLoopTargetSlipP95Us": 0,
                "mainLoopTargetSlipP99Us": 0,
                "pageflipIntervalP50Us": 0,
                "pageflipIntervalP95Us": 0,
                "pageflipIntervalP99Us": 0,
                "commitToPresentP50Us": 0,
                "commitToPresentP95Us": 0,
                "commitToPresentP99Us": 0,
                "missedRefresh1x": 0,
                "missedRefresh2x": 0,
                "missedRefresh3xOrMore": 0,
            },
            "resourceEfficiency": {
                "nativeCycles": 7,
                "inputReady": 0,
                "rawInputEvents": 0,
                "coalescedInputEvents": 0,
                "pointerSamples": 0,
                "primarySceneAttempts": 0,
                "primarySceneRenders": 0,
                "primarySceneSubmits": 0,
                "cursorOnlyOpportunities": 0,
                "cursorOnlySubmits": 0,
                "protocolOnlyCompletions": 0,
                "pureInputCompletions": 0,
                "inputOnlyCycles": 0,
                "waylandReadDispatchCycles": 0,
                "serverTickCalls": 0,
                "clientFlushes": 0,
                "hitTestLocality": 0,
                "hitTestFullScans": 0,
                "xwaylandSyncRequests": 0,
                "xwaylandReconciliations": 0,
                "xwaylandUnchangedSkips": 0,
                "xwaylandEnvironmentMaterializations": 0,
                "pacingProgressions": 0,
                "acquirePrepareRuns": 0,
                "acquirePrepareSkips": 0,
                "explicitSyncServiceRuns": 0,
                "framePrepareRuns": 0,
                "surfacePacingServiceRuns": 0,
                "commitTimingPlanningReplans": 0,
                "presentationPlanningRuns": 0,
                "presentationPlanningSkips": 0,
            },
            "timingScopes": {},
        });

        let snapshot = serde_json::from_value::<PerformanceSnapshot>(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(snapshot).unwrap(), value);
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
