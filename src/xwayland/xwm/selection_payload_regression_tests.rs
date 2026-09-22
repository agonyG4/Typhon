use std::{
    io::{Read, Write},
    os::fd::AsRawFd,
    os::unix::net::UnixStream,
};

use x11rb::{
    protocol::{xfixes, xproto},
    x11_utils::Serialize,
};

fn install_xfixes(xwm: &mut super::super::super::Xwm) {
    super::install_extension(
        xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        super::TEST_XFIXES_FIRST_EVENT,
        151,
    );
}

fn resolve_png_offer(
    xwm: &mut super::super::super::Xwm,
    peer: &mut UnixStream,
    target: u32,
    owner: u32,
) -> crate::xwayland::XwaylandSelectionOfferId {
    resolve_png_offer_at_revision(xwm, peer, target, owner, 40, 35, 0)
}

fn resolve_png_offer_at_revision(
    xwm: &mut super::super::super::Xwm,
    peer: &mut UnixStream,
    target: u32,
    owner: u32,
    time: u32,
    selection_time: u32,
    owner_event_sequence: u16,
) -> crate::xwayland::XwaylandSelectionOfferId {
    super::resolve_targets_for_kind_with_event_for_test(
        xwm,
        peer,
        super::super::super::data_bridge::SelectionKind::Clipboard,
        super::SelectionOwnerFixture {
            selection: super::TEST_CLIPBOARD_ATOM,
            observer: super::TEST_CLIPBOARD_OBSERVER_WINDOW,
            owner,
            timestamp: time,
            selection_timestamp: selection_time,
            sequence: owner_event_sequence,
        },
        &[target],
    );
    super::complete_atom_name_for_test(
        xwm,
        peer,
        super::super::super::data_bridge::SelectionKind::Clipboard,
        target,
        b"image/png",
    );
    let events = xwm.take_selection_events();
    let [crate::xwayland::XwaylandSelectionEvent::OfferChanged { offer, .. }] = events.as_slice()
    else {
        panic!("TARGETS catalog must publish one MIME offer");
    };
    offer.id
}

fn property_reply(sequence: u16, type_atom: u32, format: u8, bytes: &[u8]) -> Vec<u8> {
    property_reply_after(sequence, type_atom, format, bytes, 0)
}

fn property_reply_after(
    sequence: u16,
    type_atom: u32,
    format: u8,
    bytes: &[u8],
    bytes_after: u32,
) -> Vec<u8> {
    let value_len = match format {
        8 => bytes.len(),
        16 => bytes.len() / 2,
        32 => bytes.len() / 4,
        _ => panic!("unsupported fixture property format"),
    } as u32;
    let mut reply = xproto::GetPropertyReply {
        format,
        sequence,
        length: bytes.len().div_ceil(4) as u32,
        type_: type_atom,
        bytes_after,
        value_len,
        value: bytes.to_vec(),
    }
    .serialize();
    reply.resize(32 + bytes.len().div_ceil(4) * 4, 0);
    reply
}

fn property_notify(window: u32, atom: u32, sequence: u16) -> [u8; 32] {
    let serialized = xproto::PropertyNotifyEvent {
        response_type: xproto::PROPERTY_NOTIFY_EVENT,
        sequence,
        window,
        atom,
        time: 1,
        state: xproto::Property::NEW_VALUE,
    }
    .serialize();
    let mut event = [0; 32];
    event[..serialized.len()].copy_from_slice(&serialized);
    event
}

fn finish_notify(
    xwm: &mut super::super::super::Xwm,
    peer: &mut UnixStream,
    requestor: u32,
    target: u32,
    property: u32,
    sequence: u16,
) {
    peer.write_all(&super::raw_selection_notify(
        requestor,
        super::TEST_CLIPBOARD_ATOM,
        target,
        property,
        1,
        sequence,
    ))
    .expect("write payload SelectionNotify");
    xwm.drain_events(32)
        .expect("consume SelectionNotify and schedule property read");
}

fn current_payload_reply_sequence(
    xwm: &super::super::super::Xwm,
    id: super::super::super::SelectionPayloadTransferId,
) -> u16 {
    xwm.data_bridge
        .selection_payloads
        .pending_sequence_for_test(id)
        .expect("one bounded payload GetProperty request") as u16
}

fn last_payload_request_sequence(
    xwm: &super::super::super::Xwm,
    id: super::super::super::SelectionPayloadTransferId,
) -> u16 {
    xwm.data_bridge
        .selection_payloads
        .last_request_sequence_for_test(id)
        .expect("latest payload request sequence") as u16
}

