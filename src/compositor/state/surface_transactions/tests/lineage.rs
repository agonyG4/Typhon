use super::*;

#[test]
fn candidate_extraction_follows_exact_direct_child_edges() {
    let mut state = CompositorState::default();
    assert!(state.surface_transactions.register(2, 1));
    assert!(state.surface_transactions.register(3, 2));

    let grandchild = test_cached_commit(10);
    let grandchild_ref = grandchild.content_update_ref(3);
    assert!(matches!(
        state.surface_transactions.cache_commit(3, grandchild),
        CacheCommitOutcome::Inserted
    ));
    let dependencies = state
        .surface_transactions
        .capture_direct_child_dependencies(2);
    assert_eq!(dependencies, vec![grandchild_ref]);

    let later_grandchild = test_cached_commit(11);
    assert!(matches!(
        state.surface_transactions.cache_commit(3, later_grandchild),
        CacheCommitOutcome::Inserted
    ));

    let mut child = test_cached_commit(12);
    child.lineage.child_dependencies = dependencies;
    let candidate = state.extract_content_update_candidate(2, child);

    assert_eq!(
        candidate
            .nodes
            .iter()
            .map(|(_, commit)| commit.commit_sequence)
            .collect::<Vec<_>>(),
        vec![SurfaceCommitSequence(10), SurfaceCommitSequence(12)]
    );
    assert!(candidate.external_content_update_dependencies.is_empty());
    let remaining = state
        .surface_transactions
        .take_cached_commits_for_surface(3);
    assert_eq!(
        remaining
            .iter()
            .map(|commit| commit.commit_sequence)
            .collect::<Vec<_>>(),
        vec![SurfaceCommitSequence(11)]
    );
}

#[test]
fn candidate_extraction_does_not_drain_orphan_grandchildren() {
    let mut state = CompositorState::default();
    assert!(state.surface_transactions.register(2, 1));
    assert!(state.surface_transactions.register(3, 2));
    assert!(matches!(
        state
            .surface_transactions
            .cache_commit(3, test_cached_commit(20)),
        CacheCommitOutcome::Inserted
    ));

    let candidate = state.extract_content_update_candidate(1, test_cached_commit(21));

    assert_eq!(candidate.nodes.len(), 1);
    assert_eq!(candidate.nodes[0].0, 1);
    assert_eq!(
        state
            .surface_transactions
            .take_cached_commits_for_surface(3)
            .len(),
        1
    );
}

#[test]
fn relationship_detach_does_not_create_a_fake_parent_update_for_a_sync_grandchild() {
    let mut state = CompositorState::default();
    assert!(state.surface_transactions.register(2, 1));
    assert!(state.surface_transactions.register(3, 2));
    assert!(matches!(
        state
            .surface_transactions
            .cache_commit(3, test_cached_commit(25)),
        CacheCommitOutcome::Inserted
    ));

    state.destroy_subsurface_role(2);

    assert!(
        state
            .surface_transactions
            .pending_surface_tree_transactions
            .is_empty()
    );
    let retained = state
        .surface_transactions
        .take_cached_commits_for_surface(3);
    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0].commit_sequence, SurfaceCommitSequence(25));
}

#[test]
fn cached_same_surface_prefix_is_emitted_in_predecessor_order() {
    let mut state = CompositorState::default();
    assert!(state.surface_transactions.register(2, 1));
    let mut predecessor = None;
    for sequence in 30..=32 {
        let mut commit = test_cached_commit(sequence);
        commit.lineage.predecessor = predecessor;
        predecessor = Some(commit.content_update_ref(2));
        assert!(matches!(
            state.surface_transactions.cache_commit(2, commit),
            CacheCommitOutcome::Inserted
        ));
    }
    let reference = ContentUpdateRef {
        surface_id: 2,
        commit_id: SurfaceCommitId::for_tests(32),
        commit_sequence: SurfaceCommitSequence(32),
    };
    let mut dependent = test_cached_commit(33);
    dependent.lineage.child_dependencies = vec![reference];

    let candidate = state.extract_content_update_candidate(1, dependent);

    assert_eq!(
        candidate
            .nodes
            .iter()
            .map(|(_, commit)| commit.commit_sequence)
            .collect::<Vec<_>>(),
        vec![
            SurfaceCommitSequence(30),
            SurfaceCommitSequence(31),
            SurfaceCommitSequence(32),
            SurfaceCommitSequence(33),
        ]
    );
}

