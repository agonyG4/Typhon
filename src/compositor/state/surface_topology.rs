use super::*;

/// Canonical and staged spatial/stack hierarchy for compositor surfaces.
///
/// Protocol relationship identity and pending positions remain owned by
/// `SurfaceTransactionState`; this owner stores only placement and stack order.
#[derive(Debug, Default)]
pub(in crate::compositor) struct SurfaceTopologyState {
    placements: HashMap<u32, SurfacePlacement>,
    committed_subsurface_stacks: HashMap<u32, Vec<u32>>,
    latched_subsurface_stacks: HashMap<u32, Vec<u32>>,
    pending_subsurface_stacks: HashMap<u32, Vec<u32>>,
}

impl SurfaceTopologyState {
    pub(in crate::compositor) fn placement(&self, surface_id: u32) -> SurfacePlacement {
        self.placements
            .get(&surface_id)
            .copied()
            .unwrap_or_default()
    }

    pub(super) fn set_placement(&mut self, surface_id: u32, placement: SurfacePlacement) -> bool {
        if self.placement(surface_id) == placement {
            return false;
        }
        if placement == SurfacePlacement::root() {
            self.placements.remove(&surface_id);
        } else {
            self.placements.insert(surface_id, placement);
        }
        true
    }

    pub(super) fn remove_placement(&mut self, surface_id: u32) -> bool {
        self.placements.remove(&surface_id).is_some()
    }

    pub(super) fn remove_placements_with_parent(&mut self, parent_surface_id: u32) {
        self.placements
            .retain(|_, placement| placement.parent_surface_id != Some(parent_surface_id));
    }

    pub(in crate::compositor) fn placement_count(&self) -> usize {
        self.placements.len()
    }

