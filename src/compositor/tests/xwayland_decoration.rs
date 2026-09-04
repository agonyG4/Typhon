use super::*;
use crate::xwayland::xwm::{X11DecorationHints, X11MetadataDelta, X11MotifDecorationHint};

#[test]
fn admitted_x11_window_configures_x_to_its_persisted_frame_geometry() {
    let mut fixture = first_buffer_fixture();
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    snapshot.geometry.x = 0;
    snapshot.geometry.y = 0;

    let commands = fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot.clone()));
    let frame = fixture
        .server
        .state
        .x11_authoritative_geometry(snapshot.handle)
        .expect("admitted X11 geometry");

    assert!(commands.iter().any(|command| matches!(
        command,
        XwmCommand::ConfigureFrame {
            window,
            geometry,
            frame_extents,
            ..
        } if *window == snapshot.handle
            && geometry == &frame
            && *frame_extents == [0, 0, 26, 0]
    )));
}

#[test]
fn admitted_x11_decoration_hint_change_reconfigures_without_geometry_drift() {
    let mut fixture = first_buffer_fixture();
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    let handle = snapshot.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));
    let geometry_before = fixture
        .server
        .state
        .x11_authoritative_geometry(handle)
        .expect("admitted X11 geometry");
    let scene_generation_before = fixture.server.scene_render_generation();

    let commands = fixture
        .server
        .apply_xwayland_window_event(XwmEvent::MetadataChanged {
            window: handle,
            delta: X11MetadataDelta::DecorationHints(X11DecorationHints {
                motif: X11MotifDecorationHint::Undecorated,
                gtk_frame_extents: None,
            }),
        });

    assert!(fixture.server.scene_render_generation() > scene_generation_before);
    assert_eq!(
        fixture.server.state.x11_authoritative_geometry(handle),
        Some(geometry_before)
    );
    assert!(commands.iter().any(|command| matches!(
        command,
        XwmCommand::ConfigureFrame {
            window,
            geometry,
            frame_extents,
        } if *window == handle
            && *geometry == geometry_before
            && *frame_extents == [0; 4]
    )));
    assert!(
        fixture
            .server
            .state
            .native_decoration_render_instances(fixture.server.renderable_surfaces())
            .is_empty()
    );

    let scene_generation_before = fixture.server.scene_render_generation();
    let commands = fixture
        .server
        .apply_xwayland_window_event(XwmEvent::MetadataChanged {
            window: handle,
            delta: X11MetadataDelta::DecorationHints(X11DecorationHints::default()),
        });
    assert!(fixture.server.scene_render_generation() > scene_generation_before);
    assert!(commands.iter().any(|command| matches!(
        command,
        XwmCommand::ConfigureFrame {
            window,
            geometry,
            frame_extents,
        } if *window == handle
            && *geometry == geometry_before
            && *frame_extents == [0, 0, 26, 0]
    )));
}
