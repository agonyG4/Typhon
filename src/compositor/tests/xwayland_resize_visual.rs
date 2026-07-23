use super::*;
use crate::compositor::{WindowInteractionKind, WindowInteractionSource};
use crate::render_backend::buffer::BufferSize;

#[test]
fn xwayland_resize_preview_survives_buffer_commit() {
    let mut fixture = first_buffer_fixture();
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    snapshot.geometry = X11Geometry {
        x: 100,
        y: 100,
        width: 640,
        height: 480,
    };
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));

    let interaction_id = ResizeInteractionId::new(1);
    let preview_placement = SurfacePlacement::absolute_root_at(120, 100);
    assert!(fixture.server.state.preview_resize_root_window_to(
        fixture.surface_id,
        620,
        480,
        preview_placement,
        crate::compositor::ResizeEdges::new(false, false, true, false),
        interaction_id,
    ));

    let pending = fixture
        .server
        .state
        .current_surface_buffers
        .get(&fixture.surface_id)
        .cloned()
        .expect("committed XWayland buffer");
    fixture.server.state.commit_xwayland_surface_buffer(
        fixture.surface_id,
        pending,
        Vec::new(),
        SurfacePublicationSource::Immediate,
    );

    let visual = fixture.server.state.toplevel_visual_geometries[&fixture.surface_id];
    assert_eq!(visual.active_resize, Some(interaction_id));
    let root = fixture
        .server
        .renderable_surfaces()
        .iter()
        .find(|surface| surface.surface_id == fixture.surface_id)
        .expect("published XWayland root");
    assert_eq!(
        root.render_placement,
        Some(SurfacePlacement::absolute_root_at(120, 100))
    );
    assert_eq!(
        root.visual_clip,
        Some(SurfaceTargetRect::new(120, 100, 620, 480))
    );
    assert_eq!(
        root.visual_clip
            .map(|clip| clip.x() + i32::try_from(clip.width()).unwrap()),
        Some(740)
    );
}

#[test]
fn xwayland_attachment_replacement_preserves_resize_preview() {
    let mut fixture = stationary_pointer_xwayland_fixture();
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.parent_surface_id;
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
    let interaction_id = ResizeInteractionId::new(2);
    assert!(fixture.server.state.preview_resize_root_window_to(
        fixture.parent_surface_id,
        620,
        480,
        SurfacePlacement::absolute_root_at(120, 100),
        crate::compositor::ResizeEdges::new(false, false, true, false),
        interaction_id,
    ));

    fixture
        .server
        .apply_xwayland_association_event(XwmAssociationEvent::Associated {
            generation: handle.generation(),
            window: handle,
            surface_id: fixture.popup_surface_id,
        });

    assert!(
        fixture
            .server
            .state
            .window_id_for_surface(fixture.parent_surface_id)
            .is_none()
    );
    assert_eq!(
        fixture
            .server
            .state
            .window_id_for_surface(fixture.popup_surface_id),
        Some(window_id)
    );
    assert_eq!(
        fixture.server.state.toplevel_visual_geometries[&fixture.popup_surface_id].active_resize,
        Some(interaction_id)
    );
    let root = fixture
        .server
        .renderable_surfaces()
        .iter()
        .find(|surface| surface.surface_id == fixture.popup_surface_id)
        .expect("replacement XWayland root");
    assert_eq!(
        root.render_placement,
        Some(SurfacePlacement::absolute_root_at(120, 100))
    );
    assert_eq!(
        root.visual_clip,
        Some(SurfaceTargetRect::new(120, 100, 620, 480))
    );
    assert_eq!(fixture.server.state.focused_window_id, Some(window_id));
}

