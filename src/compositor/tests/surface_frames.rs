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
fn compatibility_legacy_double_render_mark_is_idempotent() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_live_surface_with_unpresented_buffer_frame_callback(&socket_path).unwrap();
    wait_for_server_commands(&commands);
    commands
        .send(ServerCommand::CaptureLegacyPreparedFrame)
        .unwrap();
    commands
        .send(ServerCommand::MarkPreparedFrameCallbacksRendered)
        .unwrap();
    commands.send(ServerCommand::FinishPreparedFrame).unwrap();
    wait_for_server_commands(&commands);
    assert!(!capture_pending_frame_callbacks(&commands));

    let server = stop_controllable_test_server(commands, server_thread);
    let metrics = server.frame_callback_metrics();
    assert_eq!(metrics.callbacks_marked_rendered, 1);
    assert_eq!(metrics.callbacks_completed_after_immediate_admission, 1);
    assert_eq!(metrics.callbacks_completed_at_presentation_fallback, 0);
    assert_eq!(metrics.callbacks_completed_after_abandonment, 0);
}

#[test]
fn completed_callback_batch_cannot_regress_on_duplicate_render_mark() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let batch_id = capture_live_frame_batch(&socket_path, &commands, 11);
    commands
        .send(ServerCommand::MarkFrameCallbacksRendered(batch_id))
        .unwrap();
    commands
        .send(ServerCommand::CompleteFrameCallbacksAfterAdmission {
            batch_id,
            admission: FrameCallbackAdmission::Immediate,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    assert!(capture_frame_callback_pacing_is_completed(
        &commands, batch_id
    ));
    let before = capture_frame_callback_metrics(&commands);

    commands
        .send(ServerCommand::MarkFrameCallbacksRendered(batch_id))
        .unwrap();
    wait_for_server_commands(&commands);
    assert!(capture_frame_callback_pacing_is_completed(
        &commands, batch_id
    ));
    let after = capture_frame_callback_metrics(&commands);

    assert_eq!(after.callbacks_marked_rendered, 1);
    assert_eq!(
        after.callbacks_marked_rendered,
        before.callbacks_marked_rendered
    );
    assert_eq!(
        after.last_callback_render_completed_ns,
        before.last_callback_render_completed_ns
    );
    assert_eq!(
        after.last_callback_commit_to_render_ns,
        before.last_callback_commit_to_render_ns
    );
    assert_eq!(after.callbacks_completed_after_immediate_admission, 1);

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn duplicate_render_mark_before_admission_is_idempotent() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let batch_id = capture_live_frame_batch(&socket_path, &commands, 12);
    commands
        .send(ServerCommand::MarkFrameCallbacksRendered(batch_id))
        .unwrap();
    wait_for_server_commands(&commands);
    let before = capture_frame_callback_metrics(&commands);
    commands
        .send(ServerCommand::MarkFrameCallbacksRendered(batch_id))
        .unwrap();
    wait_for_server_commands(&commands);
    let after = capture_frame_callback_metrics(&commands);

    assert!(capture_pending_frame_callbacks(&commands));
    assert_eq!(after.callbacks_marked_rendered, 1);
    assert_eq!(
        after.callbacks_marked_rendered,
        before.callbacks_marked_rendered
    );
    assert_eq!(
        after.last_callback_render_completed_ns,
        before.last_callback_render_completed_ns
    );
    assert_eq!(
        after.last_callback_commit_to_render_ns,
        before.last_callback_commit_to_render_ns
    );

    commands
        .send(ServerCommand::CompleteFrameCallbacksAfterAdmission {
            batch_id,
            admission: FrameCallbackAdmission::Ready,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    let server = stop_controllable_test_server(commands, server_thread);
    assert_eq!(
        server
            .frame_callback_metrics()
            .callbacks_completed_after_ready_admission,
        1
    );
}

#[test]
fn empty_completed_callback_batch_remains_terminal_after_render_mark() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let batch_id = capture_live_frame_batch(&socket_path, &commands, 13);
    commands
        .send(ServerCommand::MarkFrameCallbacksRendered(batch_id))
        .unwrap();
    commands
        .send(ServerCommand::CompleteFrameCallbacksAfterAdmission {
            batch_id,
            admission: FrameCallbackAdmission::Immediate,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    assert!(capture_frame_callback_pacing_is_completed(
        &commands, batch_id
    ));

    commands
        .send(ServerCommand::MarkFrameCallbacksRendered(batch_id))
        .unwrap();
    wait_for_server_commands(&commands);
    assert!(capture_frame_callback_pacing_is_completed(
        &commands, batch_id
    ));

    let server = stop_controllable_test_server(commands, server_thread);
    assert_eq!(server.frame_callback_metrics().callbacks_marked_rendered, 1);
}

#[test]
fn render_ahead_ready_callback_stays_owned_until_admission() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let batch_id = capture_live_frame_batch(&socket_path, &commands, 1);
    commands
        .send(ServerCommand::MarkFrameCallbacksRendered(batch_id))
        .unwrap();
    commands
        .send(ServerCommand::NoteFrameCallbacksDeferredReady(batch_id))
        .unwrap();
    wait_for_server_commands(&commands);
    assert!(capture_pending_frame_callbacks(&commands));
    let metrics = capture_frame_callback_metrics(&commands);
    assert_eq!(metrics.callbacks_marked_rendered, 1);
    assert_eq!(metrics.callbacks_deferred_ready, 1);

    commands
        .send(ServerCommand::CompleteFrameCallbacksAfterAdmission {
            batch_id,
            admission: FrameCallbackAdmission::Ready,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    assert!(!capture_pending_frame_callbacks(&commands));
    let metrics = capture_frame_callback_metrics(&commands);
    assert_eq!(metrics.callbacks_completed_after_ready_admission, 1);

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn accepted_immediate_admission_completes_callback_before_pageflip() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let batch_id = capture_live_frame_batch(&socket_path, &commands, 2);
    commands
        .send(ServerCommand::MarkFrameCallbacksRendered(batch_id))
        .unwrap();
    wait_for_server_commands(&commands);
    assert!(capture_pending_frame_callbacks(&commands));
    commands
        .send(ServerCommand::CompleteFrameCallbacksAfterAdmission {
            batch_id,
            admission: FrameCallbackAdmission::Immediate,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    assert!(!capture_pending_frame_callbacks(&commands));
    let metrics = capture_frame_callback_metrics(&commands);
    assert_eq!(metrics.callbacks_completed_after_immediate_admission, 1);

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn only_the_exact_ready_batch_completes_at_ready_admission() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let first = capture_live_frame_batch(&socket_path, &commands, 3);
    let second = capture_live_frame_batch(&socket_path, &commands, 4);
    assert_ne!(first, second);
    commands
        .send(ServerCommand::MarkFrameCallbacksRendered(first))
        .unwrap();
    commands
        .send(ServerCommand::MarkFrameCallbacksRendered(second))
        .unwrap();
    wait_for_server_commands(&commands);

    commands
        .send(ServerCommand::CompleteFrameCallbacksAfterAdmission {
            batch_id: second,
            admission: FrameCallbackAdmission::Ready,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    assert!(capture_pending_frame_callbacks(&commands));

    commands
        .send(ServerCommand::CompleteFrameCallbacksAfterAdmission {
            batch_id: first,
            admission: FrameCallbackAdmission::Ready,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    assert!(!capture_pending_frame_callbacks(&commands));

    let metrics = capture_frame_callback_metrics(&commands);
    assert_eq!(metrics.callbacks_completed_after_ready_admission, 2);
    assert_eq!(metrics.callbacks_completed_after_immediate_admission, 0);

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn failed_admission_retains_callback_for_a_later_retry() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let batch_id = capture_live_frame_batch(&socket_path, &commands, 5);
    commands
        .send(ServerCommand::MarkFrameCallbacksRendered(batch_id))
        .unwrap();
    commands
        .send(ServerCommand::NoteFrameCallbackAdmissionFailure(batch_id))
        .unwrap();
    wait_for_server_commands(&commands);
    assert!(capture_pending_frame_callbacks(&commands));
    let metrics = capture_frame_callback_metrics(&commands);
    assert_eq!(metrics.callbacks_completed_after_immediate_admission, 0);
    assert_eq!(metrics.callbacks_completed_after_ready_admission, 0);
    assert_eq!(metrics.callbacks_retained_after_failed_admission, 1);

    commands
        .send(ServerCommand::CompleteFrameCallbacksAfterAdmission {
            batch_id,
            admission: FrameCallbackAdmission::Immediate,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    assert!(!capture_pending_frame_callbacks(&commands));

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn render_completed_callback_is_resolved_at_direct_pageflip() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let batch_id = capture_live_frame_batch(&socket_path, &commands, 6);
    commands
        .send(ServerCommand::MarkFrameCallbacksRendered(batch_id))
        .unwrap();
    wait_for_server_commands(&commands);
    commands
        .send(ServerCommand::CompleteDirectFrameBatch {
            frame_id: 6,
            batch_id,
            direct_surface_id: 1,
            presentation: FramePresentation::synchronized_zero_copy(
                PresentationClock::Monotonic,
                1,
                0,
                1,
            )
            .unwrap(),
        })
        .unwrap();
    wait_for_server_commands(&commands);
    let server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(
        server.frame_callback_metrics().callbacks_found_at_pageflip,
        1
    );
    assert_eq!(
        server
            .frame_callback_metrics()
            .callbacks_completed_at_presentation_fallback,
        1
    );
}

#[test]
fn presentation_fallback_sends_deferred_callback_exactly_once() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let batch_id = capture_live_frame_batch(&socket_path, &commands, 7);
    commands
        .send(ServerCommand::MarkFrameCallbacksRendered(batch_id))
        .unwrap();
    wait_for_server_commands(&commands);
    commands
        .send(ServerCommand::CompleteFrameBatchNow {
            frame_id: 7,
            batch_id,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    assert!(!capture_pending_frame_callbacks(&commands));

    let server = stop_controllable_test_server(commands, server_thread);
    let metrics = server.frame_callback_metrics();
    assert_eq!(metrics.callbacks_completed_at_presentation_fallback, 1);
    assert_eq!(metrics.callbacks_completed_after_immediate_admission, 0);
    assert_eq!(metrics.callbacks_completed_after_ready_admission, 0);
}

#[test]
fn invalidated_ready_batch_cancels_callback_without_admission() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let batch_id = capture_live_frame_batch(&socket_path, &commands, 8);
    commands
        .send(ServerCommand::MarkFrameCallbacksRendered(batch_id))
        .unwrap();
    commands
        .send(ServerCommand::DiscardFrameBatch {
            batch_id,
            reason: FrameBatchDiscardReason::OutputDestroyed,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    assert!(!capture_pending_frame_callbacks(&commands));

    let server = stop_controllable_test_server(commands, server_thread);
    let metrics = server.frame_callback_metrics();
    assert_eq!(metrics.callbacks_completed_after_ready_admission, 0);
    assert_eq!(metrics.callbacks_completed_after_immediate_admission, 0);
    assert_eq!(metrics.callbacks_completed_after_abandonment, 1);
    assert_eq!(metrics.callbacks_in_discarded_rendered_batches, 1);
}

#[test]
fn render_failure_requeues_callback_for_one_later_admission() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let failed_batch = capture_live_frame_batch(&socket_path, &commands, 9);
    commands
        .send(ServerCommand::MarkFrameCallbacksRendered(failed_batch))
        .unwrap();
    commands
        .send(ServerCommand::RestoreFrameBatchAfterRenderFailure(
            failed_batch,
        ))
        .unwrap();
    wait_for_server_commands(&commands);
    assert!(capture_pending_frame_callbacks(&commands));
    let metrics = capture_frame_callback_metrics(&commands);
    assert_eq!(metrics.callbacks_completed_after_abandonment, 0);
    assert_eq!(metrics.callbacks_in_discarded_rendered_batches, 0);

    let (batch_reply, batch_receiver) = std::sync::mpsc::channel();
    commands
        .send(ServerCommand::CaptureFrameBatch {
            frame_id: 10,
            reply: batch_reply,
        })
        .unwrap();
    let retry_batch = batch_receiver.recv().unwrap();
    commands
        .send(ServerCommand::MarkFrameCallbacksRendered(retry_batch))
        .unwrap();
    commands
        .send(ServerCommand::CompleteFrameCallbacksAfterAdmission {
            batch_id: retry_batch,
            admission: FrameCallbackAdmission::Immediate,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    assert!(!capture_pending_frame_callbacks(&commands));

    let server = stop_controllable_test_server(commands, server_thread);
    let metrics = server.frame_callback_metrics();
    assert_eq!(metrics.callbacks_completed_after_immediate_admission, 1);
    assert_eq!(metrics.callbacks_completed_after_ready_admission, 0);
    assert_eq!(metrics.callbacks_completed_after_abandonment, 0);
}

#[test]
fn chromium_like_o1_virtual_oracle_sustains_one_refresh_callbacks_with_two_buffers() {
    const REFRESH_NS: u64 = 6_060_606;
    const CLIENT_REACTION_NS: u64 = 500_000;
    const FRAMES: usize = 180;

    #[derive(Debug, Clone, Copy)]
    struct VirtualFrame {
        buffer: usize,
    }

    let mut available = [true, true];
    let mut next_buffer = 0usize;
    let mut ready = std::collections::VecDeque::new();
    let mut callback_times = Vec::new();
    let mut render_ahead_successes = 0usize;
    let mut now_ns = 0u64;

    for _frame_number in 0..FRAMES {
        let buffer = (0..available.len())
            .map(|offset| (next_buffer + offset) % available.len())
            .find(|buffer| available[*buffer])
            .expect("two-buffer client must have a released buffer");
        available[buffer] = false;
        next_buffer = (buffer + 1) % available.len();
        let mut frame = VirtualFrame { buffer };

        // The client buffer is released at materialization, independently of
        // whether this output frame is still READY or already admitted.
        available[buffer] = true;
        ready.push_back(frame);

        // O1 may keep one frame in the output lane while rendering another
        // future frame.  The callback belongs to the exact ready frame and is
        // sent only when that frame crosses admission.
        if ready.len() > 1 {
            let admitted = ready.pop_front().expect("ready frame exists");
            frame = admitted;
            callback_times.push(now_ns);
            render_ahead_successes += 1;
            available[frame.buffer] = true;
            assert!(now_ns.saturating_add(CLIENT_REACTION_NS) <= now_ns + REFRESH_NS);
        }

        now_ns = now_ns.saturating_add(REFRESH_NS);
    }

    while let Some(frame) = ready.pop_front() {
        callback_times.push(now_ns);
        available[frame.buffer] = true;
        assert!(now_ns.saturating_add(CLIENT_REACTION_NS) <= now_ns + REFRESH_NS);
        now_ns = now_ns.saturating_add(REFRESH_NS);
    }

    let callback_intervals = callback_times
        .windows(2)
        .map(|pair| pair[1].saturating_sub(pair[0]))
        .collect::<Vec<_>>();
    assert!(render_ahead_successes > 0);
    assert_eq!(callback_times.len(), FRAMES);
    assert!(
        callback_intervals
            .iter()
            .all(|interval| *interval == REFRESH_NS)
    );
    assert!(available.into_iter().all(|released| released));
}

#[test]
fn retryable_callback_transfer_preserves_live_callback_ownership() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let batch_id = capture_live_frame_batch(&socket_path, &commands, 2);

    let (reply, receiver) = std::sync::mpsc::channel();
    commands
        .send(ServerCommand::PrepareTerminalCallbackOwnership {
            batch_id,
            disposition: TerminalCallbackDisposition::Retryable,
            reply,
        })
        .unwrap();
    assert_eq!(
        receiver.recv().unwrap(),
        TerminalCallbackOwnership::Transferred {
            owner: batch_id,
            callbacks: 1,
        }
    );
    commands
        .send(ServerCommand::RestoreFrameBatchAfterRenderFailure(batch_id))
        .unwrap();
    wait_for_server_commands(&commands);
    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn no_visual_change_resolves_live_callback_without_feedback() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let batch_id = capture_live_frame_batch(&socket_path, &commands, 3);

    let (reply, receiver) = std::sync::mpsc::channel();
    commands
        .send(ServerCommand::PrepareTerminalCallbackOwnership {
            batch_id,
            disposition: TerminalCallbackDisposition::NoVisualChange,
            reply,
        })
        .unwrap();
    assert_eq!(
        receiver.recv().unwrap(),
        TerminalCallbackOwnership::Resolved { completed: 1 }
    );
    commands
        .send(ServerCommand::CompleteNoVisualChangeFrameBatch(batch_id))
        .unwrap();
    wait_for_server_commands(&commands);
    let server = stop_controllable_test_server(commands, server_thread);
    assert_eq!(
        server
            .frame_callback_metrics()
            .callbacks_completed_after_immediate_admission,
        0
    );
}

#[test]
fn safe_abandonment_cancels_live_callback_without_leak() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let batch_id = capture_live_frame_batch(&socket_path, &commands, 4);

    let (reply, receiver) = std::sync::mpsc::channel();
    commands
        .send(ServerCommand::PrepareTerminalCallbackOwnership {
            batch_id,
            disposition: TerminalCallbackDisposition::Cancelled,
            reply,
        })
        .unwrap();
    assert_eq!(
        receiver.recv().unwrap(),
        TerminalCallbackOwnership::Cancelled { callbacks: 1 }
    );
    commands
        .send(ServerCommand::DiscardFrameBatch {
            batch_id,
            reason: FrameBatchDiscardReason::OutputDestroyed,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    let _server = stop_controllable_test_server(commands, server_thread);
}

fn capture_live_frame_batch(
    socket_path: &PathBuf,
    commands: &std::sync::mpsc::Sender<ServerCommand>,
    frame_id: u64,
) -> CompositorFrameBatchId {
    create_live_surface_with_unpresented_buffer_frame_callback(socket_path).unwrap();
    wait_for_server_commands(commands);
    let (batch_reply, batch_receiver) = std::sync::mpsc::channel();
    commands
        .send(ServerCommand::CaptureFrameBatch {
            frame_id,
            reply: batch_reply,
        })
        .unwrap();
    batch_receiver.recv().unwrap()
}

fn capture_frame_callback_pacing_is_completed(
    commands: &std::sync::mpsc::Sender<ServerCommand>,
    batch_id: CompositorFrameBatchId,
) -> bool {
    let (reply, receiver) = std::sync::mpsc::channel();
    commands
        .send(ServerCommand::CaptureFrameCallbackPacingCompleted { batch_id, reply })
        .unwrap();
    receiver.recv_timeout(Duration::from_secs(1)).unwrap()
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
fn compatibility_no_visual_change_without_protocol_work_keeps_batch_unowned() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let (reply, receiver) = std::sync::mpsc::channel();
    commands
        .send(ServerCommand::SettleNoVisualChangeWork {
            owns_frame_batch: false,
            reply,
        })
        .unwrap();
    assert!(!receiver.recv_timeout(Duration::from_secs(1)).unwrap());

    let (prepared_reply, prepared_receiver) = std::sync::mpsc::channel();
    commands
        .send(ServerCommand::CapturePreparedFrame(prepared_reply))
        .unwrap();
    assert!(
        !prepared_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
    );

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn compatibility_no_visual_change_with_callback_owns_one_terminal_batch() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_live_surface_with_unpresented_buffer_frame_callback(&socket_path).unwrap();
    wait_for_server_commands(&commands);

    let (reply, receiver) = std::sync::mpsc::channel();
    commands
        .send(ServerCommand::SettleNoVisualChangeWork {
            owns_frame_batch: true,
            reply,
        })
        .unwrap();
    assert!(receiver.recv_timeout(Duration::from_secs(1)).unwrap());
    assert!(!capture_pending_frame_work(&commands));

    let (prepared_reply, prepared_receiver) = std::sync::mpsc::channel();
    commands
        .send(ServerCommand::CapturePreparedFrame(prepared_reply))
        .unwrap();
    assert!(
        !prepared_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
    );

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn compatibility_no_visual_change_with_presentation_feedback_owns_one_terminal_batch() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = create_surface_with_unpresented_presentation_feedback(&socket_path).unwrap();
    retain_live_test_connection(connection);
    wait_for_server_commands(&commands);

    let (reply, receiver) = std::sync::mpsc::channel();
    commands
        .send(ServerCommand::SettleNoVisualChangeWork {
            owns_frame_batch: true,
            reply,
        })
        .unwrap();
    assert!(receiver.recv_timeout(Duration::from_secs(1)).unwrap());
    assert!(!capture_pending_frame_work(&commands));

    let (prepared_reply, prepared_receiver) = std::sync::mpsc::channel();
    commands
        .send(ServerCommand::CapturePreparedFrame(prepared_reply))
        .unwrap();
    assert!(
        !prepared_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
    );

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
    assert_eq!(metrics.callbacks_completed_after_immediate_admission, 1);
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
    assert_eq!(
        after_second_settlement.callbacks_completed_after_immediate_admission,
        1
    );
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
        0
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
fn wayland_client_shm_release_happens_after_materialization_before_present() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_surface_with_buffer_release(&socket_path, &commands);
    let server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(state.unwrap().buffer_release_count, 1);
    let metrics = server.shm_buffer_lifetime_metrics();
    assert!(metrics.shm_materializations_total >= 1);
    assert!(metrics.shm_releases_after_materialization_total >= 1);
    assert_eq!(metrics.presentation_bound_shm_release_total, 0);
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
    assert_eq!(after_frame_b, vec![buffer_a, buffer_b]);
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

    assert!(metrics.buffer_releases_completed >= 3);
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
    assert_eq!(surface.generation, server.render_generation());
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
fn wayland_surface_damage_only_commit_keeps_owned_shm_snapshot() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    create_client_toplevel_with_shm_damage_only_update(&socket_path).unwrap();
    let server = stop_test_server(running, server_thread);

    assert_eq!(server.renderable_surfaces().len(), 1);
    let surface = &server.renderable_surfaces()[0];
    assert_eq!(surface.generation, server.render_generation());
    assert_eq!(
        server.render_generation_cause(),
        RenderGenerationCause::SurfaceDamage
    );
    assert_eq!(
        surface.cpu_pixels(),
        Some(vec![0xff11_1111, 0xff22_2222, 0xff33_3333, 0xff44_4444].as_slice())
    );
}

#[test]
fn wayland_bufferless_transform_commit_updates_retained_mapping_without_damage() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let buffer = TestShmBuffer::new(&shm, &qh, 2, 3).unwrap();

    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    buffer.attach(&surface, 2, 3);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    let initial = capture_renderable_surface_snapshot(&commands);

    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    let no_op = capture_renderable_surface_snapshot(&commands);

    let transforms = [
        (
            wayland_client::protocol::wl_output::Transform::Normal,
            wayland_server::protocol::wl_output::Transform::Normal,
            (2, 3),
        ),
        (
            wayland_client::protocol::wl_output::Transform::_90,
            wayland_server::protocol::wl_output::Transform::_90,
            (3, 2),
        ),
        (
            wayland_client::protocol::wl_output::Transform::_180,
            wayland_server::protocol::wl_output::Transform::_180,
            (2, 3),
        ),
        (
            wayland_client::protocol::wl_output::Transform::_270,
            wayland_server::protocol::wl_output::Transform::_270,
            (3, 2),
        ),
        (
            wayland_client::protocol::wl_output::Transform::Flipped,
            wayland_server::protocol::wl_output::Transform::Flipped,
            (2, 3),
        ),
        (
            wayland_client::protocol::wl_output::Transform::Flipped90,
            wayland_server::protocol::wl_output::Transform::Flipped90,
            (3, 2),
        ),
        (
            wayland_client::protocol::wl_output::Transform::Flipped180,
            wayland_server::protocol::wl_output::Transform::Flipped180,
            (2, 3),
        ),
        (
            wayland_client::protocol::wl_output::Transform::Flipped270,
            wayland_server::protocol::wl_output::Transform::Flipped270,
            (3, 2),
        ),
    ];
    let mut previous = no_op[0].clone();
    for (client_transform, server_transform, (width, height)) in transforms {
        surface.set_buffer_transform(client_transform);
        surface.commit();
        connection.flush().unwrap();
        queue.roundtrip(&mut RegistryTestState::default()).unwrap();
        let updated = capture_renderable_surface_snapshot(&commands);
        assert_eq!(updated.len(), 1);
        assert_eq!(updated[0].buffer_id, initial[0].buffer_id);
        assert_eq!(updated[0].pixel_checksum, initial[0].pixel_checksum);
        assert_eq!(updated[0].buffer_transform, server_transform);
        assert_eq!((updated[0].width, updated[0].height), (width, height));
        if server_transform != wayland_server::protocol::wl_output::Transform::Normal {
            assert!(updated[0].generation > previous.generation);
        }
        previous = updated[0].clone();
    }

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();

    assert_eq!(no_op[0].generation, initial[0].generation);
}

#[test]
fn wayland_bufferless_mapping_commit_publishes_viewport_resets_and_scale() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let viewporter: client_wp_viewporter::WpViewporter = globals.bind(&qh, 1..=1, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let viewport = viewporter.get_viewport(&surface, &qh, ());
    let buffer = TestShmBuffer::new(&shm, &qh, 4, 2).unwrap();

    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();

    viewport.set_source(0.0, 0.0, 2.0, 2.0);
    viewport.set_destination(2, 2);
    buffer.attach(&surface, 4, 2);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    let initial = capture_renderable_surface_snapshot(&commands);

    viewport.set_source(1.0, 0.0, 2.0, 2.0);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    let source_changed = capture_renderable_surface_snapshot(&commands);

    viewport.set_destination(3, 4);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    let destination_changed = capture_renderable_surface_snapshot(&commands);

    viewport.set_source(-1.0, -1.0, -1.0, -1.0);
    viewport.set_destination(-1, -1);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    let reset = capture_renderable_surface_snapshot(&commands);

    surface.set_buffer_scale(2);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    let scaled = capture_renderable_surface_snapshot(&commands);

    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    let after_no_op = capture_renderable_surface_snapshot(&commands);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();

    assert_eq!(initial.len(), 1);
    assert_eq!(source_changed.len(), 1);
    assert_eq!(destination_changed.len(), 1);
    assert_eq!(reset.len(), 1);
    assert_eq!(scaled.len(), 1);
    let buffer_id = initial[0].buffer_id;
    assert!(source_changed[0].buffer_id == buffer_id);
    assert_eq!(source_changed[0].pixel_checksum, initial[0].pixel_checksum);
    assert_eq!(
        destination_changed[0].pixel_checksum,
        initial[0].pixel_checksum
    );
    assert_eq!(reset[0].pixel_checksum, initial[0].pixel_checksum);
    assert_eq!(scaled[0].pixel_checksum, initial[0].pixel_checksum);
    assert_eq!(initial[0].viewport_source, Some((0, 0, 512, 512)));
    assert_eq!(source_changed[0].viewport_source, Some((256, 0, 512, 512)));
    assert_eq!((initial[0].width, initial[0].height), (2, 2));
    assert_eq!((source_changed[0].width, source_changed[0].height), (2, 2));
    assert_eq!(destination_changed[0].viewport_destination, Some((3, 4)));
    assert_eq!(reset[0].viewport_source, None);
    assert_eq!(reset[0].viewport_destination, None);
    assert_eq!((reset[0].width, reset[0].height), (4, 2));
    assert_eq!(scaled[0].buffer_scale, 2);
    assert_eq!((scaled[0].width, scaled[0].height), (2, 1));
    assert_eq!(after_no_op[0].generation, scaled[0].generation);
    assert!(source_changed[0].generation > initial[0].generation);
    assert!(destination_changed[0].generation > source_changed[0].generation);
    assert!(reset[0].generation > destination_changed[0].generation);
    assert!(scaled[0].generation > reset[0].generation);
}

#[test]
fn wayland_damage_request_order_is_independent_of_mapping_updates() {
    let run = |kind: u8, mapping_first: bool| {
        let socket_name = unique_socket_name();
        let server = OwnCompositorServer::bind(&socket_name).unwrap();
        let socket_path = runtime_socket_path(&socket_name);
        let (commands, server_thread) = spawn_controllable_test_server(server);
        let stream = UnixStream::connect(&socket_path).unwrap();
        let connection = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
        let qh = queue.handle();
        let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
        let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
        let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
        let viewporter: client_wp_viewporter::WpViewporter = globals.bind(&qh, 1..=1, ()).unwrap();
        let surface = compositor.create_surface(&qh, ());
        let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
        let _toplevel = xdg_surface.get_toplevel(&qh, ());
        let viewport = viewporter.get_viewport(&surface, &qh, ());
        let (buffer_width, buffer_height) = match kind {
            0 => (3, 2),
            1 | 2 => (4, 2),
            _ => panic!("unknown mapping test kind"),
        };
        let buffer = TestShmBuffer::new(&shm, &qh, buffer_width, buffer_height).unwrap();

        surface.commit();
        connection.flush().unwrap();
        queue.roundtrip(&mut RegistryTestState::default()).unwrap();

        let apply_mapping = || match kind {
            0 => surface.set_buffer_transform(wayland_client::protocol::wl_output::Transform::_90),
            1 => surface.set_buffer_scale(2),
            2 => {
                viewport.set_source(1.0, 0.0, 2.0, 2.0);
                viewport.set_destination(2, 2);
            }
            _ => unreachable!(),
        };
        let apply_damage = || surface.damage(0, 0, 1, 1);
        apply_mapping();
        buffer.attach(&surface, buffer_width, buffer_height);
        surface.commit();
        connection.flush().unwrap();
        queue.roundtrip(&mut RegistryTestState::default()).unwrap();
        commands
            .send(ServerCommand::MarkRenderDamagePresented)
            .unwrap();
        wait_for_server_commands(&commands);

        buffer.attach_without_damage(&surface);
        if mapping_first {
            apply_mapping();
            apply_damage();
        } else {
            apply_damage();
            apply_mapping();
        }
        surface.commit();
        connection.flush().unwrap();
        queue.roundtrip(&mut RegistryTestState::default()).unwrap();

        let server = stop_controllable_test_server(commands, server_thread);
        server.renderable_surfaces()[0].damage.clone()
    };

    for kind in 0..=2 {
        let expected = RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
            x: match kind {
                0 => 0,
                1 => 0,
                2 => 1,
                _ => unreachable!(),
            },
            y: match kind {
                0 => 1,
                1 | 2 => 0,
                _ => unreachable!(),
            },
            width: match kind {
                0 => 1,
                1 => 2,
                2 => 1,
                _ => unreachable!(),
            },
            height: match kind {
                0 => 1,
                1 => 2,
                2 => 1,
                _ => unreachable!(),
            },
        }]);
        assert_eq!(run(kind, false), expected, "mapping kind {kind}");
        assert_eq!(run(kind, true), expected, "mapping kind {kind}");
    }
}

#[test]
fn wayland_bufferless_viewport_destroy_resets_retained_mapping() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let viewporter: client_wp_viewporter::WpViewporter = globals.bind(&qh, 1..=1, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let viewport = viewporter.get_viewport(&surface, &qh, ());
    let buffer = TestShmBuffer::new(&shm, &qh, 4, 2).unwrap();

    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();

    viewport.set_source(0.0, 0.0, 2.0, 2.0);
    viewport.set_destination(2, 2);
    buffer.attach(&surface, 4, 2);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    let initial = capture_renderable_surface_snapshot(&commands);
    let surface_id = initial[0].surface_id;
    let initial_ownership = capture_surface_buffer_ownership(&commands, surface_id);

    viewport.destroy();
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    let reset = capture_renderable_surface_snapshot(&commands);
    let reset_ownership = capture_surface_buffer_ownership(&commands, surface_id);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();

    assert_eq!(initial.len(), 1);
    assert_eq!(reset.len(), 1);
    assert_eq!(reset[0].buffer_id, initial[0].buffer_id);
    assert_eq!(reset[0].pixel_checksum, initial[0].pixel_checksum);
    assert_eq!(reset[0].viewport_source, None);
    assert_eq!(reset[0].viewport_destination, None);
    assert_eq!((reset[0].width, reset[0].height), (4, 2));
    assert!(reset[0].generation > initial[0].generation);
    assert!(initial_ownership.current_surface_buffer);
    assert!(reset_ownership.current_surface_buffer);
    assert_eq!(reset_ownership.pending_dmabuf_releases, 0);
}

#[test]
fn wayland_bufferless_window_geometry_none_is_not_a_delta() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let buffer = TestShmBuffer::new(&shm, &qh, 4, 2).unwrap();

    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();

    xdg_surface.set_window_geometry(1, 2, 3, 1);
    buffer.attach(&surface, 4, 2);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    let initial = capture_renderable_surface_snapshot(&commands);
    let initial_generation = initial[0].generation;
    let initial_metrics = capture_core_compliance_metrics(&commands);
    let initial_geometry = capture_committed_window_geometry(&commands);

    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    let no_op = capture_renderable_surface_snapshot(&commands);
    let no_op_metrics = capture_core_compliance_metrics(&commands);
    let no_op_geometry = capture_committed_window_geometry(&commands);

    xdg_surface.set_window_geometry(2, 3, 2, 2);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    let changed = capture_renderable_surface_snapshot(&commands);
    let changed_metrics = capture_core_compliance_metrics(&commands);
    let changed_geometry = capture_committed_window_geometry(&commands);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();

    assert_eq!(initial.len(), 1);
    assert_eq!(no_op.len(), 1);
    assert_eq!(changed.len(), 1);
    assert_eq!(no_op[0].generation, initial_generation);
    assert_eq!(no_op[0].buffer_id, initial[0].buffer_id);
    assert_eq!(no_op[0].pixel_checksum, initial[0].pixel_checksum);
    assert_eq!(
        no_op_metrics.surface_commit_mapping_full_promotions,
        initial_metrics.surface_commit_mapping_full_promotions
    );
    assert_eq!(no_op_geometry, initial_geometry);
    assert!(changed[0].generation > no_op[0].generation);
    assert_eq!(changed[0].buffer_id, initial[0].buffer_id);
    assert_eq!(changed[0].pixel_checksum, initial[0].pixel_checksum);
    assert_eq!(changed_geometry, Some(XdgWindowGeometry::new(2, 3, 2, 2)));
    assert_eq!(
        changed_metrics.surface_commit_mapping_full_promotions,
        no_op_metrics.surface_commit_mapping_full_promotions + 1
    );
}

#[test]
fn wayland_bufferless_mapping_refreshes_pointer_hit_cache_without_pointer_motion() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let viewporter: client_wp_viewporter::WpViewporter = globals.bind(&qh, 1..=1, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let viewport = viewporter.get_viewport(&surface, &qh, ());
    let buffer = TestShmBuffer::new(&shm, &qh, 100, 80).unwrap();

    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    viewport.set_destination(100, 80);
    buffer.attach(&surface, 100, 80);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();

    let initial = capture_renderable_surface_snapshot(&commands);
    let point_x = f64::from(initial[0].origin_x) + 40.0;
    let point_y = f64::from(initial[0].origin_y) + 30.0;
    commands
        .send(ServerCommand::PointerMotion {
            x: point_x,
            y: point_y,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    let primed = capture_pointer_scene_hit(&commands, point_x, point_y);
    let generation_before = capture_pointer_hit_generation(&commands);
    let pointer_metrics_before = capture_pointer_input_metrics(&commands);
    let last_position_before = capture_last_pointer_position(&commands);

    surface.offset(20, 10);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    let updated = capture_renderable_surface_snapshot(&commands);
    let hit_after_offset = capture_pointer_scene_hit(&commands, point_x, point_y);
    let generation_after_offset = capture_pointer_hit_generation(&commands);
    let pointer_metrics_after_offset = capture_pointer_input_metrics(&commands);
    let last_position_after = capture_last_pointer_position(&commands);

    viewport.set_source(10.0, 0.0, 80.0, 60.0);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    let generation_after_source_crop = capture_pointer_hit_generation(&commands);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();

    assert_eq!(primed.0, Some(initial[0].surface_id));
    assert_eq!(primed.1, Some((40.0, 30.0)));
    assert_eq!(hit_after_offset.0, Some(updated[0].surface_id));
    assert_eq!(hit_after_offset.1, Some((20.0, 20.0)));
    assert_ne!(generation_after_offset, generation_before);
    assert!(
        pointer_metrics_after_offset.pointer_hit_generation_invalidations
            > pointer_metrics_before.pointer_hit_generation_invalidations
    );
    assert_eq!(last_position_after, last_position_before);
    assert_eq!(generation_after_source_crop, generation_after_offset);
}

#[test]
fn wayland_bufferless_window_geometry_refreshes_pointer_hit_cache_after_derived_origin_change() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let _pointer = seat.get_pointer(&qh, ());
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let buffer = TestShmBuffer::new(&shm, &qh, 100, 80).unwrap();

    let mut state = RegistryTestState::default();
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    xdg_surface.set_window_geometry(0, 0, 100, 80);
    buffer.attach(&surface, 100, 80);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let initial = capture_renderable_surface_snapshot(&commands);
    let surface_id = initial[0].surface_id;
    let point_x = f64::from(initial[0].origin_x) + 40.0;
    let point_y = f64::from(initial[0].origin_y) + 30.0;
    commands
        .send(ServerCommand::PointerMotion {
            x: point_x,
            y: point_y,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let primed = capture_pointer_scene_hit(&commands, point_x, point_y);
    let generation_before = capture_pointer_hit_generation(&commands);
    let pointer_metrics_before = capture_pointer_input_metrics(&commands);
    let last_position_before = capture_last_pointer_position(&commands);
    let pointer_focus_before = capture_pointer_focus_surface_id(&commands);

    xdg_surface.set_window_geometry(10, 0, 100, 80);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let updated = capture_renderable_surface_snapshot(&commands);
    let pointer_metrics_after_publication = capture_pointer_input_metrics(&commands);
    let hit_after_geometry = capture_pointer_scene_hit(&commands, point_x, point_y);
    let generation_after = capture_pointer_hit_generation(&commands);
    let pointer_metrics_after = capture_pointer_input_metrics(&commands);
    let last_position_after = capture_last_pointer_position(&commands);
    let pointer_focus_after = capture_pointer_focus_surface_id(&commands);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();

    assert_eq!(primed, (Some(surface_id), Some((40.0, 30.0))));
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0].surface_id, surface_id);
    assert_eq!(updated[0].origin_x, initial[0].origin_x - 10);
    assert_eq!(updated[0].origin_y, initial[0].origin_y);
    assert_eq!(updated[0].buffer_id, initial[0].buffer_id);
    assert_eq!(updated[0].pixel_checksum, initial[0].pixel_checksum);
    assert_eq!(hit_after_geometry, (Some(surface_id), Some((50.0, 30.0))));
    assert_ne!(generation_after, generation_before);
    assert!(
        pointer_metrics_after_publication.full_scene_hit_scans
            > pointer_metrics_before.full_scene_hit_scans
    );
    assert!(
        pointer_metrics_after.pointer_hit_generation_invalidations
            > pointer_metrics_before.pointer_hit_generation_invalidations
    );
    assert_eq!(
        pointer_metrics_after.full_scene_hit_scans,
        pointer_metrics_after_publication.full_scene_hit_scans
    );
    assert_eq!(last_position_after, last_position_before);
    assert_eq!(pointer_focus_after, pointer_focus_before);
}

#[test]
fn wayland_buffer_window_geometry_refreshes_pointer_hit_cache_after_derived_origin_change() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let _pointer = seat.get_pointer(&qh, ());
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let initial_buffer = TestShmBuffer::new(&shm, &qh, 100, 80).unwrap();

    let mut state = RegistryTestState::default();
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    xdg_surface.set_window_geometry(0, 0, 100, 80);
    initial_buffer.attach(&surface, 100, 80);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let initial = capture_renderable_surface_snapshot(&commands);
    let surface_id = initial[0].surface_id;
    let point_x = f64::from(initial[0].origin_x) + 40.0;
    let point_y = f64::from(initial[0].origin_y) + 30.0;
    commands
        .send(ServerCommand::PointerMotion {
            x: point_x,
            y: point_y,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let primed = capture_pointer_scene_hit(&commands, point_x, point_y);
    let generation_before = capture_pointer_hit_generation(&commands);
    let pointer_metrics_before = capture_pointer_input_metrics(&commands);
    let last_position_before = capture_last_pointer_position(&commands);
    let pointer_focus_before = capture_pointer_focus_surface_id(&commands);

    xdg_surface.set_window_geometry(10, 0, 100, 80);
    let geometry_buffer = TestShmBuffer::new(&shm, &qh, 100, 80).unwrap();
    geometry_buffer.attach(&surface, 100, 80);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let updated = capture_renderable_surface_snapshot(&commands);
    let pointer_metrics_after_publication = capture_pointer_input_metrics(&commands);
    let hit_after_geometry = capture_pointer_scene_hit(&commands, point_x, point_y);
    let generation_after = capture_pointer_hit_generation(&commands);
    let pointer_metrics_after = capture_pointer_input_metrics(&commands);
    let last_position_after = capture_last_pointer_position(&commands);
    let pointer_focus_after = capture_pointer_focus_surface_id(&commands);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();

    assert_eq!(primed, (Some(surface_id), Some((40.0, 30.0))));
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0].surface_id, surface_id);
    assert_eq!(updated[0].origin_x, initial[0].origin_x - 10);
    assert_eq!(updated[0].origin_y, initial[0].origin_y);
    assert_eq!(hit_after_geometry, (Some(surface_id), Some((50.0, 30.0))));
    assert_ne!(generation_after, generation_before);
    assert!(
        pointer_metrics_after_publication.full_scene_hit_scans
            > pointer_metrics_before.full_scene_hit_scans
    );
    assert!(
        pointer_metrics_after.pointer_hit_generation_invalidations
            > pointer_metrics_before.pointer_hit_generation_invalidations
    );
    assert_eq!(
        pointer_metrics_after.full_scene_hit_scans,
        pointer_metrics_after_publication.full_scene_hit_scans
    );
    assert_eq!(last_position_after, last_position_before);
    assert_eq!(pointer_focus_after, pointer_focus_before);
}

#[test]
fn wayland_retained_mapping_resize_preview_converges_after_final_commit() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 64, 48).unwrap();
    surface.commit();
    connection.flush().unwrap();

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&surface, &shm, &qh, 64, 48).unwrap();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let initial = capture_renderable_surface_snapshot(&commands);
    let surface_id = initial[0].surface_id;
    let pointer_x = f64::from(initial[0].origin_x) + 20.0;
    let pointer_y = f64::from(initial[0].origin_y) + 20.0;
    commands
        .send(ServerCommand::PointerMotion {
            x: pointer_x,
            y: pointer_y,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    let initial_hit = capture_pointer_scene_hit(&commands, pointer_x, pointer_y);

    commands
        .send(ServerCommand::BeginResize {
            x: f64::from(initial[0].origin_x) + 62.0,
            y: f64::from(initial[0].origin_y) + 46.0,
        })
        .unwrap();
    commands
        .send(ServerCommand::UpdateInteraction {
            x: f64::from(initial[0].origin_x) + 198.0,
            y: f64::from(initial[0].origin_y) + 148.0,
        })
        .unwrap();
    commands.send(ServerCommand::PrepareFrame).unwrap();
    commands
        .send(ServerCommand::AdmitInteractiveVisualState {
            render_ahead: false,
        })
        .unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let resize_interaction = capture_window_interaction_debug_snapshot(&commands);
    assert!(
        resize_interaction.is_some(),
        "interactive resize should start"
    );

    surface.offset(5, 7);
    surface.commit();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let intermediate = capture_renderable_surface_snapshot(&commands);
    let intermediate_visual = capture_toplevel_visual_geometry(&commands);
    let intermediate_root = capture_root_window_geometry(&commands, surface_id);
    let intermediate_hit = capture_pointer_scene_hit(&commands, pointer_x, pointer_y);

    commands.send(ServerCommand::EndInteraction).unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&surface, &shm, &qh, 200, 150).unwrap();
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let final_snapshot = capture_renderable_surface_snapshot(&commands);
    let final_visual = capture_toplevel_visual_geometry(&commands);
    let final_root = capture_root_window_geometry(&commands, surface_id);
    let final_hit = capture_pointer_scene_hit(&commands, pointer_x, pointer_y);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();

    assert_eq!(initial_hit.0, Some(surface_id));
    assert_eq!(initial_hit.1, Some((20.0, 20.0)));
    assert_eq!(intermediate.len(), 1);
    assert_eq!((intermediate[0].width, intermediate[0].height), (64, 48));
    assert_eq!(
        (intermediate[0].content_x, intermediate[0].content_y),
        (5, 7)
    );
    assert!(
        intermediate_visual.is_some_and(|visual| {
            visual.active_resize && (visual.width, visual.height) == (200, 150)
        }),
        "intermediate visual geometry: {intermediate_visual:?}, interaction: {resize_interaction:?}"
    );
    assert_eq!(
        intermediate_root.map(|geometry| (geometry.width, geometry.height)),
        Some((200, 150))
    );
    assert_eq!(intermediate_hit.0, Some(surface_id));
    assert_eq!(intermediate_hit.1, Some((15.0, 13.0)));
    assert_eq!(final_snapshot.len(), 1);
    assert_eq!(
        (final_snapshot[0].width, final_snapshot[0].height),
        (200, 150)
    );
    assert!(final_visual.is_some_and(|visual| {
        !visual.active_resize && (visual.width, visual.height) == (200, 150)
    }));
    assert_eq!(
        final_root.map(|geometry| (geometry.width, geometry.height)),
        Some((200, 150))
    );
    assert_eq!(final_hit.0, Some(surface_id));
    assert_eq!(final_hit.1, Some((20.0, 20.0)));
    assert_eq!(state.surface_leave_count, 0);
}

#[test]
fn wayland_bufferless_offset_commit_updates_retained_mapping_without_damage() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let buffer = TestShmBuffer::new(&shm, &qh, 4, 3).unwrap();

    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    buffer.attach(&surface, 4, 3);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    let initial = capture_renderable_surface_snapshot(&commands);

    surface.offset(7, 9);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    let updated = capture_renderable_surface_snapshot(&commands);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();

    assert_eq!(initial.len(), 1);
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0].buffer_id, initial[0].buffer_id);
    assert_eq!(updated[0].pixel_checksum, initial[0].pixel_checksum);
    assert_eq!((updated[0].content_x, updated[0].content_y), (7, 9));
    assert!(updated[0].generation > initial[0].generation);
}

#[derive(Clone, Copy)]
enum SynchronizedViewportUpdate {
    SetSource,
    SetFractionalSource,
    SetDestination,
    ResetSource,
    ResetDestination,
    DestroyViewport,
    CreateViewportAndSetSource,
}

fn run_synchronized_viewport_updates(
    updates: &[SynchronizedViewportUpdate],
) -> (RenderableSurfaceSnapshot, RenderableSurfaceSnapshot) {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let subcompositor: client_wl_subcompositor::WlSubcompositor =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let viewporter: client_wp_viewporter::WpViewporter = globals.bind(&qh, 1..=1, ()).unwrap();
    let parent = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let child = compositor.create_surface(&qh, ());
    let _subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    let mut viewport = Some(viewporter.get_viewport(&child, &qh, ()));
    let buffer = TestShmBuffer::new(&shm, &qh, 4, 2).unwrap();

    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();

    viewport.as_ref().unwrap().set_source(0.0, 0.0, 4.0, 2.0);
    viewport.as_ref().unwrap().set_destination(4, 2);
    buffer.attach(&child, 4, 2);
    child.commit();
    commit_test_buffered_surface(&parent, &shm, &qh, 20, 15).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    let initial = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .find(|surface| surface.parent_surface_id.is_some())
        .expect("synchronized child should be mapped");

    for update in updates {
        match update {
            SynchronizedViewportUpdate::SetSource => viewport
                .as_ref()
                .expect("viewport should be alive")
                .set_source(1.0, 0.0, 2.0, 2.0),
            SynchronizedViewportUpdate::SetFractionalSource => viewport
                .as_ref()
                .expect("viewport should be alive")
                .set_source(0.0, 0.0, 2.5, 2.0),
            SynchronizedViewportUpdate::SetDestination => viewport
                .as_ref()
                .expect("viewport should be alive")
                .set_destination(3, 4),
            SynchronizedViewportUpdate::ResetSource => viewport
                .as_ref()
                .expect("viewport should be alive")
                .set_source(-1.0, -1.0, -1.0, -1.0),
            SynchronizedViewportUpdate::ResetDestination => viewport
                .as_ref()
                .expect("viewport should be alive")
                .set_destination(-1, -1),
            SynchronizedViewportUpdate::DestroyViewport => {
                viewport.take().expect("viewport should be alive").destroy();
            }
            SynchronizedViewportUpdate::CreateViewportAndSetSource => {
                let replacement = viewporter.get_viewport(&child, &qh, ());
                replacement.set_source(2.0, 0.0, 2.0, 2.0);
                viewport = Some(replacement);
            }
        }
        child.commit();
        connection.flush().unwrap();
        queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    }

    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    let final_snapshot = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .find(|surface| surface.parent_surface_id.is_some())
        .expect("synchronized child should remain mapped");

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
    (initial, final_snapshot)
}

#[test]
fn synchronized_viewport_source_then_destination_preserves_both_fields() {
    let (initial, final_snapshot) = run_synchronized_viewport_updates(&[
        SynchronizedViewportUpdate::SetSource,
        SynchronizedViewportUpdate::SetDestination,
    ]);

    assert_eq!(final_snapshot.viewport_source, Some((256, 0, 512, 512)));
    assert_eq!(final_snapshot.viewport_destination, Some((3, 4)));
    assert_eq!(final_snapshot.buffer_id, initial.buffer_id);
    assert_eq!(final_snapshot.pixel_checksum, initial.pixel_checksum);
}

#[test]
fn synchronized_viewport_destination_then_source_preserves_both_fields() {
    let (initial, final_snapshot) = run_synchronized_viewport_updates(&[
        SynchronizedViewportUpdate::SetDestination,
        SynchronizedViewportUpdate::SetSource,
    ]);

    assert_eq!(final_snapshot.viewport_source, Some((256, 0, 512, 512)));
    assert_eq!(final_snapshot.viewport_destination, Some((3, 4)));
    assert_eq!(final_snapshot.buffer_id, initial.buffer_id);
    assert_eq!(final_snapshot.pixel_checksum, initial.pixel_checksum);
}

#[test]
fn synchronized_viewport_final_composed_state_is_valid_after_intermediate_reset() {
    let (initial, final_snapshot) = run_synchronized_viewport_updates(&[
        SynchronizedViewportUpdate::ResetDestination,
        SynchronizedViewportUpdate::SetFractionalSource,
        SynchronizedViewportUpdate::SetDestination,
    ]);

    assert_eq!(initial.viewport_source, Some((0, 0, 1024, 512)));
    assert_eq!(initial.viewport_destination, Some((4, 2)));
    assert_eq!(final_snapshot.viewport_source, Some((0, 0, 640, 512)));
    assert_eq!(final_snapshot.viewport_destination, Some((3, 4)));
}

#[test]
fn synchronized_viewport_destination_reset_preserves_newer_source() {
    let (initial, final_snapshot) = run_synchronized_viewport_updates(&[
        SynchronizedViewportUpdate::SetSource,
        SynchronizedViewportUpdate::ResetDestination,
    ]);

    assert_eq!(final_snapshot.viewport_source, Some((256, 0, 512, 512)));
    assert_eq!(final_snapshot.viewport_destination, None);
    assert_eq!(final_snapshot.buffer_id, initial.buffer_id);
    assert_eq!(final_snapshot.pixel_checksum, initial.pixel_checksum);
}

#[test]
fn synchronized_viewport_source_reset_preserves_newer_destination() {
    let (initial, final_snapshot) = run_synchronized_viewport_updates(&[
        SynchronizedViewportUpdate::SetDestination,
        SynchronizedViewportUpdate::ResetSource,
    ]);

    assert_eq!(final_snapshot.viewport_source, None);
    assert_eq!(final_snapshot.viewport_destination, Some((3, 4)));
    assert_eq!(final_snapshot.buffer_id, initial.buffer_id);
    assert_eq!(final_snapshot.pixel_checksum, initial.pixel_checksum);
}

#[test]
fn synchronized_viewport_destroy_resets_both_fields_after_cached_destination_update() {
    let (initial, final_snapshot) = run_synchronized_viewport_updates(&[
        SynchronizedViewportUpdate::SetDestination,
        SynchronizedViewportUpdate::DestroyViewport,
    ]);

    assert_eq!(final_snapshot.viewport_source, None);
    assert_eq!(final_snapshot.viewport_destination, None);
    assert_eq!(final_snapshot.buffer_id, initial.buffer_id);
    assert_eq!(final_snapshot.pixel_checksum, initial.pixel_checksum);
}

#[test]
fn synchronized_viewport_new_source_after_destroy_preserves_destination_reset() {
    let (initial, final_snapshot) = run_synchronized_viewport_updates(&[
        SynchronizedViewportUpdate::DestroyViewport,
        SynchronizedViewportUpdate::CreateViewportAndSetSource,
    ]);

    assert_eq!(final_snapshot.viewport_source, Some((512, 0, 512, 512)));
    assert_eq!(final_snapshot.viewport_destination, None);
    assert_eq!(final_snapshot.buffer_id, initial.buffer_id);
    assert_eq!(final_snapshot.pixel_checksum, initial.pixel_checksum);
}

#[test]
fn synchronized_bufferless_mapping_is_captured_before_delayed_publication() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind_native_base(&socket_name).unwrap();
    server.set_presentation_clock(PresentationClock::Monotonic);
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let subcompositor: client_wl_subcompositor::WlSubcompositor =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let timing: client_wp_commit_timing_manager_v1::WpCommitTimingManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let parent = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let child = compositor.create_surface(&qh, ());
    let _subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());

    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    commit_test_buffered_surface(&child, &shm, &qh, 4, 3).unwrap();
    commit_test_buffered_surface(&parent, &shm, &qh, 20, 15).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    let initial_surfaces = capture_renderable_surface_snapshot(&commands);
    let initial = initial_surfaces
        .iter()
        .find(|surface| surface.parent_surface_id.is_some())
        .cloned()
        .unwrap_or_else(|| panic!("synchronized child should be mapped: {initial_surfaces:?}"));

    child.set_buffer_transform(client_wl_output::Transform::_180);
    child.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();

    let timer = timing.get_timer(&parent, &qh, ());
    let now = PresentationTimestamp::from_clock(PresentationClock::Monotonic).unwrap();
    let (seconds_hi, seconds_lo) = now.protocol_seconds();
    timer.set_timestamp(seconds_hi, seconds_lo.saturating_add(1), now.nanoseconds());
    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();

    child.set_buffer_transform(client_wl_output::Transform::_90);
    child.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    let before_release = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .find(|surface| surface.parent_surface_id.is_some())
        .expect("delayed child should remain mapped");
    assert_eq!(
        before_release.buffer_transform,
        wayland_server::protocol::wl_output::Transform::Normal
    );

    std::thread::sleep(std::time::Duration::from_millis(1_100));
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    let after_first_release = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .find(|surface| surface.parent_surface_id.is_some())
        .expect("first delayed child update should publish");
    assert_eq!(
        after_first_release.buffer_transform,
        wayland_server::protocol::wl_output::Transform::_180
    );

    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    let after_second_parent = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .find(|surface| surface.parent_surface_id.is_some())
        .expect("second delayed child update should publish");

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();

    assert_eq!(
        initial.buffer_transform,
        wayland_server::protocol::wl_output::Transform::Normal
    );
    assert_eq!(
        after_second_parent.buffer_transform,
        wayland_server::protocol::wl_output::Transform::_90
    );
    assert_eq!(after_first_release.buffer_id, initial.buffer_id);
    assert_eq!(after_first_release.pixel_checksum, initial.pixel_checksum);
    assert_eq!(after_second_parent.buffer_id, initial.buffer_id);
    assert_eq!(after_second_parent.pixel_checksum, initial.pixel_checksum);
}

#[test]
fn wayland_same_buffer_object_reuse_materializes_new_content_once() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let client_state = create_client_toplevel_with_reused_shm_buffer(&socket_path).unwrap();
    let server = stop_test_server(running, server_thread);

    assert_eq!(client_state.buffer_release_count, 2);
    assert_eq!(server.renderable_surfaces().len(), 1);
    assert_eq!(
        server.renderable_surfaces()[0].cpu_pixels(),
        Some(vec![0xffaa_0000, 0xff00_aa00, 0xff00_00aa, 0xffaa_aa00].as_slice())
    );
}

#[test]
fn two_buffer_shm_client_rotates_with_pending_and_ready_output_batches() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let result = exercise_two_buffer_shm_with_pending_and_ready_batches(&socket_path, &commands);
    let server = stop_controllable_test_server(commands, server_thread);
    let (state, resource_counts) = result.unwrap();

    assert_eq!(state.buffer_release_count, 3);
    assert_eq!(resource_counts.1, 0);
    assert_eq!(resource_counts.2, 2);
    let metrics = server.shm_buffer_lifetime_metrics();
    assert_eq!(metrics.shm_releases_after_materialization_total, 3);
    assert_eq!(metrics.presentation_bound_shm_release_total, 0);
}

#[test]
fn failed_shm_materialization_releases_without_false_materialization_proof() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let client_state = create_client_toplevel_with_invalid_shm_buffer(&socket_path).unwrap();
    let server = stop_test_server(running, server_thread);
    let metrics = server.shm_buffer_lifetime_metrics();

    assert_eq!(client_state.buffer_release_count, 1);
    assert_eq!(server.renderable_surfaces().len(), 0);
    assert_eq!(metrics.shm_materializations_total, 0);
    assert_eq!(metrics.shm_materialization_failures_total, 1);
    assert_eq!(metrics.shm_releases_after_materialization_total, 0);
    assert_eq!(metrics.shm_releases_deferred_unmaterialized_total, 1);
}

#[test]
fn unassigned_shm_is_retained_until_parent_activation_materializes_it() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let client_state = create_unassigned_shm_surface_then_adopt_toplevel(&socket_path).unwrap();
    let server = stop_test_server(running, server_thread);
    let metrics = server.shm_buffer_lifetime_metrics();

    assert_eq!(client_state.buffer_release_count, 1);
    assert_eq!(server.renderable_surfaces().len(), 2);
    assert_eq!(metrics.shm_releases_after_materialization_total, 2);
    assert_eq!(metrics.shm_releases_superseded_without_read_total, 0);
}

