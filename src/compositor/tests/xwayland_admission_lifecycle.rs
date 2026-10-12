use super::*;

#[test]
fn x11_window_ready_admits_oversized_initial_geometry_through_managed_policy() {
    let mut fixture = first_buffer_fixture();
    assert!(fixture.server.state.set_output_size(1_920, 1_080));
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    snapshot.geometry = X11Geometry {
        x: -1_000,
        y: -500,
        width: 1_920,
        height: 1_080,
    };
    let handle = snapshot.handle;

    let commands = fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));

    let window_id = fixture
        .server
        .state
        .window_id_for_x11_handle(handle)
        .expect("normal X11 window admitted");
    let window = fixture
        .server
        .state
        .window(window_id)
        .expect("admitted window");
    let accepted = window.x11_geometry.expect("accepted X11 geometry");
    let materialized = window
        .x11_materialized_client_rect
        .expect("accepted client rectangle");
    let usable = fixture.server.state.usable_output_geometry();
    let extents = fixture.server.state.x11_decoration_frame_extents(handle);
    let [left, right, top, bottom] = extents.map(i64::from);

    assert!(
        accepted.client.height < 1_080,
        "initial height must be constrained"
    );
    assert_eq!(accepted.client.width, 1_920);
    assert_eq!(accepted.client.width, accepted.frame.width);
    assert_eq!(accepted.client.height, accepted.frame.height);
    assert_eq!(
        materialized,
        crate::compositor::PresentationRect::new(
            f64::from(accepted.client.x),
            f64::from(accepted.client.y),
            f64::from(accepted.client.width),
            f64::from(accepted.client.height),
        )
        .expect("accepted rectangle is valid")
    );
    assert!(i64::from(accepted.client.x) - left >= usable.x.ceil() as i64);
    assert!(
        i64::from(accepted.client.x) + i64::from(accepted.client.width) + right
            <= (usable.x + usable.width).floor() as i64
    );
    assert!(i64::from(accepted.client.y) - top >= usable.y.ceil() as i64);
    assert!(
        i64::from(accepted.client.y) + i64::from(accepted.client.height) + bottom
            <= (usable.y + usable.height).floor() as i64
    );
    assert_eq!(
        commands
            .iter()
            .filter(|command| matches!(
                command,
                XwmCommand::ConfigureFrame { window, geometry, frame_extents }
                    if *window == handle && *geometry == accepted.client && *frame_extents == extents
            ))
            .count(),
        1,
        "admission must publish one accepted ConfigureFrame authority"
    );
}

#[test]
fn initial_fullscreen_keeps_a_constrained_normal_restore_geometry() {
    let mut fixture = first_buffer_fixture();
    assert!(fixture.server.state.set_output_size(1_920, 1_080));
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    snapshot.geometry = X11Geometry {
        x: -900,
        y: -700,
        width: 1_920,
        height: 1_080,
    };
    snapshot.state.fullscreen = true;
    let handle = snapshot.handle;

    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));

    let window_id = fixture
        .server
        .state
        .window_id_for_x11_handle(handle)
        .expect("fullscreen X11 window admitted");
    let window = fixture
        .server
        .state
        .window(window_id)
        .expect("admitted window");
    assert_eq!(
        window.state.mode(),
        crate::compositor::ToplevelMode::Fullscreen
    );
    let visible = window.x11_geometry.expect("fullscreen X11 geometry").frame;
    assert_eq!((visible.width, visible.height), (1_920, 1_080));
    let restore = window
        .state
        .restore_geometry()
        .expect("normal restore geometry captured before fullscreen");
    let extents = fixture.server.state.x11_decoration_frame_extents(handle);
    let [left, right, top, bottom] = extents.map(i64::from);
    assert!(restore.width < 1_920 || restore.height < 1_080);
    assert!(i64::from(restore.width) + left + right <= 1_920);
    assert!(i64::from(restore.height) + top + bottom <= 1_080);
}

