//! The X11 window manager boundary.
//!
//! Raw x11rb values stay below this module.  The compositor receives only the
//! generation-bound handles, snapshots, events, and commands defined here.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    fmt,
    os::fd::RawFd,
};

use crate::compositor::{DesktopWindowKind, WindowConstraints, WindowMetadata};
use x11rb::protocol::xproto;
mod adoption;
mod association;
mod atoms;
mod capabilities;
mod commands;
mod connection;
pub mod data_bridge;
mod events;
pub(crate) mod ewmh;
pub(crate) mod focus;
pub(crate) mod icccm;
mod lifecycle;
mod ownership;
mod properties;
pub mod randr;
mod reactor;
mod resize_sync;
#[allow(dead_code)]
pub(crate) mod shape;
pub(crate) mod startup;
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
use window::X11WindowRegistry;
pub use window::{X11WindowLifecycle, X11WindowType};

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
    MaximizedHorizontal,
    MaximizedVertical,
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
    pub fields: X11ConfigureFlags,
    pub border_width: u32,
    pub sibling: Option<X11WindowHandle>,
    pub stack_mode: Option<X11StackMode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct X11ConfigureFlags {
    pub x: bool,
    pub y: bool,
    pub width: bool,
    pub height: bool,
    pub border_width: bool,
    pub sibling: bool,
    pub stack_mode: bool,
}

impl X11ConfigureFlags {
    pub const fn all() -> Self {
        Self {
            x: true,
            y: true,
            width: true,
            height: true,
            border_width: true,
            sibling: false,
            stack_mode: false,
        }
    }
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
    pub window_type: Option<X11WindowType>,
    pub override_redirect: bool,
    pub geometry: X11Geometry,
    pub metadata: WindowMetadata,
    pub constraints: WindowConstraints,
    pub state: X11PublishedState,
    pub transient_for: Option<X11WindowHandle>,
    pub supports_delete: bool,
    pub supports_take_focus: bool,
    pub accepts_input: Option<bool>,
    pub window_role: Option<String>,
    pub startup_id: Option<String>,
    pub user_time: Option<u32>,
    pub urgency: bool,
    pub sync_counter: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum X11MetadataDelta {
    Title(Option<String>),
    AppId(Option<String>),
    Pid(Option<u32>),
    Constraints(WindowConstraints),
    TransientFor(Option<X11WindowHandle>),
    WindowType(Option<X11WindowType>),
    AcceptsInput(Option<bool>),
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
    WindowMapRequested(X11WindowHandle),
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
    ConfigureNotify {
        window: X11WindowHandle,
        geometry: X11Geometry,
    },
    StateRequested {
        window: X11WindowHandle,
        request: X11StateRequest,
    },
    FocusRequested {
        window: X11WindowHandle,
        source: u32,
        timestamp: u32,
        current_time: u32,
        user_time: Option<u32>,
    },
    CloseRequestedByClient(X11WindowHandle),
    ResizeSyncAcked {
        window: X11WindowHandle,
        counter_value: u64,
    },
    ResizeSyncPresented(X11WindowHandle),
    ResizeSyncImmediate(X11WindowHandle),
    ResizeSyncTimedOut(X11WindowHandle),
}

#[derive(Debug, Clone, PartialEq)]
pub enum XwmCommand {
    Map(X11WindowHandle),
    Unmap(X11WindowHandle),
    Configure {
        window: X11WindowHandle,
        geometry: X11Geometry,
        fields: X11ConfigureFlags,
        border_width: u32,
    },
    ConfigureNotify {
        window: X11WindowHandle,
        geometry: X11Geometry,
    },
    Stack {
        window: X11WindowHandle,
        sibling: Option<X11WindowHandle>,
        mode: X11StackMode,
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
    Ownership(String),
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
            Self::Ownership(error) => write!(formatter, "XWM ownership setup failed: {error}"),
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
    RootEventMask(u32),
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
            Self::RootEventMask(mask) => write!(
                formatter,
                "XWM root event mask is incomplete: observed=0x{mask:x}"
            ),
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
    pub(crate) connection: connection::X11Connection,
    pub(crate) adoption: adoption::AdoptionTracker,
    pub(crate) screen_number: usize,
    pub(crate) root: u32,
    pub(crate) atoms: XwmAtoms,
    pub(crate) capabilities: XwmCapabilities,
    pub(crate) windows: X11WindowRegistry,
    pub(crate) outgoing_events: VecDeque<XwmEvent>,
    pub(crate) association: SurfaceAssociationJoin,
    pub(crate) resize_sync: ResizeSyncTracker,
    pub(crate) focus: focus::FocusTracker,
    pub(crate) sync_alarms: HashMap<X11WindowHandle, u32>,
    pub(crate) sync_handles_by_counter: HashMap<u32, X11WindowHandle>,
    pub(crate) next_resize_counter_values: HashMap<X11WindowHandle, u64>,
    pub(crate) expected_configures: HashMap<X11WindowHandle, X11Geometry>,
    pub(crate) immediate_resize_windows: HashSet<X11WindowHandle>,
    pub(crate) last_resize_geometries: HashMap<X11WindowHandle, X11Geometry>,
    pub(crate) shapes: HashMap<X11WindowHandle, shape::ShapeRegion>,
    pub(crate) data_bridge: data_bridge::DataBridge,
    pub(crate) randr: randr::RandrSnapshot,
    pub(crate) pending_properties:
        HashMap<x11rb::connection::SequenceNumber, properties::PendingProperty>,
    pub(crate) deferred_properties: VecDeque<properties::PendingProperty>,
    pub(crate) property_metrics: properties::PropertyMetrics,
    root_event_mask_probe: Option<x11rb::connection::SequenceNumber>,
    root_event_mask: Option<xproto::EventMask>,
    buffer_ready_surfaces: HashSet<u32>,
    pub(crate) supporting_wm_check: u32,
    raw_fd: RawFd,
}

impl Xwm {
    pub fn randr_snapshot(&self) -> &randr::RandrSnapshot {
        &self.randr
    }

