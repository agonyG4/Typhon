use super::*;

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
        if commit.explicit_sync.is_some() {
            // Acquire admission belongs to SurfaceTree preparation. Settle an
            // impossible unprepared node here without re-entering admission.
            self.release_unpublished_surface_tree_nodes(vec![(surface_id, commit)]);
            debug_assert!(
                false,
                "SurfaceTree publication received raw captured explicit-sync state"
            );
            return;
        }
        debug_assert!(commit.explicit_sync.is_none());
        let CachedSubsurfaceCommit {
            commit_id,
            commit_sequence,
            lineage: _,
            attachment,
            damage,
            frame_callbacks,
            explicit_sync: _,
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
                pending.commit_sequence = commit_sequence;
                debug_assert!(pending.surface_size.is_some());
                if !self.surface_buffer_publication_preconditions(
                    surface_id,
                    &pending,
                    captured_layer_surface,
                ) {
                    self.release_pending_surface_buffer(pending);
                    self.complete_frame_callbacks(frame_callbacks);
                    self.discard_presentation_feedbacks(std::mem::take(
                        &mut presentation_feedbacks,
                    ));
                    parent_commit_applied = false;
                } else {
                    let callbacks = self.settle_direct_surface_buffer_before_publication(
                        surface_id,
                        commit_sequence,
                        &mut pending,
                        frame_callbacks,
                        window_geometry,
                    );
                    parent_commit_applied = self.publish_admitted_surface_buffer(
                        surface_id,
                        pending,
                        damage.unwrap_or_else(RenderableSurfaceDamage::full),
                        callbacks,
                        std::mem::take(&mut presentation_feedbacks),
                        SurfacePublicationSource::SurfaceTree,
                        window_geometry,
                    );
                }
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
    pub(in crate::compositor) fn publish_surface_tree_nodes(
        &mut self,
        transaction: PendingSurfaceTreeTransaction,
    ) {
        if transaction
            .nodes
            .iter()
            .any(|(_, commit)| commit.explicit_sync.is_some())
        {
            // SurfaceTree admission consumes raw explicit-sync state before
            // queueing. If it reappears here, settle all transaction-owned
            // resources before asserting; publication must never re-admit it.
            let root_surface_id = transaction.root_surface_id;
            let released = self.release_pending_surface_tree_transaction(
                transaction,
                AcquireWatchCancelReason::SurfaceDestroyed,
            );
            if let Some(resize_commit) = released.resize_commit {
                self.release_detached_resize_capture(root_surface_id, resize_commit);
            }
            self.complete_frame_callbacks(released.callbacks);
            debug_assert!(
                false,
                "SurfaceTree publication received raw captured explicit-sync state"
            );
            return;
        }
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
        self.surface_transactions
            .metrics
            .maximum_transaction_wait_ms = self
            .surface_transactions
            .metrics
            .maximum_transaction_wait_ms
            .max(maximum_wait_ms);
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: subsurface_tx root={root_id} decision=prepared changed_nodes={changed_nodes}",
            );
        }
        self.begin_surface_tree_publication();
        // Seed the authority snapshot before applying any node. Capturing only
        // when each node is reached would allow the first applied root/child
        // commit to become part of the supposed "before" geometry.
        let _ = self.capture_xdg_geometry_before_surface_commit(root_id);
        for (surface_id, commit) in commits {
            self.apply_cached_subsurface_commit(surface_id, commit);
        }
        self.finish_surface_tree_publication();
        self.debug_assert_surface_tree_invariants();
        self.surface_transactions
            .metrics
            .tree_transactions_published = self
            .surface_transactions
            .metrics
            .tree_transactions_published
            .saturating_add(1);
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: subsurface_tx root={root_id} decision=published changed_nodes={} tree_generation={}",
                changed_nodes, self.render_generation,
            );
        }
        if crate::compositor::state::roles::surface_tree_debug_enabled() {
            let xdg_geometry = self
                .effective_xdg_window_geometry(root_id)
                .map(|geometry| geometry.geometry)
                .map_or_else(
                    || "none".to_string(),
                    |geometry| {
                        format!(
                            "{},{},{},{}",
                            geometry.x, geometry.y, geometry.width, geometry.height
                        )
                    },
                );
            let root_commit_sequence = self
                .renderable_surfaces
                .iter()
                .find(|surface| surface.surface_id == root_id)
                .map(|surface| surface.commit_sequence.get());
            let origins = render::surface_origins(&self.renderable_surfaces);
            let active_surfaces = self.active_scene_surfaces();
            let active_origins = self.active_scene_surface_origins();
            let mut nodes = Vec::new();
            let mut omitted = 0usize;
            for (surface, (origin_x, origin_y)) in self.renderable_surfaces.iter().zip(origins) {
                if self.root_surface_id_for_surface(surface.surface_id) != root_id {
                    continue;
                }
                if nodes.len() == 16 {
                    omitted = omitted.saturating_add(1);
                    continue;
                }
                let relationship = self
                    .surface_transactions
                    .captured_relationship(surface.surface_id)
                    .map(|relationship| relationship.relationship_id.get().to_string())
                    .unwrap_or_else(|| "none".to_string());
                let active_origin = active_surfaces
                    .iter()
                    .position(|active| active.surface_id == surface.surface_id)
                    .and_then(|index| active_origins.get(index).copied())
                    .map_or_else(|| "none".to_string(), |(x, y)| format!("{x},{y}"));
                nodes.push(format!(
                    "surface={} parent={} relationship={} local={},{} origin={},{} active_origin={} commit_sequence={}",
                    surface.surface_id,
                    surface
                        .placement
                        .parent_surface_id
                        .map_or_else(|| "none".to_string(), |parent| parent.to_string()),
                    relationship,
                    surface.placement.local_x,
                    surface.placement.local_y,
                    origin_x,
                    origin_y,
                    active_origin,
                    surface.commit_sequence.get(),
                ));
            }
            eprintln!(
                "event=surface_tree_published root={} commit_sequence={} xdg_geometry={} nodes=[{}] omitted={}",
                root_id,
                root_commit_sequence.map_or_else(|| "none".to_string(), |value| value.to_string()),
                xdg_geometry,
                nodes.join("; "),
                omitted,
            );
        }
    }
}
