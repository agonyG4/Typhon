use crate::color::Rgba;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MaterialRole {
    Window,
    Panel,
    Control,
    Overlay,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlurProfile {
    radius_px: f32,
}

impl BlurProfile {
    pub const NONE: Self = Self::new(0.0);

    pub const fn new(radius_px: f32) -> Self {
        Self { radius_px }
    }

    pub const fn radius_px(self) -> f32 {
        self.radius_px
    }

    pub const fn is_enabled(self) -> bool {
        self.radius_px > 0.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Material {
    pub role: MaterialRole,
    pub background: Rgba,
    pub blur: BlurProfile,
}

impl Material {
    pub const fn new(role: MaterialRole, background: Rgba, blur: BlurProfile) -> Self {
        Self {
            role,
            background,
            blur,
        }
    }
}
