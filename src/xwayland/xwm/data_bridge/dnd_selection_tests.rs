use std::{
    io::{Read, Write},
    num::NonZeroU64,
    os::{fd::AsRawFd, unix::net::UnixStream},
};

use super::super::dnd::DndStatusResult;
use super::*;
use crate::xwayland::{
    CanonicalDndSessionId, XwaylandDndAction, XwaylandDndMimeCatalog, XwaylandDndSourceProxyId,
    XwaylandDndVersion,
};

fn read_requests(peer: &mut UnixStream) -> Vec<u8> {
    peer.set_nonblocking(true).unwrap();
    let mut requests = Vec::new();
    let mut buffer = [0u8; 1024];
    loop {
        match peer.read(&mut buffer) {
            Ok(0) => break,
            Ok(bytes_read) => requests.extend_from_slice(&buffer[..bytes_read]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) => panic!("read X11 requests: {error}"),
        }
    }
    requests
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

#[derive(Debug, PartialEq, Eq)]
struct WirePropertyChange {
    window: u32,
    property: u32,
    property_type: u32,
    format: u8,
    value: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
struct WireSelectionNotify {
    time: u32,
    requestor: u32,
    selection: u32,
    target: u32,
    property: u32,
}

#[derive(Debug, Default)]
struct WireRequests {
    opcodes: Vec<u8>,
    property_changes: Vec<WirePropertyChange>,
    selection_notifies: Vec<WireSelectionNotify>,
}

fn read_wire_requests(peer: &mut UnixStream) -> WireRequests {
    let bytes = read_requests(peer);
    let mut requests = WireRequests::default();
    let mut offset = 0;
    while offset < bytes.len() {
        let words = u16::from_ne_bytes([bytes[offset + 2], bytes[offset + 3]]) as usize;
        assert!(words > 0, "unexpected BigRequests encoding");
        let request_len = words * 4;
        assert!(offset + request_len <= bytes.len(), "truncated X11 request");
        let request = &bytes[offset..offset + request_len];
        requests.opcodes.push(request[0]);
        if request[0] == xproto::CHANGE_PROPERTY_REQUEST {
            let format = request[16];
            let units = u32_at(request, 20) as usize;
            let value_len = units * usize::from(format / 8);
            requests.property_changes.push(WirePropertyChange {
                window: u32_at(request, 4),
                property: u32_at(request, 8),
                property_type: u32_at(request, 12),
                format,
                value: request[24..24 + value_len].to_vec(),
            });
        } else if request[0] == xproto::SEND_EVENT_REQUEST
            && request[12] & 0x7f == xproto::SELECTION_NOTIFY_EVENT
        {
            let event = &request[12..44];
            requests.selection_notifies.push(WireSelectionNotify {
                time: u32_at(event, 4),
                requestor: u32_at(event, 8),
                selection: u32_at(event, 12),
                target: u32_at(event, 16),
                property: u32_at(event, 20),
            });
        }
        offset += request_len;
    }
    requests
}

fn values32(bytes: &[u8]) -> Vec<u32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| u32_at(chunk, 0))
        .collect()
}

fn active_fixture(
    mime_types: &[&str],
) -> (
    super::super::super::Xwm,
    UnixStream,
    crate::xwayland::XwaylandDndAdapterId,
    u32,
    X11WindowHandle,
) {
    let generation = super::super::super::XwaylandGeneration::new(NonZeroU64::new(83).unwrap());
    let (mut xwm, peer) = super::super::super::test_fixture_for_tests(generation);
    let id = crate::xwayland::XwaylandDndAdapterId::new(
        CanonicalDndSessionId::Wayland(NonZeroU64::new(9103).unwrap()),
        generation,
    )
    .unwrap();
    let target = X11WindowHandle::new(generation, 0x773);
    let source_proxy = 0x883;
    let catalog =
        XwaylandDndMimeCatalog::try_new(mime_types.iter().map(|mime| (*mime).to_owned()).collect())
            .unwrap();
    assert!(xwm.data_bridge.dnd.install_wayland_session(
        id,
        catalog,
        vec![XwaylandDndAction::Copy],
    ));
    assert!(xwm.data_bridge.dnd.bind_source_proxy(id, source_proxy));
    assert!(
        xwm.data_bridge
            .dnd
            .confirm_source_ownership(id, source_proxy, 1234)
    );
    let mime_atoms = (0..mime_types.len())
        .map(|index| Some(99 + index as u32))
        .collect();
    xwm.data_bridge
        .dnd
        .outgoing_session_mut()
        .unwrap()
        .mime_atoms = mime_atoms;
    assert!(xwm.data_bridge.dnd.set_discovered_target(
        id,
        target,
        target.xid(),
        XwaylandDndVersion::new(5).unwrap(),
    ));
    assert!(xwm.data_bridge.dnd.mark_entered(id));
    assert!(
        xwm.data_bridge
            .dnd
            .position(id, target, 10, 20, Some(XwaylandDndAction::Copy))
    );
    xwm.data_bridge
        .dnd_outgoing
        .initialize_generation(generation);
    (xwm, peer, id, source_proxy, target)
}