#[test]
fn shutdown_releases_retained_unmaterialized_shm_once() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let (connection, buffer) = create_unassigned_shm_surface_for_shutdown(&socket_path).unwrap();
    let mut server = stop_controllable_test_server(commands, server_thread);
    server.finish_commit_debug_for_shutdown();
    let metrics = server.shm_buffer_lifetime_metrics();

    assert_eq!(metrics.shm_releases_deferred_unmaterialized_total, 1);
    assert_eq!(metrics.shm_releases_superseded_without_read_total, 0);
    assert_eq!(metrics.shm_releases_after_materialization_total, 0);
    drop(buffer);
    drop(connection);
}

#[test]
fn repeated_wayland_buffer_commits_use_indexed_surface_access() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    create_client_toplevel_with_repeated_shm_buffer_commits(&socket_path, 1_000).unwrap();
    let server = stop_test_server(running, server_thread);
    let metrics = server.surface_locality_metrics();

    assert_eq!(metrics.content_indexed_lookups, 1_001);
    assert_eq!(metrics.global_renderable_index_rebuilds, 0);
}

#[test]
fn repeated_wayland_damage_only_commits_use_indexed_surface_access() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    create_client_toplevel_with_repeated_shm_damage_only_updates(&socket_path, 1_000).unwrap();
    let server = stop_test_server(running, server_thread);
    let metrics = server.surface_locality_metrics();

    assert_eq!(metrics.content_indexed_lookups, 1_001);
    assert_eq!(metrics.global_renderable_index_rebuilds, 0);
}

