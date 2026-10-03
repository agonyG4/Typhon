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

pub(in crate::compositor) struct CanonicalPresentationScene<'a> {
    pub(in crate::compositor) surfaces: Cow<'a, [RenderableSurface]>,
    pub(in crate::compositor) scene_node_ids: Cow<'a, [SceneNodeId]>,
    pub(in crate::compositor) presentation_owner_root_surface_ids: Cow<'a, [u32]>,
    pub(in crate::compositor) surface_origins: Cow<'a, [(i32, i32)]>,
    pub(in crate::compositor) popup_surface_ids: Cow<'a, [u32]>,
    pub(in crate::compositor) fullscreen_plan: FullscreenCompositionPlan,
    pub(in crate::compositor) visibility: FullscreenRenderPlanMetrics,
}

impl CanonicalPresentationScene<'_> {
    pub(in crate::compositor) fn surface_index(&self, surface_id: u32) -> Option<usize> {
        self.surfaces
            .iter()
            .position(|surface| surface.surface_id == surface_id)
    }

    pub(in crate::compositor) fn scene_node_for_surface(
        &self,
        surface_id: u32,
    ) -> Option<SceneNodeId> {
        self.scene_node_ids
            .get(self.surface_index(surface_id)?)
            .copied()
    }

    pub(in crate::compositor) fn owner_root_for_surface(&self, surface_id: u32) -> Option<u32> {
        self.presentation_owner_root_surface_ids
            .get(self.surface_index(surface_id)?)
            .copied()
    }

    pub(in crate::compositor) fn surface_order(&self, surface_id: u32) -> Option<u32> {
        u32::try_from(self.surface_index(surface_id)?).ok()
    }

    pub(in crate::compositor) fn visual_group_orders(&self) -> Vec<Option<u32>> {
        let mut orders = vec![None; self.surfaces.len()];
        for (group_order, group) in crate::compositor::render::visual_stack_groups(
            self.surfaces.as_ref(),
            self.popup_surface_ids.as_ref(),
        )
        .iter()
        .enumerate()
        {
            let Some(group_id) =
                crate::compositor::render::VisualStackGroup::id_for_order(group_order)
            else {
                continue;
            };
            for surface_index in group.surface_indices() {
                if let Some(order) = orders.get_mut(*surface_index) {
                    *order = Some(group_id.get());
                }
            }
        }
        orders
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
        self.refresh_presentation_feedback_eligibility();
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
            self.refresh_presentation_feedback_eligibility();
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
                buffer.format().is_opaque_rgb8888()
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
        let owner_occludes_underlays = mode.is_dominant()
            && self.fullscreen_owner_proves_full_occlusion(owner_root_surface_id, eligibility);
        let mut plan = FullscreenCompositionPlan {
            owner_root_surface_id: Some(owner_root_surface_id),
            mode,
            owner_occludes_underlays,
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
                    // Root counts are filled after composition underlays have
                    // been derived, so they report actual scene culling.
                }
            }
        }

        if !plan.owner_occludes_underlays {
            let owner_surface_index = self
                .active_scene_surfaces()
                .iter()
                .position(|surface| surface.surface_id == owner_root_surface_id)
                .expect("dominant fullscreen owner is present in the active scene");
            let mut seen_underlay_roots = HashSet::new();
            for surface in self
                .active_scene_surfaces()
                .iter()
                .take(owner_surface_index)
            {
                let root_surface_id = self.presentation_owner_root_for_surface(surface.surface_id);
                if !seen_underlay_roots.insert(root_surface_id) {
                    continue;
                }
                if matches!(
                    self.fullscreen_root_classification(owner_root_surface_id, root_surface_id),
                    FullscreenRootClassification::CulledByFullscreen(_)
                ) {
                    plan.composition_underlay_roots.push(root_surface_id);
                }
            }
        }

        let mut counted_roots = HashSet::new();
        for surface in self.active_scene_surfaces() {
            let root_surface_id = self.presentation_owner_root_for_surface(surface.surface_id);
            if !counted_roots.insert(root_surface_id)
                || plan.allows_composition_root(root_surface_id)
            {
                continue;
            }
            if self.layer_surfaces.contains_key(&root_surface_id) {
                plan.culled_layer_roots = plan.culled_layer_roots.saturating_add(1);
            } else if self.window_id_for_surface(root_surface_id).is_some() {
                plan.culled_application_roots = plan.culled_application_roots.saturating_add(1);
            }
        }

        plan.culled_surface_count = self
            .active_scene_surfaces()
            .iter()
            .filter(|surface| {
                !plan.allows_composition_root(
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
            && plan.allowed_layer_roots.is_empty()
            && plan.composition_underlay_roots.is_empty();
        plan
    }

    fn fullscreen_owner_proves_full_occlusion(
        &self,
        owner_root_surface_id: u32,
        eligibility: FullscreenPresentationEligibility,
    ) -> bool {
        if !eligibility.fully_opaque
            || !eligibility.exactly_covers_output
            || self.presentation_animation_pending_for_root(owner_root_surface_id)
        {
            return false;
        }

        let Some(geometry) = self.current_visual_root_window_geometry(owner_root_surface_id) else {
            return false;
        };
        if geometry.width != self.output_size.width
            || geometry.height != self.output_size.height
            || geometry.placement.root_mode != RootPlacementMode::Absolute
            || geometry.placement.local_x != 0
            || geometry.placement.local_y != 0
        {
            return false;
        }
        let Some(presented_geometry) =
            self.presented_visual_root_window_geometry(owner_root_surface_id)
        else {
            return false;
        };
        if presented_geometry.width != geometry.width
            || presented_geometry.height != geometry.height
            || presented_geometry.placement != geometry.placement
        {
            return false;
        }

        let Some(owner_surface) = self
            .active_scene_surfaces()
            .iter()
            .find(|surface| surface.surface_id == owner_root_surface_id)
        else {
            return false;
        };
        if owner_surface.visual_clip.is_some()
            || owner_surface.width != self.output_size.width
            || owner_surface.height != self.output_size.height
            || owner_surface
                .render_placement
                .is_some_and(|placement| placement != owner_surface.placement)
            || owner_surface.placement != SurfacePlacement::absolute_root_at(0, 0)
        {
            return false;
        }

        let Some(window_id) = self.window_id_for_surface(owner_root_surface_id) else {
            return false;
        };
        let Some(window) = self.window(window_id) else {
            return false;
        };
        if !window.canonical_opacity().is_opaque() || !window.canonical_clip().is_unbounded() {
            return false;
        }

        let Some(scene_node_id) = self.presentation_scene_node_id_for_root(owner_root_surface_id)
        else {
            return false;
        };
        // Sparse presentation properties in a published snapshot mean identity;
        // no snapshot means the currently presented state is unknown.
        if self.presented_presentation.is_none() {
            return false;
        }
        let geometry_identity =
            !self.presented_presentation_geometry_is_non_identity_for_scene_node(scene_node_id);
        let opacity_identity =
            !self.presented_presentation_opacity_is_non_identity_for_scene_node(scene_node_id);
        let clip_unbounded = self
            .presented_presentation_clip_for_scene_node(scene_node_id)
            .is_unbounded();
        geometry_identity
            && opacity_identity
            && clip_unbounded
            && self.fullscreen_owner_effects_preserve_full_occlusion(owner_root_surface_id)
    }

    fn fullscreen_owner_effects_preserve_full_occlusion(&self, owner_root_surface_id: u32) -> bool {
        let scene = self.resolved_effect_scene();
        let registry = self.trusted_effect_registry.current();
        let active_surfaces = self.active_scene_surfaces();
        let owner_visual_group = self.visual_group_for_surface(owner_root_surface_id);
        let output_bounds =
            crate::effects::EffectRect::new(0, 0, self.output_size.width, self.output_size.height)
                .expect("configured output size is nonzero");

        for instance in &scene.instances {
            let crate::compositor::EffectAnchor::ReplaceSurface(surface_id) = instance.anchor
            else {
                continue;
            };
            if !active_surfaces
                .iter()
                .any(|surface| surface.surface_id == surface_id)
                || instance.region.intersect_rect(output_bounds).is_empty()
            {
                continue;
            }

            let effect_root = self.root_surface_id_for_surface(surface_id);
            let affects_owner = if surface_id == owner_root_surface_id {
                true
            } else if instance.anchor_scope == crate::compositor::EffectAnchorScope::VisualGroup {
                if effect_root == owner_root_surface_id {
                    true
                } else {
                    let current_effect_group = self.visual_group_for_surface(surface_id);
                    match (
                        instance.visual_group,
                        current_effect_group,
                        owner_visual_group,
                    ) {
                        (Some(resolved), Some(current), Some(owner)) if resolved == current => {
                            current == owner
                        }
                        (Some(_), Some(_), Some(_)) => {
                            // A resolved group that disagrees with the active scene is
                            // ambiguous, so it cannot support an occlusion proof.
                            return false;
                        }
                        _ => {
                            // Missing group identity makes an active group-scoped
                            // replacement impossible to map reliably.
                            return false;
                        }
                    }
                }
            } else {
                false
            };
            if !affects_owner {
                continue;
            }

            let Some(program) = registry.effect_for_program(instance.program) else {
                // A replacement with missing trusted program metadata has no
                // formal opacity guarantee.
                return false;
            };
            if program.program.program.alpha_mode != crate::effects::EffectAlphaMode::Opaque {
                return false;
            }
            // The effect graph copies this trusted alpha mode to its final
            // composite pass, where Opaque forces output alpha to one.
        }

        true
    }

    fn fullscreen_root_classification(
        &self,
        owner_root_surface_id: u32,
        root_surface_id: u32,
    ) -> FullscreenRootClassification {
        if self.root_belongs_to_fullscreen_owner_family(owner_root_surface_id, root_surface_id) {
            return FullscreenRootClassification::OwnerFamily;
        }
        if self.surface_role(root_surface_id) == SurfaceRole::DragIcon {
            return FullscreenRootClassification::AllowedAboveFullscreen(
                FullscreenAboveFullscreenReason::DragIcon,
            );
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
        let wallpaper_culled = plan.mode.is_dominant()
            && self
                .active_scene_presentation_owner_roots_in_order()
                .iter()
                .any(|root_surface_id| {
                    self.layer_surfaces
                        .get(root_surface_id)
                        .is_some_and(|role| role.committed.layer == Layer::Background)
                        && !plan.allows_composition_root(*root_surface_id)
                });
        FullscreenRenderPlanMetrics {
            fullscreen_active: plan.owner_root_surface_id.is_some(),
            owner_root_surface_id: plan.owner_root_surface_id,
            fullscreen_composition_active: plan.mode.is_dominant(),
            fullscreen_transition_pending: plan
                .owner_root_surface_id
                .is_some_and(|owner| self.presentation_animation_pending_for_root(owner)),
            solitary_tree_active: plan.solitary_owner_only,
            culled_surface_count: plan.culled_surface_count,
            wallpaper_culled,
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
        self.native_frame_renderable_surfaces_with_composition_plan()
            .0
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
        let scene = self.canonical_presentation_scene();
        (
            scene.surfaces,
            scene.scene_node_ids,
            scene.fullscreen_plan,
            scene.visibility,
        )
    }

    pub(in crate::compositor) fn canonical_presentation_scene(
        &self,
    ) -> CanonicalPresentationScene<'_> {
        let surfaces: Cow<'_, [RenderableSurface]> = Cow::Borrowed(self.active_scene_surfaces());
        let scene_nodes: Cow<'_, [SceneNodeId]> =
            Cow::Borrowed(self.active_scene_surface_scene_nodes_in_order());
        let owner_roots: Cow<'_, [u32]> =
            Cow::Borrowed(self.active_scene_presentation_owner_roots_in_order());
        let surface_origins: Cow<'_, [(i32, i32)]> =
            Cow::Borrowed(self.active_scene_surface_origins());
        let popup_surface_ids: Cow<'_, [u32]> =
            Cow::Borrowed(self.active_scene_popup_surface_ids());
        debug_assert_eq!(surfaces.len(), scene_nodes.len());
        debug_assert_eq!(surfaces.len(), owner_roots.len());
        debug_assert_eq!(surfaces.len(), surface_origins.len());
        let eligibility = self.fullscreen_presentation_eligibility();
        let plan = self.fullscreen_composition_plan_for_eligibility(eligibility);
        let metrics = self.fullscreen_render_plan_metrics_for_plan(&plan, eligibility);
        if !plan.mode.is_dominant() {
            return CanonicalPresentationScene {
                surfaces,
                scene_node_ids: scene_nodes,
                presentation_owner_root_surface_ids: owner_roots,
                surface_origins,
                popup_surface_ids,
                fullscreen_plan: plan,
                visibility: metrics,
            };
        }

        let has_culled_surfaces = owner_roots
            .iter()
            .any(|owner_root| !plan.allows_composition_root(*owner_root));
        if !has_culled_surfaces {
            return CanonicalPresentationScene {
                surfaces,
                scene_node_ids: scene_nodes,
                presentation_owner_root_surface_ids: owner_roots,
                surface_origins,
                popup_surface_ids,
                fullscreen_plan: plan,
                visibility: metrics,
            };
        }

        let mut filtered_surfaces = Vec::with_capacity(surfaces.len());
        let mut filtered_scene_nodes = Vec::with_capacity(scene_nodes.len());
        let mut filtered_owner_roots = Vec::with_capacity(owner_roots.len());
        for index in 0..surfaces.len() {
            if plan.allows_composition_root(owner_roots[index]) {
                filtered_surfaces.push(surfaces[index].clone());
                filtered_scene_nodes.push(scene_nodes[index]);
                filtered_owner_roots.push(owner_roots[index]);
            }
        }
        let filtered_origins = render::surface_origins(&filtered_surfaces);
        CanonicalPresentationScene {
            surfaces: Cow::Owned(filtered_surfaces),
            scene_node_ids: Cow::Owned(filtered_scene_nodes),
            presentation_owner_root_surface_ids: Cow::Owned(filtered_owner_roots),
            surface_origins: Cow::Owned(filtered_origins),
            popup_surface_ids,
            fullscreen_plan: plan,
            visibility: metrics,
        }
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
        presentation_owner_root_surface_ids: &[u32],
        sample: &PresentationSceneSample,
    ) -> Cow<'a, [RenderableSurface]> {
        assert_eq!(
            surfaces.len(),
            presentation_owner_root_surface_ids.len(),
            "presentation owner roots must remain aligned with native frame surfaces"
        );
        if sample.transforms.is_empty() {
            return surfaces;
        }

        let mut transformed = surfaces.to_vec();
        let canonical_origins = render::surface_origins(surfaces.as_ref());
        let mut presented_origins = HashMap::new();
        for transform in &sample.transforms {
            for (index, (surface, owner_root_surface_id)) in surfaces
                .iter()
                .zip(presentation_owner_root_surface_ids.iter().copied())
                .enumerate()
            {
                if owner_root_surface_id != transform.root_surface_id {
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

    pub(in crate::compositor) fn presented_presentation_geometry_is_non_identity_for_scene_node(
        &self,
        scene_node_id: SceneNodeId,
    ) -> bool {
        self.presented_presentation_transform_for_scene_node(scene_node_id)
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
mod occlusion_proof_tests {
    use super::*;
    use crate::effects::{
        EFFECT_MANIFEST_VERSION, EffectAlphaMode, EffectDefinition, EffectFailurePolicy,
        EffectFrameDemand, EffectManifest, EffectNode, EffectNodeId, EffectOutsets, EffectProgram,
        EffectProgramId, EffectRect, EffectRegion, EffectSource, EffectWorkingSpace, MaskMode,
        MaskSpec,
    };
    use crate::presentation_animation::{
        AnimationTime, PresentationClip, PresentationClipRect, PresentationFrameSnapshot,
        PresentationGroupClip, PresentationGroupOpacity, PresentationGroupTransform,
        PresentationOpacity, PresentationRect, PresentationRevisionId,
        PresentationSampleTimeSource, PresentationSceneSample, PresentationTransactionId,
    };
    use crate::render_backend::buffer::{BufferSize, DrmFormat};
    use std::collections::BTreeMap;

    const UNDERLAY_ROOT: u32 = 941;
    const OWNER_ROOT: u32 = 942;

    fn fullscreen_owner_fixture() -> (CompositorState, WindowId, SceneNodeId, PresentationRect) {
        let mut state = CompositorState::default();
        let output_id = state
            .ensure_native_output_id()
            .expect("test output identity");
        let output_size = BufferSize::new(state.output_size.width, state.output_size.height)
            .expect("test output size");
        let underlay_window = WindowId::from_raw(41).expect("underlay window id");
        let owner_window = WindowId::from_raw(42).expect("owner window id");
        let underlay = super::super::desktop_window_tests::x11_scanout_surface(
            UNDERLAY_ROOT,
            output_size.width,
            output_size.height,
            SurfacePlacement::root_at(0, 0),
            DrmFormat::Xrgb8888,
        );
        let owner = super::super::desktop_window_tests::x11_scanout_surface(
            OWNER_ROOT,
            output_size.width,
            output_size.height,
            SurfacePlacement::absolute_root_at(0, 0),
            DrmFormat::Xrgb8888,
        );
        state.install_native_frame_test_scene(
            vec![underlay, owner],
            &[(UNDERLAY_ROOT, underlay_window), (OWNER_ROOT, owner_window)],
            Some(OWNER_ROOT),
        );

        let presentation = PresentationSceneSample::empty_for_output(
            output_id,
            AnimationTime::from_nanos(0),
            PresentationSampleTimeSource::ZeroFallback,
        );
        let targets = state.native_frame_presentation_targets(state.active_scene_surfaces());
        let presented_windows =
            state.presented_window_geometries_for_targets(&presentation, &targets);
        let snapshot = PresentationFrameSnapshot::from_sample_with_presented_windows(
            &presentation,
            presented_windows,
        );
        state.publish_presented_presentation(1, &snapshot);

        let owner_scene_node = state
            .presentation_scene_node_id_for_root(OWNER_ROOT)
            .expect("fullscreen owner scene node");
        let output_rect = PresentationRect::new(
            0.0,
            0.0,
            f64::from(output_size.width),
            f64::from(output_size.height),
        )
        .expect("test output rect");
        (state, owner_window, owner_scene_node, output_rect)
    }

    fn register_mask_program(
        state: &CompositorState,
        alpha_mode: EffectAlphaMode,
    ) -> EffectProgramId {
        let program_id = EffectProgramId::new(77).expect("effect program ID");
        let content_node = EffectNodeId::new(1).expect("content node ID");
        let masked_node = EffectNodeId::new(2).expect("mask node ID");
        let program = EffectProgram {
            id: program_id,
            nodes: vec![
                EffectNode::source(content_node, EffectSource::TargetContent),
                EffectNode::mask(
                    masked_node,
                    content_node,
                    MaskSpec {
                        mode: MaskMode::Alpha,
                    },
                ),
            ],
            output: masked_node,
            working_space: EffectWorkingSpace::LinearSrgb,
            alpha_mode,
            outsets: EffectOutsets::ZERO,
            frame_demand: EffectFrameDemand::OnDamage,
            failure_policy: EffectFailurePolicy::Passthrough,
        };
        let effect_name = match alpha_mode {
            EffectAlphaMode::Opaque => "test.fullscreen-opaque-mask",
            EffectAlphaMode::Preserve => "test.fullscreen-preserve-mask",
        }
        .to_owned();
        let manifest = EffectManifest {
            version: EFFECT_MANIFEST_VERSION,
            effects: BTreeMap::from([(
                effect_name.clone(),
                EffectDefinition {
                    name: effect_name,
                    program,
                    parameters: BTreeMap::new(),
                    shader_assets: Vec::new(),
                },
            )]),
        };
        state
            .trusted_effect_registry()
            .reload(manifest, |_| Ok(()))
            .expect("preserve-alpha effect registry generation");
        program_id
    }

    fn register_preserving_mask_program(state: &CompositorState) -> EffectProgramId {
        register_mask_program(state, EffectAlphaMode::Preserve)
    }

    fn owner_proves_full_occlusion(state: &CompositorState) -> bool {
        let eligibility = state.fullscreen_presentation_eligibility();
        state.fullscreen_owner_proves_full_occlusion(OWNER_ROOT, eligibility)
    }

    #[test]
    fn fullscreen_occlusion_proof_accepts_settled_xrgb_dmabuf_without_replacement_effect() {
        let (state, _, _, _) = fullscreen_owner_fixture();
        let eligibility = state.fullscreen_presentation_eligibility();

        assert!(
            eligibility.eligible,
            "fixture should prove XRGB/DMABUF opacity"
        );
        assert!(state.fullscreen_owner_proves_full_occlusion(OWNER_ROOT, eligibility));

        let plan = state.fullscreen_composition_plan();
        assert!(plan.owner_occludes_underlays);
        assert!(!plan.allows_composition_root(UNDERLAY_ROOT));
        assert!(!plan.allows_interaction_root(UNDERLAY_ROOT));
    }

    #[test]
    fn fullscreen_occlusion_proof_rejects_non_opaque_canonical_window_opacity() {
        let (mut state, owner_window, _, _) = fullscreen_owner_fixture();
        state
            .set_window_canonical_opacity(
                owner_window,
                PresentationOpacity::new(0.5).expect("valid opacity"),
                None,
            )
            .expect("canonical opacity update");

        assert!(!owner_proves_full_occlusion(&state));
    }

    #[test]
    fn fullscreen_occlusion_proof_rejects_bounded_canonical_clip() {
        let (mut state, owner_window, _, _) = fullscreen_owner_fixture();
        let clip = PresentationClipRect::new(0.0, 0.0, 640.0, 480.0).expect("bounded clip");
        state
            .set_window_canonical_clip(owner_window, PresentationClip::Rect(clip), None)
            .expect("canonical clip update");

        assert!(!owner_proves_full_occlusion(&state));
    }

    #[test]
    fn fullscreen_occlusion_proof_rejects_non_identity_presented_opacity() {
        let (mut state, _, scene_node, _) = fullscreen_owner_fixture();
        let mut snapshot = state
            .presented_presentation
            .clone()
            .expect("published presentation snapshot");
        snapshot
            .opacities
            .push(PresentationGroupOpacity::with_scene_node(
                scene_node,
                OWNER_ROOT,
                PresentationOpacity::new(0.5).expect("valid opacity"),
                None,
            ));
        state.publish_presented_presentation(2, &snapshot);

        assert!(!owner_proves_full_occlusion(&state));
    }

    #[test]
    fn fullscreen_occlusion_proof_rejects_bounded_presented_clip() {
        let (mut state, _, scene_node, _) = fullscreen_owner_fixture();
        let mut snapshot = state
            .presented_presentation
            .clone()
            .expect("published presentation snapshot");
        let clip = PresentationClipRect::new(0.0, 0.0, 640.0, 480.0).expect("bounded clip");
        snapshot.clips.push(PresentationGroupClip::with_scene_node(
            scene_node,
            OWNER_ROOT,
            PresentationClip::Rect(clip),
            Some(clip),
            None,
        ));
        state.publish_presented_presentation(2, &snapshot);

        assert!(!owner_proves_full_occlusion(&state));
    }

    #[test]
    fn fullscreen_occlusion_proof_rejects_non_identity_presented_geometry() {
        let (mut state, _, scene_node, canonical_rect) = fullscreen_owner_fixture();
        let mut snapshot = state
            .presented_presentation
            .clone()
            .expect("published presentation snapshot");
        let presented_rect = PresentationRect::new(
            canonical_rect.x() + 1.0,
            canonical_rect.y(),
            canonical_rect.width(),
            canonical_rect.height(),
        )
        .expect("translated presentation rect");
        snapshot
            .transforms
            .push(PresentationGroupTransform::with_scene_node(
                scene_node,
                OWNER_ROOT,
                PresentationTransactionId::from_raw(1).expect("transaction ID"),
                PresentationRevisionId::from_raw(1).expect("revision ID"),
                canonical_rect,
                presented_rect,
                false,
            ));
        state.publish_presented_presentation(2, &snapshot);

        assert!(!owner_proves_full_occlusion(&state));
    }

    #[test]
    fn fullscreen_occlusion_proof_rejects_missing_published_presentation_snapshot() {
        let (mut state, _, _, _) = fullscreen_owner_fixture();
        state.presented_presentation = None;

        assert!(!owner_proves_full_occlusion(&state));
    }

    #[test]
    fn fullscreen_occlusion_proof_rejects_missing_replacement_program_metadata() {
        let (mut state, _, _, _) = fullscreen_owner_fixture();
        assert!(
            state.set_internal_surface_effect(
                OWNER_ROOT,
                EffectAnchor::ReplaceSurface(OWNER_ROOT),
                EffectProgramId::new(98).expect("unknown program ID"),
                EffectRegion::from_rect(
                    EffectRect::new(0, 0, state.output_size.width, state.output_size.height)
                        .expect("replacement region"),
                ),
            )
        );

        assert!(!owner_proves_full_occlusion(&state));
    }

    #[test]
    fn fullscreen_occlusion_proof_accepts_trusted_opaque_replacement_effect() {
        let (mut state, _, _, _) = fullscreen_owner_fixture();
        let program = register_mask_program(&state, EffectAlphaMode::Opaque);
        let registry = state.trusted_effect_registry().current();
        let registered = registry
            .effect_for_program(program)
            .expect("trusted opaque effect");
        assert_eq!(
            registered.program.program.alpha_mode,
            EffectAlphaMode::Opaque
        );
        assert!(
            state.set_internal_surface_effect(
                OWNER_ROOT,
                EffectAnchor::ReplaceSurface(OWNER_ROOT),
                program,
                EffectRegion::from_rect(
                    EffectRect::new(0, 0, state.output_size.width, state.output_size.height)
                        .expect("replacement region"),
                ),
            )
        );

        assert!(owner_proves_full_occlusion(&state));
    }

    #[test]
    fn fullscreen_occlusion_proof_ignores_replacement_effect_in_other_visual_group() {
        let (mut state, _, _, _) = fullscreen_owner_fixture();
        let program = register_preserving_mask_program(&state);
        assert!(
            state.set_internal_surface_effect(
                UNDERLAY_ROOT,
                EffectAnchor::ReplaceSurface(UNDERLAY_ROOT),
                program,
                EffectRegion::from_rect(
                    EffectRect::new(0, 0, state.output_size.width, state.output_size.height)
                        .expect("underlay replacement region"),
                ),
            )
        );

        assert!(owner_proves_full_occlusion(&state));
        let plan = state.fullscreen_composition_plan();
        assert!(plan.owner_occludes_underlays);
        assert!(!plan.allows_composition_root(UNDERLAY_ROOT));
    }

    #[test]
    fn fullscreen_occlusion_proof_ignores_before_after_and_output_postprocess_effects() {
        for anchor in [
            EffectAnchor::BeforeSurface(OWNER_ROOT),
            EffectAnchor::AfterSurface(OWNER_ROOT),
            EffectAnchor::OutputPostProcess,
        ] {
            let (mut state, _, _, _) = fullscreen_owner_fixture();
            let program = register_preserving_mask_program(&state);
            assert!(
                state.set_internal_surface_effect(
                    OWNER_ROOT,
                    anchor,
                    program,
                    EffectRegion::from_rect(
                        EffectRect::new(0, 0, state.output_size.width, state.output_size.height)
                            .expect("effect region"),
                    ),
                )
            );

            assert!(owner_proves_full_occlusion(&state));
        }
    }

    #[test]
    fn fullscreen_occlusion_proof_rejects_preserve_alpha_replacement_effect() {
        let (mut state, _, _, _) = fullscreen_owner_fixture();
        let program = register_preserving_mask_program(&state);
        let registry = state.trusted_effect_registry().current();
        let registered = registry
            .effect_for_program(program)
            .expect("trusted preserve-alpha effect");
        assert_eq!(
            registered.program.program.alpha_mode,
            EffectAlphaMode::Preserve
        );
        assert!(
            state.set_internal_surface_effect(
                OWNER_ROOT,
                EffectAnchor::ReplaceSurface(OWNER_ROOT),
                program,
                EffectRegion::from_rect(
                    EffectRect::new(0, 0, state.output_size.width, state.output_size.height)
                        .expect("replacement region"),
                ),
            )
        );

        let eligibility = state.fullscreen_presentation_eligibility();
        assert!(eligibility.fully_opaque);
        assert!(eligibility.exactly_covers_output);
        assert!(
            !state.fullscreen_owner_proves_full_occlusion(OWNER_ROOT, eligibility),
            "an alpha-preserving replacement can introduce transparent output"
        );

        let plan = state.fullscreen_composition_plan();
        assert!(!plan.owner_occludes_underlays);
        assert!(plan.allows_composition_root(UNDERLAY_ROOT));
        assert!(!plan.allows_interaction_root(UNDERLAY_ROOT));
        let scanout = state.direct_scanout_scene_analysis();
        assert!(scanout.candidate.is_none());
        assert!(scanout.blockers.reasons().contains(
            &crate::compositor::direct_scanout::DirectScanoutSceneRejection::FullscreenUnderlayVisible
        ));
    }
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
