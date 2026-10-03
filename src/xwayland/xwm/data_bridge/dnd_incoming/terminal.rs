use super::super::dnd_wire;
use super::*;

#[derive(Debug, Clone, Copy)]
struct DropSnapshot {
    offer_id: XwaylandDndOfferId,
    generation: XwaylandGeneration,
    source: Window,
    logical_target_root: Window,
    target_proxy: Window,
    version: crate::xwayland::XwaylandDndVersion,
    phase: IncomingDndWirePhase,
}

pub(crate) fn drop_received(xwm: &mut Xwm, data: [u32; 5], now_ns: u64) -> Result<(), XwmError> {
    let Some(drop) = dnd_wire::decode_xdnd_drop(data) else {
        return Ok(());
    };
    if !root_proxy_is_owned(xwm) {
        return Ok(());
    }
    let Some(snapshot) = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .filter(|session| {
            is_exact_source(
                session,
                xwm.generation,
                drop.source,
                xwm.root,
                target_proxy(xwm).unwrap_or_default(),
            )
        })
        .map(|session| DropSnapshot {
            offer_id: session.offer_id,
            generation: session.generation,
            source: session.source.xid(),
            logical_target_root: session.logical_target_root,
            target_proxy: session.target_proxy,
            version: session.version,
            phase: session.wire_phase,
        })
    else {
        return Ok(());
    };
    if snapshot.phase != IncomingDndWirePhase::Hover {
        return Ok(());
    }
    if snapshot.version.get() < 5 {
        reject_pending_position(xwm, snapshot.offer_id)?;
        let _ = metadata::leave_offer(xwm, snapshot.offer_id);
        return Ok(());
    }
    if drop.timestamp == 0 {
        reject_pending_position(xwm, snapshot.offer_id)?;
        send_finished(xwm, snapshot, false, None, true)?;
        if !metadata::leave_offer(xwm, snapshot.offer_id) {
            cleanup_terminal(xwm, snapshot.offer_id);
        }
        return Ok(());
    }

    let admission = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .filter(|session| session.offer_id == snapshot.offer_id)
        .map(|session| {
            let accepted_mime = session.accepted_mime.as_deref().is_some_and(|mime| {
                session
                    .atom_to_mime
                    .values()
                    .any(|candidate| candidate == mime)
            });
            let action = session.selected_action.filter(|action| {
                matches!(
                    action,
                    crate::xwayland::XwaylandDndAction::Copy
                        | crate::xwayland::XwaylandDndAction::Move
                        | crate::xwayland::XwaylandDndAction::Ask
                )
            });
            (
                session.generation == xwm.generation
                    && session.version.get() >= 5
                    && session.canonical_started
                    && session.metadata_complete
                    && !session.status_pending
                    && session.pending_status_deadline_ns.is_none()
                    && accepted_mime
                    && action.is_some(),
                action,
                session.status_pending,
            )
        });
    let Some((true, Some(action), false)) = admission else {
        reject_pending_position(xwm, snapshot.offer_id)?;
        send_finished(xwm, snapshot, false, None, true)?;
        let _ = metadata::leave_offer(xwm, snapshot.offer_id);
        return Ok(());
    };

    if let Some(session) = xwm
        .data_bridge
        .dnd
        .incoming_session_mut()
        .filter(|session| session.offer_id == snapshot.offer_id)
    {
        session.wire_phase = IncomingDndWirePhase::DropSubmitted {
            drop_timestamp: drop.timestamp,
            action,
            acknowledgement_deadline_ns: now_ns.saturating_add(INCOMING_DND_DROP_ACK_TIMEOUT_NS),
        };
    } else {
        return Ok(());
    }
    metadata::cancel_metadata_replies(xwm, Some(snapshot.offer_id));
    if !xwm
        .data_bridge
        .dnd_incoming
        .push_event(XwaylandDndIncomingEvent::Drop {
            offer_id: snapshot.offer_id,
        })
    {
        send_finished(xwm, snapshot, false, None, true)?;
        if let Some(session) = xwm
            .data_bridge
            .dnd
            .incoming_session_mut()
            .filter(|session| session.offer_id == snapshot.offer_id)
        {
            session.wire_phase = IncomingDndWirePhase::Hover;
        }
        if !metadata::leave_offer(xwm, snapshot.offer_id) {
            cleanup_terminal(xwm, snapshot.offer_id);
        }
    }
    Ok(())
}