    pub fn window_count(&self) -> usize {
        self.windows.len()
    }

    pub(crate) fn note_focus_in(&mut self, xid: u32) {
        self.focus.note_focus_in(xid);
    }

    pub(crate) fn note_focus_out(&mut self, xid: u32) {
        self.focus.note_focus_out(xid);
    }

    pub(crate) fn note_focus_destroyed(&mut self, xid: u32) {
        self.focus.note_destroyed(xid);
    }

    pub(crate) fn note_active_window_request(
        &mut self,
        handle: X11WindowHandle,
        timestamp: u32,
    ) -> (u32, Option<u32>) {
        let user_time = self
            .windows
            .get(handle)
            .and_then(|record| record.properties.user_time);
        self.focus.note_user_time(user_time);
        let (current_time, last_user_time) =
            self.focus.note_activation_request(handle.xid(), timestamp);
        (current_time, last_user_time.or(user_time))
    }

    pub(crate) fn note_focus_command(&mut self, handle: Option<X11WindowHandle>, timestamp: u32) {
        self.focus
            .note_focus_command(handle.map(X11WindowHandle::xid), timestamp);
    }

    pub fn required_extensions_available(&self) -> bool {
        self.capabilities.required_contract_available()
    }

    pub(crate) fn property_metrics(&self) -> properties::PropertyMetrics {
        self.property_metrics
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
        let inserted = self
            .windows
            .insert_observed_with_kind(handle, kind, geometry);
        if inserted {
            let deadline = crate::native::event_loop::monotonic_now_ns()
                .unwrap_or_default()
                .saturating_add(adoption::ADOPTION_TIMEOUT_NS);
            self.adoption
                .observe(handle, adoption::AdoptionWait::MapToAssociation, deadline);
            properties::begin_initial(self, handle)?;
        }
        Ok(inserted)
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
        properties::cancel(self, handle);
        Ok(self.windows.remove(handle).is_some())
    }

    pub(crate) fn refresh_window_properties(
        &mut self,
        handle: X11WindowHandle,
    ) -> Result<(), XwmError> {
        properties::begin_refresh(self, handle, false)
    }

    pub(crate) fn refresh_window_property(
        &mut self,
        handle: X11WindowHandle,
        kind: properties::PropertyKind,
    ) -> Result<(), XwmError> {
        properties::refresh_property(self, handle, kind)
    }

    pub(crate) fn cancel_window_properties(&mut self, handle: X11WindowHandle) {
        properties::cancel(self, handle);
    }

    pub fn window_snapshot(&self, handle: X11WindowHandle) -> Option<&X11WindowSnapshot> {
        self.windows.get(handle)?.snapshot.as_ref()
    }

