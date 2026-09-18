use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EasingCurve {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    EaseInCubic,
    EaseOutCubic,
    EaseInOutCubic,
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
