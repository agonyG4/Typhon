//! Regression tests for reverse selection ownership and request serving.

use std::{
    fs::File,
    io::Write,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::net::UnixStream,
    },
};

use crate::xwayland::{XwaylandProxySelectionId, XwaylandSelectionKind};
use x11rb::{
    connection::Connection,
    protocol::{Event, xproto},
    x11_utils::Serialize,
};

use super::{finish_proxy_catalog, generation, initialize_selection_wire, test_fixture};

const REQUESTOR: u32 = 0xf001;
const TARGET_PROPERTY: u32 = 0xf002;
const MULTIPLE_PROPERTY: u32 = 0xf003;

#[derive(Debug, PartialEq, Eq)]
struct PropertyChange {
    window: u32,
    property: u32,
    property_type: u32,
    mode: u8,
    format: u8,
    value: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
struct Notify {
    time: u32,
    requestor: u32,
    selection: u32,
    target: u32,
    property: u32,
}

fn word(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("four byte field"),
    )
}

fn fixture_requests(peer: &mut UnixStream) -> (Vec<PropertyChange>, Vec<Notify>, Vec<u8>) {
    let bytes = super::read_fixture_requests(peer);
    let mut changes = Vec::new();
    let mut notifies = Vec::new();
    let mut opcodes = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        let byte_len = usize::from(u16::from_le_bytes([bytes[offset + 2], bytes[offset + 3]])) * 4;
        assert!(byte_len >= 4 && offset + byte_len <= bytes.len());
        let request = &bytes[offset..offset + byte_len];
        opcodes.push(request[0]);
        if request[0] == xproto::CHANGE_PROPERTY_REQUEST {
            let format = request[16];
            let units = word(request, 20) as usize;
            let value_len = units * usize::from(format / 8);
            changes.push(PropertyChange {
                window: word(request, 4),
                property: word(request, 8),
                property_type: word(request, 12),
                mode: request[1],
                format,
                value: request[24..24 + value_len].to_vec(),
            });
        } else if request[0] == xproto::SEND_EVENT_REQUEST
            && request[12] & 0x7f == xproto::SELECTION_NOTIFY_EVENT
        {
            let event = &request[12..44];
            notifies.push(Notify {
                time: word(event, 4),
                requestor: word(event, 8),
                selection: word(event, 12),
                target: word(event, 16),
                property: word(event, 20),
            });
        }
        offset += byte_len;
    }
    (changes, notifies, opcodes)
}

fn values32(bytes: &[u8]) -> Vec<u32> {
    bytes.chunks_exact(4).map(|chunk| word(chunk, 0)).collect()
}

fn proxy_fixture(
    kind: XwaylandSelectionKind,
) -> (
    super::super::super::Xwm,
    UnixStream,
    XwaylandProxySelectionId,
    u32,
) {
    proxy_fixture_with_authority(kind, true)
}

fn proxy_fixture_without_authority(
    kind: XwaylandSelectionKind,
) -> (
    super::super::super::Xwm,
    UnixStream,
    XwaylandProxySelectionId,
    u32,
) {
    proxy_fixture_with_authority(kind, false)
}

fn proxy_fixture_with_authority(
    kind: XwaylandSelectionKind,
    install_authority: bool,
) -> (
    super::super::super::Xwm,
    UnixStream,
    XwaylandProxySelectionId,
    u32,
) {
    let (mut xwm, mut peer) = test_fixture(generation(301));
    initialize_selection_wire(&mut xwm, &mut peer);
    let id = XwaylandProxySelectionId {
        kind,
        selection_generation: 17,
        source_key: crate::compositor::SelectionSourceKey(1701),
    };
    let mime_types = vec!["image/png".to_owned()];
    super::super::super::selection_wire::submit_proxy_selection_snapshots(
        &mut xwm,
        [super::proxy_snapshot(kind, 17, 1701, &["image/png"])],
    )
    .expect("submit proxy catalog");
    finish_proxy_catalog(&mut xwm, &mut peer, id, &mime_types);
    let selection_kind = match kind {
        XwaylandSelectionKind::Clipboard => {
            super::super::super::data_bridge::SelectionKind::Clipboard
        }
        XwaylandSelectionKind::Primary => super::super::super::data_bridge::SelectionKind::Primary,
    };
    if install_authority {
        let _ = super::read_fixture_requests(&mut peer);
        super::super::super::selection_proxy::disable_claim_for_test(&mut xwm, selection_kind);
    }
    let prepared = super::super::super::selection_wire::prepared_proxy_selection_for_test(
        &xwm,
        selection_kind,
    )
    .expect("prepared catalog");
    let target = prepared.data_targets[0].target;
    if install_authority {
        assert!(
            super::super::super::selection_proxy::install_test_authority(
                &mut xwm,
                selection_kind,
                id,
                12_345,
            )
        );
    }
    (xwm, peer, id, target)
}

fn raw_selection_request(event: &xproto::SelectionRequestEvent) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    bytes[0] = xproto::SELECTION_REQUEST_EVENT;
    bytes[2..4].copy_from_slice(&event.sequence.to_ne_bytes());
    bytes[4..8].copy_from_slice(&event.time.to_ne_bytes());
    bytes[8..12].copy_from_slice(&event.owner.to_ne_bytes());
    bytes[12..16].copy_from_slice(&event.requestor.to_ne_bytes());
    bytes[16..20].copy_from_slice(&event.selection.to_ne_bytes());
    bytes[20..24].copy_from_slice(&event.target.to_ne_bytes());
    bytes[24..28].copy_from_slice(&event.property.to_ne_bytes());
    bytes
}

fn raw_selection_clear(event: &xproto::SelectionClearEvent) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    bytes[0] = xproto::SELECTION_CLEAR_EVENT;
    bytes[2..4].copy_from_slice(&event.sequence.to_ne_bytes());
    bytes[4..8].copy_from_slice(&event.time.to_ne_bytes());
    bytes[8..12].copy_from_slice(&event.owner.to_ne_bytes());
    bytes[12..16].copy_from_slice(&event.selection.to_ne_bytes());
    bytes
}

fn raw_proxy_time_notify(owner: u32, atom: u32, timestamp: u32, sequence: u16) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    bytes[0] = xproto::PROPERTY_NOTIFY_EVENT;
    bytes[2..4].copy_from_slice(&sequence.to_ne_bytes());
    bytes[4..8].copy_from_slice(&owner.to_ne_bytes());
    bytes[8..12].copy_from_slice(&atom.to_ne_bytes());
    bytes[12..16].copy_from_slice(&timestamp.to_ne_bytes());
    bytes[16] = u8::from(xproto::Property::NEW_VALUE);
    bytes
}

fn take_owner_requests(peer: &mut UnixStream) -> (Vec<(u32, u32, u32)>, usize) {
    owner_requests(&super::read_fixture_requests(peer))
}

fn owner_requests(bytes: &[u8]) -> (Vec<(u32, u32, u32)>, usize) {
    let mut claims = Vec::new();
    let mut confirmations = 0;
    let mut offset = 0;
    while offset < bytes.len() {
        let byte_len = usize::from(u16::from_ne_bytes([bytes[offset + 2], bytes[offset + 3]])) * 4;
        assert!(byte_len >= 4 && offset + byte_len <= bytes.len());
        let request = &bytes[offset..offset + byte_len];
        if request[0] == xproto::SET_SELECTION_OWNER_REQUEST {
            claims.push((word(request, 4), word(request, 8), word(request, 12)));
        } else if request[0] == xproto::GET_SELECTION_OWNER_REQUEST {
            confirmations += 1;
        }
        offset += byte_len;
    }
    (claims, confirmations)
}

fn destroyed_windows(bytes: &[u8]) -> Vec<u32> {
    let mut windows = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        let byte_len = usize::from(u16::from_ne_bytes([bytes[offset + 2], bytes[offset + 3]])) * 4;
        assert!(byte_len >= 4 && offset + byte_len <= bytes.len());
        let request = &bytes[offset..offset + byte_len];
        if request[0] == xproto::DESTROY_WINDOW_REQUEST {
            windows.push(word(request, 4));
        }
        offset += byte_len;
    }
    windows
}

fn proxy_owner_event_masks(bytes: &[u8]) -> Vec<(u32, u32)> {
    let mut owners = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        let byte_len = usize::from(u16::from_ne_bytes([bytes[offset + 2], bytes[offset + 3]])) * 4;
        assert!(byte_len >= 4 && offset + byte_len <= bytes.len());
        let request = &bytes[offset..offset + byte_len];
        if request[0] == xproto::CREATE_WINDOW_REQUEST {
            let value_mask = word(request, 28);
            let event_mask_bit = 1_u32 << 11;
            if value_mask & event_mask_bit != 0 {
                let preceding_values = (value_mask & (event_mask_bit - 1)).count_ones() as usize;
                owners.push((word(request, 4), word(request, 32 + preceding_values * 4)));
            }
        }
        offset += byte_len;
    }
    owners
}

fn inject_proxy_time(
    xwm: &mut super::super::super::Xwm,
    peer: &mut UnixStream,
    kind: XwaylandSelectionKind,
    timestamp: u32,
) {
    let selection_kind = match kind {
        XwaylandSelectionKind::Clipboard => {
            super::super::super::data_bridge::SelectionKind::Clipboard
        }
        XwaylandSelectionKind::Primary => super::super::super::data_bridge::SelectionKind::Primary,
    };
    let owner = xwm
        .data_bridge
        .selection_proxy
        .owner_window(selection_kind)
        .expect("private proxy owner window");
    let property = xwm
        .atoms
        .get(super::super::super::atoms::XwmAtomName::SelectionProxyTime);
    let sequence = super::super::super::selection_proxy::timestamp_probe_sequence_for_test(
        xwm,
        selection_kind,
    )
    .expect("one timestamp probe remains outstanding");
    peer.write_all(&raw_proxy_time_notify(
        owner,
        property,
        timestamp,
        sequence as u16,
    ))
    .expect("write serialized proxy timestamp PropertyNotify");
    xwm.drain_events(64)
        .expect("route proxy timestamp PropertyNotify");
}

fn confirm_proxy_ownership(
    xwm: &mut super::super::super::Xwm,
    peer: &mut UnixStream,
    kind: XwaylandSelectionKind,
    id: XwaylandProxySelectionId,
    timestamp: u32,
) -> u16 {
    let selection_kind = match kind {
        XwaylandSelectionKind::Clipboard => {
            super::super::super::data_bridge::SelectionKind::Clipboard
        }
        XwaylandSelectionKind::Primary => super::super::super::data_bridge::SelectionKind::Primary,
    };
    let owner = xwm
        .data_bridge
        .selection_proxy
        .owner_window(selection_kind)
        .expect("private proxy owner window");
    inject_proxy_time(xwm, peer, kind, timestamp);
    let (claims, confirmations) = take_owner_requests(peer);
    assert_eq!(claims, [(owner, selection_atom(xwm, kind), timestamp)]);
    assert_eq!(confirmations, 1);
    assert_eq!(
        super::super::super::selection_proxy::authority_for_test(xwm, selection_kind),
        None,
        "serving authority stays absent until GetSelectionOwner returns"
    );
    let (pending_id, pending_timestamp, sequence) =
        super::super::super::selection_proxy::pending_owner_confirmation_for_test(
            xwm,
            selection_kind,
        )
        .expect("asynchronous owner confirmation is pending");
    assert_eq!(pending_id, id);
    assert_eq!(pending_timestamp, timestamp);
    peer.write_all(&super::raw_get_selection_owner_reply(
        sequence as u16,
        owner,
    ))
    .expect("write serialized GetSelectionOwner reply");
    xwm.drain_events(64)
        .expect("confirm proxy ownership without blocking");
    sequence as u16
}

fn normalize_proxy_destroy(xwm: &mut super::super::super::Xwm, owner: u32) {
    super::super::super::events::normalize(
        xwm,
        Event::DestroyNotify(xproto::DestroyNotifyEvent {
            response_type: xproto::DESTROY_NOTIFY_EVENT,
            sequence: 0,
            event: owner,
            window: owner,
        }),
    )
    .expect("route proxy owner DestroyNotify through production normalization");
}

#[test]
fn same_timestamp_external_takeover_cannot_be_clobbered_by_proxy_release() {
    use super::super::super::data_bridge::SelectionKind;

    let (mut xwm, mut peer, id, _) =
        proxy_fixture_without_authority(XwaylandSelectionKind::Clipboard);
    let _ = fixture_requests(&mut peer);
    let timestamp = 0x1234_5001;
    let confirmation_sequence = confirm_proxy_ownership(
        &mut xwm,
        &mut peer,
        XwaylandSelectionKind::Clipboard,
        id,
        timestamp,
    );
    let _ = fixture_requests(&mut peer);

    let selection = selection_atom(&xwm, XwaylandSelectionKind::Clipboard);
    let owner = xwm
        .data_bridge
        .selection_proxy
        .owner_window(SelectionKind::Clipboard)
        .expect("private proxy owner window");
    let observer = xwm
        .data_bridge
        .selection_wire
        .observer_window_for_test(SelectionKind::Clipboard)
        .expect("Clipboard XFixes observer");
    peer.write_all(&super::raw_xfixes_selection_event(
        selection,
        observer,
        x11rb::protocol::xfixes::SelectionEvent::SET_SELECTION_OWNER,
        0xbeef,
        timestamp,
        timestamp,
        confirmation_sequence,
    ))
    .expect("queue equal-timestamp external owner event without processing it yet");

    super::super::super::selection_wire::submit_proxy_selection_snapshots(
        &mut xwm,
        [super::proxy_clear_snapshot(
            XwaylandSelectionKind::Clipboard,
            18,
        )],
    )
    .expect("canonical clear withdraws proxy ownership");
    let requests = super::read_fixture_requests(&mut peer);
    let (claims, _) = owner_requests(&requests);
    assert!(
        !claims.iter().any(|(claim_owner, claim_selection, _)| {
            *claim_owner == x11rb::NONE && *claim_selection == selection
        }),
        "release must not use SetSelectionOwner(None, T), since equal-timestamp takeover is legal"
    );
    assert!(
        destroyed_windows(&requests).contains(&owner),
        "the exact Typhon owner XID is the identity-safe release primitive"
    );
}

#[test]
fn proxy_owner_window_subscribes_to_structure_notify() {
    let (mut xwm, mut peer) = test_fixture(generation(302));
    super::super::super::selection_proxy::initialize(&mut xwm)
        .expect("initialize private proxy owner windows");
    xwm.connection
        .flush()
        .expect("flush proxy owner creation requests");
    let clipboard_owner = xwm
        .data_bridge
        .selection_proxy
        .owner_window(super::super::super::data_bridge::SelectionKind::Clipboard)
        .expect("Clipboard proxy owner");
    let primary_owner = xwm
        .data_bridge
        .selection_proxy
        .owner_window(super::super::super::data_bridge::SelectionKind::Primary)
        .expect("Primary proxy owner");
    let masks = proxy_owner_event_masks(&super::read_fixture_requests(&mut peer));
    for owner in [clipboard_owner, primary_owner] {
        let event_mask = masks
            .iter()
            .find_map(|(window, mask)| (*window == owner).then_some(*mask))
            .expect("serialized CreateWindow request for proxy owner");
        assert_ne!(
            event_mask & xproto::EventMask::PROPERTY_CHANGE.bits(),
            0,
            "proxy owner needs PropertyNotify for timestamp acquisition"
        );
        assert_ne!(
            event_mask & xproto::EventMask::STRUCTURE_NOTIFY.bits(),
            0,
            "proxy owner needs reliable DestroyNotify delivery for retirement"
        );
    }
}

#[test]
fn ready_proxy_begins_timestamp_probe_without_claiming_ownership() {
    let (xwm, mut peer, _, _) = proxy_fixture_without_authority(XwaylandSelectionKind::Clipboard);
    let owner = xwm
        .data_bridge
        .selection_proxy
        .owner_window(super::super::super::data_bridge::SelectionKind::Clipboard)
        .expect("private proxy owner window");
    let (changes, _, opcodes) = fixture_requests(&mut peer);

    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].window, owner);
    assert_eq!(changes[0].mode, u8::from(xproto::PropMode::APPEND));
    assert!(changes[0].value.is_empty());
    assert!(!opcodes.contains(&xproto::SET_SELECTION_OWNER_REQUEST));
}

