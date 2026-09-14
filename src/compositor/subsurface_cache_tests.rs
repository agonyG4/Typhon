use super::*;
use crate::compositor::subsurface::ContentUpdateRef;
use crate::compositor::{
    CommitTimingConstraint, CompositorState, SurfacePublicationState, client_pacing_now_ns,
    compositor_surface_id, empty_cached_subsurface_commit,
};
use std::{os::unix::net::UnixStream, sync::Arc};

use wayland_server::{Display, protocol::wl_callback};

fn test_client(
    display_handle: &mut wayland_server::DisplayHandle,
) -> (wayland_server::Client, UnixStream) {
    let (server_end, peer) = UnixStream::pair().expect("cache test client");
    let client = display_handle
        .insert_client(server_end, Arc::new(()))
        .expect("insert cache test client");
    (client, peer)
}

#[test]
fn new_role_defaults_to_synchronized() {
    let mut state = SubsurfaceTransactionState::default();
    assert!(state.register(2, 1));
    assert_eq!(
        state.requested_mode(2),
        Some(SubsurfaceSyncMode::Synchronized)
    );
    assert!(state.is_effectively_synchronized(2));
}

#[test]
fn set_sync_and_set_desync_record_requested_mode() {
    let mut state = SubsurfaceTransactionState::default();
    assert!(state.register(2, 1));
    assert!(state.set_mode(2, SubsurfaceSyncMode::Desynchronized));
    assert_eq!(
        state.requested_mode(2),
        Some(SubsurfaceSyncMode::Desynchronized)
    );
    assert!(state.set_mode(2, SubsurfaceSyncMode::Synchronized));
    assert_eq!(
        state.requested_mode(2),
        Some(SubsurfaceSyncMode::Synchronized)
    );
}

#[test]
fn desynchronized_descendant_under_synchronized_ancestor_remains_effectively_sync() {
    let mut state = SubsurfaceTransactionState::default();
    assert!(state.register(2, 1));
    assert!(state.register(3, 2));
    assert!(state.set_mode(3, SubsurfaceSyncMode::Desynchronized));
    assert!(state.is_effectively_synchronized(3));
    assert!(state.set_mode(2, SubsurfaceSyncMode::Desynchronized));
    assert!(!state.is_effectively_synchronized(3));
}

#[test]
fn role_registration_rejects_reuse_and_cycles() {
    let mut state = SubsurfaceTransactionState::default();
    assert!(state.register(2, 1));
    assert!(!state.register(2, 3));
    assert!(!state.register(1, 2));
}

#[test]
fn role_destruction_removes_only_that_role_while_surface_teardown_removes_subtree() {
    let mut state = SubsurfaceTransactionState::default();
    assert!(state.register(2, 1));
    assert!(state.register(3, 2));
    assert!(state.remove_role(2).is_empty());
    assert_eq!(state.parent(2), None);
    assert_eq!(state.parent(3), Some(2));

    assert!(state.register(4, 1));
    assert!(state.register(5, 4));
    assert!(state.remove_subtree(4).is_empty());
    assert_eq!(state.parent(4), None);
    assert_eq!(state.parent(5), None);
}

#[test]
fn pacing_boundaries_are_never_merged_or_reordered() {
    let mut state = SubsurfaceTransactionState::default();
    assert!(state.register(2, 1));

    let mut first = crate::compositor::state::empty_cached_subsurface_commit();
    first.pacing = CapturedSurfacePacing {
        fifo_set_barrier: true,
        ..CapturedSurfacePacing::default()
    };
    let mut second = crate::compositor::state::empty_cached_subsurface_commit();
    second.pacing = CapturedSurfacePacing {
        fifo_wait_barrier: true,
        ..CapturedSurfacePacing::default()
    };

    assert!(matches!(
        state.cache_commit(2, first),
        CacheCommitOutcome::Inserted
    ));
    assert!(matches!(
        state.cache_commit(2, second),
        CacheCommitOutcome::Inserted
    ));
    assert_eq!(state.roles[&2].cached_commits.len(), 2);
    assert!(state.roles[&2].cached_commits[0].pacing.fifo_set_barrier);
    assert!(state.roles[&2].cached_commits[1].pacing.fifo_wait_barrier);
}

