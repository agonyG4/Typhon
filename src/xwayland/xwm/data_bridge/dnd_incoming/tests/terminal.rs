use super::*;
use crate::xwayland::XwaylandDndAction as Action;
use std::{
    collections::HashMap,
    io::{Read, Write},
    os::fd::AsRawFd,
    os::unix::net::UnixStream,
    thread,
};
use x11rb::protocol::xproto;

const DROP_TIME: u32 = 0x7654_3210;

#[path = "terminal_progress_tests.rs"]
mod progress;

fn finished_messages(bytes: &[u8], xwm: &Xwm) -> Vec<(Window, [u32; 5])> {
    bytes
        .windows(32)
        .filter(|event| {
            event[0] & 0x7f == xproto::CLIENT_MESSAGE_EVENT
                && u32::from_ne_bytes(event[8..12].try_into().unwrap())
                    == xwm.atoms.get(XwmAtomName::XdndFinished)
        })
        .map(|event| {
            let data = std::array::from_fn(|index| {
                let start = 12 + index * 4;
                u32::from_ne_bytes(event[start..start + 4].try_into().unwrap())
            });
            (u32::from_ne_bytes(event[4..8].try_into().unwrap()), data)
        })
        .collect()
}

fn protocol_message_count(bytes: &[u8], atom: Atom) -> usize {
    bytes
        .windows(32)
        .filter(|event| {
            event[0] & 0x7f == xproto::CLIENT_MESSAGE_EVENT
                && u32::from_ne_bytes(event[8..12].try_into().unwrap()) == atom
        })
        .count()
}

fn assert_only_canonical_leave(xwm: &mut Xwm, offer_id: XwaylandDndOfferId) {
    assert!(matches!(
        xwm.data_bridge.dnd_incoming.take_events().as_slice(),
        [XwaylandDndIncomingEvent::Leave { offer_id: left }] if *left == offer_id
    ));
}

fn set_requested_action(xwm: &mut Xwm, action: Action) -> XwaylandDndIncomingPositionId {
    let session = xwm.data_bridge.dnd.incoming_session_mut().unwrap();
    let mut position = session.latest_position.unwrap();
    position.requested_action = action;
    session.latest_position = Some(position);
    session.source_actions = match action {
        Action::Copy => vec![Action::Copy],
        Action::Move => vec![Action::Move, Action::Copy],
        Action::Ask => vec![Action::Ask, Action::Copy, Action::Move],
        Action::Link | Action::Private => vec![action, Action::Copy],
    };
    position.position_id
}

fn accept_position(
    xwm: &mut Xwm,
    peer: &mut UnixStream,
    offer_id: XwaylandDndOfferId,
    action: Action,
) -> bool {
    let position_id = set_requested_action(xwm, action);
    super::source_feedback(
        xwm,
        offer_id,
        position_id,
        Some("text/plain".to_owned()),
        Some(action),
    )
    .unwrap();
    xwm.connection.flush().unwrap();
    let statuses = status_messages(&read_peer(peer), xwm);
    assert_eq!(
        statuses.len(),
        1,
        "the exact current Position receives one Status"
    );
    statuses[0].1[1] & 1 != 0
}

fn drop_message(xwm: &Xwm, source: Window, timestamp: u32) -> xproto::ClientMessageEvent {
    client_message(
        xwm,
        xwm.root,
        XwmAtomName::XdndDrop,
        [source, 0xaaaa, timestamp, 0xbbbb, 0xcccc],
    )
}

fn submit_drop_and_ack(xwm: &mut Xwm, offer_id: XwaylandDndOfferId, timestamp: u32, now_ns: u64) {
    let source = xwm.data_bridge.dnd.incoming_session().unwrap().source.xid();
    let event = drop_message(xwm, source, timestamp);
    assert!(super::super::client_message(xwm, event, now_ns).unwrap());
    assert!(matches!(
        xwm.data_bridge.dnd_incoming.take_events().as_slice(),
        [XwaylandDndIncomingEvent::Drop { offer_id: submitted }] if *submitted == offer_id
    ));
    super::super::resolve_drop(xwm, offer_id, true, now_ns + 1).unwrap();
    assert!(matches!(
        xwm.data_bridge.dnd.incoming_session().unwrap().wire_phase,
        IncomingDndWirePhase::AwaitingWaylandFinish { drop_timestamp, .. }
            if drop_timestamp == timestamp
    ));
}

fn accepted_drop(action: Action) -> (Xwm, UnixStream, XwaylandDndOfferId, Atom, u32, u16) {
    let (mut xwm, mut peer, offer_id, mime_atom, _, _, server_sequence) = fake_incoming_hover();
    assert!(accept_position(&mut xwm, &mut peer, offer_id, action));
    submit_drop_and_ack(&mut xwm, offer_id, DROP_TIME, 40);
    (xwm, peer, offer_id, mime_atom, DROP_TIME, server_sequence)
}

fn terminal_deadlines(xwm: &Xwm) -> (u64, u64, bool) {
    match xwm.data_bridge.dnd.incoming_session().unwrap().wire_phase {
        IncomingDndWirePhase::AwaitingWaylandFinish {
            deadline_ns,
            hard_deadline_ns,
            cancel_submitted,
            ..
        } => (deadline_ns, hard_deadline_ns, cancel_submitted),
        phase => panic!("expected dropped phase, got {phase:?}"),
    }
}

fn selection_notify_event(
    requestor: Window,
    selection: Atom,
    target: Atom,
    property: Atom,
    timestamp: u32,
) -> xproto::SelectionNotifyEvent {
    xproto::SelectionNotifyEvent {
        response_type: xproto::SELECTION_NOTIFY_EVENT,
        sequence: 0,
        time: timestamp,
        requestor,
        selection,
        target,
        property,
    }
}

fn deliver_selection_notify(
    xwm: &mut Xwm,
    requestor: Window,
    target: Atom,
    property: Atom,
    timestamp: u32,
    now_ns: u64,
) -> bool {
    let selection = xwm.atoms.get(XwmAtomName::XdndSelection);
    super::super::selection_notify(
        xwm,
        selection_notify_event(requestor, selection, target, property, timestamp),
        now_ns,
    )
    .unwrap()
}

fn start_root_proxy_verification(xwm: &mut Xwm, now_ns: u64) -> u16 {
    let event = xproto::PropertyNotifyEvent {
        response_type: xproto::PROPERTY_NOTIFY_EVENT,
        sequence: 0,
        window: xwm.root,
        atom: xwm.atoms.get(XwmAtomName::XdndProxy),
        time: 1,
        state: xproto::Property::NEW_VALUE,
    };
    assert!(super::super::property_notify(xwm, event, now_ns).unwrap());
    match xwm.data_bridge.dnd_incoming.root_proxy_authority {
        RootProxyAuthority::Verifying { sequence, .. } => sequence as u16,
        authority => panic!("expected root proxy verification, got {authority:?}"),
    }
}

fn send_root_proxy_reply(peer: &mut UnixStream, sequence: u16, proxy: Window) {
    peer.write_all(&get_property_reply(
        sequence,
        u32::from(AtomEnum::WINDOW),
        32,
        &proxy.to_ne_bytes(),
    ))
    .unwrap();
}

fn lose_root_proxy(xwm: &mut Xwm, peer: &mut UnixStream, now_ns: u64, foreign_proxy: Window) {
    let sequence = start_root_proxy_verification(xwm, now_ns);
    let _verification_request = read_peer(peer);
    send_root_proxy_reply(peer, sequence, foreign_proxy);
    super::poll_replies(xwm, 8, now_ns.saturating_add(1)).unwrap();
}

fn deliver_post_drop_direct_payload(
    xwm: &mut Xwm,
    peer: &mut UnixStream,
    offer_id: XwaylandDndOfferId,
    mime_atom: Atom,
    timestamp: u32,
    server_sequence: &mut u16,
    payload: &[u8],
) {
    let (mut reader, transfer_id, requestor, property) = begin_fake_selection_transfer_at(
        xwm,
        peer,
        offer_id,
        mime_atom,
        timestamp,
        server_sequence,
        300,
    );
    notify_fake_selection_transfer(
        xwm,
        peer,
        transfer_id,
        mime_atom,
        timestamp,
        requestor,
        property,
        *server_sequence,
        server_sequence,
    );
    let sequence = transfer_pending_sequence(xwm, transfer_id).unwrap();
    peer.write_all(&get_property_reply(sequence, mime_atom, 8, payload))
        .unwrap();
    xwm.drain_events(32).unwrap();
    let mut received = vec![0; payload.len()];
    reader.read_exact(&mut received).unwrap();
    assert_eq!(received, payload);
    assert!(!transfer_is_active(xwm, transfer_id));
}

type FakeProperty = (Atom, u8, Vec<u8>);

