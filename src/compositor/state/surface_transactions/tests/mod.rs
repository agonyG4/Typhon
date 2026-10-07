use super::*;
use crate::compositor::state::subsurfaces::*;
use crate::compositor::state_data::{PendingViewportChange, ViewportSourceRect};
use crate::render_backend::buffer::BufferSize;
use wayland_protocols::wp::viewporter::server::wp_viewport;
use wayland_protocols::xdg::shell::server::{xdg_surface, xdg_toplevel};

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

fn test_pending_shm_buffer(
    state: &mut CompositorState,
    client: &wayland_server::Client,
    display_handle: &wayland_server::DisplayHandle,
    object_id: u32,
    width: u32,
    height: u32,
) -> PendingSurfaceBuffer {
    let width_i32 = i32::try_from(width).expect("test buffer width");
    let height_i32 = i32::try_from(height).expect("test buffer height");
    let stride = width_i32.checked_mul(4).expect("test buffer stride");
    let pool_size = stride
        .checked_mul(height_i32)
        .expect("test buffer pool size");
    let buffer_data = crate::compositor::shm::ShmBufferData {
        identity: state.allocate_buffer_identity().expect("buffer identity"),
        pool: std::sync::Arc::new(crate::compositor::shm::ShmPoolData::new(
            std::sync::Arc::new(std::fs::File::open("/dev/null").expect("test file")),
            pool_size,
        )),
        offset: 0,
        width: width_i32,
        height: height_i32,
        stride,
        format: wayland_server::WEnum::Value(wl_shm::Format::Argb8888),
    };
    let resource = client
            .create_resource::<wl_buffer::WlBuffer, crate::compositor::shm::ShmBufferData, CompositorState>(
                display_handle,
                object_id,
                buffer_data.clone(),
            )
            .expect("buffer resource");
    PendingSurfaceBuffer {
        resource,
        data: PendingBufferData::Shm(buffer_data),
        x: 0,
        y: 0,
        explicit_release: None,
        surface_size: None,
        viewport_source: None,
        viewport_destination: None,
        buffer_scale: 1,
        commit_sequence: SurfaceCommitSequence::initial(),
        resize_commit: None,
        resize_capture_finalized: false,
        buffer_transform: wl_output::Transform::Normal,
        opaque_region: SurfaceOpaqueRegion::None,
    }
}

fn test_pending_dmabuf_buffer(
    state: &mut CompositorState,
    client: &wayland_server::Client,
    display_handle: &wayland_server::DisplayHandle,
    object_id: u32,
    width: u32,
    height: u32,
) -> PendingSurfaceBuffer {
    let size = BufferSize::new(width, height).expect("test buffer size");
    let stride = width.checked_mul(4).expect("test buffer stride");
    let handle = DmabufBufferHandle::new(
        size,
        DrmFormat::Argb8888,
        vec![crate::render_backend::buffer::DmabufPlane::new(
            std::fs::File::open("/dev/null")
                .expect("test dma-buf plane")
                .into(),
            crate::render_backend::buffer::DmabufPlaneDescriptor {
                plane_index: 0,
                offset: 0,
                stride,
                modifier: DrmModifier::LINEAR,
            },
        )],
    )
    .expect("test dma-buf handle");
    let buffer_data = crate::compositor::dmabuf::DmabufBufferData {
        identity: state.allocate_buffer_identity().expect("buffer identity"),
        handle,
    };
    let resource = client
        .create_resource::<
            wl_buffer::WlBuffer,
            crate::compositor::dmabuf::DmabufBufferData,
            CompositorState,
        >(display_handle, object_id, buffer_data.clone())
        .expect("buffer resource");
    PendingSurfaceBuffer {
        resource,
        data: PendingBufferData::Dmabuf(buffer_data),
        x: 0,
        y: 0,
        explicit_release: None,
        surface_size: None,
        viewport_source: None,
        viewport_destination: None,
        buffer_scale: 1,
        commit_sequence: SurfaceCommitSequence::initial(),
        resize_commit: None,
        resize_capture_finalized: false,
        buffer_transform: wl_output::Transform::Normal,
        opaque_region: SurfaceOpaqueRegion::None,
    }
}

fn submit_test_unready_buffer_commit(
    state: &mut CompositorState,
    client: &wayland_server::Client,
    display_handle: &wayland_server::DisplayHandle,
    surface_id: u32,
    object_id: u32,
    sequence: u64,
    predecessor: Option<ContentUpdateRef>,
) -> (ContentUpdateRef, AcquireCommitId, u32, BufferId) {
    submit_test_unready_buffer_commit_with_obligations(
        state,
        client,
        display_handle,
        surface_id,
        object_id,
        sequence,
        predecessor,
        None,
        None,
    )
}

