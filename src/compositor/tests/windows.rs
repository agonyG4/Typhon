use super::*;

#[test]
fn window_drag_moves_root_surface_without_moving_children_independently() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_client_toplevel_with_positioned_subsurface_buffer(&socket_path).unwrap();
    wait_for_server_commands(&commands);
    commands
        .send(ServerCommand::BeginMove {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 1.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 1.0,
        })
        .unwrap();
    commands
        .send(ServerCommand::UpdateInteraction {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 33.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 25.0,
        })
        .unwrap();
    commands.send(ServerCommand::EndInteraction).unwrap();
    wait_for_server_commands(&commands);
    assert_eq!(
        capture_render_generation_cause(&commands),
        RenderGenerationCause::WindowMove
    );
    let server = stop_controllable_test_server(commands, server_thread);

    let origins = render::surface_origins(server.renderable_surfaces());
    assert_eq!(
        server.render_generation_cause(),
        RenderGenerationCause::WindowMove
    );
    let parent_index = server
        .renderable_surfaces()
        .iter()
        .position(|surface| surface.width == 2 && surface.height == 2)
        .expect("parent toplevel should remain renderable");
    let child_index = server
        .renderable_surfaces()
        .iter()
        .position(|surface| surface.width == 1 && surface.height == 1)
        .expect("child subsurface should remain renderable");

    assert_eq!(origins[parent_index], (104, 96));
    assert_eq!(origins[child_index], (114, 108));
}

#[test]
fn wayland_client_receives_resize_configure_for_focused_window() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state =
        create_configured_client_toplevel_then_resize_focused(&socket_path, &commands, 960, 640);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert_eq!(state.toplevel_width, 960);
    assert_eq!(state.toplevel_height, 640);
}

#[test]
fn window_minimize_hides_focused_toplevel_until_restored() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_client_toplevel_with_sized_shm_buffer(&socket_path, 300, 200).unwrap();
    wait_for_server_commands(&commands);
    commands.send(ServerCommand::MinimizeFocused).unwrap();
    wait_for_server_commands(&commands);
    commands.send(ServerCommand::RestoreNextMinimized).unwrap();
    wait_for_server_commands(&commands);
    let server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(server.renderable_surfaces().len(), 1);
    assert_eq!(server.renderable_surfaces()[0].width, 300);
    assert_eq!(server.renderable_surfaces()[0].height, 200);
}

#[test]
fn minimized_toplevel_stays_hidden_when_client_commits_new_buffer() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let client = LiveTestClient::connect(&socket_path).unwrap();
    let surface = client
        .create_toplevel_surface("oblivion.minimize-commit-test", 300, 200)
        .unwrap();
    wait_for_server_commands(&commands);
    assert_eq!(capture_renderable_surface_count(&commands), 1);

    commands.send(ServerCommand::MinimizeFocused).unwrap();
    wait_for_server_commands(&commands);
    assert_eq!(capture_renderable_surface_count(&commands), 0);

    client.commit_surface(&surface, 320, 220).unwrap();
    wait_for_server_commands(&commands);
    assert_eq!(capture_renderable_surface_count(&commands), 0);

    commands.send(ServerCommand::RestoreNextMinimized).unwrap();
    wait_for_server_commands(&commands);
    let server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(server.renderable_surfaces().len(), 1);
    assert_eq!(server.renderable_surfaces()[0].width, 320);
    assert_eq!(server.renderable_surfaces()[0].height, 220);
}

#[test]
fn window_maximize_configures_focused_toplevel_and_restores_geometry() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_buffered_toplevel_then_toggle_maximize(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(state.toplevel_width, 1280);
    assert_eq!(state.toplevel_height, 800);
    assert!(state.toplevel_has_state(client_xdg_toplevel::State::Maximized));
}

