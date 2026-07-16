#![allow(dead_code)]

use super::{ToplevelMode, WindowBackend, WindowGeometry, WindowId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WindowBackendCommand {
    Configure {
        window: WindowId,
        geometry: WindowGeometry,
        mode: ToplevelMode,
        resizing: bool,
    },
    Close {
        window: WindowId,
    },
    SetActivated {
        window: WindowId,
        activated: bool,
    },
}

pub(crate) fn backend_for_window(window: WindowBackend) -> WindowBackend {
    window
}
