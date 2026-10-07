use super::*;

impl CompositorState {
    pub(in crate::compositor) fn pointer_constraint_ids_for_surface(
        &self,
        surface_id: u32,
    ) -> Vec<u64> {
        self.pointer_constraint_runtime
            .constraint_ids_for_surface(surface_id)
    }

    #[cfg(test)]
    pub(in crate::compositor) fn pointer_constraint_ids_for_test(&self) -> Vec<u64> {
        self.pointer_constraint_runtime.constraint_ids()
    }

    #[cfg(test)]
    pub(in crate::compositor) fn pointer_constraint_surface_snapshot_for_test(
        &self,
        constraint_id: u64,
    ) -> Option<PointerConstraintRuntimeSnapshot> {
        self.pointer_constraint_runtime
            .surface_snapshot(constraint_id)
    }

    pub(in crate::compositor) fn sync_cursor_visibility_request(&mut self) {
        self.refresh_presentation_feedback_eligibility();
        let desired_visible = self.interaction_cursor_override.is_some()
            || self.cursor_visibility.theme_fallback_visible();
        if self.cursor_visibility.visible == desired_visible {
            return;
        }
        self.cursor_visibility.visible = desired_visible;
        pointer_debug_log(format!(
            "cursor visibility backend_request visible={} client_hidden={} lock_hidden={:?}",
            desired_visible,
            self.cursor_visibility
                .client_hidden_pointer
                .as_ref()
                .map(|pointer| pointer.id().protocol_id())
                .map_or_else(|| "none".to_string(), |id| id.to_string()),
            self.cursor_visibility.lock_hidden_constraint_id
        ));
        self.pending_pointer_constraint_backend_requests.push(
            PointerConstraintBackendRequest::ApplyCursorVisibility {
                visible: desired_visible,
            },
        );
    }

    pub(in crate::compositor) fn resume_pending_pointer_constraint_activation(&mut self) {
        if self.window_interaction.is_some() {
            return;
        }
        let ids = self
            .pointer_constraint_runtime
            .constraints
            .values()
            .filter(|constraint| {
                constraint.committed
                    && !constraint.active
                    && !constraint.backend_pending
                    && constraint.protocol_resource_alive
                    && !constraint.defunct
            })
            .map(|constraint| constraint.id)
            .collect::<Vec<_>>();
        for id in ids {
            self.maybe_request_pointer_constraint_activation(id);
        }
    }

    pub(in crate::compositor) fn suspend_pointer_constraints_for_window_interaction(
        &mut self,
        root_surface_id: u32,
    ) {
        let constraint_ids = self
            .pointer_constraint_runtime
            .constraints
            .values()
            .filter(|constraint| {
                (constraint.active || constraint.backend_pending)
                    && self.presentation_owner_root_for_surface(compositor_surface_id(
                        &constraint.surface,
                    )) == root_surface_id
            })
            .map(|constraint| (constraint.id, constraint.active))
            .collect::<Vec<_>>();

        for (constraint_id, was_active) in constraint_ids {
            self.deactivate_pointer_constraint_by_id_with_restore_policy(
                constraint_id,
                was_active,
                true,
                true,
                PointerConstraintRestorePolicy::PreserveCurrentPosition,
            );
        }
    }

    pub(in crate::compositor) fn allocate_internal_pointer_constraint_id(&mut self) -> u64 {
        self.pointer_constraint_runtime
            .allocate_internal_constraint_id()
    }

    fn merge_pending_pointer_constraint_surface_state(
        &mut self,
        surface_id: u32,
        newer: CapturedPointerConstraintSurfaceState,
    ) {
        let older = self
            .pointer_constraint_runtime
            .pending_pointer_constraint_surface_states
            .remove(&surface_id)
            .unwrap_or_default();
        self.pointer_constraint_runtime
            .pending_pointer_constraint_surface_states
            .insert(surface_id, older.merge(newer));
    }

