use super::*;

#[test]
fn acquire_dependency_ids_are_allocated_by_surface_transaction_state() {
    let mut state = CompositorState::default();

    assert_eq!(
        state
            .surface_transactions
            .allocate_acquire_commit_id()
            .expect("first acquire dependency ID")
            .get(),
        1
    );
    assert_eq!(
        state
            .surface_transactions
            .allocate_acquire_commit_id()
            .expect("second acquire dependency ID")
            .get(),
        2
    );
}

#[test]
fn tree_acquire_preparation_consumes_raw_state_and_preserves_release_ownership() {
    for (sequence, acquire_ready) in [(1_u64, true), (2, false)] {
        let mut state = CompositorState::default();
        let (display, client, surface_id) = test_surface_and_client(&mut state);
        let display_handle = display.handle();
        let surface = state
            .surface_resource_by_id(surface_id)
            .expect("test surface resource");
        let sync_state =
            std::sync::Arc::new(SyncobjSurfaceState::new(surface_id, surface.downgrade()));
        let acquire = if acquire_ready {
            let Some(device) = crate::syncobj::DrmSyncobjDevice::open_available() else {
                continue;
            };
            let timeline = device
                .create_timeline_for_tests()
                .expect("test acquire timeline");
            timeline
                .signal_point(300 + sequence)
                .expect("signal test acquire");
            ExplicitSyncPoint::new(
                timeline,
                0,
                u32::try_from(300 + sequence).expect("test acquire point"),
            )
        } else {
            ExplicitSyncPoint::for_tests_with_signal_script(
                200 + u32::try_from(sequence).expect("test acquire handle"),
                300 + sequence,
                [false],
            )
        };
        sync_state.set_pending_acquire(acquire);
        sync_state.set_pending_release(ExplicitSyncPoint::for_tests(
            400 + u32::try_from(sequence).expect("test release handle"),
            500 + sequence,
        ));

        let mut commit = test_mergeable_commit(sequence);
        let commit_id = commit.commit_id;
        commit.attachment = Some(PendingSurfaceAttachment::Buffer(
            test_pending_dmabuf_buffer(&mut state, &client, &display_handle, 2, 64, 64),
        ));
        commit.explicit_sync = Some(CapturedExplicitSyncState::capture(sync_state));
        let mut nodes = vec![(surface_id, commit)];

        state
            .prepare_surface_tree_surface_state(surface_id, &mut nodes, &[])
            .expect("mapping preflight");
        let dependencies = state
            .prepare_surface_tree_acquires(&mut nodes)
            .expect("valid explicit-sync points");

        let prepared = &nodes[0].1;
        assert!(prepared.explicit_sync.is_none());
        let Some(PendingSurfaceAttachment::Buffer(pending)) = prepared.attachment.as_ref() else {
            panic!("prepared node must retain its buffer");
        };
        assert!(pending.explicit_release.is_some());
        if acquire_ready {
            assert!(dependencies.is_empty());
        } else {
            assert_eq!(dependencies.len(), 1);
            assert_eq!(dependencies[0].surface_commit_id, commit_id);
            assert_eq!(dependencies[0].surface_id, surface_id);
            assert_eq!(
                dependencies[0].buffer_id,
                pending.resource.id().protocol_id()
            );
            assert_eq!(
                dependencies[0].state,
                PendingAcquireState::RegistrationPending
            );
        }
        assert!(state.pending_acquire_watch_changes.is_empty());
    }
}