#[test]
fn initial_maximized_keeps_a_constrained_normal_restore_geometry() {
    let mut fixture = first_buffer_fixture();
    assert!(fixture.server.state.set_output_size(1_920, 1_080));
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    snapshot.geometry = X11Geometry {
        x: -900,
        y: -700,
        width: 1_920,
        height: 1_080,
    };
    snapshot.state.maximized = true;
    let handle = snapshot.handle;

    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));

    let window_id = fixture
        .server
        .state
        .window_id_for_x11_handle(handle)
        .expect("maximized X11 window admitted");
    let window = fixture
        .server
        .state
        .window(window_id)
        .expect("admitted window");
    assert_eq!(
        window.state.mode(),
        crate::compositor::ToplevelMode::Maximized
    );
    let restore = window
        .state
        .restore_geometry()
        .expect("normal restore geometry captured before maximize");
    let extents = fixture.server.state.x11_decoration_frame_extents(handle);
    let [left, right, top, bottom] = extents.map(i64::from);
    assert!(i64::from(restore.width) + left + right <= 1_920);
    assert!(i64::from(restore.height) + top + bottom <= 1_080);
}

#[test]
fn client_positioned_initial_x11_geometry_skips_managed_work_area_clamping() {
    let mut fixture = first_buffer_fixture();
    assert!(fixture.server.state.set_output_size(1_920, 1_080));
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    snapshot.window_types = X11WindowTypes::new(vec![X11WindowType::PopupMenu]);
    snapshot.geometry = X11Geometry {
        x: -120,
        y: -80,
        width: 2_000,
        height: 1_200,
    };
    let handle = snapshot.handle;

    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));

    let window_id = fixture
        .server
        .state
        .window_id_for_x11_handle(handle)
        .expect("client-positioned X11 window admitted");
    let window = fixture
        .server
        .state
        .window(window_id)
        .expect("admitted popup");
    assert_eq!(
        window.x11_placement_policy,
        Some(crate::compositor::X11PlacementPolicy::ClientPositioned)
    );
    let geometry = window.x11_geometry.expect("popup geometry").client;
    assert_eq!((geometry.x, geometry.y), (-120, -80));
    assert_eq!((geometry.width, geometry.height), (2_000, 1_200));
}

