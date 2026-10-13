use super::*;

#[test]
fn x11_moveresize_button_zero_uses_the_current_pressed_button() {
    let mut fixture = first_buffer_fixture();
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    let handle = snapshot.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));

    fixture.server.send_pointer_motion(101.0, 101.0);
    fixture.server.send_pointer_button(0x110, true);
    let result = fixture.server.state.begin_x11_client_window_interaction(
        handle,
        101.0,
        101.0,
        crate::compositor::WindowInteractionKind::Move,
        0,
    );

    assert_eq!(result, X11MoveResizeBeginResult::Began);
    assert_eq!(
        fixture
            .server
            .window_interaction_debug_snapshot()
            .expect("button-zero moveresize interaction")
            .trigger_button,
        Some(0x110)
    );
    assert_eq!(fixture.server.resize_flow_metrics().x11_moveresize_began, 1);
}

#[test]
fn delayed_managed_configure_notify_cannot_restore_an_older_client_geometry() {
    let mut fixture = first_buffer_fixture();
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    snapshot.geometry = X11Geometry {
        x: 100,
        y: 120,
        width: 640,
        height: 480,
    };
    let handle = snapshot.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));
    let managed_y = handle_geometry(&fixture.server, handle).y;

    let geometry = |x, width| X11Geometry {
        x,
        y: managed_y,
        width,
        height: 480,
    };
    for (x, width) in [(110, 630), (120, 620), (130, 610)] {
        fixture
            .server
            .apply_xwayland_window_event(XwmEvent::ConfigureRequested {
                window: handle,
                request: X11ConfigureRequest {
                    requested: geometry(x, width),
                    fields: X11ConfigureFlags {
                        x: true,
                        width: true,
                        ..X11ConfigureFlags::default()
                    },
                    client_event_sequence: None,
                    border_width: 0,
                    sibling: None,
                    stack_mode: None,
                },
            });
    }

    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::ConfigureNotify {
            window: handle,
            geometry: geometry(110, 630),
            above_sibling: None,
        });

    assert_eq!(
        handle_geometry(&fixture.server, handle),
        geometry(130, 610),
        "an older self-generated notification must not roll back the newest desired box"
    );
}

#[test]
fn compositor_move_persists_pointer_owned_geometry_before_delayed_notify() {
    let mut fixture = first_buffer_fixture();
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    let handle = snapshot.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));

    fixture.server.send_pointer_motion(101.0, 101.0);
    fixture.server.send_pointer_button(0x110, true);
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::MoveResizeRequested {
            window: handle,
            request: X11MoveResizeRequest {
                root_x: 101,
                root_y: 101,
                direction: X11MoveResizeDirection::Move,
                button: 1,
                source: 1,
            },
        });
    fixture.server.send_pointer_motion(301.0, 241.0);

    let visual = fixture
        .server
        .state
        .current_visual_root_window_geometry(fixture.surface_id)
        .expect("visual geometry after pointer move");
    let expected = X11Geometry {
        x: visual.placement.local_x,
        y: visual.placement.local_y,
        width: visual.width,
        height: visual.height,
    };

    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::ConfigureNotify {
            window: handle,
            geometry: X11Geometry {
                x: expected.x - 200,
                y: expected.y - 140,
                ..expected
            },
            above_sibling: None,
        });

    assert_eq!(handle_geometry(&fixture.server, handle), expected);
}

