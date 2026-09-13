use super::super::*;

use std::os::unix::net::UnixStream;
use std::sync::Arc;

use wayland_server::protocol::{wl_buffer, wl_callback, wl_output, wl_shm};

use super::*;

#[test]
fn terminal_surface_tree_transaction_is_discarded_without_watcher_cancel() {
    let mut state = CompositorState {
        external_acquire_readiness: true,
        surface_pipeline_trace:
            crate::compositor::surface_pipeline_trace::SurfacePipelineTrace::new(true, 16),
        ..Default::default()
    };
    let display = wayland_server::Display::<CompositorState>::new().expect("test display");
    let mut display_handle = display.handle();
    let (server_end, _peer) = UnixStream::pair().expect("test client socket");
    let client = display_handle
        .insert_client(server_end, Arc::new(()))
        .expect("test client");
    let surface =
        state.test_create_unmapped_surface_resource_at_version(&client, &display_handle, 1);
    let surface_id = compositor_surface_id(&surface);
    state.surface_presentation_generations.insert(surface_id, 1);
    state.mark_client_terminal(client.id());
    let mut node = empty_cached_subsurface_commit();
    node.commit_sequence = SurfaceCommitSequence(1);
    let callback = client
        .create_resource::<wl_callback::WlCallback, (), CompositorState>(&display_handle, 1, ())
        .expect("frame callback resource");
    node.frame_callbacks.push(callback.clone());
    state
        .pending_surface_tree_transactions
        .push(PendingSurfaceTreeTransaction {
            id: SurfaceTreeTransactionId::new(101),
            root_surface_id: surface_id,
            nodes: vec![(surface_id, node)],
            publication_lifetimes: SurfaceTreeNodeLifetimes::Captured(vec![
                SurfaceTreeNodeLifetime {
                    surface_id,
                    owner_client_id: client.id(),
                    surface_presentation_generation: 1,
                },
            ]),
            dependencies: vec![SurfaceTreeAcquireDependency {
                surface_commit_id: SurfaceCommitId::for_tests(102),
                commit_id: AcquireCommitId::for_tests(103),
                surface_id,
                owner_client_id: Some(client.id()),
                surface_presentation_generation: Some(1),
                buffer_id: 308,
                acquire: ExplicitSyncPoint::for_tests_with_signal_script(104, 105, [true]),
                state: PendingAcquireState::Ready,
            }],
            commit_timing_readiness: None,
            received_at: Instant::now(),
        });

    state.commit_ready_surface_tree_transactions();

    assert!(state.pending_surface_tree_transactions.is_empty());
    assert!(state.renderable_surface(surface_id).is_none());
    assert!(state.pending_acquire_watch_changes.is_empty());
    assert!(callback.is_alive());
    let records = state.surface_pipeline_trace.records().collect::<Vec<_>>();
    assert!(records.iter().any(|record| {
        record.kind == SurfacePipelineEvent::AcquireReadyDiscarded
            && record.rejection_reason == Some(SurfacePipelineRejectionReason::TerminalClient)
    }));
    assert!(records.iter().any(|record| {
        record.kind == SurfacePipelineEvent::PublicationRejected
            && record.rejection_reason == Some(SurfacePipelineRejectionReason::TerminalClient)
    }));
    assert!(
        !records
            .iter()
            .any(|record| record.kind == SurfacePipelineEvent::TransactionPromoted)
    );
}

