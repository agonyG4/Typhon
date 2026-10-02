//! Reverse XDND target-side state. This module keeps root-target wire and
//! XdndSelection progress separate from Clipboard/PRIMARY and C2 transfers.

use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    num::NonZeroU64,
    os::fd::{AsRawFd, OwnedFd, RawFd},
};

use x11rb::{
    connection::{Connection, RequestConnection, SequenceNumber},
    cookie::Cookie,
    protocol::xproto::{self, Atom, AtomEnum, ConnectionExt as XprotoConnectionExt, Window},
    wrapper::ConnectionExt as XprotoWrapperExt,
};

use super::super::{X11WindowHandle, XwaylandGeneration, Xwm, XwmError, atoms::XwmAtomName};
use crate::xwayland::{
    MAX_PENDING_XWAYLAND_DND_INCOMING_EVENTS, XwaylandDndIncomingEvent, XwaylandDndOfferId,
};

pub(crate) const TARGET_STATUS_TIMEOUT_NS: u64 = 1_000_000_000;
pub(crate) const TARGET_METADATA_TIMEOUT_NS: u64 = 2_000_000_000;
pub(crate) const MAX_PENDING_INCOMING_DND_REPLIES: usize = 64;
pub(crate) const MAX_ACTIVE_INCOMING_DND_TRANSFERS: usize = 16;
pub(crate) const MAX_PENDING_INCOMING_DND_TRANSFER_REPLIES: usize = 4;
pub(crate) const MAX_INCOMING_DND_CHUNK_BYTES: usize = 64 * 1024;
pub(crate) const INCOMING_DND_IDLE_TIMEOUT_NS: u64 = 30_000_000_000;

const INCOMING_DND_CHUNK_UNITS: u32 = (MAX_INCOMING_DND_CHUNK_BYTES / 4) as u32;
const XDND_STATUS_WANT_POSITION_UPDATES: u32 = 1 << 1;
const MAX_WRITE_CALLS_PER_DISPATCH: usize = 32;
const MAX_EINTR_RETRIES_PER_DISPATCH: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct IncomingPosition {
    pub(crate) root_x: f64,
    pub(crate) root_y: f64,
    pub(crate) timestamp: u32,
    pub(crate) requested_action: crate::xwayland::XwaylandDndAction,
}

