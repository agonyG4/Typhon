use super::*;
use crate::compositor::{WindowGeometry, WindowInteractionKind, WindowInteractionSource};

#[test]
fn queued_final_resize_cannot_acquire_a_newer_resize_epoch() {
    let mut fixture = first_buffer_fixture();
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    let initial_geometry = X11Geometry {
        x: 100,
        y: 100,
        width: 640,
        height: 480,
    };
    snapshot.geometry = initial_geometry;
    let handle = snapshot.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));
    let window_id = fixture
        .server
        .state
        .window_id_for_x11_handle(handle)
        .expect("admitted X11 window");
    let _ = fixture.server.take_xwayland_backend_commands(0);

    let edges = crate::compositor::ResizeEdges::new(false, false, false, true);
    assert!(fixture.server.state.begin_window_interaction_for_root(
        crate::compositor::BeginWindowInteraction::for_test(
            Some(window_id),
            fixture.surface_id,
            0.0,
            0.0,
            WindowInteractionKind::Resize(edges),
            WindowInteractionSource::NativeBinding,
            Some(fixture.surface_id),
        )
    ));
    assert!(fixture.server.state.update_window_interaction(30.0, 0.0));
    assert!(
        fixture
            .server
            .state
            .flush_pending_floating_interaction_geometry()
    );
    let first_epoch = fixture
        .server
        .state
        .x11_resize_interaction_epoch(handle)
        .expect("first resize interaction epoch");
    let first_visual = fixture
        .server
        .state
        .current_visual_root_window_geometry(fixture.surface_id)
        .expect("first resize visual geometry");
    let first_geometry = X11Geometry {
        x: first_visual.placement.local_x,
        y: first_visual.placement.local_y,
        width: first_visual.width,
        height: first_visual.height,
    };

    fixture.server.state.end_window_interaction();
    assert!(
        fixture
            .server
            .state
            .has_pending_x11_resize_backend_command(handle, first_epoch)
    );
    assert_eq!(
        fixture.server.state.x11_resize_interaction_epoch(handle),
        Some(first_epoch),
        "the queued final still belongs to the first epoch before E2 begins"
    );

    assert!(fixture.server.state.begin_window_interaction_for_root(
        crate::compositor::BeginWindowInteraction::for_test(
            Some(window_id),
            fixture.surface_id,
            0.0,
            0.0,
            WindowInteractionKind::Resize(edges),
            WindowInteractionSource::NativeBinding,
            Some(fixture.surface_id),
        )
    ));
    let second_interaction_epoch = fixture
        .server
        .window_interaction_debug_snapshot()
        .and_then(|snapshot| snapshot.resize_interaction_id)
        .expect("second resize interaction epoch");
    assert_ne!(second_interaction_epoch, first_epoch);
    assert!(
        !fixture
            .server
            .state
            .has_pending_x11_resize_backend_command(handle, second_interaction_epoch),
        "queued E1 work must not keep E2 alive before E2 queues local work"
    );
    assert!(fixture.server.state.update_window_interaction(80.0, 0.0));
    assert!(
        fixture
            .server
            .state
            .flush_pending_floating_interaction_geometry()
    );
    let second_epoch = fixture
        .server
        .state
        .x11_resize_interaction_epoch(handle)
        .expect("second resize owns the preview");
    assert_ne!(second_epoch, first_epoch);

    let commands = fixture.server.take_xwayland_backend_commands(0);
    assert!(
        !commands.iter().any(|command| matches!(
            command,
            crate::xwayland::xwm::XwmCommand::BeginResizeSync {
                geometry,
                final_pending: true,
                resize_epoch: Some(command_epoch),
                ..
            } if *geometry == first_geometry && *command_epoch == second_epoch
        )),
        "a queued final from E1 must not be relabeled as E2"
    );
}