#[test]
fn timestamp_probe_timeout_suppresses_id_until_late_event_drains() {
    use super::super::super::data_bridge::SelectionKind;

    let (mut xwm, mut peer, id, _) =
        proxy_fixture_without_authority(XwaylandSelectionKind::Clipboard);
    let _ = fixture_requests(&mut peer);
    let probe_sequence = super::super::super::selection_proxy::timestamp_probe_sequence_for_test(
        &xwm,
        SelectionKind::Clipboard,
    )
    .expect("timestamp probe has an X request identity");
    let deadline = super::super::super::selection_proxy::next_deadline_ns(&xwm)
        .expect("timestamp probe has a bounded deadline");
    let outcome = xwm.handle_deadlines(deadline);
    assert!(outcome.error.is_none());
    assert_eq!(
        super::super::super::selection_proxy::suppressed_id_for_test(
            &xwm,
            SelectionKind::Clipboard
        ),
        Some(id)
    );
    assert_eq!(
        super::super::super::selection_proxy::timestamp_probe_sequence_for_test(
            &xwm,
            SelectionKind::Clipboard
        ),
        Some(probe_sequence),
        "timed-out timestamp probe remains identifiable until drained"
    );

    inject_proxy_time(
        &mut xwm,
        &mut peer,
        XwaylandSelectionKind::Clipboard,
        0x7654_3210,
    );
    let (changes, _, opcodes) = fixture_requests(&mut peer);
    assert!(changes.is_empty());
    assert!(!opcodes.contains(&xproto::SET_SELECTION_OWNER_REQUEST));
    assert!(!opcodes.contains(&xproto::DESTROY_WINDOW_REQUEST));
    assert_eq!(
        super::super::super::selection_proxy::timestamp_probe_sequence_for_test(
            &xwm,
            SelectionKind::Clipboard
        ),
        None,
        "late event drains the old probe identity"
    );
    assert_eq!(
        super::super::super::selection_proxy::authority_for_test(&xwm, SelectionKind::Clipboard),
        None
    );
}

#[test]
fn timestamp_probe_timeout_retires_owner_when_previous_claim_may_be_held() {
    use super::super::super::data_bridge::SelectionKind;

    let (mut xwm, mut peer, id_a, _) =
        proxy_fixture_without_authority(XwaylandSelectionKind::Clipboard);
    let _ = fixture_requests(&mut peer);
    confirm_proxy_ownership(
        &mut xwm,
        &mut peer,
        XwaylandSelectionKind::Clipboard,
        id_a,
        0x3456_7801,
    );
    let _ = fixture_requests(&mut peer);
    let owner = xwm
        .data_bridge
        .selection_proxy
        .owner_window(SelectionKind::Clipboard)
        .expect("active owner for source A");
    let id_b = XwaylandProxySelectionId {
        kind: XwaylandSelectionKind::Clipboard,
        selection_generation: 18,
        source_key: crate::compositor::SelectionSourceKey(1702),
    };
    super::super::super::selection_wire::submit_proxy_selection_snapshots(
        &mut xwm,
        [super::proxy_snapshot(
            XwaylandSelectionKind::Clipboard,
            id_b.selection_generation,
            id_b.source_key.0,
            &["image/png"],
        )],
    )
    .expect("replace A with B while retaining physical ownership");
    finish_proxy_catalog(&mut xwm, &mut peer, id_b, &["image/png".to_owned()]);
    let _ = fixture_requests(&mut peer);

    let deadline = super::super::super::selection_proxy::next_deadline_ns(&xwm)
        .expect("B timestamp probe has a bounded timeout");
    let outcome = xwm.handle_deadlines(deadline);
    assert!(outcome.error.is_none());
    let requests = super::read_fixture_requests(&mut peer);
    assert_eq!(destroyed_windows(&requests), [owner]);
    assert!(owner_requests(&requests).0.is_empty());
    assert_eq!(
        super::super::super::selection_proxy::retiring_owner_for_test(
            &xwm,
            SelectionKind::Clipboard
        )
        .map(|(retiring, _)| retiring),
        Some(owner)
    );
    assert_eq!(
        super::super::super::selection_proxy::suppressed_id_for_test(
            &xwm,
            SelectionKind::Clipboard
        ),
        Some(id_b)
    );
}

#[test]
fn owner_confirmation_timeout_retires_ambiguous_proxy_owner() {
    use super::super::super::data_bridge::SelectionKind;

    let (mut xwm, mut peer, id, _) =
        proxy_fixture_without_authority(XwaylandSelectionKind::Clipboard);
    let _ = fixture_requests(&mut peer);
    let timestamp = 0x1234_5678;
    inject_proxy_time(
        &mut xwm,
        &mut peer,
        XwaylandSelectionKind::Clipboard,
        timestamp,
    );
    let (claims, confirmations) = take_owner_requests(&mut peer);
    let owner = xwm
        .data_bridge
        .selection_proxy
        .owner_window(SelectionKind::Clipboard)
        .expect("private proxy owner window");
    assert_eq!(
        claims,
        [(
            owner,
            selection_atom(&xwm, XwaylandSelectionKind::Clipboard),
            timestamp
        )]
    );
    assert_eq!(confirmations, 1);
    assert_eq!(
        super::super::super::selection_proxy::authority_for_test(&xwm, SelectionKind::Clipboard),
        None
    );

    let deadline = super::super::super::selection_proxy::next_deadline_ns(&xwm)
        .expect("owner confirmation has a bounded deadline");
    let outcome = xwm.handle_deadlines(deadline);
    assert!(outcome.error.is_none());
    assert_eq!(
        super::super::super::selection_proxy::pending_owner_confirmation_for_test(
            &xwm,
            SelectionKind::Clipboard
        ),
        None
    );
    assert_eq!(
        super::super::super::selection_proxy::suppressed_id_for_test(
            &xwm,
            SelectionKind::Clipboard
        ),
        Some(id)
    );
    assert_eq!(
        super::super::super::selection_proxy::held_timestamp_for_test(
            &xwm,
            SelectionKind::Clipboard
        ),
        None
    );
    let requests = super::read_fixture_requests(&mut peer);
    assert_eq!(destroyed_windows(&requests), [owner]);
    let (claims, pending_confirmations) = owner_requests(&requests);
    assert!(claims.is_empty());
    assert_eq!(pending_confirmations, 0);
    assert_eq!(confirmations, 1);
    assert_eq!(
        xwm.data_bridge
            .selection_proxy
            .owner_window(SelectionKind::Clipboard),
        None,
        "ambiguous owner is unavailable until DestroyNotify"
    );
    normalize_proxy_destroy(&mut xwm, owner);
    let fresh_owner = xwm
        .data_bridge
        .selection_proxy
        .owner_window(SelectionKind::Clipboard)
        .expect("DestroyNotify retires the ambiguous owner identity");
    assert_ne!(fresh_owner, owner);
    super::super::super::selection_proxy::prepared_catalog_changed(
        &mut xwm,
        SelectionKind::Clipboard,
        deadline,
    )
    .expect("timed-out ID remains suppressed after fresh owner creation");
    let requests = super::read_fixture_requests(&mut peer);
    assert!(
        proxy_owner_event_masks(&requests)
            .iter()
            .any(|(window, _)| *window == fresh_owner)
    );
    let (changes, _, opcodes) = fixture_requests(&mut peer);
    assert!(!changes.iter().any(|change| change.window == fresh_owner));
    assert!(!opcodes.contains(&xproto::SET_SELECTION_OWNER_REQUEST));
}

#[test]
fn ownership_uses_property_time_and_waits_for_server_confirmation() {
    let (mut xwm, mut peer, id, _) =
        proxy_fixture_without_authority(XwaylandSelectionKind::Clipboard);
    let selection_kind = super::super::super::data_bridge::SelectionKind::Clipboard;
    let owner = xwm
        .data_bridge
        .selection_proxy
        .owner_window(selection_kind)
        .expect("private proxy owner window");
    let (_, _, probe_opcodes) = fixture_requests(&mut peer);
    assert!(probe_opcodes.contains(&xproto::CHANGE_PROPERTY_REQUEST));
    assert!(!probe_opcodes.contains(&xproto::SET_SELECTION_OWNER_REQUEST));

    let timestamp = 0x1234_5678;
    inject_proxy_time(
        &mut xwm,
        &mut peer,
        XwaylandSelectionKind::Clipboard,
        timestamp,
    );
    let (claims, confirmations) = take_owner_requests(&mut peer);
    assert_eq!(
        claims,
        [(
            owner,
            selection_atom(&xwm, XwaylandSelectionKind::Clipboard),
            timestamp
        )]
    );
    assert_eq!(confirmations, 1);
    assert_eq!(
        super::super::super::selection_proxy::authority_for_test(&xwm, selection_kind),
        None
    );
    let (pending_id, pending_timestamp, sequence) =
        super::super::super::selection_proxy::pending_owner_confirmation_for_test(
            &xwm,
            selection_kind,
        )
        .expect("GetSelectionOwner confirmation pending");
    assert_eq!(pending_id, id);
    assert_eq!(pending_timestamp, timestamp);
    let owner_reply = super::raw_get_selection_owner_reply(sequence as u16, owner);
    peer.write_all(&owner_reply)
        .expect("write serialized owner confirmation");
    peer.flush().expect("flush serialized owner confirmation");
    xwm.drain_events(64)
        .expect("install authority only after confirmation");
    assert_eq!(
        super::super::super::selection_proxy::authority_for_test(&xwm, selection_kind),
        Some((id, timestamp)),
        "pending={:?}, suppressed={:?}, held={:?}, owner={:?}",
        super::super::super::selection_proxy::pending_owner_confirmation_for_test(
            &xwm,
            selection_kind
        ),
        super::super::super::selection_proxy::suppressed_id_for_test(&xwm, selection_kind),
        super::super::super::selection_proxy::held_timestamp_for_test(&xwm, selection_kind),
        xwm.data_bridge.selection_proxy.owner_window(selection_kind)
    );
    let mut timestamp_request = request(
        &xwm,
        XwaylandSelectionKind::Clipboard,
        xwm.atoms
            .get(super::super::super::atoms::XwmAtomName::Timestamp),
        TARGET_PROPERTY,
        timestamp,
    );
    timestamp_request.sequence = sequence as u16;
    peer.write_all(&raw_selection_request(&timestamp_request))
        .expect("write serialized TIMESTAMP SelectionRequest");
    xwm.drain_events(64)
        .expect("route TIMESTAMP request through the production path");
    let (changes, notifies, _) = fixture_requests(&mut peer);
    assert!(changes.iter().any(|change| {
        change.window == REQUESTOR
            && change.property == TARGET_PROPERTY
            && change.property_type == u32::from(xproto::AtomEnum::INTEGER)
            && change.format == 32
            && values32(&change.value) == [timestamp]
    }));
    assert!(
        notifies
            .iter()
            .any(|notify| { notify.requestor == REQUESTOR && notify.property == TARGET_PROPERTY })
    );
}

#[test]
fn repeated_same_proxy_id_does_not_reclaim_or_replace_timestamp() {
    let (mut xwm, mut peer, id, _) =
        proxy_fixture_without_authority(XwaylandSelectionKind::Clipboard);
    let _ = fixture_requests(&mut peer);
    let _ = confirm_proxy_ownership(
        &mut xwm,
        &mut peer,
        XwaylandSelectionKind::Clipboard,
        id,
        0x1234_5678,
    );
    let _ = fixture_requests(&mut peer);
    super::super::super::selection_wire::submit_proxy_selection_snapshots(
        &mut xwm,
        [super::proxy_snapshot(
            XwaylandSelectionKind::Clipboard,
            17,
            1701,
            &["image/png"],
        )],
    )
    .expect("resubmit exact current proxy snapshot");
    super::super::super::selection_proxy::prepared_catalog_changed(
        &mut xwm,
        super::super::super::data_bridge::SelectionKind::Clipboard,
        20,
    )
    .expect("reconcile same prepared ID");
    let (_, _, opcodes) = fixture_requests(&mut peer);
    assert!(!opcodes.contains(&xproto::CHANGE_PROPERTY_REQUEST));
    assert!(!opcodes.contains(&xproto::SET_SELECTION_OWNER_REQUEST));
    assert_eq!(
        super::super::super::selection_proxy::authority_for_test(
            &xwm,
            super::super::super::data_bridge::SelectionKind::Clipboard
        ),
        Some((id, 0x1234_5678))
    );
}

#[test]
fn late_timestamp_probe_for_old_id_is_discarded_before_new_probe() {
    let (mut xwm, mut peer, id_a, _) =
        proxy_fixture_without_authority(XwaylandSelectionKind::Clipboard);
    let _ = fixture_requests(&mut peer);
    let id_b = XwaylandProxySelectionId {
        kind: XwaylandSelectionKind::Clipboard,
        selection_generation: 18,
        source_key: crate::compositor::SelectionSourceKey(1702),
    };
    super::super::super::selection_wire::submit_proxy_selection_snapshots(
        &mut xwm,
        [super::proxy_snapshot(
            XwaylandSelectionKind::Clipboard,
            id_b.selection_generation,
            id_b.source_key.0,
            &["image/png"],
        )],
    )
    .expect("replace desired source while old time probe is pending");
    super::finish_proxy_catalog(&mut xwm, &mut peer, id_b, &["image/png".to_owned()]);
    let _ = fixture_requests(&mut peer);

    inject_proxy_time(
        &mut xwm,
        &mut peer,
        XwaylandSelectionKind::Clipboard,
        0x7654_3210,
    );
    let (changes, _, opcodes) = fixture_requests(&mut peer);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].mode, u8::from(xproto::PropMode::APPEND));
    assert!(changes[0].value.is_empty());
    assert!(!opcodes.contains(&xproto::SET_SELECTION_OWNER_REQUEST));
    assert_eq!(
        super::super::super::selection_proxy::pending_owner_confirmation_for_test(
            &xwm,
            super::super::super::data_bridge::SelectionKind::Clipboard
        ),
        None
    );
    assert_ne!(id_a, id_b);
}

#[test]
fn production_selection_request_event_routes_to_proxy_engine() {
    let (mut xwm, mut peer, id, _) =
        proxy_fixture_without_authority(XwaylandSelectionKind::Clipboard);
    let _ = fixture_requests(&mut peer);
    let owner_confirmation_sequence = confirm_proxy_ownership(
        &mut xwm,
        &mut peer,
        XwaylandSelectionKind::Clipboard,
        id,
        12_345,
    );
    let mut event = request(
        &xwm,
        XwaylandSelectionKind::Clipboard,
        xwm.atoms
            .get(super::super::super::atoms::XwmAtomName::Targets),
        TARGET_PROPERTY,
        12_345,
    );
    event.sequence = owner_confirmation_sequence;
    peer.write_all(&raw_selection_request(&event))
        .expect("write serialized SelectionRequest event");
    xwm.drain_events(64)
        .expect("route production SelectionRequest event");

    let (changes, notifies, _) = fixture_requests(&mut peer);
    assert!(
        changes
            .iter()
            .any(|change| { change.window == REQUESTOR && change.property == TARGET_PROPERTY })
    );
    assert!(notifies.iter().any(|notify| {
        notify.requestor == REQUESTOR
            && notify.selection == event.selection
            && notify.property == TARGET_PROPERTY
    }));
}

