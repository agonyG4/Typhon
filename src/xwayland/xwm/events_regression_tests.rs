use std::{
    collections::HashMap,
    io::{self, Read, Write},
    num::NonZeroU64,
};

use crate::compositor::{DesktopWindowKind, WindowConstraints, WindowMetadata};
use crate::xwayland::XwaylandAssociationEvent;
use x11rb::{
    protocol::{Event, sync, xproto},
    x11_utils::ExtensionInformation,
};

use super::super::X11WindowSnapshot;
use super::super::{ResizeSyncState, X11WindowType, X11WindowTypes, XwmCommand, XwmEvent};
use super::tests::{
    complete_property_refresh, generation, map_event, prepare_managed_window, ready_events,
    ready_surface_id, test_fixture, unmap_event,
};
use super::{X11Geometry, X11WindowLifecycle, normalize};

fn read_fixture_requests(peer: &mut std::os::unix::net::UnixStream) -> Vec<u8> {
    peer.set_nonblocking(true)
        .expect("nonblocking fixture peer");
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        match peer.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => bytes.extend_from_slice(&buffer[..read]),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(error) => panic!("read fixture X11 requests: {error}"),
        }
    }
    bytes
}

fn request_opcodes(bytes: &[u8]) -> Vec<u8> {
    let mut opcodes = Vec::new();
    let mut offset: usize = 0;
    while offset.saturating_add(4) <= bytes.len() {
        let opcode = bytes[offset];
        let length = usize::from(u16::from_le_bytes([bytes[offset + 2], bytes[offset + 3]]));
        assert!(length > 0, "X11 request has zero length at byte {offset}");
        let request_bytes = length * 4;
        assert!(
            offset.saturating_add(request_bytes) <= bytes.len(),
            "truncated X11 request at byte {offset}"
        );
        opcodes.push(opcode);
        offset += request_bytes;
    }
    assert_eq!(offset, bytes.len(), "trailing bytes after X11 requests");
    opcodes
}

fn request_minor_opcodes(bytes: &[u8], major_opcode: u8) -> Vec<u8> {
    let mut minors = Vec::new();
    let mut offset: usize = 0;
    while offset.saturating_add(4) <= bytes.len() {
        let length = usize::from(u16::from_le_bytes([bytes[offset + 2], bytes[offset + 3]]));
        assert!(length > 0, "X11 request has zero length at byte {offset}");
        let request_bytes = length * 4;
        assert!(offset.saturating_add(request_bytes) <= bytes.len());
        if bytes[offset] == major_opcode {
            minors.push(bytes[offset + 1]);
        }
        offset += request_bytes;
    }
    assert_eq!(offset, bytes.len(), "trailing bytes after X11 requests");
    minors
}

fn sync_request_counter_values(bytes: &[u8]) -> Vec<u64> {
    let mut values = Vec::new();
    let mut offset: usize = 0;
    while offset.saturating_add(4) <= bytes.len() {
        let length = usize::from(u16::from_le_bytes([bytes[offset + 2], bytes[offset + 3]]));
        assert!(length > 0, "X11 request has zero length at byte {offset}");
        let request_bytes = length * 4;
        assert!(offset.saturating_add(request_bytes) <= bytes.len());
        if bytes[offset] == 25 {
            let low = u64::from(u32::from_le_bytes(
                bytes[offset + 32..offset + 36]
                    .try_into()
                    .expect("sync low value"),
            ));
            let high = i64::from(i32::from_le_bytes(
                bytes[offset + 36..offset + 40]
                    .try_into()
                    .expect("sync high value"),
            ));
            values.push(((high as i128) << 32 | i128::from(low)) as u64);
        }
        offset += request_bytes;
    }
    assert_eq!(offset, bytes.len(), "trailing bytes after X11 requests");
    values
}

fn sync_snapshot(handle: super::X11WindowHandle, counter: u64) -> X11WindowSnapshot {
    X11WindowSnapshot {
        handle,
        surface_id: 42,
        kind: DesktopWindowKind::Managed,
        window_types: Default::default(),
        override_redirect: false,
        geometry: X11Geometry {
            x: 100,
            y: 100,
            width: 800,
            height: 600,
        },
        metadata: WindowMetadata::default(),
        constraints: WindowConstraints::default(),
        state: Default::default(),
        transient_for: None,
        supports_delete: false,
        supports_take_focus: false,
        accepts_input: None,
        window_role: None,
        startup_id: None,
        user_time: None,
        urgency: false,
        supports_sync_request: true,
        sync_counter: Some(counter),
    }
}

fn prepare_mapped_override_redirect_window(
    xwm: &mut super::super::Xwm,
    xid: u32,
) -> super::X11WindowHandle {
    let handle = super::X11WindowHandle::new(xwm.generation, xid);
    assert!(xwm.windows.insert_observed_with_kind(
        handle,
        DesktopWindowKind::OverrideRedirect,
        X11Geometry {
            x: 10,
            y: 20,
            width: 100,
            height: 80,
        },
    ));
    xwm.windows
        .adopt_mapped(handle)
        .expect("map override-redirect window");
    handle
}

fn query_tree_reply(sequence: u16, root: u32, children: &[u32]) -> Vec<u8> {
    let length = u32::try_from(children.len()).expect("child count fits X11 reply");
    let mut reply = vec![0_u8; 32 + children.len() * 4];
    reply[0] = 1;
    reply[2..4].copy_from_slice(&sequence.to_ne_bytes());
    reply[4..8].copy_from_slice(&length.to_ne_bytes());
    reply[8..12].copy_from_slice(&root.to_ne_bytes());
    reply[12..16].copy_from_slice(&root.to_ne_bytes());
    reply[16..18].copy_from_slice(
        &u16::try_from(children.len())
            .expect("child count fits X11 reply")
            .to_ne_bytes(),
    );
    for (index, child) in children.iter().copied().enumerate() {
        let offset = 32 + index * 4;
        reply[offset..offset + 4].copy_from_slice(&child.to_ne_bytes());
    }
    reply
}

#[test]
fn override_redirect_query_tree_emits_current_bottom_to_top_snapshot() {
    let generation = generation(250);
    let (mut xwm, mut peer) = test_fixture(generation);
    let bottom = prepare_mapped_override_redirect_window(&mut xwm, 250);
    let top = prepare_mapped_override_redirect_window(&mut xwm, 251);

    xwm.mark_override_redirect_stack_dirty();
    xwm.drain_events(256).expect("issue root stack query");
    let (sequence, epoch) = xwm
        .override_redirect_stack_query_for_test()
        .expect("one root stack query");
    peer.write_all(&query_tree_reply(
        sequence as u16,
        xwm.root,
        &[bottom.xid(), top.xid(), 0xdead_beef],
    ))
    .expect("root stack reply");
    xwm.drain_events(256).expect("consume root stack reply");

    assert!(
        xwm.take_events().any(|event| matches!(
            event,
            XwmEvent::OverrideRedirectStackSnapshot {
                generation: event_generation,
                epoch: event_epoch,
                bottom_to_top,
            } if event_generation == generation
                && event_epoch == epoch
                && bottom_to_top == vec![bottom, top]
        )),
        "current root order should be emitted bottom-to-top"
    );
    let metrics = xwm.override_redirect_stack_metrics_for_test();
    assert_eq!(metrics.snapshots_emitted, 1);
    assert_eq!(metrics.replies_superseded, 0);
    assert_eq!(metrics.replies_incomplete, 0);
}

