use super::*;
use crate::xwayland::{
    CanonicalDndSessionId, MAX_PENDING_XWAYLAND_DND_TRANSITIONS, WaylandDndAction, X11WindowHandle,
    XwaylandDndAction, XwaylandDndMimeCatalog, XwaylandDndOffer, XwaylandDndOfferId,
    XwaylandDndOutbox, XwaylandDndTransition, XwaylandDndVersion, XwaylandGeneration,
};
use std::{
    io::Read,
    num::NonZeroU64,
    os::fd::{AsRawFd, OwnedFd},
    os::unix::net::UnixStream,
    sync::Arc,
};
use wayland_server::{Client, Display};

fn generation(value: u64) -> XwaylandGeneration {
    XwaylandGeneration::new(NonZeroU64::new(value).expect("nonzero generation"))
}

fn xwayland_offer(
    serial: u64,
    source_xid: u32,
    actions: Vec<XwaylandDndAction>,
) -> XwaylandDndOffer {
    let generation = generation(41);
    XwaylandDndOffer::new(
        XwaylandDndOfferId::new(
            generation,
            NonZeroU64::new(serial).expect("nonzero offer serial"),
        ),
        X11WindowHandle::new(generation, source_xid),
        XwaylandDndVersion::new(5).expect("valid XDND version"),
        XwaylandDndMimeCatalog::try_new(vec!["text/plain".to_owned(), "text/uri-list".to_owned()])
            .expect("bounded MIME catalog"),
        actions,
    )
    .expect("valid XWayland offer")
}

#[test]
fn xwayland_dnd_outbox_coalesces_only_exact_adjacent_positions() {
    let generation = generation(81);
    let session = CanonicalDndSessionId::Wayland(NonZeroU64::new(1).unwrap());
    let target_a = X11WindowHandle::new(generation, 0x810);
    let target_b = X11WindowHandle::new(generation, 0x811);
    let mut outbox = XwaylandDndOutbox::default();

    outbox
        .push(XwaylandDndTransition::TargetEntered {
            session_id: session,
            target: target_a,
            x: 0.0,
            y: 0.0,
            mime_types: XwaylandDndMimeCatalog::default(),
            source_actions: Vec::new(),
        })
        .unwrap();
    for index in 0..MAX_PENDING_XWAYLAND_DND_TRANSITIONS * 8 {
        let x = index as f64;
        outbox
            .push(XwaylandDndTransition::TargetPositioned {
                session_id: session,
                target: target_a,
                x,
                y: x * 2.0,
                action: None,
                mime_types: XwaylandDndMimeCatalog::default(),
                source_actions: Vec::new(),
            })
            .unwrap();
    }
    outbox
        .push(XwaylandDndTransition::TargetPositioned {
            session_id: session,
            target: target_b,
            x: 50.0,
            y: 60.0,
            action: None,
            mime_types: XwaylandDndMimeCatalog::default(),
            source_actions: Vec::new(),
        })
        .unwrap();
    let replacement_session = CanonicalDndSessionId::Wayland(NonZeroU64::new(2).unwrap());
    outbox
        .push(XwaylandDndTransition::TargetPositioned {
            session_id: replacement_session,
            target: target_b,
            x: 70.0,
            y: 80.0,
            action: None,
            mime_types: XwaylandDndMimeCatalog::default(),
            source_actions: Vec::new(),
        })
        .unwrap();

    let transitions = outbox.drain();
    assert_eq!(transitions.len(), 4);
    assert!(matches!(
        transitions[0],
        XwaylandDndTransition::TargetEntered { target, .. } if target == target_a
    ));
    assert!(matches!(
        transitions[1],
        XwaylandDndTransition::TargetPositioned {
            target,
            x,
            y,
            ..
        }
            if target == target_a
                && x == (MAX_PENDING_XWAYLAND_DND_TRANSITIONS * 8 - 1) as f64
                && y == (MAX_PENDING_XWAYLAND_DND_TRANSITIONS * 8 - 1) as f64 * 2.0
    ));
    assert!(matches!(
        transitions[2],
        XwaylandDndTransition::TargetPositioned { session_id: current, target, .. }
            if current == session && target == target_b
    ));
    assert!(matches!(
        transitions[3],
        XwaylandDndTransition::TargetPositioned { session_id: current, target, .. }
            if current == replacement_session && target == target_b
    ));
}

#[test]
fn xwayland_dnd_outbox_keeps_edges_ordered_and_preserves_source_finish() {
    let generation = generation(82);
    let session = CanonicalDndSessionId::Wayland(NonZeroU64::new(3).unwrap());
    let offer_id = XwaylandDndOfferId::new(generation, NonZeroU64::new(4).unwrap());
    let target_a = X11WindowHandle::new(generation, 0x820);
    let target_b = X11WindowHandle::new(generation, 0x821);
    let mime_types = XwaylandDndMimeCatalog::default();
    let mut outbox = XwaylandDndOutbox::default();
    let required = [
        XwaylandDndTransition::TargetEntered {
            session_id: session,
            target: target_a,
            x: 1.0,
            y: 2.0,
            mime_types: mime_types.clone(),
            source_actions: Vec::new(),
        },
        XwaylandDndTransition::TargetPositioned {
            session_id: session,
            target: target_a,
            x: 3.0,
            y: 4.0,
            action: None,
            mime_types: mime_types.clone(),
            source_actions: Vec::new(),
        },
        XwaylandDndTransition::TargetLeft {
            session_id: session,
            target: target_a,
        },
        XwaylandDndTransition::TargetEntered {
            session_id: session,
            target: target_b,
            x: 5.0,
            y: 6.0,
            mime_types: mime_types.clone(),
            source_actions: Vec::new(),
        },
        XwaylandDndTransition::DropRequested {
            session_id: session,
            target: target_b,
            action: XwaylandDndAction::Copy,
            mime_types,
            source_actions: vec![XwaylandDndAction::Copy],
        },
        XwaylandDndTransition::TargetFinished {
            session_id: session,
            target: target_b,
            accepted: true,
            action: Some(XwaylandDndAction::Copy),
        },
        XwaylandDndTransition::SourceFeedback {
            offer_id,
            accepted_mime: Some("text/plain".to_owned()),
            action: Some(XwaylandDndAction::Copy),
        },
        XwaylandDndTransition::SourceFinished {
            offer_id,
            accepted: true,
            action: Some(XwaylandDndAction::Copy),
        },
        XwaylandDndTransition::Retired {
            session_id: session,
            generation,
        },
    ];
    for transition in required {
        outbox.push(transition).unwrap();
    }

    let transitions = outbox.drain();
    assert_eq!(transitions.len(), 9);
    assert!(
        matches!(transitions[0], XwaylandDndTransition::TargetEntered { target, .. } if target == target_a)
    );
    assert!(
        matches!(transitions[1], XwaylandDndTransition::TargetPositioned { target, .. } if target == target_a)
    );
    assert!(
        matches!(transitions[2], XwaylandDndTransition::TargetLeft { target, .. } if target == target_a)
    );
    assert!(
        matches!(transitions[3], XwaylandDndTransition::TargetEntered { target, .. } if target == target_b)
    );
    assert!(
        matches!(transitions[4], XwaylandDndTransition::DropRequested { target, .. } if target == target_b)
    );
    assert!(
        matches!(transitions[5], XwaylandDndTransition::TargetFinished { target, .. } if target == target_b)
    );
    assert!(
        matches!(transitions[6], XwaylandDndTransition::SourceFeedback { offer_id: current, .. } if current == offer_id)
    );
    assert!(
        matches!(transitions[7], XwaylandDndTransition::SourceFinished { offer_id: current, accepted: true, .. } if current == offer_id)
    );
    assert!(
        matches!(transitions[8], XwaylandDndTransition::Retired { session_id: current, generation: current_generation } if current == session && current_generation == generation)
    );
}

#[test]
fn xwayland_dnd_outbox_coalesces_feedback_only_for_the_exact_offer() {
    let offer_generation = generation(83);
    let offer_id = XwaylandDndOfferId::new(offer_generation, NonZeroU64::new(1).unwrap());
    let replacement_offer_id = XwaylandDndOfferId::new(generation(84), NonZeroU64::new(1).unwrap());
    let mut outbox = XwaylandDndOutbox::default();
    for transition in [
        XwaylandDndTransition::SourceFeedback {
            offer_id,
            accepted_mime: Some("text/plain".to_owned()),
            action: Some(XwaylandDndAction::Copy),
        },
        XwaylandDndTransition::SourceFeedback {
            offer_id,
            accepted_mime: None,
            action: None,
        },
        XwaylandDndTransition::SourceFeedback {
            offer_id: replacement_offer_id,
            accepted_mime: Some("text/uri-list".to_owned()),
            action: Some(XwaylandDndAction::Move),
        },
        XwaylandDndTransition::SourceFinished {
            offer_id: replacement_offer_id,
            accepted: true,
            action: Some(XwaylandDndAction::Move),
        },
    ] {
        outbox.push(transition).unwrap();
    }

    let transitions = outbox.drain();
    assert_eq!(transitions.len(), 3);
    assert!(
        matches!(transitions[0], XwaylandDndTransition::SourceFeedback { offer_id: current, accepted_mime: None, .. } if current == offer_id)
    );
    assert!(
        matches!(transitions[1], XwaylandDndTransition::SourceFeedback { offer_id: current, .. } if current == replacement_offer_id)
    );
    assert!(
        matches!(transitions[2], XwaylandDndTransition::SourceFinished { offer_id: current, accepted: true, .. } if current == replacement_offer_id)
    );
}

