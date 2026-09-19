use super::*;
use crate::compositor::direct_scanout::{
    DirectScanoutSceneBlockers, DirectScanoutSceneCandidate, DirectScanoutSceneRejection,
    direct_scanout_viewport_compatibility,
};
use crate::compositor::presentation_coverage::{
    PresentationCoverageAnalysis, PresentationCoverageContentKind,
};
use crate::compositor::render::SurfaceTargetRect;
use crate::render_backend::buffer::{BufferSize, SurfaceBufferSource};
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
        let coverage = self.presentation_coverage_analysis();

        let mut blockers = DirectScanoutSceneBlockers::default();
        if let Some(rejection) =
            crate::compositor::direct_scanout::direct_scanout_scene_rejection_for_effects(
                self.effect_scene_summary(),
            )
        {
            blockers.push(rejection);
        }
        if self.presentation_animation_has_pending_visible_geometry() {
            blockers.push(DirectScanoutSceneRejection::AnimationTransform);
        }
        if self.presentation_animation_has_pending_visible_opacity() {
            blockers.push(DirectScanoutSceneRejection::PresentationOpacity);
        }
        if self.lifecycle_animation_has_pending_visible() {
            blockers.push(DirectScanoutSceneRejection::LifecycleAnimation);
        }

        let Some(covering_group) = coverage.covering_application_group.as_ref() else {
            blockers.push(DirectScanoutSceneRejection::NoOutputCoveringApplication);
            return DirectScanoutSceneAnalysis {
                coverage,
                candidate: None,
                blockers,
            };
        };
        let root_surface_id = covering_group.root_surface_id;

        if self
            .window_id_for_surface(root_surface_id)
            .and_then(|window_id| self.window(window_id))
            .is_some_and(|window| !window.canonical_opacity().is_opaque())
        {
            blockers.push(DirectScanoutSceneRejection::PresentationOpacity);
        }

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
        if self.presented_presentation_opacity_is_non_identity(root_surface_id) {
            blockers.push(DirectScanoutSceneRejection::PresentationOpacity);
        }
        if !covering_group.visible_surface_ids_above_covering.is_empty() {
            blockers.push(DirectScanoutSceneRejection::OwnerTreeContentAboveSource);
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

        let Some(covering_surface) = covering_group.covering_surface.as_ref() else {
            blockers.push(DirectScanoutSceneRejection::OwnerDoesNotCoverOutput);
            if self.has_pending_frame_prepare_work() {
                blockers.push(DirectScanoutSceneRejection::PendingOrUnpublishedWork);
            }
            return DirectScanoutSceneAnalysis {
                coverage,
                candidate: None,
                blockers,
            };
        };
        let source_surface_id = covering_surface.surface_id;
        let Some(source) = active_surfaces
            .iter()
            .find(|surface| surface.surface_id == source_surface_id)
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

        let buffer = source.dmabuf_handle().cloned();
        if source.buffer_source() != SurfaceBufferSource::Dmabuf {
            blockers.push(DirectScanoutSceneRejection::NonDmabuf);
        }
        if buffer.is_none() {
            blockers.push(DirectScanoutSceneRejection::OwnerRootBufferMissing);
        }
        if let Some(buffer) = buffer.as_ref() {
            if !buffer.format().is_opaque_rgb8888() {
                blockers.push(DirectScanoutSceneRejection::FormatNotProvenOpaque);
            }
            if buffer.size() != output_size {
                blockers.push(DirectScanoutSceneRejection::BufferSizeMismatch);
            }
            if let Err(rejection) = direct_scanout_viewport_compatibility(
                buffer.size(),
                output_size,
                source.buffer_scale,
                source.buffer_transform,
                source.viewport_source,
                source.viewport_destination,
            ) {
                blockers.push(rejection);
            }
        }
        if source.visual_clip.is_some() {
            blockers.push(DirectScanoutSceneRejection::VisualClipPresent);
        }
        if self.active_toplevel_resizes.contains_key(&root_surface_id)
            || source
                .render_placement
                .is_some_and(|placement| placement != source.placement)
            || source.render_target_size.is_some()
        {
            blockers.push(DirectScanoutSceneRejection::ResizePreviewActive);
        }
        if covering_surface.target
            != SurfaceTargetRect::new(0, 0, output_size.width, output_size.height)
        {
            blockers.push(DirectScanoutSceneRejection::PlacementMismatch);
        }
        if self.has_pending_frame_prepare_work() {
            blockers.push(DirectScanoutSceneRejection::PendingOrUnpublishedWork);
        }
        let surface_presentation_generation = self
            .surface_presentation_generations
            .get(&source.surface_id)
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
                    surface_id: source.surface_id,
                    root_surface_id,
                    presented_window_rect,
                    content_epoch: self
                        .surface_content_epoch(source.surface_id)
                        .map_or(source.commit_sequence.get(), |sequence| sequence.get()),
                    generation: source.generation,
                    surface_presentation_generation,
                    commit_sequence: source.commit_sequence,
                    buffer_identity: source.buffer_identity().clone(),
                    buffer,
                    buffer_size: output_size,
                    output_size,
                    viewport_identity_metadata_present: source.viewport_source.is_some()
                        || source.viewport_destination.is_some(),
                    presentation: self
                        .surface_resources
                        .get(&source.surface_id)
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
                .primary()
                .expect("rejected scene must expose a primary blocker")
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
}
