use super::*;
use crate::core::SceneNodeId;
use std::time::Duration;

fn rect(x: f64, y: f64, width: f64, height: f64) -> PresentationRect {
    PresentationRect::new(x, y, width, height).expect("valid presentation rect")
}

fn close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1.0e-6, "{actual} != {expected}");
}

#[test]
fn easing_has_exact_start_and_end_samples() {
    let transition = PresentationTransition::new(
        rect(0.0, 10.0, 100.0, 80.0),
        rect(100.0, 50.0, 200.0, 160.0),
        AnimationTime::from_nanos(1_000),
        AnimationCurve::easing(Duration::from_nanos(1_000), EasingCurve::Linear),
    );
    assert_eq!(
        transition.sample(AnimationTime::from_nanos(1_000)).rect,
        rect(0.0, 10.0, 100.0, 80.0)
    );
    let end = transition.sample(AnimationTime::from_nanos(2_000));
    assert_eq!(end.rect, rect(100.0, 50.0, 200.0, 160.0));
    assert!(end.mathematically_settled);
}

#[test]
fn analytical_springs_settle_without_per_frame_integration() {
    for spec in [
        SpringSpec::new(100.0, 10.0),
        SpringSpec::new(100.0, 20.0),
        SpringSpec::new(100.0, 30.0),
    ] {
        let transition = PresentationTransition::new(
            rect(0.0, 0.0, 10.0, 10.0),
            rect(100.0, 0.0, 10.0, 10.0),
            AnimationTime::from_nanos(0),
            AnimationCurve::spring(spec),
        );
        let sample = transition.sample(AnimationTime::from_nanos(3_000_000_000));
        assert!(sample.mathematically_settled);
        assert_eq!(sample.rect, rect(100.0, 0.0, 10.0, 10.0));
        assert_eq!(sample.velocity, PresentationVelocity::default());
    }
}

#[test]
fn absolute_sampling_is_repeatable_after_a_long_gap() {
    let transition = PresentationTransition::new(
        rect(0.0, 0.0, 10.0, 10.0),
        rect(10.0, 10.0, 20.0, 20.0),
        AnimationTime::from_nanos(100),
        AnimationCurve::spring(SpringSpec::new(90.0, 12.0)),
    );
    let at = AnimationTime::from_nanos(40_000_000);
    assert_eq!(transition.sample(at), transition.sample(at));
}

#[test]
fn easing_retarget_preserves_start_velocity() {
    let mut engine = PresentationEngine::enabled();
    let curve = AnimationCurve::easing(Duration::from_secs(1), EasingCurve::EaseOut);
    engine.start(
        8,
        rect(0.0, 0.0, 100.0, 100.0),
        rect(100.0, 0.0, 100.0, 100.0),
        AnimationTime::from_nanos(0),
        curve,
    );
    let at = AnimationTime::from_nanos(300_000_000);
    let before = engine.sample_compat(8, at).expect("active sample");
    engine.retarget(8, rect(0.0, 80.0, 120.0, 90.0), at, curve);
    let after = engine.sample_compat(8, at).expect("retargeted sample");
    close(after.velocity.x(), before.velocity.x());
    close(after.velocity.y(), before.velocity.y());
}

#[test]
fn presentation_damage_rounds_outward() {
    assert_eq!(
        presentation_damage(rect(1.1, 2.2, 10.1, 8.1), rect(10.9, 12.8, 10.1, 8.1)),
        PresentationDamageRect::new(1, 2, 20, 19)
    );
}

#[test]
fn geometry_transform_maps_and_inverts_fractional_rectangles() {
    let transform = PresentationGeometryTransform::new(
        rect(10.0, 20.0, 100.0, 80.0),
        rect(12.5, 24.25, 125.0, 100.0),
    );
    let mapped = transform
        .map_rect(rect(20.0, 30.0, 10.0, 8.0))
        .expect("mapped rect");
    assert_eq!(mapped, rect(25.0, 36.75, 12.5, 10.0));
    let canonical = transform
        .inverse_map_point_unbounded((31.25, 44.25))
        .expect("inverse point");
    close(canonical.0, 25.0);
    close(canonical.1, 36.0);
}

#[test]
fn invalid_geometry_never_enters_the_engine() {
    let mut engine = PresentationEngine::enabled();
    let invalid = PresentationRect::from_raw_for_test(0.0, 0.0, f64::NAN, 10.0);
    assert_eq!(
        engine.commit(PresentationTransactionRequest::geometry(
            AnimationTime::from_nanos(0),
            vec![PresentationGeometryMutation::new(
                SceneNodeId::from_raw(71).expect("node"),
                invalid,
                rect(10.0, 0.0, 10.0, 10.0),
                AnimationCurve::easing(Duration::from_millis(1), EasingCurve::Linear),
            )],
        )),
        Err(PresentationTransactionError::InvalidGeometry)
    );
    assert_eq!(engine.active_count(), 0);
}