fn fake_proxy_property_server(
    mut peer: UnixStream,
    root: Window,
    root_proxy_atom: Atom,
    properties: HashMap<(Window, Atom), FakeProperty>,
    mut sequence: u16,
) -> (HashMap<(Window, Atom), FakeProperty>, usize, usize) {
    let mut properties = properties;
    let mut root_publications = 0;
    let mut root_deletions = 0;
    loop {
        let mut header = [0; 4];
        peer.read_exact(&mut header).unwrap();
        let request_len = usize::from(u16::from_ne_bytes([header[2], header[3]])) * 4;
        assert!(request_len >= 4);
        let mut request = vec![0; request_len];
        request[..4].copy_from_slice(&header);
        peer.read_exact(&mut request[4..]).unwrap();
        sequence = sequence.wrapping_add(1);
        match request[0] {
            xproto::CHANGE_PROPERTY_REQUEST => {
                let window = u32::from_ne_bytes(request[4..8].try_into().unwrap());
                let property = u32::from_ne_bytes(request[8..12].try_into().unwrap());
                let type_atom = u32::from_ne_bytes(request[12..16].try_into().unwrap());
                let format = request[16];
                let items = u32::from_ne_bytes(request[20..24].try_into().unwrap()) as usize;
                let data_len = items * usize::from(format / 8);
                let value = request[24..24 + data_len].to_vec();
                if window == root && property == root_proxy_atom {
                    root_publications += 1;
                }
                properties.insert((window, property), (type_atom, format, value));
            }
            xproto::DELETE_PROPERTY_REQUEST => {
                let window = u32::from_ne_bytes(request[4..8].try_into().unwrap());
                let property = u32::from_ne_bytes(request[8..12].try_into().unwrap());
                if window == root && property == root_proxy_atom {
                    root_deletions += 1;
                }
                properties.remove(&(window, property));
            }
            xproto::GET_PROPERTY_REQUEST => {
                let window = u32::from_ne_bytes(request[4..8].try_into().unwrap());
                let property = u32::from_ne_bytes(request[8..12].try_into().unwrap());
                let reply =
                    if let Some((type_atom, format, value)) = properties.get(&(window, property)) {
                        get_property_reply(sequence, *type_atom, *format, value)
                    } else {
                        let mut reply = vec![0; 32];
                        reply[0] = 1;
                        reply[2..4].copy_from_slice(&sequence.to_ne_bytes());
                        reply
                    };
                peer.write_all(&reply).unwrap();
            }
            xproto::UNGRAB_SERVER_REQUEST => {
                return (properties, root_publications, root_deletions);
            }
            _ => {}
        }
    }
}

fn word_property(type_atom: Atom, value: u32) -> FakeProperty {
    (type_atom, 32, value.to_ne_bytes().to_vec())
}

#[test]
fn startup_acquires_and_publishes_only_the_ready_private_root_proxy() {
    let generation = XwaylandGeneration::new(NonZeroU64::new(190).unwrap());
    let (mut xwm, peer) = super::super::super::super::test_fixture_for_tests(generation);
    super::super::initialize_target_proxy(&mut xwm).unwrap();
    let proxy = super::super::target_proxy(&xwm).unwrap();
    let root_proxy_atom = xwm.atoms.get(XwmAtomName::XdndProxy);
    let aware_atom = xwm.atoms.get(XwmAtomName::XdndAware);
    let root = xwm.root;
    let server = thread::spawn(move || {
        fake_proxy_property_server(peer, root, root_proxy_atom, HashMap::new(), 0)
    });

    assert!(super::super::acquire_root_proxy(&mut xwm).unwrap());
    let (properties, root_publications, _) = server.join().unwrap();
    assert_eq!(root_publications, 1);
    assert_eq!(
        properties.get(&(root, root_proxy_atom)),
        Some(&word_property(u32::from(AtomEnum::WINDOW), proxy))
    );
    assert_eq!(
        properties.get(&(proxy, root_proxy_atom)),
        Some(&word_property(u32::from(AtomEnum::WINDOW), proxy))
    );
    assert_eq!(
        properties.get(&(proxy, aware_atom)),
        Some(&word_property(u32::from(AtomEnum::ATOM), 5))
    );
    assert!(matches!(
        xwm.data_bridge.dnd_incoming.root_proxy_authority,
        RootProxyAuthority::Owned { generation: owner, proxy: owned }
            if owner == generation && owned == proxy
    ));
}

#[test]
fn startup_respects_a_valid_foreign_root_proxy_and_blocks_only_reverse_dnd() {
    let generation = XwaylandGeneration::new(NonZeroU64::new(191).unwrap());
    let (mut xwm, peer) = super::super::super::super::test_fixture_for_tests(generation);
    super::super::initialize_target_proxy(&mut xwm).unwrap();
    let proxy = super::super::target_proxy(&xwm).unwrap();
    let root = xwm.root;
    let root_proxy_atom = xwm.atoms.get(XwmAtomName::XdndProxy);
    let aware_atom = xwm.atoms.get(XwmAtomName::XdndAware);
    let foreign_proxy: Window = 0x778;
    let properties = HashMap::from([
        (
            (root, root_proxy_atom),
            word_property(u32::from(AtomEnum::WINDOW), foreign_proxy),
        ),
        (
            (foreign_proxy, root_proxy_atom),
            word_property(u32::from(AtomEnum::WINDOW), foreign_proxy),
        ),
        (
            (foreign_proxy, aware_atom),
            word_property(u32::from(AtomEnum::ATOM), 5),
        ),
    ]);
    let expected_root_property = properties[&(root, root_proxy_atom)].clone();
    let server = thread::spawn(move || {
        fake_proxy_property_server(peer, root, root_proxy_atom, properties, 0)
    });

    assert!(!super::super::acquire_root_proxy(&mut xwm).unwrap());
    let (properties, root_publications, _) = server.join().unwrap();
    assert_eq!(root_publications, 0);
    assert_eq!(
        properties.get(&(root, root_proxy_atom)),
        Some(&expected_root_property)
    );
    assert!(matches!(
        xwm.data_bridge.dnd_incoming.root_proxy_authority,
        RootProxyAuthority::BlockedForeign { generation: owner, proxy: foreign }
            if owner == generation && foreign == foreign_proxy
    ));
    assert_ne!(proxy, foreign_proxy);
    assert!(!super::super::root_proxy_is_owned(&xwm));
}

#[test]
fn generation_teardown_clears_drop_delete_verification_and_live_transfer_state() {
    let (mut xwm, mut peer, offer_id, mime_atom, drop_timestamp, mut server_sequence) =
        accepted_drop(Action::Move);
    // accept_position emitted one XdndStatus SendEvent request after the
    // fixture's request counter was last synchronized.
    server_sequence = server_sequence.wrapping_add(1);
    let (mut reader, transfer_id, transfer_requestor, transfer_property) =
        super::begin_fake_selection_transfer_at(
            &mut xwm,
            &mut peer,
            offer_id,
            mime_atom,
            drop_timestamp,
            &mut server_sequence,
            80,
        );
    assert!(
        xwm.data_bridge
            .dnd_incoming
            .transfers
            .contains_key(&transfer_id)
    );

    let root_proxy_atom = xwm.atoms.get(XwmAtomName::XdndProxy);
    let root_change = xproto::PropertyNotifyEvent {
        response_type: xproto::PROPERTY_NOTIFY_EVENT,
        sequence: 0,
        window: xwm.root,
        atom: root_proxy_atom,
        time: 1,
        state: xproto::Property::NEW_VALUE,
    };
    assert!(super::super::property_notify(&mut xwm, root_change, 90).unwrap());
    xwm.connection.flush().unwrap();
    let verification_requests = super::take_requests(&mut peer, &mut server_sequence);
    assert!(
        verification_requests
            .chunks_exact(4)
            .any(|chunk| chunk[0] == xproto::GET_PROPERTY_REQUEST)
    );
    assert!(matches!(
        xwm.data_bridge.dnd_incoming.root_proxy_authority,
        RootProxyAuthority::Verifying { .. }
    ));

    super::super::source_finished(&mut xwm, offer_id, true, Some(Action::Move), 91).unwrap();
    let delete = xwm.data_bridge.dnd_incoming.move_delete.unwrap();
    assert_eq!(delete.drop_timestamp, drop_timestamp);
    assert!(matches!(
        xwm.data_bridge.dnd.incoming_session().unwrap().wire_phase,
        IncomingDndWirePhase::DeletePending { .. }
    ));
    let delete_requests = super::take_requests(&mut peer, &mut server_sequence);
    assert_eq!(
        super::selection_conversion_timestamps(
            &delete_requests,
            xwm.atoms.get(XwmAtomName::XdndSelection),
        ),
        vec![drop_timestamp]
    );
    assert!(
        xwm.data_bridge
            .dnd_incoming
            .requestors
            .contains_key(&transfer_requestor)
    );
    assert!(
        xwm.data_bridge
            .dnd
            .internal_windows
            .contains(&delete.requestor)
    );

    let target_proxy = xwm.data_bridge.dnd_incoming.target_proxy.unwrap();
    let root = xwm.root;
    let aware_atom = xwm.atoms.get(XwmAtomName::XdndAware);
    let properties = HashMap::from([
        (
            (root, root_proxy_atom),
            word_property(u32::from(AtomEnum::WINDOW), target_proxy),
        ),
        (
            (target_proxy, root_proxy_atom),
            word_property(u32::from(AtomEnum::WINDOW), target_proxy),
        ),
        (
            (target_proxy, aware_atom),
            word_property(u32::from(AtomEnum::ATOM), 5),
        ),
    ]);
    peer.set_nonblocking(false).unwrap();
    let server = thread::spawn(move || {
        fake_proxy_property_server(peer, root, root_proxy_atom, properties, server_sequence)
    });
    let generation = offer_id.generation();
    xwm.clear_generation(generation);
    let (properties, root_publications, root_deletions) = server.join().unwrap();

    assert_eq!(root_publications, 0);
    assert_eq!(root_deletions, 1);
    assert!(!properties.contains_key(&(root, root_proxy_atom)));
    assert!(xwm.data_bridge.dnd.incoming_session().is_none());
    assert_eq!(
        xwm.data_bridge.dnd_incoming.root_proxy_authority,
        RootProxyAuthority::Unpublished
    );
    assert!(xwm.data_bridge.dnd_incoming.generation.is_none());
    assert!(xwm.data_bridge.dnd_incoming.target_proxy.is_none());
    assert!(xwm.data_bridge.dnd_incoming.move_delete.is_none());
    assert!(xwm.data_bridge.dnd_incoming.events.is_empty());
    assert!(xwm.data_bridge.dnd_incoming.pending.is_empty());
    assert!(xwm.data_bridge.dnd_incoming.transfers.is_empty());
    assert!(xwm.data_bridge.dnd_incoming.requestors.is_empty());
    assert!(xwm.data_bridge.dnd_incoming.transfer_replies.is_empty());
    assert!(
        xwm.data_bridge
            .dnd_incoming
            .sink_interests()
            .next()
            .is_none()
    );
    assert!(!xwm.data_bridge.dnd.internal_windows.contains(&target_proxy));
    let mut eof_probe = [0];
    assert_eq!(reader.read(&mut eof_probe).unwrap(), 0);

    assert!(super::super::property_notify(&mut xwm, root_change, 100).unwrap());
    assert_eq!(
        xwm.data_bridge.dnd_incoming.root_proxy_authority,
        RootProxyAuthority::Unpublished
    );
    assert_eq!(super::super::poll_replies(&mut xwm, 8, 100).unwrap(), 0);
    let xdnd_selection_atom = xwm.atoms.get(XwmAtomName::XdndSelection);
    assert!(
        super::super::selection_notify(
            &mut xwm,
            selection_notify_event(
                transfer_requestor,
                xdnd_selection_atom,
                mime_atom,
                transfer_property,
                drop_timestamp,
            ),
            101,
        )
        .unwrap()
    );
    let stale_drop = drop_message(&xwm, 0x441, drop_timestamp);
    assert!(super::super::client_message(&mut xwm, stale_drop, 101).unwrap());
    super::super::source_finished(&mut xwm, offer_id, true, Some(Action::Move), 102).unwrap();
    assert!(xwm.data_bridge.dnd.incoming_session().is_none());
}