#[test]
fn synchronized_cache_does_not_grow_past_the_surface_limit() {
    let mut state = SubsurfaceTransactionState::default();
    assert!(state.register(2, 1));

    for sequence in 0..8 {
        let mut commit = crate::compositor::state::empty_cached_subsurface_commit();
        commit.commit_id = SurfaceCommitId::for_tests(sequence + 1);
        commit.commit_sequence = SurfaceCommitSequence(sequence + 1);
        commit.pacing.fifo_set_barrier = true;
        assert!(matches!(
            state.cache_commit(2, commit),
            CacheCommitOutcome::Inserted
        ));
    }
    let mut rejected = crate::compositor::state::empty_cached_subsurface_commit();
    rejected.commit_id = SurfaceCommitId::for_tests(9);
    rejected.commit_sequence = SurfaceCommitSequence(9);
    rejected.pacing.fifo_set_barrier = true;
    assert!(matches!(
        state.cache_commit(2, rejected),
        CacheCommitOutcome::Rejected {
            reason: CacheAdmissionFailure::PerSurfaceEntryLimit,
            ..
        }
    ));

    assert_eq!(state.roles[&2].cached_commits.len(), 8);
    assert_eq!(state.cached_entry_count(), 8);
    assert_eq!(state.maximum_cached_entries(), 8);
    assert!(state.debug_accounting_is_consistent());
}

#[test]
fn ordinary_commits_after_a_boundary_merge_into_the_tail() {
    let mut state = SubsurfaceTransactionState::default();
    assert!(state.register(2, 1));

    let mut boundary = crate::compositor::state::empty_cached_subsurface_commit();
    boundary.commit_id = SurfaceCommitId::for_tests(1);
    boundary.pacing.fifo_set_barrier = true;
    assert!(matches!(
        state.cache_commit(2, boundary),
        CacheCommitOutcome::Inserted
    ));

    let mut tail_merges = 0;
    for sequence in 2..=4 {
        let mut commit = crate::compositor::state::empty_cached_subsurface_commit();
        commit.commit_id = SurfaceCommitId::for_tests(sequence);
        commit.commit_sequence = SurfaceCommitSequence(sequence);
        if matches!(
            state.cache_commit(2, commit),
            CacheCommitOutcome::Merged { .. }
        ) {
            tail_merges += 1;
        }
    }

    assert_eq!(tail_merges, 2);
    assert_eq!(state.roles[&2].cached_commits.len(), 2);
    assert_eq!(
        state.roles[&2].cached_commits[0].commit_id,
        SurfaceCommitId::for_tests(1)
    );
    assert_eq!(
        state.roles[&2].cached_commits[1].commit_id,
        SurfaceCommitId::for_tests(4)
    );
}

#[test]
fn boundary_stress_remains_bounded_without_retaining_rejected_commits() {
    let mut state = SubsurfaceTransactionState::default();
    assert!(state.register(2, 1));

    for sequence in 1..=100_000 {
        let mut commit = crate::compositor::state::empty_cached_subsurface_commit();
        commit.commit_id = SurfaceCommitId::for_tests(sequence);
        commit.commit_sequence = SurfaceCommitSequence(sequence);
        commit.pacing.fifo_set_barrier = true;
        state.cache_commit(2, commit);
    }

    assert_eq!(state.roles[&2].cached_commits.len(), 8);
    assert_eq!(state.cached_entry_count(), 8);
    assert_eq!(state.maximum_cached_entries(), 8);
    assert!(state.debug_accounting_is_consistent());
}

