use super::*;
use crate::compositor::state::xwayland_dnd::XwaylandIncomingPositionDisposition;
use crate::xwayland::XwaylandDndIncomingEvent;
use std::io::Write as _;
use x11rb::{connection::Connection as _, protocol::xproto::AtomEnum};

fn take_wayland_events(peer: &mut UnixStream) -> Vec<(u32, u16, Vec<u8>)> {
    peer.set_nonblocking(true).expect("nonblocking test client");
    let mut bytes = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        match std::io::Read::read(peer, &mut buffer) {
            Ok(0) => break,
            Ok(length) => bytes.extend_from_slice(&buffer[..length]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) => panic!("read Wayland test events: {error}"),
        }
    }
    let mut events = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        assert!(offset + 8 <= bytes.len(), "complete Wayland event header");
        let object_id = u32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap());
        let word = u32::from_ne_bytes(bytes[offset + 4..offset + 8].try_into().unwrap());
        let size = (word >> 16) as usize;
        assert!(
            size >= 8 && offset + size <= bytes.len(),
            "complete Wayland event"
        );
        events.push((
            object_id,
            word as u16,
            bytes[offset + 8..offset + size].to_vec(),
        ));
        offset += size;
    }
    events
}

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
    let p1 = XwaylandDndIncomingPositionId::new(offer_id, NonZeroU64::new(1).unwrap());
    assert_eq!(
        state.apply_incoming_xwayland_position(
            offer_id,
            p1,
            vec![XwaylandDndAction::Copy],
            10.0,
            10.0,
        ),
        Some(XwaylandIncomingPositionDisposition::EnteredNewWaylandTarget)
    );
    let wayland_offer = active_wayland_offer(&state).expect("incoming Wayland offer");
    let canonical_id = state.active_drag.as_ref().expect("active drag").id;

    let p2 = XwaylandDndIncomingPositionId::new(offer_id, NonZeroU64::new(2).unwrap());
    assert_eq!(
        state.apply_incoming_xwayland_position(
            offer_id,
            p2,
            vec![XwaylandDndAction::Move, XwaylandDndAction::Copy],
            10.0,
            10.0,
        ),
        Some(XwaylandIncomingPositionDisposition::SameWaylandTarget)
    );
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
    let mut display = Display::<CompositorState>::new().expect("test display");
    let (client, mut peer) = test_client(&display);
    let mut state = CompositorState::new(None);
    activate_xwayland_generation(&mut state, &client, generation(41));
    let surface_a = state.test_create_surface_resource(
        &client,
        &display.handle(),
        80,
        80,
        SurfacePlacement::absolute_root_at(0, 0),
    );
    let device = state.test_create_data_device(&client, &display.handle());

    let offer = xwayland_offer(16, 0x116, vec![XwaylandDndAction::Copy]);
    let offer_id = offer.id();
    assert!(state.begin_xwayland_drag_session(offer));
    let p1 = XwaylandDndIncomingPositionId::new(offer_id, NonZeroU64::new(1).unwrap());
    assert_eq!(
        state.apply_incoming_xwayland_position(
            offer_id,
            p1,
            vec![XwaylandDndAction::Copy],
            10.0,
            10.0,
        ),
        Some(XwaylandIncomingPositionDisposition::EnteredNewWaylandTarget)
    );
    assert!(matches!(
        state.active_drag.as_ref().and_then(|active| active.target.as_ref()),
        Some(ActiveDragTarget::Wayland { surface, .. })
            if same_surface_resource(surface, &surface_a)
    ));
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
    let original_offer_id = wayland_offer.id();
    display.flush_clients().unwrap();
    let _p1_wire_events = take_wayland_events(&mut peer);

    let p2 = XwaylandDndIncomingPositionId::new(offer_id, NonZeroU64::new(2).unwrap());
    assert_eq!(
        state.apply_incoming_xwayland_position(
            offer_id,
            p2,
            vec![XwaylandDndAction::Copy],
            20.0,
            20.0,
        ),
        Some(XwaylandIncomingPositionDisposition::SameWaylandTarget)
    );
    assert_eq!(
        active_wayland_offer(&state).unwrap().id(),
        original_offer_id
    );
    let p2_feedback = state.take_xwayland_dnd_transitions();
    assert!(matches!(
        p2_feedback.as_slice(),
        [crate::xwayland::XwaylandDndTransition::SourceFeedback {
            offer_id: current,
            position_id,
            accepted_mime: Some(mime),
            action: Some(XwaylandDndAction::Copy),
        }] if *current == offer_id && *position_id == p2 && mime == "text/plain"
    ));
    display.flush_clients().unwrap();
    let p2_events = take_wayland_events(&mut peer);
    assert_eq!(
        p2_events
            .iter()
            .filter_map(
                |(object_id, opcode, _)| (*object_id == device.id().protocol_id())
                    .then_some(*opcode)
            )
            .collect::<Vec<_>>(),
        [3],
        "A→A emits motion without leave/enter or a replacement offer"
    );
    assert!(p2_events.iter().any(|(object_id, opcode, _)| {
        *object_id == original_offer_id.protocol_id() && *opcode == 1
    }));
}

