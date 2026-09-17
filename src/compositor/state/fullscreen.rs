use super::*;
use crate::compositor::direct_scanout::direct_scanout_viewport_compatibility;
use crate::compositor::{SurfaceContentType, SurfacePresentationMetadata};
use std::borrow::Cow;

fn select_fullscreen_root_content_type(
    owner_root_surface_id: u32,
    surface_id: u32,
    candidate: SurfaceContentType,
    current: SurfaceContentType,
) -> SurfaceContentType {
    if surface_id == owner_root_surface_id && candidate != SurfaceContentType::None {
        candidate
    } else {
        current
    }
}

impl CompositorState {
    pub(in crate::compositor) fn fullscreen_tree_presentation_metadata(
        &self,
    ) -> Option<SurfacePresentationMetadata> {
        let owner = self.fullscreen_presentation?;
        let mut metadata = SurfacePresentationMetadata::default();
        for surface in self.active_scene_surfaces() {
            if self.root_surface_id_for_surface(surface.surface_id) != owner.owner_root_surface_id {
                continue;
            }
            let Some(surface_metadata) = self
                .surface_resources
                .get(&surface.surface_id)
                .and_then(|surface| surface.data::<SurfaceData>())
                .map(|data| data.current_presentation())
            else {
                continue;
            };
            if surface_metadata.hint.is_async() {
                metadata.hint = surface_metadata.hint;
            }
            metadata.content_type = select_fullscreen_root_content_type(
                owner.owner_root_surface_id,
                surface.surface_id,
                surface_metadata.content_type,
                metadata.content_type,
            );
        }
        Some(metadata)
    }

    pub(in crate::compositor) fn window_geometry_for_mode(
        &self,
        mode: ToplevelMode,
    ) -> WindowGeometry {
        match mode {
            ToplevelMode::Normal => WindowGeometry::new(
                SurfacePlacement::root(),
                self.output_size.width,
                self.output_size.height,
            ),
            ToplevelMode::Maximized => self.maximized_window_geometry(),
            ToplevelMode::Fullscreen => self.fullscreen_window_geometry(),
        }
    }

    pub(in crate::compositor) fn window_geometry_for_surface_mode(
        &self,
        surface_id: u32,
        mode: ToplevelMode,
    ) -> WindowGeometry {
        if mode == ToplevelMode::Maximized
            && self.surface_uses_server_side_decorations(surface_id, mode)
        {
            let usable = self.usable_output_geometry();
            let titlebar = self.decoration_theme.metrics().titlebar_height as f64;
            let titlebar = titlebar.min(usable.height).max(0.0) as u32;
            return WindowGeometry::new(
                SurfacePlacement::absolute_root_at(
                    usable.x as i32,
                    (usable.y + f64::from(titlebar)) as i32,
                ),
                usable.width as u32,
                (usable.height as u32).saturating_sub(titlebar),
            );
        }
        self.window_geometry_for_mode(mode)
    }

    pub(in crate::compositor) fn maximized_window_geometry(&self) -> WindowGeometry {
        let usable = self.usable_output_geometry();
        WindowGeometry::new(
            SurfacePlacement::absolute_root_at(usable.x as i32, usable.y as i32),
            usable.width as u32,
            usable.height as u32,
        )
    }

    pub(in crate::compositor) fn fullscreen_window_geometry(&self) -> WindowGeometry {
        WindowGeometry::new(
            SurfacePlacement::absolute_root_at(0, 0),
            self.output_size.width,
            self.output_size.height,
        )
    }

    pub(in crate::compositor) fn set_fullscreen_presentation_owner(&mut self, surface_id: u32) {
        let event = if self
            .fullscreen_presentation
            .is_some_and(|owner| owner.owner_root_surface_id == surface_id)
        {
            "fullscreen_owner_refreshed"
        } else {
            "fullscreen_owner_set"
        };
        self.fullscreen_presentation = Some(FullscreenPresentationState {
            owner_root_surface_id: surface_id,
            output_width: self.output_size.width,
            output_height: self.output_size.height,
        });
        if crate::compositor::fullscreen::fullscreen_trace_enabled() {
            eprintln!(
                "oblivion-one fullscreen: event={event} root_surface_id={surface_id} reason=authoritative_mode_state"
            );
        }
    }

