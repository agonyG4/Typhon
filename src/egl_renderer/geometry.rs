use oblivion_one::compositor::ServerFrameColor;

use super::OutputFramebufferOrigin;

pub(super) const MIN_VERTEX_BUFFER_BYTES: usize = 4096;
pub(super) const VERTEX_STRIDE: i32 = std::mem::size_of::<EglTexturedVertex>() as i32;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct EglRect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

impl EglRect {
    pub(super) const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub(super) const fn x(self) -> f32 {
        self.x
    }

    pub(super) const fn y(self) -> f32 {
        self.y
    }

    pub(super) const fn width(self) -> f32 {
        self.width
    }

    pub(super) const fn height(self) -> f32 {
        self.height
    }

    pub(super) fn intersects_output_rect(self, rect: super::OutputRect) -> bool {
        let left = f64::from(self.x);
        let top = f64::from(self.y);
        let right = left + f64::from(self.width);
        let bottom = top + f64::from(self.height);
        let rect_left = f64::from(rect.x);
        let rect_top = f64::from(rect.y);
        let rect_right = rect_left + f64::from(rect.width);
        let rect_bottom = rect_top + f64::from(rect.height);
        self.width > 0.0
            && self.height > 0.0
            && rect.width > 0
            && rect.height > 0
            && right > rect_left
            && rect_right > left
            && bottom > rect_top
            && rect_bottom > top
    }

    fn intersection(self, other: Self) -> Option<Self> {
        let left = self.x.max(other.x);
        let top = self.y.max(other.y);
        let right = (self.x + self.width).min(other.x + other.width);
        let bottom = (self.y + self.height).min(other.y + other.height);
        (right > left && bottom > top).then_some(Self::new(left, top, right - left, bottom - top))
    }
}

