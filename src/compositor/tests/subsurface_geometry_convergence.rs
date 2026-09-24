use super::*;

#[test]
fn gecko_xdg_geometry_origin_converges_after_fullscreen_restore() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let sequence =
        capture_gecko_mode_transition_geometry_evolution(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(
        sequence.fullscreen_committed,
        "the compositor fullscreen transition must commit"
    );
    let fullscreen = sequence
        .fullscreen_authority
        .expect("fullscreen root authority snapshot");
    assert_eq!(
        (
            fullscreen.canonical_surface_placement.local_x,
            fullscreen.canonical_surface_placement.local_y
        ),
        (0, 0)
    );
    assert!(
        sequence.floating_committed,
        "the compositor restore transition must commit"
    );
    let pending_float = sequence
        .floating_transition_authority
        .expect("floating transition authority before client commit");
    let pending_float_visual = pending_float
        .visual_geometry
        .expect("the restore visual remains until its configure response is committed");
    assert!(pending_float_visual.mode_transition);
    assert!(!pending_float_visual.active_resize);
    assert!(pending_float_visual.xdg_configure_serial.is_some());
    assert!(
        pending_float_visual.ack_commit_sequence_floor.is_some(),
        "the automatic configure ACK establishes a boundary, but no client commit has followed it"
    );
    assert_eq!(
        pending_float.logical_frame_origin,
        Some((72, 72)),
        "the compositor's floating frame is already authoritative"
    );
    assert_eq!(
        pending_float.canonical_surface_placement.root_mode,
        RootPlacementMode::CascadedWindow
    );
    assert_eq!(
        (
            pending_float.canonical_surface_placement.local_x,
            pending_float.canonical_surface_placement.local_y
        ),
        (0, 0)
    );

    let floating = sequence
        .floating_authority
        .expect("floating root authority after client commit");
    assert!(
        floating.visual_geometry.is_none(),
        "the matching client commit leaves canonical frame authority in place"
    );
    assert_eq!(
        floating.logical_frame_origin,
        Some((72, 72)),
        "canonical logical frame remains the restored floating location"
    );
    assert_eq!(
        floating.canonical_surface_placement.root_mode,
        RootPlacementMode::CascadedWindow
    );
    assert_eq!(sequence.stages.len(), 6);
    assert_eq!(floating.canonical_surface_placement.local_x, 0);
    assert_eq!(floating.canonical_surface_placement.local_y, 0);

    let mut expected_root_id = None;
    let mut expected_child_id = None;
    let mut expected_relationship = None;
    let mut anchored_content_origin = None;
    let mut previous_parent_commit_sequence = None;
    for (stage_index, stage) in sequence.stages.iter().enumerate() {
        assert_eq!(stage.committed_geometry, Some(stage.expected_geometry));
        assert_eq!(
            stage.requested_position,
            (stage.expected_geometry.x, stage.expected_geometry.y)
        );

        let roots = stage
            .tree
            .iter()
            .filter(|surface| surface.parent_surface_id.is_none())
            .collect::<Vec<_>>();
        assert_eq!(
            roots.len(),
            1,
            "stage {stage_index}: exactly one renderable XDG root"
        );
        let root = roots[0];
        let children = stage
            .tree
            .iter()
            .filter(|surface| surface.parent_surface_id.is_some())
            .collect::<Vec<_>>();
        assert_eq!(
            children.len(),
            1,
            "stage {stage_index}: exactly one content subsurface"
        );
        let child = children[0];
        assert_eq!(child.parent_surface_id, Some(root.surface_id));
        assert_eq!((child.local_x, child.local_y), stage.requested_position);
        assert_eq!((root.content_x, root.content_y), (0, 0));
        assert_eq!((child.content_x, child.content_y), (0, 0));
        assert!(
            child.commit_sequence < root.commit_sequence,
            "stage {stage_index}: child content commits before the parent publishes captured position state"
        );
        if let Some(previous) = previous_parent_commit_sequence {
            assert!(root.commit_sequence > previous);
        }
        previous_parent_commit_sequence = Some(root.commit_sequence);

        let authority = stage
            .root_authority
            .expect("root placement authority for each settled commit");
        let geometry = stage.expected_geometry;
        let logical = authority
            .logical_window_geometry
            .expect("logical window frame authority");
        assert!(
            authority.visual_geometry.is_none(),
            "stage {stage_index}: the converged transition no longer overrides the canonical frame"
        );
        let frame_origin = authority
            .logical_frame_origin
            .expect("resolved logical frame origin");
        assert_eq!(frame_origin, (72, 72));
        assert_eq!(logical.placement, authority.canonical_surface_placement);
        let expected_render_origin = (frame_origin.0 - geometry.x, frame_origin.1 - geometry.y);
        let expected_render_placement = SurfacePlacement {
            local_x: authority
                .canonical_surface_placement
                .local_x
                .saturating_sub(geometry.x),
            local_y: authority
                .canonical_surface_placement
                .local_y
                .saturating_sub(geometry.y),
            ..authority.canonical_surface_placement
        };
        let expected_root_origin = (
            expected_render_origin.0 + root.content_x,
            expected_render_origin.1 + root.content_y,
        );
        assert_eq!(
            authority.committed_window_geometry,
            Some(geometry),
            "stage {stage_index}: committed XDG geometry is the parent's published state"
        );
        assert_eq!(
            (root.render_x, root.render_y),
            (
                expected_render_placement.local_x,
                expected_render_placement.local_y
            ),
            "stage {stage_index}: derived root render placement; authority={authority:?}; root={root:?}; child={child:?}"
        );
        if geometry.x == 0 && geometry.y == 0 {
            assert!(
                authority.render_placement.is_none()
                    || authority.render_placement == Some(expected_render_placement),
                "stage {stage_index}: unmodified root placement is already the correct buffer origin; authority={authority:?}"
            );
        } else {
            assert_eq!(
                authority.render_placement,
                Some(expected_render_placement),
                "stage {stage_index}: renderable root stores the derived placement; authority={authority:?}"
            );
        }
        assert_eq!(
            authority.renderable_placement,
            authority.canonical_surface_placement
        );
        assert_eq!(
            (root.origin_x, root.origin_y),
            expected_root_origin,
            "stage {stage_index}: root resolved origin; authority={authority:?}"
        );
        assert_eq!(authority.resolved_render_origin, expected_root_origin);
        assert_eq!(
            authority.active_scene_origin,
            Some(expected_root_origin),
            "stage {stage_index}: active-scene root converges in this publication"
        );
        let expected_child_origin = (
            root.origin_x + child.local_x + child.content_x,
            root.origin_y + child.local_y + child.content_y,
        );
        assert_eq!(
            (child.origin_x, child.origin_y),
            expected_child_origin,
            "stage {stage_index}: child global origin follows committed client position"
        );
        assert_eq!(
            child.active_scene_origin,
            Some(expected_child_origin),
            "stage {stage_index}: active-scene child converges in this publication"
        );
        assert_eq!(
            expected_child_origin,
            (
                frame_origin.0 + root.content_x + child.content_x,
                frame_origin.1 + root.content_y + child.content_y,
            ),
            "stage {stage_index}: matching XDG origin and client position anchor content to frame"
        );

        if let Some(root_id) = expected_root_id {
            assert_eq!(root.surface_id, root_id);
        } else {
            expected_root_id = Some(root.surface_id);
        }
        if let Some(child_id) = expected_child_id {
            assert_eq!(child.surface_id, child_id);
        } else {
            expected_child_id = Some(child.surface_id);
        }
        if let Some(relationship) = expected_relationship {
            assert_eq!(child.relationship_id, Some(relationship));
        } else {
            expected_relationship = child.relationship_id;
            assert!(expected_relationship.is_some());
        }
        if let Some(origin) = anchored_content_origin {
            assert_eq!(expected_child_origin, origin);
        } else {
            anchored_content_origin = Some(expected_child_origin);
        }
    }
}

