//! The X11 window manager boundary.
//!
//! Raw x11rb values stay below this module.  The compositor receives only the
//! generation-bound handles, snapshots, events, and commands defined here.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    fmt,
    os::fd::RawFd,
    os::unix::net::UnixStream,
};

use crate::compositor::{DesktopWindowKind, WindowConstraints, WindowMetadata};
use x11rb::rust_connection::{DefaultStream, RustConnection};

mod association;
mod atoms;
mod capabilities;
mod commands;
mod connection;
mod events;
mod resize_sync;
mod window;

#[cfg(test)]
mod tests;

pub use association::{
    AssociatedSurface, SurfaceAssociationJoin, SurfaceAssociationJoinError, XwmAssociationEvent,
};
use atoms::XwmAtoms;
use capabilities::XwmCapabilities;
pub use resize_sync::{RESIZE_SYNC_TIMEOUT_NS, ResizeSyncError, ResizeSyncState};
pub(crate) use resize_sync::{ResizeSyncCommit, ResizeSyncTracker};
pub use window::X11WindowLifecycle;
use window::X11WindowRegistry;

use super::{X11WindowHandle, XwaylandAssociationEvent, XwaylandGeneration};

const XWM_EVENT_BUDGET: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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
    ResizeSyncAcked {
        window: X11WindowHandle,
        counter_value: u64,
    },
    ResizeSyncPresented(X11WindowHandle),
    ResizeSyncTimedOut(X11WindowHandle),
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
    SyncClientLists {
        client_list: Vec<X11WindowHandle>,
        stacking: Vec<X11WindowHandle>,
    },
    BeginResizeSync {
        window: X11WindowHandle,
        geometry: X11Geometry,
        counter_value: u64,
        deadline_ns: u64,
    },
    SetAllowCommits {
        window: X11WindowHandle,
        allowed: bool,
    },
    CompleteResizeSync(X11WindowHandle),
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
    IdAllocation(String),
    StaleGeneration,
    Association(SurfaceAssociationJoinError),
    ResizeSync(ResizeSyncError),
}

