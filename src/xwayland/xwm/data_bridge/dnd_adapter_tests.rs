use std::{
    io::{Read, Write},
    num::NonZeroU64,
};

use super::*;
use crate::xwayland::{XwaylandDndAction, XwaylandDndMimeCatalog};

#[path = "dnd_adapter_pending_drop_tests.rs"]
mod pending_drop_tests;

fn identity(generation: XwaylandGeneration, session: u64) -> XwaylandDndAdapterId {
    XwaylandDndAdapterId::new(
        CanonicalDndSessionId::Wayland(NonZeroU64::new(session).unwrap()),
        generation,
    )
    .unwrap()
}

fn catalog(types: &[&str]) -> XwaylandDndMimeCatalog {
    XwaylandDndMimeCatalog::try_new(types.iter().map(|mime| (*mime).to_owned()).collect()).unwrap()
}

fn install_session(xwm: &mut Xwm, id: XwaylandDndAdapterId) {
    install_session_with_actions(
        xwm,
        id,
        vec![XwaylandDndAction::Copy, XwaylandDndAction::Move],
    );
}

fn install_session_with_actions(
    xwm: &mut Xwm,
    id: XwaylandDndAdapterId,
    source_actions: Vec<XwaylandDndAction>,
) {
    assert!(xwm.data_bridge.dnd.install_wayland_session(
        id,
        catalog(&["text/plain"]),
        source_actions,
    ));
}

fn install_waiting_position(
    xwm: &mut Xwm,
    id: XwaylandDndAdapterId,
    target: X11WindowHandle,
    recipient: u32,
    source_proxy: u32,
    action: XwaylandDndAction,
) {
    install_waiting_position_with_actions(
        xwm,
        id,
        target,
        recipient,
        source_proxy,
        action,
        vec![XwaylandDndAction::Copy, XwaylandDndAction::Move],
    );
}

fn install_waiting_position_with_actions(
    xwm: &mut Xwm,
    id: XwaylandDndAdapterId,
    target: X11WindowHandle,
    recipient: u32,
    source_proxy: u32,
    action: XwaylandDndAction,
    source_actions: Vec<XwaylandDndAction>,
) {
    install_session_with_actions(xwm, id, source_actions);
    assert!(xwm.data_bridge.dnd.bind_source_proxy(id, source_proxy));
    xwm.data_bridge.dnd.internal_windows.insert(source_proxy);
    assert!(
        xwm.data_bridge
            .dnd
            .confirm_source_ownership(id, source_proxy, 1234)
    );
    assert!(xwm.data_bridge.dnd.set_discovered_target(
        id,
        target,
        recipient,
        XwaylandDndVersion::new(5).unwrap(),
    ));
    assert!(xwm.data_bridge.dnd.mark_entered(id));
    assert!(xwm.data_bridge.dnd.position(id, target, 5, 6, Some(action)));
    let position = CoalescedPosition {
        x: 5.0,
        y: 6.0,
        action: Some(action),
    };
    assert_eq!(
        xwm.data_bridge.dnd.queue_position(id, target, position),
        PositionDisposition::SendNow(position)
    );
    send_coalesced_position(xwm, id, target, position, 0).unwrap();
}

fn status_message(
    xwm: &Xwm,
    source_proxy: u32,
    target: X11WindowHandle,
    accepted: bool,
    action: XwaylandDndAction,
) -> xproto::ClientMessageEvent {
    xproto::ClientMessageEvent {
        response_type: xproto::CLIENT_MESSAGE_EVENT,
        format: 32,
        sequence: 0,
        window: source_proxy,
        type_: xwm.atoms.get(XwmAtomName::XdndStatus),
        data: xproto::ClientMessageData::from([
            target.xid(),
            if accepted {
                super::super::dnd_wire::XDND_STATUS_ACCEPTED
            } else {
                0
            },
            0,
            0,
            match action {
                XwaylandDndAction::Copy => xwm.atoms.get(XwmAtomName::XdndActionCopy),
                XwaylandDndAction::Move => xwm.atoms.get(XwmAtomName::XdndActionMove),
                XwaylandDndAction::Ask => xwm.atoms.get(XwmAtomName::XdndActionAsk),
                XwaylandDndAction::Link => xwm.atoms.get(XwmAtomName::XdndActionLink),
                XwaylandDndAction::Private => xwm.atoms.get(XwmAtomName::XdndActionPrivate),
            },
        ]),
    }
}

fn drop_transition(
    id: XwaylandDndAdapterId,
    target: X11WindowHandle,
    action: XwaylandDndAction,
) -> XwaylandDndTransition {
    XwaylandDndTransition::DropRequested {
        session_id: id.session_id(),
        target,
        action,
        mime_types: catalog(&["text/plain"]),
        source_actions: vec![XwaylandDndAction::Copy, XwaylandDndAction::Move],
    }
}

#[allow(clippy::too_many_arguments)] // Keeps the synthetic peer setup explicit at each call site.
fn send_accepted_drop(
    xwm: &mut Xwm,
    peer: &mut std::os::unix::net::UnixStream,
    id: XwaylandDndAdapterId,
    target: X11WindowHandle,
    recipient: u32,
    source_proxy: u32,
    target_version: u8,
    status_action: XwaylandDndAction,
    source_actions: Vec<XwaylandDndAction>,
) {
    install_waiting_position_with_actions(
        xwm,
        id,
        target,
        recipient,
        source_proxy,
        status_action,
        source_actions.clone(),
    );
    xwm.data_bridge.dnd.active.as_mut().unwrap().target_version =
        Some(XwaylandDndVersion::new(target_version).unwrap());
    xwm.flush().unwrap();
    let _position = read_peer(peer);
    inject_status_message(xwm, peer, source_proxy, target, true, status_action);
    let _status_feedback = xwm.data_bridge.dnd.take_feedback();
    apply_transitions(
        xwm,
        [XwaylandDndTransition::DropRequested {
            session_id: id.session_id(),
            target,
            action: status_action,
            mime_types: catalog(&["text/plain"]),
            source_actions,
        }],
        100,
    )
    .unwrap();
    xwm.flush().unwrap();
    let messages = wire_client_messages(&read_peer(peer));
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].destination, recipient);
    assert_eq!(messages[0].window, target.xid());
    assert_eq!(messages[0].type_atom, xwm.atoms.get(XwmAtomName::XdndDrop));
    assert_eq!(messages[0].data, [source_proxy, 0, 1234, 0, 0]);
}

fn finished_message(
    xwm: &Xwm,
    source_proxy: u32,
    target: X11WindowHandle,
    accepted: bool,
    action: Option<XwaylandDndAction>,
) -> xproto::ClientMessageEvent {
    xproto::ClientMessageEvent {
        response_type: xproto::CLIENT_MESSAGE_EVENT,
        format: 32,
        sequence: 0,
        window: source_proxy,
        type_: xwm.atoms.get(XwmAtomName::XdndFinished),
        data: xproto::ClientMessageData::from([
            target.xid(),
            if accepted {
                super::super::dnd_wire::XDND_FINISHED_ACCEPTED
            } else {
                0
            },
            action.map_or(x11rb::NONE, |action| match action {
                XwaylandDndAction::Copy => xwm.atoms.get(XwmAtomName::XdndActionCopy),
                XwaylandDndAction::Move => xwm.atoms.get(XwmAtomName::XdndActionMove),
                XwaylandDndAction::Ask => xwm.atoms.get(XwmAtomName::XdndActionAsk),
                XwaylandDndAction::Link => xwm.atoms.get(XwmAtomName::XdndActionLink),
                XwaylandDndAction::Private => xwm.atoms.get(XwmAtomName::XdndActionPrivate),
            }),
            0,
            0,
        ]),
    }
}

fn request_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

#[derive(Debug)]
struct WireClientMessage {
    destination: u32,
    window: u32,
    type_atom: u32,
    data: [u32; 5],
}