#[test]
fn direct_property_payload_is_written_once_and_requestor_is_reused() {
    let generation = super::generation(201);
    let (mut xwm, mut peer) = super::test_fixture(generation);
    xwm.data_bridge.selection_payloads.initialize_generation(
        super::super::super::data_bridge::BridgeGeneration::from(generation),
    );
    install_xfixes(&mut xwm);
    let target = 0xd501;
    let offer_id = resolve_png_offer(&mut xwm, &mut peer, target, 0x501);
    let (sink_stream, mut receiver) = UnixStream::pair().expect("create Wayland payload sink");
    let id = super::super::super::selection_payload::start_request(
        &mut xwm,
        crate::xwayland::XwaylandSelectionDataRequest {
            offer_id,
            mime_type: "image/png".to_owned(),
            sink: sink_stream.into(),
        },
        10,
    )
    .expect("start exact catalog conversion")
    .expect("current MIME offer is accepted");
    xwm.flush().expect("flush payload request batch");
    let conversion = super::convert_selection_requests(&super::read_fixture_requests(&mut peer))
        .pop()
        .expect("one ConvertSelection request");
    assert_eq!(
        conversion.target, target,
        "catalog target atom is preserved exactly"
    );
    assert_eq!(conversion.time, x11rb::CURRENT_TIME);

    let property = xwm
        .atoms
        .get(super::super::super::atoms::XwmAtomName::SelectionData);
    let selection_sequence = last_payload_request_sequence(&xwm, id);
    finish_notify(
        &mut xwm,
        &mut peer,
        conversion.requestor,
        target,
        property,
        selection_sequence,
    );
    let _ = super::read_fixture_requests(&mut peer);
    let sequence = current_payload_reply_sequence(&xwm, id);
    peer.write_all(&property_reply(sequence, target, 8, b"abc"))
        .expect("write direct selection property reply");
    let drain = xwm
        .drain_events(32)
        .expect("write property bytes and complete payload");
    assert_eq!(
        xwm.data_bridge.selection_payloads.active_count(),
        0,
        "direct payload transfer reaches terminal state after its property reply: {drain:?}"
    );
    receiver
        .set_read_timeout(Some(std::time::Duration::from_secs(1)))
        .expect("bound fixture sink read");
    let mut actual = [0; 3];
    receiver
        .read_exact(&mut actual)
        .expect("read exact Wayland sink payload");
    assert_eq!(&actual, b"abc");
    assert_eq!(xwm.data_bridge.selection_payloads.active_count(), 0);
    let deletes = super::fixture_request_opcodes(&super::read_fixture_requests(&mut peer))
        .into_iter()
        .filter(|(opcode, _)| *opcode == xproto::DELETE_PROPERTY_REQUEST)
        .count();
    assert_eq!(deletes, 1, "direct property is deleted after consumption");

    let (sink_stream, _receiver) = UnixStream::pair().expect("create second payload sink");
    let second = super::super::super::selection_payload::start_request(
        &mut xwm,
        crate::xwayland::XwaylandSelectionDataRequest {
            offer_id,
            mime_type: "image/png".to_owned(),
            sink: sink_stream.into(),
        },
        20,
    )
    .expect("start second conversion")
    .expect("same current offer remains readable");
    xwm.flush().expect("flush reused payload request");
    let second_conversion =
        super::convert_selection_requests(&super::read_fixture_requests(&mut peer))
            .pop()
            .expect("second ConvertSelection");
    assert_ne!(second, id);
    assert_eq!(second_conversion.requestor, conversion.requestor);
    xwm.clear_generation(generation);
}