fn reject_pending_position(xwm: &mut Xwm, offer_id: XwaylandDndOfferId) -> Result<(), XwmError> {
    let pending_position = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .filter(|session| session.offer_id == offer_id && session.status_pending)
        .and_then(|session| session.latest_position.map(|position| position.position_id));
    if let Some(position_id) = pending_position {
        metadata::send_status(xwm, offer_id, position_id, false, None)?;
    }
    Ok(())
}

/// Preserve a terminal cancellation even if the admitted Drop has not yet
/// reached runtime synchronization. Replacing that queued Drop is safe: the
/// runtime cancellation falls back to cancel when canonical Drop was not
/// applied and to finish(false) when it was.
fn enqueue_cancel_after_drop(xwm: &mut Xwm, offer_id: XwaylandDndOfferId) -> bool {
    let events = &mut xwm.data_bridge.dnd_incoming.events;
    let already_queued = events.iter().any(|event| {
        matches!(
            event,
            XwaylandDndIncomingEvent::CancelAfterDrop {
                offer_id: queued_offer
            } if *queued_offer == offer_id
        )
    });
    events.retain(|event| incoming_event_offer_id(event) != Some(offer_id));
    if already_queued {
        return true;
    }
    // Any remaining event belongs to a retired offer: there can be only one
    // live incoming session. Evict stale queue entries if necessary so an
    // exact terminal cancellation cannot be lost behind old hover traffic.
    while events.len() >= MAX_PENDING_XWAYLAND_DND_INCOMING_EVENTS {
        events.pop_front();
    }
    xwm.data_bridge
        .dnd_incoming
        .push_event(XwaylandDndIncomingEvent::CancelAfterDrop { offer_id })
}

fn incoming_event_offer_id(event: &XwaylandDndIncomingEvent) -> Option<XwaylandDndOfferId> {
    match event {
        XwaylandDndIncomingEvent::Begin { offer, .. } => Some(offer.id()),
        XwaylandDndIncomingEvent::Position { offer_id, .. }
        | XwaylandDndIncomingEvent::Leave { offer_id }
        | XwaylandDndIncomingEvent::Drop { offer_id }
        | XwaylandDndIncomingEvent::CancelAfterDrop { offer_id } => Some(*offer_id),
    }
}

pub(crate) fn resolve_drop(
    xwm: &mut Xwm,
    offer_id: XwaylandDndOfferId,
    accepted: bool,
    now_ns: u64,
) -> Result<(), XwmError> {
    let submitted = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .filter(|session| {
            session.offer_id == offer_id
                && session.generation == xwm.generation
                && matches!(
                    session.wire_phase,
                    IncomingDndWirePhase::DropSubmitted { .. }
                )
        })
        .map(|session| {
            (
                session.source.xid(),
                session.logical_target_root,
                session.target_proxy,
                session.version,
                session.wire_phase,
            )
        });
    let Some((source, logical_target_root, target_proxy, version, phase)) = submitted else {
        return Ok(());
    };
    let IncomingDndWirePhase::DropSubmitted {
        drop_timestamp,
        action,
        ..
    } = phase
    else {
        return Ok(());
    };
    if accepted {
        if let Some(session) = xwm
            .data_bridge
            .dnd
            .incoming_session_mut()
            .filter(|session| session.offer_id == offer_id)
        {
            session.wire_phase = IncomingDndWirePhase::AwaitingWaylandFinish {
                drop_timestamp,
                action,
                deadline_ns: now_ns.saturating_add(INCOMING_DND_TERMINAL_TIMEOUT_NS),
                cancel_submitted: false,
            };
        }
    } else {
        let snapshot = DropSnapshot {
            offer_id,
            generation: xwm.generation,
            source,
            logical_target_root,
            target_proxy,
            version,
            phase,
        };
        send_finished(xwm, snapshot, false, None, true)?;
        cleanup_terminal(xwm, offer_id);
    }
    Ok(())
}