fn mark_drop_awaiting_finished(
    xwm: &mut super::super::super::Xwm,
    id: crate::xwayland::XwaylandDndAdapterId,
    target: X11WindowHandle,
) {
    let session = xwm
        .data_bridge
        .dnd
        .outgoing_session_mut()
        .filter(|session| session.id == id)
        .expect("exact active XDND session");
    session.last_status = Some(DndStatusResult {
        accepted: true,
        action: Some(XwaylandDndAction::Copy),
        requested_action: Some(XwaylandDndAction::Copy),
    });
    assert!(
        xwm.data_bridge
            .dnd
            .request_drop(id, target, XwaylandDndAction::Copy)
    );
    assert!(xwm.data_bridge.dnd.mark_awaiting_finished(
        id,
        XwaylandDndAction::Copy,
        test_now_ns().saturating_add(super::super::dnd::TARGET_FINISHED_TIMEOUT_NS),
    ));
}

fn selection_request_event(
    xwm: &super::super::super::Xwm,
    source_proxy: u32,
    target_window: X11WindowHandle,
    target: u32,
    property: u32,
    time: u32,
) -> xproto::SelectionRequestEvent {
    xproto::SelectionRequestEvent {
        response_type: xproto::SELECTION_REQUEST_EVENT,
        sequence: 0,
        time,
        owner: source_proxy,
        requestor: target_window.xid(),
        selection: xwm.atoms.get(XwmAtomName::XdndSelection),
        target,
        property,
    }
}

fn pending_multiple_sequence(xwm: &super::super::super::Xwm) -> u16 {
    let Some(sequence) = xwm.data_bridge.dnd.pending_replies.keys().next() else {
        panic!("MULTIPLE property GetProperty reply is pending");
    };
    *sequence as u16
}

fn test_now_ns() -> u64 {
    crate::native::event_loop::monotonic_now_ns().unwrap_or_default()
}

fn multiple_property_reply(sequence: u16, atom_pair: u32, values: &[u32]) -> Vec<u8> {
    let mut reply = vec![0; 32 + values.len() * 4];
    reply[0] = 1;
    reply[1] = 32;
    reply[2..4].copy_from_slice(&sequence.to_ne_bytes());
    reply[4..8].copy_from_slice(&(values.len() as u32).to_ne_bytes());
    reply[8..12].copy_from_slice(&atom_pair.to_ne_bytes());
    reply[16..20].copy_from_slice(&(values.len() as u32).to_ne_bytes());
    for (index, value) in values.iter().enumerate() {
        reply[32 + index * 4..36 + index * 4].copy_from_slice(&value.to_ne_bytes());
    }
    reply
}

fn complete_multiple_read(
    xwm: &mut super::super::super::Xwm,
    peer: &mut UnixStream,
    pairs: &[u32],
) {
    let sequence = pending_multiple_sequence(xwm);
    let atom_pair = xwm.atoms.get(XwmAtomName::AtomPair);
    peer.write_all(&multiple_property_reply(sequence, atom_pair, pairs))
        .expect("send MULTIPLE property reply");
    xwm.drain_events(32)
        .expect("process MULTIPLE property reply");
    xwm.flush().expect("flush MULTIPLE completion");
}

