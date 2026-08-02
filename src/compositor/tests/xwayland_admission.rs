use super::*;

#[test]
fn stale_generation_admission_rejection_is_not_popup_cancellation() {
    let mut fixture = super::first_buffer_fixture();
    let mut snapshot = super::fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    snapshot.handle = X11WindowHandle::new(
        XwaylandGeneration::new(NonZeroU64::new(2).expect("nonzero generation")),
        snapshot.handle.xid(),
    );

    let _ = fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));

    assert_eq!(
        fixture
            .server
            .xwayland_scene_metrics()
            .pre_admission_popup_cancellations,
        0
    );
}

#[test]
fn duplicate_window_admission_rejection_is_not_popup_cancellation() {
    let mut fixture = super::first_buffer_fixture();
    super::admit_first_buffer(&mut fixture, 0, 0);
    let mut duplicate = super::fake_snapshot();
    duplicate.surface_id = fixture.surface_id;

    let _ = fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(duplicate));

    assert_eq!(
        fixture
            .server
            .xwayland_scene_metrics()
            .pre_admission_popup_cancellations,
        0
    );
}

#[test]
fn invalid_surface_admission_rejection_is_not_popup_cancellation() {
    let socket = super::super::unique_socket_name();
    let mut server = super::super::OwnCompositorServer::bind_cpu_composition(&socket)
        .expect("bind fake compositor server");
    let mut snapshot = super::fake_snapshot();
    snapshot.surface_id = 0;

    let _ = server.apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));

    assert_eq!(
        server
            .xwayland_scene_metrics()
            .pre_admission_popup_cancellations,
        0
    );
}

#[test]
fn unknown_x11_cleanup_does_not_count_as_popup_redundant_cleanup() {
    let socket = super::super::unique_socket_name();
    let mut server = super::super::OwnCompositorServer::bind_cpu_composition(&socket)
        .expect("bind fake compositor server");
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());

    let _ = server.apply_xwayland_window_event(XwmEvent::WindowDestroyed(X11WindowHandle::new(
        generation, 901,
    )));
    let _ = server.apply_xwayland_window_event(XwmEvent::WindowWithdrawn(X11WindowHandle::new(
        generation, 901,
    )));

    assert_eq!(
        server
            .xwayland_scene_metrics()
            .popup_lifecycle_redundant_cleanup,
        0
    );
}

#[test]
fn popup_role_redundant_cleanup_counts_once_at_role_owner() {
    let socket = super::super::unique_socket_name();
    let mut server = super::super::OwnCompositorServer::bind_cpu_composition(&socket)
        .expect("bind fake compositor server");

    server.state.unregister_popup_surface(902);

    assert_eq!(
        server
            .xwayland_scene_metrics()
            .popup_lifecycle_redundant_cleanup,
        1
    );
}