#[test]
fn ordinary_and_boundary_commits_remain_ordered_at_the_tail() {
    let mut state = SubsurfaceTransactionState::default();
    assert!(state.register(2, 1));

    let mut ordinary = crate::compositor::state::empty_cached_subsurface_commit();
    ordinary.commit_id = SurfaceCommitId::for_tests(1);
    assert!(matches!(
        state.cache_commit(2, ordinary),
        CacheCommitOutcome::Inserted
    ));

    let mut boundary = crate::compositor::state::empty_cached_subsurface_commit();
    boundary.commit_id = SurfaceCommitId::for_tests(2);
    boundary.pacing.fifo_set_barrier = true;
    assert!(matches!(
        state.cache_commit(2, boundary),
        CacheCommitOutcome::Inserted
    ));

    let mut after_boundary = crate::compositor::state::empty_cached_subsurface_commit();
    after_boundary.commit_id = SurfaceCommitId::for_tests(3);
    assert!(matches!(
        state.cache_commit(2, after_boundary),
        CacheCommitOutcome::Inserted
    ));

    assert_eq!(state.roles[&2].cached_commits.len(), 3);
    assert_eq!(
        state.roles[&2]
            .cached_commits
            .iter()
            .map(|commit| commit.commit_id)
            .collect::<Vec<_>>(),
        vec![
            SurfaceCommitId::for_tests(1),
            SurfaceCommitId::for_tests(2),
            SurfaceCommitId::for_tests(3),
        ]
    );
}

#[test]
fn desync_transition_does_not_create_a_synthetic_parent_content_update() {
    let mut state = CompositorState::default();
    let display = Display::<CompositorState>::new().expect("test display");
    let mut display_handle = display.handle();
    let (client, _peer) = test_client(&mut display_handle);
    let parent =
        state.test_create_unmapped_surface_resource_at_version(&client, &display_handle, 1);
    let child = state.test_create_unmapped_surface_resource_at_version(&client, &display_handle, 1);
    let grandchild =
        state.test_create_unmapped_surface_resource_at_version(&client, &display_handle, 1);
    let parent_id = compositor_surface_id(&parent);
    let child_id = compositor_surface_id(&child);
    let grandchild_id = compositor_surface_id(&grandchild);
    for surface_id in [parent_id, child_id, grandchild_id] {
        state.surface_presentation_generations.insert(surface_id, 1);
    }
    assert!(state.subsurface_transactions.register_with_client(
        child_id,
        parent_id,
        Some(client.id())
    ));
    assert!(state.subsurface_transactions.register_with_client(
        grandchild_id,
        child_id,
        Some(client.id())
    ));
    assert!(
        state
            .subsurface_transactions
            .set_mode(grandchild_id, SubsurfaceSyncMode::Desynchronized,)
    );

    let mut grandchild_commit = empty_cached_subsurface_commit();
    grandchild_commit.commit_id = SurfaceCommitId::for_tests(7);
    grandchild_commit.commit_sequence = SurfaceCommitSequence(7);
    grandchild_commit.pacing.commit_timing = Some(
        CommitTimingConstraint::from_protocol(client_pacing_now_ns() / 1_000_000_000 + 60, 0)
            .expect("future commit timing"),
    );
    assert!(matches!(
        state
            .subsurface_transactions
            .cache_commit(grandchild_id, grandchild_commit),
        CacheCommitOutcome::Inserted
    ));

    state.set_subsurface_sync_mode(child_id, SubsurfaceSyncMode::Desynchronized);

    assert_eq!(state.pending_surface_tree_transactions.len(), 1);
    assert_eq!(
        state.pending_surface_tree_transactions[0]
            .nodes
            .iter()
            .map(|(surface_id, _)| *surface_id)
            .collect::<Vec<_>>(),
        vec![grandchild_id],
    );
}

