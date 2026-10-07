use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor::state::pointer_constraints) enum PointerConstraintRestorePolicy {
    HonorCursorPositionHint,
    PreserveCurrentPosition,
}

impl CompositorState {
    pub(in crate::compositor) fn maybe_request_pointer_constraint_activation(
        &mut self,
        constraint_id: u64,
    ) {
        if self.window_interaction.is_some() {
            pointer_debug_log(format!(
                "constraint activation deferred id={} reason=window_interaction",
                constraint_id
            ));
            return;
        }
        let Some((pointer, surface)) = self
            .pointer_constraint_runtime
            .constraints
            .get(&constraint_id)
            .and_then(|constraint| {
                if !constraint.committed
                    || constraint.active
                    || constraint.backend_pending
                    || !constraint.protocol_resource_alive
                    || constraint.defunct
                {
                    return None;
                }
                Some((constraint.pointer.clone(), constraint.surface.clone()))
            })
        else {
            return;
        };
        if !pointer.is_alive() || !surface.is_alive() {
            return;
        }
        let Some(focused) = self.pointer_surface.clone() else {
            return;
        };
        if !resource_belongs_to_surface_client(&pointer, &focused)
            || !resource_belongs_to_surface_client(&pointer, &surface)
            || self.presentation_owner_root_for_surface(compositor_surface_id(&focused))
                != self.presentation_owner_root_for_surface(compositor_surface_id(&surface))
        {
            pointer_debug_log(format!(
                "pointer.constraint activation deferred id={} reason=focus_client_or_root_mismatch focused={} owner={}",
                constraint_id,
                compositor_surface_id(&focused),
                compositor_surface_id(&surface)
            ));
            return;
        }
        if self
            .pointer_constraint_runtime
            .active_backend_constraint
            .is_some()
            || self
                .pointer_constraint_runtime
                .pending_backend_constraint
                .is_some()
        {
            pointer_debug_log(format!(
                "backend activate requested id={} skipped current_active={:?} current_pending={:?}",
                constraint_id,
                self.pointer_constraint_runtime.active_backend_constraint,
                self.pointer_constraint_runtime.pending_backend_constraint
            ));
            return;
        }
        let Some((backend_id, mode)) = self
            .pointer_constraint_runtime
            .constraints
            .get(&constraint_id)
            .map(|constraint| (constraint.backend_id(), constraint.mode))
        else {
            return;
        };
        let request = match mode {
            PointerConstraintMode::Locked => {
                PointerConstraintBackendRequest::ActivateLocked { id: backend_id }
            }
            PointerConstraintMode::Confined => {
                let Some(resolved) =
                    self.pointer_constraint_output_region_with_timing(constraint_id)
                else {
                    pointer_debug_log(format!(
                        "constraint activation skipped id={} reason=region_unresolved mode={:?}",
                        constraint_id, mode
                    ));
                    return;
                };
                let Some(region) = resolved.region else {
                    pointer_debug_log(format!(
                        "constraint activation skipped id={} reason=region_empty mode={:?}",
                        constraint_id, mode
                    ));
                    return;
                };
                PointerConstraintBackendRequest::ActivateConfined {
                    id: backend_id,
                    region,
                    region_resolution_timing: resolved.timing,
                }
            }
            PointerConstraintMode::None => PointerConstraintBackendRequest::Deactivate {
                id: backend_id,
                restore_position: None,
                restore_origin: None,
            },
        };
        let Some(constraint) = self
            .pointer_constraint_runtime
            .constraints
            .get_mut(&constraint_id)
        else {
            return;
        };
        if constraint.active
            || constraint.backend_pending
            || !constraint.protocol_resource_alive
            || constraint.defunct
        {
            return;
        }
        let backend_id = constraint.backend_id();
        self.pointer_constraint_runtime.pending_backend_constraint = Some(backend_id);
        constraint.backend_pending = true;
        pointer_debug_log(format!(
            "constraint activation queued id={} generation={}",
            backend_id.constraint_id, backend_id.generation
        ));
        self.pending_pointer_constraint_backend_requests
            .push(request);
        crate::xwayland::trace::emit("focus_pointer_constraint", || {
            crate::xwayland::trace::TraceFields::new()
                .field("source", "compositor")
                .field("constraint_id", constraint_id)
                .field("surface_id", compositor_surface_id(&surface))
                .field("requested", true)
                .field("active", false)
                .field("focus_generation", self.focus_generation)
        });
    }

