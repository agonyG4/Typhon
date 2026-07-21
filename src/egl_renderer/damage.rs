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
    pub(crate) buffer_age: bool,
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
    presentation_pending: bool,
) -> BufferAge {
    if presentation_pending {
        BufferAge::Value(0)
    } else {
        software_buffer_age(presentation_serial, last_presented_serial)
    }
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
    SwapDamageUnsupported,
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
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::CurrentDamageFull => "current_damage_full",
            Self::FirstFrameOrInvalidated => "history_invalid",
            Self::BufferAgeUnsupported => "buffer_age_unsupported",
            Self::SwapDamageUnsupported => "swap_damage_unsupported",
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
            partial_enabled: partial_repaint_enabled(),
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
        if !self.capabilities.swap_buffers_with_damage {
            return self.full_plan(
                current_damage,
                age_value(age),
                FullRepaintReason::SwapDamageUnsupported,
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

fn partial_repaint_enabled() -> bool {
    std::env::var_os("OBLIVION_ONE_ENABLE_PARTIAL_REPAINT").is_some_and(|value| value == "1")
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
mod partial_repaint_tests {
    use super::*;

    fn rect(x: i32, y: i32, width: u32, height: u32) -> OutputRect {
        OutputRect::new(x, y, width, height)
    }

    fn partial_capabilities() -> EglPartialRepaintCapabilities {
        EglPartialRepaintCapabilities {
            buffer_age: true,
            swap_buffers_with_damage: true,
        }
    }

    fn partial_planner(
        output_size: (u32, u32),
        capabilities: EglPartialRepaintCapabilities,
    ) -> PartialRepaintPlanner {
        let mut planner = PartialRepaintPlanner::new(output_size, capabilities);
        planner.partial_enabled = true;
        planner
    }

    #[test]
    fn output_damage_clips_all_edges_and_discards_empty_rectangles() {
        let damage = OutputDamage::rects(
            100,
            80,
            [
                rect(-5, 10, 10, 10),
                rect(95, 10, 10, 10),
                rect(10, -5, 10, 10),
                rect(10, 75, 10, 10),
                rect(0, 0, 0, 5),
            ],
        );
        assert_eq!(
            damage,
            OutputDamage::Rects(vec![
                rect(0, 10, 5, 10),
                rect(95, 10, 5, 10),
                rect(10, 0, 10, 5),
                rect(10, 75, 10, 5),
            ])
        );
    }

    #[test]
    fn output_damage_coalesces_overlapping_and_touching_rectangles() {
        assert_eq!(
            OutputDamage::rects(
                100,
                100,
                [rect(5, 5, 10, 10), rect(15, 5, 5, 10), rect(8, 8, 4, 4)],
            ),
            OutputDamage::Rects(vec![rect(5, 5, 15, 10)])
        );
    }

    #[test]
    fn output_damage_converts_top_left_rectangles_for_gl_and_egl() {
        let damage = OutputDamage::rects(100, 80, [rect(4, 7, 9, 11)]);
        assert_eq!(
            damage
                .to_gl_scissors(100, 80, OutputFramebufferOrigin::BottomLeft)
                .unwrap(),
            vec![[4, 62, 9, 11]]
        );
        assert_eq!(
            damage.to_egl_rects(100, 80).unwrap().as_slice(),
            &[4, 62, 9, 11]
        );
    }

    #[test]
    fn output_damage_converts_one_pixel_rectangles_at_every_edge() {
        let damage = OutputDamage::rects(
            8,
            6,
            [
                rect(0, 0, 1, 1),
                rect(0, 5, 1, 1),
                rect(7, 2, 1, 1),
                rect(3, 0, 1, 1),
            ],
        );
        assert_eq!(
            damage
                .to_gl_scissors(8, 6, OutputFramebufferOrigin::BottomLeft)
                .unwrap(),
            vec![[0, 5, 1, 1], [0, 0, 1, 1], [7, 3, 1, 1], [3, 5, 1, 1]]
        );
    }

    #[test]
    fn first_frame_and_unsupported_buffer_age_force_full_repaint() {
        let current = OutputDamage::rects(100, 80, [rect(2, 3, 4, 5)]);
        let mut planner = partial_planner((100, 80), partial_capabilities());
        assert_eq!(
            planner.plan(current.clone(), BufferAge::Value(1)).mode,
            RepaintMode::Full
        );
        let mut unsupported = partial_planner(
            (100, 80),
            EglPartialRepaintCapabilities {
                buffer_age: false,
                swap_buffers_with_damage: true,
            },
        );
        assert_eq!(
            unsupported.plan(current, BufferAge::Unsupported).mode,
            RepaintMode::Full
        );
    }

    #[test]
    fn software_buffer_age_uses_output_presentation_serials() {
        assert_eq!(software_buffer_age(10, None), BufferAge::Value(0));
        assert_eq!(software_buffer_age(10, Some(9)), BufferAge::Value(2));
        assert_eq!(software_buffer_age(10, Some(8)), BufferAge::Value(3));
        assert_eq!(software_buffer_age(10, Some(10)), BufferAge::Value(1));
        assert_eq!(software_buffer_age(10, Some(11)), BufferAge::Value(-1));
    }

    #[test]
    fn pending_presentation_invalidates_reused_render_target_age() {
        assert_eq!(
            render_target_buffer_age(10, Some(8), false),
            BufferAge::Value(3)
        );
        assert_eq!(
            render_target_buffer_age(10, Some(8), true),
            BufferAge::Value(0)
        );
    }

    #[test]
    fn buffer_age_beyond_three_slot_history_forces_full_repaint() {
        let mut planner = partial_planner((100, 80), partial_capabilities());
        for x in [1, 10, 20] {
            let plan = planner.plan(
                OutputDamage::rects(100, 80, [rect(x, 1, 2, 2)]),
                BufferAge::Value(1),
            );
            planner.commit_presented(&plan);
        }

        let unsupported = planner.plan(
            OutputDamage::rects(100, 80, [rect(30, 1, 2, 2)]),
            BufferAge::Value(4),
        );

        assert_eq!(unsupported.mode, RepaintMode::Full);
        assert_eq!(
            unsupported.fallback_reason,
            Some(FullRepaintReason::InsufficientHistory)
        );
    }

    #[test]
    fn empty_logical_damage_skips_even_when_history_is_invalid() {
        let mut planner = partial_planner((100, 80), partial_capabilities());

        assert_eq!(
            planner.plan(OutputDamage::Empty, BufferAge::Value(0)).mode,
            RepaintMode::Skip
        );
        assert_eq!(planner.history_depth(), 0);
    }

    #[test]
    fn usable_ages_accumulate_only_required_logical_damage() {
        let first = OutputDamage::rects(100, 80, [rect(1, 1, 3, 3)]);
        let second = OutputDamage::rects(100, 80, [rect(20, 20, 3, 3)]);
        let third = OutputDamage::rects(100, 80, [rect(40, 40, 3, 3)]);
        let mut planner = partial_planner((100, 80), partial_capabilities());
        let plan = planner.plan(first, BufferAge::Value(0));
        planner.commit_presented(&plan);
        let plan = planner.plan(second.clone(), BufferAge::Value(1));
        assert_eq!(plan.repair_damage, second);
        planner.commit_presented(&plan);
        let plan = planner.plan(third, BufferAge::Value(2));
        assert_eq!(
            plan.repair_damage,
            OutputDamage::Rects(vec![rect(40, 40, 3, 3), rect(20, 20, 3, 3)])
        );
        planner.commit_presented(&plan);
        let fourth = OutputDamage::rects(100, 80, [rect(60, 60, 3, 3)]);
        assert_eq!(
            planner
                .plan(fourth, BufferAge::Value(3))
                .repair_damage
                .rect_count(),
            3
        );
    }

    #[test]
    fn invalid_age_history_and_resize_force_full_repaint() {
        let current = OutputDamage::rects(100, 80, [rect(2, 3, 4, 5)]);
        let mut planner = partial_planner((100, 80), partial_capabilities());
        let first = planner.plan(OutputDamage::Full, BufferAge::Value(0));
        planner.commit_presented(&first);
        assert_eq!(
            planner.plan(current.clone(), BufferAge::Value(0)).mode,
            RepaintMode::Full
        );
        assert_eq!(
            planner.plan(current.clone(), BufferAge::Value(9)).mode,
            RepaintMode::Full
        );
        planner.resize((120, 80));
        assert_eq!(
            planner.plan(current, BufferAge::Value(1)).mode,
            RepaintMode::Full
        );
    }

    #[test]
    fn failed_swap_does_not_advance_history_and_empty_stays_empty() {
        let current = OutputDamage::rects(100, 80, [rect(2, 3, 4, 5)]);
        let mut planner = partial_planner((100, 80), partial_capabilities());
        let first = planner.plan(OutputDamage::Full, BufferAge::Value(0));
        planner.commit_presented(&first);
        let failed = planner.plan(current, BufferAge::Value(1));
        planner.swap_failed();
        assert_eq!(planner.history_depth(), 0);
        assert_eq!(
            planner
                .plan(OutputDamage::Empty, BufferAge::Value(1))
                .current_damage,
            OutputDamage::Empty
        );
        assert_eq!(failed.current_damage.rect_count(), 1);
    }

    #[test]
    fn rendered_candidate_does_not_advance_history_until_matching_commit() {
        let mut planner = partial_planner((100, 80), partial_capabilities());
        let candidate = planner.plan(OutputDamage::Full, BufferAge::Value(0));

        assert_eq!(planner.history_depth(), 0);
        planner.commit_presented(&candidate);
        assert_eq!(planner.history_depth(), 1);
    }

    #[test]
    fn discarded_rendered_candidate_does_not_advance_or_invalidate_history() {
        let mut planner = partial_planner((100, 80), partial_capabilities());
        let presented = planner.plan(
            OutputDamage::rects(100, 80, [rect(1, 2, 2, 2)]),
            BufferAge::Value(0),
        );
        planner.commit_presented(&presented);
        let discarded = planner.plan(
            OutputDamage::rects(100, 80, [rect(4, 5, 6, 7)]),
            BufferAge::Value(2),
        );

        planner.discard_rendered(&discarded);

        assert_eq!(planner.history_depth(), 1);
        assert_eq!(
            planner
                .plan(
                    OutputDamage::rects(100, 80, [rect(20, 21, 2, 2)]),
                    BufferAge::Value(2),
                )
                .repair_damage
                .rect_count(),
            2
        );
    }

    #[test]
    fn two_rendered_candidates_can_coexist_before_one_is_committed() {
        let mut planner = partial_planner((100, 80), partial_capabilities());
        let first = planner.plan(OutputDamage::Full, BufferAge::Value(0));
        let second = planner.plan(
            OutputDamage::rects(100, 80, [rect(8, 9, 3, 3)]),
            BufferAge::Value(0),
        );

        assert_eq!(planner.history_depth(), 0);
        planner.discard_rendered(&first);
        planner.commit_presented(&second);
        assert_eq!(planner.history_depth(), 1);
    }

    #[test]
    fn policy_falls_back_for_many_rectangles_or_near_full_area() {
        let mut planner = partial_planner((100, 100), partial_capabilities());
        let first = planner.plan(OutputDamage::Full, BufferAge::Value(0));
        planner.commit_presented(&first);
        let many = OutputDamage::Rects(
            (0..=MAX_PARTIAL_REPAINT_RECTS)
                .map(|index| rect((index * 3) as i32, 1, 1, 1))
                .collect(),
        );
        assert_eq!(
            planner.plan(many, BufferAge::Value(1)).mode,
            RepaintMode::Full
        );
        let near_full = OutputDamage::rects(100, 100, [rect(0, 0, 90, 90)]);
        assert_eq!(
            planner.plan(near_full, BufferAge::Value(1)).mode,
            RepaintMode::Full
        );
    }

    #[test]
    fn partial_repaint_is_disabled_by_default() {
        let mut planner = PartialRepaintPlanner::new((100, 80), partial_capabilities());
        let first = planner.plan(OutputDamage::Full, BufferAge::Value(0));
        planner.commit_presented(&first);

        let plan = planner.plan(
            OutputDamage::rects(100, 80, [rect(4, 7, 9, 11)]),
            BufferAge::Value(1),
        );

        assert_eq!(plan.mode, RepaintMode::Full);
    }

    #[test]
    fn render_execution_plan_clears_each_partial_scissor_and_restores_state() {
        let plan = RepaintPlan {
            current_damage: OutputDamage::rects(100, 80, [rect(4, 7, 9, 11)]),
            repair_damage: OutputDamage::rects(100, 80, [rect(4, 7, 9, 11), rect(30, 40, 5, 6)]),
            buffer_age: Some(2),
            mode: RepaintMode::Partial,
            fallback_reason: None,
        };

        assert_eq!(
            plan.render_execution(100, 80, OutputFramebufferOrigin::BottomLeft)
                .unwrap(),
            RenderExecution::Scissored {
                scissors: vec![[4, 62, 9, 11], [30, 34, 5, 6]],
                disable_scissor_after: true,
            }
        );
        assert_eq!(plan.swap_damage(), &plan.repair_damage);
    }

    #[test]
    fn skipped_plan_has_no_gl_execution() {
        let plan = RepaintPlan {
            current_damage: OutputDamage::Empty,
            repair_damage: OutputDamage::Empty,
            buffer_age: Some(1),
            mode: RepaintMode::Skip,
            fallback_reason: None,
        };

        assert_eq!(
            plan.render_execution(100, 80, OutputFramebufferOrigin::BottomLeft),
            None
        );
    }

    #[test]
    fn successful_swap_records_logical_damage_instead_of_expanded_repair() {
        let mut planner = partial_planner((100, 80), partial_capabilities());
        let initial = planner.plan(
            OutputDamage::rects(100, 80, [rect(1, 1, 2, 2)]),
            BufferAge::Value(0),
        );
        planner.commit_presented(&initial);
        let second = planner.plan(
            OutputDamage::rects(100, 80, [rect(20, 20, 2, 2)]),
            BufferAge::Value(2),
        );
        planner.commit_presented(&second);

        let third = planner.plan(
            OutputDamage::rects(100, 80, [rect(40, 40, 2, 2)]),
            BufferAge::Value(2),
        );
        assert_eq!(third.repair_damage.rect_count(), 2);
        assert!(
            !third
                .repair_damage
                .rects_slice()
                .contains(&rect(1, 1, 2, 2))
        );
    }

    #[test]
    fn full_damage_conversion_and_checked_area_are_explicit() {
        assert_eq!(
            OutputDamage::Full
                .to_gl_scissors(8, 6, OutputFramebufferOrigin::BottomLeft)
                .unwrap(),
            vec![[0, 0, 8, 6]]
        );
        let overflowing = OutputDamage::Rects(vec![
            rect(0, 0, u32::MAX, u32::MAX),
            rect(0, 0, u32::MAX, u32::MAX),
        ]);
        assert_eq!(overflowing.pixels(u32::MAX, u32::MAX), None);
    }

    #[test]
    fn full_current_damage_wins_and_surface_invalidation_forces_full() {
        let mut planner = partial_planner((100, 80), partial_capabilities());
        let first = planner.plan(OutputDamage::Full, BufferAge::Value(0));
        planner.commit_presented(&first);
        assert_eq!(
            planner.plan(OutputDamage::Full, BufferAge::Value(1)).mode,
            RepaintMode::Full
        );
        planner.invalidate();
        let partial = OutputDamage::rects(100, 80, [rect(1, 2, 3, 4)]);
        assert_eq!(
            planner.plan(partial, BufferAge::Value(1)).fallback_reason,
            Some(FullRepaintReason::FirstFrameOrInvalidated)
        );
    }

    #[test]
    fn histories_are_isolated_per_planner_surface() {
        let mut first = partial_planner((100, 80), partial_capabilities());
        let second = partial_planner((100, 80), partial_capabilities());
        let plan = first.plan(OutputDamage::Full, BufferAge::Value(0));
        first.commit_presented(&plan);

        assert_eq!(first.history_depth(), 1);
        assert_eq!(second.history_depth(), 0);
    }

    #[test]
    fn triple_buffer_swapchain_oracle_matches_full_reference() {
        let mut planner = partial_planner((12, 1), partial_capabilities());
        let mut buffers = [vec![0u8; 12], vec![0u8; 12], vec![0u8; 12]];
        let mut last_presented = [None::<u32>; 3];
        let serial = std::cell::Cell::new(0u32);

        let mut present = |planner: &mut PartialRepaintPlanner,
                           buffer_index: usize,
                           reference: &[u8],
                           logical: OutputDamage,
                           fail_swap: bool| {
            let age = last_presented[buffer_index]
                .map(|last| serial.get().saturating_sub(last).saturating_add(1))
                .unwrap_or(0);
            let plan = planner.plan(logical, BufferAge::Value(age as i32));
            assert_ne!(plan.mode, RepaintMode::Skip);
            match &plan.repair_damage {
                OutputDamage::Empty => panic!("rendered oracle plan cannot be empty"),
                OutputDamage::Full => buffers[buffer_index].copy_from_slice(reference),
                OutputDamage::Rects(rects) => {
                    for rect in rects {
                        let start = usize::try_from(rect.x.max(0)).unwrap();
                        let end = start
                            .saturating_add(rect.width as usize)
                            .min(reference.len());
                        buffers[buffer_index][start..end].copy_from_slice(&reference[start..end]);
                    }
                }
            }
            if fail_swap {
                planner.swap_failed();
                return;
            }
            assert_eq!(buffers[buffer_index], reference);
            serial.set(serial.get().saturating_add(1));
            last_presented[buffer_index] = Some(serial.get());
            planner.commit_presented(&plan);
        };

        let mut reference = vec![0u8; 12];
        reference[1] = 1;
        present(&mut planner, 0, &reference, OutputDamage::Full, false);

        reference[4] = 2;
        present(
            &mut planner,
            1,
            &reference,
            OutputDamage::rects(12, 1, [rect(4, 0, 1, 1)]),
            false,
        );

        reference[7] = 3;
        present(
            &mut planner,
            2,
            &reference,
            OutputDamage::rects(12, 1, [rect(7, 0, 1, 1)]),
            false,
        );

        reference[8] = 4;
        present(
            &mut planner,
            2,
            &reference,
            OutputDamage::rects(12, 1, [rect(8, 0, 1, 1)]),
            false,
        );

        let serial_before_skip = serial.get();
        let skipped = planner.plan(OutputDamage::Empty, BufferAge::Value(3));
        assert_eq!(skipped.mode, RepaintMode::Skip);
        assert_eq!(serial.get(), serial_before_skip);

        reference[1] = 0;
        reference[2] = 5;
        present(
            &mut planner,
            1,
            &reference,
            OutputDamage::rects(12, 1, [rect(1, 0, 2, 1)]),
            false,
        );

        reference[10] = 6;
        present(
            &mut planner,
            2,
            &reference,
            OutputDamage::rects(12, 1, [rect(10, 0, 1, 1)]),
            true,
        );
        assert_eq!(serial.get(), serial_before_skip + 1);
        present(
            &mut planner,
            2,
            &reference,
            OutputDamage::rects(12, 1, [rect(10, 0, 1, 1)]),
            false,
        );

        planner.resize((16, 1));
        let resized_reference = vec![9u8; 16];
        let resized = planner.plan(OutputDamage::Full, BufferAge::Value(0));
        assert_eq!(resized.mode, RepaintMode::Full);
        let mut resized_buffer = vec![0u8; 16];
        resized_buffer.copy_from_slice(&resized_reference);
        assert_eq!(resized_buffer, resized_reference);
        planner.commit_presented(&resized);
    }

    #[test]
    fn logical_top_damage_maps_to_origin_specific_gl_rows() {
        let damage = OutputDamage::rects(100, 80, [rect(4, 0, 9, 11)]);

        assert_eq!(
            damage
                .to_gl_scissors(
                    100,
                    80,
                    crate::egl_renderer::OutputFramebufferOrigin::BottomLeft
                )
                .unwrap(),
            vec![[4, 69, 9, 11]]
        );
        assert_eq!(
            damage
                .to_gl_scissors(
                    100,
                    80,
                    crate::egl_renderer::OutputFramebufferOrigin::TopLeftScanout,
                )
                .unwrap(),
            vec![[4, 0, 9, 11]]
        );
    }

    #[test]
    fn logical_bottom_damage_maps_to_origin_specific_gl_rows() {
        let damage = OutputDamage::rects(100, 80, [rect(4, 69, 9, 11)]);

        assert_eq!(
            damage
                .to_gl_scissors(
                    100,
                    80,
                    crate::egl_renderer::OutputFramebufferOrigin::BottomLeft
                )
                .unwrap(),
            vec![[4, 0, 9, 11]]
        );
        assert_eq!(
            damage
                .to_gl_scissors(
                    100,
                    80,
                    crate::egl_renderer::OutputFramebufferOrigin::TopLeftScanout,
                )
                .unwrap(),
            vec![[4, 69, 9, 11]]
        );
    }

    #[test]
    fn partial_render_execution_uses_scanout_damage_rows() {
        let plan = RepaintPlan {
            current_damage: OutputDamage::rects(100, 80, [rect(4, 0, 9, 11)]),
            repair_damage: OutputDamage::rects(100, 80, [rect(4, 0, 9, 11)]),
            buffer_age: Some(2),
            mode: RepaintMode::Partial,
            fallback_reason: None,
        };

        assert_eq!(
            plan.render_execution(
                100,
                80,
                crate::egl_renderer::OutputFramebufferOrigin::TopLeftScanout,
            )
            .unwrap(),
            RenderExecution::Scissored {
                scissors: vec![[4, 0, 9, 11]],
                disable_scissor_after: true,
            }
        );
    }
}
