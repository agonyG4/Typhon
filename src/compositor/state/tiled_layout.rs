use super::*;

use std::collections::HashMap;

use crate::wm::layout::{
    DwindleTree, InsertHint, LayoutConstraints, LayoutError, LayoutPoint, LayoutRect,
    LayoutWindowSnapshot, TiledFallbackReason, TiledLayoutManager, TiledLayoutSolution,
};
use crate::wm::{LayoutMembership, WorkspaceLocation};

#[derive(Debug)]
pub(in crate::compositor) struct PreparedTiledMigration {
    candidate_trees: HashMap<WorkspaceLocation, DwindleTree>,
    final_solutions: HashMap<WorkspaceLocation, TiledLayoutSolution>,
    affected_locations: Vec<WorkspaceLocation>,
    pub(in crate::compositor) fallback_windows: Vec<WindowId>,
    floating_restores: Vec<(WindowId, WindowGeometry)>,
}

#[derive(Debug)]
struct PreparedLocationReflow {
    location: WorkspaceLocation,
    candidate_tree: DwindleTree,
    final_solution: TiledLayoutSolution,
    fallback_windows: Vec<WindowId>,
    floating_restores: Vec<(WindowId, WindowGeometry)>,
}

impl CompositorState {
    pub(in crate::compositor) fn toggle_focused_tiled_layout(&mut self) -> bool {
        let Some(window_id) = self.focused_window_id else {
            return false;
        };
        let Some(window) = self.window(window_id).cloned() else {
            return false;
        };
        if !window.is_workspace_managed()
            || window.state.mode() != ToplevelMode::Normal
            || window.state.is_minimized()
        {
            return false;
        }
        let Some(management) = window.management else {
            return false;
        };
        match management.layout() {
            LayoutMembership::Floating => self.toggle_window_to_tiled(window_id, management),
            LayoutMembership::Tiled => self.toggle_window_to_floating(window_id, management),
        }
    }

    fn toggle_window_to_tiled(
        &mut self,
        window_id: WindowId,
        management: crate::wm::WindowManagementState,
    ) -> bool {
        let location = management.location();
        let root = self.layout_root_rect();
        let mut snapshots = self.layout_snapshots(location);
        let Some(window) = self.window(window_id).cloned() else {
            return false;
        };
        snapshots.push(snapshot_for_window(&window));
        let hint = self.layout_insert_hint(location, root);
        let current_geometry = self
            .current_visual_root_window_geometry(window.root_surface_id)
            .or_else(|| self.current_root_window_geometry(window.root_surface_id));

        if self.tiled_layout.insert(location, window_id, hint).is_err() {
            return false;
        }
        let plan = match self.tiled_layout.calculate(location, root, &snapshots) {
            Ok(plan) => plan,
            Err(_) => {
                let _ = self.tiled_layout.remove(location, window_id);
                return false;
            }
        };
        if let Some(current_geometry) = current_geometry
            && let Some(window) = self.window_mut(window_id)
        {
            window.floating_geometry = Some(current_geometry);
        }
        if let Some(window) = self.window_mut(window_id) {
            window.management = Some(management.with_layout(LayoutMembership::Tiled));
        }
        self.mark_astrea_toplevel_dirty(window_id);
        self.apply_tiled_layout_plan(plan, None);
        true
    }

    fn toggle_window_to_floating(
        &mut self,
        window_id: WindowId,
        management: crate::wm::WindowManagementState,
    ) -> bool {
        let location = management.location();
        let root = self.layout_root_rect();
        let Some(window) = self.window(window_id).cloned() else {
            return false;
        };
        let floating_geometry = window.floating_geometry;
        let original_layout = self.tiled_layout.clone();
        if self.tiled_layout.remove(location, window_id).is_err() {
            return false;
        }
        let snapshots = self.layout_snapshots(location);
        let plan = match self.tiled_layout.calculate(location, root, &snapshots) {
            Ok(plan) => plan,
            Err(_) => {
                self.tiled_layout = original_layout;
                return false;
            }
        };
        if let Some(window) = self.window_mut(window_id) {
            window.management = Some(management.with_layout(LayoutMembership::Floating));
        }
        self.mark_astrea_toplevel_dirty(window_id);
        self.apply_tiled_layout_plan(
            plan,
            floating_geometry.map(|geometry| (window_id, geometry)),
        );
        true
    }

