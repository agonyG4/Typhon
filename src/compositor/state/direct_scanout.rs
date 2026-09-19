use super::*;
use crate::compositor::direct_scanout::{
    DirectScanoutEffectAnalysis, DirectScanoutEffectDisposition,
    DirectScanoutEffectInstanceAnalysis, DirectScanoutEffectSource,
    MAX_DIRECT_SCANOUT_EFFECT_DETAILS,
};
use crate::compositor::direct_scanout::{
    DirectScanoutSceneBlockers, DirectScanoutSceneCandidate, DirectScanoutSceneRejection,
    direct_scanout_viewport_compatibility,
};
use crate::compositor::effects::EffectAnchor;
use crate::compositor::presentation_coverage::{
    PresentationCoverageAnalysis, PresentationCoverageContentKind,
};
use crate::compositor::render::SurfaceTargetRect;
use crate::effects::EffectRect;
use crate::render_backend::buffer::{BufferSize, SurfaceBufferSource};
use crate::wm::WorkspaceLocation;
use wayland_server::Resource;

#[derive(Debug, Clone)]
pub struct DirectScanoutSceneAnalysis {
    pub coverage: PresentationCoverageAnalysis,
    pub effects: DirectScanoutEffectAnalysis,
    pub candidate: Option<DirectScanoutSceneCandidate>,
    pub blockers: DirectScanoutSceneBlockers,
}

impl CompositorState {
    pub(in crate::compositor) fn direct_scanout_effect_analysis(
        &self,
        fullscreen_plan: &FullscreenCompositionPlan,
        output_size: BufferSize,
        source: Option<DirectScanoutEffectSource>,
    ) -> DirectScanoutEffectAnalysis {
        if self.effect_scene_summary().visible_instance_count == 0 {
            return DirectScanoutEffectAnalysis::default();
        }

        let scene = self.resolved_effect_scene();
        let output_bounds = EffectRect::new(0, 0, output_size.width, output_size.height)
            .expect("configured output size is nonzero");
        let mut analysis = DirectScanoutEffectAnalysis {
            raw_instance_count: scene.summary.visible_instance_count,
            instances_truncated: scene.instances.len() > MAX_DIRECT_SCANOUT_EFFECT_DETAILS,
            ..DirectScanoutEffectAnalysis::default()
        };

        for instance in &scene.instances {
            let disposition = if !self
                .effect_instance_allows_presentation(instance, fullscreen_plan)
            {
                analysis.culled_instance_count = analysis.culled_instance_count.saturating_add(1);
                DirectScanoutEffectDisposition::PresentationCulled
            } else {
                analysis.presentation_instance_count =
                    analysis.presentation_instance_count.saturating_add(1);
                if instance.region.intersect_rect(output_bounds).is_empty() {
                    analysis.outside_output_instance_count =
                        analysis.outside_output_instance_count.saturating_add(1);
                    DirectScanoutEffectDisposition::OutsideOutput
                } else {
                    let disposition = self.classify_direct_scanout_effect(instance, source);
                    match disposition {
                        DirectScanoutEffectDisposition::OccludedByOpaqueScanoutSource => {
                            analysis.occluded_instance_count =
                                analysis.occluded_instance_count.saturating_add(1);
                        }
                        DirectScanoutEffectDisposition::ContributingAboveSource
                        | DirectScanoutEffectDisposition::ContributingAtSource
                        | DirectScanoutEffectDisposition::OutputPostProcess
                        | DirectScanoutEffectDisposition::UnknownOrder => {
                            analysis.contributing_instance_count =
                                analysis.contributing_instance_count.saturating_add(1);
                        }
                        DirectScanoutEffectDisposition::PresentationCulled
                        | DirectScanoutEffectDisposition::OutsideOutput => {}
                    }
                    disposition
                }
            };

            if analysis.instances.len() < MAX_DIRECT_SCANOUT_EFFECT_DETAILS {
                analysis
                    .instances
                    .push(DirectScanoutEffectInstanceAnalysis {
                        id: instance.id,
                        program: instance.program,
                        anchor: instance.anchor,
                        region: instance.region.clone(),
                        disposition,
                    });
            }
        }
        analysis.requires_composition = analysis.contributing_instance_count > 0;
        analysis
    }

