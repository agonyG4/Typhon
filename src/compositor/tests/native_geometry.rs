use super::*;

type Geometry = (i32, i32, u32, u32);

fn two_window_fixture() -> (
    Sender<ServerCommand>,
    JoinHandle<OwnCompositorServer>,
    LiveTestClient,
    LiveTestClient,
) {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let mut first_client = LiveTestClient::connect(&socket_path).unwrap();
    first_client
        .create_toplevel_surface("oblivion.geometry-activation-a", 900, 300)
        .unwrap();
    let mut second_client = LiveTestClient::connect(&socket_path).unwrap();
    second_client
        .create_toplevel_surface("oblivion.geometry-activation-b", 500, 300)
        .unwrap();
    wait_for_server_commands(&commands);
    (commands, server_thread, first_client, second_client)
}

fn root_snapshots(snapshots: &[RenderableSurfaceSnapshot]) -> Vec<RenderableSurfaceSnapshot> {
    snapshots
        .iter()
        .filter(|surface| surface.parent_surface_id.is_none())
        .cloned()
        .collect()
}

fn surface_geometry(snapshots: &[RenderableSurfaceSnapshot], surface_id: u32) -> Geometry {
    let surface = snapshots
        .iter()
        .find(|surface| surface.surface_id == surface_id)
        .expect("surface should remain renderable");
    (
        surface.origin_x,
        surface.origin_y,
        surface.width,
        surface.height,
    )
}

fn point_is_inside(surface: &RenderableSurfaceSnapshot, x: i32, y: i32) -> bool {
    let right = surface.origin_x + i32::try_from(surface.width).expect("test width fits i32");
    let bottom = surface.origin_y + i32::try_from(surface.height).expect("test height fits i32");
    x >= surface.origin_x && x < right && y >= surface.origin_y && y < bottom
}

fn exclusive_hit_point(
    target_surface_id: u32,
    snapshots: &[RenderableSurfaceSnapshot],
) -> (f64, f64) {
    let roots = root_snapshots(snapshots);
    let target_index = roots
        .iter()
        .position(|surface| surface.surface_id == target_surface_id)
        .expect("target root should be renderable");
    let target = &roots[target_index];
    let right = target.origin_x + i32::try_from(target.width).expect("test width fits i32");
    let bottom = target.origin_y + i32::try_from(target.height).expect("test height fits i32");
    let windows_above = &roots[target_index.saturating_add(1)..];

    for y in (target.origin_y + 1..bottom.saturating_sub(1)).step_by(4) {
        for x in (target.origin_x + 1..right.saturating_sub(1)).step_by(4) {
            if point_is_inside(target, x, y)
                && windows_above
                    .iter()
                    .all(|surface| !point_is_inside(surface, x, y))
            {
                return (f64::from(x) + 0.5, f64::from(y) + 0.5);
            }
        }
    }

    panic!("target surface {target_surface_id} has no exclusive point below the current stack");
}

fn rectangles_overlap(left: Geometry, right: Geometry) -> bool {
    let (left_x, left_y, left_width, left_height) = left;
    let (right_x, right_y, right_width, right_height) = right;
    let left_right = i64::from(left_x) + i64::from(left_width);
    let right_right = i64::from(right_x) + i64::from(right_width);
    let left_bottom = i64::from(left_y) + i64::from(left_height);
    let right_bottom = i64::from(right_y) + i64::from(right_height);
    i64::from(left_x) < right_right
        && i64::from(right_x) < left_right
        && i64::from(left_y) < right_bottom
        && i64::from(right_y) < left_bottom
}

fn assert_geometry_unchanged(
    baseline: &[RenderableSurfaceSnapshot],
    current: &[RenderableSurfaceSnapshot],
    first_surface_id: u32,
    second_surface_id: u32,
) {
    assert_eq!(
        surface_geometry(current, first_surface_id),
        surface_geometry(baseline, first_surface_id),
        "first XDG geometry changed"
    );
    assert_eq!(
        surface_geometry(current, second_surface_id),
        surface_geometry(baseline, second_surface_id),
        "second XDG geometry changed"
    );
}

