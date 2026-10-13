use super::*;

#[test]
fn managed_x11_interactive_move_rederives_materialized_render_placement() {
    let surface_id = 42;
    let mut snapshot = test_x11_snapshot(surface_id);
    snapshot.geometry = crate::xwayland::xwm::X11Geometry {
        x: 359,
        y: 141,
        width: 1_437,
        height: 788,
    };
    let mut state = CompositorState::new(None);
    let window_id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_x11(window_id, snapshot))
        .expect("X11 desktop window");
    state.append_renderable_surface(test_renderable_surface(surface_id, 1_437, 788));
    let original = SurfacePlacement::absolute_root_at(359, 141);
    assert!(state.set_x11_frame_geometry_from_source(
        window_id,
        WindowGeometry::new(original, 1_437, 788),
        X11GeometryAuthoritySource::InteractiveMove,
    ));
    state.set_surface_placement(surface_id, original);
    assert!(state.apply_x11_materialized_root_render_placement(surface_id));
    assert!(!state.toplevel_visual_geometries.contains_key(&surface_id));
    assert!(!state.active_toplevel_resizes.contains_key(&surface_id));
    assert_eq!(
        state
            .renderable_surface(surface_id)
            .expect("managed X11 root")
            .render_placement,
        Some(original)
    );

    let mut interaction = test_window_interaction(1, WindowInteractionKind::Move, None);
    interaction.window_id = window_id;
    interaction.start_placement = original;
    interaction.start_width = 1_437;
    interaction.start_height = 788;
    state.window_interaction = Some(interaction);

    assert!(state.update_window_interaction_by_id(interaction.id, 99.0, 100.0));
    assert!(state.flush_pending_floating_interaction_geometry());

    let moved = SurfacePlacement::absolute_root_at(358, 141);
    assert_eq!(state.surface_placement(surface_id), moved);
    let window = state.window(window_id).expect("managed X11 window");
    assert_eq!(
        window.x11_geometry.expect("X11 geometry").frame.placement,
        moved
    );
    let materialized = window
        .x11_materialized_client_rect
        .expect("materialized X11 client rectangle");
    assert_eq!(
        (materialized.x() as i32, materialized.y() as i32),
        (358, 141)
    );

    let root = state
        .renderable_surface(surface_id)
        .expect("managed X11 root");
    assert_eq!(root.render_placement, Some(moved));
    let scene_surfaces = state.active_scene_surfaces();
    let scene_index = scene_surfaces
        .iter()
        .position(|surface| surface.surface_id == surface_id)
        .expect("root in active scene");
    let scene_root = &scene_surfaces[scene_index];
    assert_eq!(scene_root.placement, moved);
    assert_eq!(scene_root.render_placement, Some(moved));
    assert_eq!(
        state.active_scene_surface_origins()[scene_index],
        (358, 141)
    );
    assert!(matches!(
        state.take_backend_commands().as_slice(),
        [crate::compositor::window_backend::WindowBackendCommand::Configure {
            geometry,
            resizing: false,
            ..
        }] if geometry.placement == moved
    ));

    assert!(state.update_window_interaction_by_id(interaction.id, -175.0, 111.0));
    assert!(state.flush_pending_floating_interaction_geometry());

    let large_move = SurfacePlacement::absolute_root_at(84, 152);
    assert_eq!(state.surface_placement(surface_id), large_move);
    let window = state.window(window_id).expect("managed X11 window");
    assert_eq!(
        window.x11_geometry.expect("X11 geometry").frame.placement,
        large_move
    );
    assert_eq!(
        (
            window
                .x11_materialized_client_rect
                .expect("materialized X11 client rectangle")
                .x() as i32,
            window
                .x11_materialized_client_rect
                .expect("materialized X11 client rectangle")
                .y() as i32,
        ),
        (84, 152)
    );
    let scene_surfaces = state.active_scene_surfaces();
    let scene_index = scene_surfaces
        .iter()
        .position(|surface| surface.surface_id == surface_id)
        .expect("root in active scene");
    assert_eq!(
        scene_surfaces[scene_index].render_placement,
        Some(large_move)
    );
    assert_eq!(state.active_scene_surface_origins()[scene_index], (84, 152));
    let root = state
        .renderable_surface(surface_id)
        .expect("managed X11 root");
    let assignment = &state.active_scene_surface_render_assignments()[scene_index];
    assert!(
        crate::compositor::render::surface_local_point_for_assignment(
            root, assignment, 184.0, 252.0,
        )
        .is_some()
    );
    assert!(
        crate::compositor::render::surface_local_point_for_assignment(
            root, assignment, 1_659.0, 241.0,
        )
        .is_none()
    );
    assert!(matches!(
        state.take_backend_commands().as_slice(),
        [crate::compositor::window_backend::WindowBackendCommand::Configure {
            geometry,
            resizing: false,
            ..
        }] if geometry.placement == large_move
    ));
}

