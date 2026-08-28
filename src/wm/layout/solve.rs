use std::collections::HashMap;

use crate::core::WindowId;
use crate::wm::WorkspaceLocation;

use super::constraints::{ClientRectError, resolve_client_rect_within_tile};
use super::dwindle::{DwindleNodeId, DwindleNodeKind, DwindleTree};
use super::geometry::{LayoutRect, SplitAxis, SplitRatio};
use super::plan::{LayoutError, LayoutWindowSnapshot, TiledLayoutPlan, TiledWindowTarget};

/// Cheap independent lower bounds for a subtree.
///
/// These bounds are useful for pruning and diagnostics only.  Coupled
/// aspect/increment feasibility is answered by `FeasibilityContext`, not by
/// comparing a rectangle with these two scalars.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SubtreeLowerBounds {
    pub min_width: u32,
    pub min_height: u32,
}

/// Compatibility name for callers of the v1.1 solver API.
pub type SubtreeRequirements = SubtreeLowerBounds;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SplitRatioRange {
    min: SplitRatio,
    max: SplitRatio,
}

impl SplitRatioRange {
    pub fn new(min: f64, max: f64) -> Option<Self> {
        let min = min.max(SplitRatio::MIN);
        let max = max.min(SplitRatio::MAX);
        (min <= max).then(|| Self {
            min: SplitRatio::new(min).expect("safety-clamped minimum ratio"),
            max: SplitRatio::new(max).expect("safety-clamped maximum ratio"),
        })
    }

    pub const fn min(self) -> SplitRatio {
        self.min
    }
    pub const fn max(self) -> SplitRatio {
        self.max
    }

