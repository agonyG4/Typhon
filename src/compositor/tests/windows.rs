use super::*;

fn root_surface_origin(surfaces: &[RenderableSurfaceSnapshot]) -> Option<(i32, i32)> {
    surfaces
        .iter()
        .find(|surface| surface.parent_surface_id.is_none())
        .map(|surface| (surface.local_x, surface.local_y))
}

fn root_buffer_id(surfaces: &[RenderableSurfaceSnapshot]) -> Option<u64> {
    surfaces
        .iter()
        .find(|surface| surface.parent_surface_id.is_none())
        .map(|surface| surface.buffer_id)
}

fn map_exclusive_top_panel(
    socket_path: &PathBuf,
) -> (
    Connection,
    EventQueue<RegistryTestState>,
    client_wl_surface::WlSurface,
    client_zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
) {
    let stream = UnixStream::connect(socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let panel = layer_shell.get_layer_surface(
        &surface,
        None,
        client_zwlr_layer_shell_v1::Layer::Top,
        "reserved-panel".to_string(),
        &qh,
        (),
    );
    panel.set_anchor(
        client_zwlr_layer_surface_v1::Anchor::Top
            | client_zwlr_layer_surface_v1::Anchor::Left
            | client_zwlr_layer_surface_v1::Anchor::Right,
    );
    panel.set_size(0, 32);
    panel.set_exclusive_zone(32);
    surface.commit();
    connection.flush().unwrap();

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&surface, &shm, &qh, 1280, 32).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    (connection, queue, surface, panel)
}

#[test]
fn window_drag_moves_root_surface_without_moving_children_independently() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_client_toplevel_with_positioned_subsurface_buffer(&socket_path).unwrap();
    wait_for_server_commands(&commands);
    commands
        .send(ServerCommand::BeginMove {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 1.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 1.0,
        })
        .unwrap();
    commands
        .send(ServerCommand::UpdateInteraction {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 33.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 25.0,
        })
        .unwrap();
    commands.send(ServerCommand::EndInteraction).unwrap();
    wait_for_server_commands(&commands);
    assert_eq!(
        capture_render_generation_cause(&commands),
        RenderGenerationCause::WindowMove
    );
    let server = stop_controllable_test_server(commands, server_thread);

    let origins = render::surface_origins(server.renderable_surfaces());
    assert_eq!(
        server.render_generation_cause(),
        RenderGenerationCause::WindowMove
    );
    let parent_index = server
        .renderable_surfaces()
        .iter()
        .position(|surface| surface.width == 2 && surface.height == 2)
        .expect("parent toplevel should remain renderable");
    let child_index = server
        .renderable_surfaces()
        .iter()
        .position(|surface| surface.width == 1 && surface.height == 1)
        .expect("child subsurface should remain renderable");

    assert_eq!(origins[parent_index], (104, 96));
    assert_eq!(origins[child_index], (114, 108));
}

#[test]
fn wayland_client_receives_resize_configure_for_focused_window() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state =
        create_configured_client_toplevel_then_resize_focused(&socket_path, &commands, 960, 640);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert_eq!(state.toplevel_width, 960);
    assert_eq!(state.toplevel_height, 640);
}

#[test]
fn window_minimize_hides_focused_toplevel_until_restored() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_client_toplevel_with_sized_shm_buffer(&socket_path, 300, 200).unwrap();
    wait_for_server_commands(&commands);
    commands.send(ServerCommand::MinimizeFocused).unwrap();
    wait_for_server_commands(&commands);
    commands.send(ServerCommand::RestoreNextMinimized).unwrap();
    wait_for_server_commands(&commands);
    let server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(server.renderable_surfaces().len(), 1);
    assert_eq!(server.renderable_surfaces()[0].width, 300);
    assert_eq!(server.renderable_surfaces()[0].height, 200);
}

#[test]
fn minimized_toplevel_stays_hidden_when_client_commits_new_buffer() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let client = LiveTestClient::connect(&socket_path).unwrap();
    let surface = client
        .create_toplevel_surface("oblivion.minimize-commit-test", 300, 200)
        .unwrap();
    wait_for_server_commands(&commands);
    assert_eq!(capture_renderable_surface_count(&commands), 1);

    commands.send(ServerCommand::MinimizeFocused).unwrap();
    wait_for_server_commands(&commands);
    assert_eq!(capture_renderable_surface_count(&commands), 0);

    client.commit_surface(&surface, 320, 220).unwrap();
    wait_for_server_commands(&commands);
    assert_eq!(capture_renderable_surface_count(&commands), 0);

    commands.send(ServerCommand::RestoreNextMinimized).unwrap();
    wait_for_server_commands(&commands);
    let server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(server.renderable_surfaces().len(), 1);
    assert_eq!(server.renderable_surfaces()[0].width, 320);
    assert_eq!(server.renderable_surfaces()[0].height, 220);
}

#[test]
fn window_maximize_configures_focused_toplevel_and_restores_geometry() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_buffered_toplevel_then_toggle_maximize(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(state.toplevel_width, 1280);
    assert_eq!(state.toplevel_height, 800);
    assert!(state.toplevel_has_state(client_xdg_toplevel::State::Maximized));
}

