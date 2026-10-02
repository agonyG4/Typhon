use super::*;
use std::{io, os::fd::RawFd};

/// Start one exact Wayland receive request against this incoming XdndSelection
/// offer. The X timestamp is the most recent accepted Position timestamp.
pub(crate) fn start_data_request(
    xwm: &mut Xwm,
    request: crate::xwayland::XwaylandDndDataRequest,
    now_ns: u64,
) -> Result<Option<crate::xwayland::XwaylandDndIncomingTransferId>, XwmError> {
    if xwm.data_bridge.dnd_incoming.transfers.len() >= MAX_ACTIVE_INCOMING_DND_TRANSFERS {
        return Ok(None);
    }
    let Some((offer_id, source, target, timestamp)) = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .filter(|session| {
            session.generation == xwm.generation
                && session.offer_id == request.offer_id
                && session.canonical_started
                && session.metadata_complete
        })
        .and_then(|session| {
            let target = session
                .atom_to_mime
                .iter()
                .find_map(|(atom, mime)| (mime == &request.mime_type).then_some(*atom))?;
            let timestamp = session.latest_position?.timestamp;
            (timestamp != 0).then_some((session.offer_id, session.source.xid(), target, timestamp))
        })
    else {
        return Ok(None);
    };
    if request.offer_id.generation() != xwm.generation {
        return Ok(None);
    }
    // SAFETY: the owned sink remains alive until this function returns and
    // receives O_NONBLOCK while preserving its other status flags.
    let flags = unsafe { libc::fcntl(request.sink.as_raw_fd(), libc::F_GETFL) };
    if flags < 0 {
        return Ok(None);
    }
    if flags & libc::O_NONBLOCK == 0
        // SAFETY: this updates flags on the live owned descriptor.
        && unsafe {
            libc::fcntl(
                request.sink.as_raw_fd(),
                libc::F_SETFL,
                flags | libc::O_NONBLOCK,
            )
        } < 0
    {
        return Ok(None);
    }
    let requestor = xwm
        .connection
        .generate_id()
        .map_err(|error| XwmError::IdAllocation(error.to_string()))?;
    let create = xwm
        .connection
        .create_window(
            0,
            requestor,
            xwm.root,
            0,
            0,
            1,
            1,
            0,
            xproto::WindowClass::INPUT_ONLY,
            0,
            &xproto::CreateWindowAux::new().event_mask(xproto::EventMask::PROPERTY_CHANGE),
        )
        .map_err(XwmError::Connection)?;
    std::mem::forget(create);
    let Some(id) = xwm.data_bridge.dnd_incoming.allocate_transfer_id(offer_id) else {
        let _ = xwm.connection.destroy_window(requestor);
        return Ok(None);
    };
    let property = xwm.atoms.get(XwmAtomName::SelectionData);
    let transfer = IncomingTransfer {
        id,
        generation: xwm.generation,
        offer_id,
        source,
        requestor,
        target,
        property,
        selection_timestamp: timestamp,
        sink: Some(request.sink),
        phase: IncomingTransferPhase::AwaitSelectionNotify,
        mode: IncomingPropertyMode::Initial,
        offset_units: 0,
        bytes_after: 0,
        expected_type_format: None,
        buffer: Vec::new(),
        written: 0,
        continue_after_write: ContinueAfterWrite::None,
        idle_deadline_ns: now_ns.saturating_add(INCOMING_DND_IDLE_TIMEOUT_NS),
        sink_writable_interest: false,
        reactor_token: None,
        pending_reply: None,
    };
    xwm.data_bridge
        .dnd_incoming
        .requestors
        .insert(requestor, id);
    xwm.data_bridge.dnd.internal_windows.insert(requestor);
    xwm.data_bridge.dnd_incoming.transfers.insert(id, transfer);
    let cookie = xwm
        .connection
        .convert_selection(
            requestor,
            xwm.atoms.get(XwmAtomName::XdndSelection),
            target,
            property,
            timestamp,
        )
        .map_err(XwmError::Connection)?;
    std::mem::forget(cookie);
    xwm.connection.flush().map_err(XwmError::Connection)?;
    Ok(Some(id))
}

