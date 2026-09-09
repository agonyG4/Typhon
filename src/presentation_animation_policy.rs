//! Presentation animation policy selection and geometry-transition curves.

use crate::presentation_animation::{AnimationCurve, SpringSpec};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationAnimationKind {
    ProgrammaticMove,
    ProgrammaticResize,
    LayoutReflow,
    MaximizeEnter,
    MaximizeExit,
    FullscreenEnter,
    FullscreenExit,
    XwaylandModeChange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationAnimationStyle {
    Macos,
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
                AnimationCurve::spring(SpringSpec::new(260.0, 34.0))
            }
            (PresentationAnimationStyle::Macos, PresentationAnimationKind::ProgrammaticResize) => {
                AnimationCurve::spring(SpringSpec::new(240.0, 32.0))
            }
            (PresentationAnimationStyle::Macos, PresentationAnimationKind::LayoutReflow) => {
                AnimationCurve::spring(SpringSpec::new(230.0, 30.0))
            }
            (PresentationAnimationStyle::Macos, PresentationAnimationKind::MaximizeEnter) => {
                AnimationCurve::spring(SpringSpec::new(210.0, 28.0))
            }
            (PresentationAnimationStyle::Macos, PresentationAnimationKind::MaximizeExit) => {
                AnimationCurve::spring(SpringSpec::new(220.0, 29.0))
            }
            (PresentationAnimationStyle::Macos, PresentationAnimationKind::FullscreenEnter) => {
                AnimationCurve::spring(SpringSpec::new(205.0, 28.0))
            }
            (PresentationAnimationStyle::Macos, PresentationAnimationKind::FullscreenExit) => {
                AnimationCurve::spring(SpringSpec::new(215.0, 29.0))
            }
            (PresentationAnimationStyle::Macos, PresentationAnimationKind::XwaylandModeChange) => {
                AnimationCurve::spring(SpringSpec::new(230.0, 31.0))
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
        None | Some("") | Some("default") | Some("macos") => PresentationAnimationStyle::Macos,
        Some(value) => {
            eprintln!(
                "oblivion-one presentation animation: unknown OBLIVION_ONE_ANIMATION_STYLE={value:?}; using macos policy"
            );
            PresentationAnimationStyle::Macos
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            (PresentationAnimationKind::XwaylandModeChange, 230.0, 31.0),
        ];

        let policy = PresentationAnimationPolicy::macos();
        for (kind, stiffness, damping) in expected {
            let AnimationCurve::Spring(spec) = policy.curve_for(kind) else {
                panic!("macOS policy should use spring curves for {kind:?}");
            };
            close(spec.stiffness(), stiffness);
            close(spec.damping(), damping);
        }
    }

    #[test]
    fn animation_style_aliases_resolve_to_the_macos_policy() {
        for value in [None, Some(""), Some("default"), Some("macos")] {
            assert_eq!(
                presentation_animation_style_from_env(value),
                PresentationAnimationStyle::Macos
            );
        }
        assert_eq!(
            presentation_animation_style_from_env(Some("unknown")),
            PresentationAnimationStyle::Macos
        );
    }

    fn close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-6, "{actual} != {expected}");
    }
}