#[test]
fn queued_intermediate_resize_cannot_acquire_a_newer_resize_epoch() {
    let mut fixture = first_buffer_fixture();
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    snapshot.geometry = X11Geometry {
        x: 100,
        y: 100,
        width: 640,
        height: 480,
    };
    let handle = snapshot.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));
    let window_id = fixture
        .server
        .state
        .window_id_for_x11_handle(handle)
        .expect("admitted X11 window");
    let _ = fixture.server.take_xwayland_backend_commands(0);

    let edges = crate::compositor::ResizeEdges::new(false, false, false, true);
    assert!(fixture.server.state.begin_window_interaction_for_root(
        crate::compositor::BeginWindowInteraction::for_test(
            Some(window_id),
            fixture.surface_id,
            0.0,
            0.0,
            WindowInteractionKind::Resize(edges),
            WindowInteractionSource::NativeBinding,
            Some(fixture.surface_id),
        )
    ));
    assert!(fixture.server.state.update_window_interaction(40.0, 0.0));
    assert!(
        fixture
            .server
            .state
            .flush_pending_floating_interaction_geometry()
    );
    let first_epoch = fixture
        .server
        .state
        .x11_resize_interaction_epoch(handle)
        .expect("first resize owns the preview");
    let first_visual = fixture
        .server
        .state
        .current_visual_root_window_geometry(fixture.surface_id)
        .expect("first resize visual geometry");
    let first_geometry = X11Geometry {
        x: first_visual.placement.local_x,
        y: first_visual.placement.local_y,
        width: first_visual.width,
        height: first_visual.height,
    };

    fixture.server.state.end_window_interaction();
    assert!(fixture.server.state.begin_window_interaction_for_root(
        crate::compositor::BeginWindowInteraction::for_test(
            Some(window_id),
            fixture.surface_id,
            0.0,
            0.0,
            WindowInteractionKind::Resize(edges),
            WindowInteractionSource::NativeBinding,
            Some(fixture.surface_id),
        )
    ));
    assert!(fixture.server.state.update_window_interaction(80.0, 0.0));
    assert!(
        fixture
            .server
            .state
            .flush_pending_floating_interaction_geometry()
    );
    let second_epoch = fixture
        .server
        .state
        .x11_resize_interaction_epoch(handle)
        .expect("second resize owns the preview");
    assert_ne!(second_epoch, first_epoch);

    let commands = fixture.server.take_xwayland_backend_commands(0);
    assert!(
        !commands.iter().any(|command| matches!(
            command,
            crate::xwayland::xwm::XwmCommand::BeginResizeSync {
                geometry,
                final_pending: false,
                resize_epoch: Some(command_epoch),
                ..
            } if *geometry == first_geometry && *command_epoch == second_epoch
        )),
        "an intermediate configure from E1 must not be relabeled as E2"
    );

    let second_visual = fixture
        .server
        .state
        .current_visual_root_window_geometry(fixture.surface_id)
        .expect("second resize visual geometry");
    fixture.server.state.queue_backend_configure(
        window_id,
        second_visual,
        crate::compositor::ToplevelMode::Normal,
        true,
    );
    assert!(
        fixture
            .server
            .state
            .has_pending_x11_resize_backend_command(handle, second_epoch)
    );
    assert!(
        !fixture
            .server
            .state
            .has_pending_x11_resize_backend_command(handle, first_epoch),
        "queued E2 work must not keep E1 alive"
    );
}