fn submit_test_unready_buffer_commit_with_obligations(
    state: &mut CompositorState,
    client: &wayland_server::Client,
    display_handle: &wayland_server::DisplayHandle,
    surface_id: u32,
    object_id: u32,
    sequence: u64,
    predecessor: Option<ContentUpdateRef>,
    frame_callback: Option<wl_callback::WlCallback>,
    presentation_feedback: Option<PendingPresentationFeedback>,
) -> (ContentUpdateRef, AcquireCommitId, u32, BufferId) {
    let mut commit = test_mergeable_commit(sequence);
    commit.lineage.predecessor = predecessor;
    let reference = commit.content_update_ref(surface_id);
    let mut buffer = test_pending_shm_buffer(state, client, display_handle, object_id, 64, 64);
    buffer.commit_sequence = commit.commit_sequence;
    let buffer_id = buffer.resource.id().protocol_id();
    let buffer_identity_id = buffer.data.buffer_id();
    commit.attachment = Some(PendingSurfaceAttachment::Buffer(buffer));
    if let Some(callback) = frame_callback {
        commit.frame_callbacks.push(callback);
    }
    if let Some(feedback) = presentation_feedback {
        commit.presentation_feedbacks.push(feedback);
    }
    let mut nodes = vec![(surface_id, commit)];
    assert!(
        state
            .prepare_surface_tree_surface_state(surface_id, &mut nodes, &[])
            .is_ok()
    );
    let acquire_commit_id = AcquireCommitId::for_tests(1_000 + sequence);
    let dependency = SurfaceTreeAcquireDependency {
        surface_commit_id: reference.commit_id,
        commit_id: acquire_commit_id,
        surface_id,
        owner_client_id: Some(client.id()),
        surface_presentation_generation: Some(1),
        buffer_id,
        acquire: ExplicitSyncPoint::for_tests_with_signal_script(
            u32::try_from(10_000 + sequence).expect("test acquire handle"),
            20_000 + sequence,
            [false],
        ),
        state: PendingAcquireState::EventfdBacked,
    };
    state.merge_or_queue_surface_tree_transaction(
        surface_id,
        nodes,
        vec![dependency],
        Vec::new(),
        SurfaceTreeSubmissionKind::ClientAdmission,
    );
    (reference, acquire_commit_id, buffer_id, buffer_identity_id)
}

fn submit_test_ready_buffer_commit(
    state: &mut CompositorState,
    client: &wayland_server::Client,
    display_handle: &wayland_server::DisplayHandle,
    surface_id: u32,
    object_id: u32,
    sequence: u64,
    predecessor: ContentUpdateRef,
) -> ContentUpdateRef {
    let mut commit = test_mergeable_commit(sequence);
    commit.lineage.predecessor = Some(predecessor);
    let reference = commit.content_update_ref(surface_id);
    let mut buffer = test_pending_shm_buffer(state, client, display_handle, object_id, 64, 64);
    buffer.commit_sequence = commit.commit_sequence;
    commit.attachment = Some(PendingSurfaceAttachment::Buffer(buffer));
    let mut nodes = vec![(surface_id, commit)];
    assert!(
        state
            .prepare_surface_tree_surface_state(surface_id, &mut nodes, &[])
            .is_ok()
    );
    state.merge_or_queue_surface_tree_transaction(
        surface_id,
        nodes,
        Vec::new(),
        Vec::new(),
        SurfaceTreeSubmissionKind::ClientAdmission,
    );
    reference
}

fn acquire_watch_was_cancelled_as_superseded(
    state: &CompositorState,
    commit_id: AcquireCommitId,
) -> bool {
    state.pending_acquire_watch_changes.iter().any(|change| {
        matches!(
            change,
            AcquireWatchChange::Cancel {
                commit_id: cancelled,
                reason: AcquireWatchCancelReason::Superseded,
            } if *cancelled == commit_id
        )
    })
}

fn test_blocking_external_dependency(
    state: &mut CompositorState,
    client: &wayland_server::Client,
    display_handle: &wayland_server::DisplayHandle,
    object_id: u32,
) -> ContentUpdateRef {
    let resource =
        state.test_create_unmapped_surface_resource_at_version(client, display_handle, object_id);
    let surface_id = compositor_surface_id(&resource);
    state.surface_presentation_generations.insert(surface_id, 1);
    ContentUpdateRef {
        surface_id,
        commit_id: SurfaceCommitId::for_tests(u64::MAX),
        commit_sequence: SurfaceCommitSequence(u64::MAX),
    }
}