    pub fn clamp(self, ratio: SplitRatio) -> SplitRatio {
        SplitRatio::new(ratio.value().clamp(self.min.value(), self.max.value()))
            .expect("range always contains valid ratios")
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RatioOverride {
    split: DwindleNodeId,
    ratio: SplitRatio,
}

impl RatioOverride {
    pub const fn new(split: DwindleNodeId, ratio: SplitRatio) -> Self {
        Self { split, ratio }
    }
    pub const fn split(self) -> DwindleNodeId {
        self.split
    }
    pub const fn ratio(self) -> SplitRatio {
        self.ratio
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedSplit {
    node: DwindleNodeId,
    parent_rect: LayoutRect,
    preferred_ratio: SplitRatio,
    effective_ratio: SplitRatio,
    feasible_range: SplitRatioRange,
    boundary: i32,
}

impl ResolvedSplit {
    pub const fn node(self) -> DwindleNodeId {
        self.node
    }
    pub const fn parent_rect(self) -> LayoutRect {
        self.parent_rect
    }
    pub const fn preferred_ratio(self) -> SplitRatio {
        self.preferred_ratio
    }
    pub const fn effective_ratio(self) -> SplitRatio {
        self.effective_ratio
    }
    pub const fn feasible_range(self) -> SplitRatioRange {
        self.feasible_range
    }
    pub const fn boundary(self) -> i32 {
        self.boundary
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConstraintInfeasibility {
    pub node: Option<DwindleNodeId>,
    pub axis: Option<SplitAxis>,
    pub required_width: u32,
    pub required_height: u32,
    pub available_width: u32,
    pub available_height: u32,
    pub windows: Vec<WindowId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TiledLayoutSolution {
    plan: TiledLayoutPlan,
    splits: Vec<ResolvedSplit>,
    requirements: HashMap<DwindleNodeId, SubtreeLowerBounds>,
    node_visits: usize,
}

impl TiledLayoutSolution {
    pub(super) fn empty(location: WorkspaceLocation) -> Self {
        Self {
            plan: TiledLayoutPlan::new(location, Vec::new()),
            splits: Vec::new(),
            requirements: HashMap::new(),
            node_visits: 0,
        }
    }
    pub const fn plan(&self) -> &TiledLayoutPlan {
        &self.plan
    }
    pub fn into_plan(self) -> TiledLayoutPlan {
        self.plan
    }
    pub const fn location(&self) -> WorkspaceLocation {
        self.plan.location()
    }
    pub fn updates(&self) -> &[TiledWindowTarget] {
        self.plan.updates()
    }
    pub fn len(&self) -> usize {
        self.plan.len()
    }
    pub fn is_empty(&self) -> bool {
        self.plan.is_empty()
    }
    pub fn target_for_window(&self, window: WindowId) -> Option<TiledWindowTarget> {
        self.plan.target_for_window(window)
    }
    pub fn splits(&self) -> &[ResolvedSplit] {
        &self.splits
    }
    pub fn lower_bounds_for(&self, node: DwindleNodeId) -> Option<SubtreeLowerBounds> {
        self.requirements.get(&node).copied()
    }
    pub fn requirements_for(&self, node: DwindleNodeId) -> Option<SubtreeRequirements> {
        self.lower_bounds_for(node)
    }
    pub const fn node_visits(&self) -> usize {
        self.node_visits
    }
}

pub(crate) fn solve_tree(
    tree: &DwindleTree,
    location: WorkspaceLocation,
    root: LayoutRect,
    snapshots: &[LayoutWindowSnapshot],
) -> Result<TiledLayoutSolution, LayoutError> {
    solve_tree_with_ratio_overrides(tree, location, root, snapshots, &[])
}

pub(crate) fn solve_tree_with_ratio_overrides(
    tree: &DwindleTree,
    location: WorkspaceLocation,
    root: LayoutRect,
    snapshots: &[LayoutWindowSnapshot],
    overrides: &[RatioOverride],
) -> Result<TiledLayoutSolution, LayoutError> {
    let snapshots = snapshot_map(snapshots)?;
    let mut requirements = HashMap::with_capacity(tree.len().saturating_mul(2));
    let mut node_visits = 0;
    let Some(root_id) = tree.root() else {
        return Ok(TiledLayoutSolution::empty(location));
    };
    if aggregate_lower_bounds(
        tree,
        root_id,
        &snapshots,
        &mut requirements,
        &mut node_visits,
    )?
    .is_none()
    {
        return Ok(TiledLayoutSolution {
            plan: TiledLayoutPlan::new(location, Vec::new()),
            splits: Vec::new(),
            requirements,
            node_visits,
        });
    }

    let exact = snapshots.values().any(|snapshot| {
        !snapshot.minimized
            && (snapshot.constraints.min_aspect.is_some()
                || snapshot.constraints.max_aspect.is_some())
    });
    let mut feasibility = FeasibilityContext::new(tree, &snapshots, exact);
    if exact && !feasibility.can_fit(root_id, root)? {
        return Err(feasibility.infeasible(root_id, root));
    }
    let overrides = overrides
        .iter()
        .copied()
        .map(|override_| (override_.split(), override_.ratio()))
        .collect::<HashMap<_, _>>();
    let mut updates = Vec::with_capacity(tree.len());
    let mut splits = Vec::new();
    resolve_node(
        tree,
        root_id,
        root,
        &snapshots,
        &requirements,
        &overrides,
        &mut feasibility,
        &mut updates,
        &mut splits,
        &mut node_visits,
    )?;
    Ok(TiledLayoutSolution {
        plan: TiledLayoutPlan::new(location, updates),
        splits,
        requirements,
        node_visits: node_visits.saturating_add(feasibility.node_visits),
    })
}

/// Exact pure feasibility query for one active subtree.
pub fn subtree_fits(
    tree: &DwindleTree,
    node: DwindleNodeId,
    rect: LayoutRect,
    snapshots: &[LayoutWindowSnapshot],
) -> Result<bool, LayoutError> {
    let snapshots = snapshot_map(snapshots)?;
    FeasibilityContext::new(tree, &snapshots, true).can_fit(node, rect)
}

/// Exact minimum width for a subtree at a fixed available height.
pub fn minimum_width_for_height(
    tree: &DwindleTree,
    node: DwindleNodeId,
    height: u32,
    width_cap: u32,
    snapshots: &[LayoutWindowSnapshot],
) -> Result<Option<u32>, LayoutError> {
    let snapshots = snapshot_map(snapshots)?;
    FeasibilityContext::new(tree, &snapshots, true)
        .minimum_width_for_height(node, height, width_cap)
}

/// Exact minimum height for a subtree at a fixed available width.
pub fn minimum_height_for_width(
    tree: &DwindleTree,
    node: DwindleNodeId,
    width: u32,
    height_cap: u32,
    snapshots: &[LayoutWindowSnapshot],
) -> Result<Option<u32>, LayoutError> {
    let snapshots = snapshot_map(snapshots)?;
    FeasibilityContext::new(tree, &snapshots, true)
        .minimum_height_for_width(node, width, height_cap)
}

fn snapshot_map(
    snapshots: &[LayoutWindowSnapshot],
) -> Result<HashMap<WindowId, LayoutWindowSnapshot>, LayoutError> {
    let mut map = HashMap::with_capacity(snapshots.len());
    for snapshot in snapshots {
        snapshot
            .constraints
            .validate()
            .map_err(|_| LayoutError::InvalidConstraints(snapshot.window))?;
        if map.insert(snapshot.window, *snapshot).is_some() {
            return Err(LayoutError::DuplicateSnapshot(snapshot.window));
        }
    }
    Ok(map)
}

fn aggregate_lower_bounds(
    tree: &DwindleTree,
    id: DwindleNodeId,
    snapshots: &HashMap<WindowId, LayoutWindowSnapshot>,
    requirements: &mut HashMap<DwindleNodeId, SubtreeLowerBounds>,
    node_visits: &mut usize,
) -> Result<Option<SubtreeLowerBounds>, LayoutError> {
    *node_visits = node_visits.saturating_add(1);
    let result = match tree.node_kind(id).ok_or(LayoutError::InvalidTree)? {
        DwindleNodeKind::Leaf { window } => {
            let snapshot = snapshots
                .get(&window)
                .ok_or(LayoutError::MissingSnapshot(window))?;
            if snapshot.minimized {
                None
            } else {
                let (min_width, min_height) = snapshot
                    .constraints
                    .independent_lower_bounds()
                    .map_err(|_| {
                        LayoutError::ConstraintInfeasible(ConstraintInfeasibility {
                            node: Some(id),
                            axis: None,
                            required_width: snapshot.constraints.min_width.unwrap_or(1),
                            required_height: snapshot.constraints.min_height.unwrap_or(1),
                            available_width: 0,
                            available_height: 0,
                            windows: vec![window],
                        })
                    })?;
                Some(SubtreeLowerBounds {
                    min_width,
                    min_height,
                })
            }
        }
        DwindleNodeKind::Split {
            axis,
            first,
            second,
            ..
        } => {
            let first = aggregate_lower_bounds(tree, first, snapshots, requirements, node_visits)?;
            let second =
                aggregate_lower_bounds(tree, second, snapshots, requirements, node_visits)?;
            match (first, second) {
                (None, None) => None,
                (Some(value), None) | (None, Some(value)) => Some(value),
                (Some(first), Some(second)) => Some(match axis {
                    SplitAxis::Horizontal => SubtreeLowerBounds {
                        min_width: first.min_width.saturating_add(second.min_width),
                        min_height: first.min_height.max(second.min_height),
                    },
                    SplitAxis::Vertical => SubtreeLowerBounds {
                        min_width: first.min_width.max(second.min_width),
                        min_height: first.min_height.saturating_add(second.min_height),
                    },
                }),
            }
        }
    };
    if let Some(value) = result {
        requirements.insert(id, value);
    }
    Ok(result)
}

struct FeasibilityContext<'a> {
    tree: &'a DwindleTree,
    snapshots: &'a HashMap<WindowId, LayoutWindowSnapshot>,
    exact: bool,
    width_cache: HashMap<(DwindleNodeId, u32), Option<u32>>,
    height_cache: HashMap<(DwindleNodeId, u32), Option<u32>>,
    node_visits: usize,
}

impl<'a> FeasibilityContext<'a> {
    fn new(
        tree: &'a DwindleTree,
        snapshots: &'a HashMap<WindowId, LayoutWindowSnapshot>,
        exact: bool,
    ) -> Self {
        Self {
            tree,
            snapshots,
            exact,
            width_cache: HashMap::new(),
            height_cache: HashMap::new(),
            node_visits: 0,
        }
    }

    fn can_fit(&mut self, id: DwindleNodeId, rect: LayoutRect) -> Result<bool, LayoutError> {
        self.node_visits = self.node_visits.saturating_add(1);
        match self.tree.node_kind(id).ok_or(LayoutError::InvalidTree)? {
            DwindleNodeKind::Leaf { window } => {
                let snapshot = self
                    .snapshots
                    .get(&window)
                    .ok_or(LayoutError::MissingSnapshot(window))?;
                if snapshot.minimized {
                    Ok(true)
                } else if !self.exact {
                    Ok(snapshot.constraints.independent_lower_bounds().is_ok_and(
                        |(width, height)| width <= rect.width() && height <= rect.height(),
                    ))
                } else {
                    Ok(resolve_client_rect_within_tile(rect, snapshot.constraints).is_ok())
                }
            }
            DwindleNodeKind::Split {
                axis,
                first,
                second,
                ..
            } => {
                if !self.exact {
                    return Ok(true);
                }
                Ok(self
                    .feasible_boundary_range(axis, first, second, rect)?
                    .is_some())
            }
        }
    }

    fn minimum_width_for_height(
        &mut self,
        id: DwindleNodeId,
        height: u32,
        cap: u32,
    ) -> Result<Option<u32>, LayoutError> {
        if let Some(value) = self.width_cache.get(&(id, height)) {
            return Ok(value.filter(|value| *value <= cap));
        }
        let mut low = 1;
        let mut high = cap.max(1);
        while low < high {
            let middle = low + (high - low) / 2;
            if self.can_fit(id, rect_with_size(middle, height))? {
                high = middle;
            } else {
                low = middle.saturating_add(1);
            }
        }
        let value = self
            .can_fit(id, rect_with_size(low, height))?
            .then_some(low);
        if value.is_some() {
            self.width_cache.insert((id, height), value);
        }
        Ok(value)
    }

    fn minimum_height_for_width(
        &mut self,
        id: DwindleNodeId,
        width: u32,
        cap: u32,
    ) -> Result<Option<u32>, LayoutError> {
        if let Some(value) = self.height_cache.get(&(id, width)) {
            return Ok(value.filter(|value| *value <= cap));
        }
        let mut low = 1;
        let mut high = cap.max(1);
        while low < high {
            let middle = low + (high - low) / 2;
            if self.can_fit(id, rect_with_size(width, middle))? {
                high = middle;
            } else {
                low = middle.saturating_add(1);
            }
        }
        let value = self.can_fit(id, rect_with_size(width, low))?.then_some(low);
        if value.is_some() {
            self.height_cache.insert((id, width), value);
        }
        Ok(value)
    }

    fn feasible_boundary_range(
        &mut self,
        axis: SplitAxis,
        first: DwindleNodeId,
        second: DwindleNodeId,
        rect: LayoutRect,
    ) -> Result<Option<(i32, i32)>, LayoutError> {
        let (minimum, maximum, extent, origin) = match axis {
            SplitAxis::Horizontal => {
                let first = self.minimum_width_for_height(first, rect.height(), rect.width())?;
                let second = self.minimum_width_for_height(second, rect.height(), rect.width())?;
                let (Some(first), Some(second)) = (first, second) else {
                    return Ok(None);
                };
                (
                    rect.x()
                        .saturating_add(i32::try_from(first).unwrap_or(i32::MAX)),
                    rect.right()
                        .saturating_sub(i32::try_from(second).unwrap_or(i32::MAX)),
                    rect.width(),
                    rect.x(),
                )
            }
            SplitAxis::Vertical => {
                let first = self.minimum_height_for_width(first, rect.width(), rect.height())?;
                let second = self.minimum_height_for_width(second, rect.width(), rect.height())?;
                let (Some(first), Some(second)) = (first, second) else {
                    return Ok(None);
                };
                (
                    rect.y()
                        .saturating_add(i32::try_from(first).unwrap_or(i32::MAX)),
                    rect.bottom()
                        .saturating_sub(i32::try_from(second).unwrap_or(i32::MAX)),
                    rect.height(),
                    rect.y(),
                )
            }
        };
        let safety_min = rect
            .split_boundary(axis, SplitRatio::new(SplitRatio::MIN).expect("safe ratio"))
            .max(origin.saturating_add(1));
        let safety_max = rect
            .split_boundary(axis, SplitRatio::new(SplitRatio::MAX).expect("safe ratio"))
            .min(
                origin.saturating_add(i32::try_from(extent).unwrap_or(i32::MAX).saturating_sub(1)),
            );
        let minimum = minimum.max(safety_min);
        let maximum = maximum.min(safety_max);
        Ok((minimum <= maximum).then_some((minimum, maximum)))
    }

    fn infeasible(&mut self, root: DwindleNodeId, rect: LayoutRect) -> LayoutError {
        LayoutError::ConstraintInfeasible(ConstraintInfeasibility {
            node: Some(root),
            axis: self.tree.node_kind(root).and_then(|kind| match kind {
                DwindleNodeKind::Split { axis, .. } => Some(axis),
                DwindleNodeKind::Leaf { .. } => None,
            }),
            required_width: self
                .minimum_width_for_height(root, rect.height(), rect.width())
                .ok()
                .flatten()
                .unwrap_or(rect.width().saturating_add(1)),
            required_height: self
                .minimum_height_for_width(root, rect.width(), rect.height())
                .ok()
                .flatten()
                .unwrap_or(rect.height().saturating_add(1)),
            available_width: rect.width(),
            available_height: rect.height(),
            windows: active_windows(self.tree, root, self.snapshots).unwrap_or_default(),
        })
    }
}

#[expect(clippy::too_many_arguments)]
fn resolve_node(
    tree: &DwindleTree,
    id: DwindleNodeId,
    rect: LayoutRect,
    snapshots: &HashMap<WindowId, LayoutWindowSnapshot>,
    requirements: &HashMap<DwindleNodeId, SubtreeLowerBounds>,
    overrides: &HashMap<DwindleNodeId, SplitRatio>,
    feasibility: &mut FeasibilityContext<'_>,
    updates: &mut Vec<TiledWindowTarget>,
    splits: &mut Vec<ResolvedSplit>,
    node_visits: &mut usize,
) -> Result<bool, LayoutError> {
    *node_visits = node_visits.saturating_add(1);
    match tree.node_kind(id).ok_or(LayoutError::InvalidTree)? {
        DwindleNodeKind::Leaf { window } => {
            let snapshot = snapshots
                .get(&window)
                .ok_or(LayoutError::MissingSnapshot(window))?;
            if snapshot.minimized {
                return Ok(false);
            }
            let client =
                resolve_client_rect_within_tile(rect, snapshot.constraints).map_err(|error| {
                    match error {
                        ClientRectError::InvalidConstraints(_) => {
                            LayoutError::InvalidConstraints(window)
                        }
                        ClientRectError::Infeasible => {
                            LayoutError::ConstraintInfeasible(ConstraintInfeasibility {
                                node: Some(id),
                                axis: None,
                                required_width: requirements.get(&id).map_or(0, |r| r.min_width),
                                required_height: requirements.get(&id).map_or(0, |r| r.min_height),
                                available_width: rect.width(),
                                available_height: rect.height(),
                                windows: vec![window],
                            })
                        }
                    }
                })?;
            updates.push(TiledWindowTarget::new(window, rect, client));
            Ok(true)
        }
        DwindleNodeKind::Split {
            axis,
            ratio,
            first,
            second,
        } => {
            let first_requirements = requirements.get(&first).copied();
            let second_requirements = requirements.get(&second).copied();
            match (first_requirements, second_requirements) {
                (None, None) => Ok(false),
                (Some(_), None) => resolve_node(
                    tree,
                    first,
                    rect,
                    snapshots,
                    requirements,
                    overrides,
                    feasibility,
                    updates,
                    splits,
                    node_visits,
                ),
                (None, Some(_)) => resolve_node(
                    tree,
                    second,
                    rect,
                    snapshots,
                    requirements,
                    overrides,
                    feasibility,
                    updates,
                    splits,
                    node_visits,
                ),
                (Some(first_requirements), Some(second_requirements)) => {
                    let (minimum_boundary, maximum_boundary, extent, origin) = if feasibility.exact
                    {
                        let Some((minimum, maximum)) =
                            feasibility.feasible_boundary_range(axis, first, second, rect)?
                        else {
                            return Err(feasibility.infeasible(id, rect));
                        };
                        (
                            minimum,
                            maximum,
                            match axis {
                                SplitAxis::Horizontal => rect.width(),
                                SplitAxis::Vertical => rect.height(),
                            },
                            match axis {
                                SplitAxis::Horizontal => rect.x(),
                                SplitAxis::Vertical => rect.y(),
                            },
                        )
                    } else {
                        match axis {
                            SplitAxis::Horizontal => (
                                rect.x().saturating_add(
                                    i32::try_from(first_requirements.min_width).unwrap_or(i32::MAX),
                                ),
                                rect.right().saturating_sub(
                                    i32::try_from(second_requirements.min_width)
                                        .unwrap_or(i32::MAX),
                                ),
                                rect.width(),
                                rect.x(),
                            ),
                            SplitAxis::Vertical => (
                                rect.y().saturating_add(
                                    i32::try_from(first_requirements.min_height)
                                        .unwrap_or(i32::MAX),
                                ),
                                rect.bottom().saturating_sub(
                                    i32::try_from(second_requirements.min_height)
                                        .unwrap_or(i32::MAX),
                                ),
                                rect.height(),
                                rect.y(),
                            ),
                        }
                    };
                    let range = SplitRatioRange::new(
                        f64::from(minimum_boundary.saturating_sub(origin)) / f64::from(extent),
                        f64::from(maximum_boundary.saturating_sub(origin)) / f64::from(extent),
                    )
                    .ok_or_else(|| infeasible_split(tree, id, axis, rect, snapshots))?;
                    let preferred_ratio = overrides.get(&id).copied().unwrap_or(ratio);
                    let effective_ratio = range.clamp(preferred_ratio);
                    let boundary = rect
                        .split_boundary(axis, effective_ratio)
                        .clamp(minimum_boundary, maximum_boundary);
                    let first_rect = rect
                        .first_child(axis, boundary)
                        .ok_or(LayoutError::InvalidSplit { axis })?;
                    let second_rect = rect
                        .second_child(axis, boundary)
                        .ok_or(LayoutError::InvalidSplit { axis })?;
                    splits.push(ResolvedSplit {
                        node: id,
                        parent_rect: rect,
                        preferred_ratio,
                        effective_ratio,
                        feasible_range: range,
                        boundary,
                    });
                    resolve_node(
                        tree,
                        first,
                        first_rect,
                        snapshots,
                        requirements,
                        overrides,
                        feasibility,
                        updates,
                        splits,
                        node_visits,
                    )?;
                    resolve_node(
                        tree,
                        second,
                        second_rect,
                        snapshots,
                        requirements,
                        overrides,
                        feasibility,
                        updates,
                        splits,
                        node_visits,
                    )?;
                    Ok(true)
                }
            }
        }
    }
}

fn infeasible_split(
    tree: &DwindleTree,
    node: DwindleNodeId,
    axis: SplitAxis,
    rect: LayoutRect,
    snapshots: &HashMap<WindowId, LayoutWindowSnapshot>,
) -> LayoutError {
    LayoutError::ConstraintInfeasible(ConstraintInfeasibility {
        node: Some(node),
        axis: Some(axis),
        required_width: 0,
        required_height: 0,
        available_width: rect.width(),
        available_height: rect.height(),
        windows: active_windows(tree, node, snapshots).unwrap_or_default(),
    })
}

fn rect_with_size(width: u32, height: u32) -> LayoutRect {
    LayoutRect::new(0, 0, width.max(1), height.max(1)).expect("positive feasibility rectangle")
}

fn active_windows(
    tree: &DwindleTree,
    id: DwindleNodeId,
    snapshots: &HashMap<WindowId, LayoutWindowSnapshot>,
) -> Result<Vec<WindowId>, LayoutError> {
    let mut windows = Vec::new();
    collect_active_windows(tree, id, snapshots, &mut windows)?;
    Ok(windows)
}

fn collect_active_windows(
    tree: &DwindleTree,
    id: DwindleNodeId,
    snapshots: &HashMap<WindowId, LayoutWindowSnapshot>,
    windows: &mut Vec<WindowId>,
) -> Result<(), LayoutError> {
    match tree.node_kind(id).ok_or(LayoutError::InvalidTree)? {
        DwindleNodeKind::Leaf { window } => {
            if !snapshots
                .get(&window)
                .ok_or(LayoutError::MissingSnapshot(window))?
                .minimized
            {
                windows.push(window);
            }
        }
        DwindleNodeKind::Split { first, second, .. } => {
            collect_active_windows(tree, first, snapshots, windows)?;
            collect_active_windows(tree, second, snapshots, windows)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::core::WindowId;
    use crate::wm::layout::constraints::LayoutConstraints;
    use crate::wm::layout::dwindle::{DwindleNodeKind, InsertHint};
    use crate::wm::layout::geometry::{LayoutRect, SplitRatio};
    use crate::wm::layout::plan::{LayoutWindowSnapshot, TiledLayoutManager};
    use crate::wm::{WorkspaceId, WorkspaceLocation};

    fn window(raw: u64) -> WindowId {
        WindowId::from_raw(raw).expect("non-zero test window id")
    }
    fn location() -> WorkspaceLocation {
        WorkspaceLocation::Regular(WorkspaceId::new(1).expect("non-zero workspace"))
    }

    #[test]
    fn derives_exact_boundary_requirements_for_independent_leaves() {
        let mut manager = TiledLayoutManager::default();
        let location = location();
        manager
            .insert(location, window(1), InsertHint::default())
            .expect("first leaf");
        let root = LayoutRect::new(0, 0, 1920, 1080).expect("root");
        manager
            .insert(
                location,
                window(2),
                InsertHint {
                    focused: Some(window(1)),
                    anchor_rect: Some(root),
                    ..InsertHint::default()
                },
            )
            .expect("second leaf");
        let root_id = manager.tree(location).unwrap().root().unwrap();
        manager
            .tree_mut(location)
            .set_split_ratio(root_id, 0.9)
            .expect("preferred ratio");
        let solution = manager
            .calculate_solution(
                location,
                root,
                &[
                    LayoutWindowSnapshot::new(window(1)).with_constraints(LayoutConstraints {
                        min_width: Some(700),
                        ..LayoutConstraints::default()
                    }),
                    LayoutWindowSnapshot::new(window(2)).with_constraints(LayoutConstraints {
                        min_width: Some(500),
                        ..LayoutConstraints::default()
                    }),
                ],
            )
            .expect("feasible solution");
        let split = solution.splits().first().expect("resolved root split");
        assert!((split.feasible_range().min().value() - (700.0 / 1920.0)).abs() < 0.0001);
        assert!((split.feasible_range().max().value() - (1420.0 / 1920.0)).abs() < 0.0001);
        assert!((split.preferred_ratio().value() - 0.9).abs() < 0.0001);
        assert_eq!(split.boundary(), 1420);
    }

    #[test]
    fn nested_aspect_constraints_participate_in_root_feasibility() {
        let mut manager = TiledLayoutManager::default();
        let location = location();
        let root = LayoutRect::new(0, 0, 240, 100).expect("root");
        for id in 1..=3 {
            let hint = if id == 1 {
                InsertHint::default()
            } else {
                InsertHint {
                    focused: Some(window(1)),
                    anchor_rect: Some(root),
                    ..InsertHint::default()
                }
            };
            manager.insert(location, window(id), hint).expect("leaf");
        }
        let exact = LayoutConstraints {
            min_aspect: Some(2.0),
            max_aspect: Some(2.0),
            ..LayoutConstraints::default()
        };
        let solution = manager
            .calculate_solution(
                location,
                root,
                &[
                    LayoutWindowSnapshot::new(window(1)),
                    LayoutWindowSnapshot::new(window(2)),
                    LayoutWindowSnapshot::new(window(3)).with_constraints(exact),
                ],
            )
            .expect("nested exact feasibility");
        assert!(!solution.splits().is_empty());
    }

    #[test]
    fn exact_feasibility_is_monotonic_for_a_nested_tree() {
        let mut manager = TiledLayoutManager::default();
        let location = location();
        let root = LayoutRect::new(0, 0, 80, 60).expect("root");
        manager
            .insert(location, window(1), InsertHint::default())
            .expect("first");
        manager
            .insert(
                location,
                window(2),
                InsertHint {
                    focused: Some(window(1)),
                    anchor_rect: Some(root),
                    ..InsertHint::default()
                },
            )
            .expect("second");
        let constraints = LayoutConstraints {
            base_width: Some(8),
            width_increment: Some(3),
            min_aspect: Some(16.0 / 9.0),
            max_aspect: Some(16.0 / 9.0),
            ..LayoutConstraints::default()
        };
        let snapshots = [
            LayoutWindowSnapshot::new(window(1)).with_constraints(constraints),
            LayoutWindowSnapshot::new(window(2)),
        ];
        assert!(
            manager
                .calculate_solution(location, root, &snapshots)
                .is_ok()
        );
        assert!(
            manager
                .calculate_solution(
                    location,
                    LayoutRect::new(0, 0, 100, 80).expect("larger"),
                    &snapshots
                )
                .is_ok()
        );
    }

    #[test]
    fn ratio_overrides_do_not_mutate_canonical_preferred_ratios() {
        let mut manager = TiledLayoutManager::default();
        let location = location();
        let root = LayoutRect::new(0, 0, 1000, 800).expect("root");
        manager
            .insert(location, window(1), InsertHint::default())
            .expect("first");
        manager
            .insert(
                location,
                window(2),
                InsertHint {
                    focused: Some(window(1)),
                    anchor_rect: Some(root),
                    ..InsertHint::default()
                },
            )
            .expect("second");
        let split = manager.tree(location).unwrap().root().unwrap();
        manager
            .tree_mut(location)
            .set_split_ratio(split, 0.25)
            .expect("canonical ratio");
        let solution = TiledLayoutManager::calculate_tree_with_ratio_overrides(
            manager.tree(location).unwrap(),
            location,
            root,
            &[
                LayoutWindowSnapshot::new(window(1)),
                LayoutWindowSnapshot::new(window(2)),
            ],
            &[super::RatioOverride::new(
                split,
                SplitRatio::new(0.8).expect("ratio"),
            )],
        )
        .expect("override solution");
        assert!((solution.splits()[0].preferred_ratio().value() - 0.8).abs() < 0.0001);
        assert!(matches!(
            manager.tree(location).unwrap().node_kind(split),
            Some(DwindleNodeKind::Split { ratio, .. }) if (ratio.value() - 0.25).abs() < 0.0001
        ));
    }

    #[test]
    fn deterministic_mutation_stress_repeatedly_validates_and_solves() {
        let mut manager = TiledLayoutManager::default();
        let location = location();
        let mut live = Vec::new();
        for step in 0..4_000u32 {
            let operation = (step.wrapping_mul(37).wrapping_add(11)) % 7;
            match operation {
                0..=2 if live.len() < 24 => {
                    let id = window(u64::from(step).saturating_add(1));
                    if manager.insert(location, id, InsertHint::default()).is_ok() {
                        live.push(id);
                        manager.tree(location).unwrap().debug_validate().unwrap();
                    }
                }
                3..=4 if !live.is_empty() => {
                    let index = usize::try_from(step).unwrap_or_default() % live.len();
                    let id = live.swap_remove(index);
                    manager.remove(location, id).expect("live removal");
                    manager
                        .tree(location)
                        .map(|tree| tree.debug_validate().unwrap());
                }
                5 => {
                    if let Some(root) = manager.tree(location).and_then(|tree| tree.root()) {
                        let ratio = 0.05 + f64::from(step % 90) / 100.0;
                        let _ = manager.tree_mut(location).set_split_ratio(root, ratio);
                    }
                }
                _ => {}
            }
            let snapshots = live
                .iter()
                .map(|id| {
                    LayoutWindowSnapshot::new(*id)
                        .with_minimized(step % 13 == 0 && id.get() % 2 == 0)
                })
                .collect::<Vec<_>>();
            let root = LayoutRect::new(0, 0, 800 + (step % 5) * 127, 600 + (step % 7) * 53)
                .expect("stress root");
            let _ = manager.calculate_solution(location, root, &snapshots);
            if let Some(tree) = manager.tree(location) {
                tree.debug_validate().unwrap();
            }
        }
    }

    #[test]
    fn large_synthetic_trees_keep_solution_visits_linear() {
        let location = location();
        for count in [1usize, 10, 50, 100, 500] {
            let mut manager = TiledLayoutManager::default();
            for raw in 1..=u64::try_from(count).expect("count fits") {
                let hint = manager
                    .tree(location)
                    .and_then(|tree| tree.leaves().get(tree.leaves().len() / 2).copied())
                    .map(|(anchor, _)| InsertHint {
                        focused: Some(anchor),
                        anchor_rect: Some(
                            LayoutRect::new(0, 0, 1_000_000_000, 1_000_000_000)
                                .expect("synthetic root"),
                        ),
                        ..InsertHint::default()
                    })
                    .unwrap_or_default();
                manager
                    .insert(location, window(raw), hint)
                    .expect("synthetic insert");
            }
            let snapshots = (1..=u64::try_from(count).expect("count fits"))
                .map(|raw| LayoutWindowSnapshot::new(window(raw)))
                .collect::<Vec<_>>();
            let solution = manager
                .calculate_solution(
                    location,
                    LayoutRect::new(0, 0, 1_000_000_000, 1_000_000_000).expect("synthetic root"),
                    &snapshots,
                )
                .expect("synthetic solution");
            assert_eq!(solution.len(), count);
            assert!(
                solution.node_visits() <= count.saturating_mul(4).saturating_add(2),
                "node visits must remain linear for {count} leaves"
            );
        }
    }
}
