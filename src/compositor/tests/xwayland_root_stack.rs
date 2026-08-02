use super::*;

#[test]
fn override_redirect_configure_notify_requests_reconciliation_without_restaking() {
    let socket = super::super::unique_socket_name();
    let mut server = super::super::OwnCompositorServer::bind_cpu_composition(&socket)
        .expect("bind fake compositor server");
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let mut first = fake_snapshot();
    first.kind = DesktopWindowKind::OverrideRedirect;
    first.override_redirect = true;
    let mut second = first.clone();
    second.handle = X11WindowHandle::new(generation, 103);
    second.surface_id = 10;
    let first_id = server.state.allocate_window_id().expect("first window id");
    let second_id = server.state.allocate_window_id().expect("second window id");
    server
        .state
        .insert_desktop_window(crate::compositor::DesktopWindow::new_x11(
            first_id,
            first.clone(),
        ))
        .expect("insert first OR window");
    server
        .state
        .insert_desktop_window(crate::compositor::DesktopWindow::new_x11(
            second_id,
            second.clone(),
        ))
        .expect("insert second OR window");

    let original_stacking = server.state.window_stacking.clone();
    let commands = server.apply_xwayland_window_event(XwmEvent::ConfigureNotify {
        window: first.handle,
        geometry: first.geometry,
        above_sibling: Some(second.handle),
    });

    assert_eq!(server.state.window_stacking, original_stacking);
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, XwmCommand::RestackExact { .. }))
    );
    assert_eq!(
        server
            .xwayland_scene_metrics()
            .override_redirect_restack_writebacks_prevented,
        1
    );
}

#[test]
fn override_redirect_configure_notify_without_sibling_does_not_write_back_observed_order() {
    let socket = super::super::unique_socket_name();
    let mut server = super::super::OwnCompositorServer::bind_cpu_composition(&socket)
        .expect("bind fake compositor server");
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let mut first = fake_snapshot();
    first.kind = DesktopWindowKind::OverrideRedirect;
    first.override_redirect = true;
    let mut second = first.clone();
    second.handle = X11WindowHandle::new(generation, 104);
    second.surface_id = 11;
    let first_id = server.state.allocate_window_id().expect("first window id");
    let second_id = server.state.allocate_window_id().expect("second window id");
    server
        .state
        .insert_desktop_window(crate::compositor::DesktopWindow::new_x11(
            first_id,
            first.clone(),
        ))
        .expect("insert first OR window");
    server
        .state
        .insert_desktop_window(crate::compositor::DesktopWindow::new_x11(
            second_id,
            second.clone(),
        ))
        .expect("insert second OR window");

    let original_stacking = server.state.window_stacking.clone();
    let commands = server.apply_xwayland_window_event(XwmEvent::ConfigureNotify {
        window: second.handle,
        geometry: second.geometry,
        above_sibling: None,
    });

    assert_eq!(server.state.window_stacking, original_stacking);
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, XwmCommand::RestackExact { .. }))
    );
    assert_eq!(
        server
            .xwayland_scene_metrics()
            .override_redirect_restack_writebacks_prevented,
        0
    );
}

