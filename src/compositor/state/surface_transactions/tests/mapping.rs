use super::*;

#[test]
fn real_merge_frozen_final_invalid_surface_mapping_is_rejected() {
    let mut state = CompositorState::default();
    let (display, client, root_surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let mut first = test_mergeable_commit(200);
    first.viewport_destination = PendingViewportChange {
        source: Some(Some(
            ViewportSourceRect::new(0.0, 0.0, 2.5, 2.0).expect("source"),
        )),
        destination: Some(Some(BufferSize::new(4, 4).expect("destination"))),
    };
    let mut second = test_mergeable_commit(201);
    second.viewport_destination.destination = Some(None);
    let (child_id, candidate) = test_real_merge_frozen_candidate(
        &mut state,
        &client,
        &display_handle,
        root_surface_id,
        first,
        second,
    );
    let blocker = test_blocking_external_dependency(&mut state, &client, &display_handle, 3);

    state.submit_surface_tree_nodes_with_kind(
        candidate.root_surface_id,
        candidate.nodes,
        vec![blocker],
        SurfaceTreeSubmissionKind::ClientAdmission,
    );

    assert!(
        state
            .surface_transactions
            .pending_surface_tree_transactions
            .is_empty()
    );
    let child = state
        .surface_resource_by_id(child_id)
        .expect("child resource");
    let data = child.data::<SurfaceData>().expect("child data");
    assert_eq!(
        data.viewport_for_change(PendingViewportChange::default()),
        Default::default()
    );
    assert!(!state.current_surface_buffers.contains_key(&child_id));
    assert!(state.renderable_surface_index(child_id).is_none());
}

#[test]
fn cumulative_viewport_error_keeps_source_owner_for_frozen_candidate() {
    let mut state = CompositorState::default();
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let surface = state
        .surface_resource_by_id(surface_id)
        .expect("surface resource")
        .clone();
    let source_owner = client
        .create_resource::<wp_viewport::WpViewport, ViewportData, CompositorState>(
            &display_handle,
            2,
            ViewportData {
                surface: surface.clone(),
            },
        )
        .expect("source viewport resource");
    let later_owner = client
        .create_resource::<wp_viewport::WpViewport, ViewportData, CompositorState>(
            &display_handle,
            3,
            ViewportData { surface },
        )
        .expect("later viewport resource");
    let mut first = test_mergeable_commit(190);
    first.lineage.merge_frozen = true;
    first.viewport_destination = PendingViewportChange {
        source: Some(Some(
            ViewportSourceRect::new(0.0, 0.0, 2.5, 2.0).expect("source"),
        )),
        destination: Some(Some(BufferSize::new(4, 4).expect("destination"))),
    };
    first.viewport_error_owner = Some(source_owner.clone());
    let first_ref = first.content_update_ref(surface_id);
    let mut second = test_mergeable_commit(191);
    second.lineage.predecessor = Some(first_ref);
    second.viewport_destination.destination = Some(None);
    second.viewport_error_owner = Some(later_owner);
    let blocker = test_blocking_external_dependency(&mut state, &client, &display_handle, 4);

    state.submit_surface_tree_nodes_with_kind(
        surface_id,
        vec![(surface_id, first), (surface_id, second)],
        vec![blocker],
        SurfaceTreeSubmissionKind::ClientAdmission,
    );

    let record = state
        .protocol_error_trace
        .records()
        .last()
        .expect("cumulative viewport error record");
    assert_eq!(
        record.resource_id,
        Some(source_owner.id().protocol_id()),
        "the source update owns a later destination-only mapping error"
    );
}

#[test]
fn real_commit_surface_tree_projects_pending_viewport_before_buffer_mapping() {
    let mut state = CompositorState::default();
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let surface = state
        .surface_resource_by_id(surface_id)
        .expect("surface resource")
        .clone();
    let source_owner = client
        .create_resource::<wp_viewport::WpViewport, ViewportData, CompositorState>(
            &display_handle,
            2,
            ViewportData {
                surface: surface.clone(),
            },
        )
        .expect("source viewport resource");
    let later_owner = client
        .create_resource::<wp_viewport::WpViewport, ViewportData, CompositorState>(
            &display_handle,
            3,
            ViewportData { surface },
        )
        .expect("later viewport resource");
    state
        .surface_resource_by_id(surface_id)
        .expect("surface resource")
        .data::<SurfaceData>()
        .expect("surface data")
        .apply_viewport_change_with_owner(
            PendingViewportChange {
                source: Some(Some(
                    ViewportSourceRect::new(0.0, 0.0, 2.5, 2.0).expect("source"),
                )),
                destination: Some(Some(BufferSize::new(4, 4).expect("destination"))),
            },
            Some(source_owner),
        );

    let mut first = test_mergeable_commit(300);
    first.viewport_destination.source = Some(None);
    let (first_ref, _blocker) = queue_blocked_pacing_predecessor(
        &mut state,
        &client,
        &display_handle,
        surface_id,
        4,
        first,
    );

    let mut second = test_mergeable_commit(301);
    second.lineage.predecessor = Some(first_ref);
    second.viewport_destination.destination = Some(None);
    second.viewport_error_owner = Some(later_owner);
    second.attachment = Some(PendingSurfaceAttachment::Buffer(test_pending_shm_buffer(
        &mut state,
        &client,
        &display_handle,
        5,
        100,
        80,
    )));
    assert!(matches!(
        state.derive_surface_mapping_for_commit(surface_id, &second),
        Some(Ok(Some(_)))
    ));
    let release_before = state.buffer_release_metrics();

    state.commit_surface_tree_request(surface_id, second);

    assert!(state.protocol_error_trace.records().next().is_none());
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
        other => panic!("expected admitted buffer, got {other:?}"),
    };
    assert_eq!(pending.viewport_source, None);
    assert_eq!(pending.viewport_destination, None);
    assert_eq!(
        pending.surface_size,
        Some(BufferSize::new(100, 80).unwrap())
    );
    let release_after = state.buffer_release_metrics();
    assert_eq!(
        release_after.buffer_releases_completed, release_before.buffer_releases_completed,
        "admitted buffer was not released by direct admission"
    );
}

