use super::*;
#[test]
fn idle_inhibit_capability_registers_protocol_and_tracks_inhibitor() {
    let socket_name = unique_socket_name();
    let capabilities = InputProtocolCapabilities {
        idle_inhibit: true,
        ..InputProtocolCapabilities::desktop_baseline()
    };
    let server =
        OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let inhibited =
        create_idle_inhibitor_for_surface_and_capture_state(&socket_path, &commands).unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert!(inhibited);
}

#[test]
fn wayland_client_surface_commit_sends_output_enter() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let state = create_client_surface_and_wait_for_enter(&socket_path);
    stop_test_server(running, server_thread);

    let state = state.unwrap();
    assert!(state.surface_enter_count >= 1);
}

#[test]
fn wayland_surface_offset_request_updates_rendered_surface_offset() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    create_client_surface_with_buffer_offset(&socket_path).unwrap();
    let server = stop_test_server(running, server_thread);

    let surface = server
        .renderable_surfaces()
        .iter()
        .find(|surface| surface.width == 40 && surface.height == 30)
        .expect("surface should be rendered");
    assert_eq!((surface.x, surface.y), (5, 7));
}

#[test]
fn wayland_client_receives_output_and_seat_capabilities() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let state = bind_output_and_seat(&socket_path);
    stop_test_server(running, server_thread);

    let state = state.unwrap();
    assert!(state.output_done);
    assert!(state.seat_has_pointer);
    assert!(state.seat_has_keyboard);
}

#[test]
fn wl_output_v1_bind_receives_only_v1_events() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let state = bind_output_at_version(&socket_path, 1);
    stop_test_server(running, server_thread);

    let state = state.unwrap();
    assert_eq!(state.output_mode_count, 1);
    assert!(!state.output_done);
    assert_eq!(state.output_scale_count, 0);
    assert!(!state.output_name);
    assert!(!state.output_description);
}

#[test]
fn wl_seat_v1_bind_does_not_receive_name() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let state = bind_seat_at_version(&socket_path, 1);
    stop_test_server(running, server_thread);

    let state = state.unwrap();
    assert!(state.seat_has_pointer);
    assert!(state.seat_has_keyboard);
    assert!(!state.seat_name);
}

#[test]
fn wayland_client_receives_updated_output_mode_after_resize() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = bind_output_then_set_output_size(&socket_path, &commands, 1600, 900);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert!(state.output_done);
    assert!(state.output_mode_count >= 2);
    assert_eq!(state.output_width, 1600);
    assert_eq!(state.output_height, 900);
}

#[test]
fn wayland_client_receives_selected_output_refresh() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = bind_output_then_set_output_refresh(&socket_path, &commands, 165);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert!(state.output_mode_count >= 2);
    assert_eq!(state.output_refresh_millihertz, 165_000);
}

#[test]
fn output_resize_preserves_selected_refresh() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state =
        bind_output_then_set_output_refresh_and_size(&socket_path, &commands, 165, 1600, 900);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert!(state.output_mode_count >= 3);
    assert_eq!(state.output_width, 1600);
    assert_eq!(state.output_height, 900);
    assert_eq!(state.output_refresh_millihertz, 165_000);
}

#[test]
fn wayland_client_receives_fractional_scale_updates_after_output_scale_change() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_fractional_scale_surface_then_set_output_scale(&socket_path, &commands, 1.5);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert_eq!(state.fractional_preferred_scales, vec![120, 180]);
}

#[test]
fn duplicate_fractional_scale_surface_does_not_disconnect_client() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let state = create_duplicate_fractional_scale_surface(&socket_path);
    stop_test_server(running, server_thread);

    let state = state.unwrap();
    assert_eq!(state.fractional_preferred_scales, vec![120, 120]);
}

#[test]
fn wayland_client_receives_keyboard_keymap_when_requested() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let state = request_keyboard_from_seat(&socket_path);
    stop_test_server(running, server_thread);

    let state = state.unwrap();
    assert!(state.keyboard_keymap);
    assert!(state.keyboard_repeat_info);
}

#[test]
fn wl_keyboard_v1_bind_does_not_receive_repeat_info() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let state = request_keyboard_from_seat_at_version(&socket_path, 1);
    stop_test_server(running, server_thread);

    let state = state.unwrap();
    assert!(state.keyboard_keymap);
    assert!(!state.keyboard_repeat_info);
}

