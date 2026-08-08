use super::*;
use wayland_server::Resource;

fn drain_pointer_events(
    fixture: &mut super::StationaryPointerXwaylandFixture,
    state: &mut super::super::RegistryTestState,
) {
    fixture.server.tick().expect("flush Wayland pointer events");
    let guard = fixture
        .connection
        .prepare_read()
        .expect("prepare Wayland pointer event read");
    match guard.read() {
        Ok(_) => {}
        Err(wayland_client::backend::WaylandError::Io(error))
            if error.kind() == std::io::ErrorKind::WouldBlock =>
        {
            return;
        }
        Err(error) => panic!("read Wayland pointer events: {error}"),
    }
    fixture
        .queue
        .dispatch_pending(state)
        .expect("dispatch Wayland pointer events");
}

fn client_owned_x11_interaction_fixture() -> (
    super::StationaryPointerXwaylandFixture,
    super::super::RegistryTestState,
    X11WindowHandle,
    SurfacePlacement,
) {
    let mut fixture = stationary_pointer_xwayland_fixture();
    let mut client_state = super::super::RegistryTestState::default();
    let parent = fake_snapshot_for_surface(fixture.parent_surface_id, 200);
    let parent_handle = parent.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(parent));
    let placement = fixture.server.renderable_surfaces()[0].placement;
    fixture.server.send_pointer_motion(
        f64::from(placement.local_x + 1),
        f64::from(placement.local_y + 1),
    );
    drain_pointer_events(&mut fixture, &mut client_state);
    client_state.pointer_event_log.clear();
    fixture.server.send_pointer_button(0x110, true);
    drain_pointer_events(&mut fixture, &mut client_state);
    client_state.pointer_event_log.clear();
    assert_eq!(
        fixture.server.state.begin_x11_client_window_interaction(
            parent_handle,
            f64::from(placement.local_x + 1),
            f64::from(placement.local_y + 1),
            WindowInteractionKind::Move,
            1,
        ),
        X11MoveResizeBeginResult::Began
    );
    (fixture, client_state, parent_handle, placement)
}

#[test]
fn xwayland_scene_commit_preserves_bookkeeping_for_unchanged_pointer_target() {
    let mut fixture = stationary_pointer_xwayland_fixture();
    let mut client_state = super::super::RegistryTestState::default();
    let mut parent = fake_snapshot();
    parent.surface_id = fixture.parent_surface_id;
    parent.geometry = X11Geometry {
        x: 40,
        y: 40,
        width: 2,
        height: 2,
    };
    let parent_handle = parent.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(parent));
    let placement = fixture.server.renderable_surfaces()[0].placement;
    fixture.server.send_pointer_motion(
        f64::from(placement.local_x + 1),
        f64::from(placement.local_y + 1),
    );
    drain_pointer_events(&mut fixture, &mut client_state);
    let enter_serial = client_state.pointer_enter_serial;
    client_state.pointer_event_log.clear();
    assert_eq!(fixture.server.state.pointer_entered_surfaces.len(), 1);
    assert_eq!(
        fixture
            .server
            .state
            .pointer_surface
            .as_ref()
            .map(crate::compositor::compositor_surface_id),
        Some(fixture.parent_surface_id)
    );
    let token = fixture
        .server
        .begin_xwayland_scene_batch()
        .expect("begin unchanged-target batch");
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::MetadataChanged {
            window: parent_handle,
            delta: crate::xwayland::xwm::X11MetadataDelta::Title(None),
        });
    fixture
        .server
        .commit_xwayland_scene_batch(token)
        .expect("commit unchanged-target batch");
    drain_pointer_events(&mut fixture, &mut client_state);

    assert_eq!(fixture.server.state.pointer_entered_surfaces.len(), 1);
    assert!(client_state.pointer_event_log.is_empty());
    assert_eq!(client_state.pointer_enter_serial, enter_serial);
}

