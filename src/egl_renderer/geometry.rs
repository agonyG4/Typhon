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

pub(super) fn opaque_regions_cover_rect(regions: &[EglRect], target: EglRect) -> bool {
    let mut remaining = [None; MAX_OPAQUE_COVERAGE_PIECES];
    remaining[0] = Some(target);
    let mut remaining_len = 1;
    for region in regions {
        let mut next = [None; MAX_OPAQUE_COVERAGE_PIECES];
        let mut next_len = 0;
        for piece in remaining.iter().take(remaining_len).flatten() {
            let mut pieces = [None; 4];
            let piece_count = subtract_rect(*piece, *region, &mut pieces);
            for residual in pieces.iter().take(piece_count).flatten() {
                if next_len == MAX_OPAQUE_COVERAGE_PIECES {
                    return false;
                }
                next[next_len] = Some(*residual);
                next_len += 1;
            }
        }
        remaining = next;
        remaining_len = next_len;
        if remaining_len == 0 {
            return true;
        }
    }
    false
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
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}

impl EglUvRect {
    const FULL: Self = Self {
        left: 0.0,
        top: 0.0,
        right: 1.0,
        bottom: 1.0,
    };

    pub(super) const fn new(left: f32, top: f32, right: f32, bottom: f32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EglDrawLayer {
    Solid(ServerFrameColor),
    SolidRgba(u32),
    DecorationAsset(u64),
    Surface(u32),
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
    pub(super) bounds: EglRect,
    pub(super) opaque_regions: Vec<EglRect>,
    pub(super) vertex_start: u32,
    pub(super) vertex_count: u32,
    pub(super) sampling: SurfaceSampling,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(super) struct EglTexturedVertex {
    position: [f32; 2],
    uv: [f32; 2],
}

unsafe impl bytemuck::Zeroable for EglTexturedVertex {}
unsafe impl bytemuck::Pod for EglTexturedVertex {}

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
            bounds: rect,
            opaque_regions: Vec::new(),
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
    let source_left = f64::from(source_width) * f64::from(uv.left);
    let source_top = f64::from(source_height) * f64::from(uv.top);
    let source_right = f64::from(source_width) * f64::from(uv.right);
    let source_bottom = f64::from(source_height) * f64::from(uv.bottom);
    const PIXEL_TOLERANCE: f64 = 0.0001;
    let pixel_aligned = |value: f64| (value - value.round()).abs() <= PIXEL_TOLERANCE;
    let one_to_one_crop = pixel_aligned(source_left)
        && pixel_aligned(source_top)
        && pixel_aligned(source_right)
        && pixel_aligned(source_bottom)
        && (source_right - source_left - f64::from(target_width)).abs() <= PIXEL_TOLERANCE
        && (source_bottom - source_top - f64::from(target_height)).abs() <= PIXEL_TOLERANCE
        && target_width > 0
        && target_height > 0;
    if one_to_one_crop {
        SurfaceSampling::ExactNearest
    } else {
        SurfaceSampling::ScaledLinear
    }
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
            uv: [uv.left, uv.top],
        },
        EglTexturedVertex {
            position: [left, bottom],
            uv: [uv.left, uv.bottom],
        },
        EglTexturedVertex {
            position: [right, bottom],
            uv: [uv.right, uv.bottom],
        },
        EglTexturedVertex {
            position: [left, top],
            uv: [uv.left, uv.top],
        },
        EglTexturedVertex {
            position: [right, bottom],
            uv: [uv.right, uv.bottom],
        },
        EglTexturedVertex {
            position: [right, top],
            uv: [uv.right, uv.top],
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
    fn draw_command_bounds_intersect_only_repaired_output_area() {
        let command = EglRect::new(20.0, 30.0, 40.0, 50.0);
        assert!(command.intersects_output_rect(OutputRect::new(0, 0, 25, 40)));
        assert!(!command.intersects_output_rect(OutputRect::new(0, 0, 19, 29)));
    }

    #[test]
    fn opaque_coverage_respects_partial_regions_and_holes() {
        let target = EglRect::new(0.0, 0.0, 10.0, 10.0);
        let ring = [
            EglRect::new(0.0, 0.0, 10.0, 4.0),
            EglRect::new(0.0, 6.0, 10.0, 4.0),
            EglRect::new(0.0, 4.0, 4.0, 2.0),
            EglRect::new(6.0, 4.0, 4.0, 2.0),
        ];
        assert!(opaque_regions_cover_rect(
            &ring,
            EglRect::new(0.0, 0.0, 4.0, 4.0)
        ));
        assert!(!opaque_regions_cover_rect(&ring, target));
        assert!(!opaque_regions_cover_rect(
            &ring,
            EglRect::new(4.0, 4.0, 2.0, 2.0)
        ));
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