#[test]
fn managed_x11_interactive_move_preserves_committed_root_surface_offsets() {
    let surface_id = 42;
    let mut snapshot = test_x11_snapshot(surface_id);
    snapshot.geometry = crate::xwayland::xwm::X11Geometry {
        x: 400,
        y: 200,
        width: 800,
        height: 600,
    };
    let mut state = CompositorState::new(None);
    let window_id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_x11(window_id, snapshot))
        .expect("X11 desktop window");
    let mut root = test_renderable_surface(surface_id, 800, 600);
    root.x = 7;
    root.y = 11;
    state.append_renderable_surface(root);
    let original = SurfacePlacement::absolute_root_at(400, 200);
    assert!(state.set_x11_frame_geometry_from_source(
        window_id,
        WindowGeometry::new(original, 800, 600),
        X11GeometryAuthoritySource::InteractiveMove,
    ));
    assert!(state.set_surface_placement(surface_id, original));
    assert!(state.apply_x11_materialized_root_render_placement(surface_id));
    assert_eq!(
        state
            .renderable_surface(surface_id)
            .expect("managed X11 root")
            .render_placement,
        Some(SurfacePlacement::absolute_root_at(393, 189))
    );
    assert_eq!(state.active_scene_surface_origins()[0], (400, 200));

    let mut interaction = test_window_interaction(1, WindowInteractionKind::Move, None);
    interaction.window_id = window_id;
    interaction.start_placement = original;
    interaction.start_width = 800;
    interaction.start_height = 600;
    state.window_interaction = Some(interaction);
    assert!(state.update_window_interaction_by_id(interaction.id, 99.0, 100.0));
    assert!(state.flush_pending_floating_interaction_geometry());

    let moved = SurfacePlacement::absolute_root_at(399, 200);
    assert_eq!(state.surface_placement(surface_id), moved);
    assert_eq!(
        state
            .renderable_surface(surface_id)
            .expect("managed X11 root")
            .render_placement,
        Some(SurfacePlacement::absolute_root_at(392, 189))
    );
    assert_eq!(state.active_scene_surface_origins()[0], (399, 200));
}

#[test]
fn managed_x11_move_keeps_explicit_visual_geometry_in_control() {
    let surface_id = 42;
    let mut snapshot = test_x11_snapshot(surface_id);
    snapshot.geometry = crate::xwayland::xwm::X11Geometry {
        x: 359,
        y: 141,
        width: 1_437,
        height: 788,
    };
    let mut state = CompositorState::new(None);
    let window_id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_x11(window_id, snapshot))
        .expect("X11 desktop window");
    let mut root = test_renderable_surface(surface_id, 1_437, 788);
    root.x = 7;
    root.y = 11;
    state.append_renderable_surface(root);
    let canonical = SurfacePlacement::absolute_root_at(359, 141);
    assert!(state.set_x11_frame_geometry_from_source(
        window_id,
        WindowGeometry::new(canonical, 1_437, 788),
        X11GeometryAuthoritySource::InteractiveMove,
    ));
    assert!(state.set_surface_placement(surface_id, canonical));

    let visual_start = SurfacePlacement::absolute_root_at(500, 300);
    state.install_toplevel_visual_geometry(
        surface_id,
        WindowGeometry::new(visual_start, 1_437, 788),
    );
    assert!(state.toplevel_visual_geometries.contains_key(&surface_id));
    let mut interaction = test_window_interaction(1, WindowInteractionKind::Move, None);
    interaction.window_id = window_id;
    interaction.start_placement = visual_start;
    interaction.start_width = 1_437;
    interaction.start_height = 788;
    state.window_interaction = Some(interaction);

    assert!(state.update_window_interaction_by_id(interaction.id, 99.0, 100.0));
    assert!(state.flush_pending_floating_interaction_geometry());

    let moved = SurfacePlacement::absolute_root_at(499, 300);
    assert_eq!(state.surface_placement(surface_id), moved);
    let root = state
        .renderable_surface(surface_id)
        .expect("managed X11 root");
    assert_eq!(root.render_placement, Some(moved));
    assert_ne!(
        root.render_placement,
        Some(SurfacePlacement::absolute_root_at(492, 289)),
        "the materialized fallback must not replace the explicit visual assignment"
    );
    assert_eq!(state.active_scene_surface_origins()[0], (506, 311));
}

#[test]
fn x11_materialized_render_placement_is_limited_to_compositor_managed_windows() {
    for policy in [
        X11PlacementPolicy::ClientPositioned,
        X11PlacementPolicy::ParentRelative,
        X11PlacementPolicy::OverrideRedirect,
    ] {
        let surface_id = 42;
        let mut state = CompositorState::new(None);
        let window_id = state.allocate_window_id().expect("window id");
        state
            .insert_desktop_window(DesktopWindow::new_x11(
                window_id,
                test_x11_snapshot(surface_id),
            ))
            .expect("X11 desktop window");
        state.append_renderable_surface(test_renderable_surface(surface_id, 300, 200));
        state
            .window_mut(window_id)
            .expect("X11 window")
            .x11_placement_policy = Some(policy);

        assert_eq!(
            state.derive_x11_materialized_root_render_placement(surface_id),
            None,
            "policy {policy:?} does not use managed materialized geometry"
        );
        assert!(!state.apply_x11_materialized_root_render_placement(surface_id));
        assert_eq!(
            state
                .renderable_surface(surface_id)
                .expect("X11 root")
                .render_placement,
            None
        );
    }
}