impl fmt::Display for XwmError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connection(error) => write!(formatter, "XWM connection error: {error}"),
            Self::InvalidCommand(command) => write!(formatter, "invalid XWM command: {command}"),
            Self::IdAllocation(error) => {
                write!(formatter, "XWM resource allocation failed: {error}")
            }
            Self::StaleGeneration => formatter.write_str("stale XWM generation"),
            Self::Association(error) => write!(formatter, "XWM association error: {error}"),
            Self::ResizeSync(error) => {
                write!(formatter, "XWM resize synchronization error: {error}")
            }
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
    pub(crate) association: SurfaceAssociationJoin,
    pub(crate) resize_sync: ResizeSyncTracker,
    pub(crate) sync_alarms: HashMap<X11WindowHandle, u32>,
    pub(crate) sync_handles_by_counter: HashMap<u32, X11WindowHandle>,
    pub(crate) next_resize_counter_values: HashMap<X11WindowHandle, u64>,
    buffer_ready_surfaces: HashSet<u32>,
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
        self.observe_window_with_kind(handle, DesktopWindowKind::Managed, X11Geometry::default())
    }

    pub(crate) fn observe_window_with_kind(
        &mut self,
        handle: X11WindowHandle,
        kind: DesktopWindowKind,
        geometry: X11Geometry,
    ) -> Result<bool, XwmError> {
        if handle.generation() != self.generation {
            return Err(XwmError::StaleGeneration);
        }
        Ok(self
            .windows
            .insert_observed_with_kind(handle, kind, geometry))
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
        self.clear_resize_sync(handle);
        Ok(self.windows.remove(handle).is_some())
    }

    pub fn window_snapshot(&self, handle: X11WindowHandle) -> Option<&X11WindowSnapshot> {
        self.windows.get(handle)?.snapshot.as_ref()
    }

    pub fn clear_generation(&mut self, generation: XwaylandGeneration) {
        self.windows.clear_generation(generation);
        self.association.clear_generation(generation);
        self.clear_resize_sync_generation(generation);
        if generation == self.generation {
            self.buffer_ready_surfaces.clear();
        }
    }

    pub fn mark_window_buffer_ready(&mut self, handle: X11WindowHandle) -> Result<(), XwmError> {
        if handle.generation() != self.generation {
            return Err(XwmError::StaleGeneration);
        }
        self.windows
            .mark_buffer_ready(handle)
            .map_err(XwmError::InvalidCommand)?;
        self.emit_ready_if_complete(handle)
    }

    pub fn mark_surface_buffer_ready(
        &mut self,
        generation: XwaylandGeneration,
        surface_id: u32,
    ) -> Result<(), XwmError> {
        if generation != self.generation {
            return Err(XwmError::StaleGeneration);
        }
        self.buffer_ready_surfaces.insert(surface_id);
        let handles = self
            .association
            .completed
            .iter()
            .filter_map(|(handle, association)| {
                (association.surface_id == surface_id).then_some(*handle)
            })
            .collect::<Vec<_>>();
        for handle in handles {
            self.mark_window_buffer_ready(handle)?;
            match self.resize_sync.note_commit(handle) {
                ResizeSyncCommit::Presented | ResizeSyncCommit::FallbackPresented => self
                    .outgoing_events
                    .push_back(XwmEvent::ResizeSyncPresented(handle)),
                ResizeSyncCommit::Deferred | ResizeSyncCommit::Ignored => {}
            }
        }
        Ok(())
    }

    pub(crate) fn note_sync_counter_notify(&mut self, counter: u32, value: u64) {
        let Some(handle) = self.sync_handles_by_counter.get(&counter).copied() else {
            return;
        };
        if self.resize_sync.acknowledge(handle, value) {
            self.outgoing_events.push_back(XwmEvent::ResizeSyncAcked {
                window: handle,
                counter_value: value,
            });
        }
    }

    pub(crate) fn next_resize_sync_deadline_ns(&self) -> Option<u64> {
        self.resize_sync.next_deadline_ns()
    }

    pub(crate) fn handle_resize_sync_deadline(&mut self, now_ns: u64) {
        for handle in self.resize_sync.expired_handles(now_ns) {
            if self.resize_sync.timeout(handle, now_ns) {
                self.outgoing_events
                    .push_back(XwmEvent::ResizeSyncTimedOut(handle));
            }
        }
    }

    pub(crate) fn complete_resize_sync(&mut self, handle: X11WindowHandle) -> Result<(), XwmError> {
        if !self.resize_sync.complete(handle) {
            return Err(XwmError::InvalidCommand("resize sync is not presented"));
        }
        self.clear_resize_sync_alarm(handle);
        Ok(())
    }

    pub(crate) fn clear_resize_sync(&mut self, handle: X11WindowHandle) {
        self.resize_sync.clear(handle);
        self.clear_resize_sync_alarm(handle);
    }

    fn clear_resize_sync_generation(&mut self, generation: XwaylandGeneration) {
        let handles = self
            .sync_alarms
            .keys()
            .filter(|handle| handle.generation() == generation)
            .copied()
            .collect::<Vec<_>>();
        self.resize_sync.clear_generation(generation);
        self.next_resize_counter_values
            .retain(|handle, _| handle.generation() != generation);
        for handle in handles {
            self.clear_resize_sync_alarm(handle);
        }
    }

    fn clear_resize_sync_alarm(&mut self, handle: X11WindowHandle) {
        let Some(alarm) = self.sync_alarms.remove(&handle) else {
            return;
        };
        self.sync_handles_by_counter
            .retain(|_, mapped_handle| *mapped_handle != handle);
        use x11rb::protocol::sync::ConnectionExt as _;
        let _ = self.connection.sync_destroy_alarm(alarm);
    }

    pub fn note_x11_surface_serial(
        &mut self,
        handle: X11WindowHandle,
        serial_lo: u32,
        serial_hi: u32,
    ) -> Result<(), XwmError> {
        if handle.generation() != self.generation {
            return Err(XwmError::StaleGeneration);
        }
        let Some(serial) = super::serial_from_parts(serial_lo, serial_hi) else {
            return Err(XwmError::Association(
                SurfaceAssociationJoinError::InvalidSerial,
            ));
        };
        self.association
            .note_x11_serial(handle, serial)
            .map_err(XwmError::Association)?;
        self.sync_completed_associations();
        Ok(())
    }

    pub fn ingest_wayland_association(
        &mut self,
        event: XwaylandAssociationEvent,
    ) -> Result<(), XwmError> {
        let generation = match event {
            XwaylandAssociationEvent::Committed { generation, .. }
            | XwaylandAssociationEvent::Removed { generation, .. } => generation,
        };
        if generation != self.generation {
            return Err(XwmError::StaleGeneration);
        }
        match event {
            XwaylandAssociationEvent::Committed {
                generation,
                serial,
                surface_id,
            } => self
                .association
                .commit_wayland(generation, serial, surface_id)
                .map_err(XwmError::Association)?,
            XwaylandAssociationEvent::Removed { surface_id, .. } => {
                self.association.remove_wayland_surface(surface_id);
            }
        }
        self.sync_completed_associations();
        Ok(())
    }

    pub fn remove_x11_association(&mut self, handle: X11WindowHandle) -> Result<(), XwmError> {
        if handle.generation() != self.generation {
            return Err(XwmError::StaleGeneration);
        }
        self.association.remove_x11_window(handle);
        Ok(())
    }

    pub fn take_association_events(&mut self) -> Vec<XwmAssociationEvent> {
        self.association.take_events()
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

    fn sync_completed_associations(&mut self) {
        let associations = self
            .association
            .completed
            .iter()
            .map(|(handle, association)| (*handle, *association))
            .collect::<Vec<_>>();
        for (handle, association) in associations {
            if !self.windows.contains(handle) {
                continue;
            }
            let needs_association = self
                .windows
                .get(handle)
                .is_some_and(|record| record.association.is_none());
            if needs_association {
                let _ = self.windows.mark_associated(handle, association);
            }
            if self.buffer_ready_surfaces.contains(&association.surface_id) {
                let _ = self.windows.mark_buffer_ready(handle);
                match self.resize_sync.note_commit(handle) {
                    ResizeSyncCommit::Presented | ResizeSyncCommit::FallbackPresented => self
                        .outgoing_events
                        .push_back(XwmEvent::ResizeSyncPresented(handle)),
                    ResizeSyncCommit::Deferred | ResizeSyncCommit::Ignored => {}
                }
            }
            let _ = self.emit_ready_if_complete(handle);
        }
    }

    fn emit_ready_if_complete(&mut self, handle: X11WindowHandle) -> Result<(), XwmError> {
        let snapshot = self
            .windows
            .try_ready(handle)
            .map_err(XwmError::InvalidCommand)?;
        if let Some(snapshot) = snapshot {
            self.outgoing_events
                .push_back(XwmEvent::WindowReady(snapshot));
        }
        Ok(())
    }
}

impl Drop for Xwm {
    fn drop(&mut self) {
        use x11rb::protocol::sync::ConnectionExt as _;
        use x11rb::{
            connection::Connection as _, protocol::xproto::ConnectionExt as XprotoConnectionExt,
        };

        for alarm in self.sync_alarms.values().copied() {
            let _ = self.connection.sync_destroy_alarm(alarm);
        }
        let _ = self.connection.destroy_window(self.supporting_wm_check);
        let _ = self.connection.flush();
    }
}
