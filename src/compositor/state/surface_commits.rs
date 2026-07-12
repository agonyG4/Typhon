#![allow(clippy::too_many_arguments)]

use super::*;

impl CompositorState {
    pub(in crate::compositor) fn commit_surface_buffer(
        &mut self,
        surface_id: u32,
        pending: PendingSurfaceBuffer,
        damage: RenderableSurfaceDamage,
        window_geometry: Option<XdgWindowGeometry>,
        source: SurfacePublicationSource,
    ) {
        let resize_commit = pending.resize_commit.as_deref().copied();
        let commit_sequence = pending.commit_sequence;
        if let Some(surface) = self.surface_resource_by_id(surface_id) {
            self.ensure_surface_entered_outputs(&surface);
        }

        let generation = self.next_render_generation_value();
        self.note_explicit_commit_visual_generation(
            SurfaceCommitId::from_sequence(commit_sequence),
            generation,
        );
        self.apply_committed_window_geometry(surface_id, window_geometry);
        let resize_placement = match self.take_pending_resize_commit_placement(surface_id, &pending)
        {
            Ok(placement) => placement,
            Err(_) => return,
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
            Err(_) => return,
        };
        let buffer_height = match pending.data.height() {
            Ok(height) => height,
            Err(_) => return,
        };
        let Some(buffer_size) = BufferSize::new(buffer_width, buffer_height) else {
            return;
        };
        let surface_size = pending.surface_size.unwrap_or(buffer_size);
        let width = surface_size.width;
        let height = surface_size.height;
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: commit surface={surface_id} wl_buffer={} buffer_id={} buffer={}x{} surface={}x{} offset={},{} shm={} dmabuf={} dmabuf_layout={:?} commit_resize_serial={:?} pending_resize={:?} window_geometry={:?}",
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
                window_geometry
                    .as_ref()
                    .or_else(|| self.surface_window_geometries.get(&surface_id)),
            );
        }
        if self.popup_surfaces.contains_key(&surface_id) && !self.popup_node_is_alive(surface_id) {
            pending.release_target().release();
            self.record_surface_publication(
                surface_id,
                root_surface_id,
                commit_sequence,
                None,
                source,
                None,
            );
            return;
        }
        if let Some(root_surface_id) = self.minimized_root_surface_id_for_surface(surface_id) {
            let damage = damage.normalized_for_surface(buffer_width, buffer_height);
            if self
                .commit_minimized_surface_buffer(
                    root_surface_id,
                    surface_id,
                    &pending,
                    buffer_size,
                    width,
                    height,
                    placement,
                    generation,
                    damage.clone(),
                )
                .is_err()
            {
                return;
            }
            self.track_committed_buffer_lifetime(surface_id, &pending);
            self.current_surface_buffers.insert(surface_id, pending);
            self.record_surface_publication(
                surface_id,
                root_surface_id,
                commit_sequence,
                Some(self.current_surface_buffers[&surface_id].data.buffer_id()),
                source,
                Some(BufferSize { width, height }),
            );
            self.record_surface_damage_commit(
                surface_id,
                damage,
                buffer_size.width,
                buffer_size.height,
            );
            if self
                .toplevel_visual_geometries
                .contains_key(&root_surface_id)
            {
                self.update_toplevel_visual_render_assignment(root_surface_id);
            }
            if let Some(resize_commit) = resize_commit {
                self.complete_applied_resize_transaction(surface_id, resize_commit);
            }
            return;
        }
        if let Some(existing) = self
            .renderable_surfaces
            .iter_mut()
            .find(|surface| surface.surface_id == surface_id)
        {
            let damage = if existing.buffer_size() == buffer_size
                && existing.buffer_id() == pending.data.buffer_id()
            {
                damage.normalized_for_surface(buffer_width, buffer_height)
            } else {
                RenderableSurfaceDamage::Full
            };
            if update_renderable_surface_buffer(
                existing,
                &pending,
                buffer_size,
                width,
                height,
                placement,
                generation,
                resize_commit,
                damage,
            )
            .is_err()
            {
                return;
            }
            let visual_placement = existing.placement;
            self.store_surface_placement(surface_id, visual_placement);
        } else {
            let damage = RenderableSurfaceDamage::Full;
            let surface =
                match pending.to_renderable_surface(surface_id, placement, generation, damage) {
                    Ok(surface) => surface,
                    Err(_) => return,
                };
            self.renderable_surfaces.push(surface);
        }
        if window_geometry.is_some()
            && self.update_popup_surface_placement_from_committed_state(surface_id)
        {
            let placement = self.surface_placement(surface_id);
            if let Some(surface) = self
                .renderable_surfaces
                .iter_mut()
                .find(|surface| surface.surface_id == surface_id)
            {
                surface.placement = placement;
            }
            self.invalidate_surface_origin_cache();
        }
        self.reorder_renderable_surfaces_by_committed_stack();
        if self
            .toplevel_visual_geometries
            .contains_key(&root_surface_id)
        {
            self.update_toplevel_visual_render_assignment(root_surface_id);
        }

        let committed_popup = self.popup_surfaces.contains_key(&surface_id);
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
            self.raise_renderable_surface_tree(surface_id);
        }

        self.track_committed_buffer_lifetime(surface_id, &pending);
        let published_buffer_id = pending.data.buffer_id();
        self.current_surface_buffers.insert(surface_id, pending);
        if let Some(surface) = self
            .renderable_surfaces
            .iter()
            .find(|surface| surface.surface_id == surface_id)
        {
            let damage = surface.damage.clone();
            let size = surface.buffer_size();
            self.record_surface_damage_commit(surface_id, damage, size.width, size.height);
        }
        self.set_render_generation(generation, RenderGenerationCause::SurfaceCommit);
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
        if committed_popup {
            self.refresh_pointer_focus_at_last_position();
        }
    }

    pub(in crate::compositor) fn minimized_root_surface_id_for_surface(
        &self,
        surface_id: u32,
    ) -> Option<u32> {
        let root_surface_id = self.root_surface_id_for_surface(surface_id);
        self.toplevel_surfaces
            .get(&root_surface_id)
            .is_some_and(|toplevel| toplevel.window.is_minimized())
            .then_some(root_surface_id)
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::compositor) fn commit_minimized_surface_buffer(
        &mut self,
        root_surface_id: u32,
        surface_id: u32,
        pending: &PendingSurfaceBuffer,
        buffer_size: BufferSize,
        width: u32,
        height: u32,
        placement: SurfacePlacement,
        generation: u64,
        damage: RenderableSurfaceDamage,
    ) -> io::Result<()> {
        let renderable_count = self.renderable_surfaces.len();
        self.renderable_surfaces
            .retain(|surface| surface.surface_id != surface_id);
        if self.renderable_surfaces.len() != renderable_count {
            self.invalidate_surface_origin_cache();
        }
        let Some(toplevel) = self.toplevel_surfaces.get_mut(&root_surface_id) else {
            return Ok(());
        };
        if let Some(existing) = toplevel.window.minimized_surface_mut(surface_id) {
            update_renderable_surface_buffer(
                existing,
                pending,
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
                pending.to_renderable_surface(surface_id, placement, generation, damage)?;
            toplevel.window.push_minimized_surface(surface);
        }
        Ok(())
    }

    pub(in crate::compositor) fn commit_surface_damage_only(
        &mut self,
        surface_id: u32,
        commit_sequence: SurfaceCommitSequence,
        damage: RenderableSurfaceDamage,
        surface_size: Option<BufferSize>,
        buffer_scale: u32,
        window_geometry: Option<XdgWindowGeometry>,
    ) -> bool {
        let Some(current) = self.current_surface_buffers.get(&surface_id).cloned() else {
            return false;
        };
        let Ok(buffer_width) = current.data.width() else {
            return false;
        };
        let Ok(buffer_height) = current.data.height() else {
            return false;
        };
        let Some(buffer_size) = BufferSize::new(buffer_width, buffer_height) else {
            return false;
        };
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
                ("buffer", format!("{:?}", current.resource.id())),
                ("buffer_id", current.data.buffer_id().get().to_string()),
                ("damage", (!damage.is_empty()).to_string()),
                ("render_generation", generation.to_string()),
                ("source", "damage_only".to_string()),
            ],
        );
        self.apply_committed_window_geometry(surface_id, window_geometry);
        let Some(existing) = self
            .renderable_surfaces
            .iter_mut()
            .find(|surface| surface.surface_id == surface_id)
        else {
            return false;
        };

        let damage = if existing.buffer_size() == buffer_size {
            damage.normalized_for_surface(buffer_width, buffer_height)
        } else {
            RenderableSurfaceDamage::Full
        };
        if current.data.is_shm()
            && existing.buffer_size() == buffer_size
            && let Some(pixels) = existing.shm_pixels_mut()
            && current
                .data
                .read_pixels_into_with_damage(pixels, &damage)
                .is_err()
        {
            return false;
        }

        let requested_surface_size = match current.surface_size_for_state(
            SurfaceViewportCommit {
                source: current.viewport_source,
                destination: surface_size,
            },
            buffer_scale,
        ) {
            Ok(surface_size) => surface_size,
            Err(_) => buffer_size,
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
            requested_surface_size,
            resize_pending,
        );
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: damage-only commit surface {surface_id} buffer={}x{} requested_surface={}x{} applied_surface={}x{} shm={} dmabuf={} pending_resize={:?}",
                buffer_width,
                buffer_height,
                requested_surface_size.width,
                requested_surface_size.height,
                surface_size.width,
                surface_size.height,
                current.data.is_shm(),
                current.data.is_dmabuf(),
                self.resize_configure_flows
                    .get(&surface_id)
                    .and_then(ResizeConfigureFlow::in_flight_serial),
            );
        }
        existing.x = current.x;
        existing.y = current.y;
        existing.width = surface_size.width;
        existing.height = surface_size.height;
        existing.generation = generation;
        existing.damage = existing.damage.clone().union(
            damage,
            existing.buffer_size().width,
            existing.buffer_size().height,
        );
        let journal_damage = existing.damage.clone();
        let journal_size = existing.buffer_size();
        self.record_surface_damage_commit(
            surface_id,
            journal_damage,
            journal_size.width,
            journal_size.height,
        );
        self.set_render_generation(generation, RenderGenerationCause::SurfaceDamage);
        let root_surface_id = self.root_surface_id_for_surface(surface_id);
        if self
            .toplevel_visual_geometries
            .contains_key(&root_surface_id)
        {
            self.update_toplevel_visual_render_assignment(root_surface_id);
        }
        true
    }

    pub(in crate::compositor) fn apply_committed_window_geometry(
        &mut self,
        surface_id: u32,
        window_geometry: Option<XdgWindowGeometry>,
    ) -> bool {
        let Some(window_geometry) = window_geometry else {
            return false;
        };
        let changed = self
            .surface_window_geometries
            .insert(surface_id, window_geometry)
            != Some(window_geometry);
        if self.toplevel_surfaces.contains_key(&surface_id) {
            self.update_toplevel_visual_render_assignment(surface_id);
        }
        self.update_popup_surface_placement_from_committed_state(surface_id);
        if let Some(positioner) = self
            .popup_surfaces
            .get(&surface_id)
            .map(|popup| popup.positioner)
            && positioner.reactive
            && self.configured_xdg_surfaces.contains(&surface_id)
        {
            self.configure_popup_surface(surface_id, positioner, None);
        }
        let child_popups = self
            .popup_surfaces
            .iter()
            .filter_map(|(popup_surface_id, popup)| {
                (popup.parent_surface_id == Some(surface_id)
                    && popup.positioner.reactive
                    && self.configured_xdg_surfaces.contains(popup_surface_id))
                .then_some((*popup_surface_id, popup.positioner))
            })
            .collect::<Vec<_>>();
        for (popup_surface_id, positioner) in child_popups {
            self.configure_popup_surface(popup_surface_id, positioner, None);
        }
        changed
    }

    pub(in crate::compositor) fn commit_surface_request_with_captured_sync(
        &mut self,
        surface_id: u32,
        surface_commit_id: SurfaceCommitId,
        commit_sequence: SurfaceCommitSequence,
        source: SurfacePublicationSource,
        mut pending: PendingSurfaceBuffer,
        damage: RenderableSurfaceDamage,
        frame_callbacks: Vec<wl_callback::WlCallback>,
        explicit_sync: Option<CapturedExplicitSyncState>,
        window_geometry: Option<XdgWindowGeometry>,
    ) {
        pending.commit_sequence = commit_sequence;
        if !self.is_cursor_surface(surface_id) {
            let pending_surface_size = pending.surface_size.or_else(|| {
                BufferSize::new(pending.data.width().ok()?, pending.data.height().ok()?)
            });
            if !self.layer_surface_can_publish_buffer(surface_id, pending_surface_size) {
                pending.release_target().release();
                self.complete_frame_callbacks(frame_callbacks);
                return;
            }
            self.configure_xdg_surface_if_needed(surface_id);
        }
        let Some(CapturedExplicitSyncState {
            state: sync_state,
            acquire,
            release,
        }) = explicit_sync
        else {
            let mut callbacks =
                self.supersede_older_pending_attachments_for_surface(surface_id, commit_sequence);
            callbacks.extend(self.cancel_pending_acquire_commits_for_surface(
                surface_id,
                AcquireWatchCancelReason::Superseded,
            ));
            callbacks.extend(frame_callbacks);
            self.finalize_pending_buffer_resize_capture(surface_id, &mut pending, window_geometry);
            self.commit_surface_buffer_by_role(
                surface_id,
                pending,
                damage,
                callbacks,
                source,
                window_geometry,
            );
            return;
        };

        if !pending.data.is_dmabuf() {
            sync_state.post_error(
                SYNCOBJ_SURFACE_ERROR_UNSUPPORTED_BUFFER,
                "explicit sync is only supported for linux-dmabuf buffers",
            );
            return;
        }

        let Some(acquire) = acquire else {
            sync_state.post_error(
                SYNCOBJ_SURFACE_ERROR_NO_ACQUIRE_POINT,
                "dmabuf commit is missing an acquire timeline point",
            );
            return;
        };
        let Some(release) = release else {
            sync_state.post_error(
                SYNCOBJ_SURFACE_ERROR_NO_RELEASE_POINT,
                "dmabuf commit is missing a release timeline point",
            );
            return;
        };

        if acquire.timeline.same_timeline(&release.timeline) && acquire.point >= release.point {
            sync_state.post_error(
                SYNCOBJ_SURFACE_ERROR_CONFLICTING_POINTS,
                "acquire timeline point must be lower than release point on the same timeline",
            );
            return;
        }

        pending.explicit_release = Some(release);
        let acquire_ready = acquire.is_signaled();
        if acquire_ready {
            self.note_explicit_commit_ready(surface_commit_id);
            let mut callbacks =
                self.supersede_older_pending_attachments_for_surface(surface_id, commit_sequence);
            callbacks.extend(self.cancel_pending_acquire_commits_for_surface(
                surface_id,
                AcquireWatchCancelReason::Superseded,
            ));
            callbacks.extend(frame_callbacks);
            self.finalize_pending_buffer_resize_capture(surface_id, &mut pending, window_geometry);
            self.commit_surface_buffer_by_role(
                surface_id,
                pending,
                damage,
                callbacks,
                source,
                window_geometry,
            );
            return;
        }

        let Some(commit_id) = self.acquire_commit_ids.allocate() else {
            sync_state.post_error(
                SYNCOBJ_SURFACE_ERROR_NO_ACQUIRE_POINT,
                "explicit sync commit identity space exhausted",
            );
            return;
        };
        let mut callbacks =
            self.retain_oldest_pending_acquire_for_surface(surface_id, surface_commit_id);
        callbacks.extend(frame_callbacks);
        self.finalize_pending_buffer_resize_capture(surface_id, &mut pending, window_geometry);
        let buffer_id = pending.resource.id().protocol_id();
        let received_at = Instant::now();
        client_pacing_log(
            "acquire_wait_queued",
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
                ("acquire_commit_id", commit_id.get().to_string()),
                ("buffer", buffer_id.to_string()),
            ],
        );
        let callback_count = callbacks.len();
        self.pending_explicit_sync_commits
            .push(PendingExplicitSyncCommit {
                surface_commit_id,
                commit_id,
                surface_id,
                commit_sequence,
                pending,
                damage,
                window_geometry,
                frame_callbacks: callbacks,
                acquire: acquire.clone(),
                acquire_state: PendingAcquireState::RegistrationPending,
            });
        self.note_explicit_commit_acquire_wait(surface_commit_id, callback_count);
        self.resize_flow_metrics.commits_delayed_by_explicit_sync = self
            .resize_flow_metrics
            .commits_delayed_by_explicit_sync
            .saturating_add(1);
        self.resize_flow_metrics.max_pending_explicit_sync_commits = self
            .resize_flow_metrics
            .max_pending_explicit_sync_commits
            .max(self.pending_explicit_sync_commits.len());
        if compositor_debug_surface_logging_enabled() {
            let pending = self
                .pending_explicit_sync_commits
                .last()
                .expect("explicit-sync commit was just queued");
            eprintln!(
                "oblivion-one compositor: resize_flow surface={surface_id} decision=captured commit_generation={} commit_has_buffer=true explicit_sync=waiting acked_serial={:?} pending_explicit_sync={}",
                pending
                    .pending
                    .resize_commit
                    .as_deref()
                    .map_or(0, |snapshot| snapshot.commit_sequence),
                pending
                    .pending
                    .resize_commit
                    .as_deref()
                    .map(|snapshot| snapshot.serial),
                self.pending_explicit_sync_commits.len(),
            );
        }
        if self.external_acquire_readiness {
            self.pending_acquire_watch_changes
                .push(AcquireWatchChange::Register(AcquireWatchRequest {
                    commit_id,
                    surface_id,
                    buffer_id,
                    acquire,
                    received_at,
                }));
        }
    }

    pub(in crate::compositor) fn commit_surface_without_buffer(
        &mut self,
        surface_id: u32,
        data: &SurfaceData,
        state: BufferlessSurfaceCommitState,
    ) {
        let BufferlessSurfaceCommitState {
            commit_sequence,
            damage,
            explicit_sync,
            surface_size,
            buffer_scale,
            resize_commit: captured_resize_commit,
            resize_capture_finalized,
            window_geometry,
        } = state;
        let _ = commit_sequence;
        if let Some(sync_state) = explicit_sync {
            let (acquire, release) = sync_state.take_points();
            if acquire.is_some() || release.is_some() {
                sync_state.post_error(
                    SYNCOBJ_SURFACE_ERROR_NO_BUFFER,
                    "explicit sync points were set without an attached buffer",
                );
                return;
            }
        }

        if self.is_cursor_surface(surface_id) {
            if let Some(damage) = damage {
                self.commit_cursor_surface_damage_only(
                    surface_id,
                    damage,
                    surface_size,
                    buffer_scale,
                );
            }
            let callbacks = data.take_frame_callbacks();
            if self
                .client_cursor_render_state()
                .is_some_and(|cursor| cursor.surface.surface_id == surface_id)
            {
                self.pending_frame_callbacks.extend(callbacks);
            } else {
                self.complete_frame_callbacks(callbacks);
            }
            return;
        }

        if !self.apply_layer_surface_commit(surface_id) {
            return;
        }
        self.configure_xdg_surface_if_needed(surface_id);
        let mut resize_commit = if resize_capture_finalized {
            captured_resize_commit
        } else {
            self.capture_acked_resize_for_surface_commit(surface_id)
        };
        if let Some(snapshot) = resize_commit.as_mut() {
            if let Some(window_geometry) = window_geometry {
                *snapshot = snapshot.with_committed_window_geometry(window_geometry);
            }
            let committed_size = window_geometry
                .or_else(|| self.surface_window_geometries.get(&surface_id).copied())
                .map(|geometry| BufferSize {
                    width: geometry.width as u32,
                    height: geometry.height as u32,
                })
                .or(surface_size)
                .or_else(|| self.current_committed_surface_content_size(surface_id))
                .unwrap_or(BufferSize {
                    width: 1,
                    height: 1,
                });
            *snapshot = snapshot.with_committed_size(committed_size.width, committed_size.height);
        }
        let viewport_size_changed = surface_size.is_some_and(|surface_size| {
            self.renderable_surfaces
                .iter()
                .find(|surface| surface.surface_id == surface_id)
                .is_some_and(|surface| {
                    surface.width != surface_size.width || surface.height != surface_size.height
                })
        });
        if let Some(damage) = damage
            .or(viewport_size_changed.then_some(RenderableSurfaceDamage::Full))
            .or(window_geometry
                .is_some()
                .then_some(RenderableSurfaceDamage::Full))
        {
            self.commit_surface_damage_only(
                surface_id,
                commit_sequence,
                damage,
                surface_size,
                buffer_scale,
                window_geometry,
            );
        } else {
            self.apply_committed_window_geometry(surface_id, window_geometry);
        }
        if let Some(resize_commit) = resize_commit {
            self.complete_pending_resize_from_current_geometry(surface_id, resize_commit);
        }
        self.complete_frame_callbacks_now(data);
    }

    pub(in crate::compositor) fn complete_pending_resize_from_current_geometry(
        &mut self,
        surface_id: u32,
        resize: ResizeCommitSnapshot,
    ) -> bool {
        let committed_size = resize
            .committed_window_geometry
            .or_else(|| self.surface_window_geometries.get(&surface_id).copied())
            .map(|geometry| BufferSize {
                width: geometry.width as u32,
                height: geometry.height as u32,
            })
            .or_else(|| {
                resize
                    .committed_size
                    .map(|(width, height)| BufferSize { width, height })
            })
            .or_else(|| self.current_committed_surface_content_size(surface_id));
        let Some(committed_size) = committed_size else {
            return false;
        };
        let placement =
            resize.placement_for_committed_size(committed_size.width, committed_size.height);
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: resize commit surface={surface_id} decision=accepted reason=geometry-only serial={} requested={}x{} actual={}x{} placement={},{}",
                resize.serial,
                resize.width,
                resize.height,
                committed_size.width,
                committed_size.height,
                placement.local_x,
                placement.local_y,
            );
        }
        if !resize.resizing {
            let completes_active = self
                .active_toplevel_resizes
                .get(&surface_id)
                .is_some_and(|active| active.interaction_id == resize.interaction_id);
            if completes_active {
                self.active_toplevel_resizes.remove(&surface_id);
                self.toplevel_visual_geometries.insert(
                    surface_id,
                    ToplevelVisualGeometry {
                        placement,
                        width: committed_size.width,
                        height: committed_size.height,
                        active_resize: None,
                    },
                );
                self.update_toplevel_visual_render_assignment(surface_id);
            }
            self.store_surface_placement(surface_id, placement);
            self.advance_render_generation(RenderGenerationCause::WindowResize);
        }
        self.complete_applied_resize_transaction(surface_id, resize);
        true
    }

    pub(in crate::compositor) fn commit_surface_remove_content(
        &mut self,
        surface_id: u32,
        commit_sequence: SurfaceCommitSequence,
        frame_callbacks: Vec<wl_callback::WlCallback>,
        source: SurfacePublicationSource,
    ) {
        let decision = self.surface_publication_decision(surface_id, commit_sequence);
        if decision != SurfacePublicationDecision::Publish {
            self.record_surface_publication_rejection(
                surface_id,
                commit_sequence,
                None,
                source,
                decision,
            );
            self.complete_frame_callbacks(frame_callbacks);
            return;
        }
        let mut callbacks =
            self.supersede_older_pending_attachments_for_surface(surface_id, commit_sequence);
        callbacks.extend(self.cancel_pending_acquire_commits_for_surface(
            surface_id,
            AcquireWatchCancelReason::Superseded,
        ));
        callbacks.extend(frame_callbacks);
        let root_surface_id = self.root_surface_id_for_surface(surface_id);
        if let Some(node) = self.popup_nodes.get_mut(&surface_id) {
            node.mapped = false;
        }
        self.dismiss_popup_children_for_parent(surface_id);
        self.unmap_surface_content(surface_id);
        self.note_layer_surface_unmapped(surface_id);
        self.record_surface_publication(
            surface_id,
            root_surface_id,
            commit_sequence,
            None,
            source,
            None,
        );
        self.complete_frame_callbacks(callbacks);
    }

    pub(in crate::compositor) fn unmap_surface_content(&mut self, surface_id: u32) -> bool {
        let renderable_ids = self
            .renderable_surfaces
            .iter()
            .map(|surface| surface.surface_id)
            .collect::<Vec<_>>();
        let mut removed_surface_ids = renderable_ids
            .into_iter()
            .filter(|candidate_id| self.surface_is_descendant_of(*candidate_id, surface_id))
            .collect::<Vec<_>>();
        removed_surface_ids.sort_unstable();
        removed_surface_ids.dedup();
        if removed_surface_ids.is_empty() {
            return false;
        }

        for removed_surface_id in &removed_surface_ids {
            self.current_surface_buffers.remove(removed_surface_id);
            if let Some(buffer) = self.active_dmabuf_buffers.remove(removed_surface_id) {
                self.queue_dmabuf_buffer_release(buffer);
            }
        }
        self.clear_resize_state_for_surfaces_with_reason(
            &removed_surface_ids,
            WindowInteractionEndReason::SurfaceUnmapped,
        );
        self.renderable_surfaces
            .retain(|surface| !removed_surface_ids.contains(&surface.surface_id));
        self.clear_popup_grab_for_surface_ids(&removed_surface_ids);
        self.popup_grab_stack
            .retain(|surface_id| !removed_surface_ids.contains(surface_id));
        self.recent_input_serials
            .retain(|input| !removed_surface_ids.contains(&compositor_surface_id(&input.surface)));
        self.clear_pointer_button_state_for_removed_surfaces(
            &removed_surface_ids,
            "surface-destroyed",
        );
        if self
            .pointer_surface
            .as_ref()
            .is_some_and(|surface| removed_surface_ids.contains(&compositor_surface_id(surface)))
        {
            self.clear_pointer_focus();
        }
        if self
            .focused_surface
            .as_ref()
            .is_some_and(|surface| removed_surface_ids.contains(&compositor_surface_id(surface)))
        {
            self.focused_surface = None;
            if self.keyboard_surface.as_ref().is_some_and(|surface| {
                removed_surface_ids.contains(&compositor_surface_id(surface))
            }) {
                self.clear_keyboard_focus();
            }
            let _ = self.focus_topmost_renderable_toplevel();
        }

        self.invalidate_surface_origin_cache();
        self.advance_render_generation(RenderGenerationCause::SurfaceUnmap);
        true
    }

    pub(in crate::compositor) fn unmap_xdg_role_surfaces(&mut self, surface_id: u32) -> bool {
        let renderable_ids = self
            .renderable_surfaces
            .iter()
            .map(|surface| surface.surface_id)
            .collect::<Vec<_>>();
        let mut removed_surface_ids = renderable_ids
            .into_iter()
            .filter(|candidate_id| self.surface_is_descendant_of(*candidate_id, surface_id))
            .collect::<Vec<_>>();
        removed_surface_ids.push(surface_id);
        removed_surface_ids.sort_unstable();
        removed_surface_ids.dedup();

        let previous_renderable_count = self.renderable_surfaces.len();
        self.renderable_surfaces
            .retain(|surface| !removed_surface_ids.contains(&surface.surface_id));
        self.clear_popup_grab_for_surface_ids(&removed_surface_ids);
        self.popup_grab_stack
            .retain(|surface_id| !removed_surface_ids.contains(surface_id));
        self.recent_input_serials
            .retain(|input| !removed_surface_ids.contains(&compositor_surface_id(&input.surface)));
        self.clear_resize_state_for_surfaces_with_reason(
            &removed_surface_ids,
            WindowInteractionEndReason::SurfaceUnmapped,
        );
        self.clear_pointer_button_state_for_removed_surfaces(
            &removed_surface_ids,
            "surface-destroyed",
        );
        if self
            .pointer_surface
            .as_ref()
            .is_some_and(|surface| removed_surface_ids.contains(&compositor_surface_id(surface)))
        {
            self.clear_pointer_focus();
        }
        if self
            .focused_surface
            .as_ref()
            .is_some_and(|surface| removed_surface_ids.contains(&compositor_surface_id(surface)))
        {
            self.focused_surface = None;
            if self.keyboard_surface.as_ref().is_some_and(|surface| {
                removed_surface_ids.contains(&compositor_surface_id(surface))
            }) {
                self.clear_keyboard_focus();
            }
            let _ = self.focus_topmost_renderable_toplevel();
        }

        if self.renderable_surfaces.len() == previous_renderable_count {
            return false;
        }

        self.invalidate_surface_origin_cache();
        self.advance_render_generation(RenderGenerationCause::SurfaceUnmap);
        true
    }

    pub(in crate::compositor) fn clear_resize_state_for_surfaces(&mut self, surface_ids: &[u32]) {
        self.clear_resize_state_for_surfaces_with_reason(
            surface_ids,
            WindowInteractionEndReason::SurfaceDestroyed,
        );
    }

    pub(in crate::compositor) fn clear_resize_state_for_surfaces_with_reason(
        &mut self,
        surface_ids: &[u32],
        reason: WindowInteractionEndReason,
    ) {
        let before_flows = self.resize_configure_flows.len();
        self.resize_configure_flows
            .retain(|surface_id, _| !surface_ids.contains(surface_id));
        let removed_flows = before_flows.saturating_sub(self.resize_configure_flows.len());
        if self
            .pending_interactive_resize_update
            .is_some_and(|update| surface_ids.contains(&update.root_surface_id))
        {
            self.pending_interactive_resize_update = None;
        }
        for commit in &mut self.pending_explicit_sync_commits {
            if surface_ids.contains(&commit.surface_id) {
                commit.pending.resize_commit = None;
            }
        }
        for transaction in &mut self.pending_surface_tree_transactions {
            for (surface_id, commit) in &mut transaction.nodes {
                if !surface_ids.contains(surface_id) {
                    continue;
                }
                commit.resize_commit = None;
                if let Some(PendingSurfaceAttachment::Buffer(buffer)) = commit.attachment.as_mut() {
                    buffer.resize_commit = None;
                }
            }
        }
        let before_previews = self.active_toplevel_resizes.len();
        self.active_toplevel_resizes
            .retain(|surface_id, _| !surface_ids.contains(surface_id));
        let removed_previews = before_previews.saturating_sub(self.active_toplevel_resizes.len());
        let visual_ids = self
            .toplevel_visual_geometries
            .keys()
            .copied()
            .filter(|surface_id| surface_ids.contains(surface_id))
            .collect::<Vec<_>>();
        for surface_id in visual_ids {
            self.toplevel_visual_geometries.remove(&surface_id);
            self.clear_toplevel_visual_render_assignment(surface_id);
        }
        if self
            .window_interaction
            .is_some_and(|interaction| surface_ids.contains(&interaction.root_surface_id))
        {
            self.clear_window_interaction_state(reason);
        }
        debug_assert!(
            self.window_interaction.is_some() || self.interaction_cursor_override.is_none()
        );
        if removed_flows > 0 || removed_previews > 0 {
            self.resize_flow_metrics.resize_interactions_canceled = self
                .resize_flow_metrics
                .resize_interactions_canceled
                .saturating_add(1);
        }
    }

    pub(in crate::compositor) fn track_committed_buffer_lifetime(
        &mut self,
        surface_id: u32,
        pending: &PendingSurfaceBuffer,
    ) {
        if pending.data.is_shm() {
            if let Some(release) = self.active_dmabuf_buffers.remove(&surface_id) {
                self.queue_dmabuf_buffer_release(release);
            }
            self.queue_buffer_release(pending.resource.clone());
            return;
        }

        let new_release = pending.release_target();
        if let Some(previous) = self
            .active_dmabuf_buffers
            .insert(surface_id, new_release.clone())
            && !previous.same_buffer_resource(&new_release)
        {
            self.queue_dmabuf_buffer_release(previous);
        }
    }

    pub(in crate::compositor) fn queue_buffer_release(&mut self, buffer: wl_buffer::WlBuffer) {
        self.pending_buffer_releases.push(buffer);
    }

    pub(in crate::compositor) fn queue_dmabuf_buffer_release(
        &mut self,
        release: SurfaceBufferRelease,
    ) {
        self.pending_dmabuf_buffer_releases.push(release);
    }

    pub(in crate::compositor) fn commit_cursor_surface_buffer(
        &mut self,
        surface_id: u32,
        pending: PendingSurfaceBuffer,
        _damage: RenderableSurfaceDamage,
        frame_callbacks: Vec<wl_callback::WlCallback>,
    ) {
        self.unmap_surface_content(surface_id);
        let generation = self.next_render_generation_value();
        let damage = RenderableSurfaceDamage::Full;
        let Ok(surface) =
            pending.to_renderable_surface(surface_id, SurfacePlacement::root(), generation, damage)
        else {
            return;
        };
        let buffer_size = surface.buffer_size();
        self.track_committed_buffer_lifetime(surface_id, &pending);
        self.current_surface_buffers.insert(surface_id, pending);
        self.client_cursor_surfaces.insert(surface_id, surface);
        self.record_surface_damage_commit(
            surface_id,
            RenderableSurfaceDamage::Full,
            buffer_size.width,
            buffer_size.height,
        );
        self.set_render_generation(generation, RenderGenerationCause::CursorCommit);
        if self
            .active_client_cursor
            .as_ref()
            .is_some_and(|active| active.surface_id == surface_id)
            && self.cursor_visibility.lock_hidden_constraint_id.is_none()
        {
            self.pending_frame_callbacks.extend(frame_callbacks);
        } else {
            self.complete_frame_callbacks(frame_callbacks);
        }
    }

    pub(in crate::compositor) fn commit_unassigned_surface_buffer(
        &mut self,
        surface_id: u32,
        pending: PendingSurfaceBuffer,
        frame_callbacks: Vec<wl_callback::WlCallback>,
        source: SurfacePublicationSource,
    ) {
        let commit_sequence = pending.commit_sequence;
        let buffer_id = pending.data.buffer_id();
        let root_surface_id = self.root_surface_id_for_surface(surface_id);
        if surface_tree_debug_enabled() {
            eprintln!(
                "oblivion-one compositor: surface_commit surface={surface_id} role=unassigned decision=retain_not_publish buffer_id={}",
                buffer_id.get()
            );
        }
        self.renderable_surfaces
            .retain(|surface| surface.surface_id != surface_id);
        self.track_committed_buffer_lifetime(surface_id, &pending);
        self.current_surface_buffers.insert(surface_id, pending);
        self.record_surface_publication(
            surface_id,
            root_surface_id,
            commit_sequence,
            Some(buffer_id),
            source,
            None,
        );
        self.complete_frame_callbacks(frame_callbacks);
    }

    pub(in crate::compositor) fn adopt_current_surface_content_for_role(
        &mut self,
        surface_id: u32,
    ) -> bool {
        if matches!(
            self.surface_role(surface_id),
            SurfaceRole::Unassigned | SurfaceRole::Cursor
        ) {
            return false;
        }
        if self
            .renderable_surfaces
            .iter()
            .any(|surface| surface.surface_id == surface_id)
        {
            debug_assert!(
                !matches!(
                    self.surface_role(surface_id),
                    SurfaceRole::Subsurface { .. }
                ) || self
                    .renderable_surfaces
                    .iter()
                    .find(|surface| surface.surface_id == surface_id)
                    .is_some_and(|surface| surface.placement.parent_surface_id.is_some())
            );
            return false;
        }
        let Some(current) = self.current_surface_buffers.get(&surface_id).cloned() else {
            return false;
        };
        let generation = self.next_render_generation_value();
        let placement = self.surface_placement(surface_id);
        let Ok(surface) = current.to_renderable_surface(
            surface_id,
            placement,
            generation,
            RenderableSurfaceDamage::Full,
        ) else {
            return false;
        };
        let buffer_size = surface.buffer_size();
        self.renderable_surfaces
            .retain(|surface| surface.surface_id != surface_id);
        self.renderable_surfaces.push(surface);
        self.record_surface_damage_commit(
            surface_id,
            RenderableSurfaceDamage::Full,
            buffer_size.width,
            buffer_size.height,
        );
        self.reorder_renderable_surfaces_by_committed_stack();
        self.set_render_generation(generation, RenderGenerationCause::SurfaceCommit);
        if surface_tree_debug_enabled() {
            eprintln!(
                "oblivion-one compositor: surface_adopt surface={surface_id} had_buffer=true removed_root_node=false transactions_rekeyed=0"
            );
        }
        true
    }

    pub(in crate::compositor) fn commit_surface_buffer_by_role(
        &mut self,
        surface_id: u32,
        pending: PendingSurfaceBuffer,
        damage: RenderableSurfaceDamage,
        frame_callbacks: Vec<wl_callback::WlCallback>,
        source: SurfacePublicationSource,
        window_geometry: Option<XdgWindowGeometry>,
    ) {
        match self.surface_role(surface_id) {
            SurfaceRole::Cursor => {
                self.commit_cursor_surface_buffer(surface_id, pending, damage, frame_callbacks);
            }
            SurfaceRole::Unassigned => {
                self.commit_unassigned_surface_buffer(surface_id, pending, frame_callbacks, source);
            }
            SurfaceRole::XdgToplevel
            | SurfaceRole::XdgPopup
            | SurfaceRole::LayerSurface
            | SurfaceRole::Subsurface { .. } => {
                let commit_sequence = pending.commit_sequence;
                let buffer_id = pending.data.buffer_id();
                match self.surface_publication_decision(surface_id, commit_sequence) {
                    SurfacePublicationDecision::Publish => {
                        self.commit_surface_buffer(
                            surface_id,
                            pending,
                            damage,
                            window_geometry,
                            source,
                        );
                        self.note_layer_surface_buffer_published(surface_id);
                        self.pending_frame_callbacks.extend(frame_callbacks);
                    }
                    decision => {
                        self.record_surface_publication_rejection(
                            surface_id,
                            commit_sequence,
                            Some(buffer_id),
                            source,
                            decision,
                        );
                        pending.release_target().release();
                        self.complete_frame_callbacks(frame_callbacks);
                    }
                }
            }
        }
    }

    pub(in crate::compositor) fn commit_cursor_surface_damage_only(
        &mut self,
        surface_id: u32,
        damage: RenderableSurfaceDamage,
        surface_size: Option<BufferSize>,
        buffer_scale: u32,
    ) -> bool {
        let Some(current) = self.current_surface_buffers.get(&surface_id).cloned() else {
            return false;
        };
        let Ok(buffer_width) = current.data.width() else {
            return false;
        };
        let Ok(buffer_height) = current.data.height() else {
            return false;
        };
        let Some(buffer_size) = BufferSize::new(buffer_width, buffer_height) else {
            return false;
        };
        let generation = self.next_render_generation_value();
        let Some(existing) = self.client_cursor_surfaces.get_mut(&surface_id) else {
            return false;
        };
        let damage = if existing.buffer_size() == buffer_size {
            damage.normalized_for_surface(buffer_width, buffer_height)
        } else {
            RenderableSurfaceDamage::Full
        };
        if current.data.is_shm()
            && existing.buffer_size() == buffer_size
            && let Some(pixels) = existing.shm_pixels_mut()
            && current
                .data
                .read_pixels_into_with_damage(pixels, &damage)
                .is_err()
        {
            return false;
        }
        if let Ok(size) = current.surface_size_for_state(
            SurfaceViewportCommit {
                source: current.viewport_source,
                destination: surface_size,
            },
            buffer_scale,
        ) {
            existing.width = size.width;
            existing.height = size.height;
        }
        existing.x = current.x;
        existing.y = current.y;
        existing.generation = generation;
        existing.damage = existing.damage.clone().union(
            damage,
            existing.buffer_size().width,
            existing.buffer_size().height,
        );
        let journal_damage = existing.damage.clone();
        let journal_size = existing.buffer_size();
        self.record_surface_damage_commit(
            surface_id,
            journal_damage,
            journal_size.width,
            journal_size.height,
        );
        self.set_render_generation(generation, RenderGenerationCause::CursorCommit);
        true
    }

    pub(in crate::compositor) fn commit_cursor_surface_removal_request(
        &mut self,
        surface_id: u32,
        data: &SurfaceData,
        explicit_sync: Option<Arc<SyncobjSurfaceState>>,
    ) {
        if let Some(sync_state) = explicit_sync {
            let (acquire, release) = sync_state.take_points();
            if acquire.is_some() || release.is_some() {
                sync_state.post_error(
                    SYNCOBJ_SURFACE_ERROR_NO_BUFFER,
                    "explicit sync points were set without an attached buffer",
                );
                return;
            }
        }
        let was_visible = self
            .client_cursor_render_state()
            .is_some_and(|cursor| cursor.surface.surface_id == surface_id);
        let removed = self.client_cursor_surfaces.remove(&surface_id).is_some();
        self.current_surface_buffers.remove(&surface_id);
        if let Some(release) = self.active_dmabuf_buffers.remove(&surface_id) {
            self.queue_dmabuf_buffer_release(release);
        }
        if removed {
            self.advance_render_generation(RenderGenerationCause::CursorCommit);
            pointer_debug_log(format!(
                "cursor surface buffer removed surface={}",
                surface_id
            ));
        }
        let callbacks = data.take_frame_callbacks();
        if was_visible && removed {
            self.pending_frame_callbacks.extend(callbacks);
        } else {
            self.complete_frame_callbacks(callbacks);
        }
    }

    pub(in crate::compositor) fn surface_placement(&self, surface_id: u32) -> SurfacePlacement {
        self.surface_placements
            .get(&surface_id)
            .copied()
            .unwrap_or_default()
    }

    pub(in crate::compositor) fn store_surface_placement(
        &mut self,
        surface_id: u32,
        placement: SurfacePlacement,
    ) {
        self.invalidate_surface_origin_cache();
        if placement == SurfacePlacement::root() {
            self.surface_placements.remove(&surface_id);
        } else {
            self.surface_placements.insert(surface_id, placement);
        }
    }
}