#[test]
fn captured_explicit_sync_commit_registers_one_surface_tree_acquire() {
    let mut state = CompositorState {
        external_acquire_readiness: true,
        ..CompositorState::default()
    };
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let surface = state
        .surface_resource_by_id(surface_id)
        .expect("test surface resource");
    let acquire = ExplicitSyncPoint::for_tests_with_signal_script(30, 31, [false]);
    let release = ExplicitSyncPoint::for_tests(40, 41);
    let sync_state = std::sync::Arc::new(SyncobjSurfaceState::new(surface_id, surface.downgrade()));
    sync_state.set_pending_acquire(acquire.clone());
    sync_state.set_pending_release(release.clone());

    let mut commit = test_mergeable_commit(1);
    let pending = test_pending_dmabuf_buffer(&mut state, &client, &display_handle, 2, 64, 64);
    let buffer_id = pending.data.buffer_id();
    let buffer_protocol_id = pending.resource.id().protocol_id();
    commit.attachment = Some(PendingSurfaceAttachment::Buffer(pending));
    commit.explicit_sync = Some(CapturedExplicitSyncState::capture(sync_state));

    state.commit_surface_tree_request(surface_id, commit);

    let dependency_commit_id = {
        let transaction = state
            .surface_transactions
            .pending_tree_at_for_test(0)
            .expect("explicit-sync commit enters the SurfaceTree queue");
        assert_eq!(transaction.dependencies.len(), 1);
        let dependency = &transaction.dependencies[0];
        assert_eq!(dependency.commit_id.get(), 1);
        assert_eq!(dependency.surface_id, surface_id);
        assert_eq!(dependency.buffer_id, buffer_protocol_id);
        assert_eq!(dependency.acquire, acquire);
        assert_eq!(dependency.state, PendingAcquireState::RegistrationPending);
        let PendingSurfaceAttachment::Buffer(pending) = transaction.nodes[0]
            .1
            .attachment
            .as_ref()
            .expect("prepared explicit-sync buffer")
        else {
            panic!("prepared explicit-sync buffer must remain attached");
        };
        assert_eq!(pending.explicit_release.as_ref(), Some(&release));
        dependency.commit_id
    };

    let watch_changes = state.take_acquire_watch_changes();
    let [AcquireWatchChange::Register(watch)] = watch_changes.as_slice() else {
        panic!("one SurfaceTree acquire watch must be registered");
    };
    assert_eq!(watch.commit_id, dependency_commit_id);
    assert_eq!(watch.surface_id, surface_id);
    assert_eq!(watch.buffer_id, buffer_protocol_id);
    assert_eq!(watch.acquire, acquire);

    assert!(!state.mark_acquire_commit_ready(dependency_commit_id, surface_id + 1, &acquire,));
    assert!(state.mark_acquire_commit_eventfd_backed(dependency_commit_id));
    assert!(state.mark_acquire_commit_ready(dependency_commit_id, surface_id, &acquire,));
    assert!(!state.mark_acquire_commit_ready(dependency_commit_id, surface_id, &acquire,));
    state.commit_ready_surface_tree_transactions();

    assert_eq!(state.surface_transactions.pending_tree_count(), 0);
    assert_eq!(
        state
            .surface_publications
            .get(&surface_id)
            .and_then(|publication| publication.latest_published_buffer_id),
        Some(buffer_id)
    );
}

#[test]
fn locally_signaled_surface_tree_acquire_publishes_during_readiness_progression() {
    let mut state = CompositorState {
        external_acquire_readiness: false,
        ..CompositorState::default()
    };
    let Some(device) = crate::syncobj::DrmSyncobjDevice::open_available() else {
        return;
    };
    let timeline = device
        .create_timeline_for_tests()
        .expect("test acquire timeline");
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let surface = state
        .surface_resource_by_id(surface_id)
        .expect("test surface resource");
    let acquire = ExplicitSyncPoint::new(timeline.clone(), 0, 301);
    let release = ExplicitSyncPoint::for_tests(302, 303);
    let sync_state = std::sync::Arc::new(SyncobjSurfaceState::new(surface_id, surface.downgrade()));
    sync_state.set_pending_acquire(acquire);
    sync_state.set_pending_release(release.clone());

    let mut commit = test_mergeable_commit(1);
    let pending = test_pending_dmabuf_buffer(&mut state, &client, &display_handle, 2, 64, 64);
    let buffer_id = pending.data.buffer_id();
    commit.attachment = Some(PendingSurfaceAttachment::Buffer(pending));
    commit.explicit_sync = Some(CapturedExplicitSyncState::capture(sync_state));

    state.commit_surface_tree_request(surface_id, commit);
    let transaction = state
        .surface_transactions
        .pending_tree_at_for_test(0)
        .expect("unsignaled SurfaceTree transaction waits");
    assert_eq!(transaction.dependencies.len(), 1);
    assert_eq!(
        transaction.dependencies[0].state,
        PendingAcquireState::RegistrationPending
    );
    assert_eq!(
        state
            .surface_publications
            .get(&surface_id)
            .and_then(|publication| publication.latest_published_buffer_id),
        None
    );

    timeline
        .signal_point(301)
        .expect("signal local acquire point");
    // prepare_frame now calls this live progression boundary directly.
    state.commit_ready_surface_tree_transactions();

    assert_eq!(state.surface_transactions.pending_tree_count(), 0);
    assert_eq!(
        state
            .surface_publications
            .get(&surface_id)
            .and_then(|publication| publication.latest_published_buffer_id),
        Some(buffer_id)
    );
}