    pub fn clear_generation(&mut self, generation: XwaylandGeneration) {
        self.windows.clear_generation(generation);
        self.adoption.clear_generation(generation);
        self.association.clear_generation(generation);
        self.clear_resize_sync_generation(generation);
        self.shapes
            .retain(|handle, _| handle.generation() != generation);
        self.data_bridge
            .clear_generation(data_bridge::BridgeGeneration::from(generation));
        properties::cancel_generation(self, generation);
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
        self.emit_ready_if_complete(handle).map(|_| ())
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
                ResizeSyncCommit::Presented | ResizeSyncCommit::FallbackPresented => {
                    if std::env::var_os("TYPHON_XWAYLAND_LOG").is_some() {
                        let geometry = self
                            .resize_sync
                            .transaction(handle)
                            .map(|(_, geometry, _)| geometry);
                        eprintln!(
                            "oblivion-one xwayland: event=x11_resize_presented xid={} transaction_id={} counter={} geometry={:?} latest_desired={:?}",
                            handle.xid(),
                            self.resize_sync.transaction_id(handle).unwrap_or_default(),
                            self.resize_sync
                                .state(handle)
                                .counter_value()
                                .unwrap_or_default(),
                            geometry,
                            self.resize_sync
                                .desired(handle)
                                .map(|desired| desired.geometry),
                        );
                    }
                    self.outgoing_events
                        .push_back(XwmEvent::ResizeSyncPresented(handle))
                }
                ResizeSyncCommit::Deferred | ResizeSyncCommit::Ignored => {}
            }
        }
        Ok(())
    }

    pub(crate) fn clear_surface_buffer_ready(&mut self, surface_id: u32) {
        self.buffer_ready_surfaces.remove(&surface_id);
    }

    pub(crate) fn note_sync_counter_notify(&mut self, counter: u32, value: u64) {
        let Some(handle) = self.sync_handles_by_counter.get(&counter).copied() else {
            return;
        };
        if self.resize_sync.acknowledge(handle, value) {
            if std::env::var_os("TYPHON_XWAYLAND_LOG").is_some() {
                let geometry = self
                    .resize_sync
                    .transaction(handle)
                    .map(|(_, geometry, _)| geometry);
                eprintln!(
                    "oblivion-one xwayland: event=x11_resize_acked xid={} transaction_id={} counter={} geometry={:?} latest_desired={:?}",
                    handle.xid(),
                    self.resize_sync.transaction_id(handle).unwrap_or_default(),
                    value,
                    geometry,
                    self.resize_sync
                        .desired(handle)
                        .map(|desired| desired.geometry),
                );
            }
            self.outgoing_events.push_back(XwmEvent::ResizeSyncAcked {
                window: handle,
                counter_value: value,
            });
        }
    }

    pub(crate) fn next_resize_sync_deadline_ns(&self) -> Option<u64> {
        self.resize_sync.next_deadline_ns()
    }