#[test]
fn window_maximize_uses_current_output_size() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    commands
        .send(ServerCommand::SetOutputSize {
            width: 1600,
            height: 900,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    let state = create_buffered_toplevel_then_toggle_maximize(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(state.toplevel_width, 1600);
    assert_eq!(state.toplevel_height, 900);
    assert!(state.toplevel_has_state(client_xdg_toplevel::State::Maximized));
}

#[test]
fn maximized_uses_reserved_usable_geometry() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let _panel = map_exclusive_top_panel(&socket_path);

    let state = create_buffered_toplevel_then_toggle_maximize(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(state.toplevel_width, 1280);
    assert_eq!(state.toplevel_height, 768);
    assert!(state.toplevel_has_state(client_xdg_toplevel::State::Maximized));
    assert!(!state.toplevel_has_state(client_xdg_toplevel::State::Fullscreen));
}

#[test]
fn window_unmaximize_restores_previous_toplevel_geometry() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state =
        create_buffered_toplevel_then_toggle_maximize_twice(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(state.toplevel_width, 300);
    assert_eq!(state.toplevel_height, 200);
    assert!(!state.toplevel_has_state(client_xdg_toplevel::State::Maximized));
}

#[test]
fn window_fullscreen_configures_focused_toplevel_and_restores_geometry() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_buffered_toplevel_then_toggle_fullscreen(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(state.toplevel_width, 1280);
    assert_eq!(state.toplevel_height, 800);
    assert!(state.toplevel_has_state(client_xdg_toplevel::State::Fullscreen));
}

#[test]
fn fullscreen_ignores_layer_shell_exclusive_zone_for_main_geometry() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let _panel = map_exclusive_top_panel(&socket_path);

    let state = create_buffered_toplevel_then_toggle_fullscreen(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(state.toplevel_width, 1280);
    assert_eq!(state.toplevel_height, 800);
    assert!(state.toplevel_has_state(client_xdg_toplevel::State::Fullscreen));
    assert!(!state.toplevel_has_state(client_xdg_toplevel::State::Maximized));
}

#[test]
fn fullscreen_later_root_uses_absolute_output_origin() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let (state, surface_ids) = create_three_buffered_toplevels_then_toggle_mode(
        &socket_path,
        &commands,
        ServerCommand::ToggleFullscreenFocused,
        false,
    )
    .unwrap();
    let snapshots = capture_renderable_surface_snapshot(&commands);
    let fullscreen = snapshots
        .iter()
        .find(|surface| surface.surface_id == surface_ids[2])
        .expect("fullscreen root should be renderable");
    let role = capture_xdg_role_snapshot(&commands, surface_ids[2]);
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(state.toplevel_has_state(client_xdg_toplevel::State::Fullscreen));
    assert_eq!((fullscreen.origin_x, fullscreen.origin_y), (0, 0));
    assert_eq!((fullscreen.width, fullscreen.height), (1280, 800));
    assert_eq!(
        role.placement.expect("fullscreen placement").root_mode,
        RootPlacementMode::Absolute
    );
}

#[test]
fn fullscreen_origin_is_independent_of_raise_and_focus_order() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let (state, surface_ids) = create_three_buffered_toplevels_then_toggle_mode(
        &socket_path,
        &commands,
        ServerCommand::ToggleFullscreenFocused,
        true,
    )
    .unwrap();
    let snapshots = capture_renderable_surface_snapshot(&commands);
    let fullscreen = snapshots
        .iter()
        .find(|surface| surface.surface_id == surface_ids[2])
        .expect("fullscreen root should be renderable");
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(state.toplevel_has_state(client_xdg_toplevel::State::Fullscreen));
    assert_eq!((fullscreen.origin_x, fullscreen.origin_y), (0, 0));
}

#[test]
fn fullscreen_client_request_uses_absolute_output_origin() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let (state, surface_ids) =
        create_three_buffered_toplevels_then_client_fullscreen(&socket_path, &commands).unwrap();
    let snapshots = capture_renderable_surface_snapshot(&commands);
    let fullscreen = snapshots
        .iter()
        .find(|surface| surface.surface_id == surface_ids[2])
        .expect("fullscreen root should be renderable");
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(state.toplevel_has_state(client_xdg_toplevel::State::Fullscreen));
    assert_eq!((fullscreen.origin_x, fullscreen.origin_y), (0, 0));
    assert_eq!((fullscreen.width, fullscreen.height), (1280, 800));
}

#[test]
fn fullscreen_csd_window_geometry_aligns_to_output() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_csd_toplevel_then_client_fullscreen(&socket_path, &commands).unwrap();
    let snapshots = capture_renderable_surface_snapshot(&commands);
    let root = snapshots
        .iter()
        .find(|surface| surface.parent_surface_id.is_none())
        .expect("fullscreen CSD root should be renderable");
    let geometry = capture_committed_window_geometry(&commands);
    let role = capture_xdg_role_snapshot(&commands, root.surface_id);
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(state.toplevel_has_state(client_xdg_toplevel::State::Fullscreen));
    assert_eq!((state.toplevel_width, state.toplevel_height), (1280, 800));
    assert_eq!(geometry, Some(XdgWindowGeometry::new(20, 20, 1280, 800)));
    assert_eq!((root.origin_x + 20, root.origin_y + 20), (0, 0));
    assert_eq!((root.width, root.height), (1320, 840));
    assert_eq!(
        role.placement.expect("fullscreen CSD placement").root_mode,
        RootPlacementMode::Absolute
    );
}

#[test]
fn unfullscreen_csd_clears_absolute_render_assignment() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let _state = create_csd_toplevel_then_client_fullscreen(&socket_path, &commands).unwrap();
    commands
        .send(ServerCommand::ToggleFullscreenFocused)
        .unwrap();
    wait_for_server_commands(&commands);
    let snapshots = capture_renderable_surface_snapshot(&commands);
    let root = snapshots
        .iter()
        .find(|surface| surface.parent_surface_id.is_none())
        .expect("restored CSD root should be renderable");
    let role = capture_xdg_role_snapshot(&commands, root.surface_id);
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!((root.origin_x, root.origin_y), (72, 72));
    assert_eq!(
        role.placement.unwrap_or_default().root_mode,
        RootPlacementMode::CascadedWindow
    );
}

#[test]
fn maximized_later_root_uses_absolute_usable_output_origin() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let (state, surface_ids) = create_three_buffered_toplevels_then_toggle_mode(
        &socket_path,
        &commands,
        ServerCommand::ToggleMaximizeFocused,
        false,
    )
    .unwrap();
    let usable = capture_usable_output_geometry(&commands);
    let snapshots = capture_renderable_surface_snapshot(&commands);
    let maximized = snapshots
        .iter()
        .find(|surface| surface.surface_id == surface_ids[2])
        .expect("maximized root should be renderable");
    let role = capture_xdg_role_snapshot(&commands, surface_ids[2]);
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(state.toplevel_has_state(client_xdg_toplevel::State::Maximized));
    assert_eq!(
        (maximized.origin_x, maximized.origin_y),
        (usable.x as i32, usable.y as i32)
    );
    assert_eq!(
        (maximized.width, maximized.height),
        (usable.width as u32, usable.height as u32)
    );
    assert_eq!(
        role.placement.expect("maximized placement").root_mode,
        RootPlacementMode::Absolute
    );
}

#[test]
fn maximized_later_root_respects_reserved_usable_geometry_without_cascade() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let _panel = map_exclusive_top_panel(&socket_path);

    let (state, surface_ids) = create_three_buffered_toplevels_then_toggle_mode(
        &socket_path,
        &commands,
        ServerCommand::ToggleMaximizeFocused,
        false,
    )
    .unwrap();
    let usable = capture_usable_output_geometry(&commands);
    let snapshots = capture_renderable_surface_snapshot(&commands);
    let maximized = snapshots
        .iter()
        .find(|surface| surface.surface_id == surface_ids[2])
        .expect("maximized root should be renderable");
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(state.toplevel_has_state(client_xdg_toplevel::State::Maximized));
    assert_eq!(
        (maximized.origin_x, maximized.origin_y),
        (usable.x as i32, usable.y as i32)
    );
    assert_eq!(
        (maximized.width, maximized.height),
        (usable.width as u32, usable.height as u32)
    );
}