#[test]
fn restored_replacement_xwayland_root_moves_and_keeps_current_render_origin() {
    let mut fixture = stationary_pointer_xwayland_fixture();
    let original_geometry = X11Geometry {
        x: 359,
        y: 141,
        width: 1_437,
        height: 788,
    };
    let (window_id, handle, source_content) =
        prepare_pending_xwayland_restore(&mut fixture, original_geometry);
    commit_replacement_xwayland_content(
        &mut fixture,
        &source_content,
        original_geometry.width,
        original_geometry.height,
        7,
        11,
    );
    fixture
        .server
        .state
        .cancel_presentation_geometry_for_root(fixture.popup_surface_id);

    let state = &fixture.server.state;
    let window = state
        .window(window_id)
        .expect("restored managed X11 window");
    assert_eq!(window.root_surface_id, fixture.popup_surface_id);
    assert_eq!(
        window.x11_geometry.expect("X11 geometry").frame.placement,
        SurfacePlacement::absolute_root_at(359, 141)
    );
    assert!(!state.pending_xwayland_restores.contains_key(&window_id));
    assert!(
        !state
            .toplevel_visual_geometries
            .contains_key(&fixture.popup_surface_id)
    );
    assert!(
        !state
            .active_toplevel_resizes
            .contains_key(&fixture.popup_surface_id)
    );
    assert!(
        !state
            .toplevel_surfaces
            .contains_key(&fixture.popup_surface_id),
        "replacement XWayland root has no native toplevel visual owner"
    );
    assert!(!state.presentation_animation_pending_for_root(fixture.popup_surface_id));
    assert!(
        state
            .presented_presentation_transform(fixture.popup_surface_id)
            .is_none()
    );
    let initial_client_rect = window
        .x11_materialized_client_rect
        .expect("materialized X11 client rectangle");
    assert_eq!(
        (
            initial_client_rect.x() as i32,
            initial_client_rect.y() as i32
        ),
        (359, 141)
    );
    let (root_x, root_y, root_render_placement) = state
        .renderable_surface(fixture.popup_surface_id)
        .map(|root| (root.x, root.y, root.render_placement))
        .expect("replacement XWayland root");
    assert_eq!((root_x, root_y), (7, 11));
    assert_eq!(
        root_render_placement,
        Some(SurfacePlacement::absolute_root_at(352, 130))
    );
    let scene_index = state
        .active_scene_surfaces()
        .iter()
        .position(|surface| surface.surface_id == fixture.popup_surface_id)
        .expect("replacement root in active scene");
    assert_eq!(
        state.active_scene_surface_origins()[scene_index],
        (359, 141)
    );

    let start_pointer_x = 459.0;
    let start_pointer_y = 241.0;
    fixture
        .server
        .send_pointer_motion(start_pointer_x, start_pointer_y);
    fixture.server.send_pointer_button(0x110, true);
    assert_eq!(
        fixture.server.state.begin_x11_client_window_interaction(
            handle,
            start_pointer_x,
            start_pointer_y,
            crate::compositor::WindowInteractionKind::Move,
            1,
        ),
        X11MoveResizeBeginResult::Began
    );
    let interaction = fixture
        .server
        .state
        .window_interaction
        .expect("interactive Move began on replacement root");
    assert_eq!(interaction.root_surface_id, fixture.popup_surface_id);
    assert!(
        !fixture
            .server
            .state
            .toplevel_visual_geometries
            .contains_key(&fixture.popup_surface_id)
    );
    assert!(
        !fixture
            .server
            .state
            .active_toplevel_resizes
            .contains_key(&fixture.popup_surface_id)
    );
    assert_eq!(
        interaction.start_placement,
        SurfacePlacement::absolute_root_at(359, 141)
    );
    let _ = fixture.server.state.take_backend_commands();

    fixture
        .server
        .send_pointer_motion(start_pointer_x - 1.0, start_pointer_y);
    let _ = fixture
        .server
        .state
        .flush_pending_floating_interaction_geometry();
    let first_move = SurfacePlacement::absolute_root_at(358, 141);
    let window = fixture.server.state.window(window_id).expect("X11 window");
    assert_eq!(
        window.x11_geometry.expect("X11 geometry").frame.placement,
        first_move
    );
    assert_eq!(
        (
            window
                .x11_materialized_client_rect
                .expect("materialized client rectangle")
                .x() as i32,
            window
                .x11_materialized_client_rect
                .expect("materialized client rectangle")
                .y() as i32,
        ),
        (358, 141)
    );
    let root = fixture
        .server
        .state
        .renderable_surface(fixture.popup_surface_id)
        .expect("replacement XWayland root");
    assert_eq!(root.placement, first_move);
    let materialized_rect = window
        .x11_materialized_client_rect
        .expect("materialized X11 client rectangle after move");
    assert_eq!(
        root.render_placement,
        Some(SurfacePlacement::absolute_root_at(
            (materialized_rect.x().round() as i32).saturating_sub(root.x),
            (materialized_rect.y().round() as i32).saturating_sub(root.y),
        )),
        "replacement render placement must preserve the authoritative client origin"
    );
    let scene_index = fixture
        .server
        .state
        .active_scene_surfaces()
        .iter()
        .position(|surface| surface.surface_id == fixture.popup_surface_id)
        .expect("replacement root in active scene");
    assert_eq!(
        fixture.server.state.active_scene_surface_origins()[scene_index],
        (358, 141)
    );
    assert!(matches!(
        fixture.server.state.take_backend_commands().as_slice(),
        [crate::compositor::window_backend::WindowBackendCommand::Configure {
            geometry,
            resizing: false,
            ..
        }] if geometry.placement == first_move
    ));

    fixture
        .server
        .send_pointer_motion(start_pointer_x - 275.0, start_pointer_y + 11.0);
    let _ = fixture
        .server
        .state
        .flush_pending_floating_interaction_geometry();
    let large_move = SurfacePlacement::absolute_root_at(84, 152);
    assert_eq!(
        fixture
            .server
            .state
            .surface_placement(fixture.popup_surface_id),
        large_move
    );
    assert_eq!(
        fixture
            .server
            .state
            .renderable_surface(fixture.popup_surface_id)
            .expect("replacement XWayland root")
            .render_placement,
        Some(SurfacePlacement::absolute_root_at(77, 141))
    );
    let scene_index = fixture
        .server
        .state
        .active_scene_surfaces()
        .iter()
        .position(|surface| surface.surface_id == fixture.popup_surface_id)
        .expect("replacement root in active scene");
    assert_eq!(
        fixture.server.state.active_scene_surface_origins()[scene_index],
        (84, 152)
    );
    assert!(matches!(
        fixture.server.state.take_backend_commands().as_slice(),
        [crate::compositor::window_backend::WindowBackendCommand::Configure {
            geometry,
            resizing: false,
            ..
        }] if geometry.placement == large_move
    ));

    let root = fixture
        .server
        .state
        .renderable_surface(fixture.popup_surface_id)
        .expect("replacement XWayland root");
    let scene_surfaces = fixture.server.state.active_scene_surfaces();
    let scene_index = scene_surfaces
        .iter()
        .position(|surface| surface.surface_id == fixture.popup_surface_id)
        .expect("replacement root in active scene");
    let assignment = &fixture
        .server
        .state
        .active_scene_surface_render_assignments()[scene_index];
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

    let replacement_content = fixture
        .server
        .state
        .current_surface_buffers
        .get(&fixture.popup_surface_id)
        .cloned()
        .expect("authoritative replacement content");
    for _ in 0..3 {
        assert!(fixture.server.state.commit_xwayland_surface_buffer(
            fixture.popup_surface_id,
            replacement_content.clone(),
            Vec::new(),
            SurfacePublicationSource::Immediate,
        ));
        assert_eq!(
            fixture
                .server
                .state
                .surface_placement(fixture.popup_surface_id),
            large_move
        );
        let root = fixture
            .server
            .state
            .renderable_surface(fixture.popup_surface_id)
            .expect("replacement XWayland root");
        assert_eq!(
            root.render_placement,
            Some(SurfacePlacement::absolute_root_at(77, 141))
        );
        let scene_index = fixture
            .server
            .state
            .active_scene_surfaces()
            .iter()
            .position(|surface| surface.surface_id == fixture.popup_surface_id)
            .expect("replacement root in active scene");
        assert_eq!(
            fixture.server.state.active_scene_surface_origins()[scene_index],
            (84, 152)
        );
        assert_ne!(
            root.render_placement,
            Some(SurfacePlacement::absolute_root_at(352, 130))
        );
    }
}
