use super::*;
use crate::xwayland::{
    CanonicalDndSessionId, WaylandDndAction, X11WindowHandle, XwaylandDndAction,
    XwaylandDndMimeCatalog, XwaylandDndOffer, XwaylandDndOfferId, XwaylandDndVersion,
    XwaylandGeneration,
};
use std::{
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
    assert!(state.xwayland_dnd_transition.is_none());
    assert!(state.xwayland.client_identity.is_none());
    assert!(state.last_xwayland_dnd_offer_id.is_none());
    assert!(!state.begin_xwayland_drag_session(xwayland_offer(
        1,
        0x100,
        vec![XwaylandDndAction::Copy],
    )));
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
    assert!(matches!(
        state.xwayland_dnd_transition.take(),
        Some(crate::xwayland::XwaylandDndTransition::TargetEntered {
            target,
            mime_types,
            ..
        }) if target == x11_handle
            && mime_types.as_slice().len() == 64
            && !mime_types.as_slice().contains(&omitted_mime_type)
    ));
    let session_id = state.active_drag.as_ref().expect("active drag").id;
    assert!(!state.update_xwayland_drag_target_status(
        session_id,
        x11_handle,
        Some(omitted_mime_type),
        Some(XwaylandDndAction::Copy),
    ));

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
fn x11_target_status_drop_and_finish_use_exact_canonical_identity_and_typed_action() {
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
        Some("text/plain".to_owned()),
        Some(XwaylandDndAction::Copy),
    ));
    assert!(!state.update_xwayland_drag_target_status(
        session_id,
        X11WindowHandle::new(generation, target_handle.xid() + 1),
        Some("text/plain".to_owned()),
        Some(XwaylandDndAction::Copy),
    ));
    assert!(!state.update_xwayland_drag_target_status(
        session_id,
        target_handle,
        Some("text/plain".to_owned()),
        Some(XwaylandDndAction::Link),
    ));
    assert!(state.update_xwayland_drag_target_status(
        session_id,
        target_handle,
        Some("text/plain".to_owned()),
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
    assert!(matches!(
        state.xwayland_dnd_transition.take(),
        Some(crate::xwayland::XwaylandDndTransition::DropRequested {
            session_id: current,
            target,
            mime_type,
            action: XwaylandDndAction::Copy,
            ..
        }) if current == session_id && target == target_handle && mime_type == "text/plain"
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
