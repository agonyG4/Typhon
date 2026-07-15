use super::*;
use crate::render_backend::buffer::SurfaceBufferSource;
use std::borrow::Cow;

impl CompositorState {
    pub(in crate::compositor) fn window_geometry_for_mode(
        &self,
        mode: ToplevelMode,
    ) -> WindowGeometry {
        match mode {
            ToplevelMode::Floating => WindowGeometry::new(
                SurfacePlacement::root(),
                self.output_size.width,
                self.output_size.height,
            ),
            ToplevelMode::Maximized => self.maximized_window_geometry(),
            ToplevelMode::Fullscreen => self.fullscreen_window_geometry(),
        }
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
        self.fullscreen_presentation = Some(FullscreenPresentationState {
            owner_root_surface_id: surface_id,
            output_width: self.output_size.width,
            output_height: self.output_size.height,
        });
    }

    pub(in crate::compositor) fn clear_fullscreen_presentation_owner(&mut self, surface_id: u32) {
        if self
            .fullscreen_presentation
            .is_some_and(|owner| owner.owner_root_surface_id == surface_id)
        {
            self.fullscreen_presentation = None;
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
        let Some(toplevel) = self.toplevel_surfaces.get(&owner.owner_root_surface_id) else {
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
        if toplevel.window.is_minimized() {
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
            .renderable_surfaces
            .iter()
            .find(|surface| surface.surface_id == owner.owner_root_surface_id);
        let transform_or_scale_compatible = root.is_some_and(|surface| {
            surface.buffer_scale == 1
                && surface.buffer_transform == wl_output::Transform::Normal
                && surface.viewport_source.is_none()
                && surface.viewport_destination.is_none()
        });
        let fully_opaque = root
            .and_then(RenderableSurface::dmabuf_handle)
            .is_some_and(|buffer| {
                buffer.format() == DrmFormat::Xrgb8888
                    && buffer.size().width == self.output_size.width
                    && buffer.size().height == self.output_size.height
            })
            && root.is_some_and(|surface| {
                surface.visual_clip.is_none()
                    && surface.render_placement.is_none()
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

    pub(in crate::compositor) fn direct_scanout_scene_candidate(
        &self,
    ) -> Result<DirectScanoutSceneCandidate, DirectScanoutSceneRejection> {
        let owner = self
            .fullscreen_presentation
            .ok_or(DirectScanoutSceneRejection::NoFullscreenOwner)?;
        let toplevel = self
            .toplevel_surfaces
            .get(&owner.owner_root_surface_id)
            .ok_or(DirectScanoutSceneRejection::OwnerMissing)?;
        if toplevel.window.is_minimized() {
            return Err(DirectScanoutSceneRejection::OwnerMinimized);
        }

        let popup_visible = self
            .popup_nodes
            .values()
            .any(|node| node.lifecycle == PopupLifecycle::Alive && node.mapped);
        if let Some(rejection) = direct_scanout_scene_rejection_for_flags(
            self.visible_layer_surface_above_content_count() > 0,
            popup_visible,
        ) {
            return Err(rejection);
        }

        let geometry = self
            .current_visual_root_window_geometry(owner.owner_root_surface_id)
            .ok_or(DirectScanoutSceneRejection::OwnerDoesNotCoverOutput)?;
        if geometry.width != self.output_size.width
            || geometry.height != self.output_size.height
            || geometry.placement != SurfacePlacement::absolute_root_at(0, 0)
        {
            return Err(DirectScanoutSceneRejection::OwnerDoesNotCoverOutput);
        }

        let owner_index = self
            .renderable_surfaces
            .iter()
            .position(|surface| surface.surface_id == owner.owner_root_surface_id)
            .ok_or(DirectScanoutSceneRejection::OwnerRootBufferMissing)?;
        let root = &self.renderable_surfaces[owner_index];
        if self.renderable_surfaces.iter().any(|surface| {
            surface.surface_id != owner.owner_root_surface_id
                && self.root_surface_id_for_surface(surface.surface_id)
                    == owner.owner_root_surface_id
        }) {
            return Err(DirectScanoutSceneRejection::OwnerTreeHasAdditionalSurface);
        }
        if root.buffer_source() != SurfaceBufferSource::Dmabuf {
            return Err(DirectScanoutSceneRejection::NonDmabuf);
        }
        let buffer = root
            .dmabuf_handle()
            .cloned()
            .ok_or(DirectScanoutSceneRejection::OwnerRootBufferMissing)?;
        if buffer.format() != DrmFormat::Xrgb8888 {
            return Err(DirectScanoutSceneRejection::FormatNotOpaqueXrgb8888);
        }
        let output_size = BufferSize::new(self.output_size.width, self.output_size.height)
            .ok_or(DirectScanoutSceneRejection::OwnerDoesNotCoverOutput)?;
        if buffer.size() != output_size {
            return Err(DirectScanoutSceneRejection::BufferSizeMismatch);
        }
        if root.buffer_scale != 1 {
            return Err(DirectScanoutSceneRejection::BufferScaleUnsupported);
        }
        if root.buffer_transform != wl_output::Transform::Normal {
            return Err(DirectScanoutSceneRejection::BufferTransformUnsupported);
        }
        if root.viewport_source.is_some() || root.viewport_destination.is_some() {
            return Err(DirectScanoutSceneRejection::ViewportUnsupported);
        }
        if root.visual_clip.is_some() {
            return Err(DirectScanoutSceneRejection::VisualClipPresent);
        }
        if self
            .active_toplevel_resizes
            .contains_key(&owner.owner_root_surface_id)
            || root.render_placement.is_some()
        {
            return Err(DirectScanoutSceneRejection::ResizePreviewActive);
        }
        if root.x != 0
            || root.y != 0
            || root.width != output_size.width
            || root.height != output_size.height
            || root.placement != SurfacePlacement::absolute_root_at(0, 0)
        {
            return Err(DirectScanoutSceneRejection::PlacementMismatch);
        }
        if self.has_pending_frame_prepare_work() {
            return Err(DirectScanoutSceneRejection::PendingOrUnpublishedWork);
        }

        Ok(DirectScanoutSceneCandidate {
            surface_id: root.surface_id,
            root_surface_id: owner.owner_root_surface_id,
            generation: root.generation,
            commit_sequence: root.commit_sequence,
            buffer_identity: root.buffer_identity().clone(),
            buffer,
            buffer_size: output_size,
            output_size,
        })
    }

    pub(in crate::compositor) fn fullscreen_render_plan_metrics(
        &self,
    ) -> FullscreenRenderPlanMetrics {
        let eligibility = self.fullscreen_presentation_eligibility();
        let owner_root_surface_id = eligibility.owner.map(|owner| owner.owner_root_surface_id);
        let visible_overlay_count = self.visible_fullscreen_overlay_count();
        let solitary_tree_active = self.direct_scanout_scene_candidate().is_ok();
        let culled_surface_count = owner_root_surface_id
            .map(|owner| {
                self.renderable_surfaces
                    .iter()
                    .filter(|surface| self.root_surface_id_for_surface(surface.surface_id) != owner)
                    .count()
                    .saturating_sub(visible_overlay_count)
            })
            .unwrap_or_default();
        FullscreenRenderPlanMetrics {
            fullscreen_active: owner_root_surface_id.is_some(),
            owner_root_surface_id,
            solitary_tree_active,
            culled_surface_count,
            wallpaper_culled: solitary_tree_active,
            visible_overlay_count,
            rejection: eligibility.rejection,
        }
    }

    pub(in crate::compositor) fn native_frame_renderable_surfaces(
        &self,
    ) -> Cow<'_, [RenderableSurface]> {
        let metrics = self.fullscreen_render_plan_metrics();
        if !metrics.solitary_tree_active {
            return Cow::Borrowed(&self.renderable_surfaces);
        }
        let Some(owner_root_surface_id) = metrics.owner_root_surface_id else {
            return Cow::Borrowed(&self.renderable_surfaces);
        };
        let overlay_tree_root_ids = self.fullscreen_overlay_tree_root_ids();
        Cow::Owned(
            self.renderable_surfaces
                .iter()
                .filter(|surface| {
                    let root_surface_id = self.root_surface_id_for_surface(surface.surface_id);
                    root_surface_id == owner_root_surface_id
                        || overlay_tree_root_ids.contains(&root_surface_id)
                })
                .cloned()
                .collect(),
        )
    }

    fn visible_fullscreen_overlay_count(&self) -> usize {
        self.layer_surfaces
            .values()
            .filter(|role| role.mapped && role.committed.layer == Layer::Overlay)
            .count()
    }

    fn visible_layer_surface_above_content_count(&self) -> usize {
        self.layer_surfaces
            .values()
            .filter(|role| role.mapped && role.committed.layer.scene_rank() > 2)
            .count()
    }

    fn fullscreen_overlay_tree_root_ids(&self) -> Vec<u32> {
        self.layer_surfaces
            .iter()
            .filter_map(|(surface_id, role)| {
                (role.mapped && role.committed.layer == Layer::Overlay).then_some(*surface_id)
            })
            .collect()
    }
}
