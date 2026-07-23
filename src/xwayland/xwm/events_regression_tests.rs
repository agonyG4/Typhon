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
use super::super::{ResizeSyncState, XwmCommand, XwmEvent};
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

#[test]
fn configure_request_and_destroy_notify_are_normalized_in_one_x11_drain() {
    let generation = generation(205);
    let (mut xwm, mut peer) = test_fixture(generation);
    let handle = prepare_managed_window(&mut xwm, 205, true, false, false);

    let configure = xproto::ConfigureRequestEvent {
        response_type: 23,
        sequence: 0,
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
            XwmEvent::ConfigureRequested { window, .. } if *window == handle
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