#[test]
fn xwayland_repeated_intermediate_commits_preserve_visual_box() {
    let mut fixture = first_buffer_fixture();
    admit_first_buffer(&mut fixture, 100, 100);
    let interaction_id = ResizeInteractionId::new(3);
    let preview_placement = SurfacePlacement::absolute_root_at(120, 100);
    assert!(fixture.server.state.preview_resize_root_window_to(
        fixture.surface_id,
        620,
        480,
        preview_placement,
        crate::compositor::ResizeEdges::new(false, false, true, false),
        interaction_id,
    ));

    let mut previous_generation = 0;
    for _ in 0..3 {
        let before_assignment = crate::compositor::surface_render_space_assignments(
            fixture.server.renderable_surfaces(),
            1.0,
        )[0]
        .visual_clip;
        let pending = fixture
            .server
            .state
            .current_surface_buffers
            .get(&fixture.surface_id)
            .cloned()
            .expect("committed XWayland buffer");
        fixture.server.state.commit_xwayland_surface_buffer(
            fixture.surface_id,
            pending,
            Vec::new(),
            SurfacePublicationSource::Immediate,
        );

        let root = fixture
            .server
            .renderable_surfaces()
            .iter()
            .find(|surface| surface.surface_id == fixture.surface_id)
            .expect("published XWayland root");
        assert!(root.generation > previous_generation);
        previous_generation = root.generation;
        assert_eq!(
            root.render_placement,
            Some(SurfacePlacement::absolute_root_at(120, 100))
        );
        assert_eq!(
            root.visual_clip,
            Some(SurfaceTargetRect::new(120, 100, 620, 480))
        );
        let after_assignment = crate::compositor::surface_render_space_assignments(
            fixture.server.renderable_surfaces(),
            1.0,
        )[0]
        .visual_clip;
        assert_eq!(
            before_assignment, after_assignment,
            "buffer publication must not change the visual clip assignment"
        );
        assert_eq!(
            fixture.server.state.toplevel_visual_geometries[&fixture.surface_id].active_resize,
            Some(interaction_id)
        );
    }
}

#[test]
fn rapid_resize_visual_box_tracks_latest_pointer_while_content_is_throttled() {
    let mut fixture = first_buffer_fixture();
    admit_first_buffer(&mut fixture, 100, 100);
    let root = fixture
        .server
        .state
        .current_visual_root_window_geometry(fixture.surface_id)
        .expect("initial XWayland visual geometry");
    let edges = crate::compositor::ResizeEdges::new(false, false, true, false);
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
                WindowInteractionKind::Resize(edges),
                WindowInteractionSource::NativeBinding,
                Some(fixture.surface_id),
            )
        )
    );

    for pointer_x in [-40.0, -180.0, -360.0] {
        assert!(
            fixture
                .server
                .state
                .update_window_interaction(pointer_x, 0.0)
        );
        let applied = fixture
            .server
            .state
            .apply_pending_interactive_resize_update();
        assert!(
            applied,
            "pointer sample was not applied: visual={:?} pending={:?} active={:?}",
            fixture
                .server
                .state
                .current_visual_root_window_geometry(fixture.surface_id),
            fixture.server.state.pending_interactive_resize_update,
            fixture
                .server
                .state
                .active_toplevel_resizes
                .get(&fixture.surface_id),
        );

        let expected_width = (i64::from(root.width) - pointer_x as i64)
            .max(160)
            .try_into()
            .expect("preview width fits u32");
        let expected_x = root.placement.local_x.saturating_add(
            i32::try_from(root.width)
                .unwrap()
                .saturating_sub(i32::try_from(expected_width).unwrap()),
        );
        let visual = fixture
            .server
            .state
            .current_visual_root_window_geometry(fixture.surface_id)
            .expect("current preview geometry");
        assert_eq!(visual.width, expected_width);
        assert_eq!(visual.placement.local_x, expected_x);

        let surface = fixture
            .server
            .renderable_surfaces()
            .iter()
            .find(|surface| surface.surface_id == fixture.surface_id)
            .expect("XWayland root remains renderable");
        assert_eq!(surface.render_target_size, None);
        let elements = crate::compositor::render_scene_elements_for_surfaces(
            std::slice::from_ref(surface),
            1.0,
        );
        let element = &elements[0];
        assert_eq!(
            element.backing_target().map(|target| target.width()),
            Some(expected_width)
        );
        assert_eq!(
            element.backing_target().map(|target| target.x()),
            Some(expected_x)
        );
    }
}

