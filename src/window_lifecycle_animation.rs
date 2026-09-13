use crate::compositor::ResolvedEffectScene;
use crate::core::WindowId;
use crate::presentation_animation::{AnimationTime, PresentationRect};
use std::collections::BTreeMap;
use std::num::NonZeroU64;
use std::sync::Arc;

/// Keep the default in the existing short desktop-animation class; the
/// staged geometry supplies the visual richness rather than extra duration.
pub const ASTREA_LAMP_BASE_DURATION_MS: u64 = 280;
/// The shape starts gently and grows toward a compact neck as the Dock gets
/// closer to the moving visual group.
pub const ASTREA_LAMP_INITIAL_SHAPE_FACTOR: f64 = 0.20;
pub const ASTREA_LAMP_MAX_SHAPE_FACTOR: f64 = 0.80;
/// Relative stage weights for the directional bump/stretch/squash timeline.
pub const ASTREA_LAMP_BUMP_WEIGHT: f64 = 0.12;
pub const ASTREA_LAMP_STRETCH_WEIGHT: f64 = 0.70;
pub const ASTREA_LAMP_SQUASH_WEIGHT: f64 = 1.0;
pub const ASTREA_LAMP_NEAR_EDGE_BIAS: f64 = 0.18;
pub const ASTREA_LAMP_NECK_BASE: f64 = 0.20;
pub const ASTREA_LAMP_NECK_RANGE: f64 = 0.80;
/// Keep the Genie opaque until its final endpoint cleanup interval.
pub const ASTREA_LAMP_FINAL_OPACITY_START: f64 = 0.98;
const MAX_LIFECYCLE_RENDER_EVIDENCE_ENTRIES: usize = 65_536;
const MAX_LIFECYCLE_RENDER_FALLBACK_ENTRIES: usize = 65_536;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleVisualSourceKind {
    NoOwnedEffects,
    ResolvedOwnedEffects,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LampDirection {
    Top,
    Right,
    Bottom,
    Left,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LifecycleVisualGroup {
    pub canonical_client_rect: PresentationRect,
    pub canonical_visual_rect: PresentationRect,
    pub presented_source_client_rect: PresentationRect,
    pub presented_source_visual_rect: PresentationRect,
    pub anchor_rect: PresentationRect,
    pub lamp_direction: LampDirection,
    pub shape_factor: f64,
    pub bump_distance: f64,
}

impl LifecycleVisualGroup {
    pub fn from_bounds(
        canonical_client_rect: PresentationRect,
        canonical_visual_rect: PresentationRect,
        presented_source_client_rect: PresentationRect,
        anchor_rect: PresentationRect,
        output_width: u32,
        output_height: u32,
    ) -> Option<Self> {
        if !valid_lamp_rects(
            canonical_client_rect,
            canonical_visual_rect,
            presented_source_client_rect,
        ) || !valid_rect(anchor_rect)
        {
            return None;
        }
        let presented_source_visual_rect = presented_visual_rect(
            canonical_client_rect,
            canonical_visual_rect,
            presented_source_client_rect,
        )?;
        let lamp_direction = infer_lamp_direction(
            presented_source_visual_rect,
            anchor_rect,
            output_width,
            output_height,
        );
        let shape_factor = lamp_shape_factor(
            presented_source_visual_rect,
            anchor_rect,
            lamp_direction,
        );
        let bump_distance = lamp_bump_distance(
            presented_source_visual_rect,
            anchor_rect,
            lamp_direction,
        );
        Some(Self {
            canonical_client_rect,
            canonical_visual_rect,
            presented_source_client_rect,
            presented_source_visual_rect,
            anchor_rect,
            lamp_direction,
            shape_factor,
            bump_distance,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LifecycleVisualSource {
    pub window_id: WindowId,
    pub root_surface_id: u32,
    pub transition_id: LifecycleTransitionId,
    pub kind: LifecycleVisualSourceKind,
    pub effect_scene: Arc<ResolvedEffectScene>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LifecycleTransitionRequest {
    pub window_id: WindowId,
    pub root_surface_id: u32,
    pub visual_group: LifecycleVisualGroup,
    pub direction: LifecycleDirection,
    pub resolved_effect_scene: ResolvedEffectScene,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LampWindowSample {
    pub window_id: WindowId,
    pub root_surface_id: u32,
    pub transition_id: LifecycleTransitionId,
    pub visual_group: LifecycleVisualGroup,
    pub progress: f64,
    pub opacity: f64,
    pub direction: LifecycleDirection,
    pub mathematically_settled: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LifecycleSceneSample {
    pub sampled_at: AnimationTime,
    pub lamps: Vec<LampWindowSample>,
    pub visual_sources: Vec<LifecycleVisualSource>,
}

impl LifecycleSceneSample {
    pub fn visual_source_for_window(&self, window_id: WindowId) -> Option<&LifecycleVisualSource> {
        self.visual_sources
            .iter()
            .find(|source| source.window_id == window_id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LifecycleFrameLamp {
    pub window_id: WindowId,
    pub root_surface_id: u32,
    pub transition_id: LifecycleTransitionId,
    pub visual_group: LifecycleVisualGroup,
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
                visual_group: lamp.visual_group,
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

    pub fn qualified_from_sample(
        sample: &LifecycleSceneSample,
        evidence: &LifecycleRenderEvidence,
    ) -> Self {
        let mut snapshot = Self::from_sample(sample);
        snapshot.lamps.retain(|lamp| {
            evidence.contains(lamp.window_id, lamp.root_surface_id, lamp.transition_id)
        });
        snapshot.refresh_signature();
        snapshot
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LifecycleRenderEvidenceEntry {
    pub window_id: WindowId,
    pub root_surface_id: u32,
    pub transition_id: LifecycleTransitionId,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LifecycleRenderEvidence {
    pub consumed: Vec<LifecycleRenderEvidenceEntry>,
}

impl LifecycleRenderEvidence {
    pub fn from_consumed(entries: impl IntoIterator<Item = LifecycleRenderEvidenceEntry>) -> Self {
        let mut evidence = Self::default();
        for entry in entries {
            evidence.record(entry);
        }
        evidence
    }

    pub fn record(&mut self, entry: LifecycleRenderEvidenceEntry) {
        if !self.consumed.contains(&entry)
            && self.consumed.len() < MAX_LIFECYCLE_RENDER_EVIDENCE_ENTRIES
        {
            self.consumed.push(entry);
        }
    }

    pub fn contains(
        &self,
        window_id: WindowId,
        root_surface_id: u32,
        transition_id: LifecycleTransitionId,
    ) -> bool {
        self.consumed.iter().any(|entry| {
            entry.window_id == window_id
                && entry.root_surface_id == root_surface_id
                && entry.transition_id == transition_id
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LifecycleRenderFallbackReason {
    ResolvedSourceAllocation,
    ResolvedSourceCapture,
    LampProgramUnavailable,
    LifecycleResourceUnavailable,
    MeshBudget,
    NoConsumedRepresentation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LifecycleRenderFallbackEntry {
    pub window_id: WindowId,
    pub root_surface_id: u32,
    pub transition_id: LifecycleTransitionId,
    pub reason: LifecycleRenderFallbackReason,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LifecycleRenderFallbacks {
    pub failed: Vec<LifecycleRenderFallbackEntry>,
}

impl LifecycleRenderFallbacks {
    pub fn record(&mut self, entry: LifecycleRenderFallbackEntry) {
        if !self.failed.iter().any(|existing| {
            existing.window_id == entry.window_id
                && existing.root_surface_id == entry.root_surface_id
                && existing.transition_id == entry.transition_id
        }) && self.failed.len() < MAX_LIFECYCLE_RENDER_FALLBACK_ENTRIES
        {
            self.failed.push(entry);
        }
    }

    pub const fn is_empty(&self) -> bool {
        self.failed.is_empty()
    }
}

/// Returns the finite conservative region that can be affected by a Lamp
/// representation. All three lifecycle rectangles participate because the
/// warp can cover any of them over the transition.
pub fn lamp_footprint(
    visual_group: LifecycleVisualGroup,
) -> Option<PresentationRect> {
    if !valid_visual_group(visual_group) {
        return None;
    }
    let left = visual_group
        .presented_source_visual_rect
        .x()
        .min(visual_group.canonical_visual_rect.x())
        .min(visual_group.anchor_rect.x());
    let top = visual_group
        .presented_source_visual_rect
        .y()
        .min(visual_group.canonical_visual_rect.y())
        .min(visual_group.anchor_rect.y());
    let right = (visual_group.presented_source_visual_rect.x()
        + visual_group.presented_source_visual_rect.width())
        .max(visual_group.canonical_visual_rect.x() + visual_group.canonical_visual_rect.width())
        .max(visual_group.anchor_rect.x() + visual_group.anchor_rect.width());
    let bottom = (visual_group.presented_source_visual_rect.y()
        + visual_group.presented_source_visual_rect.height())
        .max(visual_group.canonical_visual_rect.y() + visual_group.canonical_visual_rect.height())
        .max(visual_group.anchor_rect.y() + visual_group.anchor_rect.height());
    let bump = visual_group.bump_distance.max(0.0);
    let (axis_start, axis_end) = axis_bounds(
        visual_group.presented_source_visual_rect,
        visual_group.lamp_direction,
    );
    let (left, top, right, bottom) = match visual_group.lamp_direction {
        LampDirection::Top | LampDirection::Bottom => (
            left,
            top.min(axis_start - bump),
            right,
            bottom.max(axis_end + bump),
        ),
        LampDirection::Left | LampDirection::Right => (
            left.min(axis_start - bump),
            top,
            right.max(axis_end + bump),
            bottom,
        ),
    };
    PresentationRect::new(left, top, right - left, bottom - top)
}

pub fn lamp_footprint_intersects_output(
    visual_group: LifecycleVisualGroup,
    output_width: u32,
    output_height: u32,
) -> bool {
    let Some(footprint) = lamp_footprint(visual_group) else {
        return false;
    };
    footprint.x() < f64::from(output_width)
        && footprint.y() < f64::from(output_height)
        && footprint.x() + footprint.width() > 0.0
        && footprint.y() + footprint.height() > 0.0
}

fn lifecycle_snapshot_signature(lamps: &[LifecycleFrameLamp]) -> u64 {
    let mut signature = 0xcbf2_9ce4_8422_2325_u64;
    for lamp in lamps {
        for value in [
            lamp.window_id.get(),
            u64::from(lamp.root_surface_id),
            lamp.transition_id.get(),
            lamp.visual_group.canonical_client_rect.x().to_bits(),
            lamp.visual_group.canonical_client_rect.y().to_bits(),
            lamp.visual_group.canonical_client_rect.width().to_bits(),
            lamp.visual_group.canonical_client_rect.height().to_bits(),
            lamp.visual_group.canonical_visual_rect.x().to_bits(),
            lamp.visual_group.canonical_visual_rect.y().to_bits(),
            lamp.visual_group.canonical_visual_rect.width().to_bits(),
            lamp.visual_group.canonical_visual_rect.height().to_bits(),
            lamp.visual_group.presented_source_client_rect.x().to_bits(),
            lamp.visual_group.presented_source_client_rect.y().to_bits(),
            lamp.visual_group.presented_source_client_rect.width().to_bits(),
            lamp.visual_group.presented_source_client_rect.height().to_bits(),
            lamp.visual_group.presented_source_visual_rect.x().to_bits(),
            lamp.visual_group.presented_source_visual_rect.y().to_bits(),
            lamp.visual_group.presented_source_visual_rect.width().to_bits(),
            lamp.visual_group.presented_source_visual_rect.height().to_bits(),
            lamp.visual_group.anchor_rect.x().to_bits(),
            lamp.visual_group.anchor_rect.y().to_bits(),
            lamp.visual_group.anchor_rect.width().to_bits(),
            lamp.visual_group.anchor_rect.height().to_bits(),
            lamp.visual_group.shape_factor.to_bits(),
            lamp.visual_group.bump_distance.to_bits(),
            lamp.progress.to_bits(),
            lamp.opacity.to_bits(),
            u64::from(lamp.mathematically_settled),
            match lamp.direction {
                LifecycleDirection::Minimize => 1,
                LifecycleDirection::Restore => 2,
            },
            match lamp.visual_group.lamp_direction {
                LampDirection::Top => 3,
                LampDirection::Right => 4,
                LampDirection::Bottom => 5,
                LampDirection::Left => 6,
            },
        ] {
            signature ^= value;
            signature = signature.wrapping_mul(0x1000_0000_01b3);
        }
    }
    signature
}

#[derive(Debug, Clone)]
struct LifecycleTransition {
    window_id: WindowId,
    root_surface_id: u32,
    transition_id: LifecycleTransitionId,
    visual_group: LifecycleVisualGroup,
    direction: LifecycleDirection,
    start_progress: f64,
    target_progress: f64,
    started_at: AnimationTime,
    duration_nanos: u64,
    resolved_effect_scene: Arc<ResolvedEffectScene>,
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
                let current = transition_progress(transition, now);
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
            visual_group,
            direction,
            resolved_effect_scene,
        } = request;

        let existing = self.transitions.get(&window_id).cloned();
        let start_progress = existing
            .as_ref()
            .map(|transition| transition_progress(transition, now))
            .unwrap_or_else(|| match direction {
                LifecycleDirection::Minimize => 0.0,
                LifecycleDirection::Restore => 1.0,
            });
        let visual_group = existing
            .as_ref()
            .map(|transition| transition.visual_group)
            .unwrap_or(visual_group);
        if !valid_visual_group(visual_group) {
            return None;
        }
        let resolved_effect_scene = existing
            .as_ref()
            .map(|transition| Arc::clone(&transition.resolved_effect_scene))
            .unwrap_or_else(|| Arc::new(resolved_effect_scene));
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
                visual_group,
                direction,
                start_progress,
                target_progress: direction.target_progress(),
                started_at: now,
                duration_nanos,
                resolved_effect_scene,
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
            .cloned()
            .map(|transition| sample_transition(transition, now))
    }

    pub fn visual_group(&self, window_id: WindowId) -> Option<LifecycleVisualGroup> {
        self.transitions
            .get(&window_id)
            .map(|transition| transition.visual_group)
    }

    pub fn sample_scene(&self, now: AnimationTime) -> LifecycleSceneSample {
        LifecycleSceneSample {
            sampled_at: now,
            lamps: self
                .transitions
                .values()
                .cloned()
                .map(|transition| sample_transition(transition, now))
                .collect(),
            visual_sources: self
                .transitions
                .values()
                .map(|transition| LifecycleVisualSource {
                    window_id: transition.window_id,
                    root_surface_id: transition.root_surface_id,
                    transition_id: transition.transition_id,
                    kind: lifecycle_visual_source_kind(&transition.resolved_effect_scene),
                    effect_scene: Arc::clone(&transition.resolved_effect_scene),
                })
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

    /// Snap one exact transition to its semantic endpoint while retaining
    /// lifecycle ownership until the endpoint is physically presented.
    pub fn snap_to_endpoint(
        &mut self,
        window_id: WindowId,
        transition_id: LifecycleTransitionId,
        now: AnimationTime,
    ) -> bool {
        let Some(transition) = self.transitions.get_mut(&window_id) else {
            return false;
        };
        if transition.transition_id != transition_id {
            return false;
        }
        transition.start_progress = transition.direction.target_progress();
        transition.target_progress = transition.direction.target_progress();
        transition.started_at = now;
        transition.duration_nanos = 0;
        true
    }

    /// Retire one exact transition after the renderer has requested a
    /// recoverable lifecycle fallback. This is not a physical presentation
    /// acknowledgement and therefore does not affect any physical ledger.
    pub fn retire_render_fallback(
        &mut self,
        window_id: WindowId,
        transition_id: LifecycleTransitionId,
    ) -> bool {
        let Some(transition) = self.transitions.get(&window_id) else {
            return false;
        };
        if transition.transition_id != transition_id {
            return false;
        }
        self.transitions.remove(&window_id);
        true
    }

    /// Retire a transition proven unable to change this output. This is a
    /// logical no-visual-change settlement, not physical presentation ACK.
    pub fn settle_no_visual_change(
        &mut self,
        window_id: WindowId,
        transition_id: LifecycleTransitionId,
    ) -> bool {
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
    let progress = transition_progress(&transition, now);
    LampWindowSample {
        window_id: transition.window_id,
        root_surface_id: transition.root_surface_id,
        transition_id: transition.transition_id,
        visual_group: transition.visual_group,
        progress,
        opacity: lamp_opacity(progress),
        direction: transition.direction,
        mathematically_settled: (progress - transition.target_progress).abs() <= f64::EPSILON,
    }
}

fn transition_progress(transition: &LifecycleTransition, now: AnimationTime) -> f64 {
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

fn lifecycle_visual_source_kind(scene: &ResolvedEffectScene) -> LifecycleVisualSourceKind {
    if scene.is_empty() {
        LifecycleVisualSourceKind::NoOwnedEffects
    } else {
        LifecycleVisualSourceKind::ResolvedOwnedEffects
    }
}

fn effective_duration_nanos(speed: f64) -> u64 {
    let speed = if speed.is_finite() {
        speed.clamp(0.5, 2.0)
    } else {
        1.0
    };
    ((ASTREA_LAMP_BASE_DURATION_MS as f64 * 1_000_000.0) / speed).round() as u64
}

pub fn canonical_visual_rect(
    canonical_client_rect: PresentationRect,
    owned_visual_rects: impl IntoIterator<Item = PresentationRect>,
    decoration_rect: Option<PresentationRect>,
) -> Option<PresentationRect> {
    if !valid_rect(canonical_client_rect) {
        return None;
    }
    let mut left = canonical_client_rect.x();
    let mut top = canonical_client_rect.y();
    let mut right = left + canonical_client_rect.width();
    let mut bottom = top + canonical_client_rect.height();
    for rect in owned_visual_rects {
        if !valid_rect(rect) {
            return None;
        }
        left = left.min(rect.x());
        top = top.min(rect.y());
        right = right.max(rect.x() + rect.width());
        bottom = bottom.max(rect.y() + rect.height());
    }
    if let Some(rect) = decoration_rect {
        if !valid_rect(rect) {
            return None;
        }
        left = left.min(rect.x());
        top = top.min(rect.y());
        right = right.max(rect.x() + rect.width());
        bottom = bottom.max(rect.y() + rect.height());
    }
    PresentationRect::new(left, top, right - left, bottom - top)
}

pub fn presented_visual_rect(
    canonical_client_rect: PresentationRect,
    canonical_visual_rect: PresentationRect,
    presented_source_client_rect: PresentationRect,
) -> Option<PresentationRect> {
    if !valid_lamp_rects(
        canonical_client_rect,
        canonical_visual_rect,
        presented_source_client_rect,
    ) {
        return None;
    }
    let scale_x = presented_source_client_rect.width() / canonical_client_rect.width();
    let scale_y = presented_source_client_rect.height() / canonical_client_rect.height();
    PresentationRect::new(
        presented_source_client_rect.x()
            + (canonical_visual_rect.x() - canonical_client_rect.x()) * scale_x,
        presented_source_client_rect.y()
            + (canonical_visual_rect.y() - canonical_client_rect.y()) * scale_y,
        canonical_visual_rect.width() * scale_x,
        canonical_visual_rect.height() * scale_y,
    )
}

fn valid_rect(rect: PresentationRect) -> bool {
    rect.x().is_finite()
        && rect.y().is_finite()
        && rect.width().is_finite()
        && rect.height().is_finite()
        && rect.width() > 0.0
        && rect.height() > 0.0
}

fn valid_visual_group(group: LifecycleVisualGroup) -> bool {
    valid_rect(group.canonical_client_rect)
        && valid_rect(group.canonical_visual_rect)
        && valid_rect(group.presented_source_client_rect)
        && valid_rect(group.presented_source_visual_rect)
        && valid_rect(group.anchor_rect)
        && group.shape_factor.is_finite()
        && group.shape_factor >= 0.0
        && group.bump_distance.is_finite()
        && group.bump_distance >= 0.0
}

fn infer_lamp_direction(
    source_rect: PresentationRect,
    anchor_rect: PresentationRect,
    output_width: u32,
    output_height: u32,
) -> LampDirection {
    let output_width = f64::from(output_width.max(1));
    let output_height = f64::from(output_height.max(1));
    let distances = [
        (anchor_rect.y().abs(), LampDirection::Top),
        (
            (output_height - anchor_rect.y() - anchor_rect.height()).abs(),
            LampDirection::Bottom,
        ),
        (anchor_rect.x().abs(), LampDirection::Left),
        (
            (output_width - anchor_rect.x() - anchor_rect.width()).abs(),
            LampDirection::Right,
        ),
    ];
    let minimum = distances
        .iter()
        .map(|(distance, _)| *distance)
        .fold(f64::INFINITY, f64::min);
    let ties = distances
        .iter()
        .filter(|(distance, _)| (*distance - minimum).abs() <= 1.0)
        .count();
    if ties == 1 && minimum <= output_width.min(output_height) * 0.25 {
        distances
            .iter()
            .find(|(distance, _)| (*distance - minimum).abs() <= 1.0)
            .map(|(_, direction)| *direction)
            .unwrap_or_else(|| fallback_lamp_direction(source_rect, anchor_rect))
    } else {
        fallback_lamp_direction(source_rect, anchor_rect)
    }
}

fn fallback_lamp_direction(
    source_rect: PresentationRect,
    anchor_rect: PresentationRect,
) -> LampDirection {
    let source_center_x = source_rect.x() + source_rect.width() * 0.5;
    let source_center_y = source_rect.y() + source_rect.height() * 0.5;
    let anchor_center_x = anchor_rect.x() + anchor_rect.width() * 0.5;
    let anchor_center_y = anchor_rect.y() + anchor_rect.height() * 0.5;
    let delta_x = anchor_center_x - source_center_x;
    let delta_y = anchor_center_y - source_center_y;
    if delta_y.abs() >= delta_x.abs() {
        if delta_y >= 0.0 {
            LampDirection::Bottom
        } else {
            LampDirection::Top
        }
    } else if delta_x >= 0.0 {
        LampDirection::Right
    } else {
        LampDirection::Left
    }
}

fn axis_bounds(rect: PresentationRect, direction: LampDirection) -> (f64, f64) {
    match direction {
        LampDirection::Top | LampDirection::Bottom => {
            (rect.y(), rect.y() + rect.height())
        }
        LampDirection::Left | LampDirection::Right => {
            (rect.x(), rect.x() + rect.width())
        }
    }
}

fn lamp_shape_factor(
    source_rect: PresentationRect,
    anchor_rect: PresentationRect,
    direction: LampDirection,
) -> f64 {
    let extent = match direction {
        LampDirection::Top | LampDirection::Bottom => source_rect.height(),
        LampDirection::Left | LampDirection::Right => source_rect.width(),
    }
    .max(1.0);
    let (source_start, source_end) = axis_bounds(source_rect, direction);
    let (anchor_start, anchor_end) = axis_bounds(anchor_rect, direction);
    let gap = if source_end < anchor_start {
        anchor_start - source_end
    } else if anchor_end < source_start {
        source_start - anchor_end
    } else {
        0.0
    };
    let closeness = 1.0 / (1.0 + gap / extent);
    (ASTREA_LAMP_INITIAL_SHAPE_FACTOR
        + (ASTREA_LAMP_MAX_SHAPE_FACTOR - ASTREA_LAMP_INITIAL_SHAPE_FACTOR) * closeness)
        .clamp(ASTREA_LAMP_INITIAL_SHAPE_FACTOR, ASTREA_LAMP_MAX_SHAPE_FACTOR)
}

fn lamp_bump_distance(
    source_rect: PresentationRect,
    anchor_rect: PresentationRect,
    direction: LampDirection,
) -> f64 {
    let (source_start, source_end) = axis_bounds(source_rect, direction);
    let (anchor_start, anchor_end) = axis_bounds(anchor_rect, direction);
    (source_end.min(anchor_end) - source_start.max(anchor_start)).max(0.0)
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LampStageChannels {
    pub bump_progress: f64,
    pub stretch_progress: f64,
    pub squash_progress: f64,
}

fn in_out_cubic(value: f64) -> f64 {
    let value = value.clamp(0.0, 1.0);
    if value < 0.5 {
        4.0 * value * value * value
    } else {
        1.0 - (-2.0 * value + 2.0).powi(3) * 0.5
    }
}

fn stage_progress(progress: f64, start: f64, end: f64) -> f64 {
    if end <= start {
        return 0.0;
    }
    in_out_cubic(((progress - start) / (end - start)).clamp(0.0, 1.0))
}

fn stage_fractions(shape_factor: f64, bump_distance: f64) -> (f64, f64, f64) {
    let bump_weight = if bump_distance.is_finite() && bump_distance > f64::EPSILON {
        ASTREA_LAMP_BUMP_WEIGHT
    } else {
        0.0
    };
    let stretch_weight = (ASTREA_LAMP_STRETCH_WEIGHT
        * if shape_factor.is_finite() {
            shape_factor.clamp(ASTREA_LAMP_INITIAL_SHAPE_FACTOR, ASTREA_LAMP_MAX_SHAPE_FACTOR)
        } else {
            ASTREA_LAMP_INITIAL_SHAPE_FACTOR
        })
    .max(f64::EPSILON);
    let total = bump_weight + stretch_weight + ASTREA_LAMP_SQUASH_WEIGHT;
    (
        bump_weight / total,
        stretch_weight / total,
        ASTREA_LAMP_SQUASH_WEIGHT / total,
    )
}

pub fn lamp_stage_channels(
    progress: f64,
    shape_factor: f64,
    bump_distance: f64,
) -> LampStageChannels {
    let progress = normalized_progress(progress);
    let (bump_fraction, stretch_fraction, _) = stage_fractions(shape_factor, bump_distance);
    let bump_end = bump_fraction;
    let stretch_end = bump_fraction + stretch_fraction;
    LampStageChannels {
        bump_progress: if bump_fraction > 0.0 {
            stage_progress(progress, 0.0, bump_end)
        } else {
            0.0
        },
        stretch_progress: stage_progress(progress, bump_end, stretch_end),
        squash_progress: stage_progress(progress, stretch_end, 1.0),
    }
}

fn axis_position(rect: PresentationRect, direction: LampDirection, normalized: f64) -> f64 {
    let normalized = normalized.clamp(0.0, 1.0);
    match direction {
        LampDirection::Top => rect.y() + rect.height() * (1.0 - normalized),
        LampDirection::Bottom => rect.y() + rect.height() * normalized,
        LampDirection::Left => rect.x() + rect.width() * (1.0 - normalized),
        LampDirection::Right => rect.x() + rect.width() * normalized,
    }
}

fn cross_position(rect: PresentationRect, direction: LampDirection, normalized: f64) -> f64 {
    let normalized = normalized.clamp(0.0, 1.0);
    match direction {
        LampDirection::Top | LampDirection::Bottom => rect.x() + rect.width() * normalized,
        LampDirection::Left | LampDirection::Right => rect.y() + rect.height() * normalized,
    }
}

fn movement_extent(rect: PresentationRect, direction: LampDirection) -> f64 {
    match direction {
        LampDirection::Top | LampDirection::Bottom => rect.height(),
        LampDirection::Left | LampDirection::Right => rect.width(),
    }
    .max(1.0)
}

pub fn lamp_warp_point_directional(
    source: PresentationRect,
    anchor: PresentationRect,
    direction: LampDirection,
    shape_factor: f64,
    bump_distance: f64,
    channels: LampStageChannels,
    point: [f64; 2],
) -> [f64; 2] {
    if !valid_lamp_rects(source, source, anchor) || !point.into_iter().all(f64::is_finite) {
        return [0.0, 0.0];
    }
    let u = ((point[0] - source.x()) / source.width()).clamp(0.0, 1.0);
    let v = ((point[1] - source.y()) / source.height()).clamp(0.0, 1.0);
    let movement_normalized = match direction {
        LampDirection::Top => 1.0 - v,
        LampDirection::Right => u,
        LampDirection::Bottom => v,
        LampDirection::Left => 1.0 - u,
    };
    let cross_normalized = match direction {
        LampDirection::Top | LampDirection::Bottom => u,
        LampDirection::Left | LampDirection::Right => v,
    };
    let (bump_fraction, stretch_fraction, _) =
        stage_fractions(shape_factor, bump_distance);
    let base_motion = (bump_fraction * channels.bump_progress
        + stretch_fraction * channels.stretch_progress)
        .clamp(0.0, 1.0);
    let bump_ratio = (bump_distance.max(0.0) / movement_extent(source, direction)).clamp(0.0, 1.0);
    let near_edge_bias = bump_ratio * ASTREA_LAMP_NEAR_EDGE_BIAS * channels.bump_progress;
    let biased_motion = (base_motion
        + (movement_normalized - 0.5) * near_edge_bias * (1.0 - base_motion))
        .clamp(0.0, 1.0);
    let source_axis = axis_position(source, direction, movement_normalized);
    let target_axis = axis_position(anchor, direction, movement_normalized);
    let pre_squash_axis = source_axis + (target_axis - source_axis) * biased_motion;
    let axis = pre_squash_axis
        + (target_axis - pre_squash_axis) * channels.squash_progress.clamp(0.0, 1.0);

    let source_cross = cross_position(source, direction, cross_normalized);
    let target_cross = cross_position(anchor, direction, cross_normalized);
    let source_cross_center = cross_position(source, direction, 0.5);
    let shape_factor = if shape_factor.is_finite() {
        shape_factor.clamp(ASTREA_LAMP_INITIAL_SHAPE_FACTOR, ASTREA_LAMP_MAX_SHAPE_FACTOR)
    } else {
        ASTREA_LAMP_INITIAL_SHAPE_FACTOR
    };
    let neck_scale = (1.0
        - shape_factor
            * channels.stretch_progress.clamp(0.0, 1.0)
            * (ASTREA_LAMP_NECK_BASE + ASTREA_LAMP_NECK_RANGE * movement_normalized))
        .clamp(0.05, 1.0);
    let neck_candidate = source_cross_center + (source_cross - source_cross_center) * neck_scale;
    let stretched_cross = if (target_cross - neck_candidate).abs()
        <= (target_cross - source_cross).abs()
    {
        neck_candidate
    } else {
        source_cross
    };
    let cross_motion = (biased_motion + channels.squash_progress).clamp(0.0, 1.0);
    let cross = stretched_cross + (target_cross - stretched_cross) * cross_motion;

    let warped = match direction {
        LampDirection::Top | LampDirection::Bottom => [cross, axis],
        LampDirection::Left | LampDirection::Right => [axis, cross],
    };
    if warped.into_iter().all(f64::is_finite) {
        warped
    } else {
        [0.0, 0.0]
    }
}

pub fn lamp_warp_point(
    source: PresentationRect,
    anchor: PresentationRect,
    point: [f64; 2],
    progress: f64,
) -> [f64; 2] {
    let progress = normalized_progress(progress);
    if progress <= 0.0 {
        return point;
    }
    if progress >= 1.0 {
        let u = ((point[0] - source.x()) / source.width()).clamp(0.0, 1.0);
        let v = ((point[1] - source.y()) / source.height()).clamp(0.0, 1.0);
        return [anchor.x() + u * anchor.width(), anchor.y() + v * anchor.height()];
    }
    let direction = infer_lamp_direction(source, anchor, 1920, 1080);
    let shape_factor = lamp_shape_factor(source, anchor, direction);
    let bump_distance = lamp_bump_distance(source, anchor, direction);
    lamp_warp_point_directional(
        source,
        anchor,
        direction,
        shape_factor,
        bump_distance,
        lamp_stage_channels(progress, shape_factor, bump_distance),
        point,
    )
}

pub fn lamp_opacity(progress: f64) -> f64 {
    let progress = normalized_progress(progress);
    if progress <= ASTREA_LAMP_FINAL_OPACITY_START {
        return 1.0;
    }
    if progress >= 1.0 {
        return 0.0;
    }
    let normalized =
        (progress - ASTREA_LAMP_FINAL_OPACITY_START) / (1.0 - ASTREA_LAMP_FINAL_OPACITY_START);
    let smooth = normalized * normalized * (3.0 - 2.0 * normalized);
    (1.0 - smooth).clamp(0.0, 1.0)
}

pub fn lamp_warp_visual_point(
    visual_group: LifecycleVisualGroup,
    point: [f64; 2],
    progress: f64,
) -> [f64; 2] {
    let progress = normalized_progress(progress);
    if !valid_visual_group(visual_group) || !point.into_iter().all(f64::is_finite) {
        return [0.0, 0.0];
    }
    let source = visual_group.presented_source_visual_rect;
    if progress <= 0.0 {
        return point;
    }
    if progress >= 1.0 {
        let u = ((point[0] - source.x()) / source.width()).clamp(0.0, 1.0);
        let v = ((point[1] - source.y()) / source.height()).clamp(0.0, 1.0);
        return [
            visual_group.anchor_rect.x() + u * visual_group.anchor_rect.width(),
            visual_group.anchor_rect.y() + v * visual_group.anchor_rect.height(),
        ];
    }
    lamp_warp_point_directional(
        source,
        visual_group.anchor_rect,
        visual_group.lamp_direction,
        visual_group.shape_factor,
        visual_group.bump_distance,
        lamp_stage_channels(
            progress,
            visual_group.shape_factor,
            visual_group.bump_distance,
        ),
        point,
    )
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
        request_with_group(
            window_id,
            root_surface_id,
            LifecycleVisualGroup::from_bounds(
                source_rect,
                source_rect,
                source_rect,
                anchor_rect,
                1920,
                1080,
            )
            .expect("valid visual group"),
            direction,
        )
    }

    fn request_with_group(
        window_id: WindowId,
        root_surface_id: u32,
        visual_group: LifecycleVisualGroup,
        direction: LifecycleDirection,
    ) -> LifecycleTransitionRequest {
        LifecycleTransitionRequest {
            window_id,
            root_surface_id,
            visual_group,
            direction,
            resolved_effect_scene: ResolvedEffectScene::default(),
        }
    }
    use crate::presentation_animation::PresentationRect;

    fn rect(x: f64, y: f64, width: f64, height: f64) -> PresentationRect {
        PresentationRect::new(x, y, width, height).expect("valid rectangle")
    }

    #[test]
    fn lamp_footprint_covers_ssd_above_client_after_visual_group_fix() {
        let client = rect(400.0, 100.0, 800.0, 600.0);
        let ssd_outer = rect(384.0, 60.0, 832.0, 640.0);
        let anchor = rect(900.0, 900.0, 64.0, 64.0);

        let visual = LifecycleVisualGroup::from_bounds(
            client,
            ssd_outer,
            client,
            anchor,
            1920,
            1080,
        )
        .expect("valid visual group");
        let footprint = lamp_footprint(visual).expect("valid footprint");

        assert!(footprint.y() <= ssd_outer.y());
        assert!(footprint.y() + footprint.height() >= ssd_outer.y() + ssd_outer.height());
    }

    #[test]
    fn canonical_visual_group_unions_client_subsurface_and_ssd_outer_bounds() {
        let actual = canonical_visual_rect(
            rect(400.0, 100.0, 800.0, 600.0),
            [rect(360.0, 120.0, 32.0, 760.0)],
            Some(rect(384.0, 60.0, 832.0, 640.0)),
        );
        assert_eq!(actual, Some(rect(360.0, 60.0, 856.0, 820.0)));
    }

    #[test]
    fn canonical_visual_group_does_not_expand_csd_window() {
        let actual = canonical_visual_rect(rect(400.0, 100.0, 800.0, 600.0), [], None);
        assert_eq!(actual, Some(rect(400.0, 100.0, 800.0, 600.0)));
    }

    #[test]
    fn presented_visual_group_reuses_canonical_to_presented_client_affine() {
        let actual = presented_visual_rect(
            rect(400.0, 100.0, 800.0, 600.0),
            rect(360.0, 60.0, 832.0, 640.0),
            rect(200.0, 160.0, 960.0, 720.0),
        );
        assert_eq!(actual, Some(rect(152.0, 112.0, 998.4, 768.0)));
    }

    #[test]
    fn active_transition_ignores_live_visual_mutations_on_reversal() {
        let window = WindowId::from_raw(18).expect("valid window id");
        let first_group = LifecycleVisualGroup::from_bounds(
            rect(400.0, 100.0, 800.0, 600.0),
            rect(384.0, 60.0, 832.0, 640.0),
            rect(400.0, 100.0, 800.0, 600.0),
            rect(900.0, 900.0, 64.0, 64.0),
            1920,
            1080,
        )
        .expect("valid visual group");
        // These changed bounds model a relaid-out SSD/subsurface group and a
        // newly reported Dock anchor while the first transition still owns
        // physical presentation. The animator must ignore both snapshots.
        let changed_group = LifecycleVisualGroup::from_bounds(
            rect(400.0, 100.0, 800.0, 600.0),
            rect(384.0, 20.0, 832.0, 680.0),
            rect(400.0, 100.0, 800.0, 600.0),
            rect(1000.0, 900.0, 64.0, 64.0),
            1920,
            1080,
        )
        .expect("valid visual group");
        let mut animator = WindowLifecycleAnimator::new(true);
        let first = animator
            .start_or_reverse(
                request_with_group(
                    window,
                    18,
                    first_group,
                    LifecycleDirection::Minimize,
                ),
                AnimationTime::from_nanos(0),
                1.0,
            )
            .expect("minimize starts");
        let before = animator
            .sample(window, AnimationTime::from_nanos(100_000_000))
            .expect("sample before reversal");
        let second = animator
            .start_or_reverse(
                request_with_group(window, 18, changed_group, LifecycleDirection::Restore),
                AnimationTime::from_nanos(100_000_000),
                1.0,
            )
            .expect("restore reverses");
        let after = animator
            .sample(window, AnimationTime::from_nanos(100_000_000))
            .expect("sample after reversal");

        assert_ne!(first, second);
        assert_eq!(before.visual_group, after.visual_group);
        assert_eq!(
            before.visual_group.anchor_rect,
            after.visual_group.anchor_rect
        );
    }

    #[test]
    fn settled_new_transition_captures_new_visual_bounds() {
        let window = WindowId::from_raw(19).expect("valid window id");
        let first_group = LifecycleVisualGroup::from_bounds(
            rect(400.0, 100.0, 800.0, 600.0),
            rect(384.0, 60.0, 832.0, 640.0),
            rect(400.0, 100.0, 800.0, 600.0),
            rect(900.0, 900.0, 64.0, 64.0),
            1920,
            1080,
        )
        .expect("valid visual group");
        let second_group = LifecycleVisualGroup::from_bounds(
            rect(400.0, 100.0, 800.0, 600.0),
            rect(384.0, 20.0, 832.0, 680.0),
            rect(400.0, 100.0, 800.0, 600.0),
            rect(900.0, 900.0, 64.0, 64.0),
            1920,
            1080,
        )
        .expect("valid visual group");
        let mut animator = WindowLifecycleAnimator::new(true);
        let first = animator
            .start_or_reverse(
                request_with_group(window, 19, first_group, LifecycleDirection::Minimize),
                AnimationTime::from_nanos(0),
                1.0,
            )
            .expect("minimize starts");
        assert!(animator.snap_to_endpoint(
            window,
            first,
            AnimationTime::from_nanos(280_000_000),
        ));
        assert!(animator.acknowledge(window, first, true));
        animator
            .start_or_reverse(
                request_with_group(window, 19, second_group, LifecycleDirection::Minimize),
                AnimationTime::from_nanos(300_000_000),
                1.0,
            )
            .expect("new transition starts");
        let sample = animator
            .sample(window, AnimationTime::from_nanos(300_000_000))
            .expect("new transition sample");
        assert_eq!(sample.visual_group.canonical_visual_rect.y(), 20.0);
    }

    #[test]
    fn lifecycle_footprint_covers_subsurface_outside_root_and_anchor() {
        let visual_group = LifecycleVisualGroup::from_bounds(
            rect(400.0, 100.0, 800.0, 600.0),
            canonical_visual_rect(
                rect(400.0, 100.0, 800.0, 600.0),
                [rect(360.0, 120.0, 32.0, 760.0)],
                None,
            )
            .expect("valid visual bounds"),
            rect(400.0, 100.0, 800.0, 600.0),
            rect(1200.0, 900.0, 64.0, 64.0),
            1920,
            1080,
        )
        .expect("valid visual group");
        let footprint = lamp_footprint(visual_group).expect("valid footprint");
        assert!(footprint.x() <= 360.0);
        assert!(footprint.y() <= 100.0);
        assert!(footprint.x() + footprint.width() >= 1264.0);
        assert!(footprint.y() + footprint.height() >= 964.0);
    }

    #[test]
    fn lifecycle_footprint_includes_overlap_bump_excursion() {
        let source = rect(400.0, 800.0, 800.0, 200.0);
        let anchor = rect(900.0, 900.0, 64.0, 64.0);
        let visual_group = LifecycleVisualGroup::from_bounds(
            source,
            source,
            source,
            anchor,
            1920,
            1080,
        )
        .expect("valid visual group");
        assert!(visual_group.bump_distance > 0.0);
        let footprint = lamp_footprint(visual_group).expect("valid footprint");
        assert!(footprint.y() < source.y());
        assert!(footprint.y() + footprint.height() > source.y() + source.height());
    }

    #[test]
    fn lifecycle_footprint_intersection_is_finite_at_every_output_edge() {
        let cases = [
            (rect(-90.0, 100.0, 120.0, 120.0), rect(-30.0, 110.0, 20.0, 20.0)),
            (rect(1870.0, 100.0, 120.0, 120.0), rect(1900.0, 110.0, 20.0, 20.0)),
            (rect(100.0, -90.0, 120.0, 120.0), rect(110.0, -30.0, 20.0, 20.0)),
            (rect(100.0, 1070.0, 120.0, 120.0), rect(110.0, 1090.0, 20.0, 20.0)),
        ];
        for (source, anchor) in cases {
            let visual_group = LifecycleVisualGroup::from_bounds(
                source,
                source,
                source,
                anchor,
                1920,
                1080,
            )
            .expect("valid visual group");
            assert!(lamp_footprint(visual_group)
                .expect("finite footprint")
                .x()
                .is_finite());
            assert!(lamp_footprint_intersects_output(visual_group, 1920, 1080));
        }
    }

    #[test]
    fn lamp_opacity_stays_full_until_narrow_endpoint_interval() {
        assert_eq!(lamp_opacity(0.97), 1.0);
        assert!(lamp_opacity(0.99) > 0.0);
        assert_eq!(lamp_opacity(1.0), 0.0);
    }

    #[test]
    fn lamp_stage_channels_are_continuous_and_bump_is_optional() {
        let without_bump = lamp_stage_channels(0.0, 0.2, 0.0);
        assert_eq!(without_bump.bump_progress, 0.0);
        assert_eq!(without_bump.stretch_progress, 0.0);
        assert_eq!(without_bump.squash_progress, 0.0);
        let with_bump = lamp_stage_channels(0.5, 0.6, 32.0);
        assert!(with_bump.bump_progress >= 0.0 && with_bump.bump_progress <= 1.0);
        assert!(with_bump.stretch_progress >= 0.0 && with_bump.stretch_progress <= 1.0);
        assert!(with_bump.squash_progress >= 0.0 && with_bump.squash_progress <= 1.0);
        let before = lamp_stage_channels(0.24, 0.6, 32.0);
        let after = lamp_stage_channels(0.26, 0.6, 32.0);
        assert!((before.bump_progress - after.bump_progress).abs() < 0.2);
        assert!((before.stretch_progress - after.stretch_progress).abs() < 0.2);
        let (bump_fraction, stretch_fraction, _) = stage_fractions(0.6, 32.0);
        for boundary in [bump_fraction, bump_fraction + stretch_fraction] {
            let before = lamp_stage_channels(boundary - 1.0e-6, 0.6, 32.0);
            let after = lamp_stage_channels(boundary + 1.0e-6, 0.6, 32.0);
            assert!((before.bump_progress - after.bump_progress).abs() < 1.0e-3);
            assert!((before.stretch_progress - after.stretch_progress).abs() < 1.0e-3);
            assert!((before.squash_progress - after.squash_progress).abs() < 1.0e-3);
        }
    }

    #[test]
    fn lamp_direction_selection_prefers_each_output_edge() {
        let source = rect(700.0, 400.0, 200.0, 200.0);
        let anchors = [
            (rect(900.0, 12.0, 64.0, 32.0), LampDirection::Top),
            (rect(1840.0, 480.0, 64.0, 64.0), LampDirection::Right),
            (rect(900.0, 1036.0, 64.0, 32.0), LampDirection::Bottom),
            (rect(12.0, 480.0, 64.0, 64.0), LampDirection::Left),
        ];
        for (anchor, expected) in anchors {
            let group = LifecycleVisualGroup::from_bounds(
                source,
                source,
                source,
                anchor,
                1920,
                1080,
            )
            .expect("valid visual group");
            assert_eq!(group.lamp_direction, expected);
        }
    }

    #[test]
    fn directional_warp_has_exact_endpoints_and_finite_intermediates() {
        let source = rect(300.0, 200.0, 640.0, 480.0);
        let anchor = rect(700.0, 900.0, 64.0, 64.0);
        let point = [620.0, 440.0];
        let shape = 0.4;
        let bump = 0.0;
        let channels = lamp_stage_channels(0.5, shape, bump);
        let intermediate = lamp_warp_point_directional(
            source,
            anchor,
            LampDirection::Bottom,
            shape,
            bump,
            channels,
            point,
        );
        assert!(intermediate.into_iter().all(f64::is_finite));
        assert_eq!(lamp_warp_point(source, anchor, point, 0.0), point);
        assert_eq!(
            lamp_warp_point(source, anchor, point, 1.0),
            [anchor.x() + 0.5 * anchor.width(), anchor.y() + 0.5 * anchor.height()]
        );
    }

    #[test]
    fn directional_warp_is_equivalent_under_axis_rotation() {
        let bottom_source = rect(0.0, 0.0, 100.0, 100.0);
        let bottom_anchor = rect(20.0, 150.0, 40.0, 40.0);
        let bottom_point = [30.0, 40.0];
        let rotated_source = rect(0.0, 0.0, 100.0, 100.0);
        let rotated_anchor = rect(150.0, 40.0, 40.0, 40.0);
        let rotated_point = [40.0, 70.0];
        let shape = lamp_shape_factor(bottom_source, bottom_anchor, LampDirection::Bottom);
        let bump = lamp_bump_distance(bottom_source, bottom_anchor, LampDirection::Bottom);
        let channels = lamp_stage_channels(0.55, shape, bump);
        let bottom = lamp_warp_point_directional(
            bottom_source,
            bottom_anchor,
            LampDirection::Bottom,
            shape,
            bump,
            channels,
            bottom_point,
        );
        let rotated = lamp_warp_point_directional(
            rotated_source,
            rotated_anchor,
            LampDirection::Right,
            shape,
            bump,
            channels,
            rotated_point,
        );
        assert!((rotated[0] - bottom[1]).abs() < 1e-9);
        assert!((rotated[1] - (100.0 - bottom[0])).abs() < 1e-9);
    }

    #[test]
    fn directional_warp_handles_tiny_and_huge_geometry_without_nonfinite_values() {
        for (source, anchor, point) in [
            (rect(0.0, 0.0, 0.001, 0.001), rect(0.002, 0.002, 0.001, 0.001), [0.0, 0.0]),
            (
                rect(-1.0e6, -1.0e6, 2.0e6, 2.0e6),
                rect(1.0e6, 1.0e6, 1.0, 1.0),
                [0.0, 0.0],
            ),
        ] {
            let group = LifecycleVisualGroup::from_bounds(
                source,
                source,
                source,
                anchor,
                1920,
                1080,
            )
            .expect("valid extreme visual group");
            let warped = lamp_warp_visual_point(group, point, 0.5);
            assert!(warped.into_iter().all(f64::is_finite));
        }
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
    fn policy_endpoint_snap_preserves_exact_transition_ownership() {
        let window = WindowId::from_raw(10).expect("valid window id");
        let source = rect(20.0, 20.0, 400.0, 300.0);
        let anchor = rect(900.0, 700.0, 48.0, 48.0);
        let mut animator = WindowLifecycleAnimator::new(true);
        let transition = animator
            .start_or_reverse(
                request(window, 10, source, anchor, LifecycleDirection::Minimize),
                AnimationTime::from_nanos(0),
                1.0,
            )
            .expect("minimize starts");

        assert!(animator.snap_to_endpoint(
            window,
            transition,
            AnimationTime::from_nanos(100_000_000),
        ));
        let endpoint = animator
            .sample(window, AnimationTime::from_nanos(100_000_000))
            .expect("snapped transition remains active");
        assert_eq!(endpoint.transition_id, transition);
        assert_eq!(endpoint.progress, 1.0);
        assert_eq!(endpoint.visual_group.presented_source_client_rect, source);
        assert_eq!(endpoint.visual_group.anchor_rect, anchor);
        assert!(endpoint.mathematically_settled);
        assert_eq!(animator.active_count(), 1);
        assert!(animator.acknowledge(window, transition, true));
    }

    #[test]
    fn render_evidence_qualifies_only_consumed_transition_identities() {
        let first_window = WindowId::from_raw(15).expect("valid window id");
        let second_window = WindowId::from_raw(16).expect("valid window id");
        let source = rect(0.0, 0.0, 100.0, 100.0);
        let second_source = rect(1000.0, 1000.0, 100.0, 100.0);
        let anchor = rect(200.0, 200.0, 10.0, 10.0);
        let second_anchor = rect(1200.0, 1200.0, 10.0, 10.0);
        let mut animator = WindowLifecycleAnimator::new(true);
        let first = animator
            .start_or_reverse(
                request(
                    first_window,
                    15,
                    source,
                    anchor,
                    LifecycleDirection::Minimize,
                ),
                AnimationTime::from_nanos(0),
                1.0,
            )
            .expect("first transition starts");
        let second = animator
            .start_or_reverse(
                request(
                    second_window,
                    16,
                    second_source,
                    second_anchor,
                    LifecycleDirection::Minimize,
                ),
                AnimationTime::from_nanos(0),
                1.0,
            )
            .expect("second transition starts");
        assert!(lamp_footprint_intersects_output(
            LifecycleVisualGroup::from_bounds(source, source, source, anchor, 800, 600)
                .expect("valid visual group"),
            800,
            600,
        ));
        assert!(!lamp_footprint_intersects_output(
            LifecycleVisualGroup::from_bounds(
                second_source,
                second_source,
                second_source,
                second_anchor,
                800,
                600,
            )
            .expect("valid visual group"),
            800,
            600,
        ));
        let sample = animator.sample_scene(AnimationTime::from_nanos(100_000_000));
        let evidence = LifecycleRenderEvidence::from_consumed([LifecycleRenderEvidenceEntry {
            window_id: first_window,
            root_surface_id: 15,
            transition_id: first,
        }]);
        let qualified = LifecycleFrameSnapshot::qualified_from_sample(&sample, &evidence);
        assert_eq!(qualified.lamps.len(), 1);
        assert_eq!(qualified.lamps[0].window_id, first_window);
        assert_eq!(qualified.lamps[0].transition_id, first);
        assert_ne!(first, second);
    }

    #[test]
    fn lamp_footprint_intersection_is_conservative_and_output_aware() {
        let source = rect(300.0, 300.0, 20.0, 20.0);
        let full = rect(310.0, 310.0, 30.0, 30.0);
        let anchor = rect(330.0, 330.0, 10.0, 10.0);
        let visual_group = LifecycleVisualGroup::from_bounds(
            source, full, source, anchor, 400, 400,
        )
        .expect("valid visual group");
        assert!(!lamp_footprint_intersects_output(
            visual_group,
            100,
            100,
        ));
        assert!(lamp_footprint_intersects_output(
            visual_group,
            400,
            400,
        ));
    }

    #[test]
    fn no_visual_change_settlement_is_separate_from_physical_ack() {
        let window = WindowId::from_raw(17).expect("valid window id");
        let mut animator = WindowLifecycleAnimator::new(true);
        let transition = animator
            .start_or_reverse(
                request(
                    window,
                    17,
                    rect(300.0, 300.0, 100.0, 100.0),
                    rect(500.0, 500.0, 10.0, 10.0),
                    LifecycleDirection::Minimize,
                ),
                AnimationTime::from_nanos(0),
                1.0,
            )
            .expect("transition starts");
        assert!(animator.settle_no_visual_change(window, transition));
        assert_eq!(animator.active_count(), 0);
        assert!(!animator.acknowledge(window, transition, true));
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
                .visual_group
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