    pub(in crate::compositor) fn pointer_constraint_activation_anchor(
        &self,
        constraint_id: u64,
        region: Option<&OutputRegion>,
        current: OutputPosition,
    ) -> Option<OutputPosition> {
        let constraint = self
            .pointer_constraint_runtime
            .constraints
            .get(&constraint_id)?;
        let Some(region) = region else {
            return Some(current);
        };
        if region.closest_point(current) == current {
            return Some(current);
        }
        let owner_root =
            self.presentation_owner_root_for_surface(compositor_surface_id(&constraint.surface));
        if let Some(press) = self.held_pointer_buttons.iter().rev().find(|press| {
            press.root_surface_id == owner_root
                && resource_belongs_to_surface_client(&press.surface, &constraint.surface)
        }) {
            let pressed = OutputPosition {
                x: press.output_x,
                y: press.output_y,
            };
            if region.closest_point(pressed) == pressed {
                return Some(pressed);
            }
        }
        Some(region.closest_point(current))
    }

    pub(in crate::compositor) fn pointer_constraint_output_region(
        &mut self,
        constraint_id: u64,
    ) -> Option<OutputRegion> {
        self.pointer_constraint_output_region_with_timing(constraint_id)
            .and_then(|resolved| resolved.region)
    }

    pub(in crate::compositor) fn pointer_constraint_output_region_with_timing(
        &mut self,
        constraint_id: u64,
    ) -> Option<ResolvedPointerConstraintRegion> {
        let (surface_id, constraint_region, surface_resource) = self
            .pointer_constraint_runtime
            .constraints
            .get(&constraint_id)
            .map(|constraint| {
                (
                    compositor_surface_id(&constraint.surface),
                    constraint.committed_region.clone(),
                    constraint.surface.clone(),
                )
            })?;
        self.refresh_surface_origin_cache();
        let index = self
            .renderable_surfaces
            .iter()
            .position(|renderable| renderable.surface_id == surface_id)?;
        let renderable = &self.renderable_surfaces[index];
        let origin = self.surface_origin_cache.get(index).copied()?;
        let input_region = surface_resource
            .data::<SurfaceData>()
            .map(SurfaceData::committed_input_region_snapshot)?;
        resolve_pointer_constraint_output_region_with_timing(
            &constraint_region,
            &input_region,
            renderable.width,
            renderable.height,
            origin,
        )
    }