#[test]
fn shutdown_settles_pending_surface_tree_explicit_sync_ownership_once() {
    let mut state = CompositorState {
        external_acquire_readiness: true,
        ..CompositorState::default()
    };
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let surface = state
        .surface_resource_by_id(surface_id)
        .expect("test surface resource");
    let acquire = ExplicitSyncPoint::for_tests_with_signal_script(30, 31, [false]);
    let release = ExplicitSyncPoint::for_tests_with_signal_script(40, 41, [true]);
    let release_signal_script = release
        .signal_script
        .as_ref()
        .expect("test release signal script")
        .clone();
    let sync_state = std::sync::Arc::new(SyncobjSurfaceState::new(surface_id, surface.downgrade()));
    sync_state.set_pending_acquire(acquire);
    sync_state.set_pending_release(release.clone());

    let callback = client
        .create_resource::<wl_callback::WlCallback, (), CompositorState>(&display_handle, 3, ())
        .expect("frame callback resource");
    let callback_id = callback.id();
    state
        .pending_frame_callback_surfaces
        .insert(callback_id.clone(), surface_id);
    let mut commit = test_mergeable_commit(1);
    let pending = test_pending_dmabuf_buffer(&mut state, &client, &display_handle, 2, 64, 64);
    let buffer_id = pending.data.buffer_id();
    commit.attachment = Some(PendingSurfaceAttachment::Buffer(pending));
    commit.frame_callbacks.push(callback);
    commit.explicit_sync = Some(CapturedExplicitSyncState::capture(sync_state));
    state.note_explicit_commit_captured(
        commit.commit_id,
        surface_id,
        commit.commit_sequence.get(),
        Some(buffer_id.get()),
        &commit.frame_callbacks,
    );

    state.commit_surface_tree_request(surface_id, commit);
    let registrations = state.take_acquire_watch_changes();
    assert!(matches!(
        registrations.as_slice(),
        [AcquireWatchChange::Register(_)]
    ));
    let transaction = state
        .surface_transactions
        .pending_tree_at_for_test(0)
        .expect("explicit-sync work is transaction-owned");
    assert_eq!(transaction.dependencies.len(), 1);
    let PendingSurfaceAttachment::Buffer(pending) = transaction.nodes[0]
        .1
        .attachment
        .as_ref()
        .expect("prepared explicit-sync buffer")
    else {
        panic!("prepared explicit-sync buffer remains attached");
    };
    assert_eq!(pending.explicit_release.as_ref(), Some(&release));
    assert_eq!(
        release_signal_script.lock().expect("release script").len(),
        1
    );
    let release_count_before = state.buffer_release_metrics().buffer_releases_completed;

    let mut releases = ShutdownDmabufReleaseSet::default();
    state.release_cached_resources_for_shutdown(&mut releases);

    assert_eq!(state.surface_transactions.pending_tree_count(), 0);
    assert!(
        !state
            .pending_frame_callback_surfaces
            .contains_key(&callback_id)
    );
    assert_eq!(
        state.buffer_release_metrics().buffer_releases_completed,
        release_count_before
    );
    assert_eq!(
        release_signal_script.lock().expect("release script").len(),
        1,
        "release ownership remains aggregated until shutdown settlement"
    );
    assert!(matches!(
        state.take_acquire_watch_changes().as_slice(),
        [AcquireWatchChange::Cancel {
            reason: AcquireWatchCancelReason::BackendShutdown,
            ..
        }]
    ));

    releases.complete(&mut state);
    assert!(
        release_signal_script
            .lock()
            .expect("release script")
            .is_empty()
    );
    assert_eq!(
        state.buffer_release_metrics().buffer_releases_completed,
        release_count_before + 1
    );
    state.release_client_buffers_for_shutdown();
    assert_eq!(
        state.buffer_release_metrics().buffer_releases_completed,
        release_count_before + 1,
        "later shutdown cleanup must not settle the same buffer twice"
    );
}