#[test]
fn wayland_client_receives_keyboard_key_from_native_input_bridge() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_focused_toplevel_and_receive_key(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert!(state.keyboard_keymap);
    assert!(state.keyboard_key);
}

#[test]
fn mapped_toplevel_receives_keyboard_focus_before_first_key() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_focused_toplevel_without_keypress(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert_eq!(state.keyboard_enter_count, 1);
    assert!(!state.keyboard_key);
    assert_eq!(
        state
            .keyboard_event_log
            .iter()
            .filter(|event| **event == "keyboard_modifiers")
            .count(),
        1
    );
}

#[test]
fn tab_after_initial_focus_is_only_a_key_event() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_focused_toplevel_then_press_tab(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert_eq!(state.keyboard_enter_count, 1);
    assert_eq!(state.keyboard_keys, vec![15]);
}

#[test]
fn wayland_client_receives_control_modifier_before_modified_key() {
    const CONTROL_MODIFIER_MASK: u32 = 1 << 2;

    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_focused_toplevel_and_receive_ctrl_modified_key(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert_eq!(state.keyboard_keys, vec![29, 30]);
    assert!(
        state
            .keyboard_mods_depressed
            .contains(&CONTROL_MODIFIER_MASK)
    );
}

#[test]
fn repeated_keyboard_input_on_same_surface_does_not_resend_enter() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_focused_toplevel_and_receive_two_keys(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert_eq!(state.keyboard_enter_count, 1);
    assert_eq!(state.keyboard_leave_count, 0);
}

#[test]
fn keyboard_input_ignores_resources_from_other_wayland_clients() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state =
        create_decoy_keyboard_then_focused_toplevel_and_receive_key(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert!(state.keyboard_keymap);
    assert!(state.keyboard_key);
}

#[test]
fn wayland_client_receives_pointer_motion_from_native_input_bridge() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_focused_toplevel_and_receive_pointer_motion(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert!(state.pointer_enter);
    assert!(state.pointer_motion);
}

#[test]
fn wayland_client_receives_pointer_axis_from_native_input_bridge() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_focused_toplevel_and_receive_pointer_axis(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert!(state.pointer_enter);
    assert_eq!(state.pointer_vertical_axis, Some(15.0));
    assert_eq!(state.pointer_enter_frame_count, 1);
    assert_eq!(state.pointer_frame_count, 3);
}

#[test]
fn pointer_click_on_same_surface_does_not_resend_enter_before_leave() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state =
        create_toplevel_then_click_and_move_pointer_on_same_surface(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert_eq!(state.pointer_enter_count, 1);
    assert_eq!(state.pointer_leave_count, 0);
}

#[test]
fn pointer_cursor_surface_commit_is_not_rendered_as_client_content() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let result = create_toplevel_then_set_and_commit_cursor_surface(
        &socket_path,
        &commands,
        false,
        false,
        None,
    )
    .unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(result.renderable_count, 1);
}

#[test]
fn valid_cursor_surface_commit_exposes_hotspot_adjusted_overlay_snapshot() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let result = create_toplevel_then_set_and_commit_cursor_surface(
        &socket_path,
        &commands,
        false,
        false,
        None,
    );
    let _server = stop_controllable_test_server(commands, server_thread);

    let cursor = result
        .unwrap()
        .cursor
        .expect("valid committed cursor should have an overlay snapshot");
    assert_eq!(cursor.logical_x, render::FIRST_SURFACE_OFFSET.0 + 19);
    assert_eq!(cursor.logical_y, render::FIRST_SURFACE_OFFSET.1 + 13);
    assert_eq!((cursor.width, cursor.height), (24, 24));
}

#[test]
fn cursor_surface_null_attachment_removes_overlay_without_mapping_client_content() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let result = create_toplevel_then_set_and_commit_cursor_surface(
        &socket_path,
        &commands,
        true,
        false,
        None,
    );
    let _server = stop_controllable_test_server(commands, server_thread);

    let result = result.unwrap();
    assert_eq!(result.renderable_count, 1);
    assert_eq!(result.cursor, None);
}

#[test]
fn visible_cursor_surface_frame_callback_waits_for_frame_completion() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let result = create_toplevel_then_set_and_commit_cursor_surface(
        &socket_path,
        &commands,
        false,
        true,
        None,
    );
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(result.unwrap().callback_state, Some((true, false)));
}