#[test]
fn window_maximize_uses_current_output_size() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    commands
        .send(ServerCommand::SetOutputSize {
            width: 1600,
            height: 900,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    let state = create_buffered_toplevel_then_toggle_maximize(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(state.toplevel_width, 1600);
    assert_eq!(state.toplevel_height, 900);
    assert!(state.toplevel_has_state(client_xdg_toplevel::State::Maximized));
}

#[test]
fn window_unmaximize_restores_previous_toplevel_geometry() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state =
        create_buffered_toplevel_then_toggle_maximize_twice(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(state.toplevel_width, 300);
    assert_eq!(state.toplevel_height, 200);
    assert!(!state.toplevel_has_state(client_xdg_toplevel::State::Maximized));
}

#[test]
fn window_fullscreen_configures_focused_toplevel_and_restores_geometry() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_buffered_toplevel_then_toggle_fullscreen(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(state.toplevel_width, 1280);
    assert_eq!(state.toplevel_height, 800);
    assert!(state.toplevel_has_state(client_xdg_toplevel::State::Fullscreen));
}

#[test]
fn window_resize_drag_sends_configure_for_root_surface() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_buffered_toplevel_then_resize_drag(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert_eq!(state.toplevel_width, 300);
    assert_eq!(state.toplevel_height, 200);
}

#[test]
fn resize_drag_coalesces_pointer_updates_behind_in_flight_configure() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_buffered_toplevel_then_coalesced_resize_drag(&socket_path, &commands);
    let server = stop_controllable_test_server(commands, server_thread);
    let origins = render::surface_origins(server.renderable_surfaces());

    let state = state.unwrap();
    assert_eq!(state.toplevel_configure_count, 3);
    assert_eq!(state.toplevel_width, 340);
    assert_eq!(state.toplevel_height, 230);
    assert!(!state.toplevel_has_state(client_xdg_toplevel::State::Resizing));
    assert_eq!(origins.first().copied(), Some(render::FIRST_SURFACE_OFFSET));
    let metrics = server.resize_flow_metrics();
    assert_eq!(metrics.raw_pointer_resize_updates, 3);
    assert_eq!(metrics.pending_resize_updates_replaced, 2);
    assert_eq!(metrics.resize_updates_applied, 1);
}

#[test]
fn resize_drag_configure_reports_resizing_state_while_active() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_buffered_toplevel_then_active_resize_configure(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert!(state.toplevel_has_state(client_xdg_toplevel::State::Resizing));
}

#[test]
fn resize_drag_does_not_send_next_configure_without_client_progress() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_buffered_toplevel_then_resize_drag_without_client_commit_between_frames(
        &socket_path,
        &commands,
    );
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert_eq!(state.toplevel_configure_count, 3);
}

#[test]
fn queued_resize_configure_reports_pending_frame_work() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let pending =
        create_buffered_toplevel_then_queue_resize_configure_and_capture_pending_frame_work(
            &socket_path,
            &commands,
        )
        .unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(pending);
}

#[test]
fn unmapping_resized_window_clears_queued_resize_configure() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let pending =
        create_buffered_toplevel_then_queue_resize_configure_and_unmap(&socket_path, &commands)
            .unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(!pending);
}

#[test]
fn prepare_frame_flushes_queued_resize_configure_before_present_frame() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let (before_prepare, after_prepare) =
        create_buffered_toplevel_then_prepare_queued_resize_configure_and_capture_pending_frame_work(
            &socket_path,
            &commands,
        )
        .unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(before_prepare);
    assert!(!after_prepare);
}

#[test]
fn resize_drag_updates_visual_target_before_client_commit() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_client_toplevel_with_sized_shm_buffer(&socket_path, 300, 200).unwrap();
    wait_for_server_commands(&commands);
    commands
        .send(ServerCommand::BeginFrameAction {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 304.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 204.0,
        })
        .unwrap();
    commands
        .send(ServerCommand::UpdateInteraction {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 344.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 234.0,
        })
        .unwrap();
    commands.send(ServerCommand::PrepareFrame).unwrap();
    wait_for_server_commands(&commands);
    let server = stop_controllable_test_server(commands, server_thread);

    let surface = &server.renderable_surfaces()[0];
    assert_eq!(server.renderable_surfaces().len(), 1);
    assert_eq!(surface.width, 300);
    assert_eq!(surface.height, 200);
    assert_eq!(surface.generation, 1);
    assert_eq!(
        surface.visual_clip,
        Some(render::SurfaceTargetRect::new(0, 0, 340, 230))
    );
}

