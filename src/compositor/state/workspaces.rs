use crate::wm::{WorkspaceId, WorkspaceSwitchOutcome};

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WindowWorkspaceChange {
    window_id: WindowId,
    previous: WorkspaceId,
    new: WorkspaceId,
}

#[derive(Debug, Default)]
struct WorkspaceMembershipTransition {
    changes: Vec<WindowWorkspaceChange>,
    active_scene_changed: bool,
}

impl CompositorState {
    pub(in crate::compositor) fn active_workspace(&self) -> WorkspaceId {
        self.workspace_manager.active_workspace()
    }

    pub(in crate::compositor) fn window_is_visible_in_active_workspace(
        &self,
        window_id: WindowId,
    ) -> bool {
        let Some(owner_id) = self.workspace_owner_window_id(window_id) else {
            // Legacy/test-only auxiliary windows can be renderable before a
            // managed owner is known.  They remain globally visible until
            // transient metadata gives them an inherited workspace.
            return self
                .window(window_id)
                .is_some_and(|window| window.management.is_none());
        };
        let Some(window) = self.window(owner_id) else {
            return false;
        };
        window
            .management
            .is_some_and(|management| management.workspace() == self.active_workspace())
            && !window.state.is_minimized()
    }

    pub(in crate::compositor) fn surface_is_visible_in_active_workspace(
        &self,
        surface_id: u32,
    ) -> bool {
        let root_surface_id = self
            .popup_nodes
            .get(&surface_id)
            .map(|node| node.owner_root_id)
            .unwrap_or_else(|| self.root_surface_id_for_surface(surface_id));
        self.window_id_for_surface(root_surface_id)
            .is_none_or(|window_id| self.window_is_visible_in_active_workspace(window_id))
    }

    pub(in crate::compositor) fn workspace_owner_window_id(
        &self,
        window_id: WindowId,
    ) -> Option<WindowId> {
        let mut current = window_id;
        for _ in 0..=self.desktop_windows.len() {
            let window = self.window(current)?;
            if window.management.is_some() {
                return Some(current);
            }
            let parent = window
                .relationships
                .parent
                .or(window.relationships.transient_for)?;
            current = parent;
        }
        None
    }

    pub(in crate::compositor) fn activate_workspace(
        &mut self,
        workspace: WorkspaceId,
    ) -> WorkspaceSwitchOutcome {
        let affected_windows = self
            .desktop_windows
            .keys()
            .copied()
            .filter(|window_id| self.window_is_visible_in_active_workspace(*window_id))
            .collect::<Vec<_>>();
        let outcome = self.workspace_manager.activate(workspace);
        let WorkspaceSwitchOutcome::Changed { .. } = outcome else {
            return outcome;
        };

        let focused_root = self
            .focused_surface
            .as_ref()
            .map(|surface| self.root_surface_id_for_surface(compositor_surface_id(surface)));
        let focused_layer_surface =
            focused_root.is_some_and(|root| self.layer_surfaces.contains_key(&root));
        if !focused_layer_surface {
            self.focused_surface = None;
            self.focused_window_id = None;
            self.clear_keyboard_focus();
            self.clear_pointer_focus();
        } else {
            self.last_application_keyboard_focus = None;
        }
        self.cancel_workspace_transition_state_for_window_ids(&affected_windows);
        self.rebuild_active_scene_view();
        self.reconcile_idle_inhibition();
        self.mark_astrea_toplevel_structure_dirty();
        self.queue_workspace_publication_commands();
        self.advance_render_generation(RenderGenerationCause::WorkspaceSwitch);
        self.refresh_pointer_focus_at_last_position();
        self.recompute_layer_keyboard_focus();
        let layer_focus_after_switch = self.focused_surface.as_ref().is_some_and(|surface| {
            self.layer_surfaces
                .contains_key(&self.root_surface_id_for_surface(compositor_surface_id(surface)))
        });
        if !focused_layer_surface && !layer_focus_after_switch {
            let _ = self.focus_topmost_renderable_toplevel();
        }
        self.publish_workspace_state();
        outcome
    }