#[test]
fn real_commit_surface_tree_captures_resize_size_after_projected_mapping() {
    let mut state = CompositorState::default();
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    install_test_resize_capture(&mut state, &client, &display_handle, surface_id);

    let mut first = test_mergeable_commit(310);
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

    let mut second = test_mergeable_commit(311);
    second.lineage.predecessor = Some(first_ref);
    second.attachment = Some(PendingSurfaceAttachment::Buffer(test_pending_shm_buffer(
        &mut state,
        &client,
        &display_handle,
        3,
        100,
        80,
    )));
    assert!(matches!(
        state.derive_surface_mapping_for_commit(surface_id, &second),
        Some(Ok(Some(_)))
    ));

    state.commit_surface_tree_request(surface_id, second);

    let pending = match state.surface_transactions.pending_surface_tree_transactions[1].nodes[0]
        .1
        .attachment
        .as_ref()
    {
        Some(PendingSurfaceAttachment::Buffer(buffer)) => buffer,
        other => panic!("expected admitted buffer, got {other:?}"),
    };
    assert_eq!(pending.surface_size, Some(BufferSize::new(50, 50).unwrap()));
    assert_eq!(
        pending
            .resize_commit
            .as_deref()
            .and_then(|snapshot| snapshot.committed_size),
        Some((50, 50))
    );
}

#[test]
fn real_commit_surface_tree_projects_pending_scale_before_buffer_mapping() {
    let mut state = CompositorState::default();
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    install_test_resize_capture(&mut state, &client, &display_handle, surface_id);
    let mut first = test_mergeable_commit(320);
    first.buffer_scale = Some(2);
    let (first_ref, _blocker) = queue_blocked_pacing_predecessor(
        &mut state,
        &client,
        &display_handle,
        surface_id,
        2,
        first,
    );

    let mut second = test_mergeable_commit(321);
    second.lineage.predecessor = Some(first_ref);
    second.attachment = Some(PendingSurfaceAttachment::Buffer(test_pending_shm_buffer(
        &mut state,
        &client,
        &display_handle,
        3,
        100,
        50,
    )));
    state.commit_surface_tree_request(surface_id, second);

    let pending = match state.surface_transactions.pending_surface_tree_transactions[1].nodes[0]
        .1
        .attachment
        .as_ref()
    {
        Some(PendingSurfaceAttachment::Buffer(buffer)) => buffer,
        other => panic!("expected admitted buffer, got {other:?}"),
    };
    assert_eq!(pending.buffer_scale, 2);
    assert_eq!(pending.surface_size, Some(BufferSize::new(50, 25).unwrap()));
    assert_eq!(
        pending
            .resize_commit
            .as_deref()
            .and_then(|snapshot| snapshot.committed_size),
        Some((50, 25))
    );
}

#[test]
fn real_commit_surface_tree_projects_pending_transform_before_buffer_mapping() {
    let mut state = CompositorState::default();
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    install_test_resize_capture(&mut state, &client, &display_handle, surface_id);
    let mut first = test_mergeable_commit(330);
    first.buffer_transform = Some(wl_output::Transform::_90);
    let (first_ref, _blocker) = queue_blocked_pacing_predecessor(
        &mut state,
        &client,
        &display_handle,
        surface_id,
        2,
        first,
    );

    let mut second = test_mergeable_commit(331);
    second.lineage.predecessor = Some(first_ref);
    second.attachment = Some(PendingSurfaceAttachment::Buffer(test_pending_shm_buffer(
        &mut state,
        &client,
        &display_handle,
        3,
        100,
        50,
    )));
    state.commit_surface_tree_request(surface_id, second);

    let pending = match state.surface_transactions.pending_surface_tree_transactions[1].nodes[0]
        .1
        .attachment
        .as_ref()
    {
        Some(PendingSurfaceAttachment::Buffer(buffer)) => buffer,
        other => panic!("expected admitted buffer, got {other:?}"),
    };
    assert_eq!(pending.buffer_transform, wl_output::Transform::_90);
    assert_eq!(
        pending.surface_size,
        Some(BufferSize::new(50, 100).unwrap())
    );
    assert_eq!(
        pending
            .resize_commit
            .as_deref()
            .and_then(|snapshot| snapshot.committed_size),
        Some((50, 100))
    );
}

