use crate::wm::{
    LayoutMembership, SpecialWorkspaceId, SpecialWorkspaceToggleOutcome, WorkspaceId,
    WorkspaceLocation, WorkspaceSwitchOutcome,
};
use crate::{
    animation_control::{AnimationEffect, AnimationRuntimeCapabilities, AnimationSlot},
    presentation_animation::{AnimationCurve, AnimationTime, SpringSpec},
};

use super::pointer_constraints::PointerConstraintDeactivationReason;
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WindowWorkspaceChange {
    window_id: WindowId,
    previous: WorkspaceLocation,
    new: WorkspaceLocation,
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

    fn workspace_slide_admission_fallback(
        &self,
        previous: WorkspaceId,
        target: WorkspaceId,
        sampled_at: AnimationTime,
    ) -> Option<&'static str> {
        if !self.animation_control.enabled()
            || self.animation_control.effective_effect(
                AnimationSlot::WorkspaceSwitch,
                AnimationRuntimeCapabilities::default(),
            ) != AnimationEffect::WorkspaceSlide
        {
            return Some("effect_disabled");
        }

        let Some(previous_position) = self.workspace_manager.workspace_axis_position(previous)
        else {
            return Some("invalid_workspace_axis");
        };
        let Some(target_position) = self.workspace_manager.workspace_axis_position(target) else {
            return Some("invalid_workspace_axis");
        };
        let start_position = self
            .workspace_presentation
            .transition()
            .and_then(|_| {
                self.workspace_presentation
                    .sample_at(&self.workspace_manager, sampled_at, 1.0, 1.0)
            })
            .map_or(previous_position, |sample| sample.position);
        let path_start = start_position.min(target_position);
        let path_end = start_position.max(target_position);
        let traverses_fullscreen = self.desktop_windows.keys().copied().any(|window_id| {
            let Some(owner_id) = self.workspace_owner_window_id(window_id) else {
                return false;
            };
            let Some(window) = self.window(owner_id) else {
                return false;
            };
            if window.state.is_minimized() || window.state.mode() != ToplevelMode::Fullscreen {
                return false;
            }
            let Some(WorkspaceLocation::Regular(workspace)) =
                window.management.map(|management| management.location())
            else {
                return false;
            };
            self.workspace_manager
                .workspace_axis_position(workspace)
                .is_some_and(|position| position >= path_start && position <= path_end)
        });
        if traverses_fullscreen {
            return Some("fullscreen_path");
        }
        if self.lifecycle_animation_has_pending_visible() {
            return Some("retained_lifecycle_active");
        }
        if self.window_exit_payloads.active_payloads().next().is_some() {
            return Some("window_exit_active");
        }
        if self.active_drag.is_some()
            || self.implicit_pointer_grab.is_some()
            || !self.held_pointer_buttons.is_empty()
            || self.window_interaction.is_some()
        {
            return Some("dnd_or_interaction");
        }
        None
    }

    fn begin_workspace_slide(
        &mut self,
        outcome: WorkspaceSwitchOutcome,
        sampled_at: AnimationTime,
    ) {
        let WorkspaceSwitchOutcome::Changed { previous, current } = outcome else {
            return;
        };
        if let Some(reason) = self.workspace_slide_admission_fallback(previous, current, sampled_at)
        {
            let transition_id = self
                .workspace_presentation
                .transition()
                .map_or(0, |transition| transition.id.get());
            self.workspace_presentation.clear();
            if crate::pointer_debug::cursor_responsiveness_trace_enabled() {
                eprintln!(
                    "workspace_slide_fallback transition_id={transition_id} reason={reason} previous_workspace={} target_workspace={}",
                    previous.get(),
                    current.get(),
                );
            }
            return;
        }

        let curve = crate::animation_control::scale_curve(
            AnimationCurve::spring(SpringSpec::new(300.0, 38.1051).with_settlement(0.0001, 0.002)),
            self.animation_control.configuration().speed,
        );
        let retarget = self.workspace_presentation.transition().is_some();
        if let Some(transition_id) = self.workspace_presentation.activate_for_switch(
            outcome,
            &self.workspace_manager,
            sampled_at,
            curve,
        ) && crate::pointer_debug::cursor_responsiveness_trace_enabled()
            && let Some(transition) = self.workspace_presentation.transition()
        {
            let event = if retarget {
                "workspace_slide_retarget"
            } else {
                "workspace_slide_start"
            };
            eprintln!(
                "{event} transition_id={} previous_workspace={} target_workspace={} start_position={:.6} start_velocity={:.6} target_position={:.6}",
                transition_id.get(),
                previous.get(),
                current.get(),
                transition.start_position,
                transition.start_velocity,
                transition.target_position,
            );
        }
    }

    pub(in crate::compositor) fn toggle_default_special_workspace(
        &mut self,
    ) -> SpecialWorkspaceToggleOutcome {
        let previous_selection = self.active_scene_selection();
        self.workspace_scene_transition_active = true;
        let outcome = self
            .workspace_manager
            .toggle_special_workspace(SpecialWorkspaceId::DEFAULT);
        let new_selection = self.active_scene_selection();
        let special_location = WorkspaceLocation::Special(SpecialWorkspaceId::DEFAULT);
        let layout_batch = matches!(outcome, SpecialWorkspaceToggleOutcome::Opened { .. })
            && ((self.tiled_layout.tree(special_location).is_some()
                && self.tiled_layout_dirty.contains(&special_location))
                || self.location_has_deferred_floating_restore(special_location));
        if layout_batch {
            self.begin_layout_reflow_batch();
            if self.tiled_layout_dirty.contains(&special_location) {
                let _ = self.reflow_tiled_location(special_location);
            }
            let _ = self.apply_deferred_floating_restores(special_location);
        }
        let departing_window_ids =
            self.departing_window_ids_for_scene_selection(previous_selection, new_selection);
        self.cancel_workspace_transition_state_for_window_ids(&departing_window_ids);
        let scene_update = self.rebuild_active_scene_view();
        self.deactivate_pointer_constraints_for_departing_window_ids(
            &departing_window_ids,
            PointerConstraintDeactivationReason::WorkspaceDeparture,
        );
        self.reconcile_idle_inhibition();
        self.mark_astrea_toplevel_structure_dirty();
        self.recompute_layer_keyboard_focus();
        if matches!(outcome, SpecialWorkspaceToggleOutcome::Opened { .. })
            && self.active_exclusive_layer_surface_id().is_none()
            && self
                .topmost_renderable_toplevel_window_id()
                .and_then(|window_id| self.window(window_id))
                .is_some_and(|window| {
                    window
                        .management
                        .is_some_and(|management| management.special_workspace().is_some())
                })
        {
            let _ = self.focus_topmost_renderable_toplevel();
        }
        if matches!(outcome, SpecialWorkspaceToggleOutcome::Closed { .. })
            && self
                .focused_window_id
                .is_some_and(|window_id| !self.window_is_visible_in_active_scene(window_id))
        {
            self.focused_window_id = None;
            if self.active_exclusive_layer_surface_id().is_none() {
                self.focused_surface = None;
                self.clear_keyboard_focus();
                let _ = self.focus_topmost_renderable_toplevel();
            } else {
                self.recompute_layer_keyboard_focus();
            }
        }
        self.workspace_scene_transition_active = false;
        self.reconcile_wayland_dnd_target_after_scene_transition();
        self.refresh_pointer_focus_at_last_position();
        if scene_update.visual_scene_changed {
            self.advance_render_generation(RenderGenerationCause::WorkspaceSwitch);
        }
        if layout_batch {
            let _ = self.finish_layout_reflow_batch();
        }
        outcome
    }

    fn location_is_visible_in_selection(
        location: WorkspaceLocation,
        selection: ActiveSceneSelection,
    ) -> bool {
        match location {
            WorkspaceLocation::Regular(workspace) => workspace == selection.regular,
            WorkspaceLocation::Special(special) => selection.special == Some(special),
        }
    }

    fn departing_window_ids_for_scene_selection(
        &self,
        previous: ActiveSceneSelection,
        new: ActiveSceneSelection,
    ) -> Vec<WindowId> {
        self.desktop_windows
            .keys()
            .copied()
            .filter(|window_id| {
                let Some(owner_id) = self.workspace_owner_window_id(*window_id) else {
                    return false;
                };
                let Some(location) = self
                    .window(owner_id)
                    .and_then(|window| window.management)
                    .map(|management| management.location())
                else {
                    return false;
                };
                Self::location_is_visible_in_selection(location, previous)
                    && !Self::location_is_visible_in_selection(location, new)
            })
            .collect()
    }

    pub(in crate::compositor) fn active_scene_selection(&self) -> ActiveSceneSelection {
        ActiveSceneSelection {
            regular: self.active_workspace(),
            special: self.workspace_manager.visible_special_workspace(),
        }
    }

    pub(in crate::compositor) fn scene_work_owner_for_window(
        &self,
        window_id: WindowId,
    ) -> SceneWorkOwner {
        let Some(owner_id) = self.workspace_owner_window_id(window_id) else {
            return SceneWorkOwner::Global;
        };
        self.window(owner_id)
            .and_then(|window| window.management)
            .map_or(SceneWorkOwner::Global, |management| {
                SceneWorkOwner::Location(management.location())
            })
    }

    pub(in crate::compositor) fn scene_work_owner_for_surface(
        &self,
        surface_id: u32,
    ) -> SceneWorkOwner {
        let root_surface_id = self
            .popup_nodes
            .get(&surface_id)
            .map(|node| node.owner_root_id)
            .unwrap_or_else(|| self.root_surface_id_for_surface(surface_id));
        self.window_id_for_surface(root_surface_id)
            .map_or(SceneWorkOwner::Global, |window_id| {
                self.scene_work_owner_for_window(window_id)
            })
    }

    pub(in crate::compositor) fn window_is_visible_in_active_scene(
        &self,
        window_id: WindowId,
    ) -> bool {
        let Some(owner_id) = self.workspace_owner_window_id(window_id) else {
            // A surface with no managed application owner is output/global
            // work. Managed auxiliary surfaces resolve to their canonical
            // root before reaching this branch.
            return self
                .window(window_id)
                .is_some_and(|window| window.management.is_none());
        };
        let Some(window) = self.window(owner_id) else {
            return false;
        };
        let visible = window
            .management
            .is_some_and(|management| match management.location() {
                WorkspaceLocation::Regular(workspace) => {
                    workspace == self.active_scene_selection().regular
                }
                WorkspaceLocation::Special(special) => {
                    self.active_scene_selection().special == Some(special)
                }
            });
        visible && !window.state.is_minimized()
    }

    pub(in crate::compositor) fn surface_is_visible_in_active_scene(
        &self,
        surface_id: u32,
    ) -> bool {
        let root_surface_id = self
            .popup_nodes
            .get(&surface_id)
            .map(|node| node.owner_root_id)
            .unwrap_or_else(|| self.root_surface_id_for_surface(surface_id));
        self.window_id_for_surface(root_surface_id)
            .is_none_or(|window_id| self.window_is_visible_in_active_scene(window_id))
    }

    pub(in crate::compositor) fn regular_workspace_is_visible_for_frame_work(
        &self,
        workspace: WorkspaceId,
    ) -> bool {
        workspace == self.active_workspace()
            || self
                .workspace_presentation
                .path_contains_workspace(&self.workspace_manager, workspace)
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
        let previous_workspace = self.active_workspace();
        let affected_windows = self
            .desktop_windows
            .keys()
            .copied()
            .filter(|window_id| {
                self.workspace_owner_window_id(*window_id)
                    .and_then(|owner_id| self.window(owner_id))
                    .and_then(|window| window.management)
                    .is_some_and(|management| {
                        management.regular_workspace() == Some(previous_workspace)
                    })
            })
            .collect::<Vec<_>>();
        let outcome = self.workspace_manager.activate(workspace);
        let WorkspaceSwitchOutcome::Changed { .. } = outcome else {
            return outcome;
        };
        let sampled_at = AnimationTime::monotonic_now().unwrap_or(AnimationTime::from_nanos(0));
        self.begin_workspace_slide(outcome, sampled_at);
        self.refresh_frame_work_visibility();
        self.workspace_scene_transition_active = true;
        let location = WorkspaceLocation::Regular(workspace);
        let layout_batch = (self.tiled_layout.tree(location).is_some()
            && self.tiled_layout_dirty.contains(&location))
            || self.location_has_deferred_floating_restore(location);
        if layout_batch {
            self.begin_layout_reflow_batch();
            if self.tiled_layout_dirty.contains(&location) {
                let _ = self.reflow_tiled_location(location);
            }
            let _ = self.apply_deferred_floating_restores(location);
        }

        let focused_root = self
            .focused_surface
            .as_ref()
            .map(|surface| self.root_surface_id_for_surface(compositor_surface_id(surface)));
        let focused_layer_surface =
            focused_root.is_some_and(|root| self.layer_surfaces.contains_key(&root));
        let focused_app_leaves = self
            .focused_window_id
            .is_some_and(|window_id| affected_windows.contains(&window_id));
        if !focused_layer_surface && focused_app_leaves {
            self.focused_surface = None;
            self.focused_window_id = None;
            self.clear_keyboard_focus();
        } else if focused_layer_surface {
            self.last_application_keyboard_focus = None;
        }
        self.cancel_workspace_transition_state_for_window_ids(&affected_windows);
        self.rebuild_active_scene_view();
        self.deactivate_pointer_constraints_for_departing_window_ids(
            &affected_windows,
            PointerConstraintDeactivationReason::WorkspaceDeparture,
        );
        self.reconcile_idle_inhibition();
        self.mark_astrea_toplevel_structure_dirty();
        self.queue_workspace_publication_commands();
        self.recompute_layer_keyboard_focus();
        let layer_focus_after_switch = self.focused_surface.as_ref().is_some_and(|surface| {
            self.layer_surfaces
                .contains_key(&self.root_surface_id_for_surface(compositor_surface_id(surface)))
        });
        if !focused_layer_surface && !layer_focus_after_switch {
            let _ = self.focus_topmost_renderable_toplevel();
        }
        self.workspace_scene_transition_active = false;
        self.reconcile_wayland_dnd_target_after_scene_transition();
        self.refresh_pointer_focus_at_last_position();
        self.advance_render_generation(RenderGenerationCause::WorkspaceSwitch);
        if layout_batch {
            let _ = self.finish_layout_reflow_batch();
        }
        self.publish_workspace_state();
        outcome
    }

    pub(in crate::compositor) fn move_window_family_to_workspace(
        &mut self,
        window_id: WindowId,
        workspace: WorkspaceId,
    ) -> bool {
        self.move_window_family_to_location(window_id, WorkspaceLocation::Regular(workspace))
    }

    fn move_window_family_to_location(
        &mut self,
        window_id: WindowId,
        destination: WorkspaceLocation,
    ) -> bool {
        if let WorkspaceLocation::Regular(workspace) = destination
            && !self.workspace_manager.contains(workspace)
        {
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
                    .map(|_| (family_id, destination))
            })
            .collect::<HashMap<_, _>>();
        self.apply_workspace_membership_transition(
            self.plan_workspace_membership_transition(&desired),
        )
    }

    pub(in crate::compositor) fn move_focused_window_to_or_from_special_workspace(
        &mut self,
    ) -> bool {
        let Some(window_id) = self.focused_window_id else {
            return false;
        };
        let family_root = self.workspace_family_root(window_id);
        let Some(current) = self
            .window(family_root)
            .and_then(|window| window.management)
            .map(|management| management.location())
        else {
            return false;
        };
        let destination = match current {
            WorkspaceLocation::Regular(_) => {
                WorkspaceLocation::Special(SpecialWorkspaceId::DEFAULT)
            }
            WorkspaceLocation::Special(_) => WorkspaceLocation::Regular(self.active_workspace()),
        };
        self.move_window_family_to_location(window_id, destination)
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
            self.invalidate_locked_relative_recipient_cache();
        }
        let preserved_dnd_grab_surface = self
            .implicit_pointer_grab
            .as_ref()
            .filter(|grab| {
                root_surface_ids.contains(&grab.root_surface_id)
                    && self.wayland_dnd_grab_owns_held_gesture(grab)
            })
            .map(|grab| (grab.root_surface_id, compositor_surface_id(&grab.surface)));
        if self.implicit_pointer_grab.as_ref().is_some_and(|grab| {
            root_surface_ids.contains(&grab.root_surface_id)
                || root_surface_ids.contains(
                    &self.root_surface_id_for_surface(compositor_surface_id(&grab.surface)),
                )
        }) && preserved_dnd_grab_surface.is_none()
        {
            self.end_implicit_pointer_grab("workspace-switch");
        }
        self.held_pointer_buttons.retain(|press| {
            preserved_dnd_grab_surface.is_some_and(|(root_surface_id, surface_id)| {
                press.root_surface_id == root_surface_id
                    && compositor_surface_id(&press.surface) == surface_id
            }) || (!root_surface_ids.contains(&press.root_surface_id)
                && !window_ids
                    .iter()
                    .any(|window_id| press.window_id == Some(*window_id)))
        });
        if self.last_pointer_press.as_ref().is_some_and(|press| {
            let departing = root_surface_ids.contains(&press.root_surface_id)
                || window_ids
                    .iter()
                    .any(|window_id| press.window_id == Some(*window_id));
            let still_owned =
                preserved_dnd_grab_surface.is_some_and(|(root_surface_id, surface_id)| {
                    press.root_surface_id == root_surface_id
                        && compositor_surface_id(&press.surface) == surface_id
                        && self.held_pointer_buttons.iter().any(|held| {
                            held.button == press.button
                                && held.root_surface_id == root_surface_id
                                && compositor_surface_id(&held.surface) == surface_id
                        })
                });
            departing && !still_owned
        }) {
            self.last_pointer_press = None;
        }
        self.clear_popup_grab_for_surface_ids(&root_surface_ids);
    }

    fn wayland_dnd_grab_owns_held_gesture(&self, grab: &ImplicitPointerGrab) -> bool {
        if grab.routing != ImplicitPointerRouting::WaylandDnd {
            return false;
        }

        let grab_surface_id = compositor_surface_id(&grab.surface);
        if !grab.surface.is_alive()
            || self.root_surface_id_for_surface(grab_surface_id) != grab.root_surface_id
            || !self
                .renderable_surfaces
                .iter()
                .any(|surface| surface.surface_id == grab_surface_id)
            || !self.held_pointer_buttons.iter().any(|press| {
                press.root_surface_id == grab.root_surface_id
                    && compositor_surface_id(&press.surface) == grab_surface_id
                    && press.surface.is_alive()
                    && press
                        .surface
                        .client()
                        .zip(grab.surface.client())
                        .is_some_and(|(press_client, grab_client)| {
                            press_client.id() == grab_client.id()
                        })
            })
        {
            return false;
        }

        let Some(active) = self.active_drag.as_ref() else {
            // Cancellation ends the transfer, not the still-held physical
            // gesture. The verified Wayland DnD grab remains its owner until
            // the final button release removes this routing state.
            return true;
        };
        let Some(origin_surface) = active.origin.wayland_origin_surface() else {
            return false;
        };
        let origin_surface_id = compositor_surface_id(origin_surface);
        let same_client = origin_surface
            .client()
            .zip(grab.surface.client())
            .is_some_and(|(origin, grab)| origin.id() == grab.id());

        matches!(
            &active.id,
            crate::xwayland::CanonicalDndSessionId::Wayland(_)
        ) && active.lifecycle_driver == DragLifecycleDriver::WaylandImplicitPointerGrab
            && active.phase == DragSessionPhase::Dragging
            && origin_surface.is_alive()
            && self.root_surface_id_for_surface(origin_surface_id) == grab.root_surface_id
            && same_client
    }

    fn plan_workspace_membership_transition(
        &self,
        desired: &HashMap<WindowId, WorkspaceLocation>,
    ) -> WorkspaceMembershipTransition {
        let changes = desired
            .iter()
            .filter_map(|(window_id, new)| {
                let previous = self.window(*window_id)?.management?.location();
                (previous != *new).then_some(WindowWorkspaceChange {
                    window_id: *window_id,
                    previous,
                    new: *new,
                })
            })
            .collect::<Vec<_>>();
        let active_scene_changed = changes.iter().any(|change| {
            let previous_visible = self.location_is_visible_in_active_scene(change.previous);
            let new_visible = self.location_is_visible_in_active_scene(change.new);
            (previous_visible != new_visible) || (previous_visible && new_visible)
        });
        WorkspaceMembershipTransition {
            changes,
            active_scene_changed,
        }
    }

    fn location_is_visible_in_active_scene(&self, location: WorkspaceLocation) -> bool {
        match location {
            WorkspaceLocation::Regular(workspace) => workspace == self.active_workspace(),
            WorkspaceLocation::Special(special) => {
                self.workspace_manager.visible_special_workspace() == Some(special)
            }
        }
    }

    fn apply_workspace_membership_transition(
        &mut self,
        transition: WorkspaceMembershipTransition,
    ) -> bool {
        if transition.changes.is_empty() {
            return false;
        }
        let tiled_changes = transition
            .changes
            .iter()
            .filter_map(|change| {
                self.window(change.window_id)
                    .and_then(|window| window.management)
                    .filter(|management| management.layout() == LayoutMembership::Tiled)
                    .map(|_| (change.window_id, change.previous, change.new))
            })
            .collect::<Vec<_>>();
        let layout_batch = tiled_changes.iter().any(|(_, previous, new)| {
            self.location_is_visible_in_active_scene(*previous)
                || self.location_is_visible_in_active_scene(*new)
        });
        let prepared_tiled_migration = if tiled_changes.is_empty() {
            None
        } else {
            let Some(prepared) = self.migrate_tiled_layouts(&tiled_changes) else {
                return false;
            };
            Some(prepared)
        };
        if transition.active_scene_changed {
            self.workspace_scene_transition_active = true;
        }
        if layout_batch {
            self.begin_layout_reflow_batch();
        }
        if let Some(prepared) = prepared_tiled_migration.as_ref() {
            self.commit_prepared_tiled_migration(prepared);
        }
        let departing_window_ids = transition
            .changes
            .iter()
            .filter_map(|change| {
                (self.location_is_visible_in_active_scene(change.previous)
                    && !self.location_is_visible_in_active_scene(change.new))
                .then_some(change.window_id)
            })
            .collect::<Vec<_>>();
        let workspace_presence_changed = transition.changes.iter().any(|change| {
            matches!(change.previous, WorkspaceLocation::Regular(_))
                || matches!(change.new, WorkspaceLocation::Regular(_))
        });
        for change in &transition.changes {
            if let Some(window) = self.window_mut(change.window_id) {
                window.management = window
                    .management
                    .map(|management| management.with_location(change.new));
            }
            if self
                .window(change.window_id)
                .is_some_and(|window| matches!(window.backend, WindowBackend::X11(_)))
            {
                let command = match change.new {
                    WorkspaceLocation::Regular(workspace) => {
                        crate::compositor::window_backend::WindowBackendCommand::SetWorkspace {
                            window: change.window_id,
                            workspace: workspace.to_ewmh(),
                        }
                    }
                    WorkspaceLocation::Special(_) => {
                        crate::compositor::window_backend::WindowBackendCommand::ClearWorkspace {
                            window: change.window_id,
                        }
                    }
                };
                self.backend_commands.push(command);
            }
            self.mark_astrea_toplevel_dirty(change.window_id);
        }
        if workspace_presence_changed {
            self.mark_astrea_toplevel_structure_dirty();
        }
        if let Some(prepared) = prepared_tiled_migration {
            let _ = self.apply_prepared_tiled_migration(prepared);
        }
        if transition.active_scene_changed {
            self.cancel_workspace_transition_state_for_window_ids(&departing_window_ids);
        }
        if !transition.active_scene_changed {
            self.rebuild_scene_work_index();
            if layout_batch {
                let _ = self.finish_layout_reflow_batch();
            }
            return true;
        }
        self.rebuild_active_scene_view();
        self.deactivate_pointer_constraints_for_departing_window_ids(
            &departing_window_ids,
            PointerConstraintDeactivationReason::WindowMovedOffScene,
        );
        self.reconcile_idle_inhibition();
        self.mark_astrea_toplevel_structure_dirty();
        if self
            .focused_window_id
            .is_some_and(|window_id| !self.window_is_visible_in_active_scene(window_id))
        {
            self.focused_surface = None;
            self.focused_window_id = None;
            self.clear_keyboard_focus();
        }
        if self.active_exclusive_layer_surface_id().is_none() && self.focused_window_id.is_none() {
            let _ = self.focus_topmost_renderable_toplevel();
        } else {
            self.recompute_layer_keyboard_focus();
        }
        self.workspace_scene_transition_active = false;
        self.reconcile_wayland_dnd_target_after_scene_transition();
        self.refresh_pointer_focus_at_last_position();
        self.advance_render_generation(RenderGenerationCause::WorkspaceMove);
        if layout_batch {
            let _ = self.finish_layout_reflow_batch();
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
        // Auxiliary surfaces have no independent WorkspaceLocation. Rebuild
        // their derived scene owner whenever canonical parentage/transient
        // links change, even when no managed-root membership field changed.
        self.refresh_active_scene_surface_order();
        self.rebuild_scene_work_index();
    }

    fn resolve_inherited_workspace(
        &self,
        window_id: WindowId,
        desired: &mut HashMap<WindowId, WorkspaceLocation>,
        resolving: &mut HashSet<WindowId>,
    ) -> Option<WorkspaceLocation> {
        if let Some(workspace) = desired.get(&window_id).copied() {
            return Some(workspace);
        }
        if !resolving.insert(window_id) {
            return self
                .window(window_id)
                .and_then(|window| window.management.map(|management| management.location()));
        }
        let result = self.window(window_id).and_then(|window| {
            let parent_id = window
                .relationships
                .parent
                .or(window.relationships.transient_for);
            let inherited = parent_id
                .and_then(|parent| self.resolve_inherited_workspace(parent, desired, resolving));
            let own_location = window.management.map(|management| management.location());
            inherited.or(own_location)
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