#[derive(Debug)]
struct WirePropertyChange {
    property: u32,
    property_type: u32,
    format: u8,
    value: Vec<u8>,
}

#[derive(Debug)]
struct WireSelectionNotify {
    time: u32,
    requestor: u32,
    selection: u32,
    target: u32,
    property: u32,
}

#[derive(Debug, Default)]
struct WireRequests {
    property_changes: Vec<WirePropertyChange>,
    selection_notifies: Vec<WireSelectionNotify>,
}

fn decode_wire_requests(bytes: &[u8]) -> WireRequests {
    let mut decoded = WireRequests::default();
    let mut offset = 0;
    while offset < bytes.len() {
        let words = u16::from_ne_bytes([bytes[offset + 2], bytes[offset + 3]]) as usize;
        let request_len = words * 4;
        assert!(request_len >= 4 && offset + request_len <= bytes.len());
        let request = &bytes[offset..offset + request_len];
        if request[0] == xproto::CHANGE_PROPERTY_REQUEST {
            let format = request[16];
            let value_len = request_u32(request, 20) as usize * usize::from(format / 8);
            decoded.property_changes.push(WirePropertyChange {
                property: request_u32(request, 8),
                property_type: request_u32(request, 12),
                format,
                value: request[24..24 + value_len].to_vec(),
            });
        } else if request[0] == xproto::SEND_EVENT_REQUEST {
            let event = &request[12..44];
            if event[0] & 0x7f == xproto::SELECTION_NOTIFY_EVENT {
                decoded.selection_notifies.push(WireSelectionNotify {
                    time: request_u32(event, 4),
                    requestor: request_u32(event, 8),
                    selection: request_u32(event, 12),
                    target: request_u32(event, 16),
                    property: request_u32(event, 20),
                });
            }
        }
        offset += request_len;
    }
    decoded
}

fn wire_opcodes(bytes: &[u8]) -> Vec<u8> {
    let mut opcodes = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        let words = u16::from_ne_bytes([bytes[offset + 2], bytes[offset + 3]]) as usize;
        let request_len = words * 4;
        assert!(request_len >= 4 && offset + request_len <= bytes.len());
        opcodes.push(bytes[offset]);
        offset += request_len;
    }
    opcodes
}

fn wire_client_messages(bytes: &[u8]) -> Vec<WireClientMessage> {
    let mut messages = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        let words = u16::from_ne_bytes([bytes[offset + 2], bytes[offset + 3]]) as usize;
        let request_len = words * 4;
        assert!(request_len >= 4 && offset + request_len <= bytes.len());
        let request = &bytes[offset..offset + request_len];
        if request[0] == xproto::SEND_EVENT_REQUEST {
            let event = &request[12..44];
            if event[0] & 0x7f == xproto::CLIENT_MESSAGE_EVENT {
                let event_data = &event[12..32];
                messages.push(WireClientMessage {
                    destination: request_u32(request, 4),
                    window: request_u32(event, 4),
                    type_atom: request_u32(event, 8),
                    data: std::array::from_fn(|index| request_u32(event_data, index * 4)),
                });
            }
        }
        offset += request_len;
    }
    messages
}

fn inject_client_message(
    xwm: &mut Xwm,
    peer: &mut std::os::unix::net::UnixStream,
    event: xproto::ClientMessageEvent,
) {
    let mut bytes = [0u8; 32];
    bytes[0] = event.response_type;
    bytes[1] = event.format;
    bytes[2..4].copy_from_slice(&event.sequence.to_ne_bytes());
    bytes[4..8].copy_from_slice(&event.window.to_ne_bytes());
    bytes[8..12].copy_from_slice(&event.type_.to_ne_bytes());
    for (index, value) in event.data.as_data32().into_iter().enumerate() {
        bytes[12 + index * 4..16 + index * 4].copy_from_slice(&value.to_ne_bytes());
    }
    peer.write_all(&bytes).expect("inject X ClientMessage");
    assert_eq!(xwm.drain_events(1).unwrap().events_processed, 1);
}

fn inject_status_message(
    xwm: &mut Xwm,
    peer: &mut std::os::unix::net::UnixStream,
    source_proxy: u32,
    target: X11WindowHandle,
    accepted: bool,
    action: XwaylandDndAction,
) {
    let event = status_message(xwm, source_proxy, target, accepted, action);
    inject_client_message(xwm, peer, event);
}

fn inject_finished_message(
    xwm: &mut Xwm,
    peer: &mut std::os::unix::net::UnixStream,
    source_proxy: u32,
    target: X11WindowHandle,
    accepted: bool,
    action: Option<XwaylandDndAction>,
) {
    let event = finished_message(xwm, source_proxy, target, accepted, action);
    inject_client_message(xwm, peer, event);
}

fn inject_selection_request(
    xwm: &mut Xwm,
    peer: &mut std::os::unix::net::UnixStream,
    event: xproto::SelectionRequestEvent,
) {
    let mut bytes = [0u8; 32];
    bytes[0] = event.response_type;
    bytes[2..4].copy_from_slice(&event.sequence.to_ne_bytes());
    bytes[4..8].copy_from_slice(&event.time.to_ne_bytes());
    bytes[8..12].copy_from_slice(&event.owner.to_ne_bytes());
    bytes[12..16].copy_from_slice(&event.requestor.to_ne_bytes());
    bytes[16..20].copy_from_slice(&event.selection.to_ne_bytes());
    bytes[20..24].copy_from_slice(&event.target.to_ne_bytes());
    bytes[24..28].copy_from_slice(&event.property.to_ne_bytes());
    peer.write_all(&bytes).expect("inject X SelectionRequest");
    assert!(xwm.drain_events(32).unwrap().events_processed >= 1);
}

fn read_peer(peer: &mut std::os::unix::net::UnixStream) -> Vec<u8> {
    use std::io::Read;

    peer.set_nonblocking(true).unwrap();
    let mut output = Vec::new();
    let mut buffer = [0u8; 1024];
    loop {
        match peer.read(&mut buffer) {
            Ok(0) => break,
            Ok(length) => output.extend_from_slice(&buffer[..length]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) => panic!("read peer X11 requests: {error}"),
        }
    }
    output
}

#[test]
fn source_proxy_is_internal_unique_per_session_and_never_adopted() {
    let generation = XwaylandGeneration::new(NonZeroU64::new(71).unwrap());
    let (mut xwm, _peer) = super::super::super::test_fixture_for_tests(generation);
    let first = identity(generation, 9001);
    install_session(&mut xwm, first);
    create_source_proxy(&mut xwm, first, 10).unwrap();
    let first_proxy = xwm
        .data_bridge
        .dnd
        .active_session()
        .unwrap()
        .source_proxy
        .unwrap();
    assert!(xwm.data_bridge.dnd.internal_windows.contains(&first_proxy));
    assert!(
        !xwm.windows
            .contains(X11WindowHandle::new(generation, first_proxy))
    );

    retire_hover_session_with_leave(&mut xwm, first).unwrap();
    let second = identity(generation, 9002);
    install_session(&mut xwm, second);
    create_source_proxy(&mut xwm, second, 20).unwrap();
    let second_proxy = xwm
        .data_bridge
        .dnd
        .active_session()
        .unwrap()
        .source_proxy
        .unwrap();
    assert_ne!(first_proxy, second_proxy);
    assert!(!xwm.data_bridge.dnd.internal_windows.contains(&first_proxy));
    assert!(!destroy_notify(&mut xwm, first_proxy).unwrap());
    assert!(!is_internal_window(&xwm, first_proxy));
    assert!(is_internal_window(&xwm, second_proxy));
    assert!(
        !xwm.windows
            .contains(X11WindowHandle::new(generation, second_proxy))
    );
}

