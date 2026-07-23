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
    assert!(capture_only_pending_surface_frame_callbacks(&commands));
    commands.send(ServerCommand::PresentFrame).unwrap();
    wait_for_server_commands(&commands);
    assert!(!capture_pending_frame_callbacks(&commands));

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn protocol_only_frame_tick_completes_callbacks_without_creating_a_visual_batch() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_live_surface_with_unpresented_buffer_frame_callback(&socket_path).unwrap();
    wait_for_server_commands(&commands);
    let (reply, receiver) = std::sync::mpsc::channel();
    commands
        .send(ServerCommand::CompleteProtocolOnlyFrameTick(reply))
        .unwrap();
    assert_eq!(
        receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
        ProtocolOnlyCompletion::Completed { callback_count: 1 }
    );
    assert!(!capture_pending_frame_callbacks(&commands));
    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn captured_presentation_feedback_is_not_new_frame_work() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = create_surface_with_unpresented_presentation_feedback(&socket_path).unwrap();
    retain_live_test_connection(connection);
    wait_for_server_commands(&commands);
    assert!(capture_pending_frame_work(&commands));

    let (batch_reply, batch_receiver) = std::sync::mpsc::channel();
    commands
        .send(ServerCommand::CaptureFrameBatch {
            frame_id: 1,
            reply: batch_reply,
        })
        .unwrap();
    let batch_id = batch_receiver
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();
    assert!(
        !capture_pending_frame_work(&commands),
        "feedback owned by a captured frame must not request another frame"
    );

    commands
        .send(ServerCommand::CompleteFrameBatchNow {
            frame_id: 1,
            batch_id,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn legacy_prepared_batch_is_settled_after_a_skipped_frame_without_unowned_work() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);

    commands
        .send(ServerCommand::CaptureLegacyPreparedFrame)
        .unwrap();
    wait_for_server_commands(&commands);
    assert!(!capture_pending_frame_work(&commands));

    let (prepared_reply, prepared_receiver) = std::sync::mpsc::channel();
    commands
        .send(ServerCommand::CapturePreparedFrame(prepared_reply))
        .unwrap();
    assert!(prepared_receiver.recv().unwrap());

    // This is the legacy NativePaintOutcome::Skipped terminal path: the batch was
    // captured before paint, and no new unowned work remains after that capture.
    commands.send(ServerCommand::FinishPreparedFrame).unwrap();
    wait_for_server_commands(&commands);

    let (prepared_reply, prepared_receiver) = std::sync::mpsc::channel();
    commands
        .send(ServerCommand::CapturePreparedFrame(prepared_reply))
        .unwrap();
    assert!(!prepared_receiver.recv().unwrap());

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn legacy_skipped_frame_completes_a_captured_callback_once() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_surface_with_unpresented_buffer_frame_callback(&socket_path).unwrap();
    wait_for_server_commands(&commands);
    commands
        .send(ServerCommand::CaptureLegacyPreparedFrame)
        .unwrap();
    commands.send(ServerCommand::FinishPreparedFrame).unwrap();
    wait_for_server_commands(&commands);

    let server = stop_controllable_test_server(commands, server_thread);
    let metrics = server.frame_callback_metrics();
    assert_eq!(metrics.callbacks_captured, 1);
    assert_eq!(metrics.callbacks_completed_after_render, 1);
    assert_eq!(metrics.callbacks_completed_after_abandonment, 0);
}

#[test]
fn callback_committed_after_skipped_batch_is_captured_by_the_next_frame() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let (_state, before_first_settlement, after_first_settlement, after_second_settlement) =
        exercise_legacy_skipped_frame_late_callback(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(before_first_settlement.callbacks_captured, 0);
    assert_eq!(after_first_settlement.callbacks_captured, 0);
    assert_eq!(after_second_settlement.callbacks_captured, 1);
    assert_eq!(after_second_settlement.callbacks_completed_after_render, 1);
}

