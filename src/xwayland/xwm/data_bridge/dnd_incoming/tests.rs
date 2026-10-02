use super::*;
use std::num::NonZeroU64;
use std::{
    io::{Read, Write},
    os::fd::AsRawFd,
    os::unix::net::UnixStream,
};

use x11rb::{protocol::xproto, x11_utils::Serialize};

fn offer_id(generation: XwaylandGeneration, serial: u64) -> XwaylandDndOfferId {
    XwaylandDndOfferId::new(generation, NonZeroU64::new(serial).expect("nonzero serial"))
}

fn read_peer(peer: &mut UnixStream) -> Vec<u8> {
    peer.set_nonblocking(true).unwrap();
    let mut output = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        match peer.read(&mut buffer) {
            Ok(0) => break,
            Ok(length) => output.extend_from_slice(&buffer[..length]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) => panic!("read fake X server requests: {error}"),
        }
    }
    output
}

fn count_requests(bytes: &[u8]) -> u16 {
    let mut offset = 0;
    let mut count = 0_u16;
    while offset + 4 <= bytes.len() {
        let length = usize::from(u16::from_ne_bytes([bytes[offset + 2], bytes[offset + 3]]));
        assert!(length != 0, "fake fixture uses ordinary X11 requests");
        offset += length * 4;
        assert!(offset <= bytes.len(), "complete X11 request packet");
        count = count.wrapping_add(1);
    }
    assert_eq!(offset, bytes.len(), "complete X11 request stream");
    count
}

fn take_requests(peer: &mut UnixStream, sequence: &mut u16) -> Vec<u8> {
    let requests = read_peer(peer);
    *sequence = sequence.wrapping_add(count_requests(&requests));
    requests
}

fn proxy_properties(bytes: &[u8]) -> Vec<(Window, Atom, Atom, u8, Vec<u8>)> {
    let mut properties = Vec::new();
    let mut offset = 0;
    while offset + 4 <= bytes.len() {
        let length = usize::from(u16::from_ne_bytes([bytes[offset + 2], bytes[offset + 3]]));
        let request_len = length * 4;
        assert!(request_len >= 4 && offset + request_len <= bytes.len());
        if bytes[offset] == xproto::CHANGE_PROPERTY_REQUEST {
            assert!(request_len >= 24);
            let window = u32::from_ne_bytes(bytes[offset + 4..offset + 8].try_into().unwrap());
            let property = u32::from_ne_bytes(bytes[offset + 8..offset + 12].try_into().unwrap());
            let type_atom = u32::from_ne_bytes(bytes[offset + 12..offset + 16].try_into().unwrap());
            let format = bytes[offset + 16];
            let items = u32::from_ne_bytes(bytes[offset + 20..offset + 24].try_into().unwrap());
            let data_bytes = usize::try_from(items).unwrap() * usize::from(format / 8);
            properties.push((
                window,
                property,
                type_atom,
                format,
                bytes[offset + 24..offset + 24 + data_bytes].to_vec(),
            ));
        }
        offset += request_len;
    }
    assert_eq!(offset, bytes.len());
    properties
}