#[test]
fn override_redirect_snapshot_follows_root_order_without_changing_managed_order() {
    let socket = super::super::unique_socket_name();
    let mut server = super::super::OwnCompositorServer::bind_cpu_composition(&socket)
        .expect("bind fake compositor server");
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let _xwayland_peer = install_test_xwayland_identity(&mut server, generation);
    let managed = fake_snapshot();
    let mut first = managed.clone();
    first.handle = X11WindowHandle::new(generation, 105);
    first.surface_id = 12;
    first.kind = DesktopWindowKind::OverrideRedirect;
    first.override_redirect = true;
    let mut second = first.clone();
    second.handle = X11WindowHandle::new(generation, 106);
    second.surface_id = 13;
    let managed_id = server.state.allocate_window_id().expect("managed id");
    let first_id = server.state.allocate_window_id().expect("first id");
    let second_id = server.state.allocate_window_id().expect("second id");
    for (id, snapshot) in [
        (managed_id, managed.clone()),
        (first_id, first.clone()),
        (second_id, second.clone()),
    ] {
        server
            .state
            .insert_desktop_window(crate::compositor::DesktopWindow::new_x11(id, snapshot))
            .expect("insert X11 window");
    }

    server.apply_xwayland_window_event(XwmEvent::OverrideRedirectStackSnapshot {
        generation,
        epoch: 1,
        bottom_to_top: vec![second.handle, first.handle],
    });
    assert_eq!(
        server.state.window_stacking,
        vec![managed_id, second_id, first_id]
    );

    server.apply_xwayland_window_event(XwmEvent::OverrideRedirectStackSnapshot {
        generation,
        epoch: 2,
        bottom_to_top: vec![first.handle, second.handle],
    });
    assert_eq!(
        server.state.window_stacking,
        vec![managed_id, first_id, second_id]
    );
}

#[test]
fn snapshot_before_override_redirect_admission_is_applied_at_batch_commit() {
    let mut fixture = stationary_pointer_xwayland_fixture();
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let mut first = fake_snapshot();
    first.handle = X11WindowHandle::new(generation, 110);
    first.surface_id = fixture.parent_surface_id;
    first.kind = DesktopWindowKind::OverrideRedirect;
    first.override_redirect = true;
    let mut second = first.clone();
    second.handle = X11WindowHandle::new(generation, 111);
    second.surface_id = fixture.popup_surface_id;

    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(first.clone()));
    let first_id = fixture
        .server
        .state
        .window_id_for_x11_handle(first.handle)
        .expect("first override-redirect id");
    let token = fixture
        .server
        .begin_xwayland_scene_batch()
        .expect("begin snapshot admission batch");
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::OverrideRedirectStackSnapshot {
            generation,
            epoch: 1,
            bottom_to_top: vec![second.handle, first.handle],
        });
    assert!(
        fixture
            .server
            .state
            .applied_override_redirect_stack
            .is_none()
    );
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(second.clone()));
    fixture
        .server
        .commit_xwayland_scene_batch(token)
        .expect("commit snapshot admission batch");
    let second_id = fixture
        .server
        .state
        .window_id_for_x11_handle(second.handle)
        .expect("second override-redirect id");

    assert_eq!(
        fixture.server.state.window_stacking,
        vec![second_id, first_id]
    );
    assert_eq!(
        fixture.server.state.applied_override_redirect_stack,
        Some((generation, 1))
    );
}

#[test]
fn newer_root_snapshot_replaces_older_snapshot_before_commit() {
    let socket = super::super::unique_socket_name();
    let mut server = super::super::OwnCompositorServer::bind_cpu_composition(&socket)
        .expect("bind fake compositor server");
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let _xwayland_peer = install_test_xwayland_identity(&mut server, generation);
    let mut first = fake_snapshot();
    first.kind = DesktopWindowKind::OverrideRedirect;
    first.override_redirect = true;
    let mut second = first.clone();
    second.handle = X11WindowHandle::new(generation, 112);
    second.surface_id = 16;
    let first_id = server.state.allocate_window_id().expect("first id");
    let second_id = server.state.allocate_window_id().expect("second id");
    for (id, snapshot) in [(first_id, first.clone()), (second_id, second.clone())] {
        server
            .state
            .insert_desktop_window(crate::compositor::DesktopWindow::new_x11(id, snapshot))
            .expect("insert override-redirect window");
    }

    let token = server
        .begin_xwayland_scene_batch()
        .expect("begin snapshot coalescing batch");
    server.apply_xwayland_window_event(XwmEvent::OverrideRedirectStackSnapshot {
        generation,
        epoch: 1,
        bottom_to_top: vec![second.handle, first.handle],
    });
    server.apply_xwayland_window_event(XwmEvent::OverrideRedirectStackSnapshot {
        generation,
        epoch: 2,
        bottom_to_top: vec![first.handle, second.handle],
    });
    assert!(server.state.applied_override_redirect_stack.is_none());
    server
        .commit_xwayland_scene_batch(token)
        .expect("commit snapshot coalescing batch");

    assert_eq!(server.state.window_stacking, vec![first_id, second_id]);
    assert_eq!(
        server
            .xwayland_scene_metrics()
            .override_redirect_stack_snapshots_applied,
        1
    );
}

