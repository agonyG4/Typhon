use super::*;

const SOURCE: Window = 0x441;
const MIME_ATOM: Atom = 0x551;

fn pending_mime_offer(
    generation_id: u64,
    entered_at_ns: u64,
) -> (Xwm, UnixStream, XwaylandDndOfferId, u16, u64, u16) {
    let generation = XwaylandGeneration::new(NonZeroU64::new(generation_id).unwrap());
    let (mut xwm, mut peer) = super::super::super::super::test_fixture_for_tests(generation);
    super::super::initialize_target_proxy(&mut xwm).unwrap();
    xwm.connection.flush().unwrap();
    let mut server_sequence = 0;
    super::take_requests(&mut peer, &mut server_sequence);

    super::metadata::begin_enter(&mut xwm, [SOURCE, 5 << 24, MIME_ATOM, 0, 0], entered_at_ns)
        .unwrap();
    xwm.connection.flush().unwrap();
    super::take_requests(&mut peer, &mut server_sequence);

    let offer_id = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .expect("Enter installs an incoming offer")
        .offer_id;
    let (sequence, deadline_ns) = xwm
        .data_bridge
        .dnd_incoming
        .pending
        .iter()
        .find_map(|(sequence, pending)| match pending {
            PendingMetadataReply::AtomName {
                offer_id: pending_offer,
                atom,
                deadline_ns,
            } if *pending_offer == offer_id && *atom == MIME_ATOM => {
                Some((*sequence as u16, *deadline_ns))
            }
            _ => None,
        })
        .expect("inline MIME atom starts one asynchronous GetAtomName");
    (xwm, peer, offer_id, sequence, deadline_ns, server_sequence)
}

fn valid_atom_name_reply(sequence: u16) -> Vec<u8> {
    let name = b"text/plain";
    let mut reply = xproto::GetAtomNameReply {
        sequence,
        length: name.len().div_ceil(4) as u32,
        name: name.to_vec(),
    }
    .serialize();
    reply.resize(32 + name.len().div_ceil(4) * 4, 0);
    reply
}

fn complete_mime_name(xwm: &mut Xwm, peer: &mut UnixStream, sequence: u16, now_ns: u64) {
    peer.write_all(&valid_atom_name_reply(sequence)).unwrap();
    super::metadata::poll_replies(xwm, 16, now_ns).unwrap();
}

fn action_list_query(
    xwm: &Xwm,
    offer_id: XwaylandDndOfferId,
) -> (u16, u64, crate::xwayland::XwaylandDndIncomingPositionId) {
    xwm.data_bridge
        .dnd_incoming
        .pending
        .iter()
        .find_map(|(sequence, pending)| match pending {
            PendingMetadataReply::ActionList {
                offer_id: pending_offer,
                position_id,
                deadline_ns,
            } if *pending_offer == offer_id => Some((*sequence as u16, *deadline_ns, *position_id)),
            _ => None,
        })
        .expect("current Ask Position has one exact ActionList query")
}

fn action_list_reply(
    xwm: &Xwm,
    sequence: u16,
    actions: &[crate::xwayland::XwaylandDndAction],
) -> Vec<u8> {
    let atoms = actions
        .iter()
        .copied()
        .map(|action| super::action_atom_for_test(xwm, action));
    let bytes = atoms
        .flat_map(|atom| atom.to_ne_bytes())
        .collect::<Vec<_>>();
    super::get_property_reply(sequence, u32::from(AtomEnum::ATOM), 32, &bytes)
}

fn position_at(
    xwm: &mut Xwm,
    timestamp: u32,
    action: crate::xwayland::XwaylandDndAction,
    now_ns: u64,
) {
    let action_atom = super::action_atom_for_test(xwm, action);
    super::metadata::position(xwm, [SOURCE, 0, timestamp, action_atom, 0], now_ns).unwrap();
}

