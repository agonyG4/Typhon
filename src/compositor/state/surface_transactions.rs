use super::*;

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
