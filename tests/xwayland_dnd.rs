use std::num::NonZeroU64;

use oblivion_one::xwayland::{
    CanonicalDndSessionId, WaylandDndAction, X11WindowHandle, XwaylandDndAction,
    XwaylandDndAdapterId, XwaylandDndMimeCatalog, XwaylandDndOffer, XwaylandDndOfferId,
    XwaylandDndVersion, XwaylandGeneration,
    xwm::data_bridge::dnd::{DndManager, DndWireProgress},
};

fn generation(value: u64) -> XwaylandGeneration {
    XwaylandGeneration::new(NonZeroU64::new(value).expect("nonzero"))
}

fn offer_id(generation: XwaylandGeneration, serial: u64) -> XwaylandDndOfferId {
    XwaylandDndOfferId::new(
        generation,
        NonZeroU64::new(serial).expect("nonzero offer serial"),
    )
}

#[test]
fn xdnd_actions_convert_semantically_without_numeric_aliasing() {
    assert_eq!(
        XwaylandDndAction::Copy.to_wayland_action(),
        Some(WaylandDndAction::Copy)
    );
    assert_eq!(
        XwaylandDndAction::Move.to_wayland_action(),
        Some(WaylandDndAction::Move)
    );
    assert_eq!(
        XwaylandDndAction::Ask.to_wayland_action(),
        Some(WaylandDndAction::Ask)
    );
    assert_eq!(XwaylandDndAction::Link.to_wayland_action(), None);
    assert_eq!(XwaylandDndAction::Private.to_wayland_action(), None);
}

#[test]
fn dnd_offer_is_generation_qualified_and_has_bounded_metadata() {
    let offer_generation = generation(7);
    let source = X11WindowHandle::new(offer_generation, 11);
    let id = offer_id(offer_generation, 1);
    let mime_types =
        XwaylandDndMimeCatalog::try_new(vec!["text/plain".to_owned(), "text/uri-list".to_owned()])
            .unwrap();
    let offer = XwaylandDndOffer::new(
        id,
        source,
        XwaylandDndVersion::new(5).unwrap(),
        mime_types,
        vec![XwaylandDndAction::Copy, XwaylandDndAction::Link],
    )
    .unwrap();

    assert_eq!(offer.id(), id);
    assert_eq!(offer.source(), source);
    assert_eq!(
        offer.mime_types().as_slice(),
        ["text/plain", "text/uri-list"]
    );
    assert_eq!(
        offer.source_actions(),
        [XwaylandDndAction::Copy, XwaylandDndAction::Link]
    );
    assert!(
        XwaylandDndAdapterId::new(CanonicalDndSessionId::Xwayland(id), generation(8),).is_none()
    );
    assert!(
        XwaylandDndOffer::new(
            id,
            X11WindowHandle::new(generation(8), 11),
            XwaylandDndVersion::new(5).unwrap(),
            XwaylandDndMimeCatalog::default(),
            vec![],
        )
        .is_err()
    );
    assert!(
        XwaylandDndMimeCatalog::try_new(vec![
            "text/plain".to_owned();
            XwaylandDndMimeCatalog::MAX_MIME_TYPES + 1
        ])
        .is_err()
    );
}

#[test]
fn adapter_replacement_retires_exact_identity_and_consumes_terminal_once() {
    let generation = generation(7);
    let source = X11WindowHandle::new(generation, 11);
    let first_target = X11WindowHandle::new(generation, 21);
    let first_session = CanonicalDndSessionId::Xwayland(offer_id(generation, 1));
    let first = XwaylandDndAdapterId::new(first_session, generation).unwrap();
    let mut manager = DndManager::default();
    assert!(manager.install_canonical_session(first, Some(source)));
    assert_eq!(
        manager.progress(first),
        Some(DndWireProgress::AwaitingEnter)
    );
    assert!(manager.mark_entered(first));
    assert!(manager.position(first, first_target, 40, 50, Some(XwaylandDndAction::Copy),));
    assert!(manager.mark_drop_ready(first));
    assert!(manager.consume_terminal_event(first));
    assert!(!manager.consume_terminal_event(first));
    assert!(manager.terminal_event_consumed(first));
    assert!(!manager.install_canonical_session(first, Some(source)));

    let second_session =
        CanonicalDndSessionId::Wayland(NonZeroU64::new(2).expect("nonzero session"));
    let second = XwaylandDndAdapterId::new(second_session, generation).unwrap();
    assert!(manager.install_canonical_session(second, None));
    assert_eq!(manager.active_id(), Some(second));
    assert_eq!(manager.progress(first), None);
    assert!(!manager.position(first, first_target, 1, 2, Some(XwaylandDndAction::Move),));
    assert!(!manager.consume_terminal_event(first));
    assert_eq!(
        manager.progress(second),
        Some(DndWireProgress::AwaitingEnter)
    );
    assert_eq!(
        manager.active_session().map(|session| session.id),
        Some(second)
    );
}

#[test]
fn generation_retirement_only_clears_the_exact_adapter_session() {
    let current_generation = generation(8);
    let id = XwaylandDndAdapterId::new(
        CanonicalDndSessionId::Wayland(NonZeroU64::new(12).expect("nonzero session")),
        current_generation,
    )
    .unwrap();
    let mut manager = DndManager::default();
    assert!(manager.install_canonical_session(id, None));

    manager.clear_generation(generation(7));
    assert_eq!(manager.active_id(), Some(id));
    manager.clear_generation(current_generation);
    assert_eq!(manager.active_id(), None);
    assert!(!manager.mark_entered(id));
}