#[test]
fn generation_teardown_preserves_a_replacement_foreign_root_proxy() {
    let generation = XwaylandGeneration::new(NonZeroU64::new(192).unwrap());
    let (mut xwm, peer) = super::super::super::super::test_fixture_for_tests(generation);
    super::super::initialize_target_proxy(&mut xwm).unwrap();
    let root = xwm.root;
    let root_proxy_atom = xwm.atoms.get(XwmAtomName::XdndProxy);
    let foreign_proxy: Window = 0x77a;
    let foreign_property = word_property(u32::from(AtomEnum::WINDOW), foreign_proxy);
    let server_foreign_property = foreign_property.clone();
    xwm.data_bridge.dnd_incoming.root_proxy_authority = RootProxyAuthority::Owned {
        generation,
        proxy: super::super::target_proxy(&xwm).unwrap(),
    };
    let server = thread::spawn(move || {
        fake_proxy_property_server(
            peer,
            root,
            root_proxy_atom,
            HashMap::from([((root, root_proxy_atom), server_foreign_property)]),
            0,
        )
    });

    xwm.clear_generation(generation);

    let (properties, root_publications, root_deletions) = server.join().unwrap();
    assert_eq!(root_publications, 0);
    assert_eq!(root_deletions, 0);
    assert_eq!(
        properties.get(&(root, root_proxy_atom)),
        Some(&foreign_property)
    );
    assert_eq!(
        xwm.data_bridge.dnd_incoming.root_proxy_authority,
        RootProxyAuthority::Unpublished
    );
    assert!(xwm.data_bridge.dnd_incoming.target_proxy.is_none());
}

#[test]
fn drop_while_status_is_pending_is_rejected_without_canonical_drop() {
    let (mut xwm, mut peer, _offer_id, _, _, _, _) = fake_incoming_hover();
    let source = xwm.data_bridge.dnd.incoming_session().unwrap().source.xid();
    let event = drop_message(&xwm, source, DROP_TIME);
    assert!(super::super::client_message(&mut xwm, event, 50).unwrap());
    let rejected_events = xwm.data_bridge.dnd_incoming.take_events();
    assert!(
        rejected_events
            .iter()
            .all(|event| !matches!(event, XwaylandDndIncomingEvent::Drop { .. }))
    );
    assert!(matches!(
        rejected_events.as_slice(),
        [XwaylandDndIncomingEvent::Leave { offer_id: left }] if *left == _offer_id
    ));
    assert!(xwm.data_bridge.dnd.incoming_session().is_none());
    xwm.connection.flush().unwrap();
    let bytes = read_peer(&mut peer);
    let statuses = status_messages(&bytes, &xwm);
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].1[1] & 1, 0);
    assert_eq!(finished_messages(&bytes, &xwm).len(), 1);
    assert_eq!(finished_messages(&bytes, &xwm)[0].1, [xwm.root, 0, 0, 0, 0]);
}

#[test]
fn drop_without_mime_or_selected_action_never_submits_canonical_drop() {
    for feedback in [(None, None), (Some("text/plain".to_owned()), None)] {
        let (mut xwm, mut peer, offer_id, _, _, _, _) = fake_incoming_hover();
        let position_id = xwm
            .data_bridge
            .dnd
            .incoming_session()
            .unwrap()
            .latest_position
            .unwrap()
            .position_id;
        super::source_feedback(&mut xwm, offer_id, position_id, feedback.0, feedback.1).unwrap();
        xwm.connection.flush().unwrap();
        let _ = read_peer(&mut peer);
        let source = xwm.data_bridge.dnd.incoming_session().unwrap().source.xid();
        let event = drop_message(&xwm, source, DROP_TIME);
        super::super::client_message(&mut xwm, event, 51).unwrap();
        assert_only_canonical_leave(&mut xwm, offer_id);
        assert!(xwm.data_bridge.dnd.incoming_session().is_none());
        xwm.connection.flush().unwrap();
        let messages = finished_messages(&read_peer(&mut peer), &xwm);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].1, [xwm.root, 0, 0, 0, 0]);
    }
}

#[test]
fn zero_drop_timestamp_fails_closed_without_guessing_position_time() {
    let (mut xwm, mut peer, offer_id, _, position_time, _, _) = fake_incoming_hover();
    assert!(accept_position(&mut xwm, &mut peer, offer_id, Action::Copy));
    let source = xwm.data_bridge.dnd.incoming_session().unwrap().source.xid();
    let event = drop_message(&xwm, source, 0);
    super::super::client_message(&mut xwm, event, 52).unwrap();
    let rejected_events = xwm.data_bridge.dnd_incoming.take_events();
    assert!(
        rejected_events
            .iter()
            .all(|event| !matches!(event, XwaylandDndIncomingEvent::Drop { .. }))
    );
    assert!(matches!(
        rejected_events.as_slice(),
        [XwaylandDndIncomingEvent::Leave { offer_id: left }] if *left == offer_id
    ));
    assert!(xwm.data_bridge.dnd.incoming_session().is_none());
    xwm.connection.flush().unwrap();
    let messages = finished_messages(&read_peer(&mut peer), &xwm);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].1, [xwm.root, 0, 0, 0, 0]);
    assert_ne!(position_time, 0);
}

#[test]
fn pre_v5_root_position_is_rejected_and_cannot_reach_canonical_drop() {
    let (mut xwm, mut peer, offer_id, _, _, _, _) = fake_incoming_hover_with_version(4);
    let session = xwm.data_bridge.dnd.incoming_session().unwrap();
    assert_eq!(session.version.get(), 4);
    assert!(session.metadata_complete);
    assert_eq!(session.mime_types, ["text/plain"]);
    let position_id = session.latest_position.unwrap().position_id;
    super::source_feedback(
        &mut xwm,
        offer_id,
        position_id,
        Some("text/plain".to_owned()),
        Some(Action::Copy),
    )
    .unwrap();
    xwm.connection.flush().unwrap();
    let rejected = status_messages(&read_peer(&mut peer), &xwm);
    assert_eq!(rejected.len(), 1);
    assert_eq!(rejected[0].1[1] & 1, 0, "v4 receives rejected Status only");
    let source = xwm.data_bridge.dnd.incoming_session().unwrap().source.xid();
    let event = drop_message(&xwm, source, DROP_TIME);
    super::super::client_message(&mut xwm, event, 53).unwrap();
    assert_only_canonical_leave(&mut xwm, offer_id);
    assert!(xwm.data_bridge.dnd.incoming_session().is_none());
    xwm.connection.flush().unwrap();
    assert!(finished_messages(&read_peer(&mut peer), &xwm).is_empty());
}

#[test]
fn canonical_drop_failure_sends_one_v5_failure_finished() {
    let (mut xwm, mut peer, offer_id, _, _, _, _) = fake_incoming_hover();
    assert!(accept_position(&mut xwm, &mut peer, offer_id, Action::Copy));
    let source = xwm.data_bridge.dnd.incoming_session().unwrap().source.xid();
    let event = drop_message(&xwm, source, DROP_TIME);
    assert!(super::super::client_message(&mut xwm, event, 60).unwrap());
    assert!(matches!(
        xwm.data_bridge.dnd_incoming.take_events().as_slice(),
        [XwaylandDndIncomingEvent::Drop { offer_id: submitted }] if *submitted == offer_id
    ));
    super::super::resolve_drop(&mut xwm, offer_id, false, 60).unwrap();
    super::super::resolve_drop(&mut xwm, offer_id, false, 61).unwrap();
    assert!(xwm.data_bridge.dnd.incoming_session().is_none());
    let messages = finished_messages(&read_peer(&mut peer), &xwm);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].1, [xwm.root, 0, 0, 0, 0]);
}

