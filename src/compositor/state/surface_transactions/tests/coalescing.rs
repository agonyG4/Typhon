use super::*;

#[test]
fn unready_root_head_survives_newer_unready_attachment() {
    let mut state = CompositorState {
        external_acquire_readiness: true,
        ..CompositorState::default()
    };
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let (anchor_ref, anchor_acquire, anchor_buffer, _) = submit_test_unready_buffer_commit(
        &mut state,
        &client,
        &display_handle,
        surface_id,
        2,
        1,
        None,
    );

    let (successor_ref, successor_acquire, successor_buffer, _) = submit_test_unready_buffer_commit(
        &mut state,
        &client,
        &display_handle,
        surface_id,
        3,
        2,
        Some(anchor_ref),
    );

    assert_eq!(
        state
            .surface_transactions
            .pending_surface_tree_transactions
            .len(),
        2
    );
    let anchor = &state.surface_transactions.pending_surface_tree_transactions[0];
    assert_eq!(anchor.dependencies.len(), 1);
    assert_eq!(
        anchor.dependencies[0].surface_commit_id,
        anchor_ref.commit_id
    );
    assert_eq!(anchor.dependencies[0].commit_id, anchor_acquire);
    assert_eq!(anchor.dependencies[0].buffer_id, anchor_buffer);
    assert_eq!(anchor.nodes[0].1.commit_id, anchor_ref.commit_id);
    let Some(PendingSurfaceAttachment::Buffer(anchor_pending)) =
        anchor.nodes[0].1.attachment.as_ref()
    else {
        panic!("progress anchor must retain its pending buffer");
    };
    assert_eq!(anchor_pending.resource.id().protocol_id(), anchor_buffer);

    let successor = &state.surface_transactions.pending_surface_tree_transactions[1];
    assert_eq!(successor.dependencies.len(), 1);
    assert_eq!(
        successor.dependencies[0].surface_commit_id,
        successor_ref.commit_id
    );
    assert_eq!(successor.dependencies[0].commit_id, successor_acquire);
    assert_eq!(successor.dependencies[0].buffer_id, successor_buffer);
    assert_eq!(successor.nodes[0].1.commit_id, successor_ref.commit_id);
    assert_eq!(state.buffer_release_metrics.buffer_releases_completed, 0);
    assert!(!acquire_watch_was_cancelled_as_superseded(
        &state,
        anchor_acquire
    ));
    assert!(state.pending_acquire_watch_changes.iter().any(|change| {
        matches!(change, AcquireWatchChange::Register(watch) if watch.commit_id == anchor_acquire)
    }));
}

#[test]
fn newer_unready_commits_coalesce_into_successor_not_progress_anchor() {
    let mut state = CompositorState {
        external_acquire_readiness: true,
        ..CompositorState::default()
    };
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let (anchor_ref, anchor_acquire, anchor_buffer, _) = submit_test_unready_buffer_commit(
        &mut state,
        &client,
        &display_handle,
        surface_id,
        2,
        1,
        None,
    );
    let (tail_ref, tail_acquire, _tail_buffer, _) = submit_test_unready_buffer_commit(
        &mut state,
        &client,
        &display_handle,
        surface_id,
        3,
        2,
        Some(anchor_ref),
    );

    let (latest_ref, latest_acquire, latest_buffer, _) = submit_test_unready_buffer_commit(
        &mut state,
        &client,
        &display_handle,
        surface_id,
        4,
        3,
        Some(tail_ref),
    );

    assert_eq!(
        state
            .surface_transactions
            .pending_surface_tree_transactions
            .len(),
        2
    );
    let anchor = &state.surface_transactions.pending_surface_tree_transactions[0];
    assert_eq!(
        anchor.dependencies[0].surface_commit_id,
        anchor_ref.commit_id
    );
    assert_eq!(anchor.dependencies[0].commit_id, anchor_acquire);
    assert_eq!(anchor.dependencies[0].buffer_id, anchor_buffer);
    assert_eq!(anchor.nodes[0].1.commit_id, anchor_ref.commit_id);
    let Some(PendingSurfaceAttachment::Buffer(anchor_pending)) =
        anchor.nodes[0].1.attachment.as_ref()
    else {
        panic!("progress anchor must retain its pending buffer");
    };
    assert_eq!(anchor_pending.resource.id().protocol_id(), anchor_buffer);

    let tail = &state.surface_transactions.pending_surface_tree_transactions[1];
    assert_eq!(tail.dependencies.len(), 1);
    assert_eq!(tail.dependencies[0].surface_commit_id, latest_ref.commit_id);
    assert_eq!(tail.dependencies[0].commit_id, latest_acquire);
    assert_eq!(tail.dependencies[0].buffer_id, latest_buffer);
    assert_eq!(tail.nodes[0].1.commit_id, latest_ref.commit_id);
    assert!(!acquire_watch_was_cancelled_as_superseded(
        &state,
        anchor_acquire
    ));
    assert!(acquire_watch_was_cancelled_as_superseded(
        &state,
        tail_acquire
    ));
    assert_eq!(state.buffer_release_metrics.buffer_releases_completed, 1);
    assert_eq!(
        state
            .surface_transactions
            .metrics
            .unready_progress_anchors_preserved_from_newer_unready,
        1
    );
}

