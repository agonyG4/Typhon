use super::*;
use crate::animation_control::{AnimationEffect, AnimationRuntimeCapabilities, AnimationSlot};
#[cfg(test)]
use crate::compositor::decoration::types::DecorationMode;
use crate::presentation_animation::PresentationRetainedVisualKind;
#[cfg(test)]
use crate::presentation_animation::{PresentationGroupTransform, TransitionId};
use crate::window_lifecycle_animation::{
    LifecycleDirection, LifecycleEffectKind, LifecycleFrameSnapshot, LifecycleMotionRequest,
    LifecycleRenderFallbackEntry, LifecycleSceneSample, LifecycleVisualGroup,
    LifecycleVisualSource, LifecycleVisualSourceKind, LifecycleWindowSample,
};
use std::num::NonZeroU64;

impl CompositorState {
    /// Freeze only the compositor-owned effect instances belonging to one
    /// lifecycle visual group before logical minimize removes that group from
    /// the canonical scene. Output-wide effects intentionally do not become a
    /// lifecycle source; they remain owned by the normal output composition.
    pub(in crate::compositor) fn resolved_effect_scene_for_lifecycle_root(
        &self,
        root_surface_id: u32,
    ) -> ResolvedEffectScene {
        let scene = self.resolved_effect_scene();
        let instances = scene
            .instances
            .into_iter()
            .filter(|instance| match instance.anchor {
                EffectAnchor::BeforeSurface(surface_id)
                | EffectAnchor::ReplaceSurface(surface_id)
                | EffectAnchor::AfterSurface(surface_id) => {
                    self.root_surface_id_for_surface(surface_id) == root_surface_id
                }
                EffectAnchor::OutputPostProcess => false,
            })
            .collect();
        ResolvedEffectScene::new(scene.generation, instances)
    }

    pub(in crate::compositor) fn map_effect_scene_to_presentation(
        scene: &ResolvedEffectScene,
        canonical_rect: PresentationRect,
        presented_rect: PresentationRect,
    ) -> ResolvedEffectScene {
        let transform = PresentationGroupTransform::with_scene_node(
            crate::core::SceneNodeId::from_raw(1).expect("effect scene node"),
            0,
            crate::presentation_animation::PresentationTransactionId::new(NonZeroU64::MIN),
            TransitionId::new(NonZeroU64::MIN),
            canonical_rect,
            presented_rect,
            false,
        );
        let instances = scene
            .instances
            .iter()
            .cloned()
            .map(|mut instance| {
                instance.region = super::super::effects::map_effect_region(
                    transform,
                    &instance.region,
                    instance.target_bounds,
                );
                if let Some(target_bounds) =
                    super::super::effects::map_effect_rect(transform, instance.target_bounds)
                {
                    instance.target_bounds = target_bounds;
                }
                instance.signature =
                    instance.signature.wrapping_mul(0x0000_0100_0000_01b3) ^ transform.signature();
                instance
            })
            .collect();
        ResolvedEffectScene::new(scene.generation, instances)
    }
}

#[cfg(test)]
#[path = "lifecycle_animation_tests.rs"]
mod tests;

impl CompositorState {
    pub(in crate::compositor) fn lifecycle_effect(
        &self,
        direction: LifecycleDirection,
    ) -> AnimationEffect {
        let slot = match direction {
            LifecycleDirection::Minimize => AnimationSlot::WindowMinimize,
            LifecycleDirection::Restore => AnimationSlot::WindowRestore,
        };
        self.animation_control
            .effective_effect(slot, self.animation_runtime_capabilities())
    }

    pub(in crate::compositor) fn animation_runtime_capabilities(
        &self,
    ) -> AnimationRuntimeCapabilities {
        AnimationRuntimeCapabilities {
            lamp_renderer: self.lifecycle_animation_renderer_available == Some(true),
            squash_renderer: self.lifecycle_squash_renderer_available == Some(true),
        }
    }