#[test]
fn compositor_minimize_unmaps_once_and_restore_maps_with_live_association() {
    let mut fixture = first_buffer_fixture();
    assert!(fixture.server.state.set_output_size(1_920, 1_080));
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    snapshot.geometry = X11Geometry {
        x: 136,
        y: 100,
        width: 800,
        height: 600,
    };
    let handle = snapshot.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));
    let window_id = fixture
        .server
        .state
        .window_id_for_x11_handle(handle)
        .expect("normal X11 window admitted");
    let original_geometry = fixture
        .server
        .state
        .window(window_id)
        .expect("admitted window")
        .x11_geometry
        .expect("managed X11 geometry");
    let original_materialized = fixture
        .server
        .state
        .window(window_id)
        .expect("admitted window")
        .x11_materialized_client_rect;
    let original_placement = fixture.server.state.surface_placement(fixture.surface_id);
    assert_eq!(
        (
            original_geometry.client.width,
            original_geometry.client.height
        ),
        (800, 600)
    );
    assert!(
        !fixture
            .server
            .state
            .restore_minimized_desktop_window(window_id)
    );
    assert!(
        fixture
            .server
            .take_xwayland_backend_commands(0)
            .iter()
            .all(|command| !matches!(command, XwmCommand::Map(window) if *window == handle)),
        "a restore request for an already Normal window must not queue Map"
    );
    fixture
        .server
        .set_lifecycle_animation_renderer_available(false);

    assert!(fixture.server.state.minimize_desktop_window(window_id));
    let minimized = fixture.server.take_xwayland_backend_commands(0);
    assert_eq!(
        minimized
            .iter()
            .filter(|command| matches!(command, XwmCommand::Unmap(window) if *window == handle))
            .count(),
        1,
        "compositor minimize must queue one physical X11 unmap without animation"
    );
    assert!(minimized.iter().any(|command| matches!(
        command,
        XwmCommand::SetState { window, state, .. }
            if *window == handle && state.hidden
    )));
    assert!(minimized.iter().all(|command| !matches!(
        command,
        XwmCommand::ConfigureFrame { window, .. } if *window == handle
    )));
    let minimized_window = fixture
        .server
        .state
        .window(window_id)
        .expect("minimized X11 window");
    assert_eq!(minimized_window.x11_geometry, Some(original_geometry));
    assert_eq!(
        minimized_window.x11_materialized_client_rect,
        original_materialized
    );
    assert_eq!(
        fixture.server.state.surface_placement(fixture.surface_id),
        original_placement
    );
    assert!(!fixture.server.state.minimize_desktop_window(window_id));
    assert_eq!(
        fixture.server.state.window_id_for_x11_handle(handle),
        Some(window_id),
        "WM-owned unmap must preserve managed window identity"
    );
    assert!(
        fixture
            .server
            .state
            .desktop_windows
            .contains_key(&window_id)
    );
    assert!(fixture.server.state.window_stacking.contains(&window_id));

    assert!(
        fixture
            .server
            .state
            .restore_minimized_desktop_window(window_id)
    );
    let restored = fixture.server.take_xwayland_backend_commands(1);
    assert_eq!(
        restored
            .iter()
            .filter(|command| matches!(command, XwmCommand::Map(window) if *window == handle))
            .count(),
        1,
        "restore must map even while the old Wayland association still exists"
    );
    assert!(restored.iter().all(|command| !matches!(
        command,
        XwmCommand::ConfigureFrame { window, .. } if *window == handle
    )));
    let restored_window = fixture
        .server
        .state
        .window(window_id)
        .expect("restoring X11 window");
    assert_eq!(restored_window.x11_geometry, Some(original_geometry));
    assert_eq!(
        restored_window.x11_materialized_client_rect,
        original_materialized
    );
    assert_eq!(
        fixture.server.state.surface_placement(fixture.surface_id),
        original_placement
    );
    assert_eq!(
        fixture.server.state.window_id_for_x11_handle(handle),
        Some(window_id)
    );
    assert!(
        fixture
            .server
            .state
            .restore_minimized_desktop_window(window_id)
    );
    assert!(
        fixture
            .server
            .state
            .pending_xwayland_restores
            .contains_key(&window_id),
        "a repeated request remains attached to the pending source handoff"
    );
    assert!(
        fixture
            .server
            .take_xwayland_backend_commands(2)
            .iter()
            .all(|command| !matches!(command, XwmCommand::Map(window) if *window == handle))
    );
}

#[test]
fn compositor_minimize_unmaps_managed_x11_without_renderable_surfaces() {
    let mut fixture = first_buffer_fixture();
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    let handle = snapshot.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));
    let window_id = fixture
        .server
        .state
        .window_id_for_x11_handle(handle)
        .expect("normal X11 window admitted");
    fixture
        .server
        .set_lifecycle_animation_renderer_available(false);
    fixture.server.state.renderable_surfaces.clear();
    fixture.server.state.rebuild_renderable_surface_index();

    assert!(fixture.server.state.minimize_desktop_window(window_id));
    let commands = fixture.server.take_xwayland_backend_commands(0);
    assert_eq!(
        commands
            .iter()
            .filter(|command| matches!(command, XwmCommand::Unmap(window) if *window == handle))
            .count(),
        1,
        "a managed X11 window must unmap even when no surface can be rendered"
    );
    assert!(commands.iter().any(|command| matches!(
        command,
        XwmCommand::SetState { window, state, .. }
            if *window == handle && state.hidden
    )));
    assert!(
        fixture
            .server
            .state
            .window(window_id)
            .is_some_and(|window| window.state.is_minimized())
    );
    assert_eq!(
        fixture.server.state.window_id_for_x11_handle(handle),
        Some(window_id)
    );
}

