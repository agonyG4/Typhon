use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, VecDeque},
    error::Error,
    ffi::c_void,
    fs::{self, OpenOptions},
    io, mem,
    os::{
        fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd},
        unix::fs::OpenOptionsExt,
    },
    path::{Path, PathBuf},
    ptr,
    rc::Rc,
    slice,
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};

use crate::egl_renderer::dmabuf::{query_egl_dmabuf_feedback, query_egl_main_device};
use crate::egl_renderer::{EglInstance, EglSwapBuffersWithDamage};
use crate::egl_renderer::{
    EglSceneDrawRequest, GlEglImageTargetTexture2DOes, GlesSceneFrameStats, GlesSceneRenderer,
    choose_egl_config, choose_native_egl_config, create_gles_context, egl_swap_buffers_with_damage,
    load_egl_image_target_texture_2d, load_swap_buffers_with_damage, native_visual_label,
};
use gbm::AsRaw as GbmAsRaw;
use khronos_egl as egl;
#[cfg(test)]
use oblivion_one::compositor::OutputRect;
use oblivion_one::compositor::{
    AcquireWatchChange, DesktopComposeRequest, DesktopFrameCopyKind, DesktopSceneRebuildKind,
    DesktopSceneRenderer, DesktopVisualState, FramePresentation,
    OutputPosition as CompositorOutputPosition, OutputRegion, OwnCompositorServer,
    PointerConstraintBackendId, PointerConstraintBackendRequest, PointerConstraintMode,
    PointerMotionSample as CompositorPointerMotionSample, PresentationClock,
    RelativePointerMotion as CompositorRelativePointerMotion, RenderGenerationCause,
    RenderSceneElement, RenderSceneElementId, RenderableSurface, ShellDockItem,
    ShellOverlayRenderer, ShellOverlayState, ShellTopbarModel, SpotlightModel,
    cursor_texture_pixels, cursor_texture_size, dock_item_at, render_scene_elements_for_surfaces,
};
use oblivion_one::native::{
    drm::{
        DrmPresentationEvent, DrmTimestampClock, drain_drm_page_flip_events,
        query_drm_timestamp_clock, sample_clock_microseconds, submit_legacy_page_flip,
    },
    event_loop::{NativeEventLoop, NativeEventSource, monotonic_now_ns},
    explicit_sync::{
        AcquireReadyResult, AcquireRegistrationResult, DrmAcquirePointNotifier,
        ExplicitSyncWatchRegistry,
    },
    scheduler::{NativeFrameScheduler, PageFlipCompletionResult, SchedulerDecision},
};
use oblivion_one::render_backend::egl_gles::EglGlesDmabufFeedback;
use oblivion_one::session::NativeSessionProbe;
use oblivion_one::syncobj::DrmSyncobjDevice;
use oblivion_one::{
    CompositorAppGpuPreference, EffectiveCompositorAppGpuPolicy, shell_quote,
    spawn_compositor_app_with_policy,
};

type NativeResult<T> = Result<T, Box<dyn Error>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NativePerfLogger {
    enabled: bool,
}

impl NativePerfLogger {
    fn from_env() -> Self {
        let enabled = std::env::var("OBLIVION_ONE_PERF_LOG")
            .ok()
            .is_some_and(|value| native_perf_log_value_enabled(&value));
        Self { enabled }
    }

    const fn enabled(self) -> bool {
        self.enabled
    }

    fn log<F>(self, event: &str, fields: F)
    where
        F: FnOnce() -> Vec<NativePerfField>,
    {
        if self.enabled {
            println!("{}", native_perf_line(event, &fields()));
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NativePerfField {
    key: &'static str,
    value: String,
}

impl NativePerfField {
    fn str(key: &'static str, value: impl Into<String>) -> Self {
        Self {
            key,
            value: value.into(),
        }
    }

    fn u64(key: &'static str, value: u64) -> Self {
        Self::str(key, value.to_string())
    }

    fn usize(key: &'static str, value: usize) -> Self {
        Self::str(key, value.to_string())
    }

    fn f64(key: &'static str, value: f64) -> Self {
        Self::str(key, format!("{value:.2}"))
    }

    fn bool(key: &'static str, value: bool) -> Self {
        Self::str(key, if value { "true" } else { "false" })
    }
}

fn native_perf_log_value_enabled(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on" | "debug" | "trace"
    )
}

fn native_perf_line(event: &str, fields: &[NativePerfField]) -> String {
    let mut line = format!("perf {event}");
    for field in fields {
        line.push(' ');
        line.push_str(field.key);
        line.push('=');
        line.push_str(&native_perf_value(&field.value));
    }
    line
}

fn native_perf_value(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_-./:@+=".contains(character))
    {
        return value.to_string();
    }

    let mut quoted = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            character => quoted.push(character),
        }
    }
    quoted.push('"');
    quoted
}