    pub(in crate::compositor) fn clear_fullscreen_presentation_owner(&mut self, surface_id: u32) {
        if self
            .fullscreen_presentation
            .is_some_and(|owner| owner.owner_root_surface_id == surface_id)
        {
            self.fullscreen_presentation = None;
            if crate::compositor::fullscreen::fullscreen_trace_enabled() {
                eprintln!(
                    "oblivion-one fullscreen: event=fullscreen_owner_cleared root_surface_id={surface_id} reason=authoritative_mode_or_lifecycle_transition"
                );
            }
        }
    }

    pub(in crate::compositor) fn refresh_fullscreen_presentation_owner(&mut self, surface_id: u32) {
        if self
            .fullscreen_presentation
            .is_some_and(|owner| owner.owner_root_surface_id == surface_id)
        {
            self.set_fullscreen_presentation_owner(surface_id);
        }
    }

    pub(in crate::compositor) fn fullscreen_presentation_eligibility(
        &self,
    ) -> FullscreenPresentationEligibility {
        let Some(owner) = self.fullscreen_presentation else {
            return FullscreenPresentationEligibility {
                owner: None,
                eligible: false,
                rejection: Some(FullscreenPresentationRejection::NoFullscreenOwner),
                fully_opaque: false,
                exactly_covers_output: false,
                overlays_visible: false,
                software_cursor_visible: false,
            };
        };
        let owner_has_toplevel = self
            .toplevel_surfaces
            .contains_key(&owner.owner_root_surface_id)
            || self
                .window_by_root_surface
                .contains_key(&owner.owner_root_surface_id);
        if !owner_has_toplevel {
            return FullscreenPresentationEligibility {
                owner: Some(owner),
                eligible: false,
                rejection: Some(FullscreenPresentationRejection::OwnerMissing),
                fully_opaque: false,
                exactly_covers_output: false,
                overlays_visible: false,
                software_cursor_visible: false,
            };
        };
        if !self.surface_is_visible_in_active_scene(owner.owner_root_surface_id) {
            return FullscreenPresentationEligibility {
                owner: Some(owner),
                eligible: false,
                rejection: Some(FullscreenPresentationRejection::OwnerMinimized),
                fully_opaque: false,
                exactly_covers_output: false,
                overlays_visible: false,
                software_cursor_visible: false,
            };
        }
        if self
            .toplevel_window_state(owner.owner_root_surface_id)
            .is_some_and(WindowState::is_minimized)
        {
            return FullscreenPresentationEligibility {
                owner: Some(owner),
                eligible: false,
                rejection: Some(FullscreenPresentationRejection::OwnerMinimized),
                fully_opaque: false,
                exactly_covers_output: false,
                overlays_visible: false,
                software_cursor_visible: false,
            };
        }
        let geometry = self
            .current_visual_root_window_geometry(owner.owner_root_surface_id)
            .unwrap_or_else(|| self.fullscreen_window_geometry());
        let exactly_covers_output = geometry.width == self.output_size.width
            && geometry.height == self.output_size.height
            && geometry.placement.root_mode == RootPlacementMode::Absolute
            && geometry.placement.local_x == 0
            && geometry.placement.local_y == 0;
        let overlays_visible = self.visible_fullscreen_overlay_count() > 0;
        let root = self
            .active_scene_surfaces()
            .iter()
            .find(|surface| surface.surface_id == owner.owner_root_surface_id);
        let viewport_compatibility = root.and_then(|surface| {
            surface.dmabuf_handle().map(|buffer| {
                direct_scanout_viewport_compatibility(
                    buffer.size(),
                    BufferSize::new(self.output_size.width, self.output_size.height)
                        .expect("configured output size is nonzero"),
                    surface.buffer_scale,
                    surface.buffer_transform,
                    surface.viewport_source,
                    surface.viewport_destination,
                )
            })
        });
        let transform_or_scale_compatible = viewport_compatibility
            .as_ref()
            .is_some_and(|compatibility| compatibility.is_ok());
        let fully_opaque = root
            .and_then(RenderableSurface::dmabuf_handle)
            .is_some_and(|buffer| {
                buffer.format() == DrmFormat::Xrgb8888
                    && buffer.size().width == self.output_size.width
                    && buffer.size().height == self.output_size.height
            })
            && root.is_some_and(|surface| {
                surface.visual_clip.is_none()
                    && surface
                        .render_placement
                        .is_none_or(|placement| placement == surface.placement)
                    && surface.placement == SurfacePlacement::absolute_root_at(0, 0)
            })
            && transform_or_scale_compatible;
        let software_cursor_visible = false;
        let rejection = if !exactly_covers_output {
            Some(FullscreenPresentationRejection::OwnerDoesNotCoverOutput)
        } else if overlays_visible {
            Some(FullscreenPresentationRejection::OverlayVisible)
        } else if !transform_or_scale_compatible {
            Some(FullscreenPresentationRejection::TransformOrScaleIncompatible)
        } else if !fully_opaque {
            Some(FullscreenPresentationRejection::OwnerOpacityUnknown)
        } else if software_cursor_visible {
            Some(FullscreenPresentationRejection::SoftwareCursorVisible)
        } else {
            None
        };
        FullscreenPresentationEligibility {
            owner: Some(owner),
            eligible: rejection.is_none(),
            rejection,
            fully_opaque,
            exactly_covers_output,
            overlays_visible,
            software_cursor_visible,
        }
    }