    pub(in crate::compositor) fn lifecycle_effect_kind_for_window(
        &self,
        window_id: WindowId,
        direction: LifecycleDirection,
    ) -> Option<LifecycleEffectKind> {
        let active_effect = self
            .scene_node_id_for_window_group(window_id)
            .and_then(|scene_node_id| {
                self.presentation_animator.active_retained_visual(
                    scene_node_id,
                    PresentationRetainedVisualKind::WindowLifecycle,
                )
            })
            .and_then(|identity| {
                let now = AnimationTime::monotonic_now().unwrap_or(AnimationTime::from_nanos(0));
                self.window_lifecycle_animator
                    .sample(identity, now)
                    .map(|sample| sample.effect)
            });
        active_effect.or_else(|| {
            LifecycleEffectKind::from_animation_effect(self.lifecycle_effect(direction))
        })
    }

    pub(in crate::compositor) fn lifecycle_minimize_source_rect(
        &self,
        root_surface_id: u32,
    ) -> Option<PresentationRect> {
        self.presented_window_geometry(root_surface_id)
            .map(PresentedWindowGeometry::presented_rect)
            .or_else(|| self.current_presentation_rect_for_root(root_surface_id))
    }

    pub(in crate::compositor) fn lifecycle_anchor_rect(
        &self,
        window_id: WindowId,
    ) -> Option<PresentationRect> {
        let anchor = self.astrea_toplevel_publisher.minimize_anchor(window_id)?;
        PresentationRect::new(
            f64::from(anchor.x),
            f64::from(anchor.y),
            f64::from(anchor.width),
            f64::from(anchor.height),
        )
    }

    pub(in crate::compositor) fn lifecycle_window_rect(
        &self,
        root_surface_id: u32,
    ) -> Option<PresentationRect> {
        let geometry = self
            .current_visual_root_window_geometry(root_surface_id)
            .or_else(|| self.current_root_window_geometry(root_surface_id))?;
        self.presentation_rect_for_geometry(root_surface_id, geometry)
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::compositor) fn begin_lifecycle_minimize(
        &mut self,
        window_id: WindowId,
        root_surface_id: u32,
        presented_source_client_rect: Option<PresentationRect>,
        canonical_client_rect: Option<PresentationRect>,
        visual_group: Option<LifecycleVisualGroup>,
        resolved_effect_scene: ResolvedEffectScene,
        lifecycle_decorations: Vec<DecorationRenderInstance>,
    ) {
        let Some(effect) =
            self.lifecycle_effect_kind_for_window(window_id, LifecycleDirection::Minimize)
        else {
            return;
        };
        let Some(scene_node_id) = self.scene_node_id_for_window_group(window_id) else {
            return;
        };
        let Some(now) = AnimationTime::monotonic_now() else {
            return;
        };
        let speed = self.animation_control.configuration().speed;
        let frozen_decoration = lifecycle_decorations
            .into_iter()
            .find(|decoration| decoration.root_surface_id() == root_surface_id);
        let Some((_identity, _payload, _was_reversal)) = self.install_lifecycle_motion(
            scene_node_id,
            window_id,
            root_surface_id,
            presented_source_client_rect,
            canonical_client_rect,
            visual_group,
            LifecycleDirection::Minimize,
            effect,
            resolved_effect_scene,
            frozen_decoration,
            now,
            speed,
        ) else {
            return;
        };
        // Lamp takes over Group properties only after the retained transaction
        // and lifecycle executor have both accepted the same identity.
        self.presentation_animator.cancel_all(scene_node_id);
    }