#[test]
fn target_discovery_keeps_one_exact_current_chain_and_waits_before_enter() {
    let generation = XwaylandGeneration::new(NonZeroU64::new(72).unwrap());
    let (mut xwm, _peer) = super::super::super::test_fixture_for_tests(generation);
    let id = identity(generation, 9003);
    let target_a = X11WindowHandle::new(generation, 0x440);
    let target_b = X11WindowHandle::new(generation, 0x441);
    let mime_types = catalog(&["text/plain"]);
    let actions = vec![XwaylandDndAction::Copy, XwaylandDndAction::Move];

    apply_transitions(
        &mut xwm,
        [XwaylandDndTransition::TargetEntered {
            session_id: id.session_id(),
            target: target_a,
            x: 12.5,
            y: 14.0,
            mime_types: mime_types.clone(),
            source_actions: actions.clone(),
        }],
        100,
    )
    .unwrap();
    let source_proxy = xwm
        .data_bridge
        .dnd
        .active_session()
        .unwrap()
        .source_proxy
        .unwrap();
    assert_eq!(
        xwm.data_bridge
            .dnd
            .active_session()
            .unwrap()
            .discovery_target,
        Some(target_a)
    );
    assert_eq!(
        xwm.data_bridge.dnd.active_session().unwrap().progress,
        DndWireProgress::AwaitingEnter
    );
    assert!(xwm.data_bridge.dnd.internal_windows.contains(&source_proxy));
    assert!(
        !xwm.windows
            .contains(X11WindowHandle::new(generation, source_proxy))
    );
    assert_eq!(
            xwm.data_bridge
                .dnd
                .pending_replies
                .values()
                .filter(|reply| matches!(reply, DndPendingReply::TargetProxy { actual, .. } if *actual == target_a))
                .count(),
            1
        );

    apply_transitions(
        &mut xwm,
        [XwaylandDndTransition::TargetEntered {
            session_id: id.session_id(),
            target: target_b,
            x: 15.0,
            y: 16.0,
            mime_types,
            source_actions: actions,
        }],
        101,
    )
    .unwrap();
    let target_queries = xwm
        .data_bridge
        .dnd
        .pending_replies
        .values()
        .filter_map(|reply| match reply {
            DndPendingReply::TargetProxy { actual, .. } => Some(*actual),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(target_queries, [target_b]);
    assert_eq!(
        xwm.data_bridge
            .dnd
            .active_session()
            .unwrap()
            .latest_position,
        Some(CoalescedPosition {
            x: 15.0,
            y: 16.0,
            action: Some(XwaylandDndAction::Copy),
        })
    );
}

#[test]
fn ready_target_receives_enter_then_initial_position() {
    let generation = XwaylandGeneration::new(NonZeroU64::new(73).unwrap());
    let (mut xwm, mut peer) = super::super::super::test_fixture_for_tests(generation);
    let id = identity(generation, 9004);
    let actual = X11WindowHandle::new(generation, 0x550);
    let recipient = 0x551;
    install_session(&mut xwm, id);
    assert!(xwm.data_bridge.dnd.bind_source_proxy(id, 0x880));
    assert!(
        xwm.data_bridge
            .dnd
            .confirm_source_ownership(id, 0x880, 1234)
    );
    assert!(
        xwm.data_bridge.dnd.active.as_mut().unwrap().mime_atoms[0]
            .replace(77)
            .is_none()
    );
    assert!(xwm.data_bridge.dnd.set_discovered_target(
        id,
        actual,
        recipient,
        XwaylandDndVersion::new(5).unwrap(),
    ));
    xwm.data_bridge.dnd.active.as_mut().unwrap().latest_position = Some(CoalescedPosition {
        x: 10.5,
        y: 20.0,
        action: Some(XwaylandDndAction::Copy),
    });

    maybe_send_enter(&mut xwm, id, 200).unwrap();
    let session = xwm.data_bridge.dnd.active_session().unwrap();
    assert_eq!(session.progress, DndWireProgress::AwaitingStatus);
    assert_eq!(
        session.status_deadline_ns,
        Some(200 + TARGET_STATUS_TIMEOUT_NS)
    );

    xwm.flush().unwrap();
    peer.set_nonblocking(true).unwrap();
    let mut requests = Vec::new();
    let mut buffer = [0u8; 256];
    loop {
        match peer.read(&mut buffer) {
            Ok(0) => break,
            Ok(bytes_read) => requests.extend_from_slice(&buffer[..bytes_read]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) => panic!("read encoded XDND messages: {error}"),
        }
    }
    assert_eq!(requests.len(), 88);
    assert_eq!(requests[0], 25, "first request is SendEvent");
    assert_eq!(requests[44], 25, "second request is SendEvent");
    assert_eq!(
        request_u32(&requests, 20),
        xwm.atoms.get(XwmAtomName::XdndEnter)
    );
    assert_eq!(
        request_u32(&requests, 64),
        xwm.atoms.get(XwmAtomName::XdndPosition)
    );
    assert_eq!(request_u32(&requests, 24), 0x880);
    assert_eq!(request_u32(&requests, 68), 0x880);
}

#[test]
fn status_requires_the_exact_outstanding_position_and_never_carries_mime() {
    let generation = XwaylandGeneration::new(NonZeroU64::new(74).unwrap());
    let (mut xwm, _peer) = super::super::super::test_fixture_for_tests(generation);
    let id = identity(generation, 9005);
    let target = X11WindowHandle::new(generation, 0x660);
    let proxy = 0x661;
    install_session(&mut xwm, id);
    assert!(xwm.data_bridge.dnd.bind_source_proxy(id, 0x880));
    xwm.data_bridge.dnd.internal_windows.insert(0x880);
    assert!(
        xwm.data_bridge
            .dnd
            .confirm_source_ownership(id, 0x880, 1234)
    );
    assert!(xwm.data_bridge.dnd.set_discovered_target(
        id,
        target,
        proxy,
        XwaylandDndVersion::new(5).unwrap(),
    ));
    assert!(xwm.data_bridge.dnd.mark_entered(id));
    assert!(
        xwm.data_bridge
            .dnd
            .position(id, target, 1, 2, Some(XwaylandDndAction::Copy))
    );
    let outstanding = CoalescedPosition {
        x: 1.0,
        y: 2.0,
        action: Some(XwaylandDndAction::Copy),
    };
    assert_eq!(
        xwm.data_bridge.dnd.queue_position(id, target, outstanding),
        PositionDisposition::SendNow(outstanding)
    );

    let mut status = xproto::ClientMessageEvent {
        response_type: xproto::CLIENT_MESSAGE_EVENT,
        format: 32,
        sequence: 0,
        window: 0x880,
        type_: xwm.atoms.get(XwmAtomName::XdndStatus),
        data: xproto::ClientMessageData::from([
            target.xid(),
            super::super::dnd_wire::XDND_STATUS_ACCEPTED,
            0,
            0,
            xwm.atoms.get(XwmAtomName::XdndActionCopy),
        ]),
    };
    status.data = xproto::ClientMessageData::from([
        target.xid() + 1,
        super::super::dnd_wire::XDND_STATUS_ACCEPTED,
        0,
        0,
        xwm.atoms.get(XwmAtomName::XdndActionCopy),
    ]);
    assert!(client_message(&mut xwm, status, 300).unwrap());
    assert!(xwm.data_bridge.dnd.take_feedback().is_empty());
    assert!(
        xwm.data_bridge.dnd.active_session().unwrap().progress == DndWireProgress::AwaitingStatus
    );

    status.data = xproto::ClientMessageData::from([
        target.xid(),
        super::super::dnd_wire::XDND_STATUS_ACCEPTED,
        0,
        0,
        xwm.atoms.get(XwmAtomName::XdndActionCopy),
    ]);
    assert!(client_message(&mut xwm, status, 301).unwrap());
    assert_eq!(
        xwm.data_bridge.dnd.take_feedback(),
        [DndFeedback::Status(DndStatusFeedback {
            id,
            target,
            accepted: true,
            action: Some(XwaylandDndAction::Copy),
        })]
    );
    assert_eq!(
        xwm.data_bridge.dnd.active_session().unwrap().progress,
        DndWireProgress::Positioned
    );
}

#[test]
fn accepted_physical_drop_sends_xdnd_drop_without_leave() {
    let generation = XwaylandGeneration::new(NonZeroU64::new(780).unwrap());
    let (mut xwm, mut peer) = super::super::super::test_fixture_for_tests(generation);
    let id = identity(generation, 9780);
    let target = X11WindowHandle::new(generation, 0x780);
    let recipient = 0x781;
    let source_proxy = 0x880;
    install_session(&mut xwm, id);
    xwm.data_bridge
        .dnd_outgoing
        .initialize_generation(generation);
    assert!(xwm.data_bridge.dnd.bind_source_proxy(id, source_proxy));
    xwm.data_bridge.dnd.internal_windows.insert(source_proxy);
    assert!(
        xwm.data_bridge
            .dnd
            .confirm_source_ownership(id, source_proxy, 1234)
    );
    assert!(xwm.data_bridge.dnd.set_discovered_target(
        id,
        target,
        recipient,
        XwaylandDndVersion::new(5).unwrap(),
    ));
    assert!(xwm.data_bridge.dnd.mark_entered(id));
    xwm.data_bridge.dnd.active.as_mut().unwrap().mime_atoms[0] = Some(99);
    assert!(
        xwm.data_bridge
            .dnd
            .position(id, target, 5, 6, Some(XwaylandDndAction::Copy),)
    );
    let position = CoalescedPosition {
        x: 5.0,
        y: 6.0,
        action: Some(XwaylandDndAction::Copy),
    };
    assert_eq!(
        xwm.data_bridge.dnd.queue_position(id, target, position),
        PositionDisposition::SendNow(position)
    );

    inject_status_message(
        &mut xwm,
        &mut peer,
        source_proxy,
        target,
        true,
        XwaylandDndAction::Copy,
    );
    xwm.data_bridge.dnd.take_feedback();

    apply_transitions(
        &mut xwm,
        [drop_transition(id, target, XwaylandDndAction::Copy)],
        200,
    )
    .unwrap();
    xwm.flush().unwrap();
    let drop_wire = wire_client_messages(&read_peer(&mut peer));
    assert_eq!(drop_wire.len(), 1, "one ClientMessage is sent");
    assert_eq!(drop_wire[0].destination, recipient);
    assert_eq!(drop_wire[0].window, target.xid());
    assert_eq!(drop_wire[0].type_atom, xwm.atoms.get(XwmAtomName::XdndDrop));
    assert_eq!(drop_wire[0].data, [source_proxy, 0, 1234, 0, 0]);
    let session = xwm.data_bridge.dnd.active_session().unwrap();
    assert_eq!(session.progress, DndWireProgress::AwaitingFinished);
    assert_eq!(session.source_proxy, Some(source_proxy));
    assert!(session.ownership_confirmed);
    assert_eq!(
        session.authorized_drop_action,
        Some(XwaylandDndAction::Copy)
    );
    assert!(session.finished_deadline_ns.is_some());
    apply_transitions(
        &mut xwm,
        [
            XwaylandDndTransition::TargetPositioned {
                session_id: id.session_id(),
                target,
                x: 90.0,
                y: 91.0,
                action: Some(XwaylandDndAction::Move),
                mime_types: catalog(&["text/plain"]),
                source_actions: vec![XwaylandDndAction::Copy, XwaylandDndAction::Move],
            },
            XwaylandDndTransition::TargetLeft {
                session_id: id.session_id(),
                target,
            },
        ],
        200,
    )
    .unwrap();
    xwm.flush().unwrap();
    assert!(wire_client_messages(&read_peer(&mut peer)).is_empty());
    assert_eq!(
        xwm.data_bridge.dnd.active_session().unwrap().target,
        Some(target)
    );
    assert_eq!(
        xwm.data_bridge.dnd.active_session().unwrap().progress,
        DndWireProgress::AwaitingFinished
    );

    let request_time = 1235;
    let selection_atom = xwm.atoms.get(XwmAtomName::XdndSelection);
    inject_selection_request(
        &mut xwm,
        &mut peer,
        xproto::SelectionRequestEvent {
            response_type: xproto::SELECTION_REQUEST_EVENT,
            sequence: 0,
            time: request_time,
            owner: source_proxy,
            requestor: target.xid(),
            selection: selection_atom,
            target: 99,
            property: 0xa80,
        },
    );
    let mut source_requests = super::super::super::dnd_outgoing::take_requests(&mut xwm);
    assert_eq!(source_requests.len(), 1);
    let source_request = source_requests.pop().unwrap();
    assert_eq!(source_request.transfer_id.source.adapter_id, id);
    assert_eq!(source_request.transfer_id.source.xid, source_proxy);
    assert_eq!(source_request.target, target);
    assert_eq!(source_request.requestor, target.xid());
    assert_eq!(source_request.mime_type, "text/plain");
    let transfer_id = source_request.transfer_id;
    let payload = b"fake peer payload";
    assert_eq!(
        unsafe {
            libc::write(
                std::os::fd::AsRawFd::as_raw_fd(&source_request.sink),
                payload.as_ptr().cast(),
                payload.len(),
            )
        },
        payload.len() as isize
    );
    drop(source_request);
    assert!(
        super::super::super::dnd_outgoing::resolve_requests(&mut xwm, [(transfer_id, true)], 201,)
            .unwrap()
    );
    xwm.flush().unwrap();
    let data_wire = decode_wire_requests(&read_peer(&mut peer));
    let property = data_wire
        .property_changes
        .iter()
        .find(|change| change.property == 0xa80)
        .expect("post-Drop direct MIME payload");
    assert_eq!(property.property_type, 99);
    assert_eq!(property.format, 8);
    assert_eq!(property.value, payload);
    assert_eq!(data_wire.selection_notifies.len(), 1);
    assert_eq!(data_wire.selection_notifies[0].time, request_time);
    assert_eq!(data_wire.selection_notifies[0].requestor, target.xid());
    assert_eq!(
        data_wire.selection_notifies[0].selection,
        xwm.atoms.get(XwmAtomName::XdndSelection)
    );
    assert_eq!(data_wire.selection_notifies[0].target, 99);
    assert_eq!(data_wire.selection_notifies[0].property, 0xa80);

    let finished = finished_message(
        &xwm,
        source_proxy,
        target,
        true,
        Some(XwaylandDndAction::Copy),
    );
    inject_client_message(&mut xwm, &mut peer, finished);
    assert_eq!(
        xwm.take_dnd_feedback(),
        [crate::xwayland::XwaylandDndFeedback::Terminal {
            session_id: id.session_id(),
            target,
            accepted: true,
            action: Some(XwaylandDndAction::Copy),
        }]
    );
    assert!(xwm.data_bridge.dnd.terminal_event_consumed(id));
    inject_client_message(&mut xwm, &mut peer, finished);
    assert!(
        xwm.take_dnd_feedback().is_empty(),
        "duplicate Finished is inert"
    );

    apply_transitions(
        &mut xwm,
        [XwaylandDndTransition::TargetFinished {
            session_id: id.session_id(),
            target,
            accepted: true,
            action: Some(XwaylandDndAction::Copy),
        }],
        202,
    )
    .unwrap();
    xwm.flush().unwrap();
    let retirement = read_peer(&mut peer);
    let retirement_opcodes = wire_opcodes(&retirement);
    assert!(retirement_opcodes.contains(&xproto::DESTROY_WINDOW_REQUEST));
    assert!(
        !retirement_opcodes.contains(&xproto::SET_SELECTION_OWNER_REQUEST),
        "selection ownership is released by window destruction"
    );
    assert!(
        wire_client_messages(&retirement)
            .iter()
            .all(|message| message.type_atom != xwm.atoms.get(XwmAtomName::XdndLeave))
    );
    assert_eq!(xwm.data_bridge.dnd.active_id(), None);
    assert!(!xwm.data_bridge.dnd.internal_windows.contains(&source_proxy));
    assert_eq!(
        super::super::super::dnd_outgoing::transfer_count_for_test(&xwm),
        0
    );
    assert_eq!(
        super::super::super::dnd_outgoing::multiple_group_count_for_test(&xwm),
        0
    );
    assert!(xwm.data_bridge.dnd.pending_replies.is_empty());
    assert!(xwm.data_bridge.dnd.next_deadline_ns().is_none());
    assert!(super::super::super::dnd_outgoing::source_interests(&xwm).is_empty());

    let selection_atom = xwm.atoms.get(XwmAtomName::XdndSelection);
    inject_selection_request(
        &mut xwm,
        &mut peer,
        xproto::SelectionRequestEvent {
            response_type: xproto::SELECTION_REQUEST_EVENT,
            sequence: 0,
            time: request_time,
            owner: source_proxy,
            requestor: target.xid(),
            selection: selection_atom,
            target: 99,
            property: 0xa81,
        },
    );
    assert!(super::super::super::dnd_outgoing::take_requests(&mut xwm).is_empty());
    xwm.flush().unwrap();
    let late_request = decode_wire_requests(&read_peer(&mut peer));
    assert_eq!(late_request.selection_notifies.len(), 1);
    assert_eq!(late_request.selection_notifies[0].property, x11rb::NONE);
}

#[test]
fn physical_drop_waits_for_the_outstanding_position_status() {
    let generation = XwaylandGeneration::new(NonZeroU64::new(781).unwrap());
    let (mut xwm, mut peer) = super::super::super::test_fixture_for_tests(generation);
    let id = identity(generation, 9781);
    let target = X11WindowHandle::new(generation, 0x782);
    let recipient = 0x783;
    let source_proxy = 0x884;
    install_waiting_position(
        &mut xwm,
        id,
        target,
        recipient,
        source_proxy,
        XwaylandDndAction::Copy,
    );
    xwm.flush().unwrap();
    let initial_position = wire_client_messages(&read_peer(&mut peer));
    assert_eq!(initial_position.len(), 1);
    assert_eq!(
        initial_position[0].type_atom,
        xwm.atoms.get(XwmAtomName::XdndPosition)
    );

    apply_transitions(
        &mut xwm,
        [drop_transition(id, target, XwaylandDndAction::Copy)],
        100,
    )
    .unwrap();
    xwm.flush().unwrap();
    assert!(wire_client_messages(&read_peer(&mut peer)).is_empty());
    assert_eq!(
        xwm.data_bridge.dnd.active_session().unwrap().progress,
        DndWireProgress::DropPendingAwaitingStatus
    );

    inject_status_message(
        &mut xwm,
        &mut peer,
        source_proxy,
        target,
        true,
        XwaylandDndAction::Copy,
    );
    xwm.flush().unwrap();
    let messages = wire_client_messages(&read_peer(&mut peer));
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].destination, recipient);
    assert_eq!(messages[0].window, target.xid());
    assert_eq!(messages[0].type_atom, xwm.atoms.get(XwmAtomName::XdndDrop));
    assert_eq!(messages[0].data, [source_proxy, 0, 1234, 0, 0]);
    assert_eq!(
        xwm.data_bridge.dnd.active_session().unwrap().progress,
        DndWireProgress::AwaitingFinished
    );
}