#[test]
fn queued_position_only_configure_cannot_acquire_a_newer_resize_epoch() {
    let mut fixture = first_buffer_fixture();
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    snapshot.geometry = X11Geometry {
        x: 100,
        y: 100,
        width: 640,
        height: 480,
    };
    let handle = snapshot.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));
    let window_id = fixture
        .server
        .state
        .window_id_for_x11_handle(handle)
        .expect("admitted X11 window");
    let _ = fixture.server.take_xwayland_backend_commands(0);

    let edges = crate::compositor::ResizeEdges::new(false, false, false, true);
    assert!(fixture.server.state.begin_window_interaction_for_root(
        crate::compositor::BeginWindowInteraction::for_test(
            Some(window_id),
            fixture.surface_id,
            0.0,
            0.0,
            WindowInteractionKind::Resize(edges),
            WindowInteractionSource::NativeBinding,
            Some(fixture.surface_id),
        )
    ));
    assert!(fixture.server.state.update_window_interaction(30.0, 0.0));
    assert!(
        fixture
            .server
            .state
            .flush_pending_floating_interaction_geometry()
    );
    let first_epoch = fixture
        .server
        .state
        .x11_resize_interaction_epoch(handle)
        .expect("first resize owns the preview");
    let stale_geometry =
        WindowGeometry::new(SurfacePlacement::absolute_root_at(400, 300), 640, 480);
    let stale_x11_geometry = X11Geometry {
        x: stale_geometry.placement.local_x,
        y: stale_geometry.placement.local_y,
        width: stale_geometry.width,
        height: stale_geometry.height,
    };
    fixture.server.state.queue_backend_configure(
        window_id,
        stale_geometry,
        crate::compositor::ToplevelMode::Normal,
        false,
    );
    fixture.server.state.end_window_interaction();

    assert!(fixture.server.state.begin_window_interaction_for_root(
        crate::compositor::BeginWindowInteraction::for_test(
            Some(window_id),
            fixture.surface_id,
            0.0,
            0.0,
            WindowInteractionKind::Resize(edges),
            WindowInteractionSource::NativeBinding,
            Some(fixture.surface_id),
        )
    ));
    assert!(fixture.server.state.update_window_interaction(80.0, 0.0));
    assert!(
        fixture
            .server
            .state
            .flush_pending_floating_interaction_geometry()
    );
    let second_epoch = fixture
        .server
        .state
        .x11_resize_interaction_epoch(handle)
        .expect("second resize owns the preview");
    assert_ne!(second_epoch, first_epoch);

    let commands = fixture.server.take_xwayland_backend_commands(0);
    assert!(!commands.iter().any(|command| matches!(
        command,
        XwmCommand::ConfigureFrame { geometry, .. }
            if *geometry == stale_x11_geometry
    )));
    assert!(!commands.iter().any(|command| matches!(
        command,
        XwmCommand::Configure {
            geometry,
            resize_epoch: Some(command_epoch),
            ..
        } if *geometry == stale_x11_geometry && *command_epoch == first_epoch
    )));
    assert!(!commands.iter().any(|command| matches!(
        command,
        XwmCommand::Configure {
            geometry,
            resize_epoch: Some(command_epoch),
            ..
        } if *geometry == stale_x11_geometry && *command_epoch == second_epoch
    )));
}

#[test]
fn ordinary_configure_queued_before_resize_does_not_acquire_its_epoch() {
    let mut fixture = first_buffer_fixture();
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    snapshot.geometry = X11Geometry {
        x: 100,
        y: 100,
        width: 640,
        height: 480,
    };
    let handle = snapshot.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));
    let window_id = fixture
        .server
        .state
        .window_id_for_x11_handle(handle)
        .expect("admitted X11 window");
    let _ = fixture.server.take_xwayland_backend_commands(0);
    let ordinary_geometry =
        WindowGeometry::new(SurfacePlacement::absolute_root_at(400, 300), 640, 480);
    let ordinary_x11_geometry = X11Geometry {
        x: ordinary_geometry.placement.local_x,
        y: ordinary_geometry.placement.local_y,
        width: ordinary_geometry.width,
        height: ordinary_geometry.height,
    };
    fixture.server.state.queue_backend_configure(
        window_id,
        ordinary_geometry,
        crate::compositor::ToplevelMode::Normal,
        false,
    );

    let edges = crate::compositor::ResizeEdges::new(false, false, false, true);
    assert!(fixture.server.state.begin_window_interaction_for_root(
        crate::compositor::BeginWindowInteraction::for_test(
            Some(window_id),
            fixture.surface_id,
            0.0,
            0.0,
            WindowInteractionKind::Resize(edges),
            WindowInteractionSource::NativeBinding,
            Some(fixture.surface_id),
        )
    ));
    let resize_epoch = fixture
        .server
        .window_interaction_debug_snapshot()
        .and_then(|snapshot| snapshot.resize_interaction_id)
        .expect("resize interaction epoch");
    assert!(fixture.server.state.preview_resize_root_window_to(
        fixture.surface_id,
        600,
        480,
        SurfacePlacement::absolute_root_at(100, 100),
        edges,
        ResizeInteractionId::new(resize_epoch),
    ));

    let commands = fixture.server.take_xwayland_backend_commands(0);
    assert!(
        commands.iter().any(|command| matches!(
            command,
            crate::xwayland::xwm::XwmCommand::ConfigureFrame { geometry, .. }
                if *geometry == ordinary_x11_geometry
        )),
        "a pre-resize ordinary configure remains unassociated"
    );
    assert!(!commands.iter().any(|command| matches!(
        command,
        crate::xwayland::xwm::XwmCommand::Configure {
            geometry,
            resize_epoch: Some(command_epoch),
            ..
        } if *command_epoch == resize_epoch && *geometry == ordinary_x11_geometry
    )));
}