#[test]
fn unresolved_single_surface_stream_keeps_only_anchor_and_latest_successor() {
    const LAST_SEQUENCE: u64 = 400;
    let mut state = CompositorState {
        external_acquire_readiness: true,
        ..CompositorState::default()
    };
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let (anchor_ref, anchor_acquire, anchor_buffer, _) = submit_test_unready_buffer_commit(
        &mut state,
        &client,
        &display_handle,
        surface_id,
        2,
        1,
        None,
    );
    let mut predecessor = anchor_ref;
    let mut latest_acquire = anchor_acquire;
    let mut latest_buffer = anchor_buffer;
    for sequence in 2..=LAST_SEQUENCE {
        let (reference, acquire, buffer, _) = submit_test_unready_buffer_commit(
            &mut state,
            &client,
            &display_handle,
            surface_id,
            u32::try_from(sequence + 1).expect("test buffer resource id"),
            sequence,
            Some(predecessor),
        );
        predecessor = reference;
        latest_acquire = acquire;
        latest_buffer = buffer;
        assert_eq!(
            state
                .surface_transactions
                .pending_surface_tree_transactions
                .len(),
            2
        );
        assert_eq!(
            state.surface_transactions.pending_surface_tree_transactions[0].dependencies[0]
                .commit_id,
            anchor_acquire
        );
        assert_eq!(
            state.surface_transactions.pending_surface_tree_transactions[0].nodes[0]
                .1
                .commit_id,
            anchor_ref.commit_id
        );
        assert!(!acquire_watch_was_cancelled_as_superseded(
            &state,
            anchor_acquire
        ));
    }

    assert_eq!(
        state
            .surface_transactions
            .pending_surface_tree_transactions
            .len(),
        2
    );
    let anchor = &state.surface_transactions.pending_surface_tree_transactions[0];
    assert_eq!(anchor.dependencies.len(), 1);
    assert_eq!(
        anchor.dependencies[0].surface_commit_id,
        anchor_ref.commit_id
    );
    assert_eq!(anchor.dependencies[0].commit_id, anchor_acquire);
    assert_eq!(anchor.dependencies[0].buffer_id, anchor_buffer);
    assert_eq!(anchor.nodes[0].1.commit_id, anchor_ref.commit_id);
    let Some(PendingSurfaceAttachment::Buffer(anchor_pending)) =
        anchor.nodes[0].1.attachment.as_ref()
    else {
        panic!("progress anchor must retain its pending buffer");
    };
    assert_eq!(anchor_pending.resource.id().protocol_id(), anchor_buffer);

    let tail = &state.surface_transactions.pending_surface_tree_transactions[1];
    assert_eq!(tail.dependencies.len(), 1);
    assert_eq!(
        tail.dependencies[0].surface_commit_id,
        predecessor.commit_id
    );
    assert_eq!(tail.dependencies[0].commit_id, latest_acquire);
    assert_eq!(tail.dependencies[0].buffer_id, latest_buffer);
    assert_eq!(tail.nodes[0].1.commit_id, predecessor.commit_id);
    assert_eq!(
        tail.nodes[0].1.commit_sequence,
        SurfaceCommitSequence(LAST_SEQUENCE)
    );
    assert_eq!(
        state
            .surface_transactions
            .metrics
            .explicit_sync_queue_overflow,
        0
    );
    assert_eq!(
        state
            .surface_pacing_metrics
            .queue_admission_resource_exhaustion,
        0
    );
    assert_eq!(
        state
            .surface_transactions
            .metrics
            .maximum_waiting_slots_per_root,
        2
    );
    assert_eq!(
        state
            .surface_transactions
            .metrics
            .maximum_explicit_sync_queue_depth,
        2
    );
    assert_eq!(
        state
            .surface_transactions
            .metrics
            .unready_progress_anchors_preserved_from_newer_unready,
        1
    );
    assert_eq!(state.buffer_release_metrics.buffer_releases_completed, 398);
    assert!(!acquire_watch_was_cancelled_as_superseded(
        &state,
        anchor_acquire
    ));
    assert!(!acquire_watch_was_cancelled_as_superseded(
        &state,
        latest_acquire
    ));
}

