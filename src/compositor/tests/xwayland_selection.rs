use super::*;
use std::num::NonZeroU64;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;

use crate::compositor::{SelectionSourceBackend, SelectionSourceKind};
use crate::xwayland::{
    XwaylandGeneration, XwaylandSelectionEvent, XwaylandSelectionKind, XwaylandSelectionOffer,
    XwaylandSelectionOfferId,
};

fn offer(kind: XwaylandSelectionKind, generation: u64, revision: u64) -> XwaylandSelectionOffer {
    XwaylandSelectionOffer {
        id: XwaylandSelectionOfferId {
            generation: XwaylandGeneration::new(NonZeroU64::new(generation).unwrap()),
            kind,
            revision,
        },
        mime_types: vec!["image/png".to_owned(), "text/plain".to_owned()],
    }
}

#[test]
fn xwayland_clipboard_offer_is_committed_to_canonical_selection_state() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    let offer = offer(XwaylandSelectionKind::Clipboard, 1, 7);

    server.apply_xwayland_selection_event(XwaylandSelectionEvent::OfferChanged {
        kind: XwaylandSelectionKind::Clipboard,
        offer: offer.clone(),
    });

    let active = server
        .state
        .selection_state
        .active_selection(SelectionKind::Clipboard)
        .expect("XWayland offer should become the canonical clipboard source");
    assert_eq!(active.source_kind, SelectionSourceKind::Xwayland);
    assert_eq!(active.mime_types, offer.mime_types);
    assert!(matches!(
        server.state.selection_state.source_backend(active.source_key),
        Some(SelectionSourceBackend::Xwayland { offer_id }) if *offer_id == offer.id
    ));
}

#[test]
fn stale_xwayland_clear_does_not_remove_a_newer_generation() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    let current = offer(XwaylandSelectionKind::Primary, 2, 9);
    server.apply_xwayland_selection_event(XwaylandSelectionEvent::OfferChanged {
        kind: XwaylandSelectionKind::Primary,
        offer: current.clone(),
    });

    server.apply_xwayland_selection_event(XwaylandSelectionEvent::Cleared {
        kind: XwaylandSelectionKind::Primary,
        generation: XwaylandGeneration::new(NonZeroU64::new(1).unwrap()),
    });

    let active = server
        .state
        .selection_state
        .active_selection(SelectionKind::Primary)
        .expect("late G1 clear must preserve the G2 primary offer");
    assert_eq!(active.source_kind, SelectionSourceKind::Xwayland);
    assert_eq!(active.mime_types, current.mime_types);
}

#[test]
fn primary_offer_is_committed_and_matching_generation_clear_removes_it() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    let primary = offer(XwaylandSelectionKind::Primary, 3, 11);
    server.apply_xwayland_selection_event(XwaylandSelectionEvent::OfferChanged {
        kind: XwaylandSelectionKind::Primary,
        offer: primary.clone(),
    });
    let active = server
        .state
        .selection_state
        .active_selection(SelectionKind::Primary)
        .expect("XWayland PRIMARY offer should be canonical");
    assert_eq!(active.source_kind, SelectionSourceKind::Xwayland);
    assert_eq!(active.mime_types, primary.mime_types);

    server.apply_xwayland_selection_event(XwaylandSelectionEvent::Cleared {
        kind: XwaylandSelectionKind::Primary,
        generation: primary.id.generation,
    });
    assert!(
        server
            .state
            .selection_state
            .active_selection(SelectionKind::Primary)
            .is_none()
    );
}

#[test]
fn stale_xwayland_clear_does_not_remove_host_clipboard_source() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    server.state.install_host_clipboard_selection(
        crate::compositor::HostClipboardOfferId(44),
        vec!["text/plain".to_owned()],
    );
    server.apply_xwayland_selection_event(XwaylandSelectionEvent::Cleared {
        kind: XwaylandSelectionKind::Clipboard,
        generation: XwaylandGeneration::new(NonZeroU64::new(1).unwrap()),
    });
    let active = server
        .state
        .selection_state
        .active_selection(SelectionKind::Clipboard)
        .expect("stale XWayland clear must preserve the host clipboard");
    assert_eq!(active.source_kind, SelectionSourceKind::HostClipboardBridge);
}

#[test]
fn xwayland_receive_moves_sink_into_bounded_mailbox() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    let current = offer(XwaylandSelectionKind::Clipboard, 4, 12);
    server.apply_xwayland_selection_event(XwaylandSelectionEvent::OfferChanged {
        kind: XwaylandSelectionKind::Clipboard,
        offer: current.clone(),
    });
    let active = server
        .state
        .selection_state
        .active_selection(SelectionKind::Clipboard)
        .expect("canonical source");
    let source_key = active.source_key;
    let (sink, _reader) = UnixStream::pair().unwrap();
    let sink_fd = sink.as_raw_fd();
    server.state.request_selection_data(
        SelectionKind::Clipboard,
        source_key,
        "image/png".to_owned(),
        sink.into(),
    );
    let mut requests = server.take_xwayland_selection_data_requests();
    assert_eq!(requests.len(), 1);
    let request = requests.pop().unwrap();
    assert_eq!(request.offer_id, current.id);
    assert_eq!(request.mime_type, "image/png");
    assert_eq!(request.sink.as_raw_fd(), sink_fd);
}

#[test]
fn xwayland_clipboard_and_primary_are_published_to_data_control_clients() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let manager: client_ext_data_control_manager_v1::ExtDataControlManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let _device = manager.get_data_device(&seat, &qh, ());
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();

    let clipboard = offer(XwaylandSelectionKind::Clipboard, 5, 13);
    let primary = offer(XwaylandSelectionKind::Primary, 5, 14);
    commands
        .send(ServerCommand::ApplyXwaylandSelectionEvent(
            XwaylandSelectionEvent::OfferChanged {
                kind: XwaylandSelectionKind::Clipboard,
                offer: clipboard,
            },
        ))
        .unwrap();
    commands
        .send(ServerCommand::ApplyXwaylandSelectionEvent(
            XwaylandSelectionEvent::OfferChanged {
                kind: XwaylandSelectionKind::Primary,
                offer: primary,
            },
        ))
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    assert!(state.data_control_selection_events.ends_with(&[true]));
    assert!(
        state
            .data_control_primary_selection_events
            .ends_with(&[true])
    );
    assert!(state.data_control_clipboard_offer.as_ref().is_some());
    assert!(state.data_control_primary_offer.as_ref().is_some());
    stop_controllable_test_server(commands, server_thread);
}

#[test]
fn xwayland_receive_mailbox_drops_requests_after_sixty_four() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    let current = offer(XwaylandSelectionKind::Clipboard, 6, 15);
    server.apply_xwayland_selection_event(XwaylandSelectionEvent::OfferChanged {
        kind: XwaylandSelectionKind::Clipboard,
        offer: current,
    });
    let source_key = server
        .state
        .selection_state
        .active_selection(SelectionKind::Clipboard)
        .unwrap()
        .source_key;
    for _ in 0..65 {
        let (sink, _reader) = UnixStream::pair().unwrap();
        server.state.request_selection_data(
            SelectionKind::Clipboard,
            source_key,
            "image/png".to_owned(),
            sink.into(),
        );
    }
    assert_eq!(server.take_xwayland_selection_data_requests().len(), 64);
}