#[test]
fn rapid_minimize_restore_queues_map_after_unmap_without_losing_identity() {
    let mut fixture = first_buffer_fixture();
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    let handle = snapshot.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));
    let window_id = fixture
        .server
        .state
        .window_id_for_x11_handle(handle)
        .expect("normal X11 window admitted");

    assert!(fixture.server.state.minimize_desktop_window(window_id));
    assert!(
        fixture
            .server
            .state
            .restore_minimized_desktop_window(window_id)
    );
    let commands = fixture.server.take_xwayland_backend_commands(2);
    let mapping = commands
        .iter()
        .filter_map(|command| match command {
            XwmCommand::Unmap(window) if *window == handle => Some("unmap"),
            XwmCommand::Map(window) if *window == handle => Some("map"),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(mapping, ["unmap", "map"]);
    assert_eq!(
        fixture.server.state.window_id_for_x11_handle(handle),
        Some(window_id),
        "rapid reversal must preserve the persistent X11 window identity"
    );
    assert!(
        fixture
            .server
            .state
            .pending_xwayland_restores
            .contains_key(&window_id),
        "the Map request keeps restore active while replacement content is pending"
    );
    assert!(
        fixture
            .server
            .state
            .window(window_id)
            .expect("reversed X11 window")
            .state
            .is_minimized(),
        "the visible restore handoff remains pending until valid XWayland content arrives"
    );
}

#[test]
fn managed_configure_after_remap_uses_preserved_frame_extents_and_geometry_policy() {
    let mut fixture = first_buffer_fixture();
    assert!(fixture.server.state.set_output_size(1_920, 1_080));
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    snapshot.geometry = X11Geometry {
        x: 80,
        y: 80,
        width: 800,
        height: 600,
    };
    let handle = snapshot.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));
    let window_id = fixture
        .server
        .state
        .window_id_for_x11_handle(handle)
        .expect("normal X11 window admitted");
    fixture
        .server
        .set_lifecycle_animation_renderer_available(false);
    let extents_before = fixture.server.state.x11_decoration_frame_extents(handle);
    assert!(extents_before.iter().any(|extent| *extent != 0));

    assert!(fixture.server.state.minimize_desktop_window(window_id));
    let minimize = fixture.server.take_xwayland_backend_commands(0);
    assert!(minimize.iter().any(|command| matches!(
        command,
        XwmCommand::Unmap(window) if *window == handle
    )));
    assert!(
        fixture
            .server
            .state
            .restore_minimized_desktop_window(window_id)
    );
    let restore = fixture.server.take_xwayland_backend_commands(1);
    assert!(restore.iter().any(|command| matches!(
        command,
        XwmCommand::Map(window) if *window == handle
    )));
    assert_eq!(
        fixture.server.state.x11_decoration_frame_extents(handle),
        extents_before,
        "remapping must preserve the managed frame extents"
    );
    assert!(restore.iter().all(|command| !matches!(
        command,
        XwmCommand::SetFrameExtents(window, _)
            if *window == handle
    )));

    let commands = fixture
        .server
        .apply_xwayland_window_event(XwmEvent::ConfigureRequested {
            window: handle,
            request: X11ConfigureRequest {
                requested: X11Geometry {
                    x: 0,
                    y: 0,
                    width: 1_920,
                    height: 1_080,
                },
                fields: X11ConfigureFlags {
                    width: true,
                    height: true,
                    ..X11ConfigureFlags::default()
                },
                client_event_sequence: None,
                border_width: 0,
                sibling: None,
                stack_mode: None,
            },
        });
    let accepted = match commands.as_slice() {
        [XwmCommand::Configure { geometry, .. }] => *geometry,
        other => panic!("expected one accepted post-remap Configure, got {other:?}"),
    };
    let usable = fixture.server.state.usable_output_geometry();
    let [left, right, top, bottom] = extents_before.map(i64::from);
    assert!(accepted.width < 1_920 || accepted.height < 1_080);
    assert!(i64::from(accepted.x) - left >= usable.x.ceil() as i64);
    assert!(
        i64::from(accepted.x) + i64::from(accepted.width) + right
            <= (usable.x + usable.width).floor() as i64
    );
    assert!(i64::from(accepted.y) - top >= usable.y.ceil() as i64);
    assert!(
        i64::from(accepted.y) + i64::from(accepted.height) + bottom
            <= (usable.y + usable.height).floor() as i64
    );
    assert_eq!(
        fixture.server.state.x11_authoritative_geometry(handle),
        Some(accepted),
        "the request must pass through managed geometry authority after remap"
    );
}

