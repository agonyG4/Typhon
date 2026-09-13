use super::*;

#[test]
fn new_subsurface_pending_stack_uses_latched_baseline() {
    let mut state = CompositorState::default();
    state.committed_subsurface_stacks.insert(1, vec![1, 2, 3]);
    state.latched_subsurface_stacks.insert(1, vec![1, 3, 2]);

    state.add_subsurface_to_pending_stack(1, 4);

    assert_eq!(state.latched_subsurface_stacks[&1], vec![1, 3, 2]);
    assert_eq!(state.pending_subsurface_stacks[&1], vec![1, 3, 2, 4]);
}

#[test]
fn new_subsurface_pending_stack_preserves_existing_restack() {
    let mut state = CompositorState::default();
    state.pending_subsurface_stacks.insert(1, vec![1, 3, 2]);

    state.add_subsurface_to_pending_stack(1, 4);

    assert_eq!(state.pending_subsurface_stacks[&1], vec![1, 3, 2, 4]);

    state.add_subsurface_to_pending_stack(1, 4);

    assert_eq!(state.pending_subsurface_stacks[&1], vec![1, 3, 2, 4]);
}

#[test]
fn wayland_client_can_create_subsurface_on_oblivion_server() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let result = create_client_subsurface(&socket_path);
    stop_test_server(running, server_thread);

    result.unwrap();
}

#[test]
fn wayland_client_subsurface_commit_tracks_parent_relative_position() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let result = create_client_toplevel_with_positioned_subsurface_buffer(&socket_path);
    let server = stop_test_server(running, server_thread);

    result.unwrap();
    let child = server
        .renderable_surfaces()
        .iter()
        .find(|surface| surface.width == 1 && surface.height == 1)
        .expect("child subsurface snapshot should be renderable");
    let parent = server
        .renderable_surfaces()
        .iter()
        .find(|surface| surface.width == 2 && surface.height == 2)
        .expect("parent toplevel snapshot should be renderable");

    assert_eq!(
        child.placement,
        SurfacePlacement::subsurface(parent.surface_id, 10, 12)
    );
}

#[test]
fn subsurface_committed_before_parent_stays_above_parent_when_parent_maps() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    create_subsurface_buffer_before_parent_buffer(&socket_path).unwrap();
    let server = stop_test_server(running, server_thread);

    let parent_index = server
        .renderable_surfaces()
        .iter()
        .position(|surface| surface.width == 2 && surface.height == 2)
        .expect("parent surface should be renderable");
    let child_index = server
        .renderable_surfaces()
        .iter()
        .position(|surface| surface.width == 1 && surface.height == 1)
        .expect("subsurface should be renderable");

    assert!(child_index > parent_index);
}

#[test]
fn gecko_pre_role_surface_is_adopted_as_single_subsurface_node() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let snapshots = capture_gecko_pre_role_subsurface_adoption(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(snapshots.after_roleless_commit.is_empty());
    let parent = snapshots
        .after_adoption
        .iter()
        .find(|surface| surface.width == 1992 && surface.height == 1189)
        .expect("parent should be renderable");
    let child_nodes = snapshots
        .after_adoption
        .iter()
        .filter(|surface| surface.width == 1920 && surface.height == 1080)
        .collect::<Vec<_>>();
    assert_eq!(child_nodes.len(), 1);
    assert_eq!(child_nodes[0].parent_surface_id, Some(parent.surface_id));
    let parent_index = snapshots
        .after_adoption
        .iter()
        .position(|surface| surface.surface_id == parent.surface_id)
        .expect("parent should be renderable");
    let child_index = snapshots
        .after_adoption
        .iter()
        .position(|surface| surface.surface_id == child_nodes[0].surface_id)
        .expect("child should be renderable");
    assert!(child_index > parent_index);
}

#[test]
fn default_synchronized_child_is_invisible_until_parent_commit() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let SynchronizedCommitSnapshots {
        before_parent,
        after_parent,
        before_child_generation,
        after_child_generation,
        after_parent_generation,
    } = capture_default_synchronized_child_before_and_after_parent_commit(&socket_path, &commands)
        .unwrap();
    let server = stop_controllable_test_server(commands, server_thread);

    assert!(before_parent.is_empty());
    assert_eq!(after_parent.len(), 2);
    assert_eq!(after_parent[0].width, 20);
    assert_eq!(after_parent[1].width, 11);
    assert_eq!(after_child_generation, before_child_generation);
    assert_eq!(after_parent_generation, before_child_generation + 1);
    let metrics = server.subsurface_transaction_metrics();
    assert_eq!(metrics.synchronized_child_commits_cached, 1);
    assert_eq!(metrics.cached_commits_appended, 1);
    assert_eq!(metrics.cached_commits_merged, 0);
    assert_eq!(metrics.cached_commits_rejected, 0);
    assert_eq!(metrics.current_cached_entries, 0);
    assert_eq!(metrics.maximum_cached_entries, 1);
    assert_eq!(metrics.tree_transactions_published, 2);
    assert_eq!(metrics.maximum_cached_nodes, 1);
    assert_eq!(metrics.synchronized_child_immediate_publish_attempts, 0);
}

