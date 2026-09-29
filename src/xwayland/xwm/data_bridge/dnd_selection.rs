//! `XdndSelection` request routing, including bounded MULTIPLE conversion.

use std::collections::HashSet;

use x11rb::{
    protocol::xproto::{self, AtomEnum, ConnectionExt as XprotoConnectionExt, PropMode},
    wrapper::ConnectionExt as XprotoWrapperExt,
};

use super::{
    super::{X11WindowHandle, Xwm, XwmError, atoms::XwmAtomName},
    dnd::{
        DndPendingReply, DndWireProgress, MAX_MULTIPLE_PAIRS, MAX_PENDING_DND_REPLIES,
        SOURCE_OWNERSHIP_TIMEOUT_NS,
    },
};
use crate::xwayland::XwaylandDndAdapterId;

#[derive(Clone, Copy)]
pub(super) struct MultipleRequestContext {
    pub id: XwaylandDndAdapterId,
    pub source_proxy: u32,
    pub target: X11WindowHandle,
    pub requestor: u32,
    pub property: u32,
    pub request_time: u32,
    pub deadline_ns: u64,
}

struct ValidSelectionRequest {
    id: XwaylandDndAdapterId,
    source_proxy: u32,
    target: X11WindowHandle,
    timestamp: u32,
    mime_targets: Vec<(String, u32)>,
}

pub(crate) fn selection_request(
    xwm: &mut Xwm,
    event: xproto::SelectionRequestEvent,
    now_ns: u64,
) -> Result<bool, XwmError> {
    if event.selection != xwm.atoms.get(XwmAtomName::XdndSelection) {
        return Ok(false);
    }
    let multiple_atom = xwm.atoms.get(XwmAtomName::Multiple);
    let is_multiple = event.target == multiple_atom;
    if is_multiple && event.property == x11rb::NONE {
        super::super::dnd_outgoing::send_multiple_selection_notify(
            xwm,
            event.requestor,
            event.time,
            None,
        )?;
        return Ok(true);
    }
    let property = if event.property == x11rb::NONE && !is_multiple {
        event.target
    } else {
        event.property
    };
    let Some(request) = valid_selection_request(xwm, &event) else {
        if is_multiple {
            super::super::dnd_outgoing::send_multiple_selection_notify(
                xwm,
                event.requestor,
                event.time,
                None,
            )?;
        } else {
            super::super::dnd_outgoing::complete_single_conversion(
                xwm,
                event.requestor,
                event.time,
                event.target,
                property,
                false,
            )?;
        }
        return Ok(true);
    };
    if !selection_time_is_current_or_after(event.time, request.timestamp) {
        if is_multiple {
            super::super::dnd_outgoing::send_multiple_selection_notify(
                xwm,
                event.requestor,
                event.time,
                None,
            )?;
        } else {
            super::super::dnd_outgoing::complete_single_conversion(
                xwm,
                event.requestor,
                event.time,
                event.target,
                property,
                false,
            )?;
        }
        return Ok(true);
    }
    let targets_atom = xwm.atoms.get(XwmAtomName::Targets);
    let timestamp_atom = xwm.atoms.get(XwmAtomName::Timestamp);
    if event.target == targets_atom {
        let mime_atoms = request
            .mime_targets
            .iter()
            .map(|(_, atom)| *atom)
            .collect::<Vec<_>>();
        let targets = super::dnd_wire::selection_targets(
            targets_atom,
            timestamp_atom,
            multiple_atom,
            &mime_atoms,
        );
        let success = {
            let written = xwm.connection.change_property32(
                PropMode::REPLACE,
                event.requestor,
                property,
                AtomEnum::ATOM,
                &targets,
            );
            match written {
                Ok(cookie) => {
                    std::mem::forget(cookie);
                    true
                }
                Err(_) => false,
            }
        };
        super::super::dnd_outgoing::complete_single_conversion(
            xwm,
            event.requestor,
            event.time,
            event.target,
            property,
            success,
        )?;
        return Ok(true);
    }
    if event.target == timestamp_atom {
        let success = {
            let written = xwm.connection.change_property32(
                PropMode::REPLACE,
                event.requestor,
                property,
                AtomEnum::INTEGER,
                &[request.timestamp],
            );
            match written {
                Ok(cookie) => {
                    std::mem::forget(cookie);
                    true
                }
                Err(_) => false,
            }
        };
        super::super::dnd_outgoing::complete_single_conversion(
            xwm,
            event.requestor,
            event.time,
            event.target,
            property,
            success,
        )?;
        return Ok(true);
    }
    if is_multiple {
        if xwm.data_bridge.dnd.pending_replies.len() >= MAX_PENDING_DND_REPLIES {
            super::super::dnd_outgoing::send_multiple_selection_notify(
                xwm,
                event.requestor,
                event.time,
                None,
            )?;
            return Ok(true);
        }
        let cookie = xwm
            .connection
            .get_property(
                false,
                event.requestor,
                property,
                xwm.atoms.get(XwmAtomName::AtomPair),
                0,
                (MAX_MULTIPLE_PAIRS * 2) as u32,
            )
            .map_err(XwmError::Connection)?;
        let sequence = cookie.sequence_number();
        std::mem::forget(cookie);
        let deadline_ns = now_ns.saturating_add(SOURCE_OWNERSHIP_TIMEOUT_NS);
        xwm.data_bridge.dnd.pending_replies.insert(
            sequence,
            DndPendingReply::MultipleRead {
                id: request.id,
                source_proxy: request.source_proxy,
                target: request.target,
                requestor: event.requestor,
                property,
                request_time: event.time,
                deadline_ns,
            },
        );
        return Ok(true);
    }
    if let Some((mime_type, mime_atom)) = request
        .mime_targets
        .iter()
        .find(|(_, atom)| *atom == event.target)
    {
        if super::super::dnd_outgoing::property_in_use(xwm, event.requestor, property) {
            super::super::dnd_outgoing::complete_single_conversion(
                xwm,
                event.requestor,
                event.time,
                event.target,
                property,
                false,
            )?;
            return Ok(true);
        }
        let notification = super::super::dnd_outgoing::ConversionNotification::Single {
            requestor: event.requestor,
            time: event.time,
            target_atom: *mime_atom,
            property,
        };
        let source = crate::xwayland::XwaylandDndSourceProxyId {
            adapter_id: request.id,
            xid: request.source_proxy,
        };
        let started = super::super::dnd_outgoing::start_transfer(
            xwm,
            super::super::dnd_outgoing::DndTransferRequest {
                source,
                target: request.target,
                requestor: event.requestor,
                property,
                mime_type: mime_type.clone(),
                property_type: *mime_atom,
                notification,
            },
            now_ns,
        )?;
        if started.is_none() {
            super::super::dnd_outgoing::complete_single_conversion(
                xwm,
                event.requestor,
                event.time,
                event.target,
                property,
                false,
            )?;
        }
        return Ok(true);
    }
    // DELETE and every unimplemented special target fail closed.
    super::super::dnd_outgoing::complete_single_conversion(
        xwm,
        event.requestor,
        event.time,
        event.target,
        property,
        false,
    )?;
    Ok(true)
}