#[test]
fn client_hidden_minimize_preserves_normal_managed_geometry() {
    let mut fixture = first_buffer_fixture();
    assert!(fixture.server.state.set_output_size(1_920, 1_080));
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    snapshot.geometry = X11Geometry {
        x: 136,
        y: 100,
        width: 800,
        height: 600,
    };
    let handle = snapshot.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));
    let window_id = fixture
        .server
        .state
        .window_id_for_x11_handle(handle)
        .expect("normal X11 window admitted");
    let original_geometry = fixture
        .server
        .state
        .window(window_id)
        .expect("admitted window")
        .x11_geometry
        .expect("managed X11 geometry");
    let original_materialized = fixture
        .server
        .state
        .window(window_id)
        .expect("admitted window")
        .x11_materialized_client_rect;
    let original_placement = fixture.server.state.surface_placement(fixture.surface_id);
    assert_eq!(
        (
            original_geometry.client.width,
            original_geometry.client.height
        ),
        (800, 600)
    );
    let _ = fixture.server.take_xwayland_backend_commands(0);

    let immediate = fixture
        .server
        .apply_xwayland_window_event(XwmEvent::StateRequested {
            window: handle,
            request: crate::xwayland::xwm::X11StateRequest {
                action: crate::xwayland::xwm::X11StateAction::Add,
                first: Some(crate::xwayland::xwm::X11StateAtom::Hidden),
                second: None,
            },
        });
    assert!(
        immediate
            .iter()
            .all(|command| !matches!(command, XwmCommand::Unmap(_))),
        "StateRequested must not issue a second direct unmap"
    );
    let backend = fixture.server.take_xwayland_backend_commands(0);
    assert_eq!(
        backend
            .iter()
            .filter(|command| matches!(command, XwmCommand::Unmap(window) if *window == handle))
            .count(),
        1,
        "client Hidden and compositor minimize must converge on one backend transition"
    );
    assert!(backend.iter().any(|command| matches!(
        command,
        XwmCommand::SetState { window, state, .. }
            if *window == handle && state.hidden
    )));
    assert!(backend.iter().all(|command| !matches!(
        command,
        XwmCommand::ConfigureFrame { window, .. } if *window == handle
    )));
    let minimized_window = fixture
        .server
        .state
        .window(window_id)
        .expect("minimized X11 window");
    assert_eq!(minimized_window.x11_geometry, Some(original_geometry));
    assert_eq!(
        minimized_window.x11_materialized_client_rect,
        original_materialized
    );
    assert_eq!(
        fixture.server.state.surface_placement(fixture.surface_id),
        original_placement
    );
    assert!(
        fixture
            .server
            .state
            .window(window_id)
            .is_some_and(|window| window.state.is_minimized())
    );
    assert_eq!(
        fixture.server.state.window_id_for_x11_handle(handle),
        Some(window_id)
    );
}