    pub(in crate::compositor) fn fullscreen_composition_plan(&self) -> FullscreenCompositionPlan {
        let eligibility = self.fullscreen_presentation_eligibility();
        self.fullscreen_composition_plan_for_eligibility(eligibility)
    }

    fn fullscreen_composition_plan_for_eligibility(
        &self,
        eligibility: FullscreenPresentationEligibility,
    ) -> FullscreenCompositionPlan {
        let Some(owner) = eligibility.owner else {
            return FullscreenCompositionPlan::default();
        };
        let owner_root_surface_id = owner.owner_root_surface_id;
        let owner_present = self
            .active_scene_surfaces()
            .iter()
            .any(|surface| surface.surface_id == owner_root_surface_id);
        let owner_visible = self.surface_is_visible_in_active_scene(owner_root_surface_id)
            && !self
                .toplevel_window_state(owner_root_surface_id)
                .is_some_and(WindowState::is_minimized);
        let transition_pending =
            self.presentation_animation_pending_for_root(owner_root_surface_id);
        let mode = if owner_present
            && owner_visible
            && eligibility.exactly_covers_output
            && !transition_pending
        {
            FullscreenCompositionMode::Dominant
        } else {
            FullscreenCompositionMode::Transitioning
        };
        let mut plan = FullscreenCompositionPlan {
            owner_root_surface_id: Some(owner_root_surface_id),
            mode,
            ..FullscreenCompositionPlan::default()
        };
        if !mode.is_dominant() {
            return plan;
        }

        let mut seen_roots = HashSet::new();
        for surface in self.active_scene_surfaces() {
            let root_surface_id = self.presentation_owner_root_for_surface(surface.surface_id);
            if !seen_roots.insert(root_surface_id) {
                continue;
            }
            match self.fullscreen_root_classification(owner_root_surface_id, root_surface_id) {
                FullscreenRootClassification::OwnerFamily => {
                    plan.owner_family_roots.push(root_surface_id);
                }
                FullscreenRootClassification::AllowedAboveFullscreen(reason) => {
                    if self.layer_surfaces.contains_key(&root_surface_id) {
                        plan.allowed_layer_roots.push(root_surface_id);
                    } else {
                        plan.allowed_application_roots.push(root_surface_id);
                    }
                    plan.above_fullscreen_reason.get_or_insert(reason);
                }
                FullscreenRootClassification::CulledByFullscreen(_) => {
                    if self.layer_surfaces.contains_key(&root_surface_id) {
                        plan.culled_layer_roots = plan.culled_layer_roots.saturating_add(1);
                    } else if self.window_id_for_surface(root_surface_id).is_some() {
                        plan.culled_application_roots =
                            plan.culled_application_roots.saturating_add(1);
                    }
                }
            }
        }

        plan.culled_surface_count = self
            .active_scene_surfaces()
            .iter()
            .filter(|surface| {
                !plan.allows_presentation_root(
                    self.presentation_owner_root_for_surface(surface.surface_id),
                )
            })
            .count();
        let has_additional_owner_family = self.active_scene_surfaces().iter().any(|surface| {
            let root_surface_id = self.presentation_owner_root_for_surface(surface.surface_id);
            root_surface_id != owner_root_surface_id
                && plan.owner_family_roots.contains(&root_surface_id)
        });
        let popup_visible = !self.active_scene_popup_surface_ids().is_empty();
        plan.solitary_owner_only = !has_additional_owner_family
            && !popup_visible
            && plan.allowed_application_roots.is_empty()
            && plan.allowed_layer_roots.is_empty();
        plan
    }