#[test]
fn override_redirect_query_tree_superseded_reply_is_not_emitted() {
    let generation = generation(251);
    let (mut xwm, mut peer) = test_fixture(generation);
    let bottom = prepare_mapped_override_redirect_window(&mut xwm, 252);
    let top = prepare_mapped_override_redirect_window(&mut xwm, 253);

    xwm.mark_override_redirect_stack_dirty();
    xwm.drain_events(256).expect("issue root stack query");
    let (first_sequence, first_epoch) = xwm
        .override_redirect_stack_query_for_test()
        .expect("first root stack query");
    xwm.mark_override_redirect_stack_dirty();
    xwm.drain_events(256)
        .expect("retain dirty root stack query");
    peer.write_all(&query_tree_reply(
        first_sequence as u16,
        xwm.root,
        &[bottom.xid(), top.xid()],
    ))
    .expect("superseded root stack reply");
    xwm.drain_events(256)
        .expect("consume superseded root stack reply");

    assert!(
        xwm.take_events()
            .all(|event| !matches!(event, XwmEvent::OverrideRedirectStackSnapshot { .. }))
    );
    assert_eq!(
        xwm.override_redirect_stack_metrics_for_test()
            .replies_superseded,
        1,
        "a reply for epoch {first_epoch} should be counted as superseded"
    );
    let metrics = xwm.override_redirect_stack_metrics_for_test();
    assert_eq!(metrics.snapshots_emitted, 0);
    assert_eq!(metrics.replies_incomplete, 0);
    assert_eq!(
        xwm.override_redirect_stack_metrics_for_test()
            .queries_issued,
        2,
        "a superseded reply should cause exactly one follow-up query"
    );
}

#[test]
fn incomplete_override_redirect_query_tree_reply_does_not_remove_live_window() {
    let generation = generation(252);
    let (mut xwm, mut peer) = test_fixture(generation);
    let present = prepare_mapped_override_redirect_window(&mut xwm, 254);
    let missing = prepare_mapped_override_redirect_window(&mut xwm, 255);

    xwm.mark_override_redirect_stack_dirty();
    xwm.drain_events(256).expect("issue root stack query");
    let (sequence, _) = xwm
        .override_redirect_stack_query_for_test()
        .expect("root stack query");
    peer.write_all(&query_tree_reply(
        sequence as u16,
        xwm.root,
        &[present.xid()],
    ))
    .expect("incomplete root stack reply");
    xwm.drain_events(256)
        .expect("consume incomplete root stack reply");

    assert!(
        xwm.take_events()
            .all(|event| !matches!(event, XwmEvent::OverrideRedirectStackSnapshot { .. }))
    );
    assert!(xwm.windows.contains(missing));
    assert_eq!(
        xwm.override_redirect_stack_metrics_for_test()
            .replies_incomplete,
        1
    );
    let metrics = xwm.override_redirect_stack_metrics_for_test();
    assert_eq!(metrics.snapshots_emitted, 0);
    assert_eq!(metrics.replies_superseded, 0);
    assert_eq!(
        xwm.override_redirect_stack_metrics_for_test()
            .queries_issued,
        2,
        "an incomplete reply should request one fresh reconciliation"
    );
}

#[test]
fn partial_xwm_drain_does_not_emit_root_snapshot_before_queued_lifecycle_events() {
    let generation = generation(253);
    let (mut xwm, mut peer) = test_fixture(generation);
    let bottom = prepare_mapped_override_redirect_window(&mut xwm, 256);
    let top = prepare_mapped_override_redirect_window(&mut xwm, 257);

    xwm.mark_override_redirect_stack_dirty();
    xwm.drain_events(256).expect("issue root stack query");
    let (sequence, _) = xwm
        .override_redirect_stack_query_for_test()
        .expect("one root stack query");
    peer.write_all(&query_tree_reply(
        sequence as u16,
        xwm.root,
        &[bottom.xid(), top.xid()],
    ))
    .expect("root stack reply");

    let mut unrelated_property = [0_u8; 32];
    unrelated_property[0] = 28;
    unrelated_property[4..8].copy_from_slice(&0xdead_beef_u32.to_ne_bytes());
    for _ in 0..256 {
        peer.write_all(&unrelated_property)
            .expect("unrelated property event");
    }
    let configure = xproto::ConfigureNotifyEvent {
        response_type: 22,
        sequence: 0,
        event: 1,
        window: top.xid(),
        above_sibling: bottom.xid(),
        x: 10,
        y: 20,
        width: 100,
        height: 80,
        border_width: 0,
        override_redirect: true,
    };
    peer.write_all(&<[u8; 32]>::from(configure))
        .expect("queued stacking event");

    let partial = xwm.drain_events(256).expect("partial XWM drain");
    assert!(partial.budget_exhausted);
    assert!(!partial.events_quiescent);
    assert!(partial.property_replies_quiescent);
    assert!(!partial.quiescent);
    assert!(
        xwm.take_events()
            .all(|event| !matches!(event, XwmEvent::OverrideRedirectStackSnapshot { .. }))
    );
    assert_eq!(
        xwm.override_redirect_stack_query_for_test()
            .map(|query| query.0),
        Some(sequence),
        "partial drain must not replace the pending root-stack query"
    );

    let complete = xwm.drain_events(256).expect("quiescent XWM drain");
    assert!(!complete.budget_exhausted);
    assert!(complete.quiescent);
    let (final_sequence, _) = xwm
        .override_redirect_stack_query_for_test()
        .expect("final root stack query after the lifecycle event");
    assert_ne!(final_sequence, sequence);
    assert!(
        xwm.take_events()
            .all(|event| !matches!(event, XwmEvent::OverrideRedirectStackSnapshot { .. }))
    );
}

#[test]
fn override_redirect_unmap_before_ready_emits_one_admission_cancellation() {
    let generation = generation(254);
    let (mut xwm, _peer) = test_fixture(generation);
    let handle = super::X11WindowHandle::new(generation, 258);
    assert!(xwm.windows.insert_observed_with_kind(
        handle,
        DesktopWindowKind::OverrideRedirect,
        X11Geometry::default()
    ));
    normalize(&mut xwm, map_event(handle.xid(), true)).expect("normalize popup MapNotify");
    let _ = xwm.take_events().collect::<Vec<_>>();

    normalize(&mut xwm, unmap_event(handle.xid())).expect("normalize popup UnmapNotify");
    assert_eq!(
        xwm.take_events().collect::<Vec<_>>(),
        vec![XwmEvent::WindowAdmissionCancelled {
            window: handle,
            reason: super::super::X11AdmissionCancellationReason::Unmap,
        }]
    );
    let record = xwm.windows.get(handle).expect("withdrawn popup record");
    assert_eq!(record.lifecycle, X11WindowLifecycle::Withdrawn);
    assert!(record.association.is_none());
    assert!(!record.buffer_ready);
}

#[test]
fn override_redirect_destroy_before_ready_emits_one_admission_cancellation() {
    let generation = generation(255);
    let (mut xwm, _peer) = test_fixture(generation);
    let handle = super::X11WindowHandle::new(generation, 259);
    assert!(xwm.windows.insert_observed_with_kind(
        handle,
        DesktopWindowKind::OverrideRedirect,
        X11Geometry::default()
    ));
    normalize(&mut xwm, map_event(handle.xid(), true)).expect("normalize popup MapNotify");
    let _ = xwm.take_events().collect::<Vec<_>>();

    normalize(
        &mut xwm,
        Event::DestroyNotify(xproto::DestroyNotifyEvent {
            response_type: 17,
            sequence: 0,
            event: 1,
            window: handle.xid(),
        }),
    )
    .expect("normalize popup DestroyNotify");
    assert_eq!(
        xwm.take_events().collect::<Vec<_>>(),
        vec![XwmEvent::WindowAdmissionCancelled {
            window: handle,
            reason: super::super::X11AdmissionCancellationReason::Destroy,
        }]
    );
    assert!(!xwm.windows.contains(handle));
}

