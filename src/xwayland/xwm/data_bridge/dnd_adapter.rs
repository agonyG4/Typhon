//! X11 wire protocol adapter for canonical Wayland-origin DND sessions.

#[path = "dnd_adapter_discovery.rs"]
mod discovery;
use discovery::{
    create_source_proxy, issue_aware_query, issue_proxy_validation,
    publish_source_metadata_and_claim, reply_is_current, start_target_discovery,
    valid_aware_version, valid_single_window,
};

use x11rb::{
    connection::{Connection, DiscardMode, RequestConnection, RequestKind},
    cookie::Cookie,
    protocol::xproto::{self, AtomEnum, ConnectionExt as XprotoConnectionExt, PropMode},
    wrapper::ConnectionExt as XprotoWrapperExt,
};

use super::{
    super::{X11WindowHandle, XwaylandGeneration, Xwm, XwmError, atoms::XwmAtomName},
    dnd::{
        CoalescedPosition, DND_REPLY_BUDGET, DndFeedback, DndPendingReply, DndStatusFeedback,
        DndStatusResult, DndTerminalFeedback, DndWireProgress, PositionDisposition,
        SOURCE_OWNERSHIP_TIMEOUT_NS, SOURCE_TIMESTAMP_TIMEOUT_NS, TARGET_DISCOVERY_TIMEOUT_NS,
        TARGET_FINISHED_TIMEOUT_NS, TARGET_STATUS_TIMEOUT_NS, status_action_authorizes_frozen_drop,
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
                if xwm.data_bridge.dnd.active_session().is_some_and(|session| {
                    matches!(
                        session.progress,
                        DndWireProgress::DropPending
                            | DndWireProgress::DropPendingAwaitingStatus
                            | DndWireProgress::AwaitingFinished
                            | DndWireProgress::TerminalConsumed
                    )
                }) {
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
                if let Some(session) = xwm
                    .data_bridge
                    .dnd
                    .outgoing_session_mut()
                    .filter(|session| session.id == id)
                {
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
                let (current_target, can_send_position, discovering, frozen) = {
                    let Some(session) = xwm
                        .data_bridge
                        .dnd
                        .outgoing_session_mut()
                        .filter(|session| session.id == id)
                    else {
                        continue;
                    };
                    let frozen = matches!(
                        session.progress,
                        DndWireProgress::DropPending
                            | DndWireProgress::DropPendingAwaitingStatus
                            | DndWireProgress::AwaitingFinished
                            | DndWireProgress::TerminalConsumed
                    );
                    if !frozen
                        && (session.target == Some(target)
                            || session.discovery_target == Some(target))
                    {
                        session.latest_position = Some(CoalescedPosition { x, y, action });
                    }
                    (
                        session.target == Some(target),
                        matches!(
                            session.progress,
                            DndWireProgress::Positioned | DndWireProgress::AwaitingStatus
                        ),
                        session.discovery_target == Some(target),
                        frozen,
                    )
                };
                if frozen {
                    continue;
                }
                if current_target && can_send_position {
                    match xwm.data_bridge.dnd.queue_position(
                        id,
                        target,
                        CoalescedPosition { x, y, action },
                    ) {
                        PositionDisposition::SendNow(position) => {
                            send_coalesced_position(xwm, id, target, position, now_ns)?;
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
                session_id,
                target,
                action,
                ..
            } => {
                let Some(id) = XwaylandDndAdapterId::new(session_id, xwm.generation) else {
                    continue;
                };
                let Some(session) = xwm
                    .data_bridge
                    .dnd
                    .active_session()
                    .filter(|session| session.id == id)
                    .cloned()
                else {
                    continue;
                };
                if !matches!(session_id, CanonicalDndSessionId::Wayland(_))
                    || matches!(
                        session.progress,
                        DndWireProgress::DropPending
                            | DndWireProgress::DropPendingAwaitingStatus
                            | DndWireProgress::AwaitingFinished
                            | DndWireProgress::TerminalConsumed
                    )
                {
                    continue;
                }
                if session.target != Some(target)
                    || !xwm.data_bridge.dnd.request_drop(id, target, action)
                {
                    reject_drop_request(xwm, id, target)?;
                    continue;
                }
                if xwm.data_bridge.dnd.awaiting_status(id) {
                    continue;
                }
                resolve_pending_drop(xwm, id, target, now_ns)?;
            }
            XwaylandDndTransition::TargetFinished {
                session_id, target, ..
            } => {
                let Some(id) = XwaylandDndAdapterId::new(session_id, xwm.generation) else {
                    continue;
                };
                if xwm
                    .data_bridge
                    .dnd
                    .active_session()
                    .is_some_and(|session| session.id == id && session.target == Some(target))
                {
                    retire_terminal_session_without_leave(xwm, id)?;
                }
            }
            XwaylandDndTransition::Retired {
                session_id,
                generation,
            } => {
                if generation == xwm.generation
                    && let Some(id) = XwaylandDndAdapterId::new(session_id, generation)
                {
                    if let CanonicalDndSessionId::Xwayland(offer_id) = session_id {
                        super::dnd_incoming::canonical_retired(xwm, offer_id);
                    }
                    retire_session_for_canonical_retirement(xwm, id)?;
                }
            }
            XwaylandDndTransition::SourceFeedback {
                offer_id,
                accepted_mime,
                action,
            } => super::dnd_incoming::source_feedback(xwm, offer_id, accepted_mime, action)?,
            XwaylandDndTransition::SourceFinished { .. } => {}
        }
    }
    Ok(())
}

fn reject_drop_request(
    xwm: &mut Xwm,
    id: XwaylandDndAdapterId,
    target: X11WindowHandle,
) -> Result<(), XwmError> {
    if let Some(current_target) = xwm
        .data_bridge
        .dnd
        .active_session()
        .filter(|session| session.id == id)
        .and_then(|session| session.target)
    {
        send_leave_for_current_target(xwm, id, current_target)?;
    }
    if xwm.data_bridge.dnd.consume_drop_rejection(id) {
        push_terminal_feedback(xwm, id, target, false, None)?;
    }
    Ok(())
}

fn reject_pending_drop(
    xwm: &mut Xwm,
    id: XwaylandDndAdapterId,
    target: X11WindowHandle,
) -> Result<(), XwmError> {
    send_leave_for_current_target(xwm, id, target)?;
    if xwm.data_bridge.dnd.consume_terminal_result(id) {
        push_terminal_feedback(xwm, id, target, false, None)?;
    }
    Ok(())
}

fn push_terminal_feedback(
    xwm: &mut Xwm,
    id: XwaylandDndAdapterId,
    target: X11WindowHandle,
    accepted: bool,
    action: Option<XwaylandDndAction>,
) -> Result<(), XwmError> {
    if xwm
        .data_bridge
        .dnd
        .push_terminal_feedback(DndTerminalFeedback {
            id,
            target,
            accepted,
            action,
        })
    {
        Ok(())
    } else {
        Err(XwmError::InvalidCommand(
            "XDND terminal feedback capacity exhausted",
        ))
    }
}

fn resolve_pending_drop(
    xwm: &mut Xwm,
    id: XwaylandDndAdapterId,
    target: X11WindowHandle,
    now_ns: u64,
) -> Result<(), XwmError> {
    let Some(session) = xwm
        .data_bridge
        .dnd
        .active_session()
        .filter(|session| {
            session.id == id
                && session.target == Some(target)
                && session.progress == DndWireProgress::DropPending
        })
        .cloned()
    else {
        return Ok(());
    };
    let Some(status) = session.last_status.filter(|status| status.accepted) else {
        return reject_pending_drop(xwm, id, target);
    };
    let Some(accepted_action) = status.action else {
        return reject_pending_drop(xwm, id, target);
    };
    let (Some(pending_drop_action), Some(target_version)) =
        (session.pending_drop_action, session.target_version)
    else {
        return reject_pending_drop(xwm, id, target);
    };
    if !session.source_actions.contains(&accepted_action)
        || accepted_action.to_wayland_action().is_none()
        || !status_action_authorizes_frozen_drop(
            pending_drop_action,
            accepted_action,
            target_version,
        )
    {
        return reject_pending_drop(xwm, id, target);
    }
    let (Some(source_proxy), Some(recipient), Some(timestamp)) = (
        session.source_proxy,
        session.wire_recipient,
        session
            .ownership_timestamp
            .filter(|_| session.ownership_confirmed),
    ) else {
        return reject_pending_drop(xwm, id, target);
    };
    let event = super::dnd_wire::encode_xdnd_drop(super::dnd_wire::XdndDropFields {
        actual_target: target.xid(),
        source_proxy,
        timestamp,
        drop_atom: xwm.atoms.get(XwmAtomName::XdndDrop),
    });
    let deadline_ns = now_ns.saturating_add(TARGET_FINISHED_TIMEOUT_NS);
    if !xwm
        .data_bridge
        .dnd
        .mark_awaiting_finished(id, accepted_action, deadline_ns)
    {
        return Ok(());
    }
    xwm.connection
        .send_event(false, recipient, xproto::EventMask::NO_EVENT, event)
        .map_err(XwmError::Connection)?;
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
    let position = CoalescedPosition { action, ..position };
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
    if !xwm.data_bridge.dnd.mark_initial_position_sent(id, position) {
        return Ok(());
    }
    let _ = xwm
        .data_bridge
        .dnd
        .set_status_deadline(id, now_ns.saturating_add(TARGET_STATUS_TIMEOUT_NS));
    Ok(())
}

fn send_coalesced_position(
    xwm: &mut Xwm,
    id: XwaylandDndAdapterId,
    target: X11WindowHandle,
    position: CoalescedPosition,
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
    let Some(coordinates) = super::dnd_wire::pack_root_coordinates(position.x, position.y) else {
        if matches!(session.progress, DndWireProgress::DropPendingAwaitingStatus) {
            reject_pending_drop(xwm, id, target)?;
        } else {
            send_leave_for_current_target(xwm, id, target)?;
            reject_target(xwm, id, target);
            let _ = xwm.data_bridge.dnd.leave_target(id, target);
        }
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
        if matches!(session.progress, DndWireProgress::DropPendingAwaitingStatus) {
            reject_pending_drop(xwm, id, target)?;
        } else {
            send_leave_for_current_target(xwm, id, target)?;
            reject_target(xwm, id, target);
            let _ = xwm.data_bridge.dnd.leave_target(id, target);
        }
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
    let _ = xwm
        .data_bridge
        .dnd
        .set_status_deadline(id, now_ns.saturating_add(TARGET_STATUS_TIMEOUT_NS));
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
    if xwm.data_bridge.dnd.active_session().is_some_and(|session| {
        session.id == id
            && matches!(
                session.progress,
                DndWireProgress::DropPending
                    | DndWireProgress::DropPendingAwaitingStatus
                    | DndWireProgress::AwaitingFinished
                    | DndWireProgress::TerminalConsumed
            )
    }) {
        return Ok(());
    }
    send_leave_for_current_target(xwm, id, target)?;
    cancel_discovery_replies(xwm, id, Some(target));
    let manager = &mut xwm.data_bridge.dnd;
    let _ = manager.leave_target(id, target);
    if let Some(session) = manager
        .outgoing_session_mut()
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
    xwm.data_bridge.dnd.push_status_feedback(DndStatusFeedback {
        id,
        target,
        accepted: false,
        action: None,
    });
    if let Some(session) = xwm
        .data_bridge
        .dnd
        .outgoing_session_mut()
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
    retire_hover_session_with_leave(xwm, id)
}

fn retire_hover_session_with_leave(
    xwm: &mut Xwm,
    id: XwaylandDndAdapterId,
) -> Result<(), XwmError> {
    retire_session_resources(xwm, id, RetirementWireSemantics::LeaveHoverTarget)
}

fn retire_terminal_session_without_leave(
    xwm: &mut Xwm,
    id: XwaylandDndAdapterId,
) -> Result<(), XwmError> {
    retire_session_resources(xwm, id, RetirementWireSemantics::NoLeaveAfterDrop)
}

fn retire_session_for_canonical_retirement(
    xwm: &mut Xwm,
    id: XwaylandDndAdapterId,
) -> Result<(), XwmError> {
    let dropped = xwm
        .data_bridge
        .dnd
        .active_session()
        .filter(|session| session.id == id)
        .is_some_and(|session| {
            matches!(
                session.progress,
                DndWireProgress::AwaitingFinished | DndWireProgress::TerminalConsumed
            )
        });
    if dropped {
        retire_terminal_session_without_leave(xwm, id)
    } else {
        retire_hover_session_with_leave(xwm, id)
    }
}

#[derive(Clone, Copy)]
enum RetirementWireSemantics {
    LeaveHoverTarget,
    NoLeaveAfterDrop,
}

fn retire_session_resources(
    xwm: &mut Xwm,
    id: XwaylandDndAdapterId,
    wire_semantics: RetirementWireSemantics,
) -> Result<(), XwmError> {
    let exact_session = xwm
        .data_bridge
        .dnd
        .active_session()
        .filter(|session| session.id == id)
        .cloned();
    let Some(session) = exact_session else {
        return Ok(());
    };
    if matches!(wire_semantics, RetirementWireSemantics::LeaveHoverTarget)
        && !matches!(
            session.progress,
            DndWireProgress::AwaitingFinished | DndWireProgress::TerminalConsumed
        )
        && let Some(target) = session.target
    {
        send_leave_for_current_target(xwm, id, target)?;
    }
    if let Some(source_proxy) = session.source_proxy {
        super::super::dnd_outgoing::cancel_source(
            xwm,
            crate::xwayland::XwaylandDndSourceProxyId {
                adapter_id: id,
                xid: source_proxy,
            },
            crate::native::event_loop::monotonic_now_ns().unwrap_or_default(),
        )?;
        xwm.data_bridge.dnd.internal_windows.remove(&source_proxy);
        if !session.source_proxy_destroyed {
            let cookie = xwm
                .connection
                .destroy_window(source_proxy)
                .map_err(XwmError::Connection)?;
            std::mem::forget(cookie);
        }
    }
    discard_pending_session_replies(xwm, id);
    let _ = xwm.data_bridge.dnd.retire(id);
    Ok(())
}

fn discard_pending_session_replies(xwm: &mut Xwm, id: XwaylandDndAdapterId) {
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
    for sequence in sequences {
        xwm.connection.discard_reply(
            sequence,
            RequestKind::HasResponse,
            DiscardMode::DiscardReply,
        );
        xwm.data_bridge.dnd.pending_replies.remove(&sequence);
    }
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
        retire_hover_session_with_leave(xwm, id)?;
        return Ok(true);
    }
    if let Some(session) = xwm
        .data_bridge
        .dnd
        .outgoing_session_mut()
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
    if event.type_ == xwm.atoms.get(XwmAtomName::XdndFinished) {
        handle_finished(xwm, event)?;
        return Ok(true);
    }
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
    let progress = session.progress;
    let requested_action = session
        .outstanding_position
        .and_then(|position| position.action);
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
        || !matches!(
            progress,
            DndWireProgress::AwaitingStatus | DndWireProgress::DropPendingAwaitingStatus
        )
    {
        return Ok(true);
    }
    let accepted_bit = data[1] & super::dnd_wire::XDND_STATUS_ACCEPTED != 0;
    let (accepted, action) = super::dnd_wire::decode_status_action(
        accepted_bit,
        data[4],
        requested_action,
        &source_actions,
        &xwm.atoms,
    );
    let result = DndStatusResult {
        accepted,
        action,
        requested_action,
    };
    let Some(acknowledgement) =
        xwm.data_bridge
            .dnd
            .acknowledge_status(id, source_proxy, target, recipient, result)
    else {
        return Ok(true);
    };
    let feedback = DndStatusFeedback {
        id,
        target,
        accepted,
        action,
    };
    xwm.data_bridge.dnd.push_status_feedback(feedback);
    if let Some(position) = acknowledgement.next_position {
        send_coalesced_position(xwm, id, target, position, now_ns)?;
    } else if acknowledgement.pending_drop_action.is_some() {
        resolve_pending_drop(xwm, id, target, now_ns)?;
    }
    Ok(true)
}

fn handle_finished(xwm: &mut Xwm, event: xproto::ClientMessageEvent) -> Result<(), XwmError> {
    let Some(session) = xwm.data_bridge.dnd.active_session().cloned() else {
        return Ok(());
    };
    let (Some(source_proxy), Some(target), Some(version), Some(accepted_status_action)) = (
        session.source_proxy,
        session.target,
        session.target_version,
        session.authorized_drop_action,
    ) else {
        return Ok(());
    };
    if event.format != 32
        || event.window != source_proxy
        || xwm.generation != session.id.generation()
        || target.generation() != xwm.generation
        || session.progress != DndWireProgress::AwaitingFinished
    {
        return Ok(());
    }
    let data = event.data.as_data32();
    if data[0] != target.xid() {
        return Ok(());
    }
    let decoded = super::dnd_wire::decode_xdnd_finished(
        data,
        version.get(),
        accepted_status_action,
        &session.source_actions,
        &xwm.atoms,
    );
    if !xwm.data_bridge.dnd.consume_terminal_result(session.id) {
        return Ok(());
    }
    push_terminal_feedback(xwm, session.id, target, decoded.accepted, decoded.action)
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
                    .outgoing_session_mut()
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
                        .outgoing_session_mut()
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
                    .outgoing_session_mut()
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
                    if let Some(session) =
                        xwm.data_bridge
                            .dnd
                            .outgoing_session_mut()
                            .filter(|session| {
                                session.id == id && session.source_proxy == Some(source_proxy)
                            })
                    {
                        session.ownership_confirmed = true;
                        session.ownership_deadline_ns = None;
                    }
                    maybe_send_enter(xwm, id, now_ns)?;
                } else {
                    reject_current_target(xwm, id);
                    retire_hover_session_with_leave(xwm, id)?;
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
        retire_hover_session_with_leave(xwm, id)?;
    } else if session
        .discovery_deadline_ns
        .is_some_and(|deadline| now_ns >= deadline)
        && let Some(target) = session.discovery_target
    {
        reject_target(xwm, id, target);
    } else if session
        .finished_deadline_ns
        .is_some_and(|deadline| now_ns >= deadline)
        && session.progress == DndWireProgress::AwaitingFinished
        && let Some(target) = session.target
    {
        if xwm.data_bridge.dnd.consume_terminal_result(id) {
            push_terminal_feedback(xwm, id, target, false, None)?;
        }
    } else if session
        .status_deadline_ns
        .is_some_and(|deadline| now_ns >= deadline)
        && let Some(target) = session.target
    {
        if session.progress == DndWireProgress::DropPendingAwaitingStatus {
            if xwm.data_bridge.dnd.status_timed_out(id, now_ns) {
                reject_pending_drop(xwm, id, target)?;
            }
        } else if session.progress == DndWireProgress::AwaitingStatus
            && xwm.data_bridge.dnd.status_timed_out(id, now_ns)
        {
            send_leave_for_current_target(xwm, id, target)?;
            reject_target(xwm, id, target);
            let _ = xwm.data_bridge.dnd.leave_target(id, target);
        }
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
                    retire_hover_session_with_leave(xwm, id)?;
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

pub(crate) fn take_feedback(xwm: &mut Xwm) -> Vec<crate::xwayland::XwaylandDndFeedback> {
    xwm.data_bridge
        .dnd
        .take_feedback()
        .into_iter()
        .map(|feedback| match feedback {
            DndFeedback::Status(feedback) => crate::xwayland::XwaylandDndFeedback::Status {
                session_id: feedback.id.session_id(),
                target: feedback.target,
                accepted: feedback.accepted,
                action: feedback.action,
            },
            DndFeedback::Terminal(feedback) => crate::xwayland::XwaylandDndFeedback::Terminal {
                session_id: feedback.id.session_id(),
                target: feedback.target,
                accepted: feedback.accepted,
                action: feedback.action,
            },
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
    let Some(session) = xwm
        .data_bridge
        .dnd
        .active_session()
        .filter(|session| session.source_proxy == Some(event.owner) && session.ownership_confirmed)
        .cloned()
    else {
        return Ok(false);
    };
    let id = session.id;
    match session.progress {
        DndWireProgress::DropPending | DndWireProgress::DropPendingAwaitingStatus => {
            if let Some(target) = session.target {
                reject_pending_drop(xwm, id, target)?;
            }
        }
        DndWireProgress::AwaitingFinished => {
            if let Some(target) = session.target
                && xwm.data_bridge.dnd.consume_terminal_result(id)
            {
                push_terminal_feedback(xwm, id, target, false, None)?;
            }
        }
        DndWireProgress::TerminalConsumed => {}
        _ => {
            reject_current_target(xwm, id);
            retire_hover_session_with_leave(xwm, id)?;
        }
    }
    Ok(true)
}

pub(crate) fn destroy_notify(xwm: &mut Xwm, window: u32) -> Result<bool, XwmError> {
    let destroyed_source = xwm
        .data_bridge
        .dnd
        .active_session()
        .filter(|session| session.source_proxy == Some(window))
        .cloned();
    let was_internal = xwm.data_bridge.dnd.internal_windows.remove(&window);
    if let Some(session) = destroyed_source {
        let id = session.id;
        let source = crate::xwayland::XwaylandDndSourceProxyId {
            adapter_id: id,
            xid: window,
        };
        xwm.data_bridge.dnd.mark_source_proxy_destroyed(id, window);
        super::super::dnd_outgoing::cancel_source(
            xwm,
            source,
            crate::native::event_loop::monotonic_now_ns().unwrap_or_default(),
        )?;
        discard_pending_session_replies(xwm, id);
        match session.progress {
            DndWireProgress::DropPending | DndWireProgress::DropPendingAwaitingStatus => {
                if let Some(target) = session.target {
                    reject_pending_drop(xwm, id, target)?;
                }
            }
            DndWireProgress::AwaitingFinished => {
                if let Some(target) = session.target
                    && xwm.data_bridge.dnd.consume_terminal_result(id)
                {
                    push_terminal_feedback(xwm, id, target, false, None)?;
                }
            }
            DndWireProgress::TerminalConsumed => {}
            _ => {
                reject_current_target(xwm, id);
                retire_hover_session_with_leave(xwm, id)?;
            }
        }
        return Ok(true);
    }
    let destroyed_target = xwm
        .data_bridge
        .dnd
        .active_session()
        .filter(|session| {
            session.target.is_some_and(|target| target.xid() == window)
                || session.wire_recipient == Some(window)
        })
        .cloned();
    if let Some(session) = destroyed_target {
        let Some(target) = session.target else {
            return Ok(was_internal);
        };
        match session.progress {
            DndWireProgress::DropPending
            | DndWireProgress::DropPendingAwaitingStatus
            | DndWireProgress::AwaitingFinished => {
                discard_pending_session_replies(xwm, session.id);
                if xwm.data_bridge.dnd.consume_terminal_result(session.id) {
                    push_terminal_feedback(xwm, session.id, target, false, None)?;
                }
            }
            DndWireProgress::TerminalConsumed => {}
            _ => {
                reject_target(xwm, session.id, target);
                let _ = xwm.data_bridge.dnd.leave_target(session.id, target);
            }
        }
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
    if let Some(id) = xwm
        .data_bridge
        .dnd
        .active_id()
        .filter(|id| id.generation() == generation)
    {
        retire_session_for_canonical_retirement(xwm, id)?;
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