#[test]
fn fullscreen_exact_cover_requires_absolute_zero_origin() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let _state = create_buffered_toplevel_then_shortcut_fullscreen_with_committed_geometry(
        &socket_path,
        &commands,
    )
    .unwrap();
    let exact = capture_fullscreen_presentation_eligibility(&commands);
    assert!(exact.exactly_covers_output);

    set_focused_root_visual_geometry(
        &commands,
        SurfacePlacement::root_at(
            -render::FIRST_SURFACE_OFFSET.0,
            -render::FIRST_SURFACE_OFFSET.1,
        ),
        1280,
        800,
    );
    assert!(!capture_fullscreen_presentation_eligibility(&commands).exactly_covers_output);

    set_focused_root_visual_geometry(
        &commands,
        SurfacePlacement::absolute_root_at(1, 0),
        1280,
        800,
    );
    assert!(!capture_fullscreen_presentation_eligibility(&commands).exactly_covers_output);

    set_focused_root_visual_geometry(
        &commands,
        SurfacePlacement::absolute_root_at(0, 0),
        1279,
        800,
    );
    let wrong_size = capture_fullscreen_presentation_eligibility(&commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(!wrong_size.exactly_covers_output);
}

#[test]
fn unfullscreen_restores_exact_previous_floating_placement() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let custom_x = 215;
    let custom_y = 137;

    let state = create_buffered_toplevel_then_window_commands(
        &socket_path,
        &commands,
        &[
            ServerCommand::BeginMove {
                x: f64::from(render::FIRST_SURFACE_OFFSET.0 + 1),
                y: f64::from(render::FIRST_SURFACE_OFFSET.1 + 1),
            },
            ServerCommand::UpdateInteraction {
                x: f64::from(render::FIRST_SURFACE_OFFSET.0 + custom_x + 1),
                y: f64::from(render::FIRST_SURFACE_OFFSET.1 + custom_y + 1),
            },
            ServerCommand::EndInteraction,
            ServerCommand::ToggleFullscreenFocused,
            ServerCommand::ToggleFullscreenFocused,
        ],
    )
    .unwrap();
    let snapshots = capture_renderable_surface_snapshot(&commands);
    let root = snapshots
        .iter()
        .find(|surface| surface.parent_surface_id.is_none())
        .expect("restored root should be renderable");
    let role = capture_xdg_role_snapshot(&commands, root.surface_id);
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!((root.local_x, root.local_y), (custom_x, custom_y));
    assert_eq!((root.width, root.height), (300, 200));
    assert_eq!(
        role.placement.expect("restored placement").root_mode,
        RootPlacementMode::CascadedWindow
    );
    assert!(!state.toplevel_has_state(client_xdg_toplevel::State::Fullscreen));
}

#[test]
fn unmaximize_restores_exact_previous_floating_placement() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let custom_x = 215;
    let custom_y = 137;

    let state = create_buffered_toplevel_then_window_commands(
        &socket_path,
        &commands,
        &[
            ServerCommand::BeginMove {
                x: f64::from(render::FIRST_SURFACE_OFFSET.0 + 1),
                y: f64::from(render::FIRST_SURFACE_OFFSET.1 + 1),
            },
            ServerCommand::UpdateInteraction {
                x: f64::from(render::FIRST_SURFACE_OFFSET.0 + custom_x + 1),
                y: f64::from(render::FIRST_SURFACE_OFFSET.1 + custom_y + 1),
            },
            ServerCommand::EndInteraction,
            ServerCommand::ToggleMaximizeFocused,
            ServerCommand::ToggleMaximizeFocused,
        ],
    )
    .unwrap();
    let snapshots = capture_renderable_surface_snapshot(&commands);
    let root = snapshots
        .iter()
        .find(|surface| surface.parent_surface_id.is_none())
        .expect("restored root should be renderable");
    let role = capture_xdg_role_snapshot(&commands, root.surface_id);
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!((root.local_x, root.local_y), (custom_x, custom_y));
    assert_eq!((root.width, root.height), (300, 200));
    assert_eq!(
        role.placement.expect("restored placement").root_mode,
        RootPlacementMode::CascadedWindow
    );
    assert!(!state.toplevel_has_state(client_xdg_toplevel::State::Maximized));
}

#[test]
fn fullscreen_output_resize_preserves_absolute_origin() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_buffered_toplevel_then_mode_and_output_resize(
        &socket_path,
        &commands,
        ServerCommand::ToggleFullscreenFocused,
        1600,
        900,
    )
    .unwrap();
    let snapshots = capture_renderable_surface_snapshot(&commands);
    let root = snapshots
        .iter()
        .find(|surface| surface.parent_surface_id.is_none())
        .expect("fullscreen root should be renderable");
    let role = capture_xdg_role_snapshot(&commands, root.surface_id);
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(state.toplevel_has_state(client_xdg_toplevel::State::Fullscreen));
    assert_eq!((state.toplevel_width, state.toplevel_height), (1600, 900));
    assert_eq!((root.origin_x, root.origin_y), (0, 0));
    assert_eq!((root.width, root.height), (1600, 900));
    assert_eq!(
        role.placement.expect("fullscreen placement").root_mode,
        RootPlacementMode::Absolute
    );
}

#[test]
fn maximized_output_resize_preserves_absolute_usable_origin() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_buffered_toplevel_then_mode_and_output_resize(
        &socket_path,
        &commands,
        ServerCommand::ToggleMaximizeFocused,
        1600,
        900,
    )
    .unwrap();
    let usable = capture_usable_output_geometry(&commands);
    let snapshots = capture_renderable_surface_snapshot(&commands);
    let root = snapshots
        .iter()
        .find(|surface| surface.parent_surface_id.is_none())
        .expect("maximized root should be renderable");
    let role = capture_xdg_role_snapshot(&commands, root.surface_id);
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(state.toplevel_has_state(client_xdg_toplevel::State::Maximized));
    assert_eq!((state.toplevel_width, state.toplevel_height), (1600, 900));
    assert_eq!(
        (root.origin_x, root.origin_y),
        (usable.x as i32, usable.y as i32)
    );
    assert_eq!(
        (root.width, root.height),
        (usable.width as u32, usable.height as u32)
    );
    assert_eq!(
        role.placement.expect("maximized placement").root_mode,
        RootPlacementMode::Absolute
    );
}

#[test]
fn maximized_geometry_updates_when_exclusive_panel_maps() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_buffered_toplevel_then_toggle_maximize(&socket_path, &commands).unwrap();
    assert!(state.toplevel_has_state(client_xdg_toplevel::State::Maximized));
    let _panel = map_exclusive_top_panel(&socket_path);
    let usable = capture_usable_output_geometry(&commands);
    let snapshots = capture_renderable_surface_snapshot(&commands);
    let root = snapshots
        .iter()
        .find(|surface| surface.parent_surface_id.is_none())
        .expect("maximized root should be renderable");
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(
        (root.origin_x, root.origin_y),
        (usable.x as i32, usable.y as i32)
    );
}