    pub(in crate::compositor) fn reflow_tiled_location(
        &mut self,
        location: WorkspaceLocation,
    ) -> bool {
        self.reflow_tiled_locations(std::slice::from_ref(&location))
    }

    fn reflow_tiled_locations(&mut self, locations: &[WorkspaceLocation]) -> bool {
        let root = self.layout_root_rect();
        let mut plans = Vec::with_capacity(locations.len());
        for location in locations {
            let snapshots = self.layout_snapshots(*location);
            let Ok(plan) = self.tiled_layout.calculate(*location, root, &snapshots) else {
                return false;
            };
            plans.push(plan);
        }

        self.begin_layout_reflow_batch();
        let mut changed = false;
        for plan in plans {
            changed |= self.apply_tiled_layout_plan_inner(plan, None, false);
        }
        if changed {
            self.refresh_active_scene_surface_order();
        }
        let scene_effect = self.finish_layout_reflow_batch();
        changed || scene_effect
    }

    pub(in crate::compositor) fn current_tiled_geometry(
        &self,
        window_id: WindowId,
    ) -> Option<WindowGeometry> {
        let location = self
            .window(window_id)
            .and_then(|window| window.management)
            .filter(|management| management.layout() == LayoutMembership::Tiled)
            .map(|management| management.location())?;
        let plan = self
            .tiled_layout
            .calculate(
                location,
                self.layout_root_rect(),
                &self.layout_snapshots(location),
            )
            .ok()?;
        let target = plan.target_for_window(window_id)?;
        Some(WindowGeometry::new(
            SurfacePlacement::absolute_root_at(target.client().x(), target.client().y()),
            target.client().width(),
            target.client().height(),
        ))
    }

    pub(in crate::compositor) fn remove_tiled_window_from_layout(
        &mut self,
        window_id: WindowId,
    ) -> bool {
        let Some(location) = self
            .window(window_id)
            .and_then(|window| window.management)
            .filter(|management| management.layout() == LayoutMembership::Tiled)
            .map(|management| management.location())
        else {
            return false;
        };
        self.cancel_tiled_resize_for_location(location, WindowInteractionEndReason::ExplicitCancel);
        if self.tiled_layout.remove(location, window_id).is_err() {
            return false;
        }
        if self.location_is_visible_for_layout(location) {
            let _ = self.reflow_tiled_location(location);
            self.tiled_layout_dirty.remove(&location);
        } else if self.tiled_layout.tree(location).is_none() {
            self.tiled_layout_dirty.remove(&location);
        } else {
            self.tiled_layout_dirty.insert(location);
        }
        true
    }

    pub(in crate::compositor) fn reconcile_tiled_constraints(
        &mut self,
        window_id: WindowId,
    ) -> bool {
        let Some(location) = self
            .window(window_id)
            .and_then(|window| window.management)
            .filter(|management| management.layout() == LayoutMembership::Tiled)
            .map(|management| management.location())
        else {
            return false;
        };
        self.cancel_tiled_resize_for_location(location, WindowInteractionEndReason::ExplicitCancel);
        let root = self.layout_root_rect();
        let snapshots = self.layout_snapshots(location);
        match self.tiled_layout.calculate(location, root, &snapshots) {
            Ok(solution) => {
                self.resize_flow_metrics.tiled_constraint_reflows = self
                    .resize_flow_metrics
                    .tiled_constraint_reflows
                    .saturating_add(1);
                if self.location_is_visible_for_layout(location) {
                    self.apply_tiled_layout_plan(solution, None)
                } else {
                    self.tiled_layout_dirty.insert(location);
                    true
                }
            }
            Err(LayoutError::ConstraintInfeasible(_)) => {
                self.auto_float_tiled_window(window_id, TiledFallbackReason::ConstraintUpdate)
            }
            Err(_) => false,
        }
    }