#[test]
fn production_selection_clear_revokes_proxy_authority() {
    use super::super::super::data_bridge::SelectionKind;

    let (mut xwm, mut peer, id, _) =
        proxy_fixture_without_authority(XwaylandSelectionKind::Clipboard);
    let _ = fixture_requests(&mut peer);
    let owner_confirmation_sequence = confirm_proxy_ownership(
        &mut xwm,
        &mut peer,
        XwaylandSelectionKind::Clipboard,
        id,
        12_345,
    );
    let owner = xwm
        .data_bridge
        .selection_proxy
        .owner_window(super::super::super::data_bridge::SelectionKind::Clipboard)
        .expect("private proxy owner window");
    let clear = xproto::SelectionClearEvent {
        response_type: xproto::SELECTION_CLEAR_EVENT,
        sequence: owner_confirmation_sequence,
        time: 20_000,
        owner,
        selection: selection_atom(&xwm, XwaylandSelectionKind::Clipboard),
    };
    peer.write_all(&raw_selection_clear(&clear))
        .expect("write serialized SelectionClear event");
    xwm.drain_events(64)
        .expect("route production SelectionClear event");

    let request = request(
        &xwm,
        XwaylandSelectionKind::Clipboard,
        xwm.atoms
            .get(super::super::super::atoms::XwmAtomName::Targets),
        TARGET_PROPERTY,
        12_345,
    );
    super::super::super::selection_proxy::handle_selection_request(&mut xwm, request, 1)
        .expect("exercise authority after SelectionClear");
    let (changes, notifies, _) = fixture_requests(&mut peer);
    assert!(
        !changes
            .iter()
            .any(|change| { change.window == REQUESTOR && change.property == TARGET_PROPERTY })
    );
    assert!(
        notifies
            .iter()
            .any(|notify| { notify.requestor == REQUESTOR && notify.property == x11rb::NONE })
    );

    let observer = xwm
        .data_bridge
        .selection_wire
        .observer_window_for_test(super::super::super::data_bridge::SelectionKind::Clipboard)
        .expect("clipboard XFixes observer");
    peer.write_all(&super::raw_xfixes_selection_event(
        selection_atom(&xwm, XwaylandSelectionKind::Clipboard),
        observer,
        x11rb::protocol::xfixes::SelectionEvent::SET_SELECTION_OWNER,
        0xdead,
        20_001,
        20_000,
        owner_confirmation_sequence,
    ))
    .expect("write later external-owner XFixes notification");
    xwm.drain_events(64)
        .expect("process owner loss after SelectionClear");
    let (changes, _, opcodes) = fixture_requests(&mut peer);
    assert!(!opcodes.contains(&xproto::SET_SELECTION_OWNER_REQUEST));
    let timestamp_property = xwm
        .atoms
        .get(super::super::super::atoms::XwmAtomName::SelectionProxyTime);
    let clipboard_owner = xwm
        .data_bridge
        .selection_proxy
        .owner_window(SelectionKind::Clipboard)
        .expect("Clipboard proxy owner window");
    assert!(!changes.iter().any(|change| {
        change.window == clipboard_owner && change.property == timestamp_property
    }));
    assert_eq!(
        super::super::super::selection_proxy::authority_for_test(
            &xwm,
            super::super::super::data_bridge::SelectionKind::Clipboard
        ),
        None
    );
    assert_eq!(
        super::super::super::selection_proxy::suppressed_id_for_test(
            &xwm,
            super::super::super::data_bridge::SelectionKind::Clipboard
        ),
        Some(id)
    );
}

#[test]
fn destroyed_proxy_owner_is_disabled_for_the_generation() {
    use super::super::super::data_bridge::SelectionKind;

    let (mut xwm, mut peer, id, _) =
        proxy_fixture_without_authority(XwaylandSelectionKind::Clipboard);
    let _ = fixture_requests(&mut peer);
    let confirmation_sequence = confirm_proxy_ownership(
        &mut xwm,
        &mut peer,
        XwaylandSelectionKind::Clipboard,
        id,
        0x1234_5678,
    );
    let _ = fixture_requests(&mut peer);
    let owner = xwm
        .data_bridge
        .selection_proxy
        .owner_window(SelectionKind::Clipboard)
        .expect("private proxy owner window");

    super::super::super::events::normalize(
        &mut xwm,
        Event::DestroyNotify(xproto::DestroyNotifyEvent {
            response_type: xproto::DESTROY_NOTIFY_EVENT,
            sequence: confirmation_sequence,
            event: 1,
            window: owner,
        }),
    )
    .expect("internal owner destruction reaches the proxy manager");
    assert!(
        super::super::super::selection_proxy::owner_window_disabled_for_test(
            &xwm,
            SelectionKind::Clipboard
        )
    );
    assert!(!xwm.data_bridge.selection_wire.is_internal_window(owner));
    assert_eq!(
        super::super::super::selection_proxy::authority_for_test(&xwm, SelectionKind::Clipboard),
        None
    );
    assert_eq!(
        super::super::super::selection_proxy::suppressed_id_for_test(
            &xwm,
            SelectionKind::Clipboard
        ),
        Some(id)
    );

    super::super::super::selection_proxy::prepared_catalog_changed(
        &mut xwm,
        SelectionKind::Clipboard,
        crate::native::event_loop::monotonic_now_ns().unwrap_or_default(),
    )
    .expect("reconciliation leaves a destroyed generation owner disabled");
    let bytes = super::read_fixture_requests(&mut peer);
    let mut opcodes = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        let byte_len = usize::from(u16::from_ne_bytes([bytes[offset + 2], bytes[offset + 3]])) * 4;
        assert!(byte_len >= 4 && offset + byte_len <= bytes.len());
        opcodes.push(bytes[offset]);
        offset += byte_len;
    }
    assert!(!opcodes.contains(&xproto::CREATE_WINDOW_REQUEST));
    assert!(!opcodes.contains(&xproto::CHANGE_PROPERTY_REQUEST));
    assert!(!opcodes.contains(&xproto::SET_SELECTION_OWNER_REQUEST));
}

#[test]
fn early_request_fails_until_owner_confirmation_then_uses_data_engine() {
    let (mut xwm, mut peer, id, target) =
        proxy_fixture_without_authority(XwaylandSelectionKind::Clipboard);
    let _ = fixture_requests(&mut peer);
    let timestamp = 0x1234_5678;
    inject_proxy_time(
        &mut xwm,
        &mut peer,
        XwaylandSelectionKind::Clipboard,
        timestamp,
    );
    let (claims, confirmations) = take_owner_requests(&mut peer);
    let owner = xwm
        .data_bridge
        .selection_proxy
        .owner_window(super::super::super::data_bridge::SelectionKind::Clipboard)
        .expect("private proxy owner window");
    assert_eq!(
        claims,
        [(
            owner,
            selection_atom(&xwm, XwaylandSelectionKind::Clipboard),
            timestamp
        )]
    );
    assert_eq!(confirmations, 1);
    let (_, _, sequence) =
        super::super::super::selection_proxy::pending_owner_confirmation_for_test(
            &xwm,
            super::super::super::data_bridge::SelectionKind::Clipboard,
        )
        .expect("owner confirmation remains pending");

    let mut early = request(
        &xwm,
        XwaylandSelectionKind::Clipboard,
        target,
        TARGET_PROPERTY,
        timestamp,
    );
    early.sequence = sequence as u16;
    peer.write_all(&raw_selection_request(&early))
        .expect("write early SelectionRequest");
    xwm.drain_events(64)
        .expect("route early SelectionRequest while authority is absent");
    assert!(super::super::super::selection_proxy::take_managed_data_requests(&mut xwm).is_empty());
    let (changes, notifies, _) = fixture_requests(&mut peer);
    assert!(
        !changes
            .iter()
            .any(|change| change.property == TARGET_PROPERTY)
    );
    assert!(
        notifies
            .iter()
            .any(|notify| notify.requestor == REQUESTOR && notify.property == x11rb::NONE)
    );

    peer.write_all(&super::raw_get_selection_owner_reply(
        sequence as u16,
        owner,
    ))
    .expect("write owner confirmation after early failure");
    xwm.drain_events(64)
        .expect("install authority from server-confirmed reply");
    assert_eq!(
        super::super::super::selection_proxy::authority_for_test(
            &xwm,
            super::super::super::data_bridge::SelectionKind::Clipboard
        ),
        Some((id, timestamp))
    );

    let mut ready = request(
        &xwm,
        XwaylandSelectionKind::Clipboard,
        target,
        TARGET_PROPERTY,
        timestamp,
    );
    ready.sequence = sequence as u16;
    peer.write_all(&raw_selection_request(&ready))
        .expect("write confirmed data SelectionRequest");
    xwm.drain_events(64)
        .expect("route confirmed data request into B3-B1 engine");
    assert_eq!(
        super::super::super::selection_proxy::take_managed_data_requests(&mut xwm).len(),
        1
    );
}

#[test]
fn failed_owner_confirmation_suppresses_the_current_proxy_id() {
    let (mut xwm, mut peer, id, _) =
        proxy_fixture_without_authority(XwaylandSelectionKind::Clipboard);
    let _ = fixture_requests(&mut peer);
    let owner = xwm
        .data_bridge
        .selection_proxy
        .owner_window(super::super::super::data_bridge::SelectionKind::Clipboard)
        .expect("private proxy owner window");
    let timestamp = 0x1234_5678;
    inject_proxy_time(
        &mut xwm,
        &mut peer,
        XwaylandSelectionKind::Clipboard,
        timestamp,
    );
    let (claims, confirmations) = take_owner_requests(&mut peer);
    assert_eq!(
        claims,
        [(
            owner,
            selection_atom(&xwm, XwaylandSelectionKind::Clipboard),
            timestamp
        )]
    );
    assert_eq!(confirmations, 1);
    let (_, _, sequence) =
        super::super::super::selection_proxy::pending_owner_confirmation_for_test(
            &xwm,
            super::super::super::data_bridge::SelectionKind::Clipboard,
        )
        .expect("owner confirmation pending");
    peer.write_all(&super::raw_get_selection_owner_reply(
        sequence as u16,
        0xbeef,
    ))
    .expect("write failed owner confirmation");
    xwm.drain_events(64)
        .expect("process failed owner confirmation");

    let selection_kind = super::super::super::data_bridge::SelectionKind::Clipboard;
    assert_eq!(
        super::super::super::selection_proxy::authority_for_test(&xwm, selection_kind),
        None
    );
    assert_eq!(
        super::super::super::selection_proxy::suppressed_id_for_test(&xwm, selection_kind),
        Some(id)
    );
    super::super::super::selection_proxy::prepared_catalog_changed(&mut xwm, selection_kind, 10)
        .expect("reconcile suppressed source");
    let requests = super::read_fixture_requests(&mut peer);
    let (claims, confirmations) = owner_requests(&requests);
    assert!(claims.is_empty());
    assert_eq!(confirmations, 0);
    assert!(destroyed_windows(&requests).is_empty());
    assert_eq!(
        xwm.data_bridge.selection_proxy.owner_window(selection_kind),
        Some(owner),
        "a reply proving another owner won does not require retiring Typhon's window"
    );
    let (_, _, opcodes) = fixture_requests(&mut peer);
    assert!(!opcodes.contains(&xproto::CHANGE_PROPERTY_REQUEST));
}

#[test]
fn canonical_clear_retires_exact_confirmed_owner_window() {
    let (mut xwm, mut peer, id, _) =
        proxy_fixture_without_authority(XwaylandSelectionKind::Clipboard);
    let _ = fixture_requests(&mut peer);
    let timestamp = 0xffff_fffd;
    confirm_proxy_ownership(
        &mut xwm,
        &mut peer,
        XwaylandSelectionKind::Clipboard,
        id,
        timestamp,
    );
    let _ = fixture_requests(&mut peer);
    let owner = xwm
        .data_bridge
        .selection_proxy
        .owner_window(super::super::super::data_bridge::SelectionKind::Clipboard)
        .expect("confirmed private proxy owner");

    super::super::super::selection_wire::submit_proxy_selection_snapshots(
        &mut xwm,
        [super::proxy_clear_snapshot(
            XwaylandSelectionKind::Clipboard,
            18,
        )],
    )
    .expect("withdraw canonical Wayland proxy offer");
    let requests = super::read_fixture_requests(&mut peer);
    let (claims, confirmations) = owner_requests(&requests);
    assert!(
        claims.is_empty(),
        "release never uses SetSelectionOwner(None)"
    );
    assert_eq!(confirmations, 0);
    assert_eq!(destroyed_windows(&requests), [owner]);
    assert_eq!(
        xwm.data_bridge
            .selection_proxy
            .owner_window(super::super::super::data_bridge::SelectionKind::Clipboard),
        None,
        "replacement owner waits until DestroyNotify confirms retirement"
    );
    assert!(proxy_owner_event_masks(&requests).is_empty());
    assert_eq!(
        super::super::super::selection_proxy::authority_for_test(
            &xwm,
            super::super::super::data_bridge::SelectionKind::Clipboard
        ),
        None
    );
    assert_eq!(
        super::super::super::selection_proxy::held_timestamp_for_test(
            &xwm,
            super::super::super::data_bridge::SelectionKind::Clipboard
        ),
        None
    );
}

#[test]
fn expected_retirement_recreates_fresh_owner_only_after_destroy_notify() {
    use super::super::super::data_bridge::SelectionKind;

    let (mut xwm, mut peer, id_a, _) =
        proxy_fixture_without_authority(XwaylandSelectionKind::Clipboard);
    let _ = fixture_requests(&mut peer);
    confirm_proxy_ownership(
        &mut xwm,
        &mut peer,
        XwaylandSelectionKind::Clipboard,
        id_a,
        0x1234_5001,
    );
    let _ = fixture_requests(&mut peer);
    let kind = SelectionKind::Clipboard;
    let old_owner = xwm
        .data_bridge
        .selection_proxy
        .owner_window(kind)
        .expect("active owner before retirement");
    let internal_count = xwm
        .data_bridge
        .selection_wire
        .internal_window_count_for_test();

    super::super::super::selection_wire::submit_proxy_selection_snapshots(
        &mut xwm,
        [super::proxy_clear_snapshot(
            XwaylandSelectionKind::Clipboard,
            18,
        )],
    )
    .expect("begin owner retirement");
    let retirement_requests = super::read_fixture_requests(&mut peer);
    assert_eq!(destroyed_windows(&retirement_requests), [old_owner]);
    assert_eq!(
        super::super::super::selection_proxy::retiring_owner_for_test(&xwm, kind)
            .map(|(owner, _)| owner),
        Some(old_owner)
    );
    assert!(xwm.data_bridge.selection_wire.is_internal_window(old_owner));

    let id_b = XwaylandProxySelectionId {
        kind: XwaylandSelectionKind::Clipboard,
        selection_generation: 19,
        source_key: crate::compositor::SelectionSourceKey(1902),
    };
    super::super::super::selection_wire::submit_proxy_selection_snapshots(
        &mut xwm,
        [super::proxy_snapshot(
            XwaylandSelectionKind::Clipboard,
            19,
            1902,
            &["image/png"],
        )],
    )
    .expect("new Wayland source appears during retirement");
    finish_proxy_catalog(&mut xwm, &mut peer, id_b, &["image/png".to_owned()]);
    let timestamp_atom = xwm
        .atoms
        .get(super::super::super::atoms::XwmAtomName::SelectionProxyTime);
    let (changes, _, opcodes) = fixture_requests(&mut peer);
    assert!(
        !changes
            .iter()
            .any(|change| change.property == timestamp_atom)
    );
    assert!(!opcodes.contains(&xproto::SET_SELECTION_OWNER_REQUEST));
    assert!(proxy_owner_event_masks(&retirement_requests).is_empty());
    assert_eq!(
        super::super::super::selection_proxy::suppressed_id_for_test(&xwm, kind),
        None
    );

    peer.write_all(&raw_proxy_time_notify(
        old_owner,
        timestamp_atom,
        0x7654_3210,
        0,
    ))
    .expect("queue stale timestamp PropertyNotify for retiring owner");
    xwm.drain_events(64)
        .expect("retiring XID stays internal and cannot start a claim");
    let (changes, _, opcodes) = fixture_requests(&mut peer);
    assert!(changes.is_empty());
    assert!(!opcodes.contains(&xproto::SET_SELECTION_OWNER_REQUEST));
    assert!(super::super::super::selection_proxy::retiring_owner_for_test(&xwm, kind).is_some());

    let observer = xwm
        .data_bridge
        .selection_wire
        .observer_window_for_test(kind)
        .expect("Clipboard XFixes observer");
    peer.write_all(&super::raw_xfixes_selection_event(
        selection_atom(&xwm, XwaylandSelectionKind::Clipboard),
        observer,
        x11rb::protocol::xfixes::SelectionEvent::SET_SELECTION_OWNER,
        0,
        0x1234_5002,
        0x1234_5002,
        0,
    ))
    .expect("queue owner=None observation while retirement is expected");
    xwm.drain_events(64)
        .expect("owner=None observation continues through inbound selection wire");
    assert_eq!(
        super::super::super::selection_proxy::suppressed_id_for_test(&xwm, kind),
        None,
        "intentional owner=None must not suppress the newer Wayland proxy"
    );

    normalize_proxy_destroy(&mut xwm, old_owner);
    let fresh_owner = xwm
        .data_bridge
        .selection_proxy
        .owner_window(kind)
        .expect("expected DestroyNotify creates the next active owner");
    assert_ne!(fresh_owner, old_owner);
    assert!(!xwm.data_bridge.selection_wire.is_internal_window(old_owner));
    assert!(
        xwm.data_bridge
            .selection_wire
            .is_internal_window(fresh_owner)
    );
    assert_eq!(
        xwm.data_bridge
            .selection_wire
            .internal_window_count_for_test(),
        internal_count,
        "retired XID is unregistered as its replacement is registered"
    );
    let bytes = super::read_fixture_requests(&mut peer);
    let masks = proxy_owner_event_masks(&bytes);
    assert!(masks.iter().any(|(window, mask)| {
        *window == fresh_owner
            && mask & xproto::EventMask::PROPERTY_CHANGE.bits() != 0
            && mask & xproto::EventMask::STRUCTURE_NOTIFY.bits() != 0
    }));
    let (changes, _, opcodes) = fixture_requests(&mut peer);
    assert!(changes.iter().any(|change| {
        change.window == fresh_owner
            && change.property == timestamp_atom
            && change.mode == u8::from(xproto::PropMode::APPEND)
    }));
    assert!(!opcodes.contains(&xproto::SET_SELECTION_OWNER_REQUEST));
    let timestamp_b = 0x5678_9012;
    confirm_proxy_ownership(
        &mut xwm,
        &mut peer,
        XwaylandSelectionKind::Clipboard,
        id_b,
        timestamp_b,
    );
    assert_eq!(
        super::super::super::selection_proxy::authority_for_test(&xwm, kind),
        Some((id_b, timestamp_b))
    );

    for cycle in 0..3_u64 {
        let selection_generation = 20 + cycle * 2;
        let source_key = 2_000 + cycle;
        let id = XwaylandProxySelectionId {
            kind: XwaylandSelectionKind::Clipboard,
            selection_generation,
            source_key: crate::compositor::SelectionSourceKey(source_key),
        };
        super::super::super::selection_wire::submit_proxy_selection_snapshots(
            &mut xwm,
            [super::proxy_snapshot(
                XwaylandSelectionKind::Clipboard,
                selection_generation,
                source_key,
                &["image/png"],
            )],
        )
        .expect("submit repeated replacement for owner bookkeeping");
        finish_proxy_catalog(&mut xwm, &mut peer, id, &["image/png".to_owned()]);
        let active_owner = xwm
            .data_bridge
            .selection_proxy
            .owner_window(kind)
            .expect("recovered active owner before repeated retirement");
        confirm_proxy_ownership(
            &mut xwm,
            &mut peer,
            XwaylandSelectionKind::Clipboard,
            id,
            0x5678_9200 + cycle as u32,
        );
        let _ = fixture_requests(&mut peer);
        super::super::super::selection_wire::submit_proxy_selection_snapshots(
            &mut xwm,
            [super::proxy_clear_snapshot(
                XwaylandSelectionKind::Clipboard,
                selection_generation + 1,
            )],
        )
        .expect("retire repeated owner identity");
        let requests = super::read_fixture_requests(&mut peer);
        assert_eq!(destroyed_windows(&requests), [active_owner]);
        normalize_proxy_destroy(&mut xwm, active_owner);
        let replacement = xwm
            .data_bridge
            .selection_proxy
            .owner_window(kind)
            .expect("repeated DestroyNotify restores an active owner");
        assert_ne!(replacement, active_owner);
        assert_eq!(
            xwm.data_bridge
                .selection_wire
                .internal_window_count_for_test(),
            internal_count,
            "repeated retirements do not grow the internal XID registry"
        );
        let _ = super::read_fixture_requests(&mut peer);
    }
}

