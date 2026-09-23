use super::*;
use crate::animation_control::{AnimationEffect, AnimationRuntimeCapabilities, AnimationSlot};
#[cfg(test)]
use crate::compositor::decoration::types::DecorationMode;
use crate::presentation_animation::PresentationRetainedVisualKind;
#[cfg(test)]
use crate::presentation_animation::{PresentationGroupTransform, TransitionId};
use crate::window_lifecycle_animation::{
    LampWindowSample, LifecycleDirection, LifecycleFrameSnapshot, LifecycleMotionRequest,
    LifecycleRenderFallbackEntry, LifecycleSceneSample, LifecycleVisualGroup,
    LifecycleVisualSource, LifecycleVisualSourceKind,
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
        }
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
        if self.lifecycle_effect(LifecycleDirection::Minimize) != AnimationEffect::MinimizeLamp {
            self.lifecycle_cancel_window(window_id);
            return;
        }
        let Some(scene_node_id) = self.scene_node_id_for_window_group(window_id) else {
            return;
        };
        let Some(now) = AnimationTime::monotonic_now() else {
            return;
        };
        let speed = self.animation_control.configuration().speed;
        let Some((_identity, payload, was_reversal)) = self.install_lifecycle_motion(
            scene_node_id,
            window_id,
            root_surface_id,
            presented_source_client_rect,
            canonical_client_rect,
            visual_group,
            LifecycleDirection::Minimize,
            resolved_effect_scene,
            now,
            speed,
        ) else {
            return;
        };
        // Lamp takes over Group Geometry only after the retained transaction
        // and lifecycle executor have both accepted the same identity.
        self.presentation_animator.cancel_geometry(scene_node_id);
        let active_root_surface_id = payload.root_surface_id;
        self.lifecycle_render_suppressed_roots
            .remove(&active_root_surface_id);
        if !was_reversal {
            self.replace_lifecycle_decoration_snapshot(
                active_root_surface_id,
                lifecycle_decorations,
            );
        }
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
        resolved_effect_scene: ResolvedEffectScene,
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
            LifecycleVisualGroup::from_bounds(
                canonical_client_rect,
                canonical_client_rect,
                presented_source_client_rect,
                anchor_rect,
                self.output_size.width,
                self.output_size.height,
            )?
        };
        if !crate::window_lifecycle_animation::lamp_footprint_intersects_output(
            visual_group,
            self.output_size.width,
            self.output_size.height,
        ) {
            return None;
        }

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

    fn replace_lifecycle_decoration_snapshot(
        &mut self,
        root_surface_id: u32,
        decorations: Vec<DecorationRenderInstance>,
    ) {
        self.lifecycle_decorations.remove(&root_surface_id);
        if let Some(decoration) = decorations
            .into_iter()
            .find(|decoration| decoration.root_surface_id() == root_surface_id)
        {
            self.lifecycle_decorations
                .insert(root_surface_id, decoration);
        }
    }

    pub(in crate::compositor) fn begin_lifecycle_restore(
        &mut self,
        window_id: WindowId,
        root_surface_id: u32,
        visual_group: Option<LifecycleVisualGroup>,
        resolved_effect_scene: ResolvedEffectScene,
        lifecycle_decorations: Vec<DecorationRenderInstance>,
    ) {
        if self.lifecycle_effect(LifecycleDirection::Restore) != AnimationEffect::MinimizeLamp {
            self.lifecycle_cancel_window(window_id);
            return;
        }
        let Some(scene_node_id) = self.scene_node_id_for_window_group(window_id) else {
            return;
        };
        let Some(now) = AnimationTime::monotonic_now() else {
            return;
        };
        let speed = self.animation_control.configuration().speed;
        let Some((_identity, payload, was_reversal)) = self.install_lifecycle_motion(
            scene_node_id,
            window_id,
            root_surface_id,
            None,
            None,
            visual_group,
            LifecycleDirection::Restore,
            resolved_effect_scene,
            now,
            speed,
        ) else {
            return;
        };
        let active_root_surface_id = payload.root_surface_id;
        if !was_reversal {
            self.replace_lifecycle_decoration_snapshot(
                active_root_surface_id,
                lifecycle_decorations,
            );
        }
        self.lifecycle_render_suppressed_roots
            .insert(active_root_surface_id);
    }

    pub(in crate::compositor) fn lifecycle_scene_sample_at(
        &self,
        at: AnimationTime,
    ) -> LifecycleSceneSample {
        let active_identities = self
            .presentation_animator
            .active_retained_visuals(PresentationRetainedVisualKind::WindowLifecycle);
        let mut lamps = Vec::with_capacity(active_identities.len());
        let mut visual_sources = Vec::with_capacity(active_identities.len());
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
            lamps.push(LampWindowSample {
                window_id: payload.window_id,
                root_surface_id: payload.root_surface_id,
                presentation_identity: identity,
                payload_id: payload.payload_id,
                visual_group: payload.visual_group,
                progress: motion.progress,
                opacity: motion.opacity,
                direction: motion.direction,
                mathematically_settled: motion.mathematically_settled,
            });
            visual_sources.push(LifecycleVisualSource {
                window_id: payload.window_id,
                root_surface_id: payload.root_surface_id,
                presentation_identity: identity,
                payload_id: payload.payload_id,
                kind,
                effect_scene: std::sync::Arc::clone(&payload.effect_scene),
            });
        }
        LifecycleSceneSample {
            sampled_at: at,
            lamps,
            visual_sources,
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
        let roots = sample
            .lamps
            .iter()
            .map(|lamp| lamp.root_surface_id)
            .collect::<HashSet<_>>();
        if roots.is_empty() {
            return Vec::new();
        }
        let mut surfaces = self
            .renderable_surfaces
            .iter()
            .filter(|surface| roots.contains(&self.root_surface_id_for_surface(surface.surface_id)))
            .cloned()
            .collect::<Vec<_>>();
        for lamp in &sample.lamps {
            if let Some(window) = self.window(lamp.window_id)
                && window.state.is_minimized()
            {
                surfaces.extend(window.state.minimized_surfaces().iter().cloned());
            }
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
            .lamps
            .iter()
            .map(|lamp| lamp.root_surface_id)
            .collect::<HashSet<_>>();
        let mut frozen = Vec::new();
        let mut frozen_roots = HashSet::new();
        for root_surface_id in roots.iter().copied() {
            if let Some(decoration) = self.lifecycle_decorations.get(&root_surface_id) {
                frozen.push(decoration.clone());
                frozen_roots.insert(root_surface_id);
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

    pub(in crate::compositor) fn lifecycle_surface_is_suppressed(&self, surface_id: u32) -> bool {
        self.lifecycle_render_suppressed_roots
            .contains(&self.root_surface_id_for_surface(surface_id))
    }

    pub(in crate::compositor) fn lifecycle_frame_snapshot_at(
        &self,
        at: AnimationTime,
    ) -> LifecycleFrameSnapshot {
        LifecycleFrameSnapshot::from_sample(&self.lifecycle_scene_sample_at(at))
    }

    fn lifecycle_lamp_intersects_output(
        &self,
        lamp: &crate::window_lifecycle_animation::LampWindowSample,
    ) -> bool {
        crate::window_lifecycle_animation::lamp_footprint_intersects_output(
            lamp.visual_group,
            self.output_size.width,
            self.output_size.height,
        )
    }

    fn lifecycle_lamp_has_visible_pixels(
        &self,
        lamp: &crate::window_lifecycle_animation::LampWindowSample,
    ) -> bool {
        lamp.opacity > f64::EPSILON && self.lifecycle_lamp_intersects_output(lamp)
    }

    pub(in crate::compositor) fn settle_lifecycle_no_visual_change(&mut self) -> bool {
        let now = AnimationTime::monotonic_now().unwrap_or(AnimationTime::from_nanos(0));
        let candidates = self
            .lifecycle_scene_sample_at(now)
            .lamps
            .into_iter()
            .filter(|lamp| !self.lifecycle_lamp_has_visible_pixels(lamp))
            .filter(|lamp| {
                !self.presented_lifecycle.lamps.iter().any(|presented| {
                    presented.presentation_identity.scene_node_id()
                        == lamp.presentation_identity.scene_node_id()
                        && !presented.mathematically_settled
                        && presented.opacity > f64::EPSILON
                        && crate::window_lifecycle_animation::lamp_footprint_intersects_output(
                            presented.visual_group,
                            self.output_size.width,
                            self.output_size.height,
                        )
                })
            })
            .map(|lamp| {
                (
                    lamp.presentation_identity,
                    lamp.payload_id,
                    lamp.root_surface_id,
                )
            })
            .collect::<Vec<_>>();
        let mut settled = false;
        for (identity, payload_id, root_surface_id) in candidates {
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
                self.lifecycle_render_suppressed_roots
                    .remove(&root_surface_id);
                self.lifecycle_decorations.remove(&root_surface_id);
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
            .lamps
            .iter()
            .any(|lamp| self.lifecycle_lamp_has_visible_pixels(lamp));
        let presented_intersects = self.presented_lifecycle.lamps.iter().any(|lamp| {
            !lamp.mathematically_settled
                && lamp.opacity > f64::EPSILON
                && crate::window_lifecycle_animation::lamp_footprint_intersects_output(
                    lamp.visual_group,
                    self.output_size.width,
                    self.output_size.height,
                )
        });
        active_intersects || presented_intersects
    }

    pub(in crate::compositor) fn lifecycle_render_suppressed_roots(&self) -> &HashSet<u32> {
        &self.lifecycle_render_suppressed_roots
    }

    pub(in crate::compositor) fn publish_presented_lifecycle(
        &mut self,
        frame_id: u64,
        snapshot: &LifecycleFrameSnapshot,
    ) {
        self.publish_presented_lifecycle_with_replacements(frame_id, snapshot, &[], false);
    }

    pub(in crate::compositor) fn publish_presented_lifecycle_with_replacements(
        &mut self,
        frame_id: u64,
        snapshot: &LifecycleFrameSnapshot,
        canonical_root_surface_ids: &[u32],
        rendered_scene_replacement: bool,
    ) {
        self.presented_lifecycle_frame_id = frame_id;
        let mut qualified = snapshot.clone();
        for old in &self.presented_lifecycle.lamps {
            let replaced = snapshot
                .lamps
                .iter()
                .any(|lamp| lamp.root_surface_id == old.root_surface_id);
            let canonical_replaced = canonical_root_surface_ids.contains(&old.root_surface_id);
            let rendered_replaced = rendered_scene_replacement && !replaced;
            let still_visible = !old.mathematically_settled
                && old.opacity > f64::EPSILON
                && crate::window_lifecycle_animation::lamp_footprint_intersects_output(
                    old.visual_group,
                    self.output_size.width,
                    self.output_size.height,
                );
            if !replaced && !canonical_replaced && !rendered_replaced && still_visible {
                qualified.lamps.push(*old);
            }
        }
        qualified.refresh_signature();
        self.presented_lifecycle = qualified;
        for lamp in &snapshot.lamps {
            if self.presentation_animator.active_retained_visual(
                lamp.presentation_identity.scene_node_id(),
                lamp.presentation_identity.kind(),
            ) != Some(lamp.presentation_identity)
            {
                continue;
            }
            if !self
                .retained_lifecycle_payloads
                .get_exact(lamp.presentation_identity)
                .is_some_and(|payload| {
                    payload.payload_id == lamp.payload_id
                        && payload.root_surface_id == lamp.root_surface_id
                        && payload.window_id == lamp.window_id
                })
            {
                continue;
            }
            let acknowledged = self
                .window_lifecycle_animator
                .acknowledge(lamp.presentation_identity, lamp.mathematically_settled);
            let retired = acknowledged.is_some_and(|identity| {
                self.presentation_animator
                    .retire_active_retained_visual_exact(identity)
            });
            if retired {
                self.retained_lifecycle_payloads
                    .retire_exact(lamp.presentation_identity);
            }
            if retired && matches!(lamp.direction, LifecycleDirection::Restore) {
                self.lifecycle_render_suppressed_roots
                    .remove(&lamp.root_surface_id);
            }
            if retired {
                self.lifecycle_decorations.remove(&lamp.root_surface_id);
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
        let active = self.lifecycle_scene_sample_at(now).lamps;
        for lamp in active {
            if self.lifecycle_effect(lamp.direction) != AnimationEffect::MinimizeLamp {
                self.window_lifecycle_animator
                    .snap_to_endpoint(lamp.presentation_identity, now);
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
        self.lifecycle_render_suppressed_roots
            .remove(&fallback.root_surface_id);
        self.lifecycle_decorations.remove(&fallback.root_surface_id);
        true
    }

    pub(in crate::compositor) fn set_lifecycle_animation_renderer_available(
        &mut self,
        available: bool,
    ) {
        self.lifecycle_animation_renderer_available = Some(available);
        if !available {
            let active = self
                .presentation_animator
                .active_retained_visuals(PresentationRetainedVisualKind::WindowLifecycle);
            for identity in active {
                self.window_lifecycle_animator.cancel(identity);
                self.presentation_animator
                    .retire_active_retained_visual_exact(identity);
                self.retained_lifecycle_payloads.retire_exact(identity);
            }
            for identity in self.window_lifecycle_animator.cancel_all() {
                self.presentation_animator
                    .retire_retained_visual_exact(identity);
                self.retained_lifecycle_payloads.retire_exact(identity);
            }
            for (identity, _) in self.retained_lifecycle_payloads.drain_all() {
                self.presentation_animator
                    .retire_retained_visual_exact(identity);
                self.window_lifecycle_animator.cancel(identity);
            }
            self.lifecycle_render_suppressed_roots.clear();
            self.lifecycle_decorations.clear();
        }
    }

    pub(in crate::compositor) fn lifecycle_cancel_window(&mut self, window_id: WindowId) {
        let root_surface_id = self.window(window_id).map(|window| window.root_surface_id);
        let scene_node_id = self.scene_node_id_for_window_group(window_id);
        let active_identity = scene_node_id.and_then(|scene_node_id| {
            self.presentation_animator.active_retained_visual(
                scene_node_id,
                PresentationRetainedVisualKind::WindowLifecycle,
            )
        });
        let frozen_root_surface_id = active_identity
            .and_then(|identity| self.retained_lifecycle_payloads.get_exact(identity))
            .map(|payload| payload.root_surface_id);
        if let Some(identity) = active_identity {
            self.window_lifecycle_animator.cancel(identity);
            self.presentation_animator
                .retire_active_retained_visual_exact(identity);
            self.retained_lifecycle_payloads.retire_exact(identity);
        }
        let orphaned_roots = scene_node_id
            .map(|scene_node_id| {
                self.window_lifecycle_animator
                    .cancel_scene_executions(scene_node_id)
            })
            .unwrap_or_default()
            .into_iter()
            .filter_map(|identity| {
                self.presentation_animator
                    .retire_retained_visual_exact(identity);
                self.retained_lifecycle_payloads
                    .retire_exact(identity)
                    .map(|payload| payload.root_surface_id)
            })
            .collect::<Vec<_>>();
        let orphaned_payloads = scene_node_id
            .map(|scene_node_id| {
                self.retained_lifecycle_payloads
                    .remove_scene_node(scene_node_id)
            })
            .unwrap_or_default();
        for (identity, payload) in orphaned_payloads {
            self.presentation_animator
                .retire_retained_visual_exact(identity);
            self.window_lifecycle_animator.cancel(identity);
            self.lifecycle_render_suppressed_roots
                .remove(&payload.root_surface_id);
            self.lifecycle_decorations.remove(&payload.root_surface_id);
        }
        for root_surface_id in [frozen_root_surface_id, root_surface_id]
            .into_iter()
            .flatten()
            .chain(orphaned_roots)
        {
            self.lifecycle_render_suppressed_roots
                .remove(&root_surface_id);
            self.lifecycle_decorations.remove(&root_surface_id);
        }
    }

    pub(in crate::compositor) fn lifecycle_teardown_window(&mut self, window_id: WindowId) {
        let root_surface_id = self.window(window_id).map(|window| window.root_surface_id);
        self.lifecycle_cancel_window(window_id);
        self.presented_lifecycle
            .lamps
            .retain(|lamp| lamp.window_id != window_id);
        self.presented_lifecycle.refresh_signature();
        if let Some(root_surface_id) = root_surface_id {
            self.lifecycle_render_suppressed_roots
                .remove(&root_surface_id);
        }
    }

    pub(in crate::compositor) const fn presented_lifecycle_frame_id(&self) -> u64 {
        self.presented_lifecycle_frame_id
    }
}