#[test]
fn converted_content_updates_keep_distinct_pacing_candidates() {
    let mut state = CompositorState::default();
    let display = Display::<CompositorState>::new().expect("test display");
    let mut display_handle = display.handle();
    let (client, _peer) = test_client(&mut display_handle);
    let parent =
        state.test_create_unmapped_surface_resource_at_version(&client, &display_handle, 1);
    let child = state.test_create_unmapped_surface_resource_at_version(&client, &display_handle, 1);
    let parent_id = compositor_surface_id(&parent);
    let child_id = compositor_surface_id(&child);
    for surface_id in [parent_id, child_id] {
        state.surface_presentation_generations.insert(surface_id, 1);
    }
    assert!(state.subsurface_transactions.register_with_client(
        child_id,
        parent_id,
        Some(client.id())
    ));

    let mut first = empty_cached_subsurface_commit();
    first.commit_id = SurfaceCommitId::for_tests(11);
    first.commit_sequence = SurfaceCommitSequence(11);
    first.pacing.fifo_set_barrier = true;
    assert!(matches!(
        state.subsurface_transactions.cache_commit(child_id, first),
        CacheCommitOutcome::Inserted
    ));

    let mut second = empty_cached_subsurface_commit();
    second.commit_id = SurfaceCommitId::for_tests(12);
    second.commit_sequence = SurfaceCommitSequence(12);
    second.pacing.fifo_set_barrier = true;
    second.pacing.commit_timing = Some(
        CommitTimingConstraint::from_protocol(client_pacing_now_ns() / 1_000_000_000 + 60, 0)
            .expect("future commit timing"),
    );
    assert!(matches!(
        state.subsurface_transactions.cache_commit(child_id, second),
        CacheCommitOutcome::Inserted
    ));

    state.set_subsurface_sync_mode(child_id, SubsurfaceSyncMode::Desynchronized);

    assert_eq!(
        state
            .surface_publications
            .get(&child_id)
            .and_then(|publication| publication.latest_published),
        Some(SurfaceCommitSequence(11)),
    );
    assert_eq!(state.pending_surface_tree_transactions.len(), 1);
    assert_eq!(state.pending_surface_tree_transactions[0].nodes.len(), 1);
    assert_eq!(
        state.pending_surface_tree_transactions[0].nodes[0]
            .1
            .commit_sequence,
        SurfaceCommitSequence(12),
    );
}

#[test]
fn cached_child_content_update_is_not_merged_after_parent_dependency_capture() {
    let mut state = SubsurfaceTransactionState::default();
    assert!(state.register(2, 1));
    assert!(state.register(3, 2));

    let mut first = crate::compositor::state::empty_cached_subsurface_commit();
    first.commit_id = SurfaceCommitId::for_tests(21);
    first.commit_sequence = SurfaceCommitSequence(21);
    assert!(matches!(
        state.cache_commit(3, first),
        CacheCommitOutcome::Inserted
    ));

    let mut parent_commit = crate::compositor::state::empty_cached_subsurface_commit();
    parent_commit.commit_id = SurfaceCommitId::for_tests(22);
    parent_commit.commit_sequence = SurfaceCommitSequence(22);
    assert!(matches!(
        state.cache_commit(2, parent_commit),
        CacheCommitOutcome::Inserted
    ));
    assert_eq!(
        state.capture_direct_child_dependencies(2),
        vec![ContentUpdateRef {
            surface_id: 3,
            commit_id: SurfaceCommitId::for_tests(21),
            commit_sequence: SurfaceCommitSequence(21),
        }],
    );

    let mut later = crate::compositor::state::empty_cached_subsurface_commit();
    later.commit_id = SurfaceCommitId::for_tests(23);
    later.commit_sequence = SurfaceCommitSequence(23);
    assert!(matches!(
        state.cache_commit(3, later),
        CacheCommitOutcome::Inserted
    ));

    assert_eq!(state.roles[&3].cached_commits.len(), 2);
    assert_eq!(
        state.roles[&3]
            .cached_commits
            .iter()
            .map(|commit| commit.commit_sequence)
            .collect::<Vec<_>>(),
        vec![SurfaceCommitSequence(21), SurfaceCommitSequence(23)],
    );
}

#[test]
fn content_update_lineage_captures_the_previous_surface_commit() {
    let mut state = CompositorState::default();
    state.surface_publications.insert(
        2,
        SurfacePublicationState {
            latest_received: SurfaceCommitSequence(30),
            ..SurfacePublicationState::default()
        },
    );

    let lineage = state.capture_content_update_lineage(
        2,
        SurfaceCommitId::for_tests(31),
        SurfaceCommitSequence(31),
    );

    assert_eq!(
        lineage.predecessor,
        Some(ContentUpdateRef {
            surface_id: 2,
            commit_id: SurfaceCommitId::for_tests(30),
            commit_sequence: SurfaceCommitSequence(30),
        }),
    );
}