pub(crate) fn resolve_cancel_after_drop(
    xwm: &mut Xwm,
    offer_id: XwaylandDndOfferId,
    cancelled: bool,
) -> Result<(), XwmError> {
    let Some((source, logical_target_root, target_proxy, version, phase)) = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .filter(|session| {
            session.offer_id == offer_id
                && session.generation == xwm.generation
                && matches!(
                    session.wire_phase,
                    IncomingDndWirePhase::AwaitingWaylandFinish { .. }
                )
        })
        .map(|session| {
            (
                session.source.xid(),
                session.logical_target_root,
                session.target_proxy,
                session.version,
                session.wire_phase,
            )
        })
    else {
        return Ok(());
    };
    if !cancelled {
        send_finished(
            xwm,
            DropSnapshot {
                offer_id,
                generation: xwm.generation,
                source,
                logical_target_root,
                target_proxy,
                version,
                phase,
            },
            false,
            None,
            true,
        )?;
        cleanup_terminal(xwm, offer_id);
    }
    Ok(())
}

pub(crate) fn source_finished(
    xwm: &mut Xwm,
    offer_id: XwaylandDndOfferId,
    accepted: bool,
    action: Option<crate::xwayland::XwaylandDndAction>,
    now_ns: u64,
) -> Result<(), XwmError> {
    let Some(snapshot) = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .filter(|session| {
            session.offer_id == offer_id
                && session.generation == xwm.generation
                && matches!(
                    session.wire_phase,
                    IncomingDndWirePhase::AwaitingWaylandFinish { .. }
                )
        })
        .map(|session| DropSnapshot {
            offer_id,
            generation: session.generation,
            source: session.source.xid(),
            logical_target_root: session.logical_target_root,
            target_proxy: session.target_proxy,
            version: session.version,
            phase: session.wire_phase,
        })
    else {
        return Ok(());
    };
    let IncomingDndWirePhase::AwaitingWaylandFinish {
        drop_timestamp,
        action: frozen_action,
        ..
    } = snapshot.phase
    else {
        return Ok(());
    };
    if !accepted {
        send_finished(xwm, snapshot, false, None, true)?;
        cleanup_terminal(xwm, offer_id);
        return Ok(());
    }
    let final_action = match (frozen_action, action) {
        (
            crate::xwayland::XwaylandDndAction::Copy,
            Some(crate::xwayland::XwaylandDndAction::Copy),
        ) => Some(crate::xwayland::XwaylandDndAction::Copy),
        (
            crate::xwayland::XwaylandDndAction::Move,
            Some(crate::xwayland::XwaylandDndAction::Move),
        ) => Some(crate::xwayland::XwaylandDndAction::Move),
        (
            crate::xwayland::XwaylandDndAction::Ask,
            Some(
                action @ (crate::xwayland::XwaylandDndAction::Copy
                | crate::xwayland::XwaylandDndAction::Move),
            ),
        ) => Some(action),
        _ => None,
    };
    match final_action {
        Some(crate::xwayland::XwaylandDndAction::Copy) => {
            send_finished(
                xwm,
                snapshot,
                true,
                Some(crate::xwayland::XwaylandDndAction::Copy),
                true,
            )?;
            cleanup_terminal(xwm, offer_id);
        }
        Some(crate::xwayland::XwaylandDndAction::Move) => {
            start_move_delete(xwm, offer_id, drop_timestamp, now_ns)?;
        }
        _ => {
            send_finished(xwm, snapshot, false, None, true)?;
            cleanup_terminal(xwm, offer_id);
        }
    }
    Ok(())
}

