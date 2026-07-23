use super::*;

impl CompositorState {
    pub(in crate::compositor) fn commit_cursor_surface_buffer(
        &mut self,
        surface_id: u32,
        pending: PendingSurfaceBuffer,
        _damage: RenderableSurfaceDamage,
        frame_callbacks: Vec<wl_callback::WlCallback>,
    ) {
        let commit_id = SurfaceCommitId::from_sequence(pending.commit_sequence);
        self.unmap_surface_content(surface_id);
        let generation = self.next_render_generation_value();
        let damage = RenderableSurfaceDamage::Full;
        let Ok(surface) =
            pending.to_renderable_surface(surface_id, SurfacePlacement::root(), generation, damage)
        else {
            return;
        };
        let buffer_size = surface.buffer_size();
        self.note_explicit_commit_published(commit_id);
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
            current.buffer_transform,
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
        explicit_sync: Option<Arc<SyncobjSurfaceState>>,
    ) {
        if let Some(sync_state) = explicit_sync {
            let (acquire, release) = sync_state.take_points();
            if acquire.is_some() || release.is_some() {
                sync_state.post_error_with_metrics(
                    &mut self.compliance_metrics,
                    SYNCOBJ_SURFACE_ERROR_NO_BUFFER,
                    "explicit sync points were set without an attached buffer",
                );
                return;
            }
        }
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
    }
}
