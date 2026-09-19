use super::*;
use crate::presentation_animation::PresentationOpacity;
use crate::wm::{SpecialWorkspaceId, WorkspaceId};
use std::time::Duration;

#[derive(Debug)]
pub(in crate::compositor) struct PendingPresentationGeometryTransaction {
    pub(in crate::compositor) started_at: AnimationTime,
    pub(in crate::compositor) members:
        Vec<crate::presentation_animation::PresentationGeometryMutation>,
}

impl PendingPresentationGeometryTransaction {
    pub(in crate::compositor) fn upsert_geometry_mutation(
        &mut self,
        mutation: crate::presentation_animation::PresentationGeometryMutation,
    ) {
        if let Some(existing) = self
            .members
            .iter_mut()
            .find(|member| member.scene_node_id() == mutation.scene_node_id())
        {
            existing.target = mutation.target;
            existing.curve = mutation.curve;
        } else {
            self.members.push(mutation);
        }
    }
}

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
    surface_scene_nodes: HashMap<u32, SceneNodeId>,
    surface_scene_nodes_in_order: Vec<SceneNodeId>,
    scene_node_indices: HashMap<SceneNodeId, usize>,
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

    pub(in crate::compositor) fn surface_scene_nodes_in_order(&self) -> &[SceneNodeId] {
        &self.surface_scene_nodes_in_order
    }

    #[allow(dead_code)]
    pub(in crate::compositor) fn scene_node_id_for_surface(
        &self,
        surface_id: u32,
    ) -> Option<SceneNodeId> {
        self.surface_scene_nodes.get(&surface_id).copied()
    }

    #[allow(dead_code)]
    pub(in crate::compositor) fn surface_index_for_scene_node(
        &self,
        scene_node_id: SceneNodeId,
    ) -> Option<usize> {
        self.scene_node_indices.get(&scene_node_id).copied()
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

fn materialize_presented_window_geometry(
    canonical_geometry: WindowGeometry,
    canonical_rect: PresentationRect,
    presented_rect: PresentationRect,
) -> WindowGeometry {
    let delta_x = saturating_i32_from_f64((presented_rect.x() - canonical_rect.x()).round());
    let delta_y = saturating_i32_from_f64((presented_rect.y() - canonical_rect.y()).round());
    WindowGeometry::new(
        SurfacePlacement {
            parent_surface_id: None,
            local_x: canonical_geometry.placement.local_x.saturating_add(delta_x),
            local_y: canonical_geometry.placement.local_y.saturating_add(delta_y),
            root_mode: canonical_geometry.placement.root_mode,
        },
        saturating_u32_from_f64(presented_rect.width().round()),
        saturating_u32_from_f64(presented_rect.height().round()),
    )
}

impl CompositorState {
    fn presentation_output_id(&self) -> crate::core::OutputId {
        self.native_output_id()
            .expect("native presentation requires allocated logical OutputId")
    }

    pub(in crate::compositor) fn presentation_rect_for_geometry(
        &self,
        root_surface_id: u32,
        geometry: WindowGeometry,
    ) -> Option<PresentationRect> {
        let surfaces = self.active_scene_surfaces();
        let root_index = surfaces.iter().position(|surface| {
            surface.surface_id == root_surface_id && surface.placement.parent_surface_id.is_none()
        })?;
        let root_ordinal = surfaces
            .iter()
            .take(root_index)
            .filter(|surface| {
                surface.placement.parent_surface_id.is_none()
                    && self.root_surface_id_for_surface(surface.surface_id) == surface.surface_id
            })
            .count();
        let base = match geometry.placement.root_mode {
            RootPlacementMode::CascadedWindow => render::cascaded_root_position(root_ordinal),
            RootPlacementMode::Absolute => (0, 0),
        };
        PresentationRect::new(
            f64::from(base.0.saturating_add(geometry.placement.local_x)),
            f64::from(base.1.saturating_add(geometry.placement.local_y)),
            f64::from(geometry.width),
            f64::from(geometry.height),
        )
    }

    fn presentation_targets_for_surfaces(
        &self,
        surfaces: &[RenderableSurface],
    ) -> NativeFramePresentationTargets {
        let mut seen_window_groups = std::collections::HashSet::new();
        let windows = surfaces
            .iter()
            .filter_map(|surface| {
                let root_surface_id = self.presentation_owner_root_for_surface(surface.surface_id);
                let scene_node_id = self.presentation_scene_node_id_for_root(root_surface_id)?;
                if !seen_window_groups.insert(scene_node_id) {
                    return None;
                }
                let geometry = self
                    .current_visual_root_window_geometry(root_surface_id)
                    .or_else(|| self.current_root_window_geometry(root_surface_id))?;
                let canonical_rect =
                    self.presentation_rect_for_geometry(root_surface_id, geometry)?;
                let canonical_opacity = self
                    .window_id_for_surface(root_surface_id)
                    .and_then(|window_id| self.window(window_id))
                    .map_or(
                        PresentationOpacity::OPAQUE,
                        DesktopWindow::canonical_opacity,
                    );
                Some(
                    PresentationWindowTarget::with_scene_node(
                        scene_node_id,
                        root_surface_id,
                        canonical_rect,
                    )
                    .with_canonical_opacity(canonical_opacity),
                )
            })
            .collect();
        NativeFramePresentationTargets::from_windows(windows)
    }

    pub(in crate::compositor) fn native_frame_presentation_targets(
        &self,
        surfaces: &[RenderableSurface],
    ) -> NativeFramePresentationTargets {
        self.presentation_targets_for_surfaces(surfaces)
    }

    pub(in crate::compositor) fn set_window_canonical_opacity(
        &mut self,
        window_id: WindowId,
        opacity: PresentationOpacity,
        curve: Option<AnimationCurve>,
    ) -> Result<(), crate::presentation_animation::PresentationTransactionError> {
        let Some(window) = self.window(window_id) else {
            return Err(crate::presentation_animation::PresentationTransactionError::
                MissingPresentationOwner);
        };
        let scene_node_id = self.scene_node_id_for_window_group(window_id).ok_or(
            crate::presentation_animation::PresentationTransactionError::MissingPresentationOwner,
        )?;
        let previous = window.canonical_opacity();
        if previous == opacity && !self.presentation_animator.has_opacity_track(scene_node_id) {
            return Ok(());
        }
        let Some(now) = self
            .layout_animation_epoch
            .or_else(AnimationTime::monotonic_now)
        else {
            return Err(crate::presentation_animation::PresentationTransactionError::Empty);
        };

        if let Some(curve) = curve.filter(|_| self.presentation_animator.is_enabled()) {
            let request = crate::presentation_animation::PresentationTransactionRequest::opacity(
                now,
                vec![
                    crate::presentation_animation::PresentationOpacityMutation::new(
                        scene_node_id,
                        previous,
                        opacity,
                        curve,
                    ),
                ],
            );
            match self.presentation_animator.commit(request) {
                Ok(_) | Err(crate::presentation_animation::PresentationTransactionError::Empty) => {
                }
                Err(error) => return Err(error),
            }
        } else {
            self.presentation_animator.cancel_opacity(scene_node_id);
        }

        if let Some(window) = self.window_mut(window_id) {
            window.set_canonical_opacity(opacity);
        }
        self.advance_render_generation(RenderGenerationCause::WindowMode);
        Ok(())
    }

    #[doc(hidden)]
    pub fn install_native_frame_test_scene(
        &mut self,
        surfaces: Vec<RenderableSurface>,
        windows: &[(u32, WindowId)],
        fullscreen_owner: Option<u32>,
    ) {
        for surface in &surfaces {
            self.ensure_surface_scene_node(surface.surface_id);
        }
        self.renderable_surfaces = surfaces;
        self.rebuild_renderable_surface_index();
        self.surface_placements = self
            .renderable_surfaces
            .iter()
            .map(|surface| (surface.surface_id, surface.placement))
            .collect();
        self.window_by_root_surface.clear();
        self.desktop_windows.clear();
        self.window_stacking.clear();
        for &(root_surface_id, window_id) in windows {
            self.window_by_root_surface
                .insert(root_surface_id, window_id);
            self.desktop_windows.insert(
                window_id,
                DesktopWindow::new_xdg(window_id, root_surface_id),
            );
            self.ensure_window_scene_nodes(window_id, root_surface_id);
            self.window_stacking.push(window_id);
        }
        self.fullscreen_presentation =
            fullscreen_owner.map(|owner_root_surface_id| FullscreenPresentationState {
                owner_root_surface_id,
                output_width: self.output_size.width,
                output_height: self.output_size.height,
            });
        self.rebuild_active_scene_view();
    }

    #[doc(hidden)]
    pub fn install_native_frame_test_scene_with_server_decorations(
        &mut self,
        surfaces: Vec<RenderableSurface>,
        windows: &[(u32, WindowId)],
        fullscreen_owner: Option<u32>,
    ) {
        self.install_native_frame_test_scene(surfaces, windows, fullscreen_owner);
        for &(root_surface_id, _) in windows {
            self.xdg_decoration_states
                .insert(root_surface_id, WindowDecorationState::new());
        }
        self.rebuild_active_scene_view();
    }

    #[doc(hidden)]
    pub fn start_test_presentation_transition(
        &mut self,
        root_surface_id: u32,
        start: PresentationRect,
        target: PresentationRect,
        at: AnimationTime,
    ) {
        let Some(scene_node_id) = self.presentation_scene_node_id_for_root(root_surface_id) else {
            return;
        };
        let _ = self.presentation_animator.commit(
            crate::presentation_animation::PresentationTransactionRequest::geometry(
                at,
                vec![
                    crate::presentation_animation::PresentationGeometryMutation::new(
                        scene_node_id,
                        start,
                        target,
                        AnimationCurve::easing(Duration::from_millis(1), EasingCurve::Linear),
                    ),
                ],
            ),
        );
    }

    pub(in crate::compositor) fn presentation_scene_sample_at(
        &self,
        at: AnimationTime,
    ) -> PresentationSceneSample {
        let targets = self.presentation_targets_for_surfaces(self.active_scene_surfaces());
        self.presentation_animator.sample(
            self.presentation_output_id(),
            at,
            crate::presentation_animation::PresentationSampleTimeSource::MonotonicFallback,
            targets.windows(),
        )
    }

    pub(in crate::compositor) fn presentation_scene_sample_for_targets_at(
        &self,
        at: AnimationTime,
        targets: &NativeFramePresentationTargets,
    ) -> PresentationSceneSample {
        self.presentation_scene_sample_for_targets_at_with_source(
            at,
            crate::presentation_animation::PresentationSampleTimeSource::MonotonicFallback,
            targets,
        )
    }

    pub(in crate::compositor) fn presentation_scene_sample_for_targets_at_with_source(
        &self,
        at: AnimationTime,
        source: crate::presentation_animation::PresentationSampleTimeSource,
        targets: &NativeFramePresentationTargets,
    ) -> PresentationSceneSample {
        self.presentation_animator.sample(
            self.presentation_output_id(),
            at,
            source,
            targets.windows(),
        )
    }

    pub(in crate::compositor) fn presentation_animation_has_unsettled_visible_at(
        &self,
        _at: AnimationTime,
    ) -> bool {
        self.presentation_animation_has_pending_visible()
    }

    pub(in crate::compositor) fn presentation_animation_has_pending_visible(&self) -> bool {
        let surfaces = self.native_frame_renderable_surfaces();
        let targets = self.native_frame_presentation_targets(surfaces.as_ref());
        let visible_keys = targets.scene_node_ids().collect::<Vec<_>>();
        self.presentation_animator
            .has_pending_visible(&visible_keys)
    }

    pub(in crate::compositor) fn presentation_animation_pending_for_root(
        &self,
        root_surface_id: u32,
    ) -> bool {
        let Some(scene_node_id) = self.presentation_scene_node_id_for_root(root_surface_id) else {
            return false;
        };
        self.presentation_animator
            .has_pending_visible(&[scene_node_id])
    }

    pub(in crate::compositor) fn presentation_scene_node_id_for_root(
        &self,
        root_surface_id: u32,
    ) -> Option<SceneNodeId> {
        let group = self
            .window_id_for_surface(root_surface_id)
            .and_then(|window_id| self.scene_node_id_for_window_group(window_id));
        #[cfg(test)]
        {
            group.or_else(|| SceneNodeId::from_raw(u64::from(root_surface_id)))
        }
        #[cfg(not(test))]
        {
            group
        }
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
            .as_ref()?
            .transform_for_root(root_surface_id)
    }

    pub(in crate::compositor) fn presented_presentation_transform_for_scene_node(
        &self,
        scene_node_id: SceneNodeId,
    ) -> Option<PresentationGroupTransform> {
        self.presented_presentation
            .as_ref()?
            .transform_for_scene_node(scene_node_id)
    }

    pub(in crate::compositor) fn presented_presentation_opacity(
        &self,
        root_surface_id: u32,
    ) -> PresentationOpacity {
        self.presented_presentation
            .as_ref()
            .map_or(PresentationOpacity::OPAQUE, |presentation| {
                presentation.opacity_for_root(root_surface_id)
            })
    }

    pub(in crate::compositor) fn presented_presentation_opacity_for_scene_node(
        &self,
        scene_node_id: SceneNodeId,
    ) -> PresentationOpacity {
        self.presented_presentation
            .as_ref()
            .map_or(PresentationOpacity::OPAQUE, |presentation| {
                presentation.opacity_for_scene_node(scene_node_id)
            })
    }

    pub(in crate::compositor) fn presented_presentation_opacity_is_non_identity(
        &self,
        root_surface_id: u32,
    ) -> bool {
        !self
            .presented_presentation_opacity(root_surface_id)
            .is_opaque()
    }

    pub(in crate::compositor) fn presented_presentation_opacity_is_non_identity_for_scene_node(
        &self,
        scene_node_id: SceneNodeId,
    ) -> bool {
        !self
            .presented_presentation_opacity_for_scene_node(scene_node_id)
            .is_opaque()
    }

    pub(in crate::compositor) fn presented_window_geometry(
        &self,
        root_surface_id: u32,
    ) -> Option<PresentedWindowGeometry> {
        self.presented_window_geometries
            .binary_search_by_key(&root_surface_id, PresentedWindowGeometry::root_surface_id)
            .ok()
            .map(|index| self.presented_window_geometries[index])
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

    pub(in crate::compositor) fn presented_window_geometries_for_targets(
        &self,
        presentation: &PresentationSceneSample,
        targets: &NativeFramePresentationTargets,
    ) -> Vec<PresentedWindowGeometry> {
        let mut windows = targets
            .windows()
            .iter()
            .map(|target| {
                let root_surface_id = target.root_surface_id();
                let presented_rect = presentation
                    .transform_for_root(root_surface_id)
                    .map_or(target.canonical_rect(), |transform| {
                        transform.presented_rect
                    });
                PresentedWindowGeometry::with_scene_node(
                    target.window_group_scene_node_id(),
                    root_surface_id,
                    presented_rect,
                )
            })
            .collect::<Vec<_>>();
        windows.sort_unstable_by_key(PresentedWindowGeometry::root_surface_id);
        windows
    }

    pub(in crate::compositor) fn presented_visual_root_window_geometry(
        &self,
        root_surface_id: u32,
    ) -> Option<WindowGeometry> {
        let canonical_geometry = self
            .current_visual_root_window_geometry(root_surface_id)
            .or_else(|| self.current_root_window_geometry(root_surface_id))?;
        let canonical_rect =
            self.presentation_rect_for_geometry(root_surface_id, canonical_geometry)?;
        let presented_rect = self
            .presented_window_geometry(root_surface_id)
            .map(PresentedWindowGeometry::presented_rect)?;
        Some(materialize_presented_window_geometry(
            canonical_geometry,
            canonical_rect,
            presented_rect,
        ))
    }

    pub(in crate::compositor) fn rebase_interaction_to_presented_origin(
        &mut self,
        root_surface_id: u32,
        presented_origin: SurfacePlacement,
        cause: RenderGenerationCause,
    ) -> Option<WindowGeometry> {
        let canonical_geometry = self
            .current_visual_root_window_geometry(root_surface_id)
            .or_else(|| self.current_root_window_geometry(root_surface_id))?;

        // Keep the last physically presented transform until the next frame
        // publishes its replacement. This preserves the direct-scanout
        // blocker while input and canonical layout take over immediately.
        self.cancel_presentation_geometry_for_root(root_surface_id);
        let placement_changed =
            self.set_surface_placement_with_cause(root_surface_id, presented_origin, cause);
        if let Some(window_id) = self.window_id_for_surface(root_surface_id)
            && self
                .window(window_id)
                .is_some_and(|window| matches!(window.backend, WindowBackend::X11(_)))
        {
            self.set_x11_frame_geometry(
                window_id,
                WindowGeometry::new(
                    presented_origin,
                    canonical_geometry.width,
                    canonical_geometry.height,
                ),
            );
        }
        let visual_changed =
            if let Some(visual) = self.toplevel_visual_geometries.get_mut(&root_surface_id) {
                if visual.placement != presented_origin {
                    visual.placement = presented_origin;
                    true
                } else {
                    false
                }
            } else {
                self.toplevel_visual_geometries.insert(
                    root_surface_id,
                    ToplevelVisualGeometry {
                        placement: presented_origin,
                        width: canonical_geometry.width,
                        height: canonical_geometry.height,
                        active_resize: None,
                        mode_transition: false,
                    },
                );
                true
            };
        self.update_toplevel_visual_render_assignment(root_surface_id);
        if visual_changed && !placement_changed {
            self.advance_render_generation(cause);
        }
        if visual_changed || placement_changed {
            self.advance_pointer_hit_generation();
        }
        Some(WindowGeometry::new(
            presented_origin,
            canonical_geometry.width,
            canonical_geometry.height,
        ))
    }

    pub(in crate::compositor) fn publish_presented_presentation(
        &mut self,
        frame_id: u64,
        presentation: &PresentationFrameSnapshot,
    ) {
        let expected_output_id = self.presentation_output_id();
        if presentation.output_id != expected_output_id {
            return;
        }
        self.presented_presentation_frame_id = frame_id;
        self.presented_presentation = Some(presentation.clone());
        self.presented_window_geometries = presentation.presented_windows.clone();
        for transform in &presentation.transforms {
            if transform.mathematically_settled {
                self.presentation_animator.acknowledge_presented_geometry(
                    expected_output_id,
                    crate::presentation_animation::PresentedGeometryAck::from_transform(
                        presentation.output_id,
                        *transform,
                    ),
                );
            }
        }
        for opacity in &presentation.opacities {
            if opacity
                .transition
                .is_some_and(|transition| transition.mathematically_settled)
                && let Some(ack) =
                    crate::presentation_animation::PresentedOpacityAck::from_group_opacity(
                        presentation.output_id,
                        *opacity,
                    )
            {
                self.presentation_animator
                    .acknowledge_presented_opacity(expected_output_id, ack);
            }
        }
        self.advance_pointer_hit_generation();
    }

    pub(in crate::compositor) fn presented_presentation_frame_id(&self) -> u64 {
        self.presented_presentation_frame_id
    }

    pub(in crate::compositor) fn cancel_presentation_for_root(&mut self, root_surface_id: u32) {
        // Destructive physical-identity removal is for root teardown. An
        // interaction handoff cancels the animator through its immediate
        // visual installer and keeps the last pageflip ledger intact.
        if let Some(window_id) = self.window_id_for_surface(root_surface_id)
            && let Some(scene_node_id) = self.scene_node_id_for_window_group(window_id)
        {
            self.presentation_animator.cancel_all(scene_node_id);
        }
        if let Some(presentation) = self.presented_presentation.as_mut() {
            presentation
                .transforms
                .retain(|transform| transform.root_surface_id != root_surface_id);
            presentation
                .opacities
                .retain(|opacity| opacity.root_surface_id != root_surface_id);
            presentation.refresh_signature();
        }
        self.presented_window_geometries
            .retain(|window| window.root_surface_id() != root_surface_id);
    }

    pub(in crate::compositor) fn cancel_presentation_geometry_for_root(
        &mut self,
        root_surface_id: u32,
    ) {
        if let Some(scene_node_id) = self.presentation_scene_node_id_for_root(root_surface_id) {
            self.presentation_animator.cancel_geometry(scene_node_id);
        }
    }

    pub(in crate::compositor) fn publish_presented_window_geometry(
        &mut self,
        frame_id: u64,
        geometry: PresentedWindowGeometry,
    ) {
        self.presented_presentation_frame_id = frame_id;
        self.presented_window_geometries = vec![geometry];
        self.advance_pointer_hit_generation();
    }

    pub(in crate::compositor) fn animate_toplevel_visual_geometry(
        &mut self,
        root_surface_id: u32,
        previous_geometry: WindowGeometry,
        target_geometry: WindowGeometry,
        kind: PresentationAnimationKind,
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
            if let Some(window_id) = self.window_id_for_surface(root_surface_id)
                && let Some(scene_node_id) = self.scene_node_id_for_window_group(window_id)
            {
                self.presentation_animator.cancel_geometry(scene_node_id);
            }
            return;
        };
        let Some(scene_node_id) = self.presentation_scene_node_id_for_root(root_surface_id) else {
            return;
        };
        if interaction_active {
            self.presentation_animator.cancel_geometry(scene_node_id);
            return;
        }
        let Some(previous) =
            self.presentation_rect_for_geometry(root_surface_id, previous_geometry)
        else {
            self.presentation_animator.cancel_geometry(scene_node_id);
            return;
        };
        let Some(target) = self.presentation_rect_for_geometry(root_surface_id, target_geometry)
        else {
            self.presentation_animator.cancel_geometry(scene_node_id);
            return;
        };
        let current_curve = self.animation_control.curve_for(kind);
        let Some(curve) = current_curve else {
            self.presentation_animator.cancel_geometry(scene_node_id);
            return;
        };
        let mutation = crate::presentation_animation::PresentationGeometryMutation::new(
            scene_node_id,
            previous,
            target,
            curve,
        );
        if self.layout_batch_depth > 0 {
            if let Some(pending) = self.pending_presentation_geometry_transaction.as_mut() {
                pending.upsert_geometry_mutation(mutation);
            }
            return;
        }
        let _ = self.presentation_animator.commit(
            crate::presentation_animation::PresentationTransactionRequest::geometry(
                now,
                vec![mutation],
            ),
        );
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
        let surface_scene_nodes_in_order = surfaces
            .iter()
            .map(|surface| {
                self.scene_node_id_for_surface(surface.surface_id)
                    .expect("active renderable surface has no canonical scene node")
            })
            .collect::<Vec<_>>();
        let surface_scene_nodes = surfaces
            .iter()
            .zip(surface_scene_nodes_in_order.iter().copied())
            .map(|(surface, node)| (surface.surface_id, node))
            .collect::<HashMap<_, _>>();
        let scene_node_indices = surface_scene_nodes_in_order
            .iter()
            .copied()
            .enumerate()
            .map(|(index, node)| (node, index))
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
        self.active_scene_view.surface_scene_nodes = surface_scene_nodes;
        self.active_scene_view.surface_scene_nodes_in_order = surface_scene_nodes_in_order;
        self.active_scene_view.scene_node_indices = scene_node_indices;
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

    pub(in crate::compositor) fn active_scene_surface_scene_nodes_in_order(
        &self,
    ) -> &[SceneNodeId] {
        self.active_scene_view.surface_scene_nodes_in_order()
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

    #[allow(dead_code)]
    pub(in crate::compositor) fn active_scene_node_for_surface(
        &self,
        surface_id: u32,
    ) -> Option<SceneNodeId> {
        self.active_scene_view.scene_node_id_for_surface(surface_id)
    }

    #[allow(dead_code)]
    pub(in crate::compositor) fn active_scene_surface_index_for_node(
        &self,
        scene_node_id: SceneNodeId,
    ) -> Option<usize> {
        self.active_scene_view
            .surface_index_for_scene_node(scene_node_id)
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
