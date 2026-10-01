use super::*;
use crate::animation_control::{AnimationEffect, AnimationSlot, scale_curve};
use crate::presentation_animation::{
    AnimationCurve, AnimationTime, EasingCurve, PresentationGeometryMutation, PresentationOpacity,
    PresentationOpacityMutation, PresentationRect, PresentationRetainedVisualKind,
    PresentationTransactionRequest,
};
use std::time::Duration;

const WINDOW_OPEN_SCALE_FACTOR: f64 = 0.94;
const WINDOW_OPEN_GLIDE_OFFSET_LOGICAL_PIXELS: f64 = 24.0;
const WINDOW_OPEN_BASE_DURATION: Duration = Duration::from_millis(180);

#[derive(Debug, Clone, PartialEq)]
pub(super) struct WindowOpenAnimationPlan {
    pub(super) geometry_start: PresentationRect,
    pub(super) geometry_target: PresentationRect,
    pub(super) opacity_start: PresentationOpacity,
    pub(super) opacity_target: PresentationOpacity,
    pub(super) geometry_curve: AnimationCurve,
    pub(super) opacity_curve: AnimationCurve,
}

pub(super) fn window_open_animation_plan(
    effect: AnimationEffect,
    target: PresentationRect,
    canonical_opacity: PresentationOpacity,
    speed: f64,
) -> Option<WindowOpenAnimationPlan> {
    let geometry_start = match effect {
        AnimationEffect::WindowScale => {
            let width = target.width() * WINDOW_OPEN_SCALE_FACTOR;
            let height = target.height() * WINDOW_OPEN_SCALE_FACTOR;
            PresentationRect::new(
                target.x() + (target.width() - width) / 2.0,
                target.y() + (target.height() - height) / 2.0,
                width,
                height,
            )?
        }
        AnimationEffect::WindowGlide => PresentationRect::new(
            target.x(),
            target.y() + WINDOW_OPEN_GLIDE_OFFSET_LOGICAL_PIXELS,
            target.width(),
            target.height(),
        )?,
        _ => return None,
    };
    let curve = || {
        scale_curve(
            AnimationCurve::easing(WINDOW_OPEN_BASE_DURATION, EasingCurve::EaseOutCubic),
            speed,
        )
    };

    Some(WindowOpenAnimationPlan {
        geometry_start,
        geometry_target: target,
        opacity_start: PresentationOpacity::TRANSPARENT,
        opacity_target: canonical_opacity,
        geometry_curve: curve(),
        opacity_curve: curve(),
    })
}

impl CompositorState {
    pub(in crate::compositor) fn begin_window_open_animation_after_surface_tree_publication(
        &mut self,
        root_surface_id: u32,
    ) {
        if self.surface_tree_generation.is_some() {
            if !self
                .surface_tree_pending_window_open_animations
                .contains(&root_surface_id)
            {
                self.surface_tree_pending_window_open_animations
                    .push(root_surface_id);
            }
        } else {
            self.maybe_begin_window_open_animation(root_surface_id);
        }
    }

    pub(in crate::compositor) fn window_open_geometry_track_active(
        &self,
        root_surface_id: u32,
    ) -> bool {
        self.window_id_for_surface(root_surface_id)
            .and_then(|window_id| self.scene_node_id_for_window_group(window_id))
            .is_some_and(|scene_node_id| {
                self.presentation_animator.has_geometry_track(scene_node_id)
            })
    }

    pub(in crate::compositor) fn retarget_window_open_after_mode_transition(
        &mut self,
        root_surface_id: u32,
        presentation: ModeTransitionPresentation,
        geometry_track_was_active: bool,
    ) {
        if presentation != ModeTransitionPresentation::Unpresented
            || !geometry_track_was_active
            || self.window_open_geometry_track_active(root_surface_id)
        {
            return;
        }

        self.begin_window_open_animation_after_surface_tree_publication(root_surface_id);
    }

    pub(in crate::compositor) fn retarget_window_open_after_pending_normal_restore(
        &mut self,
        root_surface_id: u32,
        target_geometry_changed: bool,
    ) {
        if !target_geometry_changed
            || self.mode_transition_presentation(root_surface_id)
                != ModeTransitionPresentation::Unpresented
        {
            return;
        }
        let Some(window_id) = self.window_id_for_surface(root_surface_id) else {
            return;
        };
        let Some(scene_node_id) = self.scene_node_id_for_window_group(window_id) else {
            return;
        };
        if !self.presentation_animator.has_geometry_track(scene_node_id) {
            return;
        }

        self.presentation_animator.cancel_geometry(scene_node_id);
        self.begin_window_open_animation_after_surface_tree_publication(root_surface_id);
    }

    pub(in crate::compositor) fn maybe_begin_window_open_animation(
        &mut self,
        root_surface_id: u32,
    ) -> bool {
        let Some(window_id) = self.window_id_for_surface(root_surface_id) else {
            return false;
        };
        let Some(window) = self.window(window_id) else {
            return false;
        };
        if window.root_surface_id != root_surface_id
            || !window.is_workspace_managed()
            || window.state.is_minimized()
            || self.renderable_surface_index(root_surface_id).is_none()
            || !self.window_is_visible_in_active_scene(window_id)
        {
            return false;
        }
        let Some(scene_node_id) = self.scene_node_id_for_window_group(window_id) else {
            return false;
        };
        if self
            .presentation_animator
            .active_retained_visual(
                scene_node_id,
                PresentationRetainedVisualKind::WindowLifecycle,
            )
            .is_some()
        {
            return false;
        }

        let effect = self.animation_control.effective_effect(
            AnimationSlot::WindowOpen,
            self.animation_runtime_capabilities(),
        );
        if !matches!(
            effect,
            AnimationEffect::WindowScale | AnimationEffect::WindowGlide
        ) {
            return false;
        }

        let canonical_opacity = window.canonical_opacity();
        let canonical_geometry = window
            .x11_geometry
            .as_ref()
            .map(|geometry| geometry.frame)
            .or_else(|| self.current_visual_root_window_geometry(root_surface_id))
            .or_else(|| self.current_root_window_geometry(root_surface_id));
        let Some(target) = canonical_geometry
            .and_then(|geometry| self.presentation_rect_for_geometry(root_surface_id, geometry))
        else {
            return false;
        };
        let speed = self.animation_control.configuration().speed;
        let Some(plan) = window_open_animation_plan(effect, target, canonical_opacity, speed)
        else {
            return false;
        };
        let Some(now) = AnimationTime::monotonic_now() else {
            return false;
        };

        let opacity = PresentationOpacityMutation::new(
            scene_node_id,
            plan.opacity_start,
            plan.opacity_target,
            plan.opacity_curve,
        );
        let request = if self.presentation_animator.has_geometry_track(scene_node_id) {
            PresentationTransactionRequest::opacity(now, vec![opacity])
        } else {
            PresentationTransactionRequest::mixed(
                now,
                vec![PresentationGeometryMutation::new(
                    scene_node_id,
                    plan.geometry_start,
                    plan.geometry_target,
                    plan.geometry_curve,
                )],
                vec![opacity],
            )
        };

        self.presentation_animator.commit(request).is_ok()
    }
}