    #[allow(clippy::too_many_arguments)]
    fn install_lifecycle_motion(
        &mut self,
        scene_node_id: crate::core::SceneNodeId,
        window_id: WindowId,
        root_surface_id: u32,
        presented_source_client_rect: Option<PresentationRect>,
        canonical_client_rect: Option<PresentationRect>,
        visual_group: Option<LifecycleVisualGroup>,
        direction: LifecycleDirection,
        effect: LifecycleEffectKind,
        resolved_effect_scene: ResolvedEffectScene,
        frozen_decoration: Option<DecorationRenderInstance>,
        now: AnimationTime,
        speed: f64,
    ) -> Option<(
        crate::presentation_animation::PresentationRetainedVisualIdentity,
        std::sync::Arc<super::lifecycle_retained::RetainedLifecyclePayload>,
        bool,
    )> {
        let previous_identity = self.presentation_animator.active_retained_visual(
            scene_node_id,
            PresentationRetainedVisualKind::WindowLifecycle,
        );
        let previous_payload = match previous_identity {
            Some(identity) => Some(std::sync::Arc::clone(
                self.retained_lifecycle_payloads.get_exact(identity)?,
            )),
            None => None,
        };
        let visual_group = if let Some(payload) = previous_payload.as_ref() {
            payload.visual_group
        } else if let Some(visual_group) = visual_group {
            visual_group
        } else {
            let canonical_client_rect =
                canonical_client_rect.or_else(|| self.lifecycle_window_rect(root_surface_id))?;
            let presented_source_client_rect = match direction {
                LifecycleDirection::Minimize => presented_source_client_rect
                    .or_else(|| self.lifecycle_minimize_source_rect(root_surface_id))?,
                LifecycleDirection::Restore => canonical_client_rect,
            };
            let anchor_rect = self.lifecycle_anchor_rect(window_id)?;
            LifecycleVisualGroup::from_effect_bounds(
                effect,
                canonical_client_rect,
                canonical_client_rect,
                presented_source_client_rect,
                anchor_rect,
                self.output_size.width,
                self.output_size.height,
            )?
        };
        let visual_intersects_output =
            crate::window_lifecycle_animation::lifecycle_visual_bounds_intersect_output(
                effect,
                visual_group,
                0.0,
                self.output_size.width,
                self.output_size.height,
            );
        if !visual_intersects_output {
            return None;
        }
        let fresh_surface_presentation = if previous_payload.is_none() {
            Some(self.capture_lifecycle_surface_presentation(window_id, root_surface_id)?)
        } else {
            None
        };

        let identity = self
            .presentation_animator
            .begin_retained_visual(
                scene_node_id,
                PresentationRetainedVisualKind::WindowLifecycle,
                now,
            )
            .ok()?;
        let activated_previous = match self
            .presentation_animator
            .activate_retained_visual_exact(identity)
        {
            Ok(previous) => previous,
            Err(_) => {
                self.presentation_animator
                    .retire_retained_visual_exact(identity);
                return None;
            }
        };
        if activated_previous != previous_identity {
            let restored = self
                .presentation_animator
                .restore_retained_visual_owner_exact(identity, activated_previous);
            let retired = self
                .presentation_animator
                .retire_retained_visual_exact(identity);
            debug_assert!(
                restored && retired,
                "unexpected lifecycle owner changed during start"
            );
            return None;
        }

        let payload = if let Some(payload) = previous_payload {
            payload
        } else {
            let Some(payload) = super::lifecycle_retained::RetainedLifecyclePayload::capture(
                identity,
                window_id,
                root_surface_id,
                visual_group,
                resolved_effect_scene,
                frozen_decoration,
                fresh_surface_presentation
                    .expect("fresh lifecycle captured surface presentation before reservation"),
            ) else {
                self.rollback_lifecycle_reservation(identity, previous_identity);
                return None;
            };
            payload
        };
        let mapping_can_commit = if let Some(previous_identity) = previous_identity {
            self.retained_lifecycle_payloads.can_transfer_exact(
                previous_identity,
                identity,
                &payload,
            )
        } else {
            self.retained_lifecycle_payloads.can_publish_exact(identity)
        };
        if !mapping_can_commit {
            self.rollback_lifecycle_reservation(identity, previous_identity);
            return None;
        }
        let started = self.window_lifecycle_animator.start_or_reverse(
            LifecycleMotionRequest {
                presentation_identity: identity,
                direction,
                effect,
                canonical_opacity: self
                    .window(window_id)
                    .map(|window| window.canonical_opacity().get())
                    .unwrap_or(1.0),
            },
            previous_identity,
            now,
            speed,
        );
        if started != Some(identity) {
            self.rollback_lifecycle_reservation(identity, previous_identity);
            return None;
        }

        let published = if let Some(previous_identity) = previous_identity {
            self.retained_lifecycle_payloads
                .transfer_exact(previous_identity, identity, &payload)
        } else {
            self.retained_lifecycle_payloads
                .publish_exact(identity, std::sync::Arc::clone(&payload))
        };
        debug_assert!(
            published,
            "lifecycle payload mapping must commit with motion"
        );
        if let Some(previous_identity) = previous_identity {
            debug_assert!(
                self.presentation_animator
                    .retire_retained_visual_exact(previous_identity)
            );
        }
        Some((identity, payload, previous_identity.is_some()))
    }

