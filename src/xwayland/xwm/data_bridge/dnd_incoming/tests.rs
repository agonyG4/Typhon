use super::*;
use std::num::NonZeroU64;
use std::{
    io::{Read, Write},
    os::fd::AsRawFd,
    os::unix::net::UnixStream,
};

use x11rb::{protocol::xproto, x11_utils::Serialize};

mod action_list;
mod position_authority;
mod position_wire;
mod terminal;

fn offer_id(generation: XwaylandGeneration, serial: u64) -> XwaylandDndOfferId {
    XwaylandDndOfferId::new(generation, NonZeroU64::new(serial).expect("nonzero serial"))
}

pub(crate) fn read_peer(peer: &mut UnixStream) -> Vec<u8> {
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

fn selection_conversion_timestamps(bytes: &[u8], selection: Atom) -> Vec<u32> {
    let mut timestamps = Vec::new();
    let mut offset = 0;
    while offset + 4 <= bytes.len() {
        let length_words = usize::from(u16::from_ne_bytes([bytes[offset + 2], bytes[offset + 3]]));
        let request_len = length_words * 4;
        assert!(request_len >= 4 && offset + request_len <= bytes.len());
        if bytes[offset] == xproto::CONVERT_SELECTION_REQUEST {
            assert_eq!(request_len, 24);
            assert_eq!(
                u32::from_ne_bytes(bytes[offset + 8..offset + 12].try_into().unwrap()),
                selection
            );
            timestamps.push(u32::from_ne_bytes(
                bytes[offset + 20..offset + 24].try_into().unwrap(),
            ));
        }
        offset += request_len;
    }
    assert_eq!(offset, bytes.len());
    timestamps
}

pub(crate) fn take_requests(peer: &mut UnixStream, sequence: &mut u16) -> Vec<u8> {
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
    logical_target: Window,
    message_type: XwmAtomName,
    data: [u32; 5],
) -> xproto::ClientMessageEvent {
    xproto::ClientMessageEvent {
        response_type: xproto::CLIENT_MESSAGE_EVENT,
        format: 32,
        sequence: 0,
        window: logical_target,
        type_: xwm.atoms.get(message_type),
        data: xproto::ClientMessageData::from(data),
    }
}

fn inject_client_message(xwm: &mut Xwm, peer: &mut UnixStream, event: xproto::ClientMessageEvent) {
    peer.write_all(&event.serialize()).unwrap();
    assert_eq!(xwm.drain_events(8).unwrap().events_processed, 1);
}

pub(crate) fn status_messages(bytes: &[u8], xwm: &Xwm) -> Vec<(Window, [u32; 5])> {
    bytes
        .windows(32)
        .filter(|event| {
            event[0] & 0x7f == xproto::CLIENT_MESSAGE_EVENT
                && u32::from_ne_bytes(event[8..12].try_into().unwrap())
                    == xwm.atoms.get(XwmAtomName::XdndStatus)
        })
        .map(|event| {
            let data = std::array::from_fn(|index| {
                let start = 12 + index * 4;
                u32::from_ne_bytes(event[start..start + 4].try_into().unwrap())
            });
            (u32::from_ne_bytes(event[4..8].try_into().unwrap()), data)
        })
        .collect()
}

fn inject_position(
    xwm: &mut Xwm,
    peer: &mut UnixStream,
    source: Window,
    timestamp: u32,
    action: Atom,
    packed_coordinates: u32,
    server_sequence: u16,
) {
    let root = xwm.root;
    let mut position = client_message(
        xwm,
        root,
        XwmAtomName::XdndPosition,
        [source, 0x05a3, packed_coordinates, timestamp, action],
    );
    position.sequence = server_sequence;
    inject_client_message(xwm, peer, position);
}

fn inject_copy_position(
    xwm: &mut Xwm,
    peer: &mut UnixStream,
    timestamp: u32,
    x: i16,
    server_sequence: u16,
) {
    let copy = xwm.atoms.get(XwmAtomName::XdndActionCopy);
    let coordinates = (u32::from(x as u16) << 16) | u32::from((-7_i16) as u16);
    inject_position(
        xwm,
        peer,
        0x441,
        timestamp,
        copy,
        coordinates,
        server_sequence,
    );
}

pub(crate) fn get_property_reply(
    sequence: u16,
    type_atom: Atom,
    format: u8,
    value: &[u8],
) -> Vec<u8> {
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

pub(crate) fn fake_incoming_hover() -> (Xwm, UnixStream, XwaylandDndOfferId, Atom, u32, Window, u16)
{
    let generation = XwaylandGeneration::new(NonZeroU64::new(81).unwrap());
    let (mut xwm, mut peer) = super::super::super::test_fixture_for_tests(generation);
    super::initialize_target_proxy(&mut xwm).unwrap();
    xwm.connection.flush().unwrap();
    let target_proxy = super::target_proxy(&xwm).unwrap();
    xwm.data_bridge.dnd_incoming.root_proxy_authority = RootProxyAuthority::Owned {
        generation: xwm.generation,
        proxy: target_proxy,
    };
    let mut server_sequence = 0;
    let proxy_setup = take_requests(&mut peer, &mut server_sequence);
    assert!(xwm.data_bridge.dnd.internal_windows.contains(&target_proxy));
    assert!(
        !xwm.windows
            .contains(X11WindowHandle::new(xwm.generation, target_proxy))
    );
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
        xwm.root,
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
        xwm.root,
        XwmAtomName::XdndPosition,
        [
            source,
            0x05a3,
            packed,
            timestamp,
            xwm.atoms.get(XwmAtomName::XdndActionCopy),
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

#[test]
fn accepted_v5_drop_submits_one_exact_canonical_drop_event() {
    let (mut xwm, _peer, offer_id, _, _, target_proxy, _) = fake_incoming_hover();
    xwm.data_bridge.dnd_incoming.root_proxy_authority = RootProxyAuthority::Owned {
        generation: xwm.generation,
        proxy: target_proxy,
    };
    let position_id = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .latest_position
        .unwrap()
        .position_id;
    super::source_feedback(
        &mut xwm,
        offer_id,
        position_id,
        Some("text/plain".to_owned()),
        Some(crate::xwayland::XwaylandDndAction::Copy),
    )
    .unwrap();
    xwm.data_bridge.dnd_incoming.take_events();

    let source = xwm.data_bridge.dnd.incoming_session().unwrap().source.xid();
    let drop_timestamp = 0x7654_3210;
    let first_drop = client_message(
        &xwm,
        xwm.root,
        XwmAtomName::XdndDrop,
        [source, 0xaaaa, drop_timestamp, 0xbbbb, 0xcccc],
    );
    assert!(super::client_message(&mut xwm, first_drop, 30_000_000_000).unwrap());
    assert!(matches!(
        xwm.data_bridge.dnd_incoming.take_events().as_slice(),
        [XwaylandDndIncomingEvent::Drop { offer_id: queued }] if *queued == offer_id
    ));
    assert!(matches!(
        xwm.data_bridge.dnd.incoming_session().unwrap().wire_phase,
        IncomingDndWirePhase::DropSubmitted { drop_timestamp: timestamp, action, .. }
            if timestamp == drop_timestamp && action == crate::xwayland::XwaylandDndAction::Copy
    ));
    let duplicate_drop = client_message(
        &xwm,
        xwm.root,
        XwmAtomName::XdndDrop,
        [source, 0, drop_timestamp, 0, 0],
    );
    assert!(super::client_message(&mut xwm, duplicate_drop, 30_000_000_001).unwrap());
    assert!(xwm.data_bridge.dnd_incoming.take_events().is_empty());
}

pub(crate) fn action_atom_for_test(xwm: &Xwm, action: crate::xwayland::XwaylandDndAction) -> Atom {
    match action {
        crate::xwayland::XwaylandDndAction::Copy => xwm.atoms.get(XwmAtomName::XdndActionCopy),
        crate::xwayland::XwaylandDndAction::Move => xwm.atoms.get(XwmAtomName::XdndActionMove),
        crate::xwayland::XwaylandDndAction::Link => xwm.atoms.get(XwmAtomName::XdndActionLink),
        crate::xwayland::XwaylandDndAction::Ask => xwm.atoms.get(XwmAtomName::XdndActionAsk),
        crate::xwayland::XwaylandDndAction::Private => {
            xwm.atoms.get(XwmAtomName::XdndActionPrivate)
        }
    }
}

pub(crate) fn position_at_for_test(
    xwm: &mut Xwm,
    timestamp: u32,
    action: crate::xwayland::XwaylandDndAction,
    x: i16,
    y: i16,
    now_ns: u64,
) -> Result<(), XwmError> {
    let source = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .expect("test Position has an incoming offer")
        .source
        .xid();
    let coordinates = (u32::from(x as u16) << 16) | u32::from(y as u16);
    let action = action_atom_for_test(xwm, action);
    super::metadata::position(
        xwm,
        [source, 0x05a3, coordinates, timestamp, action],
        now_ns,
    )
}

pub(crate) fn begin_fake_selection_transfer(
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
    begin_fake_selection_transfer_at(
        xwm,
        peer,
        offer_id,
        mime_atom,
        timestamp,
        server_sequence,
        20,
    )
}

fn begin_fake_selection_transfer_at(
    xwm: &mut Xwm,
    peer: &mut UnixStream,
    offer_id: XwaylandDndOfferId,
    mime_atom: Atom,
    timestamp: u32,
    server_sequence: &mut u16,
    started_at_ns: u64,
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
        started_at_ns,
    )
    .unwrap()
    .expect("exact live offer starts one incoming transfer");
    xwm.connection.flush().unwrap();
    let requests = take_requests(peer, server_sequence);
    assert_eq!(
        selection_conversion_timestamps(&requests, xwm.atoms.get(XwmAtomName::XdndSelection),),
        vec![timestamp],
        "XConvertSelection carries the timestamp authoritative for the wire phase"
    );
    let transfer = &xwm.data_bridge.dnd_incoming.transfers[&transfer_id];
    assert_eq!(transfer.target, mime_atom);
    assert_eq!(transfer.selection_timestamp, timestamp);
    assert_ne!(transfer.selection_timestamp, x11rb::CURRENT_TIME);
    let requestor = transfer.requestor;
    let property = transfer.property;
    (reader, transfer_id, requestor, property)
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
    let (reader, transfer_id, requestor, property) =
        begin_fake_selection_transfer(xwm, peer, offer_id, mime_atom, timestamp, server_sequence);
    notify_fake_selection_transfer(
        xwm,
        peer,
        transfer_id,
        mime_atom,
        timestamp,
        requestor,
        property,
        *server_sequence,
        server_sequence,
    );
    (reader, transfer_id, requestor, property)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn notify_fake_selection_transfer(
    xwm: &mut Xwm,
    peer: &mut UnixStream,
    transfer_id: crate::xwayland::XwaylandDndIncomingTransferId,
    mime_atom: Atom,
    timestamp: u32,
    requestor: Window,
    property: Atom,
    event_sequence: u16,
    server_sequence: &mut u16,
) {
    peer.write_all(&selection_notify(
        requestor,
        xwm.atoms.get(XwmAtomName::XdndSelection),
        mime_atom,
        property,
        timestamp,
        event_sequence,
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
    *server_sequence = server_sequence.wrapping_add(count_requests(&property_read));
}

pub(crate) fn transfer_selection_timestamp(
    xwm: &Xwm,
    transfer_id: crate::xwayland::XwaylandDndIncomingTransferId,
) -> Option<u32> {
    xwm.data_bridge
        .dnd_incoming
        .transfers
        .get(&transfer_id)
        .map(|transfer| transfer.selection_timestamp)
}

pub(crate) fn transfer_pending_sequence(
    xwm: &Xwm,
    transfer_id: crate::xwayland::XwaylandDndIncomingTransferId,
) -> Option<u16> {
    xwm.data_bridge
        .dnd_incoming
        .transfers
        .get(&transfer_id)
        .and_then(|transfer| transfer.pending_reply)
        .map(|sequence| sequence as u16)
}

pub(crate) fn transfer_is_active(
    xwm: &Xwm,
    transfer_id: crate::xwayland::XwaylandDndIncomingTransferId,
) -> bool {
    xwm.data_bridge
        .dnd_incoming
        .transfers
        .contains_key(&transfer_id)
}

#[test]
fn fake_x_hover_status_direct_payload_and_leave_follow_the_incoming_bridge() {
    let (mut xwm, mut peer, offer_id, mime_atom, timestamp, _target_proxy, mut server_sequence) =
        fake_incoming_hover();
    let position_id = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .latest_position
        .unwrap()
        .position_id;
    super::source_feedback(
        &mut xwm,
        offer_id,
        position_id,
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

    let leave = client_message(&xwm, xwm.root, XwmAtomName::XdndLeave, [0x441, 0, 0, 0, 0]);
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
fn inbound_proxy_uses_root_as_logical_target_and_consumes_proxy_window_traffic() {
    let (mut xwm, mut peer, offer_id, _, timestamp, target_proxy, _) = fake_incoming_hover();
    let root = xwm.root;
    let first_position = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .latest_position
        .unwrap();
    assert_ne!(root, target_proxy);
    assert_eq!(
        xwm.data_bridge
            .dnd
            .incoming_session()
            .unwrap()
            .logical_target_root,
        root
    );
    assert_eq!(
        xwm.data_bridge.dnd.incoming_session().unwrap().target_proxy,
        target_proxy
    );

    inject_copy_position(&mut xwm, &mut peer, timestamp + 1, 13, 0);
    let second_position = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .latest_position
        .unwrap();
    assert_eq!(second_position.position_id.offer_id(), offer_id);
    assert_ne!(second_position.position_id, first_position.position_id);
    assert_eq!(second_position.timestamp, timestamp + 1);
    assert!(matches!(
        xwm.data_bridge.dnd_incoming.take_events().as_slice(),
        [XwaylandDndIncomingEvent::Position { position_id, .. }]
            if *position_id == second_position.position_id
    ));

    let malformed_proxy_message = client_message(
        &xwm,
        target_proxy,
        XwmAtomName::XdndPosition,
        [
            0x441,
            0x05a3,
            0,
            timestamp + 2,
            xwm.atoms.get(XwmAtomName::XdndActionCopy),
        ],
    );
    assert!(!super::is_logical_root_target(
        &xwm,
        &malformed_proxy_message
    ));
    assert!(super::super::dnd::client_message(&mut xwm, malformed_proxy_message, 50).unwrap());
    let unchanged = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .latest_position
        .unwrap();
    assert_eq!(unchanged.position_id, second_position.position_id);
    assert!(xwm.data_bridge.dnd_incoming.take_events().is_empty());
    xwm.connection.flush().unwrap();
    assert!(status_messages(&read_peer(&mut peer), &xwm).is_empty());
}

#[test]
fn status_action_compatibility_is_typed_and_rejects_illegal_fallbacks() {
    use crate::xwayland::XwaylandDndAction as Action;

    let cases = [
        (Action::Copy, Action::Copy, vec![Action::Copy], true),
        (
            Action::Copy,
            Action::Move,
            vec![Action::Copy, Action::Move],
            false,
        ),
        (Action::Move, Action::Move, vec![Action::Move], true),
        (
            Action::Move,
            Action::Copy,
            vec![Action::Move, Action::Copy],
            true,
        ),
        (
            Action::Move,
            Action::Ask,
            vec![Action::Move, Action::Ask, Action::Copy],
            false,
        ),
        (
            Action::Ask,
            Action::Ask,
            vec![Action::Ask, Action::Copy],
            true,
        ),
        (
            Action::Ask,
            Action::Copy,
            vec![Action::Ask, Action::Copy],
            true,
        ),
        (
            Action::Ask,
            Action::Move,
            vec![Action::Ask, Action::Move, Action::Copy],
            false,
        ),
        (Action::Link, Action::Copy, vec![Action::Copy], true),
        (Action::Private, Action::Copy, vec![Action::Copy], true),
    ];

    for (requested, selected, source_actions, accepted) in cases {
        let (mut xwm, mut peer, offer_id, _, _, _, _) = fake_incoming_hover();
        let position_id = {
            let session = xwm.data_bridge.dnd.incoming_session_mut().unwrap();
            let mut position = session.latest_position.unwrap();
            position.requested_action = requested;
            session.latest_position = Some(position);
            session.source_actions = source_actions;
            position.position_id
        };
        super::source_feedback(
            &mut xwm,
            offer_id,
            position_id,
            Some("text/plain".to_owned()),
            Some(selected),
        )
        .unwrap();
        xwm.connection.flush().unwrap();
        let statuses = status_messages(&read_peer(&mut peer), &xwm);
        assert_eq!(statuses.len(), 1, "{requested:?} with {selected:?}");
        let (recipient, data) = statuses[0];
        assert_eq!(recipient, 0x441);
        assert_eq!(data[0], xwm.root);
        assert_eq!(
            data[1] & 1 != 0,
            accepted,
            "{requested:?} with {selected:?}"
        );
        let expected_atom = if accepted {
            match selected {
                Action::Copy => xwm.atoms.get(XwmAtomName::XdndActionCopy),
                Action::Move => xwm.atoms.get(XwmAtomName::XdndActionMove),
                Action::Ask => xwm.atoms.get(XwmAtomName::XdndActionAsk),
                Action::Link | Action::Private => unreachable!("unrepresentable XdndStatus action"),
            }
        } else {
            0
        };
        assert_eq!(data[4], expected_atom, "{requested:?} with {selected:?}");
    }
}

#[test]
fn direct_selection_conversion_survives_a_later_position() {
    let (mut xwm, mut peer, offer_id, mime_atom, timestamp, _, mut server_sequence) =
        fake_incoming_hover();
    let p1 = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .latest_position
        .unwrap();
    let (mut reader, transfer_id, requestor, property) = begin_fake_selection_transfer(
        &mut xwm,
        &mut peer,
        offer_id,
        mime_atom,
        timestamp,
        &mut server_sequence,
    );

    inject_copy_position(&mut xwm, &mut peer, timestamp + 1, 22, server_sequence);
    let p2 = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .latest_position
        .unwrap();
    assert_ne!(p1.position_id, p2.position_id);
    assert_eq!(p2.timestamp, timestamp + 1);
    assert_eq!(
        xwm.data_bridge.dnd_incoming.transfers[&transfer_id].selection_timestamp,
        timestamp
    );

    notify_fake_selection_transfer(
        &mut xwm,
        &mut peer,
        transfer_id,
        mime_atom,
        timestamp,
        requestor,
        property,
        server_sequence,
        &mut server_sequence,
    );
    let sequence = xwm.data_bridge.dnd_incoming.transfers[&transfer_id]
        .pending_reply
        .unwrap() as u16;
    peer.write_all(&get_property_reply(sequence, mime_atom, 8, b"P1 direct"))
        .unwrap();
    xwm.drain_events(32).unwrap();
    let mut received = [0; 9];
    reader.read_exact(&mut received).unwrap();
    assert_eq!(&received, b"P1 direct");
    assert!(
        !xwm.data_bridge
            .dnd_incoming
            .transfers
            .contains_key(&transfer_id)
    );
}

#[test]
fn replacement_enter_retires_transfer_before_late_selection_notify() {
    let (mut xwm, mut peer, offer_id, mime_atom, timestamp, _, mut server_sequence) =
        fake_incoming_hover();
    let (_, transfer_id, requestor, property) = begin_fake_selection_transfer(
        &mut xwm,
        &mut peer,
        offer_id,
        mime_atom,
        timestamp,
        &mut server_sequence,
    );
    let replacement_source = 0x442;
    let replacement_enter = client_message(
        &xwm,
        xwm.root,
        XwmAtomName::XdndEnter,
        [replacement_source, 5 << 24, 0x552, 0, 0],
    );
    inject_client_message(&mut xwm, &mut peer, replacement_enter);
    let replacement = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .expect("replacement Enter owns the new session")
        .offer_id;
    assert_ne!(replacement, offer_id);
    assert!(
        !xwm.data_bridge
            .dnd_incoming
            .transfers
            .contains_key(&transfer_id)
    );
    xwm.connection.flush().unwrap();
    let replacement_requests = take_requests(&mut peer, &mut server_sequence);
    assert!(!replacement_requests.is_empty());

    let stale_position =
        crate::xwayland::XwaylandDndIncomingPositionId::new(offer_id, NonZeroU64::new(1).unwrap());
    super::source_feedback(
        &mut xwm,
        offer_id,
        stale_position,
        Some("text/plain".to_owned()),
        Some(crate::xwayland::XwaylandDndAction::Copy),
    )
    .unwrap();
    xwm.connection.flush().unwrap();
    assert!(
        read_peer(&mut peer).is_empty(),
        "old offer feedback is inert"
    );

    peer.write_all(&selection_notify(
        requestor,
        xwm.atoms.get(XwmAtomName::XdndSelection),
        mime_atom,
        property,
        timestamp,
        server_sequence,
    ))
    .unwrap();
    let _ = xwm.drain_events(32).unwrap();
    assert_eq!(
        xwm.data_bridge.dnd.incoming_session().unwrap().offer_id,
        replacement
    );
    assert!(
        !xwm.data_bridge
            .dnd_incoming
            .transfers
            .contains_key(&transfer_id)
    );
    xwm.connection.flush().unwrap();
    let late_requests = read_peer(&mut peer);
    assert!(
        !late_requests
            .chunks_exact(4)
            .any(|chunk| chunk[0] == xproto::GET_PROPERTY_REQUEST)
    );
}

#[test]
fn timeout_then_p2_feedback_keeps_p1_selection_authority_separate() {
    let (mut xwm, mut peer, offer_id, mime_atom, timestamp, _, mut server_sequence) =
        fake_incoming_hover();
    let p1 = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .latest_position
        .unwrap();
    let status_deadline = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .pending_status_deadline_ns
        .unwrap();
    let transfer_started_at = status_deadline.saturating_sub(super::TARGET_STATUS_TIMEOUT_NS);
    let (mut reader, transfer_id, requestor, property) = begin_fake_selection_transfer_at(
        &mut xwm,
        &mut peer,
        offer_id,
        mime_atom,
        timestamp,
        &mut server_sequence,
        transfer_started_at,
    );

    super::expire_deadlines(&mut xwm, status_deadline).unwrap();
    xwm.connection.flush().unwrap();
    let timeout_status = read_peer(&mut peer);
    let statuses = status_messages(&timeout_status, &xwm);
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].1[0], xwm.root);
    assert_eq!(statuses[0].1[1] & 1, 0, "P1 deadline rejects P1");
    server_sequence = server_sequence.wrapping_add(count_requests(&timeout_status));

    inject_copy_position(&mut xwm, &mut peer, timestamp + 1, 31, server_sequence);
    let p2 = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .latest_position
        .unwrap();
    assert_ne!(p1.position_id, p2.position_id);
    assert_eq!(p2.timestamp, timestamp + 1);

    super::source_feedback(
        &mut xwm,
        offer_id,
        p1.position_id,
        Some("text/plain".to_owned()),
        Some(crate::xwayland::XwaylandDndAction::Copy),
    )
    .unwrap();
    xwm.connection.flush().unwrap();
    assert!(
        read_peer(&mut peer).is_empty(),
        "late SourceFeedback(P1) cannot answer pending P2"
    );

    notify_fake_selection_transfer(
        &mut xwm,
        &mut peer,
        transfer_id,
        mime_atom,
        timestamp,
        requestor,
        property,
        server_sequence,
        &mut server_sequence,
    );
    assert_eq!(
        xwm.data_bridge.dnd_incoming.transfers[&transfer_id].phase,
        IncomingTransferPhase::ReadingProperty,
        "P1's original SelectionNotify remains authorized after P2"
    );
    let sequence = xwm.data_bridge.dnd_incoming.transfers[&transfer_id]
        .pending_reply
        .unwrap() as u16;
    peer.write_all(&get_property_reply(sequence, mime_atom, 8, b"P1 payload"))
        .unwrap();
    xwm.drain_events(32).unwrap();
    let mut received = [0; 10];
    reader.read_exact(&mut received).unwrap();
    assert_eq!(&received, b"P1 payload");

    super::source_feedback(
        &mut xwm,
        offer_id,
        p2.position_id,
        Some("text/plain".to_owned()),
        Some(crate::xwayland::XwaylandDndAction::Copy),
    )
    .unwrap();
    xwm.connection.flush().unwrap();
    let p2_status = status_messages(&read_peer(&mut peer), &xwm);
    assert_eq!(p2_status.len(), 1);
    assert_eq!(p2_status[0].1[0], xwm.root);
    assert_eq!(p2_status[0].1[1] & 1, 1, "P2 receives its own acceptance");
    assert_eq!(
        p2_status[0].1[4],
        xwm.atoms.get(XwmAtomName::XdndActionCopy)
    );
}

#[test]
fn fake_x_hover_incr_payload_reads_next_chunk_only_after_sink_delivery() {
    let (mut xwm, mut peer, offer_id, mime_atom, timestamp, _target_proxy, mut server_sequence) =
        fake_incoming_hover();
    let (mut reader, transfer_id, requestor, property) = begin_fake_selection_transfer(
        &mut xwm,
        &mut peer,
        offer_id,
        mime_atom,
        timestamp,
        &mut server_sequence,
    );
    inject_copy_position(&mut xwm, &mut peer, timestamp + 1, 44, server_sequence);
    assert_eq!(
        xwm.data_bridge.dnd_incoming.transfers[&transfer_id].selection_timestamp, timestamp,
        "a later Position does not replace the INCR transfer's Position timestamp"
    );
    notify_fake_selection_transfer(
        &mut xwm,
        &mut peer,
        transfer_id,
        mime_atom,
        timestamp,
        requestor,
        property,
        server_sequence,
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
    let first_survivor =
        crate::xwayland::XwaylandDndIncomingPositionId::new(first, NonZeroU64::new(2).unwrap());
    let replacement_position =
        crate::xwayland::XwaylandDndIncomingPositionId::new(second, NonZeroU64::new(1).unwrap());
    assert_ne!(
        crate::xwayland::XwaylandDndIncomingPositionId::new(first, NonZeroU64::new(1).unwrap(),),
        replacement_position,
        "the same serial remains distinct across replacement offers"
    );
    let mut manager = DndIncomingManager::default();
    let event = |offer_id, position_id, x| XwaylandDndIncomingEvent::Position {
        offer_id,
        position_id,
        x,
        y: 4.0,
        requested_action: crate::xwayland::XwaylandDndAction::Copy,
        source_actions: vec![crate::xwayland::XwaylandDndAction::Copy],
        x_timestamp: x as u32,
    };
    assert!(manager.push_event(event(
        first,
        crate::xwayland::XwaylandDndIncomingPositionId::new(first, NonZeroU64::new(1).unwrap(),),
        1.0,
    )));
    assert!(manager.push_event(event(first, first_survivor, 2.0)));
    assert!(manager.push_event(event(second, replacement_position, 3.0)));
    assert_eq!(manager.events.len(), 2);
    assert!(matches!(
        manager.events.front(),
        Some(XwaylandDndIncomingEvent::Position { x: 2.0, .. })
    ));
    assert!(matches!(
        manager.events.get(1),
        Some(XwaylandDndIncomingEvent::Position { position_id, .. })
            if *position_id == replacement_position
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
        position_id: crate::xwayland::XwaylandDndIncomingPositionId::new(
            first_id,
            NonZeroU64::new(1).unwrap(),
        ),
        x: 0.0,
        y: 0.0,
        requested_action: crate::xwayland::XwaylandDndAction::Copy,
        x_timestamp: 1,
    }));
    for serial in 2..MAX_PENDING_XWAYLAND_DND_INCOMING_EVENTS as u64 {
        let id = offer_id(generation, serial);
        assert!(manager.push_event(XwaylandDndIncomingEvent::Position {
            offer_id: id,
            position_id: crate::xwayland::XwaylandDndIncomingPositionId::new(
                id,
                NonZeroU64::new(1).unwrap(),
            ),
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

#[test]
fn current_position_rejection_sends_one_rejected_status() {
    let (mut xwm, mut peer, offer_id, _, _, _, _) = fake_incoming_hover();
    let position_id = xwm
        .data_bridge
        .dnd
        .incoming_session()
        .unwrap()
        .latest_position
        .unwrap()
        .position_id;
    super::source_feedback(&mut xwm, offer_id, position_id, None, None).unwrap();
    xwm.connection.flush().unwrap();
    let rejection = status_messages(&read_peer(&mut peer), &xwm);
    assert_eq!(rejection.len(), 1);
    assert_eq!(rejection[0].1[0], xwm.root);
    assert_eq!(rejection[0].1[1] & 1, 0);
    assert_eq!(rejection[0].1[4], 0);

    super::source_feedback(&mut xwm, offer_id, position_id, None, None).unwrap();
    xwm.connection.flush().unwrap();
    assert!(status_messages(&read_peer(&mut peer), &xwm).is_empty());
}