fn reject_position_at(
    xwm: &mut Xwm,
    peer: &mut UnixStream,
    deadline_ns: u64,
    server_sequence: &mut u16,
) -> [u32; 5] {
    super::metadata::expire_deadlines(xwm, deadline_ns).unwrap();
    xwm.connection.flush().unwrap();
    let response = super::read_peer(peer);
    *server_sequence = server_sequence.wrapping_add(super::count_requests(&response));
    let statuses = super::status_messages(&response, xwm);
    assert_eq!(statuses.len(), 1, "one Position deadline sends one Status");
    assert_eq!(statuses[0].0, SOURCE);
    assert_eq!(statuses[0].1[0], xwm.root);
    assert_eq!(
        statuses[0].1[1] & 1,
        0,
        "Status deadline rejects the Position"
    );
    statuses[0].1
}

#[test]
fn late_mime_metadata_completes_offer_without_resurrecting_timed_out_position() {
    use crate::xwayland::XwaylandDndAction as Action;

    let entered_at_ns = 20_000_000_000;
    let (
        mut xwm,
        mut peer,
        offer_id,
        atom_name_sequence,
        metadata_deadline_ns,
        mut server_sequence,
    ) = pending_mime_offer(101, entered_at_ns);
    let p1_time = entered_at_ns + 100_000_000;
    position_at(&mut xwm, 0x1001, Action::Copy, p1_time);
    let p1 = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .latest_position
        .unwrap();
    let status_deadline_ns = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .pending_status_deadline_ns
        .unwrap();
    assert!(status_deadline_ns < metadata_deadline_ns);
    reject_position_at(
        &mut xwm,
        &mut peer,
        status_deadline_ns,
        &mut server_sequence,
    );
    assert!(
        !xwm.data_bridge
            .dnd
            .incoming_session()
            .unwrap()
            .status_pending
    );
    assert!(xwm.data_bridge.dnd_incoming.take_events().is_empty());

    complete_mime_name(
        &mut xwm,
        &mut peer,
        atom_name_sequence,
        status_deadline_ns + 100_000_000,
    );
    let session = xwm.data_bridge.dnd.incoming_session().unwrap();
    assert!(
        session.metadata_complete,
        "late MIME data remains offer-scoped"
    );
    assert_eq!(session.mime_types, ["text/plain"]);
    assert_eq!(session.latest_position, Some(p1));
    assert!(!session.canonical_started);
    assert!(
        xwm.data_bridge.dnd_incoming.take_events().is_empty(),
        "completed MIME metadata cannot publish Begin(P1) or Position(P1)"
    );

    let p2_time = status_deadline_ns + 200_000_000;
    position_at(&mut xwm, 0x1002, Action::Copy, p2_time);
    let p2 = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .latest_position
        .unwrap();
    assert_ne!(p1.position_id, p2.position_id);
    assert!(
        xwm.data_bridge
            .dnd
            .incoming_session()
            .unwrap()
            .status_pending
    );
    assert!(matches!(
        xwm.data_bridge.dnd_incoming.take_events().as_slice(),
        [XwaylandDndIncomingEvent::Begin { position_id, offer, .. }]
            if *position_id == p2.position_id
                && offer.id() == offer_id
                && offer.source_actions() == [Action::Copy]
    ));
}

