use super::*;
use std::num::NonZeroU64;

fn id(value: u64) -> WindowId {
    WindowId::new(NonZeroU64::new(value).expect("nonzero test id"))
}

#[test]
fn revision_split_join_round_trips_boundaries() {
    for value in [0, 1, u32::MAX as u64, u32::MAX as u64 + 1, u64::MAX] {
        let (high, low) = split_u64(value);
        assert_eq!(join_u64(high, low), value);
    }
}

#[test]
fn snapshot_strings_are_bounded_at_utf8_boundaries() {
    let snapshot = AstreaToplevelSnapshot::bounded(
        id(1),
        Some(&"é".repeat(MAX_ASTREA_TOPLEVEL_APP_ID_BYTES)),
        Some(&"😀".repeat(MAX_ASTREA_TOPLEVEL_TITLE_BYTES)),
        None,
        AstreaToplevelKind::XdgToplevel,
        AstreaToplevelStates::default(),
        0,
    );
    assert!(snapshot.app_id.len() <= MAX_ASTREA_TOPLEVEL_APP_ID_BYTES);
    assert!(snapshot.title.len() <= MAX_ASTREA_TOPLEVEL_TITLE_BYTES);
    assert!(snapshot.app_id.is_char_boundary(snapshot.app_id.len()));
    assert!(snapshot.title.is_char_boundary(snapshot.title.len()));
}

#[test]
fn collection_keeps_the_lowest_bounded_prefix() {
    let mut collection = AstreaToplevelCollection::default();
    for value in (1..=MAX_ASTREA_TOPLEVELS_PER_MANAGER + 2).rev() {
        collection.total = collection.total.saturating_add(1);
        collection.eligible_ids.insert(id(value as u64));
        collection.snapshots.insert(
            id(value as u64),
            AstreaToplevelSnapshot::bounded(
                id(value as u64),
                None,
                None,
                None,
                AstreaToplevelKind::XdgToplevel,
                AstreaToplevelStates::default(),
                0,
            ),
        );
        if collection.snapshots.len() > MAX_ASTREA_TOPLEVELS_PER_MANAGER {
            let largest = collection.snapshots.keys().next_back().copied().unwrap();
            collection.snapshots.remove(&largest);
        }
    }
    assert_eq!(
        collection.total,
        (MAX_ASTREA_TOPLEVELS_PER_MANAGER + 2) as u32
    );
    assert_eq!(collection.snapshots.len(), MAX_ASTREA_TOPLEVELS_PER_MANAGER);
    assert_eq!(
        collection.eligible_ids.len(),
        MAX_ASTREA_TOPLEVELS_PER_MANAGER + 2
    );
    assert_eq!(collection.snapshots.keys().next().unwrap().get(), 1);
    assert_eq!(
        collection.snapshots.keys().next_back().unwrap().get(),
        MAX_ASTREA_TOPLEVELS_PER_MANAGER as u64
    );
}

#[test]
fn dirty_windows_are_coalesced_and_bounded() {
    let mut publisher = AstreaToplevelPublisher::default();
    publisher.mark_window_dirty(id(1));
    publisher.mark_window_dirty(id(1));
    publisher.mark_window_dirty(id(2));

    assert_eq!(publisher.dirty_window_ids(), vec![id(1), id(2)]);
    assert_eq!(publisher.metrics.dirty_windows_queued, 2);
    assert_eq!(publisher.metrics.dirty_updates_coalesced, 1);
}

#[test]
fn action_tokens_reject_manager_scoped_duplicates_and_reuse_released_tokens() {
    let mut tracker = AstreaActionTracker::default();
    let mut other_manager = AstreaActionTracker::default();
    let token = AstreaActionToken::new(7, 11);

    assert_eq!(
        tracker.reserve(token, AstreaToplevelAction::Activate, id(1)),
        Ok(())
    );
    assert_eq!(
        tracker.can_reserve(token),
        Err(AstreaActionBeginError::Duplicate)
    );
    assert_eq!(
        tracker.reserve(token, AstreaToplevelAction::Close, id(2)),
        Err(AstreaActionBeginError::Duplicate)
    );
    assert_eq!(
        other_manager.reserve(token, AstreaToplevelAction::Close, id(2)),
        Ok(())
    );
    assert_eq!(
        tracker.release(token),
        Some(PendingAstreaAction {
            token,
            action: AstreaToplevelAction::Activate,
            window_id: id(1),
        })
    );
    assert_eq!(
        tracker.reserve(token, AstreaToplevelAction::Close, id(2)),
        Ok(())
    );
}