    fn auto_float_tiled_window(
        &mut self,
        window_id: WindowId,
        reason: TiledFallbackReason,
    ) -> bool {
        let Some(window) = self.window(window_id).cloned() else {
            return false;
        };
        let Some(management) = window
            .management
            .filter(|management| management.layout() == LayoutMembership::Tiled)
        else {
            return false;
        };
        let location = management.location();
        let fallback_root = self.layout_root_rect();
        let restore = window
            .floating_geometry
            .or_else(|| {
                self.current_visual_root_window_geometry(window.root_surface_id)
                    .or_else(|| self.current_root_window_geometry(window.root_surface_id))
            })
            .or_else(|| {
                Some(WindowGeometry::new(
                    SurfacePlacement::absolute_root_at(fallback_root.x(), fallback_root.y()),
                    fallback_root.width(),
                    fallback_root.height(),
                ))
            });
        let Some(mut candidate_tree) = self.tiled_layout.tree(location).cloned() else {
            return false;
        };
        if candidate_tree.remove(window_id).is_err() {
            return false;
        }
        let snapshots = self.candidate_layout_snapshots_for_tree(&candidate_tree);
        let Ok(solution) = TiledLayoutManager::calculate_tree(
            &candidate_tree,
            location,
            fallback_root,
            &snapshots,
        ) else {
            return false;
        };
        self.cancel_tiled_resize_for_location(location, WindowInteractionEndReason::ExplicitCancel);
        self.tiled_layout.replace_tree(location, candidate_tree);
        if let Some(window) = self.window_mut(window_id) {
            window.management = Some(management.with_layout(LayoutMembership::Floating));
            if window.floating_geometry.is_none() {
                window.floating_geometry = restore;
            }
        }
        self.record_tiled_fallback(reason);
        self.mark_astrea_toplevel_dirty(window_id);
        if self.location_is_visible_for_layout(location) {
            self.apply_tiled_layout_plan(solution, restore.map(|geometry| (window_id, geometry)))
        } else {
            self.tiled_layout_dirty.insert(location);
            true
        }
    }