#[test]
fn xwayland_scene_commit_keeps_implicit_grab_target() {
    let mut fixture = stationary_pointer_xwayland_fixture();
    let mut parent = fake_snapshot();
    parent.surface_id = fixture.parent_surface_id;
    parent.geometry = X11Geometry {
        x: 40,
        y: 40,
        width: 2,
        height: 2,
    };
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(parent.clone()));
    let placement = fixture.server.renderable_surfaces()[0].placement;
    fixture.server.send_pointer_motion(
        f64::from(placement.local_x + 1),
        f64::from(placement.local_y + 1),
    );
    fixture.server.send_pointer_button(0x110, true);
    assert!(fixture.server.state.implicit_pointer_grab.is_some());

    let token = fixture
        .server
        .begin_xwayland_scene_batch()
        .expect("begin grabbed batch");
    let mut popup = parent;
    popup.handle = X11WindowHandle::new(
        XwaylandGeneration::new(NonZeroU64::new(1).expect("generation")),
        111,
    );
    popup.surface_id = fixture.popup_surface_id;
    popup.kind = DesktopWindowKind::OverrideRedirect;
    popup.override_redirect = true;
    popup.geometry = X11Geometry {
        x: placement.local_x,
        y: placement.local_y,
        width: 2,
        height: 2,
    };
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(popup));
    fixture
        .server
        .commit_xwayland_scene_batch(token)
        .expect("commit grabbed batch");

    assert_eq!(
        fixture
            .server
            .state
            .pointer_surface
            .as_ref()
            .map(crate::compositor::compositor_surface_id),
        Some(fixture.parent_surface_id)
    );
    assert!(fixture.server.state.implicit_pointer_grab.is_some());
}

#[test]
fn xwayland_scene_commit_emits_one_repaint_request() {
    let socket = super::super::unique_socket_name();
    let mut server = super::super::OwnCompositorServer::bind_cpu_composition(&socket)
        .expect("bind compositor server");
    let token = server
        .begin_xwayland_scene_batch()
        .expect("begin XWayland scene batch");
    server.apply_xwayland_window_event(XwmEvent::WindowMapRequested(fake_snapshot().handle));
    server
        .commit_xwayland_scene_batch(token)
        .expect("commit XWayland scene batch");

    assert!(!server.take_xwayland_scene_repaint_request());
    assert!(!server.take_xwayland_scene_repaint_request());
}

#[test]
fn visible_admission_requests_one_repaint_and_metadata_only_does_not() {
    let mut fixture = stationary_pointer_xwayland_fixture();
    let token = fixture
        .server
        .begin_xwayland_scene_batch()
        .expect("begin visible admission batch");
    let parent = fake_snapshot_for_surface(fixture.parent_surface_id, 116);
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(parent.clone()));
    fixture
        .server
        .commit_xwayland_scene_batch(token)
        .expect("commit visible admission batch");
    assert!(fixture.server.take_xwayland_scene_repaint_request());

    let token = fixture
        .server
        .begin_xwayland_scene_batch()
        .expect("begin metadata-only batch");
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::MetadataChanged {
            window: parent.handle,
            delta: crate::xwayland::xwm::X11MetadataDelta::Title(parent.metadata.title.clone()),
        });
    fixture
        .server
        .commit_xwayland_scene_batch(token)
        .expect("commit metadata-only batch");
    assert!(!fixture.server.take_xwayland_scene_repaint_request());

    let token = fixture
        .server
        .begin_xwayland_scene_batch()
        .expect("begin visible teardown batch");
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowWithdrawn(parent.handle));
    fixture
        .server
        .commit_xwayland_scene_batch(token)
        .expect("commit visible teardown batch");
    assert!(fixture.server.take_xwayland_scene_repaint_request());
}

#[test]
fn stale_and_identical_snapshots_do_not_request_repaint() {
    let socket = super::super::unique_socket_name();
    let mut server = super::super::OwnCompositorServer::bind_cpu_composition(&socket)
        .expect("bind compositor server");
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let _peer = super::install_test_xwayland_identity(&mut server, generation);
    server.apply_xwayland_window_event(XwmEvent::OverrideRedirectStackSnapshot {
        generation,
        epoch: 1,
        bottom_to_top: Vec::new(),
    });

    let token = server
        .begin_xwayland_scene_batch()
        .expect("begin stale snapshot batch");
    server.apply_xwayland_window_event(XwmEvent::OverrideRedirectStackSnapshot {
        generation,
        epoch: 1,
        bottom_to_top: Vec::new(),
    });
    server
        .commit_xwayland_scene_batch(token)
        .expect("commit stale snapshot batch");
    assert!(!server.take_xwayland_scene_repaint_request());

    let token = server
        .begin_xwayland_scene_batch()
        .expect("begin identical snapshot batch");
    server.apply_xwayland_window_event(XwmEvent::OverrideRedirectStackSnapshot {
        generation,
        epoch: 2,
        bottom_to_top: Vec::new(),
    });
    server
        .commit_xwayland_scene_batch(token)
        .expect("commit identical snapshot batch");
    assert!(!server.take_xwayland_scene_repaint_request());
}

