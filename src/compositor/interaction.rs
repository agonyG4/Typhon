use wayland_protocols::xdg::shell::server::xdg_toplevel;
use wayland_server::{WEnum, protocol::wl_surface};

use super::{SurfacePlacement, render};

const MIN_WINDOW_WIDTH: u32 = 160;
const MIN_WINDOW_HEIGHT: u32 = 120;
const WINDOW_FRAME_RESIZE_THICKNESS: f64 = render::SERVER_FRAME_BORDER_THICKNESS as f64;
const WINDOW_RESIZE_DRAG_THRESHOLD: i32 = 3;

#[derive(Debug, Clone)]
pub(super) struct PointerTarget {
    pub(super) surface: wl_surface::WlSurface,
    pub(super) surface_x: f64,
    pub(super) surface_y: f64,
}

#[derive(Debug, Clone)]
pub(super) struct PointerPress {
    pub(super) serial: u32,
    pub(super) button: u32,
    pub(super) surface: wl_surface::WlSurface,
    pub(super) root_surface_id: u32,
    pub(super) output_x: f64,
    pub(super) output_y: f64,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct RootSurfaceHit {
    pub(super) root_surface_id: u32,
    pub(super) local_x: f64,
    pub(super) local_y: f64,
    pub(super) width: u32,
    pub(super) height: u32,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct WindowFrameHit {
    pub(super) root_surface_id: u32,
    pub(super) kind: WindowInteractionKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WindowInteractionKind {
    Move,
    Resize(ResizeEdges),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ResizeEdges {
    pub(super) top: bool,
    pub(super) bottom: bool,
    pub(super) left: bool,
    pub(super) right: bool,
}

impl ResizeEdges {
    pub(super) const BOTTOM_RIGHT: Self = Self {
        top: false,
        bottom: true,
        left: false,
        right: true,
    };

    pub(super) const fn new(top: bool, bottom: bool, left: bool, right: bool) -> Self {
        Self {
            top,
            bottom,
            left,
            right,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct WindowInteraction {
    pub(super) root_surface_id: u32,
    pub(super) kind: WindowInteractionKind,
    pub(super) start_pointer_x: f64,
    pub(super) start_pointer_y: f64,
    pub(super) start_placement: SurfacePlacement,
    pub(super) start_width: u32,
    pub(super) start_height: u32,
    pub(super) drag_committed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PendingResizeConfigure {
    pub(super) surface_id: u32,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) placement: SurfacePlacement,
    pub(super) edges: ResizeEdges,
    pub(super) resizing: bool,
}

impl PendingResizeConfigure {
    pub(super) const fn resize_commit(self, serial: u32) -> PendingResizeCommit {
        PendingResizeCommit {
            serial,
            width: self.width,
            height: self.height,
            placement: self.placement,
            edges: self.edges,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PendingResizeCommit {
    pub(super) serial: u32,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) placement: SurfacePlacement,
    pub(super) edges: ResizeEdges,
}

impl PendingResizeCommit {
    pub(super) fn placement_for_committed_size(self, width: u32, height: u32) -> SurfacePlacement {
        let mut placement = self.placement;
        if self.edges.left {
            let target_right = placement
                .local_x
                .saturating_add(i32::try_from(self.width).unwrap_or(i32::MAX));
            placement.local_x =
                target_right.saturating_sub(i32::try_from(width).unwrap_or(i32::MAX));
        }
        if self.edges.top {
            let target_bottom = placement
                .local_y
                .saturating_add(i32::try_from(self.height).unwrap_or(i32::MAX));
            placement.local_y =
                target_bottom.saturating_sub(i32::try_from(height).unwrap_or(i32::MAX));
        }
        placement
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ResizeGeometry {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) width: u32,
    pub(super) height: u32,
}

pub(super) fn interactive_resize_geometry(
    interaction: WindowInteraction,
    edges: ResizeEdges,
    dx: i32,
    dy: i32,
) -> ResizeGeometry {
    let start_width = i32::try_from(interaction.start_width).unwrap_or(i32::MAX);
    let start_height = i32::try_from(interaction.start_height).unwrap_or(i32::MAX);
    let min_width = i32::try_from(MIN_WINDOW_WIDTH).unwrap_or(i32::MAX);
    let min_height = i32::try_from(MIN_WINDOW_HEIGHT).unwrap_or(i32::MAX);
    let mut x = interaction.start_placement.local_x;
    let mut y = interaction.start_placement.local_y;
    let mut width = start_width;
    let mut height = start_height;

    if edges.left {
        width = start_width.saturating_sub(dx).max(min_width);
        x = interaction
            .start_placement
            .local_x
            .saturating_add(start_width.saturating_sub(width));
    } else if edges.right {
        width = start_width.saturating_add(dx).max(min_width);
    }

    if edges.top {
        height = start_height.saturating_sub(dy).max(min_height);
        y = interaction
            .start_placement
            .local_y
            .saturating_add(start_height.saturating_sub(height));
    } else if edges.bottom {
        height = start_height.saturating_add(dy).max(min_height);
    }

    ResizeGeometry {
        x,
        y,
        width: width.try_into().unwrap_or(MIN_WINDOW_WIDTH),
        height: height.try_into().unwrap_or(MIN_WINDOW_HEIGHT),
    }
}

pub(super) fn resize_drag_threshold_reached(edges: ResizeEdges, dx: i32, dy: i32) -> bool {
    let horizontal = edges.left || edges.right;
    let vertical = edges.top || edges.bottom;
    (horizontal && dx.abs() >= WINDOW_RESIZE_DRAG_THRESHOLD)
        || (vertical && dy.abs() >= WINDOW_RESIZE_DRAG_THRESHOLD)
}

pub(super) fn resize_edges_from_xdg(edges: WEnum<xdg_toplevel::ResizeEdge>) -> Option<ResizeEdges> {
    match edges {
        WEnum::Value(xdg_toplevel::ResizeEdge::Top) => {
            Some(ResizeEdges::new(true, false, false, false))
        }
        WEnum::Value(xdg_toplevel::ResizeEdge::Bottom) => {
            Some(ResizeEdges::new(false, true, false, false))
        }
        WEnum::Value(xdg_toplevel::ResizeEdge::Left) => {
            Some(ResizeEdges::new(false, false, true, false))
        }
        WEnum::Value(xdg_toplevel::ResizeEdge::TopLeft) => {
            Some(ResizeEdges::new(true, false, true, false))
        }
        WEnum::Value(xdg_toplevel::ResizeEdge::BottomLeft) => {
            Some(ResizeEdges::new(false, true, true, false))
        }
        WEnum::Value(xdg_toplevel::ResizeEdge::Right) => {
            Some(ResizeEdges::new(false, false, false, true))
        }
        WEnum::Value(xdg_toplevel::ResizeEdge::TopRight) => {
            Some(ResizeEdges::new(true, false, false, true))
        }
        WEnum::Value(xdg_toplevel::ResizeEdge::BottomRight) => Some(ResizeEdges::BOTTOM_RIGHT),
        WEnum::Value(xdg_toplevel::ResizeEdge::None) | WEnum::Unknown(_) => None,
        _ => None,
    }
}

pub(super) fn resize_edges_for_window_point(
    local_x: f64,
    local_y: f64,
    width: u32,
    height: u32,
) -> ResizeEdges {
    let left = local_x < f64::from(width) / 2.0;
    let top = local_y < f64::from(height) / 2.0;
    ResizeEdges::new(top, !top, left, !left)
}

pub(super) fn window_frame_action_for_local_point(
    local_x: f64,
    local_y: f64,
    width: u32,
    height: u32,
) -> Option<WindowInteractionKind> {
    let width = f64::from(width);
    let height = f64::from(height);
    let resize_top = -WINDOW_FRAME_RESIZE_THICKNESS;
    let resize_left = -WINDOW_FRAME_RESIZE_THICKNESS;
    let resize_right = width + WINDOW_FRAME_RESIZE_THICKNESS;
    let resize_bottom = height + WINDOW_FRAME_RESIZE_THICKNESS;

    if local_x < resize_left
        || local_x >= resize_right
        || local_y < resize_top
        || local_y >= resize_bottom
    {
        return None;
    }

    let inside_content = local_x >= 0.0 && local_x < width && local_y >= 0.0 && local_y < height;
    if inside_content {
        return None;
    }

    let within_vertical_frame = local_y >= resize_top && local_y < resize_bottom;
    let within_horizontal_frame = local_x >= resize_left && local_x < resize_right;
    let near_left = local_x >= resize_left && local_x < 0.0 && within_vertical_frame;
    let near_right = local_x >= width && local_x < resize_right && within_vertical_frame;
    let near_top = local_y >= resize_top && local_y < 0.0 && within_horizontal_frame;
    let near_bottom = local_y >= height && local_y < resize_bottom && within_horizontal_frame;

    if near_left || near_right || near_top || near_bottom {
        return Some(WindowInteractionKind::Resize(ResizeEdges::new(
            near_top,
            near_bottom,
            near_left,
            near_right,
        )));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resize_interaction(edges: ResizeEdges) -> WindowInteraction {
        WindowInteraction {
            root_surface_id: 1,
            kind: WindowInteractionKind::Resize(edges),
            start_pointer_x: 0.0,
            start_pointer_y: 0.0,
            start_placement: SurfacePlacement::root_at(72, 72),
            start_width: 300,
            start_height: 200,
            drag_committed: true,
        }
    }

    #[test]
    fn bottom_right_negative_delta_shrinks_without_growing_first() {
        let edges = ResizeEdges::BOTTOM_RIGHT;

        let geometry = interactive_resize_geometry(resize_interaction(edges), edges, -1, -1);

        assert_eq!(geometry.width, 299);
        assert_eq!(geometry.height, 199);
        assert_eq!(geometry.x, 72);
        assert_eq!(geometry.y, 72);
    }

    #[test]
    fn alt_resize_edges_follow_nearest_window_corner() {
        assert_eq!(
            resize_edges_for_window_point(24.0, 24.0, 300, 200),
            ResizeEdges::new(true, false, true, false)
        );
        assert_eq!(
            resize_edges_for_window_point(276.0, 24.0, 300, 200),
            ResizeEdges::new(true, false, false, true)
        );
        assert_eq!(
            resize_edges_for_window_point(24.0, 176.0, 300, 200),
            ResizeEdges::new(false, true, true, false)
        );
        assert_eq!(
            resize_edges_for_window_point(276.0, 176.0, 300, 200),
            ResizeEdges::BOTTOM_RIGHT
        );
    }
}
