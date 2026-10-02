use super::*;
use std::os::fd::AsRawFd;

fn assert_pre_map_xdg_mode_configure(mode: crate::compositor::ToplevelMode) {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());
    let expected_state = match mode {
        crate::compositor::ToplevelMode::Fullscreen => {
            toplevel.set_fullscreen(None);
            client_xdg_toplevel::State::Fullscreen
        }
        crate::compositor::ToplevelMode::Maximized => {
            toplevel.set_maximized();
            client_xdg_toplevel::State::Maximized
        }
        _ => unreachable!("test only covers initial Fullscreen and Maximized requests"),
    };
    surface.commit();
    connection.flush().unwrap();

    let mut client_state = RegistryTestState::default();
    queue.roundtrip(&mut client_state).unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut client_state).unwrap();
    assert!(client_state.toplevel_configured);
    assert!(client_state.toplevel_has_state(expected_state));
    assert!(client_state.toplevel_width > 0);
    assert!(client_state.toplevel_height > 0);

    let server = stop_controllable_test_server(commands, server_thread);
    assert_eq!(server.state.compliance_metrics.protocol_errors_total, 0);
}

#[test]
fn pre_map_set_fullscreen_advertises_state_without_mode_animation() {
    assert_pre_map_xdg_mode_configure(crate::compositor::ToplevelMode::Fullscreen);
}

#[test]
fn pre_map_set_maximized_advertises_state_without_mode_animation() {
    assert_pre_map_xdg_mode_configure(crate::compositor::ToplevelMode::Maximized);
}

#[test]
fn xdg_surface_destroy_with_live_popup_cleans_role_and_leaves_resource_inert() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let parent_surface = compositor.create_surface(&qh, ());
    let parent_xdg_surface = wm_base.get_xdg_surface(&parent_surface, &qh, ());
    let _parent_toplevel = parent_xdg_surface.get_toplevel(&qh, ());

    let popup_surface = compositor.create_surface(&qh, ());
    let popup_surface_id = popup_surface.id().protocol_id();
    let popup_xdg_surface = wm_base.get_xdg_surface(&popup_surface, &qh, ());
    let popup = wm_base.create_positioner(&qh, ());
    popup.set_size(80, 50);
    popup.set_anchor_rect(10, 20, 30, 10);
    let popup_role = popup_xdg_surface.get_popup(Some(&parent_xdg_surface), &popup, &qh, ());
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();

    popup_xdg_surface.destroy();
    popup_role.reposition(&popup, 17);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    connection
        .roundtrip()
        .expect("reposition on a retired popup role must be ignored");
    popup_role.destroy();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    connection
        .roundtrip()
        .expect("late popup destruction must not disconnect the client");

    let server = stop_controllable_test_server(commands, server_thread);
    assert_eq!(server.state.compliance_metrics.protocol_errors_total, 0);
    assert_eq!(
        server
            .state
            .compliance_metrics
            .lifecycle_xdg_surface_destroy_with_role_total,
        1
    );
    assert!(!server.state.popup_surfaces.contains_key(&popup_surface_id));
}

#[test]
fn unmapped_xdg_toplevel_does_not_establish_keyboard_focus_until_first_buffer_commit() {
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
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let _keyboard = seat.get_keyboard(&qh, ());
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush().unwrap();

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);

    assert_eq!(capture_presentation_transition_curve(&commands), None);

    assert_eq!(capture_focused_surface_id(&commands), None);
    assert_eq!(capture_keyboard_focus_surface_id(&commands), None);
    assert_eq!(state.keyboard_enter_count, 0);

    commit_test_buffered_surface(&surface, &shm, &qh, 32, 32).unwrap();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    let root_surface_id = capture_focused_surface_id(&commands).expect("mapped toplevel root");
    let first_open = capture_presentation_transition_start(&commands, root_surface_id)
        .expect("first root map starts Window Open");

    assert!(capture_focused_surface_id(&commands).is_some());
    assert!(capture_keyboard_focus_surface_id(&commands).is_some());
    assert_eq!(state.keyboard_enter_count, 1);

    // Mapping an already-existing subsurface must not restart the root's Open.
    let subcompositor: client_wl_subcompositor::WlSubcompositor =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let child = compositor.create_surface(&qh, ());
    let _subsurface = subcompositor.get_subsurface(&child, &surface, &qh, ());
    child.commit();
    commit_test_buffered_surface(&child, &shm, &qh, 8, 8).unwrap();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(
        capture_presentation_transition_start(&commands, root_surface_id)
            .expect("root Open remains active")
            .transition_id,
        first_open.transition_id
    );

    // Ordinary root buffer commits also leave the original Open transition intact.
    commit_test_buffered_surface(&surface, &shm, &qh, 40, 40).unwrap();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(
        capture_presentation_transition_start(&commands, root_surface_id)
            .expect("root Open remains active after a later buffer commit")
            .transition_id,
        first_open.transition_id
    );

    // XDG unmap cancels the property tracks; the next successful map gets a new Open.
    surface.attach(None, 0, 0);
    surface.commit();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(
        capture_presentation_transition_curve_for_root(&commands, root_surface_id),
        None
    );

    commit_test_buffered_surface(&surface, &shm, &qh, 48, 48).unwrap();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_ne!(
        capture_presentation_transition_start(&commands, root_surface_id)
            .expect("remap starts a fresh Open")
            .transition_id,
        first_open.transition_id
    );

    stop_controllable_test_server(commands, server_thread);
}

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
fn xdg_popup_keyboard_grab_completes_a_normal_popup_transaction() {
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
    let _keyboard = seat.get_keyboard(&qh, ());

    let parent_surface = compositor.create_surface(&qh, ());
    let parent_xdg_surface = wm_base.get_xdg_surface(&parent_surface, &qh, ());
    let _parent_toplevel = parent_xdg_surface.get_toplevel(&qh, ());
    parent_surface.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&parent_surface, &shm, &qh, 120, 90).unwrap();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    commands
        .send(ServerCommand::KeyboardKey {
            key: 30,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let serial = state
        .keyboard_key_serial
        .expect("keyboard press serial was not delivered");

    let popup_surface = compositor.create_surface(&qh, ());
    let popup_xdg_surface = wm_base.get_xdg_surface(&popup_surface, &qh, ());
    let positioner = wm_base.create_positioner(&qh, ());
    positioner.set_size(80, 50);
    positioner.set_anchor_rect(10, 20, 30, 10);
    positioner.set_anchor(client_xdg_positioner::Anchor::BottomRight);
    positioner.set_gravity(client_xdg_positioner::Gravity::BottomRight);
    let popup = popup_xdg_surface.get_popup(Some(&parent_xdg_surface), &positioner, &qh, ());
    popup.grab(&seat, serial);
    commit_test_buffered_surface_after_initial_configure(
        &popup_surface,
        &shm,
        &qh,
        &connection,
        &mut queue,
        &mut state,
        80,
        50,
    )
    .unwrap();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.popup_configure_count, 1);

    stop_controllable_test_server(commands, server_thread);
}

#[test]
fn invalid_activation_serial_still_completes_gtk_toplevel_startup() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let fractional_scale_manager: client_wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let viewporter: client_wp_viewporter::WpViewporter = globals.bind(&qh, 1..=1, ()).unwrap();
    let activation: client_xdg_activation_v1::XdgActivationV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let _fractional_scale = fractional_scale_manager.get_fractional_scale(&surface, &qh, ());
    let _viewport = viewporter.get_viewport(&surface, &qh, ());
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());

    surface.commit();
    let token = activation.get_activation_token(&qh, ());
    token.set_serial(0, &seat);
    token.commit();
    connection.flush().unwrap();

    let mut pollfd = libc::pollfd {
        fd: connection.backend().poll_fd().as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let ready = unsafe { libc::poll(&mut pollfd, 1, 1_000) };
    assert_eq!(
        ready, 1,
        "initial configure was not delivered without wl_display.sync"
    );

    let mut state = RegistryTestState::default();
    queue.blocking_dispatch(&mut state).unwrap();
    let _server = stop_test_server(running, server_thread);

    assert!(state.toplevel_configured);
    assert!(state.surface_configured);
    assert_eq!(state.activation_token_done.as_deref(), Some(""));
}

#[test]
fn xdg_toplevel_v5_receives_capabilities_before_initial_configure() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let state = create_configured_client_toplevel(&socket_path);
    stop_test_server(running, server_thread);

    let state = state.unwrap();
    assert_eq!(state.toplevel_wm_capabilities_count, 1);
    assert_eq!(state.toplevel_wm_capabilities, vec![2, 3]);
    assert_eq!(
        state.toplevel_event_log.first().copied(),
        Some("wm_capabilities")
    );
    assert_eq!(
        state.toplevel_event_log.get(1).copied(),
        Some("toplevel_configure")
    );
    assert_eq!(
        state.toplevel_event_log.get(2).copied(),
        Some("xdg_surface_configure")
    );
}