#[test]
fn terminal_buffer_surface_tree_transaction_releases_owned_buffer_once() {
    let mut state = CompositorState {
        external_acquire_readiness: false,
        surface_pipeline_trace:
            crate::compositor::surface_pipeline_trace::SurfacePipelineTrace::new(true, 16),
        ..Default::default()
    };
    let display = wayland_server::Display::<CompositorState>::new().expect("test display");
    let mut display_handle = display.handle();
    let (server_end, _peer) = UnixStream::pair().expect("test client socket");
    let client = display_handle
        .insert_client(server_end, Arc::new(()))
        .expect("test client");
    let surface =
        state.test_create_unmapped_surface_resource_at_version(&client, &display_handle, 1);
    let surface_id = compositor_surface_id(&surface);
    let owner_client_id = client.id();
    state.surface_presentation_generations.insert(surface_id, 1);

    let buffer_data = ShmBufferData {
        identity: state.allocate_buffer_identity().expect("buffer identity"),
        pool: Arc::new(ShmPoolData::new(
            Arc::new(std::fs::File::open("/dev/null").expect("test file")),
            4,
        )),
        offset: 0,
        width: 1,
        height: 1,
        stride: 4,
        format: wayland_server::WEnum::Value(wl_shm::Format::Argb8888),
    };
    let buffer = client
        .create_resource::<wl_buffer::WlBuffer, ShmBufferData, CompositorState>(
            &display_handle,
            2,
            buffer_data.clone(),
        )
        .expect("buffer resource");
    let callback = client
        .create_resource::<wl_callback::WlCallback, (), CompositorState>(&display_handle, 3, ())
        .expect("frame callback resource");
    let acquire = ExplicitSyncPoint::for_tests_with_signal_script(401, 402, [true]);
    let release = ExplicitSyncPoint::for_tests_with_signal_script(403, 404, [true]);
    let acquire_commit_id = AcquireCommitId::for_tests(405);
    let mut node = empty_cached_subsurface_commit();
    node.commit_id = SurfaceCommitId::for_tests(406);
    node.commit_sequence = SurfaceCommitSequence(1);
    node.attachment = Some(PendingSurfaceAttachment::Buffer(PendingSurfaceBuffer {
        resource: buffer.clone(),
        data: PendingBufferData::Shm(buffer_data),
        x: 0,
        y: 0,
        explicit_release: Some(release),
        surface_size: Some(BufferSize::new(1, 1).expect("buffer size")),
        viewport_source: None,
        viewport_destination: None,
        buffer_scale: 1,
        commit_sequence: SurfaceCommitSequence(1),
        resize_commit: None,
        resize_capture_finalized: false,
        buffer_transform: wl_output::Transform::Normal,
        opaque_region: SurfaceOpaqueRegion::None,
    }));
    node.frame_callbacks.push(callback.clone());
    state
        .pending_surface_tree_transactions
        .push(PendingSurfaceTreeTransaction {
            id: SurfaceTreeTransactionId::new(407),
            root_surface_id: surface_id,
            nodes: vec![(surface_id, node)],
            publication_lifetimes: SurfaceTreeNodeLifetimes::Captured(vec![
                SurfaceTreeNodeLifetime {
                    surface_id,
                    owner_client_id: owner_client_id.clone(),
                    surface_presentation_generation: 1,
                },
            ]),
            dependencies: vec![SurfaceTreeAcquireDependency {
                surface_commit_id: SurfaceCommitId::for_tests(406),
                commit_id: acquire_commit_id,
                surface_id,
                owner_client_id: Some(owner_client_id.clone()),
                surface_presentation_generation: Some(1),
                buffer_id: buffer.id().protocol_id(),
                acquire: acquire.clone(),
                state: PendingAcquireState::EventfdBacked,
            }],
            commit_timing_readiness: None,
            received_at: Instant::now(),
        });
    state.enable_external_acquire_readiness();
    assert_eq!(state.take_acquire_watch_changes().len(), 1);
    let release_before = state.buffer_release_metrics();
    let render_before = state.render_generation;
    let scene_render_before = state.scene_render_generation;

    state.post_protocol_error_deferred(
        &client,
        &surface,
        1u32,
        "test fatal error before normal client teardown",
    );
    assert!(state.terminal_client_ids.contains(&owner_client_id));
    assert!(state.mark_acquire_commit_ready(acquire_commit_id, surface_id, &acquire));
    state.commit_ready_surface_tree_transactions();

    assert!(state.pending_surface_tree_transactions.is_empty());
    assert!(!state.current_surface_buffers.contains_key(&surface_id));
    assert!(state.renderable_surface(surface_id).is_none());
    assert!(!state.surface_publications.contains_key(&surface_id));
    assert_eq!(state.render_generation, render_before);
    assert_eq!(state.scene_render_generation, scene_render_before);
    assert!(state.pending_surface_presentation_feedbacks.is_empty());
    assert!(state.pending_acquire_watch_changes.is_empty());
    assert!(state.pending_dmabuf_buffer_releases.is_empty());
    assert!(state.deferred_dmabuf_buffer_releases.is_empty());
    assert!(state.explicit_release_signal_retries.is_empty());
    assert!(state.active_toplevel_resizes.is_empty());
    assert!(
        !state
            .pending_frame_callback_surfaces
            .contains_key(&callback.id())
    );
    assert!(
        !state
            .pending_frame_callback_timing
            .contains_key(&callback.id())
    );
    assert!(callback.is_alive());

    let release_after = state.buffer_release_metrics();
    assert_eq!(
        release_after.buffer_releases_completed,
        release_before.buffer_releases_completed + 1
    );
    assert_eq!(
        release_after.buffer_release_duplicate_attempts,
        release_before.buffer_release_duplicate_attempts
    );
    assert_eq!(
        release_after.explicit_release_signal_failures,
        release_before.explicit_release_signal_failures
    );
    assert_eq!(
        release_after.explicit_release_signal_retries,
        release_before.explicit_release_signal_retries
    );
    let records = state.surface_pipeline_trace.records().collect::<Vec<_>>();
    assert!(records.iter().any(|record| {
        record.kind == SurfacePipelineEvent::PublicationRejected
            && record.rejection_reason == Some(SurfacePipelineRejectionReason::TerminalClient)
    }));
    assert!(
        !records
            .iter()
            .any(|record| record.kind == SurfacePipelineEvent::TransactionPromoted)
    );

    assert!(!state.mark_acquire_commit_ready(acquire_commit_id, surface_id, &acquire));
    state.commit_ready_surface_tree_transactions();
    assert_eq!(
        state.buffer_release_metrics().buffer_releases_completed,
        release_after.buffer_releases_completed
    );
    assert!(state.pending_dmabuf_buffer_releases.is_empty());
    assert!(state.pending_acquire_watch_changes.is_empty());
}

