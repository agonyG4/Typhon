use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeInputStrategy {
    SeatManaged,
    RawEvdevFallback,
    Unavailable,
}

impl NativeInputStrategy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SeatManaged => "seat-managed libinput",
            Self::RawEvdevFallback => "raw evdev fallback",
            Self::Unavailable => "unavailable",
        }
    }

    pub const fn is_available(self) -> bool {
        !matches!(self, Self::Unavailable)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeOutputStrategy {
    GbmKms,
    DumbFramebuffer,
    Unavailable,
}

impl NativeOutputStrategy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GbmKms => "gbm/egl/kms prerequisites available",
            Self::DumbFramebuffer => "dumb framebuffer fallback",
            Self::Unavailable => "unavailable",
        }
    }

    pub const fn is_available(self) -> bool {
        !matches!(self, Self::Unavailable)
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct NativeSessionDependencies {
    pub runtime_dir: bool,
    pub seat_manager_available: bool,
    pub libseat_available: bool,
    pub libinput_available: bool,
    pub xkbcommon_available: bool,
    pub kms_device_available: bool,
    pub render_device_available: bool,
    pub connected_output_available: bool,
    pub gbm_available: bool,
    pub egl_available: bool,
    pub raw_input_access: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeSessionPlan {
    pub dependencies: NativeSessionDependencies,
    pub input_strategy: NativeInputStrategy,
    pub output_strategy: NativeOutputStrategy,
}

impl NativeSessionPlan {
    pub const fn from_dependencies(dependencies: NativeSessionDependencies) -> Self {
        let input_strategy = if dependencies.seat_manager_available
            && dependencies.libseat_available
            && dependencies.libinput_available
            && dependencies.xkbcommon_available
        {
            NativeInputStrategy::SeatManaged
        } else if dependencies.raw_input_access {
            NativeInputStrategy::RawEvdevFallback
        } else {
            NativeInputStrategy::Unavailable
        };

        let output_strategy = if dependencies.kms_device_available
            && dependencies.render_device_available
            && dependencies.connected_output_available
            && dependencies.gbm_available
            && dependencies.egl_available
        {
            NativeOutputStrategy::GbmKms
        } else if dependencies.kms_device_available && dependencies.connected_output_available {
            NativeOutputStrategy::DumbFramebuffer
        } else {
            NativeOutputStrategy::Unavailable
        };

        Self {
            dependencies,
            input_strategy,
            output_strategy,
        }
    }

    pub const fn can_attempt_native_session(self) -> bool {
        self.dependencies.runtime_dir
            && self.input_strategy.is_available()
            && self.output_strategy.is_available()
    }

    pub const fn is_production_ready(self) -> bool {
        self.dependencies.runtime_dir
            && matches!(self.input_strategy, NativeInputStrategy::SeatManaged)
            && matches!(self.output_strategy, NativeOutputStrategy::GbmKms)
    }

    pub fn warnings(self) -> Vec<&'static str> {
        let mut warnings = Vec::new();
        if !self.dependencies.runtime_dir {
            warnings.push("XDG_RUNTIME_DIR is missing; SDDM/native clients need a runtime dir");
        }
        match self.input_strategy {
            NativeInputStrategy::SeatManaged => {}
            NativeInputStrategy::RawEvdevFallback => warnings.push(
                "using raw evdev fallback; replace this with seat-managed libinput before production SDDM use",
            ),
            NativeInputStrategy::Unavailable => warnings.push(
                "native input unavailable; install/use libseat + libinput or grant readable input devices",
            ),
        }
        match self.output_strategy {
            NativeOutputStrategy::GbmKms => {}
            NativeOutputStrategy::DumbFramebuffer => warnings.push(
                "using dumb framebuffer fallback; GBM/EGL/KMS render loop is required for compositor-class performance",
            ),
            NativeOutputStrategy::Unavailable => warnings.push(
                "native output unavailable; no connected KMS output was detected",
            ),
        }
        warnings
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeSessionProbe {
    pub runtime_dir: Option<PathBuf>,
    pub kms_device: Option<PathBuf>,
    pub render_device: Option<PathBuf>,
    pub connected_output: Option<PathBuf>,
    pub raw_input_device: Option<PathBuf>,
    pub plan: NativeSessionPlan,
}

impl NativeSessionProbe {
    pub fn detect() -> Self {
        let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|path| path.is_dir());
        let kms_device = first_device_with_prefix(Path::new("/dev/dri"), "card");
        let render_device = first_device_with_prefix(Path::new("/dev/dri"), "renderD");
        let connected_output = first_connected_output(Path::new("/sys/class/drm"));
        let raw_input_device = first_readable_input_event(Path::new("/dev/input"));

        let dependencies = NativeSessionDependencies {
            runtime_dir: runtime_dir.is_some(),
            seat_manager_available: seat_manager_available(),
            libseat_available: pkg_config_library_exists("libseat"),
            libinput_available: pkg_config_library_exists("libinput"),
            xkbcommon_available: pkg_config_library_exists("xkbcommon"),
            kms_device_available: kms_device.is_some(),
            render_device_available: render_device.is_some(),
            connected_output_available: connected_output.is_some(),
            gbm_available: pkg_config_library_exists("gbm"),
            egl_available: pkg_config_library_exists("egl"),
            raw_input_access: raw_input_device.is_some(),
        };
        let plan = NativeSessionPlan::from_dependencies(dependencies);

        Self {
            runtime_dir,
            kms_device,
            render_device,
            connected_output,
            raw_input_device,
            plan,
        }
    }
}

fn seat_manager_available() -> bool {
    Path::new("/run/systemd/seats").is_dir()
        || Path::new("/run/seatd.sock").exists()
        || std::env::var_os("XDG_SEAT").is_some()
}

fn pkg_config_library_exists(name: &str) -> bool {
    Command::new("pkg-config")
        .arg("--exists")
        .arg(name)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn first_device_with_prefix(root: &Path, prefix: &str) -> Option<PathBuf> {
    let mut paths = fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(prefix))
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths.into_iter().next()
}

fn first_connected_output(root: &Path) -> Option<PathBuf> {
    let mut outputs = fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("card") && name.contains('-'))
                && fs::read_to_string(path.join("status"))
                    .is_ok_and(|status| status.trim() == "connected")
        })
        .collect::<Vec<_>>();
    outputs.sort();
    outputs.into_iter().next()
}

