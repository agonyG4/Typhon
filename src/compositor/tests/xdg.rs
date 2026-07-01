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
fn xdg_activation_token_focuses_requested_toplevel_once() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let activation: client_xdg_activation_v1::XdgActivationV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();

    let target_surface = compositor.create_surface(&qh, ());
    let target_xdg = wm_base.get_xdg_surface(&target_surface, &qh, ());
    let _target_toplevel = target_xdg.get_toplevel(&qh, ());
    target_surface.commit();

    let focused_surface = compositor.create_surface(&qh, ());
    let focused_xdg = wm_base.get_xdg_surface(&focused_surface, &qh, ());
    let _focused_toplevel = focused_xdg.get_toplevel(&qh, ());
    focused_surface.commit();
    connection.flush().unwrap();

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);
    let initial_focus = capture_focused_surface_id(&commands).expect("second toplevel focused");

    let activation_token = activation.get_activation_token(&qh, ());
    activation_token.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let token = state
        .activation_token_done
        .take()
        .expect("activation token should be committed");

    activation.activate(token.clone(), &target_surface);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let activated_focus = capture_focused_surface_id(&commands).expect("target toplevel focused");
    assert_ne!(activated_focus, initial_focus);

    activation.activate(token, &focused_surface);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    assert_eq!(capture_focused_surface_id(&commands), Some(activated_focus));

    let _server = stop_controllable_test_server(commands, server_thread);
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

#[test]
fn xdg_parent_unmap_dismisses_popup_and_late_destroy_is_idempotent() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let mut state = RegistryTestState::default();

    let (parent_surface, parent_xdg_surface, _parent_toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 120, 90).unwrap();
    parent_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let popup_surface = compositor.create_surface(&qh, ());
    let popup_surface_id = popup_surface.id().protocol_id();
    let popup_xdg_surface = wm_base.get_xdg_surface(&popup_surface, &qh, ());
    let positioner = wm_base.create_positioner(&qh, ());
    positioner.set_size(60, 40);
    positioner.set_anchor_rect(10, 10, 1, 1);
    let popup = popup_xdg_surface.get_popup(Some(&parent_xdg_surface), &positioner, &qh, ());
    popup_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&popup_surface, &shm, &qh, 60, 40).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_renderable_surface_count(&commands), 2);

    parent_surface.attach(None, 0, 0);
    parent_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let snapshot = capture_xdg_role_snapshot(&commands, popup_surface_id);
    assert_eq!(snapshot.popup_count, 1);
    assert_eq!(snapshot.popup_node_count, 1);
    assert_eq!(capture_renderable_surface_count(&commands), 0);

    popup.destroy();
    popup_surface.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let snapshot = capture_xdg_role_snapshot(&commands, popup_surface_id);
    assert_eq!(snapshot.popup_count, 0);
    assert_eq!(snapshot.popup_node_count, 0);
    assert!(!snapshot.popup_grab_active);

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn rapid_xdg_popup_cycles_leave_no_stale_popup_state() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let mut state = RegistryTestState::default();

    let (parent_surface, parent_xdg_surface, _parent_toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 120, 90).unwrap();
    parent_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    for index in 0..1000 {
        let popup_surface = compositor.create_surface(&qh, ());
        let popup_surface_id = popup_surface.id().protocol_id();
        let popup_xdg_surface = wm_base.get_xdg_surface(&popup_surface, &qh, ());
        let positioner = wm_base.create_positioner(&qh, ());
        positioner.set_size(60, 40);
        positioner.set_anchor_rect(10, 10, 1, 1);
        let popup = popup_xdg_surface.get_popup(Some(&parent_xdg_surface), &positioner, &qh, ());
        popup_surface.commit();
        connection.flush().unwrap();
        queue.roundtrip(&mut state).unwrap();
        commit_test_buffered_surface(&popup_surface, &shm, &qh, 60, 40).unwrap();
        connection.flush().unwrap();
        queue.roundtrip(&mut state).unwrap();

        if index % 100 == 0 {
            let child_surface = compositor.create_surface(&qh, ());
            let child_xdg_surface = wm_base.get_xdg_surface(&child_surface, &qh, ());
            let child_positioner = wm_base.create_positioner(&qh, ());
            child_positioner.set_size(24, 24);
            child_positioner.set_anchor_rect(2, 2, 1, 1);
            let child_popup =
                child_xdg_surface.get_popup(Some(&popup_xdg_surface), &child_positioner, &qh, ());
            child_surface.commit();
            connection.flush().unwrap();
            queue.roundtrip(&mut state).unwrap();
            commit_test_buffered_surface(&child_surface, &shm, &qh, 24, 24).unwrap();
            connection.flush().unwrap();
            queue.roundtrip(&mut state).unwrap();
            child_popup.destroy();
            child_surface.destroy();
        }

        if index % 200 == 199 {
            parent_surface.attach(None, 0, 0);
            parent_surface.commit();
            connection.flush().unwrap();
            queue.roundtrip(&mut state).unwrap();
        }

        popup.destroy();
        popup_surface.destroy();
        connection.flush().unwrap();
        queue.roundtrip(&mut state).unwrap();

        if index % 200 == 199 {
            commit_test_buffered_surface(&parent_surface, &shm, &qh, 120, 90).unwrap();
            connection.flush().unwrap();
            queue.roundtrip(&mut state).unwrap();
        }

        if index % 100 == 99 {
            let snapshot = capture_xdg_role_snapshot(&commands, popup_surface_id);
            assert_eq!(snapshot.popup_count, 0);
            assert_eq!(snapshot.popup_node_count, 0);
            assert!(!snapshot.popup_grab_active);
        }
    }

    let snapshot = capture_xdg_role_snapshot(&commands, 0);
    assert_eq!(snapshot.popup_count, 0);
    assert_eq!(snapshot.popup_node_count, 0);
    assert!(!snapshot.popup_grab_active);
    assert_eq!(capture_renderable_surface_count(&commands), 1);

    let _server = stop_controllable_test_server(commands, server_thread);
}
