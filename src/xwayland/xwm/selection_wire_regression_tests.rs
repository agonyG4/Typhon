use std::{io::Write, os::unix::net::UnixStream};

use x11rb::{
    protocol::{xfixes, xproto},
    x11_utils::Serialize,
};

use super::{
    regression_tests::{install_extension, read_fixture_requests},
    tests::{generation, test_fixture},
};

#[derive(Debug, PartialEq, Eq)]
struct ConvertSelectionRequest {
    requestor: u32,
    selection: u32,
    target: u32,
    property: u32,
    time: u32,
}

fn convert_selection_requests(bytes: &[u8]) -> Vec<ConvertSelectionRequest> {
    const CONVERT_SELECTION_OPCODE: u8 = 24;
    let mut requests = Vec::new();
    let mut offset: usize = 0;
    while offset.saturating_add(4) <= bytes.len() {
        let opcode = bytes[offset];
        let length = usize::from(u16::from_le_bytes([bytes[offset + 2], bytes[offset + 3]]));
        let request_bytes = length * 4;
        assert!(request_bytes >= 4 && offset + request_bytes <= bytes.len());
        if opcode == CONVERT_SELECTION_OPCODE {
            assert_eq!(request_bytes, 24, "ConvertSelection has six 32-bit words");
            let word = |at: usize| {
                u32::from_le_bytes(bytes[offset + at..offset + at + 4].try_into().unwrap())
            };
            requests.push(ConvertSelectionRequest {
                requestor: word(4),
                selection: word(8),
                target: word(12),
                property: word(16),
                time: word(20),
            });
        }
        offset += request_bytes;
    }
    assert_eq!(offset, bytes.len(), "trailing bytes after X11 requests");
    requests
}

fn fixture_request_opcodes(bytes: &[u8]) -> Vec<(u8, u8)> {
    let mut requests = Vec::new();
    let mut offset: usize = 0;
    while offset.saturating_add(4) <= bytes.len() {
        let length = usize::from(u16::from_le_bytes([bytes[offset + 2], bytes[offset + 3]]));
        let request_bytes = length * 4;
        assert!(request_bytes >= 4 && offset + request_bytes <= bytes.len());
        requests.push((bytes[offset], bytes[offset + 1]));
        offset += request_bytes;
    }
    assert_eq!(offset, bytes.len(), "trailing bytes after X11 requests");
    requests
}

fn discover_preexisting_owners(
    xwm: &mut super::super::Xwm,
    peer: &mut UnixStream,
    clipboard_owner: u32,
    primary_owner: u32,
) -> Vec<ConvertSelectionRequest> {
    super::super::selection_wire::initialize(xwm).expect("initialize selection wire");
    let requests = read_fixture_requests(peer);
    let opcodes = fixture_request_opcodes(&requests);
    let subscription_positions = opcodes
        .iter()
        .enumerate()
        .filter_map(|(index, (opcode, _))| (*opcode == 201).then_some(index))
        .collect::<Vec<_>>();
    let owner_probe_positions = opcodes
        .iter()
        .enumerate()
        .filter_map(|(index, (opcode, _))| (*opcode == 23).then_some(index))
        .collect::<Vec<_>>();
    assert_eq!(
        subscription_positions.len(),
        2,
        "one XFixes subscription per selection"
    );
    assert_eq!(
        owner_probe_positions.len(),
        2,
        "one initial owner probe per selection"
    );
    assert!(
        subscription_positions.iter().max().unwrap() < owner_probe_positions.iter().min().unwrap(),
        "both XFixes subscriptions must precede the owner probes"
    );

    let clipboard_sequence = super::super::selection_wire::pending_sequence_for_test(
        xwm,
        super::super::data_bridge::SelectionKind::Clipboard,
        false,
    )
    .expect("clipboard owner probe sequence");
    let primary_sequence = super::super::selection_wire::pending_sequence_for_test(
        xwm,
        super::super::data_bridge::SelectionKind::Primary,
        false,
    )
    .expect("primary owner probe sequence");
    assert!(clipboard_sequence < primary_sequence);
    peer.write_all(&raw_get_selection_owner_reply(
        clipboard_sequence as u16,
        clipboard_owner,
    ))
    .expect("write clipboard owner probe reply");
    peer.write_all(&raw_get_selection_owner_reply(
        primary_sequence as u16,
        primary_owner,
    ))
    .expect("write primary owner probe reply");
    xwm.drain_events(32)
        .expect("complete asynchronous initial owner probes");
    convert_selection_requests(&read_fixture_requests(peer))
}

const TEST_XFIXES_FIRST_EVENT: u8 = 100;
const TEST_CLIPBOARD_ATOM: u32 = 0xcafe;
const TEST_TARGETS_ATOM: u32 = 0xcaff;
const TEST_SELECTION_TARGETS_PROPERTY: u32 = 0xcb00;
const TEST_CLIPBOARD_OBSERVER_WINDOW: u32 = 0xa010;
const TEST_CLIPBOARD_REQUESTOR_WINDOW: u32 = 0xa011;
const TEST_PRIMARY_ATOM: u32 = 1;
const TEST_PRIMARY_OBSERVER_WINDOW: u32 = 0xa012;
const TEST_PRIMARY_REQUESTOR_WINDOW: u32 = 0xa013;

#[derive(Clone, Copy)]
struct SelectionOwnerFixture {
    selection: u32,
    observer: u32,
    owner: u32,
    timestamp: u32,
    selection_timestamp: u32,
    sequence: u16,
}

fn raw_clipboard_owner_event(
    owner: u32,
    timestamp: u32,
    selection_timestamp: u32,
    sequence: u16,
) -> [u8; 32] {
    raw_selection_owner_event(
        TEST_CLIPBOARD_ATOM,
        TEST_CLIPBOARD_OBSERVER_WINDOW,
        owner,
        timestamp,
        selection_timestamp,
        sequence,
    )
}

fn raw_selection_owner_event(
    selection: u32,
    observer: u32,
    owner: u32,
    timestamp: u32,
    selection_timestamp: u32,
    sequence: u16,
) -> [u8; 32] {
    raw_xfixes_selection_event(
        selection,
        observer,
        xfixes::SelectionEvent::SET_SELECTION_OWNER,
        owner,
        timestamp,
        selection_timestamp,
        sequence,
    )
}

fn raw_xfixes_selection_event(
    selection: u32,
    observer: u32,
    subtype: xfixes::SelectionEvent,
    owner: u32,
    timestamp: u32,
    selection_timestamp: u32,
    sequence: u16,
) -> [u8; 32] {
    xfixes::SelectionNotifyEvent {
        response_type: TEST_XFIXES_FIRST_EVENT + xfixes::SELECTION_NOTIFY_EVENT,
        subtype,
        sequence,
        window: observer,
        owner,
        selection,
        timestamp,
        selection_timestamp,
    }
    .serialize()
}

fn assert_owner_loss_clears_pending_targets(
    generation_id: u64,
    kind: super::super::data_bridge::SelectionKind,
    selection_atom: u32,
    observer: u32,
    owner_loss_subtype: xfixes::SelectionEvent,
) {
    use super::super::data_bridge::selection::TargetsDiscoveryState;

    let (mut xwm, mut peer) = test_fixture(generation(generation_id));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );
    peer.write_all(&raw_selection_owner_event(
        selection_atom,
        observer,
        0x330,
        40,
        35,
        0,
    ))
    .expect("write serialized XFixes owner event");
    xwm.drain_events(32).expect("start TARGETS conversion");
    let conversion = convert_selection_requests(&read_fixture_requests(&mut peer))
        .pop()
        .expect("owner TARGETS conversion");
    peer.write_all(&raw_selection_notify(
        conversion.requestor,
        conversion.selection,
        conversion.target,
        conversion.property,
        conversion.time,
        1,
    ))
    .expect("write serialized SelectionNotify");
    xwm.drain_events(32)
        .expect("start asynchronous TARGETS property read");
    let pending_sequence =
        super::super::selection_wire::pending_sequence_for_test(&xwm, kind, true)
            .expect("owner property reply must be pending before owner loss");
    let _ = read_fixture_requests(&mut peer);

    peer.write_all(&raw_xfixes_selection_event(
        selection_atom,
        observer,
        owner_loss_subtype,
        0,
        41,
        35,
        2,
    ))
    .expect("write serialized XFixes owner-loss event with owner NONE");
    xwm.drain_events(32)
        .expect("normalize XFixes owner-loss event through the XWM stream");

    let selection = xwm
        .data_bridge
        .selections
        .current(kind)
        .expect("selection channel remains generation scoped");
    assert_eq!(selection.owner, None);
    assert_eq!(selection.origin, None);
    assert!(selection.targets.is_empty());
    assert_eq!(selection.targets_state, TargetsDiscoveryState::Idle);
    assert!(
        super::super::selection_wire::pending_sequence_for_test(&xwm, kind, true).is_none(),
        "owner loss must cancel the previous revision's tracked property reply"
    );
    let owner_loss_requests = read_fixture_requests(&mut peer);
    assert!(
        convert_selection_requests(&owner_loss_requests).is_empty(),
        "clearing an owner must not start another TARGETS conversion"
    );

    peer.write_all(&raw_get_property_reply(
        pending_sequence as u16,
        4,
        &[TEST_TARGETS_ATOM],
    ))
    .expect("write late old-revision property reply");
    xwm.drain_events(32)
        .expect("discard late property reply after owner loss");
    let selection = xwm
        .data_bridge
        .selections
        .current(kind)
        .expect("selection channel remains generation scoped");
    assert_eq!(selection.owner, None);
    assert_eq!(selection.targets_state, TargetsDiscoveryState::Idle);
    assert!(selection.targets.is_empty());
}

fn raw_selection_notify(
    requestor: u32,
    selection: u32,
    target: u32,
    property: u32,
    time: u32,
    sequence: u16,
) -> [u8; 32] {
    let serialized = xproto::SelectionNotifyEvent {
        response_type: xproto::SELECTION_NOTIFY_EVENT,
        sequence,
        time,
        requestor,
        selection,
        target,
        property,
    }
    .serialize();
    let mut event = [0_u8; 32];
    event[..serialized.len()].copy_from_slice(&serialized);
    event
}

fn raw_get_selection_owner_reply(sequence: u16, owner: u32) -> [u8; 32] {
    let serialized = xproto::GetSelectionOwnerReply {
        sequence,
        length: 0,
        owner,
    }
    .serialize();
    let mut reply = [0_u8; 32];
    reply[..serialized.len()].copy_from_slice(&serialized);
    reply
}

fn raw_get_property_reply(sequence: u16, type_atom: u32, targets: &[u32]) -> Vec<u8> {
    let mut value = Vec::with_capacity(targets.len() * 4);
    for target in targets {
        value.extend_from_slice(&target.to_ne_bytes());
    }
    xproto::GetPropertyReply {
        format: 32,
        sequence,
        length: targets.len() as u32,
        type_: type_atom,
        bytes_after: 0,
        value_len: targets.len() as u32,
        value,
    }
    .serialize()
}