#[derive(Debug)]
pub(crate) struct IncomingDndSession {
    pub(crate) generation: XwaylandGeneration,
    pub(crate) offer_id: XwaylandDndOfferId,
    pub(crate) source: X11WindowHandle,
    pub(crate) logical_target_root: Window,
    pub(crate) target_proxy: Window,
    pub(crate) version: crate::xwayland::XwaylandDndVersion,
    #[allow(dead_code)] // retained for exact provisional-enter diagnostics/tests
    pub(crate) inline_mime_atoms: Vec<Atom>,
    pub(crate) mime_atoms: Vec<Atom>,
    #[allow(dead_code)] // distinguishes inline metadata from XdndTypeList replies
    pub(crate) more_types: bool,
    pub(crate) type_list_complete: bool,
    pub(crate) pending_atom_names: usize,
    pub(crate) metadata_complete: bool,
    pub(crate) mime_types: Vec<String>,
    pub(crate) atom_to_mime: BTreeMap<Atom, String>,
    pub(crate) source_actions: Vec<crate::xwayland::XwaylandDndAction>,
    pub(crate) available_actions: Vec<crate::xwayland::XwaylandDndAction>,
    pub(crate) action_list_required: bool,
    pub(crate) action_list_queried: bool,
    pub(crate) action_list_complete: bool,
    pub(crate) latest_position: Option<IncomingPosition>,
    pub(crate) canonical_started: bool,
    pub(crate) pending_status_deadline_ns: Option<u64>,
    pub(crate) status_pending: bool,
    pub(crate) accepted_mime: Option<String>,
    pub(crate) selected_action: Option<crate::xwayland::XwaylandDndAction>,
    pub(crate) metadata_deadline_ns: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IncomingTransferPhase {
    AwaitSelectionNotify,
    ReadingProperty,
    WaitingForIncrValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IncomingPropertyMode {
    Initial,
    Direct,
    Incr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContinueAfterWrite {
    ReadMoreProperty,
    ReadNextIncrChunk,
    Finish,
    None,
}

#[derive(Debug)]
struct IncomingTransfer {
    id: crate::xwayland::XwaylandDndIncomingTransferId,
    generation: XwaylandGeneration,
    offer_id: XwaylandDndOfferId,
    source: Window,
    requestor: Window,
    target: Atom,
    property: Atom,
    selection_timestamp: u32,
    sink: Option<OwnedFd>,
    phase: IncomingTransferPhase,
    mode: IncomingPropertyMode,
    offset_units: u32,
    bytes_after: u32,
    expected_type_format: Option<(Atom, u8)>,
    buffer: Vec<u8>,
    written: usize,
    continue_after_write: ContinueAfterWrite,
    idle_deadline_ns: u64,
    sink_writable_interest: bool,
    reactor_token: Option<u64>,
    pending_reply: Option<SequenceNumber>,
}

#[derive(Debug, Default)]
pub(crate) struct DndIncomingManager {
    pub(crate) generation: Option<XwaylandGeneration>,
    pub(crate) target_proxy: Option<Window>,
    pub(crate) next_offer_serial: u64,
    pub(crate) next_transfer_serial: u64,
    pub(crate) events: VecDeque<XwaylandDndIncomingEvent>,
    pub(crate) pending: BTreeMap<SequenceNumber, PendingMetadataReply>,
    transfers: BTreeMap<crate::xwayland::XwaylandDndIncomingTransferId, IncomingTransfer>,
    requestors: HashMap<Window, crate::xwayland::XwaylandDndIncomingTransferId>,
    transfer_replies: BTreeMap<SequenceNumber, crate::xwayland::XwaylandDndIncomingTransferId>,
    retired_requestors: HashSet<Window>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum PendingMetadataReply {
    TypeList {
        offer_id: XwaylandDndOfferId,
        deadline_ns: u64,
    },
    ActionList {
        offer_id: XwaylandDndOfferId,
        deadline_ns: u64,
    },
    AtomName {
        offer_id: XwaylandDndOfferId,
        atom: Atom,
        deadline_ns: u64,
    },
}

impl DndIncomingManager {
    pub(crate) fn allocate_offer_id(
        &mut self,
        generation: XwaylandGeneration,
    ) -> Option<XwaylandDndOfferId> {
        if self.generation != Some(generation) {
            self.generation = Some(generation);
            self.next_offer_serial = 0;
        }
        self.next_offer_serial = self.next_offer_serial.checked_add(1)?;
        NonZeroU64::new(self.next_offer_serial)
            .map(|serial| XwaylandDndOfferId::new(generation, serial))
    }

    pub(crate) fn push_event(&mut self, event: XwaylandDndIncomingEvent) -> bool {
        if let XwaylandDndIncomingEvent::Position { offer_id, .. } = &event
            && let Some(existing) = self.events.iter_mut().rev().find(|entry| {
                matches!(entry, XwaylandDndIncomingEvent::Position { offer_id: queued, .. } if queued == offer_id)
            })
        {
            *existing = event;
            return true;
        }
        let is_position = matches!(event, XwaylandDndIncomingEvent::Position { .. });
        let is_begin = matches!(event, XwaylandDndIncomingEvent::Begin { .. });
        // Every admitted Begin reserves one bounded slot for its eventual
        // Leave. Coalesced Positions cannot consume that edge capacity.
        let limit = if is_position || is_begin {
            MAX_PENDING_XWAYLAND_DND_INCOMING_EVENTS.saturating_sub(1)
        } else {
            MAX_PENDING_XWAYLAND_DND_INCOMING_EVENTS
        };
        if self.events.len() >= limit {
            if !is_position
                && let Some(index) = self
                    .events
                    .iter()
                    .position(|queued| matches!(queued, XwaylandDndIncomingEvent::Position { .. }))
            {
                self.events.remove(index);
            }
            if self.events.len() >= limit {
                return false;
            }
        }
        self.events.push_back(event);
        true
    }

    pub(crate) fn take_events(&mut self) -> Vec<XwaylandDndIncomingEvent> {
        self.events.drain(..).collect()
    }

    fn allocate_transfer_id(
        &mut self,
        offer_id: XwaylandDndOfferId,
    ) -> Option<crate::xwayland::XwaylandDndIncomingTransferId> {
        self.next_transfer_serial = self.next_transfer_serial.checked_add(1)?;
        NonZeroU64::new(self.next_transfer_serial)
            .map(|serial| crate::xwayland::XwaylandDndIncomingTransferId::new(offer_id, serial))
    }

    pub(crate) fn clear_generation(&mut self, generation: XwaylandGeneration) {
        if self.generation != Some(generation) {
            return;
        }
        self.generation = None;
        self.target_proxy = None;
        self.pending.clear();
        self.events.clear();
        self.transfers.clear();
        self.requestors.clear();
        self.transfer_replies.clear();
        self.retired_requestors.clear();
    }

    pub(crate) fn sink_interests(
        &self,
    ) -> impl Iterator<Item = (crate::xwayland::XwaylandDndIncomingTransferId, RawFd)> + '_ {
        self.transfers.values().filter_map(|transfer| {
            (transfer.sink_writable_interest && transfer.sink.is_some())
                .then(|| {
                    transfer
                        .sink
                        .as_ref()
                        .map(|sink| (transfer.id, sink.as_raw_fd()))
                })
                .flatten()
        })
    }

    pub(crate) fn bind_reactor_token(
        &mut self,
        id: crate::xwayland::XwaylandDndIncomingTransferId,
        token: Option<u64>,
    ) {
        if let Some(transfer) = self.transfers.get_mut(&id) {
            transfer.reactor_token = token;
        }
    }

    fn transfer_matches_reactor(
        &self,
        id: crate::xwayland::XwaylandDndIncomingTransferId,
        generation: XwaylandGeneration,
        token: u64,
    ) -> bool {
        self.transfers.get(&id).is_some_and(|transfer| {
            transfer.generation == generation
                && transfer.reactor_token == Some(token)
                && transfer.sink.is_some()
        })
    }

    pub(crate) fn owns_requestor(&self, window: Window) -> bool {
        self.requestors.contains_key(&window) || self.retired_requestors.contains(&window)
    }
}

/// Create a private, 1x1 InputOnly protocol window. It is never mapped or
/// registered in the desktop window registry. C3-A deliberately does not
/// install the root XdndProxy property.
pub(crate) fn initialize_target_proxy(xwm: &mut Xwm) -> Result<(), XwmError> {
    if xwm.data_bridge.dnd_incoming.generation == Some(xwm.generation)
        && xwm.data_bridge.dnd_incoming.target_proxy.is_some()
    {
        return Ok(());
    }
    let proxy = xwm
        .connection
        .generate_id()
        .map_err(|error| XwmError::IdAllocation(error.to_string()))?;
    let create = xwm
        .connection
        .create_window(
            0,
            proxy,
            xwm.root,
            0,
            0,
            1,
            1,
            0,
            xproto::WindowClass::INPUT_ONLY,
            0,
            &xproto::CreateWindowAux::new().event_mask(
                xproto::EventMask::PROPERTY_CHANGE | xproto::EventMask::STRUCTURE_NOTIFY,
            ),
        )
        .map_err(XwmError::Connection)?;
    std::mem::forget(create);
    let self_proxy = xwm
        .connection
        .change_property32(
            xproto::PropMode::REPLACE,
            proxy,
            xwm.atoms.get(XwmAtomName::XdndProxy),
            xproto::AtomEnum::WINDOW,
            &[proxy],
        )
        .map_err(XwmError::Connection)?;
    std::mem::forget(self_proxy);
    let aware = xwm
        .connection
        .change_property32(
            xproto::PropMode::REPLACE,
            proxy,
            xwm.atoms.get(XwmAtomName::XdndAware),
            xproto::AtomEnum::ATOM,
            &[5],
        )
        .map_err(XwmError::Connection)?;
    std::mem::forget(aware);
    xwm.data_bridge.dnd.internal_windows.insert(proxy);
    xwm.data_bridge.dnd_incoming.generation = Some(xwm.generation);
    xwm.data_bridge.dnd_incoming.target_proxy = Some(proxy);
    Ok(())
}

pub(crate) fn target_proxy(xwm: &Xwm) -> Option<Window> {
    (xwm.data_bridge.dnd_incoming.generation == Some(xwm.generation))
        .then_some(xwm.data_bridge.dnd_incoming.target_proxy)
        .flatten()
}

#[allow(dead_code)] // held for the C3-B product activation gate
fn root_proxy_may_be_replaced(existing: Option<Window>, valid: bool, own_proxy: Window) -> bool {
    existing.is_none() || existing == Some(own_proxy) || !valid
}

#[allow(dead_code)] // held for the C3-B product activation gate
fn root_proxy_should_be_released(current: Option<Window>, own_proxy: Window) -> bool {
    current == Some(own_proxy)
}

#[allow(dead_code)] // used by root-proxy ownership in C3-B
fn read_single_u32_property(
    xwm: &Xwm,
    window: Window,
    property: Atom,
    property_type: Atom,
) -> Result<Option<u32>, XwmError> {
    let cookie = xwm
        .connection
        .get_property(false, window, property, AtomEnum::ANY, 0, 2)
        .map_err(XwmError::Connection)?;
    let reply = cookie.reply_unchecked().map_err(XwmError::Connection)?;
    let Some(reply) = reply.filter(|reply| {
        reply.type_ == property_type && reply.format == 32 && reply.bytes_after == 0
    }) else {
        return Ok(None);
    };
    let values = reply
        .value32()
        .map(|values| values.collect::<Vec<_>>())
        .unwrap_or_default();
    Ok((values.len() == 1).then_some(values[0]))
}

#[allow(dead_code)] // used by root-proxy ownership in C3-B
fn foreign_root_proxy_is_valid(xwm: &Xwm, proxy: Window) -> Result<bool, XwmError> {
    let self_proxy = read_single_u32_property(
        xwm,
        proxy,
        xwm.atoms.get(XwmAtomName::XdndProxy),
        u32::from(AtomEnum::WINDOW),
    )?;
    if self_proxy != Some(proxy) {
        return Ok(false);
    }
    let aware = read_single_u32_property(
        xwm,
        proxy,
        xwm.atoms.get(XwmAtomName::XdndAware),
        u32::from(AtomEnum::ATOM),
    )?;
    Ok(aware.is_some_and(|version| version >= 4))
}

/// Future C3-B root-discovery acquisition primitive. It is deliberately not
/// called from C3-A startup, so production root drop discovery stays off.
#[allow(dead_code)] // product discovery intentionally remains disabled until C3-B
pub(crate) fn acquire_root_proxy(xwm: &mut Xwm) -> Result<bool, XwmError> {
    let Some(proxy) = target_proxy(xwm) else {
        return Ok(false);
    };
    let grab = xwm.connection.grab_server().map_err(XwmError::Connection)?;
    std::mem::forget(grab);
    let result = (|| {
        let current = read_single_u32_property(
            xwm,
            xwm.root,
            xwm.atoms.get(XwmAtomName::XdndProxy),
            u32::from(AtomEnum::WINDOW),
        )?;
        let valid = current
            .filter(|current| *current != proxy)
            .map(|current| foreign_root_proxy_is_valid(xwm, current))
            .transpose()?
            .unwrap_or(false);
        if !root_proxy_may_be_replaced(current, valid, proxy) {
            return Ok(false);
        }
        let cookie = xwm
            .connection
            .change_property32(
                xproto::PropMode::REPLACE,
                xwm.root,
                xwm.atoms.get(XwmAtomName::XdndProxy),
                AtomEnum::WINDOW,
                &[proxy],
            )
            .map_err(XwmError::Connection)?;
        std::mem::forget(cookie);
        Ok(true)
    })();
    let ungrab = match xwm.connection.ungrab_server() {
        Ok(cookie) => {
            std::mem::forget(cookie);
            Ok(())
        }
        Err(error) => Err(XwmError::Connection(error)),
    };
    xwm.connection.flush().map_err(XwmError::Connection)?;
    match result {
        Err(error) => Err(error),
        Ok(acquired) => {
            ungrab?;
            Ok(acquired)
        }
    }
}

/// Release root discovery only while the property still names our exact
/// generation-owned proxy.
#[allow(dead_code)] // product discovery intentionally remains disabled until C3-B
pub(crate) fn release_root_proxy(xwm: &mut Xwm) -> Result<bool, XwmError> {
    let Some(proxy) = xwm.data_bridge.dnd_incoming.target_proxy else {
        return Ok(false);
    };
    let grab = xwm.connection.grab_server().map_err(XwmError::Connection)?;
    std::mem::forget(grab);
    let result = (|| {
        let current = read_single_u32_property(
            xwm,
            xwm.root,
            xwm.atoms.get(XwmAtomName::XdndProxy),
            u32::from(AtomEnum::WINDOW),
        )?;
        if !root_proxy_should_be_released(current, proxy) {
            return Ok(false);
        }
        let cookie = xwm
            .connection
            .delete_property(xwm.root, xwm.atoms.get(XwmAtomName::XdndProxy))
            .map_err(XwmError::Connection)?;
        std::mem::forget(cookie);
        Ok(true)
    })();
    let ungrab = match xwm.connection.ungrab_server() {
        Ok(cookie) => {
            std::mem::forget(cookie);
            Ok(())
        }
        Err(error) => Err(XwmError::Connection(error)),
    };
    xwm.connection.flush().map_err(XwmError::Connection)?;
    match result {
        Err(error) => Err(error),
        Ok(released) => {
            ungrab?;
            Ok(released)
        }
    }
}

pub(crate) fn owns_message_target(xwm: &Xwm, message: &xproto::ClientMessageEvent) -> bool {
    target_proxy(xwm) == Some(message.window)
}

pub(crate) fn is_exact_source(
    session: &IncomingDndSession,
    generation: XwaylandGeneration,
    source: Window,
    logical_target: Window,
    proxy: Window,
) -> bool {
    session.generation == generation
        && session.source.xid() == source
        && session.logical_target_root == logical_target
        && session.target_proxy == proxy
}

pub(crate) fn retire_proxy(xwm: &mut Xwm, generation: XwaylandGeneration) {
    if xwm.data_bridge.dnd_incoming.generation != Some(generation) {
        return;
    }
    let _ = release_root_proxy(xwm);
    let requestors = xwm
        .data_bridge
        .dnd_incoming
        .requestors
        .keys()
        .copied()
        .collect::<Vec<_>>();
    for requestor in requestors {
        let _ = xwm.connection.destroy_window(requestor);
    }
    let sequences = xwm
        .data_bridge
        .dnd_incoming
        .pending
        .keys()
        .chain(xwm.data_bridge.dnd_incoming.transfer_replies.keys())
        .copied()
        .collect::<Vec<_>>();
    for sequence in sequences {
        xwm.connection.discard_reply(
            sequence,
            x11rb::connection::RequestKind::HasResponse,
            x11rb::connection::DiscardMode::DiscardReply,
        );
    }
    if let Some(proxy) = xwm.data_bridge.dnd_incoming.target_proxy.take() {
        xwm.data_bridge.dnd.internal_windows.remove(&proxy);
        let _ = xwm.connection.destroy_window(proxy);
    }
    xwm.data_bridge.dnd_incoming.clear_generation(generation);
}

pub(crate) fn client_message(
    xwm: &mut Xwm,
    event: xproto::ClientMessageEvent,
    now_ns: u64,
) -> Result<bool, XwmError> {
    let message_type = event.type_;
    let is_incoming = [
        XwmAtomName::XdndEnter,
        XwmAtomName::XdndPosition,
        XwmAtomName::XdndLeave,
        XwmAtomName::XdndDrop,
    ]
    .into_iter()
    .any(|name| message_type == xwm.atoms.get(name));
    if !is_incoming {
        return Ok(false);
    }
    // These messages are consumed before generic ClientMessage normalization,
    // including malformed or misaddressed messages, so a source XID can never
    // be adopted as an ordinary DesktopWindow through XDND traffic.
    if !owns_message_target(xwm, &event) {
        return Ok(false);
    }
    if event.format != 32 {
        return Ok(true);
    }
    let data = event.data.as_data32();
    if message_type == xwm.atoms.get(XwmAtomName::XdndEnter) {
        metadata::begin_enter(xwm, data, now_ns)?;
    } else if message_type == xwm.atoms.get(XwmAtomName::XdndPosition) {
        metadata::position(xwm, data, now_ns)?;
    } else if message_type == xwm.atoms.get(XwmAtomName::XdndLeave) {
        metadata::leave(xwm, data[0])?;
    } else if message_type == xwm.atoms.get(XwmAtomName::XdndDrop) {
        // C3-A has no terminal authority. Treat a synthetic Drop as a
        // fail-closed leave and never call the canonical drop contract.
        metadata::leave(xwm, data[0])?;
    }
    Ok(true)
}

mod metadata;
#[cfg(test)]
mod tests;
mod transfer;

#[cfg(test)]
use metadata::representable_source_actions;
pub(crate) use metadata::{expire_deadlines, next_deadline_ns, poll_replies, source_feedback};
pub(crate) use transfer::{
    canonical_retired, handle_sink_ready, property_notify, requestor_destroyed, selection_notify,
    source_destroyed, start_data_request,
};
#[cfg(test)]
use transfer::{next_after_property_chunk, write_sink_bytes};
