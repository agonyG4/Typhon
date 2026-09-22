//! Bounded X11 selection-property to Wayland-FD transfers.
//!
//! This module owns incoming X11 payload transactions and their private
//! requestor windows. It deliberately does not use the generic FD transfer
//! manager: INCR acknowledgement is the backpressure protocol.

use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    io,
    num::NonZeroU64,
    os::fd::{AsRawFd, OwnedFd},
};

use x11rb::{
    CURRENT_TIME,
    connection::{Connection, DiscardMode, RequestConnection, RequestKind, SequenceNumber},
    cookie::Cookie,
    protocol::xproto::{self, Atom, ConnectionExt as XprotoConnectionExt, Window, WindowClass},
};

use crate::xwayland::XwaylandSelectionDataRequest;

use super::{
    Xwm, XwmError,
    atoms::XwmAtomName,
    connection::X11Connection,
    data_bridge::{BridgeGeneration, SelectionKind},
    selection_wire::resolve_payload_target,
};

pub(crate) const MAX_ACTIVE_SELECTION_PAYLOAD_TRANSFERS: usize = 64;
pub(crate) const MAX_SELECTION_PAYLOAD_SLOTS_PER_CHANNEL: usize = 64;
pub(crate) const MAX_PENDING_SELECTION_PAYLOAD_REPLIES: usize = 4;
pub(crate) const MAX_SELECTION_PAYLOAD_CHUNK_BYTES: usize = 64 * 1024;
pub(crate) const SELECTION_PAYLOAD_IDLE_TIMEOUT_NS: u64 = 30_000_000_000;

const SELECTION_PAYLOAD_CHUNK_UNITS: u32 = (MAX_SELECTION_PAYLOAD_CHUNK_BYTES / 4) as u32;
const MAX_WRITE_CALLS_PER_DISPATCH: usize = 32;
const MAX_EINTR_RETRIES_PER_DISPATCH: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SelectionPayloadTransferId(NonZeroU64);