    pub(in crate::compositor) fn resolve_pointer_constraint_backend_request(
        &mut self,
        request: PointerConstraintBackendRequest,
        current_position: OutputPosition,
    ) -> Option<ResolvedPointerConstraintBackendRequest> {
        let PointerConstraintBackendRequest::ActivateLocked { id } = request else {
            return Some(ResolvedPointerConstraintBackendRequest {
                request,
                locked_anchor: None,
                region_resolution_timing: None,
            });
        };
        if !self.pointer_constraint_backend_activation_current(id) {
            return None;
        }
        let Some((pointer, surface, mode)) = self
            .pointer_constraint_runtime
            .constraints
            .get(&id.constraint_id)
            .map(|constraint| {
                (
                    constraint.pointer.clone(),
                    constraint.surface.clone(),
                    constraint.mode,
                )
            })
        else {
            self.abort_pointer_constraint_backend_activation(id, "constraint_missing");
            return None;
        };
        if mode != PointerConstraintMode::Locked {
            self.abort_pointer_constraint_backend_activation(id, "mode_changed");
            return None;
        }
        if !pointer.is_alive() || !surface.is_alive() {
            self.abort_pointer_constraint_backend_activation(id, "resource_dead");
            return None;
        }
        let Some(focused) = self.pointer_surface.clone() else {
            self.abort_pointer_constraint_backend_activation(id, "focus_missing");
            return None;
        };
        if !resource_belongs_to_surface_client(&pointer, &focused)
            || !resource_belongs_to_surface_client(&pointer, &surface)
            || self.presentation_owner_root_for_surface(compositor_surface_id(&focused))
                != self.presentation_owner_root_for_surface(compositor_surface_id(&surface))
        {
            self.abort_pointer_constraint_backend_activation(id, "focus_client_or_root_changed");
            return None;
        }
        let Some(resolved_region) =
            self.pointer_constraint_output_region_with_timing(id.constraint_id)
        else {
            self.abort_pointer_constraint_backend_activation(id, "region_unresolved");
            return None;
        };
        let ResolvedPointerConstraintRegion { region, timing } = resolved_region;
        let Some(region) = region.as_ref() else {
            self.abort_pointer_constraint_backend_activation(id, "region_empty");
            return None;
        };
        let Some(anchor) = self.pointer_constraint_activation_anchor(
            id.constraint_id,
            Some(region),
            current_position,
        ) else {
            self.abort_pointer_constraint_backend_activation(id, "anchor_unresolved");
            return None;
        };
        let target = self
            .pointer_target_for_grabbed_surface_at_output(&surface, anchor.x, anchor.y)
            .unwrap_or(PointerTarget {
                surface: surface.clone(),
                surface_x: anchor.x,
                surface_y: anchor.y,
            });
        self.ensure_pointer_focus(&surface);
        if !self.pointer_resource_entered_surface(&pointer, &surface) {
            self.send_pointer_enter_to_resource(&pointer, &target);
        }
        pointer_debug_log(format!(
            "pointer.constraint activation_resolved id={} generation={} cursor=({},{}) anchor=({},{})",
            id.constraint_id,
            id.generation,
            current_position.x,
            current_position.y,
            anchor.x,
            anchor.y
        ));
        Some(ResolvedPointerConstraintBackendRequest {
            request: PointerConstraintBackendRequest::ActivateLocked { id },
            locked_anchor: Some(anchor),
            region_resolution_timing: timing,
        })
    }

    fn abort_pointer_constraint_backend_activation(
        &mut self,
        id: PointerConstraintBackendId,
        reason: &str,
    ) {
        if self.pointer_constraint_runtime.pending_backend_constraint == Some(id) {
            self.pointer_constraint_runtime.pending_backend_constraint = None;
        }
        if let Some(constraint) = self
            .pointer_constraint_runtime
            .constraints
            .get_mut(&id.constraint_id)
            && constraint.generation == id.generation
        {
            constraint.backend_pending = false;
        }
        pointer_debug_log(format!(
            "constraint activation aborted id={} generation={} reason={}",
            id.constraint_id, id.generation, reason
        ));
    }