    pub(in crate::compositor) fn migrate_tiled_layouts(
        &mut self,
        changes: &[(WindowId, WorkspaceLocation, WorkspaceLocation)],
    ) -> Option<PreparedTiledMigration> {
        let mut affected = Vec::new();
        let mut incoming_by_location: HashMap<WorkspaceLocation, Vec<WindowId>> = HashMap::new();
        for (window_id, previous, new) in changes {
            if previous == new
                || !self
                    .tiled_layout
                    .tree(*previous)
                    .is_some_and(|tree| tree.contains_window(*window_id))
            {
                continue;
            }
            if !affected.contains(previous) {
                affected.push(*previous);
            }
            if !affected.contains(new) {
                affected.push(*new);
            }
            incoming_by_location
                .entry(*new)
                .or_default()
                .push(*window_id);
        }
        if affected.is_empty() {
            return Some(PreparedTiledMigration {
                candidate_trees: HashMap::new(),
                final_solutions: HashMap::new(),
                affected_locations: Vec::new(),
                fallback_windows: Vec::new(),
                floating_restores: Vec::new(),
            });
        }
        for incoming in incoming_by_location.values_mut() {
            incoming.sort_unstable();
        }

        // Only source/destination trees participate in this candidate.
        let mut candidate_trees = affected
            .iter()
            .copied()
            .map(|location| {
                (
                    location,
                    self.tiled_layout
                        .tree(location)
                        .cloned()
                        .unwrap_or_default(),
                )
            })
            .collect::<HashMap<_, _>>();
        for (window_id, previous, new) in changes {
            if previous == new
                || !self
                    .tiled_layout
                    .tree(*previous)
                    .is_some_and(|tree| tree.contains_window(*window_id))
            {
                continue;
            }
            let source = candidate_trees.get_mut(previous)?;
            if source.remove(*window_id).is_err() {
                return None;
            }
            let destination = candidate_trees.entry(*new).or_default();
            if destination
                .insert(*window_id, InsertHint::default())
                .is_err()
            {
                return None;
            }
        }
        let root = self.layout_root_rect();
        let mut fallback_windows = Vec::new();
        for location in affected.iter().copied() {
            loop {
                let tree = candidate_trees.get(&location)?;
                let snapshots = self.candidate_layout_snapshots_for_tree(tree);
                let feasible =
                    TiledLayoutManager::calculate_tree(tree, location, root, &snapshots).is_ok();
                if feasible {
                    break;
                }
                let window_id = incoming_by_location.get(&location).and_then(|windows| {
                    windows.iter().rev().copied().find(|window_id| {
                        candidate_trees
                            .get(&location)
                            .is_some_and(|tree| tree.contains_window(*window_id))
                    })
                })?;
                if candidate_trees
                    .get_mut(&location)
                    .is_none_or(|tree| tree.remove(window_id).is_err())
                {
                    return None;
                }
                fallback_windows.push(window_id);
            }
        }
        let mut final_solutions = HashMap::new();
        for location in affected.iter().copied() {
            let tree = candidate_trees.get(&location)?;
            let snapshots = self.candidate_layout_snapshots_for_tree(tree);
            let Ok(solution) = TiledLayoutManager::calculate_tree(tree, location, root, &snapshots)
            else {
                return None;
            };
            final_solutions.insert(location, solution);
        }
        fallback_windows.sort_unstable();
        fallback_windows.dedup();
        let floating_restores = fallback_windows
            .iter()
            .filter_map(|window_id| {
                let window = self.window(*window_id)?;
                let geometry = window
                    .floating_geometry
                    .or_else(|| self.current_visual_root_window_geometry(window.root_surface_id))
                    .or_else(|| self.current_root_window_geometry(window.root_surface_id))
                    .or_else(|| {
                        Some(WindowGeometry::new(
                            SurfacePlacement::absolute_root_at(root.x(), root.y()),
                            root.width(),
                            root.height(),
                        ))
                    })?;
                Some((*window_id, geometry))
            })
            .collect::<Vec<_>>();
        Some(PreparedTiledMigration {
            candidate_trees,
            final_solutions,
            affected_locations: affected,
            fallback_windows,
            floating_restores,
        })
    }

    pub(in crate::compositor) fn commit_prepared_tiled_migration(
        &mut self,
        prepared: &PreparedTiledMigration,
    ) {
        for location in &prepared.affected_locations {
            self.cancel_tiled_resize_for_location(
                *location,
                WindowInteractionEndReason::ExplicitCancel,
            );
        }
        for (location, tree) in &prepared.candidate_trees {
            self.tiled_layout.replace_tree(*location, tree.clone());
        }
        for window_id in &prepared.fallback_windows {
            if let Some(window) = self.window_mut(*window_id) {
                if let Some(restore) = prepared
                    .floating_restores
                    .iter()
                    .find(|(id, _)| id == window_id)
                    .map(|(_, geometry)| *geometry)
                {
                    window.floating_geometry = Some(restore);
                }
                if let Some(management) = window.management {
                    window.management = Some(management.with_layout(LayoutMembership::Floating));
                }
            }
            self.record_tiled_fallback(TiledFallbackReason::WorkspaceMigration);
            self.mark_astrea_toplevel_dirty(*window_id);
        }
    }

