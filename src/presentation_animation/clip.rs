/// A finite rectangle in WindowGroup-local canonical logical coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresentationClipRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl PresentationClipRect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Option<Self> {
        (x.is_finite()
            && y.is_finite()
            && width.is_finite()
            && height.is_finite()
            && width >= 0.0
            && height >= 0.0)
            .then_some(Self {
                x,
                y,
                width,
                height,
            })
    }

    pub const fn x(self) -> f64 {
        self.x
    }

    pub const fn y(self) -> f64 {
        self.y
    }

    pub const fn width(self) -> f64 {
        self.width
    }

    pub const fn height(self) -> f64 {
        self.height
    }

    pub const fn is_empty(self) -> bool {
        self.width == 0.0 || self.height == 0.0
    }

    pub fn is_finite(self) -> bool {
        self.x.is_finite()
            && self.y.is_finite()
            && self.width.is_finite()
            && self.height.is_finite()
            && self.width >= 0.0
            && self.height >= 0.0
    }
}

/// Semantic WindowGroup presentation mask. `Unbounded` is the identity value.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum PresentationClip {
    #[default]
    Unbounded,
    Rect(PresentationClipRect),
}

impl PresentationClip {
    pub const fn is_unbounded(self) -> bool {
        matches!(self, Self::Unbounded)
    }

    pub const fn rect(self) -> Option<PresentationClipRect> {
        match self {
            Self::Unbounded => None,
            Self::Rect(rect) => Some(rect),
        }
    }
}
