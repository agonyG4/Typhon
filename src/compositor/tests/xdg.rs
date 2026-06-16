use super::*;

#[test]
fn wayland_client_can_create_xdg_toplevel_on_oblivion_server() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let result = create_client_toplevel(&socket_path);
    let server = stop_test_server(running, server_thread);

    result.unwrap();
    assert_eq!(server.state.xdg_toplevels, 1);
    assert_eq!(server.state.last_app_id.as_deref(), Some("oblivion.test"));
}

#[test]
fn shell_dock_items_track_open_toplevel_app_ids() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    create_client_toplevel_with_app_id_and_sized_shm_buffer(&socket_path, "kitty", 300, 200)
        .unwrap();
    let server = stop_test_server(running, server_thread);

    let dock_items = server.shell_dock_items();
    assert_eq!(dock_items.len(), 1);
    assert_eq!(dock_items[0].label, "kitty");
    assert!(dock_items[0].active);
}

#[test]
fn wayland_client_receives_xdg_toplevel_and_surface_configure() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let state = create_configured_client_toplevel(&socket_path);
    stop_test_server(running, server_thread);

    let state = state.unwrap();
    assert!(state.toplevel_configured);
    assert!(state.surface_configured);
}

#[test]
fn xdg_toplevel_configure_waits_for_initial_empty_commit() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let state = create_toplevel_and_check_initial_commit_configure_order(&socket_path);
    stop_test_server(running, server_thread);

    let state = state.unwrap();
    assert!(!state.configured_before_initial_commit);
    assert!(state.configured_after_initial_commit);
}

#[test]
fn recreated_xdg_role_on_same_wl_surface_receives_initial_configure() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let (state, _surface_id, snapshot) =
        recreate_toplevel_role_on_same_surface(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(state.toplevel_configured);
    assert!(state.surface_configured);
    assert_eq!(state.toplevel_configure_count, 1);
    assert_eq!(state.surface_configure_count, 2);
    assert_eq!(state.surface_configure_serials.len(), 2);
    assert_ne!(
        state.surface_configure_serials[0],
        state.surface_configure_serials[1]
    );
    assert!(snapshot.surface_registered);
    assert!(snapshot.configured);
    assert_eq!(snapshot.toplevel_count, 1);
    assert!(snapshot.toplevel_registered);
    assert_eq!(snapshot.popup_count, 0);
    assert!(!snapshot.window_geometry_present);
    assert_eq!(snapshot.placement, None);
}

#[test]
fn wayland_client_xdg_popup_is_configured_and_rendered_as_child_surface() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let state = create_client_toplevel_with_configured_popup(&socket_path);
    let server = stop_test_server(running, server_thread);

    let state = state.unwrap();
    assert!(state.popup_configured);
    assert!(state.surface_configured);
    assert_eq!((state.popup_x, state.popup_y), (43, 34));
    assert_eq!((state.popup_width, state.popup_height), (60, 40));
    assert_eq!(server.renderable_surfaces().len(), 2);
    assert_eq!(server.state.xdg_popups, 1);
    let popup = server
        .renderable_surfaces()
        .iter()
        .find(|surface| surface.placement.parent_surface_id.is_some())
        .expect("popup should be rendered as child surface");
    assert_eq!(popup.placement.local_x, 43);
    assert_eq!(popup.placement.local_y, 34);
    assert_eq!(popup.width, 60);
    assert_eq!(popup.height, 40);
}

#[test]
fn xdg_popup_configure_waits_for_initial_empty_commit() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let state = create_popup_and_check_initial_commit_configure_order(&socket_path);
    stop_test_server(running, server_thread);

    let state = state.unwrap();
    assert!(!state.configured_before_initial_commit);
    assert!(state.configured_after_initial_commit);
}

#[test]
fn wayland_client_xdg_popup_constraint_adjustment_slides_inside_parent() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let state = create_client_popup_with_constrained_positioner(&socket_path);
    let server = stop_test_server(running, server_thread);

    let state = state.unwrap();
    assert!(state.popup_configured);
    assert_eq!((state.popup_x, state.popup_y), (40, 40));
    assert_eq!((state.popup_width, state.popup_height), (80, 50));
    let popup = server
        .renderable_surfaces()
        .iter()
        .find(|surface| surface.placement.parent_surface_id.is_some())
        .expect("popup should be rendered as child surface");
    assert_eq!(popup.placement.local_x, 40);
    assert_eq!(popup.placement.local_y, 40);
}

#[test]
fn wayland_client_xdg_popup_reposition_sends_repositioned_and_reconfigures() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let state = create_client_popup_then_reposition(&socket_path);
    let server = stop_test_server(running, server_thread);

    let state = state.unwrap();
    assert_eq!(state.popup_repositioned_token, Some(77));
    assert!(state.popup_configure_count >= 2);
    assert_eq!((state.popup_x, state.popup_y), (6, 8));
    assert_eq!((state.popup_width, state.popup_height), (50, 30));
    let popup = server
        .renderable_surfaces()
        .iter()
        .find(|surface| surface.placement.parent_surface_id.is_some())
        .expect("popup should be rendered as child surface");
    assert_eq!(popup.placement.local_x, 6);
    assert_eq!(popup.placement.local_y, 8);
}

#[test]
fn wayland_client_xdg_popup_uses_parent_and_popup_window_geometry_for_placement() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let state = create_client_popup_with_window_geometry(&socket_path);
    let server = stop_test_server(running, server_thread);

    let state = state.unwrap();
    assert!(state.popup_configured);
    assert_eq!((state.popup_x, state.popup_y), (10, 20));
    assert_eq!((state.popup_width, state.popup_height), (40, 30));
    let popup = server
        .renderable_surfaces()
        .iter()
        .find(|surface| surface.placement.parent_surface_id.is_some())
        .expect("popup should be rendered as child surface");
    assert_eq!(popup.placement.local_x, 16);
    assert_eq!(popup.placement.local_y, 26);
}

#[test]
fn xdg_popup_set_window_geometry_does_not_reconfigure_non_reactive_popup() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let state = create_non_reactive_popup_then_set_window_geometry(&socket_path);
    let server = stop_test_server(running, server_thread);

    let state = state.unwrap();
    assert_eq!(state.popup_configure_count, 0);
    let popup = server
        .renderable_surfaces()
        .iter()
        .find(|surface| surface.placement.parent_surface_id.is_some())
        .expect("popup should stay renderable after large content commit");
    assert_eq!(popup.width, 177);
    assert_eq!(popup.height, 493);
}

#[test]
fn xdg_popup_grab_retargets_button_release_to_popup_under_cursor() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let (state, popup_surface_id) =
        create_grabbed_popup_then_release_under_cursor(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(state.pointer_button_surface_id, Some(popup_surface_id));
}

#[test]
fn xdg_popup_grab_moves_pointer_focus_to_popup_under_cursor_on_commit() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let (state, popup_surface_id) =
        create_grabbed_popup_under_cursor(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(state.pointer_enter_surface_id, Some(popup_surface_id));
}

#[test]
fn xdg_popup_grab_sends_popup_done_on_outside_click() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_grabbed_popup_then_click_outside(&socket_path, &commands).unwrap();
    let server = stop_controllable_test_server(commands, server_thread);

    assert!(state.popup_done);
    assert!(
        server
            .renderable_surfaces()
            .iter()
            .all(|surface| surface.placement.parent_surface_id.is_none())
    );
}

#[test]
fn xdg_popup_grab_suppresses_pointer_axis_outside_popup() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_grabbed_popup_then_axis_outside(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(!state.pointer_axis);
    assert_eq!(state.pointer_vertical_axis, None);
}