    pub(in crate::compositor) fn pointer_constraint_backend_activated(
        &mut self,
        id: PointerConstraintBackendId,
        activation_anchor: OutputPosition,
    ) {
        if self.pointer_constraint_runtime.pending_backend_constraint != Some(id) {
            pointer_debug_log(format!(
                "backend activated stale id={:?} current_active={:?} current_pending={:?}",
                id,
                self.pointer_constraint_runtime.active_backend_constraint,
                self.pointer_constraint_runtime.pending_backend_constraint
            ));
            return;
        }
        let activation = {
            let Some(constraint) = self
                .pointer_constraint_runtime
                .constraints
                .get_mut(&id.constraint_id)
            else {
                self.pointer_constraint_runtime.pending_backend_constraint = None;
                return;
            };
            if constraint.generation != id.generation
                || !constraint.committed
                || !constraint.protocol_resource_alive
                || constraint.defunct
            {
                constraint.backend_pending = false;
                self.pointer_constraint_runtime.pending_backend_constraint = None;
                return;
            }
            constraint.backend_pending = false;
            if constraint.active {
                return;
            }
            constraint.active = true;
            self.pointer_constraint_runtime.pending_backend_constraint = None;
            self.pointer_constraint_runtime.active_backend_constraint = Some(id);
            Some((
                constraint.id,
                constraint.generation,
                constraint.mode,
                compositor_surface_id(&constraint.surface),
                constraint.surface.clone(),
                constraint.pointer.clone(),
                constraint.locked_resource.clone(),
                constraint.confined_resource.clone(),
            ))
        };
        let Some((
            constraint_id,
            generation,
            mode,
            surface_id,
            surface,
            pointer,
            locked_resource,
            confined_resource,
        )) = activation
        else {
            return;
        };
        crate::xwayland::trace::emit("focus_pointer_constraint", || {
            crate::xwayland::trace::TraceFields::new()
                .field("source", "backend")
                .field("constraint_id", constraint_id)
                .field("surface_id", surface_id)
                .field("requested", true)
                .field("active", true)
                .field("generation", generation)
        });
        if mode == PointerConstraintMode::Locked {
            if let Some(pending) = self
                .pointer_constraint_runtime
                .pending_locked_pointer_reveal
                .take()
            {
                pointer_debug_log(format!(
                    "pointer.unlock transition_cancel id={} generation={} reason=new_lock",
                    pending.backend_id.constraint_id, pending.backend_id.generation
                ));
            }
            pointer_debug_log(format!(
                "pointer.constraint backend_activated id={} generation={} mode={:?} surface={} pointer={} client={} anchor_output=({},{})",
                id.constraint_id,
                id.generation,
                mode,
                surface_id,
                pointer.id().protocol_id(),
                wayland_resource_client_label(&pointer),
                activation_anchor.x,
                activation_anchor.y
            ));
            self.cursor_visibility.lock_hidden_constraint_id = Some(constraint_id);
            if self.active_client_cursor_has_content() {
                self.advance_render_generation(RenderGenerationCause::CursorState);
            }
            self.sync_cursor_visibility_request();
            let (surface_x, surface_y) = self
                .pointer_target_at(activation_anchor.x, activation_anchor.y)
                .filter(|target| same_surface_resource(&target.surface, &surface))
                .map(|target| (target.surface_x, target.surface_y))
                .unwrap_or((0.0, 0.0));
            self.ensure_pointer_focus(&surface);
            if !self.pointer_resource_entered_surface(&pointer, &surface) {
                let target = PointerTarget {
                    surface: surface.clone(),
                    surface_x,
                    surface_y,
                };
                self.send_pointer_enter_to_resource(&pointer, &target);
            }
            pointer_debug_log(format!(
                "pointer.lock route_active id={} generation={} surface={} pointer={} anchor_output=({},{}) anchor_local=({},{})",
                constraint_id,
                generation,
                compositor_surface_id(&surface),
                pointer.id().protocol_id(),
                activation_anchor.x,
                activation_anchor.y,
                surface_x,
                surface_y
            ));
            self.invalidate_locked_relative_recipient_cache();
            self.pointer_constraint_runtime
                .active_locked_pointer_routing = Some(ActiveLockedPointerRouting {
                constraint_id,
                generation,
                pointer,
                surface,
                surface_x,
                surface_y,
                activation_anchor,
            });
        } else {
            pointer_debug_log(format!(
                "pointer.constraint backend_activated id={} generation={} mode={:?} surface={} pointer={} client={} cursor_output=({},{})",
                id.constraint_id,
                id.generation,
                mode,
                surface_id,
                pointer.id().protocol_id(),
                wayland_resource_client_label(&pointer),
                self.last_pointer_x,
                self.last_pointer_y
            ));
            if mode == PointerConstraintMode::Confined
                && let Some(region) = self.pointer_constraint_output_region(constraint_id)
            {
                let clamped = region.closest_point(OutputPosition {
                    x: self.last_pointer_x,
                    y: self.last_pointer_y,
                });
                self.update_pointer_position(clamped.x, clamped.y);
                let target = self
                    .pointer_target_for_surface_at_output(&surface, clamped.x, clamped.y)
                    .unwrap_or(PointerTarget {
                        surface: surface.clone(),
                        surface_x: 0.0,
                        surface_y: 0.0,
                    });
                self.ensure_pointer_focus(&surface);
                if !self.pointer_resource_entered_surface(&pointer, &surface) {
                    self.send_pointer_enter_to_resource(&pointer, &target);
                }
                pointer_debug_log(format!(
                    "confined route activate id={} surface={} region={:?}",
                    constraint_id,
                    compositor_surface_id(&surface),
                    region.rects
                ));
                self.pointer_constraint_runtime
                    .active_confined_pointer_routing = Some(ActiveConfinedPointerRouting {
                    constraint_id,
                    generation,
                    pointer,
                    surface,
                    region,
                });
            }
        }
        match mode {
            PointerConstraintMode::Locked => {
                if let Some(resource) = &locked_resource
                    && resource.is_alive()
                {
                    resource.locked();
                }
            }
            PointerConstraintMode::Confined => {
                if let Some(resource) = &confined_resource
                    && resource.is_alive()
                {
                    resource.confined();
                }
            }
            PointerConstraintMode::None => {}
        }
    }

