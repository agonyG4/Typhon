use super::*;

impl CompositorState {
    pub(super) fn surface_buffer_publication_preconditions(
        &mut self,
        surface_id: u32,
        pending: &PendingSurfaceBuffer,
        layer_surface: Option<CapturedLayerSurfaceCommitState>,
    ) -> bool {
        if !self.is_cursor_surface(surface_id) {
            let pending_surface_size = pending.surface_size.or_else(|| {
                BufferSize::new(pending.data.width().ok()?, pending.data.height().ok()?)
            });
            if let Some(layer_surface) = layer_surface {
                if !self.layer_surface_can_publish_buffer(
                    surface_id,
                    pending_surface_size,
                    layer_surface,
                ) {
                    return false;
                }
            } else if self.layer_surfaces.contains_key(&surface_id) {
                return false;
            }
            if self.xdg_surface_is_configured(surface_id) {
                self.mark_xdg_buffer_commit(surface_id);
            }
        }
        true
    }

    pub(super) fn prepare_surface_tree_buffer_for_publication(
        &mut self,
        surface_id: u32,
        pending: &mut PendingSurfaceBuffer,
        frame_callbacks: Vec<wl_callback::WlCallback>,
        window_geometry: Option<XdgWindowGeometry>,
    ) -> Vec<wl_callback::WlCallback> {
        let callbacks = frame_callbacks;
        self.finalize_pending_buffer_resize_capture(surface_id, pending, window_geometry);
        callbacks
    }