    pub(in crate::compositor) fn apply_prepared_tiled_migration(
        &mut self,
        prepared: PreparedTiledMigration,
    ) -> bool {
        self.begin_layout_reflow_batch();
        let mut changed = false;
        for location in &prepared.affected_locations {
            if self.location_is_visible_for_layout(*location) {
                if let Some(solution) = prepared.final_solutions.get(location) {
                    changed |= self.apply_tiled_layout_plan_inner(solution.clone(), None, false);
                }
                for (window_id, geometry) in &prepared.floating_restores {
                    if self.window(*window_id).is_some_and(|window| {
                        window.management.is_some_and(|management| {
                            management.location() == *location
                                && management.layout() == LayoutMembership::Floating
                        })
                    }) {
                        let Some(window) = self.window(*window_id).cloned() else {
                            continue;
                        };
                        self.apply_layout_geometry(*window_id, window.backend, *geometry);
                        changed = true;
                    }
                }
            } else {
                self.tiled_layout_dirty.insert(*location);
                for (window_id, geometry) in &prepared.floating_restores {
                    if self.window(*window_id).is_some_and(|window| {
                        window.management.is_some_and(|management| {
                            management.location() == *location
                                && management.layout() == LayoutMembership::Floating
                        })
                    }) {
                        self.tiled_floating_restores.insert(*window_id, *geometry);
                    }
                }
            }
        }
        if changed {
            self.refresh_active_scene_surface_order();
        }
        let scene_effect = self.finish_layout_reflow_batch();
        changed || scene_effect
    }

    fn record_tiled_fallback(&mut self, reason: TiledFallbackReason) {
        match reason {
            TiledFallbackReason::ConstraintUpdate => {
                self.resize_flow_metrics.tiled_constraint_auto_floats = self
                    .resize_flow_metrics
                    .tiled_constraint_auto_floats
                    .saturating_add(1);
            }
            TiledFallbackReason::WorkspaceMigration => {
                self.resize_flow_metrics.tiled_migration_fallbacks = self
                    .resize_flow_metrics
                    .tiled_migration_fallbacks
                    .saturating_add(1);
            }
            TiledFallbackReason::WorkAreaShrink | TiledFallbackReason::InsertInfeasible => {}
        }
    }

    fn candidate_layout_snapshots_for_tree(&self, tree: &DwindleTree) -> Vec<LayoutWindowSnapshot> {
        tree.windows()
            .filter_map(|window_id| self.window(window_id).map(snapshot_for_window))
            .collect()
    }

    pub(in crate::compositor) fn reflow_usable_output_geometry(&mut self) -> bool {
        self.begin_layout_reflow_batch();
        if let Some(session) = self.tiled_resize_session {
            self.cancel_tiled_resize_for_location(
                session.location,
                WindowInteractionEndReason::WorkAreaChange,
            );
        }
        let locations = self.tiled_layout.locations().collect::<Vec<_>>();
        let location_count = locations.len();
        self.reconfigure_stateful_windows_for_output_size();
        let root = self.layout_root_rect();
        let mut prepared = Vec::new();
        for location in locations {
            if !self.location_is_visible_for_layout(location) {
                self.tiled_layout_dirty.insert(location);
                continue;
            }
            let Some(candidate) = self.prepare_location_reflow(location, root) else {
                let scene_effect = self.finish_layout_reflow_batch();
                return scene_effect;
            };
            prepared.push(candidate);
        }

        for candidate in &prepared {
            self.tiled_layout
                .replace_tree(candidate.location, candidate.candidate_tree.clone());
            for window_id in &candidate.fallback_windows {
                if let Some(window) = self.window_mut(*window_id) {
                    if let Some(restore) = candidate
                        .floating_restores
                        .iter()
                        .find(|(id, _)| id == window_id)
                        .map(|(_, geometry)| *geometry)
                    {
                        window.floating_geometry = Some(restore);
                    }
                    if let Some(management) = window.management {
                        window.management =
                            Some(management.with_layout(LayoutMembership::Floating));
                    }
                }
                self.record_tiled_fallback(TiledFallbackReason::WorkAreaShrink);
                self.mark_astrea_toplevel_dirty(*window_id);
            }
        }

        let mut changed = false;
        for candidate in prepared {
            changed |= self.apply_tiled_layout_plan_inner(candidate.final_solution, None, false);
            for (window_id, geometry) in candidate.floating_restores {
                if self.window(window_id).is_some_and(|window| {
                    window.management.is_some_and(|management| {
                        management.location() == candidate.location
                            && management.layout() == LayoutMembership::Floating
                    })
                }) {
                    let Some(window) = self.window(window_id).cloned() else {
                        continue;
                    };
                    self.apply_layout_geometry(window_id, window.backend, geometry);
                    changed = true;
                }
            }
        }
        if changed {
            self.refresh_active_scene_surface_order();
        }
        let scene_effect = self.finish_layout_reflow_batch();
        self.resize_flow_metrics.tiled_work_area_reflows = self
            .resize_flow_metrics
            .tiled_work_area_reflows
            .saturating_add(location_count as u64);
        changed || scene_effect
    }