#[test]
fn progress_anchor_publishes_before_its_latest_successor() {
    let mut state = CompositorState {
        external_acquire_readiness: true,
        ..CompositorState::default()
    };
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let (anchor_ref, anchor_acquire, _anchor_buffer, anchor_buffer_identity) =
        submit_test_unready_buffer_commit(
            &mut state,
            &client,
            &display_handle,
            surface_id,
            2,
            1,
            None,
        );
    let (tail_ref, tail_acquire, _tail_buffer, _) = submit_test_unready_buffer_commit(
        &mut state,
        &client,
        &display_handle,
        surface_id,
        3,
        2,
        Some(anchor_ref),
    );
    let (latest_ref, latest_acquire, _latest_buffer, latest_buffer_identity) =
        submit_test_unready_buffer_commit(
            &mut state,
            &client,
            &display_handle,
            surface_id,
            4,
            3,
            Some(tail_ref),
        );

    state.surface_transactions.pending_surface_tree_transactions[0].dependencies[0].state =
        PendingAcquireState::Ready;
    state.commit_ready_surface_tree_transactions();

    assert_eq!(
        state.surface_publications[&surface_id].latest_published,
        Some(anchor_ref.commit_sequence)
    );
    assert_eq!(
        state.current_surface_buffers[&surface_id].buffer_id(),
        anchor_buffer_identity
    );
    assert_eq!(
        state
            .surface_transactions
            .pending_surface_tree_transactions
            .len(),
        1
    );
    assert_eq!(
        state.surface_transactions.pending_surface_tree_transactions[0].dependencies[0].commit_id,
        latest_acquire
    );
    assert_eq!(
        state.surface_transactions.pending_surface_tree_transactions[0].dependencies[0]
            .surface_commit_id,
        latest_ref.commit_id
    );
    assert!(!acquire_watch_was_cancelled_as_superseded(
        &state,
        anchor_acquire
    ));

    state.surface_transactions.pending_surface_tree_transactions[0].dependencies[0].state =
        PendingAcquireState::Ready;
    state.commit_ready_surface_tree_transactions();

    assert!(
        state
            .surface_transactions
            .pending_surface_tree_transactions
            .is_empty()
    );
    assert_eq!(
        state.surface_publications[&surface_id].latest_published,
        Some(latest_ref.commit_sequence)
    );
    assert_eq!(
        state.current_surface_buffers[&surface_id].buffer_id(),
        latest_buffer_identity
    );
    assert!(acquire_watch_was_cancelled_as_superseded(
        &state,
        tail_acquire
    ));
}

#[test]
fn ready_attachment_replacement_keeps_existing_immediate_behavior() {
    let mut state = CompositorState {
        external_acquire_readiness: true,
        ..CompositorState::default()
    };
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let (anchor_ref, anchor_acquire, _, _) = submit_test_unready_buffer_commit(
        &mut state,
        &client,
        &display_handle,
        surface_id,
        2,
        1,
        None,
    );
    let ready_ref = submit_test_ready_buffer_commit(
        &mut state,
        &client,
        &display_handle,
        surface_id,
        3,
        2,
        anchor_ref,
    );

    assert!(
        state
            .surface_transactions
            .pending_surface_tree_transactions
            .is_empty()
    );
    assert_eq!(
        state.surface_publications[&surface_id].latest_published,
        Some(ready_ref.commit_sequence)
    );
    assert!(acquire_watch_was_cancelled_as_superseded(
        &state,
        anchor_acquire
    ));
    assert_eq!(
        state
            .surface_transactions
            .metrics
            .unready_progress_anchors_preserved_from_newer_unready,
        0
    );
}