#[test]
fn late_action_list_reply_after_status_timeout_is_inert_and_copy_remains_usable() {
    use crate::xwayland::XwaylandDndAction as Action;

    let (mut xwm, mut peer, offer_id, _, _, _, mut server_sequence) = fake_incoming_hover();
    let ask_time = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .metadata_deadline_ns
        + 1;
    position_at(&mut xwm, 0x2001, Action::Ask, ask_time);
    let (q1, q1_deadline_ns, p1_id) = action_list_query(&xwm, offer_id);
    let p1_status_deadline_ns = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .pending_status_deadline_ns
        .unwrap();
    xwm.connection.flush().unwrap();
    super::take_requests(&mut peer, &mut server_sequence);

    reject_position_at(
        &mut xwm,
        &mut peer,
        p1_status_deadline_ns,
        &mut server_sequence,
    );
    assert!(
        !xwm.data_bridge
            .dnd_incoming
            .pending
            .contains_key(&(q1 as u64)),
        "P1 Status resolution removes Q1"
    );
    assert_eq!(
        xwm.data_bridge
            .dnd
            .incoming_session()
            .unwrap()
            .latest_position
            .unwrap()
            .position_id,
        p1_id
    );
    assert!(
        !xwm.data_bridge
            .dnd_incoming
            .pending
            .contains_key(&(q1 as u64)),
        "resolved P1 relinquishes its exact ActionList request"
    );

    peer.write_all(&action_list_reply(
        &xwm,
        q1,
        &[Action::Ask, Action::Copy, Action::Move],
    ))
    .unwrap();
    super::metadata::poll_replies(&mut xwm, 16, p1_status_deadline_ns + 1).unwrap();
    let session = xwm.data_bridge.dnd.incoming_session().unwrap();
    assert!(session.canonical_started);
    assert_eq!(session.latest_position.unwrap().position_id, p1_id);
    assert!(!session.action_list_cached);
    assert!(xwm.data_bridge.dnd_incoming.take_events().is_empty());
    super::metadata::source_feedback(
        &mut xwm,
        offer_id,
        p1_id,
        Some("text/plain".to_owned()),
        Some(Action::Copy),
    )
    .unwrap();
    xwm.connection.flush().unwrap();
    assert!(
        super::read_peer(&mut peer).is_empty(),
        "wire-resolved P1 cannot be accepted by late SourceFeedback"
    );
    assert!(
        xwm.data_bridge
            .dnd
            .incoming_session()
            .unwrap()
            .accepted_mime
            .is_none()
    );

    super::metadata::expire_deadlines(&mut xwm, q1_deadline_ns).unwrap();
    assert!(
        xwm.data_bridge.dnd.incoming_session().is_some(),
        "obsolete ActionList deadline cannot retire the offer"
    );

    let p2_time = q1_deadline_ns + 1;
    position_at(&mut xwm, 0x2002, Action::Copy, p2_time);
    let p2 = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .latest_position
        .unwrap();
    assert_ne!(p1_id, p2.position_id);
    assert!(matches!(
        xwm.data_bridge.dnd_incoming.take_events().as_slice(),
        [XwaylandDndIncomingEvent::Position { position_id, .. }]
            if *position_id == p2.position_id
    ));
    super::metadata::source_feedback(
        &mut xwm,
        offer_id,
        p2.position_id,
        Some("text/plain".to_owned()),
        Some(Action::Copy),
    )
    .unwrap();
    xwm.connection.flush().unwrap();
    let p2_status = super::status_messages(&super::read_peer(&mut peer), &xwm);
    assert_eq!(p2_status.len(), 1);
    assert_eq!(p2_status[0].1[1] & 1, 1);
}

#[test]
fn malformed_late_action_list_reply_after_status_timeout_keeps_offer_alive() {
    let (mut xwm, mut peer, offer_id, _, _, _, mut server_sequence) = fake_incoming_hover();
    let ask_time = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .metadata_deadline_ns
        + 1;
    position_at(
        &mut xwm,
        0x3001,
        crate::xwayland::XwaylandDndAction::Ask,
        ask_time,
    );
    let (q1, _, p1_id) = action_list_query(&xwm, offer_id);
    let status_deadline_ns = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .pending_status_deadline_ns
        .unwrap();
    xwm.connection.flush().unwrap();
    super::take_requests(&mut peer, &mut server_sequence);
    reject_position_at(
        &mut xwm,
        &mut peer,
        status_deadline_ns,
        &mut server_sequence,
    );

    peer.write_all(&super::get_property_reply(
        q1,
        u32::from(AtomEnum::ATOM),
        8,
        b"bad",
    ))
    .unwrap();
    super::metadata::poll_replies(&mut xwm, 16, status_deadline_ns + 1).unwrap();
    let session = xwm.data_bridge.dnd.incoming_session().unwrap();
    assert_eq!(session.offer_id, offer_id);
    assert_eq!(session.latest_position.unwrap().position_id, p1_id);
    assert!(session.canonical_started);
    assert!(!session.action_list_cached);
    assert!(xwm.data_bridge.dnd_incoming.take_events().is_empty());
}