    fn prepare_location_reflow(
        &self,
        location: WorkspaceLocation,
        root: LayoutRect,
    ) -> Option<PreparedLocationReflow> {
        let mut candidate_tree = self.tiled_layout.tree(location)?.clone();
        let mut fallback_windows = Vec::new();
        loop {
            let snapshots = self.candidate_layout_snapshots_for_tree(&candidate_tree);
            match TiledLayoutManager::calculate_tree(&candidate_tree, location, root, &snapshots) {
                Ok(final_solution) => {
                    let floating_restores = fallback_windows
                        .iter()
                        .filter_map(|window_id| {
                            let window = self.window(*window_id)?;
                            let geometry = window
                                .floating_geometry
                                .or_else(|| {
                                    self.current_visual_root_window_geometry(window.root_surface_id)
                                })
                                .or_else(|| {
                                    self.current_root_window_geometry(window.root_surface_id)
                                })
                                .or_else(|| {
                                    Some(WindowGeometry::new(
                                        SurfacePlacement::absolute_root_at(root.x(), root.y()),
                                        root.width(),
                                        root.height(),
                                    ))
                                })?;
                            Some((*window_id, geometry))
                        })
                        .collect();
                    return Some(PreparedLocationReflow {
                        location,
                        candidate_tree,
                        final_solution,
                        fallback_windows,
                        floating_restores,
                    });
                }
                Err(LayoutError::ConstraintInfeasible(witness)) => {
                    let window_id = candidate_tree
                        .leaves()
                        .into_iter()
                        .rev()
                        .map(|(window_id, _)| window_id)
                        .find(|window_id| witness.windows.contains(window_id))?;
                    if candidate_tree.remove(window_id).is_err() {
                        return None;
                    }
                    fallback_windows.push(window_id);
                }
                Err(_) => return None,
            }
        }
    }

    pub(in crate::compositor) fn layout_root_rect(&self) -> LayoutRect {
        let usable = self.usable_output_geometry();
        let x = if usable.x.is_finite() {
            usable
                .x
                .round()
                .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
        } else {
            0
        };
        let y = if usable.y.is_finite() {
            usable
                .y
                .round()
                .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
        } else {
            0
        };
        let width = if usable.width.is_finite() {
            usable.width.round().clamp(1.0, f64::from(u32::MAX)) as u32
        } else {
            1
        };
        let height = if usable.height.is_finite() {
            usable.height.round().clamp(1.0, f64::from(u32::MAX)) as u32
        } else {
            1
        };
        LayoutRect::new(x, y, width, height).expect("layout root dimensions are positive")
    }

    pub(in crate::compositor) fn layout_snapshots(
        &self,
        location: WorkspaceLocation,
    ) -> Vec<LayoutWindowSnapshot> {
        self.tiled_layout
            .tree(location)
            .into_iter()
            .flat_map(|tree| tree.windows())
            .filter_map(|window_id| {
                let window = self.window(window_id)?;
                debug_assert!(window.management.is_some_and(|management| {
                    management.location() == location
                        && management.layout() == LayoutMembership::Tiled
                }));
                Some(snapshot_for_window(window))
            })
            .collect()
    }