fn state_with_preview_resize(
    resizing: bool,
) -> (
    CompositorState,
    u32,
    ResizeCommitSnapshot,
    PendingResizeConfigure,
) {
    let mut state = CompositorState::default();
    let surface_id = 42;
    let identity = BufferIdAllocator::default()
        .allocate()
        .expect("test buffer identity");
    let desired = PendingResizeConfigure {
        surface_id,
        width: 1200,
        height: 700,
        placement: SurfacePlacement::root_at(100, 80),
        edges: ResizeEdges::BOTTOM_RIGHT,
        resizing,
        interaction_id: ResizeInteractionId::new(1),
    };
    let resize = ResizeCommitSnapshot {
        serial: 7,
        sequence: 1,
        commit_sequence: 1,
        width: desired.width,
        height: desired.height,
        placement: desired.placement,
        edges: desired.edges,
        resizing,
        emitted_at: Instant::now(),
        committed_size: Some((944, 502)),
        buffer_id: Some(identity.id().get()),
        interaction_id: desired.interaction_id,
    };

    state.renderable_surfaces.push(RenderableSurface {
        surface_id,
        x: 0,
        y: 0,
        width: 944,
        height: 502,
        placement: SurfacePlacement::root(),
        render_placement: None,
        visual_clip: None,
        generation: 1,
        commit_sequence: SurfaceCommitSequence::initial(),
        buffer: crate::render_backend::buffer::CommittedSurfaceBuffer::shm_snapshot(
            identity,
            BufferSize::new(944, 502).expect("test size"),
            vec![0; 944 * 502],
        ),
        damage: RenderableSurfaceDamage::Full,
    });
    state
        .surface_window_geometries
        .insert(surface_id, XdgWindowGeometry::new(0, 0, 944, 502));
    state.active_toplevel_resizes.insert(
        surface_id,
        ActiveToplevelResize {
            interaction_id: desired.interaction_id,
            flow_sequence: 1,
            edges: desired.edges,
            activated_at: Instant::now(),
        },
    );
    assert!(state.preview_resize_root_window_to(
        surface_id,
        desired.width,
        desired.height,
        desired.placement,
        desired.edges,
        desired.interaction_id,
    ));
    (state, surface_id, resize, desired)
}

#[test]
fn intermediate_geometry_only_commit_preserves_active_resize_preview() {
    let (mut state, surface_id, resize, _desired) = state_with_preview_resize(true);

    assert!(state.complete_pending_resize_from_current_geometry(surface_id, resize));

    assert!(
        !state
            .resize_configure_flows
            .get(&surface_id)
            .is_some_and(ResizeConfigureFlow::has_in_flight)
    );
    let surface = &state.renderable_surfaces[0];
    assert_eq!(
        surface.visual_clip,
        Some(render::SurfaceTargetRect::new(100, 80, 1200, 700))
    );

    let visual = state
        .current_visual_root_window_geometry(surface_id)
        .expect("visual geometry");
    let committed = state
        .current_root_window_geometry(surface_id)
        .expect("committed geometry");

    assert_eq!(visual.width, 1200);
    assert_eq!(visual.height, 700);
    assert_eq!(visual.placement, SurfacePlacement::root_at(100, 80));
    assert_eq!(committed.width, 944);
    assert_eq!(committed.height, 502);
}

