use super::*;

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
