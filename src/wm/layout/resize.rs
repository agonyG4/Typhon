#[cfg(test)]
mod tests {
    use super::{ResizeEdges, TiledResizeHandle};
    use crate::core::WindowId;
    use crate::wm::layout::dwindle::{DwindleNodeKind, DwindleTree, InsertHint};
    use crate::wm::layout::geometry::{LayoutRect, SplitAxis};

    fn window(raw: u64) -> WindowId {
        WindowId::from_raw(raw).expect("non-zero test window id")
    }

    #[test]
    fn resolves_right_edge_to_nearest_horizontal_first_child_ancestor() {
        let mut tree = DwindleTree::default();
        tree.insert(window(1), InsertHint::default())
            .expect("first");
        tree.insert(window(2), InsertHint::default())
            .expect("second");
        let root = tree.root().expect("root");
        assert!(matches!(
            tree.node_kind(root),
            Some(DwindleNodeKind::Split {
                axis: SplitAxis::Horizontal,
                ..
            })
        ));
        let handle = TiledResizeHandle::for_window(
            &tree,
            window(1),
            ResizeEdges::RIGHT,
            LayoutRect::new(0, 0, 1920, 1080).expect("root"),
        )
        .expect("right edge divider");
        assert_eq!(handle.horizontal().unwrap().split(), root);
    }

    #[test]
    fn outer_edge_has_no_adjustable_axis() {
        let mut tree = DwindleTree::default();
        tree.insert(window(1), InsertHint::default()).expect("leaf");
        assert!(
            TiledResizeHandle::for_window(
                &tree,
                window(1),
                ResizeEdges::RIGHT,
                LayoutRect::new(0, 0, 100, 100).expect("root"),
            )
            .is_none()
        );
    }
}
use super::dwindle::{DwindleNodeId, DwindleNodeKind, DwindleTree};
use super::geometry::{LayoutRect, SplitAxis, SplitRatio};
use super::solve::TiledLayoutSolution;
use crate::core::WindowId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResizeEdges {
    pub top: bool,
    pub bottom: bool,
    pub left: bool,
    pub right: bool,
}

impl ResizeEdges {
    pub const TOP: Self = Self {
        top: true,
        ..Self::NONE
    };
    pub const BOTTOM: Self = Self {
        bottom: true,
        ..Self::NONE
    };
    pub const LEFT: Self = Self {
        left: true,
        ..Self::NONE
    };
    pub const RIGHT: Self = Self {
        right: true,
        ..Self::NONE
    };
    pub const NONE: Self = Self {
        top: false,
        bottom: false,
        left: false,
        right: false,
    };

    pub const fn new(top: bool, bottom: bool, left: bool, right: bool) -> Self {
        Self {
            top,
            bottom,
            left,
            right,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TiledResizeAxis {
    split: DwindleNodeId,
    axis: SplitAxis,
    start_ratio: SplitRatio,
    parent_rect: LayoutRect,
    start_boundary: i32,
}

impl TiledResizeAxis {
    pub const fn split(self) -> DwindleNodeId {
        self.split
    }

    pub const fn axis(self) -> SplitAxis {
        self.axis
    }

    pub const fn start_ratio(self) -> SplitRatio {
        self.start_ratio
    }

    pub const fn parent_rect(self) -> LayoutRect {
        self.parent_rect
    }

    pub const fn start_boundary(self) -> i32 {
        self.start_boundary
    }

    pub fn requested_ratio(self, displacement: i32) -> f64 {
        let extent = match self.axis {
            SplitAxis::Horizontal => self.parent_rect.width(),
            SplitAxis::Vertical => self.parent_rect.height(),
        };
        if extent == 0 {
            return self.start_ratio.value();
        }
        let boundary = self.start_boundary.saturating_add(displacement);
        let origin = match self.axis {
            SplitAxis::Horizontal => self.parent_rect.x(),
            SplitAxis::Vertical => self.parent_rect.y(),
        };
        f64::from(boundary.saturating_sub(origin)) / f64::from(extent)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TiledResizeHandle {
    horizontal: Option<TiledResizeAxis>,
    vertical: Option<TiledResizeAxis>,
}

impl TiledResizeHandle {
    pub const fn horizontal(self) -> Option<TiledResizeAxis> {
        self.horizontal
    }

    pub const fn vertical(self) -> Option<TiledResizeAxis> {
        self.vertical
    }

    pub fn for_window(
        tree: &DwindleTree,
        window: WindowId,
        edges: ResizeEdges,
        root: LayoutRect,
    ) -> Option<Self> {
        let leaf = tree.leaf_for_window(window)?;
        let mut current = leaf;
        let mut horizontal = None;
        let mut vertical = None;
        while let Some(parent) = tree.parent_of(current)? {
            let DwindleNodeKind::Split {
                axis,
                ratio,
                first,
                second: _,
            } = tree.node_kind(parent)?
            else {
                return None;
            };
            let in_first = first == current;
            let candidate = match axis {
                SplitAxis::Horizontal if (edges.right && in_first) || (edges.left && !in_first) => {
                    Some(&mut horizontal)
                }
                SplitAxis::Vertical if (edges.bottom && in_first) || (edges.top && !in_first) => {
                    Some(&mut vertical)
                }
                _ => None,
            };
            if let Some(slot) = candidate
                && slot.is_none()
            {
                // The interaction-start geometry is supplied by the compositor in the
                // full session. The pure handle keeps the root as a deterministic
                // fallback parent until the solved start rect is installed.
                let parent_rect = root;
                let boundary = parent_rect.split_boundary(axis, ratio);
                *slot = Some(TiledResizeAxis {
                    split: parent,
                    axis,
                    start_ratio: ratio,
                    parent_rect,
                    start_boundary: boundary,
                });
            }
            current = parent;
        }
        (horizontal.is_some() || vertical.is_some()).then_some(Self {
            horizontal,
            vertical,
        })
    }

    pub fn from_solution(
        tree: &DwindleTree,
        solution: &TiledLayoutSolution,
        window: WindowId,
        edges: ResizeEdges,
    ) -> Option<Self> {
        let leaf = tree.leaf_for_window(window)?;
        let mut current = leaf;
        let mut horizontal = None;
        let mut vertical = None;
        while let Some(parent) = tree.parent_of(current)? {
            let DwindleNodeKind::Split {
                axis,
                ratio,
                first,
                second: _,
            } = tree.node_kind(parent)?
            else {
                return None;
            };
            let in_first = first == current;
            let relevant = match axis {
                SplitAxis::Horizontal if (edges.right && in_first) || (edges.left && !in_first) => {
                    &mut horizontal
                }
                SplitAxis::Vertical if (edges.bottom && in_first) || (edges.top && !in_first) => {
                    &mut vertical
                }
                _ => {
                    current = parent;
                    continue;
                }
            };
            if relevant.is_none() {
                let resolved = solution
                    .splits()
                    .iter()
                    .find(|split| split.node() == parent)?;
                *relevant = Some(TiledResizeAxis {
                    split: parent,
                    axis,
                    start_ratio: ratio,
                    parent_rect: resolved.parent_rect(),
                    start_boundary: resolved.boundary(),
                });
            }
            current = parent;
        }
        (horizontal.is_some() || vertical.is_some()).then_some(Self {
            horizontal,
            vertical,
        })
    }
}