#[test]
fn physical_drop_sends_only_the_preexisting_coalesced_position_before_drop() {
    let generation = XwaylandGeneration::new(NonZeroU64::new(782).unwrap());
    let (mut xwm, mut peer) = super::super::super::test_fixture_for_tests(generation);
    let id = identity(generation, 9782);
    let target = X11WindowHandle::new(generation, 0x784);
    let recipient = 0x785;
    let source_proxy = 0x885;
    install_waiting_position(
        &mut xwm,
        id,
        target,
        recipient,
        source_proxy,
        XwaylandDndAction::Copy,
    );
    xwm.flush().unwrap();
    let _initial = read_peer(&mut peer);
    let latest = CoalescedPosition {
        x: 52.0,
        y: 61.0,
        action: Some(XwaylandDndAction::Copy),
    };
    assert_eq!(
        xwm.data_bridge.dnd.queue_position(id, target, latest),
        PositionDisposition::Coalesced
    );
    apply_transitions(
        &mut xwm,
        [drop_transition(id, target, XwaylandDndAction::Copy)],
        100,
    )
    .unwrap();
    xwm.flush().unwrap();
    assert!(wire_client_messages(&read_peer(&mut peer)).is_empty());

    inject_status_message(
        &mut xwm,
        &mut peer,
        source_proxy,
        target,
        true,
        XwaylandDndAction::Copy,
    );
    xwm.flush().unwrap();
    let released = wire_client_messages(&read_peer(&mut peer));
    assert_eq!(released.len(), 1);
    assert_eq!(released[0].destination, recipient);
    assert_eq!(released[0].window, target.xid());
    assert_eq!(
        released[0].type_atom,
        xwm.atoms.get(XwmAtomName::XdndPosition)
    );
    assert_eq!(
        released[0].data[2],
        super::super::dnd_wire::pack_root_coordinates(52.0, 61.0).unwrap()
    );
    assert_eq!(
        xwm.data_bridge.dnd.active_session().unwrap().progress,
        DndWireProgress::DropPendingAwaitingStatus
    );

    inject_status_message(
        &mut xwm,
        &mut peer,
        source_proxy,
        target,
        true,
        XwaylandDndAction::Copy,
    );
    xwm.flush().unwrap();
    let drop = wire_client_messages(&read_peer(&mut peer));
    assert_eq!(drop.len(), 1);
    assert_eq!(drop[0].destination, recipient);
    assert_eq!(drop[0].window, target.xid());
    assert_eq!(drop[0].type_atom, xwm.atoms.get(XwmAtomName::XdndDrop));
    assert_eq!(drop[0].data, [source_proxy, 0, 1234, 0, 0]);
}