#[test]
fn maximized_geometry_updates_when_exclusive_panel_unmaps() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_buffered_toplevel_then_toggle_maximize(&socket_path, &commands).unwrap();
    assert!(state.toplevel_has_state(client_xdg_toplevel::State::Maximized));
    let (panel_connection, mut panel_queue, panel_surface, _panel) =
        map_exclusive_top_panel(&socket_path);
    panel_surface.attach(None, 0, 0);
    panel_surface.commit();
    panel_connection.flush().unwrap();
    wait_for_server_commands(&commands);
    panel_queue
        .roundtrip(&mut RegistryTestState::default())
        .unwrap();
    let usable = capture_usable_output_geometry(&commands);
    let snapshots = capture_renderable_surface_snapshot(&commands);
    let root = snapshots
        .iter()
        .find(|surface| surface.parent_surface_id.is_none())
        .expect("maximized root should be renderable");
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!((usable.x, usable.y), (0.0, 0.0));
    assert_eq!((root.origin_x, root.origin_y), (0, 0));
}

#[test]
fn fullscreen_subsurface_remains_relative_to_absolute_root() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_client_toplevel_with_positioned_subsurface_buffer(&socket_path).unwrap();
    wait_for_server_commands(&commands);
    commands
        .send(ServerCommand::ToggleFullscreenFocused)
        .unwrap();
    wait_for_server_commands(&commands);
    let snapshots = capture_renderable_surface_snapshot(&commands);
    let root = snapshots
        .iter()
        .find(|surface| surface.parent_surface_id.is_none())
        .expect("fullscreen root should be renderable");
    let child = snapshots
        .iter()
        .find(|surface| surface.parent_surface_id == Some(root.surface_id))
        .expect("fullscreen child should be renderable");
    let role = capture_xdg_role_snapshot(&commands, root.surface_id);
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!((root.origin_x, root.origin_y), (0, 0));
    assert_eq!(
        role.placement.expect("fullscreen root placement").root_mode,
        RootPlacementMode::Absolute
    );
    assert_eq!((child.local_x, child.local_y), (10, 12));
    assert_eq!((child.origin_x, child.origin_y), (10, 12));
}

#[test]
fn window_resize_drag_sends_configure_for_root_surface() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_buffered_toplevel_then_resize_drag(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert_eq!(state.toplevel_width, 300);
    assert_eq!(state.toplevel_height, 200);
}

#[test]
fn resize_drag_coalesces_pointer_updates_behind_in_flight_configure() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_buffered_toplevel_then_coalesced_resize_drag(&socket_path, &commands);
    let server = stop_controllable_test_server(commands, server_thread);
    let origins = render::surface_origins(server.renderable_surfaces());

    let state = state.unwrap();
    assert_eq!(state.toplevel_configure_count, 2);
    assert_eq!(state.toplevel_width, 340);
    assert_eq!(state.toplevel_height, 230);
    assert!(state.toplevel_has_state(client_xdg_toplevel::State::Resizing));
    assert_eq!(origins.first().copied(), Some(render::FIRST_SURFACE_OFFSET));
    let metrics = server.resize_flow_metrics();
    assert_eq!(metrics.raw_pointer_resize_updates, 3);
    assert_eq!(metrics.pending_resize_updates_replaced, 2);
    assert_eq!(metrics.resize_updates_applied, 1);
}

#[test]
fn resize_drag_configure_reports_resizing_state_while_active() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_buffered_toplevel_then_active_resize_configure(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert!(state.toplevel_has_state(client_xdg_toplevel::State::Resizing));
}

#[test]
fn resize_drag_does_not_send_next_configure_without_client_progress() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_buffered_toplevel_then_resize_drag_without_client_commit_between_frames(
        &socket_path,
        &commands,
    );
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert_eq!(state.toplevel_configure_count, 2);
}

#[test]
fn queued_resize_configure_reports_pending_frame_work() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let pending =
        create_buffered_toplevel_then_queue_resize_configure_and_capture_pending_frame_work(
            &socket_path,
            &commands,
        )
        .unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(pending);
}

#[test]
fn unmapping_resized_window_clears_queued_resize_configure() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let pending =
        create_buffered_toplevel_then_queue_resize_configure_and_unmap(&socket_path, &commands)
            .unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(!pending);
}

#[test]
fn prepare_frame_flushes_queued_resize_configure_before_present_frame() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let (before_prepare, after_prepare) =
        create_buffered_toplevel_then_prepare_queued_resize_configure_and_capture_pending_frame_work(
            &socket_path,
            &commands,
        )
        .unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(before_prepare);
    assert!(!after_prepare);
}

#[test]
fn resize_drag_updates_visual_target_before_client_commit() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_client_toplevel_with_sized_shm_buffer(&socket_path, 300, 200).unwrap();
    wait_for_server_commands(&commands);
    commands
        .send(ServerCommand::BeginFrameAction {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 304.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 204.0,
        })
        .unwrap();
    commands
        .send(ServerCommand::UpdateInteraction {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 344.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 234.0,
        })
        .unwrap();
    commands.send(ServerCommand::PrepareFrame).unwrap();
    wait_for_server_commands(&commands);
    let server = stop_controllable_test_server(commands, server_thread);

    let surface = &server.renderable_surfaces()[0];
    assert_eq!(server.renderable_surfaces().len(), 1);
    assert_eq!(surface.width, 300);
    assert_eq!(surface.height, 200);
    assert_eq!(surface.generation, 1);
    assert_eq!(
        surface.visual_clip,
        Some(render::SurfaceTargetRect::new(0, 0, 340, 230))
    );
}

fn state_with_preview_resize(
    resizing: bool,
) -> (
    CompositorState,
    u32,
    ResizeCommitSnapshot,
    PendingResizeConfigure,
) {
    let mut state = CompositorState::default();
    let surface_id = 42;
    let identity = BufferIdAllocator::default()
        .allocate()
        .expect("test buffer identity");
    let desired = PendingResizeConfigure {
        surface_id,
        width: 1200,
        height: 700,
        placement: SurfacePlacement::root_at(100, 80),
        edges: ResizeEdges::BOTTOM_RIGHT,
        resizing,
        interaction_id: ResizeInteractionId::new(1),
    };
    let resize = ResizeCommitSnapshot {
        serial: 7,
        sequence: 1,
        commit_sequence: 1,
        width: desired.width,
        height: desired.height,
        placement: desired.placement,
        edges: desired.edges,
        resizing,
        emitted_at: Instant::now(),
        committed_size: Some((944, 502)),
        committed_window_geometry: None,
        buffer_id: Some(identity.id().get()),
        interaction_id: desired.interaction_id,
    };

    state.renderable_surfaces.push(RenderableSurface {
        surface_id,
        x: 0,
        y: 0,
        width: 944,
        height: 502,
        placement: SurfacePlacement::root(),
        render_placement: None,
        visual_clip: None,
        generation: 1,
        commit_sequence: SurfaceCommitSequence::initial(),
        buffer: crate::render_backend::buffer::CommittedSurfaceBuffer::shm_snapshot(
            identity,
            BufferSize::new(944, 502).expect("test size"),
            vec![0; 944 * 502],
        ),
        viewport_source: None,
        damage: RenderableSurfaceDamage::Full,
    });
    state
        .surface_window_geometries
        .insert(surface_id, XdgWindowGeometry::new(0, 0, 944, 502));
    state.active_toplevel_resizes.insert(
        surface_id,
        ActiveToplevelResize {
            interaction_id: desired.interaction_id,
            flow_sequence: 1,
            edges: desired.edges,
            activated_at: Instant::now(),
        },
    );
    assert!(state.preview_resize_root_window_to(
        surface_id,
        desired.width,
        desired.height,
        desired.placement,
        desired.edges,
        desired.interaction_id,
    ));
    install_captured_resize_snapshot(&mut state, surface_id, desired, resize);
    (state, surface_id, resize, desired)
}