#[test]
fn offered_mime_request_after_drop_queues_one_exact_source_read() {
    let generation = super::super::super::XwaylandGeneration::new(NonZeroU64::new(81).unwrap());
    let (mut xwm, _peer) = super::super::super::test_fixture_for_tests(generation);
    let id = crate::xwayland::XwaylandDndAdapterId::new(
        CanonicalDndSessionId::Wayland(NonZeroU64::new(9101).unwrap()),
        generation,
    )
    .unwrap();
    let target = X11WindowHandle::new(generation, 0x770);
    let source_proxy = 0x880;
    let catalog = XwaylandDndMimeCatalog::try_new(vec!["text/plain".to_owned()]).unwrap();
    assert!(xwm.data_bridge.dnd.install_wayland_session(
        id,
        catalog,
        vec![XwaylandDndAction::Copy],
    ));
    assert!(xwm.data_bridge.dnd.bind_source_proxy(id, source_proxy));
    assert!(
        xwm.data_bridge
            .dnd
            .confirm_source_ownership(id, source_proxy, 1234)
    );
    xwm.data_bridge
        .dnd
        .outgoing_session_mut()
        .unwrap()
        .mime_atoms[0] = Some(99);
    assert!(xwm.data_bridge.dnd.set_discovered_target(
        id,
        target,
        target.xid(),
        XwaylandDndVersion::new(5).unwrap(),
    ));
    assert!(xwm.data_bridge.dnd.mark_entered(id));
    assert!(xwm.data_bridge.dnd.position(
        id,
        target,
        10,
        20,
        Some(crate::xwayland::XwaylandDndAction::Copy)
    ));
    xwm.data_bridge
        .dnd_outgoing
        .initialize_generation(generation);
    mark_drop_awaiting_finished(&mut xwm, id, target);

    let selection_atom = xwm.atoms.get(XwmAtomName::XdndSelection);
    let handled = selection_request(
        &mut xwm,
        xproto::SelectionRequestEvent {
            response_type: xproto::SELECTION_REQUEST_EVENT,
            sequence: 0,
            time: 1235,
            owner: source_proxy,
            requestor: target.xid(),
            selection: selection_atom,
            target: 99,
            property: 100,
        },
        10,
    )
    .unwrap();
    assert!(handled);
    let mut requests = super::super::super::dnd_outgoing::take_requests(&mut xwm);
    assert_eq!(requests.len(), 1);
    let request = requests.pop().unwrap();
    assert_eq!(request.transfer_id.source.adapter_id, id);
    assert_eq!(request.transfer_id.source.xid, source_proxy);
    assert_eq!(request.target, target);
    assert_eq!(request.mime_type, "text/plain");
    assert!(super::super::super::dnd_outgoing::take_requests(&mut xwm).is_empty());
    drop(request);
    super::super::super::dnd_outgoing::cancel_source(
        &mut xwm,
        XwaylandDndSourceProxyId {
            adapter_id: id,
            xid: source_proxy,
        },
        11,
    )
    .unwrap();
}

#[test]
fn post_drop_mime_read_reaches_wayland_source_once_and_returns_direct_payload() {
    let (mut xwm, mut peer, id, source_proxy, target) = active_fixture(&["text/plain"]);
    let mime_atom = 99;
    let property = 0xa11;
    mark_drop_awaiting_finished(&mut xwm, id, target);
    let event = selection_request_event(&xwm, source_proxy, target, mime_atom, property, 1235);
    assert!(selection_request(&mut xwm, event, test_now_ns()).unwrap());
    let mut source_requests = super::super::super::dnd_outgoing::take_requests(&mut xwm);
    assert_eq!(source_requests.len(), 1);
    let source_request = source_requests.pop().unwrap();
    assert_eq!(source_request.mime_type, "text/plain");
    assert_eq!(source_request.target, target);
    assert_eq!(source_request.requestor, target.xid());
    let transfer_id = source_request.transfer_id;
    let data = b"post-drop payload";
    assert_eq!(
        unsafe {
            libc::write(
                source_request.sink.as_raw_fd(),
                data.as_ptr().cast(),
                data.len(),
            )
        },
        data.len() as isize
    );
    drop(source_request);
    assert!(
        super::super::super::dnd_outgoing::resolve_requests(
            &mut xwm,
            [(transfer_id, true)],
            test_now_ns(),
        )
        .unwrap()
    );

    xwm.flush().expect("flush direct MIME completion");
    let wire = read_wire_requests(&mut peer);
    let payload = wire
        .property_changes
        .iter()
        .find(|change| change.property == property)
        .expect("direct MIME property after Drop");
    assert_eq!(payload.property_type, mime_atom);
    assert_eq!(payload.format, 8);
    assert_eq!(payload.value, data);
    assert_eq!(wire.selection_notifies.len(), 1);
    assert_eq!(
        wire.selection_notifies[0].selection,
        xwm.atoms.get(XwmAtomName::XdndSelection)
    );
    assert_eq!(wire.selection_notifies[0].target, mime_atom);
    assert_eq!(wire.selection_notifies[0].property, property);
    assert_eq!(wire.selection_notifies[0].requestor, target.xid());
    assert!(super::super::super::dnd_outgoing::take_requests(&mut xwm).is_empty());
}