#[test]
fn rejected_final_status_leaves_and_queues_ordered_terminal_cancellation() {
    let generation = XwaylandGeneration::new(NonZeroU64::new(783).unwrap());
    let (mut xwm, mut peer) = super::super::super::test_fixture_for_tests(generation);
    let id = identity(generation, 9783);
    let target = X11WindowHandle::new(generation, 0x786);
    let source_proxy = 0x886;
    install_waiting_position(
        &mut xwm,
        id,
        target,
        0x787,
        source_proxy,
        XwaylandDndAction::Copy,
    );
    xwm.flush().unwrap();
    let _initial = read_peer(&mut peer);
    apply_transitions(
        &mut xwm,
        [drop_transition(id, target, XwaylandDndAction::Copy)],
        100,
    )
    .unwrap();
    inject_status_message(
        &mut xwm,
        &mut peer,
        source_proxy,
        target,
        false,
        XwaylandDndAction::Copy,
    );
    xwm.flush().unwrap();
    let messages = wire_client_messages(&read_peer(&mut peer));
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].type_atom, xwm.atoms.get(XwmAtomName::XdndLeave));
    assert_eq!(messages[0].window, target.xid());
    assert_eq!(
        xwm.data_bridge.dnd.take_feedback(),
        [
            DndFeedback::Status(DndStatusFeedback {
                id,
                target,
                accepted: false,
                action: None,
            }),
            DndFeedback::Terminal(DndTerminalFeedback {
                id,
                target,
                accepted: false,
                action: None,
            }),
        ]
    );
    assert_eq!(
        xwm.data_bridge.dnd.active_session().unwrap().progress,
        DndWireProgress::TerminalConsumed
    );
}

