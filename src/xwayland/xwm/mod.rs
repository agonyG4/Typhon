//! Raw x11rb values stay below this module.  The compositor receives only the
//! generation-bound handles, snapshots, events, and commands defined here.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    fmt,
    os::fd::RawFd,
};

use crate::compositor::{
    DesktopWindowKind, WindowConstraints, WindowMetadata, XwaylandSurfaceCommitObserved,
};
use crate::xwayland::trace::{self, TraceFields};
use x11rb::protocol::xproto;
mod adoption;
mod api;
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
mod moveresize;
mod ownership;
mod properties;
pub mod randr;
mod reactor;
mod resize_runtime;
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
pub use moveresize::{X11MoveResizeDirection, X11MoveResizeRequest};
pub use resize_sync::{RESIZE_SYNC_TIMEOUT_NS, ResizeSyncError, ResizeSyncState};
pub(crate) use resize_sync::{ResizeSyncCommit, ResizeSyncTracker};
use window::{KindReconciliation, X11WindowRegistry};
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
    Kind(DesktopWindowKind),
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
    MoveResizeRequested {
        window: X11WindowHandle,
        request: X11MoveResizeRequest,
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
    ResizeSyncAckObserved {
        window: X11WindowHandle,
        counter_value: u64,
    },
    ResizeSyncPresented(X11WindowHandle),
    /// A transaction presented while another desired geometry still belongs to
    /// the same interactive resize chain, or while the transaction is not the
    /// final release configure.  The compositor must keep its preview active
    /// and only advance the XSync state machine.
    ResizeSyncPresentedIntermediate(X11WindowHandle),
    ResizeSyncImmediate(X11WindowHandle),
    ResizeSyncTimedOut(X11WindowHandle),
    ResizeSyncTimedOutWithFollowup(X11WindowHandle),
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
    RaiseFamily {
        family: Vec<X11WindowHandle>,
    },
    StackFamily {
        family: Vec<X11WindowHandle>,
        mode: X11StackMode,
    },
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
        final_pending: bool,
    },
    SetAllowCommits {
        window: X11WindowHandle,
        allowed: bool,
    },
    ReleaseResizeCommits {
        window: X11WindowHandle,
        counter_value: u64,
        association_serial: std::num::NonZeroU64,
        commit_floor: crate::compositor::SurfaceCommitSequence,
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
    pub(crate) timed_out_resize_counters: HashMap<X11WindowHandle, u64>,
    pub(crate) next_resize_counter_values: HashMap<X11WindowHandle, u64>,
    pub(crate) family_order: HashMap<X11WindowHandle, u64>,
    pub(crate) next_family_order: u64,
    pub(crate) expected_configures: HashMap<X11WindowHandle, X11Geometry>,
    pub(crate) immediate_resize_windows: HashSet<X11WindowHandle>,
    pub(crate) fallback_resize_windows: HashSet<X11WindowHandle>,
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
    buffer_ready_commits: Vec<XwaylandSurfaceCommitObserved>,
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

    pub(crate) fn reconcile_window_kind(
        &mut self,
        handle: X11WindowHandle,
        kind: DesktopWindowKind,
    ) -> Result<KindReconciliation, XwmError> {
        let reconciliation = self
            .windows
            .reconcile_kind(handle, kind)
            .map_err(XwmError::InvalidCommand)?;
        if matches!(reconciliation, KindReconciliation::Changed { .. })
            && self
                .windows
                .get(handle)
                .is_some_and(|record| record.snapshot.is_some())
        {
            self.outgoing_events.push_back(XwmEvent::MetadataChanged {
                window: handle,
                delta: X11MetadataDelta::Kind(kind),
            });
        }
        Ok(reconciliation)
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
            self.buffer_ready_commits.clear();
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
        trace::emit("buffer_ready_level_observed", || {
            TraceFields::new()
                .field("source", "compositor")
                .field("generation", generation.get())
                .field("surface_id", surface_id)
                .field("buffer_ready_level", true)
        });
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
        }
        self.process_pending_resize_commits();
        Ok(())
    }

    pub fn mark_surface_commit_observed(
        &mut self,
        event: XwaylandSurfaceCommitObserved,
    ) -> Result<(), XwmError> {
        if event.generation != self.generation {
            return Err(XwmError::StaleGeneration);
        }
        trace::emit("buffer_commit_edge_observed", || {
            TraceFields::new()
                .field("source", "compositor")
                .field("generation", event.generation.get())
                .field("surface_id", event.surface_id)
                .field("association_serial", event.association_serial.get())
                .field("commit_sequence", event.commit_sequence.get())
                .optional("buffer_id", event.buffer_id.map(|buffer| buffer.get()))
                .optional("buffer_width", event.buffer_size.map(|size| size.width))
                .optional("buffer_height", event.buffer_size.map(|size| size.height))
        });
        self.buffer_ready_commits.push(event);
        self.process_pending_resize_commits();
        Ok(())
    }

    pub(crate) fn process_pending_resize_commits(&mut self) {
        let pending = std::mem::take(&mut self.buffer_ready_commits);
        for event in pending {
            let handles = self
                .association
                .completed
                .iter()
                .filter_map(|(handle, association)| {
                    (association.surface_id == event.surface_id
                        && association.serial == event.association_serial)
                        .then_some(*handle)
                })
                .collect::<Vec<_>>();
            let mut retain = handles.is_empty();
            for handle in handles {
                let commit_result = self.resize_sync.note_commit(
                    handle,
                    event.association_serial,
                    event.commit_sequence,
                );
                trace::emit("resize_commit_result", || {
                    TraceFields::new()
                        .field("source", "xwm")
                        .field("xid", handle.xid())
                        .field("surface_id", event.surface_id)
                        .field("association_serial", event.association_serial.get())
                        .field("commit_sequence", event.commit_sequence.get())
                        .field("resize_result", format!("{commit_result:?}"))
                        .field(
                            "resize_state",
                            format!("{:?}", self.resize_sync.state(handle)),
                        )
                        .optional(
                            "resize_counter",
                            self.resize_sync.state(handle).counter_value(),
                        )
                });
                match commit_result {
                    ResizeSyncCommit::Presented | ResizeSyncCommit::FallbackPresented => {
                        retain = false;
                        let final_presented = self.resize_sync.transaction(handle).is_some_and(
                            |(_, _, final_pending)| {
                                final_pending && self.resize_sync.desired(handle).is_none()
                            },
                        );
                        self.outgoing_events.push_back(if final_presented {
                            XwmEvent::ResizeSyncPresented(handle)
                        } else {
                            XwmEvent::ResizeSyncPresentedIntermediate(handle)
                        });
                    }
                    ResizeSyncCommit::Deferred => retain = true,
                    ResizeSyncCommit::Ignored => {}
                }
            }
            if retain {
                self.buffer_ready_commits.push(event);
            }
        }
    }

    pub(crate) fn clear_surface_buffer_ready(&mut self, surface_id: u32) {
        self.buffer_ready_surfaces.remove(&surface_id);
        self.buffer_ready_commits
            .retain(|event| event.surface_id != surface_id);
    }

    pub(crate) fn note_sync_counter_notify(&mut self, counter: u32, value: u64) {
        let Some(handle) = self.sync_handles_by_counter.get(&counter).copied() else {
            return;
        };
        self.note_resize_sync_ack(handle, value);
    }

    pub(crate) fn note_resize_sync_ack_for_test(&mut self, handle: X11WindowHandle, value: u64) {
        self.note_resize_sync_ack(handle, value);
    }

    pub(crate) fn note_family_order(&mut self, family: &[X11WindowHandle]) {
        for handle in family {
            self.family_order.insert(*handle, self.next_family_order);
            self.next_family_order = self.next_family_order.saturating_add(1);
        }
    }

    fn note_resize_sync_ack(&mut self, handle: X11WindowHandle, value: u64) {
        let state_before = self.resize_sync.state(handle);
        if self.resize_sync.acknowledge(handle, value) {
            trace::emit("resize_ack_observed", || {
                TraceFields::new()
                    .field("source", "x11")
                    .field("xid", handle.xid())
                    .field("resize_counter", value)
                    .field("resize_state_before", format!("{state_before:?}"))
                    .field(
                        "resize_state_after",
                        format!("{:?}", self.resize_sync.state(handle)),
                    )
                    .field("allow_commits", false)
            });
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
            self.outgoing_events
                .push_back(XwmEvent::ResizeSyncAckObserved {
                    window: handle,
                    counter_value: value,
                });
        } else if self.resize_sync.sync_disabled(handle)
            && self.timed_out_resize_counters.get(&handle) == Some(&value)
        {
            self.timed_out_resize_counters.remove(&handle);
            self.resize_sync.reenable_sync(handle);
            self.clear_resize_sync_alarm(handle);
            trace::emit("resize_sync_recovered", || {
                TraceFields::new()
                    .field("source", "x11")
                    .field("xid", handle.xid())
                    .field("resize_counter", value)
                    .field(
                        "resize_state",
                        format!("{:?}", self.resize_sync.state(handle)),
                    )
            });
        }
    }
}
