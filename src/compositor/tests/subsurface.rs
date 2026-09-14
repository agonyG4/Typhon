use super::*;
use crate::compositor::subsurface::SubsurfaceRelationshipPhase;

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
fn pending_sibling_is_a_valid_restack_reference_before_application() {
    let mut state = CompositorState::default();
    assert!(state.subsurface_transactions.register(2, 1));
    assert!(state.subsurface_transactions.register(3, 1));
    state.pending_subsurface_stacks.insert(1, vec![1, 3, 2]);

    assert!(state.restack_subsurface(2, 1, 3, false));
    assert_eq!(state.pending_subsurface_stacks[&1], vec![1, 2, 3]);
}

#[test]
fn registering_subsurface_does_not_change_committed_stack() {
    let mut state = CompositorState::default();
    state.committed_subsurface_stacks.insert(1, vec![1, 2, 3]);

    assert!(state.subsurface_transactions.register(4, 1));
    state.add_subsurface_to_pending_stack(1, 4);

    assert_eq!(state.committed_subsurface_stacks[&1], vec![1, 2, 3]);
    assert_eq!(state.pending_subsurface_stacks[&1], vec![1, 2, 3, 4]);
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
fn gecko_pre_role_surface_waits_for_parent_commit_before_adoption() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let snapshots = capture_gecko_pre_role_subsurface_adoption(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(snapshots.after_roleless_commit.is_empty());
    assert!(
        snapshots
            .after_relationship
            .iter()
            .all(|surface| { surface.width != 1 || surface.height != 1 })
    );
    assert!(
        snapshots
            .before_parent_commit
            .iter()
            .all(|surface| { surface.width != 1920 || surface.height != 1080 })
    );
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
fn roleless_parent_current_content_does_not_map_applied_subsurface() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let RolelessParentSubsurfaceSnapshots {
        after_parent_roleless_commit,
        after_child_commit,
        after_parent_relationship_commit,
        child_after_parent_relationship_commit,
    } = capture_roleless_parent_subsurface_mapping(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(after_parent_roleless_commit.current_surface_buffer);
    assert_eq!(after_parent_roleless_commit.permanent_role, None);
    assert!(!after_parent_roleless_commit.renderable_surface);
    assert!(!after_parent_roleless_commit.subsurface_parent_is_mapped);

    assert_eq!(
        after_child_commit.permanent_role,
        Some(PermanentSurfaceRole::Subsurface)
    );
    assert_eq!(
        after_child_commit.subsurface_relationship_phase,
        Some(SubsurfaceRelationshipPhase::PendingParentCommit)
    );
    assert!(after_child_commit.current_surface_buffer);
    assert!(!after_child_commit.renderable_surface);
    assert!(!after_child_commit.subsurface_can_map);
    assert!(after_child_commit.subsurface_content_is_inactive);

    assert_eq!(
        after_parent_relationship_commit.subsurface_relationship_phase,
        None
    );
    assert!(after_parent_relationship_commit.current_surface_buffer);
    assert!(!after_parent_relationship_commit.renderable_surface);
    assert!(!after_parent_relationship_commit.subsurface_parent_is_mapped);

    assert_eq!(
        child_after_parent_relationship_commit.subsurface_relationship_phase,
        Some(SubsurfaceRelationshipPhase::Applied)
    );
    assert!(child_after_parent_relationship_commit.current_surface_buffer);
    assert!(!child_after_parent_relationship_commit.renderable_surface);
    assert!(!child_after_parent_relationship_commit.subsurface_can_map);
    assert!(child_after_parent_relationship_commit.subsurface_content_is_inactive);
}

#[test]
fn roleless_parent_retains_latest_child_until_genuine_parent_mapping() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let RolelessParentMappingSnapshots {
        after_child_relationship_commit,
        after_child_replacement,
        after_parent_becomes_mapped,
    } = capture_roleless_parent_later_mapping(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(
        after_child_relationship_commit
            .iter()
            .map(|surface| (surface.width, surface.height))
            .collect::<Vec<_>>(),
        vec![(30, 20)]
    );
    assert_eq!(
        after_child_replacement
            .iter()
            .map(|surface| (surface.width, surface.height))
            .collect::<Vec<_>>(),
        vec![(30, 20)]
    );
    assert_eq!(
        after_parent_becomes_mapped
            .iter()
            .map(|surface| (surface.width, surface.height))
            .collect::<Vec<_>>(),
        vec![(30, 20), (20, 15), (13, 11)]
    );
    assert_eq!(
        after_parent_becomes_mapped[2].parent_surface_id,
        Some(after_parent_becomes_mapped[1].surface_id)
    );
}

#[test]
fn applied_subsurface_under_roleless_parent_never_reports_presented() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let state = capture_roleless_applied_subsurface_feedback(&socket_path).unwrap();
    stop_test_server(running, server_thread);

    assert_eq!(state.presentation_presented_count, 0);
    assert_eq!(state.presentation_discarded_count, 1);
}

#[test]
fn dormant_subsurface_role_is_inactive_after_relationship_destroy() {
    let mut state = CompositorState::default();
    state.surface_role_lifecycles.insert(
        7,
        SurfaceRoleLifecycle {
            permanent: Some(PermanentSurfaceRole::Subsurface),
            live_instance: None,
            xdg_association: false,
        },
    );

    assert_eq!(state.surface_role(7), SurfaceRole::Unassigned);
    assert!(state.subsurface_content_is_inactive(7));
    assert!(!state.subsurface_can_map(7));
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
fn desynchronized_child_is_retained_until_parent_relationship_activation() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let DesynchronizedSubsurfaceSnapshots {
        before_parent,
        after_latest_child,
        after_parent,
    } = capture_desynchronized_subsurface_before_parent_commit(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);
    let child_size = |surfaces: &[RenderableSurfaceSnapshot]| {
        surfaces
            .iter()
            .find(|surface| surface.parent_surface_id.is_some())
            .map(|surface| (surface.width, surface.height))
    };

    assert_eq!(child_size(&before_parent), None);
    assert_eq!(child_size(&after_latest_child), None);
    assert_eq!(child_size(&after_parent), Some((9, 7)));
}

#[test]
fn preactivation_subsurface_feedback_is_not_presented() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = capture_preactivation_subsurface_presentation_feedback(&socket_path).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(state.presentation_presented_count, 0);
    assert_eq!(state.presentation_discarded_count, 1);
}