#[test]
fn copy_source_finished_sends_one_success_and_consumes_the_exact_offer() {
    let (mut xwm, mut peer, offer_id, _, _, _) = accepted_drop(Action::Copy);
    super::super::source_finished(&mut xwm, offer_id, true, Some(Action::Copy), 70).unwrap();
    super::super::source_finished(&mut xwm, offer_id, true, Some(Action::Copy), 71).unwrap();
    assert!(xwm.data_bridge.dnd.incoming_session().is_none());
    assert!(xwm.data_bridge.dnd_incoming.move_delete.is_none());
    assert!(xwm.data_bridge.dnd_incoming.transfers.is_empty());
    let bytes = read_peer(&mut peer);
    let messages = finished_messages(&bytes, &xwm);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].0, 0x441);
    assert_eq!(
        messages[0].1,
        [
            xwm.root,
            1,
            xwm.atoms.get(XwmAtomName::XdndActionCopy),
            0,
            0
        ]
    );
    assert_eq!(
        protocol_message_count(&bytes, xwm.atoms.get(XwmAtomName::XdndLeave)),
        0
    );
    let (reader, writer) = UnixStream::pair().unwrap();
    let request = crate::xwayland::XwaylandDndDataRequest {
        offer_id,
        mime_type: "text/plain".to_owned(),
        sink: writer.into(),
    };
    assert!(
        super::start_data_request(&mut xwm, request, 72)
            .unwrap()
            .is_none()
    );
    drop(reader);
}

#[test]
fn canonical_source_finished_transition_reaches_wire_and_retired_keeps_move_cleanup() {
    let (mut xwm, mut peer, copy_offer, _, _, _) = accepted_drop(Action::Copy);
    let copy_finished = crate::xwayland::XwaylandDndTransition::SourceFinished {
        offer_id: copy_offer,
        accepted: true,
        action: Some(Action::Copy),
    };
    super::super::super::dnd_adapter::apply_transitions(&mut xwm, [copy_finished], 70).unwrap();
    assert_eq!(finished_messages(&read_peer(&mut peer), &xwm).len(), 1);
    assert!(xwm.data_bridge.dnd.incoming_session().is_none());

    let (mut xwm, mut peer, move_offer, _, _, _) = accepted_drop(Action::Move);
    let move_finished = crate::xwayland::XwaylandDndTransition::SourceFinished {
        offer_id: move_offer,
        accepted: true,
        action: Some(Action::Move),
    };
    super::super::super::dnd_adapter::apply_transitions(&mut xwm, [move_finished], 80).unwrap();
    let requestor = xwm.data_bridge.dnd_incoming.move_delete.unwrap().requestor;
    let retired = crate::xwayland::XwaylandDndTransition::Retired {
        session_id: crate::xwayland::CanonicalDndSessionId::Xwayland(move_offer),
        generation: move_offer.generation(),
    };
    super::super::super::dnd_adapter::apply_transitions(&mut xwm, [retired], 81).unwrap();
    assert_eq!(
        xwm.data_bridge.dnd_incoming.move_delete.unwrap().requestor,
        requestor
    );
    assert!(matches!(
        xwm.data_bridge.dnd.incoming_session().unwrap().wire_phase,
        IncomingDndWirePhase::DeletePending { .. }
    ));
    assert!(finished_messages(&read_peer(&mut peer), &xwm).is_empty());
}

#[test]
fn synthetic_fake_x_wayland_copy_completes_drop_data_finished_and_cleanup_in_order() {
    let (mut xwm, mut peer, offer_id, mime_atom, timestamp, _, mut server_sequence) =
        fake_incoming_hover();
    assert!(accept_position(&mut xwm, &mut peer, offer_id, Action::Copy));
    submit_drop_and_ack(&mut xwm, offer_id, timestamp, 250);
    deliver_post_drop_direct_payload(
        &mut xwm,
        &mut peer,
        offer_id,
        mime_atom,
        timestamp,
        &mut server_sequence,
        b"copy payload",
    );
    super::super::source_finished(&mut xwm, offer_id, true, Some(Action::Copy), 310).unwrap();

    let bytes = read_peer(&mut peer);
    let finished = finished_messages(&bytes, &xwm);
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0].0, 0x441);
    assert_eq!(
        finished[0].1,
        [
            xwm.root,
            1,
            xwm.atoms.get(XwmAtomName::XdndActionCopy),
            0,
            0
        ]
    );
    assert_eq!(
        protocol_message_count(&bytes, xwm.atoms.get(XwmAtomName::XdndLeave)),
        0
    );
    assert!(xwm.data_bridge.dnd.incoming_session().is_none());
    assert!(xwm.data_bridge.dnd_incoming.transfers.is_empty());
    assert!(xwm.data_bridge.dnd_incoming.pending.is_empty());
    assert!(xwm.data_bridge.dnd_incoming.move_delete.is_none());
    assert!(super::super::next_deadline_ns(&xwm).is_none());
}

#[test]
fn synthetic_fake_x_wayland_move_finishes_only_after_delete_selection_reply() {
    let (mut xwm, mut peer, offer_id, mime_atom, timestamp, _, mut server_sequence) =
        fake_incoming_hover();
    assert!(accept_position(&mut xwm, &mut peer, offer_id, Action::Move));
    submit_drop_and_ack(&mut xwm, offer_id, timestamp, 320);
    deliver_post_drop_direct_payload(
        &mut xwm,
        &mut peer,
        offer_id,
        mime_atom,
        timestamp,
        &mut server_sequence,
        b"move payload",
    );
    super::super::source_finished(&mut xwm, offer_id, true, Some(Action::Move), 380).unwrap();
    let delete = xwm.data_bridge.dnd_incoming.move_delete.unwrap();
    assert!(matches!(
        xwm.data_bridge.dnd.incoming_session().unwrap().wire_phase,
        IncomingDndWirePhase::DeletePending { drop_timestamp, final_action: Action::Move }
            if drop_timestamp == timestamp
    ));
    let conversion = take_requests(&mut peer, &mut server_sequence);
    assert_eq!(
        selection_conversion_timestamps(&conversion, xwm.atoms.get(XwmAtomName::XdndSelection)),
        vec![timestamp]
    );
    assert!(finished_messages(&conversion, &xwm).is_empty());

    let event = selection_notify_event(
        delete.requestor,
        xwm.atoms.get(XwmAtomName::XdndSelection),
        xwm.atoms.get(XwmAtomName::Delete),
        delete.property,
        timestamp,
    );
    super::super::selection_notify(&mut xwm, event, 381).unwrap();
    let finished = finished_messages(&read_peer(&mut peer), &xwm);
    assert_eq!(finished.len(), 1);
    assert_eq!(
        finished[0].1,
        [
            xwm.root,
            1,
            xwm.atoms.get(XwmAtomName::XdndActionMove),
            0,
            0
        ]
    );
    assert!(xwm.data_bridge.dnd.incoming_session().is_none());
    assert!(xwm.data_bridge.dnd_incoming.transfers.is_empty());
    assert!(xwm.data_bridge.dnd_incoming.move_delete.is_none());
    assert!(super::super::next_deadline_ns(&xwm).is_none());
}

#[test]
fn position_leave_and_duplicate_drop_are_inert_after_canonical_drop() {
    let (mut xwm, mut peer, offer_id, _, _, _) = accepted_drop(Action::Copy);
    let original = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .latest_position
        .unwrap();
    let source = xwm.data_bridge.dnd.incoming_session().unwrap().source.xid();
    let replacement_enter = client_message(
        &xwm,
        xwm.root,
        XwmAtomName::XdndEnter,
        [0x442, 5 << 24, 0x552, 0, 0],
    );
    assert!(super::super::client_message(&mut xwm, replacement_enter, 59).unwrap());
    assert_eq!(
        xwm.data_bridge.dnd.incoming_session().unwrap().offer_id,
        offer_id,
        "a new source cannot replace a dropped incoming session"
    );
    let position = client_message(
        &xwm,
        xwm.root,
        XwmAtomName::XdndPosition,
        [
            source,
            0x777,
            0x1234_5678,
            0x1111,
            xwm.atoms.get(XwmAtomName::XdndActionMove),
        ],
    );
    let leave = client_message(&xwm, xwm.root, XwmAtomName::XdndLeave, [source, 0, 0, 0, 0]);
    let duplicate = drop_message(&xwm, source, DROP_TIME + 1);
    for event in [position, leave, duplicate] {
        assert!(super::super::client_message(&mut xwm, event, 73).unwrap());
    }
    let session = xwm.data_bridge.dnd.incoming_session().unwrap();
    assert_eq!(session.offer_id, offer_id);
    assert_eq!(session.latest_position.unwrap(), original);
    assert!(matches!(
        session.wire_phase,
        IncomingDndWirePhase::AwaitingWaylandFinish { .. }
    ));
    assert!(xwm.data_bridge.dnd_incoming.take_events().is_empty());
    xwm.connection.flush().unwrap();
    let messages = read_peer(&mut peer);
    assert_eq!(
        protocol_message_count(&messages, xwm.atoms.get(XwmAtomName::XdndStatus)),
        0
    );
    assert_eq!(
        protocol_message_count(&messages, xwm.atoms.get(XwmAtomName::XdndFinished)),
        0
    );
    assert_eq!(
        protocol_message_count(&messages, xwm.atoms.get(XwmAtomName::XdndLeave)),
        0
    );
}

#[test]
fn rejected_source_finished_ignores_action_and_sends_failure_once() {
    let (mut xwm, mut peer, offer_id, _, _, _) = accepted_drop(Action::Copy);
    super::super::source_finished(&mut xwm, offer_id, false, Some(Action::Move), 80).unwrap();
    super::super::source_finished(&mut xwm, offer_id, false, Some(Action::Copy), 81).unwrap();
    let messages = finished_messages(&read_peer(&mut peer), &xwm);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].1, [xwm.root, 0, 0, 0, 0]);
}