    pub(in crate::compositor) fn pointer_constraint_backend_activation_current(
        &self,
        id: PointerConstraintBackendId,
    ) -> bool {
        self.pointer_constraint_runtime.pending_backend_constraint == Some(id)
            && self
                .pointer_constraint_runtime
                .constraints
                .get(&id.constraint_id)
                .is_some_and(|constraint| {
                    constraint.generation == id.generation
                        && constraint.committed
                        && constraint.backend_pending
                        && !constraint.active
                        && constraint.protocol_resource_alive
                        && !constraint.defunct
                })
    }

    pub(in crate::compositor) fn pointer_constraint_backend_failed(
        &mut self,
        id: PointerConstraintBackendId,
        _reason: &str,
    ) {
        if self.pointer_constraint_runtime.pending_backend_constraint == Some(id) {
            self.pointer_constraint_runtime.pending_backend_constraint = None;
        }
        self.cancel_pending_locked_pointer_reveal_for_id(id, "backend_failed");
        let Some(constraint) = self
            .pointer_constraint_runtime
            .constraints
            .get_mut(&id.constraint_id)
        else {
            return;
        };
        if constraint.generation != id.generation {
            return;
        }
        constraint.backend_pending = false;
        if constraint.lifetime == PointerConstraintLifetime::Oneshot {
            constraint.defunct = true;
        }
    }

    pub(in crate::compositor) fn pointer_constraint_backend_deactivated(
        &mut self,
        id: PointerConstraintBackendId,
    ) {
        if self
            .pointer_constraint_runtime
            .active_backend_constraint
            .is_some_and(|current_id| current_id != id)
            || self
                .pointer_constraint_runtime
                .pending_backend_constraint
                .is_some_and(|current_id| current_id != id)
        {
            pointer_debug_log(format!(
                "backend deactivated stale id={id:?} current_active={:?} current_pending={:?}",
                self.pointer_constraint_runtime.active_backend_constraint,
                self.pointer_constraint_runtime.pending_backend_constraint
            ));
            return;
        }
        let pending_matches = self
            .pointer_constraint_runtime
            .pending_locked_pointer_reveal
            .as_ref()
            .is_some_and(|pending| pending.backend_id == id);
        if let Some(current_id) = self
            .pointer_constraint_runtime
            .constraints
            .get(&id.constraint_id)
            .map(PointerConstraint::backend_id)
        {
            if current_id != id {
                pointer_debug_log(format!(
                    "backend deactivated stale id={id:?} current={current_id:?}"
                ));
                return;
            }
        } else if !pending_matches {
            pointer_debug_log(format!(
                "backend deactivated stale id={id:?} reason=unknown"
            ));
            return;
        }
        if self.pointer_constraint_runtime.active_backend_constraint == Some(id) {
            self.pointer_constraint_runtime.active_backend_constraint = None;
        }
        self.deactivate_pointer_constraint_by_id(id.constraint_id, true, true, false);
        self.mark_pending_locked_pointer_backend_settled(id);
        if self
            .pointer_constraint_runtime
            .constraints
            .get(&id.constraint_id)
            .is_some_and(|constraint| {
                constraint.defunct && !constraint.committed && !constraint.active
            })
        {
            self.pointer_constraint_runtime
                .constraints
                .remove(&id.constraint_id);
        }
        self.resume_pending_pointer_constraint_activation();
    }

