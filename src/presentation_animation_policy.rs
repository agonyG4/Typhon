//! Presentation animation policy selection and geometry-transition curves.

use std::time::Duration;

use crate::presentation_animation::{AnimationCurve, EasingCurve, SpringSpec};

// A half-pixel residual is below the integer-compatible materialization scale.
// At 165 Hz, 8 px/s is about 0.048 px per frame; at 60 Hz it is about 0.133
// px per frame. This is a conservative perceptual threshold, not an Apple
// private WindowServer constant.
const MACOS_SETTLEMENT_DISPLACEMENT_PX: f64 = 0.5;
const MACOS_SETTLEMENT_VELOCITY_PX_PER_SEC: f64 = 8.0;

const fn macos_spring(stiffness: f64, damping: f64) -> AnimationCurve {
    AnimationCurve::spring(SpringSpec::new(stiffness, damping).with_settlement(
        MACOS_SETTLEMENT_DISPLACEMENT_PX,
        MACOS_SETTLEMENT_VELOCITY_PX_PER_SEC,
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationAnimationKind {
    ProgrammaticMove,
    ProgrammaticResize,
    LayoutReflow,
    MaximizeEnter,
    MaximizeExit,
    FullscreenEnter,
    FullscreenExit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationAnimationStyle {
    Macos,
    Kde,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentationAnimationPolicy {
    style: PresentationAnimationStyle,
}

impl PresentationAnimationPolicy {
    pub const fn macos() -> Self {
        Self {
            style: PresentationAnimationStyle::Macos,
        }
    }

    pub const fn kde() -> Self {
        Self {
            style: PresentationAnimationStyle::Kde,
        }
    }

    pub fn from_environment() -> Self {
        Self::from_style(presentation_animation_style_from_env(
            std::env::var("OBLIVION_ONE_ANIMATION_STYLE")
                .ok()
                .as_deref(),
        ))
    }

    pub const fn from_style(style: PresentationAnimationStyle) -> Self {
        Self { style }
    }

    pub const fn style(self) -> PresentationAnimationStyle {
        self.style
    }

    pub const fn curve_for(self, kind: PresentationAnimationKind) -> AnimationCurve {
        match (self.style, kind) {
            (PresentationAnimationStyle::Macos, PresentationAnimationKind::ProgrammaticMove) => {
                macos_spring(260.0, 34.0)
            }
            (PresentationAnimationStyle::Macos, PresentationAnimationKind::ProgrammaticResize) => {
                macos_spring(240.0, 32.0)
            }
            (PresentationAnimationStyle::Macos, PresentationAnimationKind::LayoutReflow) => {
                macos_spring(230.0, 30.0)
            }
            (PresentationAnimationStyle::Macos, PresentationAnimationKind::MaximizeEnter) => {
                macos_spring(210.0, 28.0)
            }
            (PresentationAnimationStyle::Macos, PresentationAnimationKind::MaximizeExit) => {
                macos_spring(220.0, 29.0)
            }
            (PresentationAnimationStyle::Macos, PresentationAnimationKind::FullscreenEnter) => {
                macos_spring(205.0, 28.0)
            }
            (PresentationAnimationStyle::Macos, PresentationAnimationKind::FullscreenExit) => {
                macos_spring(215.0, 29.0)
            }
            (PresentationAnimationStyle::Kde, PresentationAnimationKind::ProgrammaticMove) => {
                AnimationCurve::easing(Duration::from_millis(160), EasingCurve::EaseOutCubic)
            }
            (PresentationAnimationStyle::Kde, PresentationAnimationKind::ProgrammaticResize)
            | (PresentationAnimationStyle::Kde, PresentationAnimationKind::LayoutReflow) => {
                AnimationCurve::easing(Duration::from_millis(200), EasingCurve::EaseOutCubic)
            }
            (PresentationAnimationStyle::Kde, PresentationAnimationKind::MaximizeEnter)
            | (PresentationAnimationStyle::Kde, PresentationAnimationKind::MaximizeExit)
            | (PresentationAnimationStyle::Kde, PresentationAnimationKind::FullscreenEnter)
            | (PresentationAnimationStyle::Kde, PresentationAnimationKind::FullscreenExit) => {
                AnimationCurve::easing(Duration::from_millis(250), EasingCurve::EaseOutCubic)
            }
        }
    }
}

impl Default for PresentationAnimationPolicy {
    fn default() -> Self {
        Self::from_environment()
    }
}

pub fn presentation_animation_style_from_env(value: Option<&str>) -> PresentationAnimationStyle {
    match value {
        None | Some("") | Some("default") | Some("kde") => PresentationAnimationStyle::Kde,
        Some("macos") => PresentationAnimationStyle::Macos,
        Some(value) => {
            eprintln!(
                "oblivion-one presentation animation: unknown OBLIVION_ONE_ANIMATION_STYLE={value:?}; using kde policy"
            );
            PresentationAnimationStyle::Kde
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presentation_animation::{
        PresentationRect, PresentationVelocity, PresentationWindowTarget,
    };

    #[test]
    fn macos_policy_resolves_the_expected_curve_for_each_geometry_kind() {
        let expected = [
            (PresentationAnimationKind::ProgrammaticMove, 260.0, 34.0),
            (PresentationAnimationKind::ProgrammaticResize, 240.0, 32.0),
            (PresentationAnimationKind::LayoutReflow, 230.0, 30.0),
            (PresentationAnimationKind::MaximizeEnter, 210.0, 28.0),
            (PresentationAnimationKind::MaximizeExit, 220.0, 29.0),
            (PresentationAnimationKind::FullscreenEnter, 205.0, 28.0),
            (PresentationAnimationKind::FullscreenExit, 215.0, 29.0),
        ];

        let policy = PresentationAnimationPolicy::macos();
        for (kind, stiffness, damping) in expected {
            let AnimationCurve::Spring(spec) = policy.curve_for(kind) else {
                panic!("macOS policy should use spring curves for {kind:?}");
            };
            close(spec.stiffness(), stiffness);
            close(spec.damping(), damping);
            close(spec.displacement_epsilon(), 0.5);
            close(spec.velocity_epsilon(), 8.0);
        }
    }

    #[test]
    fn macos_policy_keeps_geometry_springs_near_critical_damping() {
        let kinds = [
            PresentationAnimationKind::ProgrammaticMove,
            PresentationAnimationKind::ProgrammaticResize,
            PresentationAnimationKind::LayoutReflow,
            PresentationAnimationKind::MaximizeEnter,
            PresentationAnimationKind::MaximizeExit,
            PresentationAnimationKind::FullscreenEnter,
            PresentationAnimationKind::FullscreenExit,
        ];

        for kind in kinds {
            let AnimationCurve::Spring(spec) = PresentationAnimationPolicy::macos().curve_for(kind)
            else {
                panic!("macOS policy should use spring curves for {kind:?}");
            };
            let damping_ratio = spec.damping() / (2.0 * spec.stiffness().sqrt());
            assert!(
                (0.90..=1.10).contains(&damping_ratio),
                "{kind:?}: {damping_ratio}"
            );
        }
    }

    #[test]
    fn macos_policy_springs_settle_a_1920_pixel_transition_within_800_ms() {
        let kinds = [
            PresentationAnimationKind::ProgrammaticMove,
            PresentationAnimationKind::ProgrammaticResize,
            PresentationAnimationKind::LayoutReflow,
            PresentationAnimationKind::MaximizeEnter,
            PresentationAnimationKind::MaximizeExit,
            PresentationAnimationKind::FullscreenEnter,
            PresentationAnimationKind::FullscreenExit,
        ];
        let start = PresentationRect::new(0.0, 0.0, 1.0, 1.0).expect("start rect");
        let target = PresentationRect::new(1_920.0, 0.0, 1.0, 1.0).expect("target rect");
        let target_window = PresentationWindowTarget::new(1, target);

        for kind in kinds {
            let mut animator = crate::presentation_animation::PresentationAnimator::enabled();
            animator
                .start(
                    1,
                    start,
                    target,
                    crate::presentation_animation::AnimationTime::from_nanos(0),
                    PresentationAnimationPolicy::macos().curve_for(kind),
                )
                .expect("transition should start");

            let initial = animator.sample_scene(
                crate::presentation_animation::AnimationTime::from_nanos(0),
                &[target_window],
            );
            assert!(
                !initial.windows[0].mathematically_settled,
                "{kind:?} settled at t=0"
            );

            let early = animator.sample_scene(
                crate::presentation_animation::AnimationTime::from_nanos(50_000_000),
                &[target_window],
            );
            assert!(
                !early.windows[0].mathematically_settled,
                "{kind:?} settled too early"
            );
            assert_ne!(early.windows[0].rect, target, "{kind:?} snapped too early");

            let settled = animator.sample_scene(
                crate::presentation_animation::AnimationTime::from_nanos(800_000_000),
                &[target_window],
            );
            assert!(
                settled.windows[0].mathematically_settled,
                "{kind:?} remained pending beyond the v1.1 ceiling"
            );
            assert_eq!(settled.windows[0].rect, target);
            assert_eq!(settled.windows[0].velocity, PresentationVelocity::default());
        }
    }

    #[test]
    fn macos_policy_settlement_envelope_is_deterministic_for_representative_displacements() {
        let kinds = [
            PresentationAnimationKind::ProgrammaticMove,
            PresentationAnimationKind::ProgrammaticResize,
            PresentationAnimationKind::LayoutReflow,
            PresentationAnimationKind::MaximizeEnter,
            PresentationAnimationKind::MaximizeExit,
            PresentationAnimationKind::FullscreenEnter,
            PresentationAnimationKind::FullscreenExit,
        ];
        let displacements = [200.0, 500.0, 1_000.0, 1_920.0];

        for kind in kinds {
            let mut envelope = Vec::new();
            for displacement in displacements {
                let start = PresentationRect::new(0.0, 0.0, 1.0, 1.0).expect("start rect");
                let target =
                    PresentationRect::new(displacement, 0.0, 1.0, 1.0).expect("target rect");
                let target_window = PresentationWindowTarget::new(1, target);
                let mut animator = crate::presentation_animation::PresentationAnimator::enabled();
                animator
                    .start(
                        1,
                        start,
                        target,
                        crate::presentation_animation::AnimationTime::from_nanos(0),
                        PresentationAnimationPolicy::macos().curve_for(kind),
                    )
                    .expect("transition should start");

                let settled_at_ms = (0u32..=800).find(|milliseconds| {
                    animator
                        .sample_scene(
                            crate::presentation_animation::AnimationTime::from_nanos(
                                u64::from(*milliseconds) * 1_000_000,
                            ),
                            &[target_window],
                        )
                        .windows[0]
                        .mathematically_settled
                });
                let settled_at_ms = settled_at_ms.unwrap_or_else(|| {
                    panic!("{kind:?} did not settle {displacement} px by 800 ms")
                });
                assert!(settled_at_ms <= 800);
                envelope.push(settled_at_ms);
            }
            eprintln!("macos settlement envelope {kind:?}: {envelope:?} ms");
        }
    }

    #[test]
    fn animation_style_aliases_resolve_to_the_intended_policy() {
        for value in [None, Some(""), Some("default"), Some("kde")] {
            assert_eq!(
                presentation_animation_style_from_env(value),
                PresentationAnimationStyle::Kde
            );
        }
        assert_eq!(
            presentation_animation_style_from_env(Some("macos")),
            PresentationAnimationStyle::Macos
        );
        assert_eq!(
            presentation_animation_style_from_env(Some("unknown")),
            PresentationAnimationStyle::Kde
        );
    }

    #[test]
    fn kde_policy_uses_the_fixed_duration_geometry_table() {
        let expected = [
            (PresentationAnimationKind::ProgrammaticMove, 160),
            (PresentationAnimationKind::ProgrammaticResize, 200),
            (PresentationAnimationKind::LayoutReflow, 200),
            (PresentationAnimationKind::MaximizeEnter, 250),
            (PresentationAnimationKind::MaximizeExit, 250),
            (PresentationAnimationKind::FullscreenEnter, 250),
            (PresentationAnimationKind::FullscreenExit, 250),
        ];

        for (kind, duration_ms) in expected {
            assert_eq!(
                PresentationAnimationPolicy::kde().curve_for(kind),
                AnimationCurve::easing(
                    Duration::from_millis(duration_ms),
                    EasingCurve::EaseOutCubic,
                )
            );
        }
    }

    fn close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-6, "{actual} != {expected}");
    }
}