#[test]
fn configure_request_and_destroy_notify_are_normalized_in_one_x11_drain() {
    let generation = generation(205);
    let (mut xwm, mut peer) = test_fixture(generation);
    let handle = prepare_managed_window(&mut xwm, 205, true, false, false);

    let configure = xproto::ConfigureRequestEvent {
        response_type: 23,
        sequence: 77,
        parent: 1,
        window: handle.xid(),
        sibling: 0,
        x: 10,
        y: 20,
        width: 640,
        height: 480,
        border_width: 0,
        value_mask: xproto::ConfigWindow::default(),
        stack_mode: xproto::StackMode::ABOVE,
    };
    let destroy = xproto::DestroyNotifyEvent {
        response_type: 17,
        sequence: 0,
        event: 1,
        window: handle.xid(),
    };
    peer.write_all(&<[u8; 32]>::from(configure))
        .expect("configure event");
    peer.write_all(&<[u8; 32]>::from(destroy))
        .expect("destroy event");

    let drain = xwm.drain_events(256).expect("drain fake X server events");
    assert_eq!(drain.processed, 2);
    let events = xwm.take_events().collect::<Vec<_>>();
    assert!(events.iter().any(|event| {
        matches!(
            event,
            XwmEvent::ConfigureRequested { window, request }
                if *window == handle && request.client_event_sequence == Some(77)
        )
    }));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, XwmEvent::WindowDestroyed(window) if *window == handle))
    );
    assert!(!xwm.windows.contains(handle));
}

#[test]
fn older_self_generated_configure_notify_is_not_forwarded_as_external_geometry() {
    let generation = generation(216);
    let (mut xwm, _peer) = test_fixture(generation);
    let handle = prepare_managed_window(&mut xwm, 216, true, false, false);
    let geometries = [
        X11Geometry {
            x: 110,
            y: 100,
            width: 630,
            height: 480,
        },
        X11Geometry {
            x: 120,
            y: 100,
            width: 620,
            height: 480,
        },
        X11Geometry {
            x: 130,
            y: 100,
            width: 610,
            height: 480,
        },
    ];
    for geometry in geometries {
        xwm.note_expected_configure_with_context(
            handle,
            geometry,
            super::super::X11ConfigureFlags::all(),
            super::super::ConfigureSource::ClientRequest,
            None,
        );
    }

    normalize(
        &mut xwm,
        Event::ConfigureNotify(xproto::ConfigureNotifyEvent {
            response_type: 22,
            sequence: 0,
            event: 1,
            window: handle.xid(),
            above_sibling: 0,
            x: geometries[0].x as i16,
            y: geometries[0].y as i16,
            width: geometries[0].width as u16,
            height: geometries[0].height as u16,
            border_width: 0,
            override_redirect: false,
        }),
    )
    .expect("normalize delayed ConfigureNotify");

    assert!(
        xwm.take_events().all(|event| !matches!(
            event,
            XwmEvent::ConfigureNotify { window, .. } if window == handle
        )),
        "a delayed self-generated notification must remain inside XWM history"
    );
}

#[test]
fn configure_sequence_diagnostics_count_conflicts_and_sequence_only_rejections() {
    let generation = generation(220);
    let (mut xwm, _peer) = test_fixture(generation);
    let handle = prepare_managed_window(&mut xwm, 220, true, false, false);
    let first = X11Geometry {
        x: 110,
        y: 100,
        width: 630,
        height: 480,
    };
    let second = X11Geometry {
        x: 120,
        y: 100,
        width: 620,
        height: 480,
    };
    xwm.note_expected_configure_with_context(
        handle,
        first,
        super::super::X11ConfigureFlags::all(),
        super::super::ConfigureSource::Compositor,
        Some(1),
    );
    xwm.note_expected_configure_with_context(
        handle,
        second,
        super::super::X11ConfigureFlags::all(),
        super::super::ConfigureSource::Compositor,
        Some(2),
    );

    let conflict = xwm.note_configure_notify(handle, first, Some(2));
    let sequence_only = xwm.note_configure_notify(
        handle,
        X11Geometry {
            x: 999,
            y: 999,
            width: 1,
            height: 1,
        },
        Some(2),
    );
    let metrics = xwm.configure_metrics();

    assert!(conflict.sequence_geometry_conflict);
    assert_eq!(
        sequence_only.classification,
        super::super::ConfigureNotifyClassification::SequenceOnlyRejected
    );
    assert_eq!(metrics.sequence_geometry_conflicts, 1);
    assert_eq!(metrics.sequence_only_matches_rejected, 1);
}

#[test]
fn retired_geometry_metrics_cover_multiple_matches_and_ambiguous_reuse() {
    let generation = generation(226);
    let (mut xwm, _peer) = test_fixture(generation);
    let handle = prepare_managed_window(&mut xwm, 226, true, false, false);
    let repeated = X11Geometry {
        x: 110,
        y: 100,
        width: 630,
        height: 480,
    };
    for (geometry, sequence) in [
        (repeated, 10),
        (X11Geometry { x: 120, ..repeated }, 11),
        (repeated, 20),
        (X11Geometry { x: 130, ..repeated }, 30),
    ] {
        xwm.note_expected_configure_with_context(
            handle,
            geometry,
            super::super::X11ConfigureFlags::all(),
            super::super::ConfigureSource::Compositor,
            Some(sequence),
        );
    }
    xwm.note_configure_notify(handle, X11Geometry { x: 130, ..repeated }, Some(30));
    xwm.note_configure_notify(handle, repeated, Some(20));
    xwm.note_configure_notify(handle, repeated, Some(99));

    let metrics = xwm.configure_metrics();
    assert_eq!(metrics.retired_geometry_multiple_matches, 2);
    assert_eq!(metrics.retired_cookie_match_stale, 1);
    assert_eq!(metrics.retired_geometry_ambiguous_managed, 1);
}

#[test]
fn override_redirect_sequence_collision_applies_external_geometry_and_keeps_pending_self_configure()
{
    let generation = generation(222);
    let (mut xwm, _peer) = test_fixture(generation);
    let handle = prepare_managed_window(&mut xwm, 222, true, false, false);
    let mut snapshot = sync_snapshot(handle, 0);
    snapshot.kind = DesktopWindowKind::OverrideRedirect;
    snapshot.override_redirect = true;
    let record = xwm.windows.get_mut(handle).expect("window record");
    record.kind = DesktopWindowKind::OverrideRedirect;
    record.snapshot = Some(snapshot);
    let pending_geometry = X11Geometry {
        x: 110,
        y: 100,
        width: 630,
        height: 480,
    };
    let external_geometry = X11Geometry {
        x: 220,
        y: 140,
        width: 620,
        height: 470,
    };
    xwm.note_expected_configure_with_context(
        handle,
        pending_geometry,
        super::super::X11ConfigureFlags::all(),
        super::super::ConfigureSource::Compositor,
        Some(10),
    );

    normalize(
        &mut xwm,
        Event::ConfigureNotify(xproto::ConfigureNotifyEvent {
            response_type: 22,
            sequence: 10,
            event: 1,
            window: handle.xid(),
            above_sibling: 0,
            x: external_geometry.x as i16,
            y: external_geometry.y as i16,
            width: external_geometry.width as u16,
            height: external_geometry.height as u16,
            border_width: 0,
            override_redirect: true,
        }),
    )
    .expect("normalize override-redirect sequence collision");

    assert!(xwm.take_events().any(|event| matches!(
        event,
        XwmEvent::ConfigureNotify { window, geometry, .. }
            if window == handle && geometry == external_geometry
    )));
    assert_eq!(
        xwm.windows.get(handle).expect("window record").geometry,
        external_geometry
    );
    assert_eq!(
        xwm.configure_timelines
            .get(&handle)
            .expect("timeline")
            .pending_len(),
        1
    );
    assert_eq!(
        xwm.configure_timelines
            .get(&handle)
            .expect("timeline")
            .acknowledged(),
        None
    );
    assert_eq!(
        xwm.configure_metrics()
            .sequence_collision_client_authoritative_applied,
        1
    );

    normalize(
        &mut xwm,
        Event::ConfigureNotify(xproto::ConfigureNotifyEvent {
            response_type: 22,
            sequence: 10,
            event: 1,
            window: handle.xid(),
            above_sibling: 0,
            x: pending_geometry.x as i16,
            y: pending_geometry.y as i16,
            width: pending_geometry.width as u16,
            height: pending_geometry.height as u16,
            border_width: 0,
            override_redirect: true,
        }),
    )
    .expect("normalize later matching self-configure");
    assert_eq!(
        xwm.configure_timelines
            .get(&handle)
            .expect("timeline")
            .pending_len(),
        0
    );
    assert_eq!(
        xwm.windows.get(handle).expect("window record").geometry,
        external_geometry
    );
}