#[test]
fn external_content_update_dependencies_wait_for_their_owner() {
    let mut state = CompositorState::default();
    let dependency = ContentUpdateRef {
        surface_id: 2,
        commit_id: SurfaceCommitId::for_tests(40),
        commit_sequence: SurfaceCommitSequence(40),
    };
    state
        .surface_transactions
        .pending_surface_tree_transactions
        .push(PendingSurfaceTreeTransaction {
            id: SurfaceTreeTransactionId::new(1),
            root_surface_id: 1,
            nodes: vec![(2, test_cached_commit(40))],
            publication_lifetimes: SurfaceTreeNodeLifetimes::Synthetic,
            dependencies: Vec::new(),
            external_content_update_dependencies: Vec::new(),
            commit_timing_readiness: None,
            received_at: Instant::now(),
        });
    let waiting = PendingSurfaceTreeTransaction {
        id: SurfaceTreeTransactionId::new(2),
        root_surface_id: 3,
        nodes: vec![(3, test_cached_commit(41))],
        publication_lifetimes: SurfaceTreeNodeLifetimes::Synthetic,
        dependencies: Vec::new(),
        external_content_update_dependencies: vec![dependency],
        commit_timing_readiness: None,
        received_at: Instant::now(),
    };

    assert!(!state.content_update_dependencies_ready(&waiting));
    state
        .surface_transactions
        .pending_surface_tree_transactions
        .clear();
    state.surface_publications.insert(
        2,
        SurfacePublicationState {
            latest_published: Some(SurfaceCommitSequence(40)),
            ..SurfacePublicationState::default()
        },
    );
    assert!(state.content_update_dependencies_ready(&waiting));
}

#[test]
fn standalone_later_same_surface_node_does_not_cover_its_predecessor() {
    let mut state = CompositorState::default();
    let predecessor = test_mergeable_commit(1);
    let predecessor_ref = predecessor.content_update_ref(2);
    let mut newer = test_mergeable_commit(2);
    newer.lineage.predecessor = Some(predecessor_ref);

    state
        .surface_transactions
        .pending_surface_tree_transactions
        .push(PendingSurfaceTreeTransaction {
            id: SurfaceTreeTransactionId::new(8),
            root_surface_id: 1,
            nodes: vec![(2, predecessor)],
            publication_lifetimes: SurfaceTreeNodeLifetimes::Synthetic,
            dependencies: Vec::new(),
            external_content_update_dependencies: Vec::new(),
            commit_timing_readiness: None,
            received_at: Instant::now(),
        });
    let waiting = PendingSurfaceTreeTransaction {
        id: SurfaceTreeTransactionId::new(9),
        root_surface_id: 2,
        nodes: vec![(2, newer)],
        publication_lifetimes: SurfaceTreeNodeLifetimes::Synthetic,
        dependencies: Vec::new(),
        external_content_update_dependencies: vec![predecessor_ref],
        commit_timing_readiness: None,
        received_at: Instant::now(),
    };

    assert!(state.content_update_ref_is_pending(predecessor_ref));
    assert!(!transaction_covers_content_update_ref(
        &waiting,
        predecessor_ref
    ));
    assert!(!state.content_update_dependencies_ready(&waiting));
    state.surface_publications.insert(
        2,
        SurfacePublicationState {
            latest_published: Some(predecessor_ref.commit_sequence),
            ..SurfacePublicationState::default()
        },
    );
    assert!(state.content_update_dependencies_ready(&waiting));
}

