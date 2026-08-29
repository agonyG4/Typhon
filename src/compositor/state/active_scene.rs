use super::*;
use crate::wm::{SpecialWorkspaceId, WorkspaceId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) struct ActiveSceneSelection {
    pub(in crate::compositor) regular: WorkspaceId,
    pub(in crate::compositor) special: Option<SpecialWorkspaceId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(in crate::compositor) enum SceneWorkOwner {
    Global,
    Location(crate::wm::WorkspaceLocation),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::compositor) struct ActiveSceneUpdate {
    pub(in crate::compositor) selection_changed: bool,
    pub(in crate::compositor) visual_scene_changed: bool,
}

#[derive(Debug, Default)]
pub(in crate::compositor) struct ActiveSceneView {
    surfaces: Vec<RenderableSurface>,
    surface_indices: HashMap<u32, usize>,
    surface_origins: Vec<(i32, i32)>,
    popup_surface_ids: Vec<u32>,
    selection: Option<ActiveSceneSelection>,
    rebuild_count: u64,
    incremental_surface_update_count: u64,
}

impl ActiveSceneView {
    pub(in crate::compositor) fn surfaces(&self) -> &[RenderableSurface] {
        &self.surfaces
    }

    pub(in crate::compositor) fn popup_surface_ids(&self) -> &[u32] {
        &self.popup_surface_ids
    }

    pub(in crate::compositor) fn surface_origins(&self) -> &[(i32, i32)] {
        &self.surface_origins
    }

    #[cfg(test)]
    pub(in crate::compositor) const fn rebuild_count(&self) -> u64 {
        self.rebuild_count
    }

    #[cfg(test)]
    pub(in crate::compositor) const fn incremental_surface_update_count(&self) -> u64 {
        self.incremental_surface_update_count
    }
}

impl CompositorState {
    fn active_scene_renderable_surfaces(&self) -> Vec<RenderableSurface> {
        let mut surfaces = self
            .renderable_surfaces
            .iter()
            .enumerate()
            .filter(|(_, surface)| self.surface_is_visible_in_active_scene(surface.surface_id))
            .map(|(position, surface)| (position, surface.clone()))
            .collect::<Vec<_>>();
        surfaces.sort_by_key(|(position, surface)| {
            self.renderable_root_stack_key(
                self.root_surface_id_for_surface(surface.surface_id),
                *position,
            )
        });
        surfaces.into_iter().map(|(_, surface)| surface).collect()
    }

    pub(in crate::compositor) fn rebuild_active_scene_view(&mut self) -> ActiveSceneUpdate {
        let selection = self.active_scene_selection();
        let surfaces = self.active_scene_renderable_surfaces();
        let surface_indices = surfaces
            .iter()
            .enumerate()
            .map(|(index, surface)| (surface.surface_id, index))
            .collect();
        let popup_surface_ids = self.active_popup_surface_ids_from_state();
        let surface_origins = render::surface_origins(&surfaces);
        let previous_selection = self.active_scene_view.selection;
        let previous_surface_ids = self
            .active_scene_view
            .surfaces
            .iter()
            .map(|surface| surface.surface_id)
            .collect::<Vec<_>>();
        let surface_ids = surfaces
            .iter()
            .map(|surface| surface.surface_id)
            .collect::<Vec<_>>();
        let update = ActiveSceneUpdate {
            selection_changed: previous_selection != Some(selection),
            visual_scene_changed: previous_surface_ids != surface_ids
                || self.active_scene_view.popup_surface_ids != popup_surface_ids
                || self.active_scene_view.surface_origins != surface_origins,
        };
        self.active_scene_view.surfaces = surfaces;
        self.active_scene_view.surface_indices = surface_indices;
        self.active_scene_view.surface_origins = surface_origins;
        self.active_scene_view.popup_surface_ids = popup_surface_ids;
        self.active_scene_view.selection = Some(selection);
        self.active_scene_view.rebuild_count =
            self.active_scene_view.rebuild_count.saturating_add(1);
        self.advance_pointer_hit_generation();
        self.refresh_frame_work_visibility();
        self.rebuild_scene_work_index();
        update
    }

    fn active_popup_surface_ids_from_state(&self) -> Vec<u32> {
        let mut popup_surface_ids = self
            .popup_surfaces
            .keys()
            .copied()
            .filter(|surface_id| {
                self.popup_nodes.get(surface_id).is_some_and(|node| {
                    node.lifecycle == PopupLifecycle::Alive
                        && node.mapped
                        && self.surface_is_visible_in_active_scene(*surface_id)
                })
            })
            .collect::<Vec<_>>();
        popup_surface_ids.sort_unstable();
        popup_surface_ids
    }

    pub(in crate::compositor) fn refresh_active_scene_popup_view(&mut self) {
        let popup_surface_ids = self.active_popup_surface_ids_from_state();
        if popup_surface_ids != self.active_scene_view.popup_surface_ids {
            self.active_scene_view.popup_surface_ids = popup_surface_ids;
            self.advance_pointer_hit_generation();
        }
    }

    pub(in crate::compositor) fn refresh_active_scene_surface_order(&mut self) {
        let visible_ids = self
            .active_scene_renderable_surfaces()
            .into_iter()
            .map(|surface| surface.surface_id)
            .collect::<Vec<_>>();
        let cached_ids = self
            .active_scene_view
            .surfaces
            .iter()
            .map(|surface| surface.surface_id)
            .collect::<Vec<_>>();
        if visible_ids != cached_ids {
            self.rebuild_active_scene_view();
        }
    }

    pub(in crate::compositor) fn refresh_active_scene_surface(&mut self, surface_id: u32) {
        if self.active_scene_view.selection != Some(self.active_scene_selection()) {
            self.rebuild_active_scene_view();
            return;
        }

        let visible = self.surface_is_visible_in_active_scene(surface_id);
        let cached_index = self
            .active_scene_view
            .surface_indices
            .get(&surface_id)
            .copied();
        if !visible {
            if cached_index.is_some() {
                self.rebuild_active_scene_view();
            }
            return;
        }

        let Some(updated) = self.renderable_surface(surface_id).cloned() else {
            if cached_index.is_some() {
                self.rebuild_active_scene_view();
            }
            return;
        };
        if let Some(index) = cached_index {
            let origin_changed = {
                let previous = &self.active_scene_view.surfaces[index];
                previous.x != updated.x
                    || previous.y != updated.y
                    || previous.placement != updated.placement
                    || previous.render_placement != updated.render_placement
            };
            self.active_scene_view.surfaces[index] = updated;
            if origin_changed {
                self.active_scene_view.surface_origins =
                    render::surface_origins(&self.active_scene_view.surfaces);
            }
            self.active_scene_view.incremental_surface_update_count = self
                .active_scene_view
                .incremental_surface_update_count
                .saturating_add(1);
        } else {
            self.rebuild_active_scene_view();
        }
    }

    pub(in crate::compositor) fn refresh_active_scene_surface_tree(
        &mut self,
        root_surface_id: u32,
    ) {
        self.compliance_metrics.active_root_scene_refreshes = self
            .compliance_metrics
            .active_root_scene_refreshes
            .saturating_add(1);
        if self.active_scene_view.selection != Some(self.active_scene_selection()) {
            self.rebuild_active_scene_view();
            return;
        }
        let affected = self
            .renderable_surfaces
            .iter()
            .filter(|surface| {
                self.root_surface_id_for_surface(surface.surface_id) == root_surface_id
            })
            .map(|surface| surface.surface_id)
            .collect::<Vec<_>>();
        let mut updated = 0usize;
        let mut membership_changed = false;
        let mut origins_changed = false;
        for surface_id in affected {
            let visible = self.surface_is_visible_in_active_scene(surface_id);
            let cached_index = self
                .active_scene_view
                .surface_indices
                .get(&surface_id)
                .copied();
            let Some(source) = self.renderable_surface(surface_id).cloned() else {
                membership_changed |= cached_index.is_some();
                continue;
            };
            match (visible, cached_index) {
                (true, Some(index)) => {
                    let previous = &self.active_scene_view.surfaces[index];
                    origins_changed |= previous.x != source.x
                        || previous.y != source.y
                        || previous.placement != source.placement
                        || previous.render_placement != source.render_placement;
                    self.active_scene_view.surfaces[index] = source;
                    updated = updated.saturating_add(1);
                }
                (true, None) | (false, Some(_)) => membership_changed = true,
                (false, None) => {}
            }
        }
        if membership_changed {
            self.rebuild_active_scene_view();
        } else if updated > 0 {
            if origins_changed {
                self.active_scene_view.surface_origins =
                    render::surface_origins(&self.active_scene_view.surfaces);
            }
            self.active_scene_view.incremental_surface_update_count = self
                .active_scene_view
                .incremental_surface_update_count
                .saturating_add(updated as u64);
        }
    }

    pub(in crate::compositor) fn active_scene_surfaces(&self) -> &[RenderableSurface] {
        self.active_scene_view.surfaces()
    }

    pub(in crate::compositor) fn active_scene_popup_surface_ids(&self) -> &[u32] {
        self.active_scene_view.popup_surface_ids()
    }

    pub(in crate::compositor) fn active_scene_surface_origins(&self) -> &[(i32, i32)] {
        self.active_scene_view.surface_origins()
    }

    pub(in crate::compositor) fn active_scene_surface_index(
        &self,
        surface_id: u32,
    ) -> Option<usize> {
        self.active_scene_view
            .surface_indices
            .get(&surface_id)
            .copied()
    }

    #[cfg(test)]
    pub(in crate::compositor) const fn active_scene_rebuild_count(&self) -> u64 {
        self.active_scene_view.rebuild_count()
    }

    #[cfg(test)]
    pub(in crate::compositor) const fn active_scene_surface_update_count(&self) -> u64 {
        self.active_scene_view.incremental_surface_update_count()
    }
}