#[test]
fn incoming_position_retarget_does_not_reject_before_the_new_target_can_accept() {
    let mut display = Display::<CompositorState>::new().expect("test display");
    let (client_a, mut peer_a) = test_client(&display);
    let (client_b, mut peer_b) = test_client(&display);
    let mut state = CompositorState::new(None);
    activate_xwayland_generation(&mut state, &client_a, generation(41));
    let surface_a = state.test_create_surface_resource(
        &client_a,
        &display.handle(),
        80,
        80,
        SurfacePlacement::absolute_root_at(0, 0),
    );
    let surface_b = state.test_create_surface_resource(
        &client_b,
        &display.handle(),
        80,
        80,
        SurfacePlacement::absolute_root_at(100, 0),
    );
    let device_a = state.test_create_data_device(&client_a, &display.handle());
    let device_b = state.test_create_data_device(&client_b, &display.handle());

    let offer = xwayland_offer(34, 0x134, vec![XwaylandDndAction::Copy]);
    let offer_id = offer.id();
    assert!(state.begin_xwayland_drag_session(offer));
    let p1 = XwaylandDndIncomingPositionId::new(offer_id, NonZeroU64::new(1).unwrap());
    assert_eq!(
        state.apply_incoming_xwayland_position(
            offer_id,
            p1,
            vec![XwaylandDndAction::Copy],
            10.0,
            10.0,
        ),
        Some(XwaylandIncomingPositionDisposition::EnteredNewWaylandTarget)
    );
    assert!(matches!(
        state.active_drag.as_ref().and_then(|active| active.target.as_ref()),
        Some(ActiveDragTarget::Wayland { surface, .. })
            if same_surface_resource(surface, &surface_a)
    ));
    let offer_a = active_wayland_offer(&state).expect("P1 enters target A");
    state.update_drag_acceptance(&offer_a, Some("text/plain".to_owned()));
    state.update_drag_actions(
        &offer_a,
        WaylandDndAction::Copy.mask(),
        WaylandDndAction::Copy.mask(),
    );
    assert!(matches!(
        state.take_xwayland_dnd_transitions().as_slice(),
        [XwaylandDndTransition::SourceFeedback {
            position_id,
            accepted_mime: Some(mime),
            action: Some(XwaylandDndAction::Copy),
            ..
        }] if *position_id == p1 && mime == "text/plain"
    ));
    display.flush_clients().unwrap();
    let initial_a_events = take_wayland_events(&mut peer_a);
    assert_eq!(
        initial_a_events
            .iter()
            .filter_map(
                |(object_id, opcode, _)| (*object_id == device_a.id().protocol_id())
                    .then_some(*opcode)
            )
            .collect::<Vec<_>>(),
        [0, 1],
        "target A receives data_offer before enter for P1"
    );

    let p2 = XwaylandDndIncomingPositionId::new(offer_id, NonZeroU64::new(2).unwrap());
    assert_eq!(
        state.apply_incoming_xwayland_position(
            offer_id,
            p2,
            vec![XwaylandDndAction::Move, XwaylandDndAction::Copy],
            120.0,
            10.0,
        ),
        Some(XwaylandIncomingPositionDisposition::EnteredNewWaylandTarget)
    );

    assert!(matches!(
        state.active_drag.as_ref().and_then(|active| active.target.as_ref()),
        Some(ActiveDragTarget::Wayland { surface, .. })
            if same_surface_resource(surface, &surface_b)
    ));
    let offer_b = active_wayland_offer(&state).expect("P2 enters target B");
    assert_eq!(
        state
            .data_offers
            .get(&offer_b.id())
            .map(|binding| binding.source_actions),
        Some(WaylandDndAction::Move.mask() | WaylandDndAction::Copy.mask()),
        "B's offer carries P2's source actions"
    );
    assert!(
        state.take_xwayland_dnd_transitions().is_empty(),
        "leaving A must not publish accepted A feedback or a synthetic P2 rejection"
    );
    display.flush_clients().unwrap();
    let a_position_events = take_wayland_events(&mut peer_a);
    let b_position_events = take_wayland_events(&mut peer_b);
    assert_eq!(
        a_position_events
            .iter()
            .filter_map(
                |(object_id, opcode, _)| (*object_id == device_a.id().protocol_id())
                    .then_some(*opcode)
            )
            .collect::<Vec<_>>(),
        [2],
        "target A receives leave"
    );
    assert_eq!(
        b_position_events
            .iter()
            .filter_map(
                |(object_id, opcode, _)| (*object_id == device_b.id().protocol_id())
                    .then_some(*opcode)
            )
            .collect::<Vec<_>>(),
        [0, 1],
        "target B receives data_offer before enter"
    );
    assert!(
        b_position_events
            .iter()
            .any(|(object_id, opcode, payload)| {
                *object_id == offer_b.id().protocol_id()
                    && *opcode == 1
                    && u32::from_ne_bytes(payload[..4].try_into().unwrap())
                        == (WaylandDndAction::Move.mask() | WaylandDndAction::Copy.mask())
            })
    );

    state.update_drag_acceptance(&offer_b, Some("text/plain".to_owned()));
    state.update_drag_actions(
        &offer_b,
        WaylandDndAction::Copy.mask(),
        WaylandDndAction::Copy.mask(),
    );
    assert!(matches!(
        state.take_xwayland_dnd_transitions().as_slice(),
        [XwaylandDndTransition::SourceFeedback {
            position_id,
            accepted_mime: Some(mime),
            action: Some(XwaylandDndAction::Copy),
            ..
        }] if *position_id == p2 && mime == "text/plain"
    ));
}

