use std::{collections::HashMap, num::NonZeroU64};

use crate::core::WindowId;
use crate::wm::WorkspaceLocation;

use super::constraints::LayoutConstraints;
use super::dwindle::{DwindleTree, DwindleTreeError, InsertHint};
use super::geometry::{LayoutRect, SplitAxis};
use super::solve::{ConstraintInfeasibility, RatioOverride, TiledLayoutSolution};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutWindowSnapshot {
    pub window: WindowId,
    pub minimized: bool,
    pub constraints: LayoutConstraints,
}

impl LayoutWindowSnapshot {
    pub const fn new(window: WindowId) -> Self {
        Self {
            window,
            minimized: false,
            constraints: LayoutConstraints {
                min_width: None,
                min_height: None,
                max_width: None,
                max_height: None,
                base_width: None,
                base_height: None,
                width_increment: None,
                height_increment: None,
                min_aspect: None,
                max_aspect: None,
            },
        }
    }

    pub const fn with_minimized(mut self, minimized: bool) -> Self {
        self.minimized = minimized;
        self
    }

    pub const fn with_constraints(mut self, constraints: LayoutConstraints) -> Self {
        self.constraints = constraints;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TiledWindowTarget {
    window: WindowId,
    tile: LayoutRect,
    client: LayoutRect,
}

impl TiledWindowTarget {
    pub const fn new(window: WindowId, tile: LayoutRect, client: LayoutRect) -> Self {
        Self {
            window,
            tile,
            client,
        }
    }

    pub const fn window(self) -> WindowId {
        self.window
    }

    pub const fn tile(self) -> LayoutRect {
        self.tile
    }

    pub const fn client(self) -> LayoutRect {
        self.client
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TiledLayoutPlan {
    location: WorkspaceLocation,
    updates: Vec<TiledWindowTarget>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LayoutGeneration(NonZeroU64);

impl LayoutGeneration {
    pub const INITIAL: Self = Self(NonZeroU64::new(1).expect("layout generation is non-zero"));

    pub const fn get(self) -> u64 {
        self.0.get()
    }

    pub fn next(self) -> Self {
        Self(NonZeroU64::new(self.get().saturating_add(1).max(1)).expect("non-zero generation"))
    }
}

impl Default for LayoutGeneration {
    fn default() -> Self {
        Self::INITIAL
    }
}

impl TiledLayoutPlan {
    pub(super) fn new(location: WorkspaceLocation, updates: Vec<TiledWindowTarget>) -> Self {
        Self { location, updates }
    }

    pub const fn location(&self) -> WorkspaceLocation {
        self.location
    }

    pub fn updates(&self) -> &[TiledWindowTarget] {
        &self.updates
    }

    pub fn len(&self) -> usize {
        self.updates.len()
    }

    pub fn is_empty(&self) -> bool {
        self.updates.is_empty()
    }

    pub fn target_for_window(&self, window: WindowId) -> Option<TiledWindowTarget> {
        self.updates
            .iter()
            .find(|target| target.window == window)
            .copied()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum LayoutError {
    MissingTree(WorkspaceLocation),
    MissingSnapshot(WindowId),
    DuplicateSnapshot(WindowId),
    InvalidTree,
    InvalidConstraints(WindowId),
    ConstraintViolation {
        window: WindowId,
        rect: LayoutRect,
        constraints: LayoutConstraints,
    },
    ConstraintInfeasible(ConstraintInfeasibility),
    InvalidSplit {
        axis: SplitAxis,
    },
    Tree(DwindleTreeError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TiledFallbackReason {
    ConstraintUpdate,
    WorkspaceMigration,
    WorkAreaShrink,
    InsertInfeasible,
}

impl From<DwindleTreeError> for LayoutError {
    fn from(error: DwindleTreeError) -> Self {
        Self::Tree(error)
    }
}

/// Lazy collection of independent Dwindle trees keyed by canonical location.
#[derive(Debug, Clone, Default)]
pub struct TiledLayoutManager {
    spaces: HashMap<WorkspaceLocation, DwindleTree>,
}

impl TiledLayoutManager {
    pub fn tree(&self, location: WorkspaceLocation) -> Option<&DwindleTree> {
        self.spaces.get(&location)
    }

    pub fn tree_mut(&mut self, location: WorkspaceLocation) -> &mut DwindleTree {
        self.spaces.entry(location).or_default()
    }

    pub fn locations(&self) -> impl Iterator<Item = WorkspaceLocation> + '_ {
        self.spaces.keys().copied()
    }

    pub fn insert(
        &mut self,
        location: WorkspaceLocation,
        window: WindowId,
        hint: InsertHint,
    ) -> Result<(), LayoutError> {
        if self.spaces.iter().any(|(existing_location, tree)| {
            *existing_location != location && tree.contains_window(window)
        }) {
            return Err(LayoutError::Tree(DwindleTreeError::DuplicateWindow(window)));
        }
        self.tree_mut(location).insert(window, hint)?;
        Ok(())
    }

    pub fn remove(
        &mut self,
        location: WorkspaceLocation,
        window: WindowId,
    ) -> Result<(), LayoutError> {
        let tree = self
            .spaces
            .get_mut(&location)
            .ok_or(LayoutError::MissingTree(location))?;
        tree.remove(window)?;
        if tree.is_empty() {
            self.spaces.remove(&location);
        }
        Ok(())
    }

    pub(crate) fn replace_tree(&mut self, location: WorkspaceLocation, tree: DwindleTree) {
        if tree.is_empty() {
            self.spaces.remove(&location);
        } else {
            self.spaces.insert(location, tree);
        }
    }

    pub fn calculate(
        &self,
        location: WorkspaceLocation,
        root: LayoutRect,
        snapshots: &[LayoutWindowSnapshot],
    ) -> Result<TiledLayoutSolution, LayoutError> {
        let Some(tree) = self.tree(location) else {
            return Ok(TiledLayoutSolution::empty(location));
        };
        Self::calculate_tree(tree, location, root, snapshots)
    }

    pub fn calculate_solution(
        &self,
        location: WorkspaceLocation,
        root: LayoutRect,
        snapshots: &[LayoutWindowSnapshot],
    ) -> Result<TiledLayoutSolution, LayoutError> {
        self.calculate(location, root, snapshots)
    }

    pub fn calculate_tree(
        tree: &DwindleTree,
        location: WorkspaceLocation,
        root: LayoutRect,
        snapshots: &[LayoutWindowSnapshot],
    ) -> Result<TiledLayoutSolution, LayoutError> {
        super::solve::solve_tree(tree, location, root, snapshots)
    }

    pub fn calculate_tree_with_ratio_overrides(
        tree: &DwindleTree,
        location: WorkspaceLocation,
        root: LayoutRect,
        snapshots: &[LayoutWindowSnapshot],
        overrides: &[RatioOverride],
    ) -> Result<TiledLayoutSolution, LayoutError> {
        super::solve::solve_tree_with_ratio_overrides(tree, location, root, snapshots, overrides)
    }
}

#[cfg(test)]
mod tests {
    use crate::core::WindowId;
    use crate::wm::{WorkspaceId, WorkspaceLocation};

    use super::{LayoutConstraints, LayoutWindowSnapshot, TiledLayoutManager};
    use crate::wm::layout::dwindle::InsertHint;
    use crate::wm::layout::geometry::LayoutRect;

    fn window(raw: u64) -> WindowId {
        WindowId::from_raw(raw).expect("non-zero test window id")
    }

    fn location(raw: u32) -> WorkspaceLocation {
        WorkspaceLocation::Regular(WorkspaceId::new(raw).expect("non-zero workspace"))
    }

    fn snapshot(window: WindowId) -> LayoutWindowSnapshot {
        LayoutWindowSnapshot::new(window)
    }

    fn area() -> LayoutRect {
        LayoutRect::new(0, 0, 101, 80).expect("valid area")
    }

    #[test]
    fn two_windows_partition_the_root_without_overlap_or_hole() {
        let mut manager = TiledLayoutManager::default();
        let loc = location(1);
        manager
            .insert(loc, window(1), InsertHint::default())
            .expect("first insert");
        manager
            .insert(
                loc,
                window(2),
                InsertHint {
                    focused: Some(window(1)),
                    anchor_rect: Some(area()),
                    ..InsertHint::default()
                },
            )
            .expect("second insert");

        let plan = manager
            .calculate(loc, area(), &[snapshot(window(1)), snapshot(window(2))])
            .expect("layout plan");
        assert_eq!(plan.len(), 2);
        let first = plan.target_for_window(window(1)).expect("first target");
        let second = plan.target_for_window(window(2)).expect("second target");
        assert_eq!(first.tile().right(), second.tile().x());
        assert_eq!(first.tile().width() + second.tile().width(), area().width());
        assert_eq!(first.tile().height(), area().height());
        assert_eq!(second.tile().height(), area().height());
    }

    #[test]
    fn minimized_leaf_collapses_without_destroying_topology() {
        let mut manager = TiledLayoutManager::default();
        let loc = location(1);
        manager
            .insert(loc, window(1), InsertHint::default())
            .expect("first insert");
        manager
            .insert(loc, window(2), InsertHint::default())
            .expect("second insert");

        let plan = manager
            .calculate(
                loc,
                area(),
                &[
                    snapshot(window(1)),
                    LayoutWindowSnapshot::new(window(2)).with_minimized(true),
                ],
            )
            .expect("collapsed plan");
        assert_eq!(plan.len(), 1);
        assert_eq!(plan.target_for_window(window(1)).unwrap().tile(), area());
        assert!(
            manager
                .tree(loc)
                .is_some_and(|tree| tree.contains_window(window(2)))
        );
    }

    #[test]
    fn impossible_leaf_constraints_are_rejected_without_partial_updates() {
        let mut manager = TiledLayoutManager::default();
        let loc = location(3);
        manager
            .insert(loc, window(1), InsertHint::default())
            .expect("first insert");
        manager
            .insert(
                loc,
                window(2),
                InsertHint {
                    focused: Some(window(1)),
                    anchor_rect: Some(area()),
                    ..InsertHint::default()
                },
            )
            .expect("second insert");

        let result = manager.calculate(
            loc,
            area(),
            &[
                LayoutWindowSnapshot::new(window(1)).with_constraints(LayoutConstraints {
                    min_width: Some(80),
                    ..LayoutConstraints::default()
                }),
                LayoutWindowSnapshot::new(window(2)).with_constraints(LayoutConstraints {
                    min_width: Some(80),
                    ..LayoutConstraints::default()
                }),
            ],
        );
        assert!(matches!(
            result,
            Err(super::LayoutError::ConstraintInfeasible(_))
        ));
    }

    #[test]
    fn locations_have_independent_lazy_trees() {
        let mut manager = TiledLayoutManager::default();
        let first = location(1);
        let second = location(2);
        manager
            .insert(first, window(1), InsertHint::default())
            .expect("first location insert");
        manager
            .insert(second, window(2), InsertHint::default())
            .expect("second location insert");

        assert_eq!(manager.tree(first).unwrap().len(), 1);
        assert_eq!(manager.tree(second).unwrap().len(), 1);
        assert!(!manager.tree(first).unwrap().contains_window(window(2)));
        assert!(!manager.tree(second).unwrap().contains_window(window(1)));

        manager.remove(first, window(1)).expect("remove first");
        assert!(manager.tree(first).is_none());
        assert!(manager.tree(second).is_some());
    }

    #[test]
    fn one_window_cannot_belong_to_two_location_trees() {
        let mut manager = TiledLayoutManager::default();
        let first = location(1);
        let second = location(2);
        manager
            .insert(first, window(1), InsertHint::default())
            .expect("first insert");
        assert!(matches!(
            manager.insert(second, window(1), InsertHint::default()),
            Err(super::LayoutError::Tree(
                super::DwindleTreeError::DuplicateWindow(_)
            ))
        ));
    }
}