pub(crate) fn selection_notify(
    xwm: &mut Xwm,
    event: xproto::SelectionNotifyEvent,
    now_ns: u64,
) -> Result<bool, XwmError> {
    if event.selection != xwm.atoms.get(XwmAtomName::XdndSelection) {
        return Ok(false);
    }
    let Some(id) = xwm
        .data_bridge
        .dnd_incoming
        .requestors
        .get(&event.requestor)
        .copied()
    else {
        // Keep an unrelated XdndSelection notification out of the clipboard
        // transfer state machines. It has no authority over this adapter.
        return Ok(true);
    };
    let Some(transfer) = xwm.data_bridge.dnd_incoming.transfers.get(&id) else {
        return Ok(true);
    };
    let none = event.property == u32::from(AtomEnum::NONE);
    let exact_session = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .is_some_and(|session| {
            session.generation == transfer.generation
                && session.offer_id == transfer.offer_id
                && session.source.xid() == transfer.source
                && session.canonical_started
        });
    if transfer.generation != xwm.generation
        || transfer.offer_id != id.offer_id()
        || !exact_session
        || transfer.phase != IncomingTransferPhase::AwaitSelectionNotify
        || event.requestor != transfer.requestor
        || event.target != transfer.target
        || event.time != transfer.selection_timestamp
        || (!none && event.property != transfer.property)
    {
        finish_transfer(xwm, id);
        return Ok(true);
    }
    if none {
        finish_transfer(xwm, id);
        return Ok(true);
    }
    if let Some(transfer) = xwm.data_bridge.dnd_incoming.transfers.get_mut(&id) {
        transfer.phase = IncomingTransferPhase::ReadingProperty;
        transfer.mode = IncomingPropertyMode::Initial;
        transfer.offset_units = 0;
    }
    issue_transfer_property_read(xwm, id, now_ns)?;
    Ok(true)
}

pub(crate) fn property_notify(
    xwm: &mut Xwm,
    event: xproto::PropertyNotifyEvent,
    now_ns: u64,
) -> Result<bool, XwmError> {
    let Some(id) = xwm
        .data_bridge
        .dnd_incoming
        .requestors
        .get(&event.window)
        .copied()
    else {
        return Ok(false);
    };
    if event.atom != xwm.atoms.get(XwmAtomName::SelectionData) {
        return Ok(true);
    }
    if event.state != xproto::Property::NEW_VALUE {
        return Ok(true);
    }
    let Some(transfer) = xwm.data_bridge.dnd_incoming.transfers.get_mut(&id) else {
        return Ok(true);
    };
    if transfer.phase != IncomingTransferPhase::WaitingForIncrValue {
        return Ok(true);
    }
    transfer.phase = IncomingTransferPhase::ReadingProperty;
    transfer.mode = IncomingPropertyMode::Incr;
    transfer.offset_units = 0;
    issue_transfer_property_read(xwm, id, now_ns)?;
    Ok(true)
}

fn issue_transfer_property_read(
    xwm: &mut Xwm,
    id: crate::xwayland::XwaylandDndIncomingTransferId,
    now_ns: u64,
) -> Result<(), XwmError> {
    if xwm.data_bridge.dnd_incoming.transfer_replies.len()
        >= MAX_PENDING_INCOMING_DND_TRANSFER_REPLIES
    {
        return Ok(());
    }
    let Some(transfer) = xwm.data_bridge.dnd_incoming.transfers.get(&id) else {
        return Ok(());
    };
    let cookie = xwm
        .connection
        .get_property(
            false,
            transfer.requestor,
            transfer.property,
            AtomEnum::ANY,
            transfer.offset_units,
            INCOMING_DND_CHUNK_UNITS,
        )
        .map_err(XwmError::Connection)?;
    let sequence = cookie.sequence_number();
    std::mem::forget(cookie);
    if let Some(transfer) = xwm.data_bridge.dnd_incoming.transfers.get_mut(&id) {
        transfer.pending_reply = Some(sequence);
        transfer.idle_deadline_ns = now_ns.saturating_add(INCOMING_DND_IDLE_TIMEOUT_NS);
    }
    xwm.data_bridge
        .dnd_incoming
        .transfer_replies
        .insert(sequence, id);
    xwm.connection.flush().map_err(XwmError::Connection)
}