#[test]
fn incoming_position_without_a_wayland_target_publishes_one_rejection() {
    let mut display = Display::<CompositorState>::new().expect("test display");
    let (client, mut peer) = test_client(&display);
    let mut state = CompositorState::new(None);
    activate_xwayland_generation(&mut state, &client, generation(41));
    state.test_create_surface_resource(
        &client,
        &display.handle(),
        80,
        80,
        SurfacePlacement::absolute_root_at(0, 0),
    );
    let device = state.test_create_data_device(&client, &display.handle());
    let offer = xwayland_offer(35, 0x135, vec![XwaylandDndAction::Copy]);
    let offer_id = offer.id();
    assert!(state.begin_xwayland_drag_session(offer));
    let p1 = XwaylandDndIncomingPositionId::new(offer_id, NonZeroU64::new(1).unwrap());
    assert_eq!(
        state.apply_incoming_xwayland_position(
            offer_id,
            p1,
            vec![XwaylandDndAction::Copy],
            10.0,
            10.0,
        ),
        Some(XwaylandIncomingPositionDisposition::EnteredNewWaylandTarget)
    );
    let offer_a = active_wayland_offer(&state).expect("P1 enters target A");
    state.update_drag_acceptance(&offer_a, Some("text/plain".to_owned()));
    state.update_drag_actions(
        &offer_a,
        WaylandDndAction::Copy.mask(),
        WaylandDndAction::Copy.mask(),
    );
    let _p1_feedback = state.take_xwayland_dnd_transitions();
    display.flush_clients().unwrap();
    let _p1_events = take_wayland_events(&mut peer);

    let p2 = XwaylandDndIncomingPositionId::new(offer_id, NonZeroU64::new(2).unwrap());
    assert_eq!(
        state.apply_incoming_xwayland_position(
            offer_id,
            p2,
            vec![XwaylandDndAction::Copy],
            500.0,
            500.0,
        ),
        Some(XwaylandIncomingPositionDisposition::NoWaylandTarget)
    );
    assert!(
        state
            .active_drag
            .as_ref()
            .is_some_and(|active| active.target.is_none())
    );
    assert!(matches!(
        state.take_xwayland_dnd_transitions().as_slice(),
        [XwaylandDndTransition::SourceFeedback {
            position_id,
            accepted_mime: None,
            action: None,
            ..
        }] if *position_id == p2
    ));
    display.flush_clients().unwrap();
    assert_eq!(
        take_wayland_events(&mut peer)
            .iter()
            .filter_map(
                |(object_id, opcode, _)| (*object_id == device.id().protocol_id())
                    .then_some(*opcode)
            )
            .collect::<Vec<_>>(),
        [2],
        "target A receives leave when P2 has no Wayland target"
    );
}