#[test]
fn xwayland_dnd_outbox_rejects_required_overflow_and_resets_to_exact_retirement() {
    let generation = generation(85);
    let session = CanonicalDndSessionId::Wayland(NonZeroU64::new(5).unwrap());
    let target = X11WindowHandle::new(generation, 0x850);
    let mut outbox = XwaylandDndOutbox::default();
    for _ in 0..MAX_PENDING_XWAYLAND_DND_TRANSITIONS {
        outbox
            .push(XwaylandDndTransition::TargetLeft {
                session_id: session,
                target,
            })
            .unwrap();
    }
    let overflow = outbox.push(XwaylandDndTransition::DropRequested {
        session_id: session,
        target,
        action: XwaylandDndAction::Copy,
        mime_types: XwaylandDndMimeCatalog::default(),
        source_actions: vec![XwaylandDndAction::Copy],
    });
    assert!(
        matches!(overflow, Err(XwaylandDndTransition::DropRequested { session_id, target: current_target, .. }) if session_id == session && current_target == target)
    );
    assert_eq!(outbox.drain().len(), MAX_PENDING_XWAYLAND_DND_TRANSITIONS);

    outbox.replace_with_retired(session, generation);
    assert!(matches!(
        outbox.drain().as_slice(),
        [XwaylandDndTransition::Retired { session_id: current, generation: current_generation }]
            if *current == session && *current_generation == generation
    ));
}

#[test]
fn xwayland_dnd_outbox_clears_only_the_retired_generation() {
    let old_generation = generation(86);
    let replacement_generation = generation(87);
    let old_session = CanonicalDndSessionId::Wayland(NonZeroU64::new(6).unwrap());
    let replacement_session = CanonicalDndSessionId::Wayland(NonZeroU64::new(7).unwrap());
    let mut outbox = XwaylandDndOutbox::default();
    for (session_id, target) in [
        (old_session, X11WindowHandle::new(old_generation, 0x860)),
        (
            replacement_session,
            X11WindowHandle::new(replacement_generation, 0x870),
        ),
    ] {
        outbox
            .push(XwaylandDndTransition::TargetEntered {
                session_id,
                target,
                x: 0.0,
                y: 0.0,
                mime_types: XwaylandDndMimeCatalog::default(),
                source_actions: Vec::new(),
            })
            .unwrap();
    }

    outbox.clear_generation(old_generation);
    assert!(matches!(
        outbox.drain().as_slice(),
        [XwaylandDndTransition::TargetEntered { session_id, target, .. }]
            if *session_id == replacement_session
                && *target == X11WindowHandle::new(replacement_generation, 0x870)
    ));
}

fn test_client(display: &Display<CompositorState>) -> (Client, UnixStream) {
    let (server, peer) = UnixStream::pair().expect("test client socket");
    let client = display
        .handle()
        .insert_client(server, Arc::new(()))
        .expect("test client insertion");
    (client, peer)
}

fn activate_xwayland_generation(
    state: &mut CompositorState,
    client: &Client,
    generation: XwaylandGeneration,
) {
    state.xwayland.client_identity = Some(crate::compositor::XwaylandClientIdentity {
        client_id: client.id(),
        generation,
    });
}

fn active_wayland_offer(state: &CompositorState) -> Option<wl_data_offer::WlDataOffer> {
    state
        .active_drag
        .as_ref()?
        .target
        .as_ref()?
        .wayland_offer()
        .cloned()
}

#[derive(Debug, PartialEq, Eq)]
enum SourceWireEvent {
    Target(Option<String>),
    Cancelled,
    DropPerformed,
    Finished,
    Action(u32),
    Other(u16),
}

fn source_wire_events(
    display: &mut Display<CompositorState>,
    peer: &mut UnixStream,
    source: &wl_data_source::WlDataSource,
) -> Vec<SourceWireEvent> {
    display.flush_clients().expect("flush test client events");
    peer.set_nonblocking(true).expect("nonblocking test peer");

    let mut wire = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        match peer.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => wire.extend_from_slice(&chunk[..read]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) => panic!("read test client events: {error}"),
        }
    }

    let source_id = source.id().protocol_id();
    let mut events = Vec::new();
    let mut offset = 0;
    while offset + 8 <= wire.len() {
        let object_id = u32::from_ne_bytes(wire[offset..offset + 4].try_into().unwrap());
        let header = u32::from_ne_bytes(wire[offset + 4..offset + 8].try_into().unwrap());
        let size = (header >> 16) as usize;
        let opcode = (header & 0xffff) as u16;
        assert!(size >= 8, "invalid Wayland event frame size {size}");
        assert!(
            offset + size <= wire.len(),
            "incomplete Wayland event frame"
        );
        if object_id == source_id {
            let payload = &wire[offset + 8..offset + size];
            let event = match opcode {
                0 => {
                    assert!(payload.len() >= 4, "target event must contain a string");
                    let length = u32::from_ne_bytes(payload[..4].try_into().unwrap()) as usize;
                    if length == 0 {
                        SourceWireEvent::Target(None)
                    } else {
                        assert!(payload.len() >= 4 + length, "incomplete target string");
                        assert_eq!(payload[4 + length - 1], 0, "target string terminator");
                        SourceWireEvent::Target(Some(
                            String::from_utf8(payload[4..4 + length - 1].to_vec())
                                .expect("target MIME is UTF-8"),
                        ))
                    }
                }
                2 => SourceWireEvent::Cancelled,
                3 => SourceWireEvent::DropPerformed,
                4 => SourceWireEvent::Finished,
                5 => {
                    assert!(payload.len() >= 4, "action event must contain an action");
                    SourceWireEvent::Action(u32::from_ne_bytes(payload[..4].try_into().unwrap()))
                }
                _ => SourceWireEvent::Other(opcode),
            };
            events.push(event);
        }
        offset += size;
    }
    assert_eq!(offset, wire.len(), "complete Wayland event frames");
    events
}

struct WaylandSourceX11TargetDrag {
    display: Display<CompositorState>,
    _client: Client,
    peer: UnixStream,
    state: CompositorState,
    source: wl_data_source::WlDataSource,
    session_id: CanonicalDndSessionId,
    generation: XwaylandGeneration,
    target: X11WindowHandle,
    x11_target_surface: wl_surface::WlSurface,
    wayland_target_surface: wl_surface::WlSurface,
}

impl WaylandSourceX11TargetDrag {
    fn source_events(&mut self) -> Vec<SourceWireEvent> {
        source_wire_events(&mut self.display, &mut self.peer, &self.source)
    }

    fn remove_target_window(&mut self) {
        let window_id = self
            .state
            .window_id_for_x11_handle(self.target)
            .expect("X11 target window");
        assert!(self.state.remove_desktop_window(window_id).is_some());
    }
}

fn install_x11_drag_target(
    state: &mut CompositorState,
    surface: &wl_surface::WlSurface,
    generation: XwaylandGeneration,
    xid: u32,
) -> X11WindowHandle {
    install_x11_drag_target_at(state, surface, generation, xid, 100)
}

fn install_x11_drag_target_at(
    state: &mut CompositorState,
    surface: &wl_surface::WlSurface,
    generation: XwaylandGeneration,
    xid: u32,
    x: i32,
) -> X11WindowHandle {
    let surface_id = compositor_surface_id(surface);
    let target = X11WindowHandle::new(generation, xid);
    let mut snapshot = super::desktop_window_tests::x11_snapshot(generation, xid, surface_id);
    snapshot.geometry = crate::xwayland::xwm::X11Geometry {
        x,
        y: 0,
        width: 800,
        height: 600,
    };
    snapshot.decoration_hints.motif = crate::xwayland::xwm::X11MotifDecorationHint::Undecorated;
    super::desktop_window_tests::insert_x11(state, snapshot);
    state.test_set_surface_placement(surface_id, SurfacePlacement::absolute_root_at(x, 0));
    target
}

