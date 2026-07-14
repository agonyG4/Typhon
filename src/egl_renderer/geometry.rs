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
    Wallpaper,
    Solid(ServerFrameColor),
    Surface(u32),
    Cursor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SurfaceSampling {
    ExactNearest,
    ScaledLinear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct EglDrawCommand {
    pub(super) layer: EglDrawLayer,
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
            vertex_start,
            vertex_count,
            sampling,
        });
    }
}

pub(super) fn surface_sampling_for_plan(
    source_width: u32,
    source_height: u32,
    target_x: i32,
    target_y: i32,
    target_width: u32,
    target_height: u32,
    uv: EglUvRect,
) -> SurfaceSampling {
    if source_width == target_width
        && source_height == target_height
        && target_x >= 0
        && target_y >= 0
        && uv.left == 0.0
        && uv.top == 0.0
        && uv.right == 1.0
        && uv.bottom == 1.0
    {
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
    use crate::egl_renderer::OutputFramebufferOrigin;

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