#[test]
fn destroying_buffer_cancels_only_its_surface_tree_and_acquire_watch() {
    let mut state = CompositorState::default();
    let (display, client, target_surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let unrelated_surface =
        state.test_create_unmapped_surface_resource_at_version(&client, &display_handle, 3);
    let unrelated_surface_id = compositor_surface_id(&unrelated_surface);
    state
        .surface_presentation_generations
        .insert(unrelated_surface_id, 1);
    let callback = client
        .create_resource::<wl_callback::WlCallback, (), CompositorState>(&display_handle, 4, ())
        .expect("frame callback resource");
    let callback_id = callback.id();
    let resize_interaction_id = ResizeInteractionId::new(91);
    let mut resize_flow = ResizeConfigureFlow::default();
    let resize_configure = PendingResizeConfigure {
        surface_id: target_surface_id,
        width: 64,
        height: 64,
        placement: SurfacePlacement::root_at(0, 0),
        edges: ResizeEdges::BOTTOM_RIGHT,
        resizing: true,
        interaction_id: resize_interaction_id,
    };
    assert!(resize_flow.mark_sent(resize_configure, 77, 1));
    assert_eq!(resize_flow.ack(77), ResizeAckDecision::Matched);
    let resize_snapshot = resize_flow
        .capture(10)
        .expect("acked resize capture for pending commit");
    state
        .resize_configure_flows
        .insert(target_surface_id, resize_flow);
    state
        .pending_frame_callback_surfaces
        .insert(callback_id.clone(), target_surface_id);
    let target_commit_id = SurfaceCommitId::for_tests(10);
    state.note_explicit_commit_captured(
        target_commit_id,
        target_surface_id,
        1,
        None,
        std::slice::from_ref(&callback),
    );
    let mut target_commit = test_mergeable_commit(10);
    let mut target_buffer =
        test_pending_shm_buffer(&mut state, &client, &display_handle, 5, 64, 64);
    target_buffer.commit_sequence = target_commit.commit_sequence;
    target_buffer.resize_commit = Some(Box::new(resize_snapshot));
    let target_buffer_resource = target_buffer.resource.clone();
    target_commit.attachment = Some(PendingSurfaceAttachment::Buffer(target_buffer));
    target_commit.frame_callbacks.push(callback);
    let mut target_nodes = vec![(target_surface_id, target_commit)];
    assert!(
        state
            .prepare_surface_tree_surface_state(target_surface_id, &mut target_nodes, &[])
            .is_ok()
    );
    let target_acquire_commit_id = AcquireCommitId::for_tests(1_010);
    let dependency = SurfaceTreeAcquireDependency {
        surface_commit_id: SurfaceCommitId::for_tests(10),
        commit_id: target_acquire_commit_id,
        surface_id: target_surface_id,
        owner_client_id: Some(client.id()),
        surface_presentation_generation: Some(1),
        buffer_id: target_buffer_resource.id().protocol_id(),
        acquire: ExplicitSyncPoint::for_tests_with_signal_script(20_010, 20_010, [false]),
        state: PendingAcquireState::EventfdBacked,
    };
    state.merge_or_queue_surface_tree_transaction(
        target_surface_id,
        target_nodes,
        vec![dependency],
        Vec::new(),
        SurfaceTreeSubmissionKind::ClientAdmission,
    );
    submit_test_unready_buffer_commit(
        &mut state,
        &client,
        &display_handle,
        unrelated_surface_id,
        6,
        11,
        None,
    );
    state.enable_external_acquire_readiness();
    let registrations = state.take_acquire_watch_changes();
    assert_eq!(
        registrations
            .iter()
            .filter(|change| matches!(change, AcquireWatchChange::Register(_)))
            .count(),
        2
    );

    let release_count_before = state.buffer_release_metrics().buffer_releases_completed;
    let resize_releases_before = state.resize_flow_metrics.resize_captures_released;

    state.cancel_pending_surface_trees_for_buffer(
        &target_buffer_resource,
        AcquireWatchCancelReason::BufferDestroyed,
    );

    assert_eq!(state.surface_transactions.pending_tree_count(), 1);
    assert!(
        state
            .surface_transactions
            .pending_trees()
            .all(|transaction| transaction.root_surface_id == unrelated_surface_id)
    );
    assert_eq!(
        state.buffer_release_metrics().buffer_releases_completed,
        release_count_before + 1
    );
    assert!(
        !state
            .pending_frame_callback_surfaces
            .contains_key(&callback_id)
    );
    assert_eq!(resize_releases_before, 0);
    assert_eq!(
        state.resize_flow_metrics.resize_captures_released,
        resize_releases_before + 1
    );
    assert_eq!(
        state
            .pending_acquire_watch_changes
            .iter()
            .filter(|change| {
                matches!(
                    change,
                    AcquireWatchChange::Cancel {
                        commit_id: cancelled,
                        reason: AcquireWatchCancelReason::BufferDestroyed,
                    } if *cancelled == target_acquire_commit_id
                )
            })
            .count(),
        1
    );

    state.cancel_pending_surface_trees_for_buffer(
        &target_buffer_resource,
        AcquireWatchCancelReason::BufferDestroyed,
    );
    assert_eq!(
        state.buffer_release_metrics().buffer_releases_completed,
        release_count_before + 1
    );
    assert!(
        !state
            .pending_frame_callback_surfaces
            .contains_key(&callback_id)
    );
    assert_eq!(
        state.resize_flow_metrics.resize_captures_released,
        resize_releases_before + 1
    );
}

#[test]
fn unprepared_tree_node_fails_closed_without_sync_readmission() {
    let mut state = CompositorState {
        external_acquire_readiness: true,
        ..CompositorState::default()
    };
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let surface = state
        .surface_resource_by_id(surface_id)
        .expect("test surface resource");
    let sync_state = std::sync::Arc::new(SyncobjSurfaceState::new(surface_id, surface.downgrade()));
    sync_state.set_pending_acquire(ExplicitSyncPoint::for_tests_with_signal_script(
        120,
        121,
        [false],
    ));
    sync_state.set_pending_release(ExplicitSyncPoint::for_tests(122, 123));

    let mut commit = test_mergeable_commit(1);
    commit.attachment = Some(PendingSurfaceAttachment::Buffer(test_pending_shm_buffer(
        &mut state,
        &client,
        &display_handle,
        2,
        64,
        64,
    )));
    commit.explicit_sync = Some(CapturedExplicitSyncState::capture(sync_state));
    let mut nodes = vec![(surface_id, commit)];
    state
        .prepare_surface_tree_surface_state(surface_id, &mut nodes, &[])
        .expect("mapping preflight");
    let transaction = PendingSurfaceTreeTransaction {
        id: SurfaceTreeTransactionId::new(1),
        root_surface_id: surface_id,
        nodes,
        publication_lifetimes: SurfaceTreeNodeLifetimes::Synthetic,
        dependencies: Vec::new(),
        external_content_update_dependencies: Vec::new(),
        commit_timing_readiness: None,
        received_at: Instant::now(),
    };
    let releases_before = state.buffer_release_metrics.buffer_releases_completed;

    let publication = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        state.publish_surface_tree_nodes(transaction);
    }));
    if cfg!(debug_assertions) {
        assert!(
            publication.is_err(),
            "debug builds should flag the invariant violation"
        );
    } else {
        assert!(publication.is_ok());
    }

    assert!(state.pending_acquire_watch_changes.is_empty());
    assert_eq!(
        state.buffer_release_metrics.buffer_releases_completed,
        releases_before + 1,
    );
    assert_eq!(state.compliance_metrics.protocol_errors_total, 0);
}