fn wayland_source_x11_target_drag(
    action: XwaylandDndAction,
    source_actions: u32,
) -> WaylandSourceX11TargetDrag {
    let display = Display::<CompositorState>::new().expect("test display");
    let (client, peer) = test_client(&display);
    let mut state = CompositorState::new(None);
    let generation = generation(61);
    activate_xwayland_generation(&mut state, &client, generation);
    let origin = state.test_create_surface_resource(
        &client,
        &display.handle(),
        40,
        40,
        SurfacePlacement::absolute_root_at(1000, 1000),
    );
    let x11_target_surface = state.test_create_surface_resource(
        &client,
        &display.handle(),
        800,
        600,
        SurfacePlacement::absolute_root_at(100, 0),
    );
    let target = install_x11_drag_target(&mut state, &x11_target_surface, generation, 0x601);
    let wayland_target_surface = state.test_create_surface_resource(
        &client,
        &display.handle(),
        80,
        80,
        SurfacePlacement::absolute_root_at(0, 0),
    );
    state.test_create_data_device(&client, &display.handle());

    let source = state.test_create_data_source(&client, &display.handle());
    state.offer_data_source_mime_type(&source, "text/plain".to_owned());
    if let Some(binding) = state.data_sources.get_mut(&source.id()) {
        binding.actions = source_actions;
        binding.actions_set = true;
    }
    state.begin_drag_session(Some(source.clone()), origin, None, 8);
    let session_id = state.active_drag.as_ref().expect("canonical drag").id;
    state.update_drag_target_at(110.0, 100.0);
    assert!(state.update_xwayland_drag_target_status(session_id, target, true, Some(action),));

    WaylandSourceX11TargetDrag {
        display,
        _client: client,
        peer,
        state,
        source,
        session_id,
        generation,
        target,
        x11_target_surface,
        wayland_target_surface,
    }
}

#[test]
fn x11_status_acceptance_does_not_require_or_invent_a_mime() {
    let mut drag =
        wayland_source_x11_target_drag(XwaylandDndAction::Copy, WaylandDndAction::Copy.mask());

    assert!(drag.state.update_xwayland_drag_target_status(
        drag.session_id,
        drag.target,
        true,
        Some(XwaylandDndAction::Copy),
    ));
    let active = drag.state.active_drag.as_ref().expect("active drag");
    assert_eq!(active.target_action, Some(XwaylandDndAction::Copy));
    assert_eq!(active.accepted_mime, None);
}

#[test]
fn x11_target_switch_preserves_ordered_leave_and_enter_before_drain() {
    let mut drag = wayland_source_x11_target_drag(
        XwaylandDndAction::Copy,
        WaylandDndAction::Copy.mask() | WaylandDndAction::Move.mask(),
    );
    let target_a = drag.target;
    let target_b_surface = drag.state.test_create_surface_resource(
        &drag._client,
        &drag.display.handle(),
        200,
        600,
        SurfacePlacement::absolute_root_at(1000, 0),
    );
    let target_b = install_x11_drag_target_at(
        &mut drag.state,
        &target_b_surface,
        drag.generation,
        0x602,
        1000,
    );

    drag.state.update_drag_target_at(1010.0, 100.0);

    let transitions = drag.state.take_xwayland_dnd_transitions();
    assert_eq!(transitions.len(), 3);
    assert!(matches!(
        transitions.as_slice(),
        [
            XwaylandDndTransition::TargetEntered { target: first, .. },
            XwaylandDndTransition::TargetLeft { target: left, .. },
            XwaylandDndTransition::TargetEntered { target: entered, .. },
        ] if *first == target_a && *left == target_a && *entered == target_b
    ));
    assert!(matches!(
        drag.state.active_drag.as_ref().and_then(|active| active.target.as_ref()),
        Some(ActiveDragTarget::Xwayland { window }) if *window == target_b
    ));
}

#[test]
fn target_enter_is_not_overwritten_by_motion_before_drain() {
    let mut drag = wayland_source_x11_target_drag(
        XwaylandDndAction::Copy,
        WaylandDndAction::Copy.mask() | WaylandDndAction::Move.mask(),
    );
    let target = drag.target;
    drag.state.update_drag_target_at(120.0, 130.0);
    drag.state.update_drag_target_at(140.0, 150.0);

    let transitions = drag.state.take_xwayland_dnd_transitions();
    assert_eq!(transitions.len(), 2);
    assert!(matches!(
        transitions.as_slice(),
        [
            XwaylandDndTransition::TargetEntered { target: entered, .. },
            XwaylandDndTransition::TargetPositioned {
                target: positioned,
                x: 140.0,
                y: 150.0,
                ..
            },
        ] if *entered == target && *positioned == target
    ));
}

#[test]
fn continuous_positions_do_not_cancel_but_mandatory_overflow_retires_exact_drag() {
    let mut drag = wayland_source_x11_target_drag(
        XwaylandDndAction::Copy,
        WaylandDndAction::Copy.mask() | WaylandDndAction::Move.mask(),
    );
    let target_a = drag.target;
    let target_b_surface = drag.state.test_create_surface_resource(
        &drag._client,
        &drag.display.handle(),
        200,
        600,
        SurfacePlacement::absolute_root_at(1000, 0),
    );
    let _target_b = install_x11_drag_target_at(
        &mut drag.state,
        &target_b_surface,
        drag.generation,
        0x603,
        1000,
    );

    for index in 0..MAX_PENDING_XWAYLAND_DND_TRANSITIONS * 8 {
        let coordinate = 110.0 + index as f64;
        drag.state
            .update_drag_target_at(coordinate.min(850.0), 120.0);
    }
    assert_eq!(drag.state.xwayland_dnd_outbox.len(), 2);
    assert_eq!(
        drag.state.active_drag.as_ref().map(|active| active.id),
        Some(drag.session_id)
    );

    for index in 0..30 {
        if index % 2 == 0 {
            drag.state.update_drag_target_at(1010.0, 100.0);
        } else {
            drag.state.update_drag_target_at(110.0, 100.0);
        }
    }
    for index in 0..MAX_PENDING_XWAYLAND_DND_TRANSITIONS * 8 {
        let coordinate = 110.0 + index as f64;
        drag.state
            .update_drag_target_at(coordinate.min(850.0), 120.0);
    }
    assert_eq!(drag.state.xwayland_dnd_outbox.len(), 63);
    assert!(matches!(
        drag.state
            .active_drag
            .as_ref()
            .and_then(|active| active.target.as_ref()),
        Some(ActiveDragTarget::Xwayland { window }) if *window == target_a
    ));
    assert!(drag.state.update_xwayland_drag_target_status(
        drag.session_id,
        target_a,
        true,
        Some(XwaylandDndAction::Copy),
    ));
    let _ = drag.source_events();

    drag.state.drop_active_drag();
    assert_eq!(
        drag.state.xwayland_dnd_outbox.len(),
        MAX_PENDING_XWAYLAND_DND_TRANSITIONS
    );
    assert_eq!(drag.source_events(), [SourceWireEvent::DropPerformed]);
    assert_eq!(
        drag.state.active_drag.as_ref().map(|active| active.phase),
        Some(DragSessionPhase::DropPendingXwaylandTarget)
    );
    assert!(!drag.state.finish_xwayland_drag_target(
        drag.session_id,
        target_a,
        true,
        Some(XwaylandDndAction::Copy),
    ));

    assert!(drag.state.active_drag.is_none());
    assert_eq!(drag.state.compliance_metrics.dnd_sessions_cancelled, 1);
    assert_eq!(drag.state.compliance_metrics.dnd_source_cancelled_events, 1);
    assert_eq!(drag.state.xwayland_dnd_outbox.len(), 1);
    assert!(matches!(
        drag.state.take_xwayland_dnd_transitions().as_slice(),
        [XwaylandDndTransition::Retired { session_id, generation }]
            if *session_id == drag.session_id && *generation == drag.generation
    ));
    assert_eq!(
        drag.source_events(),
        [SourceWireEvent::Target(None), SourceWireEvent::Cancelled]
    );
}