#[test]
fn merging_content_updates_preserves_lineage_and_union_dependencies() {
    let predecessor = ContentUpdateRef {
        surface_id: 2,
        commit_id: SurfaceCommitId::for_tests(40),
        commit_sequence: SurfaceCommitSequence(40),
    };
    let first_dependency = ContentUpdateRef {
        surface_id: 3,
        commit_id: SurfaceCommitId::for_tests(41),
        commit_sequence: SurfaceCommitSequence(41),
    };
    let second_dependency = ContentUpdateRef {
        surface_id: 4,
        commit_id: SurfaceCommitId::for_tests(42),
        commit_sequence: SurfaceCommitSequence(42),
    };
    let mut older = empty_cached_subsurface_commit();
    older.commit_id = SurfaceCommitId::for_tests(43);
    older.commit_sequence = SurfaceCommitSequence(43);
    older.lineage.predecessor = Some(predecessor);
    older.lineage.child_dependencies = vec![first_dependency];
    let mut newer = empty_cached_subsurface_commit();
    newer.commit_id = SurfaceCommitId::for_tests(44);
    newer.commit_sequence = SurfaceCommitSequence(44);
    newer.lineage.predecessor = Some(ContentUpdateRef {
        surface_id: 2,
        commit_id: SurfaceCommitId::for_tests(43),
        commit_sequence: SurfaceCommitSequence(43),
    });
    newer.lineage.child_dependencies = vec![first_dependency, second_dependency];

    older.merge(newer);

    assert_eq!(older.commit_id, SurfaceCommitId::for_tests(44));
    assert_eq!(older.commit_sequence, SurfaceCommitSequence(44));
    assert_eq!(older.lineage.predecessor, Some(predecessor));
    assert_eq!(
        older.lineage.child_dependencies,
        vec![first_dependency, second_dependency],
    );
}

#[test]
fn cache_transitions_and_teardown_settle_accounting() {
    let mut state = SubsurfaceTransactionState::default();
    assert!(state.register(2, 1));
    assert!(state.register(3, 2));

    assert!(matches!(
        state.cache_commit(
            2,
            crate::compositor::state::empty_cached_subsurface_commit()
        ),
        CacheCommitOutcome::Inserted
    ));
    assert_eq!(state.cached_entry_count(), 1);
    assert_eq!(state.take_cached_commits_for_surface(2).len(), 1);
    assert_eq!(state.cached_entry_count(), 0);
    assert_eq!(state.cached_node_count(), 0);
    assert!(state.debug_accounting_is_consistent());

    assert!(matches!(
        state.cache_commit(
            3,
            crate::compositor::state::empty_cached_subsurface_commit()
        ),
        CacheCommitOutcome::Inserted
    ));
    assert!(state.set_mode(2, SubsurfaceSyncMode::Desynchronized));
    assert!(state.set_mode(3, SubsurfaceSyncMode::Desynchronized));
    assert_eq!(state.take_cached_commits_for_surface(3).len(), 1);
    assert_eq!(state.cached_entry_count(), 0);
    assert!(state.debug_accounting_is_consistent());

    assert!(matches!(
        state.cache_commit(
            3,
            crate::compositor::state::empty_cached_subsurface_commit()
        ),
        CacheCommitOutcome::Inserted
    ));
    assert_eq!(state.remove_subtree(2).len(), 1);
    assert_eq!(state.cached_entry_count(), 0);
    assert_eq!(state.cached_obligation_count(), 0);
    assert_eq!(state.cached_node_count(), 0);
    assert!(state.debug_accounting_is_consistent());
}

