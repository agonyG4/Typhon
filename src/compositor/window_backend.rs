#![allow(dead_code)]

use super::desktop_window::{WindowBackend, WindowId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WindowBackendCommand {
    Configure {
        window: WindowId,
        width: u32,
        height: u32,
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
