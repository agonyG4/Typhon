//! Bounded Wayland-source-to-X11 selection byte transport.
//!
//! Unlike the incoming B2 payload manager, this path reads a nonblocking
//! Typhon-owned pipe end and uses X11 property deletion as INCR backpressure.

use std::{
    collections::{BTreeMap, VecDeque},
    io,
    num::NonZeroU64,
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
};

use x11rb::{
    connection::{Connection, RequestConnection},
    protocol::xproto::PropMode,
    wrapper::ConnectionExt as XprotoWrapperExt,
};

use super::{
    Xwm, XwmError,
    data_bridge::BridgeGeneration,
    selection_proxy::{MAX_PENDING_PROXY_SELECTION_REQUESTS, ProxySelectionRequestId},
};
use crate::xwayland::{
    XwaylandProxySelectionDataRequest, XwaylandProxySelectionId, XwaylandProxySelectionTransferId,
};

pub(crate) const MAX_ACTIVE_OUTGOING_SELECTION_TRANSFERS: usize = 64;
pub(crate) const MAX_OUTGOING_SELECTION_CHUNK_BYTES: usize = 64 * 1024;
pub(crate) const OUTGOING_SELECTION_IDLE_TIMEOUT_NS: u64 = 30_000_000_000;
const MAX_EINTR_RETRIES_PER_DISPATCH: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutgoingPhase {
    AwaitSourceAcceptance,
    ReadingDirect,
    WaitingForInitialDelete,
    WaitingForChunkDelete,
    WaitingForSource,
    WaitingForFinalDelete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ConversionOwner {
    pub request_id: ProxySelectionRequestId,
    pub multiple_pair: Option<usize>,
}

#[derive(Debug)]
pub(crate) struct StartTransfer {
    pub proxy_id: XwaylandProxySelectionId,
    pub requestor: u32,
    pub target: u32,
    pub property: u32,
    pub property_type: u32,
    pub mime_type: String,
    pub owner: ConversionOwner,
    pub now_ns: u64,
}

#[derive(Debug)]
pub(crate) struct OutgoingSelectionTransfer {
    pub id: XwaylandProxySelectionTransferId,
    pub generation: BridgeGeneration,
    pub proxy_id: XwaylandProxySelectionId,
    pub kind: crate::xwayland::XwaylandSelectionKind,
    pub requestor: u32,
    pub target: u32,
    pub property: u32,
    pub property_type: u32,
    pub mime_type: String,
    pub read: OwnedFd,
    pub registered_fd: Option<RawFd>,
    pub reactor_token: Option<u64>,
    pub phase: OutgoingPhase,
    pub buffer: VecDeque<u8>,
    pub eof: bool,
    pub deadline_ns: u64,
    pub owner: ConversionOwner,
    pub notify_ready: bool,
    pub max_chunk_bytes: usize,
}

#[derive(Debug, Default)]
pub(crate) struct SelectionOutgoingManager {
    pub active_generation: Option<BridgeGeneration>,
    pub transfers: BTreeMap<XwaylandProxySelectionTransferId, OutgoingSelectionTransfer>,
    pub pending_requests: VecDeque<XwaylandProxySelectionDataRequest>,
    next_transfer_id: u64,
}

impl SelectionOutgoingManager {
    pub(crate) fn initialize_generation(&mut self, generation: BridgeGeneration) {
        if self.active_generation == Some(generation) {
            return;
        }
        self.transfers.clear();
        self.pending_requests.clear();
        self.active_generation = Some(generation);
    }

    pub(crate) fn clear_generation(
        &mut self,
        generation: BridgeGeneration,
    ) -> Vec<x11rb::connection::SequenceNumber> {
        if self.active_generation != Some(generation) {
            return Vec::new();
        }
        self.active_generation = None;
        self.transfers.clear();
        self.pending_requests.clear();
        Vec::new()
    }

    pub(crate) fn active_count(&self) -> usize {
        self.transfers.len()
    }

    pub(crate) fn take_requests(&mut self) -> Vec<XwaylandProxySelectionDataRequest> {
        self.pending_requests.drain(..).collect()
    }

    pub(crate) fn source_interests(
        &self,
    ) -> impl Iterator<Item = (XwaylandProxySelectionTransferId, RawFd)> + '_ {
        self.transfers.values().filter_map(|transfer| {
            (transfer.phase != OutgoingPhase::AwaitSourceAcceptance
                && transfer.phase != OutgoingPhase::WaitingForInitialDelete
                && transfer.phase != OutgoingPhase::WaitingForChunkDelete
                && transfer.phase != OutgoingPhase::WaitingForFinalDelete)
                .then_some((transfer.id, transfer.read.as_raw_fd()))
        })
    }

    pub(crate) fn bind_reactor_token(
        &mut self,
        id: XwaylandProxySelectionTransferId,
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
        id: XwaylandProxySelectionTransferId,
        generation: BridgeGeneration,
        token: u64,
    ) -> bool {
        self.transfers.get(&id).is_some_and(|transfer| {
            transfer.generation == generation
                && transfer.reactor_token == Some(token)
                && transfer.registered_fd == Some(transfer.read.as_raw_fd())
                && self.active_generation == Some(generation)
                && transfer.phase != OutgoingPhase::AwaitSourceAcceptance
        })
    }

    pub(crate) fn next_deadline_ns(&self) -> Option<u64> {
        self.transfers
            .values()
            .map(|transfer| transfer.deadline_ns)
            .min()
    }
}