#[test]
fn pending_drop_status_timeout_leaves_and_retires_after_canonical_ack() {
    let generation = XwaylandGeneration::new(NonZeroU64::new(784).unwrap());
    let (mut xwm, mut peer) = super::super::super::test_fixture_for_tests(generation);
    let id = identity(generation, 9784);
    let target = X11WindowHandle::new(generation, 0x788);
    let source_proxy = 0x887;
    install_waiting_position(
        &mut xwm,
        id,
        target,
        0x789,
        source_proxy,
        XwaylandDndAction::Copy,
    );
    xwm.flush().unwrap();
    let _initial = read_peer(&mut peer);
    apply_transitions(
        &mut xwm,
        [drop_transition(id, target, XwaylandDndAction::Copy)],
        100,
    )
    .unwrap();
    handle_deadline(&mut xwm, TARGET_STATUS_TIMEOUT_NS + 1).unwrap();
    xwm.flush().unwrap();
    let messages = wire_client_messages(&read_peer(&mut peer));
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].type_atom, xwm.atoms.get(XwmAtomName::XdndLeave));
    assert_eq!(
        xwm.take_dnd_feedback(),
        [crate::xwayland::XwaylandDndFeedback::Terminal {
            session_id: id.session_id(),
            target,
            accepted: false,
            action: None,
        }]
    );
    assert_eq!(
        xwm.data_bridge.dnd.active_session().unwrap().progress,
        DndWireProgress::TerminalConsumed
    );

    apply_transitions(
        &mut xwm,
        [XwaylandDndTransition::TargetFinished {
            session_id: id.session_id(),
            target,
            accepted: false,
            action: None,
        }],
        TARGET_STATUS_TIMEOUT_NS + 2,
    )
    .unwrap();
    assert_eq!(xwm.data_bridge.dnd.active_id(), None);
    assert!(xwm.data_bridge.dnd.pending_replies.is_empty());
}

#[test]
fn successful_move_finished_matches_the_status_authorizing_drop() {
    let generation = XwaylandGeneration::new(NonZeroU64::new(785).unwrap());
    let (mut xwm, mut peer) = super::super::super::test_fixture_for_tests(generation);
    let id = identity(generation, 9785);
    let target = X11WindowHandle::new(generation, 0x78a);
    let recipient = 0x78b;
    let source_proxy = 0x888;
    send_accepted_drop(
        &mut xwm,
        &mut peer,
        id,
        target,
        recipient,
        source_proxy,
        5,
        XwaylandDndAction::Move,
        vec![XwaylandDndAction::Copy, XwaylandDndAction::Move],
    );
    inject_finished_message(
        &mut xwm,
        &mut peer,
        source_proxy,
        target,
        true,
        Some(XwaylandDndAction::Move),
    );
    assert_eq!(
        xwm.take_dnd_feedback(),
        [crate::xwayland::XwaylandDndFeedback::Terminal {
            session_id: id.session_id(),
            target,
            accepted: true,
            action: Some(XwaylandDndAction::Move),
        }]
    );
}

#[test]
fn mismatched_v5_finished_action_fails_closed_and_stale_finished_is_inert() {
    let generation = XwaylandGeneration::new(NonZeroU64::new(786).unwrap());
    let (mut xwm, mut peer) = super::super::super::test_fixture_for_tests(generation);
    let id = identity(generation, 9786);
    let target = X11WindowHandle::new(generation, 0x78c);
    let recipient = 0x78d;
    let source_proxy = 0x889;
    send_accepted_drop(
        &mut xwm,
        &mut peer,
        id,
        target,
        recipient,
        source_proxy,
        5,
        XwaylandDndAction::Copy,
        vec![XwaylandDndAction::Copy, XwaylandDndAction::Move],
    );

    let stale_proxy = finished_message(
        &xwm,
        source_proxy + 1,
        target,
        true,
        Some(XwaylandDndAction::Copy),
    );
    inject_client_message(&mut xwm, &mut peer, stale_proxy);
    let stale_target = finished_message(
        &xwm,
        source_proxy,
        X11WindowHandle::new(generation, target.xid() + 1),
        true,
        Some(XwaylandDndAction::Copy),
    );
    inject_client_message(&mut xwm, &mut peer, stale_target);
    assert!(xwm.take_dnd_feedback().is_empty());
    assert_eq!(
        xwm.data_bridge.dnd.active_session().unwrap().progress,
        DndWireProgress::AwaitingFinished
    );

    inject_finished_message(
        &mut xwm,
        &mut peer,
        source_proxy,
        target,
        true,
        Some(XwaylandDndAction::Move),
    );
    assert_eq!(
        xwm.take_dnd_feedback(),
        [crate::xwayland::XwaylandDndFeedback::Terminal {
            session_id: id.session_id(),
            target,
            accepted: false,
            action: None,
        }]
    );
    assert_eq!(
        xwm.data_bridge.dnd.active_session().unwrap().progress,
        DndWireProgress::TerminalConsumed
    );
    assert!(wire_client_messages(&read_peer(&mut peer)).is_empty());
    inject_finished_message(
        &mut xwm,
        &mut peer,
        source_proxy,
        target,
        true,
        Some(XwaylandDndAction::Copy),
    );
    assert!(xwm.take_dnd_feedback().is_empty());
}

#[test]
fn v5_finished_rejection_ignores_a_non_none_performed_action() {
    let generation = XwaylandGeneration::new(NonZeroU64::new(795).unwrap());
    let (mut xwm, mut peer) = super::super::super::test_fixture_for_tests(generation);
    let id = identity(generation, 9795);
    let target = X11WindowHandle::new(generation, 0x798);
    let recipient = 0x799;
    let source_proxy = 0x894;
    send_accepted_drop(
        &mut xwm,
        &mut peer,
        id,
        target,
        recipient,
        source_proxy,
        5,
        XwaylandDndAction::Copy,
        vec![XwaylandDndAction::Copy, XwaylandDndAction::Move],
    );
    inject_finished_message(
        &mut xwm,
        &mut peer,
        source_proxy,
        target,
        false,
        Some(XwaylandDndAction::Move),
    );
    assert_eq!(
        xwm.take_dnd_feedback(),
        [crate::xwayland::XwaylandDndFeedback::Terminal {
            session_id: id.session_id(),
            target,
            accepted: false,
            action: None,
        }]
    );
    assert_eq!(
        xwm.data_bridge.dnd.active_session().unwrap().progress,
        DndWireProgress::TerminalConsumed
    );
}

#[test]
fn version_three_and_four_finished_use_last_accepted_status_action() {
    for (generation_value, session_value, version) in [(796, 9796, 3), (797, 9797, 4)] {
        let generation = XwaylandGeneration::new(NonZeroU64::new(generation_value).unwrap());
        let (mut xwm, mut peer) = super::super::super::test_fixture_for_tests(generation);
        let id = identity(generation, session_value);
        let target = X11WindowHandle::new(generation, generation_value as u32 + 0x10);
        let recipient = target.xid() + 1;
        let source_proxy = target.xid() + 2;
        send_accepted_drop(
            &mut xwm,
            &mut peer,
            id,
            target,
            recipient,
            source_proxy,
            version,
            XwaylandDndAction::Copy,
            vec![XwaylandDndAction::Copy, XwaylandDndAction::Move],
        );
        // Versions 3 and 4 do not define v5 success or performed-action data.
        inject_finished_message(
            &mut xwm,
            &mut peer,
            source_proxy,
            target,
            false,
            Some(XwaylandDndAction::Ask),
        );
        assert_eq!(
            xwm.take_dnd_feedback(),
            [crate::xwayland::XwaylandDndFeedback::Terminal {
                session_id: id.session_id(),
                target,
                accepted: true,
                action: Some(XwaylandDndAction::Copy),
            }]
        );
    }
}