    fn rollback_lifecycle_reservation(
        &mut self,
        identity: crate::presentation_animation::PresentationRetainedVisualIdentity,
        previous_identity: Option<
            crate::presentation_animation::PresentationRetainedVisualIdentity,
        >,
    ) {
        let restored = self
            .presentation_animator
            .restore_retained_visual_owner_exact(identity, previous_identity);
        let retired = self
            .presentation_animator
            .retire_retained_visual_exact(identity);
        debug_assert!(
            restored && retired,
            "failed lifecycle start restores its previous owner"
        );
    }

    pub(in crate::compositor) fn begin_lifecycle_restore(
        &mut self,
        window_id: WindowId,
        root_surface_id: u32,
        visual_group: Option<LifecycleVisualGroup>,
        resolved_effect_scene: ResolvedEffectScene,
        lifecycle_decorations: Vec<DecorationRenderInstance>,
    ) {
        let Some(effect) =
            self.lifecycle_effect_kind_for_window(window_id, LifecycleDirection::Restore)
        else {
            return;
        };
        let Some(scene_node_id) = self.scene_node_id_for_window_group(window_id) else {
            return;
        };
        let Some(now) = AnimationTime::monotonic_now() else {
            return;
        };
        let speed = self.animation_control.configuration().speed;
        let frozen_decoration = lifecycle_decorations
            .into_iter()
            .find(|decoration| decoration.root_surface_id() == root_surface_id);
        let Some((_identity, _payload, _was_reversal)) = self.install_lifecycle_motion(
            scene_node_id,
            window_id,
            root_surface_id,
            None,
            None,
            visual_group,
            LifecycleDirection::Restore,
            effect,
            resolved_effect_scene,
            frozen_decoration,
            now,
            speed,
        ) else {
            return;
        };
    }

    pub(in crate::compositor) fn lifecycle_scene_sample_at(
        &self,
        at: AnimationTime,
    ) -> LifecycleSceneSample {
        let active_identities = self
            .presentation_animator
            .active_retained_visuals(PresentationRetainedVisualKind::WindowLifecycle);
        let mut samples = Vec::with_capacity(active_identities.len());
        for identity in active_identities {
            let Some(motion) = self.window_lifecycle_animator.sample(identity, at) else {
                continue;
            };
            let Some(payload) = self.retained_lifecycle_payloads.get_exact(identity) else {
                continue;
            };
            if motion.presentation_identity != identity {
                continue;
            }
            let kind = if payload.effect_scene.is_empty() {
                LifecycleVisualSourceKind::NoOwnedEffects
            } else {
                LifecycleVisualSourceKind::ResolvedOwnedEffects
            };
            let visual_source = LifecycleVisualSource {
                window_id: payload.window_id,
                root_surface_id: payload.root_surface_id,
                presentation_identity: identity,
                payload_id: payload.payload_id,
                kind,
                effect_scene: std::sync::Arc::clone(&payload.effect_scene),
            };
            samples.push(LifecycleWindowSample {
                window_id: payload.window_id,
                root_surface_id: payload.root_surface_id,
                presentation_identity: identity,
                payload_id: payload.payload_id,
                visual_group: payload.visual_group,
                visual_source,
                effect: motion.effect,
                progress: motion.progress,
                effect_opacity: motion.effect_opacity,
                direction: motion.direction,
                mathematically_settled: motion.mathematically_settled,
            });
        }
        LifecycleSceneSample {
            sampled_at: at,
            samples,
        }
    }