fn fake_snapshot_for_surface(surface_id: u32, xid: u32) -> X11WindowSnapshot {
    let mut snapshot = fake_snapshot();
    snapshot.handle = X11WindowHandle::new(
        XwaylandGeneration::new(NonZeroU64::new(1).expect("generation")),
        xid,
    );
    snapshot.surface_id = surface_id;
    snapshot.geometry = X11Geometry {
        x: 40,
        y: 40,
        width: 2,
        height: 2,
    };
    snapshot
}

#[test]
fn root_stack_change_commits_render_order_and_final_pointer_target() {
    let mut fixture = stationary_pointer_xwayland_fixture();
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let mut popup = fake_snapshot();
    popup.handle = X11WindowHandle::new(generation, 114);
    popup.surface_id = fixture.popup_surface_id;
    popup.kind = DesktopWindowKind::OverrideRedirect;
    popup.override_redirect = true;
    popup.geometry = X11Geometry {
        x: 40,
        y: 40,
        width: 2,
        height: 2,
    };
    let mut toplevel = popup.clone();
    toplevel.handle = X11WindowHandle::new(generation, 115);
    toplevel.surface_id = fixture.parent_surface_id;

    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(popup.clone()));
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(toplevel.clone()));
    let placement = fixture.server.renderable_surfaces()[0].placement;
    fixture.server.send_pointer_motion(
        f64::from(placement.local_x + 1),
        f64::from(placement.local_y + 1),
    );
    assert_eq!(
        fixture
            .server
            .state
            .pointer_surface
            .as_ref()
            .map(crate::compositor::compositor_surface_id),
        Some(fixture.parent_surface_id)
    );

    let token = fixture
        .server
        .begin_xwayland_scene_batch()
        .expect("begin root-stack pointer batch");
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::OverrideRedirectStackSnapshot {
            generation,
            epoch: 1,
            bottom_to_top: vec![toplevel.handle, popup.handle],
        });
    fixture
        .server
        .commit_xwayland_scene_batch(token)
        .expect("commit root-stack pointer batch");

    assert_eq!(
        fixture
            .server
            .state
            .pointer_surface
            .as_ref()
            .map(crate::compositor::compositor_surface_id),
        Some(fixture.popup_surface_id)
    );
}

#[test]
fn changed_pointer_target_queues_one_leave_enter_and_final_frame() {
    let mut fixture = stationary_pointer_xwayland_fixture();
    let mut client_state = super::super::RegistryTestState::default();
    let mut parent = fake_snapshot_for_surface(fixture.parent_surface_id, 117);
    parent.geometry = X11Geometry {
        x: 40,
        y: 40,
        width: 2,
        height: 2,
    };
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(parent));
    let placement = fixture.server.renderable_surfaces()[0].placement;
    fixture.server.send_pointer_motion(
        f64::from(placement.local_x + 1),
        f64::from(placement.local_y + 1),
    );
    drain_pointer_events(&mut fixture, &mut client_state);
    client_state.pointer_event_log.clear();
    assert_eq!(fixture.server.state.pointer_resources.len(), 1);

    let token = fixture
        .server
        .begin_xwayland_scene_batch()
        .expect("begin pointer crossing batch");
    let mut popup = fake_snapshot_for_surface(fixture.popup_surface_id, 118);
    popup.kind = DesktopWindowKind::OverrideRedirect;
    popup.override_redirect = true;
    popup.geometry = X11Geometry {
        x: placement.local_x,
        y: placement.local_y,
        width: 2,
        height: 2,
    };
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(popup));
    fixture
        .server
        .commit_xwayland_scene_batch(token)
        .expect("commit pointer crossing batch");
    drain_pointer_events(&mut fixture, &mut client_state);

    assert_eq!(
        client_state.pointer_event_log,
        vec!["leave", "enter", "frame"]
    );
}

