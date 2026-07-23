use super::*;

#[derive(Debug)]
pub(in crate::compositor) struct SurfaceTreeAcquireDependency {
    pub(in crate::compositor) surface_commit_id: SurfaceCommitId,
    pub(in crate::compositor) commit_id: AcquireCommitId,
    pub(in crate::compositor) surface_id: u32,
    pub(in crate::compositor) buffer_id: u32,
    pub(in crate::compositor) acquire: ExplicitSyncPoint,
    pub(in crate::compositor) state: PendingAcquireState,
}

#[derive(Debug)]
pub(in crate::compositor) struct PendingSurfaceTreeTransaction {
    pub(in crate::compositor) root_surface_id: u32,
    pub(in crate::compositor) nodes: Vec<(u32, CachedSubsurfaceCommit)>,
    pub(in crate::compositor) dependencies: Vec<SurfaceTreeAcquireDependency>,
    pub(in crate::compositor) received_at: Instant,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) struct SurfacePublicationState {
    pub(in crate::compositor) latest_received: SurfaceCommitSequence,
    pub(in crate::compositor) latest_attachment_received: Option<SurfaceCommitSequence>,
    pub(in crate::compositor) latest_published: Option<SurfaceCommitSequence>,
    pub(in crate::compositor) latest_published_buffer_id: Option<BufferId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) enum SurfacePublicationDecision {
    Publish,
    StaleAlreadyPublished,
    SupersededByNewerAttachment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) enum SurfacePublicationContext {
    ImmediateLatestAttachment,
    OrderedExplicitSyncQueue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(in crate::compositor) enum SurfacePublicationSource {
    Immediate,
    ExplicitSync,
    SurfaceTree,
    RemoveContent,
}

impl SurfacePublicationSource {
    pub(in crate::compositor) const fn as_str(self) -> &'static str {
        match self {
            Self::Immediate => "immediate",
            Self::ExplicitSync => "explicit_sync",
            Self::SurfaceTree => "surface_tree",
            Self::RemoveContent => "remove_content",
        }
    }

    pub(in crate::compositor) const fn publication_context(self) -> SurfacePublicationContext {
        match self {
            Self::ExplicitSync | Self::SurfaceTree => {
                SurfacePublicationContext::OrderedExplicitSyncQueue
            }
            Self::Immediate | Self::RemoveContent => {
                SurfacePublicationContext::ImmediateLatestAttachment
            }
        }
    }
}

#[derive(Debug)]
pub(in crate::compositor) struct ReleasedSurfaceTreeState {
    pub(in crate::compositor) callbacks: Vec<wl_callback::WlCallback>,
    pub(in crate::compositor) resize_commit: Option<ResizeCommitSnapshot>,
}

#[derive(Debug, Default)]
pub(in crate::compositor) struct SurfaceTreeMergeStats {
    pub(in crate::compositor) incoming_nodes: usize,
    pub(in crate::compositor) existing_nodes: usize,
    pub(in crate::compositor) bufferless_nodes: usize,
    pub(in crate::compositor) attachments_replaced: usize,
    pub(in crate::compositor) explicit_detaches: usize,
    pub(in crate::compositor) dependencies_preserved: usize,
    pub(in crate::compositor) dependencies_replaced: usize,
    pub(in crate::compositor) callbacks_merged: usize,
    pub(in crate::compositor) feedbacks_merged: usize,
    pub(in crate::compositor) resize_snapshots_preserved: usize,
    pub(in crate::compositor) resize_snapshots_replaced: usize,
}

pub(in crate::compositor) struct BufferlessSurfaceCommitState {
    pub(in crate::compositor) commit_sequence: SurfaceCommitSequence,
    pub(in crate::compositor) damage: Option<RenderableSurfaceDamage>,
    pub(in crate::compositor) explicit_sync: Option<Arc<SyncobjSurfaceState>>,
    pub(in crate::compositor) surface_size: Option<BufferSize>,
    pub(in crate::compositor) buffer_scale: u32,
    pub(in crate::compositor) resize_commit: Option<ResizeCommitSnapshot>,
    pub(in crate::compositor) resize_capture_finalized: bool,
    pub(in crate::compositor) window_geometry: Option<XdgWindowGeometry>,
}

impl PendingSurfaceTreeTransaction {
    pub(in crate::compositor) fn is_ready(&self) -> bool {
        self.dependencies
            .iter()
            .all(|dependency| dependency.state == PendingAcquireState::Ready)
    }
}