    pub(in crate::compositor) fn placement_surface_ids_for_root(
        &self,
        root_surface_id: u32,
    ) -> impl Iterator<Item = u32> + '_ {
        self.placements
            .keys()
            .copied()
            .filter(move |surface_id| self.root_surface_id(*surface_id) == root_surface_id)
    }

    pub(in crate::compositor) fn parent_surface_id(&self, surface_id: u32) -> Option<u32> {
        self.placement(surface_id).parent_surface_id
    }

    pub(in crate::compositor) fn root_surface_id(&self, surface_id: u32) -> u32 {
        let mut current = surface_id;
        for _ in 0..self.placements.len().saturating_add(1) {
            let Some(parent) = self
                .placement(current)
                .parent_surface_id
                .filter(|parent_id| *parent_id != current)
            else {
                return current;
            };
            current = parent;
        }
        surface_id
    }

    pub(in crate::compositor) fn is_descendant_of(
        &self,
        surface_id: u32,
        ancestor_surface_id: u32,
    ) -> bool {
        let mut current = surface_id;
        for _ in 0..self.placements.len().saturating_add(1) {
            if current == ancestor_surface_id {
                return true;
            }
            let Some(parent_surface_id) = self.parent_surface_id(current) else {
                return false;
            };
            if parent_surface_id == current {
                return false;
            }
            current = parent_surface_id;
        }
        false
    }

    // Preserve a captured-but-unpublished restack when the next child appears.
    // The baseline order is pending, then latched, then committed, then parent.
    fn pending_stack_for_parent(&mut self, parent_id: u32) -> &mut Vec<u32> {
        self.pending_subsurface_stacks
            .entry(parent_id)
            .or_insert_with(|| {
                self.latched_subsurface_stacks
                    .get(&parent_id)
                    .cloned()
                    .or_else(|| self.committed_subsurface_stacks.get(&parent_id).cloned())
                    .unwrap_or_else(|| vec![parent_id])
            })
    }

    pub(in crate::compositor) fn add_child_to_pending_stack(
        &mut self,
        parent_id: u32,
        surface_id: u32,
    ) {
        let stack = self.pending_stack_for_parent(parent_id);
        stack.retain(|id| *id == parent_id || *id != surface_id);
        if !stack.contains(&parent_id) {
            stack.insert(0, parent_id);
        }
        stack.push(surface_id);
    }

    pub(in crate::compositor) fn restack_pending_child(
        &mut self,
        surface_id: u32,
        parent_id: u32,
        reference_id: u32,
        above: bool,
    ) -> bool {
        if reference_id == surface_id {
            return false;
        }

        // Do not initialize or partially mutate pending state when the
        // structurally valid reference is absent from the current baseline.
        let reference_present = self
            .pending_subsurface_stacks
            .get(&parent_id)
            .or_else(|| self.latched_subsurface_stacks.get(&parent_id))
            .or_else(|| self.committed_subsurface_stacks.get(&parent_id))
            .is_some_and(|stack| stack.contains(&reference_id))
            || (reference_id == parent_id
                && !self.pending_subsurface_stacks.contains_key(&parent_id)
                && !self.latched_subsurface_stacks.contains_key(&parent_id)
                && !self.committed_subsurface_stacks.contains_key(&parent_id));
        if !reference_present {
            return false;
        }

        let stack = self.pending_stack_for_parent(parent_id);
        stack.retain(|id| *id == parent_id || *id != surface_id);
        if !stack.contains(&parent_id) {
            stack.insert(0, parent_id);
        }
        let Some(reference_index) = stack.iter().position(|id| *id == reference_id) else {
            return false;
        };
        let insert_index = if above {
            reference_index + 1
        } else {
            reference_index
        };
        stack.insert(insert_index.min(stack.len()), surface_id);
        true
    }

    pub(in crate::compositor) fn take_pending_stack_for_capture(
        &mut self,
        parent_id: u32,
    ) -> Option<Vec<u32>> {
        let stack = self.pending_subsurface_stacks.remove(&parent_id)?;
        self.latched_subsurface_stacks
            .insert(parent_id, stack.clone());
        Some(stack)
    }

    pub(in crate::compositor) fn commit_subsurface_stack(
        &mut self,
        parent_id: u32,
        mut stack: Vec<u32>,
    ) -> bool {
        if !stack.contains(&parent_id) {
            stack.insert(0, parent_id);
        }
        stack.dedup();
        let changed = self
            .committed_subsurface_stacks
            .get(&parent_id)
            .is_none_or(|current| *current != stack);
        self.committed_subsurface_stacks.insert(parent_id, stack);
        changed
    }

    pub(in crate::compositor) fn detach_child_from_stack_lineage(
        &mut self,
        parent_id: u32,
        surface_id: u32,
    ) {
        fn remove_from_stack(stacks: &mut HashMap<u32, Vec<u32>>, parent_id: u32, surface_id: u32) {
            let Some(stack) = stacks.get_mut(&parent_id) else {
                return;
            };
            stack.retain(|id| *id != surface_id);
            stack.dedup();
            if stack.len() <= 1 && stack.first().copied() == Some(parent_id) {
                stacks.remove(&parent_id);
            }
        }

        remove_from_stack(&mut self.committed_subsurface_stacks, parent_id, surface_id);
        remove_from_stack(&mut self.latched_subsurface_stacks, parent_id, surface_id);
        remove_from_stack(&mut self.pending_subsurface_stacks, parent_id, surface_id);
    }

    pub(in crate::compositor) fn cleanup_surface_stacks(
        &mut self,
        surface_id: u32,
        mut parent_is_live: impl FnMut(u32) -> bool,
    ) {
        self.committed_subsurface_stacks.remove(&surface_id);
        self.latched_subsurface_stacks.remove(&surface_id);
        self.pending_subsurface_stacks.remove(&surface_id);
        for stack in self.committed_subsurface_stacks.values_mut() {
            stack.retain(|id| *id != surface_id);
            stack.dedup();
        }
        for stack in self.pending_subsurface_stacks.values_mut() {
            stack.retain(|id| *id != surface_id);
            stack.dedup();
        }
        for stack in self.latched_subsurface_stacks.values_mut() {
            stack.retain(|id| *id != surface_id);
            stack.dedup();
        }
        self.committed_subsurface_stacks.retain(|parent_id, stack| {
            parent_is_live(*parent_id) && stack.iter().any(|id| id != parent_id)
        });
        self.pending_subsurface_stacks.retain(|parent_id, stack| {
            parent_is_live(*parent_id) && stack.iter().any(|id| id != parent_id)
        });
        self.latched_subsurface_stacks.retain(|parent_id, stack| {
            parent_is_live(*parent_id) && stack.iter().any(|id| id != parent_id)
        });
    }

    pub(in crate::compositor) fn append_surface_tree_order(
        &self,
        surface_id: u32,
        visible_ids: &HashSet<u32>,
        ordered_ids: &mut Vec<u32>,
    ) {
        if !visible_ids.contains(&surface_id) || ordered_ids.contains(&surface_id) {
            return;
        }

        if let Some(stack) = self.committed_subsurface_stacks.get(&surface_id) {
            for stacked_id in stack {
                if *stacked_id == surface_id {
                    ordered_ids.push(surface_id);
                } else {
                    self.append_surface_tree_order(*stacked_id, visible_ids, ordered_ids);
                }
            }
        } else {
            ordered_ids.push(surface_id);
        }

        let children = self
            .placements
            .iter()
            .filter_map(|(child_id, placement)| {
                (placement.parent_surface_id == Some(surface_id)
                    && visible_ids.contains(child_id)
                    && !ordered_ids.contains(child_id))
                .then_some(*child_id)
            })
            .collect::<Vec<_>>();
        for child_id in children {
            self.append_surface_tree_order(child_id, visible_ids, ordered_ids);
        }
    }

    #[cfg(test)]
    pub(in crate::compositor) fn committed_stack(&self, parent_id: u32) -> Option<&[u32]> {
        self.committed_subsurface_stacks
            .get(&parent_id)
            .map(Vec::as_slice)
    }

    #[cfg(test)]
    pub(in crate::compositor) fn latched_stack(&self, parent_id: u32) -> Option<&[u32]> {
        self.latched_subsurface_stacks
            .get(&parent_id)
            .map(Vec::as_slice)
    }

    #[cfg(test)]
    pub(in crate::compositor) fn pending_stack(&self, parent_id: u32) -> Option<&[u32]> {
        self.pending_subsurface_stacks
            .get(&parent_id)
            .map(Vec::as_slice)
    }

    #[cfg(debug_assertions)]
    pub(in crate::compositor) fn debug_assert_stack_invariants(&self) {
        for (parent_id, stack) in &self.committed_subsurface_stacks {
            Self::debug_assert_stack_invariant(*parent_id, stack);
        }
        for (parent_id, stack) in &self.latched_subsurface_stacks {
            Self::debug_assert_stack_invariant(*parent_id, stack);
        }
        for (parent_id, stack) in &self.pending_subsurface_stacks {
            Self::debug_assert_stack_invariant(*parent_id, stack);
        }
    }

    #[cfg(debug_assertions)]
    fn debug_assert_stack_invariant(parent_id: u32, stack: &[u32]) {
        let mut stack_ids = HashSet::new();
        debug_assert!(stack.iter().all(|surface_id| stack_ids.insert(*surface_id)));
        debug_assert_eq!(stack.iter().filter(|id| **id == parent_id).count(), 1);
    }

    pub(super) fn install_native_frame_placement_fixture(
        &mut self,
        placements: impl IntoIterator<Item = (u32, SurfacePlacement)>,
    ) {
        self.placements.clear();
        self.placements.extend(placements);
    }

    #[cfg(test)]
    pub(in crate::compositor) fn placement_entry(
        &self,
        surface_id: u32,
    ) -> Option<SurfacePlacement> {
        self.placements.get(&surface_id).copied()
    }

    #[cfg(test)]
    pub(in crate::compositor) fn install_stack_fixture(
        &mut self,
        committed: impl IntoIterator<Item = (u32, Vec<u32>)>,
        latched: impl IntoIterator<Item = (u32, Vec<u32>)>,
        pending: impl IntoIterator<Item = (u32, Vec<u32>)>,
    ) {
        self.committed_subsurface_stacks = committed.into_iter().collect();
        self.latched_subsurface_stacks = latched.into_iter().collect();
        self.pending_subsurface_stacks = pending.into_iter().collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_and_descendant_queries_are_bounded_for_parent_cycles() {
        let mut topology = SurfaceTopologyState::default();
        topology.set_placement(1, SurfacePlacement::subsurface(2, 0, 0));
        topology.set_placement(2, SurfacePlacement::subsurface(1, 0, 0));

        assert_eq!(topology.root_surface_id(1), 1);
        assert!(topology.is_descendant_of(1, 1));
        assert!(!topology.is_descendant_of(1, 3));
    }

    #[test]
    fn native_frame_fixture_preserves_explicit_default_placement_entries() {
        let mut topology = SurfaceTopologyState::default();
        topology.install_native_frame_placement_fixture([(1, SurfacePlacement::root())]);

        assert_eq!(topology.placement_entry(1), Some(SurfacePlacement::root()));
    }

    #[test]
    fn pending_stack_uses_latched_baseline_before_committed_stack() {
        let mut topology = SurfaceTopologyState::default();
        topology.install_stack_fixture(
            [(1, vec![1, 2, 3])],
            [(1, vec![1, 3, 2])],
            std::iter::empty(),
        );

        topology.add_child_to_pending_stack(1, 4);

        assert_eq!(topology.latched_stack(1), Some(&[1, 3, 2][..]));
        assert_eq!(topology.pending_stack(1), Some(&[1, 3, 2, 4][..]));
    }

    #[test]
    fn adding_a_child_preserves_pending_restack_and_does_not_duplicate_child() {
        let mut topology = SurfaceTopologyState::default();
        topology.install_stack_fixture([], [], [(1, vec![1, 3, 2])]);

        topology.add_child_to_pending_stack(1, 4);
        topology.add_child_to_pending_stack(1, 4);

        assert_eq!(topology.pending_stack(1), Some(&[1, 3, 2, 4][..]));
    }

    #[test]
    fn committed_tree_order_falls_back_to_visible_placed_children() {
        let mut topology = SurfaceTopologyState::default();
        topology.set_placement(2, SurfacePlacement::subsurface(1, 0, 0));
        topology.set_placement(3, SurfacePlacement::subsurface(1, 0, 0));
        topology.commit_subsurface_stack(1, vec![1, 2]);
        let visible = HashSet::from([1, 2, 3]);
        let mut order = Vec::new();

        topology.append_surface_tree_order(1, &visible, &mut order);

        assert_eq!(order, vec![1, 2, 3]);
    }
}