#[test]
fn managed_sequence_collision_preserves_geometry_and_pending_ownership() {
    let generation = generation(223);
    let (mut xwm, _peer) = test_fixture(generation);
    let handle = prepare_managed_window(&mut xwm, 223, true, false, false);
    let pending_geometry = X11Geometry {
        x: 110,
        y: 100,
        width: 630,
        height: 480,
    };
    let external_geometry = X11Geometry {
        x: 220,
        y: 140,
        width: 620,
        height: 470,
    };
    xwm.note_expected_configure_with_context(
        handle,
        pending_geometry,
        super::super::X11ConfigureFlags::all(),
        super::super::ConfigureSource::Compositor,
        Some(10),
    );

    normalize(
        &mut xwm,
        Event::ConfigureNotify(xproto::ConfigureNotifyEvent {
            response_type: 22,
            sequence: 10,
            event: 1,
            window: handle.xid(),
            above_sibling: 0,
            x: external_geometry.x as i16,
            y: external_geometry.y as i16,
            width: external_geometry.width as u16,
            height: external_geometry.height as u16,
            border_width: 0,
            override_redirect: false,
        }),
    )
    .expect("normalize managed sequence collision");

    assert!(xwm.take_events().all(|event| !matches!(
        event,
        XwmEvent::ConfigureNotify { window, .. } if window == handle
    )));
    assert_eq!(
        xwm.configure_timelines
            .get(&handle)
            .expect("timeline")
            .pending_len(),
        1
    );
    assert_eq!(
        xwm.configure_timelines
            .get(&handle)
            .expect("timeline")
            .desired(),
        pending_geometry
    );
    assert_eq!(
        xwm.configure_metrics().sequence_collision_managed_preserved,
        1
    );
    assert_eq!(xwm.configure_metrics().sequence_only_matches_rejected, 1);
}

#[test]
fn client_positioned_sequence_collision_applies_external_geometry_and_keeps_pending() {
    let generation = generation(224);
    let (mut xwm, _peer) = test_fixture(generation);
    let handle = prepare_managed_window(&mut xwm, 224, true, false, false);
    let mut snapshot = sync_snapshot(handle, 0);
    snapshot.window_types = X11WindowTypes::new(vec![X11WindowType::Notification]);
    xwm.windows.get_mut(handle).expect("window record").snapshot = Some(snapshot);
    let pending_geometry = X11Geometry {
        x: 110,
        y: 100,
        width: 630,
        height: 480,
    };
    let external_geometry = X11Geometry {
        x: 220,
        y: 140,
        width: 620,
        height: 470,
    };
    xwm.note_expected_configure_with_context(
        handle,
        pending_geometry,
        super::super::X11ConfigureFlags::all(),
        super::super::ConfigureSource::Compositor,
        Some(10),
    );

    normalize(
        &mut xwm,
        Event::ConfigureNotify(xproto::ConfigureNotifyEvent {
            response_type: 22,
            sequence: 10,
            event: 1,
            window: handle.xid(),
            above_sibling: 0,
            x: external_geometry.x as i16,
            y: external_geometry.y as i16,
            width: external_geometry.width as u16,
            height: external_geometry.height as u16,
            border_width: 0,
            override_redirect: false,
        }),
    )
    .expect("normalize client-positioned sequence collision");

    assert!(xwm.take_events().any(|event| matches!(
        event,
        XwmEvent::ConfigureNotify { window, geometry, .. }
            if window == handle && geometry == external_geometry
    )));
    assert_eq!(
        xwm.configure_timelines
            .get(&handle)
            .expect("timeline")
            .pending_len(),
        1
    );
    assert_eq!(
        xwm.configure_metrics()
            .sequence_collision_client_authoritative_applied,
        1
    );
}

#[test]
fn transient_parent_relative_sequence_collision_applies_external_geometry() {
    let generation = generation(225);
    let (mut xwm, _peer) = test_fixture(generation);
    let handle = prepare_managed_window(&mut xwm, 225, true, false, false);
    let mut snapshot = sync_snapshot(handle, 0);
    snapshot.transient_for = Some(super::super::X11WindowHandle::new(generation, 999));
    xwm.windows.get_mut(handle).expect("window record").snapshot = Some(snapshot);
    let pending_geometry = X11Geometry {
        x: 110,
        y: 100,
        width: 630,
        height: 480,
    };
    let external_geometry = X11Geometry {
        x: 220,
        y: 140,
        width: 620,
        height: 470,
    };
    xwm.note_expected_configure_with_context(
        handle,
        pending_geometry,
        super::super::X11ConfigureFlags::all(),
        super::super::ConfigureSource::Compositor,
        Some(10),
    );

    normalize(
        &mut xwm,
        Event::ConfigureNotify(xproto::ConfigureNotifyEvent {
            response_type: 22,
            sequence: 10,
            event: 1,
            window: handle.xid(),
            above_sibling: 0,
            x: external_geometry.x as i16,
            y: external_geometry.y as i16,
            width: external_geometry.width as u16,
            height: external_geometry.height as u16,
            border_width: 0,
            override_redirect: false,
        }),
    )
    .expect("normalize transient sequence collision");

    assert!(xwm.take_events().any(|event| matches!(
        event,
        XwmEvent::ConfigureNotify { window, geometry, .. }
            if window == handle && geometry == external_geometry
    )));
    assert_eq!(
        xwm.configure_timelines
            .get(&handle)
            .expect("timeline")
            .pending_len(),
        1
    );
}

#[test]
fn override_redirect_can_reuse_retired_geometry_as_external_state() {
    let generation = generation(217);
    let (mut xwm, _peer) = test_fixture(generation);
    let handle = prepare_managed_window(&mut xwm, 217, true, false, false);
    let mut snapshot = sync_snapshot(handle, 0);
    snapshot.kind = DesktopWindowKind::OverrideRedirect;
    snapshot.override_redirect = true;
    xwm.windows.get_mut(handle).expect("window record").snapshot = Some(snapshot);

    let retired_geometry = X11Geometry {
        x: 110,
        y: 100,
        width: 630,
        height: 480,
    };
    let current_geometry = X11Geometry {
        x: 120,
        y: 100,
        width: 620,
        height: 480,
    };
    xwm.note_expected_configure_with_context(
        handle,
        retired_geometry,
        super::super::X11ConfigureFlags::all(),
        super::super::ConfigureSource::Compositor,
        Some(1),
    );
    xwm.note_expected_configure_with_context(
        handle,
        current_geometry,
        super::super::X11ConfigureFlags::all(),
        super::super::ConfigureSource::Compositor,
        Some(2),
    );

    normalize(
        &mut xwm,
        Event::ConfigureNotify(xproto::ConfigureNotifyEvent {
            response_type: 22,
            sequence: 2,
            event: 1,
            window: handle.xid(),
            above_sibling: 0,
            x: current_geometry.x as i16,
            y: current_geometry.y as i16,
            width: current_geometry.width as u16,
            height: current_geometry.height as u16,
            border_width: 0,
            override_redirect: true,
        }),
    )
    .expect("normalize current override-redirect ConfigureNotify");
    let _ = xwm.take_events().collect::<Vec<_>>();

    normalize(
        &mut xwm,
        Event::ConfigureNotify(xproto::ConfigureNotifyEvent {
            response_type: 22,
            sequence: 0,
            event: 1,
            window: handle.xid(),
            above_sibling: 0,
            x: retired_geometry.x as i16,
            y: retired_geometry.y as i16,
            width: retired_geometry.width as u16,
            height: retired_geometry.height as u16,
            border_width: 0,
            override_redirect: true,
        }),
    )
    .expect("normalize reused retired override-redirect ConfigureNotify");

    assert!(xwm.take_events().any(|event| matches!(
        event,
        XwmEvent::ConfigureNotify { window, geometry, .. }
            if window == handle && geometry == retired_geometry
    )));
}