#[test]
fn frozen_non_ask_action_cannot_change_after_drop() {
    for (frozen, reported) in [(Action::Copy, Action::Move), (Action::Move, Action::Copy)] {
        let (mut xwm, mut peer, offer_id, _, _, _) = accepted_drop(frozen);
        super::super::source_finished(&mut xwm, offer_id, true, Some(reported), 90).unwrap();
        let messages = finished_messages(&read_peer(&mut peer), &xwm);
        assert_eq!(messages.len(), 1, "{frozen:?} -> {reported:?}");
        assert_eq!(messages[0].1, [xwm.root, 0, 0, 0, 0]);
    }
}

#[test]
fn ask_may_resolve_to_copy_or_move_but_not_a_nonconcrete_action() {
    for final_action in [Action::Copy, Action::Move] {
        let (mut xwm, mut peer, offer_id, _, timestamp, mut server_sequence) =
            accepted_drop(Action::Ask);
        super::super::source_finished(&mut xwm, offer_id, true, Some(final_action), 100).unwrap();
        if final_action == Action::Move {
            let delete = xwm.data_bridge.dnd_incoming.move_delete.unwrap();
            let requests = take_requests(&mut peer, &mut server_sequence);
            assert_eq!(
                selection_conversion_timestamps(
                    &requests,
                    xwm.atoms.get(XwmAtomName::XdndSelection)
                ),
                vec![timestamp]
            );
            assert!(finished_messages(&requests, &xwm).is_empty());
            let selection_atom = xwm.atoms.get(XwmAtomName::XdndSelection);
            let delete_atom = xwm.atoms.get(XwmAtomName::Delete);
            let event = selection_notify_event(
                delete.requestor,
                selection_atom,
                delete_atom,
                delete.property,
                timestamp,
            );
            super::super::selection_notify(&mut xwm, event, 101).unwrap();
        }
        let messages = finished_messages(&read_peer(&mut peer), &xwm);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].1[0], xwm.root);
        assert_eq!(messages[0].1[1], 1);
        assert_eq!(
            messages[0].1[2],
            match final_action {
                Action::Copy => xwm.atoms.get(XwmAtomName::XdndActionCopy),
                Action::Move => xwm.atoms.get(XwmAtomName::XdndActionMove),
                _ => unreachable!(),
            }
        );
    }

    for invalid in [
        None,
        Some(Action::Ask),
        Some(Action::Link),
        Some(Action::Private),
    ] {
        let (mut xwm, mut peer, offer_id, _, _, _) = accepted_drop(Action::Ask);
        super::super::source_finished(&mut xwm, offer_id, true, invalid, 110).unwrap();
        let messages = finished_messages(&read_peer(&mut peer), &xwm);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].1, [xwm.root, 0, 0, 0, 0]);
    }
}

#[test]
fn move_delete_success_precedes_finished_and_uses_drop_timestamp() {
    let (mut xwm, mut peer, offer_id, _, timestamp, mut server_sequence) =
        accepted_drop(Action::Move);
    super::super::source_finished(&mut xwm, offer_id, true, Some(Action::Move), 120).unwrap();
    let delete = xwm.data_bridge.dnd_incoming.move_delete.unwrap();
    assert!(matches!(
        xwm.data_bridge.dnd.incoming_session().unwrap().wire_phase,
        IncomingDndWirePhase::DeletePending { drop_timestamp, final_action: Action::Move }
            if drop_timestamp == timestamp
    ));
    let conversion = take_requests(&mut peer, &mut server_sequence);
    assert_eq!(
        selection_conversion_timestamps(&conversion, xwm.atoms.get(XwmAtomName::XdndSelection)),
        vec![timestamp]
    );
    assert!(finished_messages(&conversion, &xwm).is_empty());

    let selection_atom = xwm.atoms.get(XwmAtomName::XdndSelection);
    let delete_atom = xwm.atoms.get(XwmAtomName::Delete);
    let event = selection_notify_event(
        delete.requestor,
        selection_atom,
        delete_atom,
        delete.property,
        timestamp,
    );
    super::super::selection_notify(&mut xwm, event, 121).unwrap();
    let finished = finished_messages(&read_peer(&mut peer), &xwm);
    assert_eq!(finished.len(), 1);
    assert_eq!(
        finished[0].1,
        [
            xwm.root,
            1,
            xwm.atoms.get(XwmAtomName::XdndActionMove),
            0,
            0
        ]
    );
    assert!(xwm.data_bridge.dnd_incoming.move_delete.is_none());
    assert!(xwm.data_bridge.dnd.incoming_session().is_none());
}

#[test]
fn move_delete_refusal_timeout_and_late_reply_fail_once() {
    for timeout in [false, true] {
        let (mut xwm, mut peer, offer_id, _, timestamp, mut server_sequence) =
            accepted_drop(Action::Move);
        super::super::source_finished(&mut xwm, offer_id, true, Some(Action::Move), 130).unwrap();
        let delete = xwm.data_bridge.dnd_incoming.move_delete.unwrap();
        let conversion = take_requests(&mut peer, &mut server_sequence);
        assert_eq!(
            selection_conversion_timestamps(&conversion, xwm.atoms.get(XwmAtomName::XdndSelection)),
            vec![timestamp]
        );
        if timeout {
            super::super::expire_deadlines(&mut xwm, delete.deadline_ns).unwrap();
        } else {
            let selection_atom = xwm.atoms.get(XwmAtomName::XdndSelection);
            let delete_atom = xwm.atoms.get(XwmAtomName::Delete);
            let event = selection_notify_event(
                delete.requestor,
                selection_atom,
                delete_atom,
                u32::from(AtomEnum::NONE),
                timestamp,
            );
            super::super::selection_notify(&mut xwm, event, 131).unwrap();
        }
        let failed = finished_messages(&read_peer(&mut peer), &xwm);
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].1, [xwm.root, 0, 0, 0, 0]);
        assert!(xwm.data_bridge.dnd_incoming.move_delete.is_none());
        assert!(xwm.data_bridge.dnd.incoming_session().is_none());

        let late = selection_notify_event(
            delete.requestor,
            xwm.atoms.get(XwmAtomName::XdndSelection),
            xwm.atoms.get(XwmAtomName::Delete),
            delete.property,
            timestamp,
        );
        assert!(super::super::selection_notify(&mut xwm, late, 132).unwrap());
        assert!(finished_messages(&read_peer(&mut peer), &xwm).is_empty());
    }
}

#[test]
fn move_delete_requestor_destruction_and_canonical_retirement_are_phase_sensitive() {
    let (mut xwm, mut peer, offer_id, _, _, _) = accepted_drop(Action::Move);
    super::super::source_finished(&mut xwm, offer_id, true, Some(Action::Move), 135).unwrap();
    let delete = xwm.data_bridge.dnd_incoming.move_delete.unwrap();
    super::super::canonical_retired(&mut xwm, offer_id);
    assert!(xwm.data_bridge.dnd_incoming.move_delete.is_some());
    assert!(xwm.data_bridge.dnd.incoming_session().is_some());
    assert!(finished_messages(&read_peer(&mut peer), &xwm).is_empty());
    assert!(super::super::requestor_destroyed(
        &mut xwm,
        delete.requestor
    ));
    let failure = finished_messages(&read_peer(&mut peer), &xwm);
    assert_eq!(failure.len(), 1);
    assert_eq!(failure[0].1, [xwm.root, 0, 0, 0, 0]);
    assert!(xwm.data_bridge.dnd_incoming.move_delete.is_none());
    assert!(xwm.data_bridge.dnd.incoming_session().is_none());
}

#[test]
fn canonical_retired_before_drop_is_silent_and_after_drop_fails_closed() {
    let (mut before, mut before_peer, before_id, _, _, _, _) = fake_incoming_hover();
    super::super::canonical_retired(&mut before, before_id);
    assert!(before.data_bridge.dnd.incoming_session().is_none());
    assert!(finished_messages(&read_peer(&mut before_peer), &before).is_empty());

    let (mut after, mut after_peer, after_id, _, _, _) = accepted_drop(Action::Copy);
    super::super::canonical_retired(&mut after, after_id);
    super::super::canonical_retired(&mut after, after_id);
    assert!(after.data_bridge.dnd.incoming_session().is_none());
    let failed = finished_messages(&read_peer(&mut after_peer), &after);
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].1, [after.root, 0, 0, 0, 0]);
}

#[test]
fn source_destruction_cancels_before_and_after_drop_without_finished() {
    let (mut before, mut before_peer, before_id, _, _, _, _) = fake_incoming_hover();
    let before_source = before
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .source
        .xid();
    assert!(super::super::source_destroyed(&mut before, before_source).unwrap());
    assert_only_canonical_leave(&mut before, before_id);
    assert!(before.data_bridge.dnd.incoming_session().is_none());
    assert!(finished_messages(&read_peer(&mut before_peer), &before).is_empty());

    let (mut after, mut after_peer, after_id, _, _, _) = accepted_drop(Action::Copy);
    let after_source = after
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .source
        .xid();
    assert!(super::super::source_destroyed(&mut after, after_source).unwrap());
    assert!(matches!(
        after.data_bridge.dnd_incoming.take_events().as_slice(),
        [XwaylandDndIncomingEvent::CancelAfterDrop { offer_id }] if *offer_id == after_id
    ));
    assert!(after.data_bridge.dnd.incoming_session().is_none());
    assert!(finished_messages(&read_peer(&mut after_peer), &after).is_empty());
}

