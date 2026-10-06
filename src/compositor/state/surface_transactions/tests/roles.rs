use super::*;
use crate::compositor::subsurface::*;

#[test]
fn new_role_defaults_to_synchronized() {
    let mut state = SurfaceTransactionState::default();
    assert!(state.register(2, 1));
    assert_eq!(
        state.requested_mode(2),
        Some(SubsurfaceSyncMode::Synchronized)
    );
    assert!(state.is_effectively_synchronized(2));
}

#[test]
fn set_sync_and_set_desync_record_requested_mode() {
    let mut state = SurfaceTransactionState::default();
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
    let mut state = SurfaceTransactionState::default();
    assert!(state.register(2, 1));
    assert!(state.register(3, 2));
    assert!(state.set_mode(3, SubsurfaceSyncMode::Desynchronized));
    assert!(state.is_effectively_synchronized(3));
    assert!(state.set_mode(2, SubsurfaceSyncMode::Desynchronized));
    assert!(!state.is_effectively_synchronized(3));
}

#[test]
fn role_registration_rejects_reuse_and_cycles() {
    let mut state = SurfaceTransactionState::default();
    assert!(state.register(2, 1));
    assert!(!state.register(2, 3));
    assert!(!state.register(1, 2));
}

#[test]
fn role_destruction_removes_only_that_role_while_surface_teardown_removes_subtree() {
    let mut state = SurfaceTransactionState::default();
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
    let mut state = SurfaceTransactionState::default();
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