#[test]
fn client_positioned_notification_can_reuse_retired_geometry_as_external_state() {
    let generation = generation(221);
    let (mut xwm, _peer) = test_fixture(generation);
    let handle = prepare_managed_window(&mut xwm, 221, true, false, false);
    let mut snapshot = sync_snapshot(handle, 0);
    snapshot.window_types = X11WindowTypes::new(vec![X11WindowType::Notification]);
    xwm.windows.get_mut(handle).expect("window record").snapshot = Some(snapshot);

    let retired_geometry = X11Geometry {
        x: 110,
        y: 100,
        width: 630,
        height: 480,
    };
    let current_geometry = X11Geometry {
        x: 120,
        y: 100,
        width: 620,
        height: 480,
    };
    xwm.note_expected_configure_with_context(
        handle,
        retired_geometry,
        super::super::X11ConfigureFlags::all(),
        super::super::ConfigureSource::Compositor,
        Some(1),
    );
    xwm.note_expected_configure_with_context(
        handle,
        current_geometry,
        super::super::X11ConfigureFlags::all(),
        super::super::ConfigureSource::Compositor,
        Some(2),
    );

    for (geometry, sequence) in [(current_geometry, 2), (retired_geometry, 0)] {
        normalize(
            &mut xwm,
            Event::ConfigureNotify(xproto::ConfigureNotifyEvent {
                response_type: 22,
                sequence,
                event: 1,
                window: handle.xid(),
                above_sibling: 0,
                x: geometry.x as i16,
                y: geometry.y as i16,
                width: geometry.width as u16,
                height: geometry.height as u16,
                border_width: 0,
                override_redirect: false,
            }),
        )
        .expect("normalize client-positioned ConfigureNotify");
    }

    assert!(xwm.take_events().any(|event| matches!(
        event,
        XwmEvent::ConfigureNotify { window, geometry, .. }
            if window == handle && geometry == retired_geometry
    )));
}

#[test]
fn command_after_destroy_configure_is_obsolete_not_fatal() {
    let generation = generation(206);
    let (mut xwm, _peer) = test_fixture(generation);
    let handle = prepare_managed_window(&mut xwm, 206, true, false, false);
    xwm.windows.destroy(handle).expect("destroy managed window");

    let result = super::super::commands::execute(
        &mut xwm,
        XwmCommand::Configure {
            window: handle,
            geometry: X11Geometry {
                width: 640,
                height: 480,
                ..X11Geometry::default()
            },
            fields: super::super::X11ConfigureFlags::all(),
            source: super::super::ConfigureSource::Compositor,
            border_width: 0,
        },
    );

    assert!(matches!(
        result,
        Ok(super::super::XwmCommandOutcome::DroppedTargetGone { window: dropped })
            if dropped == handle
    ));
}

#[test]
fn stacking_only_configure_does_not_create_geometry_timeline() {
    let generation = generation(218);
    let (mut xwm, _peer) = test_fixture(generation);
    let handle = prepare_managed_window(&mut xwm, 218, true, false, false);

    let result = super::super::commands::execute(
        &mut xwm,
        XwmCommand::Configure {
            window: handle,
            geometry: X11Geometry {
                x: 110,
                y: 100,
                width: 630,
                height: 480,
            },
            fields: super::super::X11ConfigureFlags {
                sibling: true,
                stack_mode: true,
                ..super::super::X11ConfigureFlags::default()
            },
            source: super::super::ConfigureSource::Compositor,
            border_width: 0,
        },
    );

    assert!(result.is_ok());
    assert!(!xwm.configure_timelines.contains_key(&handle));
}

#[test]
fn unknown_configure_notify_does_not_allocate_timeline_state() {
    let generation = generation(219);
    let (mut xwm, _peer) = test_fixture(generation);
    let handle = super::super::X11WindowHandle::new(generation, 219);

    normalize(
        &mut xwm,
        Event::ConfigureNotify(xproto::ConfigureNotifyEvent {
            response_type: 22,
            sequence: 0,
            event: 1,
            window: handle.xid(),
            above_sibling: 0,
            x: 10,
            y: 20,
            width: 640,
            height: 480,
            border_width: 0,
            override_redirect: false,
        }),
    )
    .expect("normalize unknown ConfigureNotify");

    assert!(!xwm.configure_timelines.contains_key(&handle));
    assert!(xwm.take_events().next().is_none());
}

#[test]
fn observing_a_create_without_mapping_does_not_start_adoption_deadline() {
    let generation = generation(207);
    let (mut xwm, _peer) = test_fixture(generation);
    let handle = super::super::X11WindowHandle::new(generation, 207);

    xwm.observe_window_with_kind(
        handle,
        DesktopWindowKind::OverrideRedirect,
        X11Geometry::default(),
    )
    .expect("observe helper window");

    assert!(
        xwm.adoption.next_deadline_ns().is_none(),
        "unmapped observation must not own a map-adoption deadline"
    );
}

#[test]
fn mapped_managed_window_starts_adoption_deadline_only_after_map_notify() {
    let generation = generation(215);
    let (mut xwm, _peer) = test_fixture(generation);
    let handle = super::super::X11WindowHandle::new(generation, 215);
    xwm.observe_window_with_kind(handle, DesktopWindowKind::Managed, X11Geometry::default())
        .expect("observe managed window");
    assert!(xwm.adoption.next_deadline_ns().is_none());

    normalize(&mut xwm, map_event(handle.xid(), false)).expect("external MapNotify");

    assert!(xwm.adoption.next_deadline_ns().is_some());
}

#[test]
fn destroy_clears_an_active_adoption_deadline() {
    let generation = generation(208);
    let (mut xwm, _peer) = test_fixture(generation);
    let handle = prepare_managed_window(&mut xwm, 208, true, false, false);
    xwm.adoption.observe(
        handle,
        super::super::adoption::AdoptionWait::MapToAssociation,
        10,
    );

    normalize(
        &mut xwm,
        Event::DestroyNotify(xproto::DestroyNotifyEvent {
            response_type: 17,
            sequence: 0,
            event: 1,
            window: handle.xid(),
        }),
    )
    .expect("destroy mapped window");

    assert!(
        xwm.adoption.next_deadline_ns().is_none(),
        "destroy must release adoption ownership immediately"
    );
}

#[test]
fn unmap_clears_an_active_adoption_deadline() {
    let generation = generation(211);
    let (mut xwm, _peer) = test_fixture(generation);
    let handle = prepare_managed_window(&mut xwm, 211, true, false, false);
    xwm.adoption.observe(
        handle,
        super::super::adoption::AdoptionWait::MapToAssociation,
        10,
    );

    normalize(&mut xwm, unmap_event(handle.xid())).expect("unmap managed window");

    assert!(xwm.adoption.next_deadline_ns().is_none());
}

#[test]
fn hundreds_of_unmapped_observations_never_enter_adoption_tracking() {
    let generation = generation(209);
    let (mut xwm, _peer) = test_fixture(generation);

    for xid in 1..=500 {
        let handle = super::super::X11WindowHandle::new(generation, xid);
        xwm.observe_window_with_kind(
            handle,
            DesktopWindowKind::OverrideRedirect,
            X11Geometry::default(),
        )
        .expect("observe helper window");
    }

    assert_eq!(xwm.adoption.pending_len(), 0);
    assert!(!xwm.collect_adoption_expirations(10));
}