#[test]
fn legacy_immediate_present_settles_feedback_and_release_once_without_new_work() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_surface_with_presentation_feedback_and_present(
        &socket_path,
        &commands,
        ServerCommand::CaptureAndCompleteRenderedLegacyPreparedFrame,
    )
    .unwrap();
    let server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(state.presentation_presented_count, 1);
    assert_eq!(state.presentation_discarded_count, 0);
    assert_eq!(
        state
            .presentation_feedback_event_log
            .iter()
            .filter(|(_, outcome)| *outcome == "presented")
            .count(),
        1
    );
    assert_eq!(
        state
            .frame_completion_event_log
            .iter()
            .filter(|event| **event == "buffer_release")
            .count(),
        1
    );
    assert!(!server.has_prepared_frame_batch());
    assert_eq!(server.frame_batch_count(), 0);
}

#[test]
fn one_hundred_legacy_skipped_cycles_do_not_retain_frame_batches() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);

    for _ in 0..100 {
        commands
            .send(ServerCommand::CaptureAndFinishLegacyPreparedFrame)
            .unwrap();
    }
    wait_for_server_commands(&commands);
    let server = stop_controllable_test_server(commands, server_thread);

    assert!(!server.has_prepared_frame_batch());
    assert_eq!(server.frame_batch_count(), 0);
}

#[test]
fn prepared_terminal_settlement_does_not_consume_an_older_submitted_batch() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);

    commands
        .send(ServerCommand::CaptureLegacySubmittedAndPreparedFrames)
        .unwrap();
    commands.send(ServerCommand::FinishPreparedFrame).unwrap();
    wait_for_server_commands(&commands);
    let server = stop_controllable_test_server(commands, server_thread);

    assert!(!server.has_prepared_frame_batch());
    assert!(server.has_submitted_frame_batch());
    assert_eq!(server.frame_batch_count(), 1);
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
fn frame_callback_request_without_commit_is_not_captured_by_unrelated_render() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let (before_commit, after_commit) =
        exercise_uncommitted_frame_callback_ownership(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(!before_commit);
    assert!(after_commit);
}

#[test]
fn frame_callbacks_keep_request_order_within_and_across_commits() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let (expected, completed) =
        exercise_committed_frame_callback_order(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(completed, expected);
}

#[test]
fn uncommitted_frame_callback_is_not_protocol_frame_work() {
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

    assert!(!protocol_only.unwrap());
}

#[test]
fn present_frame_completes_frame_callback_after_its_followup_commit() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_visible_surface_frame_callback_commit_and_present(&socket_path, &commands);
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
fn explicit_frame_batch_does_not_release_a_late_commit() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let result = exercise_explicit_frame_batch_shm_release_late_commit(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let ((buffer_a, buffer_b, _buffer_c), after_frame_a, after_frame_b) = result.unwrap();
    assert_eq!(after_frame_a, vec![buffer_a, buffer_b]);
    assert_eq!(after_frame_b, vec![buffer_a, buffer_b, _buffer_c]);
}

#[test]
fn destroyed_clients_scrub_pending_captured_and_retired_releases() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    exercise_destroyed_client_release_states(&socket_path, &commands).unwrap();
    let server = stop_controllable_test_server(commands, server_thread);
    let metrics = server.buffer_release_metrics();

    assert_eq!(metrics.buffer_releases_completed, 0);
    assert!(metrics.buffer_releases_discarded >= 3);
    assert_eq!(metrics.buffer_release_duplicate_attempts, 0);
}

#[test]
fn wayland_client_dmabuf_release_is_sent_on_matching_present_after_replacement() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_dmabuf_surface_then_replace_buffer(&socket_path, &commands);
    let _server = stop_controllable_test_server(commands, server_thread);

    let state = state.unwrap();
    assert_eq!(state.buffer_release_count, 1);
    assert_eq!(
        state.frame_completion_event_log,
        vec!["frame_callback", "buffer_release"]
    );
}

#[test]
fn unrelated_extra_present_does_not_gate_or_duplicate_dmabuf_release() {
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
        pool: Arc::new(ShmPoolData::new(file, 16)),
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
        pool: Arc::new(ShmPoolData::new(file, 16)),
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
