//! Deterministic compositor presentation transitions.
//!
//! This module owns temporary presentation state only. Canonical window/layout
//! geometry stays in the compositor and is sampled here at an absolute native
//! presentation timestamp.

use std::{cell::Cell, collections::BTreeMap, num::NonZeroU64, time::Duration};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AnimationTime(u64);

impl AnimationTime {
    pub const fn from_nanos(nanos: u64) -> Self {
        Self(nanos)
    }

    pub const fn as_nanos(self) -> u64 {
        self.0
    }

    pub fn monotonic_now() -> Option<Self> {
        let mut time = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) } != 0 {
            return None;
        }
        let seconds = u64::try_from(time.tv_sec).ok()?;
        let nanos = u64::try_from(time.tv_nsec).ok()?;
        seconds
            .checked_mul(1_000_000_000)?
            .checked_add(nanos)
            .map(Self)
    }

    fn elapsed_seconds(self, start: Self) -> f64 {
        self.0.saturating_sub(start.0) as f64 / 1_000_000_000.0
    }
}

/// Stable identity for one mathematical presentation transition.
///
/// The identity is deliberately separate from the sampled timestamp. A frame
/// rendered for an older transition may become physically visible after the
/// compositor has already retargeted the same root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TransitionId(NonZeroU64);

impl TransitionId {
    pub const fn new(value: NonZeroU64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresentationRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl PresentationRect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Option<Self> {
        let rect = Self {
            x,
            y,
            width,
            height,
        };
        (x.is_finite() && y.is_finite() && width.is_finite() && height.is_finite())
            .then_some(rect)
            .filter(|rect| rect.width > 0.0 && rect.height > 0.0)
    }

    pub const fn x(self) -> f64 {
        self.x
    }

    pub const fn y(self) -> f64 {
        self.y
    }

    pub const fn width(self) -> f64 {
        self.width
    }

    pub const fn height(self) -> f64 {
        self.height
    }

    pub fn is_identity_with(self, target: Self) -> bool {
        self == target
    }
}

/// One frame-local presentation owner and its canonical window-space rect.
///
/// The native output path constructs these only from its already-filtered
/// renderable surface set. They are not persistent compositor visibility
/// state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresentationWindowTarget {
    root_surface_id: u32,
    canonical_rect: PresentationRect,
}

impl PresentationWindowTarget {
    pub const fn new(root_surface_id: u32, canonical_rect: PresentationRect) -> Self {
        Self {
            root_surface_id,
            canonical_rect,
        }
    }

    pub const fn root_surface_id(self) -> u32 {
        self.root_surface_id
    }

    pub const fn canonical_rect(self) -> PresentationRect {
        self.canonical_rect
    }
}

/// Immutable frame-local presentation-owner targets.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NativeFramePresentationTargets {
    windows: Vec<PresentationWindowTarget>,
}

impl NativeFramePresentationTargets {
    pub(crate) fn from_windows(windows: Vec<PresentationWindowTarget>) -> Self {
        Self { windows }
    }

    pub fn windows(&self) -> &[PresentationWindowTarget] {
        &self.windows
    }

    pub fn root_surface_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.windows.iter().map(|window| window.root_surface_id())
    }
}

/// Frame-local mapping between canonical group space and the pixels rendered
/// for one presentation sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresentationGeometryTransform {
    pub canonical_rect: PresentationRect,
    pub presented_rect: PresentationRect,
}

impl PresentationGeometryTransform {
    pub const fn new(canonical_rect: PresentationRect, presented_rect: PresentationRect) -> Self {
        Self {
            canonical_rect,
            presented_rect,
        }
    }

    pub fn map_point(self, point: (f64, f64)) -> (f64, f64) {
        (
            self.presented_rect.x() + (point.0 - self.canonical_rect.x()) * self.scale_x(),
            self.presented_rect.y() + (point.1 - self.canonical_rect.y()) * self.scale_y(),
        )
    }

    /// Invert the presentation affine without imposing a client hit-test
    /// boundary. Callers decide whether the resulting canonical point is
    /// inside a client surface, decoration, or input region.
    pub fn inverse_map_point_unbounded(self, point: (f64, f64)) -> Option<(f64, f64)> {
        if !point.0.is_finite() || !point.1.is_finite() {
            return None;
        }
        let scale_x = self.scale_x();
        let scale_y = self.scale_y();
        if !scale_x.is_finite() || !scale_y.is_finite() || scale_x <= 0.0 || scale_y <= 0.0 {
            return None;
        }
        Some((
            self.canonical_rect.x() + (point.0 - self.presented_rect.x()) / scale_x,
            self.canonical_rect.y() + (point.1 - self.presented_rect.y()) / scale_y,
        ))
    }