fn xwm_has_readable_bytes(xwm: &Xwm) -> bool {
    let mut poll = libc::pollfd {
        fd: xwm.connection.stream().as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: poll receives one initialized pollfd whose descriptor is owned by the live XWM.
    unsafe { libc::poll(&mut poll, 1, 0) > 0 && poll.revents & libc::POLLIN != 0 }
}

fn client_message(
    xwm: &Xwm,
    target_proxy: Window,
    message_type: XwmAtomName,
    data: [u32; 5],
) -> xproto::ClientMessageEvent {
    xproto::ClientMessageEvent {
        response_type: xproto::CLIENT_MESSAGE_EVENT,
        format: 32,
        sequence: 0,
        window: target_proxy,
        type_: xwm.atoms.get(message_type),
        data: xproto::ClientMessageData::from(data),
    }
}

fn inject_client_message(xwm: &mut Xwm, peer: &mut UnixStream, event: xproto::ClientMessageEvent) {
    peer.write_all(&event.serialize()).unwrap();
    assert_eq!(xwm.drain_events(8).unwrap().events_processed, 1);
}

fn get_property_reply(sequence: u16, type_atom: Atom, format: u8, value: &[u8]) -> Vec<u8> {
    let value_len = match format {
        8 => value.len(),
        16 => value.len() / 2,
        32 => value.len() / 4,
        _ => panic!("unsupported fake property format"),
    } as u32;
    let mut reply = xproto::GetPropertyReply {
        format,
        sequence,
        length: value.len().div_ceil(4) as u32,
        type_: type_atom,
        bytes_after: 0,
        value_len,
        value: value.to_vec(),
    }
    .serialize();
    reply.resize(32 + value.len().div_ceil(4) * 4, 0);
    reply
}

fn selection_notify(
    requestor: Window,
    selection: Atom,
    target: Atom,
    property: Atom,
    timestamp: u32,
    sequence: u16,
) -> Vec<u8> {
    let mut event = xproto::SelectionNotifyEvent {
        response_type: xproto::SELECTION_NOTIFY_EVENT,
        sequence,
        time: timestamp,
        requestor,
        selection,
        target,
        property,
    }
    .serialize()
    .to_vec();
    event.resize(32, 0);
    event
}

fn property_new_value(window: Window, property: Atom, sequence: u16) -> Vec<u8> {
    let mut event = xproto::PropertyNotifyEvent {
        response_type: xproto::PROPERTY_NOTIFY_EVENT,
        sequence,
        window,
        atom: property,
        time: 2,
        state: xproto::Property::NEW_VALUE,
    }
    .serialize()
    .to_vec();
    event.resize(32, 0);
    event
}

fn fake_incoming_hover() -> (Xwm, UnixStream, XwaylandDndOfferId, Atom, u32, Window, u16) {
    let generation = XwaylandGeneration::new(NonZeroU64::new(81).unwrap());
    let (mut xwm, mut peer) = super::super::super::test_fixture_for_tests(generation);
    super::initialize_target_proxy(&mut xwm).unwrap();
    xwm.connection.flush().unwrap();
    let target_proxy = super::target_proxy(&xwm).unwrap();
    let mut server_sequence = 0;
    let proxy_setup = take_requests(&mut peer, &mut server_sequence);
    assert!(xwm.data_bridge.dnd.internal_windows.contains(&target_proxy));
    assert!(!proxy_setup.is_empty());
    let proxy_properties = proxy_properties(&proxy_setup);
    let self_proxy = proxy_properties
        .iter()
        .find(|(_, property, _, _, _)| *property == xwm.atoms.get(XwmAtomName::XdndProxy))
        .expect("target proxy publishes its exact self XdndProxy");
    assert_eq!(self_proxy.0, target_proxy);
    assert_eq!(
        u32::from_ne_bytes(self_proxy.4[..4].try_into().unwrap()),
        target_proxy
    );
    let aware = proxy_properties
        .iter()
        .find(|(_, property, _, _, _)| *property == xwm.atoms.get(XwmAtomName::XdndAware))
        .expect("target proxy publishes its negotiated wire version");
    assert_eq!(aware.0, target_proxy);
    assert_eq!(u32::from_ne_bytes(aware.4[..4].try_into().unwrap()), 5);
    assert!(
        proxy_properties
            .iter()
            .all(|(window, ..)| *window != xwm.root)
    );

    let source = 0x441;
    let mime_atom = 0x551;
    let timestamp = 0x1234_5678;
    let enter = client_message(
        &xwm,
        target_proxy,
        XwmAtomName::XdndEnter,
        [source, 5 << 24, mime_atom, 0, 0],
    );
    inject_client_message(&mut xwm, &mut peer, enter);
    xwm.connection.flush().unwrap();
    let _enter_requests = take_requests(&mut peer, &mut server_sequence);
    let offer_id = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .expect("Enter installs one provisional source-qualified session")
        .offer_id;
    assert!(
        !xwm.data_bridge
            .dnd
            .incoming_session()
            .unwrap()
            .metadata_complete
    );

    let packed = (12_i16 as u16 as u32) << 16 | (-7_i16 as u16 as u32);
    let position = client_message(
        &xwm,
        target_proxy,
        XwmAtomName::XdndPosition,
        [
            source,
            packed,
            timestamp,
            xwm.atoms.get(XwmAtomName::XdndActionCopy),
            0,
        ],
    );
    inject_client_message(&mut xwm, &mut peer, position);
    xwm.connection.flush().unwrap();
    let _position_requests = take_requests(&mut peer, &mut server_sequence);
    let session = xwm.data_bridge.dnd.incoming_session().unwrap();
    assert_eq!(session.latest_position.unwrap().timestamp, timestamp);
    assert_eq!(session.latest_position.unwrap().root_x, 12.0);
    assert_eq!(session.latest_position.unwrap().root_y, -7.0);

    let sequence = *xwm
        .data_bridge
        .dnd_incoming
        .pending
        .keys()
        .next()
        .expect("one asynchronous GetAtomName request");
    let mut atom_name = xproto::GetAtomNameReply {
        sequence: sequence as u16,
        length: "text/plain".len().div_ceil(4) as u32,
        name: b"text/plain".to_vec(),
    }
    .serialize();
    atom_name.resize(32 + "text/plain".len().div_ceil(4) * 4, 0);
    peer.write_all(&atom_name).unwrap();
    xwm.drain_events(32).unwrap();
    xwm.connection.flush().unwrap();
    let _metadata_requests = take_requests(&mut peer, &mut server_sequence);
    let events = xwm.data_bridge.dnd_incoming.take_events();
    let [XwaylandDndIncomingEvent::Begin { offer, .. }] = events.as_slice() else {
        panic!("completed MIME metadata and Position activate exactly one offer");
    };
    assert_eq!(offer.id(), offer_id);
    assert_eq!(offer.mime_types().as_slice(), &["text/plain"]);
    assert_eq!(
        offer.source_actions(),
        &[crate::xwayland::XwaylandDndAction::Copy]
    );
    (
        xwm,
        peer,
        offer_id,
        mime_atom,
        timestamp,
        target_proxy,
        server_sequence,
    )
}

fn start_fake_selection_transfer(
    xwm: &mut Xwm,
    peer: &mut UnixStream,
    offer_id: XwaylandDndOfferId,
    mime_atom: Atom,
    timestamp: u32,
    server_sequence: &mut u16,
) -> (
    UnixStream,
    crate::xwayland::XwaylandDndIncomingTransferId,
    Window,
    Atom,
) {
    let (reader, writer) = UnixStream::pair().unwrap();
    reader
        .set_read_timeout(Some(std::time::Duration::from_secs(1)))
        .unwrap();
    let transfer_id = super::start_data_request(
        xwm,
        crate::xwayland::XwaylandDndDataRequest {
            offer_id,
            mime_type: "text/plain".to_owned(),
            sink: writer.into(),
        },
        20,
    )
    .unwrap()
    .expect("exact live offer starts one incoming transfer");
    xwm.connection.flush().unwrap();
    let _requests = take_requests(peer, server_sequence);
    let transfer = &xwm.data_bridge.dnd_incoming.transfers[&transfer_id];
    assert_eq!(transfer.target, mime_atom);
    assert_eq!(transfer.selection_timestamp, timestamp);
    assert_ne!(transfer.selection_timestamp, x11rb::CURRENT_TIME);
    let requestor = transfer.requestor;
    let property = transfer.property;
    peer.write_all(&selection_notify(
        requestor,
        xwm.atoms.get(XwmAtomName::XdndSelection),
        mime_atom,
        property,
        timestamp,
        *server_sequence,
    ))
    .unwrap();
    assert!(xwm.drain_events(32).unwrap().events_processed >= 1);
    assert_eq!(
        xwm.data_bridge.dnd_incoming.transfers[&transfer_id].phase,
        IncomingTransferPhase::ReadingProperty,
        "exact XdndSelection SelectionNotify should start a property read"
    );
    xwm.connection.flush().unwrap();
    let property_read = read_peer(peer);
    assert!(
        property_read
            .chunks_exact(4)
            .any(|chunk| chunk[0] == xproto::GET_PROPERTY_REQUEST),
        "the fake server observes the pending XGetProperty request: {property_read:?}"
    );
    (reader, transfer_id, requestor, property)
}

#[test]
fn fake_x_hover_status_direct_payload_and_leave_follow_the_incoming_bridge() {
    let (mut xwm, mut peer, offer_id, mime_atom, timestamp, target_proxy, mut server_sequence) =
        fake_incoming_hover();
    super::source_feedback(
        &mut xwm,
        offer_id,
        Some("text/plain".to_owned()),
        Some(crate::xwayland::XwaylandDndAction::Copy),
    )
    .unwrap();
    xwm.connection.flush().unwrap();
    let status_requests = read_peer(&mut peer);
    let status_event = status_requests
        .windows(32)
        .find(|chunk| chunk[0] & 0x7f == xproto::CLIENT_MESSAGE_EVENT)
        .expect("canonical SourceFeedback sends XdndStatus");
    assert_eq!(
        u32::from_ne_bytes(status_event[4..8].try_into().unwrap()),
        0x441
    );
    assert_eq!(
        u32::from_ne_bytes(status_event[12..16].try_into().unwrap()),
        xwm.root,
        "XdndStatus names the logical root, never the internal proxy"
    );
    assert_eq!(
        u32::from_ne_bytes(status_event[8..12].try_into().unwrap()),
        xwm.atoms.get(XwmAtomName::XdndStatus)
    );

    let (mut reader, transfer_id, requestor, property) = start_fake_selection_transfer(
        &mut xwm,
        &mut peer,
        offer_id,
        mime_atom,
        timestamp,
        &mut server_sequence,
    );
    let sequence = xwm.data_bridge.dnd_incoming.transfers[&transfer_id]
        .pending_reply
        .unwrap() as u16;
    peer.write_all(&get_property_reply(
        sequence,
        mime_atom,
        8,
        b"direct payload",
    ))
    .unwrap();
    assert!(
        xwm_has_readable_bytes(&xwm),
        "fake GetProperty reply reaches the XWM socket"
    );
    xwm.drain_events(32).unwrap();
    let mut received = [0; 14];
    reader.read_exact(&mut received).unwrap();
    assert_eq!(&received, b"direct payload");
    assert!(
        !xwm.data_bridge
            .dnd_incoming
            .transfers
            .contains_key(&transfer_id)
    );
    let _ = (requestor, property);

    let leave = client_message(
        &xwm,
        target_proxy,
        XwmAtomName::XdndLeave,
        [0x441, 0, 0, 0, 0],
    );
    inject_client_message(&mut xwm, &mut peer, leave);
    assert!(xwm.data_bridge.dnd.incoming_session().is_none());
    assert!(matches!(
        xwm.data_bridge.dnd_incoming.take_events().as_slice(),
        [XwaylandDndIncomingEvent::Leave { offer_id: left }] if *left == offer_id
    ));
    let terminal = read_peer(&mut peer);
    assert!(!terminal.windows(12).any(|chunk| {
        u32::from_ne_bytes(chunk[8..12].try_into().unwrap())
            == xwm.atoms.get(XwmAtomName::XdndFinished)
    }));
}

#[test]
fn fake_x_hover_incr_payload_reads_next_chunk_only_after_sink_delivery() {
    let (mut xwm, mut peer, offer_id, mime_atom, timestamp, _target_proxy, mut server_sequence) =
        fake_incoming_hover();
    let (mut reader, transfer_id, requestor, property) = start_fake_selection_transfer(
        &mut xwm,
        &mut peer,
        offer_id,
        mime_atom,
        timestamp,
        &mut server_sequence,
    );
    let incr = xwm.atoms.get(XwmAtomName::Incr);
    let marker_sequence = xwm.data_bridge.dnd_incoming.transfers[&transfer_id]
        .pending_reply
        .unwrap() as u16;
    peer.write_all(&get_property_reply(
        marker_sequence,
        incr,
        32,
        &64_u32.to_ne_bytes(),
    ))
    .unwrap();
    assert!(
        xwm_has_readable_bytes(&xwm),
        "fake INCR marker reaches the XWM socket"
    );
    let drain = xwm.drain_events(32).unwrap();
    assert_eq!(
        xwm.data_bridge.dnd_incoming.transfers[&transfer_id].phase,
        IncomingTransferPhase::WaitingForIncrValue,
        "drain={drain:?}, transfer={:?}",
        xwm.data_bridge.dnd_incoming.transfers[&transfer_id]
    );
    let _delete_marker = take_requests(&mut peer, &mut server_sequence);

    for chunk in [b"incr-".as_slice(), b"payload".as_slice()] {
        peer.write_all(&property_new_value(requestor, property, server_sequence))
            .unwrap();
        xwm.drain_events(32).unwrap();
        xwm.connection.flush().unwrap();
        let _property_read = take_requests(&mut peer, &mut server_sequence);
        let sequence = xwm.data_bridge.dnd_incoming.transfers[&transfer_id]
            .pending_reply
            .unwrap() as u16;
        peer.write_all(&get_property_reply(sequence, mime_atom, 8, chunk))
            .unwrap();
        xwm.drain_events(32).unwrap();
        let mut received = vec![0; chunk.len()];
        reader.read_exact(&mut received).unwrap();
        assert_eq!(&received, chunk);
        if xwm
            .data_bridge
            .dnd_incoming
            .transfers
            .contains_key(&transfer_id)
        {
            assert_eq!(
                xwm.data_bridge.dnd_incoming.transfers[&transfer_id].phase,
                IncomingTransferPhase::WaitingForIncrValue
            );
            let _delete_chunk = take_requests(&mut peer, &mut server_sequence);
        }
    }

    peer.write_all(&property_new_value(requestor, property, server_sequence))
        .unwrap();
    xwm.drain_events(32).unwrap();
    xwm.connection.flush().unwrap();
    let _property_read = take_requests(&mut peer, &mut server_sequence);
    let sequence = xwm.data_bridge.dnd_incoming.transfers[&transfer_id]
        .pending_reply
        .unwrap() as u16;
    peer.write_all(&get_property_reply(sequence, mime_atom, 8, &[]))
        .unwrap();
    xwm.drain_events(32).unwrap();
    assert!(
        !xwm.data_bridge
            .dnd_incoming
            .transfers
            .contains_key(&transfer_id)
    );
}

#[test]
fn root_proxy_acquisition_preserves_a_valid_foreign_owner() {
    assert!(!root_proxy_may_be_replaced(Some(0x234), true, 0x345));
    assert!(root_proxy_may_be_replaced(None, false, 0x345));
    assert!(root_proxy_may_be_replaced(Some(0x345), true, 0x345));
    assert!(root_proxy_may_be_replaced(Some(0x234), false, 0x345));
}

#[test]
fn root_proxy_release_preserves_a_replacement_owner() {
    assert!(root_proxy_should_be_released(Some(0x345), 0x345));
    assert!(!root_proxy_should_be_released(Some(0x456), 0x345));
    assert!(!root_proxy_should_be_released(None, 0x345));
}

#[test]
fn incoming_positions_coalesce_only_for_the_exact_offer() {
    let generation = XwaylandGeneration::new(NonZeroU64::new(9).unwrap());
    let first = offer_id(generation, 1);
    let second = offer_id(generation, 2);
    let mut manager = DndIncomingManager::default();
    let event = |offer_id, x| XwaylandDndIncomingEvent::Position {
        offer_id,
        x,
        y: 4.0,
        requested_action: crate::xwayland::XwaylandDndAction::Copy,
        source_actions: vec![crate::xwayland::XwaylandDndAction::Copy],
        x_timestamp: x as u32,
    };
    assert!(manager.push_event(event(first, 1.0)));
    assert!(manager.push_event(event(first, 2.0)));
    assert!(manager.push_event(event(second, 3.0)));
    assert_eq!(manager.events.len(), 2);
    assert!(matches!(
        manager.events.front(),
        Some(XwaylandDndIncomingEvent::Position { x: 2.0, .. })
    ));
}

#[test]
fn incoming_mailbox_reserves_terminal_leave_capacity_for_each_begin() {
    let generation = XwaylandGeneration::new(NonZeroU64::new(10).unwrap());
    let first_id = offer_id(generation, 1);
    let source = X11WindowHandle::new(generation, 0x401);
    let offer = crate::xwayland::XwaylandDndOffer::new(
        first_id,
        source,
        crate::xwayland::XwaylandDndVersion::new(5).unwrap(),
        crate::xwayland::XwaylandDndMimeCatalog::try_new(vec!["text/plain".to_owned()]).unwrap(),
        vec![crate::xwayland::XwaylandDndAction::Copy],
    )
    .unwrap();
    let mut manager = DndIncomingManager::default();
    assert!(manager.push_event(XwaylandDndIncomingEvent::Begin {
        offer,
        x: 0.0,
        y: 0.0,
        requested_action: crate::xwayland::XwaylandDndAction::Copy,
        x_timestamp: 1,
    }));
    for serial in 2..MAX_PENDING_XWAYLAND_DND_INCOMING_EVENTS as u64 {
        let id = offer_id(generation, serial);
        assert!(manager.push_event(XwaylandDndIncomingEvent::Position {
            offer_id: id,
            x: serial as f64,
            y: 0.0,
            requested_action: crate::xwayland::XwaylandDndAction::Copy,
            source_actions: vec![crate::xwayland::XwaylandDndAction::Copy],
            x_timestamp: serial as u32,
        }));
    }
    assert_eq!(
        manager.events.len(),
        MAX_PENDING_XWAYLAND_DND_INCOMING_EVENTS - 1
    );
    assert!(manager.push_event(XwaylandDndIncomingEvent::Leave { offer_id: first_id }));
    assert_eq!(
        manager.events.len(),
        MAX_PENDING_XWAYLAND_DND_INCOMING_EVENTS
    );
}

#[test]
fn source_action_mapping_never_exposes_link_or_private() {
    use crate::xwayland::XwaylandDndAction as Action;
    assert_eq!(
        representable_source_actions(Action::Copy, &[]),
        vec![Action::Copy]
    );
    assert_eq!(
        representable_source_actions(Action::Move, &[]),
        vec![Action::Move, Action::Copy]
    );
    assert_eq!(
        representable_source_actions(Action::Link, &[Action::Link]),
        vec![Action::Copy]
    );
    assert_eq!(
        representable_source_actions(Action::Private, &[Action::Private]),
        vec![Action::Copy]
    );
    assert!(representable_source_actions(Action::Ask, &[Action::Ask]).is_empty());
    assert_eq!(
        representable_source_actions(Action::Ask, &[Action::Ask, Action::Move]),
        vec![Action::Ask, Action::Move]
    );
}

#[test]
fn direct_and_incr_chunks_preserve_read_and_backpressure_order() {
    assert_eq!(
        next_after_property_chunk(IncomingPropertyMode::Direct, 4, false),
        ContinueAfterWrite::ReadMoreProperty
    );
    assert_eq!(
        next_after_property_chunk(IncomingPropertyMode::Direct, 0, false),
        ContinueAfterWrite::Finish
    );
    assert_eq!(
        next_after_property_chunk(IncomingPropertyMode::Incr, 4, false),
        ContinueAfterWrite::ReadMoreProperty
    );
    assert_eq!(
        next_after_property_chunk(IncomingPropertyMode::Incr, 0, false),
        ContinueAfterWrite::ReadNextIncrChunk
    );
    assert_eq!(
        next_after_property_chunk(IncomingPropertyMode::Incr, 0, true),
        ContinueAfterWrite::Finish
    );
}

#[test]
fn synthetic_direct_selection_payload_reaches_the_wayland_sink_once() {
    let (mut reader, writer) = UnixStream::pair().unwrap();
    let chunks: [&[u8]; 2] = [b"text/", b"payload"];
    for chunk in chunks {
        assert_eq!(
            write_sink_bytes(writer.as_raw_fd(), chunk).unwrap(),
            chunk.len()
        );
    }
    let mut received = [0; 12];
    reader.read_exact(&mut received).unwrap();
    assert_eq!(&received, b"text/payload");
    assert_eq!(
        next_after_property_chunk(IncomingPropertyMode::Direct, 0, false),
        ContinueAfterWrite::Finish
    );
}

#[test]
fn synthetic_incr_selection_payload_waits_for_each_sink_chunk() {
    let (mut reader, writer) = UnixStream::pair().unwrap();
    let chunks: [&[u8]; 2] = [b"incr-", b"payload"];
    let mut received = Vec::new();
    let mut expected_len = 0;
    for chunk in chunks {
        assert_eq!(
            write_sink_bytes(writer.as_raw_fd(), chunk).unwrap(),
            chunk.len()
        );
        expected_len += chunk.len();
        assert_eq!(
            next_after_property_chunk(IncomingPropertyMode::Incr, 0, false),
            ContinueAfterWrite::ReadNextIncrChunk
        );
        let mut buffer = vec![0; chunk.len()];
        reader.read_exact(&mut buffer).unwrap();
        received.extend(buffer);
        assert_eq!(received.len(), expected_len);
    }
    assert_eq!(received, b"incr-payload");
    assert_eq!(
        next_after_property_chunk(IncomingPropertyMode::Incr, 0, true),
        ContinueAfterWrite::Finish
    );
}