#[test]
fn pending_source_read_started_before_drop_completes_after_drop() {
    let (mut xwm, mut peer, id, source_proxy, target) = active_fixture(&["text/plain"]);
    let mime_atom = 99;
    let property = 0xa12;
    let event = selection_request_event(&xwm, source_proxy, target, mime_atom, property, 1235);
    assert!(selection_request(&mut xwm, event, test_now_ns()).unwrap());
    assert_eq!(
        super::super::super::dnd_outgoing::transfer_count_for_test(&xwm),
        1,
        "the asynchronous source read owns exact transfer state before Drop"
    );

    xwm.data_bridge
        .dnd
        .outgoing_session_mut()
        .unwrap()
        .last_status = Some(DndStatusResult {
        accepted: true,
        action: Some(XwaylandDndAction::Copy),
        requested_action: Some(XwaylandDndAction::Copy),
    });
    super::super::dnd_adapter::apply_transitions(
        &mut xwm,
        [crate::xwayland::XwaylandDndTransition::DropRequested {
            session_id: id.session_id(),
            target,
            action: XwaylandDndAction::Copy,
            mime_types: XwaylandDndMimeCatalog::try_new(vec!["text/plain".to_owned()]).unwrap(),
            source_actions: vec![XwaylandDndAction::Copy],
        }],
        test_now_ns(),
    )
    .unwrap();
    assert_eq!(
        xwm.data_bridge.dnd.active_session().unwrap().progress,
        DndWireProgress::AwaitingFinished
    );
    assert_eq!(
        super::super::super::dnd_outgoing::transfer_count_for_test(&xwm),
        1,
        "physical Drop preserves the transfer that already owns the source read"
    );

    let mut requests = super::super::super::dnd_outgoing::take_requests(&mut xwm);
    assert_eq!(requests.len(), 1);
    let request = requests.pop().unwrap();
    assert_eq!(request.transfer_id.source.adapter_id, id);
    assert_eq!(request.transfer_id.source.xid, source_proxy);
    assert_eq!(request.target, target);
    let transfer_id = request.transfer_id;
    let data = b"source read crossed physical Drop";
    assert_eq!(
        unsafe { libc::write(request.sink.as_raw_fd(), data.as_ptr().cast(), data.len(),) },
        data.len() as isize
    );
    drop(request);
    assert!(
        super::super::super::dnd_outgoing::resolve_requests(
            &mut xwm,
            [(transfer_id, true)],
            test_now_ns(),
        )
        .unwrap()
    );
    xwm.flush().unwrap();

    let wire = read_wire_requests(&mut peer);
    let payload = wire
        .property_changes
        .iter()
        .find(|change| change.property == property)
        .expect("pre-Drop source read completes as a direct payload");
    assert_eq!(payload.window, target.xid());
    assert_eq!(payload.property_type, mime_atom);
    assert_eq!(payload.format, 8);
    assert_eq!(payload.value, data);
    assert_eq!(wire.selection_notifies.len(), 1);
    assert_eq!(wire.selection_notifies[0].requestor, target.xid());
    assert_eq!(
        wire.selection_notifies[0].selection,
        xwm.atoms.get(XwmAtomName::XdndSelection)
    );
    assert_eq!(wire.selection_notifies[0].target, mime_atom);
    assert_eq!(wire.selection_notifies[0].property, property);
}

#[test]
fn post_drop_mime_request_from_random_requestor_is_rejected() {
    let (mut xwm, mut peer, id, source_proxy, target) = active_fixture(&["text/plain"]);
    mark_drop_awaiting_finished(&mut xwm, id, target);
    let mut event = selection_request_event(&xwm, source_proxy, target, 99, 0xa12, 1235);
    event.requestor = 0x999;
    assert!(selection_request(&mut xwm, event, test_now_ns()).unwrap());
    assert!(super::super::super::dnd_outgoing::take_requests(&mut xwm).is_empty());
    xwm.flush().expect("flush rejected request notification");
    let wire = read_wire_requests(&mut peer);
    assert!(wire.property_changes.is_empty());
    assert_eq!(wire.selection_notifies.len(), 1);
    assert_eq!(wire.selection_notifies[0].property, x11rb::NONE);
    assert_eq!(wire.selection_notifies[0].requestor, 0x999);
}

