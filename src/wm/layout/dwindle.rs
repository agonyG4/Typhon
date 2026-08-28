use std::collections::{HashMap, HashSet};

use crate::core::WindowId;

use super::geometry::{LayoutPoint, LayoutRect, SplitAxis, SplitRatio};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DwindleNodeId {
    index: u32,
    generation: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DwindleNodeKind {
    Leaf {
        window: WindowId,
    },
    Split {
        axis: SplitAxis,
        ratio: SplitRatio,
        first: DwindleNodeId,
        second: DwindleNodeId,
    },
}

#[derive(Debug, Clone, Copy, Default)]
pub struct InsertHint {
    pub focused: Option<WindowId>,
    pub fallback: Option<WindowId>,
    pub pointer: Option<LayoutPoint>,
    pub anchor_rect: Option<LayoutRect>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DwindleTreeError {
    DuplicateWindow(WindowId),
    UnknownWindow(WindowId),
    UnknownNode,
    NodeIsNotSplit,
    InvalidRatio,
}

#[derive(Debug, Clone)]
struct DwindleNode {
    parent: Option<DwindleNodeId>,
    kind: DwindleNodeKind,
}

#[derive(Debug, Clone)]
struct ArenaSlot {
    generation: u32,
    node: Option<DwindleNode>,
}

#[derive(Debug, Clone, Default)]
struct DwindleArena {
    slots: Vec<ArenaSlot>,
    free: Vec<u32>,
}

impl DwindleArena {
    fn insert(&mut self, node: DwindleNode) -> DwindleNodeId {
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            debug_assert!(slot.node.is_none());
            slot.node = Some(node);
            return DwindleNodeId {
                index,
                generation: slot.generation,
            };
        }

        let index = u32::try_from(self.slots.len()).expect("Dwindle arena exhausted");
        self.slots.push(ArenaSlot {
            generation: 1,
            node: Some(node),
        });
        DwindleNodeId {
            index,
            generation: 1,
        }
    }

    fn get(&self, id: DwindleNodeId) -> Option<&DwindleNode> {
        self.slots
            .get(id.index as usize)
            .filter(|slot| slot.generation == id.generation)
            .and_then(|slot| slot.node.as_ref())
    }

    fn get_mut(&mut self, id: DwindleNodeId) -> Option<&mut DwindleNode> {
        self.slots
            .get_mut(id.index as usize)
            .filter(|slot| slot.generation == id.generation)
            .and_then(|slot| slot.node.as_mut())
    }

    fn remove(&mut self, id: DwindleNodeId) -> Option<DwindleNode> {
        let slot = self
            .slots
            .get_mut(id.index as usize)
            .filter(|slot| slot.generation == id.generation && slot.node.is_some())?;
        let node = slot.node.take();
        if slot.generation < u32::MAX {
            slot.generation += 1;
            self.free.push(id.index);
        }
        node
    }

    fn live_count(&self) -> usize {
        self.slots.iter().filter(|slot| slot.node.is_some()).count()
    }
}

/// Pure, arena-backed binary-space-partitioning topology for one location.
#[derive(Debug, Clone, Default)]
pub struct DwindleTree {
    root: Option<DwindleNodeId>,
    nodes: DwindleArena,
    by_window: HashMap<WindowId, DwindleNodeId>,
    insertion_order: Vec<WindowId>,
    topology_generation: u64,
}

impl DwindleTree {
    pub fn root(&self) -> Option<DwindleNodeId> {
        self.root
    }

    pub fn len(&self) -> usize {
        self.by_window.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_window.is_empty()
    }

    pub const fn topology_generation(&self) -> u64 {
        self.topology_generation
    }

    pub fn contains_window(&self, window: WindowId) -> bool {
        self.by_window.contains_key(&window)
    }

    pub fn leaf_for_window(&self, window: WindowId) -> Option<DwindleNodeId> {
        self.by_window.get(&window).copied()
    }

    pub fn node_kind(&self, id: DwindleNodeId) -> Option<DwindleNodeKind> {
        self.nodes.get(id).map(|node| node.kind)
    }

    pub fn set_split_ratio(
        &mut self,
        id: DwindleNodeId,
        ratio: f64,
    ) -> Result<(), DwindleTreeError> {
        let ratio = SplitRatio::new(ratio).ok_or(DwindleTreeError::InvalidRatio)?;
        let node = self
            .nodes
            .get_mut(id)
            .ok_or(DwindleTreeError::UnknownNode)?;
        let DwindleNodeKind::Split {
            ratio: preferred, ..
        } = &mut node.kind
        else {
            return Err(DwindleTreeError::NodeIsNotSplit);
        };
        *preferred = ratio;
        self.validate_after_mutation();
        Ok(())
    }

    pub fn parent_of(&self, id: DwindleNodeId) -> Option<Option<DwindleNodeId>> {
        self.nodes.get(id).map(|node| node.parent)
    }

    pub fn windows(&self) -> impl Iterator<Item = WindowId> + '_ {
        self.insertion_order.iter().copied()
    }

    pub fn leaves(&self) -> Vec<(WindowId, DwindleNodeId)> {
        self.insertion_order
            .iter()
            .filter_map(|window| self.by_window.get(window).map(|id| (*window, *id)))
            .collect()
    }

    pub fn insert(
        &mut self,
        window: WindowId,
        hint: InsertHint,
    ) -> Result<DwindleNodeId, DwindleTreeError> {
        if self.by_window.contains_key(&window) {
            return Err(DwindleTreeError::DuplicateWindow(window));
        }

        let leaf = self.nodes.insert(DwindleNode {
            parent: None,
            kind: DwindleNodeKind::Leaf { window },
        });
        if self.root.is_none() {
            self.root = Some(leaf);
            self.by_window.insert(window, leaf);
            self.insertion_order.push(window);
            self.topology_generation = self.topology_generation.saturating_add(1);
            self.validate_after_mutation();
            return Ok(leaf);
        }

        let anchor_window = hint
            .focused
            .filter(|candidate| self.contains_window(*candidate))
            .or_else(|| {
                hint.fallback
                    .filter(|candidate| self.contains_window(*candidate))
            })
            .or_else(|| self.first_leaf_window());
        let anchor_window = anchor_window.expect("non-empty tree has an anchor leaf");
        let anchor = self
            .by_window
            .get(&anchor_window)
            .copied()
            .expect("anchor window has a leaf");
        let parent = self.nodes.get(anchor).expect("anchor leaf is live").parent;
        let anchor_rect = hint
            .anchor_rect
            .unwrap_or_else(|| LayoutRect::new(0, 0, 1, 1).expect("unit area is valid"));
        let axis = if anchor_rect.width() >= anchor_rect.height() {
            SplitAxis::Horizontal
        } else {
            SplitAxis::Vertical
        };
        let new_first = pointer_prefers_first(hint.pointer, anchor_rect, axis).unwrap_or(false);
        let (first, second) = if new_first {
            (leaf, anchor)
        } else {
            (anchor, leaf)
        };
        let split = self.nodes.insert(DwindleNode {
            parent,
            kind: DwindleNodeKind::Split {
                axis,
                ratio: SplitRatio::DEFAULT,
                first,
                second,
            },
        });
        self.nodes
            .get_mut(first)
            .expect("new split first child is live")
            .parent = Some(split);
        self.nodes
            .get_mut(second)
            .expect("new split second child is live")
            .parent = Some(split);
        if let Some(parent) = parent {
            self.replace_child(parent, anchor, split);
        } else {
            self.root = Some(split);
        }

        self.by_window.insert(window, leaf);
        self.insertion_order.push(window);
        self.topology_generation = self.topology_generation.saturating_add(1);
        self.validate_after_mutation();
        Ok(leaf)
    }

    pub fn remove(&mut self, window: WindowId) -> Result<(), DwindleTreeError> {
        let leaf = self
            .by_window
            .get(&window)
            .copied()
            .ok_or(DwindleTreeError::UnknownWindow(window))?;
        let parent = self
            .nodes
            .get(leaf)
            .expect("window map points at a live leaf")
            .parent;
        self.by_window.remove(&window);
        self.insertion_order
            .retain(|candidate| *candidate != window);

        let Some(parent) = parent else {
            self.nodes.remove(leaf).expect("root leaf is live");
            self.root = None;
            self.topology_generation = self.topology_generation.saturating_add(1);
            self.validate_after_mutation();
            return Ok(());
        };

        let (first, second) = match self.nodes.get(parent).expect("leaf parent is live").kind {
            DwindleNodeKind::Split { first, second, .. } => (first, second),
            DwindleNodeKind::Leaf { .. } => unreachable!("leaf parent cannot be a leaf"),
        };
        let sibling = if first == leaf { second } else { first };
        let grandparent = self
            .nodes
            .get(parent)
            .expect("leaf parent remains live")
            .parent;
        self.nodes.remove(leaf).expect("leaf is live");
        self.nodes.remove(parent).expect("split parent is live");
        self.nodes.get_mut(sibling).expect("sibling is live").parent = grandparent;
        if let Some(grandparent) = grandparent {
            self.replace_child(grandparent, parent, sibling);
        } else {
            self.root = Some(sibling);
        }
        self.topology_generation = self.topology_generation.saturating_add(1);
        self.validate_after_mutation();
        Ok(())
    }

    pub fn debug_validate(&self) -> Result<(), String> {
        let Some(root) = self.root else {
            if self.nodes.live_count() != 0
                || !self.by_window.is_empty()
                || !self.insertion_order.is_empty()
            {
                return Err("empty tree retains live nodes or windows".to_string());
            }
            return Ok(());
        };
        if self.nodes.get(root).is_none() {
            return Err("root is stale".to_string());
        }
        if self.nodes.get(root).and_then(|node| node.parent).is_some() {
            return Err("root has a parent".to_string());
        }

        let mut visited = HashSet::new();
        let mut leaves = HashMap::new();
        self.validate_node(root, None, &mut visited, &mut leaves)?;
        if visited.len() != self.nodes.live_count() {
            return Err("tree contains an orphan live node".to_string());
        }
        if leaves != self.by_window {
            return Err("by_window does not match leaf membership".to_string());
        }
        if self.insertion_order.len() != leaves.len()
            || self.insertion_order.iter().collect::<HashSet<_>>().len()
                != self.insertion_order.len()
            || self
                .insertion_order
                .iter()
                .any(|window| !leaves.contains_key(window))
        {
            return Err("insertion order does not match leaves".to_string());
        }
        Ok(())
    }

    fn validate_node(
        &self,
        id: DwindleNodeId,
        expected_parent: Option<DwindleNodeId>,
        visited: &mut HashSet<DwindleNodeId>,
        leaves: &mut HashMap<WindowId, DwindleNodeId>,
    ) -> Result<(), String> {
        if !visited.insert(id) {
            return Err("cycle or duplicate child".to_string());
        }
        let node = self
            .nodes
            .get(id)
            .ok_or_else(|| "child points at a stale node".to_string())?;
        if node.parent != expected_parent {
            return Err("parent link mismatch".to_string());
        }
        match node.kind {
            DwindleNodeKind::Leaf { window } => {
                if leaves.insert(window, id).is_some() {
                    return Err("duplicate window leaf".to_string());
                }
            }
            DwindleNodeKind::Split {
                ratio,
                first,
                second,
                ..
            } => {
                if first == second || SplitRatio::new(ratio.value()).is_none() {
                    return Err("invalid split node".to_string());
                }
                self.validate_node(first, Some(id), visited, leaves)?;
                self.validate_node(second, Some(id), visited, leaves)?;
            }
        }
        Ok(())
    }

    fn replace_child(&mut self, parent: DwindleNodeId, old: DwindleNodeId, new: DwindleNodeId) {
        let node = self.nodes.get_mut(parent).expect("parent is live");
        let DwindleNodeKind::Split { first, second, .. } = &mut node.kind else {
            unreachable!("only split nodes have children");
        };
        if *first == old {
            *first = new;
        } else if *second == old {
            *second = new;
        } else {
            unreachable!("old child belongs to parent");
        }
    }

    fn first_leaf_window(&self) -> Option<WindowId> {
        let mut current = self.root?;
        loop {
            match self.nodes.get(current)?.kind {
                DwindleNodeKind::Leaf { window } => return Some(window),
                DwindleNodeKind::Split { first, .. } => current = first,
            }
        }
    }

    #[cfg(any(debug_assertions, test))]
    fn validate_after_mutation(&self) {
        debug_assert!(self.debug_validate().is_ok());
    }

    #[cfg(not(any(debug_assertions, test)))]
    const fn validate_after_mutation(&self) {}
}