    pub(in crate::compositor) fn cancel_pending_pointer_constraint_backend_requests(
        &mut self,
        id: PointerConstraintBackendId,
    ) {
        let before = self.pending_pointer_constraint_backend_requests.len();
        self.pending_pointer_constraint_backend_requests
            .retain(|request| {
                !matches!(
                    request,
                    PointerConstraintBackendRequest::ActivateLocked { id: request_id, .. }
                        | PointerConstraintBackendRequest::ActivateConfined {
                            id: request_id,
                            ..
                        }
                        | PointerConstraintBackendRequest::UpdateConfinedRegion {
                            id: request_id,
                            ..
                        } if *request_id == id
                )
            });
        let removed = before - self.pending_pointer_constraint_backend_requests.len();
        if removed > 0 {
            pointer_debug_log(format!(
                "queued activation removed id={} generation={} count={}",
                id.constraint_id, id.generation, removed
            ));
        }
        self.cancel_pending_locked_pointer_reveal_for_id(id, "constraint_backend_work_canceled");
        if self.pointer_constraint_runtime.pending_backend_constraint == Some(id) {
            self.pointer_constraint_runtime.pending_backend_constraint = None;
        }
        if let Some(constraint) = self
            .pointer_constraint_runtime
            .constraints
            .get_mut(&id.constraint_id)
            && constraint.generation == id.generation
        {
            constraint.backend_pending = false;
        }
    }

    pub(in crate::compositor) fn deactivate_pointer_constraint_by_id(
        &mut self,
        constraint_id: u64,
        compositor_driven: bool,
        emit_event: bool,
        queue_backend_deactivate: bool,
    ) {
        self.deactivate_pointer_constraint_by_id_with_restore_policy(
            constraint_id,
            compositor_driven,
            emit_event,
            queue_backend_deactivate,
            PointerConstraintRestorePolicy::HonorCursorPositionHint,
        );
    }