#[test]
fn destroyed_latched_subsurface_is_not_resurrected_by_delayed_parent_commit() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind_native_base(&socket_name).unwrap();
    server.set_presentation_clock(PresentationClock::Monotonic);
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let snapshot = capture_destroyed_latched_subsurface_snapshot(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(
        snapshot
            .iter()
            .all(|surface| (surface.width, surface.height) != (9, 7))
    );
}

#[test]
fn destroyed_and_recreated_subsurface_cannot_match_a_stale_parent_transaction() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind_native_base(&socket_name).unwrap();
    server.set_presentation_clock(PresentationClock::Monotonic);
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let DestroyRecreateSubsurfaceSnapshots {
        before_release,
        old_stack,
        after_release,
        new_stack,
    } = capture_destroy_recreate_subsurface_aba(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(
        before_release
            .iter()
            .all(|surface| surface.parent_surface_id.is_none()),
        "the stale R1 CU must remain delayed before its pacing release"
    );
    assert_eq!(old_stack.committed, None);
    assert_eq!(old_stack.latched.as_ref().map(Vec::len), Some(2));
    let recreated = after_release
        .iter()
        .find(|surface| surface.parent_surface_id.is_some())
        .expect("the later R2 CU should apply normally");
    assert_eq!((recreated.width, recreated.height), (13, 9));
    assert_eq!((recreated.local_x, recreated.local_y), (20, 20));
    assert_eq!(new_stack.committed.as_ref().map(Vec::len), Some(2));
    assert_eq!(new_stack.committed, new_stack.latched);
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
        .pending
        .as_ref()
        .and_then(|stack| stack.last().copied())
        .expect("new child should be in pending stack");
    let initial = vec![parent_id, first_id, second_id];

    assert_eq!(
        snapshots.after_first_parent_commit.committed,
        Some(initial.clone())
    );
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
        snapshots.after_child_creation.committed,
        Some(initial.clone())
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
    assert_eq!(after_creation.committed, Some(baseline));
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
    assert_eq!(before_destroy.committed, after_destroy.committed);
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
fn orphan_grandchild_update_is_not_latched_by_root_without_child_commit() {
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
    assert_eq!(deepest_size(&after_root), Some((3, 3)));
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

#[test]
fn mapped_subsurface_tree_retains_current_content_across_parent_null_and_remap() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let snapshots = capture_mapped_subsurface_unmap_remap(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    let before = &snapshots.before_parent_null;
    assert!(before.parent.current_surface_buffer);
    assert!(before.child.current_surface_buffer);
    assert!(before.grandchild.current_surface_buffer);
    assert!(before.parent.renderable_surface);
    assert!(before.child.renderable_surface);
    assert!(before.grandchild.renderable_surface);
    assert_eq!(
        before.child.subsurface_relationship_phase,
        Some(SubsurfaceRelationshipPhase::Applied)
    );
    assert_eq!(
        before.grandchild.subsurface_relationship_phase,
        Some(SubsurfaceRelationshipPhase::Applied)
    );

    let after_null = &snapshots.after_parent_null;
    assert!(!after_null.parent.current_surface_buffer);
    assert!(after_null.child.current_surface_buffer);
    assert!(after_null.grandchild.current_surface_buffer);
    assert!(!after_null.parent.renderable_surface);
    assert!(!after_null.child.renderable_surface);
    assert!(!after_null.grandchild.renderable_surface);
    assert_eq!(
        after_null.child.subsurface_relationship_phase,
        Some(SubsurfaceRelationshipPhase::Applied)
    );
    assert_eq!(
        after_null.grandchild.subsurface_relationship_phase,
        Some(SubsurfaceRelationshipPhase::Applied)
    );
    assert_eq!(after_null.child.placement, before.child.placement);
    assert_eq!(after_null.grandchild.placement, before.grandchild.placement);

    let after_hidden_child_replacement = &snapshots.after_hidden_child_replacement;
    assert!(!after_hidden_child_replacement.parent.current_surface_buffer);
    assert!(after_hidden_child_replacement.child.current_surface_buffer);
    assert!(
        after_hidden_child_replacement
            .grandchild
            .current_surface_buffer
    );
    assert!(!after_hidden_child_replacement.parent.renderable_surface);
    assert!(!after_hidden_child_replacement.child.renderable_surface);
    assert!(!after_hidden_child_replacement.grandchild.renderable_surface);

    let after_remap = &snapshots.after_parent_remap;
    assert!(after_remap.parent.current_surface_buffer);
    assert!(after_remap.child.current_surface_buffer);
    assert!(after_remap.grandchild.current_surface_buffer);
    assert!(after_remap.parent.renderable_surface);
    assert!(after_remap.child.renderable_surface);
    assert!(after_remap.grandchild.renderable_surface);
    assert_eq!(
        after_remap.child.subsurface_relationship_phase,
        Some(SubsurfaceRelationshipPhase::Applied)
    );
    assert_eq!(
        after_remap.grandchild.subsurface_relationship_phase,
        Some(SubsurfaceRelationshipPhase::Applied)
    );
    assert_eq!(
        snapshots
            .after_parent_remap_renderables
            .iter()
            .map(|surface| (surface.width, surface.height))
            .collect::<Vec<_>>(),
        vec![(20, 15), (6, 6), (3, 3)]
    );
}

#[test]
fn destroying_subsurface_retains_current_content_for_recreation() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let snapshots = capture_destroyed_subsurface_recreate(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(snapshots.before_destroy.child.current_surface_buffer);
    assert!(snapshots.before_destroy.child.renderable_surface);

    let after_destroy = &snapshots.after_destroy;
    assert_eq!(
        after_destroy.child.permanent_role,
        Some(PermanentSurfaceRole::Subsurface)
    );
    assert!(!after_destroy.child.renderable_surface);
    assert!(after_destroy.child.current_surface_buffer);
    assert_eq!(after_destroy.child.placement, None);
    assert_eq!(snapshots.parent_stack_after_destroy.committed, None);

    let after_recreate = &snapshots.after_recreate;
    assert!(after_recreate.child.renderable_surface);
    assert!(after_recreate.child.current_surface_buffer);
    assert_eq!(
        after_recreate.child.placement,
        Some(SurfacePlacement::subsurface(
            after_recreate.parent.surface_id,
            0,
            0,
        ))
    );
}

#[test]
fn destroying_subsurface_retains_dmabuf_ownership_until_dormant_replacement() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let snapshots = capture_destroyed_dmabuf_subsurface(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(snapshots.before_destroy.current_surface_buffer);
    assert!(snapshots.before_destroy.active_dmabuf);
    assert_eq!(snapshots.before_destroy.pending_dmabuf_releases, 0);

    assert!(snapshots.after_destroy.current_surface_buffer);
    assert!(snapshots.after_destroy.active_dmabuf);
    assert_eq!(snapshots.after_destroy.pending_dmabuf_releases, 0);

    assert!(snapshots.after_dormant_replacement.current_surface_buffer);
    assert!(snapshots.after_dormant_replacement.active_dmabuf);
    assert_eq!(
        snapshots.after_dormant_replacement.pending_dmabuf_releases,
        1
    );

    assert!(snapshots.after_recreate.current_surface_buffer);
    assert!(snapshots.after_recreate.active_dmabuf);
    assert_eq!(snapshots.after_recreate.pending_dmabuf_releases, 1);
}

#[test]
fn destroying_synchronized_subsurface_promotes_cached_content_for_recreation() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let snapshots = capture_destroyed_cached_subsurface(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(snapshots.before_destroy.renderable_surface);
    assert!(snapshots.before_destroy.current_surface_buffer);
    assert_eq!(
        snapshots.before_destroy.subsurface_relationship_phase,
        Some(SubsurfaceRelationshipPhase::Applied)
    );

    assert!(!snapshots.after_destroy.renderable_surface);
    assert!(snapshots.after_destroy.current_surface_buffer);
    assert_eq!(
        snapshots.after_destroy.permanent_role,
        Some(PermanentSurfaceRole::Subsurface)
    );
    assert_eq!(snapshots.after_destroy.subsurface_relationship_phase, None);

    assert!(!snapshots.after_dormant_commit.renderable_surface);
    assert!(snapshots.after_dormant_commit.current_surface_buffer);

    assert!(snapshots.after_recreate.renderable_surface);
    assert!(snapshots.after_recreate.current_surface_buffer);
    assert_eq!(
        (
            snapshots.after_recreate.placement.unwrap().local_x,
            snapshots.after_recreate.placement.unwrap().local_y
        ),
        (0, 0)
    );
    assert!(
        snapshots
            .after_recreate_renderables
            .iter()
            .any(|surface| (surface.width, surface.height) == (17, 11))
    );
}

#[test]
fn destroying_latched_subsurface_keeps_its_existing_parent_candidate() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind_native_base(&socket_name).unwrap();
    server.set_presentation_clock(PresentationClock::Monotonic);
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let snapshots = capture_detached_latched_subsurface(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(!snapshots.child_after_destroy.current_surface_buffer);
    assert!(!snapshots.child_after_destroy.renderable_surface);
    assert_eq!(
        snapshots.child_after_destroy.subsurface_relationship_phase,
        None
    );
    assert!(
        snapshots
            .child_after_destroy
            .pending_surface_tree_transactions
            >= 1
    );
    assert!(snapshots.parent.pending_surface_tree_transactions >= 1);
    assert!(snapshots.child_after_parent_release.current_surface_buffer);
    assert!(!snapshots.child_after_parent_release.renderable_surface);
}

#[test]
fn destroying_sync_ancestor_promotes_inherited_desync_cache() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let snapshots = capture_detached_inherited_desync_subsurface(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(snapshots.child_after_destroy.current_surface_buffer);
    assert!(!snapshots.child_after_destroy.renderable_surface);
    assert!(snapshots.grandchild_after_destroy.current_surface_buffer);
    assert!(!snapshots.grandchild_after_destroy.renderable_surface);
    assert_eq!(
        snapshots
            .grandchild_after_destroy
            .subsurface_relationship_phase,
        Some(SubsurfaceRelationshipPhase::PendingParentCommit)
    );

    assert!(snapshots.child_after_recreate.renderable_surface);
    assert!(snapshots.grandchild_after_recreate.renderable_surface);
}

#[test]
fn destroying_subsurface_conserves_unrelated_delayed_sibling_work() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind_native_base(&socket_name).unwrap();
    server.set_presentation_clock(PresentationClock::Monotonic);
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let snapshots = capture_detached_sibling_transaction(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(snapshots.child_after_destroy.current_surface_buffer);
    assert!(!snapshots.child_after_destroy.renderable_surface);
    assert!(
        snapshots
            .renderables_after_destroy
            .iter()
            .any(|surface| (surface.width, surface.height) == (6, 6))
    );
    assert!(
        !snapshots
            .renderables_after_destroy
            .iter()
            .any(|surface| (surface.width, surface.height) == (9, 9))
    );
    assert!(
        snapshots
            .renderables_after_parent_release
            .iter()
            .any(|surface| (surface.width, surface.height) == (9, 9))
    );
}

#[test]
fn destroying_subsurface_preserves_child_acquire_constraint() {
    let Some(acquire_timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };
    let Some(release_timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };

    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let subcompositor: client_wl_subcompositor::WlSubcompositor =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 =
        globals.bind(&qh, 3..=3, ()).unwrap();
    let syncobj: client_wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();

    let (parent, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 20, 15).unwrap();
    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();

    let child = compositor.create_surface(&qh, ());
    let child_subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    commit_test_buffered_surface(&child, &shm, &qh, 5, 5).unwrap();
    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();

    let acquire_timeline_fd = acquire_timeline.export_timeline_fd().unwrap();
    let release_timeline_fd = release_timeline.export_timeline_fd().unwrap();
    let sync_surface = syncobj.get_surface(&child, &qh, ());
    let sync_acquire_timeline = syncobj.import_timeline(acquire_timeline_fd.as_fd(), &qh, ());
    let sync_release_timeline = syncobj.import_timeline(release_timeline_fd.as_fd(), &qh, ());
    let child_buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff11_2233).unwrap();
    sync_surface.set_acquire_point(&sync_acquire_timeline, 0, 1);
    sync_surface.set_release_point(&sync_release_timeline, 0, 2);
    child.attach(Some(&child_buffer), 0, 0);
    child.damage_buffer(0, 0, 2, 2);
    child.commit();
    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    wait_for_server_commands(&commands);

    child_subsurface.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    wait_for_server_commands(&commands);
    let blocked = capture_xdg_role_snapshot(&commands, child.id().protocol_id());
    assert!(blocked.current_surface_buffer);
    assert!(!blocked.renderable_surface);
    assert_eq!(blocked.pending_surface_tree_transactions, 1);

    acquire_timeline.signal_point(1).unwrap();
    wait_for_server_commands(&commands);
    let ready = capture_xdg_role_snapshot(&commands, child.id().protocol_id());
    assert!(ready.current_surface_buffer);
    assert!(!ready.renderable_surface);
    assert_eq!(ready.pending_surface_tree_transactions, 0);

    drop(sync_surface);
    drop(sync_acquire_timeline);
    drop(sync_release_timeline);
    drop(syncobj);
    drop(dmabuf);
    drop(child_buffer);
    drop(child_subsurface);
    drop(child);
    drop(parent);
    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn destroying_subsurface_preserves_nested_relationships_and_current_content() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let snapshots = capture_destroyed_nested_subsurface(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(snapshots.before_destroy.child.renderable_surface);
    assert!(snapshots.before_destroy.grandchild.renderable_surface);
    assert_eq!(
        snapshots.child_stack_before_destroy.committed,
        Some(vec![
            snapshots.before_destroy.child.surface_id,
            snapshots.before_destroy.grandchild.surface_id,
        ])
    );

    let after_destroy = &snapshots.after_destroy;
    assert!(!after_destroy.child.renderable_surface);
    assert!(!after_destroy.grandchild.renderable_surface);
    assert!(after_destroy.child.current_surface_buffer);
    assert!(after_destroy.grandchild.current_surface_buffer);
    assert_eq!(after_destroy.child.placement, None);
    assert_eq!(
        after_destroy.grandchild.subsurface_relationship_phase,
        Some(SubsurfaceRelationshipPhase::Applied)
    );
    assert_eq!(
        snapshots.child_stack_after_destroy.committed,
        Some(vec![
            snapshots.before_destroy.child.surface_id,
            snapshots.before_destroy.grandchild.surface_id,
        ])
    );
    assert_eq!(snapshots.parent_stack_after_destroy.committed, None);

    let after_recreate = &snapshots.after_recreate;
    assert!(after_recreate.child.renderable_surface);
    assert!(after_recreate.grandchild.renderable_surface);
    assert!(after_recreate.child.current_surface_buffer);
    assert!(after_recreate.grandchild.current_surface_buffer);
    assert_eq!(
        after_recreate.child.placement,
        Some(SurfacePlacement::subsurface(
            after_recreate.parent.surface_id,
            0,
            0,
        ))
    );
}

#[test]
fn subsurface_null_clears_only_own_content_and_remaps_retained_grandchild() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let snapshots = capture_mapped_subsurface_unmap_remap(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    let after_parent_remap = &snapshots.after_parent_remap;
    assert!(after_parent_remap.parent.renderable_surface);
    assert!(after_parent_remap.child.renderable_surface);
    assert!(after_parent_remap.grandchild.renderable_surface);

    let after_child_null = &snapshots.after_child_null;
    assert!(after_child_null.parent.current_surface_buffer);
    assert!(!after_child_null.child.current_surface_buffer);
    assert!(after_child_null.grandchild.current_surface_buffer);
    assert!(after_child_null.parent.renderable_surface);
    assert!(!after_child_null.child.renderable_surface);
    assert!(!after_child_null.grandchild.renderable_surface);
    assert_eq!(
        after_child_null.grandchild.subsurface_relationship_phase,
        Some(SubsurfaceRelationshipPhase::Applied)
    );
    assert_eq!(
        after_child_null.grandchild.placement,
        after_parent_remap.grandchild.placement
    );

    let after_child_remap = &snapshots.after_child_remap;
    assert!(after_child_remap.parent.renderable_surface);
    assert!(after_child_remap.child.renderable_surface);
    assert!(after_child_remap.grandchild.renderable_surface);
    assert!(after_child_remap.grandchild.current_surface_buffer);
    assert_eq!(
        snapshots
            .after_child_remap_renderables
            .iter()
            .map(|surface| (surface.width, surface.height))
            .collect::<Vec<_>>(),
        vec![(20, 15), (7, 7), (3, 3)]
    );
}

#[test]
fn ancestor_null_does_not_clear_already_inactive_descendant_content() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let snapshots =
        capture_inactive_descendant_across_parent_null(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    let before = &snapshots.before_parent_null;
    assert!(before.parent.current_surface_buffer);
    assert!(before.parent.renderable_surface);
    assert!(!before.child.current_surface_buffer);
    assert!(!before.child.renderable_surface);
    assert!(before.grandchild.current_surface_buffer);
    assert!(!before.grandchild.renderable_surface);
    assert_eq!(
        before.grandchild.subsurface_relationship_phase,
        Some(SubsurfaceRelationshipPhase::Applied)
    );

    let after_null = &snapshots.after_parent_null;
    assert!(!after_null.parent.current_surface_buffer);
    assert!(!after_null.parent.renderable_surface);
    assert!(!after_null.child.current_surface_buffer);
    assert!(!after_null.child.renderable_surface);
    assert!(after_null.grandchild.current_surface_buffer);
    assert!(!after_null.grandchild.renderable_surface);

    let after_remap = &snapshots.after_parent_remap;
    assert!(after_remap.parent.current_surface_buffer);
    assert!(after_remap.parent.renderable_surface);
    assert!(!after_remap.child.current_surface_buffer);
    assert!(!after_remap.child.renderable_surface);
    assert!(after_remap.grandchild.current_surface_buffer);
    assert!(!after_remap.grandchild.renderable_surface);
}

#[test]
fn descendant_dmabuf_ownership_survives_ancestor_hide_until_own_null() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let snapshots =
        capture_dmabuf_subsurface_ownership_across_parent_null(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(snapshots.before_parent_null.current_surface_buffer);
    assert!(snapshots.before_parent_null.active_dmabuf);
    assert_eq!(snapshots.before_parent_null.pending_dmabuf_releases, 0);
    assert!(snapshots.before_parent_null_child.current_surface_buffer);
    assert!(snapshots.before_parent_null_child.active_dmabuf);
    assert_eq!(
        snapshots.before_parent_null_child.pending_dmabuf_releases,
        0
    );

    assert!(!snapshots.after_parent_null.current_surface_buffer);
    assert!(!snapshots.after_parent_null.active_dmabuf);
    assert_eq!(snapshots.after_parent_null.pending_dmabuf_releases, 1);
    assert!(snapshots.after_parent_null_child.current_surface_buffer);
    assert!(snapshots.after_parent_null_child.active_dmabuf);
    assert_eq!(snapshots.after_parent_null_child.pending_dmabuf_releases, 1);

    assert!(!snapshots.after_child_null.current_surface_buffer);
    assert!(!snapshots.after_child_null.active_dmabuf);
    assert_eq!(snapshots.after_child_null.pending_dmabuf_releases, 2);
}

#[test]
fn parent_null_commit_applies_pending_subsurface_without_discarding_current_content() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let snapshot =
        capture_pending_subsurface_activation_across_parent_null(&socket_path, &commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(
        snapshot
            .iter()
            .any(|surface| (surface.width, surface.height) == (5, 5)),
        "the retained child should map when the parent becomes mapped again"
    );
}
