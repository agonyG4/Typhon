use super::*;
use crate::xwayland::xwm::{
    X11DecorationHints, X11FrameExtents, X11MetadataDelta, X11MotifDecorationHint,
};

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

#[test]
fn x11_mode_transitions_publish_frame_extents_with_the_single_geometry_configure() {
    let mut fixture = first_buffer_fixture();
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    let handle = snapshot.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));
    let _ = fixture.server.take_xwayland_backend_commands(0);
    let normal_geometry = fixture
        .server
        .state
        .x11_authoritative_geometry(handle)
        .expect("normal X11 geometry");

    assert!(fixture.server.state.toggle_fullscreen_focused_window());
    let fullscreen_geometry = fixture.server.state.fullscreen_window_geometry();
    let fullscreen_commands = fixture.server.take_xwayland_backend_commands(0);
    assert_eq!(
        fullscreen_commands
            .iter()
            .filter(|command| matches!(command, XwmCommand::ConfigureFrame { window, .. } if *window == handle))
            .count(),
        1
    );
    assert!(fullscreen_commands.iter().all(|command| {
        !matches!(command, XwmCommand::Configure { window, .. } if *window == handle)
    }));
    assert!(fullscreen_commands.iter().any(|command| matches!(
        command,
        XwmCommand::ConfigureFrame {
            window,
            geometry,
            frame_extents,
        } if *window == handle
            && *geometry == X11Geometry {
                x: fullscreen_geometry.placement.local_x,
                y: fullscreen_geometry.placement.local_y,
                width: fullscreen_geometry.width,
                height: fullscreen_geometry.height,
            }
            && *frame_extents == [0; 4]
    )));

    assert!(fixture.server.state.toggle_fullscreen_focused_window());
    let restore_commands = fixture.server.take_xwayland_backend_commands(0);
    assert_eq!(
        restore_commands
            .iter()
            .filter(|command| matches!(command, XwmCommand::ConfigureFrame { window, .. } if *window == handle))
            .count(),
        1
    );
    assert!(restore_commands.iter().all(|command| {
        !matches!(command, XwmCommand::Configure { window, .. } if *window == handle)
    }));
    assert!(restore_commands.iter().any(|command| matches!(
        command,
        XwmCommand::ConfigureFrame {
            window,
            geometry,
            frame_extents,
        } if *window == handle
            && *geometry == normal_geometry
            && *frame_extents == [0, 0, 26, 0]
    )));
    assert_eq!(
        fixture.server.state.x11_authoritative_geometry(handle),
        Some(normal_geometry)
    );
}

#[test]
fn x11_fullscreen_restore_keeps_zero_extents_after_motif_change() {
    let mut fixture = first_buffer_fixture();
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    let handle = snapshot.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));
    let _ = fixture.server.take_xwayland_backend_commands(0);

    assert!(fixture.server.state.toggle_fullscreen_focused_window());
    let _ = fixture.server.take_xwayland_backend_commands(0);
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::MetadataChanged {
            window: handle,
            delta: X11MetadataDelta::DecorationHints(X11DecorationHints {
                motif: X11MotifDecorationHint::Undecorated,
                gtk_frame_extents: None,
            }),
        });
    let _ = fixture.server.take_xwayland_backend_commands(0);

    assert!(fixture.server.state.toggle_fullscreen_focused_window());
    let commands = fixture.server.take_xwayland_backend_commands(0);
    assert!(commands.iter().any(|command| matches!(
        command,
        XwmCommand::ConfigureFrame {
            window,
            frame_extents,
            ..
        } if *window == handle && *frame_extents == [0; 4]
    )));
    assert_eq!(
        fixture.server.state.x11_effective_decoration_mode(handle),
        crate::compositor::decoration::types::DecorationMode::None
    );
}