    pub(in crate::compositor) fn stage_pointer_constraint_install(
        &mut self,
        surface_id: u32,
        constraint_id: u64,
        region: SurfaceInputRegion,
    ) {
        self.merge_pending_pointer_constraint_surface_state(
            surface_id,
            CapturedPointerConstraintSurfaceState::Mutation(CapturedPointerConstraintCommit {
                constraint_id,
                lifecycle: PointerConstraintLifecycleCommit::Install,
                region: PointerConstraintRegionCommit::Set(region),
                cursor_position_hint: PointerConstraintHintCommit::NoChange,
            }),
        );
    }

    pub(in crate::compositor) fn take_pending_pointer_constraint_surface_state(
        &mut self,
        surface_id: u32,
    ) -> CapturedPointerConstraintSurfaceState {
        self.pointer_constraint_runtime
            .pending_pointer_constraint_surface_states
            .remove(&surface_id)
            .unwrap_or_default()
    }

    pub(in crate::compositor) fn stage_pointer_constraint_removal(
        &mut self,
        surface_id: u32,
        constraint_id: u64,
    ) {
        self.merge_pending_pointer_constraint_surface_state(
            surface_id,
            CapturedPointerConstraintSurfaceState::Mutation(CapturedPointerConstraintCommit {
                constraint_id,
                lifecycle: PointerConstraintLifecycleCommit::Remove,
                region: PointerConstraintRegionCommit::NoChange,
                cursor_position_hint: PointerConstraintHintCommit::NoChange,
            }),
        );
    }

    pub(in crate::compositor) fn stage_pointer_constraint_cancellation(
        &mut self,
        surface_id: u32,
        constraint_id: u64,
    ) {
        self.merge_pending_pointer_constraint_surface_state(
            surface_id,
            CapturedPointerConstraintSurfaceState::Mutation(CapturedPointerConstraintCommit {
                constraint_id,
                lifecycle: PointerConstraintLifecycleCommit::Cancel,
                region: PointerConstraintRegionCommit::NoChange,
                cursor_position_hint: PointerConstraintHintCommit::NoChange,
            }),
        );
    }

    pub(in crate::compositor) fn register_pointer_constraint(
        &mut self,
        registration: PointerConstraintRegistration,
    ) -> bool {
        let surface_id = compositor_surface_id(&registration.surface);
        let replacing_constraint_id = self
            .pointer_constraint_runtime
            .constraints
            .values()
            .find(|constraint| {
                !constraint.protocol_resource_alive
                    && constraint.committed
                    && same_surface_resource(&constraint.surface, &registration.surface)
            })
            .map(|constraint| constraint.id);
        let existing = self
            .pointer_constraint_runtime
            .constraints
            .values()
            .find(|constraint| {
                constraint.protocol_resource_alive
                    && (constraint.committed || constraint.surface_constraint_pending)
                    && same_surface_resource(&constraint.surface, &registration.surface)
            });
        if let Some(existing) = existing {
            pointer_debug_log(format!(
                "constraint reject already_constrained existing={} requested={} surface={} pointer={}",
                existing.id,
                registration.id,
                compositor_surface_id(&registration.surface),
                registration.pointer.id().protocol_id()
            ));
            return false;
        }
        if let Some(replacing_constraint_id) = replacing_constraint_id {
            pointer_debug_log(format!(
                "constraint replacement accepted old={} new={} surface={} old_effective_retirement_pending=true",
                replacing_constraint_id, registration.id, surface_id
            ));
        }

        let generation = self.pointer_constraint_runtime.allocate_generation();
        pointer_debug_log(format!(
            "constraint create id={} generation={} mode={:?} surface={} pointer={} client={}",
            registration.id,
            generation,
            registration.mode,
            compositor_surface_id(&registration.surface),
            registration.pointer.id().protocol_id(),
            wayland_resource_client_label(&registration.pointer)
        ));
        self.pointer_constraint_runtime.constraints.insert(
            registration.id,
            PointerConstraint {
                id: registration.id,
                generation,
                mode: registration.mode,
                lifetime: registration.lifetime,
                surface: registration.surface,
                pointer: registration.pointer,
                locked_resource: registration.locked_resource,
                confined_resource: registration.confined_resource,
                active: false,
                backend_pending: false,
                canceled_backend_activation: false,
                protocol_resource_alive: true,
                surface_constraint_pending: true,
                lifecycle_removal_pending: false,
                defunct: false,
                committed: false,
                committed_region: SurfaceInputRegion::Default,
                committed_cursor_position_hint: None,
            },
        );
        self.stage_pointer_constraint_install(surface_id, registration.id, registration.region);
        true
    }

