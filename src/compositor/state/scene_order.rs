use super::*;
use crate::compositor::SpecialWorkspaceDim;
use crate::compositor::effects::{
    EffectAnchor, EffectAnchorScope, EffectSceneOrder, ResolvedEffectInstance,
    fold_composite_opacity_signature, internal_effect_signature,
    special_workspace_blur_instance_id,
};
use crate::effects::{EffectRect, EffectRegion};
use crate::presentation_animation::AnimationTime;
use crate::wm::WorkspaceLocation;

// Scene bands, bottom to top. Band numbers are compared before the layer and
// stack keys, so the order here is the stacking order.
const SCENE_BAND_BOTTOM_LAYER: u8 = 1;
const SCENE_BAND_TOP_LAYER: u8 = 6;
// Windows of the fullscreen owner family, and windows whose maximize or
// fullscreen mode-transition animation is in flight, stack above Top layers.
const SCENE_BAND_RAISED_WINDOW: u8 = 7;
// Application surfaces the fullscreen plan allows above the owner (special
// workspace, notifications, overlays) stay above the raised owner family.
const SCENE_BAND_FULLSCREEN_ALLOWED_WINDOW: u8 = 8;
const SCENE_BAND_OVERLAY_LAYER: u8 = 9;
const SCENE_BAND_DRAG_ICON: u8 = 10;

/// Opacity of the black dim drawn behind a visible special workspace. The dim
/// sits between the regular bands and the special bands (see
/// `special_workspace_dim_anchor_surface_id`).
pub(crate) const SPECIAL_WORKSPACE_DIM_ALPHA: f32 = 0.2;

/// Hyprland `blur:special`: blur the regular content behind a visible special
/// workspace. Switch off to keep only the dim.
pub(crate) const SPECIAL_WORKSPACE_BLUR_ENABLED: bool = true;

/// Composite opacity at or below which the special blur is skipped entirely.
const SPECIAL_WORKSPACE_BLUR_MIN_OPACITY: f32 = 0.001;

impl CompositorState {
    pub(in crate::compositor) fn surface_root_is_in_special_workspace(
        &self,
        root_surface_id: u32,
    ) -> bool {
        self.window_id_for_surface(root_surface_id)
            .is_some_and(|window_id| {
                matches!(
                    self.scene_work_owner_for_window(window_id),
                    SceneWorkOwner::Location(WorkspaceLocation::Special(_))
                )
            })
    }

    /// Frame-level dim description for the ordered `surfaces` and their
    /// aligned presentation owner roots.
    pub(in crate::compositor) fn special_workspace_dim(
        &self,
        at: AnimationTime,
        surfaces: &[RenderableSurface],
        owner_roots: &[u32],
    ) -> SpecialWorkspaceDim {
        let alpha = self.special_workspace_dim_alpha_at(at);
        if alpha <= 0.0 {
            return SpecialWorkspaceDim::default();
        }
        SpecialWorkspaceDim::new(
            alpha,
            self.special_workspace_dim_anchor_surface_id(surfaces, owner_roots),
        )
    }