#[test]
fn v5_ask_finished_resolves_to_supported_copy_or_move() {
    for (generation_value, session_value, final_action) in [
        (787, 9787, XwaylandDndAction::Copy),
        (788, 9788, XwaylandDndAction::Move),
    ] {
        let generation = XwaylandGeneration::new(NonZeroU64::new(generation_value).unwrap());
        let (mut xwm, mut peer) = super::super::super::test_fixture_for_tests(generation);
        let id = identity(generation, session_value);
        let target = X11WindowHandle::new(generation, generation_value as u32 + 0x10);
        let recipient = target.xid() + 1;
        let source_proxy = target.xid() + 2;
        let source_actions = vec![
            XwaylandDndAction::Copy,
            XwaylandDndAction::Move,
            XwaylandDndAction::Ask,
        ];
        send_accepted_drop(
            &mut xwm,
            &mut peer,
            id,
            target,
            recipient,
            source_proxy,
            5,
            XwaylandDndAction::Ask,
            source_actions,
        );
        inject_finished_message(
            &mut xwm,
            &mut peer,
            source_proxy,
            target,
            true,
            Some(final_action),
        );
        assert_eq!(
            xwm.take_dnd_feedback(),
            [crate::xwayland::XwaylandDndFeedback::Terminal {
                session_id: id.session_id(),
                target,
                accepted: true,
                action: Some(final_action),
            }]
        );
    }
}

#[test]
fn pre_v5_ask_is_rejected_before_drop_without_guessing_an_action() {
    let generation = XwaylandGeneration::new(NonZeroU64::new(789).unwrap());
    let (mut xwm, mut peer) = super::super::super::test_fixture_for_tests(generation);
    let id = identity(generation, 9789);
    let target = X11WindowHandle::new(generation, 0x790);
    let recipient = 0x791;
    let source_proxy = 0x890;
    let actions = vec![
        XwaylandDndAction::Copy,
        XwaylandDndAction::Move,
        XwaylandDndAction::Ask,
    ];
    install_waiting_position_with_actions(
        &mut xwm,
        id,
        target,
        recipient,
        source_proxy,
        XwaylandDndAction::Ask,
        actions.clone(),
    );
    xwm.data_bridge.dnd.active.as_mut().unwrap().target_version =
        Some(XwaylandDndVersion::new(4).unwrap());
    xwm.flush().unwrap();
    let _position = read_peer(&mut peer);
    apply_transitions(
        &mut xwm,
        [XwaylandDndTransition::DropRequested {
            session_id: id.session_id(),
            target,
            action: XwaylandDndAction::Ask,
            mime_types: catalog(&["text/plain"]),
            source_actions: actions,
        }],
        100,
    )
    .unwrap();
    inject_status_message(
        &mut xwm,
        &mut peer,
        source_proxy,
        target,
        true,
        XwaylandDndAction::Ask,
    );
    xwm.flush().unwrap();
    let messages = wire_client_messages(&read_peer(&mut peer));
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].type_atom, xwm.atoms.get(XwmAtomName::XdndLeave));
    assert_eq!(
        xwm.data_bridge.dnd.take_feedback(),
        [
            DndFeedback::Status(DndStatusFeedback {
                id,
                target,
                accepted: true,
                action: Some(XwaylandDndAction::Ask),
            }),
            DndFeedback::Terminal(DndTerminalFeedback {
                id,
                target,
                accepted: false,
                action: None,
            }),
        ]
    );
    assert_eq!(
        xwm.data_bridge.dnd.active_session().unwrap().progress,
        DndWireProgress::TerminalConsumed
    );
}

#[test]
fn finished_timeout_cancels_without_post_drop_leave_and_retires_exact_session() {
    let generation = XwaylandGeneration::new(NonZeroU64::new(790).unwrap());
    let (mut xwm, mut peer) = super::super::super::test_fixture_for_tests(generation);
    let id = identity(generation, 9790);
    let target = X11WindowHandle::new(generation, 0x792);
    let recipient = 0x793;
    let source_proxy = 0x891;
    send_accepted_drop(
        &mut xwm,
        &mut peer,
        id,
        target,
        recipient,
        source_proxy,
        5,
        XwaylandDndAction::Copy,
        vec![XwaylandDndAction::Copy, XwaylandDndAction::Move],
    );
    handle_deadline(&mut xwm, 100 + TARGET_FINISHED_TIMEOUT_NS).unwrap();
    assert_eq!(
        xwm.take_dnd_feedback(),
        [crate::xwayland::XwaylandDndFeedback::Terminal {
            session_id: id.session_id(),
            target,
            accepted: false,
            action: None,
        }]
    );
    assert!(wire_client_messages(&read_peer(&mut peer)).is_empty());
    assert_eq!(
        xwm.data_bridge.dnd.active_session().unwrap().progress,
        DndWireProgress::TerminalConsumed
    );
    apply_transitions(
        &mut xwm,
        [XwaylandDndTransition::TargetFinished {
            session_id: id.session_id(),
            target,
            accepted: false,
            action: None,
        }],
        100 + TARGET_FINISHED_TIMEOUT_NS + 1,
    )
    .unwrap();
    assert_eq!(xwm.data_bridge.dnd.active_id(), None);
    assert!(!xwm.data_bridge.dnd.internal_windows.contains(&source_proxy));
}

#[test]
fn selection_clear_while_awaiting_finished_rejects_without_post_drop_leave() {
    let generation = XwaylandGeneration::new(NonZeroU64::new(791).unwrap());
    let (mut xwm, mut peer) = super::super::super::test_fixture_for_tests(generation);
    let id = identity(generation, 9791);
    let target = X11WindowHandle::new(generation, 0x794);
    let recipient = 0x795;
    let source_proxy = 0x892;
    send_accepted_drop(
        &mut xwm,
        &mut peer,
        id,
        target,
        recipient,
        source_proxy,
        5,
        XwaylandDndAction::Copy,
        vec![XwaylandDndAction::Copy, XwaylandDndAction::Move],
    );
    let selection = xwm.atoms.get(XwmAtomName::XdndSelection);
    assert!(
        selection_clear(
            &mut xwm,
            xproto::SelectionClearEvent {
                response_type: xproto::SELECTION_CLEAR_EVENT,
                sequence: 0,
                time: 1236,
                owner: source_proxy,
                selection,
            },
            200,
        )
        .unwrap()
    );
    assert_eq!(
        xwm.take_dnd_feedback(),
        [crate::xwayland::XwaylandDndFeedback::Terminal {
            session_id: id.session_id(),
            target,
            accepted: false,
            action: None,
        }]
    );
    assert!(wire_client_messages(&read_peer(&mut peer)).is_empty());
}

#[test]
fn target_and_source_destruction_while_awaiting_finished_reject_exact_session() {
    for (generation_value, session_value, destroy_source) in [(792, 9792, false), (793, 9793, true)]
    {
        let generation = XwaylandGeneration::new(NonZeroU64::new(generation_value).unwrap());
        let (mut xwm, mut peer) = super::super::super::test_fixture_for_tests(generation);
        let id = identity(generation, session_value);
        let target = X11WindowHandle::new(generation, generation_value as u32 + 0x20);
        let recipient = target.xid() + 1;
        let source_proxy = target.xid() + 2;
        send_accepted_drop(
            &mut xwm,
            &mut peer,
            id,
            target,
            recipient,
            source_proxy,
            5,
            XwaylandDndAction::Copy,
            vec![XwaylandDndAction::Copy, XwaylandDndAction::Move],
        );
        let handled = destroy_notify(
            &mut xwm,
            if destroy_source {
                source_proxy
            } else {
                target.xid()
            },
        )
        .unwrap();
        assert_eq!(handled, destroy_source);
        assert_eq!(
            xwm.take_dnd_feedback(),
            [crate::xwayland::XwaylandDndFeedback::Terminal {
                session_id: id.session_id(),
                target,
                accepted: false,
                action: None,
            }]
        );
        assert!(wire_client_messages(&read_peer(&mut peer)).is_empty());
        apply_transitions(
            &mut xwm,
            [XwaylandDndTransition::TargetFinished {
                session_id: id.session_id(),
                target,
                accepted: false,
                action: None,
            }],
            300,
        )
        .unwrap();
        assert_eq!(xwm.data_bridge.dnd.active_id(), None);
        assert!(!xwm.data_bridge.dnd.internal_windows.contains(&source_proxy));
    }
}