#[test]
fn coalesced_predecessor_is_internalized_and_publishes_after_other_readiness_clears() {
    let mut state = CompositorState::default();
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let blocker_surface =
        state.test_create_unmapped_surface_resource_at_version(&client, &display_handle, 1);
    let blocker_surface_id = compositor_surface_id(&blocker_surface);
    state
        .surface_presentation_generations
        .insert(blocker_surface_id, 1);
    let remaining_dependency = ContentUpdateRef {
        surface_id: blocker_surface_id,
        commit_id: SurfaceCommitId::for_tests(3),
        commit_sequence: SurfaceCommitSequence(3),
    };
    let predecessor = test_mergeable_commit(1);
    let predecessor_ref = predecessor.content_update_ref(surface_id);
    let mut newer = test_mergeable_commit(2);
    newer.lineage.predecessor = Some(predecessor_ref);

    let mut transaction = PendingSurfaceTreeTransaction {
        id: SurfaceTreeTransactionId::new(10),
        root_surface_id: surface_id,
        nodes: vec![(surface_id, predecessor)],
        publication_lifetimes: test_captured_lifetimes(&client.id(), &[surface_id]),
        dependencies: Vec::new(),
        external_content_update_dependencies: Vec::new(),
        commit_timing_readiness: None,
        received_at: Instant::now(),
    };
    state.merge_surface_tree_nodes_into_transaction(
        surface_id,
        &mut transaction,
        vec![(surface_id, newer)],
        test_captured_lifetimes(&client.id(), &[surface_id]),
        Vec::new(),
        vec![predecessor_ref, remaining_dependency],
    );
    state
        .surface_transactions
        .pending_surface_tree_transactions
        .push(transaction);

    assert_eq!(
        state.surface_transactions.pending_surface_tree_transactions[0].nodes[0]
            .1
            .commit_sequence,
        SurfaceCommitSequence(2)
    );
    assert!(
        !state.surface_transactions.pending_surface_tree_transactions[0]
            .external_content_update_dependencies
            .contains(&predecessor_ref)
    );
    assert!(state.content_update_ref_is_pending(predecessor_ref));
    assert_eq!(
        state.surface_transactions.pending_surface_tree_transactions[0]
            .external_content_update_dependencies,
        vec![remaining_dependency]
    );
    assert!(!state.content_update_dependencies_ready(
        &state.surface_transactions.pending_surface_tree_transactions[0]
    ));

    state.surface_publications.insert(
        blocker_surface_id,
        SurfacePublicationState {
            latest_published: Some(remaining_dependency.commit_sequence),
            ..SurfacePublicationState::default()
        },
    );
    state.commit_ready_surface_tree_transactions();
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
}

#[test]
fn coalescing_preserves_incoming_dependency_first_order() {
    let mut state = CompositorState::default();
    let (_display, client, _surface_id) = test_surface_and_client(&mut state);
    let child = test_mergeable_commit(1);
    let child_ref = child.content_update_ref(3);
    let target = test_mergeable_commit(2);
    let target_ref = target.content_update_ref(2);
    let mut dependent = test_mergeable_commit(3);
    dependent.lineage.predecessor = Some(target_ref);
    dependent.lineage.child_dependencies = vec![child_ref];

    let mut transaction = PendingSurfaceTreeTransaction {
        id: SurfaceTreeTransactionId::new(11),
        root_surface_id: 2,
        nodes: vec![(2, target)],
        publication_lifetimes: test_captured_lifetimes(&client.id(), &[2]),
        dependencies: Vec::new(),
        external_content_update_dependencies: Vec::new(),
        commit_timing_readiness: None,
        received_at: Instant::now(),
    };
    state.merge_surface_tree_nodes_into_transaction(
        2,
        &mut transaction,
        vec![(3, child), (2, dependent)],
        test_captured_lifetimes(&client.id(), &[3, 2]),
        Vec::new(),
        vec![target_ref],
    );

    assert_eq!(
        transaction
            .nodes
            .iter()
            .map(|(_, commit)| commit.commit_sequence)
            .collect::<Vec<_>>(),
        vec![SurfaceCommitSequence(1), SurfaceCommitSequence(3)]
    );
    assert!(transaction.external_content_update_dependencies.is_empty());
}