pub(super) fn poll_transfer_replies(
    xwm: &mut Xwm,
    budget: usize,
    now_ns: u64,
) -> Result<usize, XwmError> {
    let sequences = xwm
        .data_bridge
        .dnd_incoming
        .transfer_replies
        .keys()
        .copied()
        .take(budget)
        .collect::<Vec<_>>();
    let mut processed = 0;
    for sequence in sequences {
        let Some(id) = xwm
            .data_bridge
            .dnd_incoming
            .transfer_replies
            .get(&sequence)
            .copied()
        else {
            continue;
        };
        let cookie = Cookie::<
            super::super::super::connection::X11Connection,
            xproto::GetPropertyReply,
        >::new(&xwm.connection, sequence);
        let reply = match cookie.reply_unchecked() {
            Ok(reply) => reply,
            Err(x11rb::errors::ConnectionError::IoError(error))
                if error.kind() == io::ErrorKind::WouldBlock =>
            {
                continue;
            }
            Err(error) => return Err(XwmError::Connection(error)),
        };
        xwm.data_bridge
            .dnd_incoming
            .transfer_replies
            .remove(&sequence);
        if let Some(transfer) = xwm.data_bridge.dnd_incoming.transfers.get_mut(&id)
            && transfer.pending_reply == Some(sequence)
        {
            transfer.pending_reply = None;
        }
        processed += 1;
        let Some(reply) = reply else {
            finish_transfer(xwm, id);
            continue;
        };
        consume_transfer_property_reply(xwm, id, reply, now_ns)?;
    }
    let waiting = xwm
        .data_bridge
        .dnd_incoming
        .transfers
        .values()
        .filter_map(|transfer| {
            (transfer.phase == IncomingTransferPhase::ReadingProperty
                && transfer.pending_reply.is_none())
            .then_some(transfer.id)
        })
        .collect::<Vec<_>>();
    for id in waiting {
        if xwm.data_bridge.dnd_incoming.transfer_replies.len()
            >= MAX_PENDING_INCOMING_DND_TRANSFER_REPLIES
        {
            break;
        }
        issue_transfer_property_read(xwm, id, now_ns)?;
    }
    Ok(processed)
}

