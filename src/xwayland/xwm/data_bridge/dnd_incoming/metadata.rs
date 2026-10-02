use super::*;
use std::io;

pub(super) fn begin_enter(xwm: &mut Xwm, data: [u32; 5], now_ns: u64) -> Result<(), XwmError> {
    let source = data[0];
    let source_version = data[1] >> 24;
    let Some(version) = crate::xwayland::negotiate_incoming_root_version(source_version) else {
        return Ok(());
    };
    let Some(proxy) = target_proxy(xwm) else {
        return Ok(());
    };
    if source == 0 {
        return Ok(());
    }
    if let Some(previous) = xwm.data_bridge.dnd.incoming_session().map(|s| s.offer_id) {
        if !leave_offer(xwm, previous) {
            let _ = xwm.data_bridge.dnd.retire_incoming_session(previous);
        }
    } else if xwm.data_bridge.dnd.outgoing_session().is_some() {
        // One adapter owner per generation/seat. Do not evict an active C2
        // source-side session to accept a second protocol direction.
        return Ok(());
    }
    cancel_metadata_replies(xwm, None);
    let Some(offer_id) = xwm
        .data_bridge
        .dnd_incoming
        .allocate_offer_id(xwm.generation)
    else {
        return Ok(());
    };
    let more_types = data[1] & 1 != 0;
    let inline_mime_atoms = if more_types {
        Vec::new()
    } else {
        data[2..5]
            .iter()
            .copied()
            .filter(|atom| *atom != 0)
            .collect()
    };
    let session = IncomingDndSession {
        generation: xwm.generation,
        offer_id,
        source: X11WindowHandle::new(xwm.generation, source),
        logical_target_root: xwm.root,
        target_proxy: proxy,
        version,
        inline_mime_atoms: inline_mime_atoms.clone(),
        mime_atoms: inline_mime_atoms.clone(),
        more_types,
        type_list_complete: !more_types,
        pending_atom_names: 0,
        metadata_complete: false,
        mime_types: Vec::new(),
        atom_to_mime: BTreeMap::new(),
        source_actions: Vec::new(),
        available_actions: Vec::new(),
        action_list_required: false,
        action_list_queried: false,
        action_list_complete: true,
        latest_position: None,
        next_position_serial: 0,
        canonical_started: false,
        pending_status_deadline_ns: None,
        status_pending: false,
        accepted_mime: None,
        selected_action: None,
        metadata_deadline_ns: now_ns.saturating_add(TARGET_METADATA_TIMEOUT_NS),
    };
    if !xwm.data_bridge.dnd.install_incoming_session(session) {
        return Ok(());
    }
    if more_types {
        issue_property_read(
            xwm,
            offer_id,
            XwmAtomName::XdndTypeList,
            PendingPropertyKind::TypeList,
        )?;
    } else {
        issue_atom_names(xwm, offer_id, &inline_mime_atoms, now_ns)?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum PendingPropertyKind {
    TypeList,
    ActionList,
}

fn issue_property_read(
    xwm: &mut Xwm,
    offer_id: XwaylandDndOfferId,
    property: XwmAtomName,
    kind: PendingPropertyKind,
) -> Result<(), XwmError> {
    if xwm.data_bridge.dnd_incoming.pending.len() >= MAX_PENDING_INCOMING_DND_REPLIES {
        return Ok(());
    }
    let Some(session) = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .filter(|session| session.offer_id == offer_id)
    else {
        return Ok(());
    };
    let cookie = xwm
        .connection
        .get_property(
            false,
            session.source.xid(),
            xwm.atoms.get(property),
            AtomEnum::ANY,
            0,
            match kind {
                PendingPropertyKind::TypeList => 65,
                PendingPropertyKind::ActionList => 6,
            },
        )
        .map_err(XwmError::Connection)?;
    let sequence = cookie.sequence_number();
    std::mem::forget(cookie);
    let deadline_ns = session.metadata_deadline_ns;
    let pending = match kind {
        PendingPropertyKind::TypeList => PendingMetadataReply::TypeList {
            offer_id,
            deadline_ns,
        },
        PendingPropertyKind::ActionList => PendingMetadataReply::ActionList {
            offer_id,
            deadline_ns,
        },
    };
    xwm.data_bridge
        .dnd_incoming
        .pending
        .insert(sequence, pending);
    Ok(())
}

fn issue_atom_names(
    xwm: &mut Xwm,
    offer_id: XwaylandDndOfferId,
    atoms: &[Atom],
    now_ns: u64,
) -> Result<(), XwmError> {
    let mut distinct = atoms
        .iter()
        .copied()
        .filter(|atom| *atom != 0)
        .collect::<Vec<_>>();
    distinct.sort_unstable();
    distinct.dedup();
    if distinct.len() > crate::xwayland::MAX_XWAYLAND_DND_MIME_TYPES {
        retire_incoming(xwm, offer_id);
        return Ok(());
    }
    let Some(session) = xwm
        .data_bridge
        .dnd
        .incoming_session_mut()
        .filter(|session| session.offer_id == offer_id)
    else {
        return Ok(());
    };
    session.mime_atoms = distinct.clone();
    session.pending_atom_names = distinct.len();
    let no_atoms = distinct.is_empty();
    let deadline_ns = session
        .metadata_deadline_ns
        .min(now_ns.saturating_add(TARGET_METADATA_TIMEOUT_NS));
    for atom in distinct.iter().copied() {
        if xwm.data_bridge.dnd_incoming.pending.len() >= MAX_PENDING_INCOMING_DND_REPLIES {
            retire_incoming(xwm, offer_id);
            return Ok(());
        }
        let cookie = xwm
            .connection
            .get_atom_name(atom)
            .map_err(XwmError::Connection)?;
        let sequence = cookie.sequence_number();
        std::mem::forget(cookie);
        xwm.data_bridge.dnd_incoming.pending.insert(
            sequence,
            PendingMetadataReply::AtomName {
                offer_id,
                atom,
                deadline_ns,
            },
        );
    }
    if no_atoms
        && let Some(session) = xwm
            .data_bridge
            .dnd
            .incoming_session_mut()
            .filter(|session| session.offer_id == offer_id)
    {
        session.metadata_complete = session.type_list_complete;
    }
    Ok(())
}

pub(super) fn position(xwm: &mut Xwm, data: [u32; 5], now_ns: u64) -> Result<(), XwmError> {
    let Some(session) = xwm.data_bridge.dnd.incoming_session() else {
        return Ok(());
    };
    let proxy = target_proxy(xwm).unwrap_or_default();
    if !is_exact_source(session, xwm.generation, data[0], xwm.root, proxy) {
        return Ok(());
    }
    let offer_id = session.offer_id;
    let (x, y) = crate::xwayland::unpack_root_coordinates(data[1]);
    let Some(position_id) = allocate_position_id(xwm, offer_id) else {
        retire_incoming(xwm, offer_id);
        return Ok(());
    };
    let Some(requested_action) = action_from_atom(xwm, data[3]) else {
        send_rejected_wire_position(xwm, offer_id)?;
        return Ok(());
    };
    if let Some(session) = xwm
        .data_bridge
        .dnd
        .incoming_session_mut()
        .filter(|session| session.offer_id == offer_id)
    {
        session.latest_position = Some(IncomingPosition {
            position_id,
            root_x: x,
            root_y: y,
            timestamp: data[2],
            requested_action,
        });
        session.pending_status_deadline_ns = Some(now_ns.saturating_add(TARGET_STATUS_TIMEOUT_NS));
        session.status_pending = true;
        session.action_list_required = requested_action == crate::xwayland::XwaylandDndAction::Ask;
        if session.action_list_required {
            if !session.action_list_queried {
                session.action_list_queried = true;
                session.action_list_complete = false;
            }
        } else {
            session.action_list_complete = true;
        }
    }
    if requested_action == crate::xwayland::XwaylandDndAction::Ask
        && xwm
            .data_bridge
            .dnd
            .incoming_session()
            .is_some_and(|session| session.offer_id == offer_id && !session.action_list_complete)
        && !xwm.data_bridge.dnd_incoming.pending.values().any(|pending| {
            matches!(pending, PendingMetadataReply::ActionList { offer_id: current, .. } if *current == offer_id)
        })
    {
        issue_property_read(xwm, offer_id, XwmAtomName::XdndActionList, PendingPropertyKind::ActionList)?;
    }
    settle_metadata_and_position(xwm, offer_id, now_ns)
}

fn allocate_position_id(
    xwm: &mut Xwm,
    offer_id: XwaylandDndOfferId,
) -> Option<crate::xwayland::XwaylandDndIncomingPositionId> {
    let session = xwm
        .data_bridge
        .dnd
        .incoming_session_mut()
        .filter(|session| session.offer_id == offer_id && session.generation == xwm.generation)?;
    session.next_position_serial = session.next_position_serial.checked_add(1)?;
    std::num::NonZeroU64::new(session.next_position_serial)
        .map(|serial| crate::xwayland::XwaylandDndIncomingPositionId::new(offer_id, serial))
}

fn send_rejected_wire_position(
    xwm: &mut Xwm,
    offer_id: XwaylandDndOfferId,
) -> Result<(), XwmError> {
    let Some((source, logical_target_root)) = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .filter(|session| {
            session.offer_id == offer_id
                && session.generation == xwm.generation
                && session.logical_target_root == xwm.root
                && Some(session.target_proxy) == target_proxy(xwm)
        })
        .map(|session| (session.source.xid(), session.logical_target_root))
    else {
        return Ok(());
    };
    let event = xproto::ClientMessageEvent::new(
        32,
        source,
        xwm.atoms.get(XwmAtomName::XdndStatus),
        [
            logical_target_root,
            XDND_STATUS_WANT_POSITION_UPDATES,
            0,
            0,
            0,
        ],
    );
    let cookie = xwm
        .connection
        .send_event(false, source, xproto::EventMask::NO_EVENT, event)
        .map_err(XwmError::Connection)?;
    std::mem::forget(cookie);
    if let Some(session) = xwm
        .data_bridge
        .dnd
        .incoming_session_mut()
        .filter(|session| session.offer_id == offer_id)
    {
        session.status_pending = false;
        session.pending_status_deadline_ns = None;
        session.accepted_mime = None;
        session.selected_action = None;
    }
    Ok(())
}

fn action_from_atom(xwm: &Xwm, atom: Atom) -> Option<crate::xwayland::XwaylandDndAction> {
    if atom == xwm.atoms.get(XwmAtomName::XdndActionCopy) {
        Some(crate::xwayland::XwaylandDndAction::Copy)
    } else if atom == xwm.atoms.get(XwmAtomName::XdndActionMove) {
        Some(crate::xwayland::XwaylandDndAction::Move)
    } else if atom == xwm.atoms.get(XwmAtomName::XdndActionLink) {
        Some(crate::xwayland::XwaylandDndAction::Link)
    } else if atom == xwm.atoms.get(XwmAtomName::XdndActionAsk) {
        Some(crate::xwayland::XwaylandDndAction::Ask)
    } else if atom == xwm.atoms.get(XwmAtomName::XdndActionPrivate) {
        Some(crate::xwayland::XwaylandDndAction::Private)
    } else {
        None
    }
}

pub(super) fn representable_source_actions(
    requested: crate::xwayland::XwaylandDndAction,
    available: &[crate::xwayland::XwaylandDndAction],
) -> Vec<crate::xwayland::XwaylandDndAction> {
    use crate::xwayland::XwaylandDndAction as Action;
    match requested {
        Action::Copy => vec![Action::Copy],
        Action::Move => vec![Action::Move, Action::Copy],
        Action::Link | Action::Private => vec![Action::Copy],
        Action::Ask
            if available.contains(&Action::Ask)
                && (available.contains(&Action::Copy) || available.contains(&Action::Move)) =>
        {
            available
                .iter()
                .copied()
                .filter(|action| matches!(action, Action::Copy | Action::Move | Action::Ask))
                .collect()
        }
        Action::Ask => Vec::new(),
    }
}

fn settle_metadata_and_position(
    xwm: &mut Xwm,
    offer_id: XwaylandDndOfferId,
    now_ns: u64,
) -> Result<(), XwmError> {
    let Some(snapshot) = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .filter(|session| session.offer_id == offer_id)
        .map(|session| {
            (
                session.type_list_complete,
                session.pending_atom_names,
                session.action_list_required,
                session.action_list_complete,
                session.mime_types.clone(),
                session.atom_to_mime.clone(),
                session.available_actions.clone(),
                session.latest_position,
                session.canonical_started,
                session.source,
                session.version,
            )
        })
    else {
        return Ok(());
    };
    let (
        types_ready,
        names_pending,
        needs_actions,
        actions_ready,
        mime_types,
        atom_to_mime,
        available_actions,
        position,
        canonical_started,
        source,
        version,
    ) = snapshot;
    if !types_ready || names_pending != 0 || (needs_actions && !actions_ready) {
        return Ok(());
    }
    if let Some(session) = xwm
        .data_bridge
        .dnd
        .incoming_session_mut()
        .filter(|session| session.offer_id == offer_id)
    {
        session.metadata_complete = true;
    }
    let Some(position) = position else {
        return Ok(());
    };
    if mime_types.is_empty() {
        send_status(xwm, offer_id, position.position_id, false, None)?;
        return Ok(());
    }
    let actions = representable_source_actions(position.requested_action, &available_actions);
    if actions.is_empty() {
        send_status(xwm, offer_id, position.position_id, false, None)?;
        return Ok(());
    }
    let catalog = match crate::xwayland::XwaylandDndMimeCatalog::try_new(mime_types.clone()) {
        Ok(catalog) => catalog,
        Err(_) => {
            retire_incoming(xwm, offer_id);
            return Ok(());
        }
    };
    if canonical_started {
        if let Some(session) = xwm
            .data_bridge
            .dnd
            .incoming_session_mut()
            .filter(|session| session.offer_id == offer_id)
        {
            session.source_actions = actions.clone();
        }
        if !xwm.data_bridge.dnd_incoming.push_event(
            crate::xwayland::XwaylandDndIncomingEvent::Position {
                offer_id,
                position_id: position.position_id,
                x: position.root_x,
                y: position.root_y,
                requested_action: position.requested_action,
                source_actions: actions,
                x_timestamp: position.timestamp,
            },
        ) {
            retire_incoming(xwm, offer_id);
        }
        return Ok(());
    }
    let offer =
        crate::xwayland::XwaylandDndOffer::new(offer_id, source, version, catalog, actions.clone());
    let Ok(offer) = offer else {
        retire_incoming(xwm, offer_id);
        return Ok(());
    };
    if !xwm
        .data_bridge
        .dnd_incoming
        .push_event(crate::xwayland::XwaylandDndIncomingEvent::Begin {
            offer,
            position_id: position.position_id,
            x: position.root_x,
            y: position.root_y,
            requested_action: position.requested_action,
            x_timestamp: position.timestamp,
        })
    {
        retire_incoming(xwm, offer_id);
        return Ok(());
    }
    if let Some(session) = xwm
        .data_bridge
        .dnd
        .incoming_session_mut()
        .filter(|session| session.offer_id == offer_id)
    {
        session.atom_to_mime = atom_to_mime;
        session.source_actions = actions;
        session.canonical_started = true;
    }
    let _ = now_ns;
    Ok(())
}

pub(super) fn leave(xwm: &mut Xwm, source: Window) -> Result<(), XwmError> {
    let offer_id = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .filter(|session| session.source.xid() == source)
        .map(|session| session.offer_id);
    if let Some(offer_id) = offer_id {
        leave_offer(xwm, offer_id);
    }
    Ok(())
}

pub(super) fn leave_offer(xwm: &mut Xwm, offer_id: XwaylandDndOfferId) -> bool {
    if xwm
        .data_bridge
        .dnd
        .incoming_session()
        .is_none_or(|session| session.offer_id != offer_id)
    {
        return false;
    }
    let canonical_started = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .is_some_and(|session| session.offer_id == offer_id && session.canonical_started);
    if canonical_started
        && !xwm.data_bridge.dnd_incoming.events.iter().any(
            |event| matches!(event, XwaylandDndIncomingEvent::Leave { offer_id: queued } if *queued == offer_id),
        )
        && !xwm.data_bridge.dnd_incoming.push_event(XwaylandDndIncomingEvent::Leave { offer_id })
    {
        return false;
    }
    retire_incoming_state(xwm, offer_id)
}

fn retire_incoming(xwm: &mut Xwm, offer_id: XwaylandDndOfferId) -> bool {
    let pending_position = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .filter(|session| session.offer_id == offer_id && session.status_pending)
        .and_then(|session| session.latest_position.map(|position| position.position_id));
    if let Some(position_id) = pending_position {
        let _ = send_status(xwm, offer_id, position_id, false, None);
    }
    let canonical_started = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .is_some_and(|session| session.offer_id == offer_id && session.canonical_started);
    if canonical_started
        && !xwm.data_bridge.dnd_incoming.events.iter().any(
            |event| matches!(event, XwaylandDndIncomingEvent::Leave { offer_id: queued } if *queued == offer_id),
        )
        && !xwm.data_bridge.dnd_incoming.push_event(XwaylandDndIncomingEvent::Leave { offer_id })
    {
        // The queue reserves Leave capacity for every Begin. Keep the exact
        // XWM session live if that invariant is ever violated so the runtime
        // cannot silently retain a canonical drag without an adapter owner.
        return false;
    }
    retire_incoming_state(xwm, offer_id)
}

fn retire_incoming_state(xwm: &mut Xwm, offer_id: XwaylandDndOfferId) -> bool {
    cancel_metadata_replies(xwm, Some(offer_id));
    retire_offer_transfers(xwm, offer_id);
    let _ = xwm.data_bridge.dnd.retire_incoming_session(offer_id);
    true
}

pub(super) fn retire_offer_transfers(xwm: &mut Xwm, offer_id: XwaylandDndOfferId) {
    let ids = xwm
        .data_bridge
        .dnd_incoming
        .transfers
        .values()
        .filter_map(|transfer| (transfer.offer_id == offer_id).then_some(transfer.id))
        .collect::<Vec<_>>();
    for id in ids {
        super::transfer::finish_transfer(xwm, id);
    }
}

pub(super) fn cancel_metadata_replies(xwm: &mut Xwm, offer_id: Option<XwaylandDndOfferId>) {
    let sequences = xwm
        .data_bridge
        .dnd_incoming
        .pending
        .iter()
        .filter_map(|(sequence, reply)| {
            let reply_offer = match reply {
                PendingMetadataReply::TypeList { offer_id, .. }
                | PendingMetadataReply::ActionList { offer_id, .. }
                | PendingMetadataReply::AtomName { offer_id, .. } => *offer_id,
            };
            (offer_id.is_none_or(|offer_id| offer_id == reply_offer)).then_some(*sequence)
        })
        .collect::<Vec<_>>();
    for sequence in sequences {
        xwm.data_bridge.dnd_incoming.pending.remove(&sequence);
        xwm.connection.discard_reply(
            sequence,
            x11rb::connection::RequestKind::HasResponse,
            x11rb::connection::DiscardMode::DiscardReply,
        );
    }
}

fn send_status(
    xwm: &mut Xwm,
    offer_id: XwaylandDndOfferId,
    position_id: crate::xwayland::XwaylandDndIncomingPositionId,
    accepted: bool,
    action: Option<crate::xwayland::XwaylandDndAction>,
) -> Result<(), XwmError> {
    let Some(session) = xwm.data_bridge.dnd.incoming_session().filter(|session| {
        session.offer_id == offer_id
            && session.generation == xwm.generation
            && position_id.offer_id() == offer_id
            && session.status_pending
            && session.logical_target_root == xwm.root
            && Some(session.target_proxy) == target_proxy(xwm)
    }) else {
        return Ok(());
    };
    let Some(position) = session
        .latest_position
        .filter(|position| position.position_id == position_id)
    else {
        return Ok(());
    };
    let valid_action = action.filter(|action| {
        use crate::xwayland::XwaylandDndAction as Action;
        match (position.requested_action, *action) {
            (Action::Copy, Action::Copy) => true,
            (Action::Move, Action::Move) => session.source_actions.contains(action),
            // A canonical Wayland target may choose Copy as XDND's permitted
            // fallback for a source that requested Move.
            (Action::Move, Action::Copy) => true,
            (Action::Ask, Action::Ask | Action::Copy) => session.source_actions.contains(action),
            (Action::Link | Action::Private, Action::Copy) => {
                session.source_actions.contains(&Action::Copy)
            }
            _ => false,
        }
    });
    let accepted = accepted && session.accepted_mime.is_some() && valid_action.is_some();
    let selected_atom = if accepted {
        match valid_action {
            Some(crate::xwayland::XwaylandDndAction::Copy) => {
                xwm.atoms.get(XwmAtomName::XdndActionCopy)
            }
            Some(crate::xwayland::XwaylandDndAction::Move) => {
                xwm.atoms.get(XwmAtomName::XdndActionMove)
            }
            Some(crate::xwayland::XwaylandDndAction::Ask) => {
                xwm.atoms.get(XwmAtomName::XdndActionAsk)
            }
            _ => 0,
        }
    } else {
        0
    };
    let event = xproto::ClientMessageEvent::new(
        32,
        session.source.xid(),
        xwm.atoms.get(XwmAtomName::XdndStatus),
        [
            session.logical_target_root,
            u32::from(accepted) | XDND_STATUS_WANT_POSITION_UPDATES,
            0,
            0,
            selected_atom,
        ],
    );
    let cookie = xwm
        .connection
        .send_event(
            false,
            session.source.xid(),
            xproto::EventMask::NO_EVENT,
            event,
        )
        .map_err(XwmError::Connection)?;
    std::mem::forget(cookie);
    if let Some(session) = xwm
        .data_bridge
        .dnd
        .incoming_session_mut()
        .filter(|session| {
            session.offer_id == offer_id
                && session
                    .latest_position
                    .is_some_and(|position| position.position_id == position_id)
        })
    {
        session.status_pending = false;
        session.pending_status_deadline_ns = None;
        session.selected_action = accepted.then_some(valid_action).flatten();
    }
    Ok(())
}

pub(crate) fn poll_replies(xwm: &mut Xwm, budget: usize, now_ns: u64) -> Result<usize, XwmError> {
    let sequences = xwm
        .data_bridge
        .dnd_incoming
        .pending
        .keys()
        .copied()
        .take(budget)
        .collect::<Vec<_>>();
    let mut processed = 0;
    for sequence in sequences {
        let Some(pending) = xwm.data_bridge.dnd_incoming.pending.get(&sequence).copied() else {
            continue;
        };
        let offer_id = match pending {
            PendingMetadataReply::TypeList { offer_id, .. }
            | PendingMetadataReply::ActionList { offer_id, .. }
            | PendingMetadataReply::AtomName { offer_id, .. } => offer_id,
        };
        let Some(session) = xwm
            .data_bridge
            .dnd
            .incoming_session()
            .filter(|session| session.offer_id == offer_id)
        else {
            cancel_metadata_replies(xwm, Some(offer_id));
            continue;
        };
        if pending_deadline(pending) <= now_ns || session.generation != xwm.generation {
            retire_incoming(xwm, offer_id);
            continue;
        }
        match pending {
            PendingMetadataReply::TypeList { .. } => {
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
                xwm.data_bridge.dnd_incoming.pending.remove(&sequence);
                processed += 1;
                let Some(reply) = reply.filter(|reply| {
                    reply.type_ == u32::from(AtomEnum::ATOM)
                        && reply.format == 32
                        && reply.bytes_after == 0
                }) else {
                    retire_incoming(xwm, offer_id);
                    continue;
                };
                let atoms = reply
                    .value32()
                    .map(|values| values.collect::<Vec<_>>())
                    .unwrap_or_default();
                if atoms.len() > crate::xwayland::MAX_XWAYLAND_DND_MIME_TYPES {
                    retire_incoming(xwm, offer_id);
                    continue;
                }
                if let Some(session) = xwm
                    .data_bridge
                    .dnd
                    .incoming_session_mut()
                    .filter(|session| session.offer_id == offer_id)
                {
                    session.type_list_complete = true;
                }
                issue_atom_names(xwm, offer_id, &atoms, now_ns)?;
            }
            PendingMetadataReply::ActionList { .. } => {
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
                xwm.data_bridge.dnd_incoming.pending.remove(&sequence);
                processed += 1;
                let Some(reply) = reply.filter(|reply| {
                    reply.type_ == u32::from(AtomEnum::ATOM)
                        && reply.format == 32
                        && reply.bytes_after == 0
                }) else {
                    retire_incoming(xwm, offer_id);
                    continue;
                };
                let atoms = reply
                    .value32()
                    .map(|values| values.collect::<Vec<_>>())
                    .unwrap_or_default();
                if atoms.len() > crate::xwayland::MAX_XWAYLAND_DND_ACTIONS {
                    retire_incoming(xwm, offer_id);
                    continue;
                }
                let mut available = Vec::new();
                for atom in atoms {
                    if let Some(action) = action_from_atom(xwm, atom)
                        && matches!(
                            action,
                            crate::xwayland::XwaylandDndAction::Copy
                                | crate::xwayland::XwaylandDndAction::Move
                                | crate::xwayland::XwaylandDndAction::Ask
                        )
                        && !available.contains(&action)
                    {
                        available.push(action);
                    }
                }
                if let Some(session) = xwm
                    .data_bridge
                    .dnd
                    .incoming_session_mut()
                    .filter(|session| session.offer_id == offer_id)
                {
                    session.available_actions = available;
                    session.action_list_complete = true;
                }
                settle_metadata_and_position(xwm, offer_id, now_ns)?;
            }
            PendingMetadataReply::AtomName { atom, .. } => {
                let cookie = Cookie::<
                    super::super::super::connection::X11Connection,
                    xproto::GetAtomNameReply,
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
                xwm.data_bridge.dnd_incoming.pending.remove(&sequence);
                processed += 1;
                let Some(reply) = reply else {
                    retire_incoming(xwm, offer_id);
                    continue;
                };
                if reply.name.is_empty()
                    || reply.name.len() > crate::xwayland::MAX_XWAYLAND_DND_MIME_TYPE_BYTES
                    || reply.name.contains(&0)
                {
                    retire_incoming(xwm, offer_id);
                    continue;
                }
                let Ok(name) = String::from_utf8(reply.name) else {
                    retire_incoming(xwm, offer_id);
                    continue;
                };
                let Some(session) = xwm
                    .data_bridge
                    .dnd
                    .incoming_session_mut()
                    .filter(|session| session.offer_id == offer_id)
                else {
                    continue;
                };
                session.atom_to_mime.insert(atom, name.clone());
                if !session.mime_types.contains(&name) {
                    session.mime_types.push(name);
                }
                session.pending_atom_names = session.pending_atom_names.saturating_sub(1);
                session.metadata_complete =
                    session.type_list_complete && session.pending_atom_names == 0;
                settle_metadata_and_position(xwm, offer_id, now_ns)?;
            }
        }
    }
    let transfer_processed =
        super::transfer::poll_transfer_replies(xwm, budget.saturating_sub(processed), now_ns)?;
    Ok(processed.saturating_add(transfer_processed))
}

fn pending_deadline(reply: PendingMetadataReply) -> u64 {
    match reply {
        PendingMetadataReply::TypeList { deadline_ns, .. }
        | PendingMetadataReply::ActionList { deadline_ns, .. }
        | PendingMetadataReply::AtomName { deadline_ns, .. } => deadline_ns,
    }
}

pub(crate) fn expire_deadlines(xwm: &mut Xwm, now_ns: u64) -> Result<(), XwmError> {
    let expired_offer = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .filter(|session| !session.metadata_complete && now_ns >= session.metadata_deadline_ns)
        .map(|session| session.offer_id);
    if let Some(offer_id) = expired_offer {
        retire_incoming(xwm, offer_id);
    }
    let status_timeout = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .filter(|session| {
            session.status_pending
                && session
                    .pending_status_deadline_ns
                    .is_some_and(|deadline| now_ns >= deadline)
        })
        .and_then(|session| {
            session
                .latest_position
                .map(|position| (session.offer_id, position.position_id))
        });
    if let Some((offer_id, position_id)) = status_timeout {
        send_status(xwm, offer_id, position_id, false, None)?;
    }
    let expired = xwm
        .data_bridge
        .dnd_incoming
        .pending
        .iter()
        .filter_map(|(sequence, reply)| (pending_deadline(*reply) <= now_ns).then_some(*sequence))
        .collect::<Vec<_>>();
    for sequence in expired {
        xwm.data_bridge.dnd_incoming.pending.remove(&sequence);
        xwm.connection.discard_reply(
            sequence,
            x11rb::connection::RequestKind::HasResponse,
            x11rb::connection::DiscardMode::DiscardReply,
        );
    }
    let expired_transfers = xwm
        .data_bridge
        .dnd_incoming
        .transfers
        .values()
        .filter_map(|transfer| (now_ns >= transfer.idle_deadline_ns).then_some(transfer.id))
        .collect::<Vec<_>>();
    for id in expired_transfers {
        super::transfer::finish_transfer(xwm, id);
    }
    Ok(())
}

pub(crate) fn next_deadline_ns(xwm: &Xwm) -> Option<u64> {
    let pending = xwm
        .data_bridge
        .dnd_incoming
        .pending
        .values()
        .map(|reply| pending_deadline(*reply))
        .min();
    let transfer = xwm
        .data_bridge
        .dnd_incoming
        .transfers
        .values()
        .map(|transfer| transfer.idle_deadline_ns)
        .min();
    let session = xwm.data_bridge.dnd.incoming_session().and_then(|session| {
        [
            (!session.metadata_complete).then_some(session.metadata_deadline_ns),
            session.pending_status_deadline_ns,
        ]
        .into_iter()
        .flatten()
        .min()
    });
    session.into_iter().chain(pending).chain(transfer).min()
}

pub(crate) fn source_feedback(
    xwm: &mut Xwm,
    offer_id: XwaylandDndOfferId,
    position_id: crate::xwayland::XwaylandDndIncomingPositionId,
    accepted_mime: Option<String>,
    action: Option<crate::xwayland::XwaylandDndAction>,
) -> Result<(), XwmError> {
    let (feedback_is_complete, explicit_rejection, selected_action) = {
        let Some(session) = xwm
            .data_bridge
            .dnd
            .incoming_session_mut()
            .filter(|session| {
                session.offer_id == offer_id
                    && session.generation == xwm.generation
                    && session.canonical_started
                    && position_id.offer_id() == offer_id
                    && session
                        .latest_position
                        .is_some_and(|position| position.position_id == position_id)
            })
        else {
            return Ok(());
        };
        if !session.status_pending {
            return Ok(());
        }
        session.accepted_mime = accepted_mime;
        session.selected_action = action;
        (
            session.accepted_mime.is_some() && session.selected_action.is_some(),
            session.accepted_mime.is_none() && session.selected_action.is_none(),
            session.selected_action,
        )
    };
    if feedback_is_complete || explicit_rejection {
        send_status(
            xwm,
            offer_id,
            position_id,
            feedback_is_complete,
            selected_action,
        )?;
    }
    Ok(())
}

pub(crate) fn apply_source_feedback_transition(
    xwm: &mut Xwm,
    transition: crate::xwayland::XwaylandDndTransition,
) -> Result<(), XwmError> {
    let crate::xwayland::XwaylandDndTransition::SourceFeedback {
        offer_id,
        position_id,
        accepted_mime,
        action,
    } = transition
    else {
        return Ok(());
    };
    source_feedback(xwm, offer_id, position_id, accepted_mime, action)
}