#[test]
fn incoming_incr_waits_for_bounded_payload_property_and_zero_terminator() {
    let generation = super::generation(202);
    let (mut xwm, mut peer) = super::test_fixture(generation);
    xwm.data_bridge.selection_payloads.initialize_generation(
        super::super::super::data_bridge::BridgeGeneration::from(generation),
    );
    install_xfixes(&mut xwm);
    let target = 0xd502;
    let offer_id = resolve_png_offer(&mut xwm, &mut peer, target, 0x502);
    let (sink_stream, mut receiver) = UnixStream::pair().expect("create Wayland payload sink");
    let id = super::super::super::selection_payload::start_request(
        &mut xwm,
        crate::xwayland::XwaylandSelectionDataRequest {
            offer_id,
            mime_type: "image/png".to_owned(),
            sink: sink_stream.into(),
        },
        10,
    )
    .expect("start INCR conversion")
    .expect("current MIME offer is accepted");
    xwm.flush().expect("flush INCR request batch");
    let conversion = super::convert_selection_requests(&super::read_fixture_requests(&mut peer))
        .pop()
        .expect("one ConvertSelection request");
    let property = xwm
        .atoms
        .get(super::super::super::atoms::XwmAtomName::SelectionData);
    let selection_sequence = last_payload_request_sequence(&xwm, id);
    finish_notify(
        &mut xwm,
        &mut peer,
        conversion.requestor,
        target,
        property,
        selection_sequence,
    );
    let _ = super::read_fixture_requests(&mut peer);
    let sequence = current_payload_reply_sequence(&xwm, id);
    let incr_atom = xwm.atoms.get(super::super::super::atoms::XwmAtomName::Incr);
    peer.write_all(&property_reply(
        sequence,
        incr_atom,
        32,
        &0x7fff_ffff_u32.to_ne_bytes(),
    ))
    .expect("write advisory INCR marker");
    xwm.drain_events(32).expect("consume INCR marker");
    assert_eq!(xwm.data_bridge.selection_payloads.active_count(), 1);
    let marker_deletes = super::fixture_request_opcodes(&super::read_fixture_requests(&mut peer))
        .into_iter()
        .filter(|(opcode, _)| *opcode == xproto::DELETE_PROPERTY_REQUEST)
        .count();
    assert_eq!(marker_deletes, 1, "INCR marker is acknowledged once");

    for (chunk_index, chunk) in [b"A".as_slice(), b"B".as_slice(), b"".as_slice()]
        .into_iter()
        .enumerate()
    {
        let event_sequence = last_payload_request_sequence(&xwm, id);
        peer.write_all(&property_notify(
            conversion.requestor,
            property,
            event_sequence,
        ))
        .expect("write private requestor PropertyNotify");
        xwm.drain_events(32)
            .expect("internal PropertyNotify reaches the payload state machine");
        let _ = super::read_fixture_requests(&mut peer);
        let sequence = current_payload_reply_sequence(&xwm, id);
        peer.write_all(&property_reply(sequence, target, 8, chunk))
            .expect("write bounded INCR chunk property reply");
        let drain = xwm
            .drain_events(32)
            .expect("consume chunk before DeleteProperty acknowledgement");
        let deletes = super::fixture_request_opcodes(&super::read_fixture_requests(&mut peer))
            .into_iter()
            .filter(|(opcode, _)| *opcode == xproto::DELETE_PROPERTY_REQUEST)
            .count();
        assert_eq!(
            deletes,
            1,
            "one acknowledgement follows INCR chunk {chunk_index} (active={}, pending={}, drain={drain:?})",
            xwm.data_bridge.selection_payloads.active_count(),
            xwm.data_bridge.selection_payloads.pending_reply_count(),
        );
    }
    let mut actual = [0; 2];
    receiver
        .read_exact(&mut actual)
        .expect("read all INCR bytes before terminal close");
    assert_eq!(&actual, b"AB");
    assert_eq!(xwm.data_bridge.selection_payloads.active_count(), 0);
    xwm.clear_generation(generation);
}

#[test]
fn queued_stale_offer_is_dropped_instead_of_retargeted() {
    let generation = super::generation(203);
    let (mut xwm, mut peer) = super::test_fixture(generation);
    xwm.data_bridge.selection_payloads.initialize_generation(
        super::super::super::data_bridge::BridgeGeneration::from(generation),
    );
    install_xfixes(&mut xwm);
    let offer_a = resolve_png_offer(&mut xwm, &mut peer, 0xd503, 0x503);
    let offer_b = resolve_png_offer_at_revision(&mut xwm, &mut peer, 0xd504, 0x504, 41, 36, 3);
    assert_ne!(offer_a, offer_b);

    let (sink_stream, _receiver) = UnixStream::pair().expect("create stale sink");
    assert!(
        super::super::super::selection_payload::start_request(
            &mut xwm,
            crate::xwayland::XwaylandSelectionDataRequest {
                offer_id: offer_a,
                mime_type: "image/png".to_owned(),
                sink: sink_stream.into(),
            },
            10,
        )
        .expect("stale request is a local rejection")
        .is_none()
    );
    assert!(super::convert_selection_requests(&super::read_fixture_requests(&mut peer)).is_empty());
    assert_eq!(xwm.data_bridge.selection_payloads.active_count(), 0);
    xwm.clear_generation(generation);
}

