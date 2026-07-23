use std::{env, path::PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XwaylandMode {
    Off,
    BaseLazy,
    /// Foundation-profile eager startup, retained for explicit deployments
    /// that do not request managed XWM.
    BaseEager,
    ManagedLazy,
    ManagedEager,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XwaylandStartPolicy {
    Off,
    Lazy,
    Eager,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XwaylandProfile {
    Foundation,
    Managed,
}

impl XwaylandMode {
    pub fn parse(value: Option<&str>) -> Self {
        match value {
            None | Some("off") => Self::Off,
            Some("base") => Self::BaseLazy,
            Some("lazy") => Self::ManagedLazy,
            Some("eager") => Self::ManagedEager,
            Some(value) => {
                eprintln!("xwayland: unknown TYPHON_XWAYLAND value {value:?}; using off");
                Self::Off
            }
        }
    }

    pub const fn is_enabled(self) -> bool {
        !matches!(self, Self::Off)
    }

    pub const fn is_eager(self) -> bool {
        matches!(self, Self::BaseEager | Self::ManagedEager)
    }

    pub const fn is_managed(self) -> bool {
        matches!(self, Self::ManagedLazy | Self::ManagedEager)
    }

    pub const fn profile(self) -> XwaylandProfile {
        if self.is_managed() {
            XwaylandProfile::Managed
        } else {
            XwaylandProfile::Foundation
        }
    }

    pub const fn start_policy(self) -> XwaylandStartPolicy {
        match self {
            Self::Off => XwaylandStartPolicy::Off,
            Self::BaseLazy | Self::ManagedLazy => XwaylandStartPolicy::Lazy,
            Self::BaseEager | Self::ManagedEager => XwaylandStartPolicy::Eager,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XwaylandConfig {
    pub mode: XwaylandMode,
    pub profile: XwaylandProfile,
    pub binary: PathBuf,
    pub log_stderr: bool,
    pub display_min: u32,
    pub display_max: u32,
    #[cfg(test)]
    pub test_root: Option<PathBuf>,
}

pub(crate) const fn xwm_reactor_hot_path_logging_enabled(
    _log_stderr: bool,
    trace_enabled: bool,
) -> bool {
    trace_enabled
}

impl XwaylandConfig {
    pub fn from_environment() -> Self {
        let mode = XwaylandMode::parse(env::var("TYPHON_XWAYLAND").ok().as_deref());
        Self {
            mode,
            profile: mode.profile(),
            binary: env::var_os("TYPHON_XWAYLAND_BINARY")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("Xwayland")),
            log_stderr: env::var_os("TYPHON_XWAYLAND_LOG").is_some_and(|value| value == "1"),
            display_min: 1,
            display_max: 63,
            #[cfg(test)]
            test_root: None,
        }
    }

    #[cfg(test)]
    pub fn for_tests(mode: XwaylandMode, binary: PathBuf) -> Self {
        Self {
            mode,
            profile: mode.profile(),
            binary,
            log_stderr: false,
            display_min: 0,
            display_max: 63,
            test_root: None,
        }
    }

    #[cfg(test)]
    pub fn for_tests_at_root(mode: XwaylandMode, binary: PathBuf, root: PathBuf) -> Self {
        Self {
            mode,
            profile: mode.profile(),
            binary,
            log_stderr: false,
            display_min: 0,
            display_max: 63,
            test_root: Some(root),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn ordinary_xwayland_stderr_logging_does_not_enable_xwm_reactor_hot_path_logs() {
        assert!(!super::xwm_reactor_hot_path_logging_enabled(true, false));
        assert!(super::xwm_reactor_hot_path_logging_enabled(false, true));
    }
}