#[test]
fn incoming_position_over_x11_desktop_window_rejects_without_c2_target_entry() {
    let mut display = Display::<CompositorState>::new().expect("test display");
    let (client_a, mut peer_a) = test_client(&display);
    let (client_x11, mut peer_x11) = test_client(&display);
    let mut state = CompositorState::new(None);
    activate_xwayland_generation(&mut state, &client_a, generation(41));
    state.test_create_surface_resource(
        &client_a,
        &display.handle(),
        80,
        80,
        SurfacePlacement::absolute_root_at(0, 0),
    );
    let surface_x11 = state.test_create_surface_resource(
        &client_x11,
        &display.handle(),
        80,
        80,
        SurfacePlacement::absolute_root_at(100, 0),
    );
    let device_a = state.test_create_data_device(&client_a, &display.handle());
    let _x11_device = state.test_create_data_device(&client_x11, &display.handle());
    let mut x11_window = DesktopWindow::new_xdg(
        state.allocate_window_id().expect("X11 desktop window id"),
        compositor_surface_id(&surface_x11),
    );
    x11_window.backend =
        WindowBackend::X11(crate::xwayland::X11WindowHandle::new(generation(41), 0x236));
    state
        .insert_desktop_window(x11_window)
        .expect("insert X11-backed desktop window");

    let offer = xwayland_offer(36, 0x136, vec![XwaylandDndAction::Copy]);
    let offer_id = offer.id();
    assert!(state.begin_xwayland_drag_session(offer));
    let p1 = XwaylandDndIncomingPositionId::new(offer_id, NonZeroU64::new(1).unwrap());
    assert_eq!(
        state.apply_incoming_xwayland_position(
            offer_id,
            p1,
            vec![XwaylandDndAction::Copy],
            10.0,
            10.0,
        ),
        Some(XwaylandIncomingPositionDisposition::EnteredNewWaylandTarget)
    );
    let offer_a = active_wayland_offer(&state).expect("P1 enters target A");
    state.update_drag_acceptance(&offer_a, Some("text/plain".to_owned()));
    state.update_drag_actions(
        &offer_a,
        WaylandDndAction::Copy.mask(),
        WaylandDndAction::Copy.mask(),
    );
    let _p1_feedback = state.take_xwayland_dnd_transitions();
    display.flush_clients().unwrap();
    let _p1_events = take_wayland_events(&mut peer_a);
    let _ = take_wayland_events(&mut peer_x11);

    let p2 = XwaylandDndIncomingPositionId::new(offer_id, NonZeroU64::new(2).unwrap());
    assert_eq!(
        state.apply_incoming_xwayland_position(
            offer_id,
            p2,
            vec![XwaylandDndAction::Move, XwaylandDndAction::Copy],
            120.0,
            10.0,
        ),
        Some(XwaylandIncomingPositionDisposition::NoWaylandTarget)
    );
    assert!(
        state
            .active_drag
            .as_ref()
            .is_some_and(|active| active.target.is_none())
    );
    let p2_transitions = state.take_xwayland_dnd_transitions();
    assert!(matches!(
        p2_transitions.as_slice(),
        [XwaylandDndTransition::SourceFeedback {
            position_id,
            accepted_mime: None,
            action: None,
            ..
        }] if *position_id == p2
    ));
    assert!(p2_transitions.iter().all(|transition| !matches!(
        transition,
        XwaylandDndTransition::TargetEntered { .. }
            | XwaylandDndTransition::TargetPositioned { .. }
    )));
    display.flush_clients().unwrap();
    assert_eq!(
        take_wayland_events(&mut peer_a)
            .iter()
            .filter_map(
                |(object_id, opcode, _)| (*object_id == device_a.id().protocol_id())
                    .then_some(*opcode)
            )
            .collect::<Vec<_>>(),
        [2],
        "target A receives leave before the X11-backed desktop window is rejected"
    );
    assert!(take_wayland_events(&mut peer_x11).is_empty());
}

