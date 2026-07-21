use std::num::NonZeroU64;

use crate::xwayland::XwaylandAssociationEvent;

use super::super::{ResizeSyncState, XwmCommand, XwmEvent};
use super::tests::{
    generation, map_event, prepare_managed_window, ready_events, ready_surface_id, test_fixture,
    unmap_event,
};
use super::{X11Geometry, X11WindowLifecycle, normalize};

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
