use crate::compositor::{PresentationRetainedVisualPayloadId, ResolvedEffectScene};
use crate::core::WindowId;
use crate::presentation_animation::{
    AnimationTime, PresentationRect, PresentationRetainedVisualIdentity,
};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Keep the default in the existing short desktop-animation class; spatial
/// deformation supplies the visual richness rather than extra duration.
pub const ASTREA_LAMP_BASE_DURATION_MS: u64 = 280;
/// The frozen shape factor starts gently and grows toward a compact funnel as
/// the Dock gets closer to the moving visual group.
pub const ASTREA_LAMP_INITIAL_SHAPE_FACTOR: f64 = 0.20;
pub const ASTREA_LAMP_MAX_SHAPE_FACTOR: f64 = 0.80;
/// The contraction channel completes in globally eased-time space.
pub const ASTREA_LAMP_CONTRACTION_END: f64 = 0.42;
/// Translation begins while contraction is still active.
pub const ASTREA_LAMP_TRANSLATION_START: f64 = 0.15;
/// Fraction of normalized translation time spent in the quadratic soft start.
pub const ASTREA_LAMP_TRANSLATION_BLEND: f64 = 0.25;
/// Overlap retreat completes in raw lifecycle-progress space.
pub const ASTREA_LAMP_RETREAT_END: f64 = 0.30;
/// Spatial delay power applied to rows away from the destination edge.
pub const ASTREA_LAMP_STRETCH_POWER: f64 = 2.0;
/// Absorption depth measured from the Dock-facing anchor edge toward its far
/// edge. This is a spatial starting point, not an Apple implementation value.
pub const ASTREA_LAMP_ABSORB_DEPTH: f64 = 0.60;
/// Keep the sink non-degenerate while the terminal opacity interval hides it.
pub const ASTREA_LAMP_SINK_THICKNESS: f64 = 1.0;
/// Cubic rail control ordinates for the least pronounced frozen shape.
pub const ASTREA_LAMP_RAIL_C1_LOW_SHAPE: f64 = 0.14;
pub const ASTREA_LAMP_RAIL_C2_LOW_SHAPE: f64 = 0.55;
/// Cubic rail control ordinates for the most pronounced frozen shape.
pub const ASTREA_LAMP_RAIL_C1_HIGH_SHAPE: f64 = 0.04;
pub const ASTREA_LAMP_RAIL_C2_HIGH_SHAPE: f64 = 0.24;
/// Keep the Genie opaque until its final endpoint cleanup interval.
pub const ASTREA_LAMP_FINAL_OPACITY_START: f64 = 0.98;
const MAX_LIFECYCLE_RENDER_EVIDENCE_ENTRIES: usize = 65_536;
const MAX_LIFECYCLE_RENDER_FALLBACK_ENTRIES: usize = 65_536;

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
    pub portal_rect: PresentationRect,
    pub sink_rect: PresentationRect,
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
        let portal_rect =
            lamp_portal_rect(presented_source_visual_rect, anchor_rect, lamp_direction)?;
        let sink_rect = lamp_sink_rect(
            anchor_rect,
            portal_rect,
            lamp_direction,
            ASTREA_LAMP_ABSORB_DEPTH,
            ASTREA_LAMP_SINK_THICKNESS,
        )?;
        let shape_factor =
            lamp_shape_factor(presented_source_visual_rect, anchor_rect, lamp_direction);
        let bump_distance =
            lamp_bump_distance(presented_source_visual_rect, anchor_rect, lamp_direction);
        Some(Self {
            canonical_client_rect,
            canonical_visual_rect,
            presented_source_client_rect,
            presented_source_visual_rect,
            anchor_rect,
            portal_rect,
            sink_rect,
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
    pub presentation_identity: PresentationRetainedVisualIdentity,
    pub payload_id: PresentationRetainedVisualPayloadId,
    pub kind: LifecycleVisualSourceKind,
    pub effect_scene: Arc<ResolvedEffectScene>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LifecycleMotionRequest {
    pub presentation_identity: PresentationRetainedVisualIdentity,
    pub direction: LifecycleDirection,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LifecycleMotionSample {
    pub presentation_identity: PresentationRetainedVisualIdentity,
    pub progress: f64,
    pub opacity: f64,
    pub direction: LifecycleDirection,
    pub mathematically_settled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LampWindowSample {
    pub window_id: WindowId,
    pub root_surface_id: u32,
    pub presentation_identity: PresentationRetainedVisualIdentity,
    pub payload_id: PresentationRetainedVisualPayloadId,
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

    pub fn visual_source_for_identity(
        &self,
        presentation_identity: PresentationRetainedVisualIdentity,
    ) -> Option<&LifecycleVisualSource> {
        self.visual_sources
            .iter()
            .find(|source| source.presentation_identity == presentation_identity)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LifecycleFrameLamp {
    pub window_id: WindowId,
    pub root_surface_id: u32,
    pub presentation_identity: PresentationRetainedVisualIdentity,
    pub payload_id: PresentationRetainedVisualPayloadId,
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
                presentation_identity: lamp.presentation_identity,
                payload_id: lamp.payload_id,
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
            evidence.contains(
                lamp.presentation_identity,
                lamp.payload_id,
                lamp.root_surface_id,
            )
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
    pub presentation_identity: PresentationRetainedVisualIdentity,
    pub payload_id: PresentationRetainedVisualPayloadId,
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
        presentation_identity: PresentationRetainedVisualIdentity,
        payload_id: PresentationRetainedVisualPayloadId,
        root_surface_id: u32,
    ) -> bool {
        self.consumed.iter().any(|entry| {
            entry.presentation_identity == presentation_identity
                && entry.payload_id == payload_id
                && entry.root_surface_id == root_surface_id
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
    pub presentation_identity: PresentationRetainedVisualIdentity,
    pub payload_id: PresentationRetainedVisualPayloadId,
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
                && existing.presentation_identity == entry.presentation_identity
                && existing.payload_id == entry.payload_id
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
pub fn lamp_footprint(visual_group: LifecycleVisualGroup) -> Option<PresentationRect> {
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
            lamp.presentation_identity.scene_node_id().get(),
            match lamp.presentation_identity.kind() {
                crate::presentation_animation::PresentationRetainedVisualKind::WindowLifecycle => 1,
            },
            lamp.presentation_identity.transaction_id().get(),
            lamp.presentation_identity.revision_id().get(),
            lamp.payload_id.get(),
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
            lamp.visual_group
                .presented_source_client_rect
                .width()
                .to_bits(),
            lamp.visual_group
                .presented_source_client_rect
                .height()
                .to_bits(),
            lamp.visual_group.presented_source_visual_rect.x().to_bits(),
            lamp.visual_group.presented_source_visual_rect.y().to_bits(),
            lamp.visual_group
                .presented_source_visual_rect
                .width()
                .to_bits(),
            lamp.visual_group
                .presented_source_visual_rect
                .height()
                .to_bits(),
            lamp.visual_group.anchor_rect.x().to_bits(),
            lamp.visual_group.anchor_rect.y().to_bits(),
            lamp.visual_group.anchor_rect.width().to_bits(),
            lamp.visual_group.anchor_rect.height().to_bits(),
            lamp.visual_group.portal_rect.x().to_bits(),
            lamp.visual_group.portal_rect.y().to_bits(),
            lamp.visual_group.portal_rect.width().to_bits(),
            lamp.visual_group.portal_rect.height().to_bits(),
            lamp.visual_group.sink_rect.x().to_bits(),
            lamp.visual_group.sink_rect.y().to_bits(),
            lamp.visual_group.sink_rect.width().to_bits(),
            lamp.visual_group.sink_rect.height().to_bits(),
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

#[derive(Debug, Clone, Copy)]
struct LifecycleMotionState {
    presentation_identity: PresentationRetainedVisualIdentity,
    direction: LifecycleDirection,
    start_progress: f64,
    target_progress: f64,
    started_at: AnimationTime,
    duration_nanos: u64,
}

#[derive(Debug)]
pub struct WindowLifecycleAnimator {
    enabled: bool,
    transitions: BTreeMap<PresentationRetainedVisualIdentity, LifecycleMotionState>,
    #[cfg(test)]
    fail_next_start: bool,
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
            transitions: BTreeMap::new(),
            #[cfg(test)]
            fail_next_start: false,
        }
    }

    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_enabled(
        &mut self,
        enabled: bool,
        active_identities: &[PresentationRetainedVisualIdentity],
        now: AnimationTime,
    ) {
        if self.enabled && !enabled {
            for identity in active_identities {
                if let Some(transition) = self.transitions.get_mut(identity) {
                    let current = transition_progress(transition, now);
                    transition.start_progress = current;
                    transition.target_progress = transition.direction.target_progress();
                    transition.started_at = now;
                    transition.duration_nanos = 0;
                }
            }
        }
        self.enabled = enabled;
    }

    pub fn start_or_reverse(
        &mut self,
        request: LifecycleMotionRequest,
        previous_identity: Option<PresentationRetainedVisualIdentity>,
        now: AnimationTime,
        speed: f64,
    ) -> Option<PresentationRetainedVisualIdentity> {
        #[cfg(test)]
        if std::mem::take(&mut self.fail_next_start) {
            return None;
        }
        let LifecycleMotionRequest {
            presentation_identity,
            direction,
        } = request;
        if presentation_identity.kind()
            != crate::presentation_animation::PresentationRetainedVisualKind::WindowLifecycle
            || self.transitions.contains_key(&presentation_identity)
        {
            return None;
        }
        let previous = if let Some(previous_identity) = previous_identity {
            if previous_identity.scene_node_id() != presentation_identity.scene_node_id()
                || previous_identity.kind() != presentation_identity.kind()
            {
                return None;
            }
            Some(*self.transitions.get(&previous_identity)?)
        } else {
            None
        };
        let start_progress = previous
            .as_ref()
            .map(|transition| transition_progress(transition, now))
            .unwrap_or_else(|| match direction {
                LifecycleDirection::Minimize => 0.0,
                LifecycleDirection::Restore => 1.0,
            });
        let base_duration_nanos = effective_duration_nanos(speed);
        let remaining = (direction.target_progress() - start_progress).abs();
        let duration_nanos = (base_duration_nanos as f64 * remaining).round() as u64;
        let transition = LifecycleMotionState {
            presentation_identity,
            direction,
            start_progress,
            target_progress: direction.target_progress(),
            started_at: now,
            duration_nanos,
        };
        if let Some(previous_identity) = previous_identity {
            self.transitions.remove(&previous_identity);
        }
        self.transitions.insert(presentation_identity, transition);
        Some(presentation_identity)
    }

    pub fn cancel(
        &mut self,
        presentation_identity: PresentationRetainedVisualIdentity,
    ) -> Option<PresentationRetainedVisualIdentity> {
        self.transitions
            .remove(&presentation_identity)
            .map(|transition| transition.presentation_identity)
    }

    /// Clear executor entries for an explicitly torn-down logical owner.
    /// This is cache cleanup only; it does not identify the active owner.
    pub(crate) fn cancel_scene_executions(
        &mut self,
        scene_node_id: crate::core::SceneNodeId,
    ) -> Vec<PresentationRetainedVisualIdentity> {
        let identities = self
            .transitions
            .keys()
            .filter(|identity| identity.scene_node_id() == scene_node_id)
            .copied()
            .collect::<Vec<_>>();
        for identity in &identities {
            self.transitions.remove(identity);
        }
        identities
    }

    pub fn cancel_all(&mut self) -> Vec<PresentationRetainedVisualIdentity> {
        let identities = self.transitions.keys().copied().collect();
        self.transitions.clear();
        identities
    }

    pub fn sample(
        &self,
        presentation_identity: PresentationRetainedVisualIdentity,
        now: AnimationTime,
    ) -> Option<LifecycleMotionSample> {
        self.transitions
            .get(&presentation_identity)
            .map(|transition| sample_transition(transition, now))
    }

    pub fn acknowledge(
        &mut self,
        presentation_identity: PresentationRetainedVisualIdentity,
        mathematically_settled: bool,
    ) -> Option<PresentationRetainedVisualIdentity> {
        if !mathematically_settled {
            return None;
        }
        self.transitions
            .remove(&presentation_identity)
            .map(|transition| transition.presentation_identity)
    }

    /// Snap one exact transition to its semantic endpoint while retaining
    /// lifecycle ownership until the endpoint is physically presented.
    pub fn snap_to_endpoint(
        &mut self,
        presentation_identity: PresentationRetainedVisualIdentity,
        now: AnimationTime,
    ) -> bool {
        let Some(transition) = self.transitions.get_mut(&presentation_identity) else {
            return false;
        };
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
        presentation_identity: PresentationRetainedVisualIdentity,
    ) -> Option<PresentationRetainedVisualIdentity> {
        self.transitions
            .remove(&presentation_identity)
            .map(|transition| transition.presentation_identity)
    }

    /// Retire a transition proven unable to change this output. This is a
    /// logical no-visual-change settlement, not physical presentation ACK.
    pub fn settle_no_visual_change(
        &mut self,
        presentation_identity: PresentationRetainedVisualIdentity,
    ) -> Option<PresentationRetainedVisualIdentity> {
        self.transitions
            .remove(&presentation_identity)
            .map(|transition| transition.presentation_identity)
    }

    pub fn active_count(&self) -> usize {
        self.transitions.len()
    }

    #[cfg(test)]
    pub(crate) fn fail_next_start_for_test(&mut self) {
        self.fail_next_start = true;
    }
}

fn sample_transition(
    transition: &LifecycleMotionState,
    now: AnimationTime,
) -> LifecycleMotionSample {
    let progress = transition_progress(transition, now);
    LifecycleMotionSample {
        presentation_identity: transition.presentation_identity,
        progress,
        opacity: lamp_opacity(progress),
        direction: transition.direction,
        mathematically_settled: (progress - transition.target_progress).abs() <= f64::EPSILON,
    }
}

fn transition_progress(transition: &LifecycleMotionState, now: AnimationTime) -> f64 {
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

/// Fit the complete visual group inside the Dock anchor while preserving its
/// aspect ratio. The edge facing the Dock is flush with the corresponding
/// edge of the anchor; the perpendicular axis is centered.
pub fn lamp_portal_rect(
    source_rect: PresentationRect,
    anchor_rect: PresentationRect,
    direction: LampDirection,
) -> Option<PresentationRect> {
    if !valid_rect(source_rect) || !valid_rect(anchor_rect) {
        return None;
    }
    let scale = (anchor_rect.width() / source_rect.width())
        .min(anchor_rect.height() / source_rect.height());
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let width = source_rect.width() * scale;
    let height = source_rect.height() * scale;
    let cross_x = anchor_rect.x() + (anchor_rect.width() - width) * 0.5;
    let cross_y = anchor_rect.y() + (anchor_rect.height() - height) * 0.5;
    let (x, y) = match direction {
        LampDirection::Bottom => (cross_x, anchor_rect.y()),
        LampDirection::Top => (cross_x, anchor_rect.y() + anchor_rect.height() - height),
        LampDirection::Right => (anchor_rect.x(), cross_y),
        LampDirection::Left => (anchor_rect.x() + anchor_rect.width() - width, cross_y),
    };
    let portal = PresentationRect::new(x, y, width, height)?;
    (valid_rect(portal) && rect_contains_rect(anchor_rect, portal)).then_some(portal)
}

/// Place a thin terminal sink at a fixed depth inside the Dock anchor.
///
/// The portal supplies the aspect-preserved aperture on the cross axis. The
/// sink's main-axis center is measured from the edge facing the Dock, so its
/// depth is independent of the source aspect ratio. All directions use the
/// same axis construction with only the near-edge sign rotated.
pub fn lamp_sink_rect(
    anchor_rect: PresentationRect,
    portal_rect: PresentationRect,
    direction: LampDirection,
    absorb_depth: f64,
    sink_thickness: f64,
) -> Option<PresentationRect> {
    if !valid_rect(anchor_rect)
        || !valid_rect(portal_rect)
        || !rect_contains_rect(anchor_rect, portal_rect)
        || !absorb_depth.is_finite()
        || !sink_thickness.is_finite()
    {
        return None;
    }
    let absorb_depth = absorb_depth.clamp(0.0, 1.0);
    let (anchor_start, anchor_end) = axis_bounds(anchor_rect, direction);
    let axis_extent = anchor_end - anchor_start;
    let thickness = sink_thickness.max(f64::MIN_POSITIVE).min(axis_extent);
    let mut axis_start = match direction {
        LampDirection::Bottom | LampDirection::Right => {
            anchor_start + axis_extent * absorb_depth - thickness * 0.5
        }
        LampDirection::Top | LampDirection::Left => {
            anchor_end - axis_extent * absorb_depth - thickness * 0.5
        }
    };
    axis_start = axis_start.clamp(anchor_start, anchor_end - thickness);
    let sink = match direction {
        LampDirection::Top | LampDirection::Bottom => {
            PresentationRect::new(portal_rect.x(), axis_start, portal_rect.width(), thickness)?
        }
        LampDirection::Left | LampDirection::Right => {
            PresentationRect::new(axis_start, portal_rect.y(), thickness, portal_rect.height())?
        }
    };
    (valid_rect(sink) && rect_contains_rect(anchor_rect, sink)).then_some(sink)
}

fn rect_contains_rect(outer: PresentationRect, inner: PresentationRect) -> bool {
    inner.x() >= outer.x()
        && inner.y() >= outer.y()
        && inner.x() + inner.width() <= outer.x() + outer.width()
        && inner.y() + inner.height() <= outer.y() + outer.height()
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
        && valid_rect(group.portal_rect)
        && valid_rect(group.sink_rect)
        && rect_contains_rect(group.anchor_rect, group.portal_rect)
        && rect_contains_rect(group.anchor_rect, group.sink_rect)
        && group.shape_factor.is_finite()
        && group.shape_factor >= 0.0
        && group.bump_distance.is_finite()
        && group.bump_distance >= 0.0
}

pub(crate) fn valid_lifecycle_visual_group(group: LifecycleVisualGroup) -> bool {
    valid_visual_group(group)
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
        LampDirection::Top | LampDirection::Bottom => (rect.y(), rect.y() + rect.height()),
        LampDirection::Left | LampDirection::Right => (rect.x(), rect.x() + rect.width()),
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
        .clamp(
            ASTREA_LAMP_INITIAL_SHAPE_FACTOR,
            ASTREA_LAMP_MAX_SHAPE_FACTOR,
        )
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

/// The single global Lamp temporal curve; all other channels derive from it.
fn in_out_cubic(value: f64) -> f64 {
    let value = value.clamp(0.0, 1.0);
    if value < 0.5 {
        4.0 * value * value * value
    } else {
        1.0 - (-2.0 * value + 2.0).powi(3) * 0.5
    }
}

fn smoothstep01(value: f64) -> f64 {
    let value = if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    };
    value * value * (3.0 - 2.0 * value)
}

/// Blend a short zero-slope quadratic into a linear translation ramp.
///
/// For normalized `t` and blend width `b`, the unnormalized function is
/// `t^2/(2b)` before the join and `t-b/2` after it. Both branches equal `b/2`
/// and have derivative one at `t=b`. Dividing by `1-b/2` normalizes the final
/// value to one without changing that derivative equality.
fn translation_soft_start(temporal_progress: f64) -> f64 {
    let temporal_progress = if temporal_progress.is_finite() {
        temporal_progress.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let normalized = ((temporal_progress - ASTREA_LAMP_TRANSLATION_START)
        / (1.0 - ASTREA_LAMP_TRANSLATION_START))
        .clamp(0.0, 1.0);
    let blend = ASTREA_LAMP_TRANSLATION_BLEND;
    let raw = if normalized < blend {
        normalized * normalized / (2.0 * blend)
    } else {
        normalized - blend * 0.5
    };
    (raw / (1.0 - blend * 0.5)).clamp(0.0, 1.0)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LampMotionChannels {
    pub temporal_progress: f64,
    pub contraction_progress: f64,
    pub translation_progress: f64,
    pub retreat_progress: f64,
}

pub fn lamp_motion_channels(progress: f64, bump_distance: f64) -> LampMotionChannels {
    let progress = normalized_progress(progress);
    let temporal_progress = in_out_cubic(progress);
    LampMotionChannels {
        temporal_progress,
        contraction_progress: smoothstep01(temporal_progress / ASTREA_LAMP_CONTRACTION_END),
        translation_progress: translation_soft_start(temporal_progress),
        retreat_progress: if bump_distance.is_finite() && bump_distance > f64::EPSILON {
            smoothstep01(progress / ASTREA_LAMP_RETREAT_END)
        } else {
            0.0
        },
    }
}

fn normalized_shape_factor(shape_factor: f64) -> f64 {
    let shape_factor = if shape_factor.is_finite() {
        shape_factor.clamp(
            ASTREA_LAMP_INITIAL_SHAPE_FACTOR,
            ASTREA_LAMP_MAX_SHAPE_FACTOR,
        )
    } else {
        ASTREA_LAMP_INITIAL_SHAPE_FACTOR
    };
    (shape_factor - ASTREA_LAMP_INITIAL_SHAPE_FACTOR)
        / (ASTREA_LAMP_MAX_SHAPE_FACTOR - ASTREA_LAMP_INITIAL_SHAPE_FACTOR)
}

/// Evaluate the monotonic cubic rail that bounds the spatial funnel.
///
/// The control ordinates gather the high-shape rail later and more strongly;
/// they are derived from the frozen v2.2 shape factor rather than settings.
pub fn cubic_funnel_profile(t: f64, shape_factor: f64) -> f64 {
    let t = if t.is_finite() {
        t.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let normalized_shape = normalized_shape_factor(shape_factor);
    let c1 = ASTREA_LAMP_RAIL_C1_LOW_SHAPE
        + normalized_shape * (ASTREA_LAMP_RAIL_C1_HIGH_SHAPE - ASTREA_LAMP_RAIL_C1_LOW_SHAPE);
    let c2 = ASTREA_LAMP_RAIL_C2_LOW_SHAPE
        + normalized_shape * (ASTREA_LAMP_RAIL_C2_HIGH_SHAPE - ASTREA_LAMP_RAIL_C2_LOW_SHAPE);
    let u = 1.0 - t;
    (3.0 * u * u * t * c1 + 3.0 * u * t * t * c2 + t * t * t).clamp(0.0, 1.0)
}

fn retreat_source_axis(source_axis: f64, direction: LampDirection, retreat_distance: f64) -> f64 {
    match direction {
        LampDirection::Top | LampDirection::Left => source_axis + retreat_distance,
        LampDirection::Bottom | LampDirection::Right => source_axis - retreat_distance,
    }
}

fn lamp_row_translation(channels: LampMotionChannels, movement_normalized: f64) -> f64 {
    let movement_normalized = movement_normalized.clamp(0.0, 1.0);
    let contraction_progress = channels.contraction_progress.clamp(0.0, 1.0);
    let stretch = ASTREA_LAMP_STRETCH_POWER * contraction_progress * (1.0 - movement_normalized);
    let translation_progress = channels.translation_progress.clamp(0.0, 1.0);
    if translation_progress >= 1.0 {
        1.0
    } else {
        translation_progress.powf(1.0 + stretch)
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
    channels: LampMotionChannels,
    point: [f64; 2],
) -> [f64; 2] {
    let Some(portal) = lamp_portal_rect(source, anchor, direction) else {
        return [0.0, 0.0];
    };
    let Some(sink) = lamp_sink_rect(
        anchor,
        portal,
        direction,
        ASTREA_LAMP_ABSORB_DEPTH,
        ASTREA_LAMP_SINK_THICKNESS,
    ) else {
        return [0.0, 0.0];
    };
    lamp_warp_point_directional_to_target(
        source,
        sink,
        direction,
        shape_factor,
        bump_distance,
        channels,
        point,
    )
}

fn lamp_warp_point_directional_to_target(
    source: PresentationRect,
    target: PresentationRect,
    direction: LampDirection,
    shape_factor: f64,
    bump_distance: f64,
    channels: LampMotionChannels,
    point: [f64; 2],
) -> [f64; 2] {
    if !valid_rect(source) || !valid_rect(target) || !point.into_iter().all(f64::is_finite) {
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
    let funnel_weight = cubic_funnel_profile(movement_normalized, shape_factor);
    let early_contraction =
        (channels.contraction_progress.clamp(0.0, 1.0) * funnel_weight).clamp(0.0, 1.0);
    let row_translation = lamp_row_translation(channels, movement_normalized);
    let retreat_distance = if bump_distance.is_finite() {
        bump_distance
            .max(0.0)
            .min(movement_extent(source, direction))
            * channels.retreat_progress.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let source_axis = retreat_source_axis(
        axis_position(source, direction, movement_normalized),
        direction,
        retreat_distance,
    );
    let target_axis = axis_position(target, direction, movement_normalized);
    let axis = source_axis + (target_axis - source_axis) * row_translation;

    let source_cross = cross_position(source, direction, cross_normalized);
    let target_cross = cross_position(target, direction, cross_normalized);
    let cross_completion = 1.0 - (1.0 - early_contraction) * (1.0 - row_translation);
    let cross = source_cross + (target_cross - source_cross) * cross_completion.clamp(0.0, 1.0);

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
    let direction = infer_lamp_direction(source, anchor, 1920, 1080);
    let Some(portal) = lamp_portal_rect(source, anchor, direction) else {
        return [0.0, 0.0];
    };
    let Some(sink) = lamp_sink_rect(
        anchor,
        portal,
        direction,
        ASTREA_LAMP_ABSORB_DEPTH,
        ASTREA_LAMP_SINK_THICKNESS,
    ) else {
        return [0.0, 0.0];
    };
    if progress >= 1.0 {
        let u = ((point[0] - source.x()) / source.width()).clamp(0.0, 1.0);
        let v = ((point[1] - source.y()) / source.height()).clamp(0.0, 1.0);
        return [sink.x() + u * sink.width(), sink.y() + v * sink.height()];
    }
    let shape_factor = lamp_shape_factor(source, anchor, direction);
    let bump_distance = lamp_bump_distance(source, anchor, direction);
    lamp_warp_point_directional_to_target(
        source,
        sink,
        direction,
        shape_factor,
        bump_distance,
        lamp_motion_channels(progress, bump_distance),
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
            visual_group.sink_rect.x() + u * visual_group.sink_rect.width(),
            visual_group.sink_rect.y() + v * visual_group.sink_rect.height(),
        ];
    }
    lamp_warp_point_directional_to_target(
        source,
        visual_group.sink_rect,
        visual_group.lamp_direction,
        visual_group.shape_factor,
        visual_group.bump_distance,
        lamp_motion_channels(progress, visual_group.bump_distance),
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
#[path = "window_lifecycle_animation_tests.rs"]
mod tests;