#[test]
fn snapshot_before_override_redirect_teardown_is_settled_against_final_windows() {
    let socket = super::super::unique_socket_name();
    let mut server = super::super::OwnCompositorServer::bind_cpu_composition(&socket)
        .expect("bind fake compositor server");
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let _xwayland_peer = install_test_xwayland_identity(&mut server, generation);
    let mut first = fake_snapshot();
    first.kind = DesktopWindowKind::OverrideRedirect;
    first.override_redirect = true;
    let mut second = first.clone();
    second.handle = X11WindowHandle::new(generation, 113);
    second.surface_id = 17;
    let first_id = server.state.allocate_window_id().expect("first id");
    let second_id = server.state.allocate_window_id().expect("second id");
    for (id, snapshot) in [(first_id, first.clone()), (second_id, second.clone())] {
        server
            .state
            .insert_desktop_window(crate::compositor::DesktopWindow::new_x11(id, snapshot))
            .expect("insert override-redirect window");
    }

    let token = server
        .begin_xwayland_scene_batch()
        .expect("begin snapshot teardown batch");
    server.apply_xwayland_window_event(XwmEvent::OverrideRedirectStackSnapshot {
        generation,
        epoch: 3,
        bottom_to_top: vec![second.handle, first.handle],
    });
    server.apply_xwayland_window_event(XwmEvent::WindowWithdrawn(second.handle));
    server
        .commit_xwayland_scene_batch(token)
        .expect("commit snapshot teardown batch");

    assert_eq!(server.state.window_stacking, vec![first_id]);
    assert_eq!(
        server.state.applied_override_redirect_stack,
        Some((generation, 3))
    );
    assert!(server.state.window(second_id).is_none());
}

#[test]
fn snapshot_before_override_redirect_kind_change_is_settled_after_reclassification() {
    let socket = super::super::unique_socket_name();
    let mut server = super::super::OwnCompositorServer::bind_cpu_composition(&socket)
        .expect("bind fake compositor server");
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let _xwayland_peer = install_test_xwayland_identity(&mut server, generation);
    let mut first = fake_snapshot();
    first.kind = DesktopWindowKind::OverrideRedirect;
    first.override_redirect = true;
    let mut second = first.clone();
    second.handle = X11WindowHandle::new(generation, 119);
    second.surface_id = 18;
    let first_id = server.state.allocate_window_id().expect("first id");
    let second_id = server.state.allocate_window_id().expect("second id");
    for (id, snapshot) in [(first_id, first.clone()), (second_id, second.clone())] {
        server
            .state
            .insert_desktop_window(crate::compositor::DesktopWindow::new_x11(id, snapshot))
            .expect("insert override-redirect window");
    }

    let token = server
        .begin_xwayland_scene_batch()
        .expect("begin snapshot kind-change batch");
    server.apply_xwayland_window_event(XwmEvent::OverrideRedirectStackSnapshot {
        generation,
        epoch: 4,
        bottom_to_top: vec![second.handle, first.handle],
    });
    server.apply_xwayland_window_event(XwmEvent::MetadataChanged {
        window: second.handle,
        delta: crate::xwayland::xwm::X11MetadataDelta::Kind(DesktopWindowKind::Managed),
    });
    server
        .commit_xwayland_scene_batch(token)
        .expect("commit snapshot kind-change batch");

    assert_eq!(server.state.window_stacking, vec![second_id, first_id]);
    assert_ne!(
        server
            .state
            .window(second_id)
            .expect("reclassified window")
            .x11_role,
        Some(crate::compositor::X11DesktopRole::OverrideRedirect)
    );
}

