use crate::color::Rgba;

const ASTREA_FONT_FAMILY: &str = "Inter";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontWeight {
    Light,
    Regular,
    Medium,
    DemiBold,
    Bold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextStyle {
    pub family: &'static str,
    pub pixel_size: u32,
    pub weight: FontWeight,
    pub color: Rgba,
}

impl TextStyle {
    pub const fn new(
        family: &'static str,
        pixel_size: u32,
        weight: FontWeight,
        color: Rgba,
    ) -> Self {
        Self {
            family,
            pixel_size,
            weight,
            color,
        }
    }

    pub const fn with_color(self, color: Rgba) -> Self {
        Self { color, ..self }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypographyTokens {
    pub body: TextStyle,
    pub label: TextStyle,
    pub caption: TextStyle,
    pub spotlight_icon: TextStyle,
    pub spotlight_search: TextStyle,
    pub spotlight_result: TextStyle,
    pub spotlight_weather: TextStyle,
}

impl Default for TypographyTokens {
    fn default() -> Self {
        let white = Rgba::rgb(255, 255, 255);
        let placeholder = Rgba::new(255, 255, 255, 102);
        Self {
            body: TextStyle::new(ASTREA_FONT_FAMILY, 14, FontWeight::Regular, white),
            label: TextStyle::new(ASTREA_FONT_FAMILY, 13, FontWeight::Medium, white),
            caption: TextStyle::new(ASTREA_FONT_FAMILY, 11, FontWeight::Regular, placeholder),
            spotlight_icon: TextStyle::new(
                ASTREA_FONT_FAMILY,
                24,
                FontWeight::Regular,
                placeholder,
            ),
            spotlight_search: TextStyle::new(
                ASTREA_FONT_FAMILY,
                22,
                FontWeight::Light,
                placeholder,
            ),
            spotlight_result: TextStyle::new(ASTREA_FONT_FAMILY, 17, FontWeight::Regular, white),
            spotlight_weather: TextStyle::new(
                ASTREA_FONT_FAMILY,
                18,
                FontWeight::Medium,
                placeholder,
            ),
        }
    }
}