impl SelectionPayloadTransferId {
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlotSafety {
    Clean,
    Poisoned,
}

#[derive(Debug)]
struct PayloadRequestorSlot {
    window: Window,
    safety: SlotSafety,
    active: Option<SelectionPayloadTransferId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransferPhase {
    AwaitSelectionNotify,
    ReadingProperty,
    WaitingForIncrNewValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadMode {
    Initial,
    Direct,
    IncrChunk,
}

#[derive(Debug)]
struct PropertyRead {
    mode: ReadMode,
    offset: u32,
    expected_type_format: Option<(Atom, u8)>,
    previous_bytes_after: Option<u32>,
    bytes_read: u64,
}

#[derive(Debug)]
struct SelectionPayloadTransfer {
    id: SelectionPayloadTransferId,
    generation: BridgeGeneration,
    kind: SelectionKind,
    offer_id: crate::xwayland::XwaylandSelectionOfferId,
    target: Atom,
    requestor: Window,
    property: Atom,
    sink: Option<OwnedFd>,
    phase: TransferPhase,
    drain_only: bool,
    read: Option<PropertyRead>,
    pending_reply: Option<SequenceNumber>,
    last_request_sequence: SequenceNumber,
    buffer: Vec<u8>,
    written: usize,
    idle_deadline_ns: u64,
    sink_writable_interest: bool,
    reactor_token: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
struct PendingPayloadReply {
    transfer_id: SelectionPayloadTransferId,
    generation: BridgeGeneration,
}

#[derive(Debug, Default)]
pub(crate) struct SelectionPayloadManager {
    active_generation: Option<BridgeGeneration>,
    slots: [Vec<PayloadRequestorSlot>; 2],
    transfers: BTreeMap<SelectionPayloadTransferId, SelectionPayloadTransfer>,
    pending: BTreeMap<SequenceNumber, PendingPayloadReply>,
    ready_to_read: VecDeque<SelectionPayloadTransferId>,
    ready_set: HashSet<SelectionPayloadTransferId>,
    next_transfer_id: u64,
}

impl SelectionPayloadManager {
    pub(crate) fn initialize_generation(&mut self, generation: BridgeGeneration) {
        if self.active_generation == Some(generation) {
            return;
        }
        self.clear();
        self.active_generation = Some(generation);
    }

    fn clear(&mut self) -> Vec<SequenceNumber> {
        let sequences = self.pending.keys().copied().collect();
        self.active_generation = None;
        self.slots = [Vec::new(), Vec::new()];
        self.transfers.clear();
        self.pending.clear();
        self.ready_to_read.clear();
        self.ready_set.clear();
        sequences
    }

    pub(crate) fn clear_generation(&mut self, generation: BridgeGeneration) -> Vec<SequenceNumber> {
        if self.active_generation != Some(generation) {
            return Vec::new();
        }
        self.clear()
    }

    pub(crate) fn is_internal_window(&self, window: Window) -> bool {
        self.slots
            .iter()
            .flatten()
            .any(|slot| slot.window == window)
    }

    fn slot_index(kind: SelectionKind) -> usize {
        match kind {
            SelectionKind::Clipboard => 0,
            SelectionKind::Primary => 1,
        }
    }

    fn allocate_transfer_id(&mut self) -> Option<SelectionPayloadTransferId> {
        let value = self.next_transfer_id.checked_add(1)?;
        let id = NonZeroU64::new(value)?;
        self.next_transfer_id = value;
        Some(SelectionPayloadTransferId(id))
    }

    fn clean_idle_slot(&self, kind: SelectionKind) -> Option<Window> {
        self.slots[Self::slot_index(kind)]
            .iter()
            .find(|slot| slot.safety == SlotSafety::Clean && slot.active.is_none())
            .map(|slot| slot.window)
    }

    fn create_slot(&mut self, kind: SelectionKind, window: Window) {
        self.slots[Self::slot_index(kind)].push(PayloadRequestorSlot {
            window,
            safety: SlotSafety::Clean,
            active: None,
        });
        debug_assert!(
            self.slots[Self::slot_index(kind)].len() <= MAX_SELECTION_PAYLOAD_SLOTS_PER_CHANNEL
        );
        debug_assert!(
            self.slots.iter().map(Vec::len).sum::<usize>()
                <= 2 * MAX_SELECTION_PAYLOAD_SLOTS_PER_CHANNEL
        );
    }

    fn bind_transfer(&mut self, transfer: SelectionPayloadTransfer) {
        let id = transfer.id;
        let requestor = transfer.requestor;
        let kind = transfer.kind;
        let slot = self.slots[Self::slot_index(kind)]
            .iter_mut()
            .find(|slot| slot.window == requestor)
            .expect("payload slot was selected before transfer binding");
        debug_assert_eq!(slot.safety, SlotSafety::Clean);
        debug_assert!(slot.active.is_none());
        slot.active = Some(id);
        self.transfers.insert(id, transfer);
        self.debug_assert_invariants();
    }

    fn update_slot_after_transfer(
        &mut self,
        transfer: &SelectionPayloadTransfer,
        safety: SlotSafety,
    ) {
        let slot = self.slots[Self::slot_index(transfer.kind)]
            .iter_mut()
            .find(|slot| slot.window == transfer.requestor)
            .expect("active payload transfer has a requestor slot");
        slot.safety = safety;
        slot.active = None;
    }

    fn queue_read(&mut self, id: SelectionPayloadTransferId) {
        let Some(transfer) = self.transfers.get(&id) else {
            return;
        };
        if transfer.phase != TransferPhase::ReadingProperty
            || transfer.read.is_none()
            || transfer.pending_reply.is_some()
            || !transfer.buffer.is_empty()
            || self.ready_set.contains(&id)
        {
            return;
        }
        self.ready_set.insert(id);
        self.ready_to_read.push_back(id);
    }

    fn next_ready_read(&mut self) -> Option<(SelectionPayloadTransferId, Window, Atom, u32)> {
        while let Some(id) = self.ready_to_read.pop_front() {
            self.ready_set.remove(&id);
            let Some(transfer) = self.transfers.get(&id) else {
                continue;
            };
            if transfer.phase != TransferPhase::ReadingProperty
                || transfer.pending_reply.is_some()
                || !transfer.buffer.is_empty()
            {
                continue;
            }
            let Some(read) = transfer.read.as_ref() else {
                continue;
            };
            return Some((id, transfer.requestor, transfer.property, read.offset));
        }
        None
    }

    fn note_pending_reply(
        &mut self,
        id: SelectionPayloadTransferId,
        generation: BridgeGeneration,
        sequence: SequenceNumber,
    ) {
        if let Some(transfer) = self.transfers.get_mut(&id) {
            transfer.pending_reply = Some(sequence);
            self.pending.insert(
                sequence,
                PendingPayloadReply {
                    transfer_id: id,
                    generation,
                },
            );
        }
        self.debug_assert_invariants();
    }

    fn take_pending_reply(&mut self, sequence: SequenceNumber) -> Option<PendingPayloadReply> {
        let pending = self.pending.remove(&sequence)?;
        if let Some(transfer) = self.transfers.get_mut(&pending.transfer_id)
            && transfer.pending_reply == Some(sequence)
        {
            transfer.pending_reply = None;
        }
        Some(pending)
    }

    fn rearm_deadline(&mut self, id: SelectionPayloadTransferId, now_ns: u64) {
        if let Some(transfer) = self.transfers.get_mut(&id) {
            transfer.idle_deadline_ns = now_ns.saturating_add(SELECTION_PAYLOAD_IDLE_TIMEOUT_NS);
        }
    }

    pub(crate) fn next_deadline_ns(&self) -> Option<u64> {
        self.transfers
            .values()
            .map(|transfer| transfer.idle_deadline_ns)
            .min()
    }

    pub(crate) fn expire_deadlines(&mut self, now_ns: u64) -> Vec<SequenceNumber> {
        let expired = self
            .transfers
            .values()
            .filter_map(|transfer| (now_ns >= transfer.idle_deadline_ns).then_some(transfer.id))
            .collect::<Vec<_>>();
        let mut sequences = Vec::new();
        for id in expired {
            if let Some(transfer) = self.transfers.remove(&id) {
                if let Some(sequence) = transfer.pending_reply {
                    self.pending.remove(&sequence);
                    sequences.push(sequence);
                }
                self.ready_set.remove(&id);
                self.update_slot_after_transfer(&transfer, SlotSafety::Poisoned);
            }
        }
        self.ready_to_read
            .retain(|id| self.transfers.contains_key(id));
        sequences
    }

    pub(crate) fn sink_interests(
        &self,
    ) -> impl Iterator<Item = (SelectionPayloadTransferId, i32)> + '_ {
        self.transfers.values().filter_map(|transfer| {
            (transfer.sink_writable_interest && !transfer.drain_only)
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
        id: SelectionPayloadTransferId,
        reactor_token: Option<u64>,
    ) {
        if let Some(transfer) = self.transfers.get_mut(&id) {
            transfer.reactor_token = reactor_token;
        }
    }

    pub(crate) fn transfer_matches_reactor(
        &self,
        id: SelectionPayloadTransferId,
        generation: BridgeGeneration,
        reactor_token: u64,
    ) -> bool {
        self.transfers.get(&id).is_some_and(|transfer| {
            transfer.generation == generation
                && transfer.reactor_token == Some(reactor_token)
                && transfer.sink.is_some()
                && !transfer.drain_only
        })
    }

    pub(crate) fn active_count(&self) -> usize {
        self.transfers.len()
    }

    pub(crate) fn pending_reply_count(&self) -> usize {
        self.pending.len()
    }

    #[cfg(test)]
    pub(crate) fn pending_sequence_for_test(
        &self,
        id: SelectionPayloadTransferId,
    ) -> Option<SequenceNumber> {
        self.transfers.get(&id)?.pending_reply
    }

    #[cfg(test)]
    pub(crate) fn last_request_sequence_for_test(
        &self,
        id: SelectionPayloadTransferId,
    ) -> Option<SequenceNumber> {
        self.transfers
            .get(&id)
            .map(|transfer| transfer.last_request_sequence)
    }

    pub(crate) fn slot_count(&self, kind: SelectionKind) -> usize {
        self.slots[Self::slot_index(kind)].len()
    }

    fn debug_assert_invariants(&self) {
        debug_assert!(self.active_count() <= MAX_ACTIVE_SELECTION_PAYLOAD_TRANSFERS);
        debug_assert!(self.pending_reply_count() <= MAX_PENDING_SELECTION_PAYLOAD_REPLIES);
        debug_assert!(
            self.slots
                .iter()
                .all(|slots| slots.len() <= MAX_SELECTION_PAYLOAD_SLOTS_PER_CHANNEL)
        );
        debug_assert!(self.transfers.iter().all(|(id, transfer)| {
            *id == transfer.id
                && transfer.offer_id.kind
                    == match transfer.kind {
                        SelectionKind::Clipboard => {
                            crate::xwayland::XwaylandSelectionKind::Clipboard
                        }
                        SelectionKind::Primary => crate::xwayland::XwaylandSelectionKind::Primary,
                    }
                && BridgeGeneration::from(transfer.offer_id.generation) == transfer.generation
                && transfer.buffer.len() <= MAX_SELECTION_PAYLOAD_CHUNK_BYTES
                && transfer.written <= transfer.buffer.len()
        }));
    }
}

pub(crate) fn start_request(
    xwm: &mut Xwm,
    request: XwaylandSelectionDataRequest,
    now_ns: u64,
) -> Result<Option<SelectionPayloadTransferId>, XwmError> {
    let offer_id = request.offer_id;
    let generation = BridgeGeneration::from(offer_id.generation);
    if generation != BridgeGeneration::from(xwm.generation)
        || xwm.data_bridge.selection_payloads.active_generation != Some(generation)
    {
        return Ok(None);
    }
    let Some((kind, target)) = resolve_payload_target(
        &xwm.data_bridge.selection_wire,
        offer_id,
        &request.mime_type,
    ) else {
        return Ok(None);
    };
    if xwm.data_bridge.selection_payloads.active_count() >= MAX_ACTIVE_SELECTION_PAYLOAD_TRANSFERS {
        return Ok(None);
    }
    if set_fd_nonblocking(&request.sink).is_err() {
        return Ok(None);
    }

    let requestor =
        if let Some(requestor) = xwm.data_bridge.selection_payloads.clean_idle_slot(kind) {
            requestor
        } else {
            let slot_count = xwm.data_bridge.selection_payloads.slot_count(kind);
            if slot_count >= MAX_SELECTION_PAYLOAD_SLOTS_PER_CHANNEL {
                return Ok(None);
            }
            let requestor = xwm
                .connection
                .generate_id()
                .map_err(|error| XwmError::IdAllocation(error.to_string()))?;
            create_payload_requestor(xwm, requestor)?;
            xwm.data_bridge
                .selection_wire
                .register_payload_requestor_window(requestor);
            xwm.data_bridge
                .selection_payloads
                .create_slot(kind, requestor);
            requestor
        };
    let Some(id) = xwm.data_bridge.selection_payloads.allocate_transfer_id() else {
        return Ok(None);
    };
    let property = xwm.atoms.get(XwmAtomName::SelectionData);
    let transfer = SelectionPayloadTransfer {
        id,
        generation,
        kind,
        offer_id,
        target,
        requestor,
        property,
        sink: Some(request.sink),
        phase: TransferPhase::AwaitSelectionNotify,
        drain_only: false,
        read: None,
        pending_reply: None,
        last_request_sequence: 0,
        buffer: Vec::new(),
        written: 0,
        idle_deadline_ns: now_ns.saturating_add(SELECTION_PAYLOAD_IDLE_TIMEOUT_NS),
        sink_writable_interest: false,
        reactor_token: None,
    };
    xwm.data_bridge.selection_payloads.bind_transfer(transfer);

    let selection = selection_atom(xwm, kind);
    match issue_convert_selection(xwm, requestor, selection, target, property) {
        Ok(sequence) => {
            if let Some(transfer) = xwm.data_bridge.selection_payloads.transfers.get_mut(&id) {
                transfer.last_request_sequence = sequence;
            }
        }
        Err(error) => {
            finish_transfer(xwm, id, SlotSafety::Clean);
            return Err(XwmError::Connection(error));
        }
    }
    Ok(Some(id))
}

fn issue_convert_selection(
    xwm: &Xwm,
    requestor: Window,
    selection: Atom,
    target: Atom,
    property: Atom,
) -> Result<SequenceNumber, x11rb::errors::ConnectionError> {
    let cookie =
        xwm.connection
            .convert_selection(requestor, selection, target, property, CURRENT_TIME)?;
    let sequence = cookie.sequence_number();
    std::mem::forget(cookie);
    Ok(sequence)
}

pub(crate) fn handle_sink_ready(
    xwm: &mut Xwm,
    id: SelectionPayloadTransferId,
    generation: BridgeGeneration,
    reactor_token: u64,
    flags: u32,
    now_ns: u64,
) -> Result<bool, XwmError> {
    if xwm.data_bridge.selection_payloads.active_generation != Some(generation)
        || !xwm.data_bridge.selection_payloads.transfer_matches_reactor(
            id,
            generation,
            reactor_token,
        )
    {
        return Ok(false);
    }
    let terminal = libc::EPOLLERR as u32 | libc::EPOLLHUP as u32 | libc::EPOLLRDHUP as u32;
    let before = xwm
        .data_bridge
        .selection_payloads
        .transfers
        .get(&id)
        .map(|transfer| (transfer.sink_writable_interest, transfer.reactor_token));
    if flags & terminal != 0 {
        enter_drain_only(xwm, id, now_ns)?;
    } else if flags & libc::EPOLLOUT as u32 != 0 {
        write_buffer_to_sink(xwm, id, now_ns)?;
    }
    let after = xwm
        .data_bridge
        .selection_payloads
        .transfers
        .get(&id)
        .map(|transfer| (transfer.sink_writable_interest, transfer.reactor_token));
    Ok(before != after)
}

fn create_payload_requestor(xwm: &Xwm, requestor: Window) -> Result<(), XwmError> {
    let attributes = xproto::CreateWindowAux::new().event_mask(xproto::EventMask::PROPERTY_CHANGE);
    let cookie = xwm
        .connection
        .create_window(
            0,
            requestor,
            xwm.supporting_wm_check,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            0,
            &attributes,
        )
        .map_err(XwmError::Connection)?;
    std::mem::forget(cookie);
    Ok(())
}

fn selection_atom(xwm: &Xwm, kind: SelectionKind) -> Atom {
    match kind {
        SelectionKind::Clipboard => xwm.atoms.get(XwmAtomName::Clipboard),
        SelectionKind::Primary => u32::from(xproto::AtomEnum::PRIMARY),
    }
}

fn selection_kind_for_atom(xwm: &Xwm, selection: Atom) -> Option<SelectionKind> {
    if selection == xwm.atoms.get(XwmAtomName::Clipboard) {
        Some(SelectionKind::Clipboard)
    } else if selection == u32::from(xproto::AtomEnum::PRIMARY) {
        Some(SelectionKind::Primary)
    } else {
        None
    }
}

fn set_fd_nonblocking(fd: &OwnedFd) -> io::Result<()> {
    // SAFETY: fcntl operates on the live descriptor owned by `fd`.
    let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if flags & libc::O_NONBLOCK == 0 {
        // SAFETY: this preserves every existing file status flag and adds only O_NONBLOCK.
        let result =
            unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

pub(crate) fn owns_window(xwm: &Xwm, window: Window) -> bool {
    xwm.data_bridge
        .selection_payloads
        .is_internal_window(window)
}

pub(crate) fn selection_notify(
    xwm: &mut Xwm,
    event: xproto::SelectionNotifyEvent,
    now_ns: u64,
) -> Result<bool, XwmError> {
    let Some(id) = xwm
        .data_bridge
        .selection_payloads
        .transfers
        .values()
        .find(|transfer| transfer.requestor == event.requestor)
        .map(|transfer| transfer.id)
    else {
        return Ok(owns_window(xwm, event.requestor));
    };
    let Some(transfer) = xwm.data_bridge.selection_payloads.transfers.get(&id) else {
        return Ok(true);
    };
    let valid_selection = selection_kind_for_atom(xwm, event.selection) == Some(transfer.kind);
    let valid_target = event.target == transfer.target;
    let property_none = event.property == u32::from(xproto::AtomEnum::NONE);
    if transfer.phase != TransferPhase::AwaitSelectionNotify
        || !valid_selection
        || !valid_target
        || (!property_none && event.property != transfer.property)
    {
        finish_transfer(xwm, id, SlotSafety::Poisoned);
        return Ok(true);
    }
    if property_none {
        // A terminal SelectionNotify was consumed, so no later payload event
        // from this conversion can legally arrive on the slot.
        finish_transfer(xwm, id, SlotSafety::Clean);
        return Ok(true);
    }
    if let Some(transfer) = xwm.data_bridge.selection_payloads.transfers.get_mut(&id) {
        transfer.phase = TransferPhase::ReadingProperty;
        transfer.read = Some(PropertyRead {
            mode: ReadMode::Initial,
            offset: 0,
            expected_type_format: None,
            previous_bytes_after: None,
            bytes_read: 0,
        });
    }
    xwm.data_bridge
        .selection_payloads
        .rearm_deadline(id, now_ns);
    xwm.data_bridge.selection_payloads.queue_read(id);
    schedule_property_reads(xwm)?;
    Ok(true)
}

pub(crate) fn property_notify(
    xwm: &mut Xwm,
    event: xproto::PropertyNotifyEvent,
    now_ns: u64,
) -> Result<bool, XwmError> {
    if !owns_window(xwm, event.window) {
        return Ok(false);
    }
    let data_property = xwm.atoms.get(XwmAtomName::SelectionData);
    if event.atom != data_property || event.state != xproto::Property::NEW_VALUE {
        return Ok(true);
    }
    let slot = xwm
        .data_bridge
        .selection_payloads
        .slots
        .iter()
        .flatten()
        .find(|slot| slot.window == event.window);
    let Some(id) = slot.and_then(|slot| slot.active) else {
        return Ok(true);
    };
    let Some(transfer) = xwm.data_bridge.selection_payloads.transfers.get_mut(&id) else {
        return Ok(true);
    };
    if transfer.phase == TransferPhase::AwaitSelectionNotify {
        // The owner sets the initial direct property or INCR marker before it
        // sends SelectionNotify. SelectionNotify remains the start authority.
        return Ok(true);
    }
    if transfer.phase != TransferPhase::WaitingForIncrNewValue {
        finish_transfer(xwm, id, SlotSafety::Poisoned);
        return Ok(true);
    }
    transfer.phase = TransferPhase::ReadingProperty;
    transfer.read = Some(PropertyRead {
        mode: ReadMode::IncrChunk,
        offset: 0,
        expected_type_format: None,
        previous_bytes_after: None,
        bytes_read: 0,
    });
    xwm.data_bridge
        .selection_payloads
        .rearm_deadline(id, now_ns);
    xwm.data_bridge.selection_payloads.queue_read(id);
    schedule_property_reads(xwm)?;
    Ok(true)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SelectionPayloadReplyDrain {
    pub(crate) processed: usize,
    pub(crate) budget_exhausted: bool,
    pub(crate) quiescent: bool,
}

pub(crate) fn poll_replies(
    xwm: &mut Xwm,
    budget: usize,
    now_ns: u64,
) -> Result<SelectionPayloadReplyDrain, XwmError> {
    if budget == 0 {
        return Ok(SelectionPayloadReplyDrain {
            processed: 0,
            budget_exhausted: false,
            quiescent: false,
        });
    }
    let sequences = xwm
        .data_bridge
        .selection_payloads
        .pending
        .keys()
        .copied()
        .take(budget)
        .collect::<Vec<_>>();
    let budget_exhausted = sequences.len() < xwm.data_bridge.selection_payloads.pending.len();
    let mut processed = 0;
    for sequence in sequences {
        let Some(pending) = xwm.data_bridge.selection_payloads.pending.get(&sequence) else {
            continue;
        };
        let pending = *pending;
        let cookie =
            Cookie::<X11Connection, xproto::GetPropertyReply>::new(&xwm.connection, sequence);
        let reply = match cookie.reply_unchecked() {
            Ok(reply) => reply,
            Err(x11rb::errors::ConnectionError::IoError(error))
                if error.kind() == io::ErrorKind::WouldBlock =>
            {
                continue;
            }
            Err(error) => return Err(XwmError::Connection(error)),
        };
        let Some(tracked) = xwm
            .data_bridge
            .selection_payloads
            .take_pending_reply(sequence)
        else {
            continue;
        };
        processed += 1;
        let current_generation = xwm.data_bridge.selection_payloads.active_generation;
        if tracked.transfer_id != pending.transfer_id
            || tracked.generation != pending.generation
            || current_generation != Some(pending.generation)
            || !xwm
                .data_bridge
                .selection_payloads
                .transfers
                .contains_key(&pending.transfer_id)
        {
            continue;
        }
        if let Some(reply) = reply {
            consume_property_reply(xwm, pending.transfer_id, reply, now_ns)?;
        } else {
            finish_transfer(xwm, pending.transfer_id, SlotSafety::Poisoned);
        }
    }
    schedule_property_reads(xwm)?;
    let manager = &xwm.data_bridge.selection_payloads;
    Ok(SelectionPayloadReplyDrain {
        processed,
        budget_exhausted,
        quiescent: !budget_exhausted
            && manager.pending.is_empty()
            && manager.ready_to_read.is_empty(),
    })
}

fn consume_property_reply(
    xwm: &mut Xwm,
    id: SelectionPayloadTransferId,
    reply: xproto::GetPropertyReply,
    now_ns: u64,
) -> Result<(), XwmError> {
    let incr_atom = xwm.atoms.get(XwmAtomName::Incr);
    let (mode, previous_bytes_after) = {
        let Some(transfer) = xwm.data_bridge.selection_payloads.transfers.get(&id) else {
            return Ok(());
        };
        let Some(read) = transfer.read.as_ref() else {
            finish_transfer(xwm, id, SlotSafety::Poisoned);
            return Ok(());
        };
        (read.mode, read.previous_bytes_after)
    };

    if reply.value.len() > MAX_SELECTION_PAYLOAD_CHUNK_BYTES
        || !matches!(reply.format, 8 | 16 | 32)
        || reply.type_ == u32::from(xproto::AtomEnum::NONE)
    {
        finish_transfer(xwm, id, SlotSafety::Poisoned);
        return Ok(());
    }

    if mode == ReadMode::Initial && reply.type_ == incr_atom {
        if reply.format != 32 || reply.value.len() != 4 || reply.bytes_after != 0 {
            finish_transfer(xwm, id, SlotSafety::Poisoned);
            return Ok(());
        }
        // The CARDINAL is advisory only. Its bytes are validated and dropped;
        // no allocation depends on the announced lower bound.
        delete_payload_property(xwm, id)?;
        if let Some(transfer) = xwm.data_bridge.selection_payloads.transfers.get_mut(&id) {
            transfer.phase = TransferPhase::WaitingForIncrNewValue;
            transfer.read = None;
        }
        xwm.data_bridge
            .selection_payloads
            .rearm_deadline(id, now_ns);
        xwm.connection.flush().map_err(XwmError::Connection)?;
        return Ok(());
    }

    let type_format = (reply.type_, reply.format);
    if mode == ReadMode::Initial && reply.type_ == incr_atom {
        finish_transfer(xwm, id, SlotSafety::Poisoned);
        return Ok(());
    }
    if let Some(previous) = previous_bytes_after {
        let consumed = u32::try_from(reply.value.len()).unwrap_or(u32::MAX);
        if previous.checked_sub(consumed) != Some(reply.bytes_after) {
            finish_transfer(xwm, id, SlotSafety::Poisoned);
            return Ok(());
        }
    }
    if reply.bytes_after > 0 && (reply.value.is_empty() || !reply.value.len().is_multiple_of(4)) {
        finish_transfer(xwm, id, SlotSafety::Poisoned);
        return Ok(());
    }

    {
        let Some(transfer) = xwm.data_bridge.selection_payloads.transfers.get_mut(&id) else {
            return Ok(());
        };
        let Some(read) = transfer.read.as_mut() else {
            finish_transfer(xwm, id, SlotSafety::Poisoned);
            return Ok(());
        };
        if read.mode == ReadMode::Initial {
            read.mode = ReadMode::Direct;
            read.expected_type_format = Some(type_format);
        }
        if read.mode == ReadMode::Direct || read.mode == ReadMode::IncrChunk {
            if let Some(expected) = read.expected_type_format
                && expected != type_format
            {
                finish_transfer(xwm, id, SlotSafety::Poisoned);
                return Ok(());
            }
            read.expected_type_format = Some(type_format);
        }
        read.previous_bytes_after = Some(reply.bytes_after);
        read.bytes_read = read.bytes_read.saturating_add(reply.value.len() as u64);
        if reply.bytes_after > 0 {
            let units = u32::try_from(reply.value.len().div_ceil(4)).unwrap_or(u32::MAX);
            let Some(next_offset) = read.offset.checked_add(units) else {
                finish_transfer(xwm, id, SlotSafety::Poisoned);
                return Ok(());
            };
            read.offset = next_offset;
        }
        if !transfer.drain_only && !reply.value.is_empty() {
            debug_assert!(transfer.buffer.is_empty());
            debug_assert!(reply.value.len() <= MAX_SELECTION_PAYLOAD_CHUNK_BYTES);
            transfer.buffer = reply.value;
            transfer.written = 0;
        }
    }
    xwm.data_bridge
        .selection_payloads
        .rearm_deadline(id, now_ns);
    if !transfer_has_buffer(xwm, id) {
        resume_after_buffer(xwm, id, now_ns)?;
    } else {
        write_buffer_to_sink(xwm, id, now_ns)?;
    }
    Ok(())
}

fn transfer_has_buffer(xwm: &Xwm, id: SelectionPayloadTransferId) -> bool {
    xwm.data_bridge
        .selection_payloads
        .transfers
        .get(&id)
        .is_some_and(|transfer| !transfer.buffer.is_empty())
}

fn write_buffer_to_sink(
    xwm: &mut Xwm,
    id: SelectionPayloadTransferId,
    now_ns: u64,
) -> Result<(), XwmError> {
    let mut calls = 0;
    let mut eintr_retries = 0;
    loop {
        let (fd, start, end) = {
            let Some(transfer) = xwm.data_bridge.selection_payloads.transfers.get(&id) else {
                return Ok(());
            };
            let Some(sink) = transfer.sink.as_ref() else {
                return resume_after_buffer(xwm, id, now_ns);
            };
            (sink.as_raw_fd(), transfer.written, transfer.buffer.len())
        };
        if start >= end {
            if let Some(transfer) = xwm.data_bridge.selection_payloads.transfers.get_mut(&id) {
                transfer.buffer.clear();
                transfer.written = 0;
                transfer.sink_writable_interest = false;
            }
            return resume_after_buffer(xwm, id, now_ns);
        }
        if calls >= MAX_WRITE_CALLS_PER_DISPATCH {
            if let Some(transfer) = xwm.data_bridge.selection_payloads.transfers.get_mut(&id) {
                transfer.sink_writable_interest = true;
            }
            return Ok(());
        }
        let result = {
            let transfer = xwm
                .data_bridge
                .selection_payloads
                .transfers
                .get(&id)
                .expect("transfer was checked above");
            let bytes = &transfer.buffer[start..end];
            // SAFETY: the payload manager retains ownership of `fd` and the
            // buffer slice remains borrowed until this nonblocking write returns.
            unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) }
        };
        calls += 1;
        if result > 0 {
            let written = result as usize;
            if let Some(transfer) = xwm.data_bridge.selection_payloads.transfers.get_mut(&id) {
                transfer.written = transfer.written.saturating_add(written);
                transfer.sink_writable_interest = false;
            }
            xwm.data_bridge
                .selection_payloads
                .rearm_deadline(id, now_ns);
            continue;
        }
        if result == 0 {
            enter_drain_only(xwm, id, now_ns)?;
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted
            && eintr_retries < MAX_EINTR_RETRIES_PER_DISPATCH
        {
            eintr_retries += 1;
            continue;
        }
        if error.kind() == io::ErrorKind::WouldBlock || error.kind() == io::ErrorKind::Interrupted {
            if let Some(transfer) = xwm.data_bridge.selection_payloads.transfers.get_mut(&id) {
                transfer.sink_writable_interest = true;
            }
            return Ok(());
        }
        enter_drain_only(xwm, id, now_ns)?;
        return Ok(());
    }
}

fn enter_drain_only(
    xwm: &mut Xwm,
    id: SelectionPayloadTransferId,
    now_ns: u64,
) -> Result<(), XwmError> {
    if let Some(transfer) = xwm.data_bridge.selection_payloads.transfers.get_mut(&id) {
        transfer.sink.take();
        transfer.drain_only = true;
        transfer.sink_writable_interest = false;
        transfer.reactor_token = None;
        transfer.buffer.clear();
        transfer.written = 0;
    }
    resume_after_buffer(xwm, id, now_ns)
}

fn resume_after_buffer(
    xwm: &mut Xwm,
    id: SelectionPayloadTransferId,
    now_ns: u64,
) -> Result<(), XwmError> {
    let (mode, bytes_after, bytes_read) = {
        let Some(transfer) = xwm.data_bridge.selection_payloads.transfers.get(&id) else {
            return Ok(());
        };
        if !transfer.buffer.is_empty() {
            return Ok(());
        }
        let Some(read) = transfer.read.as_ref() else {
            return Ok(());
        };
        (
            read.mode,
            read.previous_bytes_after.unwrap_or(0),
            read.bytes_read,
        )
    };
    if bytes_after > 0 {
        xwm.data_bridge.selection_payloads.queue_read(id);
        return schedule_property_reads(xwm).map(|_| ());
    }
    match mode {
        ReadMode::Initial => {
            finish_transfer(xwm, id, SlotSafety::Poisoned);
        }
        ReadMode::Direct => {
            xwm.data_bridge
                .selection_payloads
                .rearm_deadline(id, now_ns);
            delete_payload_property(xwm, id)?;
            finish_transfer(xwm, id, SlotSafety::Clean);
            xwm.connection.flush().map_err(XwmError::Connection)?;
        }
        ReadMode::IncrChunk => {
            xwm.data_bridge
                .selection_payloads
                .rearm_deadline(id, now_ns);
            delete_payload_property(xwm, id)?;
            let terminal = bytes_read == 0;
            if terminal {
                finish_transfer(xwm, id, SlotSafety::Clean);
            } else if let Some(transfer) = xwm.data_bridge.selection_payloads.transfers.get_mut(&id)
            {
                transfer.phase = TransferPhase::WaitingForIncrNewValue;
                transfer.read = None;
            }
            xwm.connection.flush().map_err(XwmError::Connection)?;
        }
    }
    Ok(())
}

fn delete_payload_property(xwm: &mut Xwm, id: SelectionPayloadTransferId) -> Result<(), XwmError> {
    let Some(transfer) = xwm.data_bridge.selection_payloads.transfers.get(&id) else {
        return Ok(());
    };
    let cookie = xwm
        .connection
        .delete_property(transfer.requestor, transfer.property)
        .map_err(XwmError::Connection)?;
    let sequence = cookie.sequence_number();
    std::mem::forget(cookie);
    if let Some(transfer) = xwm.data_bridge.selection_payloads.transfers.get_mut(&id) {
        transfer.last_request_sequence = sequence;
    }
    Ok(())
}

fn schedule_property_reads(xwm: &mut Xwm) -> Result<(), XwmError> {
    let mut issued = 0;
    while xwm.data_bridge.selection_payloads.pending.len() < MAX_PENDING_SELECTION_PAYLOAD_REPLIES {
        let Some((id, requestor, property, offset)) =
            xwm.data_bridge.selection_payloads.next_ready_read()
        else {
            break;
        };
        let sequence = match issue_property_read(xwm, requestor, property, offset) {
            Ok(sequence) => sequence,
            Err(error) => {
                finish_transfer(xwm, id, SlotSafety::Poisoned);
                return Err(XwmError::Connection(error));
            }
        };
        if let Some(transfer) = xwm.data_bridge.selection_payloads.transfers.get_mut(&id) {
            transfer.last_request_sequence = sequence;
        }
        let generation = xwm
            .data_bridge
            .selection_payloads
            .transfers
            .get(&id)
            .map(|transfer| transfer.generation);
        if let Some(generation) = generation {
            xwm.data_bridge
                .selection_payloads
                .note_pending_reply(id, generation, sequence);
            issued += 1;
        }
    }
    if issued > 0 {
        xwm.connection.flush().map_err(XwmError::Connection)?;
    }
    debug_assert!(
        xwm.data_bridge.selection_payloads.pending_reply_count()
            <= MAX_PENDING_SELECTION_PAYLOAD_REPLIES
    );
    Ok(())
}

fn issue_property_read(
    xwm: &Xwm,
    requestor: Window,
    property: Atom,
    offset: u32,
) -> Result<SequenceNumber, x11rb::errors::ConnectionError> {
    let cookie = xwm.connection.get_property(
        false,
        requestor,
        property,
        xproto::AtomEnum::ANY,
        offset,
        SELECTION_PAYLOAD_CHUNK_UNITS,
    )?;
    let sequence = cookie.sequence_number();
    std::mem::forget(cookie);
    Ok(sequence)
}

fn finish_transfer(xwm: &mut Xwm, id: SelectionPayloadTransferId, safety: SlotSafety) {
    let Some(transfer) = xwm.data_bridge.selection_payloads.transfers.remove(&id) else {
        return;
    };
    if let Some(sequence) = transfer.pending_reply {
        xwm.data_bridge.selection_payloads.pending.remove(&sequence);
        xwm.connection.discard_reply(
            sequence,
            RequestKind::HasResponse,
            DiscardMode::DiscardReply,
        );
    }
    xwm.data_bridge.selection_payloads.ready_set.remove(&id);
    xwm.data_bridge
        .selection_payloads
        .update_slot_after_transfer(&transfer, safety);
    xwm.data_bridge.selection_payloads.debug_assert_invariants();
}