#[test]
fn later_ask_gets_fresh_action_list_authority_after_timed_out_ask() {
    use crate::xwayland::XwaylandDndAction as Action;

    let (mut xwm, mut peer, offer_id, _, _, _, mut server_sequence) = fake_incoming_hover();
    let ask_atom = xwm.atoms.get(XwmAtomName::XdndActionAsk);
    let copy_atom = xwm.atoms.get(XwmAtomName::XdndActionCopy);
    let ask1_time = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .metadata_deadline_ns
        + 1;
    super::metadata::position(&mut xwm, [SOURCE, 0, 0x4001, ask_atom, 0], ask1_time).unwrap();
    let (q1, q1_deadline_ns, p1_id) = action_list_query(&xwm, offer_id);
    let p1_status_deadline_ns = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .pending_status_deadline_ns
        .unwrap();
    xwm.connection.flush().unwrap();
    super::take_requests(&mut peer, &mut server_sequence);
    reject_position_at(
        &mut xwm,
        &mut peer,
        p1_status_deadline_ns,
        &mut server_sequence,
    );
    assert!(
        !xwm.data_bridge
            .dnd_incoming
            .pending
            .contains_key(&(q1 as u64)),
        "P1 Status removes Q1 before P3 starts a fresh Ask query"
    );

    let p2_time = p1_status_deadline_ns + 1;
    super::metadata::position(&mut xwm, [SOURCE, 0, 0x4002, copy_atom, 0], p2_time).unwrap();
    let p2_id = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .latest_position
        .unwrap()
        .position_id;
    assert_ne!(p1_id, p2_id);
    assert!(matches!(
        xwm.data_bridge.dnd_incoming.take_events().as_slice(),
        [XwaylandDndIncomingEvent::Position { position_id, .. }]
            if *position_id == p2_id
    ));

    let ask3_time = q1_deadline_ns + 100_000_000;
    super::metadata::position(&mut xwm, [SOURCE, 0, 0x4003, ask_atom, 0], ask3_time).unwrap();
    let (q2, q2_deadline_ns, p3_id) = action_list_query(&xwm, offer_id);
    assert_ne!(q1, q2);
    assert_ne!(p2_id, p3_id);
    assert_eq!(
        q2_deadline_ns,
        ask3_time + super::TARGET_METADATA_TIMEOUT_NS,
        "P3 starts a fresh bounded query deadline"
    );
    xwm.connection.flush().unwrap();
    super::take_requests(&mut peer, &mut server_sequence);

    peer.write_all(&action_list_reply(
        &xwm,
        q1,
        &[Action::Ask, Action::Copy, Action::Move],
    ))
    .unwrap();
    super::metadata::poll_replies(&mut xwm, 16, ask3_time + 1).unwrap();
    let session = xwm.data_bridge.dnd.incoming_session().unwrap();
    assert_eq!(session.latest_position.unwrap().position_id, p3_id);
    assert!(!session.action_list_cached);
    assert!(
        xwm.data_bridge
            .dnd_incoming
            .pending
            .contains_key(&(q2 as u64))
    );
    assert!(xwm.data_bridge.dnd_incoming.take_events().is_empty());

    peer.write_all(&action_list_reply(
        &xwm,
        q2,
        &[Action::Ask, Action::Copy, Action::Move],
    ))
    .unwrap();
    super::metadata::poll_replies(&mut xwm, 16, ask3_time + 2).unwrap();
    assert!(matches!(
        xwm.data_bridge.dnd_incoming.take_events().as_slice(),
        [XwaylandDndIncomingEvent::Position {
            position_id,
            source_actions,
            ..
        }] if *position_id == p3_id
            && source_actions == &[Action::Ask, Action::Copy, Action::Move]
    ));
}