fn raw_get_atom_name_reply(sequence: u16, name: &[u8]) -> Vec<u8> {
    let mut serialized = xproto::GetAtomNameReply {
        sequence,
        length: name.len().div_ceil(4) as u32,
        name: name.to_vec(),
    }
    .serialize();
    serialized.resize(32 + serialized.len().saturating_sub(32).div_ceil(4) * 4, 0);
    serialized
}

fn get_atom_name_requests(bytes: &[u8]) -> Vec<u32> {
    let mut atoms = Vec::new();
    let mut offset: usize = 0;
    while offset.saturating_add(8) <= bytes.len() {
        let opcode = bytes[offset];
        let length = usize::from(u16::from_le_bytes([bytes[offset + 2], bytes[offset + 3]]));
        let request_bytes = length * 4;
        assert!(request_bytes >= 4 && offset + request_bytes <= bytes.len());
        if opcode == xproto::GET_ATOM_NAME_REQUEST {
            assert_eq!(request_bytes, 8);
            atoms.push(u32::from_ne_bytes(
                bytes[offset + 4..offset + 8].try_into().unwrap(),
            ));
        }
        offset += request_bytes;
    }
    assert_eq!(offset, bytes.len(), "trailing bytes after X11 requests");
    atoms
}

fn resolve_targets_for_test(
    xwm: &mut super::super::Xwm,
    peer: &mut UnixStream,
    owner: u32,
    targets: &[u32],
) {
    resolve_targets_for_kind_for_test(
        xwm,
        peer,
        super::super::data_bridge::SelectionKind::Clipboard,
        TEST_CLIPBOARD_ATOM,
        TEST_CLIPBOARD_OBSERVER_WINDOW,
        owner,
        targets,
    );
}

fn resolve_targets_for_kind_for_test(
    xwm: &mut super::super::Xwm,
    peer: &mut UnixStream,
    kind: super::super::data_bridge::SelectionKind,
    selection: u32,
    observer: u32,
    owner: u32,
    targets: &[u32],
) {
    resolve_targets_for_kind_with_event_for_test(
        xwm,
        peer,
        kind,
        SelectionOwnerFixture {
            selection,
            observer,
            owner,
            timestamp: 40,
            selection_timestamp: 35,
            sequence: 0,
        },
        targets,
    );
}

fn resolve_targets_for_kind_with_event_for_test(
    xwm: &mut super::super::Xwm,
    peer: &mut UnixStream,
    kind: super::super::data_bridge::SelectionKind,
    owner_event: SelectionOwnerFixture,
    targets: &[u32],
) {
    peer.write_all(&raw_selection_owner_event(
        owner_event.selection,
        owner_event.observer,
        owner_event.owner,
        owner_event.timestamp,
        owner_event.selection_timestamp,
        owner_event.sequence,
    ))
    .expect("write serialized XFixes owner event");
    xwm.drain_events(32).expect("start TARGETS conversion");
    let notify_sequence = xwm
        .data_bridge
        .selection_wire
        .last_convert_selection_sequence_for_test()
        .expect("TARGETS ConvertSelection sequence") as u16;
    let conversion = convert_selection_requests(&read_fixture_requests(peer))
        .pop()
        .expect("owner TARGETS conversion");
    peer.write_all(&raw_selection_notify(
        conversion.requestor,
        conversion.selection,
        conversion.target,
        conversion.property,
        conversion.time,
        notify_sequence,
    ))
    .expect("write serialized SelectionNotify");
    xwm.drain_events(32).expect("start TARGETS property read");
    let property_sequence =
        super::super::selection_wire::pending_sequence_for_test(xwm, kind, true)
            .expect("TARGETS property sequence");
    let _ = read_fixture_requests(peer);
    peer.write_all(&raw_get_property_reply(
        property_sequence as u16,
        u32::from(xproto::AtomEnum::ATOM),
        targets,
    ))
    .expect("write serialized TARGETS property reply");
    xwm.drain_events(32).expect("resolve TARGETS property");
}

fn complete_atom_name_for_test(
    xwm: &mut super::super::Xwm,
    peer: &mut UnixStream,
    kind: super::super::data_bridge::SelectionKind,
    target: u32,
    name: &[u8],
) {
    let sequence =
        super::super::selection_wire::pending_target_atom_name_sequence_for_test(xwm, kind, target)
            .expect("atom-name reply must be pending");
    peer.write_all(&raw_get_atom_name_reply(sequence as u16, name))
        .expect("write atom-name reply");
    xwm.drain_events(32)
        .expect("resolve asynchronous atom-name reply");
}

#[test]
fn xfixes_external_clipboard_owner_starts_targets_query() {
    let (mut xwm, mut peer) = test_fixture(generation(90));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );

    peer.write_all(&raw_clipboard_owner_event(0x303, 40, 35, 0))
        .expect("write serialized XFixes owner event");
    xwm.drain_events(32)
        .expect("normalize XFixes owner event through the XWM stream");

    let requests = read_fixture_requests(&mut peer);
    let owner_recorded = xwm
        .data_bridge
        .selections
        .current(super::super::data_bridge::SelectionKind::Clipboard)
        .is_some_and(|selection| selection.owner == Some(0x303));
    assert_eq!(
        (owner_recorded, convert_selection_requests(&requests)),
        (
            true,
            vec![ConvertSelectionRequest {
                requestor: TEST_CLIPBOARD_REQUESTOR_WINDOW,
                selection: TEST_CLIPBOARD_ATOM,
                target: TEST_TARGETS_ATOM,
                property: TEST_SELECTION_TARGETS_PROPERTY,
                time: 35,
            }]
        ),
        "an external CLIPBOARD owner must be recorded and queried on the wire"
    );
}

#[test]
fn arbitrary_mime_atom_requires_async_get_atom_name() {
    let (mut xwm, mut peer) = test_fixture(generation(139));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );

    resolve_targets_for_test(&mut xwm, &mut peer, 0x380, &[TEST_TARGETS_ATOM, 0xd001]);

    let requests = read_fixture_requests(&mut peer);
    assert_eq!(get_atom_name_requests(&requests), vec![0xd001]);
}

#[test]
fn utf8_string_resolves_to_utf8_plain_text_alias() {
    use crate::xwayland::{XwaylandSelectionEvent, XwaylandSelectionKind};

    let (mut xwm, mut peer) = test_fixture(generation(140));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );
    let utf8_string = xwm.atoms.get(super::super::atoms::XwmAtomName::Utf8String);
    resolve_targets_for_test(&mut xwm, &mut peer, 0x381, &[utf8_string]);

    let events = xwm.take_selection_events();
    assert_eq!(events.len(), 1);
    match &events[0] {
        XwaylandSelectionEvent::OfferChanged { kind, offer } => {
            assert_eq!(*kind, XwaylandSelectionKind::Clipboard);
            assert_eq!(offer.mime_types, ["text/plain;charset=utf-8"]);
        }
        XwaylandSelectionEvent::Cleared { .. } => {
            panic!("UTF8_STRING must produce an external offer")
        }
    }
}

#[test]
fn exact_utf8_mime_target_wins_over_utf8_string_alias() {
    use super::super::data_bridge::SelectionKind;
    use crate::xwayland::{XwaylandSelectionEvent, XwaylandSelectionKind};

    let (mut xwm, mut peer) = test_fixture(generation(141));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );
    let utf8_string = xwm.atoms.get(super::super::atoms::XwmAtomName::Utf8String);
    let exact_mime_atom = 0xd002;
    resolve_targets_for_test(&mut xwm, &mut peer, 0x382, &[utf8_string, exact_mime_atom]);
    let requests = read_fixture_requests(&mut peer);
    assert_eq!(get_atom_name_requests(&requests), vec![exact_mime_atom]);
    let name_sequence = super::super::selection_wire::pending_target_atom_name_sequence_for_test(
        &xwm,
        SelectionKind::Clipboard,
        exact_mime_atom,
    )
    .expect("exact MIME atom-name reply must be pending");
    peer.write_all(&raw_get_atom_name_reply(
        name_sequence as u16,
        b"text/plain;charset=utf-8",
    ))
    .expect("write exact MIME atom-name reply");
    xwm.drain_events(32)
        .expect("complete exact MIME atom-name resolution");

    let mut events = xwm.take_selection_events();
    assert_eq!(events.len(), 1);
    let offer = match events.pop().unwrap() {
        XwaylandSelectionEvent::OfferChanged { kind, offer } => {
            assert_eq!(kind, XwaylandSelectionKind::Clipboard);
            offer
        }
        XwaylandSelectionEvent::Cleared { .. } => {
            panic!("exact MIME atom must produce an external offer")
        }
    };
    assert_eq!(offer.mime_types, ["text/plain;charset=utf-8"]);
    assert_eq!(offer.id.generation, generation(141));
    assert_eq!(offer.id.kind, XwaylandSelectionKind::Clipboard);
    assert_eq!(offer.id.revision, 1);
    assert_eq!(
        super::super::selection_wire::target_for_mime_for_test(
            &xwm,
            SelectionKind::Clipboard,
            offer.id,
            "text/plain;charset=utf-8",
        ),
        Some(exact_mime_atom)
    );
}

#[test]
fn text_atom_resolves_to_plain_text_alias() {
    use crate::xwayland::{XwaylandSelectionEvent, XwaylandSelectionKind};

    let (mut xwm, mut peer) = test_fixture(generation(142));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );
    let text = xwm.atoms.get(super::super::atoms::XwmAtomName::Text);
    resolve_targets_for_test(&mut xwm, &mut peer, 0x383, &[text]);

    assert!(matches!(
        xwm.take_selection_events().as_slice(),
        [XwaylandSelectionEvent::OfferChanged { kind: XwaylandSelectionKind::Clipboard, offer }]
            if offer.mime_types == ["text/plain"]
    ));
}

#[test]
fn exact_plain_text_target_wins_over_text_alias() {
    use super::super::data_bridge::SelectionKind;
    use crate::xwayland::{XwaylandSelectionEvent, XwaylandSelectionKind};

    let (mut xwm, mut peer) = test_fixture(generation(143));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );
    let text = xwm.atoms.get(super::super::atoms::XwmAtomName::Text);
    let exact_mime_atom = 0xd003;
    resolve_targets_for_test(&mut xwm, &mut peer, 0x384, &[text, exact_mime_atom]);
    let _ = read_fixture_requests(&mut peer);
    complete_atom_name_for_test(
        &mut xwm,
        &mut peer,
        SelectionKind::Clipboard,
        exact_mime_atom,
        b"text/plain",
    );

    let events = xwm.take_selection_events();
    let offer = match events.as_slice() {
        [XwaylandSelectionEvent::OfferChanged { kind, offer }] => {
            assert_eq!(*kind, XwaylandSelectionKind::Clipboard);
            offer
        }
        _ => panic!("exact text MIME target must produce one offer"),
    };
    assert_eq!(offer.mime_types, ["text/plain"]);
    assert_eq!(
        super::super::selection_wire::target_for_mime_for_test(
            &xwm,
            SelectionKind::Clipboard,
            offer.id,
            "text/plain",
        ),
        Some(exact_mime_atom)
    );
}

