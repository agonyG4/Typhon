use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EasingCurve {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    EaseInCubic,
    EaseOutCubic,
    /// The easeOutExpo cubic Bezier (0.16, 1, 0.3, 1).
    EaseOutExpo,
    EaseInOutCubic,
    /// Hyprland's easeOutBack cubic Bezier (0.34, 1.56, 0.64, 1); it
    /// overshoots past 1 before settling.
    EaseOutBack,
    /// Hyprland's "quick" cubic Bezier (0.15, 0, 0.1, 1).
    Quick,
    /// Qt's `QEasingCurve::InCurve`: a sine start blended into linear, which
    /// KWin's Glide uses for opening windows.
    QtInCurve,
    /// Qt's `QEasingCurve::OutCurve`, the mirror used for closing windows.
    QtOutCurve,
}

/// Qt's sine progress and smooth begin/end mix factor (qeasingcurve.cpp).
fn qt_sin_progress(progress: f64) -> (f64, f64) {
    let angle = progress * std::f64::consts::PI - std::f64::consts::FRAC_PI_2;
    (
        angle.sin() / 2.0 + 0.5,
        std::f64::consts::FRAC_PI_2 * angle.cos(),
    )
}

fn qt_mix_factor(value: f64) -> (f64, f64) {
    let raw = 1.0 - value * 2.0 + 0.3;
    if raw <= 0.0 {
        (0.0, 0.0)
    } else if raw >= 1.0 {
        (1.0, 0.0)
    } else {
        (raw, -2.0)
    }
}

fn qt_in_curve(progress: f64) -> (f64, f64) {
    let (sine, sine_velocity) = qt_sin_progress(progress);
    let (mix, mix_velocity) = qt_mix_factor(progress);
    (
        sine * mix + progress * (1.0 - mix),
        sine_velocity * mix + (sine - progress) * mix_velocity + (1.0 - mix),
    )
}

fn qt_out_curve(progress: f64) -> (f64, f64) {
    let (sine, sine_velocity) = qt_sin_progress(progress);
    let (mix, mix_velocity) = qt_mix_factor(1.0 - progress);
    (
        sine * mix + progress * (1.0 - mix),
        sine_velocity * mix - (sine - progress) * mix_velocity + (1.0 - mix),
    )
}

/// CSS-style cubic Bezier from (0, 0) to (1, 1) with control points
/// (x1, y1) and (x2, y2). Time is the curve's X coordinate, which is strictly
/// increasing for x1, x2 in [0, 1]; returns `(value, d value / d time)`.
fn cubic_bezier(x1: f64, y1: f64, x2: f64, y2: f64, progress: f64) -> (f64, f64) {
    let progress = progress.clamp(0.0, 1.0);
    let axis = |t: f64, p1: f64, p2: f64| {
        let remaining = 1.0 - t;
        3.0 * remaining * remaining * t * p1 + 3.0 * remaining * t * t * p2 + t * t * t
    };
    let slope = |t: f64, p1: f64, p2: f64| {
        let remaining = 1.0 - t;
        3.0 * remaining * remaining * p1
            + 6.0 * remaining * t * (p2 - p1)
            + 3.0 * t * t * (1.0 - p2)
    };
    let mut low = 0.0;
    let mut high = 1.0;
    for _ in 0..48 {
        let t = (low + high) * 0.5;
        if axis(t, x1, x2) < progress {
            low = t;
        } else {
            high = t;
        }
    }
    let t = if progress <= 0.0 {
        0.0
    } else if progress >= 1.0 {
        1.0
    } else {
        (low + high) * 0.5
    };
    let dx = slope(t, x1, x2);
    let dy = slope(t, y1, y2);
    let velocity = if dx.abs() > f64::EPSILON {
        dy / dx
    } else {
        0.0
    };
    (axis(t, y1, y2), velocity)
}

