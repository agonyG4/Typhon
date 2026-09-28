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
    let property = if event.property == x11rb::NONE {
        event.target
    } else {
        event.property
    };
    let Some(request) = valid_selection_request(xwm, &event) else {
        super::super::dnd_outgoing::complete_single_conversion(
            xwm,
            event.requestor,
            event.time,
            event.target,
            property,
            false,
        )?;
        return Ok(true);
    };
    if !selection_time_is_current_or_after(event.time, request.timestamp) || property == x11rb::NONE
    {
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
    let targets_atom = xwm.atoms.get(XwmAtomName::Targets);
    let timestamp_atom = xwm.atoms.get(XwmAtomName::Timestamp);
    let multiple_atom = xwm.atoms.get(XwmAtomName::Multiple);
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
    if event.target == multiple_atom {
        if property == x11rb::NONE
            || xwm.data_bridge.dnd.pending_replies.len() >= MAX_PENDING_DND_REPLIES
        {
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
        super::super::dnd_outgoing::complete_single_conversion(
            xwm,
            context.requestor,
            context.request_time,
            xwm.atoms.get(XwmAtomName::Multiple),
            context.property,
            false,
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
        multiple_target,
        context.property,
        pairs.clone(),
        now_ns,
    );
    let Some(group_id) = group_id else {
        super::super::dnd_outgoing::complete_single_conversion(
            xwm,
            context.requestor,
            context.request_time,
            multiple_target,
            context.property,
            false,
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
    use std::{io::Read, num::NonZeroU64, os::unix::net::UnixStream};

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
}
