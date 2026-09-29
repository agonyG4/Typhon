use super::fullscreen::CanonicalPresentationScene;
use super::*;
use crate::compositor::direct_scanout::{
    DirectScanoutEffectAnalysis, DirectScanoutEffectDisposition,
    DirectScanoutEffectInstanceAnalysis, DirectScanoutEffectSource,
    MAX_DIRECT_SCANOUT_EFFECT_DETAILS,
};
use crate::compositor::direct_scanout::{
    DirectScanoutProbeCandidate, DirectScanoutSceneBlockers, DirectScanoutSceneCandidate,
    DirectScanoutSceneRejection, direct_scanout_probe_blockers_allow_scaling,
    direct_scanout_probe_viewport_compatibility, direct_scanout_viewport_compatibility,
};
use crate::compositor::effects::{
    EffectAnchor, EffectAnchorScope, EffectSceneOrder, ResolvedEffectScene,
};
use crate::compositor::presentation_coverage::{
    PresentationCoverageAnalysis, PresentationCoverageContentKind,
};
use crate::compositor::render::SurfaceTargetRect;
use crate::effects::EffectRect;
use crate::presentation_animation::AnimationTime;
use crate::render_backend::buffer::{BufferSize, SurfaceBufferSource};
use crate::wm::WorkspaceLocation;
use wayland_server::Resource;

#[derive(Debug, Clone)]
pub struct DirectScanoutSceneAnalysis {
    pub coverage: PresentationCoverageAnalysis,
    pub effects: DirectScanoutEffectAnalysis,
    pub candidate: Option<DirectScanoutSceneCandidate>,
    pub probe_candidate: Option<DirectScanoutProbeCandidate>,
    pub blockers: DirectScanoutSceneBlockers,
}

impl CompositorState {
    pub(in crate::compositor) fn direct_scanout_effect_identity_scene(
        &self,
        scene: &CanonicalPresentationScene<'_>,
        presentation: &crate::presentation_animation::PresentationSceneSample,
        lifecycle: &crate::window_lifecycle_animation::LifecycleSceneSample,
    ) -> ResolvedEffectScene {
        let mut effects = self.resolved_effect_scene_with_presentation_and_lifecycle(
            presentation,
            &scene.fullscreen_plan,
            lifecycle,
        );
        if matches!(&scene.surfaces, std::borrow::Cow::Borrowed(_)) {
            return effects;
        }

        let visual_group_orders = scene.visual_group_orders();
        for instance in &mut effects.instances {
            let Some(surface_id) = (match instance.anchor {
                EffectAnchor::BeforeSurface(surface_id)
                | EffectAnchor::ReplaceSurface(surface_id)
                | EffectAnchor::AfterSurface(surface_id) => Some(surface_id),
                EffectAnchor::OutputPostProcess => None,
            }) else {
                continue;
            };
            let Some(index) = scene.surface_index(surface_id) else {
                continue;
            };
            let Some(group_order) = visual_group_orders.get(index).copied().flatten() else {
                continue;
            };
            let phase = match instance.anchor {
                EffectAnchor::BeforeSurface(_) => 0,
                EffectAnchor::ReplaceSurface(_) => 1,
                EffectAnchor::AfterSurface(_) => 2,
                EffectAnchor::OutputPostProcess => 3,
            };
            instance.visual_group = Some(
                VisualGroupId::new(group_order)
                    .expect("canonical visual group order is a valid identity"),
            );
            instance.scene_order = EffectSceneOrder {
                group_order,
                surface_order: match instance.anchor_scope {
                    EffectAnchorScope::Surface => {
                        u32::try_from(index).unwrap_or(u32::MAX.saturating_sub(1))
                    }
                    EffectAnchorScope::VisualGroup => 0,
                },
                phase,
            };
        }
        ResolvedEffectScene::new(effects.generation, effects.instances)
    }

    #[cfg(test)]
    pub(in crate::compositor) fn direct_scanout_effect_analysis(
        &self,
        fullscreen_plan: &FullscreenCompositionPlan,
        output_size: BufferSize,
        source: Option<DirectScanoutEffectSource>,
    ) -> DirectScanoutEffectAnalysis {
        let scene = self.canonical_presentation_scene();
        self.direct_scanout_effect_analysis_for_scene(&scene, fullscreen_plan, output_size, source)
    }