    pub fn map_rect(self, rect: PresentationRect) -> Option<PresentationRect> {
        let top_left = self.map_point((rect.x(), rect.y()));
        let bottom_right = self.map_point((rect.x() + rect.width(), rect.y() + rect.height()));
        PresentationRect::new(
            top_left.0,
            top_left.1,
            bottom_right.0 - top_left.0,
            bottom_right.1 - top_left.1,
        )
    }

    pub fn scale_x(self) -> f64 {
        self.presented_rect.width() / self.canonical_rect.width()
    }

    pub fn scale_y(self) -> f64 {
        self.presented_rect.height() / self.canonical_rect.height()
    }

    pub fn is_identity(self) -> bool {
        self.canonical_rect == self.presented_rect
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresentationGroupTransform {
    pub root_surface_id: u32,
    pub transition_id: TransitionId,
    pub canonical_rect: PresentationRect,
    pub presented_rect: PresentationRect,
    pub mathematically_settled: bool,
}

impl PresentationGroupTransform {
    pub const fn new(
        root_surface_id: u32,
        transition_id: TransitionId,
        canonical_rect: PresentationRect,
        presented_rect: PresentationRect,
        mathematically_settled: bool,
    ) -> Self {
        Self {
            root_surface_id,
            transition_id,
            canonical_rect,
            presented_rect,
            mathematically_settled,
        }
    }

    pub fn map_point(self, point: (f64, f64)) -> (f64, f64) {
        self.geometry().map_point(point)
    }

    pub fn inverse_map_point(self, point: (f64, f64)) -> Option<(f64, f64)> {
        self.inverse_map_point_unbounded(point)
    }

    /// Invert the presentation affine without imposing a client hit-test
    /// boundary. Callers decide whether the resulting canonical point is
    /// inside a client surface, decoration, or input region.
    pub fn inverse_map_point_unbounded(self, point: (f64, f64)) -> Option<(f64, f64)> {
        self.geometry().inverse_map_point_unbounded(point)
    }

    pub fn map_rect(self, rect: PresentationRect) -> Option<PresentationRect> {
        self.geometry().map_rect(rect)
    }

    pub fn map_rect_outward(self, rect: PresentationRect) -> Option<PresentationDamageRect> {
        self.map_rect(rect)
            .map(|mapped| presentation_damage(mapped, mapped))
    }

    pub fn scale_x(self) -> f64 {
        self.geometry().scale_x()
    }

    pub fn scale_y(self) -> f64 {
        self.geometry().scale_y()
    }

    pub fn is_identity(self) -> bool {
        self.geometry().is_identity()
    }

    pub const fn geometry(self) -> PresentationGeometryTransform {
        PresentationGeometryTransform::new(self.canonical_rect, self.presented_rect)
    }

    /// Deterministic bounded identity for renderer geometry and diagnostics.
    pub fn signature(self) -> u64 {
        let mut signature = 0xcbf2_9ce4_8422_2325_u64;
        for value in [
            u64::from(self.root_surface_id),
            self.transition_id.get(),
            self.canonical_rect.x().to_bits(),
            self.canonical_rect.y().to_bits(),
            self.canonical_rect.width().to_bits(),
            self.canonical_rect.height().to_bits(),
            self.presented_rect.x().to_bits(),
            self.presented_rect.y().to_bits(),
            self.presented_rect.width().to_bits(),
            self.presented_rect.height().to_bits(),
            self.mathematically_settled as u64,
        ] {
            signature ^= value;
            signature = signature.wrapping_mul(0x1000_0000_01b3);
        }
        signature
    }
}

/// Metadata-only geometry for a toplevel/window that was actually consumed by
/// the renderer in one physically presentable frame. This is never raw root
/// `wl_surface` geometry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresentedWindowGeometry {
    root_surface_id: u32,
    presented_rect: PresentationRect,
}

impl PresentedWindowGeometry {
    pub const fn new(root_surface_id: u32, presented_rect: PresentationRect) -> Self {
        Self {
            root_surface_id,
            presented_rect,
        }
    }

    pub const fn root_surface_id(&self) -> u32 {
        self.root_surface_id
    }

    pub const fn presented_rect(self) -> PresentationRect {
        self.presented_rect
    }
}

/// Small metadata-only snapshot attached to a rendered frame and promoted
/// with the native scene history. It owns no client buffers.
#[derive(Debug, Clone, PartialEq)]
pub struct PresentationFrameSnapshot {
    pub sampled_at: AnimationTime,
    pub transforms: Vec<PresentationGroupTransform>,
    pub presented_windows: Vec<PresentedWindowGeometry>,
    pub signature: u64,
}

impl PresentationFrameSnapshot {
    pub fn empty() -> Self {
        Self::from_sample(&PresentationSceneSample::empty(AnimationTime::from_nanos(
            0,
        )))
    }