fn elapsed_micros(start: Instant) -> u64 {
    start.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NativeProcessCpuSample {
    user_ticks: u64,
    system_ticks: u64,
}

#[derive(Debug, Clone)]
struct NativeAppLaunchPerf {
    program: String,
    command: String,
    pid: u32,
    spawn_us: u64,
    started_at: Instant,
    gpu_policy: EffectiveCompositorAppGpuPolicy,
    source: NativeLaunchSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeLaunchSource {
    Startup,
    Spotlight,
}

impl NativeLaunchSource {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::Spotlight => "spotlight",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NativeLaunchRequest {
    argv: Vec<String>,
    program: String,
    command: String,
    gpu_policy: EffectiveCompositorAppGpuPolicy,
    source: NativeLaunchSource,
}

fn resolve_native_app_gpu_policy(
    preference: CompositorAppGpuPreference,
    scanout: NativeScanoutKind,
) -> io::Result<EffectiveCompositorAppGpuPolicy> {
    match (preference, scanout) {
        (CompositorAppGpuPreference::CpuOnly, _) => Ok(EffectiveCompositorAppGpuPolicy::CpuOnly),
        (CompositorAppGpuPreference::Accelerated, NativeScanoutKind::NativeEglGbm) => {
            Ok(EffectiveCompositorAppGpuPolicy::Accelerated)
        }
        (CompositorAppGpuPreference::Accelerated, scanout) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "OBLIVION_ONE_NATIVE_APP_GPU=gpu requires native-egl-gbm, but active backend is {}",
                scanout.metric_name()
            ),
        )),
        (CompositorAppGpuPreference::Auto, NativeScanoutKind::NativeEglGbm) => {
            Ok(EffectiveCompositorAppGpuPolicy::Accelerated)
        }
        (CompositorAppGpuPreference::Auto, _) => Ok(EffectiveCompositorAppGpuPolicy::CpuOnly),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeRuntimeStage {
    DrainPageFlipEvents,
    Present,
}

impl NativeRuntimeStage {
    const fn as_str(self) -> &'static str {
        match self {
            Self::DrainPageFlipEvents => "drain_page_flip_events",
            Self::Present => "present",
        }
    }
}

fn native_runtime_error(
    stage: NativeRuntimeStage,
    backend: NativeScanoutKind,
    crtc_id: u32,
    frame_index: u64,
    source: io::Error,
) -> io::Error {
    io::Error::other(format!(
        "fatal native GPU runtime error stage={} backend={} crtc={} frame={} recovery=\"OBLIVION_ONE_SCANOUT_BACKEND=cpu OBLIVION_ONE_NATIVE_APP_GPU=cpu ./bin/start-oblivion-one-tty -- <app>\" cause={source}",
        stage.as_str(),
        backend.metric_name(),
        crtc_id,
        frame_index,
    ))
}

#[derive(Debug)]
struct NativeResizePerf {
    started_at: Instant,
    updates: u64,
}

#[derive(Debug, Default)]
struct NativeResizePerfState {
    active: Option<NativeResizePerf>,
}

impl NativeResizePerfState {
    fn observe_action(
        &mut self,
        action: NativeWindowAction,
        changed: bool,
        perf: NativePerfLogger,
    ) {
        match action {
            NativeWindowAction::BeginResize { x, y } if changed => {
                self.active = Some(NativeResizePerf {
                    started_at: Instant::now(),
                    updates: 0,
                });
                perf.log("resize.begin", || {
                    vec![NativePerfField::f64("x", x), NativePerfField::f64("y", y)]
                });
            }
            NativeWindowAction::UpdateInteraction { x, y } => {
                if let Some(active) = self.active.as_mut() {
                    active.updates = active.updates.saturating_add(1);
                    let elapsed_us = elapsed_micros(active.started_at);
                    perf.log("resize.update", || {
                        vec![
                            NativePerfField::u64("elapsed_us", elapsed_us),
                            NativePerfField::u64("updates", active.updates),
                            NativePerfField::f64("x", x),
                            NativePerfField::f64("y", y),
                            NativePerfField::bool("changed", changed),
                        ]
                    });
                }
            }
            NativeWindowAction::EndInteraction => {
                if let Some(active) = self.active.take() {
                    perf.log("resize.end", || {
                        vec![
                            NativePerfField::u64("elapsed_us", elapsed_micros(active.started_at)),
                            NativePerfField::u64("updates", active.updates),
                            NativePerfField::bool("changed", changed),
                        ]
                    });
                }
            }
            _ => {}
        }
    }
}

impl NativeProcessCpuSample {
    fn read_current() -> Option<Self> {
        let stat = fs::read_to_string("/proc/self/stat").ok()?;
        parse_proc_stat_cpu_ticks(&stat)
    }

    fn delta_us_since(self, previous: Self) -> (u64, u64) {
        let ticks_per_second = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        let Some(ticks_per_second) = u64::try_from(ticks_per_second)
            .ok()
            .filter(|ticks| *ticks > 0)
        else {
            return (0, 0);
        };
        let user_ticks = self.user_ticks.saturating_sub(previous.user_ticks);
        let system_ticks = self.system_ticks.saturating_sub(previous.system_ticks);
        (
            user_ticks.saturating_mul(1_000_000) / ticks_per_second,
            system_ticks.saturating_mul(1_000_000) / ticks_per_second,
        )
    }
}

fn parse_proc_stat_cpu_ticks(stat: &str) -> Option<NativeProcessCpuSample> {
    let after_comm = stat.rsplit_once(") ")?.1;
    let fields = after_comm.split_whitespace().collect::<Vec<_>>();
    Some(NativeProcessCpuSample {
        user_ticks: fields.get(11)?.parse().ok()?,
        system_ticks: fields.get(12)?.parse().ok()?,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NativeOutputBootstrap {
    runtime_dir: Option<PathBuf>,
    kms_device: Option<PathBuf>,
    render_device: Option<PathBuf>,
    connector: Option<NativeConnector>,
    kms_resources: Result<Option<KmsResources>, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NativeConnector {
    name: String,
    enabled: Option<String>,
    modes: Vec<String>,
    vrr_capable: Option<bool>,
}

impl NativeConnector {
    fn preferred_mode(&self) -> Option<&str> {
        self.modes.first().map(String::as_str)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeVrrPreference {
    Auto,
    On,
    Off,
}

impl NativeVrrPreference {
    fn from_env() -> Self {
        let Some(value) = std::env::var_os("OBLIVION_ONE_VRR") else {
            return Self::Auto;
        };
        let value = value.to_string_lossy();
        let preference = Self::parse(&value);
        if preference == Self::Auto && !value.eq_ignore_ascii_case("auto") {
            eprintln!("native KMS: unknown OBLIVION_ONE_VRR={value:?}; using auto");
        }
        preference
    }

    fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" | "enable" | "enabled" => Self::On,
            "0" | "false" | "no" | "off" | "disable" | "disabled" => Self::Off,
            _ => Self::Auto,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::On => "on",
            Self::Off => "off",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NativeVrrPlan {
    requested: NativeVrrPreference,
    supported: bool,
    planned_enabled: bool,
}

impl NativeVrrPlan {
    fn choose(requested: NativeVrrPreference, connector_vrr_capable: Option<bool>) -> Self {
        let supported = connector_vrr_capable.unwrap_or(false);
        let planned_enabled = match requested {
            NativeVrrPreference::Auto | NativeVrrPreference::On => supported,
            NativeVrrPreference::Off => false,
        };
        Self {
            requested,
            supported,
            planned_enabled,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct KmsResources {
    crtc_count: usize,
    connector_count: usize,
    encoder_count: usize,
    connected_connector_count: usize,
    first_connected_connector_id: Option<u32>,
    first_connected_mode: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct KmsTarget {
    connector_id: u32,
    crtc_id: u32,
    mode: drm_sys::drm_mode_modeinfo,
    width: u32,
    height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeModePreference {
    Auto,
    Preferred,
    HighResolution,
    HighRefresh,
    Exact {
        width: u32,
        height: u32,
        refresh_hz: Option<u32>,
    },
}

impl NativeModePreference {
    fn from_env() -> Self {
        let Some(value) = std::env::var_os("OBLIVION_ONE_MODE") else {
            return Self::Auto;
        };
        let value = value.to_string_lossy();
        let preference = Self::parse(&value);
        if preference == Self::Auto && !value.eq_ignore_ascii_case("auto") {
            eprintln!("native KMS: unknown OBLIVION_ONE_MODE={value:?}; using auto");
        }
        preference
    }

    fn parse(value: &str) -> Self {
        let value = value.trim();
        if value.eq_ignore_ascii_case("auto") {
            return Self::Auto;
        }
        if value.eq_ignore_ascii_case("highres") {
            return Self::HighResolution;
        }
        if value.eq_ignore_ascii_case("preferred") {
            return Self::Preferred;
        }
        if value.eq_ignore_ascii_case("highrr") || value.eq_ignore_ascii_case("highrefresh") {
            return Self::HighRefresh;
        }
        Self::parse_exact(value).unwrap_or(Self::Auto)
    }

    fn parse_exact(value: &str) -> Option<Self> {
        let (resolution, refresh) = value.split_once('@').unwrap_or((value, ""));
        let (width, height) = resolution
            .split_once(['x', 'X'])
            .and_then(|(width, height)| {
                Some((width.trim().parse().ok()?, height.trim().parse().ok()?))
            })?;
        let refresh_hz = if refresh.trim().is_empty() {
            None
        } else {
            Some(parse_refresh_hz(refresh.trim())?)
        };
        Some(Self::Exact {
            width,
            height,
            refresh_hz,
        })
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Preferred => "preferred",
            Self::HighResolution => "highres",
            Self::HighRefresh => "highrr",
            Self::Exact { .. } => "exact",
        }
    }
}

fn parse_refresh_hz(value: &str) -> Option<u32> {
    if let Ok(refresh) = value.parse::<u32>() {
        return Some(refresh);
    }
    let refresh = value.parse::<f64>().ok()?;
    if refresh.is_finite() && refresh > 0.0 {
        Some(refresh.round() as u32)
    } else {
        None
    }
}

impl NativeOutputBootstrap {
    fn discover() -> Self {
        let kms_device = first_dri_node("card");
        let connector =
            connected_connector_for_card(kms_device.as_deref(), Path::new("/sys/class/drm"));
        let kms_resources = query_kms_resources(kms_device.as_deref());
        let render_device = kms_device
            .as_deref()
            .and_then(|path| {
                matching_render_node_for_card(
                    path,
                    Path::new("/sys/class/drm"),
                    Path::new("/dev/dri"),
                )
            })
            .or_else(|| first_dri_node("renderD"));
        Self {
            runtime_dir: std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
            kms_device,
            render_device,
            connector,
            kms_resources,
        }
    }
}

fn matching_render_node_for_card(
    kms_device: &Path,
    drm_sysfs_root: &Path,
    dri_device_root: &Path,
) -> Option<PathBuf> {
    let card_name = kms_device.file_name()?.to_str()?;
    let drm_dir = drm_sysfs_root.join(card_name).join("device").join("drm");
    let mut render_nodes = fs::read_dir(drm_dir)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            name.starts_with("renderD")
                .then(|| dri_device_root.join(name))
        })
        .collect::<Vec<_>>();
    render_nodes.sort();
    render_nodes.into_iter().next()
}

pub fn run(
    mut server: OwnCompositorServer,
    app: Vec<String>,
    app_gpu_preference: CompositorAppGpuPreference,
) -> NativeResult<()> {
    let perf = NativePerfLogger::from_env();
    let startup_app = (!app.is_empty()).then_some(app);
    let bootstrap = NativeOutputBootstrap::discover();
    let session_probe = NativeSessionProbe::detect();
    let vrr_preference = NativeVrrPreference::from_env();
    let vrr_plan = NativeVrrPlan::choose(
        vrr_preference,
        bootstrap
            .connector
            .as_ref()
            .and_then(|connector| connector.vrr_capable),
    );

    println!("Opening native output backend.");
    println!("Wayland socket active: {}", server.socket_name());
    println!(
        "runtime dir: {}",
        display_optional_path(bootstrap.runtime_dir.as_deref())
    );
    println!(
        "kms device: {}",
        display_optional_path(bootstrap.kms_device.as_deref())
    );
    println!(
        "render device: {}",
        display_optional_path(bootstrap.render_device.as_deref())
    );
    if let Some(connector) = bootstrap.connector.as_ref() {
        println!("connected output: {}", connector.name);
        println!(
            "output enabled: {}",
            connector.enabled.as_deref().unwrap_or("unknown")
        );
        println!(
            "preferred mode: {}",
            connector.preferred_mode().unwrap_or("unknown")
        );
        println!(
            "vrr capable: {}",
            connector
                .vrr_capable
                .map(|capable| if capable { "yes" } else { "no" })
                .unwrap_or("unknown")
        );
    } else {
        println!("connected output: missing");
    }
    println!(
        "native VRR target: {} (supported {}, planned {})",
        vrr_plan.requested.as_str(),
        if vrr_plan.supported { "yes" } else { "no" },
        if vrr_plan.planned_enabled {
            "yes"
        } else {
            "no"
        }
    );
    match bootstrap.kms_resources.as_ref() {
        Ok(Some(resources)) => {
            println!(
                "kms resources: {} crtc(s), {} connector(s), {} encoder(s)",
                resources.crtc_count, resources.connector_count, resources.encoder_count
            );
            println!(
                "kms connected connectors: {}",
                resources.connected_connector_count
            );
            println!(
                "kms first connected connector id: {}",
                resources
                    .first_connected_connector_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "missing".to_string())
            );
            println!(
                "kms first connected mode: {}",
                resources
                    .first_connected_mode
                    .as_deref()
                    .unwrap_or("missing")
            );
        }
        Ok(None) => println!("kms resources: missing"),
        Err(error) => println!("kms resources: unavailable ({error})"),
    }
    println!(
        "native session input target: {}",
        session_probe.plan.input_strategy.as_str()
    );
    println!(
        "native session output target: {}",
        session_probe.plan.output_strategy.as_str()
    );
    println!("native session backend: libseat-managed input/DRM + GBM pageflip fallback");
    perf.log("native.start", || {
        vec![
            NativePerfField::str("socket", server.socket_name()),
            NativePerfField::str(
                "kms_device",
                display_optional_path(bootstrap.kms_device.as_deref()),
            ),
            NativePerfField::str(
                "render_device",
                display_optional_path(bootstrap.render_device.as_deref()),
            ),
            NativePerfField::str("vrr_policy", vrr_plan.requested.as_str()),
            NativePerfField::bool("vrr_supported", vrr_plan.supported),
            NativePerfField::bool("vrr_planned", vrr_plan.planned_enabled),
            NativePerfField::str("input_target", session_probe.plan.input_strategy.as_str()),
            NativePerfField::str("output_target", session_probe.plan.output_strategy.as_str()),
        ]
    });
    for warning in session_probe.plan.warnings() {
        eprintln!("native session: {warning}");
    }
    if let Some(command) = startup_app.as_ref() {
        println!("startup app command deferred until native scanout is ready: {command:?}");
        perf.log("app.deferred", || {
            vec![
                NativePerfField::usize("argc", command.len()),
                NativePerfField::str("command", command.join(" ")),
            ]
        });
    }

    if host_display_variables_available() && !native_scanout_forced() {
        return Err(io::Error::other(
            "native scanout guarded because a host display is active; set OBLIVION_ONE_NATIVE_SCANOUT=1 to take over the DRM output",
        )
        .into());
    }

    let Some(kms_device) = bootstrap.kms_device.as_deref() else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no /dev/dri/card* KMS device found",
        )
        .into());
    };

    probe_native_egl_gbm_device(&bootstrap, &perf);

    let seat_session = open_native_seat_session(&session_probe);
    let drm_plan = NativeDrmBackendPlan::choose(NativeDrmBackendChoice {
        preference: NativeDrmBackendPreference::from_env(),
        seat_available: seat_session.is_some(),
    });
    println!("native DRM backend target: {}", drm_plan.primary.as_str());
    let kms = NativeDrmDevice::open(drm_plan, kms_device, seat_session.clone())?;
    println!("native DRM backend active: {}", kms.kind().as_str());
    let native_syncobj_device = match DrmSyncobjDevice::from_active_drm_file(kms.file()) {
        Ok(device) => Some(device),
        Err(error) if error.kind() == io::ErrorKind::Unsupported => {
            eprintln!("native explicit sync unavailable on active DRM device: {error}");
            None
        }
        Err(error) => return Err(error.into()),
    };
    server.set_native_syncobj_device(native_syncobj_device);
    let drm_timestamp_clock = query_drm_timestamp_clock(kms.file().as_fd())?;
    let presentation_clock = match drm_timestamp_clock {
        DrmTimestampClock::Monotonic => PresentationClock::Monotonic,
        DrmTimestampClock::Realtime => PresentationClock::Realtime,
    };
    server.set_presentation_clock(presentation_clock);
    let mode_preference = NativeModePreference::from_env();
    let target = select_kms_target(kms.file(), mode_preference)?;
    let mode_label = format!(
        "{}x{}@{}",
        target.width, target.height, target.mode.vrefresh
    );
    println!(
        "native scanout target: connector {}, crtc {}, {}x{}@{}Hz ({})",
        target.connector_id,
        target.crtc_id,
        target.width,
        target.height,
        target.mode.vrefresh,
        mode_preference.as_str()
    );
    perf.log("native.kms", || {
        vec![
            NativePerfField::u64("connector", u64::from(target.connector_id)),
            NativePerfField::u64("crtc", u64::from(target.crtc_id)),
            NativePerfField::str("mode", mode_label.clone()),
            NativePerfField::str("policy", mode_preference.as_str()),
            NativePerfField::str("presentation_clock", drm_timestamp_clock.as_str()),
        ]
    });
    let refresh_hz = normalize_refresh_hz(target.mode.vrefresh);
    let refresh_interval_ns = 1_000_000_000 / u64::from(refresh_hz);
    println!(
        "native frame scheduler: {} Hz target, {} us absolute interval",
        refresh_hz,
        refresh_interval_ns / 1_000
    );

    server.set_output_size(target.width, target.height);
    server.set_output_refresh_hz(refresh_hz);
    let original_crtc = drm_ffi::mode::get_crtc(kms.file().as_fd(), target.crtc_id).ok();
    let scanout_preference = NativeScanoutPreference::from_env();
    let scanout_plan = NativeScanoutPlan::choose(NativeScanoutChoice {
        preference: scanout_preference,
        gbm_available: session_probe.plan.dependencies.gbm_available,
        egl_available: session_probe.plan.dependencies.egl_available,
        page_flip_available: true,
    });
    println!(
        "native scanout backend target: {}",
        scanout_plan.primary.as_str()
    );
    let scanout_target = scanout_plan.primary.as_str();
    let mut scanout = NativeScanoutBackend::open(
        scanout_plan.clone(),
        kms.file(),
        target.width,
        target.height,
    )?;
    perf.log("native.backend", || {
        vec![
            NativePerfField::str("drm", kms.kind().as_str()),
            NativePerfField::str("scanout", scanout.kind().as_str()),
            NativePerfField::str("scanout_target", scanout_target),
            NativePerfField::str("mode", mode_label.clone()),
            NativePerfField::u64("refresh_hz", u64::from(refresh_hz)),
            NativePerfField::u64("refresh_interval_ns", refresh_interval_ns),
        ]
    });
    let mut frame_renderer = NativeFrameRenderer::default();
    let mut input_state = NativeInputState::new(target.width, target.height);
    let cursor_preference = NativeCursorPreference::from_env();
    println!(
        "native cursor backend target: {}",
        cursor_preference.as_str()
    );
    let mut hardware_cursor = match cursor_preference {
        NativeCursorPreference::Software => None,
        NativeCursorPreference::Auto | NativeCursorPreference::Hardware => {
            match NativeHardwareCursor::create(kms.file(), target.crtc_id) {
                Ok(cursor) => Some(cursor),
                Err(error) if cursor_preference == NativeCursorPreference::Hardware => {
                    return Err(error.into());
                }
                Err(error) => {
                    eprintln!(
                        "native cursor: hardware cursor unavailable: {error}; using software"
                    );
                    perf.log("native.cursor", || {
                        vec![
                            NativePerfField::str("backend", "software"),
                            NativePerfField::str("policy", cursor_preference.as_str()),
                            NativePerfField::str("fallback", "create_failed"),
                            NativePerfField::str("error", error.to_string()),
                        ]
                    });
                    None
                }
            }
        }
    };
    let mut cursor_render_mode = if hardware_cursor.is_some() {
        NativeCursorRenderMode::Hardware
    } else {
        NativeCursorRenderMode::Software
    };
    let input_plan = NativeInputBackendPlan::choose(NativeInputBackendChoice {
        preference: NativeInputBackendPreference::from_env(),
        libseat_available: seat_session.is_some(),
        libinput_available: session_probe.plan.dependencies.libinput_available,
        raw_evdev_available: session_probe.raw_input_device.is_some(),
    });
    println!(
        "native input backend target: {}",
        input_plan.primary.as_str()
    );
    let _restore = CrtcRestore::new(
        kms.file().as_raw_fd(),
        target.crtc_id,
        target.connector_id,
        original_crtc,
    );
    let initial_damage = NativeOutputDamage::full_output(target.width, target.height);
    let initial_paint = match scanout.paint_server_frame(
        &mut frame_renderer,
        &server,
        &input_state,
        cursor_render_mode,
        &initial_damage,
    ) {
        Ok(paint) => paint,
        Err(error)
            if scanout.kind() == NativeScanoutKind::NativeEglGbm
                && scanout_preference != NativeScanoutPreference::NativeEglGbm
                && app_gpu_preference != CompositorAppGpuPreference::Accelerated =>
        {
            let fallback_plan = scanout_plan.after_failed(scanout.kind());
            if fallback_plan.primary == NativeScanoutKind::Unavailable {
                return Err(error.into());
            }
            eprintln!(
                "native scanout: initial native EGL/GBM paint failed: {error}; trying {} fallback",
                fallback_plan.primary.as_str()
            );
            perf.log("native.backend_fallback", || {
                vec![
                    NativePerfField::str("failed", NativeScanoutKind::NativeEglGbm.as_str()),
                    NativePerfField::str("fallback", fallback_plan.primary.as_str()),
                    NativePerfField::str("error", error.to_string()),
                ]
            });
            drop(scanout);
            scanout =
                NativeScanoutBackend::open(fallback_plan, kms.file(), target.width, target.height)?;
            scanout.paint_server_frame(
                &mut frame_renderer,
                &server,
                &input_state,
                cursor_render_mode,
                &initial_damage,
            )?
        }
        Err(error) => return Err(error.into()),
    };
    println!("native scanout backend active: {}", scanout.kind().as_str());
    let effective_app_gpu_policy =
        resolve_native_app_gpu_policy(app_gpu_preference, scanout.kind())?;
    if scanout.supports_gpu_buffer_protocols() {
        server.enable_gpu_buffer_protocols();
    }
    apply_native_scanout_feedback(&mut server, &scanout);
    println!("native app GPU preference: {}", app_gpu_preference.as_str());
    println!(
        "native app GPU policy effective: {}",
        effective_app_gpu_policy.as_str()
    );
    println!(
        "native app GPU policy reason: active_scanout={}",
        scanout.kind().metric_name()
    );
    perf.log("native.app_gpu_policy", || {
        vec![
            NativePerfField::str("preference", app_gpu_preference.as_str()),
            NativePerfField::str("effective", effective_app_gpu_policy.as_str()),
            NativePerfField::str("active_scanout", scanout.kind().metric_name()),
            NativePerfField::bool(
                "gpu_buffer_protocols",
                server.gpu_buffer_protocols_enabled(),
            ),
        ]
    });
    perf.log("native.frame", || {
        let mut fields = initial_paint.fields();
        fields.extend(initial_damage.fields());
        fields.extend([
            NativePerfField::str("phase", "initial"),
            NativePerfField::str("mode", mode_label.clone()),
            NativePerfField::str("cursor", cursor_render_mode.as_str()),
            NativePerfField::usize("surfaces", server.renderable_surfaces().len()),
            NativePerfField::u64("render_generation", server.render_generation()),
            NativePerfField::str("render_cause", server.render_generation_cause().as_str()),
        ]);
        fields
    });
    drm_ffi::mode::set_crtc(
        kms.file().as_fd(),
        target.crtc_id,
        scanout.fb_id(),
        0,
        0,
        &[target.connector_id],
        Some(target.mode),
    )?;
    scanout.finish_initial_scanout();
    if let Some(cursor) = hardware_cursor.as_mut() {
        let (cursor_x, cursor_y) = input_state.cursor_position();
        if let Err(error) = cursor
            .enable()
            .and_then(|()| cursor.move_to(cursor_x, cursor_y))
        {
            if cursor_preference == NativeCursorPreference::Hardware {
                return Err(error.into());
            }
            eprintln!("native cursor: hardware cursor activation failed: {error}; using software");
            hardware_cursor = None;
            cursor_render_mode = NativeCursorRenderMode::Software;
            let fallback_damage = NativeOutputDamage::full_output(target.width, target.height);
            let fallback_paint = scanout.paint_server_frame(
                &mut frame_renderer,
                &server,
                &input_state,
                cursor_render_mode,
                &fallback_damage,
            )?;
            perf.log("native.frame", || {
                let mut fields = fallback_paint.fields();
                fields.extend(fallback_damage.fields());
                fields.extend([
                    NativePerfField::str("phase", "cursor-fallback"),
                    NativePerfField::str("mode", mode_label.clone()),
                    NativePerfField::str("cursor", cursor_render_mode.as_str()),
                    NativePerfField::usize("surfaces", server.renderable_surfaces().len()),
                    NativePerfField::u64("render_generation", server.render_generation()),
                    NativePerfField::str("render_cause", server.render_generation_cause().as_str()),
                ]);
                fields
            });
            scanout.present(kms.file().as_fd(), target.crtc_id)?;
            perf.log("native.cursor", || {
                vec![
                    NativePerfField::str("backend", cursor_render_mode.as_str()),
                    NativePerfField::str("policy", cursor_preference.as_str()),
                    NativePerfField::str("fallback", "activation_failed"),
                    NativePerfField::str("error", error.to_string()),
                ]
            });
        } else {
            println!(
                "native cursor backend active: hardware ({}x{})",
                cursor.width, cursor.height
            );
            perf.log("native.cursor", || {
                vec![
                    NativePerfField::str("backend", cursor_render_mode.as_str()),
                    NativePerfField::str("policy", cursor_preference.as_str()),
                    NativePerfField::u64("width", u64::from(cursor.width)),
                    NativePerfField::u64("height", u64::from(cursor.height)),
                ]
            });
        }
    } else {
        println!("native cursor backend active: software");
        perf.log("native.cursor", || {
            vec![
                NativePerfField::str("backend", cursor_render_mode.as_str()),
                NativePerfField::str("policy", cursor_preference.as_str()),
            ]
        });
    }

    println!("native scanout active; press Alt+P to stop");
    let mut input_devices = NativeInputBackend::open(
        input_plan,
        target.width,
        target.height,
        seat_session.clone(),
    )?;
    println!(
        "native input backend active: {}",
        input_devices.kind().as_str()
    );
    perf.log("native.input", || {
        vec![
            NativePerfField::str("backend", input_devices.kind().as_str()),
            NativePerfField::u64("width", u64::from(target.width)),
            NativePerfField::u64("height", u64::from(target.height)),
        ]
    });
    set_fd_nonblocking(kms.file().as_raw_fd())?;
    let drm_file_generation = allocate_native_drm_file_generation();
    let acquire_notifier = DrmAcquirePointNotifier;
    let mut acquire_watches =
        ExplicitSyncWatchRegistry::new(refresh_interval_ns, drm_file_generation);
    server.enable_external_acquire_readiness();
    let mut event_loop = NativeEventLoop::new()?;
    event_loop.register(kms.file().as_raw_fd(), NativeEventSource::Drm)?;
    event_loop.register(
        server.listener_fd().as_raw_fd(),
        NativeEventSource::WaylandListener,
    )?;
    event_loop.register(
        server.client_dispatch_fd().as_raw_fd(),
        NativeEventSource::WaylandClients,
    )?;
    for (index, fd) in input_devices.event_fds().enumerate() {
        let index = u16::try_from(index)
            .map_err(|_| io::Error::other("too many native input event sources"))?;
        event_loop.register(fd, NativeEventSource::Input(index))?;
    }
    let scheduler_anchor_ns = monotonic_now_ns()?;
    let mut frame_scheduler = NativeFrameScheduler::new(refresh_hz, scheduler_anchor_ns);
    if let Some(token) = scanout.pending_page_flip_token() {
        frame_scheduler
            .note_async_submission(token, scheduler_anchor_ns)
            .map_err(io::Error::other)?;
    }
    event_loop.arm_deadline(earliest_native_deadline(
        frame_scheduler.next_deadline_ns(),
        acquire_watches.next_fallback_deadline_ns(),
    ))?;
    let mut last_render_generation = server.render_generation();
    let mut last_renderable_surfaces = server.renderable_surfaces().to_vec();
    let mut queued_redraw_requested = false;
    let mut frame_index = 0u64;
    let mut known_toplevels = server.xdg_toplevels();
    let mut pending_launches = VecDeque::<NativeAppLaunchPerf>::new();
    let mut mismatched_pageflip_events = 0u64;
    let mut stale_pageflip_events = 0u64;
    let mut last_acquire_ready_at_ns = None;
    let mut resize_perf = NativeResizePerfState::default();
    let mut pointer_constraint_backend = NativePointerConstraintBackend::new();
    if let Some(command) = startup_app
        && let Some(launch) = launch_native_shell_command(
            &server,
            command,
            effective_app_gpu_policy,
            NativeLaunchSource::Startup,
        )?
    {
        log_native_app_spawn(perf, &launch);
        pending_launches.push_back(launch);
    }
    loop {
        let wakeup = event_loop.wait()?;
        let scheduler_state_before = frame_scheduler.state();
        perf.log("native.wakeup", || {
            vec![
                NativePerfField::u64("ready_mask", u64::from(wakeup.reasons.bits())),
                NativePerfField::usize("ready_sources", wakeup.ready_sources),
                NativePerfField::u64("blocked_us", wakeup.blocked_ns / 1_000),
                NativePerfField::u64(
                    "deadline_late_us",
                    wakeup.timer_lateness_ns.unwrap_or(0) / 1_000,
                ),
                NativePerfField::str("scheduler_before", format!("{scheduler_state_before:?}")),
                NativePerfField::bool("pageflip_pending", scanout.page_flip_pending()),
            ]
        });
        if wakeup.reasons.timer() {
            perf.log("native.deadline", || {
                vec![
                    NativePerfField::u64(
                        "lateness_us",
                        wakeup.timer_lateness_ns.unwrap_or(0) / 1_000,
                    ),
                    NativePerfField::str("scheduler_state", format!("{scheduler_state_before:?}")),
                    NativePerfField::bool("pageflip_watchdog", frame_scheduler.page_flip_pending()),
                ]
            });
        }

        input_devices.dispatch_session_events();
        let pageflip_drain_start = Instant::now();
        let should_drain_pageflips =
            wakeup.reasons.drm() || (wakeup.reasons.timer() && frame_scheduler.page_flip_pending());
        let pageflip_drain = if should_drain_pageflips {
            scanout
                .drain_page_flip_events(kms.file().as_raw_fd())
                .map_err(|error| {
                    native_runtime_error(
                        NativeRuntimeStage::DrainPageFlipEvents,
                        scanout.kind(),
                        target.crtc_id,
                        frame_index,
                        error,
                    )
                })?
        } else {
            NativePageFlipDrain::default()
        };
        let pageflip_drain_us = elapsed_micros(pageflip_drain_start);
        mismatched_pageflip_events =
            mismatched_pageflip_events.saturating_add(pageflip_drain.mismatched_events);
        stale_pageflip_events = stale_pageflip_events.saturating_add(pageflip_drain.stale_events);
        if pageflip_drain.mismatched_events > 0 || pageflip_drain.stale_events > 0 {
            perf.log("native.pageflip_event_error", || {
                vec![
                    NativePerfField::u64("mismatched", pageflip_drain.mismatched_events),
                    NativePerfField::u64("stale", pageflip_drain.stale_events),
                    NativePerfField::u64(
                        "expected_token",
                        pageflip_drain.last_mismatch.map_or(0, |value| value.0),
                    ),
                    NativePerfField::u64(
                        "received_token",
                        pageflip_drain.last_mismatch.map_or(0, |value| value.1),
                    ),
                    NativePerfField::u64(
                        "stale_token",
                        pageflip_drain.last_stale_token.unwrap_or(0),
                    ),
                ]
            });
        }
        let pageflip_completed = pageflip_drain.completion.is_some();
        let mut frame_completed = false;
        let mut frame_rendered = false;
        let mut frame_submitted = false;
        if let Some(pageflip) = pageflip_drain.completion {
            let compositor_receive_ns = monotonic_now_ns()?;
            let scheduler_state_at_completion = frame_scheduler.state();
            let completion = frame_scheduler
                .note_page_flip_completion(pageflip.user_data, compositor_receive_ns);
            if let PageFlipCompletionResult::Completed { submitted_at_ns } = completion {
                let presentation = FramePresentation::synchronized(
                    presentation_clock,
                    pageflip.timestamp.seconds,
                    pageflip.timestamp.microseconds,
                    pageflip.sequence,
                )?;
                let compositor_receive_us = sample_clock_microseconds(drm_timestamp_clock)?;
                let kernel_timestamp_us = u64::from(pageflip.timestamp.seconds)
                    .saturating_mul(1_000_000)
                    .saturating_add(u64::from(pageflip.timestamp.microseconds));
                let finish_frame_start = Instant::now();
                server.finish_frame_with_presentation(presentation);
                if !server.has_pending_frame_work() {
                    frame_scheduler.complete_protocol_only();
                }
                frame_completed = true;
                perf.log("native.finish_frame", || {
                    vec![
                        NativePerfField::str("reason", "pageflip_complete"),
                        NativePerfField::u64("elapsed_us", elapsed_micros(finish_frame_start)),
                        NativePerfField::usize("surfaces", server.renderable_surfaces().len()),
                        NativePerfField::u64("render_generation", server.render_generation()),
                        NativePerfField::u64("pageflip_token", pageflip.user_data),
                        NativePerfField::u64("kernel_sequence", u64::from(pageflip.sequence)),
                        NativePerfField::u64("kernel_timestamp_us", kernel_timestamp_us),
                        NativePerfField::u64("compositor_receive_us", compositor_receive_us),
                        NativePerfField::u64(
                            "receive_delay_us",
                            compositor_receive_us.saturating_sub(kernel_timestamp_us),
                        ),
                        NativePerfField::u64(
                            "submit_to_completion_us",
                            compositor_receive_ns.saturating_sub(submitted_at_ns) / 1_000,
                        ),
                        NativePerfField::str(
                            "scheduler_state",
                            format!("{scheduler_state_at_completion:?}"),
                        ),
                    ]
                });
            }
        }
        let present_us = 0;
        let pageflip_pending_at_tick = scanout.page_flip_pending();
        let tick_start = Instant::now();
        let accepted = server.tick()?;
        let tick_us = elapsed_micros(tick_start);
        let mut redraw_requested = process_native_pointer_constraint_backend_requests(
            &mut server,
            &mut pointer_constraint_backend,
            &mut input_state,
            &mut hardware_cursor,
            cursor_render_mode,
        )?;
        let current_toplevels = server.xdg_toplevels();
        if current_toplevels > known_toplevels {
            for _ in known_toplevels..current_toplevels {
                let app_id = server.last_app_id().unwrap_or("unknown").to_string();
                if let Some(launch) = pending_launches.pop_front() {
                    perf.log("app.first_toplevel", || {
                        vec![
                            NativePerfField::str("program", launch.program.clone()),
                            NativePerfField::str("command", launch.command.clone()),
                            NativePerfField::str("source", launch.source.as_str()),
                            NativePerfField::u64("pid", u64::from(launch.pid)),
                            NativePerfField::str("app_id", app_id.clone()),
                            NativePerfField::u64("spawn_us", launch.spawn_us),
                            NativePerfField::u64("elapsed_us", elapsed_micros(launch.started_at)),
                            NativePerfField::usize("surfaces", server.renderable_surfaces().len()),
                        ]
                    });
                } else {
                    perf.log("app.toplevel", || {
                        vec![
                            NativePerfField::str("app_id", app_id.clone()),
                            NativePerfField::usize("surfaces", server.renderable_surfaces().len()),
                            NativePerfField::usize("total_toplevels", current_toplevels),
                        ]
                    });
                }
            }
            known_toplevels = current_toplevels;
        }
        if accepted > 0 {
            println!(
                "accepted {accepted} client(s); total {}",
                server.accepted_clients()
            );
        }
        let mut skipped_input_repaints = 0usize;
        let input_drain_start = Instant::now();
        let raw_events = input_devices.drain_events();
        let input_drain_us = elapsed_micros(input_drain_start);
        let raw_input_events = raw_events.len();
        let input_event_timestamp_usec = matches!(
            input_devices.kind(),
            NativeInputBackendKind::LibseatLibinputUdev
                | NativeInputBackendKind::DirectLibinputUdev
        )
        .then(|| {
            raw_events
                .iter()
                .filter_map(|event| event.timestamp_usec())
                .max()
        })
        .flatten();
        let coalesced_events = coalesce_pointer_motion_events(raw_events);
        let coalesced_input_events = coalesced_events.len();
        for event in coalesced_events {
            let may_change_pointer_constraints = event.may_change_pointer_constraints();
            let effect = input_state.handle_hardware_input_event(event);
            let effect_requested_redraw = effect.redraw_requested;
            if let Some((cursor_x, cursor_y)) = effect.cursor_position
                && let Some(cursor) = hardware_cursor.as_mut()
                && let Err(error) = cursor.move_to(cursor_x, cursor_y)
            {
                if cursor_preference == NativeCursorPreference::Hardware {
                    acquire_watches.shutdown(&mut event_loop)?;
                    return Err(error.into());
                }
                eprintln!("native cursor: hardware cursor move failed: {error}; using software");
                hardware_cursor = None;
                cursor_render_mode = NativeCursorRenderMode::Software;
                perf.log("native.cursor", || {
                    vec![
                        NativePerfField::str("backend", cursor_render_mode.as_str()),
                        NativePerfField::str("policy", cursor_preference.as_str()),
                        NativePerfField::str("fallback", "move_failed"),
                        NativePerfField::str("error", error.to_string()),
                    ]
                });
            }
            let application = apply_native_input_effect(
                effect,
                &mut server,
                perf,
                &mut resize_perf,
                cursor_render_mode,
                effective_app_gpu_policy,
            )?;
            if application.exit_requested {
                println!("native input exit requested; shutting down cleanly");
                acquire_watches.shutdown(&mut event_loop)?;
                return Ok(());
            }
            if let Some(launch) = application.launch {
                log_native_app_spawn(perf, &launch);
                pending_launches.push_back(launch);
            }
            if effect_requested_redraw && !application.redraw_requested {
                skipped_input_repaints = skipped_input_repaints.saturating_add(1);
            }
            redraw_requested |= application.redraw_requested;
            if may_change_pointer_constraints {
                let _ = server.tick()?;
                redraw_requested |= process_native_pointer_constraint_backend_requests(
                    &mut server,
                    &mut pointer_constraint_backend,
                    &mut input_state,
                    &mut hardware_cursor,
                    cursor_render_mode,
                )?;
            }
        }
        redraw_requested |= process_native_pointer_constraint_backend_requests(
            &mut server,
            &mut pointer_constraint_backend,
            &mut input_state,
            &mut hardware_cursor,
            cursor_render_mode,
        )?;
        if let Some(event_timestamp_us) = input_event_timestamp_usec {
            let dispatch_latency_us = monotonic_now_ns()?
                .saturating_div(1_000)
                .saturating_sub(event_timestamp_us);
            perf.log("native.input_dispatch", || {
                vec![
                    NativePerfField::usize("events", coalesced_input_events),
                    NativePerfField::u64("event_timestamp_us", event_timestamp_us),
                    NativePerfField::u64("dispatch_latency_us", dispatch_latency_us),
                ]
            });
        }
        let acquire_changes = server.take_acquire_watch_changes();
        let acquire_change_count = acquire_changes.len();
        let acquire_ready_token_count = wakeup.explicit_sync_acquire_tokens.len();
        let mut acquire_ready_count = 0usize;
        for change in acquire_changes {
            match change {
                AcquireWatchChange::Register(request) => {
                    match acquire_watches.register(
                        request,
                        &mut event_loop,
                        monotonic_now_ns()?,
                        &acquire_notifier,
                    )? {
                        AcquireRegistrationResult::AlreadyReady(request) => {
                            if server.mark_acquire_commit_ready(
                                request.commit_id,
                                request.surface_id,
                                &request.acquire,
                            ) {
                                acquire_ready_count = acquire_ready_count.saturating_add(1);
                            }
                        }
                        AcquireRegistrationResult::EventfdBacked(commit_id) => {
                            let _ = server.mark_acquire_commit_eventfd_backed(commit_id);
                        }
                        AcquireRegistrationResult::FallbackBacked(commit_id) => {
                            let _ = server.mark_acquire_commit_fallback_backed(commit_id);
                        }
                    }
                }
                AcquireWatchChange::Cancel { commit_id, reason } => {
                    let _ = acquire_watches.cancel_commit(commit_id, reason, &mut event_loop)?;
                }
            }
        }
        for token in wakeup.explicit_sync_acquire_tokens.iter().copied() {
            match acquire_watches.handle_ready(
                token,
                &mut event_loop,
                drm_file_generation,
                &acquire_notifier,
            )? {
                AcquireReadyResult::Ready(request) => {
                    if server.mark_acquire_commit_ready(
                        request.commit_id,
                        request.surface_id,
                        &request.acquire,
                    ) {
                        acquire_ready_count = acquire_ready_count.saturating_add(1);
                    }
                }
                AcquireReadyResult::BackendMismatch(_) => {}
                AcquireReadyResult::Pending | AcquireReadyResult::Stale => {}
            }
        }
        for request in acquire_watches.retry_fallback(monotonic_now_ns()?, &acquire_notifier) {
            if server.mark_acquire_commit_ready(
                request.commit_id,
                request.surface_id,
                &request.acquire,
            ) {
                acquire_ready_count = acquire_ready_count.saturating_add(1);
            }
        }
        if acquire_change_count > 0 || acquire_ready_token_count > 0 || acquire_ready_count > 0 {
            if acquire_ready_count > 0 {
                last_acquire_ready_at_ns = Some(monotonic_now_ns()?);
            }
            let metrics = acquire_watches.metrics();
            perf.log("native.explicit_sync", || {
                vec![
                    NativePerfField::usize("changes", acquire_change_count),
                    NativePerfField::usize("ready_tokens", acquire_ready_token_count),
                    NativePerfField::usize("ready_commits", acquire_ready_count),
                    NativePerfField::usize(
                        "active_eventfd_watches",
                        metrics.active_eventfd_watches,
                    ),
                    NativePerfField::usize(
                        "active_fallback_watches",
                        metrics.active_fallback_watches,
                    ),
                    NativePerfField::u64("registrations", metrics.registrations),
                    NativePerfField::u64("eventfd_wakeups", metrics.eventfd_wakeups),
                    NativePerfField::u64("stale_wakeups", metrics.stale_wakeups),
                    NativePerfField::u64("duplicate_wakeups", metrics.duplicate_wakeups),
                    NativePerfField::u64("cancellations", metrics.cancellations),
                    NativePerfField::u64("registration_failures", metrics.registration_failures),
                    NativePerfField::u64(
                        "last_registration_errno",
                        metrics.last_registration_errno.max(0) as u64,
                    ),
                    NativePerfField::u64(
                        "commit_to_acquire_ready_us",
                        metrics.last_commit_to_ready_ns / 1_000,
                    ),
                    NativePerfField::u64("fallback_activations", metrics.fallback_activations),
                    NativePerfField::usize(
                        "maximum_simultaneous_watches",
                        metrics.maximum_simultaneous_watches,
                    ),
                ]
            });
        }
        if !scanout.page_flip_pending() && server.has_pending_frame_prepare_work() {
            let prepare_frame_start = Instant::now();
            let before_generation = server.render_generation();
            server.prepare_frame();
            let after_generation = server.render_generation();
            perf.log("native.prepare_frame", || {
                vec![
                    NativePerfField::u64("elapsed_us", elapsed_micros(prepare_frame_start)),
                    NativePerfField::u64("render_generation", after_generation),
                    NativePerfField::bool("render_changed", after_generation != before_generation),
                    NativePerfField::bool("pending_frame_work", server.has_pending_frame_work()),
                ]
            });
        }
        let render_generation = server.render_generation();
        let render_generation_changed = render_generation != last_render_generation;
        let render_generation_cause = server.render_generation_cause();
        let pending_frame_work = server.has_pending_frame_work();
        let repaint_decision = native_repaint_decision(NativeRepaintInputs {
            accepted_clients: accepted > 0,
            render_generation_changed,
            pending_frame_work,
            only_pending_surface_frame_callbacks: server.has_only_pending_surface_frame_callbacks(),
            redraw_requested,
            page_flip_pending: false,
        });
        if repaint_decision.repaint {
            frame_scheduler.queue_visual_work();
            queued_redraw_requested |= redraw_requested;
        } else if repaint_decision.protocol_only_present {
            frame_scheduler.queue_protocol_work(monotonic_now_ns()?);
        }
        let scheduler_decision = frame_scheduler.decision(monotonic_now_ns()?);
        if scheduler_decision == SchedulerDecision::PageFlipWatchdogExpired {
            perf.log("native.pageflip_watchdog", || {
                vec![
                    NativePerfField::u64("frame", frame_index),
                    NativePerfField::u64("crtc", u64::from(target.crtc_id)),
                    NativePerfField::str("scanout", scanout.kind().metric_name()),
                    NativePerfField::u64("timeout_count", frame_scheduler.watchdog_timeout_count()),
                    NativePerfField::bool("drm_ready", wakeup.reasons.drm()),
                    NativePerfField::bool("final_drain_completed", pageflip_completed),
                ]
            });
            acquire_watches.shutdown(&mut event_loop)?;
            return Err(io::Error::other(format!(
                "native page flip watchdog expired: backend={} crtc={} frame={} pending=true; final DRM drain found no completion",
                scanout.kind().metric_name(),
                target.crtc_id,
                frame_index
            ))
            .into());
        }
        if scheduler_decision == SchedulerDecision::Render {
            let effective_redraw_requested = redraw_requested || queued_redraw_requested;
            let render_cause = native_repaint_cause_label(
                render_generation_cause,
                render_generation_changed,
                accepted,
                pending_frame_work,
                effective_redraw_requested,
            );
            let output_damage = native_output_damage_for_repaint(
                target.width,
                target.height,
                &last_renderable_surfaces,
                server.renderable_surfaces(),
                render_generation_cause,
                render_generation_changed,
            );
            let skip_empty_visible_damage = output_damage.is_empty()
                && render_generation_changed
                && accepted == 0
                && !effective_redraw_requested;
            if skip_empty_visible_damage {
                perf.log("native.frame_skip", || {
                    let mut fields = output_damage.fields().to_vec();
                    fields.extend([
                        NativePerfField::str("reason", "empty_visible_damage"),
                        NativePerfField::usize("skipped_input_repaints", skipped_input_repaints),
                        NativePerfField::u64("tick_us", tick_us),
                        NativePerfField::bool("pageflip_pending_at_tick", pageflip_pending_at_tick),
                        NativePerfField::u64("input_drain_us", input_drain_us),
                        NativePerfField::usize("raw_input_events", raw_input_events),
                        NativePerfField::usize("coalesced_input_events", coalesced_input_events),
                        NativePerfField::u64("pageflip_drain_us", pageflip_drain_us),
                        NativePerfField::bool("pageflip_completed", pageflip_completed),
                        NativePerfField::u64("present_us", present_us),
                        NativePerfField::u64("render_generation", render_generation),
                        NativePerfField::str("render_cause", render_cause),
                        NativePerfField::bool("pending_frame_work", pending_frame_work),
                    ]);
                    fields
                });
                if pending_frame_work {
                    let finish_frame_start = Instant::now();
                    server.finish_frame();
                    perf.log("native.finish_frame", || {
                        vec![
                            NativePerfField::str("reason", "empty_visible_damage"),
                            NativePerfField::u64("elapsed_us", elapsed_micros(finish_frame_start)),
                            NativePerfField::usize("surfaces", server.renderable_surfaces().len()),
                            NativePerfField::u64("render_generation", server.render_generation()),
                        ]
                    });
                }
                frame_scheduler.note_immediate_completion();
                queued_redraw_requested = false;
                last_render_generation = render_generation;
                last_renderable_surfaces = server.renderable_surfaces().to_vec();
            } else {
                let cpu_before = perf
                    .enabled()
                    .then(NativeProcessCpuSample::read_current)
                    .flatten();
                let paint_stats = scanout.paint_server_frame(
                    &mut frame_renderer,
                    &server,
                    &input_state,
                    cursor_render_mode,
                    &output_damage,
                )?;
                frame_rendered = true;
                let cpu_after = perf
                    .enabled()
                    .then(NativeProcessCpuSample::read_current)
                    .flatten();
                let (cpu_user_us, cpu_system_us) = cpu_before
                    .zip(cpu_after)
                    .map(|(before, after)| after.delta_us_since(before))
                    .unwrap_or((0, 0));
                let repaint_present_start = Instant::now();
                let present_result = scanout
                    .present(kms.file().as_fd(), target.crtc_id)
                    .map_err(|error| {
                        native_runtime_error(
                            NativeRuntimeStage::Present,
                            scanout.kind(),
                            target.crtc_id,
                            frame_index,
                            error,
                        )
                    })?;
                let repaint_present_us = elapsed_micros(repaint_present_start);
                let acquire_ready_to_render_submit_us = last_acquire_ready_at_ns
                    .map(|ready_at| {
                        monotonic_now_ns().map(|now| now.saturating_sub(ready_at) / 1_000)
                    })
                    .transpose()?
                    .unwrap_or(0);
                match present_result {
                    NativePresentResult::AsyncSubmitted { token } => {
                        frame_scheduler
                            .note_async_submission(token, monotonic_now_ns()?)
                            .map_err(io::Error::other)?;
                        frame_submitted = true;
                    }
                    NativePresentResult::Immediate => {
                        frame_scheduler.note_immediate_completion();
                        if server.has_pending_frame_work() {
                            let finish_frame_start = Instant::now();
                            server.finish_frame();
                            frame_completed = true;
                            perf.log("native.finish_frame", || {
                                vec![
                                    NativePerfField::str("reason", "immediate_scanout"),
                                    NativePerfField::u64(
                                        "elapsed_us",
                                        elapsed_micros(finish_frame_start),
                                    ),
                                    NativePerfField::usize(
                                        "surfaces",
                                        server.renderable_surfaces().len(),
                                    ),
                                    NativePerfField::u64(
                                        "render_generation",
                                        server.render_generation(),
                                    ),
                                ]
                            });
                        }
                    }
                    NativePresentResult::Noop => {
                        return Err(io::Error::other(
                            "native scanout rendered a frame but did not submit or complete it",
                        )
                        .into());
                    }
                }
                last_acquire_ready_at_ns = None;
                frame_index = frame_index.saturating_add(1);
                perf.log("native.frame", || {
                    let mut fields = paint_stats.fields();
                    fields.extend(output_damage.fields());
                    fields.extend([
                        NativePerfField::u64("index", frame_index),
                        NativePerfField::str("phase", "repaint"),
                        NativePerfField::str("mode", mode_label.clone()),
                        NativePerfField::str("cursor", cursor_render_mode.as_str()),
                        NativePerfField::u64("refresh_hz", u64::from(refresh_hz)),
                        NativePerfField::usize("surfaces", server.renderable_surfaces().len()),
                        NativePerfField::u64("render_generation", render_generation),
                        NativePerfField::bool("render_changed", render_generation_changed),
                        NativePerfField::str("render_cause", render_cause),
                        NativePerfField::u64("tick_us", tick_us),
                        NativePerfField::bool("pageflip_pending_at_tick", pageflip_pending_at_tick),
                        NativePerfField::u64("input_drain_us", input_drain_us),
                        NativePerfField::usize("raw_input_events", raw_input_events),
                        NativePerfField::usize("coalesced_input_events", coalesced_input_events),
                        NativePerfField::u64("pageflip_drain_us", pageflip_drain_us),
                        NativePerfField::bool("pageflip_completed", pageflip_completed),
                        NativePerfField::u64("present_us", present_us),
                        NativePerfField::u64("repaint_present_us", repaint_present_us),
                        NativePerfField::u64(
                            "acquire_ready_to_render_submit_us",
                            acquire_ready_to_render_submit_us,
                        ),
                        NativePerfField::u64("cpu_user_us", cpu_user_us),
                        NativePerfField::u64("cpu_system_us", cpu_system_us),
                        NativePerfField::bool("pending_frame_work", pending_frame_work),
                        NativePerfField::bool("redraw_requested", redraw_requested),
                        NativePerfField::usize("skipped_input_repaints", skipped_input_repaints),
                        NativePerfField::usize("accepted_clients", accepted),
                    ]);
                    fields
                });
                queued_redraw_requested = false;
                last_render_generation = render_generation;
                last_renderable_surfaces = server.renderable_surfaces().to_vec();
            }
        } else if scheduler_decision == SchedulerDecision::CompleteProtocolOnly {
            perf.log("native.frame_skip", || {
                vec![
                    NativePerfField::str("reason", "frame_callback_no_damage"),
                    NativePerfField::usize("skipped_input_repaints", skipped_input_repaints),
                    NativePerfField::u64("tick_us", tick_us),
                    NativePerfField::bool("pageflip_pending_at_tick", pageflip_pending_at_tick),
                    NativePerfField::u64("input_drain_us", input_drain_us),
                    NativePerfField::usize("raw_input_events", raw_input_events),
                    NativePerfField::usize("coalesced_input_events", coalesced_input_events),
                    NativePerfField::u64("pageflip_drain_us", pageflip_drain_us),
                    NativePerfField::bool("pageflip_completed", pageflip_completed),
                    NativePerfField::u64("present_us", present_us),
                    NativePerfField::u64("render_generation", render_generation),
                ]
            });
            let finish_frame_start = Instant::now();
            server.finish_frame();
            frame_scheduler.complete_protocol_only();
            frame_completed = true;
            perf.log("native.finish_frame", || {
                vec![
                    NativePerfField::str("reason", "frame_callback_no_damage"),
                    NativePerfField::u64("elapsed_us", elapsed_micros(finish_frame_start)),
                    NativePerfField::usize("surfaces", server.renderable_surfaces().len()),
                    NativePerfField::u64("render_generation", server.render_generation()),
                ]
            });
        } else if scheduler_decision == SchedulerDecision::WaitForPageFlip {
            perf.log("native.frame_skip", || {
                vec![
                    NativePerfField::str("reason", "pageflip_pending"),
                    NativePerfField::usize("skipped_input_repaints", skipped_input_repaints),
                    NativePerfField::u64("tick_us", tick_us),
                    NativePerfField::bool("pageflip_pending_at_tick", pageflip_pending_at_tick),
                    NativePerfField::u64("input_drain_us", input_drain_us),
                    NativePerfField::usize("raw_input_events", raw_input_events),
                    NativePerfField::usize("coalesced_input_events", coalesced_input_events),
                    NativePerfField::u64("pageflip_drain_us", pageflip_drain_us),
                    NativePerfField::bool("pageflip_completed", pageflip_completed),
                    NativePerfField::u64("present_us", present_us),
                    NativePerfField::u64("render_generation", render_generation),
                    NativePerfField::bool("render_changed", render_generation_changed),
                    NativePerfField::bool("pending_frame_work", pending_frame_work),
                    NativePerfField::bool("redraw_requested", redraw_requested),
                ]
            });
        } else if skipped_input_repaints > 0 {
            perf.log("native.frame_skip", || {
                vec![
                    NativePerfField::str("reason", "input_forwarded_no_visual"),
                    NativePerfField::usize("skipped_input_repaints", skipped_input_repaints),
                    NativePerfField::u64("tick_us", tick_us),
                    NativePerfField::bool("pageflip_pending_at_tick", pageflip_pending_at_tick),
                    NativePerfField::u64("input_drain_us", input_drain_us),
                    NativePerfField::usize("raw_input_events", raw_input_events),
                    NativePerfField::usize("coalesced_input_events", coalesced_input_events),
                    NativePerfField::u64("pageflip_drain_us", pageflip_drain_us),
                    NativePerfField::bool("pageflip_completed", pageflip_completed),
                    NativePerfField::u64("present_us", present_us),
                    NativePerfField::u64("render_generation", render_generation),
                ]
            });
        }
        perf.log("native.scheduler", || {
            vec![
                NativePerfField::str("decision", format!("{scheduler_decision:?}")),
                NativePerfField::str("state_after", format!("{:?}", frame_scheduler.state())),
                NativePerfField::bool("pageflip_pending", frame_scheduler.page_flip_pending()),
                NativePerfField::bool("visual_work_queued", frame_scheduler.visual_work_queued()),
                NativePerfField::bool(
                    "protocol_work_queued",
                    frame_scheduler.protocol_work_queued(),
                ),
                NativePerfField::bool("frame_rendered", frame_rendered),
                NativePerfField::bool("frame_submitted", frame_submitted),
                NativePerfField::bool("frame_completed", frame_completed),
                NativePerfField::u64(
                    "watchdog_timeout_count",
                    frame_scheduler.watchdog_timeout_count(),
                ),
                NativePerfField::u64("mismatched_pageflip_events", mismatched_pageflip_events),
                NativePerfField::u64("stale_pageflip_events", stale_pageflip_events),
            ]
        });
        event_loop.arm_deadline(earliest_native_deadline(
            frame_scheduler.next_deadline_ns(),
            acquire_watches.next_fallback_deadline_ns(),
        ))?;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NativeRepaintInputs {
    accepted_clients: bool,
    render_generation_changed: bool,
    pending_frame_work: bool,
    only_pending_surface_frame_callbacks: bool,
    redraw_requested: bool,
    page_flip_pending: bool,
}

fn earliest_native_deadline(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(deadline), None) | (None, Some(deadline)) => Some(deadline),
        (None, None) => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NativeRepaintDecision {
    repaint: bool,
    protocol_only_present: bool,
}

fn native_repaint_decision(inputs: NativeRepaintInputs) -> NativeRepaintDecision {
    if inputs.page_flip_pending {
        return NativeRepaintDecision {
            repaint: false,
            protocol_only_present: false,
        };
    }

    let protocol_only_present = inputs.pending_frame_work
        && inputs.only_pending_surface_frame_callbacks
        && !inputs.accepted_clients
        && !inputs.render_generation_changed
        && !inputs.redraw_requested;
    NativeRepaintDecision {
        repaint: inputs.accepted_clients
            || inputs.render_generation_changed
            || inputs.redraw_requested
            || (inputs.pending_frame_work && !protocol_only_present),
        protocol_only_present,
    }
}

fn normalize_refresh_hz(refresh_hz: u32) -> u32 {
    if refresh_hz == 0 {
        60
    } else {
        refresh_hz.clamp(30, 360)
    }
}

#[derive(Debug, Default)]
struct NativeFrameRenderer {
    scene_renderer: DesktopSceneRenderer,
    shell_overlay_renderer: ShellOverlayRenderer,
    frame: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeCursorRenderMode {
    Software,
    Hardware,
}

impl NativeCursorRenderMode {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Software => "software",
            Self::Hardware => "hardware",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeCursorPreference {
    Auto,
    Hardware,
    Software,
}

impl NativeCursorPreference {
    fn from_env() -> Self {
        match std::env::var("OBLIVION_ONE_CURSOR") {
            Ok(value) if matches!(value.as_str(), "hardware" | "hw" | "drm") => Self::Hardware,
            Ok(value) if matches!(value.as_str(), "software" | "sw" | "cpu") => Self::Software,
            Ok(value) if value == "auto" => Self::Auto,
            Ok(value) => {
                eprintln!("native cursor: unknown OBLIVION_ONE_CURSOR={value:?}; using auto");
                Self::Auto
            }
            Err(_) => Self::Auto,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Hardware => "hardware",
            Self::Software => "software",
        }
    }
}

#[derive(Debug)]
struct NativePointerConstraintBackend {
    active: Option<NativePointerConstraint>,
    cursor_visible: bool,
}

fn native_pointer_debug_log(message: impl AsRef<str>) {
    if std::env::var_os("TYPHON_POINTER_DEBUG").is_some() {
        eprintln!("typhon pointer: {}", message.as_ref());
    }
}

#[derive(Debug, Clone, PartialEq)]
struct NativePointerConstraint {
    id: PointerConstraintBackendId,
    mode: PointerConstraintMode,
    anchor: CompositorOutputPosition,
    region: Option<OutputRegion>,
}

#[derive(Debug, Default, PartialEq)]
struct NativePointerConstraintBackendAction {
    activated: Option<NativePointerConstraint>,
    deactivated: Option<PointerConstraintBackendId>,
    failed: Option<(PointerConstraintBackendId, &'static str)>,
    restore_position: Option<CompositorOutputPosition>,
    cursor_position: Option<CompositorOutputPosition>,
    cursor_visibility_changed: Option<bool>,
}

impl NativePointerConstraintBackend {
    fn new() -> Self {
        Self {
            active: None,
            cursor_visible: true,
        }
    }

    #[cfg(test)]
    fn active_locked(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(|constraint| constraint.mode == PointerConstraintMode::Locked)
    }

    fn active_constraint_state(&self) -> NativePointerConstraintState {
        match self.active.as_ref() {
            Some(NativePointerConstraint {
                mode: PointerConstraintMode::Locked,
                anchor,
                ..
            }) => NativePointerConstraintState::Locked { anchor: *anchor },
            Some(NativePointerConstraint {
                mode: PointerConstraintMode::Confined,
                region: Some(region),
                ..
            }) => NativePointerConstraintState::Confined {
                region: region.clone(),
            },
            _ => NativePointerConstraintState::None,
        }
    }

    fn handle_request(
        &mut self,
        request: PointerConstraintBackendRequest,
        cursor_position: CompositorOutputPosition,
    ) -> NativePointerConstraintBackendAction {
        match request {
            PointerConstraintBackendRequest::ActivateLocked { id, anchor } => {
                self.activate_locked(id, anchor)
            }
            PointerConstraintBackendRequest::ActivateConfined { id, region } => {
                self.activate_confined(id, cursor_position, region)
            }
            PointerConstraintBackendRequest::UpdateConfinedRegion { id, region } => {
                self.update_confined_region(id, cursor_position, region)
            }
            PointerConstraintBackendRequest::Deactivate {
                id,
                restore_position,
            } => self.deactivate(id, restore_position),
            PointerConstraintBackendRequest::WarpPointer { position } => {
                native_pointer_debug_log(format!(
                    "backend warp requested position=({},{})",
                    position.x, position.y
                ));
                NativePointerConstraintBackendAction {
                    cursor_position: Some(position),
                    ..NativePointerConstraintBackendAction::default()
                }
            }
            PointerConstraintBackendRequest::ApplyCursorVisibility { visible } => {
                if self.cursor_visible == visible {
                    NativePointerConstraintBackendAction::default()
                } else {
                    self.cursor_visible = visible;
                    NativePointerConstraintBackendAction {
                        cursor_visibility_changed: Some(visible),
                        ..NativePointerConstraintBackendAction::default()
                    }
                }
            }
        }
    }

    fn activate_locked(
        &mut self,
        id: PointerConstraintBackendId,
        anchor: CompositorOutputPosition,
    ) -> NativePointerConstraintBackendAction {
        if let Some(active) = self.active.as_ref() {
            if active.id == id {
                return NativePointerConstraintBackendAction::default();
            }
            return NativePointerConstraintBackendAction {
                failed: Some((id, "native pointer constraint already active")),
                ..NativePointerConstraintBackendAction::default()
            };
        }
        let constraint = NativePointerConstraint {
            id,
            mode: PointerConstraintMode::Locked,
            anchor,
            region: None,
        };
        self.active = Some(constraint.clone());
        NativePointerConstraintBackendAction {
            activated: Some(constraint),
            ..NativePointerConstraintBackendAction::default()
        }
    }

    fn activate_confined(
        &mut self,
        id: PointerConstraintBackendId,
        anchor: CompositorOutputPosition,
        region: OutputRegion,
    ) -> NativePointerConstraintBackendAction {
        if let Some(active) = self.active.as_ref() {
            if active.id == id {
                return NativePointerConstraintBackendAction::default();
            }
            return NativePointerConstraintBackendAction {
                failed: Some((id, "native pointer constraint already active")),
                ..NativePointerConstraintBackendAction::default()
            };
        }
        let constraint = NativePointerConstraint {
            id,
            mode: PointerConstraintMode::Confined,
            anchor,
            region: Some(region),
        };
        self.active = Some(constraint.clone());
        NativePointerConstraintBackendAction {
            activated: Some(constraint),
            ..NativePointerConstraintBackendAction::default()
        }
    }

    fn deactivate(
        &mut self,
        id: PointerConstraintBackendId,
        restore_position: Option<CompositorOutputPosition>,
    ) -> NativePointerConstraintBackendAction {
        let Some(active) = self.active.as_ref().cloned() else {
            return NativePointerConstraintBackendAction::default();
        };
        if active.id != id {
            return NativePointerConstraintBackendAction::default();
        }
        self.active = None;
        let restore_position = (active.mode == PointerConstraintMode::Locked)
            .then(|| restore_position.unwrap_or(active.anchor));
        NativePointerConstraintBackendAction {
            deactivated: Some(id),
            restore_position,
            ..NativePointerConstraintBackendAction::default()
        }
    }

    fn update_confined_region(
        &mut self,
        id: PointerConstraintBackendId,
        cursor_position: CompositorOutputPosition,
        region: OutputRegion,
    ) -> NativePointerConstraintBackendAction {
        let Some(active) = self.active.as_mut() else {
            return NativePointerConstraintBackendAction::default();
        };
        if active.id != id || active.mode != PointerConstraintMode::Confined {
            return NativePointerConstraintBackendAction::default();
        }
        active.region = Some(region.clone());
        let constrained = region.closest_point(cursor_position);
        NativePointerConstraintBackendAction {
            cursor_position: (constrained != cursor_position).then_some(constrained),
            ..NativePointerConstraintBackendAction::default()
        }
    }
}

impl Default for NativePointerConstraintBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl NativeFrameRenderer {
    fn render_server_frame(
        &mut self,
        width: u32,
        height: u32,
        server: &OwnCompositorServer,
        input_state: &NativeInputState,
        cursor_mode: NativeCursorRenderMode,
    ) -> NativeRenderedFrame<'_> {
        self.render_frame(NativeFrameRequest {
            width,
            height,
            surfaces: server.renderable_surfaces(),
            dock_items: server.shell_dock_items(),
            spotlight: input_state.spotlight(),
            shell_generation: input_state.shell_generation(),
            visual_state: input_state.desktop_visual_state(cursor_mode),
            render_generation: server.scene_render_generation(),
            client_cursor: server.client_cursor_render_state(),
        })
    }

    fn render_frame(&mut self, request: NativeFrameRequest<'_>) -> NativeRenderedFrame<'_> {
        let NativeFrameRequest {
            width,
            height,
            surfaces,
            dock_items,
            spotlight,
            shell_generation,
            visual_state,
            render_generation,
            client_cursor,
        } = request;
        let pixel_count = width.saturating_mul(height) as usize;
        self.frame.resize(pixel_count, 0);
        let shell_state = ShellOverlayState {
            topbar: ShellTopbarModel::visible("Oblivion One").with_trailing_text("Super+Space"),
            dock_items,
            spotlight: spotlight.clone(),
            generation: shell_generation,
        };
        let shell_overlay = self
            .shell_overlay_renderer
            .render(width, height, &shell_state);
        self.scene_renderer
            .compose_reusing_frame(DesktopComposeRequest {
                frame: &mut self.frame,
                frame_width: width,
                frame_height: height,
                output_scale: 1.0,
                surfaces,
                content_generation: native_scene_content_generation(
                    render_generation,
                    shell_overlay.generation,
                ),
                visual_state,
                shell_overlay: Some(shell_overlay),
                client_cursor,
            });
        NativeRenderedFrame {
            pixels: &self.frame,
            scene_rebuild_kind: self.scene_renderer.last_rebuild_kind(),
            frame_copy_kind: self.scene_renderer.last_frame_copy_kind(),
        }
    }

    fn egl_scene_draw_request<'a>(
        &'a mut self,
        width: u32,
        height: u32,
        server: &'a OwnCompositorServer,
        input_state: &NativeInputState,
        cursor_mode: NativeCursorRenderMode,
    ) -> EglSceneDrawRequest<'a> {
        let shell_state = ShellOverlayState {
            topbar: ShellTopbarModel::visible("Oblivion One").with_trailing_text("Super+Space"),
            dock_items: server.shell_dock_items(),
            spotlight: input_state.spotlight().clone(),
            generation: input_state.shell_generation(),
        };
        let shell_overlay = self
            .shell_overlay_renderer
            .render(width, height, &shell_state);
        EglSceneDrawRequest {
            width,
            height,
            surfaces: server.renderable_surfaces(),
            content_generation: native_scene_content_generation(
                server.scene_render_generation(),
                shell_overlay.generation,
            ),
            visual_state: input_state.desktop_visual_state(cursor_mode),
            output_scale: 1.0,
            shell_overlay: Some(shell_overlay),
            client_cursor: server.client_cursor_render_state(),
        }
    }
}

struct NativeRenderedFrame<'a> {
    pixels: &'a [u32],
    scene_rebuild_kind: DesktopSceneRebuildKind,
    frame_copy_kind: DesktopFrameCopyKind,
}

struct NativeFrameRequest<'a> {
    width: u32,
    height: u32,
    surfaces: &'a [RenderableSurface],
    dock_items: Vec<ShellDockItem>,
    spotlight: &'a SpotlightModel,
    shell_generation: u64,
    visual_state: DesktopVisualState,
    render_generation: u64,
    client_cursor: Option<oblivion_one::compositor::ClientCursorRenderState<'a>>,
}

const fn native_scene_content_generation(
    render_generation: u64,
    shell_overlay_generation: u64,
) -> u64 {
    render_generation
        .wrapping_mul(1_000_003)
        .wrapping_add(shell_overlay_generation)
}

const WAYLAND_SCROLL_LINE_DISTANCE: f64 = 15.0;
const EV_KEY: u16 = 0x01;
const EV_REL: u16 = 0x02;
const KEY_ESC: u16 = 1;
const KEY_1: u16 = 2;
const KEY_2: u16 = 3;
const KEY_3: u16 = 4;
const KEY_4: u16 = 5;
const KEY_5: u16 = 6;
const KEY_6: u16 = 7;
const KEY_7: u16 = 8;
const KEY_8: u16 = 9;
const KEY_9: u16 = 10;
const KEY_0: u16 = 11;
const KEY_MINUS: u16 = 12;
const KEY_EQUAL: u16 = 13;
const KEY_BACKSPACE: u16 = 14;
const KEY_Q: u16 = 16;
const KEY_W: u16 = 17;
const KEY_E: u16 = 18;
const KEY_R: u16 = 19;
const KEY_T: u16 = 20;
const KEY_Y: u16 = 21;
const KEY_U: u16 = 22;
const KEY_I: u16 = 23;
const KEY_O: u16 = 24;
const KEY_P: u16 = 25;
const KEY_ENTER: u16 = 28;
const KEY_LEFTCTRL: u16 = 29;
const KEY_A: u16 = 30;
const KEY_S: u16 = 31;
const KEY_D: u16 = 32;
const KEY_F: u16 = 33;
const KEY_G: u16 = 34;
const KEY_H: u16 = 35;
const KEY_J: u16 = 36;
const KEY_K: u16 = 37;
const KEY_L: u16 = 38;
const KEY_LEFTSHIFT: u16 = 42;
const KEY_Z: u16 = 44;
const KEY_X: u16 = 45;
const KEY_C: u16 = 46;
const KEY_V: u16 = 47;
const KEY_B: u16 = 48;
const KEY_N: u16 = 49;
const KEY_M: u16 = 50;
const KEY_COMMA: u16 = 51;
const KEY_DOT: u16 = 52;
const KEY_SLASH: u16 = 53;
const KEY_RIGHTSHIFT: u16 = 54;
const KEY_LEFTALT: u16 = 56;
const KEY_SPACE: u16 = 57;
const KEY_F11: u16 = 87;
const KEY_RIGHTCTRL: u16 = 97;
const KEY_RIGHTALT: u16 = 100;
const KEY_UP: u16 = 103;
const KEY_DOWN: u16 = 108;
const KEY_LEFTMETA: u16 = 125;
const KEY_RIGHTMETA: u16 = 126;
const BTN_LEFT: u16 = 0x110;
const BTN_RIGHT: u16 = 0x111;
const BTN_MIDDLE: u16 = 0x112;
const REL_X: u16 = 0x00;
const REL_Y: u16 = 0x01;
const REL_HWHEEL: u16 = 0x06;
const REL_WHEEL: u16 = 0x08;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NativeKeyboardEvent {
    key: u32,
    pressed: bool,
}

impl NativeKeyboardEvent {
    const fn new(key: u16, pressed: bool) -> Self {
        Self {
            key: key as u32,
            pressed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct NativePointerButtonEvent {
    button: u32,
    pressed: bool,
    x: f64,
    y: f64,
    output_width: u32,
    output_height: u32,
}

impl NativePointerButtonEvent {
    const fn new_at(
        button: u32,
        pressed: bool,
        x: f64,
        y: f64,
        output_width: u32,
        output_height: u32,
    ) -> Self {
        Self {
            button,
            pressed,
            x,
            y,
            output_width,
            output_height,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct RelativeMotion {
    dx: f64,
    dy: f64,
    dx_unaccelerated: f64,
    dy_unaccelerated: f64,
}

impl RelativeMotion {
    const fn accelerated_only(dx: f64, dy: f64) -> Self {
        Self {
            dx,
            dy,
            dx_unaccelerated: dx,
            dy_unaccelerated: dy,
        }
    }

    const fn is_zero(self) -> bool {
        self.dx == 0.0
            && self.dy == 0.0
            && self.dx_unaccelerated == 0.0
            && self.dy_unaccelerated == 0.0
    }

    const fn add(self, other: Self) -> Self {
        Self {
            dx: self.dx + other.dx,
            dy: self.dy + other.dy,
            dx_unaccelerated: self.dx_unaccelerated + other.dx_unaccelerated,
            dy_unaccelerated: self.dy_unaccelerated + other.dy_unaccelerated,
        }
    }
}

impl From<RelativeMotion> for CompositorRelativePointerMotion {
    fn from(motion: RelativeMotion) -> Self {
        Self {
            dx: motion.dx,
            dy: motion.dy,
            dx_unaccelerated: motion.dx_unaccelerated,
            dy_unaccelerated: motion.dy_unaccelerated,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct PointerMotionSample {
    timestamp_usec: u64,
    absolute: Option<(f64, f64)>,
    relative: Option<RelativeMotion>,
}

impl PointerMotionSample {
    const fn relative(timestamp_usec: u64, relative: RelativeMotion) -> Self {
        Self {
            timestamp_usec,
            absolute: None,
            relative: Some(relative),
        }
    }

    const fn absolute(timestamp_usec: u64, x: f64, y: f64) -> Self {
        Self {
            timestamp_usec,
            absolute: Some((x, y)),
            relative: None,
        }
    }

    fn coalesce(self, other: Self) -> Option<Self> {
        match (self.absolute, self.relative, other.absolute, other.relative) {
            (None, Some(left), None, Some(right)) => {
                Some(Self::relative(other.timestamp_usec, left.add(right)))
            }
            (Some(_), None, Some((x, y)), None) => Some(Self::absolute(other.timestamp_usec, x, y)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum NativeHardwareInputEvent {
    Key { code: u16, value: i32 },
    PointerButton { button: u32, pressed: bool },
    PointerMotion(PointerMotionSample),
    PointerAxis { horizontal: f64, vertical: f64 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum NativeWindowAction {
    BeginMove { x: f64, y: f64 },
    BeginResize { x: f64, y: f64 },
    UpdateInteraction { x: f64, y: f64 },
    EndInteraction,
    Minimize,
    RestoreMinimized,
    ToggleMaximize,
    ToggleFullscreen,
}

impl NativeHardwareInputEvent {
    const fn may_change_pointer_constraints(self) -> bool {
        matches!(self, Self::Key { .. } | Self::PointerButton { .. })
    }

    const fn timestamp_usec(self) -> Option<u64> {
        match self {
            Self::PointerMotion(sample) => Some(sample.timestamp_usec),
            Self::Key { .. } | Self::PointerButton { .. } | Self::PointerAxis { .. } => None,
        }
    }

    fn from_linux_event(event: LinuxInputEvent) -> Option<Self> {
        match event.type_ {
            EV_KEY if is_pointer_button(event.code) && event.value != 2 => {
                Some(Self::PointerButton {
                    button: u32::from(event.code),
                    pressed: event.value != 0,
                })
            }
            EV_KEY => Some(Self::Key {
                code: event.code,
                value: event.value,
            }),
            EV_REL => match event.code {
                REL_X => Some(Self::PointerMotion(PointerMotionSample::relative(
                    linux_input_event_time_usec(event),
                    RelativeMotion::accelerated_only(f64::from(event.value), 0.0),
                ))),
                REL_Y => Some(Self::PointerMotion(PointerMotionSample::relative(
                    linux_input_event_time_usec(event),
                    RelativeMotion::accelerated_only(0.0, f64::from(event.value)),
                ))),
                REL_WHEEL => Some(Self::PointerAxis {
                    horizontal: 0.0,
                    vertical: -f64::from(event.value) * WAYLAND_SCROLL_LINE_DISTANCE,
                }),
                REL_HWHEEL => Some(Self::PointerAxis {
                    horizontal: f64::from(event.value) * WAYLAND_SCROLL_LINE_DISTANCE,
                    vertical: 0.0,
                }),
                _ => None,
            },
            _ => None,
        }
    }
}

#[derive(Debug, Default, PartialEq)]
struct NativeInputEffect {
    redraw_requested: bool,
    visual_redraw_requested: bool,
    exit_requested: bool,
    cursor_moved: bool,
    cursor_position: Option<(i32, i32)>,
    keyboard_events: Vec<NativeKeyboardEvent>,
    pointer_motion: Option<(f64, f64)>,
    pointer_motion_usec: Option<u64>,
    relative_motion: Option<RelativeMotion>,
    pointer_buttons: Vec<NativePointerButtonEvent>,
    pointer_axis: Option<(f64, f64)>,
    window_actions: Vec<NativeWindowAction>,
    launch_command: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Default)]
enum NativePointerConstraintState {
    #[default]
    None,
    Locked {
        anchor: CompositorOutputPosition,
    },
    Confined {
        region: OutputRegion,
    },
}

impl NativePointerConstraintState {
    const fn locked(&self) -> bool {
        matches!(self, Self::Locked { .. })
    }

    fn constrain_position(&self, position: CompositorOutputPosition) -> CompositorOutputPosition {
        match self {
            Self::None | Self::Locked { .. } => position,
            Self::Confined { region } => region.closest_point(position),
        }
    }
}

impl NativeInputEffect {
    fn request_redraw(&mut self) {
        self.redraw_requested = true;
    }

    fn request_visual_redraw(&mut self) {
        self.redraw_requested = true;
        self.visual_redraw_requested = true;
    }

    fn mark_cursor_moved(&mut self, cursor_x: f64, cursor_y: f64) {
        self.cursor_moved = true;
        self.cursor_position = Some((cursor_x.round() as i32, cursor_y.round() as i32));
        self.request_redraw();
    }

    fn requires_frame_repaint(&self, cursor_mode: NativeCursorRenderMode) -> bool {
        if !self.redraw_requested {
            return false;
        }
        self.visual_redraw_requested
            || (cursor_mode == NativeCursorRenderMode::Software && self.cursor_moved)
    }
}

#[derive(Debug)]
struct NativeInputState {
    output_width: u32,
    output_height: u32,
    cursor_x: f64,
    cursor_y: f64,
    alt_pressed: bool,
    ctrl_pressed: bool,
    super_pressed: bool,
    shift_pressed: bool,
    window_interaction_active: bool,
    keyboard_shortcuts_inhibited: bool,
    pointer_constraint: NativePointerConstraintState,
    cursor_visible: bool,
    forwarded_control_keys: Vec<u16>,
    suppressed_window_shortcut_keys: Vec<u16>,
    spotlight: SpotlightModel,
    shell_generation: u64,
}

impl NativeInputState {
    fn new(output_width: u32, output_height: u32) -> Self {
        Self {
            output_width: output_width.max(1),
            output_height: output_height.max(1),
            cursor_x: f64::from(output_width.max(1)) / 2.0,
            cursor_y: f64::from(output_height.max(1)) / 2.0,
            alt_pressed: false,
            ctrl_pressed: false,
            super_pressed: false,
            shift_pressed: false,
            window_interaction_active: false,
            keyboard_shortcuts_inhibited: false,
            pointer_constraint: NativePointerConstraintState::None,
            cursor_visible: true,
            forwarded_control_keys: Vec::new(),
            suppressed_window_shortcut_keys: Vec::new(),
            spotlight: SpotlightModel::default(),
            shell_generation: 0,
        }
    }

    fn spotlight_visible(&self) -> bool {
        self.spotlight.is_visible()
    }

    #[cfg(test)]
    fn spotlight_query(&self) -> &str {
        self.spotlight.query()
    }

    const fn shell_generation(&self) -> u64 {
        self.shell_generation
    }

    const fn spotlight(&self) -> &SpotlightModel {
        &self.spotlight
    }

    #[cfg(test)]
    fn set_keyboard_shortcuts_inhibited(&mut self, inhibited: bool) {
        self.keyboard_shortcuts_inhibited = inhibited;
    }

    fn cursor_position(&self) -> (i32, i32) {
        (self.cursor_x.round() as i32, self.cursor_y.round() as i32)
    }

    fn cursor_position_f64(&self) -> CompositorOutputPosition {
        CompositorOutputPosition {
            x: self.cursor_x,
            y: self.cursor_y,
        }
    }

    fn set_pointer_locked_at(&mut self, anchor: CompositorOutputPosition) {
        self.pointer_constraint = NativePointerConstraintState::Locked { anchor };
    }

    fn set_pointer_confined(&mut self, region: OutputRegion) {
        self.pointer_constraint = NativePointerConstraintState::Confined { region };
        let position = self
            .pointer_constraint
            .constrain_position(CompositorOutputPosition {
                x: self.cursor_x,
                y: self.cursor_y,
            });
        self.cursor_x = position.x;
        self.cursor_y = position.y;
    }

    fn clear_pointer_constraint(&mut self) {
        self.pointer_constraint = NativePointerConstraintState::None;
    }

    fn set_cursor_visible(&mut self, visible: bool) -> bool {
        if self.cursor_visible == visible {
            return false;
        }
        self.cursor_visible = visible;
        true
    }

    fn restore_cursor_position(&mut self, position: CompositorOutputPosition) -> NativeInputEffect {
        self.cursor_x = position.x.clamp(0.0, f64::from(self.output_width - 1));
        self.cursor_y = position.y.clamp(0.0, f64::from(self.output_height - 1));
        let mut effect = NativeInputEffect::default();
        effect.mark_cursor_moved(self.cursor_x, self.cursor_y);
        effect
    }

    fn desktop_visual_state(&self, cursor_mode: NativeCursorRenderMode) -> DesktopVisualState {
        match cursor_mode {
            NativeCursorRenderMode::Software if self.cursor_visible => {
                let (x, y) = self.cursor_position();
                DesktopVisualState::with_cursor(x, y)
            }
            NativeCursorRenderMode::Software | NativeCursorRenderMode::Hardware => {
                DesktopVisualState::wallpaper_only()
            }
        }
    }

    fn handle_hardware_input_event(
        &mut self,
        event: NativeHardwareInputEvent,
    ) -> NativeInputEffect {
        match event {
            NativeHardwareInputEvent::Key { code, value } => self.handle_key_event(code, value),
            NativeHardwareInputEvent::PointerButton { button, pressed } => {
                self.handle_pointer_button(button, pressed)
            }
            NativeHardwareInputEvent::PointerMotion(sample) => self.handle_pointer_motion(sample),
            NativeHardwareInputEvent::PointerAxis {
                horizontal,
                vertical,
            } => self.handle_pointer_axis(horizontal, vertical),
        }
    }

    fn handle_key_event(&mut self, code: u16, value: i32) -> NativeInputEffect {
        let pressed = value != 0;
        let repeated = value == 2;
        let mut effect = NativeInputEffect::default();

        if is_shift_key(code) {
            self.shift_pressed = pressed;
            if !self.spotlight_visible() && !repeated {
                effect
                    .keyboard_events
                    .push(NativeKeyboardEvent::new(code, pressed));
                effect.request_redraw();
            }
            return effect;
        }

        if is_alt_key(code) {
            self.alt_pressed = pressed;
            if self.keyboard_shortcuts_inhibited && !repeated {
                effect
                    .keyboard_events
                    .push(NativeKeyboardEvent::new(code, pressed));
                effect.request_redraw();
            }
            if !pressed && self.window_interaction_active {
                self.window_interaction_active = false;
                effect
                    .window_actions
                    .push(NativeWindowAction::EndInteraction);
                effect.request_visual_redraw();
            }
            return effect;
        }

        if is_super_key(code) {
            self.super_pressed = pressed;
            if self.keyboard_shortcuts_inhibited && !repeated {
                effect
                    .keyboard_events
                    .push(NativeKeyboardEvent::new(code, pressed));
                effect.request_redraw();
            }
            return effect;
        }

        if is_control_key(code) {
            self.ctrl_pressed = pressed;
            if self.spotlight_visible() {
                return effect;
            }
            if pressed {
                if !self.forwarded_control_keys.contains(&code) {
                    self.forwarded_control_keys.push(code);
                    effect
                        .keyboard_events
                        .push(NativeKeyboardEvent::new(code, true));
                    effect.request_redraw();
                }
            } else if self.release_forwarded_control_key(code) {
                effect
                    .keyboard_events
                    .push(NativeKeyboardEvent::new(code, false));
                effect.request_redraw();
            }
            return effect;
        }

        if pressed && !repeated && self.alt_pressed && code == KEY_P {
            effect.exit_requested = true;
            return effect;
        }

        if self.keyboard_shortcuts_inhibited && !self.spotlight_visible() {
            if !repeated {
                effect
                    .keyboard_events
                    .push(NativeKeyboardEvent::new(code, pressed));
                effect.request_redraw();
            }
            return effect;
        }

        if !pressed && self.release_suppressed_window_shortcut_key(code) {
            return effect;
        }

        if pressed && !repeated && self.is_spotlight_toggle_key(code) {
            effect
                .keyboard_events
                .extend(self.release_forwarded_control_modifiers());
            self.spotlight.toggle();
            self.bump_shell_generation();
            effect.request_visual_redraw();
            return effect;
        }

        if self.spotlight_visible() {
            if pressed {
                self.handle_spotlight_key(code, &mut effect);
            }
            return effect;
        }

        if let Some(shortcut) = native_window_management_shortcut(self.alt_pressed, code) {
            if pressed && !repeated && self.suppress_window_shortcut_key(code) {
                effect.window_actions.push(shortcut.into_action());
                effect.request_visual_redraw();
            }
            return effect;
        }

        if self.alt_pressed {
            return effect;
        }

        if !repeated {
            effect
                .keyboard_events
                .push(NativeKeyboardEvent::new(code, pressed));
            effect.request_redraw();
        }
        effect
    }

    fn handle_spotlight_key(&mut self, code: u16, effect: &mut NativeInputEffect) {
        match code {
            KEY_ESC => {
                self.spotlight.hide();
                self.bump_shell_generation();
                effect.request_visual_redraw();
            }
            KEY_BACKSPACE => {
                if self.spotlight.backspace() {
                    self.bump_shell_generation();
                    effect.request_visual_redraw();
                }
            }
            KEY_DOWN => {
                if self.spotlight.select_next() {
                    self.bump_shell_generation();
                    effect.request_visual_redraw();
                }
            }
            KEY_UP => {
                if self.spotlight.select_previous() {
                    self.bump_shell_generation();
                    effect.request_visual_redraw();
                }
            }
            KEY_ENTER => {
                effect.launch_command = self.spotlight.selected_launch_command();
                self.spotlight.hide();
                self.bump_shell_generation();
                effect.request_visual_redraw();
            }
            _ => {
                if let Some(text) = evdev_key_to_text(code, self.shift_pressed) {
                    self.spotlight.push_text(text);
                    self.bump_shell_generation();
                    effect.request_visual_redraw();
                }
            }
        }
    }

    fn handle_pointer_button(&mut self, button: u32, pressed: bool) -> NativeInputEffect {
        if self.spotlight_visible() {
            return NativeInputEffect::default();
        }
        let mut effect = NativeInputEffect::default();
        if self.window_interaction_active && !pressed {
            self.window_interaction_active = false;
            effect
                .window_actions
                .push(NativeWindowAction::EndInteraction);
            effect.request_visual_redraw();
            return effect;
        }

        if pressed
            && let Some(action) =
                native_window_drag_action(self.alt_pressed, button, self.cursor_x, self.cursor_y)
        {
            self.window_interaction_active = true;
            effect.window_actions.push(action);
            effect.request_visual_redraw();
            return effect;
        }

        effect
            .pointer_buttons
            .push(NativePointerButtonEvent::new_at(
                button,
                pressed,
                self.cursor_x,
                self.cursor_y,
                self.output_width,
                self.output_height,
            ));
        effect.request_redraw();
        effect
    }

    fn handle_pointer_motion(&mut self, sample: PointerMotionSample) -> NativeInputEffect {
        let mut effect = NativeInputEffect {
            pointer_motion_usec: Some(sample.timestamp_usec),
            ..NativeInputEffect::default()
        };
        let locked_at_start = self.pointer_constraint.locked();
        let shell_captures_pointer = self.spotlight_visible();
        if let Some(relative) = sample.relative {
            effect.relative_motion =
                (!shell_captures_pointer && !relative.is_zero()).then_some(relative);
            if !self.pointer_constraint.locked() {
                let proposed = CompositorOutputPosition {
                    x: (self.cursor_x + relative.dx).clamp(0.0, f64::from(self.output_width - 1)),
                    y: (self.cursor_y + relative.dy).clamp(0.0, f64::from(self.output_height - 1)),
                };
                let constrained = self.pointer_constraint.constrain_position(proposed);
                self.cursor_x = constrained.x;
                self.cursor_y = constrained.y;
            }
        }
        if let Some((x, y)) = sample.absolute
            && !self.pointer_constraint.locked()
        {
            let proposed = CompositorOutputPosition {
                x: x.clamp(0.0, f64::from(self.output_width - 1)),
                y: y.clamp(0.0, f64::from(self.output_height - 1)),
            };
            let constrained = self.pointer_constraint.constrain_position(proposed);
            self.cursor_x = constrained.x;
            self.cursor_y = constrained.y;
        }
        if !self.pointer_constraint.locked() && !self.spotlight_visible() {
            if self.window_interaction_active {
                effect
                    .window_actions
                    .push(NativeWindowAction::UpdateInteraction {
                        x: self.cursor_x,
                        y: self.cursor_y,
                    });
                effect.request_visual_redraw();
            } else {
                effect.pointer_motion = Some((self.cursor_x, self.cursor_y));
            }
        }
        if !self.pointer_constraint.locked() {
            effect.mark_cursor_moved(self.cursor_x, self.cursor_y);
        }
        native_pointer_debug_log(format!(
            "pointer.motion native locked={} absolute_updated={} relative=({},{}) cursor=({},{})",
            locked_at_start,
            effect.pointer_motion.is_some(),
            sample.relative.map(|relative| relative.dx).unwrap_or(0.0),
            sample.relative.map(|relative| relative.dy).unwrap_or(0.0),
            self.cursor_x,
            self.cursor_y
        ));
        effect
    }

    #[cfg(test)]
    fn handle_pointer_motion_delta(&mut self, dx: f64, dy: f64) -> NativeInputEffect {
        self.handle_pointer_motion(PointerMotionSample::relative(
            0,
            RelativeMotion::accelerated_only(dx, dy),
        ))
    }

    fn handle_pointer_axis(&mut self, horizontal: f64, vertical: f64) -> NativeInputEffect {
        let mut effect = NativeInputEffect::default();
        if !self.spotlight_visible() {
            effect.pointer_axis = Some((horizontal, vertical));
            effect.request_redraw();
        }
        effect
    }

    fn is_spotlight_toggle_key(&self, code: u16) -> bool {
        code == KEY_SPACE && (self.super_pressed || self.ctrl_pressed)
    }

    fn release_forwarded_control_key(&mut self, code: u16) -> bool {
        let Some(index) = self
            .forwarded_control_keys
            .iter()
            .position(|forwarded| *forwarded == code)
        else {
            return false;
        };
        self.forwarded_control_keys.swap_remove(index);
        true
    }

    fn release_forwarded_control_modifiers(&mut self) -> Vec<NativeKeyboardEvent> {
        self.forwarded_control_keys
            .drain(..)
            .map(|key| NativeKeyboardEvent::new(key, false))
            .collect()
    }

    fn suppress_window_shortcut_key(&mut self, code: u16) -> bool {
        if self.suppressed_window_shortcut_keys.contains(&code) {
            return false;
        }
        self.suppressed_window_shortcut_keys.push(code);
        true
    }

    fn release_suppressed_window_shortcut_key(&mut self, code: u16) -> bool {
        let Some(index) = self
            .suppressed_window_shortcut_keys
            .iter()
            .position(|suppressed| *suppressed == code)
        else {
            return false;
        };

        self.suppressed_window_shortcut_keys.swap_remove(index);
        true
    }

    fn bump_shell_generation(&mut self) {
        self.shell_generation = self.shell_generation.wrapping_add(1);
    }
}

fn is_shift_key(code: u16) -> bool {
    matches!(code, KEY_LEFTSHIFT | KEY_RIGHTSHIFT)
}

fn is_alt_key(code: u16) -> bool {
    matches!(code, KEY_LEFTALT | KEY_RIGHTALT)
}

fn is_super_key(code: u16) -> bool {
    matches!(code, KEY_LEFTMETA | KEY_RIGHTMETA)
}

fn is_control_key(code: u16) -> bool {
    matches!(code, KEY_LEFTCTRL | KEY_RIGHTCTRL)
}

fn is_pointer_button(code: u16) -> bool {
    matches!(code, BTN_LEFT | BTN_RIGHT | BTN_MIDDLE)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeWindowManagementShortcut {
    Minimize,
    RestoreMinimized,
    ToggleMaximize,
    ToggleFullscreen,
}

impl NativeWindowManagementShortcut {
    const fn into_action(self) -> NativeWindowAction {
        match self {
            Self::Minimize => NativeWindowAction::Minimize,
            Self::RestoreMinimized => NativeWindowAction::RestoreMinimized,
            Self::ToggleMaximize => NativeWindowAction::ToggleMaximize,
            Self::ToggleFullscreen => NativeWindowAction::ToggleFullscreen,
        }
    }
}

fn native_window_management_shortcut(
    alt_pressed: bool,
    code: u16,
) -> Option<NativeWindowManagementShortcut> {
    if !alt_pressed {
        return None;
    }

    match code {
        KEY_M => Some(NativeWindowManagementShortcut::Minimize),
        KEY_R => Some(NativeWindowManagementShortcut::RestoreMinimized),
        KEY_F => Some(NativeWindowManagementShortcut::ToggleMaximize),
        KEY_ENTER | KEY_F11 => Some(NativeWindowManagementShortcut::ToggleFullscreen),
        _ => None,
    }
}

fn native_window_drag_action(
    alt_pressed: bool,
    button: u32,
    x: f64,
    y: f64,
) -> Option<NativeWindowAction> {
    if !alt_pressed {
        return None;
    }

    match u16::try_from(button).ok()? {
        BTN_LEFT => Some(NativeWindowAction::BeginMove { x, y }),
        BTN_RIGHT => Some(NativeWindowAction::BeginResize { x, y }),
        _ => None,
    }
}

fn evdev_key_to_text(code: u16, shifted: bool) -> Option<&'static str> {
    match (code, shifted) {
        (KEY_A, false) => Some("a"),
        (KEY_A, true) => Some("A"),
        (KEY_B, false) => Some("b"),
        (KEY_B, true) => Some("B"),
        (KEY_C, false) => Some("c"),
        (KEY_C, true) => Some("C"),
        (KEY_D, false) => Some("d"),
        (KEY_D, true) => Some("D"),
        (KEY_E, false) => Some("e"),
        (KEY_E, true) => Some("E"),
        (KEY_F, false) => Some("f"),
        (KEY_F, true) => Some("F"),
        (KEY_G, false) => Some("g"),
        (KEY_G, true) => Some("G"),
        (KEY_H, false) => Some("h"),
        (KEY_H, true) => Some("H"),
        (KEY_I, false) => Some("i"),
        (KEY_I, true) => Some("I"),
        (KEY_J, false) => Some("j"),
        (KEY_J, true) => Some("J"),
        (KEY_K, false) => Some("k"),
        (KEY_K, true) => Some("K"),
        (KEY_L, false) => Some("l"),
        (KEY_L, true) => Some("L"),
        (KEY_M, false) => Some("m"),
        (KEY_M, true) => Some("M"),
        (KEY_N, false) => Some("n"),
        (KEY_N, true) => Some("N"),
        (KEY_O, false) => Some("o"),
        (KEY_O, true) => Some("O"),
        (KEY_P, false) => Some("p"),
        (KEY_P, true) => Some("P"),
        (KEY_Q, false) => Some("q"),
        (KEY_Q, true) => Some("Q"),
        (KEY_R, false) => Some("r"),
        (KEY_R, true) => Some("R"),
        (KEY_S, false) => Some("s"),
        (KEY_S, true) => Some("S"),
        (KEY_T, false) => Some("t"),
        (KEY_T, true) => Some("T"),
        (KEY_U, false) => Some("u"),
        (KEY_U, true) => Some("U"),
        (KEY_V, false) => Some("v"),
        (KEY_V, true) => Some("V"),
        (KEY_W, false) => Some("w"),
        (KEY_W, true) => Some("W"),
        (KEY_X, false) => Some("x"),
        (KEY_X, true) => Some("X"),
        (KEY_Y, false) => Some("y"),
        (KEY_Y, true) => Some("Y"),
        (KEY_Z, false) => Some("z"),
        (KEY_Z, true) => Some("Z"),
        (KEY_1, false) => Some("1"),
        (KEY_1, true) => Some("!"),
        (KEY_2, false) => Some("2"),
        (KEY_2, true) => Some("@"),
        (KEY_3, false) => Some("3"),
        (KEY_3, true) => Some("#"),
        (KEY_4, false) => Some("4"),
        (KEY_4, true) => Some("$"),
        (KEY_5, false) => Some("5"),
        (KEY_5, true) => Some("%"),
        (KEY_6, false) => Some("6"),
        (KEY_6, true) => Some("^"),
        (KEY_7, false) => Some("7"),
        (KEY_7, true) => Some("&"),
        (KEY_8, false) => Some("8"),
        (KEY_8, true) => Some("*"),
        (KEY_9, false) => Some("9"),
        (KEY_9, true) => Some("("),
        (KEY_0, false) => Some("0"),
        (KEY_0, true) => Some(")"),
        (KEY_SPACE, _) => Some(" "),
        (KEY_MINUS, false) => Some("-"),
        (KEY_MINUS, true) => Some("_"),
        (KEY_EQUAL, false) => Some("="),
        (KEY_EQUAL, true) => Some("+"),
        (KEY_COMMA, false) => Some(","),
        (KEY_COMMA, true) => Some("<"),
        (KEY_DOT, false) => Some("."),
        (KEY_DOT, true) => Some(">"),
        (KEY_SLASH, false) => Some("/"),
        (KEY_SLASH, true) => Some("?"),
        _ => None,
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxInputEvent {
    _time: libc::timeval,
    type_: u16,
    code: u16,
    value: i32,
}

fn linux_input_event_time_usec(event: LinuxInputEvent) -> u64 {
    let seconds = u64::try_from(event._time.tv_sec).unwrap_or(0);
    let micros = u64::try_from(event._time.tv_usec).unwrap_or(0);
    seconds.saturating_mul(1_000_000).saturating_add(micros)
}

fn open_native_seat_session(session_probe: &NativeSessionProbe) -> Option<NativeSeatSession> {
    let dependencies = session_probe.plan.dependencies;
    if !(dependencies.seat_manager_available && dependencies.libseat_available) {
        return None;
    }
    match NativeSeatSession::open() {
        Ok(session) => {
            println!(
                "native seat: acquired {}",
                session.seat_name().unwrap_or_else(|| "unknown".to_string())
            );
            Some(session)
        }
        Err(error) => {
            eprintln!("native seat: libseat activation failed; using direct fallbacks: {error}");
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeDrmBackendKind {
    Libseat,
    Direct,
    Unavailable,
}

impl NativeDrmBackendKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Libseat => "libseat DRM",
            Self::Direct => "direct DRM",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeDrmBackendPreference {
    Auto,
    Libseat,
    Direct,
}

impl NativeDrmBackendPreference {
    fn from_env() -> Self {
        match std::env::var("OBLIVION_ONE_DRM_BACKEND") {
            Ok(value) if matches!(value.as_str(), "seat" | "libseat" | "seat-drm") => Self::Libseat,
            Ok(value) if matches!(value.as_str(), "direct" | "kms") => Self::Direct,
            Ok(value) if value == "auto" => Self::Auto,
            Ok(value) => {
                eprintln!("native DRM: unknown OBLIVION_ONE_DRM_BACKEND={value:?}; using auto");
                Self::Auto
            }
            Err(_) => Self::Auto,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NativeDrmBackendChoice {
    preference: NativeDrmBackendPreference,
    seat_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NativeDrmBackendPlan {
    primary: NativeDrmBackendKind,
    fallbacks: Vec<NativeDrmBackendKind>,
}

impl NativeDrmBackendPlan {
    fn choose(choice: NativeDrmBackendChoice) -> Self {
        match choice.preference {
            NativeDrmBackendPreference::Libseat if choice.seat_available => Self {
                primary: NativeDrmBackendKind::Libseat,
                fallbacks: Vec::new(),
            },
            NativeDrmBackendPreference::Libseat => Self::unavailable(),
            NativeDrmBackendPreference::Direct => Self {
                primary: NativeDrmBackendKind::Direct,
                fallbacks: Vec::new(),
            },
            NativeDrmBackendPreference::Auto if choice.seat_available => Self {
                primary: NativeDrmBackendKind::Libseat,
                fallbacks: vec![NativeDrmBackendKind::Direct],
            },
            NativeDrmBackendPreference::Auto => Self {
                primary: NativeDrmBackendKind::Direct,
                fallbacks: Vec::new(),
            },
        }
    }

    fn unavailable() -> Self {
        Self {
            primary: NativeDrmBackendKind::Unavailable,
            fallbacks: Vec::new(),
        }
    }

    fn candidates(&self) -> impl Iterator<Item = NativeDrmBackendKind> + '_ {
        std::iter::once(self.primary).chain(self.fallbacks.iter().copied())
    }
}

enum NativeDrmDeviceStorage {
    SeatManaged(NativeSeatDeviceFile),
    Direct(fs::File),
}

struct NativeDrmDevice {
    kind: NativeDrmBackendKind,
    storage: NativeDrmDeviceStorage,
}

impl NativeDrmDevice {
    fn open(
        plan: NativeDrmBackendPlan,
        path: &Path,
        seat_session: Option<NativeSeatSession>,
    ) -> io::Result<Self> {
        let mut last_error = None;
        for candidate in plan.candidates() {
            match Self::open_kind(candidate, path, seat_session.as_ref()) {
                Ok(device) => return Ok(device),
                Err(error) => {
                    eprintln!("native DRM: {} backend failed: {error}", candidate.as_str());
                    last_error = Some(error);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| io::Error::other("native DRM backend is unavailable")))
    }

    fn open_kind(
        kind: NativeDrmBackendKind,
        path: &Path,
        seat_session: Option<&NativeSeatSession>,
    ) -> io::Result<Self> {
        match kind {
            NativeDrmBackendKind::Libseat => {
                let session = seat_session.ok_or_else(|| {
                    io::Error::other("libseat DRM requested but no active seat session exists")
                })?;
                let file = session.open_device_file(path)?;
                println!("native DRM: opened {} through libseat", path.display());
                Ok(Self {
                    kind,
                    storage: NativeDrmDeviceStorage::SeatManaged(file),
                })
            }
            NativeDrmBackendKind::Direct => {
                let file = OpenOptions::new().read(true).write(true).open(path)?;
                println!("native DRM: opened {} directly", path.display());
                Ok(Self {
                    kind,
                    storage: NativeDrmDeviceStorage::Direct(file),
                })
            }
            NativeDrmBackendKind::Unavailable => {
                Err(io::Error::other("native DRM backend is unavailable"))
            }
        }
    }

    const fn kind(&self) -> NativeDrmBackendKind {
        self.kind
    }

    fn file(&self) -> &fs::File {
        match &self.storage {
            NativeDrmDeviceStorage::SeatManaged(device) => device.file(),
            NativeDrmDeviceStorage::Direct(file) => file,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeInputBackendKind {
    LibseatLibinputUdev,
    DirectLibinputUdev,
    RawEvdev,
    Unavailable,
}

impl NativeInputBackendKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::LibseatLibinputUdev => "libseat + libinput udev",
            Self::DirectLibinputUdev => "direct libinput udev",
            Self::RawEvdev => "raw evdev fallback",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeInputBackendPreference {
    Auto,
    LibseatLibinputUdev,
    DirectLibinputUdev,
    RawEvdev,
}

impl NativeInputBackendPreference {
    fn from_env() -> Self {
        match std::env::var("OBLIVION_ONE_INPUT_BACKEND") {
            Ok(value) if matches!(value.as_str(), "seat" | "libseat" | "seat-libinput") => {
                Self::LibseatLibinputUdev
            }
            Ok(value)
                if matches!(
                    value.as_str(),
                    "libinput" | "udev" | "direct-libinput" | "libinput-direct"
                ) =>
            {
                Self::DirectLibinputUdev
            }
            Ok(value) if matches!(value.as_str(), "raw" | "evdev") => Self::RawEvdev,
            Ok(value) if value == "auto" => Self::Auto,
            Ok(value) => {
                eprintln!("native input: unknown OBLIVION_ONE_INPUT_BACKEND={value:?}; using auto");
                Self::Auto
            }
            Err(_) => Self::Auto,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NativeInputBackendChoice {
    preference: NativeInputBackendPreference,
    libseat_available: bool,
    libinput_available: bool,
    raw_evdev_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NativeInputBackendPlan {
    primary: NativeInputBackendKind,
    fallbacks: Vec<NativeInputBackendKind>,
}

impl NativeInputBackendPlan {
    fn choose(choice: NativeInputBackendChoice) -> Self {
        match choice.preference {
            NativeInputBackendPreference::LibseatLibinputUdev
                if choice.libseat_available && choice.libinput_available =>
            {
                Self {
                    primary: NativeInputBackendKind::LibseatLibinputUdev,
                    fallbacks: Vec::new(),
                }
            }
            NativeInputBackendPreference::LibseatLibinputUdev => Self::unavailable(),
            NativeInputBackendPreference::DirectLibinputUdev if choice.libinput_available => Self {
                primary: NativeInputBackendKind::DirectLibinputUdev,
                fallbacks: Vec::new(),
            },
            NativeInputBackendPreference::DirectLibinputUdev => Self::unavailable(),
            NativeInputBackendPreference::RawEvdev if choice.raw_evdev_available => Self {
                primary: NativeInputBackendKind::RawEvdev,
                fallbacks: Vec::new(),
            },
            NativeInputBackendPreference::RawEvdev => Self::unavailable(),
            NativeInputBackendPreference::Auto
                if choice.libseat_available && choice.libinput_available =>
            {
                let mut fallbacks = Vec::new();
                fallbacks.push(NativeInputBackendKind::DirectLibinputUdev);
                if choice.raw_evdev_available {
                    fallbacks.push(NativeInputBackendKind::RawEvdev);
                }
                Self {
                    primary: NativeInputBackendKind::LibseatLibinputUdev,
                    fallbacks,
                }
            }
            NativeInputBackendPreference::Auto if choice.libinput_available => {
                let mut fallbacks = Vec::new();
                if choice.raw_evdev_available {
                    fallbacks.push(NativeInputBackendKind::RawEvdev);
                }
                Self {
                    primary: NativeInputBackendKind::DirectLibinputUdev,
                    fallbacks,
                }
            }
            NativeInputBackendPreference::Auto if choice.raw_evdev_available => Self {
                primary: NativeInputBackendKind::RawEvdev,
                fallbacks: Vec::new(),
            },
            NativeInputBackendPreference::Auto => Self::unavailable(),
        }
    }

    fn unavailable() -> Self {
        Self {
            primary: NativeInputBackendKind::Unavailable,
            fallbacks: Vec::new(),
        }
    }

    fn candidates(&self) -> impl Iterator<Item = NativeInputBackendKind> + '_ {
        std::iter::once(self.primary).chain(self.fallbacks.iter().copied())
    }
}

enum NativeInputBackend {
    LibseatLibinput(LibinputInputBackend),
    DirectLibinput(LibinputInputBackend),
    RawEvdev(NativeInputDevices),
}

enum NativeInputEventFds<'a> {
    Libinput(Option<RawFd>),
    Raw(std::slice::Iter<'a, NativeInputDevice>),
}

impl Iterator for NativeInputEventFds<'_> {
    type Item = RawFd;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Libinput(fd) => fd.take(),
            Self::Raw(devices) => devices.next().map(|device| device.file.as_raw_fd()),
        }
    }
}

impl NativeInputBackend {
    fn open(
        plan: NativeInputBackendPlan,
        output_width: u32,
        output_height: u32,
        seat_session: Option<NativeSeatSession>,
    ) -> io::Result<Self> {
        let mut last_error = None;
        for candidate in plan.candidates() {
            match Self::open_kind(candidate, output_width, output_height, seat_session.clone()) {
                Ok(backend) => return Ok(backend),
                Err(error) => {
                    eprintln!(
                        "native input: {} backend failed: {error}",
                        candidate.as_str()
                    );
                    last_error = Some(error);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| {
            io::Error::other(
                "native input unavailable: no libseat/libinput backend or readable raw evdev fallback",
            )
        }))
    }

    fn open_kind(
        kind: NativeInputBackendKind,
        output_width: u32,
        output_height: u32,
        seat_session: Option<NativeSeatSession>,
    ) -> io::Result<Self> {
        match kind {
            NativeInputBackendKind::LibseatLibinputUdev => {
                let session = seat_session.ok_or_else(|| {
                    io::Error::other("libseat/libinput requested but no active seat session exists")
                })?;
                let backend = LibinputInputBackend::open_with_libseat(
                    session,
                    "seat0",
                    output_width,
                    output_height,
                )?;
                backend.ensure_initial_devices()?;
                Ok(Self::LibseatLibinput(backend))
            }
            NativeInputBackendKind::DirectLibinputUdev => {
                let backend =
                    LibinputInputBackend::open_direct("seat0", output_width, output_height)?;
                backend.ensure_initial_devices()?;
                Ok(Self::DirectLibinput(backend))
            }
            NativeInputBackendKind::RawEvdev => {
                Ok(Self::RawEvdev(NativeInputDevices::open_readable()))
            }
            NativeInputBackendKind::Unavailable => {
                Err(io::Error::other("native input backend is unavailable"))
            }
        }
    }

    const fn kind(&self) -> NativeInputBackendKind {
        match self {
            Self::LibseatLibinput(_) => NativeInputBackendKind::LibseatLibinputUdev,
            Self::DirectLibinput(_) => NativeInputBackendKind::DirectLibinputUdev,
            Self::RawEvdev(_) => NativeInputBackendKind::RawEvdev,
        }
    }

    fn event_fds(&self) -> NativeInputEventFds<'_> {
        match self {
            Self::LibseatLibinput(backend) | Self::DirectLibinput(backend) => {
                NativeInputEventFds::Libinput(Some(backend.input.as_fd().as_raw_fd()))
            }
            Self::RawEvdev(backend) => NativeInputEventFds::Raw(backend.devices.iter()),
        }
    }

    fn dispatch_session_events(&mut self) {
        if let Self::LibseatLibinput(backend) = self {
            backend.sync_seat_lifecycle();
        }
    }

    fn drain_events(&mut self) -> Vec<NativeHardwareInputEvent> {
        match self {
            Self::LibseatLibinput(backend) | Self::DirectLibinput(backend) => {
                backend.drain_events()
            }
            Self::RawEvdev(backend) => backend.drain_events(),
        }
    }
}

struct LibinputInputBackend {
    input: input::Libinput,
    seat_session: Option<NativeSeatSession>,
    seat_lifecycle: NativeSeatLifecycle,
    seat_name: String,
    output_width: u32,
    output_height: u32,
    device_count: usize,
}

impl LibinputInputBackend {
    fn open_with_libseat(
        seat_session: NativeSeatSession,
        seat_name: &str,
        output_width: u32,
        output_height: u32,
    ) -> io::Result<Self> {
        let assigned_seat = seat_session
            .seat_name()
            .unwrap_or_else(|| seat_name.to_string());
        let interface = SeatLibinputInterface::new(seat_session.clone());
        let mut input = input::Libinput::new_with_udev(interface);
        input.udev_assign_seat(&assigned_seat).map_err(|()| {
            io::Error::other(format!("failed to assign libinput seat {assigned_seat}"))
        })?;
        input.dispatch()?;
        let device_count = drain_initial_libinput_device_events(&mut input);
        println!(
            "native input: libseat/libinput assigned {assigned_seat}, {device_count} device(s)"
        );
        Ok(Self {
            input,
            seat_session: Some(seat_session),
            seat_lifecycle: NativeSeatLifecycle::default(),
            seat_name: assigned_seat,
            output_width,
            output_height,
            device_count,
        })
    }

    fn open_direct(seat_name: &str, output_width: u32, output_height: u32) -> io::Result<Self> {
        let mut input = input::Libinput::new_with_udev(DirectLibinputInterface);
        input.udev_assign_seat(seat_name).map_err(|()| {
            io::Error::other(format!("failed to assign libinput seat {seat_name}"))
        })?;
        input.dispatch()?;
        let device_count = drain_initial_libinput_device_events(&mut input);
        println!("native input: libinput assigned {seat_name}, {device_count} device(s)");
        Ok(Self {
            input,
            seat_session: None,
            seat_lifecycle: NativeSeatLifecycle::default(),
            seat_name: seat_name.to_string(),
            output_width,
            output_height,
            device_count,
        })
    }

    fn drain_events(&mut self) -> Vec<NativeHardwareInputEvent> {
        let mut events = Vec::new();
        self.sync_seat_lifecycle();
        if let Err(error) = self.input.dispatch() {
            eprintln!("native input: libinput dispatch failed: {error}");
            return events;
        }
        for event in &mut self.input {
            if let Some(event) =
                hardware_input_event_from_libinput(event, self.output_width, self.output_height)
            {
                events.push(event);
                if events.len() >= 256 {
                    break;
                }
            }
        }
        events
    }

    fn ensure_initial_devices(&self) -> io::Result<()> {
        if self.device_count == 0 {
            Err(io::Error::other(format!(
                "libinput seat {} reported no keyboard or pointer devices",
                self.seat_name
            )))
        } else {
            Ok(())
        }
    }

    fn sync_seat_lifecycle(&mut self) {
        let Some(session) = self.seat_session.as_ref() else {
            return;
        };
        if let Err(error) = session.dispatch() {
            eprintln!("native input: libseat dispatch failed: {error}");
        }
        for event in session.drain_events() {
            match self.seat_lifecycle.apply_event(event) {
                Some(NativeSeatInputAction::Suspend) => {
                    println!("native input: seat disabled; suspending libinput");
                    self.input.suspend();
                }
                Some(NativeSeatInputAction::Resume) => {
                    println!("native input: seat enabled; resuming libinput");
                    if let Err(()) = self.input.resume() {
                        eprintln!("native input: failed to resume libinput after seat enable");
                    }
                }
                None => {}
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeSeatEvent {
    Enabled,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeSeatInputAction {
    Resume,
    Suspend,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NativeSeatLifecycle {
    active: bool,
}

impl Default for NativeSeatLifecycle {
    fn default() -> Self {
        Self { active: true }
    }
}

impl NativeSeatLifecycle {
    fn apply_event(&mut self, event: NativeSeatEvent) -> Option<NativeSeatInputAction> {
        match event {
            NativeSeatEvent::Enabled if !self.active => {
                self.active = true;
                Some(NativeSeatInputAction::Resume)
            }
            NativeSeatEvent::Enabled => None,
            NativeSeatEvent::Disabled if self.active => {
                self.active = false;
                Some(NativeSeatInputAction::Suspend)
            }
            NativeSeatEvent::Disabled => None,
        }
    }
}

#[derive(Clone)]
struct NativeSeatSession {
    inner: Rc<RefCell<NativeSeatSessionInner>>,
    events: Rc<RefCell<Vec<NativeSeatEvent>>>,
    active: Rc<Cell<bool>>,
}

struct NativeSeatSessionInner {
    seat: libseat::Seat,
    devices: HashMap<RawFd, libseat::Device>,
}

struct NativeSeatDeviceFile {
    file: Option<fs::File>,
    key: RawFd,
    session: NativeSeatSession,
}

impl NativeSeatDeviceFile {
    fn file(&self) -> &fs::File {
        self.file
            .as_ref()
            .expect("seat-managed native device file should be open")
    }
}

impl Drop for NativeSeatDeviceFile {
    fn drop(&mut self) {
        if let Some(file) = self.file.take() {
            drop(file);
        }
        self.session.close_device_key(self.key);
    }
}

impl NativeSeatSession {
    fn open() -> io::Result<Self> {
        let active = Rc::new(Cell::new(false));
        let events = Rc::new(RefCell::new(Vec::new()));
        let callback_active = Rc::clone(&active);
        let callback_events = Rc::clone(&events);
        let seat = libseat::Seat::open(move |seat, event| match event {
            libseat::SeatEvent::Enable => {
                callback_active.set(true);
                callback_events.borrow_mut().push(NativeSeatEvent::Enabled);
            }
            libseat::SeatEvent::Disable => {
                callback_active.set(false);
                callback_events.borrow_mut().push(NativeSeatEvent::Disabled);
                if let Err(error) = seat.disable() {
                    eprintln!("native seat: failed to acknowledge libseat disable: {error}");
                }
            }
        })
        .map_err(io::Error::from)?;

        let session = Self {
            inner: Rc::new(RefCell::new(NativeSeatSessionInner {
                seat,
                devices: HashMap::new(),
            })),
            events,
            active,
        };
        session.wait_for_activation()?;
        Ok(session)
    }

    fn seat_name(&self) -> Option<String> {
        let mut inner = self.inner.try_borrow_mut().ok()?;
        Some(inner.seat.name().to_string())
    }

    fn dispatch(&self) -> io::Result<()> {
        let mut inner = self.inner.borrow_mut();
        inner.seat.dispatch(0).map(|_| ()).map_err(io::Error::from)
    }

    fn wait_for_activation(&self) -> io::Result<()> {
        for _ in 0..10 {
            if self.active.get() {
                return Ok(());
            }
            let mut inner = self.inner.borrow_mut();
            inner.seat.dispatch(50).map_err(io::Error::from)?;
        }
        if self.active.get() {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "libseat did not activate this session",
            ))
        }
    }

    fn drain_events(&self) -> Vec<NativeSeatEvent> {
        std::mem::take(&mut *self.events.borrow_mut())
    }

    fn open_device_file(&self, path: &Path) -> io::Result<NativeSeatDeviceFile> {
        let fd = self
            .open_device_fd(path)
            .map_err(io::Error::from_raw_os_error)?;
        let key = fd.as_raw_fd();
        Ok(NativeSeatDeviceFile {
            file: Some(fs::File::from(fd)),
            key,
            session: self.clone(),
        })
    }

    fn open_restricted(&self, path: &Path, _flags: i32) -> Result<OwnedFd, i32> {
        self.open_device_fd(path)
    }

    fn open_device_fd(&self, path: &Path) -> Result<OwnedFd, i32> {
        if !self.active.get() {
            return Err(libc::EACCES);
        }
        let mut inner = self.inner.borrow_mut();
        let device = inner.seat.open_device(&path).map_err(i32::from)?;
        let duplicated_fd = duplicate_fd_cloexec(device.as_fd().as_raw_fd())?;
        let key = duplicated_fd.as_raw_fd();
        inner.devices.insert(key, device);
        Ok(duplicated_fd)
    }

    fn close_restricted(&self, fd: OwnedFd) {
        let key = fd.as_raw_fd();
        drop(fd);
        self.close_device_key(key);
    }

    fn close_device_key(&self, key: RawFd) {
        let mut inner = self.inner.borrow_mut();
        let Some(device) = inner.devices.remove(&key) else {
            return;
        };
        if let Err(error) = inner.seat.close_device(device) {
            eprintln!("native seat: failed to close libseat device: {error}");
        }
    }
}

fn duplicate_fd_cloexec(fd: RawFd) -> Result<OwnedFd, i32> {
    let duplicated = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if duplicated < 0 {
        Err(io::Error::last_os_error()
            .raw_os_error()
            .unwrap_or(libc::EIO))
    } else {
        Ok(unsafe { OwnedFd::from_raw_fd(duplicated) })
    }
}

#[derive(Clone)]
struct SeatLibinputInterface {
    session: NativeSeatSession,
}

impl SeatLibinputInterface {
    fn new(session: NativeSeatSession) -> Self {
        Self { session }
    }
}

impl input::LibinputInterface for SeatLibinputInterface {
    fn open_restricted(&mut self, path: &Path, flags: i32) -> Result<OwnedFd, i32> {
        self.session.open_restricted(path, flags)
    }

    fn close_restricted(&mut self, fd: OwnedFd) {
        self.session.close_restricted(fd);
    }
}

struct DirectLibinputInterface;

impl input::LibinputInterface for DirectLibinputInterface {
    fn open_restricted(&mut self, path: &Path, flags: i32) -> Result<OwnedFd, i32> {
        let access_mode = flags & libc::O_ACCMODE;
        OpenOptions::new()
            .custom_flags(flags | libc::O_CLOEXEC)
            .read(matches!(access_mode, libc::O_RDONLY | libc::O_RDWR))
            .write(matches!(access_mode, libc::O_WRONLY | libc::O_RDWR))
            .open(path)
            .map(Into::into)
            .map_err(|error| error.raw_os_error().unwrap_or(libc::EIO))
    }

    fn close_restricted(&mut self, fd: OwnedFd) {
        drop(fd);
    }
}

fn drain_initial_libinput_device_events(input: &mut input::Libinput) -> usize {
    input
        .filter(|event| matches!(event, input::Event::Device(_)))
        .count()
}

fn hardware_input_event_from_libinput(
    event: input::Event,
    output_width: u32,
    output_height: u32,
) -> Option<NativeHardwareInputEvent> {
    use input::event::keyboard::{KeyState, KeyboardEvent, KeyboardEventTrait};
    #[allow(deprecated)]
    use input::event::pointer::{
        Axis, ButtonState, PointerEvent, PointerEventTrait, PointerScrollEvent,
    };

    match event {
        input::Event::Keyboard(KeyboardEvent::Key(event)) => {
            let code = u16::try_from(event.key()).ok()?;
            let value = match event.key_state() {
                KeyState::Pressed => 1,
                KeyState::Released => 0,
            };
            Some(NativeHardwareInputEvent::Key { code, value })
        }
        input::Event::Pointer(PointerEvent::Motion(event)) => Some(
            NativeHardwareInputEvent::PointerMotion(PointerMotionSample::relative(
                event.time_usec(),
                RelativeMotion {
                    dx: event.dx(),
                    dy: event.dy(),
                    dx_unaccelerated: event.dx_unaccelerated(),
                    dy_unaccelerated: event.dy_unaccelerated(),
                },
            )),
        ),
        input::Event::Pointer(PointerEvent::MotionAbsolute(event)) => Some(
            NativeHardwareInputEvent::PointerMotion(PointerMotionSample::absolute(
                event.time_usec(),
                event.absolute_x_transformed(output_width),
                event.absolute_y_transformed(output_height),
            )),
        ),
        input::Event::Pointer(PointerEvent::Button(event)) => {
            Some(NativeHardwareInputEvent::PointerButton {
                button: event.button(),
                pressed: event.button_state() == ButtonState::Pressed,
            })
        }
        #[allow(deprecated)]
        input::Event::Pointer(PointerEvent::Axis(event)) => {
            let horizontal = libinput_scroll_axis_value(event.has_axis(Axis::Horizontal), || {
                event.axis_value(Axis::Horizontal)
            });
            let vertical = libinput_scroll_axis_value(event.has_axis(Axis::Vertical), || {
                event.axis_value(Axis::Vertical)
            });
            Some(NativeHardwareInputEvent::PointerAxis {
                horizontal,
                vertical,
            })
        }
        input::Event::Pointer(PointerEvent::ScrollWheel(event)) => {
            let horizontal = libinput_scroll_axis_value(event.has_axis(Axis::Horizontal), || {
                event.scroll_value(Axis::Horizontal)
            });
            let vertical = libinput_scroll_axis_value(event.has_axis(Axis::Vertical), || {
                event.scroll_value(Axis::Vertical)
            });
            Some(NativeHardwareInputEvent::PointerAxis {
                horizontal,
                vertical,
            })
        }
        input::Event::Pointer(PointerEvent::ScrollFinger(event)) => {
            let horizontal = libinput_scroll_axis_value(event.has_axis(Axis::Horizontal), || {
                event.scroll_value(Axis::Horizontal)
            });
            let vertical = libinput_scroll_axis_value(event.has_axis(Axis::Vertical), || {
                event.scroll_value(Axis::Vertical)
            });
            Some(NativeHardwareInputEvent::PointerAxis {
                horizontal,
                vertical,
            })
        }
        input::Event::Pointer(PointerEvent::ScrollContinuous(event)) => {
            let horizontal = libinput_scroll_axis_value(event.has_axis(Axis::Horizontal), || {
                event.scroll_value(Axis::Horizontal)
            });
            let vertical = libinput_scroll_axis_value(event.has_axis(Axis::Vertical), || {
                event.scroll_value(Axis::Vertical)
            });
            Some(NativeHardwareInputEvent::PointerAxis {
                horizontal,
                vertical,
            })
        }
        _ => None,
    }
}

fn libinput_scroll_axis_value<F>(has_axis: bool, read_value: F) -> f64
where
    F: FnOnce() -> f64,
{
    if has_axis { read_value() } else { 0.0 }
}

#[derive(Debug, Clone, Copy)]
enum PendingPointerMotion {
    Sample(PointerMotionSample),
}

fn coalesce_pointer_motion_events(
    events: Vec<NativeHardwareInputEvent>,
) -> Vec<NativeHardwareInputEvent> {
    let mut coalesced = Vec::with_capacity(events.len());
    let mut pending_motion = None;

    for event in events {
        match event {
            NativeHardwareInputEvent::PointerMotion(sample) => match pending_motion {
                Some(PendingPointerMotion::Sample(pending_sample)) => {
                    if let Some(coalesced_sample) = pending_sample.coalesce(sample) {
                        pending_motion = Some(PendingPointerMotion::Sample(coalesced_sample));
                    } else {
                        flush_pending_pointer_motion(
                            &mut coalesced,
                            PendingPointerMotion::Sample(pending_sample),
                        );
                        pending_motion = Some(PendingPointerMotion::Sample(sample));
                    }
                }
                None => pending_motion = Some(PendingPointerMotion::Sample(sample)),
            },
            event => {
                if let Some(pending) = pending_motion.take() {
                    flush_pending_pointer_motion(&mut coalesced, pending);
                }
                coalesced.push(event);
            }
        }
    }

    if let Some(pending) = pending_motion {
        flush_pending_pointer_motion(&mut coalesced, pending);
    }

    coalesced
}

fn flush_pending_pointer_motion(
    events: &mut Vec<NativeHardwareInputEvent>,
    pending: PendingPointerMotion,
) {
    match pending {
        PendingPointerMotion::Sample(sample) => {
            events.push(NativeHardwareInputEvent::PointerMotion(sample));
        }
    }
}

#[derive(Debug)]
struct NativeInputDevice {
    file: fs::File,
    path: PathBuf,
}

#[derive(Debug, Default)]
struct NativeInputDevices {
    devices: Vec<NativeInputDevice>,
}

impl NativeInputDevices {
    fn open_readable() -> Self {
        let mut devices = Vec::new();
        let mut denied_paths = Vec::new();
        for path in input_event_paths(Path::new("/dev/input")) {
            match OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
                .open(&path)
            {
                Ok(file) => devices.push(NativeInputDevice { file, path }),
                Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                    denied_paths.push(path);
                }
                Err(_) => {}
            }
        }

        if !denied_paths.is_empty() {
            eprintln!(
                "native input: permission denied for {} keyboard/mouse device(s): {}",
                denied_paths.len(),
                denied_paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            eprintln!(
                "native input: add the user to the input group or run through a seat manager to grant raw input access"
            );
        }
        if devices.is_empty() {
            eprintln!(
                "native input: no readable /dev/input/event* devices; keyboard/mouse disabled"
            );
        } else {
            println!("native input: opened {} device(s)", devices.len());
        }
        Self { devices }
    }

    fn drain_events(&mut self) -> Vec<NativeHardwareInputEvent> {
        let mut events = Vec::new();
        for device in &mut self.devices {
            while let Some(event) = read_linux_input_event(device) {
                if let Some(event) = NativeHardwareInputEvent::from_linux_event(event) {
                    events.push(event);
                }
                if events.len() >= 256 {
                    return events;
                }
            }
        }
        events
    }
}

fn input_event_paths(root: &Path) -> Vec<PathBuf> {
    input_event_paths_with_udev(root, Path::new("/run/udev/data"))
}

fn input_event_paths_with_udev(root: &Path, udev_data_root: &Path) -> Vec<PathBuf> {
    let mut paths = fs::read_dir(root)
        .ok()
        .into_iter()
        .flat_map(|entries| entries.filter_map(Result::ok))
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("event"))
        })
        .filter(|path| input_event_is_keyboard_or_pointer(path, udev_data_root))
        .collect::<Vec<_>>();
    paths.sort_by_key(|path| input_event_number(path).unwrap_or(u32::MAX));
    paths
}

fn input_event_is_keyboard_or_pointer(path: &Path, udev_data_root: &Path) -> bool {
    let Some(event_number) = input_event_number(path) else {
        return false;
    };
    let Some(minor) = event_number.checked_add(64) else {
        return false;
    };
    let Ok(udev_data) = fs::read_to_string(udev_data_root.join(format!("c13:{minor}"))) else {
        return false;
    };
    udev_data.lines().any(|line| {
        matches!(
            line,
            "E:ID_INPUT_KEYBOARD=1" | "E:ID_INPUT_MOUSE=1" | "E:ID_INPUT_TOUCHPAD=1"
        )
    })
}

fn input_event_number(path: &Path) -> Option<u32> {
    path.file_name()?
        .to_str()?
        .strip_prefix("event")?
        .parse()
        .ok()
}

fn read_linux_input_event(device: &mut NativeInputDevice) -> Option<LinuxInputEvent> {
    let mut event = mem::MaybeUninit::<LinuxInputEvent>::uninit();
    let read = unsafe {
        libc::read(
            device.file.as_raw_fd(),
            event.as_mut_ptr().cast::<c_void>(),
            mem::size_of::<LinuxInputEvent>(),
        )
    };
    if read == mem::size_of::<LinuxInputEvent>() as isize {
        return Some(unsafe { event.assume_init() });
    }
    if read < 0 {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::WouldBlock {
            eprintln!(
                "native input: failed reading {}: {error}",
                device.path.display()
            );
        }
    }
    None
}

fn apply_native_input_effect(
    effect: NativeInputEffect,
    server: &mut OwnCompositorServer,
    perf: NativePerfLogger,
    resize_perf: &mut NativeResizePerfState,
    cursor_mode: NativeCursorRenderMode,
    app_gpu_policy: EffectiveCompositorAppGpuPolicy,
) -> NativeResult<NativeInputApplication> {
    let mut application = NativeInputApplication {
        redraw_requested: effect.requires_frame_repaint(cursor_mode),
        exit_requested: effect.exit_requested,
        launch: None,
    };
    application.redraw_requested |= apply_compositor_only_pointer_position(&effect, |x, y| {
        server.update_pointer_position_without_client_dispatch(x, y)
    });
    for event in effect.keyboard_events {
        server.send_keyboard_key(event.key, event.pressed);
    }
    if effect.pointer_motion.is_some() || effect.relative_motion.is_some() {
        server.send_pointer_motion_sample(CompositorPointerMotionSample {
            timestamp_usec: effect.pointer_motion_usec.unwrap_or(0),
            absolute: effect
                .pointer_motion
                .map(|(x, y)| CompositorOutputPosition { x, y }),
            relative: effect.relative_motion.map(Into::into),
        });
    }
    for event in effect.pointer_buttons {
        if event.pressed && event.button == u32::from(BTN_LEFT) {
            let dock_items = server.shell_dock_items();
            if let Some(surface_id) = dock_item_at(
                event.output_width,
                event.output_height,
                &dock_items,
                event.x.round().max(0.0) as i32,
                event.y.round().max(0.0) as i32,
            ) {
                application.redraw_requested |= server.activate_window(surface_id);
                continue;
            }
        }
        server.send_pointer_button(event.button, event.pressed);
    }
    if let Some((horizontal, vertical)) = effect.pointer_axis {
        server.send_pointer_axis(horizontal, vertical);
    }
    for action in effect.window_actions {
        application.redraw_requested |=
            apply_native_window_action(action, server, perf, resize_perf);
    }
    if let Some(command) = effect.launch_command {
        application.launch = launch_native_shell_command(
            server,
            command,
            app_gpu_policy,
            NativeLaunchSource::Spotlight,
        )?;
    }
    Ok(application)
}

fn apply_compositor_only_pointer_position(
    effect: &NativeInputEffect,
    update: impl FnOnce(f64, f64) -> bool,
) -> bool {
    if effect.pointer_motion.is_some() {
        return false;
    }
    let Some((x, y)) = effect.cursor_position else {
        return false;
    };
    update(f64::from(x), f64::from(y))
}

fn process_native_pointer_constraint_backend_requests(
    server: &mut OwnCompositorServer,
    backend: &mut NativePointerConstraintBackend,
    input_state: &mut NativeInputState,
    hardware_cursor: &mut Option<NativeHardwareCursor>,
    cursor_mode: NativeCursorRenderMode,
) -> NativeResult<bool> {
    let mut redraw_requested = false;
    loop {
        let requests = server.take_pointer_constraint_backend_requests();
        if requests.is_empty() {
            break;
        }
        for request in requests {
            let cursor_position = input_state.cursor_position_f64();
            native_pointer_debug_log(format!(
                "pointer.constraint native_request {:?} cursor=({},{})",
                request, cursor_position.x, cursor_position.y
            ));
            if let Some(id) = pointer_constraint_activation_request_id(&request)
                && !server.pointer_constraint_backend_activation_current(id)
            {
                native_pointer_debug_log(format!(
                    "pointer.constraint native_request dropped stale id={} generation={} rollback=not_needed",
                    id.constraint_id, id.generation
                ));
                continue;
            }
            let action = backend.handle_request(request, cursor_position);
            if let Some((id, reason)) = action.failed {
                native_pointer_debug_log(format!(
                    "pointer.constraint native_failed id={} generation={} reason={}",
                    id.constraint_id, id.generation, reason
                ));
                server.pointer_constraint_backend_failed(id, reason);
            }
            if let Some(constraint) = action.activated {
                native_pointer_debug_log(format!(
                    "pointer.constraint native_activated id={} generation={} mode={:?} anchor=({},{})",
                    constraint.id.constraint_id,
                    constraint.id.generation,
                    constraint.mode,
                    constraint.anchor.x,
                    constraint.anchor.y
                ));
                match constraint.mode {
                    PointerConstraintMode::Locked => {
                        input_state.set_pointer_locked_at(constraint.anchor)
                    }
                    PointerConstraintMode::Confined => {
                        if let Some(region) = constraint.region {
                            input_state.set_pointer_confined(region);
                        }
                    }
                    PointerConstraintMode::None => input_state.clear_pointer_constraint(),
                }
                server.pointer_constraint_backend_activated(constraint.id);
            }
            if let Some(restore_position) = action.restore_position {
                native_pointer_debug_log(format!(
                    "pointer.unlock native_restore output=({},{})",
                    restore_position.x, restore_position.y
                ));
                input_state.clear_pointer_constraint();
                let effect = input_state.restore_cursor_position(restore_position);
                redraw_requested |= effect.requires_frame_repaint(cursor_mode);
                if let Some((cursor_x, cursor_y)) = effect.cursor_position
                    && let Some(cursor) = hardware_cursor.as_mut()
                {
                    cursor.move_to(cursor_x, cursor_y)?;
                }
            }
            if let Some(cursor_position) = action.cursor_position {
                let effect = input_state.restore_cursor_position(cursor_position);
                redraw_requested |= effect.requires_frame_repaint(cursor_mode);
                if let Some((cursor_x, cursor_y)) = effect.cursor_position
                    && let Some(cursor) = hardware_cursor.as_mut()
                {
                    cursor.move_to(cursor_x, cursor_y)?;
                }
            }
            if let Some(id) = action.deactivated {
                native_pointer_debug_log(format!(
                    "pointer.constraint native_deactivated id={} generation={}",
                    id.constraint_id, id.generation
                ));
                server.pointer_constraint_backend_deactivated(id);
            }
            if let Some(visible) = action.cursor_visibility_changed {
                native_pointer_debug_log(format!("cursor visibility native visible={}", visible));
                let changed = input_state.set_cursor_visible(visible);
                if cursor_mode == NativeCursorRenderMode::Software && changed {
                    redraw_requested = true;
                }
                if let Some(cursor) = hardware_cursor.as_mut() {
                    if visible {
                        let (cursor_x, cursor_y) = input_state.cursor_position();
                        cursor
                            .enable()
                            .and_then(|()| cursor.move_to(cursor_x, cursor_y))?;
                    } else {
                        cursor.disable()?;
                    }
                }
            }
        }
    }
    input_state.pointer_constraint = backend.active_constraint_state();
    Ok(redraw_requested)
}

fn pointer_constraint_activation_request_id(
    request: &PointerConstraintBackendRequest,
) -> Option<PointerConstraintBackendId> {
    match request {
        PointerConstraintBackendRequest::ActivateLocked { id, .. }
        | PointerConstraintBackendRequest::ActivateConfined { id, .. } => Some(*id),
        _ => None,
    }
}

#[derive(Debug)]
struct NativeInputApplication {
    redraw_requested: bool,
    exit_requested: bool,
    launch: Option<NativeAppLaunchPerf>,
}

fn apply_native_window_action(
    action: NativeWindowAction,
    server: &mut OwnCompositorServer,
    perf: NativePerfLogger,
    resize_perf: &mut NativeResizePerfState,
) -> bool {
    let changed = match action {
        NativeWindowAction::BeginMove { x, y } => server.begin_window_move_at(x, y),
        NativeWindowAction::BeginResize { x, y } => server.begin_window_resize_at(x, y),
        NativeWindowAction::UpdateInteraction { x, y } => server.update_window_interaction(x, y),
        NativeWindowAction::EndInteraction => {
            let was_active = server.window_interaction_active();
            server.end_window_interaction();
            was_active
        }
        NativeWindowAction::Minimize => server.minimize_focused_window(),
        NativeWindowAction::RestoreMinimized => server.restore_next_minimized_window(),
        NativeWindowAction::ToggleMaximize => server.toggle_maximize_focused_window(),
        NativeWindowAction::ToggleFullscreen => server.toggle_fullscreen_focused_window(),
    };
    resize_perf.observe_action(action, changed, perf);
    changed
}

fn launch_native_shell_command(
    server: &OwnCompositorServer,
    command: Vec<String>,
    app_gpu_policy: EffectiveCompositorAppGpuPolicy,
    source: NativeLaunchSource,
) -> NativeResult<Option<NativeAppLaunchPerf>> {
    let Some(request) = native_launch_request(command, app_gpu_policy, source) else {
        return Ok(None);
    };
    let socket_name = server.socket_name().to_string();
    let spawn_start = Instant::now();
    let spawn_result =
        spawn_compositor_app_with_policy(&socket_name, &request.argv, request.gpu_policy);
    match spawn_result {
        Ok(Some(pid)) => {
            let spawn_us = elapsed_micros(spawn_start);
            println!(
                "spawned `{}` from native {} on Oblivion Wayland socket `{socket_name}` as pid {pid} (gpu policy {})",
                request.program,
                request.source.as_str(),
                request.gpu_policy.as_str()
            );
            Ok(Some(NativeAppLaunchPerf {
                program: request.program,
                command: request.command,
                pid,
                spawn_us,
                started_at: spawn_start,
                gpu_policy: request.gpu_policy,
                source: request.source,
            }))
        }
        Ok(None) => Ok(None),
        Err(error) => Err(io::Error::other(format!(
            "failed to spawn `{}` from native {} with app policy {}: {error}",
            request.program,
            request.source.as_str(),
            request.gpu_policy.as_str()
        ))
        .into()),
    }
}

fn native_launch_request(
    command: Vec<String>,
    gpu_policy: EffectiveCompositorAppGpuPolicy,
    source: NativeLaunchSource,
) -> Option<NativeLaunchRequest> {
    let program = command.first()?.clone();
    let command_label = command
        .iter()
        .map(|arg| shell_quote(arg))
        .collect::<Vec<_>>()
        .join(" ");
    Some(NativeLaunchRequest {
        argv: command,
        program,
        command: command_label,
        gpu_policy,
        source,
    })
}

fn log_native_app_spawn(perf: NativePerfLogger, launch: &NativeAppLaunchPerf) {
    perf.log("app.spawn", || {
        vec![
            NativePerfField::str("program", launch.program.clone()),
            NativePerfField::str("command", launch.command.clone()),
            NativePerfField::str("source", launch.source.as_str()),
            NativePerfField::u64("pid", u64::from(launch.pid)),
            NativePerfField::u64("spawn_us", launch.spawn_us),
            NativePerfField::str("app_policy", launch.gpu_policy.as_str()),
        ]
    });
}

fn first_dri_node(prefix: &str) -> Option<PathBuf> {
    let mut entries = fs::read_dir("/dev/dri")
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(prefix))
        })
        .collect::<Vec<_>>();
    entries.sort();
    entries.into_iter().next()
}

fn query_kms_resources(kms_device: Option<&Path>) -> Result<Option<KmsResources>, String> {
    let Some(kms_device) = kms_device else {
        return Ok(None);
    };
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(kms_device)
        .map_err(|error| format!("failed to open {}: {error}", kms_device.display()))?;

    let mut crtcs = Vec::new();
    let mut connector_ids = Vec::new();
    let mut encoders = Vec::new();
    drm_ffi::mode::get_resources(
        file.as_fd(),
        None,
        Some(&mut crtcs),
        Some(&mut connector_ids),
        Some(&mut encoders),
    )
    .map_err(|error| format!("DRM_IOCTL_MODE_GETRESOURCES failed: {error}"))?;

    let mut connected_connector_count = 0usize;
    let mut first_connected_connector_id = None;
    let mut first_connected_mode = None;
    for connector_id in &connector_ids {
        let mut modes = Vec::new();
        let connector = drm_ffi::mode::get_connector(
            file.as_fd(),
            *connector_id,
            None,
            None,
            Some(&mut modes),
            None,
            true,
        )
        .map_err(|error| {
            format!("DRM_IOCTL_MODE_GETCONNECTOR failed for connector {connector_id}: {error}")
        })?;
        if connector.connection == 1 {
            connected_connector_count += 1;
            first_connected_connector_id.get_or_insert(*connector_id);
            if first_connected_mode.is_none() {
                first_connected_mode = modes.first().map(drm_mode_name);
            }
        }
    }

    Ok(Some(KmsResources {
        crtc_count: crtcs.len(),
        connector_count: connector_ids.len(),
        encoder_count: encoders.len(),
        connected_connector_count,
        first_connected_connector_id,
        first_connected_mode,
    }))
}

fn select_kms_target(
    file: &fs::File,
    mode_preference: NativeModePreference,
) -> io::Result<KmsTarget> {
    let mut crtcs = Vec::new();
    let mut connector_ids = Vec::new();
    drm_ffi::mode::get_resources(
        file.as_fd(),
        None,
        Some(&mut crtcs),
        Some(&mut connector_ids),
        None,
    )?;

    for connector_id in connector_ids {
        let mut modes = Vec::new();
        let mut encoder_ids = Vec::new();
        let connector = drm_ffi::mode::get_connector(
            file.as_fd(),
            connector_id,
            None,
            None,
            Some(&mut modes),
            Some(&mut encoder_ids),
            true,
        )?;
        if connector.connection != 1 {
            continue;
        }
        let Some(mode) = select_kms_mode(&modes, mode_preference) else {
            continue;
        };

        let current_encoder = (connector.encoder_id != 0).then_some(connector.encoder_id);
        for encoder_id in current_encoder.into_iter().chain(encoder_ids.into_iter()) {
            let encoder = drm_ffi::mode::get_encoder(file.as_fd(), encoder_id)?;
            if let Some(crtc_id) = select_crtc_id(&crtcs, &encoder) {
                return Ok(KmsTarget {
                    connector_id,
                    crtc_id,
                    mode,
                    width: u32::from(mode.hdisplay),
                    height: u32::from(mode.vdisplay),
                });
            }
        }
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "no connected KMS connector with a usable CRTC was found",
    ))
}

fn select_kms_mode(
    modes: &[drm_sys::drm_mode_modeinfo],
    preference: NativeModePreference,
) -> Option<drm_sys::drm_mode_modeinfo> {
    match preference {
        NativeModePreference::Preferred => modes.first().copied(),
        NativeModePreference::Auto | NativeModePreference::HighResolution => modes
            .iter()
            .copied()
            .max_by_key(|mode| (mode_area(mode), mode.vrefresh)),
        NativeModePreference::HighRefresh => modes
            .iter()
            .copied()
            .max_by_key(|mode| (mode.vrefresh, mode_area(mode))),
        NativeModePreference::Exact {
            width,
            height,
            refresh_hz,
        } => select_exact_kms_mode(modes, width, height, refresh_hz)
            .or_else(|| select_kms_mode(modes, NativeModePreference::Auto)),
    }
}

fn select_exact_kms_mode(
    modes: &[drm_sys::drm_mode_modeinfo],
    width: u32,
    height: u32,
    refresh_hz: Option<u32>,
) -> Option<drm_sys::drm_mode_modeinfo> {
    let matching_modes = modes
        .iter()
        .copied()
        .filter(|mode| u32::from(mode.hdisplay) == width && u32::from(mode.vdisplay) == height);
    if let Some(refresh_hz) = refresh_hz {
        return matching_modes.min_by_key(|mode| {
            (
                mode.vrefresh.abs_diff(refresh_hz),
                u32::MAX.saturating_sub(mode.vrefresh),
            )
        });
    }
    matching_modes.max_by_key(|mode| mode.vrefresh)
}

fn mode_area(mode: &drm_sys::drm_mode_modeinfo) -> u64 {
    u64::from(mode.hdisplay) * u64::from(mode.vdisplay)
}

fn select_crtc_id(crtcs: &[u32], encoder: &drm_sys::drm_mode_get_encoder) -> Option<u32> {
    if encoder.crtc_id != 0 && crtcs.contains(&encoder.crtc_id) {
        return Some(encoder.crtc_id);
    }

    crtcs
        .iter()
        .enumerate()
        .find(|(index, _)| encoder.possible_crtcs & (1 << index) != 0)
        .map(|(_, crtc_id)| *crtc_id)
}

fn drm_mode_name(mode: &drm_sys::drm_mode_modeinfo) -> String {
    let bytes = mode
        .name
        .iter()
        .take_while(|byte| **byte != 0)
        .map(|byte| *byte as u8)
        .collect::<Vec<_>>();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeScanoutKind {
    NativeEglGbm,
    GbmCpuWritePageFlip,
    DumbFramebuffer,
    Unavailable,
}

impl NativeScanoutKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::NativeEglGbm => "native EGL/GLES GBM pageflip",
            Self::GbmCpuWritePageFlip => "GBM CPU-write pageflip",
            Self::DumbFramebuffer => "KMS dumb framebuffer",
            Self::Unavailable => "unavailable",
        }
    }

    const fn metric_name(self) -> &'static str {
        match self {
            Self::NativeEglGbm => "native-egl-gbm",
            Self::GbmCpuWritePageFlip => "gbm-cpu-write-pageflip",
            Self::DumbFramebuffer => "dumb-framebuffer",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NativePaintStats {
    backend: NativeScanoutKind,
    scanout_format: Option<u32>,
    width: u32,
    height: u32,
    bytes: usize,
    copy_bytes: usize,
    write_bytes: usize,
    gpu_draw_us: u64,
    egl_swap_us: u64,
    shm_upload_bytes: usize,
    dmabuf_imports: usize,
    dmabuf_reuses: usize,
    dmabuf_import_failures: usize,
    scene_rebuild: DesktopSceneRebuildKind,
    frame_copy: DesktopFrameCopyKind,
    total_us: u64,
    render_us: u64,
    copy_us: u64,
    write_us: u64,
}

impl NativePaintStats {
    fn fields(self) -> Vec<NativePerfField> {
        let mut fields = vec![
            NativePerfField::str("backend", self.backend.metric_name()),
            NativePerfField::str("scanout", self.backend.as_str()),
            NativePerfField::u64("width", u64::from(self.width)),
            NativePerfField::u64("height", u64::from(self.height)),
            NativePerfField::usize("bytes", self.bytes),
            NativePerfField::usize("copy_bytes", self.copy_bytes),
            NativePerfField::usize("full_frame_cpu_copy_bytes", self.copy_bytes),
            NativePerfField::usize("write_bytes", self.write_bytes),
            NativePerfField::u64("gpu_draw_us", self.gpu_draw_us),
            NativePerfField::u64("egl_swap_us", self.egl_swap_us),
            NativePerfField::usize("shm_upload_bytes", self.shm_upload_bytes),
            NativePerfField::usize("dmabuf_imports", self.dmabuf_imports),
            NativePerfField::usize("dmabuf_reuses", self.dmabuf_reuses),
            NativePerfField::usize("dmabuf_import_failures", self.dmabuf_import_failures),
            NativePerfField::str("scene_rebuild", self.scene_rebuild.as_str()),
            NativePerfField::str("frame_copy", self.frame_copy.as_str()),
            NativePerfField::u64("paint_us", self.total_us),
            NativePerfField::u64("render_us", self.render_us),
            NativePerfField::u64("copy_us", self.copy_us),
            NativePerfField::u64("write_us", self.write_us),
        ];
        if let Some(scanout_format) = self.scanout_format {
            fields.push(NativePerfField::str(
                "scanout_format",
                native_visual_label(scanout_format),
            ));
        }
        fields
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NativeDamageSummary {
    kind: NativeDamageKind,
    rects: usize,
    pixels: u64,
}

impl NativeDamageSummary {
    fn fields(self) -> [NativePerfField; 3] {
        [
            NativePerfField::str("damage_kind", self.kind.as_str()),
            NativePerfField::usize("damage_rects", self.rects),
            NativePerfField::u64("damaged_pixels", self.pixels),
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NativeOutputDamage {
    kind: NativeDamageKind,
    rects: Vec<NativeDamageRect>,
    pixels: u64,
}

impl NativeOutputDamage {
    const fn empty() -> Self {
        Self {
            kind: NativeDamageKind::Empty,
            rects: Vec::new(),
            pixels: 0,
        }
    }

    fn full_output(width: u32, height: u32) -> Self {
        let pixels = u64::from(width).saturating_mul(u64::from(height));
        Self {
            kind: NativeDamageKind::FullOutput,
            rects: if width > 0 && height > 0 {
                vec![NativeDamageRect {
                    x: 0,
                    y: 0,
                    width,
                    height,
                }]
            } else {
                Vec::new()
            },
            pixels,
        }
    }

    fn surface_damage(rects: Vec<NativeDamageRect>) -> Self {
        let rects = coalesce_native_damage_rects(rects);
        if rects.is_empty() {
            return Self::empty();
        }
        let pixels = rects
            .iter()
            .fold(0u64, |pixels, rect| pixels.saturating_add(rect.pixels()));
        Self {
            kind: NativeDamageKind::SurfaceDamage,
            rects,
            pixels,
        }
    }

    fn is_empty(&self) -> bool {
        self.kind == NativeDamageKind::Empty || self.rects.is_empty() || self.pixels == 0
    }

    fn summary(&self) -> NativeDamageSummary {
        NativeDamageSummary {
            kind: self.kind,
            rects: self.rects.len(),
            pixels: self.pixels,
        }
    }

    fn fields(&self) -> [NativePerfField; 3] {
        self.summary().fields()
    }

    fn frame_copy_damage(&self) -> NativeFrameCopyDamage<'_> {
        match self.kind {
            NativeDamageKind::FullOutput => NativeFrameCopyDamage::Full,
            NativeDamageKind::Empty | NativeDamageKind::SurfaceDamage => {
                NativeFrameCopyDamage::Rects(&self.rects)
            }
        }
    }

    fn frame_copy_damage_for_scene(
        &self,
        scene_rebuild: DesktopSceneRebuildKind,
    ) -> NativeFrameCopyDamage<'_> {
        if scene_rebuild == DesktopSceneRebuildKind::Full {
            NativeFrameCopyDamage::Full
        } else {
            self.frame_copy_damage()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeDamageKind {
    Empty,
    SurfaceDamage,
    FullOutput,
}

impl NativeDamageKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::SurfaceDamage => "surface_damage",
            Self::FullOutput => "full",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NativeDamageRect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

impl NativeDamageRect {
    fn from_render_element_bounds(element: &RenderSceneElement) -> Option<Self> {
        let target = element.target();
        (target.width() > 0 && target.height() > 0).then_some(Self {
            x: target.x(),
            y: target.y(),
            width: target.width(),
            height: target.height(),
        })
    }

    #[cfg(test)]
    fn from_surface_damage(
        surface: &RenderableSurface,
        origin: (i32, i32),
        rect: oblivion_one::compositor::SurfaceDamageRect,
    ) -> Option<Self> {
        if surface.width == 0 || surface.height == 0 {
            return None;
        }

        let buffer_size = surface.buffer_size();
        let left = scale_damage_floor(rect.x, buffer_size.width, surface.width)?;
        let top = scale_damage_floor(rect.y, buffer_size.height, surface.height)?;
        let right = scale_damage_ceil(
            rect.x.saturating_add(rect.width),
            buffer_size.width,
            surface.width,
        )?;
        let bottom = scale_damage_ceil(
            rect.y.saturating_add(rect.height),
            buffer_size.height,
            surface.height,
        )?;
        if right <= left || bottom <= top {
            return None;
        }

        Some(Self {
            x: i32_saturating_add_u32(origin.0, left),
            y: i32_saturating_add_u32(origin.1, top),
            width: right - left,
            height: bottom - top,
        })
    }

    fn from_render_element_damage(
        element: &RenderSceneElement,
        rect: oblivion_one::compositor::SurfaceDamageRect,
    ) -> Option<Self> {
        let target = element.target();
        if target.width() == 0 || target.height() == 0 {
            return None;
        }

        let buffer_size = element.buffer_size();
        let left = scale_damage_floor(rect.x, buffer_size.width, target.width())?;
        let top = scale_damage_floor(rect.y, buffer_size.height, target.height())?;
        let right = scale_damage_ceil(
            rect.x.saturating_add(rect.width),
            buffer_size.width,
            target.width(),
        )?;
        let bottom = scale_damage_ceil(
            rect.y.saturating_add(rect.height),
            buffer_size.height,
            target.height(),
        )?;
        if right <= left || bottom <= top {
            return None;
        }

        Some(Self {
            x: i32_saturating_add_u32(target.x(), left),
            y: i32_saturating_add_u32(target.y(), top),
            width: right - left,
            height: bottom - top,
        })
    }

    fn clipped_to_output(self, output_width: u32, output_height: u32) -> Option<Self> {
        let left = i64::from(self.x).clamp(0, i64::from(output_width));
        let top = i64::from(self.y).clamp(0, i64::from(output_height));
        let right = i64::from(self.x)
            .saturating_add(i64::from(self.width))
            .clamp(0, i64::from(output_width));
        let bottom = i64::from(self.y)
            .saturating_add(i64::from(self.height))
            .clamp(0, i64::from(output_height));
        (right > left && bottom > top).then_some(Self {
            x: left as i32,
            y: top as i32,
            width: (right - left) as u32,
            height: (bottom - top) as u32,
        })
    }

    const fn pixels(self) -> u64 {
        (self.width as u64).saturating_mul(self.height as u64)
    }

    fn left(self) -> i64 {
        i64::from(self.x)
    }

    fn top(self) -> i64 {
        i64::from(self.y)
    }

    fn right(self) -> i64 {
        self.left().saturating_add(i64::from(self.width))
    }

    fn bottom(self) -> i64 {
        self.top().saturating_add(i64::from(self.height))
    }

    fn union(self, other: Self) -> Self {
        let left = self.x.min(other.x);
        let top = self.y.min(other.y);
        let right = self.right().max(other.right());
        let bottom = self.bottom().max(other.bottom());
        Self {
            x: left,
            y: top,
            width: u32::try_from(right.saturating_sub(i64::from(left))).unwrap_or(u32::MAX),
            height: u32::try_from(bottom.saturating_sub(i64::from(top))).unwrap_or(u32::MAX),
        }
    }
}

fn coalesce_native_damage_rects(rects: Vec<NativeDamageRect>) -> Vec<NativeDamageRect> {
    let mut coalesced = Vec::<NativeDamageRect>::new();
    'next_rect: for rect in rects {
        if rect.width == 0 || rect.height == 0 {
            continue;
        }
        let mut pending = rect;
        let mut index = 0;
        while index < coalesced.len() {
            let existing = coalesced[index];
            let union = existing.union(pending);
            let separate_pixels = existing.pixels().saturating_add(pending.pixels());
            if union.pixels() <= separate_pixels {
                pending = union;
                coalesced.swap_remove(index);
                index = 0;
                continue;
            }
            if existing == pending {
                continue 'next_rect;
            }
            index += 1;
        }
        coalesced.push(pending);
    }
    coalesced
}

#[derive(Debug, Clone)]
struct NativeDamageAccumulator {
    output_width: u32,
    output_height: u32,
    rects: Vec<NativeDamageRect>,
}

impl NativeDamageAccumulator {
    const fn for_output(output_width: u32, output_height: u32) -> Self {
        Self {
            output_width,
            output_height,
            rects: Vec::new(),
        }
    }

    fn from_surfaces(
        output_width: u32,
        output_height: u32,
        surfaces: &[RenderableSurface],
    ) -> Self {
        let elements = render_scene_elements_for_surfaces(surfaces, 1.0);
        Self::from_render_elements(output_width, output_height, &elements)
    }

    fn from_render_elements(
        output_width: u32,
        output_height: u32,
        elements: &[RenderSceneElement],
    ) -> Self {
        let mut accumulator = Self::for_output(output_width, output_height);
        for element in elements {
            accumulator.add_render_element(element);
        }
        accumulator
    }

    fn from_surface_bounds_changes(
        output_width: u32,
        output_height: u32,
        previous_surfaces: &[RenderableSurface],
        current_surfaces: &[RenderableSurface],
    ) -> Self {
        let previous_elements = render_scene_elements_for_surfaces(previous_surfaces, 1.0);
        let current_elements = render_scene_elements_for_surfaces(current_surfaces, 1.0);
        Self::from_render_element_bounds_changes(
            output_width,
            output_height,
            &previous_elements,
            &current_elements,
        )
    }

    fn from_render_element_bounds_changes(
        output_width: u32,
        output_height: u32,
        previous_elements: &[RenderSceneElement],
        current_elements: &[RenderSceneElement],
    ) -> Self {
        let previous_rects =
            native_element_bounds_by_id(output_width, output_height, previous_elements);
        let current_rects =
            native_element_bounds_by_id(output_width, output_height, current_elements);

        let mut accumulator = Self::for_output(output_width, output_height);
        for (surface_id, previous_rect) in &previous_rects {
            let current_rect = current_rects.get(surface_id).copied();
            if current_rect != Some(*previous_rect) {
                if let Some(current_rect) = current_rect {
                    accumulator.rects.push(*previous_rect);
                    accumulator.rects.push(current_rect);
                } else {
                    accumulator.rects.push(*previous_rect);
                }
            }
        }
        for (surface_id, current_rect) in current_rects {
            if !previous_rects.contains_key(&surface_id) {
                accumulator.rects.push(current_rect);
            }
        }
        accumulator
    }

    fn extend(&mut self, other: Self) {
        debug_assert_eq!(self.output_width, other.output_width);
        debug_assert_eq!(self.output_height, other.output_height);
        self.rects.extend(other.rects);
    }

    #[cfg(test)]
    fn add_surface(&mut self, surface: &RenderableSurface, origin: (i32, i32)) {
        let buffer_size = surface.buffer_size();
        for rect in surface
            .damage
            .clipped_rects(buffer_size.width, buffer_size.height)
        {
            let Some(rect) = NativeDamageRect::from_surface_damage(surface, origin, rect)
                .and_then(|rect| rect.clipped_to_output(self.output_width, self.output_height))
            else {
                continue;
            };
            self.rects.push(rect);
        }
    }

    fn add_render_element(&mut self, element: &RenderSceneElement) {
        let buffer_size = element.buffer_size();
        for rect in element
            .damage()
            .clipped_rects(buffer_size.width, buffer_size.height)
        {
            let Some(rect) = NativeDamageRect::from_render_element_damage(element, rect)
                .and_then(|rect| rect.clipped_to_output(self.output_width, self.output_height))
            else {
                continue;
            };
            self.rects.push(rect);
        }
    }

    #[cfg(test)]
    fn rects(&self) -> &[NativeDamageRect] {
        &self.rects
    }

    #[cfg(test)]
    fn summary(&self) -> NativeDamageSummary {
        if self.rects.is_empty() {
            return NativeDamageSummary {
                kind: NativeDamageKind::Empty,
                rects: 0,
                pixels: 0,
            };
        }

        NativeDamageSummary {
            kind: NativeDamageKind::SurfaceDamage,
            rects: self.rects.len(),
            pixels: self
                .rects
                .iter()
                .fold(0u64, |pixels, rect| pixels.saturating_add(rect.pixels())),
        }
    }

    fn into_output_damage(self) -> NativeOutputDamage {
        NativeOutputDamage::surface_damage(self.rects)
    }
}

fn native_element_bounds_by_id(
    output_width: u32,
    output_height: u32,
    elements: &[RenderSceneElement],
) -> HashMap<RenderSceneElementId, NativeDamageRect> {
    elements
        .iter()
        .filter_map(|element| {
            let rect = NativeDamageRect::from_render_element_bounds(element)?
                .clipped_to_output(output_width, output_height)?;
            Some((element.id(), rect))
        })
        .collect()
}

fn native_output_damage_for_repaint(
    width: u32,
    height: u32,
    previous_surfaces: &[RenderableSurface],
    surfaces: &[RenderableSurface],
    cause: RenderGenerationCause,
    render_generation_changed: bool,
) -> NativeOutputDamage {
    if render_generation_changed && cause.uses_surface_damage() {
        let mut damage = NativeDamageAccumulator::from_surfaces(width, height, surfaces);
        damage.extend(NativeDamageAccumulator::from_surface_bounds_changes(
            width,
            height,
            previous_surfaces,
            surfaces,
        ));
        damage.into_output_damage()
    } else if render_generation_changed
        && matches!(
            cause,
            RenderGenerationCause::WindowMove
                | RenderGenerationCause::WindowResize
                | RenderGenerationCause::SurfacePlacement
        )
    {
        let damage = NativeDamageAccumulator::from_surface_bounds_changes(
            width,
            height,
            previous_surfaces,
            surfaces,
        )
        .into_output_damage();
        if damage.rects.is_empty() {
            NativeOutputDamage::full_output(width, height)
        } else {
            damage
        }
    } else {
        NativeOutputDamage::full_output(width, height)
    }
}

fn native_repaint_cause_label(
    render_generation_cause: RenderGenerationCause,
    render_generation_changed: bool,
    accepted_clients: usize,
    pending_frame_work: bool,
    redraw_requested: bool,
) -> &'static str {
    if render_generation_changed {
        return render_generation_cause.as_str();
    }
    if redraw_requested {
        return "redraw_requested";
    }
    if pending_frame_work {
        return "pending_frame_work";
    }
    if accepted_clients > 0 {
        return "accepted_client";
    }
    "unknown"
}

fn scale_damage_floor(value: u32, from_extent: u32, to_extent: u32) -> Option<u32> {
    if from_extent == 0 {
        return None;
    }
    let scaled = u64::from(value).saturating_mul(u64::from(to_extent)) / u64::from(from_extent);
    Some(scaled.min(u64::from(u32::MAX)) as u32)
}

fn scale_damage_ceil(value: u32, from_extent: u32, to_extent: u32) -> Option<u32> {
    if from_extent == 0 {
        return None;
    }
    let numerator = u64::from(value).saturating_mul(u64::from(to_extent));
    let scaled =
        numerator.saturating_add(u64::from(from_extent).saturating_sub(1)) / u64::from(from_extent);
    Some(scaled.min(u64::from(u32::MAX)) as u32)
}

fn i32_saturating_add_u32(value: i32, addend: u32) -> i32 {
    i64::from(value)
        .saturating_add(i64::from(addend))
        .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

const NATIVE_HARDWARE_CURSOR_SIZE: u32 = 64;

struct NativeHardwareCursor {
    bo: gbm::BufferObject<()>,
    _device: gbm::Device<OwnedFd>,
    fd: RawFd,
    crtc_id: u32,
    width: u32,
    height: u32,
    active: bool,
}

impl NativeHardwareCursor {
    fn create(kms: &fs::File, crtc_id: u32) -> io::Result<Self> {
        let gbm_fd = duplicate_fd_cloexec(kms.as_raw_fd()).map_err(io::Error::from_raw_os_error)?;
        let device = gbm::Device::new(gbm_fd)?;
        let usage = gbm::BufferObjectFlags::CURSOR | gbm::BufferObjectFlags::WRITE;
        if !device.is_format_supported(gbm::Format::Argb8888, usage) {
            return Err(io::Error::other(
                "GBM device does not support writable ARGB8888 cursor buffers",
            ));
        }

        let mut bo = device.create_buffer_object(
            NATIVE_HARDWARE_CURSOR_SIZE,
            NATIVE_HARDWARE_CURSOR_SIZE,
            gbm::Format::Argb8888,
            usage,
        )?;
        let (texture_width, texture_height) = cursor_texture_size();
        let cursor_bytes = native_cursor_argb_bytes(
            &cursor_texture_pixels(),
            texture_width,
            texture_height,
            bo.width(),
            bo.height(),
            bo.stride(),
        )?;
        bo.write(&cursor_bytes)?;

        Ok(Self {
            fd: kms.as_raw_fd(),
            crtc_id,
            width: bo.width(),
            height: bo.height(),
            bo,
            _device: device,
            active: false,
        })
    }

    fn enable(&mut self) -> io::Result<()> {
        let fd = unsafe { BorrowedFd::borrow_raw(self.fd) };
        #[allow(deprecated)]
        drm_ffi::mode::set_cursor(fd, self.crtc_id, self.handle(), self.width, self.height)?;
        self.active = true;
        Ok(())
    }

    fn move_to(&mut self, x: i32, y: i32) -> io::Result<()> {
        let fd = unsafe { BorrowedFd::borrow_raw(self.fd) };
        #[allow(deprecated)]
        drm_ffi::mode::move_cursor(fd, self.crtc_id, x, y)?;
        Ok(())
    }

    fn disable(&mut self) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }
        let fd = unsafe { BorrowedFd::borrow_raw(self.fd) };
        #[allow(deprecated)]
        drm_ffi::mode::set_cursor(fd, self.crtc_id, 0, 0, 0)?;
        self.active = false;
        Ok(())
    }

    fn handle(&self) -> u32 {
        unsafe { self.bo.handle().u32_ }
    }
}

impl Drop for NativeHardwareCursor {
    fn drop(&mut self) {
        let _ = self.disable();
    }
}

fn native_cursor_argb_bytes(
    pixels: &[u32],
    source_width: u32,
    source_height: u32,
    target_width: u32,
    target_height: u32,
    pitch: u32,
) -> io::Result<Vec<u8>> {
    if source_width > target_width || source_height > target_height {
        return Err(io::Error::other(
            "native cursor texture exceeds target buffer",
        ));
    }
    let source_width = usize::try_from(source_width)
        .map_err(|_| io::Error::other("native cursor source width overflow"))?;
    let source_height = usize::try_from(source_height)
        .map_err(|_| io::Error::other("native cursor source height overflow"))?;
    let target_width = usize::try_from(target_width)
        .map_err(|_| io::Error::other("native cursor target width overflow"))?;
    let target_height = usize::try_from(target_height)
        .map_err(|_| io::Error::other("native cursor target height overflow"))?;
    let pitch =
        usize::try_from(pitch).map_err(|_| io::Error::other("invalid native cursor pitch"))?;
    let row_bytes = source_width
        .checked_mul(mem::size_of::<u32>())
        .ok_or_else(|| io::Error::other("native cursor source row overflow"))?;
    let min_pitch = target_width
        .checked_mul(mem::size_of::<u32>())
        .ok_or_else(|| io::Error::other("native cursor target row overflow"))?;
    if pitch < min_pitch {
        return Err(io::Error::other("native cursor pitch is too small"));
    }
    let pixel_count = source_width
        .checked_mul(source_height)
        .ok_or_else(|| io::Error::other("native cursor source overflow"))?;
    if pixels.len() < pixel_count {
        return Err(io::Error::other("native cursor source is too small"));
    }
    let byte_len = pitch
        .checked_mul(target_height)
        .ok_or_else(|| io::Error::other("native cursor target overflow"))?;
    let source_bytes_len = pixel_count
        .checked_mul(mem::size_of::<u32>())
        .ok_or_else(|| io::Error::other("native cursor source byte overflow"))?;
    let source_bytes =
        unsafe { slice::from_raw_parts(pixels.as_ptr().cast::<u8>(), source_bytes_len) };
    let mut bytes = vec![0; byte_len];
    for y in 0..source_height {
        let source_start = y
            .checked_mul(row_bytes)
            .ok_or_else(|| io::Error::other("native cursor source offset overflow"))?;
        let target_start = y
            .checked_mul(pitch)
            .ok_or_else(|| io::Error::other("native cursor target offset overflow"))?;
        bytes[target_start..target_start + row_bytes]
            .copy_from_slice(&source_bytes[source_start..source_start + row_bytes]);
    }
    Ok(bytes)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeScanoutPreference {
    Auto,
    NativeEglGbm,
    GbmCpuWritePageFlip,
    DumbFramebuffer,
}

impl NativeScanoutPreference {
    fn from_env() -> Self {
        match std::env::var("OBLIVION_ONE_SCANOUT_BACKEND") {
            Ok(value) if Self::is_known_value(&value) => Self::parse(&value),
            Ok(value) => {
                eprintln!(
                    "native scanout: unknown OBLIVION_ONE_SCANOUT_BACKEND={value:?}; using auto"
                );
                Self::Auto
            }
            Err(_) => Self::Auto,
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "auto" => Self::Auto,
            "gpu" | "native" | "native-gpu" | "native-egl-gbm" | "egl-gbm" | "gles-gbm"
            | "egl-gles-gbm" => Self::NativeEglGbm,
            "gbm-cpu-write"
            | "gbm-cpu-write-pageflip"
            | "cpu-gbm-write"
            | "cpu-gbm-pageflip"
            | "cpu"
            | "cpu-gbm"
            | "gbm"
            | "egl"
            | "pageflip"
            | "gbm-egl"
            | "gbm-egl-pageflip" => Self::GbmCpuWritePageFlip,
            "dumb" | "framebuffer" | "legacy" => Self::DumbFramebuffer,
            _ => Self::Auto,
        }
    }

    fn is_known_value(value: &str) -> bool {
        matches!(
            value,
            "auto"
                | "gpu"
                | "native"
                | "native-gpu"
                | "native-egl-gbm"
                | "egl-gbm"
                | "gles-gbm"
                | "egl-gles-gbm"
                | "gbm-cpu-write"
                | "gbm-cpu-write-pageflip"
                | "cpu-gbm-write"
                | "cpu-gbm-pageflip"
                | "cpu"
                | "cpu-gbm"
                | "gbm"
                | "egl"
                | "pageflip"
                | "gbm-egl"
                | "gbm-egl-pageflip"
                | "dumb"
                | "framebuffer"
                | "legacy"
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NativeScanoutChoice {
    preference: NativeScanoutPreference,
    gbm_available: bool,
    egl_available: bool,
    page_flip_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NativeScanoutPlan {
    primary: NativeScanoutKind,
    fallbacks: Vec<NativeScanoutKind>,
}

impl NativeScanoutPlan {
    fn choose(choice: NativeScanoutChoice) -> Self {
        match choice.preference {
            NativeScanoutPreference::NativeEglGbm
                if choice.gbm_available && choice.egl_available && choice.page_flip_available =>
            {
                Self {
                    primary: NativeScanoutKind::NativeEglGbm,
                    fallbacks: Vec::new(),
                }
            }
            NativeScanoutPreference::NativeEglGbm => Self::unavailable(),
            NativeScanoutPreference::GbmCpuWritePageFlip
                if choice.gbm_available && choice.page_flip_available =>
            {
                Self {
                    primary: NativeScanoutKind::GbmCpuWritePageFlip,
                    fallbacks: Vec::new(),
                }
            }
            NativeScanoutPreference::GbmCpuWritePageFlip => Self::unavailable(),
            NativeScanoutPreference::DumbFramebuffer => Self {
                primary: NativeScanoutKind::DumbFramebuffer,
                fallbacks: Vec::new(),
            },
            NativeScanoutPreference::Auto
                if choice.gbm_available && choice.egl_available && choice.page_flip_available =>
            {
                Self {
                    primary: NativeScanoutKind::NativeEglGbm,
                    fallbacks: vec![
                        NativeScanoutKind::GbmCpuWritePageFlip,
                        NativeScanoutKind::DumbFramebuffer,
                    ],
                }
            }
            NativeScanoutPreference::Auto if choice.gbm_available && choice.page_flip_available => {
                Self {
                    primary: NativeScanoutKind::GbmCpuWritePageFlip,
                    fallbacks: vec![NativeScanoutKind::DumbFramebuffer],
                }
            }
            NativeScanoutPreference::Auto => Self {
                primary: NativeScanoutKind::DumbFramebuffer,
                fallbacks: Vec::new(),
            },
        }
    }

    fn unavailable() -> Self {
        Self {
            primary: NativeScanoutKind::Unavailable,
            fallbacks: Vec::new(),
        }
    }

    fn candidates(&self) -> impl Iterator<Item = NativeScanoutKind> + '_ {
        std::iter::once(self.primary).chain(self.fallbacks.iter().copied())
    }

    fn after_failed(&self, failed: NativeScanoutKind) -> Self {
        let mut remaining = self
            .candidates()
            .skip_while(|candidate| *candidate != failed)
            .skip(1)
            .collect::<Vec<_>>();
        if remaining.is_empty() {
            return Self::unavailable();
        }
        let primary = remaining.remove(0);
        Self {
            primary,
            fallbacks: remaining,
        }
    }
}

enum NativeScanoutBackend {
    NativeEglGbm(Box<NativeEglGbmScanout>),
    Gbm(NativeGbmScanout),
    Dumb(DumbFramebuffer),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativePresentResult {
    Noop,
    AsyncSubmitted { token: u64 },
    Immediate,
}

#[derive(Debug, Default)]
struct NativePageFlipDrain {
    completion: Option<DrmPresentationEvent>,
    mismatched_events: u64,
    stale_events: u64,
    last_mismatch: Option<(u64, u64)>,
    last_stale_token: Option<u64>,
}

impl NativeScanoutBackend {
    fn open(plan: NativeScanoutPlan, kms: &fs::File, width: u32, height: u32) -> io::Result<Self> {
        let mut last_error = None;
        for candidate in plan.candidates() {
            match Self::open_kind(candidate, kms, width, height) {
                Ok(backend) => return Ok(backend),
                Err(error) => {
                    eprintln!(
                        "native scanout: {} backend failed: {error}",
                        candidate.as_str()
                    );
                    last_error = Some(error);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| io::Error::other("native scanout backend unavailable")))
    }

    fn open_kind(
        kind: NativeScanoutKind,
        kms: &fs::File,
        width: u32,
        height: u32,
    ) -> io::Result<Self> {
        match kind {
            NativeScanoutKind::NativeEglGbm => {
                if native_test_fail_native_egl_gbm_enabled() {
                    return Err(io::Error::other(
                        "native EGL/GBM failure injected by OBLIVION_ONE_TEST_FAIL_NATIVE_EGL_GBM",
                    ));
                }
                Ok(Self::NativeEglGbm(Box::new(NativeEglGbmScanout::create(
                    kms, width, height,
                )?)))
            }
            NativeScanoutKind::GbmCpuWritePageFlip => {
                Ok(Self::Gbm(NativeGbmScanout::create(kms, width, height)?))
            }
            NativeScanoutKind::DumbFramebuffer => {
                Ok(Self::Dumb(DumbFramebuffer::create(kms, width, height)?))
            }
            NativeScanoutKind::Unavailable => {
                Err(io::Error::other("native scanout backend unavailable"))
            }
        }
    }

    const fn kind(&self) -> NativeScanoutKind {
        match self {
            Self::NativeEglGbm(_) => NativeScanoutKind::NativeEglGbm,
            Self::Gbm(_) => NativeScanoutKind::GbmCpuWritePageFlip,
            Self::Dumb(_) => NativeScanoutKind::DumbFramebuffer,
        }
    }

    const fn supports_gpu_buffer_protocols(&self) -> bool {
        matches!(self, Self::NativeEglGbm(_))
    }

    fn paint_server_frame(
        &mut self,
        renderer: &mut NativeFrameRenderer,
        server: &OwnCompositorServer,
        input_state: &NativeInputState,
        cursor_mode: NativeCursorRenderMode,
        damage: &NativeOutputDamage,
    ) -> io::Result<NativePaintStats> {
        match self {
            Self::NativeEglGbm(scanout) => {
                scanout.paint_server_frame(renderer, server, input_state, cursor_mode)
            }
            Self::Gbm(scanout) => {
                scanout.paint_server_frame(renderer, server, input_state, cursor_mode, damage)
            }
            Self::Dumb(framebuffer) => {
                framebuffer.paint_server_frame(renderer, server, input_state, cursor_mode, damage)
            }
        }
    }

    fn fb_id(&self) -> u32 {
        match self {
            Self::NativeEglGbm(scanout) => scanout.fb_id(),
            Self::Gbm(scanout) => scanout.fb_id(),
            Self::Dumb(framebuffer) => framebuffer.fb_id,
        }
    }

    fn finish_initial_scanout(&mut self) {
        match self {
            Self::NativeEglGbm(scanout) => scanout.finish_initial_scanout(),
            Self::Gbm(scanout) => scanout.finish_initial_scanout(),
            Self::Dumb(_) => {}
        }
    }

    fn present(&mut self, fd: BorrowedFd<'_>, crtc_id: u32) -> io::Result<NativePresentResult> {
        let submitted_token = match self {
            Self::NativeEglGbm(scanout) => scanout.present(fd, crtc_id)?,
            Self::Gbm(scanout) => scanout.present(fd, crtc_id)?,
            Self::Dumb(_) => return Ok(NativePresentResult::Immediate),
        };
        match submitted_token {
            Some(token) => Ok(NativePresentResult::AsyncSubmitted { token }),
            None => Ok(NativePresentResult::Noop),
        }
    }

    fn drain_page_flip_events(&mut self, fd: RawFd) -> io::Result<NativePageFlipDrain> {
        match self {
            Self::NativeEglGbm(scanout) => scanout.drain_page_flip_events(fd),
            Self::Gbm(scanout) => scanout.drain_page_flip_events(fd),
            Self::Dumb(_) => Ok(NativePageFlipDrain::default()),
        }
    }

    fn page_flip_pending(&self) -> bool {
        match self {
            Self::NativeEglGbm(scanout) => scanout.page_flip_pending(),
            Self::Gbm(scanout) => scanout.page_flip_pending(),
            Self::Dumb(_) => false,
        }
    }

    fn pending_page_flip_token(&self) -> Option<u64> {
        match self {
            Self::NativeEglGbm(scanout) => scanout.page_flip.pending_token(),
            Self::Gbm(scanout) => scanout.page_flip.pending_token(),
            Self::Dumb(_) => None,
        }
    }

    fn dmabuf_feedback(&self) -> EglGlesDmabufFeedback {
        match self {
            Self::NativeEglGbm(scanout) => scanout.dmabuf_feedback.clone(),
            Self::Gbm(_) | Self::Dumb(_) => EglGlesDmabufFeedback::new(Vec::new()),
        }
    }

    fn dmabuf_main_device(&self) -> Option<u64> {
        match self {
            Self::NativeEglGbm(scanout) => scanout.dmabuf_main_device,
            Self::Gbm(_) | Self::Dumb(_) => None,
        }
    }

    fn dmabuf_main_device_path(&self) -> Option<String> {
        match self {
            Self::NativeEglGbm(scanout) => scanout.dmabuf_main_device_path.clone(),
            Self::Gbm(_) | Self::Dumb(_) => None,
        }
    }
}

fn apply_native_scanout_feedback(server: &mut OwnCompositorServer, scanout: &NativeScanoutBackend) {
    server.set_dmabuf_feedback(
        scanout.dmabuf_feedback(),
        scanout.dmabuf_main_device(),
        scanout.dmabuf_main_device_path(),
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativePageFlipError {
    AlreadyPending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativePageFlipCompletion {
    Completed { token: u64 },
    Mismatched { expected: u64, received: u64 },
    Stale { token: u64 },
}

static NEXT_NATIVE_PAGE_FLIP_TOKEN: AtomicU64 = AtomicU64::new(1);
static NEXT_NATIVE_DRM_FILE_GENERATION: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct NativePageFlipState {
    pending_token: Option<u64>,
}

impl NativePageFlipState {
    const fn can_schedule(self) -> bool {
        self.pending_token.is_none()
    }

    fn reserve_submission(&mut self) -> Result<u64, NativePageFlipError> {
        if self.pending_token.is_some() {
            return Err(NativePageFlipError::AlreadyPending);
        }
        let token = allocate_native_page_flip_token();
        self.pending_token = Some(token);
        Ok(token)
    }

    fn cancel_submission(&mut self, token: u64) -> bool {
        if self.pending_token == Some(token) {
            self.pending_token = None;
            true
        } else {
            false
        }
    }

    const fn pending_token(self) -> Option<u64> {
        self.pending_token
    }

    fn complete(&mut self, token: u64) -> NativePageFlipCompletion {
        let Some(expected) = self.pending_token else {
            return NativePageFlipCompletion::Stale { token };
        };
        if expected != token {
            return NativePageFlipCompletion::Mismatched {
                expected,
                received: token,
            };
        }
        self.pending_token = None;
        NativePageFlipCompletion::Completed { token }
    }
}

fn allocate_native_page_flip_token() -> u64 {
    loop {
        let current = NEXT_NATIVE_PAGE_FLIP_TOKEN.load(Ordering::Relaxed);
        let token = current.max(1);
        let next = next_nonzero_page_flip_token(token);
        if NEXT_NATIVE_PAGE_FLIP_TOKEN
            .compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return token;
        }
    }
}

fn allocate_native_drm_file_generation() -> u64 {
    loop {
        let current = NEXT_NATIVE_DRM_FILE_GENERATION.load(Ordering::Relaxed);
        let generation = current.max(1);
        let next = generation.checked_add(1).unwrap_or(1);
        if NEXT_NATIVE_DRM_FILE_GENERATION
            .compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return generation;
        }
    }
}

const fn next_nonzero_page_flip_token(token: u64) -> u64 {
    let next = token.wrapping_add(1);
    if next == 0 { 1 } else { next }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NativePageFlipBuffers<T> {
    current: Option<T>,
    ready: Option<T>,
    pending: Option<T>,
}

impl<T> Default for NativePageFlipBuffers<T> {
    fn default() -> Self {
        Self {
            current: None,
            ready: None,
            pending: None,
        }
    }
}

impl<T> NativePageFlipBuffers<T> {
    fn set_ready(&mut self, buffer: T) {
        self.ready = Some(buffer);
    }

    fn ready_or_current(&self) -> Option<&T> {
        self.ready.as_ref().or(self.current.as_ref())
    }

    fn finish_initial_scanout(&mut self) {
        if let Some(buffer) = self.ready.take() {
            self.current = Some(buffer);
        }
    }

    fn take_ready(&mut self) -> Option<T> {
        self.ready.take()
    }

    fn restore_ready(&mut self, buffer: T) {
        self.ready = Some(buffer);
    }

    fn set_pending(&mut self, buffer: T) {
        self.pending = Some(buffer);
    }

    fn complete_page_flip(&mut self) -> bool {
        let Some(buffer) = self.pending.take() else {
            return false;
        };
        self.current = Some(buffer);
        true
    }
}

struct NativeEglGbmScanout {
    _device: gbm::Device<OwnedFd>,
    surface: gbm::Surface<()>,
    format: gbm::Format,
    fd: RawFd,
    width: u32,
    height: u32,
    egl: EglInstance,
    egl_display: egl::Display,
    egl_context: egl::Context,
    egl_surface: egl::Surface,
    scene: GlesSceneRenderer,
    swap_buffers_with_damage: Option<EglSwapBuffersWithDamage>,
    dmabuf_feedback: EglGlesDmabufFeedback,
    dmabuf_main_device: Option<u64>,
    dmabuf_main_device_path: Option<String>,
    framebuffer_cache: NativeGbmFramebufferCache,
    buffers: NativePageFlipBuffers<NativePresentedGbmBuffer>,
    page_flip: NativePageFlipState,
}

// A locked GBM front buffer must stay alive while KMS may scan it out. Ready is
// not on KMS yet, pending is waiting for the DRM event, and current is the last
// completed pageflip/modeset buffer. Dropping this value releases the GBM BO
// back to the surface, so transitions are driven only by modeset/pageflip state.
struct NativePresentedGbmBuffer {
    _bo: gbm::BufferObject<()>,
    fb_id: u32,
}

struct NativeEglGbmFormatConfig {
    format: gbm::Format,
    config: egl::Config,
}

fn native_egl_gbm_format_candidates() -> [gbm::Format; 2] {
    [gbm::Format::Xrgb8888, gbm::Format::Argb8888]
}

fn choose_native_egl_gbm_format_config<T: AsFd>(
    egl: &EglInstance,
    egl_display: egl::Display,
    device: &gbm::Device<T>,
    usage: gbm::BufferObjectFlags,
) -> io::Result<NativeEglGbmFormatConfig> {
    let mut unsupported = Vec::new();
    let mut egl_errors = Vec::new();
    for format in native_egl_gbm_format_candidates() {
        let format_label = native_visual_label(format as u32);
        if !device.is_format_supported(format, usage) {
            unsupported.push(format_label);
            continue;
        }
        match choose_native_egl_config(egl, egl_display, format as u32) {
            Ok(config) => return Ok(NativeEglGbmFormatConfig { format, config }),
            Err(error) => egl_errors.push(format!("{format_label}: {error}")),
        }
    }
    let unsupported = if unsupported.is_empty() {
        "none".to_string()
    } else {
        unsupported.join(", ")
    };
    let egl_errors = if egl_errors.is_empty() {
        "none".to_string()
    } else {
        egl_errors.join("; ")
    };
    Err(io::Error::other(format!(
        "GBM/EGL has no compatible native scanout format; unsupported_by_gbm={unsupported}; egl_errors={egl_errors}"
    )))
}

#[derive(Debug, Default)]
struct NativeGbmFramebufferCache {
    entries: HashMap<NativeGbmFramebufferMetadata, u32>,
}

impl NativeGbmFramebufferCache {
    fn fb_id_for(&mut self, fd: BorrowedFd<'_>, bo: &gbm::BufferObject<()>) -> io::Result<u32> {
        let metadata = NativeGbmFramebufferMetadata::from_bo(bo);
        if let Some(fb_id) = self.entries.get(&metadata) {
            return Ok(*fb_id);
        }
        let fb_id = add_gbm_framebuffer(fd, bo)?;
        self.entries.insert(metadata, fb_id);
        Ok(fb_id)
    }

    fn clear(&mut self, fd: BorrowedFd<'_>) {
        for (_, fb_id) in self.entries.drain() {
            let _ = drm_ffi::mode::rm_fb(fd, fb_id);
        }
    }
}

impl NativeEglGbmScanout {
    fn create(kms: &fs::File, width: u32, height: u32) -> io::Result<Self> {
        let gbm_fd = duplicate_fd_cloexec(kms.as_raw_fd()).map_err(io::Error::from_raw_os_error)?;
        let device = gbm::Device::new(gbm_fd)?;
        let usage = gbm::BufferObjectFlags::SCANOUT | gbm::BufferObjectFlags::RENDERING;

        let egl = unsafe { EglInstance::load_required() }.map_err(native_egl_io_error)?;
        const EGL_PLATFORM_GBM_KHR: egl::Enum = 0x31d7;
        // EGL_PLATFORM_GBM_KHR requires a gbm_device pointer, not a DRM fd cast
        // to a pointer. The GBM device is kept alive by NativeEglGbmScanout.
        let egl_display = match unsafe {
            egl.get_platform_display(
                EGL_PLATFORM_GBM_KHR,
                device.as_raw_mut() as egl::NativeDisplayType,
                &[egl::ATTRIB_NONE],
            )
        } {
            Ok(display) => display,
            Err(error) => return Err(native_egl_io_error(error)),
        };
        if let Err(error) = egl.initialize(egl_display) {
            let _ = egl.terminate(egl_display);
            return Err(native_egl_io_error(error));
        }
        if let Err(error) = egl.bind_api(egl::OPENGL_ES_API) {
            let _ = egl.terminate(egl_display);
            return Err(native_egl_io_error(error));
        }
        let format_config =
            match choose_native_egl_gbm_format_config(&egl, egl_display, &device, usage) {
                Ok(format_config) => format_config,
                Err(error) => {
                    let _ = egl.terminate(egl_display);
                    return Err(error);
                }
            };
        let surface = match device.create_surface(width, height, format_config.format, usage) {
            Ok(surface) => surface,
            Err(error) => {
                let _ = egl.terminate(egl_display);
                return Err(error);
            }
        };
        let egl_config = format_config.config;
        let egl_context = match create_gles_context(&egl, egl_display, egl_config) {
            Ok(context) => context,
            Err(error) => {
                let _ = egl.terminate(egl_display);
                return Err(native_egl_io_error(error));
            }
        };
        let egl_surface = match unsafe {
            egl.create_platform_window_surface(
                egl_display,
                egl_config,
                surface.as_raw_mut() as egl::NativeWindowType,
                &[egl::ATTRIB_NONE],
            )
        } {
            Ok(surface) => surface,
            Err(error) => {
                let _ = egl.destroy_context(egl_display, egl_context);
                let _ = egl.terminate(egl_display);
                return Err(native_egl_io_error(error));
            }
        };
        if let Err(error) = egl.make_current(
            egl_display,
            Some(egl_surface),
            Some(egl_surface),
            Some(egl_context),
        ) {
            let _ = egl.destroy_surface(egl_display, egl_surface);
            let _ = egl.destroy_context(egl_display, egl_context);
            let _ = egl.terminate(egl_display);
            return Err(native_egl_io_error(error));
        }
        if let Err(error) = egl.swap_interval(egl_display, 1) {
            eprintln!("native EGL/GBM: EGL swap interval unavailable: {error}");
        }

        let egl_image_target_texture_2d: Option<GlEglImageTargetTexture2DOes> =
            load_egl_image_target_texture_2d(&egl).or_else(|| {
                eprintln!(
                    "native EGL/GBM: GL_OES_EGL_image entry point unavailable; dmabuf surfaces will be skipped"
                );
                None
            });
        let scene = match GlesSceneRenderer::new_current(
            &egl,
            width,
            height,
            egl_image_target_texture_2d,
        ) {
            Ok(scene) => scene,
            Err(error) => {
                let _ = egl.make_current(egl_display, None, None, None);
                let _ = egl.destroy_surface(egl_display, egl_surface);
                let _ = egl.destroy_context(egl_display, egl_context);
                let _ = egl.terminate(egl_display);
                return Err(native_egl_io_error(error));
            }
        };
        let swap_buffers_with_damage = load_swap_buffers_with_damage(&egl, egl_display);
        let dmabuf_feedback = query_egl_dmabuf_feedback(&egl, egl_display);
        let (dmabuf_main_device_path, dmabuf_main_device) =
            match query_egl_main_device(&egl, egl_display) {
                Some((path, main_device)) => (Some(path), Some(main_device)),
                None => (None, None),
            };
        let vendor = egl
            .query_string(Some(egl_display), egl::VENDOR)
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "unknown".to_string());
        let version = egl
            .query_string(Some(egl_display), egl::VERSION)
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "unknown".to_string());
        let gl_info = scene.renderer_info();
        println!(
            "native EGL/GBM GLES3 renderer active: EGL {vendor} {version}; GL {} {} ({}) on {} format {}",
            gl_info.vendor,
            gl_info.renderer,
            gl_info.version,
            device.backend_name(),
            native_visual_label(format_config.format as u32)
        );

        Ok(Self {
            _device: device,
            surface,
            format: format_config.format,
            fd: kms.as_raw_fd(),
            width,
            height,
            egl,
            egl_display,
            egl_context,
            egl_surface,
            scene,
            swap_buffers_with_damage,
            dmabuf_feedback,
            dmabuf_main_device,
            dmabuf_main_device_path,
            framebuffer_cache: NativeGbmFramebufferCache::default(),
            buffers: NativePageFlipBuffers::default(),
            page_flip: NativePageFlipState::default(),
        })
    }

    fn paint_server_frame(
        &mut self,
        renderer: &mut NativeFrameRenderer,
        server: &OwnCompositorServer,
        input_state: &NativeInputState,
        cursor_mode: NativeCursorRenderMode,
    ) -> io::Result<NativePaintStats> {
        if !self.surface.has_free_buffers() {
            return Err(io::Error::other(
                "native EGL/GBM surface has no free buffers",
            ));
        }

        let total_start = Instant::now();
        self.egl
            .make_current(
                self.egl_display,
                Some(self.egl_surface),
                Some(self.egl_surface),
                Some(self.egl_context),
            )
            .map_err(native_egl_io_error)?;
        let request = renderer.egl_scene_draw_request(
            self.width,
            self.height,
            server,
            input_state,
            cursor_mode,
        );
        let draw_start = Instant::now();
        let output_damage = self
            .scene
            .draw_scene(&self.egl, self.egl_display, request)
            .map_err(native_egl_io_error)?;
        let draw_us = elapsed_micros(draw_start);
        let scene_stats = self.scene.last_frame_stats();
        let swap_start = Instant::now();
        egl_swap_buffers_with_damage(
            &self.egl,
            self.egl_display,
            self.egl_surface,
            self.swap_buffers_with_damage,
            output_damage,
        )
        .map_err(native_egl_io_error)?;
        let swap_us = elapsed_micros(swap_start);
        let bo = unsafe { self.surface.lock_front_buffer() }.map_err(|error| {
            io::Error::other(format!("failed to lock GBM front buffer: {error}"))
        })?;
        let fd = unsafe { BorrowedFd::borrow_raw(self.fd) };
        let fb_id = self.framebuffer_cache.fb_id_for(fd, &bo)?;
        self.buffers
            .set_ready(NativePresentedGbmBuffer { _bo: bo, fb_id });
        Ok(native_egl_gbm_paint_stats(
            self.format as u32,
            self.width,
            self.height,
            draw_us,
            swap_us,
            elapsed_micros(total_start),
            scene_stats,
        ))
    }

    fn fb_id(&self) -> u32 {
        self.buffers
            .ready_or_current()
            .map(|buffer| buffer.fb_id)
            .unwrap_or(0)
    }

    fn finish_initial_scanout(&mut self) {
        self.buffers.finish_initial_scanout();
    }

    fn present(&mut self, fd: BorrowedFd<'_>, crtc_id: u32) -> io::Result<Option<u64>> {
        if !self.page_flip.can_schedule() {
            return Ok(None);
        }
        let Some(buffer) = self.buffers.take_ready() else {
            return Ok(None);
        };
        let token = self
            .page_flip
            .reserve_submission()
            .map_err(|_| io::Error::other("native page flip is already pending"))?;
        match submit_legacy_page_flip(fd, crtc_id, buffer.fb_id, token) {
            Ok(()) => {
                self.buffers.set_pending(buffer);
                Ok(Some(token))
            }
            Err(error) => {
                self.page_flip.cancel_submission(token);
                self.buffers.restore_ready(buffer);
                Err(error)
            }
        }
    }

    fn drain_page_flip_events(&mut self, fd: RawFd) -> io::Result<NativePageFlipDrain> {
        let mut drain = NativePageFlipDrain::default();
        for event in drain_drm_page_flip_events(fd)? {
            match self.page_flip.complete(event.user_data) {
                NativePageFlipCompletion::Completed { .. } => {
                    if drain.completion.is_none() {
                        self.buffers.complete_page_flip();
                        drain.completion = Some(event);
                    } else {
                        drain.stale_events = drain.stale_events.saturating_add(1);
                    }
                }
                NativePageFlipCompletion::Mismatched { expected, received } => {
                    drain.mismatched_events = drain.mismatched_events.saturating_add(1);
                    drain.last_mismatch = Some((expected, received));
                }
                NativePageFlipCompletion::Stale { token } => {
                    drain.stale_events = drain.stale_events.saturating_add(1);
                    drain.last_stale_token = Some(token);
                }
            }
        }
        Ok(drain)
    }

    fn page_flip_pending(&self) -> bool {
        !self.page_flip.can_schedule()
    }
}

impl Drop for NativeEglGbmScanout {
    fn drop(&mut self) {
        // GL textures/EGLImages are destroyed while the context is current.
        // DRM framebuffer IDs are removed before the locked GBM BO guards drop.
        let _ = self.egl.make_current(
            self.egl_display,
            Some(self.egl_surface),
            Some(self.egl_surface),
            Some(self.egl_context),
        );
        self.scene.destroy(&self.egl, self.egl_display);
        let _ = self.egl.make_current(self.egl_display, None, None, None);
        let _ = self.egl.destroy_surface(self.egl_display, self.egl_surface);
        let _ = self.egl.destroy_context(self.egl_display, self.egl_context);
        let _ = self.egl.terminate(self.egl_display);
        let fd = unsafe { BorrowedFd::borrow_raw(self.fd) };
        self.framebuffer_cache.clear(fd);
    }
}

fn native_egl_gbm_paint_stats(
    scanout_format: u32,
    width: u32,
    height: u32,
    draw_us: u64,
    swap_us: u64,
    total_us: u64,
    scene_stats: GlesSceneFrameStats,
) -> NativePaintStats {
    NativePaintStats {
        backend: NativeScanoutKind::NativeEglGbm,
        scanout_format: Some(scanout_format),
        width,
        height,
        bytes: 0,
        copy_bytes: 0,
        write_bytes: 0,
        gpu_draw_us: draw_us,
        egl_swap_us: swap_us,
        shm_upload_bytes: scene_stats.shm_upload_bytes,
        dmabuf_imports: scene_stats.dmabuf_imports,
        dmabuf_reuses: scene_stats.dmabuf_reuses,
        dmabuf_import_failures: scene_stats.dmabuf_import_failures,
        scene_rebuild: if scene_stats.scene_rebuilt {
            DesktopSceneRebuildKind::Full
        } else {
            DesktopSceneRebuildKind::None
        },
        frame_copy: DesktopFrameCopyKind::None,
        total_us,
        render_us: draw_us,
        copy_us: 0,
        write_us: 0,
    }
}

fn native_egl_io_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

struct NativeGbmScanout {
    _device: gbm::Device<OwnedFd>,
    fd: RawFd,
    width: u32,
    height: u32,
    buffers: Vec<NativeGbmScanoutBuffer>,
    current_index: usize,
    ready_index: Option<usize>,
    pending_index: Option<usize>,
    page_flip: NativePageFlipState,
    staging: Vec<u8>,
}

struct NativeGbmScanoutBuffer {
    bo: gbm::BufferObject<()>,
    fb_id: u32,
    pitch: u32,
}

impl NativeGbmScanout {
    fn create(kms: &fs::File, width: u32, height: u32) -> io::Result<Self> {
        let gbm_fd = duplicate_fd_cloexec(kms.as_raw_fd()).map_err(io::Error::from_raw_os_error)?;
        let device = gbm::Device::new(gbm_fd)?;
        let usage = gbm::BufferObjectFlags::SCANOUT
            | gbm::BufferObjectFlags::WRITE
            | gbm::BufferObjectFlags::LINEAR;
        if !device.is_format_supported(gbm::Format::Xrgb8888, usage) {
            return Err(io::Error::other(
                "GBM device does not support writable XRGB8888 scanout buffers",
            ));
        }

        let mut buffers = Vec::new();
        for _ in 0..3 {
            let bo = device.create_buffer_object(width, height, gbm::Format::Xrgb8888, usage)?;
            let fb_id = add_gbm_framebuffer(kms.as_fd(), &bo)?;
            let pitch = bo.stride();
            buffers.push(NativeGbmScanoutBuffer { bo, fb_id, pitch });
        }
        println!(
            "native scanout: GBM write/pageflip buffers ready: {}x{}, {} buffer(s), backend {}",
            width,
            height,
            buffers.len(),
            device.backend_name()
        );
        Ok(Self {
            _device: device,
            fd: kms.as_raw_fd(),
            width,
            height,
            buffers,
            current_index: 0,
            ready_index: None,
            pending_index: None,
            page_flip: NativePageFlipState::default(),
            staging: Vec::new(),
        })
    }

    fn paint_server_frame(
        &mut self,
        renderer: &mut NativeFrameRenderer,
        server: &OwnCompositorServer,
        input_state: &NativeInputState,
        cursor_mode: NativeCursorRenderMode,
        damage: &NativeOutputDamage,
    ) -> io::Result<NativePaintStats> {
        let total_start = Instant::now();
        let index = self.next_render_index()?;
        let render_start = Instant::now();
        let rendered =
            renderer.render_server_frame(self.width, self.height, server, input_state, cursor_mode);
        let render_us = elapsed_micros(render_start);
        let buffer = &mut self.buffers[index];
        let byte_len = buffer
            .pitch
            .checked_mul(self.height)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| io::Error::other("GBM scanout buffer size overflow"))?;
        let copy_start = Instant::now();
        self.staging.resize(byte_len, 0);
        let copy_bytes = copy_argb_frame_to_xrgb_mapping_damage(
            rendered.pixels,
            self.width,
            self.height,
            buffer.pitch,
            &mut self.staging,
            damage.frame_copy_damage_for_scene(rendered.scene_rebuild_kind),
        )?;
        let copy_us = elapsed_micros(copy_start);
        let write_start = Instant::now();
        buffer.bo.write(&self.staging)?;
        let write_us = elapsed_micros(write_start);
        self.ready_index = Some(index);
        Ok(NativePaintStats {
            backend: NativeScanoutKind::GbmCpuWritePageFlip,
            scanout_format: None,
            width: self.width,
            height: self.height,
            bytes: byte_len,
            copy_bytes,
            write_bytes: byte_len,
            gpu_draw_us: 0,
            egl_swap_us: 0,
            shm_upload_bytes: 0,
            dmabuf_imports: 0,
            dmabuf_reuses: 0,
            dmabuf_import_failures: 0,
            scene_rebuild: rendered.scene_rebuild_kind,
            frame_copy: rendered.frame_copy_kind,
            total_us: elapsed_micros(total_start),
            render_us,
            copy_us,
            write_us,
        })
    }

    fn fb_id(&self) -> u32 {
        self.ready_index
            .map(|index| self.buffers[index].fb_id)
            .unwrap_or_else(|| self.buffers[self.current_index].fb_id)
    }

    fn finish_initial_scanout(&mut self) {
        if let Some(index) = self.ready_index.take() {
            self.current_index = index;
        }
    }

    fn present(&mut self, fd: BorrowedFd<'_>, crtc_id: u32) -> io::Result<Option<u64>> {
        if !self.page_flip.can_schedule() {
            return Ok(None);
        }
        let Some(index) = self.ready_index.take() else {
            return Ok(None);
        };
        if index == self.current_index {
            return Ok(None);
        }
        let token = self
            .page_flip
            .reserve_submission()
            .map_err(|_| io::Error::other("native page flip is already pending"))?;
        match submit_legacy_page_flip(fd, crtc_id, self.buffers[index].fb_id, token) {
            Ok(()) => {
                self.pending_index = Some(index);
                Ok(Some(token))
            }
            Err(error) => {
                self.page_flip.cancel_submission(token);
                self.ready_index = Some(index);
                Err(error)
            }
        }
    }

    fn drain_page_flip_events(&mut self, fd: RawFd) -> io::Result<NativePageFlipDrain> {
        let mut drain = NativePageFlipDrain::default();
        for event in drain_drm_page_flip_events(fd)? {
            match self.page_flip.complete(event.user_data) {
                NativePageFlipCompletion::Completed { .. } => {
                    if drain.completion.is_none() {
                        if let Some(index) = self.pending_index.take() {
                            self.current_index = index;
                        }
                        drain.completion = Some(event);
                    } else {
                        drain.stale_events = drain.stale_events.saturating_add(1);
                    }
                }
                NativePageFlipCompletion::Mismatched { expected, received } => {
                    drain.mismatched_events = drain.mismatched_events.saturating_add(1);
                    drain.last_mismatch = Some((expected, received));
                }
                NativePageFlipCompletion::Stale { token } => {
                    drain.stale_events = drain.stale_events.saturating_add(1);
                    drain.last_stale_token = Some(token);
                }
            }
        }
        Ok(drain)
    }

    fn page_flip_pending(&self) -> bool {
        !self.page_flip.can_schedule()
    }

    fn next_render_index(&self) -> io::Result<usize> {
        if let Some(index) = self.ready_index {
            return Ok(index);
        }
        self.buffers
            .iter()
            .enumerate()
            .map(|(index, _)| index)
            .find(|index| {
                Some(*index) != self.pending_index
                    && Some(*index) != self.ready_index
                    && *index != self.current_index
            })
            .ok_or_else(|| io::Error::other("no free GBM scanout buffer is available"))
    }
}

impl Drop for NativeGbmScanout {
    fn drop(&mut self) {
        let fd = unsafe { BorrowedFd::borrow_raw(self.fd) };
        for buffer in &self.buffers {
            let _ = drm_ffi::mode::rm_fb(fd, buffer.fb_id);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct NativeGbmFramebufferMetadata {
    width: u32,
    height: u32,
    format: u32,
    handles: [u32; 4],
    pitches: [u32; 4],
    offsets: [u32; 4],
    modifiers: [u64; 4],
    flags: u32,
}

impl NativeGbmFramebufferMetadata {
    fn from_bo(bo: &gbm::BufferObject<()>) -> Self {
        let mut handles = [0; 4];
        let mut pitches = [0; 4];
        let mut offsets = [0; 4];
        let mut modifiers = [0; 4];
        let plane_count = bo.plane_count().clamp(1, 4);
        let modifier = u64::from(bo.modifier());
        for plane in 0..plane_count {
            let index = plane as usize;
            handles[index] = unsafe { bo.handle_for_plane(plane as i32).u32_ };
            if handles[index] == 0 {
                handles[index] = unsafe { bo.handle().u32_ };
            }
            pitches[index] = bo.stride_for_plane(plane as i32);
            if pitches[index] == 0 {
                pitches[index] = bo.stride();
            }
            offsets[index] = bo.offset(plane as i32);
            modifiers[index] = modifier;
        }
        let flags = if bo.modifier() == gbm::Modifier::Invalid {
            0
        } else {
            drm_sys::DRM_MODE_FB_MODIFIERS
        };
        Self {
            width: bo.width(),
            height: bo.height(),
            format: bo.format() as u32,
            handles,
            pitches,
            offsets,
            modifiers,
            flags,
        }
    }
}

fn add_gbm_framebuffer(fd: BorrowedFd<'_>, bo: &gbm::BufferObject<()>) -> io::Result<u32> {
    let metadata = NativeGbmFramebufferMetadata::from_bo(bo);
    drm_ffi::mode::add_fb2(
        fd,
        metadata.width,
        metadata.height,
        metadata.format,
        &metadata.handles,
        &metadata.pitches,
        &metadata.offsets,
        &metadata.modifiers,
        metadata.flags,
    )
    .map(|framebuffer| framebuffer.fb_id)
}

fn set_fd_nonblocking(fd: RawFd) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

struct DumbFramebuffer {
    fd: RawFd,
    handle: u32,
    fb_id: u32,
    width: u32,
    height: u32,
    pitch: u32,
    size: usize,
    mapping: *mut c_void,
}

impl DumbFramebuffer {
    fn create(file: &fs::File, width: u32, height: u32) -> io::Result<Self> {
        let dumb = drm_ffi::mode::dumbbuffer::create(file.as_fd(), width, height, 32, 0)?;
        let fb = match drm_ffi::mode::add_fb(
            file.as_fd(),
            width,
            height,
            dumb.pitch,
            32,
            24,
            dumb.handle,
        ) {
            Ok(fb) => fb,
            Err(error) => {
                let _ = drm_ffi::mode::dumbbuffer::destroy(file.as_fd(), dumb.handle);
                return Err(error);
            }
        };
        let map = match drm_ffi::mode::dumbbuffer::map(file.as_fd(), dumb.handle, 0, 0) {
            Ok(map) => map,
            Err(error) => {
                let _ = drm_ffi::mode::rm_fb(file.as_fd(), fb.fb_id);
                let _ = drm_ffi::mode::dumbbuffer::destroy(file.as_fd(), dumb.handle);
                return Err(error);
            }
        };
        let mapping = unsafe {
            libc::mmap(
                ptr::null_mut(),
                dumb.size as usize,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                map.offset as libc::off_t,
            )
        };
        if mapping == libc::MAP_FAILED {
            let error = io::Error::last_os_error();
            let _ = drm_ffi::mode::rm_fb(file.as_fd(), fb.fb_id);
            let _ = drm_ffi::mode::dumbbuffer::destroy(file.as_fd(), dumb.handle);
            return Err(error);
        }

        Ok(Self {
            fd: file.as_raw_fd(),
            handle: dumb.handle,
            fb_id: fb.fb_id,
            width,
            height,
            pitch: dumb.pitch,
            size: dumb.size as usize,
            mapping,
        })
    }

    fn paint_server_frame(
        &mut self,
        renderer: &mut NativeFrameRenderer,
        server: &OwnCompositorServer,
        input_state: &NativeInputState,
        cursor_mode: NativeCursorRenderMode,
        damage: &NativeOutputDamage,
    ) -> io::Result<NativePaintStats> {
        let total_start = Instant::now();
        let render_start = Instant::now();
        let rendered =
            renderer.render_server_frame(self.width, self.height, server, input_state, cursor_mode);
        let render_us = elapsed_micros(render_start);
        let bytes = unsafe { slice::from_raw_parts_mut(self.mapping.cast::<u8>(), self.size) };
        let copy_start = Instant::now();
        let copy_bytes = copy_argb_frame_to_xrgb_mapping_damage(
            rendered.pixels,
            self.width,
            self.height,
            self.pitch,
            bytes,
            damage.frame_copy_damage_for_scene(rendered.scene_rebuild_kind),
        )?;
        let copy_us = elapsed_micros(copy_start);
        Ok(NativePaintStats {
            backend: NativeScanoutKind::DumbFramebuffer,
            scanout_format: None,
            width: self.width,
            height: self.height,
            bytes: self.size,
            copy_bytes,
            write_bytes: 0,
            gpu_draw_us: 0,
            egl_swap_us: 0,
            shm_upload_bytes: 0,
            dmabuf_imports: 0,
            dmabuf_reuses: 0,
            dmabuf_import_failures: 0,
            scene_rebuild: rendered.scene_rebuild_kind,
            frame_copy: rendered.frame_copy_kind,
            total_us: elapsed_micros(total_start),
            render_us,
            copy_us,
            write_us: 0,
        })
    }
}

impl Drop for DumbFramebuffer {
    fn drop(&mut self) {
        let fd = unsafe { BorrowedFd::borrow_raw(self.fd) };
        let _ = unsafe { libc::munmap(self.mapping, self.size) };
        let _ = drm_ffi::mode::rm_fb(fd, self.fb_id);
        let _ = drm_ffi::mode::dumbbuffer::destroy(fd, self.handle);
    }
}

struct CrtcRestore {
    fd: RawFd,
    crtc_id: u32,
    connector_id: u32,
    original: Option<drm_sys::drm_mode_crtc>,
}

impl CrtcRestore {
    fn new(
        fd: RawFd,
        crtc_id: u32,
        connector_id: u32,
        original: Option<drm_sys::drm_mode_crtc>,
    ) -> Self {
        Self {
            fd,
            crtc_id,
            connector_id,
            original,
        }
    }
}

impl Drop for CrtcRestore {
    fn drop(&mut self) {
        let fd = unsafe { BorrowedFd::borrow_raw(self.fd) };
        if let Some(original) = self.original {
            let mode = (original.mode_valid != 0).then_some(original.mode);
            let connectors = if mode.is_some() {
                vec![self.connector_id]
            } else {
                Vec::new()
            };
            let _ = drm_ffi::mode::set_crtc(
                fd,
                self.crtc_id,
                original.fb_id,
                original.x,
                original.y,
                &connectors,
                mode,
            );
        }
    }
}

fn probe_native_egl_gbm_device(bootstrap: &NativeOutputBootstrap, perf: &NativePerfLogger) {
    if bootstrap.kms_device.is_none() || bootstrap.render_device.is_none() {
        perf.log("native.egl_probe", || {
            vec![
                NativePerfField::str("status", "skipped"),
                NativePerfField::str("reason", "missing_kms_or_render_device"),
            ]
        });
        return;
    }

    let kms_device = bootstrap.kms_device.as_deref().unwrap();
    let render_device = bootstrap.render_device.as_deref().unwrap();

    let egl = match unsafe { egl::DynamicInstance::<egl::EGL1_5>::load_required() } {
        Ok(egl) => egl,
        Err(e) => {
            perf.log("native.egl_probe", || {
                vec![
                    NativePerfField::str("status", "failed"),
                    NativePerfField::str("reason", "egl_load_failed"),
                    NativePerfField::str("error", e.to_string()),
                ]
            });
            return;
        }
    };

    let display_fd = match OpenOptions::new().read(true).write(true).open(kms_device) {
        Ok(fd) => fd,
        Err(e) => {
            perf.log("native.egl_probe", || {
                vec![
                    NativePerfField::str("status", "failed"),
                    NativePerfField::str("reason", "kms_device_open_failed"),
                    NativePerfField::str("device", kms_device.display().to_string()),
                    NativePerfField::str("error", e.to_string()),
                ]
            });
            return;
        }
    };
    let gbm_device = match gbm::Device::new(display_fd) {
        Ok(device) => device,
        Err(e) => {
            perf.log("native.egl_probe", || {
                vec![
                    NativePerfField::str("status", "failed"),
                    NativePerfField::str("reason", "gbm_device_create_failed"),
                    NativePerfField::str("device", kms_device.display().to_string()),
                    NativePerfField::str("error", e.to_string()),
                ]
            });
            return;
        }
    };

    const EGL_PLATFORM_GBM_KHR: egl::Enum = 0x31d7;
    // The GBM platform native display is the gbm_device pointer. Passing the
    // integer DRM fd as a pointer makes the probe succeed/fail for the wrong
    // reason on different EGL stacks.
    let display = match unsafe {
        egl.get_platform_display(
            EGL_PLATFORM_GBM_KHR,
            gbm_device.as_raw_mut() as egl::NativeDisplayType,
            &[egl::ATTRIB_NONE],
        )
    } {
        Ok(display) => display,
        Err(_) => {
            perf.log("native.egl_probe", || {
                vec![
                    NativePerfField::str("status", "failed"),
                    NativePerfField::str("reason", "get_platform_display_failed"),
                ]
            });
            return;
        }
    };

    let (major, minor) = match egl.initialize(display) {
        Ok(version) => version,
        Err(_) => {
            perf.log("native.egl_probe", || {
                vec![
                    NativePerfField::str("status", "failed"),
                    NativePerfField::str("reason", "egl_initialize_failed"),
                ]
            });
            let _ = egl.terminate(display);
            return;
        }
    };

    let vendor_str = egl
        .query_string(Some(display), egl::VENDOR)
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    let version_str = egl
        .query_string(Some(display), egl::VERSION)
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext_str = egl
        .query_string(Some(display), egl::EXTENSIONS)
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();

    let has_dmabuf_import = ext_str.contains("EGL_EXT_image_dma_buf_import");
    let has_dmabuf_import_modifiers = ext_str.contains("EGL_EXT_image_dma_buf_import_modifiers");
    let has_native_fence_sync =
        ext_str.contains("EGL_ANDROID_native_fence_sync") || ext_str.contains("EGL_KHR_fence_sync");
    let has_surfaceless = ext_str.contains("EGL_KHR_surfaceless_context");
    let has_pbuffer = ext_str.contains("EGL_KHR_pbuffer_context");

    let config_count = egl.get_config_count(display).unwrap_or_default();
    let has_config = choose_egl_config(&egl, display).is_ok();
    let has_native_xrgb_config =
        choose_native_egl_config(&egl, display, gbm::Format::Xrgb8888 as u32).is_ok();
    let has_native_argb_config =
        choose_native_egl_config(&egl, display, gbm::Format::Argb8888 as u32).is_ok();
    let usage = gbm::BufferObjectFlags::SCANOUT | gbm::BufferObjectFlags::RENDERING;
    let selected_native_format =
        choose_native_egl_gbm_format_config(&egl, display, &gbm_device, usage)
            .ok()
            .map(|format_config| format_config.format as u32);

    let feedback = query_egl_dmabuf_feedback(&egl, display);
    let (main_device_path, main_device) = query_egl_main_device(&egl, display)
        .map(|(path, device)| (Some(path), Some(device)))
        .unwrap_or((None, None));

    let table_format_count = feedback.format_table_formats().len();
    let tranche_format_count = feedback.formats().len();
    let has_nvidia_modifiers = feedback
        .formats()
        .iter()
        .any(|f| (f.modifier.0 & 0xff00_0000_0000_0000) == 0x0300_0000_0000_0000);

    perf.log("native.egl_probe", || {
        vec![
            NativePerfField::str("status", "success"),
            NativePerfField::str("vendor", &vendor_str),
            NativePerfField::str("version", &version_str),
            NativePerfField::str("kms_device", kms_device.display().to_string()),
            NativePerfField::str("render_device", render_device.display().to_string()),
            NativePerfField::u64("major", major as u64),
            NativePerfField::u64("minor", minor as u64),
            NativePerfField::bool("has_config", has_config),
            NativePerfField::bool("has_native_xrgb_config", has_native_xrgb_config),
            NativePerfField::bool("has_native_argb_config", has_native_argb_config),
            NativePerfField::str(
                "selected_native_format",
                selected_native_format
                    .map(native_visual_label)
                    .unwrap_or_else(|| "none".to_string()),
            ),
            NativePerfField::u64("config_count", config_count as u64),
            NativePerfField::bool("dmabuf_import", has_dmabuf_import),
            NativePerfField::bool("dmabuf_import_modifiers", has_dmabuf_import_modifiers),
            NativePerfField::bool("native_fence_sync", has_native_fence_sync),
            NativePerfField::bool("surfaceless_context", has_surfaceless),
            NativePerfField::bool("pbuffer_context", has_pbuffer),
            NativePerfField::u64("table_format_count", table_format_count as u64),
            NativePerfField::u64("tranche_format_count", tranche_format_count as u64),
            NativePerfField::bool("has_nvidia_modifiers", has_nvidia_modifiers),
            NativePerfField::str(
                "main_device",
                main_device
                    .map(|d| d.to_string())
                    .unwrap_or("none".to_string()),
            ),
            NativePerfField::str(
                "main_device_path",
                main_device_path.unwrap_or("none".to_string()),
            ),
        ]
    });

    let _ = egl.terminate(display);
}

#[cfg(test)]
fn copy_argb_frame_to_xrgb_mapping(
    frame: &[u32],
    width: u32,
    height: u32,
    pitch: u32,
    bytes: &mut [u8],
) -> io::Result<()> {
    copy_argb_frame_to_xrgb_mapping_damage(
        frame,
        width,
        height,
        pitch,
        bytes,
        NativeFrameCopyDamage::Full,
    )
    .map(|_| ())
}

#[derive(Debug, Clone, Copy)]
enum NativeFrameCopyDamage<'a> {
    Full,
    Rects(&'a [NativeDamageRect]),
}

fn copy_argb_frame_to_xrgb_mapping_damage(
    frame: &[u32],
    width: u32,
    height: u32,
    pitch: u32,
    bytes: &mut [u8],
    damage: NativeFrameCopyDamage<'_>,
) -> io::Result<usize> {
    let row_bytes = width
        .checked_mul(4)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| io::Error::other("native framebuffer row width overflow"))?;
    let pitch =
        usize::try_from(pitch).map_err(|_| io::Error::other("invalid framebuffer pitch"))?;
    let output_width = width;
    let output_height = height;
    let row_pixels = usize::try_from(width)
        .map_err(|_| io::Error::other("native framebuffer width overflow"))?;
    let height = usize::try_from(height)
        .map_err(|_| io::Error::other("native framebuffer height overflow"))?;
    let pixel_count = row_pixels
        .checked_mul(height)
        .ok_or_else(|| io::Error::other("native framebuffer source overflow"))?;
    if frame.len() < pixel_count {
        return Err(io::Error::other("native framebuffer source is too small"));
    }
    let frame_bytes_len = pixel_count
        .checked_mul(mem::size_of::<u32>())
        .ok_or_else(|| io::Error::other("native framebuffer source byte overflow"))?;
    // XRGB ignores the high byte, so the native ARGB words can be copied as-is.
    let frame_bytes =
        unsafe { slice::from_raw_parts(frame.as_ptr().cast::<u8>(), frame_bytes_len) };

    let full_copy_bytes = row_bytes
        .checked_mul(height)
        .ok_or_else(|| io::Error::other("native framebuffer full copy overflow"))?;
    let full_rect;
    let damage_rects = match damage {
        NativeFrameCopyDamage::Full => {
            full_rect = [NativeDamageRect {
                x: 0,
                y: 0,
                width: output_width,
                height: output_height,
            }];
            &full_rect[..]
        }
        NativeFrameCopyDamage::Rects(rects)
            if damage_rect_copy_bytes(rects, output_width, output_height)? >= full_copy_bytes =>
        {
            full_rect = [NativeDamageRect {
                x: 0,
                y: 0,
                width: output_width,
                height: output_height,
            }];
            &full_rect[..]
        }
        NativeFrameCopyDamage::Rects(rects) => rects,
    };

    let mut copied = 0usize;
    for rect in damage_rects {
        let Some(rect) = rect.clipped_to_output(width, height as u32) else {
            continue;
        };
        let left = usize::try_from(rect.x)
            .map_err(|_| io::Error::other("native framebuffer damage x overflow"))?;
        let top = usize::try_from(rect.y)
            .map_err(|_| io::Error::other("native framebuffer damage y overflow"))?;
        let rect_width = usize::try_from(rect.width)
            .map_err(|_| io::Error::other("native framebuffer damage width overflow"))?;
        let rect_height = usize::try_from(rect.height)
            .map_err(|_| io::Error::other("native framebuffer damage height overflow"))?;
        let rect_row_bytes = rect_width
            .checked_mul(mem::size_of::<u32>())
            .ok_or_else(|| io::Error::other("native framebuffer damage row overflow"))?;

        for y in top..top.saturating_add(rect_height) {
            let dst_start = y
                .checked_mul(pitch)
                .and_then(|value| value.checked_add(left.saturating_mul(mem::size_of::<u32>())))
                .ok_or_else(|| io::Error::other("native framebuffer pitch overflow"))?;
            let dst_end = dst_start
                .checked_add(rect_row_bytes)
                .ok_or_else(|| io::Error::other("native framebuffer row overflow"))?;
            let Some(dst_row) = bytes.get_mut(dst_start..dst_end) else {
                return Err(io::Error::other("native framebuffer mapping is too small"));
            };
            let src_start = y
                .checked_mul(row_bytes)
                .and_then(|value| value.checked_add(left.saturating_mul(mem::size_of::<u32>())))
                .ok_or_else(|| io::Error::other("native framebuffer source overflow"))?;
            let src_end = src_start
                .checked_add(rect_row_bytes)
                .ok_or_else(|| io::Error::other("native framebuffer source overflow"))?;
            dst_row.copy_from_slice(&frame_bytes[src_start..src_end]);
            copied = copied.saturating_add(rect_row_bytes);
        }
    }
    Ok(copied)
}

fn damage_rect_copy_bytes(
    rects: &[NativeDamageRect],
    output_width: u32,
    output_height: u32,
) -> io::Result<usize> {
    let mut bytes = 0usize;
    for rect in rects {
        let Some(rect) = rect.clipped_to_output(output_width, output_height) else {
            continue;
        };
        let rect_width = usize::try_from(rect.width)
            .map_err(|_| io::Error::other("native framebuffer damage width overflow"))?;
        let rect_height = usize::try_from(rect.height)
            .map_err(|_| io::Error::other("native framebuffer damage height overflow"))?;
        let rect_bytes = rect_width
            .checked_mul(rect_height)
            .and_then(|pixels| pixels.checked_mul(mem::size_of::<u32>()))
            .ok_or_else(|| io::Error::other("native framebuffer damage byte overflow"))?;
        bytes = bytes
            .checked_add(rect_bytes)
            .ok_or_else(|| io::Error::other("native framebuffer damage byte overflow"))?;
    }
    Ok(bytes)
}

fn host_display_variables_available() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var_os("WAYLAND_SOCKET").is_some()
        || std::env::var_os("DISPLAY").is_some()
}

fn native_scanout_forced() -> bool {
    std::env::var_os("OBLIVION_ONE_NATIVE_SCANOUT").is_some_and(|value| value == "1")
}

fn native_test_fail_native_egl_gbm_enabled() -> bool {
    if !cfg!(any(test, debug_assertions)) {
        return false;
    }
    std::env::var_os("OBLIVION_ONE_TEST_FAIL_NATIVE_EGL_GBM").is_some_and(|value| value == "1")
}

fn connected_connector_for_card(
    kms_device: Option<&Path>,
    sysfs_drm_root: &Path,
) -> Option<NativeConnector> {
    let card_name = kms_device
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())?;
    let mut connectors = fs::read_dir(sysfs_drm_root)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| connected_connector_from_entry(card_name, entry.path()))
        .collect::<Vec<_>>();
    connectors.sort_by(|left, right| left.name.cmp(&right.name));
    connectors.into_iter().next()
}

fn connected_connector_from_entry(card_name: &str, path: PathBuf) -> Option<NativeConnector> {
    let name = path.file_name()?.to_str()?.to_string();
    if !name.starts_with(&format!("{card_name}-")) {
        return None;
    }

    let status = read_trimmed(path.join("status"))?;
    if status != "connected" {
        return None;
    }

    let modes = fs::read_to_string(path.join("modes"))
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();

    Some(NativeConnector {
        name,
        enabled: read_trimmed(path.join("enabled")),
        modes,
        vrr_capable: read_bool_property(path.join("vrr_capable")),
    })
}

fn read_bool_property(path: impl AsRef<Path>) -> Option<bool> {
    match read_trimmed(path)?.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" | "enabled" => Some(true),
        "0" | "false" | "no" | "off" | "disabled" => Some(false),
        _ => None,
    }
}

fn read_trimmed(path: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|contents| contents.trim().to_string())
        .filter(|contents| !contents.is_empty())
}

fn display_optional_path(path: Option<&Path>) -> String {
    path.map(|path| path.display().to_string())
        .unwrap_or_else(|| "missing".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oblivion_one::compositor::{
        DesktopVisualState, RenderableSurfaceDamage, SurfaceDamageRect, SurfacePlacement,
        compose_nested_output, render_scene_elements_for_surfaces, surface_origins,
    };
    use oblivion_one::render_backend::buffer::{BufferSize, CommittedSurfaceBuffer};
    use oblivion_one::{CompositorAppGpuPreference, EffectiveCompositorAppGpuPolicy};

    #[test]
    fn connected_connector_for_card_prefers_connected_matching_card_output() {
        let root = std::env::current_dir()
            .unwrap()
            .join("target")
            .join("native-output-tests")
            .join(std::process::id().to_string());
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("card1-DP-1")).unwrap();
        fs::create_dir_all(root.join("card1-HDMI-A-1")).unwrap();
        fs::create_dir_all(root.join("card0-DP-1")).unwrap();
        fs::write(root.join("card1-DP-1/status"), "connected\n").unwrap();
        fs::write(root.join("card1-DP-1/enabled"), "enabled\n").unwrap();
        fs::write(root.join("card1-DP-1/modes"), "1920x1080\n1280x720\n").unwrap();
        fs::write(root.join("card1-DP-1/vrr_capable"), "1\n").unwrap();
        fs::write(root.join("card1-HDMI-A-1/status"), "disconnected\n").unwrap();
        fs::write(root.join("card0-DP-1/status"), "connected\n").unwrap();
        fs::write(root.join("card0-DP-1/modes"), "800x600\n").unwrap();

        let connector = connected_connector_for_card(Some(Path::new("/dev/dri/card1")), &root)
            .expect("connected card1 output should be detected");
        let _ = fs::remove_dir_all(&root);

        assert_eq!(connector.name, "card1-DP-1");
        assert_eq!(connector.enabled.as_deref(), Some("enabled"));
        assert_eq!(connector.preferred_mode(), Some("1920x1080"));
        assert_eq!(connector.vrr_capable, Some(true));
    }

    #[test]
    fn matching_render_node_for_card_uses_same_drm_device_directory() {
        let root = std::env::current_dir()
            .unwrap()
            .join("target")
            .join("native-render-node-tests")
            .join(std::process::id().to_string());
        let sysfs = root.join("sys");
        let dri = root.join("dev").join("dri");
        let drm_dir = sysfs.join("card2").join("device").join("drm");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&drm_dir).unwrap();
        fs::create_dir_all(&dri).unwrap();
        fs::create_dir_all(drm_dir.join("renderD130")).unwrap();
        fs::create_dir_all(drm_dir.join("card2")).unwrap();

        let render = matching_render_node_for_card(Path::new("/dev/dri/card2"), &sysfs, &dri);
        let _ = fs::remove_dir_all(&root);

        assert_eq!(render, Some(dri.join("renderD130")));
    }

    #[test]
    fn native_vrr_preference_parses_policy_values() {
        assert_eq!(
            NativeVrrPreference::parse("auto"),
            NativeVrrPreference::Auto
        );
        assert_eq!(NativeVrrPreference::parse("1"), NativeVrrPreference::On);
        assert_eq!(NativeVrrPreference::parse("true"), NativeVrrPreference::On);
        assert_eq!(NativeVrrPreference::parse("0"), NativeVrrPreference::Off);
        assert_eq!(
            NativeVrrPreference::parse("false"),
            NativeVrrPreference::Off
        );
        assert_eq!(
            NativeVrrPreference::parse("unknown"),
            NativeVrrPreference::Auto
        );
    }

    #[test]
    fn native_vrr_plan_auto_enables_only_when_connector_is_capable() {
        assert_eq!(
            NativeVrrPlan::choose(NativeVrrPreference::Auto, Some(true)),
            NativeVrrPlan {
                requested: NativeVrrPreference::Auto,
                supported: true,
                planned_enabled: true,
            }
        );
        assert_eq!(
            NativeVrrPlan::choose(NativeVrrPreference::Auto, Some(false)),
            NativeVrrPlan {
                requested: NativeVrrPreference::Auto,
                supported: false,
                planned_enabled: false,
            }
        );
    }

    #[test]
    fn drm_mode_name_reads_nul_terminated_kernel_mode_name() {
        let mut mode = drm_sys::drm_mode_modeinfo::default();
        for (index, byte) in b"2560x1440\0ignored".iter().enumerate() {
            mode.name[index] = *byte as _;
        }

        assert_eq!(drm_mode_name(&mode), "2560x1440");
    }

    #[test]
    fn native_perf_log_env_accepts_truthy_values() {
        assert!(native_perf_log_value_enabled("1"));
        assert!(native_perf_log_value_enabled("true"));
        assert!(native_perf_log_value_enabled("debug"));
        assert!(!native_perf_log_value_enabled("0"));
        assert!(!native_perf_log_value_enabled("false"));
        assert!(!native_perf_log_value_enabled(""));
    }

    #[test]
    fn native_perf_line_formats_structured_fields() {
        let line = native_perf_line(
            "app.spawn",
            &[
                NativePerfField::str("program", "zen browser"),
                NativePerfField::u64("pid", 4242),
                NativePerfField::str("app_policy", "accelerated"),
            ],
        );

        assert_eq!(
            line,
            "perf app.spawn program=\"zen browser\" pid=4242 app_policy=accelerated"
        );
    }

    #[test]
    fn native_app_gpu_preference_parses_explicit_values() {
        assert_eq!(
            CompositorAppGpuPreference::from_native_env_value(None),
            CompositorAppGpuPreference::Auto
        );
        assert_eq!(
            CompositorAppGpuPreference::parse("accelerated"),
            CompositorAppGpuPreference::Accelerated
        );
        assert_eq!(
            CompositorAppGpuPreference::parse("gpu"),
            CompositorAppGpuPreference::Accelerated
        );
        assert_eq!(
            CompositorAppGpuPreference::parse("auto"),
            CompositorAppGpuPreference::Auto
        );
        assert_eq!(
            CompositorAppGpuPreference::parse("cpu"),
            CompositorAppGpuPreference::CpuOnly
        );
        assert_eq!(
            CompositorAppGpuPreference::parse("software"),
            CompositorAppGpuPreference::CpuOnly
        );
        assert_eq!(
            CompositorAppGpuPreference::parse("unknown"),
            CompositorAppGpuPreference::Auto
        );
    }

    #[test]
    fn native_app_gpu_policy_resolves_from_active_scanout_backend() {
        assert_eq!(
            resolve_native_app_gpu_policy(
                CompositorAppGpuPreference::Auto,
                NativeScanoutKind::NativeEglGbm,
            )
            .unwrap(),
            EffectiveCompositorAppGpuPolicy::Accelerated
        );
        assert_eq!(
            resolve_native_app_gpu_policy(
                CompositorAppGpuPreference::Auto,
                NativeScanoutKind::GbmCpuWritePageFlip,
            )
            .unwrap(),
            EffectiveCompositorAppGpuPolicy::CpuOnly
        );
        assert_eq!(
            resolve_native_app_gpu_policy(
                CompositorAppGpuPreference::CpuOnly,
                NativeScanoutKind::NativeEglGbm,
            )
            .unwrap(),
            EffectiveCompositorAppGpuPolicy::CpuOnly
        );
        assert!(
            resolve_native_app_gpu_policy(
                CompositorAppGpuPreference::Accelerated,
                NativeScanoutKind::DumbFramebuffer,
            )
            .is_err()
        );
    }

    #[test]
    fn native_launch_request_ignores_empty_command() {
        assert_eq!(
            native_launch_request(
                Vec::new(),
                EffectiveCompositorAppGpuPolicy::CpuOnly,
                NativeLaunchSource::Startup,
            ),
            None
        );
    }

    #[test]
    fn native_launch_request_preserves_args_policy_and_source() {
        let request = native_launch_request(
            vec![
                "kitty".to_string(),
                "--title".to_string(),
                "two words".to_string(),
            ],
            EffectiveCompositorAppGpuPolicy::Accelerated,
            NativeLaunchSource::Startup,
        )
        .unwrap();

        assert_eq!(request.program, "kitty");
        assert_eq!(request.command, "kitty --title 'two words'");
        assert_eq!(request.argv[2], "two words");
        assert_eq!(
            request.gpu_policy,
            EffectiveCompositorAppGpuPolicy::Accelerated
        );
        assert_eq!(request.source, NativeLaunchSource::Startup);
    }

    #[test]
    fn native_runtime_error_includes_stage_backend_frame_and_recovery_command() {
        let error = native_runtime_error(
            NativeRuntimeStage::Present,
            NativeScanoutKind::NativeEglGbm,
            42,
            1842,
            io::Error::other("page flip failed"),
        );
        let message = error.to_string();

        assert!(message.contains("fatal native GPU runtime error"));
        assert!(message.contains("stage=present"));
        assert!(message.contains("backend=native-egl-gbm"));
        assert!(message.contains("crtc=42"));
        assert!(message.contains("frame=1842"));
        assert!(message.contains("OBLIVION_ONE_SCANOUT_BACKEND=cpu"));
    }

    #[test]
    fn native_damage_accumulator_reports_full_surface_damage() {
        let surface = test_renderable_surface(1, 20, 10, 80, 40, RenderableSurfaceDamage::Full);
        let mut damage = NativeDamageAccumulator::for_output(200, 120);

        damage.add_surface(&surface, (20, 10));

        assert_eq!(
            damage.summary(),
            NativeDamageSummary {
                kind: NativeDamageKind::SurfaceDamage,
                rects: 1,
                pixels: 3_200,
            }
        );
    }

    #[test]
    fn native_damage_accumulator_maps_partial_surface_damage_to_output() {
        let surface = test_renderable_surface(
            2,
            0,
            0,
            100,
            50,
            RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
                x: 10,
                y: 5,
                width: 30,
                height: 20,
            }]),
        );
        let mut damage = NativeDamageAccumulator::for_output(200, 120);

        damage.add_surface(&surface, (72, 72));

        assert_eq!(
            damage.rects(),
            &[NativeDamageRect {
                x: 82,
                y: 77,
                width: 30,
                height: 20,
            }]
        );
        assert_eq!(damage.summary().pixels, 600);
    }

    #[test]
    fn native_damage_accumulator_maps_render_scene_element_damage_to_output() {
        let surface = test_renderable_surface(
            2,
            0,
            0,
            100,
            50,
            RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
                x: 10,
                y: 5,
                width: 30,
                height: 20,
            }]),
        );
        let elements = render_scene_elements_for_surfaces(std::slice::from_ref(&surface), 1.0);

        let damage = NativeDamageAccumulator::from_render_elements(200, 120, &elements);

        assert_eq!(
            damage.rects(),
            &[NativeDamageRect {
                x: 82,
                y: 77,
                width: 30,
                height: 20,
            }]
        );
    }

    #[test]
    fn native_damage_accumulator_clips_partial_surface_damage_to_output() {
        let surface = test_renderable_surface(
            3,
            0,
            0,
            80,
            40,
            RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
                x: 60,
                y: 20,
                width: 20,
                height: 20,
            }]),
        );
        let mut damage = NativeDamageAccumulator::for_output(100, 80);

        damage.add_surface(&surface, (90, 70));

        assert!(damage.rects().is_empty());
        assert_eq!(damage.summary().kind, NativeDamageKind::Empty);
    }

    #[test]
    fn native_damage_summary_full_output_fallback_counts_output_pixels() {
        assert_eq!(
            NativeOutputDamage::full_output(1920, 1080).summary(),
            NativeDamageSummary {
                kind: NativeDamageKind::FullOutput,
                rects: 1,
                pixels: 2_073_600,
            }
        );
    }

    #[test]
    fn native_output_damage_for_window_move_covers_old_and_new_surface_bounds() {
        let previous = test_renderable_surface(7, 0, 0, 120, 80, RenderableSurfaceDamage::Full);
        let current = test_renderable_surface(7, 200, 100, 120, 80, RenderableSurfaceDamage::Full);
        let previous_origin = surface_origins(std::slice::from_ref(&previous))[0];
        let current_origin = surface_origins(std::slice::from_ref(&current))[0];

        let damage = native_output_damage_for_repaint(
            400,
            300,
            std::slice::from_ref(&previous),
            std::slice::from_ref(&current),
            RenderGenerationCause::WindowMove,
            true,
        );

        assert_eq!(damage.kind, NativeDamageKind::SurfaceDamage);
        assert_eq!(
            damage.rects,
            vec![
                NativeDamageRect {
                    x: previous_origin.0,
                    y: previous_origin.1,
                    width: 120,
                    height: 80,
                },
                NativeDamageRect {
                    x: current_origin.0,
                    y: current_origin.1,
                    width: 120,
                    height: 80,
                },
            ]
        );
    }

    #[test]
    fn native_damage_accumulator_render_element_bounds_changes_cover_old_and_new_targets() {
        let previous = test_renderable_surface(7, 0, 0, 120, 80, RenderableSurfaceDamage::Full);
        let current = test_renderable_surface(7, 200, 100, 120, 80, RenderableSurfaceDamage::Full);
        let previous_elements =
            render_scene_elements_for_surfaces(std::slice::from_ref(&previous), 1.0);
        let current_elements =
            render_scene_elements_for_surfaces(std::slice::from_ref(&current), 1.0);

        let damage = NativeDamageAccumulator::from_render_element_bounds_changes(
            400,
            300,
            &previous_elements,
            &current_elements,
        );

        assert_eq!(
            damage.rects(),
            &[
                NativeDamageRect {
                    x: 72,
                    y: 72,
                    width: 120,
                    height: 80,
                },
                NativeDamageRect {
                    x: 272,
                    y: 172,
                    width: 120,
                    height: 80,
                },
            ]
        );
    }

    #[test]
    fn native_output_damage_for_window_resize_covers_rescaled_bounds() {
        let previous = test_renderable_surface(7, 0, 0, 300, 200, RenderableSurfaceDamage::Full);
        let current = RenderableSurface {
            width: 340,
            height: 230,
            resize_preview: Some(oblivion_one::compositor::ResizePreview {
                committed_width: 300,
                committed_height: 200,
                anchor_right: false,
                anchor_bottom: false,
            }),
            ..test_renderable_surface(7, 0, 0, 300, 200, RenderableSurfaceDamage::Full)
        };
        let origin = surface_origins(std::slice::from_ref(&previous))[0];

        let damage = native_output_damage_for_repaint(
            640,
            480,
            std::slice::from_ref(&previous),
            std::slice::from_ref(&current),
            RenderGenerationCause::WindowResize,
            true,
        );

        assert_eq!(damage.kind, NativeDamageKind::SurfaceDamage);
        assert_eq!(
            damage.rects,
            vec![NativeDamageRect {
                x: origin.0,
                y: origin.1,
                width: 340,
                height: 230,
            }]
        );
    }

    #[test]
    fn native_output_damage_for_surface_commit_bounds_change_covers_old_and_new_bounds() {
        let previous = test_renderable_surface(7, 0, 0, 300, 200, RenderableSurfaceDamage::Full);
        let current = RenderableSurface {
            width: 260,
            height: 200,
            placement: SurfacePlacement::root_at(40, 0),
            damage: RenderableSurfaceDamage::Full,
            ..test_renderable_surface(7, 0, 0, 300, 200, RenderableSurfaceDamage::Full)
        };
        let previous_origin = surface_origins(std::slice::from_ref(&previous))[0];

        let damage = native_output_damage_for_repaint(
            640,
            480,
            std::slice::from_ref(&previous),
            std::slice::from_ref(&current),
            RenderGenerationCause::SurfaceCommit,
            true,
        );

        assert_eq!(damage.kind, NativeDamageKind::SurfaceDamage);
        assert_eq!(
            damage.rects,
            vec![NativeDamageRect {
                x: previous_origin.0,
                y: previous_origin.1,
                width: 300,
                height: 200,
            }]
        );
    }

    #[test]
    fn native_output_damage_forces_full_copy_after_full_scene_rebuild() {
        let rects = [NativeDamageRect {
            x: 10,
            y: 12,
            width: 20,
            height: 24,
        }];
        let damage = NativeOutputDamage::surface_damage(rects.to_vec());

        assert!(matches!(
            damage.frame_copy_damage_for_scene(DesktopSceneRebuildKind::Full),
            NativeFrameCopyDamage::Full
        ));
        assert!(matches!(
            damage.frame_copy_damage_for_scene(DesktopSceneRebuildKind::Partial),
            NativeFrameCopyDamage::Rects(partial) if partial == rects
        ));
    }

    fn test_renderable_surface(
        surface_id: u32,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        damage: RenderableSurfaceDamage,
    ) -> RenderableSurface {
        let size = BufferSize::new(width, height).expect("test surface size must be non-zero");
        RenderableSurface {
            surface_id,
            x,
            y,
            width,
            height,
            placement: SurfacePlacement::root(),
            resize_preview: None,
            generation: 0,
            buffer: CommittedSurfaceBuffer::shm_snapshot(
                size,
                vec![0; width as usize * height as usize],
            ),
            damage,
        }
    }

    #[test]
    fn proc_stat_cpu_parser_reads_user_and_system_ticks_after_comm() {
        let stat = "1234 (oblivion one) S 1 2 3 4 5 6 7 8 9 10 123 45 0 0 20 0";

        assert_eq!(
            parse_proc_stat_cpu_ticks(stat),
            Some(NativeProcessCpuSample {
                user_ticks: 123,
                system_ticks: 45,
            })
        );
    }

    #[test]
    fn kms_mode_preference_parses_exact_resolution_and_refresh() {
        assert_eq!(
            NativeModePreference::parse("1920x1080@165"),
            NativeModePreference::Exact {
                width: 1920,
                height: 1080,
                refresh_hz: Some(165),
            }
        );
    }

    #[test]
    fn kms_mode_selection_prefers_exact_refresh_when_available() {
        let modes = [
            test_drm_mode(1920, 1080, 60),
            test_drm_mode(2560, 1440, 144),
            test_drm_mode(1920, 1080, 165),
        ];

        let selected = select_kms_mode(
            &modes,
            NativeModePreference::Exact {
                width: 1920,
                height: 1080,
                refresh_hz: Some(165),
            },
        )
        .expect("exact mode should be selected");

        assert_eq!(mode_tuple(&selected), (1920, 1080, 165));
    }

    #[test]
    fn kms_mode_selection_uses_nearest_refresh_for_exact_resolution() {
        let modes = [
            test_drm_mode(1920, 1080, 60),
            test_drm_mode(1920, 1080, 144),
            test_drm_mode(2560, 1440, 75),
        ];

        let selected = select_kms_mode(
            &modes,
            NativeModePreference::Exact {
                width: 1920,
                height: 1080,
                refresh_hz: Some(165),
            },
        )
        .expect("nearest refresh should be selected");

        assert_eq!(mode_tuple(&selected), (1920, 1080, 144));
    }

    #[test]
    fn kms_mode_selection_highrr_prioritizes_refresh_then_resolution() {
        let modes = [
            test_drm_mode(3840, 2160, 60),
            test_drm_mode(2560, 1440, 144),
            test_drm_mode(1920, 1080, 165),
        ];

        let selected =
            select_kms_mode(&modes, NativeModePreference::HighRefresh).expect("mode selected");

        assert_eq!(mode_tuple(&selected), (1920, 1080, 165));
    }

    #[test]
    fn kms_mode_selection_auto_prioritizes_resolution_then_refresh() {
        let modes = [
            test_drm_mode(1920, 1080, 165),
            test_drm_mode(2560, 1440, 60),
            test_drm_mode(2560, 1440, 144),
        ];

        let selected = select_kms_mode(&modes, NativeModePreference::Auto).expect("mode selected");

        assert_eq!(mode_tuple(&selected), (2560, 1440, 144));
    }

    #[test]
    fn select_crtc_prefers_encoder_current_crtc_when_available() {
        let encoder = drm_sys::drm_mode_get_encoder {
            crtc_id: 42,
            possible_crtcs: 0b010,
            ..Default::default()
        };

        assert_eq!(select_crtc_id(&[12, 42, 77], &encoder), Some(42));
    }

    #[test]
    fn select_crtc_falls_back_to_possible_crtc_bitset() {
        let encoder = drm_sys::drm_mode_get_encoder {
            crtc_id: 0,
            possible_crtcs: 0b100,
            ..Default::default()
        };

        assert_eq!(select_crtc_id(&[12, 42, 77], &encoder), Some(77));
    }

    fn test_drm_mode(width: u16, height: u16, refresh_hz: u32) -> drm_sys::drm_mode_modeinfo {
        drm_sys::drm_mode_modeinfo {
            hdisplay: width,
            vdisplay: height,
            vrefresh: refresh_hz,
            ..Default::default()
        }
    }

    fn mode_tuple(mode: &drm_sys::drm_mode_modeinfo) -> (u16, u16, u32) {
        (mode.hdisplay, mode.vdisplay, mode.vrefresh)
    }

    #[test]
    fn native_drm_backend_plan_prefers_libseat_when_available() {
        let plan = NativeDrmBackendPlan::choose(NativeDrmBackendChoice {
            preference: NativeDrmBackendPreference::Auto,
            seat_available: true,
        });

        assert_eq!(plan.primary, NativeDrmBackendKind::Libseat);
        assert_eq!(plan.fallbacks, vec![NativeDrmBackendKind::Direct]);
    }

    #[test]
    fn native_drm_backend_plan_uses_direct_without_seat() {
        let plan = NativeDrmBackendPlan::choose(NativeDrmBackendChoice {
            preference: NativeDrmBackendPreference::Auto,
            seat_available: false,
        });

        assert_eq!(plan.primary, NativeDrmBackendKind::Direct);
        assert!(plan.fallbacks.is_empty());
    }

    #[test]
    fn native_drm_backend_plan_can_force_libseat() {
        let plan = NativeDrmBackendPlan::choose(NativeDrmBackendChoice {
            preference: NativeDrmBackendPreference::Libseat,
            seat_available: true,
        });

        assert_eq!(plan.primary, NativeDrmBackendKind::Libseat);
        assert!(plan.fallbacks.is_empty());
    }

    #[test]
    fn native_drm_backend_plan_rejects_forced_libseat_without_seat() {
        let plan = NativeDrmBackendPlan::choose(NativeDrmBackendChoice {
            preference: NativeDrmBackendPreference::Libseat,
            seat_available: false,
        });

        assert_eq!(plan.primary, NativeDrmBackendKind::Unavailable);
        assert!(plan.fallbacks.is_empty());
    }

    #[test]
    fn native_scanout_plan_prefers_native_egl_gbm_when_ready() {
        let plan = NativeScanoutPlan::choose(NativeScanoutChoice {
            preference: NativeScanoutPreference::Auto,
            gbm_available: true,
            egl_available: true,
            page_flip_available: true,
        });

        assert_eq!(plan.primary, NativeScanoutKind::NativeEglGbm);
        assert_eq!(
            plan.fallbacks,
            vec![
                NativeScanoutKind::GbmCpuWritePageFlip,
                NativeScanoutKind::DumbFramebuffer
            ]
        );
    }

    #[test]
    fn native_scanout_plan_can_force_gpu_without_cpu_fallback() {
        let plan = NativeScanoutPlan::choose(NativeScanoutChoice {
            preference: NativeScanoutPreference::NativeEglGbm,
            gbm_available: true,
            egl_available: true,
            page_flip_available: true,
        });

        assert_eq!(plan.primary, NativeScanoutKind::NativeEglGbm);
        assert!(plan.fallbacks.is_empty());
    }

    #[test]
    fn native_scanout_plan_rejects_forced_gpu_without_egl() {
        let plan = NativeScanoutPlan::choose(NativeScanoutChoice {
            preference: NativeScanoutPreference::NativeEglGbm,
            gbm_available: true,
            egl_available: false,
            page_flip_available: true,
        });

        assert_eq!(plan.primary, NativeScanoutKind::Unavailable);
        assert!(plan.fallbacks.is_empty());
    }

    #[test]
    fn native_scanout_plan_fallback_after_gpu_failure_preserves_remaining_candidates() {
        let plan = NativeScanoutPlan::choose(NativeScanoutChoice {
            preference: NativeScanoutPreference::Auto,
            gbm_available: true,
            egl_available: true,
            page_flip_available: true,
        })
        .after_failed(NativeScanoutKind::NativeEglGbm);

        assert_eq!(plan.primary, NativeScanoutKind::GbmCpuWritePageFlip);
        assert_eq!(plan.fallbacks, vec![NativeScanoutKind::DumbFramebuffer]);
    }

    #[test]
    fn native_scanout_kind_names_cpu_write_gbm_backend_honestly() {
        assert_eq!(
            NativeScanoutKind::GbmCpuWritePageFlip.as_str(),
            "GBM CPU-write pageflip"
        );
    }

    #[test]
    fn injected_native_egl_gbm_open_failure_returns_clear_error_before_kms_use() {
        let previous = std::env::var_os("OBLIVION_ONE_TEST_FAIL_NATIVE_EGL_GBM");
        unsafe {
            std::env::set_var("OBLIVION_ONE_TEST_FAIL_NATIVE_EGL_GBM", "1");
        }
        let file = fs::File::open("Cargo.toml").unwrap();

        let error =
            match NativeScanoutBackend::open_kind(NativeScanoutKind::NativeEglGbm, &file, 1, 1) {
                Ok(_) => panic!("injected native EGL/GBM failure should fail before KMS use"),
                Err(error) => error,
            };

        match previous {
            Some(value) => unsafe {
                std::env::set_var("OBLIVION_ONE_TEST_FAIL_NATIVE_EGL_GBM", value);
            },
            None => unsafe {
                std::env::remove_var("OBLIVION_ONE_TEST_FAIL_NATIVE_EGL_GBM");
            },
        }
        assert!(
            error
                .to_string()
                .contains("OBLIVION_ONE_TEST_FAIL_NATIVE_EGL_GBM")
        );
    }

    #[test]
    fn auto_gpu_open_failure_next_cpu_candidate_resolves_apps_to_cpu() {
        let plan = NativeScanoutPlan::choose(NativeScanoutChoice {
            preference: NativeScanoutPreference::Auto,
            gbm_available: true,
            egl_available: true,
            page_flip_available: true,
        });

        let fallback = plan.after_failed(NativeScanoutKind::NativeEglGbm);

        assert_eq!(fallback.primary, NativeScanoutKind::GbmCpuWritePageFlip);
        assert_eq!(
            resolve_native_app_gpu_policy(CompositorAppGpuPreference::Auto, fallback.primary)
                .unwrap(),
            EffectiveCompositorAppGpuPolicy::CpuOnly
        );
    }

    #[test]
    fn native_scanout_preference_keeps_legacy_gbm_egl_alias_for_cpu_write_backend() {
        assert_eq!(
            NativeScanoutPreference::parse("gbm-egl"),
            NativeScanoutPreference::GbmCpuWritePageFlip
        );
    }

    #[test]
    fn native_scanout_preference_accepts_canonical_cpu_write_backend_name() {
        assert_eq!(
            NativeScanoutPreference::parse("gbm-cpu-write"),
            NativeScanoutPreference::GbmCpuWritePageFlip
        );
    }

    #[test]
    fn native_scanout_preference_accepts_canonical_gpu_backend_name() {
        assert_eq!(
            NativeScanoutPreference::parse("native-egl-gbm"),
            NativeScanoutPreference::NativeEglGbm
        );
        assert_eq!(
            NativeScanoutPreference::parse("gpu"),
            NativeScanoutPreference::NativeEglGbm
        );
    }

    #[test]
    fn native_scanout_plan_uses_cpu_gbm_fallback_without_egl() {
        let plan = NativeScanoutPlan::choose(NativeScanoutChoice {
            preference: NativeScanoutPreference::Auto,
            gbm_available: true,
            egl_available: false,
            page_flip_available: true,
        });

        assert_eq!(plan.primary, NativeScanoutKind::GbmCpuWritePageFlip);
        assert_eq!(plan.fallbacks, vec![NativeScanoutKind::DumbFramebuffer]);
    }

    #[test]
    fn native_pageflip_state_blocks_overlapping_flips() {
        let mut state = NativePageFlipState::default();

        assert!(state.can_schedule());
        let token = state.reserve_submission().unwrap();
        assert_ne!(token, 0);
        assert!(!state.can_schedule());
        assert_eq!(
            state.reserve_submission(),
            Err(NativePageFlipError::AlreadyPending)
        );
        assert_eq!(
            state.complete(token),
            NativePageFlipCompletion::Completed { token }
        );
        assert!(state.can_schedule());
        assert_eq!(
            state.complete(token),
            NativePageFlipCompletion::Stale { token }
        );
    }

    #[test]
    fn native_pageflip_state_rejects_mismatch_without_clearing_pending() {
        let mut state = NativePageFlipState::default();
        let expected = state.reserve_submission().unwrap();
        let received = next_nonzero_page_flip_token(expected);

        assert_eq!(
            state.complete(received),
            NativePageFlipCompletion::Mismatched { expected, received }
        );
        assert_eq!(state.pending_token(), Some(expected));
    }

    #[test]
    fn native_pageflip_state_stale_event_cannot_complete_new_submission() {
        let mut state = NativePageFlipState::default();
        let first = state.reserve_submission().unwrap();
        assert_eq!(
            state.complete(first),
            NativePageFlipCompletion::Completed { token: first }
        );
        let second = state.reserve_submission().unwrap();

        assert_eq!(
            state.complete(first),
            NativePageFlipCompletion::Mismatched {
                expected: second,
                received: first,
            }
        );
        assert_eq!(state.pending_token(), Some(second));
    }

    #[test]
    fn native_pageflip_token_wrap_skips_zero() {
        assert_eq!(next_nonzero_page_flip_token(u64::MAX), 1);
        assert_eq!(next_nonzero_page_flip_token(1), 2);
    }

    #[test]
    fn native_pageflip_token_does_not_restart_after_backend_recreation() {
        let mut first = NativePageFlipState::default();
        let old_token = first.reserve_submission().unwrap();
        assert_eq!(
            first.complete(old_token),
            NativePageFlipCompletion::Completed { token: old_token }
        );
        let mut replacement = NativePageFlipState::default();

        let replacement_token = replacement.reserve_submission().unwrap();

        assert_ne!(replacement_token, old_token);
    }

    #[test]
    fn native_pageflip_buffers_promote_ready_to_pending_to_current() {
        let mut buffers = NativePageFlipBuffers::default();

        buffers.set_ready(10);
        assert_eq!(buffers.ready_or_current(), Some(&10));
        assert_eq!(buffers.take_ready(), Some(10));
        buffers.set_pending(10);
        assert!(buffers.complete_page_flip());
        assert_eq!(buffers.ready_or_current(), Some(&10));

        assert!(!buffers.complete_page_flip());
    }

    #[test]
    fn native_pageflip_buffers_finish_initial_scanout_promotes_ready() {
        let mut buffers = NativePageFlipBuffers::default();

        buffers.set_ready(20);
        buffers.finish_initial_scanout();

        assert_eq!(buffers.ready_or_current(), Some(&20));
        assert_eq!(buffers.take_ready(), None);
    }

    #[test]
    fn native_initial_frame_paints_real_topbar() {
        let mut renderer = NativeFrameRenderer::default();
        let spotlight = SpotlightModel::default();
        let frame = renderer
            .render_frame(NativeFrameRequest {
                width: 320,
                height: 200,
                surfaces: &[],
                dock_items: Vec::new(),
                spotlight: &spotlight,
                shell_generation: 0,
                visual_state: DesktopVisualState::wallpaper_only(),
                render_generation: 0,
                client_cursor: None,
            })
            .pixels
            .to_vec();
        let mut wallpaper = vec![0; 320 * 200];
        compose_nested_output(
            &mut wallpaper,
            320,
            200,
            &[],
            DesktopVisualState::wallpaper_only(),
        );

        assert_ne!(frame[16 * 320 + 160], wallpaper[16 * 320 + 160]);
    }

    #[test]
    fn native_initial_frame_does_not_draw_closed_spotlight_panel() {
        let mut renderer = NativeFrameRenderer::default();
        let spotlight = SpotlightModel::default();
        let frame = renderer
            .render_frame(NativeFrameRequest {
                width: 320,
                height: 200,
                surfaces: &[],
                dock_items: Vec::new(),
                spotlight: &spotlight,
                shell_generation: 0,
                visual_state: DesktopVisualState::wallpaper_only(),
                render_generation: 0,
                client_cursor: None,
            })
            .pixels
            .to_vec();
        let mut wallpaper = vec![0; 320 * 200];
        compose_nested_output(
            &mut wallpaper,
            320,
            200,
            &[],
            DesktopVisualState::wallpaper_only(),
        );

        assert_eq!(frame[100 * 320 + 160], wallpaper[100 * 320 + 160]);
    }

    #[test]
    fn native_input_ctrl_space_opens_spotlight_without_forwarding_space() {
        let mut input = NativeInputState::new(320, 200);

        let ctrl = input.handle_key_event(KEY_LEFTCTRL, 1);
        let space = input.handle_key_event(KEY_SPACE, 1);

        assert_eq!(
            ctrl.keyboard_events,
            vec![NativeKeyboardEvent::new(KEY_LEFTCTRL, true)]
        );
        assert!(space.redraw_requested);
        assert_eq!(
            space.keyboard_events,
            vec![NativeKeyboardEvent::new(KEY_LEFTCTRL, false)]
        );
        assert!(space.requires_frame_repaint(NativeCursorRenderMode::Hardware));
        assert!(input.spotlight_visible());
    }

    #[test]
    fn native_input_visible_spotlight_collects_typed_letters() {
        let mut input = NativeInputState::new(320, 200);
        input.handle_key_event(KEY_LEFTCTRL, 1);
        input.handle_key_event(KEY_SPACE, 1);

        let effect = input.handle_key_event(KEY_Z, 1);

        assert!(effect.redraw_requested);
        assert!(effect.keyboard_events.is_empty());
        assert_eq!(input.spotlight_query(), "z");
    }

    #[test]
    fn spotlight_motion_updates_visual_cursor_without_forwarding_client_motion() {
        let mut input = NativeInputState::new(320, 200);
        input.handle_key_event(KEY_LEFTCTRL, 1);
        input.handle_key_event(KEY_SPACE, 1);

        let effect = input.handle_pointer_motion_delta(24.0, 12.0);

        assert_eq!(input.cursor_position(), (184, 112));
        assert_eq!(effect.cursor_position, Some((184, 112)));
        assert_eq!(effect.pointer_motion, None);
        assert_eq!(effect.relative_motion, None);
    }

    #[test]
    fn spotlight_locked_motion_does_not_leak_relative_or_absolute_motion() {
        let mut input = NativeInputState::new(320, 200);
        let anchor = input.cursor_position_f64();
        input.set_pointer_locked_at(anchor);
        input.handle_key_event(KEY_LEFTCTRL, 1);
        input.handle_key_event(KEY_SPACE, 1);

        let effect = input.handle_pointer_motion_delta(24.0, 12.0);

        assert_eq!(input.cursor_position_f64(), anchor);
        assert_eq!(effect.cursor_position, None);
        assert_eq!(effect.pointer_motion, None);
        assert_eq!(effect.relative_motion, None);
    }

    #[test]
    fn spotlight_client_cursor_repaints_even_with_hardware_cursor_mode() {
        let mut input = NativeInputState::new(320, 200);
        input.handle_key_event(KEY_LEFTCTRL, 1);
        input.handle_key_event(KEY_SPACE, 1);
        let effect = input.handle_pointer_motion_delta(24.0, 12.0);
        let mut updated_position = None;

        let compositor_visual_changed = apply_compositor_only_pointer_position(&effect, |x, y| {
            updated_position = Some((x, y));
            true
        });
        let redraw_requested = effect.requires_frame_repaint(NativeCursorRenderMode::Hardware)
            || compositor_visual_changed;

        assert_eq!(updated_position, Some((184.0, 112.0)));
        assert!(redraw_requested);
    }

    #[test]
    fn ordinary_motion_does_not_apply_compositor_only_position_update() {
        let mut input = NativeInputState::new(320, 200);
        let effect = input.handle_pointer_motion_delta(24.0, 12.0);

        let compositor_visual_changed = apply_compositor_only_pointer_position(&effect, |_, _| {
            panic!("ordinary forwarded motion must not update position twice")
        });

        assert_eq!(effect.pointer_motion, Some((184.0, 112.0)));
        assert!(!compositor_visual_changed);
    }

    #[test]
    fn native_input_alt_p_requests_session_exit_without_forwarding_p() {
        let mut input = NativeInputState::new(320, 200);

        input.handle_key_event(KEY_LEFTALT, 1);
        let p = input.handle_key_event(KEY_P, 1);

        assert!(p.exit_requested);
        assert!(p.keyboard_events.is_empty());
    }

    #[test]
    fn native_input_ctrl_c_is_forwarded_to_clients() {
        let mut input = NativeInputState::new(320, 200);

        let ctrl = input.handle_key_event(KEY_LEFTCTRL, 1);
        let c = input.handle_key_event(KEY_C, 1);

        assert_eq!(
            ctrl.keyboard_events,
            vec![NativeKeyboardEvent::new(KEY_LEFTCTRL, true)]
        );
        assert!(!c.exit_requested);
        assert_eq!(
            c.keyboard_events,
            vec![NativeKeyboardEvent::new(KEY_C, true)]
        );
    }

    #[test]
    fn native_input_alt_mouse_buttons_start_window_interactions() {
        let mut input = NativeInputState::new(320, 200);
        input.handle_key_event(KEY_LEFTALT, 1);

        let move_start = input.handle_pointer_button(u32::from(BTN_LEFT), true);
        let motion = input.handle_pointer_motion_delta(24.0, 12.0);
        let move_end = input.handle_pointer_button(u32::from(BTN_LEFT), false);

        assert_eq!(
            move_start.window_actions,
            vec![NativeWindowAction::BeginMove { x: 160.0, y: 100.0 }]
        );
        assert!(move_start.pointer_buttons.is_empty());
        assert_eq!(
            motion.window_actions,
            vec![NativeWindowAction::UpdateInteraction { x: 184.0, y: 112.0 }]
        );
        assert!(motion.pointer_motion.is_none());
        assert_eq!(
            move_end.window_actions,
            vec![NativeWindowAction::EndInteraction]
        );
    }

    #[test]
    fn native_input_unlocked_relative_motion_moves_cursor_and_preserves_relative_delta() {
        let mut input = NativeInputState::new(320, 200);

        let effect = input.handle_pointer_motion_delta(24.0, 12.0);

        assert_eq!(input.cursor_position(), (184, 112));
        assert_eq!(
            effect.relative_motion,
            Some(RelativeMotion::accelerated_only(24.0, 12.0))
        );
        assert_eq!(effect.pointer_motion, Some((184.0, 112.0)));
        assert_eq!(effect.cursor_position, Some((184, 112)));
    }

    #[test]
    fn native_input_locked_relative_motion_preserves_delta_without_moving_cursor() {
        let mut input = NativeInputState::new(320, 200);
        input.set_pointer_locked_at(input.cursor_position_f64());

        let effect = input.handle_pointer_motion_delta(24.0, 12.0);

        assert_eq!(input.cursor_position(), (160, 100));
        assert_eq!(
            effect.relative_motion,
            Some(RelativeMotion::accelerated_only(24.0, 12.0))
        );
        assert_eq!(effect.pointer_motion, None);
        assert_eq!(effect.cursor_position, None);
        assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Software));
        assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Hardware));
    }

    #[test]
    fn native_input_locked_absolute_motion_does_not_move_cursor() {
        let mut input = NativeInputState::new(320, 200);
        input.set_pointer_locked_at(input.cursor_position_f64());

        let effect = input.handle_pointer_motion(PointerMotionSample::absolute(7, 25.0, 26.0));

        assert_eq!(input.cursor_position(), (160, 100));
        assert_eq!(effect.pointer_motion, None);
        assert_eq!(effect.cursor_position, None);
    }

    #[test]
    fn locked_pointer_motion_never_accumulates_absolute_cursor_position() {
        let mut input = NativeInputState::new(800, 600);
        let anchor = CompositorOutputPosition {
            x: 400.25,
            y: 300.75,
        };
        input.restore_cursor_position(anchor);
        input.set_pointer_locked_at(anchor);

        let mut total_dx = 0.0;
        let mut total_dy = 0.0;
        for _ in 0..100 {
            let effect = input.handle_pointer_motion(PointerMotionSample::relative(
                10,
                RelativeMotion::accelerated_only(20.0, -15.0),
            ));
            total_dx += effect.relative_motion.unwrap().dx;
            total_dy += effect.relative_motion.unwrap().dy;
            assert_eq!(effect.pointer_motion, None);
            assert_eq!(effect.cursor_position, None);
        }

        assert_eq!(total_dx, 2000.0);
        assert_eq!(total_dy, -1500.0);
        assert_eq!(input.cursor_position_f64(), anchor);
    }

    #[test]
    fn native_input_unlock_restore_sets_logical_cursor_position() {
        let mut input = NativeInputState::new(320, 200);
        input.set_pointer_locked_at(input.cursor_position_f64());
        input.handle_pointer_motion_delta(200.0, 200.0);

        input.clear_pointer_constraint();
        let effect = input.restore_cursor_position(CompositorOutputPosition { x: 35.0, y: 45.0 });

        assert_eq!(input.cursor_position(), (35, 45));
        assert_eq!(effect.cursor_position, Some((35, 45)));
        assert!(effect.requires_frame_repaint(NativeCursorRenderMode::Software));
        assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Hardware));
    }

    #[test]
    fn native_input_first_real_move_after_unlock_starts_from_restored_position() {
        let mut input = NativeInputState::new(320, 200);
        input.set_pointer_locked_at(input.cursor_position_f64());
        input.handle_pointer_motion_delta(200.0, 200.0);
        input.clear_pointer_constraint();
        input.restore_cursor_position(CompositorOutputPosition { x: 35.0, y: 45.0 });

        let effect = input.handle_pointer_motion_delta(5.0, -10.0);

        assert_eq!(input.cursor_position(), (40, 35));
        assert_eq!(effect.pointer_motion, Some((40.0, 35.0)));
    }

    #[test]
    fn native_input_confined_relative_motion_clamps_to_region_and_keeps_absolute_motion() {
        let mut input = NativeInputState::new(320, 200);
        input.restore_cursor_position(CompositorOutputPosition { x: 50.0, y: 50.0 });
        input.set_pointer_confined(OutputRegion::from_rect(
            OutputRect::new(40.0, 30.0, 60.0, 40.0).unwrap(),
        ));

        let right = input.handle_pointer_motion_delta(100.0, 0.0);
        assert_eq!(input.cursor_position(), (99, 50));
        assert_eq!(right.pointer_motion, Some((99.0, 50.0)));
        assert_eq!(
            right.relative_motion,
            Some(RelativeMotion::accelerated_only(100.0, 0.0))
        );

        let bottom = input.handle_pointer_motion_delta(0.0, 100.0);
        assert_eq!(input.cursor_position(), (99, 69));
        assert_eq!(bottom.pointer_motion, Some((99.0, 69.0)));

        let left = input.handle_pointer_motion_delta(-100.0, 0.0);
        assert_eq!(input.cursor_position(), (40, 69));
        assert_eq!(left.pointer_motion, Some((40.0, 69.0)));

        let top = input.handle_pointer_motion_delta(0.0, -100.0);
        assert_eq!(input.cursor_position(), (40, 30));
        assert_eq!(top.pointer_motion, Some((40.0, 30.0)));
    }

    #[test]
    fn native_input_confined_absolute_motion_clamps_and_requests_cursor_repaint() {
        let mut input = NativeInputState::new(320, 200);
        input.set_pointer_confined(OutputRegion::from_rect(
            OutputRect::new(40.0, 30.0, 60.0, 40.0).unwrap(),
        ));

        let effect = input.handle_pointer_motion(PointerMotionSample::absolute(7, 500.0, 1.0));

        assert_eq!(input.cursor_position(), (99, 30));
        assert_eq!(effect.pointer_motion, Some((99.0, 30.0)));
        assert_eq!(effect.cursor_position, Some((99, 30)));
        assert!(effect.requires_frame_repaint(NativeCursorRenderMode::Software));
        assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Hardware));
    }

    #[test]
    fn native_pointer_constraint_backend_activates_locked_once() {
        let mut backend = NativePointerConstraintBackend::new();
        let id = PointerConstraintBackendId {
            constraint_id: 7,
            generation: 1,
        };

        let first = backend.handle_request(
            PointerConstraintBackendRequest::ActivateLocked {
                id,
                anchor: CompositorOutputPosition { x: 10.0, y: 20.0 },
            },
            CompositorOutputPosition { x: 10.0, y: 20.0 },
        );
        let duplicate = backend.handle_request(
            PointerConstraintBackendRequest::ActivateLocked {
                id,
                anchor: CompositorOutputPosition { x: 30.0, y: 40.0 },
            },
            CompositorOutputPosition { x: 30.0, y: 40.0 },
        );

        assert_eq!(
            first.activated,
            Some(NativePointerConstraint {
                id,
                mode: PointerConstraintMode::Locked,
                anchor: CompositorOutputPosition { x: 10.0, y: 20.0 },
                region: None,
            })
        );
        assert_eq!(duplicate, NativePointerConstraintBackendAction::default());
        assert!(backend.active_locked());
    }

    #[test]
    fn native_pointer_constraint_backend_activates_confined_with_region() {
        let mut backend = NativePointerConstraintBackend::new();
        let id = PointerConstraintBackendId {
            constraint_id: 8,
            generation: 1,
        };
        let region = OutputRegion::from_rect(OutputRect::new(10.0, 20.0, 100.0, 50.0).unwrap());

        let action = backend.handle_request(
            PointerConstraintBackendRequest::ActivateConfined {
                id,
                region: region.clone(),
            },
            CompositorOutputPosition { x: 10.0, y: 20.0 },
        );

        assert_eq!(
            action.activated,
            Some(NativePointerConstraint {
                id,
                mode: PointerConstraintMode::Confined,
                anchor: CompositorOutputPosition { x: 10.0, y: 20.0 },
                region: Some(region),
            })
        );
        assert!(action.failed.is_none());
    }

    #[test]
    fn native_pointer_constraint_backend_mismatched_deactivation_cannot_unlock_newer_lock() {
        let mut backend = NativePointerConstraintBackend::new();
        let active = PointerConstraintBackendId {
            constraint_id: 9,
            generation: 2,
        };
        let stale = PointerConstraintBackendId {
            constraint_id: 9,
            generation: 1,
        };
        backend.handle_request(
            PointerConstraintBackendRequest::ActivateLocked {
                id: active,
                anchor: CompositorOutputPosition { x: 10.0, y: 20.0 },
            },
            CompositorOutputPosition { x: 10.0, y: 20.0 },
        );

        let action = backend.handle_request(
            PointerConstraintBackendRequest::Deactivate {
                id: stale,
                restore_position: Some(CompositorOutputPosition { x: 40.0, y: 50.0 }),
            },
            CompositorOutputPosition { x: 99.0, y: 99.0 },
        );

        assert_eq!(action, NativePointerConstraintBackendAction::default());
        assert!(backend.active_locked());
    }

    #[test]
    fn native_pointer_constraint_backend_deactivation_restores_hint_or_anchor() {
        let mut backend = NativePointerConstraintBackend::new();
        let id = PointerConstraintBackendId {
            constraint_id: 10,
            generation: 1,
        };
        backend.handle_request(
            PointerConstraintBackendRequest::ActivateLocked {
                id,
                anchor: CompositorOutputPosition { x: 10.0, y: 20.0 },
            },
            CompositorOutputPosition { x: 10.0, y: 20.0 },
        );

        let action = backend.handle_request(
            PointerConstraintBackendRequest::Deactivate {
                id,
                restore_position: Some(CompositorOutputPosition { x: 30.0, y: 40.0 }),
            },
            CompositorOutputPosition { x: 99.0, y: 99.0 },
        );

        assert_eq!(action.deactivated, Some(id));
        assert_eq!(
            action.restore_position,
            Some(CompositorOutputPosition { x: 30.0, y: 40.0 })
        );
        assert!(!backend.active_locked());

        backend.handle_request(
            PointerConstraintBackendRequest::ActivateLocked {
                id,
                anchor: CompositorOutputPosition { x: 10.0, y: 20.0 },
            },
            CompositorOutputPosition { x: 10.0, y: 20.0 },
        );
        let action = backend.handle_request(
            PointerConstraintBackendRequest::Deactivate {
                id,
                restore_position: None,
            },
            CompositorOutputPosition { x: 99.0, y: 99.0 },
        );

        assert_eq!(
            action.restore_position,
            Some(CompositorOutputPosition { x: 10.0, y: 20.0 })
        );
    }

    #[test]
    fn native_pointer_constraint_backend_preserves_fractional_activation_anchor() {
        let mut backend = NativePointerConstraintBackend::new();
        let id = PointerConstraintBackendId {
            constraint_id: 12,
            generation: 3,
        };
        let anchor = CompositorOutputPosition {
            x: 400.25,
            y: 300.75,
        };

        backend.handle_request(
            PointerConstraintBackendRequest::ActivateLocked { id, anchor },
            anchor,
        );
        let action = backend.handle_request(
            PointerConstraintBackendRequest::Deactivate {
                id,
                restore_position: None,
            },
            CompositorOutputPosition { x: 0.0, y: 0.0 },
        );

        assert_eq!(action.restore_position, Some(anchor));
    }

    #[test]
    fn native_pointer_constraint_backend_confined_deactivation_does_not_restore_anchor() {
        let mut backend = NativePointerConstraintBackend::new();
        let id = PointerConstraintBackendId {
            constraint_id: 11,
            generation: 1,
        };
        backend.handle_request(
            PointerConstraintBackendRequest::ActivateConfined {
                id,
                region: OutputRegion::from_rect(OutputRect::new(10.0, 20.0, 100.0, 50.0).unwrap()),
            },
            CompositorOutputPosition { x: 30.0, y: 40.0 },
        );

        let action = backend.handle_request(
            PointerConstraintBackendRequest::Deactivate {
                id,
                restore_position: None,
            },
            CompositorOutputPosition { x: 99.0, y: 99.0 },
        );

        assert_eq!(action.deactivated, Some(id));
        assert_eq!(action.restore_position, None);
        assert!(!backend.active_locked());
    }

    #[test]
    fn native_pointer_constraint_backend_updates_confined_region_in_place() {
        let mut backend = NativePointerConstraintBackend::new();
        let id = PointerConstraintBackendId {
            constraint_id: 12,
            generation: 1,
        };
        backend.handle_request(
            PointerConstraintBackendRequest::ActivateConfined {
                id,
                region: OutputRegion::from_rect(OutputRect::new(10.0, 20.0, 100.0, 50.0).unwrap()),
            },
            CompositorOutputPosition { x: 30.0, y: 40.0 },
        );

        let action = backend.handle_request(
            PointerConstraintBackendRequest::UpdateConfinedRegion {
                id,
                region: OutputRegion::from_rect(OutputRect::new(40.0, 50.0, 20.0, 10.0).unwrap()),
            },
            CompositorOutputPosition { x: 30.0, y: 40.0 },
        );

        assert_eq!(action.deactivated, None);
        assert_eq!(action.activated, None);
        assert_eq!(
            action.cursor_position,
            Some(CompositorOutputPosition { x: 40.0, y: 50.0 })
        );
        assert_eq!(action.restore_position, None);
        assert_eq!(
            backend.active_constraint_state(),
            NativePointerConstraintState::Confined {
                region: OutputRegion::from_rect(OutputRect::new(40.0, 50.0, 20.0, 10.0).unwrap())
            }
        );
    }

    #[test]
    fn native_pointer_constraint_backend_tracks_cursor_visibility_changes() {
        let mut backend = NativePointerConstraintBackend::new();

        let hide = backend.handle_request(
            PointerConstraintBackendRequest::ApplyCursorVisibility { visible: false },
            CompositorOutputPosition::default(),
        );
        let duplicate_hide = backend.handle_request(
            PointerConstraintBackendRequest::ApplyCursorVisibility { visible: false },
            CompositorOutputPosition::default(),
        );
        let show = backend.handle_request(
            PointerConstraintBackendRequest::ApplyCursorVisibility { visible: true },
            CompositorOutputPosition::default(),
        );

        assert_eq!(hide.cursor_visibility_changed, Some(false));
        assert_eq!(
            duplicate_hide,
            NativePointerConstraintBackendAction::default()
        );
        assert_eq!(show.cursor_visibility_changed, Some(true));
    }

    #[test]
    fn native_input_alt_right_starts_window_resize() {
        let mut input = NativeInputState::new(320, 200);
        input.handle_key_event(KEY_RIGHTALT, 1);

        let effect = input.handle_pointer_button(u32::from(BTN_RIGHT), true);

        assert_eq!(
            effect.window_actions,
            vec![NativeWindowAction::BeginResize { x: 160.0, y: 100.0 }]
        );
        assert!(effect.pointer_buttons.is_empty());
    }

    #[test]
    fn native_input_alt_release_ends_active_window_interaction() {
        let mut input = NativeInputState::new(320, 200);
        input.handle_key_event(KEY_LEFTALT, 1);
        input.handle_pointer_button(u32::from(BTN_LEFT), true);

        let effect = input.handle_key_event(KEY_LEFTALT, 0);

        assert_eq!(
            effect.window_actions,
            vec![NativeWindowAction::EndInteraction]
        );
    }

    #[test]
    fn native_input_alt_keyboard_shortcuts_map_to_window_actions() {
        let mut input = NativeInputState::new(320, 200);
        input.handle_key_event(KEY_LEFTALT, 1);

        let minimize = input.handle_key_event(KEY_M, 1);
        let minimize_release = input.handle_key_event(KEY_M, 0);
        let restore = input.handle_key_event(KEY_R, 1);
        let maximize = input.handle_key_event(KEY_F, 1);
        let fullscreen = input.handle_key_event(KEY_F11, 1);

        assert_eq!(minimize.window_actions, vec![NativeWindowAction::Minimize]);
        assert!(minimize.keyboard_events.is_empty());
        assert!(minimize_release.keyboard_events.is_empty());
        assert_eq!(
            restore.window_actions,
            vec![NativeWindowAction::RestoreMinimized]
        );
        assert_eq!(
            maximize.window_actions,
            vec![NativeWindowAction::ToggleMaximize]
        );
        assert_eq!(
            fullscreen.window_actions,
            vec![NativeWindowAction::ToggleFullscreen]
        );
    }

    #[test]
    fn native_input_shortcut_inhibition_forwards_window_shortcuts_to_client() {
        let mut input = NativeInputState::new(320, 200);
        input.set_keyboard_shortcuts_inhibited(true);

        let alt = input.handle_key_event(KEY_LEFTALT, 1);
        let fullscreen = input.handle_key_event(KEY_F11, 1);

        assert_eq!(
            alt.keyboard_events,
            vec![NativeKeyboardEvent::new(KEY_LEFTALT, true)]
        );
        assert!(alt.window_actions.is_empty());
        assert_eq!(
            fullscreen.keyboard_events,
            vec![NativeKeyboardEvent::new(KEY_F11, true)]
        );
        assert!(fullscreen.window_actions.is_empty());
    }

    #[test]
    fn native_input_shortcut_inhibition_keeps_emergency_exit_shortcut() {
        let mut input = NativeInputState::new(320, 200);
        input.set_keyboard_shortcuts_inhibited(true);
        input.handle_key_event(KEY_LEFTALT, 1);

        let effect = input.handle_key_event(KEY_P, 1);

        assert!(effect.exit_requested);
        assert!(effect.keyboard_events.is_empty());
    }

    #[test]
    fn native_input_backend_plan_prefers_libseat_libinput_when_available() {
        let plan = NativeInputBackendPlan::choose(NativeInputBackendChoice {
            preference: NativeInputBackendPreference::Auto,
            libseat_available: true,
            libinput_available: true,
            raw_evdev_available: true,
        });

        assert_eq!(plan.primary, NativeInputBackendKind::LibseatLibinputUdev);
        assert_eq!(
            plan.fallbacks,
            vec![
                NativeInputBackendKind::DirectLibinputUdev,
                NativeInputBackendKind::RawEvdev,
            ]
        );
    }

    #[test]
    fn native_input_backend_plan_uses_direct_libinput_without_libseat() {
        let plan = NativeInputBackendPlan::choose(NativeInputBackendChoice {
            preference: NativeInputBackendPreference::Auto,
            libseat_available: false,
            libinput_available: true,
            raw_evdev_available: true,
        });

        assert_eq!(plan.primary, NativeInputBackendKind::DirectLibinputUdev);
        assert_eq!(plan.fallbacks, vec![NativeInputBackendKind::RawEvdev]);
    }

    #[test]
    fn native_input_backend_plan_can_force_raw_evdev_for_debugging() {
        let plan = NativeInputBackendPlan::choose(NativeInputBackendChoice {
            preference: NativeInputBackendPreference::RawEvdev,
            libseat_available: true,
            libinput_available: true,
            raw_evdev_available: true,
        });

        assert_eq!(plan.primary, NativeInputBackendKind::RawEvdev);
        assert!(plan.fallbacks.is_empty());
    }

    #[test]
    fn native_input_backend_plan_falls_back_to_raw_when_libinput_is_unavailable() {
        let plan = NativeInputBackendPlan::choose(NativeInputBackendChoice {
            preference: NativeInputBackendPreference::Auto,
            libseat_available: true,
            libinput_available: false,
            raw_evdev_available: true,
        });

        assert_eq!(plan.primary, NativeInputBackendKind::RawEvdev);
        assert!(plan.fallbacks.is_empty());
    }

    #[test]
    fn native_seat_lifecycle_requests_suspend_then_resume() {
        let mut lifecycle = NativeSeatLifecycle::default();

        assert_eq!(
            lifecycle.apply_event(NativeSeatEvent::Disabled),
            Some(NativeSeatInputAction::Suspend)
        );
        assert_eq!(
            lifecycle.apply_event(NativeSeatEvent::Enabled),
            Some(NativeSeatInputAction::Resume)
        );
    }

    #[test]
    fn native_input_state_handles_normalized_relative_motion() {
        let mut input = NativeInputState::new(320, 200);

        let effect = input.handle_hardware_input_event(NativeHardwareInputEvent::PointerMotion(
            PointerMotionSample::relative(10, RelativeMotion::accelerated_only(12.0, -4.0)),
        ));

        assert_eq!(effect.pointer_motion, Some((172.0, 96.0)));
        assert!(effect.redraw_requested);
    }

    #[test]
    fn native_input_pointer_motion_can_skip_frame_repaint_with_hardware_cursor() {
        let mut input = NativeInputState::new(320, 200);

        let effect = input.handle_hardware_input_event(NativeHardwareInputEvent::PointerMotion(
            PointerMotionSample::relative(10, RelativeMotion::accelerated_only(12.0, -4.0)),
        ));

        assert_eq!(effect.pointer_motion, Some((172.0, 96.0)));
        assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Hardware));
        assert!(effect.requires_frame_repaint(NativeCursorRenderMode::Software));
    }

    #[test]
    fn native_forwarded_keyboard_input_skips_frame_repaint_without_local_visual_change() {
        let mut input = NativeInputState::new(320, 200);

        let effect = input.handle_key_event(KEY_Z, 1);

        assert_eq!(
            effect.keyboard_events,
            vec![NativeKeyboardEvent::new(KEY_Z, true)]
        );
        assert!(effect.redraw_requested);
        assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Hardware));
        assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Software));
    }

    #[test]
    fn native_forwarded_pointer_button_skips_frame_repaint_without_local_visual_change() {
        let mut input = NativeInputState::new(320, 200);

        let effect = input.handle_pointer_button(u32::from(BTN_LEFT), true);

        assert_eq!(effect.pointer_buttons.len(), 1);
        assert!(effect.redraw_requested);
        assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Hardware));
        assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Software));
    }

    #[test]
    fn native_forwarded_pointer_axis_skips_frame_repaint_without_local_visual_change() {
        let mut input = NativeInputState::new(320, 200);

        let effect = input.handle_pointer_axis(0.0, 120.0);

        assert_eq!(effect.pointer_axis, Some((0.0, 120.0)));
        assert!(effect.redraw_requested);
        assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Hardware));
        assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Software));
    }

    #[test]
    fn native_input_window_interaction_still_repaints_with_hardware_cursor() {
        let mut input = NativeInputState::new(320, 200);
        input.handle_key_event(KEY_LEFTALT, 1);
        input.handle_pointer_button(u32::from(BTN_LEFT), true);

        let effect = input.handle_hardware_input_event(NativeHardwareInputEvent::PointerMotion(
            PointerMotionSample::relative(10, RelativeMotion::accelerated_only(12.0, -4.0)),
        ));

        assert_eq!(
            effect.window_actions,
            vec![NativeWindowAction::UpdateInteraction { x: 172.0, y: 96.0 }]
        );
        assert!(effect.requires_frame_repaint(NativeCursorRenderMode::Hardware));
    }

    #[test]
    fn native_libinput_scroll_axis_value_skips_absent_axis_reader() {
        let value = libinput_scroll_axis_value(false, || panic!("axis value should not be read"));

        assert_eq!(value, 0.0);
    }

    #[test]
    fn native_xrgb_copy_preserves_ignored_high_byte_for_fast_row_copy() {
        let frame = [0x7f11_2233];
        let mut bytes = [0; 4];

        copy_argb_frame_to_xrgb_mapping(&frame, 1, 1, 4, &mut bytes).unwrap();

        assert_eq!(bytes, 0x7f11_2233u32.to_ne_bytes());
    }

    #[test]
    fn native_xrgb_copy_damage_updates_only_requested_rectangles() {
        let frame = [0xff00_0001, 0xff00_0002, 0xff00_0003, 0xff00_0004];
        let untouched = 0xa5;
        let mut bytes = [untouched; 16];

        let copied = copy_argb_frame_to_xrgb_mapping_damage(
            &frame,
            2,
            2,
            8,
            &mut bytes,
            NativeFrameCopyDamage::Rects(&[NativeDamageRect {
                x: 1,
                y: 0,
                width: 1,
                height: 2,
            }]),
        )
        .unwrap();

        assert_eq!(copied, 8);
        assert_eq!(&bytes[0..4], &[untouched; 4]);
        assert_eq!(&bytes[4..8], &0xff00_0002u32.to_ne_bytes());
        assert_eq!(&bytes[8..12], &[untouched; 4]);
        assert_eq!(&bytes[12..16], &0xff00_0004u32.to_ne_bytes());
    }

    #[test]
    fn native_xrgb_copy_damage_caps_overlapping_rects_at_full_frame_copy() {
        let frame = [0xff00_0001, 0xff00_0002, 0xff00_0003, 0xff00_0004];
        let mut bytes = [0; 16];

        let copied = copy_argb_frame_to_xrgb_mapping_damage(
            &frame,
            2,
            2,
            8,
            &mut bytes,
            NativeFrameCopyDamage::Rects(&[
                NativeDamageRect {
                    x: 0,
                    y: 0,
                    width: 2,
                    height: 2,
                },
                NativeDamageRect {
                    x: 0,
                    y: 0,
                    width: 2,
                    height: 2,
                },
            ]),
        )
        .unwrap();

        assert_eq!(copied, 16);
        assert_eq!(&bytes[0..4], &0xff00_0001u32.to_ne_bytes());
        assert_eq!(&bytes[4..8], &0xff00_0002u32.to_ne_bytes());
        assert_eq!(&bytes[8..12], &0xff00_0003u32.to_ne_bytes());
        assert_eq!(&bytes[12..16], &0xff00_0004u32.to_ne_bytes());
    }

    #[test]
    fn native_frame_renderer_repairs_surface_bounds_change_with_partial_scene_rebuild() {
        let spotlight = SpotlightModel::default();
        let mut renderer = NativeFrameRenderer::default();
        let initial_surface = test_renderable_surface(7, 0, 0, 4, 4, RenderableSurfaceDamage::Full);

        let initial = renderer.render_frame(NativeFrameRequest {
            width: 96,
            height: 96,
            surfaces: &[initial_surface],
            dock_items: Vec::new(),
            spotlight: &spotlight,
            shell_generation: 1,
            visual_state: DesktopVisualState::wallpaper_only(),
            render_generation: 1,
            client_cursor: None,
        });
        assert_eq!(initial.scene_rebuild_kind, DesktopSceneRebuildKind::Full);

        let moved_surface = test_renderable_surface(
            7,
            2,
            0,
            4,
            4,
            RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            }]),
        );

        let moved = renderer.render_frame(NativeFrameRequest {
            width: 96,
            height: 96,
            surfaces: &[moved_surface],
            dock_items: Vec::new(),
            spotlight: &spotlight,
            shell_generation: 1,
            visual_state: DesktopVisualState::wallpaper_only(),
            render_generation: 2,
            client_cursor: None,
        });

        assert_eq!(moved.scene_rebuild_kind, DesktopSceneRebuildKind::Partial);
        assert_eq!(moved.frame_copy_kind, DesktopFrameCopyKind::Partial);
    }

    #[test]
    fn native_frame_renderer_reports_full_scene_rebuild_when_surface_identity_changes() {
        let spotlight = SpotlightModel::default();
        let mut renderer = NativeFrameRenderer::default();
        let initial_surface = test_renderable_surface(7, 0, 0, 4, 4, RenderableSurfaceDamage::Full);

        let initial = renderer.render_frame(NativeFrameRequest {
            width: 96,
            height: 96,
            surfaces: &[initial_surface],
            dock_items: Vec::new(),
            spotlight: &spotlight,
            shell_generation: 1,
            visual_state: DesktopVisualState::wallpaper_only(),
            render_generation: 1,
            client_cursor: None,
        });
        assert_eq!(initial.scene_rebuild_kind, DesktopSceneRebuildKind::Full);

        let replacement_surface = test_renderable_surface(
            8,
            0,
            0,
            4,
            4,
            RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            }]),
        );

        let replacement = renderer.render_frame(NativeFrameRequest {
            width: 96,
            height: 96,
            surfaces: &[replacement_surface],
            dock_items: Vec::new(),
            spotlight: &spotlight,
            shell_generation: 1,
            visual_state: DesktopVisualState::wallpaper_only(),
            render_generation: 2,
            client_cursor: None,
        });

        assert_eq!(
            replacement.scene_rebuild_kind,
            DesktopSceneRebuildKind::Full
        );
    }

    #[test]
    fn native_cursor_argb_bytes_places_texture_pixels_in_pitched_buffer() {
        let pixels = [0xff11_2233, 0x8044_5566, 0xff77_8899, 0];

        let bytes = native_cursor_argb_bytes(&pixels, 2, 2, 4, 4, 16).unwrap();

        assert_eq!(&bytes[0..4], &0xff11_2233u32.to_ne_bytes());
        assert_eq!(&bytes[4..8], &0x8044_5566u32.to_ne_bytes());
        assert_eq!(&bytes[16..20], &0xff77_8899u32.to_ne_bytes());
        assert_eq!(&bytes[20..24], &0u32.to_ne_bytes());
        assert!(bytes[24..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn native_input_coalesces_consecutive_relative_motion_events() {
        let events = coalesce_pointer_motion_events(vec![
            NativeHardwareInputEvent::PointerMotion(PointerMotionSample::relative(
                10,
                RelativeMotion {
                    dx: 1.0,
                    dy: 0.0,
                    dx_unaccelerated: 2.0,
                    dy_unaccelerated: 0.0,
                },
            )),
            NativeHardwareInputEvent::PointerMotion(PointerMotionSample::relative(
                20,
                RelativeMotion {
                    dx: 0.0,
                    dy: 2.0,
                    dx_unaccelerated: 0.0,
                    dy_unaccelerated: 3.0,
                },
            )),
            NativeHardwareInputEvent::PointerMotion(PointerMotionSample::relative(
                30,
                RelativeMotion {
                    dx: 3.0,
                    dy: 4.0,
                    dx_unaccelerated: 5.0,
                    dy_unaccelerated: 6.0,
                },
            )),
        ]);

        assert_eq!(
            events,
            vec![NativeHardwareInputEvent::PointerMotion(
                PointerMotionSample::relative(
                    30,
                    RelativeMotion {
                        dx: 4.0,
                        dy: 6.0,
                        dx_unaccelerated: 7.0,
                        dy_unaccelerated: 9.0,
                    },
                )
            )]
        );
    }

    #[test]
    fn native_pointer_motion_sample_keeps_relative_delta_when_cursor_clamps_at_edge() {
        let mut input = NativeInputState::new(320, 200);
        let sample = PointerMotionSample::relative(
            42,
            RelativeMotion {
                dx: 1_000.0,
                dy: -1_000.0,
                dx_unaccelerated: 1_200.0,
                dy_unaccelerated: -1_200.0,
            },
        );

        let effect =
            input.handle_hardware_input_event(NativeHardwareInputEvent::PointerMotion(sample));

        assert_eq!(effect.pointer_motion, Some((319.0, 0.0)));
        assert_eq!(effect.relative_motion, Some(sample.relative.unwrap()));
    }

    #[test]
    fn native_input_coalescing_preserves_button_boundaries() {
        let events = coalesce_pointer_motion_events(vec![
            NativeHardwareInputEvent::PointerMotion(PointerMotionSample::relative(
                10,
                RelativeMotion::accelerated_only(1.0, 0.0),
            )),
            NativeHardwareInputEvent::PointerButton {
                button: u32::from(BTN_LEFT),
                pressed: true,
            },
            NativeHardwareInputEvent::PointerMotion(PointerMotionSample::relative(
                20,
                RelativeMotion::accelerated_only(0.0, 2.0),
            )),
        ]);

        assert_eq!(
            events,
            vec![
                NativeHardwareInputEvent::PointerMotion(PointerMotionSample::relative(
                    10,
                    RelativeMotion::accelerated_only(1.0, 0.0),
                )),
                NativeHardwareInputEvent::PointerButton {
                    button: u32::from(BTN_LEFT),
                    pressed: true,
                },
                NativeHardwareInputEvent::PointerMotion(PointerMotionSample::relative(
                    20,
                    RelativeMotion::accelerated_only(0.0, 2.0),
                )),
            ]
        );
    }

    #[test]
    fn native_input_coalesces_consecutive_absolute_motion_to_latest_position() {
        let events = coalesce_pointer_motion_events(vec![
            NativeHardwareInputEvent::PointerMotion(PointerMotionSample::absolute(10, 12.0, 30.0)),
            NativeHardwareInputEvent::PointerMotion(PointerMotionSample::absolute(20, 18.0, 35.0)),
        ]);

        assert_eq!(
            events,
            vec![NativeHardwareInputEvent::PointerMotion(
                PointerMotionSample::absolute(20, 18.0, 35.0)
            )]
        );
    }

    #[test]
    fn input_event_paths_select_only_real_keyboard_and_mouse_devices() {
        let root = std::env::current_dir()
            .unwrap()
            .join("target")
            .join("native-input-tests")
            .join(std::process::id().to_string());
        let dev_root = root.join("dev-input");
        let udev_root = root.join("udev-data");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&dev_root).unwrap();
        fs::create_dir_all(&udev_root).unwrap();
        fs::write(dev_root.join("event3"), "").unwrap();
        fs::write(dev_root.join("event4"), "").unwrap();
        fs::write(dev_root.join("event12"), "").unwrap();
        fs::write(udev_root.join("c13:67"), "E:ID_INPUT_MOUSE=1\n").unwrap();
        fs::write(udev_root.join("c13:68"), "E:ID_INPUT_KEYBOARD=1\n").unwrap();
        fs::write(udev_root.join("c13:76"), "E:ID_INPUT=1\n").unwrap();

        let paths = input_event_paths_with_udev(&dev_root, &udev_root);
        let names = paths
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
            .collect::<Vec<_>>();
        let _ = fs::remove_dir_all(&root);

        assert_eq!(names, ["event3", "event4"]);
    }

    #[test]
    fn native_repaint_decision_skips_visible_frame_callback_without_damage() {
        assert_eq!(
            native_repaint_decision(NativeRepaintInputs {
                accepted_clients: false,
                render_generation_changed: false,
                pending_frame_work: true,
                only_pending_surface_frame_callbacks: true,
                redraw_requested: false,
                page_flip_pending: false,
            }),
            NativeRepaintDecision {
                repaint: false,
                protocol_only_present: true,
            }
        );
    }

    #[test]
    fn native_repaint_decision_paints_non_callback_pending_frame_work() {
        assert_eq!(
            native_repaint_decision(NativeRepaintInputs {
                accepted_clients: false,
                render_generation_changed: false,
                pending_frame_work: true,
                only_pending_surface_frame_callbacks: false,
                redraw_requested: false,
                page_flip_pending: false,
            }),
            NativeRepaintDecision {
                repaint: true,
                protocol_only_present: false,
            }
        );
    }

    #[test]
    fn native_repaint_decision_paints_visual_changes_even_with_frame_callback() {
        assert_eq!(
            native_repaint_decision(NativeRepaintInputs {
                accepted_clients: false,
                render_generation_changed: true,
                pending_frame_work: true,
                only_pending_surface_frame_callbacks: true,
                redraw_requested: false,
                page_flip_pending: false,
            }),
            NativeRepaintDecision {
                repaint: true,
                protocol_only_present: false,
            }
        );
    }

    #[test]
    fn native_repaint_decision_waits_for_pending_pageflip_before_repaint() {
        assert_eq!(
            native_repaint_decision(NativeRepaintInputs {
                accepted_clients: false,
                render_generation_changed: true,
                pending_frame_work: true,
                only_pending_surface_frame_callbacks: false,
                redraw_requested: true,
                page_flip_pending: true,
            }),
            NativeRepaintDecision {
                repaint: false,
                protocol_only_present: false,
            }
        );
    }
}
