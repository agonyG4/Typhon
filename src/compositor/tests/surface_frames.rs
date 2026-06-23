use super::*;

#[test]
fn wayland_client_receives_frame_done_after_surface_commit() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let state = create_surface_with_frame_callback(&socket_path);
    stop_test_server(running, server_thread);

    assert!(state.unwrap().frame_done);
}

#[test]
fn wayland_client_frame_done_for_buffer_commit_waits_for_present() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_surface_with_buffer_frame_callback(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(state.unwrap().frame_done);
}

#[test]
fn server_reports_pending_frame_callbacks_until_present_frame() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_surface_with_unpresented_buffer_frame_callback(&socket_path).unwrap();
    wait_for_server_commands(&commands);

    assert!(capture_pending_frame_callbacks(&commands));
    assert!(!capture_only_pending_surface_frame_callbacks(&commands));
    commands.send(ServerCommand::PresentFrame).unwrap();
    wait_for_server_commands(&commands);
    assert!(!capture_pending_frame_callbacks(&commands));

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn finish_frame_completes_frame_callbacks_after_prepare_frame_only_flushes() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_surface_with_unpresented_buffer_frame_callback(&socket_path).unwrap();
    wait_for_server_commands(&commands);

    assert!(capture_pending_frame_callbacks(&commands));
    commands.send(ServerCommand::PrepareFrame).unwrap();
    wait_for_server_commands(&commands);
    assert!(capture_pending_frame_callbacks(&commands));
    commands.send(ServerCommand::FinishFrame).unwrap();
    wait_for_server_commands(&commands);
    assert!(!capture_pending_frame_callbacks(&commands));

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn visible_frame_callback_without_damage_is_protocol_only_frame_work() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let protocol_only =
        create_visible_surface_frame_callback_without_commit_and_capture_protocol_only(
            &socket_path,
            &commands,
        );
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(protocol_only.unwrap());
}

#[test]
fn present_frame_completes_frame_callback_requested_after_visible_commit() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state =
        create_visible_surface_frame_callback_without_commit_and_present(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(state.unwrap().frame_done);
}

#[test]
fn wayland_client_frame_done_reports_elapsed_millisecond_time() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_surface_with_delayed_buffer_frame_callback(
        &socket_path,
        &commands,
        Duration::from_millis(25),
    );
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert!(state.frame_done);
    assert!(state.frame_done_time.unwrap_or_default() >= 10);
}

#[test]
fn wayland_client_buffer_release_for_buffer_commit_waits_for_present() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_surface_with_buffer_release(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(state.unwrap().buffer_release_count, 1);
}

#[test]
fn wayland_client_dmabuf_release_is_not_sent_on_same_present_as_replacement() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_dmabuf_surface_then_replace_buffer(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(state.unwrap().buffer_release_count, 0);
}

#[test]
fn wayland_client_dmabuf_release_is_deferred_for_one_present_after_replacement() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state =
        create_dmabuf_surface_then_replace_buffer_and_present_twice(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(state.unwrap().buffer_release_count, 1);
}

#[test]
fn wayland_client_surface_commit_creates_renderable_shm_snapshot() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let result = create_client_toplevel_with_shm_buffer(&socket_path);
    let server = stop_test_server(running, server_thread);

    result.unwrap();
    assert_eq!(server.renderable_surfaces().len(), 1);
    let surface = &server.renderable_surfaces()[0];
    assert_eq!(surface.width, 2);
    assert_eq!(surface.height, 2);
    assert_eq!(surface.generation, 1);
    assert_eq!(
        server.render_generation_cause(),
        RenderGenerationCause::SurfaceCommit
    );
    assert_eq!(surface.buffer_source(), SurfaceBufferSource::Shm);
    assert_eq!(
        surface.cpu_pixels(),
        Some(vec![0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff].as_slice())
    );
}