#[test]
fn active_client_cursor_motion_advances_overlay_generation_without_mapping_surface() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let x = f64::from(render::FIRST_SURFACE_OFFSET.0) + 40.0;
    let y = f64::from(render::FIRST_SURFACE_OFFSET.1) + 30.0;

    let result = create_toplevel_then_set_and_commit_cursor_surface(
        &socket_path,
        &commands,
        false,
        false,
        Some((x, y)),
    );
    let _server = stop_controllable_test_server(commands, server_thread);

    let result = result.unwrap();
    assert_eq!(result.renderable_count, 1);
    assert_eq!(result.cause, RenderGenerationCause::CursorMotion);
    assert_eq!(result.cursor.unwrap().logical_x, x.round() as i32 - 1);
}

#[test]
fn client_cursor_hotspot_hide_reselect_and_destroy_transitions_are_isolated() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let snapshots = exercise_client_cursor_state_transitions(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    let initial = snapshots.initial.unwrap();
    let hotspot_changed = snapshots.hotspot_changed.unwrap();
    assert_eq!(hotspot_changed.surface_id, initial.surface_id);
    assert_eq!(hotspot_changed.logical_x, initial.logical_x - 4);
    assert_eq!(hotspot_changed.logical_y, initial.logical_y - 6);
    assert_eq!(snapshots.hidden, None);
    assert_eq!(snapshots.reselected.unwrap().surface_id, initial.surface_id);
    assert_eq!(snapshots.destroyed, None);
}

#[test]
fn compositor_only_pointer_motion_updates_client_cursor_position_and_generation() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let x = f64::from(render::FIRST_SURFACE_OFFSET.0) + 55.0;
    let y = f64::from(render::FIRST_SURFACE_OFFSET.1) + 41.0;

    let snapshot =
        create_client_cursor_then_update_position_without_dispatch(&socket_path, &commands, x, y)
            .unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    let cursor = snapshot.cursor.unwrap();
    assert_eq!(cursor.logical_x, x.round() as i32 - 3);
    assert_eq!(cursor.logical_y, y.round() as i32 - 4);
    assert!(snapshot.visual_changed);
    assert!(snapshot.render_generation_after > snapshot.render_generation_before);
    assert_eq!(snapshot.cause, RenderGenerationCause::CursorMotion);
    assert_eq!(
        snapshot.scene_generation_after,
        snapshot.scene_generation_before
    );
}

#[test]
fn compositor_only_pointer_motion_preserves_client_dispatch_state() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let snapshot = create_client_cursor_then_update_position_without_dispatch(
        &socket_path,
        &commands,
        f64::from(render::FIRST_SURFACE_OFFSET.0) + 60.0,
        f64::from(render::FIRST_SURFACE_OFFSET.1) + 44.0,
    )
    .unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(
        snapshot.pointer_event_log_after,
        snapshot.pointer_event_log_before
    );
    assert_eq!(
        snapshot.relative_motion_count_after,
        snapshot.relative_motion_count_before
    );
    assert_eq!(
        snapshot.pointer_focus_surface_after,
        snapshot.pointer_focus_surface_before
    );
}

#[test]
fn compositor_only_interaction_motion_prevents_post_grab_cursor_teleport() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let x = f64::from(render::FIRST_SURFACE_OFFSET.0) + 60.0;
    let y = f64::from(render::FIRST_SURFACE_OFFSET.1) + 44.0;

    let snapshots =
        create_client_cursor_then_synchronize_compositor_only_motion_and_send_normal_sample(
            &socket_path,
            &commands,
            x,
            y,
        )
        .unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    let initial_cursor = snapshots.initial.cursor.unwrap();
    let compositor_only_cursor = snapshots.compositor_only.cursor.unwrap();
    let normal_motion_cursor = snapshots.normal_motion.cursor.unwrap();
    assert_ne!(compositor_only_cursor, initial_cursor);
    assert_eq!(compositor_only_cursor.logical_x, x.round() as i32 - 3);
    assert_eq!(compositor_only_cursor.logical_y, y.round() as i32 - 4);
    assert!(snapshots.compositor_only.visual_changed);
    assert!(snapshots.compositor_only.render_generation > snapshots.initial.render_generation);
    assert_eq!(
        snapshots.compositor_only.scene_generation,
        snapshots.initial.scene_generation
    );
    assert_eq!(
        snapshots.compositor_only.cause,
        RenderGenerationCause::CursorMotion
    );
    assert_eq!(
        snapshots.compositor_only.pointer_event_log,
        snapshots.initial.pointer_event_log
    );
    assert_eq!(
        snapshots.compositor_only.pointer_motion_count,
        snapshots.initial.pointer_motion_count
    );
    assert_eq!(
        snapshots.compositor_only.relative_motion_count,
        snapshots.initial.relative_motion_count
    );
    assert_eq!(
        snapshots.compositor_only.pointer_focus_surface,
        snapshots.initial.pointer_focus_surface
    );
    assert_eq!(normal_motion_cursor, compositor_only_cursor);
    assert_eq!(
        snapshots.normal_motion.render_generation,
        snapshots.compositor_only.render_generation
    );
}

