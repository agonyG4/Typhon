use crate::color::Rgba;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComponentState {
    pub enabled: bool,
    pub hovered: bool,
    pub pressed: bool,
    pub focused: bool,
    pub selected: bool,
}

impl ComponentState {
    pub const fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }

    pub const fn hovered(mut self, hovered: bool) -> Self {
        self.hovered = hovered;
        self
    }

    pub const fn pressed(mut self, pressed: bool) -> Self {
        self.pressed = pressed;
        self
    }

    pub const fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    pub const fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn content_alpha(self, color: Rgba) -> Rgba {
        if self.enabled {
            color
        } else {
            scale_alpha(color, 0.4)
        }
    }

    pub fn surface_alpha(self, color: Rgba) -> Rgba {
        if !self.enabled {
            scale_alpha(color, 0.32)
        } else if self.pressed {
            scale_alpha(color, 0.88)
        } else if self.hovered || self.focused {
            scale_alpha(color, 0.94)
        } else {
            color
        }
    }
}

impl Default for ComponentState {
    fn default() -> Self {
        Self {
            enabled: true,
            hovered: false,
            pressed: false,
            focused: false,
            selected: false,
        }
    }
}

fn scale_alpha(color: Rgba, factor: f32) -> Rgba {
    color.with_alpha(
        (f32::from(color.alpha()) * factor)
            .round()
            .clamp(0.0, 255.0) as u8,
    )
}