    pub(in crate::compositor) fn remove_pointer_constraint(&mut self, constraint_id: u64) {
        let Some((surface_id, committed, surface_constraint_pending, backend_pending_id)) = self
            .pointer_constraint_runtime
            .constraints
            .get(&constraint_id)
            .map(|constraint| {
                (
                    compositor_surface_id(&constraint.surface),
                    constraint.committed,
                    constraint.surface_constraint_pending,
                    constraint
                        .backend_pending
                        .then_some(constraint.backend_id()),
                )
            })
        else {
            return;
        };
        pointer_debug_log(format!(
            "constraint protocol_object_destroyed id={} surface={} committed={} backend_pending={} effective_retirement_pending={}",
            constraint_id,
            surface_id,
            committed,
            backend_pending_id.is_some(),
            committed
        ));
        if let Some(constraint) = self
            .pointer_constraint_runtime
            .constraints
            .get_mut(&constraint_id)
        {
            constraint.locked_resource = None;
            constraint.confined_resource = None;
            constraint.protocol_resource_alive = false;
            if committed && backend_pending_id.is_some() {
                constraint.canceled_backend_activation = true;
            }
        }
        if let Some(backend_id) = backend_pending_id {
            self.cancel_pending_pointer_constraint_backend_requests(backend_id);
        }
        if committed {
            if let Some(constraint) = self
                .pointer_constraint_runtime
                .constraints
                .get_mut(&constraint_id)
            {
                constraint.lifecycle_removal_pending = true;
            }
            self.stage_pointer_constraint_removal(surface_id, constraint_id);
        } else if surface_constraint_pending {
            if let Some(constraint) = self
                .pointer_constraint_runtime
                .constraints
                .get_mut(&constraint_id)
            {
                constraint.surface_constraint_pending = false;
            }
            self.stage_pointer_constraint_cancellation(surface_id, constraint_id);
            self.pointer_constraint_runtime
                .constraints
                .remove(&constraint_id);
        } else {
            self.pointer_constraint_runtime
                .constraints
                .remove(&constraint_id);
        }
    }

    pub(in crate::compositor) fn deactivate_pointer_constraints_for_pointer(
        &mut self,
        pointer: &wl_pointer::WlPointer,
        emit_event: bool,
    ) {
        let ids = self
            .pointer_constraint_runtime
            .constraint_ids_for_pointer(pointer);
        for id in ids {
            self.cancel_pending_locked_pointer_reveal_for_constraint(id, "pointer_destroyed");
            if let Some(surface_id) = self
                .pointer_constraint_runtime
                .constraints
                .get(&id)
                .map(|constraint| compositor_surface_id(&constraint.surface))
            {
                self.pointer_constraint_runtime
                    .pending_pointer_constraint_surface_states
                    .remove(&surface_id);
            }
            if let Some(constraint) = self.pointer_constraint_runtime.constraints.get_mut(&id) {
                constraint.defunct = true;
            }
            self.deactivate_pointer_constraint_by_id(id, true, emit_event, true);
            self.pointer_constraint_runtime.constraints.remove(&id);
        }
    }