#[test]
fn xdg_toplevel_v4_does_not_receive_wm_capabilities() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let state = create_configured_client_toplevel_at_version(&socket_path, 4);
    stop_test_server(running, server_thread);

    let state = state.unwrap();
    assert_eq!(state.toplevel_wm_capabilities_count, 0);
    assert_eq!(
        state.toplevel_event_log.first().copied(),
        Some("toplevel_configure")
    );
    assert_eq!(
        state.toplevel_event_log.get(1).copied(),
        Some("xdg_surface_configure")
    );
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
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
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
    commit_test_buffered_surface(&target_surface, &shm, &qh, 32, 32).unwrap();
    commit_test_buffered_surface(&focused_surface, &shm, &qh, 32, 32).unwrap();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
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
fn sober_style_toplevel_reassociation_on_same_wl_surface_is_supported() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base_a: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let surface_id = surface.id().protocol_id();

    let xdg_surface_a = wm_base_a.get_xdg_surface(&surface, &qh, ());
    let toplevel_a = xdg_surface_a.get_toplevel(&qh, ());
    toplevel_a.set_app_id("oblivion.sober-style-old".to_string());
    toplevel_a.set_title("old title".to_string());
    toplevel_a.set_min_size(640, 480);
    xdg_surface_a.set_window_geometry(5, 6, 111, 77);
    surface.commit();
    connection.flush().unwrap();

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&surface, &shm, &qh, 64, 48).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    toplevel_a.destroy();
    xdg_surface_a.destroy();
    surface.attach(None, 0, 0);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let wm_base_b: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let xdg_surface_b = wm_base_b.get_xdg_surface(&surface, &qh, ());
    let toplevel_b = xdg_surface_b.get_toplevel(&qh, ());
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&surface, &shm, &qh, 80, 60).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    expect_roundtrip_alive(&connection);

    let snapshot = capture_xdg_role_snapshot(&commands, surface_id);
    assert!(snapshot.surface_registered);
    assert!(snapshot.configured);
    assert_eq!(snapshot.toplevel_count, 1);
    assert!(snapshot.toplevel_registered);
    assert_eq!(snapshot.popup_count, 0);
    assert!(!snapshot.window_geometry_present);
    assert_eq!(
        snapshot.placement,
        Some(SurfacePlacement::absolute_root_at(
            render::FIRST_SURFACE_OFFSET.0,
            render::FIRST_SURFACE_OFFSET.1,
        ))
    );
    assert_eq!(
        snapshot.permanent_role,
        Some(PermanentSurfaceRole::XdgToplevel)
    );
    assert!(snapshot.xdg_association);
    assert!(!snapshot.toplevel_has_app_id);
    assert!(!snapshot.toplevel_has_title);
    assert!(!snapshot.toplevel_has_non_default_constraints);
    assert_eq!(snapshot.toplevel_mode, Some(ToplevelMode::Normal));
    assert_eq!(state.surface_configure_count, 2);
    assert_eq!(state.toplevel_configure_count, 2);
    assert_eq!(state.surface_configure_serials.len(), 2);
    assert_ne!(
        state.surface_configure_serials[0],
        state.surface_configure_serials[1]
    );

    toplevel_b.destroy();
    xdg_surface_b.destroy();
    drop(wm_base_b);
    drop(wm_base_a);
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let dormant_snapshot = capture_xdg_role_snapshot(&commands, surface_id);
    assert!(dormant_snapshot.surface_registered);
    assert_eq!(dormant_snapshot.toplevel_count, 0);
    assert_eq!(dormant_snapshot.popup_count, 0);
    assert!(!dormant_snapshot.xdg_association);
    assert_eq!(
        dormant_snapshot.permanent_role,
        Some(PermanentSurfaceRole::XdgToplevel)
    );
    drop(surface);
    drop(shm);
    drop(compositor);
    drop(queue);
    drop(globals);
    drop(connection);
    wait_for_server_commands(&commands);
    let server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(server.state.surface_resources.len(), 0);
    assert_eq!(
        server.state.surface_role_lifecycle(surface_id),
        SurfaceRoleLifecycle::default()
    );
    assert_eq!(server.state.xdg_surface_resources.len(), 0);
    assert_eq!(server.state.xdg_surface_lifecycles.len(), 0);
    assert_eq!(server.state.toplevel_surfaces.len(), 0);
    assert_eq!(server.state.renderable_surfaces.len(), 0);
    assert_eq!(server.state.current_surface_buffers.len(), 0);
    assert_eq!(
        server.state.compliance_metrics.client_state_leaks_detected,
        0
    );
    assert_eq!(server.state.compliance_metrics.protocol_errors_total, 0);
    assert_eq!(
        server
            .state
            .compliance_metrics
            .xdg_same_role_reassociations_total,
        1
    );
    assert_eq!(
        server
            .state
            .compliance_metrics
            .xdg_cross_role_reassociation_rejections,
        0
    );
}

#[test]
fn xdg_toplevel_role_destroy_retires_unpublished_explicit_sync_work() {
    let Some(acquire_timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };
    let Some(release_timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };

    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    server.enable_external_acquire_readiness();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base_a: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 =
        globals.bind(&qh, 3..=3, ()).unwrap();
    let syncobj: client_wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let acquire_timeline_fd = acquire_timeline.export_timeline_fd().unwrap();
    let release_timeline_fd = release_timeline.export_timeline_fd().unwrap();
    let surface = compositor.create_surface(&qh, ());
    let sync_surface = syncobj.get_surface(&surface, &qh, ());
    let sync_acquire_timeline = syncobj.import_timeline(acquire_timeline_fd.as_fd(), &qh, ());
    let sync_release_timeline = syncobj.import_timeline(release_timeline_fd.as_fd(), &qh, ());
    let surface_id = surface.id().protocol_id();

    let xdg_surface_a = wm_base_a.get_xdg_surface(&surface, &qh, ());
    let toplevel_a = xdg_surface_a.get_toplevel(&qh, ());
    surface.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();

    let old_buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff44_4444).unwrap();
    acquire_timeline.signal_point(1).unwrap();
    sync_surface.set_acquire_point(&sync_acquire_timeline, 0, 2);
    sync_surface.set_release_point(&sync_release_timeline, 0, 3);
    let callback = surface.frame(&qh, ());
    state.tracked_frame_callback_id = Some(callback.id().protocol_id());
    surface.attach(Some(&old_buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);

    let blocked = capture_xdg_role_snapshot(&commands, surface_id);
    assert_eq!(
        blocked.pending_explicit_sync_commits + blocked.pending_surface_tree_transactions,
        1
    );
    assert_eq!(blocked.pending_surface_tree_transactions, 1);
    assert!(!blocked.current_surface_buffer);
    assert!(!blocked.renderable_surface);

    toplevel_a.destroy();
    xdg_surface_a.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);

    assert!(release_timeline.point_signaled(3).unwrap());
    assert_eq!(
        state.frame_done_callbacks,
        vec![callback.id().protocol_id()]
    );
    let retired = capture_xdg_role_snapshot(&commands, surface_id);
    assert_eq!(retired.pending_explicit_sync_commits, 0);
    assert_eq!(retired.pending_surface_tree_transactions, 0);
    assert!(!retired.current_surface_buffer);
    assert!(!retired.renderable_surface);
    assert_eq!(retired.role_destroyed_pending_commits_retired, 0);
    assert_eq!(retired.role_destroyed_pending_trees_retired, 1);
    assert_eq!(retired.role_destroyed_acquire_watches_cancelled, 1);

    acquire_timeline.signal_point(2).unwrap();
    wait_for_server_commands(&commands);
    let after_old_signal = capture_xdg_role_snapshot(&commands, surface_id);
    assert_eq!(after_old_signal.pending_explicit_sync_commits, 0);
    assert!(!after_old_signal.current_surface_buffer);
    assert!(!after_old_signal.renderable_surface);

    let wm_base_b: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let xdg_surface_b = wm_base_b.get_xdg_surface(&surface, &qh, ());
    let toplevel_b = xdg_surface_b.get_toplevel(&qh, ());
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.surface_configure_count, 2);
    assert_eq!(state.toplevel_configure_count, 2);

    let new_buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff55_5555).unwrap();
    acquire_timeline.signal_point(4).unwrap();
    sync_surface.set_acquire_point(&sync_acquire_timeline, 0, 4);
    sync_surface.set_release_point(&sync_release_timeline, 0, 5);
    surface.attach(Some(&new_buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);
    let reconstructed = capture_xdg_role_snapshot(&commands, surface_id);
    assert_eq!(reconstructed.pending_explicit_sync_commits, 0);
    assert!(reconstructed.current_surface_buffer);
    assert!(reconstructed.renderable_surface);
    assert_eq!(
        reconstructed.permanent_role,
        Some(PermanentSurfaceRole::XdgToplevel)
    );
    assert_eq!(reconstructed.role_destroyed_pending_commits_retired, 0);
    assert_eq!(reconstructed.role_destroyed_pending_trees_retired, 1);
    assert_eq!(reconstructed.reassociation_blocked_stale_work, 0);
    expect_roundtrip_alive(&connection);

    toplevel_b.destroy();
    xdg_surface_b.destroy();
    sync_surface.destroy();
    surface.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    drop(wm_base_b);
    drop(wm_base_a);
    drop(syncobj);
    drop(dmabuf);
    drop(compositor);
    drop(queue);
    drop(globals);
    drop(connection);
    wait_for_server_commands(&commands);
    let server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(server.state.compliance_metrics.protocol_errors_total, 0);
    assert_eq!(
        server.state.compliance_metrics.client_state_leaks_detected,
        0
    );
    assert_eq!(
        server
            .state
            .buffer_release_metrics
            .buffer_release_duplicate_attempts,
        0
    );
}

