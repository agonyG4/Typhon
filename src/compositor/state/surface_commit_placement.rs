use super::*;

impl CompositorState {
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
        if self.surface_placement(surface_id) == placement {
            return;
        }
        self.invalidate_surface_origin_cache();
        if placement == SurfacePlacement::root() {
            self.surface_placements.remove(&surface_id);
        } else {
            self.surface_placements.insert(surface_id, placement);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_effective_geometry_is_frozen_when_committed_content_grows() {
        let mut state = CompositorState::default();
        let surface_id = 7;
        let geometry = XdgWindowGeometry::new(16, 10, 64, 48);
        let identity = state.allocate_buffer_identity().expect("buffer identity");
        state.append_renderable_surface(RenderableSurface {
            surface_id,
            x: 0,
            y: 0,
            width: 80,
            height: 60,
            placement: SurfacePlacement::root(),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: crate::render_backend::buffer::CommittedSurfaceBuffer::shm_snapshot(
                identity,
                BufferSize::new(80, 60).expect("buffer size"),
                vec![0; 80 * 60],
            ),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: RenderableSurfaceDamage::Full,
        });
        state.set_test_effective_xdg_window_geometry(surface_id, geometry);

        state.renderable_surfaces[0].width = 200;
        state.renderable_surfaces[0].height = 150;

        assert_eq!(
            state.effective_xdg_window_geometry(surface_id),
            Some(EffectiveXdgWindowGeometry {
                geometry,
                source: EffectiveXdgWindowGeometrySource::ExplicitEffective,
            })
        );
    }
}