#[test]
fn client_hidden_restore_preserves_normal_managed_geometry() {
    let mut fixture = first_buffer_fixture();
    assert!(fixture.server.state.set_output_size(1_920, 1_080));
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    snapshot.geometry = X11Geometry {
        x: 136,
        y: 100,
        width: 800,
        height: 600,
    };
    let handle = snapshot.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));
    let window_id = fixture
        .server
        .state
        .window_id_for_x11_handle(handle)
        .expect("normal X11 window admitted");
    let _ = fixture.server.take_xwayland_backend_commands(0);

    assert!(fixture.server.state.minimize_desktop_window(window_id));
    let minimized_commands = fixture.server.take_xwayland_backend_commands(0);
    assert_eq!(
        minimized_commands
            .iter()
            .filter(|command| matches!(command, XwmCommand::Unmap(window) if *window == handle))
            .count(),
        1
    );
    let original_geometry = fixture
        .server
        .state
        .window(window_id)
        .expect("minimized X11 window")
        .x11_geometry
        .expect("managed geometry");
    let original_materialized = fixture
        .server
        .state
        .window(window_id)
        .expect("minimized X11 window")
        .x11_materialized_client_rect;
    let original_placement = fixture.server.state.surface_placement(fixture.surface_id);
    let original_visual_geometry = fixture
        .server
        .state
        .toplevel_visual_geometries
        .get(&fixture.surface_id)
        .cloned();
    assert_eq!(
        (
            original_geometry.client.width,
            original_geometry.client.height
        ),
        (800, 600)
    );
    assert!(fixture.server.state.pending_xwayland_restores.is_empty());
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::StateRequested {
            window: handle,
            request: crate::xwayland::xwm::X11StateRequest {
                action: crate::xwayland::xwm::X11StateAction::Remove,
                first: Some(crate::xwayland::xwm::X11StateAtom::Hidden),
                second: None,
            },
        });
    let restored = fixture.server.take_xwayland_backend_commands(0);
    assert_eq!(
        restored
            .iter()
            .filter(|command| matches!(command, XwmCommand::Map(window) if *window == handle))
            .count(),
        1,
        "restoring a hidden client must queue one Map"
    );
    assert!(restored.iter().all(|command| !matches!(
        command,
        XwmCommand::ConfigureFrame { window, .. } if *window == handle
    )));
    assert!(
        fixture
            .server
            .state
            .pending_xwayland_restores
            .contains_key(&window_id)
    );
    let restored_window = fixture
        .server
        .state
        .window(window_id)
        .expect("restoring X11 window");
    assert_eq!(restored_window.x11_geometry, Some(original_geometry));
    assert_eq!(
        restored_window.x11_materialized_client_rect,
        original_materialized
    );
    assert_eq!(
        fixture.server.state.surface_placement(fixture.surface_id),
        original_placement
    );
    assert_eq!(
        fixture
            .server
            .state
            .toplevel_visual_geometries
            .get(&fixture.surface_id)
            .cloned(),
        original_visual_geometry
    );
}

#[test]
fn minimized_normal_to_maximized_visible_applies_only_final_mode_geometry() {
    let mut fixture = first_buffer_fixture();
    assert!(fixture.server.state.set_output_size(1_920, 1_080));
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    snapshot.geometry = X11Geometry {
        x: 136,
        y: 100,
        width: 800,
        height: 600,
    };
    let handle = snapshot.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));
    let window_id = fixture
        .server
        .state
        .window_id_for_x11_handle(handle)
        .expect("normal X11 window admitted");
    let _ = fixture.server.take_xwayland_backend_commands(0);
    assert!(fixture.server.state.minimize_desktop_window(window_id));
    let minimized_commands = fixture.server.take_xwayland_backend_commands(0);
    assert_eq!(
        minimized_commands
            .iter()
            .filter(|command| matches!(command, XwmCommand::Unmap(window) if *window == handle))
            .count(),
        1
    );

    assert!(fixture.server.state.apply_x11_published_state(
        handle,
        crate::xwayland::xwm::X11PublishedState {
            fullscreen: false,
            maximized: true,
            hidden: false,
            activated: false,
        },
    ));
    let transition_commands = fixture.server.take_xwayland_backend_commands(0);
    let maximized = fixture.server.state.window_geometry_for_surface_mode(
        fixture.surface_id,
        crate::compositor::ToplevelMode::Maximized,
    );
    let expected = maximized;
    let expected_x11 = X11Geometry {
        x: maximized.placement.local_x,
        y: maximized.placement.local_y,
        width: maximized.width,
        height: maximized.height,
    };
    assert_eq!(
        transition_commands
            .iter()
            .filter(|command| matches!(
                command,
                XwmCommand::ConfigureFrame { window, .. } if *window == handle
            ))
            .count(),
        1,
        "the combined restore and maximize transaction must configure once"
    );
    assert!(transition_commands.iter().any(|command| matches!(
        command,
        XwmCommand::ConfigureFrame { window, geometry, .. }
            if *window == handle && *geometry == expected_x11
    )));
    assert!(
        transition_commands
            .iter()
            .any(|command| matches!(command, XwmCommand::Map(window) if *window == handle))
    );
    assert_eq!(
        fixture
            .server
            .state
            .window(window_id)
            .expect("maximized X11 window")
            .state
            .mode(),
        crate::compositor::ToplevelMode::Maximized
    );
    assert_eq!(
        fixture
            .server
            .state
            .window(window_id)
            .expect("maximized X11 window")
            .x11_geometry
            .expect("maximized geometry")
            .frame,
        expected
    );
}