#[test]
fn gecko_zero_sized_restore_visual_converges_tree_and_active_scene() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let subcompositor: client_wl_subcompositor::WlSubcompositor =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let viewporter: client_wp_viewporter::WpViewporter = globals.bind(&qh, 1..=1, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let mut state = RegistryTestState::default();

    commands
        .send(ServerCommand::SetOutputSize {
            width: 1920,
            height: 1080,
        })
        .unwrap();
    wait_for_server_commands(&commands);

    let child = compositor.create_surface(&qh, ());
    let _initial_child_buffer = attach_test_buffered_surface(&child, &shm, &qh, 1, 1).unwrap();
    child.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let parent = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());
    toplevel.set_fullscreen(None);
    xdg_surface.set_window_geometry(0, 0, 1920, 1080);
    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert!(state.toplevel_has_state(client_xdg_toplevel::State::Fullscreen));

    commit_test_buffered_surface(&parent, &shm, &qh, 1920, 1080).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);

    let viewport = viewporter.get_viewport(&child, &qh, ());
    let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    subsurface.set_desync();
    subsurface.set_position(0, 0);
    viewport.set_destination(1920, 1080);
    commit_test_buffered_surface(&child, &shm, &qh, 1920, 1080).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);

    let root_surface_id = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .find(|surface| surface.parent_surface_id.is_none())
        .map(|surface| surface.surface_id)
        .expect("mapped XDG root");
    state.suppress_xdg_surface_ack = true;
    state.suppress_xdg_surface_commit = true;
    let configure_count_before_restore = state.surface_configure_serials.len();
    commands
        .send(ServerCommand::ToggleFullscreenFocused)
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert!(!state.toplevel_has_state(client_xdg_toplevel::State::Fullscreen));

    assert!(state.surface_configure_serials.len() > configure_count_before_restore);
    let restore_configure_serial = capture_configure_serial(&commands);
    assert!(
        state.surface_configure_serials[configure_count_before_restore..]
            .contains(&restore_configure_serial)
    );
    let pre_response_authority = capture_xdg_root_placement_authority(&commands, root_surface_id)
        .expect("root placement authority immediately after restore configure");
    let pre_response_visual = pre_response_authority
        .visual_geometry
        .expect("mode visual must remain installed before configure ACK and commit");
    assert!(pre_response_visual.mode_transition);
    assert!(!pre_response_visual.active_resize);
    assert!(pre_response_visual.width == 0 || pre_response_visual.height == 0);
    assert_eq!(
        pre_response_visual.xdg_configure_serial,
        Some(restore_configure_serial)
    );
    assert_eq!(pre_response_visual.ack_commit_sequence_floor, None);

    // This root commit was received before ACK C, so it cannot satisfy the
    // ACK-to-commit response fence.
    commit_test_buffered_surface(&parent, &shm, &qh, 540, 535).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);
    let pre_ack_commit_sequence = capture_surface_presentation_lineage(&commands, root_surface_id)
        .expect("pre-ACK root commit should publish")
        .1
        .get();
    xdg_surface.ack_configure(restore_configure_serial);
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);
    let ack_only_authority = capture_xdg_root_placement_authority(&commands, root_surface_id)
        .expect("root placement authority after ACK without a following root commit");
    let ack_only_visual = ack_only_authority
        .visual_geometry
        .expect("ACK alone must not retire the mode visual");
    assert!(ack_only_visual.mode_transition);
    assert!(!ack_only_visual.active_resize);
    assert_eq!(
        ack_only_visual.xdg_configure_serial,
        Some(restore_configure_serial)
    );
    let ack_commit_sequence_floor = ack_only_visual
        .ack_commit_sequence_floor
        .expect("ACK should capture the root commit sequence boundary");
    assert!(
        pre_ack_commit_sequence <= ack_commit_sequence_floor,
        "a root commit published before ACK C must stay below its commit barrier"
    );

    let geometry = XdgWindowGeometry::new(10, 10, 520, 515);
    xdg_surface.set_window_geometry(geometry.x, geometry.y, geometry.width, geometry.height);
    subsurface.set_position(geometry.x, geometry.y);
    viewport.set_destination(geometry.width, geometry.height);
    commit_test_buffered_surface(&child, &shm, &qh, 520, 515).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);
    assert!(
        capture_xdg_root_placement_authority(&commands, root_surface_id)
            .expect("root placement authority after child commit only")
            .visual_geometry
            .is_some()
    );
    commit_test_buffered_surface(&parent, &shm, &qh, 540, 535).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);
    let response_commit_sequence = capture_surface_presentation_lineage(&commands, root_surface_id)
        .expect("post-ACK root commit should publish")
        .1
        .get();
    assert!(response_commit_sequence > ack_commit_sequence_floor);
    state.suppress_xdg_surface_ack = false;
    state.suppress_xdg_surface_commit = false;

    let authority = capture_xdg_root_placement_authority(&commands, root_surface_id)
        .expect("root placement authority");
    let current_visual_geometry = capture_root_window_geometry(&commands, root_surface_id);
    let tree = capture_renderable_surface_snapshot(&commands);
    let root = tree
        .iter()
        .find(|surface| surface.parent_surface_id.is_none())
        .expect("renderable root");
    let child = tree
        .iter()
        .find(|surface| surface.parent_surface_id.is_some())
        .expect("renderable content child");
    commands
        .send(ServerCommand::BeginResize {
            x: 72.0 + 518.0,
            y: 72.0 + 513.0,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    let resize_interaction = capture_window_interaction_debug_snapshot(&commands);
    let resize_start_size = capture_window_interaction_start_size(&commands);
    commands.send(ServerCommand::EndInteraction).unwrap();
    wait_for_server_commands(&commands);
    commands.send(ServerCommand::ToggleMaximizeFocused).unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let next_mode_source_geometry = capture_root_restore_geometry(&commands, root_surface_id);
    let expected_render_placement = SurfacePlacement {
        local_x: authority
            .canonical_surface_placement
            .local_x
            .saturating_sub(geometry.x),
        local_y: authority
            .canonical_surface_placement
            .local_y
            .saturating_sub(geometry.y),
        ..authority.canonical_surface_placement
    };
    let _server = stop_controllable_test_server(commands, server_thread);
    assert_eq!(authority.committed_window_geometry, Some(geometry));
    assert_eq!(authority.logical_frame_origin, Some((72, 72)));
    assert_eq!(
        authority.visual_geometry, None,
        "a matching non-zero client geometry must retire the zero-sized mode visual"
    );
    assert_eq!(
        authority
            .logical_window_geometry
            .map(|geometry| (geometry.width, geometry.height)),
        Some((520, 515))
    );
    assert_eq!(current_visual_geometry, authority.logical_window_geometry);
    assert_eq!(
        current_visual_geometry.map(|geometry| (geometry.width, geometry.height)),
        Some((520, 515))
    );
    assert_eq!(
        resize_interaction.is_some_and(|interaction| matches!(
            interaction.kind,
            crate::compositor::WindowInteractionKind::Resize(_)
        )),
        true,
        "normal resize path should begin after zero-sized mode visual retirement"
    );
    assert_eq!(resize_start_size, Some((520, 515)));
    assert_eq!(
        next_mode_source_geometry.map(|geometry| (geometry.width, geometry.height)),
        Some((520, 515)),
        "the next mode transition must capture canonical non-zero dimensions"
    );
    assert_eq!(next_mode_source_geometry, authority.logical_window_geometry);
    assert_eq!((root.origin_x, root.origin_y), (62, 62));
    assert_eq!((child.origin_x, child.origin_y), (72, 72));
    assert_eq!(authority.active_scene_origin, Some((62, 62)));
    assert_eq!(child.active_scene_origin, Some((72, 72)));
    assert_eq!((root.render_x, root.render_y), (62, 62));
    assert_eq!(authority.render_placement, Some(expected_render_placement));
}
