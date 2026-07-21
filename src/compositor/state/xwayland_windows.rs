use super::*;
use crate::xwayland::trace::{self, TraceFields};

impl CompositorState {
    pub(in crate::compositor) fn withdraw_xwayland_surface_content(
        &mut self,
        root_surface_id: u32,
    ) -> bool {
        let withdrawn_ids = self
            .renderable_surfaces
            .iter()
            .filter_map(|surface| {
                (self.root_surface_id_for_surface(surface.surface_id) == root_surface_id)
                    .then_some(surface.surface_id)
            })
            .collect::<std::collections::HashSet<_>>();
        if withdrawn_ids.is_empty() {
            return false;
        }

        self.renderable_surfaces
            .retain(|surface| !withdrawn_ids.contains(&surface.surface_id));
        self.invalidate_surface_origin_cache();
        self.reconcile_all_surface_output_memberships();
        self.advance_render_generation(RenderGenerationCause::SurfaceUnmap);
        true
    }

    pub(in crate::compositor) fn adopt_current_xwayland_surface_content(
        &mut self,
        surface_id: u32,
    ) -> bool {
        if !matches!(self.surface_role(surface_id), SurfaceRole::Xwayland) {
            return false;
        }

        let Some(surface_generation) = self
            .xwayland
            .surface_states
            .get(&surface_id)
            .map(|state| state.generation)
        else {
            return false;
        };
        if self
            .xwayland
            .client_identity
            .as_ref()
            .is_none_or(|identity| identity.generation != surface_generation)
        {
            return false;
        }

        let Some(window_id) = self.window_id_for_surface(surface_id) else {
            return false;
        };
        if !self.window(window_id).is_some_and(|window| {
            matches!(
                window.backend,
                WindowBackend::X11(handle) if handle.generation() == surface_generation
            )
        }) {
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
            .retain(|existing| existing.surface_id != surface_id);
        self.renderable_surfaces.push(surface);
        self.record_surface_damage_commit(
            surface_id,
            RenderableSurfaceDamage::Full,
            buffer_size.width,
            buffer_size.height,
        );
        self.reorder_renderable_surfaces_by_committed_stack();
        self.set_render_generation(generation, RenderGenerationCause::SurfaceCommit);
        true
    }

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
        let association_serial = self
            .xwayland
            .associations
            .serial_for_surface(surface_id)
            .map(|(_, serial)| serial.get());
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
        trace::emit("xwayland_commit_observed", || {
            TraceFields::new()
                .field("source", "wayland")
                .field("surface_id", surface_id)
                .field("commit_sequence", commit_sequence.get())
                .field("buffer_id", buffer_id.get())
                .field("buffer_width", buffer_size.width)
                .field("buffer_height", buffer_size.height)
                .optional("association_serial", association_serial)
                .field("buffer_ready_level", true)
        });
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_surface(surface_id: u32, width: u32, height: u32) -> RenderableSurface {
        let identity = BufferIdAllocator::default()
            .allocate()
            .expect("test buffer identity");
        RenderableSurface {
            surface_id,
            x: 0,
            y: 0,
            width,
            height,
            placement: SurfacePlacement::root(),
            render_placement: None,
            visual_clip: None,
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: crate::render_backend::buffer::CommittedSurfaceBuffer::shm_snapshot(
                identity,
                BufferSize::new(width, height).expect("test size"),
                vec![0; width as usize * height as usize],
            ),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: RenderableSurfaceDamage::Full,
        }
    }

    #[test]
    fn xwayland_withdrawal_unpublishes_render_tree_without_forgetting_placement() {
        let mut state = CompositorState::default();
        let root_id = 42;
        let child_id = 43;
        state
            .renderable_surfaces
            .push(test_surface(root_id, 10, 10));
        state.renderable_surfaces.push(test_surface(child_id, 1, 1));
        assert!(state.set_surface_placement(root_id, SurfacePlacement::absolute_root_at(10, 10),));
        assert!(
            state.set_surface_placement(child_id, SurfacePlacement::subsurface(root_id, 1, 1),)
        );
        let generation = state.render_generation;

        assert!(state.withdraw_xwayland_surface_content(root_id));

        assert!(state.renderable_surfaces.is_empty());
        assert_eq!(
            state.surface_placement(root_id),
            SurfacePlacement::absolute_root_at(10, 10),
        );
        assert_eq!(
            state.surface_placement(child_id),
            SurfacePlacement::subsurface(root_id, 1, 1),
        );
        assert!(state.render_generation > generation);
        assert!(!state.withdraw_xwayland_surface_content(root_id));
    }
}
