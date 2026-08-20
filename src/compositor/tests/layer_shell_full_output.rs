use super::*;

fn assert_full_output_background_geometry(wallpaper_first: bool) {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let (connection, mut queue, qh, compositor, shm, layer_shell) =
        connect_layer_client(&socket_path);
    let mut state = RegistryTestState::default();
    let all_edges = client_zwlr_layer_surface_v1::Anchor::Top
        | client_zwlr_layer_surface_v1::Anchor::Bottom
        | client_zwlr_layer_surface_v1::Anchor::Left
        | client_zwlr_layer_surface_v1::Anchor::Right;
    let horizontal_top = client_zwlr_layer_surface_v1::Anchor::Top
        | client_zwlr_layer_surface_v1::Anchor::Left
        | client_zwlr_layer_surface_v1::Anchor::Right;
    let horizontal_bottom = client_zwlr_layer_surface_v1::Anchor::Bottom
        | client_zwlr_layer_surface_v1::Anchor::Left
        | client_zwlr_layer_surface_v1::Anchor::Right;

    let mut map = |layer: client_zwlr_layer_shell_v1::Layer,
                   namespace: &str,
                   anchors: client_zwlr_layer_surface_v1::Anchor,
                   width: u32,
                   height: u32,
                   exclusive_zone: i32| {
        let (surface, layer_surface) =
            create_layer_surface(&compositor, &layer_shell, &qh, layer, namespace);
        layer_surface.set_anchor(anchors);
        layer_surface.set_size(width, height);
        layer_surface.set_exclusive_zone(exclusive_zone);
        surface.commit();
        connection.flush().unwrap();
        queue.roundtrip(&mut state).unwrap();
        let buffer_width = if width == 0 { 1280 } else { width };
        let buffer_height = if height == 0 { 800 } else { height };
        commit_test_buffered_surface(
            &surface,
            &shm,
            &qh,
            buffer_width as usize,
            buffer_height as usize,
        )
        .unwrap();
        connection.flush().unwrap();
        queue.roundtrip(&mut state).unwrap();
        (surface, layer_surface)
    };

    let (wallpaper_surface, wallpaper, topbar_surface, topbar, dock_surface, dock) =
        if wallpaper_first {
            let (wallpaper_surface, wallpaper) = map(
                client_zwlr_layer_shell_v1::Layer::Background,
                "paper-wallpaper-first",
                all_edges,
                0,
                0,
                -1,
            );
            let (topbar_surface, topbar) = map(
                client_zwlr_layer_shell_v1::Layer::Top,
                "paper-topbar-after",
                horizontal_top,
                0,
                45,
                45,
            );
            let (dock_surface, dock) = map(
                client_zwlr_layer_shell_v1::Layer::Top,
                "paper-dock-after",
                horizontal_bottom,
                0,
                72,
                72,
            );
            (
                wallpaper_surface,
                wallpaper,
                topbar_surface,
                topbar,
                dock_surface,
                dock,
            )
        } else {
            let (topbar_surface, topbar) = map(
                client_zwlr_layer_shell_v1::Layer::Top,
                "paper-topbar-first",
                horizontal_top,
                0,
                45,
                45,
            );
            let (dock_surface, dock) = map(
                client_zwlr_layer_shell_v1::Layer::Top,
                "paper-dock-second",
                horizontal_bottom,
                0,
                72,
                72,
            );
            let (wallpaper_surface, wallpaper) = map(
                client_zwlr_layer_shell_v1::Layer::Background,
                "paper-wallpaper-last",
                all_edges,
                0,
                0,
                -1,
            );
            (
                wallpaper_surface,
                wallpaper,
                topbar_surface,
                topbar,
                dock_surface,
                dock,
            )
        };

    let initial = capture_renderable_surface_snapshot(&commands);
    let initial_wallpaper = initial
        .iter()
        .find(|surface| surface.width == 1280 && surface.height == 800)
        .expect("zone=-1 background must receive full output geometry");
    assert_eq!(
        (initial_wallpaper.local_x, initial_wallpaper.local_y),
        (0, 0)
    );
    let usable = capture_usable_output_geometry(&commands);
    assert_eq!((usable.x, usable.y), (0.0, 45.0));
    assert_eq!((usable.width, usable.height), (1280.0, 683.0));

    commands
        .send(ServerCommand::SetOutputSize {
            width: 1600,
            height: 900,
        })
        .unwrap();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&wallpaper_surface, &shm, &qh, 1600, 900).unwrap();
    commit_test_buffered_surface(&topbar_surface, &shm, &qh, 1600, 45).unwrap();
    commit_test_buffered_surface(&dock_surface, &shm, &qh, 1600, 72).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let resized = capture_renderable_surface_snapshot(&commands);
    let resized_wallpaper = resized
        .iter()
        .find(|surface| surface.width == 1600 && surface.height == 900)
        .expect("zone=-1 background must resize to full output geometry");
    assert_eq!(
        (resized_wallpaper.local_x, resized_wallpaper.local_y),
        (0, 0)
    );
    let resized_usable = capture_usable_output_geometry(&commands);
    assert_eq!((resized_usable.x, resized_usable.y), (0.0, 45.0));
    assert_eq!(
        (resized_usable.width, resized_usable.height),
        (1600.0, 783.0)
    );

    drop((wallpaper, topbar, dock));
    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn full_output_background_ignores_positive_reservations_in_both_creation_orders() {
    assert_full_output_background_geometry(true);
    assert_full_output_background_geometry(false);
}