fn install_captured_resize_snapshot(
    state: &mut CompositorState,
    surface_id: u32,
    desired: PendingResizeConfigure,
    snapshot: ResizeCommitSnapshot,
) {
    let flow = state.resize_configure_flows.entry(surface_id).or_default();
    flow.mark_sent(desired, snapshot.serial, snapshot.sequence);
    assert_eq!(flow.ack(snapshot.serial), ResizeAckDecision::Matched);
    let captured = flow
        .capture(snapshot.commit_sequence)
        .expect("resize snapshot should be captured");
    assert_eq!(captured.sequence, snapshot.sequence);
}

#[test]
fn intermediate_geometry_only_commit_preserves_active_resize_preview() {
    let (mut state, surface_id, resize, _desired) = state_with_preview_resize(true);

    assert!(state.complete_pending_resize_from_current_geometry(surface_id, resize));

    assert!(
        !state
            .resize_configure_flows
            .get(&surface_id)
            .is_some_and(ResizeConfigureFlow::has_in_flight)
    );
    let surface = &state.renderable_surfaces[0];
    assert_eq!(
        surface.visual_clip,
        Some(render::SurfaceTargetRect::new(100, 80, 1200, 700))
    );

    let visual = state
        .current_visual_root_window_geometry(surface_id)
        .expect("visual geometry");
    let committed = state
        .current_root_window_geometry(surface_id)
        .expect("committed geometry");

    assert_eq!(visual.width, 1200);
    assert_eq!(visual.height, 700);
    assert_eq!(visual.placement, SurfacePlacement::root_at(100, 80));
    assert_eq!(committed.width, 944);
    assert_eq!(committed.height, 502);
}

#[test]
fn applied_resize_capture_is_removed_from_runtime_flow() {
    let (mut state, surface_id, resize, _desired) = state_with_preview_resize(true);

    assert_eq!(
        state
            .resize_configure_flows
            .get(&surface_id)
            .map_or(0, ResizeConfigureFlow::captured_count),
        1
    );

    assert!(state.complete_applied_resize_transaction(surface_id, resize));

    assert!(!state.resize_configure_flows.contains_key(&surface_id));
}

#[test]
fn sequential_successful_resize_commits_do_not_stall_runtime_flow() {
    let (mut state, surface_id, mut resize, desired) = state_with_preview_resize(true);

    for sequence in 1..=17 {
        resize.sequence = sequence;
        resize.commit_sequence = sequence;
        assert!(state.complete_applied_resize_transaction(surface_id, resize));
        if sequence < 17 {
            let next = ResizeCommitSnapshot {
                sequence: sequence + 1,
                commit_sequence: sequence + 1,
                ..resize
            };
            install_captured_resize_snapshot(&mut state, surface_id, desired, next);
            assert_eq!(
                state
                    .resize_configure_flows
                    .get(&surface_id)
                    .map_or(0, ResizeConfigureFlow::in_flight_configure_count),
                1
            );
        }
    }

    assert!(!state.resize_configure_flows.contains_key(&surface_id));
    assert_eq!(state.resize_flow_metrics.resize_captures_completed, 17);
}

#[test]
fn resize_flow_keeps_queued_latest_until_current_capture_completes() {
    let (mut state, surface_id, resize, desired) = state_with_preview_resize(true);
    state
        .resize_configure_flows
        .get_mut(&surface_id)
        .expect("flow")
        .queue(PendingResizeConfigure {
            width: desired.width + 10,
            ..desired
        });

    let flow = state
        .resize_configure_flows
        .get_mut(&surface_id)
        .expect("queued flow remains");
    assert!(flow.complete_applied(resize.sequence));
    let next = flow.take_sendable().expect("queued latest");
    assert_eq!(next.width, desired.width + 10);
    assert!(flow.is_empty());
}

#[test]
fn repeated_resize_completion_does_not_leave_stale_active_preview() {
    let (mut state, surface_id, intermediate, desired) = state_with_preview_resize(true);
    assert!(state.complete_applied_resize_transaction(surface_id, intermediate));

    let final_resize = ResizeCommitSnapshot {
        sequence: 2,
        commit_sequence: 2,
        resizing: false,
        ..intermediate
    };
    install_captured_resize_snapshot(&mut state, surface_id, desired, final_resize);

    assert!(state.complete_pending_resize_from_current_geometry(surface_id, final_resize));

    assert!(!state.active_toplevel_resizes.contains_key(&surface_id));
    assert_eq!(
        state
            .toplevel_visual_geometries
            .get(&surface_id)
            .and_then(|visual| visual.active_resize),
        None
    );
    assert!(!state.resize_configure_flows.contains_key(&surface_id));
}

#[test]
fn kitty_like_resize_swapchain_never_selects_destroyed_buffer_identity() {
    let mut ids = BufferIdAllocator::default();
    let old = ids.allocate().expect("old buffer identity");
    let buffer_b = ids.allocate().expect("buffer B identity");
    let buffer_c = ids.allocate().expect("buffer C identity");
    let buffer_d = ids.allocate().expect("buffer D identity");
    let handle = DmabufBufferHandle::new(
        BufferSize::new(1000, 696).expect("cell-aligned test size"),
        DrmFormat::Xrgb8888,
        vec![RenderDmabufPlane::new(
            File::open("/dev/null")
                .expect("dmabuf identity test fd")
                .into(),
            DmabufPlaneDescriptor {
                plane_index: 0,
                offset: 0,
                stride: 4000,
                modifier: DrmModifier::LINEAR,
            },
        )],
    )
    .expect("valid fake dmabuf");
    let old_key = crate::render_backend::buffer::DmabufImageKey::from_handle(old.id(), &handle);
    let keys = [buffer_b, buffer_c, buffer_d].map(|identity| {
        crate::render_backend::buffer::DmabufImageKey::from_handle(identity.id(), &handle)
    });
    let mut fake_renderer_cache = HashMap::from([(old_key.clone(), 'A')]);
    let mut selected = Vec::new();

    fake_renderer_cache.remove(&old_key);
    for (key, frame) in keys.into_iter().zip(['B', 'C', 'D']) {
        fake_renderer_cache.insert(key.clone(), frame);
        selected.push(*fake_renderer_cache.get(&key).expect("new renderer input"));
    }

    assert_eq!(selected, ['B', 'C', 'D']);
    assert!(!fake_renderer_cache.contains_key(&old_key));
    assert!(
        fake_renderer_cache
            .keys()
            .all(|key| key.buffer_id() != old.id())
    );
}