    pub fn from_sample(sample: &PresentationSceneSample) -> Self {
        Self::from_sample_with_presented_windows(sample, Vec::new())
    }

    pub fn from_sample_with_presented_windows(
        sample: &PresentationSceneSample,
        mut presented_windows: Vec<PresentedWindowGeometry>,
    ) -> Self {
        presented_windows.sort_unstable_by_key(|window| window.root_surface_id());
        let mut signature = 0xcbf2_9ce4_8422_2325_u64;
        for transform in &sample.transforms {
            signature ^= transform.signature();
            signature = signature.wrapping_mul(0x1000_0000_01b3);
        }
        for window in &presented_windows {
            signature ^= u64::from(window.root_surface_id());
            signature = signature.wrapping_mul(0x1000_0000_01b3);
            let rect = window.presented_rect();
            for value in [
                rect.x().to_bits(),
                rect.y().to_bits(),
                rect.width().to_bits(),
                rect.height().to_bits(),
            ] {
                signature ^= value;
                signature = signature.wrapping_mul(0x1000_0000_01b3);
            }
        }
        Self {
            sampled_at: sample.sampled_at,
            transforms: sample.transforms.clone(),
            presented_windows,
            signature,
        }
    }

    pub fn transform_for_root(&self, root_surface_id: u32) -> Option<PresentationGroupTransform> {
        self.transforms
            .iter()
            .find(|transform| transform.root_surface_id == root_surface_id)
            .copied()
    }

    pub fn presented_window_geometry(
        &self,
        root_surface_id: u32,
    ) -> Option<PresentedWindowGeometry> {
        self.presented_windows
            .binary_search_by_key(&root_surface_id, PresentedWindowGeometry::root_surface_id)
            .ok()
            .map(|index| self.presented_windows[index])
    }

    pub fn is_identity_for_root(&self, root_surface_id: u32) -> bool {
        self.transform_for_root(root_surface_id)
            .is_none_or(PresentationGroupTransform::is_identity)
    }