#[test]
fn dropped_terminal_timeout_preserves_queued_success_after_false_cancel_result() {
    let (mut xwm, mut peer, offer_id, _, _, _) = accepted_drop(Action::Copy);
    let deadline = match xwm.data_bridge.dnd.incoming_session().unwrap().wire_phase {
        IncomingDndWirePhase::AwaitingWaylandFinish { deadline_ns, .. } => deadline_ns,
        phase => panic!("expected dropped phase, got {phase:?}"),
    };
    super::super::expire_deadlines(&mut xwm, deadline).unwrap();
    assert!(matches!(
        xwm.data_bridge.dnd.incoming_session().unwrap().wire_phase,
        IncomingDndWirePhase::AwaitingWaylandFinish {
            cancel_submitted: true,
            ..
        }
    ));
    assert!(matches!(
        xwm.data_bridge.dnd_incoming.take_events().as_slice(),
        [XwaylandDndIncomingEvent::CancelAfterDrop { offer_id: cancelled }] if *cancelled == offer_id
    ));

    let queued_transition = crate::xwayland::XwaylandDndTransition::SourceFinished {
        offer_id,
        accepted: true,
        action: Some(Action::Copy),
    };
    super::super::resolve_cancel_after_drop(&mut xwm, offer_id, false).unwrap();

    assert!(matches!(
        xwm.data_bridge.dnd.incoming_session().unwrap().wire_phase,
        IncomingDndWirePhase::AwaitingWaylandFinish {
            cancel_submitted: true,
            ..
        }
    ));
    assert!(finished_messages(&read_peer(&mut peer), &xwm).is_empty());

    super::super::apply_transition(&mut xwm, queued_transition, deadline + 1).unwrap();
    let success = finished_messages(&read_peer(&mut peer), &xwm);
    assert_eq!(success.len(), 1);
    assert_eq!(
        success[0].1,
        [
            xwm.root,
            1,
            xwm.atoms.get(XwmAtomName::XdndActionCopy),
            0,
            0
        ]
    );
    assert!(xwm.data_bridge.dnd.incoming_session().is_none());
}

#[test]
fn dropped_terminal_timeout_fallback_fails_once_without_canonical_completion() {
    let (mut xwm, mut peer, offer_id, _, _, _) = accepted_drop(Action::Copy);
    let first_deadline = match xwm.data_bridge.dnd.incoming_session().unwrap().wire_phase {
        IncomingDndWirePhase::AwaitingWaylandFinish { deadline_ns, .. } => deadline_ns,
        phase => panic!("expected dropped phase, got {phase:?}"),
    };
    super::super::expire_deadlines(&mut xwm, first_deadline).unwrap();
    super::super::resolve_cancel_after_drop(&mut xwm, offer_id, false).unwrap();
    let fallback_deadline = match xwm.data_bridge.dnd.incoming_session().unwrap().wire_phase {
        IncomingDndWirePhase::AwaitingWaylandFinish {
            deadline_ns,
            cancel_submitted: true,
            ..
        } => deadline_ns,
        phase => panic!("expected cancellation fallback, got {phase:?}"),
    };

    super::super::expire_deadlines(&mut xwm, fallback_deadline).unwrap();
    let failure = finished_messages(&read_peer(&mut peer), &xwm);
    assert_eq!(failure.len(), 1);
    assert_eq!(failure[0].1, [xwm.root, 0, 0, 0, 0]);
    assert!(xwm.data_bridge.dnd.incoming_session().is_none());
    super::super::expire_deadlines(&mut xwm, fallback_deadline + 1).unwrap();
    assert!(finished_messages(&read_peer(&mut peer), &xwm).is_empty());
}

#[test]
fn accepted_cancel_waits_for_canonical_failure_without_duplicate_finished() {
    let (mut xwm, mut peer, offer_id, _, _, _) = accepted_drop(Action::Copy);
    let deadline = match xwm.data_bridge.dnd.incoming_session().unwrap().wire_phase {
        IncomingDndWirePhase::AwaitingWaylandFinish { deadline_ns, .. } => deadline_ns,
        phase => panic!("expected dropped phase, got {phase:?}"),
    };
    super::super::expire_deadlines(&mut xwm, deadline).unwrap();
    super::super::resolve_cancel_after_drop(&mut xwm, offer_id, true).unwrap();
    assert!(xwm.data_bridge.dnd.incoming_session().is_some());
    assert!(finished_messages(&read_peer(&mut peer), &xwm).is_empty());

    let retired = crate::xwayland::XwaylandDndTransition::SourceFinished {
        offer_id,
        accepted: false,
        action: None,
    };
    super::super::apply_transition(&mut xwm, retired, deadline + 1).unwrap();
    assert_eq!(
        finished_messages(&read_peer(&mut peer), &xwm)
            .iter()
            .map(|(_, data)| *data)
            .collect::<Vec<_>>(),
        [[xwm.root, 0, 0, 0, 0]]
    );
    super::super::apply_transition(
        &mut xwm,
        crate::xwayland::XwaylandDndTransition::SourceFinished {
            offer_id,
            accepted: false,
            action: None,
        },
        deadline + 2,
    )
    .unwrap();
    assert!(finished_messages(&read_peer(&mut peer), &xwm).is_empty());
}

#[test]
fn root_property_notify_is_consumed_asynchronously_and_loss_is_not_reclaimed() {
    let (mut xwm, mut peer, offer_id, _, _, target_proxy, _) = fake_incoming_hover();
    let root_change = xproto::PropertyNotifyEvent {
        response_type: xproto::PROPERTY_NOTIFY_EVENT,
        sequence: 0,
        window: xwm.root,
        atom: xwm.atoms.get(XwmAtomName::XdndProxy),
        time: 1,
        state: xproto::Property::NEW_VALUE,
    };
    assert!(super::super::property_notify(&mut xwm, root_change, 1).unwrap());
    let (generation, proxy, sequence) = match xwm.data_bridge.dnd_incoming.root_proxy_authority {
        RootProxyAuthority::Verifying {
            generation,
            proxy,
            sequence,
            ..
        } => (generation, proxy, sequence as u16),
        authority => panic!("root PropertyNotify did not start async verification: {authority:?}"),
    };
    assert_eq!(generation, xwm.generation);
    assert_eq!(proxy, target_proxy);
    assert!(
        !xwm.windows
            .contains(X11WindowHandle::new(generation, xwm.root))
    );
    let incoming_enter = client_message(
        &xwm,
        xwm.root,
        XwmAtomName::XdndEnter,
        [0x442, 5 << 24, 0x552, 0, 0],
    );
    assert!(super::super::client_message(&mut xwm, incoming_enter, 1).unwrap());
    assert_eq!(
        xwm.data_bridge.dnd.incoming_session().unwrap().offer_id,
        offer_id
    );
    let request = read_peer(&mut peer);
    assert!(
        request
            .chunks_exact(4)
            .any(|chunk| chunk[0] == xproto::GET_PROPERTY_REQUEST)
    );
    peer.write_all(&get_property_reply(
        sequence,
        u32::from(AtomEnum::WINDOW),
        32,
        &target_proxy.to_ne_bytes(),
    ))
    .unwrap();
    super::poll_replies(&mut xwm, 8, 2).unwrap();
    assert!(root_proxy_is_owned(&xwm));

    let root_change = xproto::PropertyNotifyEvent {
        sequence: 0,
        ..root_change
    };
    assert!(super::super::property_notify(&mut xwm, root_change, 3).unwrap());
    let sequence = match xwm.data_bridge.dnd_incoming.root_proxy_authority {
        RootProxyAuthority::Verifying { sequence, .. } => sequence as u16,
        authority => panic!("replacement check was not queued: {authority:?}"),
    };
    let _verification_request = read_peer(&mut peer);
    let foreign_proxy: Window = 0x777;
    peer.write_all(&get_property_reply(
        sequence,
        u32::from(AtomEnum::WINDOW),
        32,
        &foreign_proxy.to_ne_bytes(),
    ))
    .unwrap();
    super::poll_replies(&mut xwm, 8, 4).unwrap();
    assert!(matches!(
        xwm.data_bridge.dnd_incoming.root_proxy_authority,
        RootProxyAuthority::Lost { generation: lost, proxy: lost_proxy }
            if lost == generation && lost_proxy == target_proxy
    ));
    assert!(xwm.data_bridge.dnd.incoming_session().is_none());
    assert_only_canonical_leave(&mut xwm, offer_id);
    let _ = read_peer(&mut peer);

    assert!(super::super::property_notify(&mut xwm, root_change, 5).unwrap());
    assert!(matches!(
        xwm.data_bridge.dnd_incoming.root_proxy_authority,
        RootProxyAuthority::Lost { .. }
    ));
    assert!(
        read_peer(&mut peer).is_empty(),
        "Lost never starts a reclaim loop"
    );
}

#[test]
fn root_proxy_verification_deadline_fails_closed_without_reclaim() {
    let (mut xwm, mut peer, offer_id, _, _, target_proxy, _) = fake_incoming_hover();
    let root_change = xproto::PropertyNotifyEvent {
        response_type: xproto::PROPERTY_NOTIFY_EVENT,
        sequence: 0,
        window: xwm.root,
        atom: xwm.atoms.get(XwmAtomName::XdndProxy),
        time: 1,
        state: xproto::Property::NEW_VALUE,
    };
    assert!(super::super::property_notify(&mut xwm, root_change, 5).unwrap());
    let deadline_ns = match xwm.data_bridge.dnd_incoming.root_proxy_authority {
        RootProxyAuthority::Verifying {
            generation,
            proxy,
            deadline_ns,
            ..
        } => {
            assert_eq!(generation, xwm.generation);
            assert_eq!(proxy, target_proxy);
            deadline_ns
        }
        authority => panic!("expected bounded ownership verification, got {authority:?}"),
    };
    let _verification_request = read_peer(&mut peer);

    super::super::expire_deadlines(&mut xwm, deadline_ns).unwrap();

    assert!(matches!(
        xwm.data_bridge.dnd_incoming.root_proxy_authority,
        RootProxyAuthority::Lost { generation, proxy }
            if generation == offer_id.generation() && proxy == target_proxy
    ));
    assert!(xwm.data_bridge.dnd.incoming_session().is_none());
    let requests = read_peer(&mut peer);
    assert_eq!(
        proxy_properties(&requests)
            .iter()
            .filter(|(window, property, ..)| {
                *window == xwm.root && *property == xwm.atoms.get(XwmAtomName::XdndProxy)
            })
            .count(),
        0,
        "Lost authority must not publish or reclaim the root proxy"
    );
    assert!(finished_messages(&requests, &xwm).is_empty());
}

