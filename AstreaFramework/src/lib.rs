pub mod color;
pub mod component;
pub mod geometry;
pub mod layout;
pub mod material;
pub mod motion;
pub mod performance;
pub mod render;
pub mod theme;
pub mod typography;

pub mod prelude {
    pub use crate::{
        color::Rgba,
        component::ComponentState,
        geometry::{CornerRadii, Rect, Size},
        layout::{BoxConstraints, Insets, SpacingTokens},
        material::{BlurProfile, Material, MaterialRole},
        motion::{DurationMs, MotionSpeed},
        performance::FrameBudget,
        render::{DrawCommand, RenderPlan},
        theme::{AstreaTheme, RadiusTokens},
        typography::{FontWeight, TextStyle, TypographyTokens},
    };
}