#[test]
fn real_commit_surface_tree_reports_projected_historical_viewport_owner() {
    let mut state = CompositorState::default();
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let surface = state
        .surface_resource_by_id(surface_id)
        .expect("surface resource")
        .clone();
    let source_owner = client
        .create_resource::<wp_viewport::WpViewport, ViewportData, CompositorState>(
            &display_handle,
            2,
            ViewportData {
                surface: surface.clone(),
            },
        )
        .expect("source viewport resource");
    let later_owner = client
        .create_resource::<wp_viewport::WpViewport, ViewportData, CompositorState>(
            &display_handle,
            3,
            ViewportData { surface },
        )
        .expect("later viewport resource");
    let mut first = test_mergeable_commit(340);
    first.viewport_destination = PendingViewportChange {
        source: Some(Some(
            ViewportSourceRect::new(0.0, 0.0, 2.5, 2.0).expect("source"),
        )),
        destination: Some(Some(BufferSize::new(4, 4).expect("destination"))),
    };
    first.viewport_error_owner = Some(source_owner.clone());
    let (first_ref, _blocker) = queue_blocked_pacing_predecessor(
        &mut state,
        &client,
        &display_handle,
        surface_id,
        4,
        first,
    );

    let mut second = test_mergeable_commit(341);
    second.lineage.predecessor = Some(first_ref);
    second.viewport_destination.destination = Some(None);
    second.viewport_error_owner = Some(later_owner);
    second.attachment = Some(PendingSurfaceAttachment::Buffer(test_pending_shm_buffer(
        &mut state,
        &client,
        &display_handle,
        5,
        100,
        80,
    )));
    state.commit_surface_tree_request(surface_id, second);

    let record = state
        .protocol_error_trace
        .records()
        .last()
        .expect("projected viewport error record");
    assert_eq!(record.resource_id, Some(source_owner.id().protocol_id()));
    assert_eq!(record.error_code, Some(wp_viewport::Error::BadSize as u32));
}

#[test]
fn real_commit_surface_tree_replaces_removed_content_with_projected_mapping() {
    let mut state = CompositorState::default();
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    state
        .surface_resource_by_id(surface_id)
        .expect("surface resource")
        .data::<SurfaceData>()
        .expect("surface data")
        .apply_viewport_change_with_owner(
            PendingViewportChange {
                source: Some(Some(
                    ViewportSourceRect::new(0.0, 0.0, 2.5, 2.0).expect("source"),
                )),
                destination: Some(Some(BufferSize::new(4, 4).expect("destination"))),
            },
            None,
        );
    let old = test_pending_shm_buffer(&mut state, &client, &display_handle, 2, 1, 1);
    state
        .current_surface_buffers
        .insert(surface_id, CurrentSurfaceBuffer::from(old));

    let mut first = test_mergeable_commit(350);
    first.attachment = Some(PendingSurfaceAttachment::RemoveContent);
    let (first_ref, _blocker) = queue_blocked_pacing_predecessor(
        &mut state,
        &client,
        &display_handle,
        surface_id,
        3,
        first,
    );

    let mut second = test_mergeable_commit(351);
    second.lineage.predecessor = Some(first_ref);
    second.attachment = Some(PendingSurfaceAttachment::Buffer(test_pending_shm_buffer(
        &mut state,
        &client,
        &display_handle,
        4,
        100,
        80,
    )));
    state.commit_surface_tree_request(surface_id, second);

    let pending = match state.surface_transactions.pending_surface_tree_transactions[1].nodes[0]
        .1
        .attachment
        .as_ref()
    {
        Some(PendingSurfaceAttachment::Buffer(buffer)) => buffer,
        other => panic!("expected admitted replacement buffer, got {other:?}"),
    };
    assert!(pending.viewport_source.is_some());
    assert_eq!(
        pending.viewport_destination,
        Some(BufferSize::new(4, 4).unwrap())
    );
    assert_eq!(pending.surface_size, Some(BufferSize::new(4, 4).unwrap()));
}

#[test]
fn surface_tree_merge_preserves_independent_viewport_fields() {
    let mut state = CompositorState::default();
    let (_display, client, surface_id) = test_surface_and_client(&mut state);
    let source = ViewportSourceRect::new(1.0, 2.0, 3.0, 4.0).expect("valid source");
    let destination = BufferSize::new(5, 6).expect("valid destination");
    let mut target = test_mergeable_commit(1);
    target.viewport_destination = PendingViewportChange {
        source: Some(Some(source)),
        destination: None,
    };
    let target_ref = target.content_update_ref(surface_id);
    let mut incoming = test_mergeable_commit(2);
    incoming.lineage.predecessor = Some(target_ref);
    incoming.viewport_destination = PendingViewportChange {
        source: None,
        destination: Some(Some(destination)),
    };
    let mut transaction = PendingSurfaceTreeTransaction {
        id: SurfaceTreeTransactionId::new(11),
        root_surface_id: surface_id,
        nodes: vec![(surface_id, target)],
        publication_lifetimes: test_captured_lifetimes(&client.id(), &[surface_id]),
        dependencies: Vec::new(),
        external_content_update_dependencies: Vec::new(),
        commit_timing_readiness: None,
        received_at: Instant::now(),
    };

    state.merge_surface_tree_nodes_into_transaction(
        surface_id,
        &mut transaction,
        vec![(surface_id, incoming)],
        test_captured_lifetimes(&client.id(), &[surface_id]),
        Vec::new(),
        Vec::new(),
    );

    assert_eq!(
        transaction.nodes[0].1.viewport_destination,
        PendingViewportChange {
            source: Some(Some(source)),
            destination: Some(Some(destination)),
        }
    );
}