fn start_move_delete(
    xwm: &mut Xwm,
    offer_id: XwaylandDndOfferId,
    drop_timestamp: u32,
    now_ns: u64,
) -> Result<(), XwmError> {
    let Some(snapshot) = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .filter(|session| {
            session.offer_id == offer_id
                && session.generation == xwm.generation
                && matches!(
                    session.wire_phase,
                    IncomingDndWirePhase::AwaitingWaylandFinish { .. }
                )
        })
        .map(|session| DropSnapshot {
            offer_id,
            generation: session.generation,
            source: session.source.xid(),
            logical_target_root: session.logical_target_root,
            target_proxy: session.target_proxy,
            version: session.version,
            phase: session.wire_phase,
        })
    else {
        return Ok(());
    };
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
            &xproto::CreateWindowAux::new().event_mask(
                xproto::EventMask::PROPERTY_CHANGE | xproto::EventMask::STRUCTURE_NOTIFY,
            ),
        )
        .map_err(XwmError::Connection)?;
    std::mem::forget(create);
    let property = xwm.atoms.get(XwmAtomName::XdndDeleteResult);
    let delete = IncomingMoveDelete {
        offer_id,
        generation: xwm.generation,
        source: snapshot.source,
        requestor,
        property,
        drop_timestamp,
        deadline_ns: now_ns.saturating_add(INCOMING_DND_DELETE_TIMEOUT_NS),
    };
    xwm.data_bridge.dnd.internal_windows.insert(requestor);
    xwm.data_bridge.dnd_incoming.move_delete = Some(delete);
    if let Some(session) = xwm
        .data_bridge
        .dnd
        .incoming_session_mut()
        .filter(|session| session.offer_id == offer_id)
    {
        session.wire_phase = IncomingDndWirePhase::DeletePending {
            drop_timestamp,
            final_action: crate::xwayland::XwaylandDndAction::Move,
        };
    }
    let cookie = xwm
        .connection
        .convert_selection(
            requestor,
            xwm.atoms.get(XwmAtomName::XdndSelection),
            xwm.atoms.get(XwmAtomName::Delete),
            property,
            drop_timestamp,
        )
        .map_err(XwmError::Connection)?;
    std::mem::forget(cookie);
    xwm.connection.flush().map_err(XwmError::Connection)
}

pub(crate) fn selection_notify_delete(
    xwm: &mut Xwm,
    event: xproto::SelectionNotifyEvent,
) -> Result<bool, XwmError> {
    let Some(delete) = xwm
        .data_bridge
        .dnd_incoming
        .move_delete
        .filter(|delete| delete.requestor == event.requestor)
    else {
        return Ok(false);
    };
    let exact_session = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .is_some_and(|session| {
            session.generation == delete.generation
                && session.offer_id == delete.offer_id
                && session.source.xid() == delete.source
                && matches!(
                    session.wire_phase,
                    IncomingDndWirePhase::DeletePending {
                        drop_timestamp,
                        final_action: crate::xwayland::XwaylandDndAction::Move,
                    } if drop_timestamp == delete.drop_timestamp
                )
        });
    let exact = delete.generation == xwm.generation
        && exact_session
        && delete.source != 0
        && event.requestor == delete.requestor
        && event.selection == xwm.atoms.get(XwmAtomName::XdndSelection)
        && event.target == xwm.atoms.get(XwmAtomName::Delete)
        && event.time == delete.drop_timestamp;
    if !exact {
        return Ok(true);
    }
    let succeeded =
        event.property != u32::from(AtomEnum::NONE) && event.property == delete.property;
    send_finished_for_offer(
        xwm,
        delete.offer_id,
        succeeded,
        succeeded.then_some(crate::xwayland::XwaylandDndAction::Move),
        true,
    )?;
    cleanup_terminal(xwm, delete.offer_id);
    Ok(true)
}

pub(crate) fn requestor_destroyed(xwm: &mut Xwm, requestor: Window) -> bool {
    let Some(delete) = xwm
        .data_bridge
        .dnd_incoming
        .move_delete
        .filter(|delete| delete.requestor == requestor)
    else {
        return false;
    };
    let _ = send_finished_for_offer(xwm, delete.offer_id, false, None, true);
    cleanup_terminal(xwm, delete.offer_id);
    true
}

pub(crate) fn property_notify(xwm: &mut Xwm, event: xproto::PropertyNotifyEvent) -> bool {
    xwm.data_bridge
        .dnd_incoming
        .move_delete
        .is_some_and(|delete| delete.requestor == event.window)
}