    pub(in crate::compositor) fn deactivate_pointer_constraints_for_surface(
        &mut self,
        surface_id: u32,
        emit_event: bool,
    ) {
        self.pointer_constraint_runtime
            .pending_pointer_constraint_surface_states
            .remove(&surface_id);
        let ids = self
            .pointer_constraint_runtime
            .constraints
            .values()
            .filter(|constraint| compositor_surface_id(&constraint.surface) == surface_id)
            .map(|constraint| constraint.id)
            .collect::<Vec<_>>();
        for id in ids {
            self.cancel_pending_locked_pointer_reveal_for_constraint(id, "surface_destroyed");
            if let Some(constraint) = self.pointer_constraint_runtime.constraints.get_mut(&id) {
                constraint.defunct = true;
            }
            self.deactivate_pointer_constraint_by_id(id, true, emit_event, true);
            self.pointer_constraint_runtime.constraints.remove(&id);
        }
    }

    pub(in crate::compositor) fn deactivate_pointer_constraints_for_surface_focus_loss(
        &mut self,
        surface_id: u32,
        emit_event: bool,
    ) {
        let ids = self
            .pointer_constraint_runtime
            .constraints
            .values()
            .filter(|constraint| compositor_surface_id(&constraint.surface) == surface_id)
            .map(|constraint| constraint.id)
            .collect::<Vec<_>>();
        for id in ids {
            self.deactivate_pointer_constraint_by_id(id, true, emit_event, true);
        }
    }

    pub(in crate::compositor) fn deactivate_pointer_constraints_for_departing_window_ids(
        &mut self,
        window_ids: &[WindowId],
        reason: PointerConstraintDeactivationReason,
    ) {
        let departing_root_surface_ids = window_ids
            .iter()
            .filter_map(|window_id| {
                let owner_id = self.workspace_owner_window_id(*window_id)?;
                self.window(owner_id).map(|window| window.root_surface_id)
            })
            .collect::<HashSet<_>>();
        if departing_root_surface_ids.is_empty() {
            return;
        }
        let ids = self
            .pointer_constraint_runtime
            .constraints
            .values()
            .filter(|constraint| {
                departing_root_surface_ids.contains(&self.presentation_owner_root_for_surface(
                    compositor_surface_id(&constraint.surface),
                ))
            })
            .map(|constraint| constraint.id)
            .collect::<Vec<_>>();
        if ids.is_empty() {
            return;
        }
        pointer_debug_log(format!(
            "pointer constraints deactivating for departing windows reason={reason:?} roots={departing_root_surface_ids:?} ids={ids:?}"
        ));
        for id in ids {
            self.deactivate_pointer_constraint_by_id(id, true, true, true);
        }
    }

    pub(in crate::compositor) fn set_pointer_constraint_pending_region(
        &mut self,
        constraint_id: u64,
        region: SurfaceInputRegion,
    ) {
        let Some(surface_id) = self
            .pointer_constraint_runtime
            .constraints
            .get(&constraint_id)
            .filter(|constraint| constraint.protocol_resource_alive && !constraint.defunct)
            .map(|constraint| compositor_surface_id(&constraint.surface))
        else {
            return;
        };
        self.merge_pending_pointer_constraint_surface_state(
            surface_id,
            CapturedPointerConstraintSurfaceState::Mutation(CapturedPointerConstraintCommit {
                constraint_id,
                lifecycle: PointerConstraintLifecycleCommit::NoChange,
                region: PointerConstraintRegionCommit::Set(region),
                cursor_position_hint: PointerConstraintHintCommit::NoChange,
            }),
        );
    }