#[test]
fn damage_only_commit_preserves_rendered_size_while_resize_is_pending() {
    let existing = BufferSize {
        width: 300,
        height: 200,
    };
    let requested = BufferSize {
        width: 900,
        height: 600,
    };

    assert_eq!(
        damage_only_rendered_surface_size(existing, requested, true),
        existing
    );
    assert_eq!(
        damage_only_rendered_surface_size(existing, requested, false),
        requested
    );
}

#[test]
fn resize_drag_end_clears_resizing_state() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_buffered_toplevel_then_resize_drag_and_release(&socket_path, &commands);
    let server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert_eq!(state.toplevel_configure_count, 3);
    assert_eq!(state.toplevel_width, 340);
    assert_eq!(state.toplevel_height, 230);
    assert!(!state.toplevel_has_state(client_xdg_toplevel::State::Resizing));
    let metrics = server.resize_flow_metrics();
    assert_eq!(metrics.max_in_flight_configures, 1);
    assert_eq!(metrics.acks_unknown, 0);
    assert!(metrics.configures_sent >= 2);
    assert!(metrics.commits_captured >= 1);
    assert_eq!(metrics.preview_completions, 0);
}

#[test]
fn alt_resize_from_top_left_grows_left_and_up() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state =
        create_buffered_toplevel_then_alt_top_left_resize_drag_and_release(&socket_path, &commands);
    let server = stop_controllable_test_server(commands, server_thread);
    let origins = render::surface_origins(server.renderable_surfaces());

    let state = state.unwrap();
    assert_eq!(state.toplevel_width, 340);
    assert_eq!(state.toplevel_height, 230);
    assert_eq!(
        origins.first().copied(),
        Some((
            render::FIRST_SURFACE_OFFSET.0 - 40,
            render::FIRST_SURFACE_OFFSET.1 - 30
        ))
    );
}

#[test]
fn csd_buffer_margin_resize_release_keeps_configure_at_window_geometry() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_csd_toplevel_then_resize_drag_commit_buffer_margin_and_release(
        &socket_path,
        &commands,
    );
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert_eq!(state.toplevel_configure_count, 3);
    assert_eq!(state.toplevel_width, 340);
    assert_eq!(state.toplevel_height, 230);
    assert!(!state.toplevel_has_state(client_xdg_toplevel::State::Resizing));
}

#[test]
fn pending_window_geometry_does_not_move_current_buffer_before_commit() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let snapshots =
        capture_csd_window_geometry_pending_and_committed_resize_snapshots(&socket_path, &commands)
            .unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(
        root_surface_origin(&snapshots.before_pending),
        root_surface_origin(&snapshots.after_pending_without_commit)
    );
    assert_eq!(
        snapshots.before_pending_geometry,
        snapshots.after_pending_without_commit_geometry
    );
}

#[test]
fn geometry_only_commit_is_applied_at_commit_time() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let snapshots =
        capture_csd_window_geometry_pending_and_committed_resize_snapshots(&socket_path, &commands)
            .unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_ne!(
        snapshots.after_pending_without_commit_geometry,
        snapshots.after_geometry_commit_geometry
    );
}

#[test]
fn csd_geometry_only_final_commit_keeps_logical_visual_size() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let snapshots =
        capture_csd_consecutive_resize_regression_snapshots(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(snapshots.first_final.toplevel_width, 340);
    assert_eq!(snapshots.first_final.toplevel_height, 230);
    assert_eq!(
        snapshots.first_final.visual,
        Some(ToplevelVisualGeometrySnapshot {
            local_x: 0,
            local_y: 0,
            width: 340,
            height: 230,
            active_resize: false,
        })
    );
    assert_eq!(
        snapshots.first_final.window_geometry,
        Some(XdgWindowGeometry::new(16, 10, 340, 230))
    );
    assert!(
        snapshots
            .first_final
            .surfaces
            .iter()
            .all(|surface| !surface.resize_preview_active)
    );
}

#[test]
fn next_csd_resize_starts_from_window_geometry_not_buffer_size() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let snapshots =
        capture_csd_consecutive_resize_regression_snapshots(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(snapshots.first_final.toplevel_width, 340);
    assert_eq!(snapshots.second_preview.toplevel_width, 336);
    assert_eq!(snapshots.second_preview.toplevel_height, 230);
    assert_eq!(
        snapshots.second_preview.visual,
        Some(ToplevelVisualGeometrySnapshot {
            local_x: 0,
            local_y: 0,
            width: 336,
            height: 230,
            active_resize: true,
        })
    );
    assert!(
        snapshots
            .second_preview
            .surfaces
            .iter()
            .all(|surface| surface.resize_preview_active)
    );
}

#[test]
fn one_pixel_csd_shrink_never_grows_by_buffer_margins() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let snapshots =
        capture_csd_consecutive_resize_regression_snapshots(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(snapshots.second_final.toplevel_width, 336);
    assert_eq!(snapshots.third_preview.toplevel_width, 333);
    assert_eq!(snapshots.third_preview.toplevel_height, 230);
    assert!(
        snapshots.third_preview.toplevel_width < snapshots.second_final.toplevel_width,
        "a small CSD shrink must not grow from the raw buffer margin"
    );
    assert!(
        snapshots
            .second_final
            .surfaces
            .iter()
            .all(|surface| !surface.resize_preview_active)
    );
}

#[test]
fn left_top_csd_resize_anchors_using_logical_window_size() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let snapshots =
        capture_csd_top_left_resize_regression_snapshot(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(
        snapshots.top_left_preview.visual,
        Some(ToplevelVisualGeometrySnapshot {
            local_x: snapshots.first_final.visual.unwrap().local_x + 4,
            local_y: snapshots.first_final.visual.unwrap().local_y + 5,
            width: 336,
            height: 225,
            active_resize: true,
        })
    );
    let visual = snapshots
        .top_left_preview
        .visual
        .expect("top-left preview should be active");
    assert_eq!(
        visual.local_x + i32::try_from(visual.width).unwrap(),
        snapshots.first_final.visual.unwrap().local_x + snapshots.first_final.toplevel_width
    );
    assert_eq!(
        visual.local_y + i32::try_from(visual.height).unwrap(),
        snapshots.first_final.visual.unwrap().local_y + snapshots.first_final.toplevel_height
    );
}