#[test]
fn kitty_like_resize_swapchain_never_selects_destroyed_buffer_identity() {
    let mut ids = BufferIdAllocator::default();
    let old = ids.allocate().expect("old buffer identity");
    let buffer_b = ids.allocate().expect("buffer B identity");
    let buffer_c = ids.allocate().expect("buffer C identity");
    let buffer_d = ids.allocate().expect("buffer D identity");
    let handle = DmabufBufferHandle::new(
        BufferSize::new(1000, 696).expect("cell-aligned test size"),
        DrmFormat::Xrgb8888,
        vec![RenderDmabufPlane::new(
            File::open("/dev/null")
                .expect("dmabuf identity test fd")
                .into(),
            DmabufPlaneDescriptor {
                plane_index: 0,
                offset: 0,
                stride: 4000,
                modifier: DrmModifier::LINEAR,
            },
        )],
    )
    .expect("valid fake dmabuf");
    let old_key = crate::render_backend::buffer::DmabufImageKey::from_handle(old.id(), &handle);
    let keys = [buffer_b, buffer_c, buffer_d].map(|identity| {
        crate::render_backend::buffer::DmabufImageKey::from_handle(identity.id(), &handle)
    });
    let mut fake_renderer_cache = HashMap::from([(old_key.clone(), 'A')]);
    let mut selected = Vec::new();

    fake_renderer_cache.remove(&old_key);
    for (key, frame) in keys.into_iter().zip(['B', 'C', 'D']) {
        fake_renderer_cache.insert(key.clone(), frame);
        selected.push(*fake_renderer_cache.get(&key).expect("new renderer input"));
    }

    assert_eq!(selected, ['B', 'C', 'D']);
    assert!(!fake_renderer_cache.contains_key(&old_key));
    assert!(
        fake_renderer_cache
            .keys()
            .all(|key| key.buffer_id() != old.id())
    );
}

#[test]
fn damage_only_commit_preserves_rendered_size_while_resize_is_pending() {
    let existing = BufferSize {
        width: 300,
        height: 200,
    };
    let requested = BufferSize {
        width: 900,
        height: 600,
    };

    assert_eq!(
        damage_only_rendered_surface_size(existing, requested, true),
        existing
    );
    assert_eq!(
        damage_only_rendered_surface_size(existing, requested, false),
        requested
    );
}

#[test]
fn resize_drag_end_clears_resizing_state() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_buffered_toplevel_then_resize_drag_and_release(&socket_path, &commands);
    let server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert_eq!(state.toplevel_configure_count, 3);
    assert_eq!(state.toplevel_width, 340);
    assert_eq!(state.toplevel_height, 230);
    assert!(!state.toplevel_has_state(client_xdg_toplevel::State::Resizing));
    let metrics = server.resize_flow_metrics();
    assert_eq!(metrics.max_in_flight_configures, 2);
    assert_eq!(metrics.acks_unknown, 0);
    assert!(metrics.configures_sent >= 2);
    assert!(metrics.commits_captured >= 1);
    assert_eq!(metrics.preview_completions, 0);
}

#[test]
fn alt_resize_from_top_left_grows_left_and_up() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state =
        create_buffered_toplevel_then_alt_top_left_resize_drag_and_release(&socket_path, &commands);
    let server = stop_controllable_test_server(commands, server_thread);
    let origins = render::surface_origins(server.renderable_surfaces());

    let state = state.unwrap();
    assert_eq!(state.toplevel_width, 340);
    assert_eq!(state.toplevel_height, 230);
    assert_eq!(
        origins.first().copied(),
        Some((
            render::FIRST_SURFACE_OFFSET.0 - 40,
            render::FIRST_SURFACE_OFFSET.1 - 30
        ))
    );
}

#[test]
fn csd_buffer_margin_resize_release_keeps_configure_at_window_geometry() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_csd_toplevel_then_resize_drag_commit_buffer_margin_and_release(
        &socket_path,
        &commands,
    );
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert_eq!(state.toplevel_configure_count, 3);
    assert_eq!(state.toplevel_width, 340);
    assert_eq!(state.toplevel_height, 230);
    assert!(!state.toplevel_has_state(client_xdg_toplevel::State::Resizing));
}

#[test]
fn resize_preview_without_client_commit_advances_render_generation() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let (before_resize, after_resize) =
        create_buffered_toplevel_then_measure_configure_only_resize_generation(
            &socket_path,
            &commands,
        )
        .unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(after_resize > before_resize);
}

#[test]
fn resize_motion_inside_threshold_does_not_report_visual_update() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_client_toplevel_with_sized_shm_buffer(&socket_path, 300, 200).unwrap();
    wait_for_server_commands(&commands);
    commands
        .send(ServerCommand::BeginFrameAction {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 304.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 204.0,
        })
        .unwrap();
    let updated = update_interaction_and_report(
        &commands,
        f64::from(render::FIRST_SURFACE_OFFSET.0) + 305.0,
        f64::from(render::FIRST_SURFACE_OFFSET.1) + 205.0,
    );
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(!updated);
}

