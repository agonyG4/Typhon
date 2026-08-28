use std::fmt;

/// A compositor-independent logical rectangle used by the tiling algorithm.
///
/// Dwindle owns logical tile rectangles, not surface placements. Coordinates
/// are signed so a work area can retain an absolute output origin; dimensions
/// are always strictly positive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LayoutRect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

impl LayoutRect {
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Option<Self> {
        if width == 0 || height == 0 {
            return None;
        }
        Some(Self {
            x,
            y,
            width,
            height,
        })
    }

    pub const fn x(self) -> i32 {
        self.x
    }

    pub const fn y(self) -> i32 {
        self.y
    }

    pub const fn width(self) -> u32 {
        self.width
    }

    pub const fn height(self) -> u32 {
        self.height
    }

    pub fn right(self) -> i32 {
        self.x
            .saturating_add(self.width.min(i32::MAX as u32) as i32)
    }

    pub fn bottom(self) -> i32 {
        self.y
            .saturating_add(self.height.min(i32::MAX as u32) as i32)
    }

    /// Returns the one shared pixel boundary for a zero-gap split.
    ///
    /// `Horizontal` is a left/right split and `Vertical` is a top/bottom
    /// split. Rounding happens once here; both child rectangles use the same
    /// resulting boundary.
    pub fn split_boundary(self, axis: SplitAxis, ratio: SplitRatio) -> i32 {
        let origin = match axis {
            SplitAxis::Horizontal => self.x,
            SplitAxis::Vertical => self.y,
        };
        let extent = match axis {
            SplitAxis::Horizontal => self.width,
            SplitAxis::Vertical => self.height,
        };
        let offset = (f64::from(extent) * ratio.value()).round();
        i64::from(origin)
            .saturating_add(offset as i64)
            .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
    }

    pub fn first_child(self, axis: SplitAxis, boundary: i32) -> Option<Self> {
        match axis {
            SplitAxis::Horizontal => {
                let width = boundary
                    .checked_sub(self.x)
                    .and_then(|value| u32::try_from(value).ok())?;
                Self::new(self.x, self.y, width, self.height)
            }
            SplitAxis::Vertical => {
                let height = boundary
                    .checked_sub(self.y)
                    .and_then(|value| u32::try_from(value).ok())?;
                Self::new(self.x, self.y, self.width, height)
            }
        }
    }

    pub fn second_child(self, axis: SplitAxis, boundary: i32) -> Option<Self> {
        match axis {
            SplitAxis::Horizontal => {
                let width = self
                    .right()
                    .checked_sub(boundary)
                    .and_then(|value| u32::try_from(value).ok())?;
                Self::new(boundary, self.y, width, self.height)
            }
            SplitAxis::Vertical => {
                let height = self
                    .bottom()
                    .checked_sub(boundary)
                    .and_then(|value| u32::try_from(value).ok())?;
                Self::new(self.x, boundary, self.width, height)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LayoutPoint {
    x: i32,
    y: i32,
}

impl LayoutPoint {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    pub const fn x(self) -> i32 {
        self.x
    }

    pub const fn y(self) -> i32 {
        self.y
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SplitAxis {
    /// A left/right split.
    Horizontal,
    /// A top/bottom split.
    Vertical,
}

#[derive(Clone, Copy, PartialEq, PartialOrd)]
pub struct SplitRatio(f64);

impl SplitRatio {
    pub const MIN: f64 = 0.05;
    pub const MAX: f64 = 0.95;
    pub const DEFAULT: Self = Self(0.50);

    pub fn new(value: f64) -> Option<Self> {
        value
            .is_finite()
            .then_some(value)
            .filter(|value| (Self::MIN..=Self::MAX).contains(value))
            .map(Self)
    }

    pub const fn value(self) -> f64 {
        self.0
    }
}

impl Default for SplitRatio {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl fmt::Debug for SplitRatio {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("SplitRatio").field(&self.0).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::{LayoutPoint, LayoutRect, SplitAxis, SplitRatio};

    #[test]
    fn layout_rect_requires_positive_dimensions() {
        assert!(LayoutRect::new(0, 0, 1, 1).is_some());
        assert!(LayoutRect::new(0, 0, 0, 1).is_none());
        assert!(LayoutRect::new(0, 0, 1, 0).is_none());
    }

    #[test]
    fn layout_rect_uses_shared_boundaries_for_partitioning() {
        let parent = LayoutRect::new(10, 20, 101, 80).expect("valid parent");
        let boundary = parent.split_boundary(SplitAxis::Horizontal, SplitRatio::DEFAULT);
        let first = parent
            .first_child(SplitAxis::Horizontal, boundary)
            .expect("positive first child");
        let second = parent
            .second_child(SplitAxis::Horizontal, boundary)
            .expect("positive second child");

        assert_eq!(first.right(), second.x());
        assert_eq!(first.width() + second.width(), parent.width());
        assert_eq!(first.height(), parent.height());
        assert_eq!(second.height(), parent.height());
    }

    #[test]
    fn split_ratio_rejects_non_finite_and_unsafe_values() {
        assert!(SplitRatio::new(0.05).is_some());
        assert!(SplitRatio::new(0.95).is_some());
        assert!(SplitRatio::new(0.04).is_none());
        assert!(SplitRatio::new(0.96).is_none());
        assert!(SplitRatio::new(f64::NAN).is_none());
        assert!(SplitRatio::new(f64::INFINITY).is_none());
    }

    #[test]
    fn pointer_points_are_plain_layout_values() {
        let point = LayoutPoint::new(7, 9);
        assert_eq!(point.x(), 7);
        assert_eq!(point.y(), 9);
    }
}