const MAX_OPAQUE_COVERAGE_PIECES: usize = 32;
const MAX_VISIBLE_REGION_PIECES: usize = MAX_OPAQUE_COVERAGE_PIECES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EglVisibilityDecision {
    Drawable,
    OutsideRemaining,
    Occluded,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct EglVisibilityPlanStats {
    pub(super) commands_visited: usize,
    pub(super) commands_drawable: usize,
    pub(super) commands_rejected_outside_remaining: usize,
    pub(super) commands_rejected_occluded: usize,
    pub(super) opaque_rectangles_subtracted: usize,
    pub(super) overflow_fallback: bool,
    pub(super) early_terminated: bool,
    pub(super) peak_region_pieces: usize,
}

#[derive(Debug, Clone, Copy)]
struct EglVisibleRegion {
    pieces: [Option<EglRect>; MAX_VISIBLE_REGION_PIECES],
    len: usize,
    occlusion_disabled: bool,
}

impl EglVisibleRegion {
    fn new(rect: EglRect) -> Self {
        let mut region = Self {
            pieces: [None; MAX_VISIBLE_REGION_PIECES],
            len: 0,
            occlusion_disabled: false,
        };
        if rect.width > 0.0 && rect.height > 0.0 {
            region.pieces[0] = Some(rect);
            region.len = 1;
        }
        region
    }

    fn is_empty(self) -> bool {
        self.len == 0
    }

    fn intersects(self, rect: EglRect) -> bool {
        self.pieces
            .iter()
            .take(self.len)
            .flatten()
            .any(|piece| piece.intersection(rect).is_some())
    }

    fn subtract(&mut self, excluded: EglRect) -> bool {
        if self.occlusion_disabled {
            return true;
        }
        let mut next = [None; MAX_VISIBLE_REGION_PIECES];
        let mut next_len = 0;
        for piece in self.pieces.iter().take(self.len).flatten() {
            let mut residuals = [None; 4];
            let residual_count = subtract_rect(*piece, excluded, &mut residuals);
            for residual in residuals.iter().take(residual_count).flatten() {
                if next_len == MAX_VISIBLE_REGION_PIECES {
                    return false;
                }
                next[next_len] = Some(*residual);
                next_len += 1;
            }
        }
        self.pieces = next;
        self.len = next_len;
        true
    }

    fn disable_occlusion(&mut self, repair: EglRect) {
        self.occlusion_disabled = true;
        self.pieces = [None; MAX_VISIBLE_REGION_PIECES];
        self.len = 0;
        if repair.width > 0.0 && repair.height > 0.0 {
            self.pieces[0] = Some(repair);
            self.len = 1;
        }
    }
}

pub(super) fn plan_visibility(
    commands: &[EglDrawCommand],
    repair: EglRect,
    decisions: &mut Vec<EglVisibilityDecision>,
) -> EglVisibilityPlanStats {
    decisions.clear();
    decisions.resize(commands.len(), EglVisibilityDecision::Occluded);
    let mut stats = EglVisibilityPlanStats::default();
    let mut remaining = EglVisibleRegion::new(repair);
    stats.peak_region_pieces = remaining.len;

    for index in (0..commands.len()).rev() {
        stats.commands_visited += 1;
        if remaining.is_empty() {
            stats.early_terminated = true;
            break;
        }
        let command = &commands[index];
        let Some(visible_bounds) = command.visible_bounds() else {
            decisions[index] = EglVisibilityDecision::OutsideRemaining;
            stats.commands_rejected_outside_remaining += 1;
            continue;
        };
        if !remaining.intersects(visible_bounds) {
            decisions[index] = EglVisibilityDecision::OutsideRemaining;
            stats.commands_rejected_outside_remaining += 1;
            continue;
        }

        decisions[index] = EglVisibilityDecision::Drawable;
        stats.commands_drawable += 1;
        if remaining.occlusion_disabled {
            continue;
        }
        for opaque_region in &command.opaque_regions {
            let Some(opaque_region) = command
                .presentation_clip
                .map_or(Some(*opaque_region), |clip| {
                    opaque_region.intersection(clip)
                })
            else {
                continue;
            };
            stats.opaque_rectangles_subtracted += 1;
            if !remaining.subtract(opaque_region) {
                stats.overflow_fallback = true;
                remaining.disable_occlusion(repair);
                break;
            }
            stats.peak_region_pieces = stats.peak_region_pieces.max(remaining.len);
            if remaining.is_empty() {
                stats.early_terminated = true;
                break;
            }
        }
        if stats.early_terminated {
            break;
        }
    }
    stats.commands_rejected_occluded = decisions
        .iter()
        .filter(|decision| **decision == EglVisibilityDecision::Occluded)
        .count();
    stats
}

pub(super) fn plan_capture_visibility(
    commands: &[EglDrawCommand],
    command_indices: &[usize],
    repair: EglRect,
    unclipped_visual_groups: &[oblivion_one::compositor::VisualGroupId],
    decisions: &mut Vec<EglVisibilityDecision>,
) -> EglVisibilityPlanStats {
    decisions.clear();
    decisions.resize(commands.len(), EglVisibilityDecision::Occluded);
    let mut stats = EglVisibilityPlanStats::default();
    for &index in command_indices {
        let Some(command) = commands.get(index) else {
            continue;
        };
        stats.commands_visited = stats.commands_visited.saturating_add(1);
        let visible_bounds = if command
            .visual_group
            .is_some_and(|group| unclipped_visual_groups.contains(&group))
        {
            Some(command.bounds)
        } else {
            command.visible_bounds()
        };
        if visible_bounds
            .and_then(|bounds| repair.intersection(bounds))
            .is_none()
        {
            stats.commands_rejected_outside_remaining =
                stats.commands_rejected_outside_remaining.saturating_add(1);
            decisions[index] = EglVisibilityDecision::OutsideRemaining;
            continue;
        }
        decisions[index] = EglVisibilityDecision::Drawable;
        stats.commands_drawable = stats.commands_drawable.saturating_add(1);
    }
    stats.commands_rejected_occluded = decisions
        .iter()
        .filter(|decision| **decision == EglVisibilityDecision::Occluded)
        .count();
    stats.peak_region_pieces = usize::from(stats.commands_drawable > 0);
    stats
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct SurfaceConsumerPlan {
    surface_ids: Vec<u32>,
}

impl SurfaceConsumerPlan {
    pub(crate) fn surface_ids(&self) -> &[u32] {
        &self.surface_ids
    }

    pub(crate) fn add_surface(&mut self, surface_id: u32) {
        self.surface_ids.push(surface_id);
    }

    pub(crate) fn extend(&mut self, other: &Self) {
        self.surface_ids.extend_from_slice(&other.surface_ids);
    }

    pub(crate) fn finish(&mut self) {
        self.surface_ids.sort_unstable();
        self.surface_ids.dedup();
    }
}

pub(super) fn plan_surface_consumers(
    commands: &[EglDrawCommand],
    repairs: &[super::OutputRect],
) -> SurfaceConsumerPlan {
    let mut plan = SurfaceConsumerPlan::default();
    let mut decisions = Vec::new();
    for repair in repairs {
        let repair = EglRect::new(
            repair.x as f32,
            repair.y as f32,
            repair.width as f32,
            repair.height as f32,
        );
        plan_visibility(commands, repair, &mut decisions);
        for (command, decision) in commands.iter().zip(&decisions) {
            if *decision == EglVisibilityDecision::Drawable
                && let EglDrawLayer::Surface(surface_id) = command.layer
            {
                plan.add_surface(surface_id);
            }
        }
    }
    plan.finish();
    plan
}

pub(crate) fn add_surface_consumers_for_command_range(
    plan: &mut SurfaceConsumerPlan,
    commands: &[EglDrawCommand],
    start: usize,
    end: usize,
    repairs: &[super::OutputRect],
) {
    for command in commands
        .get(start.min(commands.len())..end.min(commands.len()))
        .into_iter()
        .flatten()
    {
        if repairs
            .iter()
            .any(|repair| command.bounds.intersects_output_rect(*repair))
            && let EglDrawLayer::Surface(surface_id) = command.layer
        {
            plan.add_surface(surface_id);
        }
    }
}

pub(crate) fn add_surface_consumers_for_capture_indices(
    plan: &mut SurfaceConsumerPlan,
    commands: &[EglDrawCommand],
    command_indices: &[usize],
    repairs: &[super::OutputRect],
) {
    for &index in command_indices {
        let Some(command) = commands.get(index) else {
            continue;
        };
        if repairs
            .iter()
            .any(|repair| command.bounds.intersects_output_rect(*repair))
            && let EglDrawLayer::Surface(surface_id) = command.layer
        {
            plan.add_surface(surface_id);
        }
    }
}

fn subtract_rect(source: EglRect, excluded: EglRect, pieces: &mut [Option<EglRect>; 4]) -> usize {
    let Some(intersection) = source.intersection(excluded) else {
        pieces[0] = Some(source);
        return 1;
    };
    let source_right = source.x + source.width;
    let source_bottom = source.y + source.height;
    let intersection_right = intersection.x + intersection.width;
    let intersection_bottom = intersection.y + intersection.height;
    let mut count = 0;
    if source.y < intersection.y {
        pieces[count] = Some(EglRect::new(
            source.x,
            source.y,
            source.width,
            intersection.y - source.y,
        ));
        count += 1;
    }
    if intersection_bottom < source_bottom {
        pieces[count] = Some(EglRect::new(
            source.x,
            intersection_bottom,
            source.width,
            source_bottom - intersection_bottom,
        ));
        count += 1;
    }
    if source.x < intersection.x {
        pieces[count] = Some(EglRect::new(
            source.x,
            intersection.y,
            intersection.x - source.x,
            intersection.height,
        ));
        count += 1;
    }
    if intersection_right < source_right {
        pieces[count] = Some(EglRect::new(
            intersection_right,
            intersection.y,
            source_right - intersection_right,
            intersection.height,
        ));
        count += 1;
    }
    count
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct EglUvRect {
    top_left: [f32; 2],
    bottom_left: [f32; 2],
    bottom_right: [f32; 2],
    top_right: [f32; 2],
}

impl EglUvRect {
    const FULL: Self = Self {
        top_left: [0.0, 0.0],
        bottom_left: [0.0, 1.0],
        bottom_right: [1.0, 1.0],
        top_right: [1.0, 0.0],
    };

    pub(super) const fn new(left: f32, top: f32, right: f32, bottom: f32) -> Self {
        Self {
            top_left: [left, top],
            bottom_left: [left, bottom],
            bottom_right: [right, bottom],
            top_right: [right, top],
        }
    }

    pub(super) const fn from_surface_uv_quad(
        quad: oblivion_one::compositor::SurfaceUvQuad,
    ) -> Self {
        Self {
            top_left: quad.top_left,
            bottom_left: quad.bottom_left,
            bottom_right: quad.bottom_right,
            top_right: quad.top_right,
        }
    }

    pub(super) fn sample(self, u: f32, v: f32) -> [f32; 2] {
        let top = [
            self.top_left[0] + (self.top_right[0] - self.top_left[0]) * u,
            self.top_left[1] + (self.top_right[1] - self.top_left[1]) * u,
        ];
        let bottom = [
            self.bottom_left[0] + (self.bottom_right[0] - self.bottom_left[0]) * u,
            self.bottom_left[1] + (self.bottom_right[1] - self.bottom_left[1]) * u,
        ];
        [
            top[0] + (bottom[0] - top[0]) * v,
            top[1] + (bottom[1] - top[1]) * v,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EglDrawLayer {
    Solid(ServerFrameColor),
    SolidRgba(u32),
    DecorationAsset(u64),
    Surface(u32),
    LifecycleResolvedVisual(oblivion_one::compositor::PresentationRetainedVisualPayloadId),
    Cursor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SurfaceSampling {
    ExactNearest,
    ScaledLinear,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct EglDrawCommand {
    pub(super) layer: EglDrawLayer,
    pub(super) visual_group: Option<oblivion_one::compositor::VisualGroupId>,
    pub(super) bounds: EglRect,
    pub(super) opaque_regions: Vec<EglRect>,
    pub(super) presentation_clip: Option<EglRect>,
    pub(super) vertex_start: u32,
    pub(super) vertex_count: u32,
    pub(super) sampling: SurfaceSampling,
}

impl EglDrawCommand {
    fn visible_bounds(&self) -> Option<EglRect> {
        self.presentation_clip
            .map_or(Some(self.bounds), |clip| self.bounds.intersection(clip))
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(super) struct EglTexturedVertex {
    pub(super) position: [f32; 2],
    pub(super) uv: [f32; 2],
}

unsafe impl bytemuck::Zeroable for EglTexturedVertex {}
unsafe impl bytemuck::Pod for EglTexturedVertex {}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(super) struct EglLampVertex {
    pub(super) position: [f32; 2],
    pub(super) uv: [f32; 2],
}

unsafe impl bytemuck::Zeroable for EglLampVertex {}
unsafe impl bytemuck::Pod for EglLampVertex {}

#[derive(Debug, Clone, Copy)]
pub(super) struct EglLampDrawCommand {
    pub(super) layer: EglDrawLayer,
    pub(super) bounds: EglRect,
    pub(super) vertex_start: u32,
    pub(super) vertex_count: u32,
    pub(super) sampling: SurfaceSampling,
    pub(super) presentation_identity:
        oblivion_one::presentation_animation::PresentationRetainedVisualIdentity,
}

pub(super) fn push_draw_command(
    vertices: &mut Vec<EglTexturedVertex>,
    commands: &mut Vec<EglDrawCommand>,
    layer: EglDrawLayer,
    rect: EglRect,
    output_width: u32,
    output_height: u32,
    framebuffer_origin: OutputFramebufferOrigin,
) {
    push_draw_command_with_uv(
        vertices,
        commands,
        layer,
        rect,
        EglUvRect::FULL,
        SurfaceSampling::ScaledLinear,
        output_width,
        output_height,
        framebuffer_origin,
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn push_draw_command_with_uv(
    vertices: &mut Vec<EglTexturedVertex>,
    commands: &mut Vec<EglDrawCommand>,
    layer: EglDrawLayer,
    rect: EglRect,
    uv: EglUvRect,
    sampling: SurfaceSampling,
    output_width: u32,
    output_height: u32,
    framebuffer_origin: OutputFramebufferOrigin,
) {
    push_draw_command_with_quad(
        vertices,
        commands,
        layer,
        rect,
        uv,
        sampling,
        output_width,
        output_height,
        framebuffer_origin,
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn push_draw_command_with_quad(
    vertices: &mut Vec<EglTexturedVertex>,
    commands: &mut Vec<EglDrawCommand>,
    layer: EglDrawLayer,
    rect: EglRect,
    uv: EglUvRect,
    sampling: SurfaceSampling,
    output_width: u32,
    output_height: u32,
    framebuffer_origin: OutputFramebufferOrigin,
) {
    let vertex_start = vertices.len() as u32;
    push_textured_quad(
        vertices,
        rect,
        uv,
        output_width,
        output_height,
        framebuffer_origin,
    );
    let vertex_count = vertices.len() as u32 - vertex_start;
    if vertex_count > 0 {
        commands.push(EglDrawCommand {
            layer,
            visual_group: None,
            bounds: rect,
            opaque_regions: Vec::new(),
            presentation_clip: None,
            vertex_start,
            vertex_count,
            sampling,
        });
    }
}

pub(super) fn surface_sampling_for_plan(
    source_width: u32,
    source_height: u32,
    _target_x: i32,
    _target_y: i32,
    target_width: u32,
    target_height: u32,
    uv: EglUvRect,
) -> SurfaceSampling {
    let points = [uv.top_left, uv.bottom_left, uv.bottom_right, uv.top_right];
    let source_left = points
        .iter()
        .map(|point| f64::from(point[0]) * f64::from(source_width))
        .fold(f64::INFINITY, f64::min);
    let source_top = points
        .iter()
        .map(|point| f64::from(point[1]) * f64::from(source_height))
        .fold(f64::INFINITY, f64::min);
    let source_right = points
        .iter()
        .map(|point| f64::from(point[0]) * f64::from(source_width))
        .fold(f64::NEG_INFINITY, f64::max);
    let source_bottom = points
        .iter()
        .map(|point| f64::from(point[1]) * f64::from(source_height))
        .fold(f64::NEG_INFINITY, f64::max);
    let horizontal_extent = edge_extent(uv.top_left, uv.top_right, source_width, source_height);
    let vertical_extent = edge_extent(uv.top_left, uv.bottom_left, source_width, source_height);
    const PIXEL_TOLERANCE: f64 = 0.0001;
    let pixel_aligned = |value: f64| (value - value.round()).abs() <= PIXEL_TOLERANCE;
    let one_to_one_crop = pixel_aligned(source_left)
        && pixel_aligned(source_top)
        && pixel_aligned(source_right)
        && pixel_aligned(source_bottom)
        && (horizontal_extent - f64::from(target_width)).abs() <= PIXEL_TOLERANCE
        && (vertical_extent - f64::from(target_height)).abs() <= PIXEL_TOLERANCE
        && target_width > 0
        && target_height > 0;
    if one_to_one_crop {
        SurfaceSampling::ExactNearest
    } else {
        SurfaceSampling::ScaledLinear
    }
}

fn edge_extent(first: [f32; 2], second: [f32; 2], width: u32, height: u32) -> f64 {
    (f64::from(first[0] - second[0]).abs() * f64::from(width))
        + (f64::from(first[1] - second[1]).abs() * f64::from(height))
}

fn push_textured_quad(
    vertices: &mut Vec<EglTexturedVertex>,
    rect: EglRect,
    uv: EglUvRect,
    output_width: u32,
    output_height: u32,
    framebuffer_origin: OutputFramebufferOrigin,
) {
    if rect.width <= 0.0 || rect.height <= 0.0 || output_width == 0 || output_height == 0 {
        return;
    }

    let output_width = output_width as f32;
    let output_height = output_height as f32;
    let left = rect.x / output_width * 2.0 - 1.0;
    let right = (rect.x + rect.width) / output_width * 2.0 - 1.0;
    let (top, bottom) = match framebuffer_origin {
        OutputFramebufferOrigin::BottomLeft => (
            1.0 - rect.y / output_height * 2.0,
            1.0 - (rect.y + rect.height) / output_height * 2.0,
        ),
        OutputFramebufferOrigin::TopLeftScanout => (
            rect.y / output_height * 2.0 - 1.0,
            (rect.y + rect.height) / output_height * 2.0 - 1.0,
        ),
    };

    vertices.extend_from_slice(&[
        EglTexturedVertex {
            position: [left, top],
            uv: uv.top_left,
        },
        EglTexturedVertex {
            position: [left, bottom],
            uv: uv.bottom_left,
        },
        EglTexturedVertex {
            position: [right, bottom],
            uv: uv.bottom_right,
        },
        EglTexturedVertex {
            position: [left, top],
            uv: uv.top_left,
        },
        EglTexturedVertex {
            position: [right, bottom],
            uv: uv.bottom_right,
        },
        EglTexturedVertex {
            position: [right, top],
            uv: uv.top_right,
        },
    ]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::egl_renderer::{OutputFramebufferOrigin, OutputRect};

    fn quad_vertices(rect: EglRect, origin: OutputFramebufferOrigin) -> Vec<EglTexturedVertex> {
        let mut vertices = Vec::new();
        push_textured_quad(&mut vertices, rect, EglUvRect::FULL, 20, 100, origin);
        vertices
    }

    fn assert_y_bounds(vertices: &[EglTexturedVertex], top: f32, bottom: f32) {
        assert_eq!(vertices.len(), 6);
        assert!((vertices[0].position[1] - top).abs() < f32::EPSILON);
        assert!((vertices[1].position[1] - bottom).abs() < f32::EPSILON);
    }

    #[test]
    fn bottom_left_origin_maps_logical_top_using_legacy_ndc() {
        let vertices = quad_vertices(
            EglRect::new(0.0, 0.0, 20.0, 10.0),
            OutputFramebufferOrigin::BottomLeft,
        );

        assert_y_bounds(&vertices, 1.0, 0.8);
    }

    #[test]
    fn top_left_scanout_origin_maps_logical_top_to_first_rows() {
        let vertices = quad_vertices(
            EglRect::new(0.0, 0.0, 20.0, 10.0),
            OutputFramebufferOrigin::TopLeftScanout,
        );

        assert_y_bounds(&vertices, -1.0, -0.8);
    }

    #[test]
    fn bottom_left_origin_maps_logical_bottom_using_legacy_ndc() {
        let vertices = quad_vertices(
            EglRect::new(0.0, 90.0, 20.0, 10.0),
            OutputFramebufferOrigin::BottomLeft,
        );

        assert_y_bounds(&vertices, -0.8, -1.0);
    }

    #[test]
    fn top_left_scanout_origin_maps_logical_bottom_to_last_rows() {
        let vertices = quad_vertices(
            EglRect::new(0.0, 90.0, 20.0, 10.0),
            OutputFramebufferOrigin::TopLeftScanout,
        );

        assert_y_bounds(&vertices, 0.8, 1.0);
    }

    #[test]
    fn full_buffer_uses_nearest_sampling() {
        assert_eq!(
            surface_sampling_for_plan(800, 600, 0, 0, 800, 600, EglUvRect::FULL),
            SurfaceSampling::ExactNearest
        );
    }

    #[test]
    fn integer_aligned_crop_uses_nearest_sampling() {
        assert_eq!(
            surface_sampling_for_plan(
                800,
                600,
                120,
                100,
                620,
                480,
                EglUvRect::new(0.15, 1.0 / 6.0, 0.925, 0.9666667),
            ),
            SurfaceSampling::ExactNearest
        );
    }

    #[test]
    fn actual_scaling_and_fractional_crops_use_linear_sampling() {
        assert_eq!(
            surface_sampling_for_plan(800, 600, 0, 0, 801, 600, EglUvRect::FULL),
            SurfaceSampling::ScaledLinear
        );
        assert_eq!(
            surface_sampling_for_plan(
                800,
                600,
                0,
                0,
                620,
                480,
                EglUvRect::new(0.1505, 1.0 / 6.0, 0.925, 0.9666667),
            ),
            SurfaceSampling::ScaledLinear
        );
    }

    #[test]
    fn oriented_surface_uv_quad_is_emitted_at_all_four_output_corners() {
        let uv = EglUvRect {
            top_left: [1.0, 0.0],
            bottom_left: [0.0, 0.0],
            bottom_right: [0.0, 1.0],
            top_right: [1.0, 1.0],
        };
        let mut vertices = Vec::new();
        push_textured_quad(
            &mut vertices,
            EglRect::new(0.0, 0.0, 20.0, 10.0),
            uv,
            20,
            10,
            OutputFramebufferOrigin::TopLeftScanout,
        );

        assert_eq!(vertices.len(), 6);
        assert_eq!(vertices[0].uv, uv.top_left);
        assert_eq!(vertices[1].uv, uv.bottom_left);
        assert_eq!(vertices[2].uv, uv.bottom_right);
        assert_eq!(vertices[5].uv, uv.top_right);
    }

    #[test]
    fn all_buffer_transforms_reach_egl_quad_corners() {
        let transforms = [
            wayland_server::protocol::wl_output::Transform::Normal,
            wayland_server::protocol::wl_output::Transform::_90,
            wayland_server::protocol::wl_output::Transform::_180,
            wayland_server::protocol::wl_output::Transform::_270,
            wayland_server::protocol::wl_output::Transform::Flipped,
            wayland_server::protocol::wl_output::Transform::Flipped90,
            wayland_server::protocol::wl_output::Transform::Flipped180,
            wayland_server::protocol::wl_output::Transform::Flipped270,
        ];
        let raw_size = oblivion_one::render_backend::buffer::BufferSize::new(3, 2).unwrap();
        for transform in transforms {
            let quad = oblivion_one::compositor::SurfaceBufferMapping::new(
                raw_size, 1, transform, None, None,
            )
            .unwrap()
            .source_uv_quad()
            .unwrap();
            let uv = EglUvRect::from_surface_uv_quad(quad);
            let mut vertices = Vec::new();
            push_textured_quad(
                &mut vertices,
                EglRect::new(0.0, 0.0, 20.0, 20.0),
                uv,
                20,
                20,
                OutputFramebufferOrigin::TopLeftScanout,
            );

            assert_eq!(vertices[0].uv, uv.top_left, "transform {transform:?}");
            assert_eq!(vertices[1].uv, uv.bottom_left, "transform {transform:?}");
            assert_eq!(vertices[2].uv, uv.bottom_right, "transform {transform:?}");
            assert_eq!(vertices[5].uv, uv.top_right, "transform {transform:?}");
        }
    }

    #[test]
    fn draw_command_bounds_intersect_only_repaired_output_area() {
        let command = EglRect::new(20.0, 30.0, 40.0, 50.0);
        assert!(command.intersects_output_rect(OutputRect::new(0, 0, 25, 40)));
        assert!(!command.intersects_output_rect(OutputRect::new(0, 0, 19, 29)));
    }

    fn test_command(bounds: EglRect, opaque_regions: Vec<EglRect>) -> EglDrawCommand {
        EglDrawCommand {
            layer: EglDrawLayer::Solid(ServerFrameColor::OutputBackground),
            visual_group: None,
            bounds,
            opaque_regions,
            presentation_clip: None,
            vertex_start: 0,
            vertex_count: 6,
            sampling: SurfaceSampling::ScaledLinear,
        }
    }

    #[test]
    fn visibility_planner_accumulates_opaque_repair_coverage() {
        let commands = vec![
            test_command(EglRect::new(0.0, 0.0, 100.0, 100.0), Vec::new()),
            test_command(
                EglRect::new(0.0, 0.0, 50.0, 100.0),
                vec![EglRect::new(0.0, 0.0, 50.0, 100.0)],
            ),
            test_command(
                EglRect::new(50.0, 0.0, 50.0, 100.0),
                vec![EglRect::new(50.0, 0.0, 50.0, 100.0)],
            ),
        ];
        let mut decisions = Vec::new();

        let stats = plan_visibility(
            &commands,
            EglRect::new(10.0, 10.0, 80.0, 80.0),
            &mut decisions,
        );

        assert_eq!(
            decisions,
            vec![
                EglVisibilityDecision::Occluded,
                EglVisibilityDecision::Drawable,
                EglVisibilityDecision::Drawable,
            ]
        );
        assert_eq!(stats.commands_visited, 2);
        assert_eq!(stats.commands_drawable, 2);
        assert_eq!(stats.commands_rejected_occluded, 1);
        assert!(stats.early_terminated);
    }

    #[test]
    fn visibility_planner_uses_only_the_repaired_part_of_large_commands() {
        let commands = vec![
            test_command(EglRect::new(0.0, 0.0, 100.0, 100.0), Vec::new()),
            test_command(
                EglRect::new(20.0, 20.0, 10.0, 10.0),
                vec![EglRect::new(20.0, 20.0, 10.0, 10.0)],
            ),
        ];
        let mut decisions = Vec::new();

        let _ = plan_visibility(
            &commands,
            EglRect::new(20.0, 20.0, 10.0, 10.0),
            &mut decisions,
        );

        assert_eq!(
            decisions,
            vec![
                EglVisibilityDecision::Occluded,
                EglVisibilityDecision::Drawable,
            ]
        );
    }

    #[test]
    fn visibility_planner_keeps_lower_content_for_transparent_upper_commands() {
        let commands = vec![
            test_command(EglRect::new(0.0, 0.0, 100.0, 100.0), Vec::new()),
            test_command(EglRect::new(0.0, 0.0, 100.0, 100.0), Vec::new()),
        ];
        let mut decisions = Vec::new();

        let _ = plan_visibility(
            &commands,
            EglRect::new(0.0, 0.0, 100.0, 100.0),
            &mut decisions,
        );

        assert_eq!(
            decisions,
            vec![
                EglVisibilityDecision::Drawable,
                EglVisibilityDecision::Drawable
            ]
        );
    }

    #[test]
    fn capture_visibility_does_not_apply_final_scene_occlusion() {
        let commands = vec![
            test_command(EglRect::new(0.0, 0.0, 100.0, 100.0), Vec::new()),
            test_command(
                EglRect::new(0.0, 0.0, 100.0, 100.0),
                vec![EglRect::new(0.0, 0.0, 100.0, 100.0)],
            ),
        ];
        let mut decisions = Vec::new();
        let stats = plan_capture_visibility(
            &commands,
            &[0],
            EglRect::new(0.0, 0.0, 100.0, 100.0),
            &[],
            &mut decisions,
        );
        assert_eq!(
            decisions,
            vec![
                EglVisibilityDecision::Drawable,
                EglVisibilityDecision::Occluded
            ]
        );
        assert_eq!(stats.commands_drawable, 1);
    }

    #[test]
    fn presentation_clip_limits_visibility_and_opaque_occlusion() {
        let lower = test_command(EglRect::new(0.0, 0.0, 100.0, 100.0), Vec::new());
        let mut upper = test_command(
            EglRect::new(0.0, 0.0, 100.0, 100.0),
            vec![EglRect::new(0.0, 0.0, 100.0, 100.0)],
        );
        upper.presentation_clip = Some(EglRect::new(0.0, 0.0, 50.0, 100.0));
        let mut decisions = Vec::new();

        plan_visibility(
            &[lower, upper],
            EglRect::new(0.0, 0.0, 100.0, 100.0),
            &mut decisions,
        );

        assert_eq!(decisions[0], EglVisibilityDecision::Drawable);
        assert_eq!(decisions[1], EglVisibilityDecision::Drawable);
    }

    #[test]
    fn same_owner_capture_bypasses_only_that_owners_final_clip() {
        let same_owner = oblivion_one::compositor::VisualGroupId::new(1).expect("group");
        let other_owner = oblivion_one::compositor::VisualGroupId::new(2).expect("group");
        let mut own = test_command(EglRect::new(0.0, 0.0, 100.0, 100.0), Vec::new());
        own.visual_group = Some(same_owner);
        own.presentation_clip = Some(EglRect::new(0.0, 0.0, 50.0, 100.0));
        let mut other = test_command(EglRect::new(0.0, 0.0, 100.0, 100.0), Vec::new());
        other.visual_group = Some(other_owner);
        other.presentation_clip = Some(EglRect::new(0.0, 0.0, 50.0, 100.0));
        let mut decisions = Vec::new();

        plan_capture_visibility(
            &[own, other],
            &[0, 1],
            EglRect::new(60.0, 0.0, 10.0, 10.0),
            &[same_owner],
            &mut decisions,
        );

        assert_eq!(decisions[0], EglVisibilityDecision::Drawable);
        assert_eq!(decisions[1], EglVisibilityDecision::OutsideRemaining);
    }

    #[test]
    fn surface_consumer_planner_discards_zero_area_clips() {
        let mut hidden = test_command(EglRect::new(0.0, 0.0, 100.0, 100.0), Vec::new());
        hidden.layer = EglDrawLayer::Surface(9);
        hidden.presentation_clip = Some(EglRect::new(10.0, 10.0, 0.0, 20.0));
        let plan = plan_surface_consumers(&[hidden], &[OutputRect::new(0, 0, 100, 100)]);

        assert!(plan.surface_ids().is_empty());
    }

    #[test]
    fn surface_consumer_plan_excludes_occluded_and_outside_surfaces() {
        let commands = vec![
            EglDrawCommand {
                layer: EglDrawLayer::Surface(1),
                visual_group: None,
                bounds: EglRect::new(0.0, 0.0, 100.0, 100.0),
                opaque_regions: Vec::new(),
                presentation_clip: None,
                vertex_start: 0,
                vertex_count: 6,
                sampling: SurfaceSampling::ScaledLinear,
            },
            EglDrawCommand {
                layer: EglDrawLayer::Surface(2),
                visual_group: None,
                bounds: EglRect::new(0.0, 0.0, 100.0, 100.0),
                opaque_regions: vec![EglRect::new(0.0, 0.0, 100.0, 100.0)],
                presentation_clip: None,
                vertex_start: 0,
                vertex_count: 6,
                sampling: SurfaceSampling::ScaledLinear,
            },
            EglDrawCommand {
                layer: EglDrawLayer::Surface(3),
                visual_group: None,
                bounds: EglRect::new(200.0, 0.0, 10.0, 10.0),
                opaque_regions: Vec::new(),
                presentation_clip: None,
                vertex_start: 0,
                vertex_count: 6,
                sampling: SurfaceSampling::ScaledLinear,
            },
        ];

        let plan = plan_surface_consumers(&commands, &[OutputRect::new(0, 0, 100, 100)]);

        assert_eq!(plan.surface_ids(), &[2]);
    }

    #[test]
    fn surface_consumer_plan_keeps_partially_visible_surface() {
        let commands = vec![
            EglDrawCommand {
                layer: EglDrawLayer::Surface(1),
                visual_group: None,
                bounds: EglRect::new(0.0, 0.0, 100.0, 100.0),
                opaque_regions: Vec::new(),
                presentation_clip: None,
                vertex_start: 0,
                vertex_count: 6,
                sampling: SurfaceSampling::ScaledLinear,
            },
            EglDrawCommand {
                layer: EglDrawLayer::Surface(2),
                visual_group: None,
                bounds: EglRect::new(0.0, 0.0, 50.0, 100.0),
                opaque_regions: vec![EglRect::new(0.0, 0.0, 50.0, 100.0)],
                presentation_clip: None,
                vertex_start: 0,
                vertex_count: 6,
                sampling: SurfaceSampling::ScaledLinear,
            },
        ];

        let plan = plan_surface_consumers(&commands, &[OutputRect::new(0, 0, 100, 100)]);

        assert_eq!(plan.surface_ids(), &[1, 2]);
    }

    #[test]
    fn surface_consumer_plan_inherits_visibility_overflow_fallback() {
        let mut opaque_regions = Vec::new();
        for row in 0..6 {
            for column in 0..6 {
                opaque_regions.push(EglRect::new(
                    column as f32 * 16.0,
                    row as f32 * 16.0,
                    8.0,
                    8.0,
                ));
            }
        }
        let commands = vec![
            EglDrawCommand {
                layer: EglDrawLayer::Surface(1),
                visual_group: None,
                bounds: EglRect::new(0.0, 0.0, 100.0, 100.0),
                opaque_regions: Vec::new(),
                presentation_clip: None,
                vertex_start: 0,
                vertex_count: 6,
                sampling: SurfaceSampling::ScaledLinear,
            },
            EglDrawCommand {
                layer: EglDrawLayer::Surface(2),
                visual_group: None,
                bounds: EglRect::new(0.0, 0.0, 100.0, 100.0),
                opaque_regions,
                presentation_clip: None,
                vertex_start: 0,
                vertex_count: 6,
                sampling: SurfaceSampling::ScaledLinear,
            },
        ];

        let plan = plan_surface_consumers(&commands, &[OutputRect::new(0, 0, 100, 100)]);

        assert_eq!(plan.surface_ids(), &[1, 2]);
    }

    #[test]
    fn visibility_planner_falls_back_to_overdraw_on_region_fragmentation() {
        let mut opaque_regions = Vec::new();
        for row in 0..6 {
            for column in 0..6 {
                opaque_regions.push(EglRect::new(
                    column as f32 * 16.0,
                    row as f32 * 16.0,
                    8.0,
                    8.0,
                ));
            }
        }
        let commands = vec![
            test_command(EglRect::new(0.0, 0.0, 100.0, 100.0), Vec::new()),
            test_command(EglRect::new(0.0, 0.0, 100.0, 100.0), opaque_regions),
        ];
        let mut decisions = Vec::new();

        let stats = plan_visibility(
            &commands,
            EglRect::new(0.0, 0.0, 100.0, 100.0),
            &mut decisions,
        );

        assert!(stats.overflow_fallback);
        assert_eq!(decisions, vec![EglVisibilityDecision::Drawable; 2]);
    }

    #[test]
    fn visibility_planner_visits_each_command_once() {
        let commands = (0..100)
            .map(|index| test_command(EglRect::new(index as f32, 0.0, 1.0, 1.0), Vec::new()))
            .collect::<Vec<_>>();
        let mut decisions = Vec::new();

        let stats = plan_visibility(
            &commands,
            EglRect::new(0.0, 0.0, 100.0, 1.0),
            &mut decisions,
        );

        assert_eq!(stats.commands_visited, commands.len());
        assert_eq!(stats.commands_drawable, commands.len());
    }

    #[test]
    fn fullscreen_scanout_quad_changes_only_position_orientation() {
        let legacy = quad_vertices(
            EglRect::new(0.0, 0.0, 20.0, 100.0),
            OutputFramebufferOrigin::BottomLeft,
        );
        let scanout = quad_vertices(
            EglRect::new(0.0, 0.0, 20.0, 100.0),
            OutputFramebufferOrigin::TopLeftScanout,
        );

        assert_eq!(legacy[0].position, [-1.0, 1.0]);
        assert_eq!(legacy[1].position, [-1.0, -1.0]);
        assert_eq!(scanout[0].position, [-1.0, -1.0]);
        assert_eq!(scanout[1].position, [-1.0, 1.0]);
        assert_eq!(legacy[0].uv, scanout[0].uv);
        assert_eq!(legacy[1].uv, scanout[1].uv);
    }
}