#[test]
fn compositor_only_pointer_motion_without_client_cursor_does_not_advance_generation() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let render_generation = server.render_generation();
    let scene_generation = server.scene_render_generation();

    let visual_changed = server.update_pointer_position_without_client_dispatch(100.0, 120.0);

    assert!(!visual_changed);
    assert_eq!(server.render_generation(), render_generation);
    assert_eq!(server.scene_render_generation(), scene_generation);
    assert_eq!(
        (server.state.last_pointer_x, server.state.last_pointer_y),
        (100.0, 120.0)
    );
}

#[test]
fn wl_pointer_v1_motion_does_not_receive_frame() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_focused_toplevel_and_receive_pointer_motion_at_seat_version(
        &socket_path,
        &commands,
        1,
    );
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert!(state.pointer_enter);
    assert!(state.pointer_motion);
    assert_eq!(state.pointer_frame_count, 0);
}

#[test]
fn wayland_pointer_motion_uses_surface_local_coordinates() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state =
        create_buffered_toplevel_and_receive_surface_local_pointer_motion(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert_eq!(state.pointer_surface_x, Some(20.0));
    assert_eq!(state.pointer_surface_y, Some(14.0));
}

#[test]
fn pointer_click_skips_subsurface_with_empty_input_region() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state =
        create_toplevel_with_empty_input_subsurface_and_click_overlap(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert_eq!(state.pointer_button_surface_id, state.parent_surface_id);
}

#[test]
fn pointer_click_hits_subsurface_when_input_region_contains_point() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_toplevel_with_custom_input_subsurface_and_click_overlap(
        &socket_path,
        &commands,
        Some((0, 0, 160, 120)),
    );
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert_eq!(state.pointer_button_surface_id, state.child_surface_id);
}

#[test]
fn pointer_release_uses_press_surface_when_surface_appears_under_cursor() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_toplevel_then_map_subsurface_before_button_release(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert_eq!(
        state.pointer_button_surface_ids,
        vec![
            state.parent_surface_id.unwrap(),
            state.parent_surface_id.unwrap()
        ]
    );
}

#[test]
fn subsurface_place_above_changes_pointer_target_after_parent_commit() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_overlapping_subsurfaces_then_place_above_after_parent_commit(
        &socket_path,
        &commands,
    );
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert_eq!(state.pointer_button_surface_id, state.child_surface_id);
    assert!(state.second_child_surface_id.is_some());
    assert!(state.pointer_leave_count >= 1);
    assert!(state.pointer_enter_count >= 2);
}

#[test]
fn subsurface_place_below_parent_excludes_child_from_top_hit() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_subsurface_below_parent_and_click_overlap(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert_eq!(state.pointer_button_surface_id, state.parent_surface_id);
}

#[test]
fn pointer_enter_v5_is_followed_by_frame() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_pointer_enter_with_v5_pointer(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert_eq!(state.pointer_enter_count, 1);
    assert_eq!(state.pointer_enter_frame_count, 1);
    assert_eq!(state.pointer_enter_without_frame_count, 0);
}

#[test]
fn xkb_v1_keymap_is_nul_terminated() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_focused_toplevel_and_receive_key(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert!(state.keyboard_keymap);
    assert_eq!(
        state.keyboard_keymap_bytes.len(),
        state.keyboard_keymap_size as usize
    );
    assert_eq!(state.keyboard_keymap_bytes.last(), Some(&0));
    assert!(
        String::from_utf8_lossy(
            &state.keyboard_keymap_bytes[..state.keyboard_keymap_bytes.len() - 1]
        )
        .contains("xkb_keymap")
    );
}
