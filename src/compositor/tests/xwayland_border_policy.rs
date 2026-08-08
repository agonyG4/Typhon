use super::*;

#[test]
fn managed_x11_configure_request_normalizes_nonzero_border_width() {
    let mut fixture = first_buffer_fixture();
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    let handle = snapshot.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot.clone()));

    let commands = fixture
        .server
        .apply_xwayland_window_event(XwmEvent::ConfigureRequested {
            window: handle,
            request: X11ConfigureRequest {
                requested: snapshot.geometry,
                fields: X11ConfigureFlags {
                    border_width: true,
                    ..X11ConfigureFlags::default()
                },
                client_event_sequence: None,
                border_width: 7,
                sibling: None,
                stack_mode: None,
            },
        });

    assert!(commands.iter().any(|command| matches!(
        command,
        XwmCommand::Configure {
            window,
            fields,
            border_width,
            ..
        } if *window == handle && fields.border_width && *border_width == 0
    )));
}
