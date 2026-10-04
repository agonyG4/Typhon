use super::*;
use crate::animation_control::{AnimationEffect, AnimationSlot, scale_curve};
use crate::presentation_animation::{
    AnimationCurve, AnimationTime, EasingCurve, PresentationGeometryMutation, PresentationOpacity,
    PresentationOpacityMutation, PresentationPropertyKind, PresentationRect,
    PresentationRetainedVisualKind, PresentationTransactionMember, PresentationTransactionRequest,
};
use std::time::Duration;

const WINDOW_OPEN_SCALE_FACTOR: f64 = 0.94;
const WINDOW_OPEN_GLIDE_OFFSET_LOGICAL_PIXELS: f64 = 24.0;
const WINDOW_OPEN_BASE_DURATION: Duration = Duration::from_millis(180);

#[derive(Debug, Clone, PartialEq)]
pub(super) struct WindowOpenAnimationPlan {
    pub(super) geometry_start: PresentationRect,
    pub(super) geometry_target: PresentationRect,
    pub(super) geometry_curve: AnimationCurve,
    pub(super) opacity: Option<WindowOpenOpacityAnimationPlan>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct WindowOpenOpacityAnimationPlan {
    pub(super) start: PresentationOpacity,
    pub(super) target: PresentationOpacity,
    pub(super) curve: AnimationCurve,
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

    let opacity = match effect {
        AnimationEffect::WindowScale => None,
        AnimationEffect::WindowGlide => Some(WindowOpenOpacityAnimationPlan {
            start: PresentationOpacity::TRANSPARENT,
            target: canonical_opacity,
            curve: curve(),
        }),
        _ => return None,
    };

    Some(WindowOpenAnimationPlan {
        geometry_start,
        geometry_target: target,
        geometry_curve: curve(),
        opacity,
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

    pub(in crate::compositor) fn window_open_presentation_activity(
        &self,
        root_surface_id: u32,
    ) -> WindowOpenPresentationActivity {
        let Some(ownership) = self
            .window_open_presentation_ownership
            .get(&root_surface_id)
        else {
            return WindowOpenPresentationActivity::default();
        };
        let Some(window_id) = self.window_id_for_surface(root_surface_id) else {
            return WindowOpenPresentationActivity::default();
        };
        if self
            .window(window_id)
            .is_none_or(|window| window.root_surface_id != root_surface_id)
            || self.scene_node_id_for_window_group(window_id) != Some(ownership.scene_node_id)
        {
            return WindowOpenPresentationActivity::default();
        }

        let member_is_exact = |member: Option<PresentationTransactionMember>, property| {
            member.is_some_and(|expected| {
                expected.scene_node_id() == ownership.scene_node_id
                    && expected.property() == Some(property)
                    && self
                        .presentation_animator
                        .active_transaction_member(ownership.scene_node_id, property)
                        == Some(expected)
            })
        };

        WindowOpenPresentationActivity {
            geometry_exact: member_is_exact(ownership.geometry, PresentationPropertyKind::Geometry),
            opacity_exact: member_is_exact(ownership.opacity, PresentationPropertyKind::Opacity),
        }
    }

    pub(in crate::compositor) fn window_open_presentation_active(
        &self,
        root_surface_id: u32,
    ) -> bool {
        self.window_open_presentation_activity(root_surface_id)
            .is_active()
    }

    pub(in crate::compositor) fn suppress_maximize_animation_for_active_window_open(
        &self,
        root_surface_id: u32,
        target_mode: ToplevelMode,
    ) -> bool {
        target_mode == ToplevelMode::Maximized
            && !self
                .animation_control
                .configuration()
                .animate_maximized_window_open
            && self.window_open_presentation_active(root_surface_id)
    }

    pub(in crate::compositor) fn cancel_window_open_presentation_ownership(
        &mut self,
        root_surface_id: u32,
    ) {
        let Some(ownership) = self
            .window_open_presentation_ownership
            .remove(&root_surface_id)
        else {
            return;
        };
        let Some(window_id) = self.window_id_for_surface(root_surface_id) else {
            return;
        };
        if self
            .window(window_id)
            .is_none_or(|window| window.root_surface_id != root_surface_id)
            || self.scene_node_id_for_window_group(window_id) != Some(ownership.scene_node_id)
        {
            return;
        }

        if let Some(member) = ownership.geometry {
            self.presentation_animator
                .cancel_presentation_member_exact(member);
        }
        if let Some(member) = ownership.opacity {
            self.presentation_animator
                .cancel_presentation_member_exact(member);
        }
    }

    pub(in crate::compositor) fn retarget_window_open_after_mode_transition(
        &mut self,
        root_surface_id: u32,
        presentation: ModeTransitionPresentation,
        window_open_was_active: bool,
    ) {
        if presentation != ModeTransitionPresentation::Unpresented
            || !window_open_was_active
            || self.window_open_presentation_active(root_surface_id)
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
        let activity = self.window_open_presentation_activity(root_surface_id);
        if !activity.geometry_exact {
            return;
        }

        if let Some(member) = self
            .window_open_presentation_ownership
            .get(&root_surface_id)
            .and_then(|ownership| ownership.geometry)
        {
            self.presentation_animator
                .cancel_presentation_member_exact(member);
        }
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
        if window.state.mode() == ToplevelMode::Maximized
            && !self
                .animation_control
                .configuration()
                .animate_maximized_window_open
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
        let geometry_is_owned = self.presentation_animator.has_geometry_track(scene_node_id);
        if geometry_is_owned && plan.opacity.is_none() {
            return false;
        }
        let Some(now) = AnimationTime::monotonic_now() else {
            return false;
        };

        let request = match (geometry_is_owned, plan.opacity) {
            (true, Some(opacity)) => PresentationTransactionRequest::opacity(
                now,
                vec![PresentationOpacityMutation::new(
                    scene_node_id,
                    opacity.start,
                    opacity.target,
                    opacity.curve,
                )],
            ),
            (false, Some(opacity)) => PresentationTransactionRequest::mixed(
                now,
                vec![PresentationGeometryMutation::new(
                    scene_node_id,
                    plan.geometry_start,
                    plan.geometry_target,
                    plan.geometry_curve,
                )],
                vec![PresentationOpacityMutation::new(
                    scene_node_id,
                    opacity.start,
                    opacity.target,
                    opacity.curve,
                )],
            ),
            (false, None) => PresentationTransactionRequest::geometry(
                now,
                vec![PresentationGeometryMutation::new(
                    scene_node_id,
                    plan.geometry_start,
                    plan.geometry_target,
                    plan.geometry_curve,
                )],
            ),
            (true, None) => return false,
        };

        let Ok(transaction) = self.presentation_animator.commit(request) else {
            return false;
        };
        let mut ownership = WindowOpenPresentationOwnership {
            scene_node_id,
            geometry: None,
            opacity: None,
        };
        for member in transaction.members().iter().copied() {
            if member.scene_node_id() != scene_node_id {
                continue;
            }
            match member.property() {
                Some(PresentationPropertyKind::Geometry) => ownership.geometry = Some(member),
                Some(PresentationPropertyKind::Opacity) => ownership.opacity = Some(member),
                Some(PresentationPropertyKind::Clip) | None => {}
            }
        }
        if ownership.geometry.is_none() && ownership.opacity.is_none() {
            return false;
        }
        self.window_open_presentation_ownership
            .insert(root_surface_id, ownership);
        true
    }
}