#[test]
fn xwayland_final_resize_keeps_backing_until_matching_content_commit() {
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
    let interaction_id = ResizeInteractionId::new(5);
    let final_placement = SurfacePlacement::absolute_root_at(120, 100);
    assert!(fixture.server.state.preview_resize_root_window_to(
        fixture.surface_id,
        620,
        480,
        final_placement,
        crate::compositor::ResizeEdges::new(false, false, true, false),
        interaction_id,
    ));

    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::ResizeSyncPresented {
            window: handle,
            transaction_id: 1,
            geometry: X11Geometry {
                x: 120,
                y: 100,
                width: 620,
                height: 480,
            },
        });
    assert_eq!(
        fixture.server.state.toplevel_visual_geometries[&fixture.surface_id].active_resize,
        None
    );
    assert_eq!(
        fixture.server.state.surface_placement(fixture.surface_id),
        final_placement
    );
    let root = fixture
        .server
        .renderable_surfaces()
        .iter()
        .find(|surface| surface.surface_id == fixture.surface_id)
        .expect("finalized XWayland root");
    assert_eq!(root.render_placement, Some(final_placement));
    assert_eq!(
        root.visual_clip,
        Some(SurfaceTargetRect::new(120, 100, 620, 480)),
        "stale content keeps the final visual box backed after pointer ownership ends"
    );

    let pending = fixture
        .server
        .state
        .current_surface_buffers
        .get(&fixture.surface_id)
        .cloned()
        .expect("committed XWayland buffer");
    fixture.server.state.commit_xwayland_surface_buffer(
        fixture.surface_id,
        pending,
        Vec::new(),
        SurfacePublicationSource::Immediate,
    );
    let root = fixture
        .server
        .renderable_surfaces()
        .iter()
        .find(|surface| surface.surface_id == fixture.surface_id)
        .expect("post-finalization XWayland root");
    assert_eq!(root.render_placement, Some(final_placement));
    assert_eq!(
        root.visual_clip,
        Some(SurfaceTargetRect::new(120, 100, 620, 480)),
        "another stale XWayland publication must not remove pending-content backing"
    );

    let mut matching_pending = fixture
        .server
        .state
        .current_surface_buffers
        .get(&fixture.surface_id)
        .cloned()
        .expect("current XWayland buffer after stale publication");
    matching_pending.surface_size = Some(BufferSize::new(620, 480).expect("matching content"));
    fixture.server.state.commit_xwayland_surface_buffer(
        fixture.surface_id,
        matching_pending,
        Vec::new(),
        SurfacePublicationSource::Immediate,
    );
    let root = fixture
        .server
        .renderable_surfaces()
        .iter()
        .find(|surface| surface.surface_id == fixture.surface_id)
        .expect("matching XWayland root");
    assert_eq!(root.render_placement, Some(final_placement));
    assert_eq!(root.visual_clip, None);
}

#[test]
fn xwayland_fullscreen_request_installs_output_visual_and_configure() {
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
        .expect("managed X11 window");
    let floating = fixture
        .server
        .state
        .current_visual_root_window_geometry(fixture.surface_id)
        .expect("floating visual geometry");

    let commands = fixture
        .server
        .apply_xwayland_window_event(XwmEvent::StateRequested {
            window: handle,
            request: crate::xwayland::xwm::X11StateRequest {
                action: crate::xwayland::xwm::X11StateAction::Add,
                first: Some(crate::xwayland::xwm::X11StateAtom::Fullscreen),
                second: None,
            },
        });
    assert!(
        commands
            .iter()
            .all(|command| !matches!(command, XwmCommand::SetState { .. }))
    );
    let backend_commands = fixture.server.take_xwayland_backend_commands(0);
    let fullscreen = fixture.server.state.fullscreen_window_geometry();
    assert!(backend_commands.iter().any(|command| matches!(
        command,
        XwmCommand::Configure { window, geometry, fields, .. }
            if *window == handle
                && *geometry == crate::xwayland::xwm::X11Geometry {
                    x: fullscreen.placement.local_x,
                    y: fullscreen.placement.local_y,
                    width: fullscreen.width,
                    height: fullscreen.height,
                }
                && *fields == crate::xwayland::xwm::X11ConfigureFlags::all()
    )));
    assert!(backend_commands.iter().any(|command| matches!(
        command,
        XwmCommand::SetState { window, state }
            if *window == handle && state.fullscreen && !state.maximized
    )));
    assert_eq!(
        fixture
            .server
            .state
            .window(window_id)
            .expect("window")
            .state
            .mode(),
        crate::compositor::ToplevelMode::Fullscreen
    );
    assert_eq!(
        fixture
            .server
            .state
            .current_visual_root_window_geometry(fixture.surface_id),
        Some(fullscreen)
    );
    let root = fixture
        .server
        .renderable_surfaces()
        .iter()
        .find(|surface| surface.surface_id == fixture.surface_id)
        .expect("fullscreen root");
    assert_eq!(root.render_target_size, None);
    assert_eq!(
        root.visual_clip,
        Some(SurfaceTargetRect::new(
            fullscreen.placement.local_x,
            fullscreen.placement.local_y,
            fullscreen.width,
            fullscreen.height,
        ))
    );
    assert_eq!(
        crate::compositor::render_scene_elements_for_surfaces(std::slice::from_ref(root), 1.0)[0]
            .backing_target(),
        root.visual_clip
    );

    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::StateRequested {
            window: handle,
            request: crate::xwayland::xwm::X11StateRequest {
                action: crate::xwayland::xwm::X11StateAction::Remove,
                first: Some(crate::xwayland::xwm::X11StateAtom::Fullscreen),
                second: None,
            },
        });
    let restore_commands = fixture.server.take_xwayland_backend_commands(0);
    assert!(restore_commands.iter().any(|command| matches!(
        command,
        XwmCommand::Configure { window, geometry, .. }
            if *window == handle
                && *geometry == crate::xwayland::xwm::X11Geometry {
                    x: floating.placement.local_x,
                    y: floating.placement.local_y,
                    width: floating.width,
                    height: floating.height,
                }
    )));
    assert_eq!(
        fixture
            .server
            .state
            .current_visual_root_window_geometry(fixture.surface_id),
        Some(floating)
    );
}