#[test]
fn explicit_sync_resize_applies_buffer_and_window_geometry_atomically() {
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
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let snapshots = capture_syncobj_resize_window_geometry_snapshots(
        &socket_path,
        &commands,
        &acquire_timeline,
        &release_timeline,
    )
    .unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(
        root_surface_origin(&snapshots.before_blocked_commit),
        root_surface_origin(&snapshots.while_acquire_blocked)
    );
    assert_eq!(
        root_buffer_id(&snapshots.before_blocked_commit),
        root_buffer_id(&snapshots.while_acquire_blocked)
    );
    assert_eq!(
        snapshots.before_blocked_geometry,
        snapshots.blocked_geometry
    );
    assert_ne!(snapshots.blocked_geometry, snapshots.after_acquire_geometry);
    assert_ne!(
        root_buffer_id(&snapshots.while_acquire_blocked),
        root_buffer_id(&snapshots.after_acquire_ready)
    );
}

#[test]
fn resize_preview_without_client_commit_advances_render_generation() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let (before_resize, after_resize) =
        create_buffered_toplevel_then_measure_configure_only_resize_generation(
            &socket_path,
            &commands,
        )
        .unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(after_resize > before_resize);
}

#[test]
fn resize_motion_inside_threshold_does_not_report_visual_update() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_client_toplevel_with_sized_shm_buffer(&socket_path, 300, 200).unwrap();
    wait_for_server_commands(&commands);
    commands
        .send(ServerCommand::BeginFrameAction {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 304.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 204.0,
        })
        .unwrap();
    let updated = update_interaction_and_report(
        &commands,
        f64::from(render::FIRST_SURFACE_OFFSET.0) + 305.0,
        f64::from(render::FIRST_SURFACE_OFFSET.1) + 205.0,
    );
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(!updated);
}

#[test]
fn resize_motion_at_same_size_does_not_report_repeated_visual_update() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_client_toplevel_with_sized_shm_buffer(&socket_path, 300, 200).unwrap();
    wait_for_server_commands(&commands);
    commands
        .send(ServerCommand::BeginFrameAction {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 304.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 204.0,
        })
        .unwrap();
    assert!(update_interaction_and_report(
        &commands,
        f64::from(render::FIRST_SURFACE_OFFSET.0) + 344.0,
        f64::from(render::FIRST_SURFACE_OFFSET.1) + 234.0,
    ));
    let updated = update_interaction_and_report(
        &commands,
        f64::from(render::FIRST_SURFACE_OFFSET.0) + 344.0,
        f64::from(render::FIRST_SURFACE_OFFSET.1) + 234.0,
    );
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(!updated);
}

#[test]
fn frame_corner_resize_click_with_tiny_motion_does_not_resize_window() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_buffered_toplevel_then_frame_corner_resize_click_with_tiny_motion(
        &socket_path,
        &commands,
    )
    .unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(state.toplevel_configure_count, 1);
}

#[test]
fn left_edge_resize_shrink_updates_visual_target_before_client_commit() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_buffered_toplevel_then_left_edge_shrink_before_client_commit(
        &socket_path,
        &commands,
    )
    .unwrap();
    let server = stop_controllable_test_server(commands, server_thread);
    let origins = render::surface_origins(server.renderable_surfaces());

    assert_eq!(state.toplevel_width, 260);
    assert_eq!(state.toplevel_height, 200);
    assert_eq!(server.renderable_surfaces()[0].width, 300);
    assert_eq!(server.renderable_surfaces()[0].height, 200);
    assert_eq!(
        server.renderable_surfaces()[0].visual_clip,
        Some(render::SurfaceTargetRect::new(40, 0, 260, 200))
    );
    assert_eq!(
        origins.first().copied(),
        Some((
            render::FIRST_SURFACE_OFFSET.0 + 40,
            render::FIRST_SURFACE_OFFSET.1,
        ))
    );
    assert_eq!(origins[0].0 + 260, render::FIRST_SURFACE_OFFSET.0 + 300);
}

#[test]
fn resize_preview_clamps_to_toplevel_min_size_before_client_commit() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state =
        create_min_size_toplevel_then_shrink_resize_before_client_commit(&socket_path, &commands)
            .unwrap();
    let server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(state.toplevel_width, 280);
    assert_eq!(state.toplevel_height, 180);
    let surface = server
        .renderable_surfaces()
        .first()
        .expect("toplevel should remain renderable");
    assert_eq!((surface.width, surface.height), (320, 220));
    assert_eq!(
        surface.visual_clip,
        Some(render::SurfaceTargetRect::new(0, 0, 280, 180))
    );
}

#[test]
fn scaled_buffer_resize_shrink_does_not_commit_physical_size_as_logical_size() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state =
        create_scaled_buffer_toplevel_then_right_edge_shrink_and_commit(&socket_path, &commands)
            .unwrap();
    let server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(state.toplevel_width, 260);
    let surface = server
        .renderable_surfaces()
        .first()
        .expect("scaled toplevel should remain renderable");
    assert_eq!((surface.width, surface.height), (260, 200));
    assert_eq!(
        (surface.buffer_size().width, surface.buffer_size().height),
        (520, 400)
    );
}

#[test]
fn scaled_buffer_left_edge_shrink_keeps_logical_commit_anchor() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state =
        create_scaled_buffer_toplevel_then_left_edge_shrink_and_commit(&socket_path, &commands)
            .unwrap();
    let server = stop_controllable_test_server(commands, server_thread);
    let origins = render::surface_origins(server.renderable_surfaces());

    assert_eq!(state.toplevel_width, 260);
    let surface = server
        .renderable_surfaces()
        .first()
        .expect("scaled toplevel should remain renderable");
    assert_eq!((surface.width, surface.height), (260, 200));
    assert_eq!(
        (surface.buffer_size().width, surface.buffer_size().height),
        (520, 400)
    );
    assert_eq!(
        origins.first().copied(),
        Some((
            render::FIRST_SURFACE_OFFSET.0 + 40,
            render::FIRST_SURFACE_OFFSET.1
        ))
    );
}

#[test]
fn xdg_toplevel_move_request_starts_interactive_move_from_pointer_serial() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_buffered_toplevel_request_move_and_drag(&socket_path, &commands).unwrap();
    let server = stop_controllable_test_server(commands, server_thread);
    let origins = render::surface_origins(server.renderable_surfaces());

    assert_eq!(origins.first().copied(), Some((112, 100)));
}

#[test]
fn xdg_toplevel_move_request_accepts_serial_from_same_client_chrome_surface() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state =
        create_toplevel_request_move_from_client_chrome_surface(&socket_path, &commands).unwrap();
    let server = stop_controllable_test_server(commands, server_thread);
    let origins = render::surface_origins(server.renderable_surfaces());
    let toplevel_index = server
        .renderable_surfaces()
        .iter()
        .position(|surface| surface.width == 100 && surface.height == 80)
        .expect("toplevel should remain renderable");
    let toplevel_id = server.renderable_surfaces()[toplevel_index].surface_id;

    assert_eq!(state.pointer_surface_x, Some(12.0));
    assert_eq!(state.pointer_surface_y, Some(14.0));
    assert_eq!(
        server.state.surface_placement(toplevel_id),
        SurfacePlacement::root_at(80, 60)
    );
    assert_eq!(origins[toplevel_index], (152, 132));
}

