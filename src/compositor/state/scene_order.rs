use super::*;
use crate::wm::WorkspaceLocation;

impl CompositorState {
    pub(in crate::compositor) fn reorder_renderable_surfaces_by_committed_stack(&mut self) -> bool {
        if self.defer_render_stack_reorder() {
            return false;
        }
        if self.renderable_surfaces.len() <= 1 {
            return false;
        }

        let original_order = self
            .renderable_surfaces
            .iter()
            .map(|surface| surface.surface_id)
            .collect::<Vec<_>>();
        let mut by_id = self
            .renderable_surfaces
            .drain(..)
            .map(|surface| (surface.surface_id, surface))
            .collect::<HashMap<_, _>>();
        let visible_ids = by_id.keys().copied().collect::<HashSet<_>>();
        let mut roots_with_trees = Vec::new();
        let mut seen_ids = HashSet::new();
        let root_ids = original_order
            .iter()
            .copied()
            .filter(|surface_id| {
                self.surface_placements
                    .get(surface_id)
                    .and_then(|placement| placement.parent_surface_id)
                    .is_none_or(|parent_id| !visible_ids.contains(&parent_id))
            })
            .collect::<Vec<_>>();

        for root_id in root_ids {
            let mut tree_ids = Vec::new();
            self.append_surface_tree_order(root_id, &visible_ids, &mut tree_ids);
            for surface_id in &tree_ids {
                seen_ids.insert(*surface_id);
            }
            roots_with_trees.push((root_id, tree_ids));
        }
        for surface_id in &original_order {
            if visible_ids.contains(surface_id) && !seen_ids.contains(surface_id) {
                let root_id = *surface_id;
                let mut tree_ids = Vec::new();
                self.append_surface_tree_order(root_id, &visible_ids, &mut tree_ids);
                for surface_id in &tree_ids {
                    seen_ids.insert(*surface_id);
                }
                roots_with_trees.push((root_id, tree_ids));
            }
        }
        let original_positions = roots_with_trees
            .iter()
            .enumerate()
            .map(|(position, (root_id, _))| (*root_id, position))
            .collect::<HashMap<_, _>>();
        roots_with_trees.sort_by_key(|(root_id, _)| {
            self.renderable_root_stack_key(
                *root_id,
                original_positions
                    .get(root_id)
                    .copied()
                    .unwrap_or(usize::MAX),
            )
        });
        let ordered_ids = roots_with_trees
            .into_iter()
            .flat_map(|(_, tree_ids)| tree_ids)
            .collect::<Vec<_>>();

        self.renderable_surfaces = ordered_ids
            .into_iter()
            .filter_map(|surface_id| by_id.remove(&surface_id))
            .collect();
        self.rebuild_renderable_surface_index();
        let changed = self
            .renderable_surfaces
            .iter()
            .map(|surface| surface.surface_id)
            .ne(original_order);
        if changed {
            self.invalidate_surface_origin_cache();
            self.refresh_active_scene_surface_order();
        }
        changed
    }

    pub(in crate::compositor) fn renderable_root_stack_key(
        &self,
        root_id: u32,
        original_position: usize,
    ) -> (u8, u8, u64, usize) {
        if let Some(role) = self.layer_surfaces.get(&root_id) {
            let band = match role.committed.layer.scene_rank() {
                0 => 0,
                1 => 1,
                3 => 6,
                4 => 7,
                rank => rank,
            };
            return (
                band,
                role.committed.layer.scene_rank(),
                role.order,
                original_position,
            );
        }

        if let Some(window_id) = self
            .desktop_windows
            .values()
            .find(|window| window.root_surface_id == root_id)
            .map(|window| window.id)
        {
            let window = self
                .window(window_id)
                .expect("desktop window root must remain registered");
            let layer_rank = match window.stack_layer {
                DesktopStackLayer::Normal | DesktopStackLayer::Above => 2,
                DesktopStackLayer::Popup => 3,
                DesktopStackLayer::Notification => 3,
                DesktopStackLayer::Overlay => 4,
            };
            let owner_id = self
                .canonical_scene_owner_window_id(window_id)
                .unwrap_or(window_id);
            let scene_band = self
                .window(owner_id)
                .and_then(|owner| owner.management)
                .map(|management| match management.location() {
                    WorkspaceLocation::Regular(_) => {
                        if management.layout() == crate::wm::LayoutMembership::Tiled {
                            2
                        } else {
                            3
                        }
                    }
                    WorkspaceLocation::Special(_) => {
                        if management.layout() == crate::wm::LayoutMembership::Tiled {
                            4
                        } else {
                            5
                        }
                    }
                })
                .unwrap_or(6);
            let stack_position = self
                .window_stacking
                .iter()
                .position(|id| *id == window_id)
                .unwrap_or(usize::MAX) as u64;
            return (scene_band, layer_rank, stack_position, original_position);
        }

        (2, 0, 0, original_position)
    }

    fn canonical_scene_owner_window_id(&self, window_id: WindowId) -> Option<WindowId> {
        let mut current = window_id;
        for _ in 0..=self.desktop_windows.len() {
            let window = self.window(current)?;
            if let Some(parent) = window
                .relationships
                .parent
                .or(window.relationships.transient_for)
            {
                current = parent;
            } else if window.management.is_some() {
                return Some(current);
            } else {
                return None;
            }
        }
        None
    }

    pub(in crate::compositor) fn append_surface_tree_order(
        &self,
        surface_id: u32,
        visible_ids: &HashSet<u32>,
        ordered_ids: &mut Vec<u32>,
    ) {
        if !visible_ids.contains(&surface_id) || ordered_ids.contains(&surface_id) {
            return;
        }

        if let Some(stack) = self.committed_subsurface_stacks.get(&surface_id) {
            for stacked_id in stack {
                if *stacked_id == surface_id {
                    ordered_ids.push(surface_id);
                } else {
                    self.append_surface_tree_order(*stacked_id, visible_ids, ordered_ids);
                }
            }
        } else {
            ordered_ids.push(surface_id);
        }

        let children = self
            .surface_placements
            .iter()
            .filter_map(|(child_id, placement)| {
                (placement.parent_surface_id == Some(surface_id)
                    && visible_ids.contains(child_id)
                    && !ordered_ids.contains(child_id))
                .then_some(*child_id)
            })
            .collect::<Vec<_>>();
        for child_id in children {
            self.append_surface_tree_order(child_id, visible_ids, ordered_ids);
        }
    }

    pub(in crate::compositor) fn reorder_renderable_surfaces_by_window_stack(&mut self) -> bool {
        let previous_active_order = self
            .active_scene_view
            .surfaces()
            .iter()
            .map(|surface| surface.surface_id)
            .collect::<Vec<_>>();
        let changed = self.reorder_renderable_surfaces_by_committed_stack();
        if changed {
            let scene_effect = previous_active_order
                != self
                    .active_scene_view
                    .surfaces()
                    .iter()
                    .map(|surface| surface.surface_id)
                    .collect::<Vec<_>>();
            self.advance_render_generation_with_scene_effect(
                RenderGenerationCause::WindowStack,
                scene_effect,
            );
        }
        changed
    }
}
