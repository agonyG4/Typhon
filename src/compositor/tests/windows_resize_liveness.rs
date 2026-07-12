use super::*;

#[test]
fn fullscreen_during_active_resize_restores_visible_pre_transition_geometry() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let offset = render::FIRST_SURFACE_OFFSET;

    let state = create_buffered_toplevel_then_window_commands(
        &socket_path,
        &commands,
        &[
            ServerCommand::BeginResize {
                x: f64::from(offset.0 + 299),
                y: f64::from(offset.1 + 199),
            },
            ServerCommand::UpdateInteraction {
                x: f64::from(offset.0 + 899),
                y: f64::from(offset.1 + 699),
            },
            ServerCommand::PresentFrame,
            ServerCommand::ToggleFullscreenFocused,
            ServerCommand::ToggleFullscreenFocused,
        ],
    )
    .unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!((state.toplevel_width, state.toplevel_height), (900, 700));
    assert!(!state.toplevel_has_state(client_xdg_toplevel::State::Fullscreen));
}

#[test]
fn maximize_during_active_resize_restores_visible_pre_transition_geometry() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let offset = render::FIRST_SURFACE_OFFSET;

    let state = create_buffered_toplevel_then_window_commands(
        &socket_path,
        &commands,
        &[
            ServerCommand::BeginResize {
                x: f64::from(offset.0 + 299),
                y: f64::from(offset.1 + 199),
            },
            ServerCommand::UpdateInteraction {
                x: f64::from(offset.0 + 899),
                y: f64::from(offset.1 + 699),
            },
            ServerCommand::PresentFrame,
            ServerCommand::ToggleMaximizeFocused,
            ServerCommand::ToggleMaximizeFocused,
        ],
    )
    .unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!((state.toplevel_width, state.toplevel_height), (900, 700));
    assert!(!state.toplevel_has_state(client_xdg_toplevel::State::Maximized));
}

#[test]
fn normal_fullscreen_restore_without_preview_is_unchanged() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_buffered_toplevel_then_window_commands(
        &socket_path,
        &commands,
        &[
            ServerCommand::ToggleFullscreenFocused,
            ServerCommand::ToggleFullscreenFocused,
        ],
    )
    .unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!((state.toplevel_width, state.toplevel_height), (300, 200));
    assert!(!state.toplevel_has_state(client_xdg_toplevel::State::Fullscreen));
}
