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
    FinalizeResize {
        window: WindowId,
        geometry: WindowGeometry,
        mode: ToplevelMode,
    },
    Close {
        window: WindowId,
    },
    SetActivated {
        window: WindowId,
        activated: bool,
    },
    Restack {
        window: WindowId,
    },
    PublishState {
        window: WindowId,
        mode: ToplevelMode,
        minimized: bool,
        activated: bool,
    },
}

pub(crate) fn backend_for_window(window: WindowBackend) -> WindowBackend {
    window
}