#[test]
fn active_transfer_and_pending_property_reply_bounds_hold() {
    let generation = super::generation(204);
    let (mut xwm, mut peer) = super::test_fixture(generation);
    xwm.data_bridge.selection_payloads.initialize_generation(
        super::super::super::data_bridge::BridgeGeneration::from(generation),
    );
    install_xfixes(&mut xwm);
    let target = 0xd505;
    let offer_id = resolve_png_offer(&mut xwm, &mut peer, target, 0x505);
    let mut ids = Vec::new();
    for index in 0..=super::super::super::selection_payload::MAX_ACTIVE_SELECTION_PAYLOAD_TRANSFERS
    {
        let (sink_stream, _receiver) = UnixStream::pair().expect("create bounded test sink");
        let started = super::super::super::selection_payload::start_request(
            &mut xwm,
            crate::xwayland::XwaylandSelectionDataRequest {
                offer_id,
                mime_type: "image/png".to_owned(),
                sink: sink_stream.into(),
            },
            10 + index as u64,
        )
        .expect("over-cap request is a local rejection");
        if let Some(id) = started {
            ids.push(id);
        }
    }
    xwm.flush()
        .expect("flush the accepted bounded request batch");
    assert_eq!(
        ids.len(),
        super::super::super::selection_payload::MAX_ACTIVE_SELECTION_PAYLOAD_TRANSFERS
    );
    assert_eq!(
        xwm.data_bridge
            .selection_payloads
            .slot_count(super::super::super::data_bridge::SelectionKind::Clipboard),
        super::super::super::selection_payload::MAX_SELECTION_PAYLOAD_SLOTS_PER_CHANNEL
    );
    let conversions = super::convert_selection_requests(&super::read_fixture_requests(&mut peer));
    assert_eq!(
        conversions.len(),
        ids.len(),
        "65th request issues no conversion"
    );
    for (id, conversion) in ids.iter().copied().zip(conversions) {
        peer.write_all(&super::raw_selection_notify(
            conversion.requestor,
            conversion.selection,
            conversion.target,
            conversion.property,
            conversion.time,
            last_payload_request_sequence(&xwm, id),
        ))
        .expect("write bounded-transfer SelectionNotify");
    }
    xwm.drain_events(256)
        .expect("schedule bounded payload property reads");
    let _ = super::read_fixture_requests(&mut peer);
    assert_eq!(
        xwm.data_bridge.selection_payloads.pending_reply_count(),
        super::super::super::selection_payload::MAX_PENDING_SELECTION_PAYLOAD_REPLIES
    );
    assert_eq!(
        xwm.data_bridge.selection_payloads.active_count(),
        super::super::super::selection_payload::MAX_ACTIVE_SELECTION_PAYLOAD_TRANSFERS
    );
    xwm.clear_generation(generation);
    assert_eq!(xwm.data_bridge.selection_payloads.active_count(), 0);
    assert_eq!(xwm.data_bridge.selection_payloads.pending_reply_count(), 0);
}