#[test]
fn control_targets_are_not_exposed_as_mime_types() {
    use crate::xwayland::{XwaylandSelectionEvent, XwaylandSelectionKind};

    let (mut xwm, mut peer) = test_fixture(generation(144));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );
    let targets = xwm.atoms.get(super::super::atoms::XwmAtomName::Targets);
    let timestamp = xwm.atoms.get(super::super::atoms::XwmAtomName::Timestamp);
    let multiple = xwm.atoms.get(super::super::atoms::XwmAtomName::Multiple);
    let incr = xwm.atoms.get(super::super::atoms::XwmAtomName::Incr);
    let utf8_string = xwm.atoms.get(super::super::atoms::XwmAtomName::Utf8String);
    resolve_targets_for_test(
        &mut xwm,
        &mut peer,
        0x385,
        &[targets, timestamp, multiple, incr, utf8_string],
    );

    assert!(matches!(
        xwm.take_selection_events().as_slice(),
        [XwaylandSelectionEvent::OfferChanged { kind: XwaylandSelectionKind::Clipboard, offer }]
            if offer.mime_types == ["text/plain;charset=utf-8"]
    ));
}

#[test]
fn empty_catalog_does_not_publish_offer() {
    let (mut xwm, mut peer) = test_fixture(generation(155));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );
    let targets = xwm.atoms.get(super::super::atoms::XwmAtomName::Targets);
    let timestamp = xwm.atoms.get(super::super::atoms::XwmAtomName::Timestamp);
    let multiple = xwm.atoms.get(super::super::atoms::XwmAtomName::Multiple);
    let incr = xwm.atoms.get(super::super::atoms::XwmAtomName::Incr);
    let selection_targets = xwm
        .atoms
        .get(super::super::atoms::XwmAtomName::SelectionTargets);
    resolve_targets_for_test(
        &mut xwm,
        &mut peer,
        0x393,
        &[targets, timestamp, multiple, incr, selection_targets],
    );
    assert!(xwm_take_selection_events_is_empty(&mut xwm));
}

#[test]
fn unknown_non_mime_atom_is_filtered() {
    let (mut xwm, mut peer) = test_fixture(generation(145));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );
    let target = 0xd004;
    resolve_targets_for_test(&mut xwm, &mut peer, 0x386, &[target]);
    complete_atom_name_for_test(
        &mut xwm,
        &mut peer,
        super::super::data_bridge::SelectionKind::Clipboard,
        target,
        b"application",
    );
    assert!(xwm_take_selection_events_is_empty(&mut xwm));
}

#[test]
fn invalid_utf8_atom_name_is_filtered() {
    let (mut xwm, mut peer) = test_fixture(generation(146));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );
    let target = 0xd005;
    resolve_targets_for_test(&mut xwm, &mut peer, 0x387, &[target]);
    complete_atom_name_for_test(
        &mut xwm,
        &mut peer,
        super::super::data_bridge::SelectionKind::Clipboard,
        target,
        &[0xff],
    );
    assert!(xwm_take_selection_events_is_empty(&mut xwm));
}

#[test]
fn oversized_atom_name_is_filtered() {
    let (mut xwm, mut peer) = test_fixture(generation(147));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );
    let target = 0xd006;
    resolve_targets_for_test(&mut xwm, &mut peer, 0x388, &[target]);
    let mut name = vec![b'a'; 4_097];
    name[4_096] = b'/';
    complete_atom_name_for_test(
        &mut xwm,
        &mut peer,
        super::super::data_bridge::SelectionKind::Clipboard,
        target,
        &name,
    );
    assert!(xwm_take_selection_events_is_empty(&mut xwm));
}

fn xwm_take_selection_events_is_empty(xwm: &mut super::super::Xwm) -> bool {
    xwm.take_selection_events().is_empty()
}

#[test]
fn catalog_preserves_target_order_deterministically() {
    use crate::xwayland::XwaylandSelectionEvent;

    let (mut xwm, mut peer) = test_fixture(generation(148));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );
    let first = 0xd007;
    let second = 0xd008;
    resolve_targets_for_test(&mut xwm, &mut peer, 0x389, &[first, second]);
    let requests = read_fixture_requests(&mut peer);
    assert_eq!(get_atom_name_requests(&requests), vec![first, second]);
    complete_atom_name_for_test(
        &mut xwm,
        &mut peer,
        super::super::data_bridge::SelectionKind::Clipboard,
        first,
        b"image/png",
    );
    complete_atom_name_for_test(
        &mut xwm,
        &mut peer,
        super::super::data_bridge::SelectionKind::Clipboard,
        second,
        b"text/html",
    );
    let events = xwm.take_selection_events();
    let [XwaylandSelectionEvent::OfferChanged { offer, .. }] = events.as_slice() else {
        panic!("two MIME targets must produce one offer");
    };
    assert_eq!(offer.mime_types, ["image/png", "text/html"]);
}

#[test]
fn atom_name_resolution_respects_pending_reply_bound() {
    use super::super::data_bridge::SelectionKind;
    use crate::xwayland::XwaylandSelectionEvent;

    let (mut xwm, mut peer) = test_fixture(generation(149));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );
    let targets = (0..128).map(|index| 0xd100 + index).collect::<Vec<_>>();
    resolve_targets_for_test(&mut xwm, &mut peer, 0x38a, &targets);
    let initial_requests = get_atom_name_requests(&read_fixture_requests(&mut peer));
    assert_eq!(initial_requests.len(), 4);
    let mut maximum_pending = initial_requests.len();
    for (index, target) in targets.iter().copied().enumerate() {
        complete_atom_name_for_test(
            &mut xwm,
            &mut peer,
            SelectionKind::Clipboard,
            target,
            format!("image/x-{index}").as_bytes(),
        );
        let refill = get_atom_name_requests(&read_fixture_requests(&mut peer));
        assert!(refill.len() <= 1, "one completed query may refill one slot");
        maximum_pending = maximum_pending.max(refill.len());
    }
    assert!(maximum_pending <= 4);
    let events = xwm.take_selection_events();
    assert!(matches!(
        events.as_slice(),
        [XwaylandSelectionEvent::OfferChanged { offer, .. }] if offer.mime_types.len() == 128
    ));
}

#[test]
fn clipboard_and_primary_catalogs_make_progress_concurrently() {
    use super::super::data_bridge::SelectionKind;
    use crate::xwayland::{XwaylandSelectionEvent, XwaylandSelectionKind};

    let (mut xwm, mut peer) = test_fixture(generation(150));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );
    let clipboard_target = 0xd200;
    let primary_target = 0xd201;
    peer.write_all(&raw_clipboard_owner_event(0x38b, 40, 35, 0))
        .expect("write clipboard owner event");
    peer.write_all(&raw_selection_owner_event(
        TEST_PRIMARY_ATOM,
        TEST_PRIMARY_OBSERVER_WINDOW,
        0x38c,
        41,
        36,
        1,
    ))
    .expect("write primary owner event");
    xwm.drain_events(32).expect("start both conversions");
    let conversions = convert_selection_requests(&read_fixture_requests(&mut peer));
    assert_eq!(conversions.len(), 2);
    for conversion in &conversions {
        peer.write_all(&raw_selection_notify(
            conversion.requestor,
            conversion.selection,
            conversion.target,
            conversion.property,
            conversion.time,
            2,
        ))
        .expect("write SelectionNotify");
    }
    xwm.drain_events(32).expect("start both property reads");
    let clipboard_property = super::super::selection_wire::pending_sequence_for_test(
        &xwm,
        SelectionKind::Clipboard,
        true,
    )
    .expect("clipboard property sequence");
    let primary_property =
        super::super::selection_wire::pending_sequence_for_test(&xwm, SelectionKind::Primary, true)
            .expect("primary property sequence");
    let _ = read_fixture_requests(&mut peer);
    peer.write_all(&raw_get_property_reply(
        clipboard_property as u16,
        u32::from(xproto::AtomEnum::ATOM),
        &[clipboard_target],
    ))
    .expect("write clipboard TARGETS reply");
    peer.write_all(&raw_get_property_reply(
        primary_property as u16,
        u32::from(xproto::AtomEnum::ATOM),
        &[primary_target],
    ))
    .expect("write primary TARGETS reply");
    xwm.drain_events(32)
        .expect("schedule both atom-name queries");
    let name_requests = get_atom_name_requests(&read_fixture_requests(&mut peer));
    assert_eq!(name_requests.len(), 2);
    assert!(name_requests.contains(&clipboard_target));
    assert!(name_requests.contains(&primary_target));
    complete_atom_name_for_test(
        &mut xwm,
        &mut peer,
        SelectionKind::Clipboard,
        clipboard_target,
        b"image/clipboard",
    );
    complete_atom_name_for_test(
        &mut xwm,
        &mut peer,
        SelectionKind::Primary,
        primary_target,
        b"image/primary",
    );
    let events = xwm.take_selection_events();
    assert_eq!(events.len(), 2);
    assert!(events.iter().any(|event| matches!(
        event,
        XwaylandSelectionEvent::OfferChanged {
            kind: XwaylandSelectionKind::Clipboard,
            ..
        }
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        XwaylandSelectionEvent::OfferChanged {
            kind: XwaylandSelectionKind::Primary,
            ..
        }
    )));
}

#[test]
fn owner_replacement_cancels_stale_atom_name_resolution() {
    use super::super::data_bridge::SelectionKind;
    use crate::xwayland::XwaylandSelectionEvent;

    let (mut xwm, mut peer) = test_fixture(generation(151));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );
    let old_target = 0xd202;
    let new_target = 0xd203;
    resolve_targets_for_test(&mut xwm, &mut peer, 0x38d, &[old_target]);
    let old_name_sequence =
        super::super::selection_wire::pending_target_atom_name_sequence_for_test(
            &xwm,
            SelectionKind::Clipboard,
            old_target,
        )
        .expect("old atom-name query");
    peer.write_all(&raw_clipboard_owner_event(0x38e, 41, 36, 2))
        .expect("write replacement owner event");
    xwm.drain_events(32)
        .expect("cancel old resolution and start replacement");
    let conversion = convert_selection_requests(&read_fixture_requests(&mut peer))
        .pop()
        .expect("replacement TARGETS conversion");
    peer.write_all(&raw_get_atom_name_reply(
        old_name_sequence as u16,
        b"image/stale",
    ))
    .expect("write stale atom-name reply");
    peer.write_all(&raw_selection_notify(
        conversion.requestor,
        conversion.selection,
        conversion.target,
        conversion.property,
        conversion.time,
        3,
    ))
    .expect("write replacement SelectionNotify");
    xwm.drain_events(32)
        .expect("ignore stale reply and start replacement property read");
    let property_sequence = super::super::selection_wire::pending_sequence_for_test(
        &xwm,
        SelectionKind::Clipboard,
        true,
    )
    .expect("replacement property sequence");
    let _ = read_fixture_requests(&mut peer);
    peer.write_all(&raw_get_property_reply(
        property_sequence as u16,
        u32::from(xproto::AtomEnum::ATOM),
        &[new_target],
    ))
    .expect("write replacement TARGETS reply");
    xwm.drain_events(32).expect("start replacement name query");
    complete_atom_name_for_test(
        &mut xwm,
        &mut peer,
        SelectionKind::Clipboard,
        new_target,
        b"image/current",
    );
    let events = xwm.take_selection_events();
    let [XwaylandSelectionEvent::OfferChanged { offer, .. }] = events.as_slice() else {
        panic!("replacement catalog must produce one offer");
    };
    assert_eq!(offer.mime_types, ["image/current"]);
}