pub(crate) fn start_transfer(
    xwm: &mut Xwm,
    params: StartTransfer,
) -> Result<Option<XwaylandProxySelectionTransferId>, XwmError> {
    let StartTransfer {
        proxy_id,
        requestor,
        target,
        property,
        property_type,
        mime_type,
        owner,
        now_ns,
    } = params;
    let generation = BridgeGeneration::from(xwm.generation);
    let manager = &xwm.data_bridge.selection_outgoing;
    if manager.active_generation != Some(generation)
        || manager.transfers.len() >= MAX_ACTIVE_OUTGOING_SELECTION_TRANSFERS
        || manager.pending_requests.len() >= MAX_PENDING_PROXY_SELECTION_REQUESTS
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
        .map(|bytes| bytes.min(MAX_OUTGOING_SELECTION_CHUNK_BYTES))
        .filter(|bytes| *bytes > 0);
    let Some(max_chunk_bytes) = max_chunk_bytes else {
        return Ok(None);
    };
    let Some(next) = manager.next_transfer_id.checked_add(1) else {
        return Ok(None);
    };
    let Some(id_value) = NonZeroU64::new(next) else {
        return Ok(None);
    };
    let id = XwaylandProxySelectionTransferId::new(id_value);
    let (read, write) = match create_pipe() {
        Ok(pipe) => pipe,
        Err(_) => return Ok(None),
    };
    if !super::selection_proxy::retain_requestor(xwm, requestor)? {
        return Ok(None);
    }
    let manager = &mut xwm.data_bridge.selection_outgoing;
    manager.next_transfer_id = next;
    manager
        .pending_requests
        .push_back(XwaylandProxySelectionDataRequest {
            transfer_id: id,
            proxy_id,
            mime_type: mime_type.clone(),
            sink: write,
        });
    manager.transfers.insert(
        id,
        OutgoingSelectionTransfer {
            id,
            generation,
            proxy_id,
            kind: proxy_id.kind,
            requestor,
            target,
            property,
            property_type,
            mime_type,
            read,
            registered_fd: None,
            reactor_token: None,
            phase: OutgoingPhase::AwaitSourceAcceptance,
            buffer: VecDeque::with_capacity(max_chunk_bytes),
            eof: false,
            deadline_ns: now_ns.saturating_add(OUTGOING_SELECTION_IDLE_TIMEOUT_NS),
            owner,
            notify_ready: false,
            max_chunk_bytes,
        },
    );
    Ok(Some(id))
}

pub(crate) fn resolve_acceptance(
    xwm: &mut Xwm,
    id: XwaylandProxySelectionTransferId,
    accepted: bool,
    now_ns: u64,
) -> Result<bool, XwmError> {
    let Some(mut transfer) = xwm.data_bridge.selection_outgoing.transfers.remove(&id) else {
        return Ok(false);
    };
    if transfer.phase != OutgoingPhase::AwaitSourceAcceptance
        || transfer.generation != BridgeGeneration::from(xwm.generation)
    {
        xwm.data_bridge
            .selection_outgoing
            .transfers
            .insert(id, transfer);
        return Ok(false);
    }
    transfer.deadline_ns = now_ns.saturating_add(OUTGOING_SELECTION_IDLE_TIMEOUT_NS);
    if !accepted {
        finish_transfer(xwm, transfer, Some(false), now_ns)?;
        return Ok(true);
    }
    super::selection_proxy::conversion_progress(xwm, transfer.owner, now_ns);
    transfer.phase = OutgoingPhase::ReadingDirect;
    let keep = pump_direct(xwm, &mut transfer, now_ns)?;
    if keep {
        xwm.data_bridge
            .selection_outgoing
            .transfers
            .insert(id, transfer);
    }
    Ok(true)
}