    fn direct_scanout_effect_analysis_for_scene(
        &self,
        scene_view: &CanonicalPresentationScene<'_>,
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
        let lifecycle = self.lifecycle_scene_sample_at(
            AnimationTime::monotonic_now().unwrap_or(AnimationTime::from_nanos(0)),
        );
        let visual_group_orders = source
            .is_some()
            .then(|| {
                matches!(&scene_view.surfaces, std::borrow::Cow::Owned(_))
                    .then(|| scene_view.visual_group_orders())
            })
            .flatten();
        let mut analysis = DirectScanoutEffectAnalysis {
            raw_instance_count: scene.summary.visible_instance_count,
            instances_truncated: scene.instances.len() > MAX_DIRECT_SCANOUT_EFFECT_DETAILS,
            ..DirectScanoutEffectAnalysis::default()
        };

        for instance in &scene.instances {
            let disposition =
                if !self.effect_instance_allows_presentation(instance, fullscreen_plan, &lifecycle)
                {
                    analysis.culled_instance_count =
                        analysis.culled_instance_count.saturating_add(1);
                    DirectScanoutEffectDisposition::PresentationCulled
                } else {
                    analysis.presentation_instance_count =
                        analysis.presentation_instance_count.saturating_add(1);
                    if instance.region.intersect_rect(output_bounds).is_empty() {
                        analysis.outside_output_instance_count =
                            analysis.outside_output_instance_count.saturating_add(1);
                        DirectScanoutEffectDisposition::OutsideOutput
                    } else {
                        let disposition = self.classify_direct_scanout_effect(
                            instance,
                            source,
                            scene_view,
                            visual_group_orders.as_deref(),
                        );
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
        scene_view: &CanonicalPresentationScene<'_>,
        visual_group_orders: Option<&[Option<u32>]>,
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
        let anchor_index = scene_view.surface_index(anchor_surface_id);
        let anchor_surface_order = anchor_index.and_then(|index| u32::try_from(index).ok());
        let anchor_group_order = if let Some(orders) = visual_group_orders {
            anchor_index.and_then(|index| orders.get(index).copied().flatten())
        } else {
            self.visual_group_for_surface(anchor_surface_id)
                .map(|group| group.get())
        };
        crate::compositor::direct_scanout::classify_direct_scanout_effect_with_scene_orders(
            instance,
            source,
            anchor_group_order,
            anchor_surface_order,
        )
    }

    pub(in crate::compositor) fn direct_scanout_scene_analysis(
        &self,
    ) -> DirectScanoutSceneAnalysis {
        self.direct_scanout_scene_analysis_impl(false)
    }

    pub(in crate::compositor) fn direct_scanout_probe_scene_analysis(
        &self,
    ) -> DirectScanoutSceneAnalysis {
        self.direct_scanout_scene_analysis_impl(true)
    }

    fn direct_scanout_scene_analysis_impl(
        &self,
        include_scaled_probe_candidate: bool,
    ) -> DirectScanoutSceneAnalysis {
        let output_size = BufferSize::new(self.output_size.width, self.output_size.height)
            .expect("configured output size is nonzero");
        let scene = self.canonical_presentation_scene();
        let active_surfaces = scene.surfaces.as_ref();
        let coverage = self.presentation_coverage_analysis_for_scene(&scene);
        let fullscreen_plan = &scene.fullscreen_plan;
        let mut effects = self.direct_scanout_effect_analysis_for_scene(
            &scene,
            fullscreen_plan,
            output_size,
            None,
        );

        let mut blockers = DirectScanoutSceneBlockers::default();
        if self.lifecycle_animation_has_pending_visible() {
            blockers.push(DirectScanoutSceneRejection::LifecycleAnimation);
        }
        if self.window_exit_payloads.active_payloads().next().is_some() {
            blockers.push(DirectScanoutSceneRejection::WindowExitAnimation);
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
                probe_candidate: None,
                blockers,
            };
        };
        let root_surface_id = covering_group.root_surface_id;
        let candidate_scene_node_id = scene
            .owner_root_for_surface(root_surface_id)
            .filter(|owner_root| *owner_root == root_surface_id)
            .and_then(|_| self.presentation_scene_node_id_for_root(root_surface_id));

        if let Some(scene_node_id) = candidate_scene_node_id {
            if self.presentation_animator.has_geometry_track(scene_node_id) {
                blockers.push(DirectScanoutSceneRejection::AnimationTransform);
            }
            if self.presentation_animator.has_opacity_track(scene_node_id) {
                blockers.push(DirectScanoutSceneRejection::PresentationOpacity);
            }
            if self.presentation_animator.has_clip_track(scene_node_id) {
                blockers.push(DirectScanoutSceneRejection::PresentationClip);
            }
        }

        if self
            .window_id_for_surface(root_surface_id)
            .and_then(|window_id| self.window(window_id))
            .is_some_and(|window| !window.canonical_opacity().is_opaque())
        {
            blockers.push(DirectScanoutSceneRejection::PresentationOpacity);
        }
        if self
            .window_id_for_surface(root_surface_id)
            .and_then(|window_id| self.window(window_id))
            .is_some_and(|window| !window.canonical_clip().is_unbounded())
        {
            blockers.push(DirectScanoutSceneRejection::PresentationClip);
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
        if candidate_scene_node_id.is_some_and(|scene_node_id| {
            !self
                .presented_presentation_clip_for_scene_node(scene_node_id)
                .is_unbounded()
        }) {
            blockers.push(DirectScanoutSceneRejection::PresentationClip);
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
                probe_candidate: None,
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
                probe_candidate: None,
                blockers,
            };
        };
        if scene.owner_root_for_surface(source.surface_id) != Some(root_surface_id) {
            blockers.push(DirectScanoutSceneRejection::OwnerMissing);
        }

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
            let viewport_compatibility =
                if buffer.size() == output_size || !include_scaled_probe_candidate {
                    direct_scanout_viewport_compatibility(
                        buffer.size(),
                        output_size,
                        source.buffer_scale,
                        source.buffer_transform,
                        source.viewport_source,
                        source.viewport_destination,
                    )
                } else {
                    direct_scanout_probe_viewport_compatibility(
                        buffer.size(),
                        output_size,
                        source.buffer_scale,
                        source.buffer_transform,
                        source.viewport_source,
                        source.viewport_destination,
                    )
                };
            if let Err(rejection) = viewport_compatibility {
                blockers.push(rejection);
            }
        }
        let visual_group_orders = matches!(&scene.surfaces, std::borrow::Cow::Owned(_))
            .then(|| scene.visual_group_orders());
        let source_index = scene.surface_index(source_surface_id);
        let source_group_order = if let Some(orders) = visual_group_orders.as_deref() {
            source_index.and_then(|index| orders.get(index).copied().flatten())
        } else {
            self.visual_group_for_surface(source_surface_id)
                .map(|group| group.get())
        };
        effects = self.direct_scanout_effect_analysis_for_scene(
            &scene,
            fullscreen_plan,
            output_size,
            Some(DirectScanoutEffectSource {
                group_order: source_group_order,
                surface_order: scene.surface_order(source_surface_id),
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
        let window_scene_node_id = candidate_scene_node_id;
        let surface_scene_node_id = scene.scene_node_for_surface(source.surface_id);
        if window_scene_node_id.is_none() || surface_scene_node_id.is_none() {
            blockers.push(DirectScanoutSceneRejection::PendingOrUnpublishedWork);
        }

        let direct_candidate = if blockers.is_empty() {
            let identity_sample_time =
                AnimationTime::monotonic_now().unwrap_or(AnimationTime::from_nanos(0));
            let targets = self.native_frame_presentation_targets_for_scene(&scene);
            let presentation = self.presentation_scene_sample_for_targets_at_with_source(
                identity_sample_time,
                crate::presentation_animation::PresentationSampleTimeSource::MonotonicFallback,
                &targets,
            );
            let lifecycle = self.lifecycle_scene_sample_at(identity_sample_time);
            let effect_identity_signature = self
                .direct_scanout_effect_identity_scene(&scene, &presentation, &lifecycle)
                .signature;
            let presented_window_rect = self
                .current_visual_root_window_geometry(root_surface_id)
                .and_then(|geometry| {
                    self.presentation_rect_for_geometry_for_surfaces(
                        active_surfaces,
                        root_surface_id,
                        geometry,
                    )
                });
            match (
                buffer.as_ref(),
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
                    surface_scene_node_id: surface_scene_node_id
                        .expect("eligible direct candidate has a surface scene node"),
                    window_scene_node_id: window_scene_node_id
                        .expect("eligible direct candidate has a WindowGroup scene node"),
                    presented_window_rect,
                    render_generation: self.scene_render_generation,
                    effect_identity_signature,
                    content_epoch: self
                        .surface_content_epoch(source.surface_id)
                        .map_or(source.commit_sequence.get(), |sequence| sequence.get()),
                    generation: source.generation,
                    surface_presentation_generation,
                    commit_sequence: source.commit_sequence,
                    buffer_identity: source.buffer_identity().clone(),
                    buffer: buffer.clone(),
                    buffer_size: buffer.size(),
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

        if include_scaled_probe_candidate
            && direct_candidate.is_none()
            && direct_scanout_probe_blockers_allow_scaling(&blockers)
            && self
                .current_visual_root_window_geometry(root_surface_id)
                .and_then(|geometry| self.presentation_rect_for_geometry(root_surface_id, geometry))
                .is_none()
        {
            blockers.push(DirectScanoutSceneRejection::PendingOrUnpublishedWork);
        }

        let probe_candidate = if include_scaled_probe_candidate
            && direct_candidate.is_none()
            && direct_scanout_probe_blockers_allow_scaling(&blockers)
            && buffer.is_some()
        {
            buffer.map(|buffer| DirectScanoutProbeCandidate {
                buffer_identity: source.buffer_identity().clone(),
                buffer_size: buffer.size(),
                output_size,
                buffer,
            })
        } else {
            None
        };

        debug_assert_eq!(direct_candidate.is_some(), blockers.is_empty());
        DirectScanoutSceneAnalysis {
            coverage,
            effects,
            candidate: direct_candidate,
            probe_candidate,
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