#[test]
fn hundreds_of_mapped_adoption_expirations_are_collected_as_one_bounded_cycle() {
    let generation = generation(210);
    let (mut xwm, _peer) = test_fixture(generation);

    for xid in 1..=500 {
        let handle = prepare_managed_window(&mut xwm, xid, true, false, false);
        xwm.adoption.observe(
            handle,
            super::super::adoption::AdoptionWait::MapToAssociation,
            10,
        );
    }

    assert_eq!(xwm.adoption.pending_len(), 500);
    assert!(xwm.collect_adoption_expirations(10));
    assert!(!xwm.collect_adoption_expirations(10));
    assert_eq!(xwm.adoption.pending_len(), 0);
}

#[test]
fn target_gone_single_target_commands_are_nonfatal_after_destroy() {
    let generation = generation(212);
    let (mut xwm, _peer) = test_fixture(generation);
    let commands = (0..14)
        .map(|offset| {
            let handle = prepare_managed_window(&mut xwm, 212 + offset, true, false, false);
            let command = match offset {
                0 => XwmCommand::Map(handle),
                1 => XwmCommand::Unmap(handle),
                2 => XwmCommand::Configure {
                    window: handle,
                    geometry: X11Geometry::default(),
                    fields: super::super::X11ConfigureFlags::all(),
                    source: super::super::ConfigureSource::Compositor,
                    border_width: 0,
                },
                3 => XwmCommand::ConfigureFrame {
                    window: handle,
                    geometry: X11Geometry::default(),
                },
                4 => XwmCommand::ConfigureNotify {
                    window: handle,
                    geometry: X11Geometry::default(),
                },
                5 => XwmCommand::Focus {
                    window: Some(handle),
                    timestamp: 1,
                },
                6 => XwmCommand::Raise(handle),
                7 => XwmCommand::Close(handle),
                8 => XwmCommand::SetState {
                    window: handle,
                    state: Default::default(),
                },
                9 => XwmCommand::BeginResizeSync {
                    window: handle,
                    geometry: X11Geometry::default(),
                    counter_value: 1,
                    deadline_ns: 10,
                    final_pending: false,
                },
                10 => XwmCommand::SetAllowCommits {
                    window: handle,
                    allowed: true,
                },
                11 => XwmCommand::ReleaseResizeCommits {
                    window: handle,
                    counter_value: 1,
                    association_serial: NonZeroU64::new(1).expect("nonzero serial"),
                    commit_floor: crate::compositor::SurfaceCommitSequence(0),
                },
                12 => XwmCommand::CompleteResizeSync(handle),
                13 => XwmCommand::Stack {
                    window: handle,
                    sibling: None,
                    mode: super::super::X11StackMode::Above,
                },
                _ => unreachable!(),
            };
            xwm.windows.destroy(handle).expect("destroy target");
            (handle, command)
        })
        .collect::<Vec<_>>();

    for (handle, command) in commands {
        assert!(matches!(
            super::super::commands::execute(&mut xwm, command),
            Ok(super::super::XwmCommandOutcome::DroppedTargetGone { window })
                if window == handle
        ));
    }
}

#[test]
fn stale_generation_commands_are_dropped_without_touching_current_xwm() {
    let current_generation = generation(213);
    let stale_generation = generation(214);
    let (mut xwm, _peer) = test_fixture(current_generation);
    let stale = super::super::X11WindowHandle::new(stale_generation, 213);

    assert!(matches!(
        super::super::commands::execute(
            &mut xwm,
            XwmCommand::Configure {
                window: stale,
                geometry: X11Geometry::default(),
                fields: super::super::X11ConfigureFlags::all(),
                source: super::super::ConfigureSource::Compositor,
                border_width: 0,
            },
        ),
        Ok(super::super::XwmCommandOutcome::DroppedStaleGeneration { window: Some(window) })
            if window == stale
    ));
    assert_eq!(xwm.generation, current_generation);
}

#[test]
fn multi_target_commands_prune_dead_handles_and_dead_siblings() {
    let generation = generation(214);
    let (mut xwm, _peer) = test_fixture(generation);
    let live = prepare_managed_window(&mut xwm, 214, true, false, false);
    let sibling = prepare_managed_window(&mut xwm, 215, true, false, false);
    let dead = prepare_managed_window(&mut xwm, 216, true, false, false);
    xwm.windows.destroy(dead).expect("destroy dead list member");

    assert!(matches!(
        super::super::commands::execute(
            &mut xwm,
            XwmCommand::SyncClientLists {
                client_list: vec![dead, live, live],
                stacking: vec![sibling, dead],
            },
        ),
        Ok(super::super::XwmCommandOutcome::AppliedAfterPruning { dropped_handles })
            if dropped_handles == 3
    ));
    assert!(matches!(
        super::super::commands::execute(
            &mut xwm,
            XwmCommand::Stack {
                window: live,
                sibling: Some(dead),
                mode: super::super::X11StackMode::Above,
            },
        ),
        Ok(super::super::XwmCommandOutcome::AppliedAfterPruning { dropped_handles: 1 })
    ));
    assert!(matches!(
        super::super::commands::execute(
            &mut xwm,
            XwmCommand::RestackExact {
                order: vec![dead, live],
                client_list: vec![dead, live],
                stacking: vec![live, dead],
            },
        ),
        Ok(super::super::XwmCommandOutcome::AppliedAfterPruning { dropped_handles: 3 })
    ));
}

#[test]
fn xsync_request_precedes_configure() {
    let generation = generation(201);
    let (mut xwm, mut peer) = test_fixture(generation);
    let handle = prepare_managed_window(&mut xwm, 201, true, false, false);
    xwm.capabilities.sync = true;
    xwm.connection.set_extensions(HashMap::from([(
        sync::X11_EXTENSION_NAME,
        ExtensionInformation {
            major_opcode: 128,
            first_event: 0,
            first_error: 0,
        },
    )]));
    xwm.windows
        .get_mut(handle)
        .expect("managed window")
        .snapshot = Some(sync_snapshot(handle, 41));

    super::super::commands::begin_resize_sync(
        &mut xwm,
        handle,
        X11Geometry {
            x: 100,
            y: 100,
            width: 900,
            height: 700,
        },
        0,
        100,
        false,
    )
    .expect("begin synchronized resize");
    xwm.flush().expect("flush resize requests");

    let core_opcodes = request_opcodes(&read_fixture_requests(&mut peer))
        .into_iter()
        .filter(|opcode| matches!(*opcode, 12 | 18 | 25))
        .collect::<Vec<_>>();
    assert_eq!(
        core_opcodes,
        vec![18, 25, 12],
        "allow-off, sync request, and ConfigureWindow must be ordered"
    );
}

#[test]
fn focus_command_uses_pointer_root_and_remains_pending_until_focus_in() {
    let generation = generation(219);
    let (mut xwm, mut peer) = test_fixture(generation);
    let handle = prepare_managed_window(&mut xwm, 219, true, false, false);

    assert!(matches!(
        super::super::commands::execute(
            &mut xwm,
            XwmCommand::Focus {
                window: Some(handle),
                timestamp: 55,
            },
        ),
        Ok(super::super::XwmCommandOutcome::Applied)
    ));
    xwm.flush().expect("flush focus request");
    let requests = read_fixture_requests(&mut peer);
    assert_eq!(request_opcodes(&requests).first().copied(), Some(42));
    assert_eq!(
        requests.get(1).copied(),
        Some(1),
        "POINTER_ROOT revert mode"
    );
    assert_eq!(xwm.focus.desired_focus(), Some(handle.xid()));
    assert_eq!(xwm.focus.confirmed_focus(), None);
    assert!(xwm.focus.pending_focus().is_some());
}

