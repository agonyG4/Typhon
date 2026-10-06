use super::*;

fn test_mergeable_commit(sequence: u64) -> CachedSubsurfaceCommit {
    let mut commit = crate::compositor::state::empty_cached_subsurface_commit();
    commit.commit_id = SurfaceCommitId::for_tests(sequence);
    commit.commit_sequence = SurfaceCommitSequence(sequence);
    commit
}

fn test_cached_commit(sequence: u64) -> CachedSubsurfaceCommit {
    let mut commit = crate::compositor::state::empty_cached_subsurface_commit();
    commit.commit_id = SurfaceCommitId::for_tests(sequence);
    commit.commit_sequence = SurfaceCommitSequence(sequence);
    commit.pacing.fifo_set_barrier = true;
    commit
}

fn test_captured_lifetimes(client_id: &ClientId, surface_ids: &[u32]) -> SurfaceTreeNodeLifetimes {
    SurfaceTreeNodeLifetimes::Captured(
        surface_ids
            .iter()
            .map(|surface_id| SurfaceTreeNodeLifetime {
                surface_id: *surface_id,
                owner_client_id: client_id.clone(),
                surface_presentation_generation: 1,
            })
            .collect(),
    )
}

fn test_surface_and_client(
    state: &mut CompositorState,
) -> (
    wayland_server::Display<CompositorState>,
    wayland_server::Client,
    u32,
) {
    let display = wayland_server::Display::<CompositorState>::new().expect("test display");
    let mut display_handle = display.handle();
    let (server_end, _peer) = std::os::unix::net::UnixStream::pair().expect("test socket");
    let client = display_handle
        .insert_client(server_end, std::sync::Arc::new(()))
        .expect("test client");
    let surface =
        state.test_create_unmapped_surface_resource_at_version(&client, &display_handle, 1);
    let surface_id = compositor_surface_id(&surface);
    state.surface_presentation_generations.insert(surface_id, 1);
    (display, client, surface_id)
}