#[test]
fn wayland_surface_damage_only_commit_updates_existing_shm_snapshot() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    create_client_toplevel_with_shm_damage_only_update(&socket_path).unwrap();
    let server = stop_test_server(running, server_thread);

    assert_eq!(server.renderable_surfaces().len(), 1);
    let surface = &server.renderable_surfaces()[0];
    assert_eq!(surface.generation, 2);
    assert_eq!(
        server.render_generation_cause(),
        RenderGenerationCause::SurfaceDamage
    );
    assert_eq!(
        surface.cpu_pixels(),
        Some(vec![0xffaa_0000, 0xff00_aa00, 0xff00_00aa, 0xffaa_aa00].as_slice())
    );
}

#[test]
fn wayland_viewport_destination_sets_renderable_surface_logical_size() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    create_client_surface_with_viewport_destination(&socket_path, 20, 10, 40, 25).unwrap();
    let server = stop_test_server(running, server_thread);

    assert_eq!(server.renderable_surfaces().len(), 1);
    let surface = &server.renderable_surfaces()[0];
    assert_eq!((surface.width, surface.height), (40, 25));
    assert_eq!(
        (surface.buffer_size().width, surface.buffer_size().height),
        (20, 10)
    );
}

#[test]
fn wayland_buffer_scale_sets_renderable_surface_logical_size() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    create_client_surface_with_buffer_scale(&socket_path, 600, 400, 2).unwrap();
    let server = stop_test_server(running, server_thread);

    assert_eq!(server.renderable_surfaces().len(), 1);
    let surface = &server.renderable_surfaces()[0];
    assert_eq!((surface.width, surface.height), (300, 200));
    assert_eq!(
        (surface.buffer_size().width, surface.buffer_size().height),
        (600, 400)
    );
}

#[test]
fn wayland_client_surface_commit_tracks_dmabuf_handle() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let result = create_client_toplevel_with_dmabuf_buffer(&socket_path);
    let server = stop_test_server(running, server_thread);

    result.unwrap();
    assert_eq!(server.renderable_surfaces().len(), 1);
    let surface = &server.renderable_surfaces()[0];
    assert_eq!(surface.width, 2);
    assert_eq!(surface.height, 2);
    assert_eq!(surface.buffer_source(), SurfaceBufferSource::Dmabuf);
    assert!(surface.cpu_pixels().is_none());
}

#[test]
fn wayland_surface_can_switch_from_shm_snapshot_to_dmabuf_handle() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let result = create_client_toplevel_with_shm_then_dmabuf_buffer(&socket_path);
    let server = stop_test_server(running, server_thread);

    result.unwrap();
    assert_eq!(server.renderable_surfaces().len(), 1);
    let surface = &server.renderable_surfaces()[0];
    assert_eq!(surface.width, 2);
    assert_eq!(surface.height, 2);
    assert_eq!(surface.generation, 2);
    assert_eq!(surface.buffer_source(), SurfaceBufferSource::Dmabuf);
    assert!(surface.cpu_pixels().is_none());
}

#[test]
fn shm_read_pixels_into_reuses_existing_pixel_storage_for_same_size() {
    let file = Arc::new(
        create_test_shm_file(&[0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff]).unwrap(),
    );
    let data = ShmBufferData {
        identity: BufferIdAllocator::default()
            .allocate()
            .expect("test buffer identity"),
        pool_size: 16,
        file,
        offset: 0,
        width: 2,
        height: 2,
        stride: 8,
        format: wayland_server::WEnum::Value(wl_shm::Format::Argb8888),
    };
    let mut pixels = vec![0; 4];
    let before = pixels.as_ptr();

    data.read_pixels_into(&mut pixels).unwrap();

    assert_eq!(
        pixels,
        vec![0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff]
    );
    assert_eq!(pixels.as_ptr(), before);
}

