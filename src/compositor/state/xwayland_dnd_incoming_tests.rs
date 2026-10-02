use super::*;

#[test]
fn source_feedback_coalesces_only_for_exact_offer_and_position() {
    let first_generation = generation(83);
    let offer_id =
        crate::xwayland::XwaylandDndOfferId::new(first_generation, NonZeroU64::new(1).unwrap());
    let replacement_offer_id =
        crate::xwayland::XwaylandDndOfferId::new(generation(84), NonZeroU64::new(1).unwrap());
    let p1 = XwaylandDndIncomingPositionId::new(offer_id, NonZeroU64::new(1).unwrap());
    let p2 = XwaylandDndIncomingPositionId::new(offer_id, NonZeroU64::new(2).unwrap());
    let replacement_p1 =
        XwaylandDndIncomingPositionId::new(replacement_offer_id, NonZeroU64::new(1).unwrap());
    let mut outbox = XwaylandDndOutbox::default();
    for transition in [
        XwaylandDndTransition::SourceFeedback {
            offer_id,
            position_id: p1,
            accepted_mime: Some("text/plain".to_owned()),
            action: Some(XwaylandDndAction::Copy),
        },
        XwaylandDndTransition::SourceFeedback {
            offer_id,
            position_id: p1,
            accepted_mime: None,
            action: None,
        },
        XwaylandDndTransition::SourceFeedback {
            offer_id,
            position_id: p2,
            accepted_mime: Some("text/plain".to_owned()),
            action: Some(XwaylandDndAction::Copy),
        },
        XwaylandDndTransition::SourceFeedback {
            offer_id: replacement_offer_id,
            position_id: replacement_p1,
            accepted_mime: Some("text/uri-list".to_owned()),
            action: Some(XwaylandDndAction::Move),
        },
    ] {
        outbox.push(transition).unwrap();
    }

    let feedback = outbox.drain();
    assert_eq!(feedback.len(), 3);
    assert!(matches!(
        feedback[0],
        XwaylandDndTransition::SourceFeedback {
            offer_id: current,
            position_id,
            accepted_mime: None,
            ..
        } if current == offer_id && position_id == p1
    ));
    assert!(matches!(
        feedback[1],
        XwaylandDndTransition::SourceFeedback {
            offer_id: current,
            position_id,
            ..
        } if current == offer_id && position_id == p2
    ));
    assert!(matches!(
        feedback[2],
        XwaylandDndTransition::SourceFeedback {
            offer_id: current,
            position_id,
            ..
        } if current == replacement_offer_id && position_id == replacement_p1
    ));
}

#[test]
fn incoming_xwayland_position_replaces_source_actions_without_replacing_drag() {
    let display = Display::<CompositorState>::new().expect("test display");
    let (client, _peer) = test_client(&display);
    let mut state = CompositorState::new(None);
    activate_xwayland_generation(&mut state, &client, generation(41));
    let target = state.test_create_surface_resource(
        &client,
        &display.handle(),
        80,
        80,
        SurfacePlacement::absolute_root_at(0, 0),
    );
    state.test_create_data_device(&client, &display.handle());

    let offer = xwayland_offer(15, 0x115, vec![XwaylandDndAction::Copy]);
    let offer_id = offer.id();
    assert!(state.begin_xwayland_drag_session(offer));
    assert!(state.update_incoming_xwayland_drag_position(offer_id, 10.0, 10.0));
    let wayland_offer = active_wayland_offer(&state).expect("incoming Wayland offer");
    let canonical_id = state.active_drag.as_ref().expect("active drag").id;

    assert!(state.update_xwayland_drag_source_actions(
        offer_id,
        vec![XwaylandDndAction::Move, XwaylandDndAction::Copy],
    ));
    assert_eq!(
        state.active_drag.as_ref().map(|active| active.id),
        Some(canonical_id)
    );
    let ActiveDragOrigin::Xwayland { offer } = &state.active_drag.as_ref().unwrap().origin else {
        panic!("incoming XWayland offer remains canonical source");
    };
    assert_eq!(
        offer.source_actions(),
        [XwaylandDndAction::Move, XwaylandDndAction::Copy]
    );
    assert_eq!(
        state
            .data_offers
            .get(&wayland_offer.id())
            .map(|binding| binding.source_actions),
        Some(WaylandDndAction::Move.mask() | WaylandDndAction::Copy.mask()),
    );
    assert!(matches!(
        state.active_drag.as_ref().and_then(|active| active.target.as_ref()),
        Some(ActiveDragTarget::Wayland { surface, .. }) if same_surface_resource(surface, &target)
    ));
}

#[test]
fn incoming_feedback_keeps_the_position_identity_that_wayland_negotiated() {
    let display = Display::<CompositorState>::new().expect("test display");
    let (client, _peer) = test_client(&display);
    let mut state = CompositorState::new(None);
    activate_xwayland_generation(&mut state, &client, generation(41));
    state.test_create_surface_resource(
        &client,
        &display.handle(),
        80,
        80,
        SurfacePlacement::absolute_root_at(0, 0),
    );
    state.test_create_data_device(&client, &display.handle());

    let offer = xwayland_offer(16, 0x116, vec![XwaylandDndAction::Copy]);
    let offer_id = offer.id();
    assert!(state.begin_xwayland_drag_session(offer));
    let p1 = XwaylandDndIncomingPositionId::new(offer_id, NonZeroU64::new(1).unwrap());
    assert!(state.update_xwayland_drag_position_id(offer_id, p1));
    assert!(state.update_incoming_xwayland_drag_position(offer_id, 10.0, 10.0));
    let wayland_offer = active_wayland_offer(&state).expect("incoming Wayland offer");
    state.update_drag_acceptance(&wayland_offer, Some("text/plain".to_owned()));
    state.update_drag_actions(
        &wayland_offer,
        WaylandDndAction::Copy.mask(),
        WaylandDndAction::Copy.mask(),
    );
    let p1_feedback = state.take_xwayland_dnd_transitions();
    assert!(matches!(
        p1_feedback.as_slice(),
        [crate::xwayland::XwaylandDndTransition::SourceFeedback {
            offer_id: current,
            position_id,
            accepted_mime: Some(mime),
            action: Some(XwaylandDndAction::Copy),
        }] if *current == offer_id && *position_id == p1 && mime == "text/plain"
    ));

    let p2 = XwaylandDndIncomingPositionId::new(offer_id, NonZeroU64::new(2).unwrap());
    assert!(state.update_xwayland_drag_position_id(offer_id, p2));
    assert!(state.update_incoming_xwayland_drag_position(offer_id, 20.0, 20.0));
    state.update_drag_acceptance(&wayland_offer, Some("text/uri-list".to_owned()));
    state.update_drag_actions(
        &wayland_offer,
        WaylandDndAction::Copy.mask(),
        WaylandDndAction::Copy.mask(),
    );
    let p2_feedback = state.take_xwayland_dnd_transitions();
    assert!(matches!(
        p2_feedback.as_slice(),
        [crate::xwayland::XwaylandDndTransition::SourceFeedback {
            offer_id: current,
            position_id,
            accepted_mime: Some(mime),
            action: Some(XwaylandDndAction::Copy),
        }] if *current == offer_id && *position_id == p2 && mime == "text/uri-list"
    ));
}