    pub(in crate::compositor) fn lifecycle_visual_group_for_scene_node(
        &self,
        scene_node_id: crate::core::SceneNodeId,
    ) -> Option<LifecycleVisualGroup> {
        let identity = self.presentation_animator.active_retained_visual(
            scene_node_id,
            PresentationRetainedVisualKind::WindowLifecycle,
        )?;
        self.retained_lifecycle_payloads
            .get_exact(identity)
            .map(|payload| payload.visual_group)
    }

    pub(in crate::compositor) fn lifecycle_renderable_surfaces(
        &self,
        sample: &LifecycleSceneSample,
    ) -> Vec<RenderableSurface> {
        let mut surfaces = Vec::new();
        let mut seen = HashSet::new();
        for lifecycle_sample in &sample.samples {
            let Some(payload) = self
                .retained_lifecycle_payloads
                .get_exact(lifecycle_sample.presentation_identity)
                .filter(|payload| {
                    payload.payload_id == lifecycle_sample.payload_id
                        && payload.window_id == lifecycle_sample.window_id
                        && payload.root_surface_id == lifecycle_sample.root_surface_id
                })
            else {
                continue;
            };
            let candidates = self.lifecycle_surface_candidates(lifecycle_sample.window_id);
            for surface in payload
                .surface_presentation
                .project(&candidates, &self.surface_presentation_generations)
            {
                let Some(generation) = self
                    .surface_presentation_generations
                    .get(&surface.surface_id)
                    .copied()
                else {
                    continue;
                };
                let key = crate::compositor::SurfacePresentationKey {
                    surface_id: surface.surface_id,
                    generation,
                };
                if seen.insert(key) {
                    surfaces.push(surface);
                }
            }
        }
        surfaces
    }

    fn capture_lifecycle_surface_presentation(
        &self,
        window_id: WindowId,
        root_surface_id: u32,
    ) -> Option<super::lifecycle_surface_snapshot::RetainedSurfacePresentationSnapshot> {
        let surfaces = self.lifecycle_surface_candidates(window_id);
        super::lifecycle_surface_snapshot::RetainedSurfacePresentationSnapshot::capture(
            root_surface_id,
            &surfaces,
            &self.surface_presentation_generations,
        )
    }

    fn lifecycle_surface_candidates(&self, window_id: WindowId) -> Vec<RenderableSurface> {
        let mut surfaces = self.renderable_surfaces.clone();
        if let Some(window) = self.window(window_id)
            && window.state.is_minimized()
        {
            surfaces.extend(window.state.minimized_surfaces().iter().cloned());
        }
        let mut seen = HashSet::new();
        surfaces.retain(|surface| seen.insert(surface.surface_id));
        surfaces
    }

