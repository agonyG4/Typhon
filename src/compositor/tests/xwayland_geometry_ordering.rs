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
                    x11_request_sequence: None,
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
    assert!(fixture.server.update_window_interaction(301.0, 241.0));

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