impl CompositorState {
    pub(in crate::compositor) fn apply_cached_subsurface_commit(
        &mut self,
        surface_id: u32,
        commit: CachedSubsurfaceCommit,
    ) {
        let CachedSubsurfaceCommit {
            commit_id,
            commit_sequence,
            attachment,
            damage,
            frame_callbacks,
            explicit_sync,
            offset,
            viewport_destination,
            buffer_scale,
            buffer_transform,
            opaque_region,
            input_region,
            presentation_feedbacks,
            resize_commit,
            resize_capture_finalized,
            window_geometry,
            cached_at: _,
        } = commit;
        let Some(surface) = self.surface_resource_by_id(surface_id) else {
            return;
        };
        let Some(data) = surface.data::<SurfaceData>() else {
            return;
        };
        let viewport = data.apply_viewport_change(viewport_destination);
        let surface_size = viewport
            .destination
            .or_else(|| viewport.source.and_then(ViewportSourceRect::logical_size));
        let committed_buffer_scale = data.apply_buffer_scale_change(buffer_scale);
        let _committed_buffer_transform = data.apply_buffer_transform_change(buffer_transform);
        let _opaque_region_changed = data.apply_opaque_region_change(opaque_region);
        let input_region_changed = data.apply_input_region_change(input_region);
        let damage = damage.or(window_geometry
            .is_some()
            .then_some(RenderableSurfaceDamage::Full));
        match attachment {
            Some(PendingSurfaceAttachment::Buffer(mut pending)) => {
                if let Some((x, y)) = offset {
                    pending.x = x;
                    pending.y = y;
                }
                debug_assert!(pending.surface_size.is_some());
                self.commit_surface_request_with_captured_sync(
                    surface_id,
                    commit_id,
                    commit_sequence,
                    SurfacePublicationSource::SurfaceTree,
                    pending,
                    damage.unwrap_or_else(RenderableSurfaceDamage::full),
                    frame_callbacks,
                    explicit_sync,
                    window_geometry,
                );
            }
            Some(PendingSurfaceAttachment::RemoveContent) => {
                if let Some(explicit_sync) = explicit_sync
                    && (explicit_sync.acquire.is_some() || explicit_sync.release.is_some())
                {
                    explicit_sync.state.post_error_with_metrics(
                        &mut self.compliance_metrics,
                        SYNCOBJ_SURFACE_ERROR_NO_BUFFER,
                        "explicit sync points were set without an attached buffer",
                    );
                    return;
                }
                if self.is_cursor_surface(surface_id) {
                    self.commit_cursor_surface_removal_request(surface_id, None);
                    self.note_explicit_commit_published(commit_id);
                    self.complete_frame_callbacks(frame_callbacks);
                } else {
                    self.commit_surface_remove_content(
                        surface_id,
                        commit_sequence,
                        frame_callbacks,
                        SurfacePublicationSource::SurfaceTree,
                    );
                }
            }
            None => {
                let explicit_sync = match explicit_sync {
                    Some(explicit_sync)
                        if explicit_sync.acquire.is_some() || explicit_sync.release.is_some() =>
                    {
                        explicit_sync.state.post_error_with_metrics(
                            &mut self.compliance_metrics,
                            SYNCOBJ_SURFACE_ERROR_NO_BUFFER,
                            "explicit sync points were set without an attached buffer",
                        );
                        return;
                    }
                    Some(explicit_sync) => Some(explicit_sync.state),
                    None => None,
                };
                self.commit_surface_without_buffer(
                    surface_id,
                    BufferlessSurfaceCommitState {
                        commit_sequence,
                        damage,
                        explicit_sync,
                        surface_size,
                        buffer_scale: committed_buffer_scale,
                        resize_commit,
                        resize_capture_finalized,
                        window_geometry,
                    },
                );
                self.note_explicit_commit_published(commit_id);
                if self
                    .renderable_surfaces
                    .iter()
                    .any(|surface| surface.surface_id == surface_id)
                {
                    self.pending_frame_callbacks.extend(frame_callbacks);
                } else {
                    self.complete_frame_callbacks(frame_callbacks);
                }
            }
        }
        if input_region_changed {
            self.refresh_pointer_focus_at_last_position();
        }
        self.pending_presentation_feedbacks
            .extend(presentation_feedbacks);
    }
}
