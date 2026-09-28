//! Bounded, XDND-owned Wayland source payload transfers.
//!
//! This deliberately has its own IDs and lifecycle. Clipboard/PRIMARY proxy
//! ownership and request state never enters this module.

use std::{
    collections::{HashMap, VecDeque},
    io,
    num::NonZeroU64,
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
};

use x11rb::{
    connection::{Connection, RequestConnection},
    protocol::xproto::{self, ConnectionExt as XprotoConnectionExt, PropMode},
    wrapper::ConnectionExt as XprotoWrapperExt,
};

use super::{Xwm, XwmError, atoms::XwmAtomName};
use crate::xwayland::{
    XwaylandDndSourceDataRequest, XwaylandDndSourceProxyId, XwaylandDndSourceTransferId,
};

pub(crate) const MAX_ACTIVE_DND_TRANSFERS: usize = 64;
pub(crate) const MAX_DND_CHUNK_BYTES: usize = 64 * 1024;
pub(crate) const DND_TRANSFER_IDLE_TIMEOUT_NS: u64 = 30_000_000_000;
const MAX_PENDING_DND_REQUESTS: usize = 64;
const MAX_EINTR_RETRIES_PER_DISPATCH: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DndTransferPhase {
    AwaitSourceAcceptance,
    ReadingDirect,
    WaitingForInitialDelete,
    WaitingForChunkDelete,
    WaitingForSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConversionNotification {
    Single {
        requestor: u32,
        time: u32,
        target_atom: u32,
        property: u32,
    },
    Multiple {
        group_id: u64,
        pair_index: usize,
    },
}

pub(crate) struct DndTransferRequest {
    pub source: XwaylandDndSourceProxyId,
    pub target: crate::xwayland::X11WindowHandle,
    pub requestor: u32,
    pub property: u32,
    pub mime_type: String,
    pub property_type: u32,
    pub notification: ConversionNotification,
}

#[derive(Debug)]
pub(crate) struct DndOutgoingTransfer {
    pub id: XwaylandDndSourceTransferId,
    pub generation: crate::xwayland::XwaylandGeneration,
    pub source: XwaylandDndSourceProxyId,
    pub target: crate::xwayland::X11WindowHandle,
    pub requestor: u32,
    pub property: u32,
    pub property_type: u32,
    pub read: OwnedFd,
    pub registered_fd: Option<RawFd>,
    pub reactor_token: Option<u64>,
    pub requestor_monitor_retained: bool,
    pub phase: DndTransferPhase,
    pub buffer: VecDeque<u8>,
    pub eof: bool,
    pub deadline_ns: u64,
    pub notification: ConversionNotification,
    pub notify_ready: bool,
    pub max_chunk_bytes: usize,
}

#[derive(Debug)]
struct MultipleGroup {
    requestor: u32,
    request_time: u32,
    target: u32,
    property: u32,
    pairs: Vec<(u32, u32)>,
    pending_conversions: usize,
    setup_complete: bool,
    deadline_ns: u64,
}

#[derive(Debug, Default)]
pub(crate) struct DndOutgoingManager {
    generation: Option<crate::xwayland::XwaylandGeneration>,
    transfers: HashMap<XwaylandDndSourceTransferId, DndOutgoingTransfer>,
    requests: VecDeque<XwaylandDndSourceDataRequest>,
    multiple_groups: HashMap<u64, MultipleGroup>,
    next_transfer_serial: u64,
    next_multiple_id: u64,
}

impl DndOutgoingManager {
    pub(crate) fn initialize_generation(
        &mut self,
        generation: crate::xwayland::XwaylandGeneration,
    ) {
        if self.generation != Some(generation) {
            self.transfers.clear();
            self.requests.clear();
            self.multiple_groups.clear();
            self.generation = Some(generation);
        }
    }

    pub(crate) fn clear_generation(&mut self, generation: crate::xwayland::XwaylandGeneration) {
        if self.generation == Some(generation) {
            self.generation = None;
            self.transfers.clear();
            self.requests.clear();
            self.multiple_groups.clear();
        }
    }

    pub(crate) fn next_deadline_ns(&self) -> Option<u64> {
        self.transfers
            .values()
            .map(|transfer| transfer.deadline_ns)
            .chain(self.multiple_groups.values().map(|group| group.deadline_ns))
            .min()
    }

    pub(crate) fn source_interests(&self) -> Vec<(XwaylandDndSourceTransferId, RawFd)> {
        let mut interests = self
            .transfers
            .values()
            .filter_map(|transfer| {
                matches!(
                    transfer.phase,
                    DndTransferPhase::ReadingDirect | DndTransferPhase::WaitingForSource
                )
                .then_some((transfer.id, transfer.read.as_raw_fd()))
            })
            .collect::<Vec<_>>();
        interests.sort_by_key(|(id, _)| id.serial);
        interests
    }

    pub(crate) fn bind_reactor_token(
        &mut self,
        id: XwaylandDndSourceTransferId,
        fd: RawFd,
        token: Option<u64>,
    ) {
        if let Some(transfer) = self.transfers.get_mut(&id) {
            transfer.reactor_token = token;
            transfer.registered_fd = token.map(|_| fd);
        }
    }

    pub(crate) fn transfer_matches_reactor(
        &self,
        id: XwaylandDndSourceTransferId,
        generation: crate::xwayland::XwaylandGeneration,
        token: u64,
    ) -> bool {
        self.transfers.get(&id).is_some_and(|transfer| {
            transfer.generation == generation
                && transfer.reactor_token == Some(token)
                && transfer.registered_fd == Some(transfer.read.as_raw_fd())
                && self.generation == Some(generation)
        })
    }

    pub(crate) fn take_requests(&mut self) -> Vec<XwaylandDndSourceDataRequest> {
        self.requests.drain(..).collect()
    }
}

pub(crate) fn create_multiple_group(
    xwm: &mut Xwm,
    requestor: u32,
    request_time: u32,
    target: u32,
    property: u32,
    pairs: Vec<(u32, u32)>,
    now_ns: u64,
) -> Option<u64> {
    if pairs.len() > 64 || property == x11rb::NONE {
        return None;
    }
    let manager = &mut xwm.data_bridge.dnd_outgoing;
    if manager.multiple_groups.len() >= MAX_PENDING_DND_REQUESTS {
        return None;
    }
    manager.next_multiple_id = manager.next_multiple_id.checked_add(1)?;
    let id = manager.next_multiple_id;
    manager.multiple_groups.insert(
        id,
        MultipleGroup {
            requestor,
            request_time,
            target,
            property,
            pairs,
            pending_conversions: 0,
            setup_complete: false,
            deadline_ns: now_ns.saturating_add(DND_TRANSFER_IDLE_TIMEOUT_NS),
        },
    );
    Some(id)
}

pub(crate) fn add_multiple_conversion(
    xwm: &mut Xwm,
    group_id: u64,
    pair_index: usize,
) -> Option<ConversionNotification> {
    let group = xwm
        .data_bridge
        .dnd_outgoing
        .multiple_groups
        .get_mut(&group_id)?;
    if pair_index >= group.pairs.len() {
        return None;
    }
    group.pending_conversions = group.pending_conversions.saturating_add(1);
    Some(ConversionNotification::Multiple {
        group_id,
        pair_index,
    })
}

pub(crate) fn set_multiple_pair_result(
    xwm: &mut Xwm,
    group_id: u64,
    pair_index: usize,
    success: bool,
    now_ns: u64,
) -> Result<(), XwmError> {
    if let Some(group) = xwm
        .data_bridge
        .dnd_outgoing
        .multiple_groups
        .get_mut(&group_id)
        && let Some(pair) = group.pairs.get_mut(pair_index)
    {
        if !success {
            pair.1 = x11rb::NONE;
        }
        group.pending_conversions = group.pending_conversions.saturating_sub(1);
        group.deadline_ns = now_ns.saturating_add(DND_TRANSFER_IDLE_TIMEOUT_NS);
    }
    maybe_finish_multiple_group(xwm, group_id)
}

pub(crate) fn finish_multiple_setup(xwm: &mut Xwm, group_id: u64) -> Result<(), XwmError> {
    if let Some(group) = xwm
        .data_bridge
        .dnd_outgoing
        .multiple_groups
        .get_mut(&group_id)
    {
        group.setup_complete = true;
    }
    maybe_finish_multiple_group(xwm, group_id)
}

fn maybe_finish_multiple_group(xwm: &mut Xwm, group_id: u64) -> Result<(), XwmError> {
    let Some(group) = xwm.data_bridge.dnd_outgoing.multiple_groups.get(&group_id) else {
        return Ok(());
    };
    if !group.setup_complete || group.pending_conversions != 0 {
        return Ok(());
    }
    let group = xwm
        .data_bridge
        .dnd_outgoing
        .multiple_groups
        .remove(&group_id)
        .expect("finished MULTIPLE group exists");
    let mut values = Vec::with_capacity(group.pairs.len() * 2);
    for (target, property) in group.pairs {
        values.extend([target, property]);
    }
    let cookie = xwm.connection.change_property32(
        PropMode::REPLACE,
        group.requestor,
        group.property,
        xwm.atoms.get(XwmAtomName::AtomPair),
        &values,
    );
    match cookie {
        Ok(cookie) => std::mem::forget(cookie),
        Err(_) => {
            send_selection_notify(
                xwm,
                group.requestor,
                group.request_time,
                xwm.atoms.get(XwmAtomName::XdndSelection),
                x11rb::NONE,
                group.target,
            )?;
            return Ok(());
        }
    }
    send_selection_notify(
        xwm,
        group.requestor,
        group.request_time,
        xwm.atoms.get(XwmAtomName::XdndSelection),
        group.property,
        group.target,
    )
}

pub(crate) fn start_transfer(
    xwm: &mut Xwm,
    request: DndTransferRequest,
    now_ns: u64,
) -> Result<Option<XwaylandDndSourceTransferId>, XwmError> {
    let DndTransferRequest {
        source,
        target,
        requestor,
        property,
        mime_type,
        property_type,
        notification,
    } = request;
    let generation = xwm.generation;
    let manager = &xwm.data_bridge.dnd_outgoing;
    if manager.generation != Some(generation)
        || manager.transfers.len() >= MAX_ACTIVE_DND_TRANSFERS
        || manager.requests.len() >= MAX_PENDING_DND_REQUESTS
        || manager
            .transfers
            .values()
            .any(|transfer| transfer.requestor == requestor && transfer.property == property)
    {
        return Ok(None);
    }
    let max_chunk_bytes = xwm
        .connection
        .maximum_request_bytes()
        .checked_sub(64)
        .map(|bytes| bytes.min(MAX_DND_CHUNK_BYTES))
        .filter(|bytes| *bytes > 0);
    let Some(max_chunk_bytes) = max_chunk_bytes else {
        return Ok(None);
    };
    let serial = manager.next_transfer_serial.checked_add(1);
    let Some(serial) = serial.and_then(NonZeroU64::new) else {
        return Ok(None);
    };
    // Share only the XWM's per-window event-mask refcount so both bridges can
    // observe PropertyNotify/DestroyNotify without clearing each other's mask.
    // XDND transfer ownership and IDs remain wholly local to this manager.
    if !super::selection_proxy::retain_requestor(xwm, requestor)? {
        return Ok(None);
    }
    let id = XwaylandDndSourceTransferId { source, serial };
    let (read, write) = match create_pipe() {
        Ok(pipe) => pipe,
        Err(_) => {
            super::selection_proxy::release_requestor(xwm, requestor, false)?;
            return Ok(None);
        }
    };
    let manager = &mut xwm.data_bridge.dnd_outgoing;
    manager.next_transfer_serial = serial.get();
    manager.requests.push_back(XwaylandDndSourceDataRequest {
        transfer_id: id,
        target,
        requestor,
        mime_type: mime_type.clone(),
        sink: write,
    });
    manager.transfers.insert(
        id,
        DndOutgoingTransfer {
            id,
            generation,
            source,
            target,
            requestor,
            property,
            property_type,
            read,
            registered_fd: None,
            reactor_token: None,
            requestor_monitor_retained: true,
            phase: DndTransferPhase::AwaitSourceAcceptance,
            buffer: VecDeque::with_capacity(max_chunk_bytes),
            eof: false,
            deadline_ns: now_ns.saturating_add(DND_TRANSFER_IDLE_TIMEOUT_NS),
            notification,
            notify_ready: false,
            max_chunk_bytes,
        },
    );
    Ok(Some(id))
}

pub(crate) fn take_requests(xwm: &mut Xwm) -> Vec<XwaylandDndSourceDataRequest> {
    xwm.data_bridge.dnd_outgoing.take_requests()
}

pub(crate) fn resolve_requests(
    xwm: &mut Xwm,
    results: impl IntoIterator<Item = (XwaylandDndSourceTransferId, bool)>,
    now_ns: u64,
) -> Result<bool, XwmError> {
    let mut changed = false;
    for (id, accepted) in results {
        changed |= resolve_acceptance(xwm, id, accepted, now_ns)?;
    }
    Ok(changed)
}

fn release_transfer_requestor(
    xwm: &mut Xwm,
    transfer: &DndOutgoingTransfer,
    destroyed: bool,
) -> Result<(), XwmError> {
    if transfer.requestor_monitor_retained {
        super::selection_proxy::release_requestor(xwm, transfer.requestor, destroyed)?;
    }
    Ok(())
}

fn touch_multiple_group(xwm: &mut Xwm, notification: ConversionNotification, now_ns: u64) {
    if let ConversionNotification::Multiple { group_id, .. } = notification
        && let Some(group) = xwm
            .data_bridge
            .dnd_outgoing
            .multiple_groups
            .get_mut(&group_id)
    {
        group.deadline_ns = now_ns.saturating_add(DND_TRANSFER_IDLE_TIMEOUT_NS);
    }
}

fn resolve_acceptance(
    xwm: &mut Xwm,
    id: XwaylandDndSourceTransferId,
    accepted: bool,
    now_ns: u64,
) -> Result<bool, XwmError> {
    let Some(mut transfer) = xwm.data_bridge.dnd_outgoing.transfers.remove(&id) else {
        return Ok(false);
    };
    if transfer.phase != DndTransferPhase::AwaitSourceAcceptance
        || transfer.generation != xwm.generation
        || !current_source(xwm, transfer.source, transfer.target, transfer.requestor)
    {
        complete_conversion(xwm, transfer.notification, false, now_ns)?;
        release_transfer_requestor(xwm, &transfer, false)?;
        return Ok(false);
    }
    if !accepted {
        complete_conversion(xwm, transfer.notification, false, now_ns)?;
        release_transfer_requestor(xwm, &transfer, false)?;
        return Ok(true);
    }
    touch_multiple_group(xwm, transfer.notification, now_ns);
    transfer.deadline_ns = now_ns.saturating_add(DND_TRANSFER_IDLE_TIMEOUT_NS);
    transfer.phase = DndTransferPhase::ReadingDirect;
    let keep = pump_direct(xwm, &mut transfer, now_ns)?;
    if keep {
        xwm.data_bridge.dnd_outgoing.transfers.insert(id, transfer);
    } else {
        release_transfer_requestor(xwm, &transfer, false)?;
    }
    Ok(true)
}

pub(crate) fn source_interests(xwm: &Xwm) -> Vec<(XwaylandDndSourceTransferId, RawFd)> {
    xwm.data_bridge.dnd_outgoing.source_interests()
}

pub(crate) fn bind_reactor_token(
    xwm: &mut Xwm,
    id: XwaylandDndSourceTransferId,
    fd: RawFd,
    token: Option<u64>,
) {
    xwm.data_bridge
        .dnd_outgoing
        .bind_reactor_token(id, fd, token);
}

pub(crate) fn handle_source_ready(
    xwm: &mut Xwm,
    id: XwaylandDndSourceTransferId,
    generation: crate::xwayland::XwaylandGeneration,
    token: u64,
    now_ns: u64,
) -> Result<bool, XwmError> {
    if !xwm
        .data_bridge
        .dnd_outgoing
        .transfer_matches_reactor(id, generation, token)
    {
        return Ok(false);
    }
    let Some(mut transfer) = xwm.data_bridge.dnd_outgoing.transfers.remove(&id) else {
        return Ok(false);
    };
    if !current_source(xwm, transfer.source, transfer.target, transfer.requestor) {
        complete_conversion(xwm, transfer.notification, false, now_ns)?;
        release_transfer_requestor(xwm, &transfer, false)?;
        return Ok(true);
    }
    touch_multiple_group(xwm, transfer.notification, now_ns);
    transfer.deadline_ns = now_ns.saturating_add(DND_TRANSFER_IDLE_TIMEOUT_NS);
    let before = transfer.phase;
    let keep = match transfer.phase {
        DndTransferPhase::ReadingDirect => pump_direct(xwm, &mut transfer, now_ns)?,
        DndTransferPhase::WaitingForSource => pump_incremental_source(xwm, &mut transfer, now_ns)?,
        _ => true,
    };
    // Removing a completed or failed transfer also changes the reactor's
    // source set even when its phase did not advance (for example a short
    // direct payload that reaches EOF in one readiness dispatch).
    let changed = !keep || before != transfer.phase;
    if keep {
        xwm.data_bridge.dnd_outgoing.transfers.insert(id, transfer);
    } else {
        release_transfer_requestor(xwm, &transfer, false)?;
    }
    Ok(changed)
}

fn current_source(
    xwm: &Xwm,
    source: XwaylandDndSourceProxyId,
    target: crate::xwayland::X11WindowHandle,
    requestor: u32,
) -> bool {
    let _ = (target, requestor);
    xwm.generation == source.adapter_id.generation()
        && xwm.data_bridge.dnd.active_session().is_some_and(|session| {
            session.id == source.adapter_id
                && session.source_proxy == Some(source.xid)
                && session.ownership_confirmed
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadProgress {
    WouldBlock,
    Eof,
    LimitReached,
}

fn read_until_limit(transfer: &mut DndOutgoingTransfer) -> Result<ReadProgress, XwmError> {
    let mut retries = 0;
    let mut scratch = [0u8; 8192];
    while transfer.buffer.len() < transfer.max_chunk_bytes && !transfer.eof {
        let len = (transfer.max_chunk_bytes - transfer.buffer.len()).min(scratch.len());
        let result =
            unsafe { libc::read(transfer.read.as_raw_fd(), scratch.as_mut_ptr().cast(), len) };
        if result > 0 {
            transfer.buffer.extend(&scratch[..result as usize]);
            retries = 0;
            continue;
        }
        if result == 0 {
            transfer.eof = true;
            return Ok(ReadProgress::Eof);
        }
        let error = io::Error::last_os_error();
        match error.kind() {
            io::ErrorKind::WouldBlock => return Ok(ReadProgress::WouldBlock),
            io::ErrorKind::Interrupted if retries < MAX_EINTR_RETRIES_PER_DISPATCH => retries += 1,
            _ => {
                return Err(XwmError::Connection(
                    x11rb::errors::ConnectionError::IoError(error),
                ));
            }
        }
    }
    if transfer.buffer.len() == transfer.max_chunk_bytes {
        Ok(ReadProgress::LimitReached)
    } else {
        Ok(ReadProgress::Eof)
    }
}

fn pump_direct(
    xwm: &mut Xwm,
    transfer: &mut DndOutgoingTransfer,
    now_ns: u64,
) -> Result<bool, XwmError> {
    let progress = match read_until_limit(transfer) {
        Ok(progress) => progress,
        Err(_) => {
            complete_conversion(xwm, transfer.notification, false, now_ns)?;
            return Ok(false);
        }
    };
    match progress {
        ReadProgress::WouldBlock if transfer.buffer.len() < transfer.max_chunk_bytes => {
            transfer.phase = DndTransferPhase::ReadingDirect;
            Ok(true)
        }
        ReadProgress::Eof if transfer.buffer.len() < transfer.max_chunk_bytes => {
            publish_bytes(
                xwm,
                transfer.requestor,
                transfer.property,
                transfer.property_type,
                transfer.buffer.make_contiguous(),
            )?;
            complete_conversion(xwm, transfer.notification, true, now_ns)?;
            Ok(false)
        }
        ReadProgress::LimitReached | ReadProgress::WouldBlock | ReadProgress::Eof => {
            let lower_bound = transfer.buffer.len().min(u32::MAX as usize) as u32;
            let cookie = xwm
                .connection
                .change_property32(
                    PropMode::REPLACE,
                    transfer.requestor,
                    transfer.property,
                    xwm.atoms.get(XwmAtomName::Incr),
                    &[lower_bound],
                )
                .map_err(XwmError::Connection)?;
            std::mem::forget(cookie);
            transfer.phase = DndTransferPhase::WaitingForInitialDelete;
            transfer.notify_ready = true;
            transfer.deadline_ns = now_ns.saturating_add(DND_TRANSFER_IDLE_TIMEOUT_NS);
            complete_conversion(xwm, transfer.notification, true, now_ns)?;
            Ok(true)
        }
    }
}

fn pump_incremental_source(
    xwm: &mut Xwm,
    transfer: &mut DndOutgoingTransfer,
    now_ns: u64,
) -> Result<bool, XwmError> {
    let progress = match read_until_limit(transfer) {
        Ok(progress) => progress,
        Err(_) => {
            publish_bytes(
                xwm,
                transfer.requestor,
                transfer.property,
                transfer.property_type,
                &[],
            )?;
            return Ok(false);
        }
    };
    match progress {
        ReadProgress::WouldBlock if transfer.buffer.is_empty() => {
            transfer.phase = DndTransferPhase::WaitingForSource;
            Ok(true)
        }
        ReadProgress::Eof if transfer.buffer.is_empty() => {
            publish_bytes(
                xwm,
                transfer.requestor,
                transfer.property,
                transfer.property_type,
                &[],
            )?;
            Ok(false)
        }
        ReadProgress::WouldBlock | ReadProgress::LimitReached | ReadProgress::Eof => {
            publish_chunk(xwm, transfer, now_ns)?;
            Ok(true)
        }
    }
}

fn publish_chunk(
    xwm: &mut Xwm,
    transfer: &mut DndOutgoingTransfer,
    now_ns: u64,
) -> Result<(), XwmError> {
    let count = transfer.buffer.len().min(transfer.max_chunk_bytes);
    let bytes = transfer.buffer.make_contiguous();
    publish_bytes(
        xwm,
        transfer.requestor,
        transfer.property,
        transfer.property_type,
        &bytes[..count],
    )?;
    transfer.buffer.drain(..count);
    transfer.phase = DndTransferPhase::WaitingForChunkDelete;
    transfer.deadline_ns = now_ns.saturating_add(DND_TRANSFER_IDLE_TIMEOUT_NS);
    touch_multiple_group(xwm, transfer.notification, now_ns);
    Ok(())
}

fn publish_bytes(
    xwm: &Xwm,
    requestor: u32,
    property: u32,
    property_type: u32,
    bytes: &[u8],
) -> Result<(), XwmError> {
    let cookie = xwm
        .connection
        .change_property8(PropMode::REPLACE, requestor, property, property_type, bytes)
        .map_err(XwmError::Connection)?;
    std::mem::forget(cookie);
    xwm.connection.flush().map_err(XwmError::Connection)
}

fn complete_conversion(
    xwm: &mut Xwm,
    notification: ConversionNotification,
    success: bool,
    now_ns: u64,
) -> Result<(), XwmError> {
    match notification {
        ConversionNotification::Single {
            requestor,
            time,
            target_atom,
            property,
        } => send_selection_notify(
            xwm,
            requestor,
            time,
            xwm.atoms.get(XwmAtomName::XdndSelection),
            target_atom,
            if success { property } else { x11rb::NONE },
        ),
        ConversionNotification::Multiple {
            group_id,
            pair_index,
        } => set_multiple_pair_result(xwm, group_id, pair_index, success, now_ns),
    }
}

pub(crate) fn complete_single_conversion(
    xwm: &mut Xwm,
    requestor: u32,
    time: u32,
    target_atom: u32,
    property: u32,
    success: bool,
) -> Result<(), XwmError> {
    send_selection_notify(
        xwm,
        requestor,
        time,
        xwm.atoms.get(XwmAtomName::XdndSelection),
        target_atom,
        if success { property } else { x11rb::NONE },
    )
}

pub(crate) fn send_selection_notify(
    xwm: &Xwm,
    requestor: u32,
    time: u32,
    selection: u32,
    target: u32,
    property: u32,
) -> Result<(), XwmError> {
    let event = xproto::SelectionNotifyEvent {
        response_type: xproto::SELECTION_NOTIFY_EVENT,
        sequence: 0,
        time,
        requestor,
        selection,
        target,
        property,
    };
    xwm.connection
        .send_event(false, requestor, xproto::EventMask::NO_EVENT, event)
        .map_err(XwmError::Connection)?;
    Ok(())
}

pub(crate) fn owns_property(xwm: &Xwm, requestor: u32, property: u32) -> bool {
    xwm.data_bridge
        .dnd_outgoing
        .transfers
        .values()
        .any(|transfer| {
            transfer.requestor == requestor
                && transfer.property == property
                && matches!(
                    transfer.phase,
                    DndTransferPhase::WaitingForInitialDelete
                        | DndTransferPhase::WaitingForChunkDelete
                )
        })
}

pub(crate) fn property_in_use(xwm: &Xwm, requestor: u32, property: u32) -> bool {
    xwm.data_bridge
        .dnd_outgoing
        .transfers
        .values()
        .any(|transfer| transfer.requestor == requestor && transfer.property == property)
}

pub(crate) fn property_deleted(
    xwm: &mut Xwm,
    requestor: u32,
    property: u32,
    now_ns: u64,
) -> Result<bool, XwmError> {
    let id = xwm
        .data_bridge
        .dnd_outgoing
        .transfers
        .iter()
        .find_map(|(id, transfer)| {
            (transfer.requestor == requestor
                && transfer.property == property
                && matches!(
                    transfer.phase,
                    DndTransferPhase::WaitingForInitialDelete
                        | DndTransferPhase::WaitingForChunkDelete
                ))
            .then_some(*id)
        });
    let Some(id) = id else {
        return Ok(false);
    };
    let Some(mut transfer) = xwm.data_bridge.dnd_outgoing.transfers.remove(&id) else {
        return Ok(false);
    };
    transfer.deadline_ns = now_ns.saturating_add(DND_TRANSFER_IDLE_TIMEOUT_NS);
    if !transfer.buffer.is_empty() {
        publish_chunk(xwm, &mut transfer, now_ns)?;
        xwm.data_bridge.dnd_outgoing.transfers.insert(id, transfer);
    } else if transfer.eof {
        publish_bytes(
            xwm,
            transfer.requestor,
            transfer.property,
            transfer.property_type,
            &[],
        )?;
        release_transfer_requestor(xwm, &transfer, false)?;
    } else {
        transfer.phase = DndTransferPhase::WaitingForSource;
        xwm.data_bridge.dnd_outgoing.transfers.insert(id, transfer);
    }
    Ok(true)
}

pub(crate) fn requestor_destroyed(xwm: &mut Xwm, requestor: u32) -> Result<(), XwmError> {
    let removed = xwm
        .data_bridge
        .dnd_outgoing
        .transfers
        .values()
        .filter(|transfer| transfer.requestor == requestor)
        .map(|transfer| transfer.id)
        .collect::<Vec<_>>();
    for id in removed {
        if let Some(transfer) = xwm.data_bridge.dnd_outgoing.transfers.remove(&id) {
            release_transfer_requestor(xwm, &transfer, true)?;
        }
    }
    xwm.data_bridge
        .dnd_outgoing
        .requests
        .retain(|request| request.requestor != requestor);
    xwm.data_bridge
        .dnd_outgoing
        .multiple_groups
        .retain(|_, group| group.requestor != requestor);
    Ok(())
}

pub(crate) fn cancel_source(
    xwm: &mut Xwm,
    source: XwaylandDndSourceProxyId,
    now_ns: u64,
) -> Result<(), XwmError> {
    let ids = xwm
        .data_bridge
        .dnd_outgoing
        .transfers
        .values()
        .filter(|transfer| transfer.source == source)
        .map(|transfer| transfer.id)
        .collect::<Vec<_>>();
    for id in ids {
        let Some(transfer) = xwm.data_bridge.dnd_outgoing.transfers.remove(&id) else {
            continue;
        };
        xwm.data_bridge
            .dnd_outgoing
            .requests
            .retain(|request| request.transfer_id != id);
        if transfer.notify_ready {
            let _ = publish_bytes(
                xwm,
                transfer.requestor,
                transfer.property,
                transfer.property_type,
                &[],
            );
        } else {
            complete_conversion(xwm, transfer.notification, false, now_ns)?;
        }
        release_transfer_requestor(xwm, &transfer, false)?;
    }
    Ok(())
}

pub(crate) fn cancel_generation(
    xwm: &mut Xwm,
    generation: crate::xwayland::XwaylandGeneration,
    now_ns: u64,
) -> Result<(), XwmError> {
    let sources = xwm
        .data_bridge
        .dnd_outgoing
        .transfers
        .values()
        .filter(|transfer| transfer.generation == generation)
        .map(|transfer| transfer.source)
        .collect::<std::collections::HashSet<_>>();
    for source in sources {
        cancel_source(xwm, source, now_ns)?;
    }
    xwm.data_bridge.dnd_outgoing.requests.clear();
    xwm.data_bridge.dnd_outgoing.multiple_groups.clear();
    Ok(())
}

pub(crate) fn expire_deadlines(xwm: &mut Xwm, now_ns: u64) -> Result<(), XwmError> {
    let expired = xwm
        .data_bridge
        .dnd_outgoing
        .transfers
        .values()
        .filter(|transfer| transfer.deadline_ns <= now_ns)
        .map(|transfer| transfer.id)
        .collect::<Vec<_>>();
    for id in expired {
        let Some(transfer) = xwm.data_bridge.dnd_outgoing.transfers.remove(&id) else {
            continue;
        };
        xwm.data_bridge
            .dnd_outgoing
            .requests
            .retain(|request| request.transfer_id != id);
        if !transfer.notify_ready {
            match transfer.notification {
                ConversionNotification::Single {
                    requestor,
                    time,
                    target_atom,
                    ..
                } => {
                    send_selection_notify(
                        xwm,
                        requestor,
                        time,
                        xwm.atoms.get(XwmAtomName::XdndSelection),
                        target_atom,
                        x11rb::NONE,
                    )?;
                }
                ConversionNotification::Multiple {
                    group_id,
                    pair_index,
                } => {
                    set_multiple_pair_result(xwm, group_id, pair_index, false, now_ns)?;
                }
            }
        }
        release_transfer_requestor(xwm, &transfer, false)?;
    }
    let expired_groups = xwm
        .data_bridge
        .dnd_outgoing
        .multiple_groups
        .iter()
        .filter_map(|(id, group)| (group.deadline_ns <= now_ns).then_some(*id))
        .collect::<Vec<_>>();
    for id in expired_groups {
        if let Some(group) = xwm.data_bridge.dnd_outgoing.multiple_groups.remove(&id) {
            send_selection_notify(
                xwm,
                group.requestor,
                group.request_time,
                xwm.atoms.get(XwmAtomName::XdndSelection),
                group.target,
                x11rb::NONE,
            )?;
        }
    }
    Ok(())
}

pub(crate) fn create_pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [-1; 2];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let read = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    let write = unsafe { OwnedFd::from_raw_fd(fds[1]) };
    let flags = unsafe { libc::fcntl(read.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(read.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok((read, write))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Read,
        num::NonZeroU64,
        os::{fd::AsRawFd, unix::net::UnixStream},
    };

    fn active_fixture() -> (
        Xwm,
        UnixStream,
        XwaylandDndSourceProxyId,
        crate::xwayland::X11WindowHandle,
    ) {
        let generation = crate::xwayland::XwaylandGeneration::new(NonZeroU64::new(91).unwrap());
        let (mut xwm, peer) = super::super::test_fixture_for_tests(generation);
        let adapter_id = crate::xwayland::XwaylandDndAdapterId::new(
            crate::xwayland::CanonicalDndSessionId::Wayland(NonZeroU64::new(9201).unwrap()),
            generation,
        )
        .unwrap();
        let target = crate::xwayland::X11WindowHandle::new(generation, 0x770);
        let source = XwaylandDndSourceProxyId {
            adapter_id,
            xid: 0x880,
        };
        assert!(
            xwm.data_bridge.dnd.install_wayland_session(
                adapter_id,
                crate::xwayland::XwaylandDndMimeCatalog::try_new(vec!["text/plain".to_owned()])
                    .unwrap(),
                vec![crate::xwayland::XwaylandDndAction::Copy],
            )
        );
        assert!(
            xwm.data_bridge
                .dnd
                .bind_source_proxy(adapter_id, source.xid)
        );
        assert!(
            xwm.data_bridge
                .dnd
                .confirm_source_ownership(adapter_id, source.xid, 1234)
        );
        xwm.data_bridge
            .dnd_outgoing
            .initialize_generation(generation);
        (xwm, peer, source, target)
    }

    fn start_payload(
        xwm: &mut Xwm,
        source: XwaylandDndSourceProxyId,
        target: crate::xwayland::X11WindowHandle,
        bytes: &[u8],
    ) -> XwaylandDndSourceTransferId {
        let id = start_transfer(
            xwm,
            DndTransferRequest {
                source,
                target,
                requestor: target.xid(),
                property: 0x900,
                mime_type: "text/plain".to_owned(),
                property_type: 99,
                notification: ConversionNotification::Single {
                    requestor: target.xid(),
                    time: 1235,
                    target_atom: 99,
                    property: 0x900,
                },
            },
            10,
        )
        .unwrap()
        .unwrap();
        let requests = take_requests(xwm);
        assert_eq!(requests.len(), 1);
        let request = requests.into_iter().next().unwrap();
        assert_eq!(
            unsafe { libc::write(request.sink.as_raw_fd(), bytes.as_ptr().cast(), bytes.len(),) },
            bytes.len() as isize
        );
        drop(request);
        id
    }

    fn read_requests(peer: &mut UnixStream) -> Vec<u8> {
        peer.set_nonblocking(true).unwrap();
        let mut requests = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            match peer.read(&mut buffer) {
                Ok(0) => break,
                Ok(bytes_read) => requests.extend_from_slice(&buffer[..bytes_read]),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => panic!("read X11 requests: {error}"),
            }
        }
        requests
    }

    fn count_x11_requests(requests: &[u8], opcode: u8) -> usize {
        let mut offset = 0;
        let mut count = 0;
        while offset + 4 <= requests.len() {
            let words = u16::from_ne_bytes([requests[offset + 2], requests[offset + 3]]) as usize;
            assert!(words > 0, "unexpected BigRequests encoding");
            let bytes = words * 4;
            assert!(offset + bytes <= requests.len(), "truncated X11 request");
            count += usize::from(requests[offset] == opcode);
            offset += bytes;
        }
        assert_eq!(offset, requests.len(), "trailing X11 request bytes");
        count
    }

    fn transfer(read: OwnedFd, source: XwaylandDndSourceProxyId) -> DndOutgoingTransfer {
        DndOutgoingTransfer {
            id: XwaylandDndSourceTransferId {
                source,
                serial: NonZeroU64::new(1).unwrap(),
            },
            generation: source.adapter_id.generation(),
            source,
            target: crate::xwayland::X11WindowHandle::new(source.adapter_id.generation(), 0x77),
            requestor: 0x77,
            property: 0x88,
            property_type: 0x99,
            read,
            registered_fd: None,
            reactor_token: None,
            requestor_monitor_retained: false,
            phase: DndTransferPhase::ReadingDirect,
            buffer: VecDeque::with_capacity(4),
            eof: false,
            deadline_ns: 10,
            notification: ConversionNotification::Single {
                requestor: 0x77,
                time: 1,
                target_atom: 2,
                property: 0x88,
            },
            notify_ready: false,
            max_chunk_bytes: 4,
        }
    }

    #[test]
    fn source_pipe_reads_nonblocking_bounded_chunks_until_eof() {
        let generation = crate::xwayland::XwaylandGeneration::new(NonZeroU64::new(19).unwrap());
        let adapter_id = crate::xwayland::XwaylandDndAdapterId::new(
            crate::xwayland::CanonicalDndSessionId::Wayland(NonZeroU64::new(29).unwrap()),
            generation,
        )
        .unwrap();
        let source = XwaylandDndSourceProxyId {
            adapter_id,
            xid: 0x55,
        };
        let (read, write) = create_pipe().unwrap();
        let flags = unsafe { libc::fcntl(read.as_raw_fd(), libc::F_GETFL) };
        assert_ne!(flags & libc::O_NONBLOCK, 0);
        let bytes = *b"abcdef";
        assert_eq!(
            unsafe { libc::write(write.as_raw_fd(), bytes.as_ptr().cast(), bytes.len(),) },
            bytes.len() as isize
        );
        drop(write);

        let mut transfer = transfer(read, source);
        assert_eq!(
            read_until_limit(&mut transfer).unwrap(),
            ReadProgress::LimitReached
        );
        assert_eq!(transfer.buffer.make_contiguous(), b"abcd");
        transfer.buffer.clear();
        assert_eq!(read_until_limit(&mut transfer).unwrap(), ReadProgress::Eof);
        assert_eq!(transfer.buffer.make_contiguous(), b"ef");
        assert!(transfer.eof);
    }

    #[test]
    fn small_payload_completes_direct_conversion_once() {
        let (mut xwm, mut peer, source, target) = active_fixture();
        let id = start_payload(&mut xwm, source, target, b"small payload");
        assert!(resolve_requests(&mut xwm, [(id, true)], 20).unwrap());
        assert!(!xwm.data_bridge.dnd_outgoing.transfers.contains_key(&id));
        assert!(source_interests(&xwm).is_empty());
        assert!(!resolve_requests(&mut xwm, [(id, true)], 21).unwrap());

        xwm.flush().unwrap();
        let requests = read_requests(&mut peer);
        assert_eq!(count_x11_requests(&requests, 25), 1);
    }

    #[test]
    fn incr_payload_waits_for_each_property_delete_before_next_chunk() {
        let (mut xwm, _peer, source, target) = active_fixture();
        let id = start_payload(&mut xwm, source, target, b"abcdef");
        xwm.data_bridge
            .dnd_outgoing
            .transfers
            .get_mut(&id)
            .unwrap()
            .max_chunk_bytes = 4;

        assert!(resolve_requests(&mut xwm, [(id, true)], 20).unwrap());
        assert_eq!(
            xwm.data_bridge
                .dnd_outgoing
                .transfers
                .get(&id)
                .unwrap()
                .phase,
            DndTransferPhase::WaitingForInitialDelete
        );
        assert!(property_deleted(&mut xwm, target.xid(), 0x900, 21).unwrap());
        assert_eq!(
            xwm.data_bridge
                .dnd_outgoing
                .transfers
                .get(&id)
                .unwrap()
                .phase,
            DndTransferPhase::WaitingForChunkDelete
        );
        assert!(property_deleted(&mut xwm, target.xid(), 0x900, 22).unwrap());
        assert_eq!(
            xwm.data_bridge
                .dnd_outgoing
                .transfers
                .get(&id)
                .unwrap()
                .phase,
            DndTransferPhase::WaitingForSource
        );

        let interests = source_interests(&xwm);
        let [(interest_id, fd)] = interests.as_slice() else {
            panic!("one INCR source fd must await readiness");
        };
        assert_eq!(*interest_id, id);
        bind_reactor_token(&mut xwm, id, *fd, Some(1));
        assert!(handle_source_ready(&mut xwm, id, source.adapter_id.generation(), 1, 23).unwrap());
        assert_eq!(
            xwm.data_bridge
                .dnd_outgoing
                .transfers
                .get(&id)
                .unwrap()
                .phase,
            DndTransferPhase::WaitingForChunkDelete
        );
        assert!(property_deleted(&mut xwm, target.xid(), 0x900, 24).unwrap());
        assert!(!xwm.data_bridge.dnd_outgoing.transfers.contains_key(&id));
        assert!(source_interests(&xwm).is_empty());
    }

    #[test]
    fn requestor_destroy_and_timeout_retire_only_the_exact_transfer() {
        let (mut xwm, _peer, source, target) = active_fixture();
        let id = start_payload(&mut xwm, source, target, b"pending");
        requestor_destroyed(&mut xwm, target.xid()).unwrap();
        assert!(!xwm.data_bridge.dnd_outgoing.transfers.contains_key(&id));
        assert!(xwm.data_bridge.dnd_outgoing.take_requests().is_empty());
        assert!(source_interests(&xwm).is_empty());

        let id = start_payload(&mut xwm, source, target, b"timeout");
        expire_deadlines(&mut xwm, 10 + DND_TRANSFER_IDLE_TIMEOUT_NS).unwrap();
        assert!(!xwm.data_bridge.dnd_outgoing.transfers.contains_key(&id));
        assert!(xwm.data_bridge.dnd_outgoing.take_requests().is_empty());
        assert!(source_interests(&xwm).is_empty());
    }
}
