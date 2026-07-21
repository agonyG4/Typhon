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
        self.invalidate_surface_origin_cache();
        if placement == SurfacePlacement::root() {
            self.surface_placements.remove(&surface_id);
        } else {
            self.surface_placements.insert(surface_id, placement);
        }
    }
}