    pub(crate) fn handle_resize_sync_deadline(&mut self, now_ns: u64) -> Result<(), XwmError> {
        for handle in self.resize_sync.expired_handles(now_ns) {
            if self.resize_sync.timeout(handle, now_ns) {
                let transaction = self.resize_sync.transaction(handle);
                let counter_value = self
                    .resize_sync
                    .state(handle)
                    .counter_value()
                    .unwrap_or_default();
                let latest_desired = self
                    .resize_sync
                    .desired(handle)
                    .map(|desired| desired.geometry);
                let allow_result = commands::set_allow_commits(self, handle, true);
                self.clear_resize_sync_alarm(handle);
                self.resize_sync.finish_timeout(handle);
                allow_result?;
                if std::env::var_os("TYPHON_XWAYLAND_LOG").is_some() {
                    let (transaction_id, geometry, _) = transaction.unwrap_or_default();
                    eprintln!(
                        "oblivion-one xwayland: event=x11_resize_timeout xid={} transaction_id={} counter={} geometry={:?} latest_desired={:?}",
                        handle.xid(),
                        transaction_id,
                        counter_value,
                        geometry,
                        latest_desired,
                    );
                }
                self.outgoing_events
                    .push_back(XwmEvent::ResizeSyncTimedOut(handle));
                if let Some(desired) = self.resize_sync.take_desired(handle) {
                    let now = crate::native::event_loop::monotonic_now_ns().unwrap_or(now_ns);
                    commands::begin_resize_sync(
                        self,
                        handle,
                        desired.geometry,
                        0,
                        now.saturating_add(RESIZE_SYNC_TIMEOUT_NS),
                    )?;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn complete_resize_sync(&mut self, handle: X11WindowHandle) -> Result<(), XwmError> {
        if !self.resize_sync.complete(handle) {
            return Err(XwmError::InvalidCommand("resize sync is not presented"));
        }
        self.clear_resize_sync_alarm(handle);
        if let Some(desired) = self.resize_sync.take_desired(handle) {
            let now = crate::native::event_loop::monotonic_now_ns().unwrap_or_default();
            commands::begin_resize_sync(
                self,
                handle,
                desired.geometry,
                0,
                now.saturating_add(RESIZE_SYNC_TIMEOUT_NS),
            )?;
        }
        Ok(())
    }

    pub(crate) fn clear_resize_sync(&mut self, handle: X11WindowHandle) {
        self.resize_sync.clear(handle);
        self.clear_resize_sync_alarm(handle);
        self.expected_configures.remove(&handle);
        self.immediate_resize_windows.remove(&handle);
        self.last_resize_geometries.remove(&handle);
    }

    pub(crate) fn note_expected_configure(
        &mut self,
        handle: X11WindowHandle,
        geometry: X11Geometry,
    ) {
        self.expected_configures.insert(handle, geometry);
    }

    pub(crate) fn note_configure_notify(
        &mut self,
        handle: X11WindowHandle,
        geometry: X11Geometry,
    ) -> bool {
        let expected = self.expected_configures.get(&handle).copied();
        if expected == Some(geometry) {
            self.expected_configures.remove(&handle);
            true
        } else {
            false
        }
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
        self.expected_configures
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
        let deadline = crate::native::event_loop::monotonic_now_ns()
            .unwrap_or_default()
            .saturating_add(adoption::ADOPTION_TIMEOUT_NS);
        self.adoption
            .observe(handle, adoption::AdoptionWait::SerialPair, deadline);
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
        let budget = budget.min(XWM_EVENT_BUDGET);
        let mut events_processed = 0;
        let mut replies_processed = 0;
        loop {
            let event_drain = events::drain(self, budget.saturating_sub(events_processed))?;
            events_processed = events_processed.saturating_add(event_drain.processed);
            self.poll_root_event_mask()?;
            let replies = properties::poll_replies(self, budget.saturating_sub(replies_processed))?;
            replies_processed = replies_processed.saturating_add(replies);
            if event_drain.processed == 0 && replies == 0
                || events_processed == budget && replies_processed == budget
            {
                break;
            }
        }
        Ok(XwmDrain {
            processed: events_processed,
            budget_exhausted: events_processed == budget && budget != 0,
        })
    }

    pub fn execute(&mut self, command: XwmCommand) -> Result<(), XwmError> {
        commands::execute(self, command)
    }

    pub fn flush(&self) -> Result<(), XwmError> {
        commands::flush(self)?;
        let _ = self.flush_output()?;
        Ok(())
    }

    pub fn take_events(&mut self) -> impl Iterator<Item = XwmEvent> + '_ {
        self.outgoing_events.drain(..)
    }

    pub(crate) fn next_adoption_deadline_ns(&self) -> Option<u64> {
        self.adoption.next_deadline_ns()
    }

    pub(crate) fn collect_adoption_expirations(&mut self, now_ns: u64) {
        for (handle, wait) in self.adoption.expired(now_ns) {
            eprintln!(
                "oblivion-one xwayland: event=adoption_timeout window={} wait={wait:?}",
                handle.xid()
            );
        }
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
                let deadline = crate::native::event_loop::monotonic_now_ns()
                    .unwrap_or_default()
                    .saturating_add(adoption::ADOPTION_TIMEOUT_NS);
                self.adoption.observe(
                    handle,
                    adoption::AdoptionWait::AssociationToBuffer,
                    deadline,
                );
            }
            if self.buffer_ready_surfaces.contains(&association.surface_id) {
                self.adoption.complete(handle);
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

    fn emit_ready_if_complete(&mut self, handle: X11WindowHandle) -> Result<bool, XwmError> {
        let Some((properties_ready, kind, map_authorized)) = self
            .windows
            .get(handle)
            .map(|record| (record.properties_ready, record.kind, record.map_authorized))
        else {
            return Ok(false);
        };
        if properties_ready
            && kind == DesktopWindowKind::Managed
            && !map_authorized
            && self
                .windows
                .mark_map_authorized(handle)
                .map_err(XwmError::InvalidCommand)?
        {
            self.outgoing_events
                .push_back(XwmEvent::WindowMapRequested(handle));
        }
        if !properties_ready {
            return Ok(false);
        }
        let snapshot = self
            .windows
            .try_ready(handle)
            .map_err(XwmError::InvalidCommand)?;
        if let Some(snapshot) = snapshot {
            self.outgoing_events
                .push_back(XwmEvent::WindowReady(snapshot));
            return Ok(true);
        }
        Ok(false)
    }
}
