use crate::core::WindowId;
use crate::presentation_animation::{AnimationTime, PresentationRect};
use std::collections::BTreeMap;
use std::num::NonZeroU64;

pub const ASTREA_LAMP_BASE_DURATION_MS: u64 = 280;
pub const ASTREA_LAMP_PULL: f64 = 2.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LifecycleTransitionId(NonZeroU64);

impl LifecycleTransitionId {
    pub fn new(value: u64) -> Self {
        Self(NonZeroU64::new(value).unwrap_or(NonZeroU64::MIN))
    }

    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleDirection {
    Minimize,
    Restore,
}

impl LifecycleDirection {
    const fn target_progress(self) -> f64 {
        match self {
            Self::Minimize => 1.0,
            Self::Restore => 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LifecycleTransitionRequest {
    pub window_id: WindowId,
    pub root_surface_id: u32,
    pub source_rect: PresentationRect,
    pub full_window_rect: PresentationRect,
    pub anchor_rect: PresentationRect,
    pub direction: LifecycleDirection,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LampWindowSample {
    pub window_id: WindowId,
    pub root_surface_id: u32,
    pub transition_id: LifecycleTransitionId,
    pub source_rect: PresentationRect,
    pub full_window_rect: PresentationRect,
    pub anchor_rect: PresentationRect,
    pub progress: f64,
    pub opacity: f64,
    pub direction: LifecycleDirection,
    pub mathematically_settled: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LifecycleSceneSample {
    pub sampled_at: AnimationTime,
    pub lamps: Vec<LampWindowSample>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LifecycleFrameLamp {
    pub window_id: WindowId,
    pub root_surface_id: u32,
    pub transition_id: LifecycleTransitionId,
    pub source_rect: PresentationRect,
    pub full_window_rect: PresentationRect,
    pub anchor_rect: PresentationRect,
    pub progress: f64,
    pub opacity: f64,
    pub mathematically_settled: bool,
    pub direction: LifecycleDirection,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LifecycleFrameSnapshot {
    pub sampled_at: Option<AnimationTime>,
    pub lamps: Vec<LifecycleFrameLamp>,
    pub signature: u64,
}

impl LifecycleFrameSnapshot {
    pub fn from_sample(sample: &LifecycleSceneSample) -> Self {
        let lamps = sample
            .lamps
            .iter()
            .map(|lamp| LifecycleFrameLamp {
                window_id: lamp.window_id,
                root_surface_id: lamp.root_surface_id,
                transition_id: lamp.transition_id,
                source_rect: lamp.source_rect,
                full_window_rect: lamp.full_window_rect,
                anchor_rect: lamp.anchor_rect,
                progress: lamp.progress,
                opacity: lamp.opacity,
                mathematically_settled: lamp.mathematically_settled,
                direction: lamp.direction,
            })
            .collect::<Vec<_>>();
        let signature = lifecycle_snapshot_signature(&lamps);
        Self {
            sampled_at: Some(sample.sampled_at),
            lamps,
            signature,
        }
    }

    pub const fn is_empty(&self) -> bool {
        self.lamps.is_empty()
    }

    pub fn contains_visible_non_identity(&self) -> bool {
        self.lamps
            .iter()
            .any(|lamp| !lamp.mathematically_settled && lamp.opacity > f64::EPSILON)
    }

    pub fn refresh_signature(&mut self) {
        self.signature = lifecycle_snapshot_signature(&self.lamps);
    }
}

fn lifecycle_snapshot_signature(lamps: &[LifecycleFrameLamp]) -> u64 {
    let mut signature = 0xcbf2_9ce4_8422_2325_u64;
    for lamp in lamps {
        for value in [
            lamp.window_id.get(),
            u64::from(lamp.root_surface_id),
            lamp.transition_id.get(),
            lamp.source_rect.x().to_bits(),
            lamp.source_rect.y().to_bits(),
            lamp.source_rect.width().to_bits(),
            lamp.source_rect.height().to_bits(),
            lamp.full_window_rect.x().to_bits(),
            lamp.full_window_rect.y().to_bits(),
            lamp.full_window_rect.width().to_bits(),
            lamp.full_window_rect.height().to_bits(),
            lamp.anchor_rect.x().to_bits(),
            lamp.anchor_rect.y().to_bits(),
            lamp.anchor_rect.width().to_bits(),
            lamp.anchor_rect.height().to_bits(),
            lamp.progress.to_bits(),
            lamp.opacity.to_bits(),
            u64::from(lamp.mathematically_settled),
            match lamp.direction {
                LifecycleDirection::Minimize => 1,
                LifecycleDirection::Restore => 2,
            },
        ] {
            signature ^= value;
            signature = signature.wrapping_mul(0x1000_0000_01b3);
        }
    }
    signature
}

#[derive(Debug, Clone, Copy)]
struct LifecycleTransition {
    window_id: WindowId,
    root_surface_id: u32,
    transition_id: LifecycleTransitionId,
    source_rect: PresentationRect,
    full_window_rect: PresentationRect,
    anchor_rect: PresentationRect,
    direction: LifecycleDirection,
    start_progress: f64,
    target_progress: f64,
    started_at: AnimationTime,
    duration_nanos: u64,
}

#[derive(Debug)]
pub struct WindowLifecycleAnimator {
    enabled: bool,
    next_transition_id: u64,
    transitions: BTreeMap<WindowId, LifecycleTransition>,
}

impl Default for WindowLifecycleAnimator {
    fn default() -> Self {
        Self::new(true)
    }
}

impl WindowLifecycleAnimator {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            next_transition_id: 1,
            transitions: BTreeMap::new(),
        }
    }

    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_enabled(&mut self, enabled: bool, now: AnimationTime) {
        if self.enabled && !enabled {
            for transition in self.transitions.values_mut() {
                let current = transition_progress(*transition, now);
                transition.start_progress = current;
                transition.target_progress = transition.direction.target_progress();
                transition.started_at = now;
                transition.duration_nanos = 0;
            }
        }
        self.enabled = enabled;
    }

    pub fn start_or_reverse(
        &mut self,
        request: LifecycleTransitionRequest,
        now: AnimationTime,
        speed: f64,
    ) -> Option<LifecycleTransitionId> {
        let LifecycleTransitionRequest {
            window_id,
            root_surface_id,
            source_rect,
            full_window_rect,
            anchor_rect,
            direction,
        } = request;
        if !valid_lamp_rects(source_rect, full_window_rect, anchor_rect) {
            return None;
        }

        let existing = self.transitions.get(&window_id).copied();
        let start_progress = existing
            .map(|transition| transition_progress(transition, now))
            .unwrap_or_else(|| match direction {
                LifecycleDirection::Minimize => 0.0,
                LifecycleDirection::Restore => 1.0,
            });
        let (source_rect, full_window_rect, anchor_rect) =
            existing.map_or((source_rect, full_window_rect, anchor_rect), |transition| {
                (
                    transition.source_rect,
                    transition.full_window_rect,
                    transition.anchor_rect,
                )
            });
        let transition_id = self.allocate_transition_id();
        let base_duration_nanos = effective_duration_nanos(speed);
        let remaining = (direction.target_progress() - start_progress).abs();
        let duration_nanos = (base_duration_nanos as f64 * remaining).round() as u64;
        self.transitions.insert(
            window_id,
            LifecycleTransition {
                window_id,
                root_surface_id,
                transition_id,
                source_rect,
                full_window_rect,
                anchor_rect,
                direction,
                start_progress,
                target_progress: direction.target_progress(),
                started_at: now,
                duration_nanos,
            },
        );
        Some(transition_id)
    }

    pub fn cancel(&mut self, window_id: WindowId) -> bool {
        self.transitions.remove(&window_id).is_some()
    }

    pub fn cancel_all(&mut self) {
        self.transitions.clear();
    }

    pub fn sample(&self, window_id: WindowId, now: AnimationTime) -> Option<LampWindowSample> {
        self.transitions
            .get(&window_id)
            .copied()
            .map(|transition| sample_transition(transition, now))
    }

    pub fn sample_scene(&self, now: AnimationTime) -> LifecycleSceneSample {
        LifecycleSceneSample {
            sampled_at: now,
            lamps: self
                .transitions
                .values()
                .copied()
                .map(|transition| sample_transition(transition, now))
                .collect(),
        }
    }

    pub fn acknowledge(
        &mut self,
        window_id: WindowId,
        transition_id: LifecycleTransitionId,
        mathematically_settled: bool,
    ) -> bool {
        if !mathematically_settled {
            return false;
        }
        let Some(transition) = self.transitions.get(&window_id) else {
            return false;
        };
        if transition.transition_id != transition_id {
            return false;
        }
        self.transitions.remove(&window_id);
        true
    }

    pub fn has_pending_visible(&self) -> bool {
        !self.transitions.is_empty()
    }

    pub fn active_count(&self) -> usize {
        self.transitions.len()
    }

    fn allocate_transition_id(&mut self) -> LifecycleTransitionId {
        let value = self.next_transition_id.max(1);
        self.next_transition_id = value.wrapping_add(1).max(1);
        LifecycleTransitionId::new(value)
    }
}

fn sample_transition(transition: LifecycleTransition, now: AnimationTime) -> LampWindowSample {
    let progress = transition_progress(transition, now);
    LampWindowSample {
        window_id: transition.window_id,
        root_surface_id: transition.root_surface_id,
        transition_id: transition.transition_id,
        source_rect: transition.source_rect,
        full_window_rect: transition.full_window_rect,
        anchor_rect: transition.anchor_rect,
        progress,
        opacity: lamp_opacity(progress),
        direction: transition.direction,
        mathematically_settled: (progress - transition.target_progress).abs() <= f64::EPSILON,
    }
}

fn transition_progress(transition: LifecycleTransition, now: AnimationTime) -> f64 {
    if transition.duration_nanos == 0 {
        return transition.target_progress;
    }
    let elapsed = now
        .as_nanos()
        .saturating_sub(transition.started_at.as_nanos());
    let timeline = (elapsed as f64 / transition.duration_nanos as f64).clamp(0.0, 1.0);
    (transition.start_progress
        + (transition.target_progress - transition.start_progress) * timeline)
        .clamp(0.0, 1.0)
}

fn effective_duration_nanos(speed: f64) -> u64 {
    let speed = if speed.is_finite() {
        speed.clamp(0.5, 2.0)
    } else {
        1.0
    };
    ((ASTREA_LAMP_BASE_DURATION_MS as f64 * 1_000_000.0) / speed).round() as u64
}

fn valid_lamp_rects(
    source: PresentationRect,
    full_window: PresentationRect,
    anchor: PresentationRect,
) -> bool {
    [source, full_window, anchor].iter().all(|rect| {
        rect.x().is_finite()
            && rect.y().is_finite()
            && rect.width().is_finite()
            && rect.height().is_finite()
            && rect.width() > 0.0
            && rect.height() > 0.0
    })
}

pub fn lamp_pull(progress: f64, longitudinal: f64) -> f64 {
    let progress = normalized_progress(progress);
    let longitudinal = if longitudinal.is_finite() {
        longitudinal.clamp(0.0, 1.0)
    } else {
        0.5
    };
    if progress <= 0.0 {
        return 0.0;
    }
    if progress >= 1.0 {
        return 1.0;
    }
    (1.0 - (1.0 - progress).powf(1.0 + ASTREA_LAMP_PULL * longitudinal)).clamp(0.0, 1.0)
}

pub fn lamp_warp_point(
    source: PresentationRect,
    anchor: PresentationRect,
    point: [f64; 2],
    progress: f64,
) -> [f64; 2] {
    if !valid_lamp_rects(source, source, anchor) || !point.into_iter().all(f64::is_finite) {
        return [0.0, 0.0];
    }
    let progress = normalized_progress(progress);
    if progress <= 0.0 {
        return point;
    }

    let source_center = [
        source.x() + source.width() * 0.5,
        source.y() + source.height() * 0.5,
    ];
    let anchor_center = [
        anchor.x() + anchor.width() * 0.5,
        anchor.y() + anchor.height() * 0.5,
    ];
    let direction = [
        anchor_center[0] - source_center[0],
        anchor_center[1] - source_center[1],
    ];
    let distance = direction[0].hypot(direction[1]);
    let longitudinal = if distance <= 1e-9 {
        0.5
    } else {
        let normal = [direction[0] / distance, direction[1] / distance];
        let corners = [
            [source.x(), source.y()],
            [source.x() + source.width(), source.y()],
            [source.x(), source.y() + source.height()],
            [source.x() + source.width(), source.y() + source.height()],
        ];
        let projection = |corner: [f64; 2]| corner[0] * normal[0] + corner[1] * normal[1];
        let mut min_projection = f64::INFINITY;
        let mut max_projection = f64::NEG_INFINITY;
        for corner in corners {
            let value = projection(corner);
            min_projection = min_projection.min(value);
            max_projection = max_projection.max(value);
        }
        let range = max_projection - min_projection;
        if range <= 1e-9 {
            0.5
        } else {
            (projection(point) - min_projection) / range
        }
    };
    let pull = lamp_pull(progress, longitudinal);
    let u = ((point[0] - source.x()) / source.width()).clamp(0.0, 1.0);
    let v = ((point[1] - source.y()) / source.height()).clamp(0.0, 1.0);
    let target = [
        anchor.x() + u * anchor.width(),
        anchor.y() + v * anchor.height(),
    ];
    [
        point[0] + (target[0] - point[0]) * pull,
        point[1] + (target[1] - point[1]) * pull,
    ]
}

pub fn lamp_opacity(progress: f64) -> f64 {
    let progress = normalized_progress(progress);
    if progress <= 0.9 {
        return 1.0;
    }
    if progress >= 1.0 {
        return 0.0;
    }
    let normalized = (progress - 0.9) / 0.1;
    let smooth = normalized * normalized * (3.0 - 2.0 * normalized);
    (1.0 - smooth).clamp(0.0, 1.0)
}

fn normalized_progress(progress: f64) -> f64 {
    if progress.is_finite() {
        progress.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

pub fn lamp_warp_window_point(
    full_window: PresentationRect,
    source: PresentationRect,
    anchor: PresentationRect,
    point: [f64; 2],
    progress: f64,
) -> [f64; 2] {
    if !valid_lamp_rects(full_window, source, anchor) || !point.into_iter().all(f64::is_finite) {
        return [0.0, 0.0];
    }
    let u = ((point[0] - full_window.x()) / full_window.width()).clamp(0.0, 1.0);
    let v = ((point[1] - full_window.y()) / full_window.height()).clamp(0.0, 1.0);
    let source_point = [
        source.x() + u * source.width(),
        source.y() + v * source.height(),
    ];
    lamp_warp_point(source, anchor, source_point, progress)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(
        window_id: WindowId,
        root_surface_id: u32,
        source_rect: PresentationRect,
        anchor_rect: PresentationRect,
        direction: LifecycleDirection,
    ) -> LifecycleTransitionRequest {
        LifecycleTransitionRequest {
            window_id,
            root_surface_id,
            source_rect,
            full_window_rect: source_rect,
            anchor_rect,
            direction,
        }
    }
    use crate::presentation_animation::PresentationRect;

    fn rect(x: f64, y: f64, width: f64, height: f64) -> PresentationRect {
        PresentationRect::new(x, y, width, height).expect("valid rectangle")
    }

    #[test]
    fn lamp_is_identity_at_zero_and_reaches_anchor_at_one() {
        let source = rect(100.0, 80.0, 800.0, 600.0);
        let anchor = rect(1200.0, 900.0, 64.0, 64.0);
        let point = [500.0, 320.0];

        assert_eq!(lamp_warp_point(source, anchor, point, 0.0), point);
        let warped = lamp_warp_point(source, anchor, point, 1.0);
        let expected = [
            anchor.x() + 0.5 * anchor.width(),
            anchor.y() + 0.4 * anchor.height(),
        ];
        assert_eq!(warped, expected);
        assert_eq!(lamp_opacity(0.0), 1.0);
        assert_eq!(lamp_opacity(1.0), 0.0);
    }

    #[test]
    fn full_window_reference_freezes_the_presented_affine_basis() {
        let full = rect(100.0, 100.0, 800.0, 600.0);
        let source = rect(200.0, 160.0, 960.0, 720.0);
        let anchor = rect(1200.0, 900.0, 64.0, 64.0);
        let point = [500.0, 400.0];
        assert_eq!(
            lamp_warp_window_point(full, source, anchor, point, 0.0),
            [680.0, 520.0]
        );
        assert_eq!(
            lamp_warp_window_point(full, source, anchor, point, 1.0),
            [1232.0, 932.0]
        );
    }

    #[test]
    fn lamp_is_finite_and_direction_independent_for_anchor_positions() {
        let source = rect(300.0, 200.0, 640.0, 480.0);
        let points = [[300.0, 200.0], [620.0, 440.0], [940.0, 680.0]];
        let anchors = [
            rect(500.0, 800.0, 64.0, 64.0),
            rect(-120.0, 300.0, 64.0, 64.0),
            rect(1100.0, 300.0, 64.0, 64.0),
            rect(500.0, -80.0, 64.0, 64.0),
            rect(1100.0, 800.0, 64.0, 64.0),
        ];

        for anchor in anchors {
            for point in points {
                let warped = lamp_warp_point(source, anchor, point, 0.5);
                assert!(warped.into_iter().all(f64::is_finite));
            }
        }
    }

    #[test]
    fn lamp_progress_and_opacity_are_monotonic() {
        let source = rect(0.0, 0.0, 1000.0, 700.0);
        let anchor = rect(1200.0, 800.0, 48.0, 48.0);
        let point = [1000.0, 700.0];
        let mut previous_distance = f64::INFINITY;
        let mut previous_opacity = 1.0;

        for step in 0..=100 {
            let t = f64::from(step) / 100.0;
            let warped = lamp_warp_point(source, anchor, point, t);
            let target = [anchor.x() + anchor.width(), anchor.y() + anchor.height()];
            let distance = (target[0] - warped[0]).hypot(target[1] - warped[1]);
            assert!(distance <= previous_distance + 1e-9);
            let opacity = lamp_opacity(t);
            assert!(opacity <= previous_opacity + 1e-9);
            previous_distance = distance;
            previous_opacity = opacity;
        }
    }

    #[test]
    fn lifecycle_transition_ids_are_fresh_for_reversal() {
        let first = LifecycleTransitionId::new(1);
        let second = LifecycleTransitionId::new(2);
        assert_ne!(first, second);
        assert_ne!(first.get(), second.get());
    }

    #[test]
    fn reversal_preserves_progress_and_rejects_stale_ack() {
        let window = WindowId::from_raw(7).expect("valid window id");
        let source = rect(0.0, 0.0, 800.0, 600.0);
        let anchor = rect(1000.0, 700.0, 64.0, 64.0);
        let mut animator = WindowLifecycleAnimator::new(true);
        let first = animator
            .start_or_reverse(
                request(window, 7, source, anchor, LifecycleDirection::Minimize),
                AnimationTime::from_nanos(0),
                1.0,
            )
            .expect("minimize starts");
        let before = animator
            .sample(window, AnimationTime::from_nanos(103_600_000))
            .expect("active sample");
        let second = animator
            .start_or_reverse(
                request(window, 7, source, anchor, LifecycleDirection::Restore),
                AnimationTime::from_nanos(103_600_000),
                1.0,
            )
            .expect("restore reverses");
        let after = animator
            .sample(window, AnimationTime::from_nanos(103_600_000))
            .expect("reversed sample");

        assert!((before.progress - 0.37).abs() < 1e-9);
        assert_eq!(before.progress, after.progress);
        assert_ne!(first, second);
        assert!(!animator.acknowledge(window, first, true));
        assert!(
            animator
                .sample(window, AnimationTime::from_nanos(103_600_000))
                .is_some()
        );
    }

    #[test]
    fn exact_endpoint_stays_owned_until_matching_ack() {
        let window = WindowId::from_raw(9).expect("valid window id");
        let source = rect(20.0, 20.0, 400.0, 300.0);
        let anchor = rect(900.0, 700.0, 48.0, 48.0);
        let mut animator = WindowLifecycleAnimator::new(true);
        let transition = animator
            .start_or_reverse(
                request(window, 9, source, anchor, LifecycleDirection::Minimize),
                AnimationTime::from_nanos(0),
                1.0,
            )
            .expect("minimize starts");
        let endpoint = animator.sample_scene(AnimationTime::from_nanos(280_000_000));
        assert!(endpoint.lamps[0].mathematically_settled);
        assert_eq!(animator.active_count(), 1);
        assert!(animator.acknowledge(window, transition, true));
        assert_eq!(animator.active_count(), 0);
    }

    #[test]
    fn reversal_keeps_the_original_anchor_frozen() {
        let window = WindowId::from_raw(11).expect("valid window id");
        let source = rect(20.0, 20.0, 400.0, 300.0);
        let first_anchor = rect(900.0, 700.0, 48.0, 48.0);
        let second_anchor = rect(-80.0, 400.0, 64.0, 64.0);
        let mut animator = WindowLifecycleAnimator::new(true);
        animator.start_or_reverse(
            request(
                window,
                11,
                source,
                first_anchor,
                LifecycleDirection::Minimize,
            ),
            AnimationTime::from_nanos(0),
            1.0,
        );
        let _ = animator.start_or_reverse(
            request(
                window,
                11,
                source,
                second_anchor,
                LifecycleDirection::Restore,
            ),
            AnimationTime::from_nanos(100_000_000),
            1.0,
        );
        assert_eq!(
            animator
                .sample(window, AnimationTime::from_nanos(100_000_000))
                .expect("reversed sample")
                .anchor_rect,
            first_anchor
        );
    }

    #[test]
    fn speed_scales_the_linear_timeline_and_disable_snaps_to_target() {
        let window = WindowId::from_raw(13).expect("valid window id");
        let source = rect(20.0, 20.0, 400.0, 300.0);
        let anchor = rect(900.0, 700.0, 48.0, 48.0);
        let mut animator = WindowLifecycleAnimator::new(true);
        animator
            .start_or_reverse(
                request(window, 13, source, anchor, LifecycleDirection::Minimize),
                AnimationTime::from_nanos(0),
                2.0,
            )
            .expect("minimize starts");
        assert_eq!(
            animator
                .sample(window, AnimationTime::from_nanos(70_000_000))
                .expect("active sample")
                .progress,
            0.5
        );
        animator.set_enabled(false, AnimationTime::from_nanos(70_000_000));
        let endpoint = animator
            .sample(window, AnimationTime::from_nanos(70_000_000))
            .expect("disabled animation retains endpoint");
        assert_eq!(endpoint.progress, 1.0);
        assert!(endpoint.mathematically_settled);
    }
}