#[test]
fn oversized_direct_property_streams_with_real_sink_backpressure() {
    let generation = super::generation(205);
    let (mut xwm, mut peer) = super::test_fixture(generation);
    xwm.data_bridge.selection_payloads.initialize_generation(
        super::super::super::data_bridge::BridgeGeneration::from(generation),
    );
    install_xfixes(&mut xwm);
    let target = 0xd506;
    let offer_id = resolve_png_offer(&mut xwm, &mut peer, target, 0x506);
    let (mut sink_stream, mut receiver) = UnixStream::pair().expect("create backpressure socket");
    sink_stream
        .set_nonblocking(true)
        .expect("make sink writer nonblocking before filling it");
    receiver
        .set_nonblocking(true)
        .expect("make receiver drain nonblocking");
    let send_buffer = 4096_i32;
    // SAFETY: the socket FD is live and the option value has the required size.
    let set_result = unsafe {
        libc::setsockopt(
            sink_stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_SNDBUF,
            (&send_buffer as *const i32).cast(),
            std::mem::size_of::<i32>() as libc::socklen_t,
        )
    };
    assert_eq!(set_result, 0);
    let filler = [b'f'; 1024];
    let mut fill_bytes = 0usize;
    let hit_would_block = loop {
        match sink_stream.write(&filler) {
            Ok(0) => panic!("nonblocking sink stopped accepting filler"),
            Ok(count) => fill_bytes += count,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                break true;
            }
            Err(error) => panic!("fill sink send buffer: {error}"),
        }
    };
    assert!(hit_would_block, "the real Unix socket reaches EAGAIN");
    let id = super::super::super::selection_payload::start_request(
        &mut xwm,
        crate::xwayland::XwaylandSelectionDataRequest {
            offer_id,
            mime_type: "image/png".to_owned(),
            sink: sink_stream.into(),
        },
        10,
    )
    .expect("start direct conversion")
    .expect("current MIME offer is accepted");
    xwm.flush().expect("flush direct request batch");
    let conversion = super::convert_selection_requests(&super::read_fixture_requests(&mut peer))
        .pop()
        .expect("one ConvertSelection");
    let property = xwm
        .atoms
        .get(super::super::super::atoms::XwmAtomName::SelectionData);
    let selection_sequence = last_payload_request_sequence(&xwm, id);
    finish_notify(
        &mut xwm,
        &mut peer,
        conversion.requestor,
        target,
        property,
        selection_sequence,
    );
    let _ = super::read_fixture_requests(&mut peer);
    let sequence = current_payload_reply_sequence(&xwm, id);
    let chunk =
        vec![b'x'; super::super::super::selection_payload::MAX_SELECTION_PAYLOAD_CHUNK_BYTES];
    peer.write_all(&property_reply_after(sequence, target, 8, &chunk, 4))
        .expect("write first bounded direct-property piece");
    xwm.drain_events(32)
        .expect("consume first direct-property piece");
    xwm.data_bridge
        .selection_payloads
        .bind_reactor_token(id, Some(77));
    assert!(
        xwm.data_bridge
            .selection_payloads
            .sink_interests()
            .any(|(transfer_id, _)| transfer_id == id)
    );
    assert!(
        super::fixture_request_opcodes(&super::read_fixture_requests(&mut peer))
            .iter()
            .all(|(opcode, _)| *opcode != xproto::DELETE_PROPERTY_REQUEST)
    );

    let mut actual = Vec::new();
    let mut scratch = [0; 8192];
    loop {
        loop {
            match receiver.read(&mut scratch) {
                Ok(0) => panic!("payload socket closed before conversion completed"),
                Ok(count) => actual.extend_from_slice(&scratch[..count]),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) => panic!("read blocked sink data: {error}"),
            }
        }
        let waiting = xwm
            .data_bridge
            .selection_payloads
            .sink_interests()
            .any(|(transfer_id, _)| transfer_id == id);
        if !waiting {
            break;
        }
        super::super::super::selection_payload::handle_sink_ready(
            &mut xwm,
            id,
            super::super::super::data_bridge::BridgeGeneration::from(generation),
            77,
            libc::EPOLLOUT as u32,
            20,
        )
        .expect("resume buffered partial write");
    }
    assert_eq!(actual.len(), fill_bytes + chunk.len());
    assert!(actual[..fill_bytes].iter().all(|byte| *byte == b'f'));
    assert_eq!(&actual[fill_bytes..], chunk.as_slice());
    assert!(
        xwm.data_bridge
            .selection_payloads
            .pending_sequence_for_test(id)
            .is_some()
    );
    let sequence = current_payload_reply_sequence(&xwm, id);
    peer.write_all(&property_reply(sequence, target, 8, b"tail"))
        .expect("write final direct-property piece");
    xwm.drain_events(32)
        .expect("consume final property piece and delete it");
    loop {
        match receiver.read(&mut scratch) {
            Ok(0) if xwm.data_bridge.selection_payloads.active_count() == 0 => break,
            Ok(0) => panic!("payload socket closed before the transfer completed"),
            Ok(count) => actual.extend_from_slice(&scratch[..count]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) => panic!("read terminal sink bytes: {error}"),
        }
    }
    assert_eq!(actual.len(), fill_bytes + chunk.len() + 4);
    assert!(actual[..fill_bytes].iter().all(|byte| *byte == b'f'));
    assert_eq!(
        &actual[fill_bytes..fill_bytes + chunk.len()],
        chunk.as_slice()
    );
    assert_eq!(&actual[fill_bytes + chunk.len()..], b"tail");
    assert_eq!(xwm.data_bridge.selection_payloads.active_count(), 0);
    let deletes = super::fixture_request_opcodes(&super::read_fixture_requests(&mut peer))
        .into_iter()
        .filter(|(opcode, _)| *opcode == xproto::DELETE_PROPERTY_REQUEST)
        .count();
    assert_eq!(
        deletes, 1,
        "the direct property is deleted after the final write"
    );
    xwm.clear_generation(generation);
}
