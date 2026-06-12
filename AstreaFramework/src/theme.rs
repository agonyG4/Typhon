use crate::{
    color::Rgba,
    geometry::CornerRadii,
    layout::SpacingTokens,
    material::{BlurProfile, Material, MaterialRole},
    typography::TypographyTokens,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RadiusTokens {
    pub window: CornerRadii,
    pub panel: CornerRadii,
    pub control: CornerRadii,
    pub popup: CornerRadii,
}

impl Default for RadiusTokens {
    fn default() -> Self {
        Self {
            window: CornerRadii::all(24.0),
            panel: CornerRadii::all(20.0),
            control: CornerRadii::all(12.0),
            popup: CornerRadii::all(18.0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AstreaTheme {
    name: &'static str,
    radius: RadiusTokens,
    spacing: SpacingTokens,
    typography: TypographyTokens,
}

impl AstreaTheme {
    pub fn default_dark() -> Self {
        Self {
            name: "Astrea Dark",
            radius: RadiusTokens::default(),
            spacing: SpacingTokens::default(),
            typography: TypographyTokens::default(),
        }
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub const fn radius(&self) -> &RadiusTokens {
        &self.radius
    }

    pub const fn spacing(&self) -> &SpacingTokens {
        &self.spacing
    }

    pub const fn typography(&self) -> &TypographyTokens {
        &self.typography
    }

    pub const fn material(&self, role: MaterialRole) -> Material {
        match role {
            MaterialRole::Window => {
                Material::new(role, Rgba::new(18, 20, 28, 214), BlurProfile::new(24.0))
            }
            MaterialRole::Panel => {
                Material::new(role, Rgba::new(22, 25, 34, 204), BlurProfile::new(20.0))
            }
            MaterialRole::Control => {
                Material::new(role, Rgba::new(44, 50, 64, 232), BlurProfile::new(8.0))
            }
            MaterialRole::Overlay => {
                Material::new(role, Rgba::new(10, 12, 18, 190), BlurProfile::new(30.0))
            }
        }
    }
}

impl Default for AstreaTheme {
    fn default() -> Self {
        Self::default_dark()
    }
}