#[test]
fn explicit_detach_replaces_unready_anchor_without_waiting_for_its_acquire() {
    let mut state = CompositorState {
        external_acquire_readiness: true,
        ..CompositorState::default()
    };
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let (anchor_ref, anchor_acquire, _, _) = submit_test_unready_buffer_commit(
        &mut state,
        &client,
        &display_handle,
        surface_id,
        2,
        1,
        None,
    );
    let mut detach = test_mergeable_commit(2);
    detach.lineage.predecessor = Some(anchor_ref);
    detach.attachment = Some(PendingSurfaceAttachment::RemoveContent);
    let mut nodes = vec![(surface_id, detach)];
    assert!(
        state
            .prepare_surface_tree_surface_state(surface_id, &mut nodes, &[])
            .is_ok()
    );
    state.merge_or_queue_surface_tree_transaction(
        surface_id,
        nodes,
        Vec::new(),
        Vec::new(),
        SurfaceTreeSubmissionKind::ClientAdmission,
    );

    assert!(
        state
            .surface_transactions
            .pending_surface_tree_transactions
            .is_empty()
    );
    assert_eq!(
        state.surface_publications[&surface_id].latest_published,
        Some(SurfaceCommitSequence(2))
    );
    assert!(acquire_watch_was_cancelled_as_superseded(
        &state,
        anchor_acquire
    ));
    assert_eq!(state.buffer_release_metrics.buffer_releases_completed, 1);
    assert_eq!(state.surface_transactions.metrics.explicit_detaches, 1);
    assert_eq!(
        state
            .surface_transactions
            .metrics
            .unready_progress_anchors_preserved_from_newer_unready,
        0
    );
}

#[test]
fn metadata_only_commit_still_coalesces_with_unready_anchor() {
    let mut state = CompositorState {
        external_acquire_readiness: true,
        ..CompositorState::default()
    };
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let (anchor_ref, anchor_acquire, anchor_buffer, _) = submit_test_unready_buffer_commit(
        &mut state,
        &client,
        &display_handle,
        surface_id,
        2,
        1,
        None,
    );
    let mut metadata = test_mergeable_commit(2);
    metadata.lineage.predecessor = Some(anchor_ref);
    metadata.offset = Some((4, 7));
    let metadata_commit_id = metadata.commit_id;
    state.merge_or_queue_surface_tree_transaction(
        surface_id,
        vec![(surface_id, metadata)],
        Vec::new(),
        Vec::new(),
        SurfaceTreeSubmissionKind::ClientAdmission,
    );

    assert_eq!(
        state
            .surface_transactions
            .pending_surface_tree_transactions
            .len(),
        1
    );
    let anchor = &state.surface_transactions.pending_surface_tree_transactions[0];
    assert_eq!(anchor.dependencies.len(), 1);
    assert_eq!(anchor.dependencies[0].commit_id, anchor_acquire);
    assert_eq!(anchor.dependencies[0].buffer_id, anchor_buffer);
    assert_eq!(anchor.nodes[0].1.commit_id, metadata_commit_id);
    assert_eq!(anchor.nodes[0].1.offset, Some((4, 7)));
    assert_eq!(state.buffer_release_metrics.buffer_releases_completed, 0);
    assert_eq!(
        state
            .surface_transactions
            .metrics
            .unready_progress_anchors_preserved_from_newer_unready,
        0
    );
}