#[test]
fn x11_fullscreen_restore_keeps_zero_extents_after_gtk_change() {
    let mut fixture = first_buffer_fixture();
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    let handle = snapshot.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));
    let _ = fixture.server.take_xwayland_backend_commands(0);

    assert!(fixture.server.state.toggle_fullscreen_focused_window());
    let _ = fixture.server.take_xwayland_backend_commands(0);
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::MetadataChanged {
            window: handle,
            delta: X11MetadataDelta::DecorationHints(X11DecorationHints {
                motif: X11MotifDecorationHint::Unspecified,
                gtk_frame_extents: Some(X11FrameExtents {
                    left: 2,
                    right: 2,
                    top: 28,
                    bottom: 2,
                }),
            }),
        });
    let _ = fixture.server.take_xwayland_backend_commands(0);

    assert!(fixture.server.state.toggle_fullscreen_focused_window());
    let commands = fixture.server.take_xwayland_backend_commands(0);
    assert!(commands.iter().any(|command| matches!(
        command,
        XwmCommand::ConfigureFrame {
            window,
            frame_extents,
            ..
        } if *window == handle && *frame_extents == [0; 4]
    )));
    assert_eq!(
        fixture.server.state.x11_effective_decoration_mode(handle),
        crate::compositor::decoration::types::DecorationMode::ClientSide
    );
}

#[test]
fn x11_maximize_restore_publishes_the_canonical_extents_without_geometry_drift() {
    let mut fixture = first_buffer_fixture();
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    let handle = snapshot.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));
    let _ = fixture.server.take_xwayland_backend_commands(0);
    let normal_geometry = fixture
        .server
        .state
        .x11_authoritative_geometry(handle)
        .expect("normal X11 geometry");

    assert!(fixture.server.state.toggle_maximize_focused_window());
    let maximize_commands = fixture.server.take_xwayland_backend_commands(0);
    let maximized_extents = fixture.server.state.x11_decoration_frame_extents(handle);
    assert!(maximize_commands.iter().any(|command| matches!(
        command,
        XwmCommand::ConfigureFrame {
            window,
            frame_extents,
            ..
        } if *window == handle && *frame_extents == maximized_extents
    )));
    assert!(maximize_commands.iter().all(|command| {
        !matches!(command, XwmCommand::Configure { window, .. } if *window == handle)
    }));

    assert!(fixture.server.state.toggle_maximize_focused_window());
    let restore_commands = fixture.server.take_xwayland_backend_commands(0);
    let normal_extents = fixture.server.state.x11_decoration_frame_extents(handle);
    assert!(restore_commands.iter().any(|command| matches!(
        command,
        XwmCommand::ConfigureFrame {
            window,
            frame_extents,
            ..
        } if *window == handle && *frame_extents == normal_extents
    )));
    assert_eq!(
        fixture.server.state.x11_authoritative_geometry(handle),
        Some(normal_geometry)
    );
}

#[test]
fn decoration_metadata_does_not_reconcile_x11_structure() {
    let mut fixture = first_buffer_fixture();
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    let handle = snapshot.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));
    let _ = fixture.server.take_xwayland_backend_commands(0);
    let window_id = fixture
        .server
        .state
        .window_id_for_x11_handle(handle)
        .expect("managed X11 window");
    let structural_before = fixture.server.state.window(window_id).map(|window| {
        (
            window.x11_role,
            window.x11_placement_policy,
            window.stack_layer,
            window.relationships,
            window.management.map(|management| {
                (
                    management.location(),
                    management.layout(),
                    management.chrome_policy(),
                )
            }),
        )
    });
    let client_lists_before = fixture.server.state.x11_client_lists();

    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::MetadataChanged {
            window: handle,
            delta: X11MetadataDelta::DecorationHints(X11DecorationHints {
                motif: X11MotifDecorationHint::Undecorated,
                gtk_frame_extents: None,
            }),
        });

    let structural_after = fixture.server.state.window(window_id).map(|window| {
        (
            window.x11_role,
            window.x11_placement_policy,
            window.stack_layer,
            window.relationships,
            window.management.map(|management| {
                (
                    management.location(),
                    management.layout(),
                    management.chrome_policy(),
                )
            }),
        )
    });
    assert_eq!(structural_after, structural_before);
    assert_eq!(fixture.server.state.x11_client_lists(), client_lists_before);
}