    pub(in crate::compositor) fn lifecycle_decoration_render_instances(
        &self,
        sample: &LifecycleSceneSample,
        surfaces: &[RenderableSurface],
    ) -> Vec<DecorationRenderInstance> {
        let roots = sample
            .samples
            .iter()
            .map(|lamp| lamp.root_surface_id)
            .collect::<HashSet<_>>();
        let mut frozen = Vec::new();
        let mut frozen_roots = HashSet::new();
        for lifecycle_sample in &sample.samples {
            if self.presentation_animator.active_retained_visual(
                lifecycle_sample.presentation_identity.scene_node_id(),
                PresentationRetainedVisualKind::WindowLifecycle,
            ) != Some(lifecycle_sample.presentation_identity)
            {
                continue;
            }
            let Some(payload) = self
                .retained_lifecycle_payloads
                .get_exact(lifecycle_sample.presentation_identity)
            else {
                continue;
            };
            if payload.payload_id != lifecycle_sample.payload_id
                || payload.root_surface_id != lifecycle_sample.root_surface_id
                || payload.window_id != lifecycle_sample.window_id
            {
                continue;
            }
            if let Some(decoration) = payload.frozen_decoration.as_ref().filter(|decoration| {
                decoration.root_surface_id() == lifecycle_sample.root_surface_id
            }) && frozen_roots.insert(lifecycle_sample.root_surface_id)
            {
                frozen.push(decoration.clone());
            }
        }
        frozen.extend(
            self.native_decoration_render_instances_for_scale(surfaces, 1.0)
                .into_iter()
                .filter(|decoration| {
                    roots.contains(&decoration.root_surface_id())
                        && !frozen_roots.contains(&decoration.root_surface_id())
                }),
        );
        frozen
    }

    pub(in crate::compositor) fn lifecycle_root_restore_suppressed(
        &self,
        root_surface_id: u32,
    ) -> bool {
        let at = AnimationTime::monotonic_now().unwrap_or(AnimationTime::from_nanos(0));
        self.lifecycle_scene_sample_at(at)
            .restore_suppresses_root(root_surface_id)
    }

    fn lifecycle_sample_has_visible_pixels(&self, sample: &LifecycleWindowSample) -> bool {
        crate::window_lifecycle_animation::lifecycle_sample_requires_physical_ack(
            sample.effect,
            sample.visual_group,
            sample.progress,
            sample.effect_opacity,
            sample.mathematically_settled,
            self.output_size.width,
            self.output_size.height,
        )
    }

    pub(in crate::compositor) fn settle_lifecycle_no_visual_change(&mut self) -> bool {
        let now = AnimationTime::monotonic_now().unwrap_or(AnimationTime::from_nanos(0));
        let candidates = self
            .lifecycle_scene_sample_at(now)
            .samples
            .into_iter()
            .filter(|sample| !self.lifecycle_sample_has_visible_pixels(sample))
            .filter(|sample| {
                !self
                    .presented_lifecycle_physical
                    .has_pending_visible_scene_node(
                        sample.presentation_identity.scene_node_id(),
                        self.output_size.width,
                        self.output_size.height,
                    )
            })
            .map(|sample| (sample.presentation_identity, sample.payload_id))
            .collect::<Vec<_>>();
        let mut settled = false;
        for (identity, payload_id) in candidates {
            if self
                .presentation_animator
                .active_retained_visual(identity.scene_node_id(), identity.kind())
                != Some(identity)
            {
                continue;
            }
            let Some(payload) = self.retained_lifecycle_payloads.get_exact(identity) else {
                continue;
            };
            if payload.payload_id != payload_id {
                continue;
            }
            if let Some(retired_identity) = self
                .window_lifecycle_animator
                .settle_no_visual_change(identity)
            {
                if !self
                    .presentation_animator
                    .retire_active_retained_visual_exact(retired_identity)
                {
                    continue;
                }
                self.retained_lifecycle_payloads.retire_exact(identity);
                settled = true;
            }
        }
        settled
    }

    pub(in crate::compositor) fn lifecycle_animation_has_pending_visible(&self) -> bool {
        let active_intersects = self
            .lifecycle_scene_sample_at(
                AnimationTime::monotonic_now().unwrap_or(AnimationTime::from_nanos(0)),
            )
            .samples
            .iter()
            .any(|sample| self.lifecycle_sample_has_visible_pixels(sample));
        active_intersects
            || self
                .presented_lifecycle_physical
                .has_pending_visible(self.output_size.width, self.output_size.height)
    }