#[test]
fn xwayland_fullscreen_shortcut_uses_same_geometry_transition() {
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
    let floating = fixture
        .server
        .state
        .current_visual_root_window_geometry(fixture.surface_id)
        .expect("floating visual geometry");

    assert!(fixture.server.state.toggle_fullscreen_focused_window());
    let enter_commands = fixture.server.take_xwayland_backend_commands(0);
    let fullscreen = fixture.server.state.fullscreen_window_geometry();
    assert!(enter_commands.iter().any(|command| matches!(
        command,
        XwmCommand::Configure { window, geometry, fields, .. }
            if *window == handle
                && *geometry == X11Geometry {
                    x: fullscreen.placement.local_x,
                    y: fullscreen.placement.local_y,
                    width: fullscreen.width,
                    height: fullscreen.height,
                }
                && *fields == X11ConfigureFlags::all()
    )));
    assert!(enter_commands.iter().any(|command| matches!(
        command,
        XwmCommand::SetState { window, state }
            if *window == handle && state.fullscreen && !state.maximized
    )));
    assert_eq!(
        fixture
            .server
            .state
            .current_visual_root_window_geometry(fixture.surface_id),
        Some(fullscreen)
    );

    assert!(fixture.server.state.toggle_fullscreen_focused_window());
    let exit_commands = fixture.server.take_xwayland_backend_commands(0);
    assert!(exit_commands.iter().any(|command| matches!(
        command,
        XwmCommand::Configure { window, geometry, fields, .. }
            if *window == handle
                && *geometry == X11Geometry {
                    x: floating.placement.local_x,
                    y: floating.placement.local_y,
                    width: floating.width,
                    height: floating.height,
                }
                && *fields == X11ConfigureFlags::all()
    )));
    assert!(exit_commands.iter().any(|command| matches!(
        command,
        XwmCommand::SetState { window, state }
            if *window == handle && !state.fullscreen && !state.maximized
    )));
    assert_eq!(
        fixture
            .server
            .state
            .current_visual_root_window_geometry(fixture.surface_id),
        Some(floating)
    );
}

#[test]
fn late_resize_presentation_cannot_retire_newer_resize_epoch() {
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

    assert!(fixture.server.state.preview_resize_root_window_to(
        fixture.surface_id,
        620,
        480,
        SurfacePlacement::absolute_root_at(120, 100),
        crate::compositor::ResizeEdges::new(false, false, true, false),
        ResizeInteractionId::new(10),
    ));
    assert!(fixture.server.state.preview_resize_root_window_to(
        fixture.surface_id,
        600,
        480,
        SurfacePlacement::absolute_root_at(140, 100),
        crate::compositor::ResizeEdges::new(false, false, true, false),
        ResizeInteractionId::new(11),
    ));

    let commands = fixture
        .server
        .apply_xwayland_window_event(XwmEvent::ResizeSyncPresented {
            window: handle,
            transaction_id: 1,
            geometry: X11Geometry {
                x: 120,
                y: 100,
                width: 620,
                height: 480,
            },
        });
    assert!(
        commands.is_empty()
            || commands
                .iter()
                .all(|command| matches!(command, XwmCommand::CompleteResizeSync(_)))
    );
    let visual = fixture.server.state.toplevel_visual_geometries[&fixture.surface_id];
    assert_eq!(visual.active_resize, Some(ResizeInteractionId::new(11)));
    assert_eq!(
        visual.placement,
        SurfacePlacement::absolute_root_at(140, 100)
    );
    assert_eq!(visual.width, 600);
}

