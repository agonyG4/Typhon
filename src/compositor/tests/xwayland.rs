use std::{num::NonZeroU64, path::PathBuf};

use crate::compositor::{DesktopWindowKind, WindowConstraints, WindowMetadata};
use crate::xwayland::xwm::{
    X11Geometry, X11PublishedState, X11WindowLifecycle, X11WindowSnapshot, XwmCommand, XwmEvent,
};
use crate::xwayland::{X11WindowHandle, XwaylandGeneration};

fn fake_snapshot() -> X11WindowSnapshot {
    X11WindowSnapshot {
        handle: X11WindowHandle::new(
            XwaylandGeneration::new(NonZeroU64::new(1).expect("nonzero")),
            100,
        ),
        surface_id: 7,
        kind: DesktopWindowKind::Managed,
        geometry: X11Geometry {
            x: 0,
            y: 0,
            width: 800,
            height: 600,
        },
        metadata: WindowMetadata {
            app_id: Some("fake-x11-client".into()),
            title: Some("Fake X11 client".into()),
            pid: None,
        },
        constraints: WindowConstraints::default(),
        state: X11PublishedState::default(),
        transient_for: None,
        supports_delete: true,
        supports_take_focus: true,
        accepts_input: Some(true),
        window_role: None,
        startup_id: None,
        user_time: None,
        urgency: false,
        sync_counter: None,
    }
}

#[test]
fn xwayland_map_lifecycle_keeps_rendering_after_x11_map() {
    assert_ne!(
        X11WindowLifecycle::MapCommanded,
        X11WindowLifecycle::Renderable
    );
    assert_ne!(
        X11WindowLifecycle::MappedAwaitingAssociation,
        X11WindowLifecycle::Renderable
    );
}

#[test]
fn managed_window_receives_exactly_one_map_command_from_fake_x11_events() {
    let socket =
        std::env::temp_dir().join(format!("typhon-xwayland-map-test-{}", std::process::id()));
    let mut server = super::OwnCompositorServer::bind_cpu_composition(
        PathBuf::from(&socket).to_string_lossy().into_owned(),
    )
    .expect("bind fake compositor server");
    let snapshot = fake_snapshot();
    let first = server.apply_xwayland_window_event(XwmEvent::WindowMapRequested(snapshot.handle));
    assert_eq!(
        first
            .iter()
            .filter(|command| matches!(command, XwmCommand::Map(_)))
            .count(),
        1
    );
    let ready = server.apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));
    assert_eq!(
        ready
            .iter()
            .filter(|command| matches!(command, XwmCommand::Map(_)))
            .count(),
        0
    );
    drop(server);
    let _ = std::fs::remove_file(socket);
}

#[test]
fn override_redirect_never_receives_managed_map_command() {
    let socket =
        std::env::temp_dir().join(format!("typhon-xwayland-override-{}", std::process::id()));
    let mut server = super::OwnCompositorServer::bind_cpu_composition(
        PathBuf::from(&socket).to_string_lossy().into_owned(),
    )
    .expect("bind fake compositor server");
    let mut snapshot = fake_snapshot();
    snapshot.kind = DesktopWindowKind::OverrideRedirect;
    let commands = server.apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, XwmCommand::Map(_)))
    );
    drop(server);
    let _ = std::fs::remove_file(socket);
}