#[test]
fn surface_tree_merge_preserves_viewport_reset_with_unrelated_field() {
    let mut state = CompositorState::default();
    let (_display, client, surface_id) = test_surface_and_client(&mut state);
    let source = ViewportSourceRect::new(1.0, 2.0, 3.0, 4.0).expect("valid source");
    let mut target = test_mergeable_commit(1);
    target.viewport_destination = PendingViewportChange {
        source: Some(Some(source)),
        destination: None,
    };
    let target_ref = target.content_update_ref(surface_id);
    let mut incoming = test_mergeable_commit(2);
    incoming.lineage.predecessor = Some(target_ref);
    incoming.viewport_destination = PendingViewportChange {
        source: None,
        destination: Some(None),
    };
    let mut transaction = PendingSurfaceTreeTransaction {
        id: SurfaceTreeTransactionId::new(12),
        root_surface_id: surface_id,
        nodes: vec![(surface_id, target)],
        publication_lifetimes: test_captured_lifetimes(&client.id(), &[surface_id]),
        dependencies: Vec::new(),
        external_content_update_dependencies: Vec::new(),
        commit_timing_readiness: None,
        received_at: Instant::now(),
    };

    state.merge_surface_tree_nodes_into_transaction(
        surface_id,
        &mut transaction,
        vec![(surface_id, incoming)],
        test_captured_lifetimes(&client.id(), &[surface_id]),
        Vec::new(),
        Vec::new(),
    );

    assert_eq!(
        transaction.nodes[0].1.viewport_destination,
        PendingViewportChange {
            source: Some(Some(source)),
            destination: Some(None),
        }
    );
}

#[test]
fn final_composed_fractional_source_without_destination_is_rejected() {
    let mut state = CompositorState::default();
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let mut first = test_mergeable_commit(120);
    first.viewport_destination = PendingViewportChange {
        source: Some(Some(
            ViewportSourceRect::new(0.0, 0.0, 2.5, 2.0).expect("source"),
        )),
        destination: Some(Some(BufferSize::new(4, 4).expect("destination"))),
    };
    let first_ref = first.content_update_ref(surface_id);
    let mut second = test_mergeable_commit(121);
    second.lineage.predecessor = Some(first_ref);
    second.viewport_destination.destination = Some(None);
    let blocker = test_blocking_external_dependency(&mut state, &client, &display_handle, 2);

    state.submit_surface_tree_nodes_with_kind(
        surface_id,
        vec![(surface_id, first), (surface_id, second)],
        vec![blocker],
        SurfaceTreeSubmissionKind::ClientAdmission,
    );

    assert!(
        state
            .surface_transactions
            .pending_surface_tree_transactions
            .is_empty(),
        "the final canonical viewport state must be rejected before queueing"
    );
}

#[test]
fn valid_final_composed_viewport_is_not_rejected_by_an_independent_delta() {
    let mut state = CompositorState::default();
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let surface = state
        .surface_resource_by_id(surface_id)
        .expect("surface resource");
    surface
        .data::<SurfaceData>()
        .expect("surface data")
        .apply_viewport_change_with_owner(
            PendingViewportChange {
                source: Some(Some(
                    ViewportSourceRect::new(0.0, 0.0, 2.5, 2.0).expect("source"),
                )),
                destination: Some(Some(BufferSize::new(4, 4).expect("destination"))),
            },
            None,
        );
    let mut first = test_mergeable_commit(130);
    first.viewport_destination.source = Some(None);
    let first_ref = first.content_update_ref(surface_id);
    let mut second = test_mergeable_commit(131);
    second.lineage.predecessor = Some(first_ref);
    second.viewport_destination.destination = Some(None);
    let blocker = test_blocking_external_dependency(&mut state, &client, &display_handle, 2);

    state.submit_surface_tree_nodes_with_kind(
        surface_id,
        vec![(surface_id, first), (surface_id, second)],
        vec![blocker],
        SurfaceTreeSubmissionKind::ClientAdmission,
    );

    let _ = (client, display_handle);
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
            .viewport_destination,
        PendingViewportChange {
            source: Some(None),
            destination: Some(None),
        }
    );
}