#[test]
fn override_redirect_snapshot_reports_accepted_without_scene_change() {
    let socket = super::super::unique_socket_name();
    let mut server = super::super::OwnCompositorServer::bind_cpu_composition(&socket)
        .expect("bind fake compositor server");
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let _xwayland_peer = install_test_xwayland_identity(&mut server, generation);

    let outcome = server
        .state
        .apply_override_redirect_stack_snapshot(generation, 1, &[]);

    assert_eq!(
        outcome,
        crate::compositor::OverrideRedirectStackSnapshotResult::Applied {
            logical_stack_changed: false
        }
    );
}

#[test]
fn override_redirect_transient_metadata_does_not_join_managed_family_or_writeback() {
    let socket = super::super::unique_socket_name();
    let mut server = super::super::OwnCompositorServer::bind_cpu_composition(&socket)
        .expect("bind fake compositor server");
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let _xwayland_peer = install_test_xwayland_identity(&mut server, generation);
    let parent = fake_snapshot();
    let mut popup = parent.clone();
    popup.handle = X11WindowHandle::new(generation, 109);
    popup.surface_id = 15;
    popup.kind = DesktopWindowKind::OverrideRedirect;
    popup.override_redirect = true;
    popup.transient_for = Some(parent.handle);
    let parent_id = server.state.allocate_window_id().expect("parent id");
    let popup_id = server.state.allocate_window_id().expect("popup id");
    server
        .state
        .insert_desktop_window(crate::compositor::DesktopWindow::new_x11(
            parent_id,
            parent.clone(),
        ))
        .expect("insert parent");
    server
        .state
        .insert_desktop_window(crate::compositor::DesktopWindow::new_x11(
            popup_id,
            popup.clone(),
        ))
        .expect("insert override-redirect popup");

    assert_eq!(
        server
            .state
            .window(popup_id)
            .and_then(|window| window.relationships.transient_for),
        None
    );
    assert_eq!(
        server
            .state
            .window(popup_id)
            .and_then(|window| window.x11_transient_for),
        Some(parent.handle)
    );
    assert_eq!(server.state.x11_stack_handles(), vec![parent.handle]);
}

#[test]
fn override_redirect_snapshot_without_active_identity_is_rejected() {
    let socket = super::super::unique_socket_name();
    let mut server = super::super::OwnCompositorServer::bind_cpu_composition(&socket)
        .expect("bind fake compositor server");
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());

    server.apply_xwayland_window_event(XwmEvent::OverrideRedirectStackSnapshot {
        generation,
        epoch: 1,
        bottom_to_top: Vec::new(),
    });

    assert!(server.state.applied_override_redirect_stack.is_none());
    assert_eq!(
        server
            .xwayland_scene_metrics()
            .override_redirect_stack_snapshots_rejected_generation,
        1
    );
}

#[test]
fn override_redirect_snapshot_with_mismatched_active_generation_is_rejected() {
    let socket = super::super::unique_socket_name();
    let mut server = super::super::OwnCompositorServer::bind_cpu_composition(&socket)
        .expect("bind fake compositor server");
    let active_generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let snapshot_generation = XwaylandGeneration::new(NonZeroU64::new(2).unwrap());
    let _peer = install_test_xwayland_identity(&mut server, active_generation);

    server.apply_xwayland_window_event(XwmEvent::OverrideRedirectStackSnapshot {
        generation: snapshot_generation,
        epoch: 1,
        bottom_to_top: Vec::new(),
    });

    assert!(server.state.applied_override_redirect_stack.is_none());
    assert_eq!(
        server
            .xwayland_scene_metrics()
            .override_redirect_stack_snapshots_rejected_generation,
        1
    );
}