pub(super) fn handle_multiple_reply(
    xwm: &mut Xwm,
    context: MultipleRequestContext,
    reply: Option<xproto::GetPropertyReply>,
    now_ns: u64,
) -> Result<(), XwmError> {
    let still_current = context.deadline_ns > now_ns
        && xwm.data_bridge.dnd.active_session().is_some_and(|session| {
            session.id == context.id
                && session.source_proxy == Some(context.source_proxy)
                && session.ownership_confirmed
                && session.target == Some(context.target)
                && (context.requestor == context.target.xid()
                    || Some(context.requestor) == session.wire_recipient)
        });
    let pairs = reply
        .as_ref()
        .and_then(|reply| parse_multiple_pairs(reply, xwm.atoms.get(XwmAtomName::AtomPair)));
    if !still_current || pairs.is_none() {
        super::super::dnd_outgoing::send_multiple_selection_notify(
            xwm,
            context.requestor,
            context.request_time,
            None,
        )?;
        return Ok(());
    }
    process_multiple_pairs(
        xwm,
        context,
        pairs.expect("validated MULTIPLE property"),
        now_ns,
    )
}

fn valid_selection_request(
    xwm: &Xwm,
    event: &xproto::SelectionRequestEvent,
) -> Option<ValidSelectionRequest> {
    let session = xwm.data_bridge.dnd.active_session()?;
    let source_proxy = session.source_proxy?;
    let target = session.target?;
    if session.id.generation() != xwm.generation
        || event.owner != source_proxy
        || !session.ownership_confirmed
        || session.progress != DndWireProgress::Positioned
        || target.generation() != xwm.generation
        || (event.requestor != target.xid() && Some(event.requestor) != session.wire_recipient)
    {
        return None;
    }
    let timestamp = session.ownership_timestamp?;
    let mime_targets = session
        .mime_types
        .as_slice()
        .iter()
        .zip(session.mime_atoms.iter())
        .filter_map(|(mime, atom)| atom.map(|atom| (mime.clone(), atom)))
        .collect();
    Some(ValidSelectionRequest {
        id: session.id,
        source_proxy,
        target,
        timestamp,
        mime_targets,
    })
}

fn selection_time_is_current_or_after(request_time: u32, ownership_time: u32) -> bool {
    request_time == 0 || (request_time.wrapping_sub(ownership_time) as i32) >= 0
}

pub(super) fn parse_multiple_pairs(
    reply: &xproto::GetPropertyReply,
    atom_pair: u32,
) -> Option<Vec<(u32, u32)>> {
    if reply.type_ != atom_pair || reply.format != 32 || reply.bytes_after != 0 {
        return None;
    }
    let values = reply.value32()?.collect::<Vec<_>>();
    if values.len() % 2 != 0 || values.len() / 2 > MAX_MULTIPLE_PAIRS {
        return None;
    }
    Some(
        values
            .chunks_exact(2)
            .map(|pair| (pair[0], pair[1]))
            .collect(),
    )
}