#[test]
fn minimized_maximized_to_normal_visible_uses_saved_restore_geometry() {
    let mut fixture = first_buffer_fixture();
    assert!(fixture.server.state.set_output_size(1_920, 1_080));
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    snapshot.geometry = X11Geometry {
        x: 136,
        y: 100,
        width: 800,
        height: 600,
    };
    let handle = snapshot.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));
    let window_id = fixture
        .server
        .state
        .window_id_for_x11_handle(handle)
        .expect("normal X11 window admitted");
    let original_geometry = fixture
        .server
        .state
        .window(window_id)
        .expect("admitted window")
        .x11_geometry
        .expect("normal geometry");
    let original_materialized = fixture
        .server
        .state
        .window(window_id)
        .expect("admitted window")
        .x11_materialized_client_rect;
    let original_placement = fixture.server.state.surface_placement(fixture.surface_id);
    let original_visual_geometry = fixture
        .server
        .state
        .toplevel_visual_geometries
        .get(&fixture.surface_id)
        .cloned();
    assert_eq!(
        (
            original_geometry.client.width,
            original_geometry.client.height
        ),
        (800, 600)
    );
    let _ = fixture.server.take_xwayland_backend_commands(0);

    assert!(fixture.server.state.apply_x11_published_state(
        handle,
        crate::xwayland::xwm::X11PublishedState {
            fullscreen: false,
            maximized: true,
            hidden: false,
            activated: false,
        },
    ));
    let maximize_commands = fixture.server.take_xwayland_backend_commands(0);
    assert_eq!(
        maximize_commands
            .iter()
            .filter(|command| matches!(
                command,
                XwmCommand::ConfigureFrame { window, .. } if *window == handle
            ))
            .count(),
        1
    );
    assert!(fixture.server.state.minimize_desktop_window(window_id));
    let minimize_commands = fixture.server.take_xwayland_backend_commands(0);
    assert_eq!(
        minimize_commands
            .iter()
            .filter(|command| matches!(command, XwmCommand::Unmap(window) if *window == handle))
            .count(),
        1
    );
    assert!(fixture.server.state.apply_x11_published_state(
        handle,
        crate::xwayland::xwm::X11PublishedState {
            fullscreen: false,
            maximized: false,
            hidden: false,
            activated: false,
        },
    ));
    let restore_commands = fixture.server.take_xwayland_backend_commands(0);
    assert_eq!(
        restore_commands
            .iter()
            .filter(|command| matches!(command, XwmCommand::Map(window) if *window == handle))
            .count(),
        1
    );
    assert_eq!(
        restore_commands
            .iter()
            .filter(|command| matches!(
                command,
                XwmCommand::ConfigureFrame { window, .. } if *window == handle
            ))
            .count(),
        1,
        "the combined unmaximize and restore transaction must configure once"
    );
    let original_frame = original_geometry.frame;
    let original_frame_x11 = X11Geometry {
        x: original_frame.placement.local_x,
        y: original_frame.placement.local_y,
        width: original_frame.width,
        height: original_frame.height,
    };
    assert!(restore_commands.iter().any(|command| matches!(
        command,
        XwmCommand::ConfigureFrame { window, geometry, .. }
            if *window == handle && *geometry == original_frame_x11
    )));
    let restored_window = fixture
        .server
        .state
        .window(window_id)
        .expect("restored normal X11 window");
    assert_eq!(
        restored_window.state.mode(),
        crate::compositor::ToplevelMode::Normal
    );
    assert_eq!(restored_window.x11_geometry, Some(original_geometry));
    assert_eq!(
        restored_window.x11_materialized_client_rect,
        original_materialized
    );
    assert_eq!(
        fixture.server.state.surface_placement(fixture.surface_id),
        original_placement
    );
    assert_eq!(
        fixture
            .server
            .state
            .toplevel_visual_geometries
            .get(&fixture.surface_id)
            .cloned(),
        original_visual_geometry
    );
}
