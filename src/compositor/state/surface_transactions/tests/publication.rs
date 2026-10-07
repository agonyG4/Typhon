use super::*;

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
        assert!(state.pending_explicit_sync_commits.is_empty());
        assert!(state.pending_acquire_watch_changes.is_empty());
    }
}

#[test]
fn tree_publication_supersedes_same_surface_explicit_sync_with_callback_ownership() {
    let mut state = CompositorState {
        external_acquire_readiness: true,
        ..CompositorState::default()
    };
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let surface = state
        .surface_resource_by_id(surface_id)
        .expect("test surface resource");

    let old_callback = client
        .create_resource::<wl_callback::WlCallback, (), CompositorState>(&display_handle, 3, ())
        .expect("old frame callback");
    let tree_callback = client
        .create_resource::<wl_callback::WlCallback, (), CompositorState>(&display_handle, 4, ())
        .expect("tree frame callback");
    state
        .pending_frame_callback_surfaces
        .insert(old_callback.id(), surface_id);
    state
        .pending_frame_callback_surfaces
        .insert(tree_callback.id(), surface_id);

    let old_acquire = ExplicitSyncPoint::for_tests_with_signal_script(30, 31, [false]);
    let old_commit_id = AcquireCommitId::for_tests(40);
    let old_buffer = test_pending_shm_buffer(&mut state, &client, &display_handle, 2, 64, 64);
    let old_buffer_id = old_buffer.resource.id().protocol_id();
    let old_feedback = client
        .create_resource::<
            wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::WpPresentationFeedback,
            (),
            CompositorState,
        >(&display_handle, 6, ())
        .expect("old presentation feedback");
    let tree_feedback = client
        .create_resource::<
            wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::WpPresentationFeedback,
            (),
            CompositorState,
        >(&display_handle, 7, ())
        .expect("tree presentation feedback");
    state
        .pending_explicit_sync_commits
        .push(PendingExplicitSyncCommit {
            surface_commit_id: SurfaceCommitId::for_tests(1),
            commit_id: old_commit_id,
            surface_id,
            owner_client_id: client.id(),
            surface_presentation_generation: 1,
            commit_sequence: SurfaceCommitSequence(1),
            pending: old_buffer,
            damage: RenderableSurfaceDamage::full(),
            window_geometry: None,
            frame_callbacks: vec![old_callback.clone()],
            presentation_feedbacks: vec![PendingPresentationFeedback {
                surface_id,
                surface_presentation_generation: 1,
                commit_sequence: SurfaceCommitSequence(1),
                surface: surface.clone(),
                feedback: old_feedback.clone(),
            }],
            acquire: old_acquire.clone(),
            acquire_state: PendingAcquireState::RegistrationPending,
        });
    state
        .pending_acquire_watch_changes
        .push(AcquireWatchChange::Register(AcquireWatchRequest {
            commit_id: old_commit_id,
            surface_id,
            buffer_id: old_buffer_id,
            acquire: old_acquire,
            received_at: Instant::now(),
        }));

    let blocker = test_blocking_external_dependency(&mut state, &client, &display_handle, 5);
    let mut commit = test_mergeable_commit(2);
    let mut tree_buffer = test_pending_shm_buffer(&mut state, &client, &display_handle, 8, 64, 64);
    tree_buffer.commit_sequence = commit.commit_sequence;
    let tree_buffer_id = tree_buffer.data.buffer_id();
    commit.attachment = Some(PendingSurfaceAttachment::Buffer(tree_buffer));
    commit.frame_callbacks.push(tree_callback.clone());
    commit
        .presentation_feedbacks
        .push(PendingPresentationFeedback {
            surface_id,
            surface_presentation_generation: 1,
            commit_sequence: commit.commit_sequence,
            surface: surface.clone(),
            feedback: tree_feedback.clone(),
        });

    state.submit_surface_tree_nodes_with_kind(
        surface_id,
        vec![(surface_id, commit)],
        vec![blocker],
        SurfaceTreeSubmissionKind::ClientAdmission,
    );
    assert_eq!(state.pending_explicit_sync_commits.len(), 1);
    assert!(!acquire_watch_was_cancelled_as_superseded(
        &state,
        old_commit_id
    ));
    let pending_node = &state.surface_transactions.pending_surface_tree_transactions[0].nodes[0].1;
    assert_eq!(pending_node.frame_callbacks.len(), 1);
    assert_eq!(pending_node.frame_callbacks[0].id(), tree_callback.id());
    assert_eq!(pending_node.presentation_feedbacks.len(), 1);
    assert_eq!(
        pending_node.presentation_feedbacks[0].feedback.id(),
        tree_feedback.id()
    );
    assert_eq!(
        state.pending_explicit_sync_commits[0].frame_callbacks[0].id(),
        old_callback.id()
    );
    assert_eq!(
        state.pending_explicit_sync_commits[0].presentation_feedbacks[0]
            .feedback
            .id(),
        old_feedback.id()
    );

    release_blocking_dependency(&mut state, blocker);
    state.commit_ready_surface_tree_transactions();

    assert!(
        state
            .surface_transactions
            .pending_surface_tree_transactions
            .is_empty()
    );
    assert!(state.pending_explicit_sync_commits.is_empty());
    assert!(
        state
            .pending_acquire_watch_changes
            .iter()
            .any(|change| matches!(
                change,
                AcquireWatchChange::Cancel {
                    commit_id,
                    reason: AcquireWatchCancelReason::Superseded,
                } if *commit_id == old_commit_id
            ))
    );
    assert!(
        !state
            .pending_frame_callback_surfaces
            .contains_key(&old_callback.id())
    );
    assert!(
        !state
            .pending_frame_callback_surfaces
            .contains_key(&tree_callback.id())
    );
    assert!(state.pending_surface_presentation_feedbacks.is_empty());
    assert_eq!(
        state
            .surface_publications
            .get(&surface_id)
            .and_then(|publication| publication.latest_published_buffer_id),
        Some(tree_buffer_id)
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

    assert!(state.pending_explicit_sync_commits.is_empty());
    assert!(state.pending_acquire_watch_changes.is_empty());
    assert_eq!(
        state.buffer_release_metrics.buffer_releases_completed,
        releases_before + 1,
    );
    assert_eq!(state.compliance_metrics.protocol_errors_total, 0);
}