    #[cfg(test)]
    pub(in crate::compositor) fn publish_presented_lifecycle(
        &mut self,
        frame_id: u64,
        snapshot: &LifecycleFrameSnapshot,
    ) {
        self.publish_presented_lifecycle_snapshot(
            frame_id,
            snapshot,
            crate::compositor::PresentedLifecycleScene::Initial,
        );
    }

    #[cfg(test)]
    pub(in crate::compositor) fn publish_presented_lifecycle_with_replacements(
        &mut self,
        frame_id: u64,
        snapshot: &LifecycleFrameSnapshot,
        canonical_root_surface_ids: &[u32],
        rendered_scene_replacement: bool,
    ) {
        let scene = if rendered_scene_replacement {
            crate::compositor::PresentedLifecycleScene::RenderedSceneReplacement {
                canonical_root_surface_ids,
            }
        } else {
            crate::compositor::PresentedLifecycleScene::Initial
        };
        self.publish_presented_lifecycle_snapshot(frame_id, snapshot, scene);
    }

    pub(in crate::compositor) fn publish_presented_lifecycle_snapshot(
        &mut self,
        frame_id: u64,
        snapshot: &LifecycleFrameSnapshot,
        scene: crate::compositor::PresentedLifecycleScene<'_>,
    ) {
        self.presented_lifecycle_physical.publish(
            frame_id,
            snapshot,
            scene,
            self.output_size.width,
            self.output_size.height,
        );
        for lifecycle_sample in &snapshot.samples {
            if self.presentation_animator.active_retained_visual(
                lifecycle_sample.presentation_identity.scene_node_id(),
                lifecycle_sample.presentation_identity.kind(),
            ) != Some(lifecycle_sample.presentation_identity)
            {
                continue;
            }
            if !self
                .retained_lifecycle_payloads
                .get_exact(lifecycle_sample.presentation_identity)
                .is_some_and(|payload| {
                    payload.payload_id == lifecycle_sample.payload_id
                        && payload.root_surface_id == lifecycle_sample.root_surface_id
                        && payload.window_id == lifecycle_sample.window_id
                })
            {
                continue;
            }
            let acknowledged = self.window_lifecycle_animator.acknowledge(
                lifecycle_sample.presentation_identity,
                lifecycle_sample.mathematically_settled,
            );
            let retired = acknowledged.is_some_and(|identity| {
                self.presentation_animator
                    .retire_active_retained_visual_exact(identity)
            });
            if retired {
                self.retained_lifecycle_payloads
                    .retire_exact(lifecycle_sample.presentation_identity);
            }
        }
        self.advance_pointer_hit_generation();
    }

    pub(in crate::compositor) fn set_lifecycle_animation_enabled(&mut self, enabled: bool) {
        let now = AnimationTime::monotonic_now().unwrap_or(AnimationTime::from_nanos(0));
        let active = self
            .presentation_animator
            .active_retained_visuals(PresentationRetainedVisualKind::WindowLifecycle);
        self.window_lifecycle_animator
            .set_enabled(enabled, &active, now);
    }

    pub(in crate::compositor) fn reconcile_lifecycle_animation_policy(&mut self) {
        let now = AnimationTime::monotonic_now().unwrap_or(AnimationTime::from_nanos(0));
        if !self.animation_control.configuration().enabled {
            let active = self.lifecycle_scene_sample_at(now).samples;
            for sample in active {
                self.window_lifecycle_animator
                    .snap_to_endpoint(sample.presentation_identity, now);
            }
        }
    }