#[test]
fn newer_root_property_event_discards_stale_owned_verification() {
    let (mut xwm, mut peer, _, _, _, target_proxy, _) = fake_incoming_hover();
    let first_sequence = start_root_proxy_verification(&mut xwm, 1);
    let _first_request = read_peer(&mut peer);
    send_root_proxy_reply(&mut peer, first_sequence, target_proxy);

    let second_sequence = start_root_proxy_verification(&mut xwm, 2);
    assert_ne!(first_sequence, second_sequence);
    let _second_request = read_peer(&mut peer);
    let foreign_proxy: Window = 0x779;
    send_root_proxy_reply(&mut peer, second_sequence, foreign_proxy);
    super::poll_replies(&mut xwm, 8, 3).unwrap();

    assert!(matches!(
        xwm.data_bridge.dnd_incoming.root_proxy_authority,
        RootProxyAuthority::Lost { proxy, .. } if proxy == target_proxy
    ));
}

#[test]
fn newest_root_property_verification_can_restore_owned_authority() {
    let (mut xwm, mut peer, _, _, _, target_proxy, _) = fake_incoming_hover();
    let first_sequence = start_root_proxy_verification(&mut xwm, 1);
    let _first_request = read_peer(&mut peer);
    send_root_proxy_reply(&mut peer, first_sequence, 0x778);

    let second_sequence = start_root_proxy_verification(&mut xwm, 2);
    assert_ne!(first_sequence, second_sequence);
    let _second_request = read_peer(&mut peer);
    send_root_proxy_reply(&mut peer, second_sequence, target_proxy);
    super::poll_replies(&mut xwm, 8, 3).unwrap();

    assert!(root_proxy_is_owned(&xwm));
}

#[test]
fn repeated_root_property_events_keep_only_the_newest_verification_live() {
    let (mut xwm, mut peer, _, _, _, target_proxy, _) = fake_incoming_hover();
    let mut sequence = start_root_proxy_verification(&mut xwm, 1);
    let _ = read_peer(&mut peer);
    for time in 2..=8 {
        send_root_proxy_reply(&mut peer, sequence, 0x777);
        let next_sequence = start_root_proxy_verification(&mut xwm, time);
        assert_ne!(sequence, next_sequence);
        assert!(matches!(
            xwm.data_bridge.dnd_incoming.root_proxy_authority,
            RootProxyAuthority::Verifying { sequence: current, .. }
                if current == u64::from(next_sequence)
        ));
        let request = read_peer(&mut peer);
        assert_eq!(request.first(), Some(&xproto::GET_PROPERTY_REQUEST));
        sequence = next_sequence;
    }

    send_root_proxy_reply(&mut peer, sequence, target_proxy);
    super::poll_replies(&mut xwm, 16, 9).unwrap();
    assert!(root_proxy_is_owned(&xwm));
}

#[test]
fn root_proxy_loss_after_drop_preserves_drop_submission_and_blocks_new_enters() {
    let (mut xwm, mut peer, offer_id, _, _, target_proxy, _) = fake_incoming_hover();
    assert!(accept_position(&mut xwm, &mut peer, offer_id, Action::Copy));
    let source = xwm.data_bridge.dnd.incoming_session().unwrap().source.xid();
    let drop = drop_message(&xwm, source, DROP_TIME);
    assert!(super::super::client_message(&mut xwm, drop, 40).unwrap());
    assert!(matches!(
        xwm.data_bridge.dnd_incoming.take_events().as_slice(),
        [XwaylandDndIncomingEvent::Drop { offer_id: submitted }] if *submitted == offer_id
    ));
    assert!(matches!(
        xwm.data_bridge.dnd.incoming_session().unwrap().wire_phase,
        IncomingDndWirePhase::DropSubmitted { .. }
    ));

    lose_root_proxy(&mut xwm, &mut peer, 41, 0x779);
    assert!(matches!(
        xwm.data_bridge.dnd_incoming.root_proxy_authority,
        RootProxyAuthority::Lost { proxy, .. } if proxy == target_proxy
    ));
    assert!(matches!(
        xwm.data_bridge.dnd.incoming_session().unwrap().wire_phase,
        IncomingDndWirePhase::DropSubmitted { .. }
    ));
    assert!(xwm.data_bridge.dnd_incoming.take_events().is_empty());

    let new_enter = client_message(
        &xwm,
        xwm.root,
        XwmAtomName::XdndEnter,
        [0x992, 5 << 24, 0x552, 0, 0],
    );
    assert!(super::super::client_message(&mut xwm, new_enter, 42).unwrap());
    assert_eq!(
        xwm.data_bridge.dnd.incoming_session().unwrap().offer_id,
        offer_id
    );
    assert!(xwm.data_bridge.dnd_incoming.take_events().is_empty());

    super::super::resolve_drop(&mut xwm, offer_id, true, 43).unwrap();
    assert!(matches!(
        xwm.data_bridge.dnd.incoming_session().unwrap().wire_phase,
        IncomingDndWirePhase::AwaitingWaylandFinish { .. }
    ));
    let finished = crate::xwayland::XwaylandDndTransition::SourceFinished {
        offer_id,
        accepted: true,
        action: Some(Action::Copy),
    };
    super::super::apply_transition(&mut xwm, finished, 44).unwrap();
    assert_eq!(
        finished_messages(&read_peer(&mut peer), &xwm)
            .iter()
            .map(|(_, data)| *data)
            .collect::<Vec<_>>(),
        [[
            xwm.root,
            1,
            xwm.atoms.get(XwmAtomName::XdndActionCopy),
            0,
            0
        ]]
    );
    assert!(matches!(
        xwm.data_bridge.dnd_incoming.root_proxy_authority,
        RootProxyAuthority::Lost { .. }
    ));
}

#[test]
fn queued_source_finished_beats_cancel_and_root_loss_for_committed_copy() {
    let (mut xwm, mut peer, offer_id, _, _, _) = accepted_drop(Action::Copy);
    let target_proxy = xwm.data_bridge.dnd_incoming.target_proxy.unwrap();
    // The canonical finish is already in the compositor outbox when timeout
    // and the root-property event are processed by the runtime.
    let queued_success = crate::xwayland::XwaylandDndTransition::SourceFinished {
        offer_id,
        accepted: true,
        action: Some(Action::Copy),
    };
    let terminal_deadline = match xwm.data_bridge.dnd.incoming_session().unwrap().wire_phase {
        IncomingDndWirePhase::AwaitingWaylandFinish { deadline_ns, .. } => deadline_ns,
        phase => panic!("expected dropped phase, got {phase:?}"),
    };
    super::super::expire_deadlines(&mut xwm, terminal_deadline).unwrap();
    assert!(matches!(
        xwm.data_bridge.dnd_incoming.take_events().as_slice(),
        [XwaylandDndIncomingEvent::CancelAfterDrop { offer_id: cancelled }]
            if *cancelled == offer_id
    ));

    lose_root_proxy(&mut xwm, &mut peer, 1, 0x779);
    assert!(matches!(
        xwm.data_bridge.dnd_incoming.root_proxy_authority,
        RootProxyAuthority::Lost { proxy, .. } if proxy == target_proxy
    ));
    assert!(matches!(
        xwm.data_bridge.dnd.incoming_session().unwrap().wire_phase,
        IncomingDndWirePhase::AwaitingWaylandFinish {
            cancel_submitted: true,
            ..
        }
    ));

    super::super::resolve_cancel_after_drop(&mut xwm, offer_id, false).unwrap();
    super::super::apply_transition(&mut xwm, queued_success, terminal_deadline + 1).unwrap();

    let finished = finished_messages(&read_peer(&mut peer), &xwm);
    assert_eq!(finished.len(), 1);
    assert_eq!(
        finished[0].1,
        [
            xwm.root,
            1,
            xwm.atoms.get(XwmAtomName::XdndActionCopy),
            0,
            0
        ]
    );
    assert!(matches!(
        xwm.data_bridge.dnd_incoming.root_proxy_authority,
        RootProxyAuthority::Lost { .. }
    ));
    assert!(xwm.data_bridge.dnd.incoming_session().is_none());

    let event = xproto::PropertyNotifyEvent {
        response_type: xproto::PROPERTY_NOTIFY_EVENT,
        sequence: 0,
        window: xwm.root,
        atom: xwm.atoms.get(XwmAtomName::XdndProxy),
        time: 3,
        state: xproto::Property::NEW_VALUE,
    };
    assert!(super::super::property_notify(&mut xwm, event, 3).unwrap());
    assert!(read_peer(&mut peer).is_empty());
}