#[test]
fn shm_read_pixels_into_with_damage_updates_only_dirty_rect() {
    let file = Arc::new(
        create_test_shm_file(&[0xff11_1111, 0xff22_2222, 0xff33_3333, 0xff44_4444]).unwrap(),
    );
    let data = ShmBufferData {
        identity: BufferIdAllocator::default()
            .allocate()
            .expect("test buffer identity"),
        pool_size: 16,
        file,
        offset: 0,
        width: 2,
        height: 2,
        stride: 8,
        format: wayland_server::WEnum::Value(wl_shm::Format::Argb8888),
    };
    let mut pixels = vec![0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff];

    data.read_pixels_into_with_damage(
        &mut pixels,
        &RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
            x: 1,
            y: 0,
            width: 1,
            height: 2,
        }]),
    )
    .unwrap();

    assert_eq!(
        pixels,
        vec![0xffff_0000, 0xff22_2222, 0xff00_00ff, 0xff44_4444]
    );
}

#[test]
fn full_surface_damage_normalizes_to_full_upload() {
    let damage = RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
        x: 0,
        y: 0,
        width: 1280,
        height: 800,
    }]);

    assert_eq!(
        damage.normalized_for_surface(1280, 800),
        RenderableSurfaceDamage::Full
    );
}

#[test]
fn empty_surface_damage_does_not_become_full() {
    let damage = RenderableSurfaceDamage::from_rects(Vec::new());

    assert!(
        !damage.is_full(),
        "an empty rectangle list must mean no visual damage"
    );
}

#[test]
fn surface_damage_union_retains_every_commit_region() {
    let damage = RenderableSurfaceDamage::Empty
        .union(
            RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
                x: 0,
                y: 0,
                width: 4,
                height: 4,
            }]),
            20,
            10,
        )
        .union(
            RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
                x: 8,
                y: 0,
                width: 4,
                height: 4,
            }]),
            20,
            10,
        )
        .union(
            RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
                x: 16,
                y: 0,
                width: 4,
                height: 4,
            }]),
            20,
            10,
        );

    assert_eq!(damage.clipped_rects(20, 10).len(), 3);
}

#[test]
fn surface_damage_union_normalizes_complete_coverage_to_full() {
    let damage = RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
        x: 0,
        y: 0,
        width: 5,
        height: 10,
    }])
    .union(
        RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
            x: 5,
            y: 0,
            width: 5,
            height: 10,
        }]),
        10,
        10,
    );

    assert_eq!(damage, RenderableSurfaceDamage::Full);
}

#[test]
fn surface_damage_journal_unions_unseen_commits_and_reports_loss() {
    let mut journal = SurfaceDamageJournal::new(2);
    let initial = journal.current_commit();
    journal.record(
        RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
            x: 0,
            y: 0,
            width: 2,
            height: 2,
        }]),
        10,
        10,
    );
    let after_first = journal.current_commit();
    journal.record(
        RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
            x: 4,
            y: 4,
            width: 2,
            height: 2,
        }]),
        10,
        10,
    );

    assert!(matches!(
        journal.damage_since(after_first, 10, 10),
        DamageSince::Known(RenderableSurfaceDamage::Partial(rects)) if rects.len() == 1
    ));
    journal.record(RenderableSurfaceDamage::Empty, 10, 10);
    assert_eq!(
        journal.damage_since(initial, 10, 10),
        DamageSince::HistoryLost
    );
    assert_eq!(
        journal.damage_since(journal.current_commit(), 10, 10),
        DamageSince::Empty
    );
}

#[test]
fn wayland_damage_rects_clip_to_surface_bounds() {
    let damage = RenderableSurfaceDamage::Partial(vec![
        SurfaceDamageRect::from_wayland_rect(-2, -1, 4, 3).unwrap(),
        SurfaceDamageRect::from_wayland_rect(3, 3, 10, 10).unwrap(),
    ]);

    assert_eq!(
        damage.clipped_rects(4, 4),
        vec![
            SurfaceDamageRect {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
            },
            SurfaceDamageRect {
                x: 3,
                y: 3,
                width: 1,
                height: 1,
            }
        ]
    );
}
