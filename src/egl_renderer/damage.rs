use std::{
    collections::VecDeque,
    env::{self, VarError},
    sync::{Arc, OnceLock},
};

#[cfg(test)]
use std::cell::Cell;

use khronos_egl as egl;
use oblivion_one::{
    compositor::{DesktopVisualState, SurfaceDamageRect, cursor_damage_rect},
    cursor_theme::{CompositorCursorImage, shared_compositor_cursor_image},
};

use super::OutputFramebufferOrigin;
use super::effects::{EffectDebugKawaseMode, effect_debug_config};

pub(crate) const MAX_PARTIAL_REPAINT_RECTS: usize = 8;
pub(crate) const MAX_DAMAGE_HISTORY_FRAMES: usize = 8;
const MAX_EXPLICIT_OUTPUT_BUFFER_AGE: u32 = 3;
const MAX_PARTIAL_REPAINT_PERCENT: u64 = 75;
pub(crate) const DAMAGE_COMPLEXITY_SHADOW_EXTENTS_FACTOR: u64 = 2;
const PARTIAL_REPAINT_COMPLEXITY_POLICY_ENV: &str = "TYPHON_PARTIAL_REPAINT_COMPLEXITY_POLICY";

#[cfg(test)]
thread_local! {
    static DAMAGE_COMPLEXITY_SHADOW_ANALYSIS_COUNT: Cell<u32> = const { Cell::new(0) };
}

/// A half-open rectangle in output physical pixels with a top-left origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OutputRect {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

impl OutputRect {
    pub(crate) const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    fn clipped(self, output_width: u32, output_height: u32) -> Option<Self> {
        let left = i64::from(self.x).clamp(0, i64::from(output_width));
        let top = i64::from(self.y).clamp(0, i64::from(output_height));
        let right = i64::from(self.x)
            .checked_add(i64::from(self.width))?
            .clamp(0, i64::from(output_width));
        let bottom = i64::from(self.y)
            .checked_add(i64::from(self.height))?
            .clamp(0, i64::from(output_height));
        (right > left && bottom > top).then_some(Self {
            x: i32::try_from(left).ok()?,
            y: i32::try_from(top).ok()?,
            width: u32::try_from(right - left).ok()?,
            height: u32::try_from(bottom - top).ok()?,
        })
    }

    const fn pixels(self) -> u64 {
        (self.width as u64).saturating_mul(self.height as u64)
    }

    fn right(self) -> i64 {
        i64::from(self.x).saturating_add(i64::from(self.width))
    }

    fn bottom(self) -> i64 {
        i64::from(self.y).saturating_add(i64::from(self.height))
    }

    fn touches_or_overlaps(self, other: Self) -> bool {
        i64::from(self.x) <= other.right()
            && i64::from(other.x) <= self.right()
            && i64::from(self.y) <= other.bottom()
            && i64::from(other.y) <= self.bottom()
    }