#[test]
fn global_entry_limit_rejects_without_eviction() {
    let mut state = SubsurfaceTransactionState::default();
    for surface_id in 2..=513 {
        assert!(state.register(surface_id, 1));
        for sequence in 0..MAX_SYNCHRONIZED_CACHED_COMMITS_PER_SURFACE {
            let mut commit = crate::compositor::state::empty_cached_subsurface_commit();
            commit.commit_id =
                SurfaceCommitId::for_tests(u64::from(surface_id) * 16 + sequence as u64);
            commit.commit_sequence =
                SurfaceCommitSequence(u64::from(surface_id) * 16 + sequence as u64);
            commit.pacing.fifo_set_barrier = true;
            assert!(matches!(
                state.cache_commit(surface_id, commit),
                CacheCommitOutcome::Inserted
            ));
        }
    }
    assert!(state.register(514, 1));

    let mut rejected = crate::compositor::state::empty_cached_subsurface_commit();
    rejected.commit_id = SurfaceCommitId::for_tests(9000);
    rejected.pacing.fifo_set_barrier = true;
    assert!(matches!(
        state.cache_commit(514, rejected),
        CacheCommitOutcome::Rejected {
            reason: CacheAdmissionFailure::TotalEntryLimit,
            ..
        }
    ));
    assert_eq!(
        state.cached_entry_count(),
        MAX_SYNCHRONIZED_CACHED_COMMITS_TOTAL
    );
    assert_eq!(
        state.maximum_cached_entries(),
        MAX_SYNCHRONIZED_CACHED_COMMITS_TOTAL
    );
    assert_eq!(state.roles[&2].cached_commits.len(), 8);
    assert_eq!(state.roles[&513].cached_commits.len(), 8);
    assert!(state.debug_accounting_is_consistent());
}

#[test]
fn aggregate_entry_high_watermark_tracks_all_cached_surfaces() {
    let mut state = SubsurfaceTransactionState::default();
    assert!(state.register(2, 1));
    assert!(state.register(3, 1));

    for surface_id in [2, 3] {
        let entry_count = if surface_id == 2 { 8 } else { 5 };
        for sequence in 0..entry_count {
            let mut commit = crate::compositor::state::empty_cached_subsurface_commit();
            commit.commit_id = SurfaceCommitId::for_tests(
                u64::from(surface_id) * 16 + u64::try_from(sequence).unwrap(),
            );
            commit.pacing.fifo_set_barrier = true;
            assert!(matches!(
                state.cache_commit(surface_id, commit),
                CacheCommitOutcome::Inserted
            ));
        }
    }

    assert_eq!(state.cached_entry_count(), 13);
    assert_eq!(state.maximum_cached_entries(), 13);
    assert_eq!(state.maximum_cached_entries_per_surface(), 8);

    assert_eq!(state.remove_role(2).len(), 8);
    assert_eq!(state.cached_entry_count(), 5);
    assert_eq!(state.maximum_cached_entries(), 13);
    assert_eq!(state.maximum_cached_entries_per_surface(), 8);
    assert!(state.debug_accounting_is_consistent());
}

#[test]
fn aggregate_obligation_high_watermark_tracks_all_cached_surfaces() {
    let display = Display::<crate::compositor::CompositorState>::new().expect("test display");
    let mut display_handle = display.handle();
    let (client, _peer) = test_client(&mut display_handle);
    let mut state = SubsurfaceTransactionState::default();
    assert!(state.register_with_client(2, 1, Some(client.id())));
    assert!(state.register_with_client(3, 1, Some(client.id())));

    for (surface_id, callback_count) in [(2, 600), (3, 500)] {
        let mut commit = crate::compositor::state::empty_cached_subsurface_commit();
        for _ in 0..callback_count {
            commit.frame_callbacks.push(
                client
                    .create_resource::<
                        wl_callback::WlCallback,
                        (),
                        crate::compositor::CompositorState,
                    >(&display_handle, 1, ())
                    .expect("callback resource"),
            );
        }
        assert!(matches!(
            state.cache_commit(surface_id, commit),
            CacheCommitOutcome::Inserted
        ));
    }

    assert_eq!(state.cached_obligation_count(), 1_100);
    assert_eq!(state.maximum_cached_obligations(), 1_100);
    assert_eq!(state.maximum_cached_obligations_per_surface(), 600);
    assert_eq!(state.maximum_cached_obligations_per_client(), 1_100);

    assert_eq!(state.remove_role(2).len(), 1);
    assert_eq!(state.cached_obligation_count(), 500);
    assert_eq!(state.maximum_cached_obligations(), 1_100);
    assert_eq!(state.maximum_cached_obligations_per_surface(), 600);
    assert_eq!(state.maximum_cached_obligations_per_client(), 1_100);
    assert!(state.debug_accounting_is_consistent());
}