#[test]
fn generation_cleanup_discards_target_catalog_and_late_reply() {
    use super::super::data_bridge::SelectionKind;

    let (mut xwm, mut peer) = test_fixture(generation(152));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );
    let target = 0xd204;
    resolve_targets_for_test(&mut xwm, &mut peer, 0x38f, &[target]);
    let sequence = super::super::selection_wire::pending_target_atom_name_sequence_for_test(
        &xwm,
        SelectionKind::Clipboard,
        target,
    )
    .expect("pending atom-name query");
    let _ = read_fixture_requests(&mut peer);
    xwm.clear_generation(generation(152));
    peer.write_all(&raw_get_atom_name_reply(sequence as u16, b"image/old"))
        .expect("write late retired-generation reply");
    xwm.drain_events(32)
        .expect("discard retired-generation atom-name reply");
    assert!(xwm_take_selection_events_is_empty(&mut xwm));
}

#[test]
fn metadata_updates_are_coalesced_per_selection_kind() {
    use crate::xwayland::XwaylandSelectionEvent;

    let (mut xwm, mut peer) = test_fixture(generation(153));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );
    let utf8_string = xwm.atoms.get(super::super::atoms::XwmAtomName::Utf8String);
    resolve_targets_for_test(&mut xwm, &mut peer, 0x390, &[utf8_string]);
    peer.write_all(&raw_clipboard_owner_event(0x391, 41, 36, 2))
        .expect("write replacement owner event");
    xwm.drain_events(32)
        .expect("replace owner before consuming metadata");
    let events = xwm.take_selection_events();
    assert!(matches!(
        events.as_slice(),
        [XwaylandSelectionEvent::Cleared { .. }]
    ));
}

#[test]
fn retirement_snapshot_clears_only_current_offers_and_is_idempotent() {
    use crate::xwayland::{XwaylandSelectionEvent, XwaylandSelectionKind};

    let current_offer_generation = generation(157);
    let (mut xwm, _peer) = test_fixture(current_offer_generation);
    let offer = xwm.seed_external_selection_offer_for_tests(XwaylandSelectionKind::Clipboard, 1);

    assert_eq!(
        xwm.take_selection_events_for_retirement(),
        [XwaylandSelectionEvent::Cleared {
            kind: XwaylandSelectionKind::Clipboard,
            generation: offer.id.generation,
        }]
    );
    assert!(xwm.take_selection_events_for_retirement().is_empty());

    let cleared_generation = generation(158);
    let (mut xwm, _peer) = test_fixture(cleared_generation);
    let _ = xwm.seed_external_selection_offer_for_tests(XwaylandSelectionKind::Primary, 1);
    let _ = xwm.take_selection_events();
    xwm.clear_external_selection_offer_for_tests(XwaylandSelectionKind::Primary);
    assert_eq!(
        xwm.take_selection_events_for_retirement(),
        [XwaylandSelectionEvent::Cleared {
            kind: XwaylandSelectionKind::Primary,
            generation: cleared_generation,
        }]
    );
    assert!(xwm.take_selection_events_for_retirement().is_empty());
}

#[test]
fn internal_owner_does_not_publish_external_offer() {
    use crate::xwayland::XwaylandSelectionEvent;

    let (mut xwm, mut peer) = test_fixture(generation(154));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );
    let utf8_string = xwm.atoms.get(super::super::atoms::XwmAtomName::Utf8String);
    resolve_targets_for_test(&mut xwm, &mut peer, 0x392, &[utf8_string]);
    peer.write_all(&raw_clipboard_owner_event(
        TEST_CLIPBOARD_REQUESTOR_WINDOW,
        41,
        36,
        2,
    ))
    .expect("write internal owner event");
    xwm.drain_events(32)
        .expect("process internal owner transition");
    let events = xwm.take_selection_events();
    assert!(matches!(
        events.as_slice(),
        [XwaylandSelectionEvent::Cleared { .. }]
    ));
}

#[test]
fn clipboard_and_primary_metadata_are_independent() {
    use crate::xwayland::{XwaylandSelectionEvent, XwaylandSelectionKind};

    let (mut xwm, mut peer) = test_fixture(generation(156));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );
    let utf8_string = xwm.atoms.get(super::super::atoms::XwmAtomName::Utf8String);
    resolve_targets_for_kind_for_test(
        &mut xwm,
        &mut peer,
        super::super::data_bridge::SelectionKind::Clipboard,
        TEST_CLIPBOARD_ATOM,
        TEST_CLIPBOARD_OBSERVER_WINDOW,
        0x394,
        &[utf8_string],
    );
    resolve_targets_for_kind_for_test(
        &mut xwm,
        &mut peer,
        super::super::data_bridge::SelectionKind::Primary,
        TEST_PRIMARY_ATOM,
        TEST_PRIMARY_OBSERVER_WINDOW,
        0x395,
        &[utf8_string],
    );
    peer.write_all(&raw_clipboard_owner_event(0x396, 42, 37, 2))
        .expect("write clipboard replacement owner event");
    xwm.drain_events(32).expect("clear only clipboard metadata");
    let events = xwm.take_selection_events();
    assert!(events.iter().any(|event| matches!(
        event,
        XwaylandSelectionEvent::Cleared {
            kind: XwaylandSelectionKind::Clipboard,
            ..
        }
    )));
    assert!(events.iter().all(|event| {
        !matches!(
            event,
            XwaylandSelectionEvent::Cleared {
                kind: XwaylandSelectionKind::Primary,
                ..
            }
        )
    }));
}

#[test]
fn xfixes_external_primary_owner_has_independent_state() {
    let (mut xwm, mut peer) = test_fixture(generation(92));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );

    peer.write_all(&raw_selection_owner_event(
        TEST_PRIMARY_ATOM,
        TEST_PRIMARY_OBSERVER_WINDOW,
        0x304,
        41,
        36,
        0,
    ))
    .expect("write serialized XFixes PRIMARY event");
    xwm.drain_events(32)
        .expect("normalize XFixes PRIMARY event through the XWM stream");

    let requests = convert_selection_requests(&read_fixture_requests(&mut peer));
    assert_eq!(
        requests,
        vec![ConvertSelectionRequest {
            requestor: TEST_PRIMARY_REQUESTOR_WINDOW,
            selection: TEST_PRIMARY_ATOM,
            target: TEST_TARGETS_ATOM,
            property: TEST_SELECTION_TARGETS_PROPERTY,
            time: 36,
        }]
    );
    assert_eq!(
        xwm.data_bridge
            .selections
            .current(super::super::data_bridge::SelectionKind::Primary)
            .and_then(|selection| selection.owner),
        Some(0x304)
    );
    assert_eq!(
        xwm.data_bridge
            .selections
            .current(super::super::data_bridge::SelectionKind::Clipboard)
            .and_then(|selection| selection.owner),
        None
    );
}

#[test]
fn xfixes_window_destroy_clears_current_selection_owner() {
    assert_owner_loss_clears_pending_targets(
        121,
        super::super::data_bridge::SelectionKind::Clipboard,
        TEST_CLIPBOARD_ATOM,
        TEST_CLIPBOARD_OBSERVER_WINDOW,
        xfixes::SelectionEvent::SELECTION_WINDOW_DESTROY,
    );
}

#[test]
fn xfixes_client_close_clears_current_selection_owner() {
    assert_owner_loss_clears_pending_targets(
        122,
        super::super::data_bridge::SelectionKind::Primary,
        TEST_PRIMARY_ATOM,
        TEST_PRIMARY_OBSERVER_WINDOW,
        xfixes::SelectionEvent::SELECTION_CLIENT_CLOSE,
    );
}

#[test]
fn pending_selection_notify_rotates_requestor_even_when_timestamp_differs() {
    use super::super::data_bridge::{SelectionKind, selection::TargetsDiscoveryState};

    let (mut xwm, mut peer) = test_fixture(generation(123));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );
    peer.write_all(&raw_clipboard_owner_event(0x331, 40, 35, 0))
        .expect("write owner A event");
    xwm.drain_events(32).expect("start owner A conversion");
    let conversion_a = convert_selection_requests(&read_fixture_requests(&mut peer))
        .pop()
        .expect("owner A conversion");

    peer.write_all(&raw_clipboard_owner_event(0x332, 41, 36, 1))
        .expect("write owner B event with a distinct timestamp");
    xwm.drain_events(32).expect("start owner B conversion");
    let requests_b = read_fixture_requests(&mut peer);
    let conversion_b = convert_selection_requests(&requests_b)
        .pop()
        .expect("owner B conversion");
    assert_ne!(conversion_b.requestor, conversion_a.requestor);

    peer.write_all(&raw_selection_notify(
        conversion_a.requestor,
        conversion_a.selection,
        conversion_a.target,
        conversion_a.property,
        conversion_a.time,
        2,
    ))
    .expect("write late owner A SelectionNotify");
    xwm.drain_events(32)
        .expect("reject owner A by its stale timestamp");
    assert!(read_fixture_requests(&mut peer).is_empty());
    let selection = xwm
        .data_bridge
        .selections
        .current(SelectionKind::Clipboard)
        .expect("owner B state after stale SelectionNotify");
    assert_eq!(selection.owner, Some(0x332));
    assert_eq!(
        selection.targets_state,
        TargetsDiscoveryState::AwaitingSelectionNotify
    );

    peer.write_all(&raw_selection_notify(
        conversion_b.requestor,
        conversion_b.selection,
        conversion_b.target,
        conversion_b.property,
        conversion_b.time,
        3,
    ))
    .expect("write matching owner B SelectionNotify");
    xwm.drain_events(32)
        .expect("accept owner B and begin property read");
    assert!(
        super::super::selection_wire::pending_sequence_for_test(
            &xwm,
            SelectionKind::Clipboard,
            true,
        )
        .is_some()
    );
    assert_eq!(
        xwm.data_bridge
            .selections
            .current(SelectionKind::Clipboard)
            .unwrap()
            .targets_state,
        TargetsDiscoveryState::AwaitingProperty
    );
}

