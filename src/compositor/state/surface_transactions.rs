use super::*;
use crate::compositor::state_data::SurfaceContentMapping;
use crate::compositor::subsurface::ContentUpdateRef;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SurfaceTreeTransactionId(u64);

impl SurfaceTreeTransactionId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug)]
pub(in crate::compositor) struct SurfaceTreeAcquireDependency {
    pub(in crate::compositor) surface_commit_id: SurfaceCommitId,
    pub(in crate::compositor) commit_id: AcquireCommitId,
    pub(in crate::compositor) surface_id: u32,
    // `None` is only used by synthetic legacy tests; production admissions
    // always capture both values and therefore fail closed if either is absent.
    pub(in crate::compositor) owner_client_id: Option<ClientId>,
    pub(in crate::compositor) surface_presentation_generation: Option<u64>,
    pub(in crate::compositor) buffer_id: u32,
    pub(in crate::compositor) acquire: ExplicitSyncPoint,
    pub(in crate::compositor) state: PendingAcquireState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::compositor) struct SurfaceTreeNodeLifetime {
    pub(in crate::compositor) surface_id: u32,
    pub(in crate::compositor) owner_client_id: ClientId,
    pub(in crate::compositor) surface_presentation_generation: u64,
}

#[derive(Debug, Clone)]
pub(in crate::compositor) enum SurfaceTreeNodeLifetimes {
    Captured(Vec<SurfaceTreeNodeLifetime>),
    #[cfg(test)]
    Synthetic,
}

impl SurfaceTreeNodeLifetimes {
    pub(in crate::compositor) fn captured(&self) -> Option<&[SurfaceTreeNodeLifetime]> {
        match self {
            Self::Captured(lifetimes) => Some(lifetimes),
            #[cfg(test)]
            Self::Synthetic => None,
        }
    }
}

#[derive(Debug)]
pub(in crate::compositor) struct PendingSurfaceTreeTransaction {
    pub(in crate::compositor) id: SurfaceTreeTransactionId,
    pub(in crate::compositor) root_surface_id: u32,
    pub(in crate::compositor) nodes: Vec<(u32, CachedSubsurfaceCommit)>,
    pub(in crate::compositor) publication_lifetimes: SurfaceTreeNodeLifetimes,
    pub(in crate::compositor) dependencies: Vec<SurfaceTreeAcquireDependency>,
    pub(in crate::compositor) external_content_update_dependencies: Vec<ContentUpdateRef>,
    pub(in crate::compositor) commit_timing_readiness: Option<CommitTimingReadiness>,
    pub(in crate::compositor) received_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) enum TransactionOrdering {
    Coalescible,
    PacingProtected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) enum SurfaceTreeSubmissionKind {
    /// A new client Content Update subject to normal admission bounds.
    ClientAdmission,
    /// Already-admitted work materialized after a synchronization transition.
    InternalMigration,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) struct SurfacePublicationState {
    pub(in crate::compositor) latest_received: SurfaceCommitSequence,
    pub(in crate::compositor) latest_attachment_received: Option<SurfaceCommitSequence>,
    pub(in crate::compositor) latest_published: Option<SurfaceCommitSequence>,
    pub(in crate::compositor) latest_published_buffer_id: Option<BufferId>,
    /// The latest Content Update explicitly settled without publication.
    ///
    /// This is a surface-local high-water mark for teardown paths which
    /// retire all unpublished work for the surface. It is not publication
    /// state and must not be used as a published sequence.
    pub(in crate::compositor) latest_terminal: Option<SurfaceCommitSequence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) struct ActiveSurfacePresentationCommit {
    pub(in crate::compositor) surface_generation: u64,
    pub(in crate::compositor) commit_sequence: SurfaceCommitSequence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) enum SurfacePublicationDecision {
    Publish,
    StaleAlreadyPublished,
    SupersededByNewerAttachment,
    SurfaceGone,
    OwnerGone,
    TerminalClient,
    StaleSurfaceGeneration,
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
    pub(in crate::compositor) mapping: Option<SurfaceContentMapping>,
    pub(in crate::compositor) resize_commit: Option<ResizeCommitSnapshot>,
    pub(in crate::compositor) resize_capture_finalized: bool,
    pub(in crate::compositor) window_geometry: Option<XdgWindowGeometry>,
}

