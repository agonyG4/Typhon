use super::*;

#[test]
fn action_list_queries_follow_the_current_ask_and_receive_a_fresh_deadline() {
    use crate::xwayland::XwaylandDndAction as Action;

    let (mut xwm, mut peer, offer_id, _, _, _, _) = fake_incoming_hover();
    let enter_deadline = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .metadata_deadline_ns;
    let mut server_sequence = 0;
    let ask_time = enter_deadline.saturating_add(1);
    position_at_for_test(&mut xwm, 0x2001, Action::Ask, 0, 0, ask_time).unwrap();
    let (q1, q1_deadline, p1) = xwm
        .data_bridge
        .dnd_incoming
        .pending
        .iter()
        .find_map(|(sequence, pending)| match pending {
            PendingMetadataReply::ActionList {
                offer_id: pending_offer,
                deadline_ns,
                ..
            } if *pending_offer == offer_id => Some((
                *sequence,
                *deadline_ns,
                xwm.data_bridge
                    .dnd
                    .incoming_session()
                    .unwrap()
                    .latest_position
                    .unwrap()
                    .position_id,
            )),
            _ => None,
        })
        .expect("Ask Position starts one ActionList request");
    assert!(
        q1_deadline > ask_time,
        "dynamic ActionList deadline starts when Ask first requires it"
    );
    assert_eq!(p1.offer_id(), offer_id);

    xwm.connection.flush().unwrap();
    let _q1_requests = take_requests(&mut peer, &mut server_sequence);
    let copy_time = ask_time.saturating_add(100_000_000);
    position_at_for_test(&mut xwm, 0x2002, Action::Copy, 0, 0, copy_time).unwrap();
    let p2 = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .latest_position
        .unwrap();
    assert_ne!(p1, p2.position_id);
    assert!(
        !xwm.data_bridge.dnd_incoming.pending.contains_key(&q1),
        "P2 no longer needs Q1, so its exact pending request is discarded"
    );
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
    let p2_status = status_messages(&read_peer(&mut peer), &xwm);
    assert_eq!(p2_status.len(), 1);
    assert_eq!(
        p2_status[0].1[1] & 1,
        1,
        "P2 Copy remains active and accepted"
    );

    super::metadata::poll_replies(&mut xwm, 16, q1_deadline).unwrap();
    assert_eq!(
        xwm.data_bridge
            .dnd
            .incoming_session()
            .unwrap()
            .latest_position,
        Some(p2),
        "Q1 timeout cannot retire the later Copy Position"
    );
    peer.write_all(&get_property_reply(
        q1 as u16,
        u32::from(AtomEnum::ATOM),
        8,
        b"bad",
    ))
    .unwrap();
    xwm.drain_events(16).unwrap();
    assert_eq!(
        xwm.data_bridge
            .dnd
            .incoming_session()
            .unwrap()
            .latest_position,
        Some(p2),
        "a late malformed Q1 reply cannot retire P2"
    );

    let p3_time =
        enter_deadline.saturating_add(super::TARGET_METADATA_TIMEOUT_NS.saturating_mul(4));
    position_at_for_test(&mut xwm, 0x2003, Action::Ask, 0, 0, p3_time).unwrap();
    let (q2, q2_deadline, p3) = xwm
        .data_bridge
        .dnd_incoming
        .pending
        .iter()
        .find_map(|(sequence, pending)| match pending {
            PendingMetadataReply::ActionList {
                offer_id: pending_offer,
                deadline_ns,
                ..
            } if *pending_offer == offer_id => Some((
                *sequence,
                *deadline_ns,
                xwm.data_bridge
                    .dnd
                    .incoming_session()
                    .unwrap()
                    .latest_position
                    .unwrap()
                    .position_id,
            )),
            _ => None,
        })
        .expect("later Ask starts a fresh ActionList request");
    assert_ne!(p2.position_id, p3);
    assert_eq!(q2_deadline, p3_time + super::TARGET_METADATA_TIMEOUT_NS);
    assert!(q2_deadline > p3_time, "P3 query is not immediately expired");
    xwm.connection.flush().unwrap();
    let _q2_requests = take_requests(&mut peer, &mut server_sequence);

    let action_atoms = [
        xwm.atoms.get(XwmAtomName::XdndActionAsk),
        xwm.atoms.get(XwmAtomName::XdndActionCopy),
        xwm.atoms.get(XwmAtomName::XdndActionMove),
    ];
    let action_bytes = action_atoms
        .into_iter()
        .flat_map(|atom| atom.to_ne_bytes())
        .collect::<Vec<_>>();
    peer.write_all(&get_property_reply(
        q2 as u16,
        u32::from(AtomEnum::ATOM),
        32,
        &action_bytes,
    ))
    .unwrap();
    super::metadata::poll_replies(&mut xwm, 16, p3_time + 1).unwrap();
    let p3_events = xwm.data_bridge.dnd_incoming.take_events();
    assert!(matches!(
        p3_events.as_slice(),
        [XwaylandDndIncomingEvent::Position {
            position_id,
            source_actions,
            ..
        }] if *position_id == p3
            && source_actions == &[Action::Ask, Action::Copy, Action::Move]
    ));
    assert_eq!(
        xwm.data_bridge
            .dnd
            .incoming_session()
            .unwrap()
            .available_actions,
        [Action::Ask, Action::Copy, Action::Move]
    );
    super::metadata::source_feedback(
        &mut xwm,
        offer_id,
        p3,
        Some("text/plain".to_owned()),
        Some(Action::Copy),
    )
    .unwrap();
    xwm.connection.flush().unwrap();
    let p3_status = status_messages(&read_peer(&mut peer), &xwm);
    assert_eq!(p3_status.len(), 1);
    assert_eq!(p3_status[0].1[1] & 1, 1);
    assert_eq!(
        p3_status[0].1[4],
        xwm.atoms.get(XwmAtomName::XdndActionCopy)
    );
}