#[test]
fn move_after_resize_release_cannot_be_dropped_by_resize_timeout() {
    let mut fixture = first_buffer_fixture();
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    snapshot.geometry = X11Geometry {
        x: 100,
        y: 100,
        width: 640,
        height: 480,
    };
    let handle = snapshot.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));

    let initial_visual = fixture
        .server
        .state
        .current_visual_root_window_geometry(fixture.surface_id)
        .expect("initial visual geometry");
    let resize_edges = crate::compositor::ResizeEdges::new(false, false, true, false);
    assert!(
        fixture.server.state.begin_window_interaction_for_root(
            crate::compositor::BeginWindowInteraction::for_test(
                fixture
                    .server
                    .state
                    .window_id_for_surface(fixture.surface_id),
                fixture.surface_id,
                0.0,
                0.0,
                WindowInteractionKind::Resize(resize_edges),
                WindowInteractionSource::NativeBinding,
                Some(fixture.surface_id),
            )
        )
    );
    assert!(fixture.server.state.update_window_interaction(20.0, 0.0));
    fixture.server.state.end_window_interaction();

    let sealed = fixture
        .server
        .state
        .current_visual_root_window_geometry(fixture.surface_id)
        .expect("sealed resize geometry");
    let resized_width = initial_visual.width.saturating_sub(20).max(160);
    let expected_resize = SurfacePlacement::absolute_root_at(
        initial_visual
            .placement
            .local_x
            .saturating_add(i32::try_from(initial_visual.width).unwrap())
            .saturating_sub(i32::try_from(resized_width).unwrap()),
        initial_visual.placement.local_y,
    );
    assert_eq!(sealed.placement, expected_resize);
    let resize_epoch = fixture
        .server
        .state
        .x11_resize_interaction_epoch(handle)
        .expect("released resize epoch stays associated with its preview");

    assert!(
        fixture.server.state.begin_window_interaction_for_root(
            crate::compositor::BeginWindowInteraction::for_test(
                fixture
                    .server
                    .state
                    .window_id_for_surface(fixture.surface_id),
                fixture.surface_id,
                0.0,
                0.0,
                WindowInteractionKind::Move,
                WindowInteractionSource::NativeBinding,
                Some(fixture.surface_id),
            )
        )
    );
    assert!(fixture.server.state.update_window_interaction(180.0, 0.0));
    fixture.server.state.end_window_interaction();
    let expected_move = SurfacePlacement::absolute_root_at(
        sealed.placement.local_x.saturating_add(180),
        sealed.placement.local_y,
    );
    let expected_move_geometry = WindowGeometry::new(expected_move, sealed.width, sealed.height);
    assert_eq!(
        fixture.server.state.surface_placement(fixture.surface_id),
        expected_move
    );
    assert!(fixture.server.state.backend_commands.iter().any(|command| matches!(
        command,
        crate::compositor::window_backend::WindowBackendCommand::Configure {
            window: command_window,
            geometry,
            resizing: false,
            resize_epoch: Some(command_epoch),
            ..
        } if *command_window == fixture.server.state.window_id_for_x11_handle(handle).expect("moved X11 window")
            && *geometry == expected_move_geometry
            && *command_epoch == resize_epoch
    )), "post-resize move must retain its enqueue-time resize context until dequeue");

    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::ResizeSyncTimedOut {
            window: handle,
            fallback_geometry: Some(X11Geometry {
                x: sealed.placement.local_x,
                y: sealed.placement.local_y,
                width: sealed.width,
                height: sealed.height,
            }),
            resize_epoch: Some(resize_epoch),
            has_followup: false,
        });

    let expected_move_x11_geometry = X11Geometry {
        x: expected_move.local_x,
        y: expected_move.local_y,
        width: expected_move_geometry.width,
        height: expected_move_geometry.height,
    };
    assert_eq!(
        fixture.server.state.x11_authoritative_geometry(handle),
        Some(expected_move_x11_geometry)
    );
    assert_eq!(
        fixture
            .server
            .state
            .current_visual_root_window_geometry(fixture.surface_id)
            .expect("current visual geometry"),
        expected_move_geometry
    );
    assert_eq!(
        fixture.server.state.x11_resize_interaction_epoch(handle),
        None,
        "timeout must retire a resize already superseded by a move"
    );

    let commands = fixture.server.take_xwayland_backend_commands(0);
    assert!(commands.iter().any(|command| matches!(
        command,
        XwmCommand::ConfigureFrame { geometry, .. }
            if *geometry == expected_move_x11_geometry
    )));
}