#[test]
fn disconnected_client_retires_pending_explicit_sync_work() {
    let Some(acquire_timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };
    let Some(release_timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };

    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    server.enable_external_acquire_readiness();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let surface_id;
    {
        let connection =
            Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
        let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
        let qh = queue.handle();
        let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
        let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
        let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 =
            globals.bind(&qh, 3..=3, ()).unwrap();
        let syncobj: client_wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1 =
            globals.bind(&qh, 1..=1, ()).unwrap();
        let acquire_timeline_fd = acquire_timeline.export_timeline_fd().unwrap();
        let release_timeline_fd = release_timeline.export_timeline_fd().unwrap();
        let surface = compositor.create_surface(&qh, ());
        surface_id = surface.id().protocol_id();
        let sync_surface = syncobj.get_surface(&surface, &qh, ());
        let sync_acquire_timeline = syncobj.import_timeline(acquire_timeline_fd.as_fd(), &qh, ());
        let sync_release_timeline = syncobj.import_timeline(release_timeline_fd.as_fd(), &qh, ());
        let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
        let _toplevel = xdg_surface.get_toplevel(&qh, ());
        surface.commit();
        connection.flush().unwrap();
        let mut state = RegistryTestState::default();
        queue.roundtrip(&mut state).unwrap();

        let buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff44_4444).unwrap();
        sync_surface.set_acquire_point(&sync_acquire_timeline, 0, 1);
        sync_surface.set_release_point(&sync_release_timeline, 0, 2);
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, 2, 2);
        surface.commit();
        connection.flush().unwrap();
        queue.roundtrip(&mut state).unwrap();
        wait_for_server_commands(&commands);

        let blocked = capture_xdg_role_snapshot(&commands, surface_id);
        assert_eq!(
            blocked.pending_explicit_sync_commits + blocked.pending_surface_tree_transactions,
            1
        );
        assert_eq!(blocked.pending_surface_tree_transactions, 1);
        assert!(!blocked.renderable_surface);
    }

    thread::sleep(Duration::from_millis(20));
    wait_for_server_commands(&commands);
    let retired = capture_xdg_role_snapshot(&commands, surface_id);
    assert_eq!(retired.pending_explicit_sync_commits, 0);
    assert_eq!(retired.pending_surface_tree_transactions, 0);
    assert!(!retired.surface_registered);
    assert!(!retired.renderable_surface);
    assert!(release_timeline.point_signaled(2).unwrap());

    stop_controllable_test_server(commands, server_thread);
}

#[test]
fn xdg_popup_role_destroy_retires_unpublished_explicit_sync_work() {
    let Some(acquire_timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };
    let Some(release_timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };

    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    server.enable_external_acquire_readiness();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base_a: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base_b: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 =
        globals.bind(&qh, 3..=3, ()).unwrap();
    let syncobj: client_wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let acquire_timeline_fd = acquire_timeline.export_timeline_fd().unwrap();
    let release_timeline_fd = release_timeline.export_timeline_fd().unwrap();
    let sync_acquire_timeline = syncobj.import_timeline(acquire_timeline_fd.as_fd(), &qh, ());
    let sync_release_timeline = syncobj.import_timeline(release_timeline_fd.as_fd(), &qh, ());
    let mut state = RegistryTestState::default();

    let (parent_surface, parent_xdg_surface, parent_toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base_a, &shm, &qh, 120, 90).unwrap();
    parent_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&parent_surface, &shm, &qh, 120, 90).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let parent_surface_id =
        capture_xdg_role_snapshot(&commands, parent_surface.id().protocol_id()).surface_id;

    let popup_surface = compositor.create_surface(&qh, ());
    let popup_surface_id = popup_surface.id().protocol_id();
    let sync_surface = syncobj.get_surface(&popup_surface, &qh, ());
    let popup_xdg_surface_a = wm_base_a.get_xdg_surface(&popup_surface, &qh, ());
    let positioner_a = wm_base_a.create_positioner(&qh, ());
    positioner_a.set_size(60, 40);
    positioner_a.set_anchor_rect(10, 20, 1, 1);
    let popup_a = popup_xdg_surface_a.get_popup(Some(&parent_xdg_surface), &positioner_a, &qh, ());
    popup_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let old_buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff44_4444).unwrap();
    sync_surface.set_acquire_point(&sync_acquire_timeline, 0, 6);
    sync_surface.set_release_point(&sync_release_timeline, 0, 7);
    let callback = popup_surface.frame(&qh, ());
    state.tracked_frame_callback_id = Some(callback.id().protocol_id());
    popup_surface.attach(Some(&old_buffer), 0, 0);
    popup_surface.damage_buffer(0, 0, 2, 2);
    popup_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);
    let blocked = capture_xdg_role_snapshot(&commands, popup_surface_id);
    assert_eq!(
        blocked.pending_explicit_sync_commits + blocked.pending_surface_tree_transactions,
        1
    );
    assert!(!blocked.current_surface_buffer);
    assert!(!blocked.renderable_surface);

    popup_a.destroy();
    popup_xdg_surface_a.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);
    assert!(release_timeline.point_signaled(7).unwrap());
    assert_eq!(
        state.frame_done_callbacks,
        vec![callback.id().protocol_id()]
    );
    let retired = capture_xdg_role_snapshot(&commands, popup_surface_id);
    assert_eq!(retired.pending_explicit_sync_commits, 0);
    assert_eq!(retired.pending_surface_tree_transactions, 0);
    assert!(!retired.current_surface_buffer);
    assert!(!retired.renderable_surface);
    assert_eq!(retired.role_destroyed_pending_trees_retired, 1);
    assert_eq!(retired.role_destroyed_acquire_watches_cancelled, 1);

    acquire_timeline.signal_point(6).unwrap();
    wait_for_server_commands(&commands);
    let after_old_signal = capture_xdg_role_snapshot(&commands, popup_surface_id);
    assert!(!after_old_signal.current_surface_buffer);
    assert!(!after_old_signal.renderable_surface);

    let popup_xdg_surface_b = wm_base_b.get_xdg_surface(&popup_surface, &qh, ());
    let positioner_b = wm_base_b.create_positioner(&qh, ());
    positioner_b.set_size(70, 50);
    positioner_b.set_anchor_rect(20, 30, 1, 1);
    let popup_b = popup_xdg_surface_b.get_popup(Some(&parent_xdg_surface), &positioner_b, &qh, ());
    popup_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.popup_configure_count, 2);

    let new_buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff55_5555).unwrap();
    acquire_timeline.signal_point(8).unwrap();
    sync_surface.set_acquire_point(&sync_acquire_timeline, 0, 8);
    sync_surface.set_release_point(&sync_release_timeline, 0, 9);
    popup_surface.attach(Some(&new_buffer), 0, 0);
    popup_surface.damage_buffer(0, 0, 2, 2);
    popup_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);
    let reconstructed = capture_xdg_role_snapshot(&commands, popup_surface_id);
    assert!(reconstructed.current_surface_buffer);
    assert!(reconstructed.renderable_surface);
    assert_eq!(
        reconstructed.permanent_role,
        Some(PermanentSurfaceRole::XdgPopup)
    );
    assert_eq!(
        reconstructed.popup_parent_surface_id,
        Some(parent_surface_id)
    );
    assert_eq!(reconstructed.reassociation_blocked_stale_work, 0);
    expect_roundtrip_alive(&connection);

    popup_b.destroy();
    popup_xdg_surface_b.destroy();
    parent_toplevel.destroy();
    parent_xdg_surface.destroy();
    sync_surface.destroy();
    popup_surface.destroy();
    parent_surface.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    drop(wm_base_b);
    drop(wm_base_a);
    drop(syncobj);
    drop(dmabuf);
    drop(shm);
    drop(compositor);
    drop(queue);
    drop(globals);
    drop(connection);
    wait_for_server_commands(&commands);
    let server = stop_controllable_test_server(commands, server_thread);
    assert_eq!(server.state.compliance_metrics.protocol_errors_total, 0);
    assert_eq!(
        server.state.compliance_metrics.client_state_leaks_detected,
        0
    );
    assert_eq!(
        server
            .state
            .buffer_release_metrics
            .buffer_release_duplicate_attempts,
        0
    );
}