#[test]
fn destroyed_implicit_grab_surface_is_cleared_during_batch() {
    let mut fixture = stationary_pointer_xwayland_fixture();
    let parent = fake_snapshot_for_surface(fixture.parent_surface_id, 120);
    let parent_handle = parent.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(parent));
    let placement = fixture.server.renderable_surfaces()[0].placement;
    fixture.server.send_pointer_motion(
        f64::from(placement.local_x + 1),
        f64::from(placement.local_y + 1),
    );
    fixture.server.send_pointer_button(0x110, true);
    assert!(fixture.server.state.implicit_pointer_grab.is_some());

    let token = fixture
        .server
        .begin_xwayland_scene_batch()
        .expect("begin destroyed-grab batch");
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowWithdrawn(parent_handle));
    fixture
        .server
        .commit_xwayland_scene_batch(token)
        .expect("commit destroyed-grab batch");

    assert!(fixture.server.state.implicit_pointer_grab.is_none());
    assert!(fixture.server.state.held_pointer_buttons.is_empty());
    fixture.server.send_pointer_button(0x110, false);
    assert!(fixture.server.state.implicit_pointer_grab.is_none());
}

#[test]
fn release_after_preserved_implicit_grab_crosses_to_final_target() {
    let mut fixture = stationary_pointer_xwayland_fixture();
    let parent = fake_snapshot_for_surface(fixture.parent_surface_id, 121);
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(parent));
    let placement = fixture.server.renderable_surfaces()[0].placement;
    fixture.server.send_pointer_motion(
        f64::from(placement.local_x + 1),
        f64::from(placement.local_y + 1),
    );
    fixture.server.send_pointer_button(0x110, true);

    let token = fixture
        .server
        .begin_xwayland_scene_batch()
        .expect("begin preserved-grab batch");
    let mut popup = fake_snapshot_for_surface(fixture.popup_surface_id, 122);
    popup.kind = DesktopWindowKind::OverrideRedirect;
    popup.override_redirect = true;
    popup.geometry = X11Geometry {
        x: placement.local_x,
        y: placement.local_y,
        width: 2,
        height: 2,
    };
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(popup));
    fixture
        .server
        .commit_xwayland_scene_batch(token)
        .expect("commit preserved-grab batch");
    assert!(fixture.server.state.implicit_pointer_grab.is_some());

    fixture.server.send_pointer_button(0x110, false);
    assert!(fixture.server.state.implicit_pointer_grab.is_none());
    assert_eq!(
        fixture
            .server
            .state
            .pointer_surface
            .as_ref()
            .map(crate::compositor::compositor_surface_id),
        Some(fixture.popup_surface_id)
    );
}

#[test]
fn client_owned_trigger_release_settles_exact_original_pointer_ownership() {
    let mut fixture = stationary_pointer_xwayland_fixture();
    let mut client_state = super::super::RegistryTestState::default();
    let parent = fake_snapshot_for_surface(fixture.parent_surface_id, 123);
    let parent_handle = parent.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(parent));
    let placement = fixture.server.renderable_surfaces()[0].placement;
    fixture.server.send_pointer_motion(
        f64::from(placement.local_x + 1),
        f64::from(placement.local_y + 1),
    );
    drain_pointer_events(&mut fixture, &mut client_state);
    let parent_surface = fixture
        .server
        .state
        .surface_resource_by_id(fixture.parent_surface_id)
        .expect("parent surface resource");
    let popup_surface = fixture
        .server
        .state
        .surface_resource_by_id(fixture.popup_surface_id)
        .expect("popup surface resource");
    for iteration in 0..100 {
        client_state.pointer_event_log.clear();
        fixture.server.state.pointer_surface = Some(parent_surface.clone());
        fixture.server.send_pointer_button(0x110, true);
        drain_pointer_events(&mut fixture, &mut client_state);
        client_state.pointer_event_log.clear();

        assert_eq!(
            fixture.server.state.begin_x11_client_window_interaction(
                parent_handle,
                f64::from(placement.local_x + 1),
                f64::from(placement.local_y + 1),
                WindowInteractionKind::Move,
                1,
            ),
            X11MoveResizeBeginResult::Began
        );
        fixture.server.update_window_interaction(
            placement.local_x as f64 + 80.0 + f64::from(iteration),
            placement.local_y as f64 + 60.0,
        );
        fixture.server.state.pointer_surface = Some(popup_surface.clone());

        let release = fixture.server.end_window_interaction_for_button(0x110);
        let context = match release {
            WindowInteractionButtonRelease::Ended {
                delivery: TriggerReleaseDelivery::ClientOwned,
                context,
            } => context,
            other => panic!("expected client-owned release, got {other:?}"),
        };
        assert!(fixture.server.send_client_owned_trigger_release(context));
        assert!(!fixture.server.send_client_owned_trigger_release(context));
        drain_pointer_events(&mut fixture, &mut client_state);

        assert_eq!(
            client_state
                .pointer_event_log
                .iter()
                .filter(|event| **event == "button_released")
                .count(),
            1,
            "iteration {iteration}"
        );
        assert_eq!(
            &client_state.pointer_event_log[..2],
            &["button_released", "frame"],
            "iteration {iteration}"
        );
        let record = fixture
            .server
            .window_interaction_release_debug_records()
            .iter()
            .rev()
            .find(|record| record.release_target_surface_id.is_some())
            .copied()
            .expect("release debug record");
        assert_eq!(record.original_surface_id, Some(fixture.parent_surface_id));
        assert_eq!(
            record.release_target_surface_id,
            Some(fixture.parent_surface_id)
        );
    }
    assert!(!fixture.server.window_interaction_active());
    assert!(fixture.server.state.held_pointer_buttons.is_empty());
    assert!(fixture.server.state.last_pointer_press.is_none());
    assert!(fixture.server.state.implicit_pointer_grab.is_none());
    let metrics = fixture.server.window_interaction_release_metrics();
    assert_eq!(metrics.window_interaction_trigger_releases, 100);
    assert_eq!(metrics.window_interaction_client_releases_forwarded, 100);
    assert_eq!(metrics.window_interaction_release_target_missing, 0);
    assert_eq!(metrics.window_interaction_duplicate_releases_prevented, 100);
    assert_eq!(
        metrics.window_interaction_post_terminal_pointer_refreshes,
        100
    );
}

