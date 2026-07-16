//! The X11 window manager boundary.
//!
//! Raw x11rb values stay below this module.  The compositor receives only the
//! generation-bound handles, snapshots, events, and commands defined here.

use std::{collections::VecDeque, fmt, os::fd::RawFd, os::unix::net::UnixStream};

use crate::compositor::{DesktopWindowKind, WindowConstraints, WindowMetadata};
use x11rb::rust_connection::{DefaultStream, RustConnection};

mod atoms;
mod capabilities;
mod commands;
mod connection;
mod events;
mod window;

#[cfg(test)]
mod tests;

use atoms::XwmAtoms;
use capabilities::XwmCapabilities;
pub use window::X11WindowLifecycle;
use window::X11WindowRegistry;

use super::{X11WindowHandle, XwaylandGeneration};

const XWM_EVENT_BUDGET: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct X11Geometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum X11StateAtom {
    Fullscreen,
    Maximized,
    Hidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum X11StateAction {
    Remove,
    Add,
    Toggle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct X11StateRequest {
    pub action: X11StateAction,
    pub first: Option<X11StateAtom>,
    pub second: Option<X11StateAtom>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum X11StackMode {
    Above,
    Below,
    TopIf,
    BottomIf,
    Opposite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct X11ConfigureRequest {
    pub requested: X11Geometry,
    pub sibling: Option<X11WindowHandle>,
    pub stack_mode: Option<X11StackMode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct X11PublishedState {
    pub fullscreen: bool,
    pub maximized: bool,
    pub hidden: bool,
    pub activated: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct X11WindowSnapshot {
    pub handle: X11WindowHandle,
    pub surface_id: u32,
    pub kind: DesktopWindowKind,
    pub geometry: X11Geometry,
    pub metadata: WindowMetadata,
    pub constraints: WindowConstraints,
    pub state: X11PublishedState,
    pub transient_for: Option<X11WindowHandle>,
    pub supports_delete: bool,
    pub supports_take_focus: bool,
    pub sync_counter: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum X11MetadataDelta {
    Title(Option<String>),
    AppId(Option<String>),
    Pid(Option<u32>),
    Constraints(WindowConstraints),
    TransientFor(Option<X11WindowHandle>),
    Protocols {
        supports_delete: bool,
        supports_take_focus: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XwmDrain {
    pub processed: usize,
    pub budget_exhausted: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum XwmEvent {
    WindowReady(X11WindowSnapshot),
    WindowWithdrawn(X11WindowHandle),
    WindowDestroyed(X11WindowHandle),
    MetadataChanged {
        window: X11WindowHandle,
        delta: X11MetadataDelta,
    },
    ConfigureRequested {
        window: X11WindowHandle,
        request: X11ConfigureRequest,
    },
    StateRequested {
        window: X11WindowHandle,
        request: X11StateRequest,
    },
    FocusRequested(X11WindowHandle),
    CloseRequestedByClient(X11WindowHandle),
}

#[derive(Debug, Clone, PartialEq)]
pub enum XwmCommand {
    Map(X11WindowHandle),
    Unmap(X11WindowHandle),
    Configure {
        window: X11WindowHandle,
        geometry: X11Geometry,
    },
    Focus {
        window: Option<X11WindowHandle>,
        timestamp: u32,
    },
    Raise(X11WindowHandle),
    Close(X11WindowHandle),
    SetState {
        window: X11WindowHandle,
        state: X11PublishedState,
    },
}

#[derive(Debug)]
pub enum XwmStartupError {
    Connection(x11rb::errors::ConnectError),
    MissingRequiredExtension(&'static str),
    InvalidScreen,
    RootSetup(x11rb::errors::ConnectionError),
    Protocol(String),
}

impl fmt::Display for XwmStartupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connection(error) => write!(formatter, "XWM connection setup failed: {error}"),
            Self::MissingRequiredExtension(name) => {
                write!(formatter, "XWM requires missing X11 extension {name}")
            }
            Self::InvalidScreen => formatter.write_str("XWM received an invalid X11 screen"),
            Self::RootSetup(error) => write!(formatter, "XWM root setup failed: {error}"),
            Self::Protocol(error) => write!(formatter, "XWM protocol setup failed: {error}"),
        }
    }
}

impl std::error::Error for XwmStartupError {}

#[derive(Debug)]
pub enum XwmError {
    Connection(x11rb::errors::ConnectionError),
    InvalidCommand(&'static str),
    StaleGeneration,
}

impl fmt::Display for XwmError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connection(error) => write!(formatter, "XWM connection error: {error}"),
            Self::InvalidCommand(command) => write!(formatter, "invalid XWM command: {command}"),
            Self::StaleGeneration => formatter.write_str("stale XWM generation"),
        }
    }
}

impl std::error::Error for XwmError {}

#[derive(Debug)]
pub struct Xwm {
    pub(crate) generation: XwaylandGeneration,
    pub(crate) connection: RustConnection<DefaultStream>,
    pub(crate) screen_number: usize,
    pub(crate) root: u32,
    pub(crate) atoms: XwmAtoms,
    pub(crate) capabilities: XwmCapabilities,
    pub(crate) windows: X11WindowRegistry,
    pub(crate) outgoing_events: VecDeque<XwmEvent>,
    pub(crate) supporting_wm_check: u32,
    raw_fd: RawFd,
}

impl Xwm {
    pub fn connect(
        generation: XwaylandGeneration,
        stream: UnixStream,
    ) -> Result<Self, XwmStartupError> {
        connection::connect(generation, stream)
    }

    pub fn raw_fd(&self) -> RawFd {
        self.raw_fd
    }

    pub fn screen_number(&self) -> usize {
        self.screen_number
    }

    pub fn root(&self) -> u32 {
        self.root
    }

    pub fn supporting_wm_check(&self) -> u32 {
        self.supporting_wm_check
    }

    pub fn window_count(&self) -> usize {
        self.windows.len()
    }

    pub fn required_extensions_available(&self) -> bool {
        self.capabilities.composite
            && self.capabilities.xfixes
            && self.capabilities.shape
            && self.capabilities.randr
            && self.capabilities.sync
    }

    pub fn observe_window(&mut self, handle: X11WindowHandle) -> Result<bool, XwmError> {
        if handle.generation() != self.generation {
            return Err(XwmError::StaleGeneration);
        }
        Ok(self.windows.insert_observed(handle))
    }

    pub fn register_snapshot(&mut self, snapshot: X11WindowSnapshot) -> Result<bool, XwmError> {
        if snapshot.handle.generation() != self.generation {
            return Err(XwmError::StaleGeneration);
        }
        Ok(self.windows.insert_snapshot(snapshot))
    }

    pub fn remove_window(&mut self, handle: X11WindowHandle) -> Result<bool, XwmError> {
        if handle.generation() != self.generation {
            return Err(XwmError::StaleGeneration);
        }
        Ok(self.windows.remove(handle).is_some())
    }

    pub fn window_snapshot(&self, handle: X11WindowHandle) -> Option<&X11WindowSnapshot> {
        self.windows.get(handle)?.snapshot.as_ref()
    }

    pub fn clear_generation(&mut self, generation: XwaylandGeneration) {
        self.windows.clear_generation(generation);
    }

    pub fn set_window_lifecycle(
        &mut self,
        handle: X11WindowHandle,
        lifecycle: X11WindowLifecycle,
    ) -> Result<(), XwmError> {
        if handle.generation() != self.generation {
            return Err(XwmError::StaleGeneration);
        }
        if !self.windows.contains(handle) {
            return Err(XwmError::InvalidCommand("unknown X11 window"));
        }
        self.windows
            .get_mut(handle)
            .expect("validated X11 window")
            .lifecycle = lifecycle;
        Ok(())
    }

    pub fn drain_events(&mut self, budget: usize) -> Result<XwmDrain, XwmError> {
        events::drain(self, budget.min(XWM_EVENT_BUDGET))
    }

    pub fn execute(&mut self, command: XwmCommand) -> Result<(), XwmError> {
        commands::execute(self, command)
    }

    pub fn flush(&self) -> Result<(), XwmError> {
        commands::flush(self)
    }

    pub fn take_events(&mut self) -> impl Iterator<Item = XwmEvent> + '_ {
        self.outgoing_events.drain(..)
    }
}

impl Drop for Xwm {
    fn drop(&mut self) {
        use x11rb::{
            connection::Connection as _, protocol::xproto::ConnectionExt as XprotoConnectionExt,
        };

        let _ = self.connection.destroy_window(self.supporting_wm_check);
        let _ = self.connection.flush();
    }
}
