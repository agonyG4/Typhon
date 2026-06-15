use oblivion_one::compositor::ServerFrameColor;

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

    pub(super) fn from_pixel_bounds(
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        texture_width: u32,
        texture_height: u32,
    ) -> Self {
        if texture_width == 0 || texture_height == 0 {
            return Self::FULL;
        }
        let texture_width = texture_width as f32;
        let texture_height = texture_height as f32;
        Self {
            left: x as f32 / texture_width,
            top: y as f32 / texture_height,
            right: x.saturating_add(width) as f32 / texture_width,
            bottom: y.saturating_add(height) as f32 / texture_height,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EglDrawLayer {
    Wallpaper,
    Solid(ServerFrameColor),
    Surface(u32),
    Cursor,
    ShellOverlay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct EglDrawCommand {
    pub(super) layer: EglDrawLayer,
    pub(super) vertex_start: u32,
    pub(super) vertex_count: u32,
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
) {
    push_draw_command_with_uv(
        vertices,
        commands,
        layer,
        rect,
        EglUvRect::FULL,
        output_width,
        output_height,
    );
}

pub(super) fn push_draw_command_with_uv(
    vertices: &mut Vec<EglTexturedVertex>,
    commands: &mut Vec<EglDrawCommand>,
    layer: EglDrawLayer,
    rect: EglRect,
    uv: EglUvRect,
    output_width: u32,
    output_height: u32,
) {
    let vertex_start = vertices.len() as u32;
    push_textured_quad(vertices, rect, uv, output_width, output_height);
    let vertex_count = vertices.len() as u32 - vertex_start;
    if vertex_count > 0 {
        commands.push(EglDrawCommand {
            layer,
            vertex_start,
            vertex_count,
        });
    }
}

fn push_textured_quad(
    vertices: &mut Vec<EglTexturedVertex>,
    rect: EglRect,
    uv: EglUvRect,
    output_width: u32,
    output_height: u32,
) {
    if rect.width <= 0.0 || rect.height <= 0.0 || output_width == 0 || output_height == 0 {
        return;
    }

    let output_width = output_width as f32;
    let output_height = output_height as f32;
    let left = rect.x / output_width * 2.0 - 1.0;
    let right = (rect.x + rect.width) / output_width * 2.0 - 1.0;
    let top = 1.0 - rect.y / output_height * 2.0;
    let bottom = 1.0 - (rect.y + rect.height) / output_height * 2.0;

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