#[test]
fn client_owned_trigger_release_preserves_other_held_buttons() {
    let mut fixture = stationary_pointer_xwayland_fixture();
    let mut client_state = super::super::RegistryTestState::default();
    let parent = fake_snapshot_for_surface(fixture.parent_surface_id, 124);
    let parent_handle = parent.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(parent));
    let placement = fixture.server.renderable_surfaces()[0].placement;
    fixture.server.send_pointer_motion(
        f64::from(placement.local_x + 1),
        f64::from(placement.local_y + 1),
    );
    drain_pointer_events(&mut fixture, &mut client_state);

    fixture.server.send_pointer_button(0x111, true);
    drain_pointer_events(&mut fixture, &mut client_state);
    client_state.pointer_event_log.clear();
    fixture.server.send_pointer_button(0x110, true);
    drain_pointer_events(&mut fixture, &mut client_state);
    client_state.pointer_event_log.clear();

    assert_eq!(
        fixture.server.state.begin_x11_client_window_interaction(
            parent_handle,
            f64::from(placement.local_x + 1),
            f64::from(placement.local_y + 1),
            WindowInteractionKind::Move,
            1,
        ),
        X11MoveResizeBeginResult::Began
    );
    let release = fixture.server.end_window_interaction_for_button(0x110);
    let context = match release {
        WindowInteractionButtonRelease::Ended {
            delivery: TriggerReleaseDelivery::ClientOwned,
            context,
        } => context,
        other => panic!("expected client-owned release, got {other:?}"),
    };
    assert!(fixture.server.send_client_owned_trigger_release(context));
    drain_pointer_events(&mut fixture, &mut client_state);

    assert_eq!(
        client_state
            .pointer_event_log
            .iter()
            .filter(|event| **event == "button_released")
            .count(),
        1
    );
    assert_eq!(fixture.server.state.held_pointer_buttons.len(), 1);
    assert_eq!(fixture.server.state.held_pointer_buttons[0].button, 0x111);
    assert!(fixture.server.state.implicit_pointer_grab.is_some());

    fixture.server.send_pointer_button(0x111, false);
    drain_pointer_events(&mut fixture, &mut client_state);
    assert!(fixture.server.state.held_pointer_buttons.is_empty());
    assert!(fixture.server.state.implicit_pointer_grab.is_none());
    assert_eq!(
        fixture
            .server
            .window_interaction_release_metrics()
            .window_interaction_post_terminal_pointer_refreshes,
        1
    );
}

#[test]
fn client_owned_release_uses_live_original_surface_without_renderable_entry() {
    let (mut fixture, mut client_state, _parent_handle, _placement) =
        client_owned_x11_interaction_fixture();
    fixture.server.state.renderable_surfaces.clear();

    let context = match fixture.server.end_window_interaction_for_button(0x110) {
        WindowInteractionButtonRelease::Ended {
            delivery: TriggerReleaseDelivery::ClientOwned,
            context,
        } => context,
        other => panic!("expected client-owned release, got {other:?}"),
    };
    assert!(fixture.server.send_client_owned_trigger_release(context));
    drain_pointer_events(&mut fixture, &mut client_state);

    assert_eq!(
        client_state
            .pointer_event_log
            .iter()
            .filter(|event| **event == "button_released")
            .count(),
        1
    );
    assert_eq!(
        fixture
            .server
            .window_interaction_release_metrics()
            .window_interaction_release_target_missing,
        0
    );
}

