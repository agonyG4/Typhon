use super::*;

impl CompositorState {
    pub(in crate::compositor) fn commit_xwayland_surface_buffer(
        &mut self,
        surface_id: u32,
        pending: PendingSurfaceBuffer,
        frame_callbacks: Vec<wl_callback::WlCallback>,
        source: SurfacePublicationSource,
    ) {
        let root_surface_id = self.root_surface_id_for_surface(surface_id);
        if self.window_id_for_surface(root_surface_id).is_none() {
            self.commit_unassigned_surface_buffer(surface_id, pending, frame_callbacks, source);
            return;
        }
        let commit_sequence = pending.commit_sequence;
        let buffer_id = pending.data.buffer_id();
        let generation = self.next_render_generation_value();
        let placement = self.surface_placement(root_surface_id);
        let Ok(surface) = pending.to_renderable_surface(
            surface_id,
            placement,
            generation,
            RenderableSurfaceDamage::Full,
        ) else {
            pending.release_target().release();
            self.complete_frame_callbacks(frame_callbacks);
            return;
        };
        let buffer_size = surface.buffer_size();
        self.track_committed_buffer_lifetime(surface_id, &pending);
        self.current_surface_buffers.insert(surface_id, pending);
        self.renderable_surfaces
            .retain(|existing| existing.surface_id != surface_id);
        self.renderable_surfaces.push(surface);
        self.record_surface_publication(
            surface_id,
            root_surface_id,
            commit_sequence,
            Some(buffer_id),
            source,
            None,
        );
        self.record_surface_damage_commit(
            surface_id,
            RenderableSurfaceDamage::Full,
            buffer_size.width,
            buffer_size.height,
        );
        self.reorder_renderable_surfaces_by_committed_stack();
        self.set_render_generation(generation, RenderGenerationCause::SurfaceCommit);
        self.note_xwayland_buffer_ready(surface_id);
        self.complete_frame_callbacks(frame_callbacks);
    }
}