#[test]
fn resize_motion_at_same_size_does_not_report_repeated_visual_update() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_client_toplevel_with_sized_shm_buffer(&socket_path, 300, 200).unwrap();
    wait_for_server_commands(&commands);
    commands
        .send(ServerCommand::BeginFrameAction {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 304.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 204.0,
        })
        .unwrap();
    assert!(update_interaction_and_report(
        &commands,
        f64::from(render::FIRST_SURFACE_OFFSET.0) + 344.0,
        f64::from(render::FIRST_SURFACE_OFFSET.1) + 234.0,
    ));
    let updated = update_interaction_and_report(
        &commands,
        f64::from(render::FIRST_SURFACE_OFFSET.0) + 344.0,
        f64::from(render::FIRST_SURFACE_OFFSET.1) + 234.0,
    );
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(!updated);
}

#[test]
fn frame_corner_resize_click_with_tiny_motion_does_not_resize_window() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_buffered_toplevel_then_frame_corner_resize_click_with_tiny_motion(
        &socket_path,
        &commands,
    )
    .unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(state.toplevel_configure_count, 1);
}

#[test]
fn left_edge_resize_shrink_updates_visual_target_before_client_commit() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_buffered_toplevel_then_left_edge_shrink_before_client_commit(
        &socket_path,
        &commands,
    )
    .unwrap();
    let server = stop_controllable_test_server(commands, server_thread);
    let origins = render::surface_origins(server.renderable_surfaces());

    assert_eq!(state.toplevel_width, 260);
    assert_eq!(state.toplevel_height, 200);
    assert_eq!(server.renderable_surfaces()[0].width, 300);
    assert_eq!(server.renderable_surfaces()[0].height, 200);
    assert_eq!(
        server.renderable_surfaces()[0].visual_clip,
        Some(render::SurfaceTargetRect::new(40, 0, 260, 200))
    );
    assert_eq!(
        origins.first().copied(),
        Some((
            render::FIRST_SURFACE_OFFSET.0 + 40,
            render::FIRST_SURFACE_OFFSET.1,
        ))
    );
    assert_eq!(origins[0].0 + 260, render::FIRST_SURFACE_OFFSET.0 + 300);
}

#[test]
fn resize_preview_clamps_to_toplevel_min_size_before_client_commit() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state =
        create_min_size_toplevel_then_shrink_resize_before_client_commit(&socket_path, &commands)
            .unwrap();
    let server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(state.toplevel_width, 280);
    assert_eq!(state.toplevel_height, 180);
    let surface = server
        .renderable_surfaces()
        .first()
        .expect("toplevel should remain renderable");
    assert_eq!((surface.width, surface.height), (320, 220));
    assert_eq!(
        surface.visual_clip,
        Some(render::SurfaceTargetRect::new(0, 0, 280, 180))
    );
}

#[test]
fn scaled_buffer_resize_shrink_does_not_commit_physical_size_as_logical_size() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state =
        create_scaled_buffer_toplevel_then_right_edge_shrink_and_commit(&socket_path, &commands)
            .unwrap();
    let server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(state.toplevel_width, 260);
    let surface = server
        .renderable_surfaces()
        .first()
        .expect("scaled toplevel should remain renderable");
    assert_eq!((surface.width, surface.height), (260, 200));
    assert_eq!(
        (surface.buffer_size().width, surface.buffer_size().height),
        (520, 400)
    );
}

#[test]
fn scaled_buffer_left_edge_shrink_keeps_logical_commit_anchor() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state =
        create_scaled_buffer_toplevel_then_left_edge_shrink_and_commit(&socket_path, &commands)
            .unwrap();
    let server = stop_controllable_test_server(commands, server_thread);
    let origins = render::surface_origins(server.renderable_surfaces());

    assert_eq!(state.toplevel_width, 260);
    let surface = server
        .renderable_surfaces()
        .first()
        .expect("scaled toplevel should remain renderable");
    assert_eq!((surface.width, surface.height), (260, 200));
    assert_eq!(
        (surface.buffer_size().width, surface.buffer_size().height),
        (520, 400)
    );
    assert_eq!(
        origins.first().copied(),
        Some((
            render::FIRST_SURFACE_OFFSET.0 + 40,
            render::FIRST_SURFACE_OFFSET.1
        ))
    );
}