#[test]
fn client_owned_release_survives_xwayland_attachment_replacement() {
    let (mut fixture, mut client_state, _parent_handle, _placement) =
        client_owned_x11_interaction_fixture();
    let mut replacement = fixture.server.renderable_surfaces()[0].clone();
    replacement.surface_id = fixture.popup_surface_id;
    fixture.server.state.renderable_surfaces.push(replacement);
    fixture.server.state.invalidate_surface_origin_cache();
    fixture
        .server
        .state
        .transfer_xwayland_visual_state_for_attachment_replacement(
            fixture.parent_surface_id,
            fixture.popup_surface_id,
        );
    fixture
        .server
        .state
        .retire_xwayland_attachment(fixture.parent_surface_id);
    assert!(
        fixture
            .server
            .state
            .surface_resource_by_id(fixture.parent_surface_id)
            .is_some_and(|surface| surface.is_alive())
    );

    let context = match fixture.server.end_window_interaction_for_button(0x110) {
        WindowInteractionButtonRelease::Ended {
            delivery: TriggerReleaseDelivery::ClientOwned,
            context,
        } => context,
        other => panic!("expected client-owned release, got {other:?}"),
    };
    assert!(fixture.server.send_client_owned_trigger_release(context));
    drain_pointer_events(&mut fixture, &mut client_state);

    assert_eq!(
        fixture
            .server
            .window_interaction_release_debug_records()
            .iter()
            .rev()
            .find(|record| record.release_target_surface_id.is_some())
            .map(|record| record.release_target_surface_id),
        Some(Some(fixture.parent_surface_id))
    );
}

#[test]
fn unmap_cancellation_does_not_release_to_a_new_pointer_target() {
    let (mut fixture, mut client_state, _parent_handle, placement) =
        client_owned_x11_interaction_fixture();
    fixture
        .server
        .state
        .test_unmap_surface(fixture.parent_surface_id);
    let popup = fake_snapshot_for_surface(fixture.popup_surface_id, 201);
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(popup));
    fixture.server.send_pointer_motion(
        f64::from(placement.local_x + 1),
        f64::from(placement.local_y + 1),
    );
    drain_pointer_events(&mut fixture, &mut client_state);
    client_state.pointer_event_log.clear();
    fixture.server.send_pointer_button(0x110, false);
    drain_pointer_events(&mut fixture, &mut client_state);

    assert!(!client_state.pointer_event_log.contains(&"button_released"));
    assert!(!fixture.server.window_interaction_active());
    assert!(fixture.server.state.held_pointer_buttons.is_empty());
    assert!(fixture.server.state.last_pointer_press.is_none());
    assert!(fixture.server.state.implicit_pointer_grab.is_none());
    assert_eq!(
        fixture
            .server
            .window_interaction_release_metrics()
            .window_interaction_post_terminal_pointer_refreshes,
        1
    );
}

#[test]
fn destroyed_original_surface_does_not_release_to_an_unrelated_surface() {
    let (mut fixture, mut client_state, _parent_handle, placement) =
        client_owned_x11_interaction_fixture();
    fixture
        .server
        .state
        .test_destroy_surface_resource(fixture.parent_surface_id);
    let popup = fake_snapshot_for_surface(fixture.popup_surface_id, 202);
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(popup));
    fixture.server.send_pointer_motion(
        f64::from(placement.local_x + 1),
        f64::from(placement.local_y + 1),
    );
    drain_pointer_events(&mut fixture, &mut client_state);
    client_state.pointer_event_log.clear();
    fixture.server.send_pointer_button(0x110, false);
    drain_pointer_events(&mut fixture, &mut client_state);

    assert!(!client_state.pointer_event_log.contains(&"button_released"));
    assert!(!fixture.server.window_interaction_active());
    assert!(fixture.server.state.held_pointer_buttons.is_empty());
    assert!(fixture.server.state.last_pointer_press.is_none());
    assert!(fixture.server.state.implicit_pointer_grab.is_none());
    assert_eq!(
        fixture
            .server
            .window_interaction_release_metrics()
            .window_interaction_post_terminal_pointer_refreshes,
        1
    );
}
