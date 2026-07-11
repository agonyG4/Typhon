#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellSurface {
    Dock,
    Topbar,
    Launcher,
    Settings,
}

impl ShellSurface {
    pub const fn status(self) -> &'static str {
        match self {
            Self::Dock => "deferred",
            Self::Topbar => "deferred",
            Self::Launcher => "deferred",
            Self::Settings => "deferred",
        }
    }
}