#[test]
fn pacing_protected_heads_keep_fifo_timing_and_merge_frozen_boundaries() {
    #[derive(Clone, Copy)]
    enum Boundary {
        Fifo,
        CommitTiming,
        MergeFrozen,
    }

    for boundary in [
        Boundary::Fifo,
        Boundary::CommitTiming,
        Boundary::MergeFrozen,
    ] {
        let mut state = CompositorState {
            external_acquire_readiness: true,
            ..CompositorState::default()
        };
        let (display, client, surface_id) = test_surface_and_client(&mut state);
        let display_handle = display.handle();
        let (anchor_ref, anchor_acquire, _, _) = submit_test_unready_buffer_commit(
            &mut state,
            &client,
            &display_handle,
            surface_id,
            2,
            1,
            None,
        );
        let anchor =
            &mut state.surface_transactions.pending_surface_tree_transactions[0].nodes[0].1;
        match boundary {
            Boundary::Fifo => anchor.pacing.fifo_set_barrier = true,
            Boundary::CommitTiming => {
                let now = client_pacing_now_ns();
                let seconds = now / 1_000_000_000 + 60;
                anchor.pacing.commit_timing = Some(
                    CommitTimingConstraint::from_protocol(seconds, (now % 1_000_000_000) as u32)
                        .expect("future commit timing constraint"),
                );
            }
            Boundary::MergeFrozen => anchor.lineage.merge_frozen = true,
        }

        let (successor_ref, _, _, _) = submit_test_unready_buffer_commit(
            &mut state,
            &client,
            &display_handle,
            surface_id,
            3,
            2,
            Some(anchor_ref),
        );

        assert_eq!(
            state
                .surface_transactions
                .pending_surface_tree_transactions
                .len(),
            2
        );
        assert_eq!(
            state.surface_transactions.pending_surface_tree_transactions[0].nodes[0]
                .1
                .commit_id,
            anchor_ref.commit_id
        );
        assert_eq!(
            state.surface_transactions.pending_surface_tree_transactions[0].dependencies[0]
                .commit_id,
            anchor_acquire
        );
        assert_eq!(
            state.surface_transactions.pending_surface_tree_transactions[1].nodes[0]
                .1
                .commit_id,
            successor_ref.commit_id
        );
        assert!(!acquire_watch_was_cancelled_as_superseded(
            &state,
            anchor_acquire
        ));
        assert_eq!(
            state
                .surface_transactions
                .metrics
                .unready_progress_anchors_preserved_from_newer_unready,
            0
        );
    }
}

#[test]
fn anchor_callbacks_stay_owned_and_tail_feedback_is_discarded_when_coalesced() {
    let mut state = CompositorState {
        external_acquire_readiness: true,
        ..CompositorState::default()
    };
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let surface = state
        .surface_resource_by_id(surface_id)
        .expect("test surface resource")
        .clone();
    let (anchor_ref, anchor_acquire, _, _) = submit_test_unready_buffer_commit(
        &mut state,
        &client,
        &display_handle,
        surface_id,
        2,
        1,
        None,
    );
    let (tail_ref, tail_acquire, _, _) = submit_test_unready_buffer_commit(
        &mut state,
        &client,
        &display_handle,
        surface_id,
        3,
        2,
        Some(anchor_ref),
    );
    let anchor_callback = client
        .create_resource::<wl_callback::WlCallback, (), CompositorState>(&display_handle, 20, ())
        .expect("anchor frame callback");
    let tail_callback = client
        .create_resource::<wl_callback::WlCallback, (), CompositorState>(&display_handle, 21, ())
        .expect("tail frame callback");
    let anchor_feedback = client
            .create_resource::<
                wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::WpPresentationFeedback,
                (),
                CompositorState,
            >(&display_handle, 30, ())
            .expect("anchor presentation feedback");
    let tail_feedback = client
            .create_resource::<
                wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::WpPresentationFeedback,
                (),
                CompositorState,
            >(&display_handle, 31, ())
            .expect("tail presentation feedback");
    let latest_callback = client
        .create_resource::<wl_callback::WlCallback, (), CompositorState>(&display_handle, 22, ())
        .expect("latest frame callback");
    let latest_feedback = client
            .create_resource::<
                wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::WpPresentationFeedback,
                (),
                CompositorState,
            >(&display_handle, 32, ())
            .expect("latest presentation feedback");
    state.surface_transactions.pending_surface_tree_transactions[0].nodes[0]
        .1
        .frame_callbacks
        .push(anchor_callback.clone());
    state.surface_transactions.pending_surface_tree_transactions[0].nodes[0]
        .1
        .presentation_feedbacks
        .push(PendingPresentationFeedback {
            surface_id,
            surface_presentation_generation: 1,
            commit_sequence: anchor_ref.commit_sequence,
            surface: surface.clone(),
            feedback: anchor_feedback.clone(),
        });
    state.surface_transactions.pending_surface_tree_transactions[1].nodes[0]
        .1
        .frame_callbacks
        .push(tail_callback.clone());
    state.surface_transactions.pending_surface_tree_transactions[1].nodes[0]
        .1
        .presentation_feedbacks
        .push(PendingPresentationFeedback {
            surface_id,
            surface_presentation_generation: 1,
            commit_sequence: tail_ref.commit_sequence,
            surface: surface.clone(),
            feedback: tail_feedback.clone(),
        });

    let (latest_ref, _, _, _) = submit_test_unready_buffer_commit_with_obligations(
        &mut state,
        &client,
        &display_handle,
        surface_id,
        4,
        3,
        Some(tail_ref),
        Some(latest_callback.clone()),
        Some(PendingPresentationFeedback {
            surface_id,
            surface_presentation_generation: 1,
            commit_sequence: SurfaceCommitSequence(3),
            surface: surface.clone(),
            feedback: latest_feedback.clone(),
        }),
    );

    let anchor = &state.surface_transactions.pending_surface_tree_transactions[0].nodes[0].1;
    assert_eq!(anchor.commit_id, anchor_ref.commit_id);
    assert_eq!(anchor.frame_callbacks.len(), 1);
    assert_eq!(
        anchor.frame_callbacks[0].id().protocol_id(),
        anchor_callback.id().protocol_id()
    );
    assert_eq!(anchor.presentation_feedbacks.len(), 1);
    assert_eq!(
        anchor.presentation_feedbacks[0].feedback.id().protocol_id(),
        anchor_feedback.id().protocol_id()
    );

    let tail = &state.surface_transactions.pending_surface_tree_transactions[1].nodes[0].1;
    assert_eq!(tail.commit_id, latest_ref.commit_id);
    assert_eq!(tail.frame_callbacks.len(), 2);
    assert_eq!(
        tail.frame_callbacks[0].id().protocol_id(),
        tail_callback.id().protocol_id()
    );
    assert_eq!(
        tail.frame_callbacks[1].id().protocol_id(),
        latest_callback.id().protocol_id()
    );
    assert_eq!(tail.presentation_feedbacks.len(), 1);
    assert_eq!(
        tail.presentation_feedbacks[0].feedback.id().protocol_id(),
        latest_feedback.id().protocol_id()
    );
    assert!(!tail.presentation_feedbacks.iter().any(|feedback| {
        feedback.feedback.id().protocol_id() == tail_feedback.id().protocol_id()
    }));
    assert_eq!(state.surface_transactions.metrics.callbacks_merged, 1);
    assert_eq!(state.surface_transactions.metrics.feedbacks_merged, 1);
    assert!(!acquire_watch_was_cancelled_as_superseded(
        &state,
        anchor_acquire
    ));
    assert!(acquire_watch_was_cancelled_as_superseded(
        &state,
        tail_acquire
    ));
}