impl EasingCurve {
    pub(crate) fn evaluate(self, progress: f64) -> (f64, f64) {
        match self {
            Self::Linear => (progress, 1.0),
            Self::EaseIn => (progress * progress, 2.0 * progress),
            Self::EaseOut => {
                let remaining = 1.0 - progress;
                (1.0 - remaining * remaining, 2.0 * remaining)
            }
            Self::EaseInOut => {
                if progress < 0.5 {
                    (2.0 * progress * progress, 4.0 * progress)
                } else {
                    let remaining = 1.0 - progress;
                    (1.0 - 2.0 * remaining * remaining, 4.0 * remaining)
                }
            }
            Self::EaseInCubic => (progress * progress * progress, 3.0 * progress * progress),
            Self::EaseOutCubic => {
                let remaining = 1.0 - progress;
                (
                    1.0 - remaining * remaining * remaining,
                    3.0 * remaining * remaining,
                )
            }
            Self::EaseOutExpo => cubic_bezier(0.16, 1.0, 0.3, 1.0, progress),
            Self::EaseOutBack => cubic_bezier(0.34, 1.56, 0.64, 1.0, progress),
            Self::Quick => cubic_bezier(0.15, 0.0, 0.1, 1.0, progress),
            Self::QtInCurve => qt_in_curve(progress),
            Self::QtOutCurve => qt_out_curve(progress),
            Self::EaseInOutCubic => {
                if progress < 0.5 {
                    (
                        4.0 * progress * progress * progress,
                        12.0 * progress * progress,
                    )
                } else {
                    let remaining = 1.0 - progress;
                    (
                        1.0 - 4.0 * remaining * remaining * remaining,
                        12.0 * remaining * remaining,
                    )
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpringSpec {
    stiffness: f64,
    damping: f64,
    displacement_epsilon: f64,
    velocity_epsilon: f64,
}

impl SpringSpec {
    pub const fn new(stiffness: f64, damping: f64) -> Self {
        Self {
            stiffness,
            damping,
            displacement_epsilon: 0.01,
            velocity_epsilon: 0.01,
        }
    }

    pub const fn with_settlement(self, displacement_epsilon: f64, velocity_epsilon: f64) -> Self {
        Self {
            displacement_epsilon,
            velocity_epsilon,
            ..self
        }
    }

    pub const fn stiffness(self) -> f64 {
        self.stiffness
    }

    pub const fn damping(self) -> f64 {
        self.damping
    }

    pub const fn displacement_epsilon(self) -> f64 {
        self.displacement_epsilon
    }

    pub const fn velocity_epsilon(self) -> f64 {
        self.velocity_epsilon
    }

    pub(crate) fn valid(self) -> bool {
        self.stiffness.is_finite()
            && self.damping.is_finite()
            && self.stiffness > 0.0
            && self.damping >= 0.0
            && self.displacement_epsilon.is_finite()
            && self.velocity_epsilon.is_finite()
            && self.displacement_epsilon > 0.0
            && self.velocity_epsilon > 0.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AnimationCurve {
    Easing {
        duration: Duration,
        curve: EasingCurve,
    },
    Spring(SpringSpec),
}

impl AnimationCurve {
    pub const fn easing(duration: Duration, curve: EasingCurve) -> Self {
        Self::Easing { duration, curve }
    }

    pub const fn spring(spec: SpringSpec) -> Self {
        Self::Spring(spec)
    }

    pub(crate) fn sample_scalar(
        self,
        start: f64,
        target: f64,
        start_velocity: f64,
        elapsed_seconds: f64,
        preserve_start_velocity: bool,
    ) -> (f64, f64, bool) {
        match self {
            Self::Easing { duration, curve } => {
                let duration_seconds = duration.as_secs_f64();
                if duration_seconds <= 0.0 || elapsed_seconds >= duration_seconds {
                    return (target, 0.0, true);
                }
                let progress = (elapsed_seconds / duration_seconds).clamp(0.0, 1.0);
                if preserve_start_velocity {
                    let t = progress;
                    let t2 = t * t;
                    let t3 = t2 * t;
                    let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
                    let h10 = t3 - 2.0 * t2 + t;
                    let h01 = -2.0 * t3 + 3.0 * t2;
                    let h00_prime = 6.0 * t2 - 6.0 * t;
                    let h10_prime = 3.0 * t2 - 4.0 * t + 1.0;
                    let h01_prime = -6.0 * t2 + 6.0 * t;
                    let initial_tangent = start_velocity * duration_seconds;
                    return (
                        h00 * start + h10 * initial_tangent + h01 * target,
                        (h00_prime * start + h10_prime * initial_tangent + h01_prime * target)
                            / duration_seconds,
                        false,
                    );
                }
                let (progress, derivative) = curve.evaluate(progress);
                (
                    start + (target - start) * progress,
                    (target - start) * derivative / duration_seconds,
                    false,
                )
            }
            Self::Spring(spec) if spec.valid() => {
                let (displacement, velocity) = spring_scalar(
                    start - target,
                    start_velocity,
                    elapsed_seconds.max(0.0),
                    spec,
                );
                let settled = displacement.abs() <= spec.displacement_epsilon
                    && velocity.abs() <= spec.velocity_epsilon;
                if settled {
                    (target, 0.0, true)
                } else {
                    (target + displacement, velocity, false)
                }
            }
            Self::Spring(_) => (target, 0.0, true),
        }
    }
}

pub(crate) fn spring_scalar(
    displacement: f64,
    velocity: f64,
    elapsed_seconds: f64,
    spec: SpringSpec,
) -> (f64, f64) {
    let alpha = spec.damping / 2.0;
    let discriminant = alpha * alpha - spec.stiffness;
    if discriminant < -1e-12 {
        let omega_d = (-discriminant).sqrt();
        let a = displacement;
        let b = (velocity + alpha * displacement) / omega_d;
        let angle = omega_d * elapsed_seconds;
        let decay = (-alpha * elapsed_seconds).exp();
        let cosine = angle.cos();
        let sine = angle.sin();
        let base = a * cosine + b * sine;
        let derivative = -a * omega_d * sine + b * omega_d * cosine;
        (decay * base, decay * (derivative - alpha * base))
    } else if discriminant.abs() <= 1e-12 {
        let a = displacement;
        let b = velocity + alpha * displacement;
        let decay = (-alpha * elapsed_seconds).exp();
        let base = a + b * elapsed_seconds;
        (decay * base, decay * (b - alpha * base))
    } else {
        let root = discriminant.sqrt();
        let r1 = -alpha + root;
        let r2 = -alpha - root;
        let c1 = (velocity - r2 * displacement) / (r1 - r2);
        let c2 = displacement - c1;
        let first = c1 * (r1 * elapsed_seconds).exp();
        let second = c2 * (r2 * elapsed_seconds).exp();
        (first + second, r1 * first + r2 * second)
    }
}