#[test]
fn same_timestamp_pending_conversion_rotates_requestor() {
    use super::super::data_bridge::{SelectionKind, selection::TargetsDiscoveryState};

    let (mut xwm, mut peer) = test_fixture(generation(127));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );
    peer.write_all(&raw_clipboard_owner_event(0x33a, 40, 35, 0))
        .expect("write owner A event");
    xwm.drain_events(32).expect("start owner A conversion");
    let conversion_a = convert_selection_requests(&read_fixture_requests(&mut peer))
        .pop()
        .expect("owner A conversion");

    peer.write_all(&raw_clipboard_owner_event(0x33b, 41, 35, 1))
        .expect("write owner B replacement at the same timestamp");
    xwm.drain_events(32).expect("start owner B conversion");
    let conversion_b = convert_selection_requests(&read_fixture_requests(&mut peer))
        .pop()
        .expect("owner B conversion");
    assert_ne!(conversion_a.requestor, conversion_b.requestor);
    assert!(
        !xwm.data_bridge
            .selection_wire
            .requestor_poisoned_for_test(SelectionKind::Clipboard)
    );

    peer.write_all(&raw_selection_notify(
        conversion_a.requestor,
        conversion_a.selection,
        conversion_a.target,
        conversion_a.property,
        conversion_a.time,
        2,
    ))
    .expect("write late owner A SelectionNotify");
    xwm.drain_events(32)
        .expect("ignore owner A by its retired requestor");
    assert!(read_fixture_requests(&mut peer).is_empty());
    let selection = xwm
        .data_bridge
        .selections
        .current(SelectionKind::Clipboard)
        .expect("owner B state after stale SelectionNotify");
    assert_eq!(selection.owner, Some(0x33b));
    assert_eq!(
        selection.targets_state,
        TargetsDiscoveryState::AwaitingSelectionNotify
    );

    peer.write_all(&raw_selection_notify(
        conversion_b.requestor,
        conversion_b.selection,
        conversion_b.target,
        conversion_b.property,
        conversion_b.time,
        3,
    ))
    .expect("write matching owner B SelectionNotify");
    xwm.drain_events(32)
        .expect("accept owner B and start its property read");
    assert!(
        super::super::selection_wire::pending_sequence_for_test(
            &xwm,
            SelectionKind::Clipboard,
            true,
        )
        .is_some()
    );
    assert_eq!(
        xwm.data_bridge
            .selections
            .current(SelectionKind::Clipboard)
            .unwrap()
            .targets_state,
        TargetsDiscoveryState::AwaitingProperty
    );
}

#[test]
fn unknown_timestamp_pending_conversion_rotates_requestor() {
    use super::super::data_bridge::{SelectionKind, selection::TargetsDiscoveryState};

    let (mut xwm, mut peer) = test_fixture(generation(126));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );
    peer.write_all(&raw_clipboard_owner_event(0x338, 40, 0, 0))
        .expect("write owner A event with unknown timestamp");
    xwm.drain_events(32).expect("start owner A conversion");
    let conversion_a = convert_selection_requests(&read_fixture_requests(&mut peer))
        .pop()
        .expect("owner A conversion with unknown timestamp");
    assert_eq!(conversion_a.time, 0);

    peer.write_all(&raw_clipboard_owner_event(0x339, 41, 0, 1))
        .expect("write owner B event with unknown timestamp");
    xwm.drain_events(32).expect("start owner B conversion");
    let conversion_b = convert_selection_requests(&read_fixture_requests(&mut peer))
        .pop()
        .expect("owner B conversion");
    assert_ne!(conversion_a.requestor, conversion_b.requestor);

    peer.write_all(&raw_selection_notify(
        conversion_a.requestor,
        conversion_a.selection,
        conversion_a.target,
        conversion_a.property,
        55,
        2,
    ))
    .expect("write late owner A SelectionNotify with unknown old time");
    xwm.drain_events(32)
        .expect("reject owner A through requestor identity");
    assert!(read_fixture_requests(&mut peer).is_empty());
    assert_eq!(
        xwm.data_bridge
            .selections
            .current(SelectionKind::Clipboard)
            .unwrap()
            .targets_state,
        TargetsDiscoveryState::AwaitingSelectionNotify
    );

    peer.write_all(&raw_selection_notify(
        conversion_b.requestor,
        conversion_b.selection,
        conversion_b.target,
        conversion_b.property,
        56,
        3,
    ))
    .expect("write matching owner B SelectionNotify");
    xwm.drain_events(32)
        .expect("accept owner B using its distinct requestor");
    assert!(
        super::super::selection_wire::pending_sequence_for_test(
            &xwm,
            SelectionKind::Clipboard,
            true,
        )
        .is_some()
    );
}

#[test]
fn owner_clear_with_pending_notify_prevents_requestor_alias() {
    use super::super::data_bridge::{SelectionKind, selection::TargetsDiscoveryState};

    let (mut xwm, mut peer) = test_fixture(generation(124));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );
    peer.write_all(&raw_clipboard_owner_event(0x333, 40, 35, 0))
        .expect("write owner A event");
    xwm.drain_events(32).expect("start owner A conversion");
    let conversion_a = convert_selection_requests(&read_fixture_requests(&mut peer))
        .pop()
        .expect("owner A conversion");

    peer.write_all(&raw_xfixes_selection_event(
        TEST_CLIPBOARD_ATOM,
        TEST_CLIPBOARD_OBSERVER_WINDOW,
        xfixes::SelectionEvent::SELECTION_WINDOW_DESTROY,
        0,
        41,
        35,
        1,
    ))
    .expect("write owner destroy with owner NONE and the same selection timestamp");
    xwm.drain_events(32)
        .expect("clear owner and retire its pending SelectionNotify");
    xwm.flush()
        .expect("flush owner-loss requestor retirement without a conversion");
    let clear_requests = read_fixture_requests(&mut peer);
    assert!(convert_selection_requests(&clear_requests).is_empty());
    assert!(
        fixture_request_opcodes(&clear_requests)
            .iter()
            .any(|(opcode, _)| *opcode == 1),
        "owner loss must serialize a fresh requestor for the pending old notification"
    );
    assert_eq!(
        xwm.data_bridge
            .selections
            .current(SelectionKind::Clipboard)
            .unwrap()
            .owner,
        None
    );

    peer.write_all(&raw_clipboard_owner_event(0x334, 42, 35, 2))
        .expect("write owner B event reusing timestamp T");
    xwm.drain_events(32).expect("start owner B conversion");
    let owner_b_requests = read_fixture_requests(&mut peer);
    let conversion_b = convert_selection_requests(&owner_b_requests)
        .pop()
        .expect("owner B conversion");
    assert_ne!(conversion_a.requestor, conversion_b.requestor);
    assert!(
        !xwm.data_bridge
            .selection_wire
            .requestor_poisoned_for_test(SelectionKind::Clipboard)
    );

    peer.write_all(&raw_selection_notify(
        conversion_a.requestor,
        conversion_a.selection,
        conversion_a.target,
        conversion_a.property,
        conversion_a.time,
        3,
    ))
    .expect("write late owner A SelectionNotify");
    xwm.drain_events(32)
        .expect("ignore owner A by its retired requestor");
    assert!(read_fixture_requests(&mut peer).is_empty());
    assert_eq!(
        xwm.data_bridge
            .selections
            .current(SelectionKind::Clipboard)
            .unwrap()
            .targets_state,
        TargetsDiscoveryState::AwaitingSelectionNotify
    );

    peer.write_all(&raw_selection_notify(
        conversion_b.requestor,
        conversion_b.selection,
        conversion_b.target,
        conversion_b.property,
        conversion_b.time,
        4,
    ))
    .expect("write matching owner B SelectionNotify");
    xwm.drain_events(32)
        .expect("accept owner B on its distinct requestor");
    assert!(
        super::super::selection_wire::pending_sequence_for_test(
            &xwm,
            SelectionKind::Clipboard,
            true,
        )
        .is_some()
    );
}

#[test]
fn awaiting_property_allows_requestor_reuse() {
    use super::super::data_bridge::{SelectionKind, selection::TargetsDiscoveryState};

    let (mut xwm, mut peer) = test_fixture(generation(135));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );

    peer.write_all(&raw_clipboard_owner_event(0x350, 40, 35, 0))
        .expect("write owner A event");
    xwm.drain_events(32).expect("start owner A conversion");
    let conversion_a = convert_selection_requests(&read_fixture_requests(&mut peer))
        .pop()
        .expect("owner A conversion");
    peer.write_all(&raw_selection_notify(
        conversion_a.requestor,
        conversion_a.selection,
        conversion_a.target,
        conversion_a.property,
        conversion_a.time,
        1,
    ))
    .expect("write owner A SelectionNotify");
    xwm.drain_events(32).expect("start owner A property read");
    assert_eq!(
        xwm.data_bridge
            .selections
            .current(SelectionKind::Clipboard)
            .unwrap()
            .targets_state,
        TargetsDiscoveryState::AwaitingProperty
    );
    let _ = read_fixture_requests(&mut peer);

    peer.write_all(&raw_clipboard_owner_event(0x351, 41, 36, 2))
        .expect("write owner B event during owner A property read");
    xwm.drain_events(32)
        .expect("reuse requestor for owner B after SelectionNotify consumption");
    let conversion_b = convert_selection_requests(&read_fixture_requests(&mut peer))
        .pop()
        .expect("owner B conversion");
    assert_eq!(conversion_b.requestor, conversion_a.requestor);
    assert_eq!(
        xwm.data_bridge
            .selections
            .current(SelectionKind::Clipboard)
            .unwrap()
            .targets_state,
        TargetsDiscoveryState::AwaitingSelectionNotify
    );
}

#[test]
fn preexisting_clipboard_owner_is_discovered_after_subscription() {
    let (mut xwm, mut peer) = test_fixture(generation(99));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );

    let conversions = discover_preexisting_owners(&mut xwm, &mut peer, 0x309, 0);

    assert_eq!(
        xwm.data_bridge
            .selections
            .current(super::super::data_bridge::SelectionKind::Clipboard)
            .and_then(|selection| selection.owner),
        Some(0x309)
    );
    assert_eq!(
        xwm.data_bridge
            .selections
            .current(super::super::data_bridge::SelectionKind::Primary)
            .and_then(|selection| selection.owner),
        None
    );
    assert_eq!(conversions.len(), 1);
    assert_eq!(conversions[0].selection, TEST_CLIPBOARD_ATOM);
}

#[test]
fn preexisting_primary_owner_is_discovered_after_subscription() {
    let (mut xwm, mut peer) = test_fixture(generation(100));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );

    let conversions = discover_preexisting_owners(&mut xwm, &mut peer, 0, 0x30a);

    assert_eq!(
        xwm.data_bridge
            .selections
            .current(super::super::data_bridge::SelectionKind::Primary)
            .and_then(|selection| selection.owner),
        Some(0x30a)
    );
    assert_eq!(conversions.len(), 1);
    assert_eq!(conversions[0].selection, TEST_PRIMARY_ATOM);
}