fn first_readable_input_event(root: &Path) -> Option<PathBuf> {
    let mut paths = fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("event"))
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths
        .into_iter()
        .find(|path| OpenOptions::new().read(true).open(path).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_session_plan_prefers_seat_managed_input_when_available() {
        let plan = NativeSessionPlan::from_dependencies(NativeSessionDependencies {
            runtime_dir: true,
            seat_manager_available: true,
            libseat_available: true,
            libinput_available: true,
            xkbcommon_available: true,
            kms_device_available: true,
            render_device_available: true,
            connected_output_available: true,
            gbm_available: true,
            egl_available: true,
            raw_input_access: false,
        });

        assert_eq!(plan.input_strategy, NativeInputStrategy::SeatManaged);
        assert!(plan.can_attempt_native_session());
    }

    #[test]
    fn native_session_plan_reports_raw_evdev_fallback() {
        let plan = NativeSessionPlan::from_dependencies(NativeSessionDependencies {
            runtime_dir: true,
            kms_device_available: true,
            connected_output_available: true,
            raw_input_access: true,
            ..NativeSessionDependencies::default()
        });

        assert_eq!(plan.input_strategy, NativeInputStrategy::RawEvdevFallback);
        assert!(!plan.is_production_ready());
        assert!(
            plan.warnings()
                .iter()
                .any(|warning| warning.contains("raw evdev"))
        );
    }

    #[test]
    fn native_session_plan_blocks_without_input_backend() {
        let plan = NativeSessionPlan::from_dependencies(NativeSessionDependencies {
            runtime_dir: true,
            kms_device_available: true,
            connected_output_available: true,
            ..NativeSessionDependencies::default()
        });

        assert_eq!(plan.input_strategy, NativeInputStrategy::Unavailable);
        assert!(!plan.can_attempt_native_session());
    }
}