#[test]
fn terminal_pending_explicit_sync_commit_is_discarded_before_teardown() {
    let mut state = CompositorState {
        external_acquire_readiness: true,
        ..Default::default()
    };
    let display = wayland_server::Display::<CompositorState>::new().expect("test display");
    let mut display_handle = display.handle();
    let (server_end, _peer) = UnixStream::pair().expect("test client socket");
    let client = display_handle
        .insert_client(server_end, Arc::new(()))
        .expect("test client");
    let surface =
        state.test_create_unmapped_surface_resource_at_version(&client, &display_handle, 1);
    let surface_id = compositor_surface_id(&surface);
    state.surface_presentation_generations.insert(surface_id, 1);
    let owner_client_id = client.id();
    let buffer_data = ShmBufferData {
        identity: state.allocate_buffer_identity().expect("buffer identity"),
        pool: Arc::new(ShmPoolData::new(
            Arc::new(std::fs::File::open("/dev/null").expect("test file")),
            4,
        )),
        offset: 0,
        width: 1,
        height: 1,
        stride: 4,
        format: wayland_server::WEnum::Value(wl_shm::Format::Argb8888),
    };
    let buffer = client
        .create_resource::<wl_buffer::WlBuffer, ShmBufferData, CompositorState>(
            &display_handle,
            2,
            buffer_data.clone(),
        )
        .expect("buffer resource");
    let callback = client
        .create_resource::<wl_callback::WlCallback, (), CompositorState>(&display_handle, 3, ())
        .expect("frame callback resource");
    let acquire = ExplicitSyncPoint::for_tests_with_signal_script(301, 302, [true]);
    let commit_id = AcquireCommitId::for_tests(303);
    state
        .pending_explicit_sync_commits
        .push(PendingExplicitSyncCommit {
            surface_commit_id: SurfaceCommitId::for_tests(304),
            commit_id,
            surface_id,
            owner_client_id: owner_client_id.clone(),
            surface_presentation_generation: 1,
            commit_sequence: SurfaceCommitSequence(1),
            pending: PendingSurfaceBuffer {
                resource: buffer,
                data: PendingBufferData::Shm(buffer_data),
                x: 0,
                y: 0,
                explicit_release: None,
                surface_size: Some(BufferSize::new(1, 1).expect("buffer size")),
                viewport_source: None,
                viewport_destination: None,
                buffer_scale: 1,
                commit_sequence: SurfaceCommitSequence(1),
                resize_commit: None,
                resize_capture_finalized: false,
                buffer_transform: wl_output::Transform::Normal,
                opaque_region: SurfaceOpaqueRegion::None,
            },
            damage: RenderableSurfaceDamage::Full,
            window_geometry: None,
            frame_callbacks: vec![callback.clone()],
            presentation_feedbacks: Vec::new(),
            acquire: acquire.clone(),
            acquire_state: PendingAcquireState::EventfdBacked,
        });
    state.rebuild_scene_work_index();
    assert_eq!(state.pending_explicit_sync_commits.len(), 1);
    let release_before = state.buffer_release_metrics();

    state.post_protocol_error_deferred(
        &client,
        &surface,
        1u32,
        "test fatal error before normal client teardown",
    );
    assert!(state.terminal_client_ids.contains(&owner_client_id));
    assert!(state.mark_acquire_commit_ready(commit_id, surface_id, &acquire));
    state.commit_ready_explicit_sync_buffers();

    assert!(state.pending_explicit_sync_commits.is_empty());
    assert!(!state.current_surface_buffers.contains_key(&surface_id));
    assert!(state.renderable_surface(surface_id).is_none());
    assert!(!state.surface_publications.contains_key(&surface_id));
    assert!(
        !state
            .pending_frame_callback_surfaces
            .contains_key(&callback.id())
    );
    assert!(
        !state
            .pending_frame_callback_timing
            .contains_key(&callback.id())
    );
    assert!(callback.is_alive());
    let release_after = state.buffer_release_metrics();
    assert_eq!(
        release_after.buffer_releases_completed,
        release_before.buffer_releases_completed + 1
    );
    assert_eq!(
        release_after.buffer_release_duplicate_attempts,
        release_before.buffer_release_duplicate_attempts
    );
    assert_eq!(
        release_after.buffer_releases_discarded,
        release_before.buffer_releases_discarded
    );

    assert!(!state.mark_acquire_commit_ready(commit_id, surface_id, &acquire));
    state.commit_ready_explicit_sync_buffers();
    assert_eq!(
        state.buffer_release_metrics().buffer_releases_completed,
        release_after.buffer_releases_completed
    );
}

