//! RED/GREEN regressions for the dormant reverse selection service.

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
    let prepared = super::super::super::selection_wire::prepared_proxy_selection_for_test(
        &xwm,
        selection_kind,
    )
    .expect("prepared catalog");
    let target = prepared.data_targets[0].target;
    assert!(
        super::super::super::selection_proxy::install_test_authority(
            &mut xwm,
            selection_kind,
            id,
            12_345,
        )
    );
    (xwm, peer, id, target)
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