    pub(in crate::compositor) fn set_pointer_constraint_pending_cursor_position_hint(
        &mut self,
        constraint_id: u64,
        surface_x: f64,
        surface_y: f64,
    ) {
        if !surface_x.is_finite() || !surface_y.is_finite() {
            pointer_debug_log(format!(
                "pointer.lock cursor_hint ignored id={} reason=non_finite hint=({},{})",
                constraint_id, surface_x, surface_y
            ));
            return;
        }
        let Some(surface_id) = self
            .pointer_constraint_runtime
            .constraints
            .get(&constraint_id)
            .filter(|constraint| constraint.protocol_resource_alive && !constraint.defunct)
            .map(|constraint| compositor_surface_id(&constraint.surface))
        else {
            return;
        };
        self.merge_pending_pointer_constraint_surface_state(
            surface_id,
            CapturedPointerConstraintSurfaceState::Mutation(CapturedPointerConstraintCommit {
                constraint_id,
                lifecycle: PointerConstraintLifecycleCommit::NoChange,
                region: PointerConstraintRegionCommit::NoChange,
                cursor_position_hint: PointerConstraintHintCommit::Set((surface_x, surface_y)),
            }),
        );
    }

    pub(in crate::compositor) fn apply_captured_pointer_constraint_surface_state(
        &mut self,
        surface_id: u32,
        state: CapturedPointerConstraintSurfaceState,
    ) {
        if self.pointer_hit_instrumentation_enabled {
            self.pointer_hit_metrics.pointer_constraint_reconciliations += 1;
        }
        let Some(transition) = state.into_transition() else {
            return;
        };
        pointer_debug_log(format!(
            "constraint surface_transition surface={} retire={:?} install={:?}",
            surface_id,
            transition
                .retire
                .as_ref()
                .map(|retirement| retirement.constraint_id),
            transition
                .install_or_update
                .as_ref()
                .map(|mutation| (mutation.constraint_id, mutation.lifecycle))
        ));
        if let Some(retire) = transition.retire {
            self.apply_captured_pointer_constraint_mutation(surface_id, retire);
        }
        if let Some(install_or_update) = transition.install_or_update {
            self.apply_captured_pointer_constraint_mutation(surface_id, install_or_update);
        }
    }

    fn apply_captured_pointer_constraint_mutation(
        &mut self,
        surface_id: u32,
        captured: CapturedPointerConstraintCommit,
    ) {
        let id = captured.constraint_id;
        let Some(constraint) = self.pointer_constraint_runtime.constraints.get(&id) else {
            return;
        };
        if compositor_surface_id(&constraint.surface) != surface_id {
            return;
        }
        let lifecycle = captured.lifecycle;
        if lifecycle == PointerConstraintLifecycleCommit::Cancel {
            let backend_id = self
                .pointer_constraint_runtime
                .constraints
                .get(&id)
                .filter(|constraint| constraint.backend_pending)
                .map(PointerConstraint::backend_id);
            if let Some(backend_id) = backend_id {
                self.cancel_pending_pointer_constraint_backend_requests(backend_id);
            }
            let remove = self
                .pointer_constraint_runtime
                .constraints
                .get(&id)
                .is_some_and(|constraint| !constraint.committed);
            if let Some(constraint) = self.pointer_constraint_runtime.constraints.get_mut(&id) {
                constraint.surface_constraint_pending = false;
                constraint.lifecycle_removal_pending = false;
            }
            if remove {
                self.pointer_constraint_runtime.constraints.remove(&id);
            }
            return;
        }

        if let Some(constraint) = self.pointer_constraint_runtime.constraints.get_mut(&id) {
            if let PointerConstraintRegionCommit::Set(region) = captured.region {
                constraint.committed_region = region;
            }
            if let PointerConstraintHintCommit::Set(hint) = captured.cursor_position_hint {
                constraint.committed_cursor_position_hint = Some(hint);
            }
        }

        match lifecycle {
            PointerConstraintLifecycleCommit::Install => {
                if let Some(constraint) = self.pointer_constraint_runtime.constraints.get_mut(&id) {
                    constraint.committed = true;
                    constraint.surface_constraint_pending = false;
                    constraint.lifecycle_removal_pending = false;
                }
                self.update_active_confined_pointer_region(id, "commit");
                self.maybe_request_pointer_constraint_activation(id);
            }
            PointerConstraintLifecycleCommit::Remove => {
                let Some((was_active, canceled_backend_activation, mode, lifetime, surface, hint)) =
                    self.pointer_constraint_runtime
                        .constraints
                        .get(&id)
                        .map(|constraint| {
                            (
                                constraint.active,
                                constraint.canceled_backend_activation,
                                constraint.mode,
                                constraint.lifetime,
                                constraint.surface.clone(),
                                constraint.committed_cursor_position_hint,
                            )
                        })
                else {
                    return;
                };
                if let Some(constraint) = self.pointer_constraint_runtime.constraints.get_mut(&id) {
                    constraint.committed = false;
                    constraint.surface_constraint_pending = false;
                    constraint.lifecycle_removal_pending = false;
                    constraint.canceled_backend_activation = false;
                    constraint.defunct = true;
                }
                self.deactivate_pointer_constraint_by_id(id, false, false, true);
                if canceled_backend_activation
                    && !was_active
                    && mode == PointerConstraintMode::Locked
                    && lifetime == PointerConstraintLifetime::Oneshot
                    && let Some(position) = self.valid_cursor_hint_output_position(&surface, hint)
                {
                    self.apply_pointer_warp(position, PointerWarpOrigin::OneshotCompatibility);
                }
                if !was_active {
                    self.pointer_constraint_runtime.constraints.remove(&id);
                }
            }
            PointerConstraintLifecycleCommit::NoChange => {
                self.update_active_confined_pointer_region(id, "commit");
                self.maybe_request_pointer_constraint_activation(id);
            }
            PointerConstraintLifecycleCommit::Cancel => unreachable!(),
        }
    }