#[test]
fn duplicate_xdg_association_on_same_wl_surface_is_rejected() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, _queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = _queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());
    toplevel.destroy();

    let healthy_connection =
        Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (healthy_globals, healthy_queue) =
        registry_queue_init::<RegistryTestState>(&healthy_connection).unwrap();
    let healthy_qh = healthy_queue.handle();
    let healthy_compositor: client_wl_compositor::WlCompositor =
        healthy_globals.bind(&healthy_qh, 1..=6, ()).unwrap();
    let _healthy_surface = healthy_compositor.create_surface(&healthy_qh, ());
    healthy_connection.flush().unwrap();
    expect_roundtrip_alive(&healthy_connection);

    let second = wm_base.get_xdg_surface(&surface, &qh, ());
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let observed = expect_protocol_error(
        &connection,
        "xdg_wm_base",
        client_xdg_wm_base::Error::Role as u32,
    );
    assert_eq!(observed.object_id, wm_base.id().protocol_id());
    expect_roundtrip_alive(&healthy_connection);
    drop(second);
    drop(surface);
    drop(compositor);
    drop(globals);
    drop(_queue);
    drop(qh);
    drop(connection);
    drop(healthy_compositor);
    drop(healthy_globals);
    drop(healthy_queue);
    drop(healthy_qh);
    drop(healthy_connection);
    wait_for_server_commands(&commands);
    let server = stop_controllable_test_server(commands, server_thread);
    assert_eq!(server.state.compliance_metrics.protocol_errors_total, 1);
    assert_eq!(
        server.state.compliance_metrics.client_state_leaks_detected,
        0
    );
}

#[test]
fn pending_surface_content_rejects_xdg_association_and_preserves_healthy_client() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection_a = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals_a, _queue_a) = registry_queue_init::<RegistryTestState>(&connection_a).unwrap();
    let qh_a = _queue_a.handle();
    let compositor_a: client_wl_compositor::WlCompositor =
        globals_a.bind(&qh_a, 1..=6, ()).unwrap();
    let wm_base_a: client_xdg_wm_base::XdgWmBase = globals_a.bind(&qh_a, 1..=6, ()).unwrap();
    let shm_a: client_wl_shm::WlShm = globals_a.bind(&qh_a, 1..=1, ()).unwrap();
    let surface_a = compositor_a.create_surface(&qh_a, ());
    let _buffer_a = attach_test_buffered_surface(&surface_a, &shm_a, &qh_a, 2, 2).unwrap();

    let connection_b = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals_b, _queue_b) = registry_queue_init::<RegistryTestState>(&connection_b).unwrap();
    let qh_b = _queue_b.handle();
    let compositor_b: client_wl_compositor::WlCompositor =
        globals_b.bind(&qh_b, 1..=6, ()).unwrap();
    let _surface_b = compositor_b.create_surface(&qh_b, ());
    connection_b.roundtrip().unwrap();

    let _xdg_surface = wm_base_a.get_xdg_surface(&surface_a, &qh_a, ());
    connection_a.flush().unwrap();
    wait_for_server_commands(&commands);
    let observed = expect_protocol_error(
        &connection_a,
        "xdg_wm_base",
        client_xdg_wm_base::Error::InvalidSurfaceState as u32,
    );
    assert_eq!(observed.object_id, wm_base_a.id().protocol_id());
    expect_roundtrip_alive(&connection_b);

    drop(surface_a);
    drop(shm_a);
    drop(compositor_a);
    drop(globals_a);
    drop(_queue_a);
    drop(qh_a);
    drop(connection_a);
    drop(compositor_b);
    drop(globals_b);
    drop(_queue_b);
    drop(qh_b);
    drop(connection_b);
    wait_for_server_commands(&commands);
    let server = stop_controllable_test_server(commands, server_thread);
    assert_eq!(server.state.compliance_metrics.protocol_errors_total, 1);
    assert_eq!(
        server.state.compliance_metrics.client_state_leaks_detected,
        0
    );
    assert!(server.state.surface_resources.is_empty());
}

#[test]
fn committed_surface_content_rejects_xdg_association_and_preserves_healthy_client() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection_a = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals_a, mut queue_a) = registry_queue_init::<RegistryTestState>(&connection_a).unwrap();
    let qh_a = queue_a.handle();
    let compositor_a: client_wl_compositor::WlCompositor =
        globals_a.bind(&qh_a, 1..=6, ()).unwrap();
    let wm_base_a: client_xdg_wm_base::XdgWmBase = globals_a.bind(&qh_a, 1..=6, ()).unwrap();
    let shm_a: client_wl_shm::WlShm = globals_a.bind(&qh_a, 1..=1, ()).unwrap();
    let surface_a = compositor_a.create_surface(&qh_a, ());
    commit_test_buffered_surface(&surface_a, &shm_a, &qh_a, 2, 2).unwrap();
    connection_a.flush().unwrap();
    queue_a
        .roundtrip(&mut RegistryTestState::default())
        .unwrap();

    let connection_b = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals_b, _queue_b) = registry_queue_init::<RegistryTestState>(&connection_b).unwrap();
    let qh_b = _queue_b.handle();
    let compositor_b: client_wl_compositor::WlCompositor =
        globals_b.bind(&qh_b, 1..=6, ()).unwrap();
    let _surface_b = compositor_b.create_surface(&qh_b, ());
    connection_b.roundtrip().unwrap();

    let _xdg_surface = wm_base_a.get_xdg_surface(&surface_a, &qh_a, ());
    connection_a.flush().unwrap();
    wait_for_server_commands(&commands);
    let observed = expect_protocol_error(
        &connection_a,
        "xdg_wm_base",
        client_xdg_wm_base::Error::InvalidSurfaceState as u32,
    );
    assert_eq!(observed.object_id, wm_base_a.id().protocol_id());
    expect_roundtrip_alive(&connection_b);

    drop(surface_a);
    drop(shm_a);
    drop(compositor_a);
    drop(globals_a);
    drop(queue_a);
    drop(qh_a);
    drop(connection_a);
    drop(compositor_b);
    drop(globals_b);
    drop(_queue_b);
    drop(qh_b);
    drop(connection_b);
    wait_for_server_commands(&commands);
    let server = stop_controllable_test_server(commands, server_thread);
    assert_eq!(server.state.compliance_metrics.protocol_errors_total, 1);
    assert_eq!(
        server.state.compliance_metrics.client_state_leaks_detected,
        0
    );
    assert!(server.state.surface_resources.is_empty());
}

#[test]
fn dormant_xdg_toplevel_reassociation_to_popup_is_rejected() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base_a: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface_a = wm_base_a.get_xdg_surface(&surface, &qh, ());
    let toplevel_a = xdg_surface_a.get_toplevel(&qh, ());
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    toplevel_a.destroy();
    xdg_surface_a.destroy();
    surface.attach(None, 0, 0);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();

    let healthy_connection =
        Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (healthy_globals, healthy_queue) =
        registry_queue_init::<RegistryTestState>(&healthy_connection).unwrap();
    let healthy_qh = healthy_queue.handle();
    let healthy_compositor: client_wl_compositor::WlCompositor =
        healthy_globals.bind(&healthy_qh, 1..=6, ()).unwrap();
    let _healthy_surface = healthy_compositor.create_surface(&healthy_qh, ());
    healthy_connection.flush().unwrap();
    expect_roundtrip_alive(&healthy_connection);

    let wm_base_b: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let xdg_surface_b = wm_base_b.get_xdg_surface(&surface, &qh, ());
    let positioner = wm_base_b.create_positioner(&qh, ());
    positioner.set_size(40, 30);
    positioner.set_anchor_rect(0, 0, 1, 1);
    xdg_surface_b.get_popup(None, &positioner, &qh, ());
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let observed = expect_protocol_error(
        &connection,
        "xdg_surface",
        client_xdg_surface::Error::AlreadyConstructed as u32,
    );
    assert_eq!(observed.object_id, xdg_surface_b.id().protocol_id());
    expect_roundtrip_alive(&healthy_connection);

    drop(healthy_compositor);
    drop(healthy_globals);
    drop(healthy_queue);
    drop(healthy_qh);
    drop(healthy_connection);
    drop(surface);
    drop(compositor);
    drop(globals);
    drop(queue);
    drop(connection);
    wait_for_server_commands(&commands);
    let server = stop_controllable_test_server(commands, server_thread);
    assert_eq!(
        server
            .state
            .compliance_metrics
            .xdg_cross_role_reassociation_rejections,
        1
    );
    assert_eq!(
        server
            .state
            .compliance_metrics
            .xdg_same_role_reassociations_total,
        0
    );
    assert_eq!(server.state.compliance_metrics.protocol_errors_total, 1);
}