#[test]
fn commit_timing_only_surface_tree_is_rejected_after_terminal_owner_before_release() {
    let mut state = CompositorState::default();
    let display = wayland_server::Display::<CompositorState>::new().expect("test display");
    let mut display_handle = display.handle();
    let (server_end, _peer) = UnixStream::pair().expect("test client socket");
    let client = display_handle
        .insert_client(server_end, Arc::new(()))
        .expect("test client");
    let surface =
        state.test_create_unmapped_surface_resource_at_version(&client, &display_handle, 1);
    let surface_id = compositor_surface_id(&surface);
    state.surface_presentation_generations.insert(surface_id, 1);
    let owner_client_id = client.id();
    let now = client_pacing_now_ns();
    let future = CommitTimingConstraint::from_protocol(now / 1_000_000_000 + 3_600, 0)
        .expect("future timing");
    let due =
        CommitTimingConstraint::from_protocol(now / 1_000_000_000 - 1, 0).expect("past timing");
    let mut commit = empty_cached_subsurface_commit();
    commit.commit_sequence = SurfaceCommitSequence(1);
    commit.attachment = Some(PendingSurfaceAttachment::RemoveContent);
    commit.pacing.commit_timing = Some(future);
    state
        .pending_surface_tree_transactions
        .push(PendingSurfaceTreeTransaction {
            id: SurfaceTreeTransactionId::new(200),
            root_surface_id: surface_id,
            nodes: vec![(surface_id, commit)],
            publication_lifetimes: SurfaceTreeNodeLifetimes::Captured(vec![
                SurfaceTreeNodeLifetime {
                    surface_id,
                    owner_client_id: owner_client_id.clone(),
                    surface_presentation_generation: 1,
                },
            ]),
            dependencies: Vec::new(),
            commit_timing_readiness: None,
            received_at: Instant::now(),
        });

    assert!(!state.transaction_is_ready(&state.pending_surface_tree_transactions[0]));
    state.mark_client_terminal(owner_client_id);
    state.commit_ready_surface_tree_transactions();
    assert_eq!(state.pending_surface_tree_transactions.len(), 1);

    state.pending_surface_tree_transactions[0].nodes[0]
        .1
        .pacing
        .commit_timing = Some(due);
    state.commit_ready_surface_tree_transactions();

    assert!(state.pending_surface_tree_transactions.is_empty());
    assert!(state.renderable_surface(surface_id).is_none());
    assert!(!state.surface_publications.contains_key(&surface_id));
}

