use super::*;

#[test]
fn canonical_acceptance_emits_status_for_the_data_four_requested_action() {
    use crate::xwayland::XwaylandDndAction as Action;

    let (mut xwm, mut peer, offer_id, _, _, _, server_sequence) = fake_incoming_hover();
    let timestamp = 0x8091_a2b3;
    let move_atom = xwm.atoms.get(XwmAtomName::XdndActionMove);
    let coordinates = super::super::super::dnd_wire::pack_root_coordinates(-123.0, 456.0).unwrap();
    inject_position(
        &mut xwm,
        &mut peer,
        0x441,
        timestamp,
        move_atom,
        coordinates,
        server_sequence,
    );
    let position = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .latest_position
        .unwrap();
    assert_eq!(position.requested_action, Action::Move);
    assert_eq!(position.timestamp, timestamp);
    assert_eq!((position.root_x, position.root_y), (-123.0, 456.0));
    xwm.data_bridge
        .dnd
        .incoming_session_mut()
        .unwrap()
        .source_actions = vec![Action::Move, Action::Copy];

    super::super::metadata::apply_source_feedback_transition(
        &mut xwm,
        crate::xwayland::XwaylandDndTransition::SourceFeedback {
            offer_id,
            position_id: position.position_id,
            accepted_mime: Some("text/plain".to_owned()),
            action: Some(Action::Move),
        },
    )
    .unwrap();
    xwm.connection.flush().unwrap();
    let statuses = status_messages(&read_peer(&mut peer), &xwm);
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].1[1] & 1, 1);
    assert_eq!(statuses[0].1[4], xwm.atoms.get(XwmAtomName::XdndActionMove));
}
