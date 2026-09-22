#![allow(clippy::question_mark)]

use super::*;
use crate::compositor::subsurface::{
    CapturedContentUpdateLineage, CapturedSubsurfaceParentState, CapturedSubsurfaceStackEntry,
    CapturedSurfaceCommitContext, ContentUpdateRef,
};

#[derive(Debug)]
struct PreparedContentUpdateCandidate {
    root_surface_id: u32,
    nodes: Vec<(u32, CachedSubsurfaceCommit)>,
    external_content_update_dependencies: Vec<ContentUpdateRef>,
}

type SurfaceMappingProjection =
    Result<Option<SurfaceContentMapping>, (SurfaceMappingError, Option<wp_viewport::WpViewport>)>;

#[derive(Debug, Clone, Copy)]
enum EffectiveContentState {
    Retained(BufferSize),
    Attached(BufferSize),
    Empty,
}

impl EffectiveContentState {
    fn buffer_size(self) -> Option<BufferSize> {
        match self {
            Self::Retained(size) | Self::Attached(size) => Some(size),
            Self::Empty => None,
        }
    }
}

#[derive(Debug, Clone)]
struct EffectiveSurfaceMappingState {
    viewport: SurfaceViewportCommit,
    buffer_scale: u32,
    buffer_transform: wl_output::Transform,
    content: EffectiveContentState,
    viewport_error_owner: Option<wp_viewport::WpViewport>,
}

impl EffectiveSurfaceMappingState {
    fn from_surface(
        data: &SurfaceData,
        current: Option<&CurrentSurfaceBuffer>,
    ) -> Result<Self, SurfaceMappingError> {
        Ok(Self {
            viewport: data.viewport_for_change(PendingViewportChange::default()),
            buffer_scale: data.buffer_scale_for_change(None),
            buffer_transform: data.buffer_transform_for_change(None),
            content: current
                .map(CurrentSurfaceBuffer::buffer_size)
                .transpose()?
                .map_or(
                    EffectiveContentState::Empty,
                    EffectiveContentState::Retained,
                ),
            viewport_error_owner: data.committed_viewport_error_owner(),
        })
    }