#[test]
fn dormant_xdg_popup_reassociation_to_toplevel_is_rejected() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base_a: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface_a = wm_base_a.get_xdg_surface(&surface, &qh, ());
    let positioner = wm_base_a.create_positioner(&qh, ());
    positioner.set_size(40, 30);
    positioner.set_anchor_rect(0, 0, 1, 1);
    let popup_a = xdg_surface_a.get_popup(None, &positioner, &qh, ());
    popup_a.destroy();
    xdg_surface_a.destroy();
    surface.attach(None, 0, 0);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();

    let healthy_connection =
        Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (healthy_globals, healthy_queue) =
        registry_queue_init::<RegistryTestState>(&healthy_connection).unwrap();
    let healthy_qh = healthy_queue.handle();
    let healthy_compositor: client_wl_compositor::WlCompositor =
        healthy_globals.bind(&healthy_qh, 1..=6, ()).unwrap();
    let _healthy_surface = healthy_compositor.create_surface(&healthy_qh, ());
    healthy_connection.flush().unwrap();
    expect_roundtrip_alive(&healthy_connection);

    let wm_base_b: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let xdg_surface_b = wm_base_b.get_xdg_surface(&surface, &qh, ());
    xdg_surface_b.get_toplevel(&qh, ());
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let observed = expect_protocol_error(
        &connection,
        "xdg_surface",
        client_xdg_surface::Error::AlreadyConstructed as u32,
    );
    assert_eq!(observed.object_id, xdg_surface_b.id().protocol_id());
    expect_roundtrip_alive(&healthy_connection);

    drop(healthy_compositor);
    drop(healthy_globals);
    drop(healthy_queue);
    drop(healthy_qh);
    drop(healthy_connection);
    drop(surface);
    drop(compositor);
    drop(globals);
    drop(queue);
    drop(connection);
    wait_for_server_commands(&commands);
    let server = stop_controllable_test_server(commands, server_thread);
    assert_eq!(
        server
            .state
            .compliance_metrics
            .xdg_cross_role_reassociation_rejections,
        1
    );
    assert_eq!(
        server
            .state
            .compliance_metrics
            .xdg_same_role_reassociations_total,
        0
    );
    assert_eq!(server.state.compliance_metrics.protocol_errors_total, 1);
}

#[test]
fn sober_style_popup_reassociation_on_same_wl_surface_is_supported() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base_a: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base_b: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let mut state = RegistryTestState::default();

    let (parent_surface, parent_xdg_surface, parent_toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base_a, &shm, &qh, 120, 90).unwrap();
    parent_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let parent_surface_id =
        capture_xdg_role_snapshot(&commands, parent_surface.id().protocol_id()).surface_id;

    let popup_surface = compositor.create_surface(&qh, ());
    let popup_surface_id = popup_surface.id().protocol_id();
    let popup_xdg_surface_a = wm_base_a.get_xdg_surface(&popup_surface, &qh, ());
    let positioner_a = wm_base_a.create_positioner(&qh, ());
    positioner_a.set_size(60, 40);
    positioner_a.set_anchor_rect(10, 20, 1, 1);
    positioner_a.set_offset(3, 4);
    let popup_a = popup_xdg_surface_a.get_popup(Some(&parent_xdg_surface), &positioner_a, &qh, ());
    popup_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&popup_surface, &shm, &qh, 60, 40).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.popup_configure_count, 1);

    popup_a.destroy();
    popup_xdg_surface_a.destroy();
    popup_surface.attach(None, 0, 0);
    popup_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let popup_xdg_surface_b = wm_base_b.get_xdg_surface(&popup_surface, &qh, ());
    let positioner_b = wm_base_b.create_positioner(&qh, ());
    positioner_b.set_size(70, 50);
    positioner_b.set_anchor_rect(20, 30, 1, 1);
    positioner_b.set_offset(7, 8);
    let popup_b = popup_xdg_surface_b.get_popup(Some(&parent_xdg_surface), &positioner_b, &qh, ());
    popup_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&popup_surface, &shm, &qh, 70, 50).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    expect_roundtrip_alive(&connection);

    let snapshot = capture_xdg_role_snapshot(&commands, popup_surface_id);
    assert!(snapshot.configured);
    assert_eq!(snapshot.popup_count, 1);
    assert_eq!(snapshot.popup_node_count, 1);
    assert_eq!(snapshot.popup_parent_surface_id, Some(parent_surface_id));
    assert_eq!(state.popup_configure_count, 2);
    assert_eq!(
        snapshot.permanent_role,
        Some(PermanentSurfaceRole::XdgPopup)
    );
    assert!(snapshot.xdg_association);

    popup_b.destroy();
    popup_xdg_surface_b.destroy();
    parent_toplevel.destroy();
    parent_xdg_surface.destroy();
    popup_surface.destroy();
    parent_surface.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    drop(wm_base_b);
    drop(wm_base_a);
    drop(shm);
    drop(compositor);
    drop(queue);
    drop(globals);
    drop(connection);
    wait_for_server_commands(&commands);
    let server = stop_controllable_test_server(commands, server_thread);
    assert_eq!(server.state.compliance_metrics.protocol_errors_total, 0);
    assert_eq!(
        server
            .state
            .compliance_metrics
            .xdg_same_role_reassociations_total,
        1
    );
    assert_eq!(
        server
            .state
            .compliance_metrics
            .xdg_cross_role_reassociation_rejections,
        0
    );
    assert_eq!(
        server.state.compliance_metrics.client_state_leaks_detected,
        0
    );
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
fn popup_placement_ignores_uncommitted_parent_and_popup_geometry() {
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
    let parent = compositor.create_surface(&qh, ());
    let parent_xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let _toplevel = parent_xdg_surface.get_toplevel(&qh, ());
    parent.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();

    parent_xdg_surface.set_window_geometry(8, 9, 100, 80);
    commit_test_buffered_surface(&parent, &shm, &qh, 120, 90).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);
    let parent_id = capture_focused_surface_id(&commands).expect("mapped parent toplevel");

    // This request is double-buffered and intentionally has no parent commit.
    parent_xdg_surface.set_window_geometry(30, 40, 60, 50);

    let popup_surface = compositor.create_surface(&qh, ());
    let popup_xdg_surface = wm_base.get_xdg_surface(&popup_surface, &qh, ());
    let positioner = wm_base.create_positioner(&qh, ());
    positioner.set_size(40, 30);
    positioner.set_anchor_rect(10, 20, 1, 1);
    positioner.set_anchor(client_xdg_positioner::Anchor::TopLeft);
    positioner.set_gravity(client_xdg_positioner::Gravity::BottomRight);
    positioner.set_reactive();
    let popup = popup_xdg_surface.get_popup(Some(&parent_xdg_surface), &positioner, &qh, ());
    popup_xdg_surface.set_window_geometry(2, 3, 40, 30);
    commit_test_buffered_surface_after_initial_configure(
        &popup_surface,
        &shm,
        &qh,
        &connection,
        &mut queue,
        &mut state,
        40,
        30,
    )
    .unwrap();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);

    let popup_placement = || {
        let surfaces = capture_renderable_surface_snapshot(&commands);
        surfaces
            .iter()
            .find(|surface| surface.parent_surface_id.is_some())
            .cloned()
            .unwrap_or_else(|| panic!("popup is not renderable: {surfaces:?}"))
    };
    let initial_popup = popup_placement();
    assert_eq!(
        capture_effective_xdg_window_geometry(&commands, initial_popup.surface_id),
        Some(XdgWindowGeometry::new(2, 3, 38, 27))
    );
    assert_eq!((initial_popup.local_x, initial_popup.local_y), (16, 26));
    assert_eq!(
        capture_xdg_root_placement_authority(&commands, initial_popup.surface_id)
            .expect("popup placement authority")
            .canonical_surface_placement
            .local_x,
        16
    );

    // Neither the pending parent request nor this popup request may affect a
    // semantic placement triggered before their respective commits.
    popup_xdg_surface.set_window_geometry(20, 20, 10, 10);
    popup.reposition(&positioner, 77);
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);
    assert_eq!(
        (popup_placement().local_x, popup_placement().local_y),
        (16, 26)
    );

    popup_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);
    assert_eq!(
        (popup_placement().local_x, popup_placement().local_y),
        (-2, 9)
    );

    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);
    assert_eq!(
        capture_xdg_root_placement_authority(&commands, initial_popup.surface_id)
            .expect("reactive popup placement authority")
            .canonical_surface_placement,
        SurfacePlacement::subsurface(parent_id, 20, 40)
    );

    // The reactive configure changes semantic placement; the current popup
    // image follows that placement when the client commits its next content.
    commit_test_buffered_surface(&popup_surface, &shm, &qh, 40, 30).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);
    assert_eq!(
        (popup_placement().local_x, popup_placement().local_y),
        (20, 40)
    );

    stop_controllable_test_server(commands, server_thread);
}