    pub(in crate::compositor) fn apply_lifecycle_render_fallback(
        &mut self,
        fallback: LifecycleRenderFallbackEntry,
    ) -> bool {
        let now = AnimationTime::monotonic_now().unwrap_or(AnimationTime::from_nanos(0));
        if self.presentation_animator.active_retained_visual(
            fallback.presentation_identity.scene_node_id(),
            fallback.presentation_identity.kind(),
        ) != Some(fallback.presentation_identity)
        {
            return false;
        }
        let Some(current) = self
            .window_lifecycle_animator
            .sample(fallback.presentation_identity, now)
        else {
            return false;
        };
        let Some(payload) = self
            .retained_lifecycle_payloads
            .get_exact(fallback.presentation_identity)
        else {
            return false;
        };
        if payload.root_surface_id != fallback.root_surface_id
            || payload.window_id != fallback.window_id
            || payload.payload_id != fallback.payload_id
            || current.presentation_identity != fallback.presentation_identity
            || current.effect != fallback.effect
        {
            return false;
        }
        let Some(identity) = self
            .window_lifecycle_animator
            .retire_render_fallback(fallback.presentation_identity)
        else {
            return false;
        };
        if !self
            .presentation_animator
            .retire_active_retained_visual_exact(identity)
        {
            return false;
        }
        self.retained_lifecycle_payloads
            .retire_exact(fallback.presentation_identity);
        true
    }

    pub(in crate::compositor) fn set_lifecycle_animation_renderer_available(
        &mut self,
        available: bool,
    ) {
        self.set_lifecycle_animation_renderer_capabilities(
            available,
            self.lifecycle_squash_renderer_available.unwrap_or(false),
        );
    }

    pub(in crate::compositor) fn set_lifecycle_animation_renderer_capabilities(
        &mut self,
        lamp_available: bool,
        squash_available: bool,
    ) {
        self.lifecycle_animation_renderer_available = Some(lamp_available);
        self.lifecycle_squash_renderer_available = Some(squash_available);
        let unavailable = self
            .presentation_animator
            .active_retained_visuals(PresentationRetainedVisualKind::WindowLifecycle)
            .into_iter()
            .filter(|identity| {
                self.window_lifecycle_animator
                    .sample(*identity, AnimationTime::from_nanos(0))
                    .is_some_and(|sample| match sample.effect {
                        LifecycleEffectKind::Lamp => !lamp_available,
                        LifecycleEffectKind::Squash => !squash_available,
                    })
            })
            .collect::<Vec<_>>();
        for identity in unavailable {
            self.window_lifecycle_animator.cancel(identity);
            self.presentation_animator
                .retire_active_retained_visual_exact(identity);
            self.retained_lifecycle_payloads.retire_exact(identity);
        }
    }

    pub(in crate::compositor) fn lifecycle_cancel_window(&mut self, window_id: WindowId) {
        let scene_node_id = self.scene_node_id_for_window_group(window_id);
        let active_identity = scene_node_id.and_then(|scene_node_id| {
            self.presentation_animator.active_retained_visual(
                scene_node_id,
                PresentationRetainedVisualKind::WindowLifecycle,
            )
        });
        if let Some(identity) = active_identity {
            self.window_lifecycle_animator.cancel(identity);
            self.presentation_animator
                .retire_active_retained_visual_exact(identity);
            self.retained_lifecycle_payloads.retire_exact(identity);
        }
        let orphaned_executions = scene_node_id
            .map(|scene_node_id| {
                self.window_lifecycle_animator
                    .cancel_scene_executions(scene_node_id)
            })
            .unwrap_or_default();
        for identity in orphaned_executions {
            self.presentation_animator
                .retire_retained_visual_exact(identity);
            self.retained_lifecycle_payloads.retire_exact(identity);
        }
        let orphaned_payloads = scene_node_id
            .map(|scene_node_id| {
                self.retained_lifecycle_payloads
                    .remove_scene_node(scene_node_id)
            })
            .unwrap_or_default();
        for (identity, _) in orphaned_payloads {
            self.presentation_animator
                .retire_retained_visual_exact(identity);
            self.window_lifecycle_animator.cancel(identity);
        }
    }

    pub(in crate::compositor) fn lifecycle_teardown_window(&mut self, window_id: WindowId) {
        self.lifecycle_cancel_window(window_id);
        self.presented_lifecycle_physical.remove_window(window_id);
    }

    pub(in crate::compositor) const fn presented_lifecycle_frame_id(&self) -> u64 {
        self.presented_lifecycle_physical.frame_id()
    }
}
