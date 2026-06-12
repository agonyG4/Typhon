use std::path::PathBuf;

use crate::{DEFAULT_APP, DEFAULT_HEIGHT, DEFAULT_REFRESH, DEFAULT_WIDTH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NestedBackend {
    Oblivion,
    Hyprland,
    Gamescope,
}

impl NestedBackend {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "oblivion" | "Oblivion" => Some(Self::Oblivion),
            "hyprland" | "Hyprland" => Some(Self::Hyprland),
            "gamescope" | "Gamescope" => Some(Self::Gamescope),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Oblivion => "oblivion",
            Self::Hyprland => "hyprland",
            Self::Gamescope => "gamescope",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NestedOptions {
    pub width: u32,
    pub height: u32,
    pub refresh: u32,
    pub app: Vec<String>,
    pub state_dir: PathBuf,
}

impl NestedOptions {
    pub fn with_defaults(state_dir: PathBuf) -> Self {
        Self {
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            refresh: DEFAULT_REFRESH,
            app: vec![DEFAULT_APP.to_string()],
            state_dir,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopOptions {
    pub width: u32,
    pub height: u32,
    pub refresh: u32,
    pub state_dir: PathBuf,
    pub executable: PathBuf,
    pub backend: NestedBackend,
}

impl DesktopOptions {
    pub fn with_defaults(state_dir: PathBuf, executable: PathBuf) -> Self {
        Self {
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            refresh: DEFAULT_REFRESH,
            state_dir,
            executable,
            backend: NestedBackend::Oblivion,
        }
    }

    pub fn into_nested_options(self) -> NestedOptions {
        NestedOptions {
            width: self.width,
            height: self.height,
            refresh: self.refresh,
            app: vec![
                self.executable.to_string_lossy().into_owned(),
                "prototype".to_string(),
                "--inside-de".to_string(),
            ],
            state_dir: self.state_dir,
        }
    }
}