#[test]
fn retirement_timeout_disables_channel_without_recreating_owner() {
    use super::super::super::data_bridge::SelectionKind;

    let (mut xwm, mut peer, id_a, _) =
        proxy_fixture_without_authority(XwaylandSelectionKind::Clipboard);
    let _ = fixture_requests(&mut peer);
    confirm_proxy_ownership(
        &mut xwm,
        &mut peer,
        XwaylandSelectionKind::Clipboard,
        id_a,
        0x2233_4401,
    );
    let _ = fixture_requests(&mut peer);
    let kind = SelectionKind::Clipboard;
    let old_owner = xwm
        .data_bridge
        .selection_proxy
        .owner_window(kind)
        .expect("active owner before retirement");
    super::super::super::selection_wire::submit_proxy_selection_snapshots(
        &mut xwm,
        [super::proxy_clear_snapshot(
            XwaylandSelectionKind::Clipboard,
            18,
        )],
    )
    .expect("begin owner retirement");
    let _ = super::read_fixture_requests(&mut peer);

    let id_b = XwaylandProxySelectionId {
        kind: XwaylandSelectionKind::Clipboard,
        selection_generation: 19,
        source_key: crate::compositor::SelectionSourceKey(1903),
    };
    super::super::super::selection_wire::submit_proxy_selection_snapshots(
        &mut xwm,
        [super::proxy_snapshot(
            XwaylandSelectionKind::Clipboard,
            19,
            1903,
            &["image/png"],
        )],
    )
    .expect("new source waits during owner retirement");
    finish_proxy_catalog(&mut xwm, &mut peer, id_b, &["image/png".to_owned()]);
    let _ = super::read_fixture_requests(&mut peer);
    let deadline = super::super::super::selection_proxy::retiring_owner_for_test(&xwm, kind)
        .expect("bounded owner retirement deadline")
        .1;
    let outcome = xwm.handle_deadlines(deadline);
    assert!(outcome.error.is_none());
    assert!(super::super::super::selection_proxy::owner_window_disabled_for_test(&xwm, kind));
    assert_eq!(
        super::super::super::selection_proxy::suppressed_id_for_test(&xwm, kind),
        Some(id_b)
    );
    assert!(xwm.data_bridge.selection_wire.is_internal_window(old_owner));
    super::super::super::selection_proxy::prepared_catalog_changed(&mut xwm, kind, deadline)
        .expect("disabled channel remains inactive");
    let requests = super::read_fixture_requests(&mut peer);
    assert!(proxy_owner_event_masks(&requests).is_empty());
    assert!(destroyed_windows(&requests).is_empty());
    assert!(owner_requests(&requests).0.is_empty());
}

#[test]
fn delayed_external_takeover_imports_after_identity_safe_retirement() {
    use super::super::super::data_bridge::SelectionKind;

    let (mut xwm, mut peer, id, _) =
        proxy_fixture_without_authority(XwaylandSelectionKind::Clipboard);
    let _ = fixture_requests(&mut peer);
    let claim_timestamp = 0x1234_5001;
    let confirmation_sequence = confirm_proxy_ownership(
        &mut xwm,
        &mut peer,
        XwaylandSelectionKind::Clipboard,
        id,
        claim_timestamp,
    );
    let _ = fixture_requests(&mut peer);

    let selection = selection_atom(&xwm, XwaylandSelectionKind::Clipboard);
    let owner = xwm
        .data_bridge
        .selection_proxy
        .owner_window(SelectionKind::Clipboard)
        .expect("confirmed private proxy owner");
    let observer = xwm
        .data_bridge
        .selection_wire
        .observer_window_for_test(SelectionKind::Clipboard)
        .expect("Clipboard XFixes observer");
    peer.write_all(&super::raw_xfixes_selection_event(
        selection,
        observer,
        x11rb::protocol::xfixes::SelectionEvent::SET_SELECTION_OWNER,
        0xbeef,
        0x1234_5002,
        0x1234_5002,
        confirmation_sequence,
    ))
    .expect("queue newer external owner event without processing it yet");

    super::super::super::selection_wire::submit_proxy_selection_snapshots(
        &mut xwm,
        [super::proxy_clear_snapshot(
            XwaylandSelectionKind::Clipboard,
            18,
        )],
    )
    .expect("canonical clear retires the exact proxy owner before takeover observation");
    let requests = super::read_fixture_requests(&mut peer);
    let (claims, confirmations) = owner_requests(&requests);
    assert!(claims.is_empty());
    assert_eq!(confirmations, 0);
    assert_eq!(destroyed_windows(&requests), [owner]);

    xwm.drain_events(64)
        .expect("later process the external takeover notification");
    assert_eq!(
        xwm.data_bridge
            .selections
            .current(SelectionKind::Clipboard)
            .and_then(|state| state.owner),
        Some(0xbeef)
    );
    let requests = super::read_fixture_requests(&mut peer);
    assert!(owner_requests(&requests).0.is_empty());
    assert!(
        super::convert_selection_requests(&requests)
            .iter()
            .any(|request| request.selection == selection)
    );
}

#[test]
fn pending_claim_clear_retires_ambiguous_owner_window() {
    let (mut xwm, mut peer, _, _) =
        proxy_fixture_without_authority(XwaylandSelectionKind::Clipboard);
    let _ = fixture_requests(&mut peer);
    let timestamp = 0x2345_6789;
    inject_proxy_time(
        &mut xwm,
        &mut peer,
        XwaylandSelectionKind::Clipboard,
        timestamp,
    );
    let (claims, confirmations) = take_owner_requests(&mut peer);
    let selection = selection_atom(&xwm, XwaylandSelectionKind::Clipboard);
    let owner = xwm
        .data_bridge
        .selection_proxy
        .owner_window(super::super::super::data_bridge::SelectionKind::Clipboard)
        .expect("private proxy owner window");
    assert_eq!(claims, [(owner, selection, timestamp)]);
    assert_eq!(confirmations, 1);

    super::super::super::selection_wire::submit_proxy_selection_snapshots(
        &mut xwm,
        [super::proxy_clear_snapshot(
            XwaylandSelectionKind::Clipboard,
            18,
        )],
    )
    .expect("clear while owner confirmation is pending");
    let requests = super::read_fixture_requests(&mut peer);
    let (claims, confirmations) = owner_requests(&requests);
    assert!(claims.is_empty());
    assert_eq!(confirmations, 0);
    assert_eq!(destroyed_windows(&requests), [owner]);
    assert_eq!(
        super::super::super::selection_proxy::pending_owner_confirmation_for_test(
            &xwm,
            super::super::super::data_bridge::SelectionKind::Clipboard
        ),
        None
    );
    assert_eq!(
        xwm.data_bridge
            .selection_proxy
            .owner_window(super::super::super::data_bridge::SelectionKind::Clipboard),
        None
    );
}

#[test]
fn source_replacement_revokes_then_rebinds_same_private_owner_window() {
    let (mut xwm, mut peer, id_a, _) =
        proxy_fixture_without_authority(XwaylandSelectionKind::Clipboard);
    let _ = fixture_requests(&mut peer);
    let timestamp_a = 0x3456_789a;
    confirm_proxy_ownership(
        &mut xwm,
        &mut peer,
        XwaylandSelectionKind::Clipboard,
        id_a,
        timestamp_a,
    );
    let _ = fixture_requests(&mut peer);
    let owner = xwm
        .data_bridge
        .selection_proxy
        .owner_window(super::super::super::data_bridge::SelectionKind::Clipboard)
        .expect("generation-local private owner window");
    let id_b = XwaylandProxySelectionId {
        kind: XwaylandSelectionKind::Clipboard,
        selection_generation: 18,
        source_key: crate::compositor::SelectionSourceKey(1702),
    };

    super::super::super::selection_wire::submit_proxy_selection_snapshots(
        &mut xwm,
        [super::proxy_snapshot(
            XwaylandSelectionKind::Clipboard,
            id_b.selection_generation,
            id_b.source_key.0,
            &["image/png"],
        )],
    )
    .expect("replace canonical proxy identity");
    assert_eq!(
        super::super::super::selection_proxy::authority_for_test(
            &xwm,
            super::super::super::data_bridge::SelectionKind::Clipboard
        ),
        None,
        "source A stops serving before B is prepared"
    );
    assert!(
        owner_requests(&super::read_fixture_requests(&mut peer))
            .0
            .is_empty()
    );
    super::finish_proxy_catalog(&mut xwm, &mut peer, id_b, &["image/png".to_owned()]);
    let (changes, _, opcodes) = fixture_requests(&mut peer);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].window, owner);
    assert_eq!(changes[0].mode, u8::from(xproto::PropMode::APPEND));
    assert!(!opcodes.contains(&xproto::SET_SELECTION_OWNER_REQUEST));

    let timestamp_b = 0x4567_89ab;
    inject_proxy_time(
        &mut xwm,
        &mut peer,
        XwaylandSelectionKind::Clipboard,
        timestamp_b,
    );
    let (claims, confirmations) = take_owner_requests(&mut peer);
    assert_eq!(
        claims,
        [(
            owner,
            selection_atom(&xwm, XwaylandSelectionKind::Clipboard),
            timestamp_b
        )]
    );
    assert_eq!(confirmations, 1);
    let (_, _, sequence) =
        super::super::super::selection_proxy::pending_owner_confirmation_for_test(
            &xwm,
            super::super::super::data_bridge::SelectionKind::Clipboard,
        )
        .expect("B owner confirmation pending");
    peer.write_all(&super::raw_get_selection_owner_reply(
        sequence as u16,
        owner,
    ))
    .expect("write B owner confirmation");
    xwm.drain_events(64)
        .expect("confirm B after same-window rebind");
    assert_eq!(
        super::super::super::selection_proxy::authority_for_test(
            &xwm,
            super::super::super::data_bridge::SelectionKind::Clipboard
        ),
        Some((id_b, timestamp_b))
    );
    assert_ne!(id_a, id_b);
}

#[test]
fn empty_data_catalog_and_missing_xfixes_do_not_start_claims() {
    let (mut xwm, mut peer) = test_fixture(generation(302));
    initialize_selection_wire(&mut xwm, &mut peer);
    let _ = fixture_requests(&mut peer);
    super::super::super::selection_wire::submit_proxy_selection_snapshots(
        &mut xwm,
        [super::proxy_snapshot(
            XwaylandSelectionKind::Clipboard,
            19,
            1901,
            &["TARGETS"],
        )],
    )
    .expect("prepare protocol-only catalog");
    assert!(
        super::super::super::selection_wire::prepared_proxy_selection_for_test(
            &xwm,
            super::super::super::data_bridge::SelectionKind::Clipboard
        )
        .is_some_and(|prepared| prepared.data_targets.is_empty())
    );
    let (_, _, opcodes) = fixture_requests(&mut peer);
    assert!(!opcodes.contains(&xproto::CHANGE_PROPERTY_REQUEST));
    assert!(!opcodes.contains(&xproto::SET_SELECTION_OWNER_REQUEST));

    let (mut no_xfixes, mut no_xfixes_peer) = test_fixture(generation(303));
    initialize_selection_wire(&mut no_xfixes, &mut no_xfixes_peer);
    let _ = fixture_requests(&mut no_xfixes_peer);
    no_xfixes.capabilities.xfixes = false;
    let id = XwaylandProxySelectionId {
        kind: XwaylandSelectionKind::Clipboard,
        selection_generation: 20,
        source_key: crate::compositor::SelectionSourceKey(2001),
    };
    super::super::super::selection_wire::submit_proxy_selection_snapshots(
        &mut no_xfixes,
        [super::proxy_snapshot(
            XwaylandSelectionKind::Clipboard,
            id.selection_generation,
            id.source_key.0,
            &["image/png"],
        )],
    )
    .expect("prepare catalog without ownership capability");
    super::finish_proxy_catalog(
        &mut no_xfixes,
        &mut no_xfixes_peer,
        id,
        &["image/png".to_owned()],
    );
    let (_, _, opcodes) = fixture_requests(&mut no_xfixes_peer);
    assert!(!opcodes.contains(&xproto::CHANGE_PROPERTY_REQUEST));
    assert!(!opcodes.contains(&xproto::SET_SELECTION_OWNER_REQUEST));
}