impl PendingSurfaceTreeTransaction {
    pub(in crate::compositor) fn commit_timing_request(&self) -> Option<CommitTimingConstraint> {
        self.nodes
            .iter()
            .filter_map(|(_, commit)| commit.pacing.commit_timing)
            .max_by(|left, right| left.ordering_key().cmp(&right.ordering_key()))
    }

    pub(in crate::compositor) fn ordering(&self) -> TransactionOrdering {
        if self
            .nodes
            .iter()
            .any(|(_, commit)| commit.pacing.is_boundary() || commit.lineage.merge_frozen)
        {
            TransactionOrdering::PacingProtected
        } else {
            TransactionOrdering::Coalescible
        }
    }

    pub(in crate::compositor) fn is_pacing_protected(&self) -> bool {
        self.ordering() == TransactionOrdering::PacingProtected
    }
}

impl SurfacePublicationDecision {
    pub(in crate::compositor) const fn pipeline_rejection_reason(
        self,
    ) -> Option<SurfacePipelineRejectionReason> {
        match self {
            Self::Publish => None,
            Self::StaleAlreadyPublished => Some(SurfacePipelineRejectionReason::AlreadyPublished),
            Self::SupersededByNewerAttachment => {
                Some(SurfacePipelineRejectionReason::SupersededByNewerAttachment)
            }
            Self::SurfaceGone => Some(SurfacePipelineRejectionReason::SurfaceGone),
            Self::OwnerGone => Some(SurfacePipelineRejectionReason::OwnerGone),
            Self::TerminalClient => Some(SurfacePipelineRejectionReason::TerminalClient),
            Self::StaleSurfaceGeneration => {
                Some(SurfacePipelineRejectionReason::StaleSurfaceGeneration)
            }
        }
    }
}

impl CompositorState {
    pub(in crate::compositor) fn surface_tree_async_publication_rejection(
        &self,
        transaction: &PendingSurfaceTreeTransaction,
    ) -> Option<(u32, SurfacePublicationDecision)> {
        let Some(lifetimes) = transaction.publication_lifetimes.captured() else {
            // Synthetic transactions are used only by legacy unit tests that do not
            // model Wayland surface resources. Production admissions always capture
            // one lifetime record for every transaction node.
            return None;
        };
        if lifetimes.len() != transaction.nodes.len() {
            return Some((
                transaction.root_surface_id,
                SurfacePublicationDecision::SurfaceGone,
            ));
        }
        for (node_index, lifetime) in lifetimes.iter().enumerate() {
            if transaction.nodes[node_index].0 != lifetime.surface_id {
                return Some((
                    lifetime.surface_id,
                    SurfacePublicationDecision::StaleSurfaceGeneration,
                ));
            }
            if let Some(rejection) = self
                .async_surface_lifecycle_rejection(lifetime.surface_id, &lifetime.owner_client_id)
            {
                return Some((lifetime.surface_id, rejection));
            }
            if self
                .surface_presentation_generations
                .get(&lifetime.surface_id)
                .copied()
                != Some(lifetime.surface_presentation_generation)
            {
                return Some((
                    lifetime.surface_id,
                    SurfacePublicationDecision::StaleSurfaceGeneration,
                ));
            }
        }
        None
    }

    pub(in crate::compositor) fn allocate_surface_tree_transaction_id(
        &mut self,
    ) -> SurfaceTreeTransactionId {
        self.next_surface_tree_transaction_id = self
            .next_surface_tree_transaction_id
            .checked_add(1)
            .expect("surface tree transaction ID overflow");
        SurfaceTreeTransactionId::new(self.next_surface_tree_transaction_id)
    }