#[test]
fn move_delete_completes_after_root_proxy_loss_and_lost_rejects_new_enter() {
    let (mut xwm, mut peer, offer_id, _, drop_timestamp, mut server_sequence) =
        accepted_drop(Action::Move);
    super::super::source_finished(&mut xwm, offer_id, true, Some(Action::Move), 50).unwrap();
    let delete = xwm.data_bridge.dnd_incoming.move_delete.unwrap();
    assert!(matches!(
        xwm.data_bridge.dnd.incoming_session().unwrap().wire_phase,
        IncomingDndWirePhase::DeletePending {
            drop_timestamp: timestamp,
            final_action: Action::Move,
        } if timestamp == drop_timestamp
    ));
    let _delete_requests = take_requests(&mut peer, &mut server_sequence);

    lose_root_proxy(&mut xwm, &mut peer, 51, 0x779);
    assert!(matches!(
        xwm.data_bridge.dnd_incoming.root_proxy_authority,
        RootProxyAuthority::Lost { .. }
    ));
    assert!(xwm.data_bridge.dnd_incoming.move_delete.is_some());
    assert!(xwm.data_bridge.dnd.incoming_session().is_some());

    let enter = client_message(
        &xwm,
        xwm.root,
        XwmAtomName::XdndEnter,
        [0x993, 5 << 24, 0x553, 0, 0],
    );
    assert!(super::super::client_message(&mut xwm, enter, 52).unwrap());
    assert_eq!(
        xwm.data_bridge.dnd.incoming_session().unwrap().offer_id,
        offer_id
    );
    assert!(xwm.data_bridge.dnd_incoming.take_events().is_empty());

    let success = selection_notify_event(
        delete.requestor,
        xwm.atoms.get(XwmAtomName::XdndSelection),
        xwm.atoms.get(XwmAtomName::Delete),
        delete.property,
        drop_timestamp,
    );
    assert!(super::super::selection_notify(&mut xwm, success, 53).unwrap());
    let finished = finished_messages(&read_peer(&mut peer), &xwm);
    assert_eq!(finished.len(), 1);
    assert_eq!(
        finished[0].1,
        [
            xwm.root,
            1,
            xwm.atoms.get(XwmAtomName::XdndActionMove),
            0,
            0
        ]
    );
    assert!(xwm.data_bridge.dnd.incoming_session().is_none());
    assert!(xwm.data_bridge.dnd_incoming.move_delete.is_none());
    assert!(matches!(
        xwm.data_bridge.dnd_incoming.root_proxy_authority,
        RootProxyAuthority::Lost { .. }
    ));

    let enter = client_message(
        &xwm,
        xwm.root,
        XwmAtomName::XdndEnter,
        [0x994, 5 << 24, 0x554, 0, 0],
    );
    assert!(super::super::client_message(&mut xwm, enter, 54).unwrap());
    assert!(xwm.data_bridge.dnd.incoming_session().is_none());
    assert!(xwm.data_bridge.dnd_incoming.take_events().is_empty());
    assert!(read_peer(&mut peer).is_empty());
}

#[test]
fn blocked_or_missing_root_authority_consumes_root_messages_without_a_session() {
    let (mut xwm, mut peer, _, _, _, target_proxy, _) = fake_incoming_hover();
    xwm.data_bridge.dnd_incoming.root_proxy_authority = RootProxyAuthority::BlockedForeign {
        generation: xwm.generation,
        proxy: 0x777,
    };
    let enter = client_message(
        &xwm,
        xwm.root,
        XwmAtomName::XdndEnter,
        [0x442, 5 << 24, 0x552, 0, 0],
    );
    assert!(super::super::client_message(&mut xwm, enter, 200).unwrap());
    assert_eq!(
        xwm.data_bridge.dnd_incoming.root_proxy_authority,
        RootProxyAuthority::BlockedForeign {
            generation: xwm.generation,
            proxy: 0x777
        }
    );
    let original_offer = xwm.data_bridge.dnd.incoming_session().unwrap().offer_id;

    xwm.data_bridge.dnd_incoming.target_proxy = None;
    xwm.data_bridge.dnd_incoming.root_proxy_authority = RootProxyAuthority::Unpublished;
    let root_position = client_message(
        &xwm,
        xwm.root,
        XwmAtomName::XdndPosition,
        [
            0x441,
            0,
            0,
            0x123,
            xwm.atoms.get(XwmAtomName::XdndActionCopy),
        ],
    );
    assert!(super::super::client_message(&mut xwm, root_position, 201).unwrap());
    assert_eq!(
        xwm.data_bridge.dnd.incoming_session().unwrap().offer_id,
        original_offer
    );
    xwm.connection.flush().unwrap();
    let _ = read_peer(&mut peer);
    assert!(!xwm.data_bridge.dnd.internal_windows.contains(&xwm.root));
    assert!(xwm.data_bridge.dnd.internal_windows.contains(&target_proxy));
}

#[test]
fn post_drop_conversion_uses_drop_time_without_rewriting_pre_drop_transfer() {
    let (mut xwm, mut peer, offer_id, mime_atom, position_time, _, mut server_sequence) =
        fake_incoming_hover();
    let (_, before_id, _, _) = begin_fake_selection_transfer(
        &mut xwm,
        &mut peer,
        offer_id,
        mime_atom,
        position_time,
        &mut server_sequence,
    );
    assert!(accept_position(&mut xwm, &mut peer, offer_id, Action::Copy));
    submit_drop_and_ack(&mut xwm, offer_id, DROP_TIME, 140);
    let (_, after_id, _, _) = begin_fake_selection_transfer_at(
        &mut xwm,
        &mut peer,
        offer_id,
        mime_atom,
        DROP_TIME,
        &mut server_sequence,
        142,
    );
    assert_eq!(
        transfer_selection_timestamp(&xwm, before_id),
        Some(position_time)
    );
    assert_eq!(
        transfer_selection_timestamp(&xwm, after_id),
        Some(DROP_TIME)
    );
}

#[test]
fn direct_and_incr_payloads_can_finish_after_canonical_drop() {
    let (mut xwm, mut peer, offer_id, mime_atom, timestamp, _, mut server_sequence) =
        fake_incoming_hover();
    accept_position(&mut xwm, &mut peer, offer_id, Action::Copy);
    submit_drop_and_ack(&mut xwm, offer_id, DROP_TIME, 150);
    let (mut direct_reader, direct_id, requestor, property) = begin_fake_selection_transfer_at(
        &mut xwm,
        &mut peer,
        offer_id,
        mime_atom,
        DROP_TIME,
        &mut server_sequence,
        152,
    );
    notify_fake_selection_transfer(
        &mut xwm,
        &mut peer,
        direct_id,
        mime_atom,
        DROP_TIME,
        requestor,
        property,
        server_sequence,
        &mut server_sequence,
    );
    let sequence = transfer_pending_sequence(&xwm, direct_id).unwrap();
    peer.write_all(&get_property_reply(sequence, mime_atom, 8, b"after drop"))
        .unwrap();
    xwm.drain_events(32).unwrap();
    let mut direct_bytes = [0; 10];
    direct_reader.read_exact(&mut direct_bytes).unwrap();
    assert_eq!(&direct_bytes, b"after drop");
    assert!(xwm.data_bridge.dnd.incoming_session().is_some());

    let (mut incr_reader, incr_id, requestor, property) = begin_fake_selection_transfer_at(
        &mut xwm,
        &mut peer,
        offer_id,
        mime_atom,
        DROP_TIME,
        &mut server_sequence,
        154,
    );
    notify_fake_selection_transfer(
        &mut xwm,
        &mut peer,
        incr_id,
        mime_atom,
        DROP_TIME,
        requestor,
        property,
        server_sequence,
        &mut server_sequence,
    );
    let incr_atom = xwm.atoms.get(XwmAtomName::Incr);
    let marker_sequence = transfer_pending_sequence(&xwm, incr_id).unwrap();
    peer.write_all(&get_property_reply(
        marker_sequence,
        incr_atom,
        32,
        &15_u32.to_ne_bytes(),
    ))
    .unwrap();
    xwm.drain_events(32).unwrap();
    assert_eq!(
        xwm.data_bridge.dnd_incoming.transfers[&incr_id].phase,
        IncomingTransferPhase::WaitingForIncrValue
    );
    let _delete_marker = take_requests(&mut peer, &mut server_sequence);
    for chunk in [b"incr ".as_slice(), b"after drop".as_slice()] {
        peer.write_all(&property_new_value(requestor, property, server_sequence))
            .unwrap();
        xwm.drain_events(32).unwrap();
        xwm.connection.flush().unwrap();
        let _property_read = take_requests(&mut peer, &mut server_sequence);
        let sequence = transfer_pending_sequence(&xwm, incr_id).unwrap();
        peer.write_all(&get_property_reply(sequence, mime_atom, 8, chunk))
            .unwrap();
        xwm.drain_events(32).unwrap();
        let mut bytes = vec![0; chunk.len()];
        incr_reader.read_exact(&mut bytes).unwrap();
        assert_eq!(bytes, chunk);
        if transfer_is_active(&xwm, incr_id) {
            let _delete_chunk = take_requests(&mut peer, &mut server_sequence);
        }
    }
    peer.write_all(&property_new_value(requestor, property, server_sequence))
        .unwrap();
    xwm.drain_events(32).unwrap();
    xwm.connection.flush().unwrap();
    let _terminal_read = take_requests(&mut peer, &mut server_sequence);
    let terminal_sequence = transfer_pending_sequence(&xwm, incr_id).unwrap();
    peer.write_all(&get_property_reply(terminal_sequence, mime_atom, 8, b""))
        .unwrap();
    xwm.drain_events(32).unwrap();
    assert!(!transfer_is_active(&xwm, incr_id));
    assert_eq!(timestamp, 0x1234_5678, "pre-Drop Position remains distinct");
}
