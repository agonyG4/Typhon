use super::*;

#[test]
fn tiled_x11_configure_request_cannot_escape_the_layout_authority() {
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
    let window_id = fixture
        .server
        .state
        .window_id_for_x11_handle(handle)
        .expect("X11 window id");
    let location = fixture
        .server
        .state
        .window(window_id)
        .expect("X11 window")
        .management
        .expect("management")
        .location();
    fixture
        .server
        .state
        .tiled_layout
        .insert(
            location,
            window_id,
            crate::wm::layout::InsertHint::default(),
        )
        .expect("tiled tree insert");
    fixture
        .server
        .state
        .window_mut(window_id)
        .expect("X11 window")
        .management = Some(
        crate::wm::WindowManagementState::new(location)
            .with_layout(crate::wm::LayoutMembership::Tiled),
    );

    let commands = fixture
        .server
        .apply_xwayland_window_event(XwmEvent::ConfigureRequested {
            window: handle,
            request: X11ConfigureRequest {
                requested: X11Geometry {
                    x: 1,
                    y: 2,
                    width: 1_920,
                    height: 1_080,
                },
                fields: X11ConfigureFlags {
                    x: true,
                    y: true,
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

    let authoritative = fixture
        .server
        .state
        .x11_authoritative_geometry(handle)
        .expect("authoritative geometry");
    assert!(matches!(
        commands.as_slice(),
        [XwmCommand::Configure { geometry, .. }] if *geometry == authoritative
    ));
    assert_eq!(
        fixture.server.state.x11_authoritative_geometry(handle),
        Some(authoritative)
    );
}
