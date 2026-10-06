use super::*;

#[test]
fn canceling_surface_retires_predecessor_and_later_projected_transaction_together() {
    let mut state = CompositorState::default();
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let mut first = test_mergeable_commit(260);
    first.viewport_destination.destination =
        Some(Some(BufferSize::new(50, 50).expect("destination")));
    let (first_ref, _blocker) = queue_blocked_pacing_predecessor(
        &mut state,
        &client,
        &display_handle,
        surface_id,
        2,
        first,
    );

    let mut second = test_mergeable_commit(261);
    second.lineage.predecessor = Some(first_ref);
    second.attachment = Some(PendingSurfaceAttachment::Buffer(test_pending_shm_buffer(
        &mut state,
        &client,
        &display_handle,
        3,
        100,
        80,
    )));
    state.submit_surface_tree_nodes_with_kind(
        surface_id,
        vec![(surface_id, second)],
        Vec::new(),
        SurfaceTreeSubmissionKind::ClientAdmission,
    );
    assert_eq!(
        state
            .surface_transactions
            .pending_surface_tree_transactions
            .len(),
        2
    );
    let pending = match state.surface_transactions.pending_surface_tree_transactions[1].nodes[0]
        .1
        .attachment
        .as_ref()
    {
        Some(PendingSurfaceAttachment::Buffer(buffer)) => buffer,
        other => panic!("expected prepared buffer, got {other:?}"),
    };
    assert_eq!(pending.surface_size, Some(BufferSize::new(50, 50).unwrap()));

    let release_before = state.buffer_release_metrics();
    state.cancel_pending_surface_trees_for_surface(
        surface_id,
        AcquireWatchCancelReason::SurfaceDestroyed,
    );

    assert!(
        state
            .surface_transactions
            .pending_surface_tree_transactions
            .is_empty()
    );
    assert!(!state.current_surface_buffers.contains_key(&surface_id));
    assert!(state.renderable_surface(surface_id).is_none());
    let release_after = state.buffer_release_metrics();
    assert_eq!(
        release_after.buffer_releases_completed,
        release_before.buffer_releases_completed + 1
    );
}

#[test]
fn root_cancellation_discards_pending_dependents_after_topology_churn() {
    let mut state = CompositorState::default();
    let (_display, _client, surface_id) = test_surface_and_client(&mut state);
    let first = test_mergeable_commit(270);
    let first_ref = first.content_update_ref(surface_id);
    let mut second = test_mergeable_commit(271);
    second.lineage.predecessor = Some(first_ref);

    state
        .surface_transactions
        .pending_surface_tree_transactions
        .extend([
            PendingSurfaceTreeTransaction {
                id: SurfaceTreeTransactionId::new(1),
                root_surface_id: 700,
                nodes: vec![(surface_id, first)],
                publication_lifetimes: SurfaceTreeNodeLifetimes::Synthetic,
                dependencies: Vec::new(),
                external_content_update_dependencies: Vec::new(),
                commit_timing_readiness: None,
                received_at: Instant::now(),
            },
            PendingSurfaceTreeTransaction {
                id: SurfaceTreeTransactionId::new(2),
                root_surface_id: 701,
                nodes: vec![(surface_id, second)],
                publication_lifetimes: SurfaceTreeNodeLifetimes::Synthetic,
                dependencies: Vec::new(),
                external_content_update_dependencies: vec![first_ref],
                commit_timing_readiness: None,
                received_at: Instant::now(),
            },
        ]);
    state.cancel_pending_surface_trees_for_root(700, AcquireWatchCancelReason::SurfaceDestroyed);

    assert!(
        state
            .surface_transactions
            .pending_surface_tree_transactions
            .is_empty()
    );
}
