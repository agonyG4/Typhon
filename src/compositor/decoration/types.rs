#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DecorationMode {
    ClientSide,
    ServerSide,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum DecorationPreference {
    #[default]
    Unset,
    ClientSide,
    ServerSide,
}

impl DecorationPreference {
    pub(crate) const fn effective_mode(
        self,
        has_decoration_object: bool,
        fullscreen: bool,
    ) -> DecorationMode {
        if fullscreen {
            return DecorationMode::None;
        }
        if !has_decoration_object {
            return DecorationMode::ClientSide;
        }
        match self {
            Self::ClientSide => DecorationMode::ClientSide,
            Self::ServerSide | Self::Unset => DecorationMode::ServerSide,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub(crate) struct DecorationExtents {
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
    pub left: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DecorationButtonKind {
    Minimize,
    MaximizeRestore,
    Close,
}

impl DecorationButtonKind {
    pub(crate) const ORDER: [Self; 3] = [Self::Minimize, Self::MaximizeRestore, Self::Close];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DecorationButtonVisualState {
    Normal,
    Hovered,
    Pressed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DecorationResizeEdge {
    Top,
    Right,
    Bottom,
    Left,
    TopRight,
    BottomRight,
    BottomLeft,
    TopLeft,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DecorationHit {
    Resize(DecorationResizeEdge),
    Titlebar,
    Button(DecorationButtonKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DecorationRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl DecorationRect {
    pub(crate) const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub(crate) fn right(self) -> i32 {
        self.x
            .saturating_add(self.width.min(i32::MAX as u32) as i32)
    }

    pub(crate) fn bottom(self) -> i32 {
        self.y
            .saturating_add(self.height.min(i32::MAX as u32) as i32)
    }

    pub(crate) fn contains(self, x: f64, y: f64) -> bool {
        x.is_finite()
            && y.is_finite()
            && x >= f64::from(self.x)
            && y >= f64::from(self.y)
            && x < f64::from(self.right())
            && y < f64::from(self.bottom())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct DecorationMetrics {
    pub titlebar_height: u32,
    pub button_visual_size: u32,
    pub button_spacing: u32,
    pub right_padding: u32,
    pub horizontal_padding: u32,
    pub border_width: u32,
    pub resize_hit_width: u32,
    pub minimum_button_hit_width: u32,
}

impl DecorationMetrics {
    pub(crate) const fn mac_tahoe() -> Self {
        Self {
            titlebar_height: 26,
            button_visual_size: 16,
            button_spacing: 9,
            right_padding: 12,
            horizontal_padding: 12,
            border_width: 0,
            resize_hit_width: 6,
            minimum_button_hit_width: 24,
        }
    }

    pub(crate) fn validate(self) -> bool {
        (20..=96).contains(&self.titlebar_height)
            && (8..=64).contains(&self.button_visual_size)
            && self.button_spacing <= 32
            && self.right_padding <= 64
            && self.horizontal_padding <= 64
            && self.border_width <= 8
            && self.resize_hit_width <= 32
            && self.minimum_button_hit_width >= self.button_visual_size
            && self.minimum_button_hit_width <= 64
    }
}
#[cfg(test)]
mod negotiation_tests {
    use super::{DecorationMode, DecorationPreference};

    #[test]
    fn explicit_preference_and_conservative_default_choose_effective_mode() {
        assert_eq!(
            DecorationPreference::ClientSide.effective_mode(true, false),
            DecorationMode::ClientSide
        );
        assert_eq!(
            DecorationPreference::ServerSide.effective_mode(true, false),
            DecorationMode::ServerSide
        );
        assert_eq!(
            DecorationPreference::Unset.effective_mode(true, false),
            DecorationMode::ServerSide
        );
        assert_eq!(
            DecorationPreference::Unset.effective_mode(false, false),
            DecorationMode::ClientSide
        );
        assert_eq!(
            DecorationPreference::ServerSide.effective_mode(true, true),
            DecorationMode::None
        );
    }
}
