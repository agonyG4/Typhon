use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativePerfLogger {
    pub(crate) enabled: bool,
}

impl NativePerfLogger {
    pub(crate) fn from_env() -> Self {
        let enabled = std::env::var("OBLIVION_ONE_PERF_LOG")
            .ok()
            .is_some_and(|value| native_perf_log_value_enabled(&value));
        Self { enabled }
    }

    pub(crate) const fn enabled(self) -> bool {
        self.enabled
    }

    pub(crate) fn log<F>(self, event: &str, fields: F)
    where
        F: FnOnce() -> Vec<NativePerfField>,
    {
        if self.enabled {
            println!("{}", native_perf_line(event, &fields()));
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativePerfField {
    pub(crate) key: &'static str,
    pub(crate) value: String,
}

impl NativePerfField {
    pub(crate) fn str(key: &'static str, value: impl Into<String>) -> Self {
        Self {
            key,
            value: value.into(),
        }
    }

    pub(crate) fn u64(key: &'static str, value: u64) -> Self {
        Self::str(key, value.to_string())
    }

    pub(crate) fn usize(key: &'static str, value: usize) -> Self {
        Self::str(key, value.to_string())
    }

    pub(crate) fn f64(key: &'static str, value: f64) -> Self {
        Self::str(key, format!("{value:.2}"))
    }

    pub(crate) fn bool(key: &'static str, value: bool) -> Self {
        Self::str(key, if value { "true" } else { "false" })
    }
}

pub(crate) fn native_perf_log_value_enabled(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on" | "debug" | "trace"
    )
}

pub(crate) fn native_perf_line(event: &str, fields: &[NativePerfField]) -> String {
    let mut line = format!("perf {event}");
    for field in fields {
        line.push(' ');
        line.push_str(field.key);
        line.push('=');
        line.push_str(&native_perf_value(&field.value));
    }
    line
}

pub(crate) fn native_perf_value(value: &str) -> String {
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

pub(crate) fn elapsed_micros(start: Instant) -> u64 {
    start.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeProcessCpuSample {
    pub(crate) user_ticks: u64,
    pub(crate) system_ticks: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct NativeAppLaunchPerf {
    pub(crate) program: String,
    pub(crate) command: String,
    pub(crate) pid: u32,
    pub(crate) spawn_us: u64,
    pub(crate) started_at: Instant,
    pub(crate) gpu_policy: EffectiveCompositorAppGpuPolicy,
    pub(crate) source: NativeLaunchSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeLaunchSource {
    Startup,
    ExternalShell,
    Spotlight,
    AltTab,
    Binding,
}

impl NativeLaunchSource {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::ExternalShell => "external-shell",
            Self::Spotlight => "spotlight",
            Self::AltTab => "alt-tab",
            Self::Binding => "binding",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeLaunchRequest {
    pub(crate) argv: Vec<String>,
    pub(crate) program: String,
    pub(crate) command: String,
    pub(crate) gpu_policy: EffectiveCompositorAppGpuPolicy,
    pub(crate) source: NativeLaunchSource,
}

pub(crate) fn resolve_native_app_gpu_policy(
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
pub(crate) enum NativeRuntimeStage {
    DrainPageFlipEvents,
    Present,
}

impl NativeRuntimeStage {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::DrainPageFlipEvents => "drain_page_flip_events",
            Self::Present => "present",
        }
    }
}

pub(crate) fn native_runtime_error(
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
pub(crate) struct NativeResizePerf {
    pub(crate) started_at: Instant,
    pub(crate) updates: u64,
}

#[derive(Debug, Default)]
pub(crate) struct NativeResizePerfState {
    pub(crate) active: Option<NativeResizePerf>,
}

impl NativeResizePerfState {
    pub(crate) fn observe_action(
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
    pub(crate) fn read_current() -> Option<Self> {
        let stat = fs::read_to_string("/proc/self/stat").ok()?;
        parse_proc_stat_cpu_ticks(&stat)
    }

    pub(crate) fn delta_us_since(self, previous: Self) -> (u64, u64) {
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

pub(crate) fn parse_proc_stat_cpu_ticks(stat: &str) -> Option<NativeProcessCpuSample> {
    let after_comm = stat.rsplit_once(") ")?.1;
    let fields = after_comm.split_whitespace().collect::<Vec<_>>();
    Some(NativeProcessCpuSample {
        user_ticks: fields.get(11)?.parse().ok()?,
        system_ticks: fields.get(12)?.parse().ok()?,
    })
}