#[test]
fn merge_frozen_final_valid_composition_is_accepted() {
    let mut state = CompositorState::default();
    let (display, client, root_surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let mut first = test_mergeable_commit(135);
    first.viewport_destination.source = Some(None);
    let first_ref = first.content_update_ref(2);
    let mut second = test_mergeable_commit(136);
    second.lineage.predecessor = Some(first_ref);
    second.viewport_destination.destination = Some(None);
    let (child_id, candidate) = test_real_merge_frozen_candidate(
        &mut state,
        &client,
        &display_handle,
        root_surface_id,
        first,
        second,
    );
    state
        .surface_resource_by_id(child_id)
        .expect("child resource")
        .data::<SurfaceData>()
        .expect("child data")
        .apply_viewport_change_with_owner(
            PendingViewportChange {
                source: Some(Some(
                    ViewportSourceRect::new(0.0, 0.0, 2.5, 2.0).expect("source"),
                )),
                destination: Some(Some(BufferSize::new(4, 4).expect("destination"))),
            },
            None,
        );
    state.submit_surface_tree_nodes_with_kind(
        candidate.root_surface_id,
        candidate.nodes,
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
        state
            .surface_resource_by_id(child_id)
            .expect("child resource")
            .data::<SurfaceData>()
            .expect("child data")
            .viewport_for_change(PendingViewportChange::default()),
        SurfaceViewportCommit::default(),
        "the valid composition publishes through the real merge_frozen lifecycle"
    );
}

#[test]
fn null_removal_precedes_later_viewport_validation() {
    let mut state = CompositorState::default();
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let retained = test_pending_shm_buffer(&mut state, &client, &display_handle, 2, 50, 50);
    let retained_buffer_id = retained.data.buffer_id().get();
    state
        .current_surface_buffers
        .insert(surface_id, CurrentSurfaceBuffer::from(retained));
    let mut first = test_cached_commit(137);
    first.attachment = Some(PendingSurfaceAttachment::RemoveContent);
    let first_ref = first.content_update_ref(surface_id);
    let mut second = test_mergeable_commit(138);
    second.lineage.predecessor = Some(first_ref);
    second.viewport_destination.source = Some(Some(
        ViewportSourceRect::new(0.0, 0.0, 100.0, 100.0).expect("source"),
    ));
    let blocker = test_blocking_external_dependency(&mut state, &client, &display_handle, 3);

    state.submit_surface_tree_nodes_with_kind(
        surface_id,
        vec![(surface_id, first), (surface_id, second)],
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
    assert_eq!(
        state.surface_transactions.pending_surface_tree_transactions[0].nodes[0]
            .1
            .attachment
            .as_ref()
            .map(|attachment| matches!(attachment, PendingSurfaceAttachment::RemoveContent)),
        Some(true)
    );
    assert_eq!(
        state
            .current_surface_buffers
            .get(&surface_id)
            .map(CurrentSurfaceBuffer::buffer_id)
            .map(|id| id.get()),
        Some(retained_buffer_id)
    );
}

#[test]
fn new_attachment_precedes_later_viewport_validation_and_releases_once() {
    let mut state = CompositorState::default();
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let retained = test_pending_shm_buffer(&mut state, &client, &display_handle, 2, 100, 100);
    let retained_buffer_id = retained.data.buffer_id().get();
    state
        .current_surface_buffers
        .insert(surface_id, CurrentSurfaceBuffer::from(retained));
    let mut first = test_cached_commit(139);
    first.attachment = Some(PendingSurfaceAttachment::Buffer(test_pending_shm_buffer(
        &mut state,
        &client,
        &display_handle,
        3,
        50,
        50,
    )));
    let first_ref = first.content_update_ref(surface_id);
    let mut second = test_mergeable_commit(140);
    second.lineage.predecessor = Some(first_ref);
    second.viewport_destination.source = Some(Some(
        ViewportSourceRect::new(0.0, 0.0, 75.0, 50.0).expect("source"),
    ));
    let blocker = test_blocking_external_dependency(&mut state, &client, &display_handle, 4);
    let release_before = state.buffer_release_metrics();

    state.submit_surface_tree_nodes_with_kind(
        surface_id,
        vec![(surface_id, first), (surface_id, second)],
        vec![blocker],
        SurfaceTreeSubmissionKind::ClientAdmission,
    );

    assert!(
        state
            .surface_transactions
            .pending_surface_tree_transactions
            .is_empty()
    );
    assert_eq!(
        state
            .current_surface_buffers
            .get(&surface_id)
            .map(CurrentSurfaceBuffer::buffer_id)
            .map(|id| id.get()),
        Some(retained_buffer_id)
    );
    let release_after = state.buffer_release_metrics();
    assert_eq!(
        release_after.buffer_releases_completed,
        release_before.buffer_releases_completed + 1,
        "the rejected new attachment is released once"
    );
    assert_eq!(
        release_after.buffer_release_duplicate_attempts,
        release_before.buffer_release_duplicate_attempts
    );
}

#[test]
fn canonical_viewport_destination_prepares_the_surviving_attachment() {
    let mut state = CompositorState::default();
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let mut first = test_mergeable_commit(140);
    first.viewport_destination.destination =
        Some(Some(BufferSize::new(50, 50).expect("destination")));
    let first_ref = first.content_update_ref(surface_id);
    let mut second = test_mergeable_commit(141);
    second.lineage.predecessor = Some(first_ref);
    let blocker = test_blocking_external_dependency(&mut state, &client, &display_handle, 3);
    second.attachment = Some(PendingSurfaceAttachment::Buffer(test_pending_shm_buffer(
        &mut state,
        &client,
        &display_handle,
        4,
        100,
        100,
    )));

    state.submit_surface_tree_nodes_with_kind(
        surface_id,
        vec![(surface_id, first), (surface_id, second)],
        vec![blocker],
        SurfaceTreeSubmissionKind::ClientAdmission,
    );

    let pending = match &state.surface_transactions.pending_surface_tree_transactions[0].nodes[0]
        .1
        .attachment
    {
        Some(PendingSurfaceAttachment::Buffer(buffer)) => buffer,
        other => panic!("expected prepared buffer, got {other:?}"),
    };
    assert_eq!(pending.surface_size, Some(BufferSize::new(50, 50).unwrap()));
    assert_eq!(
        pending.viewport_destination,
        Some(BufferSize::new(50, 50).unwrap())
    );
}

#[test]
fn pending_transaction_merge_reprepares_against_its_effective_mapping() {
    let mut state = CompositorState::default();
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let blocker = test_blocking_external_dependency(&mut state, &client, &display_handle, 2);
    let mut first = test_mergeable_commit(145);
    first.viewport_destination.destination =
        Some(Some(BufferSize::new(50, 50).expect("destination")));
    let first_ref = first.content_update_ref(surface_id);
    state.submit_surface_tree_nodes_with_kind(
        surface_id,
        vec![(surface_id, first)],
        vec![blocker],
        SurfaceTreeSubmissionKind::ClientAdmission,
    );

    let mut second = test_mergeable_commit(146);
    second.lineage.predecessor = Some(first_ref);
    second.attachment = Some(PendingSurfaceAttachment::Buffer(test_pending_shm_buffer(
        &mut state,
        &client,
        &display_handle,
        3,
        100,
        100,
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
        1
    );
    let pending = match &state.surface_transactions.pending_surface_tree_transactions[0].nodes[0]
        .1
        .attachment
    {
        Some(PendingSurfaceAttachment::Buffer(buffer)) => buffer,
        other => panic!("expected prepared buffer, got {other:?}"),
    };
    assert_eq!(pending.surface_size, Some(BufferSize::new(50, 50).unwrap()));
    assert_eq!(
        pending.viewport_destination,
        Some(BufferSize::new(50, 50).unwrap())
    );
}

#[test]
fn canonical_buffer_transform_prepares_the_surviving_attachment() {
    let mut state = CompositorState::default();
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let mut first = test_mergeable_commit(150);
    first.buffer_transform = Some(wl_output::Transform::_90);
    let first_ref = first.content_update_ref(surface_id);
    let mut second = test_mergeable_commit(151);
    second.lineage.predecessor = Some(first_ref);
    let blocker = test_blocking_external_dependency(&mut state, &client, &display_handle, 2);
    second.attachment = Some(PendingSurfaceAttachment::Buffer(test_pending_shm_buffer(
        &mut state,
        &client,
        &display_handle,
        2,
        100,
        50,
    )));

    state.submit_surface_tree_nodes_with_kind(
        surface_id,
        vec![(surface_id, first), (surface_id, second)],
        vec![blocker],
        SurfaceTreeSubmissionKind::ClientAdmission,
    );

    let pending = match &state.surface_transactions.pending_surface_tree_transactions[0].nodes[0]
        .1
        .attachment
    {
        Some(PendingSurfaceAttachment::Buffer(buffer)) => buffer,
        other => panic!("expected prepared buffer, got {other:?}"),
    };
    assert_eq!(pending.buffer_transform, wl_output::Transform::_90);
    assert_eq!(
        pending.surface_size,
        Some(BufferSize::new(50, 100).unwrap())
    );
}

#[test]
fn canonical_buffer_scale_prepares_the_surviving_attachment() {
    let mut state = CompositorState::default();
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let mut first = test_mergeable_commit(160);
    first.buffer_scale = Some(2);
    let first_ref = first.content_update_ref(surface_id);
    let mut second = test_mergeable_commit(161);
    second.lineage.predecessor = Some(first_ref);
    let blocker = test_blocking_external_dependency(&mut state, &client, &display_handle, 2);
    second.attachment = Some(PendingSurfaceAttachment::Buffer(test_pending_shm_buffer(
        &mut state,
        &client,
        &display_handle,
        2,
        100,
        50,
    )));

    state.submit_surface_tree_nodes_with_kind(
        surface_id,
        vec![(surface_id, first), (surface_id, second)],
        vec![blocker],
        SurfaceTreeSubmissionKind::ClientAdmission,
    );

    let pending = match &state.surface_transactions.pending_surface_tree_transactions[0].nodes[0]
        .1
        .attachment
    {
        Some(PendingSurfaceAttachment::Buffer(buffer)) => buffer,
        other => panic!("expected prepared buffer, got {other:?}"),
    };
    assert_eq!(pending.buffer_scale, 2);
    assert_eq!(pending.surface_size, Some(BufferSize::new(50, 25).unwrap()));
}

#[test]
fn pacing_protected_surface_mapping_state_accumulates_before_attachment() {
    let mut state = CompositorState::default();
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let mut first = test_cached_commit(170);
    first.buffer_scale = Some(2);
    first.buffer_transform = Some(wl_output::Transform::_90);
    let first_ref = first.content_update_ref(surface_id);
    let mut second = test_mergeable_commit(171);
    second.lineage.predecessor = Some(first_ref);
    second.attachment = Some(PendingSurfaceAttachment::Buffer(test_pending_shm_buffer(
        &mut state,
        &client,
        &display_handle,
        2,
        100,
        50,
    )));
    let blocker = test_blocking_external_dependency(&mut state, &client, &display_handle, 3);

    state.submit_surface_tree_nodes_with_kind(
        surface_id,
        vec![(surface_id, first), (surface_id, second)],
        vec![blocker],
        SurfaceTreeSubmissionKind::ClientAdmission,
    );

    let transaction = &state.surface_transactions.pending_surface_tree_transactions[0];
    assert_eq!(transaction.nodes.len(), 2);
    let pending = match &transaction.nodes[1].1.attachment {
        Some(PendingSurfaceAttachment::Buffer(buffer)) => buffer,
        other => panic!("expected prepared buffer, got {other:?}"),
    };
    assert_eq!(pending.buffer_scale, 2);
    assert_eq!(pending.buffer_transform, wl_output::Transform::_90);
    assert_eq!(pending.surface_size, Some(BufferSize::new(25, 50).unwrap()));
}

#[test]
fn merge_frozen_surface_mapping_state_remains_exact_and_accumulates() {
    let mut state = CompositorState::default();
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let mut first = test_mergeable_commit(180);
    first.lineage.merge_frozen = true;
    first.buffer_scale = Some(2);
    first.buffer_transform = Some(wl_output::Transform::_90);
    let first_ref = first.content_update_ref(surface_id);
    let mut second = test_mergeable_commit(181);
    second.lineage.predecessor = Some(first_ref);
    second.attachment = Some(PendingSurfaceAttachment::Buffer(test_pending_shm_buffer(
        &mut state,
        &client,
        &display_handle,
        2,
        100,
        50,
    )));
    let blocker = test_blocking_external_dependency(&mut state, &client, &display_handle, 3);

    state.submit_surface_tree_nodes_with_kind(
        surface_id,
        vec![(surface_id, first), (surface_id, second)],
        vec![blocker],
        SurfaceTreeSubmissionKind::ClientAdmission,
    );

    let transaction = &state.surface_transactions.pending_surface_tree_transactions[0];
    assert_eq!(
        transaction
            .nodes
            .iter()
            .map(|(_, commit)| commit.commit_sequence)
            .collect::<Vec<_>>(),
        vec![SurfaceCommitSequence(180), SurfaceCommitSequence(181)]
    );
    let pending = match &transaction.nodes[1].1.attachment {
        Some(PendingSurfaceAttachment::Buffer(buffer)) => buffer,
        other => panic!("expected prepared buffer, got {other:?}"),
    };
    assert_eq!(pending.buffer_scale, 2);
    assert_eq!(pending.buffer_transform, wl_output::Transform::_90);
    assert_eq!(pending.surface_size, Some(BufferSize::new(25, 50).unwrap()));
}

#[test]
fn canceled_transaction_selects_the_latest_root_resize_snapshot() {
    let root_surface_id = 70;
    let older_snapshot = ResizeCommitSnapshot {
        serial: 1,
        sequence: 1,
        commit_sequence: 1,
        width: 100,
        height: 100,
        placement: SurfacePlacement::root(),
        edges: ResizeEdges::BOTTOM_RIGHT,
        resizing: true,
        emitted_at: Instant::now(),
        committed_size: Some((100, 100)),
        effective_xdg_window_geometry: None,
        buffer_id: None,
        interaction_id: ResizeInteractionId::new(1),
    };
    let newer_snapshot = ResizeCommitSnapshot {
        sequence: 2,
        commit_sequence: 2,
        width: 200,
        height: 200,
        committed_size: Some((200, 200)),
        interaction_id: ResizeInteractionId::new(2),
        ..older_snapshot
    };
    let mut older = test_mergeable_commit(1);
    older.resize_commit = Some(older_snapshot);
    let older_ref = older.content_update_ref(root_surface_id);
    let mut newer = test_mergeable_commit(2);
    newer.lineage.predecessor = Some(older_ref);
    newer.resize_commit = Some(newer_snapshot);
    let mut nodes = vec![
        (root_surface_id, older),
        (71, test_mergeable_commit(3)),
        (root_surface_id, newer),
    ];

    let selected =
        take_tree_resize_commit(root_surface_id, &mut nodes).expect("latest root resize snapshot");

    assert_eq!(selected.commit_sequence, 2);
    assert!(nodes[0].1.resize_commit.is_some());
    assert!(nodes[2].1.resize_commit.is_none());
}

#[test]
fn separate_pending_destination_prepares_later_attachment_against_predecessor() {
    let mut state = CompositorState::default();
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let mut first = test_mergeable_commit(210);
    first.viewport_destination.destination =
        Some(Some(BufferSize::new(50, 50).expect("destination")));
    let (first_ref, blocker) = queue_blocked_pacing_predecessor(
        &mut state,
        &client,
        &display_handle,
        surface_id,
        2,
        first,
    );

    let mut second = test_mergeable_commit(211);
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
    let pending = &state.surface_transactions.pending_surface_tree_transactions[1].nodes[0].1;
    let pending = match pending.attachment.as_ref() {
        Some(PendingSurfaceAttachment::Buffer(buffer)) => buffer,
        other => panic!("expected prepared buffer, got {other:?}"),
    };
    assert_eq!(pending.surface_size, Some(BufferSize::new(50, 50).unwrap()));
    assert_eq!(
        pending.viewport_destination,
        Some(BufferSize::new(50, 50).unwrap())
    );

    release_blocking_dependency(&mut state, blocker);
    state.commit_ready_surface_tree_transactions();
    assert!(
        state
            .surface_transactions
            .pending_surface_tree_transactions
            .is_empty()
    );
    let current = state
        .current_surface_buffers
        .get(&surface_id)
        .expect("published current buffer");
    assert_eq!(
        current.viewport_destination(),
        Some(BufferSize::new(50, 50).unwrap())
    );
    assert_eq!(
        current.current_content_mapping().unwrap().surface_size,
        BufferSize::new(50, 50).unwrap()
    );
}

#[test]
fn separate_pending_scale_prepares_later_attachment_at_predecessor_scale() {
    let mut state = CompositorState::default();
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let mut first = test_mergeable_commit(220);
    first.buffer_scale = Some(2);
    let (first_ref, _blocker) = queue_blocked_pacing_predecessor(
        &mut state,
        &client,
        &display_handle,
        surface_id,
        2,
        first,
    );

    let mut second = test_mergeable_commit(221);
    second.lineage.predecessor = Some(first_ref);
    second.attachment = Some(PendingSurfaceAttachment::Buffer(test_pending_shm_buffer(
        &mut state,
        &client,
        &display_handle,
        3,
        100,
        50,
    )));
    state.submit_surface_tree_nodes_with_kind(
        surface_id,
        vec![(surface_id, second)],
        Vec::new(),
        SurfaceTreeSubmissionKind::ClientAdmission,
    );

    let pending = match state.surface_transactions.pending_surface_tree_transactions[1].nodes[0]
        .1
        .attachment
        .as_ref()
    {
        Some(PendingSurfaceAttachment::Buffer(buffer)) => buffer,
        other => panic!("expected prepared buffer, got {other:?}"),
    };
    assert_eq!(pending.buffer_scale, 2);
    assert_eq!(pending.surface_size, Some(BufferSize::new(50, 25).unwrap()));
}

#[test]
fn separate_pending_transform_prepares_later_attachment_at_predecessor_transform() {
    let mut state = CompositorState::default();
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let mut first = test_mergeable_commit(230);
    first.buffer_transform = Some(wl_output::Transform::_90);
    let (first_ref, _blocker) = queue_blocked_pacing_predecessor(
        &mut state,
        &client,
        &display_handle,
        surface_id,
        2,
        first,
    );

    let mut second = test_mergeable_commit(231);
    second.lineage.predecessor = Some(first_ref);
    second.attachment = Some(PendingSurfaceAttachment::Buffer(test_pending_shm_buffer(
        &mut state,
        &client,
        &display_handle,
        3,
        100,
        50,
    )));
    state.submit_surface_tree_nodes_with_kind(
        surface_id,
        vec![(surface_id, second)],
        Vec::new(),
        SurfaceTreeSubmissionKind::ClientAdmission,
    );

    let pending = match state.surface_transactions.pending_surface_tree_transactions[1].nodes[0]
        .1
        .attachment
        .as_ref()
    {
        Some(PendingSurfaceAttachment::Buffer(buffer)) => buffer,
        other => panic!("expected prepared buffer, got {other:?}"),
    };
    assert_eq!(pending.buffer_transform, wl_output::Transform::_90);
    assert_eq!(
        pending.surface_size,
        Some(BufferSize::new(50, 100).unwrap())
    );
}

#[test]
fn separate_pending_viewport_reset_accepts_final_valid_composition() {
    let mut state = CompositorState::default();
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    state
        .surface_resource_by_id(surface_id)
        .expect("surface resource")
        .data::<SurfaceData>()
        .expect("surface data")
        .apply_viewport_change_with_owner(
            PendingViewportChange {
                source: Some(Some(
                    ViewportSourceRect::new(0.0, 0.0, 2.5, 2.0).expect("source"),
                )),
                destination: Some(Some(BufferSize::new(4, 4).expect("destination"))),
            },
            None,
        );
    let mut first = test_mergeable_commit(240);
    first.viewport_destination.source = Some(None);
    let (first_ref, _blocker) = queue_blocked_pacing_predecessor(
        &mut state,
        &client,
        &display_handle,
        surface_id,
        2,
        first,
    );

    let mut second = test_mergeable_commit(241);
    second.lineage.predecessor = Some(first_ref);
    second.viewport_destination.destination = Some(None);
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
}

#[test]
fn separate_pending_viewport_reset_rejects_final_invalid_composition_with_source_owner() {
    let mut state = CompositorState::default();
    let (display, client, surface_id) = test_surface_and_client(&mut state);
    let display_handle = display.handle();
    let surface = state
        .surface_resource_by_id(surface_id)
        .expect("surface resource")
        .clone();
    let source_owner = client
        .create_resource::<wp_viewport::WpViewport, ViewportData, CompositorState>(
            &display_handle,
            2,
            ViewportData {
                surface: surface.clone(),
            },
        )
        .expect("source viewport resource");
    let later_owner = client
        .create_resource::<wp_viewport::WpViewport, ViewportData, CompositorState>(
            &display_handle,
            3,
            ViewportData { surface },
        )
        .expect("later viewport resource");
    let mut first = test_mergeable_commit(250);
    first.viewport_destination = PendingViewportChange {
        source: Some(Some(
            ViewportSourceRect::new(0.0, 0.0, 2.5, 2.0).expect("source"),
        )),
        destination: Some(Some(BufferSize::new(4, 4).expect("destination"))),
    };
    first.viewport_error_owner = Some(source_owner.clone());
    let (first_ref, _blocker) = queue_blocked_pacing_predecessor(
        &mut state,
        &client,
        &display_handle,
        surface_id,
        4,
        first,
    );

    let mut second = test_mergeable_commit(251);
    second.lineage.predecessor = Some(first_ref);
    second.viewport_destination.destination = Some(None);
    second.viewport_error_owner = Some(later_owner);
    state.submit_surface_tree_nodes_with_kind(
        surface_id,
        vec![(surface_id, second)],
        Vec::new(),
        SurfaceTreeSubmissionKind::ClientAdmission,
    );

    assert!(
        state
            .surface_transactions
            .pending_surface_tree_transactions
            .is_empty()
    );
    let record = state
        .protocol_error_trace
        .records()
        .last()
        .expect("cross-transaction viewport error record");
    assert_eq!(record.resource_id, Some(source_owner.id().protocol_id()));
    assert_eq!(record.error_code, Some(wp_viewport::Error::BadSize as u32));
}