#[test]
fn targets_and_timestamp_requests_return_the_exact_current_catalog_and_owner_time() {
    let generation = super::super::super::XwaylandGeneration::new(NonZeroU64::new(82).unwrap());
    let (mut xwm, mut peer) = super::super::super::test_fixture_for_tests(generation);
    let id = crate::xwayland::XwaylandDndAdapterId::new(
        CanonicalDndSessionId::Wayland(NonZeroU64::new(9102).unwrap()),
        generation,
    )
    .unwrap();
    let target = X11WindowHandle::new(generation, 0x771);
    let source_proxy = 0x881;
    let catalog =
        XwaylandDndMimeCatalog::try_new(vec!["text/plain".to_owned(), "image/png".to_owned()])
            .unwrap();
    assert!(xwm.data_bridge.dnd.install_wayland_session(
        id,
        catalog,
        vec![XwaylandDndAction::Copy],
    ));
    assert!(xwm.data_bridge.dnd.bind_source_proxy(id, source_proxy));
    assert!(
        xwm.data_bridge
            .dnd
            .confirm_source_ownership(id, source_proxy, 1234)
    );
    xwm.data_bridge
        .dnd
        .outgoing_session_mut()
        .unwrap()
        .mime_atoms = vec![Some(99), Some(100)];
    assert!(xwm.data_bridge.dnd.set_discovered_target(
        id,
        target,
        target.xid(),
        XwaylandDndVersion::new(5).unwrap(),
    ));
    assert!(xwm.data_bridge.dnd.mark_entered(id));
    assert!(xwm.data_bridge.dnd.position(
        id,
        target,
        10,
        20,
        Some(crate::xwayland::XwaylandDndAction::Copy)
    ));
    mark_drop_awaiting_finished(&mut xwm, id, target);

    let targets_atom = xwm.atoms.get(XwmAtomName::Targets);
    let timestamp_atom = xwm.atoms.get(XwmAtomName::Timestamp);
    let selection_atom = xwm.atoms.get(XwmAtomName::XdndSelection);
    for (target_atom, property) in [(targets_atom, 200), (timestamp_atom, 201)] {
        assert!(
            selection_request(
                &mut xwm,
                xproto::SelectionRequestEvent {
                    response_type: xproto::SELECTION_REQUEST_EVENT,
                    sequence: 0,
                    time: 1235,
                    owner: source_proxy,
                    requestor: target.xid(),
                    selection: selection_atom,
                    target: target_atom,
                    property,
                },
                10,
            )
            .unwrap()
        );
    }
    xwm.flush().unwrap();
    let requests = read_requests(&mut peer);
    assert_eq!(requests[0], 18);
    let targets_property = xwm.atoms.get(XwmAtomName::Targets);
    assert_eq!(u32_at(&requests, 8), 200);
    assert_eq!(u32_at(&requests, 12), u32::from(AtomEnum::ATOM));
    assert_eq!(requests[16], 32);
    assert_eq!(u32_at(&requests, 20), 5);
    let returned_targets = (0..5)
        .map(|index| u32_at(&requests, 24 + index * 4))
        .collect::<Vec<_>>();
    assert_eq!(
        returned_targets,
        [
            targets_property,
            timestamp_atom,
            xwm.atoms.get(XwmAtomName::Multiple),
            99,
            100
        ]
    );

    let targets_request_len = u16::from_ne_bytes([requests[2], requests[3]]) as usize * 4;
    let timestamp_request = targets_request_len + 44;
    assert_eq!(requests[timestamp_request], 18);
    assert_eq!(u32_at(&requests, timestamp_request + 8), 201);
    assert_eq!(
        u32_at(&requests, timestamp_request + 12),
        u32::from(AtomEnum::INTEGER)
    );
    assert_eq!(requests[timestamp_request + 16], 32);
    assert_eq!(u32_at(&requests, timestamp_request + 20), 1);
    assert_eq!(u32_at(&requests, timestamp_request + 24), 1234);
}

#[test]
fn multiple_without_property_is_refused_before_get_property_or_conversion() {
    let (mut xwm, mut peer, _, source_proxy, target_window) = active_fixture(&[]);
    let multiple = xwm.atoms.get(XwmAtomName::Multiple);
    let selection = xwm.atoms.get(XwmAtomName::XdndSelection);
    let event = selection_request_event(
        &xwm,
        source_proxy,
        target_window,
        multiple,
        x11rb::NONE,
        1235,
    );
    assert!(selection_request(&mut xwm, event, 10).unwrap());
    xwm.flush().expect("flush failed MULTIPLE notification");

    let wire = read_wire_requests(&mut peer);
    assert_eq!(wire.selection_notifies.len(), 1);
    assert_eq!(
        wire.selection_notifies[0],
        WireSelectionNotify {
            time: 1235,
            requestor: target_window.xid(),
            selection,
            target: multiple,
            property: x11rb::NONE,
        }
    );
    assert!(!wire.opcodes.contains(&xproto::GET_PROPERTY_REQUEST));
    assert!(wire.property_changes.is_empty());
    assert!(xwm.data_bridge.dnd.pending_replies.is_empty());
    assert!(super::super::super::dnd_outgoing::take_requests(&mut xwm).is_empty());
    assert_eq!(
        super::super::super::dnd_outgoing::multiple_group_count_for_test(&xwm),
        0
    );
    assert_eq!(
        super::super::super::dnd_outgoing::transfer_count_for_test(&xwm),
        0
    );
}

