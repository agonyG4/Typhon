use crate::core::SceneNodeId;

use super::ids::{PresentationRevisionId, PresentationTransactionId, TransitionId};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresentationRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl PresentationRect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Option<Self> {
        let rect = Self {
            x,
            y,
            width,
            height,
        };
        (x.is_finite() && y.is_finite() && width.is_finite() && height.is_finite())
            .then_some(rect)
            .filter(|rect| rect.width > 0.0 && rect.height > 0.0)
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

    pub fn is_identity_with(self, target: Self) -> bool {
        self == target
    }

    #[cfg(test)]
    pub(crate) const fn from_raw_for_test(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PresentationVelocity {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl PresentationVelocity {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
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

    pub(crate) fn component(self, index: usize) -> f64 {
        match index {
            0 => self.x,
            1 => self.y,
            2 => self.width,
            _ => self.height,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresentationGeometryTransform {
    pub canonical_rect: PresentationRect,
    pub presented_rect: PresentationRect,
}

impl PresentationGeometryTransform {
    pub const fn new(canonical_rect: PresentationRect, presented_rect: PresentationRect) -> Self {
        Self {
            canonical_rect,
            presented_rect,
        }
    }

    pub fn map_point(self, point: (f64, f64)) -> (f64, f64) {
        (
            self.presented_rect.x() + (point.0 - self.canonical_rect.x()) * self.scale_x(),
            self.presented_rect.y() + (point.1 - self.canonical_rect.y()) * self.scale_y(),
        )
    }

    pub fn inverse_map_point_unbounded(self, point: (f64, f64)) -> Option<(f64, f64)> {
        if !point.0.is_finite() || !point.1.is_finite() {
            return None;
        }
        let scale_x = self.scale_x();
        let scale_y = self.scale_y();
        if !scale_x.is_finite() || !scale_y.is_finite() || scale_x <= 0.0 || scale_y <= 0.0 {
            return None;
        }
        Some((
            self.canonical_rect.x() + (point.0 - self.presented_rect.x()) / scale_x,
            self.canonical_rect.y() + (point.1 - self.presented_rect.y()) / scale_y,
        ))
    }

    pub fn map_rect(self, rect: PresentationRect) -> Option<PresentationRect> {
        let top_left = self.map_point((rect.x(), rect.y()));
        let bottom_right = self.map_point((rect.x() + rect.width(), rect.y() + rect.height()));
        PresentationRect::new(
            top_left.0,
            top_left.1,
            bottom_right.0 - top_left.0,
            bottom_right.1 - top_left.1,
        )
    }

    pub fn scale_x(self) -> f64 {
        self.presented_rect.width() / self.canonical_rect.width()
    }

    pub fn scale_y(self) -> f64 {
        self.presented_rect.height() / self.canonical_rect.height()
    }

    pub fn is_identity(self) -> bool {
        self.canonical_rect == self.presented_rect
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresentationWindowSample {
    pub scene_node_id: SceneNodeId,
    pub key: u32,
    pub rect: PresentationRect,
    pub velocity: PresentationVelocity,
    pub transaction_id: PresentationTransactionId,
    pub revision_id: PresentationRevisionId,
    pub transition_id: TransitionId,
    pub mathematically_settled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentationDamageRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl PresentationDamageRect {
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

pub fn presentation_damage(
    previous: PresentationRect,
    current: PresentationRect,
) -> PresentationDamageRect {
    let left = previous.x.min(current.x).floor();
    let top = previous.y.min(current.y).floor();
    let right = (previous.x + previous.width)
        .max(current.x + current.width)
        .ceil();
    let bottom = (previous.y + previous.height)
        .max(current.y + current.height)
        .ceil();
    let left_i64 = left.clamp(i32::MIN as f64, i32::MAX as f64) as i64;
    let top_i64 = top.clamp(i32::MIN as f64, i32::MAX as f64) as i64;
    let right_i64 = right.clamp(i32::MIN as f64, i32::MAX as f64) as i64;
    let bottom_i64 = bottom.clamp(i32::MIN as f64, i32::MAX as f64) as i64;
    PresentationDamageRect::new(
        left_i64 as i32,
        top_i64 as i32,
        (right_i64.saturating_sub(left_i64)).min(i64::from(u32::MAX)) as u32,
        (bottom_i64.saturating_sub(top_i64)).min(i64::from(u32::MAX)) as u32,
    )
}