#[test]
fn action_tokens_have_a_bounded_pending_capacity() {
    let mut tracker = AstreaActionTracker::default();

    for value in 1..=MAX_ASTREA_PENDING_ACTIONS {
        assert_eq!(
            tracker.reserve(
                AstreaActionToken::new(0, value as u32),
                AstreaToplevelAction::Activate,
                id(value as u64),
            ),
            Ok(())
        );
    }

    assert_eq!(
        tracker.can_reserve(AstreaActionToken::new(
            0,
            (MAX_ASTREA_PENDING_ACTIONS + 1) as u32,
        )),
        Err(AstreaActionBeginError::Limit)
    );
    assert_eq!(
        tracker.reserve(
            AstreaActionToken::new(0, (MAX_ASTREA_PENDING_ACTIONS + 1) as u32),
            AstreaToplevelAction::Activate,
            id((MAX_ASTREA_PENDING_ACTIONS + 1) as u64),
        ),
        Err(AstreaActionBeginError::Limit)
    );
}

#[test]
fn action_tracker_clear_releases_manager_state() {
    let mut tracker = AstreaActionTracker::default();
    let first = AstreaActionToken::new(0, 1);
    let second = AstreaActionToken::new(0, 2);
    tracker
        .reserve(first, AstreaToplevelAction::Activate, id(9))
        .unwrap();
    tracker
        .reserve(second, AstreaToplevelAction::Close, id(10))
        .unwrap();

    assert_eq!(tracker.pending_len(), 2);
    tracker.clear();
    assert_eq!(tracker.pending_len(), 0);
    assert_eq!(
        tracker.reserve(first, AstreaToplevelAction::Restore, id(11)),
        Ok(())
    );
}

#[test]
fn first_reconciliation_is_the_only_unprompted_full_scan() {
    let mut publisher = AstreaToplevelPublisher::default();
    assert!(publisher.needs_full_reconciliation());
    publisher.initial_reconciliation_pending = false;
    assert!(!publisher.needs_full_reconciliation());
    publisher.mark_window_dirty(id(1));
    assert!(!publisher.needs_full_reconciliation());
}

fn collection_with_windows(count: usize) -> AstreaToplevelCollection {
    let mut collection = AstreaToplevelCollection::default();
    for value in 1..=count {
        let window_id = id(value as u64);
        let snapshot = AstreaToplevelSnapshot::bounded(
            window_id,
            Some("app"),
            Some("title"),
            Some(42),
            AstreaToplevelKind::XdgToplevel,
            AstreaToplevelStates::default(),
            0,
        );
        collection.eligible_ids.insert(window_id);
        collection.snapshots.insert(window_id, snapshot);
    }
    collection.total = count as u32;
    collection
}

#[test]
fn publication_transactions_keep_one_revision_across_all_bounded_chunks() {
    for count in [256, 257, 512, 4096] {
        let mut publisher = AstreaToplevelPublisher::default();
        let target = collection_with_windows(count);
        publisher.start_transaction(target);
        let revision = publisher.transaction.as_ref().unwrap().revision;
        let mut chunks = 0;
        while !publisher
            .transaction
            .as_ref()
            .unwrap()
            .remaining_ids
            .is_empty()
        {
            let ids = publisher.next_publication_ids();
            assert!(!ids.is_empty());
            assert!(ids.len() <= MAX_ASTREA_TOPLEVEL_UPDATES_PER_CYCLE);
            assert_eq!(publisher.transaction.as_ref().unwrap().revision, revision);
            for window_id in ids {
                publisher
                    .transaction
                    .as_mut()
                    .unwrap()
                    .remaining_ids
                    .remove(&window_id);
            }
            chunks += 1;
        }
        assert_eq!(publisher.revision, revision);
        assert!(publisher.canonical.is_empty());
        assert_eq!(
            chunks,
            count.div_ceil(MAX_ASTREA_TOPLEVEL_UPDATES_PER_CYCLE)
        );
    }
}

#[test]
fn changes_during_a_transaction_are_queued_for_a_follow_up_target() {
    let mut publisher = AstreaToplevelPublisher::default();
    let target = collection_with_windows(257);
    publisher.start_transaction(target.clone());
    let changed = AstreaToplevelSnapshot::bounded(
        id(1),
        Some("new-app"),
        Some("new-title"),
        Some(7),
        AstreaToplevelKind::XdgToplevel,
        AstreaToplevelStates::ACTIVE,
        1,
    );
    publisher.queue_follow_up(None, BTreeMap::from([(id(1), Some(changed.clone()))]));
    assert_eq!(
        publisher.transaction.as_ref().unwrap().target,
        target,
        "the active target is immutable"
    );
    assert_eq!(
        publisher.next_dirty_snapshots.get(&id(1)),
        Some(&Some(changed))
    );
    assert!(publisher.has_pending_publication());
}