fn consume_transfer_property_reply(
    xwm: &mut Xwm,
    id: crate::xwayland::XwaylandDndIncomingTransferId,
    reply: xproto::GetPropertyReply,
    now_ns: u64,
) -> Result<(), XwmError> {
    let Some(mode) = xwm
        .data_bridge
        .dnd_incoming
        .transfers
        .get(&id)
        .map(|transfer| transfer.mode)
    else {
        return Ok(());
    };
    if reply.value.len() > MAX_INCOMING_DND_CHUNK_BYTES
        || !matches!(reply.format, 8 | 16 | 32)
        || reply.type_ == u32::from(AtomEnum::NONE)
    {
        finish_transfer(xwm, id);
        return Ok(());
    }
    if mode == IncomingPropertyMode::Initial && reply.type_ == xwm.atoms.get(XwmAtomName::Incr) {
        if reply.format != 32 || reply.value.len() != 4 || reply.bytes_after != 0 {
            finish_transfer(xwm, id);
            return Ok(());
        }
        let Some(transfer) = xwm.data_bridge.dnd_incoming.transfers.get_mut(&id) else {
            return Ok(());
        };
        transfer.mode = IncomingPropertyMode::Incr;
        transfer.phase = IncomingTransferPhase::WaitingForIncrValue;
        transfer.offset_units = 0;
        let cookie = xwm
            .connection
            .delete_property(transfer.requestor, transfer.property)
            .map_err(XwmError::Connection)?;
        std::mem::forget(cookie);
        xwm.connection.flush().map_err(XwmError::Connection)?;
        return Ok(());
    }
    let type_format = (reply.type_, reply.format);
    if reply.bytes_after > 0 && (reply.value.is_empty() || !reply.value.len().is_multiple_of(4)) {
        finish_transfer(xwm, id);
        return Ok(());
    }
    let units = u32::try_from(reply.value.len().div_ceil(4)).unwrap_or(u32::MAX);
    let Some(transfer) = xwm.data_bridge.dnd_incoming.transfers.get_mut(&id) else {
        return Ok(());
    };
    if transfer
        .expected_type_format
        .is_some_and(|expected| expected != type_format)
    {
        finish_transfer(xwm, id);
        return Ok(());
    }
    transfer.expected_type_format = Some(type_format);
    if mode == IncomingPropertyMode::Initial {
        transfer.mode = IncomingPropertyMode::Direct;
    }
    if !reply.value.is_empty() {
        transfer.buffer = reply.value;
        transfer.written = 0;
    }
    transfer.bytes_after = reply.bytes_after;
    if reply.bytes_after > 0 {
        let Some(next_offset) = transfer.offset_units.checked_add(units) else {
            finish_transfer(xwm, id);
            return Ok(());
        };
        transfer.offset_units = next_offset;
    }
    transfer.continue_after_write =
        next_after_property_chunk(mode, reply.bytes_after, transfer.buffer.is_empty());
    transfer.idle_deadline_ns = now_ns.saturating_add(INCOMING_DND_IDLE_TIMEOUT_NS);
    if transfer.buffer.is_empty() {
        resume_after_transfer_buffer(xwm, id, now_ns)?;
    } else {
        write_transfer_buffer(xwm, id, now_ns)?;
    }
    Ok(())
}

pub(super) fn next_after_property_chunk(
    mode: IncomingPropertyMode,
    bytes_after: u32,
    value_empty: bool,
) -> ContinueAfterWrite {
    if bytes_after > 0 {
        ContinueAfterWrite::ReadMoreProperty
    } else if mode == IncomingPropertyMode::Incr && !value_empty {
        ContinueAfterWrite::ReadNextIncrChunk
    } else {
        ContinueAfterWrite::Finish
    }
}

