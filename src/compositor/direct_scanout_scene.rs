use super::fullscreen::{
    DirectScanoutSceneBlockers, DirectScanoutSceneCandidate, DirectScanoutSceneRejection,
    direct_scanout_viewport_compatibility,
};
use super::presentation_coverage::{
    PresentationCoverageAnalysis, PresentationCoverageContentKind, PresentationCoverageOpacity,
    analyze_presentation_coverage,
};
use super::{
    CompositorState, SceneWorkOwner, SurfaceData, SurfacePlacement, SurfacePresentationMetadata,
    WindowState,
};
use crate::render_backend::buffer::{BufferSize, DrmFormat, SurfaceBufferSource};
use crate::wm::WorkspaceLocation;
use wayland_server::Resource;

#[derive(Debug, Clone)]
pub struct DirectScanoutSceneAnalysis {
    pub coverage: PresentationCoverageAnalysis,
    pub candidate: Option<DirectScanoutSceneCandidate>,
    pub blockers: DirectScanoutSceneBlockers,
}

impl CompositorState {
    pub(in crate::compositor) fn direct_scanout_scene_analysis(
        &self,
    ) -> DirectScanoutSceneAnalysis {
        let output_size = BufferSize::new(self.output_size.width, self.output_size.height)
            .expect("configured output size is nonzero");
        let active_surfaces = self.active_scene_surfaces();
        let decorations = self.native_decoration_render_instances(active_surfaces);
        let coverage = analyze_presentation_coverage(
            active_surfaces,
            &decorations,
            self.active_scene_popup_surface_ids(),
            output_size,
            |root_surface_id| self.window_id_for_surface(root_surface_id).is_some(),
            |root_surface_id| self.layer_surfaces.contains_key(&root_surface_id),
            |root_surface_id| {
                self.current_visual_root_window_geometry(root_surface_id)
                    .is_some_and(|geometry| {
                        geometry.width == self.output_size.width
                            && geometry.height == self.output_size.height
                            && geometry.placement == SurfacePlacement::absolute_root_at(0, 0)
                    })
            },
            |root_surface_id| self.presentation_coverage_opacity(root_surface_id, output_size),
        );

        let mut blockers = DirectScanoutSceneBlockers::default();
        if let Some(rejection) = super::fullscreen::direct_scanout_scene_rejection_for_effects(
            self.effect_scene_summary(),
        ) {
            blockers.push(rejection);
        }
        if self.presentation_animation_has_pending_visible() {
            blockers.push(DirectScanoutSceneRejection::AnimationTransform);
        }
        if self.lifecycle_animation_has_pending_visible() {
            blockers.push(DirectScanoutSceneRejection::LifecycleAnimation);
        }

        let Some(covering_group) = coverage.covering_application_group.as_ref() else {
            blockers.push(DirectScanoutSceneRejection::NoCoveringApplication);
            return DirectScanoutSceneAnalysis {
                coverage,
                candidate: None,
                blockers,
            };
        };
        let root_surface_id = covering_group.root_surface_id;

        if !self.toplevel_surfaces.contains_key(&root_surface_id)
            && self.window_id_for_surface(root_surface_id).is_none()
        {
            blockers.push(DirectScanoutSceneRejection::OwnerMissing);
        }
        if !self.surface_is_visible_in_active_scene(root_surface_id)
            || self
                .toplevel_window_state(root_surface_id)
                .is_some_and(WindowState::is_minimized)
        {
            blockers.push(DirectScanoutSceneRejection::OwnerMinimized);
        }
        if self.presented_presentation_is_non_identity(root_surface_id) {
            blockers.push(DirectScanoutSceneRejection::AnimationTransform);
        }
        if covering_group.surface_ids.len() > 1 {
            blockers.push(DirectScanoutSceneRejection::OwnerTreeHasAdditionalSurface);
        }
        for visible_content in &coverage.visible_content_above {
            let rejection = match visible_content.kind {
                PresentationCoverageContentKind::Application => {
                    if self.application_root_is_special(visible_content.root_surface_id) {
                        DirectScanoutSceneRejection::OverlayVisible
                    } else {
                        DirectScanoutSceneRejection::ApplicationContentAbove
                    }
                }
                PresentationCoverageContentKind::Popup => DirectScanoutSceneRejection::PopupVisible,
                PresentationCoverageContentKind::LayerShell => {
                    DirectScanoutSceneRejection::OverlayVisible
                }
                PresentationCoverageContentKind::ServerSideDecoration => {
                    DirectScanoutSceneRejection::ServerSideDecorationVisible
                }
            };
            blockers.push(rejection);
        }

        let Some(root) = active_surfaces
            .iter()
            .find(|surface| surface.surface_id == root_surface_id)
        else {
            blockers.push(DirectScanoutSceneRejection::OwnerRootBufferMissing);
            if self.has_pending_frame_prepare_work() {
                blockers.push(DirectScanoutSceneRejection::PendingOrUnpublishedWork);
            }
            return DirectScanoutSceneAnalysis {
                coverage,
                candidate: None,
                blockers,
            };
        };

        if active_surfaces.iter().any(|surface| {
            surface.surface_id != root_surface_id
                && self.root_surface_id_for_surface(surface.surface_id) == root_surface_id
        }) {
            blockers.push(DirectScanoutSceneRejection::OwnerTreeHasAdditionalSurface);
        }
        let buffer = root.dmabuf_handle().cloned();
        if root.buffer_source() != SurfaceBufferSource::Dmabuf {
            blockers.push(DirectScanoutSceneRejection::NonDmabuf);
        }
        if buffer.is_none() {
            blockers.push(DirectScanoutSceneRejection::OwnerRootBufferMissing);
        }
        if let Some(buffer) = buffer.as_ref() {
            if buffer.format() != DrmFormat::Xrgb8888 {
                blockers.push(DirectScanoutSceneRejection::FormatNotOpaqueXrgb8888);
            }
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
        }
        if root.visual_clip.is_some() {
            blockers.push(DirectScanoutSceneRejection::VisualClipPresent);
        }
        if self.active_toplevel_resizes.contains_key(&root_surface_id)
            || root
                .render_placement
                .is_some_and(|placement| placement != root.placement)
            || root.render_target_size.is_some()
        {
            blockers.push(DirectScanoutSceneRejection::ResizePreviewActive);
        }
        if root.x != 0
            || root.y != 0
            || root.width != output_size.width
            || root.height != output_size.height
            || root.placement != SurfacePlacement::absolute_root_at(0, 0)
        {
            blockers.push(DirectScanoutSceneRejection::PlacementMismatch);
        }
        if self.has_pending_frame_prepare_work() {
            blockers.push(DirectScanoutSceneRejection::PendingOrUnpublishedWork);
        }
        let surface_presentation_generation = self
            .surface_presentation_generations
            .get(&root.surface_id)
            .copied();
        if surface_presentation_generation.is_none() {
            blockers.push(DirectScanoutSceneRejection::PendingOrUnpublishedWork);
        }

        let candidate = if blockers.is_empty() {
            let presented_window_rect = self
                .current_visual_root_window_geometry(root_surface_id)
                .and_then(|geometry| {
                    self.presentation_rect_for_geometry(root_surface_id, geometry)
                });
            match (
                buffer,
                surface_presentation_generation,
                presented_window_rect,
            ) {
                (
                    Some(buffer),
                    Some(surface_presentation_generation),
                    Some(presented_window_rect),
                ) => Some(DirectScanoutSceneCandidate {
                    surface_id: root.surface_id,
                    root_surface_id,
                    presented_window_rect,
                    content_epoch: self
                        .surface_content_epoch(root.surface_id)
                        .map_or(root.commit_sequence.get(), |sequence| sequence.get()),
                    generation: root.generation,
                    surface_presentation_generation,
                    commit_sequence: root.commit_sequence,
                    buffer_identity: root.buffer_identity().clone(),
                    buffer,
                    buffer_size: output_size,
                    output_size,
                    viewport_identity_metadata_present: root.viewport_source.is_some()
                        || root.viewport_destination.is_some(),
                    presentation: self
                        .surface_resources
                        .get(&root.surface_id)
                        .and_then(|surface| surface.data::<SurfaceData>())
                        .map_or(SurfacePresentationMetadata::default(), |data| {
                            data.current_presentation()
                        }),
                }),
                _ => {
                    blockers.push(DirectScanoutSceneRejection::PendingOrUnpublishedWork);
                    None
                }
            }
        } else {
            None
        };

        debug_assert_eq!(candidate.is_some(), blockers.is_empty());
        DirectScanoutSceneAnalysis {
            coverage,
            candidate,
            blockers,
        }
    }