    pub(in crate::compositor) fn move_window_family_to_workspace(
        &mut self,
        window_id: WindowId,
        workspace: WorkspaceId,
    ) -> bool {
        if !self.workspace_manager.contains(workspace) {
            return false;
        }
        let Some(window) = self.window(window_id) else {
            return false;
        };
        if !window.is_workspace_managed() {
            return false;
        }
        let family_root = self.workspace_family_root(window_id);
        let family = self
            .desktop_windows
            .keys()
            .copied()
            .filter(|candidate| self.window_is_in_family(*candidate, family_root))
            .collect::<Vec<_>>();
        if family.is_empty() {
            return false;
        }
        let desired = family
            .into_iter()
            .filter_map(|family_id| {
                self.window(family_id)
                    .and_then(|window| window.management)
                    .map(|_| (family_id, workspace))
            })
            .collect::<HashMap<_, _>>();
        self.apply_workspace_membership_transition(
            self.plan_workspace_membership_transition(&desired),
        )
    }

    pub(in crate::compositor) fn move_focused_window_to_workspace(
        &mut self,
        workspace: WorkspaceId,
    ) -> bool {
        let Some(window_id) = self.focused_window_id else {
            return false;
        };
        self.move_window_family_to_workspace(window_id, workspace)
    }

    fn cancel_workspace_transition_state_for_window_ids(&mut self, window_ids: &[WindowId]) {
        let root_surface_ids = window_ids
            .iter()
            .filter_map(|window_id| self.window(*window_id).map(|window| window.root_surface_id))
            .collect::<Vec<_>>();
        if self
            .window_interaction
            .is_some_and(|interaction| window_ids.contains(&interaction.window_id))
            && let Some(interaction) = self.window_interaction
        {
            self.end_window_interaction_by_id_with_reason(
                interaction.id,
                WindowInteractionEndReason::WorkspaceSwitch,
            );
        }
        if self.pointer_surface.as_ref().is_some_and(|surface| {
            let surface_id = compositor_surface_id(surface);
            root_surface_ids.contains(&self.root_surface_id_for_surface(surface_id))
        }) {
            self.clear_pointer_constraint();
        }
        if self.implicit_pointer_grab.as_ref().is_some_and(|grab| {
            root_surface_ids.contains(&grab.root_surface_id)
                || root_surface_ids.contains(
                    &self.root_surface_id_for_surface(compositor_surface_id(&grab.surface)),
                )
        }) {
            self.end_implicit_pointer_grab("workspace-switch");
        }
        self.held_pointer_buttons.retain(|press| {
            !root_surface_ids.contains(&press.root_surface_id)
                && !window_ids
                    .iter()
                    .any(|window_id| press.window_id == Some(*window_id))
        });
        if self.last_pointer_press.as_ref().is_some_and(|press| {
            root_surface_ids.contains(&press.root_surface_id)
                || window_ids
                    .iter()
                    .any(|window_id| press.window_id == Some(*window_id))
        }) {
            self.last_pointer_press = None;
        }
        self.clear_popup_grab_for_surface_ids(&root_surface_ids);
    }

    fn plan_workspace_membership_transition(
        &self,
        desired: &HashMap<WindowId, WorkspaceId>,
    ) -> WorkspaceMembershipTransition {
        let active_workspace = self.active_workspace();
        let changes = desired
            .iter()
            .filter_map(|(window_id, new)| {
                let previous = self.window(*window_id)?.management?.workspace();
                (previous != *new).then_some(WindowWorkspaceChange {
                    window_id: *window_id,
                    previous,
                    new: *new,
                })
            })
            .collect::<Vec<_>>();
        let active_scene_changed = changes.iter().any(|change| {
            (change.previous == active_workspace) != (change.new == active_workspace)
        });
        WorkspaceMembershipTransition {
            changes,
            active_scene_changed,
        }
    }

