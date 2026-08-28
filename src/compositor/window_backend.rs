#![allow(dead_code)]

use super::{ToplevelMode, WindowBackend, WindowGeometry, WindowId};

#[derive(Debug, Clone, PartialEq, Eq)]
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
    RestackExact {
        windows: Vec<WindowId>,
    },
    PublishState {
        window: WindowId,
        mode: ToplevelMode,
        minimized: bool,
        activated: bool,
    },
    SetWorkspace {
        window: WindowId,
        workspace: u32,
    },
    ClearWorkspace {
        window: WindowId,
    },
    PublishWorkspaceState {
        workspace_count: u32,
        current_workspace: u32,
        output_width: u32,
        output_height: u32,
    },
}

pub(crate) fn backend_for_window(window: WindowBackend) -> WindowBackend {
    window
}
