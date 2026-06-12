use crate::geometry::{Rect, Size};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Insets {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Insets {
    pub const ZERO: Self = Self::all(0.0);

    pub const fn all(value: f32) -> Self {
        Self {
            top: value,
            right: value,
            bottom: value,
            left: value,
        }
    }

    pub const fn symmetric(horizontal: f32, vertical: f32) -> Self {
        Self {
            top: vertical,
            right: horizontal,
            bottom: vertical,
            left: horizontal,
        }
    }

    pub const fn horizontal(self) -> f32 {
        self.left + self.right
    }

    pub const fn vertical(self) -> f32 {
        self.top + self.bottom
    }

    pub fn inset_rect(self, rect: Rect) -> Rect {
        Rect::new(
            rect.x + self.left,
            rect.y + self.top,
            (rect.width - self.horizontal()).max(0.0),
            (rect.height - self.vertical()).max(0.0),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoxConstraints {
    pub min: Size,
    pub max: Size,
}

impl BoxConstraints {
    pub const fn new(min: Size, max: Size) -> Self {
        Self { min, max }
    }

    pub const fn tight(size: Size) -> Self {
        Self {
            min: size,
            max: size,
        }
    }

    pub fn constrain(self, size: Size) -> Size {
        Size::new(
            size.width.clamp(self.min.width, self.max.width),
            size.height.clamp(self.min.height, self.max.height),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpacingTokens {
    pub xxs: f32,
    pub xs: f32,
    pub sm: f32,
    pub md: f32,
    pub lg: f32,
    pub xl: f32,
    pub xxl: f32,
    pub spotlight_margin: f32,
}

impl Default for SpacingTokens {
    fn default() -> Self {
        Self {
            xxs: 4.0,
            xs: 6.0,
            sm: 8.0,
            md: 12.0,
            lg: 14.0,
            xl: 18.0,
            xxl: 24.0,
            spotlight_margin: 14.0,
        }
    }
}