pub(crate) fn expire_deadlines(xwm: &mut Xwm, now_ns: u64) -> Result<(), XwmError> {
    let Some((offer_id, phase)) = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .map(|session| (session.offer_id, session.wire_phase))
    else {
        return Ok(());
    };
    match phase {
        IncomingDndWirePhase::DropSubmitted {
            acknowledgement_deadline_ns,
            ..
        } if now_ns >= acknowledgement_deadline_ns => {
            let _ = enqueue_cancel_after_drop(xwm, offer_id);
            send_finished_for_offer(xwm, offer_id, false, None, true)?;
            cleanup_terminal(xwm, offer_id);
        }
        IncomingDndWirePhase::AwaitingWaylandFinish {
            drop_timestamp,
            action,
            deadline_ns,
            cancel_submitted: false,
        } if now_ns >= deadline_ns => {
            if enqueue_cancel_after_drop(xwm, offer_id) {
                if let Some(session) = xwm
                    .data_bridge
                    .dnd
                    .incoming_session_mut()
                    .filter(|session| session.offer_id == offer_id)
                {
                    session.wire_phase = IncomingDndWirePhase::AwaitingWaylandFinish {
                        drop_timestamp,
                        action,
                        deadline_ns: now_ns.saturating_add(INCOMING_DND_DROP_ACK_TIMEOUT_NS),
                        cancel_submitted: true,
                    };
                }
            } else {
                send_finished_for_offer(xwm, offer_id, false, None, true)?;
                cleanup_terminal(xwm, offer_id);
            }
        }
        IncomingDndWirePhase::AwaitingWaylandFinish {
            deadline_ns,
            cancel_submitted: true,
            ..
        } if now_ns >= deadline_ns => {
            send_finished_for_offer(xwm, offer_id, false, None, true)?;
            cleanup_terminal(xwm, offer_id);
        }
        IncomingDndWirePhase::DeletePending { .. }
            if xwm
                .data_bridge
                .dnd_incoming
                .move_delete
                .is_some_and(|delete| now_ns >= delete.deadline_ns) =>
        {
            send_finished_for_offer(xwm, offer_id, false, None, true)?;
            cleanup_terminal(xwm, offer_id);
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn source_destroyed(xwm: &mut Xwm, source: Window) -> Result<bool, XwmError> {
    let Some((offer_id, phase)) = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .filter(|session| session.source.xid() == source)
        .map(|session| (session.offer_id, session.wire_phase))
    else {
        return Ok(false);
    };
    match phase {
        IncomingDndWirePhase::Hover => {
            let _ = metadata::leave_offer(xwm, offer_id);
        }
        IncomingDndWirePhase::DropSubmitted { .. }
        | IncomingDndWirePhase::AwaitingWaylandFinish { .. } => {
            let _ = enqueue_cancel_after_drop(xwm, offer_id);
            cleanup_terminal(xwm, offer_id);
        }
        IncomingDndWirePhase::DeletePending { .. } | IncomingDndWirePhase::TerminalConsumed => {
            cleanup_terminal(xwm, offer_id)
        }
    }
    Ok(true)
}

pub(crate) fn canonical_retired(xwm: &mut Xwm, offer_id: XwaylandDndOfferId) -> bool {
    let Some(phase) = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .filter(|session| session.offer_id == offer_id)
        .map(|session| session.wire_phase)
    else {
        return false;
    };
    match phase {
        IncomingDndWirePhase::Hover | IncomingDndWirePhase::DropSubmitted { .. } => {
            cleanup_terminal(xwm, offer_id);
        }
        IncomingDndWirePhase::AwaitingWaylandFinish { .. } => {
            let _ = send_finished_for_offer(xwm, offer_id, false, None, true);
            cleanup_terminal(xwm, offer_id);
        }
        // Canonical success has already completed. DELETE is now only bounded
        // X11 wire cleanup and is not a second drag authority.
        IncomingDndWirePhase::DeletePending { .. } => return true,
        IncomingDndWirePhase::TerminalConsumed => cleanup_terminal(xwm, offer_id),
    }
    false
}

pub(crate) fn root_proxy_lost(xwm: &mut Xwm) -> Result<(), XwmError> {
    let Some((offer_id, phase)) = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .map(|session| (session.offer_id, session.wire_phase))
    else {
        return Ok(());
    };
    match phase {
        IncomingDndWirePhase::Hover => {
            let _ = metadata::leave_offer(xwm, offer_id);
        }
        IncomingDndWirePhase::DropSubmitted { .. }
        | IncomingDndWirePhase::AwaitingWaylandFinish { .. } => {
            let _ = enqueue_cancel_after_drop(xwm, offer_id);
            send_finished_for_offer(xwm, offer_id, false, None, true)?;
            cleanup_terminal(xwm, offer_id);
        }
        IncomingDndWirePhase::DeletePending { .. } => {
            send_finished_for_offer(xwm, offer_id, false, None, true)?;
            cleanup_terminal(xwm, offer_id);
        }
        IncomingDndWirePhase::TerminalConsumed => cleanup_terminal(xwm, offer_id),
    }
    Ok(())
}

fn send_finished_for_offer(
    xwm: &mut Xwm,
    offer_id: XwaylandDndOfferId,
    accepted: bool,
    action: Option<crate::xwayland::XwaylandDndAction>,
    source_alive: bool,
) -> Result<(), XwmError> {
    let Some(snapshot) = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .filter(|session| session.offer_id == offer_id)
        .map(|session| DropSnapshot {
            offer_id,
            generation: session.generation,
            source: session.source.xid(),
            logical_target_root: session.logical_target_root,
            target_proxy: session.target_proxy,
            version: session.version,
            phase: session.wire_phase,
        })
    else {
        return Ok(());
    };
    send_finished(xwm, snapshot, accepted, action, source_alive)
}

fn send_finished(
    xwm: &mut Xwm,
    snapshot: DropSnapshot,
    accepted: bool,
    action: Option<crate::xwayland::XwaylandDndAction>,
    source_alive: bool,
) -> Result<(), XwmError> {
    if !source_alive
        || snapshot.generation != xwm.generation
        || snapshot.source == 0
        || snapshot.version.get() < 5
        || target_proxy(xwm) != Some(snapshot.target_proxy)
        || snapshot.logical_target_root != xwm.root
    {
        return Ok(());
    }
    let Some(event) = dnd_wire::encode_xdnd_finished(
        snapshot.source,
        snapshot.logical_target_root,
        xwm.atoms.get(XwmAtomName::XdndFinished),
        accepted,
        action,
        &xwm.atoms,
    ) else {
        return Ok(());
    };
    let cookie = xwm
        .connection
        .send_event(false, snapshot.source, xproto::EventMask::NO_EVENT, event)
        .map_err(XwmError::Connection)?;
    std::mem::forget(cookie);
    xwm.connection.flush().map_err(XwmError::Connection)
}

fn cleanup_terminal(xwm: &mut Xwm, offer_id: XwaylandDndOfferId) {
    if let Some(session) = xwm
        .data_bridge
        .dnd
        .incoming_session_mut()
        .filter(|session| session.offer_id == offer_id)
    {
        session.wire_phase = IncomingDndWirePhase::TerminalConsumed;
    }
    metadata::cancel_metadata_replies(xwm, Some(offer_id));
    metadata::retire_offer_transfers(xwm, offer_id);
    let delete_is_current = xwm
        .data_bridge
        .dnd_incoming
        .move_delete
        .is_some_and(|delete| delete.offer_id == offer_id);
    if delete_is_current && let Some(delete) = xwm.data_bridge.dnd_incoming.move_delete.take() {
        xwm.data_bridge
            .dnd
            .internal_windows
            .remove(&delete.requestor);
        let _ = xwm.connection.destroy_window(delete.requestor);
    }
    xwm.data_bridge
        .dnd_incoming
        .events
        .retain(|event| match event {
            XwaylandDndIncomingEvent::Begin { offer, .. } => offer.id() != offer_id,
            XwaylandDndIncomingEvent::Position {
                offer_id: queued, ..
            }
            | XwaylandDndIncomingEvent::Leave { offer_id: queued }
            | XwaylandDndIncomingEvent::Drop { offer_id: queued } => *queued != offer_id,
            XwaylandDndIncomingEvent::CancelAfterDrop { .. } => true,
        });
    let _ = xwm.data_bridge.dnd.retire_incoming_session(offer_id);
}