    pub(super) fn deactivate_pointer_constraint_by_id_with_restore_policy(
        &mut self,
        constraint_id: u64,
        compositor_driven: bool,
        emit_event: bool,
        queue_backend_deactivate: bool,
        restore_policy: PointerConstraintRestorePolicy,
    ) {
        self.invalidate_locked_relative_recipient_cache();
        let Some((
            was_active,
            was_pending,
            backend_id,
            mode,
            lifetime,
            surface,
            pointer,
            locked_resource,
            confined_resource,
            cursor_position_hint,
        )) = ({
            let Some(constraint) = self
                .pointer_constraint_runtime
                .constraints
                .get_mut(&constraint_id)
            else {
                return;
            };
            let was_active = constraint.active;
            let was_pending = constraint.backend_pending;
            let backend_id = constraint.backend_id();
            let mode = constraint.mode;
            let lifetime = constraint.lifetime;
            let surface = constraint.surface.clone();
            let pointer = constraint.pointer.clone();
            let locked_resource = constraint.locked_resource.clone();
            let confined_resource = constraint.confined_resource.clone();
            let cursor_position_hint = constraint.committed_cursor_position_hint;
            pointer_debug_log(format!(
                "pointer.unlock request id={} generation={} mode={:?} active={} pending={}",
                constraint.id, constraint.generation, constraint.mode, was_active, was_pending
            ));
            constraint.active = false;
            constraint.backend_pending = false;
            if compositor_driven && constraint.lifetime == PointerConstraintLifetime::Oneshot {
                constraint.defunct = true;
            }
            Some((
                was_active,
                was_pending,
                backend_id,
                mode,
                lifetime,
                surface,
                pointer,
                locked_resource,
                confined_resource,
                cursor_position_hint,
            ))
        })
        else {
            return;
        };
        if was_pending {
            self.cancel_pending_pointer_constraint_backend_requests(backend_id);
        } else if self.pointer_constraint_runtime.pending_backend_constraint == Some(backend_id) {
            self.pointer_constraint_runtime.pending_backend_constraint = None;
        }
        if self.pointer_constraint_runtime.active_backend_constraint == Some(backend_id) {
            self.pointer_constraint_runtime.active_backend_constraint = None;
        }
        let restore_position = if self
            .pointer_constraint_runtime
            .active_locked_pointer_routing
            .as_ref()
            .is_some_and(|active| active.constraint_id == constraint_id)
        {
            let restore_position = if mode == PointerConstraintMode::Locked
                && restore_policy == PointerConstraintRestorePolicy::HonorCursorPositionHint
            {
                self.locked_pointer_release_restore_decision(
                    backend_id,
                    &surface,
                    cursor_position_hint,
                )
            } else {
                None
            };
            if mode == PointerConstraintMode::Locked {
                self.clear_active_locked_pointer_routing();
                self.refresh_pointer_focus_at_last_position();
            }
            restore_position
        } else if self
            .pointer_constraint_runtime
            .active_confined_pointer_routing
            .as_ref()
            .is_some_and(|active| active.constraint_id == constraint_id)
        {
            pointer_debug_log(format!(
                "confined route deactivate id={} reason=constraint_deactivate",
                constraint_id
            ));
            self.clear_active_confined_pointer_routing();
            self.refresh_pointer_focus_at_last_position();
            None
        } else {
            None
        };
        let restore_origin = restore_position
            .is_some()
            .then_some(PointerWarpOrigin::LockedPointerCursorHint);
        if was_active {
            self.invalidate_locked_relative_recipient_cache();
            let locked_unlock_transition = mode == PointerConstraintMode::Locked
                && self.cursor_visibility.lock_hidden_constraint_id == Some(constraint_id);
            if queue_backend_deactivate {
                pointer_debug_log(format!(
                    "backend deactivate queued id={} generation={} reason=constraint_deactivate",
                    backend_id.constraint_id, backend_id.generation
                ));
                self.pending_pointer_constraint_backend_requests.push(
                    PointerConstraintBackendRequest::Deactivate {
                        id: backend_id,
                        restore_position,
                        restore_origin,
                    },
                );
            }
            if emit_event {
                match mode {
                    PointerConstraintMode::Locked => {
                        if let Some(resource) = &locked_resource
                            && resource.is_alive()
                        {
                            resource.unlocked();
                        }
                    }
                    PointerConstraintMode::Confined => {
                        if let Some(resource) = &confined_resource
                            && resource.is_alive()
                        {
                            resource.unconfined();
                        }
                    }
                    PointerConstraintMode::None => {}
                }
            }
            if locked_unlock_transition && pointer.is_alive() && surface.is_alive() {
                self.begin_pending_locked_pointer_reveal(
                    backend_id,
                    pointer,
                    surface.clone(),
                    restore_position,
                    restore_origin,
                );
            }
        } else if was_pending {
            pointer_debug_log(format!(
                "constraint pending activation canceled id={} generation={}",
                backend_id.constraint_id, backend_id.generation
            ));
            if restore_policy == PointerConstraintRestorePolicy::HonorCursorPositionHint
                && mode == PointerConstraintMode::Locked
                && lifetime == PointerConstraintLifetime::Oneshot
                && let Some(position) =
                    self.valid_cursor_hint_output_position(&surface, cursor_position_hint)
            {
                pointer_debug_log(format!(
                    "oneshot compatibility warp selected id={} generation={} output=({},{})",
                    backend_id.constraint_id, backend_id.generation, position.x, position.y
                ));
                self.apply_pointer_warp(position, PointerWarpOrigin::OneshotCompatibility);
            } else if mode == PointerConstraintMode::Locked
                && lifetime == PointerConstraintLifetime::Oneshot
            {
                pointer_debug_log(format!(
                    "oneshot compatibility warp rejected id={} generation={} reason=no_valid_committed_hint",
                    backend_id.constraint_id, backend_id.generation
                ));
            }
        }
        if self.cursor_visibility.lock_hidden_constraint_id == Some(constraint_id)
            && self
                .pointer_constraint_runtime
                .pending_locked_pointer_reveal
                .as_ref()
                .is_none_or(|pending| pending.backend_id.constraint_id != constraint_id)
        {
            self.cursor_visibility.lock_hidden_constraint_id = None;
            if self.active_client_cursor_has_content() {
                self.advance_render_generation(RenderGenerationCause::CursorState);
            }
            self.sync_cursor_visibility_request();
        }
    }

    pub(in crate::compositor) fn take_pointer_constraint_backend_requests(
        &mut self,
    ) -> Vec<PointerConstraintBackendRequest> {
        std::mem::take(&mut self.pending_pointer_constraint_backend_requests)
    }

    #[allow(dead_code)] // Used by the native-output binary; the library target omits that runtime.
    pub(in crate::compositor) fn pointer_constraint_backend_request_count(&self) -> usize {
        self.pending_pointer_constraint_backend_requests.len()
    }
}
