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
    slice, thread,
    time::{Duration, Instant},
};

use oblivion_one::compositor::{
    DesktopComposeRequest, DesktopFrameCopyKind, DesktopSceneRebuildKind, DesktopSceneRenderer,
    DesktopVisualState, OwnCompositorServer, RenderGenerationCause, RenderableSurface,
    ShellDockItem, ShellOverlayRenderer, ShellOverlayState, ShellTopbarModel, SpotlightModel,
    cursor_texture_pixels, cursor_texture_size, dock_item_at, surface_origins,
};
use oblivion_one::session::NativeSessionProbe;
use oblivion_one::spawn_cpu_compositor_app;

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
        Self {
            runtime_dir: std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
            kms_device,
            render_device: first_dri_node("renderD"),
            connector,
            kms_resources,
        }
    }
}

pub fn run(mut server: OwnCompositorServer, app: Vec<String>) -> NativeResult<()> {
    let perf = NativePerfLogger::from_env();
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
    if !app.is_empty() {
        println!("startup app command deferred until native scanout is ready: {app:?}");
        perf.log("app.deferred", || {
            vec![
                NativePerfField::usize("argc", app.len()),
                NativePerfField::str("command", app.join(" ")),
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

    let seat_session = open_native_seat_session(&session_probe);
    let drm_plan = NativeDrmBackendPlan::choose(NativeDrmBackendChoice {
        preference: NativeDrmBackendPreference::from_env(),
        seat_available: seat_session.is_some(),
    });
    println!("native DRM backend target: {}", drm_plan.primary.as_str());
    let kms = NativeDrmDevice::open(drm_plan, kms_device, seat_session.clone())?;
    println!("native DRM backend active: {}", kms.kind().as_str());
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
        ]
    });
    let frame_pacing = NativeFramePacing::from_mode(&target.mode);
    println!(
        "native frame pacing: {} Hz target, active wake {} us",
        frame_pacing.refresh_hz,
        frame_pacing.active_interval.as_micros()
    );

    server.set_output_size(target.width, target.height);
    server.set_output_refresh_hz(frame_pacing.refresh_hz);
    let original_crtc = drm_ffi::mode::get_crtc(kms.file().as_fd(), target.crtc_id).ok();
    let scanout_plan = NativeScanoutPlan::choose(NativeScanoutChoice {
        preference: NativeScanoutPreference::from_env(),
        gbm_available: session_probe.plan.dependencies.gbm_available,
        egl_available: session_probe.plan.dependencies.egl_available,
        page_flip_available: true,
    });
    println!(
        "native scanout backend target: {}",
        scanout_plan.primary.as_str()
    );
    let scanout_target = scanout_plan.primary.as_str();
    let mut scanout =
        NativeScanoutBackend::open(scanout_plan, kms.file(), target.width, target.height)?;
    println!("native scanout backend active: {}", scanout.kind().as_str());
    perf.log("native.backend", || {
        vec![
            NativePerfField::str("drm", kms.kind().as_str()),
            NativePerfField::str("scanout", scanout.kind().as_str()),
            NativePerfField::str("scanout_target", scanout_target),
            NativePerfField::str("mode", mode_label.clone()),
            NativePerfField::u64("refresh_hz", u64::from(frame_pacing.refresh_hz)),
            NativePerfField::u64(
                "active_wake_us",
                frame_pacing.active_interval.as_micros() as u64,
            ),
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
    let initial_paint = scanout.paint_server_frame(
        &mut frame_renderer,
        &server,
        &input_state,
        cursor_render_mode,
        &initial_damage,
    )?;
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
    let mut last_render_generation = server.render_generation();
    let mut last_renderable_surfaces = server.renderable_surfaces().to_vec();
    let mut frame_index = 0u64;
    let mut known_toplevels = server.xdg_toplevels();
    let mut pending_launches = VecDeque::<NativeAppLaunchPerf>::new();
    let mut resize_perf = NativeResizePerfState::default();
    loop {
        let pageflip_drain_start = Instant::now();
        let pageflip_completed = scanout.drain_page_flip_events(kms.file().as_raw_fd())?;
        let pageflip_drain_us = elapsed_micros(pageflip_drain_start);
        if pageflip_completed {
            let finish_frame_start = Instant::now();
            server.finish_frame();
            perf.log("native.finish_frame", || {
                vec![
                    NativePerfField::str("reason", "pageflip_complete"),
                    NativePerfField::u64("elapsed_us", elapsed_micros(finish_frame_start)),
                    NativePerfField::usize("surfaces", server.renderable_surfaces().len()),
                    NativePerfField::u64("render_generation", server.render_generation()),
                ]
            });
        }

        let present_start = Instant::now();
        scanout.present(kms.file().as_fd(), target.crtc_id)?;
        let present_us = elapsed_micros(present_start);
        let tick_blocked_by_pageflip = scanout.page_flip_pending();
        let tick_start = Instant::now();
        let accepted = if tick_blocked_by_pageflip {
            0
        } else {
            server.tick()?
        };
        let tick_us = if tick_blocked_by_pageflip {
            0
        } else {
            elapsed_micros(tick_start)
        };
        let current_toplevels = server.xdg_toplevels();
        if current_toplevels > known_toplevels {
            for _ in known_toplevels..current_toplevels {
                let app_id = server.last_app_id().unwrap_or("unknown").to_string();
                if let Some(launch) = pending_launches.pop_front() {
                    perf.log("app.first_toplevel", || {
                        vec![
                            NativePerfField::str("program", launch.program.clone()),
                            NativePerfField::str("command", launch.command.clone()),
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
        let mut redraw_requested = false;
        let mut input_events = false;
        let mut skipped_input_repaints = 0usize;
        let input_drain_start = Instant::now();
        let raw_events = input_devices.drain_events();
        let input_drain_us = elapsed_micros(input_drain_start);
        let raw_input_events = raw_events.len();
        let coalesced_events = coalesce_pointer_motion_events(raw_events);
        let coalesced_input_events = coalesced_events.len();
        for event in coalesced_events {
            input_events = true;
            let effect = input_state.handle_hardware_input_event(event);
            let effect_requested_redraw = effect.redraw_requested;
            if let Some((cursor_x, cursor_y)) = effect.cursor_position
                && let Some(cursor) = hardware_cursor.as_mut()
                && let Err(error) = cursor.move_to(cursor_x, cursor_y)
            {
                if cursor_preference == NativeCursorPreference::Hardware {
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
            )?;
            if application.exit_requested {
                println!("native input exit requested; shutting down cleanly");
                return Ok(());
            }
            if let Some(launch) = application.launch {
                perf.log("app.spawn", || {
                    vec![
                        NativePerfField::str("program", launch.program.clone()),
                        NativePerfField::str("command", launch.command.clone()),
                        NativePerfField::u64("pid", u64::from(launch.pid)),
                        NativePerfField::u64("spawn_us", launch.spawn_us),
                        NativePerfField::str("app_policy", "cpu-compositor"),
                    ]
                });
                pending_launches.push_back(launch);
            }
            if effect_requested_redraw && !application.redraw_requested {
                skipped_input_repaints = skipped_input_repaints.saturating_add(1);
            }
            redraw_requested |= application.redraw_requested;
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
            page_flip_pending: scanout.page_flip_pending(),
        });
        if repaint_decision.repaint {
            let render_cause = native_repaint_cause_label(
                render_generation_cause,
                render_generation_changed,
                accepted,
                pending_frame_work,
                redraw_requested,
            );
            let output_damage = native_output_damage_for_repaint(
                target.width,
                target.height,
                &last_renderable_surfaces,
                server.renderable_surfaces(),
                render_generation_cause,
                render_generation_changed,
            );
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
            let cpu_after = perf
                .enabled()
                .then(NativeProcessCpuSample::read_current)
                .flatten();
            let (cpu_user_us, cpu_system_us) = cpu_before
                .zip(cpu_after)
                .map(|(before, after)| after.delta_us_since(before))
                .unwrap_or((0, 0));
            let repaint_present_start = Instant::now();
            scanout.present(kms.file().as_fd(), target.crtc_id)?;
            let repaint_present_us = elapsed_micros(repaint_present_start);
            frame_index = frame_index.saturating_add(1);
            perf.log("native.frame", || {
                let mut fields = paint_stats.fields();
                fields.extend(output_damage.fields());
                fields.extend([
                    NativePerfField::u64("index", frame_index),
                    NativePerfField::str("phase", "repaint"),
                    NativePerfField::str("mode", mode_label.clone()),
                    NativePerfField::str("cursor", cursor_render_mode.as_str()),
                    NativePerfField::u64("refresh_hz", u64::from(frame_pacing.refresh_hz)),
                    NativePerfField::usize("surfaces", server.renderable_surfaces().len()),
                    NativePerfField::u64("render_generation", render_generation),
                    NativePerfField::bool("render_changed", render_generation_changed),
                    NativePerfField::str("render_cause", render_cause),
                    NativePerfField::u64("tick_us", tick_us),
                    NativePerfField::bool("tick_blocked_by_pageflip", tick_blocked_by_pageflip),
                    NativePerfField::u64("input_drain_us", input_drain_us),
                    NativePerfField::usize("raw_input_events", raw_input_events),
                    NativePerfField::usize("coalesced_input_events", coalesced_input_events),
                    NativePerfField::u64("pageflip_drain_us", pageflip_drain_us),
                    NativePerfField::bool("pageflip_completed", pageflip_completed),
                    NativePerfField::u64("present_us", present_us),
                    NativePerfField::u64("repaint_present_us", repaint_present_us),
                    NativePerfField::u64("cpu_user_us", cpu_user_us),
                    NativePerfField::u64("cpu_system_us", cpu_system_us),
                    NativePerfField::bool("pending_frame_work", pending_frame_work),
                    NativePerfField::bool("redraw_requested", redraw_requested),
                    NativePerfField::usize("skipped_input_repaints", skipped_input_repaints),
                    NativePerfField::usize("accepted_clients", accepted),
                ]);
                fields
            });
            last_render_generation = render_generation;
            last_renderable_surfaces = server.renderable_surfaces().to_vec();
        } else if repaint_decision.protocol_only_present {
            perf.log("native.frame_skip", || {
                vec![
                    NativePerfField::str("reason", "frame_callback_no_damage"),
                    NativePerfField::usize("skipped_input_repaints", skipped_input_repaints),
                    NativePerfField::u64("tick_us", tick_us),
                    NativePerfField::bool("tick_blocked_by_pageflip", tick_blocked_by_pageflip),
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
            perf.log("native.finish_frame", || {
                vec![
                    NativePerfField::str("reason", "frame_callback_no_damage"),
                    NativePerfField::u64("elapsed_us", elapsed_micros(finish_frame_start)),
                    NativePerfField::usize("surfaces", server.renderable_surfaces().len()),
                    NativePerfField::u64("render_generation", server.render_generation()),
                ]
            });
        } else if scanout.page_flip_pending() {
            perf.log("native.frame_skip", || {
                vec![
                    NativePerfField::str("reason", "pageflip_pending"),
                    NativePerfField::usize("skipped_input_repaints", skipped_input_repaints),
                    NativePerfField::u64("tick_us", tick_us),
                    NativePerfField::bool("tick_blocked_by_pageflip", tick_blocked_by_pageflip),
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
                    NativePerfField::bool("tick_blocked_by_pageflip", tick_blocked_by_pageflip),
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
        if server.has_pending_frame_work() && scanout.frames_complete_immediately() {
            let finish_frame_start = Instant::now();
            server.finish_frame();
            perf.log("native.finish_frame", || {
                vec![
                    NativePerfField::str("reason", "immediate_scanout"),
                    NativePerfField::u64("elapsed_us", elapsed_micros(finish_frame_start)),
                    NativePerfField::usize("surfaces", server.renderable_surfaces().len()),
                    NativePerfField::u64("render_generation", server.render_generation()),
                ]
            });
        }
        thread::sleep(native_wakeup_interval(
            NativeLoopActivity {
                accepted_clients: accepted > 0,
                input_events,
                redraw_requested,
                pending_frame_work: server.has_pending_frame_work(),
                has_surfaces: !server.renderable_surfaces().is_empty(),
            },
            frame_pacing,
        ));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NativeLoopActivity {
    accepted_clients: bool,
    input_events: bool,
    redraw_requested: bool,
    pending_frame_work: bool,
    has_surfaces: bool,
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

fn native_wakeup_interval(activity: NativeLoopActivity, pacing: NativeFramePacing) -> Duration {
    native_wakeup_interval_with_pacing(activity, pacing)
}

fn native_wakeup_interval_with_pacing(
    activity: NativeLoopActivity,
    pacing: NativeFramePacing,
) -> Duration {
    if activity.input_events {
        pacing.input_interval
    } else if activity.accepted_clients || activity.redraw_requested || activity.pending_frame_work
    {
        pacing.active_interval
    } else if activity.has_surfaces {
        pacing.active_interval.min(NATIVE_SURFACE_WAKEUP_INTERVAL)
    } else {
        pacing.idle_interval
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NativeFramePacing {
    refresh_hz: u32,
    input_interval: Duration,
    active_interval: Duration,
    idle_interval: Duration,
}

impl NativeFramePacing {
    fn from_mode(mode: &drm_sys::drm_mode_modeinfo) -> Self {
        Self::for_refresh_hz(mode.vrefresh)
    }

    fn for_refresh_hz(refresh_hz: u32) -> Self {
        let refresh_hz = normalize_refresh_hz(refresh_hz);
        Self {
            refresh_hz,
            input_interval: NATIVE_INPUT_WAKEUP_INTERVAL,
            active_interval: Duration::from_micros(1_000_000 / u64::from(refresh_hz)),
            idle_interval: NATIVE_IDLE_WAKEUP_INTERVAL,
        }
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
            render_generation: server.render_generation(),
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
            });
        NativeRenderedFrame {
            pixels: &self.frame,
            scene_rebuild_kind: self.scene_renderer.last_rebuild_kind(),
            frame_copy_kind: self.scene_renderer.last_frame_copy_kind(),
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
const NATIVE_INPUT_WAKEUP_INTERVAL: Duration = Duration::from_millis(1);
const NATIVE_SURFACE_WAKEUP_INTERVAL: Duration = Duration::from_millis(4);
const NATIVE_IDLE_WAKEUP_INTERVAL: Duration = Duration::from_millis(4);
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
enum NativeHardwareInputEvent {
    Key { code: u16, value: i32 },
    PointerButton { button: u32, pressed: bool },
    PointerMotion { dx: f64, dy: f64 },
    PointerAbsolute { x: f64, y: f64 },
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
                REL_X => Some(Self::PointerMotion {
                    dx: f64::from(event.value),
                    dy: 0.0,
                }),
                REL_Y => Some(Self::PointerMotion {
                    dx: 0.0,
                    dy: f64::from(event.value),
                }),
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
    pointer_buttons: Vec<NativePointerButtonEvent>,
    pointer_axis: Option<(f64, f64)>,
    window_actions: Vec<NativeWindowAction>,
    launch_command: Option<Vec<String>>,
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

    fn cursor_position(&self) -> (i32, i32) {
        (self.cursor_x.round() as i32, self.cursor_y.round() as i32)
    }

    fn desktop_visual_state(&self, cursor_mode: NativeCursorRenderMode) -> DesktopVisualState {
        match cursor_mode {
            NativeCursorRenderMode::Software => {
                let (x, y) = self.cursor_position();
                DesktopVisualState::with_cursor(x, y)
            }
            NativeCursorRenderMode::Hardware => DesktopVisualState::wallpaper_only(),
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
            NativeHardwareInputEvent::PointerMotion { dx, dy } => {
                self.handle_pointer_motion_delta(dx, dy)
            }
            NativeHardwareInputEvent::PointerAbsolute { x, y } => {
                self.handle_pointer_absolute(x, y)
            }
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

    fn handle_pointer_motion_delta(&mut self, dx: f64, dy: f64) -> NativeInputEffect {
        let mut effect = NativeInputEffect::default();
        self.cursor_x = (self.cursor_x + dx).clamp(0.0, f64::from(self.output_width - 1));
        self.cursor_y = (self.cursor_y + dy).clamp(0.0, f64::from(self.output_height - 1));
        if !self.spotlight_visible() {
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
        effect.mark_cursor_moved(self.cursor_x, self.cursor_y);
        effect
    }

    fn handle_pointer_absolute(&mut self, x: f64, y: f64) -> NativeInputEffect {
        let mut effect = NativeInputEffect::default();
        self.cursor_x = x.clamp(0.0, f64::from(self.output_width - 1));
        self.cursor_y = y.clamp(0.0, f64::from(self.output_height - 1));
        if !self.spotlight_visible() {
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
        effect.mark_cursor_moved(self.cursor_x, self.cursor_y);
        effect
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
    use input::event::pointer::{Axis, ButtonState, PointerEvent, PointerScrollEvent};

    match event {
        input::Event::Keyboard(KeyboardEvent::Key(event)) => {
            let code = u16::try_from(event.key()).ok()?;
            let value = match event.key_state() {
                KeyState::Pressed => 1,
                KeyState::Released => 0,
            };
            Some(NativeHardwareInputEvent::Key { code, value })
        }
        input::Event::Pointer(PointerEvent::Motion(event)) => {
            Some(NativeHardwareInputEvent::PointerMotion {
                dx: event.dx(),
                dy: event.dy(),
            })
        }
        input::Event::Pointer(PointerEvent::MotionAbsolute(event)) => {
            Some(NativeHardwareInputEvent::PointerAbsolute {
                x: event.absolute_x_transformed(output_width),
                y: event.absolute_y_transformed(output_height),
            })
        }
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
    Relative { dx: f64, dy: f64 },
    Absolute { x: f64, y: f64 },
}

fn coalesce_pointer_motion_events(
    events: Vec<NativeHardwareInputEvent>,
) -> Vec<NativeHardwareInputEvent> {
    let mut coalesced = Vec::with_capacity(events.len());
    let mut pending_motion = None;

    for event in events {
        match event {
            NativeHardwareInputEvent::PointerMotion { dx, dy } => match pending_motion {
                Some(PendingPointerMotion::Relative {
                    dx: pending_dx,
                    dy: pending_dy,
                }) => {
                    pending_motion = Some(PendingPointerMotion::Relative {
                        dx: pending_dx + dx,
                        dy: pending_dy + dy,
                    });
                }
                Some(pending) => {
                    flush_pending_pointer_motion(&mut coalesced, pending);
                    pending_motion = Some(PendingPointerMotion::Relative { dx, dy });
                }
                None => {
                    pending_motion = Some(PendingPointerMotion::Relative { dx, dy });
                }
            },
            NativeHardwareInputEvent::PointerAbsolute { x, y } => match pending_motion {
                Some(PendingPointerMotion::Absolute { .. }) => {
                    pending_motion = Some(PendingPointerMotion::Absolute { x, y });
                }
                Some(pending) => {
                    flush_pending_pointer_motion(&mut coalesced, pending);
                    pending_motion = Some(PendingPointerMotion::Absolute { x, y });
                }
                None => {
                    pending_motion = Some(PendingPointerMotion::Absolute { x, y });
                }
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
        PendingPointerMotion::Relative { dx, dy } => {
            events.push(NativeHardwareInputEvent::PointerMotion { dx, dy });
        }
        PendingPointerMotion::Absolute { x, y } => {
            events.push(NativeHardwareInputEvent::PointerAbsolute { x, y });
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
) -> NativeResult<NativeInputApplication> {
    let mut application = NativeInputApplication {
        redraw_requested: effect.requires_frame_repaint(cursor_mode),
        exit_requested: effect.exit_requested,
        launch: None,
    };
    for event in effect.keyboard_events {
        server.send_keyboard_key(event.key, event.pressed);
    }
    if let Some((x, y)) = effect.pointer_motion {
        server.send_pointer_motion(x, y);
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
        application.launch = launch_native_shell_command(server, command)?;
    }
    Ok(application)
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
) -> NativeResult<Option<NativeAppLaunchPerf>> {
    let Some(program) = command.first().cloned() else {
        return Ok(None);
    };
    let socket_name = server.socket_name().to_string();
    let command_label = command.join(" ");
    let spawn_start = Instant::now();
    match spawn_cpu_compositor_app(&socket_name, &command) {
        Ok(Some(pid)) => {
            let spawn_us = elapsed_micros(spawn_start);
            println!(
                "spawned `{program}` from native Spotlight on Oblivion Wayland socket `{socket_name}` as pid {pid}"
            );
            Ok(Some(NativeAppLaunchPerf {
                program,
                command: command_label,
                pid,
                spawn_us,
                started_at: spawn_start,
            }))
        }
        Ok(None) => Ok(None),
        Err(error) => Err(io::Error::other(format!(
            "failed to spawn `{program}` from native Spotlight: {error}"
        ))
        .into()),
    }
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
    GbmEglPageFlip,
    DumbFramebuffer,
    Unavailable,
}

impl NativeScanoutKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::GbmEglPageFlip => "GBM/KMS pageflip",
            Self::DumbFramebuffer => "KMS dumb framebuffer",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NativePaintStats {
    backend: NativeScanoutKind,
    width: u32,
    height: u32,
    bytes: usize,
    copy_bytes: usize,
    write_bytes: usize,
    scene_rebuild: DesktopSceneRebuildKind,
    frame_copy: DesktopFrameCopyKind,
    total_us: u64,
    render_us: u64,
    copy_us: u64,
    write_us: u64,
}

impl NativePaintStats {
    fn fields(self) -> Vec<NativePerfField> {
        vec![
            NativePerfField::str("scanout", self.backend.as_str()),
            NativePerfField::u64("width", u64::from(self.width)),
            NativePerfField::u64("height", u64::from(self.height)),
            NativePerfField::usize("bytes", self.bytes),
            NativePerfField::usize("copy_bytes", self.copy_bytes),
            NativePerfField::usize("write_bytes", self.write_bytes),
            NativePerfField::str("scene_rebuild", self.scene_rebuild.as_str()),
            NativePerfField::str("frame_copy", self.frame_copy.as_str()),
            NativePerfField::u64("paint_us", self.total_us),
            NativePerfField::u64("render_us", self.render_us),
            NativePerfField::u64("copy_us", self.copy_us),
            NativePerfField::u64("write_us", self.write_us),
        ]
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
    fn from_surface_bounds(surface: &RenderableSurface, origin: (i32, i32)) -> Option<Self> {
        (surface.width > 0 && surface.height > 0).then_some(Self {
            x: origin.0,
            y: origin.1,
            width: surface.width,
            height: surface.height,
        })
    }

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

fn native_rect_symmetric_difference(
    left: NativeDamageRect,
    right: NativeDamageRect,
) -> Vec<NativeDamageRect> {
    let mut rects = native_rect_difference(left, right);
    rects.extend(native_rect_difference(right, left));
    rects
}

fn native_rect_difference(
    source: NativeDamageRect,
    cut: NativeDamageRect,
) -> Vec<NativeDamageRect> {
    let Some(overlap) = native_rect_intersection(source, cut) else {
        return vec![source];
    };

    let mut rects = Vec::new();
    push_native_damage_rect(
        &mut rects,
        source.left(),
        source.top(),
        source.right(),
        overlap.top(),
    );
    push_native_damage_rect(
        &mut rects,
        source.left(),
        overlap.bottom(),
        source.right(),
        source.bottom(),
    );
    push_native_damage_rect(
        &mut rects,
        source.left(),
        overlap.top(),
        overlap.left(),
        overlap.bottom(),
    );
    push_native_damage_rect(
        &mut rects,
        overlap.right(),
        overlap.top(),
        source.right(),
        overlap.bottom(),
    );
    rects
}

fn native_rect_intersection(
    left: NativeDamageRect,
    right: NativeDamageRect,
) -> Option<NativeDamageRect> {
    let left_edge = left.left().max(right.left());
    let top = left.top().max(right.top());
    let right_edge = left.right().min(right.right());
    let bottom = left.bottom().min(right.bottom());
    (right_edge > left_edge && bottom > top).then_some(NativeDamageRect {
        x: left_edge.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
        y: top.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
        width: u32::try_from(right_edge.saturating_sub(left_edge)).unwrap_or(u32::MAX),
        height: u32::try_from(bottom.saturating_sub(top)).unwrap_or(u32::MAX),
    })
}

fn push_native_damage_rect(
    rects: &mut Vec<NativeDamageRect>,
    left: i64,
    top: i64,
    right: i64,
    bottom: i64,
) {
    if right <= left || bottom <= top {
        return;
    }
    rects.push(NativeDamageRect {
        x: left.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
        y: top.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
        width: u32::try_from(right.saturating_sub(left)).unwrap_or(u32::MAX),
        height: u32::try_from(bottom.saturating_sub(top)).unwrap_or(u32::MAX),
    });
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
        let mut accumulator = Self::for_output(output_width, output_height);
        let origins = surface_origins(surfaces);
        for (surface, origin) in surfaces.iter().zip(origins) {
            accumulator.add_surface(surface, origin);
        }
        accumulator
    }

    fn from_surface_bounds_changes(
        output_width: u32,
        output_height: u32,
        previous_surfaces: &[RenderableSurface],
        current_surfaces: &[RenderableSurface],
    ) -> Self {
        let mut previous_rects = HashMap::new();
        for (surface, origin) in previous_surfaces
            .iter()
            .zip(surface_origins(previous_surfaces))
        {
            if let Some(rect) = NativeDamageRect::from_surface_bounds(surface, origin)
                .and_then(|rect| rect.clipped_to_output(output_width, output_height))
            {
                previous_rects.insert(surface.surface_id, rect);
            }
        }

        let mut current_rects = HashMap::new();
        let mut top_left_resize_previews = HashMap::new();
        for (surface, origin) in current_surfaces
            .iter()
            .zip(surface_origins(current_surfaces))
        {
            if let Some(rect) = NativeDamageRect::from_surface_bounds(surface, origin)
                .and_then(|rect| rect.clipped_to_output(output_width, output_height))
            {
                if surface
                    .resize_preview
                    .is_some_and(|preview| !preview.anchor_right && !preview.anchor_bottom)
                {
                    top_left_resize_previews.insert(surface.surface_id, true);
                }
                current_rects.insert(surface.surface_id, rect);
            }
        }

        let mut accumulator = Self::for_output(output_width, output_height);
        for (surface_id, previous_rect) in &previous_rects {
            let current_rect = current_rects.get(surface_id).copied();
            if current_rect != Some(*previous_rect) {
                if let Some(current_rect) = current_rect {
                    if top_left_resize_previews.contains_key(surface_id)
                        && previous_rect.x == current_rect.x
                        && previous_rect.y == current_rect.y
                    {
                        accumulator.rects.extend(native_rect_symmetric_difference(
                            *previous_rect,
                            current_rect,
                        ));
                    } else {
                        accumulator.rects.push(*previous_rect);
                        accumulator.rects.push(current_rect);
                    }
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

fn native_output_damage_for_repaint(
    width: u32,
    height: u32,
    previous_surfaces: &[RenderableSurface],
    surfaces: &[RenderableSurface],
    cause: RenderGenerationCause,
    render_generation_changed: bool,
) -> NativeOutputDamage {
    if render_generation_changed && cause.uses_surface_damage() {
        NativeDamageAccumulator::from_surfaces(width, height, surfaces).into_output_damage()
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
    GbmEglPageFlip,
    DumbFramebuffer,
}

impl NativeScanoutPreference {
    fn from_env() -> Self {
        match std::env::var("OBLIVION_ONE_SCANOUT_BACKEND") {
            Ok(value) if matches!(value.as_str(), "gbm" | "egl" | "pageflip" | "gbm-egl") => {
                Self::GbmEglPageFlip
            }
            Ok(value) if matches!(value.as_str(), "dumb" | "framebuffer" | "legacy") => {
                Self::DumbFramebuffer
            }
            Ok(value) if value == "auto" => Self::Auto,
            Ok(value) => {
                eprintln!(
                    "native scanout: unknown OBLIVION_ONE_SCANOUT_BACKEND={value:?}; using auto"
                );
                Self::Auto
            }
            Err(_) => Self::Auto,
        }
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
            NativeScanoutPreference::GbmEglPageFlip
                if choice.gbm_available && choice.egl_available && choice.page_flip_available =>
            {
                Self {
                    primary: NativeScanoutKind::GbmEglPageFlip,
                    fallbacks: Vec::new(),
                }
            }
            NativeScanoutPreference::GbmEglPageFlip => Self::unavailable(),
            NativeScanoutPreference::DumbFramebuffer => Self {
                primary: NativeScanoutKind::DumbFramebuffer,
                fallbacks: Vec::new(),
            },
            NativeScanoutPreference::Auto
                if choice.gbm_available && choice.egl_available && choice.page_flip_available =>
            {
                Self {
                    primary: NativeScanoutKind::GbmEglPageFlip,
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
}

enum NativeScanoutBackend {
    Gbm(NativeGbmScanout),
    Dumb(DumbFramebuffer),
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
            NativeScanoutKind::GbmEglPageFlip => {
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
            Self::Gbm(_) => NativeScanoutKind::GbmEglPageFlip,
            Self::Dumb(_) => NativeScanoutKind::DumbFramebuffer,
        }
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
            Self::Gbm(scanout) => scanout.fb_id(),
            Self::Dumb(framebuffer) => framebuffer.fb_id,
        }
    }

    fn finish_initial_scanout(&mut self) {
        if let Self::Gbm(scanout) = self {
            scanout.finish_initial_scanout();
        }
    }

    fn present(&mut self, fd: BorrowedFd<'_>, crtc_id: u32) -> io::Result<()> {
        if let Self::Gbm(scanout) = self {
            scanout.present(fd, crtc_id)?;
        }
        Ok(())
    }

    fn drain_page_flip_events(&mut self, fd: RawFd) -> io::Result<bool> {
        if let Self::Gbm(scanout) = self {
            return scanout.drain_page_flip_events(fd);
        }
        Ok(false)
    }

    fn page_flip_pending(&self) -> bool {
        match self {
            Self::Gbm(scanout) => scanout.page_flip_pending(),
            Self::Dumb(_) => false,
        }
    }

    fn frames_complete_immediately(&self) -> bool {
        matches!(self, Self::Dumb(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativePageFlipError {
    AlreadyPending,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct NativePageFlipState {
    pending: bool,
}

impl NativePageFlipState {
    const fn can_schedule(self) -> bool {
        !self.pending
    }

    fn mark_scheduled(&mut self) -> Result<(), NativePageFlipError> {
        if self.pending {
            Err(NativePageFlipError::AlreadyPending)
        } else {
            self.pending = true;
            Ok(())
        }
    }

    fn mark_presented(&mut self) -> bool {
        let was_pending = self.pending;
        self.pending = false;
        was_pending
    }
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
            backend: NativeScanoutKind::GbmEglPageFlip,
            width: self.width,
            height: self.height,
            bytes: byte_len,
            copy_bytes,
            write_bytes: byte_len,
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

    fn present(&mut self, fd: BorrowedFd<'_>, crtc_id: u32) -> io::Result<()> {
        if !self.page_flip.can_schedule() {
            return Ok(());
        }
        let Some(index) = self.ready_index.take() else {
            return Ok(());
        };
        if index == self.current_index {
            return Ok(());
        }
        self.page_flip
            .mark_scheduled()
            .map_err(|_| io::Error::other("native page flip is already pending"))?;
        match drm_ffi::mode::page_flip(
            fd,
            crtc_id,
            self.buffers[index].fb_id,
            drm_sys::DRM_MODE_PAGE_FLIP_EVENT,
            0,
        ) {
            Ok(()) => {
                self.pending_index = Some(index);
                Ok(())
            }
            Err(error) => {
                self.page_flip.mark_presented();
                self.ready_index = Some(index);
                Err(error)
            }
        }
    }

    fn drain_page_flip_events(&mut self, fd: RawFd) -> io::Result<bool> {
        let completed = drain_drm_page_flip_events(fd)?;
        if completed == 0 {
            return Ok(false);
        }
        if let Some(index) = self.pending_index.take() {
            self.current_index = index;
        }
        Ok(self.page_flip.mark_presented())
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

#[derive(Debug, Clone, PartialEq, Eq)]
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

fn drain_drm_page_flip_events(fd: RawFd) -> io::Result<usize> {
    let mut pollfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let ready = unsafe { libc::poll(&mut pollfd, 1, 0) };
    if ready < 0 {
        return Err(io::Error::last_os_error());
    }
    if ready == 0 || pollfd.revents & libc::POLLIN == 0 {
        return Ok(0);
    }

    let mut buffer = [0u8; 1024];
    let read = unsafe { libc::read(fd, buffer.as_mut_ptr().cast::<c_void>(), buffer.len()) };
    if read < 0 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::WouldBlock {
            return Ok(0);
        }
        return Err(error);
    }
    let mut offset = 0usize;
    let read = read as usize;
    let mut completed = 0usize;
    while offset + mem::size_of::<drm_sys::drm_event>() <= read {
        let event = unsafe {
            ptr::read_unaligned(buffer.as_ptr().add(offset).cast::<drm_sys::drm_event>())
        };
        let length = event.length as usize;
        if length == 0 || offset + length > read {
            break;
        }
        if event.type_ == drm_sys::DRM_EVENT_FLIP_COMPLETE {
            completed += 1;
        }
        offset += length;
    }
    Ok(completed)
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
            width: self.width,
            height: self.height,
            bytes: self.size,
            copy_bytes,
            write_bytes: 0,
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
        compose_nested_output,
    };
    use oblivion_one::render_backend::buffer::{BufferSize, CommittedSurfaceBuffer};

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
                NativePerfField::bool("gpu", false),
            ],
        );

        assert_eq!(
            line,
            "perf app.spawn program=\"zen browser\" pid=4242 gpu=false"
        );
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
    fn native_output_damage_for_window_resize_coalesces_nested_old_new_bounds() {
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
            vec![
                NativeDamageRect {
                    x: origin.0,
                    y: origin.1 + 200,
                    width: 340,
                    height: 30,
                },
                NativeDamageRect {
                    x: origin.0 + 300,
                    y: origin.1,
                    width: 40,
                    height: 200,
                },
            ]
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
    fn native_scanout_plan_prefers_gbm_pageflip_when_ready() {
        let plan = NativeScanoutPlan::choose(NativeScanoutChoice {
            preference: NativeScanoutPreference::Auto,
            gbm_available: true,
            egl_available: true,
            page_flip_available: true,
        });

        assert_eq!(plan.primary, NativeScanoutKind::GbmEglPageFlip);
        assert_eq!(plan.fallbacks, vec![NativeScanoutKind::DumbFramebuffer]);
    }

    #[test]
    fn native_scanout_plan_uses_dumb_framebuffer_without_egl() {
        let plan = NativeScanoutPlan::choose(NativeScanoutChoice {
            preference: NativeScanoutPreference::Auto,
            gbm_available: true,
            egl_available: false,
            page_flip_available: true,
        });

        assert_eq!(plan.primary, NativeScanoutKind::DumbFramebuffer);
        assert!(plan.fallbacks.is_empty());
    }

    #[test]
    fn native_pageflip_state_blocks_overlapping_flips() {
        let mut state = NativePageFlipState::default();

        assert!(state.can_schedule());
        assert_eq!(state.mark_scheduled(), Ok(()));
        assert!(!state.can_schedule());
        assert_eq!(
            state.mark_scheduled(),
            Err(NativePageFlipError::AlreadyPending)
        );
        assert!(state.mark_presented());
        assert!(state.can_schedule());
        assert!(!state.mark_presented());
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

        let effect = input.handle_hardware_input_event(NativeHardwareInputEvent::PointerMotion {
            dx: 12.0,
            dy: -4.0,
        });

        assert_eq!(effect.pointer_motion, Some((172.0, 96.0)));
        assert!(effect.redraw_requested);
    }

    #[test]
    fn native_input_pointer_motion_can_skip_frame_repaint_with_hardware_cursor() {
        let mut input = NativeInputState::new(320, 200);

        let effect = input.handle_hardware_input_event(NativeHardwareInputEvent::PointerMotion {
            dx: 12.0,
            dy: -4.0,
        });

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

        let effect = input.handle_hardware_input_event(NativeHardwareInputEvent::PointerMotion {
            dx: 12.0,
            dy: -4.0,
        });

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
            NativeHardwareInputEvent::PointerMotion { dx: 1.0, dy: 0.0 },
            NativeHardwareInputEvent::PointerMotion { dx: 0.0, dy: 2.0 },
            NativeHardwareInputEvent::PointerMotion { dx: 3.0, dy: 4.0 },
        ]);

        assert_eq!(
            events,
            vec![NativeHardwareInputEvent::PointerMotion { dx: 4.0, dy: 6.0 }]
        );
    }

    #[test]
    fn native_input_coalescing_preserves_button_boundaries() {
        let events = coalesce_pointer_motion_events(vec![
            NativeHardwareInputEvent::PointerMotion { dx: 1.0, dy: 0.0 },
            NativeHardwareInputEvent::PointerButton {
                button: u32::from(BTN_LEFT),
                pressed: true,
            },
            NativeHardwareInputEvent::PointerMotion { dx: 0.0, dy: 2.0 },
        ]);

        assert_eq!(
            events,
            vec![
                NativeHardwareInputEvent::PointerMotion { dx: 1.0, dy: 0.0 },
                NativeHardwareInputEvent::PointerButton {
                    button: u32::from(BTN_LEFT),
                    pressed: true,
                },
                NativeHardwareInputEvent::PointerMotion { dx: 0.0, dy: 2.0 },
            ]
        );
    }

    #[test]
    fn native_input_coalesces_consecutive_absolute_motion_to_latest_position() {
        let events = coalesce_pointer_motion_events(vec![
            NativeHardwareInputEvent::PointerAbsolute { x: 12.0, y: 30.0 },
            NativeHardwareInputEvent::PointerAbsolute { x: 18.0, y: 35.0 },
        ]);

        assert_eq!(
            events,
            vec![NativeHardwareInputEvent::PointerAbsolute { x: 18.0, y: 35.0 }]
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
    fn native_wakeup_uses_idle_interval_when_no_work_is_pending() {
        assert_eq!(
            native_wakeup_interval(
                NativeLoopActivity {
                    accepted_clients: false,
                    input_events: false,
                    redraw_requested: false,
                    pending_frame_work: false,
                    has_surfaces: false,
                },
                NativeFramePacing::for_refresh_hz(60),
            ),
            NATIVE_IDLE_WAKEUP_INTERVAL
        );
    }

    #[test]
    fn native_wakeup_uses_fast_interval_after_input() {
        assert_eq!(
            native_wakeup_interval(
                NativeLoopActivity {
                    accepted_clients: false,
                    input_events: true,
                    redraw_requested: false,
                    pending_frame_work: false,
                    has_surfaces: true,
                },
                NativeFramePacing::for_refresh_hz(60),
            ),
            NATIVE_INPUT_WAKEUP_INTERVAL
        );
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

    #[test]
    fn native_frame_pacing_uses_kms_refresh_rate() {
        let pacing = NativeFramePacing::for_refresh_hz(165);

        assert_eq!(pacing.active_interval, Duration::from_micros(6060));
    }

    #[test]
    fn native_frame_pacing_falls_back_to_sixty_hz_when_refresh_is_missing() {
        let pacing = NativeFramePacing::for_refresh_hz(0);

        assert_eq!(pacing.active_interval, Duration::from_micros(16_666));
    }

    #[test]
    fn native_wakeup_uses_poll_interval_while_surfaces_are_active() {
        let pacing = NativeFramePacing::for_refresh_hz(165);

        assert_eq!(
            native_wakeup_interval_with_pacing(
                NativeLoopActivity {
                    accepted_clients: false,
                    input_events: false,
                    redraw_requested: false,
                    pending_frame_work: false,
                    has_surfaces: true,
                },
                pacing,
            ),
            NATIVE_SURFACE_WAKEUP_INTERVAL
        );
    }
}