#[test]
fn subsurface_position_changes_only_on_parent_commit() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let (before_parent, after_parent) =
        capture_subsurface_position_before_and_after_parent_commit(&socket_path, &commands)
            .unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);
    let before_child = before_parent
        .iter()
        .find(|surface| surface.parent_surface_id.is_some())
        .unwrap();
    let after_child = after_parent
        .iter()
        .find(|surface| surface.parent_surface_id.is_some())
        .unwrap();

    assert_eq!((before_child.local_x, before_child.local_y), (0, 0));
    assert_eq!((after_child.local_x, after_child.local_y), (30, 40));
}

#[test]
fn delayed_parent_commit_uses_position_captured_at_commit_boundary() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind_native_base(&socket_name).unwrap();
    server.set_presentation_clock(PresentationClock::Monotonic);
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let (after_delayed_parent, after_next_parent) =
        capture_delayed_parent_position_snapshot(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);
    let child_position = |surfaces: &[RenderableSurfaceSnapshot]| {
        surfaces
            .iter()
            .find(|surface| surface.parent_surface_id.is_some())
            .map(|surface| (surface.local_x, surface.local_y))
    };

    assert_eq!(child_position(&after_delayed_parent), Some((10, 10)));
    assert_eq!(child_position(&after_next_parent), Some((20, 20)));
}

#[test]
fn pending_subsurface_restack_survives_new_child_creation() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let snapshot =
        capture_pending_restack_survives_new_subsurface(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    let order = snapshot
        .iter()
        .map(|surface| (surface.width, surface.height))
        .collect::<Vec<_>>();
    assert_eq!(order, vec![(20, 15), (7, 7), (6, 6), (8, 8)]);
    let unique_surface_count = snapshot
        .iter()
        .map(|surface| surface.surface_id)
        .collect::<std::collections::HashSet<_>>()
        .len();
    assert_eq!(unique_surface_count, snapshot.len());
}

#[test]
fn delayed_parent_stack_lineage_survives_new_child_creation() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind_native_base(&socket_name).unwrap();
    server.set_presentation_clock(PresentationClock::Monotonic);
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let snapshots =
        capture_delayed_parent_restack_with_new_subsurface_stack_states(&socket_path, &commands)
            .unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);
    let first_restack = snapshots
        .after_first_parent_commit
        .latched
        .clone()
        .expect("first parent commit should latch a subsurface stack");
    let parent_id = first_restack[0];
    let second_id = first_restack[1];
    let first_id = first_restack[2];
    let third_id = snapshots
        .after_child_creation
        .committed
        .as_ref()
        .and_then(|stack| stack.last().copied())
        .expect("new child should be in committed stack");
    let initial = vec![parent_id, first_id, second_id];

    assert_eq!(snapshots.after_first_parent_commit.committed, Some(initial));
    assert_eq!(
        snapshots.after_first_parent_commit.latched,
        Some(first_restack.clone())
    );
    assert_eq!(snapshots.after_first_parent_commit.pending, None);
    assert_eq!(
        snapshots.after_child_creation.latched,
        Some(first_restack.clone())
    );
    assert_eq!(
        snapshots.after_child_creation.pending,
        Some(vec![parent_id, second_id, first_id, third_id])
    );
    assert_eq!(
        snapshots.after_second_restack.pending,
        Some(vec![parent_id, first_id, second_id, third_id])
    );
    assert_eq!(
        snapshots.after_first_publication.committed,
        Some(first_restack.clone())
    );
    assert_eq!(
        snapshots.after_first_publication.latched,
        Some(first_restack)
    );
    assert_eq!(
        snapshots.after_first_publication.pending,
        Some(vec![parent_id, first_id, second_id, third_id])
    );
    assert_eq!(
        snapshots.after_second_publication.committed,
        Some(vec![parent_id, first_id, second_id, third_id])
    );
    assert_eq!(
        snapshots.after_second_publication.latched,
        snapshots.after_second_publication.committed
    );
    assert_eq!(snapshots.after_second_publication.pending, None);
}