#[test]
fn canonical_started_session_does_not_publish_wire_resolved_ask_position() {
    use crate::xwayland::XwaylandDndAction as Action;

    let (mut xwm, mut peer, offer_id, _, _, _, mut server_sequence) = fake_incoming_hover();
    let ask_time = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .metadata_deadline_ns
        + 1;
    position_at(&mut xwm, 0x5001, Action::Ask, ask_time);
    let (q1, q1_deadline_ns, p1_id) = action_list_query(&xwm, offer_id);
    let status_deadline_ns = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .pending_status_deadline_ns
        .unwrap();
    xwm.connection.flush().unwrap();
    super::take_requests(&mut peer, &mut server_sequence);
    reject_position_at(
        &mut xwm,
        &mut peer,
        status_deadline_ns,
        &mut server_sequence,
    );
    super::metadata::expire_deadlines(&mut xwm, q1_deadline_ns).unwrap();
    assert!(
        xwm.data_bridge.dnd.incoming_session().is_some(),
        "Q1 metadata deadline cannot retire a canonical drag after P1 Status"
    );

    peer.write_all(&action_list_reply(
        &xwm,
        q1,
        &[Action::Ask, Action::Copy, Action::Move],
    ))
    .unwrap();
    super::metadata::poll_replies(&mut xwm, 16, q1_deadline_ns).unwrap();
    let session = xwm.data_bridge.dnd.incoming_session().unwrap();
    assert!(
        session.canonical_started,
        "the existing canonical drag survives"
    );
    assert_eq!(session.latest_position.unwrap().position_id, p1_id);
    assert!(!session.action_list_cached);
    assert!(xwm.data_bridge.dnd_incoming.take_events().is_empty());

    let p2_time = q1_deadline_ns + 1;
    position_at(&mut xwm, 0x5002, Action::Copy, p2_time);
    let p2_id = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .latest_position
        .unwrap()
        .position_id;
    assert_ne!(p1_id, p2_id);
    assert!(matches!(
        xwm.data_bridge.dnd_incoming.take_events().as_slice(),
        [XwaylandDndIncomingEvent::Position { position_id, .. }]
            if *position_id == p2_id
    ));
}