#[test]
fn generation_retirement_discards_finished_authority_and_makes_late_message_inert() {
    let generation = XwaylandGeneration::new(NonZeroU64::new(794).unwrap());
    let (mut xwm, mut peer) = super::super::super::test_fixture_for_tests(generation);
    let id = identity(generation, 9794);
    let target = X11WindowHandle::new(generation, 0x796);
    let recipient = 0x797;
    let source_proxy = 0x893;
    send_accepted_drop(
        &mut xwm,
        &mut peer,
        id,
        target,
        recipient,
        source_proxy,
        5,
        XwaylandDndAction::Copy,
        vec![XwaylandDndAction::Copy, XwaylandDndAction::Move],
    );
    assert!(xwm.data_bridge.dnd.next_deadline_ns().is_some());
    retire_generation(&mut xwm, generation).unwrap();
    assert_eq!(xwm.data_bridge.dnd.active_id(), None);
    assert!(xwm.data_bridge.dnd.pending_replies.is_empty());
    assert!(xwm.data_bridge.dnd.feedback.is_empty());
    assert!(xwm.data_bridge.dnd.next_deadline_ns().is_none());
    inject_finished_message(
        &mut xwm,
        &mut peer,
        source_proxy,
        target,
        true,
        Some(XwaylandDndAction::Copy),
    );
    assert!(xwm.take_dnd_feedback().is_empty());
}

#[test]
fn status_timeout_leaves_target_and_preserves_drag_source_proxy() {
    let generation = XwaylandGeneration::new(NonZeroU64::new(75).unwrap());
    let (mut xwm, mut peer) = super::super::super::test_fixture_for_tests(generation);
    let id = identity(generation, 9006);
    let target = X11WindowHandle::new(generation, 0x670);
    let proxy = 0x671;
    install_session(&mut xwm, id);
    assert!(xwm.data_bridge.dnd.bind_source_proxy(id, 0x880));
    xwm.data_bridge.dnd.internal_windows.insert(0x880);
    assert!(
        xwm.data_bridge
            .dnd
            .confirm_source_ownership(id, 0x880, 1234)
    );
    assert!(xwm.data_bridge.dnd.set_discovered_target(
        id,
        target,
        proxy,
        XwaylandDndVersion::new(5).unwrap(),
    ));
    assert!(xwm.data_bridge.dnd.mark_entered(id));
    assert!(
        xwm.data_bridge
            .dnd
            .position(id, target, 1, 2, Some(XwaylandDndAction::Copy))
    );
    let outstanding = CoalescedPosition {
        x: 1.0,
        y: 2.0,
        action: Some(XwaylandDndAction::Copy),
    };
    assert_eq!(
        xwm.data_bridge.dnd.queue_position(id, target, outstanding),
        PositionDisposition::SendNow(outstanding)
    );
    xwm.data_bridge.dnd.set_status_deadline(id, 400);

    handle_deadline(&mut xwm, 400).unwrap();
    assert_eq!(
        xwm.data_bridge.dnd.take_feedback(),
        [DndFeedback::Status(DndStatusFeedback {
            id,
            target,
            accepted: false,
            action: None,
        })]
    );
    let session = xwm.data_bridge.dnd.active_session().unwrap();
    assert_eq!(session.source_proxy, Some(0x880));
    assert!(session.ownership_confirmed);
    assert_eq!(session.target, None);
    assert_ne!(session.progress, DndWireProgress::AwaitingStatus);

    xwm.flush().unwrap();
    peer.set_nonblocking(true).unwrap();
    let mut requests = [0u8; 64];
    let bytes_read = peer.read(&mut requests).unwrap();
    assert_eq!(bytes_read, 44);
    assert_eq!(requests[0], 25);
    assert_eq!(
        request_u32(&requests, 20),
        xwm.atoms.get(XwmAtomName::XdndLeave)
    );
}

#[test]
fn switching_targets_sends_leave_and_starts_independent_discovery() {
    let generation = XwaylandGeneration::new(NonZeroU64::new(76).unwrap());
    let (mut xwm, mut peer) = super::super::super::test_fixture_for_tests(generation);
    let id = identity(generation, 9007);
    let target_a = X11WindowHandle::new(generation, 0x680);
    let target_b = X11WindowHandle::new(generation, 0x681);
    install_session(&mut xwm, id);
    assert!(xwm.data_bridge.dnd.bind_source_proxy(id, 0x880));
    assert!(
        xwm.data_bridge
            .dnd
            .confirm_source_ownership(id, 0x880, 1234)
    );
    assert!(xwm.data_bridge.dnd.set_discovered_target(
        id,
        target_a,
        target_a.xid(),
        XwaylandDndVersion::new(5).unwrap(),
    ));
    assert!(xwm.data_bridge.dnd.mark_entered(id));
    assert!(
        xwm.data_bridge
            .dnd
            .position(id, target_a, 1, 2, Some(XwaylandDndAction::Copy))
    );
    let outstanding = CoalescedPosition {
        x: 1.0,
        y: 2.0,
        action: Some(XwaylandDndAction::Copy),
    };
    assert_eq!(
        xwm.data_bridge
            .dnd
            .queue_position(id, target_a, outstanding),
        PositionDisposition::SendNow(outstanding)
    );

    start_target_discovery(&mut xwm, id, target_b, 500).unwrap();
    let session = xwm.data_bridge.dnd.active_session().unwrap();
    assert_eq!(session.source_proxy, Some(0x880));
    assert_eq!(session.target, None);
    assert_eq!(session.discovery_target, Some(target_b));
    assert_eq!(session.progress, DndWireProgress::AwaitingEnter);
    assert_eq!(
        xwm.data_bridge
            .dnd
            .pending_replies
            .values()
            .filter(|reply| matches!(reply, DndPendingReply::TargetProxy { actual, .. } if *actual == target_b))
            .count(),
        1
    );

    xwm.flush().unwrap();
    peer.set_nonblocking(true).unwrap();
    let mut request = [0u8; 64];
    let bytes_read = peer.read(&mut request).unwrap();
    assert_eq!(bytes_read, 64);
    assert_eq!(request[0], 25);
    assert_eq!(
        request_u32(&request, 20),
        xwm.atoms.get(XwmAtomName::XdndLeave)
    );
    assert_eq!(request[44], 20, "target discovery follows Leave");
}

#[test]
fn xdnd_selection_clear_only_retires_the_exact_confirmed_source_proxy() {
    let generation = XwaylandGeneration::new(NonZeroU64::new(77).unwrap());
    let (mut xwm, _peer) = super::super::super::test_fixture_for_tests(generation);
    let id = identity(generation, 9008);
    install_session(&mut xwm, id);
    assert!(xwm.data_bridge.dnd.bind_source_proxy(id, 0x880));
    xwm.data_bridge.dnd.internal_windows.insert(0x880);
    assert!(
        xwm.data_bridge
            .dnd
            .confirm_source_ownership(id, 0x880, 1234)
    );
    let selection = xwm.atoms.get(XwmAtomName::XdndSelection);
    let stale = xproto::SelectionClearEvent {
        response_type: xproto::SELECTION_CLEAR_EVENT,
        sequence: 0,
        time: 1235,
        owner: 0x881,
        selection,
    };
    assert!(!selection_clear(&mut xwm, stale, 600).unwrap());
    assert_eq!(xwm.data_bridge.dnd.active_id(), Some(id));

    let exact = xproto::SelectionClearEvent {
        owner: 0x880,
        ..stale
    };
    assert!(selection_clear(&mut xwm, exact, 601).unwrap());
    assert_eq!(xwm.data_bridge.dnd.active_id(), None);
    assert!(!xwm.data_bridge.dnd.internal_windows.contains(&0x880));
}