#[test]
fn incoming_target_position_selection_and_action_list_identities_remain_separate() {
    use crate::xwayland::XwaylandDndAction as Action;
    use crate::xwayland::xwm::data_bridge::dnd_incoming as incoming_dnd;

    let (mut xwm, mut x_peer, offer_id, mime_atom, timestamp, _, mut server_sequence) =
        incoming_dnd::tests::fake_incoming_hover();

    let initial_position = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .latest_position
        .unwrap();
    let offer_metadata = xwm.data_bridge.dnd.incoming_session().unwrap();
    let offer = XwaylandDndOffer::new(
        offer_id,
        offer_metadata.source,
        offer_metadata.version,
        XwaylandDndMimeCatalog::try_new(offer_metadata.mime_types.clone()).unwrap(),
        offer_metadata.source_actions.clone(),
    )
    .unwrap();

    let mut display = Display::<CompositorState>::new().expect("test display");
    let (client_a, mut peer_a) = test_client(&display);
    let (client_b, mut peer_b) = test_client(&display);
    let mut state = CompositorState::new(None);
    state.xwayland.client_identity = Some(crate::compositor::XwaylandClientIdentity {
        client_id: client_a.id(),
        generation: offer_id.generation(),
    });
    state.test_create_surface_resource(
        &client_a,
        &display.handle(),
        80,
        80,
        SurfacePlacement::absolute_root_at(0, -40),
    );
    state.test_create_surface_resource(
        &client_b,
        &display.handle(),
        80,
        80,
        SurfacePlacement::absolute_root_at(100, -40),
    );
    let device_a = state.test_create_data_device(&client_a, &display.handle());
    let device_b = state.test_create_data_device(&client_b, &display.handle());

    assert!(state.begin_xwayland_drag_session(offer));
    assert_eq!(
        state.apply_incoming_xwayland_position(
            offer_id,
            initial_position.position_id,
            offer_metadata.source_actions.clone(),
            initial_position.root_x,
            initial_position.root_y,
        ),
        Some(XwaylandIncomingPositionDisposition::EnteredNewWaylandTarget)
    );
    let offer_a = active_wayland_offer(&state).expect("P1 target A offer");
    state.update_drag_acceptance(&offer_a, Some("text/plain".to_owned()));
    state.update_drag_actions(
        &offer_a,
        WaylandDndAction::Copy.mask(),
        WaylandDndAction::Copy.mask(),
    );
    let p1_feedback_transitions = state.take_xwayland_dnd_transitions();
    let [
        XwaylandDndTransition::SourceFeedback {
            position_id: p1,
            accepted_mime: Some(_),
            action: Some(Action::Copy),
            ..
        },
    ] = p1_feedback_transitions.as_slice()
    else {
        panic!("A accepts the exact initial Position P1");
    };
    incoming_dnd::source_feedback(
        &mut xwm,
        offer_id,
        *p1,
        Some("text/plain".to_owned()),
        Some(Action::Copy),
    )
    .unwrap();
    xwm.connection.flush().unwrap();
    let p1_status =
        incoming_dnd::tests::status_messages(&incoming_dnd::tests::read_peer(&mut x_peer), &xwm);
    assert_eq!(p1_status.len(), 1);
    assert_eq!(p1_status[0].1[1] & 1, 1);
    display.flush_clients().unwrap();
    let _p1_events = take_wayland_events(&mut peer_a);

    let (mut selection_reader, transfer_id, requestor, property) =
        incoming_dnd::tests::begin_fake_selection_transfer(
            &mut xwm,
            &mut x_peer,
            offer_id,
            mime_atom,
            timestamp,
            &mut server_sequence,
        );

    let enter_deadline = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .metadata_deadline_ns;
    let p2_now = enter_deadline.saturating_add(100);
    incoming_dnd::tests::position_at_for_test(
        &mut xwm,
        timestamp + 1,
        Action::Move,
        112,
        -7,
        p2_now,
    )
    .unwrap();
    let p2_position = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .latest_position
        .unwrap();
    assert_eq!(p2_position.requested_action, Action::Move);
    assert_eq!(p2_position.timestamp, timestamp + 1);
    let p2_events = xwm.data_bridge.dnd_incoming.take_events();
    let [
        XwaylandDndIncomingEvent::Position {
            position_id: p2,
            x,
            y,
            source_actions,
            ..
        },
    ] = p2_events.as_slice()
    else {
        panic!("Move P2 is applied as one current Position event");
    };
    assert_eq!(source_actions, &[Action::Move, Action::Copy]);
    assert_eq!(
        state.apply_incoming_xwayland_position(offer_id, *p2, source_actions.clone(), *x, *y,),
        Some(XwaylandIncomingPositionDisposition::EnteredNewWaylandTarget)
    );
    assert!(state.take_xwayland_dnd_transitions().is_empty());
    assert!(
        xwm.data_bridge
            .dnd
            .incoming_session()
            .unwrap()
            .status_pending
    );
    xwm.connection.flush().unwrap();
    assert!(
        incoming_dnd::tests::status_messages(&incoming_dnd::tests::read_peer(&mut x_peer), &xwm,)
            .is_empty()
    );
    display.flush_clients().unwrap();
    assert_eq!(
        take_wayland_events(&mut peer_a)
            .iter()
            .filter_map(
                |(object_id, opcode, _)| (*object_id == device_a.id().protocol_id())
                    .then_some(*opcode)
            )
            .collect::<Vec<_>>(),
        [2]
    );
    assert_eq!(
        take_wayland_events(&mut peer_b)
            .iter()
            .filter_map(
                |(object_id, opcode, _)| (*object_id == device_b.id().protocol_id())
                    .then_some(*opcode)
            )
            .collect::<Vec<_>>(),
        [0, 1]
    );

    incoming_dnd::tests::notify_fake_selection_transfer(
        &mut xwm,
        &mut x_peer,
        transfer_id,
        mime_atom,
        timestamp,
        requestor,
        property,
        server_sequence,
        &mut server_sequence,
    );
    assert_eq!(
        incoming_dnd::tests::transfer_selection_timestamp(&xwm, transfer_id),
        Some(timestamp),
        "P2 does not replace T1 selection authority"
    );
    let property_sequence = incoming_dnd::tests::transfer_pending_sequence(&xwm, transfer_id)
        .expect("T1 transfer has its property reply pending");
    x_peer
        .write_all(&incoming_dnd::tests::get_property_reply(
            property_sequence,
            mime_atom,
            8,
            b"P1 selection T1",
        ))
        .unwrap();
    xwm.drain_events(32).unwrap();
    let mut payload = [0; 15];
    selection_reader.read_exact(&mut payload).unwrap();
    assert_eq!(&payload, b"P1 selection T1");
    assert!(!incoming_dnd::tests::transfer_is_active(&xwm, transfer_id));

    incoming_dnd::source_feedback(
        &mut xwm,
        offer_id,
        initial_position.position_id,
        Some("text/plain".to_owned()),
        Some(Action::Copy),
    )
    .unwrap();
    xwm.connection.flush().unwrap();
    assert!(
        incoming_dnd::tests::status_messages(&incoming_dnd::tests::read_peer(&mut x_peer), &xwm,)
            .is_empty()
    );

    let offer_b = active_wayland_offer(&state).expect("P2 target B offer");
    state.update_drag_acceptance(&offer_b, Some("text/plain".to_owned()));
    state.update_drag_actions(
        &offer_b,
        WaylandDndAction::Copy.mask(),
        WaylandDndAction::Copy.mask(),
    );
    assert!(matches!(
        state.take_xwayland_dnd_transitions().as_slice(),
        [XwaylandDndTransition::SourceFeedback {
            position_id,
            accepted_mime: Some(mime),
            action: Some(Action::Copy),
            ..
        }] if *position_id == *p2 && mime == "text/plain"
    ));
    incoming_dnd::source_feedback(
        &mut xwm,
        offer_id,
        *p2,
        Some("text/plain".to_owned()),
        Some(Action::Copy),
    )
    .unwrap();
    xwm.connection.flush().unwrap();
    let p2_status =
        incoming_dnd::tests::status_messages(&incoming_dnd::tests::read_peer(&mut x_peer), &xwm);
    assert_eq!(p2_status.len(), 1);
    assert_eq!(p2_status[0].1[1] & 1, 1, "B accepts P2 as Copy");
    assert_eq!(
        p2_status[0].1[4],
        incoming_dnd::tests::action_atom_for_test(&xwm, Action::Copy)
    );

    let p3_now =
        enter_deadline.saturating_add(incoming_dnd::TARGET_METADATA_TIMEOUT_NS.saturating_mul(4));
    incoming_dnd::tests::position_at_for_test(
        &mut xwm,
        timestamp + 2,
        Action::Ask,
        120,
        10,
        p3_now,
    )
    .unwrap();
    let p3_position = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .latest_position
        .unwrap();
    let (action_list_sequence, action_list_deadline) = xwm
        .data_bridge
        .dnd_incoming
        .pending
        .iter()
        .find_map(|(sequence, pending)| match pending {
            crate::xwayland::xwm::data_bridge::dnd_incoming::PendingMetadataReply::ActionList {
                offer_id: pending_offer,
                position_id,
                deadline_ns,
            } if *pending_offer == offer_id && *position_id == p3_position.position_id => {
                Some((*sequence, *deadline_ns))
            }
            _ => None,
        })
        .expect("P3 Ask starts a new ActionList request");
    assert_eq!(
        action_list_deadline,
        p3_now + incoming_dnd::TARGET_METADATA_TIMEOUT_NS
    );
    assert!(action_list_deadline > p3_now);
    xwm.connection.flush().unwrap();
    let _action_list_request =
        incoming_dnd::tests::take_requests(&mut x_peer, &mut server_sequence);
    let action_list_atoms = [Action::Ask, Action::Copy, Action::Move]
        .map(|action| incoming_dnd::tests::action_atom_for_test(&xwm, action));
    let action_list_bytes = action_list_atoms
        .into_iter()
        .flat_map(|atom| atom.to_ne_bytes())
        .collect::<Vec<_>>();
    x_peer
        .write_all(&incoming_dnd::tests::get_property_reply(
            action_list_sequence as u16,
            u32::from(AtomEnum::ATOM),
            32,
            &action_list_bytes,
        ))
        .unwrap();
    incoming_dnd::poll_replies(&mut xwm, 16, p3_now + 1).unwrap();
    let p3_events = xwm.data_bridge.dnd_incoming.take_events();
    let [
        XwaylandDndIncomingEvent::Position {
            position_id: p3,
            source_actions: p3_source_actions,
            x,
            y,
            ..
        },
    ] = p3_events.as_slice()
    else {
        panic!("P3 ActionList metadata resolves into its current Position");
    };
    assert_eq!(
        p3_source_actions,
        &[Action::Ask, Action::Copy, Action::Move]
    );
    assert_eq!(
        state.apply_incoming_xwayland_position(offer_id, *p3, p3_source_actions.clone(), *x, *y,),
        Some(XwaylandIncomingPositionDisposition::SameWaylandTarget)
    );
    assert_eq!(
        state
            .data_offers
            .get(&offer_b.id())
            .map(|binding| binding.source_actions),
        Some(
            WaylandDndAction::Ask.mask()
                | WaylandDndAction::Copy.mask()
                | WaylandDndAction::Move.mask()
        )
    );
    let p3_feedback = state.take_xwayland_dnd_transitions();
    assert!(matches!(
        p3_feedback.as_slice(),
        [XwaylandDndTransition::SourceFeedback {
            position_id,
            accepted_mime: Some(_),
            action: Some(Action::Copy),
            ..
        }] if *position_id == *p3
    ));
    incoming_dnd::source_feedback(
        &mut xwm,
        offer_id,
        *p3,
        Some("text/plain".to_owned()),
        Some(Action::Copy),
    )
    .unwrap();
    xwm.connection.flush().unwrap();
    let p3_status =
        incoming_dnd::tests::status_messages(&incoming_dnd::tests::read_peer(&mut x_peer), &xwm);
    assert_eq!(p3_status.len(), 1);
    assert_eq!(
        p3_status[0].1[1] & 1,
        1,
        "P3 Ask remains active after its metadata reply"
    );
    assert_eq!(
        p3_status[0].1[4],
        incoming_dnd::tests::action_atom_for_test(&xwm, Action::Copy)
    );
}