fn write_transfer_buffer(
    xwm: &mut Xwm,
    id: crate::xwayland::XwaylandDndIncomingTransferId,
    now_ns: u64,
) -> Result<(), XwmError> {
    let mut calls = 0;
    let mut eintr_retries = 0;
    loop {
        let Some(transfer) = xwm.data_bridge.dnd_incoming.transfers.get(&id) else {
            return Ok(());
        };
        if transfer.buffer.is_empty() || transfer.written >= transfer.buffer.len() {
            if let Some(transfer) = xwm.data_bridge.dnd_incoming.transfers.get_mut(&id) {
                transfer.buffer.clear();
                transfer.written = 0;
                transfer.sink_writable_interest = false;
            }
            return resume_after_transfer_buffer(xwm, id, now_ns);
        }
        let Some(sink) = transfer.sink.as_ref() else {
            if let Some(transfer) = xwm.data_bridge.dnd_incoming.transfers.get_mut(&id) {
                transfer.buffer.clear();
                transfer.written = 0;
            }
            return resume_after_transfer_buffer(xwm, id, now_ns);
        };
        if calls >= MAX_WRITE_CALLS_PER_DISPATCH {
            if let Some(transfer) = xwm.data_bridge.dnd_incoming.transfers.get_mut(&id) {
                transfer.sink_writable_interest = true;
            }
            return Ok(());
        }
        let fd = sink.as_raw_fd();
        let start = transfer.written;
        let bytes = &transfer.buffer[start..];
        // SAFETY: sink ownership and the source slice are held for this call.
        let result = write_sink_bytes(fd, bytes);
        calls += 1;
        if let Ok(written) = result {
            if written == 0 {
                finish_transfer(xwm, id);
                return Ok(());
            }
            let transfer = xwm
                .data_bridge
                .dnd_incoming
                .transfers
                .get_mut(&id)
                .expect("transfer remains active during write");
            transfer.written = transfer.written.saturating_add(written);
            transfer.sink_writable_interest = false;
            transfer.idle_deadline_ns = now_ns.saturating_add(INCOMING_DND_IDLE_TIMEOUT_NS);
            continue;
        }
        let error = result.expect_err("write result was checked as an error");
        if error.kind() == io::ErrorKind::Interrupted
            && eintr_retries < MAX_EINTR_RETRIES_PER_DISPATCH
        {
            eintr_retries += 1;
            continue;
        }
        if error.kind() == io::ErrorKind::WouldBlock || error.kind() == io::ErrorKind::Interrupted {
            if let Some(transfer) = xwm.data_bridge.dnd_incoming.transfers.get_mut(&id) {
                transfer.sink_writable_interest = true;
            }
            return Ok(());
        }
        // A closed Wayland fd retires only this payload transaction. The
        // X selection protocol is drained without retaining the sink.
        let transfer = xwm
            .data_bridge
            .dnd_incoming
            .transfers
            .get_mut(&id)
            .expect("transfer remains active after write error");
        transfer.sink.take();
        transfer.sink_writable_interest = false;
        transfer.buffer.clear();
        transfer.written = 0;
        return resume_after_transfer_buffer(xwm, id, now_ns);
    }
}

pub(super) fn write_sink_bytes(fd: RawFd, bytes: &[u8]) -> io::Result<usize> {
    // SAFETY: callers hold the live sink descriptor and byte slice across the
    // nonblocking write; the kernel reads no more than `bytes.len()` bytes.
    let result = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
    if result >= 0 {
        Ok(result as usize)
    } else {
        Err(io::Error::last_os_error())
    }
}

fn resume_after_transfer_buffer(
    xwm: &mut Xwm,
    id: crate::xwayland::XwaylandDndIncomingTransferId,
    now_ns: u64,
) -> Result<(), XwmError> {
    let Some((mode, continuation, after, requestor, property)) = xwm
        .data_bridge
        .dnd_incoming
        .transfers
        .get(&id)
        .map(|transfer| {
            (
                transfer.mode,
                transfer.continue_after_write,
                transfer.bytes_after,
                transfer.requestor,
                transfer.property,
            )
        })
    else {
        return Ok(());
    };
    if continuation == ContinueAfterWrite::ReadMoreProperty {
        if let Some(transfer) = xwm.data_bridge.dnd_incoming.transfers.get_mut(&id) {
            transfer.phase = IncomingTransferPhase::ReadingProperty;
        }
        return issue_transfer_property_read(xwm, id, now_ns);
    }
    if continuation == ContinueAfterWrite::ReadNextIncrChunk {
        let cookie = xwm
            .connection
            .delete_property(requestor, property)
            .map_err(XwmError::Connection)?;
        std::mem::forget(cookie);
        if let Some(transfer) = xwm.data_bridge.dnd_incoming.transfers.get_mut(&id) {
            transfer.phase = IncomingTransferPhase::WaitingForIncrValue;
            transfer.offset_units = 0;
            transfer.bytes_after = 0;
            transfer.continue_after_write = ContinueAfterWrite::None;
        }
        xwm.connection.flush().map_err(XwmError::Connection)?;
        return Ok(());
    }
    if continuation == ContinueAfterWrite::Finish {
        let _ = after;
        let cookie = xwm
            .connection
            .delete_property(requestor, property)
            .map_err(XwmError::Connection)?;
        std::mem::forget(cookie);
        let _ = mode;
        finish_transfer(xwm, id);
        xwm.connection.flush().map_err(XwmError::Connection)?;
    }
    Ok(())
}

