use super::*;
use crate::compositor::subsurface::ContentUpdateRef;

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
        let CapturedSurfaceCommitContext {
            subsurface_parent: captured_subsurface_parent,
            layer_surface: captured_layer_surface,
            xdg_decoration: captured_xdg_decoration,
        } = commit_context;
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
        let damage = damage.or(window_geometry
            .is_some()
            .then_some(RenderableSurfaceDamage::Full));
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
                    captured_layer_surface,
                );
            }
            Some(PendingSurfaceAttachment::RemoveContent) => {
                if let Some(captured) = captured_layer_surface
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
                    captured_layer_surface,
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
                commit_sequence,
                captured_subsurface_parent,
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
        if parent_commit_applied {
            let decoration_changed = self.apply_captured_xdg_decoration(
                surface_id,
                commit_sequence,
                captured_xdg_decoration,
            );
            if decoration_changed && self.render_generation == render_generation_before_publication
            {
                self.advance_render_generation(RenderGenerationCause::WindowDecoration);
            }
        }
        if self.surface_tree_generation.is_none() {
            let root_surface_id = self.root_surface_id_for_surface(surface_id);
            self.try_finalize_pending_normal_restore_from_committed_state(root_surface_id);
        }
    }
}

impl CompositorState {
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
            commit.attachment.as_ref()?;
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
        let transactions = self
            .surface_transactions
            .take_pending_trees_for_root(root_surface_id);
        let mut canceled_refs = Vec::new();
        let mut pacing_deadline_changed = false;
        let mut released = ReleasedSurfaceTreeState {
            callbacks: Vec::new(),
            resize_commit: None,
        };
        for transaction in transactions {
            canceled_refs.extend(
                transaction
                    .nodes
                    .iter()
                    .map(|(surface_id, commit)| commit.content_update_ref(*surface_id)),
            );
            pacing_deadline_changed |= transaction.commit_timing_readiness.is_some();
            let transaction = self.release_pending_surface_tree_transaction(transaction, reason);
            self.surface_transactions.metrics.root_wide_supersessions = self
                .surface_transactions
                .metrics
                .root_wide_supersessions
                .saturating_add(1);
            released.callbacks.extend(transaction.callbacks);
            if released.resize_commit.is_none() {
                released.resize_commit = transaction.resize_commit;
            } else {
                if let Some(resize_commit) = transaction.resize_commit {
                    self.release_detached_resize_capture(root_surface_id, resize_commit);
                }
            }
        }
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
        let transactions = self
            .surface_transactions
            .take_pending_trees_for_surface(surface_id);
        let mut canceled_refs = Vec::new();
        let mut pacing_deadline_changed = false;
        let mut callbacks = Vec::new();
        for transaction in transactions {
            canceled_refs.extend(
                transaction
                    .nodes
                    .iter()
                    .map(|(node_surface_id, commit)| commit.content_update_ref(*node_surface_id)),
            );
            pacing_deadline_changed |= transaction.commit_timing_readiness.is_some();
            let root_surface_id = transaction.root_surface_id;
            let released = self.release_pending_surface_tree_transaction(transaction, reason);
            callbacks.extend(released.callbacks);
            if let Some(resize_commit) = released.resize_commit {
                self.release_detached_resize_capture(root_surface_id, resize_commit);
            }
        }
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
            let dependents = self
                .surface_transactions
                .take_pending_tree_dependents(&canceled_refs);
            let mut newly_canceled_refs = Vec::new();
            for transaction in dependents {
                newly_canceled_refs.extend(
                    transaction
                        .nodes
                        .iter()
                        .map(|(surface_id, commit)| commit.content_update_ref(*surface_id)),
                );
                pacing_deadline_changed |= transaction.commit_timing_readiness.is_some();
                let root_surface_id = transaction.root_surface_id;
                let released = self.release_pending_surface_tree_transaction(transaction, reason);
                callbacks.extend(released.callbacks);
                if let Some(resize_commit) = released.resize_commit {
                    self.release_detached_resize_capture(root_surface_id, resize_commit);
                }
            }
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
}