#[test]
fn multiple_success_notifies_with_original_target_and_parent_property() {
    let (mut xwm, mut peer, _, source_proxy, target_window) = active_fixture(&[]);
    let multiple = xwm.atoms.get(XwmAtomName::Multiple);
    let selection = xwm.atoms.get(XwmAtomName::XdndSelection);
    let targets = xwm.atoms.get(XwmAtomName::Targets);
    let timestamp = xwm.atoms.get(XwmAtomName::Timestamp);
    let parent_property = 0xa00;
    let property_a = 0xa01;
    let property_b = 0xa02;
    let event = selection_request_event(
        &xwm,
        source_proxy,
        target_window,
        multiple,
        parent_property,
        1235,
    );
    selection_request(&mut xwm, event, test_now_ns()).unwrap();
    xwm.flush().expect("flush MULTIPLE property read");
    let initial = read_wire_requests(&mut peer);
    assert!(initial.opcodes.contains(&xproto::GET_PROPERTY_REQUEST));
    assert!(initial.selection_notifies.is_empty());

    complete_multiple_read(
        &mut xwm,
        &mut peer,
        &[targets, property_a, timestamp, property_b],
    );
    let wire = read_wire_requests(&mut peer);
    assert_eq!(wire.property_changes.len(), 3);
    assert_eq!(wire.property_changes[0].property, property_a);
    assert_eq!(wire.property_changes[1].property, property_b);
    assert_eq!(
        wire.property_changes[0].property_type,
        u32::from(AtomEnum::ATOM)
    );
    assert_eq!(
        values32(&wire.property_changes[0].value),
        [targets, timestamp, multiple]
    );
    assert_eq!(
        wire.property_changes[1].property_type,
        u32::from(AtomEnum::INTEGER)
    );
    assert_eq!(values32(&wire.property_changes[1].value), [1234]);
    let parent_pairs = wire
        .property_changes
        .iter()
        .find(|change| change.property == parent_property)
        .expect("rewritten parent atom-pair property");
    assert_eq!(
        parent_pairs.property_type,
        xwm.atoms.get(XwmAtomName::AtomPair)
    );
    assert_eq!(parent_pairs.format, 32);
    assert_eq!(
        values32(&parent_pairs.value),
        [targets, property_a, timestamp, property_b]
    );
    assert_eq!(wire.selection_notifies.len(), 1);
    assert_eq!(
        wire.selection_notifies[0],
        WireSelectionNotify {
            time: 1235,
            requestor: target_window.xid(),
            selection,
            target: multiple,
            property: parent_property,
        }
    );
}

#[test]
fn multiple_parent_property_write_failure_notifies_with_property_none() {
    let (mut xwm, mut peer, _, source_proxy, target_window) = active_fixture(&[]);
    let multiple = xwm.atoms.get(XwmAtomName::Multiple);
    let selection = xwm.atoms.get(XwmAtomName::XdndSelection);
    let targets = xwm.atoms.get(XwmAtomName::Targets);
    let timestamp = xwm.atoms.get(XwmAtomName::Timestamp);
    let parent_property = 0xa08;
    let property_a = 0xa09;
    let property_b = 0xa0a;
    let event = selection_request_event(
        &xwm,
        source_proxy,
        target_window,
        multiple,
        parent_property,
        1235,
    );
    selection_request(&mut xwm, event, test_now_ns()).unwrap();
    xwm.flush().expect("flush MULTIPLE property read");
    let _ = read_wire_requests(&mut peer);
    super::super::super::dnd_outgoing::fail_next_multiple_parent_property_write_for_test(&mut xwm);

    complete_multiple_read(
        &mut xwm,
        &mut peer,
        &[targets, property_a, timestamp, property_b],
    );
    let wire = read_wire_requests(&mut peer);
    assert_eq!(wire.property_changes.len(), 2);
    assert_eq!(wire.property_changes[0].property, property_a);
    assert_eq!(wire.property_changes[1].property, property_b);
    assert_eq!(wire.selection_notifies.len(), 1);
    assert_eq!(
        wire.selection_notifies[0],
        WireSelectionNotify {
            time: 1235,
            requestor: target_window.xid(),
            selection,
            target: multiple,
            property: x11rb::NONE,
        }
    );
}

#[test]
fn multiple_child_failure_only_clears_the_failed_pair_property() {
    let (mut xwm, mut peer, _, source_proxy, target_window) = active_fixture(&[]);
    let multiple = xwm.atoms.get(XwmAtomName::Multiple);
    let selection = xwm.atoms.get(XwmAtomName::XdndSelection);
    let targets = xwm.atoms.get(XwmAtomName::Targets);
    let unsupported = 0xdead;
    let timestamp = xwm.atoms.get(XwmAtomName::Timestamp);
    let parent_property = 0xa10;
    let property_a = 0xa11;
    let failed_property = 0xa12;
    let property_b = 0xa13;
    let event = selection_request_event(
        &xwm,
        source_proxy,
        target_window,
        multiple,
        parent_property,
        1235,
    );
    selection_request(&mut xwm, event, test_now_ns()).unwrap();
    xwm.flush().expect("flush MULTIPLE property read");
    let _ = read_wire_requests(&mut peer);

    complete_multiple_read(
        &mut xwm,
        &mut peer,
        &[
            targets,
            property_a,
            unsupported,
            failed_property,
            timestamp,
            property_b,
        ],
    );
    let wire = read_wire_requests(&mut peer);
    let parent_pairs = wire
        .property_changes
        .iter()
        .find(|change| change.property == parent_property)
        .expect("rewritten parent atom-pair property");
    assert_eq!(
        values32(&parent_pairs.value),
        [
            targets,
            property_a,
            unsupported,
            x11rb::NONE,
            timestamp,
            property_b,
        ]
    );
    assert_eq!(wire.selection_notifies.len(), 1);
    assert_eq!(wire.selection_notifies[0].selection, selection);
    assert_eq!(wire.selection_notifies[0].target, multiple);
    assert_eq!(wire.selection_notifies[0].property, parent_property);
    assert_eq!(wire.selection_notifies[0].time, 1235);
    assert_eq!(wire.selection_notifies[0].requestor, target_window.xid());
}