fn pointer_prefers_first(
    pointer: Option<LayoutPoint>,
    rect: LayoutRect,
    axis: SplitAxis,
) -> Option<bool> {
    let pointer = pointer?;
    if pointer.x() < rect.x()
        || pointer.x() >= rect.right()
        || pointer.y() < rect.y()
        || pointer.y() >= rect.bottom()
    {
        return None;
    }
    let midpoint = match axis {
        SplitAxis::Horizontal => rect.x().saturating_add((rect.width() / 2) as i32),
        SplitAxis::Vertical => rect.y().saturating_add((rect.height() / 2) as i32),
    };
    Some(match axis {
        SplitAxis::Horizontal => pointer.x() < midpoint,
        SplitAxis::Vertical => pointer.y() < midpoint,
    })
}

#[cfg(test)]
mod tests {
    use crate::core::WindowId;

    use super::{DwindleNodeKind, DwindleTree, InsertHint};
    use crate::wm::layout::geometry::{LayoutPoint, LayoutRect, SplitAxis};

    fn window(raw: u64) -> WindowId {
        WindowId::from_raw(raw).expect("non-zero test window id")
    }

    fn work_area() -> LayoutRect {
        LayoutRect::new(0, 0, 100, 60).expect("valid test area")
    }

    #[test]
    fn first_insert_is_the_only_root_leaf() {
        let mut tree = DwindleTree::default();

        tree.insert(window(1), InsertHint::default())
            .expect("first insertion");

        let root = tree.root().expect("root leaf");
        let one = window(1);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree.leaf_for_window(one), Some(root));
        assert!(
            matches!(tree.node_kind(root), Some(DwindleNodeKind::Leaf { window }) if window == one)
        );
        tree.debug_validate().expect("valid one-leaf tree");
    }

    #[test]
    fn wide_anchor_inserts_left_and_right_leaves() {
        let mut tree = DwindleTree::default();
        tree.insert(window(1), InsertHint::default())
            .expect("first insertion");
        tree.insert(
            window(2),
            InsertHint {
                focused: Some(window(1)),
                anchor_rect: Some(work_area()),
                ..InsertHint::default()
            },
        )
        .expect("second insertion");

        let root = tree.root().expect("split root");
        assert!(matches!(
            tree.node_kind(root),
            Some(DwindleNodeKind::Split {
                axis: SplitAxis::Horizontal,
                ..
            })
        ));
        assert_eq!(tree.len(), 2);
        tree.debug_validate().expect("valid wide split");
    }

    #[test]
    fn tall_anchor_inserts_top_and_bottom_leaves() {
        let mut tree = DwindleTree::default();
        tree.insert(window(1), InsertHint::default())
            .expect("first insertion");
        tree.insert(
            window(2),
            InsertHint {
                focused: Some(window(1)),
                anchor_rect: Some(LayoutRect::new(0, 0, 60, 100).expect("tall area")),
                ..InsertHint::default()
            },
        )
        .expect("second insertion");

        let root = tree.root().expect("split root");
        assert!(matches!(
            tree.node_kind(root),
            Some(DwindleNodeKind::Split {
                axis: SplitAxis::Vertical,
                ..
            })
        ));
    }

    #[test]
    fn pointer_hint_controls_new_leaf_side() {
        let mut tree = DwindleTree::default();
        tree.insert(window(1), InsertHint::default())
            .expect("first insertion");
        tree.insert(
            window(2),
            InsertHint {
                focused: Some(window(1)),
                pointer: Some(LayoutPoint::new(90, 20)),
                anchor_rect: Some(work_area()),
                ..InsertHint::default()
            },
        )
        .expect("second insertion");

        let root = tree.root().expect("split root");
        let DwindleNodeKind::Split { first, second, .. } = tree.node_kind(root).expect("split")
        else {
            panic!("expected split root");
        };
        assert_eq!(tree.leaf_for_window(window(1)), Some(first));
        assert_eq!(tree.leaf_for_window(window(2)), Some(second));
    }

    #[test]
    fn removal_promotes_sibling_and_rejects_stale_ids() {
        let mut tree = DwindleTree::default();
        tree.insert(window(1), InsertHint::default())
            .expect("first insertion");
        let old_leaf = tree.leaf_for_window(window(1)).expect("leaf");
        tree.insert(window(2), InsertHint::default())
            .expect("second insertion");
        tree.remove(window(1)).expect("remove first leaf");

        let root = tree.root().expect("sibling promoted");
        assert_eq!(tree.leaf_for_window(window(2)), Some(root));
        assert!(tree.node_kind(old_leaf).is_none());
        tree.debug_validate().expect("valid promoted tree");

        tree.remove(window(2)).expect("remove last leaf");
        assert_eq!(tree.root(), None);
        assert_eq!(tree.len(), 0);
        tree.debug_validate().expect("valid empty tree");
    }

    #[test]
    fn duplicate_and_unknown_windows_are_safe_errors() {
        let mut tree = DwindleTree::default();
        tree.insert(window(1), InsertHint::default())
            .expect("first insertion");
        assert!(tree.insert(window(1), InsertHint::default()).is_err());
        assert!(tree.remove(window(99)).is_err());
        tree.debug_validate().expect("errors preserve invariants");
    }
}