    fn fullscreen_root_classification(
        &self,
        owner_root_surface_id: u32,
        root_surface_id: u32,
    ) -> FullscreenRootClassification {
        if self.root_belongs_to_fullscreen_owner_family(owner_root_surface_id, root_surface_id) {
            return FullscreenRootClassification::OwnerFamily;
        }
        if let Some(role) = self.layer_surfaces.get(&root_surface_id) {
            return match role.committed.layer {
                Layer::Overlay => FullscreenRootClassification::AllowedAboveFullscreen(
                    FullscreenAboveFullscreenReason::LayerOverlay,
                ),
                Layer::Background => FullscreenRootClassification::CulledByFullscreen(
                    FullscreenCulledRootReason::LayerBackground,
                ),
                Layer::Bottom => FullscreenRootClassification::CulledByFullscreen(
                    FullscreenCulledRootReason::LayerBottom,
                ),
                Layer::Top => FullscreenRootClassification::CulledByFullscreen(
                    FullscreenCulledRootReason::LayerTop,
                ),
            };
        }
        let Some(window_id) = self.window_id_for_surface(root_surface_id) else {
            return FullscreenRootClassification::CulledByFullscreen(
                FullscreenCulledRootReason::GlobalContent,
            );
        };
        if matches!(
            self.scene_work_owner_for_window(window_id),
            SceneWorkOwner::Location(crate::wm::WorkspaceLocation::Special(_))
        ) {
            return FullscreenRootClassification::AllowedAboveFullscreen(
                FullscreenAboveFullscreenReason::SpecialWorkspaceApplication,
            );
        }
        let stack_layer = self
            .window(window_id)
            .map(|window| window.stack_layer)
            .unwrap_or(DesktopStackLayer::Normal);
        match stack_layer {
            DesktopStackLayer::Notification => {
                FullscreenRootClassification::AllowedAboveFullscreen(
                    FullscreenAboveFullscreenReason::ApplicationNotification,
                )
            }
            DesktopStackLayer::Overlay => FullscreenRootClassification::AllowedAboveFullscreen(
                FullscreenAboveFullscreenReason::ApplicationOverlay,
            ),
            DesktopStackLayer::Above => FullscreenRootClassification::AllowedAboveFullscreen(
                FullscreenAboveFullscreenReason::ApplicationAbove,
            ),
            DesktopStackLayer::Popup => FullscreenRootClassification::CulledByFullscreen(
                FullscreenCulledRootReason::OrdinaryApplicationPopup,
            ),
            DesktopStackLayer::Normal => FullscreenRootClassification::CulledByFullscreen(
                FullscreenCulledRootReason::RegularApplication,
            ),
        }
    }

    fn root_belongs_to_fullscreen_owner_family(
        &self,
        owner_root_surface_id: u32,
        root_surface_id: u32,
    ) -> bool {
        if owner_root_surface_id == root_surface_id {
            return true;
        }
        let Some(owner_window_id) = self.window_id_for_surface(owner_root_surface_id) else {
            return false;
        };
        let Some(candidate_window_id) = self.window_id_for_surface(root_surface_id) else {
            return false;
        };
        let owner_id = self
            .canonical_scene_owner_window_id(owner_window_id)
            .unwrap_or(owner_window_id);
        let candidate_id = self
            .canonical_scene_owner_window_id(candidate_window_id)
            .unwrap_or(candidate_window_id);
        owner_id == candidate_id
    }

