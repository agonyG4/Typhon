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
    xfixes::SelectionNotifyEvent {
        response_type: TEST_XFIXES_FIRST_EVENT + xfixes::SELECTION_NOTIFY_EVENT,
        subtype: xfixes::SelectionEvent::SET_SELECTION_OWNER,
        sequence,
        window: observer,
        owner,
        selection,
        timestamp,
        selection_timestamp,
    }
    .serialize()
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
    assert_ne!(conversion_a.requestor, conversion_b.requestor);

    let stale_reply = raw_get_property_reply(sequence_a as u16, 4, &[0x101, 0x102]);
    peer.write_all(&stale_reply)
        .expect("write late owner A GetProperty reply");
    xwm.drain_events(32)
        .expect("discard stale owner A property completion");
    let selection_b = xwm
        .data_bridge
        .selections
        .current(SelectionKind::Clipboard)
        .expect("owner B selection record");
    assert_eq!(selection_b.owner, Some(0x308));
    assert_eq!(
        selection_b.targets_state,
        TargetsDiscoveryState::AwaitingSelectionNotify
    );
    assert!(selection_b.targets.is_empty());

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
    let sequence_b = super::super::selection_wire::pending_sequence_for_test(
        &xwm,
        SelectionKind::Clipboard,
        true,
    )
    .expect("owner B property reply sequence");
    let _ = read_fixture_requests(&mut peer);
    peer.write_all(&raw_get_property_reply(
        sequence_b as u16,
        4,
        &[0x101, 0x102, 0x101],
    ))
    .expect("write matching owner B GetProperty reply");
    xwm.drain_events(32).expect("resolve owner B TARGETS reply");
    let selection_b = xwm
        .data_bridge
        .selections
        .current(SelectionKind::Clipboard)
        .expect("owner B selection record");
    assert_eq!(selection_b.owner, Some(0x308));
    assert_eq!(selection_b.targets_state, TargetsDiscoveryState::Resolved);
    assert_eq!(selection_b.targets, vec![0x101, 0x102]);
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
fn owner_churn_has_a_hard_requestor_window_creation_bound() {
    let (mut xwm, mut peer) = test_fixture(generation(112));
    install_extension(
        &mut xwm,
        xfixes::X11_EXTENSION_NAME,
        201,
        TEST_XFIXES_FIRST_EVENT,
        151,
    );

    let mut created_windows = 0;
    let mut conversions = 0;
    for index in 0..5_000_u32 {
        peer.write_all(&raw_clipboard_owner_event(
            0x400 + index,
            100 + index,
            50 + index,
            index as u16,
        ))
        .expect("write owner replacement event");
        xwm.drain_events(8)
            .expect("process bounded owner replacement");
        for (opcode, _) in fixture_request_opcodes(&read_fixture_requests(&mut peer)) {
            created_windows += usize::from(opcode == 1);
            conversions += usize::from(opcode == 24);
        }
    }

    let selection = xwm
        .data_bridge
        .selections
        .current(super::super::data_bridge::SelectionKind::Clipboard)
        .expect("current CLIPBOARD wire state");
    assert_eq!(created_windows, 4_095);
    assert_eq!(conversions, 4_096);
    assert_eq!(selection.owner, Some(0x400 + 4_999));
    assert_eq!(
        selection.targets_state,
        super::super::data_bridge::selection::TargetsDiscoveryState::Failed
    );
    assert!(super::super::selection_wire::is_internal_window(
        TEST_CLIPBOARD_REQUESTOR_WINDOW,
        Some(xwm.supporting_wm_check),
        Some(&xwm.data_bridge.selection_wire),
    ));
}
