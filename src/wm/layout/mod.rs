pub mod constraints;
pub mod dwindle;
mod exact_aspect;
pub mod geometry;
pub mod lattice;
pub mod plan;
pub mod resize;
pub mod solve;

pub use constraints::{
    ClientRectError, ConstraintValidationError, LayoutConstraints, resolve_client_rect_within_tile,
};
pub use dwindle::{DwindleNodeId, DwindleNodeKind, DwindleTree, DwindleTreeError, InsertHint};
pub use geometry::{LayoutPoint, LayoutRect, SplitAxis, SplitRatio};
pub use plan::{
    LayoutError, LayoutGeneration, LayoutWindowSnapshot, TiledFallbackReason, TiledLayoutManager,
    TiledLayoutPlan, TiledWindowTarget,
};
pub use resize::{ResizeEdges, TiledResizeAxis, TiledResizeHandle};
pub use solve::{
    ConstraintInfeasibility, RatioOverride, ResolvedSplit, SplitRatioRange, SubtreeLowerBounds,
    SubtreeRequirements, TiledLayoutSolution, minimum_height_for_width, minimum_width_for_height,
    subtree_fits,
};
