use wayland_protocols::xdg::shell::server::xdg_positioner;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct XdgWindowGeometry {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) width: i32,
    pub(super) height: i32,
}

impl XdgWindowGeometry {
    pub(super) fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width: width.max(1),
            height: height.max(1),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PopupRect {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) width: i32,
    pub(super) height: i32,
}

impl PopupRect {
    pub(super) fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width: width.max(1),
            height: height.max(1),
        }
    }

    fn constraint_offsets(self, popup: Self) -> (i32, i32, i32, i32) {
        let off_left = self.x.saturating_sub(popup.x);
        let off_right = popup
            .x
            .saturating_add(popup.width)
            .saturating_sub(self.x.saturating_add(self.width));
        let off_top = self.y.saturating_sub(popup.y);
        let off_bottom = popup
            .y
            .saturating_add(popup.height)
            .saturating_sub(self.y.saturating_add(self.height));
        (off_left, off_right, off_top, off_bottom)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct PopupAnchorRect {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) width: i32,
    pub(super) height: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PopupHorizontal {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PopupVertical {
    Top,
    Center,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PopupEdges {
    horizontal: PopupHorizontal,
    vertical: PopupVertical,
}

impl PopupEdges {
    const CENTER: Self = Self {
        horizontal: PopupHorizontal::Center,
        vertical: PopupVertical::Center,
    };

    pub(super) fn from_anchor(anchor: xdg_positioner::Anchor) -> Self {
        match anchor {
            xdg_positioner::Anchor::None => Self::CENTER,
            xdg_positioner::Anchor::Top => Self::new(PopupHorizontal::Center, PopupVertical::Top),
            xdg_positioner::Anchor::Bottom => {
                Self::new(PopupHorizontal::Center, PopupVertical::Bottom)
            }
            xdg_positioner::Anchor::Left => Self::new(PopupHorizontal::Left, PopupVertical::Center),
            xdg_positioner::Anchor::Right => {
                Self::new(PopupHorizontal::Right, PopupVertical::Center)
            }
            xdg_positioner::Anchor::TopLeft => Self::new(PopupHorizontal::Left, PopupVertical::Top),
            xdg_positioner::Anchor::BottomLeft => {
                Self::new(PopupHorizontal::Left, PopupVertical::Bottom)
            }
            xdg_positioner::Anchor::TopRight => {
                Self::new(PopupHorizontal::Right, PopupVertical::Top)
            }
            xdg_positioner::Anchor::BottomRight => {
                Self::new(PopupHorizontal::Right, PopupVertical::Bottom)
            }
            _ => Self::CENTER,
        }
    }

    pub(super) fn from_gravity(gravity: xdg_positioner::Gravity) -> Self {
        match gravity {
            xdg_positioner::Gravity::None => Self::CENTER,
            xdg_positioner::Gravity::Top => Self::new(PopupHorizontal::Center, PopupVertical::Top),
            xdg_positioner::Gravity::Bottom => {
                Self::new(PopupHorizontal::Center, PopupVertical::Bottom)
            }
            xdg_positioner::Gravity::Left => {
                Self::new(PopupHorizontal::Left, PopupVertical::Center)
            }
            xdg_positioner::Gravity::Right => {
                Self::new(PopupHorizontal::Right, PopupVertical::Center)
            }
            xdg_positioner::Gravity::TopLeft => {
                Self::new(PopupHorizontal::Left, PopupVertical::Top)
            }
            xdg_positioner::Gravity::BottomLeft => {
                Self::new(PopupHorizontal::Left, PopupVertical::Bottom)
            }
            xdg_positioner::Gravity::TopRight => {
                Self::new(PopupHorizontal::Right, PopupVertical::Top)
            }
            xdg_positioner::Gravity::BottomRight => {
                Self::new(PopupHorizontal::Right, PopupVertical::Bottom)
            }
            _ => Self::CENTER,
        }
    }

    const fn new(horizontal: PopupHorizontal, vertical: PopupVertical) -> Self {
        Self {
            horizontal,
            vertical,
        }
    }

    fn flip_x(self) -> Self {
        let horizontal = match self.horizontal {
            PopupHorizontal::Left => PopupHorizontal::Right,
            PopupHorizontal::Right => PopupHorizontal::Left,
            PopupHorizontal::Center => PopupHorizontal::Center,
        };
        Self::new(horizontal, self.vertical)
    }

    fn flip_y(self) -> Self {
        let vertical = match self.vertical {
            PopupVertical::Top => PopupVertical::Bottom,
            PopupVertical::Bottom => PopupVertical::Top,
            PopupVertical::Center => PopupVertical::Center,
        };
        Self::new(self.horizontal, vertical)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct PopupConstraintAdjustment {
    flip_x: bool,
    flip_y: bool,
    slide_x: bool,
    slide_y: bool,
    resize_x: bool,
    resize_y: bool,
}

impl PopupConstraintAdjustment {
    pub(super) fn from_xdg(constraint_adjustment: xdg_positioner::ConstraintAdjustment) -> Self {
        Self {
            flip_x: constraint_adjustment.contains(xdg_positioner::ConstraintAdjustment::FlipX),
            flip_y: constraint_adjustment.contains(xdg_positioner::ConstraintAdjustment::FlipY),
            slide_x: constraint_adjustment.contains(xdg_positioner::ConstraintAdjustment::SlideX),
            slide_y: constraint_adjustment.contains(xdg_positioner::ConstraintAdjustment::SlideY),
            resize_x: constraint_adjustment.contains(xdg_positioner::ConstraintAdjustment::ResizeX),
            resize_y: constraint_adjustment.contains(xdg_positioner::ConstraintAdjustment::ResizeY),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct XdgPositionerState {
    pub(super) width: i32,
    pub(super) height: i32,
    pub(super) anchor_rect: PopupAnchorRect,
    pub(super) anchor: PopupEdges,
    pub(super) gravity: PopupEdges,
    pub(super) offset_x: i32,
    pub(super) offset_y: i32,
    pub(super) constraint_adjustment: PopupConstraintAdjustment,
    pub(super) parent_size: Option<(i32, i32)>,
    pub(super) parent_configure: Option<u32>,
    pub(super) reactive: bool,
}

impl XdgPositionerState {
    pub(super) fn is_complete(self) -> bool {
        self.width > 0
            && self.height > 0
            && self.anchor_rect.width > 0
            && self.anchor_rect.height > 0
    }

    fn geometry(self) -> PopupRect {
        PopupRect::new(
            self.geometry_x(),
            self.geometry_y(),
            self.width,
            self.height,
        )
    }

    fn geometry_x(self) -> i32 {
        let anchor_x = match self.anchor.horizontal {
            PopupHorizontal::Left => self.anchor_rect.x,
            PopupHorizontal::Center => self
                .anchor_rect
                .x
                .saturating_add(self.anchor_rect.width / 2),
            PopupHorizontal::Right => self.anchor_rect.x.saturating_add(self.anchor_rect.width),
        };
        let gravity_x = match self.gravity.horizontal {
            PopupHorizontal::Left => -self.width,
            PopupHorizontal::Center => -(self.width / 2),
            PopupHorizontal::Right => 0,
        };
        anchor_x
            .saturating_add(self.offset_x)
            .saturating_add(gravity_x)
    }

    fn geometry_y(self) -> i32 {
        let anchor_y = match self.anchor.vertical {
            PopupVertical::Top => self.anchor_rect.y,
            PopupVertical::Center => self
                .anchor_rect
                .y
                .saturating_add(self.anchor_rect.height / 2),
            PopupVertical::Bottom => self.anchor_rect.y.saturating_add(self.anchor_rect.height),
        };
        let gravity_y = match self.gravity.vertical {
            PopupVertical::Top => -self.height,
            PopupVertical::Center => -(self.height / 2),
            PopupVertical::Bottom => 0,
        };
        anchor_y
            .saturating_add(self.offset_y)
            .saturating_add(gravity_y)
    }

    pub(super) fn constrained_geometry(self, target: PopupRect) -> PopupRect {
        let mut adjusted_positioner = self;
        let mut geometry = adjusted_positioner.geometry();
        let (mut off_left, mut off_right, mut off_top, mut off_bottom) =
            target.constraint_offsets(geometry);

        if (off_left > 0 || off_right > 0) && self.constraint_adjustment.flip_x {
            let mut flipped = adjusted_positioner;
            flipped.anchor = flipped.anchor.flip_x();
            flipped.gravity = flipped.gravity.flip_x();
            let flipped_geometry = flipped.geometry();
            let (new_off_left, new_off_right, _, _) = target.constraint_offsets(flipped_geometry);
            if new_off_left <= 0 && new_off_right <= 0 {
                adjusted_positioner = flipped;
                geometry = flipped_geometry;
                off_left = 0;
                off_right = 0;
            }
        }

        if (off_top > 0 || off_bottom > 0) && self.constraint_adjustment.flip_y {
            let mut flipped = adjusted_positioner;
            flipped.anchor = flipped.anchor.flip_y();
            flipped.gravity = flipped.gravity.flip_y();
            let flipped_geometry = flipped.geometry();
            let (_, _, new_off_top, new_off_bottom) = target.constraint_offsets(flipped_geometry);
            if new_off_top <= 0 && new_off_bottom <= 0 {
                geometry = flipped_geometry;
                off_top = 0;
                off_bottom = 0;
            }
        }

        if (off_left > 0 || off_right > 0) && self.constraint_adjustment.slide_x {
            if off_left > 0 {
                geometry.x = geometry.x.saturating_add(off_left);
            } else if off_right > 0 {
                geometry.x = geometry.x.saturating_sub(off_right.min(-off_left));
            }
            (off_left, off_right, _, _) = target.constraint_offsets(geometry);
        }

        if (off_top > 0 || off_bottom > 0) && self.constraint_adjustment.slide_y {
            if off_top > 0 {
                geometry.y = geometry.y.saturating_add(off_top);
            } else if off_bottom > 0 {
                geometry.y = geometry.y.saturating_sub(off_bottom.min(-off_top));
            }
            (_, _, off_top, off_bottom) = target.constraint_offsets(geometry);
        }

        if self.constraint_adjustment.resize_x {
            if off_left > 0 && off_left < geometry.width {
                geometry.x = geometry.x.saturating_add(off_left);
                geometry.width = geometry.width.saturating_sub(off_left).max(1);
            }
            if off_right > 0 && off_right < geometry.width {
                geometry.width = geometry.width.saturating_sub(off_right).max(1);
            }
        }

        if self.constraint_adjustment.resize_y {
            if off_top > 0 && off_top < geometry.height {
                geometry.y = geometry.y.saturating_add(off_top);
                geometry.height = geometry.height.saturating_sub(off_top).max(1);
            }
            if off_bottom > 0 && off_bottom < geometry.height {
                geometry.height = geometry.height.saturating_sub(off_bottom).max(1);
            }
        }

        geometry
    }
}

impl Default for XdgPositionerState {
    fn default() -> Self {
        Self {
            // xdg_positioner is incomplete until both set_size and
            // set_anchor_rect have supplied non-zero dimensions.
            width: 0,
            height: 0,
            anchor_rect: PopupAnchorRect::default(),
            anchor: PopupEdges::CENTER,
            gravity: PopupEdges::CENTER,
            offset_x: 0,
            offset_y: 0,
            constraint_adjustment: PopupConstraintAdjustment::default(),
            parent_size: None,
            parent_configure: None,
            reactive: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incomplete_positioner_is_not_usable_for_reposition() {
        assert!(!XdgPositionerState::default().is_complete());

        let mut positioner = XdgPositionerState {
            width: 80,
            height: 40,
            ..XdgPositionerState::default()
        };
        assert!(!positioner.is_complete());

        positioner.anchor_rect.width = 1;
        positioner.anchor_rect.height = 1;
        assert!(positioner.is_complete());
    }
}