#[test]
fn per_client_entry_limit_cannot_be_bypassed_by_many_surfaces() {
    let display = Display::<crate::compositor::CompositorState>::new().expect("test display");
    let mut display_handle = display.handle();
    let (client_a, _peer_a) = test_client(&mut display_handle);
    let (client_b, _peer_b) = test_client(&mut display_handle);
    let mut state = SubsurfaceTransactionState::default();

    for surface_id in 2..=33 {
        assert!(state.register_with_client(surface_id, 1, Some(client_a.id())));
        for sequence in 0..MAX_SYNCHRONIZED_CACHED_COMMITS_PER_SURFACE {
            let mut commit = crate::compositor::state::empty_cached_subsurface_commit();
            commit.commit_id =
                SurfaceCommitId::for_tests(u64::from(surface_id) * 16 + sequence as u64);
            commit.pacing.fifo_set_barrier = true;
            assert!(matches!(
                state.cache_commit(surface_id, commit),
                CacheCommitOutcome::Inserted
            ));
        }
    }
    assert_eq!(state.maximum_cached_entries_per_client(), 256);

    assert!(state.register_with_client(34, 1, Some(client_a.id())));
    let mut rejected = crate::compositor::state::empty_cached_subsurface_commit();
    rejected.commit_id = SurfaceCommitId::for_tests(9000);
    rejected.pacing.fifo_set_barrier = true;
    assert!(matches!(
        state.cache_commit(34, rejected),
        CacheCommitOutcome::Rejected {
            reason: CacheAdmissionFailure::PerClientEntryLimit,
            ..
        }
    ));
    assert_eq!(state.cached_entry_count(), 256);
    assert_eq!(state.roles[&34].cached_commits.len(), 0);

    assert!(state.register_with_client(35, 1, Some(client_b.id())));
    let mut other_client_commit = crate::compositor::state::empty_cached_subsurface_commit();
    other_client_commit.pacing.fifo_set_barrier = true;
    assert!(matches!(
        state.cache_commit(35, other_client_commit),
        CacheCommitOutcome::Inserted
    ));
    assert_eq!(state.cached_entry_count(), 257);
    assert!(state.debug_accounting_is_consistent());
}

#[test]
fn merge_path_is_bounded_by_retained_callback_obligations() {
    let display = Display::<crate::compositor::CompositorState>::new().expect("test display");
    let mut display_handle = display.handle();
    let (client, _peer) = test_client(&mut display_handle);
    let mut state = SubsurfaceTransactionState::default();
    assert!(state.register_with_client(2, 1, Some(client.id())));

    for _ in 0..MAX_SYNCHRONIZED_CACHED_OBLIGATIONS_PER_SURFACE {
        let callback = client
            .create_resource::<wl_callback::WlCallback, (), crate::compositor::CompositorState>(
                &display_handle,
                1,
                (),
            )
            .expect("callback resource");
        let mut commit = crate::compositor::state::empty_cached_subsurface_commit();
        commit.frame_callbacks.push(callback);
        assert!(matches!(
            state.cache_commit(2, commit),
            CacheCommitOutcome::Inserted | CacheCommitOutcome::Merged { .. }
        ));
    }

    assert_eq!(state.roles[&2].cached_commits.len(), 1);
    assert_eq!(
        state.cached_obligation_count(),
        MAX_SYNCHRONIZED_CACHED_OBLIGATIONS_PER_SURFACE
    );

    let callback = client
        .create_resource::<wl_callback::WlCallback, (), crate::compositor::CompositorState>(
            &display_handle,
            1,
            (),
        )
        .expect("rejected callback resource");
    let mut rejected = crate::compositor::state::empty_cached_subsurface_commit();
    rejected.frame_callbacks.push(callback);
    assert!(matches!(
        state.cache_commit(2, rejected),
        CacheCommitOutcome::Rejected {
            reason: CacheAdmissionFailure::PerSurfaceObligationLimit,
            ..
        }
    ));
    assert_eq!(state.roles[&2].cached_commits.len(), 1);
    assert_eq!(
        state.cached_obligation_count(),
        MAX_SYNCHRONIZED_CACHED_OBLIGATIONS_PER_SURFACE
    );
    assert!(state.debug_accounting_is_consistent());
}