    pub fn refresh_signature(&mut self) {
        let mut signature = 0xcbf2_9ce4_8422_2325_u64;
        for transform in &self.transforms {
            signature ^= transform.signature();
            signature = signature.wrapping_mul(0x1000_0000_01b3);
        }
        for window in &self.presented_windows {
            signature ^= u64::from(window.root_surface_id());
            signature = signature.wrapping_mul(0x1000_0000_01b3);
            let rect = window.presented_rect();
            for value in [
                rect.x().to_bits(),
                rect.y().to_bits(),
                rect.width().to_bits(),
                rect.height().to_bits(),
            ] {
                signature ^= value;
                signature = signature.wrapping_mul(0x1000_0000_01b3);
            }
        }
        self.signature = signature;
    }
}

impl Default for PresentationFrameSnapshot {
    fn default() -> Self {
        Self::empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PresentationVelocity {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl PresentationVelocity {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub const fn x(self) -> f64 {
        self.x
    }

    pub const fn y(self) -> f64 {
        self.y
    }

    pub const fn width(self) -> f64 {
        self.width
    }

    pub const fn height(self) -> f64 {
        self.height
    }

    fn component(self, index: usize) -> f64 {
        match index {
            0 => self.x,
            1 => self.y,
            2 => self.width,
            _ => self.height,
        }
    }
}

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
    fn evaluate(self, progress: f64) -> (f64, f64) {
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

    fn valid(self) -> bool {
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

    fn sample_scalar(
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresentationWindowSample {
    pub key: u32,
    pub rect: PresentationRect,
    pub velocity: PresentationVelocity,
    pub transition_id: TransitionId,
    pub mathematically_settled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresentationTransition {
    transition_id: TransitionId,
    start: PresentationRect,
    target: PresentationRect,
    start_velocity: PresentationVelocity,
    started_at: AnimationTime,
    curve: AnimationCurve,
    preserve_start_velocity: bool,
}

impl PresentationTransition {
    pub const fn new(
        start: PresentationRect,
        target: PresentationRect,
        started_at: AnimationTime,
        curve: AnimationCurve,
    ) -> Self {
        let mut transition = Self::new_with_velocity(
            start,
            target,
            PresentationVelocity::new(0.0, 0.0, 0.0, 0.0),
            started_at,
            curve,
        );
        transition.preserve_start_velocity = false;
        transition
    }

    const fn with_id(mut self, transition_id: TransitionId) -> Self {
        self.transition_id = transition_id;
        self
    }

    pub const fn new_with_velocity(
        start: PresentationRect,
        target: PresentationRect,
        start_velocity: PresentationVelocity,
        started_at: AnimationTime,
        curve: AnimationCurve,
    ) -> Self {
        Self {
            transition_id: TransitionId::new(NonZeroU64::new(1).expect("non-zero transition id")),
            start,
            target,
            start_velocity,
            started_at,
            curve,
            preserve_start_velocity: true,
        }
    }

    pub const fn target(self) -> PresentationRect {
        self.target
    }

    pub fn sample(self, now: AnimationTime) -> PresentationWindowSample {
        let elapsed = now.elapsed_seconds(self.started_at);
        let starts = [
            self.start.x,
            self.start.y,
            self.start.width,
            self.start.height,
        ];
        let targets = [
            self.target.x,
            self.target.y,
            self.target.width,
            self.target.height,
        ];
        let mut values = [0.0; 4];
        let mut velocities = [0.0; 4];
        let mut settled = true;
        for index in 0..4 {
            let (value, velocity, component_settled) = self.curve.sample_scalar(
                starts[index],
                targets[index],
                self.start_velocity.component(index),
                elapsed,
                self.preserve_start_velocity,
            );
            values[index] = value;
            velocities[index] = velocity;
            settled &= component_settled;
        }
        PresentationWindowSample {
            key: 0,
            rect: PresentationRect {
                x: values[0],
                y: values[1],
                width: values[2].max(f64::MIN_POSITIVE),
                height: values[3].max(f64::MIN_POSITIVE),
            },
            velocity: PresentationVelocity::new(
                velocities[0],
                velocities[1],
                velocities[2],
                velocities[3],
            ),
            transition_id: self.transition_id,
            mathematically_settled: settled,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentationDamageRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl PresentationDamageRect {
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

pub fn presentation_damage(
    previous: PresentationRect,
    current: PresentationRect,
) -> PresentationDamageRect {
    let left = previous.x.min(current.x).floor();
    let top = previous.y.min(current.y).floor();
    let right = (previous.x + previous.width)
        .max(current.x + current.width)
        .ceil();
    let bottom = (previous.y + previous.height)
        .max(current.y + current.height)
        .ceil();
    let left_i64 = left.clamp(i32::MIN as f64, i32::MAX as f64) as i64;
    let top_i64 = top.clamp(i32::MIN as f64, i32::MAX as f64) as i64;
    let right_i64 = right.clamp(i32::MIN as f64, i32::MAX as f64) as i64;
    let bottom_i64 = bottom.clamp(i32::MIN as f64, i32::MAX as f64) as i64;
    PresentationDamageRect::new(
        left_i64 as i32,
        top_i64 as i32,
        (right_i64.saturating_sub(left_i64)).min(i64::from(u32::MAX)) as u32,
        (bottom_i64.saturating_sub(top_i64)).min(i64::from(u32::MAX)) as u32,
    )
}

#[derive(Debug, Clone, PartialEq)]
pub struct PresentationSceneSample {
    pub sampled_at: AnimationTime,
    pub windows: Vec<PresentationWindowSample>,
    pub transforms: Vec<PresentationGroupTransform>,
    pub active_transitions: usize,
    pub sampled_windows: usize,
}

impl PresentationSceneSample {
    pub fn empty(sampled_at: AnimationTime) -> Self {
        Self {
            sampled_at,
            windows: Vec::new(),
            transforms: Vec::new(),
            active_transitions: 0,
            sampled_windows: 0,
        }
    }

    pub fn transform_for_root(&self, root_surface_id: u32) -> Option<PresentationGroupTransform> {
        self.transforms
            .iter()
            .find(|transform| transform.root_surface_id == root_surface_id)
            .copied()
    }

    pub fn frame_snapshot(&self) -> PresentationFrameSnapshot {
        PresentationFrameSnapshot::from_sample(self)
    }

    pub fn geometry_signature(&self) -> u64 {
        self.frame_snapshot().signature
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PresentationAnimationMetrics {
    pub transitions_started: u64,
    pub transitions_retargeted: u64,
    pub transitions_cancelled: u64,
    pub transitions_acknowledged: u64,
    pub stale_acknowledgements: u64,
    pub sampled_windows: u64,
}

#[derive(Debug)]
pub struct PresentationAnimator {
    enabled: bool,
    transitions: BTreeMap<u32, PresentationTransition>,
    next_transition_id: NonZeroU64,
    metrics: PresentationAnimationMetrics,
    sampled_windows: Cell<u64>,
}

impl Default for PresentationAnimator {
    fn default() -> Self {
        if animation_policy_enabled(std::env::var("OBLIVION_ONE_ANIMATIONS").ok().as_deref()) {
            Self::enabled()
        } else {
            Self::disabled()
        }
    }
}

pub fn animation_policy_enabled(value: Option<&str>) -> bool {
    match value {
        None | Some("on") => true,
        Some("off") => false,
        Some(value) => {
            eprintln!(
                "oblivion-one presentation animation: unknown OBLIVION_ONE_ANIMATIONS={value:?}; using normal policy"
            );
            true
        }
    }
}

impl PresentationAnimator {
    pub fn enabled() -> Self {
        Self {
            enabled: true,
            transitions: BTreeMap::new(),
            next_transition_id: NonZeroU64::new(1).expect("non-zero transition id"),
            metrics: PresentationAnimationMetrics::default(),
            sampled_windows: Cell::new(0),
        }
    }

    pub fn disabled() -> Self {
        Self {
            enabled: false,
            transitions: BTreeMap::new(),
            next_transition_id: NonZeroU64::new(1).expect("non-zero transition id"),
            metrics: PresentationAnimationMetrics::default(),
            sampled_windows: Cell::new(0),
        }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.metrics.transitions_cancelled = self
                .metrics
                .transitions_cancelled
                .saturating_add(self.transitions.len() as u64);
            self.transitions.clear();
        }
    }

    pub const fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn retarget(
        &mut self,
        key: u32,
        target: PresentationRect,
        now: AnimationTime,
        curve: AnimationCurve,
    ) -> Option<TransitionId> {
        if !self.enabled {
            return None;
        }
        let existing = self.transitions.get(&key)?;
        let sample = existing.sample(now);
        let (start, velocity) = (sample.rect, sample.velocity);
        if start == target && velocity == PresentationVelocity::default() {
            return Some(sample.transition_id);
        }
        let transition_id = self.allocate_transition_id()?;
        self.transitions.insert(
            key,
            PresentationTransition::new_with_velocity(start, target, velocity, now, curve)
                .with_id(transition_id),
        );
        self.metrics.transitions_retargeted = self.metrics.transitions_retargeted.saturating_add(1);
        Some(transition_id)
    }

    pub fn cancel(&mut self, key: u32) {
        if self.transitions.remove(&key).is_some() {
            self.metrics.transitions_cancelled =
                self.metrics.transitions_cancelled.saturating_add(1);
        }
    }

    pub fn start(
        &mut self,
        key: u32,
        start: PresentationRect,
        target: PresentationRect,
        now: AnimationTime,
        curve: AnimationCurve,
    ) -> Option<TransitionId> {
        if !self.enabled {
            return None;
        }
        if start == target {
            self.transitions.remove(&key);
            return None;
        }
        let transition_id = self.allocate_transition_id()?;
        self.transitions.insert(
            key,
            PresentationTransition::new(start, target, now, curve).with_id(transition_id),
        );
        self.metrics.transitions_started = self.metrics.transitions_started.saturating_add(1);
        Some(transition_id)
    }

    pub fn sample(&self, key: u32, now: AnimationTime) -> Option<PresentationWindowSample> {
        self.transitions.get(&key).map(|transition| {
            let mut sample = transition.sample(now);
            sample.key = key;
            sample
        })
    }

    pub(crate) fn transition_curve(&self, key: u32) -> Option<AnimationCurve> {
        self.transitions
            .get(&key)
            .map(|transition| transition.curve)
    }

    #[cfg(test)]
    pub(crate) fn sample_at_transition_start(&self, key: u32) -> Option<PresentationWindowSample> {
        self.transitions.get(&key).map(|transition| {
            let mut sample = transition.sample(transition.started_at);
            sample.key = key;
            sample
        })
    }

    #[cfg(test)]
    pub(crate) fn transition_started_at(&self, key: u32) -> Option<AnimationTime> {
        self.transitions
            .get(&key)
            .map(|transition| transition.started_at)
    }

    pub fn has_pending_visible(&self, visible_keys: &[u32]) -> bool {
        visible_keys
            .iter()
            .any(|key| self.transitions.contains_key(key))
    }

    pub fn active_count(&self) -> usize {
        self.transitions.len()
    }

    pub fn sample_scene(
        &self,
        now: AnimationTime,
        windows: &[PresentationWindowTarget],
    ) -> PresentationSceneSample {
        let mut sampled_windows = 0;
        let mut sampled = Vec::new();
        let mut transforms = Vec::new();
        for window in windows {
            let key = window.root_surface_id();
            if let Some(sample) = self.sample(key, now) {
                sampled_windows += 1;
                let canonical_rect = window.canonical_rect();
                transforms.push(PresentationGroupTransform::new(
                    key,
                    sample.transition_id,
                    canonical_rect,
                    sample.rect,
                    sample.mathematically_settled,
                ));
                sampled.push(sample);
            }
        }
        self.sampled_windows.set(
            self.sampled_windows
                .get()
                .saturating_add(sampled_windows as u64),
        );
        PresentationSceneSample {
            sampled_at: now,
            windows: sampled,
            transforms,
            active_transitions: self.active_count(),
            sampled_windows,
        }
    }

    pub fn acknowledge_presented_transition(
        &mut self,
        root_surface_id: u32,
        transition_id: TransitionId,
        presented_rect: PresentationRect,
    ) -> bool {
        let Some(current) = self.transitions.get(&root_surface_id) else {
            return false;
        };
        let sample = current.sample(AnimationTime::from_nanos(u64::MAX));
        if current.transition_id != transition_id
            || !sample.mathematically_settled
            || sample.rect != presented_rect
        {
            self.metrics.stale_acknowledgements =
                self.metrics.stale_acknowledgements.saturating_add(1);
            return false;
        }
        self.transitions.remove(&root_surface_id);
        self.metrics.transitions_acknowledged =
            self.metrics.transitions_acknowledged.saturating_add(1);
        true
    }

    pub fn metrics(&self) -> PresentationAnimationMetrics {
        PresentationAnimationMetrics {
            sampled_windows: self.sampled_windows.get(),
            ..self.metrics
        }
    }

    fn allocate_transition_id(&mut self) -> Option<TransitionId> {
        let current = self.next_transition_id;
        let next = NonZeroU64::new(current.get().checked_add(1)?)?;
        self.next_transition_id = next;
        Some(TransitionId::new(current))
    }
}

fn spring_scalar(
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-6, "{actual} != {expected}");
    }

    fn rect(x: f64, y: f64, width: f64, height: f64) -> PresentationRect {
        PresentationRect::new(x, y, width, height).expect("valid presentation rect")
    }

    #[test]
    fn easing_has_exact_start_and_end_samples() {
        let transition = PresentationTransition::new(
            rect(0.0, 10.0, 100.0, 80.0),
            rect(100.0, 50.0, 200.0, 160.0),
            AnimationTime::from_nanos(1_000),
            AnimationCurve::easing(Duration::from_nanos(1_000), EasingCurve::Linear),
        );

        let start = transition.sample(AnimationTime::from_nanos(1_000));
        let end = transition.sample(AnimationTime::from_nanos(2_000));
        assert_eq!(start.rect, rect(0.0, 10.0, 100.0, 80.0));
        assert_eq!(end.rect, rect(100.0, 50.0, 200.0, 160.0));
        assert!(end.mathematically_settled);
    }

    #[test]
    fn cubic_easing_has_analytic_midpoints_and_endpoint_derivatives() {
        let expected = [
            (EasingCurve::EaseInCubic, 0.125, 0.75, 0.0, 3.0),
            (EasingCurve::EaseOutCubic, 0.875, 0.75, 3.0, 0.0),
            (EasingCurve::EaseInOutCubic, 0.5, 3.0, 0.0, 0.0),
        ];

        for (curve, midpoint, midpoint_derivative, start_derivative, end_derivative) in expected {
            let (value, derivative) = curve.evaluate(0.5);
            close(value, midpoint);
            close(derivative, midpoint_derivative);
            close(curve.evaluate(0.0).1, start_derivative);
            close(curve.evaluate(1.0).1, end_derivative);
        }
    }

    #[test]
    fn cubic_easing_is_monotonic_over_normalized_progress() {
        for curve in [
            EasingCurve::EaseInCubic,
            EasingCurve::EaseOutCubic,
            EasingCurve::EaseInOutCubic,
        ] {
            let mut previous = 0.0;
            for index in 0..=100 {
                let progress = f64::from(index) / 100.0;
                let value = curve.evaluate(progress).0;
                assert!(value >= previous, "{curve:?} regressed at {progress}");
                previous = value;
            }
        }
    }

    #[test]
    fn spring_starts_from_position_and_velocity() {
        let curve = AnimationCurve::spring(SpringSpec::new(100.0, 20.0));
        let transition = PresentationTransition::new_with_velocity(
            rect(0.0, 0.0, 100.0, 100.0),
            rect(100.0, 0.0, 100.0, 100.0),
            PresentationVelocity::new(20.0, 0.0, 0.0, 0.0),
            AnimationTime::from_nanos(0),
            curve,
        );
        let sample = transition.sample(AnimationTime::from_nanos(0));
        close(sample.rect.x(), 0.0);
        close(sample.velocity.x(), 20.0);
    }

    #[test]
    fn spring_converges_and_reports_velocity() {
        let transition = PresentationTransition::new(
            rect(0.0, 0.0, 100.0, 100.0),
            rect(100.0, 0.0, 100.0, 100.0),
            AnimationTime::from_nanos(0),
            AnimationCurve::spring(SpringSpec::new(120.0, 24.0)),
        );
        let sample = transition.sample(AnimationTime::from_nanos(3_000_000_000));
        assert!(sample.mathematically_settled);
        assert_eq!(sample.rect, rect(100.0, 0.0, 100.0, 100.0));
        close(sample.velocity.x(), 0.0);
    }

    #[test]
    fn sampling_is_absolute_and_repeatable_after_a_long_gap() {
        let transition = PresentationTransition::new(
            rect(0.0, 0.0, 10.0, 10.0),
            rect(10.0, 10.0, 20.0, 20.0),
            AnimationTime::from_nanos(100),
            AnimationCurve::spring(SpringSpec::new(90.0, 12.0)),
        );
        let at = AnimationTime::from_nanos(40_000_000);
        assert_eq!(transition.sample(at), transition.sample(at));
        assert_eq!(
            transition.sample(AnimationTime::from_nanos(u64::MAX)).rect,
            rect(10.0, 10.0, 20.0, 20.0)
        );
    }

    #[test]
    fn retarget_preserves_position_and_velocity() {
        let curve = AnimationCurve::spring(SpringSpec::new(100.0, 16.0));
        let mut animator = PresentationAnimator::enabled();
        animator.start(
            7,
            rect(0.0, 0.0, 100.0, 100.0),
            rect(100.0, 0.0, 100.0, 100.0),
            AnimationTime::from_nanos(0),
            curve,
        );
        let retarget_at = AnimationTime::from_nanos(120_000_000);
        let before = animator.sample(7, retarget_at).expect("active sample");
        animator.retarget(7, rect(0.0, 80.0, 120.0, 90.0), retarget_at, curve);
        let after = animator.sample(7, retarget_at).expect("retargeted sample");
        close(after.rect.x(), before.rect.x());
        close(after.rect.y(), before.rect.y());
        close(after.velocity.x(), before.velocity.x());
        close(after.velocity.y(), before.velocity.y());
    }

    #[test]
    fn easing_retarget_preserves_nonzero_velocity() {
        let curve = AnimationCurve::easing(Duration::from_secs(1), EasingCurve::EaseOut);
        let mut animator = PresentationAnimator::enabled();
        animator.start(
            8,
            rect(0.0, 0.0, 100.0, 100.0),
            rect(100.0, 0.0, 100.0, 100.0),
            AnimationTime::from_nanos(0),
            curve,
        );
        let retarget_at = AnimationTime::from_nanos(300_000_000);
        let before = animator.sample(8, retarget_at).expect("active sample");
        animator.retarget(8, rect(0.0, 80.0, 120.0, 90.0), retarget_at, curve);
        let after = animator.sample(8, retarget_at).expect("retargeted sample");
        close(after.velocity.x(), before.velocity.x());
        close(after.velocity.y(), before.velocity.y());
    }

    #[test]
    fn damage_rounds_outward_for_subpixel_geometry() {
        let damage = presentation_damage(rect(1.1, 2.2, 10.1, 8.1), rect(10.9, 12.8, 10.1, 8.1));
        assert_eq!(damage, PresentationDamageRect::new(1, 2, 20, 19));
    }

    #[test]
    fn disabled_animator_has_no_active_transitions_and_samples_target() {
        let mut animator = PresentationAnimator::disabled();
        animator.retarget(
            1,
            rect(20.0, 30.0, 40.0, 50.0),
            AnimationTime::from_nanos(0),
            AnimationCurve::easing(Duration::from_secs(1), EasingCurve::EaseInOut),
        );
        assert_eq!(animator.active_count(), 0);
        assert_eq!(animator.sample(1, AnimationTime::from_nanos(0)), None);
    }

    #[test]
    fn invalid_geometry_is_rejected_without_nan_or_infinity() {
        assert!(PresentationRect::new(f64::NAN, 0.0, 10.0, 10.0).is_none());
        assert!(PresentationRect::new(0.0, 0.0, f64::INFINITY, 10.0).is_none());
        assert!(PresentationRect::new(0.0, 0.0, 0.0, 10.0).is_none());
    }

    #[test]
    fn group_transform_maps_fractional_geometry_and_inverts_input() {
        let transform = PresentationGroupTransform::new(
            4,
            TransitionId::new(NonZeroU64::new(9).expect("non-zero transition id")),
            rect(10.0, 20.0, 100.0, 80.0),
            rect(12.5, 24.25, 125.0, 100.0),
            false,
        );
        let mapped = transform
            .map_rect(rect(20.0, 30.0, 10.0, 8.0))
            .expect("mapped rect");
        close(mapped.x(), 25.0);
        close(mapped.y(), 36.75);
        close(mapped.width(), 12.5);
        close(mapped.height(), 10.0);
        let canonical = transform
            .inverse_map_point((31.25, 44.25))
            .expect("point inside transformed group");
        close(canonical.0, 25.0);
        close(canonical.1, 36.0);
    }

    #[test]
    fn inverse_mapping_is_unbounded_and_leaves_hit_rejection_to_callers() {
        let transform = PresentationGroupTransform::new(
            4,
            TransitionId::new(NonZeroU64::new(10).expect("non-zero transition id")),
            rect(10.0, 20.0, 100.0, 80.0),
            rect(12.5, 24.25, 125.0, 100.0),
            false,
        );
        let canonical = transform
            .inverse_map_point((0.0, 0.0))
            .expect("finite affine inverse outside the presented client rect");
        close(canonical.0, 0.0);
        close(canonical.1, 0.6);
        assert!(
            transform
                .inverse_map_point_unbounded((f64::NAN, 0.0))
                .is_none()
        );
    }

    #[test]
    fn identity_frame_records_presented_window_geometry_without_transition_state() {
        let sample = PresentationSceneSample::empty(AnimationTime::from_nanos(42));
        let snapshot = PresentationFrameSnapshot::from_sample_with_presented_windows(
            &sample,
            vec![PresentedWindowGeometry::new(
                7,
                rect(100.0, 80.0, 640.0, 480.0),
            )],
        );

        assert!(snapshot.transforms.is_empty());
        assert_eq!(
            snapshot
                .presented_window_geometry(7)
                .expect("identity window projection")
                .presented_rect(),
            rect(100.0, 80.0, 640.0, 480.0)
        );
    }

    #[test]
    fn physical_input_geometry_uses_the_same_affine_as_group_transforms() {
        let canonical = rect(500.0, 300.0, 400.0, 200.0);
        let presented = rect(450.0, 280.0, 320.0, 160.0);
        let group = PresentationGroupTransform::new(
            9,
            TransitionId::new(NonZeroU64::new(11).expect("non-zero transition id")),
            canonical,
            presented,
            false,
        );
        let physical = PresentationGeometryTransform::new(canonical, presented);

        assert_eq!(
            physical.inverse_map_point_unbounded((466.0, 288.0)),
            group.inverse_map_point_unbounded((466.0, 288.0))
        );
    }

    #[test]
    fn sampled_window_metric_counts_scene_samples() {
        let mut animator = PresentationAnimator::enabled();
        animator.start(
            1,
            rect(0.0, 0.0, 10.0, 10.0),
            rect(20.0, 0.0, 10.0, 10.0),
            AnimationTime::from_nanos(0),
            AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
        );
        let _ = animator.sample_scene(
            AnimationTime::from_nanos(5_000_000),
            &[PresentationWindowTarget::new(
                1,
                rect(20.0, 0.0, 10.0, 10.0),
            )],
        );
        assert_eq!(animator.metrics().sampled_windows, 1);
    }

    #[test]
    fn animation_policy_defaults_on_and_accepts_only_on_or_off() {
        assert!(animation_policy_enabled(None));
        assert!(animation_policy_enabled(Some("on")));
        assert!(!animation_policy_enabled(Some("off")));
        assert!(animation_policy_enabled(Some("unexpected")));
    }

    #[test]
    fn settled_transitions_emit_final_frame_sample_until_acknowledged() {
        let mut animator = PresentationAnimator::enabled();
        animator.start(
            1,
            rect(0.0, 0.0, 10.0, 10.0),
            rect(20.0, 0.0, 10.0, 10.0),
            AnimationTime::from_nanos(0),
            AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
        );
        let sample = animator.sample_scene(
            AnimationTime::from_nanos(20_000_000),
            &[PresentationWindowTarget::new(
                1,
                rect(20.0, 0.0, 10.0, 10.0),
            )],
        );
        assert_eq!(sample.windows.len(), 1);
        assert_eq!(sample.transforms.len(), 1);
        assert!(sample.windows[0].mathematically_settled);
        assert_eq!(animator.active_count(), 1);
    }

    #[test]
    fn stale_physical_ack_cannot_retire_a_retargeted_transition() {
        let mut animator = PresentationAnimator::enabled();
        let first = animator
            .start(
                1,
                rect(0.0, 0.0, 10.0, 10.0),
                rect(20.0, 0.0, 10.0, 10.0),
                AnimationTime::from_nanos(0),
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )
            .expect("first transition should be created");
        let second = animator
            .retarget(
                1,
                rect(40.0, 0.0, 10.0, 10.0),
                AnimationTime::from_nanos(5_000_000),
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )
            .expect("retarget should create a new transition identity");
        assert_ne!(first, second);
        assert!(!animator.acknowledge_presented_transition(1, first, rect(20.0, 0.0, 10.0, 10.0),));
        assert_eq!(animator.active_count(), 1);
        assert!(animator.acknowledge_presented_transition(1, second, rect(40.0, 0.0, 10.0, 10.0),));
        assert_eq!(animator.active_count(), 0);
    }
}
