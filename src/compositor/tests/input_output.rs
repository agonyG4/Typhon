use super::*;

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
fn wayland_client_receives_keyboard_key_from_nested_input_bridge() {
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
fn wayland_client_receives_pointer_motion_from_nested_input_bridge() {
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
fn wayland_client_receives_pointer_axis_from_nested_input_bridge() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_focused_toplevel_and_receive_pointer_axis(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert!(state.pointer_enter);
    assert_eq!(state.pointer_vertical_axis, Some(15.0));
    assert_eq!(state.pointer_frame_count, 2);
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

    let renderable_count =
        create_toplevel_then_set_and_commit_cursor_surface(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(renderable_count.unwrap(), 1);
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
