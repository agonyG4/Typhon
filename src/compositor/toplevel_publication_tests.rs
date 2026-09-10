use super::*;
use std::num::NonZeroU64;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use wayland_client::Proxy;
use wayland_server::Display;

struct AnchorTestOwners {
    _display: Display<()>,
    _clients: Vec<wayland_server::Client>,
    _peers: Vec<UnixStream>,
    identities: Vec<(ClientId, ObjectId)>,
}

impl AnchorTestOwners {
    fn new(count: usize) -> Self {
        let display = Display::<()>::new().expect("anchor test display");
        let mut clients = Vec::with_capacity(count);
        let mut peers = Vec::with_capacity(count);
        let mut identities = Vec::with_capacity(count);
        for _ in 0..count {
            let (server_stream, peer) = UnixStream::pair().expect("anchor test socket pair");
            let client = display
                .handle()
                .insert_client(server_stream, Arc::new(()))
                .expect("anchor test client");
            let client_id = client.id();
            let resource_id = display
                .handle()
                .backend_handle()
                .object_for_protocol_id(
                    client_id.clone(),
                    wayland_client::protocol::wl_display::WlDisplay::interface(),
                    1,
                )
                .expect("anchor test display resource");
            identities.push((client_id, resource_id));
            clients.push(client);
            peers.push(peer);
        }
        Self {
            _display: display,
            _clients: clients,
            _peers: peers,
            identities,
        }
    }
}

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
fn minimize_anchor_state_accepts_negative_coordinates_and_rejects_invalid_dimensions() {
    let owners = AnchorTestOwners::new(1);
    let (client_id, resource_id) = owners.identities[0].clone();
    let window_id = id(10);
    let mut publisher = AstreaToplevelPublisher::default();
    let valid = MinimizeAnchorRect {
        x: -300,
        y: 900,
        width: 64,
        height: 64,
    };

    assert_eq!(
        publisher.set_minimize_anchor(client_id.clone(), resource_id.clone(), window_id, valid),
        Ok(())
    );
    assert_eq!(publisher.minimize_anchor_for_test(window_id), Some(valid));
    for invalid in [
        MinimizeAnchorRect { width: 0, ..valid },
        MinimizeAnchorRect { height: 0, ..valid },
        MinimizeAnchorRect {
            width: MAX_MINIMIZE_ANCHOR_DIMENSION + 1,
            ..valid
        },
        MinimizeAnchorRect {
            height: MAX_MINIMIZE_ANCHOR_DIMENSION + 1,
            ..valid
        },
    ] {
        assert_eq!(
            publisher.set_minimize_anchor(
                client_id.clone(),
                resource_id.clone(),
                window_id,
                invalid
            ),
            Err(())
        );
        assert_eq!(publisher.minimize_anchor_for_test(window_id), Some(valid));
    }
}

#[test]
fn minimize_anchor_state_replaces_same_owner_and_ignores_stale_owner_teardown() {
    let owners = AnchorTestOwners::new(2);
    let (client_a, resource_a) = owners.identities[0].clone();
    let (client_b, resource_b) = owners.identities[1].clone();
    let window_id = id(11);
    let rect_a = MinimizeAnchorRect {
        x: 1,
        y: 2,
        width: 32,
        height: 32,
    };
    let rect_a_replaced = MinimizeAnchorRect {
        x: 3,
        y: 4,
        width: 40,
        height: 40,
    };
    let rect_b = MinimizeAnchorRect {
        x: 5,
        y: 6,
        width: 48,
        height: 48,
    };
    let mut publisher = AstreaToplevelPublisher::default();

    publisher
        .set_minimize_anchor(client_a.clone(), resource_a.clone(), window_id, rect_a)
        .unwrap();
    publisher
        .set_minimize_anchor(
            client_a.clone(),
            resource_a.clone(),
            window_id,
            rect_a_replaced,
        )
        .unwrap();
    assert_eq!(
        publisher.minimize_anchor_for_test(window_id),
        Some(rect_a_replaced)
    );
    publisher
        .set_minimize_anchor(client_b.clone(), resource_b.clone(), window_id, rect_b)
        .unwrap();
    publisher.remove_handle(&client_a, &resource_a, window_id, &resource_a);
    assert_eq!(publisher.minimize_anchor_for_test(window_id), Some(rect_b));
    publisher.remove_handle(&client_b, &resource_b, window_id, &resource_b);
    assert_eq!(publisher.minimize_anchor_for_test(window_id), None);
}

#[test]
fn minimize_anchor_state_client_and_window_teardown_are_scoped() {
    let owners = AnchorTestOwners::new(2);
    let (client_a, resource_a) = owners.identities[0].clone();
    let (client_b, resource_b) = owners.identities[1].clone();
    let window_a = id(12);
    let window_b = id(13);
    let mut publisher = AstreaToplevelPublisher::default();
    publisher
        .set_minimize_anchor(
            client_a.clone(),
            resource_a.clone(),
            window_a,
            MinimizeAnchorRect {
                x: 1,
                y: 1,
                width: 24,
                height: 24,
            },
        )
        .unwrap();
    publisher
        .set_minimize_anchor(
            client_b.clone(),
            resource_b.clone(),
            window_b,
            MinimizeAnchorRect {
                x: 2,
                y: 2,
                width: 24,
                height: 24,
            },
        )
        .unwrap();
    publisher.remove_client(&client_a);
    assert_eq!(publisher.minimize_anchor_for_test(window_a), None);
    assert!(publisher.minimize_anchor_for_test(window_b).is_some());

    publisher.mark_window_removed(window_b);
    assert_eq!(publisher.minimize_anchor_for_test(window_b), None);
}

#[test]
fn clean_publication_gate_skips_reconcile_and_pruning() {
    let mut publisher = AstreaToplevelPublisher {
        initial_reconciliation_pending: false,
        ..AstreaToplevelPublisher::default()
    };

    assert!(!publisher.should_reconcile());
    assert_eq!(publisher.metrics.publication_gate_checks, 1);
    assert_eq!(publisher.metrics.publication_clean_gate_skips, 1);
    assert_eq!(publisher.metrics.reconcile_calls, 0);
    assert_eq!(publisher.metrics.prune_passes, 0);

    publisher.mark_window_dirty(id(1));
    assert!(publisher.should_reconcile());
}

#[test]
fn action_tokens_reject_manager_scoped_duplicates_and_reuse_released_tokens() {
    assert_eq!(
        crate::compositor::toplevel_actions::MAX_ASTREA_PENDING_ACTIONS,
        64
    );
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