#[test]
fn xwayland_resize_edges_preserve_opposite_edge_across_commits() {
    let cases = [
        (
            "left",
            120,
            100,
            620,
            480,
            120,
            100,
            740,
            580,
            crate::compositor::ResizeEdges::new(false, false, true, false),
        ),
        (
            "right",
            100,
            100,
            680,
            480,
            100,
            100,
            780,
            580,
            crate::compositor::ResizeEdges::new(false, false, false, true),
        ),
        (
            "top",
            100,
            120,
            640,
            460,
            100,
            120,
            740,
            580,
            crate::compositor::ResizeEdges::new(true, false, false, false),
        ),
        (
            "bottom",
            100,
            100,
            640,
            520,
            100,
            100,
            740,
            620,
            crate::compositor::ResizeEdges::new(false, true, false, false),
        ),
    ];

    for (
        edge,
        preview_x,
        preview_y,
        preview_width,
        preview_height,
        expected_left,
        expected_top,
        expected_right,
        expected_bottom,
        resize_edges,
    ) in cases
    {
        let mut fixture = first_buffer_fixture();
        admit_first_buffer(&mut fixture, 100, 100);
        let interaction_id = ResizeInteractionId::new(4);
        let preview_placement = SurfacePlacement::absolute_root_at(preview_x, preview_y);
        assert!(
            fixture.server.state.preview_resize_root_window_to(
                fixture.surface_id,
                preview_width,
                preview_height,
                preview_placement,
                resize_edges,
                interaction_id,
            ),
            "{edge} preview should apply"
        );
        let pending = fixture
            .server
            .state
            .current_surface_buffers
            .get(&fixture.surface_id)
            .cloned()
            .expect("committed XWayland buffer");
        fixture.server.state.commit_xwayland_surface_buffer(
            fixture.surface_id,
            pending,
            Vec::new(),
            SurfacePublicationSource::Immediate,
        );

        let root = fixture
            .server
            .renderable_surfaces()
            .iter()
            .find(|surface| surface.surface_id == fixture.surface_id)
            .expect("published XWayland root");
        assert_eq!(
            root.render_placement,
            Some(preview_placement),
            "{edge} origin changed after commit"
        );
        assert_eq!(
            root.visual_clip,
            Some(SurfaceTargetRect::new(
                preview_x,
                preview_y,
                preview_width,
                preview_height,
            )),
            "{edge} clip changed after commit"
        );
        let clip = root.visual_clip.expect("active resize clip");
        assert_eq!(clip.x(), expected_left, "{edge} left edge changed");
        assert_eq!(clip.y(), expected_top, "{edge} top edge changed");
        assert_eq!(
            clip.x() + i32::try_from(clip.width()).unwrap(),
            expected_right,
            "{edge} right edge changed"
        );
        assert_eq!(
            clip.y() + i32::try_from(clip.height()).unwrap(),
            expected_bottom,
            "{edge} bottom edge changed"
        );
    }
}

#[test]
fn move_after_resize_release_cannot_be_overwritten_by_late_presentation() {
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
    assert!(
        fixture
            .server
            .state
            .apply_pending_interactive_resize_update()
    );
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
    assert_eq!(
        fixture.server.state.surface_placement(fixture.surface_id),
        sealed.placement,
        "release must seal authoritative placement before client presentation"
    );

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
    assert_eq!(
        fixture.server.state.surface_placement(fixture.surface_id),
        expected_move
    );

    assert!(fixture.server.state.finalize_x11_resize(handle));
    assert_eq!(
        fixture.server.state.surface_placement(fixture.surface_id),
        expected_move,
        "late resize presentation must not restore the sealed or pre-resize origin"
    );
    assert_eq!(
        fixture
            .server
            .state
            .current_visual_root_window_geometry(fixture.surface_id)
            .expect("current visual geometry")
            .placement,
        expected_move
    );
}