    pub(in crate::compositor) fn apply_cached_subsurface_commit(
        &mut self,
        surface_id: u32,
        commit: CachedSubsurfaceCommit,
    ) {
        let CachedSubsurfaceCommit {
            commit_id,
            commit_sequence,
            lineage: _,
            attachment,
            damage,
            frame_callbacks,
            explicit_sync,
            offset,
            viewport_destination,
            viewport_error_owner,
            buffer_scale,
            buffer_transform,
            opaque_region,
            input_region,
            background_effect,
            mut presentation_feedbacks,
            resize_commit,
            resize_capture_finalized,
            window_geometry,
            cached_at: _,
            pacing,
            presentation,
            pointer_constraint_state,
            commit_context,
        } = commit;
        self.apply_captured_surface_pacing(surface_id, commit_sequence, pacing);
        let Some(surface) = self.surface_resource_by_id(surface_id) else {
            return;
        };
        let Some(data) = surface.data::<SurfaceData>() else {
            return;
        };
        let prospective_viewport = data.viewport_for_change(viewport_destination);
        let prospective_buffer_scale = data.buffer_scale_for_change(buffer_scale);
        let prospective_buffer_transform = data.buffer_transform_for_change(buffer_transform);
        let effective_viewport_error_owner = if viewport_destination.source.is_some() {
            viewport_error_owner.clone()
        } else {
            data.committed_viewport_error_owner()
        };
        let retained_mapping = if attachment.is_none() {
            match self.current_surface_buffers.get(&surface_id) {
                Some(current) => match current.content_mapping_for_state(
                    prospective_viewport,
                    prospective_buffer_scale,
                    prospective_buffer_transform,
                    offset,
                ) {
                    Ok(mapping) => Some(mapping),
                    Err(error) => {
                        debug_assert!(
                            false,
                            "SurfaceTree preflight admitted an invalid retained mapping: {error:?}"
                        );
                        self.post_surface_mapping_error(
                            surface_id,
                            error,
                            effective_viewport_error_owner,
                        );
                        self.complete_frame_callbacks(frame_callbacks);
                        self.discard_presentation_feedbacks(presentation_feedbacks);
                        if let Some(resize_commit) = resize_commit {
                            self.release_resize_capture(surface_id, resize_commit.commit_sequence);
                        }
                        return;
                    }
                },
                None => None,
            }
        } else {
            None
        };
        data.apply_presentation(presentation);
        data.apply_viewport_change_with_owner(viewport_destination, viewport_error_owner);
        data.apply_buffer_scale_change(buffer_scale);
        data.apply_buffer_transform_change(buffer_transform);
        let opaque_region_changed = data.apply_opaque_region_change(opaque_region);
        let renderable_index = self.renderable_surface_index(surface_id);
        let (opaque_width, opaque_height) = retained_mapping
            .map(|mapping| (mapping.surface_size.width, mapping.surface_size.height))
            .or_else(|| {
                renderable_index.and_then(|index| {
                    self.renderable_surfaces
                        .get(index)
                        .map(|surface| (surface.width, surface.height))
                })
            })
            .unwrap_or((0, 0));
        let opaque_region = data.opaque_region_for_surface_size(opaque_width, opaque_height);
        if let Some(renderable) =
            renderable_index.and_then(|index| self.renderable_surfaces.get_mut(index))
        {
            renderable.set_opaque_region(opaque_region.clone());
        }
        let input_region_changed = data.apply_input_region_change(input_region);
        let background_effect_changed = data.apply_background_effect_change(background_effect);
        if background_effect_changed {
            if data.committed_background_effect().ops().is_empty() {
                self.background_effect_surface_ids.remove(&surface_id);
            } else {
                self.background_effect_surface_ids.insert(surface_id);
            }
        }
        if input_region_changed {
            self.advance_pointer_hit_generation();
        }
        let window_geometry_changed =
            self.committed_window_geometry_changed(surface_id, window_geometry);
        let damage = damage.or(window_geometry_changed.then_some(RenderableSurfaceDamage::Full));
        let damage = damage.or(opaque_region_changed.then_some(RenderableSurfaceDamage::Full));
        let pointer_hit_generation_before_publication = self.pointer_hit_generation;
        let render_generation_before_publication = self.render_generation;
        let inactive_subsurface = self.subsurface_content_is_inactive(surface_id);
        let mut parent_commit_applied = true;
        match attachment {
            Some(PendingSurfaceAttachment::Buffer(mut pending)) => {
                pending.opaque_region = opaque_region;
                if let Some((x, y)) = offset {
                    pending.x = x;
                    pending.y = y;
                }
                debug_assert!(pending.surface_size.is_some());
                parent_commit_applied = self.commit_surface_request_with_captured_sync(
                    surface_id,
                    commit_id,
                    commit_sequence,
                    SurfacePublicationSource::SurfaceTree,
                    pending,
                    damage.unwrap_or_else(RenderableSurfaceDamage::full),
                    frame_callbacks,
                    std::mem::take(&mut presentation_feedbacks),
                    explicit_sync,
                    window_geometry,
                    commit_context.layer_surface,
                );
            }
            Some(PendingSurfaceAttachment::RemoveContent) => {
                if let Some(captured) = commit_context.layer_surface
                    && !self.apply_layer_surface_commit(surface_id, captured)
                {
                    self.complete_frame_callbacks(frame_callbacks);
                    self.discard_presentation_feedbacks(presentation_feedbacks);
                    return;
                }
                if self.is_cursor_surface(surface_id) {
                    self.commit_cursor_surface_removal_request(surface_id);
                    self.note_explicit_commit_published(commit_id);
                    self.complete_frame_callbacks(frame_callbacks);
                    self.activate_current_surface_presentation_commit(
                        surface_id,
                        commit_sequence,
                        presentation_feedbacks,
                    );
                } else {
                    let activated = self.commit_surface_remove_content(
                        surface_id,
                        commit_sequence,
                        frame_callbacks,
                        SurfacePublicationSource::SurfaceTree,
                    );
                    if activated && !inactive_subsurface {
                        self.activate_current_surface_presentation_commit(
                            surface_id,
                            commit_sequence,
                            presentation_feedbacks,
                        );
                    } else {
                        self.discard_presentation_feedbacks(presentation_feedbacks);
                    }
                    parent_commit_applied = activated;
                }
            }
            None => {
                let activated = self.commit_surface_without_buffer(
                    surface_id,
                    BufferlessSurfaceCommitState {
                        commit_sequence,
                        damage,
                        mapping: retained_mapping,
                        resize_commit,
                        resize_capture_finalized,
                        window_geometry,
                    },
                    commit_context.layer_surface,
                );
                if activated {
                    let current = self.current_surface_buffers.get(&surface_id);
                    self.record_surface_publication(
                        surface_id,
                        self.root_surface_id_for_surface(surface_id),
                        commit_sequence,
                        current.map(CurrentSurfaceBuffer::buffer_id),
                        SurfacePublicationSource::SurfaceTree,
                        current.and_then(|buffer| {
                            buffer
                                .width()
                                .ok()
                                .zip(buffer.height().ok())
                                .and_then(|(width, height)| BufferSize::new(width, height))
                        }),
                    );
                }
                if renderable_index.is_some() {
                    self.queue_frame_callbacks_for_surface(surface_id, frame_callbacks);
                } else {
                    self.complete_frame_callbacks(frame_callbacks);
                }
                if activated && !inactive_subsurface {
                    self.activate_current_surface_presentation_commit(
                        surface_id,
                        commit_sequence,
                        presentation_feedbacks,
                    );
                } else {
                    self.discard_presentation_feedbacks(presentation_feedbacks);
                }
                parent_commit_applied = activated;
            }
        }
        if parent_commit_applied {
            self.apply_captured_subsurface_parent_state(
                surface_id,
                commit_context.subsurface_parent,
            );
        }
        self.apply_captured_pointer_constraint_surface_state(surface_id, pointer_constraint_state);
        if input_region_changed
            && self.pointer_hit_generation == pointer_hit_generation_before_publication
        {
            self.refresh_pointer_focus_at_last_position();
        }
        if background_effect_changed {
            self.advance_render_generation_with_scene_effect(
                RenderGenerationCause::EffectBinding,
                self.surface_is_visible_in_active_scene(surface_id),
            );
            self.refresh_effect_scene_summary();
        }
        if parent_commit_applied
            && self.apply_acked_xdg_decoration(surface_id)
            && self.render_generation == render_generation_before_publication
        {
            self.advance_render_generation(RenderGenerationCause::WindowDecoration);
        }
    }
}