#[test]
fn override_redirect_snapshot_with_mixed_generation_handles_is_rejected() {
    let socket = super::super::unique_socket_name();
    let mut server = super::super::OwnCompositorServer::bind_cpu_composition(&socket)
        .expect("bind fake compositor server");
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let other_generation = XwaylandGeneration::new(NonZeroU64::new(2).unwrap());
    let _peer = install_test_xwayland_identity(&mut server, generation);

    server.apply_xwayland_window_event(XwmEvent::OverrideRedirectStackSnapshot {
        generation,
        epoch: 1,
        bottom_to_top: vec![
            X11WindowHandle::new(generation, 201),
            X11WindowHandle::new(other_generation, 202),
        ],
    });

    assert!(server.state.applied_override_redirect_stack.is_none());
    assert_eq!(
        server
            .xwayland_scene_metrics()
            .override_redirect_stack_snapshots_rejected_generation,
        1
    );
}

#[test]
fn late_snapshot_after_generation_revoke_is_rejected_and_new_generation_can_apply() {
    let socket = super::super::unique_socket_name();
    let mut server = super::super::OwnCompositorServer::bind_cpu_composition(&socket)
        .expect("bind fake compositor server");
    let old_generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let new_generation = XwaylandGeneration::new(NonZeroU64::new(2).unwrap());
    let _old_peer = install_test_xwayland_identity(&mut server, old_generation);

    server.apply_xwayland_window_event(XwmEvent::OverrideRedirectStackSnapshot {
        generation: old_generation,
        epoch: 4,
        bottom_to_top: Vec::new(),
    });
    assert_eq!(
        server.state.applied_override_redirect_stack,
        Some((old_generation, 4))
    );

    server.revoke_xwayland_generation(old_generation);
    assert!(server.state.applied_override_redirect_stack.is_none());
    server.apply_xwayland_window_event(XwmEvent::OverrideRedirectStackSnapshot {
        generation: old_generation,
        epoch: 5,
        bottom_to_top: Vec::new(),
    });
    let _new_peer = install_test_xwayland_identity(&mut server, new_generation);
    server.apply_xwayland_window_event(XwmEvent::OverrideRedirectStackSnapshot {
        generation: new_generation,
        epoch: 1,
        bottom_to_top: Vec::new(),
    });

    assert_eq!(
        server.state.applied_override_redirect_stack,
        Some((new_generation, 1))
    );
    let metrics = server.xwayland_scene_metrics();
    assert_eq!(
        metrics.override_redirect_stack_snapshots_rejected_generation,
        1
    );
    assert_eq!(metrics.override_redirect_stack_snapshots_applied, 2);
}

#[test]
fn stale_override_redirect_snapshot_does_not_mutate_scene_order() {
    let socket = super::super::unique_socket_name();
    let mut server = super::super::OwnCompositorServer::bind_cpu_composition(&socket)
        .expect("bind fake compositor server");
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let _xwayland_peer = install_test_xwayland_identity(&mut server, generation);
    let mut first = fake_snapshot();
    first.kind = DesktopWindowKind::OverrideRedirect;
    first.override_redirect = true;
    let mut second = first.clone();
    second.handle = X11WindowHandle::new(generation, 108);
    second.surface_id = 14;
    let first_id = server.state.allocate_window_id().expect("first id");
    let second_id = server.state.allocate_window_id().expect("second id");
    server
        .state
        .insert_desktop_window(crate::compositor::DesktopWindow::new_x11(
            first_id,
            first.clone(),
        ))
        .expect("insert first OR window");
    server
        .state
        .insert_desktop_window(crate::compositor::DesktopWindow::new_x11(
            second_id,
            second.clone(),
        ))
        .expect("insert second OR window");

    server.apply_xwayland_window_event(XwmEvent::OverrideRedirectStackSnapshot {
        generation,
        epoch: 4,
        bottom_to_top: vec![first.handle, second.handle],
    });
    let current = server.state.window_stacking.clone();
    server.apply_xwayland_window_event(XwmEvent::OverrideRedirectStackSnapshot {
        generation,
        epoch: 3,
        bottom_to_top: vec![second.handle, first.handle],
    });
    assert_eq!(server.state.window_stacking, current);
    assert_eq!(
        server
            .xwayland_scene_metrics()
            .override_redirect_stack_snapshots_rejected_stale,
        1
    );
}