    fn apply_commit(
        &mut self,
        commit: &CachedSubsurfaceCommit,
    ) -> Result<Option<SurfaceContentMapping>, (SurfaceMappingError, Option<wp_viewport::WpViewport>)>
    {
        let viewport = self.viewport.apply_change(commit.viewport_destination);
        let buffer_scale = commit.buffer_scale.unwrap_or(self.buffer_scale);
        let buffer_transform = commit.buffer_transform.unwrap_or(self.buffer_transform);
        let viewport_error_owner = if commit.viewport_destination.source.is_some() {
            commit.viewport_error_owner.clone()
        } else {
            self.viewport_error_owner.clone()
        };
        let result = match commit.attachment.as_ref() {
            Some(PendingSurfaceAttachment::Buffer(pending)) => {
                let buffer_size = pending
                    .buffer_size()
                    .map_err(|error| (error, viewport_error_owner.clone()))?;
                let mapping = pending
                    .content_mapping_for_state(viewport, buffer_scale, buffer_transform)
                    .map_err(|error| (error, viewport_error_owner.clone()))?;
                (EffectiveContentState::Attached(buffer_size), Some(mapping))
            }
            Some(PendingSurfaceAttachment::RemoveContent) => {
                viewport
                    .validate_viewport_state_without_buffer()
                    .map_err(|error| (error, viewport_error_owner.clone()))?;
                (EffectiveContentState::Empty, None)
            }
            None => {
                if let Some(buffer_size) = self.content.buffer_size() {
                    viewport
                        .surface_size_for_buffer_size(buffer_size, buffer_scale, buffer_transform)
                        .map_err(|error| (error, viewport_error_owner.clone()))?;
                } else {
                    viewport
                        .validate_viewport_state_without_buffer()
                        .map_err(|error| (error, viewport_error_owner.clone()))?;
                }
                (self.content, None)
            }
        };
        self.viewport = viewport;
        self.buffer_scale = buffer_scale;
        self.buffer_transform = buffer_transform;
        self.content = result.0;
        if commit.viewport_destination.source.is_some() {
            self.viewport_error_owner = commit.viewport_error_owner.clone();
        }
        Ok(result.1)
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
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

    fn test_captured_lifetimes(
        client_id: &ClientId,
        surface_ids: &[u32],
    ) -> SurfaceTreeNodeLifetimes {
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

    fn test_blocking_external_dependency(
        state: &mut CompositorState,
        client: &wayland_server::Client,
        display_handle: &wayland_server::DisplayHandle,
        object_id: u32,
    ) -> ContentUpdateRef {
        let resource = state.test_create_unmapped_surface_resource_at_version(
            client,
            display_handle,
            object_id,
        );
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
        assert_eq!(state.pending_surface_tree_transactions.len(), 1);
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
        let child =
            state.test_create_unmapped_surface_resource_at_version(client, display_handle, 2);
        let child_id = compositor_surface_id(&child);
        state.surface_presentation_generations.insert(child_id, 1);
        assert!(
            state
                .subsurface_transactions
                .register(child_id, root_surface_id)
        );

        let first_ref = first.content_update_ref(child_id);
        assert!(matches!(
            state.subsurface_transactions.cache_commit(child_id, first),
            CacheCommitOutcome::Inserted
        ));
        assert_eq!(
            state
                .subsurface_transactions
                .capture_direct_child_dependencies(root_surface_id),
            vec![first_ref]
        );

        second.lineage.predecessor = Some(first_ref);
        let second_ref = second.content_update_ref(child_id);
        assert!(matches!(
            state.subsurface_transactions.cache_commit(child_id, second),
            CacheCommitOutcome::Inserted
        ));
        assert_eq!(
            state
                .subsurface_transactions
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

        assert!(state.pending_surface_tree_transactions.is_empty());
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

    fn test_cached_commit(sequence: u64) -> CachedSubsurfaceCommit {
        let mut commit = crate::compositor::state::empty_cached_subsurface_commit();
        commit.commit_id = SurfaceCommitId::for_tests(sequence);
        commit.commit_sequence = SurfaceCommitSequence(sequence);
        commit.pacing.fifo_set_barrier = true;
        commit
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
        assert_eq!(state.pending_surface_tree_transactions.len(), 2);
        let pending = match state.pending_surface_tree_transactions[1].nodes[0]
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

        let pending = match state.pending_surface_tree_transactions[1].nodes[0]
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

        let pending = match state.pending_surface_tree_transactions[1].nodes[0]
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

        let pending = match state.pending_surface_tree_transactions[1].nodes[0]
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

        let pending = match state.pending_surface_tree_transactions[1].nodes[0]
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
    fn candidate_extraction_follows_exact_direct_child_edges() {
        let mut state = CompositorState::default();
        assert!(state.subsurface_transactions.register(2, 1));
        assert!(state.subsurface_transactions.register(3, 2));

        let grandchild = test_cached_commit(10);
        let grandchild_ref = grandchild.content_update_ref(3);
        assert!(matches!(
            state.subsurface_transactions.cache_commit(3, grandchild),
            CacheCommitOutcome::Inserted
        ));
        let dependencies = state
            .subsurface_transactions
            .capture_direct_child_dependencies(2);
        assert_eq!(dependencies, vec![grandchild_ref]);

        let later_grandchild = test_cached_commit(11);
        assert!(matches!(
            state
                .subsurface_transactions
                .cache_commit(3, later_grandchild),
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
            .subsurface_transactions
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
        assert!(state.subsurface_transactions.register(2, 1));
        assert!(state.subsurface_transactions.register(3, 2));
        assert!(matches!(
            state
                .subsurface_transactions
                .cache_commit(3, test_cached_commit(20)),
            CacheCommitOutcome::Inserted
        ));

        let candidate = state.extract_content_update_candidate(1, test_cached_commit(21));

        assert_eq!(candidate.nodes.len(), 1);
        assert_eq!(candidate.nodes[0].0, 1);
        assert_eq!(
            state
                .subsurface_transactions
                .take_cached_commits_for_surface(3)
                .len(),
            1
        );
    }

    #[test]
    fn relationship_detach_does_not_create_a_fake_parent_update_for_a_sync_grandchild() {
        let mut state = CompositorState::default();
        assert!(state.subsurface_transactions.register(2, 1));
        assert!(state.subsurface_transactions.register(3, 2));
        assert!(matches!(
            state
                .subsurface_transactions
                .cache_commit(3, test_cached_commit(25)),
            CacheCommitOutcome::Inserted
        ));

        state.destroy_subsurface_role(2);

        assert!(state.pending_surface_tree_transactions.is_empty());
        let retained = state
            .subsurface_transactions
            .take_cached_commits_for_surface(3);
        assert_eq!(retained.len(), 1);
        assert_eq!(retained[0].commit_sequence, SurfaceCommitSequence(25));
    }

    #[test]
    fn cached_same_surface_prefix_is_emitted_in_predecessor_order() {
        let mut state = CompositorState::default();
        assert!(state.subsurface_transactions.register(2, 1));
        let mut predecessor = None;
        for sequence in 30..=32 {
            let mut commit = test_cached_commit(sequence);
            commit.lineage.predecessor = predecessor;
            predecessor = Some(commit.content_update_ref(2));
            assert!(matches!(
                state.subsurface_transactions.cache_commit(2, commit),
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
        state.pending_surface_tree_transactions.clear();
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
    fn pending_coalescing_rejects_discontinuous_lineage_without_mutating_target() {
        let mut state = CompositorState {
            external_acquire_readiness: true,
            ..CompositorState::default()
        };
        let (display, client, parent_id) = test_surface_and_client(&mut state);
        let display_handle = display.handle();
        let child_surface =
            state.test_create_unmapped_surface_resource_at_version(&client, &display_handle, 1);
        let child_id = compositor_surface_id(&child_surface);
        state.surface_presentation_generations.insert(child_id, 1);

        let acquire = SurfaceTreeAcquireDependency {
            surface_commit_id: SurfaceCommitId::for_tests(100),
            commit_id: AcquireCommitId::for_tests(101),
            surface_id: child_id,
            owner_client_id: Some(client.id()),
            surface_presentation_generation: Some(1),
            buffer_id: 308,
            acquire: ExplicitSyncPoint::for_tests_with_signal_script(102, 103, [false]),
            state: PendingAcquireState::EventfdBacked,
        };
        let a = test_mergeable_commit(1);
        let a_ref = a.content_update_ref(child_id);
        let mut p1 = test_mergeable_commit(2);
        let p1_ref = p1.content_update_ref(parent_id);
        p1.lineage.child_dependencies = vec![a_ref];
        state.queue_waiting_surface_tree(
            parent_id,
            vec![(child_id, a), (parent_id, p1)],
            vec![acquire],
        );

        let mut b = test_mergeable_commit(3);
        b.lineage.predecessor = Some(a_ref);
        let b_ref = b.content_update_ref(child_id);
        state.queue_waiting_surface_tree_with_lifetimes(
            child_id,
            vec![(child_id, b)],
            test_captured_lifetimes(&client.id(), &[child_id]),
            Vec::new(),
            vec![a_ref],
            SurfaceTreeSubmissionKind::ClientAdmission,
        );

        let mut d = test_mergeable_commit(4);
        d.lineage.predecessor = Some(b_ref);
        let mut p2 = test_mergeable_commit(5);
        p2.lineage.predecessor = Some(p1_ref);
        p2.lineage.child_dependencies = vec![d.content_update_ref(child_id)];
        state.merge_or_queue_surface_tree_transaction(
            parent_id,
            vec![(child_id, d), (parent_id, p2)],
            Vec::new(),
            vec![b_ref],
            SurfaceTreeSubmissionKind::ClientAdmission,
        );

        let parent_transactions = state
            .pending_surface_tree_transactions
            .iter()
            .filter(|transaction| transaction.root_surface_id == parent_id)
            .collect::<Vec<_>>();
        assert_eq!(parent_transactions.len(), 2);
        assert!(parent_transactions.iter().any(|transaction| {
            transaction.nodes.iter().any(|(surface_id, commit)| {
                *surface_id == child_id && commit.commit_sequence == SurfaceCommitSequence(1)
            }) && transaction.dependencies.len() == 1
        }));
        let incoming = parent_transactions
            .iter()
            .find(|transaction| {
                transaction.nodes.iter().any(|(surface_id, commit)| {
                    *surface_id == child_id && commit.commit_sequence == SurfaceCommitSequence(4)
                })
            })
            .expect("discontinuous incoming transaction remains separate");
        assert_eq!(
            incoming
                .nodes
                .iter()
                .map(|(_, commit)| commit.commit_sequence)
                .collect::<Vec<_>>(),
            vec![SurfaceCommitSequence(4), SurfaceCommitSequence(5)]
        );
        assert_eq!(incoming.external_content_update_dependencies, vec![b_ref]);
        assert!(!transaction_covers_content_update_ref(incoming, b_ref));
        assert!(!state.content_update_dependencies_ready(incoming));
        assert_eq!(state.pending_acquire_watch_changes.len(), 1);

        let target = state
            .pending_surface_tree_transactions
            .iter_mut()
            .find(|transaction| {
                transaction.root_surface_id == parent_id
                    && transaction
                        .nodes
                        .iter()
                        .any(|(_, commit)| commit.commit_sequence == SurfaceCommitSequence(1))
            })
            .expect("original target transaction");
        target.dependencies[0].state = PendingAcquireState::Ready;
        state.commit_ready_surface_tree_transactions();

        assert_eq!(
            state.surface_publications[&child_id].latest_published,
            Some(SurfaceCommitSequence(4))
        );
        assert!(state.pending_surface_tree_transactions.is_empty());
    }

    #[test]
    fn pending_coalescing_requires_contiguous_lineage_for_same_surface_replacement() {
        let a = test_mergeable_commit(10);
        let a_ref = a.content_update_ref(2);
        let mut b = test_mergeable_commit(11);
        b.lineage.predecessor = Some(a_ref);
        let b_ref = b.content_update_ref(2);

        let mut target_a = PendingSurfaceTreeTransaction {
            id: SurfaceTreeTransactionId::new(20),
            root_surface_id: 2,
            nodes: vec![(2, a)],
            publication_lifetimes: SurfaceTreeNodeLifetimes::Synthetic,
            dependencies: Vec::new(),
            external_content_update_dependencies: Vec::new(),
            commit_timing_readiness: None,
            received_at: Instant::now(),
        };
        let mut incoming_b = test_mergeable_commit(11);
        incoming_b.lineage.predecessor = Some(a_ref);
        assert!(can_coalesce_pending_surface_tree_transaction(
            &target_a,
            &[(2, incoming_b)]
        ));

        let mut merged_ab = test_mergeable_commit(10);
        let _ = merged_ab.merge(b);
        let mut incoming_c = test_mergeable_commit(12);
        incoming_c.lineage.predecessor = Some(b_ref);
        target_a.nodes[0].1 = merged_ab;
        assert!(can_coalesce_pending_surface_tree_transaction(
            &target_a,
            &[(2, incoming_c)]
        ));

        let mut incoming_d = test_mergeable_commit(12);
        incoming_d.lineage.predecessor = Some(b_ref);
        target_a.nodes[0].1 = test_mergeable_commit(10);
        assert!(!can_coalesce_pending_surface_tree_transaction(
            &target_a,
            &[(2, incoming_d)]
        ));
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
        state.pending_surface_tree_transactions.push(transaction);

        assert_eq!(
            state.pending_surface_tree_transactions[0].nodes[0]
                .1
                .commit_sequence,
            SurfaceCommitSequence(2)
        );
        assert!(
            !state.pending_surface_tree_transactions[0]
                .external_content_update_dependencies
                .contains(&predecessor_ref)
        );
        assert!(state.content_update_ref_is_pending(predecessor_ref));
        assert_eq!(
            state.pending_surface_tree_transactions[0].external_content_update_dependencies,
            vec![remaining_dependency]
        );
        assert!(
            !state.content_update_dependencies_ready(&state.pending_surface_tree_transactions[0])
        );

        state.surface_publications.insert(
            blocker_surface_id,
            SurfacePublicationState {
                latest_published: Some(remaining_dependency.commit_sequence),
                ..SurfacePublicationState::default()
            },
        );
        state.commit_ready_surface_tree_transactions();
        assert!(state.pending_surface_tree_transactions.is_empty());
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
    fn coalescing_replaces_every_node_at_the_incoming_dag_position() {
        let mut state = CompositorState::default();
        let (_display, client, _surface_id) = test_surface_and_client(&mut state);
        let g1 = test_mergeable_commit(1);
        let g1_ref = g1.content_update_ref(10);
        let c1 = test_mergeable_commit(2);
        let c1_ref = c1.content_update_ref(11);
        let p1 = test_mergeable_commit(3);
        let p1_ref = p1.content_update_ref(12);
        let mut g2 = test_mergeable_commit(4);
        g2.lineage.predecessor = Some(g1_ref);
        let g2_ref = g2.content_update_ref(10);
        let mut c2 = test_mergeable_commit(5);
        c2.lineage.predecessor = Some(c1_ref);
        c2.lineage.child_dependencies = vec![g2_ref];
        let mut p2 = test_mergeable_commit(6);
        p2.lineage.predecessor = Some(p1_ref);
        p2.lineage.child_dependencies = vec![c2.content_update_ref(11)];

        let mut transaction = PendingSurfaceTreeTransaction {
            id: SurfaceTreeTransactionId::new(12),
            root_surface_id: 12,
            nodes: vec![(10, g1), (11, c1), (12, p1)],
            publication_lifetimes: test_captured_lifetimes(&client.id(), &[10, 11, 12]),
            dependencies: Vec::new(),
            external_content_update_dependencies: Vec::new(),
            commit_timing_readiness: None,
            received_at: Instant::now(),
        };
        state.merge_surface_tree_nodes_into_transaction(
            12,
            &mut transaction,
            vec![(10, g2), (11, c2), (12, p2)],
            test_captured_lifetimes(&client.id(), &[10, 11, 12]),
            Vec::new(),
            vec![c1_ref],
        );

        assert_eq!(
            transaction
                .nodes
                .iter()
                .map(|(_, commit)| commit.commit_sequence)
                .collect::<Vec<_>>(),
            vec![
                SurfaceCommitSequence(4),
                SurfaceCommitSequence(5),
                SurfaceCommitSequence(6)
            ]
        );
        assert_eq!(
            transaction
                .publication_lifetimes
                .captured()
                .expect("captured lifetimes")
                .iter()
                .map(|lifetime| lifetime.surface_id)
                .collect::<Vec<_>>(),
            vec![10, 11, 12]
        );
        assert!(transaction.external_content_update_dependencies.is_empty());
    }

    #[test]
    fn repeated_bufferless_updates_continue_coalescing_into_one_waiting_transaction() {
        let mut state = CompositorState::default();
        let (display, client, surface_id) = test_surface_and_client(&mut state);
        let display_handle = display.handle();
        let blocking_surface =
            state.test_create_unmapped_surface_resource_at_version(&client, &display_handle, 1);
        let blocking_surface_id = compositor_surface_id(&blocking_surface);
        state
            .surface_presentation_generations
            .insert(blocking_surface_id, 1);
        let external_dependency = ContentUpdateRef {
            surface_id: blocking_surface_id,
            commit_id: SurfaceCommitId::for_tests(10_000),
            commit_sequence: SurfaceCommitSequence(10_000),
        };
        let first = test_mergeable_commit(1);
        state
            .pending_surface_tree_transactions
            .push(PendingSurfaceTreeTransaction {
                id: SurfaceTreeTransactionId::new(13),
                root_surface_id: surface_id,
                nodes: vec![(surface_id, first)],
                publication_lifetimes: test_captured_lifetimes(&client.id(), &[surface_id]),
                dependencies: Vec::new(),
                external_content_update_dependencies: vec![external_dependency],
                commit_timing_readiness: None,
                received_at: Instant::now(),
            });

        for sequence in 2..=65 {
            let predecessor = ContentUpdateRef {
                surface_id,
                commit_id: SurfaceCommitId::for_tests(sequence - 1),
                commit_sequence: SurfaceCommitSequence(sequence - 1),
            };
            let mut commit = test_mergeable_commit(sequence);
            commit.lineage.predecessor = Some(predecessor);
            state.merge_or_queue_surface_tree_transaction(
                surface_id,
                vec![(surface_id, commit)],
                Vec::new(),
                vec![predecessor],
                SurfaceTreeSubmissionKind::ClientAdmission,
            );
        }

        assert_eq!(state.pending_surface_tree_transactions.len(), 1);
        assert_eq!(
            state.pending_surface_tree_transactions[0].nodes[0]
                .1
                .commit_sequence,
            SurfaceCommitSequence(65)
        );
        assert_eq!(
            state.pending_surface_tree_transactions[0].external_content_update_dependencies,
            vec![external_dependency]
        );
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
        state.pending_surface_tree_transactions.push(owner);
        assert_eq!(
            state.pending_surface_tree_transactions[0].nodes[0]
                .1
                .commit_sequence,
            SurfaceCommitSequence(2)
        );
        assert!(transaction_covers_content_update_ref(
            &state.pending_surface_tree_transactions[0],
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
        state.pending_surface_tree_transactions.remove(0);
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
        state.retire_unpublished_work_for_xdg_role(
            surface_id,
            AcquireWatchCancelReason::RoleDestroyed,
        );
        assert!(state.content_update_ref_is_terminal(reference));
        assert!(state.pending_surface_tree_transactions.is_empty());
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

        assert_eq!(state.pending_surface_tree_transactions.len(), 1);
        let transaction = &state.pending_surface_tree_transactions[0];
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
            state.pending_surface_tree_transactions.is_empty(),
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
        assert_eq!(state.pending_surface_tree_transactions.len(), 1);
        assert_eq!(
            state.pending_surface_tree_transactions[0].nodes[0]
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

        assert!(state.pending_surface_tree_transactions.is_empty());
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

        assert_eq!(state.pending_surface_tree_transactions.len(), 1);
        assert_eq!(
            state.pending_surface_tree_transactions[0].nodes[0]
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

        assert!(state.pending_surface_tree_transactions.is_empty());
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

        let pending = match &state.pending_surface_tree_transactions[0].nodes[0]
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

        assert_eq!(state.pending_surface_tree_transactions.len(), 1);
        let pending = match &state.pending_surface_tree_transactions[0].nodes[0]
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

        let pending = match &state.pending_surface_tree_transactions[0].nodes[0]
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

        let pending = match &state.pending_surface_tree_transactions[0].nodes[0]
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

        let transaction = &state.pending_surface_tree_transactions[0];
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

        let transaction = &state.pending_surface_tree_transactions[0];
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

        let transaction = &state.pending_surface_tree_transactions[0];
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

        let transaction = &state.pending_surface_tree_transactions[0];
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
            committed_window_geometry: None,
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

        let selected = take_tree_resize_commit(root_surface_id, &mut nodes)
            .expect("latest root resize snapshot");

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

        assert_eq!(state.pending_surface_tree_transactions.len(), 2);
        let pending = &state.pending_surface_tree_transactions[1].nodes[0].1;
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
        assert!(state.pending_surface_tree_transactions.is_empty());
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

        let pending = match state.pending_surface_tree_transactions[1].nodes[0]
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

        let pending = match state.pending_surface_tree_transactions[1].nodes[0]
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

        assert_eq!(state.pending_surface_tree_transactions.len(), 2);
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

        assert!(state.pending_surface_tree_transactions.is_empty());
        let record = state
            .protocol_error_trace
            .records()
            .last()
            .expect("cross-transaction viewport error record");
        assert_eq!(record.resource_id, Some(source_owner.id().protocol_id()));
        assert_eq!(record.error_code, Some(wp_viewport::Error::BadSize as u32));
    }

    #[test]
    fn canceling_surface_retires_predecessor_and_later_projected_transaction_together() {
        let mut state = CompositorState::default();
        let (display, client, surface_id) = test_surface_and_client(&mut state);
        let display_handle = display.handle();
        let mut first = test_mergeable_commit(260);
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

        let mut second = test_mergeable_commit(261);
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
        assert_eq!(state.pending_surface_tree_transactions.len(), 2);
        let pending = match state.pending_surface_tree_transactions[1].nodes[0]
            .1
            .attachment
            .as_ref()
        {
            Some(PendingSurfaceAttachment::Buffer(buffer)) => buffer,
            other => panic!("expected prepared buffer, got {other:?}"),
        };
        assert_eq!(pending.surface_size, Some(BufferSize::new(50, 50).unwrap()));

        let release_before = state.buffer_release_metrics();
        state.cancel_pending_surface_trees_for_surface(
            surface_id,
            AcquireWatchCancelReason::SurfaceDestroyed,
        );

        assert!(state.pending_surface_tree_transactions.is_empty());
        assert!(!state.current_surface_buffers.contains_key(&surface_id));
        assert!(state.renderable_surface(surface_id).is_none());
        let release_after = state.buffer_release_metrics();
        assert_eq!(
            release_after.buffer_releases_completed,
            release_before.buffer_releases_completed + 1
        );
    }

    #[test]
    fn root_cancellation_discards_pending_dependents_after_topology_churn() {
        let mut state = CompositorState::default();
        let (_display, _client, surface_id) = test_surface_and_client(&mut state);
        let first = test_mergeable_commit(270);
        let first_ref = first.content_update_ref(surface_id);
        let mut second = test_mergeable_commit(271);
        second.lineage.predecessor = Some(first_ref);

        state.pending_surface_tree_transactions.extend([
            PendingSurfaceTreeTransaction {
                id: SurfaceTreeTransactionId::new(1),
                root_surface_id: 700,
                nodes: vec![(surface_id, first)],
                publication_lifetimes: SurfaceTreeNodeLifetimes::Synthetic,
                dependencies: Vec::new(),
                external_content_update_dependencies: Vec::new(),
                commit_timing_readiness: None,
                received_at: Instant::now(),
            },
            PendingSurfaceTreeTransaction {
                id: SurfaceTreeTransactionId::new(2),
                root_surface_id: 701,
                nodes: vec![(surface_id, second)],
                publication_lifetimes: SurfaceTreeNodeLifetimes::Synthetic,
                dependencies: Vec::new(),
                external_content_update_dependencies: vec![first_ref],
                commit_timing_readiness: None,
                received_at: Instant::now(),
            },
        ]);
        state
            .cancel_pending_surface_trees_for_root(700, AcquireWatchCancelReason::SurfaceDestroyed);

        assert!(state.pending_surface_tree_transactions.is_empty());
    }
}

struct ContentUpdateCandidateExtractor<'a> {
    state: &'a mut CompositorState,
    nodes: Vec<(u32, CachedSubsurfaceCommit)>,
    external_content_update_dependencies: Vec<ContentUpdateRef>,
    seen: HashSet<ContentUpdateRef>,
    visiting: HashSet<ContentUpdateRef>,
}

impl ContentUpdateCandidateExtractor<'_> {
    fn visit_commit(&mut self, surface_id: u32, commit: CachedSubsurfaceCommit) {
        let reference = commit.content_update_ref(surface_id);
        if !self.seen.insert(reference) {
            return;
        }
        if !self.visiting.insert(reference) {
            debug_assert!(false, "content update dependency cycle detected");
            return;
        }
        let lineage = commit.lineage.clone();
        if let Some(predecessor) = lineage.predecessor {
            self.visit_reference(predecessor);
        }
        for dependency in lineage.child_dependencies {
            self.visit_reference(dependency);
        }
        self.visiting.remove(&reference);
        self.nodes.push((surface_id, commit));
    }

    fn visit_reference(&mut self, reference: ContentUpdateRef) {
        if self.visiting.contains(&reference) {
            debug_assert!(false, "content update dependency cycle detected");
            return;
        }
        if self.seen.contains(&reference) {
            return;
        }
        if let Some(commits) = self
            .state
            .subsurface_transactions
            .take_cached_commits_through(reference)
        {
            for commit in commits {
                self.visit_commit(reference.surface_id, commit);
            }
            return;
        }
        if self.state.content_update_ref_is_published(reference) {
            return;
        }
        if self.state.content_update_ref_is_terminal(reference) {
            return;
        }
        if !self
            .external_content_update_dependencies
            .contains(&reference)
        {
            self.external_content_update_dependencies.push(reference);
        }
    }
}

fn content_update_node_covers_ref(
    node_surface_id: u32,
    commit: &CachedSubsurfaceCommit,
    reference: ContentUpdateRef,
) -> bool {
    if let Some(predecessor) = commit.lineage.predecessor {
        debug_assert_eq!(predecessor.surface_id, node_surface_id);
        debug_assert!(predecessor.commit_sequence < commit.commit_sequence);
    }
    if reference.surface_id != node_surface_id || reference.commit_sequence > commit.commit_sequence
    {
        return false;
    }
    if let Some(predecessor) = commit.lineage.predecessor
        && reference.commit_sequence <= predecessor.commit_sequence
    {
        return false;
    }
    if reference.commit_sequence == commit.commit_sequence {
        debug_assert_eq!(reference.commit_id, commit.commit_id);
        return reference.commit_id == commit.commit_id;
    }
    true
}

fn node_index_covering_content_update_ref(
    nodes: &[(u32, CachedSubsurfaceCommit)],
    reference: ContentUpdateRef,
) -> Option<usize> {
    let mut covering_index = None;
    for (index, (surface_id, commit)) in nodes.iter().enumerate() {
        if !content_update_node_covers_ref(*surface_id, commit, reference) {
            continue;
        }
        debug_assert!(
            covering_index.is_none(),
            "overlapping represented Content Update ranges"
        );
        covering_index.get_or_insert(index);
    }
    covering_index
}

fn transaction_node_index_covering_content_update_ref(
    transaction: &PendingSurfaceTreeTransaction,
    reference: ContentUpdateRef,
) -> Option<usize> {
    node_index_covering_content_update_ref(&transaction.nodes, reference)
}

fn transaction_covers_content_update_ref(
    transaction: &PendingSurfaceTreeTransaction,
    reference: ContentUpdateRef,
) -> bool {
    transaction_node_index_covering_content_update_ref(transaction, reference).is_some()
}

fn transaction_references_any_content_update(
    transaction: &PendingSurfaceTreeTransaction,
    references: &[ContentUpdateRef],
) -> bool {
    let references_node = |commit: &CachedSubsurfaceCommit| {
        commit
            .lineage
            .predecessor
            .is_some_and(|predecessor| references.contains(&predecessor))
            || commit
                .lineage
                .child_dependencies
                .iter()
                .any(|dependency| references.contains(dependency))
    };
    transaction
        .external_content_update_dependencies
        .iter()
        .any(|dependency| references.contains(dependency))
        || transaction
            .nodes
            .iter()
            .any(|(_, commit)| references_node(commit))
}

fn add_unique_content_update_ref(
    references: &mut Vec<ContentUpdateRef>,
    reference: ContentUpdateRef,
) -> bool {
    if references.contains(&reference) {
        return false;
    }
    references.push(reference);
    true
}

fn can_coalesce_pending_surface_tree_transaction(
    target: &PendingSurfaceTreeTransaction,
    incoming_nodes: &[(u32, CachedSubsurfaceCommit)],
) -> bool {
    if target.ordering() != TransactionOrdering::Coalescible
        || target
            .nodes
            .iter()
            .any(|(_, commit)| commit.lineage.merge_frozen)
        || incoming_nodes
            .iter()
            .any(|(_, commit)| commit.pacing.is_boundary() || commit.lineage.merge_frozen)
    {
        debug_assert_eq!(target.ordering(), TransactionOrdering::Coalescible);
        debug_assert!(
            incoming_nodes
                .iter()
                .all(|(_, commit)| !commit.pacing.is_boundary())
        );
        return false;
    }

    for (incoming_index, (surface_id, incoming)) in incoming_nodes.iter().enumerate() {
        if incoming_nodes[..incoming_index]
            .iter()
            .any(|(previous_surface_id, _)| previous_surface_id == surface_id)
        {
            debug_assert!(false, "coalescible candidate retains duplicate surfaces");
            return false;
        }
        let mut matching = target
            .nodes
            .iter()
            .filter(|(existing_surface_id, _)| existing_surface_id == surface_id);
        let Some((_, existing)) = matching.next() else {
            continue;
        };
        if matching.next().is_some() {
            debug_assert!(false, "coalescible transaction retains duplicate surfaces");
            return false;
        }
        if existing.commit_sequence >= incoming.commit_sequence {
            return false;
        }
        let Some(predecessor) = incoming.lineage.predecessor else {
            return false;
        };
        if predecessor.surface_id != *surface_id
            || predecessor.commit_sequence >= incoming.commit_sequence
            || !content_update_node_covers_ref(*surface_id, existing, predecessor)
        {
            return false;
        }
    }
    true
}

fn normalize_external_content_dependencies_for_nodes(
    nodes: &[(u32, CachedSubsurfaceCommit)],
    dependencies: &mut Vec<ContentUpdateRef>,
) {
    let input = std::mem::take(dependencies);
    let normalized = input
        .into_iter()
        .filter(|dependency| node_index_covering_content_update_ref(nodes, *dependency).is_none())
        .fold(Vec::new(), |mut normalized, dependency| {
            if !normalized.contains(&dependency) {
                normalized.push(dependency);
            }
            normalized
        });
    *dependencies = normalized;
    debug_assert!(dependencies.iter().all(|dependency| {
        node_index_covering_content_update_ref(nodes, *dependency).is_none()
    }));
}

fn normalize_transaction_external_content_dependencies(
    transaction: &mut PendingSurfaceTreeTransaction,
) {
    let dependencies = std::mem::take(&mut transaction.external_content_update_dependencies);
    transaction.external_content_update_dependencies = dependencies
        .into_iter()
        .filter(|dependency| !transaction_covers_content_update_ref(transaction, *dependency))
        .fold(Vec::new(), |mut normalized, dependency| {
            if !normalized.contains(&dependency) {
                normalized.push(dependency);
            }
            normalized
        });
    debug_assert!(
        transaction
            .external_content_update_dependencies
            .iter()
            .all(|dependency| !transaction_covers_content_update_ref(transaction, *dependency))
    );
}

fn normalize_surface_tree_node_order(transaction: &mut PendingSurfaceTreeTransaction) {
    let node_count = transaction.nodes.len();
    if node_count < 2 {
        return;
    }
    let mut indegree = vec![0usize; node_count];
    let mut successors = vec![Vec::<usize>::new(); node_count];
    for (dependent_index, ((_, commit), indegree_entry)) in transaction
        .nodes
        .iter()
        .zip(indegree.iter_mut())
        .enumerate()
    {
        let mut dependencies = Vec::with_capacity(
            commit.lineage.child_dependencies.len()
                + usize::from(commit.lineage.predecessor.is_some()),
        );
        if let Some(predecessor) = commit.lineage.predecessor {
            dependencies.push(predecessor);
        }
        dependencies.extend(commit.lineage.child_dependencies.iter().copied());
        for dependency in dependencies {
            let Some(dependency_index) =
                transaction_node_index_covering_content_update_ref(transaction, dependency)
            else {
                continue;
            };
            if dependency_index == dependent_index
                || successors[dependency_index].contains(&dependent_index)
            {
                continue;
            }
            successors[dependency_index].push(dependent_index);
            *indegree_entry = indegree_entry.saturating_add(1);
        }
    }

    let mut selected = vec![false; node_count];
    let mut order = Vec::with_capacity(node_count);
    for _ in 0..node_count {
        let Some(next) = (0..node_count).find(|index| !selected[*index] && indegree[*index] == 0)
        else {
            debug_assert!(
                false,
                "surface-tree content update dependency cycle detected"
            );
            return;
        };
        selected[next] = true;
        order.push(next);
        for successor in &successors[next] {
            indegree[*successor] = indegree[*successor].saturating_sub(1);
        }
    }
    if order
        .iter()
        .enumerate()
        .all(|(index, original)| index == *original)
    {
        return;
    }

    let old_nodes = std::mem::take(&mut transaction.nodes);
    let old_lifetimes = std::mem::replace(
        &mut transaction.publication_lifetimes,
        SurfaceTreeNodeLifetimes::Captured(Vec::new()),
    );
    let lifetimes = old_lifetimes
        .captured()
        .expect("surface-tree transactions use captured publication lifetimes")
        .to_vec();
    debug_assert_eq!(old_nodes.len(), lifetimes.len());
    let mut node_slots = old_nodes.into_iter().map(Some).collect::<Vec<_>>();
    let mut lifetime_slots = lifetimes.into_iter().map(Some).collect::<Vec<_>>();
    let mut reordered_nodes = Vec::with_capacity(node_count);
    let mut reordered_lifetimes = Vec::with_capacity(node_count);
    for index in order {
        reordered_nodes.push(node_slots[index].take().expect("node order index"));
        reordered_lifetimes.push(
            lifetime_slots[index]
                .take()
                .expect("node lifetime order index"),
        );
    }
    transaction.nodes = reordered_nodes;
    transaction.publication_lifetimes = SurfaceTreeNodeLifetimes::Captured(reordered_lifetimes);
}

#[cfg(any(debug_assertions, test))]
fn debug_assert_surface_tree_content_update_invariants(
    transaction: &PendingSurfaceTreeTransaction,
) {
    let lifetimes = transaction
        .publication_lifetimes
        .captured()
        .expect("surface-tree transactions use captured publication lifetimes");
    debug_assert_eq!(lifetimes.len(), transaction.nodes.len());
    for (node_index, (surface_id, commit)) in transaction.nodes.iter().enumerate() {
        debug_assert_eq!(lifetimes[node_index].surface_id, *surface_id);
        if let Some(predecessor) = commit.lineage.predecessor {
            debug_assert_eq!(predecessor.surface_id, *surface_id);
            debug_assert!(predecessor.commit_sequence < commit.commit_sequence);
        }
        for dependency in &commit.lineage.child_dependencies {
            debug_assert!(dependency.commit_sequence < commit.commit_sequence);
            if let Some(dependency_index) =
                transaction_node_index_covering_content_update_ref(transaction, *dependency)
            {
                debug_assert!(dependency_index <= node_index);
            }
        }
    }
    for (first_index, (first_surface_id, first)) in transaction.nodes.iter().enumerate() {
        for (later_surface_id, later) in transaction.nodes.iter().skip(first_index + 1) {
            if first_surface_id != later_surface_id {
                continue;
            }
            if transaction.ordering() == TransactionOrdering::Coalescible {
                debug_assert!(
                    false,
                    "coalescible transaction retains multiple ranges for one surface"
                );
            }
            debug_assert!(first.commit_sequence < later.commit_sequence);
            debug_assert!(!content_update_node_covers_ref(
                *later_surface_id,
                later,
                first.content_update_ref(*first_surface_id),
            ));
            debug_assert!(!content_update_node_covers_ref(
                *first_surface_id,
                first,
                later.content_update_ref(*later_surface_id),
            ));
        }
    }
    debug_assert!(
        transaction
            .external_content_update_dependencies
            .iter()
            .all(|dependency| !transaction_covers_content_update_ref(transaction, *dependency))
    );
}

#[cfg(not(any(debug_assertions, test)))]
#[inline]
fn debug_assert_surface_tree_content_update_invariants(
    _transaction: &PendingSurfaceTreeTransaction,
) {
}

impl CompositorState {
    const MAX_SURFACE_TREE_TRANSACTIONS_PER_ROOT: usize = 8;

    fn content_update_ref_is_published(&self, reference: ContentUpdateRef) -> bool {
        self.surface_publications
            .get(&reference.surface_id)
            .and_then(|publication| publication.latest_published)
            .is_some_and(|published| published >= reference.commit_sequence)
    }

    fn content_update_ref_is_pending(&self, reference: ContentUpdateRef) -> bool {
        self.pending_surface_tree_transactions
            .iter()
            .any(|transaction| transaction_covers_content_update_ref(transaction, reference))
    }

    fn content_update_ref_is_terminal(&self, reference: ContentUpdateRef) -> bool {
        if !self.surface_resources.contains_key(&reference.surface_id)
            || self
                .surface_client_ids
                .get(&reference.surface_id)
                .is_some_and(|client_id| self.terminal_client_ids.contains(client_id))
        {
            return true;
        }
        self.surface_publications
            .get(&reference.surface_id)
            .and_then(|publication| publication.latest_terminal)
            .is_some_and(|terminal| {
                debug_assert!(reference.commit_sequence <= terminal);
                reference.commit_sequence <= terminal
            })
    }

    pub(in crate::compositor) fn content_update_dependencies_ready(
        &self,
        transaction: &PendingSurfaceTreeTransaction,
    ) -> bool {
        transaction
            .external_content_update_dependencies
            .iter()
            .all(|dependency| {
                transaction_covers_content_update_ref(transaction, *dependency)
                    || self.content_update_ref_is_published(*dependency)
                    || (!self.content_update_ref_is_pending(*dependency)
                        && self.content_update_ref_is_terminal(*dependency))
            })
    }

    fn extract_content_update_candidate(
        &mut self,
        root_surface_id: u32,
        root_commit: CachedSubsurfaceCommit,
    ) -> PreparedContentUpdateCandidate {
        let mut extractor = ContentUpdateCandidateExtractor {
            state: self,
            nodes: Vec::new(),
            external_content_update_dependencies: Vec::new(),
            seen: HashSet::new(),
            visiting: HashSet::new(),
        };
        extractor.visit_commit(root_surface_id, root_commit);
        PreparedContentUpdateCandidate {
            root_surface_id,
            nodes: extractor.nodes,
            external_content_update_dependencies: extractor.external_content_update_dependencies,
        }
    }

    pub(in crate::compositor) fn capture_content_update_lineage(
        &mut self,
        surface_id: u32,
        commit_id: SurfaceCommitId,
        commit_sequence: SurfaceCommitSequence,
    ) -> CapturedContentUpdateLineage {
        let predecessor = self
            .surface_publications
            .get(&surface_id)
            .map(|publication| publication.latest_received)
            .filter(|sequence| *sequence != SurfaceCommitSequence::initial())
            .map(|previous_sequence| ContentUpdateRef {
                surface_id,
                commit_id: SurfaceCommitId::from_sequence(previous_sequence),
                commit_sequence: previous_sequence,
            });
        let child_dependencies = self
            .subsurface_transactions
            .capture_direct_child_dependencies(surface_id);
        debug_assert!(child_dependencies.iter().all(|dependency| {
            dependency.commit_sequence < commit_sequence && dependency.commit_id != commit_id
        }));
        CapturedContentUpdateLineage {
            predecessor,
            child_dependencies,
            merge_frozen: false,
        }
    }

    fn normalize_explicit_sync_commit(&mut self, commit: &mut CachedSubsurfaceCommit) -> bool {
        let has_buffer = matches!(
            commit.attachment.as_ref(),
            Some(PendingSurfaceAttachment::Buffer(_))
        );
        let Some(explicit_sync) = commit.explicit_sync.as_ref() else {
            return true;
        };
        if has_buffer {
            return true;
        }
        if explicit_sync.has_points() {
            explicit_sync.state.post_error_with_metrics(
                &mut self.compliance_metrics,
                &mut self.protocol_error_trace,
                &mut self.terminal_client_ids,
                SYNCOBJ_SURFACE_ERROR_NO_BUFFER,
                "explicit sync points were set without an attached buffer",
            );
            return false;
        }
        commit.explicit_sync = None;
        true
    }

    pub(in crate::compositor) fn register_subsurface_relationship(
        &mut self,
        surface_id: u32,
        parent_id: u32,
        client_id: ClientId,
    ) -> bool {
        if !self.subsurface_transactions.register_with_client(
            surface_id,
            parent_id,
            Some(client_id),
        ) {
            return false;
        }
        self.add_subsurface_to_pending_stack(parent_id, surface_id);
        true
    }

    pub(in crate::compositor) fn is_effectively_synchronized_subsurface(
        &self,
        surface_id: u32,
    ) -> bool {
        self.subsurface_transactions
            .is_effectively_synchronized(surface_id)
    }

    pub(in crate::compositor) fn subsurface_content_is_inactive(&self, surface_id: u32) -> bool {
        match self.surface_role(surface_id) {
            SurfaceRole::Subsurface { parent_id } => {
                !self
                    .subsurface_transactions
                    .relationship_is_applied_child_of(surface_id, parent_id)
                    || !self.subsurface_parent_is_mapped(parent_id)
            }
            // Destroying wl_subsurface removes only the live relationship. The
            // permanent role remains, so dormant subsurface commits must retain
            // current content without becoming eligible for presentation.
            SurfaceRole::Unassigned
                if self.permanent_surface_role(surface_id)
                    == Some(PermanentSurfaceRole::Subsurface) =>
            {
                true
            }
            _ => false,
        }
    }

    pub(in crate::compositor) fn subsurface_parent_is_mapped(&self, parent_id: u32) -> bool {
        self.renderable_surface_index(parent_id).is_some()
    }

    pub(in crate::compositor) fn subsurface_can_map(&self, surface_id: u32) -> bool {
        let SurfaceRole::Subsurface { parent_id } = self.surface_role(surface_id) else {
            return false;
        };
        self.subsurface_transactions
            .relationship_is_applied_child_of(surface_id, parent_id)
            && self.subsurface_parent_is_mapped(parent_id)
            && self.current_surface_buffers.contains_key(&surface_id)
    }

    pub(in crate::compositor) fn reconcile_applied_subsurface_mapping(
        &mut self,
        parent_id: u32,
    ) -> bool {
        if !self.subsurface_parent_is_mapped(parent_id) {
            return false;
        }
        let mut changed = false;
        for child_id in self.subsurface_transactions.applied_children_of(parent_id) {
            changed |= self.adopt_current_surface_content_for_role(child_id);
        }
        changed
    }

    pub(in crate::compositor) fn set_subsurface_sync_mode(
        &mut self,
        surface_id: u32,
        mode: SubsurfaceSyncMode,
    ) {
        if self.subsurface_transactions.requested_mode(surface_id) == Some(mode) {
            return;
        }
        let affected_surfaces = self.subsurface_transactions.subsurface_tree_ids(surface_id);
        let was_effectively_synchronized = affected_surfaces
            .iter()
            .map(|surface_id| {
                (
                    *surface_id,
                    self.is_effectively_synchronized_subsurface(*surface_id),
                )
            })
            .collect::<HashMap<_, _>>();
        if !self.subsurface_transactions.set_mode(surface_id, mode) {
            return;
        }
        self.reclassify_unreachable_synchronized_commits(
            &affected_surfaces,
            &was_effectively_synchronized,
            None,
        );
    }

    fn reclassify_unreachable_synchronized_commits(
        &mut self,
        affected_surfaces: &[u32],
        was_effectively_synchronized: &HashMap<u32, bool>,
        detached_root_commits: Option<(u32, Vec<CachedSubsurfaceCommit>)>,
    ) {
        let mut detached_root_commits = detached_root_commits;
        for surface_id in affected_surfaces {
            let was_synchronized = was_effectively_synchronized
                .get(surface_id)
                .copied()
                .unwrap_or(false);
            if !was_synchronized || self.is_effectively_synchronized_subsurface(*surface_id) {
                continue;
            }
            let commits = detached_root_commits
                .take()
                .filter(|(detached_surface_id, _)| detached_surface_id == surface_id)
                .map(|(_, commits)| commits)
                .unwrap_or_default();
            if !commits.is_empty() {
                for commit in commits {
                    let candidate = self.extract_content_update_candidate(*surface_id, commit);
                    self.submit_content_update_candidate(
                        candidate,
                        SurfaceTreeSubmissionKind::InternalMigration,
                    );
                }
            }
            while let Some(reference) = self
                .subsurface_transactions
                .oldest_cached_content_update_ref(*surface_id)
            {
                let Some(mut commits) = self
                    .subsurface_transactions
                    .take_cached_commits_through(reference)
                else {
                    break;
                };
                for commit in commits.drain(..) {
                    let candidate = self.extract_content_update_candidate(*surface_id, commit);
                    self.submit_content_update_candidate(
                        candidate,
                        SurfaceTreeSubmissionKind::InternalMigration,
                    );
                }
            }
        }
        self.update_synchronized_cache_metrics();
        self.commit_ready_surface_tree_transactions();
    }

    fn submit_content_update_candidate(
        &mut self,
        candidate: PreparedContentUpdateCandidate,
        submission_kind: SurfaceTreeSubmissionKind,
    ) {
        self.submit_surface_tree_nodes_with_kind(
            candidate.root_surface_id,
            candidate.nodes,
            candidate.external_content_update_dependencies,
            submission_kind,
        );
    }

    pub(in crate::compositor) fn set_pending_subsurface_position(
        &mut self,
        surface_id: u32,
        x: i32,
        y: i32,
    ) {
        self.subsurface_transactions
            .set_pending_position(surface_id, x, y);
    }

    fn capture_surface_commit_context(
        &mut self,
        surface_id: u32,
        commit_sequence: SurfaceCommitSequence,
    ) -> Result<CapturedSurfaceCommitContext, ()> {
        let layer_surface = self.capture_layer_surface_commit_state(surface_id)?;
        let xdg_decoration = self.capture_xdg_decoration_commit_state(surface_id, commit_sequence);
        let activations = self
            .subsurface_transactions
            .take_pending_relationship_activations_for_parent(surface_id);
        let positions = self
            .subsurface_transactions
            .take_pending_positions_for_parent(surface_id);
        let live_stack = self.pending_subsurface_stacks.remove(&surface_id);
        if let Some(stack) = &live_stack {
            self.latched_subsurface_stacks
                .insert(surface_id, stack.clone());
        }
        let stack = live_stack.map(|stack| {
            self.subsurface_transactions
                .capture_subsurface_stack(surface_id, stack)
        });
        Ok(CapturedSurfaceCommitContext {
            subsurface_parent: CapturedSubsurfaceParentState {
                activations,
                positions,
                stack,
            },
            layer_surface,
            xdg_decoration,
        })
    }

    pub(in crate::compositor) fn commit_surface_tree_request(
        &mut self,
        surface_id: u32,
        mut commit: CachedSubsurfaceCommit,
    ) {
        let commit_context =
            match self.capture_surface_commit_context(surface_id, commit.commit_sequence) {
                Ok(context) => context,
                Err(()) => {
                    self.release_unpublished_surface_tree_nodes(vec![(surface_id, commit)]);
                    return;
                }
            };
        commit.commit_context = commit_context;
        if !self.normalize_explicit_sync_commit(&mut commit) {
            self.release_unpublished_surface_tree_nodes(vec![(surface_id, commit)]);
            return;
        }
        if self.xdg_surface_is_constructed(surface_id) {
            match commit.attachment.as_ref() {
                Some(PendingSurfaceAttachment::Buffer(_))
                    if !self.xdg_surface_is_configured(surface_id) =>
                {
                    if let Some(surface) = self.surface_resource_by_id(surface_id)
                        && let Some(client) = surface.client()
                        && let Some(xdg_surface) =
                            self.xdg_surface_resources.get(&surface_id).cloned()
                    {
                        self.post_protocol_error(
                            &client,
                            &xdg_surface,
                            xdg_surface::Error::UnconfiguredBuffer,
                            "xdg_surface buffer commit was not preceded by an acknowledged configure"
                                .to_string(),
                        );
                    }
                    self.release_unpublished_surface_tree_nodes(vec![(surface_id, commit)]);
                    return;
                }
                Some(PendingSurfaceAttachment::RemoveContent) => {
                    self.begin_xdg_empty_or_unmap_commit(surface_id);
                    self.configure_xdg_surface_if_needed(surface_id);
                }
                None => {
                    if self.mark_xdg_empty_commit(surface_id) {
                        self.configure_xdg_surface_if_needed(surface_id);
                    }
                }
                _ => {}
            }
        }
        let synchronized = self.is_effectively_synchronized_subsurface(surface_id);
        let direct_mapping = if synchronized {
            None
        } else {
            match self.derive_surface_mapping_for_commit(surface_id, &commit) {
                Some(Ok(mapping)) => mapping,
                Some(Err((error, viewport_error_owner))) => {
                    self.post_surface_mapping_error(surface_id, error, viewport_error_owner);
                    self.release_unpublished_surface_tree_nodes(vec![(surface_id, commit)]);
                    return;
                }
                None => None,
            }
        };
        if synchronized {
            self.cache_synchronized_subsurface_commit(surface_id, commit);
            return;
        }
        if commit.attachment.is_some() {
            let mut superseded_callbacks = self.supersede_older_pending_attachments_for_surface(
                surface_id,
                commit.commit_sequence,
            );
            superseded_callbacks.extend(commit.frame_callbacks);
            commit.frame_callbacks = superseded_callbacks;
        }
        match commit.attachment.as_mut() {
            Some(PendingSurfaceAttachment::Buffer(pending)) => {
                if let Some(mapping) = direct_mapping {
                    pending.apply_content_mapping(mapping);
                }
                self.finalize_pending_buffer_resize_capture(
                    surface_id,
                    pending,
                    commit.window_geometry,
                );
            }
            _ => {
                commit.resize_commit = self
                    .capture_acked_resize_for_surface_commit(surface_id)
                    .map(|snapshot| {
                        commit.window_geometry.map_or(snapshot, |window_geometry| {
                            snapshot.with_committed_window_geometry(window_geometry)
                        })
                    });
                commit.resize_capture_finalized = true;
            }
        }
        let candidate = self.extract_content_update_candidate(surface_id, commit);
        self.update_synchronized_cache_metrics();
        self.submit_surface_tree_nodes_with_kind(
            candidate.root_surface_id,
            candidate.nodes,
            candidate.external_content_update_dependencies,
            SurfaceTreeSubmissionKind::ClientAdmission,
        );
    }

    fn derive_surface_mapping_for_commit(
        &self,
        surface_id: u32,
        commit: &CachedSubsurfaceCommit,
    ) -> Option<SurfaceMappingProjection> {
        let surface = self.surface_resource_by_id(surface_id)?;
        let data = surface.data::<SurfaceData>()?;
        let mut effective_state = match EffectiveSurfaceMappingState::from_surface(
            data,
            self.current_surface_buffers.get(&surface_id),
        ) {
            Ok(state) => state,
            Err(error) => return Some(Err((error, data.committed_viewport_error_owner()))),
        };
        let mut references = Vec::new();
        if let Some(predecessor) = commit.lineage.predecessor {
            references.push(predecessor);
        }
        references.extend(commit.lineage.child_dependencies.iter().copied());
        for transaction in self.pending_surface_tree_mapping_prefix(
            self.root_surface_id_for_surface(surface_id),
            &references,
        ) {
            for (pending_surface_id, pending_commit) in &transaction.nodes {
                if *pending_surface_id != surface_id {
                    continue;
                }
                if let Err((error, viewport_error_owner)) =
                    effective_state.apply_commit(pending_commit)
                {
                    debug_assert!(
                        false,
                        "admitted pending surface-tree mapping became invalid while projecting: {error:?}"
                    );
                    return Some(Err((error, viewport_error_owner)));
                }
            }
        }
        Some(effective_state.apply_commit(commit))
    }

    fn submit_surface_tree_nodes_with_kind(
        &mut self,
        surface_id: u32,
        mut nodes: Vec<(u32, CachedSubsurfaceCommit)>,
        external_content_update_dependencies: Vec<ContentUpdateRef>,
        submission_kind: SurfaceTreeSubmissionKind,
    ) {
        if let Err((error_surface_id, error, viewport_error_owner)) = self
            .validate_surface_tree_surface_state(
                surface_id,
                &nodes,
                &external_content_update_dependencies,
            )
        {
            if compositor_debug_surface_logging_enabled() {
                eprintln!(
                    "oblivion-one compositor: surface_commit validation failed surface={error_surface_id}"
                );
            }
            self.post_surface_mapping_error(error_surface_id, error, viewport_error_owner);
            self.release_unpublished_surface_tree_nodes(nodes);
            return;
        }
        if nodes
            .iter()
            .all(|(_, commit)| !commit.pacing.is_boundary() && !commit.lineage.merge_frozen)
        {
            nodes = self.canonicalize_coalescible_surface_tree_nodes(nodes);
        }
        if let Err((error_surface_id, error, viewport_error_owner)) = self
            .prepare_surface_tree_surface_state(
                surface_id,
                &mut nodes,
                &external_content_update_dependencies,
            )
        {
            if compositor_debug_surface_logging_enabled() {
                eprintln!(
                    "oblivion-one compositor: surface_commit validation failed surface={error_surface_id}"
                );
            }
            self.post_surface_mapping_error(error_surface_id, error, viewport_error_owner);
            self.release_unpublished_surface_tree_nodes(nodes);
            return;
        }
        let mut external_content_update_dependencies = external_content_update_dependencies;
        normalize_external_content_dependencies_for_nodes(
            &nodes,
            &mut external_content_update_dependencies,
        );
        let Some(dependencies) = self.prepare_surface_tree_acquires(&mut nodes) else {
            self.release_unpublished_surface_tree_nodes(nodes);
            return;
        };
        self.subsurface_transaction_metrics
            .tree_transactions_prepared = self
            .subsurface_transaction_metrics
            .tree_transactions_prepared
            .saturating_add(1);
        self.merge_or_queue_surface_tree_transaction(
            surface_id,
            nodes,
            dependencies,
            external_content_update_dependencies,
            submission_kind,
        );
    }

    fn canonicalize_coalescible_surface_tree_nodes(
        &mut self,
        nodes: Vec<(u32, CachedSubsurfaceCommit)>,
    ) -> Vec<(u32, CachedSubsurfaceCommit)> {
        debug_assert!(
            nodes.iter().all(|(_, commit)| {
                !commit.pacing.is_boundary() && !commit.lineage.merge_frozen
            })
        );
        let mut canonical = Vec::with_capacity(nodes.len());
        for (surface_id, newer) in nodes {
            let Some(existing_index) = canonical
                .iter()
                .position(|(existing_surface_id, _)| *existing_surface_id == surface_id)
            else {
                canonical.push((surface_id, newer));
                continue;
            };
            let existing = &mut canonical[existing_index].1;
            debug_assert!(existing.commit_sequence < newer.commit_sequence);
            if let Some(predecessor) = newer.lineage.predecessor {
                debug_assert_eq!(predecessor.surface_id, surface_id);
                debug_assert!(content_update_node_covers_ref(
                    surface_id,
                    existing,
                    predecessor
                ));
            }
            let attachment_changed = newer.attachment.is_some();
            let old_resize_commit = attachment_changed
                .then(|| pending_node_resize_commit(existing))
                .flatten();
            let previous_commit_id = existing.commit_id;
            let previous_callback_count = existing.frame_callbacks.len();
            let replacement_commit_id = newer.commit_id;
            if let Some(release) = existing.merge(newer) {
                self.release_pending_surface_buffer(release);
            }
            if previous_commit_id != replacement_commit_id {
                self.note_explicit_commit_merged(
                    previous_commit_id,
                    replacement_commit_id,
                    previous_callback_count,
                );
            }
            if let Some(resize_commit) = old_resize_commit {
                self.release_detached_resize_capture(surface_id, resize_commit);
            }
        }
        canonical
    }

    fn capture_surface_tree_node_lifetimes(
        &self,
        nodes: &[(u32, CachedSubsurfaceCommit)],
    ) -> Option<SurfaceTreeNodeLifetimes> {
        let mut lifetimes = Vec::with_capacity(nodes.len());
        for (surface_id, _) in nodes {
            let (owner_client_id, surface_presentation_generation) =
                self.capture_surface_publication_lifetime(*surface_id)?;
            lifetimes.push(SurfaceTreeNodeLifetime {
                surface_id: *surface_id,
                owner_client_id,
                surface_presentation_generation,
            });
        }
        Some(SurfaceTreeNodeLifetimes::Captured(lifetimes))
    }

    pub(in crate::compositor) fn merge_or_queue_surface_tree_transaction(
        &mut self,
        root_surface_id: u32,
        nodes: Vec<(u32, CachedSubsurfaceCommit)>,
        dependencies: Vec<SurfaceTreeAcquireDependency>,
        external_content_update_dependencies: Vec<ContentUpdateRef>,
        submission_kind: SurfaceTreeSubmissionKind,
    ) {
        let Some(publication_lifetimes) = self.capture_surface_tree_node_lifetimes(&nodes) else {
            self.release_unpublished_surface_tree_nodes(nodes);
            return;
        };
        let incoming_has_unready_acquire = !dependencies.is_empty();
        let incoming_has_attachment_change =
            nodes.iter().any(|(_, commit)| commit.attachment.is_some());
        let incoming_is_pacing_protected = nodes
            .iter()
            .any(|(_, commit)| commit.pacing.is_boundary() || commit.lineage.merge_frozen);
        let matching = self
            .pending_surface_tree_transactions
            .iter()
            .enumerate()
            .filter_map(|(index, transaction)| {
                (transaction.root_surface_id == root_surface_id).then_some(index)
            })
            .collect::<Vec<_>>();
        let Some(&target_index) = matching.last() else {
            let transaction = self.build_surface_tree_transaction(
                root_surface_id,
                nodes,
                publication_lifetimes,
                dependencies,
                external_content_update_dependencies,
            );
            if self.transaction_is_ready(&transaction) {
                self.publish_surface_tree_nodes(transaction);
            } else {
                self.queue_waiting_surface_tree_transaction(transaction, submission_kind);
            }
            return;
        };
        let target_is_ready =
            self.transaction_is_ready(&self.pending_surface_tree_transactions[target_index]);
        let target_is_pacing_protected =
            self.pending_surface_tree_transactions[target_index].is_pacing_protected();
        if target_is_pacing_protected || incoming_is_pacing_protected {
            if target_is_pacing_protected {
                self.queue_waiting_surface_tree_with_lifetimes(
                    root_surface_id,
                    nodes,
                    publication_lifetimes.clone(),
                    dependencies,
                    external_content_update_dependencies.clone(),
                    submission_kind,
                );
                self.commit_ready_surface_tree_transactions();
                return;
            }
            if incoming_has_attachment_change && target_is_ready {
                self.queue_waiting_surface_tree_with_lifetimes(
                    root_surface_id,
                    nodes,
                    publication_lifetimes.clone(),
                    dependencies,
                    external_content_update_dependencies.clone(),
                    submission_kind,
                );
                self.commit_ready_surface_tree_transactions();
                return;
            }
            if incoming_is_pacing_protected {
                self.queue_waiting_surface_tree_with_lifetimes(
                    root_surface_id,
                    nodes,
                    publication_lifetimes.clone(),
                    dependencies,
                    external_content_update_dependencies.clone(),
                    submission_kind,
                );
                self.commit_ready_surface_tree_transactions();
                return;
            }
        }
        if incoming_has_attachment_change && target_is_ready {
            if incoming_has_unready_acquire {
                self.subsurface_transaction_metrics
                    .ready_transactions_preserved_from_newer_unready = self
                    .subsurface_transaction_metrics
                    .ready_transactions_preserved_from_newer_unready
                    .saturating_add(1);
            } else {
                self.subsurface_transaction_metrics
                    .ready_transactions_preserved_from_newer_ready = self
                    .subsurface_transaction_metrics
                    .ready_transactions_preserved_from_newer_ready
                    .saturating_add(1);
            }
            self.queue_waiting_surface_tree_with_lifetimes(
                root_surface_id,
                nodes,
                publication_lifetimes.clone(),
                dependencies,
                external_content_update_dependencies.clone(),
                submission_kind,
            );
            self.commit_ready_surface_tree_transactions();
            return;
        }

        if !can_coalesce_pending_surface_tree_transaction(
            &self.pending_surface_tree_transactions[target_index],
            &nodes,
        ) {
            self.queue_waiting_surface_tree_with_lifetimes(
                root_surface_id,
                nodes,
                publication_lifetimes,
                dependencies,
                external_content_update_dependencies,
                submission_kind,
            );
            self.commit_ready_surface_tree_transactions();
            return;
        }

        let mut transaction = self.pending_surface_tree_transactions.remove(target_index);
        let pacing_deadline_changed = transaction.commit_timing_readiness.is_some();
        let stats = self.merge_surface_tree_nodes_into_transaction(
            root_surface_id,
            &mut transaction,
            nodes,
            publication_lifetimes,
            dependencies,
            external_content_update_dependencies,
        );
        if let Err((error_surface_id, error, viewport_error_owner)) = self
            .prepare_surface_tree_surface_state(
                transaction.root_surface_id,
                &mut transaction.nodes,
                &transaction.external_content_update_dependencies,
            )
        {
            if compositor_debug_surface_logging_enabled() {
                eprintln!(
                    "oblivion-one compositor: merged surface-tree validation failed surface={error_surface_id}"
                );
            }
            self.post_surface_mapping_error(error_surface_id, error, viewport_error_owner);
            let root_surface_id = transaction.root_surface_id;
            let released = self.release_pending_surface_tree_transaction(
                transaction,
                AcquireWatchCancelReason::Rejected,
            );
            if let Some(resize_commit) = released.resize_commit {
                self.release_detached_resize_capture(root_surface_id, resize_commit);
            }
            self.complete_frame_callbacks(released.callbacks);
            self.rebuild_scene_work_index();
            self.update_surface_tree_slot_metrics(root_surface_id);
            return;
        }
        let ready_after_merge = self.transaction_is_ready(&transaction);
        self.record_surface_tree_merge_metrics(&stats);
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: subsurface_tx root={root_surface_id} decision={} incoming_nodes={} existing_nodes={} bufferless_nodes={} attachments_replaced={} dependencies_preserved={} dependencies_replaced={} callbacks_merged={} feedbacks_merged={} resize_snapshot={} ready_after_merge={ready_after_merge}",
                if ready_after_merge {
                    "merged_ready"
                } else {
                    "merged_waiting"
                },
                stats.incoming_nodes,
                stats.existing_nodes,
                stats.bufferless_nodes,
                stats.attachments_replaced,
                stats.dependencies_preserved,
                stats.dependencies_replaced,
                stats.callbacks_merged,
                stats.feedbacks_merged,
                if stats.resize_snapshots_replaced > 0 {
                    "replaced"
                } else if stats.resize_snapshots_preserved > 0 {
                    "preserved"
                } else {
                    "none"
                },
            );
        }
        self.pending_surface_tree_transactions.push(transaction);
        if pacing_deadline_changed {
            self.invalidate_surface_pacing_deadline_cache();
        }
        self.rebuild_scene_work_index();
        self.update_surface_tree_slot_metrics(root_surface_id);
        if ready_after_merge || !incoming_has_attachment_change {
            self.commit_ready_surface_tree_transactions();
        }
    }

    pub(in crate::compositor) fn merge_surface_tree_nodes_into_transaction(
        &mut self,
        root_surface_id: u32,
        transaction: &mut PendingSurfaceTreeTransaction,
        nodes: Vec<(u32, CachedSubsurfaceCommit)>,
        publication_lifetimes: SurfaceTreeNodeLifetimes,
        dependencies: Vec<SurfaceTreeAcquireDependency>,
        external_content_update_dependencies: Vec<ContentUpdateRef>,
    ) -> SurfaceTreeMergeStats {
        let Some(publication_lifetimes) = publication_lifetimes.captured() else {
            return SurfaceTreeMergeStats::default();
        };
        debug_assert_eq!(transaction.ordering(), TransactionOrdering::Coalescible);
        debug_assert!(
            nodes.iter().all(|(_, commit)| {
                !commit.pacing.is_boundary() && !commit.lineage.merge_frozen
            })
        );
        if publication_lifetimes.len() != nodes.len() {
            return SurfaceTreeMergeStats::default();
        }
        let mut stats = SurfaceTreeMergeStats {
            incoming_nodes: nodes.len(),
            existing_nodes: transaction.nodes.len(),
            ..SurfaceTreeMergeStats::default()
        };
        let incoming_lifetimes = publication_lifetimes.to_vec();
        let incoming_surface_ids = nodes
            .iter()
            .map(|(surface_id, _)| *surface_id)
            .collect::<Vec<_>>();
        debug_assert!(
            incoming_surface_ids
                .iter()
                .enumerate()
                .all(|(index, surface_id)| !incoming_surface_ids[..index].contains(surface_id))
        );
        let Some(existing_lifetimes) = transaction
            .publication_lifetimes
            .captured()
            .filter(|lifetimes| lifetimes.len() == transaction.nodes.len())
            .map(<[SurfaceTreeNodeLifetime]>::to_vec)
        else {
            return SurfaceTreeMergeStats::default();
        };
        let mut existing_nodes = std::mem::take(&mut transaction.nodes)
            .into_iter()
            .zip(existing_lifetimes)
            .map(|((surface_id, commit), lifetime)| Some((surface_id, commit, lifetime)))
            .collect::<Vec<_>>();
        let mut merged_nodes = Vec::with_capacity(stats.existing_nodes.saturating_add(nodes.len()));
        let mut merged_lifetimes = Vec::with_capacity(merged_nodes.capacity());
        for existing in &mut existing_nodes {
            let Some((surface_id, commit, lifetime)) = existing.take() else {
                continue;
            };
            if incoming_surface_ids.contains(&surface_id) {
                *existing = Some((surface_id, commit, lifetime));
            } else {
                merged_nodes.push((surface_id, commit));
                merged_lifetimes.push(lifetime);
            }
        }
        for ((surface_id, incoming), incoming_lifetime) in nodes.into_iter().zip(incoming_lifetimes)
        {
            let attachment_changed = incoming.attachment.is_some();
            let callbacks = incoming.frame_callbacks.len();
            let feedbacks = incoming.presentation_feedbacks.len();
            if !attachment_changed {
                stats.bufferless_nodes = stats.bufferless_nodes.saturating_add(1);
            }
            if matches!(
                incoming.attachment,
                Some(PendingSurfaceAttachment::RemoveContent)
            ) {
                stats.explicit_detaches = stats.explicit_detaches.saturating_add(1);
            }
            let resize_replaced =
                incoming.resize_capture_finalized && incoming.resize_commit.is_some();
            let Some(existing_index) = existing_nodes.iter().position(|entry| {
                entry
                    .as_ref()
                    .is_some_and(|(node_surface_id, _, _)| *node_surface_id == surface_id)
            }) else {
                merged_nodes.push((surface_id, incoming));
                merged_lifetimes.push(incoming_lifetime);
                continue;
            };
            let (existing_surface_id, mut existing, _existing_lifetime) = existing_nodes
                [existing_index]
                .take()
                .expect("existing surface-tree node");
            debug_assert_eq!(existing_surface_id, surface_id);
            debug_assert!(existing.commit_sequence < incoming.commit_sequence);
            let old_buffer_id = existing
                .attachment
                .as_ref()
                .and_then(pending_attachment_buffer_protocol_id);
            let old_resize_commit = attachment_changed
                .then(|| pending_node_resize_commit(&existing))
                .flatten();
            let replaced_dependency = attachment_changed
                .then(|| {
                    old_buffer_id.and_then(|buffer_id| {
                        remove_surface_tree_dependency(transaction, surface_id, buffer_id)
                    })
                })
                .flatten();
            if let Some(dependency) = replaced_dependency {
                stats.dependencies_replaced = stats.dependencies_replaced.saturating_add(1);
                if self.external_acquire_readiness {
                    self.pending_acquire_watch_changes
                        .push(AcquireWatchChange::Cancel {
                            commit_id: dependency.commit_id,
                            reason: AcquireWatchCancelReason::Superseded,
                        });
                }
                if compositor_debug_surface_logging_enabled() {
                    eprintln!(
                        "oblivion-one compositor: subsurface_tx root={root_surface_id} decision=attachment_superseded surface={surface_id} old_buffer_id={} old_commit_id={}",
                        dependency.buffer_id,
                        dependency.commit_id.get(),
                    );
                }
            } else if !attachment_changed {
                stats.dependencies_preserved = stats.dependencies_preserved.saturating_add(
                    transaction
                        .dependencies
                        .iter()
                        .filter(|dependency| dependency.surface_id == surface_id)
                        .count(),
                );
            }
            let previous_commit_id = existing.commit_id;
            let previous_callback_count = existing.frame_callbacks.len();
            let replacement_commit_id = incoming.commit_id;
            if let Some(release) = existing.merge(incoming) {
                stats.attachments_replaced = stats.attachments_replaced.saturating_add(1);
                self.release_pending_surface_buffer(release);
            }
            if previous_commit_id != replacement_commit_id {
                self.note_explicit_commit_merged(
                    previous_commit_id,
                    replacement_commit_id,
                    previous_callback_count,
                );
            }
            if let Some(resize_commit) = old_resize_commit {
                self.release_detached_resize_capture(surface_id, resize_commit);
            }
            if !attachment_changed && existing.resize_commit.is_some() {
                stats.resize_snapshots_preserved =
                    stats.resize_snapshots_preserved.saturating_add(1);
            }
            if resize_replaced {
                stats.resize_snapshots_replaced = stats.resize_snapshots_replaced.saturating_add(1);
            }
            stats.callbacks_merged = stats.callbacks_merged.saturating_add(callbacks);
            stats.feedbacks_merged = stats.feedbacks_merged.saturating_add(feedbacks);
            merged_nodes.push((surface_id, existing));
            merged_lifetimes.push(incoming_lifetime);
        }
        debug_assert!(existing_nodes.iter().all(Option::is_none));
        transaction.nodes = merged_nodes;
        transaction.publication_lifetimes = SurfaceTreeNodeLifetimes::Captured(merged_lifetimes);
        if self.external_acquire_readiness {
            for dependency in &dependencies {
                self.pending_acquire_watch_changes
                    .push(AcquireWatchChange::Register(AcquireWatchRequest {
                        commit_id: dependency.commit_id,
                        surface_id: dependency.surface_id,
                        buffer_id: dependency.buffer_id,
                        acquire: dependency.acquire.clone(),
                        received_at: Instant::now(),
                    }));
            }
        }
        transaction.dependencies.extend(dependencies);
        for dependency in external_content_update_dependencies {
            if !transaction
                .external_content_update_dependencies
                .contains(&dependency)
            {
                transaction
                    .external_content_update_dependencies
                    .push(dependency);
            }
        }
        normalize_transaction_external_content_dependencies(transaction);
        normalize_surface_tree_node_order(transaction);
        debug_assert_surface_tree_content_update_invariants(transaction);
        stats
    }

    pub(in crate::compositor) fn update_surface_tree_slot_metrics(&mut self, root_surface_id: u32) {
        let mut ready = 0usize;
        let mut waiting = 0usize;
        for transaction in self
            .pending_surface_tree_transactions
            .iter()
            .filter(|transaction| transaction.root_surface_id == root_surface_id)
        {
            if self.transaction_is_ready(transaction) {
                ready = ready.saturating_add(1);
            } else {
                waiting = waiting.saturating_add(1);
            }
        }
        self.subsurface_transaction_metrics
            .maximum_ready_slots_per_root = self
            .subsurface_transaction_metrics
            .maximum_ready_slots_per_root
            .max(ready);
        self.subsurface_transaction_metrics
            .maximum_waiting_slots_per_root = self
            .subsurface_transaction_metrics
            .maximum_waiting_slots_per_root
            .max(waiting);
        self.subsurface_transaction_metrics
            .maximum_explicit_sync_queue_depth = self
            .subsurface_transaction_metrics
            .maximum_explicit_sync_queue_depth
            .max(ready.saturating_add(waiting));
    }

    fn validate_surface_tree_surface_state(
        &self,
        root_surface_id: u32,
        nodes: &[(u32, CachedSubsurfaceCommit)],
        external_content_update_dependencies: &[ContentUpdateRef],
    ) -> Result<(), (u32, SurfaceMappingError, Option<wp_viewport::WpViewport>)> {
        self.derive_surface_tree_surface_state(
            root_surface_id,
            nodes,
            external_content_update_dependencies,
        )
        .map(|_| ())
    }

    pub(in crate::compositor) fn prepare_surface_tree_surface_state(
        &self,
        root_surface_id: u32,
        nodes: &mut [(u32, CachedSubsurfaceCommit)],
        external_content_update_dependencies: &[ContentUpdateRef],
    ) -> Result<(), (u32, SurfaceMappingError, Option<wp_viewport::WpViewport>)> {
        let mappings = self.derive_surface_tree_surface_state(
            root_surface_id,
            nodes,
            external_content_update_dependencies,
        )?;
        for ((_, commit), mapping) in nodes.iter_mut().zip(mappings) {
            if let Some(mapping) = mapping {
                let Some(PendingSurfaceAttachment::Buffer(pending)) = commit.attachment.as_mut()
                else {
                    debug_assert!(false, "mapping derived without a pending buffer");
                    continue;
                };
                pending.apply_content_mapping(mapping);
            }
        }
        Ok(())
    }

    fn derive_surface_tree_surface_state(
        &self,
        root_surface_id: u32,
        nodes: &[(u32, CachedSubsurfaceCommit)],
        external_content_update_dependencies: &[ContentUpdateRef],
    ) -> Result<
        Vec<Option<SurfaceContentMapping>>,
        (u32, SurfaceMappingError, Option<wp_viewport::WpViewport>),
    > {
        let mut effective_states = HashMap::new();
        let mut references = external_content_update_dependencies.to_vec();
        for (surface_id, commit) in nodes {
            if !effective_states.contains_key(surface_id) {
                let initial = match self.surface_resource_by_id(*surface_id) {
                    Some(surface) => match surface.data::<SurfaceData>() {
                        Some(data) => EffectiveSurfaceMappingState::from_surface(
                            data,
                            self.current_surface_buffers.get(surface_id),
                        ),
                        None => Err(SurfaceMappingError::InvalidBufferSize),
                    },
                    None => Err(SurfaceMappingError::InvalidBufferSize),
                };
                let Ok(initial) = initial else {
                    return Err((
                        *surface_id,
                        SurfaceMappingError::InvalidBufferSize,
                        commit.viewport_error_owner.clone(),
                    ));
                };
                effective_states.insert(*surface_id, initial);
            }
            if let Some(predecessor) = commit.lineage.predecessor {
                references.push(predecessor);
            }
            references.extend(commit.lineage.child_dependencies.iter().copied());
        }
        for transaction in self.pending_surface_tree_mapping_prefix(root_surface_id, &references) {
            for (surface_id, commit) in &transaction.nodes {
                let Some(effective_state) = effective_states.get_mut(surface_id) else {
                    continue;
                };
                if let Err((error, viewport_error_owner)) = effective_state.apply_commit(commit) {
                    debug_assert!(
                        false,
                        "admitted pending surface-tree mapping became invalid while projecting: {error:?}"
                    );
                    return Err((*surface_id, error, viewport_error_owner));
                }
            }
        }
        let mut mappings = Vec::with_capacity(nodes.len());
        for (surface_id, commit) in nodes {
            let mapping = effective_states
                .get_mut(surface_id)
                .expect("effective state inserted above")
                .apply_commit(commit);
            match mapping {
                Ok(mapping) => mappings.push(mapping),
                Err((error, viewport_error_owner)) => {
                    return Err((*surface_id, error, viewport_error_owner));
                }
            }
        }
        Ok(mappings)
    }

    fn pending_surface_tree_mapping_prefix<'a>(
        &'a self,
        root_surface_id: u32,
        candidate_references: &[ContentUpdateRef],
    ) -> Vec<&'a PendingSurfaceTreeTransaction> {
        let transactions = &self.pending_surface_tree_transactions;
        let mut selected = vec![false; transactions.len()];
        for (index, transaction) in transactions.iter().enumerate() {
            if transaction.root_surface_id == root_surface_id {
                selected[index] = true;
            }
        }

        let mut references = Vec::new();
        for reference in candidate_references {
            add_unique_content_update_ref(&mut references, *reference);
        }

        let mut next_reference = 0;
        loop {
            while let Some(reference) = references.get(next_reference).copied() {
                next_reference += 1;
                if let Some(index) = transactions.iter().position(|transaction| {
                    transaction_covers_content_update_ref(transaction, reference)
                }) {
                    selected[index] = true;
                }
            }

            let mut changed = false;
            for index in 0..transactions.len() {
                if !selected[index] {
                    continue;
                }
                let transaction = &transactions[index];
                for prior_index in 0..index {
                    if transactions[prior_index].root_surface_id == transaction.root_surface_id
                        && !selected[prior_index]
                    {
                        selected[prior_index] = true;
                        changed = true;
                    }
                }
                for dependency in &transaction.external_content_update_dependencies {
                    changed |= add_unique_content_update_ref(&mut references, *dependency);
                }
                for (_, commit) in &transaction.nodes {
                    if let Some(predecessor) = commit.lineage.predecessor {
                        changed |= add_unique_content_update_ref(&mut references, predecessor);
                    }
                    for dependency in &commit.lineage.child_dependencies {
                        changed |= add_unique_content_update_ref(&mut references, *dependency);
                    }
                }
            }
            if !changed && next_reference >= references.len() {
                break;
            }
        }

        let selected_indices = selected
            .iter()
            .enumerate()
            .filter_map(|(index, selected)| selected.then_some(index))
            .collect::<Vec<_>>();
        if selected_indices.len() < 2 {
            return selected_indices
                .into_iter()
                .map(|index| &transactions[index])
                .collect();
        }

        let mut indegree = vec![0usize; transactions.len()];
        let mut successors = vec![Vec::new(); transactions.len()];
        let mut add_edge = |predecessor: usize, dependent: usize| {
            if predecessor == dependent || successors[predecessor].contains(&dependent) {
                return;
            }
            successors[predecessor].push(dependent);
            indegree[dependent] = indegree[dependent].saturating_add(1);
        };
        for dependent in selected_indices.iter().copied() {
            for predecessor in selected_indices.iter().copied() {
                if predecessor < dependent
                    && transactions[predecessor].root_surface_id
                        == transactions[dependent].root_surface_id
                {
                    add_edge(predecessor, dependent);
                }
            }
            let transaction = &transactions[dependent];
            let mut transaction_references = Vec::new();
            transaction_references.extend(
                transaction
                    .external_content_update_dependencies
                    .iter()
                    .copied(),
            );
            for (_, commit) in &transaction.nodes {
                if let Some(predecessor) = commit.lineage.predecessor {
                    transaction_references.push(predecessor);
                }
                transaction_references.extend(commit.lineage.child_dependencies.iter().copied());
            }
            for reference in transaction_references {
                if let Some(predecessor) = selected_indices.iter().copied().find(|index| {
                    transaction_covers_content_update_ref(&transactions[*index], reference)
                }) {
                    add_edge(predecessor, dependent);
                }
            }
        }

        let mut emitted = vec![false; transactions.len()];
        let mut order = Vec::with_capacity(selected_indices.len());
        for _ in 0..selected_indices.len() {
            let Some(next) = selected_indices
                .iter()
                .copied()
                .find(|index| !emitted[*index] && indegree[*index] == 0)
            else {
                debug_assert!(false, "surface-tree pending transaction ordering cycle");
                return selected_indices
                    .into_iter()
                    .map(|index| &transactions[index])
                    .collect();
            };
            emitted[next] = true;
            order.push(next);
            for successor in &successors[next] {
                indegree[*successor] = indegree[*successor].saturating_sub(1);
            }
        }
        order
            .into_iter()
            .map(|index| &transactions[index])
            .collect()
    }

    pub(in crate::compositor) fn post_surface_mapping_error(
        &mut self,
        surface_id: u32,
        error: SurfaceMappingError,
        viewport_error_owner: Option<wp_viewport::WpViewport>,
    ) {
        let Some(surface) = self.surface_resource_by_id(surface_id) else {
            return;
        };
        let Some(client) = surface.client() else {
            return;
        };
        // A cached viewport error belongs to the resource that authored the
        // cached source state. If that object has since been destroyed, its
        // error cannot be reassigned to a newer viewport or to wl_surface.
        let viewport = viewport_error_owner.filter(Resource::is_alive);
        match (viewport, error) {
            (Some(viewport), SurfaceMappingError::ViewportSourceNonIntegralWithoutDestination) => {
                self.post_protocol_error(
                    &client,
                    &viewport,
                    wp_viewport::Error::BadSize,
                    "viewport source width and height must be integral when destination is unset",
                )
            }
            (Some(viewport), SurfaceMappingError::ViewportSourceOutOfBounds) => {
                self.post_protocol_error(
                    &client,
                    &viewport,
                    wp_viewport::Error::OutOfBuffer,
                    "viewport source rectangle is outside the buffer",
                );
            }
            (None, SurfaceMappingError::ViewportSourceNonIntegralWithoutDestination)
            | (None, SurfaceMappingError::ViewportSourceOutOfBounds) => {}
            (_, SurfaceMappingError::InvalidBufferSize)
            | (_, SurfaceMappingError::BufferScaleNotIntegral) => self.post_protocol_error(
                &client,
                &surface,
                wl_surface::Error::InvalidSize,
                "surface buffer mapping is invalid",
            ),
        }
    }

    pub(in crate::compositor) fn prepare_surface_tree_acquires(
        &mut self,
        nodes: &mut [(u32, CachedSubsurfaceCommit)],
    ) -> Option<Vec<SurfaceTreeAcquireDependency>> {
        let mut dependencies = Vec::new();
        for (surface_id, commit) in nodes {
            let Some(explicit_sync) = commit.explicit_sync.take() else {
                continue;
            };
            let CapturedExplicitSyncState {
                state,
                acquire,
                release,
            } = explicit_sync;
            let Some(PendingSurfaceAttachment::Buffer(pending)) = commit.attachment.as_mut() else {
                if acquire.is_some() || release.is_some() {
                    state.post_error_with_metrics(
                        &mut self.compliance_metrics,
                        &mut self.protocol_error_trace,
                        &mut self.terminal_client_ids,
                        SYNCOBJ_SURFACE_ERROR_NO_BUFFER,
                        "explicit sync points were set without an attached buffer",
                    );
                    return None;
                }
                continue;
            };
            if !pending.data.is_dmabuf() {
                state.post_error_with_metrics(
                    &mut self.compliance_metrics,
                    &mut self.protocol_error_trace,
                    &mut self.terminal_client_ids,
                    SYNCOBJ_SURFACE_ERROR_UNSUPPORTED_BUFFER,
                    "explicit sync is only supported for linux-dmabuf buffers",
                );
                return None;
            }
            let Some(acquire) = acquire else {
                state.post_error_with_metrics(
                    &mut self.compliance_metrics,
                    &mut self.protocol_error_trace,
                    &mut self.terminal_client_ids,
                    SYNCOBJ_SURFACE_ERROR_NO_ACQUIRE_POINT,
                    "dmabuf commit is missing an acquire timeline point",
                );
                return None;
            };
            let Some(release) = release else {
                state.post_error_with_metrics(
                    &mut self.compliance_metrics,
                    &mut self.protocol_error_trace,
                    &mut self.terminal_client_ids,
                    SYNCOBJ_SURFACE_ERROR_NO_RELEASE_POINT,
                    "dmabuf commit is missing a release timeline point",
                );
                return None;
            };
            if acquire.timeline.same_timeline(&release.timeline) && acquire.point >= release.point {
                state.post_error_with_metrics(
                    &mut self.compliance_metrics,
                    &mut self.protocol_error_trace,
                    &mut self.terminal_client_ids,
                    SYNCOBJ_SURFACE_ERROR_CONFLICTING_POINTS,
                    "acquire timeline point must be lower than release point on the same timeline",
                );
                return None;
            }
            pending.explicit_release = Some(release);
            if acquire.is_signaled() {
                self.note_explicit_commit_ready(commit.commit_id);
                continue;
            }
            let Some(commit_id) = self.acquire_commit_ids.allocate() else {
                state.post_error_with_metrics(
                    &mut self.compliance_metrics,
                    &mut self.protocol_error_trace,
                    &mut self.terminal_client_ids,
                    SYNCOBJ_SURFACE_ERROR_NO_ACQUIRE_POINT,
                    "explicit sync commit identity space exhausted",
                );
                return None;
            };
            let Some((owner_client_id, surface_presentation_generation)) =
                self.capture_surface_publication_lifetime(*surface_id)
            else {
                return None;
            };
            client_pacing_log(
                "acquire_wait_queued",
                &[
                    ("surface", surface_id.to_string()),
                    (
                        "root",
                        self.root_surface_id_for_surface(*surface_id).to_string(),
                    ),
                    (
                        "client",
                        format!("{:?}", self.surface_client_ids.get(surface_id)),
                    ),
                    ("commit_sequence", commit.commit_sequence.0.to_string()),
                    ("acquire_commit_id", commit_id.get().to_string()),
                    ("buffer", pending.resource.id().protocol_id().to_string()),
                ],
            );
            dependencies.push(SurfaceTreeAcquireDependency {
                surface_commit_id: commit.commit_id,
                commit_id,
                surface_id: *surface_id,
                owner_client_id: Some(owner_client_id),
                surface_presentation_generation: Some(surface_presentation_generation),
                buffer_id: pending.resource.id().protocol_id(),
                acquire,
                state: PendingAcquireState::RegistrationPending,
            });
            self.trace_surface_pipeline_event(
                SurfacePipelineEvent::AcquirePending,
                *surface_id,
                commit.commit_sequence,
                Some(pending.resource.id().protocol_id().into()),
                None,
                None,
                None,
                None,
                None,
            );
            self.note_explicit_commit_acquire_wait(commit.commit_id, commit.frame_callbacks.len());
        }
        Some(dependencies)
    }

    fn build_surface_tree_transaction(
        &mut self,
        root_surface_id: u32,
        nodes: Vec<(u32, CachedSubsurfaceCommit)>,
        publication_lifetimes: SurfaceTreeNodeLifetimes,
        dependencies: Vec<SurfaceTreeAcquireDependency>,
        external_content_update_dependencies: Vec<ContentUpdateRef>,
    ) -> PendingSurfaceTreeTransaction {
        let mut transaction = PendingSurfaceTreeTransaction {
            id: self.allocate_surface_tree_transaction_id(),
            root_surface_id,
            nodes,
            publication_lifetimes,
            dependencies,
            external_content_update_dependencies,
            commit_timing_readiness: None,
            received_at: Instant::now(),
        };
        normalize_transaction_external_content_dependencies(&mut transaction);
        normalize_surface_tree_node_order(&mut transaction);
        debug_assert_surface_tree_content_update_invariants(&transaction);
        transaction
    }

    fn queue_waiting_surface_tree_transaction(
        &mut self,
        transaction: PendingSurfaceTreeTransaction,
        submission_kind: SurfaceTreeSubmissionKind,
    ) {
        let PendingSurfaceTreeTransaction {
            id: transaction_id,
            root_surface_id,
            nodes,
            publication_lifetimes,
            dependencies,
            external_content_update_dependencies,
            commit_timing_readiness,
            received_at,
        } = transaction;
        self.queue_waiting_surface_tree_parts(
            root_surface_id,
            transaction_id,
            nodes,
            publication_lifetimes,
            dependencies,
            external_content_update_dependencies,
            commit_timing_readiness,
            received_at,
            submission_kind,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn queue_waiting_surface_tree_parts(
        &mut self,
        root_surface_id: u32,
        transaction_id: SurfaceTreeTransactionId,
        nodes: Vec<(u32, CachedSubsurfaceCommit)>,
        publication_lifetimes: SurfaceTreeNodeLifetimes,
        dependencies: Vec<SurfaceTreeAcquireDependency>,
        external_content_update_dependencies: Vec<ContentUpdateRef>,
        commit_timing_readiness: Option<CommitTimingReadiness>,
        received_at: Instant,
        submission_kind: SurfaceTreeSubmissionKind,
    ) {
        let mut transaction = PendingSurfaceTreeTransaction {
            id: transaction_id,
            root_surface_id,
            nodes: Vec::new(),
            publication_lifetimes,
            dependencies: Vec::new(),
            external_content_update_dependencies: Vec::new(),
            commit_timing_readiness,
            received_at,
        };
        if submission_kind == SurfaceTreeSubmissionKind::ClientAdmission {
            let mut matching = self
                .pending_surface_tree_transactions
                .iter()
                .enumerate()
                .filter_map(|(index, transaction)| {
                    (transaction.root_surface_id == root_surface_id).then_some(index)
                })
                .collect::<Vec<_>>();
            let at_capacity_with_only_ready = matching.len()
                >= Self::MAX_SURFACE_TREE_TRANSACTIONS_PER_ROOT
                && matching.iter().all(|index| {
                    self.transaction_is_ready(&self.pending_surface_tree_transactions[*index])
                });
            if at_capacity_with_only_ready {
                self.subsurface_transaction_metrics.all_ready_queue_pressure = self
                    .subsurface_transaction_metrics
                    .all_ready_queue_pressure
                    .saturating_add(1);
                self.commit_ready_surface_tree_transactions();
                matching.clear();
                matching.extend(
                    self.pending_surface_tree_transactions
                        .iter()
                        .enumerate()
                        .filter_map(|(index, transaction)| {
                            (transaction.root_surface_id == root_surface_id).then_some(index)
                        }),
                );
            }
            if matching.len() >= Self::MAX_SURFACE_TREE_TRANSACTIONS_PER_ROOT {
                self.subsurface_transaction_metrics
                    .explicit_sync_queue_overflow = self
                    .subsurface_transaction_metrics
                    .explicit_sync_queue_overflow
                    .saturating_add(1);
                self.commit_ready_surface_tree_transactions();
                let still_at_capacity = self
                    .pending_surface_tree_transactions
                    .iter()
                    .filter(|transaction| transaction.root_surface_id == root_surface_id)
                    .count()
                    >= Self::MAX_SURFACE_TREE_TRANSACTIONS_PER_ROOT;
                if still_at_capacity {
                    if self.request_client_resource_exhaustion(root_surface_id) {
                        self.surface_pacing_metrics
                            .queue_admission_resource_exhaustion = self
                            .surface_pacing_metrics
                            .queue_admission_resource_exhaustion
                            .saturating_add(1);
                    }
                    self.release_unpublished_surface_tree_nodes(nodes);
                    return;
                }
            }
        }
        if self.external_acquire_readiness {
            for dependency in &dependencies {
                self.pending_acquire_watch_changes
                    .push(AcquireWatchChange::Register(AcquireWatchRequest {
                        commit_id: dependency.commit_id,
                        surface_id: dependency.surface_id,
                        buffer_id: dependency.buffer_id,
                        acquire: dependency.acquire.clone(),
                        received_at: Instant::now(),
                    }));
            }
        }
        self.subsurface_transaction_metrics
            .tree_transactions_waiting_on_acquire = self
            .subsurface_transaction_metrics
            .tree_transactions_waiting_on_acquire
            .saturating_add(1);
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: subsurface_tx root={root_surface_id} decision=waiting_acquire cached_nodes={} waiting_acquires={} callbacks={} preview_active={}",
                nodes.len(),
                dependencies.len(),
                nodes
                    .iter()
                    .map(|(_, commit)| commit.frame_callbacks.len())
                    .sum::<usize>(),
                self.active_toplevel_resizes.contains_key(&root_surface_id),
            );
        }
        for (surface_id, commit) in &nodes {
            self.trace_surface_pipeline_event(
                SurfacePipelineEvent::TransactionQueued,
                *surface_id,
                commit.commit_sequence,
                commit
                    .attachment
                    .as_ref()
                    .and_then(|attachment| match attachment {
                        PendingSurfaceAttachment::Buffer(buffer) => {
                            Some(buffer.data.buffer_id().get())
                        }
                        PendingSurfaceAttachment::RemoveContent => None,
                    }),
                None,
                Some(transaction_id.get()),
                None,
                None,
                None,
            );
        }
        transaction.nodes = nodes;
        transaction.dependencies = dependencies;
        transaction.external_content_update_dependencies = external_content_update_dependencies;
        normalize_transaction_external_content_dependencies(&mut transaction);
        normalize_surface_tree_node_order(&mut transaction);
        debug_assert_surface_tree_content_update_invariants(&transaction);
        self.pending_surface_tree_transactions.push(transaction);
        self.rebuild_scene_work_index();
        if submission_kind == SurfaceTreeSubmissionKind::ClientAdmission {
            debug_assert!(
                self.pending_surface_tree_transactions
                    .iter()
                    .filter(|transaction| transaction.root_surface_id == root_surface_id)
                    .count()
                    <= Self::MAX_SURFACE_TREE_TRANSACTIONS_PER_ROOT
            );
        }
        self.update_surface_tree_slot_metrics(root_surface_id);
        let pending_acquires = self.pending_explicit_sync_commits.len().saturating_add(
            self.pending_surface_tree_transactions
                .iter()
                .map(|transaction| transaction.dependencies.len())
                .sum::<usize>(),
        );
        self.resize_flow_metrics.max_pending_explicit_sync_commits = self
            .resize_flow_metrics
            .max_pending_explicit_sync_commits
            .max(pending_acquires);
    }

    #[cfg(test)]
    pub(in crate::compositor) fn queue_waiting_surface_tree(
        &mut self,
        root_surface_id: u32,
        nodes: Vec<(u32, CachedSubsurfaceCommit)>,
        dependencies: Vec<SurfaceTreeAcquireDependency>,
    ) {
        let Some(publication_lifetimes) = self.capture_surface_tree_node_lifetimes(&nodes) else {
            self.release_unpublished_surface_tree_nodes(nodes);
            return;
        };
        self.queue_waiting_surface_tree_with_lifetimes(
            root_surface_id,
            nodes,
            publication_lifetimes,
            dependencies,
            Vec::new(),
            SurfaceTreeSubmissionKind::ClientAdmission,
        );
    }

    fn queue_waiting_surface_tree_with_lifetimes(
        &mut self,
        root_surface_id: u32,
        nodes: Vec<(u32, CachedSubsurfaceCommit)>,
        publication_lifetimes: SurfaceTreeNodeLifetimes,
        dependencies: Vec<SurfaceTreeAcquireDependency>,
        external_content_update_dependencies: Vec<ContentUpdateRef>,
        submission_kind: SurfaceTreeSubmissionKind,
    ) {
        let transaction = self.build_surface_tree_transaction(
            root_surface_id,
            nodes,
            publication_lifetimes,
            dependencies,
            external_content_update_dependencies,
        );
        self.queue_waiting_surface_tree_transaction(transaction, submission_kind);
    }

    pub(in crate::compositor) fn publish_surface_tree_nodes(
        &mut self,
        transaction: PendingSurfaceTreeTransaction,
    ) {
        if let Some((surface_id, decision)) =
            self.surface_tree_async_publication_rejection(&transaction)
        {
            let (commit_sequence, buffer_id) = transaction
                .nodes
                .iter()
                .filter(|(node_surface_id, _)| *node_surface_id == surface_id)
                .max_by_key(|(_, commit)| commit.commit_sequence)
                .map_or((SurfaceCommitSequence::initial(), None), |(_, commit)| {
                    (
                        commit.commit_sequence,
                        commit
                            .attachment
                            .as_ref()
                            .and_then(|attachment| match attachment {
                                PendingSurfaceAttachment::Buffer(buffer) => {
                                    Some(buffer.data.buffer_id())
                                }
                                PendingSurfaceAttachment::RemoveContent => None,
                            }),
                    )
                });
            if matches!(
                decision,
                SurfacePublicationDecision::SurfaceGone
                    | SurfacePublicationDecision::OwnerGone
                    | SurfacePublicationDecision::TerminalClient
                    | SurfacePublicationDecision::StaleSurfaceGeneration
            ) {
                let has_node = transaction
                    .nodes
                    .iter()
                    .any(|(node_surface_id, _)| *node_surface_id == surface_id);
                if has_node {
                    self.trace_surface_pipeline_event_with_reason(
                        SurfacePipelineEvent::AcquireReadyDiscarded,
                        surface_id,
                        commit_sequence,
                        buffer_id.map(BufferId::get),
                        None,
                        Some(transaction.id.get()),
                        None,
                        None,
                        None,
                        decision.pipeline_rejection_reason(),
                    );
                }
            }
            self.record_surface_publication_rejection(
                surface_id,
                commit_sequence,
                buffer_id,
                SurfacePublicationSource::SurfaceTree,
                decision,
            );
            self.discard_surface_tree_transaction_with_decision(transaction, decision);
            return;
        }
        let PendingSurfaceTreeTransaction {
            root_surface_id,
            nodes,
            ..
        } = transaction;
        let stale_node = nodes.iter().find_map(|(surface_id, commit)| {
            if commit.attachment.is_none() {
                return None;
            }
            let decision = self.surface_publication_decision(
                *surface_id,
                commit.commit_sequence,
                SurfacePublicationContext::OrderedExplicitSyncQueue,
            );
            (decision != SurfacePublicationDecision::Publish).then_some((
                *surface_id,
                commit.commit_sequence,
                commit
                    .attachment
                    .as_ref()
                    .and_then(|attachment| match attachment {
                        PendingSurfaceAttachment::Buffer(buffer) => Some(buffer.data.buffer_id()),
                        PendingSurfaceAttachment::RemoveContent => None,
                    }),
                decision,
            ))
        });
        if let Some((surface_id, commit_sequence, buffer_id, decision)) = stale_node {
            self.record_surface_publication_rejection(
                surface_id,
                commit_sequence,
                buffer_id,
                SurfacePublicationSource::SurfaceTree,
                decision,
            );
            self.release_unpublished_surface_tree_nodes(nodes);
            return;
        }
        if !nodes
            .iter()
            .any(|(surface_id, _)| *surface_id == root_surface_id)
        {
            self.release_unpublished_surface_tree_nodes(nodes);
            return;
        }
        self.publish_surface_tree(root_surface_id, nodes);
    }

    pub(in crate::compositor) fn discard_surface_tree_transaction_with_decision(
        &mut self,
        transaction: PendingSurfaceTreeTransaction,
        decision: SurfacePublicationDecision,
    ) {
        let root_surface_id = transaction.root_surface_id;
        let released = self.release_pending_surface_tree_transaction(
            transaction,
            AcquireWatchCancelReason::SurfaceDestroyed,
        );
        if let Some(resize_commit) = released.resize_commit {
            self.release_detached_resize_capture(root_surface_id, resize_commit);
        }
        if decision == SurfacePublicationDecision::TerminalClient {
            self.discard_frame_callbacks(released.callbacks);
        } else {
            self.complete_frame_callbacks(released.callbacks);
        }
    }

    pub(in crate::compositor) fn cancel_pending_surface_trees_for_root(
        &mut self,
        root_surface_id: u32,
        reason: AcquireWatchCancelReason,
    ) -> ReleasedSurfaceTreeState {
        let mut retained = Vec::new();
        let mut canceled_refs = Vec::new();
        let mut pacing_deadline_changed = false;
        let mut released = ReleasedSurfaceTreeState {
            callbacks: Vec::new(),
            resize_commit: None,
        };
        for transaction in std::mem::take(&mut self.pending_surface_tree_transactions) {
            if transaction.root_surface_id == root_surface_id {
                canceled_refs.extend(
                    transaction
                        .nodes
                        .iter()
                        .map(|(surface_id, commit)| commit.content_update_ref(*surface_id)),
                );
                pacing_deadline_changed |= transaction.commit_timing_readiness.is_some();
                let transaction =
                    self.release_pending_surface_tree_transaction(transaction, reason);
                self.subsurface_transaction_metrics.root_wide_supersessions = self
                    .subsurface_transaction_metrics
                    .root_wide_supersessions
                    .saturating_add(1);
                released.callbacks.extend(transaction.callbacks);
                if released.resize_commit.is_none() {
                    released.resize_commit = transaction.resize_commit;
                } else if let Some(resize_commit) = transaction.resize_commit {
                    self.release_detached_resize_capture(root_surface_id, resize_commit);
                }
            } else {
                retained.push(transaction);
            }
        }
        self.pending_surface_tree_transactions = retained;
        let (callbacks, dependent_pacing_deadline_changed) =
            self.cancel_pending_surface_tree_dependents(canceled_refs, reason);
        pacing_deadline_changed |= dependent_pacing_deadline_changed;
        released.callbacks.extend(callbacks);
        if pacing_deadline_changed {
            self.invalidate_surface_pacing_deadline_cache();
        }
        self.rebuild_scene_work_index();
        released
    }

    pub(in crate::compositor) fn cancel_pending_surface_trees_for_surface(
        &mut self,
        surface_id: u32,
        reason: AcquireWatchCancelReason,
    ) {
        let mut retained = Vec::new();
        let mut canceled_refs = Vec::new();
        let mut pacing_deadline_changed = false;
        let mut callbacks = Vec::new();
        for transaction in std::mem::take(&mut self.pending_surface_tree_transactions) {
            if transaction
                .nodes
                .iter()
                .any(|(node_surface_id, _)| *node_surface_id == surface_id)
            {
                canceled_refs.extend(
                    transaction.nodes.iter().map(|(node_surface_id, commit)| {
                        commit.content_update_ref(*node_surface_id)
                    }),
                );
                pacing_deadline_changed |= transaction.commit_timing_readiness.is_some();
                let root_surface_id = transaction.root_surface_id;
                let released = self.release_pending_surface_tree_transaction(transaction, reason);
                callbacks.extend(released.callbacks);
                if let Some(resize_commit) = released.resize_commit {
                    self.release_detached_resize_capture(root_surface_id, resize_commit);
                }
            } else {
                retained.push(transaction);
            }
        }
        self.pending_surface_tree_transactions = retained;
        let (dependent_callbacks, dependent_pacing_deadline_changed) =
            self.cancel_pending_surface_tree_dependents(canceled_refs, reason);
        pacing_deadline_changed |= dependent_pacing_deadline_changed;
        callbacks.extend(dependent_callbacks);
        if pacing_deadline_changed {
            self.invalidate_surface_pacing_deadline_cache();
        }
        self.rebuild_scene_work_index();
        self.complete_frame_callbacks(callbacks);
    }

    fn cancel_pending_surface_tree_dependents(
        &mut self,
        canceled_refs: Vec<ContentUpdateRef>,
        reason: AcquireWatchCancelReason,
    ) -> (Vec<wl_callback::WlCallback>, bool) {
        let mut canceled_refs = canceled_refs;
        let mut callbacks = Vec::new();
        let mut pacing_deadline_changed = false;
        loop {
            let mut retained = Vec::new();
            let mut newly_canceled_refs = Vec::new();
            for transaction in std::mem::take(&mut self.pending_surface_tree_transactions) {
                if transaction_references_any_content_update(&transaction, &canceled_refs) {
                    newly_canceled_refs.extend(
                        transaction
                            .nodes
                            .iter()
                            .map(|(surface_id, commit)| commit.content_update_ref(*surface_id)),
                    );
                    pacing_deadline_changed |= transaction.commit_timing_readiness.is_some();
                    let root_surface_id = transaction.root_surface_id;
                    let released =
                        self.release_pending_surface_tree_transaction(transaction, reason);
                    callbacks.extend(released.callbacks);
                    if let Some(resize_commit) = released.resize_commit {
                        self.release_detached_resize_capture(root_surface_id, resize_commit);
                    }
                } else {
                    retained.push(transaction);
                }
            }
            self.pending_surface_tree_transactions = retained;
            if newly_canceled_refs.is_empty() {
                break;
            }
            canceled_refs.extend(newly_canceled_refs);
        }
        (callbacks, pacing_deadline_changed)
    }

    pub(in crate::compositor) fn discard_surface_tree_dependents_from_queue(
        &mut self,
        transactions: &mut Vec<PendingSurfaceTreeTransaction>,
        canceled_root_surface_id: u32,
        canceled_refs: Vec<ContentUpdateRef>,
        decision: SurfacePublicationDecision,
    ) -> bool {
        let mut canceled_roots = vec![canceled_root_surface_id];
        let mut canceled_refs = canceled_refs;
        let mut pacing_deadline_changed = false;
        loop {
            let mut retained = Vec::new();
            let mut newly_canceled_roots = Vec::new();
            let mut newly_canceled_refs = Vec::new();
            for transaction in std::mem::take(transactions) {
                if canceled_roots.contains(&transaction.root_surface_id)
                    || transaction_references_any_content_update(&transaction, &canceled_refs)
                {
                    newly_canceled_roots.push(transaction.root_surface_id);
                    newly_canceled_refs.extend(
                        transaction
                            .nodes
                            .iter()
                            .map(|(surface_id, commit)| commit.content_update_ref(*surface_id)),
                    );
                    pacing_deadline_changed |= transaction.commit_timing_readiness.is_some();
                    let root_surface_id = transaction.root_surface_id;
                    let released = self.release_pending_surface_tree_transaction(
                        transaction,
                        AcquireWatchCancelReason::SurfaceDestroyed,
                    );
                    if let Some(resize_commit) = released.resize_commit {
                        self.release_detached_resize_capture(root_surface_id, resize_commit);
                    }
                    if decision == SurfacePublicationDecision::TerminalClient {
                        self.discard_frame_callbacks(released.callbacks);
                    } else {
                        self.complete_frame_callbacks(released.callbacks);
                    }
                } else {
                    retained.push(transaction);
                }
            }
            *transactions = retained;
            if newly_canceled_roots.is_empty() && newly_canceled_refs.is_empty() {
                break;
            }
            canceled_roots.extend(newly_canceled_roots);
            canceled_refs.extend(newly_canceled_refs);
        }
        pacing_deadline_changed
    }

    pub(in crate::compositor) fn release_pending_surface_tree_transaction(
        &mut self,
        mut transaction: PendingSurfaceTreeTransaction,
        reason: AcquireWatchCancelReason,
    ) -> ReleasedSurfaceTreeState {
        for (surface_id, commit) in &transaction.nodes {
            self.trace_surface_pipeline_event(
                SurfacePipelineEvent::TransactionAbandoned,
                *surface_id,
                commit.commit_sequence,
                commit
                    .attachment
                    .as_ref()
                    .and_then(|attachment| match attachment {
                        PendingSurfaceAttachment::Buffer(buffer) => {
                            Some(buffer.data.buffer_id().get())
                        }
                        PendingSurfaceAttachment::RemoveContent => None,
                    }),
                None,
                Some(transaction.id.get()),
                None,
                None,
                None,
            );
        }
        if self.external_acquire_readiness {
            for dependency in &transaction.dependencies {
                if dependency.state == PendingAcquireState::Ready {
                    continue;
                }
                self.pending_acquire_watch_changes
                    .push(AcquireWatchChange::Cancel {
                        commit_id: dependency.commit_id,
                        reason,
                    });
            }
        }
        let resize_commit =
            take_tree_resize_commit(transaction.root_surface_id, &mut transaction.nodes);
        self.release_resize_captures_for_tree_nodes(&transaction.nodes);
        ReleasedSurfaceTreeState {
            callbacks: self.take_unpublished_surface_tree_callbacks(transaction.nodes),
            resize_commit,
        }
    }

    pub(in crate::compositor) fn release_unpublished_surface_tree_nodes(
        &mut self,
        nodes: Vec<(u32, CachedSubsurfaceCommit)>,
    ) {
        self.release_resize_captures_for_tree_nodes(&nodes);
        let callbacks = self.take_unpublished_surface_tree_callbacks(nodes);
        self.complete_frame_callbacks(callbacks);
    }

    pub(in crate::compositor) fn release_resize_captures_for_tree_nodes(
        &mut self,
        nodes: &[(u32, CachedSubsurfaceCommit)],
    ) {
        for (surface_id, commit) in nodes {
            let resize = match commit.attachment.as_ref() {
                Some(PendingSurfaceAttachment::Buffer(buffer)) => {
                    buffer.resize_commit.as_deref().copied()
                }
                _ => commit.resize_commit,
            };
            if let Some(resize) = resize {
                self.release_resize_capture(*surface_id, resize.commit_sequence);
            }
        }
    }

    pub(in crate::compositor) fn release_detached_resize_capture(
        &mut self,
        surface_id: u32,
        resize_commit: ResizeCommitSnapshot,
    ) {
        self.release_resize_capture(surface_id, resize_commit.commit_sequence);
    }

    pub(in crate::compositor) fn take_unpublished_surface_tree_callbacks(
        &mut self,
        nodes: Vec<(u32, CachedSubsurfaceCommit)>,
    ) -> Vec<wl_callback::WlCallback> {
        let mut callbacks = Vec::new();
        for (_, commit) in nodes {
            callbacks.extend(commit.frame_callbacks);
            for feedback in commit.presentation_feedbacks {
                feedback.feedback.discarded();
            }
            if let Some(PendingSurfaceAttachment::Buffer(buffer)) = commit.attachment {
                self.release_pending_surface_buffer(buffer);
            }
        }
        callbacks
    }

    pub(in crate::compositor) fn apply_captured_subsurface_parent_state(
        &mut self,
        parent_id: u32,
        captured: CapturedSubsurfaceParentState,
    ) -> bool {
        let CapturedSubsurfaceParentState {
            activations,
            mut positions,
            stack,
        } = captured;
        let mut changed = false;
        for relationship in activations {
            if relationship.parent_id != parent_id
                || !self
                    .surface_resources
                    .contains_key(&relationship.surface_id)
                || !self.surface_resources.contains_key(&parent_id)
                || !self
                    .subsurface_transactions
                    .apply_captured_relationship(relationship)
            {
                continue;
            }
            let position = positions
                .iter()
                .position(|position| position.relationship == relationship)
                .map(|index| positions.remove(index))
                .map(|position| (position.x, position.y))
                .unwrap_or((0, 0));
            let placement = SurfacePlacement::subsurface(parent_id, position.0, position.1);
            changed |= self.surface_placement(relationship.surface_id) != placement;
            self.set_surface_placement(relationship.surface_id, placement);
        }
        for position in positions {
            if position.relationship.parent_id != parent_id
                || !self
                    .subsurface_transactions
                    .relationship_is_applied(position.relationship)
            {
                continue;
            }
            let placement = SurfacePlacement::subsurface(parent_id, position.x, position.y);
            changed |= self.surface_placement(position.relationship.surface_id) != placement;
            self.set_surface_placement(position.relationship.surface_id, placement);
        }
        if let Some(stack) = stack {
            changed |= self.apply_captured_subsurface_stack_for_parent(parent_id, stack);
        }
        self.reconcile_applied_subsurface_mapping(parent_id);
        if changed {
            self.advance_render_generation_with_scene_effect(
                RenderGenerationCause::SurfaceCommit,
                self.surface_is_visible_in_active_scene(parent_id),
            );
        }
        changed
    }

    pub(in crate::compositor) fn publish_surface_tree(
        &mut self,
        root_id: u32,
        commits: Vec<(u32, CachedSubsurfaceCommit)>,
    ) {
        let changed_nodes = commits.len();
        let maximum_wait_ms = commits
            .iter()
            .map(|(_, commit)| commit)
            .map(|commit| u64::try_from(commit.cached_at.elapsed().as_millis()).unwrap_or(u64::MAX))
            .max()
            .unwrap_or(0);
        self.subsurface_transaction_metrics
            .maximum_transaction_wait_ms = self
            .subsurface_transaction_metrics
            .maximum_transaction_wait_ms
            .max(maximum_wait_ms);
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: subsurface_tx root={root_id} decision=prepared changed_nodes={changed_nodes}",
            );
        }
        self.begin_surface_tree_publication();
        for (surface_id, commit) in commits {
            self.apply_cached_subsurface_commit(surface_id, commit);
        }
        self.finish_surface_tree_publication();
        self.debug_assert_surface_tree_invariants();
        self.subsurface_transaction_metrics
            .tree_transactions_published = self
            .subsurface_transaction_metrics
            .tree_transactions_published
            .saturating_add(1);
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: subsurface_tx root={root_id} decision=published changed_nodes={} tree_generation={}",
                changed_nodes, self.render_generation,
            );
        }
    }

    pub(in crate::compositor) fn pending_stack_for_parent(
        &mut self,
        parent_id: u32,
    ) -> &mut Vec<u32> {
        self.pending_subsurface_stacks
            .entry(parent_id)
            .or_insert_with(|| {
                self.latched_subsurface_stacks
                    .get(&parent_id)
                    .cloned()
                    .or_else(|| self.committed_subsurface_stacks.get(&parent_id).cloned())
                    .unwrap_or_else(|| vec![parent_id])
            })
    }

    pub(in crate::compositor) fn add_subsurface_to_pending_stack(
        &mut self,
        parent_id: u32,
        surface_id: u32,
    ) {
        let stack = self.pending_stack_for_parent(parent_id);
        stack.retain(|id| *id == parent_id || *id != surface_id);
        if !stack.contains(&parent_id) {
            stack.insert(0, parent_id);
        }
        stack.push(surface_id);
    }

    pub(in crate::compositor) fn restack_subsurface(
        &mut self,
        surface_id: u32,
        parent_id: u32,
        reference_id: u32,
        above: bool,
    ) -> bool {
        if reference_id == surface_id {
            return false;
        }
        let valid_reference = reference_id == parent_id
            || self
                .subsurface_transactions
                .relationship_is_registered_child_of(reference_id, parent_id);
        if !valid_reference {
            return false;
        }

        let stack = self.pending_stack_for_parent(parent_id);
        stack.retain(|id| *id == parent_id || *id != surface_id);
        if !stack.contains(&parent_id) {
            stack.insert(0, parent_id);
        }
        let Some(reference_index) = stack.iter().position(|id| *id == reference_id) else {
            return false;
        };
        let insert_index = if above {
            reference_index + 1
        } else {
            reference_index
        };
        stack.insert(insert_index.min(stack.len()), surface_id);
        true
    }

    fn apply_captured_subsurface_stack_for_parent(
        &mut self,
        parent_id: u32,
        stack: Vec<CapturedSubsurfaceStackEntry>,
    ) -> bool {
        let mut applied_stack = stack
            .into_iter()
            .filter_map(|entry| match entry {
                CapturedSubsurfaceStackEntry::Parent => Some(parent_id),
                CapturedSubsurfaceStackEntry::Child(relationship)
                    if relationship.parent_id == parent_id
                        && self
                            .subsurface_transactions
                            .relationship_is_applied(relationship) =>
                {
                    Some(relationship.surface_id)
                }
                CapturedSubsurfaceStackEntry::Child(_) => None,
            })
            .collect::<Vec<_>>();
        if !applied_stack.contains(&parent_id) {
            applied_stack.insert(0, parent_id);
        }
        applied_stack.dedup();
        let changed = self
            .committed_subsurface_stacks
            .get(&parent_id)
            .is_none_or(|current| *current != applied_stack);
        self.committed_subsurface_stacks
            .insert(parent_id, applied_stack);
        if changed {
            self.reorder_renderable_surfaces_by_committed_stack();
            self.refresh_pointer_focus_at_last_position();
        }
        changed
    }

    pub(in crate::compositor) fn cleanup_subsurface_stack_state_for_surface(
        &mut self,
        surface_id: u32,
    ) {
        self.committed_subsurface_stacks.remove(&surface_id);
        self.latched_subsurface_stacks.remove(&surface_id);
        self.pending_subsurface_stacks.remove(&surface_id);
        for stack in self.committed_subsurface_stacks.values_mut() {
            stack.retain(|id| *id != surface_id);
            stack.dedup();
        }
        for stack in self.pending_subsurface_stacks.values_mut() {
            stack.retain(|id| *id != surface_id);
            stack.dedup();
        }
        for stack in self.latched_subsurface_stacks.values_mut() {
            stack.retain(|id| *id != surface_id);
            stack.dedup();
        }
        self.committed_subsurface_stacks.retain(|parent_id, stack| {
            self.surface_resources.contains_key(parent_id) && stack.iter().any(|id| id != parent_id)
        });
        self.pending_subsurface_stacks.retain(|parent_id, stack| {
            self.surface_resources.contains_key(parent_id) && stack.iter().any(|id| id != parent_id)
        });
        self.latched_subsurface_stacks.retain(|parent_id, stack| {
            self.surface_resources.contains_key(parent_id) && stack.iter().any(|id| id != parent_id)
        });
        self.reorder_renderable_surfaces_by_committed_stack();
    }

    fn detach_subsurface_from_parent_stack_lineage(&mut self, parent_id: u32, surface_id: u32) {
        fn remove_from_stack(stacks: &mut HashMap<u32, Vec<u32>>, parent_id: u32, surface_id: u32) {
            let Some(stack) = stacks.get_mut(&parent_id) else {
                return;
            };
            stack.retain(|id| *id != surface_id);
            stack.dedup();
            if stack.len() <= 1 && stack.first().copied() == Some(parent_id) {
                stacks.remove(&parent_id);
            }
        }

        remove_from_stack(&mut self.committed_subsurface_stacks, parent_id, surface_id);
        remove_from_stack(&mut self.latched_subsurface_stacks, parent_id, surface_id);
        remove_from_stack(&mut self.pending_subsurface_stacks, parent_id, surface_id);
    }

    pub(in crate::compositor) fn destroy_subsurface_role(&mut self, surface_id: u32) {
        let affected_surfaces = self.subsurface_transactions.subsurface_tree_ids(surface_id);
        let was_effectively_synchronized = affected_surfaces
            .iter()
            .map(|surface_id| {
                (
                    *surface_id,
                    self.is_effectively_synchronized_subsurface(*surface_id),
                )
            })
            .collect::<HashMap<_, _>>();
        let Some(detached) = self.subsurface_transactions.detach_role(surface_id) else {
            return;
        };
        let parent_id = detached.relationship.parent_id;
        let client_id = detached.client_id.clone();
        let promoted_refs = detached
            .cached_commits
            .iter()
            .map(|commit| commit.content_update_ref(surface_id))
            .collect::<Vec<_>>();
        if was_effectively_synchronized
            .get(&surface_id)
            .copied()
            .unwrap_or(false)
        {
            self.subsurface_transactions
                .remove_cached_parent_dependencies_to_child_commits(
                    parent_id,
                    surface_id,
                    &promoted_refs,
                );
        }
        self.update_synchronized_cache_metrics();
        self.hide_renderable_surface_subtree(surface_id);
        self.deactivate_role_instance(surface_id);
        self.set_surface_placement(surface_id, SurfacePlacement::root());
        self.detach_subsurface_from_parent_stack_lineage(parent_id, surface_id);
        self.reorder_renderable_surfaces_by_committed_stack();
        self.reclassify_unreachable_synchronized_commits(
            &affected_surfaces,
            &was_effectively_synchronized,
            Some((surface_id, detached.cached_commits)),
        );
        self.debug_assert_surface_tree_invariants();
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: subsurface_tx surface={surface_id} parent={parent_id:?} client={client_id:?} decision=destroyed reason=role_destroyed"
            );
        }
    }

    pub(in crate::compositor) fn release_cached_subsurface_commits(
        &mut self,
        commits: Vec<CachedSubsurfaceCommit>,
    ) {
        for commit in commits {
            for feedback in commit.presentation_feedbacks {
                feedback.feedback.discarded();
            }
            if let Some(PendingSurfaceAttachment::Buffer(buffer)) = commit.attachment {
                self.release_pending_surface_buffer(buffer);
            }
        }
    }

    pub(in crate::compositor) fn debug_assert_surface_tree_invariants(&self) {
        #[cfg(debug_assertions)]
        {
            let mut renderable_ids = HashSet::new();
            for surface in &self.renderable_surfaces {
                debug_assert!(renderable_ids.insert(surface.surface_id));
                if let Some(parent_id) = surface.placement.parent_surface_id {
                    debug_assert!(self.surface_resources.contains_key(&parent_id));
                }
            }
            for (parent_id, stack) in &self.committed_subsurface_stacks {
                Self::debug_assert_subsurface_stack_invariant(*parent_id, stack);
            }
            for (parent_id, stack) in &self.latched_subsurface_stacks {
                Self::debug_assert_subsurface_stack_invariant(*parent_id, stack);
            }
            for (parent_id, stack) in &self.pending_subsurface_stacks {
                Self::debug_assert_subsurface_stack_invariant(*parent_id, stack);
            }
        }
    }

    #[cfg(debug_assertions)]
    fn debug_assert_subsurface_stack_invariant(parent_id: u32, stack: &[u32]) {
        let mut stack_ids = HashSet::new();
        debug_assert!(stack.iter().all(|surface_id| stack_ids.insert(*surface_id)));
        debug_assert_eq!(stack.iter().filter(|id| **id == parent_id).count(), 1);
    }

    pub(in crate::compositor) fn take_and_bind_surface_presentation_feedbacks(
        &mut self,
        surface_id: u32,
        commit_sequence: SurfaceCommitSequence,
    ) -> Vec<PendingPresentationFeedback> {
        let Some(surface_generation) = self
            .surface_presentation_generations
            .get(&surface_id)
            .copied()
        else {
            for feedback in self
                .pending_surface_presentation_feedbacks
                .remove(&surface_id)
                .unwrap_or_default()
            {
                feedback.feedback.discarded();
            }
            return Vec::new();
        };
        self.pending_surface_presentation_feedbacks
            .remove(&surface_id)
            .unwrap_or_default()
            .into_iter()
            .map(|feedback| PendingPresentationFeedback {
                surface_id,
                surface_presentation_generation: surface_generation,
                commit_sequence,
                surface: feedback.surface,
                feedback: feedback.feedback,
            })
            .collect()
    }

    pub(in crate::compositor) fn set_surface_placement(
        &mut self,
        surface_id: u32,
        placement: SurfacePlacement,
    ) -> bool {
        self.set_surface_placement_with_cause(
            surface_id,
            placement,
            RenderGenerationCause::SurfacePlacement,
        )
    }

    pub(in crate::compositor) fn set_surface_placement_with_cause(
        &mut self,
        surface_id: u32,
        placement: SurfacePlacement,
        cause: RenderGenerationCause,
    ) -> bool {
        if self.surface_placement(surface_id) == placement {
            return false;
        }

        self.store_surface_placement(surface_id, placement);
        let root_surface_id = self.root_surface_id_for_surface(surface_id);
        if let Some(visual) = self.toplevel_visual_geometries.get_mut(&surface_id) {
            visual.placement = placement;
        }

        if let Some(surface) = self
            .renderable_surfaces
            .iter_mut()
            .find(|surface| surface.surface_id == surface_id)
        {
            surface.placement = placement;
            let refreshed_by_visual_assignment = self
                .toplevel_visual_geometries
                .contains_key(&root_surface_id)
                || self.toplevel_surfaces.contains_key(&root_surface_id);
            if refreshed_by_visual_assignment {
                self.update_toplevel_visual_render_assignment(root_surface_id);
            } else {
                self.refresh_active_scene_surface_tree(root_surface_id);
            }
            if refreshed_by_visual_assignment {
                self.compliance_metrics.prevented_duplicate_root_refreshes = self
                    .compliance_metrics
                    .prevented_duplicate_root_refreshes
                    .saturating_add(1);
            }
            self.advance_render_generation_with_scene_effect(
                cause,
                self.surface_is_visible_in_active_scene(root_surface_id),
            );
            return true;
        }

        false
    }

    pub(in crate::compositor) fn refresh_surface_origin_cache(&mut self) {
        if self.surface_origin_cache_generation != Some(self.render_generation)
            || self.surface_origin_cache.len() != self.renderable_surfaces.len()
        {
            self.surface_origin_cache = render::surface_origins(&self.renderable_surfaces);
            self.surface_origin_cache_generation = Some(self.render_generation);
            self.pointer_hit_metrics.global_origin_cache_recomputes = self
                .pointer_hit_metrics
                .global_origin_cache_recomputes
                .saturating_add(1);
        }
    }

    pub(in crate::compositor) fn invalidate_surface_origin_cache(&mut self) {
        self.surface_origin_cache_generation = None;
        self.visual_stack_groups_cache_generation = None;
        self.advance_pointer_hit_generation();
    }

    pub(in crate::compositor) fn raise_renderable_surface_tree(&mut self, surface_id: u32) -> bool {
        let tree_ids = self
            .renderable_surfaces
            .iter()
            .map(|surface| surface.surface_id)
            .filter(|candidate_id| self.surface_is_descendant_of(*candidate_id, surface_id))
            .collect::<HashSet<_>>();
        if tree_ids.is_empty() {
            return false;
        }

        let original_order = self
            .renderable_surfaces
            .iter()
            .map(|surface| surface.surface_id)
            .collect::<Vec<_>>();
        let mut tree = Vec::new();
        let mut lower = Vec::with_capacity(self.renderable_surfaces.len());
        for surface in self.renderable_surfaces.drain(..) {
            if tree_ids.contains(&surface.surface_id) {
                tree.push(surface);
            } else {
                lower.push(surface);
            }
        }
        lower.extend(tree);
        let changed = lower
            .iter()
            .map(|surface| surface.surface_id)
            .ne(original_order);
        self.renderable_surfaces = lower;
        self.rebuild_renderable_surface_index();
        if changed {
            self.invalidate_surface_origin_cache();
            self.refresh_active_scene_surface_order();
        }
        changed
    }
}

#[cfg(test)]
mod promotion_tests {
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

    fn test_captured_lifetimes(
        client_id: &ClientId,
        surface_ids: &[u32],
    ) -> SurfaceTreeNodeLifetimes {
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
        assert!(state.subsurface_transactions.register(parent_id, root_id));
        assert!(state.subsurface_transactions.register(child_id, parent_id));

        let mut child = test_cached_commit(70);
        child.pacing.commit_timing = Some(
            CommitTimingConstraint::from_protocol(client_pacing_now_ns() / 1_000_000_000 + 60, 0)
                .expect("future child timing"),
        );
        let child_ref = child.content_update_ref(child_id);
        assert!(matches!(
            state.subsurface_transactions.cache_commit(child_id, child),
            CacheCommitOutcome::Inserted
        ));
        let mut parent = test_cached_commit(71);
        let parent_ref = parent.content_update_ref(parent_id);
        parent.lineage.child_dependencies = vec![child_ref];
        assert!(matches!(
            state
                .subsurface_transactions
                .cache_commit(parent_id, parent),
            CacheCommitOutcome::Inserted
        ));

        state.destroy_subsurface_role(child_id);

        assert!(
            state
                .pending_surface_tree_transactions
                .iter()
                .any(|transaction| {
                    transaction.nodes.iter().any(|(surface_id, commit)| {
                        *surface_id == child_id
                            && commit.commit_sequence == SurfaceCommitSequence(70)
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
        let mut state = SubsurfaceTransactionState::default();
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
        let mut state = SubsurfaceTransactionState::default();
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
        assert!(state.subsurface_transactions.register(2, 1));
        assert!(state.subsurface_transactions.register(3, 2));
        assert!(state.subsurface_transactions.register(4, 2));
        let first = test_cached_commit(100);
        let first_ref = first.content_update_ref(3);
        let second = test_cached_commit(101);
        let second_ref = second.content_update_ref(3);
        assert!(matches!(
            state.subsurface_transactions.cache_commit(3, first),
            CacheCommitOutcome::Inserted
        ));
        assert!(matches!(
            state.subsurface_transactions.cache_commit(3, second),
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
            state.subsurface_transactions.cache_commit(2, older_parent),
            CacheCommitOutcome::Inserted
        ));
        let mut newer_parent = test_mergeable_commit(201);
        newer_parent.lineage.child_dependencies = vec![second_ref, sibling_ref];
        assert!(matches!(
            state.subsurface_transactions.cache_commit(2, newer_parent),
            CacheCommitOutcome::Merged { .. }
        ));

        state.destroy_subsurface_role(3);

        let parent_commits = state
            .subsurface_transactions
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
        assert!(state.subsurface_transactions.register(child, parent_id));
        let child_commit = test_cached_commit(110);
        let child_ref = child_commit.content_update_ref(child);
        let mut parent_commit = test_cached_commit(111);
        parent_commit.lineage.child_dependencies = vec![child_ref];
        state
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

        assert_eq!(state.pending_surface_tree_transactions.len(), 1);
        assert_eq!(
            state.pending_surface_tree_transactions[0]
                .nodes
                .iter()
                .map(|(_, commit)| commit.commit_sequence)
                .collect::<Vec<_>>(),
            vec![SurfaceCommitSequence(111), SurfaceCommitSequence(110)]
        );
        assert_eq!(
            state.pending_surface_tree_transactions[0].nodes[0]
                .1
                .lineage
                .child_dependencies,
            vec![child_ref]
        );
    }
}