#[test]
fn multiple_mime_parent_notification_waits_for_child_conversion() {
    let (mut xwm, mut peer, id, source_proxy, target_window) = active_fixture(&["text/plain"]);
    let multiple = xwm.atoms.get(XwmAtomName::Multiple);
    let selection = xwm.atoms.get(XwmAtomName::XdndSelection);
    let mime_atom = 99;
    let parent_property = 0xa20;
    let child_property = 0xa21;
    let event = selection_request_event(
        &xwm,
        source_proxy,
        target_window,
        multiple,
        parent_property,
        1235,
    );
    selection_request(&mut xwm, event, test_now_ns()).unwrap();
    xwm.flush().expect("flush MULTIPLE property read");
    let _ = read_wire_requests(&mut peer);
    complete_multiple_read(&mut xwm, &mut peer, &[mime_atom, child_property]);
    mark_drop_awaiting_finished(&mut xwm, id, target_window);
    let waiting = read_wire_requests(&mut peer);
    assert!(waiting.selection_notifies.is_empty());
    assert!(
        !waiting
            .property_changes
            .iter()
            .any(|change| change.property == parent_property)
    );
    let mut source_requests = super::super::super::dnd_outgoing::take_requests(&mut xwm);
    assert_eq!(source_requests.len(), 1);
    let source_request = source_requests.pop().unwrap();
    let transfer_id = source_request.transfer_id;
    let data = b"asynchronous MIME data";
    assert_eq!(
        unsafe {
            libc::write(
                source_request.sink.as_raw_fd(),
                data.as_ptr().cast(),
                data.len(),
            )
        },
        data.len() as isize
    );
    drop(source_request);
    assert!(
        super::super::super::dnd_outgoing::resolve_requests(&mut xwm, [(transfer_id, true)], 32,)
            .unwrap()
    );
    xwm.flush().expect("flush asynchronous MULTIPLE completion");
    let completed = read_wire_requests(&mut peer);
    let child = completed
        .property_changes
        .iter()
        .find(|change| change.property == child_property)
        .expect("MIME child property");
    assert_eq!(child.property_type, mime_atom);
    assert_eq!(child.format, 8);
    assert_eq!(child.value, data);
    let parent = completed
        .property_changes
        .iter()
        .find(|change| change.property == parent_property)
        .expect("MULTIPLE parent property");
    assert_eq!(values32(&parent.value), [mime_atom, child_property]);
    assert_eq!(completed.selection_notifies.len(), 1);
    assert_eq!(completed.selection_notifies[0].selection, selection);
    assert_eq!(completed.selection_notifies[0].target, multiple);
    assert_eq!(completed.selection_notifies[0].property, parent_property);
    assert_eq!(completed.selection_notifies[0].time, 1235);
    assert_eq!(
        completed.selection_notifies[0].requestor,
        target_window.xid()
    );
}