#[test]
fn xdg_toplevel_move_request_starts_interactive_move_from_pointer_serial() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_buffered_toplevel_request_move_and_drag(&socket_path, &commands).unwrap();
    let server = stop_controllable_test_server(commands, server_thread);
    let origins = render::surface_origins(server.renderable_surfaces());

    assert_eq!(origins.first().copied(), Some((112, 100)));
}

#[test]
fn xdg_toplevel_move_request_accepts_serial_from_same_client_chrome_surface() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state =
        create_toplevel_request_move_from_client_chrome_surface(&socket_path, &commands).unwrap();
    let server = stop_controllable_test_server(commands, server_thread);
    let origins = render::surface_origins(server.renderable_surfaces());
    let toplevel_index = server
        .renderable_surfaces()
        .iter()
        .position(|surface| surface.width == 100 && surface.height == 80)
        .expect("toplevel should remain renderable");
    let toplevel_id = server.renderable_surfaces()[toplevel_index].surface_id;

    assert_eq!(state.pointer_surface_x, Some(12.0));
    assert_eq!(state.pointer_surface_y, Some(14.0));
    assert_eq!(
        server.state.surface_placement(toplevel_id),
        SurfacePlacement::root_at(80, 60)
    );
    assert_eq!(origins[toplevel_index], (152, 132));
}

#[test]
fn xdg_toplevel_resize_request_uses_requested_edge() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state =
        create_buffered_toplevel_request_top_left_resize_and_drag(&socket_path, &commands).unwrap();
    let server = stop_controllable_test_server(commands, server_thread);
    let origins = render::surface_origins(server.renderable_surfaces());

    assert_eq!(state.toplevel_width, 340);
    assert_eq!(state.toplevel_height, 230);
    assert_eq!(origins.first().copied(), Some((32, 42)));
}

#[test]
fn future_server_titlebar_area_does_not_move_window_without_client_request() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_client_toplevel_with_sized_shm_buffer(&socket_path, 300, 200).unwrap();
    wait_for_server_commands(&commands);
    commands
        .send(ServerCommand::BeginFrameAction {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 60.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) - 18.0,
        })
        .unwrap();
    commands
        .send(ServerCommand::UpdateInteraction {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 102.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 9.0,
        })
        .unwrap();
    commands.send(ServerCommand::EndInteraction).unwrap();
    wait_for_server_commands(&commands);
    let server = stop_controllable_test_server(commands, server_thread);
    let origins = render::surface_origins(server.renderable_surfaces());

    assert_eq!(origins.first().copied(), Some(render::FIRST_SURFACE_OFFSET));
}

#[test]
fn frame_hit_testing_keeps_content_and_future_titlebar_clicks_for_clients() {
    assert_eq!(
        window_frame_action_for_local_point(60.0, 18.0, 300, 200),
        None
    );
    assert_eq!(
        window_frame_action_for_local_point(60.0, -18.0, 300, 200),
        None
    );
    assert_eq!(
        window_frame_action_for_local_point(304.0, 204.0, 300, 200),
        Some(WindowInteractionKind::Resize(ResizeEdges::BOTTOM_RIGHT))
    );
    assert_eq!(
        window_frame_action_for_local_point(-3.0, -3.0, 300, 200),
        Some(WindowInteractionKind::Resize(ResizeEdges::new(
            true, false, true, false
        )))
    );
}