#[test]
fn external_takeover_revokes_proxy_and_keeps_inbound_discovery_active() {
    use super::super::super::data_bridge::{SelectionKind, SelectionOrigin};

    let (mut xwm, mut peer, id, target) =
        proxy_fixture_without_authority(XwaylandSelectionKind::Clipboard);
    let _ = fixture_requests(&mut peer);
    let timestamp = 0x3456_789a;
    let confirmation_sequence = confirm_proxy_ownership(
        &mut xwm,
        &mut peer,
        XwaylandSelectionKind::Clipboard,
        id,
        timestamp,
    );
    let _ = fixture_requests(&mut peer);
    let mut data_request = request(
        &xwm,
        XwaylandSelectionKind::Clipboard,
        target,
        TARGET_PROPERTY,
        timestamp,
    );
    data_request.sequence = confirmation_sequence;
    peer.write_all(&raw_selection_request(&data_request))
        .expect("queue pending compositor data request");
    xwm.drain_events(64)
        .expect("start the active reverse data transfer");
    assert_eq!(xwm.data_bridge.selection_outgoing.active_count(), 1);
    assert_eq!(
        super::super::super::selection_proxy::request_count_for_test(&xwm),
        1
    );

    let selection = selection_atom(&xwm, XwaylandSelectionKind::Clipboard);
    let observer = xwm
        .data_bridge
        .selection_wire
        .observer_window_for_test(SelectionKind::Clipboard)
        .expect("clipboard XFixes observer");
    let external_owner = 0xdead;
    peer.write_all(&super::raw_xfixes_selection_event(
        selection,
        observer,
        x11rb::protocol::xfixes::SelectionEvent::SET_SELECTION_OWNER,
        external_owner,
        50_000,
        49_000,
        0,
    ))
    .expect("write external XFixes owner transition");
    xwm.drain_events(64)
        .expect("revoke proxy and observe external owner");

    let channel = xwm
        .data_bridge
        .selections
        .current(SelectionKind::Clipboard)
        .expect("clipboard selection state");
    assert_eq!(channel.owner, Some(external_owner));
    assert_eq!(channel.origin, Some(SelectionOrigin::X11));
    assert_eq!(
        super::super::super::selection_proxy::authority_for_test(&xwm, SelectionKind::Clipboard),
        None
    );
    assert_eq!(
        super::super::super::selection_proxy::held_timestamp_for_test(
            &xwm,
            SelectionKind::Clipboard
        ),
        None
    );
    assert_eq!(
        super::super::super::selection_proxy::suppressed_id_for_test(
            &xwm,
            SelectionKind::Clipboard
        ),
        Some(id)
    );
    assert_eq!(xwm.data_bridge.selection_outgoing.active_count(), 0);
    assert!(super::super::super::selection_proxy::take_managed_data_requests(&mut xwm).is_empty());
    assert_eq!(
        super::super::super::selection_proxy::request_count_for_test(&xwm),
        0,
        "request cancellation reports failure through the existing notification queue"
    );
    let requests = super::read_fixture_requests(&mut peer);
    let conversions = super::convert_selection_requests(&requests);
    assert!(conversions.iter().any(|request| {
        request.selection == selection
            && request.target
                == xwm
                    .atoms
                    .get(super::super::super::atoms::XwmAtomName::Targets)
    }));
    assert!(owner_requests(&requests).0.is_empty());

    super::super::super::selection_proxy::prepared_catalog_changed(
        &mut xwm,
        SelectionKind::Clipboard,
        60_000,
    )
    .expect("reconcile after external takeover");
    let follow_up = super::read_fixture_requests(&mut peer);
    assert!(owner_requests(&follow_up).0.is_empty());
    assert!(
        !fixture_requests(&mut peer)
            .2
            .contains(&xproto::CHANGE_PROPERTY_REQUEST)
    );
}

#[test]
fn clipboard_takeover_keeps_primary_proxy_owned_and_serving() {
    use super::super::super::data_bridge::SelectionKind;

    let (mut xwm, mut peer) = test_fixture(generation(330));
    initialize_selection_wire(&mut xwm, &mut peer);
    let clipboard_id = XwaylandProxySelectionId {
        kind: XwaylandSelectionKind::Clipboard,
        selection_generation: 41,
        source_key: crate::compositor::SelectionSourceKey(4101),
    };
    let primary_id = XwaylandProxySelectionId {
        kind: XwaylandSelectionKind::Primary,
        selection_generation: 42,
        source_key: crate::compositor::SelectionSourceKey(4201),
    };
    let mime_types = vec!["image/png".to_owned()];
    super::super::super::selection_wire::submit_proxy_selection_snapshots(
        &mut xwm,
        [
            super::proxy_snapshot(XwaylandSelectionKind::Clipboard, 41, 4101, &["image/png"]),
            super::proxy_snapshot(XwaylandSelectionKind::Primary, 42, 4201, &["image/png"]),
        ],
    )
    .expect("prepare both independent catalogs");
    super::finish_proxy_catalog(&mut xwm, &mut peer, clipboard_id, &mime_types);
    super::finish_proxy_catalog(&mut xwm, &mut peer, primary_id, &mime_types);

    let clipboard_timestamp = 0x1234_5001;
    let primary_timestamp = 0x1234_5002;
    for (kind, id, timestamp) in [
        (
            XwaylandSelectionKind::Clipboard,
            clipboard_id,
            clipboard_timestamp,
        ),
        (
            XwaylandSelectionKind::Primary,
            primary_id,
            primary_timestamp,
        ),
    ] {
        inject_proxy_time(&mut xwm, &mut peer, kind, timestamp);
        let (claims, confirmations) = take_owner_requests(&mut peer);
        let selection_kind = match kind {
            XwaylandSelectionKind::Clipboard => SelectionKind::Clipboard,
            XwaylandSelectionKind::Primary => SelectionKind::Primary,
        };
        let owner = xwm
            .data_bridge
            .selection_proxy
            .owner_window(selection_kind)
            .expect("generation-local proxy owner");
        assert_eq!(claims, [(owner, selection_atom(&xwm, kind), timestamp)]);
        assert_eq!(confirmations, 1);
        assert_eq!(
            super::super::super::selection_proxy::authority_for_test(&xwm, selection_kind),
            None
        );
        let (pending_id, pending_timestamp, _sequence) =
            super::super::super::selection_proxy::pending_owner_confirmation_for_test(
                &xwm,
                selection_kind,
            )
            .expect("asynchronous owner reply remains pending");
        assert_eq!(pending_id, id);
        assert_eq!(pending_timestamp, timestamp);
    }
    let (_, _, clipboard_sequence) =
        super::super::super::selection_proxy::pending_owner_confirmation_for_test(
            &xwm,
            SelectionKind::Clipboard,
        )
        .expect("Clipboard confirmation pending");
    let (_, _, primary_sequence) =
        super::super::super::selection_proxy::pending_owner_confirmation_for_test(
            &xwm,
            SelectionKind::Primary,
        )
        .expect("Primary confirmation pending");
    let clipboard_owner = xwm
        .data_bridge
        .selection_proxy
        .owner_window(SelectionKind::Clipboard)
        .expect("Clipboard owner window");
    let primary_owner = xwm
        .data_bridge
        .selection_proxy
        .owner_window(SelectionKind::Primary)
        .expect("Primary owner window");
    peer.write_all(&super::raw_get_selection_owner_reply(
        clipboard_sequence as u16,
        clipboard_owner,
    ))
    .expect("write Clipboard owner reply");
    peer.write_all(&super::raw_get_selection_owner_reply(
        primary_sequence as u16,
        primary_owner,
    ))
    .expect("write Primary owner reply");
    xwm.drain_events(64)
        .expect("confirm independent owners in request order");
    let primary_sequence = primary_sequence as u16;
    let _ = super::read_fixture_requests(&mut peer);
    assert_eq!(
        super::super::super::selection_proxy::authority_for_test(&xwm, SelectionKind::Clipboard),
        Some((clipboard_id, clipboard_timestamp))
    );
    assert_eq!(
        super::super::super::selection_proxy::authority_for_test(&xwm, SelectionKind::Primary),
        Some((primary_id, primary_timestamp))
    );

    let clipboard_target = super::super::super::selection_wire::prepared_proxy_selection_for_test(
        &xwm,
        SelectionKind::Clipboard,
    )
    .expect("prepared Clipboard catalog")
    .data_targets[0]
        .target;
    let primary_target = super::super::super::selection_wire::prepared_proxy_selection_for_test(
        &xwm,
        SelectionKind::Primary,
    )
    .expect("prepared Primary catalog")
    .data_targets[0]
        .target;
    for (index, (kind, target, time, property)) in [
        (
            XwaylandSelectionKind::Clipboard,
            clipboard_target,
            clipboard_timestamp,
            TARGET_PROPERTY,
        ),
        (
            XwaylandSelectionKind::Primary,
            primary_target,
            primary_timestamp,
            TARGET_PROPERTY + 1,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let event = request(&xwm, kind, target, property, time);
        super::super::super::selection_proxy::handle_selection_request(&mut xwm, event, 400)
            .expect("queue channel-owned source request");
        assert_eq!(
            super::super::super::selection_proxy::request_count_for_test(&xwm),
            index + 1,
            "request for {kind:?} should enter the bounded request queue"
        );
    }
    let source_requests =
        super::super::super::selection_proxy::take_managed_data_requests(&mut xwm);
    assert_eq!(source_requests.len(), 2);
    assert!(
        source_requests
            .iter()
            .any(|request| request.proxy_id == clipboard_id)
    );
    assert!(
        source_requests
            .iter()
            .any(|request| request.proxy_id == primary_id)
    );
    assert_eq!(xwm.data_bridge.selection_outgoing.active_count(), 2);

    let selection = selection_atom(&xwm, XwaylandSelectionKind::Clipboard);
    let observer = xwm
        .data_bridge
        .selection_wire
        .observer_window_for_test(SelectionKind::Clipboard)
        .expect("clipboard XFixes observer");
    peer.write_all(&super::raw_xfixes_selection_event(
        selection,
        observer,
        x11rb::protocol::xfixes::SelectionEvent::SET_SELECTION_OWNER,
        0xbeef,
        50_000,
        49_000,
        primary_sequence,
    ))
    .expect("write external Clipboard takeover");
    xwm.drain_events(64)
        .expect("revoke only Clipboard proxy state");

    assert_eq!(
        super::super::super::selection_proxy::authority_for_test(&xwm, SelectionKind::Clipboard),
        None
    );
    assert_eq!(
        super::super::super::selection_proxy::suppressed_id_for_test(
            &xwm,
            SelectionKind::Clipboard
        ),
        Some(clipboard_id)
    );
    assert_eq!(
        super::super::super::selection_proxy::held_timestamp_for_test(
            &xwm,
            SelectionKind::Clipboard
        ),
        None
    );
    assert_eq!(
        super::super::super::selection_proxy::authority_for_test(&xwm, SelectionKind::Primary),
        Some((primary_id, primary_timestamp))
    );
    assert_eq!(
        super::super::super::selection_proxy::held_timestamp_for_test(&xwm, SelectionKind::Primary),
        Some(primary_timestamp)
    );
    assert_eq!(xwm.data_bridge.selection_outgoing.active_count(), 1);
    assert_eq!(
        super::super::super::selection_proxy::request_count_for_test(&xwm),
        1,
        "the pending Primary request remains active"
    );
    let (changes, _, opcodes) = fixture_requests(&mut peer);
    assert!(!opcodes.contains(&xproto::SET_SELECTION_OWNER_REQUEST));
    let timestamp_property = xwm
        .atoms
        .get(super::super::super::atoms::XwmAtomName::SelectionProxyTime);
    let clipboard_owner = xwm
        .data_bridge
        .selection_proxy
        .owner_window(SelectionKind::Clipboard)
        .expect("Clipboard proxy owner window");
    assert!(!changes.iter().any(|change| {
        change.window == clipboard_owner && change.property == timestamp_property
    }));

    let targets = xwm
        .atoms
        .get(super::super::super::atoms::XwmAtomName::Targets);
    let mut primary_request = request(
        &xwm,
        XwaylandSelectionKind::Primary,
        targets,
        TARGET_PROPERTY + 2,
        primary_timestamp,
    );
    primary_request.sequence = primary_sequence;
    peer.write_all(&raw_selection_request(&primary_request))
        .expect("write Primary SelectionRequest after Clipboard takeover");
    xwm.drain_events(64)
        .expect("serve Primary while Clipboard is externally owned");
    let (changes, notifies, _) = fixture_requests(&mut peer);
    assert!(
        changes
            .iter()
            .any(|change| { change.window == REQUESTOR && change.property == TARGET_PROPERTY + 2 })
    );
    assert!(notifies.iter().any(|notify| {
        notify.selection == u32::from(xproto::AtomEnum::PRIMARY)
            && notify.property == TARGET_PROPERTY + 2
    }));
}

#[test]
fn generation_restart_reacquires_proxy_with_fresh_timestamp() {
    let (mut old_xwm, mut old_peer, id, _) =
        proxy_fixture_without_authority(XwaylandSelectionKind::Clipboard);
    let old_timestamp = 0x2233_4401;
    confirm_proxy_ownership(
        &mut old_xwm,
        &mut old_peer,
        XwaylandSelectionKind::Clipboard,
        id,
        old_timestamp,
    );
    let old_owner = old_xwm
        .data_bridge
        .selection_proxy
        .owner_window(super::super::super::data_bridge::SelectionKind::Clipboard)
        .expect("G1 owner window");
    old_xwm.clear_generation(generation(301));
    assert_eq!(
        super::super::super::selection_proxy::authority_for_test(
            &old_xwm,
            super::super::super::data_bridge::SelectionKind::Clipboard
        ),
        None
    );
    assert_eq!(
        super::super::super::selection_proxy::held_timestamp_for_test(
            &old_xwm,
            super::super::super::data_bridge::SelectionKind::Clipboard
        ),
        None
    );
    assert_eq!(
        old_xwm
            .data_bridge
            .selection_proxy
            .owner_window(super::super::super::data_bridge::SelectionKind::Clipboard),
        None
    );

    let (mut new_xwm, mut new_peer) = test_fixture(generation(302));
    initialize_selection_wire(&mut new_xwm, &mut new_peer);
    super::super::super::selection_wire::submit_proxy_selection_snapshots(
        &mut new_xwm,
        [super::proxy_snapshot(
            XwaylandSelectionKind::Clipboard,
            id.selection_generation,
            id.source_key.0,
            &["image/png"],
        )],
    )
    .expect("replay unchanged canonical source in G2");
    super::finish_proxy_catalog(&mut new_xwm, &mut new_peer, id, &["image/png".to_owned()]);
    let (changes, _, opcodes) = fixture_requests(&mut new_peer);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].mode, u8::from(xproto::PropMode::APPEND));
    assert!(!opcodes.contains(&xproto::SET_SELECTION_OWNER_REQUEST));
    assert_eq!(
        super::super::super::selection_proxy::authority_for_test(
            &new_xwm,
            super::super::super::data_bridge::SelectionKind::Clipboard
        ),
        None
    );

    let new_timestamp = 0x5566_7702;
    confirm_proxy_ownership(
        &mut new_xwm,
        &mut new_peer,
        XwaylandSelectionKind::Clipboard,
        id,
        new_timestamp,
    );
    assert_ne!(old_timestamp, new_timestamp);
    assert_eq!(
        super::super::super::selection_proxy::authority_for_test(
            &new_xwm,
            super::super::super::data_bridge::SelectionKind::Clipboard
        ),
        Some((id, new_timestamp))
    );
    assert_ne!(old_owner, 0);
    assert_ne!(
        new_xwm
            .data_bridge
            .selection_proxy
            .owner_window(super::super::super::data_bridge::SelectionKind::Clipboard)
            .expect("G2 owner window"),
        0
    );
}

fn selection_atom(xwm: &super::super::super::Xwm, kind: XwaylandSelectionKind) -> u32 {
    match kind {
        XwaylandSelectionKind::Clipboard => xwm
            .atoms
            .get(super::super::super::atoms::XwmAtomName::Clipboard),
        XwaylandSelectionKind::Primary => xproto::AtomEnum::PRIMARY.into(),
    }
}

fn request(
    xwm: &super::super::super::Xwm,
    kind: XwaylandSelectionKind,
    target: u32,
    property: u32,
    time: u32,
) -> xproto::SelectionRequestEvent {
    xproto::SelectionRequestEvent {
        response_type: xproto::SELECTION_REQUEST_EVENT,
        sequence: 0,
        time,
        owner: xwm
            .data_bridge
            .selection_proxy
            .owner_window(match kind {
                XwaylandSelectionKind::Clipboard => {
                    super::super::super::data_bridge::SelectionKind::Clipboard
                }
                XwaylandSelectionKind::Primary => {
                    super::super::super::data_bridge::SelectionKind::Primary
                }
            })
            .expect("private proxy owner"),
        requestor: REQUESTOR,
        selection: selection_atom(xwm, kind),
        target,
        property,
    }
}

fn targets_request_with_timestamps(
    ownership_timestamp: u32,
    request_time: u32,
) -> (Vec<PropertyChange>, Vec<Notify>) {
    let kind = XwaylandSelectionKind::Clipboard;
    let (mut xwm, mut peer, id, _) = proxy_fixture(kind);
    assert!(
        super::super::super::selection_proxy::install_test_authority(
            &mut xwm,
            super::super::super::data_bridge::SelectionKind::Clipboard,
            id,
            ownership_timestamp,
        )
    );
    let targets = xwm
        .atoms
        .get(super::super::super::atoms::XwmAtomName::Targets);
    let event = request(&xwm, kind, targets, TARGET_PROPERTY, request_time);
    super::super::super::selection_proxy::handle_selection_request(&mut xwm, event, 10)
        .expect("serve TARGETS request");
    let (changes, notifies, _) = fixture_requests(&mut peer);
    (changes, notifies)
}