#[test]
fn popup_placement_uses_the_parents_implicit_subsurface_geometry() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let subcompositor: client_wl_subcompositor::WlSubcompositor =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let parent = compositor.create_surface(&qh, ());
    let parent_xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let _toplevel = parent_xdg_surface.get_toplevel(&qh, ());
    let child = compositor.create_surface(&qh, ());
    let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    subsurface.set_position(-20, -10);
    parent.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&child, &shm, &qh, 200, 100).unwrap();
    commit_test_buffered_surface(&parent, &shm, &qh, 400, 300).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);

    let parent_id = capture_focused_surface_id(&commands).expect("mapped parent toplevel");
    let parent_geometry = capture_xdg_root_placement_authority(&commands, parent_id)
        .expect("parent geometry authority")
        .logical_window_geometry
        .expect("implicit parent geometry");
    assert_eq!((parent_geometry.width, parent_geometry.height), (420, 310));
    assert_eq!(
        capture_committed_window_geometry(&commands),
        None,
        "the parent remains never-explicit"
    );

    let popup_surface = compositor.create_surface(&qh, ());
    let popup_xdg_surface = wm_base.get_xdg_surface(&popup_surface, &qh, ());
    let positioner = wm_base.create_positioner(&qh, ());
    positioner.set_size(40, 30);
    positioner.set_anchor_rect(10, 20, 1, 1);
    positioner.set_anchor(client_xdg_positioner::Anchor::TopLeft);
    positioner.set_gravity(client_xdg_positioner::Gravity::BottomRight);
    let _popup = popup_xdg_surface.get_popup(Some(&parent_xdg_surface), &positioner, &qh, ());
    commit_test_buffered_surface_after_initial_configure(
        &popup_surface,
        &shm,
        &qh,
        &connection,
        &mut queue,
        &mut state,
        40,
        30,
    )
    .unwrap();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);

    let popup_surface = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .find(|surface| surface.width == 40 && surface.height == 30)
        .expect("popup renderable");
    assert_eq!((popup_surface.local_x, popup_surface.local_y), (-10, 10));

    stop_controllable_test_server(commands, server_thread);
}

#[test]
fn subsurface_destruction_publishes_topology_and_effective_geometry_immediately() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let subcompositor: client_wl_subcompositor::WlSubcompositor =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let parent = compositor.create_surface(&qh, ());
    let parent_xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let _toplevel = parent_xdg_surface.get_toplevel(&qh, ());
    let child = compositor.create_surface(&qh, ());
    let child_subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    child_subsurface.set_position(-20, -10);
    parent.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&child, &shm, &qh, 200, 100).unwrap();
    commit_test_buffered_surface(&parent, &shm, &qh, 400, 300).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);

    let root_id = capture_focused_surface_id(&commands).expect("mapped parent toplevel");
    let popup_surface = compositor.create_surface(&qh, ());
    let popup_xdg_surface = wm_base.get_xdg_surface(&popup_surface, &qh, ());
    let popup_positioner = wm_base.create_positioner(&qh, ());
    popup_positioner.set_size(40, 30);
    popup_positioner.set_anchor_rect(300, 200, 1, 1);
    popup_positioner.set_anchor(client_xdg_positioner::Anchor::TopLeft);
    popup_positioner.set_gravity(client_xdg_positioner::Gravity::BottomRight);
    let _popup = popup_xdg_surface.get_popup(Some(&parent_xdg_surface), &popup_positioner, &qh, ());
    popup_xdg_surface.set_window_geometry(2, 3, 40, 30);
    commit_test_buffered_surface_after_initial_configure(
        &popup_surface,
        &shm,
        &qh,
        &connection,
        &mut queue,
        &mut state,
        40,
        30,
    )
    .unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);
    let popup_protocol_surface_id = popup_surface.id().protocol_id();
    let popup_id = resolve_internal_surface_id(&commands, popup_protocol_surface_id);
    let popup_before = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .find(|surface| surface.surface_id == popup_id)
        .expect("mapped non-reactive popup");
    let popup_geometry_before = capture_effective_xdg_window_geometry(&commands, popup_id)
        .expect("popup effective geometry");
    let popup_global_window_origin_before = (
        popup_before.origin_x + popup_geometry_before.x,
        popup_before.origin_y + popup_geometry_before.y,
    );
    let popup_configure_count_before = state.popup_configure_count;
    let child_before = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .find(|surface| surface.parent_surface_id == Some(root_id))
        .expect("mapped child subsurface");
    let child_id = child_before.surface_id;
    let before = capture_xdg_root_placement_authority(&commands, root_id)
        .expect("root placement authority before role destruction");
    assert_eq!(
        capture_effective_xdg_window_geometry(&commands, root_id),
        Some(XdgWindowGeometry::new(-20, -10, 420, 310))
    );
    let pointer_x = f64::from(child_before.origin_x) + 1.0;
    let pointer_y = f64::from(child_before.origin_y) + 1.0;
    commands
        .send(ServerCommand::PointerMotion {
            x: pointer_x,
            y: pointer_y,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    assert_eq!(
        capture_pointer_scene_hit(&commands, pointer_x, pointer_y).0,
        Some(child_id),
        "pointer focus starts on the child that will be removed"
    );
    let render_generation_before = capture_render_generation(&commands);
    let scene_generation_before = capture_scene_render_generation(&commands);

    child_subsurface.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);

    let after = capture_xdg_root_placement_authority(&commands, root_id)
        .expect("root placement authority after role destruction");
    assert_eq!(
        capture_effective_xdg_window_geometry(&commands, root_id),
        Some(XdgWindowGeometry::new(0, 0, 400, 300))
    );
    assert_eq!(after.logical_frame_origin, before.logical_frame_origin);
    assert_eq!(
        after.resolved_render_origin,
        (
            before.resolved_render_origin.0 - 20,
            before.resolved_render_origin.1 - 10,
        )
    );
    assert_eq!(
        after.active_scene_origin,
        Some(after.resolved_render_origin)
    );
    let popup_after = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .find(|surface| surface.surface_id == popup_id)
        .expect("popup remains mapped after parent topology change");
    assert_eq!(
        (popup_after.local_x, popup_after.local_y),
        (popup_before.local_x + 20, popup_before.local_y + 10),
        "parent implicit-origin removal rebases the popup in the same publication"
    );
    let popup_geometry_after = capture_effective_xdg_window_geometry(&commands, popup_id)
        .expect("popup effective geometry after parent topology change");
    assert_eq!(
        (
            popup_after.origin_x + popup_geometry_after.x,
            popup_after.origin_y + popup_geometry_after.y,
        ),
        popup_global_window_origin_before,
        "topology publication preserves the popup's configured global position"
    );
    assert_eq!(state.popup_configure_count, popup_configure_count_before);
    assert!(
        !capture_renderable_surface_snapshot(&commands)
            .iter()
            .any(|surface| surface.surface_id == child_id),
        "destroyed subsurface must disappear without a parent commit"
    );
    assert_eq!(
        capture_pointer_scene_hit(&commands, pointer_x, pointer_y).0,
        Some(root_id),
        "pointer hit state is recomputed against the final root placement"
    );
    assert_eq!(
        capture_render_generation(&commands),
        render_generation_before + 1
    );
    assert_eq!(
        capture_scene_render_generation(&commands),
        scene_generation_before + 1,
        "topology and geometry must share one scene publication"
    );

    let contained_child = compositor.create_surface(&qh, ());
    let contained_subsurface = subcompositor.get_subsurface(&contained_child, &parent, &qh, ());
    contained_subsurface.set_position(50, 50);
    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&contained_child, &shm, &qh, 100, 100).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);
    let unchanged_geometry = capture_effective_xdg_window_geometry(&commands, root_id);
    let unchanged_render_generation = capture_render_generation(&commands);
    let unchanged_scene_generation = capture_scene_render_generation(&commands);

    contained_subsurface.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);

    assert_eq!(
        capture_effective_xdg_window_geometry(&commands, root_id),
        unchanged_geometry,
        "an in-bounds child does not change implicit geometry"
    );
    assert_eq!(
        capture_render_generation(&commands),
        unchanged_render_generation + 1
    );
    assert_eq!(
        capture_scene_render_generation(&commands),
        unchanged_scene_generation + 1
    );
    assert!(
        !capture_renderable_surface_snapshot(&commands)
            .iter()
            .any(|surface| surface.parent_surface_id == Some(root_id)),
        "geometry equality must not suppress topology publication"
    );

    let nested_a = compositor.create_surface(&qh, ());
    let nested_a_subsurface = subcompositor.get_subsurface(&nested_a, &parent, &qh, ());
    nested_a_subsurface.set_position(-20, -10);
    let nested_b = compositor.create_surface(&qh, ());
    let _nested_b_subsurface = subcompositor.get_subsurface(&nested_b, &nested_a, &qh, ());
    _nested_b_subsurface.set_position(-30, -20);
    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&nested_b, &shm, &qh, 100, 50).unwrap();
    commit_test_buffered_surface(&nested_a, &shm, &qh, 200, 100).unwrap();
    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);
    let nested_surfaces = capture_renderable_surface_snapshot(&commands);
    let nested_a_id = nested_surfaces
        .iter()
        .find(|surface| surface.parent_surface_id == Some(root_id))
        .expect("mapped parent of nested subtree")
        .surface_id;
    let nested_b_id = nested_surfaces
        .iter()
        .find(|surface| surface.parent_surface_id == Some(nested_a_id))
        .expect("mapped nested descendant")
        .surface_id;
    assert_eq!(
        capture_effective_xdg_window_geometry(&commands, root_id),
        Some(XdgWindowGeometry::new(-50, -30, 450, 330))
    );
    let nested_render_generation = capture_render_generation(&commands);
    let nested_scene_generation = capture_scene_render_generation(&commands);

    nested_a_subsurface.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);
    assert_eq!(
        capture_effective_xdg_window_geometry(&commands, root_id),
        Some(XdgWindowGeometry::new(0, 0, 400, 300))
    );
    let after_nested_destroy = capture_renderable_surface_snapshot(&commands);
    assert!(
        !after_nested_destroy
            .iter()
            .any(|surface| surface.surface_id == nested_a_id || surface.surface_id == nested_b_id)
    );
    assert_eq!(
        capture_render_generation(&commands),
        nested_render_generation + 1
    );
    assert_eq!(
        capture_scene_render_generation(&commands),
        nested_scene_generation + 1
    );

    parent_xdg_surface.set_window_geometry(0, 0, 400, 300);
    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let explicit_child = compositor.create_surface(&qh, ());
    let explicit_child_subsurface = subcompositor.get_subsurface(&explicit_child, &parent, &qh, ());
    explicit_child_subsurface.set_position(-20, -10);
    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&explicit_child, &shm, &qh, 200, 100).unwrap();
    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);
    let explicit_geometry = capture_effective_xdg_window_geometry(&commands, root_id);
    assert_eq!(
        explicit_geometry,
        Some(XdgWindowGeometry::new(0, 0, 400, 300))
    );

    explicit_child_subsurface.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);
    assert_eq!(
        capture_effective_xdg_window_geometry(&commands, root_id),
        explicit_geometry,
        "destroying a subsurface does not reclamp established explicit geometry"
    );

    stop_controllable_test_server(commands, server_thread);
}