#[test]
fn external_observer_waits_for_an_absorbed_predecessor_owner() {
    let mut state = CompositorState::default();
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let observer_surface =
        state.test_create_unmapped_surface_resource_at_version(&client, &display_handle, 1);
    let observer_surface_id = compositor_surface_id(&observer_surface);
    state
        .surface_presentation_generations
        .insert(observer_surface_id, 1);
    let predecessor_ref = ContentUpdateRef {
        surface_id,
        commit_id: SurfaceCommitId::for_tests(1),
        commit_sequence: SurfaceCommitSequence(1),
    };
    let predecessor = test_mergeable_commit(1);
    let mut newer = test_mergeable_commit(2);
    newer.lineage.predecessor = Some(predecessor_ref);
    let mut owner = PendingSurfaceTreeTransaction {
        id: SurfaceTreeTransactionId::new(14),
        root_surface_id: surface_id,
        nodes: vec![(surface_id, predecessor)],
        publication_lifetimes: test_captured_lifetimes(&client.id(), &[surface_id]),
        dependencies: Vec::new(),
        external_content_update_dependencies: Vec::new(),
        commit_timing_readiness: None,
        received_at: Instant::now(),
    };
    state.merge_surface_tree_nodes_into_transaction(
        surface_id,
        &mut owner,
        vec![(surface_id, newer)],
        test_captured_lifetimes(&client.id(), &[surface_id]),
        Vec::new(),
        Vec::new(),
    );
    state
        .surface_transactions
        .pending_surface_tree_transactions
        .push(owner);
    assert_eq!(
        state.surface_transactions.pending_surface_tree_transactions[0].nodes[0]
            .1
            .commit_sequence,
        SurfaceCommitSequence(2)
    );
    assert!(transaction_covers_content_update_ref(
        &state.surface_transactions.pending_surface_tree_transactions[0],
        predecessor_ref
    ));
    let waiting = PendingSurfaceTreeTransaction {
        id: SurfaceTreeTransactionId::new(15),
        root_surface_id: observer_surface_id,
        nodes: vec![(observer_surface_id, test_mergeable_commit(3))],
        publication_lifetimes: test_captured_lifetimes(&client.id(), &[observer_surface_id]),
        dependencies: Vec::new(),
        external_content_update_dependencies: vec![predecessor_ref],
        commit_timing_readiness: None,
        received_at: Instant::now(),
    };

    assert!(state.content_update_ref_is_pending(predecessor_ref));
    assert!(!state.content_update_dependencies_ready(&waiting));
    state
        .surface_transactions
        .pending_surface_tree_transactions
        .remove(0);
    state.surface_publications.insert(
        surface_id,
        SurfacePublicationState {
            latest_published: Some(SurfaceCommitSequence(2)),
            ..SurfacePublicationState::default()
        },
    );
    assert!(!state.content_update_ref_is_pending(predecessor_ref));
    assert!(state.content_update_dependencies_ready(&waiting));
}

#[test]
fn retired_role_content_update_is_a_terminal_dependency() {
    let mut state = CompositorState::default();
    let (_display, _client, surface_id) = test_surface_and_client(&mut state);
    let mut commit = empty_cached_subsurface_commit();
    commit.commit_id = SurfaceCommitId::for_tests(7);
    commit.commit_sequence = SurfaceCommitSequence(7);
    let reference = commit.content_update_ref(surface_id);
    state.surface_publications.insert(
        surface_id,
        SurfacePublicationState {
            latest_received: commit.commit_sequence,
            ..SurfacePublicationState::default()
        },
    );
    state
        .surface_transactions
        .pending_surface_tree_transactions
        .push(PendingSurfaceTreeTransaction {
            id: SurfaceTreeTransactionId::new(18),
            root_surface_id: surface_id,
            nodes: vec![(surface_id, commit)],
            publication_lifetimes: SurfaceTreeNodeLifetimes::Synthetic,
            dependencies: Vec::new(),
            external_content_update_dependencies: Vec::new(),
            commit_timing_readiness: None,
            received_at: Instant::now(),
        });

    assert!(!state.content_update_ref_is_terminal(reference));
    state.retire_unpublished_work_for_xdg_role(surface_id, AcquireWatchCancelReason::RoleDestroyed);
    assert!(state.content_update_ref_is_terminal(reference));
    assert!(
        state
            .surface_transactions
            .pending_surface_tree_transactions
            .is_empty()
    );
}