pub(super) fn finish_transfer(xwm: &mut Xwm, id: crate::xwayland::XwaylandDndIncomingTransferId) {
    let Some(transfer) = xwm.data_bridge.dnd_incoming.transfers.remove(&id) else {
        return;
    };
    if let Some(sequence) = transfer.pending_reply {
        xwm.data_bridge
            .dnd_incoming
            .transfer_replies
            .remove(&sequence);
        xwm.connection.discard_reply(
            sequence,
            x11rb::connection::RequestKind::HasResponse,
            x11rb::connection::DiscardMode::DiscardReply,
        );
    }
    xwm.data_bridge
        .dnd_incoming
        .requestors
        .remove(&transfer.requestor);
    xwm.data_bridge
        .dnd_incoming
        .retired_requestors
        .insert(transfer.requestor);
    if xwm.data_bridge.dnd_incoming.retired_requestors.len() > MAX_ACTIVE_INCOMING_DND_TRANSFERS * 2
        && let Some(oldest) = xwm
            .data_bridge
            .dnd_incoming
            .retired_requestors
            .iter()
            .next()
            .copied()
    {
        xwm.data_bridge
            .dnd_incoming
            .retired_requestors
            .remove(&oldest);
    }
    xwm.data_bridge
        .dnd
        .internal_windows
        .remove(&transfer.requestor);
    let _ = xwm.connection.destroy_window(transfer.requestor);
}

pub(crate) fn handle_sink_ready(
    xwm: &mut Xwm,
    id: crate::xwayland::XwaylandDndIncomingTransferId,
    generation: XwaylandGeneration,
    reactor_token: u64,
    flags: u32,
    now_ns: u64,
) -> Result<bool, XwmError> {
    if generation != xwm.generation
        || !xwm
            .data_bridge
            .dnd_incoming
            .transfer_matches_reactor(id, generation, reactor_token)
    {
        return Ok(false);
    }
    let terminal = libc::EPOLLERR as u32 | libc::EPOLLHUP as u32 | libc::EPOLLRDHUP as u32;
    let before = xwm
        .data_bridge
        .dnd_incoming
        .transfers
        .get(&id)
        .map(|transfer| transfer.sink_writable_interest);
    if flags & terminal != 0 {
        if let Some(transfer) = xwm.data_bridge.dnd_incoming.transfers.get_mut(&id) {
            transfer.sink.take();
            transfer.sink_writable_interest = false;
            transfer.buffer.clear();
            transfer.written = 0;
        }
        resume_after_transfer_buffer(xwm, id, now_ns)?;
    } else if flags & libc::EPOLLOUT as u32 != 0 {
        write_transfer_buffer(xwm, id, now_ns)?;
    }
    let after = xwm
        .data_bridge
        .dnd_incoming
        .transfers
        .get(&id)
        .map(|transfer| transfer.sink_writable_interest);
    Ok(before != after)
}

pub(crate) fn source_destroyed(xwm: &mut Xwm, source: Window) -> Result<bool, XwmError> {
    let Some(offer_id) = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .filter(|session| session.source.xid() == source)
        .map(|session| session.offer_id)
    else {
        return Ok(false);
    };
    let _ = super::metadata::leave_offer(xwm, offer_id);
    Ok(true)
}

pub(crate) fn requestor_destroyed(xwm: &mut Xwm, requestor: Window) -> bool {
    let Some(id) = xwm
        .data_bridge
        .dnd_incoming
        .requestors
        .get(&requestor)
        .copied()
    else {
        return false;
    };
    finish_transfer(xwm, id);
    true
}

pub(crate) fn canonical_retired(xwm: &mut Xwm, offer_id: XwaylandDndOfferId) {
    super::metadata::retire_offer_transfers(xwm, offer_id);
    let _ = xwm.data_bridge.dnd.retire_incoming_session(offer_id);
    super::metadata::cancel_metadata_replies(xwm, Some(offer_id));
}
