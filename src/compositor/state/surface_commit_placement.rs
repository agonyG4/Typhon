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
    fn repeated_committed_window_geometry_is_a_derived_work_noop() {
        let mut state = CompositorState::default();
        let geometry = XdgWindowGeometry::new(16, 10, 320, 240);

        assert!(state.apply_committed_window_geometry(7, Some(geometry)));
        assert!(!state.apply_committed_window_geometry(7, Some(geometry)));
        assert_eq!(state.compliance_metrics.surface_commit_geometry_noops, 1);
    }
}