    fn layout_insert_hint(&self, location: WorkspaceLocation, root: LayoutRect) -> InsertHint {
        let fallback = self
            .window_stacking
            .iter()
            .rev()
            .copied()
            .find(|window_id| {
                self.window(*window_id).is_some_and(|window| {
                    window.management.is_some_and(|management| {
                        management.location() == location
                            && management.layout() == LayoutMembership::Tiled
                    })
                })
            });
        let anchor_window = self
            .focused_window_id
            .filter(|window_id| {
                self.tiled_layout
                    .tree(location)
                    .is_some_and(|tree| tree.contains_window(*window_id))
            })
            .or(fallback);
        let anchor_rect = anchor_window
            .and_then(|window_id| {
                self.tiled_layout
                    .calculate(location, root, &self.layout_snapshots(location))
                    .ok()
                    .and_then(|plan| plan.target_for_window(window_id))
                    .map(|target| target.tile())
            })
            .or(Some(root));
        let pointer = self
            .last_pointer_x
            .is_finite()
            .then_some(self.last_pointer_x.round())
            .zip(
                self.last_pointer_y
                    .is_finite()
                    .then_some(self.last_pointer_y.round()),
            )
            .and_then(|(x, y)| {
                (x >= f64::from(i32::MIN)
                    && x <= f64::from(i32::MAX)
                    && y >= f64::from(i32::MIN)
                    && y <= f64::from(i32::MAX))
                .then_some(LayoutPoint::new(x as i32, y as i32))
            });
        InsertHint {
            focused: self.focused_window_id,
            fallback,
            pointer,
            anchor_rect,
        }
    }

    pub(in crate::compositor) fn location_is_visible_for_layout(
        &self,
        location: WorkspaceLocation,
    ) -> bool {
        match location {
            WorkspaceLocation::Regular(workspace) => workspace == self.active_workspace(),
            WorkspaceLocation::Special(special) => {
                self.workspace_manager.visible_special_workspace() == Some(special)
            }
        }
    }

    pub(in crate::compositor) fn apply_tiled_layout_plan(
        &mut self,
        plan: TiledLayoutSolution,
        floating_restore: Option<(WindowId, WindowGeometry)>,
    ) -> bool {
        self.begin_layout_reflow_batch();
        let changed = self.apply_tiled_layout_plan_inner(plan, floating_restore, true);
        let scene_effect = self.finish_layout_reflow_batch();
        changed || scene_effect
    }

    fn apply_tiled_layout_plan_inner(
        &mut self,
        plan: TiledLayoutSolution,
        floating_restore: Option<(WindowId, WindowGeometry)>,
        refresh_scene: bool,
    ) -> bool {
        let location = plan.location();
        if !self.location_is_visible_for_layout(location) {
            self.tiled_layout_dirty.insert(location);
            if let Some((window_id, geometry)) = floating_restore
                && self.window(window_id).is_some_and(|window| {
                    window.management.is_some_and(|management| {
                        management.location() == location
                            && management.layout() == LayoutMembership::Floating
                    })
                })
            {
                self.tiled_floating_restores.insert(window_id, geometry);
            }
            return false;
        }
        for target in plan.updates() {
            let Some(window) = self.window(target.window()).cloned() else {
                continue;
            };
            if window.state.mode() != ToplevelMode::Normal || window.state.is_minimized() {
                continue;
            }
            let geometry = WindowGeometry::new(
                SurfacePlacement::absolute_root_at(target.client().x(), target.client().y()),
                target.client().width(),
                target.client().height(),
            );
            self.apply_layout_geometry(window.id, window.backend, geometry);
        }
        if let Some((window_id, geometry)) = floating_restore
            && let Some(window) = self.window(window_id).cloned()
        {
            self.apply_layout_geometry(window_id, window.backend, geometry);
        }
        if refresh_scene {
            self.refresh_active_scene_surface_order();
        }
        self.tiled_layout_dirty.remove(&location);
        !plan.is_empty()
    }