#[test]
fn targets_and_timestamp_use_prepared_order_and_authority_timestamp() {
    let kind = XwaylandSelectionKind::Clipboard;
    let (mut xwm, mut peer, _, _) = proxy_fixture(kind);
    let targets = xwm
        .atoms
        .get(super::super::super::atoms::XwmAtomName::Targets);
    let timestamp = xwm
        .atoms
        .get(super::super::super::atoms::XwmAtomName::Timestamp);
    let prepared = super::super::super::selection_wire::prepared_proxy_selection_for_test(
        &xwm,
        super::super::super::data_bridge::SelectionKind::Clipboard,
    )
    .unwrap();
    let expected_targets = prepared.target_order;

    let event = request(&xwm, kind, targets, TARGET_PROPERTY, 12_345);
    super::super::super::selection_proxy::handle_selection_request(&mut xwm, event, 10)
        .expect("serve TARGETS");
    let (changes, notifies, _) = fixture_requests(&mut peer);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].property, TARGET_PROPERTY);
    assert_eq!(changes[0].property_type, u32::from(xproto::AtomEnum::ATOM));
    assert_eq!(changes[0].format, 32);
    assert_eq!(values32(&changes[0].value), expected_targets);
    assert_eq!(notifies.len(), 1);
    assert_eq!(notifies[0].property, TARGET_PROPERTY);
    assert!(
        xwm.data_bridge
            .selection_outgoing
            .pending_requests
            .is_empty()
    );

    let event = request(&xwm, kind, timestamp, TARGET_PROPERTY + 1, 12_345);
    super::super::super::selection_proxy::handle_selection_request(&mut xwm, event, 20)
        .expect("serve TIMESTAMP");
    let (changes, notifies, _) = fixture_requests(&mut peer);
    assert_eq!(changes.len(), 1);
    assert_eq!(
        changes[0].property_type,
        u32::from(xproto::AtomEnum::INTEGER)
    );
    assert_eq!(changes[0].format, 32);
    assert_eq!(values32(&changes[0].value), [12_345]);
    assert_eq!(notifies.len(), 1);
    assert_eq!(notifies[0].property, TARGET_PROPERTY + 1);
}

#[test]
fn selection_request_timestamp_accepts_post_wrap_time() {
    let (changes, notifies) = targets_request_with_timestamps(u32::MAX - 2, 2);

    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].property, TARGET_PROPERTY);
    assert_eq!(changes[0].property_type, u32::from(xproto::AtomEnum::ATOM));
    assert_eq!(changes[0].format, 32);
    assert_eq!(notifies.len(), 1);
    assert_eq!(notifies[0].property, TARGET_PROPERTY);
}

#[test]
fn selection_request_timestamp_rejects_pre_wrap_time() {
    let (changes, notifies) = targets_request_with_timestamps(2, u32::MAX - 2);

    assert!(changes.is_empty());
    assert_eq!(notifies.len(), 1);
    assert_eq!(notifies[0].property, x11rb::NONE);
}

#[test]
fn selection_request_current_time_remains_valid() {
    let (changes, notifies) = targets_request_with_timestamps(12_345, x11rb::CURRENT_TIME);

    assert_eq!(changes.len(), 1);
    assert_eq!(notifies.len(), 1);
    assert_eq!(notifies[0].property, TARGET_PROPERTY);
}

#[test]
fn selection_request_timestamp_accepts_ownership_timestamp() {
    let (changes, notifies) = targets_request_with_timestamps(12_345, 12_345);

    assert_eq!(changes.len(), 1);
    assert_eq!(notifies.len(), 1);
    assert_eq!(notifies[0].property, TARGET_PROPERTY);
}

#[test]
fn selection_request_timestamp_rejects_half_range_ambiguous_time() {
    let (changes, notifies) = targets_request_with_timestamps(2, 2u32.wrapping_add(0x8000_0000));

    assert!(changes.is_empty());
    assert_eq!(notifies.len(), 1);
    assert_eq!(notifies[0].property, x11rb::NONE);
}

#[test]
fn property_none_falls_back_to_target_and_unknown_atoms_fail_without_source_request() {
    let kind = XwaylandSelectionKind::Clipboard;
    let (mut xwm, mut peer, _, data_target) = proxy_fixture(kind);
    let event = request(&xwm, kind, data_target, x11rb::NONE, 12_346);
    super::super::super::selection_proxy::handle_selection_request(&mut xwm, event, 30)
        .expect("queue data conversion");
    let queued = super::super::super::selection_proxy::take_managed_data_requests(&mut xwm);
    assert_eq!(queued.len(), 1);
    assert_eq!(
        queued[0].proxy_id.source_key,
        crate::compositor::SelectionSourceKey(1701)
    );
    assert_eq!(queued[0].mime_type, "image/png");
    let transfer_id = queued[0].transfer_id;
    let transfer = xwm
        .data_bridge
        .selection_outgoing
        .transfers
        .get(&transfer_id)
        .unwrap();
    assert_eq!(transfer.property, data_target);
    assert_eq!(transfer.target, data_target);

    super::super::super::selection_proxy::resolve_managed_data_requests(
        &mut xwm,
        [(transfer_id, false)],
        31,
    )
    .expect("reject stale source explicitly");
    drop(queued);
    let (_, notifies, _) = fixture_requests(&mut peer);
    assert_eq!(notifies.len(), 1);
    assert_eq!(notifies[0].property, x11rb::NONE);

    let event = request(&xwm, kind, 0xdead, 0xf004, 12_347);
    super::super::super::selection_proxy::handle_selection_request(&mut xwm, event, 40)
        .expect("reject unknown target");
    assert!(super::super::super::selection_proxy::take_managed_data_requests(&mut xwm).is_empty());
    let (_, notifies, _) = fixture_requests(&mut peer);
    assert_eq!(notifies.len(), 1);
    assert_eq!(notifies[0].property, x11rb::NONE);

    let multiple = xwm
        .atoms
        .get(super::super::super::atoms::XwmAtomName::Multiple);
    let event = request(&xwm, kind, multiple, x11rb::NONE, 12_348);
    super::super::super::selection_proxy::handle_selection_request(&mut xwm, event, 41)
        .expect("reject MULTIPLE without its property");
    assert!(super::super::super::selection_proxy::pending_reply_sequence_for_test(&xwm).is_none());
    let (_, notifies, opcodes) = fixture_requests(&mut peer);
    assert!(!opcodes.contains(&xproto::GET_PROPERTY_REQUEST));
    assert_eq!(notifies.len(), 1);
    assert_eq!(notifies[0].target, multiple);
    assert_eq!(notifies[0].property, x11rb::NONE);
}

#[test]
fn reverse_request_contract_is_move_only_and_qualified() {
    use crate::xwayland::XwaylandProxySelectionDataRequest;

    fn assert_send<T: Send>() {}
    assert_send::<XwaylandProxySelectionDataRequest>();
}

#[test]
fn multiple_pairs_are_served_in_order_with_one_parent_notification() {
    let kind = XwaylandSelectionKind::Clipboard;
    let (mut xwm, mut peer, _, _) = proxy_fixture(kind);
    let targets = xwm
        .atoms
        .get(super::super::super::atoms::XwmAtomName::Targets);
    let timestamp = xwm
        .atoms
        .get(super::super::super::atoms::XwmAtomName::Timestamp);
    let atom_pair = xwm
        .atoms
        .get(super::super::super::atoms::XwmAtomName::AtomPair);
    let multiple = xwm
        .atoms
        .get(super::super::super::atoms::XwmAtomName::Multiple);
    let event = request(&xwm, kind, multiple, MULTIPLE_PROPERTY, 12_346);
    super::super::super::selection_proxy::handle_selection_request(&mut xwm, event, 50)
        .expect("start asynchronous MULTIPLE read");
    assert!(super::read_fixture_requests(&mut peer).contains(&xproto::GET_PROPERTY_REQUEST));
    let sequence = super::super::super::selection_proxy::pending_reply_sequence_for_test(&xwm)
        .expect("bounded GetProperty reply pending");
    let values = [
        targets,
        TARGET_PROPERTY,
        0xdead,
        TARGET_PROPERTY + 1,
        timestamp,
        TARGET_PROPERTY + 2,
    ];
    let mut reply = vec![0; 32 + values.len() * 4];
    reply[0] = 1;
    reply[1] = 32;
    reply[2..4].copy_from_slice(&(sequence as u16).to_le_bytes());
    reply[4..8].copy_from_slice(&(values.len() as u32).to_le_bytes());
    reply[8..12].copy_from_slice(&atom_pair.to_le_bytes());
    reply[16..20].copy_from_slice(&(values.len() as u32).to_le_bytes());
    for (index, value) in values.iter().enumerate() {
        reply[32 + index * 4..36 + index * 4].copy_from_slice(&value.to_le_bytes());
    }
    peer.write_all(&reply).expect("send MULTIPLE reply");
    xwm.drain_events(32).expect("process async MULTIPLE reply");

    let (changes, notifies, _) = fixture_requests(&mut peer);
    assert_eq!(changes.len(), 3);
    assert_eq!(changes[0].property, TARGET_PROPERTY);
    assert_eq!(changes[1].property, TARGET_PROPERTY + 2);
    assert_eq!(changes[2].property, MULTIPLE_PROPERTY);
    assert_eq!(changes[2].property_type, atom_pair);
    assert_eq!(changes[2].format, 32);
    assert_eq!(
        values32(&changes[2].value),
        [
            targets,
            TARGET_PROPERTY,
            x11rb::NONE,
            TARGET_PROPERTY + 1,
            timestamp,
            TARGET_PROPERTY + 2
        ]
    );
    assert_eq!(notifies.len(), 1);
    assert_eq!(notifies[0].target, multiple);
    assert_eq!(notifies[0].property, MULTIPLE_PROPERTY);
}

#[test]
fn multiple_pair_cannot_clobber_its_own_reply_property() {
    let kind = XwaylandSelectionKind::Clipboard;
    let (mut xwm, mut peer, _, _) = proxy_fixture(kind);
    let timestamp = xwm
        .atoms
        .get(super::super::super::atoms::XwmAtomName::Timestamp);
    let atom_pair = xwm
        .atoms
        .get(super::super::super::atoms::XwmAtomName::AtomPair);
    let multiple = xwm
        .atoms
        .get(super::super::super::atoms::XwmAtomName::Multiple);
    let event = request(&xwm, kind, multiple, MULTIPLE_PROPERTY, 12_346);
    super::super::super::selection_proxy::handle_selection_request(&mut xwm, event, 51)
        .expect("start MULTIPLE reply-property collision case");
    let _ = super::read_fixture_requests(&mut peer);
    let sequence = super::super::super::selection_proxy::pending_reply_sequence_for_test(&xwm)
        .expect("MULTIPLE property read pending");
    let values = [timestamp, MULTIPLE_PROPERTY];
    let mut reply = vec![0; 32 + values.len() * 4];
    reply[0] = 1;
    reply[1] = 32;
    reply[2..4].copy_from_slice(&(sequence as u16).to_le_bytes());
    reply[4..8].copy_from_slice(&(values.len() as u32).to_le_bytes());
    reply[8..12].copy_from_slice(&atom_pair.to_le_bytes());
    reply[16..20].copy_from_slice(&(values.len() as u32).to_le_bytes());
    for (index, value) in values.iter().enumerate() {
        reply[32 + index * 4..36 + index * 4].copy_from_slice(&value.to_le_bytes());
    }
    peer.write_all(&reply).expect("return colliding pair");
    xwm.drain_events(32)
        .expect("finish MULTIPLE pair processing");

    let (changes, notifies, _) = fixture_requests(&mut peer);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].property, MULTIPLE_PROPERTY);
    assert_eq!(changes[0].property_type, atom_pair);
    assert_eq!(
        values32(&changes[0].value),
        [x11rb::NONE, MULTIPLE_PROPERTY]
    );
    assert_eq!(notifies.len(), 1);
    assert_eq!(notifies[0].target, multiple);
    assert_eq!(notifies[0].property, MULTIPLE_PROPERTY);
}

#[test]
fn multiple_data_children_run_serially_and_parent_notifies_after_incr_marker() {
    let kind = XwaylandSelectionKind::Clipboard;
    let (mut xwm, mut peer, _, data_target) = proxy_fixture(kind);
    let multiple = xwm
        .atoms
        .get(super::super::super::atoms::XwmAtomName::Multiple);
    let atom_pair = xwm
        .atoms
        .get(super::super::super::atoms::XwmAtomName::AtomPair);
    let event = request(&xwm, kind, multiple, MULTIPLE_PROPERTY, 12_346);
    super::super::super::selection_proxy::handle_selection_request(&mut xwm, event, 55)
        .expect("start MULTIPLE data conversions");
    let _ = super::read_fixture_requests(&mut peer);
    let sequence = super::super::super::selection_proxy::pending_reply_sequence_for_test(&xwm)
        .expect("MULTIPLE property read pending");
    let values = [
        data_target,
        TARGET_PROPERTY,
        data_target,
        TARGET_PROPERTY + 2,
    ];
    let mut reply = vec![0; 32 + values.len() * 4];
    reply[0] = 1;
    reply[1] = 32;
    reply[2..4].copy_from_slice(&(sequence as u16).to_le_bytes());
    reply[4..8].copy_from_slice(&(values.len() as u32).to_le_bytes());
    reply[8..12].copy_from_slice(&atom_pair.to_le_bytes());
    reply[16..20].copy_from_slice(&(values.len() as u32).to_le_bytes());
    for (index, value) in values.iter().enumerate() {
        reply[32 + index * 4..36 + index * 4].copy_from_slice(&value.to_le_bytes());
    }
    peer.write_all(&reply).expect("return MULTIPLE pairs");
    xwm.drain_events(32).expect("start first child only");

    let mut requests = super::super::super::selection_proxy::take_managed_data_requests(&mut xwm);
    assert_eq!(requests.len(), 1, "the second child waits for the first");
    let first = requests.pop().unwrap();
    let first_id = first.transfer_id;
    let mut first_writer = File::from(first.sink);
    first_writer.write_all(b"abc").unwrap();
    drop(first_writer);
    super::super::super::selection_proxy::resolve_managed_data_requests(
        &mut xwm,
        [(first_id, true)],
        56,
    )
    .expect("complete first child directly");
    let (changes, notifies, _) = fixture_requests(&mut peer);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].property, TARGET_PROPERTY);
    assert_eq!(changes[0].value, b"abc");
    assert!(notifies.is_empty());

    let mut requests = super::super::super::selection_proxy::take_managed_data_requests(&mut xwm);
    assert_eq!(
        requests.len(),
        1,
        "second child begins after first is ready"
    );
    let second = requests.pop().unwrap();
    let second_id = second.transfer_id;
    let read_fd = xwm
        .data_bridge
        .selection_outgoing
        .transfers
        .get(&second_id)
        .unwrap()
        .read
        .as_raw_fd();
    xwm.data_bridge
        .selection_outgoing
        .transfers
        .get_mut(&second_id)
        .unwrap()
        .max_chunk_bytes = 3;
    let mut second_writer = File::from(second.sink);
    second_writer.write_all(b"xyz").unwrap();
    super::super::super::selection_proxy::resolve_managed_data_requests(
        &mut xwm,
        [(second_id, true)],
        57,
    )
    .expect("install second child INCR marker");
    let (changes, notifies, _) = fixture_requests(&mut peer);
    let incr = xwm.atoms.get(super::super::super::atoms::XwmAtomName::Incr);
    assert_eq!(changes.len(), 2);
    assert_eq!(changes[0].property, TARGET_PROPERTY + 2);
    assert_eq!(changes[0].property_type, incr);
    assert_eq!(values32(&changes[0].value), [3]);
    assert_eq!(changes[1].property, MULTIPLE_PROPERTY);
    assert_eq!(changes[1].property_type, atom_pair);
    assert_eq!(values32(&changes[1].value), values);
    assert_eq!(notifies.len(), 1);
    assert_eq!(notifies[0].target, multiple);
    assert_eq!(notifies[0].property, MULTIPLE_PROPERTY);

    let delete = || {
        Event::PropertyNotify(xproto::PropertyNotifyEvent {
            response_type: xproto::PROPERTY_NOTIFY_EVENT,
            sequence: 0,
            window: REQUESTOR,
            atom: TARGET_PROPERTY + 2,
            time: 0,
            state: xproto::Property::DELETE,
        })
    };
    super::super::super::events::normalize(&mut xwm, delete()).unwrap();
    let (changes, notifies, _) = fixture_requests(&mut peer);
    assert_eq!(changes[0].value, b"xyz");
    assert!(notifies.is_empty());
    drop(second_writer);
    super::super::super::events::normalize(&mut xwm, delete()).unwrap();
    let _ = fixture_requests(&mut peer);
    xwm.data_bridge
        .selection_outgoing
        .bind_reactor_token(second_id, read_fd, Some(0x601));
    let bridge_generation =
        super::super::super::data_bridge::BridgeGeneration::from(xwm.generation);
    super::super::super::selection_outgoing::handle_source_ready(
        &mut xwm,
        second_id,
        bridge_generation,
        0x601,
        libc::EPOLLIN as u32,
        58,
    )
    .unwrap();
    let (changes, notifies, _) = fixture_requests(&mut peer);
    assert_eq!(changes.len(), 1);
    assert!(changes[0].value.is_empty());
    assert!(notifies.is_empty());
}

