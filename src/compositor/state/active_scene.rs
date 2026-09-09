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
    pub(in crate::compositor) fn presentation_rect_for_geometry(
        &self,
        root_surface_id: u32,
        geometry: WindowGeometry,
    ) -> Option<PresentationRect> {
        let mut surfaces = self.active_scene_surfaces().to_vec();
        let root_index = surfaces.iter().position(|surface| {
            surface.surface_id == root_surface_id && surface.placement.parent_surface_id.is_none()
        })?;
        surfaces[root_index].render_placement = Some(geometry.placement);
        let origin = render::surface_origins(&surfaces)
            .get(root_index)
            .copied()?;
        PresentationRect::new(
            f64::from(origin.0),
            f64::from(origin.1),
            f64::from(geometry.width),
            f64::from(geometry.height),
        )
    }

    fn presentation_window_targets(&self) -> Vec<(u32, PresentationRect)> {
        self.active_scene_surfaces()
            .iter()
            .enumerate()
            .filter(|(_, surface)| {
                surface.placement.parent_surface_id.is_none()
                    && self.root_surface_id_for_surface(surface.surface_id) == surface.surface_id
            })
            .filter_map(|(index, surface)| {
                let geometry = self
                    .current_visual_root_window_geometry(surface.surface_id)
                    .or_else(|| self.current_root_window_geometry(surface.surface_id))?;
                let origin = self.active_scene_surface_origins().get(index).copied()?;
                Some((
                    surface.surface_id,
                    PresentationRect::new(
                        f64::from(origin.0),
                        f64::from(origin.1),
                        f64::from(geometry.width),
                        f64::from(geometry.height),
                    )?,
                ))
            })
            .collect()
    }

    pub(in crate::compositor) fn presentation_scene_sample_at(
        &self,
        at: AnimationTime,
    ) -> PresentationSceneSample {
        self.presentation_animator
            .sample_scene(at, &self.presentation_window_targets())
    }

    pub(in crate::compositor) fn presentation_animation_has_unsettled_visible_at(
        &self,
        _at: AnimationTime,
    ) -> bool {
        self.presentation_animation_has_pending_visible()
    }

    pub(in crate::compositor) fn presentation_animation_has_pending_visible(&self) -> bool {
        let visible_keys = self
            .presentation_window_targets()
            .into_iter()
            .map(|(key, _)| key)
            .collect::<Vec<_>>();
        self.presentation_animator
            .has_pending_visible(&visible_keys)
    }

    pub(in crate::compositor) fn presentation_animation_pending_for_root(
        &self,
        root_surface_id: u32,
    ) -> bool {
        self.presentation_animator
            .has_pending_visible(&[root_surface_id])
    }

    pub(in crate::compositor) fn presentation_animation_metrics(
        &self,
    ) -> PresentationAnimationMetrics {
        self.presentation_animator.metrics()
    }

    pub(in crate::compositor) fn presented_presentation_transform(
        &self,
        root_surface_id: u32,
    ) -> Option<PresentationGroupTransform> {
        self.presented_presentation
            .transform_for_root(root_surface_id)
    }

    pub(in crate::compositor) fn presented_root_geometry(
        &self,
        root_surface_id: u32,
    ) -> Option<PresentedRootGeometry> {
        self.presented_presentation
            .presented_root_geometry(root_surface_id)
    }

    pub(in crate::compositor) fn current_presentation_rect_for_root(
        &self,
        root_surface_id: u32,
    ) -> Option<PresentationRect> {
        let geometry = self
            .current_visual_root_window_geometry(root_surface_id)
            .or_else(|| self.current_root_window_geometry(root_surface_id))?;
        self.presentation_rect_for_geometry(root_surface_id, geometry)
    }

    pub(in crate::compositor) fn native_frame_presented_root_geometries(
        &self,
        surfaces: &[RenderableSurface],
    ) -> Vec<PresentedRootGeometry> {
        let origins = render::surface_origins(surfaces);
        let mut roots = surfaces
            .iter()
            .enumerate()
            .filter(|(_, surface)| {
                surface.placement.parent_surface_id.is_none()
                    && self.root_surface_id_for_surface(surface.surface_id) == surface.surface_id
                    && self.window_id_for_surface(surface.surface_id).is_some()
            })
            .filter_map(|(index, surface)| {
                let (x, y) = origins.get(index).copied()?;
                let size = surface.render_target_size.unwrap_or(BufferSize {
                    width: surface.width,
                    height: surface.height,
                });
                let rect = PresentationRect::new(
                    f64::from(x),
                    f64::from(y),
                    f64::from(size.width),
                    f64::from(size.height),
                )?;
                Some(PresentedRootGeometry::new(surface.surface_id, rect))
            })
            .collect::<Vec<_>>();
        roots.sort_unstable_by_key(PresentedRootGeometry::root_surface_id);
        roots
    }

    pub(in crate::compositor) fn presented_visual_root_window_geometry(
        &self,
        root_surface_id: u32,
    ) -> Option<WindowGeometry> {
        let presented_rect = self
            .presented_root_geometry(root_surface_id)
            .map(PresentedRootGeometry::presented_rect)
            .or_else(|| {
                self.presented_presentation_transform(root_surface_id)
                    .map(|transform| transform.presented_rect)
            })?;
        let root_surface = self
            .active_scene_surfaces()
            .iter()
            .find(|surface| surface.surface_id == root_surface_id)?;
        let root_mode = self
            .current_visual_root_window_geometry(root_surface_id)
            .or_else(|| self.current_root_window_geometry(root_surface_id))
            .map(|geometry| geometry.placement.root_mode)
            .unwrap_or(root_surface.placement.root_mode);
        let root_origin_without_window_offset = match root_mode {
            RootPlacementMode::Absolute => (root_surface.x, root_surface.y),
            RootPlacementMode::CascadedWindow => {
                let mut surfaces = self.active_scene_surfaces().to_vec();
                let root_index = surfaces
                    .iter()
                    .position(|surface| surface.surface_id == root_surface_id)?;
                surfaces[root_index].render_placement = Some(SurfacePlacement::root());
                render::surface_origins(&surfaces)
                    .get(root_index)
                    .copied()
                    .unwrap_or((root_surface.x, root_surface.y))
            }
        };
        Some(WindowGeometry::new(
            SurfacePlacement {
                parent_surface_id: None,
                local_x: saturating_i32_from_f64(
                    presented_rect.x().round() - f64::from(root_origin_without_window_offset.0),
                ),
                local_y: saturating_i32_from_f64(
                    presented_rect.y().round() - f64::from(root_origin_without_window_offset.1),
                ),
                root_mode,
            },
            saturating_u32_from_f64(presented_rect.width().round()),
            saturating_u32_from_f64(presented_rect.height().round()),
        ))
    }

    pub(in crate::compositor) fn take_over_presented_visual_geometry(
        &mut self,
        root_surface_id: u32,
        geometry: WindowGeometry,
        cause: RenderGenerationCause,
    ) {
        // Keep the last physically presented transform until the next frame
        // publishes its replacement. This preserves the direct-scanout
        // blocker while input and canonical layout take over immediately.
        self.presentation_animator.cancel(root_surface_id);
        let placement_changed =
            self.set_surface_placement_with_cause(root_surface_id, geometry.placement, cause);
        let visual = ToplevelVisualGeometry {
            placement: geometry.placement,
            width: geometry.width,
            height: geometry.height,
            active_resize: None,
            mode_transition: false,
        };
        let visual_changed = self
            .toplevel_visual_geometries
            .insert(root_surface_id, visual)
            != Some(visual);
        self.update_toplevel_visual_render_assignment(root_surface_id);
        if visual_changed && !placement_changed {
            self.advance_render_generation(cause);
        }
        if visual_changed || placement_changed {
            self.advance_pointer_hit_generation();
        }
    }

    pub(in crate::compositor) fn publish_presented_presentation(
        &mut self,
        frame_id: u64,
        presentation: &PresentationFrameSnapshot,
    ) {
        self.presented_presentation_frame_id = frame_id;
        self.presented_presentation = presentation.clone();
        for transform in &presentation.transforms {
            if transform.mathematically_settled {
                self.presentation_animator.acknowledge_presented_transition(
                    transform.root_surface_id,
                    transform.transition_id,
                    transform.presented_rect,
                );
            }
        }
        self.advance_pointer_hit_generation();
    }

    pub(in crate::compositor) fn presented_presentation_frame_id(&self) -> u64 {
        self.presented_presentation_frame_id
    }

    pub(in crate::compositor) fn cancel_presentation_for_root(&mut self, root_surface_id: u32) {
        self.presentation_animator.cancel(root_surface_id);
        self.presented_presentation
            .transforms
            .retain(|transform| transform.root_surface_id != root_surface_id);
        self.presented_presentation.refresh_signature();
    }

    pub(in crate::compositor) fn animate_toplevel_visual_geometry(
        &mut self,
        root_surface_id: u32,
        previous_geometry: Option<WindowGeometry>,
        target_geometry: WindowGeometry,
    ) {
        let interaction_active = self.active_toplevel_resizes.contains_key(&root_surface_id)
            || self.window_interaction.is_some_and(|interaction| {
                interaction.root_surface_id == root_surface_id
                    && matches!(
                        interaction.kind,
                        WindowInteractionKind::Move | WindowInteractionKind::Resize(_)
                    )
            });
        let Some(now) = self
            .layout_animation_epoch
            .or_else(AnimationTime::monotonic_now)
        else {
            self.presentation_animator.cancel(root_surface_id);
            return;
        };
        if interaction_active {
            self.presentation_animator.cancel(root_surface_id);
            return;
        }
        let Some(previous) = previous_geometry
            .and_then(|geometry| self.presentation_rect_for_geometry(root_surface_id, geometry))
        else {
            self.presentation_animator.cancel(root_surface_id);
            return;
        };
        let Some(target) = self.presentation_rect_for_geometry(root_surface_id, target_geometry)
        else {
            self.presentation_animator.cancel(root_surface_id);
            return;
        };
        let curve = AnimationCurve::spring(SpringSpec::new(180.0, 24.0));
        if self
            .presentation_animator
            .sample(root_surface_id, now)
            .is_some()
        {
            self.presentation_animator
                .retarget(root_surface_id, target, now, curve);
        } else {
            self.presentation_animator
                .start(root_surface_id, previous, target, now, curve);
        }
    }

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
        self.refresh_effect_scene_summary();
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

fn saturating_i32_from_f64(value: f64) -> i32 {
    if !value.is_finite() {
        return 0;
    }
    value.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}

fn saturating_u32_from_f64(value: f64) -> u32 {
    if !value.is_finite() || value <= 0.0 {
        return 1;
    }
    value.clamp(1.0, f64::from(u32::MAX)) as u32
}