#[test]
fn unpaced_same_surface_candidate_prefix_is_canonicalized_before_acquires() {
    let mut state = CompositorState::default();
    let (display, client, _root_surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let surface = |state: &mut CompositorState| {
        let resource =
            state.test_create_unmapped_surface_resource_at_version(&client, &display_handle, 1);
        let surface_id = compositor_surface_id(&resource);
        state.surface_presentation_generations.insert(surface_id, 1);
        surface_id
    };
    let grandchild_id = surface(&mut state);
    let child_id = surface(&mut state);
    let blocker_id = surface(&mut state);
    let blocker = ContentUpdateRef {
        surface_id: blocker_id,
        commit_id: SurfaceCommitId::for_tests(99),
        commit_sequence: SurfaceCommitSequence(99),
    };

    let grandchild_one = test_mergeable_commit(1);
    let grandchild_one_ref = grandchild_one.content_update_ref(grandchild_id);
    let mut grandchild_two = test_mergeable_commit(2);
    grandchild_two.lineage.predecessor = Some(grandchild_one_ref);
    let grandchild_two_ref = grandchild_two.content_update_ref(grandchild_id);
    let mut child = test_mergeable_commit(3);
    child.lineage.child_dependencies = vec![grandchild_one_ref, grandchild_two_ref];

    state.submit_surface_tree_nodes_with_kind(
        child_id,
        vec![
            (grandchild_id, grandchild_one),
            (grandchild_id, grandchild_two),
            (child_id, child),
        ],
        vec![blocker],
        SurfaceTreeSubmissionKind::ClientAdmission,
    );

    assert_eq!(
        state
            .surface_transactions
            .pending_surface_tree_transactions
            .len(),
        1
    );
    let transaction = &state.surface_transactions.pending_surface_tree_transactions[0];
    assert_eq!(
        transaction
            .nodes
            .iter()
            .map(|(surface_id, _)| *surface_id)
            .collect::<Vec<_>>(),
        vec![grandchild_id, child_id]
    );
    assert!(transaction_covers_content_update_ref(
        transaction,
        grandchild_one_ref
    ));
    assert!(transaction_covers_content_update_ref(
        transaction,
        grandchild_two_ref
    ));
    assert_eq!(
        transaction
            .publication_lifetimes
            .captured()
            .expect("captured publication lifetimes")
            .iter()
            .map(|lifetime| lifetime.surface_id)
            .collect::<Vec<_>>(),
        vec![grandchild_id, child_id]
    );
}

#[test]
fn coalesced_range_covers_absorbed_updates_but_not_oldest_predecessor() {
    let mut state = CompositorState::default();
    let (_display, client, surface_id) = test_surface_and_client(&mut state);
    let predecessor = ContentUpdateRef {
        surface_id,
        commit_id: SurfaceCommitId::for_tests(1),
        commit_sequence: SurfaceCommitSequence(1),
    };
    let mut first = test_mergeable_commit(2);
    first.lineage.predecessor = Some(predecessor);
    let first_ref = first.content_update_ref(surface_id);
    let mut second = test_mergeable_commit(3);
    second.lineage.predecessor = Some(first_ref);
    let second_ref = second.content_update_ref(surface_id);
    let mut transaction = PendingSurfaceTreeTransaction {
        id: SurfaceTreeTransactionId::new(16),
        root_surface_id: surface_id,
        nodes: vec![(surface_id, first)],
        publication_lifetimes: test_captured_lifetimes(&client.id(), &[surface_id]),
        dependencies: Vec::new(),
        external_content_update_dependencies: Vec::new(),
        commit_timing_readiness: None,
        received_at: Instant::now(),
    };

    state.merge_surface_tree_nodes_into_transaction(
        surface_id,
        &mut transaction,
        vec![(surface_id, second)],
        test_captured_lifetimes(&client.id(), &[surface_id]),
        Vec::new(),
        Vec::new(),
    );

    assert!(transaction_covers_content_update_ref(
        &transaction,
        first_ref
    ));
    assert!(transaction_covers_content_update_ref(
        &transaction,
        second_ref
    ));
    assert!(!transaction_covers_content_update_ref(
        &transaction,
        predecessor
    ));
}

#[test]
fn repeated_same_surface_coalescence_covers_every_absorbed_update() {
    let mut state = CompositorState::default();
    let (_display, client, surface_id) = test_surface_and_client(&mut state);
    let first = test_mergeable_commit(1);
    let mut transaction = PendingSurfaceTreeTransaction {
        id: SurfaceTreeTransactionId::new(17),
        root_surface_id: surface_id,
        nodes: vec![(surface_id, first)],
        publication_lifetimes: test_captured_lifetimes(&client.id(), &[surface_id]),
        dependencies: Vec::new(),
        external_content_update_dependencies: Vec::new(),
        commit_timing_readiness: None,
        received_at: Instant::now(),
    };
    let mut refs = vec![transaction.nodes[0].1.content_update_ref(surface_id)];
    for sequence in 2..=4 {
        let mut newer = test_mergeable_commit(sequence);
        newer.lineage.predecessor = refs.last().copied();
        refs.push(newer.content_update_ref(surface_id));
        state.merge_surface_tree_nodes_into_transaction(
            surface_id,
            &mut transaction,
            vec![(surface_id, newer)],
            test_captured_lifetimes(&client.id(), &[surface_id]),
            Vec::new(),
            Vec::new(),
        );
    }

    assert_eq!(transaction.nodes.len(), 1);
    assert_eq!(
        transaction.nodes[0].1.commit_sequence,
        SurfaceCommitSequence(4)
    );
    for reference in refs {
        assert!(transaction_covers_content_update_ref(
            &transaction,
            reference
        ));
    }
}

#[test]
fn pacing_protected_same_surface_prefix_remains_exact() {
    let mut state = CompositorState::default();
    let (display, client, _root_surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let surface = |state: &mut CompositorState| {
        let resource =
            state.test_create_unmapped_surface_resource_at_version(&client, &display_handle, 1);
        let surface_id = compositor_surface_id(&resource);
        state.surface_presentation_generations.insert(surface_id, 1);
        surface_id
    };
    let grandchild_id = surface(&mut state);
    let child_id = surface(&mut state);
    let blocker_id = surface(&mut state);
    let blocker = ContentUpdateRef {
        surface_id: blocker_id,
        commit_id: SurfaceCommitId::for_tests(99),
        commit_sequence: SurfaceCommitSequence(99),
    };
    let mut grandchild_one = test_cached_commit(1);
    grandchild_one.pacing.fifo_set_barrier = true;
    let grandchild_one_ref = grandchild_one.content_update_ref(grandchild_id);
    let mut grandchild_two = test_mergeable_commit(2);
    grandchild_two.lineage.predecessor = Some(grandchild_one_ref);
    let grandchild_two_ref = grandchild_two.content_update_ref(grandchild_id);
    let mut child = test_mergeable_commit(3);
    child.lineage.child_dependencies = vec![grandchild_one_ref, grandchild_two_ref];

    state.submit_surface_tree_nodes_with_kind(
        child_id,
        vec![
            (grandchild_id, grandchild_one),
            (grandchild_id, grandchild_two),
            (child_id, child),
        ],
        vec![blocker],
        SurfaceTreeSubmissionKind::ClientAdmission,
    );

    let transaction = &state.surface_transactions.pending_surface_tree_transactions[0];
    assert_eq!(transaction.ordering(), TransactionOrdering::PacingProtected);
    assert_eq!(
        transaction
            .nodes
            .iter()
            .map(|(_, commit)| commit.commit_sequence)
            .collect::<Vec<_>>(),
        vec![
            SurfaceCommitSequence(1),
            SurfaceCommitSequence(2),
            SurfaceCommitSequence(3)
        ]
    );
    assert_eq!(
        transaction_node_index_covering_content_update_ref(transaction, grandchild_one_ref),
        Some(0)
    );
    assert_eq!(
        transaction_node_index_covering_content_update_ref(transaction, grandchild_two_ref),
        Some(1)
    );
}

#[test]
fn coalescible_mixed_dag_reduces_each_surface_and_keeps_dependencies_before_dependents() {
    let mut state = CompositorState::default();
    let (display, client, _root_surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let surface = |state: &mut CompositorState| {
        let resource =
            state.test_create_unmapped_surface_resource_at_version(&client, &display_handle, 1);
        let surface_id = compositor_surface_id(&resource);
        state.surface_presentation_generations.insert(surface_id, 1);
        surface_id
    };
    let grandchild_id = surface(&mut state);
    let child_id = surface(&mut state);
    let blocker_id = surface(&mut state);
    let blocker = ContentUpdateRef {
        surface_id: blocker_id,
        commit_id: SurfaceCommitId::for_tests(99),
        commit_sequence: SurfaceCommitSequence(99),
    };
    let grandchild_one = test_mergeable_commit(1);
    let grandchild_one_ref = grandchild_one.content_update_ref(grandchild_id);
    let mut child_one = test_mergeable_commit(2);
    child_one.lineage.child_dependencies = vec![grandchild_one_ref];
    let child_one_ref = child_one.content_update_ref(child_id);
    let mut grandchild_two = test_mergeable_commit(3);
    grandchild_two.lineage.predecessor = Some(grandchild_one_ref);
    let grandchild_two_ref = grandchild_two.content_update_ref(grandchild_id);
    let mut child_two = test_mergeable_commit(4);
    child_two.lineage.predecessor = Some(child_one_ref);
    child_two.lineage.child_dependencies = vec![grandchild_two_ref];
    let child_two_ref = child_two.content_update_ref(child_id);

    state.submit_surface_tree_nodes_with_kind(
        child_id,
        vec![
            (grandchild_id, grandchild_one),
            (child_id, child_one),
            (grandchild_id, grandchild_two),
            (child_id, child_two),
        ],
        vec![blocker],
        SurfaceTreeSubmissionKind::ClientAdmission,
    );

    let transaction = &state.surface_transactions.pending_surface_tree_transactions[0];
    assert_eq!(
        transaction
            .nodes
            .iter()
            .map(|(surface_id, _)| *surface_id)
            .collect::<Vec<_>>(),
        vec![grandchild_id, child_id]
    );
    let grandchild = &transaction.nodes[0].1;
    let child = &transaction.nodes[1].1;
    assert_eq!(grandchild.commit_sequence, SurfaceCommitSequence(3));
    assert_eq!(child.commit_sequence, SurfaceCommitSequence(4));
    assert!(transaction_covers_content_update_ref(
        transaction,
        grandchild_one_ref
    ));
    assert!(transaction_covers_content_update_ref(
        transaction,
        grandchild_two_ref
    ));
    assert!(transaction_covers_content_update_ref(
        transaction,
        child_one_ref
    ));
    assert!(transaction_covers_content_update_ref(
        transaction,
        child_two_ref
    ));
    assert_eq!(
        child.lineage.child_dependencies,
        vec![grandchild_one_ref, grandchild_two_ref]
    );
}