    fn apply_workspace_membership_transition(
        &mut self,
        transition: WorkspaceMembershipTransition,
    ) -> bool {
        if transition.changes.is_empty() {
            return false;
        }
        let active_workspace = self.active_workspace();
        let departing_window_ids = transition
            .changes
            .iter()
            .filter_map(|change| (change.previous == active_workspace).then_some(change.window_id))
            .collect::<Vec<_>>();
        for change in &transition.changes {
            if let Some(window) = self.window_mut(change.window_id) {
                window.management = window
                    .management
                    .map(|management| management.with_workspace(change.new));
            }
            if self
                .window(change.window_id)
                .is_some_and(|window| matches!(window.backend, WindowBackend::X11(_)))
            {
                self.backend_commands.push(
                    crate::compositor::window_backend::WindowBackendCommand::SetWorkspace {
                        window: change.window_id,
                        workspace: change.new.to_ewmh(),
                    },
                );
            }
            self.mark_astrea_toplevel_dirty(change.window_id);
        }
        if transition.active_scene_changed {
            self.cancel_workspace_transition_state_for_window_ids(&departing_window_ids);
        }
        if !transition.active_scene_changed {
            return true;
        }
        self.rebuild_active_scene_view();
        self.reconcile_idle_inhibition();
        self.refresh_pointer_focus_at_last_position();
        self.mark_astrea_toplevel_structure_dirty();
        if self
            .focused_window_id
            .is_some_and(|window_id| !self.window_is_visible_in_active_workspace(window_id))
        {
            self.focused_surface = None;
            self.focused_window_id = None;
            self.clear_keyboard_focus();
        }
        self.advance_render_generation(RenderGenerationCause::WorkspaceMove);
        if self.active_exclusive_layer_surface_id().is_none() && self.focused_window_id.is_none() {
            let _ = self.focus_topmost_renderable_toplevel();
        } else {
            self.recompute_layer_keyboard_focus();
        }
        true
    }

    fn workspace_family_root(&self, window_id: WindowId) -> WindowId {
        let mut current = window_id;
        for _ in 0..=self.desktop_windows.len() {
            let Some(parent) = self.window(current).and_then(|window| {
                window
                    .relationships
                    .parent
                    .or(window.relationships.transient_for)
            }) else {
                return current;
            };
            current = parent;
        }
        current
    }

    fn window_is_in_family(&self, candidate: WindowId, root: WindowId) -> bool {
        let mut current = candidate;
        for _ in 0..=self.desktop_windows.len() {
            if current == root {
                return true;
            }
            let Some(window) = self.window(current) else {
                return false;
            };
            let Some(parent) = window
                .relationships
                .parent
                .or(window.relationships.transient_for)
            else {
                return false;
            };
            current = parent;
        }
        false
    }

    pub(in crate::compositor) fn reconcile_workspace_inheritance(&mut self) {
        let window_ids = self.desktop_windows.keys().copied().collect::<Vec<_>>();
        let mut desired = HashMap::with_capacity(window_ids.len());
        let mut resolving = HashSet::new();
        for window_id in window_ids {
            let _ = self.resolve_inherited_workspace(window_id, &mut desired, &mut resolving);
        }
        let transition = self.plan_workspace_membership_transition(&desired);
        self.apply_workspace_membership_transition(transition);
    }

    fn resolve_inherited_workspace(
        &self,
        window_id: WindowId,
        desired: &mut HashMap<WindowId, WorkspaceId>,
        resolving: &mut HashSet<WindowId>,
    ) -> Option<WorkspaceId> {
        if let Some(workspace) = desired.get(&window_id).copied() {
            return Some(workspace);
        }
        if !resolving.insert(window_id) {
            return self
                .window(window_id)
                .and_then(|window| window.management.map(|management| management.workspace()));
        }
        let result = self.window(window_id).and_then(|window| {
            let own_workspace = window.management.map(|management| management.workspace())?;
            if !window.is_workspace_managed() {
                return Some(own_workspace);
            }
            let parent_id = window
                .relationships
                .parent
                .or(window.relationships.transient_for);
            let inherited = parent_id
                .and_then(|parent| self.resolve_inherited_workspace(parent, desired, resolving));
            Some(inherited.unwrap_or(own_workspace))
        });
        resolving.remove(&window_id);
        if let Some(workspace) = result {
            desired.insert(window_id, workspace);
        }
        result
    }

    fn queue_workspace_publication_commands(&mut self) {
        let (output_width, output_height) = self.output_dimensions();
        self.backend_commands.push(
            crate::compositor::window_backend::WindowBackendCommand::PublishWorkspaceState {
                workspace_count: self.workspace_manager.workspace_count(),
                current_workspace: self.active_workspace().to_ewmh(),
                output_width,
                output_height,
            },
        );
    }
}