#[test]
fn multiple_incr_parent_completion_is_independent_of_later_chunks() {
    let (mut xwm, mut peer, _, source_proxy, target_window) = active_fixture(&["text/plain"]);
    let id = xwm.data_bridge.dnd.active_id().unwrap();
    mark_drop_awaiting_finished(&mut xwm, id, target_window);
    let multiple = xwm.atoms.get(XwmAtomName::Multiple);
    let selection = xwm.atoms.get(XwmAtomName::XdndSelection);
    let incr_atom = xwm.atoms.get(XwmAtomName::Incr);
    let mime_atom = 99;
    let parent_property = 0xa30;
    let child_property = 0xa31;
    let event = selection_request_event(
        &xwm,
        source_proxy,
        target_window,
        multiple,
        parent_property,
        1235,
    );
    selection_request(&mut xwm, event, test_now_ns()).unwrap();
    xwm.flush().expect("flush MULTIPLE property read");
    let _ = read_wire_requests(&mut peer);
    complete_multiple_read(&mut xwm, &mut peer, &[mime_atom, child_property]);
    let _ = read_wire_requests(&mut peer);
    let mut source_requests = super::super::super::dnd_outgoing::take_requests(&mut xwm);
    let source_request = source_requests.pop().expect("MIME source request");
    assert!(source_requests.is_empty());
    let transfer_id = source_request.transfer_id;
    assert!(
        super::super::super::dnd_outgoing::resolve_requests(&mut xwm, [(transfer_id, true)], 42,)
            .unwrap()
    );

    xwm.flush().expect("flush waiting INCR source");
    let waiting = read_wire_requests(&mut peer);
    assert!(waiting.selection_notifies.is_empty());
    assert!(
        !waiting
            .property_changes
            .iter()
            .any(|change| change.property == parent_property)
    );

    let source_requests = super::super::super::dnd_outgoing::take_requests(&mut xwm);
    assert!(source_requests.is_empty());
    // A single blocking pipe write larger than its capacity can block; fill
    // the source pipe from a writer thread while the transfer drains it.
    let data = vec![0x5a; super::super::super::dnd_outgoing::MAX_DND_CHUNK_BYTES + 1];
    let sink = source_request.sink;
    let writer = std::thread::spawn(move || {
        let fd = sink.as_raw_fd();
        let mut written = 0;
        while written < data.len() {
            let result =
                unsafe { libc::write(fd, data[written..].as_ptr().cast(), data.len() - written) };
            assert!(result > 0, "write INCR source payload");
            written += result as usize;
        }
        drop(sink);
    });
    let interests = super::super::super::dnd_outgoing::source_interests(&xwm);
    let [(interest_id, read_fd)] = interests.as_slice() else {
        panic!("one direct source transfer must await readiness");
    };
    assert_eq!(*interest_id, transfer_id);
    let mut available: libc::c_int = 0;
    let source_ready_deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let result = unsafe { libc::ioctl(*read_fd, libc::FIONREAD, &mut available) };
        assert_eq!(result, 0, "inspect INCR test pipe readiness");
        if available as usize >= super::super::super::dnd_outgoing::MAX_DND_CHUNK_BYTES {
            break;
        }
        assert!(
            std::time::Instant::now() < source_ready_deadline,
            "INCR source pipe did not fill to one chunk"
        );
        std::thread::yield_now();
    }
    super::super::super::dnd_outgoing::bind_reactor_token(&mut xwm, transfer_id, *read_fd, Some(1));
    assert!(
        super::super::super::dnd_outgoing::handle_source_ready(
            &mut xwm,
            transfer_id,
            transfer_id.source.adapter_id.generation(),
            1,
            43,
        )
        .unwrap()
    );
    writer.join().expect("finish writing source payload");
    xwm.flush().expect("flush INCR marker and parent notify");
    let announced = read_wire_requests(&mut peer);
    let incr = announced
        .property_changes
        .iter()
        .find(|change| change.property == child_property)
        .expect("INCR announcement property");
    assert_eq!(incr.property_type, incr_atom);
    assert_eq!(incr.format, 32);
    assert_eq!(
        values32(&incr.value),
        [super::super::super::dnd_outgoing::MAX_DND_CHUNK_BYTES as u32]
    );
    let parent = announced
        .property_changes
        .iter()
        .find(|change| change.property == parent_property)
        .expect("MULTIPLE parent property");
    assert_eq!(values32(&parent.value), [mime_atom, child_property]);
    assert_eq!(announced.selection_notifies.len(), 1);
    assert_eq!(announced.selection_notifies[0].selection, selection);
    assert_eq!(announced.selection_notifies[0].target, multiple);
    assert_eq!(announced.selection_notifies[0].property, parent_property);

    assert!(
        super::super::super::dnd_outgoing::property_deleted(
            &mut xwm,
            target_window.xid(),
            child_property,
            44,
        )
        .unwrap()
    );
    assert!(
        super::super::super::dnd_outgoing::property_deleted(
            &mut xwm,
            target_window.xid(),
            child_property,
            45,
        )
        .unwrap()
    );
    let interests = super::super::super::dnd_outgoing::source_interests(&xwm);
    let [(interest_id, read_fd)] = interests.as_slice() else {
        panic!("INCR source must await its final byte");
    };
    assert_eq!(*interest_id, transfer_id);
    super::super::super::dnd_outgoing::bind_reactor_token(&mut xwm, transfer_id, *read_fd, Some(2));
    assert!(
        super::super::super::dnd_outgoing::handle_source_ready(
            &mut xwm,
            transfer_id,
            transfer_id.source.adapter_id.generation(),
            2,
            46,
        )
        .unwrap()
    );
    assert!(
        super::super::super::dnd_outgoing::property_deleted(
            &mut xwm,
            target_window.xid(),
            child_property,
            47,
        )
        .unwrap()
    );
    xwm.flush().expect("flush later INCR chunks");
    let chunks = read_wire_requests(&mut peer);
    assert!(chunks.selection_notifies.is_empty());
    assert_eq!(
        chunks
            .property_changes
            .iter()
            .filter(|change| change.property == child_property)
            .count(),
        3
    );
}