    /// Full-output background blur for the special dim, composited between the
    /// regular content and the dim (regular -> blur -> dim -> special). It is
    /// anchored after the last dimmed visual group, not before the dim anchor:
    /// a `BeforeSurface` capture would include the dim quad, and
    /// `OutputPostProcess` would also blur the special windows and overlays.
    /// Returns `None` when nothing regular lies under the dim.
    pub(in crate::compositor) fn special_workspace_blur_instance(
        &self,
        dim: SpecialWorkspaceDim,
        surfaces: &[RenderableSurface],
        popup_surface_ids: &[u32],
        output_size: (u32, u32),
    ) -> Option<ResolvedEffectInstance> {
        if !SPECIAL_WORKSPACE_BLUR_ENABLED || !dim.is_active() || !self.background_effect_enabled {
            return None;
        }
        // The blur fades in and out with the dim; a fully faded blur is skipped.
        let opacity = (dim.alpha() / SPECIAL_WORKSPACE_DIM_ALPHA).clamp(0.0, 1.0);
        if opacity <= SPECIAL_WORKSPACE_BLUR_MIN_OPACITY {
            return None;
        }
        let composite_opacity = Some(opacity);
        let dimmed_end = match dim.anchor_surface_id() {
            Some(anchor_surface_id) => surfaces
                .iter()
                .position(|surface| surface.surface_id == anchor_surface_id)?,
            None => surfaces.len(),
        };
        let last_dimmed = surfaces.get(dimmed_end.checked_sub(1)?)?;
        let groups = visual_stack_groups(surfaces, popup_surface_ids);
        let visual_group =
            Self::visual_group_for_surface_in_groups(surfaces, &groups, last_dimmed.surface_id)?;
        if let Some(anchor_surface_id) = dim.anchor_surface_id()
            && Self::visual_group_for_surface_in_groups(surfaces, &groups, anchor_surface_id)
                == Some(visual_group)
        {
            return None;
        }
        let region = EffectRegion::from_rect(EffectRect::new(0, 0, output_size.0, output_size.1)?);
        let target_bounds = region.bounding_rect()?;
        let (program, parameter_block, frame_demand) = self.selected_background_blur_material();
        let anchor = EffectAnchor::AfterSurface(last_dimmed.surface_id);
        let mut instance = ResolvedEffectInstance {
            id: special_workspace_blur_instance_id(),
            program,
            anchor,
            signature: fold_composite_opacity_signature(
                internal_effect_signature(
                    last_dimmed.surface_id,
                    anchor,
                    program,
                    &region,
                    &parameter_block,
                    None,
                ),
                composite_opacity,
            ),
            region,
            target_bounds,
            parameter_block,
            coverage: None,
            frame_demand,
            visual_group: Some(visual_group),
            anchor_scope: EffectAnchorScope::VisualGroup,
            backdrop_reuse: EffectBackdropReuse::Exact,
            scene_order: EffectSceneOrder::for_anchor(anchor),
            composite_opacity,
        };
        instance.scene_order = Self::scene_order_for_frame_instance(&instance, surfaces);
        Some(instance)
    }

    /// The dim is drawn immediately after the last surface that belongs to a
    /// regular workspace window or to the background/bottom layers, so every
    /// regular surface is dimmed even when a regular fullscreen owner is
    /// raised above the Top layers. Special windows stack above all regular
    /// windows (bands 4/5, or band 8 over a fullscreen owner), so they and the
    /// Top/Overlay layers above them stay undimmed; an empty special still
    /// dims the regular content. With nothing above the regular content the
    /// dim is drawn last (`None`).
    fn special_workspace_dim_anchor_surface_id(
        &self,
        surfaces: &[RenderableSurface],
        owner_roots: &[u32],
    ) -> Option<u32> {
        let dimmed_prefix = surfaces
            .iter()
            .zip(owner_roots.iter().copied())
            .rposition(|(_, owner_root)| {
                self.surface_root_is_in_regular_workspace(owner_root)
                    || self.renderable_root_stack_key(owner_root, 0).0 <= SCENE_BAND_BOTTOM_LAYER
            })
            .map_or(0, |index| index + 1);
        surfaces
            .get(dimmed_prefix)
            .map(|surface| surface.surface_id)
    }

