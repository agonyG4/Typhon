//! X11 wire protocol adapter for canonical Wayland-origin DND sessions.

use x11rb::{
    connection::{Connection, DiscardMode, RequestConnection, RequestKind},
    cookie::Cookie,
    protocol::xproto::{self, AtomEnum, ConnectionExt as XprotoConnectionExt, PropMode},
    wrapper::ConnectionExt as XprotoWrapperExt,
};

use super::{
    super::{X11WindowHandle, XwaylandGeneration, Xwm, XwmError, atoms::XwmAtomName},
    dnd::{
        CoalescedPosition, DND_REPLY_BUDGET, DndPendingReply, DndStatusFeedback, DndWireProgress,
        PositionDisposition, SOURCE_OWNERSHIP_TIMEOUT_NS, SOURCE_TIMESTAMP_TIMEOUT_NS,
        TARGET_DISCOVERY_TIMEOUT_NS, TARGET_STATUS_TIMEOUT_NS,
    },
};
use crate::xwayland::{
    CanonicalDndSessionId, XwaylandDndAction, XwaylandDndAdapterId, XwaylandDndTransition,
    XwaylandDndVersion,
};

pub(crate) fn apply_transitions(
    xwm: &mut Xwm,
    transitions: impl IntoIterator<Item = XwaylandDndTransition>,
    now_ns: u64,
) -> Result<(), XwmError> {
    for transition in transitions {
        match transition {
            XwaylandDndTransition::TargetEntered {
                session_id,
                target,
                x,
                y,
                mime_types,
                source_actions,
            } => {
                let Some(id) = XwaylandDndAdapterId::new(session_id, xwm.generation) else {
                    continue;
                };
                if !matches!(session_id, CanonicalDndSessionId::Wayland(_))
                    || target.generation() != xwm.generation
                {
                    continue;
                }
                if xwm.data_bridge.dnd.active_id() != Some(id) {
                    retire_active_session(xwm)?;
                    if !xwm
                        .data_bridge
                        .dnd
                        .install_wayland_session(id, mime_types, source_actions)
                    {
                        continue;
                    }
                    create_source_proxy(xwm, id, now_ns)?;
                }
                if let Some(session) = xwm.data_bridge.dnd.active.as_mut() {
                    session.latest_position = Some(super::dnd::CoalescedPosition {
                        x,
                        y,
                        action: super::dnd_wire::requested_action(&session.source_actions),
                    });
                }
                start_target_discovery(xwm, id, target, now_ns)?;
            }
            XwaylandDndTransition::TargetPositioned {
                session_id,
                target,
                x,
                y,
                action,
                ..
            } => {
                let Some(id) = XwaylandDndAdapterId::new(session_id, xwm.generation) else {
                    continue;
                };
                let (current_target, positioned, discovering) = {
                    let Some(session) = xwm
                        .data_bridge
                        .dnd
                        .active
                        .as_mut()
                        .filter(|session| session.id == id)
                    else {
                        continue;
                    };
                    if session.target == Some(target) || session.discovery_target == Some(target) {
                        session.latest_position = Some(CoalescedPosition { x, y, action });
                    }
                    (
                        session.target == Some(target),
                        session.progress == DndWireProgress::Positioned,
                        session.discovery_target == Some(target),
                    )
                };
                if current_target && positioned {
                    match xwm.data_bridge.dnd.queue_position(
                        id,
                        target,
                        CoalescedPosition { x, y, action },
                    ) {
                        PositionDisposition::SendNow(_) => {
                            send_coalesced_position(xwm, id, target, now_ns)?;
                        }
                        PositionDisposition::Coalesced | PositionDisposition::Stale => {}
                    }
                } else if !discovering {
                    continue;
                }
            }
            XwaylandDndTransition::TargetLeft { session_id, target } => {
                let Some(id) = XwaylandDndAdapterId::new(session_id, xwm.generation) else {
                    continue;
                };
                leave_target(xwm, id, target)?;
            }
            XwaylandDndTransition::DropRequested {
                session_id, target, ..
            } => {
                let Some(id) = XwaylandDndAdapterId::new(session_id, xwm.generation) else {
                    continue;
                };
                // C2-A has no successful Drop/Finished path. Leave wire hover;
                // the runtime rejects this exact canonical pending target.
                leave_target(xwm, id, target)?;
            }
            XwaylandDndTransition::TargetFinished {
                session_id, target, ..
            } => {
                let Some(id) = XwaylandDndAdapterId::new(session_id, xwm.generation) else {
                    continue;
                };
                leave_target(xwm, id, target)?;
            }
            XwaylandDndTransition::Retired {
                session_id,
                generation,
            } => {
                if generation == xwm.generation
                    && let Some(id) = XwaylandDndAdapterId::new(session_id, generation)
                {
                    retire_session(xwm, id)?;
                }
            }
            XwaylandDndTransition::SourceFeedback { .. }
            | XwaylandDndTransition::SourceFinished { .. } => {}
        }
    }
    Ok(())
}

