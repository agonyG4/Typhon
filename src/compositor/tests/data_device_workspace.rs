use super::*;
use std::os::unix::net::UnixStream;
use wayland_client::{Connection, Proxy, globals::registry_queue_init};

#[test]
fn sourced_drag_retargets_workspace_at_stationary_pointer_without_cancellation() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let _pointer = seat.get_pointer(&qh, ());
    let manager: client_wl_data_device_manager::WlDataDeviceManager =
        globals.bind(&qh, 1..=3, ()).unwrap();
    let device = manager.get_data_device(&seat, &qh, ());
    let (destination, destination_xdg, _destination_toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    destination.commit();
    connection.flush().unwrap();

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commit_registered_initial_xdg_test_buffer(&destination_xdg);
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let placement = SurfacePlacement::absolute_root_at(
        render::FIRST_SURFACE_OFFSET.0,
        render::FIRST_SURFACE_OFFSET.1,
    );
    focus_root_window(&commands, destination.id().protocol_id());
    set_focused_root_visual_geometry(&commands, placement, 160, 120);
    commands
        .send(ServerCommand::ActivateWorkspace { workspace: 2 })
        .unwrap();
    wait_for_server_commands(&commands);

    let (origin, origin_xdg, _origin_toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    let icon = compositor.create_surface(&qh, ());
    let source = manager.create_data_source(&qh, ());
    source.offer("text/plain".to_string());
    source.set_actions(client_wl_data_device_manager::DndAction::Copy);
    origin.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    commit_registered_initial_xdg_test_buffer(&origin_xdg);
    commit_test_buffered_surface(&icon, &shm, &qh, 24, 24).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    focus_root_window(&commands, origin.id().protocol_id());
    set_focused_root_visual_geometry(&commands, placement, 160, 120);
    let pointer_x = f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0;
    let pointer_y = f64::from(render::FIRST_SURFACE_OFFSET.1) + 20.0;
    commands
        .send(ServerCommand::PointerMotion {
            x: pointer_x,
            y: pointer_y,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(
        state.pointer_enter_surface_id,
        Some(origin.id().protocol_id()),
        "pointer must be over the visible drag origin before pressing"
    );
    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let serial = state
        .pointer_button_serial
        .expect("start_drag must use the physical pointer press serial");

    device.start_drag(Some(&source), &origin, Some(&icon), serial);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.data_source_cancelled_count, 0);
    assert_eq!(
        state.data_device_enter_count, 1,
        "accepted StartDrag must enter its current visible target"
    );
    let icon_surface_id = capture_active_drag_icon_surface(&commands)
        .expect("the client supplied drag icon must be active");
    commands
        .send(ServerCommand::PointerButton {
            button: 0x111,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    state.event_timeline.clear();
    let pointer_motion_count = state.pointer_motion_count;

    commands
        .send(ServerCommand::ActivateWorkspace { workspace: 1 })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    assert_eq!(state.data_source_cancelled_count, 0);
    assert!(capture_active_drag_icon_surface(&commands).is_some());
    assert_eq!(
        state.data_device_enter_surface_id,
        Some(destination.id().protocol_id())
    );
    assert_eq!(state.pointer_motion_count, pointer_motion_count);
    assert!(
        !state
            .event_timeline
            .iter()
            .any(|event| matches!(event, TestWaylandEvent::PointerMotion))
    );

    let destination_offer = state
        .data_device_drag_offer
        .clone()
        .expect("workspace reconciliation must create a destination offer");
    let destination_enter_serial = state
        .data_device_enter_serial
        .expect("destination enter must carry a serial");
    destination_offer.accept(destination_enter_serial, Some("text/plain".to_string()));
    destination_offer.set_actions(
        client_wl_data_device_manager::DndAction::Copy,
        client_wl_data_device_manager::DndAction::Copy,
    );
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    // Return to the origin workspace and then revisit the destination without
    // moving the pointer; the same client-supplied icon and session survive.
    commands
        .send(ServerCommand::ActivateWorkspace { workspace: 2 })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.data_source_cancelled_count, 0);
    assert_eq!(
        state.data_device_enter_surface_id,
        Some(origin.id().protocol_id())
    );
    assert_eq!(
        capture_active_drag_icon_surface(&commands),
        Some(icon_surface_id)
    );

    let origin_internal_id = resolve_internal_surface_id(&commands, origin.id().protocol_id());
    let origin_window_id = capture_window_id_for_surface(&commands, origin_internal_id)
        .expect("origin surface must resolve to its managed window");
    let enters_before_move = state.data_device_enter_count;
    let leaves_before_move = state.data_device_leave_count;
    let (move_reply, move_result) = std::sync::mpsc::channel();
    commands
        .send(ServerCommand::MoveWindowToWorkspace {
            window_id: origin_window_id,
            workspace: 3,
            reply: move_reply,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    assert!(move_result.recv().unwrap());
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.data_source_cancelled_count, 0);
    assert_eq!(state.data_device_enter_count, enters_before_move);
    assert_eq!(state.data_device_leave_count, leaves_before_move + 1);
    assert_eq!(
        capture_active_drag_icon_surface(&commands),
        Some(icon_surface_id)
    );

    let (move_reply, move_result) = std::sync::mpsc::channel();
    commands
        .send(ServerCommand::MoveWindowToWorkspace {
            window_id: origin_window_id,
            workspace: 2,
            reply: move_reply,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    assert!(move_result.recv().unwrap());
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.data_source_cancelled_count, 0);
    assert_eq!(state.data_device_enter_count, enters_before_move + 1);
    assert_eq!(
        state.data_device_enter_surface_id,
        Some(origin.id().protocol_id())
    );

    commands
        .send(ServerCommand::ActivateWorkspace { workspace: 1 })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.data_source_cancelled_count, 0);
    assert_eq!(
        state.data_device_enter_surface_id,
        Some(destination.id().protocol_id())
    );
    assert_eq!(
        capture_active_drag_icon_surface(&commands),
        Some(icon_surface_id)
    );

    let final_offer = state
        .data_device_drag_offer
        .clone()
        .expect("round-trip reconciliation must offer the final target");
    let final_enter_serial = state
        .data_device_enter_serial
        .expect("final destination enter must carry a serial");
    final_offer.accept(final_enter_serial, Some("text/plain".to_string()));
    final_offer.set_actions(
        client_wl_data_device_manager::DndAction::Copy,
        client_wl_data_device_manager::DndAction::Copy,
    );
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    // The initiating left release is not final while another physical button
    // remains held. Only the last release may perform the drop.
    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: false,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.data_device_drop_count, 0);
    assert_eq!(state.data_source_cancelled_count, 0);
    assert_eq!(
        capture_active_drag_icon_surface(&commands),
        Some(icon_surface_id)
    );
    commands
        .send(ServerCommand::PointerButton {
            button: 0x111,
            pressed: false,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.data_source_cancelled_count, 0);
    assert_eq!(state.data_device_drop_count, 1);
    assert_eq!(state.data_source_dnd_drop_performed_count, 1);
    assert_eq!(
        capture_active_drag_icon_surface(&commands),
        Some(icon_surface_id),
        "the offer remains live through source finish"
    );
    let icon_internal_id = resolve_internal_surface_id(&commands, icon.id().protocol_id());
    assert!(
        !capture_renderable_surface_snapshot(&commands)
            .iter()
            .any(|surface| surface.surface_id == icon_internal_id),
        "the drag icon must be hidden at physical drop"
    );
    assert_eq!(
        capture_surface_role_state(&commands, icon_internal_id),
        ("drag_icon".to_string(), false)
    );

    final_offer.finish();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.data_source_dnd_finished_count, 1);
    assert_eq!(capture_active_drag_icon_surface(&commands), None);

    // Special workspace open/close must leave the same drag target intact.
    let special_icon = compositor.create_surface(&qh, ());
    let special_source = manager.create_data_source(&qh, ());
    special_source.offer("text/plain".to_string());
    special_source.set_actions(client_wl_data_device_manager::DndAction::Copy);
    commit_test_buffered_surface(&special_icon, &shm, &qh, 24, 24).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let special_serial = state
        .pointer_button_serial
        .expect("special-workspace drag must use a physical press serial");
    device.start_drag(
        Some(&special_source),
        &destination,
        Some(&special_icon),
        special_serial,
    );
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let special_icon_surface_id = capture_active_drag_icon_surface(&commands)
        .expect("special-workspace drag icon must be active");
    let enters_before_special = state.data_device_enter_count;
    let leaves_before_special = state.data_device_leave_count;
    commands
        .send(ServerCommand::ToggleDefaultSpecialWorkspace)
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    commands
        .send(ServerCommand::ToggleDefaultSpecialWorkspace)
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.data_source_cancelled_count, 0);
    assert_eq!(state.data_device_enter_count, enters_before_special);
    assert_eq!(state.data_device_leave_count, leaves_before_special);
    assert_eq!(
        capture_active_drag_icon_surface(&commands),
        Some(special_icon_surface_id)
    );

    // A second held button belongs to the same physical Wayland DnD grab.
    commands
        .send(ServerCommand::PointerButton {
            button: 0x111,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    cancel_active_drag_for_test(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_active_drag_icon_surface(&commands), None);
    let dnd_enters_after_cancel = state.data_device_enter_count;
    let dnd_leaves_after_cancel = state.data_device_leave_count;
    let drops_after_cancel = state.data_device_drop_count;
    let finishes_after_cancel = state.data_source_dnd_finished_count;

    // Special-workspace scene rebuilds and moving the DnD origin family out
    // of the visible scene must also preserve this cancelled physical grab.
    let destination_internal_id =
        resolve_internal_surface_id(&commands, destination.id().protocol_id());
    let destination_window_id = capture_window_id_for_surface(&commands, destination_internal_id)
        .expect("cancelled DnD origin must remain a mapped window");
    let (move_reply, move_result) = std::sync::mpsc::channel();
    commands
        .send(ServerCommand::MoveWindowToWorkspace {
            window_id: destination_window_id,
            workspace: 3,
            reply: move_reply,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    assert!(move_result.recv().unwrap());
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.data_source_cancelled_count, 1);
    assert_eq!(capture_active_drag_icon_surface(&commands), None);
    let pointer_motion_after_origin_move = state.pointer_motion_count;
    commands
        .send(ServerCommand::PointerMotion {
            x: pointer_x + 2.0,
            y: pointer_y + 2.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(
        state.pointer_motion_count, pointer_motion_after_origin_move,
        "moving a cancelled DnD origin family must not release held pointer routing"
    );
    let (move_reply, move_result) = std::sync::mpsc::channel();
    commands
        .send(ServerCommand::MoveWindowToWorkspace {
            window_id: destination_window_id,
            workspace: 1,
            reply: move_reply,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    assert!(move_result.recv().unwrap());
    queue.roundtrip(&mut state).unwrap();
    for _ in 0..2 {
        commands
            .send(ServerCommand::ToggleDefaultSpecialWorkspace)
            .unwrap();
        wait_for_server_commands(&commands);
        queue.roundtrip(&mut state).unwrap();
        assert_eq!(state.data_source_cancelled_count, 1);
        assert_eq!(capture_active_drag_icon_surface(&commands), None);
        assert_eq!(
            state.pointer_motion_count, pointer_motion_after_origin_move,
            "special-workspace transitions must keep ordinary pointer routing suppressed"
        );
    }

    // Cancellation ends the transfer and icon, but the physical grab still
    // owns client pointer routing across workspace changes until both buttons
    // are released.
    state.event_timeline.clear();
    commands
        .send(ServerCommand::ActivateWorkspace { workspace: 2 })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.data_source_cancelled_count, 1);
    assert!(
        !state.event_timeline.iter().any(|event| matches!(
            event,
            TestWaylandEvent::PointerEnter { .. }
                | TestWaylandEvent::PointerMotion
                | TestWaylandEvent::PointerButtonPressed
                | TestWaylandEvent::PointerButtonReleased
        )),
        "workspace departure must not resume ordinary pointer events after DnD cancellation"
    );
    let pointer_motion_before_cancelled_release = state.pointer_motion_count;
    commands
        .send(ServerCommand::PointerMotion {
            x: pointer_x + 2.0,
            y: pointer_y + 2.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(
        state.pointer_motion_count,
        pointer_motion_before_cancelled_release
    );

    // A round trip must not lose the still-held physical ownership either.
    for workspace in [1, 2] {
        state.event_timeline.clear();
        commands
            .send(ServerCommand::ActivateWorkspace { workspace })
            .unwrap();
        wait_for_server_commands(&commands);
        queue.roundtrip(&mut state).unwrap();
        assert_eq!(state.data_source_cancelled_count, 1);
        assert!(
            !state
                .event_timeline
                .iter()
                .any(|event| matches!(event, TestWaylandEvent::PointerEnter { .. })),
            "ordinary pointer focus must remain suppressed during a cancelled DnD gesture"
        );
    }
    assert_eq!(state.data_device_enter_count, dnd_enters_after_cancel);
    assert_eq!(state.data_device_leave_count, dnd_leaves_after_cancel);
    assert_eq!(state.data_device_drop_count, drops_after_cancel);
    assert_eq!(state.data_source_dnd_finished_count, finishes_after_cancel);

    state.event_timeline.clear();
    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: false,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.data_source_cancelled_count, 1);
    assert_eq!(state.data_device_drop_count, drops_after_cancel);
    assert_eq!(capture_active_drag_icon_surface(&commands), None);
    assert!(
        !state
            .event_timeline
            .iter()
            .any(|event| matches!(event, TestWaylandEvent::PointerButtonReleased)),
        "cancelled DnD first release must not be delivered to the client"
    );
    let pointer_motion_before_final_cancelled_release = state.pointer_motion_count;
    commands
        .send(ServerCommand::PointerMotion {
            x: pointer_x + 3.0,
            y: pointer_y + 3.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(
        state.pointer_motion_count, pointer_motion_before_final_cancelled_release,
        "remaining held button must keep ordinary pointer routing suppressed"
    );
    state.event_timeline.clear();
    commands
        .send(ServerCommand::PointerButton {
            button: 0x111,
            pressed: false,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.data_source_cancelled_count, 1);
    assert_eq!(state.data_device_drop_count, drops_after_cancel);
    assert_eq!(state.data_device_enter_count, dnd_enters_after_cancel);
    assert_eq!(state.data_device_leave_count, dnd_leaves_after_cancel);
    assert_eq!(state.data_source_dnd_finished_count, finishes_after_cancel);
    assert!(
        !state
            .event_timeline
            .iter()
            .any(|event| matches!(event, TestWaylandEvent::PointerButtonReleased)),
        "final cancelled DnD release must not be delivered to the client"
    );
    let pointer_motion_after_cancelled_release = state.pointer_motion_count;
    commands
        .send(ServerCommand::PointerMotion {
            x: pointer_x + 4.0,
            y: pointer_y + 4.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(
        state.pointer_motion_count,
        pointer_motion_after_cancelled_release + 1,
        "ordinary pointer routing must resume after the final physical release"
    );

    // A second sourced drag exercises the empty-workspace path. The old
    // destination must receive leave, remain un-targeted, and cancel only on
    // the physical final release.
    commands
        .send(ServerCommand::ActivateWorkspace { workspace: 2 })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let empty_icon = compositor.create_surface(&qh, ());
    let empty_source = manager.create_data_source(&qh, ());
    empty_source.offer("text/plain".to_string());
    empty_source.set_actions(client_wl_data_device_manager::DndAction::Copy);
    commit_test_buffered_surface(&empty_icon, &shm, &qh, 24, 24).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let empty_serial = state
        .pointer_button_serial
        .expect("second drag must use its physical press serial");
    device.start_drag(
        Some(&empty_source),
        &origin,
        Some(&empty_icon),
        empty_serial,
    );
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.data_source_cancelled_count, 1);
    let enters_before_empty = state.data_device_enter_count;
    let leaves_before_empty = state.data_device_leave_count;
    let empty_icon_surface_id = capture_active_drag_icon_surface(&commands)
        .expect("second client icon must be active before switching");

    commands
        .send(ServerCommand::ActivateWorkspace { workspace: 3 })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.data_source_cancelled_count, 1);
    assert_eq!(state.data_device_enter_count, enters_before_empty);
    assert_eq!(state.data_device_leave_count, leaves_before_empty + 1);
    assert_eq!(
        capture_active_drag_icon_surface(&commands),
        Some(empty_icon_surface_id)
    );

    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: false,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.data_source_cancelled_count, 2);
    assert_eq!(state.data_device_drop_count, 1);
    assert_eq!(capture_active_drag_icon_surface(&commands), None);

    commands
        .send(ServerCommand::ActivateWorkspace { workspace: 1 })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let pointer_motion_count_after_drop = state.pointer_motion_count;
    commands
        .send(ServerCommand::PointerMotion {
            x: pointer_x + 1.0,
            y: pointer_y + 1.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(
        state.pointer_motion_count,
        pointer_motion_count_after_drop + 1,
        "last release must return input to ordinary pointer routing"
    );

    // Destroying the active source is a terminal invalidation. It must not
    // turn the later physical release into a drop or leave the icon/grab live.
    let destroyed_icon = compositor.create_surface(&qh, ());
    let destroyed_source = manager.create_data_source(&qh, ());
    destroyed_source.offer("text/plain".to_string());
    destroyed_source.set_actions(client_wl_data_device_manager::DndAction::Copy);
    commit_test_buffered_surface(&destroyed_icon, &shm, &qh, 24, 24).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let destroyed_serial = state
        .pointer_button_serial
        .expect("destroyed-source drag must begin from a physical press");
    device.start_drag(
        Some(&destroyed_source),
        &destination,
        Some(&destroyed_icon),
        destroyed_serial,
    );
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let drops_before_source_destroy = state.data_device_drop_count;
    let cancels_before_source_destroy = state.data_source_cancelled_count;
    destroyed_source.destroy();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_active_drag_icon_surface(&commands), None);
    assert_eq!(state.data_device_drop_count, drops_before_source_destroy);
    assert_eq!(
        state.data_source_cancelled_count,
        cancels_before_source_destroy
    );
    let enters_after_source_destroy = state.data_device_enter_count;
    let leaves_after_source_destroy = state.data_device_leave_count;
    let finishes_after_source_destroy = state.data_source_dnd_finished_count;

    // Source destruction cancels the transfer but keeps the valid origin
    // surface's physical pointer grab until release, including across a
    // workspace round trip.
    for workspace in [2, 1, 2] {
        state.event_timeline.clear();
        commands
            .send(ServerCommand::ActivateWorkspace { workspace })
            .unwrap();
        wait_for_server_commands(&commands);
        queue.roundtrip(&mut state).unwrap();
        assert_eq!(state.data_device_drop_count, drops_before_source_destroy);
        assert_eq!(
            state.data_source_cancelled_count,
            cancels_before_source_destroy
        );
        assert_eq!(state.data_device_enter_count, enters_after_source_destroy);
        assert_eq!(state.data_device_leave_count, leaves_after_source_destroy);
        assert_eq!(
            state.data_source_dnd_finished_count,
            finishes_after_source_destroy
        );
        assert_eq!(capture_active_drag_icon_surface(&commands), None);
        assert!(
            !state.event_timeline.iter().any(|event| matches!(
                event,
                TestWaylandEvent::PointerEnter { .. }
                    | TestWaylandEvent::PointerMotion
                    | TestWaylandEvent::PointerButtonPressed
                    | TestWaylandEvent::PointerButtonReleased
            )),
            "destroying the data source must not release pointer routing during workspace changes"
        );
    }
    let pointer_motion_before_destroyed_release = state.pointer_motion_count;
    commands
        .send(ServerCommand::PointerMotion {
            x: pointer_x + 3.0,
            y: pointer_y + 3.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(
        state.pointer_motion_count, pointer_motion_before_destroyed_release,
        "destroyed-source DnD retains physical routing only until release"
    );
    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: false,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.data_device_drop_count, drops_before_source_destroy);
    assert_eq!(
        state.data_source_cancelled_count,
        cancels_before_source_destroy
    );
    assert_eq!(capture_active_drag_icon_surface(&commands), None);
    let pointer_motion_after_destroyed_release = state.pointer_motion_count;
    commands
        .send(ServerCommand::PointerMotion {
            x: pointer_x + 5.0,
            y: pointer_y + 5.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(
        state.pointer_motion_count,
        pointer_motion_after_destroyed_release + 1,
        "ordinary pointer motion resumes after source-destroyed DnD releases"
    );

    // Negative control: an ordinary client implicit grab still ends when its
    // root leaves the workspace, and subsequent clicks work normally.
    commands
        .send(ServerCommand::ActivateWorkspace { workspace: 1 })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    state.event_timeline.clear();
    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert!(
        state
            .event_timeline
            .iter()
            .any(|event| matches!(event, TestWaylandEvent::PointerButtonPressed))
    );
    commands
        .send(ServerCommand::ActivateWorkspace { workspace: 2 })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    state.event_timeline.clear();
    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: false,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert!(
        !state
            .event_timeline
            .iter()
            .any(|event| matches!(event, TestWaylandEvent::PointerButtonReleased)),
        "workspace departure must retire an ordinary implicit grab"
    );
    commands
        .send(ServerCommand::ActivateWorkspace { workspace: 1 })
        .unwrap();
    wait_for_server_commands(&commands);
    commands
        .send(ServerCommand::PointerMotion {
            x: pointer_x + 4.0,
            y: pointer_y + 4.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    state.event_timeline.clear();
    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: false,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert!(
        state
            .event_timeline
            .iter()
            .any(|event| matches!(event, TestWaylandEvent::PointerButtonPressed))
    );
    assert!(
        state
            .event_timeline
            .iter()
            .any(|event| matches!(event, TestWaylandEvent::PointerButtonReleased))
    );

    // Destroying the initiating surface invalidates the physical grab. Unlike
    // source destruction, this must not preserve a dangling WaylandDnd route.
    let destroyed_origin_icon = compositor.create_surface(&qh, ());
    let destroyed_origin_source = manager.create_data_source(&qh, ());
    destroyed_origin_source.offer("text/plain".to_string());
    destroyed_origin_source.set_actions(client_wl_data_device_manager::DndAction::Copy);
    commit_test_buffered_surface(&destroyed_origin_icon, &shm, &qh, 24, 24).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let cancels_before_origin_destroy = state.data_source_cancelled_count;
    let drops_before_origin_destroy = state.data_device_drop_count;
    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let destroyed_origin_serial = state
        .pointer_button_serial
        .expect("origin-destruction drag must begin with a real pointer serial");
    device.start_drag(
        Some(&destroyed_origin_source),
        &destination,
        Some(&destroyed_origin_icon),
        destroyed_origin_serial,
    );
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert!(capture_active_drag_icon_surface(&commands).is_some());

    _destination_toplevel.destroy();
    destination_xdg.destroy();
    destination.destroy();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(
        state.data_source_cancelled_count,
        cancels_before_origin_destroy + 1
    );
    assert_eq!(capture_active_drag_icon_surface(&commands), None);

    commands
        .send(ServerCommand::ActivateWorkspace { workspace: 2 })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let pointer_motion_after_origin_destroy = state.pointer_motion_count;
    commands
        .send(ServerCommand::PointerMotion {
            x: pointer_x + 6.0,
            y: pointer_y + 6.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(
        state.pointer_motion_count,
        pointer_motion_after_origin_destroy + 1,
        "a destroyed initiating surface must not retain a stale DnD pointer route"
    );
    assert_eq!(state.data_device_drop_count, drops_before_origin_destroy);
    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: false,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.data_device_drop_count, drops_before_origin_destroy);

    let _server = stop_controllable_test_server(commands, server_thread);
}