#[test]
fn frame_hit_testing_supports_xy_resize_on_all_corners() {
    assert_eq!(
        window_frame_action_for_local_point(-3.0, -3.0, 300, 200),
        Some(WindowInteractionKind::Resize(ResizeEdges::new(
            true, false, true, false
        )))
    );
    assert_eq!(
        window_frame_action_for_local_point(303.0, -3.0, 300, 200),
        Some(WindowInteractionKind::Resize(ResizeEdges::new(
            true, false, false, true
        )))
    );
    assert_eq!(
        window_frame_action_for_local_point(-3.0, 203.0, 300, 200),
        Some(WindowInteractionKind::Resize(ResizeEdges::new(
            false, true, true, false
        )))
    );
    assert_eq!(
        window_frame_action_for_local_point(303.0, 203.0, 300, 200),
        Some(WindowInteractionKind::Resize(ResizeEdges::BOTTOM_RIGHT))
    );
}

#[test]
fn browser_client_chrome_empty_band_click_does_not_start_drag_without_client_request() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_client_toplevel_with_app_id_and_sized_shm_buffer(
        &socket_path,
        "brave-browser",
        900,
        600,
    )
    .unwrap();
    wait_for_server_commands(&commands);
    commands
        .send(ServerCommand::BeginFrameAction {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 500.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 10.0,
        })
        .unwrap();
    commands
        .send(ServerCommand::UpdateInteraction {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 548.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 32.0,
        })
        .unwrap();
    commands.send(ServerCommand::EndInteraction).unwrap();
    wait_for_server_commands(&commands);
    let server = stop_controllable_test_server(commands, server_thread);
    let origins = render::surface_origins(server.renderable_surfaces());

    assert_eq!(origins.first().copied(), Some(render::FIRST_SURFACE_OFFSET));
}

#[test]
fn browser_client_chrome_tab_area_click_does_not_start_drag_fallback() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_client_toplevel_with_app_id_and_sized_shm_buffer(
        &socket_path,
        "brave-browser",
        900,
        600,
    )
    .unwrap();
    wait_for_server_commands(&commands);
    commands
        .send(ServerCommand::BeginFrameAction {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 140.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 10.0,
        })
        .unwrap();
    commands
        .send(ServerCommand::UpdateInteraction {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 188.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 32.0,
        })
        .unwrap();
    commands.send(ServerCommand::EndInteraction).unwrap();
    wait_for_server_commands(&commands);
    let server = stop_controllable_test_server(commands, server_thread);
    let origins = render::surface_origins(server.renderable_surfaces());

    assert_eq!(origins.first().copied(), Some(render::FIRST_SURFACE_OFFSET));
}

#[test]
fn browser_client_chrome_toolbar_center_click_does_not_start_drag_fallback() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_client_toplevel_with_app_id_and_sized_shm_buffer(&socket_path, "firefox", 900, 600)
        .unwrap();
    wait_for_server_commands(&commands);
    commands
        .send(ServerCommand::BeginFrameAction {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 500.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 10.0,
        })
        .unwrap();
    commands
        .send(ServerCommand::UpdateInteraction {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 548.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 32.0,
        })
        .unwrap();
    commands.send(ServerCommand::EndInteraction).unwrap();
    wait_for_server_commands(&commands);
    let server = stop_controllable_test_server(commands, server_thread);
    let origins = render::surface_origins(server.renderable_surfaces());

    assert_eq!(origins.first().copied(), Some(render::FIRST_SURFACE_OFFSET));
}

#[test]
fn non_browser_client_top_content_click_does_not_start_drag_fallback() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_client_toplevel_with_app_id_and_sized_shm_buffer(&socket_path, "kitty", 900, 600)
        .unwrap();
    wait_for_server_commands(&commands);
    commands
        .send(ServerCommand::BeginFrameAction {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 500.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 10.0,
        })
        .unwrap();
    commands
        .send(ServerCommand::UpdateInteraction {
            x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 548.0,
            y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 32.0,
        })
        .unwrap();
    commands.send(ServerCommand::EndInteraction).unwrap();
    wait_for_server_commands(&commands);
    let server = stop_controllable_test_server(commands, server_thread);
    let origins = render::surface_origins(server.renderable_surfaces());

    assert_eq!(origins.first().copied(), Some(render::FIRST_SURFACE_OFFSET));
}

#[test]
fn frame_corner_drag_resizes_window_without_client_request() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state =
        create_buffered_toplevel_then_frame_corner_resize_drag(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(state.toplevel_width, 340);
    assert_eq!(state.toplevel_height, 230);
}