fn create_source_proxy(
    xwm: &mut Xwm,
    id: XwaylandDndAdapterId,
    now_ns: u64,
) -> Result<(), XwmError> {
    xwm.data_bridge
        .dnd_outgoing
        .initialize_generation(xwm.generation);
    let window = xwm
        .connection
        .generate_id()
        .map_err(|error| XwmError::IdAllocation(error.to_string()))?;
    let cookie = xwm
        .connection
        .create_window(
            0,
            window,
            xwm.supporting_wm_check,
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
    std::mem::forget(cookie);
    xwm.data_bridge.dnd.internal_windows.insert(window);
    if !xwm.data_bridge.dnd.bind_source_proxy(id, window) {
        xwm.data_bridge.dnd.internal_windows.remove(&window);
        let _ = xwm.connection.destroy_window(window);
        return Ok(());
    }
    if let Some(session) = xwm.data_bridge.dnd.active.as_mut() {
        session.timestamp_deadline_ns = Some(now_ns.saturating_add(SOURCE_TIMESTAMP_TIMEOUT_NS));
    }
    let property_cookie = xwm
        .connection
        .change_property32(
            PropMode::REPLACE,
            window,
            xwm.atoms.get(XwmAtomName::XdndSourceTime),
            AtomEnum::INTEGER,
            &[1],
        )
        .map_err(XwmError::Connection)?;
    std::mem::forget(property_cookie);

    let (mime_types, deadline_ns) = {
        let session = xwm
            .data_bridge
            .dnd
            .active_session()
            .ok_or(XwmError::InvalidCommand("missing XDND session"))?;
        (
            session.mime_types.as_slice().to_vec(),
            now_ns.saturating_add(SOURCE_TIMESTAMP_TIMEOUT_NS),
        )
    };
    for (ordinal, mime_type) in mime_types.into_iter().enumerate() {
        let cookie = xwm
            .connection
            .intern_atom(false, mime_type.as_bytes())
            .map_err(XwmError::Connection)?;
        let sequence = cookie.sequence_number();
        std::mem::forget(cookie);
        xwm.data_bridge.dnd.pending_replies.insert(
            sequence,
            DndPendingReply::MimeAtom {
                id,
                ordinal,
                deadline_ns,
            },
        );
    }
    Ok(())
}

fn start_target_discovery(
    xwm: &mut Xwm,
    id: XwaylandDndAdapterId,
    actual: X11WindowHandle,
    now_ns: u64,
) -> Result<(), XwmError> {
    if actual.generation() != xwm.generation || actual.xid() == 0 {
        return Ok(());
    }
    if let Some(session) = xwm.data_bridge.dnd.active.as_ref()
        && session.target == Some(actual)
        && session.progress == DndWireProgress::Positioned
    {
        return Ok(());
    }
    cancel_discovery_replies(xwm, id, None);
    if let Some(old_target) = xwm
        .data_bridge
        .dnd
        .active_session()
        .and_then(|session| session.target)
        && old_target != actual
    {
        send_leave_for_current_target(xwm, id, old_target)?;
        let _ = xwm.data_bridge.dnd.leave_target(id, old_target);
    }
    let serial = xwm.data_bridge.dnd.next_discovery_serial.saturating_add(1);
    if serial == 0 {
        reject_target(xwm, id, actual);
        return Ok(());
    }
    xwm.data_bridge.dnd.next_discovery_serial = serial;
    let deadline_ns = now_ns.saturating_add(TARGET_DISCOVERY_TIMEOUT_NS);
    let Some(session) = xwm
        .data_bridge
        .dnd
        .active
        .as_mut()
        .filter(|session| session.id == id)
    else {
        return Ok(());
    };
    session.discovery_serial = serial;
    session.discovery_target = Some(actual);
    session.discovery_recipient = None;
    session.discovery_deadline_ns = Some(deadline_ns);
    let cookie = xwm
        .connection
        .get_property(
            false,
            actual.xid(),
            xwm.atoms.get(XwmAtomName::XdndProxy),
            AtomEnum::WINDOW,
            0,
            1,
        )
        .map_err(XwmError::Connection)?;
    let sequence = cookie.sequence_number();
    std::mem::forget(cookie);
    xwm.data_bridge.dnd.pending_replies.insert(
        sequence,
        DndPendingReply::TargetProxy {
            id,
            actual,
            serial,
            deadline_ns,
        },
    );
    Ok(())
}

fn valid_single_window(reply: &xproto::GetPropertyReply) -> Option<u32> {
    if reply.format != 32 || reply.type_ != u32::from(AtomEnum::WINDOW) || reply.bytes_after != 0 {
        return None;
    }
    let values = reply.value32()?.collect::<Vec<_>>();
    (values.len() == 1 && values[0] != 0).then_some(values[0])
}

fn valid_aware_version(reply: &xproto::GetPropertyReply) -> Option<XwaylandDndVersion> {
    if reply.format != 32 || reply.type_ != u32::from(AtomEnum::ATOM) || reply.bytes_after != 0 {
        return None;
    }
    let values = reply.value32()?.collect::<Vec<_>>();
    if values.len() != 1 {
        return None;
    }
    XwaylandDndVersion::negotiate_target(values[0])
}

fn issue_aware_query(
    xwm: &mut Xwm,
    id: XwaylandDndAdapterId,
    actual: X11WindowHandle,
    recipient: u32,
    serial: u64,
    deadline_ns: u64,
) -> Result<(), XwmError> {
    let cookie = xwm
        .connection
        .get_property(
            false,
            recipient,
            xwm.atoms.get(XwmAtomName::XdndAware),
            AtomEnum::ATOM,
            0,
            1,
        )
        .map_err(XwmError::Connection)?;
    let sequence = cookie.sequence_number();
    std::mem::forget(cookie);
    xwm.data_bridge.dnd.pending_replies.insert(
        sequence,
        DndPendingReply::Aware {
            id,
            actual,
            recipient,
            serial,
            deadline_ns,
        },
    );
    Ok(())
}

fn reply_is_current(
    xwm: &Xwm,
    id: XwaylandDndAdapterId,
    actual: X11WindowHandle,
    serial: u64,
    deadline_ns: u64,
    now_ns: u64,
) -> bool {
    xwm.generation == id.generation()
        && deadline_ns > now_ns
        && xwm.data_bridge.dnd.active_session().is_some_and(|session| {
            session.id == id
                && session.discovery_target == Some(actual)
                && session.discovery_serial == serial
                && session.discovery_deadline_ns == Some(deadline_ns)
        })
}

fn issue_proxy_validation(
    xwm: &mut Xwm,
    id: XwaylandDndAdapterId,
    actual: X11WindowHandle,
    proxy: u32,
    serial: u64,
    deadline_ns: u64,
) -> Result<(), XwmError> {
    let cookie = xwm
        .connection
        .get_property(
            false,
            proxy,
            xwm.atoms.get(XwmAtomName::XdndProxy),
            AtomEnum::WINDOW,
            0,
            1,
        )
        .map_err(XwmError::Connection)?;
    let sequence = cookie.sequence_number();
    std::mem::forget(cookie);
    xwm.data_bridge.dnd.pending_replies.insert(
        sequence,
        DndPendingReply::ProxySelf {
            id,
            actual,
            proxy,
            serial,
            deadline_ns,
        },
    );
    Ok(())
}

fn publish_source_metadata_and_claim(
    xwm: &mut Xwm,
    id: XwaylandDndAdapterId,
    now_ns: u64,
) -> Result<(), XwmError> {
    let Some(session) = xwm
        .data_bridge
        .dnd
        .active_session()
        .filter(|session| session.id == id)
    else {
        return Ok(());
    };
    let Some(source_proxy) = session.source_proxy else {
        return Ok(());
    };
    if session.mime_atoms.iter().any(Option::is_none)
        || session.mime_atoms.len() != session.mime_types.as_slice().len()
    {
        return Ok(());
    }
    let Some(timestamp) = session.ownership_timestamp else {
        return Ok(());
    };
    let already_claimed = session.ownership_claim_issued;
    let mime_atoms = session
        .mime_atoms
        .iter()
        .flatten()
        .copied()
        .collect::<Vec<_>>();
    let source_actions = session.source_actions.clone();
    if mime_atoms.len() > 3 {
        let cookie = xwm
            .connection
            .change_property32(
                PropMode::REPLACE,
                source_proxy,
                xwm.atoms.get(XwmAtomName::XdndTypeList),
                AtomEnum::ATOM,
                &mime_atoms,
            )
            .map_err(XwmError::Connection)?;
        std::mem::forget(cookie);
    }
    if source_actions.contains(&XwaylandDndAction::Ask) {
        let concrete = super::dnd_wire::wayland_actions(&source_actions)
            .into_iter()
            .filter(|action| {
                matches!(
                    action,
                    crate::xwayland::WaylandDndAction::Copy
                        | crate::xwayland::WaylandDndAction::Move
                )
            })
            .collect::<Vec<_>>();
        let atoms = concrete
            .iter()
            .filter_map(|action| match action {
                crate::xwayland::WaylandDndAction::Copy => {
                    Some(xwm.atoms.get(XwmAtomName::XdndActionCopy))
                }
                crate::xwayland::WaylandDndAction::Move => {
                    Some(xwm.atoms.get(XwmAtomName::XdndActionMove))
                }
                crate::xwayland::WaylandDndAction::Ask => None,
            })
            .collect::<Vec<_>>();
        if !atoms.is_empty() {
            let cookie = xwm
                .connection
                .change_property32(
                    PropMode::REPLACE,
                    source_proxy,
                    xwm.atoms.get(XwmAtomName::XdndActionList),
                    AtomEnum::ATOM,
                    &atoms,
                )
                .map_err(XwmError::Connection)?;
            std::mem::forget(cookie);
            let mut descriptions = Vec::new();
            for action in concrete {
                descriptions.extend_from_slice(match action {
                    crate::xwayland::WaylandDndAction::Copy => b"Copy\0",
                    crate::xwayland::WaylandDndAction::Move => b"Move\0",
                    crate::xwayland::WaylandDndAction::Ask => b"Ask\0",
                });
            }
            let cookie = xwm
                .connection
                .change_property8(
                    PropMode::REPLACE,
                    source_proxy,
                    xwm.atoms.get(XwmAtomName::XdndActionDescription),
                    xwm.atoms.get(XwmAtomName::String),
                    &descriptions,
                )
                .map_err(XwmError::Connection)?;
            std::mem::forget(cookie);
        }
    }
    if !already_claimed {
        let cookie = xwm
            .connection
            .set_selection_owner(
                source_proxy,
                xwm.atoms.get(XwmAtomName::XdndSelection),
                timestamp,
            )
            .map_err(XwmError::Connection)?;
        std::mem::forget(cookie);
        let cookie = xwm
            .connection
            .get_selection_owner(xwm.atoms.get(XwmAtomName::XdndSelection))
            .map_err(XwmError::Connection)?;
        let sequence = cookie.sequence_number();
        std::mem::forget(cookie);
        let deadline_ns = now_ns.saturating_add(SOURCE_OWNERSHIP_TIMEOUT_NS);
        if let Some(session) = xwm
            .data_bridge
            .dnd
            .active
            .as_mut()
            .filter(|session| session.id == id && session.source_proxy == Some(source_proxy))
        {
            session.ownership_claim_issued = true;
            session.ownership_deadline_ns = Some(deadline_ns);
        }
        xwm.data_bridge.dnd.pending_replies.insert(
            sequence,
            DndPendingReply::Ownership {
                id,
                source_proxy,
                timestamp,
                deadline_ns,
            },
        );
    }
    Ok(())
}

fn maybe_send_enter(xwm: &mut Xwm, id: XwaylandDndAdapterId, now_ns: u64) -> Result<(), XwmError> {
    let Some(session) = xwm
        .data_bridge
        .dnd
        .active_session()
        .filter(|session| session.id == id)
    else {
        return Ok(());
    };
    let (Some(source_proxy), Some(actual), Some(recipient), Some(version), Some(timestamp)) = (
        session.source_proxy,
        session.target,
        session.wire_recipient,
        session.target_version,
        session
            .ownership_timestamp
            .filter(|_| session.ownership_confirmed),
    ) else {
        return Ok(());
    };
    if session.progress != DndWireProgress::AwaitingEnter
        || session.mime_types.as_slice().is_empty()
        || session.mime_atoms.iter().any(Option::is_none)
        || super::dnd_wire::requested_action(&session.source_actions).is_none()
    {
        return Ok(());
    }
    let mime_atoms = session
        .mime_atoms
        .iter()
        .flatten()
        .copied()
        .collect::<Vec<_>>();
    let position = session.latest_position.unwrap_or(CoalescedPosition {
        x: 0.0,
        y: 0.0,
        action: None,
    });
    let Some(packed_coordinates) = super::dnd_wire::pack_root_coordinates(position.x, position.y)
    else {
        reject_target(xwm, id, actual);
        let _ = xwm.data_bridge.dnd.leave_target(id, actual);
        return Ok(());
    };
    let action = position
        .action
        .or_else(|| super::dnd_wire::requested_action(&session.source_actions));
    let Some(action_atom) =
        action.and_then(|action| super::dnd_wire::wayland_action_atom(&xwm.atoms, action))
    else {
        reject_target(xwm, id, actual);
        let _ = xwm.data_bridge.dnd.leave_target(id, actual);
        return Ok(());
    };
    let messages =
        super::dnd_wire::encode_enter_and_initial_position(super::dnd_wire::EnterPositionFields {
            actual_target: actual.xid(),
            source_proxy,
            version: version.get(),
            mime_atoms: &mime_atoms,
            coordinates: packed_coordinates,
            timestamp,
            action_atom,
            enter_atom: xwm.atoms.get(XwmAtomName::XdndEnter),
            position_atom: xwm.atoms.get(XwmAtomName::XdndPosition),
        });
    xwm.connection
        .send_event(false, recipient, xproto::EventMask::NO_EVENT, messages[0])
        .map_err(XwmError::Connection)?;
    if !xwm.data_bridge.dnd.mark_entered(id) {
        return Ok(());
    }
    let _ = xwm.data_bridge.dnd.position(
        id,
        actual,
        position.x.round() as i32,
        position.y.round() as i32,
        action,
    );
    xwm.connection
        .send_event(false, recipient, xproto::EventMask::NO_EVENT, messages[1])
        .map_err(XwmError::Connection)?;
    if let Some(session) = xwm
        .data_bridge
        .dnd
        .active
        .as_mut()
        .filter(|session| session.id == id)
    {
        session.awaiting_status = true;
        session.status_deadline_ns = Some(now_ns.saturating_add(TARGET_STATUS_TIMEOUT_NS));
    }
    Ok(())
}

fn send_coalesced_position(
    xwm: &mut Xwm,
    id: XwaylandDndAdapterId,
    target: X11WindowHandle,
    now_ns: u64,
) -> Result<(), XwmError> {
    let Some(session) = xwm
        .data_bridge
        .dnd
        .active_session()
        .filter(|session| session.id == id && session.target == Some(target))
    else {
        return Ok(());
    };
    let position = session.latest_position.unwrap_or(CoalescedPosition {
        x: f64::from(session.x),
        y: f64::from(session.y),
        action: session.action,
    });
    let Some(coordinates) = super::dnd_wire::pack_root_coordinates(position.x, position.y) else {
        send_leave_for_current_target(xwm, id, target)?;
        reject_target(xwm, id, target);
        let _ = xwm.data_bridge.dnd.leave_target(id, target);
        return Ok(());
    };
    let source_proxy = session.source_proxy.unwrap_or_default();
    let recipient = session.wire_recipient.unwrap_or_default();
    let timestamp = session.ownership_timestamp.unwrap_or_default();
    let Some(action_atom) = position
        .action
        .or_else(|| super::dnd_wire::requested_action(&session.source_actions))
        .and_then(|action| super::dnd_wire::wayland_action_atom(&xwm.atoms, action))
    else {
        send_leave_for_current_target(xwm, id, target)?;
        reject_target(xwm, id, target);
        let _ = xwm.data_bridge.dnd.leave_target(id, target);
        return Ok(());
    };
    let event = xproto::ClientMessageEvent {
        response_type: xproto::CLIENT_MESSAGE_EVENT,
        format: 32,
        sequence: 0,
        window: target.xid(),
        type_: xwm.atoms.get(XwmAtomName::XdndPosition),
        data: xproto::ClientMessageData::from([
            source_proxy,
            0,
            coordinates,
            timestamp,
            action_atom,
        ]),
    };
    xwm.connection
        .send_event(false, recipient, xproto::EventMask::NO_EVENT, event)
        .map_err(XwmError::Connection)?;
    if let Some(session) = xwm
        .data_bridge
        .dnd
        .active
        .as_mut()
        .filter(|session| session.id == id)
    {
        session.awaiting_status = true;
        session.status_deadline_ns = Some(now_ns.saturating_add(TARGET_STATUS_TIMEOUT_NS));
    }
    Ok(())
}

fn send_leave_for_current_target(
    xwm: &mut Xwm,
    id: XwaylandDndAdapterId,
    target: X11WindowHandle,
) -> Result<(), XwmError> {
    let Some(session) = xwm
        .data_bridge
        .dnd
        .active_session()
        .filter(|session| session.id == id && session.target == Some(target))
    else {
        return Ok(());
    };
    if session.progress != DndWireProgress::AwaitingEnter
        && let (Some(source_proxy), Some(recipient)) =
            (session.source_proxy, session.wire_recipient)
    {
        let event = xproto::ClientMessageEvent {
            response_type: xproto::CLIENT_MESSAGE_EVENT,
            format: 32,
            sequence: 0,
            window: target.xid(),
            type_: xwm.atoms.get(XwmAtomName::XdndLeave),
            data: xproto::ClientMessageData::from([source_proxy, 0, 0, 0, 0]),
        };
        xwm.connection
            .send_event(false, recipient, xproto::EventMask::NO_EVENT, event)
            .map_err(XwmError::Connection)?;
    }
    Ok(())
}

fn leave_target(
    xwm: &mut Xwm,
    id: XwaylandDndAdapterId,
    target: X11WindowHandle,
) -> Result<(), XwmError> {
    send_leave_for_current_target(xwm, id, target)?;
    cancel_discovery_replies(xwm, id, Some(target));
    let manager = &mut xwm.data_bridge.dnd;
    let _ = manager.leave_target(id, target);
    if let Some(session) = manager
        .active
        .as_mut()
        .filter(|session| session.id == id && session.discovery_target == Some(target))
    {
        session.discovery_target = None;
        session.discovery_recipient = None;
        session.discovery_deadline_ns = None;
    }
    Ok(())
}

fn cancel_discovery_replies(
    xwm: &mut Xwm,
    id: XwaylandDndAdapterId,
    target: Option<X11WindowHandle>,
) {
    let sequences = xwm
        .data_bridge
        .dnd
        .pending_replies
        .iter()
        .filter_map(|(sequence, reply)| {
            let matches = match reply {
                DndPendingReply::TargetProxy {
                    id: current,
                    actual,
                    ..
                }
                | DndPendingReply::ProxySelf {
                    id: current,
                    actual,
                    ..
                }
                | DndPendingReply::Aware {
                    id: current,
                    actual,
                    ..
                } => *current == id && target.is_none_or(|target| *actual == target),
                _ => false,
            };
            matches.then_some(*sequence)
        })
        .collect::<Vec<_>>();
    for sequence in sequences {
        xwm.connection.discard_reply(
            sequence,
            RequestKind::HasResponse,
            DiscardMode::DiscardReply,
        );
        xwm.data_bridge.dnd.pending_replies.remove(&sequence);
    }
}

fn reject_target(xwm: &mut Xwm, id: XwaylandDndAdapterId, target: X11WindowHandle) {
    xwm.data_bridge.dnd.push_feedback(DndStatusFeedback {
        id,
        target,
        accepted: false,
        action: None,
    });
    if let Some(session) = xwm
        .data_bridge
        .dnd
        .active
        .as_mut()
        .filter(|session| session.id == id && session.discovery_target == Some(target))
    {
        session.discovery_target = None;
        session.discovery_recipient = None;
        session.discovery_deadline_ns = None;
    }
}

fn retire_active_session(xwm: &mut Xwm) -> Result<(), XwmError> {
    let Some(id) = xwm.data_bridge.dnd.active_id() else {
        return Ok(());
    };
    retire_session(xwm, id)
}

fn retire_session(xwm: &mut Xwm, id: XwaylandDndAdapterId) -> Result<(), XwmError> {
    retire_session_inner(xwm, id, true)
}

fn retire_session_inner(
    xwm: &mut Xwm,
    id: XwaylandDndAdapterId,
    destroy_source_proxy: bool,
) -> Result<(), XwmError> {
    if let Some(target) = xwm
        .data_bridge
        .dnd
        .active_session()
        .filter(|session| session.id == id)
        .and_then(|session| session.target)
    {
        send_leave_for_current_target(xwm, id, target)?;
    }
    let source_proxy = xwm
        .data_bridge
        .dnd
        .active_session()
        .filter(|session| session.id == id)
        .and_then(|session| session.source_proxy);
    if let Some(source_proxy) = source_proxy {
        super::super::dnd_outgoing::cancel_source(
            xwm,
            crate::xwayland::XwaylandDndSourceProxyId {
                adapter_id: id,
                xid: source_proxy,
            },
            crate::native::event_loop::monotonic_now_ns().unwrap_or_default(),
        )?;
        if destroy_source_proxy {
            let cookie = xwm
                .connection
                .destroy_window(source_proxy)
                .map_err(XwmError::Connection)?;
            std::mem::forget(cookie);
        }
    }
    let sequences = xwm
        .data_bridge
        .dnd
        .pending_replies
        .iter()
        .filter_map(|(sequence, reply)| {
            let same = match reply {
                DndPendingReply::TargetProxy { id: current, .. }
                | DndPendingReply::ProxySelf { id: current, .. }
                | DndPendingReply::Aware { id: current, .. }
                | DndPendingReply::MimeAtom { id: current, .. }
                | DndPendingReply::Ownership { id: current, .. }
                | DndPendingReply::MultipleRead { id: current, .. } => *current == id,
            };
            same.then_some(*sequence)
        })
        .collect::<Vec<_>>();
    for sequence in &sequences {
        xwm.connection.discard_reply(
            *sequence,
            RequestKind::HasResponse,
            DiscardMode::DiscardReply,
        );
        xwm.data_bridge.dnd.pending_replies.remove(sequence);
    }
    let _ = xwm.data_bridge.dnd.retire(id);
    Ok(())
}

pub(crate) fn is_internal_window(xwm: &Xwm, window: u32) -> bool {
    xwm.data_bridge.dnd.internal_windows.contains(&window)
}

pub(crate) fn property_notify(
    xwm: &mut Xwm,
    event: xproto::PropertyNotifyEvent,
    now_ns: u64,
) -> Result<bool, XwmError> {
    if event.state != xproto::Property::NEW_VALUE
        || event.atom != xwm.atoms.get(XwmAtomName::XdndSourceTime)
    {
        return Ok(false);
    }
    let Some((id, source_proxy, deadline_ns)) =
        xwm.data_bridge.dnd.active_session().and_then(|session| {
            (session.source_proxy == Some(event.window) && session.timestamp_deadline_ns.is_some())
                .then_some((
                    session.id,
                    event.window,
                    session.timestamp_deadline_ns.unwrap_or_default(),
                ))
        })
    else {
        return Ok(false);
    };
    if deadline_ns <= now_ns {
        reject_current_target(xwm, id);
        return Ok(true);
    }
    // X uses CurrentTime (zero) as a special SetSelectionOwner request value,
    // so it cannot prove the exact property-event timestamp needed here.
    if event.time == 0 {
        reject_current_target(xwm, id);
        retire_session(xwm, id)?;
        return Ok(true);
    }
    if let Some(session) = xwm
        .data_bridge
        .dnd
        .active
        .as_mut()
        .filter(|session| session.id == id && session.source_proxy == Some(source_proxy))
    {
        session.ownership_timestamp = Some(event.time);
        session.timestamp_deadline_ns = None;
    }
    publish_source_metadata_and_claim(xwm, id, now_ns)?;
    Ok(true)
}

pub(crate) fn client_message(
    xwm: &mut Xwm,
    event: xproto::ClientMessageEvent,
    now_ns: u64,
) -> Result<bool, XwmError> {
    if event.type_ != xwm.atoms.get(XwmAtomName::XdndStatus)
        || !is_internal_window(xwm, event.window)
    {
        return Ok(false);
    }
    let Some(session) = xwm.data_bridge.dnd.active_session() else {
        return Ok(true);
    };
    let id = session.id;
    let source_proxy = session.source_proxy.unwrap_or_default();
    let source_actions = session.source_actions.clone();
    let Some(target) = session.target else {
        return Ok(true);
    };
    let Some(recipient) = session.wire_recipient else {
        return Ok(true);
    };
    let data = event.data.as_data32();
    if event.format != 32
        || event.window != source_proxy
        || data[0] != target.xid()
        || target.generation() != xwm.generation
    {
        return Ok(true);
    }
    let Some(next) = xwm
        .data_bridge
        .dnd
        .acknowledge_status(id, source_proxy, target, recipient)
    else {
        return Ok(true);
    };
    let accepted_bit = data[1] & super::dnd_wire::XDND_STATUS_ACCEPTED != 0;
    let (accepted, action) =
        super::dnd_wire::decode_status_action(accepted_bit, data[4], &source_actions, &xwm.atoms);
    let feedback = DndStatusFeedback {
        id,
        target,
        accepted,
        action,
    };
    xwm.data_bridge.dnd.push_feedback(feedback);
    if let Some(position) = next {
        if let Some(session) = xwm
            .data_bridge
            .dnd
            .active
            .as_mut()
            .filter(|session| session.id == id)
        {
            session.latest_position = Some(position);
        }
        send_coalesced_position(xwm, id, target, now_ns)?;
    }
    Ok(true)
}

fn reject_current_target(xwm: &mut Xwm, id: XwaylandDndAdapterId) {
    if let Some(target) = xwm.data_bridge.dnd.active_session().and_then(|session| {
        (session.id == id)
            .then_some(session.discovery_target)
            .flatten()
    }) {
        reject_target(xwm, id, target);
    } else if let Some(target) = xwm
        .data_bridge
        .dnd
        .active_session()
        .and_then(|session| (session.id == id).then_some(session.target).flatten())
    {
        reject_target(xwm, id, target);
    }
}

fn current_reply_session(xwm: &Xwm, id: XwaylandDndAdapterId, source_proxy: u32) -> bool {
    xwm.generation == id.generation()
        && xwm
            .data_bridge
            .dnd
            .active_session()
            .is_some_and(|session| session.id == id && session.source_proxy == Some(source_proxy))
}

pub(crate) fn poll_replies(xwm: &mut Xwm, budget: usize, now_ns: u64) -> Result<usize, XwmError> {
    let sequences = xwm
        .data_bridge
        .dnd
        .pending_replies
        .keys()
        .copied()
        .take(budget.min(DND_REPLY_BUDGET))
        .collect::<Vec<_>>();
    let mut processed = 0;
    for sequence in sequences {
        let Some(pending) = xwm.data_bridge.dnd.pending_replies.get(&sequence).copied() else {
            continue;
        };
        match pending {
            DndPendingReply::TargetProxy {
                id,
                actual,
                serial,
                deadline_ns,
            } => {
                let cookie = Cookie::<
                    super::super::connection::X11Connection,
                    xproto::GetPropertyReply,
                >::new(&xwm.connection, sequence);
                let reply = match cookie.reply_unchecked() {
                    Ok(reply) => reply,
                    Err(x11rb::errors::ConnectionError::IoError(error))
                        if error.kind() == std::io::ErrorKind::WouldBlock =>
                    {
                        continue;
                    }
                    Err(error) => return Err(XwmError::Connection(error)),
                };
                xwm.data_bridge.dnd.pending_replies.remove(&sequence);
                processed += 1;
                if !reply_is_current(xwm, id, actual, serial, deadline_ns, now_ns) {
                    continue;
                }
                if let Some(proxy) = reply.as_ref().and_then(valid_single_window) {
                    issue_proxy_validation(xwm, id, actual, proxy, serial, deadline_ns)?;
                } else {
                    issue_aware_query(xwm, id, actual, actual.xid(), serial, deadline_ns)?;
                }
            }
            DndPendingReply::ProxySelf {
                id,
                actual,
                proxy,
                serial,
                deadline_ns,
            } => {
                let cookie = Cookie::<
                    super::super::connection::X11Connection,
                    xproto::GetPropertyReply,
                >::new(&xwm.connection, sequence);
                let reply = match cookie.reply_unchecked() {
                    Ok(reply) => reply,
                    Err(x11rb::errors::ConnectionError::IoError(error))
                        if error.kind() == std::io::ErrorKind::WouldBlock =>
                    {
                        continue;
                    }
                    Err(error) => return Err(XwmError::Connection(error)),
                };
                xwm.data_bridge.dnd.pending_replies.remove(&sequence);
                processed += 1;
                if !reply_is_current(xwm, id, actual, serial, deadline_ns, now_ns) {
                    continue;
                }
                let valid = reply.as_ref().and_then(valid_single_window) == Some(proxy);
                let recipient = super::dnd_wire::validated_proxy(
                    actual.xid(),
                    Some(proxy),
                    valid.then_some(proxy),
                );
                if let Some(session) = xwm
                    .data_bridge
                    .dnd
                    .active
                    .as_mut()
                    .filter(|session| session.id == id && session.discovery_serial == serial)
                {
                    session.discovery_recipient = Some(recipient);
                }
                issue_aware_query(xwm, id, actual, recipient, serial, deadline_ns)?;
            }
            DndPendingReply::Aware {
                id,
                actual,
                recipient,
                serial,
                deadline_ns,
            } => {
                let cookie = Cookie::<
                    super::super::connection::X11Connection,
                    xproto::GetPropertyReply,
                >::new(&xwm.connection, sequence);
                let reply = match cookie.reply_unchecked() {
                    Ok(reply) => reply,
                    Err(x11rb::errors::ConnectionError::IoError(error))
                        if error.kind() == std::io::ErrorKind::WouldBlock =>
                    {
                        continue;
                    }
                    Err(error) => return Err(XwmError::Connection(error)),
                };
                xwm.data_bridge.dnd.pending_replies.remove(&sequence);
                processed += 1;
                if !reply_is_current(xwm, id, actual, serial, deadline_ns, now_ns) {
                    continue;
                }
                let version = reply.as_ref().and_then(valid_aware_version);
                if let Some(version) = version {
                    let session = &mut xwm
                        .data_bridge
                        .dnd
                        .active
                        .as_mut()
                        .expect("current discovery session");
                    session.discovery_target = None;
                    session.discovery_recipient = None;
                    session.discovery_deadline_ns = None;
                    if xwm
                        .data_bridge
                        .dnd
                        .set_discovered_target(id, actual, recipient, version)
                    {
                        maybe_send_enter(xwm, id, now_ns)?;
                    }
                } else {
                    reject_target(xwm, id, actual);
                }
            }
            DndPendingReply::MimeAtom {
                id,
                ordinal,
                deadline_ns,
            } => {
                let cookie = Cookie::<
                    super::super::connection::X11Connection,
                    xproto::InternAtomReply,
                >::new(&xwm.connection, sequence);
                let reply = match cookie.reply_unchecked() {
                    Ok(reply) => reply,
                    Err(x11rb::errors::ConnectionError::IoError(error))
                        if error.kind() == std::io::ErrorKind::WouldBlock =>
                    {
                        continue;
                    }
                    Err(error) => return Err(XwmError::Connection(error)),
                };
                xwm.data_bridge.dnd.pending_replies.remove(&sequence);
                processed += 1;
                let Some(reply) = reply else {
                    continue;
                };
                if deadline_ns <= now_ns
                    || !xwm
                        .data_bridge
                        .dnd
                        .active_session()
                        .is_some_and(|session| session.id == id && session.source_proxy.is_some())
                {
                    continue;
                }
                if reply.atom == x11rb::NONE {
                    reject_current_target(xwm, id);
                    continue;
                }
                if let Some(session) = xwm
                    .data_bridge
                    .dnd
                    .active
                    .as_mut()
                    .filter(|session| session.id == id && ordinal < session.mime_atoms.len())
                {
                    session.mime_atoms[ordinal] = Some(reply.atom);
                }
                publish_source_metadata_and_claim(xwm, id, now_ns)?;
                maybe_send_enter(xwm, id, now_ns)?;
            }
            DndPendingReply::Ownership {
                id,
                source_proxy,
                timestamp,
                deadline_ns,
            } => {
                let cookie = Cookie::<
                    super::super::connection::X11Connection,
                    xproto::GetSelectionOwnerReply,
                >::new(&xwm.connection, sequence);
                let reply = match cookie.reply_unchecked() {
                    Ok(reply) => reply,
                    Err(x11rb::errors::ConnectionError::IoError(error))
                        if error.kind() == std::io::ErrorKind::WouldBlock =>
                    {
                        continue;
                    }
                    Err(error) => return Err(XwmError::Connection(error)),
                };
                xwm.data_bridge.dnd.pending_replies.remove(&sequence);
                processed += 1;
                let confirmed = reply.is_some_and(|reply| reply.owner == source_proxy)
                    && deadline_ns > now_ns
                    && current_reply_session(xwm, id, source_proxy)
                    && xwm.data_bridge.dnd.active_session().is_some_and(|session| {
                        session.ownership_timestamp == Some(timestamp)
                            && session.ownership_claim_issued
                    });
                if confirmed {
                    if let Some(session) = xwm.data_bridge.dnd.active.as_mut().filter(|session| {
                        session.id == id && session.source_proxy == Some(source_proxy)
                    }) {
                        session.ownership_confirmed = true;
                        session.ownership_deadline_ns = None;
                    }
                    maybe_send_enter(xwm, id, now_ns)?;
                } else {
                    reject_current_target(xwm, id);
                    retire_session(xwm, id)?;
                }
            }
            DndPendingReply::MultipleRead {
                id,
                source_proxy,
                target,
                requestor,
                property,
                request_time,
                deadline_ns,
            } => {
                let cookie = Cookie::<
                    super::super::connection::X11Connection,
                    xproto::GetPropertyReply,
                >::new(&xwm.connection, sequence);
                let reply = match cookie.reply_unchecked() {
                    Ok(reply) => reply,
                    Err(x11rb::errors::ConnectionError::IoError(error))
                        if error.kind() == std::io::ErrorKind::WouldBlock =>
                    {
                        continue;
                    }
                    Err(error) => return Err(XwmError::Connection(error)),
                };
                xwm.data_bridge.dnd.pending_replies.remove(&sequence);
                processed += 1;
                super::dnd_selection::handle_multiple_reply(
                    xwm,
                    super::dnd_selection::MultipleRequestContext {
                        id,
                        source_proxy,
                        target,
                        requestor,
                        property,
                        request_time,
                        deadline_ns,
                    },
                    reply,
                    now_ns,
                )?;
            }
        }
    }
    Ok(processed)
}

pub(crate) fn handle_deadline(xwm: &mut Xwm, now_ns: u64) -> Result<(), XwmError> {
    let Some(session) = xwm.data_bridge.dnd.active_session().cloned() else {
        return Ok(());
    };
    let id = session.id;
    if session
        .timestamp_deadline_ns
        .is_some_and(|deadline| now_ns >= deadline)
        || session
            .ownership_deadline_ns
            .is_some_and(|deadline| now_ns >= deadline)
    {
        reject_current_target(xwm, id);
        retire_session(xwm, id)?;
    } else if session
        .discovery_deadline_ns
        .is_some_and(|deadline| now_ns >= deadline)
        && let Some(target) = session.discovery_target
    {
        reject_target(xwm, id, target);
    } else if session
        .status_deadline_ns
        .is_some_and(|deadline| now_ns >= deadline)
        && let Some(target) = session.target
    {
        send_leave_for_current_target(xwm, id, target)?;
        reject_target(xwm, id, target);
        let _ = xwm.data_bridge.dnd.leave_target(id, target);
    }
    let expired = xwm
        .data_bridge
        .dnd
        .pending_replies
        .iter()
        .filter_map(|(sequence, reply)| {
            let deadline = match reply {
                DndPendingReply::TargetProxy { deadline_ns, .. }
                | DndPendingReply::ProxySelf { deadline_ns, .. }
                | DndPendingReply::Aware { deadline_ns, .. }
                | DndPendingReply::MimeAtom { deadline_ns, .. }
                | DndPendingReply::Ownership { deadline_ns, .. }
                | DndPendingReply::MultipleRead { deadline_ns, .. } => *deadline_ns,
            };
            (now_ns >= deadline).then_some(*sequence)
        })
        .collect::<Vec<_>>();
    for sequence in expired {
        xwm.connection.discard_reply(
            sequence,
            RequestKind::HasResponse,
            DiscardMode::DiscardReply,
        );
        if let Some(reply) = xwm.data_bridge.dnd.pending_replies.remove(&sequence) {
            match reply {
                DndPendingReply::TargetProxy { id, actual, .. }
                | DndPendingReply::ProxySelf { id, actual, .. }
                | DndPendingReply::Aware { id, actual, .. } => reject_target(xwm, id, actual),
                DndPendingReply::MimeAtom { id, .. } | DndPendingReply::Ownership { id, .. } => {
                    reject_current_target(xwm, id);
                    retire_session(xwm, id)?;
                }
                DndPendingReply::MultipleRead {
                    requestor,
                    property,
                    request_time,
                    ..
                } => {
                    super::super::dnd_outgoing::complete_single_conversion(
                        xwm,
                        requestor,
                        request_time,
                        xwm.atoms.get(XwmAtomName::Multiple),
                        property,
                        false,
                    )?;
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn take_feedback(xwm: &mut Xwm) -> Vec<crate::xwayland::XwaylandDndStatusFeedback> {
    xwm.data_bridge
        .dnd
        .take_feedback()
        .into_iter()
        .map(|feedback| crate::xwayland::XwaylandDndStatusFeedback {
            session_id: feedback.id.session_id(),
            target: feedback.target,
            accepted: feedback.accepted,
            action: feedback.action,
        })
        .collect()
}

pub(crate) fn selection_clear(
    xwm: &mut Xwm,
    event: xproto::SelectionClearEvent,
    _now_ns: u64,
) -> Result<bool, XwmError> {
    if event.selection != xwm.atoms.get(XwmAtomName::XdndSelection) {
        return Ok(false);
    }
    let Some(session) =
        xwm.data_bridge.dnd.active_session().filter(|session| {
            session.source_proxy == Some(event.owner) && session.ownership_confirmed
        })
    else {
        return Ok(false);
    };
    let id = session.id;
    reject_current_target(xwm, id);
    retire_session(xwm, id)?;
    Ok(true)
}

pub(crate) fn destroy_notify(xwm: &mut Xwm, window: u32) -> Result<bool, XwmError> {
    let destroyed_source = xwm
        .data_bridge
        .dnd
        .active_session()
        .filter(|session| session.source_proxy == Some(window))
        .map(|session| session.id);
    let was_internal = xwm.data_bridge.dnd.internal_windows.remove(&window);
    if let Some(id) = destroyed_source {
        reject_current_target(xwm, id);
        retire_session_inner(xwm, id, false)?;
        return Ok(true);
    }
    Ok(was_internal)
}

pub(crate) fn retire_generation(
    xwm: &mut Xwm,
    generation: XwaylandGeneration,
) -> Result<(), XwmError> {
    if generation != xwm.generation {
        return Ok(());
    }
    super::super::dnd_outgoing::cancel_generation(
        xwm,
        generation,
        crate::native::event_loop::monotonic_now_ns().unwrap_or_default(),
    )?;
    if let Some(session) = xwm.data_bridge.dnd.active_session().cloned() {
        if let Some(target) = session.target {
            send_leave_for_current_target(xwm, session.id, target)?;
        }
        if let Some(window) = session.source_proxy {
            let cookie = xwm
                .connection
                .destroy_window(window)
                .map_err(XwmError::Connection)?;
            std::mem::forget(cookie);
        }
    }
    for sequence in std::mem::take(&mut xwm.data_bridge.dnd.pending_replies).into_keys() {
        xwm.connection.discard_reply(
            sequence,
            RequestKind::HasResponse,
            DiscardMode::DiscardReply,
        );
    }
    xwm.data_bridge.dnd.clear_generation(generation);
    Ok(())
}

#[cfg(test)]
#[path = "dnd_adapter_tests.rs"]
mod tests;