#[test]
fn destroying_minimized_nested_subsurface_subtree_does_not_restore_it() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let subcompositor: client_wl_subcompositor::WlSubcompositor =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let root = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&root, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let child_a = compositor.create_surface(&qh, ());
    let child_a_subsurface = subcompositor.get_subsurface(&child_a, &root, &qh, ());
    child_a_subsurface.set_position(20, 20);
    let child_b = compositor.create_surface(&qh, ());
    let _child_b_subsurface = subcompositor.get_subsurface(&child_b, &child_a, &qh, ());
    child_a.commit();
    root.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&child_b, &shm, &qh, 20, 20).unwrap();
    commit_test_buffered_surface(&child_a, &shm, &qh, 40, 40).unwrap();
    commit_test_buffered_surface(&root, &shm, &qh, 200, 120).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);

    let root_id = capture_focused_surface_id(&commands).expect("mapped toplevel root");
    let before_minimize = capture_renderable_surface_snapshot(&commands);
    let child_a_id = before_minimize
        .iter()
        .find(|surface| surface.parent_surface_id == Some(root_id))
        .expect("mapped first-level subsurface")
        .surface_id;
    let child_b_id = before_minimize
        .iter()
        .find(|surface| surface.parent_surface_id == Some(child_a_id))
        .expect("mapped nested subsurface")
        .surface_id;

    commands.send(ServerCommand::MinimizeFocused).unwrap();
    wait_for_server_commands(&commands);
    child_a_subsurface.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);

    commands.send(ServerCommand::RestoreNextMinimized).unwrap();
    wait_for_server_commands(&commands);
    let restored = capture_renderable_surface_snapshot(&commands);
    assert!(restored.iter().any(|surface| surface.surface_id == root_id));
    assert!(
        !restored
            .iter()
            .any(|surface| surface.surface_id == child_a_id || surface.surface_id == child_b_id),
        "destroyed retained subtree surfaces must not be appended during restore: {restored:?}"
    );

    stop_controllable_test_server(commands, server_thread);
}

#[test]
fn non_reactive_popup_rebases_when_parent_geometry_origin_changes() {
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
    let parent = compositor.create_surface(&qh, ());
    let parent_xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let _toplevel = parent_xdg_surface.get_toplevel(&qh, ());
    parent.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();

    parent_xdg_surface.set_window_geometry(8, 9, 100, 80);
    commit_test_buffered_surface(&parent, &shm, &qh, 120, 90).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);
    let parent_id = capture_focused_surface_id(&commands).expect("mapped parent toplevel");

    let popup_surface = compositor.create_surface(&qh, ());
    let popup_xdg_surface = wm_base.get_xdg_surface(&popup_surface, &qh, ());
    let positioner = wm_base.create_positioner(&qh, ());
    positioner.set_size(40, 30);
    positioner.set_anchor_rect(10, 20, 1, 1);
    positioner.set_anchor(client_xdg_positioner::Anchor::TopLeft);
    positioner.set_gravity(client_xdg_positioner::Gravity::BottomRight);
    let _popup = popup_xdg_surface.get_popup(Some(&parent_xdg_surface), &positioner, &qh, ());
    popup_xdg_surface.set_window_geometry(2, 3, 40, 30);
    commit_test_buffered_surface_after_initial_configure(
        &popup_surface,
        &shm,
        &qh,
        &connection,
        &mut queue,
        &mut state,
        40,
        30,
    )
    .unwrap();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    popup_xdg_surface.set_window_geometry(2, 3, 40, 30);
    popup_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);

    let popup_snapshot = || {
        capture_renderable_surface_snapshot(&commands)
            .into_iter()
            .find(|surface| surface.parent_surface_id == Some(parent_id))
            .expect("mapped popup")
    };
    let before_popup = popup_snapshot();
    assert_eq!((before_popup.local_x, before_popup.local_y), (16, 26));
    let configure_count_before = state.popup_configure_count;

    popup_xdg_surface.set_window_geometry(20, 20, 10, 10);
    popup_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);
    let own_origin_changed = popup_snapshot();
    assert_eq!(
        (own_origin_changed.local_x, own_origin_changed.local_y),
        (-2, 9)
    );
    assert_eq!(
        (
            own_origin_changed.origin_x + 20,
            own_origin_changed.origin_y + 20,
        ),
        (before_popup.origin_x + 2, before_popup.origin_y + 3),
        "changing popup geometry origin preserves its global configured window position"
    );

    popup_xdg_surface.set_window_geometry(2, 3, 40, 30);
    popup_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);
    assert_eq!(
        (popup_snapshot().local_x, popup_snapshot().local_y),
        (16, 26)
    );
    assert_eq!(state.popup_configure_count, configure_count_before);

    parent_xdg_surface.set_window_geometry(30, 40, 100, 80);
    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);

    let after_popup = popup_snapshot();
    assert_eq!((after_popup.local_x, after_popup.local_y), (38, 57));
    assert_eq!(
        (after_popup.origin_x, after_popup.origin_y),
        (before_popup.origin_x, before_popup.origin_y),
        "internal rebasing keeps the configured popup window position stable"
    );
    assert_eq!(state.popup_configure_count, configure_count_before);

    parent_xdg_surface.set_window_geometry(30, 40, 60, 50);
    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);
    let size_only_parent_change = popup_snapshot();
    assert_eq!(
        (
            size_only_parent_change.local_x,
            size_only_parent_change.local_y
        ),
        (38, 57)
    );
    assert_eq!(
        (
            size_only_parent_change.origin_x,
            size_only_parent_change.origin_y
        ),
        (after_popup.origin_x, after_popup.origin_y)
    );
    assert_eq!(state.popup_configure_count, configure_count_before);

    stop_controllable_test_server(commands, server_thread);
}

