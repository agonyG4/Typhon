use crate::{
    color::Rgba,
    geometry::{CornerRadii, Rect, Size},
    material::MaterialRole,
    theme::AstreaTheme,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DrawCommand {
    BackdropBlur {
        rect: Rect,
        radius_px: f32,
    },
    RoundedRect {
        rect: Rect,
        radii: CornerRadii,
        color: Rgba,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct RenderPlan {
    viewport: Size,
    commands: Vec<DrawCommand>,
}

impl RenderPlan {
    pub fn new(viewport: Size) -> Self {
        Self {
            viewport,
            commands: Vec::new(),
        }
    }

    pub const fn viewport(&self) -> Size {
        self.viewport
    }

    pub fn commands(&self) -> &[DrawCommand] {
        &self.commands
    }

    pub fn push_material_rect(
        &mut self,
        rect: Rect,
        radii: CornerRadii,
        role: MaterialRole,
        theme: &AstreaTheme,
    ) {
        let material = theme.material(role);
        if material.blur.is_enabled() {
            self.commands.push(DrawCommand::BackdropBlur {
                rect,
                radius_px: material.blur.radius_px(),
            });
        }
        self.commands.push(DrawCommand::RoundedRect {
            rect,
            radii,
            color: material.background,
        });
    }
}