#[test]
fn multiple_new_subsurfaces_remain_topmost_in_creation_order() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let (_, before, after_creation, after_commit) =
        capture_multiple_new_subsurface_stack_states(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);
    let baseline = before
        .committed
        .clone()
        .expect("initial parent stack should be committed");
    let expected = after_creation
        .pending
        .clone()
        .expect("new children should be pending");

    assert_eq!(expected.len(), baseline.len() + 2);
    assert_eq!(&expected[..baseline.len()], baseline.as_slice());
    assert_eq!(
        expected
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        expected.len()
    );
    assert_eq!(after_creation.committed, Some(expected.clone()));
    assert_eq!(after_commit.committed, Some(expected.clone()));
    assert_eq!(after_commit.latched, Some(expected));
    assert_eq!(after_commit.pending, None);
}

#[test]
fn destroying_new_subsurface_before_parent_commit_preserves_surviving_stack() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let (_, before_destroy, after_destroy, after_commit) =
        capture_destroyed_new_subsurface_stack_states(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);
    let surviving = after_destroy
        .committed
        .clone()
        .expect("surviving stack should remain committed");

    assert_eq!(surviving.len(), 3);
    assert_eq!(
        surviving
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        surviving.len()
    );
    assert_eq!(after_destroy.latched, Some(surviving.clone()));
    assert_eq!(after_destroy.pending, Some(surviving.clone()));
    assert_eq!(after_commit.committed, Some(surviving.clone()));
    assert_eq!(after_commit.latched, Some(surviving));
    assert_eq!(after_commit.pending, None);
    assert_ne!(before_destroy.committed, after_destroy.committed);
}

#[test]
fn delayed_parent_restack_uses_latest_latched_stack_as_next_baseline() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind_native_base(&socket_name).unwrap();
    server.set_presentation_clock(PresentationClock::Monotonic);
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let (after_first, after_second) =
        capture_delayed_parent_restack_snapshots(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);
    let order = |surfaces: &[RenderableSurfaceSnapshot]| {
        surfaces
            .iter()
            .map(|surface| (surface.width, surface.height))
            .collect::<Vec<_>>()
    };

    assert_eq!(order(&after_first), vec![(20, 15), (7, 7), (6, 6)]);
    assert_eq!(order(&after_second), vec![(20, 15), (6, 6), (7, 7)]);
}

#[test]
fn multiple_synchronized_child_commits_publish_only_the_latest_buffer() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let MultipleSynchronizedCommitSnapshots {
        before_parent,
        after_parent,
        superseded_buffer_releases,
    } = capture_multiple_synchronized_child_commits(&socket_path, &commands).unwrap();
    let server = stop_controllable_test_server(commands, server_thread);
    let before_child = before_parent
        .iter()
        .find(|surface| surface.parent_surface_id.is_some())
        .unwrap();
    let after_child = after_parent
        .iter()
        .find(|surface| surface.parent_surface_id.is_some())
        .unwrap();

    assert_eq!((before_child.width, before_child.height), (5, 5));
    assert_eq!((after_child.width, after_child.height), (13, 9));
    assert_ne!(after_child.buffer_id, before_child.buffer_id);
    assert_eq!(superseded_buffer_releases, 1);
    let metrics = server.subsurface_transaction_metrics();
    assert_eq!(metrics.cached_commits_appended, 2);
    assert_eq!(metrics.cached_commits_merged, 1);
    assert_eq!(metrics.cached_commits_rejected, 0);
    assert_eq!(metrics.current_cached_entries, 0);
    assert_eq!(metrics.maximum_cached_entries, 1);
    assert_eq!(metrics.maximum_cached_nodes, 1);
}

#[test]
fn set_desync_publishes_cached_state_when_no_ancestor_remains_synchronized() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let (before_desync, after_desync) =
        capture_cached_child_before_and_after_set_desync(&socket_path, &commands).unwrap();
    let server = stop_controllable_test_server(commands, server_thread);
    let child_size = |surfaces: &[RenderableSurfaceSnapshot]| {
        surfaces
            .iter()
            .find(|surface| surface.parent_surface_id.is_some())
            .map(|surface| (surface.width, surface.height))
    };

    assert_eq!(child_size(&before_desync), Some((5, 5)));
    assert_eq!(child_size(&after_desync), Some((9, 7)));
    assert_eq!(
        server
            .subsurface_transaction_metrics()
            .current_cached_entries,
        0
    );
}