#[test]
fn timed_out_ask_copy_transfer_and_later_position_keep_lifetimes_separate() {
    use crate::xwayland::XwaylandDndAction as Action;

    let entered_at_ns = 30_000_000_000;
    let (mut xwm, mut peer, offer_id, atom_name_sequence, _, mut server_sequence) =
        pending_mime_offer(106, entered_at_ns);
    complete_mime_name(
        &mut xwm,
        &mut peer,
        atom_name_sequence,
        entered_at_ns + 100_000_000,
    );
    let ask1_time = entered_at_ns + 200_000_000;
    position_at(&mut xwm, 0x6001, Action::Ask, ask1_time);
    let (q1, _, p1_id) = action_list_query(&xwm, offer_id);
    let p1_status_deadline_ns = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .pending_status_deadline_ns
        .unwrap();
    xwm.connection.flush().unwrap();
    super::take_requests(&mut peer, &mut server_sequence);
    reject_position_at(
        &mut xwm,
        &mut peer,
        p1_status_deadline_ns,
        &mut server_sequence,
    );

    peer.write_all(&action_list_reply(
        &xwm,
        q1,
        &[Action::Ask, Action::Copy, Action::Move],
    ))
    .unwrap();
    super::metadata::poll_replies(&mut xwm, 16, p1_status_deadline_ns + 1).unwrap();
    assert!(xwm.data_bridge.dnd_incoming.take_events().is_empty());
    assert!(
        !xwm.data_bridge
            .dnd
            .incoming_session()
            .unwrap()
            .canonical_started
    );

    let p2_timestamp = 0x6002;
    let p2_time = p1_status_deadline_ns + 100_000_000;
    position_at(&mut xwm, p2_timestamp, Action::Copy, p2_time);
    let p2_id = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .latest_position
        .unwrap()
        .position_id;
    assert_ne!(p1_id, p2_id);
    assert!(matches!(
        xwm.data_bridge.dnd_incoming.take_events().as_slice(),
        [XwaylandDndIncomingEvent::Begin { position_id, .. }]
            if *position_id == p2_id
    ));
    super::metadata::source_feedback(
        &mut xwm,
        offer_id,
        p2_id,
        Some("text/plain".to_owned()),
        Some(Action::Copy),
    )
    .unwrap();
    xwm.connection.flush().unwrap();
    let p2_status_bytes = super::read_peer(&mut peer);
    let p2_status = super::status_messages(&p2_status_bytes, &xwm);
    assert_eq!(p2_status.len(), 1);
    assert_eq!(p2_status[0].1[1] & 1, 1, "P2 is accepted");
    server_sequence = server_sequence.wrapping_add(super::count_requests(&p2_status_bytes));

    let (mut reader, transfer_id, requestor, property) = super::begin_fake_selection_transfer_at(
        &mut xwm,
        &mut peer,
        offer_id,
        MIME_ATOM,
        p2_timestamp,
        &mut server_sequence,
        p2_time,
    );
    assert_eq!(
        xwm.data_bridge.dnd_incoming.transfers[&transfer_id].selection_timestamp,
        p2_timestamp
    );

    let p3_timestamp = 0x6003;
    let p3_time = p2_time + 100_000_000;
    position_at(&mut xwm, p3_timestamp, Action::Copy, p3_time);
    let p3_id = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .latest_position
        .unwrap()
        .position_id;
    assert_ne!(p2_id, p3_id);
    assert_eq!(
        xwm.data_bridge.dnd_incoming.transfers[&transfer_id].selection_timestamp, p2_timestamp,
        "P3 does not rewrite P2's transfer timestamp"
    );
    assert!(matches!(
        xwm.data_bridge.dnd_incoming.take_events().as_slice(),
        [XwaylandDndIncomingEvent::Position { position_id, .. }]
            if *position_id == p3_id
    ));
    super::metadata::source_feedback(
        &mut xwm,
        offer_id,
        p3_id,
        Some("text/plain".to_owned()),
        Some(Action::Copy),
    )
    .unwrap();
    xwm.connection.flush().unwrap();
    let p3_status_bytes = super::read_peer(&mut peer);
    let p3_status = super::status_messages(&p3_status_bytes, &xwm);
    assert_eq!(p3_status.len(), 1);
    assert_eq!(p3_status[0].1[1] & 1, 1, "P3 receives its own acceptance");
    assert_eq!(
        p3_status[0].1[4],
        xwm.atoms.get(XwmAtomName::XdndActionCopy)
    );
    server_sequence = server_sequence.wrapping_add(super::count_requests(&p3_status_bytes));

    super::notify_fake_selection_transfer(
        &mut xwm,
        &mut peer,
        transfer_id,
        MIME_ATOM,
        p2_timestamp,
        requestor,
        property,
        server_sequence,
        &mut server_sequence,
    );
    let sequence = xwm.data_bridge.dnd_incoming.transfers[&transfer_id]
        .pending_reply
        .unwrap() as u16;
    peer.write_all(&super::get_property_reply(
        sequence,
        MIME_ATOM,
        8,
        b"P2 payload",
    ))
    .unwrap();
    xwm.drain_events(32).unwrap();
    let mut received = [0; 10];
    reader.read_exact(&mut received).unwrap();
    assert_eq!(&received, b"P2 payload");
}
