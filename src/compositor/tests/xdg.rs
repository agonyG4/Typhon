use super::*;
use std::os::fd::AsRawFd;

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
    assert_eq!(snapshot.placement, None);
    assert_eq!(
        snapshot.permanent_role,
        Some(PermanentSurfaceRole::XdgToplevel)
    );
    assert!(snapshot.xdg_association);
    assert!(!snapshot.toplevel_has_app_id);
    assert!(!snapshot.toplevel_has_title);
    assert!(!snapshot.toplevel_has_non_default_constraints);
    assert_eq!(snapshot.toplevel_mode, Some(ToplevelMode::Floating));
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
    attach_test_buffered_surface(&surface_a, &shm_a, &qh_a, 2, 2).unwrap();

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