#[test]
fn xdg_toplevel_resize_request_uses_requested_edge() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state =
        create_buffered_toplevel_request_top_left_resize_and_drag(&socket_path, &commands).unwrap();
    let server = stop_controllable_test_server(commands, server_thread);
    let origins = render::surface_origins(server.renderable_surfaces());

    assert_eq!(state.toplevel_width, 340);
    assert_eq!(state.toplevel_height, 230);
    assert_eq!(origins.first().copied(), Some((32, 42)));
}

#[test]
fn future_server_titlebar_area_does_not_move_window_without_client_request() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_client_toplevel_with_sized_shm_buffer(&socket_path, 300, 200).unwrap();
    wait_for_server_commands(&commands);
    commands
        .send(ServerCommand::BeginFrameAction {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 60.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) - 18.0,
        })
        .unwrap();
    commands
        .send(ServerCommand::UpdateInteraction {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 102.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 9.0,
        })
        .unwrap();
    commands.send(ServerCommand::EndInteraction).unwrap();
    wait_for_server_commands(&commands);
    let server = stop_controllable_test_server(commands, server_thread);
    let origins = render::surface_origins(server.renderable_surfaces());

    assert_eq!(origins.first().copied(), Some(render::FIRST_SURFACE_OFFSET));
}

#[test]
fn frame_hit_testing_keeps_content_and_future_titlebar_clicks_for_clients() {
    assert_eq!(
        window_frame_action_for_local_point(60.0, 18.0, 300, 200),
        None
    );
    assert_eq!(
        window_frame_action_for_local_point(60.0, -18.0, 300, 200),
        None
    );
    assert_eq!(
        window_frame_action_for_local_point(304.0, 204.0, 300, 200),
        Some(WindowInteractionKind::Resize(ResizeEdges::BOTTOM_RIGHT))
    );
    assert_eq!(
        window_frame_action_for_local_point(-3.0, -3.0, 300, 200),
        Some(WindowInteractionKind::Resize(ResizeEdges::new(
            true, false, true, false
        )))
    );
}

#[test]
fn frame_hit_testing_supports_xy_resize_on_all_corners() {
    assert_eq!(
        window_frame_action_for_local_point(-3.0, -3.0, 300, 200),
        Some(WindowInteractionKind::Resize(ResizeEdges::new(
            true, false, true, false
        )))
    );
    assert_eq!(
        window_frame_action_for_local_point(303.0, -3.0, 300, 200),
        Some(WindowInteractionKind::Resize(ResizeEdges::new(
            true, false, false, true
        )))
    );
    assert_eq!(
        window_frame_action_for_local_point(-3.0, 203.0, 300, 200),
        Some(WindowInteractionKind::Resize(ResizeEdges::new(
            false, true, true, false
        )))
    );
    assert_eq!(
        window_frame_action_for_local_point(303.0, 203.0, 300, 200),
        Some(WindowInteractionKind::Resize(ResizeEdges::BOTTOM_RIGHT))
    );
}

#[test]
fn browser_client_chrome_empty_band_click_does_not_start_drag_without_client_request() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_client_toplevel_with_app_id_and_sized_shm_buffer(
        &socket_path,
        "brave-browser",
        900,
        600,
    )
    .unwrap();
    wait_for_server_commands(&commands);
    commands
        .send(ServerCommand::BeginFrameAction {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 500.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 10.0,
        })
        .unwrap();
    commands
        .send(ServerCommand::UpdateInteraction {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 548.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 32.0,
        })
        .unwrap();
    commands.send(ServerCommand::EndInteraction).unwrap();
    wait_for_server_commands(&commands);
    let server = stop_controllable_test_server(commands, server_thread);
    let origins = render::surface_origins(server.renderable_surfaces());

    assert_eq!(origins.first().copied(), Some(render::FIRST_SURFACE_OFFSET));
}

#[test]
fn browser_client_chrome_tab_area_click_does_not_start_drag_fallback() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_client_toplevel_with_app_id_and_sized_shm_buffer(
        &socket_path,
        "brave-browser",
        900,
        600,
    )
    .unwrap();
    wait_for_server_commands(&commands);
    commands
        .send(ServerCommand::BeginFrameAction {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 140.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 10.0,
        })
        .unwrap();
    commands
        .send(ServerCommand::UpdateInteraction {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 188.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 32.0,
        })
        .unwrap();
    commands.send(ServerCommand::EndInteraction).unwrap();
    wait_for_server_commands(&commands);
    let server = stop_controllable_test_server(commands, server_thread);
    let origins = render::surface_origins(server.renderable_surfaces());

    assert_eq!(origins.first().copied(), Some(render::FIRST_SURFACE_OFFSET));
}

#[test]
fn browser_client_chrome_toolbar_center_click_does_not_start_drag_fallback() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_client_toplevel_with_app_id_and_sized_shm_buffer(&socket_path, "firefox", 900, 600)
        .unwrap();
    wait_for_server_commands(&commands);
    commands
        .send(ServerCommand::BeginFrameAction {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 500.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 10.0,
        })
        .unwrap();
    commands
        .send(ServerCommand::UpdateInteraction {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 548.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 32.0,
        })
        .unwrap();
    commands.send(ServerCommand::EndInteraction).unwrap();
    wait_for_server_commands(&commands);
    let server = stop_controllable_test_server(commands, server_thread);
    let origins = render::surface_origins(server.renderable_surfaces());

    assert_eq!(origins.first().copied(), Some(render::FIRST_SURFACE_OFFSET));
}

#[test]
fn non_browser_client_top_content_click_does_not_start_drag_fallback() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_client_toplevel_with_app_id_and_sized_shm_buffer(&socket_path, "kitty", 900, 600)
        .unwrap();
    wait_for_server_commands(&commands);
    commands
        .send(ServerCommand::BeginFrameAction {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 500.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 10.0,
        })
        .unwrap();
    commands
        .send(ServerCommand::UpdateInteraction {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 548.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 32.0,
        })
        .unwrap();
    commands.send(ServerCommand::EndInteraction).unwrap();
    wait_for_server_commands(&commands);
    let server = stop_controllable_test_server(commands, server_thread);
    let origins = render::surface_origins(server.renderable_surfaces());

    assert_eq!(origins.first().copied(), Some(render::FIRST_SURFACE_OFFSET));
}

#[test]
fn frame_corner_drag_resizes_window_without_client_request() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state =
        create_buffered_toplevel_then_frame_corner_resize_drag(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(state.toplevel_width, 340);
    assert_eq!(state.toplevel_height, 230);
}