#[test]
fn source_eagain_registers_read_interest_and_destroy_cancels_all_requestor_work() {
    let kind = XwaylandSelectionKind::Clipboard;
    let (mut xwm, mut peer, _, data_target) = proxy_fixture(kind);
    let bridge_generation =
        super::super::super::data_bridge::BridgeGeneration::from(xwm.generation);
    let window = super::super::tests::prepare_managed_window(&mut xwm, REQUESTOR, true, true, true);
    let event = request(&xwm, kind, data_target, TARGET_PROPERTY, 12_346);
    super::super::super::selection_proxy::handle_selection_request(&mut xwm, event, 90)
        .expect("start idle data conversion");
    let requests = super::super::super::selection_proxy::take_managed_data_requests(&mut xwm);
    let source = requests.into_iter().next().unwrap();
    let transfer_id = source.transfer_id;
    let read_fd = xwm
        .data_bridge
        .selection_outgoing
        .transfers
        .get(&transfer_id)
        .unwrap()
        .read
        .as_raw_fd();
    super::super::super::selection_proxy::resolve_managed_data_requests(
        &mut xwm,
        [(transfer_id, true)],
        91,
    )
    .expect("accept source with no bytes ready");
    assert_eq!(
        xwm.data_bridge
            .selection_outgoing
            .source_interests()
            .collect::<Vec<_>>(),
        [(transfer_id, read_fd)]
    );
    xwm.data_bridge
        .selection_outgoing
        .bind_reactor_token(transfer_id, read_fd, Some(0x701));

    let multiple = xwm
        .atoms
        .get(super::super::super::atoms::XwmAtomName::Multiple);
    let event = request(&xwm, kind, multiple, MULTIPLE_PROPERTY, 12_347);
    super::super::super::selection_proxy::handle_selection_request(&mut xwm, event, 92)
        .expect("start an asynchronous MULTIPLE read for the same client");
    let _ = super::read_fixture_requests(&mut peer);
    assert!(super::super::super::selection_proxy::pending_reply_sequence_for_test(&xwm).is_some());
    assert_eq!(
        super::super::super::selection_proxy::requestor_ref_count_for_test(&xwm, REQUESTOR),
        3
    );

    super::super::super::events::normalize(
        &mut xwm,
        Event::DestroyNotify(xproto::DestroyNotifyEvent {
            response_type: xproto::DESTROY_NOTIFY_EVENT,
            sequence: 0,
            event: 1,
            window: REQUESTOR,
        }),
    )
    .expect("sidecar cancels selection state then preserves XWM destruction");
    assert_eq!(xwm.data_bridge.selection_outgoing.active_count(), 0);
    assert_eq!(
        super::super::super::selection_proxy::request_count_for_test(&xwm),
        0
    );
    assert_eq!(
        super::super::super::selection_proxy::pending_reply_sequence_for_test(&xwm),
        None
    );
    assert_eq!(
        super::super::super::selection_proxy::requestor_ref_count_for_test(&xwm, REQUESTOR),
        0
    );
    assert!(super::super::super::selection_proxy::take_managed_data_requests(&mut xwm).is_empty());
    assert!(xwm.windows.get(window).is_none());
    drop(source.sink);

    let mut next_event = request(&xwm, kind, data_target, TARGET_PROPERTY + 2, 12_348);
    next_event.requestor = REQUESTOR + 1;
    super::super::super::selection_proxy::handle_selection_request(&mut xwm, next_event, 93)
        .expect("start replacement transfer");
    let mut requests = super::super::super::selection_proxy::take_managed_data_requests(&mut xwm);
    let replacement = requests.pop().unwrap();
    let replacement_id = replacement.transfer_id;
    super::super::super::selection_proxy::resolve_managed_data_requests(
        &mut xwm,
        [(replacement_id, true)],
        94,
    )
    .expect("register replacement source reader");
    let replacement_fd = xwm
        .data_bridge
        .selection_outgoing
        .transfers
        .get(&replacement_id)
        .unwrap()
        .read
        .as_raw_fd();
    xwm.data_bridge.selection_outgoing.bind_reactor_token(
        replacement_id,
        replacement_fd,
        Some(0x702),
    );
    assert!(
        !super::super::super::selection_outgoing::handle_source_ready(
            &mut xwm,
            transfer_id,
            bridge_generation,
            0x701,
            libc::EPOLLIN as u32,
            95,
        )
        .unwrap()
    );
    assert_eq!(xwm.data_bridge.selection_outgoing.active_count(), 1);
    assert_eq!(
        xwm.data_bridge
            .selection_outgoing
            .source_interests()
            .collect::<Vec<_>>(),
        [(replacement_id, replacement_fd)]
    );
    super::super::super::selection_proxy::requestor_destroyed(&mut xwm, REQUESTOR + 1).unwrap();
    drop(replacement.sink);
}

#[test]
fn generation_retirement_drops_queued_fds_replies_and_ordering_state() {
    let kind = XwaylandSelectionKind::Clipboard;
    let (mut xwm, mut peer, _, data_target) = proxy_fixture(kind);
    let event = request(&xwm, kind, data_target, TARGET_PROPERTY, 12_346);
    super::super::super::selection_proxy::handle_selection_request(&mut xwm, event, 100)
        .expect("queue generation-owned data transfer");
    let multiple = xwm
        .atoms
        .get(super::super::super::atoms::XwmAtomName::Multiple);
    let event = request(&xwm, kind, multiple, MULTIPLE_PROPERTY, 12_347);
    super::super::super::selection_proxy::handle_selection_request(&mut xwm, event, 101)
        .expect("queue generation-owned MULTIPLE reply");
    let _ = super::read_fixture_requests(&mut peer);
    assert!(super::super::super::selection_proxy::pending_reply_sequence_for_test(&xwm).is_some());

    xwm.clear_generation(generation(301));
    assert_eq!(xwm.data_bridge.selection_outgoing.active_count(), 0);
    assert!(
        xwm.data_bridge
            .selection_outgoing
            .pending_requests
            .is_empty()
    );
    assert_eq!(
        super::super::super::selection_proxy::request_count_for_test(&xwm),
        0
    );
    assert_eq!(
        super::super::super::selection_proxy::pending_reply_sequence_for_test(&xwm),
        None
    );
    assert!(
        xwm.data_bridge
            .selection_outgoing
            .source_interests()
            .next()
            .is_none()
    );

    let (mut next_generation, _next_peer) = test_fixture(generation(302));
    assert_eq!(
        next_generation
            .data_bridge
            .selection_outgoing
            .active_count(),
        0
    );
    assert_eq!(
        super::super::super::selection_proxy::request_count_for_test(&next_generation),
        0
    );
    assert_eq!(
        super::super::super::selection_proxy::pending_reply_sequence_for_test(&next_generation),
        None
    );
    super::super::super::selection_proxy::initialize(&mut next_generation)
        .expect("create inert G2 proxy owner windows");
    assert_eq!(
        next_generation
            .data_bridge
            .selection_outgoing
            .active_count(),
        0
    );
}

#[test]
fn identical_notification_signatures_preserve_arrival_order() {
    let kind = XwaylandSelectionKind::Clipboard;
    let (mut xwm, mut peer, _, data_target) = proxy_fixture(kind);
    let first_event = request(&xwm, kind, data_target, TARGET_PROPERTY, 12_346);
    let second_event = request(&xwm, kind, data_target, TARGET_PROPERTY + 1, 12_346);
    super::super::super::selection_proxy::handle_selection_request(&mut xwm, first_event, 110)
        .unwrap();
    super::super::super::selection_proxy::handle_selection_request(&mut xwm, second_event, 111)
        .unwrap();
    let mut requests = super::super::super::selection_proxy::take_managed_data_requests(&mut xwm);
    assert_eq!(requests.len(), 2);
    let first = requests.remove(0);
    let second = requests.remove(0);
    let first_id = first.transfer_id;
    let second_id = second.transfer_id;

    let mut second_writer = File::from(second.sink);
    second_writer.write_all(b"second").unwrap();
    drop(second_writer);
    super::super::super::selection_proxy::resolve_managed_data_requests(
        &mut xwm,
        [(second_id, true)],
        112,
    )
    .unwrap();
    let (changes, notifies, _) = fixture_requests(&mut peer);
    assert_eq!(changes[0].property, TARGET_PROPERTY + 1);
    assert!(
        notifies.is_empty(),
        "later ready request waits behind the first"
    );

    let mut first_writer = File::from(first.sink);
    first_writer.write_all(b"first").unwrap();
    drop(first_writer);
    super::super::super::selection_proxy::resolve_managed_data_requests(
        &mut xwm,
        [(first_id, true)],
        113,
    )
    .unwrap();
    let (_, notifies, _) = fixture_requests(&mut peer);
    assert_eq!(notifies.len(), 2);
    assert_eq!(notifies[0].property, TARGET_PROPERTY);
    assert_eq!(notifies[1].property, TARGET_PROPERTY + 1);
}

#[test]
fn direct_payload_reads_exact_source_bytes_and_empty_payload_succeeds() {
    let kind = XwaylandSelectionKind::Clipboard;
    let (mut xwm, mut peer, _, data_target) = proxy_fixture(kind);
    let event = request(&xwm, kind, data_target, TARGET_PROPERTY, 12_346);
    super::super::super::selection_proxy::handle_selection_request(&mut xwm, event, 60)
        .expect("start direct transfer");
    let mut requests = super::super::super::selection_proxy::take_managed_data_requests(&mut xwm);
    let source = requests.pop().expect("one source request");
    let transfer_id = source.transfer_id;
    let mut writer = File::from(source.sink);
    writer.write_all(b"abc").expect("write exact payload");
    drop(writer);
    super::super::super::selection_proxy::resolve_managed_data_requests(
        &mut xwm,
        [(transfer_id, true)],
        61,
    )
    .expect("accept source");
    let (changes, notifies, _) = fixture_requests(&mut peer);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].property, TARGET_PROPERTY);
    assert_eq!(changes[0].property_type, data_target);
    assert_eq!(changes[0].format, 8);
    assert_eq!(changes[0].value, b"abc");
    assert_eq!(notifies.len(), 1);
    assert_eq!(notifies[0].property, TARGET_PROPERTY);
    assert!(!changes.iter().any(|change| {
        change.property_type == xwm.atoms.get(super::super::super::atoms::XwmAtomName::Incr)
    }));

    let event = request(&xwm, kind, data_target, TARGET_PROPERTY + 1, 12_347);
    super::super::super::selection_proxy::handle_selection_request(&mut xwm, event, 70)
        .expect("start empty transfer");
    let mut requests = super::super::super::selection_proxy::take_managed_data_requests(&mut xwm);
    let source = requests.pop().expect("empty source request");
    let transfer_id = source.transfer_id;
    drop(source.sink);
    super::super::super::selection_proxy::resolve_managed_data_requests(
        &mut xwm,
        [(transfer_id, true)],
        71,
    )
    .expect("accept empty source");
    let (changes, notifies, _) = fixture_requests(&mut peer);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].property, TARGET_PROPERTY + 1);
    assert_eq!(changes[0].property_type, data_target);
    assert_eq!(changes[0].format, 8);
    assert!(changes[0].value.is_empty());
    assert_eq!(notifies.len(), 1);
    assert_eq!(notifies[0].property, TARGET_PROPERTY + 1);
}

#[test]
fn accepted_source_read_fd_wakes_epoll_and_completes_direct_transfer() {
    let kind = XwaylandSelectionKind::Clipboard;
    let (mut xwm, mut peer, _, data_target) = proxy_fixture(kind);
    let event = request(&xwm, kind, data_target, TARGET_PROPERTY, 12_346);
    super::super::super::selection_proxy::handle_selection_request(&mut xwm, event, 120)
        .expect("start delayed source transfer");
    let mut requests = super::super::super::selection_proxy::take_managed_data_requests(&mut xwm);
    let source = requests.pop().unwrap();
    let transfer_id = source.transfer_id;
    let read_fd = xwm
        .data_bridge
        .selection_outgoing
        .transfers
        .get(&transfer_id)
        .unwrap()
        .read
        .as_raw_fd();
    super::super::super::selection_proxy::resolve_managed_data_requests(
        &mut xwm,
        [(transfer_id, true)],
        121,
    )
    .expect("accept source while its pipe is empty");
    assert_eq!(
        xwm.data_bridge
            .selection_outgoing
            .source_interests()
            .collect::<Vec<_>>(),
        [(transfer_id, read_fd)]
    );
    xwm.data_bridge
        .selection_outgoing
        .bind_reactor_token(transfer_id, read_fd, Some(0x801));

    let epoll_fd = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
    assert!(epoll_fd >= 0);
    let epoll = unsafe { std::os::fd::OwnedFd::from_raw_fd(epoll_fd) };
    let mut registration = libc::epoll_event {
        events: (libc::EPOLLIN | libc::EPOLLERR | libc::EPOLLHUP | libc::EPOLLRDHUP) as u32,
        u64: 0x801,
    };
    assert_eq!(
        unsafe {
            libc::epoll_ctl(
                epoll.as_raw_fd(),
                libc::EPOLL_CTL_ADD,
                read_fd,
                &mut registration,
            )
        },
        0
    );
    let mut readiness = libc::epoll_event { events: 0, u64: 0 };
    assert_eq!(
        unsafe { libc::epoll_wait(epoll.as_raw_fd(), &mut readiness, 1, 0) },
        0,
        "empty source pipe is not readable yet"
    );
    let mut writer = File::from(source.sink);
    writer.write_all(b"wake").unwrap();
    drop(writer);
    assert_eq!(
        unsafe { libc::epoll_wait(epoll.as_raw_fd(), &mut readiness, 1, 1000) },
        1,
        "source write wakes the registered epoll reader"
    );
    let ready_token = unsafe { std::ptr::addr_of!(readiness.u64).read_unaligned() };
    let ready_events = unsafe { std::ptr::addr_of!(readiness.events).read_unaligned() };
    assert_eq!(ready_token, 0x801);
    assert_ne!(ready_events & libc::EPOLLIN as u32, 0);
    let bridge_generation =
        super::super::super::data_bridge::BridgeGeneration::from(xwm.generation);
    super::super::super::selection_outgoing::handle_source_ready(
        &mut xwm,
        transfer_id,
        bridge_generation,
        ready_token,
        ready_events,
        122,
    )
    .expect("read source following actual epoll readiness");
    let (changes, notifies, _) = fixture_requests(&mut peer);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].value, b"wake");
    assert_eq!(changes[0].property_type, data_target);
    assert_eq!(notifies.len(), 1);
}

