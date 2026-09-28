use std::{io::Read, num::NonZeroU64};

use super::*;
use crate::xwayland::{XwaylandDndAction, XwaylandDndMimeCatalog};

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
    assert!(xwm.data_bridge.dnd.install_wayland_session(
        id,
        catalog(&["text/plain"]),
        vec![XwaylandDndAction::Copy, XwaylandDndAction::Move],
    ));
}

fn request_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap())
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

    retire_session(&mut xwm, first).unwrap();
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
    assert!(xwm.data_bridge.dnd.internal_windows.contains(&first_proxy));
    assert!(destroy_notify(&mut xwm, first_proxy).unwrap());
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
    assert_eq!(session.progress, DndWireProgress::Positioned);
    assert!(session.awaiting_status);
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
    xwm.data_bridge.dnd.active.as_mut().unwrap().awaiting_status = true;

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
        xwm.data_bridge
            .dnd
            .active_session()
            .unwrap()
            .awaiting_status
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
        [DndStatusFeedback {
            id,
            target,
            accepted: true,
            action: Some(XwaylandDndAction::Copy),
        }]
    );
    assert!(
        !xwm.data_bridge
            .dnd
            .active_session()
            .unwrap()
            .awaiting_status
    );
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
    xwm.data_bridge.dnd.active.as_mut().unwrap().awaiting_status = true;
    xwm.data_bridge.dnd.set_status_deadline(id, 400);

    handle_deadline(&mut xwm, 400).unwrap();
    assert_eq!(
        xwm.data_bridge.dnd.take_feedback(),
        [DndStatusFeedback {
            id,
            target,
            accepted: false,
            action: None,
        }]
    );
    let session = xwm.data_bridge.dnd.active_session().unwrap();
    assert_eq!(session.source_proxy, Some(0x880));
    assert!(session.ownership_confirmed);
    assert_eq!(session.target, None);
    assert!(!session.awaiting_status);

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
    xwm.data_bridge.dnd.active.as_mut().unwrap().awaiting_status = true;

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
    assert!(xwm.data_bridge.dnd.internal_windows.contains(&0x880));
}