    pub(in crate::compositor) fn commit_surface_buffer(
        &mut self,
        surface_id: u32,
        pending: PendingSurfaceBuffer,
        damage: RenderableSurfaceDamage,
        window_geometry: Option<XdgWindowGeometry>,
        source: SurfacePublicationSource,
    ) -> bool {
        // Every call carries a wl_surface buffer attachment, even when the
        // client reattaches the same wl_buffer. Native presentation therefore
        // treats this boundary as new content rather than metadata-only work.
        let resize_commit = pending.resize_commit.as_deref().copied();
        let commit_sequence = pending.commit_sequence;
        let generation = self.next_render_generation_value();
        self.note_explicit_commit_visual_generation(
            SurfaceCommitId::from_sequence(commit_sequence),
            generation,
        );
        let pointer_hit_generation_before_publication = self.pointer_hit_generation;
        let previous_placement = self.surface_placement(surface_id);
        let xdg_geometry_publication = self.capture_xdg_geometry_before_surface_commit(surface_id);
        let resize_placement = match self.take_pending_resize_commit_placement(surface_id, &pending)
        {
            Ok(placement) => placement,
            Err(_) => {
                self.release_unmaterialized_pending_buffer(pending, false);
                return false;
            }
        };
        let mut placement = resize_placement.unwrap_or_else(|| self.surface_placement(surface_id));
        let root_surface_id = self.root_surface_id_for_surface(surface_id);
        client_pacing_log(
            "visual_generation_queued",
            &[
                ("surface", surface_id.to_string()),
                ("root", root_surface_id.to_string()),
                (
                    "client",
                    format!("{:?}", self.surface_client_ids.get(&surface_id)),
                ),
                ("commit_sequence", commit_sequence.0.to_string()),
                ("buffer", format!("{:?}", pending.resource.id())),
                ("buffer_id", pending.data.buffer_id().get().to_string()),
                ("damage", (!damage.is_empty()).to_string()),
                ("render_generation", generation.to_string()),
                ("source", format!("{source:?}")),
            ],
        );
        if let Some(resize) = resize_commit
            && resize.resizing
            && self
                .active_toplevel_resizes
                .get(&root_surface_id)
                .is_some_and(|active| active.interaction_id == resize.interaction_id)
        {
            placement = self.surface_placement(surface_id);
        }
        self.store_surface_placement(surface_id, placement);
        let buffer_width = match pending.data.width() {
            Ok(width) => width,
            Err(_) => {
                self.release_unmaterialized_pending_buffer(pending, false);
                return false;
            }
        };
        let buffer_height = match pending.data.height() {
            Ok(height) => height,
            Err(_) => {
                self.release_unmaterialized_pending_buffer(pending, false);
                return false;
            }
        };
        let Some(buffer_size) = BufferSize::new(buffer_width, buffer_height) else {
            self.release_unmaterialized_pending_buffer(pending, false);
            return false;
        };
        let surface_size = pending.surface_size.unwrap_or(buffer_size);
        let width = surface_size.width;
        let height = surface_size.height;
        let renderable_index = self.content_renderable_surface_index(surface_id);
        let existing_surface =
            renderable_index.and_then(|index| self.renderable_surfaces.get(index));
        let surface_was_renderable = existing_surface.is_some();
        let buffer_identity_changed =
            existing_surface.is_some_and(|surface| surface.buffer_id() != pending.data.buffer_id());
        let visual_mapping_changed = existing_surface.is_none_or(|surface| {
            surface.buffer_size() != buffer_size
                || surface.x != pending.x
                || surface.y != pending.y
                || surface.width != width
                || surface.height != height
                || surface.placement != placement
                || surface.buffer_scale != pending.buffer_scale
                || surface.buffer_transform != pending.buffer_transform
                || surface.viewport_source != pending.viewport_source
                || surface.viewport_destination != pending.viewport_destination
        });
        if buffer_identity_changed {
            self.compliance_metrics
                .note_surface_commit_buffer_rotation();
        }
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: commit surface={surface_id} wl_buffer={} buffer_id={} buffer={}x{} surface={}x{} offset={},{} shm={} dmabuf={} dmabuf_layout={:?} commit_resize_serial={:?} pending_resize={:?} window_geometry_request={:?} effective_window_geometry={:?}",
                pending.resource.id().protocol_id(),
                pending.data.buffer_id().get(),
                buffer_width,
                buffer_height,
                width,
                height,
                pending.x,
                pending.y,
                pending.data.is_shm(),
                pending.data.is_dmabuf(),
                pending.data.dmabuf_handle(),
                pending.resize_commit.as_deref().map(|resize| resize.serial),
                resize_commit.map(|resize| resize.serial),
                window_geometry,
                self.effective_xdg_window_geometry(surface_id)
                    .map(|geometry| geometry.geometry),
            );
        }
        if self.popup_surfaces.contains_key(&surface_id) && !self.popup_node_is_alive(surface_id) {
            self.release_unmaterialized_pending_buffer(pending, false);
            self.record_surface_publication(
                surface_id,
                root_surface_id,
                commit_sequence,
                None,
                source,
                None,
            );
            return false;
        }
        if let Some(root_surface_id) = self.minimized_root_surface_id_for_surface(surface_id) {
            let damage = damage.normalized_for_surface(buffer_width, buffer_height);
            let previous_content = self
                .toplevel_window_state(root_surface_id)
                .and_then(|window| window.minimized_surface(surface_id))
                .map(|surface| surface.buffer.clone());
            let copy_started = std::time::Instant::now();
            let (materialized, shm_release) =
                match pending.materialize_for_publication(previous_content.as_ref(), &damage) {
                    Ok(materialized) => materialized,
                    Err(_) => {
                        self.note_shm_materialization_failure(&pending);
                        self.release_unmaterialized_pending_buffer(pending, false);
                        return false;
                    }
                };
            let copy_to_release_us = copy_started.elapsed().as_micros() as u64;
            if self
                .commit_minimized_surface_buffer(
                    root_surface_id,
                    surface_id,
                    &materialized,
                    buffer_size,
                    width,
                    height,
                    placement,
                    generation,
                    damage.clone(),
                )
                .is_err()
            {
                if let Some(release) = shm_release {
                    self.release_materialized_shm(release, copy_to_release_us);
                }
                return false;
            }
            self.track_committed_buffer_lifetime(surface_id, &pending);
            self.replace_current_surface_buffer(
                surface_id,
                CurrentSurfaceBuffer::Materialized(materialized),
            );
            if let Some(release) = shm_release {
                self.release_materialized_shm(release, copy_to_release_us);
            }
            self.record_surface_publication(
                surface_id,
                root_surface_id,
                commit_sequence,
                Some(self.current_surface_buffers[&surface_id].buffer_id()),
                source,
                Some(BufferSize { width, height }),
            );
            self.record_surface_damage_commit_at(
                surface_id,
                Some(commit_sequence),
                damage,
                buffer_size.width,
                buffer_size.height,
            );
            if let Some((xdg_root, before)) = xdg_geometry_publication {
                self.publish_xdg_geometry_after_surface_commit(
                    xdg_root,
                    surface_id,
                    before,
                    window_geometry,
                    commit_sequence.get(),
                );
            }
            if let Some(resize_commit) = resize_commit {
                self.complete_applied_resize_transaction(surface_id, resize_commit);
            }
            if let Some(surface) = self.surface_resource_by_id(surface_id) {
                self.reconcile_surface_output_membership(&surface);
            }
            return false;
        }
        let committed_damage = if !visual_mapping_changed {
            let damage = damage.normalized_for_surface(buffer_width, buffer_height);
            if damage.is_empty() {
                self.compliance_metrics
                    .note_surface_commit_empty_damage_preserved();
            } else if matches!(damage, RenderableSurfaceDamage::Partial(_)) {
                self.compliance_metrics
                    .note_surface_commit_partial_damage_preserved();
            }
            damage
        } else {
            self.compliance_metrics
                .note_surface_commit_mapping_full_promotion();
            RenderableSurfaceDamage::Full
        };
        let previous_content = existing_surface.map(|surface| surface.buffer.clone());
        let copy_started = std::time::Instant::now();
        let (materialized, shm_release) = match pending
            .materialize_for_publication(previous_content.as_ref(), &committed_damage)
        {
            Ok(materialized) => materialized,
            Err(_) => {
                self.note_shm_materialization_failure(&pending);
                self.release_unmaterialized_pending_buffer(pending, false);
                return false;
            }
        };
        let copy_to_release_us = copy_started.elapsed().as_micros() as u64;
        // Old retained pixels sharing this root must relinquish their exact
        // release tokens before new canonical content becomes frame-eligible.
        self.retire_window_exit_for_root(root_surface_id);
        let updated_surface_index = if let Some(index) = renderable_index {
            let visual_placement = {
                let Some(existing) = self.renderable_surfaces.get_mut(index) else {
                    return false;
                };
                if update_renderable_surface_buffer(
                    existing,
                    &materialized,
                    buffer_size,
                    width,
                    height,
                    placement,
                    generation,
                    resize_commit,
                    committed_damage,
                )
                .is_err()
                {
                    if let Some(release) = shm_release {
                        self.release_materialized_shm(release, copy_to_release_us);
                    }
                    return false;
                }
                existing.placement
            };
            self.store_surface_placement(surface_id, visual_placement);
            index
        } else {
            let damage = RenderableSurfaceDamage::Full;
            let surface =
                materialized.to_renderable_surface(surface_id, placement, generation, damage);
            self.append_renderable_surface(surface);
            self.renderable_surfaces.len().saturating_sub(1)
        };
        let journal_damage = self
            .renderable_surfaces
            .get(updated_surface_index)
            .map(|surface| surface.damage.clone());
        let journal_size = self
            .renderable_surfaces
            .get(updated_surface_index)
            .map(RenderableSurface::buffer_size);
        let placement_changed = previous_placement != placement;
        let surface_visual_state_changed = visual_mapping_changed;
        let stack_reorder_needed = !surface_was_renderable || placement_changed;
        if stack_reorder_needed {
            self.compliance_metrics.note_surface_commit_stack_reorder();
            self.reorder_renderable_surfaces_by_committed_stack();
        } else {
            self.compliance_metrics
                .note_surface_commit_stack_reorder_skip();
        }
        let committed_popup = self.popup_surfaces.contains_key(&surface_id);
        let popup_was_mapped = self
            .popup_nodes
            .get(&surface_id)
            .is_some_and(|node| node.mapped);
        let popup_mapping_changed = committed_popup && !popup_was_mapped;
        if committed_popup {
            if let Some(node) = self.popup_nodes.get_mut(&surface_id) {
                node.mapped = true;
            }
            if compositor_debug_surface_logging_enabled() {
                eprintln!(
                    "oblivion-one compositor: popup surface {surface_id} committed {width}x{height} at buffer offset {},{}",
                    pending.x, pending.y
                );
            }
            let popup_stack_changed = popup_mapping_changed || placement_changed;
            if popup_stack_changed {
                self.compliance_metrics
                    .note_surface_commit_popup_topology_update();
                self.refresh_active_scene_popup_view();
                self.raise_renderable_surface_tree(surface_id);
            }
            if surface_visual_state_changed {
                self.refresh_active_scene_surface_tree(root_surface_id);
            }
        }
        self.track_committed_buffer_lifetime(surface_id, &pending);
        let published_buffer_id = materialized.buffer_id();
        self.replace_current_surface_buffer(
            surface_id,
            CurrentSurfaceBuffer::Materialized(materialized),
        );
        for child_id in self.surface_transactions.applied_children_of(surface_id) {
            self.adopt_current_surface_content_for_role(child_id);
        }
        let window_geometry_changed = xdg_geometry_publication.is_some_and(|(xdg_root, before)| {
            self.publish_xdg_geometry_after_surface_commit(
                xdg_root,
                surface_id,
                before,
                window_geometry,
                commit_sequence.get(),
            )
        });
        if committed_popup && surface_was_renderable && !window_geometry_changed {
            self.compliance_metrics.note_surface_commit_geometry_noop();
        }
        let visual_state_changed = surface_visual_state_changed || window_geometry_changed;
        if let Some(release) = shm_release {
            self.release_materialized_shm(release, copy_to_release_us);
        }
        if let Some((damage, size)) = journal_damage.zip(journal_size) {
            self.record_surface_damage_commit_at(
                surface_id,
                Some(commit_sequence),
                damage,
                size.width,
                size.height,
            );
        }
        if surface_id == root_surface_id {
            self.qualify_pending_normal_restore_response(root_surface_id, commit_sequence);
            let normal_restore_resolved = self.surface_tree_generation.is_none()
                && self.try_finalize_pending_normal_restore_from_committed_state(root_surface_id);
            if surface_visual_state_changed
                && self
                    .toplevel_visual_geometries
                    .contains_key(&root_surface_id)
                && !normal_restore_resolved
                && self.surface_tree_generation.is_none()
            {
                self.update_toplevel_visual_render_assignment_after_root_commit(
                    root_surface_id,
                    commit_sequence,
                );
            }
        } else if surface_visual_state_changed
            && self
                .toplevel_visual_geometries
                .contains_key(&root_surface_id)
            && self.surface_tree_generation.is_none()
        {
            self.update_toplevel_visual_render_assignment(root_surface_id);
        }
        self.publish_surface_generation(
            surface_id,
            generation,
            RenderGenerationCause::SurfaceCommit,
        );
        self.record_surface_publication(
            surface_id,
            root_surface_id,
            commit_sequence,
            Some(published_buffer_id),
            source,
            Some(BufferSize { width, height }),
        );
        if let Some(resize_commit) = resize_commit {
            self.complete_applied_resize_transaction(surface_id, resize_commit);
        }
        let pointer_focus_refreshed = self.refresh_pointer_focus_after_geometry_change(
            window_geometry_changed,
            pointer_hit_generation_before_publication,
        );
        if committed_popup && (popup_mapping_changed || placement_changed || visual_state_changed) {
            self.compliance_metrics
                .note_surface_commit_popup_pointer_refresh();
            if !pointer_focus_refreshed {
                self.refresh_pointer_focus_at_last_position();
            }
        }
        if visual_state_changed && let Some(surface) = self.surface_resource_by_id(surface_id) {
            self.reconcile_surface_output_membership(&surface);
        }
        if !surface_was_renderable
            && surface_id == root_surface_id
            && matches!(self.surface_role(surface_id), SurfaceRole::XdgToplevel)
        {
            self.begin_window_open_animation_after_surface_tree_publication(surface_id);
        }
        true
    }
    pub(in crate::compositor) fn minimized_root_surface_id_for_surface(
        &self,
        surface_id: u32,
    ) -> Option<u32> {
        let root_surface_id = self.root_surface_id_for_surface(surface_id);
        self.toplevel_window_state(root_surface_id)
            .is_some_and(WindowState::is_minimized)
            .then_some(root_surface_id)
    }
    pub(in crate::compositor) fn commit_minimized_surface_buffer(
        &mut self,
        root_surface_id: u32,
        surface_id: u32,
        materialized: &MaterializedSurfaceBuffer,
        buffer_size: BufferSize,
        width: u32,
        height: u32,
        placement: SurfacePlacement,
        generation: u64,
        damage: RenderableSurfaceDamage,
    ) -> io::Result<()> {
        if self.remove_renderable_surface(surface_id).is_some() {
            self.invalidate_surface_origin_cache();
        }
        if self.toplevel_window_state(root_surface_id).is_none() {
            return Ok(());
        }
        if let Some(existing) = self
            .toplevel_window_state_mut(root_surface_id)
            .and_then(|window| window.minimized_surface_mut(surface_id))
        {
            update_renderable_surface_buffer(
                existing,
                materialized,
                buffer_size,
                width,
                height,
                placement,
                generation,
                None,
                damage,
            )?;
        } else {
            let surface =
                materialized.to_renderable_surface(surface_id, placement, generation, damage);
            if let Some(window) = self.toplevel_window_state_mut(root_surface_id) {
                window.push_minimized_surface(surface);
            }
        }
        Ok(())
    }
    pub(in crate::compositor) fn commit_surface_mapping_only(
        &mut self,
        surface_id: u32,
        commit_sequence: SurfaceCommitSequence,
        damage: Option<RenderableSurfaceDamage>,
        mapping: SurfaceContentMapping,
        window_geometry: Option<XdgWindowGeometry>,
    ) -> bool {
        // This path publishes committed metadata and damage without a
        // wl_surface buffer attachment, so it retains the current content.
        let Some(current) = self.current_surface_buffers.get(&surface_id).cloned() else {
            return false;
        };
        let Ok(buffer_width) = current.width() else {
            return false;
        };
        let Ok(buffer_height) = current.height() else {
            return false;
        };
        let Some(buffer_size) = BufferSize::new(buffer_width, buffer_height) else {
            return false;
        };
        let root_surface_id = self.root_surface_id_for_surface(surface_id);
        let xdg_geometry_publication = self.capture_xdg_geometry_before_surface_commit(surface_id);
        let mapping_changed = current.current_content_mapping() != Ok(mapping);
        let geometry_request_present = window_geometry.is_some();
        let pointer_hit_generation_before_publication = self.pointer_hit_generation;
        if damage.is_none() && !mapping_changed && !geometry_request_present {
            if let Some(current) = self.current_surface_buffers.get_mut(&surface_id) {
                current.update_content_mapping(mapping, commit_sequence);
            }
            return true;
        }
        let generation = self.next_render_generation_value();
        client_pacing_log(
            "visual_generation_queued",
            &[
                ("surface", surface_id.to_string()),
                (
                    "root",
                    self.root_surface_id_for_surface(surface_id).to_string(),
                ),
                (
                    "client",
                    format!("{:?}", self.surface_client_ids.get(&surface_id)),
                ),
                ("commit_sequence", commit_sequence.0.to_string()),
                ("buffer", format!("{:?}", current.resource().id())),
                ("buffer_id", current.buffer_id().get().to_string()),
                (
                    "damage",
                    damage
                        .as_ref()
                        .is_some_and(|damage| !damage.is_empty())
                        .to_string(),
                ),
                ("render_generation", generation.to_string()),
                ("source", "retained_mapping".to_string()),
            ],
        );
        let placement = self.surface_placement(surface_id);
        let damage = damage.unwrap_or(RenderableSurfaceDamage::Empty);
        let damage = if mapping_changed || geometry_request_present {
            self.compliance_metrics
                .note_surface_commit_mapping_full_promotion();
            RenderableSurfaceDamage::Full
        } else {
            damage
        };
        if let Some(current) = self.current_surface_buffers.get_mut(&surface_id) {
            current.update_content_mapping(mapping, commit_sequence);
        }
        let Some(renderable_index) = self.content_renderable_surface_index(surface_id) else {
            return false;
        };
        let Some(existing) = self.renderable_surfaces.get_mut(renderable_index) else {
            return false;
        };

        let damage = if existing.buffer_size() == buffer_size {
            damage.normalized_for_surface(buffer_width, buffer_height)
        } else {
            RenderableSurfaceDamage::Full
        };
        let resize_pending = self
            .resize_configure_flows
            .get(&surface_id)
            .is_some_and(ResizeConfigureFlow::has_in_flight);
        let surface_size = damage_only_rendered_surface_size(
            BufferSize {
                width: existing.width,
                height: existing.height,
            },
            mapping.surface_size,
            resize_pending,
        );
        let mut pointer_geometry_changed = existing.x != mapping.x
            || existing.y != mapping.y
            || existing.width != surface_size.width
            || existing.height != surface_size.height
            || existing.placement != placement;
        let mut output_geometry_changed = pointer_geometry_changed;
        let visual_mapping_changed = existing.x != mapping.x
            || existing.y != mapping.y
            || existing.width != surface_size.width
            || existing.height != surface_size.height
            || existing.placement != placement
            || existing.buffer_scale != mapping.buffer_scale
            || existing.buffer_transform != mapping.buffer_transform
            || existing.viewport_source != mapping.viewport_source
            || existing.viewport_destination != mapping.viewport_destination;
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: retained-mapping commit surface {surface_id} buffer={}x{} requested_surface={}x{} applied_surface={}x{} shm={} dmabuf={} pending_resize={:?}",
                buffer_width,
                buffer_height,
                mapping.surface_size.width,
                mapping.surface_size.height,
                surface_size.width,
                surface_size.height,
                current.is_shm(),
                current.is_dmabuf(),
                self.resize_configure_flows
                    .get(&surface_id)
                    .and_then(ResizeConfigureFlow::in_flight_serial),
            );
        }
        existing.x = mapping.x;
        existing.y = mapping.y;
        existing.width = surface_size.width;
        existing.height = surface_size.height;
        existing.placement = placement;
        existing.generation = generation;
        existing.commit_sequence = commit_sequence;
        existing.buffer_scale = mapping.buffer_scale;
        existing.buffer_transform = mapping.buffer_transform;
        existing.viewport_source = mapping.viewport_source;
        existing.viewport_destination = mapping.viewport_destination;
        existing.damage = existing.damage.clone().union(
            damage,
            existing.buffer_size().width,
            existing.buffer_size().height,
        );
        let journal_damage = existing.damage.clone();
        let journal_size = existing.buffer_size();
        self.record_surface_damage_commit_at(
            surface_id,
            Some(commit_sequence),
            journal_damage,
            journal_size.width,
            journal_size.height,
        );
        let window_geometry_changed = xdg_geometry_publication.is_some_and(|(xdg_root, before)| {
            self.publish_xdg_geometry_after_surface_commit(
                xdg_root,
                surface_id,
                before,
                window_geometry,
                commit_sequence.get(),
            )
        });
        pointer_geometry_changed |= window_geometry_changed;
        output_geometry_changed |= window_geometry_changed;
        let visual_assignment_updated = visual_mapping_changed
            && xdg_geometry_publication.is_some_and(|(xdg_root, _)| xdg_root == root_surface_id)
            && self
                .toplevel_visual_geometries
                .contains_key(&root_surface_id)
            && self.surface_tree_generation.is_none();
        if visual_assignment_updated {
            if surface_id == root_surface_id {
                self.update_toplevel_visual_render_assignment_after_root_commit(
                    root_surface_id,
                    commit_sequence,
                );
            } else {
                self.update_toplevel_visual_render_assignment(root_surface_id);
            }
        }
        self.publish_surface_generation(
            surface_id,
            generation,
            RenderGenerationCause::SurfaceDamage,
        );
        self.refresh_pointer_focus_after_geometry_change(
            pointer_geometry_changed,
            pointer_hit_generation_before_publication,
        );
        if output_geometry_changed && !visual_assignment_updated {
            self.reconcile_surface_tree_output_memberships(root_surface_id);
        }
        if matches!(self.surface_role(surface_id), SurfaceRole::Xwayland) {
            self.note_xwayland_commit_observed(
                surface_id,
                commit_sequence,
                Some(current.buffer_id()),
                Some(buffer_size),
            );
        }
        true
    }
    pub(in crate::compositor) fn commit_surface_without_buffer(
        &mut self,
        surface_id: u32,
        state: BufferlessSurfaceCommitState,
        layer_surface: Option<CapturedLayerSurfaceCommitState>,
    ) -> bool {
        let BufferlessSurfaceCommitState {
            commit_sequence,
            damage,
            mapping,
            resize_commit: captured_resize_commit,
            resize_capture_finalized,
            window_geometry,
        } = state;
        let root_surface_id = self.root_surface_id_for_surface(surface_id);
        let xdg_geometry_publication = self.capture_xdg_geometry_before_surface_commit(surface_id);
        if self.is_cursor_surface(surface_id) {
            if let Some(mapping) = mapping {
                self.commit_cursor_surface_mapping_only(
                    surface_id,
                    commit_sequence,
                    damage,
                    mapping,
                );
            }
            return true;
        }

        if let Some(layer_surface) = layer_surface
            && !self.apply_layer_surface_commit(surface_id, layer_surface)
        {
            return false;
        }
        let mut resize_commit = if resize_capture_finalized {
            captured_resize_commit
        } else {
            self.capture_acked_resize_for_surface_commit(surface_id)
        };
        let has_current_buffer = self.current_surface_buffers.contains_key(&surface_id);
        if has_current_buffer && let Some(mapping) = mapping {
            self.commit_surface_mapping_only(
                surface_id,
                commit_sequence,
                damage,
                mapping,
                window_geometry,
            );
        } else if let Some((xdg_root, before)) = xdg_geometry_publication {
            self.publish_xdg_geometry_after_surface_commit(
                xdg_root,
                surface_id,
                before,
                window_geometry,
                commit_sequence.get(),
            );
        }
        if let Some(snapshot) = resize_commit.as_mut() {
            if self.committed_xdg_geometry_is_invalid(surface_id) {
                *snapshot = snapshot.without_effective_xdg_window_geometry();
            }
            let effective_size =
                self.effective_xdg_window_geometry(surface_id)
                    .and_then(|geometry| {
                        Some(BufferSize {
                            width: u32::try_from(geometry.geometry.width).ok()?,
                            height: u32::try_from(geometry.geometry.height).ok()?,
                        })
                    });
            if let Some(effective) = self
                .effective_xdg_window_geometry(surface_id)
                .map(|geometry| geometry.geometry)
            {
                *snapshot = snapshot.with_effective_xdg_window_geometry(effective);
            }
            let committed_size = if self.committed_xdg_geometry_is_invalid(surface_id) {
                BufferSize {
                    width: 0,
                    height: 0,
                }
            } else {
                effective_size
                    .or_else(|| mapping.map(|mapping| mapping.surface_size))
                    .or_else(|| self.current_committed_surface_content_size(surface_id))
                    .unwrap_or(BufferSize {
                        width: 1,
                        height: 1,
                    })
            };
            *snapshot = snapshot.with_committed_size(committed_size.width, committed_size.height);
        }
        if let Some(resize_commit) = resize_commit {
            self.complete_pending_resize_from_current_geometry(surface_id, resize_commit);
        }
        if surface_id == root_surface_id {
            self.qualify_pending_normal_restore_response(root_surface_id, commit_sequence);
            if self.surface_tree_generation.is_none() {
                self.update_toplevel_visual_render_assignment_after_root_commit(
                    root_surface_id,
                    commit_sequence,
                );
            }
        }
        true
    }
    pub(in crate::compositor) fn commit_surface_remove_content(
        &mut self,
        surface_id: u32,
        commit_sequence: SurfaceCommitSequence,
        frame_callbacks: Vec<wl_callback::WlCallback>,
        source: SurfacePublicationSource,
    ) -> bool {
        let decision = self.surface_publication_decision(
            surface_id,
            commit_sequence,
            source.publication_context(),
        );
        if decision != SurfacePublicationDecision::Publish {
            self.record_surface_publication_rejection(
                surface_id,
                commit_sequence,
                None,
                source,
                decision,
            );
            self.complete_frame_callbacks(frame_callbacks);
            return false;
        }
        let callbacks = frame_callbacks;
        let root_surface_id = self.root_surface_id_for_surface(surface_id);
        let prepared_window_exit = surface_id == root_surface_id
            && self.surface_role(surface_id) == SurfaceRole::XdgToplevel
            && self.prepare_window_exit(root_surface_id);
        if let Some(node) = self.popup_nodes.get_mut(&surface_id) {
            node.mapped = false;
        }
        self.refresh_active_scene_popup_view();
        self.dismiss_popup_children_for_parent(surface_id);
        self.clear_current_surface_content(surface_id);
        self.hide_renderable_surface_subtree(surface_id);
        self.note_layer_surface_unmapped(surface_id);
        self.record_surface_publication(
            surface_id,
            root_surface_id,
            commit_sequence,
            None,
            source,
            None,
        );
        if prepared_window_exit {
            let _ = self.activate_prepared_window_exit(root_surface_id);
        }
        self.complete_frame_callbacks(callbacks);
        true
    }
    pub(in crate::compositor) fn clear_current_surface_content(&mut self, surface_id: u32) {
        self.remove_current_surface_buffer(surface_id);
        if let Some(buffer) = self.active_dmabuf_buffers.remove(&surface_id) {
            self.queue_dmabuf_buffer_release(buffer);
        }
    }
    pub(in crate::compositor) fn commit_unassigned_surface_buffer(
        &mut self,
        surface_id: u32,
        pending: PendingSurfaceBuffer,
        frame_callbacks: Vec<wl_callback::WlCallback>,
        source: SurfacePublicationSource,
    ) -> bool {
        let commit_sequence = pending.commit_sequence;
        let buffer_id = pending.data.buffer_id();
        let buffer_size = pending.data.width().ok().and_then(|width| {
            pending
                .data
                .height()
                .ok()
                .and_then(|height| BufferSize::new(width, height))
        });
        let root_surface_id = self.root_surface_id_for_surface(surface_id);
        if surface_tree_debug_enabled() {
            eprintln!(
                "oblivion-one compositor: surface_commit surface={surface_id} role=unassigned decision=retain_not_publish buffer_id={}",
                buffer_id.get()
            );
        }
        self.retain_renderable_surfaces(|surface| surface.surface_id != surface_id);
        self.track_committed_buffer_lifetime(surface_id, &pending);
        self.replace_current_surface_buffer(
            surface_id,
            CurrentSurfaceBuffer::Unmaterialized(pending),
        );
        for child_id in self.surface_transactions.applied_children_of(surface_id) {
            self.adopt_current_surface_content_for_role(child_id);
        }
        self.note_xwayland_buffer_ready(surface_id);
        self.note_xwayland_commit_observed(
            surface_id,
            commit_sequence,
            Some(buffer_id),
            buffer_size,
        );
        self.record_surface_publication(
            surface_id,
            root_surface_id,
            commit_sequence,
            Some(buffer_id),
            source,
            None,
        );
        self.complete_frame_callbacks(frame_callbacks);
        true
    }
    pub(in crate::compositor) fn retain_inactive_subsurface_buffer(
        &mut self,
        surface_id: u32,
        pending: PendingSurfaceBuffer,
        frame_callbacks: Vec<wl_callback::WlCallback>,
        source: SurfacePublicationSource,
    ) -> bool {
        let commit_sequence = pending.commit_sequence;
        let buffer_id = pending.data.buffer_id();
        let buffer_size = pending.data.width().ok().and_then(|width| {
            pending
                .data
                .height()
                .ok()
                .and_then(|height| BufferSize::new(width, height))
        });
        let root_surface_id = self.root_surface_id_for_surface(surface_id);
        if surface_tree_debug_enabled() {
            eprintln!(
                "oblivion-one compositor: surface_commit surface={surface_id} role=subsurface decision=retain_inactive buffer_id={}",
                buffer_id.get()
            );
        }
        self.retain_renderable_surfaces(|surface| surface.surface_id != surface_id);
        self.track_committed_buffer_lifetime(surface_id, &pending);
        self.replace_current_surface_buffer(
            surface_id,
            CurrentSurfaceBuffer::Unmaterialized(pending),
        );
        self.record_surface_publication(
            surface_id,
            root_surface_id,
            commit_sequence,
            Some(buffer_id),
            source,
            buffer_size,
        );
        self.complete_frame_callbacks(frame_callbacks);
        false
    }
    pub(in crate::compositor) fn adopt_current_surface_content_for_role(
        &mut self,
        surface_id: u32,
    ) -> bool {
        if matches!(
            self.surface_role(surface_id),
            SurfaceRole::Unassigned | SurfaceRole::Cursor | SurfaceRole::Xwayland
        ) {
            return false;
        }
        if matches!(
            self.surface_role(surface_id),
            SurfaceRole::Subsurface { .. }
        ) && !self.subsurface_can_map(surface_id)
        {
            return false;
        }
        if let Some(renderable_index) = self.renderable_surface_index(surface_id) {
            debug_assert!(
                !matches!(
                    self.surface_role(surface_id),
                    SurfaceRole::Subsurface { .. }
                ) || self.renderable_surfaces[renderable_index]
                    .placement
                    .parent_surface_id
                    .is_some()
            );
            return false;
        }
        let Some(current) = self.current_surface_buffers.get(&surface_id).cloned() else {
            return false;
        };
        let (materialized, shm_release, copy_to_release_us) = match current {
            CurrentSurfaceBuffer::Materialized(materialized) => (materialized, None, 0),
            CurrentSurfaceBuffer::Unmaterialized(pending) => {
                let copy_started = std::time::Instant::now();
                let materialized = match pending
                    .materialize_for_publication(None, &RenderableSurfaceDamage::Full)
                {
                    Ok(materialized) => materialized,
                    Err(_) => {
                        self.note_shm_materialization_failure(&pending);
                        return false;
                    }
                };
                (
                    materialized.0,
                    materialized.1,
                    copy_started.elapsed().as_micros() as u64,
                )
            }
        };
        let generation = self.next_render_generation_value();
        let placement = self.surface_placement(surface_id);
        let surface = materialized.to_renderable_surface(
            surface_id,
            placement,
            generation,
            RenderableSurfaceDamage::Full,
        );
        let buffer_size = surface.buffer_size();
        let root_surface_id = self.root_surface_id_for_surface(surface_id);
        self.retire_window_exit_for_root(root_surface_id);
        self.retain_renderable_surfaces(|existing| existing.surface_id != surface_id);
        self.append_renderable_surface(surface);
        self.current_surface_buffers
            .insert(surface_id, CurrentSurfaceBuffer::Materialized(materialized));
        if let Some(release) = shm_release {
            self.release_materialized_shm(release, copy_to_release_us);
        }
        self.record_surface_damage_commit_at(
            surface_id,
            self.current_surface_buffers
                .get(&surface_id)
                .map(CurrentSurfaceBuffer::commit_sequence),
            RenderableSurfaceDamage::Full,
            buffer_size.width,
            buffer_size.height,
        );
        self.reorder_renderable_surfaces_by_committed_stack();
        self.publish_surface_generation(
            surface_id,
            generation,
            RenderGenerationCause::SurfaceCommit,
        );
        for child_id in self.surface_transactions.applied_children_of(surface_id) {
            self.adopt_current_surface_content_for_role(child_id);
        }
        if surface_id == self.root_surface_id_for_surface(surface_id)
            && matches!(self.surface_role(surface_id), SurfaceRole::XdgToplevel)
        {
            self.begin_window_open_animation_after_surface_tree_publication(surface_id);
        }
        if surface_tree_debug_enabled() {
            eprintln!(
                "oblivion-one compositor: surface_adopt surface={surface_id} had_buffer=true removed_root_node=false transactions_rekeyed=0"
            );
        }
        true
    }
    pub(in crate::compositor) fn publish_admitted_surface_buffer(
        &mut self,
        surface_id: u32,
        pending: PendingSurfaceBuffer,
        damage: RenderableSurfaceDamage,
        frame_callbacks: Vec<wl_callback::WlCallback>,
        presentation_feedbacks: Vec<PendingPresentationFeedback>,
        source: SurfacePublicationSource,
        window_geometry: Option<XdgWindowGeometry>,
    ) -> bool {
        let commit_sequence = pending.commit_sequence;
        let (accepted, activated) = match self.surface_role(surface_id) {
            SurfaceRole::Cursor => (
                true,
                self.commit_cursor_surface_buffer(surface_id, pending, damage, frame_callbacks),
            ),
            SurfaceRole::DragIcon if self.active_drag_icon_surface_id() == Some(surface_id) => self
                .commit_publishable_surface_buffer(
                    surface_id,
                    pending,
                    damage,
                    frame_callbacks,
                    source,
                    window_geometry,
                ),
            SurfaceRole::Unassigned | SurfaceRole::DragIcon => (
                true,
                self.commit_unassigned_surface_buffer(surface_id, pending, frame_callbacks, source),
            ),
            SurfaceRole::Xwayland => (
                true,
                self.commit_xwayland_surface_buffer(surface_id, pending, frame_callbacks, source),
            ),
            SurfaceRole::XdgToplevel
            | SurfaceRole::XdgPopup
            | SurfaceRole::LayerSurface
            | SurfaceRole::Subsurface { .. } => self.commit_publishable_surface_buffer(
                surface_id,
                pending,
                damage,
                frame_callbacks,
                source,
                window_geometry,
            ),
        };
        if accepted && matches!(self.surface_role(surface_id), SurfaceRole::Cursor) {
            let current = self.current_surface_buffers.get(&surface_id);
            let size = current.and_then(|buffer| {
                buffer
                    .width()
                    .ok()
                    .zip(buffer.height().ok())
                    .and_then(|(width, height)| BufferSize::new(width, height))
            });
            self.record_surface_publication(
                surface_id,
                self.root_surface_id_for_surface(surface_id),
                current
                    .map(CurrentSurfaceBuffer::commit_sequence)
                    .unwrap_or(SurfaceCommitSequence::initial()),
                current.map(CurrentSurfaceBuffer::buffer_id),
                source,
                size,
            );
        }
        if activated && !self.subsurface_content_is_inactive(surface_id) {
            if let Some(surface_generation) = self
                .surface_presentation_generations
                .get(&surface_id)
                .copied()
            {
                self.activate_surface_presentation_commit(
                    surface_id,
                    surface_generation,
                    commit_sequence,
                    presentation_feedbacks,
                );
            } else {
                self.discard_presentation_feedbacks(presentation_feedbacks);
            }
        } else {
            self.discard_presentation_feedbacks(presentation_feedbacks);
        }
        accepted
    }
    fn commit_publishable_surface_buffer(
        &mut self,
        surface_id: u32,
        pending: PendingSurfaceBuffer,
        damage: RenderableSurfaceDamage,
        frame_callbacks: Vec<wl_callback::WlCallback>,
        source: SurfacePublicationSource,
        window_geometry: Option<XdgWindowGeometry>,
    ) -> (bool, bool) {
        let commit_sequence = pending.commit_sequence;
        let buffer_id = pending.data.buffer_id();
        match self.surface_publication_decision(
            surface_id,
            commit_sequence,
            source.publication_context(),
        ) {
            SurfacePublicationDecision::Publish => {
                if self.subsurface_content_is_inactive(surface_id) {
                    self.retain_inactive_subsurface_buffer(
                        surface_id,
                        pending,
                        frame_callbacks,
                        source,
                    );
                    (true, false)
                } else {
                    let activated = self.commit_surface_buffer(
                        surface_id,
                        pending,
                        damage,
                        window_geometry,
                        source,
                    );
                    self.note_layer_surface_mapped(surface_id);
                    self.queue_frame_callbacks_for_surface(surface_id, frame_callbacks);
                    (true, activated)
                }
            }
            decision => {
                self.record_surface_publication_rejection(
                    surface_id,
                    commit_sequence,
                    Some(buffer_id),
                    source,
                    decision,
                );
                self.release_pending_surface_buffer(pending);
                self.complete_frame_callbacks(frame_callbacks);
                (false, false)
            }
        }
    }
    pub(in crate::compositor::state) fn refresh_pointer_focus_after_geometry_change(
        &mut self,
        geometry_changed: bool,
        pointer_hit_generation_before_publication: u64,
    ) -> bool {
        if !geometry_changed {
            return false;
        }
        if self.pointer_hit_generation == pointer_hit_generation_before_publication {
            self.advance_pointer_hit_generation();
        }
        self.refresh_pointer_focus_at_last_position();
        true
    }
}
