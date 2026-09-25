use super::window_open_animation::window_open_animation_plan;
use super::*;
use crate::animation_control::AnimationEffect;
use crate::presentation_animation::{
    AnimationCurve, AnimationTime, EasingCurve, PresentationOpacity, PresentationRect,
    PresentationTransactionRequest,
};
use std::time::Duration;

fn rect(x: f64, y: f64, width: f64, height: f64) -> PresentationRect {
    PresentationRect::new(x, y, width, height).expect("valid presentation rectangle")
}

fn mapped_test_window() -> (CompositorState, WindowId, u32) {
    let mut state = CompositorState::new(None);
    let root_surface_id = 91;
    let window_id = state.allocate_window_id().expect("test window ID");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(window_id, root_surface_id))
        .expect("test XDG window");
    let placement = state.surface_placement(root_surface_id);
    state.append_renderable_surface(super::desktop_window_tests::x11_shm_surface(
        root_surface_id,
        500,
        400,
        placement,
    ));
    state
        .surface_presentation_generations
        .insert(root_surface_id, 1);
    state.rebuild_active_scene_view();
    state.presentation_animator.set_enabled(true);
    (state, window_id, root_surface_id)
}

#[test]
fn scale_plan_is_centered_at_ninety_four_percent_and_uses_canonical_opacity() {
    let target = rect(100.0, 200.0, 500.0, 400.0);
    let opacity = PresentationOpacity::new(0.37).expect("canonical opacity");
    let plan = window_open_animation_plan(AnimationEffect::WindowScale, target, opacity, 1.0)
        .expect("scale plan");

    assert_eq!(plan.geometry_start, rect(115.0, 212.0, 470.0, 376.0));
    assert_eq!(plan.opacity_start, PresentationOpacity::TRANSPARENT);
    assert_eq!(plan.opacity_target, opacity);
}

#[test]
fn glide_plan_keeps_size_and_starts_twenty_four_logical_pixels_below_target() {
    let target = rect(100.0, 200.0, 500.0, 400.0);
    let opacity = PresentationOpacity::new(0.37).expect("canonical opacity");
    let plan = window_open_animation_plan(AnimationEffect::WindowGlide, target, opacity, 1.0)
        .expect("glide plan");

    assert_eq!(plan.geometry_start, rect(100.0, 224.0, 500.0, 400.0));
    assert_eq!(plan.opacity_start, PresentationOpacity::TRANSPARENT);
    assert_eq!(plan.opacity_target, opacity);
}

#[test]
fn open_plan_uses_ease_out_cubic_and_existing_speed_scaling() {
    let target = rect(10.0, 20.0, 100.0, 80.0);
    let opacity = PresentationOpacity::new(0.6).expect("canonical opacity");
    for (speed, expected_duration) in [(1.0, 180), (2.0, 90), (0.5, 360)] {
        let plan = window_open_animation_plan(AnimationEffect::WindowScale, target, opacity, speed)
            .expect("scale plan");
        for curve in [plan.geometry_curve, plan.opacity_curve] {
            assert!(matches!(
                curve,
                AnimationCurve::Easing {
                    duration,
                    curve: EasingCurve::EaseOutCubic,
                } if duration == Duration::from_millis(expected_duration)
            ));
        }
    }
}

#[test]
fn free_geometry_commits_open_geometry_and_opacity_together_without_mutating_canonical_state() {
    let (mut state, window_id, root_surface_id) = mapped_test_window();
    let target_opacity = PresentationOpacity::new(0.42).expect("canonical opacity");
    state
        .window_mut(window_id)
        .expect("test window")
        .set_canonical_opacity(target_opacity);
    let canonical_geometry = state
        .current_root_window_geometry(root_surface_id)
        .expect("canonical geometry");
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("window group node");

    assert!(state.maybe_begin_window_open_animation(root_surface_id));

    assert!(
        state
            .presentation_animator
            .has_geometry_track(scene_node_id)
    );
    assert!(state.presentation_animator.has_opacity_track(scene_node_id));
    assert_eq!(
        state.presentation_animator.track_transaction(scene_node_id),
        state
            .presentation_animator
            .opacity_track_transaction(scene_node_id)
    );
    assert_eq!(
        state.current_root_window_geometry(root_surface_id),
        Some(canonical_geometry)
    );
    assert_eq!(
        state
            .window(window_id)
            .expect("test window")
            .canonical_opacity(),
        target_opacity
    );
    assert_eq!(state.presentation_animator.transaction_count(), 1);
}

#[test]
fn existing_geometry_owner_is_preserved_when_open_adds_opacity() {
    let (mut state, window_id, root_surface_id) = mapped_test_window();
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("window group node");
    let target = state
        .current_presentation_rect_for_root(root_surface_id)
        .expect("canonical presentation rectangle");
    let now = AnimationTime::monotonic_now().expect("monotonic time");
    state
        .presentation_animator
        .commit(PresentationTransactionRequest::geometry(
            now,
            vec![
                crate::presentation_animation::PresentationGeometryMutation::new(
                    scene_node_id,
                    rect(
                        target.x() - 40.0,
                        target.y(),
                        target.width(),
                        target.height(),
                    ),
                    target,
                    AnimationCurve::easing(Duration::from_millis(400), EasingCurve::Linear),
                ),
            ],
        ))
        .expect("existing geometry transition");
    let geometry_transaction = state
        .presentation_animator
        .track_transaction(scene_node_id)
        .expect("geometry transaction");

    assert!(state.maybe_begin_window_open_animation(root_surface_id));

    assert_eq!(
        state.presentation_animator.track_transaction(scene_node_id),
        Some(geometry_transaction)
    );
    assert!(state.presentation_animator.has_opacity_track(scene_node_id));
    assert_ne!(
        state
            .presentation_animator
            .opacity_track_transaction(scene_node_id),
        Some(geometry_transaction)
    );
    assert_eq!(state.presentation_animator.transaction_count(), 2);
}

#[test]
fn minimize_or_disabled_presentation_fails_open_without_open_tracks() {
    let (mut minimized, window_id, root_surface_id) = mapped_test_window();
    minimized
        .window_mut(window_id)
        .expect("test window")
        .state
        .mark_minimized_without_surfaces();
    assert!(!minimized.maybe_begin_window_open_animation(root_surface_id));

    let (mut disabled, window_id, root_surface_id) = mapped_test_window();
    let scene_node_id = disabled
        .scene_node_id_for_window_group(window_id)
        .expect("window group node");
    disabled.presentation_animator.set_enabled(false);
    assert!(!disabled.maybe_begin_window_open_animation(root_surface_id));
    assert!(!disabled.presentation_animator.has_track(scene_node_id));
}

#[test]
fn removing_a_window_during_open_cancels_both_open_properties() {
    let (mut state, window_id, root_surface_id) = mapped_test_window();
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("window group node");
    assert!(state.maybe_begin_window_open_animation(root_surface_id));
    assert!(
        state
            .presentation_animator
            .has_geometry_track(scene_node_id)
    );
    assert!(state.presentation_animator.has_opacity_track(scene_node_id));

    state
        .remove_desktop_window(window_id)
        .expect("remove window");

    assert!(!state.presentation_animator.has_track(scene_node_id));
    assert_eq!(state.presentation_animator.transaction_count(), 0);
}