#[test]
fn wayland_same_size_buffer_rotation_preserves_partial_damage() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_client_toplevel_with_rotating_shm_buffers(&socket_path, &commands, true, true, true, 0)
        .unwrap();
    let server = stop_controllable_test_server(commands, server_thread);

    let surface = &server.renderable_surfaces()[0];
    assert_eq!(
        surface.damage,
        RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        }])
    );
}

#[test]
fn wayland_same_size_buffer_rotation_preserves_authoritative_empty_damage() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_client_toplevel_with_rotating_shm_buffers(&socket_path, &commands, true, true, false, 0)
        .unwrap();
    let server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(
        server.renderable_surfaces()[0].damage,
        RenderableSurfaceDamage::Empty
    );
}

#[test]
fn wayland_first_map_without_explicit_damage_stays_visually_live() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_client_toplevel_with_rotating_shm_buffers(
        &socket_path,
        &commands,
        false,
        false,
        false,
        0,
    )
    .unwrap();
    let server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(server.renderable_surfaces().len(), 1);
    assert_eq!(
        server.renderable_surfaces()[0].damage,
        RenderableSurfaceDamage::Full
    );
}

#[test]
fn wayland_content_commits_skip_global_stack_reorder() {
    const CONTENT_COMMITS: usize = 1_000;
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    create_client_toplevel_with_rotating_shm_buffers(
        &socket_path,
        &commands,
        true,
        true,
        true,
        CONTENT_COMMITS,
    )
    .unwrap();
    let server = stop_controllable_test_server(commands, server_thread);
    let metrics = server.core_compliance_metrics();

    assert_eq!(metrics.surface_commit_stack_reorders, 1);
    assert_eq!(metrics.surface_commit_buffer_rotations, 1_002);
    assert_eq!(
        metrics.surface_commit_stack_reorder_skips,
        (CONTENT_COMMITS + 3) as u64
    );
    assert_eq!(metrics.surface_commit_mapping_full_promotions, 1);
    assert_eq!(
        metrics.surface_commit_partial_damage_preserved,
        (CONTENT_COMMITS + 3) as u64
    );
    assert_eq!(metrics.surface_commit_empty_damage_preserved, 0);
    assert_eq!(
        metrics.surface_commit_geometry_noops,
        (CONTENT_COMMITS + 3) as u64
    );
    assert_eq!(metrics.surface_commit_popup_topology_updates, 0);
    assert_eq!(metrics.surface_commit_popup_pointer_refreshes, 0);
    assert_eq!(metrics.active_root_scene_refreshes, 1);
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
    assert_eq!(surface.generation, server.render_generation());
    assert_eq!(surface.buffer_source(), SurfaceBufferSource::Dmabuf);
    assert!(surface.cpu_pixels().is_none());
}

#[test]
fn wayland_surface_can_switch_from_dmabuf_to_shm_then_remove_content() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let result = create_client_toplevel_with_dmabuf_then_shm_and_remove(&socket_path, &commands);
    let server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(result.unwrap().buffer_release_count, 3);
    assert_eq!(server.renderable_surfaces().len(), 0);
    let metrics = server.shm_buffer_lifetime_metrics();
    assert_eq!(metrics.shm_releases_after_materialization_total, 2);
    assert_eq!(metrics.presentation_bound_shm_release_total, 0);
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