pub(super) fn process_multiple_pairs(
    xwm: &mut Xwm,
    context: MultipleRequestContext,
    pairs: Vec<(u32, u32)>,
    now_ns: u64,
) -> Result<(), XwmError> {
    let multiple_target = xwm.atoms.get(XwmAtomName::Multiple);
    let group_id = super::super::dnd_outgoing::create_multiple_group(
        xwm,
        context.requestor,
        context.request_time,
        context.property,
        pairs.clone(),
        now_ns,
    );
    let Some(group_id) = group_id else {
        super::super::dnd_outgoing::send_multiple_selection_notify(
            xwm,
            context.requestor,
            context.request_time,
            None,
        )?;
        return Ok(());
    };
    let mut used_properties = HashSet::new();
    let targets_atom = xwm.atoms.get(XwmAtomName::Targets);
    let timestamp_atom = xwm.atoms.get(XwmAtomName::Timestamp);
    let multiple_pair_atom = xwm.atoms.get(XwmAtomName::AtomPair);
    let (timestamp, mime_targets) = xwm
        .data_bridge
        .dnd
        .active_session()
        .filter(|session| {
            session.id == context.id && session.source_proxy == Some(context.source_proxy)
        })
        .map(|session| {
            (
                session.ownership_timestamp.unwrap_or_default(),
                session
                    .mime_types
                    .as_slice()
                    .iter()
                    .zip(session.mime_atoms.iter())
                    .filter_map(|(mime, atom)| atom.map(|atom| (mime.clone(), atom)))
                    .collect::<Vec<_>>(),
            )
        })
        .unwrap_or_default();
    for (ordinal, (target_atom, property)) in pairs.iter().copied().enumerate() {
        if property == x11rb::NONE
            || property == context.property
            || !used_properties.insert(property)
            || super::super::dnd_outgoing::property_in_use(xwm, context.requestor, property)
        {
            super::super::dnd_outgoing::set_multiple_pair_result(
                xwm, group_id, ordinal, false, now_ns,
            )?;
            continue;
        }
        if target_atom == targets_atom {
            let mime_atoms = mime_targets
                .iter()
                .map(|(_, atom)| *atom)
                .collect::<Vec<_>>();
            let targets = super::dnd_wire::selection_targets(
                targets_atom,
                timestamp_atom,
                multiple_target,
                &mime_atoms,
            );
            let success = match xwm.connection.change_property32(
                PropMode::REPLACE,
                context.requestor,
                property,
                AtomEnum::ATOM,
                &targets,
            ) {
                Ok(cookie) => {
                    std::mem::forget(cookie);
                    true
                }
                Err(_) => false,
            };
            if !success {
                super::super::dnd_outgoing::set_multiple_pair_result(
                    xwm, group_id, ordinal, false, now_ns,
                )?;
            }
            continue;
        }
        if target_atom == timestamp_atom {
            let success = match xwm.connection.change_property32(
                PropMode::REPLACE,
                context.requestor,
                property,
                AtomEnum::INTEGER,
                &[timestamp],
            ) {
                Ok(cookie) => {
                    std::mem::forget(cookie);
                    true
                }
                Err(_) => false,
            };
            if !success {
                super::super::dnd_outgoing::set_multiple_pair_result(
                    xwm, group_id, ordinal, false, now_ns,
                )?;
            }
            continue;
        }
        if target_atom == multiple_target || target_atom == multiple_pair_atom {
            super::super::dnd_outgoing::set_multiple_pair_result(
                xwm, group_id, ordinal, false, now_ns,
            )?;
            continue;
        }
        let Some((mime_type, mime_atom)) = mime_targets
            .iter()
            .find(|(_, mime_atom)| *mime_atom == target_atom)
        else {
            super::super::dnd_outgoing::set_multiple_pair_result(
                xwm, group_id, ordinal, false, now_ns,
            )?;
            continue;
        };
        let Some(notification) =
            super::super::dnd_outgoing::add_multiple_conversion(xwm, group_id, ordinal)
        else {
            continue;
        };
        let source = crate::xwayland::XwaylandDndSourceProxyId {
            adapter_id: context.id,
            xid: context.source_proxy,
        };
        let started = super::super::dnd_outgoing::start_transfer(
            xwm,
            super::super::dnd_outgoing::DndTransferRequest {
                source,
                target: context.target,
                requestor: context.requestor,
                property,
                mime_type: mime_type.clone(),
                property_type: *mime_atom,
                notification,
            },
            now_ns,
        )?;
        if started.is_none() {
            super::super::dnd_outgoing::set_multiple_pair_result(
                xwm, group_id, ordinal, false, now_ns,
            )?;
        }
    }
    super::super::dnd_outgoing::finish_multiple_setup(xwm, group_id)
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        num::NonZeroU64,
        os::{fd::AsRawFd, unix::net::UnixStream},
    };

    use super::*;
    use crate::xwayland::{
        CanonicalDndSessionId, XwaylandDndAction, XwaylandDndMimeCatalog, XwaylandDndSourceProxyId,
        XwaylandDndVersion,
    };

    fn read_requests(peer: &mut UnixStream) -> Vec<u8> {
        peer.set_nonblocking(true).unwrap();
        let mut requests = Vec::new();
        let mut buffer = [0u8; 1024];
        loop {
            match peer.read(&mut buffer) {
                Ok(0) => break,
                Ok(bytes_read) => requests.extend_from_slice(&buffer[..bytes_read]),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) => panic!("read X11 requests: {error}"),
            }
        }
        requests
    }

    fn u32_at(bytes: &[u8], offset: usize) -> u32 {
        u32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap())
    }

    #[derive(Debug, PartialEq, Eq)]
    struct WirePropertyChange {
        window: u32,
        property: u32,
        property_type: u32,
        format: u8,
        value: Vec<u8>,
    }

    #[derive(Debug, PartialEq, Eq)]
    struct WireSelectionNotify {
        time: u32,
        requestor: u32,
        selection: u32,
        target: u32,
        property: u32,
    }

    #[derive(Debug, Default)]
    struct WireRequests {
        opcodes: Vec<u8>,
        property_changes: Vec<WirePropertyChange>,
        selection_notifies: Vec<WireSelectionNotify>,
    }

    fn read_wire_requests(peer: &mut UnixStream) -> WireRequests {
        let bytes = read_requests(peer);
        let mut requests = WireRequests::default();
        let mut offset = 0;
        while offset < bytes.len() {
            let words = u16::from_ne_bytes([bytes[offset + 2], bytes[offset + 3]]) as usize;
            assert!(words > 0, "unexpected BigRequests encoding");
            let request_len = words * 4;
            assert!(offset + request_len <= bytes.len(), "truncated X11 request");
            let request = &bytes[offset..offset + request_len];
            requests.opcodes.push(request[0]);
            if request[0] == xproto::CHANGE_PROPERTY_REQUEST {
                let format = request[16];
                let units = u32_at(request, 20) as usize;
                let value_len = units * usize::from(format / 8);
                requests.property_changes.push(WirePropertyChange {
                    window: u32_at(request, 4),
                    property: u32_at(request, 8),
                    property_type: u32_at(request, 12),
                    format,
                    value: request[24..24 + value_len].to_vec(),
                });
            } else if request[0] == xproto::SEND_EVENT_REQUEST
                && request[12] & 0x7f == xproto::SELECTION_NOTIFY_EVENT
            {
                let event = &request[12..44];
                requests.selection_notifies.push(WireSelectionNotify {
                    time: u32_at(event, 4),
                    requestor: u32_at(event, 8),
                    selection: u32_at(event, 12),
                    target: u32_at(event, 16),
                    property: u32_at(event, 20),
                });
            }
            offset += request_len;
        }
        requests
    }

    fn values32(bytes: &[u8]) -> Vec<u32> {
        bytes
            .chunks_exact(4)
            .map(|chunk| u32_at(chunk, 0))
            .collect()
    }

    fn active_fixture(
        mime_types: &[&str],
    ) -> (
        super::super::super::Xwm,
        UnixStream,
        crate::xwayland::XwaylandDndAdapterId,
        u32,
        X11WindowHandle,
    ) {
        let generation = super::super::super::XwaylandGeneration::new(NonZeroU64::new(83).unwrap());
        let (mut xwm, peer) = super::super::super::test_fixture_for_tests(generation);
        let id = crate::xwayland::XwaylandDndAdapterId::new(
            CanonicalDndSessionId::Wayland(NonZeroU64::new(9103).unwrap()),
            generation,
        )
        .unwrap();
        let target = X11WindowHandle::new(generation, 0x773);
        let source_proxy = 0x883;
        let catalog = XwaylandDndMimeCatalog::try_new(
            mime_types.iter().map(|mime| (*mime).to_owned()).collect(),
        )
        .unwrap();
        assert!(xwm.data_bridge.dnd.install_wayland_session(
            id,
            catalog,
            vec![XwaylandDndAction::Copy],
        ));
        assert!(xwm.data_bridge.dnd.bind_source_proxy(id, source_proxy));
        assert!(
            xwm.data_bridge
                .dnd
                .confirm_source_ownership(id, source_proxy, 1234)
        );
        let mime_atoms = (0..mime_types.len())
            .map(|index| Some(99 + index as u32))
            .collect();
        xwm.data_bridge.dnd.active.as_mut().unwrap().mime_atoms = mime_atoms;
        assert!(xwm.data_bridge.dnd.set_discovered_target(
            id,
            target,
            target.xid(),
            XwaylandDndVersion::new(5).unwrap(),
        ));
        assert!(xwm.data_bridge.dnd.mark_entered(id));
        assert!(
            xwm.data_bridge
                .dnd
                .position(id, target, 10, 20, Some(XwaylandDndAction::Copy))
        );
        xwm.data_bridge
            .dnd_outgoing
            .initialize_generation(generation);
        (xwm, peer, id, source_proxy, target)
    }

    fn selection_request_event(
        xwm: &super::super::super::Xwm,
        source_proxy: u32,
        target_window: X11WindowHandle,
        target: u32,
        property: u32,
        time: u32,
    ) -> xproto::SelectionRequestEvent {
        xproto::SelectionRequestEvent {
            response_type: xproto::SELECTION_REQUEST_EVENT,
            sequence: 0,
            time,
            owner: source_proxy,
            requestor: target_window.xid(),
            selection: xwm.atoms.get(XwmAtomName::XdndSelection),
            target,
            property,
        }
    }

    fn pending_multiple_sequence(xwm: &super::super::super::Xwm) -> u16 {
        let Some(sequence) = xwm.data_bridge.dnd.pending_replies.keys().next() else {
            panic!("MULTIPLE property GetProperty reply is pending");
        };
        *sequence as u16
    }

    fn test_now_ns() -> u64 {
        crate::native::event_loop::monotonic_now_ns().unwrap_or_default()
    }

    fn multiple_property_reply(sequence: u16, atom_pair: u32, values: &[u32]) -> Vec<u8> {
        let mut reply = vec![0; 32 + values.len() * 4];
        reply[0] = 1;
        reply[1] = 32;
        reply[2..4].copy_from_slice(&sequence.to_ne_bytes());
        reply[4..8].copy_from_slice(&(values.len() as u32).to_ne_bytes());
        reply[8..12].copy_from_slice(&atom_pair.to_ne_bytes());
        reply[16..20].copy_from_slice(&(values.len() as u32).to_ne_bytes());
        for (index, value) in values.iter().enumerate() {
            reply[32 + index * 4..36 + index * 4].copy_from_slice(&value.to_ne_bytes());
        }
        reply
    }

    fn complete_multiple_read(
        xwm: &mut super::super::super::Xwm,
        peer: &mut UnixStream,
        pairs: &[u32],
    ) {
        let sequence = pending_multiple_sequence(xwm);
        let atom_pair = xwm.atoms.get(XwmAtomName::AtomPair);
        peer.write_all(&multiple_property_reply(sequence, atom_pair, pairs))
            .expect("send MULTIPLE property reply");
        xwm.drain_events(32)
            .expect("process MULTIPLE property reply");
        xwm.flush().expect("flush MULTIPLE completion");
    }

    #[test]
    fn offered_mime_request_queues_one_exact_move_only_source_read() {
        let generation = super::super::super::XwaylandGeneration::new(NonZeroU64::new(81).unwrap());
        let (mut xwm, _peer) = super::super::super::test_fixture_for_tests(generation);
        let id = crate::xwayland::XwaylandDndAdapterId::new(
            CanonicalDndSessionId::Wayland(NonZeroU64::new(9101).unwrap()),
            generation,
        )
        .unwrap();
        let target = X11WindowHandle::new(generation, 0x770);
        let source_proxy = 0x880;
        let catalog = XwaylandDndMimeCatalog::try_new(vec!["text/plain".to_owned()]).unwrap();
        assert!(xwm.data_bridge.dnd.install_wayland_session(
            id,
            catalog,
            vec![XwaylandDndAction::Copy],
        ));
        assert!(xwm.data_bridge.dnd.bind_source_proxy(id, source_proxy));
        assert!(
            xwm.data_bridge
                .dnd
                .confirm_source_ownership(id, source_proxy, 1234)
        );
        xwm.data_bridge.dnd.active.as_mut().unwrap().mime_atoms[0] = Some(99);
        assert!(xwm.data_bridge.dnd.set_discovered_target(
            id,
            target,
            target.xid(),
            XwaylandDndVersion::new(5).unwrap(),
        ));
        assert!(xwm.data_bridge.dnd.mark_entered(id));
        assert!(xwm.data_bridge.dnd.position(
            id,
            target,
            10,
            20,
            Some(crate::xwayland::XwaylandDndAction::Copy)
        ));
        xwm.data_bridge
            .dnd_outgoing
            .initialize_generation(generation);

        let selection_atom = xwm.atoms.get(XwmAtomName::XdndSelection);
        let handled = selection_request(
            &mut xwm,
            xproto::SelectionRequestEvent {
                response_type: xproto::SELECTION_REQUEST_EVENT,
                sequence: 0,
                time: 1235,
                owner: source_proxy,
                requestor: target.xid(),
                selection: selection_atom,
                target: 99,
                property: 100,
            },
            10,
        )
        .unwrap();
        assert!(handled);
        let mut requests = super::super::super::dnd_outgoing::take_requests(&mut xwm);
        assert_eq!(requests.len(), 1);
        let request = requests.pop().unwrap();
        assert_eq!(request.transfer_id.source.adapter_id, id);
        assert_eq!(request.transfer_id.source.xid, source_proxy);
        assert_eq!(request.target, target);
        assert_eq!(request.mime_type, "text/plain");
        assert!(super::super::super::dnd_outgoing::take_requests(&mut xwm).is_empty());
        drop(request);
        super::super::super::dnd_outgoing::cancel_source(
            &mut xwm,
            XwaylandDndSourceProxyId {
                adapter_id: id,
                xid: source_proxy,
            },
            11,
        )
        .unwrap();
    }

    #[test]
    fn targets_and_timestamp_requests_return_the_exact_current_catalog_and_owner_time() {
        let generation = super::super::super::XwaylandGeneration::new(NonZeroU64::new(82).unwrap());
        let (mut xwm, mut peer) = super::super::super::test_fixture_for_tests(generation);
        let id = crate::xwayland::XwaylandDndAdapterId::new(
            CanonicalDndSessionId::Wayland(NonZeroU64::new(9102).unwrap()),
            generation,
        )
        .unwrap();
        let target = X11WindowHandle::new(generation, 0x771);
        let source_proxy = 0x881;
        let catalog =
            XwaylandDndMimeCatalog::try_new(vec!["text/plain".to_owned(), "image/png".to_owned()])
                .unwrap();
        assert!(xwm.data_bridge.dnd.install_wayland_session(
            id,
            catalog,
            vec![XwaylandDndAction::Copy],
        ));
        assert!(xwm.data_bridge.dnd.bind_source_proxy(id, source_proxy));
        assert!(
            xwm.data_bridge
                .dnd
                .confirm_source_ownership(id, source_proxy, 1234)
        );
        xwm.data_bridge.dnd.active.as_mut().unwrap().mime_atoms = vec![Some(99), Some(100)];
        assert!(xwm.data_bridge.dnd.set_discovered_target(
            id,
            target,
            target.xid(),
            XwaylandDndVersion::new(5).unwrap(),
        ));
        assert!(xwm.data_bridge.dnd.mark_entered(id));
        assert!(xwm.data_bridge.dnd.position(
            id,
            target,
            10,
            20,
            Some(crate::xwayland::XwaylandDndAction::Copy)
        ));

        let targets_atom = xwm.atoms.get(XwmAtomName::Targets);
        let timestamp_atom = xwm.atoms.get(XwmAtomName::Timestamp);
        let selection_atom = xwm.atoms.get(XwmAtomName::XdndSelection);
        for (target_atom, property) in [(targets_atom, 200), (timestamp_atom, 201)] {
            assert!(
                selection_request(
                    &mut xwm,
                    xproto::SelectionRequestEvent {
                        response_type: xproto::SELECTION_REQUEST_EVENT,
                        sequence: 0,
                        time: 1235,
                        owner: source_proxy,
                        requestor: target.xid(),
                        selection: selection_atom,
                        target: target_atom,
                        property,
                    },
                    10,
                )
                .unwrap()
            );
        }
        xwm.flush().unwrap();
        let requests = read_requests(&mut peer);
        assert_eq!(requests[0], 18);
        let targets_property = xwm.atoms.get(XwmAtomName::Targets);
        assert_eq!(u32_at(&requests, 8), 200);
        assert_eq!(u32_at(&requests, 12), u32::from(AtomEnum::ATOM));
        assert_eq!(requests[16], 32);
        assert_eq!(u32_at(&requests, 20), 5);
        let returned_targets = (0..5)
            .map(|index| u32_at(&requests, 24 + index * 4))
            .collect::<Vec<_>>();
        assert_eq!(
            returned_targets,
            [
                targets_property,
                timestamp_atom,
                xwm.atoms.get(XwmAtomName::Multiple),
                99,
                100
            ]
        );

        let targets_request_len = u16::from_ne_bytes([requests[2], requests[3]]) as usize * 4;
        let timestamp_request = targets_request_len + 44;
        assert_eq!(requests[timestamp_request], 18);
        assert_eq!(u32_at(&requests, timestamp_request + 8), 201);
        assert_eq!(
            u32_at(&requests, timestamp_request + 12),
            u32::from(AtomEnum::INTEGER)
        );
        assert_eq!(requests[timestamp_request + 16], 32);
        assert_eq!(u32_at(&requests, timestamp_request + 20), 1);
        assert_eq!(u32_at(&requests, timestamp_request + 24), 1234);
    }

    #[test]
    fn multiple_without_property_is_refused_before_get_property_or_conversion() {
        let (mut xwm, mut peer, _, source_proxy, target_window) = active_fixture(&[]);
        let multiple = xwm.atoms.get(XwmAtomName::Multiple);
        let selection = xwm.atoms.get(XwmAtomName::XdndSelection);
        let event = selection_request_event(
            &xwm,
            source_proxy,
            target_window,
            multiple,
            x11rb::NONE,
            1235,
        );
        assert!(selection_request(&mut xwm, event, 10).unwrap());
        xwm.flush().expect("flush failed MULTIPLE notification");

        let wire = read_wire_requests(&mut peer);
        assert_eq!(wire.selection_notifies.len(), 1);
        assert_eq!(
            wire.selection_notifies[0],
            WireSelectionNotify {
                time: 1235,
                requestor: target_window.xid(),
                selection,
                target: multiple,
                property: x11rb::NONE,
            }
        );
        assert!(!wire.opcodes.contains(&xproto::GET_PROPERTY_REQUEST));
        assert!(wire.property_changes.is_empty());
        assert!(xwm.data_bridge.dnd.pending_replies.is_empty());
        assert!(super::super::super::dnd_outgoing::take_requests(&mut xwm).is_empty());
        assert_eq!(
            super::super::super::dnd_outgoing::multiple_group_count_for_test(&xwm),
            0
        );
        assert_eq!(
            super::super::super::dnd_outgoing::transfer_count_for_test(&xwm),
            0
        );
    }

    #[test]
    fn multiple_success_notifies_with_original_target_and_parent_property() {
        let (mut xwm, mut peer, _, source_proxy, target_window) = active_fixture(&[]);
        let multiple = xwm.atoms.get(XwmAtomName::Multiple);
        let selection = xwm.atoms.get(XwmAtomName::XdndSelection);
        let targets = xwm.atoms.get(XwmAtomName::Targets);
        let timestamp = xwm.atoms.get(XwmAtomName::Timestamp);
        let parent_property = 0xa00;
        let property_a = 0xa01;
        let property_b = 0xa02;
        let event = selection_request_event(
            &xwm,
            source_proxy,
            target_window,
            multiple,
            parent_property,
            1235,
        );
        selection_request(&mut xwm, event, test_now_ns()).unwrap();
        xwm.flush().expect("flush MULTIPLE property read");
        let initial = read_wire_requests(&mut peer);
        assert!(initial.opcodes.contains(&xproto::GET_PROPERTY_REQUEST));
        assert!(initial.selection_notifies.is_empty());

        complete_multiple_read(
            &mut xwm,
            &mut peer,
            &[targets, property_a, timestamp, property_b],
        );
        let wire = read_wire_requests(&mut peer);
        assert_eq!(wire.property_changes.len(), 3);
        assert_eq!(wire.property_changes[0].property, property_a);
        assert_eq!(wire.property_changes[1].property, property_b);
        assert_eq!(
            wire.property_changes[0].property_type,
            u32::from(AtomEnum::ATOM)
        );
        assert_eq!(
            values32(&wire.property_changes[0].value),
            [targets, timestamp, multiple]
        );
        assert_eq!(
            wire.property_changes[1].property_type,
            u32::from(AtomEnum::INTEGER)
        );
        assert_eq!(values32(&wire.property_changes[1].value), [1234]);
        let parent_pairs = wire
            .property_changes
            .iter()
            .find(|change| change.property == parent_property)
            .expect("rewritten parent atom-pair property");
        assert_eq!(
            parent_pairs.property_type,
            xwm.atoms.get(XwmAtomName::AtomPair)
        );
        assert_eq!(parent_pairs.format, 32);
        assert_eq!(
            values32(&parent_pairs.value),
            [targets, property_a, timestamp, property_b]
        );
        assert_eq!(wire.selection_notifies.len(), 1);
        assert_eq!(
            wire.selection_notifies[0],
            WireSelectionNotify {
                time: 1235,
                requestor: target_window.xid(),
                selection,
                target: multiple,
                property: parent_property,
            }
        );
    }

    #[test]
    fn multiple_parent_property_write_failure_notifies_with_property_none() {
        let (mut xwm, mut peer, _, source_proxy, target_window) = active_fixture(&[]);
        let multiple = xwm.atoms.get(XwmAtomName::Multiple);
        let selection = xwm.atoms.get(XwmAtomName::XdndSelection);
        let targets = xwm.atoms.get(XwmAtomName::Targets);
        let timestamp = xwm.atoms.get(XwmAtomName::Timestamp);
        let parent_property = 0xa08;
        let property_a = 0xa09;
        let property_b = 0xa0a;
        let event = selection_request_event(
            &xwm,
            source_proxy,
            target_window,
            multiple,
            parent_property,
            1235,
        );
        selection_request(&mut xwm, event, test_now_ns()).unwrap();
        xwm.flush().expect("flush MULTIPLE property read");
        let _ = read_wire_requests(&mut peer);
        super::super::super::dnd_outgoing::fail_next_multiple_parent_property_write_for_test(
            &mut xwm,
        );

        complete_multiple_read(
            &mut xwm,
            &mut peer,
            &[targets, property_a, timestamp, property_b],
        );
        let wire = read_wire_requests(&mut peer);
        assert_eq!(wire.property_changes.len(), 2);
        assert_eq!(wire.property_changes[0].property, property_a);
        assert_eq!(wire.property_changes[1].property, property_b);
        assert_eq!(wire.selection_notifies.len(), 1);
        assert_eq!(
            wire.selection_notifies[0],
            WireSelectionNotify {
                time: 1235,
                requestor: target_window.xid(),
                selection,
                target: multiple,
                property: x11rb::NONE,
            }
        );
    }

    #[test]
    fn multiple_child_failure_only_clears_the_failed_pair_property() {
        let (mut xwm, mut peer, _, source_proxy, target_window) = active_fixture(&[]);
        let multiple = xwm.atoms.get(XwmAtomName::Multiple);
        let selection = xwm.atoms.get(XwmAtomName::XdndSelection);
        let targets = xwm.atoms.get(XwmAtomName::Targets);
        let unsupported = 0xdead;
        let timestamp = xwm.atoms.get(XwmAtomName::Timestamp);
        let parent_property = 0xa10;
        let property_a = 0xa11;
        let failed_property = 0xa12;
        let property_b = 0xa13;
        let event = selection_request_event(
            &xwm,
            source_proxy,
            target_window,
            multiple,
            parent_property,
            1235,
        );
        selection_request(&mut xwm, event, test_now_ns()).unwrap();
        xwm.flush().expect("flush MULTIPLE property read");
        let _ = read_wire_requests(&mut peer);

        complete_multiple_read(
            &mut xwm,
            &mut peer,
            &[
                targets,
                property_a,
                unsupported,
                failed_property,
                timestamp,
                property_b,
            ],
        );
        let wire = read_wire_requests(&mut peer);
        let parent_pairs = wire
            .property_changes
            .iter()
            .find(|change| change.property == parent_property)
            .expect("rewritten parent atom-pair property");
        assert_eq!(
            values32(&parent_pairs.value),
            [
                targets,
                property_a,
                unsupported,
                x11rb::NONE,
                timestamp,
                property_b,
            ]
        );
        assert_eq!(wire.selection_notifies.len(), 1);
        assert_eq!(wire.selection_notifies[0].selection, selection);
        assert_eq!(wire.selection_notifies[0].target, multiple);
        assert_eq!(wire.selection_notifies[0].property, parent_property);
        assert_eq!(wire.selection_notifies[0].time, 1235);
        assert_eq!(wire.selection_notifies[0].requestor, target_window.xid());
    }

    #[test]
    fn multiple_mime_parent_notification_waits_for_child_conversion() {
        let (mut xwm, mut peer, _, source_proxy, target_window) = active_fixture(&["text/plain"]);
        let multiple = xwm.atoms.get(XwmAtomName::Multiple);
        let selection = xwm.atoms.get(XwmAtomName::XdndSelection);
        let mime_atom = 99;
        let parent_property = 0xa20;
        let child_property = 0xa21;
        let event = selection_request_event(
            &xwm,
            source_proxy,
            target_window,
            multiple,
            parent_property,
            1235,
        );
        selection_request(&mut xwm, event, test_now_ns()).unwrap();
        xwm.flush().expect("flush MULTIPLE property read");
        let _ = read_wire_requests(&mut peer);
        complete_multiple_read(&mut xwm, &mut peer, &[mime_atom, child_property]);
        let waiting = read_wire_requests(&mut peer);
        assert!(waiting.selection_notifies.is_empty());
        assert!(
            !waiting
                .property_changes
                .iter()
                .any(|change| change.property == parent_property)
        );
        let mut source_requests = super::super::super::dnd_outgoing::take_requests(&mut xwm);
        assert_eq!(source_requests.len(), 1);
        let source_request = source_requests.pop().unwrap();
        let transfer_id = source_request.transfer_id;
        let data = b"asynchronous MIME data";
        assert_eq!(
            unsafe {
                libc::write(
                    source_request.sink.as_raw_fd(),
                    data.as_ptr().cast(),
                    data.len(),
                )
            },
            data.len() as isize
        );
        drop(source_request);
        assert!(
            super::super::super::dnd_outgoing::resolve_requests(
                &mut xwm,
                [(transfer_id, true)],
                32,
            )
            .unwrap()
        );
        xwm.flush().expect("flush asynchronous MULTIPLE completion");
        let completed = read_wire_requests(&mut peer);
        let child = completed
            .property_changes
            .iter()
            .find(|change| change.property == child_property)
            .expect("MIME child property");
        assert_eq!(child.property_type, mime_atom);
        assert_eq!(child.format, 8);
        assert_eq!(child.value, data);
        let parent = completed
            .property_changes
            .iter()
            .find(|change| change.property == parent_property)
            .expect("MULTIPLE parent property");
        assert_eq!(values32(&parent.value), [mime_atom, child_property]);
        assert_eq!(completed.selection_notifies.len(), 1);
        assert_eq!(completed.selection_notifies[0].selection, selection);
        assert_eq!(completed.selection_notifies[0].target, multiple);
        assert_eq!(completed.selection_notifies[0].property, parent_property);
        assert_eq!(completed.selection_notifies[0].time, 1235);
        assert_eq!(
            completed.selection_notifies[0].requestor,
            target_window.xid()
        );
    }

    #[test]
    fn multiple_incr_parent_completion_is_independent_of_later_chunks() {
        let (mut xwm, mut peer, _, source_proxy, target_window) = active_fixture(&["text/plain"]);
        let multiple = xwm.atoms.get(XwmAtomName::Multiple);
        let selection = xwm.atoms.get(XwmAtomName::XdndSelection);
        let incr_atom = xwm.atoms.get(XwmAtomName::Incr);
        let mime_atom = 99;
        let parent_property = 0xa30;
        let child_property = 0xa31;
        let event = selection_request_event(
            &xwm,
            source_proxy,
            target_window,
            multiple,
            parent_property,
            1235,
        );
        selection_request(&mut xwm, event, test_now_ns()).unwrap();
        xwm.flush().expect("flush MULTIPLE property read");
        let _ = read_wire_requests(&mut peer);
        complete_multiple_read(&mut xwm, &mut peer, &[mime_atom, child_property]);
        let _ = read_wire_requests(&mut peer);
        let mut source_requests = super::super::super::dnd_outgoing::take_requests(&mut xwm);
        let source_request = source_requests.pop().expect("MIME source request");
        assert!(source_requests.is_empty());
        let transfer_id = source_request.transfer_id;
        assert!(
            super::super::super::dnd_outgoing::resolve_requests(
                &mut xwm,
                [(transfer_id, true)],
                42,
            )
            .unwrap()
        );

        xwm.flush().expect("flush waiting INCR source");
        let waiting = read_wire_requests(&mut peer);
        assert!(waiting.selection_notifies.is_empty());
        assert!(
            !waiting
                .property_changes
                .iter()
                .any(|change| change.property == parent_property)
        );

        let source_requests = super::super::super::dnd_outgoing::take_requests(&mut xwm);
        assert!(source_requests.is_empty());
        // A single blocking pipe write larger than its capacity can block; fill
        // the source pipe from a writer thread while the transfer drains it.
        let data = vec![0x5a; super::super::super::dnd_outgoing::MAX_DND_CHUNK_BYTES + 1];
        let sink = source_request.sink;
        let writer = std::thread::spawn(move || {
            let fd = sink.as_raw_fd();
            let mut written = 0;
            while written < data.len() {
                let result = unsafe {
                    libc::write(fd, data[written..].as_ptr().cast(), data.len() - written)
                };
                assert!(result > 0, "write INCR source payload");
                written += result as usize;
            }
            drop(sink);
        });
        let interests = super::super::super::dnd_outgoing::source_interests(&xwm);
        let [(interest_id, read_fd)] = interests.as_slice() else {
            panic!("one direct source transfer must await readiness");
        };
        assert_eq!(*interest_id, transfer_id);
        let mut available: libc::c_int = 0;
        let source_ready_deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let result = unsafe { libc::ioctl(*read_fd, libc::FIONREAD, &mut available) };
            assert_eq!(result, 0, "inspect INCR test pipe readiness");
            if available as usize >= super::super::super::dnd_outgoing::MAX_DND_CHUNK_BYTES {
                break;
            }
            assert!(
                std::time::Instant::now() < source_ready_deadline,
                "INCR source pipe did not fill to one chunk"
            );
            std::thread::yield_now();
        }
        super::super::super::dnd_outgoing::bind_reactor_token(
            &mut xwm,
            transfer_id,
            *read_fd,
            Some(1),
        );
        assert!(
            super::super::super::dnd_outgoing::handle_source_ready(
                &mut xwm,
                transfer_id,
                transfer_id.source.adapter_id.generation(),
                1,
                43,
            )
            .unwrap()
        );
        writer.join().expect("finish writing source payload");
        xwm.flush().expect("flush INCR marker and parent notify");
        let announced = read_wire_requests(&mut peer);
        let incr = announced
            .property_changes
            .iter()
            .find(|change| change.property == child_property)
            .expect("INCR announcement property");
        assert_eq!(incr.property_type, incr_atom);
        assert_eq!(incr.format, 32);
        assert_eq!(
            values32(&incr.value),
            [super::super::super::dnd_outgoing::MAX_DND_CHUNK_BYTES as u32]
        );
        let parent = announced
            .property_changes
            .iter()
            .find(|change| change.property == parent_property)
            .expect("MULTIPLE parent property");
        assert_eq!(values32(&parent.value), [mime_atom, child_property]);
        assert_eq!(announced.selection_notifies.len(), 1);
        assert_eq!(announced.selection_notifies[0].selection, selection);
        assert_eq!(announced.selection_notifies[0].target, multiple);
        assert_eq!(announced.selection_notifies[0].property, parent_property);

        assert!(
            super::super::super::dnd_outgoing::property_deleted(
                &mut xwm,
                target_window.xid(),
                child_property,
                44,
            )
            .unwrap()
        );
        assert!(
            super::super::super::dnd_outgoing::property_deleted(
                &mut xwm,
                target_window.xid(),
                child_property,
                45,
            )
            .unwrap()
        );
        let interests = super::super::super::dnd_outgoing::source_interests(&xwm);
        let [(interest_id, read_fd)] = interests.as_slice() else {
            panic!("INCR source must await its final byte");
        };
        assert_eq!(*interest_id, transfer_id);
        super::super::super::dnd_outgoing::bind_reactor_token(
            &mut xwm,
            transfer_id,
            *read_fd,
            Some(2),
        );
        assert!(
            super::super::super::dnd_outgoing::handle_source_ready(
                &mut xwm,
                transfer_id,
                transfer_id.source.adapter_id.generation(),
                2,
                46,
            )
            .unwrap()
        );
        assert!(
            super::super::super::dnd_outgoing::property_deleted(
                &mut xwm,
                target_window.xid(),
                child_property,
                47,
            )
            .unwrap()
        );
        xwm.flush().expect("flush later INCR chunks");
        let chunks = read_wire_requests(&mut peer);
        assert!(chunks.selection_notifies.is_empty());
        assert_eq!(
            chunks
                .property_changes
                .iter()
                .filter(|change| change.property == child_property)
                .count(),
            3
        );
    }
}
