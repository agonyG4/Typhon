use super::*;
use std::num::NonZeroU64;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;

use crate::compositor::{SelectionSourceBackend, SelectionSourceKind};
use crate::xwayland::{
    XwaylandGeneration, XwaylandProxySelectionId, XwaylandSelectionEvent, XwaylandSelectionKind,
    XwaylandSelectionOffer, XwaylandSelectionOfferId,
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
fn wayland_clipboard_source_has_generation_qualified_proxy_snapshot() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    let source_key = crate::compositor::SelectionSourceKey(701);
    server.state.selection_state.register_source(
        source_key,
        SelectionSourceKind::WaylandClipboard,
        None,
    );
    server
        .state
        .selection_state
        .offer_source_mime_type_for_key(source_key, "image/png");
    let epoch = server.state.selection_state.allocate_mutation_epoch();
    let commit = server
        .state
        .selection_state
        .commit_selection(SelectionKind::Clipboard, source_key, epoch)
        .expect("canonical clipboard commit");

    let snapshot = server.xwayland_proxy_selection_snapshot(XwaylandSelectionKind::Clipboard);

    assert_eq!(snapshot.kind, XwaylandSelectionKind::Clipboard);
    assert_eq!(snapshot.selection_generation, commit.generation);
    let offer = snapshot
        .offer
        .expect("Wayland clipboard is exportable metadata");
    assert_eq!(
        offer.id,
        XwaylandProxySelectionId {
            kind: XwaylandSelectionKind::Clipboard,
            selection_generation: commit.generation,
            source_key,
        }
    );
    assert_eq!(offer.mime_types, ["image/png"]);
}

#[test]
fn xwayland_canonical_source_is_not_exported_back_to_x11() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    let external = offer(XwaylandSelectionKind::Clipboard, 6, 21);
    server.apply_xwayland_selection_event(XwaylandSelectionEvent::OfferChanged {
        kind: XwaylandSelectionKind::Clipboard,
        offer: external,
    });

    let snapshot = server.xwayland_proxy_selection_snapshot(XwaylandSelectionKind::Clipboard);

    assert_eq!(snapshot.selection_generation, 1);
    assert!(snapshot.offer.is_none());
}

#[test]
fn clipboard_and_primary_proxy_snapshots_are_independent() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    let clipboard = crate::compositor::SelectionSourceKey(711);
    let primary = crate::compositor::SelectionSourceKey(712);
    for (key, source_kind, mime_type) in [
        (
            clipboard,
            SelectionSourceKind::WaylandClipboard,
            "image/png",
        ),
        (primary, SelectionSourceKind::WaylandPrimary, "text/plain"),
    ] {
        server
            .state
            .selection_state
            .register_source(key, source_kind, None);
        server
            .state
            .selection_state
            .offer_source_mime_type_for_key(key, mime_type);
        let epoch = server.state.selection_state.allocate_mutation_epoch();
        let kind = if key == clipboard {
            SelectionKind::Clipboard
        } else {
            SelectionKind::Primary
        };
        server
            .state
            .selection_state
            .commit_selection(kind, key, epoch)
            .expect("canonical channel commit");
    }
    let before = server.xwayland_proxy_selection_snapshot(XwaylandSelectionKind::Primary);

    let replacement = crate::compositor::SelectionSourceKey(713);
    server.state.selection_state.register_source(
        replacement,
        SelectionSourceKind::DataControl,
        None,
    );
    server
        .state
        .selection_state
        .offer_source_mime_type_for_key(replacement, "image/webp");
    let epoch = server.state.selection_state.allocate_mutation_epoch();
    server
        .state
        .selection_state
        .commit_selection(SelectionKind::Clipboard, replacement, epoch)
        .expect("replace clipboard only");
    let after = server.xwayland_proxy_selection_snapshot(XwaylandSelectionKind::Primary);

    assert_eq!(before, after);
    assert_eq!(
        before.offer.as_ref().unwrap().id.kind,
        XwaylandSelectionKind::Primary
    );
    assert_eq!(before.offer.as_ref().unwrap().mime_types, ["text/plain"]);
}

#[test]
fn proxy_identity_changes_when_source_replaces_same_mime_list() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    let first = crate::compositor::SelectionSourceKey(721);
    let replacement = crate::compositor::SelectionSourceKey(722);
    for key in [first, replacement] {
        server.state.selection_state.register_source(
            key,
            SelectionSourceKind::WaylandClipboard,
            None,
        );
        server
            .state
            .selection_state
            .offer_source_mime_type_for_key(key, "text/plain");
    }
    let epoch = server.state.selection_state.allocate_mutation_epoch();
    server
        .state
        .selection_state
        .commit_selection(SelectionKind::Clipboard, first, epoch)
        .unwrap();
    let before = server
        .xwayland_proxy_selection_snapshot(XwaylandSelectionKind::Clipboard)
        .offer
        .unwrap();
    let epoch = server.state.selection_state.allocate_mutation_epoch();
    server
        .state
        .selection_state
        .commit_selection(SelectionKind::Clipboard, replacement, epoch)
        .unwrap();
    let after = server
        .xwayland_proxy_selection_snapshot(XwaylandSelectionKind::Clipboard)
        .offer
        .unwrap();

    assert_eq!(before.mime_types, after.mime_types);
    assert_ne!(before.id, after.id);
    assert_eq!(after.id.source_key, replacement);
}

#[test]
fn host_clipboard_backend_is_exportable_with_canonical_mime_spelling() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    server.state.install_host_clipboard_selection(
        crate::compositor::HostClipboardOfferId(45),
        vec!["Text/Html;profile=stable".to_owned()],
    );

    let snapshot = server.xwayland_proxy_selection_snapshot(XwaylandSelectionKind::Clipboard);

    let offer = snapshot
        .offer
        .expect("host clipboard backend can supply bytes");
    let active = server
        .state
        .selection_state
        .active_selection(SelectionKind::Clipboard)
        .unwrap();
    assert_eq!(offer.id.source_key, active.source_key);
    assert_eq!(offer.mime_types, ["Text/Html;profile=stable"]);
}

#[test]
fn clear_advances_channel_generation_and_removes_the_export_offer() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    let source_key = crate::compositor::SelectionSourceKey(731);
    server.state.selection_state.register_source(
        source_key,
        SelectionSourceKind::WaylandClipboard,
        None,
    );
    server
        .state
        .selection_state
        .offer_source_mime_type_for_key(source_key, "image/png");
    let epoch = server.state.selection_state.allocate_mutation_epoch();
    let committed = server
        .state
        .selection_state
        .commit_selection(SelectionKind::Clipboard, source_key, epoch)
        .unwrap();
    let clear_epoch = server.state.selection_state.allocate_mutation_epoch();
    server
        .state
        .selection_state
        .clear_selection(SelectionKind::Clipboard, clear_epoch)
        .unwrap();

    let snapshot = server.xwayland_proxy_selection_snapshot(XwaylandSelectionKind::Clipboard);

    assert!(snapshot.selection_generation > committed.generation);
    assert!(snapshot.offer.is_none());
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