    pub(in crate::compositor) fn direct_scanout_scene_candidate(
        &self,
    ) -> Result<DirectScanoutSceneCandidate, DirectScanoutSceneRejection> {
        let analysis = self.direct_scanout_scene_analysis();
        analysis.candidate.ok_or_else(|| {
            analysis
                .blockers
                .reasons()
                .first()
                .copied()
                .unwrap_or(DirectScanoutSceneRejection::NoCoveringApplication)
        })
    }

    pub(in crate::compositor) fn direct_scanout_scene_blockers(
        &self,
    ) -> DirectScanoutSceneBlockers {
        self.direct_scanout_scene_analysis().blockers
    }

    fn application_root_is_special(&self, root_surface_id: u32) -> bool {
        self.window_id_for_surface(root_surface_id)
            .is_some_and(|window_id| {
                matches!(
                    self.scene_work_owner_for_window(window_id),
                    SceneWorkOwner::Location(WorkspaceLocation::Special(_))
                )
            })
    }

    fn presentation_coverage_opacity(
        &self,
        root_surface_id: u32,
        output_size: BufferSize,
    ) -> PresentationCoverageOpacity {
        let Some(root) = self
            .active_scene_surfaces()
            .iter()
            .find(|surface| surface.surface_id == root_surface_id)
        else {
            return PresentationCoverageOpacity::Unknown;
        };
        let proven = root.buffer_source() == SurfaceBufferSource::Dmabuf
            && root
                .dmabuf_handle()
                .is_some_and(|buffer| buffer.format() == DrmFormat::Xrgb8888)
            && root
                .dmabuf_handle()
                .is_some_and(|buffer| buffer.size() == output_size)
            && root.visual_clip.is_none()
            && root
                .render_placement
                .is_none_or(|placement| placement == root.placement)
            && root.render_target_size.is_none()
            && root.placement == SurfacePlacement::absolute_root_at(0, 0);
        if proven {
            PresentationCoverageOpacity::OpaqueXrgb8888
        } else {
            PresentationCoverageOpacity::Unknown
        }
    }
}