    fn surface_root_is_in_regular_workspace(&self, root_surface_id: u32) -> bool {
        self.window_id_for_surface(root_surface_id)
            .is_some_and(|window_id| {
                matches!(
                    self.scene_work_owner_for_window(window_id),
                    SceneWorkOwner::Location(WorkspaceLocation::Regular(_))
                )
            })
    }

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
                self.surface_topology
                    .parent_surface_id(*surface_id)
                    .is_none_or(|parent_id| !visible_ids.contains(&parent_id))
            })
            .collect::<Vec<_>>();

        for root_id in root_ids {
            let mut tree_ids = Vec::new();
            self.surface_topology
                .append_surface_tree_order(root_id, &visible_ids, &mut tree_ids);
            for surface_id in &tree_ids {
                seen_ids.insert(*surface_id);
            }
            roots_with_trees.push((root_id, tree_ids));
        }
        for surface_id in &original_order {
            if visible_ids.contains(surface_id) && !seen_ids.contains(surface_id) {
                let root_id = *surface_id;
                let mut tree_ids = Vec::new();
                self.surface_topology.append_surface_tree_order(
                    root_id,
                    &visible_ids,
                    &mut tree_ids,
                );
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
        if self.surface_role(root_id) == SurfaceRole::DragIcon {
            return (SCENE_BAND_DRAG_ICON, 0, 0, original_position);
        }

        if let Some(role) = self.layer_surfaces.get(&root_id) {
            let band = match role.committed.layer.scene_rank() {
                0 => 0,
                1 => SCENE_BAND_BOTTOM_LAYER,
                3 => SCENE_BAND_TOP_LAYER,
                4 => SCENE_BAND_OVERLAY_LAYER,
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
                .unwrap_or(SCENE_BAND_TOP_LAYER);
            let scene_band = if self.window_raised_above_top_layers(root_id) {
                SCENE_BAND_RAISED_WINDOW
            } else if self.window_allowed_above_fullscreen_owner(root_id) {
                SCENE_BAND_FULLSCREEN_ALLOWED_WINDOW
            } else {
                scene_band
            };
            let stack_position = self
                .window_stacking
                .iter()
                .position(|id| *id == window_id)
                .unwrap_or(usize::MAX) as u64;
            return (scene_band, layer_rank, stack_position, original_position);
        }

        (2, 0, 0, original_position)
    }

    /// True while the window belongs to the fullscreen owner family or its
    /// mode-transition geometry animation is still in flight.
    fn window_raised_above_top_layers(&self, root_id: u32) -> bool {
        if self.mode_transition_raise_active(root_id) {
            return true;
        }
        self.fullscreen_presentation.is_some_and(|owner| {
            self.root_belongs_to_fullscreen_owner_family(owner.owner_root_surface_id, root_id)
        })
    }

    fn window_allowed_above_fullscreen_owner(&self, root_id: u32) -> bool {
        self.fullscreen_presentation.is_some_and(|owner| {
            matches!(
                self.fullscreen_root_classification(owner.owner_root_surface_id, root_id),
                FullscreenRootClassification::AllowedAboveFullscreen(_)
            )
        })
    }

    /// A raise marker only counts while its root still has the geometry track
    /// it was recorded for; the settled track is the source of truth.
    fn mode_transition_raise_active(&self, root_id: u32) -> bool {
        self.mode_transition_raise_roots.contains(&root_id)
            && self
                .presentation_scene_node_id_for_root(root_id)
                .is_some_and(|scene_node_id| {
                    self.presentation_animator.has_geometry_track(scene_node_id)
                })
    }

    /// Drops raise markers whose geometry track has settled or was cancelled.
    /// Returns true when a marker was dropped, so the caller restacks.
    pub(in crate::compositor) fn sync_mode_transition_raise_roots(&mut self) -> bool {
        // Pending layout transactions have not committed their tracks yet, so
        // their markers must survive until the batch finishes.
        if self.layout_batch_depth > 0 {
            return false;
        }
        let before = self.mode_transition_raise_roots.len();
        self.mode_transition_raise_roots = self
            .mode_transition_raise_roots
            .iter()
            .copied()
            .filter(|root_id| self.mode_transition_raise_active(*root_id))
            .collect();
        self.mode_transition_raise_roots.len() != before
    }

    pub(in crate::compositor) fn restack_for_window_raise_change(&mut self) {
        self.reorder_renderable_surfaces_by_window_stack();
        self.advance_pointer_hit_generation();
    }

    pub(in crate::compositor) fn canonical_scene_owner_window_id(
        &self,
        window_id: WindowId,
    ) -> Option<WindowId> {
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
