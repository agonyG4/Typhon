use super::*;

impl CompositorState {
    pub(in crate::compositor) fn surface_placement(&self, surface_id: u32) -> SurfacePlacement {
        self.surface_topology.placement(surface_id)
    }

    pub(in crate::compositor) fn store_surface_placement(
        &mut self,
        surface_id: u32,
        placement: SurfacePlacement,
    ) {
        if self.surface_topology.set_placement(surface_id, placement) {
            self.invalidate_surface_origin_cache();
        }
    }

    pub(in crate::compositor) fn set_surface_placement(
        &mut self,
        surface_id: u32,
        placement: SurfacePlacement,
    ) -> bool {
        self.set_surface_placement_with_cause(
            surface_id,
            placement,
            RenderGenerationCause::SurfacePlacement,
        )
    }

    pub(in crate::compositor) fn set_surface_placement_with_cause(
        &mut self,
        surface_id: u32,
        placement: SurfacePlacement,
        cause: RenderGenerationCause,
    ) -> bool {
        if self.surface_placement(surface_id) == placement {
            return false;
        }

        self.store_surface_placement(surface_id, placement);
        let root_surface_id = self.root_surface_id_for_surface(surface_id);
        if let Some(visual) = self.toplevel_visual_geometries.get_mut(&surface_id) {
            visual.placement = placement;
        }

        if let Some(surface) = self
            .renderable_surfaces
            .iter_mut()
            .find(|surface| surface.surface_id == surface_id)
        {
            surface.placement = placement;
            let refreshed_by_visual_assignment = self
                .toplevel_visual_geometries
                .contains_key(&root_surface_id)
                || self.toplevel_surfaces.contains_key(&root_surface_id);
            if refreshed_by_visual_assignment {
                self.update_toplevel_visual_render_assignment(root_surface_id);
            } else {
                self.refresh_active_scene_surface_tree(root_surface_id);
            }
            if refreshed_by_visual_assignment {
                self.compliance_metrics.prevented_duplicate_root_refreshes = self
                    .compliance_metrics
                    .prevented_duplicate_root_refreshes
                    .saturating_add(1);
            }
            self.advance_render_generation_with_scene_effect(
                cause,
                self.surface_is_visible_in_active_scene(root_surface_id),
            );
            return true;
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_storage_normalizes_only_the_exact_default_root() {
        let mut state = CompositorState::default();

        state.store_surface_placement(1, SurfacePlacement::root());
        state.store_surface_placement(2, SurfacePlacement::root_at(4, 5));
        state.store_surface_placement(3, SurfacePlacement::absolute_root_at(0, 0));

        assert_eq!(state.surface_topology.placement_entry(1), None);
        assert_eq!(
            state.surface_topology.placement_entry(2),
            Some(SurfacePlacement::root_at(4, 5))
        );
        assert_eq!(
            state.surface_topology.placement_entry(3),
            Some(SurfacePlacement::absolute_root_at(0, 0))
        );
    }

    #[test]
    fn placement_changes_invalidate_origin_cache_only_when_changed() {
        let mut state = CompositorState::default();
        let initial = SurfacePlacement::root_at(10, 20);
        state.store_surface_placement(1, initial);
        state.refresh_surface_origin_cache();
        let cached_generation = state.surface_origin_cache_generation;
        assert_eq!(cached_generation, Some(state.render_generation));

        state.store_surface_placement(1, initial);
        assert_eq!(state.surface_origin_cache_generation, cached_generation);

        state.store_surface_placement(1, SurfacePlacement::root_at(30, 40));
        assert_eq!(state.surface_origin_cache_generation, None);
    }

    #[test]
    fn placement_parent_cycles_fail_closed() {
        let mut state = CompositorState::default();
        state.store_surface_placement(1, SurfacePlacement::subsurface(2, 0, 0));
        state.store_surface_placement(2, SurfacePlacement::subsurface(1, 0, 0));

        assert_eq!(state.root_surface_id_for_surface(1), 1);
        assert!(state.surface_is_descendant_of(1, 1));
        assert!(!state.surface_is_descendant_of(1, 3));
    }

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