#[test]
fn missing_xfixes_leaves_selection_wire_inactive() {
    let (mut xwm, mut peer) = test_fixture(generation(101));
    xwm.capabilities.xfixes = false;
    xwm.data_bridge.selection_wire = Default::default();

    super::super::selection_wire::initialize(&mut xwm)
        .expect("missing XFixes is a supported inactive state");

    assert!(!xwm.data_bridge.selection_wire.is_active());
    assert!(read_fixture_requests(&mut peer).is_empty());
    assert!(
        xwm.data_bridge
            .selections
            .current(super::super::data_bridge::SelectionKind::Clipboard)
            .is_some_and(|selection| selection.owner.is_none())
    );
}

#[test]
fn self_owned_xfixes_notification_does_not_reflect() {
    let (mut xwm, mut peer) = test_fixture(generation(93));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );

    peer.write_all(&raw_clipboard_owner_event(0x303, 40, 35, 0))
        .expect("write external owner before self-owned event");
    xwm.drain_events(32)
        .expect("start external TARGETS discovery");
    let _ = read_fixture_requests(&mut peer);

    peer.write_all(&raw_clipboard_owner_event(
        TEST_CLIPBOARD_REQUESTOR_WINDOW,
        41,
        36,
        1,
    ))
    .expect("write serialized self-owned XFixes event");
    xwm.drain_events(32)
        .expect("normalize self-owned XFixes event");
    xwm.flush().expect("flush queued selection-window requests");

    let selection = xwm
        .data_bridge
        .selections
        .current(super::super::data_bridge::SelectionKind::Clipboard)
        .expect("clipboard adapter state");
    assert_eq!(selection.owner, Some(TEST_CLIPBOARD_REQUESTOR_WINDOW));
    assert_eq!(
        selection.origin,
        Some(super::super::data_bridge::SelectionOrigin::Wayland)
    );
    assert_eq!(
        selection.targets_state,
        super::super::data_bridge::selection::TargetsDiscoveryState::Inactive
    );
    let requests = read_fixture_requests(&mut peer);
    assert!(convert_selection_requests(&requests).is_empty());
    assert!(
        fixture_request_opcodes(&requests)
            .iter()
            .all(|(opcode, _)| !matches!(*opcode, 1 | 4)),
        "self-owned notification must not rotate or destroy its owner window"
    );
}

#[test]
fn owner_none_clears_targets_and_pending_discovery() {
    let (mut xwm, mut peer) = test_fixture(generation(94));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );
    peer.write_all(&raw_clipboard_owner_event(0x305, 40, 35, 0))
        .expect("write owner event");
    xwm.drain_events(32).expect("start TARGETS conversion");
    let conversion = convert_selection_requests(&read_fixture_requests(&mut peer))
        .pop()
        .expect("TARGETS conversion");
    peer.write_all(&raw_selection_notify(
        conversion.requestor,
        conversion.selection,
        conversion.target,
        conversion.property,
        conversion.time,
        1,
    ))
    .expect("write SelectionNotify");
    xwm.drain_events(32)
        .expect("start asynchronous TARGETS property read");
    assert!(
        super::super::selection_wire::pending_sequence_for_test(
            &xwm,
            super::super::data_bridge::SelectionKind::Clipboard,
            true,
        )
        .is_some()
    );
    let _ = read_fixture_requests(&mut peer);

    peer.write_all(&raw_clipboard_owner_event(0, 41, 41, 2))
        .expect("write owner NONE event");
    xwm.drain_events(32).expect("clear selection wire state");

    let selection = xwm
        .data_bridge
        .selections
        .current(super::super::data_bridge::SelectionKind::Clipboard)
        .expect("selection record remains generation scoped");
    assert_eq!(selection.owner, None);
    assert!(selection.targets.is_empty());
    assert_eq!(
        selection.targets_state,
        super::super::data_bridge::selection::TargetsDiscoveryState::Idle
    );
    assert!(
        super::super::selection_wire::pending_sequence_for_test(
            &xwm,
            super::super::data_bridge::SelectionKind::Clipboard,
            true,
        )
        .is_none()
    );
}

#[test]
fn selection_notify_none_marks_conversion_failed_without_owner_loss() {
    let (mut xwm, mut peer) = test_fixture(generation(95));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );
    peer.write_all(&raw_clipboard_owner_event(0x306, 40, 35, 0))
        .expect("write owner event");
    xwm.drain_events(32).expect("start TARGETS conversion");
    let conversion = convert_selection_requests(&read_fixture_requests(&mut peer))
        .pop()
        .expect("TARGETS conversion");
    peer.write_all(&raw_selection_notify(
        conversion.requestor,
        conversion.selection,
        conversion.target,
        0,
        conversion.time,
        1,
    ))
    .expect("write failed SelectionNotify");
    xwm.drain_events(32)
        .expect("mark the current conversion failed");

    let selection = xwm
        .data_bridge
        .selections
        .current(super::super::data_bridge::SelectionKind::Clipboard)
        .expect("selection record");
    assert_eq!(selection.owner, Some(0x306));
    assert_eq!(
        selection.targets_state,
        super::super::data_bridge::selection::TargetsDiscoveryState::Failed
    );
    assert!(selection.targets.is_empty());
}

#[test]
fn stale_targets_reply_after_owner_replacement_is_ignored() {
    use super::super::data_bridge::{SelectionKind, selection::TargetsDiscoveryState};

    let (mut xwm, mut peer) = test_fixture(generation(96));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );
    peer.write_all(&raw_clipboard_owner_event(0x307, 40, 35, 0))
        .expect("write owner A event");
    xwm.drain_events(32).expect("start owner A conversion");
    let conversion_a = convert_selection_requests(&read_fixture_requests(&mut peer))
        .pop()
        .expect("owner A conversion");
    peer.write_all(&raw_selection_notify(
        conversion_a.requestor,
        conversion_a.selection,
        conversion_a.target,
        conversion_a.property,
        conversion_a.time,
        1,
    ))
    .expect("write owner A SelectionNotify");
    xwm.drain_events(32)
        .expect("start owner A GetProperty request");
    let sequence_a = super::super::selection_wire::pending_sequence_for_test(
        &xwm,
        SelectionKind::Clipboard,
        true,
    )
    .expect("owner A property reply sequence");
    let _ = read_fixture_requests(&mut peer);

    peer.write_all(&raw_clipboard_owner_event(0x308, 41, 35, 2))
        .expect("write owner B replacement with same timestamp");
    xwm.drain_events(32).expect("start owner B conversion");
    let conversion_b = convert_selection_requests(&read_fixture_requests(&mut peer))
        .pop()
        .expect("owner B conversion");
    assert_eq!(conversion_a.requestor, conversion_b.requestor);
    assert_eq!(
        xwm.data_bridge
            .selections
            .current(SelectionKind::Clipboard)
            .unwrap()
            .targets_state,
        TargetsDiscoveryState::AwaitingSelectionNotify
    );

    peer.write_all(&raw_selection_notify(
        conversion_b.requestor,
        conversion_b.selection,
        conversion_b.target,
        conversion_b.property,
        conversion_b.time,
        5,
    ))
    .expect("write owner B SelectionNotify");
    xwm.drain_events(32)
        .expect("start owner B GetProperty request");
    let _ = read_fixture_requests(&mut peer);
    peer.write_all(&raw_get_property_reply(
        sequence_a as u16,
        4,
        &[0x101, 0x102],
    ))
    .expect("write late owner A GetProperty reply");
    xwm.drain_events(32)
        .expect("discard stale owner A GetProperty completion");
    let selection_b = xwm
        .data_bridge
        .selections
        .current(SelectionKind::Clipboard)
        .expect("owner B selection record");
    assert_eq!(selection_b.owner, Some(0x308));
    assert_eq!(
        selection_b.targets_state,
        TargetsDiscoveryState::AwaitingProperty
    );
    assert!(selection_b.targets.is_empty());
}

#[test]
fn selection_bridge_windows_are_never_adopted_as_client_windows() {
    let (mut xwm, mut peer) = test_fixture(generation(97));
    for xid in [
        2,
        TEST_CLIPBOARD_OBSERVER_WINDOW,
        TEST_CLIPBOARD_REQUESTOR_WINDOW,
        TEST_PRIMARY_OBSERVER_WINDOW,
        TEST_PRIMARY_REQUESTOR_WINDOW,
    ] {
        peer.write_all(
            &xproto::CreateNotifyEvent {
                response_type: xproto::CREATE_NOTIFY_EVENT,
                sequence: 0,
                parent: 1,
                window: xid,
                x: 0,
                y: 0,
                width: 1,
                height: 1,
                border_width: 0,
                override_redirect: false,
            }
            .serialize(),
        )
        .expect("write internal window CreateNotify");
    }
    xwm.drain_events(32)
        .expect("normalize internal window events");

    for xid in [
        2,
        TEST_CLIPBOARD_OBSERVER_WINDOW,
        TEST_CLIPBOARD_REQUESTOR_WINDOW,
        TEST_PRIMARY_OBSERVER_WINDOW,
        TEST_PRIMARY_REQUESTOR_WINDOW,
    ] {
        assert!(
            !xwm.windows
                .contains(super::super::X11WindowHandle::new(generation(97), xid,)),
            "internal XID {xid:#x} entered the managed-window registry"
        );
    }
    assert!(xwm.take_events().next().is_none());
    let _ = read_fixture_requests(&mut peer);
}

#[test]
fn generation_cleanup_discards_selection_wire_state() {
    let (mut xwm, mut peer) = test_fixture(generation(102));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );
    peer.write_all(&raw_clipboard_owner_event(0x30b, 40, 35, 0))
        .expect("write owner event");
    xwm.drain_events(32).expect("start owner conversion");
    let conversion = convert_selection_requests(&read_fixture_requests(&mut peer))
        .pop()
        .expect("owner TARGETS conversion");
    peer.write_all(&raw_selection_notify(
        conversion.requestor,
        conversion.selection,
        conversion.target,
        conversion.property,
        conversion.time,
        1,
    ))
    .expect("write SelectionNotify");
    xwm.drain_events(32)
        .expect("start pending TARGETS property reply");
    assert!(
        super::super::selection_wire::pending_sequence_for_test(
            &xwm,
            super::super::data_bridge::SelectionKind::Clipboard,
            true,
        )
        .is_some()
    );
    let _ = read_fixture_requests(&mut peer);

    xwm.clear_generation(generation(102));

    assert!(!xwm.data_bridge.selection_wire.is_active());
    assert!(!super::super::selection_wire::is_internal_window(
        TEST_CLIPBOARD_OBSERVER_WINDOW,
        Some(xwm.supporting_wm_check),
        Some(&xwm.data_bridge.selection_wire),
    ));
    assert!(
        xwm.data_bridge
            .selections
            .current(super::super::data_bridge::SelectionKind::Clipboard)
            .is_none()
    );
    assert!(
        super::super::selection_wire::pending_sequence_for_test(
            &xwm,
            super::super::data_bridge::SelectionKind::Clipboard,
            true,
        )
        .is_none()
    );
}