    fn union(self, other: Self) -> Option<Self> {
        let left = self.x.min(other.x);
        let top = self.y.min(other.y);
        let right = self.right().max(other.right());
        let bottom = self.bottom().max(other.bottom());
        Some(Self {
            x: left,
            y: top,
            width: u32::try_from(right.checked_sub(i64::from(left))?).ok()?,
            height: u32::try_from(bottom.checked_sub(i64::from(top))?).ok()?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OutputDamage {
    Empty,
    Full,
    Rects(Vec<OutputRect>),
}

pub(crate) type EglOutputDamage = OutputDamage;

impl OutputDamage {
    pub(crate) fn rects(
        output_width: u32,
        output_height: u32,
        rects: impl IntoIterator<Item = OutputRect>,
    ) -> Self {
        let rects = rects
            .into_iter()
            .filter_map(|rect| rect.clipped(output_width, output_height))
            .collect();
        Self::from_clipped_rects(rects)
    }

    pub(crate) fn from_surface_rects(
        output_width: u32,
        output_height: u32,
        rects: impl IntoIterator<Item = SurfaceDamageRect>,
    ) -> Self {
        Self::rects(
            output_width,
            output_height,
            rects.into_iter().map(|rect| {
                OutputRect::new(
                    i32::try_from(rect.x).unwrap_or(i32::MAX),
                    i32::try_from(rect.y).unwrap_or(i32::MAX),
                    rect.width,
                    rect.height,
                )
            }),
        )
    }

    fn from_clipped_rects(rects: Vec<OutputRect>) -> Self {
        let rects = coalesce_rects(rects);
        if rects.is_empty() {
            Self::Empty
        } else {
            Self::Rects(rects)
        }
    }

    pub(crate) fn union(self, other: Self, output_width: u32, output_height: u32) -> Self {
        match (self, other) {
            (Self::Full, _) | (_, Self::Full) => Self::Full,
            (Self::Empty, damage) | (damage, Self::Empty) => damage,
            (Self::Rects(mut left), Self::Rects(right)) => {
                left.extend(right);
                Self::rects(output_width, output_height, left)
            }
        }
    }

    pub(crate) fn rect_count(&self) -> usize {
        match self {
            Self::Empty => 0,
            Self::Full => 1,
            Self::Rects(rects) => rects.len(),
        }
    }

    pub(crate) fn identity_signature(&self) -> u64 {
        let mut signature = 0xcbf2_9ce4_8422_2325_u64;
        let mut mix = |value: u64| {
            signature ^= value;
            signature = signature.wrapping_mul(0x1000_0000_01b3);
        };
        match self {
            Self::Empty => mix(0),
            Self::Full => mix(1),
            Self::Rects(rects) => {
                mix(2);
                for rect in rects {
                    mix(rect.x as u64);
                    mix(rect.y as u64);
                    mix(u64::from(rect.width));
                    mix(u64::from(rect.height));
                }
            }
        }
        signature
    }

    pub(crate) fn pixels(&self, output_width: u32, output_height: u32) -> Option<u64> {
        match self {
            Self::Empty => Some(0),
            Self::Full => u64::from(output_width).checked_mul(u64::from(output_height)),
            Self::Rects(rects) => rects
                .iter()
                .try_fold(0u64, |total, rect| total.checked_add(rect.pixels())),
        }
    }

    #[cfg(test)]
    pub(crate) fn rects_slice(&self) -> &[OutputRect] {
        match self {
            Self::Rects(rects) => rects,
            Self::Empty | Self::Full => &[],
        }
    }

    pub(crate) fn to_gl_scissors(
        &self,
        output_width: u32,
        output_height: u32,
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> Option<Vec<[i32; 4]>> {
        self.convert_rects(output_width, output_height, framebuffer_origin)
    }

    pub(crate) fn to_egl_rects(
        &self,
        output_width: u32,
        output_height: u32,
    ) -> Option<EglDamageRects> {
        let converted = self.convert_bottom_left_rects(output_width, output_height)?;
        let mut result = EglDamageRects::new();
        for rect in converted {
            result.push(rect);
        }
        (!result.is_empty()).then_some(result)
    }

    fn convert_bottom_left_rects(
        &self,
        output_width: u32,
        output_height: u32,
    ) -> Option<Vec<[i32; 4]>> {
        self.convert_rects(
            output_width,
            output_height,
            OutputFramebufferOrigin::BottomLeft,
        )
    }

    fn convert_rects(
        &self,
        output_width: u32,
        output_height: u32,
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> Option<Vec<[i32; 4]>> {
        let full;
        let rects = match self {
            Self::Empty => return Some(Vec::new()),
            Self::Full => {
                full = [OutputRect::new(0, 0, output_width, output_height)];
                full.as_slice()
            }
            Self::Rects(rects) => rects.as_slice(),
        };
        rects
            .iter()
            .map(|rect| {
                let rect = rect.clipped(output_width, output_height)?;
                let gl_y = match framebuffer_origin {
                    OutputFramebufferOrigin::BottomLeft => {
                        let bottom = output_height.checked_sub(rect.y.try_into().ok()?)?;
                        bottom.checked_sub(rect.height)?
                    }
                    OutputFramebufferOrigin::TopLeftScanout => rect.y.try_into().ok()?,
                };
                Some([
                    rect.x,
                    i32::try_from(gl_y).ok()?,
                    i32::try_from(rect.width).ok()?,
                    i32::try_from(rect.height).ok()?,
                ])
            })
            .collect()
    }
}

pub(crate) fn output_damage_from_effect_region(
    region: &oblivion_one::effects::EffectRegion,
    output_width: u32,
    output_height: u32,
) -> OutputDamage {
    if region.is_empty() {
        return OutputDamage::Empty;
    }
    if region.bounding_rect().is_none() {
        return OutputDamage::Full;
    }
    OutputDamage::rects(
        output_width,
        output_height,
        region
            .rects()
            .iter()
            .map(|rect| OutputRect::new(rect.x, rect.y, rect.width, rect.height)),
    )
}

pub(crate) fn merge_effect_damage(
    current_damage: OutputDamage,
    effect_damage: &oblivion_one::effects::EffectRegion,
    output_width: u32,
    output_height: u32,
) -> OutputDamage {
    current_damage.union(
        output_damage_from_effect_region(effect_damage, output_width, output_height),
        output_width,
        output_height,
    )
}

pub(crate) fn effect_execution_demand_for_repaint_plan(
    graph: &oblivion_one::effects::CompiledFrameGraph,
    plan: &RepaintPlan,
    output_width: u32,
    output_height: u32,
) -> oblivion_one::effects::EffectExecutionDemand {
    let repair_region =
        super::effect_region_from_output_damage(&plan.repair_damage, output_width, output_height);
    let conservative_full = plan.mode == RepaintMode::Full
        || (!repair_region.is_empty() && repair_region.bounding_rect().is_none());
    oblivion_one::effects::plan_effect_execution_demand_with_kawase_mode(
        graph,
        &repair_region,
        conservative_full,
        effect_debug_config().kawase_mode() == EffectDebugKawaseMode::Full,
    )
}

pub(crate) fn resolve_effect_execution_for_repaint_plan(
    planner: &PartialRepaintPlanner,
    graph: &oblivion_one::effects::CompiledFrameGraph,
    plan: &mut RepaintPlan,
    output_width: u32,
    output_height: u32,
) -> oblivion_one::effects::EffectExecutionDemand {
    if plan.mode == RepaintMode::Full {
        return effect_execution_demand_for_repaint_plan(graph, plan, output_width, output_height);
    }

    let max_iterations = graph.instances.len().saturating_add(1).max(1);
    for _ in 0..max_iterations {
        let demand =
            effect_execution_demand_for_repaint_plan(graph, plan, output_width, output_height);
        if demand.is_conservative_full() {
            plan.repair_damage = OutputDamage::Full;
            plan.mode = RepaintMode::Full;
            plan.fallback_reason = Some(FullRepaintReason::EffectExecutionConservative);
            return effect_execution_demand_for_repaint_plan(
                graph,
                plan,
                output_width,
                output_height,
            );
        }

        let previous_repair = plan.repair_damage.clone();
        let execution_repair = merge_effect_damage(
            previous_repair.clone(),
            &demand.execution_region,
            output_width,
            output_height,
        );
        planner.apply_execution_repair(plan, execution_repair);
        if plan.mode == RepaintMode::Full {
            return effect_execution_demand_for_repaint_plan(
                graph,
                plan,
                output_width,
                output_height,
            );
        }
        if plan.repair_damage == previous_repair {
            return demand;
        }
    }

    plan.repair_damage = OutputDamage::Full;
    plan.mode = RepaintMode::Full;
    plan.fallback_reason = Some(FullRepaintReason::EffectExecutionConservative);
    effect_execution_demand_for_repaint_plan(graph, plan, output_width, output_height)
}

fn coalesce_rects(mut rects: Vec<OutputRect>) -> Vec<OutputRect> {
    let mut output = Vec::<OutputRect>::new();
    while let Some(mut pending) = rects.pop() {
        let mut index = 0;
        while index < output.len() {
            let existing = output[index];
            let Some(union) = existing.union(pending) else {
                index += 1;
                continue;
            };
            if existing == pending
                || (existing.touches_or_overlaps(pending)
                    && union.pixels() <= existing.pixels().saturating_add(pending.pixels()))
            {
                pending = union;
                output.swap_remove(index);
                index = 0;
            } else {
                index += 1;
            }
        }
        output.push(pending);
    }
    output.reverse();
    output
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EglPartialRepaintCapabilities {
    /// The acquired target's contents and lineage can be identified reliably.
    pub(crate) buffer_age: bool,
    /// The renderer can repair only the damaged regions of that target.
    pub(crate) partial_render_repair: bool,
    /// The presentation path can submit damage to an EGLSurface swap.
    pub(crate) swap_buffers_with_damage: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BufferAge {
    Unsupported,
    QueryFailed,
    Value(i32),
}

pub(crate) fn software_buffer_age(
    presentation_serial: u64,
    last_presented_serial: Option<u64>,
) -> BufferAge {
    let Some(last_presented_serial) = last_presented_serial else {
        return BufferAge::Value(0);
    };
    let Some(age) = presentation_serial
        .checked_sub(last_presented_serial)
        .and_then(|distance| distance.checked_add(1))
        .and_then(|age| i32::try_from(age).ok())
    else {
        return BufferAge::Value(-1);
    };
    BufferAge::Value(age)
}

pub(crate) fn render_target_buffer_age(
    presentation_serial: u64,
    last_presented_serial: Option<u64>,
) -> BufferAge {
    software_buffer_age(presentation_serial, last_presented_serial)
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RepaintMode {
    Skip,
    Partial,
    #[default]
    Full,
}

impl RepaintMode {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Skip => "skip",
            Self::Partial => "partial",
            Self::Full => "full",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FullRepaintReason {
    CurrentDamageFull,
    FirstFrameOrInvalidated,
    BufferAgeUnsupported,
    PartialRenderRepairUnsupported,
    BufferAgeZero,
    BufferAgeInvalid,
    BufferAgeQueryFailed,
    InsufficientHistory,
    TooManyRectangles,
    DamageAreaThreshold,
    ForcedFull,
    PartialRepaintDisabled,
    EffectExecutionConservative,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PartialRepaintComplexityPolicy {
    #[default]
    Legacy,
    StructuredExperimental,
}

impl PartialRepaintComplexityPolicy {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::StructuredExperimental => "structured-experimental",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PartialRepaintComplexityAction {
    #[default]
    NotApplicable,
    LegacyFull,
    StructuredBoundingBox,
    StructuredManyRectangles,
    StructuredAreaFull,
    StructuredSafetyFull,
}

impl PartialRepaintComplexityAction {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::NotApplicable => "not_applicable",
            Self::LegacyFull => "legacy_full",
            Self::StructuredBoundingBox => "structured_bbox",
            Self::StructuredManyRectangles => "structured_many_rects",
            Self::StructuredAreaFull => "structured_area_full",
            Self::StructuredSafetyFull => "structured_safety_full",
        }
    }
}

impl FullRepaintReason {
    pub(crate) const fn histogram_index(self) -> usize {
        match self {
            Self::CurrentDamageFull => 0,
            Self::FirstFrameOrInvalidated => 1,
            Self::BufferAgeUnsupported => 2,
            Self::PartialRenderRepairUnsupported => 3,
            Self::BufferAgeZero => 4,
            Self::BufferAgeInvalid => 5,
            Self::BufferAgeQueryFailed => 6,
            Self::InsufficientHistory => 7,
            Self::TooManyRectangles => 8,
            Self::DamageAreaThreshold => 9,
            Self::ForcedFull => 10,
            Self::PartialRepaintDisabled => 11,
            Self::EffectExecutionConservative => 12,
        }
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::CurrentDamageFull => "current_damage_full",
            Self::FirstFrameOrInvalidated => "history_invalid",
            Self::BufferAgeUnsupported => "buffer_age_unsupported",
            Self::PartialRenderRepairUnsupported => "partial_render_repair_unsupported",
            Self::BufferAgeZero => "buffer_age_zero",
            Self::BufferAgeInvalid => "buffer_age_invalid",
            Self::BufferAgeQueryFailed => "buffer_age_query_failed",
            Self::InsufficientHistory => "insufficient_history",
            Self::TooManyRectangles => "too_many_rectangles",
            Self::DamageAreaThreshold => "damage_area_threshold",
            Self::ForcedFull => "forced_full",
            Self::PartialRepaintDisabled => "partial_repaint_disabled",
            Self::EffectExecutionConservative => "effect_execution_conservative",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepaintPlan {
    /// Damage that describes the logical scene rendered for this candidate.
    ///
    /// This is intentionally not the damage transition that belongs in a
    /// presentation-domain buffer-age journal. A candidate may be rendered
    /// while an older frame is still presented and may pageflip after another
    /// candidate has become the actual predecessor.
    pub(crate) render_damage: OutputDamage,
    pub(crate) repair_damage: OutputDamage,
    pub(crate) buffer_age: Option<u32>,
    pub(crate) mode: RepaintMode,
    pub(crate) fallback_reason: Option<FullRepaintReason>,
    pub(crate) complexity_policy: PartialRepaintComplexityPolicy,
    pub(crate) complexity_action: PartialRepaintComplexityAction,
}

impl Default for RepaintPlan {
    fn default() -> Self {
        Self {
            render_damage: OutputDamage::Empty,
            repair_damage: OutputDamage::Empty,
            buffer_age: None,
            mode: RepaintMode::Skip,
            fallback_reason: None,
            complexity_policy: PartialRepaintComplexityPolicy::Legacy,
            complexity_action: PartialRepaintComplexityAction::NotApplicable,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RenderExecution {
    Full,
    Scissored {
        scissors: Vec<[i32; 4]>,
        disable_scissor_after: bool,
    },
}

impl RepaintPlan {
    pub(crate) fn render_execution(
        &self,
        output_width: u32,
        output_height: u32,
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> Option<RenderExecution> {
        match self.mode {
            RepaintMode::Skip => None,
            RepaintMode::Full => Some(RenderExecution::Full),
            RepaintMode::Partial => Some(RenderExecution::Scissored {
                scissors: self.repair_damage.to_gl_scissors(
                    output_width,
                    output_height,
                    framebuffer_origin,
                )?,
                disable_scissor_after: true,
            }),
        }
    }

    pub(crate) const fn swap_damage(&self) -> &OutputDamage {
        &self.repair_damage
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PartialRepaintPlanner {
    output_size: (u32, u32),
    history: VecDeque<OutputDamage>,
    history_valid: bool,
    capabilities: EglPartialRepaintCapabilities,
    force_full: bool,
    partial_enabled: bool,
    complexity_policy: PartialRepaintComplexityPolicy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PartialRepaintCandidateDecision {
    KeepOriginal {
        action: PartialRepaintComplexityAction,
    },
    UseBoundingBox {
        bbox: OutputRect,
        action: PartialRepaintComplexityAction,
    },
    Full {
        reason: FullRepaintReason,
        action: PartialRepaintComplexityAction,
    },
}

fn decide_partial_repaint_candidate(
    repair_damage: &OutputDamage,
    output_size: (u32, u32),
    policy: PartialRepaintComplexityPolicy,
) -> PartialRepaintCandidateDecision {
    if policy == PartialRepaintComplexityPolicy::Legacy {
        // Preserve the existing fallback order exactly. In particular, an
        // over-complex candidate wins over unavailable pixel arithmetic.
        if *repair_damage == OutputDamage::Full {
            return PartialRepaintCandidateDecision::Full {
                reason: FullRepaintReason::DamageAreaThreshold,
                action: PartialRepaintComplexityAction::LegacyFull,
            };
        }
        if repair_damage.rect_count() > MAX_PARTIAL_REPAINT_RECTS {
            return PartialRepaintCandidateDecision::Full {
                reason: FullRepaintReason::TooManyRectangles,
                action: PartialRepaintComplexityAction::LegacyFull,
            };
        }
        let Some(repair_pixels) = repair_damage.pixels(output_size.0, output_size.1) else {
            return PartialRepaintCandidateDecision::Full {
                reason: FullRepaintReason::DamageAreaThreshold,
                action: PartialRepaintComplexityAction::LegacyFull,
            };
        };
        let Some(output_pixels) = output_pixel_count(output_size) else {
            return PartialRepaintCandidateDecision::Full {
                reason: FullRepaintReason::DamageAreaThreshold,
                action: PartialRepaintComplexityAction::LegacyFull,
            };
        };
        return if partial_repaint_area_threshold_reached(repair_pixels, output_pixels) {
            PartialRepaintCandidateDecision::Full {
                reason: FullRepaintReason::DamageAreaThreshold,
                action: PartialRepaintComplexityAction::LegacyFull,
            }
        } else {
            PartialRepaintCandidateDecision::KeepOriginal {
                action: PartialRepaintComplexityAction::NotApplicable,
            }
        };
    }

    if *repair_damage == OutputDamage::Full {
        return PartialRepaintCandidateDecision::Full {
            reason: FullRepaintReason::DamageAreaThreshold,
            action: PartialRepaintComplexityAction::StructuredAreaFull,
        };
    }
    let Some(original_pixels) = repair_damage.pixels(output_size.0, output_size.1) else {
        return PartialRepaintCandidateDecision::Full {
            reason: FullRepaintReason::DamageAreaThreshold,
            action: PartialRepaintComplexityAction::StructuredAreaFull,
        };
    };
    let Some(output_pixels) = output_pixel_count(output_size) else {
        return PartialRepaintCandidateDecision::Full {
            reason: FullRepaintReason::DamageAreaThreshold,
            action: PartialRepaintComplexityAction::StructuredAreaFull,
        };
    };
    if partial_repaint_area_threshold_reached(original_pixels, output_pixels) {
        return PartialRepaintCandidateDecision::Full {
            reason: FullRepaintReason::DamageAreaThreshold,
            action: PartialRepaintComplexityAction::StructuredAreaFull,
        };
    }
    if repair_damage.rect_count() <= MAX_PARTIAL_REPAINT_RECTS {
        return PartialRepaintCandidateDecision::KeepOriginal {
            action: PartialRepaintComplexityAction::NotApplicable,
        };
    }
    if repair_damage.rect_count() > oblivion_one::effects::MAX_EFFECT_REGION_RECTS {
        return PartialRepaintCandidateDecision::Full {
            reason: FullRepaintReason::TooManyRectangles,
            action: PartialRepaintComplexityAction::StructuredSafetyFull,
        };
    }

    let OutputDamage::Rects(rects) = repair_damage else {
        return PartialRepaintCandidateDecision::Full {
            reason: FullRepaintReason::DamageAreaThreshold,
            action: PartialRepaintComplexityAction::StructuredAreaFull,
        };
    };
    let Some(first_rect) = rects.first().copied() else {
        return PartialRepaintCandidateDecision::Full {
            reason: FullRepaintReason::DamageAreaThreshold,
            action: PartialRepaintComplexityAction::StructuredAreaFull,
        };
    };
    let Some(bbox) = rects
        .iter()
        .skip(1)
        .try_fold(first_rect, |bbox, rect| bbox.union(*rect))
    else {
        return PartialRepaintCandidateDecision::Full {
            reason: FullRepaintReason::DamageAreaThreshold,
            action: PartialRepaintComplexityAction::StructuredAreaFull,
        };
    };
    let bbox_damage = OutputDamage::Rects(vec![bbox]);
    let Some(bbox_pixels) = bbox_damage.pixels(output_size.0, output_size.1) else {
        return PartialRepaintCandidateDecision::Full {
            reason: FullRepaintReason::DamageAreaThreshold,
            action: PartialRepaintComplexityAction::StructuredAreaFull,
        };
    };
    let Some(max_bbox_pixels) =
        original_pixels.checked_mul(DAMAGE_COMPLEXITY_SHADOW_EXTENTS_FACTOR)
    else {
        return PartialRepaintCandidateDecision::Full {
            reason: FullRepaintReason::DamageAreaThreshold,
            action: PartialRepaintComplexityAction::StructuredAreaFull,
        };
    };
    if bbox_pixels <= max_bbox_pixels
        && !partial_repaint_area_threshold_reached(bbox_pixels, output_pixels)
    {
        PartialRepaintCandidateDecision::UseBoundingBox {
            bbox,
            action: PartialRepaintComplexityAction::StructuredBoundingBox,
        }
    } else {
        PartialRepaintCandidateDecision::KeepOriginal {
            action: PartialRepaintComplexityAction::StructuredManyRectangles,
        }
    }
}

fn output_pixel_count(output_size: (u32, u32)) -> Option<u64> {
    u64::from(output_size.0).checked_mul(u64::from(output_size.1))
}

fn partial_repaint_area_threshold_reached(repair_pixels: u64, output_pixels: u64) -> bool {
    output_pixels == 0
        || repair_pixels.saturating_mul(100)
            >= output_pixels.saturating_mul(MAX_PARTIAL_REPAINT_PERCENT)
}

fn parse_partial_repaint_complexity_policy(
    value: Result<String, VarError>,
) -> (PartialRepaintComplexityPolicy, bool) {
    match value {
        Ok(value) if value == "legacy" => (PartialRepaintComplexityPolicy::Legacy, false),
        Ok(value) if value == "structured-experimental" => (
            PartialRepaintComplexityPolicy::StructuredExperimental,
            false,
        ),
        Err(VarError::NotPresent) => (PartialRepaintComplexityPolicy::Legacy, false),
        Ok(_) | Err(VarError::NotUnicode(_)) => (PartialRepaintComplexityPolicy::Legacy, true),
    }
}

fn configured_partial_repaint_complexity_policy() -> PartialRepaintComplexityPolicy {
    static POLICY: OnceLock<PartialRepaintComplexityPolicy> = OnceLock::new();
    *POLICY.get_or_init(|| {
        let (policy, invalid) = parse_partial_repaint_complexity_policy(env::var(
            PARTIAL_REPAINT_COMPLEXITY_POLICY_ENV,
        ));
        if invalid {
            eprintln!("warning: invalid {PARTIAL_REPAINT_COMPLEXITY_POLICY_ENV}; using legacy");
        }
        policy
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DamageComplexityShadowOutcome {
    NotApplicable,
    PartialBoundingBox,
    PartialManyRectangles,
    FullAreaThreshold,
    Unavailable,
}

impl DamageComplexityShadowOutcome {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::NotApplicable => "not_applicable",
            Self::PartialBoundingBox => "partial_bbox",
            Self::PartialManyRectangles => "partial_many_rects",
            Self::FullAreaThreshold => "full_area_threshold",
            Self::Unavailable => "unavailable",
        }
    }
}

/// Diagnostic-only simulation of the reference damage-complexity policy.
/// This is deliberately separate from `RepaintPlan` and is never used to
/// choose production repaint behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DamageComplexityShadow {
    pub(crate) applicable: bool,
    pub(crate) original_rects: usize,
    pub(crate) original_pixels: u64,
    pub(crate) bbox: Option<OutputRect>,
    pub(crate) bbox_pixels: u64,
    pub(crate) bbox_accepted: bool,
    pub(crate) candidate_rects: usize,
    pub(crate) candidate_pixels: u64,
    pub(crate) added_pixels: u64,
    pub(crate) outcome: DamageComplexityShadowOutcome,
    pub(crate) would_avoid_full: bool,
}

impl DamageComplexityShadow {
    pub(crate) const fn not_applicable() -> Self {
        Self {
            applicable: false,
            original_rects: 0,
            original_pixels: 0,
            bbox: None,
            bbox_pixels: 0,
            bbox_accepted: false,
            candidate_rects: 0,
            candidate_pixels: 0,
            added_pixels: 0,
            outcome: DamageComplexityShadowOutcome::NotApplicable,
            would_avoid_full: false,
        }
    }

    fn unavailable(
        original_rects: usize,
        original_pixels: u64,
        bbox: Option<OutputRect>,
        bbox_pixels: u64,
    ) -> Self {
        Self {
            applicable: true,
            original_rects,
            original_pixels,
            bbox,
            bbox_pixels,
            bbox_accepted: false,
            candidate_rects: 0,
            candidate_pixels: 0,
            added_pixels: 0,
            outcome: DamageComplexityShadowOutcome::Unavailable,
            would_avoid_full: false,
        }
    }

    pub(crate) fn for_candidate(
        repair_damage: &OutputDamage,
        output_size: (u32, u32),
        current_reason: Option<FullRepaintReason>,
    ) -> Self {
        #[cfg(test)]
        DAMAGE_COMPLEXITY_SHADOW_ANALYSIS_COUNT
            .with(|count| count.set(count.get().saturating_add(1)));

        if current_reason != Some(FullRepaintReason::TooManyRectangles) {
            return Self::not_applicable();
        }
        let OutputDamage::Rects(rects) = repair_damage else {
            return Self::not_applicable();
        };
        if rects.len() <= MAX_PARTIAL_REPAINT_RECTS {
            return Self::not_applicable();
        }

        let original_rects = rects.len();
        let original_pixels = repair_damage.pixels(output_size.0, output_size.1);
        let original_pixels_trace = original_pixels.unwrap_or(u64::MAX);
        let Some(first_rect) = rects.first().copied() else {
            return Self::unavailable(original_rects, original_pixels_trace, None, 0);
        };
        let Some(bbox) = rects
            .iter()
            .skip(1)
            .try_fold(first_rect, |bbox, rect| bbox.union(*rect))
        else {
            return Self::unavailable(original_rects, original_pixels_trace, None, 0);
        };
        let bbox_pixels = bbox.pixels();
        let Some(output_pixels) = output_pixel_count(output_size) else {
            return Self::unavailable(
                original_rects,
                original_pixels_trace,
                Some(bbox),
                bbox_pixels,
            );
        };
        let Some(original_pixels) = original_pixels else {
            return Self::unavailable(
                original_rects,
                original_pixels_trace,
                Some(bbox),
                bbox_pixels,
            );
        };
        Self::from_metrics(
            original_rects,
            original_pixels,
            Some(bbox),
            bbox_pixels,
            output_pixels,
        )
    }

    fn from_metrics(
        original_rects: usize,
        original_pixels: u64,
        bbox: Option<OutputRect>,
        bbox_pixels: u64,
        output_pixels: u64,
    ) -> Self {
        let Some(original_extents_limit) =
            original_pixels.checked_mul(DAMAGE_COMPLEXITY_SHADOW_EXTENTS_FACTOR)
        else {
            return Self::unavailable(original_rects, original_pixels, bbox, bbox_pixels);
        };
        let Some(bbox) = bbox else {
            return Self::unavailable(original_rects, original_pixels, None, bbox_pixels);
        };
        let bbox_accepted = bbox_pixels <= original_extents_limit;
        let (candidate_rects, candidate_pixels, added_pixels) = if bbox_accepted {
            (1, bbox_pixels, bbox_pixels.saturating_sub(original_pixels))
        } else {
            (original_rects, original_pixels, 0)
        };
        let outcome = if partial_repaint_area_threshold_reached(candidate_pixels, output_pixels) {
            DamageComplexityShadowOutcome::FullAreaThreshold
        } else if bbox_accepted {
            DamageComplexityShadowOutcome::PartialBoundingBox
        } else {
            DamageComplexityShadowOutcome::PartialManyRectangles
        };
        Self {
            applicable: true,
            original_rects,
            original_pixels,
            bbox: Some(bbox),
            bbox_pixels,
            bbox_accepted,
            candidate_rects,
            candidate_pixels,
            added_pixels,
            outcome,
            would_avoid_full: matches!(
                outcome,
                DamageComplexityShadowOutcome::PartialBoundingBox
                    | DamageComplexityShadowOutcome::PartialManyRectangles
            ),
        }
    }

    #[cfg(test)]
    pub(crate) fn from_metrics_for_test(
        original_rects: usize,
        original_pixels: u64,
        bbox: Option<OutputRect>,
        bbox_pixels: u64,
        output_pixels: u64,
    ) -> Self {
        Self::from_metrics(
            original_rects,
            original_pixels,
            bbox,
            bbox_pixels,
            output_pixels,
        )
    }
}

#[cfg(test)]
mod partial_repaint_complexity_tests {
    use super::*;

    fn rect(x: i32, y: i32, width: u32, height: u32) -> OutputRect {
        OutputRect::new(x, y, width, height)
    }

    fn capabilities() -> EglPartialRepaintCapabilities {
        EglPartialRepaintCapabilities {
            buffer_age: true,
            partial_render_repair: true,
            swap_buffers_with_damage: true,
        }
    }

    fn planner_with_policy(
        output_size: (u32, u32),
        policy: PartialRepaintComplexityPolicy,
    ) -> PartialRepaintPlanner {
        let mut planner =
            PartialRepaintPlanner::new_with_policy(output_size, capabilities(), policy);
        planner.commit_presented_transition(OutputDamage::Empty);
        planner
    }

    fn plan_candidate(
        output_size: (u32, u32),
        policy: PartialRepaintComplexityPolicy,
        candidate: OutputDamage,
    ) -> RepaintPlan {
        planner_with_policy(output_size, policy).plan(candidate, BufferAge::Value(1))
    }

    fn dense_nine_rects_exactly_two_x() -> OutputDamage {
        OutputDamage::rects(
            200,
            100,
            [0, 20, 40, 60, 80, 100, 120, 140, 170]
                .into_iter()
                .map(|x| rect(x, 0, 10, 10)),
        )
    }

    fn nine_rects_bbox_exactly_seventy_five_percent(last_x: i32) -> OutputDamage {
        OutputDamage::rects(
            200,
            100,
            [0, 60, last_x]
                .into_iter()
                .flat_map(|x| [0, 35, 70].into_iter().map(move |y| rect(x, y, 30, 30))),
        )
    }

    #[test]
    fn partial_repaint_complexity_policy_names_are_stable() {
        assert_eq!(PartialRepaintComplexityPolicy::Legacy.as_str(), "legacy");
        assert_eq!(
            PartialRepaintComplexityPolicy::StructuredExperimental.as_str(),
            "structured-experimental"
        );
        assert_eq!(
            PartialRepaintComplexityAction::NotApplicable.as_str(),
            "not_applicable"
        );
        assert_eq!(
            PartialRepaintComplexityAction::LegacyFull.as_str(),
            "legacy_full"
        );
        assert_eq!(
            PartialRepaintComplexityAction::StructuredBoundingBox.as_str(),
            "structured_bbox"
        );
        assert_eq!(
            PartialRepaintComplexityAction::StructuredManyRectangles.as_str(),
            "structured_many_rects"
        );
        assert_eq!(
            PartialRepaintComplexityAction::StructuredAreaFull.as_str(),
            "structured_area_full"
        );
        assert_eq!(
            PartialRepaintComplexityAction::StructuredSafetyFull.as_str(),
            "structured_safety_full"
        );
    }

    #[test]
    fn partial_repaint_complexity_policy_parsing_defaults_invalid_values_to_legacy() {
        use std::env::VarError;

        assert_eq!(
            parse_partial_repaint_complexity_policy(Ok("legacy".to_owned())),
            (PartialRepaintComplexityPolicy::Legacy, false)
        );
        assert_eq!(
            parse_partial_repaint_complexity_policy(Ok("structured-experimental".to_owned())),
            (
                PartialRepaintComplexityPolicy::StructuredExperimental,
                false
            )
        );
        assert_eq!(
            parse_partial_repaint_complexity_policy(Err(VarError::NotPresent)),
            (PartialRepaintComplexityPolicy::Legacy, false)
        );
        assert_eq!(
            parse_partial_repaint_complexity_policy(Ok("structured".to_owned())),
            (PartialRepaintComplexityPolicy::Legacy, true)
        );
        assert_eq!(
            parse_partial_repaint_complexity_policy(Err(VarError::NotUnicode("invalid".into()))),
            (PartialRepaintComplexityPolicy::Legacy, true)
        );
        assert_eq!(
            PartialRepaintPlanner::new((100, 100), capabilities()).complexity_policy(),
            PartialRepaintComplexityPolicy::Legacy
        );
    }

    #[test]
    fn partial_repaint_complexity_legacy_keeps_the_current_fallback_order() {
        let eight_rects = OutputDamage::rects(
            200,
            100,
            (0..MAX_PARTIAL_REPAINT_RECTS).map(|index| rect(index * 20, 0, 10, 10)),
        );
        let eight_plan = plan_candidate(
            (200, 100),
            PartialRepaintComplexityPolicy::Legacy,
            eight_rects.clone(),
        );
        assert_eq!(eight_plan.mode, RepaintMode::Partial);
        assert_eq!(eight_plan.repair_damage, eight_rects);

        let nine_plan = plan_candidate(
            (200, 100),
            PartialRepaintComplexityPolicy::Legacy,
            dense_nine_rects_exactly_two_x(),
        );
        assert_eq!(nine_plan.mode, RepaintMode::Full);
        assert_eq!(
            nine_plan.fallback_reason,
            Some(FullRepaintReason::TooManyRectangles)
        );
        assert_eq!(
            nine_plan.complexity_action,
            PartialRepaintComplexityAction::LegacyFull
        );

        let large_area = plan_candidate(
            (100, 100),
            PartialRepaintComplexityPolicy::Legacy,
            OutputDamage::rects(100, 100, [rect(0, 0, 75, 100)]),
        );
        assert_eq!(large_area.mode, RepaintMode::Full);
        assert_eq!(
            large_area.fallback_reason,
            Some(FullRepaintReason::DamageAreaThreshold)
        );
        assert_eq!(
            large_area.complexity_action,
            PartialRepaintComplexityAction::LegacyFull
        );

        let full_damage = plan_candidate(
            (100, 100),
            PartialRepaintComplexityPolicy::Legacy,
            OutputDamage::Full,
        );
        assert_eq!(full_damage.mode, RepaintMode::Full);
        assert_eq!(
            full_damage.fallback_reason,
            Some(FullRepaintReason::CurrentDamageFull)
        );
        assert_eq!(
            full_damage.complexity_action,
            PartialRepaintComplexityAction::NotApplicable
        );

        let unavailable_pixels = OutputDamage::Rects(vec![
            rect(0, 0, u32::MAX, u32::MAX),
            rect(0, 0, u32::MAX, u32::MAX),
        ]);
        assert_eq!(
            decide_partial_repaint_candidate(
                &unavailable_pixels,
                (u32::MAX, u32::MAX),
                PartialRepaintComplexityPolicy::Legacy,
            ),
            PartialRepaintCandidateDecision::Full {
                reason: FullRepaintReason::DamageAreaThreshold,
                action: PartialRepaintComplexityAction::LegacyFull,
            }
        );
    }

    #[test]
    fn partial_repaint_complexity_structured_falls_back_on_unavailable_pixel_arithmetic() {
        let unavailable_pixels = OutputDamage::Rects(vec![
            rect(0, 0, u32::MAX, u32::MAX),
            rect(0, 0, u32::MAX, u32::MAX),
        ]);

        assert_eq!(
            decide_partial_repaint_candidate(
                &unavailable_pixels,
                (u32::MAX, u32::MAX),
                PartialRepaintComplexityPolicy::StructuredExperimental,
            ),
            PartialRepaintCandidateDecision::Full {
                reason: FullRepaintReason::DamageAreaThreshold,
                action: PartialRepaintComplexityAction::StructuredAreaFull,
            }
        );
    }

    #[test]
    fn partial_repaint_complexity_structured_keeps_simple_regions() {
        let candidate = OutputDamage::rects(
            200,
            100,
            (0..MAX_PARTIAL_REPAINT_RECTS).map(|index| rect(index * 20, 0, 10, 10)),
        );
        let plan = plan_candidate(
            (200, 100),
            PartialRepaintComplexityPolicy::StructuredExperimental,
            candidate.clone(),
        );

        assert_eq!(plan.mode, RepaintMode::Partial);
        assert_eq!(plan.repair_damage, candidate);
        assert_eq!(
            plan.complexity_action,
            PartialRepaintComplexityAction::NotApplicable
        );
    }

    #[test]
    fn partial_repaint_complexity_structured_checks_original_area_before_bbox() {
        let candidate = OutputDamage::rects(
            300,
            300,
            [0, 105, 210]
                .into_iter()
                .flat_map(|x| [0, 105, 210].into_iter().map(move |y| rect(x, y, 90, 90))),
        );
        assert_eq!(candidate.rect_count(), 9);
        let plan = plan_candidate(
            (300, 300),
            PartialRepaintComplexityPolicy::StructuredExperimental,
            candidate,
        );

        assert_eq!(plan.mode, RepaintMode::Full);
        assert_eq!(
            plan.fallback_reason,
            Some(FullRepaintReason::DamageAreaThreshold)
        );
        assert_eq!(
            plan.complexity_action,
            PartialRepaintComplexityAction::StructuredAreaFull
        );
    }

    #[test]
    fn partial_repaint_complexity_structured_uses_a_dense_bbox_at_exactly_two_x() {
        let plan = plan_candidate(
            (200, 100),
            PartialRepaintComplexityPolicy::StructuredExperimental,
            dense_nine_rects_exactly_two_x(),
        );

        assert_eq!(plan.mode, RepaintMode::Partial);
        assert_eq!(
            plan.repair_damage,
            OutputDamage::rects(200, 100, [rect(0, 0, 180, 10)])
        );
        assert_eq!(
            plan.complexity_action,
            PartialRepaintComplexityAction::StructuredBoundingBox
        );
    }

    #[test]
    fn partial_repaint_complexity_structured_preserves_original_when_bbox_crosses_seventy_five_percent()
     {
        let candidate = nine_rects_bbox_exactly_seventy_five_percent(120);
        assert_eq!(candidate.rect_count(), 9);
        assert_eq!(candidate.pixels(200, 100), Some(8_100));
        let plan = plan_candidate(
            (200, 100),
            PartialRepaintComplexityPolicy::StructuredExperimental,
            candidate.clone(),
        );

        assert_eq!(plan.mode, RepaintMode::Partial);
        assert_eq!(plan.repair_damage, candidate);
        assert_eq!(
            plan.complexity_action,
            PartialRepaintComplexityAction::StructuredManyRectangles
        );
    }

    #[test]
    fn partial_repaint_complexity_structured_preserves_original_for_sparse_bbox() {
        let candidate =
            OutputDamage::rects(250, 10, (0..9).map(|index| rect(index * 30, 0, 10, 10)));
        let plan = plan_candidate(
            (250, 10),
            PartialRepaintComplexityPolicy::StructuredExperimental,
            candidate.clone(),
        );

        assert_eq!(plan.mode, RepaintMode::Partial);
        assert_eq!(plan.repair_damage, candidate);
        assert_eq!(
            plan.complexity_action,
            PartialRepaintComplexityAction::StructuredManyRectangles
        );
    }

    #[test]
    fn partial_repaint_complexity_structured_obeys_safety_and_area_boundaries() {
        let at_safety_bound = OutputDamage::rects(
            256,
            100,
            (0..oblivion_one::effects::MAX_EFFECT_REGION_RECTS)
                .map(|index| rect((index * 2) as i32, 0, 1, 1)),
        );
        assert_eq!(at_safety_bound.rect_count(), 128);
        let at_bound = plan_candidate(
            (256, 100),
            PartialRepaintComplexityPolicy::StructuredExperimental,
            at_safety_bound,
        );
        assert_eq!(at_bound.mode, RepaintMode::Partial);

        let above_safety_bound = OutputDamage::rects(
            258,
            100,
            (0..=oblivion_one::effects::MAX_EFFECT_REGION_RECTS)
                .map(|index| rect((index * 2) as i32, 0, 1, 1)),
        );
        assert_eq!(above_safety_bound.rect_count(), 129);
        let over_bound = plan_candidate(
            (258, 100),
            PartialRepaintComplexityPolicy::StructuredExperimental,
            above_safety_bound,
        );
        assert_eq!(over_bound.mode, RepaintMode::Full);
        assert_eq!(
            over_bound.fallback_reason,
            Some(FullRepaintReason::TooManyRectangles)
        );
        assert_eq!(
            over_bound.complexity_action,
            PartialRepaintComplexityAction::StructuredSafetyFull
        );

        let just_below_area = plan_candidate(
            (100, 100),
            PartialRepaintComplexityPolicy::StructuredExperimental,
            OutputDamage::rects(100, 100, [rect(0, 0, 74, 100)]),
        );
        assert_eq!(just_below_area.mode, RepaintMode::Partial);
        assert_eq!(just_below_area.repair_damage.pixels(100, 100), Some(7_400));
        let at_area = plan_candidate(
            (100, 100),
            PartialRepaintComplexityPolicy::StructuredExperimental,
            OutputDamage::rects(100, 100, [rect(0, 0, 75, 100)]),
        );
        assert_eq!(at_area.mode, RepaintMode::Full);
        assert_eq!(
            at_area.complexity_action,
            PartialRepaintComplexityAction::StructuredAreaFull
        );
    }

    #[test]
    fn partial_repaint_complexity_structured_checks_bbox_density_and_strict_area() {
        let just_above_two_x = OutputDamage::rects(
            200,
            100,
            [0, 20, 40, 60, 80, 100, 120, 140, 171]
                .into_iter()
                .map(|x| rect(x, 0, 10, 10)),
        );
        let density_rejected = plan_candidate(
            (200, 100),
            PartialRepaintComplexityPolicy::StructuredExperimental,
            just_above_two_x.clone(),
        );
        assert_eq!(density_rejected.mode, RepaintMode::Partial);
        assert_eq!(density_rejected.repair_damage, just_above_two_x);
        assert_eq!(
            density_rejected.complexity_action,
            PartialRepaintComplexityAction::StructuredManyRectangles
        );

        let bbox_just_below_seventy_five = nine_rects_bbox_exactly_seventy_five_percent(119);
        let accepted = plan_candidate(
            (200, 100),
            PartialRepaintComplexityPolicy::StructuredExperimental,
            bbox_just_below_seventy_five,
        );
        assert_eq!(accepted.mode, RepaintMode::Partial);
        assert_eq!(
            accepted.repair_damage,
            OutputDamage::rects(200, 100, [rect(0, 0, 149, 100)])
        );
        assert_eq!(
            accepted.complexity_action,
            PartialRepaintComplexityAction::StructuredBoundingBox
        );

        let bbox_exactly_seventy_five = nine_rects_bbox_exactly_seventy_five_percent(120);
        assert_eq!(bbox_exactly_seventy_five.pixels(200, 100), Some(8_100));
        let preserved = plan_candidate(
            (200, 100),
            PartialRepaintComplexityPolicy::StructuredExperimental,
            bbox_exactly_seventy_five.clone(),
        );
        assert_eq!(preserved.mode, RepaintMode::Partial);
        assert_eq!(preserved.repair_damage, bbox_exactly_seventy_five);
        assert_eq!(
            preserved.complexity_action,
            PartialRepaintComplexityAction::StructuredManyRectangles
        );
    }
}

impl PartialRepaintPlanner {
    pub(crate) fn new(
        output_size: (u32, u32),
        capabilities: EglPartialRepaintCapabilities,
    ) -> Self {
        Self::new_with_policy(
            output_size,
            capabilities,
            PartialRepaintComplexityPolicy::Legacy,
        )
    }

    pub(crate) fn new_configured(
        output_size: (u32, u32),
        capabilities: EglPartialRepaintCapabilities,
    ) -> Self {
        Self::new_with_policy(
            output_size,
            capabilities,
            configured_partial_repaint_complexity_policy(),
        )
    }

    pub(crate) fn new_with_policy(
        output_size: (u32, u32),
        capabilities: EglPartialRepaintCapabilities,
        complexity_policy: PartialRepaintComplexityPolicy,
    ) -> Self {
        Self {
            output_size,
            history: VecDeque::new(),
            history_valid: false,
            capabilities,
            force_full: force_full_repaint_enabled(),
            partial_enabled: true,
            complexity_policy,
        }
    }

    pub(crate) const fn complexity_policy(&self) -> PartialRepaintComplexityPolicy {
        self.complexity_policy
    }

    pub(crate) fn plan(&mut self, current_damage: OutputDamage, age: BufferAge) -> RepaintPlan {
        self.plan_inner(current_damage, age, None)
    }

    /// Trace-only planner entry point that observes the repair candidate at
    /// the point where the legacy reference policy would replace it with
    /// `OutputDamage::Full`, regardless of the selected actual policy.
    pub(crate) fn plan_with_damage_complexity_shadow(
        &mut self,
        current_damage: OutputDamage,
        age: BufferAge,
    ) -> (RepaintPlan, DamageComplexityShadow) {
        let mut shadow = DamageComplexityShadow::not_applicable();
        let output_size = self.output_size;
        let mut observe_candidate = |repair_damage: &OutputDamage, reason: FullRepaintReason| {
            if reason == FullRepaintReason::TooManyRectangles {
                shadow =
                    DamageComplexityShadow::for_candidate(repair_damage, output_size, Some(reason));
            }
        };
        let plan = self.plan_inner(current_damage, age, Some(&mut observe_candidate));
        (plan, shadow)
    }

    fn plan_inner(
        &mut self,
        current_damage: OutputDamage,
        age: BufferAge,
        mut observe_complexity_fallback: Option<&mut dyn FnMut(&OutputDamage, FullRepaintReason)>,
    ) -> RepaintPlan {
        if current_damage == OutputDamage::Empty {
            if !self.history_valid {
                return self.full_plan(
                    current_damage,
                    age_value(age),
                    FullRepaintReason::FirstFrameOrInvalidated,
                );
            }
            return RepaintPlan {
                render_damage: current_damage,
                repair_damage: OutputDamage::Empty,
                buffer_age: age_value(age),
                mode: RepaintMode::Skip,
                fallback_reason: None,
                complexity_policy: self.complexity_policy,
                complexity_action: PartialRepaintComplexityAction::NotApplicable,
            };
        }
        if current_damage == OutputDamage::Full {
            return self.full_plan(
                current_damage,
                age_value(age),
                FullRepaintReason::CurrentDamageFull,
            );
        }
        if self.force_full {
            return self.full_plan(
                current_damage,
                age_value(age),
                FullRepaintReason::ForcedFull,
            );
        }
        if !self.partial_enabled {
            return self.full_plan(
                current_damage,
                age_value(age),
                FullRepaintReason::PartialRepaintDisabled,
            );
        }
        if !self.capabilities.buffer_age {
            return self.full_plan(
                current_damage,
                None,
                FullRepaintReason::BufferAgeUnsupported,
            );
        }
        if !self.capabilities.partial_render_repair {
            return self.full_plan(
                current_damage,
                age_value(age),
                FullRepaintReason::PartialRenderRepairUnsupported,
            );
        }
        if !self.history_valid {
            return self.full_plan(
                current_damage,
                age_value(age),
                FullRepaintReason::FirstFrameOrInvalidated,
            );
        }

        let age = match age {
            BufferAge::Unsupported => {
                return self.full_plan(
                    current_damage,
                    None,
                    FullRepaintReason::BufferAgeUnsupported,
                );
            }
            BufferAge::QueryFailed => {
                return self.full_plan(
                    current_damage,
                    None,
                    FullRepaintReason::BufferAgeQueryFailed,
                );
            }
            BufferAge::Value(0) => {
                return self.full_plan(current_damage, Some(0), FullRepaintReason::BufferAgeZero);
            }
            BufferAge::Value(value) if value < 0 => {
                self.invalidate();
                return self.full_plan(current_damage, None, FullRepaintReason::BufferAgeInvalid);
            }
            BufferAge::Value(value) => value as u32,
        };
        if age > MAX_EXPLICIT_OUTPUT_BUFFER_AGE {
            return self.full_plan(
                current_damage,
                Some(age),
                FullRepaintReason::InsufficientHistory,
            );
        }
        let prior_count = usize::try_from(age.saturating_sub(1)).unwrap_or(usize::MAX);
        if prior_count > self.history.len() {
            self.invalidate();
            return self.full_plan(
                current_damage,
                Some(age),
                FullRepaintReason::InsufficientHistory,
            );
        }
        let mut repair_damage = current_damage.clone();
        for prior in self.history.iter().take(prior_count) {
            repair_damage =
                repair_damage.union(prior.clone(), self.output_size.0, self.output_size.1);
        }
        if repair_damage == OutputDamage::Empty {
            return RepaintPlan {
                render_damage: current_damage,
                repair_damage,
                buffer_age: Some(age),
                mode: RepaintMode::Skip,
                fallback_reason: None,
                complexity_policy: self.complexity_policy,
                complexity_action: PartialRepaintComplexityAction::NotApplicable,
            };
        }
        if let Some(observe) = observe_complexity_fallback.as_deref_mut()
            && matches!(
                decide_partial_repaint_candidate(
                    &repair_damage,
                    self.output_size,
                    PartialRepaintComplexityPolicy::Legacy,
                ),
                PartialRepaintCandidateDecision::Full {
                    reason: FullRepaintReason::TooManyRectangles,
                    ..
                }
            )
        {
            observe(&repair_damage, FullRepaintReason::TooManyRectangles);
        }
        match decide_partial_repaint_candidate(
            &repair_damage,
            self.output_size,
            self.complexity_policy,
        ) {
            PartialRepaintCandidateDecision::KeepOriginal { action } => RepaintPlan {
                render_damage: current_damage,
                repair_damage,
                buffer_age: Some(age),
                mode: RepaintMode::Partial,
                fallback_reason: None,
                complexity_policy: self.complexity_policy,
                complexity_action: action,
            },
            PartialRepaintCandidateDecision::UseBoundingBox { bbox, action } => RepaintPlan {
                render_damage: current_damage,
                repair_damage: OutputDamage::Rects(vec![bbox]),
                buffer_age: Some(age),
                mode: RepaintMode::Partial,
                fallback_reason: None,
                complexity_policy: self.complexity_policy,
                complexity_action: action,
            },
            PartialRepaintCandidateDecision::Full { reason, action } => {
                let mut plan = self.full_plan(current_damage, Some(age), reason);
                plan.complexity_action = action;
                plan
            }
        }
    }

    pub(crate) fn apply_execution_repair(
        &self,
        plan: &mut RepaintPlan,
        repair_damage: OutputDamage,
    ) {
        plan.complexity_policy = self.complexity_policy;
        if plan.mode == RepaintMode::Partial {
            match decide_partial_repaint_candidate(
                &repair_damage,
                self.output_size,
                self.complexity_policy,
            ) {
                PartialRepaintCandidateDecision::KeepOriginal { action } => {
                    plan.repair_damage = repair_damage;
                    plan.complexity_action = action;
                }
                PartialRepaintCandidateDecision::UseBoundingBox { bbox, action } => {
                    plan.repair_damage = OutputDamage::Rects(vec![bbox]);
                    plan.complexity_action = action;
                }
                PartialRepaintCandidateDecision::Full { reason, action } => {
                    plan.repair_damage = OutputDamage::Full;
                    plan.mode = RepaintMode::Full;
                    plan.fallback_reason = Some(reason);
                    plan.complexity_action = action;
                }
            }
        } else {
            // Preserve the existing behavior for callers that provide a
            // non-partial plan: update the damage without reclassifying it.
            plan.repair_damage = repair_damage;
        }
    }

    fn full_plan(
        &self,
        current_damage: OutputDamage,
        buffer_age: Option<u32>,
        reason: FullRepaintReason,
    ) -> RepaintPlan {
        RepaintPlan {
            render_damage: current_damage,
            repair_damage: OutputDamage::Full,
            buffer_age,
            mode: RepaintMode::Full,
            fallback_reason: Some(reason),
            complexity_policy: self.complexity_policy,
            complexity_action: PartialRepaintComplexityAction::NotApplicable,
        }
    }

    pub(crate) fn commit_presented_transition(&mut self, transition_damage: OutputDamage) {
        self.history.push_front(transition_damage);
        self.history.truncate(MAX_DAMAGE_HISTORY_FRAMES);
        self.history_valid = true;
    }

    pub(crate) fn discard_rendered(&mut self, _plan: &RepaintPlan) {
        // A rendered candidate has no presentation authority. Keeping this an
        // explicit operation makes discard paths consume their token without
        // mutating the last-presented damage journal.
    }

    pub(crate) fn swap_failed(&mut self) {
        self.invalidate();
    }

    pub(crate) fn invalidate(&mut self) {
        self.history.clear();
        self.history_valid = false;
    }

    pub(crate) fn resize(&mut self, output_size: (u32, u32)) {
        if self.output_size != output_size {
            self.output_size = output_size;
            self.invalidate();
        }
    }

    pub(crate) fn history_depth(&self) -> usize {
        self.history.len()
    }

    pub(crate) const fn capabilities(&self) -> EglPartialRepaintCapabilities {
        self.capabilities
    }

    pub(crate) const fn partial_enabled(&self) -> bool {
        self.partial_enabled && !self.force_full
    }
}

fn age_value(age: BufferAge) -> Option<u32> {
    match age {
        BufferAge::Value(value) => u32::try_from(value).ok(),
        BufferAge::Unsupported | BufferAge::QueryFailed => None,
    }
}

fn force_full_repaint_enabled() -> bool {
    std::env::var_os("OBLIVION_ONE_FORCE_FULL_REPAINT").is_some_and(|value| value == "1")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EglDamageRects {
    values: Vec<egl::Int>,
}

impl EglDamageRects {
    fn new() -> Self {
        Self { values: Vec::new() }
    }

    fn push(&mut self, rect: [i32; 4]) {
        self.values.extend(rect);
    }

    fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub(crate) fn rect_count(&self) -> usize {
        self.values.len() / 4
    }

    pub(crate) fn as_ptr(&self) -> *const egl::Int {
        self.values.as_ptr()
    }

    #[cfg(test)]
    pub(super) fn as_slice(&self) -> &[egl::Int] {
        &self.values
    }
}

#[derive(Debug, Clone)]
pub(super) struct EglOutputDamageTracker {
    cursor_image: Arc<CompositorCursorImage>,
    output_size: (u32, u32),
    last_cursor_rect: Option<SurfaceDamageRect>,
    last_client_cursor: Option<ClientCursorDamageState>,
}

impl Default for EglOutputDamageTracker {
    fn default() -> Self {
        Self::with_cursor_image(shared_compositor_cursor_image())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct EglPresentedDamageState {
    output_size: (u32, u32),
    cursor_rect: Option<SurfaceDamageRect>,
    client_cursor: Option<ClientCursorDamageState>,
}

#[cfg(test)]
impl EglPresentedDamageState {
    pub(super) const fn empty_for_test() -> Self {
        Self {
            output_size: (1, 1),
            cursor_rect: None,
            client_cursor: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ClientCursorDamageState {
    pub(super) rect: Option<SurfaceDamageRect>,
    generation: u64,
}

impl ClientCursorDamageState {
    pub(super) fn new(
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        generation: u64,
        output_width: u32,
        output_height: u32,
    ) -> Self {
        Self {
            rect: arbitrary_cursor_damage_rect(x, y, width, height, output_width, output_height),
            generation,
        }
    }
}

impl EglOutputDamageTracker {
    pub(super) fn with_cursor_image(cursor_image: Arc<CompositorCursorImage>) -> Self {
        Self {
            cursor_image,
            output_size: (0, 0),
            last_cursor_rect: None,
            last_client_cursor: None,
        }
    }

    pub(super) fn set_cursor_image(&mut self, cursor_image: Arc<CompositorCursorImage>) {
        self.cursor_image = cursor_image;
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn damage_for_frame(
        &self,
        width: u32,
        height: u32,
        scene_changed: bool,
        authoritative_scene_damage: Option<OutputDamage>,
        visual_state: DesktopVisualState,
        client_cursor: Option<ClientCursorDamageState>,
    ) -> OutputDamage {
        let cursor_rect = visual_state.cursor.and_then(|(x, y)| {
            cursor_damage_rect_for_image(x, y, width, height, &self.cursor_image)
        });
        let size_changed = self.output_size != (width, height);

        let mut damage = if size_changed {
            OutputDamage::Full
        } else if let Some(damage) = authoritative_scene_damage {
            damage
        } else if scene_changed {
            OutputDamage::Full
        } else {
            OutputDamage::Empty
        };
        let mut overlay_rects = Vec::new();
        if self.last_cursor_rect != cursor_rect {
            overlay_rects.extend(self.last_cursor_rect);
            overlay_rects.extend(cursor_rect);
        }
        if self.last_client_cursor != client_cursor {
            overlay_rects.extend(self.last_client_cursor.and_then(|cursor| cursor.rect));
            overlay_rects.extend(client_cursor.and_then(|cursor| cursor.rect));
        }
        damage = damage.union(
            OutputDamage::from_surface_rects(width, height, overlay_rects),
            width,
            height,
        );
        damage
    }

    pub(super) fn candidate_state(
        width: u32,
        height: u32,
        visual_state: DesktopVisualState,
        client_cursor: Option<ClientCursorDamageState>,
        cursor_image: &CompositorCursorImage,
    ) -> EglPresentedDamageState {
        EglPresentedDamageState {
            output_size: (width, height),
            cursor_rect: visual_state
                .cursor
                .and_then(|(x, y)| cursor_damage_rect_for_image(x, y, width, height, cursor_image)),
            client_cursor,
        }
    }

    pub(super) fn commit_presented(&mut self, state: EglPresentedDamageState) {
        self.output_size = state.output_size;
        self.last_cursor_rect = state.cursor_rect;
        self.last_client_cursor = state.client_cursor;
    }
}

pub(super) fn cursor_damage_rect_for_image(
    cursor_x: i32,
    cursor_y: i32,
    output_width: u32,
    output_height: u32,
    cursor_image: &CompositorCursorImage,
) -> Option<SurfaceDamageRect> {
    cursor_damage_rect(
        cursor_x,
        cursor_y,
        output_width,
        output_height,
        cursor_image,
    )
}

fn arbitrary_cursor_damage_rect(
    cursor_x: i32,
    cursor_y: i32,
    cursor_width: u32,
    cursor_height: u32,
    output_width: u32,
    output_height: u32,
) -> Option<SurfaceDamageRect> {
    let rect = OutputRect::new(cursor_x, cursor_y, cursor_width, cursor_height)
        .clipped(output_width, output_height)?;
    Some(SurfaceDamageRect {
        x: rect.x.try_into().ok()?,
        y: rect.y.try_into().ok()?,
        width: rect.width,
        height: rect.height,
    })
}

#[cfg(test)]
#[path = "damage_tests.rs"]
mod partial_repaint_tests;
