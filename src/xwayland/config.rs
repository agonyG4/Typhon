use std::{env, path::PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XwaylandMode {
    Off,
    BaseLazy,
    BaseEager,
}

impl XwaylandMode {
    pub fn parse(value: Option<&str>) -> Self {
        match value {
            None | Some("off") => Self::Off,
            Some("base") => Self::BaseLazy,
            Some("eager") => Self::BaseEager,
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
        matches!(self, Self::BaseEager)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XwaylandConfig {
    pub mode: XwaylandMode,
    pub binary: PathBuf,
    pub display_min: u32,
    pub display_max: u32,
    #[cfg(test)]
    pub test_root: Option<PathBuf>,
}

impl XwaylandConfig {
    pub fn from_environment() -> Self {
        Self {
            mode: XwaylandMode::parse(env::var("TYPHON_XWAYLAND").ok().as_deref()),
            binary: env::var_os("TYPHON_XWAYLAND_BINARY")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("Xwayland")),
            display_min: 0,
            display_max: 63,
            #[cfg(test)]
            test_root: None,
        }
    }

    #[cfg(test)]
    pub fn for_tests(mode: XwaylandMode, binary: PathBuf) -> Self {
        Self {
            mode,
            binary,
            display_min: 0,
            display_max: 63,
            test_root: None,
        }
    }

    #[cfg(test)]
    pub fn for_tests_at_root(mode: XwaylandMode, binary: PathBuf, root: PathBuf) -> Self {
        Self {
            mode,
            binary,
            display_min: 0,
            display_max: 63,
            test_root: Some(root),
        }
    }
}