#[test]
fn focus_command_respects_each_icccm_focus_model() {
    let cases = [
        (
            Some(true),
            false,
            super::super::focus::FocusModel::Input,
            vec![42, 18],
        ),
        (
            Some(false),
            true,
            super::super::focus::FocusModel::TakeFocusOnly,
            vec![25, 18],
        ),
        (
            Some(false),
            false,
            super::super::focus::FocusModel::NoFocus,
            vec![18],
        ),
    ];
    for (offset, (accepts_input, supports_take_focus, model, expected_opcodes)) in
        cases.into_iter().enumerate()
    {
        let generation = generation(220 + offset as u64);
        let (mut xwm, mut peer) = test_fixture(generation);
        let handle = prepare_managed_window(&mut xwm, 220 + offset as u32, true, false, false);
        let mut snapshot = sync_snapshot(handle, 0);
        snapshot.accepts_input = accepts_input;
        snapshot.supports_take_focus = supports_take_focus;
        xwm.windows
            .get_mut(handle)
            .expect("managed window")
            .snapshot = Some(snapshot);

        assert!(matches!(
            super::super::commands::execute(
                &mut xwm,
                XwmCommand::Focus {
                    window: Some(handle),
                    timestamp: 55,
                },
            ),
            Ok(super::super::XwmCommandOutcome::Applied)
        ));
        xwm.flush().expect("flush focus request");
        assert_eq!(
            request_opcodes(&read_fixture_requests(&mut peer)),
            expected_opcodes
        );
        let pending = xwm.focus.pending_focus().expect("focus remains pending");
        assert_eq!(pending.model, model);
        assert_eq!(
            pending.sent_set_input_focus,
            matches!(model, super::super::focus::FocusModel::Input)
        );
        assert_eq!(
            pending.sent_take_focus,
            matches!(model, super::super::focus::FocusModel::TakeFocusOnly)
        );
    }
}

#[test]
fn sync_counter_initialized_once_on_manage() {
    let generation = generation(202);
    let (mut xwm, mut peer) = test_fixture(generation);
    let handle = prepare_managed_window(&mut xwm, 202, true, false, false);
    xwm.capabilities.sync = true;
    xwm.connection.set_extensions(HashMap::from([(
        sync::X11_EXTENSION_NAME,
        ExtensionInformation {
            major_opcode: 128,
            first_event: 0,
            first_error: 0,
        },
    )]));
    xwm.windows
        .get_mut(handle)
        .expect("managed window")
        .snapshot = Some(sync_snapshot(handle, 0xfeed));

    let geometry = X11Geometry {
        x: 100,
        y: 100,
        width: 900,
        height: 700,
    };
    super::super::commands::begin_resize_sync(&mut xwm, handle, geometry, 0, 100, false)
        .expect("begin first synchronized resize");
    xwm.flush().expect("flush first resize requests");
    let first_bytes = read_fixture_requests(&mut peer);
    assert_eq!(
        request_minor_opcodes(&first_bytes, 128)
            .into_iter()
            .filter(|minor| *minor == sync::SET_COUNTER_REQUEST)
            .count(),
        1,
        "the arbitrary client counter must be initialized exactly once"
    );
    let first_counter = sync_request_counter_values(&first_bytes)
        .into_iter()
        .next()
        .expect("first sync request serial");
    assert_ne!(first_counter, 0, "sync request serial must be nonzero");

    super::super::commands::begin_resize_sync(
        &mut xwm,
        handle,
        X11Geometry {
            width: 901,
            height: 701,
            ..geometry
        },
        0,
        200,
        false,
    )
    .expect("coalesce second resize");
    xwm.flush().expect("flush coalesced resize");
    let second_bytes = read_fixture_requests(&mut peer);
    assert_eq!(
        request_minor_opcodes(&second_bytes, 128)
            .into_iter()
            .filter(|minor| *minor == sync::SET_COUNTER_REQUEST)
            .count(),
        0,
        "a pending transaction must not reinitialize the same counter"
    );
}

#[test]
fn same_geometry_final_does_not_start_sync_roundtrip() {
    let generation = generation(203);
    let (mut xwm, mut peer) = test_fixture(generation);
    let handle = prepare_managed_window(&mut xwm, 203, true, false, false);
    let serial = NonZeroU64::new(0x1234).expect("association serial");
    let geometry = X11Geometry {
        x: 100,
        y: 100,
        width: 900,
        height: 700,
    };
    xwm.resize_sync
        .begin_transaction(handle, 7, 100, geometry, false)
        .expect("begin transaction");
    assert!(xwm.resize_sync.acknowledge(handle, 7));
    assert!(xwm.resize_sync.release_commits(
        handle,
        7,
        serial,
        crate::compositor::SurfaceCommitSequence(0),
    ));
    assert_eq!(
        xwm.resize_sync
            .note_commit(handle, serial, crate::compositor::SurfaceCommitSequence(1)),
        super::super::ResizeSyncCommit::Presented
    );
    assert!(xwm.resize_sync.queue_desired(handle, geometry, true));

    xwm.complete_resize_sync(handle)
        .expect("complete presented resize");
    xwm.flush().expect("flush final resize state");

    assert_eq!(xwm.resize_sync.state(handle), ResizeSyncState::Idle);
    assert!(xwm.resize_sync.desired(handle).is_none());
    assert!(matches!(
        xwm.outgoing_events.front(),
        Some(super::super::XwmEvent::ResizeSyncPresented { window, .. })
            if *window == handle
    ));
    assert!(
        request_opcodes(&read_fixture_requests(&mut peer))
            .into_iter()
            .find(|opcode| *opcode == 12)
            .is_none(),
        "same-geometry finalization must not send another ConfigureWindow"
    );
}

#[test]
fn position_only_move_bypasses_pending_resize_size_queue() {
    let generation = generation(204);
    let (mut xwm, mut peer) = test_fixture(generation);
    let handle = prepare_managed_window(&mut xwm, 204, true, false, false);
    let resize_geometry = X11Geometry {
        x: 100,
        y: 100,
        width: 900,
        height: 700,
    };
    xwm.resize_sync
        .begin_transaction(handle, 7, 100, resize_geometry, false)
        .expect("begin resize transaction");

    let move_geometry = X11Geometry {
        x: 300,
        y: 250,
        ..resize_geometry
    };
    super::super::commands::execute(
        &mut xwm,
        XwmCommand::Configure {
            window: handle,
            geometry: move_geometry,
            fields: super::super::X11ConfigureFlags {
                x: true,
                y: true,
                ..Default::default()
            },
            source: super::super::ConfigureSource::Compositor,
            border_width: 0,
        },
    )
    .expect("position-only configure");
    xwm.flush().expect("flush position-only configure");

    assert!(
        xwm.resize_sync.desired(handle).is_none(),
        "position-only movement must not become pending content geometry"
    );
    assert_eq!(
        request_opcodes(&read_fixture_requests(&mut peer))
            .into_iter()
            .filter(|opcode| *opcode == 12)
            .count(),
        1,
        "position-only configure must be sent immediately"
    );

    assert!(
        xwm.resize_sync
            .queue_desired(handle, resize_geometry, true,)
    );
    let newer_move_geometry = X11Geometry {
        x: 340,
        y: 290,
        ..resize_geometry
    };
    super::super::commands::execute(
        &mut xwm,
        XwmCommand::Configure {
            window: handle,
            geometry: newer_move_geometry,
            fields: super::super::X11ConfigureFlags {
                x: true,
                y: true,
                ..Default::default()
            },
            source: super::super::ConfigureSource::Compositor,
            border_width: 0,
        },
    )
    .expect("position-only configure while a final content target is queued");
    assert_eq!(
        xwm.resize_sync
            .desired(handle)
            .map(|desired| desired.geometry),
        Some(newer_move_geometry),
        "a pending content target may retain its size, but must use the newer compositor position"
    );
}