#[test]
fn fifo_only_surface_tree_is_rejected_after_terminal_owner_before_barrier_release() {
    let mut state = CompositorState::default();
    let display = wayland_server::Display::<CompositorState>::new().expect("test display");
    let mut display_handle = display.handle();
    let (server_end, _peer) = UnixStream::pair().expect("test client socket");
    let client = display_handle
        .insert_client(server_end, Arc::new(()))
        .expect("test client");
    let surface =
        state.test_create_unmapped_surface_resource_at_version(&client, &display_handle, 1);
    let surface_id = compositor_surface_id(&surface);
    state.surface_presentation_generations.insert(surface_id, 1);
    let owner_client_id = client.id();
    state.active_fifo_barriers.insert(
        surface_id,
        ActiveFifoBarrier {
            surface_generation: 1,
            fifo_barrier_generation: FifoBarrierGeneration::new(1),
            commit_sequence: SurfaceCommitSequence(1),
            fallback_deadline_ns: client_pacing_now_ns().saturating_add(3_600_000_000_000),
        },
    );
    let mut commit = empty_cached_subsurface_commit();
    commit.commit_sequence = SurfaceCommitSequence(1);
    commit.attachment = Some(PendingSurfaceAttachment::RemoveContent);
    commit.pacing.fifo_wait_barrier = true;
    state
        .pending_surface_tree_transactions
        .push(PendingSurfaceTreeTransaction {
            id: SurfaceTreeTransactionId::new(201),
            root_surface_id: surface_id,
            nodes: vec![(surface_id, commit)],
            publication_lifetimes: SurfaceTreeNodeLifetimes::Captured(vec![
                SurfaceTreeNodeLifetime {
                    surface_id,
                    owner_client_id: owner_client_id.clone(),
                    surface_presentation_generation: 1,
                },
            ]),
            dependencies: Vec::new(),
            commit_timing_readiness: None,
            received_at: Instant::now(),
        });

    assert!(
        state.pending_surface_tree_transactions[0]
            .dependencies
            .is_empty()
    );
    assert!(!state.transaction_is_ready(&state.pending_surface_tree_transactions[0]));
    state.mark_client_terminal(owner_client_id);
    state.commit_ready_surface_tree_transactions();
    assert_eq!(state.pending_surface_tree_transactions.len(), 1);

    state.active_fifo_barriers.remove(&surface_id);
    state.commit_ready_surface_tree_transactions();

    assert!(state.pending_surface_tree_transactions.is_empty());
    assert!(state.renderable_surface(surface_id).is_none());
    assert!(!state.surface_publications.contains_key(&surface_id));
}