    fn fullscreen_render_plan_metrics_for_plan(
        &self,
        plan: &FullscreenCompositionPlan,
        eligibility: FullscreenPresentationEligibility,
    ) -> FullscreenRenderPlanMetrics {
        FullscreenRenderPlanMetrics {
            fullscreen_active: plan.owner_root_surface_id.is_some(),
            owner_root_surface_id: plan.owner_root_surface_id,
            fullscreen_composition_active: plan.mode.is_dominant(),
            fullscreen_transition_pending: plan
                .owner_root_surface_id
                .is_some_and(|owner| self.presentation_animation_pending_for_root(owner)),
            solitary_tree_active: plan.solitary_owner_only,
            culled_surface_count: plan.culled_surface_count,
            wallpaper_culled: plan.mode.is_dominant(),
            visible_overlay_count: self.visible_fullscreen_overlay_count(),
            fullscreen_allowed_application_roots: plan.allowed_application_roots.len(),
            fullscreen_allowed_layer_roots: plan.allowed_layer_roots.len(),
            fullscreen_culled_application_roots: plan.culled_application_roots,
            fullscreen_culled_layer_roots: plan.culled_layer_roots,
            fullscreen_above_reason: plan.above_fullscreen_reason,
            rejection: eligibility.rejection,
        }
    }

    pub(in crate::compositor) fn fullscreen_render_plan_metrics(
        &self,
    ) -> FullscreenRenderPlanMetrics {
        let eligibility = self.fullscreen_presentation_eligibility();
        let plan = self.fullscreen_composition_plan_for_eligibility(eligibility);
        self.fullscreen_render_plan_metrics_for_plan(&plan, eligibility)
    }

