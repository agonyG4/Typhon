use super::*;
use crate::compositor::fullscreen::{
    direct_scanout_scene_blockers_for_visibility, direct_scanout_viewport_compatibility,
};
use crate::compositor::{SurfaceContentType, SurfacePresentationMetadata};
use crate::render_backend::buffer::SurfaceBufferSource;
use std::borrow::Cow;

impl CompositorState {
    pub(in crate::compositor) fn fullscreen_tree_presentation_metadata(
        &self,
    ) -> Option<SurfacePresentationMetadata> {
        let owner = self.fullscreen_presentation?;
        let mut metadata = SurfacePresentationMetadata::default();
        for surface in &self.renderable_surfaces {
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
            if metadata.content_type == SurfaceContentType::None
                && surface_metadata.content_type != SurfaceContentType::None
            {
                metadata.content_type = surface_metadata.content_type;
            }
        }
        Some(metadata)
    }

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
        let Some(_toplevel) = self.toplevel_surfaces.get(&owner.owner_root_surface_id) else {
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
            .renderable_surfaces
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
        let _toplevel = self
            .toplevel_surfaces
            .get(&owner.owner_root_surface_id)
            .ok_or(DirectScanoutSceneRejection::OwnerMissing)?;
        if self
            .toplevel_window_state(owner.owner_root_surface_id)
            .is_some_and(WindowState::is_minimized)
        {
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
        let viewport_compatibility = direct_scanout_viewport_compatibility(
            buffer.size(),
            output_size,
            root.buffer_scale,
            root.buffer_transform,
            root.viewport_source,
            root.viewport_destination,
        )?;
        if root.visual_clip.is_some() {
            return Err(DirectScanoutSceneRejection::VisualClipPresent);
        }
        if self
            .active_toplevel_resizes
            .contains_key(&owner.owner_root_surface_id)
            || root.render_placement.is_some()
            || root.render_target_size.is_some()
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
            content_epoch: self
                .surface_content_epoch(root.surface_id)
                .map_or(root.commit_sequence.get(), SurfaceCommitSequence::get),
            generation: root.generation,
            commit_sequence: root.commit_sequence,
            buffer_identity: root.buffer_identity().clone(),
            buffer,
            buffer_size: output_size,
            output_size,
            viewport_identity_metadata_present: viewport_compatibility.metadata_present,
            presentation: self
                .surface_resources
                .get(&root.surface_id)
                .and_then(|surface| surface.data::<SurfaceData>())
                .map_or(SurfacePresentationMetadata::default(), |data| {
                    data.current_presentation()
                }),
        })
    }

    pub(in crate::compositor) fn direct_scanout_scene_blockers(
        &self,
    ) -> DirectScanoutSceneBlockers {
        let mut blockers = DirectScanoutSceneBlockers::default();
        let Some(owner) = self.fullscreen_presentation else {
            blockers.push(DirectScanoutSceneRejection::NoFullscreenOwner);
            return blockers;
        };

        if !self
            .toplevel_surfaces
            .contains_key(&owner.owner_root_surface_id)
        {
            blockers.push(DirectScanoutSceneRejection::OwnerMissing);
        }
        if self
            .toplevel_window_state(owner.owner_root_surface_id)
            .is_some_and(WindowState::is_minimized)
        {
            blockers.push(DirectScanoutSceneRejection::OwnerMinimized);
        }

        let popup_visible = self
            .popup_nodes
            .values()
            .any(|node| node.lifecycle == PopupLifecycle::Alive && node.mapped);
        let resize_preview_active = self
            .active_toplevel_resizes
            .contains_key(&owner.owner_root_surface_id);
        for reason in direct_scanout_scene_blockers_for_visibility(
            self.visible_layer_surface_above_content_count() > 0,
            popup_visible,
            resize_preview_active,
        )
        .reasons()
        {
            blockers.push(*reason);
        }

        let geometry = self.current_visual_root_window_geometry(owner.owner_root_surface_id);
        if !geometry.is_some_and(|geometry| {
            geometry.width == self.output_size.width
                && geometry.height == self.output_size.height
                && geometry.placement == SurfacePlacement::absolute_root_at(0, 0)
        }) {
            blockers.push(DirectScanoutSceneRejection::OwnerDoesNotCoverOutput);
        }

        let Some(root) = self
            .renderable_surfaces
            .iter()
            .find(|surface| surface.surface_id == owner.owner_root_surface_id)
        else {
            blockers.push(DirectScanoutSceneRejection::OwnerRootBufferMissing);
            if self.has_pending_frame_prepare_work() {
                blockers.push(DirectScanoutSceneRejection::PendingOrUnpublishedWork);
            }
            return blockers;
        };

        if self.renderable_surfaces.iter().any(|surface| {
            surface.surface_id != owner.owner_root_surface_id
                && self.root_surface_id_for_surface(surface.surface_id)
                    == owner.owner_root_surface_id
        }) {
            blockers.push(DirectScanoutSceneRejection::OwnerTreeHasAdditionalSurface);
        }
        if root.buffer_source() != SurfaceBufferSource::Dmabuf {
            blockers.push(DirectScanoutSceneRejection::NonDmabuf);
        } else if let Some(buffer) = root.dmabuf_handle() {
            if buffer.format() != DrmFormat::Xrgb8888 {
                blockers.push(DirectScanoutSceneRejection::FormatNotOpaqueXrgb8888);
            }
            let Some(output_size) =
                BufferSize::new(self.output_size.width, self.output_size.height)
            else {
                blockers.push(DirectScanoutSceneRejection::BufferSizeMismatch);
                return blockers;
            };
            if buffer.size() != output_size {
                blockers.push(DirectScanoutSceneRejection::BufferSizeMismatch);
            }
            if let Err(rejection) = direct_scanout_viewport_compatibility(
                buffer.size(),
                output_size,
                root.buffer_scale,
                root.buffer_transform,
                root.viewport_source,
                root.viewport_destination,
            ) {
                blockers.push(rejection);
            }
        } else {
            blockers.push(DirectScanoutSceneRejection::OwnerRootBufferMissing);
        }
        if root.visual_clip.is_some() {
            blockers.push(DirectScanoutSceneRejection::VisualClipPresent);
        }
        if resize_preview_active
            || root.render_placement.is_some()
            || root.render_target_size.is_some()
        {
            blockers.push(DirectScanoutSceneRejection::ResizePreviewActive);
        }
        if root.x != 0
            || root.y != 0
            || root.width != self.output_size.width
            || root.height != self.output_size.height
            || root.placement != SurfacePlacement::absolute_root_at(0, 0)
        {
            blockers.push(DirectScanoutSceneRejection::PlacementMismatch);
        }
        if self.has_pending_frame_prepare_work() {
            blockers.push(DirectScanoutSceneRejection::PendingOrUnpublishedWork);
        }

        blockers
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_collect_simultaneous_visibility_blockers_in_candidate_order() {
        let blockers = direct_scanout_scene_blockers_for_visibility(true, true, true);

        assert_eq!(
            blockers.reasons(),
            &[
                DirectScanoutSceneRejection::OverlayVisible,
                DirectScanoutSceneRejection::PopupVisible,
                DirectScanoutSceneRejection::ResizePreviewActive,
            ]
        );
    }
}