#[test]
fn mixed_surface_tree_lifetimes_reject_unrelated_stale_node_when_acquire_is_ready() {
    let mut state = CompositorState::default();
    let display = wayland_server::Display::<CompositorState>::new().expect("test display");
    let mut display_handle = display.handle();
    let (server_end, _peer) = UnixStream::pair().expect("test client socket");
    let client = display_handle
        .insert_client(server_end, Arc::new(()))
        .expect("test client");
    let surface_a =
        state.test_create_unmapped_surface_resource_at_version(&client, &display_handle, 1);
    let surface_b =
        state.test_create_unmapped_surface_resource_at_version(&client, &display_handle, 1);
    let surface_a_id = compositor_surface_id(&surface_a);
    let surface_b_id = compositor_surface_id(&surface_b);
    state
        .surface_presentation_generations
        .insert(surface_a_id, 1);
    state
        .surface_presentation_generations
        .insert(surface_b_id, 1);
    let owner_client_id = client.id();
    let mut commit_a = empty_cached_subsurface_commit();
    commit_a.commit_sequence = SurfaceCommitSequence(1);
    commit_a.attachment = Some(PendingSurfaceAttachment::RemoveContent);
    let mut commit_b = empty_cached_subsurface_commit();
    commit_b.commit_sequence = SurfaceCommitSequence(2);
    commit_b.attachment = Some(PendingSurfaceAttachment::RemoveContent);
    let acquire = ExplicitSyncPoint::for_tests_with_signal_script(202, 203, [true]);
    state
        .pending_surface_tree_transactions
        .push(PendingSurfaceTreeTransaction {
            id: SurfaceTreeTransactionId::new(202),
            root_surface_id: surface_a_id,
            nodes: vec![(surface_a_id, commit_a), (surface_b_id, commit_b)],
            publication_lifetimes: SurfaceTreeNodeLifetimes::Captured(vec![
                SurfaceTreeNodeLifetime {
                    surface_id: surface_a_id,
                    owner_client_id: owner_client_id.clone(),
                    surface_presentation_generation: 1,
                },
                SurfaceTreeNodeLifetime {
                    surface_id: surface_b_id,
                    owner_client_id: owner_client_id.clone(),
                    surface_presentation_generation: 1,
                },
            ]),
            dependencies: vec![SurfaceTreeAcquireDependency {
                surface_commit_id: SurfaceCommitId::for_tests(204),
                commit_id: AcquireCommitId::for_tests(205),
                surface_id: surface_a_id,
                owner_client_id: Some(owner_client_id),
                surface_presentation_generation: Some(1),
                buffer_id: 206,
                acquire: acquire.clone(),
                state: PendingAcquireState::EventfdBacked,
            }],
            commit_timing_readiness: None,
            received_at: Instant::now(),
        });
    state
        .surface_presentation_generations
        .insert(surface_b_id, 2);

    assert_eq!(
        state.pending_surface_tree_transactions[0]
            .dependencies
            .len(),
        1
    );
    assert!(state.mark_acquire_commit_ready(
        AcquireCommitId::for_tests(205),
        surface_a_id,
        &acquire
    ));
    state.commit_ready_surface_tree_transactions();

    assert!(state.pending_surface_tree_transactions.is_empty());
    assert!(state.renderable_surface(surface_a_id).is_none());
    assert!(state.renderable_surface(surface_b_id).is_none());
    assert!(!state.surface_publications.contains_key(&surface_a_id));
    assert!(!state.surface_publications.contains_key(&surface_b_id));
}

#[test]
fn terminal_frame_callback_discard_removes_bookkeeping_without_done() {
    let mut state = CompositorState::default();
    let display = wayland_server::Display::<CompositorState>::new().expect("test display");
    let mut display_handle = display.handle();
    let (server_end, _peer) = UnixStream::pair().expect("test client socket");
    let client = display_handle
        .insert_client(server_end, Arc::new(()))
        .expect("test client");
    let surface =
        state.test_create_unmapped_surface_resource_at_version(&client, &display_handle, 1);
    let surface_id = compositor_surface_id(&surface);
    let callback = client
        .create_resource::<wl_callback::WlCallback, (), CompositorState>(&display_handle, 1, ())
        .expect("callback resource");
    state.queue_frame_callbacks_for_surface(surface_id, vec![callback.clone()]);
    state.discard_frame_callbacks(vec![callback.clone()]);

    assert!(state.pending_frame_callbacks.is_empty());
    assert!(state.visible_pending_frame_callbacks.is_empty());
    assert!(
        !state
            .pending_frame_callback_surfaces
            .contains_key(&callback.id())
    );
    assert!(
        !state
            .pending_frame_callback_timing
            .contains_key(&callback.id())
    );
    assert!(callback.is_alive());

    let callback_in_batch = client
        .create_resource::<wl_callback::WlCallback, (), CompositorState>(&display_handle, 1, ())
        .expect("batch callback resource");
    state
        .pending_frame_callback_surfaces
        .insert(callback_in_batch.id(), surface_id);
    state
        .visible_pending_frame_callbacks
        .push(callback_in_batch.clone());
    state.visible_pending_frame_callback_count = 1;
    let batch_id = state.take_frame_batch_for_render(300);
    state.discard_frame_callbacks(vec![callback_in_batch]);

    let batch = state.frame_batches.get(&batch_id).expect("frame batch");
    assert!(batch.callbacks.is_empty());
    assert_eq!(batch.callback_settlement.cancelled, 1);
    assert!(batch.callback_settlement.is_reconciled());
}