#[test]
fn desynchronized_grandchild_under_synchronized_ancestor_latches_with_root() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let (before_root, after_root) =
        capture_effectively_synchronized_grandchild_update(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);
    let deepest_size = |surfaces: &[RenderableSurfaceSnapshot]| {
        let child_ids = surfaces
            .iter()
            .filter_map(|surface| surface.parent_surface_id)
            .collect::<std::collections::HashSet<_>>();
        surfaces
            .iter()
            .find(|surface| {
                surface.parent_surface_id.is_some() && !child_ids.contains(&surface.surface_id)
            })
            .map(|surface| (surface.width, surface.height))
    };

    assert_eq!(deepest_size(&before_root), Some((3, 3)));
    assert_eq!(deepest_size(&after_root), Some((9, 5)));
}

#[test]
fn root_resize_publishes_content_and_synchronized_decorations_in_one_generation() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let (before_root, after_root) =
        capture_decorated_tree_during_root_resize_commit(&socket_path, &commands).unwrap();
    let server = stop_controllable_test_server(commands, server_thread);
    let before_children = before_root
        .iter()
        .filter(|surface| surface.parent_surface_id.is_some())
        .map(|surface| (surface.width, surface.height))
        .collect::<Vec<_>>();
    assert!(before_children.contains(&(300, 20)));
    assert!(before_children.contains(&(10, 180)));

    let root = after_root
        .iter()
        .find(|surface| surface.parent_surface_id.is_none())
        .unwrap();
    let after_children = after_root
        .iter()
        .filter(|surface| surface.parent_surface_id.is_some())
        .collect::<Vec<_>>();
    assert_eq!((root.width, root.height), (340, 230));
    assert!(root.resize_preview_active);
    assert!(
        after_children
            .iter()
            .any(|surface| (surface.width, surface.height) == (340, 20))
    );
    assert!(
        after_children
            .iter()
            .any(|surface| (surface.width, surface.height) == (10, 210))
    );
    assert!(
        after_children
            .iter()
            .all(|surface| surface.generation == root.generation)
    );
    assert_eq!(
        server.subsurface_transaction_metrics().maximum_cached_nodes,
        2
    );
}

#[test]
fn synchronized_child_frame_callback_waits_for_parent_tree_presentation() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let (before_parent, after_parent) =
        capture_synchronized_child_frame_callback_lifecycle(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(!before_parent);
    assert!(after_parent);
}

#[test]
fn root_commit_without_cached_child_keeps_old_child_until_next_parent_commit() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let snapshots =
        capture_root_commit_before_synchronized_child_update(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);
    let child_size = |surfaces: &[RenderableSurfaceSnapshot]| {
        surfaces
            .iter()
            .find(|surface| surface.parent_surface_id.is_some())
            .map(|surface| (surface.width, surface.height))
    };

    assert_eq!(child_size(&snapshots.after_root), Some((5, 5)));
    assert_eq!(
        child_size(&snapshots.after_child_without_parent),
        Some((5, 5))
    );
    assert_eq!(child_size(&snapshots.after_next_parent), Some((9, 7)));
}

#[test]
fn subsurface_restack_rejects_non_sibling_reference() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let result = create_subsurface_with_invalid_restack_reference(&socket_path);
    stop_test_server(running, server_thread);

    assert!(result.is_err());
}

#[test]
fn repeated_subsurface_restack_keeps_subtree_contiguous_and_teardown_cleans_stack() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let result = create_repeated_restack_then_destroy_subsurface(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);
    let (reordered, after_destroy) = result;

    let sizes = reordered
        .iter()
        .map(|surface| (surface.width, surface.height))
        .collect::<Vec<_>>();
    assert_eq!(sizes, vec![(160, 120), (81, 81), (80, 80), (40, 40)]);
    let unique_surface_count = reordered
        .iter()
        .map(|surface| surface.surface_id)
        .collect::<std::collections::HashSet<_>>()
        .len();
    assert_eq!(unique_surface_count, reordered.len());
    assert_eq!(
        reordered[3].parent_surface_id,
        Some(reordered[2].surface_id)
    );

    let remaining_sizes = after_destroy
        .iter()
        .map(|surface| (surface.width, surface.height))
        .collect::<Vec<_>>();
    assert_eq!(remaining_sizes, vec![(160, 120), (81, 81)]);
}

#[test]
fn wayland_surface_attach_null_unmaps_renderable_surface() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    create_toplevel_then_attach_null_buffer(&socket_path).unwrap();
    let server = stop_test_server(running, server_thread);

    assert!(server.renderable_surfaces().is_empty());
}

#[test]
fn wayland_surface_attach_null_unmaps_nested_subsurface_tree() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    create_toplevel_with_nested_subsurfaces_then_attach_null_buffer(&socket_path).unwrap();
    let server = stop_test_server(running, server_thread);

    assert!(server.renderable_surfaces().is_empty());
}
