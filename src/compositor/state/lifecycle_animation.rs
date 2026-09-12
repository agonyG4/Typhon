use super::*;
use crate::animation_control::{AnimationEffect, AnimationSlot};
use crate::presentation_animation::{PresentationGroupTransform, TransitionId};
use crate::window_lifecycle_animation::{
    LifecycleDirection, LifecycleFrameSnapshot, LifecycleSceneSample, LifecycleTransitionRequest,
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
        let transform = PresentationGroupTransform::new(
            0,
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

impl CompositorState {
    pub(in crate::compositor) fn lifecycle_effect(
        &self,
        direction: LifecycleDirection,
    ) -> AnimationEffect {
        if self.lifecycle_animation_renderer_available == Some(false) {
            return AnimationEffect::None;
        }
        let slot = match direction {
            LifecycleDirection::Minimize => AnimationSlot::WindowMinimize,
            LifecycleDirection::Restore => AnimationSlot::WindowRestore,
        };
        self.animation_control.effective_effect(slot)
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

    pub(in crate::compositor) fn begin_lifecycle_minimize(
        &mut self,
        window_id: WindowId,
        root_surface_id: u32,
        source_rect: Option<PresentationRect>,
        full_window_rect: Option<PresentationRect>,
        resolved_effect_scene: ResolvedEffectScene,
    ) {
        if self.lifecycle_effect(LifecycleDirection::Minimize) != AnimationEffect::MinimizeLamp {
            self.window_lifecycle_animator.cancel(window_id);
            self.lifecycle_render_suppressed_roots
                .remove(&root_surface_id);
            return;
        }
        let Some(source_rect) =
            source_rect.or_else(|| self.lifecycle_minimize_source_rect(root_surface_id))
        else {
            self.window_lifecycle_animator.cancel(window_id);
            self.lifecycle_render_suppressed_roots
                .remove(&root_surface_id);
            return;
        };
        let Some(full_window_rect) =
            full_window_rect.or_else(|| self.lifecycle_window_rect(root_surface_id))
        else {
            self.window_lifecycle_animator.cancel(window_id);
            self.lifecycle_render_suppressed_roots
                .remove(&root_surface_id);
            return;
        };
        let Some(anchor_rect) = self.lifecycle_anchor_rect(window_id) else {
            self.window_lifecycle_animator.cancel(window_id);
            self.lifecycle_render_suppressed_roots
                .remove(&root_surface_id);
            return;
        };
        let Some(now) = AnimationTime::monotonic_now() else {
            self.window_lifecycle_animator.cancel(window_id);
            self.lifecycle_render_suppressed_roots
                .remove(&root_surface_id);
            return;
        };
        let speed = self.animation_control.configuration().speed;
        // Lamp takes over the root's presentation pixels, but the last
        // pageflip-confirmed geometry remains authoritative for source
        // continuity and direct-scanout safety.
        self.presentation_animator.cancel(root_surface_id);
        self.lifecycle_render_suppressed_roots
            .remove(&root_surface_id);
        let _ = self.window_lifecycle_animator.start_or_reverse(
            LifecycleTransitionRequest {
                window_id,
                root_surface_id,
                source_rect,
                full_window_rect,
                anchor_rect,
                direction: LifecycleDirection::Minimize,
                resolved_effect_scene,
            },
            now,
            speed,
        );
    }

    pub(in crate::compositor) fn begin_lifecycle_restore(
        &mut self,
        window_id: WindowId,
        root_surface_id: u32,
        resolved_effect_scene: ResolvedEffectScene,
    ) {
        if self.lifecycle_effect(LifecycleDirection::Restore) != AnimationEffect::MinimizeLamp {
            self.window_lifecycle_animator.cancel(window_id);
            self.lifecycle_render_suppressed_roots
                .remove(&root_surface_id);
            return;
        }
        let Some(full_window_rect) = self.lifecycle_window_rect(root_surface_id) else {
            self.window_lifecycle_animator.cancel(window_id);
            self.lifecycle_render_suppressed_roots
                .remove(&root_surface_id);
            return;
        };
        let Some(anchor_rect) = self.lifecycle_anchor_rect(window_id) else {
            self.window_lifecycle_animator.cancel(window_id);
            self.lifecycle_render_suppressed_roots
                .remove(&root_surface_id);
            return;
        };
        let Some(now) = AnimationTime::monotonic_now() else {
            self.window_lifecycle_animator.cancel(window_id);
            self.lifecycle_render_suppressed_roots
                .remove(&root_surface_id);
            return;
        };
        let speed = self.animation_control.configuration().speed;
        if self
            .window_lifecycle_animator
            .start_or_reverse(
                LifecycleTransitionRequest {
                    window_id,
                    root_surface_id,
                    source_rect: full_window_rect,
                    full_window_rect,
                    anchor_rect,
                    direction: LifecycleDirection::Restore,
                    resolved_effect_scene,
                },
                now,
                speed,
            )
            .is_some()
        {
            self.lifecycle_render_suppressed_roots
                .insert(root_surface_id);
        } else {
            self.lifecycle_render_suppressed_roots
                .remove(&root_surface_id);
        }
    }

    pub(in crate::compositor) fn lifecycle_scene_sample_at(
        &self,
        at: AnimationTime,
    ) -> LifecycleSceneSample {
        self.window_lifecycle_animator.sample_scene(at)
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

    pub(in crate::compositor) fn lifecycle_animation_has_pending_visible(&self) -> bool {
        self.window_lifecycle_animator.has_pending_visible()
            || self.presented_lifecycle.contains_visible_non_identity()
    }

    pub(in crate::compositor) fn lifecycle_render_suppressed_roots(&self) -> &HashSet<u32> {
        &self.lifecycle_render_suppressed_roots
    }

    pub(in crate::compositor) fn publish_presented_lifecycle(
        &mut self,
        frame_id: u64,
        snapshot: &LifecycleFrameSnapshot,
    ) {
        self.presented_lifecycle_frame_id = frame_id;
        self.presented_lifecycle = snapshot.clone();
        for lamp in &snapshot.lamps {
            if self.window_lifecycle_animator.acknowledge(
                lamp.window_id,
                lamp.transition_id,
                lamp.mathematically_settled,
            ) && matches!(lamp.direction, LifecycleDirection::Restore)
            {
                self.lifecycle_render_suppressed_roots
                    .remove(&lamp.root_surface_id);
            }
        }
        self.advance_pointer_hit_generation();
    }

    pub(in crate::compositor) fn set_lifecycle_animation_enabled(&mut self, enabled: bool) {
        let now = AnimationTime::monotonic_now().unwrap_or(AnimationTime::from_nanos(0));
        self.window_lifecycle_animator.set_enabled(enabled, now);
    }

    pub(in crate::compositor) fn set_lifecycle_animation_renderer_available(
        &mut self,
        available: bool,
    ) {
        self.lifecycle_animation_renderer_available = Some(available);
        if !available {
            self.window_lifecycle_animator.cancel_all();
            self.lifecycle_render_suppressed_roots.clear();
        }
    }

    pub(in crate::compositor) fn lifecycle_cancel_window(&mut self, window_id: WindowId) {
        let root_surface_id = self.window(window_id).map(|window| window.root_surface_id);
        if let Some(root_surface_id) = root_surface_id {
            self.lifecycle_render_suppressed_roots
                .remove(&root_surface_id);
        }
        self.window_lifecycle_animator.cancel(window_id);
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