#[test]
fn unrelated_selection_notify_is_ignored() {
    let (mut xwm, mut peer) = test_fixture(generation(98));
    peer.write_all(&raw_selection_notify(
        TEST_CLIPBOARD_REQUESTOR_WINDOW,
        0xdead,
        TEST_TARGETS_ATOM,
        TEST_SELECTION_TARGETS_PROPERTY,
        35,
        0,
    ))
    .expect("write unrelated SelectionNotify");
    xwm.drain_events(32)
        .expect("ignore unrelated SelectionNotify");
    assert_eq!(
        xwm.data_bridge
            .selections
            .current(super::super::data_bridge::SelectionKind::Clipboard)
            .unwrap()
            .owner,
        None
    );
    assert!(read_fixture_requests(&mut peer).is_empty());
}

#[test]
fn selection_owner_replacement_has_a_distinct_stale_reply_identity() {
    use super::super::data_bridge::{
        BridgeGeneration, SelectionKind, SelectionOrigin, selection::SelectionBridge,
    };

    let generation = BridgeGeneration::from(generation(91));
    let mut selections = SelectionBridge::default();
    selections.initialize_generation(generation);
    selections
        .observe_owner(
            generation,
            SelectionKind::Clipboard,
            Some(0x301),
            Some(SelectionOrigin::X11),
            35,
        )
        .expect("owner A revision");
    let owner_a = selections
        .current(SelectionKind::Clipboard)
        .and_then(super::super::data_bridge::selection::SelectionSnapshot::identity)
        .expect("owner A identity");
    selections
        .observe_owner(
            generation,
            SelectionKind::Clipboard,
            Some(0x302),
            Some(SelectionOrigin::X11),
            35,
        )
        .expect("owner B revision");
    let owner_b = selections
        .current(SelectionKind::Clipboard)
        .and_then(super::super::data_bridge::selection::SelectionSnapshot::identity)
        .expect("owner B identity");

    assert_ne!(
        owner_a, owner_b,
        "same-generation owners with identical TARGETS and timestamps still need distinct revisions"
    );
}

#[test]
fn normal_completed_owner_churn_does_not_exhaust_requestor_windows() {
    let (mut xwm, mut peer) = test_fixture(generation(112));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );

    let mut created_windows = 0;
    let mut conversions = Vec::new();
    let mut requestor = None;
    for index in 0..10_000_u32 {
        peer.write_all(&raw_clipboard_owner_event(
            0x400 + index,
            100 + index,
            50 + index,
            (index * 2) as u16,
        ))
        .expect("write owner replacement event");
        xwm.drain_events(8)
            .expect("process completed owner replacement");
        let requests = read_fixture_requests(&mut peer);
        for (opcode, _) in fixture_request_opcodes(&requests) {
            created_windows += usize::from(opcode == 1);
        }
        let conversion = convert_selection_requests(&requests)
            .pop()
            .expect("owner TARGETS conversion");
        if let Some(previous_requestor) = requestor {
            assert_eq!(conversion.requestor, previous_requestor);
        } else {
            requestor = Some(conversion.requestor);
        }

        peer.write_all(&raw_selection_notify(
            conversion.requestor,
            conversion.selection,
            conversion.target,
            conversion.property,
            conversion.time,
            (index * 2 + 1) as u16,
        ))
        .expect("write matching SelectionNotify");
        xwm.drain_events(8)
            .expect("start completed TARGETS property read");
        let property_sequence = super::super::selection_wire::pending_sequence_for_test(
            &xwm,
            super::super::data_bridge::SelectionKind::Clipboard,
            true,
        )
        .expect("TARGETS property sequence");
        let property_requests = read_fixture_requests(&mut peer);
        assert!(
            fixture_request_opcodes(&property_requests)
                .iter()
                .all(|(opcode, _)| !matches!(*opcode, 1 | 4))
        );

        peer.write_all(&raw_get_property_reply(
            property_sequence as u16,
            u32::from(xproto::AtomEnum::ATOM),
            &[TEST_TARGETS_ATOM],
        ))
        .expect("write matching TARGETS property reply");
        let drain = xwm
            .drain_events(32)
            .expect("resolve completed TARGETS property read");
        assert!(
            drain.selection_replies_processed > 0,
            "matching property reply was not processed: {drain:?}"
        );
        assert_eq!(
            xwm.data_bridge
                .selections
                .current(super::super::data_bridge::SelectionKind::Clipboard)
                .expect("current CLIPBOARD state")
                .targets_state,
            super::super::data_bridge::selection::TargetsDiscoveryState::Resolved
        );
        conversions.push(conversion);
    }

    let selection = xwm
        .data_bridge
        .selections
        .current(super::super::data_bridge::SelectionKind::Clipboard)
        .expect("current CLIPBOARD wire state");
    assert_eq!(created_windows, 0);
    assert_eq!(conversions.len(), 10_000);
    assert!(
        conversions
            .iter()
            .all(|request| request.requestor == TEST_CLIPBOARD_REQUESTOR_WINDOW)
    );
    assert_eq!(selection.owner, Some(0x400 + 9_999));
    assert_eq!(
        selection.targets_state,
        super::super::data_bridge::selection::TargetsDiscoveryState::Resolved
    );
    assert!(super::super::selection_wire::is_internal_window(
        TEST_CLIPBOARD_REQUESTOR_WINDOW,
        Some(xwm.supporting_wm_check),
        Some(&xwm.data_bridge.selection_wire),
    ));
}

#[test]
fn rotation_bound_poison_blocks_distinguishable_timestamp_reuse() {
    use super::super::data_bridge::{SelectionKind, selection::TargetsDiscoveryState};

    let (mut xwm, mut peer) = test_fixture(generation(125));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );
    peer.write_all(&raw_clipboard_owner_event(0x335, 40, 500, 0))
        .expect("write owner A event");
    xwm.drain_events(32).expect("start owner A conversion");
    let _ = convert_selection_requests(&read_fixture_requests(&mut peer));
    xwm.data_bridge
        .selection_wire
        .exhaust_requestor_window_budget_for_test(SelectionKind::Clipboard);

    peer.write_all(&raw_clipboard_owner_event(0x336, 41, 500, 1))
        .expect("write ambiguous owner B event at exhausted rotation bound");
    xwm.drain_events(32)
        .expect("fail unsafe owner B conversion without aliasing");
    let failed_requests = read_fixture_requests(&mut peer);
    assert!(convert_selection_requests(&failed_requests).is_empty());
    assert!(
        fixture_request_opcodes(&failed_requests)
            .iter()
            .all(|(opcode, _)| !matches!(*opcode, 1 | 4))
    );
    let failed = xwm
        .data_bridge
        .selections
        .current(SelectionKind::Clipboard)
        .expect("failed owner B state");
    assert_eq!(failed.owner, Some(0x336));
    assert_eq!(failed.targets_state, TargetsDiscoveryState::Failed);

    peer.write_all(&raw_clipboard_owner_event(0x337, 42, 501, 2))
        .expect("write owner C event with a distinguishable timestamp");
    xwm.drain_events(32)
        .expect("keep the contaminated requestor blocked");
    assert!(convert_selection_requests(&read_fixture_requests(&mut peer)).is_empty());
    assert_eq!(
        xwm.data_bridge
            .selections
            .current(SelectionKind::Clipboard)
            .unwrap()
            .targets_state,
        TargetsDiscoveryState::Failed
    );
    assert!(
        xwm.data_bridge
            .selection_wire
            .requestor_poisoned_for_test(SelectionKind::Clipboard)
    );
}

#[test]
fn rotation_bound_poison_blocks_same_timestamp_reuse() {
    use super::super::data_bridge::{SelectionKind, selection::TargetsDiscoveryState};
    let (mut xwm, mut peer) = test_fixture(generation(128));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );
    peer.write_all(&raw_clipboard_owner_event(0x340, 40, 500, 0))
        .unwrap();
    xwm.drain_events(32).unwrap();
    let conversion_a = convert_selection_requests(&read_fixture_requests(&mut peer))
        .pop()
        .unwrap();
    xwm.data_bridge
        .selection_wire
        .exhaust_requestor_window_budget_for_test(SelectionKind::Clipboard);
    peer.write_all(&raw_clipboard_owner_event(0x341, 41, 500, 1))
        .unwrap();
    xwm.drain_events(32).unwrap();
    assert!(convert_selection_requests(&read_fixture_requests(&mut peer)).is_empty());
    peer.write_all(&raw_clipboard_owner_event(0x342, 42, 500, 2))
        .unwrap();
    xwm.drain_events(32).unwrap();
    let requests = read_fixture_requests(&mut peer);
    assert!(convert_selection_requests(&requests).is_empty());
    peer.write_all(&raw_selection_notify(
        conversion_a.requestor,
        conversion_a.selection,
        conversion_a.target,
        conversion_a.property,
        conversion_a.time,
        3,
    ))
    .unwrap();
    xwm.drain_events(32).unwrap();
    assert!(
        super::super::selection_wire::pending_sequence_for_test(
            &xwm,
            SelectionKind::Clipboard,
            true
        )
        .is_none()
    );
    assert_eq!(
        xwm.data_bridge
            .selections
            .current(SelectionKind::Clipboard)
            .unwrap()
            .targets_state,
        TargetsDiscoveryState::Failed
    );
    assert!(
        xwm.data_bridge
            .selection_wire
            .requestor_poisoned_for_test(SelectionKind::Clipboard)
    );
}