#[test]
fn runtime_timeout_records_original_counter_and_matching_late_ack_reenables_future_sync() {
    let generation = generation(1);
    let (mut xwm, _peer) = test_fixture(generation);
    let handle = prepare_managed_window(&mut xwm, 71, true, false, false);

    xwm.resize_sync
        .begin_transaction(handle, 19, 100, X11Geometry::default(), false)
        .expect("begin resize transaction");
    xwm.handle_resize_sync_deadline(100)
        .expect("handle resize timeout");

    assert_eq!(
        xwm.timed_out_resize_counters.get(&handle),
        Some(&19),
        "timeout recovery must retain the original nonzero counter"
    );
    assert!(xwm.resize_sync.sync_disabled(handle));

    xwm.note_resize_sync_ack_for_test(handle, 20);
    assert!(xwm.resize_sync.sync_disabled(handle));

    xwm.note_resize_sync_ack_for_test(handle, 19);
    assert!(!xwm.resize_sync.sync_disabled(handle));
    assert!(!xwm.timed_out_resize_counters.contains_key(&handle));
    assert_eq!(xwm.resize_sync.state(handle), ResizeSyncState::Idle);
}

#[test]
fn iconic_wayland_surface_removal_preserves_window_identity() {
    let generation = generation(29);
    let (mut xwm, _peer) = test_fixture(generation);
    let handle = prepare_managed_window(&mut xwm, 129, true, false, false);
    xwm.note_x11_surface_serial(handle, 0x1234, 0)
        .expect("X11 surface serial");
    xwm.ingest_wayland_association(XwaylandAssociationEvent::Committed {
        generation,
        serial: NonZeroU64::new(0x1234).expect("surface serial"),
        surface_id: 42,
    })
    .expect("Wayland association");
    xwm.mark_window_buffer_ready(handle)
        .expect("buffer readiness");
    normalize(&mut xwm, map_event(handle.xid(), false)).expect("MapNotify");
    assert!(matches!(
        ready_events(&mut xwm).as_slice(),
        [XwmEvent::WindowReady(snapshot)] if snapshot.handle == handle
    ));
    let association = xwm
        .windows
        .get(handle)
        .and_then(|record| record.association)
        .expect("ready window association");

    super::super::commands::execute(&mut xwm, XwmCommand::Unmap(handle)).expect("WM unmap command");
    normalize(&mut xwm, unmap_event(handle.xid())).expect("WM UnmapNotify");
    assert!(ready_events(&mut xwm).is_empty());

    xwm.ingest_wayland_association(XwaylandAssociationEvent::Removed {
        generation,
        serial: association.serial,
        surface_id: association.surface_id,
    })
    .expect("old Wayland surface removal");

    assert!(ready_events(&mut xwm).is_empty());
    let record = xwm.windows.get(handle).expect("iconic window record");
    assert_eq!(record.lifecycle, X11WindowLifecycle::Iconic);
    assert!(record.snapshot.is_some());
    assert!(record.association.is_none());
    assert!(!record.buffer_ready);
}

#[test]
fn old_surface_removal_after_new_map_association_keeps_replacement() {
    let generation = generation(30);
    let (mut xwm, _peer) = test_fixture(generation);
    let handle = prepare_managed_window(&mut xwm, 130, true, false, false);
    xwm.note_x11_surface_serial(handle, 0x1234, 0)
        .expect("old X11 surface serial");
    xwm.ingest_wayland_association(XwaylandAssociationEvent::Committed {
        generation,
        serial: NonZeroU64::new(0x1234).expect("old serial"),
        surface_id: 42,
    })
    .expect("old Wayland association");
    xwm.mark_window_buffer_ready(handle)
        .expect("old buffer readiness");
    normalize(&mut xwm, map_event(handle.xid(), false)).expect("first MapNotify");
    assert_eq!(ready_surface_id(&ready_events(&mut xwm)), Some(42));
    let old_association = xwm
        .windows
        .get(handle)
        .and_then(|record| record.association)
        .expect("old association");

    super::super::commands::execute(&mut xwm, XwmCommand::Unmap(handle)).expect("WM unmap command");
    normalize(&mut xwm, unmap_event(handle.xid())).expect("WM UnmapNotify");
    super::super::commands::execute(&mut xwm, XwmCommand::Map(handle))
        .expect("restore map command");

    xwm.note_x11_surface_serial(handle, 0x5678, 0)
        .expect("new X11 surface serial");
    xwm.ingest_wayland_association(XwaylandAssociationEvent::Committed {
        generation,
        serial: NonZeroU64::new(0x5678).expect("new serial"),
        surface_id: 43,
    })
    .expect("new Wayland association");
    let new_association = xwm
        .windows
        .get(handle)
        .and_then(|record| record.association)
        .expect("replacement association");
    assert_eq!(new_association.surface_id, 43);
    assert!(new_association.map_serial > old_association.map_serial);

    xwm.ingest_wayland_association(XwaylandAssociationEvent::Removed {
        generation,
        serial: old_association.serial,
        surface_id: old_association.surface_id,
    })
    .expect("late old Wayland surface removal");

    assert!(ready_events(&mut xwm).is_empty());
    assert_eq!(
        xwm.windows
            .get(handle)
            .and_then(|record| record.association)
            .map(|association| association.surface_id),
        Some(43)
    );
}

#[test]
fn iconic_client_map_request_starts_a_new_map_epoch() {
    let generation = generation(31);
    let (mut xwm, mut peer) = test_fixture(generation);
    let handle = prepare_managed_window(&mut xwm, 131, true, false, false);

    super::super::commands::execute(&mut xwm, XwmCommand::Unmap(handle)).expect("WM unmap command");
    normalize(&mut xwm, unmap_event(handle.xid())).expect("WM UnmapNotify");
    assert_eq!(
        xwm.windows.lifecycle(handle),
        Some(super::super::X11WindowLifecycle::Iconic)
    );

    normalize(
        &mut xwm,
        Event::MapRequest(xproto::MapRequestEvent {
            response_type: 20,
            sequence: 0,
            parent: 1,
            window: handle.xid(),
        }),
    )
    .expect("client MapRequest");
    complete_property_refresh(&mut xwm, &mut peer);

    let events = ready_events(&mut xwm);
    assert!(
        events.iter().any(
            |event| matches!(event, XwmEvent::WindowMapRequested(window) if *window == handle)
        )
    );

    normalize(
        &mut xwm,
        Event::MapRequest(xproto::MapRequestEvent {
            response_type: 20,
            sequence: 0,
            parent: 1,
            window: handle.xid(),
        }),
    )
    .expect("duplicate client MapRequest");
    assert!(ready_events(&mut xwm).is_empty());
}

#[test]
fn restore_before_old_surface_removed_preserves_the_new_map_epoch() {
    let generation = generation(32);
    let (mut xwm, _peer) = test_fixture(generation);
    let handle = prepare_managed_window(&mut xwm, 132, true, false, false);
    xwm.note_x11_surface_serial(handle, 0x1234, 0)
        .expect("old X11 surface serial");
    xwm.ingest_wayland_association(XwaylandAssociationEvent::Committed {
        generation,
        serial: NonZeroU64::new(0x1234).expect("old serial"),
        surface_id: 42,
    })
    .expect("old Wayland association");
    xwm.mark_window_buffer_ready(handle)
        .expect("old buffer readiness");
    normalize(&mut xwm, map_event(handle.xid(), false)).expect("MapNotify");
    let _ = ready_events(&mut xwm);
    let association = xwm
        .windows
        .get(handle)
        .and_then(|record| record.association)
        .expect("old association");

    super::super::commands::execute(&mut xwm, XwmCommand::Unmap(handle)).expect("WM unmap command");
    normalize(&mut xwm, unmap_event(handle.xid())).expect("WM UnmapNotify");
    let _ = ready_events(&mut xwm);
    super::super::commands::execute(&mut xwm, XwmCommand::Map(handle))
        .expect("restore map command");

    xwm.ingest_wayland_association(XwaylandAssociationEvent::Removed {
        generation,
        serial: association.serial,
        surface_id: association.surface_id,
    })
    .expect("old surface removal during restore");

    assert!(ready_events(&mut xwm).is_empty());
    let record = xwm.windows.get(handle).expect("restoring window record");
    assert!(record.snapshot.is_some());
    assert_eq!(
        record.lifecycle,
        super::super::X11WindowLifecycle::MapCommanded
    );
}