#[test]
fn non_reactive_nested_popups_keep_global_positions_across_geometry_origin_changes() {
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
    let root = compositor.create_surface(&qh, ());
    let root_xdg_surface = wm_base.get_xdg_surface(&root, &qh, ());
    let _toplevel = root_xdg_surface.get_toplevel(&qh, ());
    root.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    root_xdg_surface.set_window_geometry(8, 9, 100, 80);
    commit_test_buffered_surface(&root, &shm, &qh, 120, 90).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);

    let popup_a_surface = compositor.create_surface(&qh, ());
    let popup_a_xdg_surface = wm_base.get_xdg_surface(&popup_a_surface, &qh, ());
    let popup_a_positioner = wm_base.create_positioner(&qh, ());
    popup_a_positioner.set_size(40, 30);
    popup_a_positioner.set_anchor_rect(10, 20, 1, 1);
    popup_a_positioner.set_anchor(client_xdg_positioner::Anchor::TopLeft);
    popup_a_positioner.set_gravity(client_xdg_positioner::Gravity::BottomRight);
    let _popup_a =
        popup_a_xdg_surface.get_popup(Some(&root_xdg_surface), &popup_a_positioner, &qh, ());
    commit_test_buffered_surface_after_initial_configure(
        &popup_a_surface,
        &shm,
        &qh,
        &connection,
        &mut queue,
        &mut state,
        40,
        30,
    )
    .unwrap();
    popup_a_xdg_surface.set_window_geometry(2, 3, 40, 30);
    popup_a_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);

    let popup_b_surface = compositor.create_surface(&qh, ());
    let popup_b_xdg_surface = wm_base.get_xdg_surface(&popup_b_surface, &qh, ());
    let popup_b_positioner = wm_base.create_positioner(&qh, ());
    popup_b_positioner.set_size(20, 15);
    popup_b_positioner.set_anchor_rect(5, 6, 1, 1);
    popup_b_positioner.set_anchor(client_xdg_positioner::Anchor::TopLeft);
    popup_b_positioner.set_gravity(client_xdg_positioner::Gravity::BottomRight);
    let _popup_b =
        popup_b_xdg_surface.get_popup(Some(&popup_a_xdg_surface), &popup_b_positioner, &qh, ());
    commit_test_buffered_surface_after_initial_configure(
        &popup_b_surface,
        &shm,
        &qh,
        &connection,
        &mut queue,
        &mut state,
        20,
        15,
    )
    .unwrap();
    popup_b_xdg_surface.set_window_geometry(1, 2, 20, 15);
    popup_b_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);

    let root_id = capture_focused_surface_id(&commands).expect("mapped toplevel");
    let popup_a_id = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .find(|surface| surface.parent_surface_id == Some(root_id))
        .expect("mapped first popup")
        .surface_id;
    let popup_a_snapshot = || {
        capture_renderable_surface_snapshot(&commands)
            .into_iter()
            .find(|surface| surface.surface_id == popup_a_id)
            .expect("first popup remains mapped")
    };
    let popup_b_snapshot = || {
        capture_renderable_surface_snapshot(&commands)
            .into_iter()
            .find(|surface| surface.parent_surface_id == Some(popup_a_id))
            .expect("mapped nested popup")
    };
    let before_a = popup_a_snapshot();
    let before_b = popup_b_snapshot();
    assert_eq!((before_a.local_x, before_a.local_y), (16, 26));
    assert_eq!((before_b.local_x, before_b.local_y), (6, 7));
    let configure_count_before = state.popup_configure_count;

    root_xdg_surface.set_window_geometry(30, 40, 100, 80);
    root.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);
    let after_root_origin_change_a = popup_a_snapshot();
    let after_root_origin_change_b = popup_b_snapshot();
    assert_eq!(
        (
            after_root_origin_change_a.local_x,
            after_root_origin_change_a.local_y
        ),
        (38, 57)
    );
    assert_eq!(
        (
            after_root_origin_change_a.origin_x,
            after_root_origin_change_a.origin_y
        ),
        (before_a.origin_x, before_a.origin_y)
    );
    assert_eq!(
        (
            after_root_origin_change_b.origin_x,
            after_root_origin_change_b.origin_y
        ),
        (before_b.origin_x, before_b.origin_y)
    );

    popup_a_xdg_surface.set_window_geometry(20, 20, 10, 10);
    popup_a_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);
    let after_popup_a_origin_change_a = popup_a_snapshot();
    let after_popup_a_origin_change_b = popup_b_snapshot();
    assert_eq!(
        (
            after_popup_a_origin_change_a.local_x,
            after_popup_a_origin_change_a.local_y
        ),
        (20, 40)
    );
    assert_eq!(
        (
            after_popup_a_origin_change_b.local_x,
            after_popup_a_origin_change_b.local_y
        ),
        (24, 24)
    );
    assert_eq!(
        (
            after_popup_a_origin_change_b.origin_x,
            after_popup_a_origin_change_b.origin_y
        ),
        (before_b.origin_x, before_b.origin_y),
        "the child popup counter-rebases against its popup parent's geometry origin"
    );
    assert_eq!(state.popup_configure_count, configure_count_before);

    stop_controllable_test_server(commands, server_thread);
}

#[test]
fn non_reactive_popup_origin_translation_does_not_rerun_constraints() {
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
    let parent = compositor.create_surface(&qh, ());
    let parent_xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let _toplevel = parent_xdg_surface.get_toplevel(&qh, ());
    parent.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    parent_xdg_surface.set_window_geometry(8, 9, 100, 80);
    commit_test_buffered_surface(&parent, &shm, &qh, 120, 90).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);

    let popup_surface = compositor.create_surface(&qh, ());
    let popup_xdg_surface = wm_base.get_xdg_surface(&popup_surface, &qh, ());
    let positioner = wm_base.create_positioner(&qh, ());
    positioner.set_size(40, 30);
    positioner.set_anchor_rect(90, 70, 1, 1);
    positioner.set_anchor(client_xdg_positioner::Anchor::TopLeft);
    positioner.set_gravity(client_xdg_positioner::Gravity::BottomRight);
    positioner.set_constraint_adjustment(
        client_xdg_positioner::ConstraintAdjustment::SlideX
            | client_xdg_positioner::ConstraintAdjustment::SlideY,
    );
    let _popup = popup_xdg_surface.get_popup(Some(&parent_xdg_surface), &positioner, &qh, ());
    commit_test_buffered_surface_after_initial_configure(
        &popup_surface,
        &shm,
        &qh,
        &connection,
        &mut queue,
        &mut state,
        40,
        30,
    )
    .unwrap();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let popup = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .find(|surface| surface.parent_surface_id.is_some())
        .expect("mapped popup");
    let configure_count_before = state.popup_configure_count;

    parent_xdg_surface.set_window_geometry(30, 40, 60, 50);
    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);
    let rebased_popup = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .find(|surface| surface.surface_id == popup.surface_id)
        .expect("popup remains mapped");
    assert_eq!((popup.local_x, popup.local_y), (68, 59));
    assert_eq!((rebased_popup.local_x, rebased_popup.local_y), (90, 90));
    assert_eq!(
        (rebased_popup.origin_x, rebased_popup.origin_y),
        (popup.origin_x, popup.origin_y),
        "the configured popup rectangle must remain fixed while only coordinate space changes"
    );
    assert_eq!(state.popup_configure_count, configure_count_before);

    stop_controllable_test_server(commands, server_thread);
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
fn wayland_popup_content_commits_skip_popup_topology_maintenance() {
    const CONTENT_COMMITS: usize = 1_000;
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_client_popup_with_rotating_shm_buffers(&socket_path, &commands, CONTENT_COMMITS)
        .unwrap();
    let server = stop_controllable_test_server(commands, server_thread);
    let metrics = server.core_compliance_metrics();

    assert_eq!(metrics.surface_commit_popup_topology_updates, 1);
    assert_eq!(metrics.surface_commit_popup_pointer_refreshes, 1);
    assert_eq!(metrics.surface_commit_stack_reorders, 2);
    assert_eq!(
        metrics.surface_commit_stack_reorder_skips,
        CONTENT_COMMITS as u64
    );
    assert_eq!(
        metrics.surface_commit_geometry_noops,
        CONTENT_COMMITS as u64
    );
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
    const MAX_POPUP_FD_DELTA: usize = 8;
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

    let (parent_surface, parent_xdg_surface, parent_toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 120, 90).unwrap();
    parent_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let baseline_fd_count = open_fd_count();

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
            child_xdg_surface.destroy();
            child_positioner.destroy();
            child_surface.destroy();
        }

        if index % 200 == 199 {
            parent_surface.attach(None, 0, 0);
            parent_surface.commit();
            connection.flush().unwrap();
            queue.roundtrip(&mut state).unwrap();
        }

        popup.destroy();
        popup_xdg_surface.destroy();
        positioner.destroy();
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
            let surface_resource_count = capture_surface_resource_count(&commands);
            assert_eq!(surface_resource_count, 1);
            let (current_buffer_count, pending_release_count, frame_batch_count) =
                capture_shm_resource_counts(&commands);
            assert_eq!(current_buffer_count, 1);
            assert_eq!(pending_release_count, 0);
            assert_eq!(frame_batch_count, 0);
            let fd_count = open_fd_count();
            assert!(
                fd_count <= baseline_fd_count + MAX_POPUP_FD_DELTA,
                "popup cycle {index} grew open descriptors from {baseline_fd_count} to {fd_count}"
            );
        }
    }

    let snapshot = capture_xdg_role_snapshot(&commands, 0);
    assert_eq!(snapshot.popup_count, 0);
    assert_eq!(snapshot.popup_node_count, 0);
    assert!(!snapshot.popup_grab_active);
    assert_eq!(capture_renderable_surface_count(&commands), 1);

    parent_toplevel.destroy();
    parent_xdg_surface.destroy();
    parent_surface.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);
}
