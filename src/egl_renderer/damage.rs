use std::{collections::VecDeque, sync::Arc};

use khronos_egl as egl;
use oblivion_one::{
    compositor::{DesktopVisualState, SurfaceDamageRect, cursor_damage_rect},
    cursor_theme::{CompositorCursorImage, shared_compositor_cursor_image},
};

use super::OutputFramebufferOrigin;

pub(crate) const MAX_PARTIAL_REPAINT_RECTS: usize = 8;
pub(crate) const MAX_DAMAGE_HISTORY_FRAMES: usize = 8;
const MAX_EXPLICIT_OUTPUT_BUFFER_AGE: u32 = 3;
const MAX_PARTIAL_REPAINT_PERCENT: u64 = 75;

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
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepaintPlan {
    pub(crate) current_damage: OutputDamage,
    pub(crate) repair_damage: OutputDamage,
    pub(crate) buffer_age: Option<u32>,
    pub(crate) mode: RepaintMode,
    pub(crate) fallback_reason: Option<FullRepaintReason>,
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

#[derive(Debug)]
pub(crate) struct PartialRepaintPlanner {
    output_size: (u32, u32),
    history: VecDeque<OutputDamage>,
    history_valid: bool,
    capabilities: EglPartialRepaintCapabilities,
    force_full: bool,
    partial_enabled: bool,
}

impl PartialRepaintPlanner {
    pub(crate) fn new(
        output_size: (u32, u32),
        capabilities: EglPartialRepaintCapabilities,
    ) -> Self {
        Self {
            output_size,
            history: VecDeque::new(),
            history_valid: false,
            capabilities,
            force_full: force_full_repaint_enabled(),
            partial_enabled: true,
        }
    }

    pub(crate) fn plan(&mut self, current_damage: OutputDamage, age: BufferAge) -> RepaintPlan {
        if current_damage == OutputDamage::Empty {
            return RepaintPlan {
                current_damage,
                repair_damage: OutputDamage::Empty,
                buffer_age: age_value(age),
                mode: RepaintMode::Skip,
                fallback_reason: None,
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
                current_damage,
                repair_damage,
                buffer_age: Some(age),
                mode: RepaintMode::Skip,
                fallback_reason: None,
            };
        }
        if repair_damage == OutputDamage::Full {
            return self.full_plan(
                current_damage,
                Some(age),
                FullRepaintReason::DamageAreaThreshold,
            );
        }
        if repair_damage.rect_count() > MAX_PARTIAL_REPAINT_RECTS {
            return self.full_plan(
                current_damage,
                Some(age),
                FullRepaintReason::TooManyRectangles,
            );
        }
        let Some(repair_pixels) = repair_damage.pixels(self.output_size.0, self.output_size.1)
        else {
            return self.full_plan(
                current_damage,
                Some(age),
                FullRepaintReason::DamageAreaThreshold,
            );
        };
        let Some(output_pixels) =
            u64::from(self.output_size.0).checked_mul(u64::from(self.output_size.1))
        else {
            return self.full_plan(
                current_damage,
                Some(age),
                FullRepaintReason::DamageAreaThreshold,
            );
        };
        if output_pixels == 0
            || repair_pixels.saturating_mul(100)
                >= output_pixels.saturating_mul(MAX_PARTIAL_REPAINT_PERCENT)
        {
            return self.full_plan(
                current_damage,
                Some(age),
                FullRepaintReason::DamageAreaThreshold,
            );
        }
        RepaintPlan {
            current_damage,
            repair_damage,
            buffer_age: Some(age),
            mode: RepaintMode::Partial,
            fallback_reason: None,
        }
    }

    fn full_plan(
        &self,
        current_damage: OutputDamage,
        buffer_age: Option<u32>,
        reason: FullRepaintReason,
    ) -> RepaintPlan {
        RepaintPlan {
            current_damage,
            repair_damage: OutputDamage::Full,
            buffer_age,
            mode: RepaintMode::Full,
            fallback_reason: Some(reason),
        }
    }

    pub(crate) fn commit_presented(&mut self, plan: &RepaintPlan) {
        self.history.push_front(plan.current_damage.clone());
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

#[derive(Debug)]
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