#[test]
fn xwayland_origin_creates_wayland_offer_and_queues_exact_move_only_payload_request() {
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

    let first = xwayland_offer(
        1,
        0x100,
        vec![
            XwaylandDndAction::Copy,
            XwaylandDndAction::Move,
            XwaylandDndAction::Ask,
            XwaylandDndAction::Link,
            XwaylandDndAction::Private,
        ],
    );
    let first_id = first.id();
    assert!(state.begin_xwayland_drag_session(first));
    assert!(matches!(
        state.active_drag.as_ref().map(|drag| &drag.origin),
        Some(ActiveDragOrigin::Xwayland { .. })
    ));
    assert!(matches!(
        state.active_drag.as_ref().map(|drag| drag.id),
        Some(CanonicalDndSessionId::Xwayland(id)) if id == first_id
    ));

    state.update_drag_target_at(10.0, 10.0);
    assert!(matches!(
        state.active_drag.as_ref().and_then(|drag| drag.target.as_ref()),
        Some(ActiveDragTarget::Wayland { surface, offer: Some(_), .. })
            if same_surface_resource(surface, &target)
    ));
    let offer = active_wayland_offer(&state).expect("synthetic offer to Wayland target");
    let binding = state.data_offers.get(&offer.id()).expect("offer binding");
    assert_eq!(binding.mime_types, ["text/plain", "text/uri-list"]);
    assert_eq!(
        binding.source_actions,
        WaylandDndAction::Copy.mask()
            | WaylandDndAction::Move.mask()
            | WaylandDndAction::Ask.mask()
    );

    let (_reader, sink_stream) = UnixStream::pair().expect("payload pipe");
    let sink: OwnedFd = sink_stream.into();
    let expected_sink_fd = sink.as_raw_fd();
    state.receive_clipboard_offer(&offer, &client.id(), 0, "text/plain".to_owned(), sink);
    assert_eq!(state.xwayland_dnd_data_requests.len(), 1);
    let request = state
        .xwayland_dnd_data_requests
        .pop_front()
        .expect("one queued payload request");
    assert_eq!(request.offer_id, first_id);
    assert_eq!(request.mime_type, "text/plain");
    assert_eq!(request.sink.as_raw_fd(), expected_sink_fd);

    activate_xwayland_generation(&mut state, &client, generation(42));
    state.receive_clipboard_offer(
        &offer,
        &client.id(),
        0,
        "text/plain".to_owned(),
        UnixStream::pair()
            .expect("retired generation payload pipe")
            .1
            .into(),
    );
    assert!(state.xwayland_dnd_data_requests.is_empty());
    activate_xwayland_generation(&mut state, &client, generation(41));

    // Replacing the canonical source retires its old offer and pending FD.
    let (_reader, stale_sink_stream) = UnixStream::pair().expect("stale payload pipe");
    let stale_sink: OwnedFd = stale_sink_stream.into();
    state.receive_clipboard_offer(&offer, &client.id(), 0, "text/plain".to_owned(), stale_sink);
    assert_eq!(state.xwayland_dnd_data_requests.len(), 1);
    let replacement = xwayland_offer(2, 0x101, vec![XwaylandDndAction::Copy]);
    let replacement_id = replacement.id();
    assert!(state.begin_xwayland_drag_session(replacement));
    assert!(state.xwayland_dnd_data_requests.is_empty());
    assert!(!state.begin_xwayland_drag_session(xwayland_offer(
        1,
        0x100,
        vec![XwaylandDndAction::Copy],
    )));
    state.update_drag_target_at(10.0, 10.0);
    state.receive_clipboard_offer(
        &offer,
        &client.id(),
        0,
        "text/plain".to_owned(),
        UnixStream::pair().expect("late payload pipe").1.into(),
    );
    assert!(state.xwayland_dnd_data_requests.is_empty());
    assert!(matches!(
        state.active_drag.as_ref().map(|drag| drag.id),
        Some(CanonicalDndSessionId::Xwayland(id)) if id == replacement_id
    ));

    state.clear_xwayland_generation(generation(41));
    assert!(state.active_drag.is_none());
    assert!(state.xwayland_dnd_data_requests.is_empty());
    assert!(state.take_xwayland_dnd_transitions().is_empty());
    assert!(state.xwayland.client_identity.is_none());
    assert!(state.last_xwayland_dnd_offer_id.is_none());
    assert!(!state.begin_xwayland_drag_session(xwayland_offer(
        1,
        0x100,
        vec![XwaylandDndAction::Copy],
    )));
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
fn xwayland_drop_terminal_is_consumed_at_most_once() {
    let display = Display::<CompositorState>::new().expect("test display");
    let (client, _peer) = test_client(&display);
    let mut state = CompositorState::new(None);
    activate_xwayland_generation(&mut state, &client, generation(41));
    let offer = xwayland_offer(4, 0x104, vec![XwaylandDndAction::Copy]);
    let offer_id = offer.id();
    assert!(state.begin_xwayland_drag_session(offer));

    // With no target, the first exact XDND drop input terminally cancels the
    // canonical session; a duplicate cannot claim to have been consumed.
    assert!(state.drop_xwayland_drag(offer_id));
    assert!(!state.drop_xwayland_drag(offer_id));
    assert!(state.active_drag.is_none());
    assert_eq!(state.compliance_metrics.dnd_sessions_cancelled, 1);
}

#[test]
fn implicit_grab_terminal_does_not_drive_xwayland_drag() {
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
    let offer = xwayland_offer(3, 0x103, vec![XwaylandDndAction::Copy]);
    let offer_id = offer.id();
    assert!(state.begin_xwayland_drag_session(offer));
    state.update_drag_target_at(10.0, 10.0);
    activate_xwayland_generation(&mut state, &client, generation(42));
    assert!(!state.drop_xwayland_drag(offer_id));
    activate_xwayland_generation(&mut state, &client, generation(41));
    state.implicit_pointer_grab = Some(ImplicitPointerGrab {
        surface: target.clone(),
        root_surface_id: compositor_surface_id(&target),
    });

    state.end_implicit_pointer_grab("last-release");

    let active = state
        .active_drag
        .as_ref()
        .expect("XWayland drag remains active");
    assert_eq!(active.id, CanonicalDndSessionId::Xwayland(offer_id));
    assert_eq!(active.lifecycle_driver, DragLifecycleDriver::Xwayland);
    assert_eq!(active.phase, DragSessionPhase::Dragging);
    assert_eq!(active.xwayland_dnd_generation, Some(generation(41)));
}

#[test]
fn sourceless_wayland_drag_stays_with_its_initiating_client() {
    let display = Display::<CompositorState>::new().expect("test display");
    let (initiating_client, _initiating_peer) = test_client(&display);
    let (other_client, _other_peer) = test_client(&display);
    let mut state = CompositorState::new(None);
    let origin = state.test_create_surface_resource(
        &initiating_client,
        &display.handle(),
        40,
        40,
        SurfacePlacement::absolute_root_at(1000, 1000),
    );
    let other_target = state.test_create_surface_resource(
        &other_client,
        &display.handle(),
        80,
        80,
        SurfacePlacement::absolute_root_at(0, 0),
    );
    state.test_create_data_device(&other_client, &display.handle());
    let same_client_target = state.test_create_surface_resource(
        &initiating_client,
        &display.handle(),
        80,
        80,
        SurfacePlacement::absolute_root_at(200, 0),
    );
    state.test_create_data_device(&initiating_client, &display.handle());

    state.begin_drag_session(None, origin, None, 1);
    assert!(matches!(
        state.active_drag.as_ref().map(|drag| &drag.origin),
        Some(ActiveDragOrigin::WaylandSourceless {
            initiating_client: owner,
            ..
        }) if owner == &initiating_client.id()
    ));

    state.update_drag_target_at(10.0, 10.0);
    assert!(
        state
            .active_drag
            .as_ref()
            .is_some_and(|drag| drag.target.is_none())
    );

    state.update_drag_target_at(210.0, 10.0);
    assert!(matches!(
        state.active_drag.as_ref().and_then(|drag| drag.target.as_ref()),
        Some(ActiveDragTarget::Wayland { surface, offer: None, .. })
            if same_surface_resource(surface, &same_client_target)
    ));
    assert!(!same_surface_resource(&other_target, &same_client_target));
    assert!(
        state
            .data_offers
            .values()
            .all(|offer| offer.kind != DataOfferKind::DragAndDrop)
    );
}

#[test]
fn wayland_target_switches_to_exact_x11_window_and_back_without_parallel_authority() {
    let display = Display::<CompositorState>::new().expect("test display");
    let (client, _peer) = test_client(&display);
    let mut state = CompositorState::new(None);
    let origin = state.test_create_surface_resource(
        &client,
        &display.handle(),
        40,
        40,
        SurfacePlacement::absolute_root_at(1000, 1000),
    );
    let wayland_target = state.test_create_surface_resource(
        &client,
        &display.handle(),
        80,
        80,
        SurfacePlacement::absolute_root_at(0, 0),
    );
    let _device = state.test_create_data_device(&client, &display.handle());
    let x11_target = state.test_create_surface_resource(
        &client,
        &display.handle(),
        800,
        600,
        SurfacePlacement::absolute_root_at(100, 0),
    );
    let x11_surface_id = compositor_surface_id(&x11_target);
    let generation = generation(41);
    activate_xwayland_generation(&mut state, &client, generation);
    let x11_handle = X11WindowHandle::new(generation, 0x201);
    let mut snapshot =
        super::desktop_window_tests::x11_snapshot(generation, x11_handle.xid(), x11_surface_id);
    snapshot.geometry = crate::xwayland::xwm::X11Geometry {
        x: 100,
        y: 0,
        width: 800,
        height: 600,
    };
    snapshot.decoration_hints.motif = crate::xwayland::xwm::X11MotifDecorationHint::Undecorated;
    super::desktop_window_tests::insert_x11(&mut state, snapshot);
    state.test_set_surface_placement(x11_surface_id, SurfacePlacement::absolute_root_at(100, 0));

    let source = state.test_create_data_source(&client, &display.handle());
    for index in 0..64 {
        let mime_type = if index == 0 {
            "text/plain".to_owned()
        } else {
            format!("application/x-test-{index}")
        };
        state.offer_data_source_mime_type(&source, mime_type);
    }
    let omitted_mime_type = "application/x-test-64".to_owned();
    state.offer_data_source_mime_type(&source, omitted_mime_type.clone());
    if let Some(binding) = state.data_sources.get_mut(&source.id()) {
        binding.actions = WaylandDndAction::Copy.mask() | WaylandDndAction::Move.mask();
        binding.actions_set = true;
    }
    state.begin_drag_session(Some(source), origin, None, 7);

    state.update_drag_target_at(10.0, 10.0);
    let first_offer = active_wayland_offer(&state).expect("initial Wayland offer");
    let first_offer_id = first_offer.id();
    assert!(matches!(
        state.active_drag.as_ref().and_then(|drag| drag.target.as_ref()),
        Some(ActiveDragTarget::Wayland { surface, .. })
            if same_surface_resource(surface, &wayland_target)
    ));

    state.update_drag_target_at(110.0, 100.0);
    assert!(!state.data_offers.contains_key(&first_offer_id));
    assert!(matches!(
        state.active_drag.as_ref().and_then(|drag| drag.target.as_ref()),
        Some(ActiveDragTarget::Xwayland { window }) if *window == x11_handle
    ));
    assert_eq!(
        state
            .data_offers
            .values()
            .filter(|offer| offer.kind == DataOfferKind::DragAndDrop)
            .count(),
        0
    );
    let transitions = state.take_xwayland_dnd_transitions();
    assert!(matches!(
        transitions.as_slice(),
        [crate::xwayland::XwaylandDndTransition::TargetEntered {
            target,
            mime_types,
            ..
        }] if *target == x11_handle
            && mime_types.as_slice().len() == 64
            && !mime_types.as_slice().contains(&omitted_mime_type)
    ));
    let session_id = state.active_drag.as_ref().expect("active drag").id;
    assert!(state.update_xwayland_drag_target_status(
        session_id,
        x11_handle,
        true,
        Some(XwaylandDndAction::Copy),
    ));
    assert_eq!(state.active_drag.as_ref().unwrap().accepted_mime, None);

    state.update_drag_target_at(10.0, 10.0);
    let second_offer = active_wayland_offer(&state).expect("new Wayland offer");
    assert_ne!(second_offer.id(), first_offer_id);
    assert!(matches!(
        state.active_drag.as_ref().and_then(|drag| drag.target.as_ref()),
        Some(ActiveDragTarget::Wayland { surface, offer: Some(offer), .. })
            if same_surface_resource(surface, &wayland_target)
                && same_wayland_resource(offer, &second_offer)
    ));
    assert_eq!(
        state
            .data_offers
            .values()
            .filter(|offer| offer.kind == DataOfferKind::DragAndDrop)
            .count(),
        1
    );
}

#[test]
fn x11_status_drop_and_finish_use_exact_canonical_identity_without_mime() {
    let display = Display::<CompositorState>::new().expect("test display");
    let (client, _peer) = test_client(&display);
    let mut state = CompositorState::new(None);
    let generation = generation(41);
    activate_xwayland_generation(&mut state, &client, generation);
    let origin = state.test_create_surface_resource(
        &client,
        &display.handle(),
        40,
        40,
        SurfacePlacement::absolute_root_at(1000, 1000),
    );
    let x11_target = state.test_create_surface_resource(
        &client,
        &display.handle(),
        800,
        600,
        SurfacePlacement::absolute_root_at(100, 0),
    );
    let target_surface_id = compositor_surface_id(&x11_target);
    let target_handle = X11WindowHandle::new(generation, 0x301);
    let mut snapshot = super::desktop_window_tests::x11_snapshot(
        generation,
        target_handle.xid(),
        target_surface_id,
    );
    snapshot.geometry = crate::xwayland::xwm::X11Geometry {
        x: 100,
        y: 0,
        width: 800,
        height: 600,
    };
    snapshot.decoration_hints.motif = crate::xwayland::xwm::X11MotifDecorationHint::Undecorated;
    super::desktop_window_tests::insert_x11(&mut state, snapshot);
    state.test_set_surface_placement(
        target_surface_id,
        SurfacePlacement::absolute_root_at(100, 0),
    );

    let source = state.test_create_data_source(&client, &display.handle());
    state.offer_data_source_mime_type(&source, "text/plain".to_owned());
    if let Some(binding) = state.data_sources.get_mut(&source.id()) {
        binding.actions = WaylandDndAction::Copy.mask() | WaylandDndAction::Move.mask();
        binding.actions_set = true;
    }
    state.begin_drag_session(Some(source), origin, None, 8);
    let session_id = state.active_drag.as_ref().expect("canonical drag").id;
    assert!(matches!(session_id, CanonicalDndSessionId::Wayland(_)));
    state.update_drag_target_at(110.0, 100.0);

    assert!(!state.update_xwayland_drag_target_status(
        CanonicalDndSessionId::Wayland(NonZeroU64::new(999).unwrap()),
        target_handle,
        true,
        Some(XwaylandDndAction::Copy),
    ));
    assert!(!state.update_xwayland_drag_target_status(
        session_id,
        X11WindowHandle::new(generation, target_handle.xid() + 1),
        true,
        Some(XwaylandDndAction::Copy),
    ));
    assert!(!state.update_xwayland_drag_target_status(
        session_id,
        target_handle,
        true,
        Some(XwaylandDndAction::Link),
    ));
    assert!(state.update_xwayland_drag_target_status(
        session_id,
        target_handle,
        true,
        Some(XwaylandDndAction::Copy),
    ));
    assert_eq!(
        state.active_drag.as_ref().map(|drag| drag.selected_action),
        Some(WaylandDndAction::Copy.mask())
    );

    state.drop_active_drag();
    assert_eq!(
        state.active_drag.as_ref().map(|drag| drag.phase),
        Some(DragSessionPhase::DropPendingXwaylandTarget)
    );
    assert!(!state.cancel_xwayland_drag_target(session_id, target_handle));
    let transitions = state.take_xwayland_dnd_transitions();
    assert!(matches!(
        transitions.last(),
        Some(crate::xwayland::XwaylandDndTransition::DropRequested {
            session_id: current,
            target,
            action: XwaylandDndAction::Copy,
            ..
        }) if *current == session_id && *target == target_handle
    ));
    assert!(!state.finish_xwayland_drag_target(
        session_id,
        target_handle,
        true,
        Some(XwaylandDndAction::Move),
    ));
    assert!(state.finish_xwayland_drag_target(
        session_id,
        target_handle,
        true,
        Some(XwaylandDndAction::Copy),
    ));
    assert!(!state.finish_xwayland_drag_target(
        session_id,
        target_handle,
        true,
        Some(XwaylandDndAction::Copy),
    ));
    assert!(state.active_drag.is_none());
    assert_eq!(
        state.compliance_metrics.dnd_last_terminal_phase,
        Some(DragSessionPhase::Finished)
    );
    assert_eq!(state.compliance_metrics.dnd_sessions_finished, 1);
}

#[test]
fn x11_target_finish_does_not_require_a_mime() {
    let mut drag =
        wayland_source_x11_target_drag(XwaylandDndAction::Copy, WaylandDndAction::Copy.mask());
    let active = drag.state.active_drag.as_mut().expect("active drag");
    assert_eq!(active.accepted_mime, None);
    let _ = drag.source_events();
    drag.state.drop_active_drag();
    assert_eq!(
        drag.state
            .active_drag
            .as_ref()
            .map(|active| active.drop_action),
        Some(Some(XwaylandDndAction::Copy))
    );

    assert!(drag.state.finish_xwayland_drag_target(
        drag.session_id,
        drag.target,
        true,
        Some(XwaylandDndAction::Copy),
    ));
    assert!(drag.state.active_drag.is_none());
    assert_eq!(drag.state.compliance_metrics.dnd_sessions_finished, 1);
}

#[test]
fn rejected_xdnd_terminal_cancels_drop_pending_wayland_source_once() {
    let mut drag = wayland_source_x11_target_drag(
        XwaylandDndAction::Copy,
        WaylandDndAction::Copy.mask() | WaylandDndAction::Move.mask(),
    );
    assert!(drag.state.update_xwayland_drag_target_status(
        drag.session_id,
        drag.target,
        true,
        Some(XwaylandDndAction::Copy),
    ));
    let _ = drag.source_events();
    drag.state.drop_active_drag();
    assert_eq!(
        drag.state.active_drag.as_ref().map(|active| active.phase),
        Some(DragSessionPhase::DropPendingXwaylandTarget)
    );
    assert!(
        drag.state
            .finish_xwayland_drag_target(drag.session_id, drag.target, false, None)
    );
    assert!(drag.state.active_drag.is_none());
    assert_eq!(
        drag.state.compliance_metrics.dnd_last_terminal_phase,
        Some(DragSessionPhase::Cancelled)
    );
    assert_eq!(drag.state.compliance_metrics.dnd_sessions_cancelled, 1);
    assert_eq!(drag.state.compliance_metrics.dnd_sessions_finished, 0);
    assert_eq!(
        drag.source_events(),
        [SourceWireEvent::DropPerformed, SourceWireEvent::Cancelled]
    );
    let transitions = drag.state.take_xwayland_dnd_transitions();
    assert!(matches!(
        transitions.last(),
        Some(XwaylandDndTransition::TargetFinished {
            session_id,
            target,
            accepted: false,
            action: None,
        }) if *session_id == drag.session_id && *target == drag.target
    ));
}

#[test]
fn rejected_x11_status_clears_the_canonical_action() {
    let mut drag =
        wayland_source_x11_target_drag(XwaylandDndAction::Copy, WaylandDndAction::Copy.mask());

    assert!(drag.state.update_xwayland_drag_target_status(
        drag.session_id,
        drag.target,
        false,
        Some(XwaylandDndAction::Copy),
    ));
    let active = drag.state.active_drag.as_ref().expect("active drag");
    assert_eq!(active.target_action, None);
    assert_eq!(active.selected_action, 0);
    assert_eq!(active.accepted_mime, None);
}

#[test]
fn non_ask_drop_action_is_frozen_against_late_status() {
    let copy_mask = WaylandDndAction::Copy.mask();
    let move_mask = WaylandDndAction::Move.mask();
    for (frozen_action, late_action, expected_mask) in [
        (XwaylandDndAction::Copy, XwaylandDndAction::Move, copy_mask),
        (XwaylandDndAction::Move, XwaylandDndAction::Copy, move_mask),
    ] {
        let mut drag = wayland_source_x11_target_drag(frozen_action, copy_mask | move_mask);
        let _ = drag.source_events();
        drag.state.drop_active_drag();
        assert_eq!(drag.source_events(), [SourceWireEvent::DropPerformed]);

        assert!(drag.state.update_xwayland_drag_target_status(
            drag.session_id,
            drag.target,
            true,
            Some(late_action),
        ));
        let active = drag.state.active_drag.as_ref().expect("pending drop");
        assert_eq!(active.id, drag.session_id);
        assert!(matches!(
            active.target.as_ref(),
            Some(ActiveDragTarget::Xwayland { window }) if *window == drag.target
        ));
        assert_eq!(active.phase, DragSessionPhase::DropPendingXwaylandTarget);
        assert_eq!(active.target_action, Some(frozen_action));
        assert_eq!(active.drop_action, Some(frozen_action));
        assert_eq!(active.selected_action, expected_mask);
        assert!(!drag.state.update_xwayland_drag_target_status(
            CanonicalDndSessionId::Wayland(NonZeroU64::new(999).unwrap()),
            drag.target,
            true,
            Some(late_action),
        ));
        assert_eq!(
            drag.state
                .active_drag
                .as_ref()
                .and_then(|active| active.target_action),
            Some(frozen_action)
        );
        assert_eq!(
            drag.state
                .active_drag
                .as_ref()
                .and_then(|active| active.drop_action),
            Some(frozen_action)
        );
        assert!(drag.source_events().is_empty());

        assert!(
            drag.state
                .finish_xwayland_drag_target(drag.session_id, drag.target, false, None,)
        );
        assert!(drag.state.active_drag.is_none());
        assert_eq!(drag.state.compliance_metrics.dnd_sessions_cancelled, 1);
        assert_eq!(drag.state.compliance_metrics.dnd_sessions_finished, 0);
        assert_eq!(drag.source_events(), [SourceWireEvent::Cancelled]);
        assert!(!drag.state.finish_xwayland_drag_target(
            drag.session_id,
            drag.target,
            true,
            Some(late_action),
        ));
        assert!(!drag.state.update_xwayland_drag_target_status(
            drag.session_id,
            drag.target,
            true,
            Some(late_action),
        ));
        assert!(drag.source_events().is_empty());
        assert_eq!(drag.state.compliance_metrics.dnd_sessions_cancelled, 1);
    }
}

#[test]
fn matching_late_non_ask_status_preserves_action_and_allows_terminal_success() {
    let copy_mask = WaylandDndAction::Copy.mask();
    let move_mask = WaylandDndAction::Move.mask();
    for (action, action_mask) in [
        (XwaylandDndAction::Copy, copy_mask),
        (XwaylandDndAction::Move, move_mask),
    ] {
        let mut drag = wayland_source_x11_target_drag(action, copy_mask | move_mask);
        let _ = drag.source_events();
        drag.state.drop_active_drag();
        assert_eq!(drag.source_events(), [SourceWireEvent::DropPerformed]);
        assert!(drag.state.update_xwayland_drag_target_status(
            drag.session_id,
            drag.target,
            true,
            Some(action),
        ));
        let active = drag.state.active_drag.as_ref().expect("pending drop");
        assert_eq!(active.target_action, Some(action));
        assert_eq!(active.selected_action, action_mask);
        assert_eq!(active.drop_action, Some(action));
        assert!(drag.state.finish_xwayland_drag_target(
            drag.session_id,
            drag.target,
            true,
            Some(action),
        ));
        assert!(drag.state.active_drag.is_none());
        assert_eq!(drag.state.compliance_metrics.dnd_sessions_finished, 1);
    }
}

#[test]
fn exact_xdnd_selection_source_data_is_served_until_canonical_finish() {
    use std::os::fd::OwnedFd;

    let mut drag =
        wayland_source_x11_target_drag(XwaylandDndAction::Copy, WaylandDndAction::Copy.mask());
    let _ = drag.source_events();
    drag.state.drop_active_drag();
    let (sink, _reader) = UnixStream::pair().expect("payload sink");
    let adapter_id = crate::xwayland::XwaylandDndAdapterId::new(drag.session_id, drag.generation)
        .expect("exact adapter identity");

    assert!(drag.state.request_xwayland_dnd_source_data(
        crate::xwayland::XwaylandDndSourceDataRequest {
            transfer_id: crate::xwayland::XwaylandDndSourceTransferId {
                source: crate::xwayland::XwaylandDndSourceProxyId {
                    adapter_id,
                    xid: 0x880,
                },
                serial: NonZeroU64::new(1).unwrap(),
            },
            target: drag.target,
            requestor: drag.target.xid(),
            mime_type: "text/plain".to_owned(),
            sink: OwnedFd::from(sink),
        }
    ));
}

#[test]
fn dnd_transitions_leave_clipboard_and_primary_selection_state_untouched() {
    let display = Display::<CompositorState>::new().expect("test display");
    let (client, _peer) = test_client(&display);
    let mut state = CompositorState::new(None);
    activate_xwayland_generation(&mut state, &client, generation(41));

    let selection_offers = [
        (
            crate::compositor::SelectionKind::Clipboard,
            crate::compositor::SelectionSourceKind::WaylandClipboard,
            crate::compositor::SelectionSourceKey(91),
        ),
        (
            crate::compositor::SelectionKind::Primary,
            crate::compositor::SelectionSourceKind::WaylandPrimary,
            crate::compositor::SelectionSourceKey(92),
        ),
    ]
    .into_iter()
    .map(|(kind, source_kind, key)| {
        state
            .selection_state
            .register_source(key, source_kind, Some(key.0));
        state
            .selection_state
            .offer_source_mime_type_for_key(key, "text/plain");
        let epoch = state.selection_state.allocate_mutation_epoch();
        let commit = state
            .selection_state
            .commit_selection(kind, key, epoch)
            .expect("selection commit");
        let offer_id = state
            .selection_state
            .register_offer(kind, 77, commit.generation)
            .expect("selection offer");
        (kind, key, commit.generation, offer_id)
    })
    .collect::<Vec<_>>();

    let selection_snapshot = |state: &CompositorState| {
        selection_offers
            .iter()
            .map(|(kind, key, generation, offer_id)| {
                (
                    state.selection_state.active_selection(*kind).cloned(),
                    state.selection_state.current_generation(*kind),
                    state.selection_state.current_mutation_epoch(*kind),
                    state.selection_state.offer_is_current(
                        *offer_id,
                        *kind,
                        *generation,
                        77,
                        *key,
                        "text/plain",
                    ),
                )
            })
            .collect::<Vec<_>>()
    };
    let before = selection_snapshot(&state);

    let _target = state.test_create_surface_resource(
        &client,
        &display.handle(),
        80,
        80,
        SurfacePlacement::absolute_root_at(0, 0),
    );
    state.test_create_data_device(&client, &display.handle());
    let offer = xwayland_offer(5, 0x105, vec![XwaylandDndAction::Copy]);
    let offer_id = offer.id();
    assert!(state.begin_xwayland_drag_session(offer));
    state.update_drag_target_at(10.0, 10.0);
    let dnd_offer = active_wayland_offer(&state).expect("DND offer");
    state.receive_clipboard_offer(
        &dnd_offer,
        &client.id(),
        0,
        "text/plain".to_owned(),
        UnixStream::pair().expect("DND payload pipe").1.into(),
    );
    assert_eq!(state.xwayland_dnd_data_requests.len(), 1);
    assert!(state.cancel_xwayland_drag(offer_id));

    assert_eq!(selection_snapshot(&state), before);
    assert!(state.xwayland_selection_data_requests.is_empty());
}

#[test]
fn x11_target_window_retirement_after_drop_cancels_wayland_source_once() {
    let mut drag = wayland_source_x11_target_drag(
        XwaylandDndAction::Copy,
        WaylandDndAction::Copy.mask() | WaylandDndAction::Move.mask(),
    );
    assert!(
        !drag
            .source_events()
            .iter()
            .any(|event| matches!(event, SourceWireEvent::Target(Some(_))))
    );

    drag.state.drop_active_drag();
    assert_eq!(
        drag.state.active_drag.as_ref().map(|active| active.phase),
        Some(DragSessionPhase::DropPendingXwaylandTarget)
    );
    assert_eq!(drag.source_events(), [SourceWireEvent::DropPerformed]);

    drag.remove_target_window();
    assert!(drag.state.active_drag.is_none());
    assert_eq!(drag.state.compliance_metrics.dnd_source_cancelled_events, 1);
    assert_eq!(drag.state.compliance_metrics.dnd_sessions_cancelled, 1);
    assert_eq!(drag.state.compliance_metrics.dnd_sessions_finished, 0);
    let terminal = drag.source_events();
    assert_eq!(
        terminal,
        [SourceWireEvent::Target(None), SourceWireEvent::Cancelled]
    );
}

#[test]
fn x11_generation_retirement_after_drop_cancels_wayland_source_once() {
    let mut drag = wayland_source_x11_target_drag(
        XwaylandDndAction::Copy,
        WaylandDndAction::Copy.mask() | WaylandDndAction::Move.mask(),
    );
    let _ = drag.source_events();
    drag.state.drop_active_drag();
    assert_eq!(drag.source_events(), [SourceWireEvent::DropPerformed]);

    drag.state.clear_xwayland_generation(drag.generation);
    assert!(drag.state.active_drag.is_none());
    assert!(drag.state.xwayland.client_identity.is_none());
    assert_eq!(drag.state.compliance_metrics.dnd_source_cancelled_events, 1);
    assert_eq!(drag.state.compliance_metrics.dnd_sessions_cancelled, 1);
    assert_eq!(drag.state.compliance_metrics.dnd_sessions_finished, 0);
    let terminal = drag.source_events();
    assert_eq!(
        terminal,
        [SourceWireEvent::Target(None), SourceWireEvent::Cancelled]
    );
}

#[test]
fn native_wayland_target_cancellation_withdraws_target_before_cancelled() {
    let mut drag = wayland_source_x11_target_drag(
        XwaylandDndAction::Copy,
        WaylandDndAction::Copy.mask() | WaylandDndAction::Move.mask(),
    );
    drag.state.update_drag_target_at(10.0, 10.0);
    assert!(matches!(
        drag.state.active_drag.as_ref().and_then(|active| active.target.as_ref()),
        Some(ActiveDragTarget::Wayland { surface, .. })
            if same_surface_resource(surface, &drag.wayland_target_surface)
    ));
    assert!(
        drag.source_events()
            .ends_with(&[SourceWireEvent::Target(None)])
    );

    drag.state
        .cancel_drag_session("native_wayland_target_cancel_test");

    assert!(drag.state.active_drag.is_none());
    assert_eq!(drag.state.compliance_metrics.dnd_source_cancelled_events, 1);
    assert_eq!(drag.state.compliance_metrics.dnd_sessions_cancelled, 1);
    assert_eq!(
        drag.source_events(),
        [SourceWireEvent::Target(None), SourceWireEvent::Cancelled]
    );
    assert!(matches!(
        drag.state.take_xwayland_dnd_transitions().as_slice(),
        [
            XwaylandDndTransition::TargetEntered { target: entered, .. },
            XwaylandDndTransition::TargetLeft { target: left, .. },
            XwaylandDndTransition::Retired {
                session_id,
                generation,
            },
        ] if *entered == drag.target
            && *left == drag.target
            && *session_id == drag.session_id
            && *generation == drag.generation
    ));
}

#[test]
fn xwayland_generation_retirement_preserves_unrelated_wayland_drag() {
    let mut drag = wayland_source_x11_target_drag(
        XwaylandDndAction::Copy,
        WaylandDndAction::Copy.mask() | WaylandDndAction::Move.mask(),
    );
    drag.state.update_drag_target_at(10.0, 10.0);
    assert!(matches!(
        drag.state.active_drag.as_ref().and_then(|active| active.target.as_ref()),
        Some(ActiveDragTarget::Wayland { surface, .. })
            if same_surface_resource(surface, &drag.wayland_target_surface)
    ));

    drag.state.clear_xwayland_generation(drag.generation);

    let active = drag
        .state
        .active_drag
        .as_ref()
        .expect("Wayland drag remains active");
    assert_eq!(active.id, drag.session_id);
    assert_eq!(active.phase, DragSessionPhase::Dragging);
    assert_eq!(active.xwayland_dnd_generation, None);
    assert!(matches!(
        active.target.as_ref(),
        Some(ActiveDragTarget::Wayland { surface, .. })
            if same_surface_resource(surface, &drag.wayland_target_surface)
    ));
    assert_eq!(drag.state.compliance_metrics.dnd_source_cancelled_events, 0);
    assert_eq!(drag.state.compliance_metrics.dnd_sessions_cancelled, 0);
}

#[test]
fn x11_target_retirement_before_drop_only_leaves_target_once() {
    let mut drag = wayland_source_x11_target_drag(
        XwaylandDndAction::Copy,
        WaylandDndAction::Copy.mask() | WaylandDndAction::Move.mask(),
    );
    assert!(
        !drag
            .source_events()
            .iter()
            .any(|event| matches!(event, SourceWireEvent::Target(Some(_))))
    );
    assert!(matches!(
        drag.state.take_xwayland_dnd_transitions().as_slice(),
        [crate::xwayland::XwaylandDndTransition::TargetEntered { target, .. }]
            if *target == drag.target
    ));

    drag.state.retire_xwayland_drag_target(drag.target);
    assert_eq!(
        drag.state.active_drag.as_ref().map(|active| active.phase),
        Some(DragSessionPhase::Dragging)
    );
    assert!(
        drag.state
            .active_drag
            .as_ref()
            .is_some_and(|active| active.target.is_none() && active.accepted_mime.is_none())
    );
    assert_eq!(drag.state.compliance_metrics.dnd_source_cancelled_events, 0);
    assert!(matches!(
        drag.state.take_xwayland_dnd_transitions().as_slice(),
        [crate::xwayland::XwaylandDndTransition::TargetLeft { session_id, target }]
            if *session_id == drag.session_id && *target == drag.target
    ));
    assert_eq!(drag.source_events(), [SourceWireEvent::Target(None)]);

    drag.state.retire_xwayland_drag_target(drag.target);
    assert!(drag.state.take_xwayland_dnd_transitions().is_empty());
    assert!(drag.source_events().is_empty());
    assert_eq!(drag.state.compliance_metrics.dnd_source_cancelled_events, 0);
}

#[test]
fn stale_x11_target_events_cannot_finish_a_replacement_drag() {
    let mut drag = wayland_source_x11_target_drag(
        XwaylandDndAction::Copy,
        WaylandDndAction::Copy.mask() | WaylandDndAction::Move.mask(),
    );
    let _ = drag.source_events();
    drag.state.drop_active_drag();
    let _ = drag.source_events();
    drag.remove_target_window();
    assert!(drag.state.active_drag.is_none());
    assert_eq!(drag.state.compliance_metrics.dnd_source_cancelled_events, 1);
    let _ = drag.source_events();

    let replacement_generation = generation(62);
    activate_xwayland_generation(&mut drag.state, &drag._client, replacement_generation);
    let replacement_target = install_x11_drag_target(
        &mut drag.state,
        &drag.x11_target_surface,
        replacement_generation,
        drag.target.xid(),
    );
    let replacement_source = drag
        .state
        .test_create_data_source(&drag._client, &drag.display.handle());
    drag.state
        .offer_data_source_mime_type(&replacement_source, "text/plain".to_owned());
    if let Some(binding) = drag.state.data_sources.get_mut(&replacement_source.id()) {
        binding.actions = WaylandDndAction::Copy.mask();
        binding.actions_set = true;
    }
    let replacement_origin = drag.state.test_create_surface_resource(
        &drag._client,
        &drag.display.handle(),
        40,
        40,
        SurfacePlacement::absolute_root_at(1000, 1000),
    );
    drag.state
        .begin_drag_session(Some(replacement_source), replacement_origin, None, 9);
    let replacement_id = drag
        .state
        .active_drag
        .as_ref()
        .expect("replacement drag")
        .id;
    drag.state.update_drag_target_at(110.0, 100.0);
    assert!(matches!(
        drag.state.active_drag.as_ref().and_then(|active| active.target.as_ref()),
        Some(ActiveDragTarget::Xwayland { window }) if *window == replacement_target
    ));

    assert!(!drag.state.update_xwayland_drag_target_status(
        drag.session_id,
        drag.target,
        true,
        Some(XwaylandDndAction::Copy),
    ));
    assert!(!drag.state.finish_xwayland_drag_target(
        drag.session_id,
        drag.target,
        true,
        Some(XwaylandDndAction::Copy),
    ));
    assert_eq!(
        drag.state.active_drag.as_ref().map(|active| active.id),
        Some(replacement_id)
    );
    assert_eq!(
        drag.state.active_drag.as_ref().map(|active| active.phase),
        Some(DragSessionPhase::Dragging)
    );
    assert_eq!(drag.state.compliance_metrics.dnd_source_cancelled_events, 1);
}

#[test]
fn leaving_x11_target_clears_mime_feedback_when_switching_or_having_no_target() {
    let mut switch_to_wayland = wayland_source_x11_target_drag(
        XwaylandDndAction::Copy,
        WaylandDndAction::Copy.mask() | WaylandDndAction::Move.mask(),
    );
    assert!(
        !switch_to_wayland
            .source_events()
            .iter()
            .any(|event| matches!(event, SourceWireEvent::Target(Some(_))))
    );
    switch_to_wayland.state.update_drag_target_at(10.0, 10.0);
    assert!(matches!(
        switch_to_wayland
            .state
            .active_drag
            .as_ref()
            .and_then(|active| active.target.as_ref()),
        Some(ActiveDragTarget::Wayland { surface, .. })
            if same_surface_resource(surface, &switch_to_wayland.wayland_target_surface)
    ));
    assert!(
        switch_to_wayland
            .state
            .active_drag
            .as_ref()
            .is_some_and(|active| active.accepted_mime.is_none())
    );
    assert_eq!(
        switch_to_wayland.source_events(),
        [SourceWireEvent::Target(None)]
    );

    let mut move_to_no_target = wayland_source_x11_target_drag(
        XwaylandDndAction::Copy,
        WaylandDndAction::Copy.mask() | WaylandDndAction::Move.mask(),
    );
    assert!(
        !move_to_no_target
            .source_events()
            .iter()
            .any(|event| matches!(event, SourceWireEvent::Target(Some(_))))
    );
    move_to_no_target
        .state
        .update_drag_target_at(1200.0, 1200.0);
    assert!(
        move_to_no_target
            .state
            .active_drag
            .as_ref()
            .is_some_and(|active| active.target.is_none() && active.accepted_mime.is_none())
    );
    assert_eq!(
        move_to_no_target.source_events(),
        [SourceWireEvent::Target(None)]
    );
}

#[test]
fn ask_from_x11_target_requires_an_explicit_supported_copy_or_move_resolution() {
    let copy = WaylandDndAction::Copy.mask();
    let move_action = WaylandDndAction::Move.mask();
    let ask = WaylandDndAction::Ask.mask();
    let cases = [
        (None, copy | move_action | ask),
        (Some(XwaylandDndAction::Ask), copy | move_action | ask),
        (Some(XwaylandDndAction::Link), copy | move_action | ask),
        (Some(XwaylandDndAction::Private), copy | move_action | ask),
        (Some(XwaylandDndAction::Copy), move_action | ask),
        (Some(XwaylandDndAction::Move), copy | ask),
    ];

    for (final_action, source_actions) in cases {
        let mut drag = wayland_source_x11_target_drag(XwaylandDndAction::Ask, source_actions);
        let _ = drag.source_events();
        drag.state.drop_active_drag();
        assert_eq!(drag.source_events(), [SourceWireEvent::DropPerformed]);

        assert!(!drag.state.finish_xwayland_drag_target(
            drag.session_id,
            drag.target,
            true,
            final_action,
        ));
        assert_eq!(
            drag.state.active_drag.as_ref().map(|active| active.phase),
            Some(DragSessionPhase::DropPendingXwaylandTarget)
        );
        assert_eq!(drag.state.compliance_metrics.dnd_sessions_finished, 0);
        assert_eq!(drag.state.compliance_metrics.dnd_source_finished_events, 0);
        assert!(drag.source_events().is_empty());
    }
}

#[test]
fn ask_from_x11_target_finishes_only_after_source_supported_copy_or_move_feedback() {
    let copy = WaylandDndAction::Copy.mask();
    let move_action = WaylandDndAction::Move.mask();
    let ask = WaylandDndAction::Ask.mask();
    for (final_action, expected_mask) in [
        (XwaylandDndAction::Copy, copy),
        (XwaylandDndAction::Move, move_action),
    ] {
        let mut drag =
            wayland_source_x11_target_drag(XwaylandDndAction::Ask, copy | move_action | ask);
        let _ = drag.source_events();
        drag.state.drop_active_drag();
        assert_eq!(drag.source_events(), [SourceWireEvent::DropPerformed]);

        assert!(drag.state.update_xwayland_drag_target_status(
            drag.session_id,
            drag.target,
            true,
            Some(final_action),
        ));
        assert_eq!(
            drag.state
                .active_drag
                .as_ref()
                .and_then(|active| active.target_action),
            Some(XwaylandDndAction::Ask)
        );
        assert_eq!(
            drag.state
                .active_drag
                .as_ref()
                .and_then(|active| active.drop_action),
            Some(XwaylandDndAction::Ask)
        );

        assert!(drag.state.finish_xwayland_drag_target(
            drag.session_id,
            drag.target,
            true,
            Some(final_action),
        ));
        assert!(drag.state.active_drag.is_none());
        assert_eq!(drag.state.compliance_metrics.dnd_sessions_finished, 1);
        assert_eq!(drag.state.compliance_metrics.dnd_sessions_cancelled, 0);
        assert_eq!(drag.state.compliance_metrics.dnd_source_finished_events, 1);
        assert_eq!(
            drag.source_events(),
            [
                SourceWireEvent::Action(expected_mask),
                SourceWireEvent::Finished
            ]
        );
        assert!(!drag.state.finish_xwayland_drag_target(
            drag.session_id,
            drag.target,
            true,
            Some(final_action),
        ));
        assert_eq!(drag.state.compliance_metrics.dnd_sessions_finished, 1);
        assert_eq!(drag.state.compliance_metrics.dnd_source_finished_events, 1);
    }
}

#[test]
fn xwayland_source_retains_semantic_ask_action_domain_on_x11_finish() {
    let display = Display::<CompositorState>::new().expect("test display");
    let (client, _peer) = test_client(&display);
    let mut state = CompositorState::new(None);
    let generation = generation(41);
    activate_xwayland_generation(&mut state, &client, generation);
    let target_surface = state.test_create_surface_resource(
        &client,
        &display.handle(),
        800,
        600,
        SurfacePlacement::absolute_root_at(100, 0),
    );
    let target = install_x11_drag_target(&mut state, &target_surface, generation, 0x701);
    let offer = xwayland_offer(7, 0x700, vec![XwaylandDndAction::Ask]);
    let offer_id = offer.id();
    let session_id = CanonicalDndSessionId::Xwayland(offer_id);
    assert!(state.begin_xwayland_drag_session(offer));
    state.update_drag_target_at(110.0, 100.0);
    assert!(state.update_xwayland_drag_target_status(
        session_id,
        target,
        true,
        Some(XwaylandDndAction::Ask),
    ));
    assert!(state.drop_xwayland_drag(offer_id));
    assert!(state.finish_xwayland_drag_target(session_id, target, true, None));
    assert!(state.active_drag.is_none());
    assert_eq!(state.compliance_metrics.dnd_sessions_finished, 1);
    assert!(matches!(
        state.take_xwayland_dnd_transitions().last(),
        Some(crate::xwayland::XwaylandDndTransition::TargetFinished {
            session_id: current,
            target: current_target,
            accepted: true,
            action: Some(XwaylandDndAction::Ask),
        }) if *current == session_id && *current_target == target
    ));
}