    pub(in crate::compositor) fn native_frame_renderable_surfaces(
        &self,
    ) -> Cow<'_, [RenderableSurface]> {
        self.native_frame_renderable_surfaces_with_metrics().0
    }

    pub(in crate::compositor) fn native_frame_renderable_surfaces_with_metrics(
        &self,
    ) -> (Cow<'_, [RenderableSurface]>, FullscreenRenderPlanMetrics) {
        let (surfaces, _, metrics) = self.native_frame_renderable_surfaces_with_composition_plan();
        (surfaces, metrics)
    }

    pub(in crate::compositor) fn native_frame_renderable_surfaces_with_scene_nodes_and_composition_plan(
        &self,
    ) -> (
        Cow<'_, [RenderableSurface]>,
        Cow<'_, [SceneNodeId]>,
        FullscreenCompositionPlan,
        FullscreenRenderPlanMetrics,
    ) {
        let surfaces: Cow<'_, [RenderableSurface]> = Cow::Borrowed(self.active_scene_surfaces());
        let scene_nodes: Cow<'_, [SceneNodeId]> =
            Cow::Borrowed(self.active_scene_surface_scene_nodes_in_order());
        debug_assert_eq!(surfaces.len(), scene_nodes.len());
        let eligibility = self.fullscreen_presentation_eligibility();
        let plan = self.fullscreen_composition_plan_for_eligibility(eligibility);
        let metrics = self.fullscreen_render_plan_metrics_for_plan(&plan, eligibility);
        if !plan.mode.is_dominant() {
            return (surfaces, scene_nodes, plan, metrics);
        }

        let mut filtered_surfaces = Vec::with_capacity(surfaces.len());
        let mut filtered_scene_nodes = Vec::with_capacity(scene_nodes.len());
        for (surface, scene_node) in surfaces.iter().zip(scene_nodes.iter().copied()) {
            if plan.allows_presentation_root(
                self.presentation_owner_root_for_surface(surface.surface_id),
            ) {
                filtered_surfaces.push(surface.clone());
                filtered_scene_nodes.push(scene_node);
            }
        }
        (
            Cow::Owned(filtered_surfaces),
            Cow::Owned(filtered_scene_nodes),
            plan,
            metrics,
        )
    }

    pub(in crate::compositor) fn native_frame_renderable_surfaces_with_composition_plan(
        &self,
    ) -> (
        Cow<'_, [RenderableSurface]>,
        FullscreenCompositionPlan,
        FullscreenRenderPlanMetrics,
    ) {
        let (surfaces, _, plan, metrics) =
            self.native_frame_renderable_surfaces_with_scene_nodes_and_composition_plan();
        (surfaces, plan, metrics)
    }

    pub(in crate::compositor) fn apply_presentation_to_native_frame_surfaces<'a>(
        &self,
        surfaces: Cow<'a, [RenderableSurface]>,
        sample: &PresentationSceneSample,
    ) -> Cow<'a, [RenderableSurface]> {
        if sample.transforms.is_empty() {
            return surfaces;
        }

        let mut transformed = surfaces.to_vec();
        let canonical_origins = render::surface_origins(surfaces.as_ref());
        let mut presented_origins = HashMap::new();
        for transform in &sample.transforms {
            for (index, surface) in surfaces.iter().enumerate() {
                if self.root_surface_id_for_surface(surface.surface_id) != transform.root_surface_id
                {
                    continue;
                }
                let Some(origin) = canonical_origins.get(index).copied() else {
                    continue;
                };
                let Some(canonical_rect) = PresentationRect::new(
                    f64::from(origin.0),
                    f64::from(origin.1),
                    f64::from(surface.width),
                    f64::from(surface.height),
                ) else {
                    continue;
                };
                let Some(presented_rect) = transform.map_rect(canonical_rect) else {
                    continue;
                };
                presented_origins.insert(surface.surface_id, presented_rect);
            }
        }

        for surface in &mut transformed {
            let Some(presented_rect) = presented_origins.get(&surface.surface_id).copied() else {
                continue;
            };
            let presented_x = saturating_i32_from_f64(presented_rect.x().round());
            let presented_y = saturating_i32_from_f64(presented_rect.y().round());
            let presented_width = saturating_u32_from_f64(presented_rect.width().round());
            let presented_height = saturating_u32_from_f64(presented_rect.height().round());
            let parent_id = surface.placement.parent_surface_id.or_else(|| {
                surface
                    .render_placement
                    .and_then(|placement| placement.parent_surface_id)
            });
            surface.render_placement = Some(match parent_id {
                Some(parent_id) => {
                    let parent_origin = presented_origins
                        .get(&parent_id)
                        .copied()
                        .unwrap_or(presented_rect);
                    SurfacePlacement::subsurface(
                        parent_id,
                        presented_x
                            .saturating_sub(saturating_i32_from_f64(parent_origin.x().round()))
                            .saturating_sub(surface.x),
                        presented_y
                            .saturating_sub(saturating_i32_from_f64(parent_origin.y().round()))
                            .saturating_sub(surface.y),
                    )
                }
                None => SurfacePlacement::absolute_root_at(
                    presented_x.saturating_sub(surface.x),
                    presented_y.saturating_sub(surface.y),
                ),
            });
            surface.render_target_size = BufferSize::new(presented_width, presented_height);
        }
        Cow::Owned(transformed)
    }

    pub(in crate::compositor) fn presentation_geometry_signature(
        &self,
        sample: &PresentationSceneSample,
    ) -> u64 {
        sample.geometry_signature()
    }

    pub(in crate::compositor) fn presented_presentation_is_non_identity(
        &self,
        root_surface_id: u32,
    ) -> bool {
        self.presented_presentation_transform(root_surface_id)
            .is_some_and(|transform| !transform.is_identity())
    }

    fn visible_fullscreen_overlay_count(&self) -> usize {
        self.layer_surfaces
            .values()
            .filter(|role| role.mapped && role.committed.layer == Layer::Overlay)
            .count()
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

#[cfg(test)]
mod root_content_type_tests {
    use super::*;

    fn select(root: SurfaceContentType, child: SurfaceContentType) -> SurfaceContentType {
        let after_root = select_fullscreen_root_content_type(1, 1, root, SurfaceContentType::None);
        select_fullscreen_root_content_type(1, 2, child, after_root)
    }

    #[test]
    fn child_content_does_not_replace_root_none() {
        assert_eq!(
            select(SurfaceContentType::None, SurfaceContentType::Game),
            SurfaceContentType::None
        );
    }

    #[test]
    fn root_video_wins_over_child_game() {
        assert_eq!(
            select(SurfaceContentType::Video, SurfaceContentType::Game),
            SurfaceContentType::Video
        );
    }

    #[test]
    fn root_game_is_preserved_when_child_is_none() {
        assert_eq!(
            select(SurfaceContentType::Game, SurfaceContentType::None),
            SurfaceContentType::Game
        );
    }
}