#[test]
fn rotation_bound_poison_survives_owner_none() {
    use super::super::data_bridge::{SelectionKind, selection::TargetsDiscoveryState};
    let (mut xwm, mut peer) = test_fixture(generation(129));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );
    peer.write_all(&raw_clipboard_owner_event(0x343, 40, 500, 0))
        .unwrap();
    xwm.drain_events(32).unwrap();
    let conversion_a = convert_selection_requests(&read_fixture_requests(&mut peer))
        .pop()
        .unwrap();
    xwm.data_bridge
        .selection_wire
        .exhaust_requestor_window_budget_for_test(SelectionKind::Clipboard);
    peer.write_all(&raw_xfixes_selection_event(
        TEST_CLIPBOARD_ATOM,
        TEST_CLIPBOARD_OBSERVER_WINDOW,
        xfixes::SelectionEvent::SELECTION_WINDOW_DESTROY,
        0,
        41,
        500,
        1,
    ))
    .unwrap();
    xwm.drain_events(32).unwrap();
    assert!(convert_selection_requests(&read_fixture_requests(&mut peer)).is_empty());
    peer.write_all(&raw_clipboard_owner_event(0x344, 42, 500, 2))
        .unwrap();
    xwm.drain_events(32).unwrap();
    let requests = read_fixture_requests(&mut peer);
    assert!(convert_selection_requests(&requests).is_empty());
    peer.write_all(&raw_selection_notify(
        conversion_a.requestor,
        conversion_a.selection,
        conversion_a.target,
        conversion_a.property,
        conversion_a.time,
        3,
    ))
    .unwrap();
    xwm.drain_events(32).unwrap();
    assert!(
        super::super::selection_wire::pending_sequence_for_test(
            &xwm,
            SelectionKind::Clipboard,
            true
        )
        .is_none()
    );
    assert_eq!(
        xwm.data_bridge
            .selections
            .current(SelectionKind::Clipboard)
            .unwrap()
            .owner,
        Some(0x344)
    );
    assert_eq!(
        xwm.data_bridge
            .selections
            .current(SelectionKind::Clipboard)
            .unwrap()
            .targets_state,
        TargetsDiscoveryState::Failed
    );
    assert!(
        xwm.data_bridge
            .selection_wire
            .requestor_poisoned_for_test(SelectionKind::Clipboard)
    );
}

#[test]
fn unresolved_owner_churn_is_bounded_after_poison() {
    use super::super::data_bridge::SelectionKind;
    let (mut xwm, mut peer) = test_fixture(generation(130));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );
    peer.write_all(&raw_clipboard_owner_event(0x345, 40, 500, 0))
        .unwrap();
    xwm.drain_events(32).unwrap();
    let _ = read_fixture_requests(&mut peer);
    xwm.data_bridge
        .selection_wire
        .exhaust_requestor_window_budget_for_test(SelectionKind::Clipboard);
    for index in 0..32_u32 {
        peer.write_all(&raw_clipboard_owner_event(
            0x346 + index,
            41 + index,
            500,
            (index + 1) as u16,
        ))
        .unwrap();
        xwm.drain_events(32).unwrap();
        let requests = read_fixture_requests(&mut peer);
        assert!(convert_selection_requests(&requests).is_empty());
        assert!(
            fixture_request_opcodes(&requests)
                .iter()
                .all(|(opcode, _)| !matches!(*opcode, 1 | 4))
        );
    }
    assert!(
        xwm.data_bridge
            .selection_wire
            .requestor_poisoned_for_test(SelectionKind::Clipboard)
    );
}

#[test]
fn rotation_bound_poison_blocks_zero_timestamp_reuse() {
    use super::super::data_bridge::SelectionKind;
    let (mut xwm, mut peer) = test_fixture(generation(131));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );
    peer.write_all(&raw_clipboard_owner_event(0x366, 40, 500, 0))
        .unwrap();
    xwm.drain_events(32).unwrap();
    let _ = convert_selection_requests(&read_fixture_requests(&mut peer));
    xwm.data_bridge
        .selection_wire
        .exhaust_requestor_window_budget_for_test(SelectionKind::Clipboard);
    peer.write_all(&raw_clipboard_owner_event(0x367, 41, 0, 1))
        .unwrap();
    xwm.drain_events(32).unwrap();
    assert!(convert_selection_requests(&read_fixture_requests(&mut peer)).is_empty());
    peer.write_all(&raw_clipboard_owner_event(0x368, 42, 0, 2))
        .unwrap();
    xwm.drain_events(32).unwrap();
    assert!(convert_selection_requests(&read_fixture_requests(&mut peer)).is_empty());
    assert!(
        xwm.data_bridge
            .selection_wire
            .requestor_poisoned_for_test(SelectionKind::Clipboard)
    );
}

#[test]
fn poisoned_clipboard_does_not_block_primary() {
    use super::super::data_bridge::SelectionKind;
    let (mut xwm, mut peer) = test_fixture(generation(132));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );
    peer.write_all(&raw_clipboard_owner_event(0x369, 40, 500, 0))
        .unwrap();
    xwm.drain_events(32).unwrap();
    let _ = read_fixture_requests(&mut peer);
    xwm.data_bridge
        .selection_wire
        .exhaust_requestor_window_budget_for_test(SelectionKind::Clipboard);
    peer.write_all(&raw_clipboard_owner_event(0x36a, 41, 500, 1))
        .unwrap();
    xwm.drain_events(32).unwrap();
    assert!(convert_selection_requests(&read_fixture_requests(&mut peer)).is_empty());
    peer.write_all(&raw_selection_owner_event(
        TEST_PRIMARY_ATOM,
        TEST_PRIMARY_OBSERVER_WINDOW,
        0x36b,
        42,
        500,
        2,
    ))
    .unwrap();
    xwm.drain_events(32).unwrap();
    let primary = convert_selection_requests(&read_fixture_requests(&mut peer))
        .pop()
        .unwrap();
    assert_eq!(primary.selection, TEST_PRIMARY_ATOM);
    assert_eq!(primary.requestor, TEST_PRIMARY_REQUESTOR_WINDOW);
    assert!(
        xwm.data_bridge
            .selection_wire
            .requestor_poisoned_for_test(SelectionKind::Clipboard)
    );
    assert!(
        !xwm.data_bridge
            .selection_wire
            .requestor_poisoned_for_test(SelectionKind::Primary)
    );
}

#[test]
fn internal_owner_does_not_erase_requestor_poison() {
    use super::super::data_bridge::{
        SelectionKind, SelectionOrigin, selection::TargetsDiscoveryState,
    };
    let (mut xwm, mut peer) = test_fixture(generation(133));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );
    peer.write_all(&raw_clipboard_owner_event(0x36c, 40, 500, 0))
        .unwrap();
    xwm.drain_events(32).unwrap();
    let _ = read_fixture_requests(&mut peer);
    xwm.data_bridge
        .selection_wire
        .exhaust_requestor_window_budget_for_test(SelectionKind::Clipboard);
    peer.write_all(&raw_clipboard_owner_event(0x36d, 41, 500, 1))
        .unwrap();
    xwm.drain_events(32).unwrap();
    let _ = read_fixture_requests(&mut peer);
    peer.write_all(&raw_clipboard_owner_event(
        TEST_CLIPBOARD_REQUESTOR_WINDOW,
        42,
        501,
        2,
    ))
    .unwrap();
    xwm.drain_events(32).unwrap();
    assert!(convert_selection_requests(&read_fixture_requests(&mut peer)).is_empty());
    let selection = xwm
        .data_bridge
        .selections
        .current(SelectionKind::Clipboard)
        .unwrap();
    assert_eq!(selection.origin, Some(SelectionOrigin::Wayland));
    assert_eq!(selection.targets_state, TargetsDiscoveryState::Inactive);
    assert!(
        xwm.data_bridge
            .selection_wire
            .requestor_poisoned_for_test(SelectionKind::Clipboard)
    );
}

#[test]
fn successful_requestor_replacement_clears_poison() {
    use super::super::data_bridge::SelectionKind;

    let (mut xwm, mut peer) = test_fixture(generation(138));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );
    peer.write_all(&raw_clipboard_owner_event(0x370, 40, 500, 0))
        .unwrap();
    xwm.drain_events(32).unwrap();
    let _ = read_fixture_requests(&mut peer);

    peer.write_all(&raw_clipboard_owner_event(
        TEST_CLIPBOARD_REQUESTOR_WINDOW,
        41,
        501,
        1,
    ))
    .unwrap();
    xwm.drain_events(32).unwrap();
    assert!(convert_selection_requests(&read_fixture_requests(&mut peer)).is_empty());
    assert!(
        xwm.data_bridge
            .selection_wire
            .requestor_poisoned_for_test(SelectionKind::Clipboard)
    );

    peer.write_all(&raw_xfixes_selection_event(
        TEST_CLIPBOARD_ATOM,
        TEST_CLIPBOARD_OBSERVER_WINDOW,
        xfixes::SelectionEvent::SELECTION_WINDOW_DESTROY,
        0,
        42,
        501,
        2,
    ))
    .unwrap();
    xwm.drain_events(32).unwrap();
    xwm.flush().unwrap();
    let requests = read_fixture_requests(&mut peer);
    assert!(
        fixture_request_opcodes(&requests)
            .iter()
            .any(|(opcode, _)| *opcode == 1)
    );
    assert!(
        !xwm.data_bridge
            .selection_wire
            .requestor_poisoned_for_test(SelectionKind::Clipboard)
    );
}

#[test]
fn generation_teardown_clears_requestor_poison() {
    use super::super::data_bridge::SelectionKind;
    let generation = generation(134);
    let (mut xwm, mut peer) = test_fixture(generation);
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );
    peer.write_all(&raw_clipboard_owner_event(0x36e, 40, 500, 0))
        .unwrap();
    xwm.drain_events(32).unwrap();
    let _ = read_fixture_requests(&mut peer);
    xwm.data_bridge
        .selection_wire
        .exhaust_requestor_window_budget_for_test(SelectionKind::Clipboard);
    peer.write_all(&raw_clipboard_owner_event(0x36f, 41, 500, 1))
        .unwrap();
    xwm.drain_events(32).unwrap();
    let _ = read_fixture_requests(&mut peer);
    assert!(
        xwm.data_bridge
            .selection_wire
            .requestor_poisoned_for_test(SelectionKind::Clipboard)
    );
    xwm.clear_generation(generation);
    assert!(
        !xwm.data_bridge
            .selection_wire
            .requestor_poisoned_for_test(SelectionKind::Clipboard)
    );
    assert!(!xwm.data_bridge.selection_wire.is_active());
}

#[test]
fn new_generation_starts_with_clean_requestors() {
    use super::super::data_bridge::SelectionKind;

    let old_generation = generation(136);
    let (mut old_xwm, mut peer) = test_fixture(old_generation);
    install_extension(
        &mut old_xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );
    peer.write_all(&raw_clipboard_owner_event(0x370, 40, 500, 0))
        .unwrap();
    old_xwm.drain_events(32).unwrap();
    let _ = read_fixture_requests(&mut peer);
    old_xwm
        .data_bridge
        .selection_wire
        .exhaust_requestor_window_budget_for_test(SelectionKind::Clipboard);
    peer.write_all(&raw_clipboard_owner_event(0x371, 41, 500, 1))
        .unwrap();
    old_xwm.drain_events(32).unwrap();
    assert!(
        old_xwm
            .data_bridge
            .selection_wire
            .requestor_poisoned_for_test(SelectionKind::Clipboard)
    );
    old_xwm.clear_generation(old_generation);

    let (new_xwm, _peer) = test_fixture(generation(137));
    assert!(
        !new_xwm
            .data_bridge
            .selection_wire
            .requestor_poisoned_for_test(SelectionKind::Clipboard)
    );
    assert!(
        !new_xwm
            .data_bridge
            .selection_wire
            .requestor_poisoned_for_test(SelectionKind::Primary)
    );
}

#[path = "selection_payload_regression_tests.rs"]
mod selection_payload_regression_tests;