pub(crate) fn take_requests(xwm: &mut Xwm) -> Vec<XwaylandProxySelectionDataRequest> {
    xwm.data_bridge.selection_outgoing.take_requests()
}

pub(crate) fn resolve_requests(
    xwm: &mut Xwm,
    results: impl IntoIterator<Item = (XwaylandProxySelectionTransferId, bool)>,
    now_ns: u64,
) -> Result<bool, XwmError> {
    let mut changed = false;
    for (id, accepted) in results {
        changed |= resolve_acceptance(xwm, id, accepted, now_ns)?;
    }
    Ok(changed)
}

fn pump_direct(
    xwm: &mut Xwm,
    transfer: &mut OutgoingSelectionTransfer,
    now_ns: u64,
) -> Result<bool, XwmError> {
    let progress = match read_until_limit(transfer) {
        Ok(progress) => progress,
        Err(_) => {
            super::selection_proxy::conversion_ready(xwm, transfer.owner, false, now_ns)?;
            super::selection_proxy::release_requestor(xwm, transfer.requestor, false)?;
            return Ok(false);
        }
    };
    match progress {
        ReadProgress::WouldBlock if transfer.buffer.len() < transfer.max_chunk_bytes => {
            transfer.phase = OutgoingPhase::ReadingDirect;
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
            super::selection_proxy::conversion_ready(xwm, transfer.owner, true, now_ns)?;
            super::selection_proxy::release_requestor(xwm, transfer.requestor, false)?;
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
                    xwm.atoms.get(super::atoms::XwmAtomName::Incr),
                    &[lower_bound],
                )
                .map_err(XwmError::Connection)?;
            std::mem::forget(cookie);
            xwm.connection.flush().map_err(XwmError::Connection)?;
            transfer.phase = OutgoingPhase::WaitingForInitialDelete;
            transfer.notify_ready = true;
            transfer.deadline_ns = now_ns.saturating_add(OUTGOING_SELECTION_IDLE_TIMEOUT_NS);
            super::selection_proxy::conversion_ready(xwm, transfer.owner, true, now_ns)?;
            Ok(true)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadProgress {
    WouldBlock,
    Eof,
    LimitReached,
}

fn read_until_limit(transfer: &mut OutgoingSelectionTransfer) -> Result<ReadProgress, XwmError> {
    let mut eintr_retries = 0;
    let mut scratch = [0_u8; 8192];
    while transfer.buffer.len() < transfer.max_chunk_bytes && !transfer.eof {
        let remaining = transfer.max_chunk_bytes - transfer.buffer.len();
        let limit = remaining.min(scratch.len());
        let result = unsafe {
            libc::read(
                transfer.read.as_raw_fd(),
                scratch.as_mut_ptr().cast(),
                limit,
            )
        };
        if result > 0 {
            let count = result as usize;
            transfer.buffer.extend(&scratch[..count]);
            eintr_retries = 0;
            continue;
        }
        if result == 0 {
            transfer.eof = true;
            return Ok(ReadProgress::Eof);
        }
        let error = io::Error::last_os_error();
        match error.kind() {
            io::ErrorKind::WouldBlock => return Ok(ReadProgress::WouldBlock),
            io::ErrorKind::Interrupted if eintr_retries < MAX_EINTR_RETRIES_PER_DISPATCH => {
                eintr_retries += 1;
            }
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

fn finish_transfer(
    xwm: &mut Xwm,
    transfer: OutgoingSelectionTransfer,
    notify: Option<bool>,
    now_ns: u64,
) -> Result<(), XwmError> {
    if let Some(success) = notify {
        super::selection_proxy::conversion_ready(xwm, transfer.owner, success, now_ns)?;
    }
    super::selection_proxy::release_requestor(xwm, transfer.requestor, false)
}

pub(crate) fn handle_source_ready(
    xwm: &mut Xwm,
    id: XwaylandProxySelectionTransferId,
    generation: BridgeGeneration,
    token: u64,
    _flags: u32,
    now_ns: u64,
) -> Result<bool, XwmError> {
    if !xwm
        .data_bridge
        .selection_outgoing
        .transfer_matches_reactor(id, generation, token)
    {
        return Ok(false);
    }
    let Some(mut transfer) = xwm.data_bridge.selection_outgoing.transfers.remove(&id) else {
        return Ok(false);
    };
    let before = transfer.phase;
    let bytes_before = transfer.buffer.len();
    let eof_before = transfer.eof;
    let keep = match transfer.phase {
        OutgoingPhase::ReadingDirect => pump_direct(xwm, &mut transfer, now_ns)?,
        OutgoingPhase::WaitingForSource => pump_incremental_source(xwm, &mut transfer, now_ns)?,
        _ => true,
    };
    if transfer.buffer.len() > bytes_before || (transfer.eof && !eof_before) {
        transfer.deadline_ns = now_ns.saturating_add(OUTGOING_SELECTION_IDLE_TIMEOUT_NS);
        super::selection_proxy::conversion_progress(xwm, transfer.owner, now_ns);
    }
    if keep {
        xwm.data_bridge
            .selection_outgoing
            .transfers
            .insert(id, transfer);
    }
    let after = xwm
        .data_bridge
        .selection_outgoing
        .transfers
        .get(&id)
        .map(|transfer| transfer.phase);
    Ok(after != Some(before))
}

fn pump_incremental_source(
    xwm: &mut Xwm,
    transfer: &mut OutgoingSelectionTransfer,
    now_ns: u64,
) -> Result<bool, XwmError> {
    let progress = match read_until_limit(transfer) {
        Ok(progress) => progress,
        Err(_) => {
            let _ = publish_bytes(
                xwm,
                transfer.requestor,
                transfer.property,
                transfer.property_type,
                &[],
            );
            super::selection_proxy::release_requestor(xwm, transfer.requestor, false)?;
            return Ok(false);
        }
    };
    match progress {
        ReadProgress::WouldBlock if transfer.buffer.is_empty() => {
            transfer.phase = OutgoingPhase::WaitingForSource;
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
            super::selection_proxy::release_requestor(xwm, transfer.requestor, false)?;
            Ok(false)
        }
        ReadProgress::WouldBlock | ReadProgress::LimitReached | ReadProgress::Eof => {
            publish_one_chunk(xwm, transfer, now_ns)?;
            Ok(true)
        }
    }
}

fn publish_one_chunk(
    xwm: &mut Xwm,
    transfer: &mut OutgoingSelectionTransfer,
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
    transfer.phase = OutgoingPhase::WaitingForChunkDelete;
    transfer.deadline_ns = now_ns.saturating_add(OUTGOING_SELECTION_IDLE_TIMEOUT_NS);
    super::selection_proxy::conversion_progress(xwm, transfer.owner, now_ns);
    Ok(())
}

pub(crate) fn owns_property(xwm: &Xwm, requestor: u32, property: u32) -> bool {
    xwm.data_bridge
        .selection_outgoing
        .transfers
        .values()
        .any(|transfer| {
            transfer.requestor == requestor
                && transfer.property == property
                && matches!(
                    transfer.phase,
                    OutgoingPhase::WaitingForInitialDelete
                        | OutgoingPhase::WaitingForChunkDelete
                        | OutgoingPhase::WaitingForFinalDelete
                )
        })
}

pub(crate) fn property_in_use(xwm: &Xwm, requestor: u32, property: u32) -> bool {
    xwm.data_bridge
        .selection_outgoing
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
        .selection_outgoing
        .transfers
        .iter()
        .find_map(|(id, transfer)| {
            (transfer.requestor == requestor
                && transfer.property == property
                && matches!(
                    transfer.phase,
                    OutgoingPhase::WaitingForInitialDelete | OutgoingPhase::WaitingForChunkDelete
                ))
            .then_some(*id)
        });
    let Some(id) = id else { return Ok(false) };
    let Some(mut transfer) = xwm.data_bridge.selection_outgoing.transfers.remove(&id) else {
        return Ok(false);
    };
    transfer.deadline_ns = now_ns.saturating_add(OUTGOING_SELECTION_IDLE_TIMEOUT_NS);
    super::selection_proxy::conversion_progress(xwm, transfer.owner, now_ns);
    let keep = match transfer.phase {
        OutgoingPhase::WaitingForInitialDelete | OutgoingPhase::WaitingForChunkDelete => {
            if !transfer.buffer.is_empty() {
                publish_one_chunk(xwm, &mut transfer, now_ns)?;
                true
            } else if transfer.eof {
                publish_bytes(
                    xwm,
                    transfer.requestor,
                    transfer.property,
                    transfer.property_type,
                    &[],
                )?;
                finish_transfer(xwm, transfer, None, now_ns)?;
                return Ok(true);
            } else {
                transfer.phase = OutgoingPhase::WaitingForSource;
                true
            }
        }
        _ => true,
    };
    if keep {
        xwm.data_bridge
            .selection_outgoing
            .transfers
            .insert(id, transfer);
    }
    Ok(true)
}

pub(crate) fn create_pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [-1; 2];
    // Set close-on-exec on both ends, then make only the Typhon read end
    // nonblocking. The Wayland source must retain ordinary blocking writes.
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
pub(crate) fn next_transfer_id_for_test(
    manager: &mut SelectionOutgoingManager,
) -> Option<XwaylandProxySelectionTransferId> {
    manager.next_transfer_id = manager.next_transfer_id.checked_add(1)?;
    Some(XwaylandProxySelectionTransferId::new(NonZeroU64::new(
        manager.next_transfer_id,
    )?))
}

pub(crate) fn cancel_requestor(xwm: &mut Xwm, requestor: u32) {
    let ids = xwm
        .data_bridge
        .selection_outgoing
        .transfers
        .values()
        .filter(|transfer| transfer.requestor == requestor)
        .map(|transfer| transfer.id)
        .collect::<Vec<_>>();
    let ids = ids.into_iter().collect::<std::collections::HashSet<_>>();
    for id in &ids {
        xwm.data_bridge.selection_outgoing.transfers.remove(id);
    }
    xwm.data_bridge
        .selection_outgoing
        .pending_requests
        .retain(|request| !ids.contains(&request.transfer_id));
}

pub(crate) fn cancel_owner(
    xwm: &mut Xwm,
    request_id: ProxySelectionRequestId,
    _now_ns: u64,
) -> Result<bool, XwmError> {
    let ids = xwm
        .data_bridge
        .selection_outgoing
        .transfers
        .values()
        .filter(|transfer| transfer.owner.request_id == request_id)
        .map(|transfer| transfer.id)
        .collect::<Vec<_>>();
    for id in &ids {
        if let Some(transfer) = xwm.data_bridge.selection_outgoing.transfers.remove(id) {
            xwm.data_bridge
                .selection_outgoing
                .pending_requests
                .retain(|request| request.transfer_id != *id);
            super::selection_proxy::release_requestor(xwm, transfer.requestor, false)?;
        }
    }
    Ok(!ids.is_empty())
}

pub(crate) fn cancel_channel(
    xwm: &mut Xwm,
    kind: crate::xwayland::XwaylandSelectionKind,
) -> Result<(), XwmError> {
    let ids = xwm
        .data_bridge
        .selection_outgoing
        .transfers
        .values()
        .filter(|transfer| transfer.kind == kind)
        .map(|transfer| transfer.id)
        .collect::<Vec<_>>();
    for id in ids {
        let Some(transfer) = xwm.data_bridge.selection_outgoing.transfers.remove(&id) else {
            continue;
        };
        xwm.data_bridge
            .selection_outgoing
            .pending_requests
            .retain(|request| request.transfer_id != id);
        super::selection_proxy::release_requestor(xwm, transfer.requestor, false)?;
    }
    Ok(())
}

pub(crate) fn expire_deadlines(xwm: &mut Xwm, now_ns: u64) -> Result<(), XwmError> {
    let expired = xwm
        .data_bridge
        .selection_outgoing
        .transfers
        .values()
        .filter(|transfer| transfer.deadline_ns <= now_ns)
        .map(|transfer| transfer.id)
        .collect::<Vec<_>>();
    for id in expired {
        let Some(transfer) = xwm.data_bridge.selection_outgoing.transfers.remove(&id) else {
            continue;
        };
        xwm.data_bridge
            .selection_outgoing
            .pending_requests
            .retain(|request| request.transfer_id != id);
        let notify = (!transfer.notify_ready).then_some(false);
        finish_transfer(xwm, transfer, notify, now_ns)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn pipe_flag_set_is_read_end_only() {
        use super::create_pipe;
        use std::os::fd::AsRawFd;
        let (read, write) = create_pipe().expect("create bounded pipe");
        let read_flags = unsafe { libc::fcntl(read.as_raw_fd(), libc::F_GETFL) };
        let write_flags = unsafe { libc::fcntl(write.as_raw_fd(), libc::F_GETFL) };
        assert_ne!(read_flags & libc::O_NONBLOCK, 0);
        assert_eq!(write_flags & libc::O_NONBLOCK, 0);
        assert_ne!(
            unsafe { libc::fcntl(read.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        assert_ne!(
            unsafe { libc::fcntl(write.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
    }
}