    fn classify_direct_scanout_effect(
        &self,
        instance: &crate::compositor::ResolvedEffectInstance,
        source: Option<DirectScanoutEffectSource>,
    ) -> DirectScanoutEffectDisposition {
        if instance.anchor == EffectAnchor::OutputPostProcess {
            return DirectScanoutEffectDisposition::OutputPostProcess;
        }
        let Some(source) = source else {
            return DirectScanoutEffectDisposition::UnknownOrder;
        };
        let Some(anchor_surface_id) = (match instance.anchor {
            EffectAnchor::BeforeSurface(surface_id)
            | EffectAnchor::ReplaceSurface(surface_id)
            | EffectAnchor::AfterSurface(surface_id) => Some(surface_id),
            EffectAnchor::OutputPostProcess => None,
        }) else {
            return DirectScanoutEffectDisposition::UnknownOrder;
        };
        let anchor_surface_order = self.active_scene_surface_order(anchor_surface_id);
        crate::compositor::direct_scanout::classify_direct_scanout_effect(
            instance,
            source,
            anchor_surface_order,
        )
    }

    fn active_scene_surface_order(&self, surface_id: u32) -> Option<u32> {
        self.active_scene_surfaces()
            .iter()
            .position(|surface| surface.surface_id == surface_id)
            .and_then(|index| u32::try_from(index).ok())
    }

    pub(in crate::compositor) fn direct_scanout_scene_analysis(
        &self,
    ) -> DirectScanoutSceneAnalysis {
        let output_size = BufferSize::new(self.output_size.width, self.output_size.height)
            .expect("configured output size is nonzero");
        let active_surfaces = self.active_scene_surfaces();
        let coverage = self.presentation_coverage_analysis();
        let fullscreen_plan = self.fullscreen_composition_plan();
        let mut effects = self.direct_scanout_effect_analysis(&fullscreen_plan, output_size, None);

        let mut blockers = DirectScanoutSceneBlockers::default();
        if self.lifecycle_animation_has_pending_visible() {
            blockers.push(DirectScanoutSceneRejection::LifecycleAnimation);
        }

        let Some(covering_group) = coverage.covering_application_group.as_ref() else {
            blockers.push(DirectScanoutSceneRejection::NoOutputCoveringApplication);
            if effects.requires_composition {
                blockers.push(DirectScanoutSceneRejection::EffectRequiresComposition);
            }
            return DirectScanoutSceneAnalysis {
                coverage,
                effects,
                candidate: None,
                blockers,
            };
        };
        let root_surface_id = covering_group.root_surface_id;
        let candidate_scene_node_id = self.presentation_scene_node_id_for_root(root_surface_id);

        if let Some(scene_node_id) = candidate_scene_node_id {
            if self.presentation_animator.has_geometry_track(scene_node_id) {
                blockers.push(DirectScanoutSceneRejection::AnimationTransform);
            }
            if self.presentation_animator.has_opacity_track(scene_node_id) {
                blockers.push(DirectScanoutSceneRejection::PresentationOpacity);
            }
        }

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
        if candidate_scene_node_id.is_some_and(|scene_node_id| {
            self.presented_presentation_geometry_is_non_identity_for_scene_node(scene_node_id)
        }) {
            blockers.push(DirectScanoutSceneRejection::AnimationTransform);
        }
        if candidate_scene_node_id.is_some_and(|scene_node_id| {
            self.presented_presentation_opacity_is_non_identity_for_scene_node(scene_node_id)
        }) {
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
            if effects.requires_composition {
                blockers.push(DirectScanoutSceneRejection::EffectRequiresComposition);
            }
            return DirectScanoutSceneAnalysis {
                coverage,
                effects,
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
            if effects.requires_composition {
                blockers.push(DirectScanoutSceneRejection::EffectRequiresComposition);
            }
            return DirectScanoutSceneAnalysis {
                coverage,
                effects,
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
        effects = self.direct_scanout_effect_analysis(
            &fullscreen_plan,
            output_size,
            Some(DirectScanoutEffectSource {
                group_order: self
                    .visual_group_for_surface(source_surface_id)
                    .map(|group| group.get()),
                surface_order: self.active_scene_surface_order(source_surface_id),
                can_occlude: covering_surface.target
                    == SurfaceTargetRect::new(0, 0, output_size.width, output_size.height)
                    && coverage.geometrically_covers_output()
                    && coverage.can_occlude_behind_content()
                    && buffer
                        .as_ref()
                        .is_some_and(|buffer| buffer.format().is_opaque_rgb8888()),
            }),
        );
        if effects.requires_composition {
            blockers.push(DirectScanoutSceneRejection::EffectRequiresComposition);
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
            effects,
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