#[test]
fn destroying_subsurface_removes_old_parent_edges_to_promoted_commits() {
    let mut state = CompositorState::default();
    let (display, client, root_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let parent_surface =
        state.test_create_unmapped_surface_resource_at_version(&client, &display_handle, 1);
    let child_surface =
        state.test_create_unmapped_surface_resource_at_version(&client, &display_handle, 1);
    let parent_id = compositor_surface_id(&parent_surface);
    let child_id = compositor_surface_id(&child_surface);
    state.surface_presentation_generations.insert(parent_id, 1);
    state.surface_presentation_generations.insert(child_id, 1);
    assert!(state.surface_transactions.register(parent_id, root_id));
    assert!(state.surface_transactions.register(child_id, parent_id));

    let mut child = test_cached_commit(70);
    child.pacing.commit_timing = Some(
        CommitTimingConstraint::from_protocol(client_pacing_now_ns() / 1_000_000_000 + 60, 0)
            .expect("future child timing"),
    );
    let child_ref = child.content_update_ref(child_id);
    assert!(matches!(
        state.surface_transactions.cache_commit(child_id, child),
        CacheCommitOutcome::Inserted
    ));
    let mut parent = test_cached_commit(71);
    let parent_ref = parent.content_update_ref(parent_id);
    parent.lineage.child_dependencies = vec![child_ref];
    assert!(matches!(
        state.surface_transactions.cache_commit(parent_id, parent),
        CacheCommitOutcome::Inserted
    ));

    state.destroy_subsurface_role(child_id);

    assert!(
        state
            .surface_transactions
            .pending_surface_tree_transactions
            .iter()
            .any(|transaction| {
                transaction.nodes.iter().any(|(surface_id, commit)| {
                    *surface_id == child_id && commit.commit_sequence == SurfaceCommitSequence(70)
                }) && !state.transaction_is_ready(transaction)
            })
    );

    let mut later_root = test_cached_commit(72);
    later_root.lineage.child_dependencies = vec![parent_ref];
    let candidate = state.extract_content_update_candidate(root_id, later_root);
    assert_eq!(
        candidate
            .nodes
            .iter()
            .map(|(surface_id, _)| *surface_id)
            .collect::<Vec<_>>(),
        vec![parent_id, root_id]
    );
    assert!(candidate.nodes[0].1.lineage.child_dependencies.is_empty());
    assert!(
        !candidate
            .external_content_update_dependencies
            .contains(&child_ref)
    );
}

#[test]
fn old_parent_edge_removal_preserves_child_predecessors() {
    let mut state = SurfaceTransactionState::default();
    assert!(state.register(2, 1));
    assert!(state.register(3, 2));
    let first = test_cached_commit(80);
    let first_ref = first.content_update_ref(3);
    let second = {
        let mut commit = test_cached_commit(81);
        commit.lineage.predecessor = Some(first_ref);
        commit
    };
    assert!(matches!(
        state.cache_commit(3, first),
        CacheCommitOutcome::Inserted
    ));
    assert!(matches!(
        state.cache_commit(3, second),
        CacheCommitOutcome::Inserted
    ));
    let mut parent = test_cached_commit(82);
    parent.lineage.child_dependencies = vec![first_ref];
    assert!(matches!(
        state.cache_commit(2, parent),
        CacheCommitOutcome::Inserted
    ));

    assert_eq!(
        state.remove_cached_parent_dependencies_to_child_commits(2, 3, &[first_ref]),
        1
    );
    let child_commits = state.take_cached_commits_for_surface(3);
    assert_eq!(child_commits.len(), 2);
    assert_eq!(child_commits[1].lineage.predecessor, Some(first_ref));
}

#[test]
fn old_parent_edge_removal_preserves_outgoing_child_dependencies() {
    let mut state = SurfaceTransactionState::default();
    assert!(state.register(2, 1));
    assert!(state.register(3, 2));
    assert!(state.register(4, 3));
    let grandchild = test_cached_commit(90);
    let grandchild_ref = grandchild.content_update_ref(4);
    assert!(matches!(
        state.cache_commit(4, grandchild),
        CacheCommitOutcome::Inserted
    ));
    let mut child = test_cached_commit(91);
    let child_ref = child.content_update_ref(3);
    child.lineage.child_dependencies = vec![grandchild_ref];
    assert!(matches!(
        state.cache_commit(3, child),
        CacheCommitOutcome::Inserted
    ));
    let mut parent = test_cached_commit(92);
    parent.lineage.child_dependencies = vec![child_ref];
    assert!(matches!(
        state.cache_commit(2, parent),
        CacheCommitOutcome::Inserted
    ));

    assert_eq!(
        state.remove_cached_parent_dependencies_to_child_commits(2, 3, &[child_ref]),
        1
    );
    let child_commits = state.take_cached_commits_for_surface(3);
    assert_eq!(
        child_commits[0].lineage.child_dependencies,
        vec![grandchild_ref]
    );
}

#[test]
fn merged_parent_lineage_drops_all_promoted_child_refs_only() {
    let mut state = CompositorState::default();
    assert!(state.surface_transactions.register(2, 1));
    assert!(state.surface_transactions.register(3, 2));
    assert!(state.surface_transactions.register(4, 2));
    let first = test_cached_commit(100);
    let first_ref = first.content_update_ref(3);
    let second = test_cached_commit(101);
    let second_ref = second.content_update_ref(3);
    assert!(matches!(
        state.surface_transactions.cache_commit(3, first),
        CacheCommitOutcome::Inserted
    ));
    assert!(matches!(
        state.surface_transactions.cache_commit(3, second),
        CacheCommitOutcome::Inserted
    ));
    let sibling_ref = ContentUpdateRef {
        surface_id: 4,
        commit_id: SurfaceCommitId::for_tests(102),
        commit_sequence: SurfaceCommitSequence(102),
    };
    let mut older_parent = test_mergeable_commit(200);
    older_parent.lineage.child_dependencies = vec![first_ref];
    assert!(matches!(
        state.surface_transactions.cache_commit(2, older_parent),
        CacheCommitOutcome::Inserted
    ));
    let mut newer_parent = test_mergeable_commit(201);
    newer_parent.lineage.child_dependencies = vec![second_ref, sibling_ref];
    assert!(matches!(
        state.surface_transactions.cache_commit(2, newer_parent),
        CacheCommitOutcome::Merged { .. }
    ));

    state.destroy_subsurface_role(3);

    let parent_commits = state
        .surface_transactions
        .take_cached_commits_for_surface(2);
    assert_eq!(parent_commits.len(), 1);
    assert_eq!(
        parent_commits[0].lineage.child_dependencies,
        vec![sibling_ref]
    );
}

#[test]
fn admitted_parent_candidate_is_not_mutated_by_detach_cleanup() {
    let mut state = CompositorState::default();
    let (display, client, parent_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let child_surface =
        state.test_create_unmapped_surface_resource_at_version(&client, &display_handle, 1);
    let child = compositor_surface_id(&child_surface);
    let blocker_surface =
        state.test_create_unmapped_surface_resource_at_version(&client, &display_handle, 1);
    let blocker = compositor_surface_id(&blocker_surface);
    state.surface_presentation_generations.insert(child, 1);
    state.surface_presentation_generations.insert(blocker, 1);
    assert!(state.surface_transactions.register(child, parent_id));
    let child_commit = test_cached_commit(110);
    let child_ref = child_commit.content_update_ref(child);
    let mut parent_commit = test_cached_commit(111);
    parent_commit.lineage.child_dependencies = vec![child_ref];
    state
        .surface_transactions
        .pending_surface_tree_transactions
        .push(PendingSurfaceTreeTransaction {
            id: SurfaceTreeTransactionId::new(16),
            root_surface_id: parent_id,
            nodes: vec![(parent_id, parent_commit), (child, child_commit)],
            publication_lifetimes: test_captured_lifetimes(&client.id(), &[parent_id, child]),
            dependencies: Vec::new(),
            external_content_update_dependencies: vec![ContentUpdateRef {
                surface_id: blocker,
                commit_id: SurfaceCommitId::for_tests(112),
                commit_sequence: SurfaceCommitSequence(112),
            }],
            commit_timing_readiness: None,
            received_at: Instant::now(),
        });

    state.destroy_subsurface_role(child);

    assert_eq!(
        state
            .surface_transactions
            .pending_surface_tree_transactions
            .len(),
        1
    );
    assert_eq!(
        state.surface_transactions.pending_surface_tree_transactions[0]
            .nodes
            .iter()
            .map(|(_, commit)| commit.commit_sequence)
            .collect::<Vec<_>>(),
        vec![SurfaceCommitSequence(111), SurfaceCommitSequence(110)]
    );
    assert_eq!(
        state.surface_transactions.pending_surface_tree_transactions[0].nodes[0]
            .1
            .lineage
            .child_dependencies,
        vec![child_ref]
    );
}
