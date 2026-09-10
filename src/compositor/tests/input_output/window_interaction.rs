use super::*;
use crate::wm::LayoutMembership;
use wayland_protocols::xdg::decoration::zv1::client::{
    zxdg_decoration_manager_v1 as client_zxdg_decoration_manager_v1,
    zxdg_toplevel_decoration_v1 as client_zxdg_toplevel_decoration_v1,
};

fn map_server_decorated_toplevel(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<client_zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1, Box<dyn std::error::Error>>
{
    let connection = Connection::from_socket(UnixStream::connect(socket_path)?)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=2, ())?;
    let manager: client_zxdg_decoration_manager_v1::ZxdgDecorationManagerV1 =
        globals.bind(&qh, 1..=1, ())?;
    let (surface, xdg_surface, toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 300, 200)?;
    let decoration = manager.get_toplevel_decoration(&toplevel, &qh, ());
    decoration.set_mode(client_zxdg_toplevel_decoration_v1::Mode::ServerSide);
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commit_registered_initial_xdg_test_buffer(&xdg_surface);
    connection.flush()?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    retain_live_test_connection(connection);
    Ok(decoration)
}

#[test]
fn overlapping_server_decoration_does_not_focus_window_underneath() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection_a = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals_a, mut queue_a) = registry_queue_init::<RegistryTestState>(&connection_a).unwrap();
    let qh_a = queue_a.handle();
    let compositor_a: client_wl_compositor::WlCompositor =
        globals_a.bind(&qh_a, 1..=6, ()).unwrap();
    let wm_base_a: client_xdg_wm_base::XdgWmBase = globals_a.bind(&qh_a, 1..=6, ()).unwrap();
    let shm_a: client_wl_shm::WlShm = globals_a.bind(&qh_a, 1..=2, ()).unwrap();
    let manager_a: client_zxdg_decoration_manager_v1::ZxdgDecorationManagerV1 =
        globals_a.bind(&qh_a, 1..=1, ()).unwrap();
    let seat_a: client_wl_seat::WlSeat = globals_a.bind(&qh_a, 1..=7, ()).unwrap();
    let _keyboard_a = seat_a.get_keyboard(&qh_a, ());
    let _pointer_a = seat_a.get_pointer(&qh_a, ());
    let (surface_a, xdg_surface_a, toplevel_a) =
        create_test_buffered_toplevel(&compositor_a, &wm_base_a, &shm_a, &qh_a, 160, 120).unwrap();
    let decoration_a = manager_a.get_toplevel_decoration(&toplevel_a, &qh_a, ());
    decoration_a.set_mode(client_zxdg_toplevel_decoration_v1::Mode::ServerSide);
    surface_a.commit();
    connection_a.flush().unwrap();

    let connection_b = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals_b, mut queue_b) = registry_queue_init::<RegistryTestState>(&connection_b).unwrap();
    let qh_b = queue_b.handle();
    let compositor_b: client_wl_compositor::WlCompositor =
        globals_b.bind(&qh_b, 1..=6, ()).unwrap();
    let wm_base_b: client_xdg_wm_base::XdgWmBase = globals_b.bind(&qh_b, 1..=6, ()).unwrap();
    let shm_b: client_wl_shm::WlShm = globals_b.bind(&qh_b, 1..=2, ()).unwrap();
    let manager_b: client_zxdg_decoration_manager_v1::ZxdgDecorationManagerV1 =
        globals_b.bind(&qh_b, 1..=1, ()).unwrap();
    let seat_b: client_wl_seat::WlSeat = globals_b.bind(&qh_b, 1..=7, ()).unwrap();
    let _keyboard_b = seat_b.get_keyboard(&qh_b, ());
    let _pointer_b = seat_b.get_pointer(&qh_b, ());
    let (surface_b, xdg_surface_b, toplevel_b) =
        create_test_buffered_toplevel(&compositor_b, &wm_base_b, &shm_b, &qh_b, 160, 120).unwrap();
    let decoration_b = manager_b.get_toplevel_decoration(&toplevel_b, &qh_b, ());
    decoration_b.set_mode(client_zxdg_toplevel_decoration_v1::Mode::ServerSide);
    surface_b.commit();
    connection_b.flush().unwrap();

    let mut state_a = RegistryTestState::default();
    let mut state_b = RegistryTestState::default();
    queue_a.roundtrip(&mut state_a).unwrap();
    queue_b.roundtrip(&mut state_b).unwrap();
    commit_registered_initial_xdg_test_buffer(&xdg_surface_a);
    commit_registered_initial_xdg_test_buffer(&xdg_surface_b);
    connection_a.flush().unwrap();
    connection_b.flush().unwrap();
    queue_a.roundtrip(&mut state_a).unwrap();
    queue_b.roundtrip(&mut state_b).unwrap();

    wait_for_server_commands(&commands);
    let initial_surfaces = capture_renderable_surface_snapshot(&commands);
    assert_eq!(
        initial_surfaces.len(),
        2,
        "both test windows must be mapped"
    );
    let surface_a_id = initial_surfaces[0].surface_id;
    let surface_b_id = initial_surfaces[1].surface_id;
    focus_root_window(&commands, surface_b_id);
    set_focused_root_visual_geometry(
        &commands,
        SurfacePlacement::absolute_root_at(100, 70),
        160,
        120,
    );
    focus_root_window(&commands, surface_a_id);
    set_focused_root_visual_geometry(
        &commands,
        SurfacePlacement::absolute_root_at(100, 100),
        160,
        120,
    );
    raise_root_window(&commands, surface_a_id);
    focus_root_window(&commands, surface_a_id);

    let surfaces = capture_renderable_surface_snapshot(&commands);
    let surface_a_snapshot = surfaces
        .iter()
        .find(|surface| surface.surface_id == surface_a_id)
        .expect("A renderable surface");
    let a_window_id = capture_window_id_for_surface(&commands, surface_a_id).expect("A window id");
    let b_window_id = capture_window_id_for_surface(&commands, surface_b_id).expect("B window id");
    assert_ne!(a_window_id, b_window_id);

    let client_x = f64::from(surface_a_snapshot.origin_x + 80);
    let client_y = f64::from(surface_a_snapshot.origin_y + 30);
    commands
        .send(ServerCommand::PointerMotion {
            x: client_x,
            y: client_y,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue_a.roundtrip(&mut state_a).unwrap();
    queue_b.roundtrip(&mut state_b).unwrap();
    assert_eq!(capture_focused_window_id(&commands), Some(a_window_id));
    let scene_generation_before_input_region = capture_scene_render_generation(&commands);
    let titlebar_x = f64::from(surface_a_snapshot.origin_x + 20);
    let titlebar_y = f64::from(surface_a_snapshot.origin_y - 13);

    let empty_input_region = compositor_a.create_region(&qh_a, ());
    surface_a.set_input_region(Some(&empty_input_region));
    surface_a.commit();
    connection_a.flush().unwrap();
    wait_for_server_commands(&commands);
    queue_a.roundtrip(&mut state_a).unwrap();
    queue_b.roundtrip(&mut state_b).unwrap();
    assert_eq!(
        capture_scene_render_generation(&commands),
        scene_generation_before_input_region,
        "input-region-only commit must not be mistaken for a render generation change"
    );
    assert_eq!(
        capture_pointer_focus_surface_id(&commands),
        Some(surface_b_id),
        "stationary input-region removal must resolve the lower client"
    );

    commands
        .send(ServerCommand::PointerMotion {
            x: titlebar_x,
            y: titlebar_y,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue_a.roundtrip(&mut state_a).unwrap();
    queue_b.roundtrip(&mut state_b).unwrap();
    assert_eq!(capture_pointer_focus_surface_id(&commands), None);
    assert_eq!(capture_focused_window_id(&commands), Some(a_window_id));

    commands
        .send(ServerCommand::PointerMotion {
            x: client_x,
            y: client_y,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue_a.roundtrip(&mut state_a).unwrap();
    queue_b.roundtrip(&mut state_b).unwrap();

    let included_input_region = compositor_a.create_region(&qh_a, ());
    included_input_region.add(0, 0, 160, 120);
    surface_a.set_input_region(Some(&included_input_region));
    surface_a.commit();
    connection_a.flush().unwrap();
    wait_for_server_commands(&commands);
    queue_a.roundtrip(&mut state_a).unwrap();
    queue_b.roundtrip(&mut state_b).unwrap();
    assert_eq!(
        capture_pointer_focus_surface_id(&commands),
        Some(surface_a_id),
        "stationary input-region inclusion must resolve the front client"
    );
    let b_enters_before_titlebar = state_b.pointer_enter_count;

    commands
        .send(ServerCommand::PointerMotion {
            x: titlebar_x,
            y: titlebar_y,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue_a.roundtrip(&mut state_a).unwrap();
    queue_b.roundtrip(&mut state_b).unwrap();

    assert_eq!(capture_focused_window_id(&commands), Some(a_window_id));
    assert_eq!(capture_pointer_focus_surface_id(&commands), None);
    assert_eq!(
        state_b.pointer_enter_count, b_enters_before_titlebar,
        "B must not receive titlebar enter"
    );

    set_pointer_hit_instrumentation_enabled(&commands, true);
    state_a.pointer_event_log.clear();
    state_b.pointer_event_log.clear();
    let focus_generation_before_stress = capture_focus_generation(&commands);
    let b_enters_before_stress = state_b.pointer_enter_count;
    let b_leaves_before_stress = state_b.pointer_leave_count;
    let mut move_pointer = |x: f64, y: f64| {
        commands
            .send(ServerCommand::PointerMotion { x, y })
            .unwrap();
        wait_for_server_commands(&commands);
        queue_a.roundtrip(&mut state_a).unwrap();
        queue_b.roundtrip(&mut state_b).unwrap();
    };
    let button_x = f64::from(surface_a_snapshot.origin_x + 145);
    for _ in 0..1_000 {
        move_pointer(client_x, client_y);
        move_pointer(titlebar_x, titlebar_y);
        move_pointer(button_x, titlebar_y);
        move_pointer(client_x, client_y);
    }
    let pointer_crossings = state_a
        .pointer_event_log
        .iter()
        .copied()
        .filter(|event| *event == "enter" || *event == "leave")
        .collect::<Vec<_>>();
    assert_eq!(pointer_crossings.first().copied(), Some("enter"));
    assert_eq!(pointer_crossings.get(1).copied(), Some("leave"));
    assert_eq!(pointer_crossings.get(2).copied(), Some("enter"));
    assert_eq!(
        pointer_crossings
            .iter()
            .filter(|event| **event == "leave")
            .count(),
        1_000
    );
    assert_eq!(
        pointer_crossings
            .iter()
            .filter(|event| **event == "enter")
            .count(),
        1_001
    );
    assert_eq!(state_b.pointer_enter_count, b_enters_before_stress);
    assert_eq!(state_b.pointer_leave_count, b_leaves_before_stress);
    assert_eq!(capture_focused_window_id(&commands), Some(a_window_id));
    assert_eq!(
        capture_keyboard_focus_surface_id(&commands),
        Some(surface_a_id)
    );
    assert_eq!(
        capture_focus_generation(&commands),
        focus_generation_before_stress,
        "client/SSD crossings must not churn desktop focus"
    );
    let pointer_metrics = capture_pointer_input_metrics(&commands);
    assert!(pointer_metrics.pointer_scene_hit_calls >= 4_000);
    assert_eq!(pointer_metrics.pointer_scene_hit_origin_cache_clones, 0);
    assert_eq!(pointer_metrics.pointer_scene_hit_root_linear_searches, 0);
    assert!(pointer_metrics.desktop_focus_pipeline_invocations > 0);
    assert!(pointer_metrics.desktop_focus_same_window_noops > 0);

    commands
        .send(ServerCommand::BeginResize {
            x: f64::from(surface_a_snapshot.origin_x + 20),
            y: f64::from(surface_a_snapshot.origin_y - 24),
        })
        .unwrap();
    wait_for_server_commands(&commands);
    let resize_owner = capture_window_interaction_debug_snapshot(&commands)
        .expect("overlapping resize margin must start an interaction");
    assert_eq!(resize_owner.window_id, a_window_id.get());
    assert_eq!(resize_owner.root_surface_id, surface_a_id);
    commands.send(ServerCommand::EndInteraction).unwrap();
    wait_for_server_commands(&commands);

    let before_drag = capture_renderable_surface_snapshot(&commands);
    let before_a_x = before_drag
        .iter()
        .find(|surface| surface.surface_id == surface_a_id)
        .expect("A renderable surface before drag")
        .origin_x;
    let before_b_x = before_drag
        .iter()
        .find(|surface| surface.surface_id == surface_b_id)
        .expect("B renderable surface before drag")
        .origin_x;
    commands
        .send(ServerCommand::BeginMove {
            x: titlebar_x,
            y: titlebar_y,
        })
        .unwrap();
    commands
        .send(ServerCommand::UpdateInteraction {
            x: titlebar_x + 20.0,
            y: titlebar_y,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    commands.send(ServerCommand::PresentFrame).unwrap();
    wait_for_server_commands(&commands);
    let drag_owner = capture_window_interaction_debug_snapshot(&commands)
        .expect("titlebar drag must remain captured");
    assert_eq!(drag_owner.window_id, a_window_id.get());
    assert_eq!(drag_owner.root_surface_id, surface_a_id);
    let after_drag = capture_renderable_surface_snapshot(&commands);
    let after_a_x = after_drag
        .iter()
        .find(|surface| surface.surface_id == surface_a_id)
        .expect("A renderable surface after drag")
        .origin_x;
    let after_b_x = after_drag
        .iter()
        .find(|surface| surface.surface_id == surface_b_id)
        .expect("B renderable surface after drag")
        .origin_x;
    assert_ne!(after_a_x, before_a_x, "A must move from its titlebar");
    assert_eq!(after_b_x, before_b_x, "B must not move under A's titlebar");
    commands.send(ServerCommand::EndInteraction).unwrap();
    wait_for_server_commands(&commands);

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    let _ = decoration_a;
    let _ = decoration_b;
}

#[test]
fn titlebar_drag_interrupts_scaled_presentation_without_baking_scaled_size() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let _decoration = map_server_decorated_toplevel(&socket_path, &commands).unwrap();

    set_focused_root_visual_geometry(
        &commands,
        SurfacePlacement::absolute_root_at(640, 320),
        800,
        600,
    );
    commands.send(ServerCommand::PresentFrame).unwrap();
    wait_for_server_commands(&commands);
    let root_surface_id = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .find(|surface| surface.parent_surface_id.is_none())
        .expect("decorated root should be renderable")
        .surface_id;

    commands.send(ServerCommand::ToggleMaximizeFocused).unwrap();
    wait_for_server_commands(&commands);
    commands
        .send(ServerCommand::PublishFocusedPresentationAfter {
            frame_id: 2,
            elapsed_nanos: 125_000_000,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    let physical = capture_focused_presentation_after(&commands, 125_000_000)
        .expect("maximize presentation should still be active midway");
    assert_ne!(physical.rect.width(), 800.0);
    assert_ne!(physical.rect.height(), 600.0);

    let titlebar_x = physical.rect.x() + 20.0;
    let titlebar_y = physical.rect.y() - 13.0;
    commands
        .send(ServerCommand::PointerMotion {
            x: titlebar_x,
            y: titlebar_y,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    commands
        .send(ServerCommand::BeginMove {
            x: titlebar_x,
            y: titlebar_y,
        })
        .unwrap();
    wait_for_server_commands(&commands);

    let interaction = capture_window_interaction_debug_snapshot(&commands)
        .expect("transformed titlebar must start a move interaction");
    let visual =
        capture_root_window_geometry(&commands, root_surface_id).expect("restored visual geometry");
    let transition_curve = capture_presentation_transition_curve(&commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(interaction.root_surface_id, root_surface_id);
    assert_eq!(interaction.kind, WindowInteractionKind::Move);
    assert!(interaction.decoration_owned);
    assert_eq!(visual.width, 800);
    assert_eq!(visual.height, 600);
    assert_eq!(transition_curve, None);
}

#[test]
fn titlebar_move_interrupts_size_presentation_without_baking_scaled_geometry() {
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
    let manager: client_zxdg_decoration_manager_v1::ZxdgDecorationManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let (surface, xdg_surface, toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 300, 200).unwrap();
    let decoration = manager.get_toplevel_decoration(&toplevel, &qh, ());
    decoration.set_mode(client_zxdg_toplevel_decoration_v1::Mode::ServerSide);
    surface.commit();
    connection.flush().unwrap();

    let mut client_state = RegistryTestState::default();
    queue.roundtrip(&mut client_state).unwrap();
    commit_registered_initial_xdg_test_buffer(&xdg_surface);
    connection.flush().unwrap();
    queue.roundtrip(&mut client_state).unwrap();
    wait_for_server_commands(&commands);

    let root_surface_id = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .find(|surface| surface.parent_surface_id.is_none())
        .expect("decorated root should be renderable")
        .surface_id;
    focus_root_window(&commands, root_surface_id);
    set_focused_root_visual_geometry(
        &commands,
        SurfacePlacement::absolute_root_at(100, 100),
        300,
        200,
    );
    commands
        .send(ServerCommand::ResizeFocusedTo {
            width: 500,
            height: 350,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    let before = capture_toplevel_visual_geometry(&commands).expect("resized visual geometry");
    assert_eq!((before.width, before.height), (500, 350));
    assert!(capture_presentation_transition_curve(&commands).is_some());

    commands
        .send(ServerCommand::BeginMove { x: 120.0, y: 87.0 })
        .unwrap();
    wait_for_server_commands(&commands);
    let interaction = capture_window_interaction_debug_snapshot(&commands)
        .expect("titlebar move should begin during presentation");
    let after = capture_toplevel_visual_geometry(&commands).expect("canonical visual geometry");
    let transition_after_interaction = capture_presentation_transition_curve(&commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(interaction.kind, WindowInteractionKind::Move);
    assert_eq!((after.width, after.height), (500, 350));
    assert_eq!(after.local_y, 100);
    assert!(transition_after_interaction.is_none());
    drop(decoration);
}

#[test]
fn maximized_titlebar_move_restores_normal_window_under_pointer() {
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
    let manager: client_zxdg_decoration_manager_v1::ZxdgDecorationManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let (surface, xdg_surface, toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 300, 200).unwrap();
    let decoration = manager.get_toplevel_decoration(&toplevel, &qh, ());
    decoration.set_mode(client_zxdg_toplevel_decoration_v1::Mode::ServerSide);
    surface.commit();
    connection.flush().unwrap();

    let mut client_state = RegistryTestState::default();
    queue.roundtrip(&mut client_state).unwrap();
    commit_registered_initial_xdg_test_buffer(&xdg_surface);
    connection.flush().unwrap();
    queue.roundtrip(&mut client_state).unwrap();
    wait_for_server_commands(&commands);

    let root_surface_id = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .find(|surface| surface.parent_surface_id.is_none())
        .expect("decorated root should be renderable")
        .surface_id;
    focus_root_window(&commands, root_surface_id);
    set_focused_root_visual_geometry(
        &commands,
        SurfacePlacement::absolute_root_at(100, 100),
        300,
        200,
    );
    commands.send(ServerCommand::ToggleMaximizeFocused).unwrap();
    wait_for_server_commands(&commands);
    assert_eq!(
        capture_focused_toplevel_mode(&commands),
        Some(ToplevelMode::Maximized)
    );
    commands.send(ServerCommand::PresentFrame).unwrap();
    wait_for_server_commands(&commands);
    let maximized_visual = capture_toplevel_visual_geometry(&commands);
    assert!(
        maximized_visual.is_some(),
        "maximized visual should remain installed"
    );
    commands
        .send(ServerCommand::PublishFocusedPresentationAfter {
            frame_id: 2,
            elapsed_nanos: 125_000_000,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    let presented_before = capture_presented_presentation(&commands, root_surface_id);
    assert_eq!(presented_before.0, 2);
    assert!(presented_before.1.is_some());

    commands
        .send(ServerCommand::BeginMove { x: 120.0, y: 87.0 })
        .unwrap();
    wait_for_server_commands(&commands);
    let mode = capture_focused_toplevel_mode(&commands);
    let interaction = capture_window_interaction_debug_snapshot(&commands);
    let committed = capture_root_window_geometry(&commands, root_surface_id);
    let transition = capture_presentation_transition_curve(&commands);
    let presented_after_begin = capture_presented_presentation(&commands, root_surface_id);
    assert_eq!(presented_after_begin, presented_before);
    commands
        .send(ServerCommand::UpdateInteraction { x: 140.0, y: 87.0 })
        .unwrap();
    commands.send(ServerCommand::PresentFrame).unwrap();
    wait_for_server_commands(&commands);
    let moved = capture_root_window_geometry(&commands, root_surface_id);
    let presented_after_frame = capture_presented_presentation(&commands, root_surface_id);
    commands.send(ServerCommand::EndInteraction).unwrap();
    wait_for_server_commands(&commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(mode, Some(ToplevelMode::Normal));
    assert_eq!(
        interaction.map(|interaction| interaction.kind),
        Some(WindowInteractionKind::Move)
    );
    assert_eq!(
        committed.map(|geometry| (geometry.width, geometry.height)),
        Some((300, 200))
    );
    assert_eq!(
        moved.map(|geometry| geometry.placement.local_x),
        committed.map(|geometry| geometry.placement.local_x.saturating_add(20))
    );
    assert!(transition.is_none());
    assert!(presented_after_frame.0 > presented_before.0);
    assert_ne!(presented_after_frame.1, presented_before.1);
    drop(decoration);
}

#[test]
fn tiled_maximized_move_detaches_before_direct_pointer_ownership() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let mut client = LiveTestClient::connect(&socket_path).unwrap();
    let _surface_a = client
        .create_toplevel_surface("oblivion.tiled-maximized-move-a", 300, 200)
        .unwrap();
    let _surface_b = client
        .create_toplevel_surface("oblivion.tiled-maximized-move-b", 300, 200)
        .unwrap();
    wait_for_server_commands(&commands);

    let roots = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .filter(|surface| surface.parent_surface_id.is_none())
        .map(|surface| surface.surface_id)
        .collect::<Vec<_>>();
    assert_eq!(roots.len(), 2);
    focus_root_window(&commands, roots[0]);
    let window_a = capture_focused_window_id(&commands).expect("window A should be focused");
    focus_root_window(&commands, roots[1]);
    let window_b = capture_focused_window_id(&commands).expect("window B should be focused");
    assert_ne!(window_a, window_b);
    let root_a = roots[0];
    let root_b = roots[1];

    focus_root_window(&commands, root_a);
    set_focused_root_visual_geometry(
        &commands,
        SurfacePlacement::absolute_root_at(220, 140),
        700,
        450,
    );
    commands
        .send(ServerCommand::ToggleFocusedWindowLayout)
        .unwrap();
    wait_for_server_commands(&commands);
    focus_root_window(&commands, root_b);
    commands
        .send(ServerCommand::ToggleFocusedWindowLayout)
        .unwrap();
    wait_for_server_commands(&commands);

    focus_root_window(&commands, root_a);
    let tiled_geometry = capture_root_window_geometry(&commands, root_a)
        .expect("tiled geometry should be installed before maximize");
    commands.send(ServerCommand::ToggleMaximizeFocused).unwrap();
    wait_for_server_commands(&commands);
    let maximized_geometry = capture_root_window_geometry(&commands, root_a)
        .expect("maximized geometry should be installed");
    let pointer_x =
        maximized_geometry.placement.local_x as f64 + maximized_geometry.width as f64 * 0.4;
    let pointer_y = maximized_geometry.placement.local_y as f64 + 120.0;
    commands
        .send(ServerCommand::BeginMove {
            x: pointer_x,
            y: pointer_y,
        })
        .unwrap();
    wait_for_server_commands(&commands);

    let interaction = capture_window_interaction_debug_snapshot(&commands);
    let management_a = capture_window_management(&commands, window_a);
    let management_b = capture_window_management(&commands, window_b);
    let restored_geometry = capture_root_window_geometry(&commands, root_a);
    let survivor_transition = capture_presentation_transition_curve_for_root(&commands, root_b);
    commands.send(ServerCommand::EndInteraction).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(
        interaction.map(|interaction| (interaction.window_id, interaction.kind)),
        Some((window_a.get(), WindowInteractionKind::Move))
    );
    assert_eq!(
        management_a.map(|management| management.0),
        Some(LayoutMembership::Floating)
    );
    assert_eq!(management_a.map(|management| management.2), Some(false));
    assert_eq!(
        management_b.map(|management| management.0),
        Some(LayoutMembership::Tiled)
    );
    assert_eq!(management_b.map(|management| management.2), Some(true));
    assert_eq!(
        restored_geometry.map(|geometry| (geometry.width, geometry.height)),
        Some((700, 450))
    );
    assert_ne!((tiled_geometry.width, tiled_geometry.height), (700, 450));
    assert!(survivor_transition.is_some());
    let expected_x = pointer_x - 700.0 * 0.4;
    let actual_x = restored_geometry
        .expect("restored geometry")
        .placement
        .local_x as f64;
    assert!((actual_x - expected_x).abs() <= 1.0);
}

#[test]
fn maximized_resize_is_rejected_by_the_real_pointer_entry_path() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let _decoration = map_server_decorated_toplevel(&socket_path, &commands).unwrap();

    set_focused_root_visual_geometry(
        &commands,
        SurfacePlacement::absolute_root_at(100, 100),
        300,
        200,
    );
    commands.send(ServerCommand::ToggleMaximizeFocused).unwrap();
    wait_for_server_commands(&commands);
    commands.send(ServerCommand::PresentFrame).unwrap();
    wait_for_server_commands(&commands);
    commands
        .send(ServerCommand::BeginResize {
            x: 1278.0,
            y: 798.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    let interaction = capture_window_interaction_debug_snapshot(&commands);
    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    assert!(interaction.is_none(), "maximized resize must be rejected");
}

#[test]
fn window_interaction_absolute_motion_targets_only_original_surface() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_with_input_capabilities(
        &socket_name,
        InputProtocolCapabilities {
            relative_pointer: true,
            ..InputProtocolCapabilities::desktop_baseline()
        },
    )
    .unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection_a = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals_a, mut queue_a) = registry_queue_init::<RegistryTestState>(&connection_a).unwrap();
    let qh_a = queue_a.handle();
    let compositor_a: client_wl_compositor::WlCompositor =
        globals_a.bind(&qh_a, 1..=6, ()).unwrap();
    let wm_base_a: client_xdg_wm_base::XdgWmBase = globals_a.bind(&qh_a, 1..=6, ()).unwrap();
    let shm_a: client_wl_shm::WlShm = globals_a.bind(&qh_a, 1..=2, ()).unwrap();
    let seat_a: client_wl_seat::WlSeat = globals_a.bind(&qh_a, 1..=7, ()).unwrap();
    let pointer_a = seat_a.get_pointer(&qh_a, ());
    let _relative_a = globals_a
        .bind::<client_zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1, _, _>(
            &qh_a,
            1..=1,
            (),
        )
        .unwrap()
        .get_relative_pointer(&pointer_a, &qh_a, ());
    let (surface_a, _xdg_surface_a, _toplevel_a) =
        create_test_buffered_toplevel(&compositor_a, &wm_base_a, &shm_a, &qh_a, 160, 120).unwrap();
    surface_a.commit();
    connection_a.flush().unwrap();

    let connection_b = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals_b, mut queue_b) = registry_queue_init::<RegistryTestState>(&connection_b).unwrap();
    let qh_b = queue_b.handle();
    let compositor_b: client_wl_compositor::WlCompositor =
        globals_b.bind(&qh_b, 1..=6, ()).unwrap();
    let wm_base_b: client_xdg_wm_base::XdgWmBase = globals_b.bind(&qh_b, 1..=6, ()).unwrap();
    let shm_b: client_wl_shm::WlShm = globals_b.bind(&qh_b, 1..=2, ()).unwrap();
    let seat_b: client_wl_seat::WlSeat = globals_b.bind(&qh_b, 1..=7, ()).unwrap();
    let pointer_b = seat_b.get_pointer(&qh_b, ());
    let (surface_b, _xdg_surface_b, _toplevel_b) =
        create_test_buffered_toplevel(&compositor_b, &wm_base_b, &shm_b, &qh_b, 160, 120).unwrap();
    surface_b.commit();
    connection_b.flush().unwrap();

    let mut state_a = RegistryTestState::default();
    let mut state_b = RegistryTestState::default();
    queue_a.roundtrip(&mut state_a).unwrap();
    queue_b.roundtrip(&mut state_b).unwrap();

    let start_x = f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0;
    let start_y = f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0;
    let renderable_surfaces = capture_renderable_surface_snapshot(&commands);
    let surface_id_at = |x: f64, y: f64| {
        renderable_surfaces
            .iter()
            .rev()
            .find(|surface| {
                x >= f64::from(surface.origin_x)
                    && x < f64::from(surface.origin_x + surface.width as i32)
                    && y >= f64::from(surface.origin_y)
                    && y < f64::from(surface.origin_y + surface.height as i32)
            })
            .map(|surface| surface.surface_id)
    };
    let surface_a_id = surface_id_at(start_x, start_y).expect("A must cover start point");
    let surface_b_id = surface_id_at(140.0, 140.0).expect("B must cover crossing point");
    commands
        .send(ServerCommand::PointerMotion {
            x: start_x,
            y: start_y,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue_a.roundtrip(&mut state_a).unwrap();
    queue_b.roundtrip(&mut state_b).unwrap();
    assert_eq!(state_a.pointer_enter_count, 1);
    assert_eq!(state_b.pointer_enter_count, 0);
    assert_eq!(
        capture_pointer_focus_surface_id(&commands),
        Some(surface_a_id)
    );
    let keyboard_focus_at_a = capture_focused_surface_id(&commands);
    commands
        .send(ServerCommand::PointerMotion { x: 140.0, y: 140.0 })
        .unwrap();
    wait_for_server_commands(&commands);
    queue_a.roundtrip(&mut state_a).unwrap();
    queue_b.roundtrip(&mut state_b).unwrap();
    assert_eq!(state_b.pointer_enter_count, 1);
    assert_eq!(
        capture_pointer_focus_surface_id(&commands),
        Some(surface_b_id)
    );
    let keyboard_focus_at_b = capture_focused_surface_id(&commands);
    assert_ne!(keyboard_focus_at_b, keyboard_focus_at_a);
    assert_eq!(keyboard_focus_at_b, Some(surface_b_id));
    state_a.pointer_motion = false;
    state_b.pointer_event_log.clear();

    let enter_a_before = state_a.pointer_enter_count;
    let leave_a_before = state_a.pointer_leave_count;
    let enter_b_before = state_b.pointer_enter_count;
    let leave_b_before = state_b.pointer_leave_count;
    let motion_a_after_focus_change = state_a
        .pointer_event_log
        .iter()
        .filter(|event| **event == "motion")
        .count();
    commands
        .send(ServerCommand::BeginMove {
            x: start_x,
            y: start_y,
        })
        .unwrap();
    commands
        .send(ServerCommand::UpdateInteraction { x: 140.0, y: 140.0 })
        .unwrap();
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::SendWindowInteractionPointerMotion {
            timestamp_usec: 42_000,
            x: 140.0,
            y: 140.0,
            reply,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue_a.roundtrip(&mut state_a).unwrap();
    queue_b.roundtrip(&mut state_b).unwrap();

    assert_eq!(receiver.recv().unwrap(), 0);
    assert_eq!(
        capture_pointer_focus_surface_id(&commands),
        Some(surface_b_id)
    );
    let keyboard_focus_during_interaction = capture_focused_surface_id(&commands);
    assert_eq!(keyboard_focus_during_interaction, Some(surface_a_id));
    let captured_owner = capture_window_interaction_debug_snapshot(&commands)
        .expect("interaction must remain captured after pointer Leave");
    assert_eq!(captured_owner.pointer_motion_surface_id, Some(surface_a_id));
    assert_ne!(captured_owner.pointer_motion_surface_id, Some(surface_b_id));
    assert_eq!(
        state_a
            .pointer_event_log
            .iter()
            .filter(|event| **event == "motion")
            .count(),
        motion_a_after_focus_change
    );
    assert_eq!(
        state_b
            .pointer_event_log
            .iter()
            .filter(|event| **event == "motion")
            .count(),
        0
    );
    assert_eq!(state_a.pointer_surface_x, Some(20.0));
    assert_eq!(state_a.pointer_surface_y, Some(14.0));
    assert_eq!(state_a.pointer_enter_count, enter_a_before);
    assert_eq!(state_a.pointer_leave_count, leave_a_before);
    assert_eq!(state_a.relative_motion_count, 0);
    assert_eq!(state_b.pointer_enter_count, enter_b_before);
    assert_eq!(state_b.pointer_leave_count, leave_b_before);
    assert_eq!(state_b.pointer_event_log, Vec::<&'static str>::new());

    commands.send(ServerCommand::EndInteraction).unwrap();
    wait_for_server_commands(&commands);
    let terminal_refreshes_before_lifecycle = capture_window_interaction_release_metrics(&commands)
        .window_interaction_post_terminal_pointer_refreshes;

    let renderable_after_safety = capture_renderable_surface_snapshot(&commands);
    let surface_id_at_after_safety = |x: f64, y: f64| {
        renderable_after_safety
            .iter()
            .rev()
            .find(|surface| {
                x >= f64::from(surface.origin_x)
                    && x < f64::from(surface.origin_x + surface.width as i32)
                    && y >= f64::from(surface.origin_y)
                    && y < f64::from(surface.origin_y + surface.height as i32)
            })
            .map(|surface| surface.surface_id)
    };
    let (return_x, return_y) = renderable_after_safety
        .iter()
        .find_map(|surface| {
            (surface.surface_id == surface_a_id).then_some([
                (
                    f64::from(surface.origin_x) + 5.0,
                    f64::from(surface.origin_y) + 5.0,
                ),
                (
                    f64::from(surface.origin_x + surface.width as i32) - 5.0,
                    f64::from(surface.origin_y) + 5.0,
                ),
                (
                    f64::from(surface.origin_x) + 5.0,
                    f64::from(surface.origin_y + surface.height as i32) - 5.0,
                ),
                (
                    f64::from(surface.origin_x + surface.width as i32) - 5.0,
                    f64::from(surface.origin_y + surface.height as i32) - 5.0,
                ),
            ])
        })
        .and_then(|candidates| {
            candidates
                .into_iter()
                .find(|(x, y)| surface_id_at_after_safety(*x, *y) == Some(surface_a_id))
        })
        .expect("A must remain topmost at one post-move return point");

    commands
        .send(ServerCommand::PointerMotion { x: 500.0, y: 300.0 })
        .unwrap();
    wait_for_server_commands(&commands);
    queue_a.roundtrip(&mut state_a).unwrap();
    queue_b.roundtrip(&mut state_b).unwrap();
    assert_eq!(capture_pointer_focus_surface_id(&commands), None);
    assert_eq!(
        capture_focused_surface_id(&commands),
        Some(surface_a_id),
        "native move keeps the raised interaction target focused when the pointer leaves all windows"
    );

    commands
        .send(ServerCommand::PointerMotion {
            x: return_x,
            y: return_y,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue_a.roundtrip(&mut state_a).unwrap();
    queue_b.roundtrip(&mut state_b).unwrap();
    assert_eq!(
        capture_pointer_focus_surface_id(&commands),
        Some(surface_a_id)
    );
    assert_eq!(
        capture_focused_surface_id(&commands),
        keyboard_focus_during_interaction
    );
    state_a.pointer_event_log.clear();
    state_b.pointer_event_log.clear();

    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: true,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue_a.roundtrip(&mut state_a).unwrap();
    queue_b.roundtrip(&mut state_b).unwrap();
    assert_eq!(
        capture_pointer_focus_surface_id(&commands),
        Some(surface_a_id)
    );
    assert_eq!(
        capture_focused_surface_id(&commands),
        keyboard_focus_during_interaction
    );
    let after_activation = capture_renderable_surface_snapshot(&commands);
    assert_eq!(
        after_activation.last().map(|surface| surface.surface_id),
        Some(surface_a_id),
        "the captured A target must be raised before button delivery"
    );
    assert!(state_a.pointer_event_log.contains(&"button_pressed"));
    assert!(!state_b.pointer_event_log.contains(&"button_pressed"));
    state_a.pointer_event_log.clear();
    state_b.pointer_event_log.clear();

    commands
        .send(ServerCommand::PointerMotion {
            x: return_x,
            y: return_y + 100.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);

    commands
        .send(ServerCommand::BeginMove {
            x: return_x,
            y: return_y + 100.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    let captured_owner = capture_window_interaction_debug_snapshot(&commands)
        .expect("interaction must be captured by A");
    assert_eq!(captured_owner.pointer_motion_surface_id, Some(surface_a_id));
    assert_ne!(captured_owner.pointer_motion_surface_id, Some(surface_b_id));
    for (x, y) in [(140.0, 140.0), (500.0, 300.0), (return_x, return_y)] {
        commands
            .send(ServerCommand::PointerMotion { x, y })
            .unwrap();
        wait_for_server_commands(&commands);
        queue_a.roundtrip(&mut state_a).unwrap();
        queue_b.roundtrip(&mut state_b).unwrap();
        assert_eq!(
            capture_pointer_focus_surface_id(&commands),
            Some(surface_a_id)
        );
        assert_eq!(
            capture_focused_surface_id(&commands),
            keyboard_focus_during_interaction
        );
        assert_eq!(
            capture_window_interaction_debug_snapshot(&commands),
            Some(captured_owner)
        );
    }

    assert_eq!(
        state_a
            .pointer_event_log
            .iter()
            .filter(|event| **event == "motion")
            .count(),
        4
    );
    assert_eq!(state_b.pointer_event_log, Vec::<&'static str>::new());

    commands
        .send(ServerCommand::PointerButton {
            button: 0x110,
            pressed: false,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue_a.roundtrip(&mut state_a).unwrap();
    queue_b.roundtrip(&mut state_b).unwrap();
    assert_eq!(
        capture_pointer_focus_surface_id(&commands),
        Some(surface_a_id)
    );
    assert!(capture_pointer_ownership_is_clear(&commands));
    assert_eq!(
        capture_window_interaction_debug_snapshot(&commands),
        Some(captured_owner)
    );
    assert_eq!(
        state_a
            .pointer_event_log
            .iter()
            .filter(|event| **event == "button_released")
            .count(),
        1
    );
    assert!(!state_b.pointer_event_log.contains(&"button_released"));
    commands.send(ServerCommand::EndInteraction).unwrap();
    wait_for_server_commands(&commands);
    assert_eq!(capture_window_interaction_debug_snapshot(&commands), None);
    assert_eq!(
        capture_pointer_focus_surface_id(&commands),
        Some(surface_a_id)
    );
    assert_eq!(
        capture_focused_surface_id(&commands),
        keyboard_focus_during_interaction
    );
    let terminal_refreshes = capture_window_interaction_release_metrics(&commands)
        .window_interaction_post_terminal_pointer_refreshes;
    assert_eq!(terminal_refreshes, terminal_refreshes_before_lifecycle + 1);
    commands.send(ServerCommand::EndInteraction).unwrap();
    wait_for_server_commands(&commands);
    assert_eq!(
        capture_window_interaction_release_metrics(&commands)
            .window_interaction_post_terminal_pointer_refreshes,
        terminal_refreshes
    );

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();

    let _ = pointer_a;
    let _ = pointer_b;
}

#[test]
fn window_interaction_motion_preserves_exact_subsurface_target() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let subcompositor: client_wl_subcompositor::WlSubcompositor =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let _pointer = seat.get_pointer(&qh, ());
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=2, ()).unwrap();

    let (parent, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    parent.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();

    let child = compositor.create_surface(&qh, ());
    let _subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    _subsurface.set_position(10, 20);
    commit_test_buffered_surface(&child, &shm, &qh, 40, 30).unwrap();
    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let x = f64::from(render::FIRST_SURFACE_OFFSET.0) + 15.0;
    let y = f64::from(render::FIRST_SURFACE_OFFSET.1) + 25.0;
    commands
        .send(ServerCommand::PointerMotion { x, y })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(
        state.pointer_enter_surface_id,
        Some(child.id().protocol_id())
    );
    let enter_count = state.pointer_enter_count;
    let leave_count = state.pointer_leave_count;
    state.pointer_motion = false;
    state.pointer_surface_x = None;
    state.pointer_surface_y = None;

    commands.send(ServerCommand::BeginMove { x, y }).unwrap();
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::SendWindowInteractionPointerMotion {
            timestamp_usec: 43_000,
            x,
            y,
            reply,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    assert_eq!(receiver.recv().unwrap(), 1);
    assert!(state.pointer_motion);
    assert_eq!(state.pointer_surface_x, Some(5.0));
    assert_eq!(state.pointer_surface_y, Some(5.0));
    assert_eq!(state.pointer_enter_count, enter_count);
    assert_eq!(state.pointer_leave_count, leave_count);

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();
}

fn run_resize_motion_coordinate_regression(
    start_local: (f64, f64),
    update_output: (f64, f64),
    expected_local: (f64, f64),
) {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let _pointer = seat.get_pointer(&qh, ());
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=2, ()).unwrap();
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
    surface.commit();
    connection.flush().unwrap();

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    let start_output = (
        f64::from(render::FIRST_SURFACE_OFFSET.0) + start_local.0,
        f64::from(render::FIRST_SURFACE_OFFSET.1) + start_local.1,
    );
    commands
        .send(ServerCommand::PointerMotion {
            x: start_output.0,
            y: start_output.1,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let enter_count = state.pointer_enter_count;
    let leave_count = state.pointer_leave_count;
    state.pointer_motion = false;
    state.pointer_surface_x = None;
    state.pointer_surface_y = None;

    commands
        .send(ServerCommand::BeginResize {
            x: start_output.0,
            y: start_output.1,
        })
        .unwrap();
    let (update_reply, update_receiver) = mpsc::channel();
    commands
        .send(ServerCommand::UpdateInteractionResult {
            x: update_output.0,
            y: update_output.1,
            reply: update_reply,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    assert!(update_receiver.recv().unwrap());
    commands.send(ServerCommand::PresentFrame).unwrap();
    wait_for_server_commands(&commands);
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::SendWindowInteractionPointerMotion {
            timestamp_usec: 44_000,
            x: update_output.0,
            y: update_output.1,
            reply,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    assert_eq!(receiver.recv().unwrap(), 1);
    assert!(state.pointer_motion);
    assert_eq!(state.pointer_surface_x, Some(expected_local.0));
    assert_eq!(state.pointer_surface_y, Some(expected_local.1));
    assert_eq!(state.pointer_enter_count, enter_count);
    assert_eq!(state.pointer_leave_count, leave_count);

    commands.send(ServerCommand::Stop).unwrap();
    server_thread.join().unwrap();
}

#[test]
fn left_resize_dispatches_client_motion_using_updated_surface_origin() {
    run_resize_motion_coordinate_regression((0.0, 40.0), (52.0, 112.0), (0.0, 40.0));
}

#[test]
fn top_resize_dispatches_client_motion_using_updated_surface_origin() {
    run_resize_motion_coordinate_regression((40.0, 0.0), (112.0, 52.0), (40.0, 0.0));
}

#[test]
fn right_bottom_resize_dispatches_client_motion_without_origin_change() {
    run_resize_motion_coordinate_regression((159.0, 119.0), (251.0, 211.0), (179.0, 139.0));
}