    pub(in crate::compositor) fn reevaluate_pointer_constraint_activation_for_surface(
        &mut self,
        surface_id: u32,
    ) {
        let ids = self
            .pointer_constraint_runtime
            .constraints
            .values()
            .filter(|constraint| {
                compositor_surface_id(&constraint.surface) == surface_id && !constraint.defunct
            })
            .map(|constraint| constraint.id)
            .collect::<Vec<_>>();
        for id in ids {
            self.update_active_confined_pointer_region(id, "focus");
            self.maybe_request_pointer_constraint_activation(id);
        }
    }

    pub(in crate::compositor) fn pointer_constraint_transition_snapshot(
        &self,
        constraint_id: u64,
    ) -> Option<PointerConstraintTransitionSnapshot> {
        let constraint = self
            .pointer_constraint_runtime
            .constraints
            .get(&constraint_id)?;
        let surface_id = compositor_surface_id(&constraint.surface);
        let captured_hint = |state: &CapturedPointerConstraintSurfaceState| match state {
            CapturedPointerConstraintSurfaceState::Mutation(captured)
                if captured.constraint_id == constraint_id =>
            {
                match &captured.cursor_position_hint {
                    PointerConstraintHintCommit::Set(hint) => Some(*hint),
                    PointerConstraintHintCommit::NoChange => None,
                }
            }
            CapturedPointerConstraintSurfaceState::Transition(transition) => transition
                .install_or_update
                .as_ref()
                .filter(|captured| captured.constraint_id == constraint_id)
                .and_then(|captured| match &captured.cursor_position_hint {
                    PointerConstraintHintCommit::Set(hint) => Some(*hint),
                    PointerConstraintHintCommit::NoChange => None,
                }),
            _ => None,
        };
        let pending_hint = self
            .pointer_constraint_runtime
            .pending_pointer_constraint_surface_states
            .get(&surface_id)
            .and_then(captured_hint)
            .or_else(|| {
                self.surface_transactions
                    .pending_pointer_constraint_hint(surface_id, constraint_id)
            });
        Some(PointerConstraintTransitionSnapshot {
            constraint_id,
            generation: constraint.generation,
            committed_cursor_position_hint: constraint.committed_cursor_position_hint,
            pending_cursor_position_hint: pending_hint,
        })
    }
}