#[test]
fn shell_activation_preserves_xdg_geometry() {
    let (commands, server_thread, _first_client, _second_client) = two_window_fixture();
    let initial = root_snapshots(&capture_renderable_surface_snapshot(&commands));
    let first_surface_id = initial[0].surface_id;
    let second_surface_id = initial[1].surface_id;
    let first_window_id = capture_window_id_for_surface(&commands, first_surface_id)
        .expect("first surface should map to a window");
    let first_geometry = surface_geometry(&initial, first_surface_id);
    let second_geometry = surface_geometry(&initial, second_surface_id);

    commands
        .send(ServerCommand::ActivateRootWindow(first_surface_id))
        .unwrap();
    wait_for_server_commands(&commands);
    let after_activation = root_snapshots(&capture_renderable_surface_snapshot(&commands));

    assert_eq!(
        capture_focused_surface_id(&commands),
        Some(first_surface_id),
        "ShellActivation should focus the exact target"
    );
    assert_eq!(
        capture_focused_window_id(&commands),
        Some(first_window_id),
        "ShellActivation should focus the exact WindowId"
    );
    assert_eq!(
        after_activation.last().map(|surface| surface.surface_id),
        Some(first_surface_id),
        "ShellActivation should raise the exact target"
    );
    assert_eq!(
        surface_geometry(&after_activation, first_surface_id),
        first_geometry
    );
    assert_eq!(
        surface_geometry(&after_activation, second_surface_id),
        second_geometry
    );

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn native_binding_resize_activates_rear_xdg_window_without_changing_geometry() {
    let (commands, server_thread, _first_client, _second_client) = two_window_fixture();
    let baseline = root_snapshots(&capture_renderable_surface_snapshot(&commands));
    let target_surface_id = baseline[0].surface_id;
    let other_surface_id = baseline[1].surface_id;
    let target_window_id = capture_window_id_for_surface(&commands, target_surface_id)
        .expect("target surface should map to a window");
    let (x, y) = exclusive_hit_point(target_surface_id, &baseline);

    assert_eq!(
        baseline.last().map(|surface| surface.surface_id),
        Some(other_surface_id)
    );
    commands.send(ServerCommand::BeginResize { x, y }).unwrap();
    wait_for_server_commands(&commands);

    let after_begin = root_snapshots(&capture_renderable_surface_snapshot(&commands));
    assert_eq!(capture_focused_window_id(&commands), Some(target_window_id));
    assert_eq!(
        after_begin.last().map(|surface| surface.surface_id),
        Some(target_surface_id)
    );
    assert_eq!(
        capture_window_interaction_debug_snapshot(&commands)
            .expect("native resize should begin")
            .window_id,
        target_window_id.get()
    );
    assert_geometry_unchanged(&baseline, &after_begin, target_surface_id, other_surface_id);

    commands
        .send(ServerCommand::UpdateInteraction {
            x: x + 20.0,
            y: y + 20.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    assert_eq!(
        root_snapshots(&capture_renderable_surface_snapshot(&commands))
            .last()
            .map(|surface| surface.surface_id),
        Some(target_surface_id)
    );

    commands.send(ServerCommand::EndInteraction).unwrap();
    wait_for_server_commands(&commands);
    assert_eq!(
        root_snapshots(&capture_renderable_surface_snapshot(&commands))
            .last()
            .map(|surface| surface.surface_id),
        Some(target_surface_id)
    );

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn native_binding_move_activates_rear_xdg_window_before_moving_it() {
    let (commands, server_thread, _first_client, _second_client) = two_window_fixture();
    let baseline = root_snapshots(&capture_renderable_surface_snapshot(&commands));
    let target_surface_id = baseline[0].surface_id;
    let other_surface_id = baseline[1].surface_id;
    let target_window_id = capture_window_id_for_surface(&commands, target_surface_id)
        .expect("target surface should map to a window");
    let (x, y) = exclusive_hit_point(target_surface_id, &baseline);

    commands.send(ServerCommand::BeginMove { x, y }).unwrap();
    wait_for_server_commands(&commands);

    let after_begin = root_snapshots(&capture_renderable_surface_snapshot(&commands));
    assert_eq!(capture_focused_window_id(&commands), Some(target_window_id));
    assert_eq!(
        after_begin.last().map(|surface| surface.surface_id),
        Some(target_surface_id)
    );
    assert_eq!(
        capture_window_interaction_debug_snapshot(&commands)
            .expect("native move should begin")
            .window_id,
        target_window_id.get()
    );
    assert_geometry_unchanged(&baseline, &after_begin, target_surface_id, other_surface_id);

    commands
        .send(ServerCommand::UpdateInteraction {
            x: x + 40.0,
            y: y + 25.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    let after_update = root_snapshots(&capture_renderable_surface_snapshot(&commands));
    assert_ne!(
        surface_geometry(&after_update, target_surface_id),
        surface_geometry(&baseline, target_surface_id),
        "native move should move the exact target"
    );
    assert_eq!(
        surface_geometry(&after_update, other_surface_id),
        surface_geometry(&baseline, other_surface_id),
        "native move should not move the other window"
    );
    assert_eq!(
        after_update.last().map(|surface| surface.surface_id),
        Some(target_surface_id)
    );

    commands.send(ServerCommand::EndInteraction).unwrap();
    wait_for_server_commands(&commands);
    assert_eq!(
        root_snapshots(&capture_renderable_surface_snapshot(&commands))
            .last()
            .map(|surface| surface.surface_id),
        Some(target_surface_id)
    );

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn native_binding_on_topmost_window_does_not_duplicate_stack_or_restack() {
    let (commands, server_thread, _first_client, _second_client) = two_window_fixture();
    let baseline = root_snapshots(&capture_renderable_surface_snapshot(&commands));
    let target_surface_id = baseline[1].surface_id;
    let target_window_id = capture_window_id_for_surface(&commands, target_surface_id)
        .expect("target surface should map to a window");
    let (x, y) = exclusive_hit_point(target_surface_id, &baseline);

    let before_resize_generation = capture_render_generation(&commands);
    commands.send(ServerCommand::BeginResize { x, y }).unwrap();
    wait_for_server_commands(&commands);
    let after_resize = root_snapshots(&capture_renderable_surface_snapshot(&commands));
    assert_eq!(after_resize.len(), baseline.len());
    assert_eq!(
        after_resize.last().map(|surface| surface.surface_id),
        Some(target_surface_id)
    );
    assert_eq!(capture_focused_window_id(&commands), Some(target_window_id));
    assert_eq!(
        capture_window_interaction_debug_snapshot(&commands)
            .expect("native resize should begin")
            .window_id,
        target_window_id.get()
    );
    assert_eq!(
        capture_render_generation(&commands),
        before_resize_generation + 1,
        "already-topmost native resize should only install interaction cursor state"
    );
    commands.send(ServerCommand::EndInteraction).unwrap();
    wait_for_server_commands(&commands);

    let before_move_generation = capture_render_generation(&commands);
    commands.send(ServerCommand::BeginMove { x, y }).unwrap();
    wait_for_server_commands(&commands);
    let after_move = root_snapshots(&capture_renderable_surface_snapshot(&commands));
    assert_eq!(after_move.len(), baseline.len());
    assert_eq!(
        after_move.last().map(|surface| surface.surface_id),
        Some(target_surface_id)
    );
    assert_eq!(capture_focused_window_id(&commands), Some(target_window_id));
    assert_eq!(
        capture_render_generation(&commands),
        before_move_generation + 1,
        "already-topmost native move should only install interaction cursor state"
    );
    commands.send(ServerCommand::EndInteraction).unwrap();
    wait_for_server_commands(&commands);

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn rejected_native_interaction_does_not_activate_or_raise_a_window() {
    let (commands, server_thread, _first_client, _second_client) = two_window_fixture();
    let baseline = root_snapshots(&capture_renderable_surface_snapshot(&commands));
    let focused_before = capture_focused_window_id(&commands);
    let generation_before = capture_render_generation(&commands);

    commands
        .send(ServerCommand::BeginResize { x: 1.0, y: 1.0 })
        .unwrap();
    wait_for_server_commands(&commands);

    assert_eq!(capture_focused_window_id(&commands), focused_before);
    assert_eq!(
        root_snapshots(&capture_renderable_surface_snapshot(&commands)),
        baseline,
        "rejected native interaction must not change stacking"
    );
    assert_eq!(capture_render_generation(&commands), generation_before);
    assert_eq!(capture_window_interaction_debug_snapshot(&commands), None);

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn pointer_press_hits_exact_background_window_and_preserves_xdg_geometry() {
    let (commands, server_thread, _first_client, _second_client) = two_window_fixture();
    let baseline = root_snapshots(&capture_renderable_surface_snapshot(&commands));
    let first_surface_id = baseline[0].surface_id;
    let second_surface_id = baseline[1].surface_id;
    let first_window_id = capture_window_id_for_surface(&commands, first_surface_id)
        .expect("first surface should map to a window");
    let second_window_id = capture_window_id_for_surface(&commands, second_surface_id)
        .expect("second surface should map to a window");
    assert!(
        rectangles_overlap(
            surface_geometry(&baseline, first_surface_id),
            surface_geometry(&baseline, second_surface_id),
        ),
        "the fixture should retain overlap while exposing click regions"
    );

    for index in 0..100 {
        let expected_surface_id = if index % 2 == 0 {
            first_surface_id
        } else {
            second_surface_id
        };
        let expected_window_id = if expected_surface_id == first_surface_id {
            first_window_id
        } else {
            second_window_id
        };
        let before_click = root_snapshots(&capture_renderable_surface_snapshot(&commands));
        let (x, y) = exclusive_hit_point(expected_surface_id, &before_click);

        commands
            .send(ServerCommand::PointerMotion { x, y })
            .unwrap();
        wait_for_server_commands(&commands);
        commands
            .send(ServerCommand::PointerButton {
                button: 0x110,
                pressed: true,
            })
            .unwrap();
        wait_for_server_commands(&commands);

        assert_eq!(
            capture_focused_surface_id(&commands),
            Some(expected_surface_id),
            "PointerPress should focus the exact target at cycle {index}"
        );
        assert_eq!(
            capture_focused_window_id(&commands),
            Some(expected_window_id),
            "PointerPress should focus the exact WindowId at cycle {index}"
        );
        let after_press = root_snapshots(&capture_renderable_surface_snapshot(&commands));
        assert_eq!(
            after_press.last().map(|surface| surface.surface_id),
            Some(expected_surface_id),
            "PointerPress should raise the exact target at cycle {index}"
        );
        assert_geometry_unchanged(&baseline, &after_press, first_surface_id, second_surface_id);

        commands
            .send(ServerCommand::PointerButton {
                button: 0x110,
                pressed: false,
            })
            .unwrap();
        wait_for_server_commands(&commands);
        let after_release = root_snapshots(&capture_renderable_surface_snapshot(&commands));
        assert_geometry_unchanged(
            &baseline,
            &after_release,
            first_surface_id,
            second_surface_id,
        );
    }

    let _server = stop_controllable_test_server(commands, server_thread);
}