fn queue_blocked_pacing_predecessor(
    state: &mut CompositorState,
    client: &wayland_server::Client,
    display_handle: &wayland_server::DisplayHandle,
    surface_id: u32,
    blocker_object_id: u32,
    mut commit: CachedSubsurfaceCommit,
) -> (ContentUpdateRef, ContentUpdateRef) {
    commit.pacing.fifo_set_barrier = true;
    let predecessor = commit.content_update_ref(surface_id);
    let blocker =
        test_blocking_external_dependency(state, client, display_handle, blocker_object_id);
    state.submit_surface_tree_nodes_with_kind(
        surface_id,
        vec![(surface_id, commit)],
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
    (predecessor, blocker)
}

fn release_blocking_dependency(state: &mut CompositorState, blocker: ContentUpdateRef) {
    state.surface_publications.insert(
        blocker.surface_id,
        SurfacePublicationState {
            latest_published: Some(blocker.commit_sequence),
            ..SurfacePublicationState::default()
        },
    );
}

fn install_test_resize_capture(
    state: &mut CompositorState,
    client: &wayland_server::Client,
    display_handle: &wayland_server::DisplayHandle,
    surface_id: u32,
) {
    let surface = state
        .surface_resource_by_id(surface_id)
        .expect("surface resource")
        .clone();
    let xdg_surface = client
        .create_resource::<xdg_surface::XdgSurface, XdgSurfaceData, CompositorState>(
            display_handle,
            20,
            XdgSurfaceData {
                surface: surface.clone(),
                reservation: XdgAssociationReservation::Fresh,
            },
        )
        .expect("test xdg surface resource");
    let toplevel = client
        .create_resource::<xdg_toplevel::XdgToplevel, XdgToplevelData, CompositorState>(
            display_handle,
            21,
            XdgToplevelData { surface },
        )
        .expect("test xdg toplevel resource");
    let window_id = WindowId::from_raw(1).expect("test window id");
    state.toplevel_surfaces.insert(
        surface_id,
        ToplevelSurface {
            window_id,
            xdg_surface,
            toplevel,
            pending_constraints: None,
            wm_capabilities_sent: false,
        },
    );
    let interaction_id = ResizeInteractionId::new(1);
    state.active_toplevel_resizes.insert(
        surface_id,
        ActiveToplevelResize {
            interaction_id,
            flow_sequence: 1,
            edges: ResizeEdges::BOTTOM_RIGHT,
            activated_at: Instant::now(),
            superseded_by_move: false,
        },
    );
    let desired = PendingResizeConfigure {
        surface_id,
        width: 50,
        height: 50,
        placement: SurfacePlacement::root(),
        edges: ResizeEdges::BOTTOM_RIGHT,
        resizing: true,
        interaction_id,
    };
    let flow = state.resize_configure_flows.entry(surface_id).or_default();
    assert!(flow.mark_sent(desired, 7, 1));
    assert_eq!(flow.ack(7), ResizeAckDecision::Matched);
}

fn test_real_merge_frozen_candidate(
    state: &mut CompositorState,
    client: &wayland_server::Client,
    display_handle: &wayland_server::DisplayHandle,
    root_surface_id: u32,
    first: CachedSubsurfaceCommit,
    mut second: CachedSubsurfaceCommit,
) -> (u32, PreparedContentUpdateCandidate) {
    let child = state.test_create_unmapped_surface_resource_at_version(client, display_handle, 2);
    let child_id = compositor_surface_id(&child);
    state.surface_presentation_generations.insert(child_id, 1);
    assert!(
        state
            .surface_transactions
            .register(child_id, root_surface_id)
    );

    let first_ref = first.content_update_ref(child_id);
    assert!(matches!(
        state.surface_transactions.cache_commit(child_id, first),
        CacheCommitOutcome::Inserted
    ));
    assert_eq!(
        state
            .surface_transactions
            .capture_direct_child_dependencies(root_surface_id),
        vec![first_ref]
    );

    second.lineage.predecessor = Some(first_ref);
    let second_ref = second.content_update_ref(child_id);
    assert!(matches!(
        state.surface_transactions.cache_commit(child_id, second),
        CacheCommitOutcome::Inserted
    ));
    assert_eq!(
        state
            .surface_transactions
            .capture_direct_child_dependencies(root_surface_id),
        vec![second_ref]
    );

    let mut root = test_mergeable_commit(second_ref.commit_sequence.0 + 1);
    root.lineage.child_dependencies = vec![first_ref, second_ref];
    let candidate = state.extract_content_update_candidate(root_surface_id, root);
    assert_eq!(
        candidate
            .nodes
            .iter()
            .filter(|(surface_id, _)| *surface_id == child_id)
            .count(),
        2
    );
    (child_id, candidate)
}

#[path = "coalescing.rs"]
mod coalescing;
#[path = "lineage.rs"]
mod lineage;
#[path = "mapping.rs"]
mod mapping;
#[path = "publication.rs"]
mod publication;
#[path = "queue.rs"]
mod queue;