#[test]
fn outgoing_incr_is_paced_by_property_delete_and_ends_with_empty_data() {
    let kind = XwaylandSelectionKind::Clipboard;
    let (mut xwm, mut peer, _, data_target) = proxy_fixture(kind);
    let now_ns = crate::native::event_loop::monotonic_now_ns().unwrap_or_default();
    let _requestor_window =
        super::super::tests::prepare_managed_window(&mut xwm, REQUESTOR, true, true, true);
    let _ = fixture_requests(&mut peer);
    let event = request(&xwm, kind, data_target, TARGET_PROPERTY, 12_346);
    super::super::super::selection_proxy::handle_selection_request(&mut xwm, event, now_ns)
        .expect("start outgoing INCR");
    let mut requests = super::super::super::selection_proxy::take_managed_data_requests(&mut xwm);
    let source = requests.pop().expect("one source request");
    let transfer_id = source.transfer_id;
    let reader_fd = xwm
        .data_bridge
        .selection_outgoing
        .transfers
        .get(&transfer_id)
        .unwrap()
        .read
        .as_raw_fd();
    let bridge_generation =
        super::super::super::data_bridge::BridgeGeneration::from(xwm.generation);
    xwm.data_bridge
        .selection_outgoing
        .transfers
        .get_mut(&transfer_id)
        .unwrap()
        .max_chunk_bytes = 3;
    let mut writer = File::from(source.sink);
    writer.write_all(b"abc").expect("fill direct bound");
    super::super::super::selection_proxy::resolve_managed_data_requests(
        &mut xwm,
        [(transfer_id, true)],
        now_ns.saturating_add(1),
    )
    .expect("accept source");
    let (changes, notifies, _) = fixture_requests(&mut peer);
    let incr = xwm.atoms.get(super::super::super::atoms::XwmAtomName::Incr);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].property, TARGET_PROPERTY);
    assert_eq!(changes[0].property_type, incr);
    assert_eq!(changes[0].format, 32);
    assert_eq!(values32(&changes[0].value), [3]);
    assert_eq!(notifies.len(), 1);
    assert_eq!(notifies[0].property, TARGET_PROPERTY);

    let delete = || xproto::PropertyNotifyEvent {
        response_type: xproto::PROPERTY_NOTIFY_EVENT,
        sequence: 0,
        window: REQUESTOR,
        atom: TARGET_PROPERTY,
        time: 0,
        state: xproto::Property::DELETE,
    };
    let mut serialized_delete = delete().serialize().to_vec();
    serialized_delete.resize(32, 0);
    peer.write_all(&serialized_delete)
        .expect("send serialized INCR marker deletion");
    let drained = xwm
        .drain_events(32)
        .expect("consume marker deletion through event drain");
    assert_eq!(drained.events_processed, 1);
    assert_eq!(xwm.data_bridge.selection_outgoing.active_count(), 1);
    let (changes, notifies, _) = fixture_requests(&mut peer);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].property_type, data_target);
    assert_eq!(changes[0].value, b"abc");
    assert!(notifies.is_empty());

    let unrelated = xproto::PropertyNotifyEvent {
        response_type: xproto::PROPERTY_NOTIFY_EVENT,
        sequence: 0,
        window: REQUESTOR,
        atom: xwm
            .atoms
            .get(super::super::super::atoms::XwmAtomName::NetWmName),
        time: 0,
        state: xproto::Property::DELETE,
    };
    let mut serialized_unrelated = unrelated.serialize().to_vec();
    serialized_unrelated.resize(32, 0);
    peer.write_all(&serialized_unrelated)
        .expect("send unrelated serialized property deletion");
    xwm.drain_events(32)
        .expect("route unrelated property deletion normally");
    let (_, _, opcodes) = fixture_requests(&mut peer);
    assert!(opcodes.contains(&xproto::GET_PROPERTY_REQUEST));
    assert_eq!(xwm.data_bridge.selection_outgoing.active_count(), 1);

    super::super::super::events::normalize(&mut xwm, Event::PropertyNotify(delete()))
        .expect("acknowledge chunk A");
    assert!(fixture_requests(&mut peer).0.is_empty());
    let interest = xwm
        .data_bridge
        .selection_outgoing
        .source_interests()
        .next()
        .expect("source reader resumes after delete acknowledgement");
    assert_eq!(interest, (transfer_id, reader_fd));
    xwm.data_bridge
        .selection_outgoing
        .bind_reactor_token(transfer_id, reader_fd, Some(0x501));

    writer.write_all(b"def").expect("write chunk B");
    drop(writer);
    super::super::super::selection_outgoing::handle_source_ready(
        &mut xwm,
        transfer_id,
        bridge_generation,
        0x501,
        libc::EPOLLIN as u32,
        now_ns.saturating_add(2),
    )
    .expect("read source chunk B");
    let (changes, notifies, _) = fixture_requests(&mut peer);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].property_type, data_target);
    assert_eq!(changes[0].value, b"def");
    assert!(notifies.is_empty());

    super::super::super::events::normalize(&mut xwm, Event::PropertyNotify(delete()))
        .expect("acknowledge chunk B");
    assert!(fixture_requests(&mut peer).0.is_empty());
    let interest = xwm
        .data_bridge
        .selection_outgoing
        .source_interests()
        .next()
        .expect("EOF read waits for source readiness");
    xwm.data_bridge
        .selection_outgoing
        .bind_reactor_token(transfer_id, interest.1, Some(0x502));
    super::super::super::selection_outgoing::handle_source_ready(
        &mut xwm,
        transfer_id,
        bridge_generation,
        0x502,
        libc::EPOLLIN as u32,
        now_ns.saturating_add(3),
    )
    .expect("observe EOF and publish terminal chunk");
    let (changes, notifies, _) = fixture_requests(&mut peer);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].property_type, data_target);
    assert_eq!(changes[0].format, 8);
    assert!(changes[0].value.is_empty());
    assert!(notifies.is_empty());
    assert_eq!(xwm.data_bridge.selection_outgoing.active_count(), 0);
}

#[test]
fn request_timeout_fails_before_acceptance_and_cancels_after_incr_without_renotify() {
    let kind = XwaylandSelectionKind::Clipboard;
    let (mut xwm, mut peer, _, data_target) = proxy_fixture(kind);
    let event = request(&xwm, kind, data_target, TARGET_PROPERTY, 12_346);
    super::super::super::selection_proxy::handle_selection_request(&mut xwm, event, 100)
        .expect("start source request");
    let _ = fixture_requests(&mut peer);

    let deadline =
        100 + super::super::super::selection_outgoing::OUTGOING_SELECTION_IDLE_TIMEOUT_NS;
    let outcome = xwm.handle_deadlines(deadline);
    assert!(outcome.error.is_none());
    assert_eq!(xwm.data_bridge.selection_outgoing.active_count(), 0);
    assert!(
        xwm.data_bridge
            .selection_outgoing
            .pending_requests
            .is_empty()
    );
    let (changes, notifies, _) = fixture_requests(&mut peer);
    assert!(changes.is_empty());
    assert_eq!(notifies.len(), 1);
    assert_eq!(notifies[0].property, x11rb::NONE);

    let (mut xwm, mut peer, _, data_target) = proxy_fixture(kind);
    let event = request(&xwm, kind, data_target, TARGET_PROPERTY + 1, 12_346);
    super::super::super::selection_proxy::handle_selection_request(&mut xwm, event, 200)
        .expect("start transfer that will time out after INCR");
    let mut requests = super::super::super::selection_proxy::take_managed_data_requests(&mut xwm);
    let source = requests.pop().expect("move-only source request");
    let transfer_id = source.transfer_id;
    xwm.data_bridge
        .selection_outgoing
        .transfers
        .get_mut(&transfer_id)
        .expect("active transfer")
        .max_chunk_bytes = 3;
    let mut writer = File::from(source.sink);
    writer.write_all(b"xyz").expect("write threshold payload");
    super::super::super::selection_proxy::resolve_managed_data_requests(
        &mut xwm,
        [(transfer_id, true)],
        200,
    )
    .expect("publish INCR marker");
    let (changes, notifies, _) = fixture_requests(&mut peer);
    assert_eq!(changes.len(), 1);
    assert_eq!(
        changes[0].property_type,
        xwm.atoms.get(super::super::super::atoms::XwmAtomName::Incr)
    );
    assert_eq!(notifies.len(), 1);
    assert_eq!(notifies[0].property, TARGET_PROPERTY + 1);

    let outcome = xwm.handle_deadlines(
        200 + super::super::super::selection_outgoing::OUTGOING_SELECTION_IDLE_TIMEOUT_NS,
    );
    assert!(outcome.error.is_none());
    assert_eq!(xwm.data_bridge.selection_outgoing.active_count(), 0);
    let _ = x11rb::connection::Connection::flush(&xwm.connection);
    let (_, notifies, _) = fixture_requests(&mut peer);
    assert!(
        notifies.is_empty(),
        "advertised INCR is never renotified as failure"
    );
    drop(writer);
}

#[test]
fn clipboard_and_primary_outgoing_sources_progress_independently() {
    use super::super::super::data_bridge::SelectionKind;
    use crate::xwayland::XwaylandProxySelectionId;

    let (mut xwm, mut peer) = test_fixture(generation(329));
    super::initialize_selection_wire(&mut xwm, &mut peer);
    let clipboard_id = XwaylandProxySelectionId {
        kind: XwaylandSelectionKind::Clipboard,
        selection_generation: 41,
        source_key: crate::compositor::SelectionSourceKey(4101),
    };
    let primary_id = XwaylandProxySelectionId {
        kind: XwaylandSelectionKind::Primary,
        selection_generation: 42,
        source_key: crate::compositor::SelectionSourceKey(4201),
    };
    let mime_types = vec!["image/png".to_owned()];
    super::super::super::selection_wire::submit_proxy_selection_snapshots(
        &mut xwm,
        [
            super::proxy_snapshot(XwaylandSelectionKind::Clipboard, 41, 4101, &["image/png"]),
            super::proxy_snapshot(XwaylandSelectionKind::Primary, 42, 4201, &["image/png"]),
        ],
    )
    .expect("prepare both independent catalogs");
    super::finish_proxy_catalog(&mut xwm, &mut peer, clipboard_id, &mime_types);
    super::finish_proxy_catalog(&mut xwm, &mut peer, primary_id, &mime_types);
    let clipboard_kind = SelectionKind::Clipboard;
    let primary_kind = SelectionKind::Primary;
    assert!(
        super::super::super::selection_proxy::install_test_authority(
            &mut xwm,
            clipboard_kind,
            clipboard_id,
            12_345,
        )
    );
    assert!(
        super::super::super::selection_proxy::install_test_authority(
            &mut xwm,
            primary_kind,
            primary_id,
            12_346,
        )
    );
    let clipboard_target = super::super::super::selection_wire::prepared_proxy_selection_for_test(
        &xwm,
        clipboard_kind,
    )
    .unwrap()
    .data_targets[0]
        .target;
    let primary_target =
        super::super::super::selection_wire::prepared_proxy_selection_for_test(&xwm, primary_kind)
            .unwrap()
            .data_targets[0]
            .target;
    let clipboard_event = request(
        &xwm,
        XwaylandSelectionKind::Clipboard,
        clipboard_target,
        0xf110,
        12_347,
    );
    super::super::super::selection_proxy::handle_selection_request(&mut xwm, clipboard_event, 300)
        .expect("start clipboard source");
    let primary_event = request(
        &xwm,
        XwaylandSelectionKind::Primary,
        primary_target,
        0xf111,
        12_347,
    );
    super::super::super::selection_proxy::handle_selection_request(&mut xwm, primary_event, 301)
        .expect("start primary source");
    let _ = fixture_requests(&mut peer);
    let requests = super::super::super::selection_proxy::take_managed_data_requests(&mut xwm);
    assert_eq!(requests.len(), 2);
    assert!(
        requests
            .iter()
            .any(|request| request.proxy_id == clipboard_id)
    );
    assert!(
        requests
            .iter()
            .any(|request| request.proxy_id == primary_id)
    );
    let mut clipboard_request = None;
    let mut primary_request = None;
    for request in requests {
        if request.proxy_id == clipboard_id {
            clipboard_request = Some(request);
        } else if request.proxy_id == primary_id {
            primary_request = Some(request);
        }
    }
    let clipboard_request = clipboard_request.expect("clipboard move-only request");
    let primary_request = primary_request.expect("primary move-only request");
    let clipboard_transfer = clipboard_request.transfer_id;
    let primary_transfer = primary_request.transfer_id;
    let mut clipboard_writer = File::from(clipboard_request.sink);
    let mut primary_writer = File::from(primary_request.sink);
    primary_writer.write_all(b"p").expect("write primary bytes");
    drop(primary_writer);
    super::super::super::selection_proxy::resolve_managed_data_requests(
        &mut xwm,
        [(primary_transfer, true)],
        302,
    )
    .expect("primary source completes while clipboard remains pending");
    let (changes, notifies, _) = fixture_requests(&mut peer);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].property, 0xf111);
    assert_eq!(changes[0].value, b"p");
    assert_eq!(notifies.len(), 1);
    assert_eq!(notifies[0].selection, u32::from(xproto::AtomEnum::PRIMARY));
    assert_eq!(xwm.data_bridge.selection_outgoing.active_count(), 1);

    super::super::super::selection_proxy::resolve_managed_data_requests(
        &mut xwm,
        [(clipboard_transfer, true)],
        303,
    )
    .expect("clipboard source independently enters read wait");
    let read_fd = xwm
        .data_bridge
        .selection_outgoing
        .transfers
        .get(&clipboard_transfer)
        .unwrap()
        .read
        .as_raw_fd();
    let generation = super::super::super::data_bridge::BridgeGeneration::from(xwm.generation);
    xwm.data_bridge
        .selection_outgoing
        .bind_reactor_token(clipboard_transfer, read_fd, Some(0x633));
    clipboard_writer
        .write_all(b"c")
        .expect("write clipboard bytes");
    drop(clipboard_writer);
    super::super::super::selection_outgoing::handle_source_ready(
        &mut xwm,
        clipboard_transfer,
        generation,
        0x633,
        1,
        304,
    )
    .expect("finish clipboard read without changing primary result");
    let (changes, notifies, _) = fixture_requests(&mut peer);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].property, 0xf110);
    assert_eq!(changes[0].value, b"c");
    assert_eq!(notifies.len(), 1);
    assert_eq!(
        notifies[0].selection,
        xwm.atoms
            .get(super::super::super::atoms::XwmAtomName::Clipboard)
    );
}

#[test]
fn outgoing_manager_and_multiple_pair_bounds_are_explicit() {
    assert_eq!(
        super::super::super::selection_outgoing::MAX_ACTIVE_OUTGOING_SELECTION_TRANSFERS,
        64
    );
    assert_eq!(super::super::super::selection_proxy::MAX_MULTIPLE_PAIRS, 64);
    assert_eq!(
        super::super::super::selection_proxy::MAX_PENDING_OUTGOING_SELECTION_REPLIES,
        4
    );
}

#[test]
fn overloaded_requests_and_async_multiple_replies_remain_bounded() {
    let kind = XwaylandSelectionKind::Clipboard;
    let (mut xwm, mut peer, _, data_target) = proxy_fixture(kind);
    for index in 0..=super::super::super::selection_proxy::MAX_PENDING_PROXY_SELECTION_REQUESTS {
        let event = request(&xwm, kind, data_target, 0xe000 + index as u32, 12_346);
        super::super::super::selection_proxy::handle_selection_request(
            &mut xwm,
            event,
            200 + index as u64,
        )
        .expect("overload fails oldest request and keeps serving");
    }
    assert_eq!(
        super::super::super::selection_proxy::request_count_for_test(&xwm),
        super::super::super::selection_proxy::MAX_PENDING_PROXY_SELECTION_REQUESTS
    );
    assert_eq!(
        xwm.data_bridge.selection_outgoing.active_count(),
        super::super::super::selection_outgoing::MAX_ACTIVE_OUTGOING_SELECTION_TRANSFERS
    );
    assert_eq!(
        xwm.data_bridge.selection_outgoing.pending_requests.len(),
        super::super::super::selection_outgoing::MAX_ACTIVE_OUTGOING_SELECTION_TRANSFERS
    );
    assert_eq!(
        super::super::super::selection_proxy::requestor_ref_count_for_test(&xwm, REQUESTOR),
        128
    );

    let multiple = xwm
        .atoms
        .get(super::super::super::atoms::XwmAtomName::Multiple);
    for index in 0..5 {
        let mut event = request(
            &xwm,
            kind,
            multiple,
            MULTIPLE_PROPERTY + index,
            12_500 + index,
        );
        event.requestor = REQUESTOR + 10 + index;
        super::super::super::selection_proxy::handle_selection_request(
            &mut xwm,
            event,
            300 + u64::from(index),
        )
        .expect("schedule bounded MULTIPLE reply");
    }
    assert_eq!(
        super::super::super::selection_proxy::pending_reply_count_for_test(&xwm),
        super::super::super::selection_proxy::MAX_PENDING_OUTGOING_SELECTION_REPLIES
    );
    assert_eq!(
        super::super::super::selection_proxy::request_count_for_test(&xwm),
        super::super::super::selection_proxy::MAX_PENDING_PROXY_SELECTION_REQUESTS
    );
    let _ = fixture_requests(&mut peer);
}