    pub(in crate::compositor) fn location_has_deferred_floating_restore(
        &self,
        location: WorkspaceLocation,
    ) -> bool {
        self.tiled_floating_restores.keys().any(|window_id| {
            self.window(*window_id).is_some_and(|window| {
                window.management.is_some_and(|management| {
                    management.location() == location
                        && management.layout() == LayoutMembership::Floating
                })
            })
        })
    }

    pub(in crate::compositor) fn apply_deferred_floating_restores(
        &mut self,
        location: WorkspaceLocation,
    ) -> bool {
        let window_ids = self
            .tiled_floating_restores
            .keys()
            .copied()
            .filter(|window_id| {
                self.window(*window_id).is_some_and(|window| {
                    window.management.is_some_and(|management| {
                        management.location() == location
                            && management.layout() == LayoutMembership::Floating
                    })
                })
            })
            .collect::<Vec<_>>();
        let mut changed = false;
        for window_id in window_ids {
            let Some(geometry) = self.tiled_floating_restores.remove(&window_id) else {
                continue;
            };
            let Some(window) = self.window(window_id).cloned() else {
                continue;
            };
            self.apply_layout_geometry(window_id, window.backend, geometry);
            changed = true;
        }
        if changed || self.tiled_layout.tree(location).is_none() {
            self.tiled_layout_dirty.remove(&location);
        }
        changed
    }

    fn apply_layout_geometry(
        &mut self,
        window_id: WindowId,
        backend: WindowBackend,
        geometry: WindowGeometry,
    ) {
        let root_surface_id = match self.window(window_id) {
            Some(window) => window.root_surface_id,
            None => return,
        };
        let current = self
            .current_visual_root_window_geometry(root_surface_id)
            .or_else(|| self.current_root_window_geometry(root_surface_id));
        if current == Some(geometry) {
            return;
        }
        if let Some((edges, interaction_id)) = self.tiled_resize_owner(window_id) {
            let _ = self.queue_resize_root_window_to(
                root_surface_id,
                geometry.width,
                geometry.height,
                geometry.placement,
                edges,
                interaction_id,
            );
            self.mark_astrea_toplevel_dirty(window_id);
            return;
        }
        match backend {
            WindowBackend::Xdg(_) => {
                let _ = self.send_configure_root_window_to(
                    root_surface_id,
                    geometry.width,
                    geometry.height,
                    ToplevelMode::Normal.xdg_states(),
                );
                self.set_surface_placement_with_cause(
                    root_surface_id,
                    geometry.placement,
                    RenderGenerationCause::LayoutReflow,
                );
                self.install_toplevel_visual_geometry(root_surface_id, geometry);
            }
            WindowBackend::X11(_) => {
                let _ = self.set_x11_frame_geometry(window_id, geometry);
                self.set_surface_placement_with_cause(
                    root_surface_id,
                    geometry.placement,
                    RenderGenerationCause::LayoutReflow,
                );
                self.install_x11_visual_geometry(root_surface_id, geometry);
                self.queue_backend_configure(window_id, geometry, ToplevelMode::Normal, false);
                self.queue_backend_state(window_id);
            }
        }
        self.mark_astrea_toplevel_dirty(window_id);
    }
}

fn snapshot_for_window(window: &DesktopWindow) -> LayoutWindowSnapshot {
    LayoutWindowSnapshot::new(window.id)
        .with_minimized(window.state.is_minimized())
        .with_constraints(
            LayoutConstraints {
                min_width: window.constraints.min_width,
                min_height: window.constraints.min_height,
                max_width: window.constraints.max_width,
                max_height: window.constraints.max_height,
                base_width: window.constraints.base_width,
                base_height: window.constraints.base_height,
                width_increment: window.constraints.width_increment,
                height_increment: window.constraints.height_increment,
                min_aspect: window.constraints.min_aspect,
                max_aspect: window.constraints.max_aspect,
            }
            .normalized(),
        )
}
