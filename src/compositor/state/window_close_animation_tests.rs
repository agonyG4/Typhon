use super::window_close_animation::window_close_animation_plan;
use crate::animation_control::AnimationEffect;
use crate::presentation_animation::{
    AnimationCurve, EasingCurve, PresentationOpacity, PresentationRect,
};
use std::time::Duration;

fn rect(x: f64, y: f64, width: f64, height: f64) -> PresentationRect {
    PresentationRect::new(x, y, width, height).expect("valid presentation rectangle")
}

#[test]
fn scale_close_uses_physical_source_and_centered_ninety_four_percent_target() {
    let source = rect(100.0, 200.0, 500.0, 400.0);
    let source_opacity = PresentationOpacity::new(0.37).expect("presented opacity");
    let plan =
        window_close_animation_plan(AnimationEffect::WindowScale, source, source_opacity, 1.0)
            .expect("scale close plan");

    assert_eq!(plan.geometry_start, source);
    assert_eq!(plan.geometry_target, rect(115.0, 212.0, 470.0, 376.0));
    assert_eq!(plan.opacity_start, source_opacity);
    assert_eq!(plan.opacity_target, PresentationOpacity::TRANSPARENT);
}

#[test]
fn glide_close_recedes_about_the_bottom_edge_like_kwin() {
    let source = rect(33.0, 71.0, 511.0, 297.0);
    let source_opacity = PresentationOpacity::new(0.63).expect("presented opacity");
    let plan =
        window_close_animation_plan(AnimationEffect::WindowGlide, source, source_opacity, 1.0)
            .expect("glide close plan");

    assert_eq!(plan.geometry_start, source);
    let target = plan.geometry_target;
    // Pushed back to about 97% and anchored at the bottom, centered.
    assert!(target.width() < source.width() && target.width() > source.width() * 0.95);
    assert!((target.y() + target.height() - (source.y() + source.height())).abs() < 1.0e-9);
    assert!(
        (target.x() + target.width() / 2.0 - (source.x() + source.width() / 2.0)).abs() < 1.0e-9
    );
    assert_eq!(plan.opacity_start, source_opacity);
    assert_eq!(plan.opacity_target, PresentationOpacity::TRANSPARENT);
    assert!(matches!(
        plan.opacity_curve,
        AnimationCurve::Easing {
            duration,
            curve: EasingCurve::QtOutCurve,
        } if duration == Duration::from_millis(160)
    ));
}

#[test]
fn close_uses_ease_in_cubic_and_existing_speed_scaling() {
    let source = rect(10.0, 20.0, 100.0, 80.0);
    let opacity = PresentationOpacity::new(0.6).expect("presented opacity");

    for (speed, expected_duration) in [(1.0, 180), (2.0, 90), (0.5, 360)] {
        let plan =
            window_close_animation_plan(AnimationEffect::WindowScale, source, opacity, speed)
                .expect("scale close plan");
        for curve in [plan.geometry_curve, plan.opacity_curve] {
            assert!(matches!(
                curve,
                AnimationCurve::Easing {
                    duration,
                    curve: EasingCurve::EaseInCubic,
                } if duration == Duration::from_millis(expected_duration)
            ));
        }
    }
}

#[test]
fn close_planner_rejects_non_window_effects_and_invalid_speed() {
    let source = rect(0.0, 0.0, 100.0, 100.0);
    for (effect, speed) in [
        (AnimationEffect::None, 1.0),
        (AnimationEffect::MinimizeLamp, 1.0),
        (AnimationEffect::WindowScale, f64::NAN),
    ] {
        assert!(
            window_close_animation_plan(effect, source, PresentationOpacity::OPAQUE, speed,)
                .is_none()
        );
    }
}

#[test]
fn popin_close_shrinks_to_the_five_pixel_minimum_and_fades_quickly() {
    let source = rect(100.0, 200.0, 500.0, 400.0);
    let source_opacity = PresentationOpacity::new(0.8).expect("presented opacity");
    let plan =
        window_close_animation_plan(AnimationEffect::WindowPopin, source, source_opacity, 1.0)
            .expect("popin close plan");

    assert_eq!(plan.geometry_start, source);
    assert_eq!(plan.geometry_target, rect(347.5, 397.5, 5.0, 5.0));
    assert_eq!(plan.opacity_start, source_opacity);
    assert_eq!(plan.opacity_target, PresentationOpacity::TRANSPARENT);
    assert_eq!(
        plan.geometry_curve,
        AnimationCurve::easing(Duration::from_millis(250), EasingCurve::EaseOutExpo)
    );
    assert_eq!(
        plan.opacity_curve,
        AnimationCurve::easing(Duration::from_millis(150), EasingCurve::Quick)
    );
}