#[test]
fn pending_coalescing_rejects_discontinuous_lineage_without_mutating_target() {
    let mut state = CompositorState {
        external_acquire_readiness: true,
        ..CompositorState::default()
    };
    let (display, client, parent_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let child_surface =
        state.test_create_unmapped_surface_resource_at_version(&client, &display_handle, 1);
    let child_id = compositor_surface_id(&child_surface);
    state.surface_presentation_generations.insert(child_id, 1);

    let acquire = SurfaceTreeAcquireDependency {
        surface_commit_id: SurfaceCommitId::for_tests(100),
        commit_id: AcquireCommitId::for_tests(101),
        surface_id: child_id,
        owner_client_id: Some(client.id()),
        surface_presentation_generation: Some(1),
        buffer_id: 308,
        acquire: ExplicitSyncPoint::for_tests_with_signal_script(102, 103, [false]),
        state: PendingAcquireState::EventfdBacked,
    };
    let a = test_mergeable_commit(1);
    let a_ref = a.content_update_ref(child_id);
    let mut p1 = test_mergeable_commit(2);
    let p1_ref = p1.content_update_ref(parent_id);
    p1.lineage.child_dependencies = vec![a_ref];
    state.queue_waiting_surface_tree(
        parent_id,
        vec![(child_id, a), (parent_id, p1)],
        vec![acquire],
    );

    let mut b = test_mergeable_commit(3);
    b.lineage.predecessor = Some(a_ref);
    let b_ref = b.content_update_ref(child_id);
    state.queue_waiting_surface_tree_with_lifetimes(
        child_id,
        vec![(child_id, b)],
        test_captured_lifetimes(&client.id(), &[child_id]),
        Vec::new(),
        vec![a_ref],
        SurfaceTreeSubmissionKind::ClientAdmission,
    );

    let mut d = test_mergeable_commit(4);
    d.lineage.predecessor = Some(b_ref);
    let mut p2 = test_mergeable_commit(5);
    p2.lineage.predecessor = Some(p1_ref);
    p2.lineage.child_dependencies = vec![d.content_update_ref(child_id)];
    state.merge_or_queue_surface_tree_transaction(
        parent_id,
        vec![(child_id, d), (parent_id, p2)],
        Vec::new(),
        vec![b_ref],
        SurfaceTreeSubmissionKind::ClientAdmission,
    );

    let parent_transactions = state
        .surface_transactions
        .pending_surface_tree_transactions
        .iter()
        .filter(|transaction| transaction.root_surface_id == parent_id)
        .collect::<Vec<_>>();
    assert_eq!(parent_transactions.len(), 2);
    assert!(parent_transactions.iter().any(|transaction| {
        transaction.nodes.iter().any(|(surface_id, commit)| {
            *surface_id == child_id && commit.commit_sequence == SurfaceCommitSequence(1)
        }) && transaction.dependencies.len() == 1
    }));
    let incoming = parent_transactions
        .iter()
        .find(|transaction| {
            transaction.nodes.iter().any(|(surface_id, commit)| {
                *surface_id == child_id && commit.commit_sequence == SurfaceCommitSequence(4)
            })
        })
        .expect("discontinuous incoming transaction remains separate");
    assert_eq!(
        incoming
            .nodes
            .iter()
            .map(|(_, commit)| commit.commit_sequence)
            .collect::<Vec<_>>(),
        vec![SurfaceCommitSequence(4), SurfaceCommitSequence(5)]
    );
    assert_eq!(incoming.external_content_update_dependencies, vec![b_ref]);
    assert!(!transaction_covers_content_update_ref(incoming, b_ref));
    assert!(!state.content_update_dependencies_ready(incoming));
    assert_eq!(state.pending_acquire_watch_changes.len(), 1);

    let target = state
        .surface_transactions
        .pending_surface_tree_transactions
        .iter_mut()
        .find(|transaction| {
            transaction.root_surface_id == parent_id
                && transaction
                    .nodes
                    .iter()
                    .any(|(_, commit)| commit.commit_sequence == SurfaceCommitSequence(1))
        })
        .expect("original target transaction");
    target.dependencies[0].state = PendingAcquireState::Ready;
    state.commit_ready_surface_tree_transactions();

    assert_eq!(
        state.surface_publications[&child_id].latest_published,
        Some(SurfaceCommitSequence(4))
    );
    assert!(
        state
            .surface_transactions
            .pending_surface_tree_transactions
            .is_empty()
    );
}

#[test]
fn pending_coalescing_requires_contiguous_lineage_for_same_surface_replacement() {
    let a = test_mergeable_commit(10);
    let a_ref = a.content_update_ref(2);
    let mut b = test_mergeable_commit(11);
    b.lineage.predecessor = Some(a_ref);
    let b_ref = b.content_update_ref(2);

    let mut target_a = PendingSurfaceTreeTransaction {
        id: SurfaceTreeTransactionId::new(20),
        root_surface_id: 2,
        nodes: vec![(2, a)],
        publication_lifetimes: SurfaceTreeNodeLifetimes::Synthetic,
        dependencies: Vec::new(),
        external_content_update_dependencies: Vec::new(),
        commit_timing_readiness: None,
        received_at: Instant::now(),
    };
    let mut incoming_b = test_mergeable_commit(11);
    incoming_b.lineage.predecessor = Some(a_ref);
    assert!(can_coalesce_pending_surface_tree_transaction(
        &target_a,
        &[(2, incoming_b)]
    ));

    let mut merged_ab = test_mergeable_commit(10);
    let _ = merged_ab.merge(b);
    let mut incoming_c = test_mergeable_commit(12);
    incoming_c.lineage.predecessor = Some(b_ref);
    target_a.nodes[0].1 = merged_ab;
    assert!(can_coalesce_pending_surface_tree_transaction(
        &target_a,
        &[(2, incoming_c)]
    ));

    let mut incoming_d = test_mergeable_commit(12);
    incoming_d.lineage.predecessor = Some(b_ref);
    target_a.nodes[0].1 = test_mergeable_commit(10);
    assert!(!can_coalesce_pending_surface_tree_transaction(
        &target_a,
        &[(2, incoming_d)]
    ));
}

#[test]
fn coalescing_replaces_every_node_at_the_incoming_dag_position() {
    let mut state = CompositorState::default();
    let (_display, client, _surface_id) = test_surface_and_client(&mut state);
    let g1 = test_mergeable_commit(1);
    let g1_ref = g1.content_update_ref(10);
    let c1 = test_mergeable_commit(2);
    let c1_ref = c1.content_update_ref(11);
    let p1 = test_mergeable_commit(3);
    let p1_ref = p1.content_update_ref(12);
    let mut g2 = test_mergeable_commit(4);
    g2.lineage.predecessor = Some(g1_ref);
    let g2_ref = g2.content_update_ref(10);
    let mut c2 = test_mergeable_commit(5);
    c2.lineage.predecessor = Some(c1_ref);
    c2.lineage.child_dependencies = vec![g2_ref];
    let mut p2 = test_mergeable_commit(6);
    p2.lineage.predecessor = Some(p1_ref);
    p2.lineage.child_dependencies = vec![c2.content_update_ref(11)];

    let mut transaction = PendingSurfaceTreeTransaction {
        id: SurfaceTreeTransactionId::new(12),
        root_surface_id: 12,
        nodes: vec![(10, g1), (11, c1), (12, p1)],
        publication_lifetimes: test_captured_lifetimes(&client.id(), &[10, 11, 12]),
        dependencies: Vec::new(),
        external_content_update_dependencies: Vec::new(),
        commit_timing_readiness: None,
        received_at: Instant::now(),
    };
    state.merge_surface_tree_nodes_into_transaction(
        12,
        &mut transaction,
        vec![(10, g2), (11, c2), (12, p2)],
        test_captured_lifetimes(&client.id(), &[10, 11, 12]),
        Vec::new(),
        vec![c1_ref],
    );

    assert_eq!(
        transaction
            .nodes
            .iter()
            .map(|(_, commit)| commit.commit_sequence)
            .collect::<Vec<_>>(),
        vec![
            SurfaceCommitSequence(4),
            SurfaceCommitSequence(5),
            SurfaceCommitSequence(6)
        ]
    );
    assert_eq!(
        transaction
            .publication_lifetimes
            .captured()
            .expect("captured lifetimes")
            .iter()
            .map(|lifetime| lifetime.surface_id)
            .collect::<Vec<_>>(),
        vec![10, 11, 12]
    );
    assert!(transaction.external_content_update_dependencies.is_empty());
}

#[test]
fn repeated_bufferless_updates_continue_coalescing_into_one_waiting_transaction() {
    let mut state = CompositorState::default();
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let blocking_surface =
        state.test_create_unmapped_surface_resource_at_version(&client, &display_handle, 1);
    let blocking_surface_id = compositor_surface_id(&blocking_surface);
    state
        .surface_presentation_generations
        .insert(blocking_surface_id, 1);
    let external_dependency = ContentUpdateRef {
        surface_id: blocking_surface_id,
        commit_id: SurfaceCommitId::for_tests(10_000),
        commit_sequence: SurfaceCommitSequence(10_000),
    };
    let first = test_mergeable_commit(1);
    state
        .surface_transactions
        .pending_surface_tree_transactions
        .push(PendingSurfaceTreeTransaction {
            id: SurfaceTreeTransactionId::new(13),
            root_surface_id: surface_id,
            nodes: vec![(surface_id, first)],
            publication_lifetimes: test_captured_lifetimes(&client.id(), &[surface_id]),
            dependencies: Vec::new(),
            external_content_update_dependencies: vec![external_dependency],
            commit_timing_readiness: None,
            received_at: Instant::now(),
        });

    for sequence in 2..=65 {
        let predecessor = ContentUpdateRef {
            surface_id,
            commit_id: SurfaceCommitId::for_tests(sequence - 1),
            commit_sequence: SurfaceCommitSequence(sequence - 1),
        };
        let mut commit = test_mergeable_commit(sequence);
        commit.lineage.predecessor = Some(predecessor);
        state.merge_or_queue_surface_tree_transaction(
            surface_id,
            vec![(surface_id, commit)],
            Vec::new(),
            vec![predecessor],
            SurfaceTreeSubmissionKind::ClientAdmission,
        );
    }

    assert_eq!(
        state
            .surface_transactions
            .pending_surface_tree_transactions
            .len(),
        1
    );
    assert_eq!(
        state.surface_transactions.pending_surface_tree_transactions[0].nodes[0]
            .1
            .commit_sequence,
        SurfaceCommitSequence(65)
    );
    assert_eq!(
        state.surface_transactions.pending_surface_tree_transactions[0]
            .external_content_update_dependencies,
        vec![external_dependency]
    );
}
